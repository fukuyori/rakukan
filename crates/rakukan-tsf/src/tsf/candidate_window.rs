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
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering as AO};

use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, EndPaint, FillRect,
            GetMonitorInfoW, InvalidateRect, MonitorFromPoint, SelectObject, SetBkMode,
            SetTextColor, TextOutW, BACKGROUND_MODE, HDC, MONITORINFO, MONITOR_DEFAULTTONEAREST,
            PAINTSTRUCT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            TextServices::ITfThreadMgr,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, KillTimer, PostMessageW,
                RegisterClassW, SetTimer, SetWindowPos, ShowWindow, HMENU, HWND_TOPMOST,
                SWP_NOACTIVATE, SW_HIDE, SW_SHOWNOACTIVATE, WM_APP, WM_ERASEBKGND, WM_PAINT,
                WM_TIMER, WNDCLASSW, WS_BORDER, WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_POPUP,
            },
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
/// キャレット高さの推定値（画面端反転時に使用）
const CARET_HEIGHT_ESTIMATE: i32 = 24;

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

    // ─── [Live] ライブ変換用: 最後の ITfContext と client_id ───────────────────────
    // on_input から保存し、on_live_timer から RequestEditSession を呼ぶために使う。
    // thread_local なので Send 不要（常に同一 TSF スレッドからアクセス）。
    static TL_LIVE_CTX: RefCell<Option<windows::Win32::UI::TextServices::ITfContext>>
        = RefCell::new(None);
    static TL_LIVE_TID: Cell<u32> = Cell::new(0);
    /// `TL_LIVE_CTX` を取得した時点の DocumentMgr ptr。
    /// Explorer などで DM が再生成されたら stale 判定に使う。
    static TL_LIVE_DM_PTR: Cell<usize> = Cell::new(0);

    // ─── [FocusDefer] OnSetFocus 非同期化用 ─────────────────────────────────────
    // msctf._NotifyCallbacks 内で COM を触ると INVALID_POINTER_READ を誘発する
    // ケースがあるため、OnSetFocus はキューに積んで即 return し、WM_APP_FOCUS_CHANGED
    // で遅延処理する。
    static TL_PENDING_FOCUS: RefCell<VecDeque<FocusChange>> = RefCell::new(VecDeque::new());
    /// Activate 時にキャッシュする ITfThreadMgr（set_open_close で使用）。
    static TL_THREAD_MGR: RefCell<Option<ITfThreadMgr>> = RefCell::new(None);
    /// Activate 時にキャッシュする TSF client_id。
    static TL_CLIENT_ID: Cell<u32> = Cell::new(0);
}

/// OnSetFocus から遅延処理キューへ積むフォーカス変化イベント。
#[derive(Clone, Copy)]
struct FocusChange {
    prev_ptr: usize,
    next_ptr: usize,
    hwnd_val: usize,
}

/// カスタムメッセージ: OnSetFocus を msctf コールバック外で実行する。
const WM_APP_FOCUS_CHANGED: u32 = WM_APP + 1;

#[derive(Default, Clone)]
struct CandData {
    /// 現在ページの候補（最大 page_size 件）
    candidates: Vec<String>,
    /// ページ内選択インデックス
    selected: usize,
    /// ページ表示文字列（例: "2/4"、1ページのみなら空）
    page_info: String,
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
    b'R' as u16,
    b'a' as u16,
    b'k' as u16,
    b'u' as u16,
    b'n' as u16,
    b'C' as u16,
    b'a' as u16,
    b'n' as u16,
    b'd' as u16,
    0u16,
];

unsafe fn ensure_class_registered() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let hmod = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hmod.into(),
            lpszClassName: PCWSTR(CLASS_NAME_UTF16.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);
        tracing::debug!("candwin::class: registered");
    });
}

