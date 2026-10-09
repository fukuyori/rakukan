//! RPC クライアント実装 `RpcEngine`。
//!
//! DynEngine と同じ API を露出するので、TSF 側からは型 import を差し替えるだけで移行できる。
//!
//! # ホストプロセスの自動起動
//! `ensure_connected()` が呼ばれたとき、パイプに接続できなければ
//! `rakukan-engine-host.exe` を `CreateProcessW` で detached 起動してからリトライする。
//!
//! # スレッド安全性
//! 内部で 1 本の Named Pipe を `Mutex` で排他制御する。
//! 複数スレッドから同時に呼ばれても安全だが、llama の応答を待つ間ロックを
//! 保持するので並列実行はされない（DynEngine でも同じ前提）。

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};

use crate::codec::{read_frame, write_frame};
use crate::pipe::{PipeStream, pipe_name_for_current_user};
use crate::protocol::{
    BgView, ChangeOutcome, ChangeRequest, EngineGen, Expect, HostId, Owner, PIPE_BASE_NAME,
    PROTOCOL_VERSION, ReadRequest, Reason, Request, RequestSeq, Response, TsfId, Unresolved,
};
/// ホスト実行ファイル名。インストールディレクトリ直下に配置されている前提。
pub const HOST_EXE_NAME: &str = "rakukan-engine-host.exe";

/// ホスト起動が短時間に連続失敗した場合、TSF ホスト（Explorer など）からの
/// 再 spawn を一時停止して不安定化を防ぐ（Issue #55 で数え方を整理）。
///
/// 数えるのは**接続試行 1 回の最終結果**。接続 → spawn → `Hello` → `Create` の
/// どこで失敗しても 1 試行につき 1 回だけ記録し、`Hello` / `Create` まで成功したら
/// 回数と抑止を消す。抑止中の試行は、どの段階の失敗も数えず期限も延ばさない。
const HOST_FAILURE_THRESHOLD: u32 = 3;
const HOST_FAILURE_WINDOW_MS: u64 = 15_000;
const HOST_FAILURE_COOLDOWN_MS: u64 = 30_000;
/// 最初の接続試行（ホストが既に動いている場合はここで繋がる）。
const INITIAL_CONNECT_MS: u64 = 300;
/// spawn した後、ホストがパイプを作るまで待つ上限。
const CONNECT_AFTER_SPAWN_MS: u64 = 5_000;
/// 抑止中（spawn しない）に、別プロセスが起動したホストへ繋ぐ試み。
const CONNECT_WHILE_BLOCKED_MS: u64 = 500;
/// `ensure_connected` が 1 回目の失敗後に再試行するまでの待ち。
const RECONNECT_RETRY_DELAY_MS: u64 = 200;

/// spawn するホストへ渡すエンジン DLL のログレベル（`RAKUKAN_LOG`）。
///
/// DLL 内の tracing は cdylib ごとに独立した subscriber を持ち、その filter は
/// `RAKUKAN_LOG`（既定 info）だけで決まる。`config.toml` の `log_level` を
/// 上げても DLL のログは info のままで、辞書・候補まわりの DEBUG が取れない。
/// TSF が config を読んだ時点でここへ入れ、ホスト起動時に子プロセスへ渡す。
static HOST_LOG_LEVEL: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));

/// spawn するホストに渡すエンジン DLL のログレベルを設定する。
///
/// 呼び出し側（TSF）は config の読み込み・再読み込みのたびに呼ぶ。既に起動して
/// いるホストには影響しない（次回 spawn 時から反映される）。
pub fn set_host_log_level(level: Option<String>) {
    if let Ok(mut g) = HOST_LOG_LEVEL.lock() {
        *g = level;
    }
}

fn host_log_level() -> Option<String> {
    HOST_LOG_LEVEL.lock().ok().and_then(|g| g.clone())
}

/// spawn するホストへ設定する `RAKUKAN_LOG` の値を決める。
///
/// 既にこのプロセスの環境に `RAKUKAN_LOG` がある場合は何も設定しない。
/// 子はそれを継承するので、調査のために手で設定した値を config の値で
/// 上書きしてしまわないようにする。
fn spawn_log_env(env_already_set: bool, configured: Option<String>) -> Option<String> {
    if env_already_set {
        return None;
    }
    configured.filter(|level| !level.trim().is_empty())
}

/// 1 回の RPC がこの時間を超えたら WARN を出す（Step 13-2）。
const RPC_SLOW_WARN_MS: u64 = 500;

/// 呼び出し側が意図して待つ要求。閾値を超えても WARN を出さない。
const RPC_EXPECTED_SLOW: &[&str] = &[
    "Create",
    "Reload",
    "ConvertSync",
    "BgWaitMs",
    "Shutdown",
    "ShutdownIfConfigDiffers",
];

/// この要求で WARN を出す閾値（ms）。`None` = 出さない。
fn rpc_slow_threshold_ms(label: &str) -> Option<u64> {
    if RPC_EXPECTED_SLOW.contains(&label) {
        None
    } else {
        Some(RPC_SLOW_WARN_MS)
    }
}

/// 直近の区間（例: Convert 1 回）の RPC 呼び出し回数と合計時間（μs）。
///
/// 呼び出し側が `rpc_stats_reset()` してから `rpc_stats_snapshot()` で読む。
/// TSF プロセス全体で 1 組しか持たないので、複数スレッドが同時に区間を測ると
/// 混ざる。キー処理は 1 スレッドで、診断用途に足りる粒度として割り切る。
static RPC_CALLS: AtomicU32 = AtomicU32::new(0);
static RPC_MICROS: AtomicU64 = AtomicU64::new(0);

/// 区間の計測を開始する（カウンタを 0 に戻す）。
pub fn rpc_stats_reset() {
    RPC_CALLS.store(0, Ordering::Relaxed);
    RPC_MICROS.store(0, Ordering::Relaxed);
}

/// 区間の (呼び出し回数, 合計 μs) を読む。
pub fn rpc_stats_snapshot() -> (u32, u64) {
    (
        RPC_CALLS.load(Ordering::Relaxed),
        RPC_MICROS.load(Ordering::Relaxed),
    )
}

fn rpc_stats_record(elapsed_us: u64) {
    RPC_CALLS.fetch_add(1, Ordering::Relaxed);
    RPC_MICROS.fetch_add(elapsed_us, Ordering::Relaxed);
}

static HOST_FAILURE_CLOCK: LazyLock<Instant> = LazyLock::new(Instant::now);
static HOST_SPAWN_GUARD: LazyLock<Arc<Mutex<HostSpawnGuard>>> =
    LazyLock::new(|| Arc::new(Mutex::new(HostSpawnGuard::default())));
/// TSF プロセスで 1 つの台帳（Issue #56）。`RpcEngine` は `engine_reload_impl` で
/// 作り直されるので、識別子と採番を `RpcEngine` に持たせるとそこで 1 から振り直し
/// になる。プロセスの static に置く。
static REQUEST_LEDGER: LazyLock<Arc<RequestLedger>> =
    LazyLock::new(|| Arc::new(RequestLedger::new(TsfId::random())));

/// この TSF プロセスの起動インスタンス識別子（Issue #56）。
pub fn tsf_id() -> TsfId {
    REQUEST_LEDGER.tsf_id
}

pub struct RpcEngine {
    inner: Mutex<Connection<PipeTransport>>,
}

/// `RpcEngine::shutdown` の結果。通信失敗は `Err` ではなく `NoResponse`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// ホストから `ShutdownAccepted` を受信した（終了要求の受理）。
    Acknowledged,
    /// 送ったが応答を受け取れなかった（相手が先に exit した等）。
    NoResponse,
    /// 宛先のホストは既に入れ替わっていた（`ShutdownSkipped`）。今のホストは
    /// 終了していないので、終了の受理とは区別する（Issue #56）。
    Skipped,
}

/// `RpcEngine::shutdown_if_config_differs` の結果（通信失敗は `Err`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConditionalShutdown {
    /// ホストは同じ設定で動作中（`Bool(false)`）。接続・エンジンとも継続。
    SameConfig,
    /// 設定が違うので、ホストは応答後に終了する（`Bool(true)`）。既存接続は破棄済み。
    Restarting,
    /// 宛先のホストは既に入れ替わっていた（`ShutdownSkipped`）。今のホストの設定とは
    /// 比べていないので、「同じ設定」とは区別する。`Err` にすると呼び出し元が無条件の
    /// `shutdown()` へ切り替え、新しいホストを終了させてしまう。
    Skipped,
}

/// `Change` の応答（Issue #56）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeReply {
    Changed(ChangeOutcome),
    /// 適用されなかった。
    Rejected(Reason),
}

/// `Restore` の応答（Issue #56）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreReply {
    Restored {
        engine_gen: EngineGen,
        then: Option<ChangeOutcome>,
    },
    /// 適用されなかった（番号の記録を失っていて答えられない、など）。
    Rejected(Reason),
}

/// 読み取り（`Read`）が値を返せなかった理由。通信の失敗・照合での拒否・ホスト側の失敗・
/// 手元に composition が無い場合を区別する。
#[derive(Debug)]
pub enum ReadError {
    Transport(anyhow::Error),
    Rejected(Reason),
    Host(String),
    NoComposition,
}
impl ReadError {
    pub fn ownership_lost(&self) -> bool {
        matches!(
            self,
            Self::Rejected(Reason::GenMismatch | Reason::OwnerMismatch)
        )
    }
}
impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ReadError {}

#[derive(Debug)]
pub enum ChangeError {
    /// 結果不明の要求を抱えている間は、次の変更要求を送らない。
    /// `restore` で解決するか、`abandon_unresolved` で断念してから進む。
    Unresolved(Unresolved),
    /// 通信に失敗した。送った要求は結果不明として台帳に残る。
    Transport(anyhow::Error),
    /// ホストが `Error` を返した、または想定外の応答（適用されていない）。
    Host(String),
}

impl std::fmt::Display for ChangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChangeError::Unresolved(u) => write!(
                f,
                "unresolved request pending: seq={} kind={:?}",
                u.seq, u.kind
            ),
            ChangeError::Transport(e) => write!(f, "transport failed: {e}"),
            ChangeError::Host(e) => write!(f, "host error: {e}"),
        }
    }
}

impl std::error::Error for ChangeError {}

/// 要求番号と、結果不明の要求（Issue #56）。
///
/// 1 件の変更要求は「送る前に確かめる → 採番して送る前に記録 → 再送は同じ番号・
/// 同じ内容 → 応答を受け取ったら消す → 最終的に通信エラーなら残す」の順で扱う。
/// 未解決がある間は次の変更要求を送らないので、ホストの記録は TSF インスタンスに
/// つき直近 1 件で足りる。
pub(crate) struct RequestLedger {
    tsf_id: TsfId,
    state: Mutex<LedgerState>,
}

#[derive(Debug, Default)]
struct LedgerState {
    /// 最後に採った番号（= `Hello.highest_sent`）。番号は 1 から。
    last_seq: RequestSeq,
    unresolved: Option<Unresolved>,
}

