use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub keyboard: KeyboardConfig,
    #[serde(default)]
    pub input: InputConfig,
    #[serde(default)]
    pub candidate: CandidateConfig,
    #[serde(default)]
    pub conversion: ConversionConfig,
    #[serde(default)]
    pub live_conversion: LiveConversionConfig,
    #[serde(default)]
    pub diagnostics: DiagnosticsConfig,

    #[serde(default)]
    pub gpu_backend: Option<String>,
    #[serde(default)]
    pub main_gpu: i32,
    #[serde(default)]
    pub model_variant: Option<String>,

    /// 旧形式との互換用。存在する場合は candidate.page_size より優先。
    #[serde(default)]
    pub num_candidates: Option<usize>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            keyboard: KeyboardConfig::default(),
            input: InputConfig::default(),
            candidate: CandidateConfig::default(),
            conversion: ConversionConfig::default(),
            live_conversion: LiveConversionConfig::default(),
            diagnostics: DiagnosticsConfig::default(),
            gpu_backend: None,
            main_gpu: 0,
            model_variant: None,
            num_candidates: None,
        }
    }
}

impl AppConfig {
    pub fn effective_num_candidates(&self) -> usize {
        self.num_candidates
            .unwrap_or(self.candidate.page_size)
            .clamp(1, 9)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self { log_level: default_log_level() }
    }
}

fn default_log_level() -> String { "info".to_string() }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardLayout {
    Us,
    Jis,
    Custom,
}

fn default_keyboard_layout() -> KeyboardLayout { KeyboardLayout::Us }
fn default_reload_on_mode_switch() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardConfig {
    #[serde(default = "default_keyboard_layout")]
    pub layout: KeyboardLayout,
    #[serde(default)]
    pub enable_jis_keys: bool,
    #[serde(default = "default_reload_on_mode_switch")]
    pub reload_on_mode_switch: bool,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            layout: default_keyboard_layout(),
            enable_jis_keys: false,
            reload_on_mode_switch: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultInputMode {
    Hiragana,
    Katakana,
    Alphanumeric,
}

fn default_input_mode() -> DefaultInputMode { DefaultInputMode::Hiragana }
fn default_remember_last_kana_mode() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputConfig {
    #[serde(default = "default_input_mode")]
    pub default_mode: DefaultInputMode,
    #[serde(default = "default_remember_last_kana_mode")]
    pub remember_last_kana_mode: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self { default_mode: default_input_mode(), remember_last_kana_mode: true }
    }
}

fn default_page_size() -> usize { 9 }
fn default_use_number_selection() -> bool { true }
fn default_show_numbers() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateConfig {
    #[serde(default = "default_page_size")]
    pub page_size: usize,
    #[serde(default = "default_use_number_selection")]
    pub use_number_selection: bool,
    #[serde(default = "default_show_numbers")]
    pub show_numbers: bool,
}

impl Default for CandidateConfig {
    fn default() -> Self {
        Self { page_size: 9, use_number_selection: true, show_numbers: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelBehavior {
    MsIme,
    Simple,
}

fn default_engine() -> String { "karukan".to_string() }
fn default_commit_raw_with_enter() -> bool { true }
fn default_cancel_behavior() -> CancelBehavior { CancelBehavior::MsIme }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionConfig {
    #[serde(default = "default_engine")]
    pub engine: String,
    #[serde(default = "default_commit_raw_with_enter")]
    pub commit_raw_with_enter: bool,
    #[serde(default = "default_cancel_behavior")]
    pub cancel_behavior: CancelBehavior,
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            engine: default_engine(),
            commit_raw_with_enter: true,
            cancel_behavior: default_cancel_behavior(),
        }
    }
}

fn default_debounce_ms() -> u64 { 80 }
fn default_prefer_dictionary_first() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveConversionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
    #[serde(default)]
    pub use_llm: bool,
    #[serde(default = "default_prefer_dictionary_first")]
    pub prefer_dictionary_first: bool,
}

impl Default for LiveConversionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            debounce_ms: 80,
            use_llm: false,
            prefer_dictionary_first: true,
        }
    }
}

fn default_dump_active_config() -> bool { true }
fn default_warn_on_unknown_key() -> bool { true }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsConfig {
    #[serde(default = "default_dump_active_config")]
    pub dump_active_config: bool,
    #[serde(default = "default_warn_on_unknown_key")]
    pub warn_on_unknown_key: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self { dump_active_config: true, warn_on_unknown_key: true }
    }
}

