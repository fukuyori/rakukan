#![windows_subsystem = "windows"]

use anyhow::Result;
use std::{
    mem::size_of,
    ptr::null_mut,
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

use windows::Win32::{
    Foundation::{
        COLORREF, CloseHandle, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, WPARAM,
    },
    Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, CreateFontW,
        DIB_RGB_COLORS, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW,
        HBITMAP, HDC, HFONT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    },
    System::{
        Memory::{
            CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
            OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
        },
        SystemInformation::GetTickCount64,
        Threading::{
            CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SYNCHRONIZATION_ACCESS_RIGHTS,
            SetEvent, WaitForSingleObject,
        },
    },
    UI::{
        Shell::{
            NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
            NIM_SETVERSION, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
        },
        WindowsAndMessaging::{
            AppendMenuW, CW_USEDEFAULT, CreateIconIndirect, CreatePopupMenu, CreateWindowExW,
            DefWindowProcW, DestroyIcon, DispatchMessageW, GetCursorPos, GetMessageW, ICONINFO,
            LoadCursorW, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SW_HIDE,
            SetForegroundWindow, SetTimer, ShowWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
            TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage, WM_APP, WM_COMMAND, WM_CREATE,
            WM_DESTROY, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPEDWINDOW,
        },
    },
};
use windows::core::GUID;
use windows::core::PCWSTR;

const MAP_NAME: &str = "Local\\rakukan.mode";
const EVT_NAME: &str = "Local\\rakukan.mode.changed";
const RELOAD_EVT_NAME: &str = "Local\\rakukan.engine.reload";
const WM_TRAY: u32 = WM_APP + 1;
const WM_MODE_UPDATE: u32 = WM_APP + 2;

const ID_MENU_RELOAD: usize = 1002;
const ID_MENU_EXIT: usize = 1001;

/// Stable GUID for the tray icon so Windows can persist settings (e.g. promoted / always visible).
/// (Must match the GUID referenced from install.ps1 when setting IsPromoted.)
const TRAY_GUID: GUID = GUID::from_u128(0x9c8b5a79_9f7f_4d6a_bf87_2e50b5d7a2c1);

static RUNNING: AtomicBool = AtomicBool::new(true);
static LAST_UPDATE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static ICON_SHOWN: AtomicBool = AtomicBool::new(true);
const TIMER_ID: usize = 1;

/// Access mask bit (not exported as a const by windows 0.58 in this module).
const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((r as u32) | ((g as u32) << 8) | ((b as u32) << 16))
}

fn is_light_mode() -> bool {
    // 1 = light, 0 = dark
    // HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let key = to_wide_z("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize");
    let val = to_wide_z("AppsUseLightTheme");
    let mut data: u32 = 1;
    let mut cb: u32 = std::mem::size_of::<u32>() as u32;
    unsafe {
        let r = RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(val.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut data as *mut u32) as *mut core::ffi::c_void),
            Some(&mut cb),
        );
        if r.is_ok() {
            return data != 0;
        }
    }
    // fallback: assume dark taskbar is common -> use white
    false
}

fn to_wide_z(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Hiragana,
    Katakana,
    Alnum,
}

fn decode(v: u32) -> (bool, Mode) {
    let open = ((v >> 8) & 1) != 0;
    let m = match v & 0b11 {
        1 => Mode::Katakana,
        2 => Mode::Alnum,
        _ => Mode::Hiragana,
    };
    (open, m)
}

