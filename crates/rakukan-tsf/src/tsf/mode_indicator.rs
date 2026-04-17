//! モードインジケーター（カーソル近くに一時表示）
//!
//! IME のモード切替時にキャレット付近に「あ」「ア」「A」を短時間表示する。
//! mozc の IndicatorWindow に相当する機能。
//!
//! # 表示仕様
//! - WS_POPUP + WS_EX_TOPMOST + WS_EX_NOACTIVATE（フォーカスを奪わない）
//! - モード文字を 1 文字表示（32x32 程度）
//! - 表示後 1.5 秒でフェードアウト開始、約 0.5 秒で完全に消える
//! - キー入力があれば即非表示

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::{
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BACKGROUND_MODE, BeginPaint, ClientToScreen, CreateFontW, CreateSolidBrush,
            DeleteObject, EndPaint, FillRect, GetMonitorInfoW, HDC, InvalidateRect,
            MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, PAINTSTRUCT, SelectObject,
            SetBkMode, SetTextColor, TextOutW,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GUITHREADINFO, GetGUIThreadInfo, HMENU,
            HWND_TOPMOST, KillTimer, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
            SWP_NOSIZE, SetTimer, SetWindowPos, ShowWindow, WM_ERASEBKGND, WM_PAINT, WM_TIMER,
            WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        },
    },
    core::PCWSTR,
};

// ─── 定数 ────────────────────────────────────────────────────────────────────

const WIN_SIZE: i32 = 32;
const FONT_HEIGHT: i32 = 22;
const CARET_HEIGHT_ESTIMATE: i32 = 24;

/// フェードアウト開始までの待機時間 (ms)
const FADE_START_MS: u32 = 1500;
/// フェードアウト用タイマー間隔 (ms)  ― 未使用 (Layered Window 非使用のため hide で即消去)
const HIDE_TIMER_ID: usize = 100;

const COLOR_BG_LIGHT: COLORREF = COLORREF(0x00_FF_FF_FF);
const COLOR_FG_LIGHT: COLORREF = COLORREF(0x00_55_55_55);
const COLOR_BG_DARK: COLORREF = COLORREF(0x00_33_33_33);
const COLOR_FG_DARK: COLORREF = COLORREF(0x00_FF_FF_FF);

// ─── スレッドローカル状態 ──────────────────────────────────────────────────────

thread_local! {
    static TL_HWND: Cell<isize> = Cell::new(0);
    static TL_TEXT: Cell<&'static str> = Cell::new("あ");
    static TL_LIGHT: Cell<bool> = Cell::new(false);
}

/// 表示中フラグ（キー入力で即非表示にするためアトミック）
static VISIBLE: AtomicBool = AtomicBool::new(false);

// ─── ウィンドウクラス ─────────────────────────────────────────────────────────

static CLASS_NAME_UTF16: &[u16] = &[
    b'R' as u16,
    b'a' as u16,
    b'k' as u16,
    b'u' as u16,
    b'k' as u16,
    b'a' as u16,
    b'n' as u16,
    b'M' as u16,
    b'o' as u16,
    b'd' as u16,
    b'e' as u16,
    0,
];

static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);

