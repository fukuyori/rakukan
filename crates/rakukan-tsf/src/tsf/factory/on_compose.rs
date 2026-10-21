//! composition 操作の EditSession ヘルパー集約。
//!
//! 旧 factory.rs から M3 (T1-A) で純粋切り出し。動作変更なし、関数本体は完全に
//! 同一。可視性は `pub(super)` に揃え、factory.rs から `use on_compose::*;` で
//! 引き込む。
//!
//! 含まれる関数:
//! - `update_composition` / `update_composition_candidate_parts` / `update_caret_rect`
//! - `commit_then_start_composition` / `end_composition` / `commit_text`
//! - キャレット / range 取得ヘルパー (`get_caret_pos_from_context` / `get_cursor_range` /
//!   `get_insert_range_or_end` / `get_document_end_range`)
//! - 表示属性ヘルパー (`set_display_attr_prop`)

use anyhow::Result;
use windows::Win32::UI::TextServices::{
    GUID_PROP_ATTRIBUTE, ITfCompositionSink, ITfContext, TF_ES_READWRITE,
};
use windows::core::Interface;

use crate::engine::state::{
    caret_rect_set, composition_clone, composition_set_with_dm, composition_take,
};
use crate::tsf::display_attr;
use crate::tsf::edit_session::EditSession;

/// TSF コンテキストからキャレットのスクリーン座標 (x, y_bottom) を取得する。
/// mozc の FillCharPosition と同じアプローチ: GetSelection → GetTextExt。
/// 取得できない場合は None を返す（インジケーターは表示しない）。
pub(super) unsafe fn get_caret_pos_from_context(
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    ec: u32,
) -> Option<(i32, i32)> {
    let range = unsafe { get_cursor_range(ctx, ec) }?;
    let view = unsafe { ctx.GetActiveView() }.ok()?;
    let mut rect = windows::Win32::Foundation::RECT::default();
    let mut clipped = windows::Win32::Foundation::BOOL(0);
    unsafe {
        view.GetTextExt(ec, &range, &mut rect, &mut clipped).ok()?;
    }
    // rect はスクリーン座標。left = x, bottom = キャレット下端。
    Some((rect.left, rect.bottom))
}

