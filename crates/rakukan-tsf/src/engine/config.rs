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
    pub live_conversion: LiveConversionConfig,
    #[serde(default)]
    pub diagnostics: DiagnosticsConfig,

    /// 旧形式との互換用（config.toml に num_candidates = N と書いた場合に有効）。
    #[serde(default)]
    pub num_candidates: Option<usize>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            keyboard: KeyboardConfig::default(),
            input: InputConfig::default(),
            live_conversion: LiveConversionConfig::default(),
            diagnostics: DiagnosticsConfig::default(),
            num_candidates: None,
        }
    }
}

impl AppConfig {
    pub fn effective_num_candidates(&self) -> usize {
        self.num_candidates.unwrap_or(9).clamp(1, 9)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default = "default_log_level")]
    pub log_level: String,
    #[serde(default)]
    pub gpu_backend: Option<String>,
    #[serde(default)]
    pub n_gpu_layers: Option<u32>,
    #[serde(default)]
    pub main_gpu: i32,
    #[serde(default)]
    pub model_variant: Option<String>,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            log_level: default_log_level(),
            gpu_backend: None,
            n_gpu_layers: None,
            main_gpu: 0,
            model_variant: None,
        }
    }
}

fn default_log_level() -> String {
    "info".to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyboardLayout {
    Us,
    Jis,
    Custom,
}

fn default_keyboard_layout() -> KeyboardLayout {
    KeyboardLayout::Jis
}
fn default_reload_on_mode_switch() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyboardConfig {
    #[serde(default = "default_keyboard_layout")]
    pub layout: KeyboardLayout,
    #[serde(default = "default_reload_on_mode_switch")]
    pub reload_on_mode_switch: bool,
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            layout: default_keyboard_layout(),
            reload_on_mode_switch: true,
        }
    }
}

/// 起動時・初回フォーカス時のデフォルト入力モード。
/// カタカナモードは廃止（F7 変換は引き続き動作する）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultInputMode {
    Hiragana,
    Alphanumeric,
}

fn default_input_mode() -> DefaultInputMode {
    DefaultInputMode::Alphanumeric
}
fn default_remember_last_kana_mode() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputConfig {
    #[serde(default = "default_input_mode")]
    pub default_mode: DefaultInputMode,
    #[serde(default = "default_remember_last_kana_mode")]
    pub remember_last_kana_mode: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            default_mode: default_input_mode(),
            remember_last_kana_mode: true,
        }
    }
}

fn default_debounce_ms() -> u64 {
    80
}
fn default_prefer_dictionary_first() -> bool {
    true
}

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

fn default_dump_active_config() -> bool {
    true
}
fn default_warn_on_unknown_key() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsConfig {
    #[serde(default = "default_dump_active_config")]
    pub dump_active_config: bool,
    #[serde(default = "default_warn_on_unknown_key")]
    pub warn_on_unknown_key: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            dump_active_config: true,
            warn_on_unknown_key: true,
        }
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
        Self {
            path,
            last_modified,
            current,
        }
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
    let appdata = std::env::var("APPDATA").map_err(|_| anyhow::anyhow!("APPDATA not set"))?;
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
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
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
    CONFIG_MANAGER
        .lock()
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

[general]
# ログレベル: error / warn / info / debug / trace
# debug: 開発中の標準。キー入力ごとの状態変化が見える
# info:  通常運用。初期化・確定・モード変更のみ
# trace: 詳細調査時。ループ内・トークン単位まで出力される（低速）
# 環境変数 RAKUKAN_LOG が設定されている場合はそちらが優先される
log_level = "debug"

# GPU バックエンド: "cuda" / "vulkan" / "cpu"
# 未指定の場合は backend.json の保存値または自動検出を使用する
# "cuda"   : NVIDIA GPU (CUDA) ← RTX シリーズ推奨
# "vulkan" : Vulkan 対応 GPU (AMD / Intel / NVIDIA)
# "cpu"    : CPU のみ（GPU なし、VMware 等）
# gpu_backend = "cuda"

# GPU に載せるレイヤー数
# 0 で CPU のみ、未指定で全レイヤーを GPU にオフロード
# GPU 競合や他アプリの異常終了がある場合は 8 / 16 / 24 など小さめを試す
# n_gpu_layers = 16

# 使用する GPU インデックス（複数 GPU 環境で 2 枚目以降を使う場合に変更）
# main_gpu = 0

# LLM モデル ID
# model_variant = "jinen-v1-small-q5"
# model_variant = "jinen-v1-xsmall-q5"

[keyboard]
layout = "jis"
reload_on_mode_switch = true

[input]
default_mode = "alphanumeric"
remember_last_kana_mode = true

[live_conversion]
enabled = false
debounce_ms = 80
use_llm = false
prefer_dictionary_first = true

[diagnostics]
dump_active_config = true
warn_on_unknown_key = true

# 旧形式との互換用
# num_candidates = 9
"#
}
