//! RPC サーバ実装。
//!
//! 1 Named Pipe インスタンス = 1 クライアント接続。
//! クライアント接続ごとに 1 スレッドを spawn し、そのスレッド内で
//! `DynEngine` を排他的に使ってリクエストに応答する。
//!
//! # エンジン共有方針（Phase A 初期）
//! エンジンインスタンスは **グローバル 1 個** を `Mutex<DynEngine>` で共有する。
//! llama 推論は逐次なのでシリアル化で問題にならない。
//! セッションごとに別エンジンを作ると model/dict のロードが多重化して
//! VRAM/メモリを浪費するため避ける。
//!
//! セッション間の hiragana_buf 等の汚染は TSF 側が既に `ResetAll` を
//! フォーカス変化で呼ぶ前提でカバーする。

use crate::protocol::{BgStartOutcome, BgTakeOutcome, BgView, EditState, ReadRequest};
use rakukan_engine_abi::BgSlot;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use rakukan_engine_abi::{BgRunState, DynEngine, StallProbe};

use crate::codec::{read_frame, write_frame};
use crate::health::{self, Action, Health, HealthTracker, RecoveryMarker, RecoveryReason};
use crate::pipe::{PipeStream, pipe_name_for_current_user};
use crate::protocol::{
    ChangeOutcome, ChangeRequest, EngineGen, Expect, HostId, InputCharKind, Owner,
    PROTOCOL_VERSION, Reason, Request, RequestSeq, Response, TsfId,
};

/// ホストが保持する TSF ごとの直近記録（Issue #56）。線路には流さない。
///
/// 結果の記録（`last_seq` / `last_response` / `floor`）と接続管理
/// （`sessions` / `in_flight` / `last_used`）を 1 件にまとめて持つ。`Hello` で
/// 作られたばかりの記録は接続管理だけを持ち、`floor` 以下の番号には結果を答えない。
#[derive(Debug, Clone)]
pub(crate) struct RequestRecord {
    /// 最後に判定した要求番号（適用・拒否のどちらでも記録する）。
    pub last_seq: Option<RequestSeq>,
    /// `last_seq` の要求に返した応答。同じ番号の再送にはこれを返す。
    pub last_response: Option<Response>,
    /// この番号以下の要求は、適用済みかを答えられないので拒否する。
    /// 記録を作ったときの `Hello.highest_sent`。
    pub floor: RequestSeq,
    /// この TSF インスタンスの生きている接続の数。
    pub sessions: u32,
    /// 読み込んでから `dispatch` を抜けるまでの要求の数（切断後も数える）。
    pub in_flight: u32,
    /// 最後に触れた時刻（記録表の中の論理時計）。回収の順序に使う。
    pub last_used: u64,
}

/// 記録表の上限。超えたら回収できる記録（接続も処理中の要求も無いもの）を
/// 古い順に捨てる。回収できる記録が無ければ上限を超えて受け付ける。
const RECORD_CAPACITY: usize = 256;

/// 要求番号の判定結果。
#[derive(Debug, Clone)]
enum SeqCheck {
    /// 前回と同じ番号。再適用せず、当時の応答を返す。
    Replay(Response),
    /// 記録より小さい番号、または `floor` 以下。適用済みかを答えられない。
    Unavailable,
    /// 新しい番号。
    New,
}

/// TSF インスタンスごとの記録の表（Issue #56）。
///
/// ロックの順序は `state` → 記録表。記録表を保持したまま `state` を取らないこと。
#[derive(Debug, Default)]
pub(crate) struct RecordTable {
    records: HashMap<TsfId, RequestRecord>,
    clock: u64,
    /// 上限を超えても回収できなかったことを警告済みか（超えている間は 1 回だけ出す）。
    warned_over_capacity: bool,
}

impl RecordTable {
    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// `Hello` で接続を登録する。記録があれば `true`。
    ///
    /// 記録が無ければ（初回、または回収済み）、`floor = highest_sent` で新しく作る。
    /// 登録・記録の有無の確認・回収は、このロックの中でまとめて行う。
    fn hello(&mut self, tsf: TsfId, highest_sent: RequestSeq) -> bool {
        let now = self.tick();
        let found = match self.records.get_mut(&tsf) {
            Some(r) => {
                r.sessions += 1;
                r.last_used = now;
                true
            }
            None => {
                self.records.insert(
                    tsf,
                    RequestRecord {
                        last_seq: None,
                        last_response: None,
                        floor: highest_sent,
                        sessions: 1,
                        in_flight: 0,
                        last_used: now,
                    },
                );
                false
            }
        };
        self.collect();
        found
    }

    fn session_ended(&mut self, tsf: TsfId) {
        if let Some(r) = self.records.get_mut(&tsf) {
            r.sessions = r.sessions.saturating_sub(1);
        }
    }

    fn begin_request(&mut self, tsf: TsfId) {
        let now = self.tick();
        if let Some(r) = self.records.get_mut(&tsf) {
            r.in_flight += 1;
            r.last_used = now;
        }
    }

    fn end_request(&mut self, tsf: TsfId) {
        if let Some(r) = self.records.get_mut(&tsf) {
            r.in_flight = r.in_flight.saturating_sub(1);
        }
    }

    /// 要求番号を記録と照らし合わせる。記録が無ければ（`Hello` を経ていない）
    /// 答えられないものとして扱う。
    fn check(&self, tsf: TsfId, seq: RequestSeq) -> SeqCheck {
        let Some(r) = self.records.get(&tsf) else {
            return SeqCheck::Unavailable;
        };
        if let Some(last) = r.last_seq {
            if seq == last {
                return match &r.last_response {
                    Some(resp) => SeqCheck::Replay(resp.clone()),
                    None => SeqCheck::Unavailable,
                };
            }
            if seq < last {
                return SeqCheck::Unavailable;
            }
        }
        if seq <= r.floor {
            return SeqCheck::Unavailable;
        }
        SeqCheck::New
    }

    /// 判定した要求の番号と応答を記録する。前の番号の記録はここで捨てる
    /// （同じ TSF から次の番号が来た時点で、前の番号の再送はもう起こらない）。
    fn store(&mut self, tsf: TsfId, seq: RequestSeq, resp: &Response) {
        let now = self.tick();
        if let Some(r) = self.records.get_mut(&tsf) {
            r.last_seq = Some(seq);
            r.last_response = Some(resp.clone());
            r.last_used = now;
        }
    }

    /// 上限を超えていれば、接続も処理中の要求も無い記録を古い順に捨てる。
    fn collect(&mut self) {
        if self.records.len() <= RECORD_CAPACITY {
            self.warned_over_capacity = false;
            return;
        }
        let mut idle: Vec<(u64, TsfId)> = self
            .records
            .iter()
            .filter(|(_, r)| r.sessions == 0 && r.in_flight == 0)
            .map(|(id, r)| (r.last_used, *id))
            .collect();
        idle.sort_unstable_by_key(|(used, _)| *used);
        let excess = self.records.len() - RECORD_CAPACITY;
        for (_, id) in idle.into_iter().take(excess) {
            self.records.remove(&id);
        }
        if self.records.len() > RECORD_CAPACITY {
            if !self.warned_over_capacity {
                tracing::warn!(
                    "rpc: request records over capacity ({} > {RECORD_CAPACITY}); all remaining records have live sessions or in-flight requests",
                    self.records.len()
                );
                self.warned_over_capacity = true;
            }
        } else {
            self.warned_over_capacity = false;
        }
    }
}

/// `Change` を適用してよいかの判定（Issue #56）。
#[derive(Debug, Clone)]
enum Gate {
    /// 記録を変えずに、この応答を返す。
    Respond(Response),
    /// 適用せず拒否する。番号は判定済みとして記録する。
    Reject(Reason),
    /// 適用する。
    Apply,
}

/// 判定の順序は「世代 → 要求番号 → 所有者」（9/18 の訂正どおり）。
///
/// - 世代が違えば記録に触れずに `GenMismatch`（この世代の記録ではないため）
/// - 番号が前回と同じなら、所有者が今は違っていても当時の応答を返す
///   （所有権の移動では適用済みの記録を捨てない）
/// - 新しい番号で所有者が違えば `OwnerMismatch`。所有者がいない（エンジンを
///   作ったばかり）場合も同じ。所有権は `Restore` でだけ移る
fn gate_change(
    current_gen: Option<EngineGen>,
    current_owner: Option<Owner>,
    expect: &Expect,
    seq: SeqCheck,
) -> Gate {
    if current_gen != Some(expect.engine_gen) {
        return Gate::Respond(Response::Rejected(Reason::GenMismatch));
    }
    match seq {
        SeqCheck::Replay(resp) => Gate::Respond(resp),
        SeqCheck::Unavailable => Gate::Respond(Response::Rejected(Reason::ResultUnavailable)),
        SeqCheck::New if current_owner != Some(expect.owner) => Gate::Reject(Reason::OwnerMismatch),
        SeqCheck::New => Gate::Apply,
    }
}

/// 応答を返した後にホストを終了するか。要求ではなく応答から決める（Issue #56）。
///
/// `ShutdownAccepted` は `Shutdown` が自分宛てだったときだけ返る。`Bool(true)` は
/// 他の要求でも返るので、`ShutdownIfConfigDiffers` の応答のときだけ終了とみなす。
fn exits_after_response(is_conditional_shutdown: bool, resp: &Response) -> bool {
    match resp {
        Response::ShutdownAccepted => true,
        Response::Bool(true) => is_conditional_shutdown,
        _ => false,
    }
}

/// ホスト全体で共有される 1 つの DynEngine と、その生成に使った config。
pub type SharedEngine = Arc<HostShared>;

