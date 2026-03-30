//! グローバル IME 状態
//!
//! # ロック戦略
//! TSF OnKeyDown はホットパス（UIスレッド）のため、**絶対にブロックしない**。
//! - ホットパス: `try_lock()` のみ使用。取れなければ即リターン。
//! - 非ホットパス（Activate, BG スレッド): `lock()` を使用可。

use super::input_mode::InputMode;
use rakukan_engine_abi::{DynEngine, SegmentCandidate as EngineSegmentCandidate, install_dir};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering as AO};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
    IDXGIAdapter1, IDXGIAdapter3, IDXGIFactory1,
};
use windows::core::{GUID, Interface};

// ─── INPUT_MODE_ATOMIC ────────────────────────────────────────────────────────
// IMEState.input_mode の鏡。ロックなしでホットパス（OnTestKeyDown / OnKeyDown）
// から安全に読み取れるよう AtomicU8 で持つ。
// 値: 0=Hiragana, 1=Katakana, 2=Alphanumeric
// IMEState::set_mode が呼ばれるたびに同期更新される。

static INPUT_MODE_ATOMIC: AtomicU8 = AtomicU8::new(0);

pub fn input_mode_set_atomic(mode: InputMode) {
    let v = match mode {
        InputMode::Hiragana => 0u8,
        InputMode::Katakana => 1u8,
        InputMode::Alphanumeric => 2u8,
    };
    INPUT_MODE_ATOMIC.store(v, AO::Release);
}

/// ロックなし高速読み取り（ホットパス用）
#[inline]
pub fn input_mode_get_atomic() -> InputMode {
    match INPUT_MODE_ATOMIC.load(AO::Acquire) {
        1 => InputMode::Katakana,
        2 => InputMode::Alphanumeric,
        _ => InputMode::Hiragana,
    }
}

// ─── EngineWrapper ────────────────────────────────────────────────────────────
// Safety: TSF は STA で動作し、Mutex で保護するため Send/Sync を許容する。
// ただし ホットパスでは try_lock() しか使わないことを必ず守ること。

pub struct EngineWrapper(pub Option<DynEngine>);
unsafe impl Send for EngineWrapper {}
unsafe impl Sync for EngineWrapper {}

pub static RAKUKAN_ENGINE: LazyLock<Mutex<EngineWrapper>> =
    LazyLock::new(|| Mutex::new(EngineWrapper(None)));

/// バックグラウンドエンジン初期化が既に起動済みかどうかのフラグ。
/// Activate ごとに重複スポーンしないために使う。
static ENGINE_INIT_STARTED: AtomicBool = AtomicBool::new(false);
static LAST_GPU_MEMORY_LOG_MS: AtomicU64 = AtomicU64::new(0);
const GPU_MEMORY_LOG_INTERVAL_MS: u64 = 30_000;

/// エンジン DLL のロードをバックグラウンドスレッドで開始する。
///
/// Activate（UIスレッド）からの呼び出し専用。
/// DLL ロードは重いため（CUDA 初期化で数秒かかることがある）UIスレッドをブロックしない。
/// 既に起動済みの場合は何もしない（二重スポーン防止）。
/// ロード完了後、辞書・モデルのバックグラウンドロードを開始し、
/// `LANGBAR_UPDATE_PENDING` をセットして言語バー表示を更新する。
pub fn engine_start_bg_init() {
    // すでに起動済みなら何もしない（エンジンが既に存在する場合も不要）
    if ENGINE_INIT_STARTED.swap(true, AO::AcqRel) {
        // 既存エンジンで辞書・モデルがまだ未ロードなら起動する
        if let Ok(mut g) = RAKUKAN_ENGINE.try_lock() {
            if let Some(eng) = g.0.as_mut() {
                if !eng.is_dict_ready() {
                    eng.start_load_dict();
                }
                if !eng.is_kanji_ready() {
                    eng.start_load_model();
                }
            }
        }
        tracing::debug!("engine_start_bg_init: already started, skipping DLL load");
        return;
    }
    tracing::info!("engine_start_bg_init: spawning background engine init thread");
    std::thread::Builder::new()
        .name("rakukan-engine-init".into())
        .spawn(|| {
            tracing::info!("engine-init: starting DLL load");
            let load_result = {
                match RAKUKAN_ENGINE.lock() {
                    Ok(mut g) => {
                        if g.0.is_some() {
                            tracing::debug!("engine-init: engine already present, skipping");
                            return;
                        }
                        match create_engine() {
                            Ok(e) => {
                                g.0 = Some(e);
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    }
                    Err(_) => Err(anyhow::anyhow!("engine mutex poisoned")),
                }
            };
            match load_result {
                Ok(()) => {
                    // 辞書・モデルのバックグラウンドロードを起動
                    if let Ok(mut g) = RAKUKAN_ENGINE.lock() {
                        if let Some(eng) = g.0.as_mut() {
                            tracing::debug!(
                                "engine-init: is_dict_ready={} is_kanji_ready={}",
                                eng.is_dict_ready(),
                                eng.is_kanji_ready()
                            );
                            if !eng.is_dict_ready() {
                                tracing::info!("engine-init: calling start_load_dict");
                                eng.start_load_dict();
                            }
                            if !eng.is_kanji_ready() {
                                eng.start_load_model();
                            }
                        }
                    }
                    tracing::info!("engine-init: engine created successfully");
                    // 言語バーのアイコン・ツールチップを更新するよう通知
                    langbar_update_set();
                }
                Err(e) => {
                    tracing::error!("engine-init: DLL load failed: {e}");
                    // 次回 Activate で再試行できるようフラグをリセット
                    ENGINE_INIT_STARTED.store(false, AO::Release);
                }
            }
        })
        .ok();
}

/// ホットパス用: ブロックしない。取れなければ Err を返す。
#[inline]
pub fn engine_try_get() -> anyhow::Result<MutexGuard<'static, EngineWrapper>> {
    RAKUKAN_ENGINE
        .try_lock()
        .map_err(|_| anyhow::anyhow!("engine busy"))
}

/// 非ホットパス用（Activate, BG スレッド）: ブロックあり。poison 回復あり。
pub fn engine_get() -> anyhow::Result<MutexGuard<'static, EngineWrapper>> {
    match RAKUKAN_ENGINE.lock() {
        Ok(g) => Ok(g),
        Err(p) => {
            tracing::warn!("engine mutex poisoned, recovering");
            Ok(p.into_inner())
        }
    }
}

/// エンジンを作成して返す（未初期化の場合のみ作成する）。
/// 現在は engine_start_bg_init() が初期化を担当するため直接呼ばれないが、
/// engine_reload() 等の内部用途・将来の拡張のために残す。
#[allow(dead_code)]
pub fn engine_get_or_create() -> anyhow::Result<MutexGuard<'static, EngineWrapper>> {
    {
        let mut g = engine_get()?;
        if g.0.is_none() {
            let e = create_engine()?;
            g.0 = Some(e);
            drop(g);
            if let Ok(mut g2) = RAKUKAN_ENGINE.lock() {
                if let Some(eng) = g2.0.as_mut() {
                    if !eng.is_dict_ready() {
                        eng.start_load_dict();
                    }
                    if !eng.is_kanji_ready() {
                        eng.start_load_model();
                    }
                }
            }
            return engine_get();
        }
    }
    engine_get()
}

/// ホットパス用: エンジンを取得するだけ（DLL ロードしない）。
/// DLL ロードは engine_start_bg_init() が担当する。
pub fn engine_try_get_or_create() -> anyhow::Result<MutexGuard<'static, EngineWrapper>> {
    engine_try_get()
}

/// エンジンを強制的に破棄し、次回アクセス時に再生成されるようにする。
/// config.toml 変更後や DLL 入れ替え後に使用する。
pub fn engine_force_recreate() {
    match RAKUKAN_ENGINE.lock() {
        Ok(mut g) => {
            tracing::debug!("engine_force_recreate: dropping engine");
            g.0 = None;
        }
        Err(p) => {
            p.into_inner().0 = None;
            tracing::warn!("engine_force_recreate: mutex was poisoned, cleared anyway");
        }
    }
}

