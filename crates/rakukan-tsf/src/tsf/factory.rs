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
    core::{implement, Interface, IUnknown, BSTR, GUID},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_INVALIDARG, FALSE, LPARAM, POINT, RECT, TRUE, WPARAM},
        System::{
            Com::{IClassFactory, IClassFactory_Impl, CoCreateInstance, CLSCTX_INPROC_SERVER},
            Ole::CONNECT_E_CANNOTCONNECT,
        },
        UI::{
            Input::KeyboardAndMouse::GetKeyState,
            TextServices::{
                CLSID_TF_CategoryMgr,
                IEnumTfDisplayAttributeInfo,
                ITfCategoryMgr,
                ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl,
                ITfContext, ITfDisplayAttributeInfo, ITfDisplayAttributeProvider,
                ITfDisplayAttributeProvider_Impl,
                ITfDocumentMgr, ITfKeyEventSink, ITfKeyEventSink_Impl,
                ITfKeystrokeMgr, ITfLangBarItem, ITfLangBarItem_Impl,
                ITfLangBarItemButton, ITfLangBarItemButton_Impl,
                ITfLangBarItemSink, ITfMenu,
                ITfSource, ITfSource_Impl,
                ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr,
                ITfThreadMgrEventSink, ITfThreadMgrEventSink_Impl,
                TfLBIClick, TF_LANGBARITEMINFO,
                TF_ES_READWRITE, GUID_PROP_ATTRIBUTE,
            },
            WindowsAndMessaging::{GetForegroundWindow, HICON},
        },
    },
};

use crate::{
    diagnostics::{self as diag, DiagEvent},
    engine::{
        keymap::Keymap,
        state::{
            composition_clone, composition_set, composition_take,
            engine_try_get_or_create, engine_get,
            session_get, session_is_selecting_fast,
            caret_rect_get, caret_rect_set,
            SessionState,
            doc_mode_on_focus_change, doc_mode_remove,
        },
        user_action::UserAction,
        text_util,
    },
    globals::{GUID_DISPLAY_ATTRIBUTE, GUID_DISPLAY_ATTRIBUTE_INPUT},
    tsf::{
        candidate_window,
        display_attr,
        edit_session::EditSession,
        language_bar::{self, toggle_open_close, get_open_close, LANGBAR_SINK_COOKIE},
        tray_ipc,
    },
};

// ─── TextServiceState ─────────────────────────────────────────────────────────

pub struct TextServiceState {
    pub client_id:       u32,
    pub thread_mgr:      Option<ITfThreadMgr>,
    pub keymap:          Keymap,
    pub langbar_sink:    Option<ITfLangBarItemSink>,
    /// ITfThreadMgrEventSink の登録クッキー（Deactivate で解除）
    pub threadmgr_cookie: u32,
}

impl Default for TextServiceState {
    fn default() -> Self {
        Self {
            client_id: 0,
            thread_mgr: None,
            keymap: Keymap::default(),
            langbar_sink: None,
            threadmgr_cookie: 0,
        }
    }
}

// Safety: TSF は STA。RefCell + COM オブジェクトを持つが
// OnKeyDown は必ず STA スレッドから呼ばれる。
// windows-rs の #[implement] が要求するため付ける。
unsafe impl Send for TextServiceState {}

// ─── TextServiceFactory ───────────────────────────────────────────────────────

#[implement(IClassFactory, ITfTextInputProcessor, ITfKeyEventSink, ITfCompositionSink,
            ITfLangBarItemButton, ITfLangBarItem, ITfSource, ITfThreadMgrEventSink,
            ITfDisplayAttributeProvider)]
pub struct TextServiceFactory {
    pub inner: RefCell<TextServiceState>,
}

unsafe impl Send for TextServiceFactory {}
unsafe impl Sync for TextServiceFactory {}

impl TextServiceFactory {
    pub fn new() -> Self {
        Self { inner: RefCell::new(TextServiceState::default()) }
    }
}

// ─── IClassFactory ────────────────────────────────────────────────────────────

impl IClassFactory_Impl for TextServiceFactory_Impl {
    fn CreateInstance(
        &self, punkouter: Option<&IUnknown>,
        riid: *const GUID, ppvobject: *mut *mut core::ffi::c_void,
    ) -> windows::core::Result<()> {
        if punkouter.is_some() {
            return Err(windows::core::Error::new(E_FAIL, "no aggregation"));
        }
        let svc = TextServiceFactory::new();
        let itp: ITfTextInputProcessor = svc.into();
        let unk: IUnknown = itp.cast()?;
        unsafe { unk.query(riid, ppvobject).ok() }
    }
    fn LockServer(&self, _: BOOL) -> windows::core::Result<()> { Ok(()) }
}

// ─── ITfTextInputProcessor ───────────────────────────────────────────────────

impl ITfTextInputProcessor_Impl for TextServiceFactory_Impl {
    fn Activate(&self, ptim: Option<&ITfThreadMgr>, tid: u32) -> windows::core::Result<()> {
        let _t = diag::span("Activate");
        let tm = ptim.ok_or_else(|| windows::core::Error::new(E_FAIL, "null thread_mgr"))?;

        {
            let mut inner = self.inner.try_borrow_mut()
                .map_err(|_| windows::core::Error::new(E_FAIL, "borrow_mut"))?;
            inner.client_id  = tid;
            inner.thread_mgr = Some(tm.clone());
            inner.keymap     = Keymap::load();
        }

        // エンジン非同期初期化
        // DLL ロードは重い（CUDA 初期化で数秒かかる）ため、バックグラウンドスレッドで行う。
        // UIスレッドをブロックしないことでアプリのハングを防ぐ。
        // エンジン準備完了後に langbar_update_take() が true になり、
        // 次回 OnKeyDown で言語バーが更新される。
        crate::engine::state::engine_start_bg_init();

        // KeyEventSink 登録
        unsafe {
            let km: ITfKeystrokeMgr = tm.cast()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("cast KeystrokeMgr: {e}")))?;
            let ks: ITfKeyEventSink = self.cast()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("cast KeyEventSink: {e}")))?;
            km.AdviseKeyEventSink(tid, &ks, TRUE)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("AdviseKeyEventSink: {e}")))?;
        }

        // 言語バー登録
        unsafe {
            if let Ok(btn) = self.cast::<ITfLangBarItemButton>() {
                let ok = language_bar::langbar_add(tm, &btn).is_ok();
                diag::event(DiagEvent::LangbarAdd { ok, err: if ok { None } else { Some("see log".into()) } });
                if !ok { tracing::warn!("langbar_add failed"); }
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
                    tracing::info!("KEYBOARD_OPENCLOSE = {} ({})", is_open as u8, if is_open { "on" } else { "off" });
                    true
                }
                Err(e) => { tracing::warn!("set_open_close FAILED: {e}"); false }
            };
            diag::event(DiagEvent::CompartmentSet { open: is_open, ok, err: None });
        }

        // トレイ常駐プロセスへ現在モードを通知（失敗してもIMEは継続）
        {
            let mode = crate::engine::state::ime_state_get().ok()
                .map(|s| s.input_mode)
                .unwrap_or_default();
            tray_ipc::publish(true, mode);
        }

        // ITfThreadMgrEventSink を登録してフォーカス変化を受け取る
        unsafe {
            if let Ok(src) = tm.cast::<ITfSource>() {
                let sink: ITfThreadMgrEventSink = self.cast()
                    .map_err(|e| windows::core::Error::new(E_FAIL, format!("cast ThreadMgrEventSink: {e}")))?;
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

        diag::event(DiagEvent::Activate { tid });
        tracing::info!("rakukan Activate client_id={tid}");

        // Display Attribute GUIDs を ITfCategoryMgr に登録して atom を取得
        unsafe {
            if let Ok(catmgr) = CoCreateInstance::<_, ITfCategoryMgr>(
                &CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER,
            ) {
                let atom_input = catmgr.RegisterGUID(&GUID_DISPLAY_ATTRIBUTE_INPUT)
                    .unwrap_or(0);
                let atom_conv  = catmgr.RegisterGUID(&GUID_DISPLAY_ATTRIBUTE)
                    .unwrap_or(0);
                display_attr::set_atoms(atom_input, atom_conv);
                tracing::debug!("display attr atoms: input={atom_input} conv={atom_conv}");
            }
        }

        Ok(())
    }

    fn Deactivate(&self) -> windows::core::Result<()> {
        diag::event(DiagEvent::Deactivate);
        let inner = self.inner.try_borrow()
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
            }
        }
        let _ = composition_set(None);
        if let Ok(mut g) = engine_get() { if let Some(e) = g.as_mut() { e.bg_reclaim(); } }
        candidate_window::destroy();
        if let Ok(mut sess) = session_get() { sess.set_idle(); }
        tracing::info!("rakukan Deactivate");
        Ok(())
    }
}

// ─── ITfCompositionSink ──────────────────────────────────────────────────────

