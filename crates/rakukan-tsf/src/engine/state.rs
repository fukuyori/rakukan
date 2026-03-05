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
            g.0 = Some(create_engine()?);
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
                        Ok(e)  => g2.0 = Some(e),
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
    let mut num_candidates: usize = 9;
    let mut main_gpu:       i32   = 0;
    let n_gpu_layers: u32 = u32::MAX;  // DLL 側（cpu DLL は 0 に上書き）
    let mut model_variant: Option<String> = None;

    if let Ok(appdata) = std::env::var("APPDATA") {
        let path = std::path::PathBuf::from(appdata).join("rakukan").join("config.toml");
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('#') { continue; }
                if let Some(rest) = line.strip_prefix("num_candidates") {
                    let v = rest.trim().trim_start_matches('=').trim()
                        .split('#').next().unwrap_or("").trim();
                    if let Ok(n) = v.parse::<usize>() { num_candidates = n.clamp(1, 9); }
                }
                if let Some(rest) = line.strip_prefix("main_gpu") {
                    let v = rest.trim().trim_start_matches('=').trim()
                        .split('#').next().unwrap_or("").trim();
                    if let Ok(n) = v.parse::<i32>() { main_gpu = n; }
                }
                if let Some(rest) = line.strip_prefix("model_variant") {
                    let v = rest.trim().trim_start_matches('=').trim()
                        .split('#').next().unwrap_or("").trim()
                        .trim_matches('"');
                    if !v.is_empty() { model_variant = Some(v.to_string()); }
                }
            }
        }
    }
    tracing::info!("engine config: num_candidates={num_candidates} main_gpu={main_gpu} model_variant={model_variant:?}");
    let mv_json = match &model_variant {
        Some(v) => format!(r#","model_variant":"{}""#, v),
        None    => String::new(),
    };
    format!(r#"{{"num_candidates":{num_candidates},"n_gpu_layers":{n_gpu_layers},"main_gpu":{main_gpu},"n_threads":0{mv_json}}}"#)
}


/// config.toml から num_candidates を読む（ホットパスで使う軽量版）
pub fn get_num_candidates() -> usize {
    if let Ok(appdata) = std::env::var("APPDATA") {
        let path = std::path::PathBuf::from(appdata).join("rakukan").join("config.toml");
        if let Ok(text) = std::fs::read_to_string(&path) {
            for line in text.lines() {
                let line = line.trim();
                if line.starts_with('#') { continue; }
                if let Some(rest) = line.strip_prefix("num_candidates") {
                    let v = rest.trim().trim_start_matches('=').trim()
                        .split('#').next().unwrap_or("").trim();
                    if let Ok(n) = v.parse::<usize>() { return n.clamp(1, 9); }
                }
            }
        }
    }
    9  // default
}

impl std::ops::Deref for EngineWrapper {
    type Target = Option<DynEngine>;
    fn deref(&self) -> &Self::Target { &self.0 }
}
impl std::ops::DerefMut for EngineWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.0 }
}

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

// ─── SelectionState ──────────────────────────────────────────────────────────
// Convert後の候補選択モードを管理する。
// ホットパスから try_lock のみ使うこと。

/// 変換候補の選択状態
#[derive(Debug, Default, Clone)]
pub struct SelectionState {
    /// 候補リスト全体（空 = 選択モード非アクティブ）
    pub candidates:       Vec<String>,
    /// 現在選択中のグローバルインデックス
    pub selected:         usize,
    /// 選択モード突入前のプリエディット（Escape で戻るため）
    pub original_preedit: String,
    /// 候補ウィンドウ表示位置（スクリーン座標、プリエディット下端）
    #[allow(dead_code)] pub pos_x: i32,
    #[allow(dead_code)] pub pos_y: i32,
    /// 1ページあたりの表示件数（固定 9）
    pub page_size:        usize,
    /// LLM BG 変換待ち中フラグ（完了時に候補を自動更新）
    pub llm_pending:      bool,
    /// 辞書0件のときに選択モードに入らずLLM完了を待機中のプリエディット。
    /// Some(preedit) = 待機中。BG完了時に選択モードへ遷移する。
    /// 待機中に Space を再押下するとひらがなをコミットして None に戻る。
    pub llm_wait_preedit: Option<String>,
}

impl SelectionState {
    pub fn is_active(&self) -> bool {
        !self.candidates.is_empty()
    }

    pub fn activate(
        candidates: Vec<String>,
        original_preedit: String,
        pos_x: i32,
        pos_y: i32,
    ) -> Self {
        SELECTION_ACTIVE.store(true, std::sync::atomic::Ordering::Release);
        Self {
            candidates,
            selected: 0,
            original_preedit,
            pos_x,
            pos_y,
            page_size: 9,
            llm_pending: false,
            llm_wait_preedit: None,
        }
    }

