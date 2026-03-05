#![allow(non_snake_case, clippy::missing_safety_doc, unsafe_op_in_unsafe_fn)]

#[macro_use]
mod macros;
pub mod diagnostics;

mod engine;
mod extension;
mod globals;
mod tsf;

use globals::{DllModule, DLL_INSTANCE, GUID_TEXT_SERVICE};
use std::ffi::c_void;
use windows::{
    core::{GUID, IUnknown, Interface},
    Win32::{
        Foundation::{BOOL, E_FAIL, HINSTANCE, S_FALSE, S_OK, TRUE},
        System::Com::IClassFactory,
    },
};

#[allow(overflowing_literals)]
const CLASS_E_CLASSNOTAVAILABLE: windows::core::HRESULT =
    windows::core::HRESULT(0x80040111u32 as i32);

#[unsafe(no_mangle)]
pub extern "system" fn DllMain(hinst: HINSTANCE, reason: u32, _: *mut c_void) -> BOOL {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        DLL_INSTANCE.get_or_init(|| {
            std::sync::Mutex::new(DllModule {
                ref_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                hinst: Some(hinst.into()),
            })
        });
        // ログをファイルに出力（デバッグ用）
        let log_path = std::env::var("LOCALAPPDATA")
            .map(|p| format!("{}\\rakukan\\rakukan.log", p))
            .unwrap_or_default();
        if !log_path.is_empty() {
            if let Ok(f) = std::fs::OpenOptions::new()
                .create(true).append(true).open(&log_path)
            {
                let _ = tracing_subscriber::fmt()
                    .with_env_filter(
                        tracing_subscriber::EnvFilter::try_from_env("RAKUKAN_LOG")
                            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("rakukan_tsf=info")),
                    )
                    .with_writer(std::sync::Mutex::new(f))
                    .try_init();
            }
        } else {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_env("RAKUKAN_LOG")
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("rakukan=info")),
                )
                .try_init();
        }
        tracing::info!("rakukan TSF DLL loaded");
        let _ = crate::engine::keymap::keymap_save_default();
        crate::engine::state::start_reload_watcher();
    }
    TRUE
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    rclsid: *const GUID,
    riid: *const GUID,
    ppv: *mut *mut c_void,
) -> windows::core::HRESULT {
    if ppv.is_null() { return E_FAIL; }
    *ppv = std::ptr::null_mut();
    if *rclsid != GUID_TEXT_SERVICE {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = tsf::factory::ClassFactory::create();
    let unk: IUnknown = match factory.cast() {
        Ok(u) => u,
        Err(e) => return e.code(),
    };
    unk.query(riid, ppv)
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllCanUnloadNow() -> windows::core::HRESULT {
    match DllModule::get() {
        Ok(m) if m.ref_count.load(std::sync::atomic::Ordering::SeqCst) == 0 => S_OK,
        _ => S_FALSE,
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllRegisterServer() -> windows::core::HRESULT {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

    // デバッグ: 各ステップを個別に実行してエラー箇所を特定
    let log_path = format!("{}\\rakukan\\register_debug.log",
        std::env::var("LOCALAPPDATA").unwrap_or_default());
    let mut log = String::new();

    macro_rules! step {
        ($label:expr, $expr:expr) => {{
            match $expr {
                Ok(v) => { log.push_str(&format!("OK: {}
", $label)); v }
                Err(e) => {
                    let msg = format!("FAIL: {} — {}
", $label, e);
                    log.push_str(&msg);
                    let _ = std::fs::write(&log_path, &log);
                    CoUninitialize();
                    return E_FAIL;
                }
            }
        }};
    }

    log.push_str("DllRegisterServer start
");

    let dll_path = step!("get_path", crate::globals::DllModule::get_path());
    log.push_str(&format!("dll_path: {dll_path}
"));

    step!("clsid_register", tsf::registration::clsid_register(&dll_path));
    step!("profile_register", tsf::registration::profile_register(&dll_path));
    step!("category_register", tsf::registration::category_register());

    log.push_str("DllRegisterServer success
");
    let _ = std::fs::write(&log_path, &log);

    CoUninitialize();
    S_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllUnregisterServer() -> windows::core::HRESULT {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let r = match tsf::registration::unregister_server() {
        Ok(_) => S_OK,
        Err(e) => { tracing::error!("DllUnregisterServer: {e}"); E_FAIL }
    };
    CoUninitialize();
    r
}
