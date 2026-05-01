//! 編集操作系ハンドラ。F6-F10 のかな・英数変換、CycleKana、候補ナビゲーション、
//! IME トグル、モード切替、文節操作、句読点入力を集約。
//!
//! M3 (T1-A) で factory.rs から純粋切り出し。動作変更なし。

use anyhow::Result;
use windows::Win32::UI::TextServices::{ITfCompositionSink, ITfContext};

use crate::diagnostics::{self as diag, DiagEvent};
use crate::engine::state::{SessionState, caret_rect_get, engine_try_get_or_create, session_get};
use crate::engine::text_util;
use crate::tsf::candidate_window;
use crate::tsf::language_bar;

use super::{
    CandidateDir, commit_text, commit_then_start_composition, end_composition,
    engine_convert_sync_multi, update_caret_rect, update_composition,
    update_composition_candidate_parts,
};

fn is_numeric_digit(c: char) -> bool {
    c.is_ascii_digit() || ('０'..='９').contains(&c)
}

fn numeric_separator_after_digit(reading: &str, c: char) -> Option<char> {
    if !reading.chars().last().is_some_and(is_numeric_digit) {
        return None;
    }
    match c {
        '、' | ',' => Some(','),
        '。' | '.' => Some('.'),
        _ => None,
    }
}

impl super::TextServiceFactory_Impl {
    pub(super) fn on_kana_convert(
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
            if hira.is_empty() { p.clone() } else { hira }
        } else {
            p.clone()
        };
        let t = convert_fn(&source);
        engine.force_preedit(t.clone());
        crate::tsf::live_session::suppress_commit_arm();
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
    pub(super) fn on_latin_convert(
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
        crate::tsf::live_session::suppress_commit_arm();
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

    pub(super) fn on_cycle_kana(
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

    pub(super) fn on_candidate_move(
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

    pub(super) fn on_candidate_page(
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

    pub(super) fn on_candidate_select(
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
            let _ = crate::engine::state::start_live_bg_if_ready(engine, &remainder_reading);
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

    pub(super) fn on_ime_toggle(&self, ctx: ITfContext, tid: u32) -> Result<bool> {
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

    pub(super) fn on_ime_off(&self, ctx: ITfContext, tid: u32) -> Result<bool> {
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

    pub(super) fn on_ime_on(&self, ctx: ITfContext, tid: u32) -> Result<bool> {
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

    pub(super) fn on_mode_hiragana(
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

    pub(super) fn on_mode_katakana(
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
    pub(super) fn on_punctuate(
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

        let reading = engine.hiragana_text();
        if let Some(separator) = numeric_separator_after_digit(&reading, c) {
            if crate::engine::state::is_digit_separator_auto_enabled() {
                crate::tsf::live_session::conv_gen_bump();
                engine.push_raw(separator);
                let reading = engine.hiragana_text();
                let preedit = engine.preedit_display();
                let live_ready = crate::engine::state::start_live_bg_if_ready(engine, &reading);
                drop(guard);
                if live_ready {
                    candidate_window::live_input_notify(&ctx, tid);
                }
                update_composition(ctx, tid, sink, preedit)?;
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
    pub(super) fn on_segment_move_left(
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
    pub(super) fn on_segment_shrink(
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
    pub(super) fn on_segment_move_right(
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
    pub(super) fn on_segment_extend(
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
