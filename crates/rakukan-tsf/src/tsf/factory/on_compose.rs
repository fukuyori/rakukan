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

thread_local! {
    /// 表示の要求の世代（入力処理の見直しの段 6、設計書 4.2 の救済の締め切り）。救済の
    /// 締め切りで進め、それより前に要求した表示の EditSession を何もさせない。
    static DISPLAY_EPOCH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// composition に最後に SetText が成功した文字列と、その composition
    /// （`composition_identity`）。救済の締め切りの時点の表示（`shown_at_cutoff`）に使う。
    static SHOWN: std::cell::RefCell<(usize, String)> =
        const { std::cell::RefCell::new((0, String::new())) };
}

/// 未実行の表示の要求をすべて失効させる（救済の締め切り）。
pub(crate) fn void_pending_display() {
    DISPLAY_EPOCH.with(|e| e.set(e.get() + 1));
}

fn display_epoch() -> u64 {
    DISPLAY_EPOCH.with(|e| e.get())
}

/// 表示の要求を実行してよいか（要求の時点の世代のままで、救済の途中でない）。
fn display_allowed(epoch: u64) -> bool {
    display_epoch() == epoch && !crate::tsf::key_queue::rescue_active()
}

/// composition に SetText が成功した文字列を記録する（救済の締め切りの時点の表示に使う。
/// composition に SetText するすべての経路から呼ぶ）。
pub(crate) fn note_shown(comp: &windows::Win32::UI::TextServices::ITfComposition, text: &str) {
    let identity = crate::engine::state::composition_identity(comp);
    SHOWN.with(|s| *s.borrow_mut() = (identity, text.to_string()));
}