/// トレイから「エンジン再起動」が要求されたとき呼ぶ。
/// エンジンを破棄した後、バックグラウンドで再生成を試みる。
pub fn engine_reload() {
    engine_force_recreate();
    // 次回 Activate で engine_start_bg_init が再度スポーンできるようフラグをリセット
    ENGINE_INIT_STARTED.store(false, AO::Release);
    // バックグラウンドで再生成（UIスレッドをブロックしない）
    std::thread::spawn(|| {
        match RAKUKAN_ENGINE.lock() {
            Ok(mut g) => {
                if g.0.is_none() {
                    match create_engine() {
                        Ok(e) => {
                            if !e.is_dict_ready() {
                                // ガード解放前に辞書ロードを起動する必要があるため
                                // drop して再取得
                                g.0 = Some(e);
                                drop(g);
                                if let Ok(mut g2) = RAKUKAN_ENGINE.lock() {
                                    if let Some(eng) = g2.0.as_mut() {
                                        eng.start_load_dict();
                                        if !eng.is_kanji_ready() {
                                            eng.start_load_model();
                                        }
                                    }
                                }
                            } else {
                                g.0 = Some(e);
                            }
                            tracing::info!("engine_reload: engine recreated");
                        }
                        Err(e) => {
                            tracing::error!("engine_reload: create_engine failed: {e}");
                        }
                    }
                }
            }
            Err(_) => tracing::error!("engine_reload: mutex poisoned"),
        }
    });
}

/// 名前付きイベント `Local\rakukan.engine.reload` を監視するバックグラウンドスレッドを起動する。
/// トレイプロセスがこのイベントを SetEvent したとき engine_reload() を呼ぶ。
pub fn start_reload_watcher() {
    std::thread::Builder::new()
        .name("rakukan-reload-watcher".into())
        .spawn(|| {
            use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
            let name: Vec<u16> = "Local\\rakukan.engine.reload\0".encode_utf16().collect();
            let evt = unsafe {
                CreateEventW(
                    None,
                    false, // auto-reset
                    false,
                    windows::core::PCWSTR(name.as_ptr()),
                )
            };
            let evt = match evt {
                Ok(h) => h,
                Err(e) => {
                    tracing::error!("reload_watcher: CreateEventW failed: {e}");
                    return;
                }
            };
            tracing::info!("reload_watcher: listening on Local\\rakukan.engine.reload");
            loop {
                let ret = unsafe { WaitForSingleObject(evt, INFINITE) };
                if ret.0 != 0 {
                    // WAIT_ABANDONED or WAIT_FAILED
                    tracing::error!("reload_watcher: WaitForSingleObject failed ({:?})", ret);
                    break;
                }
                tracing::info!("reload_watcher: reload event received");
                engine_reload();
            }
        })
        .ok();
}

/// バックエンド DLL をロードしてエンジンを生成する。
fn create_engine() -> anyhow::Result<DynEngine> {
    let dir = install_dir().ok_or_else(|| anyhow::anyhow!("install_dir not found"))?;
    tracing::debug!("create_engine: install_dir={}", dir.display());

    // engine DLL の存在を事前確認してわかりやすいエラーを出す
    for backend in &["cpu", "vulkan", "cuda"] {
        let p = dir.join(format!("rakukan_engine_{backend}.dll"));
        tracing::debug!(
            "  {}: {}",
            backend,
            if p.exists() { "found" } else { "not found" }
        );
    }

    let cfg = build_engine_config_json();
    let engine = DynEngine::load_auto(&dir, Some(&cfg))
        .map_err(|e| anyhow::anyhow!("DLL load failed (dir={}): {e}", dir.display()))?;
    tracing::info!("engine created: backend={}", engine.backend_label());
    maybe_log_gpu_memory(&engine);
    Ok(engine)
}

/// %APPDATA%\rakukan\config.toml を読んで EngineConfig JSON を生成する。
fn build_engine_config_json() -> String {
    let cfg = super::config::current_config();
    let num_candidates = cfg.effective_num_candidates();
    let main_gpu = cfg.general.main_gpu;
    let n_gpu_layers = cfg.general.n_gpu_layers.unwrap_or(u32::MAX);
    let model_variant = cfg.general.model_variant.clone();

    tracing::info!(
        "engine config: num_candidates={num_candidates} n_gpu_layers={n_gpu_layers} main_gpu={main_gpu} model_variant={model_variant:?}"
    );
    let mv_json = match &model_variant {
        Some(v) => format!(r#","model_variant":"{}""#, v),
        None => String::new(),
    };
    format!(
        r#"{{"num_candidates":{num_candidates},"n_gpu_layers":{n_gpu_layers},"main_gpu":{main_gpu},"n_threads":0{mv_json}}}"#
    )
}

/// config.toml から num_candidates を読む（ホットパスで使う軽量版）
pub fn get_num_candidates() -> usize {
    super::config::effective_num_candidates()
}

pub fn maybe_log_gpu_memory(engine: &DynEngine) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }

    let backend = engine.backend_label();
    if backend.eq_ignore_ascii_case("cpu") || engine.n_gpu_layers() == 0 {
        return;
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last_ms = LAST_GPU_MEMORY_LOG_MS.load(AO::Acquire);
    if now_ms.saturating_sub(last_ms) < GPU_MEMORY_LOG_INTERVAL_MS {
        return;
    }
    if LAST_GPU_MEMORY_LOG_MS
        .compare_exchange(last_ms, now_ms, AO::AcqRel, AO::Acquire)
        .is_err()
    {
        return;
    }

    let adapter_index = super::config::current_config().general.main_gpu.max(0) as u32;
    match query_local_gpu_memory(adapter_index) {
        Ok((adapter_name, info)) => {
            let used_mb = info.CurrentUsage / (1024 * 1024);
            let budget_mb = info.Budget / (1024 * 1024);
            let available_mb = budget_mb.saturating_sub(used_mb);
            tracing::debug!(
                "gpu memory: backend={} adapter={} used={}MB budget={}MB available={}MB n_gpu_layers={}",
                backend,
                adapter_name,
                used_mb,
                budget_mb,
                available_mb,
                engine.n_gpu_layers()
            );
        }
        Err(err) => {
            tracing::debug!(
                "gpu memory: backend={} adapter_index={} unavailable: {}",
                backend,
                adapter_index,
                err
            );
        }
    }
}

fn query_local_gpu_memory(
    adapter_index: u32,
) -> anyhow::Result<(String, DXGI_QUERY_VIDEO_MEMORY_INFO)> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let adapter1: IDXGIAdapter1 = unsafe { factory.EnumAdapters1(adapter_index)? };
    let adapter_name = unsafe {
        let desc = adapter1.GetDesc1()?;
        wides_to_string(&desc.Description)
    };
    let adapter3: IDXGIAdapter3 = adapter1.cast()?;
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    unsafe { adapter3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)? };
    Ok((adapter_name, info))
}

fn wides_to_string(wides: &[u16]) -> String {
    let len = wides.iter().position(|&c| c == 0).unwrap_or(wides.len());
    String::from_utf16_lossy(&wides[..len])
}

impl std::ops::Deref for EngineWrapper {
    type Target = Option<DynEngine>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for EngineWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// ホットパス用: EngineWrapper の MutexGuard 型エイリアス
pub type EngineGuard = std::sync::MutexGuard<'static, EngineWrapper>;

// ─── IMEState ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct IMEState {
    pub input_mode: InputMode,
    #[allow(dead_code)]
    pub cookies: HashMap<GUID, u32>,
}

pub static IME_STATE: LazyLock<Mutex<IMEState>> = LazyLock::new(|| {
    Mutex::new(IMEState {
        input_mode: InputMode::default(),
        cookies: HashMap::new(),
    })
});