/// 現在のキャレット位置を表す長さ0の ITfRange を返す。
/// GetSelection で現在選択範囲を取得し、終端アンカーに collapse する。
/// 失敗時は None（呼び元が GetEnd にフォールバックする）。
pub(super) unsafe fn get_cursor_range(
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    ec: u32,
) -> Option<windows::Win32::UI::TextServices::ITfRange> {
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::UI::TextServices::{
        TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
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
    unsafe {
        ctx.GetSelection(ec, 0xFFFF_FFFF_u32, &mut sel_buf, &mut fetched as *mut u32)
            .ok()?;
    }
    if fetched == 0 {
        return None;
    }
    let range_ref = (*sel_buf[0].range).as_ref()?;
    let cloned = unsafe { range_ref.Clone() }.ok()?;
    if let Err(e) = unsafe { cloned.Collapse(ec, TF_ANCHOR_END) } {
        tracing::warn!("get_cursor_range: Collapse failed: {e}, range may not be zero-length");
    }
    Some(cloned)
}

/// 選択位置を長さ 0 に縮めた range を返す。縮められなければ `None`（選択中の文字を
/// 置き換えないよう、挿入に使わない）。
unsafe fn get_insertion_point(
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    ec: u32,
) -> Option<windows::Win32::UI::TextServices::ITfRange> {
    use windows::Win32::Foundation::BOOL;
    use windows::Win32::UI::TextServices::{
        TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
    };
    let mut sel_buf = [TF_SELECTION {
        range: std::mem::ManuallyDrop::new(None),
        style: TF_SELECTIONSTYLE {
            ase: TfActiveSelEnd(0),
            fInterimChar: BOOL(0),
        },
    }];
    let mut fetched: u32 = 0;
    unsafe {
        ctx.GetSelection(ec, 0xFFFF_FFFF_u32, &mut sel_buf, &mut fetched as *mut u32)
            .ok()?;
    }
    // 受け取った選択範囲の参照は、ここで取り出して所有する（ManuallyDrop のまま残さない）
    let range = std::mem::ManuallyDrop::into_inner(std::mem::replace(
        &mut sel_buf[0].range,
        std::mem::ManuallyDrop::new(None),
    ));
    if fetched == 0 {
        return None;
    }
    let range = range?;
    unsafe { range.Collapse(ec, TF_ANCHOR_END) }.ok()?;
    Some(range)
}

/// 現在カーソル位置の range を優先し、取得できなければ `GetEnd` にフォールバックする。
///
/// TSF/COM が不安定な瞬間でも panic でホストプロセスを巻き込まないよう、
/// 失敗は `E_FAIL` に変換して呼び元へ返す。
pub(super) unsafe fn get_insert_range_or_end(
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    ec: u32,
    op: &str,
) -> windows::core::Result<windows::Win32::UI::TextServices::ITfRange> {
    use windows::Win32::Foundation::E_FAIL;

    if let Some(range) = unsafe { get_cursor_range(ctx, ec) } {
        return Ok(range);
    }

    tracing::debug!("{op}: cursor range unavailable, falling back to GetEnd");
    unsafe { ctx.GetEnd(ec) }
        .map_err(|e| windows::core::Error::new(E_FAIL, format!("{op}: GetEnd: {e}")))
}

/// `GetEnd` を安全に取得する。
///
/// `commit_then_start_composition` のように「現在選択位置を使うと意味が変わる」
/// 経路では、cursor range を見に行かず `GetEnd` を明示的に使う。
pub(super) unsafe fn get_document_end_range(
    ctx: &windows::Win32::UI::TextServices::ITfContext,
    ec: u32,
    op: &str,
) -> windows::core::Result<windows::Win32::UI::TextServices::ITfRange> {
    use windows::Win32::Foundation::E_FAIL;

    unsafe { ctx.GetEnd(ec) }
        .map_err(|e| windows::core::Error::new(E_FAIL, format!("{op}: GetEnd: {e}")))
}

pub(super) fn update_composition(
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    preedit: String,
) -> Result<()> {
    use windows::Win32::Foundation::E_FAIL;

    let existing = composition_clone()?;
    // M1.8 T-MID2: stale check 用に外側 snapshot のポインタを記録。
    // EditSession クロージャは TF_ES_READWRITE で遅延実行されるため、
    // ここで取った composition が DM 破棄や invalidate_composition_for_dm で
    // stale 化したまま SetText しないよう、クロージャ先頭で再検査する。
    let existing_ptr = existing.as_ref().map(|c| c.as_raw() as usize).unwrap_or(0);
    // 表示する編集状態を、要求の時点で控える（Issue #71）
    let bind = crate::engine::rpc_composition::bind_target();
    let ctx_req = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            ITfContextComposition, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
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
            crate::engine::rpc_composition::bind_composition(
                bind,
                crate::engine::state::composition_identity(comp),
                crate::engine::state::composition_dm(),
            );
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
            crate::engine::rpc_composition::bind_composition(
                bind,
                crate::engine::state::composition_identity(&new_comp),
                dm_ptr,
            );
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

        // 打鍵のたびにキャレット矩形を更新する。
        //
        // 候補ウィンドウの表示位置は `caret_rect_get()` を読むが、CARET_RECT を
        // 更新するのは Space 押下時の `update_caret_rect()` と
        // `commit_then_start_composition()` だけだった。`update_caret_rect()` は
        // 非同期の EditSession を投げるだけなので、同じ Space の処理内で
        // `caret_rect_get()` を読む側はまだ更新前の値を見る。プロセス内で最初の
        // 変換ではそれが初期値 (0,0,0,0) にあたり、候補ウィンドウがプライマリ
        // モニタの左上に出てしまう。
        //
        // ここで更新しておけば、Space が来る前に必ず正しい矩形が入っている。
        if let Ok(view) = ctx.GetActiveView() {
            use windows::Win32::Foundation::RECT;
            let mut rect = RECT::default();
            let mut clipped = windows::Win32::Foundation::BOOL(0);
            match view.GetTextExt(ec, &range, &mut rect, &mut clipped) {
                Ok(()) => {
                    // 全ゼロは「取れなかった」と同義（成功を返しつつ空矩形を
                    // 返すアプリがある）。古い正しい値を壊さないよう捨てる。
                    if rect.bottom != 0 || rect.left != 0 {
                        caret_rect_set(rect);
                        crate::tsf::candidate_window::reposition(rect.left, rect.bottom);
                    } else {
                        tracing::debug!("update_composition: GetTextExt returned empty rect");
                    }
                }
                Err(e) => {
                    tracing::debug!("update_composition: GetTextExt failed: {e}");
                }
            }
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
///
/// 学習（`learn`）だけは、文書の結果を見てから送る（入力処理の見直しの段 5 の 5b）。
/// 残りの読みの BG 変換の開始（`bg_start`）は、その後に送る（BG の要求が結果不明で台帳に
/// 残ると、後の学習の要求が断られるため）。BG 変換を始めたかを返す。
/// 確定部分に縮めた後の範囲を読み直し、それが `commit_text` と同じときだけ学習する。
/// 文脈・編集状態を閉じる処理・残りの読みの引き継ぎは、今までどおり呼び出し側が
/// 先に送る（新しい composition にホストの表示を書くため）。EditSession が後で実行
/// される場合は、結果が出てから学習を送る（その間のキーは key_queue が保留する）。
pub(super) fn commit_then_start_composition(
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    commit_text: String,
    next_preedit: String,
    learn: Option<crate::tsf::commit_txn::LearnPlan>,
    bg_start: Option<String>,
) -> Result<bool> {
    use crate::tsf::commit_txn::{CommitEffects, DocResult, ReqState};
    use windows::Win32::Foundation::E_FAIL;

    // M1.8 T-MID1 拡張: 確定はライブ変換の世代を進める。これをしないと、
    // 確定前に積まれた Phase1A/1B の遅延 SetText が gen 一致のまま
    // 確定後の新 composition に古い preview を書き込み、テキストが
    // 二重に見える競合が起こりうる。
    crate::tsf::live_session::conv_gen_bump();

    let cell = crate::tsf::commit_txn::begin(
        commit_text.clone(),
        CommitEffects {
            learn,
            bg_start,
            ..Default::default()
        },
    );
    let session_cell = cell.clone();
    let _ = crate::tsf::commit_txn::take_bg_started();

    // composition_take() をセッション内に移動する（end_composition と同じ理由）。
    // セッション外で take すると COMPOSITION=None になった瞬間に update_composition が
    // 誤ったカーソル位置から新 composition を開始するリスクがある。
    // 新しい composition に表示する編集状態（残りの読み）を、要求の時点で控える（Issue #71）
    let bind = crate::engine::rpc_composition::bind_target();
    // 今の composition を置き換える EditSession を要求済みであることを示す（Issue #71）。
    // 実行より先に今の composition の終了通知が届いても、新しい composition のための
    // 状態・セッションを片づけない。この要求の EditSession の冒頭か、EditSession が
    // 後から実行されない結果（同期で終わった・拒否された・要求の失敗）で消す。
    let ctx_dm = unsafe { ctx.GetDocumentMgr() }
        .ok()
        .map(|dm| dm.as_raw() as usize)
        .unwrap_or(0);
    let replacement = crate::engine::state::set_pending_replacement(ctx_dm);
    crate::tsf::commit_txn::mark_partial(&cell, replacement);
    let ctx_req = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            ITfContextComposition, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
        };
        crate::engine::state::clear_pending_replacement(replacement);
        {
            let mut state = session_cell.borrow_mut();
            if *state != ReqState::Pending {
                return Ok(());
            }
            *state = ReqState::Running;
        }
        let finish = |result: DocResult| {
            *session_cell.borrow_mut() = ReqState::Done(result);
            crate::tsf::candidate_window::post_commit_done();
        };

        // SetText 排他化（M1.8 T-MID3 の commit 版）: Phase1A / update_composition
        // の SetText と直列化する。他サイトは try_lock + skip だが、確定はスキップ
        // するとユーザーテキストが失われるためブロッキングで取得する。
        // 遅延実行された Phase1A がこのセッションの後に走っても、composition_take
        // で旧 composition が外れているため古い preview の書き込みは失敗する。
        let _apply_guard = crate::engine::state::COMPOSITION_APPLY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let comp = composition_take().unwrap_or(None);
        tracing::debug!(
            "commit_then_start[session]: commit={:?} next={:?} has_comp={}",
            commit_text,
            next_preedit,
            comp.is_some()
        );
        // 途中で `?` により抜けた手順を、内容付きでログに残す（Issue #69）。
        // セッションの結果（phrSession）だけでは、どの手順で失敗したか分からない。
        let warn_step = |e: &windows::core::Error| {
            tracing::warn!(
                "commit_then_start: {e} commit={:?} next={:?}",
                commit_text,
                next_preedit
            );
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
            // 確定した文字列（縮めた後に読み直した値。読めなければ分からない）
            let mut committed: Option<String> = None;
            // composition テキストを commit_text だけに縮める
            if let Ok(range) = comp.GetRange() {
                if let Err(e) = range.SetText(ec, 0, &commit_w) {
                    // 縮められないまま EndComposition へ進むと、確定部分に残りまで
                    // 含まれ、新しい composition の残りと二重に見えるおそれがある（Issue #69）。
                    tracing::warn!(
                        "commit_then_start: SetText to the committed part failed commit={:?} next={:?}: {e}",
                        commit_text,
                        next_preedit
                    );
                }
                committed = range_text(ec, &range);
                // 確定テキストの末尾位置を保存
                if let Ok(end_range) = range.Clone() {
                    let _ = end_range.Collapse(ec, TF_ANCHOR_END);
                    insert_after_commit = Some(end_range);
                }
            } else {
                tracing::warn!("commit_then_start: comp.GetRange() failed");
            }
            comp.EndComposition(ec)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("EndComposition: {e}")))
                .inspect_err(warn_step)
                .inspect_err(|_| finish(DocResult::Failed("EndComposition")))?;
            finish(match committed {
                Some(text) => DocResult::Committed { text },
                None => DocResult::CommittedUnknown,
            });
        } else if !commit_text.is_empty() {
            let insert_point = get_insert_range_or_end(&ctx, ec, "commit_then_start direct commit")
                .inspect_err(warn_step)
                .inspect_err(|_| finish(DocResult::Failed("direct commit")))?;
            insert_point
                .SetText(ec, 0, &commit_w)
                .map_err(|e| {
                    windows::core::Error::new(E_FAIL, format!("SetText direct commit: {e}"))
                })
                .inspect_err(warn_step)
                .inspect_err(|_| finish(DocResult::Failed("direct commit")))?;
            finish(DocResult::Committed {
                text: commit_text.clone(),
            });
            if let Ok(end_range) = insert_point.Clone() {
                let _ = end_range.Collapse(ec, TF_ANCHOR_END);
                insert_after_commit = Some(end_range);
            }
        } else {
            finish(DocResult::Committed {
                text: String::new(),
            });
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
            get_document_end_range(&ctx, ec, "commit_then_start new composition")
                .inspect_err(warn_step)?
        };
        let cc: ITfContextComposition = ctx
            .cast()
            .map_err(|e| {
                windows::core::Error::new(E_FAIL, format!("cast ITfContextComposition: {e}"))
            })
            .inspect_err(warn_step)?;
        let new_comp = cc
            .StartComposition(ec, &insert_point, &sink)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("StartComposition: {e}")))
            .inspect_err(warn_step)?;
        let new_range = new_comp
            .GetRange()
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange new: {e}")))
            .inspect_err(warn_step)?;
        let dm_ptr = ctx
            .GetDocumentMgr()
            .ok()
            .map(|dm| dm.as_raw() as usize)
            .unwrap_or(0);
        crate::engine::rpc_composition::bind_composition(
            bind,
            crate::engine::state::composition_identity(&new_comp),
            dm_ptr,
        );
        let _ = composition_set_with_dm(Some(new_comp), dm_ptr);

        let preedit_w: Vec<u16> = next_preedit.encode_utf16().collect();
        new_range
            .SetText(ec, 0, &preedit_w)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText new: {e}")))
            .inspect_err(warn_step)?;

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

        // BlockSelecting 位置追従:
        // 新 composition の先頭位置を CARET_RECT に記録し、候補ウィンドウを即時移動する。
        // RequestEditSession は非同期のため、次のキー入力より先にここで更新しないと
        // Space キーハンドラが caret_rect_get() を読む時点でまだ旧値になってしまう。
        if let Ok(view) = ctx.GetActiveView() {
            use windows::Win32::Foundation::RECT;
            let mut rect = RECT::default();
            let mut clipped = windows::Win32::Foundation::BOOL(0);
            if view
                .GetTextExt(ec, &new_range, &mut rect, &mut clipped)
                .is_ok()
            {
                caret_rect_set(rect);
                // 候補ウィンドウが表示中であれば位置を更新（非表示なら何もしない）
                crate::tsf::candidate_window::reposition(rect.left, rect.bottom);
            }
        }

        Ok(())
    });
    // end_composition と同じく、edit session の結果（phrSession）まで確認して
    // 失敗をログに残す（Issue #69）。戻り値と後続の処理は変えない。
    unsafe {
        match ctx_req.RequestEditSession(tid, &session, TF_ES_READWRITE) {
            // 非同期で受け付けた: EditSession の冒頭で印を消す
            Ok(hr) if hr == windows::Win32::UI::TextServices::TF_S_ASYNC => {}
            // 実行されずに拒否された場合もある（読み取り専用など）。後から実行されないので消す
            Ok(hr) if hr.is_err() => {
                crate::engine::state::clear_pending_replacement(replacement);
                tracing::warn!("commit_then_start: edit session failed hr={hr:?}");
                crate::tsf::commit_txn::mark_not_run(&cell, "edit session refused");
            }
            Ok(_) => crate::engine::state::clear_pending_replacement(replacement),
            Err(e) => {
                crate::engine::state::clear_pending_replacement(replacement);
                tracing::warn!("commit_then_start: RequestEditSession failed: {e}");
                crate::tsf::commit_txn::mark_not_run(&cell, "request failed");
            }
        }
    }
    // 同期で実行されていれば、結果を見て学習を送る。後で実行される場合は、結果が出て
    // から送る（WM_APP_COMMIT_DONE。その間のキーは key_queue が保留する）
    crate::tsf::commit_txn::finish_completed();
    if crate::tsf::commit_txn::commit_pending() {
        crate::tsf::candidate_window::start_key_queue_timer();
    }
    Ok(crate::tsf::commit_txn::take_bg_started())
}