/// composition（`identity`）に最後に SetText が成功した文字列。一度も成功していなければ空。
pub(crate) fn shown_text(identity: usize) -> String {
    SHOWN.with(|s| {
        let s = s.borrow();
        if s.0 == identity {
            s.1.clone()
        } else {
            String::new()
        }
    })
}

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

    // 救済の途中は表示を更新しない（入力処理の見直しの段 6）
    if crate::tsf::key_queue::rescue_active() {
        tracing::debug!("update_composition: rescue in progress; display skipped");
        return Ok(());
    }
    let epoch = display_epoch();
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

        // 救済の締め切りで失効した・救済の途中: composition を作らず、SetText もしない
        if !display_allowed(epoch) {
            tracing::debug!("update_composition: display request voided by a rescue");
            return Ok(());
        }

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

        let (range, comp_for_shown) = if let Some(comp) = &existing {
            crate::engine::rpc_composition::bind_composition(
                bind,
                crate::engine::state::composition_identity(comp),
                crate::engine::state::composition_dm(),
            );
            let r = comp
                .GetRange()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange: {e}")))?;
            (r, comp.clone())
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
            let _ = composition_set_with_dm(Some(new_comp.clone()), dm_ptr);
            (r, new_comp)
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
            note_shown(&comp_for_shown, &preedit);
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

/// 部分確定の計画（入力処理の見直しの 5c-3、設計書 4.6 の部分確定の文書先行）。
pub(super) struct PartialPlan {
    /// 文書に確定する部分（計画）。文脈にもこの文字列を送る。
    pub commit_text: String,
    /// 確定部分の学習（文書に計画どおり確定できたときだけ、引き継ぎと一緒に送る）。
    pub learn: Option<crate::tsf::commit_txn::LearnPlan>,
    /// 新しい composition に引き継ぐ要求（打った文字・残りの読みの種）。
    pub then: rakukan_engine_rpc::ChangeRequest,
    /// 引き継いだ後の読みをセッションの Preedit にする。
    pub set_preedit: bool,
    /// 引き継いだ後の読みで BG 変換を始める（EditSession の後）。
    pub bg_start: bool,
    /// BG 変換を始めたら、ライブ変換のタイマーを始める。
    pub live_notify: bool,
    /// 確定してから入力するキー（CommitThenInput）。確定しなかったら、そのキーを列に戻して
    /// 確定からやり直す（入力処理の見直しの 5c-2、設計書 4.6 の `CommitThenInput`）。
    pub retry: bool,
}

/// EditSession の中のホストへの引き継ぎ（H）の結果。後処理（`commit_txn::set_after`）で使う。
enum Handoff {
    /// 引き継いだ。`display` は新しい composition に書く表示（読めなければ `None`）、
    /// `new_comp` は新しい composition に表示できたか。
    Restored {
        reading: String,
        display: Option<String>,
        new_comp: bool,
    },
    /// 引き継げなかった（新しい編集状態は journal を持って残っている）。
    Failed,
}

/// 部分確定: 文書の確定部分を確定し、ホストへ引き継いでから、その表示で新しい composition を
/// 始める（1 EditSession。入力処理の見直しの 5c-3、設計書 4.6 の部分確定の文書先行）。
///
/// end_composition + update_composition を別々に呼ぶと TSF が2セッションを
/// 別タイミングで実行し、"composition=None" の瞬間にアプリがテキストを
/// クリアすることがある。これを1セッションにまとめて防ぐ。
///
/// 呼び出し側は、ホストへ何も送らずに呼ぶ（文脈・読みのリセット・引き継ぎ・学習はこの関数が
/// 送る）。EditSession の中で、確定部分に縮めて EndComposition が成功した後にだけ、
/// `RestoreJournal` を 1 回送る（学習・文脈・リセット・引き継ぎ）。確定しなかったとき
/// （`Aborted` / `NotRun`）は composition を保持し直し、ホストには何も送らない
/// （セッションはキーの入口の状態に戻す: commit_txn）。BG 変換の開始・表示の要求し直しは、
/// EditSession の後の後処理で行う。EditSession が後で実行される場合、その間のキーは
/// key_queue が保留する。
pub(super) fn commit_then_start_composition(
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    plan: PartialPlan,
) -> Result<()> {
    use crate::tsf::commit_txn::{DocResult, ReqState};
    use windows::Win32::Foundation::E_FAIL;

    // M1.8 T-MID1 拡張: 確定はライブ変換の世代を進める。これをしないと、
    // 確定前に積まれた Phase1A/1B の遅延 SetText が gen 一致のまま
    // 確定後の新 composition に古い preview を書き込み、テキストが
    // 二重に見える競合が起こりうる。
    crate::tsf::live_session::conv_gen_bump();

    let cell = crate::tsf::commit_txn::begin(plan.commit_text.clone(), Default::default());
    let session_cell = cell.clone();
    // 実行の時点で、要求の時点と同じ文書・composition かを確かめるために控える（S0'）
    let req_dm = crate::tsf::candidate_window::current_dm_hwnd().0;
    let req_comp = crate::engine::state::composition_key();
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
    let handoff: std::rc::Rc<std::cell::RefCell<Option<Handoff>>> = Default::default();
    let session_handoff = handoff.clone();
    let commit_text = plan.commit_text.clone();
    let then = plan.then.clone();
    let learn = plan.learn.clone();
    let set_preedit = plan.set_preedit;
    let ctx_req = ctx.clone();
    let session_ctx = ctx.clone();
    let session_sink = sink.clone();
    let session = EditSession::new(move |ec| unsafe {
        use windows::Win32::UI::TextServices::{
            ITfContextComposition, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
        };
        let ctx = &session_ctx;
        crate::engine::state::clear_pending_replacement(replacement);
        // S0: 失効した要求は文書に触れない
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
        // S0': 要求の後に文書・composition が替わっていたら触れない
        if crate::tsf::candidate_window::current_dm_hwnd().0 != req_dm
            || crate::engine::state::composition_key() != req_comp
        {
            tracing::warn!("commit_then_start: document or composition changed; not run");
            finish(DocResult::NotRun("stale"));
            return Ok(());
        }
        // E: エンジンのロック（引き継ぎと表示の読み出しが済むまで持つ）
        let Ok(mut engine_guard) = crate::engine::state::engine_try_get() else {
            tracing::warn!("commit_then_start: engine busy; not run");
            finish(DocResult::NotRun("engine busy"));
            return Ok(());
        };
        if engine_guard.as_mut().is_none() {
            tracing::warn!("commit_then_start: no engine; not run");
            finish(DocResult::NotRun("no engine"));
            return Ok(());
        }

        // SetText 排他化（M1.8 T-MID3 の commit 版）: Phase1A / update_composition
        // の SetText と直列化する。他サイトは try_lock + skip だが、確定はスキップ
        // するとユーザーテキストが失われるためブロッキングで取得する。
        // 遅延実行された Phase1A がこのセッションの後に走っても、composition_take
        // で旧 composition が外れているため古い preview の書き込みは失敗する。
        let _apply_guard = crate::engine::state::COMPOSITION_APPLY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // 確定しなかったときに保持し直すため、取り出す前に文書を控える
        let comp_dm = crate::engine::state::composition_dm();
        let comp = composition_take().unwrap_or(None);
        tracing::debug!(
            "commit_then_start[session]: commit={:?} then={:?} has_comp={}",
            commit_text,
            then,
            comp.is_some()
        );
        // 確定しなかった: composition を保持し直す（5c-1 の commit_full と同じ）
        let keep = |comp: windows::Win32::UI::TextServices::ITfComposition,
                    why: &'static str,
                    changed: bool| {
            match composition_set_with_dm(Some(comp), comp_dm) {
                Ok(()) => DocResult::Aborted { why, changed },
                Err(e) => {
                    tracing::warn!(
                        "commit_then_start: re-hold composition failed after {why}: {e}"
                    );
                    DocResult::Failed(why)
                }
            }
        };

        // ── S1〜S4: 既存 composition を確定部分に縮めて終了 ──
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
            // S1
            let range = match comp.GetRange() {
                Ok(range) => range,
                Err(e) => {
                    tracing::warn!("commit_then_start: GetRange failed: {e}");
                    finish(keep(comp, "GetRange", false));
                    return Ok(());
                }
            };
            // S1': 確定しなかったときに戻す元の文字列
            let Some(original) = range_text(ec, &range) else {
                tracing::warn!("commit_then_start: original text unreadable");
                finish(keep(comp, "original text", false));
                return Ok(());
            };
            // 範囲を元の文字列へ戻す。戻せなければ範囲は変わったまま（表示が古い）
            let restore_original = || {
                let o_w: Vec<u16> = original.encode_utf16().collect();
                range.SetText(ec, 0, &o_w).is_err()
            };
            // S2: 確定部分に縮める。縮められないまま EndComposition へ進むと、確定部分に
            // 残りまで含まれる（Issue #69）ので止める
            if let Err(e) = range.SetText(ec, 0, &commit_w) {
                tracing::warn!(
                    "commit_then_start: SetText to the committed part failed commit={commit_text:?}: {e}"
                );
                finish(keep(comp, "SetText", false));
                return Ok(());
            }
            // S2': 縮めた範囲が確定部分と同じか
            let committed = range_text(ec, &range);
            if committed.as_deref() != Some(commit_text.as_str()) {
                tracing::warn!(
                    "commit_then_start: committed part differs planned={commit_text:?} read={committed:?}"
                );
                let changed = restore_original();
                finish(keep(comp, "committed part", changed));
                return Ok(());
            }
            // S3: 確定テキストの末尾位置を保存
            if let Ok(end_range) = range.Clone() {
                let _ = end_range.Collapse(ec, TF_ANCHOR_END);
                insert_after_commit = Some(end_range);
            }
            // S4
            if let Err(e) = comp.EndComposition(ec) {
                tracing::warn!("commit_then_start: EndComposition failed: {e}");
                let changed = restore_original();
                finish(keep(comp, "EndComposition", changed));
                return Ok(());
            }
        } else if !commit_text.is_empty() {
            let inserted = get_insert_range_or_end(ctx, ec, "commit_then_start direct commit")
                .and_then(|insert| insert.SetText(ec, 0, &commit_w).map(|()| insert));
            match inserted {
                Ok(insert) => {
                    if let Ok(end_range) = insert.Clone() {
                        let _ = end_range.Collapse(ec, TF_ANCHOR_END);
                        insert_after_commit = Some(end_range);
                    }
                }
                Err(e) => {
                    tracing::warn!("commit_then_start: direct commit failed: {e}");
                    finish(DocResult::Failed("direct commit"));
                    return Ok(());
                }
            }
        }
        // 選択位置を確定部分の末尾へ（新しい composition を始められなかったときに、後処理の
        // 表示の要求がここから始める）
        if let Some(point) = insert_after_commit.as_ref().and_then(|p| p.Clone().ok()) {
            let sel = TF_SELECTION {
                range: std::mem::ManuallyDrop::new(Some(point)),
                style: TF_SELECTIONSTYLE {
                    ase: TfActiveSelEnd(0),
                    fInterimChar: windows::Win32::Foundation::BOOL(0),
                },
            };
            let _ = ctx.SetSelection(ec, &[sel]);
        }

        // ── H: ホストへの引き継ぎ（RestoreJournal を 1 回）──
        let mut pre_effects = Vec::new();
        if let Some(learn) = &learn {
            use crate::engine::state::LearnAction;
            match learn.action {
                LearnAction::Learn => pre_effects.push(rakukan_engine_rpc::SideEffect::Learn {
                    reading: learn.reading.clone(),
                    surface: learn.surface.clone(),
                }),
                LearnAction::LearnForce => {
                    pre_effects.push(rakukan_engine_rpc::SideEffect::LearnForce {
                        reading: learn.reading.clone(),
                        surface: learn.surface.clone(),
                    })
                }
                LearnAction::Skip => {}
            }
        }
        if !commit_text.is_empty() {
            pre_effects.push(rakukan_engine_rpc::SideEffect::CommitContext {
                text: commit_text.clone(),
            });
        }
        let engine = engine_guard.as_mut().expect("checked");
        let host = match engine.partial_handoff(then.clone(), pre_effects) {
            Ok(reading) => {
                let display = engine.preedit_display().ok();
                if set_preedit && let Ok(mut sess) = crate::engine::state::session_get() {
                    sess.set_preedit(reading.clone());
                }
                Some((reading, display))
            }
            Err(e) => {
                tracing::warn!("commit_then_start: handoff to the host failed: {e}");
                // 種の読みは分かっている（次のキーで流し直す）
                if set_preedit
                    && let rakukan_engine_rpc::ChangeRequest::SeedPreedit { text } = &then
                    && let Ok(mut sess) = crate::engine::state::session_get()
                {
                    sess.set_preedit(text.clone());
                }
                None
            }
        };
        drop(engine_guard);
        // 結果を記録する（この後の手順が失敗しても記録は済んでいる）
        finish(DocResult::Committed {
            text: commit_text.clone(),
        });
        let Some((reading, display)) = host else {
            *session_handoff.borrow_mut() = Some(Handoff::Failed);
            return Ok(());
        };
        // 新しい composition を始められたかを、抜ける経路にかかわらず記録する
        let record = |new_comp: bool| {
            *session_handoff.borrow_mut() = Some(Handoff::Restored {
                reading: reading.clone(),
                display: display.clone(),
                new_comp,
            });
        };
        record(false);
        let Some(next) = display.clone() else {
            tracing::warn!("commit_then_start: display unreadable after the handoff");
            return Ok(());
        };
        if next.is_empty() {
            record(true);
            return Ok(());
        }

        // ── S5〜S8: 同セッション内で新 composition 開始 ──
        // EndComposition 前に保存した確定テキスト末尾位置から新 composition を開始する。
        // EndComposition 後の GetSelection はカーソルが composition 開始位置を示すことがあり
        // 使用できない。ctx.GetEnd(ec) はドキュメント末尾を返すため文章途中の編集で問題になる。
        let warn_step = |e: &windows::core::Error| {
            tracing::warn!(
                "commit_then_start: {e} commit={:?} next={:?}",
                commit_text,
                next
            );
        };
        let insert_point = if let Some(p) = insert_after_commit {
            p
        } else {
            tracing::warn!("commit_then_start: insert_after_commit=None, falling back to GetEnd");
            get_document_end_range(ctx, ec, "commit_then_start new composition")
                .inspect_err(warn_step)?
        };
        let cc: ITfContextComposition = ctx
            .cast()
            .map_err(|e| {
                windows::core::Error::new(E_FAIL, format!("cast ITfContextComposition: {e}"))
            })
            .inspect_err(warn_step)?;
        let new_comp = cc
            .StartComposition(ec, &insert_point, &session_sink)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("StartComposition: {e}")))
            .inspect_err(warn_step)?;
        // 開始した composition は、すぐ保持して新しい編集状態に結び付ける（範囲の取得や表示が
        // 失敗しても、後処理の表示の要求がこの composition に書く）
        let dm_ptr = ctx
            .GetDocumentMgr()
            .ok()
            .map(|dm| dm.as_raw() as usize)
            .unwrap_or(0);
        crate::engine::rpc_composition::bind_composition(
            crate::engine::rpc_composition::bind_target(),
            crate::engine::state::composition_identity(&new_comp),
            dm_ptr,
        );
        let _ = composition_set_with_dm(Some(new_comp.clone()), dm_ptr);
        let new_range = new_comp
            .GetRange()
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange new: {e}")))
            .inspect_err(warn_step)?;
        let preedit_w: Vec<u16> = next.encode_utf16().collect();
        new_range
            .SetText(ec, 0, &preedit_w)
            .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText new: {e}")))
            .inspect_err(warn_step)?;
        note_shown(&new_comp, &next);
        record(true);

        // 新 composition にもアンダーライン属性をセット
        set_display_attr_prop(ctx, ec, &new_range, display_attr::atom_input());

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
    // 後処理（保留したキーの再生より前）
    let after_ctx = ctx.clone();
    let after = AfterPartial {
        bg_start: plan.bg_start,
        live_notify: plan.live_notify,
        // 確定しなかったら列に戻すキー（今処理しているキー）
        retry_key: plan
            .retry
            .then(crate::tsf::key_queue::current_key)
            .flatten(),
    };
    crate::tsf::commit_txn::set_after(
        &cell,
        Box::new(move |result, keep| {
            after_partial(
                result,
                keep,
                handoff.borrow_mut().take(),
                after_ctx,
                tid,
                sink,
                after,
            )
        }),
    );
    // end_composition と同じく、edit session の結果（phrSession）まで確認して
    // 失敗をログに残す（Issue #69）。
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
    // 同期で実行されていれば、ここで後処理する。後で実行される場合は、結果が出てから
    // （WM_APP_COMMIT_DONE。その間のキーは key_queue が保留する）
    crate::tsf::commit_txn::finish_completed();
    if crate::tsf::commit_txn::commit_pending() {
        crate::tsf::candidate_window::start_key_queue_timer();
    }
    Ok(())
}

