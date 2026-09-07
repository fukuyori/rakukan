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
    Win32::{
        Foundation::{BOOL, E_FAIL, E_INVALIDARG, FALSE, LPARAM, POINT, RECT, TRUE, WPARAM},
        Graphics::Gdi::HBITMAP,
        System::{
            Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IClassFactory, IClassFactory_Impl},
            Ole::CONNECT_E_CANNOTCONNECT,
        },
        UI::{
            Input::KeyboardAndMouse::GetKeyState,
            TextServices::{
                CLSID_TF_CategoryMgr, GUID_COMPARTMENT_KEYBOARD_OPENCLOSE,
                IEnumTfDisplayAttributeInfo, ITfCategoryMgr, ITfCompartment,
                ITfCompartmentEventSink, ITfCompartmentEventSink_Impl, ITfCompartmentMgr,
                ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl, ITfContext,
                ITfDisplayAttributeInfo, ITfDisplayAttributeProvider,
                ITfDisplayAttributeProvider_Impl, ITfDocumentMgr, ITfKeyEventSink,
                ITfKeyEventSink_Impl, ITfKeystrokeMgr, ITfLangBarItem, ITfLangBarItem_Impl,
                ITfLangBarItemButton, ITfLangBarItemButton_Impl, ITfLangBarItemSink, ITfMenu,
                ITfSource, ITfSource_Impl, ITfTextInputProcessor, ITfTextInputProcessor_Impl,
                ITfThreadFocusSink, ITfThreadFocusSink_Impl, ITfThreadMgr, ITfThreadMgrEventSink,
                ITfThreadMgrEventSink_Impl, TF_ES_READWRITE, TF_LANGBARITEMINFO,
                TF_LBMENUF_RADIOCHECKED, TF_LBMENUF_SEPARATOR, TfLBIClick,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, DestroyMenu, GA_ROOT, GetAncestor,
                GetForegroundWindow, HICON, MF_SEPARATOR, TPM_LEFTALIGN, TPM_RETURNCMD,
                TPM_RIGHTBUTTON, TrackPopupMenu,
            },
        },
    },
    core::{BSTR, GUID, IUnknown, Interface, implement},
};

use crate::{
    diagnostics::{self as diag, DiagEvent},
    engine::{
        ime_mode::ImeMode,
        keymap::Keymap,
        state::{
            composition_set, doc_mode_on_focus_change, engine_get, engine_try_get_or_create,
            session_get, session_is_selecting_fast,
        },
        user_action::UserAction,
    },
    globals::{GUID_DISPLAY_ATTRIBUTE, GUID_DISPLAY_ATTRIBUTE_INPUT},
    tsf::{
        candidate_window, display_attr, ime_sync,
        language_bar::{self, LANGBAR_SINK_COOKIE},
        settings_launcher, tray_ipc,
    },
};

// M3 T1-A: factory.rs 分割。composition=on_compose, 入力=on_input, 変換=on_convert,
// 編集操作=edit_ops, dispatcher (handle_action)=dispatch。
mod dispatch;
mod edit_ops;
mod on_compose;
mod on_convert;
mod on_input;
use on_compose::{
    commit_text, commit_then_start_composition, end_composition, get_caret_pos_from_context,
    update_caret_rect, update_composition, update_composition_candidate_parts,
};

const ID_MENU_IME_ON: u32 = 1;
const ID_MENU_IME_OFF: u32 = 2;
const ID_MENU_SETTINGS: u32 = 10;
const ID_MENU_ENGINE_RELOAD: u32 = 11;

