//! Utility traits

use windows::{
    core::{GUID, HSTRING, PCWSTR},
    Win32::{
        System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW,
            HKEY, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
        },
        UI::Input::KeyboardAndMouse::{GetKeyState, VIRTUAL_KEY},
    },
};

// ─── StringExt ───────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub trait StringExt {
    fn to_wide_16(&self) -> Vec<u16>;
    fn to_wide_16_unpadded(&self) -> Vec<u16>;
    fn to_wide(&self) -> Vec<u8>;
}

impl StringExt for &str {
    fn to_wide_16(&self) -> Vec<u16> { self.encode_utf16().chain(Some(0)).collect() }
    fn to_wide_16_unpadded(&self) -> Vec<u16> { self.encode_utf16().collect() }
    fn to_wide(&self) -> Vec<u8> {
        self.encode_utf16().flat_map(|c| c.to_le_bytes()).chain(Some(0)).collect()
    }
}

impl StringExt for String {
    fn to_wide_16(&self) -> Vec<u16> { self.as_str().to_wide_16() }
    fn to_wide_16_unpadded(&self) -> Vec<u16> { self.as_str().to_wide_16_unpadded() }
    fn to_wide(&self) -> Vec<u8> { self.as_str().to_wide() }
}

// ─── GUIDExt ─────────────────────────────────────────────────────────────────

pub trait GUIDExt {
    fn to_guid_string(&self) -> String;
}

impl GUIDExt for GUID {
    fn to_guid_string(&self) -> String {
        format!(
            "{{{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
            self.data1, self.data2, self.data3,
            self.data4[0], self.data4[1],
            self.data4[2], self.data4[3], self.data4[4],
            self.data4[5], self.data4[6], self.data4[7],
        )
    }
}

// ─── RegKey ──────────────────────────────────────────────────────────────────

pub trait RegKey {
    fn create_subkey(&self, subkey: &str) -> windows::core::Result<HKEY>;
    fn set_string(&self, value_name: &str, value: &str) -> windows::core::Result<()>;
    #[allow(dead_code)]
    fn set_dword(&self, value_name: &str, value: u32) -> windows::core::Result<()>;
    fn delete_tree(&self, subkey: &str) -> windows::core::Result<()>;
    fn close(&self) -> windows::core::Result<()>;
}

impl RegKey for HKEY {
    fn create_subkey(&self, subkey_name: &str) -> windows::core::Result<HKEY> {
        let name = HSTRING::from(subkey_name);
        let mut out: HKEY = HKEY::default();
        unsafe {
            RegCreateKeyExW(
                *self,
                PCWSTR(name.as_ptr()),
                0,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut out,
                None,
            ).ok()?;
        }
        Ok(out)
    }

    fn set_string(&self, value_name: &str, value: &str) -> windows::core::Result<()> {
        let name  = HSTRING::from(value_name);
        let bytes = value.to_wide();
        unsafe {
            RegSetValueExW(
                *self,
                PCWSTR(name.as_ptr()),
                0,
                REG_SZ,
                Some(bytes.as_slice()),
            ).ok()
        }
    }

    fn delete_tree(&self, subkey: &str) -> windows::core::Result<()> {
        let sub = HSTRING::from(subkey);
        unsafe { RegDeleteTreeW(*self, PCWSTR(sub.as_ptr())).ok() }
    }

    fn set_dword(&self, value_name: &str, value: u32) -> windows::core::Result<()> {
        use windows::Win32::System::Registry::{RegSetValueExW, REG_DWORD};
        use windows::core::PCWSTR;
        let name_wide: Vec<u16> = value_name.encode_utf16().chain(Some(0)).collect();
        unsafe {
            RegSetValueExW(
                *self,
                PCWSTR(name_wide.as_ptr()),
                0,
                REG_DWORD,
                Some(&value.to_le_bytes()),
            ).ok()
        }
    }

    fn close(&self) -> windows::core::Result<()> {
        unsafe { RegCloseKey(*self).ok() }
    }
}

// ─── VKeyExt ─────────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub trait VKeyExt {
    fn is_pressed(self) -> bool;
}

impl VKeyExt for VIRTUAL_KEY {
    fn is_pressed(self) -> bool {
        unsafe { GetKeyState(self.0 as i32) as u16 & 0x8000 != 0 }
    }
}
