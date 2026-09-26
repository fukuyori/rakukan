//! rakukan エンジンのプロセス間 RPC レイヤー。
//!
//! - トランスポート: Windows Named Pipe (`\\.\pipe\rakukan-engine-<user>`)
//! - フレーミング: `[u32 LE payload-length][postcard payload]`
//! - エンコード: postcard（serde）
//!
//! # 目的
//! `rakukan_engine_*.dll`（llama.cpp 同梱・重量級 C++）を
//! TSF ホストプロセス（Zoom/Dropbox/explorer/…）に一切ロードしないため、
//! DLL ロードと llama ワーカースレッドを `rakukan-engine-host.exe` に集約し、
//! TSF 側は Pipe クライアントとしてのみ振る舞う。

pub mod client;
pub mod codec;
pub mod health;
pub mod pipe;
pub mod protocol;
pub mod server;

pub use client::{
    RpcEngine, ShutdownOutcome, rpc_stats_reset, rpc_stats_snapshot, set_host_log_level,
};
pub use protocol::{
    ChangeKind, ChangeOutcome, ChangeRequest, ConfigVersion, EngineGen, Expect, HostId,
    InputCharKind, Owner, PIPE_BASE_NAME, Reason, Request, RequestSeq, Response, TsfId, Unresolved,
};
pub use rakukan_engine_abi::{Candidate, CandidateSource, Segment, Segments};
