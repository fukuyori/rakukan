//! rakukan-engine DLL の動的ローダー
//!
//! `DynEngine` は `RakunEngine` と同じ API を持ち、実行時に選択された
//! バックエンド DLL（cuda/vulkan/cpu）に処理を委譲する。
//!
//! # バックエンド選択順
//! 1. `config.toml` の `gpu_backend` キー
//! 2. `backend.json` の `backend` キー
//! 3. デフォルト: `cpu`
//!
//! # DLL ファイル名
//! `rakukan_engine_<backend>.dll` がインストールディレクトリに存在すること。

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};

// ─── EngineVTable ──────────────────────────────────────────────────────────────
// DLL からロードした関数ポインタのコレクション

struct EngineVTable {
    // ライフサイクル
    create:               unsafe extern "C" fn(*const c_char) -> *mut c_void,
    destroy:              unsafe extern "C" fn(*mut c_void),
    free_string:          unsafe extern "C" fn(*mut c_char),

    // 文字入力
    push_char:            unsafe extern "C" fn(*mut c_void, u32) -> u8,
    backspace:            unsafe extern "C" fn(*mut c_void) -> bool,
    flush_n:              unsafe extern "C" fn(*mut c_void) -> bool,

    // プリエディット
    preedit_display:      unsafe extern "C" fn(*mut c_void) -> *mut c_char,
    preedit_is_empty:     unsafe extern "C" fn(*mut c_void) -> bool,
    hiragana_text:        unsafe extern "C" fn(*mut c_void) -> *mut c_char,
    committed_text:       unsafe extern "C" fn(*mut c_void) -> *mut c_char,

    // BG 変換
    bg_start:             unsafe extern "C" fn(*mut c_void, u32) -> bool,
    bg_status:            unsafe extern "C" fn(*mut c_void) -> *const c_char,
    bg_take_candidates:   unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
    bg_reclaim:           unsafe extern "C" fn(*mut c_void),
    bg_wait_ms:           unsafe extern "C" fn(*mut c_void, u64) -> u8,

    // 確定・リセット
    commit:               unsafe extern "C" fn(*mut c_void, *const c_char),
    commit_as_hiragana:   unsafe extern "C" fn(*mut c_void),
    reset_preedit:        unsafe extern "C" fn(*mut c_void),
    force_preedit:        unsafe extern "C" fn(*mut c_void, *const c_char),
    reset_all:            unsafe extern "C" fn(*mut c_void),

    // 変換（同期）
    convert_sync:         unsafe extern "C" fn(*mut c_void) -> *mut c_char,
    merge_candidates:     unsafe extern "C" fn(*mut c_void, *const c_char, u32) -> *mut c_char,

    // 非同期初期化
    start_load_model:     unsafe extern "C" fn(*mut c_void),
    poll_model_ready:     unsafe extern "C" fn(*mut c_void) -> bool,
    start_load_dict:      unsafe extern "C" fn(*mut c_void),
    poll_dict_ready:      unsafe extern "C" fn(*mut c_void) -> bool,

    // ステータス
    is_kanji_ready:       unsafe extern "C" fn(*mut c_void) -> bool,
    is_dict_ready:        unsafe extern "C" fn(*mut c_void) -> bool,
    backend_label:        unsafe extern "C" fn(*mut c_void) -> *mut c_char,
    n_gpu_layers:         unsafe extern "C" fn(*mut c_void) -> u32,
    main_gpu:             unsafe extern "C" fn(*mut c_void) -> i32,

    // Static
    available_models_json: unsafe extern "C" fn() -> *mut c_char,

    // 学習
    learn:                unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char),

    // 診断
    last_error:           unsafe extern "C" fn() -> *mut c_char,
}

// ─── DLL ロード ────────────────────────────────────────────────────────────────