impl ITfCompositionSink_Impl for TextServiceFactory_Impl {
    fn OnCompositionTerminated(
        &self, _: u32, _: Option<&ITfComposition>,
    ) -> windows::core::Result<()> {
        let _ = composition_set(None);
        // 候補ウィンドウと選択状態をクリア
        candidate_window::hide();
        if let Ok(mut sess) = session_get() { sess.set_idle(); }
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
    fn OnSetFocus(&self, _: BOOL) -> windows::core::Result<()> { Ok(()) }

    fn OnTestKeyDown(
        &self, _: Option<&ITfContext>, wparam: WPARAM, _: LPARAM,
    ) -> windows::core::Result<BOOL> {
        let vk = wparam.0 as u16;
        let action = match self.inner.try_borrow().ok()
            .and_then(|g| g.keymap.resolve_action(vk))
        {
            Some(a) => a,
            None    => {
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
            let eat = matches!(action,
                UserAction::ImeToggle | UserAction::ImeOn | UserAction::ImeOff |
                UserAction::ModeHiragana | UserAction::ModeKatakana | UserAction::ModeAlphanumeric);
            return Ok(if eat { TRUE } else { FALSE });
        }

        let has_preedit = engine_try_get_or_create()
            .ok()
            .and_then(|g| g.as_ref().map(|e| !e.preedit_is_empty()))
            .unwrap_or(false);

        // 選択モード中はプリエディットありと同じ扱い（候補操作キーを消費するため）
        // AtomicBool でロックなし高速チェック
        let is_selecting = session_is_selecting_fast();

        Ok(if key_should_eat(&action, has_preedit || is_selecting) { TRUE } else { FALSE })
    }

    fn OnKeyDown(
        &self, pic: Option<&ITfContext>, wparam: WPARAM, _: LPARAM,
    ) -> windows::core::Result<BOOL> {
        // バックエンド初期化完了フラグを確認して言語バー表示を更新
        if crate::engine::state::langbar_update_take() {
            self.notify_langbar_update();
        }
        let _t = diag::span("OnKeyDown");
        let vk = wparam.0 as u16;

        tracing::debug!("OnKeyDown vk={:#04x}", vk);

        // Ctrl+Shift+F12: 診断ダンプ
        if vk == 0x7B {
            let ctrl  = unsafe { GetKeyState(0x11) as u16 & 0x8000 != 0 };
            let shift = unsafe { GetKeyState(0x10) as u16 & 0x8000 != 0 };
            if ctrl && shift {
                diag::dump_snapshot();
                return Ok(TRUE);
            }
        }

        let action = match self.inner.try_borrow().ok()
            .and_then(|g| g.keymap.resolve_action(vk))
        {
            Some(a) => a,
            None    => {
                tracing::debug!("OnKeyDown vk={:#04x} → unmapped (try_borrow={:?})", vk, self.inner.try_borrow().is_ok());
                diag::event(DiagEvent::KeyIgnored { vk, reason: "unmapped" });
                return Ok(FALSE);
            }
        };
        let ctx = match pic {
            Some(c) => c.clone(),
            None    => {
                diag::event(DiagEvent::KeyIgnored { vk, reason: "no_ctx" });
                return Ok(FALSE);
            }
        };
        let tid  = self.inner.try_borrow().map(|g| g.client_id).unwrap_or(0);
        let sink: ITfCompositionSink = match unsafe { self.cast() } {
            Ok(s)  => s,
            Err(_) => {
                diag::event(DiagEvent::KeyIgnored { vk, reason: "no_sink" });
                return Ok(FALSE);
            }
        };

        tracing::trace!("OnKeyDown vk={vk:#04x} action={action:?}");

        // ── 英数モードガード（最終防衛線）─────────────────────────────────
        // OnTestKeyDown が FALSE を返してもターミナル等が OnKeyDown を直接呼ぶ場合がある。
        // アトミックなのでロック競合なし。
        {
            use crate::engine::input_mode::InputMode;
            if crate::engine::state::input_mode_get_atomic() == InputMode::Alphanumeric {
                let is_ime_ctrl = matches!(action,
                    UserAction::ImeToggle | UserAction::ImeOn | UserAction::ImeOff |
                    UserAction::ModeHiragana | UserAction::ModeKatakana | UserAction::ModeAlphanumeric);
                if !is_ime_ctrl {
                    diag::event(DiagEvent::KeyIgnored { vk, reason: "alphanumeric_mode" });
                    return Ok(FALSE);
                }
            }
        }

        match self.handle_action(action.clone(), ctx, tid, sink) {
            Ok(ate) => {
                diag::event(DiagEvent::KeyHandled { vk, action: action_name(&action), ate });
                Ok(if ate { TRUE } else { FALSE })
            }
            Err(e) => {
                diag::event(DiagEvent::Error { site: "handle_action", msg: e.to_string() });
                tracing::warn!("handle_action: {e}");
                Ok(FALSE)
            }
        }
    }

    fn OnTestKeyUp(&self, _:Option<&ITfContext>, _:WPARAM, _:LPARAM) -> windows::core::Result<BOOL> { Ok(FALSE) }
    fn OnKeyUp    (&self, _:Option<&ITfContext>, _:WPARAM, _:LPARAM) -> windows::core::Result<BOOL> { Ok(FALSE) }
    fn OnPreservedKey(&self, _:Option<&ITfContext>, _:*const GUID)   -> windows::core::Result<BOOL> { Ok(FALSE) }
}

// ─── handle_action ───────────────────────────────────────────────────────────

impl TextServiceFactory_Impl {
    fn notify_langbar_update(&self) {
        use windows::Win32::UI::TextServices::TF_LBI_ICON;
        const TF_LBI_TEXT: u32 = 2;
        if let Ok(inner) = self.inner.try_borrow() {
            if let Some(sink) = &inner.langbar_sink {
                unsafe { let _ = sink.OnUpdate(TF_LBI_ICON | TF_LBI_TEXT); }
            }
        }
    }

    fn notify_tray_update(&self, tid: u32) {
        let open = self.inner.try_borrow().ok()
            .and_then(|i| i.thread_mgr.clone().map(|tm| get_open_close(&tm)))
            .unwrap_or_else(|| {
                crate::engine::state::ime_state_get().ok()
                    .map(|s| s.input_mode != crate::engine::input_mode::InputMode::Alphanumeric)
                    .unwrap_or(true)
            });
        let mode = crate::engine::state::ime_state_get().ok()
            .map(|s| s.input_mode)
            .unwrap_or_default();
        let _ = tid;
        tray_ipc::publish(open, mode);
    }

    fn maybe_reload_runtime_config(&self) {
        let config_changed = crate::engine::config::maybe_reload_on_mode_switch();
        let new_keymap = crate::engine::keymap::Keymap::load();
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.keymap = new_keymap;
        }
        if config_changed {
            tracing::info!("runtime config reloaded on input mode switch");
        }
    }

    fn handle_action(
        &self, action: UserAction,
        ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
    ) -> Result<bool> {
        let mut guard = engine_try_get_or_create()?;
        let engine = match guard.as_mut() {
            Some(e) => e,
            None    => return Ok(false),
        };

        // ── 診断: 全アクションの入口でセッション状態とBG状態をログ ──
        {
            let bg = engine.bg_status();
            let state_name = if let Ok(s) = session_get() {
                match &*s {
                    SessionState::Idle                => "Idle".to_string(),
                    SessionState::Preedit { text }    => format!("Preedit({:?})", text),
                    SessionState::Waiting { text, .. } => format!("Waiting({:?})", text),
                    SessionState::Selecting { original_preedit, llm_pending, candidates, .. }
                        => format!("Selecting(op={:?} llm={} nc={})", original_preedit, llm_pending, candidates.len()),
                    SessionState::SplitPreedit { target, remainder }
                        => format!("Split({:?}+{:?})", target, remainder),
                }
            } else { "lock_err".to_string() };
            tracing::info!("handle_action: {:?} state={} bg={} hira={:?}",
                action_name(&action), state_name, bg, engine.hiragana_text());
        }

        // LLM候補待機中に完了した場合、候補ウィンドウを自動更新
        if session_is_selecting_fast() {
            const DICT_LIMIT_POLL: usize = 20;
            if let Ok(mut sess) = session_get() {
                let poll_info = if let SessionState::Selecting { ref original_preedit, llm_pending, .. } = *sess {
                    if llm_pending && engine.bg_status() == "done" { Some(original_preedit.clone()) } else { None }
                } else { None };
                if let Some(preedit_key) = poll_info {
                    tracing::info!("poll: bg=done llm_pending=true key={:?}, calling bg_take_candidates", preedit_key);
                    match engine.bg_take_candidates(&preedit_key) {
                        Some(llm_cands) => {
                            tracing::info!("poll: bg_take_candidates → Some({} cands)", llm_cands.len());
                            let merged = engine.merge_candidates(llm_cands, DICT_LIMIT_POLL);
                            if !merged.is_empty() {
                                let first = merged.first().cloned().unwrap_or_default();
                                if let SessionState::Selecting { ref mut candidates, ref mut selected, ref mut llm_pending, .. } = *sess {
                                    *candidates  = merged;
                                    *selected    = 0;
                                    *llm_pending = false;
                                }
                                let page_cands = sess.page_candidates().to_vec();
                                let page_info  = sess.page_info();
                                let remainder  = sess.selecting_remainder_clone();
                                let pos = caret_rect_get();
                                drop(sess);
                                drop(guard);
                                candidate_window::show_with_status(&page_cands, 0, &page_info, pos.left, pos.bottom, None);
                                update_composition_candidate_split(ctx, tid, sink, first, remainder)?;
                                return Ok(true);
                            }
                        }
                        None => {
                            // take_ready がキー不一致で None を返した: Done 状態は保持されたまま
                            // llm_pending はそのままにしておく（次のキー/Space で再試行できる）
                            tracing::warn!("poll: bg_take_candidates → None (key mismatch or lock busy), bg={}", engine.bg_status());
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
            const DICT_LIMIT_WAIT: usize = 20;
            if let Ok(mut sess) = session_get() {
                if let Some((wait_preedit, pos_x, pos_y)) = sess.waiting_info().map(|(t, x, y)| (t.to_string(), x, y)) {
                    let bg_now = engine.bg_status();
                    tracing::info!("waiting-poll: wait_preedit={:?} bg={}", wait_preedit, bg_now);
                    if bg_now == "done" {
                        tracing::info!("waiting-poll: calling bg_take_candidates({:?})", wait_preedit);
                        match engine.bg_take_candidates(&wait_preedit) {
                            Some(llm_cands) => {
                                tracing::info!("waiting-poll: got {} LLM cands", llm_cands.len());
                                // LLM候補とマージ。llm_cands が空でも辞書候補がある場合はそちらを使う。
                                let merged = if llm_cands.is_empty() {
                                    engine.merge_candidates(vec![], DICT_LIMIT_WAIT)
                                } else {
                                    engine.merge_candidates(llm_cands, DICT_LIMIT_WAIT)
                                };
                                tracing::info!("waiting-poll: merged={} cands", merged.len());
                                // preedit 1件だけでも候補ウィンドウを出す（辞書/LLMどちらかにヒットした）
                                if !merged.is_empty() {
                                    let first = merged.first().cloned().unwrap_or_default();
                                    sess.activate_selecting(merged, wait_preedit.clone(), pos_x, pos_y, false);
                                    let page_cands = sess.page_candidates().to_vec();
                                    let page_info  = sess.page_info();
                                    drop(sess);
                                    drop(guard);
                                    candidate_window::stop_waiting_timer();
                                    candidate_window::show_with_status(&page_cands, 0, &page_info, pos_x, pos_y, None);
                                    update_composition(ctx, tid, sink, first)?;
                                    return Ok(true);
                                }
                            }
                              None => {
                                // キー不一致 or ロック競合 → Done 状態は保持されたまま
                                // Waiting 状態を維持して次のキー/Space で再試行
                                tracing::warn!("waiting-poll: bg_take_candidates → None (key mismatch?), bg={}", engine.bg_status());
                            }
                        }
                        // merged が空（LLM候補なし）だった場合のみ preedit に戻す
                        // None だった場合は Waiting を維持（→ Cancel や次のSpace で対処）
                    }
                }
            }
        } // if !is_cancel

        match action {
            UserAction::Input(c)                       => self.on_input(c, ctx, tid, sink, guard),
            UserAction::InputRaw(c)                    => self.on_input_raw(c, ctx, tid, sink, guard),
            UserAction::FullWidthSpace                 => self.on_full_width_space(ctx, tid, guard),
            UserAction::Convert                        => self.on_convert(ctx, tid, sink, guard),
            UserAction::CommitRaw                      => self.on_commit_raw(ctx, tid, guard),
            UserAction::Backspace                      => self.on_backspace(ctx, tid, sink, guard),
            UserAction::Cancel | UserAction::CancelAll => self.on_cancel(ctx, tid, sink, guard),
            UserAction::Hiragana                       => self.on_kana_convert(ctx, tid, sink, guard, text_util::to_hiragana),
            UserAction::Katakana                       => self.on_kana_convert(ctx, tid, sink, guard, text_util::to_katakana),
            UserAction::HalfKatakana                   => self.on_kana_convert(ctx, tid, sink, guard, text_util::to_half_katakana),
            UserAction::FullLatin                      => self.on_latin_convert(ctx, tid, sink, guard, true),
            UserAction::HalfLatin                      => self.on_latin_convert(ctx, tid, sink, guard, false),
            UserAction::CycleKana                      => self.on_cycle_kana(ctx, tid, guard),
            UserAction::CandidateNext                  => self.on_candidate_move(ctx, tid, sink, guard, CandidateDir::Next),
            UserAction::CandidatePrev                  => self.on_candidate_move(ctx, tid, sink, guard, CandidateDir::Prev),
            UserAction::CandidatePageDown              => self.on_candidate_page(ctx, tid, sink, guard, CandidateDir::Next),
            UserAction::CandidatePageUp                => self.on_candidate_page(ctx, tid, sink, guard, CandidateDir::Prev),
            UserAction::CandidateSelect(n)             => self.on_candidate_select(n, ctx, tid, guard),
            UserAction::CursorLeft | UserAction::CursorRight => {
                let e = guard.as_ref().map(|e| !e.preedit_is_empty()).unwrap_or(false);
                Ok(e)
            },
            UserAction::Punctuate(c)                   => self.on_punctuate(c, ctx, tid, sink, guard),
            UserAction::SegmentShrink                  => self.on_segment_shrink(ctx, tid, sink, guard),
            UserAction::SegmentExtend                  => self.on_segment_extend(ctx, tid, sink, guard),
            UserAction::ImeToggle                      => { drop(guard); self.on_ime_toggle(ctx, tid) },
            UserAction::ImeOff | UserAction::ModeAlphanumeric => { drop(guard); self.on_ime_off(ctx, tid) },
            UserAction::ImeOn                          => { drop(guard); self.on_ime_on(ctx, tid) },
            UserAction::ModeHiragana                   => self.on_mode_hiragana(ctx, tid, guard),
            UserAction::ModeKatakana                   => self.on_mode_katakana(ctx, tid, guard),
            _                                          => Ok(false),
        }
    }
}

// ─── CandidateDir ─────────────────────────────────────────────────────────────

enum CandidateDir { Next, Prev }

// ─── アクション実装（impl TextServiceFactory_Impl）────────────────────────────

impl TextServiceFactory_Impl {
    fn on_input(
        &self, c: char,
        ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(true) };
        let _t = diag::span("Input");

        if let Ok(mut sess) = session_get() {
            if sess.is_waiting() {
                let pre = sess.preedit_text().unwrap_or("").to_string();
                sess.set_preedit(pre);
                candidate_window::hide();
            }
        }

        if session_is_selecting_fast() {
            let mut sess = session_get()?;
            if sess.is_selecting() {
                let committed_text = sess.current_candidate()
                    .or_else(|| sess.original_preedit())
                    .unwrap_or("")
                    .to_string();
                sess.set_idle();
                drop(sess);
                candidate_window::hide();
                engine.commit(&committed_text);
                engine.reset_preedit();
                drop(guard);

                let mut guard2 = engine_try_get_or_create()?;
                let engine2 = match guard2.as_mut() { Some(e) => e, None => return Ok(true) };
                engine2.push_char(c);
                let preedit  = engine2.preedit_display();
                let n_cands2 = crate::engine::state::get_num_candidates();
                diag::event(DiagEvent::InputChar { ch: c, preedit_after: preedit.clone() });
                engine2.bg_start(n_cands2);
                drop(guard2);
                commit_then_start_composition(ctx, tid, sink, committed_text, preedit)?;
                return Ok(true);
            }
        }
        // SESSION_SELECTING=true だったが is_selecting()=false の場合はここに来る

        engine.poll_dict_ready();
        engine.poll_model_ready();
        engine.push_char(c);
        let preedit  = engine.preedit_display();
        let hiragana = engine.hiragana_text();
        let n_cands  = crate::engine::state::get_num_candidates();
        diag::event(DiagEvent::InputChar { ch: c, preedit_after: preedit.clone() });
        tracing::trace!("Input: hiragana={:?} bg={}", hiragana, engine.bg_status());

        if !hiragana.is_empty() {
            engine.bg_start(n_cands);
            drop(guard);
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
        &self, c: char, ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
        engine.push_raw(c);
        let preedit = engine.preedit_display();
        let n_cands = crate::engine::state::get_num_candidates();
        engine.bg_start(n_cands);
        drop(guard);
        update_composition(ctx, tid, sink, preedit)?;
        Ok(true)
    }

    fn on_full_width_space(
        &self, ctx: ITfContext, tid: u32,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
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
        &self, ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
        let _t = diag::span("Convert");
        update_caret_rect(ctx.clone(), tid);
        engine.flush_pending_n();
        let preedit_empty = engine.preedit_is_empty();
        if let Ok(sess) = session_get() {
            tracing::debug!("on_convert: preedit_empty={} is_split={} is_selecting={} state={:?}",
                preedit_empty, sess.is_split_preedit(), sess.is_selecting(), &*sess);
        }
        if preedit_empty { return Ok(false); }

        // ── SplitPreedit 中: target のみを変換対象にして候補表示 ──
        {
            let sess = session_get()?;
            if sess.is_split_preedit() {
                let target    = sess.split_target().unwrap_or("").to_string();
                let remainder = sess.split_remainder().unwrap_or("").to_string();
                drop(sess);
                return convert_split_target(ctx, tid, sink, guard, target, remainder);
            }
        }

        let preedit = engine.preedit_display();

        // すでに選択モード中 → 1候補ずつ進む
        {
            let mut sess = session_get()?;
            if sess.is_selecting() {
                // llm_pending=true の場合はLLM完了を確認して候補を更新
                let llm_pending = matches!(*sess, SessionState::Selecting { llm_pending: true, .. });
                if llm_pending {
                    let original_preedit = if let SessionState::Selecting { ref original_preedit, .. } = *sess {
                        original_preedit.clone()
                    } else { String::new() };
                    drop(sess);

                    // 非ブロッキングでLLM完了を確認（最大500ms待機）
                    const WAIT_MS: u64 = 500;
                    let bg_before = engine.bg_status();
                    tracing::info!("on_convert[llm_pending]: key={:?} bg={} → wait_ms({})", original_preedit, bg_before, WAIT_MS);
                    if engine.bg_status() == "running" {
                        engine.bg_wait_ms(WAIT_MS);
                    }
                    engine.poll_model_ready();

                    let bg_done = engine.bg_status() == "done";
                    tracing::info!("on_convert[llm_pending]: after wait bg_done={}", bg_done);
                    const DICT_LIMIT: usize = 20;

                    if bg_done {
                        // LLM完了 → 候補をマージして表示
                        // hiragana_text() でキャッシュの実際のキーを確認してから呼ぶ
                        let hira_key = engine.hiragana_text();
                        tracing::info!("on_convert[llm_pending]: calling bg_take_candidates op={:?}({}) hira={:?}({})",
                            original_preedit, original_preedit.len(), hira_key, hira_key.len());
                        // op と hira が一致する方をキーとして使う（バイト数も確認）
                        let take_key = if hira_key == original_preedit {
                            original_preedit.clone()
                        } else {
                            tracing::warn!("on_convert[llm_pending]: op/hira differ, using hira={:?}", hira_key);
                            hira_key
                        };
                        match engine.bg_take_candidates(&take_key) {
                            Some(llm_cands) => {
                                tracing::info!("on_convert[llm_pending]: bg_take_candidates → Some({} cands)", llm_cands.len());
                                let merged = engine.merge_candidates(llm_cands, DICT_LIMIT);
                                tracing::info!("on_convert[llm_pending]: merged={} cands", merged.len());
                                if !merged.is_empty() {
                                    if let Ok(mut sess2) = session_get() {
                                        if let SessionState::Selecting { ref mut candidates, ref mut selected, ref mut llm_pending, .. } = *sess2 {
                                            *candidates  = merged;
                                            *selected    = 0;
                                            *llm_pending = false;
                                        }
                                        let page_cands = sess2.page_candidates().to_vec();
                                        let page_info  = sess2.page_info();
                                        let cand_text  = sess2.current_candidate()
                                            .or_else(|| sess2.original_preedit())
                                            .unwrap_or("").to_string();
                                        let remainder  = sess2.selecting_remainder_clone();
                                        let pos = caret_rect_get();
                                        drop(sess2);
                                        drop(guard);
                                        candidate_window::show(&page_cands, 0, &page_info, pos.left, pos.bottom);
                                        update_composition_candidate_split(ctx, tid, sink, cand_text, remainder)?;
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
                                    take_key, take_key.len(), bg_now
                                );
                                engine.bg_reclaim();
                                // bg_start で正しいキーで即再変換 → その場で待機 → 1回のSpace押しで候補取得
                                let llm_limit2 = crate::engine::state::get_num_candidates();
                                if engine.bg_start(llm_limit2) {
                                    tracing::info!("on_convert[llm_pending]: bg_start restarted for key={:?}, waiting inline", take_key);
                                    // ここで最大 1500ms 待つ（ユーザーは1回のSpaceで候補を得られる）
                                    const RESTART_WAIT_MS: u64 = 1500;
                                    engine.bg_wait_ms(RESTART_WAIT_MS);
                                    tracing::info!("on_convert[llm_pending]: inline wait done, bg={}", engine.bg_status());
                                } else {
                                    tracing::error!("on_convert[llm_pending]: bg_start also failed (kanji_ready={})", engine.is_kanji_ready());
                                }
                                if let Some(llm_cands) = engine.bg_take_candidates(&take_key) {
                                    tracing::info!("on_convert[llm_pending]: reclaim+retry → Some({} cands)", llm_cands.len());
                                    let merged = engine.merge_candidates(llm_cands, DICT_LIMIT);
                                    if !merged.is_empty() {
                                        if let Ok(mut sess2) = session_get() {
                                            if let SessionState::Selecting { ref mut candidates, ref mut selected, ref mut llm_pending, .. } = *sess2 {
                                                *candidates  = merged;
                                                *selected    = 0;
                                                *llm_pending = false;
                                            }
                                            let page_cands = sess2.page_candidates().to_vec();
                                            let page_info  = sess2.page_info();
                                            let cand_text  = sess2.current_candidate()
                                                .or_else(|| sess2.original_preedit())
                                                .unwrap_or("").to_string();
                                            let remainder  = sess2.selecting_remainder_clone();
                                            let pos = caret_rect_get();
                                            drop(sess2);
                                            drop(guard);
                                            candidate_window::show(&page_cands, 0, &page_info, pos.left, pos.bottom);
                                            update_composition_candidate_split(ctx, tid, sink, cand_text, remainder)?;
                                            return Ok(true);
                                        }
                                    }
                                } else {
                                    tracing::error!("on_convert[llm_pending]: retry also failed, bg={}", engine.bg_status());
                                }
                            }
                        }
                    } else {
                        // まだ変換中 → 現在の候補ウィンドウをそのまま維持
                        if let Ok(sess2) = session_get() {
                            let page_cands = sess2.page_candidates().to_vec();
                            let page_info  = sess2.page_info();
                            let pos = caret_rect_get();
                            drop(sess2);
                            drop(guard);
                            candidate_window::show_with_status(
                                &page_cands, 0, &page_info, pos.left, pos.bottom,
                                Some("⏳ 変換中...")
                            );
                            return Ok(true);
                        }
                    }
                    return Ok(true);
                }

                sess.next_with_page_wrap();
                let page_cands = sess.page_candidates().to_vec();
                let page_sel   = sess.page_selected();
                let page_info  = sess.page_info();
                let cand_text  = sess.current_candidate()
                    .or_else(|| sess.original_preedit())
                    .unwrap_or("")
                    .to_string();
                let remainder  = sess.selecting_remainder_clone();
                drop(sess);
                drop(guard);
                candidate_window::update_selection(page_sel, &page_info);
                candidate_window::show(&page_cands, page_sel, &page_info,
                    caret_rect_get().left, caret_rect_get().bottom);
                update_composition_candidate_split(ctx, tid, sink, cand_text, remainder)?;
                return Ok(true);
            }
        }

        // 新規変換
        let llm_limit   = crate::engine::state::get_num_candidates();
        const DICT_LIMIT: usize = 20;
        engine.poll_dict_ready();
        engine.poll_model_ready();
        // Done 状態の converter を先に回収する。
        // bg_take_candidates がキー不一致で None を返した場合、converter は Done に残ったまま
        // engine.kanji=None になる。is_kanji_ready() チェックより前に reclaim しないと
        // bg_start が永遠にスキップされ Waiting から抜け出せなくなる。
        engine.bg_reclaim();
        let kanji_ready = engine.is_kanji_ready();
        tracing::info!("on_convert[new]: preedit={:?} hira={:?} kanji_ready={} bg={}",
            preedit, engine.hiragana_text(), kanji_ready, engine.bg_status());
        if kanji_ready && engine.bg_status() == "idle" {
            tracing::info!("on_convert: model ready → bg_start");
            engine.bg_start(llm_limit);
        }
        if !kanji_ready {
            let err = engine.last_error();
            tracing::info!("on_convert: kanji not ready, engine status={:?}", err);
        }

        let bg_status  = engine.bg_status();
        let bg_running = !kanji_ready || bg_status == "running" || bg_status == "idle";
        tracing::info!("on_convert[new]: bg_running={} bg={}", bg_running, bg_status);

        // LLM が実行中なら完了まで最大 LLM_WAIT_MAX_MS 待機して候補を取得する。
        // TSF スレッドで待つため UI がブロックするが、完了後に composition text まで
        // 一気に更新できる（WM_TIMER 経由では composition text を更新できないため）。
        // 文字数に応じてタイムアウトを伸ばす（長文は推論に時間がかかる）。
        // 基本 3 秒 + 1 文字あたり 300ms、上限 15 秒。
        let char_count = preedit.chars().count() as u64;
        let LLM_WAIT_MAX_MS: u64 = (3000 + char_count * 300).min(15_000);
        tracing::info!("on_convert[new]: LLM_WAIT_MAX_MS={LLM_WAIT_MAX_MS}ms (chars={char_count})");
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
            candidate_window::show_with_status(&dummy, 0, "", caret.left, caret.bottom, Some("⏳ 変換中..."));
            // LLM完了を待機
            let completed = engine.bg_wait_ms(LLM_WAIT_MAX_MS);
            tracing::info!("on_convert[new]: bg_wait({LLM_WAIT_MAX_MS}ms) completed={completed}");
            if !completed {
                // タイムアウト → WM_TIMER に任せて return
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
            candidate_window::show_with_status(&dummy, 0, "", caret.left, caret.bottom, Some("⏳ 変換中..."));
            tracing::info!("on_convert[new]: kanji_ready=false bg=running → wait for prev bg to finish");
            let completed = engine.bg_wait_ms(LLM_WAIT_MAX_MS);
            tracing::info!("on_convert[new]: prev bg wait completed={completed}");
            // 前の bg が完了したら converter を回収して新しいキーで再起動
            engine.bg_reclaim();
            let kanji_ready2 = engine.is_kanji_ready();
            tracing::info!("on_convert[new]: after reclaim kanji_ready={kanji_ready2}");
            if kanji_ready2 {
                engine.bg_start(llm_limit);
                let completed2 = engine.bg_wait_ms(LLM_WAIT_MAX_MS);
                tracing::info!("on_convert[new]: new bg wait completed={completed2}");
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
        tracing::info!("on_convert[new]: post-wait hiragana_key={:?} bg={} kanji_ready={}", hiragana_key2, bg_status2, kanji_ready_now);
        // キー不一致で None が返ると Done が復元されるので、両方試した後に reclaim しておく
        let bg_cands = engine.bg_take_candidates(&hiragana_key2)
            .or_else(|| {
                if preedit != hiragana_key2 {
                    tracing::debug!("Convert: hira key miss, retry preedit={:?}", preedit);
                    engine.bg_take_candidates(&preedit)
                } else { None }
            });
        // いずれも None だった場合 → bg_reclaim + bg_start でブロッキング再試行
        let bg_cands = if bg_cands.is_none() && kanji_ready_now {
            tracing::warn!("Convert: bg_take_candidates None (hira={:?} preedit={:?}) → reclaim+restart", hiragana_key2, preedit);
            engine.bg_reclaim();
            if engine.is_kanji_ready() {
                engine.bg_start(llm_limit);
                let completed3 = engine.bg_wait_ms(LLM_WAIT_MAX_MS);
                tracing::info!("Convert: retry bg_wait completed={completed3}");
                let hira3 = engine.hiragana_text().to_string();
                engine.bg_take_candidates(&hira3)
                    .or_else(|| {
                        if preedit != hira3 { engine.bg_take_candidates(&preedit) } else { None }
                    })
                    .inspect(|c| tracing::info!("Convert: retry got {} cands", c.len()))
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
                let merged = engine.merge_candidates(llm_cands, DICT_LIMIT);
                if merged.is_empty() || (merged.len() == 1 && merged[0] == preedit) {
                    if kanji_ready_now {
                        (engine_convert_sync_multi(engine, llm_limit, DICT_LIMIT, &preedit), false)
                    } else {
                        (vec![preedit.clone()], false)
                    }
                } else {
                    (merged, false)
                }
            }
            _ => {
                if kanji_ready_now {
                    let dict_cands = engine_convert_sync_multi(engine, llm_limit, DICT_LIMIT, &preedit);
                    if dict_cands.is_empty() { (vec![preedit.clone()], false) } else { (dict_cands, false) }
                } else {
                    (vec![preedit.clone()], false)
                }
            }
        };
        // Waiting 状態を解除
        if let Ok(mut sess) = session_get() {
            if sess.is_waiting() { sess.set_preedit(preedit.clone()); }
        }
        candidate_window::stop_waiting_timer();
        let _ = bg_status2; // suppress unused warning

        let first = candidates.first().cloned().unwrap_or_else(|| preedit.clone());
        diag::event(DiagEvent::Convert { preedit: preedit.clone(), kanji_ready: true, result: first.clone() });

        let caret = caret_rect_get();
        let (page_cands, page_info) = {
            let mut sess = session_get()?;
            sess.activate_selecting(candidates.clone(), preedit.clone(), caret.left, caret.bottom, llm_pending);
            (sess.page_candidates().to_vec(), sess.page_info())
        };
        drop(guard);
        let status = if llm_pending { Some("⏳ 変換中...") } else { None };
        candidate_window::show_with_status(&page_cands, 0, &page_info, caret.left, caret.bottom, status);
        tracing::info!("on_convert[new]: update_composition first={:?} comp_exists={}", first,
            composition_clone().map(|g| g.is_some()).unwrap_or(false));
        update_composition(ctx, tid, sink, first)?;
        Ok(true)
    }

    fn on_commit_raw(
        &self, ctx: ITfContext, tid: u32,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
        {
            let mut sess = session_get()?;
            // ── SplitPreedit: target をそのまま確定、remainder を次のプリエディットへ ──
            if sess.is_split_preedit() {
                let target    = sess.split_target().unwrap_or("").to_string();
                let remainder = sess.split_remainder().unwrap_or("").to_string();
                if target.is_empty() { return Ok(false); }
                sess.set_idle();
                drop(sess);
                candidate_window::hide();
                engine.commit(&target);
                if remainder.is_empty() {
                    engine.reset_preedit();
                    drop(guard);
                    end_composition(ctx, tid, target)?;
                } else {
                    engine.force_preedit(remainder.clone());
                    drop(guard);
                    commit_then_start_composition(ctx, tid, unsafe { self.cast()? }, target, remainder)?;
                }
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
                let text      = sess.current_candidate().or_else(|| sess.original_preedit()).unwrap_or("").to_string();
                let reading   = sess.original_preedit().unwrap_or("").to_string();
                let punct     = sess.take_punct_pending();
                let remainder = sess.take_selecting_remainder();
                sess.set_idle();
                drop(sess);
                candidate_window::hide();
                if text != reading { engine.learn(&reading, &text); }
                let commit_text = if let Some(p) = punct { format!("{text}{p}") } else { text.clone() };
                engine.commit(&commit_text);
                if remainder.is_empty() {
                    engine.reset_preedit();
                    drop(guard);
                    diag::event(DiagEvent::CommitRaw { preedit: commit_text.clone() });
                    end_composition(ctx, tid, commit_text)?;
                } else {
                    engine.force_preedit(remainder.clone());
                    drop(guard);
                    diag::event(DiagEvent::CommitRaw { preedit: commit_text.clone() });
                    commit_then_start_composition(ctx, tid, unsafe { self.cast()? }, commit_text, remainder)?;
                }
                return Ok(true);
            }
        }
        engine.flush_pending_n();
        let preedit = engine.preedit_display();
        if preedit.is_empty() { return Ok(false); }
        diag::event(DiagEvent::CommitRaw { preedit: preedit.clone() });
        engine.bg_reclaim();
        engine.commit(&preedit.clone());
        engine.reset_preedit();
        drop(guard);
        end_composition(ctx, tid, preedit)?;
        Ok(true)
    }

    fn on_backspace(
        &self, ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
        {
            let mut sess = session_get()?;
            // SplitPreedit → Backspace → 全体を未変換プリエディットに戻す
            if sess.is_split_preedit() {
                let full = format!("{}{}",
                    sess.split_target().unwrap_or(""),
                    sess.split_remainder().unwrap_or(""));
                sess.set_preedit(full.clone());
                drop(sess);
                engine.force_preedit(full.clone());
                drop(guard);
                update_composition(ctx, tid, sink, full)?;
                return Ok(true);
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
            diag::event(DiagEvent::Backspace { preedit_after: preedit.clone() });
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
        &self, ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
        {
            let mut sess = session_get()?;
            // SplitPreedit → ESC → 全体を未変換プリエディットに戻す
            if sess.is_split_preedit() {
                let full = format!("{}{}",
                    sess.split_target().unwrap_or(""),
                    sess.split_remainder().unwrap_or(""));
                tracing::info!("on_cancel[SplitPreedit]: restoring full={:?}", full);
                sess.set_preedit(full.clone());
                drop(sess);
                candidate_window::hide();
                // SplitPreedit 遷移時に force_preedit(target) で hiragana_buf が
                // target のみになっている → full で復元しないと remainder が失われる
                engine.force_preedit(full.clone());
                drop(guard);
                update_composition(ctx, tid, sink, full)?;
                return Ok(true);
            }
            if sess.is_selecting() {
                // 変換中 → ESC → 未変換状態へ戻す（2回目のESCでプリエディット全消去）
                // 文節分割後の変換の場合は remainder も復元して full に戻す
                let original  = sess.original_preedit().unwrap_or("").to_string();
                let remainder = sess.selecting_remainder_clone();
                let full = if remainder.is_empty() {
                    original.clone()
                } else {
                    format!("{}{}", original, remainder)
                };
                tracing::info!("on_cancel[Selecting]: original={:?} remainder={:?} → full={:?}", original, remainder, full);
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
                tracing::info!("on_cancel[Waiting]: pre={:?} bg={}", pre, bg);
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
            tracing::info!("on_cancel[fallthrough]: preedit_empty={} bg={} hira={:?}", engine.preedit_is_empty(), bg, hira);
        }
        if engine.preedit_is_empty() { return Ok(false); }
        engine.bg_reclaim();
        engine.reset_all();
        drop(guard);
        end_composition(ctx, tid, String::new())?;
        Ok(true)
    }

    fn on_kana_convert(
        &self, ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
        convert_fn: fn(&str) -> String,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
        engine.flush_pending_n();
        let p = engine.preedit_display();
        if p.is_empty() { return Ok(false); }
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
            if hira.is_empty() { p.clone() } else { hira }
        } else {
            p.clone()
        };
        let t = convert_fn(&source);
        engine.force_preedit(t.clone());
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
        &self, ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
        full: bool, // true=F9全角, false=F10半角
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
        engine.flush_pending_n();
        let p = engine.preedit_display();
        if p.is_empty() { return Ok(false); }
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
            let romaji = engine.romaji_log_str();
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
        drop(guard);
        update_composition(ctx, tid, sink, t)?;
        Ok(true)
    }

    fn on_cycle_kana(
        &self, ctx: ITfContext, tid: u32,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
        let p = engine.preedit_display();
        if p.is_empty() { return Ok(false); }
        engine.bg_reclaim();
        let t = text_util::to_katakana(&p);
        engine.commit(&t); engine.reset_preedit(); drop(guard);
        end_composition(ctx, tid, t)?;
        Ok(true)
    }

    fn on_candidate_move(
        &self, ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        guard: crate::engine::state::EngineGuard,
        dir: CandidateDir,
    ) -> Result<bool> {
        let has_pre = guard.as_ref().map(|e| !e.preedit_is_empty()).unwrap_or(false);
        drop(guard);
        let mut sess = session_get()?;
        if !sess.is_selecting() { return Ok(has_pre); }
        match dir { CandidateDir::Next => sess.next_with_page_wrap(), CandidateDir::Prev => sess.prev() }
        let page_cands = sess.page_candidates().to_vec();
        let page_sel   = sess.page_selected();
        let page_info  = sess.page_info();
        let text = sess.current_candidate().or_else(|| sess.original_preedit()).unwrap_or("").to_string();
        let remainder  = sess.selecting_remainder_clone();
        drop(sess);
        candidate_window::update_selection(page_sel, &page_info);
        candidate_window::show(&page_cands, page_sel, &page_info, caret_rect_get().left, caret_rect_get().bottom);
        update_composition_candidate_split(ctx, tid, sink, text, remainder)?;
        Ok(true)
    }

    fn on_candidate_page(
        &self, ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        guard: crate::engine::state::EngineGuard,
        dir: CandidateDir,
    ) -> Result<bool> {
        let has_pre = guard.as_ref().map(|e| !e.preedit_is_empty()).unwrap_or(false);
        drop(guard);
        let mut sess = session_get()?;
        if !sess.is_selecting() { return Ok(has_pre); }
        match dir { CandidateDir::Next => sess.next_page(), CandidateDir::Prev => sess.prev_page() }
        let page_cands = sess.page_candidates().to_vec();
        let page_sel   = sess.page_selected();
        let page_info  = sess.page_info();
        let text = sess.current_candidate().or_else(|| sess.original_preedit()).unwrap_or("").to_string();
        let remainder  = sess.selecting_remainder_clone();
        drop(sess);
        let caret = caret_rect_get();
        candidate_window::show(&page_cands, page_sel, &page_info, caret.left, caret.bottom);
        update_composition_candidate_split(ctx, tid, sink, text, remainder)?;
        Ok(true)
    }

    fn on_candidate_select(
        &self, n: u8, ctx: ITfContext, tid: u32,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
        let has_pre = !engine.preedit_is_empty();
        let mut sess = session_get()?;
        if !sess.is_selecting() { return Ok(has_pre); }
        if !sess.select_nth_in_page(n as usize) { return Ok(true); }
        let text      = sess.current_candidate().or_else(|| sess.original_preedit()).unwrap_or("").to_string();
        let reading   = sess.original_preedit().unwrap_or("").to_string();
        let punct     = sess.take_punct_pending();
        let remainder = sess.take_selecting_remainder();
        sess.set_idle();
        drop(sess);
        candidate_window::hide();
        if text != reading { engine.learn(&reading, &text); }
        let commit_text = if let Some(p) = punct { format!("{text}{p}") } else { text.clone() };
        engine.commit(&commit_text);
        diag::event(DiagEvent::Convert { preedit: text.clone(), kanji_ready: true, result: commit_text.clone() });
        if remainder.is_empty() {
            engine.reset_preedit();
            drop(guard);
            end_composition(ctx, tid, commit_text)?;
        } else {
            engine.force_preedit(remainder.clone());
            drop(guard);
            commit_then_start_composition(ctx, tid, unsafe { self.cast()? }, commit_text, remainder)?;
        }
        Ok(true)
    }

    fn on_ime_toggle(&self, ctx: ITfContext, tid: u32) -> Result<bool> {
        {
            let mut guard = engine_try_get_or_create()?;
            if let Some(engine) = guard.as_mut() {
                let preedit = engine.preedit_display();
                if !preedit.is_empty() {
                    engine.bg_reclaim();
                    engine.commit(&preedit.clone()); engine.reset_preedit();
                    drop(guard);
                    end_composition(ctx.clone(), tid, preedit)?;
                }
            }
        }
        let (from, to, now_open) = if let Ok(mut st) = crate::engine::state::ime_state_get() {
            use crate::engine::input_mode::InputMode;
            let was_alpha = st.input_mode == InputMode::Alphanumeric;
            let new_mode  = if was_alpha { InputMode::Hiragana } else { InputMode::Alphanumeric };
            let from      = format!("{:?}", st.input_mode);
            st.set_mode(new_mode);
            (from, if was_alpha { "Hiragana" } else { "Alphanumeric" }, was_alpha)
        } else { ("unknown".into(), "unknown", true) };
        if let Ok(inner) = self.inner.try_borrow() {
            if let Some(tm) = &inner.thread_mgr {
                if let Err(e) = unsafe { language_bar::set_open_close(tm, tid, now_open) } {
                    tracing::warn!("ImeToggle: set_open_close({}) failed: {e}", now_open);
                    diag::event(DiagEvent::Error { site: "set_open_close/toggle", msg: e.to_string() });
                }
            }
        }
        diag::event(DiagEvent::ModeChange { from, to });
        self.notify_langbar_update();
        self.notify_tray_update(tid);
        self.maybe_reload_runtime_config();
        Ok(true)
    }

    fn on_ime_off(&self, ctx: ITfContext, tid: u32) -> Result<bool> {
        {
            let mut guard = engine_try_get_or_create()?;
            if let Some(engine) = guard.as_mut() {
                let preedit = engine.preedit_display();
                if !preedit.is_empty() {
                    engine.bg_reclaim();
                    engine.commit(&preedit.clone()); engine.reset_preedit();
                    drop(guard);
                    end_composition(ctx, tid, preedit)?;
                }
            }
        }
        if let Ok(mut st) = crate::engine::state::ime_state_get() {
            let from = format!("{:?}", st.input_mode);
            st.set_mode(crate::engine::input_mode::InputMode::Alphanumeric);
            diag::event(DiagEvent::ModeChange { from, to: "Alphanumeric" });
        }
        if let Ok(inner) = self.inner.try_borrow() {
            if let Some(tm) = &inner.thread_mgr {
                if let Err(e) = unsafe { language_bar::set_open_close(tm, tid, false) } {
                    tracing::warn!("ImeOff: set_open_close(false) failed: {e}");
                    diag::event(DiagEvent::Error { site: "set_open_close/off", msg: e.to_string() });
                }
            }
        }
        self.notify_langbar_update();
        self.notify_tray_update(tid);
        self.maybe_reload_runtime_config();
        Ok(true)
    }

    fn on_ime_on(&self, _ctx: ITfContext, tid: u32) -> Result<bool> {
        if let Ok(mut st) = crate::engine::state::ime_state_get() {
            let from = format!("{:?}", st.input_mode);
            st.set_mode(crate::engine::input_mode::InputMode::Hiragana);
            diag::event(DiagEvent::ModeChange { from, to: "Hiragana" });
        }
        if let Ok(inner) = self.inner.try_borrow() {
            if let Some(tm) = &inner.thread_mgr {
                if let Err(e) = unsafe { language_bar::set_open_close(tm, tid, true) } {
                    tracing::warn!("ImeOn: set_open_close(true) failed: {e}");
                    diag::event(DiagEvent::Error { site: "set_open_close/on", msg: e.to_string() });
                }
            }
        }
        self.notify_langbar_update();
        self.notify_tray_update(tid);
        self.maybe_reload_runtime_config();
        Ok(true)
    }

    fn on_mode_hiragana(
        &self, ctx: ITfContext, tid: u32,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        if let Some(engine) = guard.as_mut() {
            let preedit = engine.preedit_display();
            if !preedit.is_empty() {
                let t = preedit.clone();
                engine.bg_reclaim();
                engine.commit(&t); engine.reset_preedit(); drop(guard);
                end_composition(ctx, tid, t)?;
            } else { drop(guard); }
        }
        if let Ok(mut st) = crate::engine::state::ime_state_get() {
            let from = format!("{:?}", st.input_mode);
            st.set_mode(crate::engine::input_mode::InputMode::Hiragana);
            diag::event(DiagEvent::ModeChange { from, to: "Hiragana" });
        }
        self.notify_langbar_update();
        self.notify_tray_update(tid);
        self.maybe_reload_runtime_config();
        Ok(true)
    }

    fn on_mode_katakana(
        &self, ctx: ITfContext, tid: u32,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        if let Some(engine) = guard.as_mut() {
            let preedit = engine.preedit_display();
            if !preedit.is_empty() {
                let t = text_util::to_katakana(&preedit);
                engine.bg_reclaim();
                engine.commit(&t); engine.reset_preedit(); drop(guard);
                end_composition(ctx, tid, t)?;
            } else { drop(guard); }
        }
        if let Ok(mut st) = crate::engine::state::ime_state_get() {
            let from = format!("{:?}", st.input_mode);
            st.set_mode(crate::engine::input_mode::InputMode::Katakana);
            diag::event(DiagEvent::ModeChange { from, to: "Katakana" });
        }
        self.notify_langbar_update();
        self.notify_tray_update(tid);
        self.maybe_reload_runtime_config();
        Ok(true)
    }

    /// 句読点入力:
    ///   - プリエディットがあれば変換ウィンドウを表示し punct_pending にセット
    ///   - プリエディットが空なら直接コミット
    fn on_punctuate(
        &self, c: char,
        ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };

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
                let page_sel   = sess.page_selected();
                let page_info  = sess.page_info();
                let (pos_x, pos_y) = sess.selecting_pos().unwrap_or_default();
                drop(sess);
                drop(guard);
                candidate_window::show_with_status(
                    &page_cands, page_sel, &page_info, pos_x, pos_y,
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

        let llm_limit   = crate::engine::state::get_num_candidates();
        const DICT_LIMIT: usize = 20;
        engine.poll_dict_ready();
        engine.poll_model_ready();
        let kanji_ready = engine.is_kanji_ready();
        if kanji_ready && engine.bg_status() == "idle" {
            engine.bg_start(llm_limit);
        }
        const BG_WAIT_MS: u64 = 400;
        if kanji_ready && matches!(engine.bg_status(), "running" | "idle") {
            engine.bg_wait_ms(BG_WAIT_MS);
        }

        let bg_status  = engine.bg_status();
        let bg_running = !kanji_ready || bg_status == "running" || bg_status == "idle";

        let (candidates, llm_pending): (Vec<String>, bool) =
            match engine.bg_take_candidates(&preedit) {
            Some(llm_cands) if !llm_cands.is_empty() => {
                let merged = engine.merge_candidates(llm_cands, DICT_LIMIT);
                if merged.is_empty() || (merged.len() == 1 && merged[0] == preedit) {
                    (engine_convert_sync_multi(engine, llm_limit, DICT_LIMIT, &preedit), false)
                } else {
                    (merged, false)
                }
            }
            _ => {
                if kanji_ready && !bg_running {
                    (engine_convert_sync_multi(engine, llm_limit, DICT_LIMIT, &preedit), false)
                } else {
                    let dict_cands = engine.merge_candidates(vec![], DICT_LIMIT);
                    let dict_empty = dict_cands.is_empty()
                        || (dict_cands.len() == 1 && dict_cands[0] == preedit);
                    if dict_empty { (vec![preedit.clone()], bg_running) } else { (dict_cands, bg_running) }
                }
            }
        };

        let first = candidates.first().cloned().unwrap_or_else(|| preedit.clone());
        let caret = caret_rect_get();
        {
            let mut sess = session_get()?;
            sess.activate_selecting(candidates, preedit.clone(), caret.left, caret.bottom, llm_pending);
            sess.set_punct_pending(c);
            let page_cands = sess.page_candidates().to_vec();
            let page_info  = sess.page_info();
            drop(sess);
            drop(guard);
            let status_owned = format!("確定後「{c}」を入力");
            candidate_window::show_with_status(&page_cands, 0, &page_info, caret.left, caret.bottom, Some(&status_owned));
        }
        update_composition(ctx, tid, sink, first)?;
        Ok(true)
    }

    /// Shift+Left: 変換対象を1文字縮める（表示のみ・変換しない）
    fn on_segment_shrink(
        &self, ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
        let mut sess = session_get()?;

        tracing::debug!("on_segment_shrink: state={:?}", &*sess);

        // Selecting → SplitPreedit
        if sess.is_selecting() {
            let original        = sess.original_preedit().unwrap_or("").to_string();
            let outer_remainder = sess.selecting_remainder_clone();
            let last_start = original.char_indices().next_back().map(|(i, _)| i);
            tracing::debug!("  Selecting: original={:?} outer_rem={:?} last_start={:?}", original, outer_remainder, last_start);
            let (target, new_rem) = match last_start {
                Some(i) if i > 0 => (original[..i].to_string(), original[i..].to_string()),
                _ => return Ok(true),
            };
            let remainder = format!("{new_rem}{outer_remainder}");
            let full      = format!("{target}{remainder}");
            tracing::debug!("  → SplitPreedit: target={:?} remainder={:?} full={:?}", target, remainder, full);
            sess.set_split_preedit(target.clone(), remainder.clone());
            drop(sess);
            candidate_window::hide();
            engine.bg_reclaim();
            drop(guard);
            // target を太実線、remainder を点線で表示して境界を視覚化
            update_composition_candidate_split(ctx, tid, sink, target, remainder)?;
            // update_composition 後に state が保持されているか確認
            if let Ok(s) = crate::engine::state::session_get() {
                tracing::debug!("  after update_composition: state={:?}", &*s);
            }
            return Ok(true);
        }

        // SplitPreedit → 境界を1文字縮める
        if sess.is_split_preedit() {
            let before_target = sess.split_target().unwrap_or("").to_string();
            let shrank = sess.split_shrink();
            tracing::debug!("  SplitPreedit: before_target={:?} shrank={}", before_target, shrank);
            if !shrank { return Ok(true); }
            let target    = sess.split_target().unwrap_or("").to_string();
            let remainder = sess.split_remainder().unwrap_or("").to_string();
            tracing::debug!("  → new target={:?} remainder={:?}", target, remainder);
            drop(sess);
            drop(guard);
            // target を太実線、remainder を点線で表示して境界を視覚化
            update_composition_candidate_split(ctx, tid, sink, target, remainder)?;
            return Ok(true);
        }

        tracing::debug!("  → no matching state, eat={}", !engine.preedit_is_empty());
        Ok(!engine.preedit_is_empty())
    }

    /// Shift+Right: 変換対象を1文字広げる（表示のみ・変換しない）
    fn on_segment_extend(
        &self, ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
        let mut sess = session_get()?;

        if sess.is_split_preedit() {
            if !sess.split_extend() {
                // remainder 空 = 右端 → 全体を Preedit に戻す
                let full = sess.split_target().unwrap_or("").to_string();
                sess.set_preedit(full.clone());
                drop(sess);
                drop(guard);
                update_composition(ctx, tid, sink, full)?;
                return Ok(true);
            }
            let target    = sess.split_target().unwrap_or("").to_string();
            let remainder = sess.split_remainder().unwrap_or("").to_string();
            if remainder.is_empty() {
                sess.set_preedit(target.clone());
            }
            drop(sess);
            drop(guard);
            update_composition_candidate_split(ctx, tid, sink, target, remainder)?;
            return Ok(true);
        }

        Ok(!engine.preedit_is_empty())
    }
}

// ─── 変換ヘルパー ─────────────────────────────────────────────────────────────

/// target 文字列を変換して `Selecting` 状態へ遷移する。
///
/// `on_segment_shrink` / `on_segment_extend` の共通処理。
/// 境界変更直後に候補を再取得し、候補ウィンドウを表示する。
///
/// # 引数
/// - `target`    : 変換対象（ひらがな）
/// - `remainder` : 変換しない残り部分。確定後に次のプリエディットになる。
///                 空文字列の場合は全体変換（remainder なし）として扱う。
fn convert_split_target(
    ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
    mut guard: crate::engine::state::EngineGuard,
    target: String,
    remainder: String,
) -> Result<bool> {
    let engine = match guard.as_mut() { Some(e) => e, None => return Ok(false) };
    if target.is_empty() { return Ok(false); }

    tracing::debug!("convert_split_target: target={:?} remainder={:?}", target, remainder);

    engine.bg_reclaim();
    engine.force_preedit(target.clone());

    let llm_limit = crate::engine::state::get_num_candidates();
    const DICT_LIMIT: usize = 20;
    const SPLIT_WAIT_MS: u64 = 1500;
    let kanji_ready = engine.is_kanji_ready();

    // kanji_ready な場合は同期変換を先に実行してから bg_start する。
    // bg_start は self.kanji を move するため、後から convert_sync を呼べなくなる。
    // 先に同期変換で辞書候補を取得しておき、LLM完了後にマージする戦略を取る。
    let sync_cands: Vec<String> = if kanji_ready {
        engine_convert_sync_multi(engine, llm_limit, DICT_LIMIT, &target)
    } else {
        vec![target.clone()]
    };

    // bg_start → 最大 SPLIT_WAIT_MS 待機
    let bg_cands = if kanji_ready {
        engine.bg_start(llm_limit);
        let completed = engine.bg_wait_ms(SPLIT_WAIT_MS);
        tracing::info!("convert_split_target: bg_wait({SPLIT_WAIT_MS}ms) completed={completed}");
        if completed {
            engine.bg_take_candidates(&target)
        } else {
            None
        }
    } else {
        None
    };

    let candidates = match bg_cands {
        Some(llm_cands) if !llm_cands.is_empty() => {
            let m = engine.merge_candidates(llm_cands, DICT_LIMIT);
            if m.is_empty() { sync_cands } else { m }
        }
        _ => {
            tracing::info!("convert_split_target: LLM not ready, using sync_cands ({} cands)", sync_cands.len());
            sync_cands
        }
    };
    tracing::debug!("convert_split_target: candidates={:?}", &candidates[..candidates.len().min(3)]);
    let first = candidates.first().cloned().unwrap_or_else(|| target.clone());
    let caret = caret_rect_get();
    let (page_cands, page_info, remainder_for_display) = {
        let mut sess = session_get()?;
        sess.activate_selecting_with_remainder(
            candidates,
            target.clone(),
            caret.left,
            caret.bottom,
            false,
            remainder.clone(),
        );
        (sess.page_candidates().to_vec(), sess.page_info(), remainder)
    };
    drop(guard);
    candidate_window::show_with_status(
        &page_cands, 0, &page_info, caret.left, caret.bottom, None,
    );
    // remainder が残っている場合は「候補 + 未変換残り」を1つの composition で表示する。
    // こうしないと remainder 部分が画面から消えてしまう。
    update_composition_candidate_split(ctx, tid, sink, first, remainder_for_display)?;
    Ok(true)
}

/// 複数候補を返す版（候補ウィンドウ用）
/// プリエディット（ひらがな）をそのまま確定してコンポジションを終了する。
/// 辞書0件 + LLM 待機中に Space を2回押したときの逃げ道として使用する。
#[allow(dead_code)]
fn engine_commit_hiragana(ctx: ITfContext, tid: u32) -> Result<()> {
    let preedit = {
        let mut guard = engine_get()
            .map_err(|e| anyhow::anyhow!("engine_commit_hiragana: engine unavailable: {e}"))?;
        let engine = guard.as_mut()
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
    if preedit.is_empty() { return Ok(()); }
    tracing::debug!("engine_commit_hiragana: committing preedit={preedit:?}");
    end_composition(ctx, tid, preedit)
}

fn engine_convert_sync_multi(
    engine: &mut rakukan_engine_abi::DynEngine,
    llm_limit: usize,
    dict_limit: usize,
    preedit: &str,
) -> Vec<String> {
    // LLM候補を取得（llm_limit 件）
    let llm_cands: Vec<String> = engine.convert_sync();
    let _ = llm_limit;  // DynEngine::convert_sync は num_candidates を内部設定から読む

    // 辞書候補とマージ（dict_limit 件まで）
    let merged = engine.merge_candidates(llm_cands, dict_limit);
    if merged.is_empty() { vec![preedit.to_string()] } else { merged }
}

// ─── OnTestKeyDown ヘルパー ──────────────────────────────────────────────────

#[inline]
fn key_should_eat(action: &UserAction, has_preedit: bool) -> bool {
    match action {
        UserAction::Input(_) | UserAction::InputRaw(_) | UserAction::FullWidthSpace => true,
        UserAction::Backspace => has_preedit,
        UserAction::ImeToggle | UserAction::ImeOff | UserAction::ImeOn
        | UserAction::ModeHiragana | UserAction::ModeKatakana | UserAction::ModeAlphanumeric => true,
        UserAction::Convert | UserAction::CommitRaw | UserAction::Cancel | UserAction::CancelAll
        | UserAction::Hiragana | UserAction::Katakana | UserAction::HalfKatakana
        | UserAction::FullLatin | UserAction::HalfLatin | UserAction::CycleKana
        | UserAction::CandidateNext | UserAction::CandidatePrev
        | UserAction::CandidatePageDown | UserAction::CandidatePageUp
        | UserAction::CursorLeft | UserAction::CursorRight => has_preedit,
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
        UserAction::Input(_)           => "Input",
        UserAction::InputRaw(_)        => "InputRaw",
        UserAction::FullWidthSpace     => "FullWidthSpace",
        UserAction::Convert            => "Convert",
        UserAction::CommitRaw          => "CommitRaw",
        UserAction::Backspace          => "Backspace",
        UserAction::Cancel             => "Cancel",
        UserAction::CancelAll          => "CancelAll",
        UserAction::Hiragana           => "Hiragana",
        UserAction::Katakana           => "Katakana",
        UserAction::HalfKatakana       => "HalfKatakana",
        UserAction::FullLatin          => "FullLatin",
        UserAction::HalfLatin          => "HalfLatin",
        UserAction::CycleKana          => "CycleKana",
        UserAction::CandidateNext      => "CandidateNext",
        UserAction::CandidatePrev      => "CandidatePrev",
        UserAction::CandidatePageDown  => "CandidatePageDown",
        UserAction::CandidatePageUp    => "CandidatePageUp",
        UserAction::CandidateSelect(_) => "CandidateSelect",
        UserAction::CursorLeft         => "CursorLeft",
        UserAction::CursorRight        => "CursorRight",
        UserAction::Punctuate(_)       => "Punctuate",
        UserAction::SegmentShrink      => "SegmentShrink",
        UserAction::SegmentExtend      => "SegmentExtend",
        UserAction::ImeToggle          => "ImeToggle",
        UserAction::ImeOn              => "ImeOn",
        UserAction::ImeOff             => "ImeOff",
        UserAction::ModeHiragana       => "ModeHiragana",
        UserAction::ModeKatakana       => "ModeKatakana",
        UserAction::ModeAlphanumeric   => "ModeAlphanumeric",
        _ => "Other",
    }
}

// ─── EditSession ヘルパー ─────────────────────────────────────────────────────
// TF_ES_SYNC を使わない（TF_ES_READWRITE のみ）

/// 現在のキャレット位置を表す長さ0の ITfRange を返す。
/// GetSelection で現在選択範囲を取得し、終端アンカーに collapse する。
/// 失敗時は None（呼び元が GetEnd にフォールバックする）。
unsafe fn get_cursor_range(
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    ec: u32,
) -> Option<windows::Win32::UI::TextServices::ITfRange> {
    use windows::Win32::UI::TextServices::{TF_ANCHOR_END, TF_SELECTION, TfActiveSelEnd, TF_SELECTIONSTYLE};
    use windows::Win32::Foundation::BOOL;

    // windows-rs 0.58: GetSelection(ec, ulIndex, pSelection: &mut [TF_SELECTION]) -> *mut u32
    // TF_DEFAULT_SELECTION = 0xFFFF_FFFF
    let mut sel_buf = [TF_SELECTION {
        range: std::mem::ManuallyDrop::new(None),
        style: TF_SELECTIONSTYLE { ase: TfActiveSelEnd(0), fInterimChar: BOOL(0) },
    }];
    let mut fetched: u32 = 0;
    ctx.GetSelection(ec, 0xFFFF_FFFF_u32, &mut sel_buf, &mut fetched as *mut u32).ok()?;
    if fetched == 0 { return None; }
    let range_ref = (&*sel_buf[0].range).as_ref()?;
    let cloned = range_ref.Clone().ok()?;
    let _ = cloned.Collapse(ec, TF_ANCHOR_END);
    Some(cloned)
}

fn update_composition(
    ctx: ITfContext, tid: u32, sink: ITfCompositionSink, preedit: String,
) -> Result<()> {
    let existing = composition_clone()?;
    let ctx_req  = ctx.clone();
    let session  = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            ITfContextComposition, TfActiveSelEnd, TF_SELECTION, TF_SELECTIONSTYLE,
            TF_ANCHOR_END,
        };

        let preedit_w: Vec<u16> = preedit.encode_utf16().collect();
        tracing::debug!("update_composition[EditSession]: preedit={:?} existing={}", preedit, existing.is_some());

        let range = if let Some(comp) = &existing {
            comp.GetRange()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange: {e}")))?
        } else {
            // Fix2: GetEnd(文書末尾)ではなく現在のカーソル位置を使う
            let insert_point = get_cursor_range(&ctx, ec)
                .unwrap_or_else(|| ctx.GetEnd(ec).unwrap());
            let cc: ITfContextComposition = ctx.cast()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("cast ITfContextComposition: {e}")))?;
            let new_comp = cc.StartComposition(ec, &insert_point, &sink)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("StartComposition: {e}")))?;
            let r = new_comp.GetRange()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange new: {e}")))?;
            let _ = composition_set(Some(new_comp));
            r
        };

        range.SetText(ec, 0, &preedit_w)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText: {e}")))?;

        // アンダーライン属性をセット
        // SESSION_SELECTING アトミックで高速判定（クロージャ内なので Mutex は取れない）
        // SplitPreedit は SESSION_SELECTING=true だが atom_input を使う。
        // is_selecting() を正確に判定するには Mutex が必要だが、ここは EditSession
        // クロージャ内（TSF のロック下）なので SESSION_STATE.lock() はデッドロックの
        // リスクがある。安全のため atom_input で統一し、Selecting 時は on_candidate_move
        // 等の呼び出し元が update_composition_candidate_split を使うことで区別する。
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
        let _ = ctx_req.RequestEditSession(tid, &session, TF_ES_READWRITE)
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
    ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
    commit_text: String, next_preedit: String,
) -> Result<()> {
    let comp = composition_take()?;
    let ctx_req = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            ITfContextComposition, TF_ANCHOR_END,
            TfActiveSelEnd, TF_SELECTION, TF_SELECTIONSTYLE,
        };

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
            }
            comp.EndComposition(ec)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("EndComposition: {e}")))?;
        } else if !commit_text.is_empty() {
            let insert_point = get_cursor_range(&ctx, ec)
                .unwrap_or_else(|| ctx.GetEnd(ec).unwrap());
            insert_point.SetText(ec, 0, &commit_w)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText direct commit: {e}")))?;
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
        let insert_point = insert_after_commit
            .unwrap_or_else(|| ctx.GetEnd(ec).unwrap());
        let cc: ITfContextComposition = ctx.cast()
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("cast ITfContextComposition: {e}")))?;
        let new_comp = cc.StartComposition(ec, &insert_point, &sink)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("StartComposition: {e}")))?;
        let new_range = new_comp.GetRange()
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange new: {e}")))?;
        let _ = composition_set(Some(new_comp));

        let preedit_w: Vec<u16> = next_preedit.encode_utf16().collect();
        new_range.SetText(ec, 0, &preedit_w)
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
        let _ = ctx_req.RequestEditSession(tid, &session, TF_ES_READWRITE)
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
    if atom == 0 { return; }
    let Ok(prop) = ctx.GetProperty(&GUID_PROP_ATTRIBUTE) else { return; };
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
fn update_composition_candidate_split(
    ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
    converted: String, remainder: String,
) -> Result<()> {
    if remainder.is_empty() {
        return update_composition(ctx, tid, sink, converted);
    }

    let existing = composition_clone()?;
    let ctx_req  = ctx.clone();
    let full = format!("{converted}{remainder}");
    // remainder の UTF-16 長（ShiftStart に使う）
    let rem_utf16: i32 = remainder.encode_utf16().count() as i32;

    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            ITfContextComposition, TfActiveSelEnd, TF_SELECTION, TF_SELECTIONSTYLE, TF_ANCHOR_END,
        };

        let full_w: Vec<u16> = full.encode_utf16().collect();

        // ── Step1: テキストをセット ──
        let range = if let Some(comp) = &existing {
            comp.GetRange()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange: {e}")))?
        } else {
            let insert_point = get_cursor_range(&ctx, ec)
                .unwrap_or_else(|| ctx.GetEnd(ec).unwrap());
            let cc: ITfContextComposition = ctx.cast()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("cast: {e}")))?;
            let new_comp = cc.StartComposition(ec, &insert_point, &sink)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("StartComposition: {e}")))?;
            let r = new_comp.GetRange()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange new: {e}")))?;
            let _ = composition_set(Some(new_comp));
            r
        };

        range.SetText(ec, 0, &full_w)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText: {e}")))?;

        // ── Step2: 属性セット ──
        // 全体を atom_converted（太実線）で塗る
        set_display_attr_prop(&ctx, ec, &range, display_attr::atom_converted());

        // remainder 部分（末尾 rem_utf16 文字）を atom_input（点線）で上書き
        if rem_utf16 > 0 {
            if let Ok(rem_range) = range.Clone() {
                let mut actual = 0i32;
                // 末尾 rem_utf16 文字の範囲を選ぶ: start を後ろから rem_utf16 文字分 forward
                // = end を先頭にしてから rem_utf16 文字分縮める
                // 安全な方法: start anchor を end 側に合わせてから負方向に移動
                let _ = rem_range.Collapse(ec, TF_ANCHOR_END);
                let _ = rem_range.ShiftStart(ec, -rem_utf16, &mut actual,
                    std::ptr::null::<windows::Win32::UI::TextServices::TF_HALTCOND>());
                set_display_attr_prop(&ctx, ec, &rem_range, display_attr::atom_input());
            }
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
        let _ = ctx_req.RequestEditSession(tid, &session, TF_ES_READWRITE)
            .map_err(|e| anyhow::anyhow!("RequestEditSession candidate_split: {e}"));
    }
    Ok(())
}

