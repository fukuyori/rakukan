//! rakukan エンジンホストプロセス。
//!
//! `rakukan_engine_*.dll`（llama.cpp 同梱）を本プロセスにロードし、
//! Named Pipe RPC で TSF DLL にサービスを提供する。
//!
//! TSF DLL 側はもはや engine DLL を **直接 LoadLibrary しない** ことが目的。
//! これにより Zoom / Dropbox / explorer 等のホストアプリに llama.cpp 及び
//! そのランタイム（msvcp140 等）を持ち込まなくなり、対象プロセスの
//! クラッシュを回避する。

#![windows_subsystem = "windows"]

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use rakukan_engine_rpc::server::{HostShared, SharedEngine, serve};

/// ログの置き場所（`%LOCALAPPDATA%\rakukan`）。インストール先とは分ける。
fn log_dir() -> PathBuf {
    rakukan_engine_abi::local_data_dir().unwrap_or_else(|| std::path::PathBuf::from("."))
}

/// 書き込み用のスレッドのキューを書き出すのを待つ口（終了時・panic 時）。
static LOG_FLUSHER: std::sync::OnceLock<rakukan_log::Flusher> = std::sync::OnceLock::new();

/// 終了時・panic 時に、キューに残った行を書き出すまで待つ上限。
const LOG_FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

fn flush_log() {
    if let Some(flusher) = LOG_FLUSHER.get() {
        flusher.flush(LOG_FLUSH_TIMEOUT);
    }
}

/// ログの初期化（#75）。
///
/// TSF と同じく、起動インスタンスごとの `rakukan-engine-host-<PID>-<起動識別子>.log` + `.lock`
/// に書く。書き込みは専用のスレッドで行う（#76）。`.lock` を取れない・スレッドを起動できない
/// ときはログなしで続ける。engine DLL のログも callback でここへ流れる。
fn init_tracing(build: &rakukan_engine_abi::HostBuildId) -> Option<rakukan_log::AsyncLog> {
    let dir = log_dir();
    let instance = rakukan_log::file::acquire_instance(
        rakukan_log::file::HOST_PREFIX,
        &dir,
        std::process::id(),
    )?;
    let header = rakukan_log::header::header_line(
        "host",
        &instance.stem,
        &format!("{}@{}", build.pkg_version, build.git_sha),
    );
    let file = rakukan_log::file::RotatingLog::open(instance.log_path, Some(header));
    // ロックは書き込み用のスレッドが持ち、プロセスが終わるまで離さない
    let log = rakukan_log::AsyncLog::spawn("rakukan-log-writer", file, instance.lock).ok()?;
    let _ = LOG_FLUSHER.set(log.flusher());
    let _ = tracing_subscriber::fmt()
        .with_writer(log.clone())
        .with_ansi(false)
        .with_thread_ids(true)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .try_init();
    Some(log)
}

/// Win32 STDERR をパイプで受け、ログと同じ書き込み用のスレッドへ流す（#75）。
///
/// サイレント死の原因（llama.cpp の `fprintf(stderr, ...)` や Rust eprintln、既定の panic
/// の出力）をログに残すため。以前は同じファイルを別ハンドルで開いていたが、ローテーションと
/// 書き手を 1 つにすること（#73）に合わないので、パイプを読むスレッドが 1 行ずつキューに入れる。
///
/// `windows_subsystem = "windows"` だと CRT stderr は最初からどこにも繋がって
/// いないので、リダイレクトしないと診断情報が消える。
#[cfg(windows)]
fn redirect_stderr_to_log(log: rakukan_log::AsyncLog) {
    use std::io::BufRead;
    use std::os::windows::io::FromRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{STD_ERROR_HANDLE, SetStdHandle};
    use windows::Win32::System::Pipes::CreatePipe;

    let mut read = HANDLE::default();
    let mut write = HANDLE::default();
    if let Err(e) = unsafe { CreatePipe(&mut read, &mut write, None, 0) } {
        tracing::warn!("redirect_stderr_to_log: CreatePipe failed: {e:?}");
        return;
    }
    // 書き込み側はプロセスが終わるまで stderr として開いておく（閉じない）
    if let Err(e) = unsafe { SetStdHandle(STD_ERROR_HANDLE, write) } {
        tracing::warn!("redirect_stderr_to_log: SetStdHandle failed: {e:?}");
        return;
    }
    let reader = unsafe { std::fs::File::from_raw_handle(read.0) };
    let spawned = std::thread::Builder::new()
        .name("rakukan-stderr".to_string())
        .spawn(move || {
            let mut lines = std::io::BufReader::new(reader);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match lines.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let text = String::from_utf8_lossy(&buf);
                        let text = text.trim_end_matches(['\r', '\n']);
                        log.send(format!("{} STDERR {text}\n", utc_now()).into_bytes());
                    }
                }
            }
        });
    match spawned {
        Ok(_) => tracing::info!("stderr redirected to the log"),
        Err(e) => tracing::warn!("redirect_stderr_to_log: reader thread failed: {e}"),
    }
}

