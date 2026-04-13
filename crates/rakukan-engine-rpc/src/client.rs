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

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::codec::{read_frame, write_frame};
use crate::pipe::{PipeStream, pipe_name_for_current_user};
use crate::protocol::{PROTOCOL_VERSION, Request, Response, PIPE_BASE_NAME};
use rakukan_engine_abi::{SegmentBlock, SegmentCandidate};

/// ホスト実行ファイル名。インストールディレクトリ直下に配置されている前提。
pub const HOST_EXE_NAME: &str = "rakukan-engine-host.exe";

pub struct RpcEngine {
    inner: Mutex<Connection>,
}

struct Connection {
    stream: Option<PipeStream>,
    /// 直近で使った EngineConfig JSON。
    /// パイプが切れて再接続するとき、ホストがちょうど再起動していたケースでは
    /// Create を送り直す必要がある。そのときに使う。
    /// `reload()` を呼ぶと新しい config で上書きされる。
    config_json: Option<String>,
}

impl RpcEngine {
    /// 接続だけ試行して生成する。config_json は Create リクエストで送られる。
    pub fn connect_or_spawn(config_json: Option<String>) -> Result<Self> {
        let mut conn = Connection {
            stream: None,
            config_json,
        };
        conn.ensure_connected()?;
        Ok(Self {
            inner: Mutex::new(conn),
        })
    }

    /// ホスト側の DynEngine を新しい config_json で再生成する。
    /// TSF の `engine_reload()` から呼ばれる。
    ///
    /// 接続 (PipeStream) は使い回したまま、`Request::Reload` を送るだけ。
    /// 成功後は以降の再接続でも新しい config_json が使われるよう内部に保存する。
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

    fn call_segments(&self, req: Request) -> Result<Vec<SegmentCandidate>> {
        match self.call(req)? {
            Response::Segments(v) => Ok(v),
            Response::Error(e) => bail!("rpc error: {e}"),
            other => bail!("unexpected response: {:?}", other),
        }
    }

    fn call_segment_blocks(&self, req: Request) -> Result<Vec<SegmentBlock>> {
        match self.call(req)? {
            Response::SegmentBlocks(v) => Ok(v),
            Response::Error(e) => bail!("rpc error: {e}"),
            other => bail!("unexpected response: {:?}", other),
        }
    }

    // ── DynEngine 互換 API ──────────────────────────────────────────────

    pub fn push_char(&self, c: char) {
        let _ = self.call_unit(Request::PushChar(c as u32));
    }
    pub fn push_raw(&self, c: char) {
        let _ = self.call_unit(Request::PushRaw(c as u32));
    }
    pub fn push_fullwidth_alpha(&self, c: char) {
        let _ = self.call_unit(Request::PushFullwidthAlpha(c as u32));
    }
    pub fn backspace(&self) -> bool {
        self.call_bool(Request::Backspace).unwrap_or(false)
    }
    pub fn flush_pending_n(&self) -> bool {
        self.call_bool(Request::FlushPendingN).unwrap_or(false)
    }

    pub fn preedit_display(&self) -> String {
        self.call_string(Request::PreeditDisplay).unwrap_or_default()
    }
    pub fn preedit_is_empty(&self) -> bool {
        self.call_bool(Request::PreeditIsEmpty).unwrap_or(true)
    }
    pub fn hiragana_text(&self) -> String {
        self.call_string(Request::HiraganaText).unwrap_or_default()
    }
    pub fn romaji_log_str(&self) -> String {
        self.call_string(Request::RomajiLogStr).unwrap_or_default()
    }
    pub fn hiragana_from_romaji_log(&self) -> String {
        self.call_string(Request::HiraganaFromRomajiLog)
            .unwrap_or_default()
    }
    pub fn committed_text(&self) -> String {
        self.call_string(Request::CommittedText).unwrap_or_default()
    }

