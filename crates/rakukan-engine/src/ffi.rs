//! rakukan-engine DLL 用 C ABI エクスポート
//!
//! 3 種類の DLL（cuda / vulkan / cpu）として同じ関数名でビルドされる。
//! rakukan-tsf は libloading でこれらのいずれかを実行時にロードする。
//!
//! # メモリ管理規約
//! - `*mut c_char` を返す関数はすべて `engine_free_string` で解放すること
//! - `*mut c_void` ハンドルは `engine_destroy` で解放すること
//! - caller が渡す `*const c_char` は関数呼び出しの間だけ有効であればよい

use std::ffi::{CStr, CString, c_char, c_void};
use crate::{RakunEngine, EngineConfig};

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
    if ptr.is_null() { return ""; }
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
        unsafe { drop(Box::from_raw(handle as *mut RakunEngine)); }
    }
}

/// `engine_create` / `engine_free_string` が返した文字列を解放する。
#[unsafe(no_mangle)]
pub extern "C" fn engine_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe { drop(CString::from_raw(s)); }
    }
}

// ─── 文字入力 ──────────────────────────────────────────────────────────────────

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

/// BG 変換状態を返す: "idle" / "running" / "done" / "locked"
/// 戻り値は static str なので解放不要。
#[unsafe(no_mangle)]
pub extern "C" fn engine_bg_status(handle: *mut c_void) -> *const c_char {
    let _engine = unsafe { &*(handle as *const RakunEngine) };
    let s = crate::conv_cache::status();
    s.as_ptr() as *const c_char  // static lifetime - 解放不要
}

/// key が一致する BG 変換結果を取得する。
/// 戻り値: JSON 文字列 `["候補1","候補2",...]` または null（未完了/不一致）
/// 戻り値は `engine_free_string` で解放すること。
#[unsafe(no_mangle)]
pub extern "C" fn engine_bg_take_candidates(handle: *mut c_void, key: *const c_char) -> *mut c_char {
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
    let done = crate::conv_cache::wait_done_timeout(
        std::time::Duration::from_millis(timeout_ms),
    );
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
    let s = unsafe { std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned() };
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
    handle:   *mut c_void,
    llm_json: *const c_char,
    limit:    u32,
) -> *mut c_char {
    let engine = unsafe { &*(handle as *const RakunEngine) };
    let s = unsafe { from_cstr(llm_json) };
    let llm_cands: Vec<String> = serde_json::from_str(s).unwrap_or_default();
    let merged = engine.merge_candidates(llm_cands, limit as usize);
    let json = serde_json::to_string(&merged).unwrap_or_else(|_| "[]".into());
    unsafe { to_cstr(json) }
}

// ─── 初期化（非同期）──────────────────────────────────────────────────────────

/// モデル（漢字変換 LLM）のロードをバックグラウンドで開始する。
#[unsafe(no_mangle)]
pub extern "C" fn engine_start_load_model(handle: *mut c_void) {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    if engine.is_kanji_ready() { return; }
    let config = engine.get_config().clone();
    std::thread::spawn(move || {
        match RakunEngine::build_converter(&config) {
            Ok(converter) => {
                // エンジンが既に破棄されている可能性があるため、グローバル経由で注入できない。
                // 呼び出し元 (rakukan-tsf) が engine_is_kanji_ready をポーリングして
                // engine_inject_converter を呼ぶパターンを取る。
                // ここでは conv_cache 経由で保管しておく。
                tracing::info!("model load complete");
                // 仮置き: converter を持ったままスレッドが終わる
                // → engine_poll_model_ready / engine_inject_converter で回収
                let _ = PENDING_CONVERTER.lock().map(|mut g| *g = Some(converter));
            }
            Err(e) => tracing::warn!("model load failed: {e}"),
        }
    });
}

// pending converter をスレッド間で渡すための一時置き場
use std::sync::{Mutex, LazyLock};
use crate::kanji::KanaKanjiConverter;
static PENDING_CONVERTER: LazyLock<Mutex<Option<KanaKanjiConverter>>> =
    LazyLock::new(|| Mutex::new(None));

/// モデルが pending_converter に届いているか確認し、届いていたら engine に注入する。
/// 戻り値: true = 注入した（is_kanji_ready() が true になった）
#[unsafe(no_mangle)]
pub extern "C" fn engine_poll_model_ready(handle: *mut c_void) -> bool {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    if engine.is_kanji_ready() { return false; }  // already ready
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
    let engine = unsafe { &*(handle as *const RakunEngine) };
    if engine.is_dict_ready() { return; }

    // DictStore をロードしてエンジンに渡す（同様に poll パターン）
    std::thread::spawn(move || {
        use crate::{DictStore, find_skk_jisyo, find_mozc_dict, user_dict_path};
        let skk_paths = find_skk_jisyo();
        let skk_refs: Vec<&std::path::Path> = skk_paths.iter().map(|p| p.as_path()).collect();
        let mozc_path = find_mozc_dict();
        let mozc_ref  = mozc_path.as_deref();
        let user_path = user_dict_path();
        let user_ref  = user_path.as_deref();
        match DictStore::load(user_ref, mozc_ref, &skk_refs) {
            Ok(store) => {
                let _ = PENDING_DICT.lock().map(|mut g| *g = Some(store));
                tracing::info!("dict load complete");
            }
            Err(e) => tracing::warn!("dict load failed: {e}"),
        }
    });
}

static PENDING_DICT: LazyLock<Mutex<Option<crate::DictStore>>> =
    LazyLock::new(|| Mutex::new(None));

/// 辞書が pending に届いていたら engine に注入する。
/// 戻り値: true = 注入した
#[unsafe(no_mangle)]
pub extern "C" fn engine_poll_dict_ready(handle: *mut c_void) -> bool {
    let engine = unsafe { &mut *(handle as *mut RakunEngine) };
    if engine.is_dict_ready() { return false; }
    if let Ok(mut g) = PENDING_DICT.try_lock() {
        if let Some(store) = g.take() {
            engine.set_dict_store(store);
            tracing::info!("dict store injected into engine");
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
pub extern "C" fn engine_learn(handle: *mut c_void, reading: *const c_char, surface: *const c_char) {
    let engine  = unsafe { &mut *(handle as *mut RakunEngine) };
    let reading = unsafe { from_cstr(reading) }.to_string();
    let surface = unsafe { from_cstr(surface) }.to_string();
    if reading.is_empty() || surface.is_empty() { return; }
    engine.learn(&reading, &surface);
}
