//! RPC プロトコル定義。
//!
//! DynEngine の全メソッドを 1 対 1 で Request バリアントにマッピングする。
//! 後方互換のため、既存バリアントの順序変更や削除はせず、追加のみで拡張する（postcard の enum は順序依存）。

use rakukan_engine_abi::{SegmentBlock, SegmentCandidate};
use serde::{Deserialize, Serialize};

/// Named Pipe 名のベース。実際のパイプ名は `format!("\\\\.\\pipe\\{PIPE_BASE_NAME}-{user}")` で構成する。
pub const PIPE_BASE_NAME: &str = "rakukan-engine";

/// 現在のプロトコルバージョン。接続直後の Hello で交換する。
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    // ─── 接続 ─────────────────────────────────────────────
    /// 接続直後に必ず送る。ホスト側はバージョン不一致なら Error を返して切断する。
    Hello { protocol_version: u32 },
    /// エンジン側セッションの初期化要求。config_json は EngineConfig の JSON。
    /// 既に DynEngine が存在する場合は何もしない（idempotent）。
    Create { config_json: Option<String> },

    /// 現在の DynEngine を drop し、新しい config_json で load_auto し直す。
    /// config.toml を編集したあとの IME モード切替で呼ばれる。
    /// model / 辞書の bg ロードもホスト側で再起動する。
    Reload { config_json: Option<String> },

    // ─── 文字入力 ─────────────────────────────────────────
    PushChar(u32),
    PushRaw(u32),
    PushFullwidthAlpha(u32),
    Backspace,
    FlushPendingN,

    // ─── プリエディット状態 ────────────────────────────────
    PreeditDisplay,
    PreeditIsEmpty,
    HiraganaText,
    RomajiLogStr,
    HiraganaFromRomajiLog,
    CommittedText,

    // ─── BG 変換 ──────────────────────────────────────────
    BgStart { n_cands: u32 },
    BgStatus,
    BgTakeCandidates { key: String },
    BgTakeSegmentedCandidates { key: String },
    BgReclaim,
    BgWaitMs { timeout_ms: u64 },

    // ─── 確定・リセット ───────────────────────────────────
    Commit { text: String },
    CommitAsHiragana,
    ResetPreedit,
    ForcePreedit { text: String },
    ResetAll,

    // ─── 同期変換 ─────────────────────────────────────────
    ConvertSync,
    ConvertSyncSegmented,
    MergeCandidates { llm_cands: Vec<String>, limit: u32 },
    SegmentSurface { surface: String },
    SegmentCandidate { surface: String, reading: String },

    // ─── 非同期初期化 ─────────────────────────────────────
    StartLoadModel,
    PollModelReady,
    StartLoadDict,
    PollDictReady,

    // ─── ステータス ───────────────────────────────────────
    IsKanjiReady,
    IsDictReady,
    BackendLabel,
    NGpuLayers,
    MainGpu,
    AvailableModelsJson,

    // ─── 学習 ─────────────────────────────────────────────
    Learn { reading: String, surface: String },

    // ─── 診断 ─────────────────────────────────────────────
    LastError,
    DictStatus,

    // ─── ライフサイクル ────────────────────────────────────
    /// クライアント側が切断を宣言する。ホストは該当セッションを破棄する。
    Bye,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Hello { protocol_version: u32 },
    Unit,
    Bool(bool),
    U32(u32),
    I32(i32),
    String(String),
    Strings(Vec<String>),
    Segments(Vec<SegmentCandidate>),
    SegmentBlocks(Vec<SegmentBlock>),
    /// ホスト側で処理中に発生したエラー（DLL 未ロード、引数不正、内部 panic 等）。
    Error(String),
}