pub struct HostShared {
    /// このホストプロセスの起動インスタンス識別子（Issue #56）。起動ごとに採り直す。
    host_id: HostId,
    /// このホストで作ったエンジンの数。世代の番号に使う。
    engines_created: AtomicU64,
    /// 現在のエンジンの世代の写し。`Hello` はエンジンのロックを取らずにこれを返す
    /// （変換中でも応答できるように）。書き換えは `state` を保持したまま行う。
    engine_gen_now: Mutex<Option<EngineGen>>,
    /// TSF インスタンスごとの記録（Issue #56）。ロックの順序は `state` → `records`。
    records: Mutex<RecordTable>,
    /// エンジン本体。変換中はこのロックが長時間（最大 GEN_TIMEOUT 秒）保持される。
    pub state: Mutex<SharedEngineState>,
    /// 現在の engine 生成に使った config JSON。
    ///
    /// `state` とは別ロックにする: `ShutdownIfConfigDiffers` は変換で engine
    /// ロックが塞がっていても即応答できる必要がある（`Shutdown` が engine
    /// ロックなしで動くのと同じ理由）。ロックは比較・更新の瞬間だけ保持する。
    pub config_json: Mutex<Option<String>>,
    /// 推論の即時失敗を数え、復帰の段階を進める（Issue #43）。
    ///
    /// `state` とは別ロックにする: 変換中（engine ロック保持中）でも
    /// `EngineHealth` に即応答できる必要がある。
    health: Mutex<HealthTracker>,
    /// 復帰のためにホストを終了する要求。応答を書いた後に見る。
    exit_after_response: AtomicBool,
    /// 変換の詰まりを監視する口（Issue #57）。エンジンを作るたびに持ち替える。
    ///
    /// `state` とは別ロックにする: 監視スレッドは変換中（engine ロック保持中）でも
    /// 状態を読めなければならない。ロックは口を複製する瞬間か、詰まりを確定
    /// させる瞬間だけ保持する。
    ///
    /// **ロックの順序は `stall_probe` → `health`。** 逆向き（`health` を保持した
    /// まま `stall_probe` を取る）は作らないこと。
    stall_probe: Mutex<StallProbeSlot>,
}

/// 監視する口と、その世代（Issue #57）。
///
/// 口を持ち替える・取り外すたびに世代が 1 つ進む。世代が無いと、DLL の variant が
/// 変わって新しい `CACHE` が実行番号を 0 から数え直したときに、「この実行番号は
/// もう処理した」という監視スレッド側の記憶が誤って効き、詰まりを見逃す
/// （`acted == Some(5)` のまま新しい DLL の run 5 が詰まる場合）。
#[derive(Default)]
struct StallProbeSlot {
    /// 持ち替え・取り外しのたびに 1 つ進む。
    generation: u64,
    /// エンジンを外している間は `None`。
    probe: Option<StallProbe>,
}

impl HostShared {
    pub fn new() -> Self {
        // 直近に自己終了しているかをマーカーから読む（Issue #43）。
        let marker = health::load_marker();
        let prior = health::prior_attempts(marker, health::now_ms());
        if prior > 0 {
            let reason = marker.map_or(RecoveryReason::Unknown, |m| m.reason);
            tracing::warn!(
                "recovery marker found: prior self-exit attempts={prior} reason={}",
                reason.as_str()
            );
        }
        let host_id = HostId::random();
        tracing::info!("engine host: host_id={:032x}", host_id.0);
        Self {
            host_id,
            engines_created: AtomicU64::new(0),
            engine_gen_now: Mutex::new(None),
            records: Mutex::new(RecordTable::default()),
            state: Mutex::new(SharedEngineState::default()),
            config_json: Mutex::new(None),
            health: Mutex::new(HealthTracker::new(prior)),
            exit_after_response: AtomicBool::new(false),
            stall_probe: Mutex::new(StallProbeSlot::default()),
        }
    }

    /// 変換の詰まりを 1 回観測し、ホストが取るべき動作と、自己終了するときに
    /// マーカーへ書く試行回数を返す（Issue #57）。
    fn health_observe_stall(&self) -> (Action, u32) {
        let mut g = match self.health.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let action = g.observe_stall();
        (action, g.next_attempt())
    }

    fn lock_stall_probe(&self) -> std::sync::MutexGuard<'_, StallProbeSlot> {
        match self.stall_probe.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    /// 監視する DLL を、新しく作ったエンジンの DLL へ持ち替える。
    ///
    /// 同じ DLL を読み直した場合は OS 上は同じモジュール（変換キャッシュも同じ）
    /// なので、持ち替えても実行番号は続きから数えられる。それでも世代は進める:
    /// variant が変わって実行番号が 0 から数え直しになる場合と区別できないため。
    fn set_stall_probe(&self, probe: StallProbe) {
        let mut g = self.lock_stall_probe();
        g.generation += 1;
        g.probe = Some(probe);
    }

    /// エンジンを外したので監視も止める（Issue #57）。
    ///
    /// 口を残すと、古い DLL のワーカーが `Running` のまま残っていた場合に、
    /// その観測を根拠に現在のホストを終了させてしまう。世代も進めるので、
    /// 既に複製された口で進行中の確認も無効になる。
    fn clear_stall_probe(&self) {
        let mut g = self.lock_stall_probe();
        if g.probe.is_some() {
            g.generation += 1;
            g.probe = None;
        }
    }

    /// 現在の口と、その世代。
    fn stall_probe(&self) -> Option<(u64, StallProbe)> {
        let g = self.lock_stall_probe();
        g.probe.clone().map(|p| (g.generation, p))
    }

    /// 世代 `generation` の口がまだ現在なら、口のロックを保持したまま `f` を
    /// 実行する（Issue #57）。持ち替わっていれば `None`。
    ///
    /// `set_stall_probe` / `clear_stall_probe` は同じロックを取るので、`f` の
    /// 実行中に口が入れ替わることはない＝「確認に使った口が、終了を決める
    /// 時点でも現在のものである」ことが保証される。
    ///
    /// `f` の中から `health` を取るのは順序どおり（`stall_probe` → `health`）。
    fn with_current_probe<R>(&self, generation: u64, f: impl FnOnce() -> R) -> Option<R> {
        let g = self.lock_stall_probe();
        if g.generation != generation || g.probe.is_none() {
            return None;
        }
        Some(f())
    }

    /// `bg_status()` を 1 件観測し、ホストが取るべき動作を返す（Issue #43）。
    fn health_observe(&self, status: &str) -> Action {
        match self.health.lock() {
            Ok(mut g) => g.observe(status),
            Err(p) => p.into_inner().observe(status),
        }
    }

    /// 現在の健全性。`EngineHealth` の応答に使う。
    fn health_now(&self) -> Health {
        match self.health.lock() {
            Ok(g) => g.health(),
            Err(p) => p.into_inner().health(),
        }
    }

    /// 次に自己終了するときマーカーへ書く試行回数。
    fn next_attempt(&self) -> u32 {
        match self.health.lock() {
            Ok(g) => g.next_attempt(),
            Err(p) => p.into_inner().next_attempt(),
        }
    }

    fn request_exit(&self) {
        self.exit_after_response.store(true, Ordering::Release);
    }

    fn take_exit_request(&self) -> bool {
        self.exit_after_response.swap(false, Ordering::AcqRel)
    }

    /// config_json の現在値を短時間ロックで複製する。poisoned は回復する。
    fn config_snapshot(&self) -> Option<String> {
        match self.config_json.lock() {
            Ok(g) => g.clone(),
            Err(p) => p.into_inner().clone(),
        }
    }

    /// config_json を短時間ロックで更新する。poisoned は回復する。
    fn set_config(&self, cfg: Option<String>) {
        match self.config_json.lock() {
            Ok(mut g) => *g = cfg,
            Err(p) => *p.into_inner() = cfg,
        }
    }

    fn lock_records(&self) -> std::sync::MutexGuard<'_, RecordTable> {
        match self.records.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn engine_gen_now(&self) -> Option<EngineGen> {
        match self.engine_gen_now.lock() {
            Ok(g) => *g,
            Err(p) => *p.into_inner(),
        }
    }

    /// エンジンを入れ替えた・外したときに呼ぶ。`slot` のロックを保持したまま呼ぶこと。
    ///
    /// 新しいエンジンは読みが空なので、所有者も BG 変換の記録も持たない。
    fn replace_engine_gen(&self, slot: &mut SharedEngineState, has_engine: bool) {
        let engine_gen = has_engine.then(|| EngineGen {
            host_id: self.host_id,
            generation: self.engines_created.fetch_add(1, Ordering::Relaxed) + 1,
        });
        slot.engine_gen = engine_gen;
        slot.owner = None;
        slot.bg_owner = None;
        match self.engine_gen_now.lock() {
            Ok(mut g) => *g = engine_gen,
            Err(p) => *p.into_inner() = engine_gen,
        }
    }
}

impl Default for HostShared {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Default)]
pub struct SharedEngineState {
    pub engine: Option<DynEngine>,
    /// 現在のエンジンの世代（Issue #56）。エンジンが無ければ `None`。
    pub engine_gen: Option<EngineGen>,
    /// 共有エンジンの編集状態を持っている composition。`Restore` でだけ移る。
    pub owner: Option<Owner>,
    /// 実行中・完了済みの BG 変換を開始した composition。`Change` で開始した
    /// ときだけ記録し、`Restore` で消す。候補の取り出しはこの所有者にだけ許す
    /// （キーの一致だけでは、別 composition の同じ読みの結果を拾いうるため）。
    pub bg_owner: Option<Owner>,
}

/// 接続 1 本ぶんの状態。
#[derive(Debug, Default)]
struct Session {
    /// `Hello` で登録した TSF インスタンス。
    tsf: Option<TsfId>,
}

/// 要求 1 件を処理している間、記録の `in_flight` を数える（切断後も、
/// `dispatch` を抜けるまでは記録を回収させない）。
struct InFlight<'a> {
    engine: &'a HostShared,
    tsf: Option<TsfId>,
}

impl<'a> InFlight<'a> {
    fn begin(engine: &'a HostShared, tsf: Option<TsfId>) -> Self {
        if let Some(t) = tsf {
            engine.lock_records().begin_request(t);
        }
        Self { engine, tsf }
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        if let Some(t) = self.tsf {
            self.engine.lock_records().end_request(t);
        }
    }
}

/// Named Pipe サーバを起動し、クライアント接続を待ち受けるループを実行する。
///
/// この関数はブロッキングで走り続ける。通常は `rakukan-engine-host` のメインスレッドから呼ぶ。
pub fn serve(engine: SharedEngine) -> Result<()> {
    let pipe_name = pipe_name_for_current_user();
    tracing::info!("engine host: listening on {pipe_name}");
    spawn_stall_watchdog(engine.clone());
    loop {
        let stream = PipeStream::create_server(&pipe_name)
            .with_context(|| format!("create server pipe {pipe_name}"))?;
        if let Err(e) = stream.accept() {
            tracing::warn!("accept failed: {e}");
            continue;
        }
        let engine_c = engine.clone();
        std::thread::Builder::new()
            .name("rakukan-engine-rpc-session".into())
            .spawn(move || {
                if let Err(e) = handle_session(stream, engine_c) {
                    tracing::warn!("session ended with error: {e}");
                }
            })
            .ok();
    }
}

