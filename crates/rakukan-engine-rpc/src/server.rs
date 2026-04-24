//! RPC サーバ実装。
//!
//! 1 Named Pipe インスタンス = 1 クライアント接続。
//! クライアント接続ごとに 1 スレッドを spawn し、そのスレッド内で
//! `DynEngine` を排他的に使ってリクエストに応答する。
//!
//! # エンジン共有方針（Phase A 初期）
//! エンジンインスタンスは **グローバル 1 個** を `Mutex<DynEngine>` で共有する。
//! llama 推論は逐次なのでシリアル化で問題にならない。
//! セッションごとに別エンジンを作ると model/dict のロードが多重化して
//! VRAM/メモリを浪費するため避ける。
//!
//! セッション間の hiragana_buf 等の汚染は TSF 側が既に `ResetAll` を
//! フォーカス変化で呼ぶ前提でカバーする。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use rakukan_engine_abi::DynEngine;

use crate::codec::{read_frame, write_frame};
use crate::pipe::{PipeStream, pipe_name_for_current_user};
use crate::protocol::{InputCharKind, PROTOCOL_VERSION, Request, Response};

/// ホスト全体で共有される 1 つの DynEngine。
pub type SharedEngine = Arc<Mutex<Option<DynEngine>>>;

/// Named Pipe サーバを起動し、クライアント接続を待ち受けるループを実行する。
///
/// この関数はブロッキングで走り続ける。通常は `rakukan-engine-host` のメインスレッドから呼ぶ。
pub fn serve(engine: SharedEngine) -> Result<()> {
    let pipe_name = pipe_name_for_current_user();
    tracing::info!("engine host: listening on {pipe_name}");
    loop {
        let stream = PipeStream::create_server(&pipe_name)
            .with_context(|| format!("create server pipe {pipe_name}"))?;
        if let Err(e) = stream.accept() {
            tracing::warn!("accept failed: {e}");
            continue;
        }
        let engine_c = engine.clone();
        std::thread::Builder::new()
            .name("rakukan-engine-rpc-session".into())
            .spawn(move || {
                if let Err(e) = handle_session(stream, engine_c) {
                    tracing::warn!("session ended with error: {e}");
                }
            })
            .ok();
    }
}

fn handle_session(mut stream: PipeStream, engine: SharedEngine) -> Result<()> {
    tracing::debug!("rpc session: started");
    loop {
        let req: Request = match read_frame(&mut stream) {
            Ok(r) => r,
            Err(e) => {
                tracing::debug!("rpc session: read_frame failed, closing: {e}");
                return Ok(());
            }
        };
        // M1.6 T-HOST1: Shutdown は応答送信後にプロセス exit するため前取り判定。
        let is_shutdown = matches!(req, Request::Shutdown);
        let resp = dispatch(&engine, req);
        if let Err(e) = write_frame(&mut stream, &resp) {
            tracing::debug!("rpc session: write_frame failed, closing: {e}");
            return Ok(());
        }
        if is_shutdown {
            // OS にパイプ経由の response を配送させるため短時間待ってから exit。
            // flush は write_frame 内で完了しているが、pipe buffer から相手の read
            // までの伝播はカーネルスケジューリング依存。50ms で十分安全側に倒れる。
            std::thread::sleep(Duration::from_millis(50));
            tracing::info!("rpc: Shutdown requested, exiting host process");
            std::process::exit(0);
        }
    }
}

fn dispatch(engine: &SharedEngine, req: Request) -> Response {
    // Hello / Create は handle し、残りは DynEngine メソッドに流す
    match req {
        Request::Hello { protocol_version } => {
            if protocol_version != PROTOCOL_VERSION {
                return Response::Error(format!(
                    "protocol version mismatch: client={protocol_version} server={PROTOCOL_VERSION}"
                ));
            }
            Response::Hello {
                protocol_version: PROTOCOL_VERSION,
            }
        }
        Request::Create { config_json } => {
            // 既に存在すれば何もしない（idempotent）。
            let mut g = lock_engine(engine);
            if g.is_some() {
                return Response::Unit;
            }
            load_engine_into(&mut g, config_json.as_deref())
        }
        Request::Reload { config_json } => {
            // 既存 engine を drop してから作り直す。
            // config.toml 編集後のモード切替から呼ばれる。
            let mut g = lock_engine(engine);
            tracing::info!("rpc: Reload requested, dropping current engine");
            *g = None;
            load_engine_into(&mut g, config_json.as_deref())
        }
        Request::Bye => Response::Unit,
        Request::Shutdown => Response::Unit,
        other => {
            let mut g = match engine.lock() {
                Ok(g) => g,
                Err(p) => {
                    tracing::warn!("engine mutex poisoned, recovering");
                    p.into_inner()
                }
            };
            let Some(eng) = g.as_mut() else {
                return Response::Error("engine not created".into());
            };
            dispatch_engine(eng, other)
        }
    }
}

/// SharedEngine を lock し、poisoned を回復する小物ヘルパ。
fn lock_engine(engine: &SharedEngine) -> std::sync::MutexGuard<'_, Option<DynEngine>> {
    match engine.lock() {
        Ok(g) => g,
        Err(p) => {
            tracing::warn!("engine mutex poisoned, recovering");
            p.into_inner()
        }
    }
}

