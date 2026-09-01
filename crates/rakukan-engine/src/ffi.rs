//! rakukan-engine DLL 用 C ABI エクスポート
//!
//! 3 種類の DLL（cuda / vulkan / cpu）として同じ関数名でビルドされる。
//! rakukan-tsf は libloading でこれらのいずれかを実行時にロードする。
//!
//! # メモリ管理規約
//! - `*mut c_char` を返す関数はすべて `engine_free_string` で解放すること
//! - `*mut c_void` ハンドルは `engine_destroy` で解放すること
//! - caller が渡す `*const c_char` は関数呼び出しの間だけ有効であればよい

use crate::{EngineConfig, RakunEngine};
use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::OnceLock;

pub const ENGINE_ABI_VERSION: u32 = 9;

static LOG_INIT: OnceLock<()> = OnceLock::new();

/// `init_dll_logging` の結果。`engine_build_info` 経由で host に返し、
/// 「DLL 内ログがどこにも出ない」状態を host ログから判別できるようにする（Issue #8）。
static LOG_STATUS: OnceLock<String> = OnceLock::new();

/// DLL 内の tracing subscriber を初期化する。
///
/// cdylib は tracing の static をホストプロセスと共有しないため、これを
/// 行わないと DLL 内の tracing ログ（生成タイムアウト・EOS 未到達の警告等）は
/// どこにも出ない。%LOCALAPPDATA%\rakukan\rakukan-engine-dll.log に書き出す。
/// ログレベルは RAKUKAN_LOG 環境変数で上書き可能（既定 info）。
///
/// rlib として静的リンクされる場合（CLI 等）は呼び出し元の subscriber が
/// 先に設定されるので try_init は no-op になる。
fn init_dll_logging() {
    LOG_INIT.get_or_init(|| {
        let dir = std::env::var("LOCALAPPDATA")
            .map(|d| std::path::PathBuf::from(d).join("rakukan"))
            .unwrap_or_else(|_| std::path::PathBuf::from("."));
        let path = dir.join("rakukan-engine-dll.log");
        // 8 MiB 超で 1 世代ローテーション
        if let Ok(meta) = std::fs::metadata(&path) {
            if meta.len() > 8 * 1024 * 1024 {
                let rotated = dir.join("rakukan-engine-dll.log.1");
                let _ = std::fs::remove_file(&rotated);
                let _ = std::fs::rename(&path, &rotated);
            }
        }
        let status = match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(file) => {
                let init = tracing_subscriber::fmt()
                    .with_writer(std::sync::Mutex::new(file))
                    .with_ansi(false)
                    .with_env_filter(
                        tracing_subscriber::EnvFilter::try_from_env("RAKUKAN_LOG")
                            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
                    )
                    .try_init();
                match init {
                    Ok(()) => format!("ok path={}", path.display()),
                    // rlib として静的リンクされ、呼び出し元が先に subscriber を設定した場合など
                    Err(e) => format!("subscriber already set ({e}) path={}", path.display()),
                }
            }
            Err(e) => format!("open failed ({e}) path={}", path.display()),
        };
        let _ = LOG_STATUS.set(status);
    });
}

/// DLL の build 識別子と診断情報（`engine_build_info` の JSON 本体）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BuildInfo {
    /// crate version（workspace 共通）
    pub pkg_version: String,
    /// ビルド元 git コミット（短縮）。`-dirty` 付きは作業ツリーに変更あり。git 不明なら `unknown`
    pub git_sha: String,
    /// ビルド時刻（UTC）
    pub build_time: String,
    /// `engine_abi_version()` と同じ値
    pub abi_version: u32,
    /// DLL 内 tracing subscriber の初期化結果（`engine_create` 前は `not initialized`）
    pub log_status: String,
}