fn handle_session(stream: PipeStream, engine: SharedEngine) -> Result<()> {
    tracing::debug!("rpc session: started");
    let mut session = Session::default();
    let result = session_loop(stream, &engine, &mut session);
    if let Some(tsf) = session.tsf {
        engine.lock_records().session_ended(tsf);
    }
    result
}

fn session_loop(
    mut stream: PipeStream,
    engine: &SharedEngine,
    session: &mut Session,
) -> Result<()> {
    loop {
        let req: Request = match read_frame(&mut stream) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("rpc session: read_frame failed, closing: {e}");
                return Ok(());
            }
        };
        // ShutdownIfConfigDiffers は「config が異なる」と判定したとき（Bool(true)
        // 応答）だけ Shutdown と同じ exit 経路に乗る。終了するかは応答から決める
        // （Issue #56: 別のホスト宛ての Shutdown は ShutdownSkipped になり、終了しない）。
        let is_conditional_shutdown = matches!(req, Request::ShutdownIfConfigDiffers { .. });
        let label = request_label(&req);
        let started = std::time::Instant::now();
        let resp = {
            let _in_flight = InFlight::begin(engine, session.tsf);
            dispatch(engine, session, req)
        };
        let is_shutdown = exits_after_response(is_conditional_shutdown, &resp);
        // 変換遅延の診断: 長くブロックした要求だけを INFO で残す。
        // BgWaitMs はクライアント指定のタイムアウトまで待つのが正常動作なので
        // 1 秒以上（= engine mutex 待ち等の異常）に絞ってノイズを避ける。
        let elapsed_ms = started.elapsed().as_millis();
        if elapsed_ms >= 1_000 {
            tracing::info!("rpc: {label} took {elapsed_ms}ms");
        }
        if let Err(e) = write_frame(&mut stream, &resp) {
            tracing::debug!("rpc session: write_frame failed, closing: {e}");
            return Ok(());
        }
        // 復帰のための自己終了（Issue #43）。応答を返し切ってから落ちる。
        if engine.take_exit_request() {
            let attempt = engine.next_attempt();
            health::write_marker(RecoveryMarker {
                exited_at_ms: health::now_ms(),
                attempt,
                reason: RecoveryReason::InferenceFailed,
            });
            std::thread::sleep(Duration::from_millis(50));
            tracing::warn!("rpc: exiting host for recovery (attempt={attempt})");
            std::process::exit(0);
        }
        if is_shutdown {
            // OS にパイプ経由の response を配送させるため短時間待ってから exit。
            // flush は write_frame 内で完了しているが、pipe buffer から相手の read
            // までの伝播はカーネルスケジューリング依存。50ms で十分安全側に倒れる。
            std::thread::sleep(Duration::from_millis(50));
            tracing::info!("rpc: Shutdown requested, exiting host process");
            std::process::exit(0);
        }
    }
}

/// ログ用のリクエスト名（payload は含めない）。
pub(crate) fn request_label(req: &Request) -> &'static str {
    use Request::*;
    match req {
        Hello { .. } => "Hello",
        Create { .. } => "Create",
        Reload { .. } => "Reload",
        Bye => "Bye",
        Shutdown { .. } => "Shutdown",
        _ReservedPushChar(_) => "PushChar",
        _ReservedPushRaw(_) => "PushRaw",
        _ReservedPushFullwidthAlpha(_) => "PushFullwidthAlpha",
        _ReservedBackspace => "Backspace",
        _ReservedFlushPendingN => "FlushPendingN",
        _ReservedPreeditDisplay => "PreeditDisplay",
        _ReservedPreeditIsEmpty => "PreeditIsEmpty",
        _ReservedHiraganaText => "HiraganaText",
        _ReservedRomajiLogStr => "RomajiLogStr",
        _ReservedHiraganaFromRomajiLog => "HiraganaFromRomajiLog",
        _ReservedCommittedText => "CommittedText",
        _ReservedBgStart { .. } => "BgStart",
        _ReservedBgStatus => "BgStatus",
        _ReservedBgTakeCandidates { .. } => "BgTakeCandidates",
        _ReservedBgPeekTopCandidate { .. } => "BgPeekTopCandidate",
        #[allow(deprecated)]
        _ReservedBgTakeSegmentedCandidates { .. } => "_Reserved",
        _ReservedBgReclaim => "BgReclaim",
        _ReservedBgWaitMs { .. } => "BgWaitMs",
        _ReservedCommit { .. } => "Commit",
        _ReservedCommitAsHiragana => "CommitAsHiragana",
        _ReservedResetPreedit => "ResetPreedit",
        _ReservedForcePreedit { .. } => "ForcePreedit",
        _ReservedResetAll => "ResetAll",
        _ReservedConvertSync => "ConvertSync",
        #[allow(deprecated)]
        _ReservedConvertSyncSegmented => "_Reserved",
        #[allow(deprecated)]
        _ReservedMergeCandidates { .. } => "MergeCandidates(removed)",
        #[allow(deprecated)]
        _ReservedSegmentSurface { .. } => "_Reserved",
        #[allow(deprecated)]
        _ReservedSegmentCandidate { .. } => "_Reserved",
        #[allow(deprecated)]
        _ReservedConvertToSegments { .. } => "_Reserved",
        ResizeSegment { .. } => "ResizeSegment",
        SegmentCandidatesFor { .. } => "SegmentCandidatesFor",
        StartLoadModel => "StartLoadModel",
        PollModelReady => "PollModelReady",
        StartLoadDict => "StartLoadDict",
        PollDictReady => "PollDictReady",
        IsKanjiReady => "IsKanjiReady",
        IsDictReady => "IsDictReady",
        BackendLabel => "BackendLabel",
        NGpuLayers => "NGpuLayers",
        MainGpu => "MainGpu",
        AvailableModelsJson => "AvailableModelsJson",
        _ReservedLearn { .. } => "Learn",
        _ReservedLearnForce { .. } => "LearnForce",
        MergeCandidatesForReading { .. } => "MergeCandidatesForReading",
        LastError => "LastError",
        DictStatus => "DictStatus",
        EngineHealth => "EngineHealth",
        _ReservedInputChar { .. } => "InputChar",
        ShutdownIfConfigDiffers { .. } => "ShutdownIfConfigDiffers",
        Change { .. } => "Change",
        Restore { .. } => "Restore",
        Read { .. } => "Read",
    }
}

fn dispatch(engine: &SharedEngine, session: &mut Session, req: Request) -> Response {
    // Hello / Create は handle し、残りは DynEngine メソッドに流す
    match req {
        Request::Hello {
            protocol_version,
            tsf_id,
            highest_sent,
        } => {
            if protocol_version != PROTOCOL_VERSION {
                return Response::Error(format!(
                    "protocol version mismatch: client={protocol_version} server={PROTOCOL_VERSION}"
                ));
            }
            let record_found = {
                let mut records = engine.lock_records();
                if let Some(prev) = session.tsf.replace(tsf_id) {
                    records.session_ended(prev);
                }
                records.hello(tsf_id, highest_sent)
            };
            if !record_found && highest_sent > 0 {
                // 記録を回収された TSF が戻ってきた、または新しいホストへ繋ぎ直した。
                tracing::info!(
                    "rpc: Hello from tsf {:032x} without a record; floor={highest_sent}",
                    tsf_id.0
                );
            }
            Response::Hello {
                protocol_version: PROTOCOL_VERSION,
                host_id: engine.host_id,
                engine_gen: engine.engine_gen_now(),
                record_found,
            }
        }
        Request::Create { config_json, .. } => {
            let mut g = lock_engine(engine);
            if g.engine.is_some() && engine.config_snapshot() == config_json {
                return Response::Unit;
            }
            if g.engine.is_some() {
                tracing::info!(
                    "rpc: Create requested with changed config, reloading current engine"
                );
            }
            load_engine_into(engine, &mut g, config_json)
        }
        Request::Reload { config_json } => {
            // 既存 engine を drop してから作り直す。
            // config.toml 編集後のモード切替から呼ばれる。
            let mut g = lock_engine(engine);
            tracing::info!("rpc: Reload requested, dropping current engine");
            g.engine = None;
            engine.replace_engine_gen(&mut g, false);
            // エンジンを外す時点で監視も止める。ロードに失敗した場合に、
            // 古い DLL の `Running` を根拠に現在のホストを終了させないため
            // （成功すれば load_engine_into が新しい口を入れ直す）。
            engine.clear_stall_probe();
            load_engine_into(engine, &mut g, config_json)
        }
        Request::Bye => Response::Unit,
        Request::Change {
            seq,
            expect,
            request,
            config_version: _,
        } => dispatch_change(engine, session, seq, expect, request),
        Request::Restore {
            owner,
            seq,
            reading,
            pending_romaji,
            then,
            config_version: _,
        } => dispatch_restore(engine, session, owner, seq, reading, pending_romaji, then),
        Request::Read { expect, request } => dispatch_read(engine, session, expect, request),
        // host_id はホストの不変フィールドなので、engine ロックを取らずに答えられる
        // （変換中でも応答できる）。
        Request::Shutdown { expected_host_id } => {
            if expected_host_id == engine.host_id {
                Response::ShutdownAccepted
            } else {
                tracing::info!(
                    "rpc: Shutdown for another host ({:032x}); keeping this host",
                    expected_host_id.0
                );
                Response::ShutdownSkipped
            }
        }
        // 変換中（engine ロック保持中）でも即答する必要があるので engine を取らない。
        Request::EngineHealth => Response::String(engine.health_now().as_str().to_string()),
        Request::ShutdownIfConfigDiffers {
            config_json,
            expected_host_id,
            config_version: _,
        } => {
            // 宛先のホストは既に入れ替わっている。新しいホストの設定と比べると、
            // 「同じ設定」と「別のホストだった」を区別できなくなるので比較しない。
            if expected_host_id != engine.host_id {
                tracing::info!(
                    "rpc: ShutdownIfConfigDiffers for another host ({:032x}); keeping this host",
                    expected_host_id.0
                );
                return Response::ShutdownSkipped;
            }
            // 変換中でも応答できるよう engine ロックは取らない（Shutdown と同じ扱い）。
            // config だけを短時間ロックで比較する。
            let current = engine.config_snapshot();
            if current == config_json {
                tracing::info!("rpc: ShutdownIfConfigDiffers: config unchanged, keeping host");
                Response::Bool(false)
            } else {
                tracing::info!("rpc: ShutdownIfConfigDiffers: config differs, will exit");
                Response::Bool(true)
            }
        }
        other => {
            let mut g = match engine.state.lock() {
                Ok(g) => g,
                Err(p) => {
                    tracing::warn!("engine mutex poisoned, recovering");
                    p.into_inner()
                }
            };
            let Some(eng) = g.engine.as_mut() else {
                return Response::Error("engine not created".into());
            };
            let resp = dispatch_engine(eng, other);
            apply_health_action(engine, eng);
            resp
        }
    }
}