/// 直接の挿入の要求の状態（[`insert_text_direct`]）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InsertState {
    Pending,
    Running,
    Inserted,
    /// 入れなかった。`retryable` なら文書は変わっていない（要求し直してよい）。
    NotInserted {
        reason: &'static str,
        retryable: bool,
    },
}

/// 文字列を今の選択位置にそのまま入れる要求を出す（入力処理の見直しの段 6、設計書 4.2 の
/// `InsertTxn`。回復できないときに、保留した文字キーを打った文字のまま入れる）。
///
/// 位置を確かめられないときは入れない（設計書 4.2 の救済の原則）: composition が残って
/// いる（その中に入ると消えうる）、選択位置が取れない。結果は返した状態の箱に記録する
/// （後で実行される場合は、実行の後に `WM_APP_COMMIT_DONE` を送る）。箱が `Pending` の
/// ままなら、呼び出し側が失効（`NotInserted`）にしてよい。失効した要求は後で実行されても
/// 文書に触れない。
///
/// `still_valid` は EditSession の実行の時点で呼び、偽なら文書に触れずに失効させる（要求の
/// 後に Deactivate した・文書が破棄されたなど）。
pub(super) fn insert_text_direct(
    ctx: &ITfContext,
    tid: u32,
    text: &str,
    still_valid: Box<dyn Fn() -> bool>,
) -> std::rc::Rc<std::cell::Cell<InsertState>> {
    use windows::Win32::UI::TextServices::{
        TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
    };
    let state = std::rc::Rc::new(std::cell::Cell::new(InsertState::Pending));
    let session_state = state.clone();
    let session_ctx = ctx.clone();
    let text_w: Vec<u16> = text.encode_utf16().collect();
    let session = EditSession::new(move |ec| unsafe {
        if session_state.get() != InsertState::Pending {
            return Ok(());
        }
        if !still_valid() {
            session_state.set(InsertState::NotInserted {
                reason: "expired",
                retryable: false,
            });
            crate::tsf::candidate_window::post_commit_done();
            return Ok(());
        }
        session_state.set(InsertState::Running);
        let finish = |result: InsertState| {
            session_state.set(result);
            crate::tsf::candidate_window::post_commit_done();
        };
        match crate::engine::state::composition_held() {
            Some(false) => {}
            Some(true) => {
                finish(InsertState::NotInserted {
                    reason: "composition",
                    retryable: false,
                });
                return Ok(());
            }
            // 確かめられない: 書かない（文書は変えていないので、要求し直してよい）
            None => {
                finish(InsertState::NotInserted {
                    reason: "composition unknown",
                    retryable: true,
                });
                return Ok(());
            }
        }
        let Some(range) = get_insertion_point(&session_ctx, ec) else {
            finish(InsertState::NotInserted {
                reason: "no selection",
                retryable: false,
            });
            return Ok(());
        };
        if let Err(e) = range.SetText(ec, 0, &text_w) {
            tracing::warn!("insert_text_direct: SetText failed: {e}");
            finish(InsertState::NotInserted {
                reason: "SetText",
                retryable: true,
            });
            return Ok(());
        }
        if let Ok(cursor) = range.Clone() {
            let _ = cursor.Collapse(ec, TF_ANCHOR_END);
            let sels = [TF_SELECTION {
                range: std::mem::ManuallyDrop::new(Some(cursor)),
                style: TF_SELECTIONSTYLE {
                    ase: TfActiveSelEnd(0),
                    fInterimChar: windows::Win32::Foundation::BOOL(0),
                },
            }];
            let _ = session_ctx.SetSelection(ec, &sels);
            // 渡した range の参照を放す（ManuallyDrop のまま残さない）
            let [sel] = sels;
            drop(std::mem::ManuallyDrop::into_inner(sel.range));
        }
        finish(InsertState::Inserted);
        Ok(())
    });
    match unsafe { ctx.RequestEditSession(tid, &session, TF_ES_READWRITE) } {
        Ok(hr) if hr.is_ok() => {}
        Ok(hr) => {
            tracing::warn!("insert_text_direct: edit session refused hr={hr:?}");
            if state.get() == InsertState::Pending {
                state.set(InsertState::NotInserted {
                    reason: "refused",
                    retryable: true,
                });
            }
        }
        Err(e) => {
            tracing::warn!("insert_text_direct: request failed: {e}");
            state.set(InsertState::NotInserted {
                reason: "request failed",
                retryable: true,
            });
        }
    }
    state
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
    let Ok(prop) = (unsafe { ctx.GetProperty(&GUID_PROP_ATTRIBUTE) }) else {
        return;
    };
    // 既存の属性を先にクリアして TSF に変更を通知させる
    let _ = unsafe { prop.Clear(ec, range) };
    // windows_core::VARIANT で VT_I4 (atom) を設定
    let var = windows_core::VARIANT::from(atom as i32);
    let _ = unsafe { prop.SetValue(ec, range, &var) };
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
/// キャレットは composition 全体の末尾に置く。
pub(super) fn update_composition_candidate_parts(
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    prefix: String,
    converted: String,
    suffix: String,
) -> Result<()> {
    update_composition_parts_impl(
        ctx,
        tid,
        sink,
        prefix,
        converted,
        suffix,
        CaretPlacement::CompositionEnd,
    )
}

/// 範囲指定変換（RangeSelect）用: 先頭から `selected` までを選択範囲として表示する。
///
/// 表示属性は `update_composition_candidate_parts` と同じ（選択範囲＝実線、残り＝点線）だが、
/// キャレットを選択範囲の末尾に置く。下線を描画しないアプリでも、Shift+Right / Left で
/// キャレットが動くことで範囲の変化が分かるようにするため。
pub(super) fn update_composition_range_select(
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    selected: String,
    unselected: String,
) -> Result<()> {
    update_composition_parts_impl(
        ctx,
        tid,
        sink,
        String::new(),
        selected,
        unselected,
        CaretPlacement::ConvertedEnd,
    )
}

/// composition 内でのキャレットの置き場所。
#[derive(Clone, Copy, PartialEq, Eq)]
enum CaretPlacement {
    /// composition 全体の末尾（通常の候補表示）
    CompositionEnd,
    /// `converted` 部分の末尾（範囲指定変換）
    ConvertedEnd,
}

fn update_composition_parts_impl(
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    prefix: String,
    converted: String,
    suffix: String,
    caret: CaretPlacement,
) -> Result<()> {
    use windows::Win32::Foundation::E_FAIL;

    if prefix.is_empty() && suffix.is_empty() {
        return update_composition(ctx, tid, sink, converted);
    }

    let existing = composition_clone()?;
    // M1.8 T-MID2: update_composition と同じ stale check を入れる
    let existing_ptr = existing.as_ref().map(|c| c.as_raw() as usize).unwrap_or(0);
    // 表示する編集状態を、要求の時点で控える（Issue #71）
    let bind = crate::engine::rpc_composition::bind_target();
    let ctx_req = ctx.clone();
    let full = format!("{prefix}{converted}{suffix}");
    let prefix_utf16: i32 = prefix.encode_utf16().count() as i32;

    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            ITfContextComposition, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
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
            crate::engine::rpc_composition::bind_composition(
                bind,
                crate::engine::state::composition_identity(comp),
                crate::engine::state::composition_dm(),
            );
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
            crate::engine::rpc_composition::bind_composition(
                bind,
                crate::engine::state::composition_identity(&new_comp),
                dm_ptr,
            );
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
        let mut converted_range = None;
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
            converted_range = Some(sel_range);
        }

        // ── Step3: キャレットを置く ──
        // CompositionEnd: composition 全体の末尾（従来どおり）
        // ConvertedEnd  : converted 部分の末尾（範囲指定変換。converted 範囲が取れなければ末尾）
        let cursor_base = match (caret, converted_range) {
            (CaretPlacement::ConvertedEnd, Some(r)) => Ok(r),
            _ => range.Clone(),
        };
        if let Ok(cursor) = cursor_base {
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
pub(super) fn update_caret_rect(ctx: ITfContext, tid: u32) {
    let comp = match composition_clone() {
        Ok(Some(c)) => c,
        _ => return,
    };
    let ctx_req = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        if let Ok(range) = comp.GetRange()
            && let Ok(view) = ctx.GetActiveView()
        {
            use windows::Win32::Foundation::RECT;
            let mut rect = RECT::default();
            let mut clipped = windows::Win32::Foundation::BOOL(0);
            if view.GetTextExt(ec, &range, &mut rect, &mut clipped).is_ok() {
                caret_rect_set(rect);
            }
        }
        Ok(())
    });
    unsafe {
        let _ = ctx_req.RequestEditSession(tid, &session, TF_ES_READWRITE);
    }
}

