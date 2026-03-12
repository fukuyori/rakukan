//! グローバル IME 状態
//!
//! # ロック戦略
//! TSF OnKeyDown はホットパス（UIスレッド）のため、**絶対にブロックしない**。
//! - ホットパス: `try_lock()` のみ使用。取れなければ即リターン。
//! - 非ホットパス（Activate, BG スレッド): `lock()` を使用可。

use std::sync::{LazyLock, Mutex, MutexGuard};
use std::sync::atomic::{AtomicU8, Ordering as AO};
use windows::core::GUID;
use super::input_mode::InputMode;
use rakukan_engine_abi::{DynEngine, install_dir};
use std::collections::HashMap;

// ─── INPUT_MODE_ATOMIC ────────────────────────────────────────────────────────
// IMEState.input_mode の鏡。ロックなしでホットパス（OnTestKeyDown / OnKeyDown）
// から安全に読み取れるよう AtomicU8 で持つ。
// 値: 0=Hiragana, 1=Katakana, 2=Alphanumeric
// IMEState::set_mode が呼ばれるたびに同期更新される。

static INPUT_MODE_ATOMIC: AtomicU8 = AtomicU8::new(0);

pub fn input_mode_set_atomic(mode: InputMode) {
    let v = match mode {
        InputMode::Hiragana     => 0u8,
        InputMode::Katakana     => 1u8,
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

/// ホットパス用: ブロックしない。取れなければ Err を返す。
#[inline]
pub fn engine_try_get() -> anyhow::Result<MutexGuard<'static, EngineWrapper>> {
    RAKUKAN_ENGINE.try_lock()
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

/// エンジンを作成して返す（未初期化の場合のみ作成する）
pub fn engine_get_or_create() -> anyhow::Result<MutexGuard<'static, EngineWrapper>> {
    {
        let mut g = engine_get()?;
        if g.0.is_none() {
            let e = create_engine()?;
            g.0 = Some(e);
            drop(g);
            if let Ok(mut g2) = RAKUKAN_ENGINE.lock() {
                if let Some(eng) = g2.0.as_mut() {
                    if !eng.is_dict_ready()   { eng.start_load_dict(); }
                    if !eng.is_kanji_ready()  { eng.start_load_model(); }
                }
            }
            return engine_get();
        }
    }
    engine_get()
}

/// ホットパス用 engine_get_or_create: ブロックしない
pub fn engine_try_get_or_create() -> anyhow::Result<MutexGuard<'static, EngineWrapper>> {
    {
        let g = engine_try_get()?;
        if g.0.is_none() {
            if let Ok(mut g2) = RAKUKAN_ENGINE.lock() {
                if g2.0.is_none() {
                    match create_engine() {
                        Ok(e) => {
                            g2.0 = Some(e);
                            drop(g2);
                            if let Ok(mut g3) = RAKUKAN_ENGINE.lock() {
                                if let Some(eng) = g3.0.as_mut() {
                                    if !eng.is_dict_ready()  { eng.start_load_dict(); }
                                    if !eng.is_kanji_ready() { eng.start_load_model(); }
                                }
                            }
                        }
                        Err(e) => tracing::error!("engine create failed: {e}"),
                    }
                }
            }
        }
    }
    engine_try_get()
}

