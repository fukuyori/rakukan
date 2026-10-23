#![allow(non_snake_case, clippy::missing_safety_doc, unsafe_op_in_unsafe_fn)]
// cdylib のリンク時に link.exe が stdout へ出す情報行（「ライブラリ ... を作成中」=
// インポートライブラリ作成の通知）を rustc の linker_messages lint が warning として
// 表示する。リンカ側では抑止できず、動作にも影響しないため crate 単位で allow する。
// 実際のリンカ警告（COM エントリポイントの LNK4104）は build.rs の /IGNORE:4104 で
// 個別に抑止しており、リンクエラーは従来どおりビルド失敗になる。
#![allow(linker_messages)]

#[macro_use]
mod macros;
pub mod diagnostics;

mod engine;
mod extension;
mod globals;
mod tsf;

use globals::{DLL_INSTANCE, DllModule, GUID_TEXT_SERVICE};
use std::ffi::c_void;
use std::path::PathBuf;
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

/// TSF DLL の tracing で記録する target。
///
/// `EnvFilter` は指定した target 以外を捨てるため、同じ DLL に静的リンクされている
/// RPC クライアント（`rakukan_engine_rpc`）もここに並べないと、`rpc SLOW` や
/// `spawn_host failed` が `rakukan.log` に記録されない（Issue #54）。
const LOG_TARGETS: &[&str] = &["rakukan_tsf", "rakukan_engine_rpc"];

/// `RAKUKAN_LOG` が無いときの filter 指定（`rakukan_tsf=debug,rakukan_engine_rpc=debug` の形）。
fn default_log_filter_spec(level: &str) -> String {
    LOG_TARGETS
        .iter()
        .map(|target| format!("{target}={level}"))
        .collect::<Vec<_>>()
        .join(",")
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
        // ここでは何も起動しない・何も作らない（Issue #72）。
        // regsvr32 は DllRegisterServer の直後に FreeLibrary するので、ここで起動した
        // スレッドが外された DLL / CRT のコードを実行して落ちていた。ログ・設定・
        // 設定の監視は、最初の Activate で `runtime_init_once()` が行う。
    }
    TRUE
}

/// 実行時の初期化が済んだか（最初の Activate の後なら `true`）。
pub(crate) fn runtime_ready() -> bool {
    RUNTIME_READY.load(std::sync::atomic::Ordering::Acquire)
}

static RUNTIME_READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RUNTIME_INIT: std::sync::Once = std::sync::Once::new();

/// 実行時の初期化（Issue #72）。最初の有効な Activate から、プロセスにつき 1 回。
///
/// 順序: DLL の固定 → ログ → 既定の設定ファイル → 設定の読み込み → 既定の keymap →
/// 設定の監視。sink の登録・UI の操作・エンジンの起動はしない（再入を避ける）。
/// どれかに失敗しても入力は止めない。
pub(crate) fn runtime_init_once() {
    RUNTIME_INIT.call_once(|| {
        pin_dll();
        init_logging();
        let _ = crate::engine::config::config_save_default();
        crate::engine::config::init_config_manager();
        crate::engine::config::log_current_config("runtime_init");
        let _ = crate::engine::keymap::keymap_save_default();
        crate::engine::state::start_reload_watcher();
        RUNTIME_READY.store(true, std::sync::atomic::Ordering::Release);
    });
}

/// この DLL をプロセスが終わるまで外させない（Issue #72）。
///
/// Activate の後は設定の監視などのスレッドが DLL のコードを実行し続ける。
/// `DllCanUnloadNow` の `S_FALSE` は明示的な `FreeLibrary` を止めないので、
/// `GetModuleHandleExW(PIN)` で参照を固定する。登録・解除だけの読み込みでは呼ばれない。
fn pin_dll() {
    use windows::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW,
    };
    let mut module = windows::Win32::Foundation::HMODULE::default();
    let anchor = pin_dll as *const ();
    let pinned = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN,
            windows::core::PCWSTR(anchor as *const u16),
            &mut module,
        )
    };
    // ログの初期化より前なので、結果は初期化の後に記録する
    PIN_RESULT.get_or_init(|| pinned.map_err(|e| e.to_string()));
}

static PIN_RESULT: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();

