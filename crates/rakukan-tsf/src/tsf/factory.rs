//! rakukan TextService COM オブジェクト
//!
//! # ホットパス原則
//! `OnKeyDown` / `OnTestKeyDown` は原則ブロックしない:
//! - `try_lock()` のみ使用
//! - `TF_ES_SYNC` を使わない（`TF_ES_READWRITE` のみ）
//!
//! # Space キー（変換開始）の特例ブロッキング
//! Space キーによる `on_convert[new]` は LLM 変換完了まで TSF スレッドをブロックする。
//! これは `WM_TIMER` コールバックからは `RequestEditSession` を呼べないため
//! composition text を更新できないという制約に由来する。
//!
//! タイムアウトは文字数に応じて動的に設定（基本 3 秒 + 1 文字 300ms、上限 15 秒）。
//! タイムアウト時は `WM_TIMER` ポーリングにフォールバックし、候補ウィンドウのみ自動更新する。
//!
//! # on_convert[new] フロー
//! ```text
//! Space押下
//!   │
//!   ├─ bg=idle → bg_start → bg_wait_ms（ブロッキング）→ 候補取得 → 表示
//!   │
//!   ├─ bg=running（前変換の converter 貸し出し中）
//!   │     → prev bg_wait_ms → bg_reclaim → bg_start → bg_wait_ms → 候補取得 → 表示
//!   │
//!   ├─ bg_take_candidates=None（キー不一致）
//!   │     → bg_reclaim → bg_start → bg_wait_ms（再試行）→ 候補取得 → 表示
//!   │
//!   └─ bg_wait_ms タイムアウト → WM_TIMER ポーリングにフォールバック
//! ```

use std::cell::RefCell;

use anyhow::Result;
use windows::{
    core::{implement, IUnknown, Interface, BSTR, GUID},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_INVALIDARG, FALSE, LPARAM, POINT, RECT, TRUE, WPARAM},
        Graphics::Gdi::HBITMAP,
        System::{
            Com::{CoCreateInstance, IClassFactory, IClassFactory_Impl, CLSCTX_INPROC_SERVER},
            Ole::CONNECT_E_CANNOTCONNECT,
        },
        UI::{
            Input::KeyboardAndMouse::GetKeyState,
            TextServices::{
                CLSID_TF_CategoryMgr, IEnumTfDisplayAttributeInfo, ITfCategoryMgr, ITfComposition,
                ITfCompositionSink, ITfCompositionSink_Impl, ITfContext, ITfDisplayAttributeInfo,
                ITfDisplayAttributeProvider, ITfDisplayAttributeProvider_Impl, ITfDocumentMgr,
                ITfKeyEventSink, ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfLangBarItem,
                ITfLangBarItemButton, ITfLangBarItemButton_Impl, ITfLangBarItemSink,
                ITfLangBarItem_Impl, ITfMenu, ITfSource, ITfSource_Impl, ITfTextInputProcessor,
                ITfTextInputProcessor_Impl, ITfThreadFocusSink, ITfThreadFocusSink_Impl,
                ITfThreadMgr, ITfThreadMgrEventSink, ITfThreadMgrEventSink_Impl, TfLBIClick,
                GUID_PROP_ATTRIBUTE, TF_ES_READWRITE, TF_LANGBARITEMINFO, TF_LBMENUF_RADIOCHECKED,
                TF_LBMENUF_SEPARATOR,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, DestroyMenu, GA_ROOT, GetAncestor, GetForegroundWindow,
                HICON, MF_SEPARATOR, TPM_LEFTALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu,
            },
        },
    },
};

use crate::{
    diagnostics::{self as diag, DiagEvent},
    engine::{
        keymap::Keymap,
        state::{
            caret_rect_get, caret_rect_set, composition_clone, composition_set,
            composition_set_with_dm, composition_take, doc_mode_on_focus_change,
            engine_get, engine_try_get_or_create, session_get, session_is_selecting_fast,
            SessionState,
        },
        text_util,
        user_action::UserAction,
    },
    globals::{GUID_DISPLAY_ATTRIBUTE, GUID_DISPLAY_ATTRIBUTE_INPUT},
    tsf::{
        candidate_window, display_attr,
        edit_session::EditSession,
        language_bar::{self, get_open_close, LANGBAR_SINK_COOKIE},
        settings_launcher, tray_ipc,
    },
};

const ID_MENU_MODE_HIRAGANA: u32 = 1;
const ID_MENU_MODE_KATAKANA: u32 = 2;
const ID_MENU_MODE_ALPHANUMERIC: u32 = 3;
const ID_MENU_SETTINGS: u32 = 10;
const ID_MENU_ENGINE_RELOAD: u32 = 11;

fn current_langbar_mode(open: bool) -> crate::engine::input_mode::InputMode {
    if !open {
        crate::engine::input_mode::InputMode::Alphanumeric
    } else {
        crate::engine::state::ime_state_get()
            .ok()
            .map(|state| state.input_mode)
            .unwrap_or(crate::engine::input_mode::InputMode::Hiragana)
    }
}

/// M1.6 T-HOST3: 読込中インジケータの記号とメッセージを決める。
///
/// 現状の `mode_indicator` は 1 文字固定のため、記号のみを表示する。メッセージ
/// は将来 mode_indicator を可変長対応化したときに使う想定で返しているだけで、
/// 今は呼び出し側で捨てて良い（`_msg` で受けている）。
///
/// 経過時間は `ready_reset_elapsed_ms()` を参照。`None`（= 初期状態 or 既に
/// ready）なら単純な砂時計を返す。
fn loading_indicator_symbol() -> (&'static str, &'static str) {
    match crate::engine::state::ready_reset_elapsed_ms() {
        None => ("⏳", "エンジン読込中"),
        Some(ms) if ms < 10_000 => ("⏳", "エンジン読込中"),
        Some(ms) if ms < 30_000 => ("⌛", "エンジン読込中..."),
        Some(ms) if ms < 60_000 => ("⚠", "読込に時間がかかっています"),
        Some(_) => ("✕", "エンジン起動失敗の可能性"),
    }
}

/// `GetForegroundWindow()` の結果を `GA_ROOT` でルート HWND に正規化して返す。
///
/// Chrome / Edge 等は内部で子 HWND (`Chrome_RenderWidgetHostHWND` 等) を
/// フォーカスターゲットにしているケースがあり、`GetForegroundWindow()` が
/// その子 HWND を返すことがある。doc_mode の `hwnd_modes` キーとして使うには
/// ルートに揃えないとキーが fragment する。
fn foreground_root_hwnd() -> usize {
    unsafe {
        let raw = GetForegroundWindow();
        if raw.0.is_null() {
            return 0;
        }
        let root = GetAncestor(raw, GA_ROOT);
        if root.0.is_null() {
            raw.0 as usize
        } else {
            root.0 as usize
        }
    }
}

fn apply_langbar_mode(
    factory: &TextServiceFactory_Impl,
    new_mode: crate::engine::input_mode::InputMode,
) {
    let (tm, tid) = factory
        .inner
        .try_borrow()
        .ok()
        .and_then(|inner| inner.thread_mgr.clone().map(|tm| (tm, inner.client_id)))
        .unzip();

    if let (Some(tm), Some(tid)) = (tm, tid) {
        unsafe {
            let _ = language_bar::set_open_close(
                &tm,
                tid,
                new_mode != crate::engine::input_mode::InputMode::Alphanumeric,
            );
        }
    }

    if let Ok(mut state) = crate::engine::state::ime_state_get() {
        let from = format!("{:?}", state.input_mode);
        state.set_mode(new_mode);
        tracing::info!("langbar menu: input mode {} -> {:?}", from, new_mode);
        diag::event(DiagEvent::ModeChange {
            from,
            to: match new_mode {
                crate::engine::input_mode::InputMode::Hiragana => "Hiragana",
                crate::engine::input_mode::InputMode::Katakana => "Katakana",
                crate::engine::input_mode::InputMode::Alphanumeric => "Alphanumeric",
            },
        });
    }

    factory.notify_langbar_update();
    factory.notify_tray_update(tid.unwrap_or_default());
    factory.maybe_reload_runtime_config();
}

fn handle_langbar_menu_command(factory: &TextServiceFactory_Impl, id: u32) {
    match id {
        ID_MENU_MODE_HIRAGANA => {
            apply_langbar_mode(factory, crate::engine::input_mode::InputMode::Hiragana);
        }
        ID_MENU_MODE_KATAKANA => {
            apply_langbar_mode(factory, crate::engine::input_mode::InputMode::Katakana);
        }
        ID_MENU_MODE_ALPHANUMERIC => {
            apply_langbar_mode(factory, crate::engine::input_mode::InputMode::Alphanumeric);
        }
        ID_MENU_SETTINGS => {
            settings_launcher::launch_settings_app();
        }
        ID_MENU_ENGINE_RELOAD => {
            tracing::info!("langbar menu: ID_MENU_ENGINE_RELOAD selected");
            crate::engine::config::init_config_manager();
            crate::engine::state::engine_reload();
        }
        _ => {}
    }
}

fn show_langbar_popup_menu(
    factory: &TextServiceFactory_Impl,
    pt: &POINT,
) -> windows::core::Result<()> {
    let open = factory
        .inner
        .try_borrow()
        .ok()
        .and_then(|inner| inner.thread_mgr.clone().map(|tm| get_open_close(&tm)))
        .unwrap_or(true);
    let current_mode = current_langbar_mode(open);

    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS;

        let menu = CreatePopupMenu()?;
        let hiragana = to_wide_menu_text("ひらがな");
        let katakana = to_wide_menu_text("カタカナ");
        let alnum = to_wide_menu_text("英数");
        let settings = to_wide_menu_text("設定...");
        let reload = to_wide_menu_text("エンジン再起動");

        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(
                if current_mode == crate::engine::input_mode::InputMode::Hiragana {
                    TF_LBMENUF_RADIOCHECKED
                } else {
                    0
                },
            ),
            ID_MENU_MODE_HIRAGANA as usize,
            windows::core::PCWSTR(hiragana.as_ptr()),
        );
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(
                if current_mode == crate::engine::input_mode::InputMode::Katakana {
                    TF_LBMENUF_RADIOCHECKED
                } else {
                    0
                },
            ),
            ID_MENU_MODE_KATAKANA as usize,
            windows::core::PCWSTR(katakana.as_ptr()),
        );
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(
                if current_mode == crate::engine::input_mode::InputMode::Alphanumeric {
                    TF_LBMENUF_RADIOCHECKED
                } else {
                    0
                },
            ),
            ID_MENU_MODE_ALPHANUMERIC as usize,
            windows::core::PCWSTR(alnum.as_ptr()),
        );
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, windows::core::PCWSTR::null());
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            ID_MENU_SETTINGS as usize,
            windows::core::PCWSTR(settings.as_ptr()),
        );
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(0),
            ID_MENU_ENGINE_RELOAD as usize,
            windows::core::PCWSTR(reload.as_ptr()),
        );

        let cmd = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
            pt.x,
            pt.y,
            0,
            GetForegroundWindow(),
            None,
        );
        let _ = DestroyMenu(menu);

        if cmd.0 != 0 {
            handle_langbar_menu_command(factory, cmd.0 as u32);
        }
    }

    Ok(())
}

fn to_wide_menu_text(text: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    wide
}

// ─── TextServiceState ─────────────────────────────────────────────────────────

pub struct TextServiceState {
    pub client_id: u32,
    pub thread_mgr: Option<ITfThreadMgr>,
    pub keymap: Keymap,
    pub langbar_sink: Option<ITfLangBarItemSink>,
    /// ITfThreadMgrEventSink の登録クッキー（Deactivate で解除）
    pub threadmgr_cookie: u32,
    /// ITfThreadFocusSink の登録クッキー（Deactivate で解除）
    pub threadfocus_cookie: u32,
}

impl Default for TextServiceState {
    fn default() -> Self {
        Self {
            client_id: 0,
            thread_mgr: None,
            keymap: Keymap::default(),
            langbar_sink: None,
            threadmgr_cookie: 0,
            threadfocus_cookie: 0,
        }
    }
}

// Safety: TSF は STA。RefCell + COM オブジェクトを持つが
// OnKeyDown は必ず STA スレッドから呼ばれる。
// windows-rs の #[implement] が要求するため付ける。
unsafe impl Send for TextServiceState {}

// ─── TextServiceFactory ───────────────────────────────────────────────────────

#[implement(
    IClassFactory,
    ITfTextInputProcessor,
    ITfKeyEventSink,
    ITfCompositionSink,
    ITfLangBarItemButton,
    ITfLangBarItem,
    ITfSource,
    ITfThreadMgrEventSink,
    ITfThreadFocusSink,
    ITfDisplayAttributeProvider
)]
pub struct TextServiceFactory {
    pub inner: RefCell<TextServiceState>,
}

unsafe impl Send for TextServiceFactory {}
unsafe impl Sync for TextServiceFactory {}

impl TextServiceFactory {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(TextServiceState::default()),
        }
    }
}

// ─── IClassFactory ────────────────────────────────────────────────────────────

