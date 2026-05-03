#![allow(non_snake_case, clippy::missing_safety_doc, unsafe_op_in_unsafe_fn)]

#[macro_use]
mod macros;
pub mod diagnostics;

mod engine;
mod extension;
mod globals;
mod tsf;

use globals::{DLL_INSTANCE, DllModule, GUID_TEXT_SERVICE};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use windows::{
    Win32::{
        Foundation::{BOOL, E_FAIL, HINSTANCE, S_FALSE, S_OK, TRUE},
        System::Com::IClassFactory,
    },
    core::{GUID, IUnknown, Interface},
};

#[allow(overflowing_literals)]
const CLASS_E_CLASSNOTAVAILABLE: windows::core::HRESULT =
    windows::core::HRESULT(0x80040111u32 as i32);

const LOG_ROTATE_MAX_BYTES: u64 = 16 * 1024 * 1024;
const LOG_ROTATE_GENERATIONS: usize = 5;

fn rotated_log_path(path: &Path, generation: usize) -> Option<PathBuf> {
    let mut file_name = path.file_name()?.to_os_string();
    file_name.push(format!(".{generation}"));
    Some(path.with_file_name(file_name))
}

fn rotate_log_if_needed(path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        return;
    };
    if meta.len() <= LOG_ROTATE_MAX_BYTES {
        return;
    }

    for generation in (1..=LOG_ROTATE_GENERATIONS).rev() {
        let Some(dst) = rotated_log_path(path, generation) else {
            return;
        };
        if generation == LOG_ROTATE_GENERATIONS {
            let _ = std::fs::remove_file(&dst);
        }
        let src = if generation == 1 {
            path.to_path_buf()
        } else {
            let Some(src) = rotated_log_path(path, generation - 1) else {
                return;
            };
            src
        };
        if src.exists() {
            let _ = std::fs::rename(src, dst);
        }
    }
}

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

        // config.toml の log_level を読む（ファイルが存在する場合）。
        // subscriber 初期化は config より先に行う必要があるため、
        // ここで直接ファイルを読み込む（init_config_manager より前）。
        let config_log_level = {
            let config_path = std::env::var("APPDATA")
                .ok()
                .map(|p| format!("{}\\rakukan\\config.toml", p));
            config_path
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|s| {
                    s.lines()
                        .find(|l| l.trim_start().starts_with("log_level"))
                        .and_then(|l| l.split('=').nth(1))
                        .map(|v| v.trim().trim_matches('"').to_string())
                })
                .unwrap_or_else(|| "debug".to_string())
        };

        let make_filter = |scope: &str| {
            tracing_subscriber::EnvFilter::try_from_env("RAKUKAN_LOG").unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(format!("{}={}", scope, config_log_level))
            })
        };

        if !log_path.is_empty() {
            rotate_log_if_needed(Path::new(&log_path));
            if let Ok(f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
            {
                let _ = tracing_subscriber::fmt()
                    .compact()
                    .with_env_filter(make_filter("rakukan_tsf"))
                    .with_ansi(false)
                    .with_writer(std::sync::Mutex::new(f))
                    .try_init();
            }
        } else {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(make_filter("rakukan"))
                .try_init();
        }
        tracing::info!(
            "rakukan TSF DLL loaded  build={}",
            option_env!("RAKUKAN_BUILD_TIME").unwrap_or("unknown")
        );
        let _ = crate::engine::config::config_save_default();
        crate::engine::config::init_config_manager();
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
    if ppv.is_null() {
        return E_FAIL;
    }
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
    // 常に S_FALSE を返し、TSF DLL を unload させない。
    //
    // 背景: 2026-04-22 の Explorer crash 解析で、DllCanUnloadNow=S_OK 後の
    // FreeLibrary と、in-flight な WM_TIMER / WM_PAINT 等のメッセージが衝突し、
    // unload 済みアドレスにある wnd_proc / RegisterClassW 登録ポインタへ
    // ディスパッチされて AV (BAD_INSTRUCTION_PTR_c0000005_rakukan_tsf.dll!Unloaded)
    // を起こしていた。
    //
    // RegisterClassW は UnregisterClassW を呼ばない限り wnd_proc ポインタを
    // 内部に保持し続けるため、DLL unload と完全に整合させるのが困難。
    // 常駐させる方が安全（メモリコストはプロセス毎に ~2 MB 程度で実用上無視できる）。
    // Microsoft 標準 IME も同パターン。
    S_FALSE
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllRegisterServer() -> windows::core::HRESULT {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

    // デバッグ: 各ステップを個別に実行してエラー箇所を特定
    let log_path = format!(
        "{}\\rakukan\\register_debug.log",
        std::env::var("LOCALAPPDATA").unwrap_or_default()
    );
    let mut log = String::new();

    macro_rules! step {
        ($label:expr, $expr:expr) => {{
            match $expr {
                Ok(v) => {
                    log.push_str(&format!(
                        "OK: {}
",
                        $label
                    ));
                    v
                }
                Err(e) => {
                    let msg = format!(
                        "FAIL: {} — {}
",
                        $label, e
                    );
                    log.push_str(&msg);
                    let _ = std::fs::write(&log_path, &log);
                    CoUninitialize();
                    return E_FAIL;
                }
            }
        }};
    }

    log.push_str(
        "DllRegisterServer start
",
    );

    let dll_path = step!("get_path", crate::globals::DllModule::get_path());
    log.push_str(&format!(
        "dll_path: {dll_path}
"
    ));

    step!(
        "clsid_register",
        tsf::registration::clsid_register(&dll_path)
    );
    step!(
        "profile_register",
        tsf::registration::profile_register(&dll_path)
    );
    step!("category_register", tsf::registration::category_register());

    log.push_str(
        "DllRegisterServer success
",
    );
    let _ = std::fs::write(&log_path, &log);

    CoUninitialize();
    S_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllUnregisterServer() -> windows::core::HRESULT {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let r = match tsf::registration::unregister_server() {
        Ok(_) => S_OK,
        Err(e) => {
            tracing::error!("DllUnregisterServer: {e}");
            E_FAIL
        }
    };
    CoUninitialize();
    r
}