macro_rules! load_sym {
    ($lib:expr, $name:literal) => {{
        let sym: Symbol<_> = unsafe {
            $lib.get($name)
                .context(concat!("symbol not found: ", stringify!($name)))?
        };
        *sym
    }};
}

impl EngineVTable {
    unsafe fn load(lib: &Library) -> Result<Self> {
        Ok(EngineVTable {
            create:               load_sym!(lib, b"engine_create\0"),
            destroy:              load_sym!(lib, b"engine_destroy\0"),
            free_string:          load_sym!(lib, b"engine_free_string\0"),
            push_char:            load_sym!(lib, b"engine_push_char\0"),
            backspace:            load_sym!(lib, b"engine_backspace\0"),
            flush_n:              load_sym!(lib, b"engine_flush_n\0"),
            preedit_display:      load_sym!(lib, b"engine_preedit_display\0"),
            preedit_is_empty:     load_sym!(lib, b"engine_preedit_is_empty\0"),
            hiragana_text:        load_sym!(lib, b"engine_hiragana_text\0"),
            committed_text:       load_sym!(lib, b"engine_committed_text\0"),
            bg_start:             load_sym!(lib, b"engine_bg_start\0"),
            bg_status:            load_sym!(lib, b"engine_bg_status\0"),
            bg_take_candidates:   load_sym!(lib, b"engine_bg_take_candidates\0"),
            bg_reclaim:           load_sym!(lib, b"engine_bg_reclaim\0"),
            bg_wait_ms:           load_sym!(lib, b"engine_bg_wait_ms\0"),
            commit:               load_sym!(lib, b"engine_commit\0"),
            commit_as_hiragana:   load_sym!(lib, b"engine_commit_as_hiragana\0"),
            reset_preedit:        load_sym!(lib, b"engine_reset_preedit\0"),
            force_preedit:        load_sym!(lib, b"engine_force_preedit\0"),
            reset_all:            load_sym!(lib, b"engine_reset_all\0"),
            convert_sync:         load_sym!(lib, b"engine_convert_sync\0"),
            merge_candidates:     load_sym!(lib, b"engine_merge_candidates\0"),
            start_load_model:     load_sym!(lib, b"engine_start_load_model\0"),
            poll_model_ready:     load_sym!(lib, b"engine_poll_model_ready\0"),
            start_load_dict:      load_sym!(lib, b"engine_start_load_dict\0"),
            poll_dict_ready:      load_sym!(lib, b"engine_poll_dict_ready\0"),
            is_kanji_ready:       load_sym!(lib, b"engine_is_kanji_ready\0"),
            is_dict_ready:        load_sym!(lib, b"engine_is_dict_ready\0"),
            backend_label:        load_sym!(lib, b"engine_backend_label\0"),
            n_gpu_layers:         load_sym!(lib, b"engine_n_gpu_layers\0"),
            main_gpu:             load_sym!(lib, b"engine_main_gpu\0"),
            available_models_json: load_sym!(lib, b"engine_available_models_json\0"),
            learn:                 load_sym!(lib, b"engine_learn\0"),
            last_error:            load_sym!(lib, b"engine_last_error\0"),
        })
    }
}

// ─── DynEngine ────────────────────────────────────────────────────────────────

/// 動的にロードされた rakukan-engine DLL のラッパー。
/// `RakunEngine` と同じ API を提供する。
pub struct DynEngine {
    handle:  *mut c_void,
    vtable:  EngineVTable,
    _lib:    Arc<Library>,   // DLL をアンロードしないよう保持
}

// ロードした DLL は同一スレッドから使う（TSF STA モデル）
unsafe impl Send for DynEngine {}
unsafe impl Sync for DynEngine {}