pub fn build_info() -> BuildInfo {
    BuildInfo {
        pkg_version: env!("CARGO_PKG_VERSION").to_string(),
        git_sha: option_env!("RAKUKAN_GIT_SHA")
            .unwrap_or("unknown")
            .to_string(),
        build_time: option_env!("RAKUKAN_ENGINE_BUILD_TIME")
            .unwrap_or("unknown")
            .to_string(),
        abi_version: ENGINE_ABI_VERSION,
        log_status: LOG_STATUS
            .get()
            .cloned()
            .unwrap_or_else(|| "not initialized".to_string()),
    }
}

// ─── ヘルパー ──────────────────────────────────────────────────────────────────

/// Rust String → heap 上の CString (caller が engine_free_string で解放)
unsafe fn to_cstr(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(_) => CString::new("").unwrap().into_raw(),
    }
}

/// `*const c_char` → &str（unsafe, null チェックなし）
unsafe fn from_cstr<'a>(ptr: *const c_char) -> &'a str {
    if ptr.is_null() {
        return "";
    }
    unsafe { CStr::from_ptr(ptr).to_str().unwrap_or("") }
}

// ─── ライフサイクル ────────────────────────────────────────────────────────────

/// エンジンを生成する。
/// `config_json`: JSON 文字列（`EngineConfig` のフィールドを持つオブジェクト）。
/// null または不正な場合はデフォルト設定を使用する。
/// 戻り値は `engine_destroy` で必ず解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_create(config_json: *const c_char) -> *mut c_void {
    init_dll_logging();
    let config: EngineConfig = if config_json.is_null() {
        EngineConfig::default()
    } else {
        let s = unsafe { from_cstr(config_json) };
        serde_json::from_str(s).unwrap_or_default()
    };

    let engine = Box::new(RakunEngine::new(config));
    Box::into_raw(engine) as *mut c_void
}

/// エンジンを破棄する。
#[unsafe(no_mangle)]
pub extern "C" fn engine_destroy(handle: *mut c_void) {
    if !handle.is_null() {
        unsafe {
            drop(Box::from_raw(handle as *mut RakunEngine));
        }
    }
}

/// `engine_create` / `engine_free_string` が返した文字列を解放する。
#[unsafe(no_mangle)]
pub extern "C" fn engine_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            drop(CString::from_raw(s));
        }
    }
}

/// engine DLL の ABI バージョンを返す。
#[unsafe(no_mangle)]
pub extern "C" fn engine_abi_version() -> u32 {
    ENGINE_ABI_VERSION
}

/// DLL の build 識別子と診断情報を JSON で返す（[`BuildInfo`]）。
///
/// host は起動時にこれを読んで自分の version / git sha と突き合わせ、
/// 「ABI は同じだが別ビルド」の組み合わせを WARN する（Issue #8）。
/// 任意シンボルとして扱われるため ABI バージョンは上げない（無い DLL は「不明」扱い）。
/// 呼び出し側が `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_build_info() -> *mut c_char {
    let json = serde_json::to_string(&build_info()).unwrap_or_else(|_| "{}".to_string());
    unsafe { to_cstr(json) }
}

#[cfg(test)]
mod build_info_tests {
    use super::*;

    #[test]
    fn build_info_reports_version_abi_and_roundtrips_as_json() {
        let info = build_info();
        assert_eq!(info.pkg_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(info.abi_version, ENGINE_ABI_VERSION);
        assert!(!info.git_sha.is_empty());
        let json = serde_json::to_string(&info).unwrap();
        let back: BuildInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pkg_version, info.pkg_version);
        assert_eq!(back.abi_version, info.abi_version);
        assert_eq!(back.git_sha, info.git_sha);
    }
}

// ─── 文字入力 ──────────────────────────────────────────────────────────────────

/// ローマ字変換を経由せず hiragana_buf に直接1文字追加する。
/// テンキー記号など、かなルールに登録されている文字をそのまま入力する場合に使用する。
#[unsafe(no_mangle)]
pub extern "C" fn engine_push_raw(handle: *mut c_void, codepoint: u32) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    if let Some(c) = char::from_u32(codepoint) {
        engine.push_raw(c);
    }
}