// ─── WNDPROC ──────────────────────────────────────────────────────────────────

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
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
            // [Live] ライブ変換タイマー
            if wparam.0 == LIVE_TIMER_ID {
                crate::tsf::candidate_window::on_live_timer();
            }
            LRESULT(0)
        }
        // OnSetFocus 遅延処理: msctf コールバックから抜けた後にここで処理する
        m if m == WM_APP_FOCUS_CHANGED => {
            handle_pending_focus_changes();
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
    let n = data.candidates.len();
    let has_pager = !data.page_info.is_empty();
    let has_status = data.status_line.is_some();
    let win_h = window_height(n, has_pager, has_status);

    // 背景を白で塗りつぶし
    let bg_brush = CreateSolidBrush(COLOR_BG);
    let full = RECT {
        left: 0,
        top: 0,
        right: WIN_WIDTH,
        bottom: win_h,
    };
    FillRect(hdc, &full, bg_brush);
    let _ = DeleteObject(bg_brush);

    // フォント
    let face: Vec<u16> = "Meiryo UI\0".encode_utf16().collect();
    let font = CreateFontW(
        FONT_HEIGHT,
        0,
        0,
        0,
        400, // FW_NORMAL
        0,
        0,
        0,
        1, // DEFAULT_CHARSET
        0,
        0,
        0,
        0,
        PCWSTR(face.as_ptr()),
    );
    let old_obj = SelectObject(hdc, font);
    SetBkMode(hdc, BACKGROUND_MODE(1)); // TRANSPARENT

    let sel_brush = CreateSolidBrush(COLOR_SEL_BG);
    let wht_brush = CreateSolidBrush(COLOR_BG);
    let pager_brush = CreateSolidBrush(COLORREF(0x00_F0_F0_F0));
    let status_brush = CreateSolidBrush(COLORREF(0x00_F8_F8_F8));

    // ステータス行（先頭・グレー背景・番号なし・選択不可）
    let status_offset = if has_status {
        if let Some(ref s) = data.status_line {
            let row = RECT {
                left: 0,
                top: PADDING_Y,
                right: WIN_WIDTH,
                bottom: PADDING_Y + STATUS_HEIGHT,
            };
            FillRect(hdc, &row, status_brush);
            SetTextColor(hdc, COLORREF(0x00_88_88_88));
            let text_w: Vec<u16> = s.encode_utf16().collect();
            let _ = TextOutW(
                hdc,
                PADDING_X,
                PADDING_Y + (STATUS_HEIGHT - FONT_HEIGHT) / 2,
                &text_w,
            );
        }
        STATUS_HEIGHT
    } else {
        0
    };

    // 候補行
    for (i, cand) in data.candidates.iter().enumerate() {
        let y = PADDING_Y + status_offset + i as i32 * ITEM_HEIGHT;
        let row = RECT {
            left: 0,
            top: y,
            right: WIN_WIDTH,
            bottom: y + ITEM_HEIGHT,
        };
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
        let row = RECT {
            left: 0,
            top: y,
            right: WIN_WIDTH,
            bottom: y + PAGER_HEIGHT,
        };
        FillRect(hdc, &row, pager_brush);
        let _ = windows::Win32::Graphics::Gdi::MoveToEx(hdc, 0, y, None);
        let _ = windows::Win32::Graphics::Gdi::LineTo(hdc, WIN_WIDTH, y);
        SetTextColor(hdc, COLORREF(0x00_55_55_55));
        let pager_text = format!("◀  {}  ▶", data.page_info);
        let pager_w: Vec<u16> = pager_text.encode_utf16().collect();
        let _ = TextOutW(
            hdc,
            PADDING_X,
            y + (PAGER_HEIGHT - FONT_HEIGHT) / 2,
            &pager_w,
        );
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
    if has_status {
        with_pager + STATUS_HEIGHT
    } else {
        with_pager
    }
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

pub fn show_with_status(
    page_candidates: &[String],
    page_selected: usize,
    page_info: &str,
    x: i32,
    y: i32,
    status_line: Option<&str>,
) {
    if page_candidates.is_empty() {
        hide();
        return;
    }

    let has_pager = !page_info.is_empty();
    let has_status = status_line.is_some();

    TL_CAND.with(|c| {
        let mut d = c.borrow_mut();
        if d.candidates.as_slice() != page_candidates {
            d.candidates = page_candidates.to_vec();
        }
        d.selected = page_selected;
        d.page_info = page_info.to_string();
        d.status_line = status_line.map(|s| s.to_string());
    });

    let n = page_candidates.len();
    let win_h = window_height(n, has_pager, has_status);

    // ─── 画面端検出：ウィンドウが画面外にはみ出す場合はキャレットの上側に反転 ───
    let win_y = unsafe { calc_window_y(x, y, win_h) };

    let hwnd = get_hwnd();

    if is_valid(hwnd) {
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                x,
                win_y,
                WIN_WIDTH,
                win_h,
                SWP_NOACTIVATE,
            );
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
                x,
                win_y,
                WIN_WIDTH,
                win_h,
                HWND::default(),
                HMENU::default(),
                hmod,
                None,
            ) {
                Ok(new_hwnd) if is_valid(new_hwnd) => {
                    set_hwnd(new_hwnd);
                    let _ = ShowWindow(new_hwnd, SW_SHOWNOACTIVATE);
                    tracing::debug!("candwin::create: hwnd={:?}", new_hwnd);
                }
                Ok(_) | Err(_) => tracing::warn!("candwin::create: failed"),
            }
        }
    }
}

