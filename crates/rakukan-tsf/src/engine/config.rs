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
    pub conversion: ConversionConfig,
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
            conversion: ConversionConfig::default(),
            diagnostics: DiagnosticsConfig::default(),
            num_candidates: None,
        }
    }
}

impl AppConfig {
    /// Space 変換時に LLM から取得する候補数。
    pub fn effective_num_candidates(&self) -> usize {
        self.conversion
            .num_candidates
            .or(self.num_candidates)
            .unwrap_or(9)
            .clamp(1, 30)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitWidth {
    Fullwidth,
    Halfwidth,
}

impl Default for DigitWidth {
    fn default() -> Self {
        DigitWidth::Halfwidth
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputConfig {
    #[serde(default = "default_input_mode")]
    pub default_mode: DefaultInputMode,
    #[serde(default = "default_remember_last_kana_mode")]
    pub remember_last_kana_mode: bool,
    #[serde(default)]
    pub digit_width: DigitWidth,
    /// 確定時にユーザー辞書へ `(reading → 選択表記)` を自動登録するか。
    /// デフォルト `false` (自動学習を抑止)。`true` にすると Space 確定のたびに
    /// user_dict.toml が肥大化するため、通常は手動登録のみで運用する。
    #[serde(default)]
    pub auto_learn: bool,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            default_mode: default_input_mode(),
            remember_last_kana_mode: true,
            digit_width: DigitWidth::default(),
            auto_learn: false,
        }
    }
}

fn default_debounce_ms() -> u64 {
    80
}
fn default_prefer_dictionary_first() -> bool {
    true
}

fn default_live_conv_beam_size() -> usize {
    3
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
    /// ライブ変換の候補数（beam 幅）。1 = greedy（高速）、3 = beam（高品質、デフォルト）
    #[serde(default = "default_live_conv_beam_size")]
    pub beam_size: usize,
}

impl Default for LiveConversionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            debounce_ms: 80,
            use_llm: false,
            prefer_dictionary_first: true,
            beam_size: 3,
        }
    }
}

fn default_convert_beam_size() -> usize {
    30
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionConfig {
    /// Space 変換時のビーム幅の**上限**。num_candidates と併せて min をとる。
    /// デフォルト 30 では実質上限なしで、num_candidates がそのまま候補数になる。
    /// 変換を速くしたい場合は小さく (例: 3) 設定することで beam 幅が抑えられる。
    /// 範囲: 1〜30。
    #[serde(default = "default_convert_beam_size")]
    pub beam_size: usize,
    /// Space 変換で候補ウィンドウに表示する候補数。
    /// 新形式では `[conversion].num_candidates` に保存する。
    #[serde(default)]
    pub num_candidates: Option<usize>,
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            beam_size: default_convert_beam_size(),
            num_candidates: None,
        }
    }
}

fn default_dump_active_config() -> bool {
    false
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
            dump_active_config: false,
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
log_level = "info"

# GPU バックエンド: "auto" / "cuda" / "vulkan" / "cpu"
# "auto"   : インストール済みの DLL から cuda → vulkan → cpu の順で自動選択（デフォルト）
# "cuda"   : NVIDIA GPU (CUDA) ← RTX シリーズ推奨
# "vulkan" : Vulkan 対応 GPU (AMD / Intel / NVIDIA)
# "cpu"    : CPU のみ（GPU なし、VMware 等）
gpu_backend = "auto"

# GPU に載せるレイヤー数
# 0 で CPU のみ、未指定で全レイヤーを GPU にオフロード
# GPU 競合や他アプリの異常終了がある場合は 8 / 16 / 24 など小さめを試す
n_gpu_layers = 16

# 使用する GPU インデックス（複数 GPU 環境で 2 枚目以降を使う場合に変更）
main_gpu = 0

# LLM モデル ID
# jinen-v1-xsmall-q5  : 軽量・推奨（約 30 MB、低スペック PC 向け、デフォルト）
# jinen-v1-small-q5   : 標準（約 84 MB、通常用途）
# jinen-v1-xsmall-f16 : 高精度・大容量（約 138 MB、量子化なし FP16）
# jinen-v1-small-f16  : 高精度・大容量（約 423 MB、量子化なし FP16）
model_variant = "jinen-v1-xsmall-q5"

[keyboard]
layout = "jis"
reload_on_mode_switch = true

[input]
default_mode = "alphanumeric"
remember_last_kana_mode = true
# 数字の入力幅: "halfwidth" = 半角 (012), "fullwidth" = 全角 (０１２)
digit_width = "halfwidth"
# 確定時にユーザー辞書へ自動登録するか (デフォルト: false)。
# true にすると Space 確定のたびに user_dict.toml が追記され肥大化する。
auto_learn = false

[live_conversion]
enabled = false
debounce_ms = 80
use_llm = false
prefer_dictionary_first = true
# ライブ変換の候補数（beam 幅）: 1 = greedy（高速）, 3 = beam search（高品質、デフォルト）
beam_size = 3

[conversion]
# Space 変換のビーム幅上限（num_candidates と min をとる）。
# デフォルト 30 では num_candidates がそのまま候補数になる。
# 変換を高速化したい場合は小さく設定する（例: beam_size = 3 で最大 3 候補、高速）。
# 範囲: 1〜30。
beam_size = 30

# Space 変換で表示する候補数（1〜30、デフォルト 9）。
# 新形式は [conversion].num_candidates。旧形式のルート直下 num_candidates も引き続き読める。
# num_candidates = 9

[diagnostics]
dump_active_config = false
warn_on_unknown_key = true

# 旧形式との互換用:
# num_candidates = 9
"#
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    #[test]
    fn effective_num_candidates_reads_new_conversion_key() {
        let cfg: AppConfig = toml::from_str(
            r#"
[conversion]
num_candidates = 12
"#,
        )
        .expect("config should parse");

        assert_eq!(cfg.effective_num_candidates(), 12);
    }

    #[test]
    fn effective_num_candidates_falls_back_to_legacy_root_key() {
        let cfg: AppConfig = toml::from_str("num_candidates = 7").expect("config should parse");

        assert_eq!(cfg.effective_num_candidates(), 7);
    }
}