/// 要求の `owner.tsf_id` が、この接続の `Hello` で登録した TSF と一致するか。
// 拒否の応答をそのまま Err で返す（Box に包まない）。
#[allow(clippy::result_large_err)]
fn session_tsf(session: &Session, owner: &Owner) -> std::result::Result<TsfId, Response> {
    match session.tsf {
        Some(t) if t == owner.tsf_id => Ok(t),
        Some(_) => Err(Response::Error(
            "owner.tsf_id does not match the tsf_id sent in Hello".into(),
        )),
        None => Err(Response::Error("Hello has not been exchanged".into())),
    }
}

/// 採番された変更要求（Issue #56）。
///
/// 判定と適用は 1 回のエンジンロックの中で行う。番号の判定は記録表も見るので、
/// ロックの順序は `state` → 記録表。記録表は判定と記録の瞬間だけ取る（その間も
/// `state` を保持しているので、同じ TSF の別の要求が割り込むことはない）。
fn dispatch_change(
    engine: &SharedEngine,
    session: &Session,
    seq: RequestSeq,
    expect: Expect,
    request: ChangeRequest,
) -> Response {
    let tsf = match session_tsf(session, &expect.owner) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let mut g = lock_engine(engine);
    let state = &mut *g;
    // エンジンが無ければ世代も無いので、ここで GenMismatch になる
    apply_recorded_change(
        &engine.records,
        tsf,
        seq,
        state.engine_gen,
        state.owner,
        &expect,
        || {
            let Some(eng) = state.engine.as_mut() else {
                return Response::Rejected(Reason::GenMismatch);
            };
            inject_ready(eng);
            let outcome = apply_change(eng, &mut state.bg_owner, expect.owner, request);
            apply_health_action(engine, eng);
            match outcome {
                Ok(outcome) => Response::Changed { outcome },
                Err(error) => Response::Error(error),
            }
        },
    )
}

/// 照合・保持応答の再送・適用を 1 か所にまとめる。テストも dispatch と同じ経路を通す。
fn apply_recorded_change(
    records: &Mutex<RecordTable>,
    tsf: TsfId,
    seq: RequestSeq,
    generation: Option<EngineGen>,
    owner: Option<Owner>,
    expect: &Expect,
    apply: impl FnOnce() -> Response,
) -> Response {
    let check = records
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .check(tsf, seq);
    let response = match gate_change(generation, owner, expect, check) {
        Gate::Respond(response) => return response,
        Gate::Reject(reason) => Response::Rejected(reason),
        Gate::Apply => apply(),
    };
    records
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .store(tsf, seq, &response);
    response
}

/// 読みと未確定ローマ字を復元し、所有権を移す（Issue #56）。
///
/// 1 回のエンジンロックの中で `reset_preedit`（`input_log` も消す）→
/// `force_preedit` → `push_char` × k → `then` の適用を行う。世代は照合しない
/// （世代を採り直すための要求なので）。番号の判定は `Change` と同じで、同じ番号の
/// 再送には保持した応答を返す＝再送で編集状態が 2 回変わらない。
///
/// 保存済みの応答は、エンジンの有無より先に確かめる。応答だけが失われた後に
/// エンジンが外れても、同じ番号の再送には保存した応答を返す。
///
/// 復元の前に Done の変換器を回収し（`bg_reclaim` は待たない）、BG 変換の所有者の
/// 記録を消す。別の所有者の変換が実行中なら、中断も待機もせずそのまま走らせ、
/// 所有者の記録だけを消す。その結果は `Change` 経由では誰にも取り出させない。
fn dispatch_restore(
    engine: &SharedEngine,
    session: &Session,
    owner: Owner,
    seq: RequestSeq,
    reading: String,
    pending_romaji: String,
    then: Option<ChangeRequest>,
) -> Response {
    let tsf = match session_tsf(session, &owner) {
        Ok(t) => t,
        Err(resp) => return resp,
    };
    let mut g = lock_engine(engine);
    match engine.lock_records().check(tsf, seq) {
        SeqCheck::Replay(resp) => return resp,
        SeqCheck::Unavailable => return Response::Rejected(Reason::ResultUnavailable),
        SeqCheck::New => {}
    }
    let state = &mut *g;
    let (Some(eng), Some(engine_gen)) = (state.engine.as_mut(), state.engine_gen) else {
        return Response::Error("engine not created".into());
    };
    inject_ready(eng);
    if let Err(error) = eng.bg_slot() {
        return Response::Error(error.to_string());
    }
    eng.reset_preedit();
    eng.force_preedit(reading);
    for c in pending_romaji.chars() {
        eng.push_char(c);
    }
    eng.bg_reclaim();
    state.bg_owner = None;
    state.owner = Some(owner);
    let then = then
        .map(|req| apply_change(eng, &mut state.bg_owner, owner, req))
        .transpose();
    apply_health_action(engine, eng);
    let resp = match then {
        Ok(then) => Response::Restored { engine_gen, then },
        Err(error) => Response::Error(error),
    };
    engine.lock_records().store(tsf, seq, &resp);
    resp
}

/// 変更を適用した後の読みと未確定ローマ字を写し取る。
fn edit_state(eng: &DynEngine) -> EditState {
    EditState {
        reading: eng.hiragana_text(),
        pending_romaji: eng.pending_romaji(),
    }
}
/// `(bg_owner, BgSlot)` からの純粋な判定。`Restore` は bg_owner を消すので、
/// 同じ所有者が復元し直した後は、自分の実行中の変換も `WorkerBusy` になる（意図した挙動）。
fn bg_view(bg_owner: Option<Owner>, owner: Owner, slot: BgSlot) -> BgView {
    match slot {
        BgSlot::Empty => BgView::Idle,
        _ if bg_owner != Some(owner) => BgView::WorkerBusy,
        BgSlot::Queued { .. } | BgSlot::Running { .. } => BgView::Running,
        BgSlot::Done { failed: true, .. } => BgView::Failed,
        BgSlot::Done { failed: false, .. } => BgView::Done,
    }
}
fn classify_start(
    bg_owner: Option<Owner>,
    owner: Owner,
    slot: BgSlot,
    no_reading: bool,
    ready: bool,
) -> BgStartOutcome {
    if no_reading {
        return BgStartOutcome::NoReading;
    }
    if slot != BgSlot::Empty && bg_owner != Some(owner) {
        return BgStartOutcome::WorkerBusy;
    }
    match slot {
        BgSlot::Queued { same_reading } | BgSlot::Running { same_reading } => {
            if same_reading {
                BgStartOutcome::AlreadyRunning
            } else {
                BgStartOutcome::RunningOther
            }
        }
        BgSlot::Done { .. } => BgStartOutcome::Started,
        BgSlot::Empty if ready => BgStartOutcome::Started,
        BgSlot::Empty => BgStartOutcome::NotReady,
    }
}
fn reclaim_unowned(eng: &mut DynEngine, bg_owner: Option<Owner>) -> Result<(), String> {
    if bg_owner.is_none()
        && matches!(
            eng.bg_slot().map_err(|e| e.to_string())?,
            BgSlot::Done { .. }
        )
    {
        eng.bg_reclaim();
    }
    Ok(())
}
fn start_bg(
    eng: &mut DynEngine,
    bg_owner: &mut Option<Owner>,
    owner: Owner,
    n: u32,
) -> Result<BgStartOutcome, String> {
    reclaim_unowned(eng, *bg_owner)?;
    let slot = eng.bg_slot().map_err(|e| e.to_string())?;
    let outcome = classify_start(
        *bg_owner,
        owner,
        slot,
        eng.hiragana_text().is_empty(),
        eng.is_kanji_ready(),
    );
    if outcome != BgStartOutcome::Started {
        return Ok(outcome);
    }
    if matches!(slot, BgSlot::Done { .. }) {
        eng.bg_reclaim_blocking().map_err(|e| e.to_string())?;
    }
    if eng.bg_start(n as usize) {
        *bg_owner = Some(owner);
        Ok(BgStartOutcome::Started)
    } else {
        Ok(BgStartOutcome::NotReady)
    }
}
fn apply_change(
    eng: &mut DynEngine,
    bg_owner: &mut Option<Owner>,
    owner: Owner,
    request: ChangeRequest,
) -> Result<ChangeOutcome, String> {
    reclaim_unowned(eng, *bg_owner)?;
    eng.bg_slot().map_err(|e| e.to_string())?;
    Ok(match request {
        ChangeRequest::InputChar {
            c,
            kind,
            bg_start_n_cands,
        } => {
            if let Some(ch) = char::from_u32(c) {
                match kind {
                    InputCharKind::Char => eng.push_char(ch),
                    InputCharKind::Raw => eng.push_raw(ch),
                    InputCharKind::FullwidthAlpha => eng.push_fullwidth_alpha(ch),
                }
            }
            if let Some(n) = bg_start_n_cands {
                start_bg(eng, bg_owner, owner, n)?;
            }
            ChangeOutcome::InputChar {
                preedit: eng.preedit_display(),
                hiragana: eng.hiragana_text(),
                bg: bg_view(*bg_owner, owner, eng.bg_slot().map_err(|e| e.to_string())?),
                edit: edit_state(eng),
            }
        }
        ChangeRequest::Backspace => {
            let value = eng.backspace();
            ChangeOutcome::Bool {
                value,
                edit: edit_state(eng),
            }
        }
        ChangeRequest::FlushPendingN => {
            let value = eng.flush_pending_n();
            ChangeOutcome::Bool {
                value,
                edit: edit_state(eng),
            }
        }
        ChangeRequest::BgStart { n_cands } => {
            let outcome = start_bg(eng, bg_owner, owner, n_cands)?;
            ChangeOutcome::BgStart {
                outcome,
                edit: edit_state(eng),
            }
        }
        ChangeRequest::BgTakeCandidates { key } => {
            let outcome = if *bg_owner != Some(owner) {
                BgTakeOutcome::NotYours
            } else {
                match eng.bg_take_checked(&key).map_err(|e| e.to_string())? {
                    Some(c) => BgTakeOutcome::Taken(c),
                    None => BgTakeOutcome::NotReady,
                }
            };
            ChangeOutcome::BgTake {
                outcome,
                edit: edit_state(eng),
            }
        }
        other => {
            match other {
                ChangeRequest::PushChar(c) => {
                    if let Some(c) = char::from_u32(c) {
                        eng.push_char(c);
                    }
                }
                ChangeRequest::PushRaw(c) => {
                    if let Some(c) = char::from_u32(c) {
                        eng.push_raw(c);
                    }
                }
                ChangeRequest::PushFullwidthAlpha(c) => {
                    if let Some(c) = char::from_u32(c) {
                        eng.push_fullwidth_alpha(c);
                    }
                }
                ChangeRequest::BgReclaim => eng.bg_reclaim(),
                ChangeRequest::Commit { text } => eng.commit(&text),
                ChangeRequest::CommitAsHiragana => eng.commit_as_hiragana(),
                ChangeRequest::ResetPreedit => eng.reset_preedit(),
                ChangeRequest::ForcePreedit { text } => eng.force_preedit(text),
                ChangeRequest::ResetAll => eng.reset_all(),
                ChangeRequest::Learn { reading, surface } => {
                    warn_if_dict_missing(eng, "Learn", &reading);
                    eng.learn(&reading, &surface);
                }
                ChangeRequest::LearnForce { reading, surface } => {
                    warn_if_dict_missing(eng, "LearnForce", &reading);
                    eng.learn_force(&reading, &surface);
                }
                _ => unreachable!(),
            }
            ChangeOutcome::Unit {
                edit: edit_state(eng),
            }
        }
    })
}
trait BgReader {
    fn slot(&self) -> Result<BgSlot, String>;
    fn reclaim(&mut self);
}
impl BgReader for DynEngine {
    fn slot(&self) -> Result<BgSlot, String> {
        self.bg_slot().map_err(|e| e.to_string())
    }
    fn reclaim(&mut self) {
        self.bg_reclaim();
    }
}
// 拒否・エラーの応答をそのまま Err で返す（Box に包まない）。
#[allow(clippy::result_large_err)]
fn checked_read_slot(
    generation: Option<EngineGen>,
    owner: Option<Owner>,
    expect: Expect,
    bg_owner: Option<Owner>,
    bg: &mut impl BgReader,
) -> Result<BgSlot, Response> {
    if generation != Some(expect.engine_gen) {
        return Err(Response::Rejected(Reason::GenMismatch));
    }
    if owner != Some(expect.owner) {
        return Err(Response::Rejected(Reason::OwnerMismatch));
    }
    let slot = bg.slot().map_err(Response::Error)?;
    if bg_owner.is_none() && matches!(slot, BgSlot::Done { .. }) {
        bg.reclaim();
        return bg.slot().map_err(Response::Error);
    }
    Ok(slot)
}