/// Shift+アルファベット用: hiragana_buf に全角大文字、romaji_input_log に ASCII 大文字を記録する。
#[unsafe(no_mangle)]
pub extern "C" fn engine_push_fullwidth_alpha(handle: *mut c_void, codepoint: u32) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    if let Some(c) = char::from_u32(codepoint) {
        engine.push_fullwidth_alpha(c);
    }
}

/// 1 文字を入力する（Unicode コードポイント）。
/// 戻り値: 0 = 通常, 1 = BG 変換を新たに起動した
#[unsafe(no_mangle)]
pub extern "C" fn engine_push_char(handle: *mut c_void, codepoint: u32) -> u8 {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    if let Some(c) = char::from_u32(codepoint) {
        engine.push_char(c);
    }
    0
}

/// Backspace を処理する。戻り値: true = プリエディットを消費した
#[unsafe(no_mangle)]
pub extern "C" fn engine_backspace(handle: *mut c_void) -> bool {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    engine.backspace()
}

/// 末尾 "n" を "ん" に確定する。戻り値: true = 変換した
#[unsafe(no_mangle)]
pub extern "C" fn engine_flush_n(handle: *mut c_void) -> bool {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    engine.flush_pending_n()
}

// ─── プリエディット状態 ────────────────────────────────────────────────────────

/// 現在のプリエディット文字列（ひらがな + pending ローマ字）を返す。
/// 戻り値は `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_preedit_display(handle: *mut c_void) -> *mut c_char {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    unsafe { to_cstr(engine.current_preedit().display()) }
}

/// プリエディットが空かどうか
#[unsafe(no_mangle)]
pub extern "C" fn engine_preedit_is_empty(handle: *mut c_void) -> bool {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    engine.preedit_is_empty()
}

/// ひらがなテキスト（pending ローマ字を含まない）
/// 戻り値は `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_hiragana_text(handle: *mut c_void) -> *mut c_char {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    unsafe { to_cstr(engine.hiragana_text().to_string()) }
}

/// ローマ字入力ログ（F9/F10 のカリフォルニア大学小学長文字変換用）
/// 戻り値は `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_romaji_log_str(handle: *mut c_void) -> *mut c_char {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    unsafe { to_cstr(engine.romaji_log_str()) }
}

/// romaji_input_log からひらがなを復元する（F6/F7/F8 でかなに戻す用）
/// 戻り値は `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_hiragana_from_romaji_log(handle: *mut c_void) -> *mut c_char {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    unsafe { to_cstr(engine.hiragana_from_romaji_log()) }
}

/// 確定済みテキスト（LLM コンテキスト用）
/// 戻り値は `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_committed_text(handle: *mut c_void) -> *mut c_char {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    unsafe { to_cstr(engine.committed_text().to_string()) }
}

// ─── バックグラウンド変換 ──────────────────────────────────────────────────────

/// バックグラウンド変換を起動する。
/// 戻り値: true = 起動した, false = 未準備 or ひらがな空
#[unsafe(no_mangle)]
pub extern "C" fn engine_bg_start(handle: *mut c_void, n_cands: u32) -> bool {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    engine.bg_start(n_cands as usize)
}

/// BG 変換状態を返す: "idle" / "running" / "done"
/// 戻り値は null 終端の static バイト列なので解放不要。
/// NOTE: Rust の &str は null 非終端なので s.as_ptr() を直接返してはいけない。
///       CStr::from_ptr() が "done" の後のメモリをゴミとして読み続けるため。
#[unsafe(no_mangle)]
pub extern "C" fn engine_bg_status(handle: *mut c_void) -> *const c_char {
    let _engine = unsafe { &*(handle as *const RakunEngine) };
    match crate::conv_cache::status() {
        "running" => b"running\0".as_ptr() as *const c_char,
        "done" => b"done\0".as_ptr() as *const c_char,
        _ => b"idle\0".as_ptr() as *const c_char,
    }
}