/// スペース押下時のみ呼ぶ: caret_rect をキャレット位置で更新する
fn update_caret_rect(ctx: ITfContext, tid: u32) {
    let comp = match composition_clone() {
        Ok(Some(c)) => c,
        _           => return,
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
    unsafe { let _ = ctx_req.RequestEditSession(tid, &session, TF_ES_READWRITE); }
}

fn end_composition(ctx: ITfContext, tid: u32, text: String) -> Result<()> {
    use windows::Win32::UI::TextServices::{
        TF_ANCHOR_END,
        TfActiveSelEnd, TF_SELECTION, TF_SELECTIONSTYLE,
    };
    let comp = match composition_take()? {
        Some(c) => c,
        None    => return Ok(()),
    };
    let ctx2 = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        let text_w: Vec<u16> = text.encode_utf16().collect();
        tracing::trace!("SetText(end): {:?}", text);
        let range = comp.GetRange()
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange: {e}")))?;
        range.SetText(ec, 0, &text_w)
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
        let _ = ctx.RequestEditSession(tid, &session, TF_ES_READWRITE)
            .map_err(|e| anyhow::anyhow!("RequestEditSession end: {e}"));
    }
    Ok(())
}

fn commit_text(ctx: ITfContext, tid: u32, text: String) -> Result<()> {
    let ctx_req = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            TF_ANCHOR_END, TfActiveSelEnd, TF_SELECTION, TF_SELECTIONSTYLE,
        };
        let text_w: Vec<u16> = text.encode_utf16().collect();
        // 現在のカーソル位置に挿入（GetEnd=文書末尾ではなくカーソル位置）
        let insert = get_cursor_range(&ctx, ec)
            .unwrap_or_else(|| ctx.GetEnd(ec).unwrap());
        insert.SetText(ec, 0, &text_w)
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
        let _ = ctx_req.RequestEditSession(tid, &session, TF_ES_READWRITE)
            .map_err(|e| anyhow::anyhow!("RequestEditSession commit: {e}"));
    }
    Ok(())
}