impl IClassFactory_Impl for TextServiceFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Option<&IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut core::ffi::c_void,
    ) -> windows::core::Result<()> {
        if punkouter.is_some() {
            return Err(windows::core::Error::new(E_FAIL, "no aggregation"));
        }
        let svc = TextServiceFactory::new();
        let itp: ITfTextInputProcessor = svc.into();
        let unk: IUnknown = itp.cast()?;
        unsafe { unk.query(riid, ppvobject).ok() }
    }
    fn LockServer(&self, _: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

// ─── ITfTextInputProcessor ───────────────────────────────────────────────────

impl ITfTextInputProcessor_Impl for TextServiceFactory_Impl {
    fn Activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> windows::core::Result<()> {
        let _t = diag::span("Activate");
        let tm = ptim.ok_or_else(|| windows::core::Error::new(E_FAIL, "null thread_mgr"))?;

        {
            let mut inner = self
                .inner
                .try_borrow_mut()
                .map_err(|_| windows::core::Error::new(E_FAIL, "borrow_mut"))?;
            inner.client_id = tid;
            inner.thread_mgr = Some(tm.clone());
            inner.keymap = Keymap::load();
        }

        // OnSetFocus 遅延処理で set_open_close を呼ぶために ThreadMgr をキャッシュ
        candidate_window::cache_thread_mgr(tm.clone(), tid);

        // エンジン DLL は Activate では一切ロードしない。
        // Zoom / Dropbox 等の「IME を実際には使わないアプリ」では
        // rakukan_engine_*.dll（llama.cpp 同梱・重量級）を対象プロセスに
        // 持ち込むだけでクラッシュを誘発する事例があるため（msvcp140.dll の
        // クロスロード AV）、初回の実入力まで DLL ロードを完全に遅延する。
        //
        // 初回入力時に engine_try_get_or_create() が自動的に bg init を起動する。

        // KeyEventSink 登録
        unsafe {
            let km: ITfKeystrokeMgr = tm.cast().map_err(|e| {
                windows::core::Error::new(E_FAIL, format!("cast KeystrokeMgr: {e}"))
            })?;
            let ks: ITfKeyEventSink = self.cast().map_err(|e| {
                windows::core::Error::new(E_FAIL, format!("cast KeyEventSink: {e}"))
            })?;
            km.AdviseKeyEventSink(tid, &ks, TRUE).map_err(|e| {
                windows::core::Error::new(E_FAIL, format!("AdviseKeyEventSink: {e}"))
            })?;
        }

        // 言語バー登録
        unsafe {
            if let Ok(btn) = self.cast::<ITfLangBarItemButton>() {
                let ok = language_bar::langbar_add(tm, &btn).is_ok();
                diag::event(DiagEvent::LangbarAdd {
                    ok,
                    err: if ok { None } else { Some("see log".into()) },
                });
                if !ok {
                    tracing::warn!("langbar_add failed");
                }
            }
        }

        // KEYBOARD_OPENCLOSE を保存済み InputMode に合わせて設定する。
        // 常に true (on) にリセットすると、Alphanumeric モードでウィンドウを
        // 切り替えて戻るたびにターミナルが IME ON と誤認し、かな入力が再開する。
        // アトミックを使うことでロック競合なく正確なモードを読む。
        let is_open = {
            use crate::engine::input_mode::InputMode;
            crate::engine::state::input_mode_get_atomic() != InputMode::Alphanumeric
        };
        unsafe {
            let ok = match language_bar::set_open_close(tm, tid, is_open) {
                Ok(()) => {
                    tracing::info!(
                        "KEYBOARD_OPENCLOSE = {} ({})",
                        is_open as u8,
                        if is_open { "on" } else { "off" }
                    );
                    true
                }
                Err(e) => {
                    tracing::warn!("set_open_close FAILED: {e}");
                    false
                }
            };
            diag::event(DiagEvent::CompartmentSet {
                open: is_open,
                ok,
                err: None,
            });
        }

        // トレイ常駐プロセスへ現在モードを通知（失敗してもIMEは継続）
        {
            let mode = crate::engine::state::ime_state_get()
                .ok()
                .map(|s| s.input_mode)
                .unwrap_or_default();
            tray_ipc::publish(is_open, mode);
        }

        // ITfThreadMgrEventSink を登録してフォーカス変化を受け取る
        unsafe {
            if let Ok(src) = tm.cast::<ITfSource>() {
                let sink: ITfThreadMgrEventSink = self.cast().map_err(|e| {
                    windows::core::Error::new(E_FAIL, format!("cast ThreadMgrEventSink: {e}"))
                })?;
                let unk: IUnknown = sink.cast()?;
                match src.AdviseSink(&ITfThreadMgrEventSink::IID, &unk) {
                    Ok(cookie) => {
                        if let Ok(mut inner) = self.inner.try_borrow_mut() {
                            inner.threadmgr_cookie = cookie;
                        }
                        tracing::debug!("ITfThreadMgrEventSink registered cookie={cookie}");
                    }
                    Err(e) => tracing::warn!("AdviseSink(ThreadMgrEventSink) failed: {e}"),
                }
            }
        }

        // ITfThreadFocusSink を登録してスレッド (= アプリ) 単位のフォーカス消失を受け取る。
        // Alt+Tab 等で別アプリに移ったとき、ITfThreadMgrEventSink::OnSetFocus は TSF 対応
        // アプリ以外では発火しないため、これが無いと候補ウィンドウが残ることがある。
        unsafe {
            if let Ok(src) = tm.cast::<ITfSource>() {
                if let Ok(sink) = self.cast::<ITfThreadFocusSink>() {
                    if let Ok(unk) = sink.cast::<IUnknown>() {
                        match src.AdviseSink(&ITfThreadFocusSink::IID, &unk) {
                            Ok(cookie) => {
                                if let Ok(mut inner) = self.inner.try_borrow_mut() {
                                    inner.threadfocus_cookie = cookie;
                                }
                                tracing::debug!("ITfThreadFocusSink registered cookie={cookie}");
                            }
                            Err(e) => tracing::warn!("AdviseSink(ThreadFocusSink) failed: {e}"),
                        }
                    }
                }
            }
        }

        diag::event(DiagEvent::Activate { tid });
        tracing::info!("rakukan Activate client_id={tid}");

        // Display Attribute GUIDs を ITfCategoryMgr に登録して atom を取得
        unsafe {
            if let Ok(catmgr) = CoCreateInstance::<_, ITfCategoryMgr>(
                &CLSID_TF_CategoryMgr,
                None,
                CLSCTX_INPROC_SERVER,
            ) {
                let atom_input = catmgr
                    .RegisterGUID(&GUID_DISPLAY_ATTRIBUTE_INPUT)
                    .unwrap_or(0);
                let atom_conv = catmgr.RegisterGUID(&GUID_DISPLAY_ATTRIBUTE).unwrap_or(0);
                display_attr::set_atoms(atom_input, atom_conv);
                tracing::debug!("display attr atoms: input={atom_input} conv={atom_conv}");
            }
        }

        // Activate 時点で現在フォーカス中の DM に対して初期モードを適用する。
        // ITfThreadMgrEventSink の OnSetFocus は最初のフォーカスに対して呼ばれないことがある
        // ため、ここで config.input.default_mode を確定・適用する。
        {
            use crate::engine::input_mode::InputMode;
            let hwnd_val = foreground_root_hwnd();
            let focused_dm_ptr = {
                let inner = self.inner.try_borrow().ok();
                inner.and_then(|g| {
                    g.thread_mgr.as_ref().and_then(|tm| {
                        unsafe { tm.GetFocus().ok() }.map(|dm| {
                            use windows::core::Interface;
                            dm.as_raw() as usize
                        })
                    })
                })
            };
            if let Some(dm_ptr) = focused_dm_ptr {
                if let Some(mode) = doc_mode_on_focus_change(0, dm_ptr, hwnd_val) {
                    if let Ok(mut st) = crate::engine::state::ime_state_get() {
                        tracing::info!(
                            "Activate: initial mode={mode:?} (config.input.default_mode)"
                        );
                        st.set_mode(mode);
                    }
                    // KEYBOARD_OPENCLOSE を正しいモードで再設定
                    let is_open2 = mode != InputMode::Alphanumeric;
                    if let Ok(inner) = self.inner.try_borrow() {
                        if let Some(tm) = &inner.thread_mgr {
                            unsafe {
                                let _ = language_bar::set_open_close(tm, tid, is_open2);
                            }
                        }
                    }
                }
            }
        }

        // Activate 中に初期モードや OPENCLOSE を補正した後、言語バー/トレイ表示を同期する。
        // これを行わないと、実際のモードは Alphanumeric でも起動直後の表示だけ「あ」のまま
        // 残ることがある。
        self.notify_langbar_update();
        self.notify_tray_update(tid);

        Ok(())
    }

    fn Deactivate(&self) -> windows::core::Result<()> {
        diag::event(DiagEvent::Deactivate);
        let inner = self
            .inner
            .try_borrow()
            .map_err(|_| windows::core::Error::new(E_FAIL, "borrow"))?;
        if let Some(tm) = &inner.thread_mgr {
            unsafe {
                if let Ok(km) = tm.cast::<ITfKeystrokeMgr>() {
                    let _ = km.UnadviseKeyEventSink(inner.client_id);
                }
                if let Ok(btn) = self.cast::<ITfLangBarItemButton>() {
                    let _ = language_bar::langbar_remove(tm, &btn);
                }
                // ITfThreadMgrEventSink 登録解除
                if inner.threadmgr_cookie != 0 {
                    if let Ok(src) = tm.cast::<ITfSource>() {
                        let _ = src.UnadviseSink(inner.threadmgr_cookie);
                        tracing::debug!("ITfThreadMgrEventSink unregistered");
                    }
                }
                // ITfThreadFocusSink 登録解除
                if inner.threadfocus_cookie != 0 {
                    if let Ok(src) = tm.cast::<ITfSource>() {
                        let _ = src.UnadviseSink(inner.threadfocus_cookie);
                        tracing::debug!("ITfThreadFocusSink unregistered");
                    }
                }
            }
        }
        let _ = composition_set(None);
        if let Ok(mut g) = engine_get() {
            if let Some(e) = g.as_mut() {
                e.bg_reclaim();
            }
        }
        candidate_window::destroy();
        candidate_window::stop_live_timer();
        candidate_window::clear_thread_mgr();
        crate::tsf::mode_indicator::destroy();
        if let Ok(mut sess) = session_get() {
            sess.set_idle();
        }
        tracing::info!("rakukan Deactivate");
        Ok(())
    }
}

// ─── ITfCompositionSink ──────────────────────────────────────────────────────

impl ITfCompositionSink_Impl for TextServiceFactory_Impl {
    fn OnCompositionTerminated(
        &self,
        _: u32,
        _: Option<&ITfComposition>,
    ) -> windows::core::Result<()> {
        let _ = composition_set(None);
        // 候補ウィンドウと選択状態をクリア
        candidate_window::hide();
        candidate_window::stop_live_timer(); // LiveConv タイマーも停止
        if let Ok(mut sess) = session_get() {
            sess.set_idle();
        }
        // BG 変換の converter を先に回収してから（その後 reset_all で状態をクリア）
        // bg_reclaim は reset_all の前に呼ぶ
        // アプリが composition を強制終了した場合（例: メモ帳の最大 composition 長超過）、
        // composition テキストはアプリ側で確定済み。エンジンの hiragana_buf 等は
        // 不要になるため、converter の回収有無に関わらず必ず reset_all() を呼ぶ。
        // ※ 以前は conv が Some の場合に return していたため hiragana_buf が残り、
        //    次のキー入力で古いひらがなが末尾に追加される「途中切れ」バグがあった。
        if let Ok(mut g) = engine_get() {
            if let Some(e) = g.as_mut() {
                e.bg_reclaim();
                e.reset_all();
            }
        }
        tracing::debug!("OnCompositionTerminated");
        Ok(())
    }
}

// ─── ITfKeyEventSink ─────────────────────────────────────────────────────────

impl ITfKeyEventSink_Impl for TextServiceFactory_Impl {
    fn OnSetFocus(&self, _: BOOL) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnTestKeyDown(
        &self,
        _: Option<&ITfContext>,
        wparam: WPARAM,
        _: LPARAM,
    ) -> windows::core::Result<BOOL> {
        let vk = normalize_key_event_vk(wparam.0 as u16);
        let action = match self
            .inner
            .try_borrow()
            .ok()
            .and_then(|g| g.keymap.resolve_action(vk))
        {
            Some(a) => a,
            None => {
                // 重要キーは、keymap 取得に失敗しても確実に動かす（RefCell 競合対策）
                match vk {
                    0x0D => UserAction::CommitRaw, // VK_RETURN
                    0x20 => UserAction::Convert,   // VK_SPACE
                    0x08 => UserAction::Backspace, // VK_BACK
                    0x1B => UserAction::Cancel,    // VK_ESCAPE
                    0x1A => UserAction::ImeOff,    // VK_IME_OFF
                    0x16 => UserAction::ImeOn,     // VK_IME_ON
                    0x19 => UserAction::ImeToggle, // VK_KANJI (often IME toggle)
                    _ => return Ok(FALSE),
                }
            }
        };

        // ロックなし高速チェック: アトミックでモード取得（try_lock 失敗でも正確）
        let mode = crate::engine::state::input_mode_get_atomic();
        // コンパートメントは外部アプリへの「通知」であり、真の状態ではない。
        // 起動直後はコンパートメントが 0（オフ）のまま mode=Hiragana になる場合があり、
        // コンパートメントを参照すると ImeToggle が逆方向に動くバグを引き起こす。
        // → mode アトミックのみを正とし、コンパートメントは参照しない。
        let ime_off = mode == crate::engine::input_mode::InputMode::Alphanumeric;
        if ime_off {
            let eat = matches!(
                action,
                UserAction::ImeToggle
                    | UserAction::ImeOn
                    | UserAction::ImeOff
                    | UserAction::ModeHiragana
                    | UserAction::ModeKatakana
                    | UserAction::ModeAlphanumeric
            );
            return Ok(if eat { TRUE } else { FALSE });
        }

        let has_preedit = engine_try_get_or_create()
            .ok()
            .and_then(|g| g.as_ref().map(|e| !e.preedit_is_empty()))
            .unwrap_or(false);

        // 選択モード中はプリエディットありと同じ扱い（候補操作キーを消費するため）
        // AtomicBool でロックなし高速チェック
        let is_selecting = session_is_selecting_fast();

        Ok(if key_should_eat(&action, has_preedit || is_selecting) {
            TRUE
        } else {
            FALSE
        })
    }

    fn OnKeyDown(
        &self,
        pic: Option<&ITfContext>,
        wparam: WPARAM,
        _: LPARAM,
    ) -> windows::core::Result<BOOL> {
        // バックエンド初期化完了フラグを確認して言語バー表示を更新
        if crate::engine::state::langbar_update_take() {
            self.notify_langbar_update();
        }
        let _t = diag::span("OnKeyDown");
        let vk = normalize_key_event_vk(wparam.0 as u16);

        tracing::trace!("OnKeyDown vk={:#04x}", vk);

        // Ctrl+Shift+F12: 診断ダンプ
        if vk == 0x7B {
            let ctrl = unsafe { GetKeyState(0x11) as u16 & 0x8000 != 0 };
            let shift = unsafe { GetKeyState(0x10) as u16 & 0x8000 != 0 };
            if ctrl && shift {
                diag::dump_snapshot();
                return Ok(TRUE);
            }
        }

        let action = match self
            .inner
            .try_borrow()
            .ok()
            .and_then(|g| g.keymap.resolve_action(vk))
        {
            Some(a) => a,
            None => {
                tracing::debug!(
                    "OnKeyDown vk={:#04x} → unmapped (try_borrow={:?})",
                    vk,
                    self.inner.try_borrow().is_ok()
                );
                diag::event(DiagEvent::KeyIgnored {
                    vk,
                    reason: "unmapped",
                });
                return Ok(FALSE);
            }
        };
        let ctx = match pic {
            Some(c) => c.clone(),
            None => {
                diag::event(DiagEvent::KeyIgnored {
                    vk,
                    reason: "no_ctx",
                });
                return Ok(FALSE);
            }
        };
        let tid = self.inner.try_borrow().map(|g| g.client_id).unwrap_or(0);
        let sink: ITfCompositionSink = match unsafe { self.cast() } {
            Ok(s) => s,
            Err(_) => {
                diag::event(DiagEvent::KeyIgnored {
                    vk,
                    reason: "no_sink",
                });
                return Ok(FALSE);
            }
        };

        tracing::trace!("OnKeyDown vk={vk:#04x} action={action:?}");

        // モードインジケーターを非表示（キー入力があれば消す）
        crate::tsf::mode_indicator::hide();

        // ── 英数モードガード（最終防衛線）─────────────────────────────────
        // OnTestKeyDown が FALSE を返してもターミナル等が OnKeyDown を直接呼ぶ場合がある。
        // アトミックなのでロック競合なし。
        {
            use crate::engine::input_mode::InputMode;
            if crate::engine::state::input_mode_get_atomic() == InputMode::Alphanumeric {
                let is_ime_ctrl = matches!(
                    action,
                    UserAction::ImeToggle
                        | UserAction::ImeOn
                        | UserAction::ImeOff
                        | UserAction::ModeHiragana
                        | UserAction::ModeKatakana
                        | UserAction::ModeAlphanumeric
                );
                if !is_ime_ctrl {
                    diag::event(DiagEvent::KeyIgnored {
                        vk,
                        reason: "alphanumeric_mode",
                    });
                    return Ok(FALSE);
                }
            }
        }

        match self.handle_action(action.clone(), ctx, tid, sink) {
            Ok(ate) => {
                diag::event(DiagEvent::KeyHandled {
                    vk,
                    action: action_name(&action),
                    ate,
                });
                Ok(if ate { TRUE } else { FALSE })
            }
            Err(e) => {
                diag::event(DiagEvent::Error {
                    site: "handle_action",
                    msg: e.to_string(),
                });
                tracing::warn!("handle_action: {e}");
                Ok(FALSE)
            }
        }
    }

    fn OnTestKeyUp(
        &self,
        _: Option<&ITfContext>,
        _: WPARAM,
        _: LPARAM,
    ) -> windows::core::Result<BOOL> {
        Ok(FALSE)
    }
    fn OnKeyUp(&self, _: Option<&ITfContext>, _: WPARAM, _: LPARAM) -> windows::core::Result<BOOL> {
        Ok(FALSE)
    }
    fn OnPreservedKey(
        &self,
        _: Option<&ITfContext>,
        _: *const GUID,
    ) -> windows::core::Result<BOOL> {
        Ok(FALSE)
    }
}

fn normalize_key_event_vk(vk: u16) -> u16 {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;

    let ctrl = unsafe { GetKeyState(0x11) as u16 & 0x8000 != 0 };
    let shift = unsafe { GetKeyState(0x10) as u16 & 0x8000 != 0 };
    let alt = unsafe { GetKeyState(0x12) as u16 & 0x8000 != 0 };
    let space_down = unsafe { GetKeyState(0x20) as u16 & 0x8000 != 0 };

    if vk == 0x27 && ctrl && alt && !shift && space_down {
        return 0x20;
    }
    vk
}

// ─── handle_action ───────────────────────────────────────────────────────────

impl TextServiceFactory_Impl {
    fn notify_langbar_update(&self) {
        use windows::Win32::UI::TextServices::TF_LBI_ICON;
        const TF_LBI_TEXT: u32 = 2;
        if let Ok(inner) = self.inner.try_borrow() {
            if let Some(sink) = &inner.langbar_sink {
                unsafe {
                    let _ = sink.OnUpdate(TF_LBI_ICON | TF_LBI_TEXT);
                }
            }
        }
    }

    fn notify_tray_update(&self, tid: u32) {
        let open = self
            .inner
            .try_borrow()
            .ok()
            .and_then(|i| i.thread_mgr.clone().map(|tm| get_open_close(&tm)))
            .unwrap_or_else(|| {
                crate::engine::state::ime_state_get()
                    .ok()
                    .map(|s| s.input_mode != crate::engine::input_mode::InputMode::Alphanumeric)
                    .unwrap_or(true)
            });
        let mode = crate::engine::state::ime_state_get()
            .ok()
            .map(|s| s.input_mode)
            .unwrap_or_default();
        let _ = tid;
        tray_ipc::publish(open, mode);
    }

    /// モード切替時にキャレット近くにインジケーターを表示する。
    ///
    /// mozc と同じアプローチで TSF の `GetSelection` → `GetTextExt` を使い
    /// キャレット位置をリアルタイムに取得する。取得できない場合は表示しない。
    fn show_mode_indicator(&self, mode_name: &str, ctx: ITfContext, tid: u32) {
        use crate::tsf::edit_session::EditSession;
        use crate::tsf::mode_indicator;

        let mode_char: &'static str = match mode_name {
            "Hiragana" => "あ",
            "Katakana" => "ア",
            _ => "A",
        };

        let ctx2 = ctx.clone();
        let session = EditSession::new(move |ec| unsafe {
            // セレクション範囲を取得してキャレット位置を特定
            if let Some((x, y)) = get_caret_pos_from_context(&ctx2, ec) {
                mode_indicator::show(mode_char, x, y);
            }
            Ok(())
        });
        unsafe {
            let _ = ctx.RequestEditSession(tid, &session, TF_ES_READWRITE);
        }
    }

    fn maybe_reload_runtime_config(&self) {
        let config_changed = crate::engine::config::maybe_reload_on_mode_switch();
        let new_keymap = crate::engine::keymap::Keymap::load();
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.keymap = new_keymap;
        }
        if config_changed {
            tracing::info!("runtime config reloaded on input mode switch");
            crate::engine::state::engine_reload();
        }
    }

    fn handle_action(
        &self,
        action: UserAction,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
    ) -> Result<bool> {
        let mut guard = engine_try_get_or_create()?;
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };

        // ── 診断: 全アクションの入口でセッション状態とBG状態をログ ──
        {
            let bg = engine.bg_status();
            let state_name = if let Ok(s) = session_get() {
                match &*s {
                    SessionState::Idle => "Idle".to_string(),
                    SessionState::Preedit { text } => format!("Preedit({:?})", text),
                    SessionState::Waiting { text, .. } => format!("Waiting({:?})", text),
                    SessionState::Selecting {
                        original_preedit,
                        llm_pending,
                        candidates,
                        ..
                    } => format!(
                        "Selecting(op={:?} llm={} nc={})",
                        original_preedit,
                        llm_pending,
                        candidates.len()
                    ),
                    SessionState::LiveConv { reading, preview } => {
                        format!("LiveConv(r={:?} p={:?})", reading, preview)
                    }
                    SessionState::RangeSelect {
                        full_reading,
                        select_end,
                        ..
                    } => {
                        format!("RangeSelect(r={:?} end={})", full_reading, select_end)
                    }
                }
            } else {
                "lock_err".to_string()
            };
            tracing::debug!(
                "handle_action: {:?} state={} bg={} hira={:?}",
                action_name(&action),
                state_name,
                bg,
                engine.hiragana_text()
            );
        }

        // ── [Phase 1B] ライブ変換プレビューキューチェック ────────────────────
        // WM_TIMER から RequestEditSession が呼べない場合のフォールバック。
        // タイマーが書き込んだプレビューをここで拾って composition に反映する。
        // Preedit 状態のみ適用（変換中・選択中には適用しない）。
        //
        // キューエントリには書き込み時点の gen / reading が添えられており、
        // 現在の gen / reading と一致しない場合は stale として discard する
        // （M1.8 T-MID1: 中間文字消失 race の対策）。
        {
            use crate::engine::state::{live_conv_gen_snapshot, LIVE_PREVIEW_QUEUE, LIVE_PREVIEW_READY};
            if LIVE_PREVIEW_READY.swap(false, std::sync::atomic::Ordering::AcqRel) {
                if let Ok(mut q) = LIVE_PREVIEW_QUEUE.try_lock() {
                    if let Some(entry) = q.take() {
                        // stale 判定
                        let current_gen = live_conv_gen_snapshot();
                        let current_reading = engine.hiragana_text().to_string();
                        let stale_gen = entry.gen_when_requested != current_gen;
                        let stale_reading = entry.reading != current_reading;
                        if stale_gen || stale_reading {
                            tracing::warn!(
                                "[Live] Phase1B: discarded stale preview entry_gen={} cur_gen={} entry_reading={:?} cur_reading={:?}",
                                entry.gen_when_requested,
                                current_gen,
                                entry.reading,
                                current_reading
                            );
                            // Preedit 中のみ適用
                        } else {
                        let preview = entry.preview;
                        let apply = if let Ok(sess) = session_get() {
                            matches!(*sess, SessionState::Preedit { .. })
                        } else {
                            false
                        };

                        if apply {
                            // engine borrow は reading/pending 取得で終わり
                            // preedit = hiragana + pending_romaji 構成なので、
                            // BG 変換結果 `preview` に pending を付けて表示する
                            // ことで「ta」→「た」→「t」押下時の "t" が消えないようにする。
                            let reading = engine.hiragana_text().to_string();
                            let preedit_full = engine.preedit_display();
                            let pending =
                                text_util::suffix_after_prefix_or_empty(
                                    &preedit_full,
                                    &reading,
                                    "phase1b pending",
                                )
                                .to_string();
                            if !reading.is_empty() {
                                // 尻切れ防壁（M1.5 T-BUG2）: Phase1B 経路でも同じく
                                // preview の長さが reading に対して極端に短ければ破棄
                                let preview = candidate_window::sanity_check_preview(
                                    &reading,
                                    preview,
                                    "Phase1B",
                                );
                                tracing::info!(
                                    "[Live] Phase1B: applying preview={:?} reading={:?} pending={:?}",
                                    preview,
                                    reading,
                                    pending
                                );
                                if let Ok(mut sess) = session_get() {
                                    sess.set_live_conv(reading, preview.clone());
                                }
                                // engine の borrow はここで終わり（以降 engine を使わない）
                                drop(guard);
                                let ctx2 = ctx.clone();
                                let display_shown = if pending.is_empty() {
                                    preview
                                } else {
                                    format!("{preview}{pending}")
                                };
                                update_composition(ctx2, tid, sink.clone(), display_shown)?;
                                // guard と engine を再取得
                                guard = engine_try_get_or_create()?;
                            }
                        }
                        } // end stale check branch (non-stale)
                    }
                }
            }
        }
        // guard 再取得後に engine を更新（Phase 1B で再取得した場合に対応）
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };

        // LLM候補待機中に完了した場合、候補ウィンドウを自動更新
        if session_is_selecting_fast() {
            const DICT_LIMIT_POLL: usize = 40;
            if let Ok(mut sess) = session_get() {
                let poll_info = if let SessionState::Selecting {
                    ref original_preedit,
                    llm_pending,
                    ..
                } = *sess
                {
                    if llm_pending && engine.bg_status() == "done" {
                        Some(original_preedit.clone())
                    } else {
                        None
                    }
                } else {
                    None
                };
                if let Some(preedit_key) = poll_info {
                    tracing::debug!(
                        "poll: bg=done llm_pending=true key={:?}, calling bg_take_candidates",
                        preedit_key
                    );
                    match engine.bg_take_candidates(&preedit_key) {
                        Some(llm_cands) => {
                            tracing::debug!(
                                "poll: bg_take_candidates → Some({} cands)",
                                llm_cands.len()
                            );
                            let merged = engine.merge_candidates(llm_cands, DICT_LIMIT_POLL);
                            tracing::debug!("poll: merge_candidates → {:?}", merged);
                            if !merged.is_empty() {
                                let first = merged.first().cloned().unwrap_or_default();
                                if let SessionState::Selecting {
                                    ref mut candidates,
                                    ref mut selected,
                                    ref mut llm_pending,
                                    ..
                                } = *sess
                                {
                                    *candidates = merged;
                                    *selected = 0;
                                    *llm_pending = false;
                                }
                                let page_cands = sess.page_candidates().to_vec();
                                let page_info = sess.page_info();
                                let prefix = sess.selecting_prefix_clone();
                                let remainder = sess.selecting_remainder_clone();
                                let pos = caret_rect_get();
                                drop(sess);
                                drop(guard);
                                candidate_window::show_with_status(
                                    &page_cands,
                                    0,
                                    &page_info,
                                    pos.left,
                                    pos.bottom,
                                    None,
                                );
                                update_composition_candidate_parts(
                                    ctx, tid, sink, prefix, first, remainder,
                                )?;
                                return Ok(true);
                            }
                        }
                        None => {
                            // take_ready がキー不一致で None を返した: Done 状態は保持されたまま
                            // llm_pending はそのままにしておく（次のキー/Space で再試行できる）
                            tracing::warn!(
                                "poll: bg_take_candidates → None (key mismatch or lock busy), bg={}",
                                engine.bg_status()
                            );
                        }
                    }
                }
            }
        }

        // 辞書0件でLLM完了待機中（選択モード外）→ BG完了したら選択モードへ遷移
        // Cancel/CancelAll はこのポーリングをスキップして on_cancel に直接渡す
        // （ポーリングで Waiting→Preedit 遷移すると on_cancel が fallthrough して全消去になる）
        let is_cancel = matches!(action, UserAction::Cancel | UserAction::CancelAll);
        if !is_cancel {
            const DICT_LIMIT_WAIT: usize = 40;
            if let Ok(mut sess) = session_get() {
                if let Some((wait_preedit, pos_x, pos_y)) =
                    sess.waiting_info().map(|(t, x, y)| (t.to_string(), x, y))
                {
                    let bg_now = engine.bg_status();
                    tracing::debug!(
                        "waiting-poll: wait_preedit={:?} bg={}",
                        wait_preedit,
                        bg_now
                    );
                    if bg_now == "done" {
                        tracing::debug!(
                            "waiting-poll: calling bg_take_candidates({:?})",
                            wait_preedit
                        );
                        match engine.bg_take_candidates(&wait_preedit) {
                            Some(llm_cands) => {
                                tracing::debug!("waiting-poll: got {} LLM cands", llm_cands.len());
                                // LLM候補とマージ。llm_cands が空でも辞書候補がある場合はそちらを使う。
                                let merged = if llm_cands.is_empty() {
                                    engine.merge_candidates(vec![], DICT_LIMIT_WAIT)
                                } else {
                                    engine.merge_candidates(llm_cands, DICT_LIMIT_WAIT)
                                };
                                tracing::debug!("waiting-poll: merged={} cands", merged.len());
                                // preedit 1件だけでも候補ウィンドウを出す（辞書/LLMどちらかにヒットした）
                                if !merged.is_empty() {
                                    let first = merged.first().cloned().unwrap_or_default();
                                    sess.activate_selecting(
                                        merged,
                                        wait_preedit.clone(),
                                        pos_x,
                                        pos_y,
                                        false,
                                    );
                                    let page_cands = sess.page_candidates().to_vec();
                                    let page_info = sess.page_info();
                                    drop(sess);
                                    drop(guard);
                                    candidate_window::stop_waiting_timer();
                                    candidate_window::show_with_status(
                                        &page_cands,
                                        0,
                                        &page_info,
                                        pos_x,
                                        pos_y,
                                        None,
                                    );
                                    update_composition(ctx, tid, sink, first)?;
                                    return Ok(true);
                                }
                            }
                            None => {
                                // キー不一致 or ロック競合 → Done 状態は保持されたまま
                                // Waiting 状態を維持して次のキー/Space で再試行
                                tracing::warn!(
                                    "waiting-poll: bg_take_candidates → None (key mismatch?), bg={}",
                                    engine.bg_status()
                                );
                            }
                        }
                        // merged が空（LLM候補なし）だった場合のみ preedit に戻す
                        // None だった場合は Waiting を維持（→ Cancel や次のSpace で対処）
                    }
                }
            }
        } // if !is_cancel

        match action {
            UserAction::Input(c) => self.on_input(c, ctx, tid, sink, guard),
            UserAction::InputRaw(c) => self.on_input_raw(c, ctx, tid, sink, guard),
            UserAction::FullWidthSpace => self.on_full_width_space(ctx, tid, guard),
            UserAction::Convert => self.on_convert(ctx, tid, sink, guard),
            UserAction::CommitRaw => self.on_commit_raw(ctx, tid, sink, guard),
            UserAction::Backspace => self.on_backspace(ctx, tid, sink, guard),
            UserAction::Cancel | UserAction::CancelAll => self.on_cancel(ctx, tid, sink, guard),
            UserAction::Hiragana => {
                self.on_kana_convert(ctx, tid, sink, guard, text_util::to_hiragana)
            }
            UserAction::Katakana => {
                self.on_kana_convert(ctx, tid, sink, guard, text_util::to_katakana)
            }
            UserAction::HalfKatakana => {
                self.on_kana_convert(ctx, tid, sink, guard, text_util::to_half_katakana)
            }
            UserAction::FullLatin => self.on_latin_convert(ctx, tid, sink, guard, true),
            UserAction::HalfLatin => self.on_latin_convert(ctx, tid, sink, guard, false),
            UserAction::CycleKana => self.on_cycle_kana(ctx, tid, guard),
            UserAction::CandidateNext => {
                self.on_candidate_move(ctx, tid, sink, guard, CandidateDir::Next)
            }
            UserAction::CandidatePrev => {
                self.on_candidate_move(ctx, tid, sink, guard, CandidateDir::Prev)
            }
            UserAction::CandidatePageDown => {
                self.on_candidate_page(ctx, tid, sink, guard, CandidateDir::Next)
            }
            UserAction::CandidatePageUp => {
                self.on_candidate_page(ctx, tid, sink, guard, CandidateDir::Prev)
            }
            UserAction::CandidateSelect(n) => self.on_candidate_select(n, ctx, tid, sink, guard),
            UserAction::CursorLeft => self.on_segment_move_left(ctx, tid, sink, guard),
            UserAction::CursorRight => self.on_segment_move_right(ctx, tid, sink, guard),
            UserAction::Punctuate(c) => self.on_punctuate(c, ctx, tid, sink, guard),
            UserAction::SegmentShrink => self.on_segment_shrink(ctx, tid, sink, guard),
            UserAction::SegmentExtend => self.on_segment_extend(ctx, tid, sink, guard),
            UserAction::ImeToggle => {
                drop(guard);
                self.on_ime_toggle(ctx, tid)
            }
            UserAction::ImeOff | UserAction::ModeAlphanumeric => {
                drop(guard);
                self.on_ime_off(ctx, tid)
            }
            UserAction::ImeOn => {
                drop(guard);
                self.on_ime_on(ctx, tid)
            }
            UserAction::ModeHiragana => self.on_mode_hiragana(ctx, tid, guard),
            UserAction::ModeKatakana => self.on_mode_katakana(ctx, tid, guard),
            _ => Ok(false),
        }
    }
}

