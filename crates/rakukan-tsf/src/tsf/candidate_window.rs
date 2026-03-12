//! 候補ウィンドウ (Win32 ポップアップ)
//!
//! 変換候補を番号付きリストで表示する軽量 Win32 ウィンドウ。
//!
//! # スレッド安全性
//! TSF は STA で動作するため、すべての操作は同一 UIスレッドから呼ばれる。
//! HWND/CandData は `thread_local!` で管理し、Send/Sync を回避する。
//!
//! # ウィンドウ仕様
//! - `WS_POPUP | WS_BORDER`、`WS_EX_TOPMOST | WS_EX_NOACTIVATE`
//! - GDI で番号付きリスト描画（選択行はハイライト）
//! - キャレット位置の直下に表示
//!
//! # LLM 完了ポーリング（`WM_TIMER` ベース）
//! Waiting 状態（⏳ 変換中）に遷移した際に `start_waiting_timer()` を呼ぶことで、
//! 80ms ごとに `bg_status == "done"` をポーリングする `WM_TIMER` を起動する。
//! LLM 変換完了を検知したら候補ウィンドウを自動更新し、タイマーを停止する。
//!
//! TSF の `RequestEditSession` は TSF スレッドのキー入力コンテキスト外から呼べないため、
//! タイマーコールバックでは候補ウィンドウの表示のみ行い、composition text の更新は
//! 次のキー入力（Space 等）時の `waiting-poll` ブランチで行う。

use std::cell::{Cell, RefCell};

use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect,
            InvalidateRect, PAINTSTRUCT, SelectObject, SetBkMode,
            SetTextColor, TextOutW, BACKGROUND_MODE, HDC,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos,
            ShowWindow, HMENU, HWND_TOPMOST, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
            WM_ERASEBKGND, WM_PAINT, WM_TIMER, WNDCLASSW, WS_BORDER,
            WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_POPUP,
            SetTimer, KillTimer,
        },
    },
};

// ─── レイアウト定数 ───────────────────────────────────────────────────────────

const PADDING_X: i32 = 10;
const PADDING_Y: i32 = 4;
const ITEM_HEIGHT: i32 = 26;
const FONT_HEIGHT: i32 = 17;
const WIN_WIDTH: i32 = 260;
/// ページインジケーター行の高さ
const PAGER_HEIGHT: i32 = 22;

/// 選択行のハイライト色（濃い青）
const COLOR_SEL_BG: COLORREF = COLORREF(0x00_B4_4E_20); // #204EB4 → BGR
/// 選択行のテキスト色（白）
const COLOR_SEL_FG: COLORREF = COLORREF(0x00_FF_FF_FF);
/// 通常行の背景色（白）
const COLOR_BG: COLORREF = COLORREF(0x00_FF_FF_FF);
/// 通常行のテキスト色（黒）
const COLOR_FG: COLORREF = COLORREF(0x00_00_00_00);

// ─── スレッドローカル状態 ──────────────────────────────────────────────────────

thread_local! {
    /// 候補ウィンドウの HWND（null = 未作成）
    static TL_HWND: Cell<isize> = Cell::new(0);
    /// 表示中の候補データ（WM_PAINT コールバックで参照）
    static TL_CAND: RefCell<CandData> = RefCell::new(CandData::default());
}

#[derive(Default, Clone)]
struct CandData {
    /// 現在ページの候補（最大 page_size 件）
    candidates: Vec<String>,
    /// ページ内選択インデックス
    selected:   usize,
    /// ページ表示文字列（例: "2/4"、1ページのみなら空）
    page_info:  String,
    /// 候補の上に表示するステータス行（選択不可、グレー表示）
    status_line: Option<String>,
}

// ─── HWND ヘルパー ────────────────────────────────────────────────────────────

#[inline]
fn get_hwnd() -> HWND {
    TL_HWND.with(|c| HWND(c.get() as *mut _))
}

#[inline]
fn set_hwnd(hwnd: HWND) {
    TL_HWND.with(|c| c.set(hwnd.0 as isize));
}

#[inline]
fn is_valid(hwnd: HWND) -> bool {
    !hwnd.0.is_null()
}

// ─── ウィンドウクラス登録（一度だけ）────────────────────────────────────────

const CLASS_NAME_UTF16: &[u16] = &[
    b'R' as u16, b'a' as u16, b'k' as u16, b'u' as u16, b'n' as u16,
    b'C' as u16, b'a' as u16, b'n' as u16, b'd' as u16, 0u16,
];

unsafe fn ensure_class_registered() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let hmod = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc:  Some(wnd_proc),
            hInstance:    hmod.into(),
            lpszClassName: PCWSTR(CLASS_NAME_UTF16.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);
        tracing::debug!("候補ウィンドウクラス登録");
    });
}