unsafe fn ensure_class_registered() {
    if CLASS_REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    let hmod = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hmod.into(),
        lpszClassName: PCWSTR(CLASS_NAME_UTF16.as_ptr()),
        ..Default::default()
    };
    RegisterClassW(&wc);
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
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
        WM_ERASEBKGND => LRESULT(1),
        WM_TIMER => {
            if wparam.0 == HIDE_TIMER_ID {
                hide();
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

// ─── 描画 ─────────────────────────────────────────────────────────────────────

unsafe fn draw(hdc: HDC) {
    let text = TL_TEXT.with(|t| t.get());
    let light = TL_LIGHT.with(|l| l.get());

    let (bg, fg) = if light {
        (COLOR_BG_LIGHT, COLOR_FG_LIGHT)
    } else {
        (COLOR_BG_DARK, COLOR_FG_DARK)
    };

    let bg_brush = CreateSolidBrush(bg);
    let rc = RECT {
        left: 0,
        top: 0,
        right: WIN_SIZE,
        bottom: WIN_SIZE,
    };
    FillRect(hdc, &rc, bg_brush);
    let _ = DeleteObject(bg_brush);

    let face: Vec<u16> = "Yu Gothic UI\0".encode_utf16().collect();
    let font = CreateFontW(
        FONT_HEIGHT,
        0,
        0,
        0,
        700,
        0,
        0,
        0,
        1,
        0,
        0,
        0,
        0,
        PCWSTR(face.as_ptr()),
    );
    let old_font = SelectObject(hdc, font);
    SetBkMode(hdc, BACKGROUND_MODE(1)); // TRANSPARENT
    SetTextColor(hdc, fg);

    let wbuf: Vec<u16> = text.encode_utf16().collect();
    // 中央揃え
    let tx = (WIN_SIZE - FONT_HEIGHT) / 2;
    let ty = (WIN_SIZE - FONT_HEIGHT) / 2;
    let _ = TextOutW(hdc, tx, ty, &wbuf);

    let _ = SelectObject(hdc, old_font);
    let _ = DeleteObject(font);
}

// ─── 公開 API ────────────────────────────────────────────────────────────────

/// OS からキャレット位置を取得する（スクリーン座標）。
/// TSF の GetTextExt で取得できなかった場合の二次フォールバック。
/// 取得できない場合は None を返し、インジケーターは表示しない（mozc 準拠）。
fn get_caret_screen_pos() -> Option<(i32, i32)> {
    unsafe {
        // GetGUIThreadInfo でキャレット位置を取得
        // TSF はアプリの UI スレッド上で動くので、現在のスレッド ID を指定する。
        let tid = windows::Win32::System::Threading::GetCurrentThreadId();
        let mut gti = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        if GetGUIThreadInfo(tid, &mut gti).is_ok() {
            let hwnd_caret = gti.hwndCaret;
            if !hwnd_caret.0.is_null() {
                let mut pt = POINT {
                    x: gti.rcCaret.left,
                    y: gti.rcCaret.bottom,
                };
                if ClientToScreen(hwnd_caret, &mut pt).as_bool() {
                    return Some((pt.x, pt.y));
                }
            }
        }

        // マウスカーソルへのフォールバックは行わない（mozc 準拠）。
        // 位置が特定できない場合はインジケーターを表示しない。
        None
    }
}

/// モードインジケーターを表示する。
///
/// `mode_char`: 表示文字（"あ", "ア", "A"）
/// `x`, `y`: キャレット位置（スクリーン座標、y はキャレット下端）
pub fn show(mode_char: &'static str, x: i32, y: i32) {
    // キャレット位置が未設定 (0,0) の場合は OS API で取得する
    let (x, y) = if x == 0 && y == 0 {
        match get_caret_screen_pos() {
            Some((cx, cy)) => (cx, cy),
            None => {
                tracing::debug!("mode_indicator: no caret position available");
                return;
            }
        }
    } else {
        (x, y)
    };

    let light = super::language_bar::is_light_mode();
    TL_TEXT.with(|t| t.set(mode_char));
    TL_LIGHT.with(|l| l.set(light));

    let win_y = unsafe { calc_window_y(x, y) };
    let hwnd = TL_HWND.with(|h| HWND(h.get() as *mut _));

    if is_valid(hwnd) {
        unsafe {
            let _ = SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                x,
                win_y,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOSIZE,
            );
            let _ = InvalidateRect(hwnd, None, BOOL(0));
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            // タイマーリセット
            let _ = KillTimer(hwnd, HIDE_TIMER_ID);
            let _ = SetTimer(hwnd, HIDE_TIMER_ID, FADE_START_MS, None);
        }
    } else {
        unsafe {
            ensure_class_registered();
            let hmod = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            match CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                PCWSTR(CLASS_NAME_UTF16.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                x,
                win_y,
                WIN_SIZE,
                WIN_SIZE,
                HWND::default(),
                HMENU::default(),
                hmod,
                None,
            ) {
                Ok(new_hwnd) if is_valid(new_hwnd) => {
                    TL_HWND.with(|h| h.set(new_hwnd.0 as isize));
                    let _ = ShowWindow(new_hwnd, SW_SHOWNOACTIVATE);
                    let _ = SetTimer(new_hwnd, HIDE_TIMER_ID, FADE_START_MS, None);
                    tracing::debug!("mode_indicator::create: hwnd={:?}", new_hwnd);
                }
                Ok(_) | Err(_) => tracing::warn!("mode_indicator::create: failed"),
            }
        }
    }
    VISIBLE.store(true, Ordering::Release);
}

/// モードインジケーターを非表示にする。
pub fn hide() {
    if !VISIBLE.load(Ordering::Acquire) {
        return;
    }
    let hwnd = TL_HWND.with(|h| HWND(h.get() as *mut _));
    if is_valid(hwnd) {
        unsafe {
            let _ = KillTimer(hwnd, HIDE_TIMER_ID);
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
    VISIBLE.store(false, Ordering::Release);
}

/// モードインジケーターを破棄する（Deactivate 時）。
pub fn destroy() {
    let hwnd = TL_HWND.with(|h| HWND(h.get() as *mut _));
    if is_valid(hwnd) {
        unsafe {
            let _ = KillTimer(hwnd, HIDE_TIMER_ID);
            let _ = DestroyWindow(hwnd);
        }
    }
    TL_HWND.with(|h| h.set(0));
    VISIBLE.store(false, Ordering::Release);
}

/// 表示中かどうか
#[allow(dead_code)]
pub fn is_visible() -> bool {
    VISIBLE.load(Ordering::Acquire)
}

// ─── ヘルパー ─────────────────────────────────────────────────────────────────

fn is_valid(hwnd: HWND) -> bool {
    !hwnd.0.is_null()
}

unsafe fn calc_window_y(x: i32, caret_bottom: i32) -> i32 {
    let pt = POINT { x, y: caret_bottom };
    let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(hmon, &mut mi).as_bool() {
        let work_bottom = mi.rcWork.bottom;
        if caret_bottom + WIN_SIZE > work_bottom {
            return caret_bottom - CARET_HEIGHT_ESTIMATE - WIN_SIZE - 4;
        }
    }
    caret_bottom
}