// ─── CandidateDir ─────────────────────────────────────────────────────────────

enum CandidateDir {
    Next,
    Prev,
}

// ─── アクション実装（impl TextServiceFactory_Impl）────────────────────────────

impl TextServiceFactory_Impl {
    fn prepare_for_direct_input(&self) -> Result<()> {
        if let Ok(mut sess) = session_get() {
            if sess.is_waiting() {
                let pre = sess.preedit_text().unwrap_or("").to_string();
                sess.set_preedit(pre);
                candidate_window::hide();
            }
        }
        Ok(())
    }

    fn on_input(
        &self,
        c: char,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        // M1.8 T-MID1: キー入力は reading を変化させるので、ライブ変換世代を
        // 前進させる。Phase1B キューに残っている古い preview は apply 時に
        // gen 不一致で discard される。
        crate::engine::state::live_conv_gen_bump();
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => {
                // M1.6 T-HOST4: engine 未ロード中の握り潰しを撤去。
                // キーを後で replay するためにバッファへ積み、composition は
                // 空のままにして次回復帰時にまとめて流し込む。return Ok(true) で
                // アプリ側にはキーが消費されたことを示す（アプリがそのまま受け
                // 取ってしまうと二重入力になるため）。
                let kind = if c.is_ascii_uppercase() {
                    crate::engine::state::InputCharKind::FullwidthAlpha
                } else {
                    crate::engine::state::InputCharKind::Char
                };
                crate::engine::state::push_pending_key(c, kind, false);
                // M1.6 T-HOST3: 読込中のキャレット近傍フィードバック。
                // 経過時間に応じて記号を切り替える。位置は (0,0) で
                // get_caret_screen_pos() fallback に任せる。
                let (sym, _msg) = loading_indicator_symbol();
                crate::tsf::mode_indicator::show(sym, 0, 0);
                return Ok(true);
            }
        };
        // engine が復帰した時点で、過去に積んだキーを先に replay する。
        // 現在の c を処理する前に hiragana_buf を最新状態に揃えることで
        // 「先に押したキーほど先に反映される」挙動を保つ。
        {
            let pending = crate::engine::state::drain_pending_keys();
            for (pc, pk, raw) in pending {
                if raw {
                    engine.push_raw(pc);
                } else {
                    let _ = engine.input_char(pc, pk, None);
                }
            }
        }
        crate::engine::state::maybe_log_gpu_memory(engine);
        let _t = diag::span("Input");

        if let Ok(mut sess) = session_get() {
            crate::engine::state::SUPPRESS_LIVE_COMMIT_ONCE
                .store(false, std::sync::atomic::Ordering::Release);
            if sess.is_live_conv() {
                let (reading, preview) = sess
                    .live_conv_parts()
                    .map(|(r, p)| (r.to_string(), p.to_string()))
                    .unwrap_or_default();
                candidate_window::hide();
                candidate_window::stop_live_timer();
                use crate::engine::state::{LIVE_PREVIEW_QUEUE, LIVE_PREVIEW_READY};
                LIVE_PREVIEW_READY.store(false, std::sync::atomic::Ordering::Release);
                if let Ok(mut q) = LIVE_PREVIEW_QUEUE.try_lock() {
                    *q = None;
                }

                let kind = if c.is_ascii_uppercase() {
                    crate::engine::state::InputCharKind::FullwidthAlpha
                } else {
                    crate::engine::state::InputCharKind::Char
                };
                let live_beam = crate::engine::state::get_live_conv_beam_size();
                let (preedit, new_reading, _bg) = engine.input_char(c, kind, Some(live_beam));
                let suffix = new_reading
                    .strip_prefix(&reading)
                    .unwrap_or(new_reading.as_str())
                    .to_string();
                // preedit = hiragana + pending_romaji の構成なので、
                // hiragana 長を超えた部分が未確定ローマ字。
                // 表示にはこれを末尾に付けて見えるようにするが、
                // session に保存する display はひらがなのみの版にする
                // （次回 suffix 計算や BG preview 更新で汚染されないように）。
                let pending = text_util::suffix_after_prefix_or_empty(
                    &preedit,
                    &new_reading,
                    "live_conv input pending",
                );
                let display_hira = format!("{preview}{suffix}");
                let display_shown = format!("{display_hira}{pending}");
                sess.set_live_conv(new_reading.clone(), display_hira);
                diag::event(DiagEvent::InputChar {
                    ch: c,
                    preedit_after: display_shown.clone(),
                });
                drop(sess);
                drop(guard);
                candidate_window::live_input_notify(&ctx, tid);
                update_composition(ctx, tid, sink, display_shown)?;
                return Ok(true);
            }
            // RangeSelect 中の入力 → キャンセルしてひらがなに戻す
            if sess.is_range_select() {
                if let SessionState::RangeSelect { full_reading, .. } = &*sess {
                    let reading = full_reading.clone();
                    sess.set_preedit(reading.clone());
                    candidate_window::hide();
                    engine.force_preedit(reading);
                }
            }
        }

        self.prepare_for_direct_input()?;

        if session_is_selecting_fast() {
            let mut sess = session_get()?;
            if sess.is_selecting() {
                let selected_text = sess
                    .current_candidate()
                    .or_else(|| sess.original_preedit())
                    .unwrap_or("")
                    .to_string();
                let reading = sess.original_preedit().unwrap_or("").to_string();
                let prefix = sess.selecting_prefix_clone();
                let punct = sess.take_punct_pending();
                let remainder = sess.take_selecting_remainder();
                sess.set_idle();
                drop(sess);
                candidate_window::hide();
                candidate_window::stop_live_timer();
                let committed_text = if let Some(p) = punct {
                    format!("{selected_text}{p}")
                } else {
                    selected_text.clone()
                };
                let full_text = format!("{prefix}{committed_text}{remainder}");
                if selected_text != reading && crate::engine::state::is_auto_learn_enabled() {
                    engine.learn(&reading, &selected_text);
                }
                engine.commit(&full_text);
                engine.reset_preedit();
                drop(guard);

                let mut guard2 = engine_try_get_or_create()?;
                let engine2 = match guard2.as_mut() {
                    Some(e) => e,
                    None => return Ok(true),
                };
                let kind = if c.is_ascii_uppercase() {
                    crate::engine::state::InputCharKind::FullwidthAlpha
                } else {
                    crate::engine::state::InputCharKind::Char
                };
                // 打鍵時の prefetch は live プレビュー用なので live_conv_beam_size を使う
                // (num_candidates=30 を渡すと毎打鍵で重い beam=30 変換が走りワーカーが詰まる)
                let live_beam = crate::engine::state::get_live_conv_beam_size();
                let (preedit, _hiragana, _bg) = engine2.input_char(c, kind, Some(live_beam));
                diag::event(DiagEvent::InputChar {
                    ch: c,
                    preedit_after: preedit.clone(),
                });
                drop(guard2);
                commit_then_start_composition(ctx, tid, sink, full_text, preedit)?;
                return Ok(true);
            }
        }
        // SESSION_SELECTING=true だったが is_selecting()=false の場合はここに来る

        // ラッチ付き ready ポーリング: ready 後は RPC スキップ。
        let _ = crate::engine::state::poll_dict_ready_cached(engine);
        let _ = crate::engine::state::poll_model_ready_cached(engine);

        // バッチ RPC: push + preedit + hiragana + bg_status + 条件付き bg_start
        // を 1 往復で処理する。0.4.5 で 1 打鍵 8〜9 RPC → 1 RPC に短縮。
        //
        // 打鍵時の prefetch は live プレビュー用なので live_conv_beam_size を使う。
        // ここで num_candidates (最大 30) を渡すと毎打鍵で重い beam=30 変換が走り
        // ワーカーが詰まってライブプレビューが遅延する。Space 押下時は on_convert 内で
        // bg_reclaim + bg_start(num_candidates) により fresh に変換し直す。
        let live_beam = crate::engine::state::get_live_conv_beam_size();
        let kind = if c.is_ascii_uppercase() {
            crate::engine::state::InputCharKind::FullwidthAlpha
        } else {
            crate::engine::state::InputCharKind::Char
        };
        let (preedit, hiragana, bg_status) = engine.input_char(c, kind, Some(live_beam));
        diag::event(DiagEvent::InputChar {
            ch: c,
            preedit_after: preedit.clone(),
        });
        tracing::trace!("Input: hiragana={:?} bg={}", hiragana, bg_status);