unsafe impl Send for IMEState {}
unsafe impl Sync for IMEState {}

impl IMEState {
    /// ホットパス用: ブロックしない
    #[inline]
    pub fn try_get() -> anyhow::Result<MutexGuard<'static, IMEState>> {
        IME_STATE
            .try_lock()
            .map_err(|_| anyhow::anyhow!("ime_state busy"))
    }

    pub fn set_mode(&mut self, mode: InputMode) {
        tracing::info!("input mode: {:?} → {:?}", self.input_mode, mode);
        self.input_mode = mode;
        // ホットパス用アトミックも同期更新
        input_mode_set_atomic(mode);
    }
}

/// ホットパス用
#[inline]
pub fn ime_state_get() -> anyhow::Result<MutexGuard<'static, IMEState>> {
    IMEState::try_get()
}

// ─── CompositionWrapper ───────────────────────────────────────────────────────

use windows::Win32::UI::TextServices::ITfComposition;

pub(crate) struct CompositionWrapper(pub Option<ITfComposition>);
unsafe impl Send for CompositionWrapper {}
unsafe impl Sync for CompositionWrapper {}

pub static COMPOSITION: LazyLock<Mutex<CompositionWrapper>> =
    LazyLock::new(|| Mutex::new(CompositionWrapper(None)));

/// ホットパス用: ブロックしない
#[inline]
pub fn composition_try_get() -> anyhow::Result<MutexGuard<'static, CompositionWrapper>> {
    COMPOSITION
        .try_lock()
        .map_err(|_| anyhow::anyhow!("composition busy"))
}

pub fn composition_set(comp: Option<ITfComposition>) -> anyhow::Result<()> {
    // set はホットパスでもブロックを許容（短い操作のため）
    let mut g = COMPOSITION.lock().map_err(|p| {
        let _ = p;
        anyhow::anyhow!("composition poison")
    })?;
    g.0 = comp;
    Ok(())
}

pub fn composition_take() -> anyhow::Result<Option<ITfComposition>> {
    let mut g = composition_try_get()?;
    Ok(g.0.take())
}

pub fn composition_clone() -> anyhow::Result<Option<ITfComposition>> {
    let g = composition_try_get()?;
    Ok(g.0.clone())
}

// ─── SessionState ────────────────────────────────────────────────────────────
// TSF 層の論理状態を 1 か所に集約する。SelectionState は縮退・削除済み。

#[derive(Debug, Clone, Default)]
pub enum SessionState {
    #[default]
    Idle,
    Preedit {
        text: String,
    },
    Waiting {
        text: String,
        pos_x: i32,
        pos_y: i32,
    },
    /// 文節分割表示状態。
    /// Left/Right で選択文節を移動、Shift+Left/Right で選択範囲を調整する。
    SplitPreedit {
        conversion: ConversionState,
    },
    Selecting {
        original_preedit: String,
        candidates: Vec<String>,
        structured_candidates: Vec<EngineSegmentCandidate>,
        selected: usize,
        page_size: usize,
        llm_pending: bool,
        pos_x: i32,
        pos_y: i32,
        /// 句読点保留（「、」「。」押下時にセット、確定時に末尾連結）
        punct_pending: Option<char>,
        prefix: String,
        prefix_reading: String,
        /// 文節分割後に変換した場合の残り部分（確定後に次のプリエディットになる）
        remainder: String,
        /// 文節分割後に変換した場合の残り部分の読み
        remainder_reading: String,
        /// SplitPreedit から Selecting に入ったときの前方ブロック。
        split_prefix_blocks: Vec<SplitBlock>,
        /// SplitPreedit から Selecting に入ったときの後方ブロック。
        split_suffix_blocks: Vec<SplitBlock>,
    },
    /// ライブ変換表示中。
    ///
    /// BG 変換が完了しトップ候補を composition に表示している状態。
    /// キーを押していないので候補ウィンドウは出ない（Preedit の視覚的な上書き）。
    ///
    /// - `reading` : エンジンの hiragana_buf（Space 押下時の変換キー）
    /// - `preview` : BG 変換のトップ候補（現在 composition に表示中）
    ///
    /// 遷移:
    ///   Enter        → `preview` をコミット
    ///   Space        → `reading` で on_convert（通常変換フロー）
    ///   Input(c)     → Preedit へ戻し新文字を処理
    ///   Backspace/ESC → Preedit へ戻し `reading` を再表示
    ///   IME オフ     → `preview` をコミット
    LiveConv {
        reading: String,
        preview: String,
    },
}