/// ログの初期化。プロセス間で共有せず、起動インスタンスごとのファイルに書く（Issue #60）。
///
/// 書き手は 1 つに限る: `.lock` を排他で取れたときだけ書く（#73）。書き込みは専用のスレッドで
/// 行い、キー処理はキューに入れるだけにする（#76）。どちらかができなければログなしで続ける。
fn init_logging() {
    {
        let log_dir = std::env::var("LOCALAPPDATA")
            .map(|p| PathBuf::from(format!("{p}\\rakukan")))
            .unwrap_or_default();
        let build = option_env!("RAKUKAN_BUILD_TIME").unwrap_or("unknown");

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

        // `RAKUKAN_LOG` を明示した場合はそちらを優先する。
        let make_filter = || {
            tracing_subscriber::EnvFilter::try_from_env("RAKUKAN_LOG").unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(default_log_filter_spec(&config_log_level))
            })
        };

        let writer = if log_dir.as_os_str().is_empty() {
            None
        } else {
            rakukan_log::file::acquire_instance(
                rakukan_log::file::TSF_PREFIX,
                &log_dir,
                std::process::id(),
            )
            .and_then(|instance| {
                let header = rakukan_log::header::header_line("tsf", &instance.stem, build);
                let file = rakukan_log::file::RotatingLog::open(instance.log_path, Some(header));
                // ロックは書き込み用のスレッドが持ち、プロセスが終わるまで離さない
                rakukan_log::AsyncLog::spawn("rakukan-log-writer", file, instance.lock)
                    .ok()
                    .map(|log| (instance.stem, log))
            })
        };
        let Some((log_stem, log)) = writer else {
            // ロックを取れない・スレッドを起動できない: ログなしで続ける
            return;
        };
        let init = tracing_subscriber::fmt()
            .compact()
            .with_env_filter(make_filter())
            .with_ansi(false)
            .with_thread_ids(true)
            .with_writer(log)
            .try_init();
        // 失敗するのは別の subscriber が既に設定されている場合なので、そちらへ記録される
        if let Err(e) = init {
            tracing::warn!("tracing subscriber was not installed: {e}");
        }
        tracing::info!(
            "rakukan TSF DLL loaded  build={} log={} app={}",
            build,
            log_stem,
            crate::engine::state::current_exe_name_lower()
        );
        match PIN_RESULT.get() {
            Some(Ok(())) => tracing::debug!("dll pinned for the life of the process"),
            Some(Err(e)) => tracing::warn!("dll pin failed: {e}"),
            None => {}
        }
        // 古いログの掃除は DLL の中では行わない（Issue #72）。トレイへ移す（#74）。
    }
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

/// 登録・解除の記録（`%LOCALAPPDATA%\rakukan\register_debug.log`、Issue #72）。
///
/// 実行時のログ（tracing）は最初の Activate まで初期化しないので、regsvr32 による
/// 登録・解除は tracing に頼らず、このファイルへ各段階をその場で追記する。
/// 1 MiB を超えたら作り直す。
struct RegisterLog {
    path: Option<PathBuf>,
}

impl RegisterLog {
    const MAX_BYTES: u64 = 1024 * 1024;

    fn open() -> Self {
        let path = std::env::var("LOCALAPPDATA").ok().map(|p| {
            let dir = PathBuf::from(p).join("rakukan");
            let _ = std::fs::create_dir_all(&dir);
            dir.join("register_debug.log")
        });
        if let Some(p) = &path
            && std::fs::metadata(p).is_ok_and(|m| m.len() > Self::MAX_BYTES)
        {
            let _ = std::fs::remove_file(p);
        }
        Self { path }
    }

    fn line(&self, text: &str) {
        use std::io::Write;
        let Some(path) = &self.path else {
            return;
        };
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = writeln!(f, "{} pid={} {text}", utc_now_text(), std::process::id());
        }
    }
}

/// `YYYY-MM-DDTHH:MM:SSZ`（UTC）。登録の記録用（時刻のクレートに依存しない）。
fn utc_now_text() -> String {
    utc_text(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    )
}

