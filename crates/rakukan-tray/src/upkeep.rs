//! トレイの起動時の作業（#33・#74）: 多重起動の抑止、トレイのログ、ログの掃除、
//! キーボードの一覧の補い。
//!
//! 掃除と一覧の補いは UI のスレッドで行わない（起動時に 1 回、別のスレッドで行う）。

use std::path::{Path, PathBuf};

/// 多重起動の抑止に使う名前。セッションごと（`Local\`）に 1 つ。
const SINGLETON_NAME: &str = "Local\\rakukan.tray.singleton";

/// キーボードの一覧を補った版を覚えておく場所（HKCU）。
const STATE_KEY: &str = "Software\\rakukan";
const TIP_LISTED_VALUE: &str = "TipListedVersion";

/// 一覧に足すスクリプト（インストール先に置く）。
const REGISTER_TIP_SCRIPT: &str = "register-tip.ps1";

fn to_wide_z(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 同じセッションで別のトレイが動いていれば `true`。
///
/// mutex のハンドルは閉じず、プロセスが終わるまで持つ。名前付きの mutex を作れない場合は
/// 抑止なしで続ける（今までと同じ動き）。
pub fn another_instance_running() -> bool {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::PCWSTR;
    let name = to_wide_z(SINGLETON_NAME);
    match unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) } {
        Ok(_handle) => (unsafe { GetLastError() }) == ERROR_ALREADY_EXISTS,
        Err(_) => false,
    }
}

/// ログの置き場所（`%LOCALAPPDATA%\rakukan`）。
fn log_dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(PathBuf::from(base).join("rakukan"))
}

/// このビルドの識別（ログのヘッダーに使う）。
fn build_id() -> String {
    format!(
        "{}@{}",
        env!("CARGO_PKG_VERSION"),
        option_env!("RAKUKAN_GIT_SHA").unwrap_or("unknown")
    )
}

/// トレイのログ（`rakukan-tray-<PID>-<起動識別子>.log` + `.lock`）。
///
/// 書き込みは専用のスレッドで行う。`.lock` を取れない・スレッドを起動できないときは
/// ログなしで続ける。戻り値は (ログ, 自分のインスタンスの stem)。
pub fn init_logging() -> Option<(rakukan_log::AsyncLog, String)> {
    let dir = log_dir()?;
    let instance = rakukan_log::file::acquire_instance(
        rakukan_log::file::TRAY_PREFIX,
        &dir,
        std::process::id(),
    )?;
    let header = rakukan_log::header::header_line("tray", &instance.stem, &build_id());
    let file = rakukan_log::file::RotatingLog::open(instance.log_path, Some(header));
    let log = rakukan_log::AsyncLog::spawn("rakukan-log-writer", file, instance.lock).ok()?;
    {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        let _ = tracing_subscriber::registry()
            .with(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
            )
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(log.clone())
                    .with_ansi(false)
                    .with_thread_ids(true),
            )
            .try_init();
    }
    Some((log, instance.stem))
}

/// ログの掃除と一覧の補いを、別のスレッドで 1 回行う。
pub fn spawn_startup_work(own_stem: Option<String>) {
    let spawned = std::thread::Builder::new()
        .name("rakukan-tray-upkeep".to_string())
        .spawn(move || {
            if let Some(dir) = log_dir() {
                let own = own_stem.as_deref().unwrap_or("");
                let report =
                    rakukan_log::file::cleanup_logs(&dir, (rakukan_log::file::TRAY_PREFIX, own));
                tracing::info!(
                    "log cleanup: removed_instances={} removed_files={} kept_instances={} remaining_files={}",
                    report.removed_instances,
                    report.removed_files,
                    report.kept_instances,
                    report.remaining_files
                );
            }
            ensure_tip_listed_once();
        });
    if let Err(e) = spawned {
        tracing::warn!("tray upkeep thread failed to start: {e}");
    }
}

/// キーボードの一覧に rakukan が無ければ足す。版ごとに 1 回だけ行う。
///
/// インストーラーは一覧への追加を、インストーラーを起動したユーザーの権限で行うが、
/// 昇格した端末から起動した場合と別のユーザーでは入らない。それをここで補う。
/// 同じ版で 2 回目は行わないので、利用者が一覧から外した場合はその状態を保つ。
fn ensure_tip_listed_once() {
    let version = env!("CARGO_PKG_VERSION");
    if read_listed_version().as_deref() == Some(version) {
        return;
    }
    let Some(script) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join(REGISTER_TIP_SCRIPT)))
        .filter(|p| p.exists())
    else {
        tracing::info!(
            "{REGISTER_TIP_SCRIPT} not found next to the tray; keyboard list not checked"
        );
        return;
    };
    match run_register_tip(&script) {
        Ok(()) => {
            tracing::info!("keyboard list checked for {version}");
            write_listed_version(version);
        }
        Err(e) => tracing::warn!("keyboard list check failed: {e}"),
    }
}

fn run_register_tip(script: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let windir = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let powershell = PathBuf::from(windir)
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    let status = std::process::Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{REGISTER_TIP_SCRIPT} exited with {status}"))
    }
}

fn read_listed_version() -> Option<String> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::PCWSTR;
    let key = to_wide_z(STATE_KEY);
    let value = to_wide_z(TIP_LISTED_VALUE);
    let mut buf = vec![0u16; 256];
    let mut len = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if rc.is_err() {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

fn write_listed_version(version: &str) {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegSetKeyValueW};
    use windows::core::PCWSTR;
    let key = to_wide_z(STATE_KEY);
    let value = to_wide_z(TIP_LISTED_VALUE);
    let data = to_wide_z(version);
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            PCWSTR(key.as_ptr()),
            PCWSTR(value.as_ptr()),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    };
    if rc.is_err() {
        tracing::warn!("cannot record {TIP_LISTED_VALUE}: {rc:?}");
    }
}
