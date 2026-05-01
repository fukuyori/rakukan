//! 変換系ハンドラ。`on_convert` / `on_commit_raw` / `on_backspace` / `on_cancel` を集約。
//!
//! M3 (T1-A) で factory.rs から純粋切り出し。動作変更なし。

use anyhow::Result;
use std::time::Instant;
use windows::Win32::UI::TextServices::{ITfCompositionSink, ITfContext};

use crate::diagnostics::{self as diag, DiagEvent};
use crate::engine::state::{
    SessionState, caret_rect_get, composition_clone, engine_try_get_or_create, session_get,
};
use crate::tsf::candidate_window;

use super::{
    commit_text, commit_then_start_composition, end_composition, engine_convert_sync_multi,
    update_caret_rect, update_composition, update_composition_candidate_parts,
};

#[inline]
fn convert_mark(stage: &'static str, start: Instant, last: &mut Instant) {
    let now = Instant::now();
    let step_us = now.duration_since(*last).as_micros();
    let total_us = now.duration_since(start).as_micros();
    *last = now;
    tracing::info!("convert_timing stage={stage} step_us={step_us} total_us={total_us}");
}

impl super::TextServiceFactory_Impl {
    pub(super) fn on_convert(
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
                // Phase 1B キューをクリア
                crate::tsf::live_session::queue_preview_clear();
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
        let convert_start = Instant::now();
        let mut convert_last = convert_start;
        let llm_limit = crate::engine::state::get_num_candidates();
        const DICT_LIMIT: usize = 40;
        let _ = crate::engine::state::poll_dict_ready_cached(engine);
        let _ = crate::engine::state::poll_model_ready_cached(engine);
        convert_mark("ready_poll", convert_start, &mut convert_last);
        // Done 状態の converter を先に回収する。
        // bg_take_candidates がキー不一致で None を返した場合、converter は Done に残ったまま
        // engine.kanji=None になる。is_kanji_ready() チェックより前に reclaim しないと
        // bg_start が永遠にスキップされ Waiting から抜け出せなくなる。
        engine.bg_reclaim();
        convert_mark("bg_reclaim", convert_start, &mut convert_last);
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
            convert_mark("bg_start", convert_start, &mut convert_last);
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
            convert_mark("waiting_show", convert_start, &mut convert_last);
            // LLM完了を待機（短時間のみ）。超えたら WM_TIMER に任せて抜ける。
            let completed = engine.bg_wait_ms(LLM_WAIT_INLINE_MS);
            convert_mark("bg_wait_inline", convert_start, &mut convert_last);
            tracing::debug!("on_convert[new]: bg_wait({LLM_WAIT_INLINE_MS}ms) completed={completed}");
            if !completed {
                tracing::info!(
                    "convert_timing result=timer_fallback total_us={}",
                    convert_start.elapsed().as_micros()
                );
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
            convert_mark("waiting_show_prev_bg", convert_start, &mut convert_last);
            tracing::debug!(
                "on_convert[new]: kanji_ready=false bg=running → wait for prev bg to finish"
            );
            let completed = engine.bg_wait_ms(LLM_WAIT_INLINE_MS);
            convert_mark("prev_bg_wait_inline", convert_start, &mut convert_last);
            tracing::debug!("on_convert[new]: prev bg wait completed={completed}");
            if !completed {
                // 前の bg が inline 時間で終わらない → WM_TIMER に任せる
                tracing::info!(
                    "convert_timing result=prev_bg_timer_fallback total_us={}",
                    convert_start.elapsed().as_micros()
                );
                drop(guard);
                candidate_window::start_waiting_timer();
                return Ok(true);
            }
            // 前の bg が完了したら converter を回収して新しいキーで再起動
            engine.bg_reclaim();
            convert_mark("prev_bg_reclaim", convert_start, &mut convert_last);
            let kanji_ready2 = engine.is_kanji_ready();
            tracing::debug!("on_convert[new]: after reclaim kanji_ready={kanji_ready2}");
            if kanji_ready2 {
                engine.bg_start(llm_limit);
                convert_mark("new_bg_start_after_prev", convert_start, &mut convert_last);
                let completed2 = engine.bg_wait_ms(LLM_WAIT_INLINE_MS);
                convert_mark("new_bg_wait_inline", convert_start, &mut convert_last);
                tracing::debug!("on_convert[new]: new bg wait completed={completed2}");
                if !completed2 {
                    tracing::info!(
                        "convert_timing result=new_bg_timer_fallback total_us={}",
                        convert_start.elapsed().as_micros()
                    );
                    drop(guard);
                    candidate_window::start_waiting_timer();
                    return Ok(true);
                }
                // kanji_ready を更新して後続の候補取得処理へ続行
            } else {
                // モデル自体が未ロード → タイマーに任せる
                tracing::info!(
                    "convert_timing result=model_not_ready_timer_fallback total_us={}",
                    convert_start.elapsed().as_micros()
                );
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
        convert_mark("bg_take_candidates", convert_start, &mut convert_last);
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
            convert_mark("retry_bg_reclaim", convert_start, &mut convert_last);
            if engine.is_kanji_ready() {
                engine.bg_start(llm_limit);
                convert_mark("retry_bg_start", convert_start, &mut convert_last);
                let completed3 = engine.bg_wait_ms(LLM_WAIT_INLINE_MS);
                convert_mark("retry_bg_wait_inline", convert_start, &mut convert_last);
                tracing::debug!("Convert: retry bg_wait completed={completed3}");
                if !completed3 {
                    tracing::info!(
                        "convert_timing result=retry_timer_fallback total_us={}",
                        convert_start.elapsed().as_micros()
                    );
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
                convert_mark("merge_candidates", convert_start, &mut convert_last);
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
                    convert_mark("sync_multi_fallback", convert_start, &mut convert_last);
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
        convert_mark("session_ready", convert_start, &mut convert_last);
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
        convert_mark("candidate_window_show", convert_start, &mut convert_last);
        tracing::debug!(
            "on_convert[new]: update_composition first={:?} comp_exists={}",
            first,
            composition_clone().map(|g| g.is_some()).unwrap_or(false)
        );
        update_composition(ctx, tid, sink, first)?;
        convert_mark("update_composition", convert_start, &mut convert_last);
        tracing::info!(
            "convert_timing result=shown candidates={} llm_pending={} total_us={}",
            candidates.len(),
            llm_pending,
            convert_start.elapsed().as_micros()
        );
        Ok(true)
    }

    pub(super) fn on_commit_raw(
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
                let _ = crate::engine::state::start_live_bg_if_ready(engine, &unselected);
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
                    let _ =
                        crate::engine::state::start_live_bg_if_ready(engine, &remainder_reading);
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
        if crate::tsf::live_session::suppress_commit_take() {
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

    pub(super) fn on_backspace(
        &self,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        // M1.8 T-MID1: reading が短くなるので gen を前進させる。
        crate::tsf::live_session::conv_gen_bump();
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
                crate::tsf::live_session::queue_preview_clear();
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

    pub(super) fn on_cancel(
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
                crate::tsf::live_session::queue_preview_clear();
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


}
