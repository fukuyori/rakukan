//! システムトレイ用：入力モード共有（DLL→トレイ常駐プロセス）
//!
//! TSF DLL は複数プロセスにロードされうるため、
//! - `Local\\rakukan.mode` の共有メモリ（u32）へ現在モードを書き込み
//! - `Local\\rakukan.mode.changed` のイベントを SetEvent
//! という *最小IPC* でトレイアプリに通知する。
//!
//! 値フォーマット：
//! - bit0..1 : mode (0=Hiragana, 1=Katakana, 2=Alphanumeric)
//! - bit8    : open (1=open, 0=closed)

use std::sync::OnceLock;

use windows::Win32::{
    Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE},
    System::{
        Memory::{CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_WRITE, PAGE_READWRITE, MEMORY_MAPPED_VIEW_ADDRESS},
        Threading::{CreateEventW, SetEvent},
    },
};

use crate::engine::input_mode::InputMode;

const MAP_NAME: &str = "Local\\rakukan.mode";
const EVT_NAME: &str = "Local\\rakukan.mode.changed";

#[derive(Copy, Clone)]
struct TrayIpc {
    map: HANDLE,
    evt: HANDLE,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
}

unsafe impl Send for TrayIpc {}
unsafe impl Sync for TrayIpc {}

static IPC: OnceLock<TrayIpc> = OnceLock::new();

fn to_wide_z(s: &str) -> Vec<u16> {
    let mut v: Vec<u16> = s.encode_utf16().collect();
    v.push(0);
    v
}

fn encode(open: bool, mode: InputMode) -> u32 {
    let m = match mode {
        InputMode::Hiragana => 0u32,
        InputMode::Katakana => 1u32,
        InputMode::Alphanumeric => 2u32,
    };
    m | ((open as u32) << 8)
}

/// 共有メモリとイベントを初期化する。
/// 失敗しても IME 本体は動作継続するため、Result は握りつぶしやすい。
pub fn init() -> windows::core::Result<()> {
    if IPC.get().is_some() {
        return Ok(());
    }

    unsafe {
        let map_name = to_wide_z(MAP_NAME);
        let evt_name = to_wide_z(EVT_NAME);

        let map = CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            None,
            PAGE_READWRITE,
            0,
            4,
            windows::core::PCWSTR(map_name.as_ptr()),
        )?;

        let view = MapViewOfFile(map, FILE_MAP_WRITE, 0, 0, 4);
        if view.Value.is_null() {
            let _ = CloseHandle(map);
            return Err(windows::core::Error::from_win32());
        }

        let evt = CreateEventW(
            None,
            false, // auto-reset
            false,
            windows::core::PCWSTR(evt_name.as_ptr()),
        )?;

        let ipc = TrayIpc { map, evt, view };
        let _ = IPC.set(ipc);
    }
    Ok(())
}

/// 現在モードを共有し、トレイへ通知する。
pub fn publish(open: bool, mode: InputMode) {
    let _ = init();
    let Some(ipc) = IPC.get().copied() else { return; };
    unsafe {
        (ipc.view.Value as *mut u32).write_volatile(encode(open, mode));
        let _ = SetEvent(ipc.evt);
    }
}

/// 明示的に解放したい場合（通常は不要）。
#[allow(dead_code)]
pub fn shutdown() {
    if let Some(ipc) = IPC.get().copied() {
        unsafe {
            let _ = UnmapViewOfFile(ipc.view);
            let _ = CloseHandle(ipc.evt);
            let _ = CloseHandle(ipc.map);
        }
    }
}
