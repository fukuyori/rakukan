//! `handle_action`: ユーザアクションを各 on_* ハンドラへ振り分ける dispatcher。
//!
//! M3 (T1-A) で factory.rs から純粋切り出し。動作変更なし。
//! 各 on_* メソッドは on_input / on_convert / edit_ops に配置されている。

use anyhow::Result;
use windows::Win32::UI::TextServices::{ITfCompositionSink, ITfContext};

use crate::engine::state::{
    CandidateViewSource, SessionState, caret_rect_get, engine_try_get_or_create, session_get,
    session_is_selecting_fast,
};
use crate::engine::text_util;
use crate::engine::user_action::UserAction;
use crate::tsf::candidate_window;

use super::{CandidateDir, action_name, update_composition, update_composition_candidate_parts};

impl super::TextServiceFactory_Impl {
    pub(super) fn handle_action(
        &self,
        action: UserAction,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
    ) -> Result<bool> {
        // 結果が出たまま後処理されていない確定があれば、先に後処理する（入力処理の見直しの
        // 段 5。通常は WM_APP_COMMIT_DONE で済んでいる。メッセージを送れなかった場合の保険）
        crate::tsf::commit_txn::finish_completed();
        let mut guard = match engine_try_get_or_create() {
            Ok(guard) => guard,
            Err(e) => {
                // composition を確かめられなければ、あるものとして扱う（保留する側へ倒す）
                let composing = crate::engine::state::composition_held().unwrap_or(true);
                // IME のモードキー: composition が無ければ確定するものも無いので、エンジン
                // 無しで切り替える（設計書 4.2）。composition があれば下で保留する
                if super::is_ime_mode_action(&action) && !composing {
                    let new = match action {
                        UserAction::ImeOff => crate::engine::ime_mode::ImeMode::Off,
                        UserAction::ImeOn => crate::engine::ime_mode::ImeMode::On,
                        _ => crate::engine::state::ime_mode_get_atomic().toggled(),
                    };
                    return self.switch_ime(None, tid, new);
                }
                // 直接届いたキーは、今の入力の有無で IME が食べるキーだけを保留する
                // （OnTestKeyDown と同じ判定。食べないキーは今までどおりアプリへ渡す）
                if !crate::tsf::key_queue::in_replay()
                    && !super::key_should_eat(
                        &action,
                        composing || crate::engine::state::session_is_selecting_fast(),
                    )
                {
                    return Ok(false);
                }
                // エンジンの準備中・ロックの競合（#70、入力処理の見直しの段 6）: キーを
                // アプリへ渡さず、止めて保留の列に入れ、エンジンが使えるようになってから
                // 実行する（保持の上限を過ぎたら key_queue が回復できない場合の処置をする）
                tracing::debug!("engine unavailable ({e}); {} held", action_name(&action));
                crate::tsf::key_queue::suspend_current();
                super::show_engine_waiting();
                return Ok(true);
            }
        };
        // 所有権を復元してよいのは、フォーカス中の生存している文脈へのキー入力だけ。
        let focused_dm = (|| {
            use windows::core::Interface;
            let tm = self.inner.try_borrow().ok()?.thread_mgr.clone()?;
            let focused = unsafe { tm.GetFocus() }.ok()?;
            let current = unsafe { ctx.GetDocumentMgr() }.ok()?;
            if focused.as_raw() != current.as_raw() {
                return None;
            }
            let dm = crate::engine::dm_registry::lookup(current.as_raw() as usize)?;
            crate::engine::dm_registry::is_live(dm).then_some(dm)
        })();
        let focused_context = focused_dm.is_some();
        if let Some(dm) = focused_dm {
            candidate_window::bind_input_focus(dm, super::foreground_root_hwnd());
        }
        // 読みの長さの上限（入力処理の見直しの段 5 の 5d、設計書 4.3）: 上限に達した読みに
        // 文字を足すキーは、先に今の composition を確定してから実行する。確定が後で実行
        // される場合は、このキーを保留の列に戻し、結果の後で実行する
        // 判定はキーの文書へ結び付けた後で、その文書の読みに対して行う
        if focused_context
            && is_text_input(&action)
            && !crate::engine::state::session_is_selecting_fast()
            && let Some(len) = reading_at_limit()
        {
            tracing::info!(
                "length limit: committing a reading of {len} chars before {}",
                action_name(&action)
            );
            // 確定はエンジンを取り直すので、ここで放す
            drop(guard);
            let committed =
                self.handle_action(UserAction::CommitRaw, ctx.clone(), tid, sink.clone());
            // 再生の中で確定がエンジンを取れなかった場合の止まった印は、確定のものなので消す
            let commit_not_run = crate::tsf::key_queue::take_suspended();
            match committed {
                // 確定できなかった（エンジンを取れなかったなど）: 上限を少し超えるが、この
                // キーは今までどおり実行する（次の文字キーでもう一度確定を試す）
                Err(e) => tracing::warn!("length limit: commit failed; key handled as is: {e}"),
                Ok(_) if commit_not_run => {
                    tracing::warn!("length limit: commit not run (engine busy); key handled as is")
                }
                Ok(_) if crate::tsf::commit_txn::commit_pending() => {
                    crate::tsf::key_queue::suspend_current();
                    return Ok(true);
                }
                Ok(_) => {}
            }
            // エンジンを取り直せなければ、このキーを止めて列に戻し、後で実行する
            guard = match engine_try_get_or_create() {
                Ok(guard) => guard,
                Err(e) => {
                    tracing::warn!("length limit: engine busy after the commit; key held: {e}");
                    crate::tsf::key_queue::suspend_current();
                    return Ok(true);
                }
            };
        }
        let _input_scope = crate::engine::rpc_composition::InputScope::enter(focused_context);
        // アプリ側の確定は、復元も状態の確認も通さない。
        if matches!(action, UserAction::CommitRaw) {
            return self.on_commit_raw(ctx, tid, sink, guard);
        }
        let engine = match guard.as_mut() {
            Some(e) => e,
            None => return Ok(false),
        };
        // 入口の読み取りは 1 回。所有権の確認で得た BG 状態を、下の診断ログでも使う。
        let entry = if focused_context {
            match engine.recover_on_input() {
                Ok(entry) => Some(entry),
                Err(e) => {
                    // ホストが応えない（入力処理の見直しの段 6）: エンジンの準備中と同じく、
                    // IME が食べるキーはアプリへ渡さず、止めて保留の列に入れる。journal の
                    // 末尾が確かめられないまま上限を超えていれば、回復できない場合に進む
                    let composing = crate::engine::state::composition_held().unwrap_or(true);
                    if !crate::tsf::key_queue::in_replay()
                        && !super::key_should_eat(
                            &action,
                            composing || crate::engine::state::session_is_selecting_fast(),
                        )
                    {
                        return Ok(false);
                    }
                    if let Some(reason) = crate::engine::rpc_composition::journal_stuck() {
                        tracing::warn!(
                            "host not answering and {reason}; {} goes to the unrecoverable path: {e}",
                            action_name(&action)
                        );
                        crate::tsf::key_queue::suspend_unrecoverable();
                    } else {
                        tracing::debug!("host not answering ({e}); {} held", action_name(&action));
                        crate::tsf::key_queue::suspend_current();
                    }
                    super::show_engine_waiting();
                    return Ok(true);
                }
            }
        } else {
            None
        };
        // 復元直後の処置（入力処理の見直しの段 7、設計書 4.7 の表）
        if let Some(entry) = entry.as_ref().filter(|entry| entry.restored) {
            // BG の処置（ホストが BG 変換の所有者を消している）: タイマーを止め、preview の列を
            // 空にし、変換の世代を進め（遅れて届く前の preview を当てない）、モデル待ちを消す。
            // セッションが持つ LiveConv の preview そのものは、Exact なら保つ（4.7 の表）
            candidate_window::stop_waiting_timer();
            candidate_window::stop_live_timer();
            crate::tsf::live_session::queue_preview_clear();
            crate::tsf::live_session::conv_gen_bump();
            candidate_window::clear_model_wait();
            if let Some(edit) = crate::engine::rpc_composition::local_edit() {
                let display = format!("{}{}", edit.reading, edit.pending_romaji);
                let plan = session_get()
                    .map(|session| after_restore(entry.rebased, &session))
                    .unwrap_or(AfterRestore::ToPreedit);
                match plan {
                    // Exact で変換中・ライブ変換: セッションと候補ウィンドウを保ち、今のキーは
                    // 通常どおり（↓ は次の候補へ）
                    AfterRestore::Keep => {
                        if let Ok(mut session) = session_get() {
                            session.clear_llm_pending();
                        }
                        if session_get().is_ok_and(|s| s.is_live_conv()) {
                            let _ =
                                crate::engine::state::start_live_bg_if_ready(engine, &edit.reading);
                        }
                    }
                    // Exact で Preedit / Waiting: Preedit にして（Waiting なら候補ウィンドウを
                    // 隠し）、今のキーは通常どおり（Space は変換する）
                    AfterRestore::ToPreedit => {
                        if let Ok(mut session) = session_get() {
                            session.set_preedit(display.clone());
                        }
                        candidate_window::hide();
                        let _ = crate::engine::state::start_live_bg_if_ready(engine, &edit.reading);
                    }
                    // Rebased: セッションの全体の読みで Preedit にし、表示を作り直す。文字を
                    // 生むキーは続け、表示に依存する操作（Space・候補・F キー・Backspace・Esc・
                    // カーソル）は修復だけで終える（押した時点の表示がもう無い。4.1 の AfterRepair）
                    AfterRestore::Repair => {
                        let full = session_get()
                            .ok()
                            .and_then(|session| session.conversion_full_reading())
                            .filter(|full| !full.is_empty());
                        let display = match full {
                            Some(full) if full != edit.reading => {
                                engine.force_preedit(full.clone())?;
                                full
                            }
                            _ => display,
                        };
                        if let Ok(mut session) = session_get() {
                            session.set_preedit(display.clone());
                        }
                        candidate_window::hide();
                        if !continues_after_repair(&action) {
                            drop(guard);
                            update_composition(ctx, tid, sink, display)?;
                            return Ok(true);
                        }
                    }
                }
            }
        }

        // ── 診断: 全アクションの入口でセッション状態とBG状態をログ ──
        {
            let bg = match entry.and_then(|entry| entry.bg) {
                Some(bg) => Ok(bg),
                None => engine.bg_status(),
            };
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
                    SessionState::LiveConv {
                        reading,
                        preview,
                        preview_for,
                    } => {
                        format!(
                            "LiveConv(r={:?} p={:?} for={:?})",
                            reading, preview, preview_for
                        )
                    }
                    SessionState::RangeSelect {
                        full_reading,
                        select_end,
                        ..
                    } => {
                        format!("RangeSelect(r={:?} end={})", full_reading, select_end)
                    }
                    SessionState::BlockSelecting {
                        current_index,
                        blocks,
                        ..
                    } => {
                        format!(
                            "BlockSelecting(idx={} nblocks={})",
                            current_index,
                            blocks.len()
                        )
                    }
                }
            } else {
                "lock_err".to_string()
            };
            tracing::debug!(
                "handle_action: {:?} state={} bg={:?} hira={:?}",
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
        // キューエントリには書き込み時点の gen / reading / session_nonce が添えられており、
        // 現在値と一致しない場合は stale として discard する:
        // - gen 不一致 (M1.8 T-MID1): reading が進んでいるのに古い preview で中間を上書き
        // - reading 不一致 (M1.8 T-MID1): gen 一致でも reading が違う場合の二重防壁
        // - session_nonce 不一致 (M2 §5.3): composition が破棄→再生成された後に
        //   古い preview がキューに残って次の composition に紛れ込む経路を断つ
        {
            use crate::tsf::live_session::{
                conv_gen_snapshot, queue_preview_consume, session_nonce_snapshot,
            };
            if let Some(entry) = queue_preview_consume() {
                // CommitRaw (Enter) では適用しない: ここで適用すると、まだ画面に
                // 出ていない preview をこの Enter 自身が LiveConv 化してそのまま
                // 確定し、「表示=ひらがな、確定=変換済み」の不一致になる。
                // 確定は表示済みテキストのみ (WYSIWYG)。entry は破棄する。
                // stale 判定
                let current_gen = conv_gen_snapshot();
                let current_nonce = session_nonce_snapshot();
                let current_reading = engine.hiragana_text()?.to_string();
                let stale_gen = entry.gen_when_requested != current_gen;
                let stale_reading = entry.reading != current_reading;
                let stale_nonce = entry.session_nonce_at_request != current_nonce;
                if matches!(action, UserAction::CommitRaw) {
                    tracing::info!(
                        "[Live] Phase1B: discard undisplayed preview on CommitRaw preview={:?}",
                        entry.preview
                    );
                } else if stale_gen || stale_reading || stale_nonce {
                    tracing::warn!(
                        "[Live] Phase1B: discarded stale preview entry_gen={} cur_gen={} entry_nonce={} cur_nonce={} entry_reading={:?} cur_reading={:?}",
                        entry.gen_when_requested,
                        current_gen,
                        entry.session_nonce_at_request,
                        current_nonce,
                        entry.reading,
                        current_reading
                    );
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
                        let reading = engine.hiragana_text()?.to_string();
                        let preedit_full = engine.preedit_display()?;
                        let pending = text_util::suffix_after_prefix_or_empty(
                            &preedit_full,
                            &reading,
                            "phase1b pending",
                        )
                        .to_string();
                        if !reading.is_empty() {
                            tracing::info!(
                                "[Live] Phase1B: applying preview={:?} reading={:?} pending={:?}",
                                preview,
                                reading,
                                pending
                            );
                            if let Ok(mut sess) = session_get() {
                                sess.set_live_conv(reading.clone(), preview.clone(), reading);
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
                    if llm_pending && engine.bg_status()? == rakukan_engine_rpc::BgView::Done {
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
                    match engine.bg_take_candidates(&preedit_key)? {
                        Some(llm_cands) => {
                            tracing::debug!(
                                "poll: bg_take_candidates → Some({} cands)",
                                llm_cands.len()
                            );
                            // 読みを明示的に渡す（merge_candidates() は内部
                            // バッファ参照のため、ユーザー辞書・学習履歴が
                            // 反映されず LLM 候補だけになる）。
                            let merged = engine.merge_candidates_for_reading(
                                &preedit_key,
                                llm_cands,
                                DICT_LIMIT_POLL,
                            );
                            tracing::debug!("poll: merge_candidates → {:?}", merged);
                            if !merged.is_empty() {
                                sess.replace_selecting_candidates(merged, CandidateViewSource::Bg);
                                if let SessionState::Selecting {
                                    ref mut llm_pending,
                                    ..
                                } = *sess
                                {
                                    *llm_pending = false;
                                }
                                let page_cands = sess.page_candidates().to_vec();
                                let page_selected = sess.page_selected();
                                let page_info = sess.page_info();
                                let cand_text = sess
                                    .current_candidate()
                                    .or_else(|| sess.original_preedit())
                                    .unwrap_or("")
                                    .to_string();
                                let candidate_view = sess.current_candidate_view().cloned();
                                let prefix = sess.selecting_prefix_clone();
                                let remainder = sess.selecting_remainder_clone();
                                let pos = caret_rect_get();
                                drop(sess);
                                drop(guard);
                                candidate_window::show_with_status(
                                    &page_cands,
                                    page_selected,
                                    &page_info,
                                    pos.left,
                                    pos.bottom,
                                    None,
                                );
                                if let Some(view) = candidate_view {
                                    tracing::info!(
                                        "candidate_display_probe event=pending_update reading_len={} source={} first_candidate={:?} page_selected={} selected_candidate={:?} composition_candidate={:?} selected_match={} llm_pending=false corresponding_reading_len={} suffix_len={}",
                                        preedit_key.chars().count(),
                                        view.source.as_str(),
                                        page_cands.first().map(String::as_str).unwrap_or(""),
                                        page_selected,
                                        cand_text,
                                        cand_text,
                                        true,
                                        view.corresponding_reading_len,
                                        view.suffix.chars().count()
                                    );
                                }
                                update_composition_candidate_parts(
                                    ctx, tid, sink, prefix, cand_text, remainder,
                                )?;
                                return Ok(true);
                            }
                        }
                        None => {
                            // take_ready がキー不一致で None を返した: Done 状態は保持されたまま
                            // llm_pending はそのままにしておく（次のキー/Space で再試行できる）
                            tracing::warn!(
                                "poll: bg_take_candidates → None (key mismatch or lock busy), bg={}",
                                engine.bg_status()?
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
            if let Ok(mut sess) = session_get()
                && let Some((wait_preedit, pos_x, pos_y)) =
                    sess.waiting_info().map(|(t, x, y)| (t.to_string(), x, y))
            {
                let bg_now = engine.bg_status()?;
                tracing::debug!(
                    "waiting-poll: wait_preedit={:?} bg={}",
                    wait_preedit,
                    bg_now
                );
                if bg_now == rakukan_engine_rpc::BgView::Done {
                    tracing::debug!(
                        "waiting-poll: calling bg_take_candidates({:?})",
                        wait_preedit
                    );
                    // bg のキーは hiragana_buf。wait_preedit は preedit_display()（未確定
                    // ローマ字を含む。例「たt」）なので、不一致なら hiragana_text() で再試行し、
                    // 取れたキーを読みにする（Step 10-5。candidate_window の on_waiting_timer と同じ）。
                    let hira_key = engine.hiragana_text()?.to_string();
                    let taken = match engine.bg_take_candidates(&wait_preedit)? {
                        Some(c) => Some((wait_preedit.clone(), c)),
                        None if hira_key != wait_preedit => {
                            tracing::debug!(
                                "waiting-poll: key mismatch, retry hira={:?}",
                                hira_key
                            );
                            engine
                                .bg_take_candidates(&hira_key)?
                                .map(|c| (hira_key.clone(), c))
                        }
                        None => None,
                    };
                    match taken {
                        Some((matched_key, llm_cands)) => {
                            tracing::debug!("waiting-poll: got {} LLM cands", llm_cands.len());
                            // LLM候補とマージ。llm_cands が空でも辞書候補がある場合はそちらを使う。
                            let merged = if llm_cands.is_empty() {
                                engine.merge_candidates_for_reading(
                                    &matched_key,
                                    vec![],
                                    DICT_LIMIT_WAIT,
                                )
                            } else {
                                engine.merge_candidates_for_reading(
                                    &matched_key,
                                    llm_cands,
                                    DICT_LIMIT_WAIT,
                                )
                            };
                            tracing::debug!("waiting-poll: merged={} cands", merged.len());
                            // preedit 1件だけでも候補ウィンドウを出す（辞書/LLMどちらかにヒットした）
                            if !merged.is_empty() {
                                let first = merged.first().cloned().unwrap_or_default();
                                // 未確定ローマ字は接尾辞として候補の後ろに残す（Step 10-5）
                                let suffix = crate::engine::state::pending_suffix_display(
                                    &wait_preedit,
                                    &matched_key,
                                );
                                sess.activate_selecting_with_affixes(
                                    merged,
                                    matched_key.clone(),
                                    pos_x,
                                    pos_y,
                                    false,
                                    String::new(),
                                    String::new(),
                                    suffix.clone(),
                                    String::new(),
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
                                update_composition_candidate_parts(
                                    ctx,
                                    tid,
                                    sink,
                                    String::new(),
                                    first,
                                    suffix,
                                )?;
                                return Ok(true);
                            }
                        }
                        None => {
                            // キー不一致 or ロック競合 → Done 状態は保持されたまま
                            // Waiting 状態を維持して次のキー/Space で再試行
                            tracing::warn!(
                                "waiting-poll: bg_take_candidates → None (key mismatch?), bg={}",
                                engine.bg_status()?
                            );
                        }
                    }
                    // merged が空（LLM候補なし）だった場合のみ preedit に戻す
                    // None だった場合は Waiting を維持（→ Cancel や次のSpace で対処）
                }
            }
        } // if !is_cancel

        match action {
            UserAction::Input(c) => {
                if let Some(symbol) = text_util::direct_input_symbol(c) {
                    self.on_punctuate(symbol, ctx, tid, sink, guard)
                } else {
                    self.on_input(c, ctx, tid, sink, guard)
                }
            }
            UserAction::InputRaw(c) => {
                if let Some(symbol) = text_util::direct_input_symbol(c) {
                    self.on_punctuate(symbol, ctx, tid, sink, guard)
                } else {
                    self.on_input_raw(c, ctx, tid, sink, guard)
                }
            }
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
            UserAction::CursorHome => self.on_cursor_jump(ctx, tid, sink, guard, false),
            UserAction::CursorEnd => self.on_cursor_jump(ctx, tid, sink, guard, true),
            UserAction::Punctuate(c) => self.on_punctuate(c, ctx, tid, sink, guard),
            UserAction::SegmentShrink => self.on_segment_shrink(ctx, tid, sink, guard),
            UserAction::SegmentExtend => self.on_segment_extend(ctx, tid, sink, guard),
            UserAction::ImeToggle => {
                drop(guard);
                self.on_ime_toggle(ctx, tid)
            }
            UserAction::ImeOff => {
                drop(guard);
                self.on_ime_off(ctx, tid)
            }
            UserAction::ImeOn => {
                drop(guard);
                self.on_ime_on(ctx, tid)
            }
            _ => Ok(false),
        }
    }
}

/// 読みの長さの上限（文字数。設計書 4.3）。
const READING_LIMIT: usize = 2_000;

/// 文字を生む（読みを伸ばす）キーか。
/// 復元直後の処置（設計書 4.7 の表）。
#[derive(Debug, PartialEq, Eq)]
enum AfterRestore {
    /// セッションと候補ウィンドウを保つ（Exact で変換中・ライブ変換）。
    Keep,
    /// Preedit にする（Exact で Preedit / Waiting / それ以外）。
    ToPreedit,
    /// 全体の読みで Preedit にし、表示を作り直す（Rebased）。
    Repair,
}

fn after_restore(rebased: bool, session: &SessionState) -> AfterRestore {
    if rebased {
        return AfterRestore::Repair;
    }
    match session {
        SessionState::Selecting { .. }
        | SessionState::BlockSelecting { .. }
        | SessionState::RangeSelect { .. }
        | SessionState::LiveConv { .. } => AfterRestore::Keep,
        _ => AfterRestore::ToPreedit,
    }
}

/// Rebased の修復の後も、今のキーを続けて実行するか（文字を生むキーとモードキー）。
fn continues_after_repair(action: &UserAction) -> bool {
    is_text_input(action) || super::is_ime_mode_action(action)
}

fn is_text_input(action: &UserAction) -> bool {
    matches!(
        action,
        UserAction::Input(_)
            | UserAction::InputRaw(_)
            | UserAction::FullWidthSpace
            | UserAction::Punctuate(_)
    )
}

/// 今の読みが上限に達していれば、その文字数を返す。
fn reading_at_limit() -> Option<usize> {
    at_limit(crate::engine::rpc_composition::local_reading_len()?)
}

fn at_limit(len: usize) -> Option<usize> {
    (len >= READING_LIMIT).then_some(len)
}

#[cfg(test)]
mod length_limit_tests {
    use super::*;

    #[test]
    fn only_text_input_at_the_limit_commits_first() {
        assert_eq!(at_limit(READING_LIMIT - 1), None);
        assert_eq!(at_limit(READING_LIMIT), Some(READING_LIMIT));
        assert!(is_text_input(&UserAction::Input('a')));
        assert!(is_text_input(&UserAction::Punctuate('。')));
        assert!(!is_text_input(&UserAction::Backspace));
        assert!(!is_text_input(&UserAction::CommitRaw));
    }
}

#[cfg(test)]
mod after_restore_tests {
    use super::*;

    #[test]
    fn the_session_after_a_restore_follows_the_table() {
        let preedit = SessionState::Preedit { text: "か".into() };
        let range = SessionState::RangeSelect {
            full_reading: "しかん".into(),
            select_end: 1,
            original_preview: String::new(),
        };
        // Exact: 変換中は保ち、Preedit・Idle は Preedit にする
        assert_eq!(after_restore(false, &range), AfterRestore::Keep);
        assert_eq!(after_restore(false, &preedit), AfterRestore::ToPreedit);
        assert_eq!(
            after_restore(false, &SessionState::Idle),
            AfterRestore::ToPreedit
        );
        // Rebased: どれでも修復
        assert_eq!(after_restore(true, &range), AfterRestore::Repair);
        assert_eq!(after_restore(true, &preedit), AfterRestore::Repair);
    }

    #[test]
    fn only_keys_that_make_text_continue_after_a_repair() {
        assert!(continues_after_repair(&UserAction::Input('a')));
        assert!(continues_after_repair(&UserAction::ImeToggle));
        // 復元直後の Space（Rebased）は修復だけ
        assert!(!continues_after_repair(&UserAction::Convert));
        assert!(!continues_after_repair(&UserAction::Backspace));
        assert!(!continues_after_repair(&UserAction::Cancel));
    }
}