impl DynEngine {
    /// 指定した DLL パスからエンジンを生成する。
    pub fn from_dll(dll_path: &Path, config_json: Option<&str>) -> Result<Self> {
        tracing::info!("Loading engine DLL: {}", dll_path.display());
        let lib = unsafe { Library::new(dll_path) }
            .with_context(|| format!("DLL load failed: {}", dll_path.display()))?;
        let vtable = unsafe { EngineVTable::load(&lib) }?;

        let handle = unsafe {
            let cfg = config_json.and_then(|s| CString::new(s).ok());
            let ptr = cfg.as_ref().map(|c| c.as_ptr()).unwrap_or(std::ptr::null());
            (vtable.create)(ptr)
        };
        if handle.is_null() {
            bail!("engine_create returned null");
        }

        Ok(DynEngine { handle, vtable, _lib: Arc::new(lib) })
    }

    /// バックエンドを自動検出して適切な DLL をロードする。
    ///
    /// `install_dir`: rakukan DLL が配置されているディレクトリ
    /// `config_json`: EngineConfig JSON（null の場合はデフォルト）
    pub fn load_auto(install_dir: &Path, config_json: Option<&str>) -> Result<Self> {
        let backend = detect_backend();
        tracing::info!("Selected backend: {backend}");
        Self::load_backend(install_dir, &backend, config_json)
    }

    /// 指定バックエンド名の DLL をロードする。
    pub fn load_backend(install_dir: &Path, backend: &str, config_json: Option<&str>) -> Result<Self> {
        let dll_name = format!("rakukan_engine_{}.dll", backend);
        let dll_path = install_dir.join(&dll_name);
        if !dll_path.exists() {
            // フォールバック: cpu
            if backend != "cpu" {
                tracing::warn!("{} not found, falling back to cpu", dll_name);
                return Self::load_backend(install_dir, "cpu", config_json);
            }
            bail!("engine DLL not found: {}", dll_path.display());
        }
        Self::from_dll(&dll_path, config_json)
    }

    // ── ヘルパー ───────────────────────────────────────────────────────────