#[derive(Debug, Clone)]
pub struct SplitBlock {
    pub reading: String,
    pub display: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversionSegment {
    pub reading: String,
    pub surface: String,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ConversionCandidateView {
    pub candidates: Vec<EngineSegmentCandidate>,
    pub selected: usize,
    pub page_size: usize,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ConversionState {
    pub segments: Vec<ConversionSegment>,
    pub focused_index: usize,
    pub candidate_view: Option<ConversionCandidateView>,
}

#[allow(dead_code)]
impl ConversionState {
    pub fn from_split_blocks(blocks: &[SplitBlock], focused_index: usize) -> Option<Self> {
        if blocks.is_empty() {
            return None;
        }
        let segments = blocks
            .iter()
            .map(|block| ConversionSegment {
                reading: block.reading.clone(),
                surface: block.display.clone(),
            })
            .collect::<Vec<_>>();
        let focused_index = focused_index.min(segments.len().saturating_sub(1));
        Some(Self {
            segments,
            focused_index,
            candidate_view: None,
        })
    }

    pub fn focused(&self) -> Option<&ConversionSegment> {
        self.segments.get(self.focused_index)
    }

    pub fn focused_mut(&mut self) -> Option<&mut ConversionSegment> {
        self.segments.get_mut(self.focused_index)
    }

    pub fn composed_surface(&self) -> String {
        self.segments
            .iter()
            .map(|seg| seg.surface.as_str())
            .collect::<String>()
    }

    pub fn composed_reading(&self) -> String {
        self.segments
            .iter()
            .map(|seg| seg.reading.as_str())
            .collect::<String>()
    }

    pub fn display_parts(&self) -> (String, String, String) {
        let focused_index = self.focused_index.min(self.segments.len());
        let prefix = self.segments[..focused_index]
            .iter()
            .map(|seg| seg.surface.as_str())
            .collect::<String>();
        let target = self
            .segments
            .get(focused_index)
            .map(|seg| seg.surface.clone())
            .unwrap_or_default();
        let suffix = self.segments[focused_index.saturating_add(1)..]
            .iter()
            .map(|seg| seg.surface.as_str())
            .collect::<String>();
        (prefix, target, suffix)
    }

    pub fn reading_parts(&self) -> (String, String, String) {
        let focused_index = self.focused_index.min(self.segments.len());
        let prefix = self.segments[..focused_index]
            .iter()
            .map(|seg| seg.reading.as_str())
            .collect::<String>();
        let target = self
            .segments
            .get(focused_index)
            .map(|seg| seg.reading.clone())
            .unwrap_or_default();
        let suffix = self.segments[focused_index.saturating_add(1)..]
            .iter()
            .map(|seg| seg.reading.as_str())
            .collect::<String>();
        (prefix, target, suffix)
    }

    pub fn focus_left(&mut self) -> bool {
        if self.segments.is_empty() {
            return false;
        }
        if self.focused_index == 0 {
            self.focused_index = self.segments.len() - 1;
        } else {
            self.focused_index -= 1;
        }
        self.clear_candidate_view();
        true
    }

    pub fn focus_right(&mut self) -> bool {
        if self.segments.is_empty() {
            return false;
        }
        self.focused_index = (self.focused_index + 1) % self.segments.len();
        self.clear_candidate_view();
        true
    }

    pub fn width_expand_right(&mut self) -> bool {
        if self.focused_index + 1 >= self.segments.len() {
            return false;
        }

        let moved = {
            let next = &mut self.segments[self.focused_index + 1];
            let Some(ch) = take_first_char(&mut next.reading) else {
                return false;
            };
            if next.surface == next.reading {
                next.surface = next.reading.clone();
            }
            ch
        };

        if let Some(current) = self.focused_mut() {
            current.reading.push(moved);
            current.surface = current.reading.clone();
        }

        if self.segments[self.focused_index + 1].reading.is_empty() {
            self.segments.remove(self.focused_index + 1);
        } else {
            self.segments[self.focused_index + 1].surface =
                self.segments[self.focused_index + 1].reading.clone();
        }
        self.clear_candidate_view();
        true
    }

    pub fn width_shrink_right(&mut self) -> bool {
        let Some(current) = self.focused() else {
            return false;
        };
        if current.reading.chars().count() <= 1 {
            return false;
        }

        let moved = {
            let current = self.focused_mut().expect("focused segment must exist");
            let Some(ch) = take_last_char(&mut current.reading) else {
                return false;
            };
            if current.surface == current.reading.clone() + &ch.to_string() {
                current.surface = current.reading.clone();
            }
            ch
        };

        if self.focused_index + 1 < self.segments.len() {
            let next = &mut self.segments[self.focused_index + 1];
            next.reading.insert(0, moved);
            next.surface = next.reading.clone();
        } else {
            self.segments.push(ConversionSegment {
                reading: moved.to_string(),
                surface: moved.to_string(),
            });
        }
        self.clear_candidate_view();
        true
    }

    pub fn set_candidate_view(&mut self, candidates: Vec<EngineSegmentCandidate>) {
        self.candidate_view = Some(ConversionCandidateView {
            candidates,
            selected: 0,
            page_size: 9,
        });
    }

    pub fn clear_candidate_view(&mut self) {
        self.candidate_view = None;
    }

    pub fn has_candidate_view(&self) -> bool {
        self.candidate_view.is_some()
    }

    pub fn candidate_surface(&self) -> Option<&str> {
        let view = self.candidate_view.as_ref()?;
        view.candidates
            .get(view.selected)
            .map(|candidate| candidate.surface.as_str())
    }

    pub fn current_structured_candidate_clone(&self) -> Option<EngineSegmentCandidate> {
        let view = self.candidate_view.as_ref()?;
        view.candidates.get(view.selected).cloned()
    }

    pub fn next_candidate(&mut self) -> bool {
        let Some(view) = self.candidate_view.as_mut() else {
            return false;
        };
        if view.candidates.is_empty() {
            return false;
        }
        view.selected = (view.selected + 1) % view.candidates.len();
        true
    }

    pub fn prev_candidate(&mut self) -> bool {
        let Some(view) = self.candidate_view.as_mut() else {
            return false;
        };
        if view.candidates.is_empty() {
            return false;
        }
        view.selected = if view.selected == 0 {
            view.candidates.len() - 1
        } else {
            view.selected - 1
        };
        true
    }

    pub fn current_page(&self) -> usize {
        self.candidate_view
            .as_ref()
            .map(|view| view.selected / view.page_size)
            .unwrap_or(0)
    }

    pub fn total_pages(&self) -> usize {
        let Some(view) = self.candidate_view.as_ref() else {
            return 0;
        };
        if view.candidates.is_empty() {
            0
        } else {
            view.candidates.len().div_ceil(view.page_size)
        }
    }

    pub fn page_candidates(&self) -> Vec<String> {
        let Some(view) = self.candidate_view.as_ref() else {
            return Vec::new();
        };
        if view.candidates.is_empty() {
            return Vec::new();
        }
        let start = (view.selected / view.page_size) * view.page_size;
        let end = (start + view.page_size).min(view.candidates.len());
        view.candidates[start..end]
            .iter()
            .map(|candidate| candidate.surface.clone())
            .collect()
    }

    pub fn page_selected(&self) -> usize {
        self.candidate_view
            .as_ref()
            .map(|view| view.selected % view.page_size)
            .unwrap_or(0)
    }

    pub fn page_info(&self) -> String {
        let total = self.total_pages();
        if total <= 1 {
            String::new()
        } else {
            format!("{}/{}", self.current_page() + 1, total)
        }
    }

    pub fn next_candidate_with_page_wrap(&mut self) -> bool {
        let Some(view) = self.candidate_view.as_mut() else {
            return false;
        };
        if view.candidates.is_empty() {
            return false;
        }
        let next_idx = (view.selected + 1) % view.candidates.len();
        let cur_page = view.selected / view.page_size;
        let next_page = next_idx / view.page_size;
        view.selected = if next_page != cur_page {
            next_page * view.page_size
        } else {
            next_idx
        };
        true
    }

    pub fn next_candidate_page(&mut self) -> bool {
        let Some(view) = self.candidate_view.as_mut() else {
            return false;
        };
        if view.candidates.is_empty() {
            return false;
        }
        let total_pages = view.candidates.len().div_ceil(view.page_size);
        let cur = view.selected / view.page_size;
        let next = (cur + 1) % total_pages;
        view.selected = next * view.page_size;
        true
    }

    pub fn prev_candidate_page(&mut self) -> bool {
        let Some(view) = self.candidate_view.as_mut() else {
            return false;
        };
        if view.candidates.is_empty() {
            return false;
        }
        let total_pages = view.candidates.len().div_ceil(view.page_size);
        let cur = view.selected / view.page_size;
        let prev = if cur == 0 { total_pages - 1 } else { cur - 1 };
        view.selected = prev * view.page_size;
        true
    }

    pub fn select_nth_candidate_in_page(&mut self, n: usize) -> bool {
        if n < 1 {
            return false;
        }
        let Some(view) = self.candidate_view.as_mut() else {
            return false;
        };
        let idx = (view.selected / view.page_size) * view.page_size + (n - 1);
        if idx < view.candidates.len() {
            view.selected = idx;
            true
        } else {
            false
        }
    }

    pub fn replace_focused_with_candidate(&mut self, candidate: &EngineSegmentCandidate) -> bool {
        if self.segments.is_empty() {
            return false;
        }
        let replacement = candidate
            .segments
            .iter()
            .map(|segment| ConversionSegment {
                reading: segment.reading.clone(),
                surface: segment.surface.clone(),
            })
            .collect::<Vec<_>>();
        if replacement.is_empty() {
            return false;
        }
        let focused_index = self
            .focused_index
            .min(self.segments.len().saturating_sub(1));
        self.segments
            .splice(focused_index..focused_index + 1, replacement);
        self.focused_index = focused_index.min(self.segments.len().saturating_sub(1));
        self.clear_candidate_view();
        true
    }
}

#[allow(dead_code)]
fn take_first_char(text: &mut String) -> Option<char> {
    let ch = text.chars().next()?;
    text.drain(..ch.len_utf8());
    Some(ch)
}

#[allow(dead_code)]
fn take_last_char(text: &mut String) -> Option<char> {
    let ch = text.chars().next_back()?;
    let len = text.len().saturating_sub(ch.len_utf8());
    text.truncate(len);
    Some(ch)
}

fn block_to_segment(block: &SplitBlock) -> ConversionSegment {
    ConversionSegment {
        reading: block.reading.clone(),
        surface: block.display.clone(),
    }
}

fn conversion_from_blocks(
    blocks: Vec<SplitBlock>,
    sel_start: usize,
    sel_end: usize,
) -> Option<ConversionState> {
    if blocks.is_empty() {
        return None;
    }
    let len = blocks.len();
    let sel_start = sel_start.min(len.saturating_sub(1));
    let sel_end = sel_end.clamp(sel_start.saturating_add(1), len);

    let mut segments = Vec::with_capacity(sel_start + 1 + len.saturating_sub(sel_end));
    segments.extend(blocks[..sel_start].iter().map(block_to_segment));
    segments.push(ConversionSegment {
        reading: blocks[sel_start..sel_end]
            .iter()
            .map(|b| b.reading.as_str())
            .collect::<String>(),
        surface: blocks[sel_start..sel_end]
            .iter()
            .map(|b| b.display.as_str())
            .collect::<String>(),
    });
    segments.extend(blocks[sel_end..].iter().map(block_to_segment));

    Some(ConversionState {
        segments,
        focused_index: sel_start,
        candidate_view: None,
    })
}

pub static SESSION_STATE: LazyLock<Mutex<SessionState>> =
    LazyLock::new(|| Mutex::new(SessionState::Idle));

pub static SESSION_SELECTING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

// ─── LIVE_PREVIEW キュー（Phase 1B: WM_TIMER → handle_action 橋渡し）─────────
//
// WM_TIMER コールバック（WndProc）から RequestEditSession が呼べない場合の
// フォールバック。タイマーが変換結果をここに書き込み、次のキー入力時に
// handle_action が読み出して composition を更新する。
//
// Phase 1A（WM_TIMER から直接 RequestEditSession）が確認できれば使わなくなる。

pub static LIVE_PREVIEW_QUEUE: LazyLock<Mutex<Option<String>>> = LazyLock::new(|| Mutex::new(None));
pub static LIVE_PREVIEW_READY: AtomicBool = AtomicBool::new(false);
pub static SUPPRESS_LIVE_COMMIT_ONCE: AtomicBool = AtomicBool::new(false);

pub fn session_get() -> anyhow::Result<MutexGuard<'static, SessionState>> {
    SESSION_STATE
        .lock()
        .map_err(|_| anyhow::anyhow!("session_state poisoned"))
}

#[inline]
pub fn session_is_selecting_fast() -> bool {
    SESSION_SELECTING.load(std::sync::atomic::Ordering::Acquire)
}

impl SessionState {
    pub fn set_idle(&mut self) {
        *self = SessionState::Idle;
        SESSION_SELECTING.store(false, std::sync::atomic::Ordering::Release);
    }

    pub fn set_preedit(&mut self, text: String) {
        *self = SessionState::Preedit { text };
        SESSION_SELECTING.store(false, std::sync::atomic::Ordering::Release);
    }

    /// ライブ変換表示状態へ遷移。
    /// `reading` = hiragana_buf（変換キー）、`preview` = BG トップ候補。
    pub fn set_live_conv(&mut self, reading: String, preview: String) {
        *self = SessionState::LiveConv { reading, preview };
        SESSION_SELECTING.store(false, std::sync::atomic::Ordering::Release);
    }

    pub fn is_live_conv(&self) -> bool {
        matches!(self, SessionState::LiveConv { .. })
    }

    /// LiveConv の (reading, preview) を返す。
    pub fn live_conv_parts(&self) -> Option<(&str, &str)> {
        if let SessionState::LiveConv { reading, preview } = self {
            Some((reading.as_str(), preview.as_str()))
        } else {
            None
        }
    }

    pub fn set_waiting(&mut self, text: String, pos_x: i32, pos_y: i32) {
        *self = SessionState::Waiting { text, pos_x, pos_y };
        SESSION_SELECTING.store(false, std::sync::atomic::Ordering::Release);
    }

    /// 文節分割表示状態へ移行（target=実線、remainder=点線）
    ///
    /// SplitPreedit 中もキーを IME が消費する必要があるため SESSION_SELECTING を true に保つ。
    /// false にすると OnTestKeyDown が FALSE を返し、アプリが Shift+左/右を直接処理して
    /// コンポジション内の文字が消える原因になる。
    pub fn set_split_preedit_blocks(
        &mut self,
        blocks: Vec<SplitBlock>,
        sel_start: usize,
        sel_end: usize,
    ) {
        let conversion =
            conversion_from_blocks(blocks, sel_start, sel_end).unwrap_or_else(|| ConversionState {
                segments: vec![ConversionSegment {
                    reading: String::new(),
                    surface: String::new(),
                }],
                focused_index: 0,
                candidate_view: None,
            });
        *self = SessionState::SplitPreedit { conversion };
        SESSION_SELECTING.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn is_split_preedit(&self) -> bool {
        matches!(self, SessionState::SplitPreedit { .. })
    }

    pub fn split_conversion_clone(&self) -> Option<ConversionState> {
        if let SessionState::SplitPreedit { conversion } = self {
            Some(conversion.clone())
        } else {
            None
        }
    }

    pub fn set_split_conversion(&mut self, conversion: ConversionState) {
        *self = SessionState::SplitPreedit { conversion };
        SESSION_SELECTING.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn split_candidate_active(&self) -> bool {
        matches!(
            self,
            SessionState::SplitPreedit { conversion } if conversion.has_candidate_view()
        )
    }

    pub fn clear_split_candidate_view(&mut self) -> bool {
        if let SessionState::SplitPreedit { conversion } = self {
            if conversion.has_candidate_view() {
                conversion.clear_candidate_view();
                return true;
            }
        }
        false
    }

    pub fn split_target(&self) -> Option<String> {
        if let SessionState::SplitPreedit { conversion } = self {
            Some(conversion.reading_parts().1)
        } else {
            None
        }
    }

    pub fn split_prefix(&self) -> Option<String> {
        if let SessionState::SplitPreedit { conversion } = self {
            Some(conversion.reading_parts().0)
        } else {
            None
        }
    }

    pub fn split_remainder(&self) -> Option<String> {
        if let SessionState::SplitPreedit { conversion } = self {
            Some(conversion.reading_parts().2)
        } else {
            None
        }
    }

    pub fn split_display_prefix(&self) -> Option<String> {
        if let SessionState::SplitPreedit { conversion } = self {
            Some(conversion.display_parts().0)
        } else {
            None
        }
    }

    pub fn split_display_target(&self) -> Option<String> {
        if let SessionState::SplitPreedit { conversion } = self {
            Some(conversion.display_parts().1)
        } else {
            None
        }
    }

    pub fn split_display_remainder(&self) -> Option<String> {
        if let SessionState::SplitPreedit { conversion } = self {
            Some(conversion.display_parts().2)
        } else {
            None
        }
    }

    pub fn split_move_left(&mut self) -> bool {
        if let SessionState::SplitPreedit { conversion } = self {
            conversion.focus_left()
        } else {
            false
        }
    }

    pub fn split_move_right(&mut self) -> bool {
        if let SessionState::SplitPreedit { conversion } = self {
            conversion.focus_right()
        } else {
            false
        }
    }

    /// Shift+Left: 選択範囲の右端を左へ縮める。
    #[allow(dead_code)]
    pub fn split_shrink(&mut self) -> bool {
        if let SessionState::SplitPreedit { conversion } = self {
            conversion.width_shrink_right()
        } else {
            false
        }
    }

    /// Shift+Right: 選択範囲の右端を右へ広げる。
    #[allow(dead_code)]
    pub fn split_extend(&mut self) -> bool {
        if let SessionState::SplitPreedit { conversion } = self {
            conversion.width_expand_right()
        } else {
            false
        }
    }

    pub fn activate_selecting(
        &mut self,
        candidates: Vec<String>,
        original_preedit: String,
        pos_x: i32,
        pos_y: i32,
        llm_pending: bool,
    ) {
        self.activate_selecting_with_affixes(
            candidates,
            original_preedit,
            pos_x,
            pos_y,
            llm_pending,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            Vec::new(),
            Vec::new(),
        );
    }

    pub fn activate_selecting_with_affixes(
        &mut self,
        candidates: Vec<String>,
        original_preedit: String,
        pos_x: i32,
        pos_y: i32,
        llm_pending: bool,
        prefix: String,
        prefix_reading: String,
        remainder: String,
        remainder_reading: String,
        split_prefix_blocks: Vec<SplitBlock>,
        split_suffix_blocks: Vec<SplitBlock>,
    ) {
        *self = SessionState::Selecting {
            original_preedit,
            candidates,
            structured_candidates: Vec::new(),
            selected: 0,
            page_size: 9,
            llm_pending,
            pos_x,
            pos_y,
            punct_pending: None,
            prefix,
            prefix_reading,
            remainder,
            remainder_reading,
            split_prefix_blocks,
            split_suffix_blocks,
        };
        SESSION_SELECTING.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn is_selecting(&self) -> bool {
        matches!(self, SessionState::Selecting { .. })
    }

    pub fn is_candidate_list_active(&self) -> bool {
        self.is_selecting() || self.split_candidate_active()
    }

    pub fn is_waiting(&self) -> bool {
        matches!(self, SessionState::Waiting { .. })
    }

    pub fn preedit_text(&self) -> Option<&str> {
        match self {
            SessionState::Preedit { text } => Some(text.as_str()),
            SessionState::Waiting { text, .. } => Some(text.as_str()),
            SessionState::Selecting {
                original_preedit, ..
            } => Some(original_preedit.as_str()),
            SessionState::SplitPreedit { .. } => None,
            // LiveConv では preview（変換後テキスト）を表示テキストとして返す
            SessionState::LiveConv { preview, .. } => Some(preview.as_str()),
            SessionState::Idle => None,
        }
    }

    pub fn waiting_info(&self) -> Option<(&str, i32, i32)> {
        match self {
            SessionState::Waiting { text, pos_x, pos_y } => Some((text.as_str(), *pos_x, *pos_y)),
            _ => None,
        }
    }

    pub fn current_candidate(&self) -> Option<&str> {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                ..
            } => candidates.get(*selected).map(|s| s.as_str()),
            SessionState::SplitPreedit { conversion } => conversion.candidate_surface(),
            _ => None,
        }
    }

    pub fn current_structured_candidate_clone(&self) -> Option<EngineSegmentCandidate> {
        match self {
            SessionState::Selecting {
                structured_candidates,
                selected,
                ..
            } => structured_candidates.get(*selected).cloned(),
            SessionState::SplitPreedit { conversion } => {
                conversion.current_structured_candidate_clone()
            }
            _ => None,
        }
    }

    pub fn original_preedit(&self) -> Option<&str> {
        match self {
            SessionState::Selecting {
                original_preedit, ..
            } => Some(original_preedit.as_str()),
            SessionState::Preedit { text } => Some(text.as_str()),
            SessionState::Waiting { text, .. } => Some(text.as_str()),
            SessionState::SplitPreedit { .. } => None,
            // LiveConv では reading（ひらがな）が元のプリエディット
            SessionState::LiveConv { reading, .. } => Some(reading.as_str()),
            SessionState::Idle => None,
        }
    }

    /// Selecting 状態の remainder を取り出す（空文字列の場合は空 String）
    pub fn take_selecting_remainder(&mut self) -> String {
        if let SessionState::Selecting { remainder, .. } = self {
            std::mem::take(remainder)
        } else {
            String::new()
        }
    }

    /// Selecting 状態の remainder を参照する（コピーを返す）
    pub fn selecting_remainder_clone(&self) -> String {
        if let SessionState::Selecting { remainder, .. } = self {
            remainder.clone()
        } else {
            String::new()
        }
    }

    pub fn selecting_remainder_reading_clone(&self) -> String {
        if let SessionState::Selecting {
            remainder_reading, ..
        } = self
        {
            remainder_reading.clone()
        } else {
            String::new()
        }
    }

    pub fn selecting_prefix_clone(&self) -> String {
        if let SessionState::Selecting { prefix, .. } = self {
            prefix.clone()
        } else {
            String::new()
        }
    }

    pub fn selecting_prefix_reading_clone(&self) -> String {
        if let SessionState::Selecting { prefix_reading, .. } = self {
            prefix_reading.clone()
        } else {
            String::new()
        }
    }

    pub fn selecting_prefix_blocks_clone(&self) -> Vec<SplitBlock> {
        if let SessionState::Selecting {
            split_prefix_blocks,
            ..
        } = self
        {
            split_prefix_blocks.clone()
        } else {
            Vec::new()
        }
    }

    pub fn selecting_suffix_blocks_clone(&self) -> Vec<SplitBlock> {
        if let SessionState::Selecting {
            split_suffix_blocks,
            ..
        } = self
        {
            split_suffix_blocks.clone()
        } else {
            Vec::new()
        }
    }

    pub fn current_page(&self) -> usize {
        match self {
            SessionState::Selecting {
                selected,
                page_size,
                ..
            } => selected / page_size,
            SessionState::SplitPreedit { conversion } => conversion.current_page(),
            _ => 0,
        }
    }

    pub fn total_pages(&self) -> usize {
        match self {
            SessionState::Selecting {
                candidates,
                page_size,
                ..
            } => {
                if candidates.is_empty() {
                    0
                } else {
                    (candidates.len() + page_size - 1) / page_size
                }
            }
            SessionState::SplitPreedit { conversion } => conversion.total_pages(),
            _ => 0,
        }
    }

    pub fn page_candidates(&self) -> Vec<String> {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                page_size,
                ..
            } => {
                if candidates.is_empty() {
                    return Vec::new();
                }
                let start = (selected / page_size) * page_size;
                let end = (start + page_size).min(candidates.len());
                candidates[start..end].to_vec()
            }
            SessionState::SplitPreedit { conversion } => conversion.page_candidates(),
            _ => Vec::new(),
        }
    }

    pub fn page_selected(&self) -> usize {
        match self {
            SessionState::Selecting {
                selected,
                page_size,
                ..
            } => selected % page_size,
            SessionState::SplitPreedit { conversion } => conversion.page_selected(),
            _ => 0,
        }
    }

    pub fn page_info(&self) -> String {
        let total = self.total_pages();
        if total <= 1 {
            String::new()
        } else {
            format!("{}/{}", self.current_page() + 1, total)
        }
    }

    pub fn next_with_page_wrap(&mut self) {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                page_size,
                ..
            } => {
                if candidates.is_empty() {
                    return;
                }
                let next_idx = (*selected + 1) % candidates.len();
                let cur_page = *selected / *page_size;
                let next_page = next_idx / *page_size;
                *selected = if next_page != cur_page {
                    next_page * *page_size
                } else {
                    next_idx
                };
            }
            SessionState::SplitPreedit { conversion } => {
                let _ = conversion.next_candidate_with_page_wrap();
            }
            _ => {}
        }
    }

    pub fn prev(&mut self) {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                ..
            } => {
                if candidates.is_empty() {
                    return;
                }
                *selected = if *selected == 0 {
                    candidates.len() - 1
                } else {
                    *selected - 1
                };
            }
            SessionState::SplitPreedit { conversion } => {
                let _ = conversion.prev_candidate();
            }
            _ => {}
        }
    }

    pub fn next_page(&mut self) {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                page_size,
                ..
            } => {
                if candidates.is_empty() {
                    return;
                }
                let total_pages = candidates.len().div_ceil(*page_size);
                let cur = *selected / *page_size;
                let next = (cur + 1) % total_pages;
                *selected = next * *page_size;
            }
            SessionState::SplitPreedit { conversion } => {
                let _ = conversion.next_candidate_page();
            }
            _ => {}
        }
    }

    pub fn prev_page(&mut self) {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                page_size,
                ..
            } => {
                if candidates.is_empty() {
                    return;
                }
                let total_pages = candidates.len().div_ceil(*page_size);
                let cur = *selected / *page_size;
                let prev = if cur == 0 { total_pages - 1 } else { cur - 1 };
                *selected = prev * *page_size;
            }
            SessionState::SplitPreedit { conversion } => {
                let _ = conversion.prev_candidate_page();
            }
            _ => {}
        }
    }

    pub fn select_nth_in_page(&mut self, n: usize) -> bool {
        if n < 1 {
            return false;
        }
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                page_size,
                ..
            } => {
                let idx = (*selected / *page_size) * *page_size + (n - 1);
                if idx < candidates.len() {
                    *selected = idx;
                    true
                } else {
                    false
                }
            }
            SessionState::SplitPreedit { conversion } => conversion.select_nth_candidate_in_page(n),
            _ => false,
        }
    }

    /// 句読点保留をセット（確定時に変換テキスト末尾に連結する）
    pub fn set_punct_pending(&mut self, c: char) {
        if let SessionState::Selecting { punct_pending, .. } = self {
            *punct_pending = Some(c);
        }
    }

    /// 句読点保留を取り出す
    pub fn take_punct_pending(&mut self) -> Option<char> {
        if let SessionState::Selecting { punct_pending, .. } = self {
            punct_pending.take()
        } else {
            None
        }
    }

    /// Selecting 状態での pos_x, pos_y を返す
    pub fn selecting_pos(&self) -> Option<(i32, i32)> {
        if let SessionState::Selecting { pos_x, pos_y, .. } = self {
            Some((*pos_x, *pos_y))
        } else {
            None
        }
    }
}