impl RequestLedger {
    pub(crate) fn new(tsf_id: TsfId) -> Self {
        Self {
            tsf_id,
            state: Mutex::new(LedgerState::default()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, LedgerState> {
        match self.state.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        }
    }

    fn highest_sent(&self) -> RequestSeq {
        self.lock().last_seq
    }

    fn unresolved(&self) -> Option<Unresolved> {
        self.lock().unresolved
    }

    /// 未解決が無ければ番号を採り、結果不明として記録してから返す。
    fn begin_change(
        &self,
        request: &ChangeRequest,
        expect: &Expect,
    ) -> std::result::Result<RequestSeq, Unresolved> {
        let mut g = self.lock();
        if let Some(u) = g.unresolved {
            return Err(u);
        }
        g.last_seq += 1;
        let seq = g.last_seq;
        g.unresolved = Some(Unresolved {
            seq,
            kind: request.kind(),
            owner: expect.owner,
            engine_gen: expect.engine_gen,
        });
        Ok(seq)
    }

    /// `Restore` の番号を採る。`Restore` は未解決を解決するための要求なので、
    /// 未解決があっても送れる。`then` を載せるときは、送っている間はそれを
    /// 結果不明として記録する。戻り値の 2 つ目は、それまでの未解決。
    fn begin_restore(
        &self,
        owner: Owner,
        engine_gen: EngineGen,
        then: Option<&ChangeRequest>,
    ) -> (RequestSeq, Option<Unresolved>) {
        let mut g = self.lock();
        g.last_seq += 1;
        let seq = g.last_seq;
        let prev = g.unresolved;
        if let Some(req) = then {
            g.unresolved = Some(Unresolved {
                seq,
                kind: req.kind(),
                owner,
                engine_gen,
            });
        }
        (seq, prev)
    }

    /// 応答を受け取った（適用・拒否のどちらでも答えが出た）。
    fn resolve(&self, seq: RequestSeq) {
        let mut g = self.lock();
        if g.unresolved.is_some_and(|u| u.seq == seq) {
            g.unresolved = None;
        }
    }

    fn set_unresolved(&self, unresolved: Option<Unresolved>) {
        self.lock().unresolved = unresolved;
    }

    fn abandon(&self) -> Option<Unresolved> {
        self.lock().unresolved.take()
    }
}

/// 接続・spawn・時刻・待機の口。実装は Named Pipe と `CreateProcess`（`PipeTransport`）。
/// テストでは失敗の順序を台本で返す fake に差し替え、実際の再試行処理を通す（Issue #55）。
pub(crate) trait HostTransport {
    type Stream: Read + Write;
    /// パイプへ接続する。`timeout` は接続待ちの上限（応答待ちの期限ではない）。
    fn connect(&mut self, timeout: Duration) -> Result<Self::Stream>;
    /// ホストプロセスを起動する。起動できたかしか分からない（listen までは待たない）。
    fn spawn(&mut self) -> Result<()>;
    fn sleep(&mut self, d: Duration);
    /// `HostSpawnGuard` の時刻（単調、ms）。
    fn now_ms(&mut self) -> u64;
}

/// 本番の transport。
pub(crate) struct PipeTransport;

impl HostTransport for PipeTransport {
    type Stream = PipeStream;
    fn connect(&mut self, timeout: Duration) -> Result<PipeStream> {
        PipeStream::connect_client(&pipe_name_for_current_user(), timeout)
    }
    fn spawn(&mut self) -> Result<()> {
        spawn_host()
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn now_ms(&mut self) -> u64 {
        monotonic_now_ms()
    }
}

/// `ConfigSource` が返す、1 回の接続試行で `Create` に使う設定。
pub struct CreateConfig {
    /// `Create` で送る EngineConfig JSON（`None` なら既定値）。
    pub json: Option<String>,
    /// ログに残す、この設定の組の識別子（TSF では `rev=… sha256=…`）。
    /// `Create` が成功したときの記録に使う。
    pub label: String,
}

/// 接続試行のたびに `Create` で送る設定を返す関数（Issue #65）。
///
/// TSF は、`config.toml` を読み直した設定の組から JSON を作って返す。接続試行 1 回に
/// つき 1 回だけ呼び、その試行ではその値を固定して使う。自動の再接続では `RpcEngine` の
/// 内部のロック（と、呼び出し元が持つエンジンのロック）を保持したまま呼ぶので、この
/// 関数の中でそれらのロックを取ったり、RPC を呼んだりしてはいけない。
pub type ConfigSource = Arc<dyn Fn() -> CreateConfig + Send + Sync>;

struct Connection<T: HostTransport> {
    transport: T,
    stream: Option<T::Stream>,
    /// 直近で使った EngineConfig JSON。
    /// パイプが切れて再接続するとき、ホストがちょうど再起動していたケースでは
    /// Create を送り直す必要がある。そのときに使う。
    /// `reload()` を呼ぶと新しい config で上書きされる。
    /// `config_source` があるときは使わない。
    config_json: Option<String>,
    /// あれば、接続試行のたびにここから `Create` の JSON を取る（Issue #65）。
    config_source: Option<ConfigSource>,
    /// 直近に `Create` が成功した設定の組の識別子（`config_source` があるときだけ）。
    created_with: Option<String>,
    /// spawn の抑止。本番はプロセスで 1 つ（`HOST_SPAWN_GUARD`）、テストは個別。
    guard: Arc<Mutex<HostSpawnGuard>>,
    /// 要求番号と未解決要求。本番はプロセスで 1 つ（`REQUEST_LEDGER`）、テストは個別。
    ledger: Arc<RequestLedger>,
    /// 直近の `Hello` で得たホストの識別子（Issue #56）。`Shutdown` の宛先に使う。
    host_id: Option<HostId>,
    /// 直近に知ったエンジンの世代（`Hello` か `Restored`）。
    engine_gen: Option<EngineGen>,
}

/// `Hello` の応答から取り出す値。
#[derive(Debug, Clone, Copy)]
struct HelloInfo {
    host_id: HostId,
    engine_gen: Option<EngineGen>,
    record_found: bool,
}

#[derive(Debug, Default)]
struct HostSpawnGuard {
    window_start_ms: Option<u64>,
    failure_count: u32,
    blocked_until_ms: Option<u64>,
}

impl HostSpawnGuard {
    fn reset(&mut self) {
        self.window_start_ms = None;
        self.failure_count = 0;
        self.blocked_until_ms = None;
    }

    /// 抑止中かどうか。期限を過ぎていれば抑止を解いて `false`。
    fn is_blocked(&mut self, now_ms: u64) -> bool {
        match self.blocked_until_ms {
            Some(until_ms) if now_ms < until_ms => true,
            Some(_) => {
                self.blocked_until_ms = None;
                false
            }
            None => false,
        }
    }

    fn can_spawn(&mut self, now_ms: u64) -> Result<()> {
        if self.is_blocked(now_ms) {
            let remaining_ms = self
                .blocked_until_ms
                .map(|until| until.saturating_sub(now_ms))
                .unwrap_or(0);
            bail!(
                "host spawn temporarily disabled for {}ms after repeated startup failures",
                remaining_ms
            );
        }
        Ok(())
    }

    /// 失敗を 1 回数える。閾値に達したら抑止を開始して `true` を返す。
    ///
    /// 集計窓は**最初の失敗から** `HOST_FAILURE_WINDOW_MS` を超えた次の失敗で区切り直す
    /// （常に直近 15 秒を集計する方式ではない）。既に抑止中なら数えず、期限も延ばさない。
    fn record_failure(&mut self, now_ms: u64) -> bool {
        if self.is_blocked(now_ms) {
            return false;
        }
        let reset_window = self
            .window_start_ms
            .map(|start| now_ms.saturating_sub(start) > HOST_FAILURE_WINDOW_MS)
            .unwrap_or(true);
        if reset_window {
            self.window_start_ms = Some(now_ms);
            self.failure_count = 1;
        } else {
            self.failure_count = self.failure_count.saturating_add(1);
        }

        if self.failure_count >= HOST_FAILURE_THRESHOLD {
            self.window_start_ms = None;
            self.failure_count = 0;
            self.blocked_until_ms = Some(now_ms.saturating_add(HOST_FAILURE_COOLDOWN_MS));
            return true;
        }
        false
    }
}

/// 接続試行 1 回の途中経過。終了処理（`finish_attempt`）が記録とログに使う。
#[derive(Debug, Default)]
struct AttemptContext {
    /// 試行の開始時、または spawn の直前に抑止中だった。
    /// 抑止中の失敗はどの段階でも数えず、期限も延ばさない。
    blocked: bool,
    /// spawn を試みた結果（試みていなければ `None`）。失敗しても接続待ちは続ける
    /// （別プロセスが起動したホストへ繋がれば成功扱い）。
    spawn: Option<std::result::Result<(), String>>,
    /// spawn してから接続できるまでの時間（接続できた場合）。
    spawn_to_connect_ms: Option<u64>,
    /// この試行の `Create` に使う設定の組の識別子（`config_source` があるときだけ）。
    config_label: Option<String>,
}

/// 接続試行が失敗した段階。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AttemptStage {
    /// spawn 後の接続待ちで失敗（spawn 自体の成否は `AttemptContext::spawn`）。
    ConnectAfterSpawn,
    /// 抑止中の接続で失敗。
    ConnectWhileBlocked,
    Hello,
    Create,
}

impl AttemptStage {
    fn as_str(self) -> &'static str {
        match self {
            AttemptStage::ConnectAfterSpawn => "connect_after_spawn",
            AttemptStage::ConnectWhileBlocked => "connect_while_blocked",
            AttemptStage::Hello => "hello",
            AttemptStage::Create => "create",
        }
    }
}

struct AttemptFailure {
    stage: AttemptStage,
    error: anyhow::Error,
}

impl RpcEngine {
    /// 接続だけ試行して生成する。config_json は Create リクエストで送られる。
    pub fn connect_or_spawn(config_json: Option<String>) -> Result<Self> {
        let mut conn = Connection::new(
            PipeTransport,
            config_json,
            HOST_SPAWN_GUARD.clone(),
            REQUEST_LEDGER.clone(),
        );
        conn.ensure_connected()?;
        Ok(Self {
            inner: Mutex::new(conn),
        })
    }

    /// 接続だけ試行して生成する。`Create` の JSON は、初回の接続も以降の再接続も、
    /// 接続試行のたびに `source` から取る（Issue #65）。
    pub fn connect_or_spawn_with_source(source: ConfigSource) -> Result<Self> {
        let mut conn = Connection::new(
            PipeTransport,
            None,
            HOST_SPAWN_GUARD.clone(),
            REQUEST_LEDGER.clone(),
        );
        conn.config_source = Some(source);
        conn.ensure_connected()?;
        Ok(Self {
            inner: Mutex::new(conn),
        })
    }

