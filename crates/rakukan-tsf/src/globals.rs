use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex, MutexGuard, OnceLock,
};

use anyhow::{Context, Result};
use windows::{
    core::GUID,
    Win32::{
        Foundation::{HMODULE, MAX_PATH},
        System::LibraryLoader::GetModuleFileNameW,
        UI::TextServices::{
            TF_ATTR_INPUT, TF_ATTR_TARGET_CONVERTED, TF_CT_NONE,
            TF_DA_COLOR, TF_DA_COLOR_0, TF_DISPLAYATTRIBUTE,
            TF_LS_DOT, TF_LS_SOLID,
        },
    },
};



#[allow(dead_code)]
pub const CLSID_PREFIX: &str = "CLSID\\";
#[allow(dead_code)]
pub const INPROC_SUFFIX: &str = "\\InProcServer32";
pub const SERVICE_NAME: &str = "Rakukan";

// rakukan unique GUIDs
pub const GUID_TEXT_SERVICE: GUID = GUID::from_u128(0xc0ddf8b0_1f1e_4c2d_a9e3_5f7b8d6e2a4c);
pub const GUID_PROFILE:      GUID = GUID::from_u128(0xc0ddf8b1_1f1e_4c2d_a9e3_5f7b8d6e2a4c);
/// 選択中候補（変換確定待ち）のアンダーライン属性 GUID
pub const GUID_DISPLAY_ATTRIBUTE: GUID = GUID::from_u128(0xc0ddf8b2_1f1e_4c2d_a9e3_5f7b8d6e2a4c);
/// 未変換プリエディットのアンダーライン属性 GUID
pub const GUID_DISPLAY_ATTRIBUTE_INPUT: GUID = GUID::from_u128(0xc0ddf8b3_1f1e_4c2d_a9e3_5f7b8d6e2a4c);


#[allow(dead_code)]
pub const TEXTSERVICE_LANGBARITEMSINK_COOKIE: u32 = 0x414D414B;

/// 選択中候補（変換確定待ち）: 実線アンダーライン
pub const DISPLAY_ATTRIBUTE_CONVERTED: TF_DISPLAYATTRIBUTE = TF_DISPLAYATTRIBUTE {
    crText: TF_DA_COLOR { r#type: TF_CT_NONE, Anonymous: TF_DA_COLOR_0 { nIndex: 0i32 } },
    crBk:   TF_DA_COLOR { r#type: TF_CT_NONE, Anonymous: TF_DA_COLOR_0 { nIndex: 0i32 } },
    lsStyle: TF_LS_SOLID,
    fBoldLine: windows::Win32::Foundation::TRUE,
    crLine: TF_DA_COLOR { r#type: TF_CT_NONE, Anonymous: TF_DA_COLOR_0 { nIndex: 0i32 } },
    bAttr: TF_ATTR_TARGET_CONVERTED,
};

/// 未変換プリエディット: 点線アンダーライン
pub const DISPLAY_ATTRIBUTE_INPUT: TF_DISPLAYATTRIBUTE = TF_DISPLAYATTRIBUTE {
    crText: TF_DA_COLOR { r#type: TF_CT_NONE, Anonymous: TF_DA_COLOR_0 { nIndex: 0i32 } },
    crBk:   TF_DA_COLOR { r#type: TF_CT_NONE, Anonymous: TF_DA_COLOR_0 { nIndex: 0i32 } },
    lsStyle: TF_LS_DOT,
    fBoldLine: windows::Win32::Foundation::FALSE,
    crLine: TF_DA_COLOR { r#type: TF_CT_NONE, Anonymous: TF_DA_COLOR_0 { nIndex: 0i32 } },
    bAttr: TF_ATTR_INPUT,
};



// ─── DLL instance ────────────────────────────────────────────────────────────

pub static DLL_INSTANCE: OnceLock<Mutex<DllModule>> = OnceLock::new();

unsafe impl Sync for DllModule {}
unsafe impl Send for DllModule {}

#[derive(Debug)]
pub struct DllModule {
    pub ref_count: Arc<AtomicUsize>,
    pub hinst: Option<HMODULE>,
}

#[allow(dead_code)]
impl DllModule {
    pub fn new() -> Self {
        Self {
            ref_count: Arc::new(AtomicUsize::new(0)),
            hinst: None,
        }
    }

    pub fn get() -> Result<MutexGuard<'static, DllModule>> {
        DLL_INSTANCE
            .get()
            .ok_or_else(|| anyhow::anyhow!("DllModule not initialized"))?
            .lock()
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    pub fn get_path() -> Result<String> {
        let hinst = DllModule::get()?.hinst;
        let mut buf: [u16; MAX_PATH as usize] = [0; MAX_PATH as usize];
        let len = unsafe {
            GetModuleFileNameW(
                hinst.context("DLL instance not found")?,
                &mut buf,
            )
        };
        Ok(String::from_utf16_lossy(&buf[..len as usize]))
    }

    pub fn add_ref(&mut self) -> usize {
        self.ref_count.fetch_add(1, Ordering::SeqCst)
    }

    pub fn release(&mut self) -> usize {
        self.ref_count.fetch_sub(1, Ordering::SeqCst)
    }
}