    /// DLL が返した C 文字列を Rust String に変換して解放する
    unsafe fn take_cstr(&self, ptr: *mut c_char) -> Option<String> {
        if ptr.is_null() { return None; }
        let s = unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() };
        unsafe { (self.vtable.free_string)(ptr) };
        Some(s)
    }

    /// Rust &str → CString（一時的な変換）
    fn to_cstring(s: &str) -> CString {
        CString::new(s.replace('\0', "")).unwrap_or_default()
    }

    // ── 文字入力 ────────────────────────────────────────────────────────────

    pub fn push_char(&mut self, c: char) {
        unsafe { (self.vtable.push_char)(self.handle, c as u32); }
    }

    pub fn backspace(&mut self) -> bool {
        unsafe { (self.vtable.backspace)(self.handle) }
    }

    pub fn flush_pending_n(&mut self) -> bool {
        unsafe { (self.vtable.flush_n)(self.handle) }
    }

    // ── プリエディット状態 ──────────────────────────────────────────────────

    pub fn preedit_display(&self) -> String {
        unsafe {
            let ptr = (self.vtable.preedit_display)(self.handle);
            self.take_cstr(ptr).unwrap_or_default()
        }
    }

    pub fn preedit_is_empty(&self) -> bool {
        unsafe { (self.vtable.preedit_is_empty)(self.handle) }
    }

    pub fn hiragana_text(&self) -> String {
        unsafe {
            let ptr = (self.vtable.hiragana_text)(self.handle);
            self.take_cstr(ptr).unwrap_or_default()
        }
    }

    pub fn committed_text(&self) -> String {
        unsafe {
            let ptr = (self.vtable.committed_text)(self.handle);
            self.take_cstr(ptr).unwrap_or_default()
        }
    }

    // ── BG 変換 ─────────────────────────────────────────────────────────────

    /// BG 変換を起動する。true = 起動した
    pub fn bg_start(&mut self, n_cands: usize) -> bool {
        unsafe { (self.vtable.bg_start)(self.handle, n_cands as u32) }
    }

    /// BG 状態文字列（診断用）
    pub fn bg_status(&self) -> &'static str {
        unsafe {
            let ptr = (self.vtable.bg_status)(self.handle);
            // static str なので解放不要。ASCII 限定なので安全。
            CStr::from_ptr(ptr).to_str().unwrap_or("unknown")
        }
    }

    /// key が一致する BG 変換結果を取得する。
    pub fn bg_take_candidates(&mut self, key: &str) -> Option<Vec<String>> {
        let ckey = Self::to_cstring(key);
        unsafe {
            let ptr = (self.vtable.bg_take_candidates)(self.handle, ckey.as_ptr());
            let json = self.take_cstr(ptr)?;
            serde_json::from_str(&json).ok()
        }
    }

    /// Done 状態の converter を engine に戻す
    pub fn bg_reclaim(&mut self) {
        unsafe { (self.vtable.bg_reclaim)(self.handle); }
    }

    /// BG 変換完了を最大 `timeout_ms` ミリ秒ブロック待機する。
    /// Done になれば `true`、タイムアウトなら `false`。
    pub fn bg_wait_ms(&mut self, timeout_ms: u64) -> bool {
        unsafe { (self.vtable.bg_wait_ms)(self.handle, timeout_ms) != 0 }
    }

    // ── 確定・リセット ──────────────────────────────────────────────────────

    pub fn commit(&mut self, text: &str) {
        let cs = Self::to_cstring(text);
        unsafe { (self.vtable.commit)(self.handle, cs.as_ptr()); }
    }

    pub fn commit_as_hiragana(&mut self) {
        unsafe { (self.vtable.commit_as_hiragana)(self.handle); }
    }

    pub fn reset_preedit(&mut self) {
        unsafe { (self.vtable.reset_preedit)(self.handle); }
    }

    pub fn force_preedit(&mut self, text: String) {
        let c = std::ffi::CString::new(text.replace('\0', "")).unwrap_or_default();
        unsafe { (self.vtable.force_preedit)(self.handle, c.as_ptr()); }
    }

    pub fn reset_all(&mut self) {
        unsafe { (self.vtable.reset_all)(self.handle); }
    }

    // ── 変換（同期フォールバック）──────────────────────────────────────────

    pub fn convert_sync(&mut self) -> Vec<String> {
        unsafe {
            let ptr = (self.vtable.convert_sync)(self.handle);
            match self.take_cstr(ptr) {
                Some(json) => serde_json::from_str(&json).unwrap_or_default(),
                None       => vec![],
            }
        }
    }

    pub fn merge_candidates(&self, llm_cands: Vec<String>, limit: usize) -> Vec<String> {
        let json = serde_json::to_string(&llm_cands).unwrap_or_else(|_| "[]".into());
        let cjson = Self::to_cstring(&json);
        unsafe {
            let ptr = (self.vtable.merge_candidates)(self.handle, cjson.as_ptr(), limit as u32);
            match self.take_cstr(ptr) {
                Some(s) => serde_json::from_str(&s).unwrap_or_default(),
                None    => vec![],
            }
        }
    }

    // ── 非同期初期化 ────────────────────────────────────────────────────────

    pub fn start_load_model(&mut self) {
        unsafe { (self.vtable.start_load_model)(self.handle); }
    }

    /// true = モデルが新たに利用可能になった（langbar 更新トリガー）
    pub fn poll_model_ready(&mut self) -> bool {
        unsafe { (self.vtable.poll_model_ready)(self.handle) }
    }

    pub fn start_load_dict(&mut self) {
        unsafe { (self.vtable.start_load_dict)(self.handle); }
    }

    /// true = 辞書が新たに利用可能になった
    pub fn poll_dict_ready(&mut self) -> bool {
        unsafe { (self.vtable.poll_dict_ready)(self.handle) }
    }

    // ── ステータス ──────────────────────────────────────────────────────────

    pub fn is_kanji_ready(&self) -> bool {
        unsafe { (self.vtable.is_kanji_ready)(self.handle) }
    }

    pub fn is_dict_ready(&self) -> bool {
        unsafe { (self.vtable.is_dict_ready)(self.handle) }
    }

    pub fn backend_label(&self) -> String {
        unsafe {
            let ptr = (self.vtable.backend_label)(self.handle);
            self.take_cstr(ptr).unwrap_or_else(|| "unknown".into())
        }
    }

    pub fn n_gpu_layers(&self) -> u32 {
        unsafe { (self.vtable.n_gpu_layers)(self.handle) }
    }

    pub fn main_gpu(&self) -> i32 {
        unsafe { (self.vtable.main_gpu)(self.handle) }
    }

    pub fn available_models_json(&self) -> String {
        unsafe {
            let ptr = (self.vtable.available_models_json)();
            self.take_cstr(ptr).unwrap_or_else(|| "[]".into())
        }
    }

    pub fn learn(&mut self, reading: &str, surface: &str) {
        let r = Self::to_cstring(reading);
        let s = Self::to_cstring(surface);
        unsafe { (self.vtable.learn)(self.handle, r.as_ptr(), s.as_ptr()); }
    }

    /// エンジン DLL 側の最後のエラー/ステータスメッセージを返す（診断用）
    pub fn last_error(&self) -> String {
        let ptr = unsafe { (self.vtable.last_error)() };
        if ptr.is_null() { return String::new(); }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.vtable.free_string)(ptr) };
        s
    }
}