    pub fn clear(&mut self) {
        self.candidates.clear();
        self.selected = 0;
        self.original_preedit.clear();
        self.llm_wait_preedit = None;
        SELECTION_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    }

    pub fn current_candidate(&self) -> Option<&str> {
        self.candidates.get(self.selected).map(|s| s.as_str())
    }

    // ─── ページング ───────────────────────────────────────────────────────────

    /// 現在のページ番号（0-origin）
    pub fn current_page(&self) -> usize {
        self.selected / self.page_size
    }

    /// 総ページ数
    pub fn total_pages(&self) -> usize {
        if self.candidates.is_empty() { return 0; }
        (self.candidates.len() + self.page_size - 1) / self.page_size
    }

    /// 現在ページの候補スライスを返す
    pub fn page_candidates(&self) -> &[String] {
        if self.candidates.is_empty() { return &[]; }
        let start = self.current_page() * self.page_size;
        let end   = (start + self.page_size).min(self.candidates.len());
        &self.candidates[start..end]
    }

    /// 現在ページ内での選択インデックス（0-origin、候補ウィンドウ描画用）
    pub fn page_selected(&self) -> usize {
        self.selected % self.page_size
    }

    /// Space キー用: 1候補ずつ進む。
    /// ページ末尾（= 次が次ページ先頭）のときは次ページ先頭へジャンプする。
    /// 全候補の末尾では先頭ページへ折り返す。
    pub fn next_with_page_wrap(&mut self) {
        if self.candidates.is_empty() { return; }
        let next_idx = (self.selected + 1) % self.candidates.len();
        // 次のインデックスが別ページに属すか（= 現在がページ末尾）
        let cur_page  = self.selected / self.page_size;
        let next_page = next_idx / self.page_size;
        if next_page != cur_page {
            // ページをまたぐ → 次ページ先頭へ
            self.selected = next_page * self.page_size;
        } else {
            self.selected = next_idx;
        }
    }

    /// ページ内の前候補へ（ページ先頭で前ページへ折り返す）
    pub fn prev(&mut self) {
        if !self.candidates.is_empty() {
            if self.selected == 0 {
                self.selected = self.candidates.len() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    /// 次ページの先頭へ（最終ページなら先頭ページへ折り返す）
    pub fn next_page(&mut self) {
        if self.candidates.is_empty() { return; }
        let next = (self.current_page() + 1) % self.total_pages();
        self.selected = next * self.page_size;
    }

    /// 前ページの先頭へ（先頭ページなら最終ページへ折り返す）
    pub fn prev_page(&mut self) {
        if self.candidates.is_empty() { return; }
        let cur = self.current_page();
        let prev = if cur == 0 { self.total_pages() - 1 } else { cur - 1 };
        self.selected = prev * self.page_size;
    }

    /// 現在ページ内で 1-indexed の選択（1〜page_size）
    /// 範囲外なら false を返す
    pub fn select_nth_in_page(&mut self, n: usize) -> bool {
        if n < 1 { return false; }
        let idx = self.current_page() * self.page_size + (n - 1);
        if idx < self.candidates.len() {
            self.selected = idx;
            true
        } else {
            false
        }
    }

    /// ページインジケーター文字列（例: "2/4"）。1ページのみなら空文字列。
    pub fn page_info(&self) -> String {
        let total = self.total_pages();
        if total <= 1 {
            String::new()
        } else {
            format!("{}/{}", self.current_page() + 1, total)
        }
    }

    /// 後方互換: 全体インデックスで直接選択（1-indexed）
    #[allow(dead_code)]
    pub fn select_nth(&mut self, n: usize) -> bool {
        self.select_nth_in_page(n)
    }
}

/// ロックなしで候補選択モード中かを確認するためのフラグ
/// SelectionState の candidates が空かどうかを mirrors する
pub static SELECTION_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

pub static SELECTION_STATE: LazyLock<Mutex<SelectionState>> =
    LazyLock::new(|| Mutex::new(SelectionState::default()));

/// ホットパス用: ブロックしない
#[inline]
pub fn selection_try_get() -> anyhow::Result<MutexGuard<'static, SelectionState>> {
    SELECTION_STATE
        .try_lock()
        .map_err(|_| anyhow::anyhow!("selection_state busy"))
}

/// 安全側の取得: 必要な場面（確定/次入力など）ではブロックしてでも取得する。
/// try_lock 失敗で「確定が落ちる（文字が消える）」のを防ぐ。
pub fn selection_get() -> anyhow::Result<MutexGuard<'static, SelectionState>> {
    SELECTION_STATE
        .lock()
        .map_err(|_| anyhow::anyhow!("selection_state poisoned"))
}

/// ロックなし高速チェック: 候補選択モード中かどうかを確認する（ホットパス用）
#[inline]
pub fn selection_is_active_fast() -> bool {
    SELECTION_ACTIVE.load(std::sync::atomic::Ordering::Acquire)
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
