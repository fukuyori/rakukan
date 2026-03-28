//! TSF Display Attribute 実装
//!
//! プリエディット（確定前文字）にアンダーラインを表示するための
//! `ITfDisplayAttributeInfo` / `IEnumTfDisplayAttributeInfo` 実装。
//!
//! アンダーラインの種類:
//!   - 未変換プリエディット : 点線 (TF_LS_DOT)   → GUID_DISPLAY_ATTRIBUTE_INPUT
//!   - 選択中候補           : 実線 (TF_LS_SOLID)  → GUID_DISPLAY_ATTRIBUTE (CONVERTED)

use std::{
    cell::RefCell,
    sync::atomic::{AtomicU32, Ordering},
};

use windows::{
    Win32::UI::TextServices::{
        IEnumTfDisplayAttributeInfo, IEnumTfDisplayAttributeInfo_Impl, ITfDisplayAttributeInfo,
        ITfDisplayAttributeInfo_Impl, TF_DISPLAYATTRIBUTE,
    },
    core::{BSTR, GUID, implement},
};

use crate::globals::{
    DISPLAY_ATTRIBUTE_CONVERTED, DISPLAY_ATTRIBUTE_INPUT, GUID_DISPLAY_ATTRIBUTE,
    GUID_DISPLAY_ATTRIBUTE_INPUT,
};

// ─── GuidAtom キャッシュ ──────────────────────────────────────────────────────

/// 0 = 未登録（TF_INVALID_GUIDATOM）
static ATOM_INPUT: AtomicU32 = AtomicU32::new(0);
static ATOM_CONVERTED: AtomicU32 = AtomicU32::new(0);

pub fn set_atoms(input: u32, converted: u32) {
    ATOM_INPUT.store(input, Ordering::Relaxed);
    ATOM_CONVERTED.store(converted, Ordering::Relaxed);
}

/// 未変換プリエディット用の atom（点線）
pub fn atom_input() -> u32 {
    ATOM_INPUT.load(Ordering::Relaxed)
}
/// 選択中候補用の atom（太実線）
pub fn atom_converted() -> u32 {
    ATOM_CONVERTED.load(Ordering::Relaxed)
}

// ─── ITfDisplayAttributeInfo 実装 ────────────────────────────────────────────

#[implement(ITfDisplayAttributeInfo)]
pub struct DisplayAttrInfo {
    guid: GUID,
    attr: TF_DISPLAYATTRIBUTE,
    desc: &'static str,
}

impl DisplayAttrInfo {
    fn new(guid: GUID, attr: TF_DISPLAYATTRIBUTE, desc: &'static str) -> ITfDisplayAttributeInfo {
        Self { guid, attr, desc }.into()
    }
}

impl ITfDisplayAttributeInfo_Impl for DisplayAttrInfo_Impl {
    fn GetGUID(&self) -> windows::core::Result<GUID> {
        Ok(self.guid)
    }

    fn GetDescription(&self) -> windows::core::Result<BSTR> {
        Ok(BSTR::from(self.desc))
    }

    fn GetAttributeInfo(&self, pda: *mut TF_DISPLAYATTRIBUTE) -> windows::core::Result<()> {
        if pda.is_null() {
            return Ok(());
        }
        unsafe {
            *pda = self.attr;
        }
        Ok(())
    }

    fn SetAttributeInfo(&self, _pda: *const TF_DISPLAYATTRIBUTE) -> windows::core::Result<()> {
        // read-only — アプリによる上書きは無視
        Ok(())
    }

    fn Reset(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

// ─── IEnumTfDisplayAttributeInfo 実装 ────────────────────────────────────────

#[implement(IEnumTfDisplayAttributeInfo)]
pub struct EnumDisplayAttrInfo {
    items: Vec<ITfDisplayAttributeInfo>,
    pos: RefCell<usize>,
}

unsafe impl Send for EnumDisplayAttrInfo {}
unsafe impl Sync for EnumDisplayAttrInfo {}

impl EnumDisplayAttrInfo {
    pub fn new(items: Vec<ITfDisplayAttributeInfo>) -> IEnumTfDisplayAttributeInfo {
        Self {
            items,
            pos: RefCell::new(0),
        }
        .into()
    }
}

impl IEnumTfDisplayAttributeInfo_Impl for EnumDisplayAttrInfo_Impl {
    fn Next(
        &self,
        celt: u32,
        rgelt: *mut Option<ITfDisplayAttributeInfo>,
        pcelt_fetched: *mut u32,
    ) -> windows::core::Result<()> {
        use windows::Win32::Foundation::S_FALSE;
        let pos = *self.pos.borrow();
        let available = self.items.len().saturating_sub(pos);
        let fetched = (celt as usize).min(available);
        unsafe {
            for i in 0..fetched {
                *rgelt.add(i) = Some(self.items[pos + i].clone());
            }
            if !pcelt_fetched.is_null() {
                *pcelt_fetched = fetched as u32;
            }
        }
        *self.pos.borrow_mut() = pos + fetched;
        if fetched == celt as usize {
            Ok(())
        } else {
            Err(windows::core::Error::from(S_FALSE))
        }
    }

    fn Skip(&self, celt: u32) -> windows::core::Result<()> {
        let mut pos = self.pos.borrow_mut();
        *pos = (*pos + celt as usize).min(self.items.len());
        Ok(())
    }

    fn Reset(&self) -> windows::core::Result<()> {
        *self.pos.borrow_mut() = 0;
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IEnumTfDisplayAttributeInfo> {
        let cloned = EnumDisplayAttrInfo {
            items: self.items.clone(),
            pos: RefCell::new(*self.pos.borrow()),
        };
        Ok(cloned.into())
    }
}

// ─── ヘルパー ─────────────────────────────────────────────────────────────────

/// 全 DisplayAttributeInfo を列挙する
pub fn make_all() -> Vec<ITfDisplayAttributeInfo> {
    vec![
        DisplayAttrInfo::new(
            GUID_DISPLAY_ATTRIBUTE_INPUT,
            DISPLAY_ATTRIBUTE_INPUT,
            "Rakukan Input",
        ),
        DisplayAttrInfo::new(
            GUID_DISPLAY_ATTRIBUTE,
            DISPLAY_ATTRIBUTE_CONVERTED,
            "Rakukan Converted",
        ),
    ]
}

/// GUID から対応する ITfDisplayAttributeInfo を返す
pub fn get_by_guid(guid: &GUID) -> windows::core::Result<ITfDisplayAttributeInfo> {
    use windows::Win32::Foundation::E_INVALIDARG;
    if *guid == GUID_DISPLAY_ATTRIBUTE_INPUT {
        return Ok(DisplayAttrInfo::new(
            GUID_DISPLAY_ATTRIBUTE_INPUT,
            DISPLAY_ATTRIBUTE_INPUT,
            "Rakukan Input",
        ));
    }
    if *guid == GUID_DISPLAY_ATTRIBUTE {
        return Ok(DisplayAttrInfo::new(
            GUID_DISPLAY_ATTRIBUTE,
            DISPLAY_ATTRIBUTE_CONVERTED,
            "Rakukan Converted",
        ));
    }
    Err(windows::core::Error::from(E_INVALIDARG))
}
