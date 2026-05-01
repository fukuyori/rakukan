//! ユーザ入力ハンドラ。`on_input` / `on_input_raw` / `on_full_width_space` と
//! `prepare_for_direct_input` ヘルパーを集約。
//!
//! M3 (T1-A) で factory.rs から純粋切り出し。動作変更なし。
//! 関数は別ファイルからも呼ばれるため `pub(super)` を付与している。

use anyhow::Result;
use windows::Win32::UI::TextServices::{ITfCompositionSink, ITfContext};

use crate::diagnostics::{self as diag, DiagEvent};
use crate::engine::state::{
    SessionState, engine_try_get_or_create, session_get, session_is_selecting_fast,
};
use crate::engine::text_util;
use crate::tsf::candidate_window;

use super::{
    commit_text, commit_then_start_composition, end_composition, loading_indicator_symbol,
    update_composition,
};

impl super::TextServiceFactory_Impl {
    pub(super) fn prepare_for_direct_input(&self) -> Result<()> {
        if let Ok(mut sess) = session_get() {
            if sess.is_waiting() {
                let pre = sess.preedit_text().unwrap_or("").to_string();
                sess.set_preedit(pre);
                candidate_window::hide();
            }
        }
        Ok(())
    }

    pub(super) fn on_input(
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
        crate::tsf::live_session::conv_gen_bump();
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
            crate::tsf::live_session::suppress_commit_clear();
            if sess.is_live_conv() {
                let (reading, preview) = sess
                    .live_conv_parts()
                    .map(|(r, p)| (r.to_string(), p.to_string()))
                    .unwrap_or_default();
                candidate_window::hide();
                candidate_window::stop_live_timer();
                crate::tsf::live_session::queue_preview_clear();

                let kind = if c.is_ascii_uppercase() {
                    crate::engine::state::InputCharKind::FullwidthAlpha
                } else {
                    crate::engine::state::InputCharKind::Char
                };
                let (preedit, new_reading, _bg) = engine.input_char(c, kind, None);
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
                let live_ready = crate::engine::state::start_live_bg_if_ready(engine, &new_reading);
                drop(sess);
                drop(guard);
                if live_ready {
                    candidate_window::live_input_notify(&ctx, tid);
                }
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
                // 打鍵時の prefetch はライブプレビュー用なので、読みが十分長い場合だけ
                // 後段で live_conv_beam_size を使って起動する。
                let (preedit, hiragana, _bg) = engine2.input_char(c, kind, None);
                let _ = crate::engine::state::start_live_bg_if_ready(engine2, &hiragana);
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

        // バッチ RPC: push + preedit + hiragana + bg_status を 1 往復で処理する。
        // ライブ変換の bg_start は、3文字以上になった場合だけ後段で起動する。
        //
        // 打鍵時の prefetch はライブプレビュー用なので live_conv_beam_size を使う。
        // ただし 1〜2文字では起動せず、3文字以上になってから開始する。
        // Space 押下時は on_convert 内で bg_reclaim + bg_start(num_candidates) により
        // fresh に変換し直す。
        let kind = if c.is_ascii_uppercase() {
            crate::engine::state::InputCharKind::FullwidthAlpha
        } else {
            crate::engine::state::InputCharKind::Char
        };
        let (preedit, hiragana, bg_status) = engine.input_char(c, kind, None);
        diag::event(DiagEvent::InputChar {
            ch: c,
            preedit_after: preedit.clone(),
        });
        tracing::trace!("Input: hiragana={:?} bg={}", hiragana, bg_status);

        if !hiragana.is_empty() {
            let live_ready = crate::engine::state::start_live_bg_if_ready(engine, &hiragana);
            drop(guard);
            // [Phase0] ライブ変換実験: コンテキストをキャッシュしてタイマーを起動
            if live_ready {
                candidate_window::live_input_notify(&ctx, tid);
            }
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
    pub(super) fn on_input_raw(
        &self,
        c: char,
        ctx: ITfContext,
        tid: u32,
        sink: ITfCompositionSink,
        mut guard: crate::engine::state::EngineGuard,
    ) -> Result<bool> {
        // M1.8 T-MID1: reading 変化経路。on_input と同じく gen を前進させる。
        crate::tsf::live_session::conv_gen_bump();
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
            crate::tsf::live_session::suppress_commit_clear();
            if sess.is_live_conv() {
                let (reading, preview) = sess
                    .live_conv_parts()
                    .map(|(r, p)| (r.to_string(), p.to_string()))
                    .unwrap_or_default();
                candidate_window::hide();
                candidate_window::stop_live_timer();
                crate::tsf::live_session::queue_preview_clear();

                engine.push_raw(c);
                let new_reading = engine.hiragana_text().to_string();
                let suffix = new_reading
                    .strip_prefix(&reading)
                    .unwrap_or(new_reading.as_str())
                    .to_string();
                let display = format!("{preview}{suffix}");
                sess.set_live_conv(new_reading.clone(), display.clone());
                let live_ready = crate::engine::state::start_live_bg_if_ready(engine, &new_reading);
                drop(sess);
                drop(guard);
                if live_ready {
                    candidate_window::live_input_notify(&ctx, tid);
                }
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
                // ライブプレビュー用の prefetch は、3文字以上になった場合だけ開始する。
                // Space 押下時は別途 bg_reclaim + bg_start(num_candidates) で fresh に変換する。
                let reading = engine2.hiragana_text();
                let _ = crate::engine::state::start_live_bg_if_ready(engine2, &reading);
                drop(guard2);
                commit_then_start_composition(ctx, tid, sink, full_text, preedit)?;
                return Ok(true);
            }
        }
        engine.push_raw(c);
        let preedit = engine.preedit_display();
        // ライブプレビュー用の prefetch は、3文字以上になった場合だけ開始する。
        // Space 押下時は on_convert 内で bg_reclaim + bg_start(num_candidates) により
        // fresh に変換し直すため、ここの prefetch 結果は Space には流用されない。
        let reading = engine.hiragana_text();
        let live_ready = crate::engine::state::start_live_bg_if_ready(engine, &reading);
        if live_ready {
            candidate_window::live_input_notify(&ctx, tid);
        }
        drop(guard);
        update_composition(ctx, tid, sink, preedit)?;
        Ok(true)
    }

    pub(super) fn on_full_width_space(
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

}