/// 指定 config_json で DynEngine::load_auto し、既存 slot に入れる。
/// 辞書・モデルの bg ロードも起動する。
fn load_engine_into(slot: &mut Option<DynEngine>, config_json: Option<&str>) -> Response {
    let install = match rakukan_engine_abi::install_dir() {
        Some(p) => p,
        None => return Response::Error("install_dir not found".into()),
    };
    match DynEngine::load_auto(&install, config_json) {
        Ok(mut eng) => {
            if !eng.is_dict_ready() {
                eng.start_load_dict();
            }
            if !eng.is_kanji_ready() {
                eng.start_load_model();
            }
            *slot = Some(eng);
            Response::Unit
        }
        Err(e) => Response::Error(format!("load_auto failed: {e}")),
    }
}

fn dispatch_engine(eng: &mut DynEngine, req: Request) -> Response {
    use Request::*;
    match req {
        Hello { .. } | Create { .. } | Reload { .. } | Bye | Shutdown => Response::Unit, // handled upstream

        PushChar(c) => {
            if let Some(ch) = char::from_u32(c) {
                eng.push_char(ch);
            }
            Response::Unit
        }
        PushRaw(c) => {
            if let Some(ch) = char::from_u32(c) {
                eng.push_raw(ch);
            }
            Response::Unit
        }
        PushFullwidthAlpha(c) => {
            if let Some(ch) = char::from_u32(c) {
                eng.push_fullwidth_alpha(ch);
            }
            Response::Unit
        }
        Backspace => Response::Bool(eng.backspace()),
        FlushPendingN => Response::Bool(eng.flush_pending_n()),

        PreeditDisplay => Response::String(eng.preedit_display()),
        PreeditIsEmpty => Response::Bool(eng.preedit_is_empty()),
        HiraganaText => Response::String(eng.hiragana_text()),
        RomajiLogStr => Response::String(eng.romaji_log_str()),
        HiraganaFromRomajiLog => Response::String(eng.hiragana_from_romaji_log()),
        CommittedText => Response::String(eng.committed_text()),

        BgStart { n_cands } => Response::Bool(eng.bg_start(n_cands as usize)),
        BgStatus => Response::String(eng.bg_status().to_string()),
        BgTakeCandidates { key } => match eng.bg_take_candidates(&key) {
            Some(v) => Response::Strings(v),
            None => Response::Strings(vec![]),
        },
        #[allow(deprecated)]
        _ReservedBgTakeSegmentedCandidates { .. } => Response::Error("removed".into()),
        BgReclaim => {
            eng.bg_reclaim();
            Response::Unit
        }
        BgWaitMs { timeout_ms } => Response::Bool(eng.bg_wait_ms(timeout_ms)),

        Commit { text } => {
            eng.commit(&text);
            Response::Unit
        }
        CommitAsHiragana => {
            eng.commit_as_hiragana();
            Response::Unit
        }
        ResetPreedit => {
            eng.reset_preedit();
            Response::Unit
        }
        ForcePreedit { text } => {
            eng.force_preedit(text);
            Response::Unit
        }
        ResetAll => {
            eng.reset_all();
            Response::Unit
        }

        ConvertSync => Response::Strings(eng.convert_sync()),
        #[allow(deprecated)]
        _ReservedConvertSyncSegmented => Response::Error("removed".into()),
        MergeCandidates { llm_cands, limit } => {
            Response::Strings(eng.merge_candidates(llm_cands, limit as usize))
        }
        #[allow(deprecated)]
        _ReservedSegmentSurface { .. } => Response::Error("removed".into()),
        #[allow(deprecated)]
        _ReservedSegmentCandidate { .. } => Response::Error("removed".into()),

        #[allow(deprecated)]
        _ReservedConvertToSegments { .. } => {
            Response::Error("ConvertToSegments has been removed in ABI v6".into())
        }
        ResizeSegment { .. } => Response::Error("resize_segment not yet implemented".into()),
        SegmentCandidatesFor { .. } => {
            Response::Error("segment_candidates_for not yet implemented".into())
        }

        StartLoadModel => {
            eng.start_load_model();
            Response::Unit
        }
        PollModelReady => Response::Bool(eng.poll_model_ready()),
        StartLoadDict => {
            eng.start_load_dict();
            Response::Unit
        }
        PollDictReady => Response::Bool(eng.poll_dict_ready()),

        IsKanjiReady => Response::Bool(eng.is_kanji_ready()),
        IsDictReady => Response::Bool(eng.is_dict_ready()),
        BackendLabel => Response::String(eng.backend_label()),
        NGpuLayers => Response::U32(eng.n_gpu_layers()),
        MainGpu => Response::I32(eng.main_gpu()),
        AvailableModelsJson => Response::String(eng.available_models_json()),

        Learn { reading, surface } => {
            eng.learn(&reading, &surface);
            Response::Unit
        }
        LastError => Response::String(eng.last_error()),
        DictStatus => Response::String(eng.dict_status()),

        InputChar {
            c,
            kind,
            bg_start_n_cands,
        } => {
            if let Some(ch) = char::from_u32(c) {
                match kind {
                    InputCharKind::Char => eng.push_char(ch),
                    InputCharKind::FullwidthAlpha => eng.push_fullwidth_alpha(ch),
                    InputCharKind::Raw => eng.push_raw(ch),
                }
            }
            let preedit = eng.preedit_display();
            let hiragana = eng.hiragana_text();
            let bg_status = eng.bg_status().to_string();
            if let Some(n) = bg_start_n_cands {
                if !hiragana.is_empty() {
                    eng.bg_start(n as usize);
                }
            }
            Response::InputCharResult {
                preedit,
                hiragana,
                bg_status,
            }
        }
    }
}

/// `Duration` を使う公開ヘルパ（main から idle 自死ロジックを書く用途）。
#[allow(dead_code)]
pub fn sleep_short() {
    std::thread::sleep(Duration::from_millis(50));
}