/// key が一致する BG 変換結果を取得する。
/// 戻り値: JSON 文字列 `["候補1","候補2",...]` または null（未完了/不一致）
/// 戻り値は `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_bg_take_candidates(
    handle: *mut c_void,
    key: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    let key_str = unsafe { from_cstr(key) };
    match engine.bg_take_candidates(key_str) {
        Some(cands) => {
            let json = serde_json::to_string(&cands).unwrap_or_else(|_| "[]".into());
            unsafe { to_cstr(json) }
        }
        None => std::ptr::null_mut(),
    }
}

/// M2 §5.2: ライブ変換 preview 用、トップ候補だけを覗き見る (cache 状態を進めない)。
/// 戻り値: トップ候補の文字列、または null（未完了/不一致）
/// 戻り値は `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_bg_peek_top_candidate(
    handle: *mut c_void,
    key: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    let key_str = unsafe { from_cstr(key) };
    match engine.bg_peek_top_candidate(key_str) {
        Some(s) => unsafe { to_cstr(s) },
        None => std::ptr::null_mut(),
    }
}

/// Done 状態の converter を engine に戻す（commit/cancel 時に呼ぶ）
#[unsafe(no_mangle)]
pub extern "C" fn engine_bg_reclaim(handle: *mut c_void) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    engine.bg_reclaim();
}

/// BG 変換完了を最大 timeout_ms ミリ秒ブロック待機する。
/// Done になれば 1、タイムアウトまたは Running でなければ 0 を返す。
/// Space 押下時に UIスレッドから呼ぶ用途を想定。
#[unsafe(no_mangle)]
pub extern "C" fn engine_bg_wait_ms(_handle: *mut c_void, timeout_ms: u64) -> u8 {
    let done = crate::conv_cache::wait_done_timeout(std::time::Duration::from_millis(timeout_ms));
    if done { 1 } else { 0 }
}

// ─── 確定・リセット ────────────────────────────────────────────────────────────

/// テキストを確定してプリエディットをクリア
#[unsafe(no_mangle)]
pub extern "C" fn engine_commit(handle: *mut c_void, text: *const c_char) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    let s = unsafe { from_cstr(text) };
    engine.commit(s);
}

/// ひらがなのままコミット
#[unsafe(no_mangle)]
pub extern "C" fn engine_commit_as_hiragana(handle: *mut c_void) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    engine.commit_as_hiragana();
}

/// プリエディットのみクリア（committed テキストは保持）
#[unsafe(no_mangle)]
pub extern "C" fn engine_reset_preedit(handle: *mut c_void) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    engine.reset_preedit();
}

/// プリエディットを指定文字列で強制置換（F6〜F10 文字種変換用）
#[unsafe(no_mangle)]
pub extern "C" fn engine_force_preedit(handle: *mut c_void, text: *const c_char) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    let s = unsafe {
        std::ffi::CStr::from_ptr(text)
            .to_string_lossy()
            .into_owned()
    };
    engine.force_preedit(s);
}

/// すべての状態をリセット
#[unsafe(no_mangle)]
pub extern "C" fn engine_reset_all(handle: *mut c_void) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    engine.reset_all();
}

// ─── 変換（同期フォールバック）────────────────────────────────────────────────

/// 現在のひらがなを同期変換して候補を返す。
/// 戻り値: JSON `["候補1","候補2",...]` または null（エラー/空）
/// `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_convert_sync(handle: *mut c_void) -> *mut c_char {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    match engine.convert_default() {
        Ok(cands) if !cands.is_empty() => {
            let json = serde_json::to_string(&cands).unwrap_or_else(|_| "[]".into());
            unsafe { to_cstr(json) }
        }
        _ => std::ptr::null_mut(),
    }
}