// ─── CARET_RECT ──────────────────────────────────────────────────────────────
// GetTextExt で取得したキャレット矩形をEditSession→handlerに渡す橋渡し用。
// RECT は外部クレートの型なので newtype でラップして Send/Sync を実装する。

use windows::Win32::Foundation::RECT;

pub(crate) struct CaretRect(RECT);
unsafe impl Send for CaretRect {}
unsafe impl Sync for CaretRect {}

pub static CARET_RECT: LazyLock<Mutex<CaretRect>> =
    LazyLock::new(|| Mutex::new(CaretRect(RECT::default())));

pub fn caret_rect_set(r: RECT) {
    if let Ok(mut g) = CARET_RECT.lock() {
        g.0 = r;
    }
}

pub fn caret_rect_get() -> RECT {
    CARET_RECT.lock().map(|g| g.0).unwrap_or_default()
}

// ─── LangBar 更新通知 ─────────────────────────────────────────────────────────
// バックグラウンドスレッドでエンジン初期化が完了したとき、
// 言語バー表示を更新するためのフラグ。
// STA スレッドが次回キー入力時にこれを確認して OnUpdate を呼ぶ。

use std::sync::atomic::Ordering as AtomicOrdering;

pub static LANGBAR_UPDATE_PENDING: AtomicBool = AtomicBool::new(false);