// ─── WNDPROC ──────────────────────────────────────────────────────────────────

unsafe extern "system" fn wnd_proc(
    hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            if !hdc.is_invalid() {
                draw(hdc);
                let _ = EndPaint(hwnd, &ps);
            }
            LRESULT(0)
        }
        // 背景消去を抑制（WM_PAINT で全描画するため）
        WM_ERASEBKGND => LRESULT(1),
        // LLM完了ポーリングタイマー
        WM_TIMER => {
            if wparam.0 == WAITING_TIMER_ID {
                crate::tsf::candidate_window::on_waiting_timer();
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ─── 描画 ─────────────────────────────────────────────────────────────────────

unsafe fn draw(hdc: HDC) {
    let data = TL_CAND.with(|c| c.borrow().clone());
    if data.candidates.is_empty() {
        return;
    }
    let n          = data.candidates.len();
    let has_pager  = !data.page_info.is_empty();
    let has_status = data.status_line.is_some();
    let win_h      = window_height(n, has_pager, has_status);

    // 背景を白で塗りつぶし
    let bg_brush = CreateSolidBrush(COLOR_BG);
    let full = RECT { left: 0, top: 0, right: WIN_WIDTH, bottom: win_h };
    FillRect(hdc, &full, bg_brush);
    let _ = DeleteObject(bg_brush);

    // フォント
    let face: Vec<u16> = "Meiryo UI\0".encode_utf16().collect();
    let font = CreateFontW(
        FONT_HEIGHT, 0, 0, 0,
        400,          // FW_NORMAL
        0, 0, 0,
        1,            // DEFAULT_CHARSET
        0, 0,
        0,
        0,
        PCWSTR(face.as_ptr()),
    );
    let old_obj = SelectObject(hdc, font);
    SetBkMode(hdc, BACKGROUND_MODE(1)); // TRANSPARENT

    let sel_brush    = CreateSolidBrush(COLOR_SEL_BG);
    let wht_brush    = CreateSolidBrush(COLOR_BG);
    let pager_brush  = CreateSolidBrush(COLORREF(0x00_F0_F0_F0));
    let status_brush = CreateSolidBrush(COLORREF(0x00_F8_F8_F8));

    // ステータス行（先頭・グレー背景・番号なし・選択不可）
    let status_offset = if has_status {
        if let Some(ref s) = data.status_line {
            let row = RECT { left: 0, top: PADDING_Y, right: WIN_WIDTH, bottom: PADDING_Y + STATUS_HEIGHT };
            FillRect(hdc, &row, status_brush);
            SetTextColor(hdc, COLORREF(0x00_88_88_88));
            let text_w: Vec<u16> = s.encode_utf16().collect();
            let _ = TextOutW(hdc, PADDING_X, PADDING_Y + (STATUS_HEIGHT - FONT_HEIGHT) / 2, &text_w);
        }
        STATUS_HEIGHT
    } else { 0 };

    // 候補行
    for (i, cand) in data.candidates.iter().enumerate() {
        let y = PADDING_Y + status_offset + i as i32 * ITEM_HEIGHT;
        let row = RECT { left: 0, top: y, right: WIN_WIDTH, bottom: y + ITEM_HEIGHT };
        let is_sel = i == data.selected;
        FillRect(hdc, &row, if is_sel { sel_brush } else { wht_brush });
        SetTextColor(hdc, if is_sel { COLOR_SEL_FG } else { COLOR_FG });
        let text = format!("{} {}", i + 1, cand);
        let text_w: Vec<u16> = text.encode_utf16().collect();
        let _ = TextOutW(hdc, PADDING_X, y + (ITEM_HEIGHT - FONT_HEIGHT) / 2, &text_w);
    }

    // ページインジケーター行（複数ページがある場合のみ）
    if has_pager {
        let y = PADDING_Y + status_offset + n as i32 * ITEM_HEIGHT;
        let row = RECT { left: 0, top: y, right: WIN_WIDTH, bottom: y + PAGER_HEIGHT };
        FillRect(hdc, &row, pager_brush);
        let _ = windows::Win32::Graphics::Gdi::MoveToEx(hdc, 0, y, None);
        let _ = windows::Win32::Graphics::Gdi::LineTo(hdc, WIN_WIDTH, y);
        SetTextColor(hdc, COLORREF(0x00_55_55_55));
        let pager_text = format!("◀  {}  ▶", data.page_info);
        let pager_w: Vec<u16> = pager_text.encode_utf16().collect();
        let _ = TextOutW(hdc, PADDING_X, y + (PAGER_HEIGHT - FONT_HEIGHT) / 2, &pager_w);
    }

    let _ = DeleteObject(sel_brush);
    let _ = DeleteObject(wht_brush);
    let _ = DeleteObject(pager_brush);
    let _ = DeleteObject(status_brush);
    SelectObject(hdc, old_obj);
    let _ = DeleteObject(font);
}

const STATUS_HEIGHT: i32 = 22;

#[inline]
fn window_height(n: usize, has_pager: bool, has_status: bool) -> i32 {
    let base = PADDING_Y * 2 + n as i32 * ITEM_HEIGHT;
    let with_pager = if has_pager { base + PAGER_HEIGHT } else { base };
    if has_status { with_pager + STATUS_HEIGHT } else { with_pager }
}

// ─── 公開 API ────────────────────────────────────────────────────────────────

/// 候補ウィンドウを表示または更新する。
///
/// - `page_candidates`: 現在ページの候補スライス（最大 9 件）
/// - `page_selected`: ページ内の選択インデックス（0-origin）
/// - `page_info`: ページ表示文字列（例: "2/4"）。1ページのみなら ""
/// - `x`, `y`: 表示位置（スクリーン座標）
pub fn show(page_candidates: &[String], page_selected: usize, page_info: &str, x: i32, y: i32) {
    show_with_status(page_candidates, page_selected, page_info, x, y, None);
}

pub fn show_with_status(page_candidates: &[String], page_selected: usize, page_info: &str, x: i32, y: i32, status_line: Option<&str>) {
    if page_candidates.is_empty() {
        hide();
        return;
    }

    let has_pager  = !page_info.is_empty();
    let has_status = status_line.is_some();

    TL_CAND.with(|c| {
        let mut d = c.borrow_mut();
        if d.candidates.as_slice() != page_candidates {
            d.candidates = page_candidates.to_vec();
        }
        d.selected   = page_selected;
        d.page_info  = page_info.to_string();
        d.status_line = status_line.map(|s| s.to_string());
    });

    let n     = page_candidates.len();
    let win_h = window_height(n, has_pager, has_status);
    let hwnd  = get_hwnd();

    if is_valid(hwnd) {
        unsafe {
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, WIN_WIDTH, win_h, SWP_NOACTIVATE);
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    } else {
        unsafe {
            ensure_class_registered();
            let hmod = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            match CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                PCWSTR(CLASS_NAME_UTF16.as_ptr()),
                PCWSTR::null(),
                WS_POPUP | WS_BORDER,
                x, y, WIN_WIDTH, win_h,
                HWND::default(), HMENU::default(), hmod, None,
            ) {
                Ok(new_hwnd) if is_valid(new_hwnd) => {
                    set_hwnd(new_hwnd);
                    let _ = ShowWindow(new_hwnd, SW_SHOWNOACTIVATE);
                    tracing::debug!("候補ウィンドウ作成 hwnd={:?}", new_hwnd);
                }
                Ok(_) | Err(_) => tracing::warn!("候補ウィンドウ作成失敗"),
            }
        }
    }
}

/// 選択インデックスとページ情報だけ更新して再描画する（位置は変えない）。
pub fn update_selection(page_selected: usize, page_info: &str) {
    TL_CAND.with(|c| {
        let mut d = c.borrow_mut();
        d.selected  = page_selected;
        d.page_info = page_info.to_string();
    });
    let hwnd = get_hwnd();
    if is_valid(hwnd) {
        unsafe { let _ = InvalidateRect(hwnd, None, BOOL(0)); }
    }
}

/// 候補ウィンドウを隠す。ウィンドウ自体は破棄しない（次回 show で再利用）。
pub fn hide() {
    let hwnd = get_hwnd();
    if is_valid(hwnd) {
        unsafe { let _ = ShowWindow(hwnd, SW_HIDE); }
    }
}

/// 候補ウィンドウを破棄する（Deactivate 時に呼ぶ）。
pub fn destroy() {
    let hwnd = get_hwnd();
    if is_valid(hwnd) {
        unsafe { let _ = DestroyWindow(hwnd); }
        set_hwnd(HWND::default());
        tracing::debug!("候補ウィンドウ破棄");
    }
}


// ─── LLM待機タイマー ──────────────────────────────────────────────────────────

const WAITING_TIMER_ID: usize = 0x1234;
const WAITING_POLL_MS:  u32   = 80; // 80ms ごとにポーリング

/// Waiting状態に入った時に呼ぶ。LLM完了を80msごとに監視するタイマーを起動する。
pub fn start_waiting_timer() {
    let hwnd = get_hwnd();
    if is_valid(hwnd) {
        unsafe { SetTimer(hwnd, WAITING_TIMER_ID, WAITING_POLL_MS, None); }
        tracing::debug!("waiting timer started");
    }
}

/// Waiting状態を抜けた時に呼ぶ。タイマーを停止する。
pub fn stop_waiting_timer() {
    let hwnd = get_hwnd();
    if is_valid(hwnd) {
        unsafe { let _ = KillTimer(hwnd, WAITING_TIMER_ID); }
        tracing::debug!("waiting timer stopped");
    }
}

/// WM_TIMER コールバック（TSFスレッド上で呼ばれる）。
/// bg_status == "done" になったら候補を取り出して表示する。
pub fn on_waiting_timer() {
    use crate::engine::state::{session_get, SessionState};
    use crate::engine::state::engine_get;

    // セッションが Waiting 状態かチェック
    let wait_info = {
        match session_get() {
            Ok(sess) => {
                if let SessionState::Waiting { text, pos_x, pos_y } = &*sess {
                    Some((text.clone(), *pos_x, *pos_y))
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    };

    let (wait_preedit, pos_x, pos_y) = match wait_info {
        Some(v) => v,
        None => {
            // Waiting ではなくなっていたらタイマー停止
            stop_waiting_timer();
            return;
        }
    };

    // engine の bg_status を確認
    let bg_done = {
        match engine_get() {
            Ok(g) => g.as_ref().map(|e| e.bg_status() == "done").unwrap_or(false),
            Err(_) => false,
        }
    };

    if !bg_done {
        return; // まだ実行中 → 次の WM_TIMER を待つ
    }

    // bg=done → 候補を取り出して表示
    stop_waiting_timer();

    const DICT_LIMIT: usize = 20;
    let _llm_limit = crate::engine::state::get_num_candidates();

    let result = (|| -> Option<(Vec<String>, String)> {
        let mut guard = engine_get().ok()?;
        let engine = guard.as_mut()?;

        // bg_start は hiragana_buf をキーとして使う。
        // wait_preedit は preedit_display()（pending_romaji 含む）なので不一致の場合がある。
        // hiragana_text() でフォールバックして両方試す。
        let hira_key = engine.hiragana_text();
        let llm_cands = engine.bg_take_candidates(&wait_preedit)
            .or_else(|| {
                if hira_key != wait_preedit {
                    tracing::debug!("on_waiting_timer: key mismatch, retry hira={:?}", hira_key);
                    engine.bg_take_candidates(&hira_key)
                } else {
                    None
                }
            });

        let llm_cands = match llm_cands {
            Some(c) => c,
            None => {
                // キー不一致 → bg_reclaim して bg_start で正しいキーで再起動
                tracing::warn!("on_waiting_timer: key mismatch preedit={:?} hira={:?}, reclaim+restart", wait_preedit, hira_key);
                engine.bg_reclaim();
                let llm_limit2 = crate::engine::state::get_num_candidates();
                if engine.bg_start(llm_limit2) {
                    tracing::info!("on_waiting_timer: bg_start restarted → re-arm timer");
                    // タイマーを再起動して次のポーリングで取得
                    start_waiting_timer();
                } else {
                    tracing::error!("on_waiting_timer: bg_start failed");
                }
                return None;
            }
        };

        let merged = if llm_cands.is_empty() {
            engine.merge_candidates(vec![], DICT_LIMIT)
        } else {
            engine.merge_candidates(llm_cands, DICT_LIMIT)
        };
        if merged.is_empty() {
            return None;
        }
        let first = merged.first().cloned().unwrap_or_else(|| wait_preedit.clone());
        Some((merged, first))
    })();

    let (merged, _first) = match result {
        Some(v) => v,
        None => {
            tracing::warn!("on_waiting_timer: bg_take_candidates returned None or empty");
            return;
        }
    };

    // セッションを Selecting に遷移
    let page_info_str;
    let page_cands;
    {
        let mut sess = match session_get() { Ok(s) => s, Err(_) => return };
        sess.activate_selecting(merged, wait_preedit.clone(), pos_x, pos_y, false);
        page_cands = sess.page_candidates().to_vec();
        page_info_str = sess.page_info().to_string();
    }

    show_with_status(&page_cands, 0, &page_info_str, pos_x, pos_y, None);

    // composition text を更新するには TSF API が必要だが、ここは WndProc コンテキスト
    // → composition 更新は次のキー入力時にポーリングが拾う（既存の poll ブランチ）
    // ここでは候補ウィンドウだけ更新して、ユーザーに候補が来たことを見せる
    tracing::info!("on_waiting_timer: showed {} cands for {:?}", page_cands.len(), wait_preedit);
}