/// dict + LLM 候補をマージして返す。
/// `llm_json`: JSON 配列文字列（LLM 候補）
/// 戻り値: JSON `["候補1","候補2",...]`
/// `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_merge_candidates(
    handle: *mut c_void,
    llm_json: *const c_char,
    limit: u32,
) -> *mut c_char {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    let s = unsafe { from_cstr(llm_json) };
    let llm_cands: Vec<String> = serde_json::from_str(s).unwrap_or_default();
    // 辞書検索を直接デバッグ
    let hiragana = engine.hiragana_text().to_string();
    let dict_debug = if let Some(store) = engine.dict_store_ref() {
        let d = store.lookup_dict(&hiragana, limit as usize);
        format!("lookup_dict({:?})={:?}", hiragana, d)
    } else {
        "dict_store=None".to_string()
    };
    set_dict_status(dict_debug);
    let merged = engine.merge_candidates(llm_cands, limit as usize);
    let json = serde_json::to_string(&merged).unwrap_or_else(|_| "[]".into());
    unsafe { to_cstr(json) }
}

/// 指定 reading で dict + LLM 候補をマージして返す。
/// 戻り値: JSON `["候補1","候補2",...]`
/// `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_merge_candidates_for_reading(
    handle: *mut c_void,
    reading: *const c_char,
    llm_json: *const c_char,
    limit: u32,
) -> *mut c_char {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    let reading = unsafe { from_cstr(reading) };
    let s = unsafe { from_cstr(llm_json) };
    let llm_cands: Vec<String> = serde_json::from_str(s).unwrap_or_default();
    let dict_debug = if let Some(store) = engine.dict_store_ref() {
        let d = store.lookup_dict(reading, limit as usize);
        format!("lookup_dict({:?})={:?}", reading, d)
    } else {
        "dict_store=None".to_string()
    };
    set_dict_status(dict_debug);
    let merged = engine.merge_candidates_for_reading(reading, llm_cands, limit as usize);
    let json = serde_json::to_string(&merged).unwrap_or_else(|_| "[]".into());
    unsafe { to_cstr(json) }
}

// ─── 初期化（非同期）──────────────────────────────────────────────────────────

/// モデル（漢字変換 LLM）のロードをバックグラウンドで開始する。
///
/// `engine.kanji = None` でも、converter が BG 変換のため conv_cache 側に
/// 出張中（pending / Running / Done）ならモデルは存在するのでロードしない。
/// かつてはこの区別がなく、変換中や commit 直後の Activate のたびに
/// モデルを二重ロードしていた（7月ログで engine::init ×800 回）。
/// 併せて `MODEL_LOADING` ガードで並行 spawn を抑止する（`DICT_LOADING` と同形式）。
#[unsafe(no_mangle)]
pub extern "C" fn engine_start_load_model(handle: *mut c_void) {
    static MODEL_LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    if engine.is_kanji_ready() {
        return;
    }
    // converter が conv_cache の Done に眠っているだけなら回収して復帰（ロード不要）
    if let Some(conv) = crate::conv_cache::try_reclaim_done() {
        tracing::info!("start_load_model: converter reclaimed from conv_cache (no reload)");
        engine.set_kanji_converter(conv);
        return;
    }
    // pending / Running 中もモデルは存在する。回収は bg_start / bg_take_candidates が行う
    if crate::conv_cache::has_converter() {
        tracing::debug!("start_load_model: converter busy in conv_cache (no reload)");
        return;
    }
    let config = engine.get_config().clone();
    let fingerprint = config_fingerprint(&config);
    // 前回のロードが完了して注入待ち（engine_poll_model_ready 待ち）なら何もしない。
    // ただし config が変わっていたら古い converter を破棄してロードし直す。
    if let Ok(mut g) = PENDING_CONVERTER.lock() {
        match &*g {
            Some((fp, _)) if *fp == fingerprint => return,
            Some(_) => {
                tracing::info!("start_load_model: discarding pending converter (config changed)");
                *g = None;
            }
            None => {}
        }
    }
    // 多重 spawn 防止
    if MODEL_LOADING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    set_last_error(String::new()); // clear
    std::thread::spawn(move || {
        set_last_error("model loading...".to_string());
        match RakunEngine::build_converter(&config) {
            Ok(converter) => {
                set_last_error("model load complete".to_string());
                let _ = PENDING_CONVERTER
                    .lock()
                    .map(|mut g| *g = Some((fingerprint, converter)));
            }
            Err(e) => {
                let msg = format!("model load failed: {e}");
                set_last_error(msg);
            }
        }
        MODEL_LOADING.store(false, std::sync::atomic::Ordering::Release);
    });
}

