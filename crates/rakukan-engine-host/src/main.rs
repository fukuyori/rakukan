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
use std::sync::{Arc, Mutex};

use anyhow::Result;
use rakukan_engine_rpc::server::{SharedEngine, serve};

fn log_path() -> PathBuf {
    rakukan_engine_abi::install_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rakukan-engine-host.log")
}

const LOG_ROTATE_MAX_BYTES: u64 = 16 * 1024 * 1024;
const LOG_ROTATE_GENERATIONS: usize = 5;

fn rotated_log_path(path: &std::path::Path, generation: usize) -> Option<PathBuf> {
    let mut file_name = path.file_name()?.to_os_string();
    file_name.push(format!(".{generation}"));
    Some(path.with_file_name(file_name))
}

fn rotate_log_if_needed(path: &std::path::Path) {
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

fn init_tracing(log_path: &std::path::Path) {
    // ログは %LOCALAPPDATA%\rakukan\rakukan-engine-host.log に書き出す。
    // ファイル作成に失敗しても最低限 stderr に出す。
    rotate_log_if_needed(log_path);
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        Ok(file) => {
            let _ = tracing_subscriber::fmt()
                .with_writer(Mutex::new(file))
                .with_ansi(false)
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                )
                .try_init();
        }
        Err(_) => {
            let _ = tracing_subscriber::fmt().try_init();
        }
    }
}

/// Win32 STDERR を log ファイルに向ける。サイレント死の原因（llama.cpp の
/// `fprintf(stderr, ...)` や Rust eprintln）を log と同じファイルに残すため。
///
/// `windows_subsystem = "windows"` だと CRT stderr は最初からどこにも繋がって
/// いないので、リダイレクトしないと診断情報が消える。
#[cfg(windows)]
fn redirect_stderr_to_log(log_path: &std::path::Path) {
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Console::{STD_ERROR_HANDLE, SetStdHandle};

    let file = match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("redirect_stderr_to_log: open failed: {e}");
            return;
        }
    };
    let handle = HANDLE(file.as_raw_handle());
    if let Err(e) = unsafe { SetStdHandle(STD_ERROR_HANDLE, handle) } {
        tracing::warn!("redirect_stderr_to_log: SetStdHandle failed: {e:?}");
        return;
    }
    // OS ハンドルはプロセス終了まで開いていてほしい。File を drop すると
    // CloseHandle されるので forget でリーク（プロセス終了時に OS が回収）。
    std::mem::forget(file);
    tracing::info!("stderr redirected to log file");
}

#[cfg(not(windows))]
fn redirect_stderr_to_log(_log_path: &std::path::Path) {}

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
        // 既存 hook（既定の stderr 出力）も呼ぶ。stderr は↑でログへリダイレクト
        // 済みなので、Rust デフォルトのバックトレースもログに入る。
        prev(info);
    }));
}

fn main() -> Result<()> {
    let log_path = log_path();
    init_tracing(&log_path);
    install_panic_hook();
    redirect_stderr_to_log(&log_path);
    tracing::info!("rakukan-engine-host starting (pid={})", std::process::id());

    // エンジンはまだ作らない。最初のクライアント Create リクエストで
    // DynEngine::load_auto が呼ばれる。これにより「ホストを起動しても
    // model/dict ロードは初回クライアント接続までは走らない」という
    // 遅延ロード特性が維持される。
    let engine: SharedEngine = Arc::new(Mutex::new(None));

    // serve() はブロッキングで Named Pipe を待ち受け続ける。
    if let Err(e) = serve(engine) {
        tracing::error!("serve terminated with error: {e}");
        return Err(e);
    }
    Ok(())
}