    pub fn bg_start(&self, n_cands: usize) -> bool {
        self.call_bool(Request::BgStart { n_cands: n_cands as u32 })
            .unwrap_or(false)
    }
    /// `DynEngine::bg_status` との互換のため `&'static str` を返す。
    /// エンジンが返しうる状態は有限なので既知値に正規化し、それ以外は "unknown"。
    pub fn bg_status(&self) -> &'static str {
        let s = self.call_string(Request::BgStatus).unwrap_or_default();
        match s.as_str() {
            "idle" => "idle",
            "running" => "running",
            "done" => "done",
            "pending" => "pending",
            "error" => "error",
            _ => "unknown",
        }
    }
    pub fn bg_take_candidates(&self, key: &str) -> Option<Vec<String>> {
        match self.call_strings(Request::BgTakeCandidates { key: key.into() }) {
            Ok(v) if !v.is_empty() => Some(v),
            _ => None,
        }
    }
    pub fn bg_take_segmented_candidates(&self, key: &str) -> Option<Vec<SegmentCandidate>> {
        match self.call_segments(Request::BgTakeSegmentedCandidates { key: key.into() }) {
            Ok(v) if !v.is_empty() => Some(v),
            _ => None,
        }
    }
    pub fn bg_reclaim(&self) {
        let _ = self.call_unit(Request::BgReclaim);
    }
    pub fn bg_wait_ms(&self, timeout_ms: u64) -> bool {
        self.call_bool(Request::BgWaitMs { timeout_ms }).unwrap_or(false)
    }

    pub fn commit(&self, text: &str) {
        let _ = self.call_unit(Request::Commit { text: text.into() });
    }
    pub fn commit_as_hiragana(&self) {
        let _ = self.call_unit(Request::CommitAsHiragana);
    }
    pub fn reset_preedit(&self) {
        let _ = self.call_unit(Request::ResetPreedit);
    }
    pub fn force_preedit(&self, text: String) {
        let _ = self.call_unit(Request::ForcePreedit { text });
    }
    pub fn reset_all(&self) {
        let _ = self.call_unit(Request::ResetAll);
    }

    pub fn convert_sync(&self) -> Vec<String> {
        self.call_strings(Request::ConvertSync).unwrap_or_default()
    }
    pub fn convert_sync_segmented(&self) -> Vec<SegmentCandidate> {
        self.call_segments(Request::ConvertSyncSegmented).unwrap_or_default()
    }
    pub fn merge_candidates(&self, llm_cands: Vec<String>, limit: usize) -> Vec<String> {
        self.call_strings(Request::MergeCandidates {
            llm_cands,
            limit: limit as u32,
        })
        .unwrap_or_default()
    }
    pub fn segment_surface(&self, surface: &str) -> Vec<String> {
        self.call_strings(Request::SegmentSurface { surface: surface.into() })
            .unwrap_or_default()
    }
    pub fn segment_candidate(&self, surface: &str, reading: &str) -> Vec<SegmentBlock> {
        self.call_segment_blocks(Request::SegmentCandidate {
            surface: surface.into(),
            reading: reading.into(),
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

    pub fn learn(&self, reading: &str, surface: &str) {
        let _ = self.call_unit(Request::Learn {
            reading: reading.into(),
            surface: surface.into(),
        });
    }
    pub fn last_error(&self) -> String {
        self.call_string(Request::LastError).unwrap_or_default()
    }
    pub fn dict_status(&self) -> String {
        self.call_string(Request::DictStatus).unwrap_or_default()
    }
}

impl Connection {
    fn call_with_retry(&mut self, req: Request) -> Result<Response> {
        for attempt in 0..2 {
            if self.stream.is_none() {
                if let Err(e) = self.ensure_connected() {
                    if attempt == 1 {
                        return Err(e);
                    }
                    continue;
                }
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
    /// `config_json` は `self.config_json` を使う。これにより、ホストが一度クラッシュして
    /// 新プロセスで立ち上がり直したケースでも、直近の `reload()` で指定された設定で
    /// Create され直すため、古い config に巻き戻ることがない。
    fn ensure_connected(&mut self) -> Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }
        let pipe_name = pipe_name_for_current_user();

        // 1. まず接続を試行
        match PipeStream::connect_client(&pipe_name, Duration::from_millis(300)) {
            Ok(s) => {
                self.stream = Some(s);
            }
            Err(_) => {
                // 2. 失敗: ホストを spawn してから再接続
                if let Err(e) = spawn_host() {
                    tracing::warn!("spawn_host failed: {e}");
                }
                let s = PipeStream::connect_client(&pipe_name, Duration::from_secs(5))
                    .with_context(|| format!("connect after spawn to {pipe_name}"))?;
                self.stream = Some(s);
            }
        }

        // 3. Hello 交換
        let s = self.stream.as_mut().expect("connected");
        write_frame(
            s,
            &Request::Hello {
                protocol_version: PROTOCOL_VERSION,
            },
        )?;
        match read_frame::<_, Response>(s)? {
            Response::Hello { protocol_version } if protocol_version == PROTOCOL_VERSION => {}
            Response::Hello { protocol_version } => {
                bail!("protocol version mismatch: server={protocol_version}")
            }
            Response::Error(e) => bail!("hello error: {e}"),
            other => bail!("unexpected hello response: {:?}", other),
        }

        // 4. Create（保存済み config_json を使う）
        write_frame(
            s,
            &Request::Create {
                config_json: self.config_json.clone(),
            },
        )?;
        match read_frame::<_, Response>(s)? {
            Response::Unit => Ok(()),
            Response::Error(e) => bail!("create error: {e}"),
            other => bail!("unexpected create response: {:?}", other),
        }
    }
}

/// `rakukan-engine-host.exe` を install_dir から detached で起動する。
fn spawn_host() -> Result<()> {
    let install = rakukan_engine_abi::install_dir().ok_or_else(|| anyhow!("install_dir not found"))?;
    let exe = install.join(HOST_EXE_NAME);
    if !exe.exists() {
        bail!("host exe not found: {}", exe.display());
    }
    spawn_detached(&exe)
}

#[cfg(target_os = "windows")]
fn spawn_detached(exe: &PathBuf) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    std::process::Command::new(exe)
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .spawn()
        .with_context(|| format!("spawn {}", exe.display()))?;
    Ok(())
}

#[cfg(not(target_os = "windows"))]
fn spawn_detached(_exe: &PathBuf) -> Result<()> {
    bail!("only windows is supported");
}

// 未使用 import 警告回避
#[allow(dead_code)]
const _: &str = PIPE_BASE_NAME;