/// range の文字列を読む（入力処理の見直しの段 5）。読めなければ `None`。
///
/// 確定に失敗したときに元の文字列へ戻すため、書き換える前に読んでおく。
unsafe fn range_text(
    ec: u32,
    range: &windows::Win32::UI::TextServices::ITfRange,
) -> Option<String> {
    let range = unsafe { range.Clone() }.ok()?;
    let mut text: Vec<u16> = Vec::new();
    let mut buf = [0u16; 256];
    loop {
        let mut got: u32 = 0;
        // TF_TF_MOVESTART: 読んだ分だけ（複製した）range の先頭を進める
        unsafe {
            range
                .GetText(
                    ec,
                    windows::Win32::UI::TextServices::TF_TF_MOVESTART,
                    &mut buf,
                    &mut got,
                )
                .ok()?
        };
        if got == 0 {
            break;
        }
        text.extend_from_slice(&buf[..got as usize]);
        if (got as usize) < buf.len() {
            break;
        }
    }
    String::from_utf16(&text).ok()
}

/// composition 全体を `text` で確定し、文書の結果を見てからホストへの後処理を送る
/// （入力処理の見直しの段 5、設計書 4.6）。
///
/// 文書の操作は `end_composition` と同じ（`text` に SetText してから EndComposition）。
/// 違いは次の 2 点:
/// - ホストへの後処理（`effects`: BG の回収・学習・文脈・編集状態を閉じる）は、
///   EditSession の後で送る。呼び出し側は、これらを先に送らない。
/// - 確定した文字列を記録し、文脈にはそれを使う。学習は、それが `text` と同じときだけ。
///   確定に失敗したとき・確定した文字列が分からないときは、学習と文脈を送らない
///   （composition は今までどおり手放し、BG の回収と編集状態を閉じる処理は送る）。
///
/// EditSession が後で実行される場合は、結果が出てから後処理を送る（`WM_APP_COMMIT_DONE`。
/// その間に届いたキーは key_queue が保留する）。
pub(super) fn commit_full(
    ctx: ITfContext,
    tid: u32,
    text: String,
    effects: crate::tsf::commit_txn::CommitEffects,
) -> Result<()> {
    use crate::tsf::commit_txn::{DocResult, ReqState};
    use windows::Win32::Foundation::E_FAIL;
    use windows::Win32::UI::TextServices::{
        TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
    };
    crate::tsf::live_session::conv_gen_bump();
    let cell = crate::tsf::commit_txn::begin(text.clone(), effects);
    let session_cell = cell.clone();
    let ctx2 = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        {
            let mut state = session_cell.borrow_mut();
            if *state != ReqState::Pending {
                return Ok(());
            }
            *state = ReqState::Running;
        }
        let finish = |result: DocResult| {
            *session_cell.borrow_mut() = ReqState::Done(result);
            crate::tsf::candidate_window::post_commit_done();
        };
        let _apply_guard = crate::engine::state::COMPOSITION_APPLY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let comp = composition_take().unwrap_or(None);
        let text_w: Vec<u16> = text.encode_utf16().collect();
        let Some(comp) = comp else {
            tracing::debug!("commit_full: no composition, inserting text directly");
            if text.is_empty() {
                finish(DocResult::Committed {
                    text: String::new(),
                });
                return Ok(());
            }
            let inserted = get_insert_range_or_end(&ctx2, ec, "commit_full direct insert")
                .and_then(|insert| insert.SetText(ec, 0, &text_w));
            match inserted {
                Ok(()) => finish(DocResult::Committed { text: text.clone() }),
                Err(e) => {
                    tracing::warn!("commit_full: direct insert failed text={text:?}: {e}");
                    finish(DocResult::Failed("direct insert"));
                }
            }
            return Ok(());
        };
        tracing::debug!("commit_full[session]: text={:?}", text);
        let range = match comp.GetRange() {
            Ok(range) => range,
            Err(e) => {
                tracing::warn!("commit_full: GetRange failed text={text:?}: {e}");
                finish(DocResult::Failed("GetRange"));
                return Err(windows::core::Error::new(E_FAIL, format!("GetRange: {e}")));
            }
        };
        // 置き換えに失敗したときに確定する文字列（表示中の文字列）を知るため、先に読む
        let original = range_text(ec, &range);
        let mut committed = Some(text.clone());
        if let Err(e) = range.SetText(ec, 0, &text_w) {
            // 0x80040209 は TS_E_READONLY（end_composition と同じ。1 回だけ再試行）
            if let Err(e2) = range.SetText(ec, 0, &text_w) {
                let app = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_default();
                tracing::warn!(
                    "commit_full: SetText failed text={text:?} app={app:?}: {e} / retry: {e2}"
                );
                // 置き換えられなくても EndComposition まで進め、表示中の文字列を確定する
                // （end_composition と同じ）。確定した文字列は読めた元の文字列（読めて
                // いなければ分からない）
                committed = original.clone();
            }
        }
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
        if let Err(e) = comp.EndComposition(ec) {
            tracing::warn!("commit_full: EndComposition failed text={text:?}: {e}");
            // composition は今までどおり手放す
            finish(DocResult::Failed("EndComposition"));
            return Err(windows::core::Error::new(
                E_FAIL,
                format!("EndComposition: {e}"),
            ));
        }
        finish(match committed {
            Some(text) => DocResult::Committed { text },
            None => DocResult::CommittedUnknown,
        });
        Ok(())
    });
    unsafe {
        match ctx.RequestEditSession(tid, &session, TF_ES_READWRITE) {
            Ok(hr) if hr == windows::Win32::UI::TextServices::TF_S_ASYNC => {}
            Ok(hr) if hr.is_err() => {
                tracing::warn!("commit_full: edit session failed hr={hr:?}");
                crate::tsf::commit_txn::mark_not_run(&cell, "edit session refused");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("commit_full: RequestEditSession failed: {e}");
                crate::tsf::commit_txn::mark_not_run(&cell, "request failed");
            }
        }
    }
    // 同期で実行されていれば、ここで後処理する（後で実行される場合は WM_APP_COMMIT_DONE と
    // 再生のタイマー。その間のキーは key_queue が保留する）
    crate::tsf::commit_txn::finish_completed();
    if crate::tsf::commit_txn::commit_pending() {
        crate::tsf::candidate_window::start_key_queue_timer();
    }
    Ok(())
}