    /// ホスト側の DynEngine を新しい config_json で再生成する。
    /// TSF の `engine_reload()` から呼ばれる。
    ///
    /// 接続 (PipeStream) は使い回したまま、`Request::Reload` を送るだけ。
    /// 成功後は以降の再接続でも新しい config_json が使われるよう内部に保存する
    /// （`connect_or_spawn_with_source` で作った場合、再接続の `Create` は設定の関数から取る）。
    pub fn reload(&self, config_json: Option<String>) -> Result<()> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RpcEngine mutex poisoned"))?;
        guard.config_json = config_json.clone();
        match guard.call_with_retry(Request::Reload { config_json })? {
            Response::Unit => Ok(()),
            Response::Error(e) => bail!("reload error: {e}"),
            other => bail!("unexpected reload response: {:?}", other),
        }
    }

    /// ホストプロセスに self-exit を依頼する（M1.6 T-HOST1）。
    ///
    /// `Reload` の代替: DLL drop → 再ロードの race を避けるため、プロセス全体を
    /// 終了させて次回 API 呼び出しで自動 re-spawn させる。
    ///
    /// - `Request::Shutdown` を送って `Response::ShutdownAccepted` を受信
    /// - 宛先は直近の `Hello` で得たホスト。送る前に確定し、再接続した後の再送でも
    ///   差し替えない（新しいホストは `ShutdownSkipped` を返すので、再起動は 1 回で止まる）
    /// - 受理・無応答なら内部 `PipeStream` を破棄（サーバが exit したので以降は無効）
    /// - `config_json` は保持する（次回の接続の `Create` で再送する。設定の関数
    ///   （`connect_or_spawn_with_source`）がある場合は使わず、関数から取る）
    /// - サーバが応答を返す前に exit してしまい read が失敗するケースも想定し、
    ///   通信失敗は `Err` にせず `Ok(ShutdownOutcome::NoResponse)` で返す（exit が目的なので）。
    ///   呼び出し側は、設定の反映待ちを解除する根拠に `Acknowledged` だけを使う（Issue #65）
    pub fn shutdown(&self, config_json: Option<String>) -> Result<ShutdownOutcome> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RpcEngine mutex poisoned"))?;
        guard.shutdown(config_json)
    }

    /// ホストの現在 config と異なる場合だけ self-exit を依頼する（reload storm 対策）。
    ///
    /// - `Ok(Restarting)` = config が異なり、ホストは exit する。既存接続は破棄済み。
    ///   次回 API 呼び出しで新 config の Create 付きで再 spawn される。
    /// - `Ok(SameConfig)` = ホストは既に同一 config で動作中。接続・エンジンとも継続。
    /// - `Ok(Skipped)` = 宛先のホストは既に入れ替わっていた。比べていない。
    /// - `Err(_)`    = 比較不能（旧プロトコルのホストが variant をデコードできず
    ///   切断した、ホストが応答しない等）。呼び出し元は無条件 `shutdown()` に
    ///   フォールバックすること。
    ///
    /// 宛先は `shutdown()` と同じく送る前に確定し、再送でも差し替えない。
    /// `config_json` は結果に依らず内部に保存する（次回 re-spawn 時の Create に使う。
    /// 設定の関数がある場合は使わず、関数から取る）。
    pub fn shutdown_if_config_differs(
        &self,
        config_json: Option<String>,
    ) -> Result<ConditionalShutdown> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RpcEngine mutex poisoned"))?;
        guard.shutdown_if_config_differs(config_json)
    }

    /// 採番した変更要求を送る（Issue #56）。
    ///
    /// 結果不明の要求を抱えていれば送らず `ChangeError::Unresolved` を返す。
    /// 通信に失敗した場合（再接続・再送も含めて）は `ChangeError::Transport` を返し、
    /// その要求は結果不明として残る。`Rejected(ResultUnavailable)` を受け取った場合も
    /// 同じく結果不明として残る。
    pub fn change(
        &self,
        expect: Expect,
        request: ChangeRequest,
    ) -> std::result::Result<ChangeReply, ChangeError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ChangeError::Host("RpcEngine mutex poisoned".into()))?;
        guard.send_change(expect, request)
    }

    /// 読みと未確定ローマ字を復元し、`owner` へ所有権を移す（Issue #56）。
    ///
    /// 結果不明の要求があっても送れる（それを解決するための要求）。`Restored` を
    /// 受け取れば未解決は消え、以後の `change` はその `engine_gen` を期待値にする。
    /// `Rejected(ResultUnavailable)` の場合は、`then` があればそれを、無ければそれまでの
    /// 未解決を結果不明として残す。
    pub fn restore(
        &self,
        owner: Owner,
        reading: String,
        pending_romaji: String,
        then: Option<ChangeRequest>,
    ) -> std::result::Result<RestoreReply, ChangeError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| ChangeError::Host("RpcEngine mutex poisoned".into()))?;
        guard.send_restore(owner, reading, pending_romaji, then)
    }

    /// 結果不明の要求（プロセスで 1 件まで）。
    pub fn unresolved(&self) -> Option<Unresolved> {
        REQUEST_LEDGER.unresolved()
    }

    /// 結果不明の要求を断念する（送り直さない）。`Commit` / `Learn` / `LearnForce` /
    /// `BgTakeCandidates` のように、復元では戻せない効果を持つ要求に使う。
    pub fn abandon_unresolved(&self) -> Option<Unresolved> {
        let abandoned = REQUEST_LEDGER.abandon();
        if let Some(u) = abandoned {
            tracing::info!(
                "rpc: abandoned unresolved request seq={} kind={:?}",
                u.seq,
                u.kind
            );
        }
        abandoned
    }

    /// 直近に知ったエンジンの世代（`Hello` か `Restored`）。`change` の期待値に使う。
    ///
    /// `Hello` 由来の値は、その後の `Create` より前の世代であることがある。新しい
    /// composition の最初の `change` の期待値には使わず、最初の `restore` が返した
    /// 世代を使う。
    pub fn engine_gen(&self) -> Option<EngineGen> {
        self.inner.lock().ok().and_then(|g| g.engine_gen)
    }

    fn call(&self, req: Request) -> Result<Response> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| anyhow!("RpcEngine mutex poisoned"))?;
        guard.call_with_retry(req)
    }

    fn call_unit(&self, req: Request) -> Result<()> {
        match self.call(req)? {
            Response::Unit => Ok(()),
            Response::Error(e) => bail!("rpc error: {e}"),
            other => bail!("unexpected response: {:?}", other),
        }
    }

    fn call_bool(&self, req: Request) -> Result<bool> {
        match self.call(req)? {
            Response::Bool(b) => Ok(b),
            Response::Error(e) => bail!("rpc error: {e}"),
            other => bail!("unexpected response: {:?}", other),
        }
    }

    fn call_string(&self, req: Request) -> Result<String> {
        match self.call(req)? {
            Response::String(s) => Ok(s),
            Response::Error(e) => bail!("rpc error: {e}"),
            other => bail!("unexpected response: {:?}", other),
        }
    }

    fn call_strings(&self, req: Request) -> Result<Vec<String>> {
        match self.call(req)? {
            Response::Strings(v) => Ok(v),
            Response::Error(e) => bail!("rpc error: {e}"),
            other => bail!("unexpected response: {:?}", other),
        }
    }

    // ── DynEngine 互換 API ──────────────────────────────────────────────

    /// composition 依存の読み取り。失敗を空値に戻さない。
    pub fn read(
        &self,
        expect: Expect,
        request: ReadRequest,
    ) -> std::result::Result<Response, ReadError> {
        match self
            .call(Request::Read { expect, request })
            .map_err(ReadError::Transport)?
        {
            Response::Rejected(reason) => Err(ReadError::Rejected(reason)),
            Response::Error(e) => Err(ReadError::Host(e)),
            response => Ok(response),
        }
    }
    pub fn read_string(
        &self,
        expect: Expect,
        request: ReadRequest,
    ) -> std::result::Result<String, ReadError> {
        match self.read(expect, request)? {
            Response::String(s) => Ok(s),
            other => Err(ReadError::Host(format!(
                "unexpected read response: {other:?}"
            ))),
        }
    }
    pub fn bg_status(&self, expect: Expect) -> std::result::Result<BgView, ReadError> {
        match self.read(expect, ReadRequest::BgStatus)? {
            Response::Bg(bg) => Ok(bg),
            other => Err(ReadError::Host(format!(
                "unexpected BG response: {other:?}"
            ))),
        }
    }
    pub fn bg_peek_top_candidate(
        &self,
        expect: Expect,
        key: &str,
    ) -> std::result::Result<Option<String>, ReadError> {
        match self.read(expect, ReadRequest::BgPeekTopCandidate { key: key.into() })? {
            Response::OptionalString(s) => Ok(s),
            other => Err(ReadError::Host(format!(
                "unexpected peek response: {other:?}"
            ))),
        }
    }

    /// 辞書・学習履歴を `reading` で引いて LLM 候補とマージする。
    ///
    /// 旧 `merge_candidates()`（ホスト内部の hiragana_buf を参照）は Issue #9 で削除した。
    /// 呼び出し側は「実際に候補が取れたキー」を必ず渡すこと。
    pub fn merge_candidates_for_reading(
        &self,
        reading: &str,
        llm_cands: Vec<String>,
        limit: usize,
    ) -> Vec<String> {
        self.call_strings(Request::MergeCandidatesForReading {
            reading: reading.into(),
            llm_cands,
            limit: limit as u32,
        })
        .unwrap_or_default()
    }
    pub fn start_load_model(&self) {
        let _ = self.call_unit(Request::StartLoadModel);
    }
    pub fn poll_model_ready(&self) -> bool {
        self.call_bool(Request::PollModelReady).unwrap_or(false)
    }
    pub fn start_load_dict(&self) {
        let _ = self.call_unit(Request::StartLoadDict);
    }
    pub fn poll_dict_ready(&self) -> bool {
        self.call_bool(Request::PollDictReady).unwrap_or(false)
    }

    pub fn is_kanji_ready(&self) -> bool {
        self.call_bool(Request::IsKanjiReady).unwrap_or(false)
    }
    pub fn is_dict_ready(&self) -> bool {
        self.call_bool(Request::IsDictReady).unwrap_or(false)
    }
    pub fn backend_label(&self) -> String {
        self.call_string(Request::BackendLabel)
            .unwrap_or_else(|_| "unknown".into())
    }
    pub fn n_gpu_layers(&self) -> u32 {
        match self.call(Request::NGpuLayers) {
            Ok(Response::U32(v)) => v,
            _ => 0,
        }
    }
    pub fn main_gpu(&self) -> i32 {
        match self.call(Request::MainGpu) {
            Ok(Response::I32(v)) => v,
            _ => -1,
        }
    }
    pub fn available_models_json(&self) -> String {
        self.call_string(Request::AvailableModelsJson)
            .unwrap_or_else(|_| "[]".into())
    }

    pub fn last_error(&self) -> String {
        self.call_string(Request::LastError).unwrap_or_default()
    }
    /// ホストの健全性（Issue #43）。`ok` / `recovering` / `unrecoverable`。
    ///
    /// 取れなかった場合は `ok` 扱いにする。文言を決めるためだけの問い合わせなので、
    /// ここで失敗しても従来どおりの表示にフォールバックすればよい。
    pub fn engine_health(&self) -> String {
        self.call_string(Request::EngineHealth)
            .unwrap_or_else(|_| crate::health::Health::Ok.as_str().to_string())
    }

    pub fn dict_status(&self) -> String {
        self.call_string(Request::DictStatus).unwrap_or_default()
    }
}