fn utc_text(secs: u64) -> String {
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // 1970-01-01 からの日数を暦日へ（Howard Hinnant の civil_from_days）
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllRegisterServer() -> windows::core::HRESULT {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

    let log = RegisterLog::open();
    macro_rules! step {
        ($label:expr, $expr:expr) => {{
            match $expr {
                Ok(v) => {
                    log.line(&format!("register OK: {}", $label));
                    v
                }
                Err(e) => {
                    log.line(&format!("register FAIL: {} — {}", $label, e));
                    CoUninitialize();
                    return E_FAIL;
                }
            }
        }};
    }

    log.line(&format!(
        "DllRegisterServer start build={}",
        option_env!("RAKUKAN_BUILD_TIME").unwrap_or("unknown")
    ));
    let dll_path = step!("get_path", crate::globals::DllModule::get_path());
    log.line(&format!("dll_path: {dll_path}"));
    step!(
        "clsid_register",
        tsf::registration::clsid_register(&dll_path)
    );
    step!(
        "profile_register",
        tsf::registration::profile_register(&dll_path)
    );
    step!("category_register", tsf::registration::category_register());
    log.line("DllRegisterServer success");

    CoUninitialize();
    S_OK
}

#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllUnregisterServer() -> windows::core::HRESULT {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoUninitialize};
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    let log = RegisterLog::open();
    log.line(&format!(
        "DllUnregisterServer start build={}",
        option_env!("RAKUKAN_BUILD_TIME").unwrap_or("unknown")
    ));
    // 各段階の結果を残す。戻り値は今までどおり S_OK（既に解除済みの項目の失敗で
    // 解除全体を失敗にしない）。成否の判断は記録とレジストリの状態で行う（#33）。
    for (label, result) in tsf::registration::unregister_server() {
        match result {
            Ok(()) => log.line(&format!("unregister OK: {label}")),
            Err(e) => log.line(&format!("unregister FAIL: {label} — {e}")),
        }
    }
    log.line("DllUnregisterServer end");
    CoUninitialize();
    S_OK
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tracing_subscriber::layer::{Context, SubscriberExt};

    #[test]
    fn register_log_time_is_utc_text() {
        assert_eq!(utc_text(0), "1970-01-01T00:00:00Z");
        // 2026-10-10 の TSF ログの起動識別子 1791599318973 の秒の部分
        assert_eq!(utc_text(1_791_599_318), "2026-10-10T02:28:38Z");
        assert_eq!(utc_text(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn default_files_are_created_only_when_absent() {
        let dir = std::env::temp_dir().join(format!(
            "rakukan-tsf-test-write-if-absent-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("config.toml");
        assert!(crate::engine::config::write_if_absent(&path, b"first").expect("create"));
        // 既にあるファイルは上書きしない（同時に作られた設定を消さない）
        assert!(!crate::engine::config::write_if_absent(&path, b"second").expect("exists"));
        assert_eq!(std::fs::read(&path).expect("read"), b"first");
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct CountTargets {
        tsf: Arc<AtomicUsize>,
        rpc: Arc<AtomicUsize>,
        other: Arc<AtomicUsize>,
    }

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for CountTargets {
        fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
            let counter = match event.metadata().target() {
                "rakukan_tsf" => &self.tsf,
                "rakukan_engine_rpc" => &self.rpc,
                _ => &self.other,
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn default_log_filter_spec_lists_tsf_and_rpc_client() {
        assert_eq!(
            default_log_filter_spec("debug"),
            "rakukan_tsf=debug,rakukan_engine_rpc=debug"
        );
    }

    #[test]
    fn default_log_filter_records_rpc_client_events() {
        let tsf = Arc::new(AtomicUsize::new(0));
        let rpc = Arc::new(AtomicUsize::new(0));
        let other = Arc::new(AtomicUsize::new(0));
        let subscriber = tracing_subscriber::registry()
            .with(tracing_subscriber::EnvFilter::new(default_log_filter_spec(
                "info",
            )))
            .with(CountTargets {
                tsf: tsf.clone(),
                rpc: rpc.clone(),
                other: other.clone(),
            });

        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(target: "rakukan_tsf", "tsf warn");
            tracing::warn!(target: "rakukan_engine_rpc", "rpc SLOW");
            // 指定レベル未満は記録しない
            tracing::debug!(target: "rakukan_engine_rpc", "rpc read failed");
            // 列挙していない target は記録しない
            tracing::warn!(target: "rakukan_engine", "engine warn");
        });

        assert_eq!(tsf.load(Ordering::Relaxed), 1);
        assert_eq!(rpc.load(Ordering::Relaxed), 1);
        assert_eq!(other.load(Ordering::Relaxed), 0);
    }
}