/// キャレット下端 `caret_bottom` から候補ウィンドウの表示 Y を計算する。
///
/// ウィンドウが作業領域（タスクバー除く）の下端を超える場合は
/// キャレットの上側（`caret_bottom - CARET_HEIGHT_ESTIMATE - win_h`）に反転する。
unsafe fn calc_window_y(x: i32, caret_bottom: i32, win_h: i32) -> i32 {
    let pt = POINT { x, y: caret_bottom };
    let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut mi).as_bool() {
        let work_bottom = mi.rcWork.bottom;
        if caret_bottom + win_h > work_bottom {
            // 上側に反転：4ドット上
            let flipped = caret_bottom - CARET_HEIGHT_ESTIMATE - win_h - 4;
            tracing::debug!(
                "candwin::flip: caret_bottom={} win_h={} work_bottom={} → y={}",
                caret_bottom,
                win_h,
                work_bottom,
                flipped
            );
            return flipped.max(mi.rcWork.top);
        }
    }
    // 下側：1ドット下
    caret_bottom + 1
}

/// 選択インデックスとページ情報だけ更新して再描画する（位置は変えない）。
pub fn update_selection(page_selected: usize, page_info: &str) {
    TL_CAND.with(|c| {
        let mut d = c.borrow_mut();
        d.selected = page_selected;
        d.page_info = page_info.to_string();
    });
    let hwnd = get_hwnd();
    if is_valid(hwnd) {
        unsafe {
            let _ = InvalidateRect(hwnd, None, BOOL(0));
        }
    }
}

/// 候補ウィンドウを隠す。ウィンドウ自体は破棄しない（次回 show で再利用）。
pub fn hide() {
    let hwnd = get_hwnd();
    if is_valid(hwnd) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// 候補ウィンドウを破棄する（Deactivate 時に呼ぶ）。
pub fn destroy() {
    let hwnd = get_hwnd();
    if is_valid(hwnd) {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        set_hwnd(HWND::default());
        tracing::debug!("candwin::destroy");
    }
}

// ─── HWND 確保ヘルパー ────────────────────────────────────────────────────────

/// 候補ウィンドウ HWND を確保する（未作成なら 1x1 の非表示ウィンドウを作成）。
///
/// PostMessage / SetTimer のターゲット HWND が必要な全ての経路から呼ばれる。
fn ensure_hwnd() -> HWND {
    let h = get_hwnd();
    if is_valid(h) {
        return h;
    }
    unsafe {
        ensure_class_registered();
        let hmod = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            PCWSTR(CLASS_NAME_UTF16.as_ptr()),
            PCWSTR::null(),
            WS_POPUP | WS_BORDER,
            0,
            0,
            1,
            1,
            HWND::default(),
            HMENU::default(),
            hmod,
            None,
        ) {
            Ok(new_hwnd) if is_valid(new_hwnd) => {
                set_hwnd(new_hwnd);
                tracing::debug!("candwin::ensure_hwnd: created hwnd={:?}", new_hwnd);
                new_hwnd
            }
            _ => {
                tracing::warn!("candwin::ensure_hwnd: CreateWindowExW failed");
                HWND::default()
            }
        }
    }
}

// ─── OnSetFocus 遅延処理 ──────────────────────────────────────────────────────

/// Activate 時に ITfThreadMgr と client_id をキャッシュする。
///
/// WM_APP_FOCUS_CHANGED ハンドラの中で set_open_close を呼ぶために必要。
pub fn cache_thread_mgr(tm: ITfThreadMgr, tid: u32) {
    TL_THREAD_MGR.with(|c| *c.borrow_mut() = Some(tm));
    TL_CLIENT_ID.with(|c| c.set(tid));
}

/// Deactivate 時にキャッシュをクリアする。
pub fn clear_thread_mgr() {
    TL_THREAD_MGR.with(|c| *c.borrow_mut() = None);
    TL_CLIENT_ID.with(|c| c.set(0));
    TL_PENDING_FOCUS.with(|q| q.borrow_mut().clear());
}

fn current_focus_dm_ptr() -> Option<usize> {
    use windows::core::Interface;

    TL_THREAD_MGR.with(|c| {
        c.borrow()
            .as_ref()
            .and_then(|tm| unsafe { tm.GetFocus().ok() })
            .map(|dm| dm.as_raw() as usize)
    })
}

