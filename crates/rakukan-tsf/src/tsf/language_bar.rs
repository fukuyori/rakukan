//! 言語バー（システムトレイ）インジケーター

use windows::Win32::{
    Foundation::HINSTANCE,
    UI::TextServices::{
        GUID_COMPARTMENT_KEYBOARD_OPENCLOSE, ITfCompartmentMgr, ITfLangBarItemButton,
        ITfLangBarItemMgr, ITfThreadMgr, TF_LANGBARITEMINFO, TF_LBI_STYLE_BTN_BUTTON,
    },
    UI::WindowsAndMessaging::{HICON, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, LoadImageW},
};
use windows_core::Interface;

use crate::globals::{DllModule, GUID_TEXT_SERVICE};

pub const LANGBAR_SINK_COOKIE: u32 = 0xACA_CACA;

pub fn make_langbar_info() -> TF_LANGBARITEMINFO {
    let mut info = TF_LANGBARITEMINFO {
        clsidService: GUID_TEXT_SERVICE,
        // 独自 GUID — GUID_LBI_INPUTMODE を使うとシステムインジケーターと重複する
        guidItem: windows::core::GUID::from_u128(0xACACACA0_0000_0000_0000_ACACACACACAC),
        dwStyle: TF_LBI_STYLE_BTN_BUTTON | 2u32, // 2 = TF_LBI_STYLE_SHOWNINTRAY
        ulSort: 0,
        szDescription: [0; 32],
    };
    let desc: Vec<u16> = "rakukan".encode_utf16().collect();
    for (i, &c) in desc.iter().take(31).enumerate() {
        info.szDescription[i] = c;
    }
    info
}

pub unsafe fn langbar_add(
    thread_mgr: &ITfThreadMgr,
    item: &ITfLangBarItemButton,
) -> anyhow::Result<()> {
    thread_mgr
        .cast::<ITfLangBarItemMgr>()
        .map_err(|e| anyhow::anyhow!("cast ITfLangBarItemMgr: {e}"))?
        .AddItem(item)
        .map_err(|e| anyhow::anyhow!("AddItem: {e}"))?;
    Ok(())
}

pub unsafe fn langbar_remove(
    thread_mgr: &ITfThreadMgr,
    item: &ITfLangBarItemButton,
) -> anyhow::Result<()> {
    let _ = thread_mgr
        .cast::<ITfLangBarItemMgr>()
        .map_err(|e| anyhow::anyhow!("cast ITfLangBarItemMgr: {e}"))?
        .RemoveItem(item);
    Ok(())
}

// ─── コンパートメント操作 ────────────────────────────────────────────────────
// windows_core::VARIANT::from(i32) を使う（内部で VT_I4 を正しく設定する）

pub unsafe fn set_open_close(
    thread_mgr: &ITfThreadMgr,
    tid: u32,
    open: bool,
) -> anyhow::Result<()> {
    let mgr = thread_mgr
        .cast::<ITfCompartmentMgr>()
        .map_err(|e| anyhow::anyhow!("ITfCompartmentMgr cast: {e}"))?;
    let comp = mgr
        .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE)
        .map_err(|e| anyhow::anyhow!("GetCompartment: {e}"))?;
    // windows_core::VARIANT::from(i32) は内部で VT_I4 を正しく設定する
    let var = windows_core::VARIANT::from(if open { 1i32 } else { 0i32 });
    comp.SetValue(tid, &var)
        .map_err(|e| anyhow::anyhow!("SetValue hr={e}"))?;
    Ok(())
}

pub unsafe fn toggle_open_close(thread_mgr: &ITfThreadMgr, tid: u32) -> anyhow::Result<()> {
    // 現在値を取得してトグル
    let mgr = thread_mgr
        .cast::<ITfCompartmentMgr>()
        .map_err(|e| anyhow::anyhow!("ITfCompartmentMgr cast: {e}"))?;
    let comp = mgr
        .GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE)
        .map_err(|e| anyhow::anyhow!("GetCompartment: {e}"))?;
    let current = comp
        .GetValue()
        .ok()
        .and_then(|v| i32::try_from(&v).ok())
        .unwrap_or(1);
    let var = windows_core::VARIANT::from(if current == 0 { 1i32 } else { 0i32 });
    comp.SetValue(tid, &var)
        .map_err(|e| anyhow::anyhow!("SetValue hr={e}"))?;
    Ok(())
}

pub fn get_open_close(thread_mgr: &ITfThreadMgr) -> bool {
    unsafe {
        let Ok(mgr) = thread_mgr.cast::<ITfCompartmentMgr>() else {
            return true;
        };
        let Ok(comp) = mgr.GetCompartment(&GUID_COMPARTMENT_KEYBOARD_OPENCLOSE) else {
            return true;
        };
        comp.GetValue()
            .ok()
            .and_then(|v| i32::try_from(&v).ok())
            .map(|n| n != 0)
            .unwrap_or(true)
    }
}

// ─── アイコン ────────────────────────────────────────────────────────────────

pub unsafe fn load_tray_icon() -> windows::core::Result<HICON> {
    let hinst: HINSTANCE = DllModule::get()
        .ok()
        .and_then(|m| m.hinst)
        .map(|h| unsafe { std::mem::transmute(h) })
        .unwrap_or_default();
    let handle = LoadImageW(
        hinst,
        windows::core::PCWSTR(1u16 as *mut u16),
        IMAGE_ICON,
        0,
        0,
        LR_DEFAULTSIZE | LR_SHARED,
    )?;
    Ok(HICON(handle.0))
}