impl<T: HostTransport> Connection<T> {
    fn new(
        transport: T,
        config_json: Option<String>,
        guard: Arc<Mutex<HostSpawnGuard>>,
        ledger: Arc<RequestLedger>,
    ) -> Self {
        Self {
            transport,
            stream: None,
            config_json,
            config_source: None,
            created_with: None,
            guard,
            ledger,
            host_id: None,
            engine_gen: None,
        }
    }

    /// 終了要求の宛先。接続が無ければ先に繋いで `Hello` の値を得る。
    ///
    /// ここで確定した値を要求に入れて送るので、`call_with_retry` の中で再接続して
    /// 別のホストに繋がっても宛先は差し替わらない。
    fn shutdown_target(&mut self) -> Result<HostId> {
        self.ensure_connected()?;
        self.host_id
            .ok_or_else(|| anyhow!("host_id unknown after Hello"))
    }

    fn shutdown(&mut self, config_json: Option<String>) -> Result<ShutdownOutcome> {
        if let Some(cfg) = config_json {
            self.config_json = Some(cfg);
        }
        let result = self.shutdown_target().and_then(|expected_host_id| {
            self.call_with_retry(Request::Shutdown { expected_host_id })
        });
        match result {
            Ok(Response::ShutdownAccepted) => {
                // サーバは応答後に exit する。既存接続はもう使えない。
                self.stream = None;
                tracing::info!("rpc: Shutdown acknowledged by host");
                Ok(ShutdownOutcome::Acknowledged)
            }
            Ok(Response::ShutdownSkipped) => {
                // 今繋がっているホストは宛先ではない（既に入れ替わっていた）。終了して
                // いないので接続はそのまま使える。
                tracing::info!("rpc: Shutdown skipped (target host already replaced)");
                Ok(ShutdownOutcome::Skipped)
            }
            Ok(Response::Error(e)) => {
                self.stream = None;
                bail!("shutdown error: {e}")
            }
            Ok(other) => {
                self.stream = None;
                bail!("unexpected shutdown response: {:?}", other)
            }
            Err(e) => {
                // 応答を読む前に相手が exit した可能性が高い。警告にとどめ、エラーにはしない。
                // ただし「終了応答を受けた」とは区別する。
                self.stream = None;
                tracing::warn!("rpc: shutdown call failed (likely host exited early): {e}");
                Ok(ShutdownOutcome::NoResponse)
            }
        }
    }

    fn shutdown_if_config_differs(
        &mut self,
        config_json: Option<String>,
    ) -> Result<ConditionalShutdown> {
        if let Some(cfg) = config_json.clone() {
            self.config_json = Some(cfg);
        }
        let result = self.shutdown_target().and_then(|expected_host_id| {
            self.call_with_retry(Request::ShutdownIfConfigDiffers {
                config_json,
                expected_host_id,
                config_version: None,
            })
        });
        match result {
            Ok(Response::Bool(true)) => {
                // ホストは応答後に exit する。既存接続はもう使えない。
                self.stream = None;
                tracing::info!("rpc: ShutdownIfConfigDiffers: host restarting (config differs)");
                Ok(ConditionalShutdown::Restarting)
            }
            Ok(Response::Bool(false)) => Ok(ConditionalShutdown::SameConfig),
            Ok(Response::ShutdownSkipped) => {
                tracing::info!(
                    "rpc: ShutdownIfConfigDiffers skipped (target host already replaced)"
                );
                Ok(ConditionalShutdown::Skipped)
            }
            Ok(Response::Error(e)) => bail!("shutdown_if_config_differs error: {e}"),
            Ok(other) => bail!(
                "unexpected shutdown_if_config_differs response: {:?}",
                other
            ),
            Err(e) => {
                // 旧ホスト（variant 未知でデコード失敗→切断）や half-dead ホスト。
                self.stream = None;
                Err(e)
            }
        }
    }

    fn send_change(
        &mut self,
        expect: Expect,
        request: ChangeRequest,
    ) -> std::result::Result<ChangeReply, ChangeError> {
        if let Some(u) = self.ledger.unresolved() {
            return Err(ChangeError::Unresolved(u));
        }
        // 採番の前に繋いでおく。`Hello.highest_sent` は送った番号までを申告する値で、
        // まだ送っていない番号を含めると、新しい記録の `floor` がその要求を拒否する。
        self.ensure_connected().map_err(ChangeError::Transport)?;
        let seq = self
            .ledger
            .begin_change(&request, &expect)
            .map_err(ChangeError::Unresolved)?;
        // 再送（call_with_retry の 2 回目）も同じ番号・同じ内容で送る
        let result = self.call_with_retry(Request::Change {
            seq,
            expect,
            request,
            config_version: None,
        });
        let resp = match result {
            Ok(r) => r,
            // 結果不明として残す
            Err(e) => return Err(ChangeError::Transport(e)),
        };
        // 答えが出た（適用・拒否・エラーのどれでも）。ただし `ResultUnavailable` は
        // 適用されたかをホストも判断できないという答えなので、結果不明のまま残す
        if !matches!(resp, Response::Rejected(Reason::ResultUnavailable)) {
            self.ledger.resolve(seq);
        }
        match resp {
            Response::Changed { outcome } => Ok(ChangeReply::Changed(outcome)),
            Response::Rejected(reason) => {
                tracing::debug!("rpc: Change seq={seq} rejected: {reason:?}");
                Ok(ChangeReply::Rejected(reason))
            }
            Response::Error(e) => Err(ChangeError::Host(e)),
            other => Err(ChangeError::Host(format!(
                "unexpected change response: {other:?}"
            ))),
        }
    }

    fn send_restore(
        &mut self,
        owner: Owner,
        reading: String,
        pending_romaji: String,
        then: Option<ChangeRequest>,
    ) -> std::result::Result<RestoreReply, ChangeError> {
        // 採番の前に繋いでおく（send_change と同じ理由）
        self.ensure_connected().map_err(ChangeError::Transport)?;
        let known_gen = self.engine_gen.unwrap_or_default();
        let (seq, prev) = self.ledger.begin_restore(owner, known_gen, then.as_ref());
        let result = self.call_with_retry(Request::Restore {
            owner,
            seq,
            reading,
            pending_romaji,
            then,
            config_version: None,
        });
        let resp = match result {
            Ok(r) => r,
            // `then` があればそれを結果不明として残す（無ければそれまでの未解決のまま）
            Err(e) => return Err(ChangeError::Transport(e)),
        };
        match resp {
            Response::Restored { engine_gen, then } => {
                // 読みを丸ごと上書きしたので、それまでの未解決もここで解決する
                self.ledger.set_unresolved(None);
                self.engine_gen = Some(engine_gen);
                Ok(RestoreReply::Restored { engine_gen, then })
            }
            // 適用されなかった: `then` も適用されていないので、それまでの未解決に戻す。
            // `ResultUnavailable` だけは復元と `then` が適用済みの可能性があるので戻さない
            // （`then` があればそれが、無ければそれまでの未解決が結果不明のまま残る）
            Response::Rejected(reason) => {
                if reason != Reason::ResultUnavailable {
                    self.ledger.set_unresolved(prev);
                }
                tracing::debug!("rpc: Restore seq={seq} rejected: {reason:?}");
                Ok(RestoreReply::Rejected(reason))
            }
            Response::Error(e) => {
                self.ledger.set_unresolved(prev);
                Err(ChangeError::Host(e))
            }
            other => {
                self.ledger.set_unresolved(prev);
                Err(ChangeError::Host(format!(
                    "unexpected restore response: {other:?}"
                )))
            }
        }
    }

    /// 1 回の RPC。所要時間を計測して区間カウンタへ足し、閾値を超えたら WARN を
    /// 出す（Step 13-2）。再接続のリトライも含めた実時間を測る。
    fn call_with_retry(&mut self, req: Request) -> Result<Response> {
        let label = crate::server::request_label(&req);
        let started = Instant::now();
        let result = self.call_with_retry_inner(req);
        let elapsed_us = started.elapsed().as_micros() as u64;
        rpc_stats_record(elapsed_us);
        if let Some(threshold) = rpc_slow_threshold_ms(label)
            && elapsed_us / 1000 >= threshold
        {
            tracing::warn!("rpc SLOW {label} elapsed_us={elapsed_us}");
        }
        result
    }

    fn call_with_retry_inner(&mut self, req: Request) -> Result<Response> {
        for attempt in 0..2 {
            if self.stream.is_none()
                && let Err(e) = self.ensure_connected()
            {
                if attempt == 1 {
                    return Err(e);
                }
                continue;
            }
            let stream = self.stream.as_mut().expect("just ensured");
            if let Err(e) = write_frame(stream, &req) {
                tracing::debug!("rpc write failed, reconnecting: {e}");
                self.stream = None;
                continue;
            }
            match read_frame::<_, Response>(stream) {
                Ok(r) => return Ok(r),
                Err(e) => {
                    tracing::debug!("rpc read failed, reconnecting: {e}");
                    self.stream = None;
                    continue;
                }
            }
        }
        Err(anyhow!("rpc call failed after retry"))
    }