#[cfg(not(windows))]
fn redirect_stderr_to_log(_log: rakukan_log::AsyncLog) {}

/// 今の時刻（UTC、`2026-10-23T01:02:03.456Z`）。stderr の行に付ける。
fn utc_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // 1970-01-01 からの日数を年月日へ（Howard Hinnant の civil_from_days）
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60,
        now.subsec_millis()
    )
}

/// named mutex による多重起動防止。
///
/// 同一 pipe 名で複数ホストが listen すると、クライアント接続が 2 プロセスへ
/// 分散して engine 状態の不整合（片方に Create、もう片方に変換要求）を起こす。
/// 実ログで 0.6 秒差の二重起動を確認済み（2026-07-02 12:03:52, pid 30800/24740）。
/// mutex ハンドルは HANDLE (no Drop) なのでプロセス終了まで保持される。
#[cfg(windows)]
fn exit_if_already_running() {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::PCWSTR;

    // pipe 名 (pipe_name_for_current_user) と同じユーザー単位のスコープにする
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".into());
    let sanitized: String = user
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = format!("Local\\rakukan-engine-host-{sanitized}");
    let wide: Vec<u16> = std::ffi::OsStr::new(&name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // engine_reload は旧ホストへ Shutdown を送った直後に新ホストを spawn する。
    // 旧プロセスの終了（= mutex ハンドル解放）が新プロセスの検査より遅れる
    // ことがあるため、短いリトライで正当な世代交代を誤検出しないようにする。
    use windows::Win32::Foundation::CloseHandle;
    const RETRY_MAX: u32 = 20;
    const RETRY_INTERVAL_MS: u64 = 100;
    for attempt in 0..RETRY_MAX {
        match unsafe { CreateMutexW(None, false, PCWSTR(wide.as_ptr())) } {
            Ok(handle) => {
                if unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
                    // 獲得成功。HANDLE は Drop で閉じられないためプロセス終了まで有効。
                    return;
                }
                let _ = unsafe { CloseHandle(handle) };
                if attempt == 0 {
                    tracing::info!(
                        "another rakukan-engine-host holds the singleton mutex, waiting up to {}ms",
                        RETRY_MAX as u64 * RETRY_INTERVAL_MS
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(RETRY_INTERVAL_MS));
            }
            Err(e) => {
                // ガードなしでも従来動作と同じなので続行する
                tracing::warn!("exit_if_already_running: CreateMutexW failed: {e}");
                return;
            }
        }
    }
    tracing::info!(
        "another rakukan-engine-host is already running for this user, exiting (pid={})",
        std::process::id()
    );
    std::process::exit(0);
}

#[cfg(not(windows))]
fn exit_if_already_running() {}

/// Rust panic を tracing log に書き出す panic hook を設定する。
///
/// `panic = "abort"` 設定でも panic hook は abort 前に実行される。これがないと
/// engine DLL 内の Rust panic が log に何も残さず process がいきなり死ぬ。
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown>".to_string());
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_string()
        };
        tracing::error!(
            "PANIC at {}: {} (thread={:?}, pid={})",
            location,
            payload,
            std::thread::current().name().unwrap_or("<unnamed>"),
            std::process::id()
        );
        // 既存 hook（既定の stderr 出力）も呼ぶ。stderr はパイプでログへ流しているので、
        // Rust デフォルトのバックトレースもログに入る。abort の前にキューを書き出す
        prev(info);
        flush_log();
    }));
}

fn main() -> Result<()> {
    // ログの初期化は多重起動の判定より後にする（#75）。終わるだけのホストにログを作らない
    exit_if_already_running();
    let build = rakukan_engine_abi::host_build_id();
    if let Some(log) = init_tracing(&build) {
        redirect_stderr_to_log(log);
    }
    install_panic_hook();
    tracing::info!(
        "rakukan-engine-host starting (pid={}) version={} git={}",
        std::process::id(),
        build.pkg_version,
        build.git_sha
    );

    // エンジンはまだ作らない。最初のクライアント Create リクエストで
    // DynEngine::load_auto が呼ばれる。これにより「ホストを起動しても
    // model/dict ロードは初回クライアント接続までは走らない」という
    // 遅延ロード特性が維持される。
    let engine: SharedEngine = Arc::new(HostShared::new());

    // serve() はブロッキングで Named Pipe を待ち受け続ける。
    let served = serve(engine);
    if let Err(e) = &served {
        tracing::error!("serve terminated with error: {e}");
    }
    flush_log();
    served
}