        if !hiragana.is_empty() {
            drop(guard);
            // [Phase0] ライブ変換実験: コンテキストをキャッシュしてタイマーを起動
            candidate_window::live_input_notify(&ctx, tid);
            update_composition(ctx, tid, sink, preedit)?;
            return Ok(true);
        }
        drop(guard);
        update_composition(ctx, tid, sink, preedit)?;
        Ok(true)
    }

    /// ローマ字変換を経由せず hiragana_buf に直接書き込む入力処理。
    /// テンキー記号（/ * - + .）など、かなルールに登録されている文字を
    /// そのまま入力する場合に使用する。
    fn on_input_raw(
        &self,
        c: char,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        // M1.8 T-MID1: reading 変化経路。on_input と同じく gen を前進させる。
        crate::engine::state::live_conv_gen_bump();
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => {
                // M1.6 T-HOST4: raw 経路（テンキー記号等）も握り潰しをやめて
                // buffer へ。raw フラグを立てて後で `push_raw` 経由で replay する。
                crate::engine::state::push_pending_key(
                    c,
                    crate::engine::state::InputCharKind::Char,
                    true,
                );
                // M1.6 T-HOST3: 読込中の視覚フィードバック
                let (sym, _msg) = loading_indicator_symbol();
                crate::tsf::mode_indicator::show(sym, 0, 0);
                return Ok(true);
            }
        };
        // 積まれていた未処理キーを先に流し込む（on_input と同じ replay ポリシー）。
        {
            let pending = crate::engine::state::drain_pending_keys();
            for (pc, pk, raw) in pending {
                if raw {
                    engine.push_raw(pc);
                } else {
                    let _ = engine.input_char(pc, pk, None);
                }
            }
        }
        crate::engine::state::maybe_log_gpu_memory(engine);
        if let Ok(mut sess) = session_get() {
            crate::engine::state::SUPPRESS_LIVE_COMMIT_ONCE
                .store(false, std::sync::atomic::Ordering::Release);
            if sess.is_live_conv() {
                let (reading, preview) = sess
                    .live_conv_parts()
                    .map(|(r, p)| (r.to_string(), p.to_string()))
                    .unwrap_or_default();
                candidate_window::hide();
                candidate_window::stop_live_timer();
                use crate::engine::state::{LIVE_PREVIEW_QUEUE, LIVE_PREVIEW_READY};
                LIVE_PREVIEW_READY.store(false, std::sync::atomic::Ordering::Release);
                if let Ok(mut q) = LIVE_PREVIEW_QUEUE.try_lock() {
                    *q = None;
                }

                engine.push_raw(c);
                let new_reading = engine.hiragana_text().to_string();
                let suffix = new_reading
                    .strip_prefix(&reading)
                    .unwrap_or(new_reading.as_str())
                    .to_string();
                let display = format!("{preview}{suffix}");
                sess.set_live_conv(new_reading.clone(), display.clone());
                engine.bg_start(crate::engine::state::get_live_conv_beam_size());
                drop(sess);
                drop(guard);
                candidate_window::live_input_notify(&ctx, tid);
                update_composition(ctx, tid, sink, display)?;
                return Ok(true);
            }
        }

        self.prepare_for_direct_input()?;
        if session_is_selecting_fast() {
            let mut sess = session_get()?;
            if sess.is_selecting() {
                let selected_text = sess
                    .current_candidate()
                    .or_else(|| sess.original_preedit())
                    .unwrap_or("")
                    .to_string();
                let reading = sess.original_preedit().unwrap_or("").to_string();
                let prefix = sess.selecting_prefix_clone();
                let punct = sess.take_punct_pending();
                let remainder = sess.take_selecting_remainder();
                sess.set_idle();
                drop(sess);
                candidate_window::hide();
                candidate_window::stop_live_timer();
                let committed_text = if let Some(p) = punct {
                    format!("{selected_text}{p}")
                } else {
                    selected_text.clone()
                };
                let full_text = format!("{prefix}{committed_text}{remainder}");
                if selected_text != reading && crate::engine::state::is_auto_learn_enabled() {
                    engine.learn(&reading, &selected_text);
                }
                engine.commit(&full_text);
                engine.reset_preedit();
                drop(guard);

                let mut guard2 = engine_try_get_or_create()?;
                let engine2 = match guard2.as_mut() {
                    Some(e) => e,
                    None => return Ok(true),
                };
                engine2.push_raw(c);
                let preedit = engine2.preedit_display();
                // 打鍵時の prefetch はライブプレビュー用なので beam=live_conv_beam_size
                // （num_candidates=30 だと毎打鍵で重い beam=30 変換が走り、ワーカーが詰まる）。
                // Space 押下時は別途 bg_reclaim + bg_start(num_candidates) で fresh に変換する。
                let live_beam = crate::engine::state::get_live_conv_beam_size();
                engine2.bg_start(live_beam);
                drop(guard2);
                commit_then_start_composition(ctx, tid, sink, full_text, preedit)?;
                return Ok(true);
            }
        }
        engine.push_raw(c);
        let preedit = engine.preedit_display();
        // 打鍵時の prefetch はライブプレビュー用なので beam=live_conv_beam_size を使う。
        // num_candidates (最大 30) を使うと毎打鍵で重い beam=30 変換が走り、ワーカーが
        // 詰まってライブプレビューが更新されない。Space 押下時は on_convert 内で
        // bg_reclaim + bg_start(num_candidates) により fresh に変換し直すため、
        // ここの prefetch 結果は Space には流用されない（キャッシュは捨てられる）。
        let live_beam = crate::engine::state::get_live_conv_beam_size();
        engine.bg_start(live_beam);
        candidate_window::live_input_notify(&ctx, tid);
        drop(guard);
        update_composition(ctx, tid, sink, preedit)?;
        Ok(true)
    }

    fn on_full_width_space(
        &self,
        ctx: ITfContext,
        tid: u32,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        let preedit = engine.preedit_display();
        if !preedit.is_empty() {
            engine.commit(&preedit.clone());
            engine.reset_preedit();
            drop(guard);
            end_composition(ctx.clone(), tid, preedit)?;
        } else {
            drop(guard);
        }
        commit_text(ctx, tid, "　".into())?;
        Ok(true)
    }

    fn on_convert(
        &self,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        crate::engine::state::maybe_log_gpu_memory(engine);
        let _t = diag::span("Convert");
        update_caret_rect(ctx.clone(), tid);
        engine.flush_pending_n();
        let preedit_empty = engine.preedit_is_empty();
        if let Ok(sess) = session_get() {
            tracing::debug!(
                "on_convert: preedit_empty={} is_selecting={} state={:?}",
                preedit_empty,
                sess.is_selecting(),
                &*sess
            );
        }
        if preedit_empty {
            use crate::engine::input_mode::InputMode;
            drop(guard);
            match crate::engine::state::input_mode_get_atomic() {
                InputMode::Hiragana | InputMode::Katakana => {
                    commit_text(ctx, tid, "　".into())?;
                    return Ok(true);
                }
                InputMode::Alphanumeric => {
                    commit_text(ctx, tid, " ".into())?;
                    return Ok(true);
                }
            }
        }

        // ── LiveConv（ライブ変換表示中）: Space → reading で通常変換へ ──────
        // engine の hiragana_buf は LiveConv 遷移後も変化していないため、
        // session を Preedit に戻すだけで通常の on_convert フローに乗れる。
        {
            let mut sess = session_get()?;
            if sess.is_live_conv() {
                let reading = sess
                    .live_conv_parts()
                    .map(|(r, _)| r.to_string())
                    .unwrap_or_default();
                tracing::debug!(
                    "[Live] on_convert: LiveConv → Preedit reading={:?}",
                    reading
                );
                sess.set_preedit(reading.clone());
                drop(sess);
                // LIVE_PREVIEW_QUEUE をクリア
                use crate::engine::state::{LIVE_PREVIEW_QUEUE, LIVE_PREVIEW_READY};
                LIVE_PREVIEW_READY.store(false, std::sync::atomic::Ordering::Release);
                if let Ok(mut q) = LIVE_PREVIEW_QUEUE.try_lock() {
                    *q = None;
                }
                // タイマーは止めない（変換中は timer が発火しても Preedit でなければスキップ）
            }
        }

        // ── RangeSelect 中: 選択範囲を変換して候補表示 ──
        {
            let mut sess = session_get()?;
            if sess.is_range_select() {
                let (selected, unselected) = sess.range_select_parts().unwrap_or_default();
                if selected.is_empty() {
                    return Ok(true);
                }
                // Preedit に遷移して通常変換フローへ
                // engine の hiragana_buf を選択範囲に設定
                engine.bg_reclaim();
                engine.force_preedit(selected.clone());
                sess.set_preedit(selected.clone());
                // remainder を Selecting に渡すために保持
                let remainder = unselected.clone();
                let remainder_reading = unselected;
                drop(sess);
                candidate_window::stop_live_timer();

                // on_convert[new] と同じ「bg_start → 短時間 inline 待機 → 取れなければ
                // WM_TIMER fallback」方式に統一。旧実装の `convert_sync` + `bg_wait_ms(1500)`
                // の二重ブロック（最長数秒）を排除し、hot path のロック占有を 250ms 以下に抑える。
                const LLM_WAIT_INLINE_MS: u64 = 250;
                const DICT_LIMIT: usize = 40;
                let n_cands = crate::engine::state::get_num_candidates();
                let kanji_ready = engine.is_kanji_ready();
                let target = selected;
                let caret = caret_rect_get();

                if !kanji_ready {
                    // モデル未ロード → target をそのままプレビューとして Selecting 化
                    let candidates = vec![target.clone()];
                    {
                        let mut sess = session_get()?;
                        sess.activate_selecting_with_affixes(
                            candidates.clone(),
                            target,
                            caret.left,
                            caret.bottom,
                            false,
                            String::new(),
                            String::new(),
                            remainder,
                            remainder_reading,
                        );
                    }
                    drop(guard);
                    candidate_window::show(&candidates, 0, "", caret.left, caret.bottom);
                    return Ok(true);
                }

                // ⏳ 表示して待機状態にしてから bg_start
                if let Ok(mut sess) = session_get() {
                    sess.set_waiting_with_affixes(
                        target.clone(),
                        caret.left,
                        caret.bottom,
                        remainder.clone(),
                        remainder_reading.clone(),
                    );
                }
                let dummy = vec![target.clone()];
                candidate_window::show_with_status(
                    &dummy,
                    0,
                    "",
                    caret.left,
                    caret.bottom,
                    Some("⏳ 変換中..."),
                );

                if engine.bg_status() != "done" {
                    engine.bg_start(n_cands);
                }
                let completed = engine.bg_wait_ms(LLM_WAIT_INLINE_MS);
                if !completed {
                    // 短時間で終わらない → WM_TIMER fallback（Waiting の remainder は保持済み）
                    drop(guard);
                    candidate_window::start_waiting_timer();
                    return Ok(true);
                }

                // inline 完走 → 取得してマージ
                let llm_cands = engine.bg_take_candidates(&target).unwrap_or_default();
                let candidates = engine.merge_candidates(llm_cands, DICT_LIMIT);
                let candidates = if candidates.is_empty() {
                    vec![target.clone()]
                } else {
                    candidates
                };

                {
                    let mut sess = session_get()?;
                    sess.activate_selecting_with_affixes(
                        candidates.clone(),
                        target,
                        caret.left,
                        caret.bottom,
                        false,
                        String::new(),
                        String::new(),
                        remainder,
                        remainder_reading,
                    );
                }
                drop(guard);
                candidate_window::stop_waiting_timer();
                let page_size = 9usize;
                let page_cands: Vec<String> = candidates.into_iter().take(page_size).collect();
                candidate_window::show(&page_cands, 0, "", caret.left, caret.bottom);
                return Ok(true);
            }
        }

        let preedit = engine.preedit_display();

        // すでに選択モード中 → 1候補ずつ進む
        {
            let mut sess = session_get()?;
            if sess.is_selecting() {
                // llm_pending=true の場合はLLM完了を確認して候補を更新
                let llm_pending = matches!(
                    *sess,
                    SessionState::Selecting {
                        llm_pending: true,
                        ..
                    }
                );
                if llm_pending {
                    let original_preedit = if let SessionState::Selecting {
                        ref original_preedit,
                        ..
                    } = *sess
                    {
                        original_preedit.clone()
                    } else {
                        String::new()
                    };
                    drop(sess);

                    // 非ブロッキングでLLM完了を確認（最大500ms待機）
                    const WAIT_MS: u64 = 500;
                    let bg_before = engine.bg_status();
                    tracing::debug!(
                        "on_convert[llm_pending]: key={:?} bg={} → wait_ms({})",
                        original_preedit,
                        bg_before,
                        WAIT_MS
                    );
                    if engine.bg_status() == "running" {
                        engine.bg_wait_ms(WAIT_MS);
                    }
                    let _ = crate::engine::state::poll_model_ready_cached(engine);

                    let bg_done = engine.bg_status() == "done";
                    tracing::debug!("on_convert[llm_pending]: after wait bg_done={}", bg_done);
                    const DICT_LIMIT: usize = 40;

                    if bg_done {
                        // LLM完了 → 候補をマージして表示
                        // hiragana_text() でキャッシュの実際のキーを確認してから呼ぶ
                        let hira_key = engine.hiragana_text();
                        tracing::debug!(
                            "on_convert[llm_pending]: calling bg_take_candidates op={:?}({}) hira={:?}({})",
                            original_preedit,
                            original_preedit.len(),
                            hira_key,
                            hira_key.len()
                        );
                        // op と hira が一致する方をキーとして使う（バイト数も確認）
                        let take_key = if hira_key == original_preedit {
                            original_preedit.clone()
                        } else {
                            tracing::warn!(
                                "on_convert[llm_pending]: op/hira differ, using hira={:?}",
                                hira_key
                            );
                            hira_key
                        };
                        match engine.bg_take_candidates(&take_key) {
                            Some(llm_cands) => {
                                tracing::debug!(
                                    "on_convert[llm_pending]: bg_take_candidates → Some({} cands)",
                                    llm_cands.len()
                                );
                                let merged = engine.merge_candidates(llm_cands, DICT_LIMIT);
                                tracing::debug!("merge_candidates → {:?}", merged);
                                tracing::debug!(
                                    "on_convert[llm_pending]: merged={} cands",
                                    merged.len()
                                );
                                if !merged.is_empty() {
                                    if let Ok(mut sess2) = session_get() {
                                        if let SessionState::Selecting {
                                            ref mut candidates,
                                            ref mut selected,
                                            ref mut llm_pending,
                                            ..
                                        } = *sess2
                                        {
                                            *candidates = merged;
                                            *selected = 0;
                                            *llm_pending = false;
                                        }
                                        let page_cands = sess2.page_candidates().to_vec();
                                        let page_info = sess2.page_info();
                                        let cand_text = sess2
                                            .current_candidate()
                                            .or_else(|| sess2.original_preedit())
                                            .unwrap_or("")
                                            .to_string();
                                        let prefix = sess2.selecting_prefix_clone();
                                        let remainder = sess2.selecting_remainder_clone();
                                        let pos = caret_rect_get();
                                        drop(sess2);
                                        drop(guard);
                                        candidate_window::show(
                                            &page_cands,
                                            0,
                                            &page_info,
                                            pos.left,
                                            pos.bottom,
                                        );
                                        update_composition_candidate_parts(
                                            ctx, tid, sink, prefix, cand_text, remainder,
                                        )?;
                                        return Ok(true);
                                    }
                                }
                            }
                            None => {
                                // bg_reclaim で converter を強制回収 → 即 bg_start で再変換起動
                                // (bg_reclaim だけして bg_start しないと converter が engine に戻ったまま
                                //  次の変換が永遠に起動されない)
                                let bg_now = engine.bg_status();
                                tracing::warn!(
                                    "on_convert[llm_pending]: take_key={:?}({}) returned None, bg={}. reclaim+restart.",
                                    take_key,
                                    take_key.len(),
                                    bg_now
                                );
                                engine.bg_reclaim();
                                // bg_start で正しいキーで即再変換 → その場で待機 → 1回のSpace押しで候補取得
                                let llm_limit2 = crate::engine::state::get_num_candidates();
                                if engine.bg_start(llm_limit2) {
                                    tracing::debug!(
                                        "on_convert[llm_pending]: bg_start restarted for key={:?}, waiting inline",
                                        take_key
                                    );
                                    // ここで最大 1500ms 待つ（ユーザーは1回のSpaceで候補を得られる）
                                    const RESTART_WAIT_MS: u64 = 1500;
                                    engine.bg_wait_ms(RESTART_WAIT_MS);
                                    tracing::debug!(
                                        "on_convert[llm_pending]: inline wait done, bg={}",
                                        engine.bg_status()
                                    );
                                } else {
                                    tracing::error!(
                                        "on_convert[llm_pending]: bg_start also failed (kanji_ready={})",
                                        engine.is_kanji_ready()
                                    );
                                }
                                if let Some(llm_cands) = engine.bg_take_candidates(&take_key) {
                                    tracing::debug!(
                                        "on_convert[llm_pending]: reclaim+retry → Some({} cands)",
                                        llm_cands.len()
                                    );
                                    let merged = engine.merge_candidates(llm_cands, DICT_LIMIT);
                                    tracing::debug!("merge_candidates → {:?}", merged);
                                    if !merged.is_empty() {
                                        if let Ok(mut sess2) = session_get() {
                                            if let SessionState::Selecting {
                                                ref mut candidates,
                                                ref mut selected,
                                                ref mut llm_pending,
                                                ..
                                            } = *sess2
                                            {
                                                *candidates = merged;
                                                *selected = 0;
                                                *llm_pending = false;
                                            }
                                            let page_cands = sess2.page_candidates().to_vec();
                                            let page_info = sess2.page_info();
                                            let cand_text = sess2
                                                .current_candidate()
                                                .or_else(|| sess2.original_preedit())
                                                .unwrap_or("")
                                                .to_string();
                                            let prefix = sess2.selecting_prefix_clone();
                                            let remainder = sess2.selecting_remainder_clone();
                                            let pos = caret_rect_get();
                                            drop(sess2);
                                            drop(guard);
                                            candidate_window::show(
                                                &page_cands,
                                                0,
                                                &page_info,
                                                pos.left,
                                                pos.bottom,
                                            );
                                            update_composition_candidate_parts(
                                                ctx, tid, sink, prefix, cand_text, remainder,
                                            )?;
                                            return Ok(true);
                                        }
                                    }
                                } else {
                                    tracing::error!(
                                        "on_convert[llm_pending]: retry also failed, bg={}",
                                        engine.bg_status()
                                    );
                                }
                            }
                        }
                    } else {
                        // まだ変換中 → 現在の候補ウィンドウをそのまま維持
                        if let Ok(sess2) = session_get() {
                            let page_cands = sess2.page_candidates().to_vec();
                            let page_info = sess2.page_info();
                            let pos = caret_rect_get();
                            drop(sess2);
                            drop(guard);
                            candidate_window::show_with_status(
                                &page_cands,
                                0,
                                &page_info,
                                pos.left,
                                pos.bottom,
                                Some("⏳ 変換中..."),
                            );
                            return Ok(true);
                        }
                    }
                    return Ok(true);
                }

                sess.next_with_page_wrap();
                let page_cands = sess.page_candidates().to_vec();
                let page_sel = sess.page_selected();
                let page_info = sess.page_info();
                let cand_text = sess
                    .current_candidate()
                    .or_else(|| sess.original_preedit())
                    .unwrap_or("")
                    .to_string();
                let prefix = sess.selecting_prefix_clone();
                let remainder = sess.selecting_remainder_clone();
                drop(sess);
                drop(guard);
                candidate_window::update_selection(page_sel, &page_info);
                candidate_window::show(
                    &page_cands,
                    page_sel,
                    &page_info,
                    caret_rect_get().left,
                    caret_rect_get().bottom,
                );
                update_composition_candidate_parts(ctx, tid, sink, prefix, cand_text, remainder)?;
                return Ok(true);
            }
        }

        // 新規変換
        let llm_limit = crate::engine::state::get_num_candidates();
        const DICT_LIMIT: usize = 40;
        let _ = crate::engine::state::poll_dict_ready_cached(engine);
        let _ = crate::engine::state::poll_model_ready_cached(engine);
        // Done 状態の converter を先に回収する。
        // bg_take_candidates がキー不一致で None を返した場合、converter は Done に残ったまま
        // engine.kanji=None になる。is_kanji_ready() チェックより前に reclaim しないと
        // bg_start が永遠にスキップされ Waiting から抜け出せなくなる。
        engine.bg_reclaim();
        let kanji_ready = engine.is_kanji_ready();
        tracing::debug!(
            "on_convert[new]: preedit={:?} hira={:?} kanji_ready={} bg={}",
            preedit,
            engine.hiragana_text(),
            kanji_ready,
            engine.bg_status()
        );
        if kanji_ready && engine.bg_status() == "idle" {
            tracing::debug!("on_convert: model ready → bg_start");
            engine.bg_start(llm_limit);
        }
        if !kanji_ready {
            let err = engine.last_error();
            tracing::warn!("on_convert: kanji not ready, engine status={:?}", err);
        }

        let bg_status = engine.bg_status();
        let bg_running = !kanji_ready || bg_status == "running" || bg_status == "idle";
        tracing::debug!(
            "on_convert[new]: bg_running={} bg={}",
            bg_running,
            bg_status
        );

        // LLM が実行中の場合、**短時間だけ** 同期で完了を待ち、タイムアウトしたら
        // WM_TIMER ポーリング経路に委譲する。ここで長く待つと RAKUKAN_ENGINE と
        // RpcEngine の Connection ミューテックスが押さえっぱなしになり、
        // 続くキー入力のホットパス（try_lock）がすべて弾かれて「入力が止まる」
        // 症状になる。inline 完走はキャッシュヒット等の高速ケースに限定し、
        // 通常は ⏳ 表示 + WM_TIMER で非同期に解決する。
        const LLM_WAIT_INLINE_MS: u64 = 250;
        tracing::debug!("on_convert[new]: LLM_WAIT_INLINE_MS={LLM_WAIT_INLINE_MS}ms");
        if bg_running && kanji_ready {
            let caret = caret_rect_get();
            // まず ⏳ を即表示してユーザーに変換中であることを伝える
            if let Ok(mut sess) = session_get() {
                if !sess.is_waiting() {
                    sess.set_waiting(preedit.clone(), caret.left, caret.bottom);
                }
            }
            let dummy = vec![preedit.clone()];
            // drop guard の前に ⏳ 表示（RequestEditSession 前）
            candidate_window::show_with_status(
                &dummy,
                0,
                "",
                caret.left,
                caret.bottom,
                Some("⏳ 変換中..."),
            );
            // LLM完了を待機（短時間のみ）。超えたら WM_TIMER に任せて抜ける。
            let completed = engine.bg_wait_ms(LLM_WAIT_INLINE_MS);
            tracing::debug!("on_convert[new]: bg_wait({LLM_WAIT_INLINE_MS}ms) completed={completed}");
            if !completed {
                drop(guard);
                candidate_window::start_waiting_timer();
                return Ok(true);
            }
        } else if bg_running {
            // kanji_ready=false だが bg=running の場合：
            // 前の変換の converter がまだ conv_cache に貸し出されている。
            // 完了を待って reclaim し、新しいキーで bg_start を再試行する。
            let caret = caret_rect_get();
            if let Ok(mut sess) = session_get() {
                if !sess.is_waiting() {
                    sess.set_waiting(preedit.clone(), caret.left, caret.bottom);
                }
            }
            let dummy = vec![preedit.clone()];
            candidate_window::show_with_status(
                &dummy,
                0,
                "",
                caret.left,
                caret.bottom,
                Some("⏳ 変換中..."),
            );
            tracing::debug!(
                "on_convert[new]: kanji_ready=false bg=running → wait for prev bg to finish"
            );
            let completed = engine.bg_wait_ms(LLM_WAIT_INLINE_MS);
            tracing::debug!("on_convert[new]: prev bg wait completed={completed}");
            if !completed {
                // 前の bg が inline 時間で終わらない → WM_TIMER に任せる
                drop(guard);
                candidate_window::start_waiting_timer();
                return Ok(true);
            }
            // 前の bg が完了したら converter を回収して新しいキーで再起動
            engine.bg_reclaim();
            let kanji_ready2 = engine.is_kanji_ready();
            tracing::debug!("on_convert[new]: after reclaim kanji_ready={kanji_ready2}");
            if kanji_ready2 {
                engine.bg_start(llm_limit);
                let completed2 = engine.bg_wait_ms(LLM_WAIT_INLINE_MS);
                tracing::debug!("on_convert[new]: new bg wait completed={completed2}");
                if !completed2 {
                    drop(guard);
                    candidate_window::start_waiting_timer();
                    return Ok(true);
                }
                // kanji_ready を更新して後続の候補取得処理へ続行
            } else {
                // モデル自体が未ロード → タイマーに任せる
                drop(guard);
                candidate_window::start_waiting_timer();
                return Ok(true);
            }
        }

        // bg 完了（または idle/stopped）→ 候補を取得して表示
        // bg_start のキーは hiragana_buf。preedit は preedit_display()（pending_romaji含む）で
        // 不一致になる場合があるため、hiragana_text() を優先キーとして使う。
        let bg_status2 = engine.bg_status();
        let hiragana_key2 = engine.hiragana_text().to_string();
        // kanji_ready は最新の状態に更新（前 bg の reclaim 後に変化している場合がある）
        let kanji_ready_now = engine.is_kanji_ready();
        tracing::debug!(
            "on_convert[new]: post-wait hiragana_key={:?} bg={} kanji_ready={}",
            hiragana_key2,
            bg_status2,
            kanji_ready_now
        );
        // キー不一致で None が返ると Done が復元されるので、両方試した後に reclaim しておく
        let bg_cands = engine.bg_take_candidates(&hiragana_key2).or_else(|| {
            if preedit != hiragana_key2 {
                tracing::debug!("Convert: hira key miss, retry preedit={:?}", preedit);
                engine.bg_take_candidates(&preedit)
            } else {
                None
            }
        });
        tracing::debug!(
            "on_convert[new]: bg_cands={:?}",
            bg_cands.as_ref().map(|c| c.len())
        );
        // いずれも None だった場合 → bg_reclaim + bg_start で inline 再試行。
        // 短時間で取れなければ WM_TIMER fallback に委譲して抜ける。
        let bg_cands = if bg_cands.is_none() && kanji_ready_now {
            tracing::warn!(
                "Convert: bg_take_candidates None (hira={:?} preedit={:?}) → reclaim+restart",
                hiragana_key2,
                preedit
            );
            engine.bg_reclaim();
            if engine.is_kanji_ready() {
                engine.bg_start(llm_limit);
                let completed3 = engine.bg_wait_ms(LLM_WAIT_INLINE_MS);
                tracing::debug!("Convert: retry bg_wait completed={completed3}");
                if !completed3 {
                    drop(guard);
                    candidate_window::start_waiting_timer();
                    return Ok(true);
                }
                let hira3 = engine.hiragana_text().to_string();
                engine
                    .bg_take_candidates(&hira3)
                    .or_else(|| {
                        if preedit != hira3 {
                            engine.bg_take_candidates(&preedit)
                        } else {
                            None
                        }
                    })
                    .inspect(|c| tracing::debug!("Convert: retry got {} cands", c.len()))
            } else {
                engine.bg_reclaim();
                None
            }
        } else {
            bg_cands
        };
        // それでも None なら reclaim だけしておく
        if bg_cands.is_none() {
            engine.bg_reclaim();
        }

        let (candidates, llm_pending): (Vec<String>, bool) = match bg_cands {
            Some(llm_cands) if !llm_cands.is_empty() => {
                // bg_take_candidates 成功時に kanji が復元されているため再評価
                let kanji_ready_now = engine.is_kanji_ready();
                let merged = engine.merge_candidates(llm_cands, DICT_LIMIT);
                tracing::debug!(
                    "merge_candidates(kanji_ready={}) → {:?} [dict: {:?}]",
                    kanji_ready_now,
                    merged,
                    engine.dict_status()
                );
                if merged.is_empty() || (merged.len() == 1 && merged[0] == preedit) {
                    if kanji_ready_now {
                        (
                            engine_convert_sync_multi(engine, llm_limit, DICT_LIMIT, &preedit),
                            false,
                        )
                    } else {
                        (vec![preedit.clone()], false)
                    }
                } else {
                    (merged, false)
                }
            }
            _ => {
                if kanji_ready_now {
                    let dict_cands =
                        engine_convert_sync_multi(engine, llm_limit, DICT_LIMIT, &preedit);
                    if dict_cands.is_empty() {
                        (vec![preedit.clone()], false)
                    } else {
                        (dict_cands, false)
                    }
                } else {
                    (vec![preedit.clone()], false)
                }
            }
        };
        // Waiting 状態を解除
        if let Ok(mut sess) = session_get() {
            if sess.is_waiting() {
                sess.set_preedit(preedit.clone());
            }
        }
        candidate_window::stop_waiting_timer();
        let _ = bg_status2; // suppress unused warning

        let first = candidates
            .first()
            .cloned()
            .unwrap_or_else(|| preedit.clone());
        diag::event(DiagEvent::Convert {
            preedit: preedit.clone(),
            kanji_ready: true,
            result: first.clone(),
        });

        let caret = caret_rect_get();
        drop(guard);
        let (page_cands, page_info) = {
            let mut sess = session_get()?;
            sess.activate_selecting(
                candidates.clone(),
                preedit.clone(),
                caret.left,
                caret.bottom,
                llm_pending,
            );
            (sess.page_candidates().to_vec(), sess.page_info())
        };
        let status = if llm_pending {
            Some("⏳ 変換中...")
        } else {
            None
        };
        candidate_window::show_with_status(
            &page_cands,
            0,
            &page_info,
            caret.left,
            caret.bottom,
            status,
        );
        tracing::debug!(
            "on_convert[new]: update_composition first={:?} comp_exists={}",
            first,
            composition_clone().map(|g| g.is_some()).unwrap_or(false)
        );
        update_composition(ctx, tid, sink, first)?;
        Ok(true)
    }

    fn on_commit_raw(
        &self,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        crate::engine::state::maybe_log_gpu_memory(engine);
        {
            let mut sess = session_get()?;
            // ── LiveConv（ライブ変換プレビュー表示中）: Enter → preview をコミット ──
            if sess.is_live_conv() {
                let (reading, preview) = sess
                    .live_conv_parts()
                    .map(|(r, p)| (r.to_string(), p.to_string()))
                    .unwrap_or_default();
                if preview.is_empty() {
                    return Ok(false);
                }
                sess.set_idle();
                drop(sess);
                candidate_window::hide();
                candidate_window::stop_live_timer();
                if preview != reading && crate::engine::state::is_auto_learn_enabled() {
                    engine.learn(&reading, &preview);
                }
                engine.commit(&preview);
                engine.reset_preedit();
                drop(guard);
                tracing::info!("[Live] on_commit_raw[LiveConv]: commit {:?}", preview);
                diag::event(DiagEvent::CommitRaw {
                    preedit: preview.clone(),
                });
                end_composition(ctx, tid, preview)?;
                return Ok(true);
            }
            // ── RangeSelect: 選択範囲をひらがなのまま確定、残りで LiveConv 再開 ──
            if sess.is_range_select() {
                let (selected, unselected) = sess.range_select_parts().unwrap_or_default();
                if selected.is_empty() {
                    return Ok(false);
                }
                if unselected.is_empty() {
                    // 全体選択 → 全部確定
                    sess.set_idle();
                    drop(sess);
                    candidate_window::hide();
                    engine.commit(&selected);
                    engine.reset_preedit();
                    drop(guard);
                    end_composition(ctx, tid, selected)?;
                    return Ok(true);
                }
                // 部分確定 → 残りで LiveConv 再開
                sess.set_idle();
                drop(sess);
                candidate_window::hide();
                engine.commit(&selected);
                engine.reset_preedit();
                // 残りを engine に設定して LiveConv 再開
                for c in unselected.chars() {
                    engine.push_raw(c);
                }
                let live_beam = crate::engine::state::get_live_conv_beam_size();
                engine.bg_start(live_beam);
                let preedit = engine.preedit_display();
                {
                    let mut sess = session_get()?;
                    sess.set_preedit(unselected.clone());
                }
                drop(guard);
                commit_then_start_composition(ctx, tid, sink, selected, preedit)?;
                return Ok(true);
            }
            // ── Waiting（⏳変換中）: ひらがなのままコミット ──
            if sess.is_waiting() {
                let text = sess.preedit_text().unwrap_or("").to_string();
                sess.set_idle();
                drop(sess);
                candidate_window::hide();
                engine.bg_reclaim();
                engine.commit(&text);
                engine.reset_preedit();
                drop(guard);
                tracing::info!("on_commit_raw[Waiting]: commit {:?}", text);
                end_composition(ctx, tid, text)?;
                return Ok(true);
            }
            // ── Selecting ──
            if sess.is_selecting() {
                let text = sess
                    .current_candidate()
                    .or_else(|| sess.original_preedit())
                    .unwrap_or("")
                    .to_string();
                let reading = sess.original_preedit().unwrap_or("").to_string();
                let punct = sess.take_punct_pending();
                let prefix = sess.selecting_prefix_clone();
                let remainder = sess.take_selecting_remainder();
                let remainder_reading = sess.selecting_remainder_reading_clone();
                sess.set_idle();
                drop(sess);
                let commit_text = if let Some(p) = punct {
                    format!("{text}{p}")
                } else {
                    text.clone()
                };
                if text != reading && crate::engine::state::is_auto_learn_enabled() {
                    engine.learn(&reading, &text);
                }
                candidate_window::hide();
                candidate_window::stop_live_timer();
                let confirmed = format!("{prefix}{commit_text}");
                if !remainder_reading.is_empty() {
                    // remainder がある → 確定部分を commit し、残りで LiveConv 再開
                    engine.commit(&confirmed);
                    engine.reset_preedit();
                    for c in remainder_reading.chars() {
                        engine.push_raw(c);
                    }
                    let live_beam = crate::engine::state::get_live_conv_beam_size();
                    engine.bg_start(live_beam);
                    let preedit = engine.preedit_display();
                    {
                        let mut sess = session_get()?;
                        sess.set_preedit(remainder_reading.clone());
                    }
                    drop(guard);
                    commit_then_start_composition(ctx, tid, sink, confirmed, preedit)?;
                } else {
                    let full_text = format!("{confirmed}{remainder}");
                    engine.commit(&full_text);
                    engine.reset_preedit();
                    drop(guard);
                    diag::event(DiagEvent::CommitRaw {
                        preedit: full_text.clone(),
                    });
                    end_composition(ctx, tid, full_text)?;
                }
                return Ok(true);
            }
        }
        engine.flush_pending_n();
        if crate::engine::state::SUPPRESS_LIVE_COMMIT_ONCE
            .swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            tracing::debug!("[Live] on_commit_raw[fallback]: suppressed once");
        } else if crate::engine::config::current_config()
            .live_conversion
            .enabled
        {
            const LIVE_COMMIT_WAIT_MS: u64 = 180;
            let reading = engine.hiragana_text().to_string();
            if !reading.is_empty() {
                let n_cands = crate::engine::state::get_num_candidates();
                let bg_before = engine.bg_status();
                tracing::debug!(
                    "[Live] on_commit_raw[fallback]: reading={:?} bg_before={}",
                    reading,
                    bg_before
                );
                if engine.is_kanji_ready() && bg_before == "idle" {
                    let _ = engine.bg_start(n_cands);
                }
                if matches!(engine.bg_status(), "running" | "idle") {
                    let completed = engine.bg_wait_ms(LIVE_COMMIT_WAIT_MS);
                    tracing::debug!(
                        "[Live] on_commit_raw[fallback]: bg_wait({LIVE_COMMIT_WAIT_MS}ms) completed={}",
                        completed
                    );
                }
                if engine.bg_status() == "done" {
                    if let Some(preview) = engine
                        .bg_take_candidates(&reading)
                        .and_then(|c| c.into_iter().next())
                    {
                        if !preview.is_empty() && preview != reading {
                            if let Ok(mut sess) = session_get() {
                                sess.set_idle();
                            }
                            candidate_window::hide();
                            candidate_window::stop_live_timer();
                            if crate::engine::state::is_auto_learn_enabled() {
                                engine.learn(&reading, &preview);
                            }
                            engine.commit(&preview);
                            engine.reset_preedit();
                            drop(guard);
                            tracing::info!(
                                "[Live] on_commit_raw[fallback]: commit preview {:?}",
                                preview
                            );
                            diag::event(DiagEvent::CommitRaw {
                                preedit: preview.clone(),
                            });
                            end_composition(ctx, tid, preview)?;
                            return Ok(true);
                        }
                    }
                }
            }
        }
        let preedit = engine.preedit_display();
        if preedit.is_empty() {
            return Ok(false);
        }
        diag::event(DiagEvent::CommitRaw {
            preedit: preedit.clone(),
        });
        engine.bg_reclaim();
        engine.commit(&preedit.clone());
        engine.reset_preedit();
        drop(guard);
        end_composition(ctx, tid, preedit)?;
        Ok(true)
    }

    fn on_backspace(
        &self,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        // M1.8 T-MID1: reading が短くなるので gen を前進させる。
        crate::engine::state::live_conv_gen_bump();
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        {
            let mut sess = session_get()?;
            // LiveConv → Backspace → ひらがな表示に戻す（1文字削除はエンジンが行う）
            if sess.is_live_conv() {
                let reading = sess
                    .live_conv_parts()
                    .map(|(r, _)| r.to_string())
                    .unwrap_or_default();
                sess.set_preedit(reading.clone());
                drop(sess);
                candidate_window::stop_live_timer();
                use crate::engine::state::{LIVE_PREVIEW_QUEUE, LIVE_PREVIEW_READY};
                LIVE_PREVIEW_READY.store(false, std::sync::atomic::Ordering::Release);
                if let Ok(mut q) = LIVE_PREVIEW_QUEUE.try_lock() {
                    *q = None;
                }
                // ひらがな表示に戻してから通常の backspace 処理へフォールスルー
                drop(guard);
                update_composition(ctx.clone(), tid, sink.clone(), reading)?;
                guard = engine_try_get_or_create()?;
                let engine2 = match guard.as_mut() {
                    Some(e) => e,
                    None => return Ok(true),
                };
                let consumed = engine2.backspace();
                if consumed {
                    engine2.bg_reclaim();
                    let preedit = engine2.preedit_display();
                    drop(guard);
                    if preedit.is_empty() {
                        end_composition(ctx, tid, String::new())?;
                    } else {
                        update_composition(ctx, tid, sink, preedit)?;
                    }
                }
                return Ok(consumed);
            }
            // RangeSelect → Backspace → LiveConv に戻る
            if sess.is_range_select() {
                if let SessionState::RangeSelect {
                    full_reading,
                    original_preview,
                    ..
                } = &*sess
                {
                    let reading = full_reading.clone();
                    let preview = original_preview.clone();
                    sess.set_live_conv(reading, preview.clone());
                    drop(sess);
                    candidate_window::hide();
                    drop(guard);
                    update_composition(ctx, tid, sink, preview)?;
                    return Ok(true);
                }
            }
            if sess.is_selecting() {
                let original = sess.original_preedit().unwrap_or("").to_string();
                sess.set_preedit(original.clone());
                drop(sess);
                candidate_window::hide();
                drop(guard);
                update_composition(ctx, tid, sink, original)?;
                return Ok(true);
            }
            if sess.is_waiting() {
                let pre = sess.preedit_text().unwrap_or("").to_string();
                sess.set_preedit(pre);
                candidate_window::hide();
            }
        }
        let consumed = engine.backspace();
        if consumed {
            engine.bg_reclaim();
            let preedit = engine.preedit_display();
            diag::event(DiagEvent::Backspace {
                preedit_after: preedit.clone(),
            });
            drop(guard);
            if preedit.is_empty() {
                end_composition(ctx, tid, String::new())?;
            } else {
                update_composition(ctx, tid, sink, preedit)?;
            }
        }
        Ok(consumed)
    }

    fn on_cancel(
        &self,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        {
            let mut sess = session_get()?;
            // LiveConv → ESC → ひらがな表示に戻す（変換はキャンセル）
            if sess.is_live_conv() {
                let reading = sess
                    .live_conv_parts()
                    .map(|(r, _)| r.to_string())
                    .unwrap_or_default();
                tracing::debug!("[Live] on_cancel[LiveConv]: restore reading={:?}", reading);
                sess.set_preedit(reading.clone());
                drop(sess);
                candidate_window::stop_live_timer();
                use crate::engine::state::{LIVE_PREVIEW_QUEUE, LIVE_PREVIEW_READY};
                LIVE_PREVIEW_READY.store(false, std::sync::atomic::Ordering::Release);
                if let Ok(mut q) = LIVE_PREVIEW_QUEUE.try_lock() {
                    *q = None;
                }
                drop(guard);
                update_composition(ctx, tid, sink, reading)?;
                return Ok(true);
            }
            // RangeSelect → ESC → LiveConv に戻る（元の preview を復元）
            if sess.is_range_select() {
                if let SessionState::RangeSelect {
                    full_reading,
                    original_preview,
                    ..
                } = &*sess
                {
                    let reading = full_reading.clone();
                    let preview = original_preview.clone();
                    sess.set_live_conv(reading, preview.clone());
                    drop(sess);
                    candidate_window::hide();
                    drop(guard);
                    update_composition(ctx, tid, sink, preview)?;
                    return Ok(true);
                }
            }
            if sess.is_selecting() {
                // 変換中 → ESC → 未変換状態へ戻す（2回目のESCでプリエディット全消去）
                // 文節分割後の変換の場合は remainder も復元して full に戻す
                let original = sess.original_preedit().unwrap_or("").to_string();
                let prefix = sess.selecting_prefix_clone();
                let remainder = sess.selecting_remainder_clone();
                let full = format!("{prefix}{original}{remainder}");
                tracing::debug!(
                    "on_cancel[Selecting]: prefix={:?} original={:?} remainder={:?} → full={:?}",
                    prefix,
                    original,
                    remainder,
                    full
                );
                sess.set_preedit(full.clone());
                drop(sess);
                candidate_window::hide();
                engine.bg_reclaim();
                // engine の hiragana_buf を full に復元（force_preedit(target) で縮んでいるため）
                engine.force_preedit(full.clone());
                drop(guard);
                update_composition(ctx, tid, sink, full)?;
                return Ok(true);
            }
            if sess.is_waiting() {
                let pre = sess.preedit_text().unwrap_or("").to_string();
                let bg = engine.bg_status();
                tracing::debug!("on_cancel[Waiting]: pre={:?} bg={}", pre, bg);
                if pre.is_empty() {
                    // text が空の場合は Idle にしてプリエディットをクリア
                    tracing::warn!("on_cancel[Waiting]: pre is empty → end_composition");
                    sess.set_idle();
                    drop(sess);
                    engine.bg_reclaim();
                    engine.reset_all();
                    drop(guard);
                    end_composition(ctx, tid, String::new())?;
                    return Ok(true);
                }
                sess.set_preedit(pre.clone());
                candidate_window::hide();
                candidate_window::stop_waiting_timer();
                // BG変換（Done状態）は保持 → 次のSpace押下で候補取得可能
                drop(sess);
                drop(guard);
                update_composition(ctx, tid, sink, pre)?;
                return Ok(true);
            }
        }
        // 未変換状態 → ESC → プリエディット全消去
        {
            let bg = engine.bg_status();
            let hira = engine.hiragana_text().to_string();
            tracing::debug!(
                "on_cancel[fallthrough]: preedit_empty={} bg={} hira={:?}",
                engine.preedit_is_empty(),
                bg,
                hira
            );
        }
        if engine.preedit_is_empty() {
            return Ok(false);
        }
        engine.bg_reclaim();
        engine.reset_all();
        drop(guard);
        end_composition(ctx, tid, String::new())?;
        Ok(true)
    }

    fn on_kana_convert(
        &self,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
        convert_fn: fn(&str) -> String,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        engine.flush_pending_n();
        let p = engine.preedit_display();
        if p.is_empty() {
            return Ok(false);
        }
        engine.bg_reclaim();

        // F9/F10 で全角/半角ラテン文字に変換済みの場合、
        // hiragana_buf はラテン文字のみになっている。
        // romaji_input_log からひらがなを復元してから変換する。
        let has_kana = p.chars().any(|c| {
            let n = c as u32;
            (0x3041..=0x3096).contains(&n)   // ひらがな
            || (0x30A1..=0x30FC).contains(&n) // カタカナ
            || (0xFF65..=0xFF9F).contains(&n) // 半角カタカナ
        });
        let source = if !has_kana {
            // ラテン文字のみ → romaji_log からひらがなを復元
            let hira = engine.hiragana_from_romaji_log();
            if hira.is_empty() {
                p.clone()
            } else {
                hira
            }
        } else {
            p.clone()
        };
        let t = convert_fn(&source);
        engine.force_preedit(t.clone());
        crate::engine::state::SUPPRESS_LIVE_COMMIT_ONCE
            .store(true, std::sync::atomic::Ordering::Release);
        if let Ok(mut sess) = session_get() {
            if sess.is_selecting() || sess.is_live_conv() {
                sess.set_preedit(t.clone());
                candidate_window::hide();
                candidate_window::stop_live_timer();
            } else if sess.is_waiting() {
                sess.set_preedit(t.clone());
                candidate_window::hide();
                candidate_window::stop_waiting_timer();
            }
        }
        drop(guard);
        update_composition(ctx, tid, sink, t)?;
        Ok(true)
    }

    /// F9（全角英数）/ F10（半角英数）変換。
    ///
    /// - 初回: romaji_input_log を使ってかな→ローマ字に変換し、全角/半角小文字にする
    /// - 2回目以降: 現在の文字列のサイクル状態から次状態へ進む
    ///   F9サイクル: 全角小→全角大→全角先頭大→全角小→…
    ///   F10サイクル: 半角小→半角大→半角先頭大→半角小→…
    /// - F6を押すとひらがな（romaji_log から force_preedit で元のかなに戻す）
    fn on_latin_convert(
        &self,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
        full: bool, // true=F9全角, false=F10半角
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        engine.flush_pending_n();
        let p = engine.preedit_display();
        if p.is_empty() {
            return Ok(false);
        }
        engine.bg_reclaim();

        // ひらがな/カタカナを含む場合は初回変換（ローマ字ログをFFI経由で取得）
        // 既にラテン文字のみの場合はサイクル継続
        // プリエディットにひらがな/カタカナが含まれる場合は初回変換
        // ラテン文字のみの場合はサイクル継続
        let has_kana = p.chars().any(|c| {
            let n = c as u32;
            (0x3041..=0x3096).contains(&n)   // ひらがな
            || (0x30A1..=0x30FC).contains(&n) // カタカナ
            || (0xFF65..=0xFF9F).contains(&n) // 半角カタカナ
        });
        let t = if has_kana {
            // かな → romaji_log_str でローマ字を復元して変換
            let hira = engine.hiragana_from_romaji_log();
            let pending_suffix = p
                .strip_prefix(&hira)
                .map(str::to_string)
                .unwrap_or_default();
            let romaji = format!("{}{}", engine.romaji_log_str(), pending_suffix);
            if full {
                text_util::romaji_to_fullwidth_latin(&romaji)
            } else {
                text_util::romaji_to_halfwidth_latin(&romaji)
            }
        } else {
            // すでにラテン文字 → サイクル
            if full {
                text_util::to_full_latin(&p)
            } else {
                text_util::to_half_latin(&p)
            }
        };
        engine.force_preedit(t.clone());
        crate::engine::state::SUPPRESS_LIVE_COMMIT_ONCE
            .store(true, std::sync::atomic::Ordering::Release);
        if let Ok(mut sess) = session_get() {
            if sess.is_selecting() || sess.is_live_conv() {
                sess.set_preedit(t.clone());
                candidate_window::hide();
                candidate_window::stop_live_timer();
            } else if sess.is_waiting() {
                sess.set_preedit(t.clone());
                candidate_window::hide();
                candidate_window::stop_waiting_timer();
            }
        }
        drop(guard);
        update_composition(ctx, tid, sink, t)?;
        Ok(true)
    }

    fn on_cycle_kana(
        &self,
        ctx: ITfContext,
        tid: u32,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        let p = engine.preedit_display();
        if p.is_empty() {
            return Ok(false);
        }
        engine.bg_reclaim();
        let t = text_util::to_katakana(&p);
        engine.commit(&t);
        engine.reset_preedit();
        drop(guard);
        end_composition(ctx, tid, t)?;
        Ok(true)
    }

    fn on_candidate_move(
        &self,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        guard: crate::engine::state::EngineGuard,
        dir: CandidateDir,
    ) -> Result<bool> {
        let has_pre = guard
            .as_ref()
            .map(|e| !e.preedit_is_empty())
            .unwrap_or(false);
        drop(guard);
        let mut sess = session_get()?;
        if !sess.is_candidate_list_active() {
            return Ok(has_pre);
        }
        match dir {
            CandidateDir::Next => sess.next_with_page_wrap(),
            CandidateDir::Prev => sess.prev(),
        }
        let page_cands = sess.page_candidates();
        let page_sel = sess.page_selected();
        let page_info = sess.page_info();
        let text = sess
            .current_candidate()
            .or_else(|| sess.original_preedit())
            .unwrap_or("")
            .to_string();
        let prefix = sess.selecting_prefix_clone();
        let remainder = sess.selecting_remainder_clone();
        drop(sess);
        candidate_window::update_selection(page_sel, &page_info);
        candidate_window::show(
            &page_cands,
            page_sel,
            &page_info,
            caret_rect_get().left,
            caret_rect_get().bottom,
        );
        update_composition_candidate_parts(ctx, tid, sink, prefix, text, remainder)?;
        Ok(true)
    }

    fn on_candidate_page(
        &self,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        guard: crate::engine::state::EngineGuard,
        dir: CandidateDir,
    ) -> Result<bool> {
        let has_pre = guard
            .as_ref()
            .map(|e| !e.preedit_is_empty())
            .unwrap_or(false);
        drop(guard);
        let mut sess = session_get()?;
        if !sess.is_candidate_list_active() {
            return Ok(has_pre);
        }
        match dir {
            CandidateDir::Next => sess.next_page(),
            CandidateDir::Prev => sess.prev_page(),
        }
        let page_cands = sess.page_candidates();
        let page_sel = sess.page_selected();
        let page_info = sess.page_info();
        let text = sess
            .current_candidate()
            .or_else(|| sess.original_preedit())
            .unwrap_or("")
            .to_string();
        let prefix = sess.selecting_prefix_clone();
        let remainder = sess.selecting_remainder_clone();
        drop(sess);
        let caret = caret_rect_get();
        candidate_window::show(&page_cands, page_sel, &page_info, caret.left, caret.bottom);
        update_composition_candidate_parts(ctx, tid, sink, prefix, text, remainder)?;
        Ok(true)
    }

    fn on_candidate_select(
        &self,
        n: u8,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        let has_pre = !engine.preedit_is_empty();
        let mut sess = session_get()?;
        if !sess.is_candidate_list_active() {
            return Ok(has_pre);
        }
        if !sess.select_nth_in_page(n as usize) {
            return Ok(true);
        }
        let text = sess
            .current_candidate()
            .or_else(|| sess.original_preedit())
            .unwrap_or("")
            .to_string();
        let reading = sess.original_preedit().unwrap_or("").to_string();
        let prefix = sess.selecting_prefix_clone();
        let punct = sess.take_punct_pending();
        let remainder = sess.take_selecting_remainder();
        let remainder_reading = sess.selecting_remainder_reading_clone();
        sess.set_idle();
        drop(sess);
        let commit_text = if let Some(p) = punct {
            format!("{text}{p}")
        } else {
            text.clone()
        };
        if text != reading && crate::engine::state::is_auto_learn_enabled() {
            engine.learn(&reading, &text);
        }
        candidate_window::hide();
        let confirmed = format!("{prefix}{commit_text}");
        if !remainder_reading.is_empty() {
            // remainder がある → 確定部分を commit し、残りで LiveConv 再開
            engine.commit(&confirmed);
            engine.reset_preedit();
            for c in remainder_reading.chars() {
                engine.push_raw(c);
            }
            let live_beam = crate::engine::state::get_live_conv_beam_size();
            engine.bg_start(live_beam);
            let preedit = engine.preedit_display();
            {
                let mut sess = session_get()?;
                sess.set_preedit(remainder_reading.clone());
            }
            drop(guard);
            commit_then_start_composition(ctx, tid, sink, confirmed, preedit)?;
        } else {
            let full_text = format!("{confirmed}{remainder}");
            diag::event(DiagEvent::Convert {
                preedit: text.clone(),
                kanji_ready: true,
                result: full_text.clone(),
            });
            engine.commit(&full_text);
            engine.reset_preedit();
            drop(guard);
            end_composition(ctx, tid, full_text)?;
        }
        Ok(true)
    }

    fn on_ime_toggle(&self, ctx: ITfContext, tid: u32) -> Result<bool> {
        {
            let mut guard = engine_try_get_or_create()?;
            if let Some(engine) = guard.as_mut() {
                // LiveConv 中は preview をコミットしてから IME を切り替える
                let commit_text = {
                    let sess = session_get();
                    if let Ok(s) = &sess {
                        if s.is_live_conv() {
                            s.live_conv_parts().map(|(_, p)| p.to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                let preedit = commit_text.unwrap_or_else(|| engine.preedit_display());
                if !preedit.is_empty() {
                    engine.bg_reclaim();
                    engine.commit(&preedit.clone());
                    engine.reset_preedit();
                    drop(guard);
                    if let Ok(mut sess) = session_get() {
                        sess.set_idle();
                    }
                    candidate_window::stop_live_timer();
                    end_composition(ctx.clone(), tid, preedit)?;
                }
            }
        }
        let (from, to, now_open) = if let Ok(mut st) = crate::engine::state::ime_state_get() {
            use crate::engine::input_mode::InputMode;
            let was_alpha = st.input_mode == InputMode::Alphanumeric;
            let new_mode = if was_alpha {
                InputMode::Hiragana
            } else {
                InputMode::Alphanumeric
            };
            let from = format!("{:?}", st.input_mode);
            st.set_mode(new_mode);
            (
                from,
                if was_alpha {
                    "Hiragana"
                } else {
                    "Alphanumeric"
                },
                was_alpha,
            )
        } else {
            ("unknown".into(), "unknown", true)
        };
        if let Ok(inner) = self.inner.try_borrow() {
            if let Some(tm) = &inner.thread_mgr {
                if let Err(e) = unsafe { language_bar::set_open_close(tm, tid, now_open) } {
                    tracing::warn!("ImeToggle: set_open_close({}) failed: {e}", now_open);
                    diag::event(DiagEvent::Error {
                        site: "set_open_close/toggle",
                        msg: e.to_string(),
                    });
                }
            }
        }
        diag::event(DiagEvent::ModeChange { from, to });
        self.notify_langbar_update();
        self.notify_tray_update(tid);
        self.show_mode_indicator(to, ctx, tid);
        self.maybe_reload_runtime_config();
        Ok(true)
    }

    fn on_ime_off(&self, ctx: ITfContext, tid: u32) -> Result<bool> {
        {
            let mut guard = engine_try_get_or_create()?;
            if let Some(engine) = guard.as_mut() {
                // LiveConv 中は preview をコミットしてから IME をオフにする
                let commit_text = {
                    let sess = session_get();
                    if let Ok(s) = &sess {
                        if s.is_live_conv() {
                            s.live_conv_parts().map(|(_, p)| p.to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                };
                let preedit = commit_text.unwrap_or_else(|| engine.preedit_display());
                if !preedit.is_empty() {
                    engine.bg_reclaim();
                    engine.commit(&preedit.clone());
                    engine.reset_preedit();
                    drop(guard);
                    if let Ok(mut sess) = session_get() {
                        sess.set_idle();
                    }
                    candidate_window::stop_live_timer();
                    end_composition(ctx.clone(), tid, preedit)?;
                }
            }
        }
        if let Ok(mut st) = crate::engine::state::ime_state_get() {
            let from = format!("{:?}", st.input_mode);
            st.set_mode(crate::engine::input_mode::InputMode::Alphanumeric);
            diag::event(DiagEvent::ModeChange {
                from,
                to: "Alphanumeric",
            });
        }
        if let Ok(inner) = self.inner.try_borrow() {
            if let Some(tm) = &inner.thread_mgr {
                if let Err(e) = unsafe { language_bar::set_open_close(tm, tid, false) } {
                    tracing::warn!("ImeOff: set_open_close(false) failed: {e}");
                    diag::event(DiagEvent::Error {
                        site: "set_open_close/off",
                        msg: e.to_string(),
                    });
                }
            }
        }
        self.notify_langbar_update();
        self.notify_tray_update(tid);
        self.show_mode_indicator("Alphanumeric", ctx, tid);
        self.maybe_reload_runtime_config();
        Ok(true)
    }

    fn on_ime_on(&self, ctx: ITfContext, tid: u32) -> Result<bool> {
        if let Ok(mut st) = crate::engine::state::ime_state_get() {
            let from = format!("{:?}", st.input_mode);
            st.set_mode(crate::engine::input_mode::InputMode::Hiragana);
            diag::event(DiagEvent::ModeChange {
                from,
                to: "Hiragana",
            });
        }
        if let Ok(inner) = self.inner.try_borrow() {
            if let Some(tm) = &inner.thread_mgr {
                if let Err(e) = unsafe { language_bar::set_open_close(tm, tid, true) } {
                    tracing::warn!("ImeOn: set_open_close(true) failed: {e}");
                    diag::event(DiagEvent::Error {
                        site: "set_open_close/on",
                        msg: e.to_string(),
                    });
                }
            }
        }
        self.notify_langbar_update();
        self.notify_tray_update(tid);
        self.show_mode_indicator("Hiragana", ctx, tid);
        self.maybe_reload_runtime_config();
        Ok(true)
    }

    fn on_mode_hiragana(
        &self,
        ctx: ITfContext,
        tid: u32,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        if let Some(engine) = guard.as_mut() {
            let preedit = engine.preedit_display();
            if !preedit.is_empty() {
                let t = preedit.clone();
                engine.bg_reclaim();
                engine.commit(&t);
                engine.reset_preedit();
                drop(guard);
                end_composition(ctx.clone(), tid, t)?;
            } else {
                drop(guard);
            }
        }
        if let Ok(mut st) = crate::engine::state::ime_state_get() {
            let from = format!("{:?}", st.input_mode);
            st.set_mode(crate::engine::input_mode::InputMode::Hiragana);
            diag::event(DiagEvent::ModeChange {
                from,
                to: "Hiragana",
            });
        }
        self.notify_langbar_update();
        self.notify_tray_update(tid);
        self.show_mode_indicator("Hiragana", ctx, tid);
        self.maybe_reload_runtime_config();
        Ok(true)
    }

    fn on_mode_katakana(
        &self,
        ctx: ITfContext,
        tid: u32,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        if let Some(engine) = guard.as_mut() {
            let preedit = engine.preedit_display();
            if !preedit.is_empty() {
                let t = text_util::to_katakana(&preedit);
                engine.bg_reclaim();
                engine.commit(&t);
                engine.reset_preedit();
                drop(guard);
                end_composition(ctx.clone(), tid, t)?;
            } else {
                drop(guard);
            }
        }
        if let Ok(mut st) = crate::engine::state::ime_state_get() {
            let from = format!("{:?}", st.input_mode);
            st.set_mode(crate::engine::input_mode::InputMode::Katakana);
            diag::event(DiagEvent::ModeChange {
                from,
                to: "Katakana",
            });
        }
        self.notify_langbar_update();
        self.notify_tray_update(tid);
        self.show_mode_indicator("Katakana", ctx, tid);
        self.maybe_reload_runtime_config();
        Ok(true)
    }

    /// 句読点入力:
    ///   - プリエディットがあれば変換ウィンドウを表示し punct_pending にセット
    ///   - プリエディットが空なら直接コミット
    fn on_punctuate(
        &self,
        c: char,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };

        // プリエディットが空 → 直接コミット
        if engine.preedit_is_empty() {
            drop(guard);
            commit_text(ctx, tid, c.to_string())?;
            return Ok(true);
        }

        // 候補選択中に句読点 → 現在の punct_pending を上書きしてウィンドウを更新
        {
            let mut sess = session_get()?;
            if sess.is_selecting() {
                sess.set_punct_pending(c);
                // 候補ウィンドウのステータスラインに句読点が付くことを示す
                let page_cands = sess.page_candidates().to_vec();
                let page_sel = sess.page_selected();
                let page_info = sess.page_info();
                let (pos_x, pos_y) = sess.selecting_pos().unwrap_or_default();
                drop(sess);
                drop(guard);
                candidate_window::show_with_status(
                    &page_cands,
                    page_sel,
                    &page_info,
                    pos_x,
                    pos_y,
                    Some(&format!("確定後「{c}」を入力")),
                );
                return Ok(true);
            }
        }

        // 未変換プリエディットあり → Convert と同じフローで変換ウィンドウを開く
        // punct_pending は activate_selecting 後にセットする
        engine.flush_pending_n();
        let preedit = engine.preedit_display();
        update_caret_rect(ctx.clone(), tid);

        let llm_limit = crate::engine::state::get_num_candidates();
        const DICT_LIMIT: usize = 40;
        let _ = crate::engine::state::poll_dict_ready_cached(engine);
        let _ = crate::engine::state::poll_model_ready_cached(engine);
        let kanji_ready = engine.is_kanji_ready();
        if kanji_ready && engine.bg_status() == "idle" {
            engine.bg_start(llm_limit);
        }
        const BG_WAIT_MS: u64 = 400;
        if kanji_ready && matches!(engine.bg_status(), "running" | "idle") {
            engine.bg_wait_ms(BG_WAIT_MS);
        }

        let bg_status = engine.bg_status();
        let bg_running = !kanji_ready || bg_status == "running" || bg_status == "idle";

        let (candidates, llm_pending): (Vec<String>, bool) =
            match engine.bg_take_candidates(&preedit) {
                Some(llm_cands) if !llm_cands.is_empty() => {
                    let merged = engine.merge_candidates(llm_cands, DICT_LIMIT);
                    tracing::debug!("merge_candidates → {:?}", merged);
                    if merged.is_empty() || (merged.len() == 1 && merged[0] == preedit) {
                        (
                            engine_convert_sync_multi(engine, llm_limit, DICT_LIMIT, &preedit),
                            false,
                        )
                    } else {
                        (merged, false)
                    }
                }
                _ => {
                    if kanji_ready && !bg_running {
                        (
                            engine_convert_sync_multi(engine, llm_limit, DICT_LIMIT, &preedit),
                            false,
                        )
                    } else {
                        let dict_cands = engine.merge_candidates(vec![], DICT_LIMIT);
                        let dict_empty = dict_cands.is_empty()
                            || (dict_cands.len() == 1 && dict_cands[0] == preedit);
                        if dict_empty {
                            (vec![preedit.clone()], bg_running)
                        } else {
                            (dict_cands, bg_running)
                        }
                    }
                }
            };

        let first = candidates
            .first()
            .cloned()
            .unwrap_or_else(|| preedit.clone());
        let caret = caret_rect_get();
        {
            let mut sess = session_get()?;
            sess.activate_selecting(
                candidates,
                preedit.clone(),
                caret.left,
                caret.bottom,
                llm_pending,
            );
            sess.set_punct_pending(c);
            let page_cands = sess.page_candidates().to_vec();
            let page_info = sess.page_info();
            drop(sess);
            drop(guard);
            let status_owned = format!("確定後「{c}」を入力");
            candidate_window::show_with_status(
                &page_cands,
                0,
                &page_info,
                caret.left,
                caret.bottom,
                Some(&status_owned),
            );
        }
        update_composition(ctx, tid, sink, first)?;
        Ok(true)
    }

    /// Left: 選択文節を左へ移動する。
    fn on_segment_move_left(
        &self,
        _ctx: ITfContext,
        _tid: u32,
        _sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        Ok(!engine.preedit_is_empty())
    }

    /// Shift+Left: 選択範囲を左側から縮めるのではなく、右端を左へ戻す。
    fn on_segment_shrink(
        &self,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        let mut sess = session_get()?;

        tracing::debug!("on_segment_shrink: state={:?}", &*sess);

        // LiveConv → RangeSelect（全文ひらがなに戻して先頭から範囲指定）
        if sess.is_live_conv() {
            let (reading, preview) = sess
                .live_conv_parts()
                .map(|(r, p)| (r.to_string(), p.to_string()))
                .unwrap_or_default();
            if reading.is_empty() {
                return Ok(true);
            }
            let chars: Vec<char> = reading.chars().collect();
            let select_end = chars.len(); // Shift+Left なので最初は全選択から1文字縮める
            sess.set_range_select(reading.clone(), select_end.saturating_sub(1), preview);
            let (selected, unselected) = sess.range_select_parts().unwrap_or_default();
            drop(sess);
            candidate_window::hide();
            candidate_window::stop_live_timer();
            engine.bg_reclaim();
            drop(guard);
            update_composition_candidate_parts(
                ctx,
                tid,
                sink,
                String::new(),
                selected,
                unselected,
            )?;
            return Ok(true);
        }

        // RangeSelect → Shift+Left で選択範囲を縮める
        if sess.is_range_select() {
            if !sess.range_select_shrink() {
                return Ok(true);
            }
            let (selected, unselected) = sess.range_select_parts().unwrap_or_default();
            drop(sess);
            drop(guard);
            update_composition_candidate_parts(
                ctx,
                tid,
                sink,
                String::new(),
                selected,
                unselected,
            )?;
            return Ok(true);
        }

        // Selecting → RangeSelect（ひらがなに戻して末尾から範囲指定）
        if sess.is_selecting() {
            let reading = sess.original_preedit().unwrap_or("").to_string();
            if reading.is_empty() {
                return Ok(true);
            }
            let char_count = reading.chars().count();
            sess.set_range_select(reading.clone(), char_count.saturating_sub(1), String::new());
            let (selected, unselected) = sess.range_select_parts().unwrap_or_default();
            drop(sess);
            candidate_window::hide();
            candidate_window::stop_live_timer();
            engine.bg_reclaim();
            engine.force_preedit(reading);
            drop(guard);
            update_composition_candidate_parts(
                ctx,
                tid,
                sink,
                String::new(),
                selected,
                unselected,
            )?;
            return Ok(true);
        }

        // Preedit → RangeSelect（末尾から 1 文字除いて選択）
        if matches!(&*sess, SessionState::Preedit { .. }) {
            let reading = engine.hiragana_text().to_string();
            let char_count = reading.chars().count();
            if char_count > 1 {
                sess.set_range_select(reading, char_count - 1, String::new());
                let (selected, unselected) = sess.range_select_parts().unwrap_or_default();
                drop(sess);
                candidate_window::stop_live_timer();
                engine.bg_reclaim();
                drop(guard);
                update_composition_candidate_parts(
                    ctx,
                    tid,
                    sink,
                    String::new(),
                    selected,
                    unselected,
                )?;
                return Ok(true);
            }
        }

        tracing::debug!("  → no matching state, eat={}", !engine.preedit_is_empty());
        Ok(!engine.preedit_is_empty())
    }

    /// Right: 選択文節を右へ移動する。
    fn on_segment_move_right(
        &self,
        _ctx: ITfContext,
        _tid: u32,
        _sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        Ok(!engine.preedit_is_empty())
    }

    /// Shift+Right: 選択範囲を右へ広げる。
    fn on_segment_extend(
        &self,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        let mut sess = session_get()?;

        // LiveConv → RangeSelect（先頭 1 文字を選択して開始）
        if sess.is_live_conv() {
            let (reading, preview) = sess
                .live_conv_parts()
                .map(|(r, p)| (r.to_string(), p.to_string()))
                .unwrap_or_default();
            if reading.is_empty() {
                return Ok(true);
            }
            sess.set_range_select(reading, 1, preview);
            let (selected, unselected) = sess.range_select_parts().unwrap_or_default();
            drop(sess);
            candidate_window::hide();
            candidate_window::stop_live_timer();
            engine.bg_reclaim();
            drop(guard);
            update_composition_candidate_parts(
                ctx,
                tid,
                sink,
                String::new(),
                selected,
                unselected,
            )?;
            return Ok(true);
        }

        // RangeSelect → Shift+Right で選択範囲を伸ばす
        if sess.is_range_select() {
            if !sess.range_select_extend() {
                return Ok(true);
            }
            let (selected, unselected) = sess.range_select_parts().unwrap_or_default();
            drop(sess);
            drop(guard);
            update_composition_candidate_parts(
                ctx,
                tid,
                sink,
                String::new(),
                selected,
                unselected,
            )?;
            return Ok(true);
        }

        // Selecting → RangeSelect（先頭 1 文字を選択して開始）
        if sess.is_selecting() {
            let reading = sess.original_preedit().unwrap_or("").to_string();
            if !reading.is_empty() {
                sess.set_range_select(reading.clone(), 1, String::new());
                let (selected, unselected) = sess.range_select_parts().unwrap_or_default();
                drop(sess);
                candidate_window::hide();
                candidate_window::stop_live_timer();
                engine.bg_reclaim();
                engine.force_preedit(reading);
                drop(guard);
                update_composition_candidate_parts(
                    ctx,
                    tid,
                    sink,
                    String::new(),
                    selected,
                    unselected,
                )?;
                return Ok(true);
            }
        }

        // Preedit → RangeSelect（先頭 1 文字を選択して開始）
        if matches!(&*sess, SessionState::Preedit { .. }) {
            let reading = engine.hiragana_text().to_string();
            if !reading.is_empty() {
                sess.set_range_select(reading, 1, String::new());
                let (selected, unselected) = sess.range_select_parts().unwrap_or_default();
                drop(sess);
                candidate_window::stop_live_timer();
                engine.bg_reclaim();
                drop(guard);
                update_composition_candidate_parts(
                    ctx,
                    tid,
                    sink,
                    String::new(),
                    selected,
                    unselected,
                )?;
                return Ok(true);
            }
        }

        Ok(!engine.preedit_is_empty())
    }
}

// ─── 変換ヘルパー ─────────────────────────────────────────────────────────────

/// 複数候補を返す版（候補ウィンドウ用）
/// プリエディット（ひらがな）をそのまま確定してコンポジションを終了する。
/// 辞書0件 + LLM 待機中に Space を2回押したときの逃げ道として使用する。
#[allow(dead_code)]
fn engine_commit_hiragana(ctx: ITfContext, tid: u32) -> Result<()> {
    let preedit = {
        let mut guard = engine_get()
            .map_err(|e| anyhow::anyhow!("engine_commit_hiragana: engine unavailable: {e}"))?;
        let engine = guard
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("engine_commit_hiragana: engine is None"))?;
        let p = engine.preedit_display();
        if !p.is_empty() {
            engine.bg_reclaim();
            engine.commit(&p);
            engine.reset_preedit();
        }
        // 選択待機状態もクリア
        if let Ok(mut sess) = session_get() {
            if sess.is_waiting() || sess.is_selecting() {
                sess.set_idle();
            }
        }
        p
    };
    if preedit.is_empty() {
        return Ok(());
    }
    tracing::debug!("engine_commit_hiragana: committing preedit={preedit:?}");
    end_composition(ctx, tid, preedit)
}

fn engine_convert_sync_multi(
    engine: &mut crate::engine::state::DynEngine,
    llm_limit: usize,
    dict_limit: usize,
    preedit: &str,
) -> Vec<String> {
    // LLM候補を取得（llm_limit 件）
    let llm_cands: Vec<String> = engine.convert_sync();
    let _ = llm_limit; // DynEngine::convert_sync は num_candidates を内部設定から読む

    // 辞書候補とマージ（dict_limit 件まで）
    let merged = engine.merge_candidates(llm_cands, dict_limit);
    tracing::debug!("merge_candidates → {:?}", merged);
    if merged.is_empty() {
        vec![preedit.to_string()]
    } else {
        merged
    }
}

// ─── OnTestKeyDown ヘルパー ──────────────────────────────────────────────────

#[inline]
fn key_should_eat(action: &UserAction, has_preedit: bool) -> bool {
    match action {
        UserAction::Input(_) | UserAction::InputRaw(_) | UserAction::FullWidthSpace => true,
        UserAction::Backspace => has_preedit,
        UserAction::Convert => true,
        UserAction::ImeToggle
        | UserAction::ImeOff
        | UserAction::ImeOn
        | UserAction::ModeHiragana
        | UserAction::ModeKatakana
        | UserAction::ModeAlphanumeric => true,
        UserAction::CommitRaw
        | UserAction::Cancel
        | UserAction::CancelAll
        | UserAction::Hiragana
        | UserAction::Katakana
        | UserAction::HalfKatakana
        | UserAction::FullLatin
        | UserAction::HalfLatin
        | UserAction::CycleKana
        | UserAction::CandidateNext
        | UserAction::CandidatePrev
        | UserAction::CandidatePageDown
        | UserAction::CandidatePageUp
        | UserAction::CursorLeft
        | UserAction::CursorRight => has_preedit,
        // Shift+Left/Right: composition がアクティブな間は必ず消費する。
        // 透過させるとアプリが composition テキストを直接編集してしまう。
        // has_preedit=false（composition なし）のときだけ透過。
        UserAction::SegmentShrink | UserAction::SegmentExtend => has_preedit,
        UserAction::Punctuate(_) => true,
        UserAction::CandidateSelect(_) => has_preedit,
        _ => false,
    }
}

#[inline]
fn action_name(a: &UserAction) -> &'static str {
    match a {
        UserAction::Input(_) => "Input",
        UserAction::InputRaw(_) => "InputRaw",
        UserAction::FullWidthSpace => "FullWidthSpace",
        UserAction::Convert => "Convert",
        UserAction::CommitRaw => "CommitRaw",
        UserAction::Backspace => "Backspace",
        UserAction::Cancel => "Cancel",
        UserAction::CancelAll => "CancelAll",
        UserAction::Hiragana => "Hiragana",
        UserAction::Katakana => "Katakana",
        UserAction::HalfKatakana => "HalfKatakana",
        UserAction::FullLatin => "FullLatin",
        UserAction::HalfLatin => "HalfLatin",
        UserAction::CycleKana => "CycleKana",
        UserAction::CandidateNext => "CandidateNext",
        UserAction::CandidatePrev => "CandidatePrev",
        UserAction::CandidatePageDown => "CandidatePageDown",
        UserAction::CandidatePageUp => "CandidatePageUp",
        UserAction::CandidateSelect(_) => "CandidateSelect",
        UserAction::CursorLeft => "CursorLeft",
        UserAction::CursorRight => "CursorRight",
        UserAction::Punctuate(_) => "Punctuate",
        UserAction::SegmentShrink => "SegmentShrink",
        UserAction::SegmentExtend => "SegmentExtend",
        UserAction::ImeToggle => "ImeToggle",
        UserAction::ImeOn => "ImeOn",
        UserAction::ImeOff => "ImeOff",
        UserAction::ModeHiragana => "ModeHiragana",
        UserAction::ModeKatakana => "ModeKatakana",
        UserAction::ModeAlphanumeric => "ModeAlphanumeric",
        _ => "Other",
    }
}

// ─── EditSession ヘルパー ─────────────────────────────────────────────────────
// TF_ES_SYNC を使わない（TF_ES_READWRITE のみ）

/// TSF コンテキストからキャレットのスクリーン座標 (x, y_bottom) を取得する。
/// mozc の FillCharPosition と同じアプローチ: GetSelection → GetTextExt。
/// 取得できない場合は None を返す（インジケーターは表示しない）。
unsafe fn get_caret_pos_from_context(
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    ec: u32,
) -> Option<(i32, i32)> {
    let range = get_cursor_range(ctx, ec)?;
    let view = ctx.GetActiveView().ok()?;
    let mut rect = windows::Win32::Foundation::RECT::default();
    let mut clipped = windows::Win32::Foundation::BOOL(0);
    view.GetTextExt(ec, &range, &mut rect, &mut clipped).ok()?;
    // rect はスクリーン座標。left = x, bottom = キャレット下端。
    Some((rect.left, rect.bottom))
}

/// 現在のキャレット位置を表す長さ0の ITfRange を返す。
/// GetSelection で現在選択範囲を取得し、終端アンカーに collapse する。
/// 失敗時は None（呼び元が GetEnd にフォールバックする）。
unsafe fn get_cursor_range(
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    ec: u32,
) -> Option<windows::Win32::UI::TextServices::ITfRange> {
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::UI::TextServices::{
        TfActiveSelEnd, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE,
    };

    // windows-rs 0.58: GetSelection(ec, ulIndex, pSelection: &mut [TF_SELECTION]) -> *mut u32
    // TF_DEFAULT_SELECTION = 0xFFFF_FFFF
    let mut sel_buf = [TF_SELECTION {
        range: std::mem::ManuallyDrop::new(None),
        style: TF_SELECTIONSTYLE {
            ase: TfActiveSelEnd(0),
            fInterimChar: BOOL(0),
        },
    }];
    let mut fetched: u32 = 0;
    ctx.GetSelection(ec, 0xFFFF_FFFF_u32, &mut sel_buf, &mut fetched as *mut u32)
        .ok()?;
    if fetched == 0 {
        return None;
    }
    let range_ref = (&*sel_buf[0].range).as_ref()?;
    let cloned = range_ref.Clone().ok()?;
    if let Err(e) = cloned.Collapse(ec, TF_ANCHOR_END) {
        tracing::warn!("get_cursor_range: Collapse failed: {e}, range may not be zero-length");
    }
    Some(cloned)
}

/// 現在カーソル位置の range を優先し、取得できなければ `GetEnd` にフォールバックする。
///
/// TSF/COM が不安定な瞬間でも panic でホストプロセスを巻き込まないよう、
/// 失敗は `E_FAIL` に変換して呼び元へ返す。
unsafe fn get_insert_range_or_end(
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    ec: u32,
    op: &str,
) -> windows::core::Result<windows::Win32::UI::TextServices::ITfRange> {
    use windows::Win32::Foundation::E_FAIL;

    if let Some(range) = get_cursor_range(ctx, ec) {
        return Ok(range);
    }

    tracing::debug!("{op}: cursor range unavailable, falling back to GetEnd");
    ctx.GetEnd(ec)
        .map_err(|e| windows::core::Error::new(E_FAIL, format!("{op}: GetEnd: {e}")))
}

/// `GetEnd` を安全に取得する。
///
/// `commit_then_start_composition` のように「現在選択位置を使うと意味が変わる」
/// 経路では、cursor range を見に行かず `GetEnd` を明示的に使う。
unsafe fn get_document_end_range(
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    ec: u32,
    op: &str,
) -> windows::core::Result<windows::Win32::UI::TextServices::ITfRange> {
    use windows::Win32::Foundation::E_FAIL;

    ctx.GetEnd(ec)
        .map_err(|e| windows::core::Error::new(E_FAIL, format!("{op}: GetEnd: {e}")))
}

fn update_composition(
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    preedit: String,
) -> Result<()> {
    let existing = composition_clone()?;
    // M1.8 T-MID2: stale check 用に外側 snapshot のポインタを記録。
    // EditSession クロージャは TF_ES_READWRITE で遅延実行されるため、
    // ここで取った composition が DM 破棄や invalidate_composition_for_dm で
    // stale 化したまま SetText しないよう、クロージャ先頭で再検査する。
    let existing_ptr = existing.as_ref().map(|c| c.as_raw() as usize).unwrap_or(0);
    let ctx_req = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            ITfContextComposition, TfActiveSelEnd, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE,
        };

        // M1.8 T-MID2: クロージャ実行時点で composition が
        // 外側 snapshot と同一かを再確認。異なれば SetText せず no-op。
        // - existing=Some, current=None: invalidate_composition_for_dm で stale 化
        // - existing=Some(a), current=Some(b) で a != b: composition が置換された
        // - existing=None, current=Some: 別経路で新規 composition が立った
        // のいずれも安全側で abort する。
        let current = composition_clone()
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("comp re-check: {e}")))?;
        let current_ptr = current.as_ref().map(|c| c.as_raw() as usize).unwrap_or(0);
        if current_ptr != existing_ptr {
            tracing::debug!(
                "update_composition: stale snapshot, abort SetText (existing={:#x} current={:#x})",
                existing_ptr,
                current_ptr
            );
            return Ok(());
        }

        let preedit_w: Vec<u16> = preedit.encode_utf16().collect();
        tracing::debug!(
            "update_composition[EditSession]: preedit={:?} existing={}",
            preedit,
            existing.is_some()
        );

        let range = if let Some(comp) = &existing {
            comp.GetRange()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange: {e}")))?
        } else {
            // Fix2: GetEnd(文書末尾)ではなく現在のカーソル位置を使う
            let insert_point = get_insert_range_or_end(&ctx, ec, "update_composition")?;
            let cc: ITfContextComposition = ctx.cast().map_err(|e| {
                windows::core::Error::new(E_FAIL, format!("cast ITfContextComposition: {e}"))
            })?;
            let new_comp = cc
                .StartComposition(ec, &insert_point, &sink)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("StartComposition: {e}")))?;
            let r = new_comp
                .GetRange()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange new: {e}")))?;
            let dm_ptr = ctx
                .GetDocumentMgr()
                .ok()
                .map(|dm| dm.as_raw() as usize)
                .unwrap_or(0);
            let _ = composition_set_with_dm(Some(new_comp), dm_ptr);
            r
        };

        // M1.8 T-MID3: SetText 排他化。Phase1A 経路の SetText と直列化する。
        // busy なら skip し、上位は no-op として処理する（次回 update_composition
        // が新しい preedit で再 SetText するので整合は保てる）。
        {
            let _apply_guard = match crate::engine::state::COMPOSITION_APPLY_LOCK.try_lock() {
                Ok(g) => g,
                Err(_) => {
                    tracing::debug!(
                        "update_composition: COMPOSITION_APPLY_LOCK busy, skip SetText"
                    );
                    return Ok(());
                }
            };
            range
                .SetText(ec, 0, &preedit_w)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText: {e}")))?;
        }

        // アンダーライン属性をセット
        // SESSION_SELECTING アトミックで高速判定（クロージャ内なので Mutex は取れない）
        let atom = display_attr::atom_input();
        set_display_attr_prop(&ctx, ec, &range, atom);

        // プリエディット中もカーソルを末尾に置く（アプリのキャレット表示を正しくする）
        if let Ok(cursor) = range.Clone() {
            let _ = cursor.Collapse(ec, TF_ANCHOR_END);
            let sel = TF_SELECTION {
                range: std::mem::ManuallyDrop::new(Some(cursor)),
                style: TF_SELECTIONSTYLE {
                    ase: TfActiveSelEnd(0),
                    fInterimChar: windows::Win32::Foundation::BOOL(0),
                },
            };
            let _ = ctx.SetSelection(ec, &[sel]);
        }

        Ok(())
    });
    unsafe {
        let _ = ctx_req
            .RequestEditSession(tid, &session, TF_ES_READWRITE)
            .map_err(|e| anyhow::anyhow!("RequestEditSession update: {e}"));
    }
    Ok(())
}