fn create_mode_icon(text: &str) -> Result<windows::Win32::UI::WindowsAndMessaging::HICON> {
    // 32x32 ARGB DIB
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: 32,
            biHeight: -32, // top-down
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0 as u32,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits: *mut core::ffi::c_void = null_mut();
    let hdc = unsafe { CreateCompatibleDC(HDC(null_mut())) };
    let hbmp: HBITMAP = unsafe { CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)? };
    let old = unsafe { SelectObject(hdc, hbmp) };

    // Fill background (opaque) so the glyph remains visible on any taskbar theme.
    // BGRA order.
    let light = is_light_mode();
    if !bits.is_null() {
        let p = bits as *mut u8;
        // light mode: near-white background, dark mode: near-black background
        let (br, bg, bb) = if light {
            (240u8, 240u8, 240u8)
        } else {
            (24u8, 24u8, 24u8)
        };
        unsafe {
            for i in 0..(32 * 32) {
                *p.add(i * 4) = bb; // B
                *p.add(i * 4 + 1) = bg; // G
                *p.add(i * 4 + 2) = br; // R
                *p.add(i * 4 + 3) = 255; // A
            }
        }
    }

    // Font
    let face = to_wide_z("Yu Gothic UI");
    // Bigger, bolder font for better readability.
    let hfont: HFONT = unsafe {
        CreateFontW(
            -28,
            0,
            0,
            0,
            800,
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
    };
    let old_font = unsafe { SelectObject(hdc, hfont) };
    unsafe { SetBkMode(hdc, TRANSPARENT) };

    // Foreground
    let light = is_light_mode();
    unsafe {
        if light {
            SetTextColor(hdc, rgb(0, 0, 0));
        } else {
            SetTextColor(hdc, rgb(255, 255, 255));
        }
    }
    let mut rc2 = windows::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 32,
        bottom: 32,
    };
    let mut wbuf = to_wide_z(text);
    wbuf.pop(); // remove NUL for DrawTextW slice
    let _ = unsafe {
        DrawTextW(
            hdc,
            &mut wbuf,
            &mut rc2,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        )
    };

    // GDI does not guarantee alpha channel writes; force opaque alpha for all pixels.
    if !bits.is_null() {
        let p = bits as *mut u8;
        unsafe {
            for i in 0..(32 * 32) {
                *p.add(i * 4 + 3) = 255;
            }
        }
    }

    // Create 1bpp mask (all opaque)
    // windows 0.58 returns HBITMAP directly here (not Result).
    let mask_bits = [0u8; 128];
    let mask: HBITMAP = unsafe {
        windows::Win32::Graphics::Gdi::CreateBitmap(
            32,
            32,
            1,
            1,
            Some(mask_bits.as_ptr() as *const core::ffi::c_void),
        )
    };
    let ii = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: hbmp,
    };
    let hicon = unsafe { CreateIconIndirect(&ii)? };

    // Cleanup GDI objects
    let _ = unsafe { SelectObject(hdc, old_font) };
    let _ = unsafe { DeleteObject(hfont) };
    let _ = unsafe { SelectObject(hdc, old) };
    let _ = unsafe { DeleteDC(hdc) };
    let _ = unsafe { DeleteObject(mask) };
    let _ = unsafe { DeleteObject(hbmp) };

    Ok(hicon)
}

struct Shared {
    map: HANDLE,
    evt: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

impl Drop for Shared {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(self.view);
            let _ = CloseHandle(self.evt);
            let _ = CloseHandle(self.map);
        }
    }
}

unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    fn open_or_create() -> Result<Self> {
        let map_name = to_wide_z(MAP_NAME);
        let evt_name = to_wide_z(EVT_NAME);

        // Try open first
        let map = unsafe { OpenFileMappingW(FILE_MAP_READ.0, false, PCWSTR(map_name.as_ptr())) }
            .or_else(|_| {
                // create if not exists (so tray can run before IME activates)
                unsafe {
                    CreateFileMappingW(
                        INVALID_HANDLE_VALUE,
                        None,
                        PAGE_READWRITE,
                        0,
                        4,
                        PCWSTR(map_name.as_ptr()),
                    )
                }
            })?;

        let view = unsafe { MapViewOfFile(map, FILE_MAP_READ, 0, 0, 4) };
        if view.Value.is_null() {
            let _ = unsafe { CloseHandle(map) };
            anyhow::bail!("MapViewOfFile failed");
        }

        let evt = unsafe {
            OpenEventW(
                SYNCHRONIZATION_ACCESS_RIGHTS(SYNCHRONIZE_ACCESS | EVENT_MODIFY_STATE.0),
                false,
                PCWSTR(evt_name.as_ptr()),
            )
        }
        .or_else(|_| unsafe { CreateEventW(None, false, false, PCWSTR(evt_name.as_ptr())) })?;

        Ok(Self { map, evt, view })
    }

    fn read(&self) -> u32 {
        unsafe { (self.view.Value as *const u32).read_volatile() }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => LRESULT(0),
        WM_MODE_UPDATE => {
            // wParam: low 16bit open flag, high 16bit mode
            let v = w.0 as u32;
            let open = (v & 1) != 0;
            let mode = match (v >> 16) & 0xffff {
                1 => Mode::Katakana,
                2 => Mode::Alnum,
                _ => Mode::Hiragana,
            };
            LAST_UPDATE_MS.store(unsafe { GetTickCount64() }, Ordering::Release);
            if !ICON_SHOWN.load(Ordering::Acquire) {
                let _ = add_notify_icon(hwnd, open, mode);
                ICON_SHOWN.store(true, Ordering::Release);
            }
            let _ = update_notify_icon(hwnd, open, mode);
            LRESULT(0)
        }
        WM_TIMER => {
            let now = unsafe { GetTickCount64() };
            let last = LAST_UPDATE_MS.load(Ordering::Acquire);
            // If no update for a while, hide the indicator (IME likely inactive).
            if last != 0 && now.saturating_sub(last) > 2500 {
                if ICON_SHOWN.load(Ordering::Acquire) {
                    let _ = delete_notify_icon(hwnd);
                    ICON_SHOWN.store(false, Ordering::Release);
                }
            }
            LRESULT(0)
        }
        WM_TRAY => {
            // lParam: マウスメッセージ
            if l.0 as u32 == WM_RBUTTONUP {
                let _ = show_context_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (w.0 & 0xffff) as usize;
            if id == ID_MENU_RELOAD {
                signal_engine_reload();
                return LRESULT(0);
            }
            if id == ID_MENU_EXIT {
                RUNNING.store(false, Ordering::Release);
                unsafe {
                    PostQuitMessage(0);
                }
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, w, l) }
        }
        WM_DESTROY => {
            RUNNING.store(false, Ordering::Release);
            unsafe {
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, w, l) },
    }
}

/// TSF DLL に「エンジン再起動」を要求する。
/// 名前付きイベント `Local\rakukan.engine.reload` を SetEvent する。
fn signal_engine_reload() {
    let name = to_wide_z(RELOAD_EVT_NAME);
    unsafe {
        use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
        match OpenEventW(
            EVENT_MODIFY_STATE,
            false,
            windows::core::PCWSTR(name.as_ptr()),
        ) {
            Ok(h) => {
                let _ = SetEvent(h);
                let _ = windows::Win32::Foundation::CloseHandle(h);
            }
            Err(_) => {
                // TSF DLL がまだロードされていないか、すでに終了している
            }
        }
    }
}

fn show_context_menu(hwnd: HWND) -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{MENU_ITEM_FLAGS, MF_SEPARATOR};
    let hmenu = unsafe { CreatePopupMenu()? };
    let txt_reload = to_wide_z("エンジン再起動");
    let _ = unsafe {
        AppendMenuW(
            hmenu,
            MENU_ITEM_FLAGS(0),
            ID_MENU_RELOAD,
            PCWSTR(txt_reload.as_ptr()),
        )
    };
    let _ = unsafe { AppendMenuW(hmenu, MF_SEPARATOR, 0, PCWSTR::null()) };
    let txt_exit = to_wide_z("終了");
    let _ = unsafe {
        AppendMenuW(
            hmenu,
            MENU_ITEM_FLAGS(0),
            ID_MENU_EXIT,
            PCWSTR(txt_exit.as_ptr()),
        )
    };
    let mut pt = windows::Win32::Foundation::POINT { x: 0, y: 0 };
    let _ = unsafe { GetCursorPos(&mut pt) };
    // TrackPopupMenu を正しく閉じるために必要
    let _ = unsafe { SetForegroundWindow(hwnd) };
    let _ = unsafe {
        TrackPopupMenu(
            hmenu,
            TPM_LEFTALIGN | TPM_BOTTOMALIGN | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            0,
            hwnd,
            None,
        )
    };
    Ok(())
}

fn update_notify_icon(hwnd: HWND, open: bool, mode: Mode) -> Result<()> {
    let text = if !open {
        "A"
    } else {
        match mode {
            Mode::Hiragana => "あ",
            Mode::Katakana => "ア",
            Mode::Alnum => "A",
        }
    };

    let tip = format!("Rakukan: {text}");
    let mut tip_w: Vec<u16> = tip.encode_utf16().collect();
    tip_w.push(0);

    let hicon = create_mode_icon(text)?;

    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uFlags = NIF_GUID | NIF_ICON | NIF_TIP;
    nid.guidItem = TRAY_GUID;
    nid.hIcon = hicon;
    // szTip is [u16; 128]
    for (i, c) in tip_w.iter().take(nid.szTip.len() - 1).enumerate() {
        nid.szTip[i] = *c;
    }
    let _ = unsafe { Shell_NotifyIconW(NIM_MODIFY, &nid) };

    let _ = unsafe { DestroyIcon(hicon) };
    Ok(())
}