#[allow(dead_code)]
pub fn langbar_update_set() {
    LANGBAR_UPDATE_PENDING.store(true, AtomicOrdering::Release);
}
pub fn langbar_update_take() -> bool {
    LANGBAR_UPDATE_PENDING.swap(false, AtomicOrdering::AcqRel)
}

// ─── DocumentManager モードストア ────────────────────────────────────────────
//
// MS-IME準拠: アプリ（DocumentManager）ごとに InputMode を記憶する。
//
// # キー戦略
// Edge・Firefox 等のブラウザはページ遷移やタブ切り替えのたびに
// DocumentManager を新規作成・破棄する。DM ポインタだけをキーにすると
// DM が再作成されるたびにモードがリセットされる。
//
// そのため DM ポインタと HWND の 2 段階フォールバックを採用する:
//   1. dm_modes: DM ポインタ → モード（正確なマッチ）
//   2. hwnd_modes: HWND → モード（DM 再作成時のフォールバック）
//   3. dm_to_hwnd: DM ポインタ → HWND（保存時に HWND も更新するために必要）
//
// モードの保存タイミング（focus が離れるとき）:
//   - dm_modes[prev_dm_ptr] = 現在モード
//   - dm_to_hwnd で prev_dm_ptr の HWND を引いて hwnd_modes[hwnd] = 現在モード
//
// モードの復元タイミング（focus が来るとき）:
//   - dm_modes に next_dm_ptr が存在: それを返す
//   - なければ hwnd_modes に next_hwnd が存在: それを返す（ブラウザの DM 再作成対応）
//   - なければ config.input.default_mode を返す