/// 確定テキストを commit し、即座に新しい composition を開始する（1 EditSession）。
///
/// end_composition + update_composition を別々に呼ぶと TSF が2セッションを
/// 別タイミングで実行し、"composition=None" の瞬間にアプリがテキストを
/// クリアすることがある。これを1セッションにまとめて防ぐ。
fn commit_then_start_composition(
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    commit_text: String,
    next_preedit: String,
) -> Result<()> {
    // composition_take() をセッション内に移動する（end_composition と同じ理由）。
    // セッション外で take すると COMPOSITION=None になった瞬間に update_composition が
    // 誤ったカーソル位置から新 composition を開始するリスクがある。
    let ctx_req = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            ITfContextComposition, TfActiveSelEnd, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE,
        };

        let comp = composition_take().unwrap_or(None);
        tracing::debug!(
            "commit_then_start[session]: commit={:?} next={:?} has_comp={}",
            commit_text,
            next_preedit,
            comp.is_some()
        );

        // ── Step1: 既存 composition を確定テキストで終了 ──
        // 文節分割後に候補表示している場合、composition のテキストは
        // "確定部分 + remainder" の全体になっている。
        // EndComposition だけだとその全体が確定されてしまうため、
        // 先に SetText で commit_text だけに縮めてから EndComposition する。
        let commit_w: Vec<u16> = commit_text.encode_utf16().collect();
        // EndComposition 後の挿入位置: composition range の末尾（確定テキストの直後）を保存する。
        // EndComposition 後は GetSelection が composition 開始位置を返すことがあるため
        // EndComposition 前に range の末尾を取得しておく。
        let mut insert_after_commit: Option<windows::Win32::UI::TextServices::ITfRange> = None;
        if let Some(comp) = comp {
            // composition テキストを commit_text だけに縮める
            if let Ok(range) = comp.GetRange() {
                let _ = range.SetText(ec, 0, &commit_w);
                // 確定テキストの末尾位置を保存
                if let Ok(end_range) = range.Clone() {
                    let _ = end_range.Collapse(ec, TF_ANCHOR_END);
                    insert_after_commit = Some(end_range);
                }
            } else {
                tracing::warn!("commit_then_start: comp.GetRange() failed");
            }
            comp.EndComposition(ec)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("EndComposition: {e}")))?;
        } else if !commit_text.is_empty() {
            let insert_point =
                get_insert_range_or_end(&ctx, ec, "commit_then_start direct commit")?;
            insert_point.SetText(ec, 0, &commit_w).map_err(|e| {
                windows::core::Error::new(E_FAIL, format!("SetText direct commit: {e}"))
            })?;
            if let Ok(end_range) = insert_point.Clone() {
                let _ = end_range.Collapse(ec, TF_ANCHOR_END);
                insert_after_commit = Some(end_range);
            }
        }

        if next_preedit.is_empty() {
            return Ok(());
        }

        // ── Step2: 同セッション内で新 composition 開始 ──
        // EndComposition 前に保存した確定テキスト末尾位置から新 composition を開始する。
        // EndComposition 後の GetSelection はカーソルが composition 開始位置を示すことがあり
        // 使用できない。ctx.GetEnd(ec) はドキュメント末尾を返すため文章途中の編集で問題になる。
        let insert_point = if let Some(p) = insert_after_commit {
            p
        } else {
            tracing::warn!("commit_then_start: insert_after_commit=None, falling back to GetEnd");
            get_document_end_range(&ctx, ec, "commit_then_start new composition")?
        };
        let cc: ITfContextComposition = ctx.cast().map_err(|e| {
            windows::core::Error::new(E_FAIL, format!("cast ITfContextComposition: {e}"))
        })?;
        let new_comp = cc
            .StartComposition(ec, &insert_point, &sink)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("StartComposition: {e}")))?;
        let new_range = new_comp
            .GetRange()
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange new: {e}")))?;
        let dm_ptr = ctx
            .GetDocumentMgr()
            .ok()
            .map(|dm| dm.as_raw() as usize)
            .unwrap_or(0);
        let _ = composition_set_with_dm(Some(new_comp), dm_ptr);

        let preedit_w: Vec<u16> = next_preedit.encode_utf16().collect();
        new_range
            .SetText(ec, 0, &preedit_w)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText new: {e}")))?;

        // 新 composition にもアンダーライン属性をセット
        set_display_attr_prop(&ctx, ec, &new_range, display_attr::atom_input());

        // カーソルを末尾に
        if let Ok(cursor) = new_range.Clone() {
            let _ = cursor.Collapse(ec, TF_ANCHOR_END);
            let sel = TF_SELECTION {
                range: std::mem::ManuallyDrop::new(Some(cursor)),
                style: TF_SELECTIONSTYLE {
                    ase: TfActiveSelEnd(0),
                    fInterimChar: windows::Win32::Foundation::BOOL(0),
                },
            };
            let _ = ctx.SetSelection(ec, &[sel]);
        }
        Ok(())
    });
    unsafe {
        let _ = ctx_req
            .RequestEditSession(tid, &session, TF_ES_READWRITE)
            .map_err(|e| anyhow::anyhow!("RequestEditSession commit_then_start: {e}"));
    }
    Ok(())
}

