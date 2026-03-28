//! グローバル IME 状態
//!
//! # ロック戦略
//! TSF OnKeyDown はホットパス（UIスレッド）のため、**絶対にブロックしない**。
//! - ホットパス: `try_lock()` のみ使用。取れなければ即リターン。
//! - 非ホットパス（Activate, BG スレッド): `lock()` を使用可。

use super::input_mode::InputMode;
use rakukan_engine_abi::{DynEngine, install_dir};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering as AO};
use std::sync::{LazyLock, Mutex, MutexGuard};
use windows::core::GUID;

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
    Ok(engine)
}

/// %APPDATA%\rakukan\config.toml を読んで EngineConfig JSON を生成する。
fn build_engine_config_json() -> String {
    let cfg = super::config::current_config();
    let num_candidates = cfg.effective_num_candidates();
    let main_gpu = cfg.general.main_gpu;
    let n_gpu_layers: u32 = u32::MAX; // DLL 側（cpu DLL は 0 に上書き）
    let model_variant = cfg.general.model_variant.clone();

    tracing::info!(
        "engine config: num_candidates={num_candidates} main_gpu={main_gpu} model_variant={model_variant:?}"
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
        blocks: Vec<SplitBlock>,
        sel_start: usize,
        sel_end: usize,
    },
    Selecting {
        original_preedit: String,
        original_surface: String,
        candidates: Vec<String>,
        selected: usize,
        page_size: usize,
        llm_pending: bool,
        pos_x: i32,
        pos_y: i32,
        /// 句読点保留（「、」「。」押下時にセット、確定時に末尾連結）
        punct_pending: Option<char>,
        prefix: String,
        /// 文節分割後に変換した場合の残り部分（確定後に次のプリエディットになる）
        remainder: String,
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

fn split_reading_parts(
    blocks: &[SplitBlock],
    sel_start: usize,
    sel_end: usize,
) -> (String, String, String) {
    let sel_start = sel_start.min(blocks.len());
    let sel_end = sel_end.clamp(sel_start, blocks.len());
    let prefix = blocks[..sel_start]
        .iter()
        .map(|b| b.reading.as_str())
        .collect::<String>();
    let target = blocks[sel_start..sel_end]
        .iter()
        .map(|b| b.reading.as_str())
        .collect::<String>();
    let suffix = blocks[sel_end..]
        .iter()
        .map(|b| b.reading.as_str())
        .collect::<String>();
    (prefix, target, suffix)
}

fn split_display_parts(
    blocks: &[SplitBlock],
    sel_start: usize,
    sel_end: usize,
) -> (String, String, String) {
    let sel_start = sel_start.min(blocks.len());
    let sel_end = sel_end.clamp(sel_start, blocks.len());
    let prefix = blocks[..sel_start]
        .iter()
        .map(|b| b.display.as_str())
        .collect::<String>();
    let target = blocks[sel_start..sel_end]
        .iter()
        .map(|b| b.display.as_str())
        .collect::<String>();
    let suffix = blocks[sel_end..]
        .iter()
        .map(|b| b.display.as_str())
        .collect::<String>();
    (prefix, target, suffix)
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
        let len = blocks.len();
        let sel_start = sel_start.min(len.saturating_sub(1));
        let sel_end = sel_end.clamp(sel_start.saturating_add(1), len);
        *self = SessionState::SplitPreedit {
            blocks,
            sel_start,
            sel_end,
        };
        SESSION_SELECTING.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn is_split_preedit(&self) -> bool {
        matches!(self, SessionState::SplitPreedit { .. })
    }

    pub fn split_target(&self) -> Option<String> {
        if let SessionState::SplitPreedit {
            blocks,
            sel_start,
            sel_end,
        } = self
        {
            Some(split_reading_parts(blocks, *sel_start, *sel_end).1)
        } else {
            None
        }
    }

    pub fn split_prefix(&self) -> Option<String> {
        if let SessionState::SplitPreedit {
            blocks,
            sel_start,
            sel_end,
        } = self
        {
            Some(split_reading_parts(blocks, *sel_start, *sel_end).0)
        } else {
            None
        }
    }

    pub fn split_remainder(&self) -> Option<String> {
        if let SessionState::SplitPreedit {
            blocks,
            sel_start,
            sel_end,
        } = self
        {
            Some(split_reading_parts(blocks, *sel_start, *sel_end).2)
        } else {
            None
        }
    }

    pub fn split_display_prefix(&self) -> Option<String> {
        if let SessionState::SplitPreedit {
            blocks,
            sel_start,
            sel_end,
        } = self
        {
            Some(split_display_parts(blocks, *sel_start, *sel_end).0)
        } else {
            None
        }
    }

    pub fn split_display_target(&self) -> Option<String> {
        if let SessionState::SplitPreedit {
            blocks,
            sel_start,
            sel_end,
        } = self
        {
            Some(split_display_parts(blocks, *sel_start, *sel_end).1)
        } else {
            None
        }
    }

    pub fn split_display_remainder(&self) -> Option<String> {
        if let SessionState::SplitPreedit {
            blocks,
            sel_start,
            sel_end,
        } = self
        {
            Some(split_display_parts(blocks, *sel_start, *sel_end).2)
        } else {
            None
        }
    }

    pub fn split_move_left(&mut self) -> bool {
        if let SessionState::SplitPreedit {
            sel_start, sel_end, ..
        } = self
        {
            let width = sel_end.saturating_sub(*sel_start);
            if *sel_start > 0 {
                *sel_start -= 1;
                *sel_end = *sel_start + width;
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn split_move_right(&mut self) -> bool {
        if let SessionState::SplitPreedit {
            blocks,
            sel_start,
            sel_end,
        } = self
        {
            let width = sel_end.saturating_sub(*sel_start);
            if *sel_end < blocks.len() {
                *sel_start += 1;
                *sel_end = (*sel_start + width).min(blocks.len());
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Shift+Left: 選択範囲の右端を左へ縮める。
    pub fn split_shrink(&mut self) -> bool {
        if let SessionState::SplitPreedit {
            sel_start, sel_end, ..
        } = self
        {
            if *sel_end > sel_start.saturating_add(1) {
                *sel_end -= 1;
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Shift+Right: 選択範囲の右端を右へ広げる。
    pub fn split_extend(&mut self) -> bool {
        if let SessionState::SplitPreedit {
            blocks, sel_end, ..
        } = self
        {
            if *sel_end < blocks.len() {
                *sel_end += 1;
                true
            } else {
                false
            }
        } else {
            false
        }
    }

    pub fn activate_selecting(
        &mut self,
        candidates: Vec<String>,
        original_preedit: String,
        original_surface: String,
        pos_x: i32,
        pos_y: i32,
        llm_pending: bool,
    ) {
        self.activate_selecting_with_affixes(
            candidates,
            original_preedit,
            original_surface,
            pos_x,
            pos_y,
            llm_pending,
            String::new(),
            String::new(),
        );
    }

    pub fn activate_selecting_with_affixes(
        &mut self,
        candidates: Vec<String>,
        original_preedit: String,
        original_surface: String,
        pos_x: i32,
        pos_y: i32,
        llm_pending: bool,
        prefix: String,
        remainder: String,
    ) {
        *self = SessionState::Selecting {
            original_preedit,
            original_surface,
            candidates,
            selected: 0,
            page_size: 9,
            llm_pending,
            pos_x,
            pos_y,
            punct_pending: None,
            prefix,
            remainder,
        };
        SESSION_SELECTING.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn is_selecting(&self) -> bool {
        matches!(self, SessionState::Selecting { .. })
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

    pub fn selecting_prefix_clone(&self) -> String {
        if let SessionState::Selecting { prefix, .. } = self {
            prefix.clone()
        } else {
            String::new()
        }
    }

    pub fn selecting_original_surface(&self) -> Option<&str> {
        if let SessionState::Selecting {
            original_surface, ..
        } = self
        {
            Some(original_surface.as_str())
        } else {
            None
        }
    }

    pub fn current_page(&self) -> usize {
        match self {
            SessionState::Selecting {
                selected,
                page_size,
                ..
            } => selected / page_size,
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
            _ => 0,
        }
    }

    pub fn page_candidates(&self) -> &[String] {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                page_size,
                ..
            } => {
                if candidates.is_empty() {
                    return &[];
                }
                let start = (selected / page_size) * page_size;
                let end = (start + page_size).min(candidates.len());
                &candidates[start..end]
            }
            _ => &[],
        }
    }

    pub fn page_selected(&self) -> usize {
        match self {
            SessionState::Selecting {
                selected,
                page_size,
                ..
            } => selected % page_size,
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
        if let SessionState::Selecting {
            candidates,
            selected,
            page_size,
            ..
        } = self
        {
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
    }

    pub fn prev(&mut self) {
        if let SessionState::Selecting {
            candidates,
            selected,
            ..
        } = self
        {
            if candidates.is_empty() {
                return;
            }
            *selected = if *selected == 0 {
                candidates.len() - 1
            } else {
                *selected - 1
            };
        }
    }

    pub fn next_page(&mut self) {
        if let SessionState::Selecting {
            candidates,
            selected,
            page_size,
            ..
        } = self
        {
            if candidates.is_empty() {
                return;
            }
            let total_pages = (candidates.len() + *page_size - 1) / *page_size;
            let cur = *selected / *page_size;
            let next = (cur + 1) % total_pages;
            *selected = next * *page_size;
        }
    }

    pub fn prev_page(&mut self) {
        if let SessionState::Selecting {
            candidates,
            selected,
            page_size,
            ..
        } = self
        {
            if candidates.is_empty() {
                return;
            }
            let total_pages = (candidates.len() + *page_size - 1) / *page_size;
            let cur = *selected / *page_size;
            let prev = if cur == 0 { total_pages - 1 } else { cur - 1 };
            *selected = prev * *page_size;
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
    use super::{SessionState, SplitBlock};

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
}