impl Drop for DynEngine {
    fn drop(&mut self) {
        unsafe { (self.vtable.destroy)(self.handle); }
    }
}

// ─── バックエンド自動検出 ──────────────────────────────────────────────────────

/// config.toml → backend.json → "cpu" の優先順でバックエンドを返す
fn detect_backend() -> String {
    // 1. config.toml
    if let Some(b) = read_config_toml_backend() {
        tracing::info!("backend from config.toml: {b}");
        return b;
    }
    // 2. backend.json
    if let Some(b) = read_backend_json() {
        tracing::info!("backend from backend.json: {b}");
        return b;
    }
    // 3. デフォルト
    tracing::warn!("backend not configured, using cpu");
    "cpu".into()
}

fn appdata_rakukan() -> Option<PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(PathBuf::from(appdata).join("rakukan"))
}

fn read_config_toml_backend() -> Option<String> {
    let path = appdata_rakukan()?.join("config.toml");
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') { continue; }
        if let Some(rest) = line.strip_prefix("gpu_backend") {
            let rest = rest.trim().trim_start_matches('=').trim();
            let val = rest.split('#').next().unwrap_or("").trim().trim_matches('"').trim_matches('\'');
            if matches!(val, "cuda" | "vulkan" | "cpu") {
                return Some(val.to_string());
            }
        }
    }
    None
}

fn read_backend_json() -> Option<String> {
    let path = appdata_rakukan()?.join("backend.json");
    let text = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    let b = v["backend"].as_str()?.to_string();
    if matches!(b.as_str(), "cuda" | "vulkan" | "cpu") { Some(b) } else { None }
}

// ─── DLL ディレクトリ検出 ──────────────────────────────────────────────────────

/// rakukan DLL がインストールされているディレクトリを返す。
/// `rakukan_tsf.dll` と同じディレクトリを想定する。
/// Windows では `GetModuleFileNameW` で取得する。
#[cfg(target_os = "windows")]
pub fn install_dir() -> Option<PathBuf> {
    let appdata = std::env::var("LOCALAPPDATA").ok()?;
    Some(PathBuf::from(appdata).join("rakukan"))
}

#[cfg(not(target_os = "windows"))]
pub fn install_dir() -> Option<PathBuf> {
    Some(PathBuf::from("/usr/local/lib/rakukan"))
}