/// `OnUninitDocumentMgr` から呼ばれる。
///
/// msctf コールバック中に COM 参照を drop しないよう、ここでは stale 印を付けるだけに留める。
/// 実際の fallback / timer 停止は `on_live_timer` 側で安全な文脈で実施する。
pub fn invalidate_live_context_for_dm(dm_ptr: usize) {
    if dm_ptr == 0 {
        return;
    }

    let matched = TL_LIVE_DM_PTR.with(|c| {
        if c.get() == dm_ptr {
            c.set(0);
            true
        } else {
            false
        }
    });

    if matched {
        tracing::debug!("[Live] invalidate_live_context_for_dm: marked stale dm={dm_ptr:#x}");
    }
}

/// OnSetFocus から呼ばれる。イベントをキューに積み、WM_APP_FOCUS_CHANGED を
/// PostMessage して即 return する（msctf._NotifyCallbacks からの再入を避ける）。
pub fn post_focus_changed(prev_ptr: usize, next_ptr: usize, hwnd_val: usize) {
    TL_PENDING_FOCUS.with(|q| {
        q.borrow_mut().push_back(FocusChange {
            prev_ptr,
            next_ptr,
            hwnd_val,
        });
    });
    let hwnd = ensure_hwnd();
    if is_valid(hwnd) {
        unsafe {
            let _ = PostMessageW(hwnd, WM_APP_FOCUS_CHANGED, WPARAM(0), LPARAM(0));
        }
    } else {
        // HWND 作成失敗時はキューから落として捨てる（次回の焦点遷移でまた積まれる）
        TL_PENDING_FOCUS.with(|q| {
            q.borrow_mut().pop_back();
        });
        tracing::warn!("post_focus_changed: no hwnd, event dropped");
    }
}

/// WM_APP_FOCUS_CHANGED ハンドラ。キューに溜まった全イベントを順次処理する。
fn handle_pending_focus_changes() {
    loop {
        let fc_opt = TL_PENDING_FOCUS.with(|q| q.borrow_mut().pop_front());
        let Some(fc) = fc_opt else {
            break;
        };
        process_focus_change(fc);
    }
}

/// 実際のフォーカス変化処理（旧 OnSetFocus の本体）。
///
/// msctf._NotifyCallbacks コールバックの外で実行されるため、
/// COM 再入 (set_open_close) や ITfContext の Drop (stop_live_timer) が安全。
fn process_focus_change(fc: FocusChange) {
    use crate::engine::input_mode::InputMode;

    tracing::debug!(
        "OnSetFocus(deferred): prev_dm={:#x} next_dm={:#x} hwnd={:#x}",
        fc.prev_ptr,
        fc.next_ptr,
        fc.hwnd_val
    );

    // 別コンテキストへの移動 → 候補ウィンドウを閉じる + ライブタイマー停止
    hide();
    stop_live_timer();

    // フォーカス先が null の場合は prev_dm のモード保存のみ
    if fc.next_ptr == 0 {
        let _ = crate::engine::state::doc_mode_on_focus_change(fc.prev_ptr, 0, fc.hwnd_val);
        return;
    }

    let Some(new_mode) =
        crate::engine::state::doc_mode_on_focus_change(fc.prev_ptr, fc.next_ptr, fc.hwnd_val)
    else {
        return;
    };

    // モードを適用
    if let Ok(mut st) = crate::engine::state::ime_state_get() {
        if st.input_mode != new_mode {
            tracing::info!(
                "OnSetFocus(deferred): mode {:?} → {:?}",
                st.input_mode,
                new_mode
            );
            st.set_mode(new_mode);
        }
    }

    // KEYBOARD_OPENCLOSE を更新（ターミナル判定用）。
    // この SetValue は msctf への再入だが、既に _NotifyCallbacks を抜けているので安全。
    let is_open = new_mode != InputMode::Alphanumeric;
    let tm_opt = TL_THREAD_MGR.with(|c| c.borrow().clone());
    if let Some(tm) = tm_opt {
        let tid = TL_CLIENT_ID.with(|c| c.get());
        unsafe {
            let _ = crate::tsf::language_bar::set_open_close(&tm, tid, is_open);
        }
    }

    // トレイアイコン更新
    crate::tsf::tray_ipc::publish(is_open, new_mode);
}

// ─── LLM待機タイマー ──────────────────────────────────────────────────────────

const WAITING_TIMER_ID: usize = 0x1234;
const WAITING_POLL_MS: u32 = 80; // 80ms ごとにポーリング