fn add_notify_icon(hwnd: HWND, open: bool, mode: Mode) -> Result<()> {
    // NIM_ADD でアイコンを必ず設定しないと、環境によってはトレイに表示されない。
    let text = if !open {
        "A"
    } else {
        match mode {
            Mode::Hiragana => "あ",
            Mode::Katakana => "ア",
            Mode::Alnum => "A",
        }
    };
    let tip = format!("Rakukan: {text}");
    let mut tip_w: Vec<u16> = tip.encode_utf16().collect();
    tip_w.push(0);
    let hicon = create_mode_icon(text)?;

    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uFlags = NIF_GUID | NIF_MESSAGE | NIF_ICON | NIF_TIP;
    nid.guidItem = TRAY_GUID;
    nid.uCallbackMessage = WM_TRAY;
    nid.hIcon = hicon;
    for (i, c) in tip_w.iter().take(nid.szTip.len() - 1).enumerate() {
        nid.szTip[i] = *c;
    }
    let _ = unsafe { Shell_NotifyIconW(NIM_ADD, &nid) };

    // Use the latest NOTIFYICON behavior (improves reliability on Windows 11).
    let mut ver = nid;
    ver.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    let _ = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &ver) };

    let _ = unsafe { DestroyIcon(hicon) };
    Ok(())
}

fn delete_notify_icon(hwnd: HWND) -> Result<()> {
    let mut nid = NOTIFYICONDATAW::default();
    nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = hwnd;
    nid.uFlags = NIF_GUID;
    nid.guidItem = TRAY_GUID;
    let _ = unsafe { Shell_NotifyIconW(NIM_DELETE, &nid) };
    Ok(())
}

fn main() -> Result<()> {
    unsafe {
        let class = to_wide_z("rakukan.tray");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hCursor: LoadCursorW(None, windows::Win32::UI::WindowsAndMessaging::IDC_ARROW)?,
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            Default::default(),
            PCWSTR(class.as_ptr()),
            PCWSTR(class.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            None,
            None,
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?,
            Some(null_mut()),
        )?;
        let _ = ShowWindow(hwnd, SW_HIDE);

        // 初期表示（アイコン未設定だとトレイに出ない環境があるため、NIM_ADDで必ず設定）
        add_notify_icon(hwnd, true, Mode::Hiragana)?;
        ICON_SHOWN.store(true, Ordering::Release);
        LAST_UPDATE_MS.store(GetTickCount64(), Ordering::Release);
        let _ = SetTimer(hwnd, TIMER_ID, 1000, None);

        let shared = Shared::open_or_create()?;

        // notifier thread
        // HWND is not Send; pass its raw value across threads.
        let hwnd2 = hwnd.0 as usize;
        let evt_for_shutdown = shared.evt; // HANDLE is Copy
        let watcher = thread::spawn(move || {
            // shared is owned by this thread; it will be dropped at thread end.
            while RUNNING.load(Ordering::Acquire) {
                let _ = WaitForSingleObject(shared.evt, INFINITE);
                if !RUNNING.load(Ordering::Acquire) {
                    break;
                }
                let (open, mode) = decode(shared.read());
                let mode_id = match mode {
                    Mode::Hiragana => 0u32,
                    Mode::Katakana => 1u32,
                    Mode::Alnum => 2u32,
                };
                let w = WPARAM(((mode_id as u32) << 16 | (open as u32)) as usize);
                let hwnd_send = HWND(hwnd2 as *mut core::ffi::c_void);
                let _ = PostMessageW(hwnd_send, WM_MODE_UPDATE, w, LPARAM(0));
            }
        });

        // message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(null_mut()), 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = delete_notify_icon(hwnd);

        // stop watcher and wait
        RUNNING.store(false, Ordering::Release);
        let _ = SetEvent(evt_for_shutdown);
        let _ = watcher.join();
    }
    Ok(())
}
