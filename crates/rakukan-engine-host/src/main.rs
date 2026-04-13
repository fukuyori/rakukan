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

use std::sync::{Arc, Mutex};

use anyhow::Result;
use rakukan_engine_rpc::server::{SharedEngine, serve};

fn init_tracing() {
    // ログは %LOCALAPPDATA%\rakukan\rakukan-engine-host.log に書き出す。
    // ファイル作成に失敗しても最低限 stderr に出す。
    let log_path = rakukan_engine_abi::install_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rakukan-engine-host.log");
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
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

fn main() -> Result<()> {
    init_tracing();
    tracing::info!(
        "rakukan-engine-host starting (pid={})",
        std::process::id()
    );

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
