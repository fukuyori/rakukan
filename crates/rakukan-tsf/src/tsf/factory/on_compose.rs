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
        view.GetTextExt(ec, &range, &mut rect, &mut clipped)
            .ok()?;
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
    let range_ref = (&*sel_buf[0].range).as_ref()?;
    let cloned = unsafe { range_ref.Clone() }.ok()?;
    if let Err(e) = unsafe { cloned.Collapse(ec, TF_ANCHOR_END) } {
        tracing::warn!("get_cursor_range: Collapse failed: {e}, range may not be zero-length");
    }
    Some(cloned)
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
pub(super) fn commit_then_start_composition(
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    commit_text: String,
    next_preedit: String,
) -> Result<()> {
    use windows::Win32::Foundation::E_FAIL;

    // composition_take() をセッション内に移動する（end_composition と同じ理由）。
    // セッション外で take すると COMPOSITION=None になった瞬間に update_composition が
    // 誤ったカーソル位置から新 composition を開始するリスクがある。
    let ctx_req = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            ITfContextComposition, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
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
pub(super) fn update_composition_candidate_parts(
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    prefix: String,
    converted: String,
    suffix: String,
) -> Result<()> {
    use windows::Win32::Foundation::E_FAIL;

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
pub(super) fn update_caret_rect(ctx: ITfContext, tid: u32) {
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

pub(super) fn end_composition(ctx: ITfContext, tid: u32, text: String) -> Result<()> {
    use windows::Win32::Foundation::E_FAIL;
    use windows::Win32::UI::TextServices::{
        TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
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