fn dispatch_read(
    engine: &SharedEngine,
    session: &Session,
    expect: Expect,
    request: ReadRequest,
) -> Response {
    if let Err(resp) = session_tsf(session, &expect.owner) {
        return resp;
    }
    let mut g = lock_engine(engine);
    if g.engine_gen != Some(expect.engine_gen) {
        return Response::Rejected(Reason::GenMismatch);
    }
    if g.owner != Some(expect.owner) {
        return Response::Rejected(Reason::OwnerMismatch);
    }
    let generation = g.engine_gen;
    let current_owner = g.owner;
    let bg_owner = g.bg_owner;
    let Some(eng) = g.engine.as_mut() else {
        return Response::Rejected(Reason::GenMismatch);
    };
    inject_ready(eng);
    let slot = match checked_read_slot(generation, current_owner, expect, bg_owner, eng) {
        Ok(slot) => slot,
        Err(response) => return response,
    };
    use ReadRequest::*;
    match request {
        PreeditDisplay => Response::String(eng.preedit_display()),
        PreeditIsEmpty => Response::Bool(eng.preedit_is_empty()),
        HiraganaText => Response::String(eng.hiragana_text()),
        RomajiLogStr => Response::String(eng.romaji_log_str()),
        HiraganaFromRomajiLog => Response::String(eng.hiragana_from_romaji_log()),
        CommittedText => Response::String(eng.committed_text()),
        BgStatus => Response::Bg(bg_view(bg_owner, expect.owner, slot)),
        BgPeekTopCandidate { key } => {
            if bg_owner != Some(expect.owner) {
                return Response::OptionalString(None);
            }
            match eng.bg_peek_checked(&key) {
                Ok(candidate) => Response::OptionalString(candidate),
                Err(error) => Response::Error(error.to_string()),
            }
        }
        BgWaitMs { timeout_ms } => {
            if bg_view(bg_owner, expect.owner, slot) == BgView::Running {
                eng.bg_wait_ms(timeout_ms);
            }
            match eng.bg_slot() {
                Ok(slot) => Response::Bg(bg_view(bg_owner, expect.owner, slot)),
                Err(e) => Response::Error(e.to_string()),
            }
        }
        ConvertSync => Response::Strings(eng.convert_sync()),
    }
}

/// SharedEngine の engine 側を lock し、poisoned を回復する小物ヘルパ。
fn lock_engine(engine: &SharedEngine) -> std::sync::MutexGuard<'_, SharedEngineState> {
    match engine.state.lock() {
        Ok(g) => g,
        Err(p) => {
            tracing::warn!("engine mutex poisoned, recovering");
            p.into_inner()
        }
    }
}

/// 指定 config_json で DynEngine::load_auto し、既存 slot に入れる。
/// 辞書・モデルの bg ロードも起動する。
fn load_engine_into(
    host: &HostShared,
    slot: &mut SharedEngineState,
    config_json: Option<String>,
) -> Response {
    let install = match rakukan_engine_abi::install_dir() {
        Some(p) => p,
        None => return Response::Error("install_dir not found".into()),
    };
    match DynEngine::load_auto(&install, config_json.as_deref()) {
        Ok(mut eng) => {
            if !eng.is_dict_ready() {
                eng.start_load_dict();
            }
            if !eng.is_kanji_ready() {
                eng.start_load_model();
            }
            host.set_stall_probe(eng.stall_probe());
            slot.engine = Some(eng);
            host.replace_engine_gen(slot, true);
            host.set_config(config_json);
            Response::Unit
        }
        Err(e) => Response::Error(format!("load_auto failed: {e}")),
    }
}

/// 辞書が未注入のまま学習要求が来たら host ログへ WARN を出す。
///
/// DLL 側も `learn: dict_store not initialized` を出すが、そちらは
/// `rakukan-engine-dll.log` にしか残らず、既定のログレベルでは他の DEBUG 行と
/// 混ざらないため見落としやすい。辞書のロード自体が失敗している場合は
/// `dict_status` に理由が入るので添える。
fn warn_if_dict_missing(eng: &mut DynEngine, req_name: &str, reading: &str) {
    if eng.is_dict_ready() {
        return;
    }
    tracing::warn!(
        "rpc: {} discarded (dict not injected): reading={:?} dict_status={:?}",
        req_name,
        reading,
        eng.dict_status()
    );
}

/// 推論の即時失敗を観測して復帰の段階を進める（Issue #43）。
///
/// 判断はホストが持つ。TSF は `EngineHealth` で状態を聞いて文言を決めるだけで、
/// 再起動の判断は持たない（複数の TSF プロセスが共有ホストを撃つのを避ける）。
fn apply_health_action(shared: &SharedEngine, eng: &mut DynEngine) {
    let status = eng.bg_status();
    match shared.health_observe(status) {
        Action::None => {}
        Action::ExitHost => {
            tracing::warn!(
                "engine health: inference failed {} times in a row — exiting host so a fresh one is spawned",
                health::FAILURE_THRESHOLD
            );
            shared.request_exit();
        }
        Action::MarkUnrecoverable => {
            tracing::error!(
                "engine health: still failing after {} restarts — giving up (unrecoverable)",
                health::UNRECOVERABLE_ATTEMPTS
            );
        }
        Action::Recovered => {
            tracing::info!("engine health: inference succeeded — recovery state cleared");
            health::clear_marker();
        }
    }
}

// ─── 変換の詰まりの監視（Issue #57）─────────────────────────────────────────────

/// 状態 1 回分の読みから、詰まりを確かめる対象の実行番号を選ぶ。
///
/// 同じ実行に対して動作するのは 1 回だけ（`acted` に覚える）。`unrecoverable` に
/// なってホストが生き続けても、同じ詰まりで毎秒ログを出さないため。
///
/// `acted` は**口の世代と実行番号の組**で覚える。DLL の variant が変わると
/// 新しい `CACHE` は実行番号を 0 から数え直すので、番号だけで覚えると
/// 「新しい DLL の同じ番号の実行」を処理済みとみなして見逃す。
fn stall_candidate(
    state: BgRunState,
    threshold: Duration,
    generation: u64,
    acted: Option<(u64, u64)>,
) -> Option<u64> {
    match state {
        BgRunState::Running { run_id, elapsed }
            if elapsed >= threshold && acted != Some((generation, run_id)) =>
        {
            Some(run_id)
        }
        // 不明（poison）は完了にも詰まりにも数えない
        _ => None,
    }
}

