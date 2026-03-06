//! rakukan TextService COM オブジェクト
//!
//! # ホットパス原則
//! OnKeyDown / OnTestKeyDown は **絶対にブロックしない**:
//! - try_lock() のみ使用
//! - TF_ES_SYNC を使わない（TF_ES_READWRITE のみ）
//! - BG 変換結果を待たない（準備できていなければ「変換中」を表示して即リターン）

use std::cell::RefCell;

use anyhow::Result;
use windows::{
    core::{implement, Interface, IUnknown, BSTR, GUID},
    Win32::{
        Foundation::{BOOL, E_FAIL, E_INVALIDARG, FALSE, LPARAM, POINT, RECT, TRUE, WPARAM},
        System::{
            Com::{IClassFactory, IClassFactory_Impl},
            Ole::CONNECT_E_CANNOTCONNECT,
        },
        UI::{
            Input::KeyboardAndMouse::GetKeyState,
            TextServices::{
                ITfComposition, ITfCompositionSink, ITfCompositionSink_Impl,
                ITfContext, ITfDocumentMgr, ITfKeyEventSink, ITfKeyEventSink_Impl,
                ITfKeystrokeMgr, ITfLangBarItem, ITfLangBarItem_Impl,
                ITfLangBarItemButton, ITfLangBarItemButton_Impl,
                ITfLangBarItemSink, ITfMenu,
                ITfSource, ITfSource_Impl,
                ITfTextInputProcessor, ITfTextInputProcessor_Impl, ITfThreadMgr,
                ITfThreadMgrEventSink, ITfThreadMgrEventSink_Impl,
                TfLBIClick, TF_LANGBARITEMINFO,
                TF_ES_READWRITE,
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
            engine_try_get_or_create, engine_get_or_create, engine_get,
            selection_try_get,
            session_get, session_is_selecting_fast,
            session_sync_from_selection, selection_sync_from_session,
            caret_rect_get, caret_rect_set,
            doc_mode_on_focus_change, doc_mode_remove,
        },
        user_action::UserAction,
        text_util,
    },
    tsf::{
        candidate_window,
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
            ITfLangBarItemButton, ITfLangBarItem, ITfSource, ITfThreadMgrEventSink)]
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

        // エンジン事前初期化（漢字モデルはバックグラウンドで）
        // エンジン DLL のロードに失敗しても Activate() 自体は成功させる。
        // 失敗すると Windows TSF が TIP を壊れているとマークし選択不可になるため。
        // エンジンなしでも辞書ロード済み or 後ロードで動作継続できる。
        {
            match engine_get_or_create() {
                Ok(mut guard) => {
                    if let Some(engine) = guard.as_mut() {
                        if !engine.is_kanji_ready() { engine.start_load_model(); }
                        if !engine.is_dict_ready()  { engine.start_load_dict(); }
                    }
                }
                Err(e) => {
                    tracing::error!("Activate: engine init failed (continuing without engine): {e}");
                }
            }
        }

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
        if let Ok(mut sel) = selection_try_get() { sel.clear(); }
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
        if let Ok(mut sel) = selection_try_get() { sel.clear(); }
        // BG 変換の converter を先に回収してから（その後 reset_all で状態をクリア）
        // bg_reclaim は reset_all の前に呼ぶ
        // アプリが composition を強制終了した場合（例: メモ帳の最大 composition 長超過）、
        // composition テキストはアプリ側で確定済み。エンジンの hiragana_buf 等は
        // 不要になるため、converter の回収有無に関わらず必ず reset_all() を呼ぶ。
        // ※ 以前は conv が Some の場合に return していたため hiragana_buf が残り、
        //    次のキー入力で古いひらがなが末尾に追加される「途中切れ」バグがあった。
        if let Ok(mut g) = engine_get_or_create() {
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

    /// トレイ常駐プロセスへ現在の入力モードを通知する。
    /// UIスレッド（ホットパス）から呼ばれる可能性があるため、重い処理はしない。
    fn notify_tray_update(&self, tid: u32) {
        // open は TSF コンパートメント（あれば）を優先
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

        let _ = tid; // 将来: client_id を含めた拡張用
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
        // try_get: ブロックしない
        let mut guard = engine_try_get_or_create()?;
        let engine = match guard.as_mut() {
            Some(e) => e,
            None    => return Ok(false),
        };

        // LLM候補待機中に完了した場合、候補ウィンドウを自動更新
        if session_is_selecting_fast() {
            const DICT_LIMIT_POLL: usize = 50;
            if let Ok(mut sel) = selection_try_get() {
                if sel.llm_pending && engine.bg_status() == "done" {
                    let preedit_key = sel.original_preedit.clone();
                    if let Some(llm_cands) = engine.bg_take_candidates(&preedit_key) {
                        let merged = engine.merge_candidates(llm_cands, DICT_LIMIT_POLL);
                        if !merged.is_empty() {
                            let first = merged.first().cloned().unwrap_or_default();
                            sel.candidates  = merged;
                            sel.selected    = 0;
                            sel.llm_pending = false;
                            session_sync_from_selection(&sel);
                            let page_cands = sel.page_candidates().to_vec();
                            let page_info  = sel.page_info();
                            let pos = caret_rect_get();
                            drop(sel);
                            drop(guard);
                            candidate_window::show_with_status(&page_cands, 0, &page_info, pos.left, pos.bottom, None);
                            update_composition(ctx, tid, sink, first)?;
                            return Ok(true);
                        }
                    } else {
                        sel.llm_pending = false;
                    }
                }
            }
        }

        // 辞書0件でLLM完了待機中（選択モード外）→ BG完了したら選択モードへ遷移
        {
            const DICT_LIMIT_WAIT: usize = 50;
            if let Ok(mut sess) = session_get() {
                if let Some((wait_preedit, pos_x, pos_y)) = sess.waiting_info().map(|(t, x, y)| (t.to_string(), x, y)) {
                    if engine.bg_status() == "done" {
                        if let Some(llm_cands) = engine.bg_take_candidates(&wait_preedit) {
                            let merged = engine.merge_candidates(llm_cands, DICT_LIMIT_WAIT);
                            if !merged.is_empty() && !(merged.len() == 1 && merged[0] == wait_preedit) {
                                let first = merged.first().cloned().unwrap_or_default();
                                sess.activate_selecting(merged, wait_preedit.clone(), pos_x, pos_y, false);
                                selection_sync_from_session(&sess);
                                let page_cands = sess.page_candidates().to_vec();
                                let page_info  = sess.page_info();
                                drop(sess);
                                drop(guard);
                                candidate_window::show_with_status(&page_cands, 0, &page_info, pos_x, pos_y, None);
                                update_composition(ctx, tid, sink, first)?;
                                return Ok(true);
                            }
                        }
                        // BG 完了したが候補なし → 待機解除してウィンドウを閉じる
                        sess.set_preedit(wait_preedit);
                        selection_sync_from_session(&sess);
                        drop(sess);
                        candidate_window::hide();
                    }
                }
            }
        }

        match action {
            // ─ 文字入力 ──────────────────────────────────────────────────────
            UserAction::Input(c) => {
                let _t = diag::span("Input");

                // LLM待機中（辞書0件で Space 待ち）に新文字が来たら待機を解除する
                if let Ok(mut sess) = session_get() {
                    if sess.is_waiting() {
                        let pre = sess.preedit_text().unwrap_or("").to_string();
                        sess.set_preedit(pre);
                        selection_sync_from_session(&sess);
                        candidate_window::hide();
                    }
                }

                // 選択モード中に新たなキーが来たら選択モードを解除してコミット
                // AtomicBool で先にロックなしチェックしてから Mutex を取得
                if session_is_selecting_fast() {
                    let mut sess = session_get()?;
                    if sess.is_selecting() {
                        let committed_text = sess.current_candidate()
                            .or_else(|| sess.original_preedit())
                            .unwrap_or("")
                            .to_string();
                        sess.set_idle();
                        selection_sync_from_session(&sess);
                        drop(sess);
                        candidate_window::hide();
                        // エンジン内部状態を確定（UIへの確定挿入は後段の commit_then_start_composition が担当）
                        engine.commit(&committed_text);
                        engine.reset_preedit();
                        // ここで TSF へ確定挿入を行うと、後段の commit_then_start_composition と二重確定になり
                        // 「漢字漢字」のように重複する。
                        // したがって TSF 側の確定挿入は commit_then_start_composition に一本化する。
                        drop(guard);

                        // guard を再取得して次の文字を処理（新しい composition は composition_set 側で作られる）
                        // guard2はこのスコープでのみ保持し、重い処理の前に必ずロックを解放する
                        let (preedit, _hiragana, _committed2, _n_cands2) = {
                            let mut guard2 = engine_try_get_or_create()?;
                            let engine2 = match guard2.as_mut() {
                                Some(e) => e,
                                None => return Ok(true),
                            };

                            engine2.push_char(c);
                            let preedit    = engine2.preedit_display();
                            let _hiragana  = engine2.hiragana_text();
                            let committed2 = engine2.committed_text();
                            let n_cands2   = crate::engine::state::get_num_candidates();

                            diag::event(DiagEvent::InputChar { ch: c, preedit_after: preedit.clone() });

                            engine2.bg_start(n_cands2);
                            (preedit, _hiragana, committed2, n_cands2)
                        };
                        // Bug1修正: end + start を1セッションにまとめる
                        commit_then_start_composition(ctx, tid, sink, committed_text, preedit)?;
                        return Ok(true);
                    }
                }
                // SELECTION_ACTIVE=true だったが is_active()=false の場合はここに来る
                // （競合状態: すでに別スレッドが解除済み → そのまま続行）

                // ポーリング: 辞書・モデルのバックグラウンドロード完了を反映
                engine.poll_dict_ready();
                engine.poll_model_ready();

                engine.push_char(c);
                let preedit   = engine.preedit_display();
                let hiragana  = engine.hiragana_text();
                let _committed = engine.committed_text();
                let n_cands   = crate::engine::state::get_num_candidates();
                diag::event(DiagEvent::InputChar { ch: c, preedit_after: preedit.clone() });
                tracing::trace!("Input: hiragana={:?} bg={}", hiragana, engine.bg_status());

                // ── BG 変換投機起動 ──────────────────────────────────────
                if !hiragana.is_empty() {
                    engine.bg_start(n_cands);
                    drop(guard);
                    tracing::trace!("BG: 起動試行 {:?}", hiragana);
                    update_composition(ctx, tid, sink, preedit)?;
                    return Ok(true);
                }

                drop(guard);
                update_composition(ctx, tid, sink, preedit)?;
                Ok(true)
            }

            UserAction::FullWidthSpace => {
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

            // ─ 変換 ── ブロックしない ─────────────────────────────────────────
            UserAction::Convert => {
                let _t = diag::span("Convert");
                // 候補ウィンドウ配置のためカーソル矩形を取得（Convert時のみ）
                update_caret_rect(ctx.clone(), tid);
                // 末尾 "n" -> "ん" に確定してから変換
                engine.flush_pending_n();
                if engine.preedit_is_empty() { return Ok(false); }
                let preedit     = engine.preedit_display();

                // ── すでに選択モード中 → 1候補ずつ進む（ページ末で次ページへ）──
                {
                    let mut sess = session_get()?;
                    if sess.is_selecting() {
                        sess.next_with_page_wrap();
                        selection_sync_from_session(&sess);
                        let page_cands = sess.page_candidates().to_vec();
                        let page_sel   = sess.page_selected();
                        let page_info  = sess.page_info();
                        let cand_text  = sess.current_candidate()
                            .or_else(|| sess.original_preedit())
                            .unwrap_or("")
                            .to_string();
                        drop(sess);
                        drop(guard);
                        candidate_window::update_selection(page_sel, &page_info);
                        candidate_window::show(&page_cands, page_sel, &page_info,
                            caret_rect_get().left, caret_rect_get().bottom);
                        update_composition(ctx, tid, sink, cand_text)?;
                        return Ok(true);
                    }
                }

                // ── 新規変換: BG変換結果を取得または同期変換 ──
                // LLM に要求する候補数は設定値（デフォルト9）に固定する。
                // 辞書は高速（μs）なので別途 50 件まで引いてマージする。
                let llm_limit   = crate::engine::state::get_num_candidates(); // 9
                const DICT_LIMIT: usize = 50;
                let kanji_ready = engine.is_kanji_ready();
                // BG 変換が idle なら起動する
                if kanji_ready && engine.bg_status() == "idle" {
                    engine.bg_start(llm_limit);
                }

                // ── BG 待機（案A）──
                // BG がまだ running の場合、最大 400ms ブロック待機する。
                // GPU 環境では大抵この範囲で完了し、Space 1回で候補が出る。
                // CPU 環境でタイムアウトしても⏳フォールバックに流れる。
                const BG_WAIT_MS: u64 = 400;
                if kanji_ready && matches!(engine.bg_status(), "running" | "idle") {
                    let completed = engine.bg_wait_ms(BG_WAIT_MS);
                    tracing::debug!("Convert: bg_wait_ms({BG_WAIT_MS}) → completed={completed}");
                }

                tracing::debug!("Convert preedit={preedit:?} kanji_ready={kanji_ready} bg={}", engine.bg_status());

                // BG変換完了ステータスを取得
                let bg_status  = engine.bg_status();
                // モデルロード中、または BG 変換実行中は「変換中...」を表示
                let bg_running = !kanji_ready  // モデルまだロード中
                    || bg_status == "running"
                    || bg_status == "idle";    // 400ms 待機後もタイムアウトした場合

                let (candidates, llm_pending): (Vec<String>, bool) =
                    match engine.bg_take_candidates(&preedit) {
                    Some(llm_cands) if !llm_cands.is_empty() => {
                        let merged = engine.merge_candidates(llm_cands.clone(), DICT_LIMIT);
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
                            // 辞書候補を即時表示し、LLM完了時に自動更新
                            let dict_cands = engine.merge_candidates(vec![], DICT_LIMIT);
                            let dict_empty = dict_cands.is_empty()
                                || (dict_cands.len() == 1 && dict_cands[0] == preedit);

                            if dict_empty && bg_running {
                                // 辞書0件 + LLM未完了：選択モードに入らず待機。
                                // 候補ウィンドウに「⏳ 変換中...」を表示してフィードバックを返す。
                                let caret = caret_rect_get();
                                if let Ok(mut sess) = session_get() {
                                    // すでに待機中なら Space 2回目 → ひらがなをコミット
                                    if sess.is_waiting() {
                                        sess.set_preedit(preedit.clone());
                                        selection_sync_from_session(&sess);
                                        drop(sess);
                                        drop(guard);
                                        candidate_window::hide();
                                        engine_commit_hiragana(ctx, tid)?;
                                        return Ok(true);
                                    }
                                    sess.set_waiting(preedit.clone(), caret.left, caret.bottom);
                                    selection_sync_from_session(&sess);
                                }
                                tracing::debug!("Convert: dict=0, BG running → waiting for LLM (preedit={preedit:?})");
                                // preedit を仮候補として渡すことで候補ウィンドウを表示できる
                                let dummy = vec![preedit.clone()];
                                drop(guard);
                                candidate_window::show_with_status(&dummy, 0, "", caret.left, caret.bottom, Some("⏳ 変換中..."));
                                return Ok(true);
                            }

                            if dict_empty {
                                (vec![preedit.clone()], bg_running)
                            } else {
                                (dict_cands, bg_running)
                            }
                        }
                    }
                };

                let first = candidates.first().cloned().unwrap_or_else(|| preedit.clone());
                diag::event(DiagEvent::Convert {
                    preedit: preedit.clone(), kanji_ready: true, result: first.clone(),
                });

                // 選択モード開始
                let caret = caret_rect_get();
                let pos_x = caret.left;
                let pos_y = caret.bottom;
                let (page_cands, page_info) = {
                    let mut sess = session_get()?;
                    sess.activate_selecting(candidates.clone(), preedit.clone(), pos_x, pos_y, llm_pending);
                    selection_sync_from_session(&sess);
                    (sess.page_candidates().to_vec(), sess.page_info())
                };
                drop(guard);
                let status = if llm_pending { Some("⏳ 変換中...") } else { None };
                candidate_window::show_with_status(&page_cands, 0, &page_info, pos_x, pos_y, status);
                update_composition(ctx, tid, sink, first)?;
                Ok(true)
            }

            // ─ Enter ─────────────────────────────────────────────────────────
            UserAction::CommitRaw => {
                // 選択モード中 → 現在候補をコミット
                {
                    let mut sess = session_get()?;
                    if sess.is_selecting() {
                        let text    = sess.current_candidate()
                            .or_else(|| sess.original_preedit())
                            .unwrap_or("")
                            .to_string();
                        let reading = sess.original_preedit().unwrap_or("").to_string();
                        sess.set_idle();
                        selection_sync_from_session(&sess);
                        drop(sess);
                        candidate_window::hide();
                        // ひらがな以外（プリエディットそのまま確定）は学習しない
                        if text != reading {
                            engine.learn(&reading, &text);
                        }
                        engine.commit(&text);
                        engine.reset_preedit();
                        drop(guard);
                        diag::event(DiagEvent::CommitRaw { preedit: text.clone() });
                        end_composition(ctx, tid, text)?;
                        return Ok(true);
                    }
                }
                // 通常モード → プリエディットをそのままコミット
                // 末尾 "n" -> "ん" に確定してからコミット
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

            // ─ Backspace ─────────────────────────────────────────────────────
            UserAction::Backspace => {
                // 選択モード中 → 選択モードを解除してプリエディット表示に戻す
                {
                    let mut sess = session_get()?;
                    if sess.is_selecting() {
                        let original = sess.original_preedit().unwrap_or("").to_string();
                        sess.set_preedit(original.clone());
                        selection_sync_from_session(&sess);
                        drop(sess);
                        candidate_window::hide();
                        drop(guard);
                        update_composition(ctx, tid, sink, original)?;
                        return Ok(true);
                    }
                    // LLM 待機中なら待機解除（文字が変わるので待ち直し）
                    if sess.is_waiting() {
                        let pre = sess.preedit_text().unwrap_or("").to_string();
                        sess.set_preedit(pre);
                        selection_sync_from_session(&sess);
                        candidate_window::hide();
                    }
                }
                let consumed = engine.backspace();
                if consumed {
                    // Backspace で hiragana が変わった → BG キャッシュを無効化
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

            // ─ Escape / Ctrl+Backspace ───────────────────────────────────────
            UserAction::Cancel | UserAction::CancelAll => {
                // 選択モード中 → 元プリエディットに戻す（キャンセル）
                {
                    let mut sess = session_get()?;
                    if sess.is_selecting() {
                        let original = sess.original_preedit().unwrap_or("").to_string();
                        sess.set_preedit(original.clone());
                        selection_sync_from_session(&sess);
                        drop(sess);
                        candidate_window::hide();
                        drop(guard);
                        update_composition(ctx, tid, sink, original)?;
                        return Ok(true);
                    }
                    // LLM 待機中なら待機解除（プリエディットはそのまま残す）
                    if sess.is_waiting() {
                        let pre = sess.preedit_text().unwrap_or("").to_string();
                        sess.set_preedit(pre);
                        selection_sync_from_session(&sess);
                        candidate_window::hide();
                        tracing::debug!("Cancel: waiting cleared");
                    }
                }
                if engine.preedit_is_empty() { return Ok(false); }
                engine.bg_reclaim();
                engine.reset_all();
                drop(guard);
                end_composition(ctx, tid, String::new())?;
                Ok(true)
            }

            // ─ 文字種変換 ────────────────────────────────────────────────────
            UserAction::Hiragana => {
                engine.flush_pending_n();
                let p = engine.preedit_display();
                if p.is_empty() { return Ok(false); }
                engine.bg_reclaim();
                // ひらがなに戻す（カタカナ→ひらがな変換を含む）
                let t = text_util::to_hiragana(&p);
                engine.force_preedit(t.clone());
                drop(guard);
                update_composition(ctx, tid, sink, t)?; Ok(true)
            }
            UserAction::Katakana => {
                engine.flush_pending_n();
                let p = engine.preedit_display();
                if p.is_empty() { return Ok(false); }
                engine.bg_reclaim();
                let t = text_util::to_katakana(&p);
                engine.force_preedit(t.clone());
                drop(guard);
                update_composition(ctx, tid, sink, t)?; Ok(true)
            }
            UserAction::HalfKatakana => {
                engine.flush_pending_n();
                let p = engine.preedit_display();
                if p.is_empty() { return Ok(false); }
                engine.bg_reclaim();
                let t = text_util::to_half_katakana(&p);
                engine.force_preedit(t.clone());
                drop(guard);
                update_composition(ctx, tid, sink, t)?; Ok(true)
            }
            UserAction::FullLatin => {
                engine.flush_pending_n();
                let p = engine.preedit_display();
                if p.is_empty() { return Ok(false); }
                engine.bg_reclaim();
                let t = text_util::to_full_latin(&p);
                engine.force_preedit(t.clone());
                drop(guard);
                update_composition(ctx, tid, sink, t)?; Ok(true)
            }
            UserAction::HalfLatin => {
                engine.flush_pending_n();
                let p = engine.preedit_display();
                if p.is_empty() { return Ok(false); }
                engine.bg_reclaim();
                let t = text_util::to_half_latin(&p);
                engine.force_preedit(t.clone());
                drop(guard);
                update_composition(ctx, tid, sink, t)?; Ok(true)
            }
            UserAction::CycleKana => {
                let p = engine.preedit_display();
                if p.is_empty() { return Ok(false); }
                engine.bg_reclaim();
                let t = text_util::to_katakana(&p);
                engine.commit(&t); engine.reset_preedit(); drop(guard);
                end_composition(ctx, tid, t)?; Ok(true)
            }

            // ─ 候補操作（Phase 4）────────────────────────────────────────────
            UserAction::CandidateNext => {
                let mut sess = session_get()?;
                if !sess.is_selecting() { return Ok(!engine.preedit_is_empty()); }
                sess.next_with_page_wrap();
                selection_sync_from_session(&sess);
                let page_cands = sess.page_candidates().to_vec();
                let page_sel   = sess.page_selected();
                let page_info  = sess.page_info();
                let text = sess.current_candidate()
                    .or_else(|| sess.original_preedit()).unwrap_or("").to_string();
                drop(sess);
                drop(guard);
                candidate_window::update_selection(page_sel, &page_info);
                candidate_window::show(&page_cands, page_sel, &page_info,
                    caret_rect_get().left, caret_rect_get().bottom);
                update_composition(ctx, tid, sink, text)?;
                Ok(true)
            }

            UserAction::CandidatePrev => {
                let mut sess = session_get()?;
                if !sess.is_selecting() { return Ok(!engine.preedit_is_empty()); }
                sess.prev();
                selection_sync_from_session(&sess);
                let page_cands = sess.page_candidates().to_vec();
                let page_sel   = sess.page_selected();
                let page_info  = sess.page_info();
                let text = sess.current_candidate()
                    .or_else(|| sess.original_preedit()).unwrap_or("").to_string();
                drop(sess);
                drop(guard);
                candidate_window::update_selection(page_sel, &page_info);
                candidate_window::show(&page_cands, page_sel, &page_info,
                    caret_rect_get().left, caret_rect_get().bottom);
                update_composition(ctx, tid, sink, text)?;
                Ok(true)
            }

            UserAction::CandidatePageDown => {
                let mut sess = session_get()?;
                if !sess.is_selecting() { return Ok(!engine.preedit_is_empty()); }
                sess.next_page();
                selection_sync_from_session(&sess);
                let page_cands = sess.page_candidates().to_vec();
                let page_sel   = sess.page_selected();
                let page_info  = sess.page_info();
                let text = sess.current_candidate()
                    .or_else(|| sess.original_preedit()).unwrap_or("").to_string();
                drop(sess);
                drop(guard);
                let caret = caret_rect_get();
                candidate_window::show(&page_cands, page_sel, &page_info, caret.left, caret.bottom);
                update_composition(ctx, tid, sink, text)?;
                Ok(true)
            }

            UserAction::CandidatePageUp => {
                let mut sess = session_get()?;
                if !sess.is_selecting() { return Ok(!engine.preedit_is_empty()); }
                sess.prev_page();
                selection_sync_from_session(&sess);
                let page_cands = sess.page_candidates().to_vec();
                let page_sel   = sess.page_selected();
                let page_info  = sess.page_info();
                let text = sess.current_candidate()
                    .or_else(|| sess.original_preedit()).unwrap_or("").to_string();
                drop(sess);
                drop(guard);
                let caret = caret_rect_get();
                candidate_window::show(&page_cands, page_sel, &page_info, caret.left, caret.bottom);
                update_composition(ctx, tid, sink, text)?;
                Ok(true)
            }

            UserAction::CandidateSelect(n) => {
                let mut sess = session_get()?;
                if !sess.is_selecting() { return Ok(!engine.preedit_is_empty()); }
                if !sess.select_nth_in_page(n as usize) { return Ok(true); }
                let text = sess.current_candidate()
                    .or_else(|| sess.original_preedit()).unwrap_or("").to_string();
                let reading = sess.original_preedit().unwrap_or("").to_string();
                sess.set_idle();
                selection_sync_from_session(&sess);
                drop(sess);
                candidate_window::hide();
                if text != reading {
                    engine.learn(&reading, &text);
                }
                engine.commit(&text);
                engine.reset_preedit();
                drop(guard);
                diag::event(DiagEvent::Convert {
                    preedit: text.clone(), kanji_ready: true, result: text.clone(),
                });
                end_composition(ctx, tid, text)?;
                Ok(true)
            }

            UserAction::CursorLeft | UserAction::CursorRight => Ok(!engine.preedit_is_empty()),

            // ─ IME トグル ────────────────────────────────────────────────────
            // KEYBOARD_OPENCLOSE をモードに応じて更新する。
            // Alphanumeric 時は 0、かな入力時は 1。ターミナル等はこの値を参照する。
            UserAction::ImeToggle => {
                let preedit = engine.preedit_display();
                if !preedit.is_empty() {
                    engine.bg_reclaim();
                    engine.commit(&preedit.clone()); engine.reset_preedit();
                    drop(guard);
                    end_composition(ctx, tid, preedit)?;
                } else { drop(guard); }
                let (from, to, now_open) = if let Ok(mut st) = crate::engine::state::ime_state_get() {
                    use crate::engine::input_mode::InputMode;
                    let was_alpha = st.input_mode == InputMode::Alphanumeric;
                    let new_mode = if was_alpha { InputMode::Hiragana } else { InputMode::Alphanumeric };
                    let from = format!("{:?}", st.input_mode);
                    st.set_mode(new_mode);
                    (from, if was_alpha { "Hiragana" } else { "Alphanumeric" }, was_alpha)
                } else { ("unknown".into(), "unknown", true) };
                // TSF コンパートメントも更新（ターミナル等が参照する）
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

            UserAction::ImeOff | UserAction::ModeAlphanumeric => {
                let preedit = engine.preedit_display();
                if !preedit.is_empty() {
                    engine.bg_reclaim();
                    engine.commit(&preedit.clone()); engine.reset_preedit();
                    drop(guard);
                    end_composition(ctx, tid, preedit)?;
                } else { drop(guard); }
                if let Ok(mut st) = crate::engine::state::ime_state_get() {
                    let from = format!("{:?}", st.input_mode);
                    st.set_mode(crate::engine::input_mode::InputMode::Alphanumeric);
                    diag::event(DiagEvent::ModeChange { from, to: "Alphanumeric" });
                }
                // TSF コンパートメントを閉じる（ターミナル等が参照する）
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

            UserAction::ImeOn => {
                drop(guard);
                if let Ok(mut st) = crate::engine::state::ime_state_get() {
                    let from = format!("{:?}", st.input_mode);
                    st.set_mode(crate::engine::input_mode::InputMode::Hiragana);
                    diag::event(DiagEvent::ModeChange { from, to: "Hiragana" });
                }
                // TSF コンパートメントを開く
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

            UserAction::ModeHiragana => {
                let preedit = engine.preedit_display();
                if !preedit.is_empty() {
                    let t = preedit.clone();
                    engine.bg_reclaim();
                    engine.commit(&t); engine.reset_preedit(); drop(guard);
                    end_composition(ctx, tid, t)?;
                } else { drop(guard); }
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

            UserAction::ModeKatakana => {
                let preedit = engine.preedit_display();
                if !preedit.is_empty() {
                    let t = text_util::to_katakana(&preedit);
                    engine.bg_reclaim();
                    engine.commit(&t); engine.reset_preedit(); drop(guard);
                    end_composition(ctx, tid, t)?;
                } else { drop(guard); }
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

            _ => Ok(false),
        }
    }
}

// ─── 変換ヘルパー ─────────────────────────────────────────────────────────────

/// 複数候補を返す版（候補ウィンドウ用）
/// プリエディット（ひらがな）をそのまま確定してコンポジションを終了する。
/// 辞書0件 + LLM 待機中に Space を2回押したときの逃げ道として使用する。
fn engine_commit_hiragana(ctx: ITfContext, tid: u32) -> Result<()> {
    let preedit = {
        let mut guard = engine_get_or_create()
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
                selection_sync_from_session(&sess);
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
        UserAction::Input(_) | UserAction::FullWidthSpace => true,
        UserAction::Backspace => has_preedit,
        UserAction::ImeToggle | UserAction::ImeOff | UserAction::ImeOn
        | UserAction::ModeHiragana | UserAction::ModeKatakana | UserAction::ModeAlphanumeric => true,
        UserAction::Convert | UserAction::CommitRaw | UserAction::Cancel | UserAction::CancelAll
        | UserAction::Hiragana | UserAction::Katakana | UserAction::HalfKatakana
        | UserAction::FullLatin | UserAction::HalfLatin | UserAction::CycleKana
        | UserAction::CandidateNext | UserAction::CandidatePrev
        | UserAction::CandidatePageDown | UserAction::CandidatePageUp
        | UserAction::CursorLeft | UserAction::CursorRight => has_preedit,
        UserAction::CandidateSelect(_) => has_preedit,
        _ => false,
    }
}

#[inline]
fn action_name(a: &UserAction) -> &'static str {
    match a {
        UserAction::Input(_)           => "Input",
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
        tracing::trace!("SetText(update): {:?}", preedit);

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
        // まれに composition ハンドルが失われる（アプリ/TSFの競合）ことがある。
        // その場合でも確定テキストが消えないよう、カーソル位置へ直接コミットする。
        let commit_w: Vec<u16> = commit_text.encode_utf16().collect();
        if let Some(comp) = comp {
            // composition 内のテキストは候補表示で最新化されている前提。
            // ここで SetText すると、アプリによっては確定が二重化することがあるため EndComposition のみにする。
            comp.EndComposition(ec)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("EndComposition: {e}")))?;
        } else if !commit_text.is_empty() {
            let insert_point = get_cursor_range(&ctx, ec)
                .unwrap_or_else(|| ctx.GetEnd(ec).unwrap());
            insert_point.SetText(ec, 0, &commit_w)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText direct commit: {e}")))?;
        }

        if next_preedit.is_empty() {
            return Ok(());
        }

        // ── Step2: 同セッション内で新 composition 開始 ──
        let insert_point = get_cursor_range(&ctx, ec)
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