#[derive(Debug)]
struct ConfigManager {
    path: PathBuf,
    last_modified: Option<SystemTime>,
    current: AppConfig,
}

impl ConfigManager {
    fn new() -> Self {
        let path = config_path().unwrap_or_else(|_| PathBuf::from("config.toml"));
        let current = load_app_config_from_path(&path).unwrap_or_default();
        let last_modified = file_modified(&path);
        Self { path, last_modified, current }
    }

    fn reload_if_changed(&mut self) -> Result<bool> {
        let modified = file_modified(&self.path);
        if modified == self.last_modified {
            return Ok(false);
        }
        let cfg = load_app_config_from_path(&self.path)?;
        self.current = cfg;
        self.last_modified = modified;
        Ok(true)
    }
}

static CONFIG_MANAGER: LazyLock<Mutex<ConfigManager>> =
    LazyLock::new(|| Mutex::new(ConfigManager::new()));

pub fn config_path() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA")
        .map_err(|_| anyhow::anyhow!("APPDATA not set"))?;
    Ok(PathBuf::from(appdata).join("rakukan").join("config.toml"))
}

fn file_modified(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

pub fn load_app_config_from_path(path: &PathBuf) -> Result<AppConfig> {
    let text = std::fs::read_to_string(path)?;
    let cfg: AppConfig = toml::from_str(&text)?;
    Ok(cfg)
}

pub fn config_save_default() -> Result<()> {
    let path = config_path()?;
    if !path.exists() {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(&path, default_config_text())?;
        tracing::info!("config.toml created: {}", path.display());
    }
    Ok(())
}

pub fn init_config_manager() {
    if let Ok(mut mgr) = CONFIG_MANAGER.lock() {
        mgr.path = config_path().unwrap_or_else(|_| mgr.path.clone());
        mgr.current = load_app_config_from_path(&mgr.path).unwrap_or_default();
        mgr.last_modified = file_modified(&mgr.path);
    }
}

pub fn current_config() -> AppConfig {
    CONFIG_MANAGER.lock()
        .map(|g| g.current.clone())
        .unwrap_or_default()
}

pub fn effective_num_candidates() -> usize {
    current_config().effective_num_candidates()
}

pub fn keyboard_layout() -> KeyboardLayout {
    current_config().keyboard.layout
}

pub fn maybe_reload_on_mode_switch() -> bool {
    let mut mgr = match CONFIG_MANAGER.lock() {
        Ok(g) => g,
        Err(p) => {
            tracing::warn!("config manager poisoned, recovering");
            p.into_inner()
        }
    };

    if !mgr.current.keyboard.reload_on_mode_switch {
        return false;
    }

    match mgr.reload_if_changed() {
        Ok(changed) => {
            if changed {
                tracing::info!(
                    "config.toml reloaded on mode switch: layout={:?} num_candidates={} live_conversion={}",
                    mgr.current.keyboard.layout,
                    mgr.current.effective_num_candidates(),
                    mgr.current.live_conversion.enabled,
                );
            }
            changed
        }
        Err(e) => {
            tracing::warn!("config.toml reload failed; keeping previous config: {e}");
            false
        }
    }
}

fn default_config_text() -> &'static str {
    r#"# rakukan 設定ファイル
# 入力モード変更時に再読込されます。
# gpu_backend など再ビルドが必要な項目は cargo make install を再実行してください。

[general]
log_level = "info"

[keyboard]
layout = "us"
enable_jis_keys = false
reload_on_mode_switch = true

[input]
default_mode = "hiragana"
remember_last_kana_mode = true

[candidate]
page_size = 9
use_number_selection = true
show_numbers = true

[conversion]
engine = "karukan"
commit_raw_with_enter = true
cancel_behavior = "ms_ime"

[live_conversion]
enabled = false
debounce_ms = 80
use_llm = false
prefer_dictionary_first = true

[diagnostics]
dump_active_config = true
warn_on_unknown_key = true

# 旧形式との互換用。
# num_candidates = 9

# GPU バックエンドやモデル切替は再インストール/再ビルド前提です。
# gpu_backend = "cuda"
# main_gpu = 0
# model_variant = "small"
"#
}
