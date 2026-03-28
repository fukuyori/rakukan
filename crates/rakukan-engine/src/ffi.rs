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

pub const ENGINE_ABI_VERSION: u32 = 2;

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

/// surface を分節して JSON 配列で返す。
/// 戻り値: JSON `["変換","が"]`
/// `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_segment_surface(
    handle: *mut c_void,
    surface: *const c_char,
) -> *mut c_char {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    let surface = unsafe { from_cstr(surface) };
    let segments = engine.segment_surface(surface);
    let json = serde_json::to_string(&segments).unwrap_or_else(|_| "[]".into());
    unsafe { to_cstr(json) }
}

// ─── 初期化（非同期）──────────────────────────────────────────────────────────

/// モデル（漢字変換 LLM）のロードをバックグラウンドで開始する。
#[unsafe(no_mangle)]
pub extern "C" fn engine_start_load_model(handle: *mut c_void) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    if engine.is_kanji_ready() {
        return;
    }
    set_last_error(String::new()); // clear
    let config = engine.get_config().clone();
    std::thread::spawn(move || {
        set_last_error("model loading...".to_string());
        match RakunEngine::build_converter(&config) {
            Ok(converter) => {
                set_last_error("model load complete".to_string());
                let _ = PENDING_CONVERTER.lock().map(|mut g| *g = Some(converter));
            }
            Err(e) => {
                let msg = format!("model load failed: {e}");
                set_last_error(msg);
            }
        }
    });
}

// pending converter をスレッド間で渡すための一時置き場
use crate::kanji::KanaKanjiConverter;
use std::sync::{LazyLock, Mutex};
static PENDING_CONVERTER: LazyLock<Mutex<Option<KanaKanjiConverter>>> =
    LazyLock::new(|| Mutex::new(None));

/// モデルが pending_converter に届いているか確認し、届いていたら engine に注入する。
/// 戻り値: true = 注入した（is_kanji_ready() が true になった）
#[unsafe(no_mangle)]
pub extern "C" fn engine_poll_model_ready(handle: *mut c_void) -> bool {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    if engine.is_kanji_ready() {
        return false;
    } // already ready
    if let Ok(mut g) = PENDING_CONVERTER.try_lock() {
        if let Some(conv) = g.take() {
            engine.set_kanji_converter(conv);
            tracing::info!("converter injected into engine");
            return true;
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
