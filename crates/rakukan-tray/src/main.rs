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
        CloseHandle, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, LRESULT, WPARAM,
    },
    System::{
        Memory::{
            CreateFileMappingW, FILE_MAP_READ, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
            OpenFileMappingW, PAGE_READWRITE, UnmapViewOfFile,
        },
        Threading::{
            CreateEventW, EVENT_MODIFY_STATE, INFINITE, OpenEventW, SYNCHRONIZATION_ACCESS_RIGHTS,
            SetEvent, WaitForSingleObject,
        },
    },
    UI::{
        Shell::{
            NIF_GUID, NIM_DELETE,
            NOTIFYICONDATAW, Shell_NotifyIconW,
        },
        WindowsAndMessaging::{
            AppendMenuW, CW_USEDEFAULT, CreatePopupMenu, CreateWindowExW,
            DefWindowProcW, DispatchMessageW, GetCursorPos, GetMessageW,
            LoadCursorW, MSG, PostMessageW, PostQuitMessage, RegisterClassW, SW_HIDE,
            SetForegroundWindow, ShowWindow, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
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
static ICON_SHOWN: AtomicBool = AtomicBool::new(true);

/// Access mask bit (not exported as a const by windows 0.58 in this module).
const SYNCHRONIZE_ACCESS: u32 = 0x0010_0000;

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
            // mozc 方式: モード表示は TSF 言語バー (GUID_LBI_INPUTMODE) が担う。
            // トレイアイコン (Shell_NotifyIcon) は表示しない。
            LRESULT(0)
        }
        WM_TIMER => {
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

        // mozc 方式: トレイアイコンは表示しない（言語バーに一本化）。
        // タイマーも不要。
        // add_notify_icon(hwnd, true, Mode::Hiragana)?;
        ICON_SHOWN.store(false, Ordering::Release);

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
