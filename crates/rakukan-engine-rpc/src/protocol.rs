//! RPC プロトコル定義。
//!
//! DynEngine の全メソッドを 1 対 1 で Request バリアントにマッピングする。
//! 後方互換のため、既存バリアントの順序変更や削除はせず、追加のみで拡張する（postcard の enum は順序依存）。

use rakukan_engine_abi::Segments as SegmentsModel;
use serde::{Deserialize, Serialize};

/// Named Pipe 名のベース。実際のパイプ名は `format!("\\\\.\\pipe\\{PIPE_BASE_NAME}-{user}")` で構成する。
pub const PIPE_BASE_NAME: &str = "rakukan-engine";

/// 現在のプロトコルバージョン。接続直後の Hello で交換する。
///
/// - v1: 0.4.4 初版
/// - v2: `InputChar` / `InputCharResult` バッチ RPC を追加（0.4.5）
/// - v3: `ConvertToSegments` / `ResizeSegment` / `SegmentCandidatesFor` を追加（Phase A）
pub const PROTOCOL_VERSION: u32 = 3;

/// `InputChar` バッチ RPC で指定する入力モード。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum InputCharKind {
    /// `push_char` と等価（ローマ字変換経由）
    Char,
    /// `push_fullwidth_alpha` と等価（A-Z を全角英字に）
    FullwidthAlpha,
    /// `push_raw` と等価（かなルールに登録された記号等を直接）
    Raw,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    // ─── 接続 ─────────────────────────────────────────────
    /// 接続直後に必ず送る。ホスト側はバージョン不一致なら Error を返して切断する。
    Hello {
        protocol_version: u32,
    },
    /// エンジン側セッションの初期化要求。config_json は EngineConfig の JSON。
    /// 既に DynEngine が存在する場合は何もしない（idempotent）。
    Create {
        config_json: Option<String>,
    },

    /// 現在の DynEngine を drop し、新しい config_json で load_auto し直す。
    /// config.toml を編集したあとの IME モード切替で呼ばれる。
    /// model / 辞書の bg ロードもホスト側で再起動する。
    Reload {
        config_json: Option<String>,
    },

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
    BgStart {
        n_cands: u32,
    },
    BgStatus,
    BgTakeCandidates {
        key: String,
    },
    #[deprecated = "removed in ABI v7; do not use"]
    _ReservedBgTakeSegmentedCandidates {
        key: String,
    },
    BgReclaim,
    BgWaitMs {
        timeout_ms: u64,
    },

    // ─── 確定・リセット ───────────────────────────────────
    Commit {
        text: String,
    },
    CommitAsHiragana,
    ResetPreedit,
    ForcePreedit {
        text: String,
    },
    ResetAll,

    // ─── 同期変換 ─────────────────────────────────────────
    ConvertSync,
    #[deprecated = "removed in ABI v7; do not use"]
    _ReservedConvertSyncSegmented,
    MergeCandidates {
        llm_cands: Vec<String>,
        limit: u32,
    },
    #[deprecated = "removed in ABI v7; do not use"]
    _ReservedSegmentSurface {
        surface: String,
    },
    #[deprecated = "removed in ABI v7; do not use"]
    _ReservedSegmentCandidate {
        surface: String,
        reading: String,
    },

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
    Learn {
        reading: String,
        surface: String,
    },

    // ─── 診断 ─────────────────────────────────────────────
    LastError,
    DictStatus,

    // ─── ライフサイクル ────────────────────────────────────
    /// クライアント側が切断を宣言する。ホストは該当セッションを破棄する。
    Bye,

    // ─── Segments モデル (v3) ───────────────────────────────
    /// Reserved: was ConvertToSegments. Kept for postcard enum ordinal compatibility.
    #[deprecated = "removed in ABI v6; do not use"]
    _ReservedConvertToSegments {
        reading: String,
        context: String,
        num_candidates: u32,
    },
    ResizeSegment {
        segments_json: String,
        index: u32,
        offset: i32,
        num_candidates: u32,
    },
    SegmentCandidatesFor {
        reading: String,
        context: String,
        num_candidates: u32,
    },

    // ─── バッチ入力 (v2) ───────────────────────────────────
    /// 1 キーストロークを 1 RPC で処理するバッチ API。
    ///
    /// ホスト側は以下を順に実行し、結果をまとめて `InputCharResult` で返す:
    /// 1. `kind` に応じて `push_char` / `push_fullwidth_alpha` / `push_raw`
    /// 2. `preedit_display()` を取得
    /// 3. `hiragana_text()` を取得
    /// 4. `bg_status()` を取得
    /// 5. `bg_start_n_cands` が `Some` かつ hiragana が非空なら `bg_start(n)`
    InputChar {
        c: u32,
        kind: InputCharKind,
        bg_start_n_cands: Option<u32>,
    },

    // ─── プロセス終了（M1.6 T-HOST1）─────────────────────────
    /// ホストプロセスに self-exit を依頼する。
    ///
    /// `Reload` の代替経路: 旧 `Reload` は engine DLL を drop → 新規 load で
    /// 反映していたが、BG スレッドが DLL を参照している瞬間に unmap が走ると
    /// AV を誘発する。`Shutdown` を受けたホストは `Response::Unit` を返して
    /// flush 後、`std::process::exit(0)` でプロセスごと終了する。OS が
    /// 全スレッドと DLL マッピングをまとめて回収するため race が原理的に起きない。
    ///
    /// クライアント側は応答受信後、既存接続を破棄する。次回 API 呼び出し時に
    /// `connect_or_spawn` で自動的にホストを再 spawn する経路が既にあるため、
    /// TSF 側コードはほぼ無変更で済む。
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Hello {
        protocol_version: u32,
    },
    Unit,
    Bool(bool),
    U32(u32),
    I32(i32),
    String(String),
    Strings(Vec<String>),
    #[deprecated = "removed in ABI v7; do not use"]
    _ReservedSegments(Vec<u8>),
    #[deprecated = "removed in ABI v7; do not use"]
    _ReservedSegmentBlocks(Vec<u8>),
    SegmentsModel(SegmentsModel),
    /// ホスト側で処理中に発生したエラー（DLL 未ロード、引数不正、内部 panic 等）。
    Error(String),

    /// `Request::InputChar` の結果。
    /// `bg_status` は DynEngine 側の `&'static str` を所有 String にしたもの。
    InputCharResult {
        preedit: String,
        hiragana: String,
        bg_status: String,
    },
}