struct ModeStore {
    dm_modes: HashMap<usize, InputMode>,   // DM ptr → mode
    hwnd_modes: HashMap<usize, InputMode>, // HWND → mode（DM 再作成時フォールバック）
    dm_to_hwnd: HashMap<usize, usize>,     // DM ptr → HWND（保存時の HWND 特定用）
}

static DOC_MODE_STORE: LazyLock<Mutex<ModeStore>> = LazyLock::new(|| {
    Mutex::new(ModeStore {
        dm_modes: HashMap::new(),
        hwnd_modes: HashMap::new(),
        dm_to_hwnd: HashMap::new(),
    })
});

/// DocumentManager のフォーカス変化時に呼ぶ。
///
/// - `prev_dm_ptr`: フォーカスを失った DocumentManager のポインタ（0 = なし）
/// - `next_dm_ptr`: フォーカスを得た DocumentManager のポインタ（0 = なし）
/// - `next_hwnd`: フォーカス先ウィンドウの HWND（ターミナル判定用）
///
/// 返り値: フォーカス先に適用すべき InputMode
pub fn doc_mode_on_focus_change(
    prev_dm_ptr: usize,
    next_dm_ptr: usize,
    next_hwnd: usize,
) -> Option<InputMode> {
    use super::config::{DefaultInputMode, current_config};

    let cfg = current_config();
    let remember = cfg.input.remember_last_kana_mode;

    // config.input.default_mode → InputMode へ変換
    let config_default = match cfg.input.default_mode {
        DefaultInputMode::Alphanumeric => InputMode::Alphanumeric,
        DefaultInputMode::Hiragana => InputMode::Hiragana,
    };

    let mut store = match DOC_MODE_STORE.try_lock() {
        Ok(g) => g,
        Err(_) => return None,
    };

    // 前の DocumentManager のモードを保存
    if prev_dm_ptr != 0 && remember {
        let mode = input_mode_get_atomic();
        store.dm_modes.insert(prev_dm_ptr, mode);
        // HWND も更新（ブラウザが DM を再作成しても HWND 経由で復元できるように）
        if let Some(&hwnd) = store.dm_to_hwnd.get(&prev_dm_ptr) {
            if hwnd != 0 {
                store.hwnd_modes.insert(hwnd, mode);
                tracing::debug!(
                    "doc_mode: saved mode={mode:?} for dm={prev_dm_ptr:#x} hwnd={hwnd:#x}"
                );
            }
        } else {
            tracing::debug!("doc_mode: saved mode={mode:?} for dm={prev_dm_ptr:#x} (hwnd unknown)");
        }
    }

    if next_dm_ptr == 0 {
        return None;
    }

    // DM→HWND マッピングを更新（フォーカスが来るたびに記録）
    if next_hwnd != 0 {
        store.dm_to_hwnd.insert(next_dm_ptr, next_hwnd);
    }

    // 初回フォーカス時のデフォルトモードを決定
    // ターミナルは config に関わらず常に Alphanumeric
    let resolve_default = |hwnd: usize| -> InputMode {
        if is_terminal_hwnd(hwnd) {
            tracing::debug!("doc_mode: terminal detected (hwnd={hwnd:#x}), default=Alphanumeric");
            InputMode::Alphanumeric
        } else {
            tracing::debug!("doc_mode: default={config_default:?} (config.input.default_mode)");
            config_default
        }
    };

    let mode = if remember {
        if let Some(&saved) = store.dm_modes.get(&next_dm_ptr) {
            // 既知の DM → 前回モードを復元
            tracing::debug!("doc_mode: restored mode={saved:?} from dm={next_dm_ptr:#x}");
            saved
        } else if let Some(&saved) = store.hwnd_modes.get(&next_hwnd) {
            // DM は新規だが同じ HWND → HWND 経由で復元（ブラウザの DM 再作成対応）
            tracing::debug!(
                "doc_mode: restored mode={saved:?} from hwnd={next_hwnd:#x} (dm={next_dm_ptr:#x} is new)"
            );
            store.dm_modes.insert(next_dm_ptr, saved);
            saved
        } else {
            // 完全初回 → デフォルトモードを記録して返す
            let m = resolve_default(next_hwnd);
            store.dm_modes.insert(next_dm_ptr, m);
            if next_hwnd != 0 {
                store.hwnd_modes.insert(next_hwnd, m);
            }
            m
        }
    } else {
        // remember=false: 毎回デフォルトモードを適用
        resolve_default(next_hwnd)
    };

    Some(mode)
}

