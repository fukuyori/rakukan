//! 知らせの窓（キャレットの近くに文を一時表示する）
//!
//! 日本語入力を使えなくなったとき（回復できない場合の処置で直接入力に入ったとき）と、
//! 使えるようになったときに、何が起きたかと戻し方を知らせる（入力処理の見直しの段 6、
//! docs/Input_Pipeline_Plan.md 4.2 の第 9 版の案）。`mode_indicator` は 1〜2 文字の記号しか
//! 出せないので、文を出すために別に持つ。
//!
//! # 表示仕様
//! - WS_POPUP + WS_EX_TOPMOST + WS_EX_NOACTIVATE（フォーカスを奪わない）
//! - キャレットの下に出す。キャレットの位置が取れなければ、前面の窓の下寄りの中央に出す
//! - 決めた時間で消える（キーでは消さない。打鍵の途中に出るので、すぐ消えると読めない）

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::{
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BACKGROUND_MODE, BeginPaint, CreateCompatibleDC, CreateFontW, CreateSolidBrush,
            DT_CALCRECT, DT_LEFT, DT_NOPREFIX, DT_WORDBREAK, DeleteDC, DeleteObject, DrawTextW,
            EndPaint, FillRect, GetMonitorInfoW, HDC, HFONT, InvalidateRect,
            MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, PAINTSTRUCT, SelectObject,
            SetBkMode, SetTextColor,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, GetForegroundWindow, GetWindowRect,
            HMENU, HWND_TOPMOST, KillTimer, RegisterClassW, SW_HIDE, SW_SHOWNOACTIVATE,
            SWP_NOACTIVATE, SetTimer, SetWindowPos, ShowWindow, WM_ERASEBKGND, WM_PAINT, WM_TIMER,
            WNDCLASSW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
        },
    },
    core::PCWSTR,
};

/// 日本語入力を使えなくなったときの知らせ（原因ごとに分けない）。
pub(crate) const UNAVAILABLE: Notice = Notice {
    text: "日本語入力を使えません。PC を再起動してください。直らない場合は、Rakukan を再インストールしてください。",
    show_ms: 10_000,
};

/// 日本語入力が使えるようになったときの知らせ。
pub(crate) const RECOVERED: Notice = Notice {
    text: "日本語入力が使えるようになりました。",
    show_ms: 3_000,
};

/// 知らせの文と、表示しておく時間。
#[derive(Clone, Copy)]
pub(crate) struct Notice {
    pub text: &'static str,
    pub show_ms: u32,
}

/// 文を折り返す幅（ピクセル）と余白・文字の高さ。
const MAX_TEXT_WIDTH: i32 = 360;
const PADDING: i32 = 10;
const FONT_HEIGHT: i32 = 18;
/// キャレットの下に空ける間（`mode_indicator` の記号と重ならないように）。
const BELOW_CARET: i32 = 36;
const HIDE_TIMER_ID: usize = 101;

const COLOR_BG_LIGHT: COLORREF = COLORREF(0x00_FF_FF_FF);
const COLOR_FG_LIGHT: COLORREF = COLORREF(0x00_33_33_33);
const COLOR_BG_DARK: COLORREF = COLORREF(0x00_33_33_33);
const COLOR_FG_DARK: COLORREF = COLORREF(0x00_FF_FF_FF);

thread_local! {
    static TL_HWND: Cell<isize> = const { Cell::new(0) };
    static TL_TEXT: Cell<&'static str> = const { Cell::new("") };
    static TL_LIGHT: Cell<bool> = const { Cell::new(false) };
    static TL_SIZE: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
}

static CLASS_REGISTERED: AtomicBool = AtomicBool::new(false);
static CLASS_NAME: &str = "RakukanNotice\0";

fn class_name() -> Vec<u16> {
    CLASS_NAME.encode_utf16().collect()
}