pub(super) fn end_composition(ctx: ITfContext, tid: u32, text: String) -> Result<()> {
    use windows::Win32::Foundation::E_FAIL;
    use windows::Win32::UI::TextServices::{
        TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
    };
    // 確定はライブ変換の世代を進める（commit_then_start_composition と同じ理由）。
    // 確定前に積まれた Phase1A/1B の遅延 SetText を gen 不一致で棄却させる。
    crate::tsf::live_session::conv_gen_bump();
    // composition_take() をセッション内に移動する。
    // セッション外で take すると COMPOSITION=None になった直後に次のキー入力が来たとき、
    // update_composition が existing=None を見て誤った位置から新 composition を開始してしまう。
    let ctx2 = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        // SetText 排他化（commit_then_start_composition と同様、確定なので
        // try_lock + skip ではなくブロッキングで取得する）
        let _apply_guard = crate::engine::state::COMPOSITION_APPLY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let comp = match composition_take().unwrap_or(None) {
            Some(c) => c,
            None => {
                tracing::debug!("end_composition: no composition, inserting text directly");
                // composition がない場合はカーソル位置に直接挿入
                if !text.is_empty() {
                    let text_w: Vec<u16> = text.encode_utf16().collect();
                    let insert =
                        get_insert_range_or_end(&ctx2, ec, "end_composition direct insert")?;
                    if let Err(e) = insert.SetText(ec, 0, &text_w) {
                        tracing::warn!(
                            "end_composition: direct insert SetText failed text={:?}: {e}",
                            text
                        );
                    }
                }
                return Ok(());
            }
        };

        let text_w: Vec<u16> = text.encode_utf16().collect();
        tracing::debug!("end_composition[session]: text={:?}", text);
        let range = comp.GetRange().map_err(|e| {
            tracing::warn!("end_composition: GetRange failed text={:?}: {e}", text);
            windows::core::Error::new(E_FAIL, format!("GetRange: {e}"))
        })?;
        if let Err(e) = range.SetText(ec, 0, &text_w) {
            // 0x80040209 は TSF では TS_E_READONLY（ドキュメントが一時的に読み取り専用。
            // FormatMessage は同値の OLE エラー文字列を出すため紛らわしい）。
            // 一時的なロックなら同一セッション内の再試行で通ることがある。
            if let Err(e2) = range.SetText(ec, 0, &text_w) {
                // TSF DLL はアプリのプロセス内で動くため current_exe = 発生アプリ
                let app = std::env::current_exe()
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .unwrap_or_default();
                tracing::warn!(
                    "end_composition: SetText failed text={:?} app={:?}: {e} / retry: {e2}",
                    text,
                    app
                );
                // SetText できなくても EndComposition までは進め、表示中のテキスト
                // （preedit）をそのまま確定させる。中断すると composition が宙吊りに
                // なり確定テキストが丸ごと消える（7月に 2 件実測）。表示済みテキストの
                // 確定は WYSIWYG 不変条件の範囲内。
            }
        }

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

        comp.EndComposition(ec).map_err(|e| {
            tracing::warn!(
                "end_composition: EndComposition failed text={:?}: {e}",
                text
            );
            windows::core::Error::new(E_FAIL, format!("EndComposition: {e}"))
        })?;
        Ok(())
    });
    // 確定はユーザーテキストを失うと致命的なので、edit session の結果
    // (phrSession) まで確認して失敗を必ずログに残す。
    unsafe {
        match ctx.RequestEditSession(tid, &session, TF_ES_READWRITE) {
            Ok(hr) if hr.is_err() => {
                tracing::warn!("end_composition: edit session failed hr={hr:?}");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("end_composition: RequestEditSession failed: {e}");
            }
        }
    }
    Ok(())
}

pub(super) fn commit_text(ctx: ITfContext, tid: u32, text: String) -> Result<()> {
    use windows::Win32::Foundation::E_FAIL;

    let ctx_req = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
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