/// GUID_PROP_ATTRIBUTE プロパティを range にセットしてアンダーラインを要求する
///
/// atom が 0（未登録）の場合は何もしない。
/// アプリが属性を無視する場合もあるが、メモ帳・Word 等の標準アプリでは表示される。
unsafe fn set_display_attr_prop(
    ctx: &ITfContext,
    ec: u32,
    range: &windows::Win32::UI::TextServices::ITfRange,
    atom: u32,
) {
    if atom == 0 {
        return;
    }
    let Ok(prop) = ctx.GetProperty(&GUID_PROP_ATTRIBUTE) else {
        return;
    };
    // 既存の属性を先にクリアして TSF に変更を通知させる
    let _ = prop.Clear(ec, range);
    // windows_core::VARIANT で VT_I4 (atom) を設定
    let var = windows_core::VARIANT::from(atom as i32);
    let _ = prop.SetValue(ec, range, &var);
}

/// 変換候補（`converted`）と未変換残り（`remainder`）を1つの composition に表示する。
///
/// `converted` + `remainder` を結合して composition にセットし、属性は
/// converted 部分を atom_converted（太実線）、remainder 部分を atom_input（点線）で付与する。
/// TSF の `ShiftEnd`/`ShiftStart` は実装によって挙動が異なるため使用しない。
/// `GetProperty → EnumerateRanges` ではなく 1 property に 2 値を書く安全な方法として
/// 先に全体を atom_converted で塗り、その後 remainder 部分のみ atom_input で上書きする。
///
/// `remainder` が空の場合は通常の `update_composition` と同じ動作になる。
fn update_composition_candidate_parts(
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    prefix: String,
    converted: String,
    suffix: String,
) -> Result<()> {
    if prefix.is_empty() && suffix.is_empty() {
        return update_composition(ctx, tid, sink, converted);
    }

    let existing = composition_clone()?;
    // M1.8 T-MID2: update_composition と同じ stale check を入れる
    let existing_ptr = existing.as_ref().map(|c| c.as_raw() as usize).unwrap_or(0);
    let ctx_req = ctx.clone();
    let full = format!("{prefix}{converted}{suffix}");
    let prefix_utf16: i32 = prefix.encode_utf16().count() as i32;

    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            ITfContextComposition, TfActiveSelEnd, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE,
        };

        // M1.8 T-MID2: クロージャ実行時点の stale check
        let current = composition_clone()
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("comp re-check: {e}")))?;
        let current_ptr = current.as_ref().map(|c| c.as_raw() as usize).unwrap_or(0);
        if current_ptr != existing_ptr {
            tracing::debug!(
                "update_composition_candidate_parts: stale snapshot, abort SetText (existing={:#x} current={:#x})",
                existing_ptr,
                current_ptr
            );
            return Ok(());
        }

        let full_w: Vec<u16> = full.encode_utf16().collect();

        // ── Step1: テキストをセット ──
        let range = if let Some(comp) = &existing {
            comp.GetRange()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange: {e}")))?
        } else {
            let insert_point =
                get_insert_range_or_end(&ctx, ec, "update_composition_candidate_parts")?;
            let cc: ITfContextComposition = ctx
                .cast()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("cast: {e}")))?;
            let new_comp = cc
                .StartComposition(ec, &insert_point, &sink)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("StartComposition: {e}")))?;
            let r = new_comp
                .GetRange()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange new: {e}")))?;
            let dm_ptr = ctx
                .GetDocumentMgr()
                .ok()
                .map(|dm| dm.as_raw() as usize)
                .unwrap_or(0);
            let _ = composition_set_with_dm(Some(new_comp), dm_ptr);
            r
        };

        // M1.8 T-MID3: SetText 排他化（candidate_parts 経路）。
        {
            let _apply_guard = match crate::engine::state::COMPOSITION_APPLY_LOCK.try_lock() {
                Ok(g) => g,
                Err(_) => {
                    tracing::debug!(
                        "update_composition_candidate_parts: COMPOSITION_APPLY_LOCK busy, skip SetText"
                    );
                    return Ok(());
                }
            };
            range
                .SetText(ec, 0, &full_w)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText: {e}")))?;
        }

        // ── Step2: 属性セット ──
        // 全体を atom_input（点線）で塗り、選択中ブロックのみ atom_converted（太実線）で上書きする
        set_display_attr_prop(&ctx, ec, &range, display_attr::atom_input());
        if let Ok(sel_range) = range.Clone() {
            let mut actual = 0i32;
            let suffix_utf16: i32 = suffix.encode_utf16().count() as i32;
            let _ = sel_range.ShiftStart(
                ec,
                prefix_utf16,
                &mut actual,
                std::ptr::null::<windows::Win32::UI::TextServices::TF_HALTCOND>(),
            );
            if suffix_utf16 > 0 {
                let _ = sel_range.ShiftEnd(
                    ec,
                    -suffix_utf16,
                    &mut actual,
                    std::ptr::null::<windows::Win32::UI::TextServices::TF_HALTCOND>(),
                );
            }
            set_display_attr_prop(&ctx, ec, &sel_range, display_attr::atom_converted());
        }

        // ── Step3: カーソルを末尾に ──
        if let Ok(cursor) = range.Clone() {
            let _ = cursor.Collapse(ec, TF_ANCHOR_END);
            let sel = TF_SELECTION {
                range: std::mem::ManuallyDrop::new(Some(cursor)),
                style: TF_SELECTIONSTYLE {
                    ase: TfActiveSelEnd(0),
                    fInterimChar: windows::Win32::Foundation::BOOL(0),
                },
            };
            let _ = ctx.SetSelection(ec, &[sel]);
        }
        Ok(())
    });
    unsafe {
        let _ = ctx_req
            .RequestEditSession(tid, &session, TF_ES_READWRITE)
            .map_err(|e| anyhow::anyhow!("RequestEditSession candidate_split: {e}"));
    }
    Ok(())
}