/// 変換の詰まりを監視するスレッドを起動する（Issue #57）。
///
/// [`health::STALL_POLL_INTERVAL`] ごとに DLL の変換キャッシュから「実行番号と
/// `Running` に入ってからの経過時間」を読み、同じ実行が [`health::STALL_THRESHOLD`]
/// 以上 `Running` のままなら、DLL 側の 1 回のロックの中で「同じ実行がまだ
/// `Running` で、経過時間が閾値以上か」を確かめてから段階を進める。確かめる
/// 前に完了していたり次の実行に入っていたりすれば、何もしない。
///
/// 監視はエンジンのロック（変換中は長く保持される）を経由しない。DLL を保持する
/// 口（[`StallProbe`]）を複製して使うので、途中でエンジンが作り直されても
/// 呼んでいる DLL はアンロードされない。
///
/// **閾値の超過を確認した時点で復帰を決定する。** 確認の直後にその変換が完了して
/// も、直後に始まった別の実行が巻き添えになっても、設計どおりの挙動として扱う
/// （「閾値以上止まっていた」という事実は確認の後に変わらないため）。
///
/// 複製した口が古くなる場合は、確定の直前に世代を照合して捨てる。エンジンを
/// 作り直した／外した後に、古い DLL の `Running` を根拠に現在のホストを終了
/// させないため（[`HostShared::with_current_probe`]）。
///
/// 自己終了はこのスレッドから直接行う（詰まった変換に応答を待つ相手はいない）。
/// その瞬間に処理中の要求があれば応答は失われ、クライアントは透過再接続で
/// 新しいホストへ繋ぎ直す。
fn spawn_stall_watchdog(shared: SharedEngine) {
    let spawned = std::thread::Builder::new()
        .name("rakukan-stall-watchdog".into())
        .spawn(move || {
            // (口の世代, 実行番号)
            let mut acted: Option<(u64, u64)> = None;
            loop {
                std::thread::sleep(health::STALL_POLL_INTERVAL);
                let Some((generation, probe)) = shared.stall_probe() else {
                    continue;
                };
                let Some(run_id) = stall_candidate(
                    probe.run_state(),
                    health::STALL_THRESHOLD,
                    generation,
                    acted,
                ) else {
                    continue;
                };
                // 確認の直後に完了しても、この実行が閾値以上止まっていた事実は変わらない
                match probe.confirm_stalled(run_id, health::STALL_THRESHOLD) {
                    Some(true) => {}
                    Some(false) => continue,
                    None => {
                        tracing::debug!(
                            "stall watchdog: state unavailable while confirming run_id={run_id}"
                        );
                        continue;
                    }
                }
                // 確認の後の経過時間（ログ用。確認の時点で閾値以上だった）
                let elapsed_ms = match probe.run_state() {
                    BgRunState::Running {
                        run_id: r, elapsed, ..
                    } if r == run_id => elapsed.as_millis(),
                    _ => health::STALL_THRESHOLD.as_millis(),
                };
                // 口のロックを保持したまま観測から動作までを行う。ここで
                // 世代が違えば、確認に使った口はもう現在のものではない。
                let acted_now = shared.with_current_probe(generation, || {
                    on_stall_confirmed(&shared, run_id, elapsed_ms);
                });
                if acted_now.is_none() {
                    tracing::debug!(
                        "stall watchdog: probe replaced while confirming run_id={run_id} generation={generation} — ignoring"
                    );
                    continue;
                }
                acted = Some((generation, run_id));
            }
        });
    if let Err(e) = spawned {
        tracing::error!(
            "stall watchdog: failed to spawn ({e}); stalled conversions will not be detected"
        );
    }
}

/// 詰まりを確定させ、段階を 1 つ進める（Issue #57）。
///
/// 呼び出し元が口のロックを保持したまま呼ぶ（[`HostShared::with_current_probe`]）。
/// この関数の中で `health` を取るのはロックの順序どおり。
///
/// 自己終了の前に待たない: #43 の `handle_session` 側は「応答をパイプに書いてから
/// 相手が読むまで」を待つための sleep だが、詰まりの経路には応答を待つ相手が
/// いない。ホストのログの出力先は素の `File`（`with_writer(Mutex::new(file))`）
/// なので、`tracing::warn!` の時点で OS へ渡っており、sleep は要らない。
fn on_stall_confirmed(shared: &SharedEngine, run_id: u64, elapsed_ms: u128) {
    let threshold_s = health::STALL_THRESHOLD.as_secs();
    let (action, attempt) = shared.health_observe_stall();
    match action {
        Action::ExitHost => {
            tracing::warn!(
                "engine health: conversion stalled reason=stall run_id={run_id} elapsed_ms={elapsed_ms} threshold_s={threshold_s} attempt={attempt} — exiting host so a fresh one is spawned"
            );
            health::write_marker(RecoveryMarker {
                exited_at_ms: health::now_ms(),
                attempt,
                reason: RecoveryReason::Stall,
            });
            std::process::exit(0);
        }
        Action::MarkUnrecoverable => {
            tracing::error!(
                "engine health: conversion stalled reason=stall run_id={run_id} elapsed_ms={elapsed_ms} threshold_s={threshold_s} attempt={attempt} limit={} — giving up (unrecoverable)",
                health::UNRECOVERABLE_ATTEMPTS
            );
        }
        Action::None | Action::Recovered => {}
    }
}

/// 辞書・モデルはバックグラウンドでロードされ、poll でエンジンへ注入される
/// （DLL の BG スレッドはエンジンを直接触れない）。この poll を TSF 側の
/// ラッチ任せにすると、ホストが入れ替わったとき（クラッシュ・外部終了・
/// 再 spawn）にラッチが立ったままで二度と poll されず、`dict_store=None`
/// のまま固定される。要求を処理する前に host 側で必ず注入を試みる。
/// 注入済みなら `is_*_ready()` の判定だけで終わるので RPC も往復しない。
fn inject_ready(eng: &mut DynEngine) {
    if !eng.is_dict_ready() {
        eng.poll_dict_ready();
    }
    if !eng.is_kanji_ready() {
        eng.poll_model_ready();
    }
}

fn dispatch_engine(eng: &mut DynEngine, req: Request) -> Response {
    use Request::*;

    inject_ready(eng);

    match req {
        Hello { .. }
        | Create { .. }
        | Reload { .. }
        | Bye
        | Shutdown { .. }
        | EngineHealth
        | ShutdownIfConfigDiffers { .. }
        | Change { .. }
        | Restore { .. }
        | Read { .. } => Response::Unit, // handled upstream

        _ReservedPushChar(_)
        | _ReservedPushRaw(_)
        | _ReservedPushFullwidthAlpha(_)
        | _ReservedBackspace
        | _ReservedFlushPendingN
        | _ReservedBgStart { .. }
        | _ReservedBgTakeCandidates { .. }
        | _ReservedBgReclaim
        | _ReservedCommit { .. }
        | _ReservedCommitAsHiragana
        | _ReservedResetPreedit
        | _ReservedForcePreedit { .. }
        | _ReservedResetAll
        | _ReservedLearn { .. }
        | _ReservedLearnForce { .. }
        | _ReservedInputChar { .. }
        | _ReservedPreeditDisplay
        | _ReservedPreeditIsEmpty
        | _ReservedHiraganaText
        | _ReservedRomajiLogStr
        | _ReservedHiraganaFromRomajiLog
        | _ReservedCommittedText
        | _ReservedBgStatus
        | _ReservedBgPeekTopCandidate { .. }
        | _ReservedBgWaitMs { .. }
        | _ReservedConvertSync => Response::Error("unsupported".into()),

        #[allow(deprecated)]
        _ReservedBgTakeSegmentedCandidates { .. } => Response::Error("removed".into()),

        #[allow(deprecated)]
        _ReservedConvertSyncSegmented => Response::Error("removed".into()),
        #[allow(deprecated)]
        _ReservedMergeCandidates { .. } => Response::Error(
            "MergeCandidates has been removed; use MergeCandidatesForReading".into(),
        ),
        #[allow(deprecated)]
        _ReservedSegmentSurface { .. } => Response::Error("removed".into()),
        #[allow(deprecated)]
        _ReservedSegmentCandidate { .. } => Response::Error("removed".into()),

        #[allow(deprecated)]
        _ReservedConvertToSegments { .. } => {
            Response::Error("ConvertToSegments has been removed in ABI v6".into())
        }
        ResizeSegment { .. } => Response::Error("resize_segment not yet implemented".into()),
        SegmentCandidatesFor { .. } => {
            Response::Error("segment_candidates_for not yet implemented".into())
        }

        StartLoadModel => {
            eng.start_load_model();
            Response::Unit
        }
        PollModelReady => Response::Bool(eng.poll_model_ready()),
        StartLoadDict => {
            eng.start_load_dict();
            Response::Unit
        }
        PollDictReady => Response::Bool(eng.poll_dict_ready()),

        IsKanjiReady => Response::Bool(eng.is_kanji_ready()),
        IsDictReady => Response::Bool(eng.is_dict_ready()),
        BackendLabel => Response::String(eng.backend_label()),
        NGpuLayers => Response::U32(eng.n_gpu_layers()),
        MainGpu => Response::I32(eng.main_gpu()),
        AvailableModelsJson => Response::String(eng.available_models_json()),

        MergeCandidatesForReading {
            reading,
            llm_cands,
            limit,
        } => {
            Response::Strings(eng.merge_candidates_for_reading(&reading, llm_cands, limit as usize))
        }
        LastError => Response::String(eng.last_error()),
        DictStatus => Response::String(eng.dict_status()),
    }
}

/// `Duration` を使う公開ヘルパ（main から idle 自死ロジックを書く用途）。
#[allow(dead_code)]
pub fn sleep_short() {
    std::thread::sleep(Duration::from_millis(50));
}

#[cfg(test)]
mod stall_tests {
    use super::*;

    const LIMIT: Duration = Duration::from_secs(30);

    fn running(run_id: u64, secs: u64) -> BgRunState {
        BgRunState::Running {
            run_id,
            elapsed: Duration::from_secs(secs),
        }
    }

    /// 世代 1 の口で見ている、という既定。
    const GEN: u64 = 1;

    #[test]
    fn candidate_only_past_the_threshold() {
        assert_eq!(stall_candidate(running(4, 29), LIMIT, GEN, None), None);
        assert_eq!(stall_candidate(running(4, 30), LIMIT, GEN, None), Some(4));
    }