/// DocumentManager が破棄されたとき（OnUninitDocumentMgr）にエントリを削除する。
/// hwnd_modes は残す（同じ HWND で DM が再作成されたとき復元に使うため）。
pub fn doc_mode_remove(dm_ptr: usize) {
    if let Ok(mut store) = DOC_MODE_STORE.try_lock() {
        store.dm_modes.remove(&dm_ptr);
        store.dm_to_hwnd.remove(&dm_ptr);
        tracing::trace!("doc_mode: removed dm={dm_ptr:#x}");
    }
}

/// HWND がターミナル系ウィンドウかどうかを判定する。
///
/// 判定対象:
/// - Windows Terminal: `CASCADIA_HOSTING_WINDOW_CLASS`
/// - 旧来の ConHost:   `ConsoleWindowClass`
/// - VSCode 統合ターミナル等は親が上記クラスを持つ場合あり（簡易判定のみ）
fn is_terminal_hwnd(hwnd_val: usize) -> bool {
    if hwnd_val == 0 {
        return false;
    }

    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

    let hwnd = HWND(hwnd_val as *mut _);
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    if len == 0 {
        return false;
    }

    let class_name = String::from_utf16_lossy(&buf[..len]);
    tracing::trace!("doc_mode: hwnd={hwnd_val:#x} class={class_name:?}");

    matches!(
        class_name.as_str(),
        "CASCADIA_HOSTING_WINDOW_CLASS"  // Windows Terminal
        | "ConsoleWindowClass"           // conhost.exe
        | "VirtualConsoleClass"          // mintty 等
        | "mintty"
    )
}

#[cfg(test)]
mod tests {
    use super::{ConversionState, SessionState, SplitBlock};

    fn ascii_blocks() -> Vec<SplitBlock> {
        vec![
            SplitBlock {
                reading: "a".into(),
                display: "A".into(),
            },
            SplitBlock {
                reading: "b".into(),
                display: "B".into(),
            },
            SplitBlock {
                reading: "c".into(),
                display: "C".into(),
            },
            SplitBlock {
                reading: "d".into(),
                display: "D".into(),
            },
        ]
    }

    fn sample_blocks() -> Vec<SplitBlock> {
        vec![
            SplitBlock {
                reading: "わがはい".into(),
                display: "吾輩".into(),
            },
            SplitBlock {
                reading: "は".into(),
                display: "は".into(),
            },
            SplitBlock {
                reading: "ねこ".into(),
                display: "猫".into(),
            },
            SplitBlock {
                reading: "で".into(),
                display: "で".into(),
            },
            SplitBlock {
                reading: "ある".into(),
                display: "ある".into(),
            },
        ]
    }

    #[test]
    fn split_move_right_moves_selection_window() {
        let mut sess = SessionState::Idle;
        sess.set_split_preedit_blocks(sample_blocks(), 0, 1);

        assert!(sess.split_move_right());
        assert_eq!(sess.split_display_prefix().as_deref(), Some("吾輩"));
        assert_eq!(sess.split_display_target().as_deref(), Some("は"));
        assert_eq!(sess.split_display_remainder().as_deref(), Some("猫である"));

        assert!(sess.split_move_right());
        assert_eq!(sess.split_display_prefix().as_deref(), Some("吾輩は"));
        assert_eq!(sess.split_display_target().as_deref(), Some("猫"));
        assert_eq!(sess.split_display_remainder().as_deref(), Some("である"));
    }

    #[test]
    fn split_extend_grows_selection_without_losing_anchor() {
        let mut sess = SessionState::Idle;
        sess.set_split_preedit_blocks(sample_blocks(), 0, 1);

        assert!(sess.split_extend());
        assert_eq!(sess.split_display_prefix().as_deref(), Some(""));
        assert_eq!(sess.split_display_target().as_deref(), Some("吾輩は"));
        assert_eq!(sess.split_display_remainder().as_deref(), Some("猫である"));

        assert!(sess.split_extend());
        assert_eq!(sess.split_display_target().as_deref(), Some("吾輩は猫"));
        assert_eq!(sess.split_display_remainder().as_deref(), Some("である"));
    }

    #[test]
    fn split_shrink_keeps_at_least_one_block_selected() {
        let mut sess = SessionState::Idle;
        sess.set_split_preedit_blocks(sample_blocks(), 0, 3);

        assert!(sess.split_shrink());
        assert_eq!(sess.split_display_target().as_deref(), Some("吾輩は"));
        assert!(sess.split_shrink());
        assert_eq!(sess.split_display_target().as_deref(), Some("吾輩"));
        assert!(!sess.split_shrink());
        assert_eq!(sess.split_display_target().as_deref(), Some("吾輩"));
    }

    #[test]
    fn split_move_right_stops_at_last_block() {
        let mut sess = SessionState::Idle;
        sess.set_split_preedit_blocks(sample_blocks(), 4, 5);

        assert!(!sess.split_move_right());
        assert_eq!(sess.split_display_prefix().as_deref(), Some("吾輩は猫で"));
        assert_eq!(sess.split_display_target().as_deref(), Some("ある"));
        assert_eq!(sess.split_display_remainder().as_deref(), Some(""));
    }

    #[test]
    fn split_operations_keep_full_text_and_valid_selection_after_repetition() {
        let mut sess = SessionState::Idle;
        sess.set_split_preedit_blocks(ascii_blocks(), 0, 1);

        assert!(sess.split_extend());
        assert!(sess.split_move_right());
        assert!(sess.split_shrink());
        assert!(sess.split_move_right());
        assert!(sess.split_extend());
        assert!(sess.split_move_left());

        let full = format!(
            "{}{}{}",
            sess.split_display_prefix().unwrap_or_default(),
            sess.split_display_target().unwrap_or_default(),
            sess.split_display_remainder().unwrap_or_default()
        );
        assert_eq!(full, "ABCD");

        let target = sess.split_display_target().unwrap_or_default();
        assert!(!target.is_empty());
        assert_eq!(sess.split_display_prefix().as_deref(), Some("A"));
        assert_eq!(sess.split_display_target().as_deref(), Some("BC"));
        assert_eq!(sess.split_display_remainder().as_deref(), Some("D"));
    }

    #[test]
    fn conversion_state_focus_moves_with_wraparound() {
        let blocks = vec![
            SplitBlock {
                reading: "あ".into(),
                display: "亜".into(),
            },
            SplitBlock {
                reading: "い".into(),
                display: "伊".into(),
            },
            SplitBlock {
                reading: "う".into(),
                display: "宇".into(),
            },
        ];
        let mut conv = ConversionState::from_split_blocks(&blocks, 0).unwrap();

        assert_eq!(
            conv.display_parts(),
            ("".into(), "亜".into(), "伊宇".into())
        );
        assert!(conv.focus_left());
        assert_eq!(
            conv.display_parts(),
            ("亜伊".into(), "宇".into(), "".into())
        );
        assert!(conv.focus_right());
        assert_eq!(
            conv.display_parts(),
            ("".into(), "亜".into(), "伊宇".into())
        );
    }

    #[test]
    fn conversion_state_width_expand_and_shrink_move_one_character() {
        let blocks = vec![
            SplitBlock {
                reading: "あ".into(),
                display: "あ".into(),
            },
            SplitBlock {
                reading: "いう".into(),
                display: "いう".into(),
            },
        ];
        let mut conv = ConversionState::from_split_blocks(&blocks, 0).unwrap();

        assert!(conv.width_expand_right());
        assert_eq!(
            conv.reading_parts(),
            ("".into(), "あい".into(), "う".into())
        );

        assert!(conv.width_shrink_right());
        assert_eq!(
            conv.reading_parts(),
            ("".into(), "あ".into(), "いう".into())
        );
    }
}