/// 部分確定の後処理の指定（[`after_partial`]）。
struct AfterPartial {
    bg_start: bool,
    live_notify: bool,
    /// 確定しなかったら列に戻すキー（CommitThenInput のキーの通し番号と写し。5c-2）。
    retry_key: Option<(u64, crate::tsf::key_queue::HeldKey)>,
}

/// 確定しなかった、確定してから入力するキーの扱い（5c-2）。
#[derive(Debug, PartialEq, Eq)]
enum RetryStep {
    /// やり直さない（確定した・やり直す対象のキーでない）。
    None,
    /// 列に戻してやり直す。`redisplay` なら、先に表示を作り直す（範囲が縮めた文字列のまま）。
    Retry { redisplay: bool },
}

/// 結果から、確定してから入力するキーをやり直すかを決める。確定しなかった（文書にも
/// ホストにも触れていない）ときは、composition が残っているかにかかわらず、打った文字を
/// 捨てずにやり直す（再生で今の状態から処理し直す。フォーカスが移っていれば元の文書へ入れる）。
fn retry_step(result: &crate::tsf::commit_txn::DocResult, has_key: bool) -> RetryStep {
    use crate::tsf::commit_txn::DocResult;
    match result {
        _ if !has_key => RetryStep::None,
        DocResult::Aborted { changed, .. } => RetryStep::Retry {
            redisplay: *changed,
        },
        DocResult::NotRun(_) => RetryStep::Retry { redisplay: false },
        _ => RetryStep::None,
    }
}