    #[test]
    fn same_run_is_acted_on_once() {
        assert_eq!(
            stall_candidate(running(4, 90), LIMIT, GEN, Some((GEN, 4))),
            None
        );
        // 次の実行が詰まれば、また対象になる
        assert_eq!(
            stall_candidate(running(5, 30), LIMIT, GEN, Some((GEN, 4))),
            Some(5)
        );
    }

    #[test]
    fn same_run_id_in_a_new_generation_is_acted_on_again() {
        // DLL を持ち替えると実行番号は 0 から数え直しになりうる。
        // 世代が違えば、同じ番号でも処理済みとみなさない。
        assert_eq!(
            stall_candidate(running(4, 90), LIMIT, GEN + 1, Some((GEN, 4))),
            Some(4)
        );
        // 同じ世代なら従来どおり 1 回だけ
        assert_eq!(
            stall_candidate(running(4, 90), LIMIT, GEN + 1, Some((GEN + 1, 4))),
            None
        );
    }

    #[test]
    fn unknown_or_idle_is_never_a_stall() {
        assert_eq!(stall_candidate(BgRunState::Unknown, LIMIT, GEN, None), None);
        assert_eq!(
            stall_candidate(BgRunState::NotRunning { last_run_id: 4 }, LIMIT, GEN, None),
            None
        );
    }

    /// 口を持ち替えると世代が進み、古い世代での確定は捨てられる。
    #[test]
    fn probe_generation_advances_and_gates_actions() {
        let shared: SharedEngine = Arc::new(HostShared::new());
        // 口が無い間は世代を照合しても通さない
        assert!(shared.stall_probe().is_none());
        assert!(shared.with_current_probe(0, || ()).is_none());

        let probe = StallProbe::null_for_tests();
        shared.set_stall_probe(probe.clone());
        let (generation, _) = shared.stall_probe().expect("probe is set");

        assert!(shared.with_current_probe(generation, || ()).is_some());

        // 持ち替え: 古い世代は通らない
        shared.set_stall_probe(probe);
        assert!(shared.with_current_probe(generation, || ()).is_none());
        let (next_generation, _) = shared.stall_probe().expect("probe is set");
        assert_eq!(next_generation, generation + 1);

        // 取り外し: 口が無くなり、直前の世代も通らない
        shared.clear_stall_probe();
        assert!(shared.stall_probe().is_none());
        assert!(shared.with_current_probe(next_generation, || ()).is_none());
    }
}

#[cfg(test)]
mod record_tests {
    use super::*;

    const A: TsfId = TsfId(0xA);
    const B: TsfId = TsfId(0xB);

    fn gen_(n: u64) -> EngineGen {
        EngineGen {
            host_id: HostId(7),
            generation: n,
        }
    }

    fn owner(tsf: TsfId, composition: u64) -> Owner {
        Owner {
            tsf_id: tsf,
            composition,
        }
    }

    fn expect(n: u64, o: Owner) -> Expect {
        Expect {
            engine_gen: gen_(n),
            owner: o,
        }
    }

    fn is_replay_of(check: SeqCheck, want: &Response) -> bool {
        match check {
            SeqCheck::Replay(r) => format!("{r:?}") == format!("{want:?}"),
            _ => false,
        }
    }

    #[test]
    fn same_seq_replays_and_smaller_or_floor_is_unavailable() {
        let mut t = RecordTable::default();
        assert!(!t.hello(A, 0), "初回は記録なし");
        assert!(matches!(t.check(A, 1), SeqCheck::New));
        let resp = Response::Changed {
            outcome: ChangeOutcome::Bool {
                value: true,
                edit: Default::default(),
            },
        };
        t.store(A, 1, &resp);
        // 同じ番号の再送には当時の応答を返す（再適用しない）
        assert!(is_replay_of(t.check(A, 1), &resp));
        assert!(matches!(t.check(A, 2), SeqCheck::New));
        t.store(A, 2, &Response::Rejected(Reason::OwnerMismatch));
        // 記録より小さい番号は答えられない
        assert!(matches!(t.check(A, 1), SeqCheck::Unavailable));
        // Hello を経ていない TSF の要求も答えられない
        assert!(matches!(t.check(B, 1), SeqCheck::Unavailable));
    }

    #[test]
    fn record_created_by_hello_rejects_up_to_highest_sent() {
        // 回収後・新しいホストで、クライアントが送った最大の番号を申告する
        let mut t = RecordTable::default();
        assert!(!t.hello(A, 5));
        for seq in 1..=5 {
            assert!(
                matches!(t.check(A, seq), SeqCheck::Unavailable),
                "floor 以下の {seq} を新規として適用しない"
            );
        }
        assert!(matches!(t.check(A, 6), SeqCheck::New));
        // 同じホストへの再接続では記録をそのまま使う
        t.session_ended(A);
        assert!(t.hello(A, 6));
    }

    #[test]
    fn collect_skips_live_sessions_and_in_flight_requests() {
        let mut t = RecordTable::default();
        // 上限ちょうどまで埋め、全部の接続を閉じる
        for i in 0..RECORD_CAPACITY as u128 {
            t.hello(TsfId(1000 + i), 0);
            t.session_ended(TsfId(1000 + i));
        }
        // 最も古い記録は、切断後も要求を処理中（ロック待ちなど）
        t.begin_request(TsfId(1000));
        t.records.get_mut(&TsfId(1000)).unwrap().last_used = 0;
        t.hello(A, 0);
        assert_eq!(t.records.len(), RECORD_CAPACITY);
        assert!(t.records.contains_key(&TsfId(1000)), "処理中は回収しない");
        assert!(
            !t.records.contains_key(&TsfId(1001)),
            "次に古いものを捨てる"
        );
        assert!(t.records.contains_key(&A));

        t.end_request(TsfId(1000));
        assert_eq!(t.records[&TsfId(1000)].in_flight, 0);
        t.hello(B, 0);
        assert!(
            !t.records.contains_key(&TsfId(1000)),
            "処理が終われば回収できる"
        );
    }

    #[test]
    fn over_capacity_without_collectable_records_still_accepts() {
        let mut t = RecordTable::default();
        for i in 0..=RECORD_CAPACITY as u128 {
            t.hello(TsfId(i), 0); // 全部接続したまま
        }
        assert_eq!(t.records.len(), RECORD_CAPACITY + 1);
        assert!(t.warned_over_capacity);
    }

    #[test]
    fn gate_order_is_generation_then_seq_then_owner() {
        let a1 = owner(A, 1);
        let a2 = owner(A, 2);

        // 世代違いは番号・所有者より先に拒否し、記録しない
        let g = gate_change(Some(gen_(2)), Some(a1), &expect(1, a1), SeqCheck::New);
        assert!(matches!(
            g,
            Gate::Respond(Response::Rejected(Reason::GenMismatch))
        ));
        // エンジンが無い（世代が無い）場合も世代違い
        let g = gate_change(None, None, &expect(1, a1), SeqCheck::New);
        assert!(matches!(
            g,
            Gate::Respond(Response::Rejected(Reason::GenMismatch))
        ));

        // 同じ番号の再送は、所有者が移った後でも当時の応答を返す
        let applied = Response::Changed {
            outcome: ChangeOutcome::Unit {
                edit: Default::default(),
            },
        };
        let g = gate_change(
            Some(gen_(1)),
            Some(a2),
            &expect(1, a1),
            SeqCheck::Replay(applied),
        );
        assert!(matches!(
            g,
            Gate::Respond(Response::Changed {
                outcome: ChangeOutcome::Unit { .. }
            })
        ));
        // 答えられない番号は所有者を見ずに拒否（記録しない）
        let g = gate_change(
            Some(gen_(1)),
            Some(a1),
            &expect(1, a1),
            SeqCheck::Unavailable,
        );
        assert!(matches!(
            g,
            Gate::Respond(Response::Rejected(Reason::ResultUnavailable))
        ));

        // 新しい番号: 所有者が違えば拒否（番号は判定済みとして記録する）
        let g = gate_change(Some(gen_(1)), Some(a2), &expect(1, a1), SeqCheck::New);
        assert!(matches!(g, Gate::Reject(Reason::OwnerMismatch)));
        // 所有者がいない（作ったばかりのエンジン）場合も、Restore で取るまでは拒否
        let g = gate_change(Some(gen_(1)), None, &expect(1, a1), SeqCheck::New);
        assert!(matches!(g, Gate::Reject(Reason::OwnerMismatch)));
        // 全部一致すれば適用
        let g = gate_change(Some(gen_(1)), Some(a1), &expect(1, a1), SeqCheck::New);
        assert!(matches!(g, Gate::Apply));
    }

    #[test]
    fn stored_restore_reply_is_replayed_without_an_engine() {
        // Restore が適用・記録された後、応答だけが失われ、エンジンも外れた状態
        let shared: SharedEngine = Arc::new(HostShared::new());
        let session = Session { tsf: Some(A) };
        let restored = Response::Restored {
            engine_gen: gen_(1),
            then: None,
        };
        {
            let mut records = shared.lock_records();
            records.hello(A, 0);
            records.store(A, 1, &restored);
        }
        assert!(lock_engine(&shared).engine.is_none());

        // 同じ番号の再送には、エンジンの有無より先に保存した応答を返す
        let resp = dispatch_restore(
            &shared,
            &session,
            owner(A, 1),
            1,
            "た".into(),
            String::new(),
            None,
        );
        assert_eq!(format!("{resp:?}"), format!("{restored:?}"));
        // 記録より小さい番号は答えられない
        let resp = dispatch_restore(
            &shared,
            &session,
            owner(A, 1),
            0,
            "た".into(),
            String::new(),
            None,
        );
        assert!(matches!(
            resp,
            Response::Rejected(Reason::ResultUnavailable)
        ));
        // 新しい番号だけがエンジンを要求する
        let resp = dispatch_restore(
            &shared,
            &session,
            owner(A, 1),
            2,
            "た".into(),
            String::new(),
            None,
        );
        assert!(matches!(resp, Response::Error(_)), "{resp:?}");
    }