// ─── [Live] ライブ変換タイマー ────────────────────────────────────────────────────
//
// 目的: WM_TIMER コールバックから RequestEditSession を直接呼べるか検証する。
// 呼べれば Phase 1A (Direct方式) へ進む。
// 呼べなければ（E_FAIL / deadlock）Phase 1B (Queue方式) へ進む。
//
// 起動条件: on_input が呼ばれるたびにデバウンス時刻をリセットし、タイマーを起動する。
// 発火条件: LIVE_DEBOUNCE_MS 経過後に bg_status=="done" を確認し、
//           RequestEditSession でプレビューを composition に書き込む。

const LIVE_TIMER_ID: usize = 0x1235;
const LIVE_POLL_MS: u32 = 50; // 50ms ごとにポーリング

/// タイマーが初回発火したか（bg_not_done ログの重複抑制用）。
/// 入力サイクルごとに reset される。
static LIVE_TIMER_FIRED_ONCE_STATIC: AtomicBool = AtomicBool::new(false);

/// 最後のキー入力時刻（Unix ms）。デバウンス用。
static LIVE_LAST_INPUT_MS: AtomicU64 = AtomicU64::new(0);

/// config.live_conversion.debounce_ms の実行時コピー（live_input_notify がセット）。
static LIVE_DEBOUNCE_CFG_MS: AtomicU64 = AtomicU64::new(80);

/// Waiting状態に入った時に呼ぶ。LLM完了を80msごとに監視するタイマーを起動する。
pub fn start_waiting_timer() {
    let hwnd = get_hwnd();
    if is_valid(hwnd) {
        unsafe {
            SetTimer(hwnd, WAITING_TIMER_ID, WAITING_POLL_MS, None);
        }
        tracing::debug!("waiting timer started");
    }
}

/// Waiting状態を抜けた時に呼ぶ。タイマーを停止する。
pub fn stop_waiting_timer() {
    let hwnd = get_hwnd();
    if is_valid(hwnd) {
        unsafe {
            let _ = KillTimer(hwnd, WAITING_TIMER_ID);
        }
        tracing::debug!("waiting timer stopped");
    }
}