unsafe fn ensure_class_registered() {
    if CLASS_REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }
    let name = class_name();
    let hmod = unsafe { GetModuleHandleW(PCWSTR::null()) }.unwrap_or_default();
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: hmod.into(),
        // RegisterClassW は名前を写すので、呼び出しの間だけ生きていればよい
        lpszClassName: PCWSTR(name.as_ptr()),
        ..Default::default()
    };
    unsafe { RegisterClassW(&wc) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = unsafe { BeginPaint(hwnd, &mut ps) };
            if !hdc.is_invalid() {
                unsafe { draw(hdc) };
                let _ = unsafe { EndPaint(hwnd, &ps) };
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
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

unsafe fn create_font() -> HFONT {
    let face: Vec<u16> = "Yu Gothic UI\0".encode_utf16().collect();
    unsafe {
        CreateFontW(
            FONT_HEIGHT,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            PCWSTR(face.as_ptr()),
        )
    }
}

/// 文を折り返したときの大きさ（文の部分だけ）。
unsafe fn measure(text: &str) -> (i32, i32) {
    unsafe {
        let hdc = CreateCompatibleDC(None);
        let font = create_font();
        let old = SelectObject(hdc, font);
        let mut buf: Vec<u16> = text.encode_utf16().collect();
        let mut rc = RECT {
            left: 0,
            top: 0,
            right: MAX_TEXT_WIDTH,
            bottom: 0,
        };
        DrawTextW(
            hdc,
            &mut buf,
            &mut rc,
            DT_CALCRECT | DT_WORDBREAK | DT_LEFT | DT_NOPREFIX,
        );
        let _ = SelectObject(hdc, old);
        let _ = DeleteObject(font);
        let _ = DeleteDC(hdc);
        (rc.right - rc.left, rc.bottom - rc.top)
    }
}

unsafe fn draw(hdc: HDC) {
    let text = TL_TEXT.with(|t| t.get());
    let light = TL_LIGHT.with(|l| l.get());
    let (w, h) = TL_SIZE.with(|s| s.get());
    let (bg, fg) = if light {
        (COLOR_BG_LIGHT, COLOR_FG_LIGHT)
    } else {
        (COLOR_BG_DARK, COLOR_FG_DARK)
    };
    unsafe {
        let brush = CreateSolidBrush(bg);
        let all = RECT {
            left: 0,
            top: 0,
            right: w,
            bottom: h,
        };
        FillRect(hdc, &all, brush);
        let _ = DeleteObject(brush);
        let font = create_font();
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, BACKGROUND_MODE(1)); // TRANSPARENT
        SetTextColor(hdc, fg);
        let mut buf: Vec<u16> = text.encode_utf16().collect();
        let mut rc = RECT {
            left: PADDING,
            top: PADDING,
            right: w - PADDING,
            bottom: h - PADDING,
        };
        DrawTextW(hdc, &mut buf, &mut rc, DT_WORDBREAK | DT_LEFT | DT_NOPREFIX);
        let _ = SelectObject(hdc, old);
        let _ = DeleteObject(font);
    }
}

/// 表示する位置（左上）。キャレットの下、取れなければ前面の窓の下寄りの中央。作業領域に
/// 収める。
fn position(w: i32, h: i32) -> Option<(i32, i32)> {
    let (x, y) = match super::mode_indicator::caret_screen_pos() {
        Some((x, caret_bottom)) => (x, caret_bottom + BELOW_CARET),
        None => unsafe {
            let fg = GetForegroundWindow();
            let mut rc = RECT::default();
            if fg.0.is_null() || GetWindowRect(fg, &mut rc).is_err() {
                return None;
            }
            ((rc.left + rc.right - w) / 2, rc.bottom - h - 48)
        },
    };
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let hmon = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    if unsafe { GetMonitorInfoW(hmon, &mut mi) }.as_bool() {
        let work = mi.rcWork;
        let x = x.clamp(work.left, (work.right - w).max(work.left));
        let y = y.clamp(work.top, (work.bottom - h).max(work.top));
        return Some((x, y));
    }
    Some((x, y))
}

/// 知らせを出す（同じ窓で前の知らせを置き換える）。位置が決まらなければ出さない（WARN）。
pub(crate) fn show(notice: Notice) {
    let (tw, th) = unsafe { measure(notice.text) };
    let (w, h) = (tw + PADDING * 2, th + PADDING * 2);
    let Some((x, y)) = position(w, h) else {
        tracing::warn!("notice: no position to show {:?}", notice.text);
        return;
    };
    TL_TEXT.with(|t| t.set(notice.text));
    TL_LIGHT.with(|l| l.set(super::language_bar::is_light_mode()));
    TL_SIZE.with(|s| s.set((w, h)));
    let mut hwnd = TL_HWND.with(|t| HWND(t.get() as *mut _));
    unsafe {
        if hwnd.0.is_null() {
            ensure_class_registered();
            let name = class_name();
            let hmod = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            match CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                PCWSTR(name.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                x,
                y,
                w,
                h,
                HWND::default(),
                HMENU::default(),
                hmod,
                None,
            ) {
                Ok(new_hwnd) if !new_hwnd.0.is_null() => {
                    TL_HWND.with(|t| t.set(new_hwnd.0 as isize));
                    hwnd = new_hwnd;
                }
                _ => {
                    tracing::warn!("notice: CreateWindowExW failed");
                    return;
                }
            }
        }
        let _ = SetWindowPos(hwnd, HWND_TOPMOST, x, y, w, h, SWP_NOACTIVATE);
        let _ = InvalidateRect(hwnd, None, BOOL(0));
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let _ = KillTimer(hwnd, HIDE_TIMER_ID);
        let _ = SetTimer(hwnd, HIDE_TIMER_ID, notice.show_ms, None);
    }
    tracing::info!("notice: shown {:?}", notice.text);
}

/// 知らせを隠す。
pub(crate) fn hide() {
    let hwnd = TL_HWND.with(|t| HWND(t.get() as *mut _));
    if !hwnd.0.is_null() {
        unsafe {
            let _ = KillTimer(hwnd, HIDE_TIMER_ID);
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

/// 知らせの窓を破棄する（Deactivate）。
pub(crate) fn destroy() {
    let hwnd = TL_HWND.with(|t| HWND(t.get() as *mut _));
    if !hwnd.0.is_null() {
        unsafe {
            let _ = KillTimer(hwnd, HIDE_TIMER_ID);
            let _ = DestroyWindow(hwnd);
        }
    }
    TL_HWND.with(|t| t.set(0));
}
