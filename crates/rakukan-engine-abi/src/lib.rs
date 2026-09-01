//! rakukan-engine DLL の動的ローダー
//!
//! `DynEngine` は `RakunEngine` と同じ API を持ち、実行時に選択された
//! バックエンド DLL（cuda/vulkan/cpu）に処理を委譲する。
//!
//! # バックエンド選択順
//! 1. `config.toml` の `gpu_backend` キー（`cuda` / `vulkan` / `cpu` / `auto`）
//!    - 明示指定はその DLL だけを試し、失敗しても他の backend へ fallback しない。
//! 2. キー未指定または `auto` の場合は、`cuda` → `vulkan` → `cpu` の順に
//!    **実際にロードを試み**、最初に成功したものを採用する（Issue #2: DLL ファイルは
//!    あるが CUDA ランタイムが無くロードできない環境で次へ進めるようにする）。
//!
//! # DLL ファイル名
//! `rakukan_engine_<backend>.dll` がインストールディレクトリに存在すること。

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use libloading::{Library, Symbol};

const EXPECTED_ENGINE_ABI_VERSION: u32 = 9;

// ─── Segments モデル（CONVERTER_REDESIGN Phase A） ────────────────────────────

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum CandidateSource {
    Llm,
    Dict,
    History,
    Digit,
    Literal,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Candidate {
    pub surface: String,
    pub source: CandidateSource,
    pub annotation: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Segment {
    pub reading: String,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
    pub fixed: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Segments {
    pub segments: Vec<Segment>,
    pub history_size: usize,
    pub focused: usize,
}

impl Segments {
    pub fn compose_surface(&self) -> String {
        self.segments
            .iter()
            .map(|s| {
                s.candidates
                    .get(s.selected)
                    .map(|c| c.surface.as_str())
                    .unwrap_or("")
            })
            .collect()
    }

    pub fn compose_reading(&self) -> String {
        self.segments.iter().map(|s| s.reading.as_str()).collect()
    }

    pub fn empty() -> Self {
        Segments {
            segments: vec![],
            history_size: 0,
            focused: 0,
        }
    }
}

// ─── EngineVTable ──────────────────────────────────────────────────────────────
// DLL からロードした関数ポインタのコレクション

struct EngineVTable {
    // ライフサイクル
    create: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    destroy: unsafe extern "C" fn(*mut c_void),
    free_string: unsafe extern "C" fn(*mut c_char),

    // 文字入力
    push_char: unsafe extern "C" fn(*mut c_void, u32) -> u8,
    push_raw: unsafe extern "C" fn(*mut c_void, u32),
    push_fullwidth_alpha: unsafe extern "C" fn(*mut c_void, u32),
    backspace: unsafe extern "C" fn(*mut c_void) -> bool,
    flush_n: unsafe extern "C" fn(*mut c_void) -> bool,

    // プリエディット
    preedit_display: unsafe extern "C" fn(*mut c_void) -> *mut c_char,
    preedit_is_empty: unsafe extern "C" fn(*mut c_void) -> bool,
    hiragana_text: unsafe extern "C" fn(*mut c_void) -> *mut c_char,
    romaji_log_str: unsafe extern "C" fn(*mut c_void) -> *mut c_char,
    hiragana_from_romaji_log: unsafe extern "C" fn(*mut c_void) -> *mut c_char,
    committed_text: unsafe extern "C" fn(*mut c_void) -> *mut c_char,

    // BG 変換
    bg_start: unsafe extern "C" fn(*mut c_void, u32) -> bool,
    bg_status: unsafe extern "C" fn(*mut c_void) -> *const c_char,
    bg_take_candidates: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
    bg_peek_top_candidate: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
    bg_reclaim: unsafe extern "C" fn(*mut c_void),
    bg_wait_ms: unsafe extern "C" fn(*mut c_void, u64) -> u8,

    // 確定・リセット
    commit: unsafe extern "C" fn(*mut c_void, *const c_char),
    commit_as_hiragana: unsafe extern "C" fn(*mut c_void),
    reset_preedit: unsafe extern "C" fn(*mut c_void),
    force_preedit: unsafe extern "C" fn(*mut c_void, *const c_char),
    reset_all: unsafe extern "C" fn(*mut c_void),

    // 変換（同期）
    convert_sync: unsafe extern "C" fn(*mut c_void) -> *mut c_char,
    /// 旧 API。ABI の並びを保つためシンボルは読み込むが、host からは呼ばない
    /// （`merge_candidates_for_reading` を使う。Issue #9）。
    #[allow(dead_code)]
    merge_candidates: unsafe extern "C" fn(*mut c_void, *const c_char, u32) -> *mut c_char,
    merge_candidates_for_reading:
        unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char, u32) -> *mut c_char,

    // 非同期初期化
    start_load_model: unsafe extern "C" fn(*mut c_void),
    poll_model_ready: unsafe extern "C" fn(*mut c_void) -> bool,
    start_load_dict: unsafe extern "C" fn(*mut c_void),
    poll_dict_ready: unsafe extern "C" fn(*mut c_void) -> bool,

    // ステータス
    is_kanji_ready: unsafe extern "C" fn(*mut c_void) -> bool,
    is_dict_ready: unsafe extern "C" fn(*mut c_void) -> bool,
    backend_label: unsafe extern "C" fn(*mut c_void) -> *mut c_char,
    n_gpu_layers: unsafe extern "C" fn(*mut c_void) -> u32,
    main_gpu: unsafe extern "C" fn(*mut c_void) -> i32,

    // Static
    available_models_json: unsafe extern "C" fn() -> *mut c_char,

    // 学習
    learn: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char),
    learn_force: unsafe extern "C" fn(*mut c_void, *const c_char, *const c_char),

    // 診断
    last_error: unsafe extern "C" fn() -> *mut c_char,
    dict_status: unsafe extern "C" fn() -> *mut c_char,

    // 診断（任意シンボル。無い DLL は古いビルド）
    build_info: Option<unsafe extern "C" fn() -> *mut c_char>,
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

macro_rules! load_sym_opt {
    ($lib:expr, $name:literal) => {{
        let sym = unsafe { $lib.get($name) };
        sym.ok().map(|sym: Symbol<_>| *sym)
    }};
}

impl EngineVTable {
    unsafe fn load(lib: &Library) -> Result<Self> {
        let abi_version: Option<unsafe extern "C" fn() -> u32> =
            load_sym_opt!(lib, b"engine_abi_version\0");
        let Some(abi_version) = abi_version else {
            bail!(
                "installed engine DLL is outdated: missing engine_abi_version; run `cargo make full-install`"
            );
        };
        let actual = unsafe { abi_version() };
        if actual != EXPECTED_ENGINE_ABI_VERSION {
            bail!(
                "installed engine DLL ABI mismatch: expected {}, got {}; run `cargo make full-install`",
                EXPECTED_ENGINE_ABI_VERSION,
                actual
            );
        }

        Ok(EngineVTable {
            create: load_sym!(lib, b"engine_create\0"),
            destroy: load_sym!(lib, b"engine_destroy\0"),
            free_string: load_sym!(lib, b"engine_free_string\0"),
            push_char: load_sym!(lib, b"engine_push_char\0"),
            push_raw: load_sym!(lib, b"engine_push_raw\0"),
            push_fullwidth_alpha: load_sym!(lib, b"engine_push_fullwidth_alpha\0"),
            backspace: load_sym!(lib, b"engine_backspace\0"),
            flush_n: load_sym!(lib, b"engine_flush_n\0"),
            preedit_display: load_sym!(lib, b"engine_preedit_display\0"),
            preedit_is_empty: load_sym!(lib, b"engine_preedit_is_empty\0"),
            hiragana_text: load_sym!(lib, b"engine_hiragana_text\0"),
            romaji_log_str: load_sym!(lib, b"engine_romaji_log_str\0"),
            hiragana_from_romaji_log: load_sym!(lib, b"engine_hiragana_from_romaji_log\0"),
            committed_text: load_sym!(lib, b"engine_committed_text\0"),
            bg_start: load_sym!(lib, b"engine_bg_start\0"),
            bg_status: load_sym!(lib, b"engine_bg_status\0"),
            bg_take_candidates: load_sym!(lib, b"engine_bg_take_candidates\0"),
            bg_peek_top_candidate: load_sym!(lib, b"engine_bg_peek_top_candidate\0"),
            bg_reclaim: load_sym!(lib, b"engine_bg_reclaim\0"),
            bg_wait_ms: load_sym!(lib, b"engine_bg_wait_ms\0"),
            commit: load_sym!(lib, b"engine_commit\0"),
            commit_as_hiragana: load_sym!(lib, b"engine_commit_as_hiragana\0"),
            reset_preedit: load_sym!(lib, b"engine_reset_preedit\0"),
            force_preedit: load_sym!(lib, b"engine_force_preedit\0"),
            reset_all: load_sym!(lib, b"engine_reset_all\0"),
            convert_sync: load_sym!(lib, b"engine_convert_sync\0"),
            merge_candidates: load_sym!(lib, b"engine_merge_candidates\0"),
            merge_candidates_for_reading: load_sym!(lib, b"engine_merge_candidates_for_reading\0"),
            start_load_model: load_sym!(lib, b"engine_start_load_model\0"),
            poll_model_ready: load_sym!(lib, b"engine_poll_model_ready\0"),
            start_load_dict: load_sym!(lib, b"engine_start_load_dict\0"),
            poll_dict_ready: load_sym!(lib, b"engine_poll_dict_ready\0"),
            is_kanji_ready: load_sym!(lib, b"engine_is_kanji_ready\0"),
            is_dict_ready: load_sym!(lib, b"engine_is_dict_ready\0"),
            backend_label: load_sym!(lib, b"engine_backend_label\0"),
            n_gpu_layers: load_sym!(lib, b"engine_n_gpu_layers\0"),
            main_gpu: load_sym!(lib, b"engine_main_gpu\0"),
            available_models_json: load_sym!(lib, b"engine_available_models_json\0"),
            learn: load_sym!(lib, b"engine_learn\0"),
            learn_force: load_sym!(lib, b"engine_learn_force\0"),
            last_error: load_sym!(lib, b"engine_last_error\0"),
            dict_status: load_sym!(lib, b"engine_dict_status\0"),
            build_info: load_sym_opt!(lib, b"engine_build_info\0"),
        })
    }
}

// ─── DynEngine ────────────────────────────────────────────────────────────────

/// 動的にロードされた rakukan-engine DLL のラッパー。
/// `RakunEngine` と同じ API を提供する。
pub struct DynEngine {
    handle: *mut c_void,
    vtable: EngineVTable,
    _lib: Arc<Library>, // DLL をアンロードしないよう保持
}

// ロードした DLL は同一スレッドから使う（TSF STA モデル）
unsafe impl Send for DynEngine {}
unsafe impl Sync for DynEngine {}

impl DynEngine {
    /// 指定した DLL パスからエンジンを生成する。
    pub fn from_dll(dll_path: &Path, config_json: Option<&str>) -> Result<Self> {
        tracing::info!("Loading engine DLL: {}", dll_path.display());
        let lib = unsafe { Library::new(dll_path) }.map_err(|e| {
            let hint = load_failure_hint(&e);
            anyhow::anyhow!("DLL load failed: {} ({e}){hint}", dll_path.display())
        })?;
        let vtable = unsafe { EngineVTable::load(&lib) }?;

        let handle = unsafe {
            let cfg = config_json.and_then(|s| CString::new(s).ok());
            let ptr = cfg.as_ref().map(|c| c.as_ptr()).unwrap_or(std::ptr::null());
            (vtable.create)(ptr)
        };
        if handle.is_null() {
            bail!("engine_create returned null");
        }

        let engine = DynEngine {
            handle,
            vtable,
            _lib: Arc::new(lib),
        };
        engine.log_build_info(dll_path);
        Ok(engine)
    }

    /// DLL の build 識別子を読んで host のものと突き合わせ、結果をログに残す。
    ///
    /// - INFO: DLL path / ABI / version / git sha / build time / DLL 内ログの初期化結果
    /// - WARN: host と DLL が別ビルド（version または git sha が異なる）
    /// - WARN: DLL 内ログが初期化できていない（DLL 側の警告がどこにも出ない状態）
    /// - WARN: DLL が `engine_build_info` を持たない（古いビルド。突き合わせ不能）
    fn log_build_info(&self, dll_path: &Path) {
        let host = host_build_id();
        match self.build_info() {
            Some(info) => {
                tracing::info!(
                    "engine DLL loaded: path={} abi={} dll_version={} dll_git={} dll_build_time={} host_version={} host_git={} dll_log={}",
                    dll_path.display(),
                    info.abi_version,
                    info.pkg_version,
                    info.git_sha,
                    info.build_time,
                    host.pkg_version,
                    host.git_sha,
                    info.log_status
                );
                if let Some(msg) = build_mismatch(&host, &info) {
                    tracing::warn!("{msg}");
                }
                if !info.log_status.starts_with("ok") {
                    tracing::warn!(
                        "engine DLL logging is not active ({}); DLL-side warnings such as `dict load failed` will not be written anywhere",
                        info.log_status
                    );
                }
            }
            None => tracing::warn!(
                "engine DLL loaded: path={} abi={} — DLL has no engine_build_info (older build); cannot verify that host ({}@{}) and DLL come from the same build",
                dll_path.display(),
                EXPECTED_ENGINE_ABI_VERSION,
                host.pkg_version,
                host.git_sha
            ),
        }
    }

    /// DLL の build 識別子（`engine_build_info`）。古い DLL には無いので `None`。
    pub fn build_info(&self) -> Option<EngineBuildInfo> {
        let f = self.vtable.build_info?;
        let ptr = unsafe { f() };
        let json = unsafe { self.take_cstr(ptr) }?;
        match serde_json::from_str::<EngineBuildInfo>(&json) {
            Ok(info) => Some(info),
            Err(e) => {
                tracing::warn!("engine_build_info returned unparsable JSON ({e}): {json}");
                None
            }
        }
    }

    /// `config.toml` の `gpu_backend` に従って DLL をロードする。
    ///
    /// - 明示指定（`cuda` / `vulkan` / `cpu`）: その DLL だけを試し、失敗はそのままエラーに
    ///   する（fallback しない）。
    /// - `auto` / 未指定: `cuda` → `vulkan` → `cpu` の順に実際にロードし、失敗したら理由を
    ///   記録して次へ進む。全て失敗した場合は各 backend の理由をまとめて返す。
    ///
    /// `install_dir`: rakukan DLL が配置されているディレクトリ
    /// `config_json`: EngineConfig JSON（null の場合はデフォルト）
    pub fn load_auto(install_dir: &Path, config_json: Option<&str>) -> Result<Self> {
        load_with_selection(detect_backend(), install_dir, |_backend, dll_path| {
            Self::load_dll_checked(dll_path, config_json)
        })
    }

    /// 指定バックエンド名の DLL をロードする。他の backend へは fallback しない。
    pub fn load_backend(
        install_dir: &Path,
        backend: &str,
        config_json: Option<&str>,
    ) -> Result<Self> {
        Self::load_dll_checked(&backend_dll_path(install_dir, backend), config_json)
    }

    /// ファイルの有無を確認してから DLL をロードする（1 backend ぶんの試行）。
    fn load_dll_checked(dll_path: &Path, config_json: Option<&str>) -> Result<Self> {
        if !dll_path.exists() {
            bail!("engine DLL not found: {}", dll_path.display());
        }
        Self::from_dll(dll_path, config_json)
    }

    // ── ヘルパー ───────────────────────────────────────────────────────────

    /// DLL が返した C 文字列を Rust String に変換して解放する
    unsafe fn take_cstr(&self, ptr: *mut c_char) -> Option<String> {
        if ptr.is_null() {
            return None;
        }
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
        unsafe {
            (self.vtable.push_char)(self.handle, c as u32);
        }
    }

    pub fn push_raw(&mut self, c: char) {
        unsafe {
            (self.vtable.push_raw)(self.handle, c as u32);
        }
    }

    pub fn push_fullwidth_alpha(&mut self, c: char) {
        unsafe {
            (self.vtable.push_fullwidth_alpha)(self.handle, c as u32);
        }
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

    pub fn romaji_log_str(&self) -> String {
        unsafe {
            let ptr = (self.vtable.romaji_log_str)(self.handle);
            self.take_cstr(ptr).unwrap_or_default()
        }
    }

    pub fn hiragana_from_romaji_log(&self) -> String {
        unsafe {
            let ptr = (self.vtable.hiragana_from_romaji_log)(self.handle);
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

    /// M2 §5.2: ライブ変換 preview 用、トップ候補だけを覗き見る (cache を進めない)。
    pub fn bg_peek_top_candidate(&self, key: &str) -> Option<String> {
        let ckey = Self::to_cstring(key);
        unsafe {
            let ptr = (self.vtable.bg_peek_top_candidate)(self.handle, ckey.as_ptr());
            self.take_cstr(ptr)
        }
    }

    /// Done 状態の converter を engine に戻す
    pub fn bg_reclaim(&mut self) {
        unsafe {
            (self.vtable.bg_reclaim)(self.handle);
        }
    }

    /// BG 変換完了を最大 `timeout_ms` ミリ秒ブロック待機する。
    /// Done になれば `true`、タイムアウトなら `false`。
    pub fn bg_wait_ms(&mut self, timeout_ms: u64) -> bool {
        unsafe { (self.vtable.bg_wait_ms)(self.handle, timeout_ms) != 0 }
    }

    // ── 確定・リセット ──────────────────────────────────────────────────────

    pub fn commit(&mut self, text: &str) {
        let cs = Self::to_cstring(text);
        unsafe {
            (self.vtable.commit)(self.handle, cs.as_ptr());
        }
    }

    pub fn commit_as_hiragana(&mut self) {
        unsafe {
            (self.vtable.commit_as_hiragana)(self.handle);
        }
    }

    pub fn reset_preedit(&mut self) {
        unsafe {
            (self.vtable.reset_preedit)(self.handle);
        }
    }

    pub fn force_preedit(&mut self, text: String) {
        let c = std::ffi::CString::new(text.replace('\0', "")).unwrap_or_default();
        unsafe {
            (self.vtable.force_preedit)(self.handle, c.as_ptr());
        }
    }

    pub fn reset_all(&mut self) {
        unsafe {
            (self.vtable.reset_all)(self.handle);
        }
    }

    // ── 変換（同期フォールバック）──────────────────────────────────────────

    pub fn convert_sync(&mut self) -> Vec<String> {
        unsafe {
            let ptr = (self.vtable.convert_sync)(self.handle);
            match self.take_cstr(ptr) {
                Some(json) => serde_json::from_str(&json).unwrap_or_default(),
                None => vec![],
            }
        }
    }

    pub fn merge_candidates_for_reading(
        &self,
        reading: &str,
        llm_cands: Vec<String>,
        limit: usize,
    ) -> Vec<String> {
        let creading = Self::to_cstring(reading);
        let json = serde_json::to_string(&llm_cands).unwrap_or_else(|_| "[]".into());
        let cjson = Self::to_cstring(&json);
        unsafe {
            let ptr = (self.vtable.merge_candidates_for_reading)(
                self.handle,
                creading.as_ptr(),
                cjson.as_ptr(),
                limit as u32,
            );
            match self.take_cstr(ptr) {
                Some(s) => serde_json::from_str(&s).unwrap_or_default(),
                None => vec![],
            }
        }
    }

    // ── 非同期初期化 ────────────────────────────────────────────────────────

    pub fn start_load_model(&mut self) {
        unsafe {
            (self.vtable.start_load_model)(self.handle);
        }
    }

    /// true = モデルが新たに利用可能になった（langbar 更新トリガー）
    pub fn poll_model_ready(&mut self) -> bool {
        unsafe { (self.vtable.poll_model_ready)(self.handle) }
    }

    pub fn start_load_dict(&mut self) {
        unsafe {
            (self.vtable.start_load_dict)(self.handle);
        }
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
        unsafe {
            (self.vtable.learn)(self.handle, r.as_ptr(), s.as_ptr());
        }
    }

    pub fn learn_force(&mut self, reading: &str, surface: &str) {
        let r = Self::to_cstring(reading);
        let s = Self::to_cstring(surface);
        unsafe {
            (self.vtable.learn_force)(self.handle, r.as_ptr(), s.as_ptr());
        }
    }

    /// エンジン DLL 側の最後のエラー/ステータスメッセージを返す（診断用）
    pub fn last_error(&self) -> String {
        let ptr = unsafe { (self.vtable.last_error)() };
        if ptr.is_null() {
            return String::new();
        }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.vtable.free_string)(ptr) };
        s
    }

    pub fn dict_status(&self) -> String {
        let ptr = unsafe { (self.vtable.dict_status)() };
        if ptr.is_null() {
            return String::new();
        }
        let s = unsafe { std::ffi::CStr::from_ptr(ptr) }
            .to_string_lossy()
            .into_owned();
        unsafe { (self.vtable.free_string)(ptr) };
        s
    }
}

impl Drop for DynEngine {
    fn drop(&mut self) {
        unsafe {
            (self.vtable.destroy)(self.handle);
        }
    }
}

// ─── build 識別子（Issue #8）───────────────────────────────────────────────────

/// engine DLL 側の build 識別子（DLL の `engine_build_info` JSON）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EngineBuildInfo {
    pub pkg_version: String,
    pub git_sha: String,
    #[serde(default)]
    pub build_time: String,
    pub abi_version: u32,
    #[serde(default)]
    pub log_status: String,
}

/// host 側（本 crate をリンクしたバイナリ）の build 識別子。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostBuildId {
    pub pkg_version: String,
    pub git_sha: String,
}

/// host 側の build 識別子を返す。git sha は `build.rs`（`build-support/git_info.rs`）が埋め込む。
pub fn host_build_id() -> HostBuildId {
    HostBuildId {
        pkg_version: env!("CARGO_PKG_VERSION").to_string(),
        git_sha: option_env!("RAKUKAN_GIT_SHA")
            .unwrap_or("unknown")
            .to_string(),
    }
}

/// host と DLL が別ビルドなら WARN 用のメッセージを返す。
///
/// - version が異なる → 別ビルド。
/// - version が同じでも git sha が両方判明していて異なる → 別ビルド
///   （ABI 番号が同じでも互換性は保証しない）。
/// - どちらかの sha が `unknown` → 判定不能なので警告しない。
pub fn build_mismatch(host: &HostBuildId, dll: &EngineBuildInfo) -> Option<String> {
    let describe = |v: &str, sha: &str| format!("{v}@{sha}");
    if host.pkg_version != dll.pkg_version {
        return Some(format!(
            "host and engine DLL come from different versions: host={} dll={}; rebuild and install both from the same source (`cargo make build-engine` + `cargo make build-tsf` + `cargo make install`)",
            describe(&host.pkg_version, &host.git_sha),
            describe(&dll.pkg_version, &dll.git_sha)
        ));
    }
    let known = |sha: &str| !sha.is_empty() && sha != "unknown";
    if known(&host.git_sha) && known(&dll.git_sha) && host.git_sha != dll.git_sha {
        return Some(format!(
            "host and engine DLL come from different builds (same version, ABI {}): host={} dll={}; ABI equality does not guarantee compatibility — rebuild and install both from the same source",
            dll.abi_version,
            describe(&host.pkg_version, &host.git_sha),
            describe(&dll.pkg_version, &dll.git_sha)
        ));
    }
    None
}

#[cfg(test)]
mod build_id_tests {
    use super::*;

    fn host(v: &str, sha: &str) -> HostBuildId {
        HostBuildId {
            pkg_version: v.into(),
            git_sha: sha.into(),
        }
    }
    fn dll(v: &str, sha: &str) -> EngineBuildInfo {
        EngineBuildInfo {
            pkg_version: v.into(),
            git_sha: sha.into(),
            build_time: String::new(),
            abi_version: EXPECTED_ENGINE_ABI_VERSION,
            log_status: "ok".into(),
        }
    }

    #[test]
    fn same_build_is_not_a_mismatch() {
        assert_eq!(
            build_mismatch(&host("0.10.4", "abc123"), &dll("0.10.4", "abc123")),
            None
        );
    }

    #[test]
    fn different_version_is_a_mismatch() {
        let msg = build_mismatch(&host("0.10.4", "abc123"), &dll("0.10.5", "def456")).unwrap();
        assert!(msg.contains("different versions"), "{msg}");
        assert!(
            msg.contains("0.10.4@abc123") && msg.contains("0.10.5@def456"),
            "{msg}"
        );
    }

    #[test]
    fn same_version_different_sha_is_a_mismatch() {
        // Issue #8: 0.10.4 の host に、同 version・同 ABI だが別コミットの DLL
        let msg = build_mismatch(&host("0.10.4", "abc123"), &dll("0.10.4", "def456")).unwrap();
        assert!(msg.contains("different builds"), "{msg}");
        assert!(msg.contains("same version"), "{msg}");
    }

    #[test]
    fn dirty_tree_counts_as_different_build() {
        assert!(
            build_mismatch(&host("0.10.4", "abc123"), &dll("0.10.4", "abc123-dirty")).is_some()
        );
    }

    #[test]
    fn unknown_sha_does_not_warn() {
        assert_eq!(
            build_mismatch(&host("0.10.4", "unknown"), &dll("0.10.4", "abc123")),
            None
        );
        assert_eq!(
            build_mismatch(&host("0.10.4", "abc123"), &dll("0.10.4", "unknown")),
            None
        );
    }

    #[test]
    fn build_info_json_from_dll_parses_without_optional_fields() {
        let info: EngineBuildInfo =
            serde_json::from_str(r#"{"pkg_version":"0.10.4","git_sha":"abc","abi_version":9}"#)
                .unwrap();
        assert_eq!(info.abi_version, 9);
        assert!(info.log_status.is_empty());
    }
}

// ─── バックエンド自動検出 ──────────────────────────────────────────────────────

/// config.toml の gpu_backend 指定をどう解釈するか。
enum BackendSelection {
    /// `cuda` / `vulkan` / `cpu` のいずれかが明示されている
    Explicit(String),
    /// 未指定、または `auto` が指定されている（DLL 存在で実行時判定）
    Auto,
}

/// config.toml の `gpu_backend` キーを読み取り、明示指定 / auto を区別して返す。
fn detect_backend() -> BackendSelection {
    match read_config_toml_backend() {
        Some(b) if matches!(b.as_str(), "cuda" | "vulkan" | "cpu") => {
            tracing::debug!("backend::select: from config.toml={b}");
            BackendSelection::Explicit(b)
        }
        Some(b) => {
            // "auto" もしくは想定外文字列 → 自動検出にフォールバック
            tracing::debug!("backend::select: config.toml={b} -> auto");
            BackendSelection::Auto
        }
        None => {
            tracing::debug!("backend::select: gpu_backend not set -> auto");
            BackendSelection::Auto
        }
    }
}

/// `auto` で試す backend の順序。
pub const AUTO_BACKEND_ORDER: [&str; 3] = ["cuda", "vulkan", "cpu"];

/// backend 名から DLL のフルパスを組み立てる。
pub fn backend_dll_path(install_dir: &Path, backend: &str) -> PathBuf {
    install_dir.join(format!("rakukan_engine_{backend}.dll"))
}

/// 選択方針に従って backend を順に試し、最初に `try_load` が成功したものを採用する。
///
/// ファイルの存在確認と実ロードを別々の選択ロジックにせず、`try_load` 1 回を
/// 「その backend の試行」とみなす。テストで実 DLL を使わずに検証できるよう、
/// ロード処理は引数で注入する。
///
/// - `Explicit`: 1 回だけ試す。失敗はそのまま返す。
/// - `Auto`: [`AUTO_BACKEND_ORDER`] の順に試す。失敗理由を WARN で残して次へ進み、
///   全て失敗したら理由をまとめたエラーを返す。
fn load_with_selection<T>(
    selection: BackendSelection,
    install_dir: &Path,
    mut try_load: impl FnMut(&str, &Path) -> Result<T>,
) -> Result<T> {
    match selection {
        BackendSelection::Explicit(backend) => {
            let dll_path = backend_dll_path(install_dir, &backend);
            tracing::info!(
                "Selected backend (explicit): {backend} path={}",
                dll_path.display()
            );
            try_load(&backend, &dll_path)
                .with_context(|| format!("backend {backend} (explicit, no fallback) failed"))
        }
        BackendSelection::Auto => {
            let mut failures: Vec<String> = Vec::new();
            for backend in AUTO_BACKEND_ORDER {
                let dll_path = backend_dll_path(install_dir, backend);
                tracing::info!(
                    "backend::auto: trying {backend} path={}",
                    dll_path.display()
                );
                match try_load(backend, &dll_path) {
                    Ok(engine) => {
                        if !failures.is_empty() {
                            tracing::warn!(
                                "backend::auto: fell back to {backend} after: {}",
                                failures.join("; ")
                            );
                        }
                        tracing::info!(
                            "Selected backend (auto): {backend} path={}",
                            dll_path.display()
                        );
                        return Ok(engine);
                    }
                    Err(e) => {
                        tracing::warn!("backend::auto: {backend} failed: {e:#}; trying next");
                        failures.push(format!("{backend}: {e:#}"));
                    }
                }
            }
            bail!("all backends failed (auto): {}", failures.join("; "))
        }
    }
}

/// `LoadLibrary` 失敗の原因を補足するヒント。
///
/// `ERROR_MOD_NOT_FOUND`（126）は「DLL 自体はあるが依存 DLL が見つからない」場合にも
/// 返るため（Issue #2: CUDA ランタイム未導入）、その旨を添える。取得できなければ空。
fn load_failure_hint(e: &libloading::Error) -> &'static str {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cur {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            return match io.raw_os_error() {
                Some(126) => {
                    " [依存 DLL が見つかりません。CUDA 版なら CUDA ランタイムの有無を確認してください]"
                }
                _ => "",
            };
        }
        cur = err.source();
    }
    ""
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
        if line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("gpu_backend") {
            let rest = rest.trim().trim_start_matches('=').trim();
            let val = rest
                .split('#')
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('"')
                .trim_matches('\'');
            if matches!(val, "cuda" | "vulkan" | "cpu" | "auto") {
                return Some(val.to_string());
            }
        }
    }
    None
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