    /// Named Pipe を開き、Hello → Create を完了するところまでをひとまとめに行う。
    ///
    /// `Create` の JSON は、`config_source` があれば接続試行のたびにそこから取り
    /// （Issue #65）、無ければ `self.config_json` を使う。
    ///
    /// ## race condition リトライ
    /// `engine_reload()` でホストに `Shutdown` を送った直後、ホストが応答後 50ms
    /// sleep してから `process::exit(0)` する間に新しい client が connect →
    /// Hello を投げると、host が exit したタイミングで read が "read length"
    /// で死ぬ。これを 1 回だけリトライする。1 回目の失敗は `HostSpawnGuard` に
    /// 1 回数えられ、2 回目で成功すれば `Hello` / `Create` の成功で回数は 0 に戻る。
    /// ただし、既に 2 回の失敗が窓の中に記録されていれば、この 1 回で閾値に達して
    /// 抑止に入る（その場合も 2 回目は抑止中の接続として既存ホストを試すので、
    /// ホストが起動していれば回復し、抑止も解ける）。
    fn ensure_connected(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }
        if let Err(first_err) = self.try_connect_once() {
            tracing::warn!(
                "ensure_connected: handshake failed ({first_err}); retrying after {RECONNECT_RETRY_DELAY_MS}ms"
            );
            self.transport
                .sleep(Duration::from_millis(RECONNECT_RETRY_DELAY_MS));
            return self
                .try_connect_once()
                .with_context(|| format!("retry after first failure: {first_err}"));
        }
        Ok(())
    }

    /// 接続試行 1 回（connect → spawn → Hello → Create）。
    ///
    /// 途中の結果は `run_attempt` が `AttemptContext` に残し、**終了処理
    /// `finish_attempt` だけ**が `HostSpawnGuard` に記録する（Issue #55）。
    /// 失敗したストリームは `run_attempt` の中で破棄され、成功したものだけが
    /// `self.stream` に入る。リトライは `ensure_connected` 側で行う。
    fn try_connect_once(&mut self) -> Result<()> {
        // この試行の `Create` に使う JSON。パイプを開く前に 1 回だけ取り、試行の間は
        // 固定する（Issue #65）。
        let (config_json, config_label) = match &self.config_source {
            Some(source) => {
                let config = source();
                (config.json, Some(config.label))
            }
            None => (self.config_json.clone(), None),
        };
        let now_ms = self.transport.now_ms();
        let mut ctx = AttemptContext {
            blocked: self.with_guard(|g| g.is_blocked(now_ms)),
            config_label,
            ..AttemptContext::default()
        };
        let result = self.run_attempt(&mut ctx, config_json);
        self.finish_attempt(&ctx, result)
    }

    fn run_attempt(
        &mut self,
        ctx: &mut AttemptContext,
        config_json: Option<String>,
    ) -> std::result::Result<(), AttemptFailure> {
        // 1. まず接続を試行（ホストが動いていればここで繋がる）
        let mut stream = match self
            .transport
            .connect(Duration::from_millis(INITIAL_CONNECT_MS))
        {
            Ok(s) => s,
            Err(initial_err) => {
                // 2. 失敗: spawn の直前にも許可を確認する。短時間に連続失敗している間は
                // spawn を一時停止し、Explorer などの TSF ホストから外部プロセス起動を
                // 連打しない。抑止中でも、別プロセスが起動したホストには繋ぎに行く。
                let now_ms = self.transport.now_ms();
                match self.with_guard(|g| g.can_spawn(now_ms)) {
                    Ok(()) => {
                        let spawn = self.transport.spawn();
                        if let Err(e) = &spawn {
                            // 起動できなくても接続待ちは続ける（別プロセスが起動した
                            // ホストへ繋がれば成功扱い）。診断ログだけ残す。
                            tracing::warn!("spawn_host failed: {e}");
                        }
                        ctx.spawn = Some(spawn.map_err(|e| e.to_string()));
                        let spawned_at_ms = self.transport.now_ms();
                        match self
                            .transport
                            .connect(Duration::from_millis(CONNECT_AFTER_SPAWN_MS))
                        {
                            Ok(s) => {
                                ctx.spawn_to_connect_ms =
                                    Some(self.transport.now_ms().saturating_sub(spawned_at_ms));
                                s
                            }
                            Err(e) => {
                                return Err(AttemptFailure {
                                    stage: AttemptStage::ConnectAfterSpawn,
                                    error: e.context(format!(
                                        "connect after spawn (initial error: {initial_err})"
                                    )),
                                });
                            }
                        }
                    }
                    Err(blocked_err) => {
                        ctx.blocked = true;
                        tracing::warn!(
                            "host spawn suppressed after repeated failures: {blocked_err}"
                        );
                        match self
                            .transport
                            .connect(Duration::from_millis(CONNECT_WHILE_BLOCKED_MS))
                        {
                            Ok(s) => s,
                            Err(e) => {
                                return Err(AttemptFailure {
                                    stage: AttemptStage::ConnectWhileBlocked,
                                    error: e.context(format!(
                                        "connect while spawn suppressed (initial error: {initial_err})"
                                    )),
                                });
                            }
                        }
                    }
                }
            }
        };

        // 3. Hello 交換。これまでに送った最大の番号を毎回申告する（ホストが記録を
        // 失っていれば、それ以下の番号を新規として適用させないため）
        let hello = match Self::handshake_hello(
            &mut stream,
            self.ledger.tsf_id,
            self.ledger.highest_sent(),
        ) {
            Ok(h) => h,
            Err(error) => {
                return Err(AttemptFailure {
                    stage: AttemptStage::Hello,
                    error,
                });
            }
        };
        // 4. Create（この試行で固定した JSON を使う）
        if let Err(error) = Self::handshake_create(&mut stream, config_json) {
            return Err(AttemptFailure {
                stage: AttemptStage::Create,
                error,
            });
        }
        if !hello.record_found
            && let Some(u) = self.ledger.unresolved()
        {
            // 記録が無いので結果は分からないまま。未解決は消さない（復元か断念で解決する）
            tracing::info!(
                "rpc: host has no record for this TSF; request seq={} kind={:?} stays unresolved",
                u.seq,
                u.kind
            );
        }
        self.host_id = Some(hello.host_id);
        self.engine_gen = hello.engine_gen;
        self.stream = Some(stream);
        Ok(())
    }

    fn handshake_hello(
        stream: &mut T::Stream,
        tsf_id: TsfId,
        highest_sent: RequestSeq,
    ) -> Result<HelloInfo> {
        write_frame(
            stream,
            &Request::Hello {
                protocol_version: PROTOCOL_VERSION,
                tsf_id,
                highest_sent,
            },
        )?;
        match read_frame::<_, Response>(stream)? {
            Response::Hello {
                protocol_version,
                host_id,
                engine_gen,
                record_found,
            } if protocol_version == PROTOCOL_VERSION => Ok(HelloInfo {
                host_id,
                engine_gen,
                record_found,
            }),
            Response::Hello {
                protocol_version, ..
            } => {
                bail!("protocol version mismatch: server={protocol_version}")
            }
            Response::Error(e) => bail!("hello error: {e}"),
            other => bail!("unexpected hello response: {:?}", other),
        }
    }

    fn handshake_create(stream: &mut T::Stream, config_json: Option<String>) -> Result<()> {
        write_frame(
            stream,
            &Request::Create {
                config_json,
                config_version: None,
            },
        )?;
        match read_frame::<_, Response>(stream)? {
            Response::Unit => Ok(()),
            Response::Error(e) => bail!("create error: {e}"),
            other => bail!("unexpected create response: {:?}", other),
        }
    }

    /// 接続試行の終了処理。`HostSpawnGuard` への記録はここでだけ行う。
    ///
    /// - 成功（`Hello` / `Create` まで）: 回数と抑止を消す
    /// - 失敗: 通常の試行なら 1 回数える。抑止中の試行はどの段階でも数えず、期限も延ばさない
    ///
    /// Guard のロックは記録の間だけ持ち、接続待ちや I/O の間は持たない。
    fn finish_attempt(
        &mut self,
        ctx: &AttemptContext,
        result: std::result::Result<(), AttemptFailure>,
    ) -> Result<()> {
        let now_ms = self.transport.now_ms();
        let spawn_label = match &ctx.spawn {
            None => "not_attempted",
            Some(Ok(())) => "ok",
            Some(Err(_)) => "failed",
        };
        match result {
            Ok(()) => {
                let (had_failures, was_blocked) = self.with_guard(|g| {
                    let state = (g.failure_count != 0, g.blocked_until_ms.is_some());
                    g.reset();
                    state
                });
                // 送った組で `Create` が成功したことを、反映待ちとは別に記録する（Issue #65）
                self.created_with = ctx.config_label.clone();
                tracing::info!(
                    "host connected: Hello/Create ok config=({}) spawn={spawn_label} spawn_to_connect_ms={:?} while_blocked={}",
                    self.created_with.as_deref().unwrap_or("stored"),
                    ctx.spawn_to_connect_ms,
                    ctx.blocked
                );
                if had_failures || was_blocked {
                    tracing::info!("host connection recovered; clearing startup failure guard");
                }
                Ok(())
            }
            Err(failure) => {
                let stage = failure.stage.as_str();
                // 記録の可否は「試行の途中で抑止中だったか」(ctx.blocked) で決め、
                // ログには現在の抑止状態も添える（期限をまたいだ試行を判別できるように）。
                let outcome = self.with_guard(|g| {
                    if ctx.blocked {
                        None
                    } else {
                        Some(g.record_failure(now_ms))
                    }
                    .map(|newly_blocked| (newly_blocked, g.failure_count, g.blocked_until_ms))
                    .unwrap_or((false, g.failure_count, g.blocked_until_ms))
                });
                let (newly_blocked, count, blocked_until) = outcome;
                let blocked_now = blocked_until.is_some_and(|until| now_ms < until);
                if ctx.blocked || (!newly_blocked && blocked_now) {
                    // 抑止中の試行（開始時・spawn 直前）か、試行の途中で同じプロセスの別の試行が
                    // 抑止に入った（record_failure は抑止中なら数えない）。どちらも数えない
                    tracing::warn!(
                        "host connect failed while spawn suppressed: stage={stage} spawn={spawn_label} blocked_during_attempt={} blocked_now={blocked_now} count={count} blocked_until={blocked_until:?} (not counted, cooldown unchanged): {}",
                        ctx.blocked,
                        failure.error
                    );
                } else if newly_blocked {
                    tracing::warn!(
                        "host startup failed {HOST_FAILURE_THRESHOLD} times within {HOST_FAILURE_WINDOW_MS}ms; spawn suppressed for {HOST_FAILURE_COOLDOWN_MS}ms (stage={stage} spawn={spawn_label} count={count} blocked_until={blocked_until:?}): {}",
                        failure.error
                    );
                } else {
                    tracing::warn!(
                        "recorded host startup failure: stage={stage} spawn={spawn_label} count={count} blocked_until={blocked_until:?}: {}",
                        failure.error
                    );
                }
                Err(failure.error)
            }
        }
    }

    fn with_guard<R>(&self, f: impl FnOnce(&mut HostSpawnGuard) -> R) -> R {
        match self.guard.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(poisoned) => {
                tracing::warn!("host spawn guard mutex poisoned, recovering");
                let mut guard = poisoned.into_inner();
                f(&mut guard)
            }
        }
    }
}

/// `rakukan-engine-host.exe` を install_dir から detached で起動する。
fn spawn_host() -> Result<()> {
    let install =
        rakukan_engine_abi::install_dir().ok_or_else(|| anyhow!("install_dir not found"))?;
    let exe = install.join(HOST_EXE_NAME);
    if !exe.exists() {
        bail!("host exe not found: {}", exe.display());
    }
    spawn_detached(&exe)
}

fn monotonic_now_ms() -> u64 {
    HOST_FAILURE_CLOCK.elapsed().as_millis() as u64
}

#[cfg(target_os = "windows")]
fn spawn_detached(exe: &PathBuf) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    let mut cmd = std::process::Command::new(exe);
    cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    // エンジン DLL のログレベルを config に追随させる。手動で `RAKUKAN_LOG` を
    // 設定して起動した場合（調査時など）を壊さないよう、既に環境にあるときは
    // 触らない。
    if let Some(level) = spawn_log_env(std::env::var_os("RAKUKAN_LOG").is_some(), host_log_level())
    {
        cmd.env("RAKUKAN_LOG", level);
    }
    cmd.spawn()
        .with_context(|| format!("spawn {}", exe.display()))?;
    Ok(())
}
const _: &str = PIPE_BASE_NAME;

#[cfg(not(target_os = "windows"))]
fn spawn_detached(_exe: &PathBuf) -> Result<()> {
    bail!("only windows is supported");
}

