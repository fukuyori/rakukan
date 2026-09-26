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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use rakukan_engine_abi::{BgRunState, DynEngine, StallProbe};

use crate::codec::{read_frame, write_frame};
use crate::health::{self, Action, Health, HealthTracker, RecoveryMarker, RecoveryReason};
use crate::pipe::{PipeStream, pipe_name_for_current_user};
use crate::protocol::{
    EngineGen, HostId, InputCharKind, PROTOCOL_VERSION, Request, RequestSeq, Response,
};

/// ホストが保持する TSF ごとの直近記録（Issue #56）。線路には流さない。
// TODO(#56a): 照合・記録を実装する時に HostShared へ持たせる。
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct RequestRecord {
    pub last_seq: Option<RequestSeq>,
    pub last_response: Option<Response>,
    pub floor: RequestSeq,
    pub sessions: u32,
    pub in_flight: u32,
    pub last_used: u64,
}

/// ホスト全体で共有される 1 つの DynEngine と、その生成に使った config。
pub type SharedEngine = Arc<HostShared>;

pub struct HostShared {
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
        Self {
            state: Mutex::new(SharedEngineState { engine: None }),
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
}

impl Default for HostShared {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SharedEngineState {
    pub engine: Option<DynEngine>,
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

fn handle_session(mut stream: PipeStream, engine: SharedEngine) -> Result<()> {
    tracing::debug!("rpc session: started");
    loop {
        let req: Request = match read_frame(&mut stream) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("rpc session: read_frame failed, closing: {e}");
                return Ok(());
            }
        };
        // M1.6 T-HOST1: Shutdown は応答送信後にプロセス exit するため前取り判定。
        let is_shutdown = matches!(req, Request::Shutdown { .. });
        // ShutdownIfConfigDiffers は「config が異なる」と判定したとき（Bool(true)
        // 応答）だけ Shutdown と同じ exit 経路に乗る。
        let is_conditional_shutdown = matches!(req, Request::ShutdownIfConfigDiffers { .. });
        let label = request_label(&req);
        let started = std::time::Instant::now();
        let resp = dispatch(&engine, req);
        let is_shutdown =
            is_shutdown || (is_conditional_shutdown && matches!(resp, Response::Bool(true)));
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
        PushChar(_) => "PushChar",
        PushRaw(_) => "PushRaw",
        PushFullwidthAlpha(_) => "PushFullwidthAlpha",
        Backspace => "Backspace",
        FlushPendingN => "FlushPendingN",
        PreeditDisplay => "PreeditDisplay",
        PreeditIsEmpty => "PreeditIsEmpty",
        HiraganaText => "HiraganaText",
        RomajiLogStr => "RomajiLogStr",
        HiraganaFromRomajiLog => "HiraganaFromRomajiLog",
        CommittedText => "CommittedText",
        BgStart { .. } => "BgStart",
        BgStatus => "BgStatus",
        BgTakeCandidates { .. } => "BgTakeCandidates",
        BgPeekTopCandidate { .. } => "BgPeekTopCandidate",
        #[allow(deprecated)]
        _ReservedBgTakeSegmentedCandidates { .. } => "_Reserved",
        BgReclaim => "BgReclaim",
        BgWaitMs { .. } => "BgWaitMs",
        Commit { .. } => "Commit",
        CommitAsHiragana => "CommitAsHiragana",
        ResetPreedit => "ResetPreedit",
        ForcePreedit { .. } => "ForcePreedit",
        ResetAll => "ResetAll",
        ConvertSync => "ConvertSync",
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
        Learn { .. } => "Learn",
        LearnForce { .. } => "LearnForce",
        MergeCandidatesForReading { .. } => "MergeCandidatesForReading",
        LastError => "LastError",
        DictStatus => "DictStatus",
        EngineHealth => "EngineHealth",
        InputChar { .. } => "InputChar",
        ShutdownIfConfigDiffers { .. } => "ShutdownIfConfigDiffers",
        Change { .. } => "Change",
        Restore { .. } => "Restore",
    }
}

fn dispatch(engine: &SharedEngine, req: Request) -> Response {
    // Hello / Create は handle し、残りは DynEngine メソッドに流す
    match req {
        Request::Hello {
            protocol_version, ..
        } => {
            if protocol_version != PROTOCOL_VERSION {
                return Response::Error(format!(
                    "protocol version mismatch: client={protocol_version} server={PROTOCOL_VERSION}"
                ));
            }
            Response::Hello {
                protocol_version: PROTOCOL_VERSION,
                // TODO(#56a): expose the host's real startup id, engine generation, and record state.
                host_id: HostId::default(),
                engine_gen: Some(EngineGen::default()),
                record_found: false,
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
            // エンジンを外す時点で監視も止める。ロードに失敗した場合に、
            // 古い DLL の `Running` を根拠に現在のホストを終了させないため
            // （成功すれば load_engine_into が新しい口を入れ直す）。
            engine.clear_stall_probe();
            load_engine_into(engine, &mut g, config_json)
        }
        Request::Bye => Response::Unit,
        // TODO(#56a): 照合・記録・復元を実装するまでは、成功と誤認されないよう Error を返す。
        Request::Change { .. } | Request::Restore { .. } => {
            Response::Error("Change / Restore are not implemented yet (#56)".to_string())
        }
        // TODO(#56a): compare expected_host_id and return Accepted or Skipped accordingly.
        Request::Shutdown { .. } => Response::ShutdownAccepted,
        // 変換中（engine ロック保持中）でも即答する必要があるので engine を取らない。
        Request::EngineHealth => Response::String(engine.health_now().as_str().to_string()),
        // TODO(#56a): expected_host_id が現在のホストと違えば比較せず ShutdownSkipped を返す。
        Request::ShutdownIfConfigDiffers { config_json, .. } => {
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

fn dispatch_engine(eng: &mut DynEngine, req: Request) -> Response {
    use Request::*;

    // 辞書・モデルはバックグラウンドでロードされ、poll でエンジンへ注入される
    // （DLL の BG スレッドはエンジンを直接触れない）。この poll を TSF 側の
    // ラッチ任せにすると、ホストが入れ替わったとき（クラッシュ・外部終了・
    // 再 spawn）にラッチが立ったままで二度と poll されず、`dict_store=None`
    // のまま固定される。要求を処理する前に host 側で必ず注入を試みる。
    // 注入済みなら `is_*_ready()` の判定だけで終わるので RPC も往復しない。
    if !eng.is_dict_ready() {
        eng.poll_dict_ready();
    }
    if !eng.is_kanji_ready() {
        eng.poll_model_ready();
    }

    match req {
        Hello { .. }
        | Create { .. }
        | Reload { .. }
        | Bye
        | Shutdown { .. }
        | EngineHealth
        | ShutdownIfConfigDiffers { .. }
        | Change { .. }
        | Restore { .. } => Response::Unit, // handled upstream

        PushChar(c) => {
            if let Some(ch) = char::from_u32(c) {
                eng.push_char(ch);
            }
            Response::Unit
        }
        PushRaw(c) => {
            if let Some(ch) = char::from_u32(c) {
                eng.push_raw(ch);
            }
            Response::Unit
        }
        PushFullwidthAlpha(c) => {
            if let Some(ch) = char::from_u32(c) {
                eng.push_fullwidth_alpha(ch);
            }
            Response::Unit
        }
        Backspace => Response::Bool(eng.backspace()),
        FlushPendingN => Response::Bool(eng.flush_pending_n()),

        PreeditDisplay => Response::String(eng.preedit_display()),
        PreeditIsEmpty => Response::Bool(eng.preedit_is_empty()),
        HiraganaText => Response::String(eng.hiragana_text()),
        RomajiLogStr => Response::String(eng.romaji_log_str()),
        HiraganaFromRomajiLog => Response::String(eng.hiragana_from_romaji_log()),
        CommittedText => Response::String(eng.committed_text()),

        BgStart { n_cands } => Response::Bool(eng.bg_start(n_cands as usize)),
        BgStatus => Response::String(eng.bg_status().to_string()),
        BgTakeCandidates { key } => match eng.bg_take_candidates(&key) {
            Some(v) => Response::Strings(v),
            None => Response::Strings(vec![]),
        },
        BgPeekTopCandidate { key } => match eng.bg_peek_top_candidate(&key) {
            Some(s) => Response::String(s),
            None => Response::String(String::new()),
        },
        #[allow(deprecated)]
        _ReservedBgTakeSegmentedCandidates { .. } => Response::Error("removed".into()),
        BgReclaim => {
            eng.bg_reclaim();
            Response::Unit
        }
        BgWaitMs { timeout_ms } => Response::Bool(eng.bg_wait_ms(timeout_ms)),

        Commit { text } => {
            eng.commit(&text);
            Response::Unit
        }
        CommitAsHiragana => {
            eng.commit_as_hiragana();
            Response::Unit
        }
        ResetPreedit => {
            eng.reset_preedit();
            Response::Unit
        }
        ForcePreedit { text } => {
            eng.force_preedit(text);
            Response::Unit
        }
        ResetAll => {
            eng.reset_all();
            Response::Unit
        }

        ConvertSync => Response::Strings(eng.convert_sync()),
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

        Learn { reading, surface } => {
            warn_if_dict_missing(eng, "Learn", &reading);
            eng.learn(&reading, &surface);
            Response::Unit
        }
        LearnForce { reading, surface } => {
            warn_if_dict_missing(eng, "LearnForce", &reading);
            eng.learn_force(&reading, &surface);
            Response::Unit
        }
        MergeCandidatesForReading {
            reading,
            llm_cands,
            limit,
        } => {
            Response::Strings(eng.merge_candidates_for_reading(&reading, llm_cands, limit as usize))
        }
        LastError => Response::String(eng.last_error()),
        DictStatus => Response::String(eng.dict_status()),

        InputChar {
            c,
            kind,
            bg_start_n_cands,
        } => {
            if let Some(ch) = char::from_u32(c) {
                match kind {
                    InputCharKind::Char => eng.push_char(ch),
                    InputCharKind::FullwidthAlpha => eng.push_fullwidth_alpha(ch),
                    InputCharKind::Raw => eng.push_raw(ch),
                }
            }
            let preedit = eng.preedit_display();
            let hiragana = eng.hiragana_text();
            let bg_status = eng.bg_status().to_string();
            if let Some(n) = bg_start_n_cands
                && !hiragana.is_empty()
            {
                eng.bg_start(n as usize);
            }
            Response::InputCharResult {
                preedit,
                hiragana,
                bg_status,
            }
        }
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