/// WM_TIMER コールバック（TSFスレッド上で呼ばれる）。
/// bg_status == "done" になったら候補を取り出して表示する。
pub fn on_waiting_timer() {
    use crate::engine::state::engine_get;
    use crate::engine::state::{session_get, SessionState};

    // セッションが Waiting 状態かチェック
    let wait_info = {
        match session_get() {
            Ok(sess) => {
                if let SessionState::Waiting {
                    text,
                    pos_x,
                    pos_y,
                    remainder,
                    remainder_reading,
                } = &*sess
                {
                    Some((
                        text.clone(),
                        *pos_x,
                        *pos_y,
                        remainder.clone(),
                        remainder_reading.clone(),
                    ))
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    };

    let (wait_preedit, pos_x, pos_y, remainder, remainder_reading) = match wait_info {
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

    const DICT_LIMIT: usize = 40;
    let _llm_limit = crate::engine::state::get_num_candidates();

    let result = (|| -> Option<(Vec<String>, String)> {
        let mut guard = engine_get().ok()?;
        let engine = guard.as_mut()?;

        // bg_start は hiragana_buf をキーとして使う。
        // wait_preedit は preedit_display()（pending_romaji 含む）なので不一致の場合がある。
        // hiragana_text() でフォールバックして両方試す。
        let hira_key = engine.hiragana_text();
        let llm_cands = engine.bg_take_candidates(&wait_preedit).or_else(|| {
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
                tracing::warn!(
                    "on_waiting_timer: key mismatch preedit={:?} hira={:?}, reclaim+restart",
                    wait_preedit,
                    hira_key
                );
                engine.bg_reclaim();
                let llm_limit2 = crate::engine::state::get_num_candidates();
                if engine.bg_start(llm_limit2) {
                    tracing::debug!("on_waiting_timer: bg_start restarted → re-arm timer");
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
        let first = merged
            .first()
            .cloned()
            .unwrap_or_else(|| wait_preedit.clone());
        Some((merged, first))
    })();

    let (merged, _first) = match result {
        Some(v) => v,
        None => {
            tracing::warn!("on_waiting_timer: bg_take_candidates returned None or empty");
            return;
        }
    };

    // セッションを Selecting に遷移。範囲指定変換経由で remainder があれば引き継ぐ。
    let page_info_str;
    let page_cands;
    {
        let mut sess = match session_get() {
            Ok(s) => s,
            Err(_) => return,
        };
        sess.activate_selecting_with_affixes(
            merged,
            wait_preedit.clone(),
            pos_x,
            pos_y,
            false,
            String::new(),
            String::new(),
            remainder,
            remainder_reading,
        );
        page_cands = sess.page_candidates().to_vec();
        page_info_str = sess.page_info().to_string();
    }

    show_with_status(&page_cands, 0, &page_info_str, pos_x, pos_y, None);

    // composition text を更新するには TSF API が必要だが、ここは WndProc コンテキスト
    // → composition 更新は次のキー入力時にポーリングが拾う（既存の poll ブランチ）
    // ここでは候補ウィンドウだけ更新して、ユーザーに候補が来たことを見せる
    tracing::debug!(
        "on_waiting_timer: showed {} cands for {:?}",
        page_cands.len(),
        wait_preedit
    );
}

// ─── [Live] ライブ変換実装 ─────────────────────────────────────────────────────────

/// on_input から呼ぶ。最終入力時刻を記録し、ライブタイマーを起動する。
///
/// `config.live_conversion.enabled = false` の場合は何もしない。
/// ライブタイマーが既に動いている場合でも SetTimer は内部でリセットされる（同一 ID なら上書き）。
pub fn live_input_notify(ctx: &windows::Win32::UI::TextServices::ITfContext, tid: u32) {
    use windows::core::Interface;

    // ── config.live_conversion.enabled チェック ─────────────────────────────
    let cfg = crate::engine::config::current_config();
    if !cfg.live_conversion.enabled {
        // enabled=false のときは何もしない（タイマーも起動しない）
        return;
    }
    let debounce_ms = cfg.live_conversion.debounce_ms;

    // デバウンス時刻をリセット
    let now = current_millis();
    LIVE_LAST_INPUT_MS.store(now, AO::Relaxed);
    // config の debounce_ms をスレッドローカルにキャッシュ（タイマー側で参照）
    LIVE_DEBOUNCE_CFG_MS.store(debounce_ms, AO::Relaxed);
    // 新規入力サイクル開始 → 初回発火フラグをリセット
    LIVE_TIMER_FIRED_ONCE_STATIC.store(false, AO::Relaxed);

    // ITfContext を thread_local にキャッシュ（on_live_timer の Phase1A で使用）
    TL_LIVE_CTX.with(|c| {
        *c.borrow_mut() = Some(ctx.clone());
    });
    TL_LIVE_TID.with(|c| c.set(tid));
    let live_dm_ptr = unsafe { ctx.GetDocumentMgr().ok() }
        .map(|dm| dm.as_raw() as usize)
        .unwrap_or(0);
    TL_LIVE_DM_PTR.with(|c| c.set(live_dm_ptr));
    if live_dm_ptr == 0 {
        tracing::debug!("[Live] live_input_notify: no document manager, Phase1A disabled");
    }

    // HWND が未作成の場合は非表示ウィンドウを先行作成してタイマー用HWNDを確保する
    let hwnd = {
        let h = get_hwnd();
        if is_valid(h) {
            h
        } else {
            unsafe {
                ensure_class_registered();
                let hmod = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
                match CreateWindowExW(
                    WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                    PCWSTR(CLASS_NAME_UTF16.as_ptr()),
                    PCWSTR::null(),
                    WS_POPUP | WS_BORDER,
                    0,
                    0,
                    1,
                    1,
                    HWND::default(),
                    HMENU::default(),
                    hmod,
                    None,
                ) {
                    Ok(new_hwnd) if is_valid(new_hwnd) => {
                        set_hwnd(new_hwnd);
                        tracing::info!(
                            "[Live] live_input_notify: pre-created hidden hwnd={:?}",
                            new_hwnd
                        );
                        new_hwnd
                    }
                    _ => {
                        tracing::warn!("[Live] live_input_notify: hwnd creation failed");
                        return;
                    }
                }
            }
        }
    };

    // ライブタイマーを起動（既存なら上書き）
    unsafe {
        SetTimer(hwnd, LIVE_TIMER_ID, LIVE_POLL_MS, None);
    }
    tracing::info!(
        "[Live] live_input_notify: timer armed debounce={}ms",
        debounce_ms
    );
}

/// ライブタイマーを明示的に停止する（IMEオフ・確定・キャンセル時）。
pub fn stop_live_timer() {
    let hwnd = get_hwnd();
    if is_valid(hwnd) {
        unsafe {
            let _ = KillTimer(hwnd, LIVE_TIMER_ID);
        }
    }
    TL_LIVE_CTX.with(|c| {
        *c.borrow_mut() = None;
    });
    TL_LIVE_TID.with(|c| c.set(0));
    TL_LIVE_DM_PTR.with(|c| c.set(0));
    tracing::debug!("[Live] stop_live_timer");
}

/// 現在時刻をミリ秒で返す（デバウンス用）。
fn current_millis() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// WM_TIMER(LIVE_TIMER_ID) コールバック。
///
/// # 動作方針
/// - Phase 1A 試行: WM_TIMER から RequestEditSession を直接呼ぶ（成功すれば即時更新）
/// - Phase 1B fallback: 失敗時は LIVE_PREVIEW_QUEUE に書き込み → 次キー入力で反映
pub fn on_live_timer() {
    use crate::engine::state::{
        composition_clone, engine_try_get, LIVE_PREVIEW_QUEUE, LIVE_PREVIEW_READY,
    };
    use crate::tsf::edit_session::EditSession;
    use windows::Win32::UI::TextServices::TF_ES_READWRITE;

    // ── [診断] タイマー発火確認（条件チェック前に必ず出力）──────────────────
    let debounce_ms = LIVE_DEBOUNCE_CFG_MS.load(AO::Relaxed);
    let now = current_millis();
    let last = LIVE_LAST_INPUT_MS.load(AO::Relaxed);
    let elapsed = now.saturating_sub(last);
    if elapsed < debounce_ms {
        return; // まだデバウンス中（ログなし）
    }

    // ── エンジンのプリエディットバッファ確認 ─────────────────────────────────
    // 通常入力中のセッション状態は Idle のまま（SessionState::Preedit へ遷移するのは
    // Selecting/Waiting から戻るときのみ）。エンジンバッファで直接判定する。
    //
    // engine_try_get() が Err を返すのは一時的なロック競合（Space 変換などが保持中）。
    // このときに stop_live_timer() するとタイマーが死んで次の on_input まで復活しない
    // ため、busy のときは何もせず次回発火を待つ。
    let probe = {
        match engine_try_get() {
            Ok(g) => g.as_ref().map(|e| {
                let h = e.hiragana_text().to_string();
                let bg = e.bg_status();
                (h, bg)
            }),
            Err(_) => {
                tracing::trace!("[Live] on_live_timer: engine busy, retry next tick");
                return;
            }
        }
    };
    let (hiragana, bg_status_str) = match probe {
        Some(v) => v,
        None => {
            stop_live_timer();
            return;
        }
    };
    let has_preedit = !hiragana.is_empty();
    tracing::info!(
        "[Live] on_live_timer: FIRED elapsed={}ms has_preedit={} hira={:?} bg={}",
        elapsed,
        has_preedit,
        hiragana,
        bg_status_str
    );

    if !has_preedit {
        stop_live_timer();
        return;
    }
    let bg_done = bg_status_str == "done";
    if !bg_done {
        if bg_status_str == "idle" {
            // bg=idle かつ preedit あり → ライブタイマーから bg_start を自己起動
            // poll_*_ready() を呼ばないと is_kanji_ready() が false のままになる
            let started = match engine_try_get() {
                Ok(mut g) => g
                    .as_mut()
                    .map(|e| {
                        let _ = crate::engine::state::poll_dict_ready_cached(e);
                        let _ = crate::engine::state::poll_model_ready_cached(e);
                        let kanji_ready = e.is_kanji_ready();
                        let dict_ready = e.is_dict_ready();
                        tracing::info!(
                            "[Live] on_live_timer: kanji_ready={} dict_ready={}",
                            kanji_ready,
                            dict_ready
                        );
                        if !kanji_ready {
                            // モデル未ロード → タイマーを止めてロック競合を防ぐ
                            // モデルロード完了後、次の on_input で live_input_notify が再起動する
                            return false;
                        }
                        e.bg_start(crate::engine::state::get_live_conv_beam_size())
                    })
                    .unwrap_or(false),
                Err(_) => false,
            };
            tracing::info!("[Live] on_live_timer: bg=idle → bg_start={}", started);
            if !started {
                stop_live_timer(); // kanji_ready=false の間はタイマーを止める
                return;
            }
        } else {
            // bg=running: まだ変換中
            if !LIVE_TIMER_FIRED_ONCE_STATIC.swap(true, AO::Relaxed) {
                tracing::info!("[Live] on_live_timer: waiting bg={}", bg_status_str);
            }
        }
        return;
    }
    LIVE_TIMER_FIRED_ONCE_STATIC.store(false, AO::Relaxed);

    // ── トップ候補を取得 ────────────────────────────────────────────────────
    let (reading, pending, preview) = {
        let Ok(mut g) = engine_try_get() else {
            tracing::warn!("[Live] on_live_timer: engine busy");
            return;
        };
        let Some(eng) = g.as_mut() else { return };
        let reading = eng.hiragana_text().to_string();
        if reading.is_empty() {
            return;
        }
        let preedit_full = eng.preedit_display();
        let pending =
            crate::engine::text_util::suffix_after_prefix_or_empty(
                &preedit_full,
                &reading,
                "on_live_timer pending",
            )
            .to_string();
        let preview = eng
            .bg_take_candidates(&reading)
            .and_then(|c| c.into_iter().next());
        (reading, pending, preview)
    };

    let Some(preview) = preview else {
        tracing::debug!("[Live] on_live_timer: no candidates for {:?}", reading);
        return;
    };

    let display_shown = if pending.is_empty() {
        preview.clone()
    } else {
        format!("{preview}{pending}")
    };

    tracing::info!(
        "[Live] on_live_timer: reading={:?} preview={:?}",
        reading,
        preview
    );

    // ── Phase 1A: RequestEditSession で直接 composition を更新 ───────────
    let ctx_opt = TL_LIVE_CTX.with(|c| c.borrow().clone());
    let tid = TL_LIVE_TID.with(|c| c.get());
    let live_dm_ptr = TL_LIVE_DM_PTR.with(|c| c.get());
    let focused_dm_ptr = current_focus_dm_ptr();
    let dm_matches_focus = live_dm_ptr != 0 && focused_dm_ptr == Some(live_dm_ptr);

    let phase1a_possible = ctx_opt.is_some()
        && tid > 0
        && composition_clone().map(|g| g.is_some()).unwrap_or(false)
        && dm_matches_focus;

    if ctx_opt.is_some() && tid > 0 && !dm_matches_focus {
        tracing::debug!(
            "[Live] Phase1A skipped: stale or unfocused dm cached={live_dm_ptr:#x} focus={:?}",
            focused_dm_ptr
        );
    }

    if let Some(ctx) = ctx_opt.filter(|_| phase1a_possible) {
        let ctx_req = ctx.clone();
        let preview_1a = display_shown.clone();
        let captured_dm_ptr = live_dm_ptr;

        let session = EditSession::new(move |ec| unsafe {
            use windows::Win32::Foundation::E_FAIL;
            use windows::Win32::UI::TextServices::{
                TfActiveSelEnd, TF_ANCHOR_END, TF_SELECTION, TF_SELECTIONSTYLE,
            };
            let now_focus = current_focus_dm_ptr();
            if now_focus != Some(captured_dm_ptr) {
                return Err(windows::core::Error::new(
                    E_FAIL,
                    format!(
                        "focus DM changed between request and callback execution: expected={captured_dm_ptr:#x} actual={now_focus:?}"
                    ),
                ));
            }
            let comp = crate::engine::state::composition_clone()
                .unwrap_or(None)
                .ok_or_else(|| windows::core::Error::new(E_FAIL, "no composition"))?;
            let range = comp
                .GetRange()
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("GetRange: {e}")))?;
            let text_w: Vec<u16> = preview_1a.encode_utf16().collect();
            range
                .SetText(ec, 0, &text_w)
                .map_err(|e| windows::core::Error::new(E_FAIL, format!("SetText: {e}")))?;

            let atom = crate::tsf::display_attr::atom_input();
            if atom != 0 {
                if let Ok(prop) =
                    ctx.GetProperty(&windows::Win32::UI::TextServices::GUID_PROP_ATTRIBUTE)
                {
                    let _ = prop.Clear(ec, &range);
                    let var = windows_core::VARIANT::from(atom as i32);
                    let _ = prop.SetValue(ec, &range, &var);
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
                let _ = ctx.SetSelection(ec, &[sel]);
            }
            Ok(())
        });

        let result = unsafe { ctx_req.RequestEditSession(tid, &session, TF_ES_READWRITE) };

        match result {
            Ok(_) => {
                if let Ok(mut sess) = crate::engine::state::session_get() {
                    sess.set_live_conv(reading.clone(), preview.clone());
                }
                stop_live_timer();
                return;
            }
            Err(_) => {
                // Phase 1A 失敗 → Phase 1B フォールバック
            }
        }
    }

    // ── Phase 1B フォールバック: キューに書き込む ────────────────────────────
    // 次のキー入力時に handle_action の冒頭ポーリングが拾って composition を更新する。
    if let Ok(mut q) = LIVE_PREVIEW_QUEUE.try_lock() {
        *q = Some(preview.clone());
        LIVE_PREVIEW_READY.store(true, AO::Release);
        tracing::debug!(
            "[Live] Phase1B: queued preview={:?} for reading={:?}",
            preview,
            reading
        );
    } else {
        tracing::warn!("[Live] Phase1B: LIVE_PREVIEW_QUEUE busy, skipping");
    }

    // Phase 1B ではタイマーを停止する（キュー上書きを防ぐ）
    stop_live_timer();
}