#[cfg(test)]
mod backend_selection_tests {
    use super::*;
    use std::cell::RefCell;

    /// 指定した backend だけ成功するローダを作り、試行順を記録する。
    fn loader<'a>(
        ok: &'a [&'static str],
        attempts: &'a RefCell<Vec<String>>,
    ) -> impl FnMut(&str, &Path) -> Result<String> + 'a {
        move |backend, dll_path| {
            attempts.borrow_mut().push(backend.to_string());
            assert!(
                dll_path.ends_with(format!("rakukan_engine_{backend}.dll")),
                "path={}",
                dll_path.display()
            );
            if ok.contains(&backend) {
                Ok(backend.to_string())
            } else {
                Err(anyhow::anyhow!("simulated load failure for {backend}"))
            }
        }
    }

    fn dir() -> PathBuf {
        PathBuf::from("C:/install")
    }

    #[test]
    fn auto_falls_back_from_cuda_to_vulkan() {
        let attempts = RefCell::new(Vec::new());
        let got = load_with_selection(
            BackendSelection::Auto,
            &dir(),
            loader(&["vulkan", "cpu"], &attempts),
        )
        .unwrap();
        assert_eq!(got, "vulkan");
        assert_eq!(*attempts.borrow(), vec!["cuda", "vulkan"]);
    }

    #[test]
    fn auto_falls_back_to_cpu_when_gpu_backends_fail() {
        let attempts = RefCell::new(Vec::new());
        let got = load_with_selection(BackendSelection::Auto, &dir(), loader(&["cpu"], &attempts))
            .unwrap();
        assert_eq!(got, "cpu");
        assert_eq!(*attempts.borrow(), vec!["cuda", "vulkan", "cpu"]);
    }

    #[test]
    fn auto_reports_every_failure_when_all_fail() {
        let attempts = RefCell::new(Vec::new());
        let err = load_with_selection(BackendSelection::Auto, &dir(), loader(&[], &attempts))
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("all backends failed"), "{msg}");
        for backend in AUTO_BACKEND_ORDER {
            assert!(
                msg.contains(&format!("{backend}: simulated load failure for {backend}")),
                "{msg}"
            );
        }
        assert_eq!(*attempts.borrow(), vec!["cuda", "vulkan", "cpu"]);
    }

    #[test]
    fn auto_stops_at_first_success() {
        let attempts = RefCell::new(Vec::new());
        let got = load_with_selection(
            BackendSelection::Auto,
            &dir(),
            loader(&["cuda", "vulkan", "cpu"], &attempts),
        )
        .unwrap();
        assert_eq!(got, "cuda");
        assert_eq!(*attempts.borrow(), vec!["cuda"]);
    }

    #[test]
    fn explicit_backend_does_not_fall_back() {
        let attempts = RefCell::new(Vec::new());
        let err = load_with_selection(
            BackendSelection::Explicit("cuda".into()),
            &dir(),
            loader(&["vulkan", "cpu"], &attempts),
        )
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("explicit, no fallback"), "{msg}");
        assert!(msg.contains("simulated load failure for cuda"), "{msg}");
        assert_eq!(
            *attempts.borrow(),
            vec!["cuda"],
            "vulkan / cpu を試してはいけない"
        );
    }

    #[test]
    fn explicit_backend_loads_only_that_backend() {
        let attempts = RefCell::new(Vec::new());
        let got = load_with_selection(
            BackendSelection::Explicit("vulkan".into()),
            &dir(),
            loader(&["cuda", "vulkan", "cpu"], &attempts),
        )
        .unwrap();
        assert_eq!(got, "vulkan");
        assert_eq!(*attempts.borrow(), vec!["vulkan"]);
    }

    #[test]
    fn load_backend_with_missing_dll_is_an_error_without_cpu_fallback() {
        // 存在しないディレクトリ: 以前は cpu へ暗黙に fallback していたが、明示指定は失敗を返す
        let missing = std::env::temp_dir().join("rakukan-abi-test-does-not-exist");
        let err = match DynEngine::load_backend(&missing, "cuda", None) {
            Ok(_) => panic!("存在しない DLL のロードが成功してはいけない"),
            Err(e) => e,
        };
        let msg = format!("{err:#}");
        assert!(msg.contains("engine DLL not found"), "{msg}");
        assert!(msg.contains("rakukan_engine_cuda.dll"), "{msg}");
    }

    #[test]
    fn backend_dll_path_uses_backend_name() {
        let p = backend_dll_path(&dir(), "vulkan");
        assert!(p.ends_with("rakukan_engine_vulkan.dll"), "{}", p.display());
    }
}
