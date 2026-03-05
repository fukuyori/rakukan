//! DLL registration / unregistration
//!
//! azooKey-Windows の register.rs に準拠した実装。

use crate::{
    extension::{GUIDExt, RegKey},
    globals::{DllModule, GUID_PROFILE, GUID_TEXT_SERVICE, SERVICE_NAME},
};
use anyhow::Result;
use windows::{
    core::Interface,
    Win32::{
        System::{
            Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
            Registry::HKEY_CLASSES_ROOT,
        },
        UI::{
            Input::KeyboardAndMouse::HKL,
            TextServices::{
                CLSID_TF_CategoryMgr, CLSID_TF_InputProcessorProfiles,
                ITfCategoryMgr, ITfInputProcessorProfileMgr, ITfInputProcessorProfiles,
                GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
                GUID_TFCAT_TIPCAP_COMLESS,
                GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
                GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
                GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
                GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
                GUID_TFCAT_TIP_KEYBOARD,
            },
        },
    },
};

/// 日本語 LANGID（固定値）
const LANG_JAPANESE: u16 = 0x0411;

// ─── 登録 ────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
pub fn register_server() -> Result<()> {
    let dll_path = DllModule::get_path()?;
    tracing::info!("register_server: dll_path={dll_path}");

    clsid_register(&dll_path)?;
    tracing::info!("register_server: CLSID OK");

    profile_register(&dll_path)?;
    tracing::info!("register_server: Profile OK");

    category_register()?;
    tracing::info!("register_server: Category OK");

    Ok(())
}

/// HKCR\CLSID\{...}\InProcServer32
pub fn clsid_register(dll_path: &str) -> Result<()> {
    let clsid_str = GUID_TEXT_SERVICE.to_guid_string();

    let clsid_key = HKEY_CLASSES_ROOT
        .create_subkey(&format!("CLSID\\{}", clsid_str))
        .map_err(|e| anyhow::anyhow!("create CLSID key: {e}"))?;
    clsid_key.set_string("", SERVICE_NAME)?;
    clsid_key.close()?;

    let inproc_key = HKEY_CLASSES_ROOT
        .create_subkey(&format!("CLSID\\{}\\InProcServer32", clsid_str))
        .map_err(|e| anyhow::anyhow!("create InProcServer32: {e}"))?;
    inproc_key.set_string("", dll_path)?;
    inproc_key.set_string("ThreadingModel", "Apartment")?;
    inproc_key.close()?;

    Ok(())
}

/// ITfInputProcessorProfiles::Register + ITfInputProcessorProfileMgr::RegisterProfile
pub fn profile_register(dll_path: &str) -> Result<()> {
    let desc: Vec<u16> = SERVICE_NAME.encode_utf16().chain(Some(0)).collect();
    let icon: Vec<u16> = dll_path.encode_utf16().chain(Some(0)).collect();

    unsafe {
        // ITfInputProcessorProfiles を取得
        let profiles: ITfInputProcessorProfiles = CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        ).map_err(|e| anyhow::anyhow!("CoCreateInstance Profiles: {e}"))?;

        // CLSID を TSF に登録（RegisterProfile の前に必要）
        profiles.Register(&GUID_TEXT_SERVICE)
            .map_err(|e| anyhow::anyhow!("Register: {e}"))?;

        // ITfInputProcessorProfileMgr にキャストして RegisterProfile
        let mgr: ITfInputProcessorProfileMgr = Interface::cast(&profiles)
            .map_err(|e| anyhow::anyhow!("cast ProfileMgr: {e}"))?;

        mgr.RegisterProfile(
            &GUID_TEXT_SERVICE,
            LANG_JAPANESE,
            &GUID_PROFILE,
            &desc,
            &icon,
            0,
            HKL(std::ptr::null_mut()),
            0,
            true,   // bEnabledByDefault
            0,
        ).map_err(|e| anyhow::anyhow!("RegisterProfile: {e}"))?;
    }

    Ok(())
}

/// ITfCategoryMgr::RegisterCategory
/// GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT がないとトレイに表示されない
pub fn category_register() -> Result<()> {
    const CATEGORIES: &[windows::core::GUID] = &[
        GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
        GUID_TFCAT_TIPCAP_COMLESS,
        GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
        GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
        GUID_TFCAT_TIP_KEYBOARD,
        GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
        GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
    ];

    unsafe {
        let catmgr: ITfCategoryMgr = CoCreateInstance(
            &CLSID_TF_CategoryMgr,
            None,
            CLSCTX_INPROC_SERVER,
        ).map_err(|e| anyhow::anyhow!("CoCreateInstance CategoryMgr: {e}"))?;

        for cat in CATEGORIES {
            catmgr.RegisterCategory(&GUID_TEXT_SERVICE, cat, &GUID_TEXT_SERVICE)
                .map_err(|e| anyhow::anyhow!("RegisterCategory: {e}"))?;
        }
    }

    Ok(())
}

// ─── 削除 ────────────────────────────────────────────────────────────────────

pub fn unregister_server() -> Result<()> {
    let _ = category_unregister();
    let _ = profile_unregister();
    let _ = clsid_unregister();
    tracing::info!("Unregistered");
    Ok(())
}

fn clsid_unregister() -> Result<()> {
    let clsid_str = GUID_TEXT_SERVICE.to_guid_string();
    let _ = HKEY_CLASSES_ROOT.delete_tree(&format!("CLSID\\{}", clsid_str));
    Ok(())
}

fn profile_unregister() -> Result<()> {
    unsafe {
        let profiles: ITfInputProcessorProfiles = CoCreateInstance(
            &CLSID_TF_InputProcessorProfiles,
            None,
            CLSCTX_INPROC_SERVER,
        ).map_err(|e| anyhow::anyhow!("{e}"))?;

        // モダン API
        if let Ok(mgr) = Interface::cast::<ITfInputProcessorProfileMgr>(&profiles) {
            let _ = mgr.UnregisterProfile(&GUID_TEXT_SERVICE, LANG_JAPANESE, &GUID_PROFILE, 0);
        }
        // 旧 API
        let _ = profiles.Unregister(&GUID_TEXT_SERVICE);
    }
    Ok(())
}

fn category_unregister() -> Result<()> {
    const CATEGORIES: &[windows::core::GUID] = &[
        GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER,
        GUID_TFCAT_TIPCAP_COMLESS,
        GUID_TFCAT_TIPCAP_INPUTMODECOMPARTMENT,
        GUID_TFCAT_TIPCAP_UIELEMENTENABLED,
        GUID_TFCAT_TIP_KEYBOARD,
        GUID_TFCAT_TIPCAP_IMMERSIVESUPPORT,
        GUID_TFCAT_TIPCAP_SYSTRAYSUPPORT,
    ];

    unsafe {
        if let Ok(catmgr) = CoCreateInstance::<_, ITfCategoryMgr>(
            &CLSID_TF_CategoryMgr, None, CLSCTX_INPROC_SERVER,
        ) {
            for cat in CATEGORIES {
                let _ = catmgr.UnregisterCategory(&GUID_TEXT_SERVICE, cat, &GUID_TEXT_SERVICE);
            }
        }
    }
    Ok(())
}