// pending converter をスレッド間で渡すための一時置き場。
// ビルド時の config フィンガープリントを添えて保存し、Reload で config が
// 変わった場合に古い converter を誤って注入しないようにする。
use crate::kanji::KanaKanjiConverter;
use std::sync::{LazyLock, Mutex};
static PENDING_CONVERTER: LazyLock<Mutex<Option<(String, KanaKanjiConverter)>>> =
    LazyLock::new(|| Mutex::new(None));

fn config_fingerprint(config: &EngineConfig) -> String {
    serde_json::to_string(config).unwrap_or_default()
}

/// モデルが pending_converter に届いているか確認し、届いていたら engine に注入する。
/// 戻り値: true = 注入した（is_kanji_ready() が true になった）
#[unsafe(no_mangle)]
pub extern "C" fn engine_poll_model_ready(handle: *mut c_void) -> bool {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    if engine.is_kanji_ready() {
        // 二重ロード等で残った注入待ち converter は破棄する
        if let Ok(mut g) = PENDING_CONVERTER.try_lock() {
            if g.take().is_some() {
                tracing::info!("poll_model_ready: discarded stale pending converter");
            }
        }
        return true;
    }
    if let Ok(mut g) = PENDING_CONVERTER.try_lock() {
        // config が一致する converter のみ注入する（Reload 直後の取り違え防止）
        let fingerprint = config_fingerprint(engine.get_config());
        match g.take() {
            Some((fp, conv)) if fp == fingerprint => {
                engine.set_kanji_converter(conv);
                tracing::info!("converter injected into engine");
                return true;
            }
            Some(_) => {
                tracing::info!("poll_model_ready: discarded pending converter (config mismatch)");
            }
            None => {}
        }
    }
    false
}

/// 辞書のロードをバックグラウンドで開始する。
#[unsafe(no_mangle)]
pub extern "C" fn engine_start_load_dict(handle: *mut c_void) {
    static DICT_LOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    let engine = unsafe { &*(handle as *const RakunEngine) };
    if engine.is_dict_ready() {
        return;
    }

    // 多重 spawn 防止
    if DICT_LOADING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }

    // 前回セッションの残留データをクリア
    let _ = PENDING_DICT.lock().map(|mut g| *g = None);

    // DictStore をロードしてエンジンに渡す（同様に poll パターン）
    std::thread::spawn(move || {
        use crate::dict::loader::{LoadResult, load_dict};

        set_dict_status(format!(
            "starting: build={}",
            option_env!("RAKUKAN_ENGINE_BUILD_TIME").unwrap_or("unknown")
        ));

        match load_dict() {
            LoadResult::Ok(store) => {
                let user_n = store.user_entry_count();
                let _ = PENDING_DICT.lock().map(|mut g| *g = Some(store));
                set_dict_status(format!("ok: mozc=true user_entries={}", user_n));
            }
            LoadResult::Failed { step, reason } => {
                set_dict_status(format!("failed at [{}]: {}", step, reason));
                tracing::warn!("dict load failed at [{}]: {}", step, reason);
            }
        }
        DICT_LOADING.store(false, std::sync::atomic::Ordering::Release);
    });
}

static PENDING_DICT: LazyLock<Mutex<Option<crate::DictStore>>> = LazyLock::new(|| Mutex::new(None));

/// 辞書が pending に届いていたら engine に注入する。
/// 戻り値: true = 注入した
#[unsafe(no_mangle)]
pub extern "C" fn engine_poll_dict_ready(handle: *mut c_void) -> bool {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    if engine.is_dict_ready() {
        return false;
    }
    if let Ok(mut g) = PENDING_DICT.try_lock() {
        if let Some(store) = g.take() {
            engine.set_dict_store(store);
            set_dict_status("injected: mozc=true".to_string());
            return true;
        }
    }
    false
}

