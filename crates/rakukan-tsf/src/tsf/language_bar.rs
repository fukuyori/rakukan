//! 言語バー（システムトレイ）インジケーター

use windows::Win32::{
    Foundation::HINSTANCE,
    UI::TextServices::{
        GUID_COMPARTMENT_KEYBOARD_OPENCLOSE, GUID_LBI_INPUTMODE, ITfCompartmentMgr,
        ITfLangBarItemButton, ITfLangBarItemMgr, ITfThreadMgr, TF_LANGBARITEMINFO,
        TF_LBI_STYLE_BTN_BUTTON, TF_LBI_STYLE_SHOWNINTRAY,
    },
    UI::WindowsAndMessaging::{HICON, IMAGE_ICON, LR_DEFAULTSIZE, LR_SHARED, LoadImageW},
};
use windows_core::Interface;

use crate::globals::{DllModule, GUID_TEXT_SERVICE};

pub const LANGBAR_SINK_COOKIE: u32 = 0xACA_CACA;

pub fn make_langbar_info() -> TF_LANGBARITEMINFO {
    let mut info = TF_LANGBARITEMINFO {
        clsidService: GUID_TEXT_SERVICE,
        // GUID_LBI_INPUTMODE: Windows 標準の入力モードボタン。
        // キーボードレイアウト表示の隣に統合される（mozc と同じ方式）。
        // クリック時のポップアップメニューは OnClick 側で明示的に表示する。
        guidItem: GUID_LBI_INPUTMODE,
        dwStyle: TF_LBI_STYLE_BTN_BUTTON | TF_LBI_STYLE_SHOWNINTRAY,
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

#[allow(dead_code)]
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

// ─── モード別アイコン動的生成 ────────────────────────────────────────────────

use std::ptr::null_mut;
use windows::Win32::Graphics::Gdi::HDC;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, CreateFontW,
    DIB_RGB_COLORS, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW,
    HBITMAP, HFONT, SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};

/// テーマ (ライト/ダーク) を検出する。判定不能時はダークと見なす。
pub fn is_light_mode() -> bool {
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, KEY_READ, REG_DWORD, RegOpenKeyExW, RegQueryValueExW,
    };
    unsafe {
        let mut hkey = Default::default();
        let subkey: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            windows::core::PCWSTR(subkey.as_ptr()),
            0,
            KEY_READ,
            &mut hkey,
        )
        .is_err()
        {
            return false;
        }
        let val_name: Vec<u16> = "SystemUsesLightTheme"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut data = 0u32;
        let mut size = 4u32;
        let mut kind = REG_DWORD;
        if RegQueryValueExW(
            hkey,
            windows::core::PCWSTR(val_name.as_ptr()),
            None,
            Some(&mut kind),
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut size),
        )
        .is_ok()
        {
            data != 0
        } else {
            false
        }
    }
}

/// モード文字（"あ" / "ア" / "A"）から 16x16 HICON を動的生成する。
pub fn create_mode_icon(text: &str) -> windows::core::Result<HICON> {
    const SIZE: i32 = 16;
    let bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE,
            biHeight: -SIZE, // top-down
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

    // 背景塗り (テーマ対応)
    let light = is_light_mode();
    if !bits.is_null() {
        let p = bits as *mut u8;
        let (br, bg, bb) = if light {
            (240u8, 240u8, 240u8)
        } else {
            (24u8, 24u8, 24u8)
        };
        unsafe {
            for i in 0..((SIZE * SIZE) as usize) {
                *p.add(i * 4) = bb;
                *p.add(i * 4 + 1) = bg;
                *p.add(i * 4 + 2) = br;
                *p.add(i * 4 + 3) = 255;
            }
        }
    }

    // フォント
    let face: Vec<u16> = "Yu Gothic UI"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let hfont: HFONT = unsafe {
        CreateFontW(
            -14,
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
            windows::core::PCWSTR(face.as_ptr()),
        )
    };
    let old_font = unsafe { SelectObject(hdc, hfont) };
    unsafe { SetBkMode(hdc, TRANSPARENT) };

    // 文字色
    let color = if light {
        0x00_00_00_00u32
    } else {
        0x00_FF_FF_FFu32
    };
    unsafe { SetTextColor(hdc, windows::Win32::Foundation::COLORREF(color)) };

    let mut rc = windows::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: SIZE,
        bottom: SIZE,
    };
    let mut wbuf: Vec<u16> = text.encode_utf16().collect();
    let _ = unsafe {
        DrawTextW(
            hdc,
            &mut wbuf,
            &mut rc,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        )
    };

    // alpha 補正
    if !bits.is_null() {
        let p = bits as *mut u8;
        unsafe {
            for i in 0..((SIZE * SIZE) as usize) {
                *p.add(i * 4 + 3) = 255;
            }
        }
    }

    // 1bpp マスク
    let mask_bits = [0u8; (SIZE * SIZE / 8) as usize];
    let mask: HBITMAP = unsafe {
        windows::Win32::Graphics::Gdi::CreateBitmap(
            SIZE,
            SIZE,
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

    // GDI クリーンアップ
    let _ = unsafe { SelectObject(hdc, old_font) };
    let _ = unsafe { DeleteObject(hfont) };
    let _ = unsafe { SelectObject(hdc, old) };
    let _ = unsafe { DeleteDC(hdc) };
    let _ = unsafe { DeleteObject(mask) };
    let _ = unsafe { DeleteObject(hbmp) };

    Ok(hicon)
}