/// エンジンを強制的に破棄し、次回アクセス時に再生成されるようにする。
/// config.toml 変更後や DLL 入れ替え後に使用する。
pub fn engine_force_recreate() {
    match RAKUKAN_ENGINE.lock() {
        Ok(mut g) => {
            tracing::info!("engine_force_recreate: dropping engine");
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
                                        if !eng.is_kanji_ready() { eng.start_load_model(); }
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
            use windows::Win32::System::Threading::{
                CreateEventW, WaitForSingleObject, INFINITE,
            };
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
    tracing::info!("create_engine: install_dir={}", dir.display());

    // engine DLL の存在を事前確認してわかりやすいエラーを出す
    for backend in &["cpu", "vulkan", "cuda"] {
        let p = dir.join(format!("rakukan_engine_{backend}.dll"));
        tracing::debug!("  {}: {}", backend, if p.exists() { "found" } else { "not found" });
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
    let main_gpu = cfg.main_gpu;
    let n_gpu_layers: u32 = u32::MAX;  // DLL 側（cpu DLL は 0 に上書き）
    let model_variant = cfg.model_variant.clone();

    tracing::info!("engine config: num_candidates={num_candidates} main_gpu={main_gpu} model_variant={model_variant:?}");
    let mv_json = match &model_variant {
        Some(v) => format!(r#","model_variant":"{}""#, v),
        None    => String::new(),
    };
    format!(r#"{{"num_candidates":{num_candidates},"n_gpu_layers":{n_gpu_layers},"main_gpu":{main_gpu},"n_threads":0{mv_json}}}"#)
}


/// config.toml から num_candidates を読む（ホットパスで使う軽量版）
pub fn get_num_candidates() -> usize {
    super::config::effective_num_candidates()
}

impl std::ops::Deref for EngineWrapper {
    type Target = Option<DynEngine>;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl std::ops::DerefMut for EngineWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
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
        cookies:    HashMap::new(),
    })
});

unsafe impl Send for IMEState {}
unsafe impl Sync for IMEState {}

impl IMEState {
    /// ホットパス用: ブロックしない
    #[inline]
    pub fn try_get() -> anyhow::Result<MutexGuard<'static, IMEState>> {
        IME_STATE.try_lock().map_err(|_| anyhow::anyhow!("ime_state busy"))
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
    COMPOSITION.try_lock().map_err(|_| anyhow::anyhow!("composition busy"))
}

pub fn composition_set(comp: Option<ITfComposition>) -> anyhow::Result<()> {
    // set はホットパスでもブロックを許容（短い操作のため）
    let mut g = COMPOSITION.lock().map_err(|p| { let _ = p; anyhow::anyhow!("composition poison") })?;
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
    /// Shift+Left/Right で境界を調整中。Space で target を変換、Enter で target 確定。
    /// - target   : 変換対象（実線アンダーライン）
    /// - remainder: 変換しない残り（点線アンダーライン）
    SplitPreedit {
        target: String,
        remainder: String,
    },
    Selecting {
        original_preedit: String,
        candidates: Vec<String>,
        selected: usize,
        page_size: usize,
        llm_pending: bool,
        pos_x: i32,
        pos_y: i32,
        /// 句読点保留（「、」「。」押下時にセット、確定時に末尾連結）
        punct_pending: Option<char>,
        /// 文節分割後に変換した場合の残り部分（確定後に次のプリエディットになる）
        remainder: String,
    },
}

pub static SESSION_STATE: LazyLock<Mutex<SessionState>> =
    LazyLock::new(|| Mutex::new(SessionState::Idle));

pub static SESSION_SELECTING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);


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

    pub fn set_waiting(&mut self, text: String, pos_x: i32, pos_y: i32) {
        *self = SessionState::Waiting { text, pos_x, pos_y };
        SESSION_SELECTING.store(false, std::sync::atomic::Ordering::Release);
    }

    /// 文節分割表示状態へ移行（target=実線、remainder=点線）
    ///
    /// SplitPreedit 中もキーを IME が消費する必要があるため SESSION_SELECTING を true に保つ。
    /// false にすると OnTestKeyDown が FALSE を返し、アプリが Shift+左/右を直接処理して
    /// コンポジション内の文字が消える原因になる。
    pub fn set_split_preedit(&mut self, target: String, remainder: String) {
        *self = SessionState::SplitPreedit { target, remainder };
        SESSION_SELECTING.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn is_split_preedit(&self) -> bool {
        matches!(self, SessionState::SplitPreedit { .. })
    }

    pub fn split_target(&self) -> Option<&str> {
        if let SessionState::SplitPreedit { target, .. } = self { Some(target) } else { None }
    }

    pub fn split_remainder(&self) -> Option<&str> {
        if let SessionState::SplitPreedit { remainder, .. } = self { Some(remainder) } else { None }
    }

    /// Shift+Left: target の末尾1文字を remainder の先頭へ移動。
    /// target が1文字以下になる場合は false を返して何もしない。
    pub fn split_shrink(&mut self) -> bool {
        if let SessionState::SplitPreedit { target, remainder } = self {
            let last = target.char_indices().next_back().map(|(i, _)| i);
            match last {
                Some(i) if i > 0 => {
                    let ch: String = target[i..].to_string();
                    target.truncate(i);
                    remainder.insert_str(0, &ch);
                    true
                }
                _ => false,
            }
        } else {
            false
        }
    }

    /// Shift+Right: remainder の先頭1文字を target の末尾へ移動。
    /// remainder が空なら false を返す。
    pub fn split_extend(&mut self) -> bool {
        if let SessionState::SplitPreedit { target, remainder } = self {
            if remainder.is_empty() { return false; }
            let end = remainder.char_indices().nth(1).map(|(i, _)| i).unwrap_or(remainder.len());
            let ch: String = remainder[..end].to_string();
            remainder.drain(..end);
            target.push_str(&ch);
            true
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
        self.activate_selecting_with_remainder(candidates, original_preedit, pos_x, pos_y, llm_pending, String::new());
    }

    pub fn activate_selecting_with_remainder(
        &mut self,
        candidates: Vec<String>,
        original_preedit: String,
        pos_x: i32,
        pos_y: i32,
        llm_pending: bool,
        remainder: String,
    ) {
        *self = SessionState::Selecting {
            original_preedit,
            candidates,
            selected: 0,
            page_size: 9,
            llm_pending,
            pos_x,
            pos_y,
            punct_pending: None,
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
            SessionState::Selecting { original_preedit, .. } => Some(original_preedit.as_str()),
            SessionState::SplitPreedit { target, .. } => Some(target.as_str()),
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
            SessionState::Selecting { candidates, selected, .. } => {
                candidates.get(*selected).map(|s| s.as_str())
            }
            _ => None,
        }
    }

    pub fn original_preedit(&self) -> Option<&str> {
        match self {
            SessionState::Selecting { original_preedit, .. } => Some(original_preedit.as_str()),
            SessionState::Preedit { text } => Some(text.as_str()),
            SessionState::Waiting { text, .. } => Some(text.as_str()),
            SessionState::SplitPreedit { target, .. } => Some(target.as_str()),
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

    pub fn current_page(&self) -> usize {
        match self {
            SessionState::Selecting { selected, page_size, .. } => selected / page_size,
            _ => 0,
        }
    }

    pub fn total_pages(&self) -> usize {
        match self {
            SessionState::Selecting { candidates, page_size, .. } => {
                if candidates.is_empty() { 0 } else { (candidates.len() + page_size - 1) / page_size }
            }
            _ => 0,
        }
    }

    pub fn page_candidates(&self) -> &[String] {
        match self {
            SessionState::Selecting { candidates, selected, page_size, .. } => {
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
            SessionState::Selecting { selected, page_size, .. } => selected % page_size,
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
        if let SessionState::Selecting { candidates, selected, page_size, .. } = self {
            if candidates.is_empty() { return; }
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
        if let SessionState::Selecting { candidates, selected, .. } = self {
            if candidates.is_empty() { return; }
            *selected = if *selected == 0 { candidates.len() - 1 } else { *selected - 1 };
        }
    }

    pub fn next_page(&mut self) {
        if let SessionState::Selecting { candidates, selected, page_size, .. } = self {
            if candidates.is_empty() { return; }
            let total_pages = (candidates.len() + *page_size - 1) / *page_size;
            let cur = *selected / *page_size;
            let next = (cur + 1) % total_pages;
            *selected = next * *page_size;
        }
    }

    pub fn prev_page(&mut self) {
        if let SessionState::Selecting { candidates, selected, page_size, .. } = self {
            if candidates.is_empty() { return; }
            let total_pages = (candidates.len() + *page_size - 1) / *page_size;
            let cur = *selected / *page_size;
            let prev = if cur == 0 { total_pages - 1 } else { cur - 1 };
            *selected = prev * *page_size;
        }
    }

    pub fn select_nth_in_page(&mut self, n: usize) -> bool {
        if n < 1 { return false; }
        match self {
            SessionState::Selecting { candidates, selected, page_size, .. } => {
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

use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};

pub static LANGBAR_UPDATE_PENDING: AtomicBool = AtomicBool::new(false);

#[allow(dead_code)]
pub fn langbar_update_set()   { LANGBAR_UPDATE_PENDING.store(true,  AtomicOrdering::Release); }
pub fn langbar_update_take() -> bool { LANGBAR_UPDATE_PENDING.swap(false, AtomicOrdering::AcqRel) }

// ─── DocumentManager モードストア ────────────────────────────────────────────
//
// MS-IME準拠: アプリ（DocumentManager）ごとに InputMode を記憶する。
//
// # キー
// ITfDocumentMgr の COM ポインタ値（usize）をキーに使う。
// DocumentManager はアプリウィンドウのライフタイムに結びつき、
// ウィンドウが閉じれば解放されるため、古いエントリは自然に無効化される。
// エントリ数は開いているウィンドウ数に比例するため上限は設けない
// （数百件で問題になるレベルではない）。
//
// # ターミナル判定
// Windows Terminal / ConHost のウィンドウクラス名で判定し、
// 初回フォーカス時に Alphanumeric をデフォルトにする。

static DOC_MODE_STORE: LazyLock<Mutex<HashMap<usize, InputMode>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
    next_hwnd:   usize,
) -> Option<InputMode> {
    let mut store = match DOC_MODE_STORE.try_lock() {
        Ok(g)  => g,
        Err(_) => return None,
    };

    // 前のDocumentManagerのモードを保存（0 = DocumentManagerなし = スキップ）
    if prev_dm_ptr != 0 {
        store.insert(prev_dm_ptr, input_mode_get_atomic());
    }

    if next_dm_ptr == 0 {
        return None;
    }

    let mode = if let Some(&saved) = store.get(&next_dm_ptr) {
        // 既知のDocumentManager → 前回モードを復元
        saved
    } else {
        // 初回フォーカス → デフォルトモードを決定
        let default_mode = if is_terminal_hwnd(next_hwnd) {
            tracing::debug!("doc_mode: terminal detected (hwnd={next_hwnd:#x}), default=Alphanumeric");
            InputMode::Alphanumeric
        } else {
            InputMode::Hiragana
        };
        store.insert(next_dm_ptr, default_mode);
        default_mode
    };

    Some(mode)
}

/// DocumentManager が破棄されたとき（OnUninitDocumentMgr）にエントリを削除する。
pub fn doc_mode_remove(dm_ptr: usize) {
    if let Ok(mut store) = DOC_MODE_STORE.try_lock() {
        store.remove(&dm_ptr);
    }
}

/// HWND がターミナル系ウィンドウかどうかを判定する。
///
/// 判定対象:
/// - Windows Terminal: `CASCADIA_HOSTING_WINDOW_CLASS`
/// - 旧来の ConHost:   `ConsoleWindowClass`
/// - VSCode 統合ターミナル等は親が上記クラスを持つ場合あり（簡易判定のみ）
fn is_terminal_hwnd(hwnd_val: usize) -> bool {
    if hwnd_val == 0 { return false; }

    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

    let hwnd = HWND(hwnd_val as *mut _);
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    if len == 0 { return false; }

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