/// スペース押下時のみ呼ぶ: caret_rect をキャレット位置で更新する
fn update_caret_rect(ctx: ITfContext, tid: u32) {
    let comp = match composition_clone() {
        Ok(Some(c)) => c,
        _ => return,
    };
    let ctx_req = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        if let Ok(range) = comp.GetRange() {
            if let Ok(view) = ctx.GetActiveView() {
                use windows::Win32::Foundation::RECT;
                let mut rect = RECT::default();
                let mut clipped = windows::Win32::Foundation::BOOL(0);
                if view.GetTextExt(ec, &range, &mut rect, &mut clipped).is_ok() {
                    caret_rect_set(rect);
                }
            }
        }
        Ok(())
    });
    unsafe {
        let _ = ctx_req.RequestEditSession(tid, &session, TF_ES_READWRITE);
    }
}

fn end_composition(ctx: ITfContext, tid: u32, text: String) -> Result<()> {
    use windows::Win32::UI::TextServices::{
        TfActiveSelEnd, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE,
    };
    // composition_take() をセッション内に移動する。
    // セッション外で take すると COMPOSITION=None になった直後に次のキー入力が来たとき、
    // update_composition が existing=None を見て誤った位置から新 composition を開始してしまう。
    let ctx2 = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        let comp = match composition_take().unwrap_or(None) {
            Some(c) => c,
            None => {
                tracing::debug!("end_composition: no composition, inserting text directly");
                // composition がない場合はカーソル位置に直接挿入
                if !text.is_empty() {
                    let text_w: Vec<u16> = text.encode_utf16().collect();
                    let insert =
                        get_insert_range_or_end(&ctx2, ec, "end_composition direct insert")?;
                    let _ = insert.SetText(ec, 0, &text_w);
                }
                return Ok(());
            }
        };

        let text_w: Vec<u16> = text.encode_utf16().collect();
        tracing::debug!("end_composition[session]: text={:?}", text);
        let range = comp
            .GetRange()
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange: {e}")))?;
        range
            .SetText(ec, 0, &text_w)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText end: {e}")))?;

        // Fix3: EndComposition の前に SetSelection する
        // （EndComposition 後に SetSelection するとアプリがカーソルをリセットしてしまうため）
        if let Ok(cursor) = range.Clone() {
            let _ = cursor.Collapse(ec, TF_ANCHOR_END);
            let sel = TF_SELECTION {
                range: std::mem::ManuallyDrop::new(Some(cursor)),
                style: TF_SELECTIONSTYLE {
                    ase: TfActiveSelEnd(0),
                    fInterimChar: windows::Win32::Foundation::BOOL(0),
                },
            };
            let _ = ctx2.SetSelection(ec, &[sel]);
        }

        comp.EndComposition(ec)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("EndComposition: {e}")))?;
        Ok(())
    });
    unsafe {
        let _ = ctx
            .RequestEditSession(tid, &session, TF_ES_READWRITE)
            .map_err(|e| anyhow::anyhow!("RequestEditSession end: {e}"));
    }
    Ok(())
}