/// M1.6 T-HOST3: 読込中インジケータの記号とメッセージを決める。
///
/// 現状の `mode_indicator` は 1 文字固定のため、記号のみを表示する。メッセージ
/// は将来 mode_indicator を可変長対応化したときに使う想定で返しているだけで、
/// 今は呼び出し側で捨てて良い（`_msg` で受けている）。
///
/// 経過時間は `ready_reset_elapsed_ms()` を参照。`None`（= 初期状態 or 既に
/// ready）なら単純な砂時計を返す。
pub(super) fn loading_indicator_symbol() -> (&'static str, &'static str) {
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

/// 言語バーメニューからの IME オン/オフ。キー操作と同じ `switch_ime` を通す。
/// メニュー経由では ITfContext が無いため合成文字列の確定とインジケーター表示は行わない。
fn apply_langbar_mode(factory: &TextServiceFactory_Impl, new_mode: ImeMode) {
    let tid = factory
        .inner
        .try_borrow()
        .ok()
        .map(|inner| inner.client_id)
        .unwrap_or_default();
    tracing::info!("langbar menu: ime {:?}", new_mode);
    if let Err(e) = factory.switch_ime(None, tid, new_mode) {
        tracing::warn!("langbar menu: switch_ime failed: {e}");
    }
}

fn handle_langbar_menu_command(factory: &TextServiceFactory_Impl, id: u32) {
    match id {
        ID_MENU_IME_ON => {
            apply_langbar_mode(factory, ImeMode::On);
        }
        ID_MENU_IME_OFF => {
            apply_langbar_mode(factory, ImeMode::Off);
        }
        ID_MENU_SETTINGS => {
            settings_launcher::launch_settings_app();
        }
        ID_MENU_ENGINE_RELOAD => {
            tracing::info!("langbar menu: ID_MENU_ENGINE_RELOAD selected");
            crate::engine::config::init_config_manager();
            // 手動の「エンジン再起動」はハング復旧用途なので config が同じでも必ず再起動する
            crate::engine::state::engine_reload_force();
        }
        _ => {}
    }
}

fn show_langbar_popup_menu(
    factory: &TextServiceFactory_Impl,
    pt: &POINT,
) -> windows::core::Result<()> {
    let current_mode = crate::engine::state::ime_mode_get_atomic();

    unsafe {
        use windows::Win32::Foundation::{GetLastError, SetLastError, WIN32_ERROR};
        use windows::Win32::UI::WindowsAndMessaging::MENU_ITEM_FLAGS;

        // 言語バーのクリックがここまで届いたか、どの座標・所有ウィンドウで
        // メニューを出そうとしたかを残す（DPI 変更後にメニューが出なくなる
        // 報告の切り分け用。2026-09-06: 150% へ変更後にメニューが出なくなった）。
        let owner = GetForegroundWindow();
        tracing::info!(
            "langbar OnClick: pt=({}, {}) owner_hwnd={:#x} mode={:?}",
            pt.x,
            pt.y,
            owner.0 as usize,
            current_mode
        );

        let menu = CreatePopupMenu()?;
        let ime_on = to_wide_menu_text("IME オン");
        let ime_off = to_wide_menu_text("IME オフ");
        let settings = to_wide_menu_text("設定...");
        let reload = to_wide_menu_text("エンジン再起動");

        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(if current_mode == ImeMode::On {
                TF_LBMENUF_RADIOCHECKED
            } else {
                0
            }),
            ID_MENU_IME_ON as usize,
            windows::core::PCWSTR(ime_on.as_ptr()),
        );
        let _ = AppendMenuW(
            menu,
            MENU_ITEM_FLAGS(if current_mode == ImeMode::Off {
                TF_LBMENUF_RADIOCHECKED
            } else {
                0
            }),
            ID_MENU_IME_OFF as usize,
            windows::core::PCWSTR(ime_off.as_ptr()),
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

        // TPM_RETURNCMD 指定時はキャンセルも失敗も戻り値 0 なので、直前に
        // last error を 0 にしておき、0 が返ったときに GetLastError で区別する。
        SetLastError(WIN32_ERROR(0));
        let cmd = TrackPopupMenu(
            menu,
            TPM_LEFTALIGN | TPM_RIGHTBUTTON | TPM_RETURNCMD,
            pt.x,
            pt.y,
            0,
            owner,
            None,
        );
        let last_error = GetLastError();
        let _ = DestroyMenu(menu);

        if cmd.0 != 0 {
            tracing::info!("langbar menu: selected cmd={}", cmd.0);
            handle_langbar_menu_command(factory, cmd.0 as u32);
        } else {
            tracing::info!(
                "langbar menu: closed without selection (last_error={})",
                last_error.0
            );
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

#[derive(Default)]
pub struct TextServiceState {
    pub client_id: u32,
    pub thread_mgr: Option<ITfThreadMgr>,
    pub keymap: Keymap,
    pub langbar_sink: Option<ITfLangBarItemSink>,
    /// ITfThreadMgrEventSink の登録クッキー（Deactivate で解除）
    pub threadmgr_cookie: u32,
    /// ITfThreadFocusSink の登録クッキー（Deactivate で解除）
    pub threadfocus_cookie: u32,
    /// KEYBOARD_OPENCLOSE コンパートメントの ITfCompartmentEventSink 登録
    /// （Deactivate で解除。解除には登録先のコンパートメントが要る）
    pub openclose_cookie: u32,
    pub openclose_comp: Option<ITfCompartment>,
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
    ITfCompartmentEventSink,
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

        // KEYBOARD_OPENCLOSE を保存済みの IME オン/オフに合わせて設定する。
        // 常に true (on) にリセットすると、IME オフでウィンドウを
        // 切り替えて戻るたびにターミナルが IME ON と誤認し、かな入力が再開する。
        // アトミックを使うことでロック競合なく正確なモードを読む。
        let is_open = crate::engine::state::ime_mode_get_atomic().is_on();
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
        tray_ipc::publish(crate::engine::state::ime_mode_get_atomic());

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
            if let Ok(src) = tm.cast::<ITfSource>()
                && let Ok(sink) = self.cast::<ITfThreadFocusSink>()
                && let Ok(unk) = sink.cast::<IUnknown>()
            {
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

        // KEYBOARD_OPENCLOSE コンパートメントの変更を受け取る。IMM アプリ
        // （クリスタ等）の ImmSetOpenStatus や AHK の IMC_SETOPENSTATUS は
        // CtfImm 経由でここに来る。処理は candidate_window の遅延ハンドラで行う。
        unsafe {
            let registered = (|| -> windows::core::Result<(u32, ITfCompartment)> {
                let mgr: ITfCompartmentMgr = tm.cast()?;
                let comp = mgr.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE)?;
                let src: ITfSource = comp.cast()?;
                let sink: ITfCompartmentEventSink = self.cast()?;
                let unk: IUnknown = sink.cast()?;
                let cookie = src.AdviseSink(&ITfCompartmentEventSink::IID, &unk)?;
                Ok((cookie, comp))
            })();
            match registered {
                Ok((cookie, comp)) => {
                    if let Ok(mut inner) = self.inner.try_borrow_mut() {
                        inner.openclose_cookie = cookie;
                        inner.openclose_comp = Some(comp);
                    }
                    tracing::debug!(
                        "ITfCompartmentEventSink(OPENCLOSE) registered cookie={cookie}"
                    );
                }
                Err(e) => tracing::warn!("AdviseSink(CompartmentEventSink) failed: {e}"),
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
            if let Some(dm_ptr) = focused_dm_ptr
                && let Some(mode) = doc_mode_on_focus_change(0, dm_ptr, hwnd_val)
            {
                tracing::info!("Activate: initial mode={mode:?} (config.input.default_mode)");
                // 内部状態と KEYBOARD_OPENCLOSE を同時に揃える
                let tm = self
                    .inner
                    .try_borrow()
                    .ok()
                    .and_then(|i| i.thread_mgr.clone());
                ime_sync::apply(tm.as_ref(), tid, mode, true, "activate");
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
                if inner.threadmgr_cookie != 0
                    && let Ok(src) = tm.cast::<ITfSource>()
                {
                    let _ = src.UnadviseSink(inner.threadmgr_cookie);
                    tracing::debug!("ITfThreadMgrEventSink unregistered");
                }
                // ITfThreadFocusSink 登録解除
                if inner.threadfocus_cookie != 0
                    && let Ok(src) = tm.cast::<ITfSource>()
                {
                    let _ = src.UnadviseSink(inner.threadfocus_cookie);
                    tracing::debug!("ITfThreadFocusSink unregistered");
                }
                // ITfCompartmentEventSink 登録解除（inner は不変借用なので値だけ読む）
                if inner.openclose_cookie != 0
                    && let Some(comp) = inner.openclose_comp.as_ref()
                    && let Ok(src) = comp.cast::<ITfSource>()
                {
                    let _ = src.UnadviseSink(inner.openclose_cookie);
                    tracing::debug!("ITfCompartmentEventSink unregistered");
                }
            }
        }
        // 解除したコンパートメントの参照は落とす（Activate で登録し直される）。
        drop(inner);
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.openclose_cookie = 0;
            inner.openclose_comp = None;
        }
        let _ = composition_set(None);
        if let Ok(mut g) = engine_get()
            && let Some(e) = g.as_mut()
        {
            e.bg_reclaim();
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
        if let Ok(mut g) = engine_get()
            && let Some(e) = g.as_mut()
        {
            e.bg_reclaim();
            e.reset_all();
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
                // 重要キーは、keymap 取得に失敗しても確実に動かす（RefCell 競合対策）。
                // 集合は resolve_action ①.5 と同じ（keymap.rs の essential_fallback_action）。
                match crate::engine::keymap::essential_fallback_action(vk) {
                    Some(a) => a,
                    None => return Ok(FALSE),
                }
            }
        };

        // ロックなし高速チェック: アトミックでモード取得（try_lock 失敗でも正確）
        // コンパートメントは内部状態から導出して書く「通知」であり、真の状態ではない。
        // 起動直後はコンパートメントが 0（オフ）のまま内部が On になる場合があり、
        // コンパートメントを参照すると ImeToggle が逆方向に動くバグを引き起こす。
        // → 内部状態のアトミックのみを正とし、コンパートメントは参照しない。
        if !crate::engine::state::ime_mode_get_atomic().is_on() {
            let eat = matches!(
                action,
                UserAction::ImeToggle | UserAction::ImeOn | UserAction::ImeOff
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

        // ── IME オフガード（最終防衛線）───────────────────────────────────
        // OnTestKeyDown が FALSE を返してもターミナル等が OnKeyDown を直接呼ぶ場合がある。
        // アトミックなのでロック競合なし。
        if !crate::engine::state::ime_mode_get_atomic().is_on() {
            let is_ime_ctrl = matches!(
                action,
                UserAction::ImeToggle | UserAction::ImeOn | UserAction::ImeOff
            );
            if !is_ime_ctrl {
                diag::event(DiagEvent::KeyIgnored {
                    vk,
                    reason: "ime_off",
                });
                return Ok(FALSE);
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

/// `OnTestKeyDown` / `OnKeyDown` 共通の VK 正規化。
///
/// 修飾キーの状態を読んで `keymap::normalize_key_event`（純粋関数）に委ねる。
/// VK の読み替え規則（Ctrl+Alt+Right → Ctrl+Space、JIS 半角/全角 0xF3/0xF4 → 0x19）は
/// keymap.rs 側に集約してあり、ここでは持たない。
fn normalize_key_event_vk(vk: u16) -> u16 {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState;

    let ctrl = unsafe { GetKeyState(0x11) as u16 & 0x8000 != 0 };
    let shift = unsafe { GetKeyState(0x10) as u16 & 0x8000 != 0 };
    let alt = unsafe { GetKeyState(0x12) as u16 & 0x8000 != 0 };
    let space_down = unsafe { GetKeyState(0x20) as u16 & 0x8000 != 0 };

    crate::engine::keymap::normalize_key_event(vk, ctrl, shift, alt, space_down).0
}

// ─── handle_action ───────────────────────────────────────────────────────────

impl TextServiceFactory_Impl {
    fn notify_langbar_update(&self) {
        use windows::Win32::UI::TextServices::TF_LBI_ICON;
        const TF_LBI_TEXT: u32 = 2;
        if let Ok(inner) = self.inner.try_borrow()
            && let Some(sink) = &inner.langbar_sink
        {
            unsafe {
                let _ = sink.OnUpdate(TF_LBI_ICON | TF_LBI_TEXT);
            }
        }
    }

    fn notify_tray_update(&self, tid: u32) {
        let _ = tid;
        tray_ipc::publish(crate::engine::state::ime_mode_get_atomic());
    }

    /// モード切替時にキャレット近くにインジケーターを表示する。
    ///
    /// mozc と同じアプローチで TSF の `GetSelection` → `GetTextExt` を使い
    /// キャレット位置をリアルタイムに取得する。取得できない場合は表示しない。
    fn show_mode_indicator(&self, mode: ImeMode, ctx: ITfContext, tid: u32) {
        use crate::tsf::edit_session::EditSession;
        use crate::tsf::mode_indicator;

        // ラッチが立っていない場合はエンジンに直接問い合わせてラッチを更新する。
        // モード切替はキー入力前にも発生するため、初回切替時にラッチが false のまま
        // 「ー」がカーソル位置に表示されるのを防ぐ。
        if !crate::engine::state::is_conversion_ready()
            && let Ok(g) = crate::engine::state::engine_try_get()
            && let Some(eng) = g.as_ref()
        {
            crate::engine::state::poll_dict_ready_cached(eng);
        }

        let ready = crate::engine::state::is_conversion_ready();
        let mode_char: &'static str = match mode {
            // エンジン準備前は「あ」を出さない（変換できない状態を誤認させないため）
            ImeMode::On if !ready => return,
            m => m.label(),
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

    /// 入力モード切替時の設定再読込。
    ///
    /// config と keymap はそれぞれ mtime gate を持ち、変わっていなければファイルを
    /// 読まない（キースレッド上の同期 I/O を避ける）。両者の失敗は互いに影響しない。
    fn maybe_reload_runtime_config(&self) {
        let config_changed = crate::engine::config::maybe_reload_on_mode_switch();
        if let Some(new_keymap) = crate::engine::keymap::Keymap::reload_if_changed()
            && let Ok(mut inner) = self.inner.try_borrow_mut()
        {
            inner.keymap = new_keymap;
        }
        if config_changed {
            tracing::info!("runtime config reloaded on input mode switch");
            crate::engine::state::engine_reload();
        }
    }
}

// ─── CandidateDir ─────────────────────────────────────────────────────────────

pub(super) enum CandidateDir {
    Next,
    Prev,
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
        if let Ok(mut sess) = session_get()
            && (sess.is_waiting() || sess.is_selecting())
        {
            sess.set_idle();
        }
        p
    };
    if preedit.is_empty() {
        return Ok(());
    }
    tracing::debug!("engine_commit_hiragana: committing preedit={preedit:?}");
    end_composition(ctx, tid, preedit)
}

/// 同期変換 + 辞書マージ。
///
/// - `reading`: 辞書・学習履歴を引くキー（`hiragana_text()` 相当。未確定ローマ字を含まない）
/// - `preedit`: 候補が 1 件も無いときに返す表示文字列
fn engine_convert_sync_multi(
    engine: &mut crate::engine::state::DynEngine,
    llm_limit: usize,
    dict_limit: usize,
    reading: &str,
    preedit: &str,
) -> Vec<String> {
    // LLM候補を取得（llm_limit 件）
    let llm_cands: Vec<String> = engine.convert_sync();
    let _ = llm_limit; // DynEngine::convert_sync は num_candidates を内部設定から読む

    // 辞書候補とマージ（dict_limit 件まで）。reading を明示的に渡す（Issue #9）。
    let merged = engine.merge_candidates_for_reading(reading, llm_cands, dict_limit);
    tracing::debug!("merge_candidates(reading={:?}) → {:?}", reading, merged);
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
        UserAction::ImeToggle | UserAction::ImeOff | UserAction::ImeOn => true,
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
        | UserAction::CursorRight
        // Home / End: 未確定文字列がある間はアプリへ渡さない（Issue #11）。
        // 透過させるとアプリ側が未確定文字列を無視してキャレットを行頭 / 行末へ動かす。
        | UserAction::CursorHome
        | UserAction::CursorEnd => has_preedit,
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
pub(super) fn action_name(a: &UserAction) -> &'static str {
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
        UserAction::CursorHome => "CursorHome",
        UserAction::CursorEnd => "CursorEnd",
        UserAction::Punctuate(_) => "Punctuate",
        UserAction::SegmentShrink => "SegmentShrink",
        UserAction::SegmentExtend => "SegmentExtend",
        UserAction::ImeToggle => "ImeToggle",
        UserAction::ImeOn => "ImeOn",
        UserAction::ImeOff => "ImeOff",
        _ => "Other",
    }
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

        let current_mode = crate::engine::state::ime_mode_get_atomic();

        unsafe {
            let ime_on = "IME オン".encode_utf16().collect::<Vec<_>>();
            let ime_off = "IME オフ".encode_utf16().collect::<Vec<_>>();
            let settings = "設定...".encode_utf16().collect::<Vec<_>>();
            let reload = "エンジン再起動".encode_utf16().collect::<Vec<_>>();

            let _ = menu.AddMenuItem(
                ID_MENU_IME_ON,
                if current_mode == ImeMode::On {
                    TF_LBMENUF_RADIOCHECKED
                } else {
                    0
                },
                HBITMAP::default(),
                HBITMAP::default(),
                &ime_on,
                std::ptr::null_mut(),
            );
            let _ = menu.AddMenuItem(
                ID_MENU_IME_OFF,
                if current_mode == ImeMode::Off {
                    TF_LBMENUF_RADIOCHECKED
                } else {
                    0
                },
                HBITMAP::default(),
                HBITMAP::default(),
                &ime_off,
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
        let mode_char = langbar_mode_char();
        language_bar::create_mode_icon(mode_char)
            .or_else(|_| unsafe { language_bar::load_tray_icon() })
    }
    fn GetText(&self) -> windows::core::Result<BSTR> {
        // トレイは1〜2文字しか表示できないためモード文字のみ返す
        // バックエンド情報は GetTooltipString に集約
        Ok(BSTR::from(langbar_mode_char()))
    }
}

/// 言語バーのアイコン / テキストに使う 1 文字。
///
/// 内部状態（`ime_mode`）だけを見る。コンパートメントは参照しない
/// （コンパートメントは内部状態から導出して書く側であり、読むと片方だけ
/// 更新された瞬間に表示がずれる）。IME オンでエンジン準備前は「ー」。
fn langbar_mode_char() -> &'static str {
    match crate::engine::state::ime_mode_get_atomic() {
        ImeMode::Off => "A",
        ImeMode::On if !crate::engine::state::is_conversion_ready() => "ー",
        ImeMode::On => "あ",
    }
}

// ─── ITfThreadMgrEventSink ────────────────────────────────────────────────────
//
// フォーカスが変わるたびに OnSetFocus が呼ばれる。
// DocumentManager ポインタをキーに IME オン/オフを記憶し、
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

impl ITfCompartmentEventSink_Impl for TextServiceFactory_Impl {
    fn OnChange(&self, rguid: *const GUID) -> windows::core::Result<()> {
        // msctf のコールバック内。ここで msctf に再入しない（OnSetFocus と同じ方針）。
        if !rguid.is_null() && unsafe { *rguid } == GUID_COMPARTMENT_KEYBOARD_OPENCLOSE {
            candidate_window::post_openclose_changed();
        }
        Ok(())
    }
}

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
        if let Ok(sink) = punk.cast::<ITfLangBarItemSink>()
            && let Ok(mut inner) = self.inner.try_borrow_mut()
        {
            inner.langbar_sink = Some(sink);
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

#[cfg(test)]
mod key_should_eat_tests {
    use super::key_should_eat;
    use crate::engine::user_action::UserAction;

    #[test]
    fn home_end_are_eaten_only_while_preedit_exists() {
        for action in [UserAction::CursorHome, UserAction::CursorEnd] {
            assert!(key_should_eat(&action, true), "{action:?} with preedit");
            assert!(
                !key_should_eat(&action, false),
                "{action:?} without preedit"
            );
        }
    }

    #[test]
    fn left_right_keep_the_same_gating_as_home_end() {
        for has_preedit in [true, false] {
            assert_eq!(
                key_should_eat(&UserAction::CursorLeft, has_preedit),
                key_should_eat(&UserAction::CursorHome, has_preedit)
            );
            assert_eq!(
                key_should_eat(&UserAction::CursorRight, has_preedit),
                key_should_eat(&UserAction::CursorEnd, has_preedit)
            );
        }
    }
}