// ─── ステータス ────────────────────────────────────────────────────────────────

/// kanji 変換器が準備できているか
#[unsafe(no_mangle)]
pub extern "C" fn engine_is_kanji_ready(handle: *mut c_void) -> bool {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    engine.is_kanji_ready()
}

/// 辞書が準備できているか
#[unsafe(no_mangle)]
pub extern "C" fn engine_is_dict_ready(handle: *mut c_void) -> bool {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    engine.is_dict_ready()
}

/// バックエンドラベル（例: "CUDA", "Vulkan", "CPU"）
/// 戻り値は `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_backend_label(handle: *mut c_void) -> *mut c_char {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    unsafe { to_cstr(engine.backend_label()) }
}

/// 利用可能なモデル一覧を JSON で返す。
/// `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_available_models_json() -> *mut c_char {
    let models = RakunEngine::available_models();
    let json = serde_json::to_string(&models).unwrap_or_else(|_| "[]".into());
    unsafe { to_cstr(json) }
}

/// n_gpu_layers 設定値を返す（診断用）
#[unsafe(no_mangle)]
pub extern "C" fn engine_n_gpu_layers(handle: *mut c_void) -> u32 {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    engine.get_config().n_gpu_layers
}

/// main_gpu 設定値を返す（診断用）
#[unsafe(no_mangle)]
pub extern "C" fn engine_main_gpu(handle: *mut c_void) -> i32 {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    engine.get_config().main_gpu
}

/// 選択した候補をユーザー辞書に学習する。
/// reading: ひらがな読み、surface: 確定した漢字表記
#[unsafe(no_mangle)]
pub extern "C" fn engine_learn(
    handle: *mut c_void,
    reading: *const c_char,
    surface: *const c_char,
) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    let reading = unsafe { from_cstr(reading) }.to_string();
    let surface = unsafe { from_cstr(surface) }.to_string();
    if reading.is_empty() || surface.is_empty() {
        return;
    }
    engine.learn(&reading, &surface);
}

/// 辞書ガードなしで学習する（候補ウィンドウからの明示選択、案C）。
#[unsafe(no_mangle)]
pub extern "C" fn engine_learn_force(
    handle: *mut c_void,
    reading: *const c_char,
    surface: *const c_char,
) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    let reading = unsafe { from_cstr(reading) }.to_string();
    let surface = unsafe { from_cstr(surface) }.to_string();
    if reading.is_empty() || surface.is_empty() {
        return;
    }
    engine.learn_force(&reading, &surface);
}

// ─── 最後のエラーメッセージ（診断用）────────────────────────────────────────

static LAST_ERROR: LazyLock<Mutex<String>> = LazyLock::new(|| Mutex::new(String::new()));
static DICT_STATUS: LazyLock<Mutex<String>> =
    LazyLock::new(|| Mutex::new("not started".to_string()));

fn set_last_error(msg: String) {
    if let Ok(mut g) = LAST_ERROR.lock() {
        *g = msg;
    }
}

fn set_dict_status(msg: String) {
    if let Ok(mut g) = DICT_STATUS.lock() {
        *g = msg;
    }
}

/// 最後に発生したエラーメッセージを返す（TSF 側ログ用）
/// 呼び出し側が engine_free_string で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_last_error() -> *mut c_char {
    let msg = LAST_ERROR.lock().map(|g| g.clone()).unwrap_or_default();
    unsafe { to_cstr(msg) }
}

/// 辞書ロード状態を返す（TSF 側ログ用）
#[unsafe(no_mangle)]
pub extern "C" fn engine_dict_status() -> *mut c_char {
    let msg = DICT_STATUS.lock().map(|g| g.clone()).unwrap_or_default();
    unsafe { to_cstr(msg) }
}