fn commit_text(ctx: ITfContext, tid: u32, text: String) -> Result<()> {
    let ctx_req = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            TfActiveSelEnd, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE,
        };
        let text_w: Vec<u16> = text.encode_utf16().collect();
        // 現在のカーソル位置に挿入（GetEnd=文書末尾ではなくカーソル位置）
        let insert = get_insert_range_or_end(&ctx, ec, "commit_text")?;
        insert
            .SetText(ec, 0, &text_w)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText commit: {e}")))?;
        // 挿入したテキストの末尾にカーソルを移動
        if let Ok(cursor) = insert.Clone() {
            let _ = cursor.Collapse(ec, TF_ANCHOR_END);
            let sel = TF_SELECTION {
                range: std::mem::ManuallyDrop::new(Some(cursor)),
                style: TF_SELECTIONSTYLE {
                    ase: TfActiveSelEnd(0),
                    fInterimChar: windows::Win32::Foundation::BOOL(0),
                },
            };
            let _ = ctx.SetSelection(ec, &[sel]);
        }
        Ok(())
    });
    unsafe {
        let _ = ctx_req
            .RequestEditSession(tid, &session, TF_ES_READWRITE)
            .map_err(|e| anyhow::anyhow!("RequestEditSession commit: {e}"));
    }
    Ok(())
}

// ─── ITfLangBarItem ──────────────────────────────────────────────────────────

/// 現在のバックエンドラベルを返す（例: "CPU" / "Vulkan" / "CUDA" / "初期化中..."）
fn current_backend_label() -> String {
    engine_try_get_or_create()
        .ok()
        .as_deref() // Option<MutexGuard<EngineWrapper>> → Option<&EngineWrapper>
        .and_then(|g| g.as_ref()) // Deref: EngineWrapper → Option<RakunEngine>
        .map(|e| e.backend_label())
        .unwrap_or_else(|| "初期化中...".to_string())
}

impl ITfLangBarItem_Impl for TextServiceFactory_Impl {
    fn GetInfo(&self, p: *mut TF_LANGBARITEMINFO) -> windows::core::Result<()> {
        unsafe {
            *p = language_bar::make_langbar_info();
        }
        Ok(())
    }
    fn GetStatus(&self) -> windows::core::Result<u32> {
        Ok(0)
    }
    fn Show(&self, _: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
    fn GetTooltipString(&self) -> windows::core::Result<BSTR> {
        let label = current_backend_label();
        Ok(BSTR::from(format!("rakukan [{}]", label)))
    }
}

impl ITfLangBarItemButton_Impl for TextServiceFactory_Impl {
    fn OnClick(&self, _: TfLBIClick, pt: &POINT, _: *const RECT) -> windows::core::Result<()> {
        show_langbar_popup_menu(self, pt)
    }
    fn InitMenu(&self, menu: Option<&ITfMenu>) -> windows::core::Result<()> {
        let Some(menu) = menu else {
            return Ok(());
        };

        let open = self
            .inner
            .try_borrow()
            .ok()
            .and_then(|inner| inner.thread_mgr.clone().map(|tm| get_open_close(&tm)))
            .unwrap_or(true);
        let current_mode = current_langbar_mode(open);

        unsafe {
            let hiragana = "ひらがな".encode_utf16().collect::<Vec<_>>();
            let katakana = "カタカナ".encode_utf16().collect::<Vec<_>>();
            let alnum = "英数".encode_utf16().collect::<Vec<_>>();
            let settings = "設定...".encode_utf16().collect::<Vec<_>>();
            let reload = "エンジン再起動".encode_utf16().collect::<Vec<_>>();

            let _ = menu.AddMenuItem(
                ID_MENU_MODE_HIRAGANA,
                if current_mode == crate::engine::input_mode::InputMode::Hiragana {
                    TF_LBMENUF_RADIOCHECKED
                } else {
                    0
                },
                HBITMAP::default(),
                HBITMAP::default(),
                &hiragana,
                std::ptr::null_mut(),
            );
            let _ = menu.AddMenuItem(
                ID_MENU_MODE_KATAKANA,
                if current_mode == crate::engine::input_mode::InputMode::Katakana {
                    TF_LBMENUF_RADIOCHECKED
                } else {
                    0
                },
                HBITMAP::default(),
                HBITMAP::default(),
                &katakana,
                std::ptr::null_mut(),
            );
            let _ = menu.AddMenuItem(
                ID_MENU_MODE_ALPHANUMERIC,
                if current_mode == crate::engine::input_mode::InputMode::Alphanumeric {
                    TF_LBMENUF_RADIOCHECKED
                } else {
                    0
                },
                HBITMAP::default(),
                HBITMAP::default(),
                &alnum,
                std::ptr::null_mut(),
            );
            let _ = menu.AddMenuItem(
                0,
                TF_LBMENUF_SEPARATOR,
                HBITMAP::default(),
                HBITMAP::default(),
                &[],
                std::ptr::null_mut(),
            );
            let _ = menu.AddMenuItem(
                ID_MENU_SETTINGS,
                0,
                HBITMAP::default(),
                HBITMAP::default(),
                &settings,
                std::ptr::null_mut(),
            );
            let _ = menu.AddMenuItem(
                ID_MENU_ENGINE_RELOAD,
                0,
                HBITMAP::default(),
                HBITMAP::default(),
                &reload,
                std::ptr::null_mut(),
            );
        }
        Ok(())
    }
    fn OnMenuSelect(&self, id: u32) -> windows::core::Result<()> {
        handle_langbar_menu_command(self, id);
        Ok(())
    }
    fn GetIcon(&self) -> windows::core::Result<HICON> {
        let open = self
            .inner
            .try_borrow()
            .ok()
            .and_then(|i| i.thread_mgr.clone().map(|tm| get_open_close(&tm)))
            .unwrap_or(true);
        let mode_char = if !open {
            "A"
        } else {
            use crate::engine::state::ime_state_get;
            ime_state_get()
                .ok()
                .map(|s| match s.input_mode {
                    crate::engine::input_mode::InputMode::Hiragana => "あ",
                    crate::engine::input_mode::InputMode::Katakana => "ア",
                    crate::engine::input_mode::InputMode::Alphanumeric => "A",
                })
                .unwrap_or("あ")
        };
        language_bar::create_mode_icon(mode_char)
            .or_else(|_| unsafe { language_bar::load_tray_icon() })
    }
    fn GetText(&self) -> windows::core::Result<BSTR> {
        // トレイは1〜2文字しか表示できないためモード文字のみ返す
        // バックエンド情報は GetTooltipString に集約
        let open = self
            .inner
            .try_borrow()
            .ok()
            .and_then(|i| i.thread_mgr.clone().map(|tm| get_open_close(&tm)))
            .unwrap_or(true);
        let mode_char = if !open {
            "A"
        } else {
            use crate::engine::state::ime_state_get;
            ime_state_get()
                .ok()
                .map(|s| match s.input_mode {
                    crate::engine::input_mode::InputMode::Hiragana => "あ",
                    crate::engine::input_mode::InputMode::Katakana => "ア",
                    crate::engine::input_mode::InputMode::Alphanumeric => "A",
                })
                .unwrap_or("あ")
        };
        Ok(BSTR::from(mode_char))
    }
}

// ─── ITfThreadMgrEventSink ────────────────────────────────────────────────────
//
// フォーカスが変わるたびに OnSetFocus が呼ばれる。
// DocumentManager ポインタをキーに InputMode を記憶し、
// 次回フォーカス時に復元する（MS-IME準拠）。

impl ITfThreadMgrEventSink_Impl for TextServiceFactory_Impl {
    fn OnInitDocumentMgr(&self, _pdim: Option<&ITfDocumentMgr>) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnUninitDocumentMgr(&self, pdim: Option<&ITfDocumentMgr>) -> windows::core::Result<()> {
        if let Some(dm) = pdim {
            let ptr = {
                use windows::core::Interface;
                dm.as_raw() as usize
            };
            crate::engine::state::dispose_dm_resources(ptr);
            tracing::trace!("OnUninitDocumentMgr: removed dm={ptr:#x}");
        }
        Ok(())
    }

    fn OnSetFocus(
        &self,
        pdimfocus: Option<&ITfDocumentMgr>,
        pdimprevfocus: Option<&ITfDocumentMgr>,
    ) -> windows::core::Result<()> {
        // このハンドラは msctf!_NotifyCallbacks から同期的に呼ばれる。
        // ここで msctf を再入（SetValue 等）したり COM 参照を drop すると、
        // explorer タスクバーなどで `INVALID_POINTER_READ c0000005 in
        // msctf!CThreadInputMgr::_NotifyCallbacks` を誘発することがあるため、
        // イベントをキューに積むだけで即 return する。
        // 実際の処理は WM_APP_FOCUS_CHANGED で msctf コールバック外で実行する。
        let dm_id = |d: &ITfDocumentMgr| -> usize {
            use windows::core::Interface;
            d.as_raw() as usize
        };
        let next_ptr = pdimfocus.map(dm_id).unwrap_or(0);
        let prev_ptr = pdimprevfocus.map(dm_id).unwrap_or(0);

        // 同一 DM へのフォーカス通知は無視（TSF 通知ストーム対策）
        if prev_ptr == next_ptr {
            return Ok(());
        }

        let hwnd_val = foreground_root_hwnd();
        candidate_window::post_focus_changed(prev_ptr, next_ptr, hwnd_val);
        Ok(())
    }

    fn OnPushContext(&self, _pic: Option<&ITfContext>) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnPopContext(&self, _pic: Option<&ITfContext>) -> windows::core::Result<()> {
        Ok(())
    }
}

// ─── ITfThreadFocusSink ────────────────────────────────────────────────────────
//
// スレッド (= アプリ) 単位のフォーカス変化通知。Alt+Tab や別プロセスへの
// フォーカス遷移で発火する。ITfThreadMgrEventSink::OnSetFocus は TSF 対応
// アプリ間でしか呼ばれないため、非対応アプリへ抜けたときの候補ウィンドウ
// 残留を防ぐためにこちらも必要。

impl ITfThreadFocusSink_Impl for TextServiceFactory_Impl {
    fn OnSetThreadFocus(&self) -> windows::core::Result<()> {
        tracing::debug!("OnSetThreadFocus");
        Ok(())
    }

    fn OnKillThreadFocus(&self) -> windows::core::Result<()> {
        tracing::debug!("OnKillThreadFocus: hide candidate window & stop live timer");
        candidate_window::hide();
        candidate_window::stop_live_timer();
        candidate_window::stop_waiting_timer();
        Ok(())
    }
}

impl ITfSource_Impl for TextServiceFactory_Impl {
    fn AdviseSink(&self, riid: *const GUID, punk: Option<&IUnknown>) -> windows::core::Result<u32> {
        let riid = unsafe { *riid };
        if riid != ITfLangBarItemSink::IID {
            return Err(windows::core::Error::new(E_INVALIDARG, "invalid sink IID"));
        }
        let punk = punk.ok_or_else(|| windows::core::Error::new(E_INVALIDARG, "null punk"))?;
        if let Ok(sink) = punk.cast::<ITfLangBarItemSink>() {
            if let Ok(mut inner) = self.inner.try_borrow_mut() {
                inner.langbar_sink = Some(sink);
            }
        }
        Ok(LANGBAR_SINK_COOKIE)
    }
    fn UnadviseSink(&self, cookie: u32) -> windows::core::Result<()> {
        if cookie != LANGBAR_SINK_COOKIE {
            return Err(windows::core::Error::new(
                CONNECT_E_CANNOTCONNECT,
                "bad cookie",
            ));
        }
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.langbar_sink = None;
        }
        Ok(())
    }
}

pub struct ClassFactory;
impl ClassFactory {
    pub fn create() -> IClassFactory {
        TextServiceFactory::new().into()
    }
}

// ─── ITfDisplayAttributeProvider ─────────────────────────────────────────────

impl ITfDisplayAttributeProvider_Impl for TextServiceFactory_Impl {
    fn EnumDisplayAttributeInfo(&self) -> windows::core::Result<IEnumTfDisplayAttributeInfo> {
        let items = display_attr::make_all();
        Ok(display_attr::EnumDisplayAttrInfo::new(items))
    }

    fn GetDisplayAttributeInfo(
        &self,
        guid: *const GUID,
    ) -> windows::core::Result<ITfDisplayAttributeInfo> {
        if guid.is_null() {
            return Err(windows::core::Error::from(
                windows::Win32::Foundation::E_INVALIDARG,
            ));
        }
        display_attr::get_by_guid(unsafe { &*guid })
    }
}