    #[test]
    fn exit_is_decided_from_the_response() {
        assert!(exits_after_response(false, &Response::ShutdownAccepted));
        assert!(!exits_after_response(false, &Response::ShutdownSkipped));
        assert!(!exits_after_response(true, &Response::ShutdownSkipped));
        assert!(exits_after_response(true, &Response::Bool(true)));
        assert!(!exits_after_response(true, &Response::Bool(false)));
        // 他の要求の Bool(true)（Backspace など）では終了しない
        assert!(!exits_after_response(false, &Response::Bool(true)));
    }

    #[test]
    fn shutdown_checks_host_id_without_the_engine_lock() {
        let shared: SharedEngine = Arc::new(HostShared::new());
        let mut session = Session::default();
        // 変換中を模してエンジンのロックを保持したままでも答えられる
        let _held = shared.state.lock().unwrap();
        let resp = dispatch(
            &shared,
            &mut session,
            Request::Shutdown {
                expected_host_id: shared.host_id,
            },
        );
        assert!(matches!(resp, Response::ShutdownAccepted));
        let resp = dispatch(
            &shared,
            &mut session,
            Request::Shutdown {
                expected_host_id: HostId(shared.host_id.0 ^ 1),
            },
        );
        assert!(matches!(resp, Response::ShutdownSkipped));
        let resp = dispatch(
            &shared,
            &mut session,
            Request::ShutdownIfConfigDiffers {
                config_json: Some("{}".into()),
                expected_host_id: HostId(shared.host_id.0 ^ 1),
                config_version: None,
            },
        );
        assert!(
            matches!(resp, Response::ShutdownSkipped),
            "別ホスト宛ては設定を比べない"
        );
        let resp = dispatch(
            &shared,
            &mut session,
            Request::ShutdownIfConfigDiffers {
                config_json: Some("{}".into()),
                expected_host_id: shared.host_id,
                config_version: None,
            },
        );
        assert!(matches!(resp, Response::Bool(true)));
    }

    #[test]
    fn hello_reports_host_id_and_record_state_and_counts_sessions() {
        let shared: SharedEngine = Arc::new(HostShared::new());
        let hello = |session: &mut Session, highest_sent| {
            dispatch(
                &shared,
                session,
                Request::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    tsf_id: A,
                    highest_sent,
                },
            )
        };
        let mut session = Session::default();
        match hello(&mut session, 3) {
            Response::Hello {
                host_id,
                engine_gen,
                record_found,
                ..
            } => {
                assert_eq!(host_id, shared.host_id);
                assert_eq!(engine_gen, None, "エンジンを作る前");
                assert!(!record_found);
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(session.tsf, Some(A));
        assert_eq!(shared.lock_records().records[&A].floor, 3);

        // 2 本目の接続
        let mut second = Session::default();
        assert!(matches!(
            hello(&mut second, 3),
            Response::Hello {
                record_found: true,
                ..
            }
        ));
        assert_eq!(shared.lock_records().records[&A].sessions, 2);

        // 要求の処理中は in_flight を数え、抜けたら戻す
        {
            let _f = InFlight::begin(&shared, session.tsf);
            assert_eq!(shared.lock_records().records[&A].in_flight, 1);
        }
        assert_eq!(shared.lock_records().records[&A].in_flight, 0);

        // Hello を経ていない接続や、別の TSF を名乗る Change は受け付けない
        let change = |tsf| Request::Change {
            seq: 4,
            expect: expect(1, owner(tsf, 1)),
            request: ChangeRequest::Backspace,
            config_version: None,
        };
        let resp = dispatch(&shared, &mut Session::default(), change(A));
        assert!(matches!(resp, Response::Error(_)));
        let resp = dispatch(&shared, &mut session, change(B));
        assert!(matches!(resp, Response::Error(_)));
        // エンジンが無ければ世代違い（記録しない）
        let resp = dispatch(&shared, &mut session, change(A));
        assert!(matches!(resp, Response::Rejected(Reason::GenMismatch)));
        assert_eq!(shared.lock_records().records[&A].last_seq, None);
        // 復元もエンジンが無ければ適用しない
        let resp = dispatch(
            &shared,
            &mut session,
            Request::Restore {
                owner: owner(A, 1),
                seq: 4,
                reading: "た".into(),
                pending_romaji: "t".into(),
                then: None,
                config_version: None,
            },
        );
        assert!(matches!(resp, Response::Error(_)));
        assert_eq!(shared.lock_records().records[&A].last_seq, None);
    }
}

#[cfg(test)]
mod bg_protocol_tests {
    use super::*;
    fn owner(n: u64) -> Owner {
        Owner {
            tsf_id: TsfId(123),
            composition: n,
        }
    }
    fn generation() -> EngineGen {
        EngineGen {
            host_id: HostId(4),
            generation: 1,
        }
    }
    fn expect() -> Expect {
        Expect {
            engine_gen: generation(),
            owner: owner(1),
        }
    }
    struct FakeBg {
        slot: BgSlot,
        reclaim_count: usize,
        contended: bool,
        poisoned: bool,
    }
    impl BgReader for FakeBg {
        fn slot(&self) -> Result<BgSlot, String> {
            if self.poisoned {
                Err("poison".into())
            } else {
                Ok(self.slot)
            }
        }
        fn reclaim(&mut self) {
            self.reclaim_count += 1;
            if !self.contended {
                self.slot = BgSlot::Empty;
            }
        }
    }
    fn done() -> FakeBg {
        FakeBg {
            slot: BgSlot::Done {
                same_reading: true,
                failed: false,
            },
            reclaim_count: 0,
            contended: false,
            poisoned: false,
        }
    }
    #[test]
    fn rejected_reads_do_not_reclaim() {
        let mut bg = done();
        assert!(matches!(
            checked_read_slot(None, Some(owner(1)), expect(), None, &mut bg),
            Err(Response::Rejected(Reason::GenMismatch))
        ));
        assert!(matches!(
            checked_read_slot(Some(generation()), Some(owner(2)), expect(), None, &mut bg),
            Err(Response::Rejected(Reason::OwnerMismatch))
        ));
        assert_eq!(bg.reclaim_count, 0);
        assert!(matches!(bg.slot, BgSlot::Done { .. }));
    }
    #[test]
    fn reads_preserve_own_done_and_reclaim_orphan_once_without_touching_replay() {
        let mut own = done();
        checked_read_slot(
            Some(generation()),
            Some(owner(1)),
            expect(),
            Some(owner(1)),
            &mut own,
        )
        .unwrap();
        assert_eq!(own.reclaim_count, 0);
        assert_eq!(bg_view(Some(owner(1)), owner(1), own.slot), BgView::Done);
        let mut orphan = done();
        for _ in 0..2 {
            assert_eq!(
                checked_read_slot(
                    Some(generation()),
                    Some(owner(1)),
                    expect(),
                    None,
                    &mut orphan
                )
                .unwrap(),
                BgSlot::Empty
            );
        }
        assert_eq!(orphan.reclaim_count, 1);
    }
    #[test]
    fn queued_running_and_missed_orphan_done_are_worker_busy() {
        for slot in [
            BgSlot::Queued { same_reading: true },
            BgSlot::Running { same_reading: true },
            BgSlot::Done {
                same_reading: true,
                failed: true,
            },
        ] {
            assert_eq!(bg_view(None, owner(1), slot), BgView::WorkerBusy);
            assert_eq!(
                classify_start(None, owner(1), slot, false, false),
                BgStartOutcome::WorkerBusy
            );
        }
        let mut bg = done();
        bg.contended = true;
        let slot =
            checked_read_slot(Some(generation()), Some(owner(1)), expect(), None, &mut bg).unwrap();
        assert_eq!(bg_view(None, owner(1), slot), BgView::WorkerBusy);
    }
    #[test]
    fn start_classifies_same_other_empty_and_own_done() {
        for slot in [
            BgSlot::Queued { same_reading: true },
            BgSlot::Running { same_reading: true },
        ] {
            assert_eq!(
                classify_start(Some(owner(1)), owner(1), slot, false, false),
                BgStartOutcome::AlreadyRunning
            );
        }
        for slot in [
            BgSlot::Queued {
                same_reading: false,
            },
            BgSlot::Running {
                same_reading: false,
            },
        ] {
            assert_eq!(
                classify_start(Some(owner(1)), owner(1), slot, false, false),
                BgStartOutcome::RunningOther
            );
        }
        assert_eq!(
            classify_start(Some(owner(1)), owner(1), done().slot, false, false),
            BgStartOutcome::Started
        );
        assert_eq!(
            classify_start(None, owner(1), BgSlot::Empty, false, false),
            BgStartOutcome::NotReady
        );
        assert_eq!(
            classify_start(None, owner(1), BgSlot::Empty, true, true),
            BgStartOutcome::NoReading
        );
    }
    #[test]
    fn poison_is_error_not_idle_or_rejection() {
        let mut bg = done();
        bg.poisoned = true;
        assert!(matches!(
            checked_read_slot(Some(generation()), Some(owner(1)), expect(), None, &mut bg),
            Err(Response::Error(_))
        ));
    }
    #[test]
    fn bg_take_replay_uses_retained_outcome_without_taking_twice() {
        for candidates in [vec!["candidate".to_string()], vec![]] {
            let records = Mutex::new(RecordTable::default());
            records.lock().unwrap().hello(owner(1).tsf_id, 0);
            let mut bg = done();
            let mut takes = 0;
            for _ in 0..2 {
                let response = apply_recorded_change(
                    &records,
                    owner(1).tsf_id,
                    1,
                    Some(generation()),
                    Some(owner(1)),
                    &expect(),
                    || {
                        takes += 1;
                        bg.slot = BgSlot::Empty;
                        Response::Changed {
                            outcome: ChangeOutcome::BgTake {
                                outcome: BgTakeOutcome::Taken(candidates.clone()),
                                edit: EditState::default(),
                            },
                        }
                    },
                );
                assert!(
                    matches!(response, Response::Changed { outcome: ChangeOutcome::BgTake { outcome: BgTakeOutcome::Taken(ref c), .. } } if *c == candidates)
                );
                assert_eq!(bg.slot, BgSlot::Empty);
            }
            assert_eq!(takes, 1);
            // 読み取りは、保持している Change の応答を置き換えない。
            checked_read_slot(Some(generation()), Some(owner(1)), expect(), None, &mut bg).unwrap();
            assert!(matches!(
                records.lock().unwrap().check(owner(1).tsf_id, 1),
                SeqCheck::Replay(_)
            ));
        }
    }
}