// 未使用 import 警告回避
#[allow(dead_code)]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::InputCharKind;

    #[test]
    fn rpc_slow_threshold_skips_expected_slow_requests() {
        // 呼び出し側が意図して待つ要求は WARN の対象外
        assert_eq!(rpc_slow_threshold_ms("ConvertSync"), None);
        assert_eq!(rpc_slow_threshold_ms("BgWaitMs"), None);
        assert_eq!(rpc_slow_threshold_ms("Create"), None);
        // それ以外は閾値つき
        assert_eq!(rpc_slow_threshold_ms("PushChar"), Some(RPC_SLOW_WARN_MS));
        assert_eq!(
            rpc_slow_threshold_ms("MergeCandidatesForReading"),
            Some(RPC_SLOW_WARN_MS)
        );
    }

    #[test]
    fn rpc_stats_accumulate_and_reset() {
        rpc_stats_reset();
        rpc_stats_record(1_200);
        rpc_stats_record(800);
        assert_eq!(rpc_stats_snapshot(), (2, 2_000));
        rpc_stats_reset();
        assert_eq!(rpc_stats_snapshot(), (0, 0));
    }

    #[test]
    fn spawn_log_env_respects_existing_environment() {
        // 手で RAKUKAN_LOG を設定して起動した調査用ホストを壊さない
        assert_eq!(spawn_log_env(true, Some("debug".into())), None);
        assert_eq!(spawn_log_env(true, None), None);
    }

    #[test]
    fn spawn_log_env_uses_configured_level() {
        assert_eq!(
            spawn_log_env(false, Some("debug".into())),
            Some("debug".to_string())
        );
        assert_eq!(spawn_log_env(false, None), None);
        // 空文字は EnvFilter を壊すので渡さない
        assert_eq!(spawn_log_env(false, Some("  ".into())), None);
    }

    // ── HostSpawnGuard 単体 ───────────────────────────────────────────────

    #[test]
    fn host_spawn_guard_blocks_after_repeated_failures() {
        let mut guard = HostSpawnGuard::default();

        assert!(guard.can_spawn(0).is_ok());
        assert!(!guard.record_failure(100));
        assert!(guard.can_spawn(101).is_ok());
        assert!(!guard.record_failure(200));
        assert!(guard.can_spawn(201).is_ok());
        assert!(guard.record_failure(300), "3 回目で抑止に入る");

        let blocked = guard.can_spawn(301).unwrap_err().to_string();
        assert!(blocked.contains("temporarily disabled"));
        assert!(guard.can_spawn(300 + HOST_FAILURE_COOLDOWN_MS).is_ok());
    }

    #[test]
    fn host_spawn_guard_window_boundary() {
        // 集計窓は最初の失敗から 15,000ms までが同じ窓、15,001ms で新しい窓
        let mut guard = HostSpawnGuard::default();
        guard.record_failure(100);
        guard.record_failure(100 + HOST_FAILURE_WINDOW_MS);
        assert_eq!(guard.failure_count, 2, "ちょうど 15,000ms は同じ窓");

        let mut guard = HostSpawnGuard::default();
        guard.record_failure(100);
        guard.record_failure(100 + HOST_FAILURE_WINDOW_MS + 1);
        assert_eq!(guard.failure_count, 1, "15,001ms は新しい窓");
        assert!(guard.blocked_until_ms.is_none());
    }

    #[test]
    fn host_spawn_guard_success_clears_block() {
        let mut guard = HostSpawnGuard::default();

        guard.record_failure(100);
        guard.record_failure(200);
        guard.record_failure(300);
        assert!(guard.can_spawn(301).is_err());

        guard.reset();
        assert!(guard.can_spawn(302).is_ok());
        assert_eq!(guard.failure_count, 0);
        assert!(guard.blocked_until_ms.is_none());
    }

    #[test]
    fn host_spawn_guard_does_not_extend_cooldown_while_blocked() {
        let mut guard = HostSpawnGuard::default();
        guard.record_failure(100);
        guard.record_failure(200);
        assert!(guard.record_failure(300));
        let until = guard.blocked_until_ms.expect("blocked");

        // 抑止中の失敗は数えず、期限も動かさない
        assert!(!guard.record_failure(1_000));
        assert_eq!(guard.failure_count, 0);
        assert_eq!(guard.blocked_until_ms, Some(until));

        // 期限直前は抑止、期限到達で解ける
        assert!(guard.is_blocked(until - 1));
        assert!(!guard.is_blocked(until));
        assert!(guard.blocked_until_ms.is_none());
    }

    // ── 接続経路（fake transport で実際の再試行処理を通す）────────────────

    use std::collections::VecDeque;
    use std::io::Cursor;

    /// 台本どおりの応答を返し、書き込まれた要求を記録するストリーム。
    struct FakeStream {
        incoming: Cursor<Vec<u8>>,
        outgoing: Arc<Mutex<Vec<u8>>>,
    }

    impl Read for FakeStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.incoming.read(buf)
        }
    }

    impl Write for FakeStream {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.outgoing.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// ホストの応答を台本にしたストリーム。空なら最初の read で EOF（"read length"）。
    fn stream_with(responses: &[Response]) -> FakeStream {
        let mut bytes = Vec::new();
        for r in responses {
            write_frame(&mut bytes, r).unwrap();
        }
        FakeStream {
            incoming: Cursor::new(bytes),
            outgoing: Arc::default(),
        }
    }

    fn hello_ok() -> Response {
        Response::Hello {
            protocol_version: PROTOCOL_VERSION,
            host_id: HostId::default(),
            engine_gen: None,
            record_found: false,
        }
    }

    /// 接続・spawn の結果を台本で返し、仮想時計を進める transport。
    #[derive(Default)]
    struct FakeTransport {
        connects: VecDeque<std::result::Result<FakeStream, &'static str>>,
        spawns: VecDeque<std::result::Result<(), &'static str>>,
        now_ms: u64,
        connect_timeouts: Vec<u64>,
        spawn_calls: u32,
        slept_ms: u64,
    }

    impl HostTransport for FakeTransport {
        type Stream = FakeStream;
        fn connect(&mut self, timeout: Duration) -> Result<FakeStream> {
            self.connect_timeouts.push(timeout.as_millis() as u64);
            match self.connects.pop_front() {
                Some(Ok(s)) => {
                    self.now_ms += 10;
                    Ok(s)
                }
                Some(Err(e)) => {
                    self.now_ms += timeout.as_millis() as u64;
                    Err(anyhow!("connect_client: timeout: {e}"))
                }
                None => {
                    self.now_ms += timeout.as_millis() as u64;
                    Err(anyhow!("connect_client: timeout: no host (unscripted)"))
                }
            }
        }
        fn spawn(&mut self) -> Result<()> {
            self.spawn_calls += 1;
            match self.spawns.pop_front() {
                Some(Ok(())) | None => Ok(()),
                Some(Err(e)) => Err(anyhow!("host exe not found: {e}")),
            }
        }
        fn sleep(&mut self, d: Duration) {
            let ms = d.as_millis() as u64;
            self.slept_ms += ms;
            self.now_ms += ms;
        }
        fn now_ms(&mut self) -> u64 {
            self.now_ms
        }
    }

    fn connection(transport: FakeTransport) -> Connection<FakeTransport> {
        Connection::new(
            transport,
            None,
            Arc::new(Mutex::new(HostSpawnGuard::default())),
            Arc::new(RequestLedger::new(TsfId(0x75F))),
        )
    }

    fn guard_state(conn: &Connection<FakeTransport>) -> (u32, Option<u64>) {
        conn.with_guard(|g| (g.failure_count, g.blocked_until_ms))
    }

    /// ストリームに書かれた要求から、`Create` の JSON を取り出す。
    fn sent_create_json(outgoing: &Arc<Mutex<Vec<u8>>>) -> Option<String> {
        let mut cur = Cursor::new(outgoing.lock().unwrap().clone());
        let _hello: Request = read_frame(&mut cur).unwrap();
        match read_frame::<_, Request>(&mut cur).unwrap() {
            Request::Create { config_json, .. } => config_json,
            other => panic!("expected Create, got {other:?}"),
        }
    }

    /// 接続試行のたびに 1 回だけ呼ばれ、`cfg1`, `cfg2`, … を返す設定の関数。
    fn counting_source() -> (ConfigSource, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let source: ConfigSource = Arc::new(move || {
            let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
            CreateConfig {
                json: Some(format!("cfg{n}")),
                label: format!("rev={n}"),
            }
        });
        (source, calls)
    }

    #[test]
    fn records_the_label_of_the_config_that_created_successfully() {
        // 1 回目は Hello で切れ（rev=1 は成功の記録に残らない）、再試行の rev=2 で成功
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([
                Ok(stream_with(&[])),
                Ok(stream_with(&[hello_ok(), Response::Unit])),
            ]),
            ..Default::default()
        });
        let (source, _) = counting_source();
        conn.config_source = Some(source);
        conn.ensure_connected().expect("retry connects");
        assert_eq!(conn.created_with.as_deref(), Some("rev=2"));
    }

    #[test]
    fn a_source_returning_no_json_does_not_fall_back_to_the_stored_one() {
        let stream = stream_with(&[hello_ok(), Response::Unit]);
        let outgoing = stream.outgoing.clone();
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(stream)]),
            ..Default::default()
        });
        conn.config_json = Some("stored".into());
        conn.config_source = Some(Arc::new(|| CreateConfig {
            json: None,
            label: "defaults".into(),
        }));
        conn.try_connect_once().expect("connect");
        assert_eq!(sent_create_json(&outgoing), None);
    }

    #[test]
    fn create_uses_the_config_source_once_per_attempt() {
        use std::sync::atomic::Ordering;
        let stream = stream_with(&[hello_ok(), Response::Unit]);
        let outgoing = stream.outgoing.clone();
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(stream)]),
            ..Default::default()
        });
        conn.config_json = Some("stored".into());
        let (source, calls) = counting_source();
        conn.config_source = Some(source);
        conn.try_connect_once().expect("connect");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "試行 1 回につき 1 回だけ呼ぶ"
        );
        assert_eq!(
            sent_create_json(&outgoing).as_deref(),
            Some("cfg1"),
            "保存済みの文字列ではなく、関数の値で Create する"
        );
    }

    #[test]
    fn each_retry_attempt_takes_the_config_again() {
        use std::sync::atomic::Ordering;
        // 1 回目は Hello で切れ、`ensure_connected` の再試行で繋がる
        let second = stream_with(&[hello_ok(), Response::Unit]);
        let outgoing = second.outgoing.clone();
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(stream_with(&[])), Ok(second)]),
            ..Default::default()
        });
        let (source, calls) = counting_source();
        conn.config_source = Some(source);
        conn.ensure_connected().expect("retry connects");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(sent_create_json(&outgoing).as_deref(), Some("cfg2"));
    }

    #[test]
    fn create_without_a_source_uses_the_stored_json() {
        let stream = stream_with(&[hello_ok(), Response::Unit]);
        let outgoing = stream.outgoing.clone();
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(stream)]),
            ..Default::default()
        });
        conn.config_json = Some("stored".into());
        conn.try_connect_once().expect("connect");
        assert_eq!(sent_create_json(&outgoing).as_deref(), Some("stored"));
    }

    #[test]
    fn spawn_ok_then_connect_failure_counts_once() {
        // 初回接続失敗 → spawn 成功 → 5 秒の接続待ちも失敗: 1 試行につき 1 回だけ
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Err("no host"), Err("still no host")]),
            ..Default::default()
        });
        assert!(conn.try_connect_once().is_err());
        assert_eq!(guard_state(&conn), (1, None));
        assert_eq!(conn.transport.spawn_calls, 1);
        assert_eq!(
            conn.transport.connect_timeouts,
            vec![INITIAL_CONNECT_MS, CONNECT_AFTER_SPAWN_MS]
        );
        assert!(conn.stream.is_none());
    }

    #[test]
    fn spawn_failure_then_connect_failure_counts_once() {
        // spawn 自体が失敗（exe 無し）→ それでも接続待ちに進み → 失敗: 1 回だけ
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Err("no host"), Err("still no host")]),
            spawns: VecDeque::from([Err("missing")]),
            ..Default::default()
        });
        assert!(conn.try_connect_once().is_err());
        assert_eq!(guard_state(&conn), (1, None));
        assert_eq!(conn.transport.spawn_calls, 1);
        assert_eq!(
            conn.transport.connect_timeouts.len(),
            2,
            "spawn 失敗でも接続待ちに進む"
        );
    }

    #[test]
    fn spawn_failure_but_other_host_connects_resets_guard() {
        // spawn は失敗したが、別プロセスが起動したホストへ接続・Hello・Create が成功
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([
                Err("no host"),
                Ok(stream_with(&[hello_ok(), Response::Unit])),
            ]),
            spawns: VecDeque::from([Err("missing")]),
            ..Default::default()
        });
        conn.with_guard(|g| {
            g.record_failure(0);
        });
        assert!(conn.try_connect_once().is_ok());
        assert_eq!(guard_state(&conn), (0, None), "失敗を数えず、リセットする");
        assert!(conn.stream.is_some());
    }

    #[test]
    fn hello_and_create_failures_count_once_and_drop_stream() {
        // Hello がエラー応答
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(stream_with(&[Response::Error("nope".into())]))]),
            ..Default::default()
        });
        assert!(conn.try_connect_once().is_err());
        assert_eq!(guard_state(&conn), (1, None));
        assert!(conn.stream.is_none());

        // Hello は通るが Create がエラー応答
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(stream_with(&[
                hello_ok(),
                Response::Error("load_auto failed".into()),
            ]))]),
            ..Default::default()
        });
        assert!(conn.try_connect_once().is_err());
        assert_eq!(guard_state(&conn), (1, None));
        assert!(conn.stream.is_none());

        // 接続できたが応答が来ない（Shutdown 直後の "read length"）
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(stream_with(&[]))]),
            ..Default::default()
        });
        let err = conn.try_connect_once().unwrap_err().to_string();
        assert!(err.contains("read length"), "{err}");
        assert_eq!(guard_state(&conn), (1, None));
        assert!(conn.stream.is_none());
    }

    #[test]
    fn double_retry_reaches_threshold_within_one_rpc() {
        // ホストが起動できない状態で 1 回の RPC:
        // ensure_connected 2 回 × try_connect_once 2 回 = 4 試行。3 試行目で抑止に入り、
        // 4 試行目は spawn せず抑止中の接続だけ試して数えない。
        let mut conn = connection(FakeTransport::default());
        let err = conn.call_with_retry_inner(Request::Bye).unwrap_err();
        assert!(
            err.to_string().contains("retry after first failure"),
            "{err}"
        );

        let (count, blocked_until) = guard_state(&conn);
        assert_eq!(count, 0, "閾値到達で回数は 0 に戻る");
        let until = blocked_until.expect("3 試行目で抑止に入る");
        assert_eq!(conn.transport.spawn_calls, 3, "4 試行目は spawn しない");
        assert_eq!(
            conn.transport.connect_timeouts,
            vec![
                INITIAL_CONNECT_MS,
                CONNECT_AFTER_SPAWN_MS,
                INITIAL_CONNECT_MS,
                CONNECT_AFTER_SPAWN_MS,
                INITIAL_CONNECT_MS,
                CONNECT_AFTER_SPAWN_MS,
                INITIAL_CONNECT_MS,
                CONNECT_WHILE_BLOCKED_MS,
            ],
            "RPC 回数（1）と接続試行数（4）は別"
        );
        assert_eq!(conn.transport.slept_ms, 2 * RECONNECT_RETRY_DELAY_MS);
        // 3 試行目の失敗時刻 + 抑止時間。4 試行目で延びていない
        let third_failure_at =
            3 * (INITIAL_CONNECT_MS + CONNECT_AFTER_SPAWN_MS) + RECONNECT_RETRY_DELAY_MS;
        assert_eq!(until, third_failure_at + HOST_FAILURE_COOLDOWN_MS);
    }

    #[test]
    fn failures_while_blocked_do_not_count_or_extend() {
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([
                // 抑止中: 初回接続失敗 → spawn せず 500ms 接続も失敗
                Err("no host"),
                Err("no host"),
                // 抑止中: 最初の 300ms 接続で繋がったが Hello で失敗
                Ok(stream_with(&[Response::Error("nope".into())])),
                // 抑止中: 繋がったが Create で失敗
                Ok(stream_with(&[hello_ok(), Response::Error("nope".into())])),
            ]),
            ..Default::default()
        });
        conn.with_guard(|g| {
            g.record_failure(0);
            g.record_failure(1);
            assert!(g.record_failure(2));
        });
        let until = guard_state(&conn).1.expect("blocked");

        for _ in 0..3 {
            assert!(conn.try_connect_once().is_err());
            assert_eq!(
                guard_state(&conn),
                (0, Some(until)),
                "回数も期限も変わらない"
            );
        }
        assert_eq!(conn.transport.spawn_calls, 0);
        assert_eq!(
            conn.transport.connect_timeouts,
            vec![
                INITIAL_CONNECT_MS,
                CONNECT_WHILE_BLOCKED_MS,
                INITIAL_CONNECT_MS,
                INITIAL_CONNECT_MS
            ]
        );
    }

    #[test]
    fn success_while_blocked_clears_block_and_deadline_allows_spawn() {
        // 抑止中に既存ホストへ完全に接続できたら解除
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([
                Err("no host"),
                Ok(stream_with(&[hello_ok(), Response::Unit])),
            ]),
            ..Default::default()
        });
        conn.with_guard(|g| {
            g.record_failure(0);
            g.record_failure(1);
            assert!(g.record_failure(2));
        });
        assert!(conn.try_connect_once().is_ok());
        assert_eq!(guard_state(&conn), (0, None));
        assert_eq!(conn.transport.spawn_calls, 0, "抑止中は spawn しない");

        // 期限直前は抑止（spawn しない）、期限到達後は spawn できる
        let mut conn = connection(FakeTransport::default());
        conn.with_guard(|g| {
            g.record_failure(0);
            g.record_failure(1);
            assert!(g.record_failure(2));
        });
        let until = guard_state(&conn).1.unwrap();
        conn.transport.now_ms = until - 1 - INITIAL_CONNECT_MS;
        assert!(conn.try_connect_once().is_err());
        assert_eq!(conn.transport.spawn_calls, 0, "期限直前は抑止");
        conn.transport.now_ms = until;
        assert!(conn.try_connect_once().is_err());
        assert_eq!(conn.transport.spawn_calls, 1, "期限到達後は spawn できる");
        assert_eq!(guard_state(&conn).0, 1, "抑止が解けた後の失敗は数え直す");
    }

    #[test]
    fn shutdown_race_recovers_on_retry() {
        // 1 回目: 繋がったが旧ホストが exit して応答なし → 1 回数える
        // 2 回目（200ms 後）: 新ホストへ接続・Hello・Create 成功 → リセット
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([
                Ok(stream_with(&[])),
                Ok(stream_with(&[hello_ok(), Response::Unit])),
            ]),
            ..Default::default()
        });
        assert!(conn.ensure_connected().is_ok());
        assert_eq!(guard_state(&conn), (0, None));
        assert_eq!(conn.transport.slept_ms, RECONNECT_RETRY_DELAY_MS);
        assert!(conn.stream.is_some());
    }

    #[test]
    fn shutdown_race_with_two_prior_failures_enters_cooldown_then_recovers() {
        // 既に 2 失敗が窓の中にあると、race の 1 回で閾値に達して抑止に入る。
        // それでも再試行は抑止中の接続として既存ホストを試すので、起動していれば回復し、抑止も解ける。
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([
                Ok(stream_with(&[])),
                Err("exiting"),
                Ok(stream_with(&[hello_ok(), Response::Unit])),
            ]),
            ..Default::default()
        });
        conn.with_guard(|g| {
            g.record_failure(0);
            g.record_failure(1);
        });
        assert!(conn.ensure_connected().is_ok());
        assert_eq!(guard_state(&conn), (0, None), "成功で抑止も解ける");
        assert_eq!(conn.transport.spawn_calls, 0, "抑止中なので spawn しない");
        assert_eq!(
            conn.transport.connect_timeouts,
            vec![
                INITIAL_CONNECT_MS,
                INITIAL_CONNECT_MS,
                CONNECT_WHILE_BLOCKED_MS
            ]
        );
    }

    // ── Issue #56: 要求番号・未解決要求・終了の宛先 ────────────────────────

    use crate::protocol::ChangeKind;

    const HOST_A: HostId = HostId(0xA0);
    const HOST_B: HostId = HostId(0xB0);

    fn hello_from(host_id: HostId, record_found: bool) -> Response {
        Response::Hello {
            protocol_version: PROTOCOL_VERSION,
            host_id,
            engine_gen: None,
            record_found,
        }
    }

    /// 書き込まれた要求を後から読めるストリーム（接続を捨てた後も読める）。
    fn logged_stream(responses: &[Response]) -> (FakeStream, Arc<Mutex<Vec<u8>>>) {
        let s = stream_with(responses);
        let log = s.outgoing.clone();
        (s, log)
    }

    fn sent(log: &Arc<Mutex<Vec<u8>>>) -> Vec<Request> {
        let bytes = log.lock().unwrap().clone();
        let mut cur = Cursor::new(bytes);
        let mut out = Vec::new();
        while let Ok(req) = read_frame::<_, Request>(&mut cur) {
            out.push(req);
        }
        out
    }

    fn owner1() -> Owner {
        Owner {
            tsf_id: TsfId(0x75F),
            composition: 1,
        }
    }

    fn gen1() -> EngineGen {
        EngineGen {
            host_id: HOST_A,
            generation: 1,
        }
    }

    fn expect1() -> Expect {
        Expect {
            engine_gen: gen1(),
            owner: owner1(),
        }
    }

    #[test]
    fn change_is_numbered_and_resolved_by_any_reply() {
        let (s, log) = logged_stream(&[
            hello_from(HOST_A, false),
            Response::Unit,
            Response::Changed {
                outcome: ChangeOutcome::Bool {
                    value: true,
                    edit: Default::default(),
                },
            },
            Response::Rejected(Reason::OwnerMismatch),
        ]);
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(s)]),
            ..Default::default()
        });
        let r = conn
            .send_change(expect1(), ChangeRequest::Backspace)
            .unwrap();
        assert_eq!(
            r,
            ChangeReply::Changed(ChangeOutcome::Bool {
                value: true,
                edit: Default::default()
            })
        );
        assert_eq!(conn.ledger.unresolved(), None);
        // 拒否も答えなので未解決は残らない
        let r = conn
            .send_change(expect1(), ChangeRequest::Backspace)
            .unwrap();
        assert_eq!(r, ChangeReply::Rejected(Reason::OwnerMismatch));
        assert_eq!(conn.ledger.unresolved(), None);

        let reqs = sent(&log);
        assert!(matches!(
            reqs[0],
            Request::Hello {
                tsf_id: TsfId(0x75F),
                highest_sent: 0,
                ..
            }
        ));
        assert!(matches!(reqs[2], Request::Change { seq: 1, .. }));
        assert!(matches!(reqs[3], Request::Change { seq: 2, .. }));
    }

    #[test]
    fn lost_reply_is_resent_with_the_same_seq_and_stays_unresolved_on_failure() {
        // 1 本目: 要求を書いた後に応答が来ない（パイプだけ切れた）
        let (first, first_log) = logged_stream(&[hello_from(HOST_A, false), Response::Unit]);
        // 2 本目: 再接続後も応答が来ない
        let (second, second_log) = logged_stream(&[hello_from(HOST_A, true), Response::Unit]);
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(first), Ok(second)]),
            ..Default::default()
        });
        let err = conn
            .send_change(
                expect1(),
                ChangeRequest::Commit {
                    text: "漢字".into(),
                },
            )
            .unwrap_err();
        assert!(matches!(err, ChangeError::Transport(_)), "{err}");

        // 再送は同じ番号・同じ内容。再接続の Hello は送った最大の番号を申告する
        let first_reqs = sent(&first_log);
        let second_reqs = sent(&second_log);
        assert!(matches!(first_reqs[2], Request::Change { seq: 1, .. }));
        assert!(matches!(
            second_reqs[0],
            Request::Hello {
                highest_sent: 1,
                ..
            }
        ));
        match (&first_reqs[2], &second_reqs[2]) {
            (
                Request::Change {
                    seq: a,
                    request: ra,
                    ..
                },
                Request::Change {
                    seq: b,
                    request: rb,
                    ..
                },
            ) => {
                assert_eq!(a, b);
                assert_eq!(ra, rb);
            }
            other => panic!("unexpected {other:?}"),
        }

        // 結果不明のまま残り、次の変更要求は送らない
        let u = conn.ledger.unresolved().expect("unresolved");
        assert_eq!(u.seq, 1);
        assert_eq!(u.kind, ChangeKind::Commit);
        let err = conn
            .send_change(expect1(), ChangeRequest::ResetPreedit)
            .unwrap_err();
        assert!(matches!(err, ChangeError::Unresolved(u) if u.seq == 1));
        assert_eq!(
            conn.ledger.highest_sent(),
            1,
            "送らなかった要求は採番しない"
        );

        // 断念すれば次へ進める
        assert_eq!(conn.ledger.abandon().map(|u| u.seq), Some(1));
        assert_eq!(conn.ledger.unresolved(), None);
    }

    #[test]
    fn restore_resolves_the_pending_request_and_learns_the_generation() {
        let (s, log) = logged_stream(&[
            hello_from(HOST_A, false),
            Response::Unit,
            Response::Rejected(Reason::ResultUnavailable),
            Response::Restored {
                engine_gen: gen1(),
                then: Some(ChangeOutcome::InputChar {
                    preedit: "たt".into(),
                    hiragana: "た".into(),
                    bg: crate::protocol::BgView::Idle,
                    edit: Default::default(),
                }),
            },
        ]);
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(s)]),
            ..Default::default()
        });
        // 結果不明の打鍵を抱えている
        let pending = Unresolved {
            seq: 7,
            kind: ChangeKind::InputChar,
            owner: owner1(),
            engine_gen: gen1(),
        };
        conn.ledger.set_unresolved(Some(pending));
        conn.ledger.lock().last_seq = 7;
        let input = ChangeRequest::InputChar {
            c: 't' as u32,
            kind: InputCharKind::Char,
            bg_start_n_cands: None,
        };

        // 答えられないと拒否された復元は、復元と `then` が適用済みかもしれないので
        // 元の未解決には戻さず、`then` を結果不明として残す
        let r = conn
            .send_restore(owner1(), "た".into(), String::new(), Some(input.clone()))
            .unwrap();
        assert_eq!(r, RestoreReply::Rejected(Reason::ResultUnavailable));
        let u = conn.ledger.unresolved().expect("then stays unresolved");
        assert_eq!(
            (u.seq, u.kind, u.owner),
            (8, ChangeKind::InputChar, owner1())
        );

        // 未解決があっても復元は送れる。受理されれば未解決は消え、世代を覚える
        let r = conn
            .send_restore(owner1(), "た".into(), String::new(), Some(input))
            .unwrap();
        assert!(
            matches!(r, RestoreReply::Restored { engine_gen, then: Some(_) } if engine_gen == gen1())
        );
        assert_eq!(conn.ledger.unresolved(), None);
        assert_eq!(conn.engine_gen, Some(gen1()));

        let reqs = sent(&log);
        assert!(matches!(reqs[2], Request::Restore { seq: 8, .. }));
        assert!(matches!(reqs[3], Request::Restore { seq: 9, .. }));
    }

    #[test]
    fn result_unavailable_keeps_the_change_unresolved() {
        let (s, _log) = logged_stream(&[
            hello_from(HOST_A, false),
            Response::Unit,
            Response::Rejected(Reason::ResultUnavailable),
        ]);
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(s)]),
            ..Default::default()
        });
        let r = conn
            .send_change(expect1(), ChangeRequest::Backspace)
            .unwrap();
        assert_eq!(r, ChangeReply::Rejected(Reason::ResultUnavailable));
        let u = conn.ledger.unresolved().expect("unresolved");
        assert_eq!((u.seq, u.kind), (1, ChangeKind::Backspace));

        // 次の変更要求は送らない
        let err = conn
            .send_change(expect1(), ChangeRequest::ResetPreedit)
            .unwrap_err();
        assert!(matches!(err, ChangeError::Unresolved(u) if u.seq == 1));
        assert_eq!(conn.ledger.highest_sent(), 1);
    }

    #[test]
    fn restore_rejections_without_then() {
        let (s, _log) = logged_stream(&[
            hello_from(HOST_A, false),
            Response::Unit,
            Response::Rejected(Reason::ResultUnavailable),
            Response::Rejected(Reason::OwnerMismatch),
            Response::Error("engine not created".into()),
        ]);
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(s)]),
            ..Default::default()
        });
        let pending = Unresolved {
            seq: 7,
            kind: ChangeKind::InputChar,
            owner: owner1(),
            engine_gen: gen1(),
        };
        conn.ledger.set_unresolved(Some(pending));
        conn.ledger.lock().last_seq = 7;

        // `then` が無ければ、`ResultUnavailable` でもそれまでの未解決がそのまま残る
        let r = conn
            .send_restore(owner1(), "た".into(), String::new(), None)
            .unwrap();
        assert_eq!(r, RestoreReply::Rejected(Reason::ResultUnavailable));
        assert_eq!(conn.ledger.unresolved(), Some(pending));

        // それ以外の拒否と Error でも元の未解決へ戻る
        let r = conn
            .send_restore(owner1(), "た".into(), String::new(), None)
            .unwrap();
        assert_eq!(r, RestoreReply::Rejected(Reason::OwnerMismatch));
        assert_eq!(conn.ledger.unresolved(), Some(pending));
        let err = conn
            .send_restore(owner1(), "た".into(), String::new(), None)
            .unwrap_err();
        assert!(matches!(err, ChangeError::Host(_)), "{err}");
        assert_eq!(conn.ledger.unresolved(), Some(pending));
    }

    #[test]
    fn shutdown_target_is_fixed_before_sending_and_not_replaced_on_resend() {
        // 1 本目（ホスト A）: Shutdown を書いた後に切れる
        let (first, first_log) = logged_stream(&[hello_from(HOST_A, false), Response::Unit]);
        // 2 本目（新しいホスト B）: 再送された Shutdown は A 宛てなので Skipped
        let (second, second_log) = logged_stream(&[
            hello_from(HOST_B, false),
            Response::Unit,
            Response::ShutdownSkipped,
        ]);
        let mut conn = connection(FakeTransport {
            connects: VecDeque::from([Ok(first), Ok(second)]),
            ..Default::default()
        });
        assert_eq!(conn.shutdown(None).unwrap(), ShutdownOutcome::Skipped);
        assert!(matches!(
            sent(&first_log)[2],
            Request::Shutdown {
                expected_host_id: HOST_A
            }
        ));
        assert!(
            matches!(
                sent(&second_log)[2],
                Request::Shutdown {
                    expected_host_id: HOST_A
                }
            ),
            "再接続で B の host_id に差し替えない"
        );
        assert_eq!(conn.host_id, Some(HOST_B));
        assert!(conn.stream.is_some(), "B は終了していないので接続を残す");
    }

    #[test]
    fn shutdown_outcomes_follow_the_reply() {
        let run = |reply: Option<Response>| {
            let mut script = vec![hello_from(HOST_A, false), Response::Unit];
            script.extend(reply);
            let mut conn = connection(FakeTransport {
                // 応答が無い場合、再送先も応答しない
                connects: VecDeque::from([Ok(stream_with(&script)), Ok(stream_with(&[]))]),
                ..Default::default()
            });
            let out = conn.shutdown(None);
            (out.map_err(|e| e.to_string()), conn.stream.is_some())
        };
        assert_eq!(
            run(Some(Response::ShutdownAccepted)),
            (Ok(ShutdownOutcome::Acknowledged), false)
        );
        assert_eq!(
            run(Some(Response::ShutdownSkipped)),
            (Ok(ShutdownOutcome::Skipped), true)
        );
        // 旧形式の Unit は受理とみなさない
        assert!(run(Some(Response::Unit)).0.is_err());
        assert_eq!(run(None).0, Ok(ShutdownOutcome::NoResponse));
    }

    #[test]
    fn conditional_shutdown_distinguishes_skipped_from_same_config() {
        let run = |reply: Response| {
            let (s, log) = logged_stream(&[hello_from(HOST_A, false), Response::Unit, reply]);
            let mut conn = connection(FakeTransport {
                connects: VecDeque::from([Ok(s)]),
                ..Default::default()
            });
            let out = conn.shutdown_if_config_differs(Some("{}".into())).unwrap();
            assert!(matches!(
                sent(&log)[2],
                Request::ShutdownIfConfigDiffers {
                    expected_host_id: HOST_A,
                    config_version: None,
                    ..
                }
            ));
            (out, conn.stream.is_some())
        };
        assert_eq!(
            run(Response::Bool(false)),
            (ConditionalShutdown::SameConfig, true)
        );
        assert_eq!(
            run(Response::Bool(true)),
            (ConditionalShutdown::Restarting, false)
        );
        assert_eq!(
            run(Response::ShutdownSkipped),
            (ConditionalShutdown::Skipped, true)
        );
    }
}
