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
                // stale 判定
                let current_gen = conv_gen_snapshot();
                let current_nonce = session_nonce_snapshot();
                let current_reading = engine.hiragana_text().to_string();
                let stale_gen = entry.gen_when_requested != current_gen;
                let stale_reading = entry.reading != current_reading;
                let stale_nonce = entry.session_nonce_at_request != current_nonce;
                if stale_gen || stale_reading || stale_nonce {
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
                        let reading = engine.hiragana_text().to_string();
                        let preedit_full = engine.preedit_display();
                        let pending = text_util::suffix_after_prefix_or_empty(
                            &preedit_full,
                            &reading,
                            "phase1b pending",
                        )
                        .to_string();
                        if !reading.is_empty() {
                            // 尻切れ防壁（M1.5 T-BUG2）: Phase1B 経路でも同じく
                            // preview の長さが reading に対して極端に短ければ破棄
                            let preview = candidate_window::sanity_check_preview(
                                &reading, preview, "Phase1B",
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