// ─── ITfLangBarItem ──────────────────────────────────────────────────────────

/// 現在のバックエンドラベルを返す（例: "CPU" / "Vulkan" / "CUDA" / "初期化中..."）
fn current_backend_label() -> String {
    engine_try_get_or_create().ok()
        .as_deref()          // Option<MutexGuard<EngineWrapper>> → Option<&EngineWrapper>
        .and_then(|g| g.as_ref())   // Deref: EngineWrapper → Option<RakunEngine>
        .map(|e| e.backend_label())
        .unwrap_or_else(|| "初期化中...".to_string())
}

impl ITfLangBarItem_Impl for TextServiceFactory_Impl {
    fn GetInfo(&self, p: *mut TF_LANGBARITEMINFO) -> windows::core::Result<()> {
        unsafe { *p = language_bar::make_langbar_info(); }
        Ok(())
    }
    fn GetStatus(&self) -> windows::core::Result<u32> { Ok(0) }
    fn Show(&self, _: BOOL) -> windows::core::Result<()> { Ok(()) }
    fn GetTooltipString(&self) -> windows::core::Result<BSTR> {
        let label = current_backend_label();
        Ok(BSTR::from(format!("rakukan [{}]", label)))
    }
}

impl ITfLangBarItemButton_Impl for TextServiceFactory_Impl {
    fn OnClick(&self, _: TfLBIClick, _: &POINT, _: *const RECT) -> windows::core::Result<()> {
        let (tm, tid) = self.inner.try_borrow().ok()
            .and_then(|i| i.thread_mgr.clone().map(|tm| (tm, i.client_id)))
            .unzip();
        if let (Some(tm), Some(tid)) = (tm, tid) {
            unsafe { let _ = toggle_open_close(&tm, tid); }

            // OPENCLOSE を変えた後、IME_STATE.input_mode と同期する。
            // 同期しないと OnTestKeyDown/GetText が古い状態で動作し続ける。
            let now_open = get_open_close(&tm);
            if let Ok(mut st) = crate::engine::state::ime_state_get() {
                use crate::engine::input_mode::InputMode;
                let new_mode = if now_open { InputMode::Hiragana } else { InputMode::Alphanumeric };
                let from = format!("{:?}", st.input_mode);
                st.set_mode(new_mode);
                tracing::info!("OnClick: OPENCLOSE={} input mode: {} → {:?}", now_open as u8, from, st.input_mode);
                diag::event(DiagEvent::ModeChange {
                    from,
                    to: if now_open { "Hiragana" } else { "Alphanumeric" },
                });
            }
        }
        self.notify_langbar_update();

        // open/close 変更をトレイへ通知
        let open = self.inner.try_borrow().ok()
            .and_then(|i| i.thread_mgr.clone().map(|tm| get_open_close(&tm)))
            .unwrap_or(true);
        let mode = crate::engine::state::ime_state_get().ok()
            .map(|s| s.input_mode)
            .unwrap_or_default();
        tray_ipc::publish(open, mode);
        Ok(())
    }
    fn InitMenu(&self, _: Option<&ITfMenu>) -> windows::core::Result<()> { Ok(()) }
    fn OnMenuSelect(&self, _: u32) -> windows::core::Result<()> { Ok(()) }
    fn GetIcon(&self) -> windows::core::Result<HICON> {
        unsafe { language_bar::load_tray_icon() }
    }
    fn GetText(&self) -> windows::core::Result<BSTR> {
        // トレイは1〜2文字しか表示できないためモード文字のみ返す
        // バックエンド情報は GetTooltipString に集約
        let open = self.inner.try_borrow().ok()
            .and_then(|i| i.thread_mgr.clone().map(|tm| get_open_close(&tm)))
            .unwrap_or(true);
        let mode_char = if !open {
            "A"
        } else {
            use crate::engine::state::ime_state_get;
            ime_state_get().ok()
                .map(|s| match s.input_mode {
                    crate::engine::input_mode::InputMode::Hiragana     => "あ",
                    crate::engine::input_mode::InputMode::Katakana     => "ア",
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
            let ptr = dm as *const _ as usize;
            doc_mode_remove(ptr);
            tracing::trace!("OnUninitDocumentMgr: removed dm={ptr:#x}");
        }
        Ok(())
    }

    fn OnSetFocus(
        &self,
        pdimfocus:     Option<&ITfDocumentMgr>,
        pdimprevfocus: Option<&ITfDocumentMgr>,
    ) -> windows::core::Result<()> {
        let next_ptr = pdimfocus.map(|d| d as *const _ as usize).unwrap_or(0);
        let prev_ptr = pdimprevfocus.map(|d| d as *const _ as usize).unwrap_or(0);

        // フォーカス先ウィンドウの HWND を取得（ターミナル判定用）
        let hwnd_val = unsafe { GetForegroundWindow().0 as usize };

        tracing::debug!(
            "OnSetFocus: prev_dm={prev_ptr:#x} next_dm={next_ptr:#x} hwnd={hwnd_val:#x}"
        );

        let Some(new_mode) = doc_mode_on_focus_change(prev_ptr, next_ptr, hwnd_val) else {
            return Ok(());
        };

        // モードを適用
        {
            if let Ok(mut st) = crate::engine::state::ime_state_get() {
                if st.input_mode != new_mode {
                    tracing::info!(
                        "OnSetFocus: mode {:?} → {:?}",
                        st.input_mode, new_mode
                    );
                    st.set_mode(new_mode);
                }
            }
        }

        // KEYBOARD_OPENCLOSE を更新（ターミナルはこれを見てキーをルーティングする）
        {
            use crate::engine::input_mode::InputMode;
            let is_open = new_mode != InputMode::Alphanumeric;
            if let Ok(inner) = self.inner.try_borrow() {
                if let Some(tm) = &inner.thread_mgr {
                    let tid = inner.client_id;
                    unsafe {
                        let _ = language_bar::set_open_close(tm, tid, is_open);
                    }
                }
            }
        }

        // トレイアイコン更新
        tray_ipc::publish(new_mode != crate::engine::input_mode::InputMode::Alphanumeric, new_mode);

        Ok(())
    }

    fn OnPushContext(&self, _pic: Option<&ITfContext>) -> windows::core::Result<()> { Ok(()) }
    fn OnPopContext (&self, _pic: Option<&ITfContext>) -> windows::core::Result<()> { Ok(()) }
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
            return Err(windows::core::Error::new(CONNECT_E_CANNOTCONNECT, "bad cookie"));
        }
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.langbar_sink = None;
        }
        Ok(())
    }
}

pub struct ClassFactory;
impl ClassFactory {
    pub fn create() -> IClassFactory { TextServiceFactory::new().into() }
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
            return Err(windows::core::Error::from(windows::Win32::Foundation::E_INVALIDARG));
        }
        display_attr::get_by_guid(unsafe { &*guid })
    }
}