/// 変換中のセッションの表示を、composition に書き直す要求を出す（確定しなかったが範囲が
/// 縮めた文字列のまま残ったとき。5c-2）。変換中でなければ何もしない。
fn redisplay_conversion(ctx: ITfContext, tid: u32, sink: ITfCompositionSink) {
    let parts = crate::engine::state::session_get().ok().and_then(|sess| {
        if sess.is_block_selecting() {
            sess.block_selecting_composition_parts()
        } else if sess.is_selecting() {
            let text = sess
                .current_candidate()
                .or_else(|| sess.original_preedit())
                .unwrap_or("")
                .to_string();
            Some((
                sess.selecting_prefix_clone(),
                text,
                sess.selecting_remainder_clone(),
            ))
        } else {
            None
        }
    });
    if let Some((prefix, text, remainder)) = parts
        && let Err(e) = update_composition_candidate_parts(ctx, tid, sink, prefix, text, remainder)
    {
        tracing::warn!("commit_then_start: redisplay request failed: {e}");
    }
}

/// 部分確定の後処理（EditSession の後、保留したキーの再生より前。設計書 4.6 の部分確定の
/// 文書先行）。
fn after_partial(
    result: &crate::tsf::commit_txn::DocResult,
    keep: bool,
    handoff: Option<Handoff>,
    ctx: ITfContext,
    tid: u32,
    sink: ITfCompositionSink,
    after: AfterPartial,
) {
    use crate::tsf::commit_txn::DocResult;
    let AfterPartial {
        bg_start,
        live_notify,
        retry_key,
    } = after;
    // 確定してから入力するキー: 確定しなかったら列に戻してやり直す（5c-2）
    match retry_step(result, retry_key.is_some()) {
        RetryStep::Retry { redisplay } => {
            let (seq, key) = retry_key.expect("checked");
            if redisplay {
                redisplay_conversion(ctx.clone(), tid, sink.clone());
            }
            if !crate::tsf::key_queue::retry_after_commit(seq, key) {
                tracing::warn!(
                    "commit_then_start: commit not done after {} retries; key dropped seq={seq}",
                    crate::tsf::key_queue::COMMIT_RETRY_LIMIT
                );
                crate::tsf::mode_indicator::show("文書へ書き込めません", 0, 0);
            }
        }
        RetryStep::None => {
            if let Some((seq, _)) = &retry_key {
                crate::tsf::key_queue::forget_commit_retries(*seq);
            }
        }
    }
    match (result, handoff) {
        (
            DocResult::Committed { .. },
            Some(Handoff::Restored {
                reading,
                display,
                new_comp,
            }),
        ) => {
            // 新しい composition に表示できなかった: 表示の要求を 1 回だけ出す（失効・再要求の
            // 仕組みは持たない。次のキーの表示が上書きする）
            if !new_comp {
                let display = display.or_else(|| {
                    crate::engine::state::engine_get()
                        .ok()
                        .and_then(|mut g| g.as_mut().and_then(|e| e.preedit_display().ok()))
                });
                match display {
                    Some(display) => {
                        if let Err(e) = update_composition(ctx.clone(), tid, sink, display) {
                            tracing::warn!("commit_then_start: display request failed: {e}");
                        }
                    }
                    None => tracing::warn!(
                        "commit_then_start: no display for the handed-off input; the next key shows it"
                    ),
                }
            }
            if bg_start {
                let started = crate::engine::state::engine_get().ok().is_some_and(|g| {
                    g.as_ref()
                        .is_some_and(|e| crate::engine::state::start_live_bg_if_ready(e, &reading))
                });
                if started && live_notify {
                    crate::tsf::candidate_window::live_input_notify(&ctx, tid);
                }
            }
        }
        (DocResult::Committed { .. }, _) => {
            // 確定部分は確定したが、ホストへ引き継げなかった。回復できない切断なら、引き継ぐ
            // 文字を打った文字のまま入れる（第 9 版の救済）。そうでなければ、次のキーで
            // 新しい編集状態の journal を流し直す
            if crate::tsf::key_queue::link_unrecoverable() {
                if let Some((text, skipped)) =
                    crate::engine::rpc_composition::take_unconfirmed_tail()
                {
                    tracing::warn!(
                        "commit_then_start: host unavailable; inserting the handed-off input as typed ({skipped} other request(s) dropped)"
                    );
                    if !text.is_empty() {
                        let dm = crate::tsf::candidate_window::current_dm_hwnd().0;
                        let _ = insert_text_direct(
                            &ctx,
                            tid,
                            &text,
                            Box::new(move || {
                                crate::tsf::candidate_window::current_dm_hwnd().0 == dm
                            }),
                        );
                    }
                }
            } else {
                tracing::warn!(
                    "commit_then_start: handoff not confirmed; the next key replays the journal"
                );
            }
        }
        // 確定しなかった: composition もホストの読みも残す（セッションは commit_txn が戻した）
        _ if keep => {}
        // 確定しなかった（文書にもホストにも触れていない）が、要求の時点の composition が今は
        // 無い・別のものに替わった: 今のホストの状態は替わった先のもの（終わった composition の
        // 状態は終了通知の後始末が閉じる）なので触れない
        (DocResult::Aborted { .. } | DocResult::NotRun(_), _) => {
            tracing::warn!(
                "commit_then_start: not committed and the composition changed; host state left as is"
            );
        }
        // 確定に失敗し、composition を手放した: ホストの古い読みを閉じる
        _ => {
            if let Ok(mut guard) = crate::engine::state::engine_get()
                && let Some(engine) = guard.as_mut()
            {
                engine.bg_reclaim();
                engine.reset_preedit();
            } else {
                tracing::warn!("commit_then_start: no engine; the old reading is not closed");
            }
        }
    }
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
            // 文書が変わっていないと確かめられないので、入れ直さない（二重にしない）
            finish(InsertState::NotInserted {
                reason: "SetText",
                retryable: false,
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

/// composition の救済の要求の状態（[`rescue_composition`]）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RescueStep {
    Pending,
    Running,
    Done(RescueOutcome),
}

/// composition の救済の結果（設計書 4.2 の手順 4 の表の `RescueOutcome`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RescueOutcome {
    /// 全部入れて確定した。
    Rescued,
    /// 文書は変わっていない（要求が実行されなかった・SetText の失敗の後も表示のまま）。
    /// 新しい要求でもう一度試してよい。
    Retryable,
    /// 書いたが EndComposition に失敗した（確定していない）。
    WrittenNotEnded,
    /// 表示のまま確定した・アプリが確定した・範囲が編集されていた。`unrescued` は入らな
    /// かった文字列。`ended` は、この要求が composition を終えたか。
    PartlyKept { unrescued: String, ended: bool },
    /// 書かずにやめた（composition には触れずに残す）。
    Stopped { unrescued: String },
    /// 失効した（Deactivate・文書の破棄）。文書に触れていない。
    Expired,
}

/// 締め切りの時点の表示 `shown` に出ていなかった部分（`shown` が `target` の先頭でなければ、
/// `target` 全体を確かめられないものとする）。
pub(crate) fn not_shown_part(target: &str, shown: &str) -> String {
    target
        .strip_prefix(shown)
        .map_or_else(|| target.to_string(), str::to_string)
}

/// composition の救済の要求を出す（入力処理の見直しの段 6、設計書 4.2 の `RescueTxn`）。
///
/// 1 つの EditSession の中で、実行の時点の状態で 1 回だけ判断する（設計書 4.2 の手順 4 の表）:
/// - composition `identity` を保持し、範囲が `shown` のまま: 範囲を `target` に SetText して
///   EndComposition（`last` なら SetText せず EndComposition だけ: 手順 5 の最後の 1 回）。
///   SetText に失敗したら範囲を読み直し、`shown` のままなら `Retryable`
/// - 範囲が `shown` と違う（アプリが編集した）: 上書きせず EndComposition だけ
/// - composition を保持していない（アプリが確定した）: 書かない
///
/// `still_valid` は実行の時点で呼び、偽なら文書に触れずに `Expired` にする。結果は返した
/// 箱に記録し、`WM_APP_COMMIT_DONE` を送る。箱が `Pending` のままなら、呼び出し側が失効
/// （`Done(Retryable)`）にしてよい（失効した要求は、後で実行されても文書に触れない）。
pub(super) fn rescue_composition(
    ctx: &ITfContext,
    tid: u32,
    identity: usize,
    shown: String,
    target: String,
    last: bool,
    still_valid: Box<dyn Fn() -> bool>,
) -> std::rc::Rc<std::cell::RefCell<RescueStep>> {
    use windows::Win32::UI::TextServices::{
        TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE, TfActiveSelEnd,
    };
    let state = std::rc::Rc::new(std::cell::RefCell::new(RescueStep::Pending));
    let session_state = state.clone();
    let session_ctx = ctx.clone();
    let session = EditSession::new(move |ec| unsafe {
        if *session_state.borrow() != RescueStep::Pending {
            return Ok(());
        }
        let finish = |outcome: RescueOutcome| {
            *session_state.borrow_mut() = RescueStep::Done(outcome);
            crate::tsf::candidate_window::post_commit_done();
        };
        if !still_valid() {
            finish(RescueOutcome::Expired);
            return Ok(());
        }
        *session_state.borrow_mut() = RescueStep::Running;
        let rest = not_shown_part(&target, &shown);
        let _apply_guard = crate::engine::state::COMPOSITION_APPLY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let comp = composition_clone()
            .ok()
            .flatten()
            .filter(|c| crate::engine::state::composition_identity(c) == identity);
        let Some(comp) = comp else {
            // 締め切りの後にアプリが確定した（終了の通知を受けた）: 書かない
            finish(RescueOutcome::PartlyKept {
                unrescued: rest,
                ended: false,
            });
            return Ok(());
        };
        let Ok(range) = comp.GetRange() else {
            finish(RescueOutcome::Stopped {
                unrescued: target.clone(),
            });
            return Ok(());
        };
        let Some(now) = range_text(ec, &range) else {
            finish(RescueOutcome::Stopped {
                unrescued: target.clone(),
            });
            return Ok(());
        };
        // 確定の後の選択位置を範囲の末尾へ（EndComposition の前に置く: end_composition と同じ）
        let caret_to_end = |range: &windows::Win32::UI::TextServices::ITfRange| {
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
                let [sel] = sels;
                drop(std::mem::ManuallyDrop::into_inner(sel.range));
            }
        };
        let end = || -> bool {
            match comp.EndComposition(ec) {
                Ok(()) => {
                    let _ = composition_take();
                    true
                }
                Err(e) => {
                    tracing::warn!("rescue_composition: EndComposition failed: {e}");
                    false
                }
            }
        };
        if now != shown || last {
            // 範囲が締め切りの時点と違う（利用者の編集を残す）、または予算を使い切った
            // （表示どおりに確定する）: 上書きせず EndComposition だけ
            if now != shown {
                tracing::warn!(
                    "rescue_composition: the composition was edited after the cutoff; ending it as is"
                );
            }
            caret_to_end(&range);
            let ended = end();
            if last && now == shown && !ended {
                finish(RescueOutcome::Stopped {
                    unrescued: target.clone(),
                });
            } else {
                finish(RescueOutcome::PartlyKept {
                    unrescued: rest,
                    ended,
                });
            }
            return Ok(());
        }
        let target_w: Vec<u16> = target.encode_utf16().collect();
        if let Err(e) = range.SetText(ec, 0, &target_w) {
            tracing::warn!("rescue_composition: SetText failed: {e}");
            // 読み直して、表示のままなら何も変わっていない
            let outcome = match range_text(ec, &range) {
                Some(t) if t == shown => RescueOutcome::Retryable,
                _ => RescueOutcome::Stopped {
                    unrescued: target.clone(),
                },
            };
            finish(outcome);
            return Ok(());
        }
        caret_to_end(&range);
        // 書いた後は元に戻さず、再試行もしない（EndComposition の結果は不確か）
        if end() {
            finish(RescueOutcome::Rescued);
        } else {
            finish(RescueOutcome::WrittenNotEnded);
        }
        Ok(())
    });
    match unsafe { ctx.RequestEditSession(tid, &session, TF_ES_READWRITE) } {
        Ok(hr) if hr.is_ok() => {}
        Ok(hr) => {
            tracing::warn!("rescue_composition: edit session refused hr={hr:?}");
            if *state.borrow() == RescueStep::Pending {
                *state.borrow_mut() = RescueStep::Done(RescueOutcome::Retryable);
            }
        }
        Err(e) => {
            tracing::warn!("rescue_composition: request failed: {e}");
            if *state.borrow() == RescueStep::Pending {
                *state.borrow_mut() = RescueStep::Done(RescueOutcome::Retryable);
            }
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

    // 救済の途中は表示を更新しない（入力処理の見直しの段 6）
    if crate::tsf::key_queue::rescue_active() {
        tracing::debug!("update_composition_candidate_parts: rescue in progress; display skipped");
        return Ok(());
    }
    let epoch = display_epoch();
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

        // 救済の締め切りで失効した・救済の途中: composition を作らず、SetText もしない
        if !display_allowed(epoch) {
            tracing::debug!(
                "update_composition_candidate_parts: display request voided by a rescue"
            );
            return Ok(());
        }

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
        let (range, comp_for_shown) = if let Some(comp) = &existing {
            crate::engine::rpc_composition::bind_composition(
                bind,
                crate::engine::state::composition_identity(comp),
                crate::engine::state::composition_dm(),
            );
            let r = comp
                .GetRange()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange: {e}")))?;
            (r, comp.clone())
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
            let _ = composition_set_with_dm(Some(new_comp.clone()), dm_ptr);
            (r, new_comp)
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
            note_shown(&comp_for_shown, &full);
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
    // 確定しなかったときに composition を保持し直すか（IME の切り替えの前の確定は手放す）
    let keep_on_abort = !effects.close_on_abort;
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
        // 確定しなかったときに保持し直すため、取り出す前に文書を控える
        let comp_dm = crate::engine::state::composition_dm();
        let comp = composition_take().unwrap_or(None);
        let text_w: Vec<u16> = text.encode_utf16().collect();
        // 確定しなかった: composition を保持し直す（入力処理の見直しの段 5 の 5c の残り、
        // 設計書 4.6 の「取り出した composition は確定しなかった結果で必ず戻す」）
        let keep = |comp: windows::Win32::UI::TextServices::ITfComposition,
                    why: &'static str,
                    changed: bool| {
            if !keep_on_abort {
                // composition は今までどおり手放す
                return DocResult::Failed(why);
            }
            match composition_set_with_dm(Some(comp), comp_dm) {
                Ok(()) => DocResult::Aborted { why, changed },
                Err(e) => {
                    // 保持し直せなかった: composition を手放した扱いで後処理する
                    tracing::warn!("commit_full: re-hold composition failed after {why}: {e}");
                    DocResult::Failed(why)
                }
            }
        };
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
                finish(keep(comp, "GetRange", false));
                return Err(windows::core::Error::new(E_FAIL, format!("GetRange: {e}")));
            }
        };
        // 置き換えに失敗したときに確定する文字列（表示中の文字列）を知るため、先に読む
        let original = range_text(ec, &range);
        let mut committed = Some(text.clone());
        // 範囲を置き換えたか（確定しなかったときに元の文字列へ戻す必要があるか）
        let mut replaced = true;
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
                replaced = false;
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
            // 確定しなかった: 置き換えた範囲を元の文字列へ戻し、composition を保持し直す。
            // 戻せなければ、置き換えた文字列のまま残る（表示が古い）
            let changed = replaced
                && !original.as_ref().is_some_and(|o| {
                    let o_w: Vec<u16> = o.encode_utf16().collect();
                    range.SetText(ec, 0, &o_w).is_ok()
                });
            finish(keep(comp, "EndComposition", changed));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tsf::commit_txn::DocResult;

    #[test]
    fn a_commit_then_input_key_is_retried_only_when_not_committed() {
        // 確定しなかった: やり直す。範囲が縮めた文字列のまま残ったら表示を作り直す
        assert_eq!(
            retry_step(
                &DocResult::Aborted {
                    why: "EndComposition",
                    changed: true
                },
                true
            ),
            RetryStep::Retry { redisplay: true }
        );
        assert_eq!(
            retry_step(
                &DocResult::Aborted {
                    why: "GetRange",
                    changed: false
                },
                true
            ),
            RetryStep::Retry { redisplay: false }
        );
        assert_eq!(
            retry_step(&DocResult::NotRun("engine busy"), true),
            RetryStep::Retry { redisplay: false }
        );
        // 確定した・確定に失敗して composition を手放した: やり直さない
        assert_eq!(
            retry_step(
                &DocResult::Committed {
                    text: "漢字".into()
                },
                true
            ),
            RetryStep::None
        );
        assert_eq!(
            retry_step(&DocResult::Failed("direct commit"), true),
            RetryStep::None
        );
        // やり直す対象のキーでない（Enter・番号キー・RangeSelect）
        assert_eq!(
            retry_step(&DocResult::NotRun("timeout"), false),
            RetryStep::None
        );
    }
}
