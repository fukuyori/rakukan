use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub diagnostics: DiagnosticsConfig,

    /// 旧形式との互換用（config.toml に num_candidates = N と書いた場合に有効）。
    #[serde(default)]
    pub num_candidates: Option<usize>,
}

/// 候補ウィンドウの見た目。
///
/// `candidate_font_height` は候補ウィンドウのフォント高さ（ピクセル）。
/// 行の高さ・余白・最小幅もこの値を基準に同じ比率で拡大されるため、
/// ここだけ変えればウィンドウ全体が破綻せずに拡大縮小する。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceConfig {
    #[serde(default = "default_candidate_font_height")]
    pub candidate_font_height: i32,
}

/// 既定のフォント高さ。candidate_window.rs のレイアウト定数はこの値を基準に決めてある。
pub const DEFAULT_CANDIDATE_FONT_HEIGHT: i32 = 17;

fn default_candidate_font_height() -> i32 {
    DEFAULT_CANDIDATE_FONT_HEIGHT
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            candidate_font_height: default_candidate_font_height(),
        }
    }
}

impl AppConfig {
    /// Space 変換時に LLM から取得する候補数。
    pub fn effective_num_candidates(&self) -> usize {
        self.conversion
            .num_candidates
            .or(self.num_candidates)
            .unwrap_or(6)
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

/// 起動時・初回フォーカス時の IME オン/オフ。
///
/// 旧設定値 `"hiragana"` / `"alphanumeric"` は互換のため受け付け、
/// それぞれ `on` / `off` として扱う。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefaultImeMode {
    #[serde(alias = "hiragana")]
    On,
    #[serde(alias = "alphanumeric")]
    Off,
}

fn default_ime_mode() -> DefaultImeMode {
    DefaultImeMode::Off
}
fn default_remember_last_kana_mode() -> bool {
    true
}
fn default_digit_separator_auto() -> bool {
    true
}
fn default_digit_candidates_order() -> Vec<DigitCandidateKind> {
    vec![
        DigitCandidateKind::Arabic,
        DigitCandidateKind::Fullwidth,
        DigitCandidateKind::Positional,
        DigitCandidateKind::PerDigit,
        DigitCandidateKind::Daiji,
    ]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum DigitWidth {
    Fullwidth,
    #[default]
    Halfwidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AlphaWidth {
    #[default]
    Fullwidth,
    Halfwidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SymbolWidth {
    #[default]
    Fullwidth,
    Halfwidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitCandidateKind {
    Arabic,
    Fullwidth,
    Positional,
    PerDigit,
    Daiji,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputConfig {
    #[serde(default = "default_ime_mode")]
    pub default_mode: DefaultImeMode,
    #[serde(default = "default_remember_last_kana_mode")]
    pub remember_last_kana_mode: bool,
    #[serde(default)]
    pub digit_width: DigitWidth,
    /// 英字の入力幅。デフォルトは全角。
    /// `halfwidth` にすると入力時の英字を半角のまま保持し、候補も半角先頭。
    #[serde(default)]
    pub alpha_width: AlphaWidth,
    /// 記号の入力幅。デフォルトは全角。
    /// `halfwidth` にすると入力時の記号を半角のまま保持し、候補も半角先頭。
    #[serde(default)]
    pub symbol_width: SymbolWidth,
    /// 数字直後の `、` / `。` を `,` / `.` として扱う。
    #[serde(default = "default_digit_separator_auto")]
    pub digit_separator_auto: bool,
    /// 数字候補の表示順。指定した種別だけを候補に出す。
    #[serde(default = "default_digit_candidates_order")]
    pub digit_candidates_order: Vec<DigitCandidateKind>,
    /// 確定時に学習するか (デフォルト `true`)。
    /// Phase 1: 従来どおり user_dict.toml に追記される (肥大化注意)。
    /// Phase 2 以降: 独立した learn_history に記録され user_dict.toml には書かない。
    #[serde(default = "default_auto_learn")]
    pub auto_learn: bool,
    /// アクティブになったとき IME をオフで始めるアプリ（exe 名。大文字小文字は区別しない）。
    ///
    /// ターミナルのように常に英数で打ち始めたいアプリを挙げる。既定は旧来の
    /// コンソール・Windows Terminal・mintty。空配列を書けば何も適用しない。
    /// 操作中に IME を変えればその状態が続き、インアクティブになると捨てられる
    /// （次にアクティブになったらまたオフで始まる）。
    #[serde(default = "default_ime_off_apps")]
    pub ime_off_apps: Vec<String>,
    /// アクティブになり、アプリ本体とは別の入力先に入ったとき IME をオンにする
    /// アプリ（exe 名。大文字小文字は区別しない）。
    ///
    /// Photoshop の文字ツールのように、入力のたびに別の入力先が作られるアプリで
    /// 使う。アクティブ化後 1 回だけ適用し、その後は操作した状態が続く。
    #[serde(default)]
    pub ime_on_apps: Vec<String>,
}

/// `ime_off_apps` の既定値。
///
/// 0.11.7 まではウィンドウクラス名（`CASCADIA_HOSTING_WINDOW_CLASS` 等）を
/// コードに直書きして判定していた（Issue #51）。config を唯一の情報源にする
/// ため exe 名へ移したが、キーが無い既存の設定でも従来どおりオフになるよう
/// 既定値として持つ。
fn default_ime_off_apps() -> Vec<String> {
    vec![
        "conhost.exe".to_string(),
        "WindowsTerminal.exe".to_string(),
        "mintty.exe".to_string(),
        // 実機で一致を確認した名前（2026-09-15）。旧来のクラス名判定では
        // 拾えなかったターミナル。
        "wezterm-gui.exe".to_string(),
        "ghostty.exe".to_string(),
    ]
}

fn default_auto_learn() -> bool {
    true
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            default_mode: default_ime_mode(),
            remember_last_kana_mode: true,
            digit_width: DigitWidth::default(),
            alpha_width: AlphaWidth::default(),
            symbol_width: SymbolWidth::default(),
            digit_separator_auto: default_digit_separator_auto(),
            digit_candidates_order: default_digit_candidates_order(),
            auto_learn: default_auto_learn(),
            ime_off_apps: default_ime_off_apps(),
            ime_on_apps: Vec::new(),
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
    1
}

fn default_live_conv_min_chars() -> usize {
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
    /// ライブ変換の候補数（beam 幅）。1 = greedy（高速、デフォルト）、3 = beam（高品質）
    #[serde(default = "default_live_conv_beam_size")]
    pub beam_size: usize,
    /// ライブ変換を開始する最小文字数（デフォルト 3）。
    /// 1 にすると 1 文字から変換を試みる（より積極的、負荷増）。
    /// 2 以上を推奨。0 は 1 と同じ扱い。
    #[serde(default = "default_live_conv_min_chars")]
    pub min_chars: usize,
}

impl Default for LiveConversionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            debounce_ms: 80,
            use_llm: false,
            prefer_dictionary_first: true,
            beam_size: 1,
            min_chars: default_live_conv_min_chars(),
        }
    }
}

fn default_convert_beam_size() -> usize {
    6
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionConfig {
    /// Space 変換時のビーム幅の**上限**。num_candidates と併せて min をとる。
    /// デフォルト 6 では候補数 6 と揃え、候補表の幅を保つ。
    /// 体感速度を優先する場合は小さく、候補幅を優先する場合は大きく設定する。
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
    /// 推論を必ず失敗させる（既定 false）。
    ///
    /// GPU デバイス消失（ドライバ更新・スリープ復帰・TDR）で推論が即時失敗する
    /// 壊れ方は意図的に再現できないため、復帰の段階（Issue #43）を実機で確認する
    /// ための診断用スイッチ。変換は辞書候補だけになる。
    #[serde(default)]
    pub force_inference_failure: bool,
}

impl Default for DiagnosticsConfig {
    fn default() -> Self {
        Self {
            dump_active_config: false,
            warn_on_unknown_key: true,
            force_inference_failure: false,
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
        Self::from_path(config_path().unwrap_or_else(|_| PathBuf::from("config.toml")))
    }

    /// 初回の読み込み（Issue #61）。
    ///
    /// 保持すべき前の設定が無いので、失敗したら既定値を使う。ただし**必ず警告を
    /// 残す**。無言で既定値へ戻すと、利用者からは「設定が勝手に初期化された」と
    /// しか見えない。
    fn from_path(path: PathBuf) -> Self {
        let current = match load_app_config_from_path(&path) {
            Ok(cfg) => cfg,
            Err(e) => {
                tracing::warn!(
                    "config.toml load failed; starting with defaults: path={} error={e}",
                    path.display()
                );
                AppConfig::default()
            }
        };
        let last_modified = file_modified(&path);
        publish_atomics(&current);
        Self {
            path,
            last_modified,
            current,
        }
    }

    /// 再初期化（Issue #61）。**失敗したら直前の有効な設定を保つ。**
    ///
    /// ファイルが無い場合も保持する。削除を暗黙の「設定リセット」にしない。
    /// ここでは `config_save_default()` を呼ばず、ファイルを作り直さない。
    ///
    /// 失敗時に `last_modified` を進めないので、利用者が TOML を直せば次の
    /// 読み込みで反映される。
    fn reinit(&mut self) {
        match load_app_config_from_path(&self.path) {
            Ok(cfg) => {
                self.current = cfg;
                self.last_modified = file_modified(&self.path);
                publish_atomics(&self.current);
            }
            Err(e) => {
                tracing::warn!(
                    "config.toml reload failed; keeping previous config: path={} error={e}",
                    self.path.display()
                );
            }
        }
    }

    fn reload_if_changed(&mut self) -> Result<bool> {
        let modified = file_modified(&self.path);
        if modified == self.last_modified {
            return Ok(false);
        }
        let cfg = load_app_config_from_path(&self.path)?;
        publish_atomics(&cfg);
        self.current = cfg;
        self.last_modified = modified;
        Ok(true)
    }
}

/// 描画パスから読む値をアトミックに複製しておく。
///
/// 候補ウィンドウの WM_PAINT は 1 行ごとに寸法を参照するため、そこで
/// CONFIG_MANAGER の Mutex を取ると描画のたびにロック競合が起きる。
/// 設定を読み込んだタイミングでアトミックへ写しておき、描画側はロックなしで読む。
fn publish_atomics(cfg: &AppConfig) {
    // エンジン DLL のログレベルは `RAKUKAN_LOG` だけで決まり、config.toml の
    // log_level が効かない。次に spawn するホストへ渡せるよう、config を読んだ
    // 時点で RPC クライアントに預ける（起動済みのホストには影響しない）。
    rakukan_engine_rpc::set_host_log_level(Some(cfg.general.log_level.clone()));
    CANDIDATE_FONT_HEIGHT.store(
        cfg.appearance
            .candidate_font_height
            .clamp(MIN_CANDIDATE_FONT_HEIGHT, MAX_CANDIDATE_FONT_HEIGHT),
        Ordering::Relaxed,
    );
}

const MIN_CANDIDATE_FONT_HEIGHT: i32 = 10;
const MAX_CANDIDATE_FONT_HEIGHT: i32 = 72;

static CANDIDATE_FONT_HEIGHT: AtomicI32 = AtomicI32::new(DEFAULT_CANDIDATE_FONT_HEIGHT);

/// 候補ウィンドウのフォント高さ（ピクセル）。描画パスから毎行呼ばれる想定でロックしない。
pub fn candidate_font_height() -> i32 {
    CANDIDATE_FONT_HEIGHT.load(Ordering::Relaxed)
}

/// `refresh_appearance_if_changed` 用の mtime キャッシュ。
/// 外側の Option は「未チェック」、内側の Option は「ファイルが無い」を表す。
/// CONFIG_MANAGER の last_modified とは独立に持つ（下記コメント参照）。
static APPEARANCE_MTIME: Mutex<Option<Option<SystemTime>>> = Mutex::new(None);

/// 候補ウィンドウの表示直前に呼ぶ軽量チェック。config.toml の mtime が変わって
/// いれば読み直し、描画用アトミック（フォントサイズ）だけを更新する。
///
/// 設定アプリの保存は名前付きイベント `Local\rakukan.engine.reload` で通知されるが、
/// これは auto-reset イベントで、SetEvent が起こすのは待機スレッド 1 本だけ。
/// TSF DLL はアプリごとに別プロセスで動くため、イベントを受け取れなかった
/// プロセスでは `publish_atomics` が走らず、フォントサイズの変更が反映されない
/// ことがあった。表示 1 回につき stat 1 回のコストで、表示するプロセス自身が
/// 最新値を拾う。
///
/// CONFIG_MANAGER の current や last_modified は**意図的に触らない**。ここで
/// 消費すると、手編集された config.toml をモード切替時の `reload_if_changed` が
/// 「変更なし」と誤判定し、エンジン再起動がスキップされてしまうため。
pub fn refresh_appearance_if_changed() {
    let Ok(path) = config_path() else {
        return;
    };
    let modified = file_modified(&path);
    let Ok(mut last) = APPEARANCE_MTIME.lock() else {
        return;
    };
    if *last == Some(modified) {
        return;
    }
    *last = Some(modified);
    match load_app_config_from_path(&path) {
        Ok(cfg) => publish_atomics(&cfg),
        Err(e) => tracing::warn!("refresh_appearance: config.toml load failed: {e}"),
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

/// config.toml を読み直す。**読めなければ直前の設定を保つ**（Issue #61）。
///
/// 呼び出し元は DllMain（初回）、`engine_reload` のスレッド、言語バーの
/// 「エンジン再起動」。このうち初回だけは、直前に `config_save_default()` が
/// ファイルを作る。再初期化の経路ではファイルを作り直さない。
pub fn init_config_manager() {
    if let Ok(mut mgr) = CONFIG_MANAGER.lock() {
        mgr.path = config_path().unwrap_or_else(|_| mgr.path.clone());
        mgr.reinit();
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
# jinen-v2-xsmall-q5  : v2 世代 (Qwen3) 軽量（約 28 MB）
# jinen-v2-small-q5   : v2 世代 (Qwen3) 標準（約 81 MB）
# jinen-v2-xsmall-f16 : v2 世代 (Qwen3) FP16（約 72 MB）
# jinen-v2-small-f16  : v2 世代 (Qwen3) FP16（約 220 MB）
model_variant = "jinen-v1-xsmall-q5"

[keyboard]
layout = "jis"
reload_on_mode_switch = true

[input]
# 起動時の IME 状態: "off" = 直接入力, "on" = かな漢字変換
default_mode = "off"
# 前回の IME オン/オフをアプリ（ウィンドウ）ごとに記憶する
remember_last_kana_mode = true
# 数字の入力幅: "halfwidth" = 半角 (012), "fullwidth" = 全角 (０１２)
digit_width = "halfwidth"
# 英字の入力幅: "fullwidth" = 全角 (ＡＢＣ), "halfwidth" = 半角 (ABC)
alpha_width = "fullwidth"
# 記号の入力幅: "fullwidth" = 全角 (＠＃＆), "halfwidth" = 半角 (@#&)
symbol_width = "fullwidth"
# 数字直後の 、/。 を ,/. として入力する
digit_separator_auto = true
# 数字だけの reading に対して提示する候補種別と順序
digit_candidates_order = ["arabic", "fullwidth", "positional", "per_digit", "daiji"]
# 確定時に学習するか (デフォルト: true)。
# false にすると学習を完全に抑止する。
auto_learn = true
# アクティブになったとき IME をオフで始めるアプリ (exe 名)。
# ターミナルのように常に英数で打ち始めたいアプリを挙げる。
# 操作中に IME を変えればその状態が続き、インアクティブになると捨てられる。
# 空配列を書けば何も適用しない。
ime_off_apps = ["conhost.exe", "WindowsTerminal.exe", "mintty.exe", "wezterm-gui.exe", "ghostty.exe"]
# アクティブになり、アプリ本体とは別の入力先に入ったとき IME をオンにするアプリ (exe 名)。
# Photoshop の文字ツールのように、入力のたびに別の入力先が作られるアプリで使う。
# アクティブ化後 1 回だけ適用し、その後は操作した状態が続く。
ime_on_apps = []

[live_conversion]
enabled = false
debounce_ms = 80
use_llm = false
prefer_dictionary_first = true
# ライブ変換の候補数（beam 幅）: 1 = greedy（高速、デフォルト）, 3 = beam search（高品質）
beam_size = 1
# ライブ変換を開始する最小文字数（デフォルト 3）。
# 2 にすると 2 文字から変換を開始する（より積極的、変換負荷増）。
# 1 は 2 以上を推奨するが設定可能。
min_chars = 3

[conversion]
# Space 変換のビーム幅上限（num_candidates と min をとる）。
# デフォルト 6 では候補数 6 と揃え、候補表の幅を保つ。
# 体感速度を優先する場合は小さく、候補幅を優先する場合は大きく設定する。
# 範囲: 1〜30。
beam_size = 6

# Space 変換で表示する候補数（1〜30、デフォルト 6）。
# 新形式は [conversion].num_candidates。旧形式のルート直下 num_candidates も引き続き読める。
# num_candidates = 6

[appearance]
# 候補ウィンドウのフォントサイズ（ピクセル）。既定 17
# 行の高さ・余白・最小幅も同じ比率で拡大するので、この値だけ変えればよい。
# 10〜72 にクランプされる。次回の候補表示から反映。
candidate_font_height = 17

[diagnostics]
dump_active_config = false
warn_on_unknown_key = true
# 診断用: 推論を必ず失敗させる (デフォルト: false)。
# GPU デバイス消失からの復帰 (Issue #43) の確認用。変換は辞書候補だけになる。
# force_inference_failure = true

# 旧形式との互換用:
# num_candidates = 6
"#
}

#[cfg(test)]
mod tests {
    use super::AppConfig;

    #[test]
    fn ime_off_apps_has_terminal_defaults() {
        // キーが無い設定でも、従来のターミナル判定と同じ結果になること
        let cfg = AppConfig::default();
        assert_eq!(
            cfg.input.ime_off_apps,
            vec![
                "conhost.exe",
                "WindowsTerminal.exe",
                "mintty.exe",
                "wezterm-gui.exe",
                "ghostty.exe"
            ]
        );
        assert!(cfg.input.ime_on_apps.is_empty());

        let parsed: AppConfig = toml::from_str(
            "[input]
default_mode = \"off\"
",
        )
        .expect("parse");
        assert_eq!(parsed.input.ime_off_apps.len(), 5);
    }

    #[test]
    fn ime_apps_can_be_set_and_emptied() {
        let cfg: AppConfig = toml::from_str(
            r#"
[input]
ime_off_apps = ["wezterm.exe"]
ime_on_apps = ["Photoshop.exe"]
"#,
        )
        .expect("parse");
        assert_eq!(cfg.input.ime_off_apps, vec!["wezterm.exe"]);
        assert_eq!(cfg.input.ime_on_apps, vec!["Photoshop.exe"]);

        // 空配列を書けば既定値を打ち消せる
        let none: AppConfig = toml::from_str(
            "[input]
ime_off_apps = []
",
        )
        .expect("parse");
        assert!(none.input.ime_off_apps.is_empty());
    }

    #[test]
    fn force_inference_failure_defaults_to_false() {
        assert!(!AppConfig::default().diagnostics.force_inference_failure);
        let cfg: AppConfig = toml::from_str(
            "[diagnostics]
warn_on_unknown_key = true
",
        )
        .expect("config should parse");
        assert!(!cfg.diagnostics.force_inference_failure);
    }

    #[test]
    fn force_inference_failure_can_be_enabled() {
        let cfg: AppConfig = toml::from_str(
            "[diagnostics]
force_inference_failure = true
",
        )
        .expect("parse");
        assert!(cfg.diagnostics.force_inference_failure);
    }

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

    #[test]
    fn effective_num_candidates_defaults_to_fast_profile() {
        let cfg = AppConfig::default();

        assert_eq!(cfg.effective_num_candidates(), 6);
        assert_eq!(cfg.live_conversion.beam_size, 1);
        assert_eq!(cfg.conversion.beam_size, 6);
    }

    #[test]
    fn live_conversion_min_chars_defaults_to_three() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.live_conversion.min_chars, 3);
    }

    #[test]
    fn live_conversion_min_chars_parses_from_toml() {
        let cfg: AppConfig = toml::from_str(
            r#"
[live_conversion]
enabled = true
min_chars = 2
"#,
        )
        .expect("config should parse");
        assert_eq!(cfg.live_conversion.min_chars, 2);
        assert!(cfg.live_conversion.enabled);
    }

    #[test]
    fn live_conversion_min_chars_falls_back_to_default_when_omitted() {
        let cfg: AppConfig = toml::from_str(
            r#"
[live_conversion]
enabled = true
"#,
        )
        .expect("config should parse");
        assert_eq!(cfg.live_conversion.min_chars, 3);
    }
}

#[cfg(test)]
mod config_load_failure_tests {
    //! 読み込みに失敗したときの扱い（Issue #61）
    use super::{ConfigManager, load_app_config_from_path};
    use std::path::PathBuf;

    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rakukan-config-test-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    pub(crate) fn write_config(path: &PathBuf, body: &str) {
        std::fs::write(path, body).expect("write config");
    }

    /// `[conversion] num_candidates` は publish_atomics が触らないので、
    /// 他のテストと干渉せずに「反映されたか」を見られる。
    pub(crate) const VALID: &str = "[conversion]\nnum_candidates = 7\n";
    pub(crate) const FIXED: &str = "[conversion]\nnum_candidates = 4\n";
    pub(crate) const BROKEN: &str = "[conversion\nnum_candidates = ";

    #[test]
    fn broken_toml_is_a_parse_error() {
        // 前提の確認: BROKEN は本当にパースできない
        let dir = temp_dir("precond");
        let path = dir.join("config.toml");
        write_config(&path, BROKEN);
        assert!(load_app_config_from_path(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_load_uses_defaults_on_parse_error() {
        let dir = temp_dir("first");
        let path = dir.join("config.toml");
        write_config(&path, BROKEN);

        // 初回は保持すべき前の設定が無いので既定値を使う
        let mgr = ConfigManager::from_path(path);
        assert_eq!(
            mgr.current.effective_num_candidates(),
            super::AppConfig::default().effective_num_candidates()
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reinit_keeps_the_previous_config_on_parse_error() {
        let dir = temp_dir("keep");
        let path = dir.join("config.toml");
        write_config(&path, VALID);

        let mut mgr = ConfigManager::from_path(path.clone());
        assert_eq!(mgr.current.effective_num_candidates(), 7);

        // 壊れた TOML に書き換えて読み直しても、既定値へ戻らない
        write_config(&path, BROKEN);
        mgr.reinit();
        assert_eq!(
            mgr.current.effective_num_candidates(),
            7,
            "壊れた TOML で設定が入れ替わった"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reinit_keeps_the_previous_config_when_the_file_is_missing() {
        let dir = temp_dir("missing");
        let path = dir.join("config.toml");
        write_config(&path, VALID);

        let mut mgr = ConfigManager::from_path(path.clone());
        assert_eq!(mgr.current.effective_num_candidates(), 7);

        // 削除を暗黙の「設定リセット」にしない
        std::fs::remove_file(&path).expect("remove");
        mgr.reinit();
        assert_eq!(
            mgr.current.effective_num_candidates(),
            7,
            "ファイル不在で設定が入れ替わった"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reinit_does_not_recreate_a_missing_file() {
        let dir = temp_dir("norecreate");
        let path = dir.join("config.toml");
        write_config(&path, VALID);

        let mut mgr = ConfigManager::from_path(path.clone());
        std::fs::remove_file(&path).expect("remove");
        mgr.reinit();

        // 再初期化の経路では config_save_default() を呼ばない
        assert!(!path.exists(), "再初期化がファイルを作り直した");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reinit_applies_a_fixed_toml() {
        let dir = temp_dir("fixed");
        let path = dir.join("config.toml");
        write_config(&path, VALID);

        let mut mgr = ConfigManager::from_path(path.clone());
        write_config(&path, BROKEN);
        mgr.reinit();
        assert_eq!(mgr.current.effective_num_candidates(), 7);

        // 直せば次の読み込みで反映される
        write_config(&path, FIXED);
        mgr.reinit();
        assert_eq!(
            mgr.current.effective_num_candidates(),
            4,
            "直した TOML が反映されない"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod config_load_warning_tests {
    //! 読み込み失敗の警告に何が残るか（Issue #61 の受入条件）
    use super::ConfigManager;
    use super::config_load_failure_tests::{BROKEN, VALID, temp_dir, write_config};
    use std::io::Write;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    /// tracing の出力を受け取る writer。
    #[derive(Clone)]
    struct LogCapture(Arc<Mutex<Vec<u8>>>);

    impl LogCapture {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Vec::new())))
        }
        fn text(&self) -> String {
            let buf = self.0.lock().expect("lock");
            String::from_utf8_lossy(&buf).into_owned()
        }
    }

    impl Write for LogCapture {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("lock").extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// `f` の間の WARN 以上を捕まえる。
    ///
    /// `with_default` はこのスレッドだけに効くので、並行して走る他のテストの
    /// 出力は混ざらない。
    fn capture_warnings<R>(f: impl FnOnce() -> R) -> (R, String) {
        let capture = LogCapture::new();
        let writer = capture.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || writer.clone())
            .finish();
        let value = tracing::subscriber::with_default(subscriber, f);
        (value, capture.text())
    }

    /// `error=` の後ろが空でないこと（エラー内容が残っていること）。
    fn has_error_detail(text: &str) -> bool {
        text.split("error=")
            .skip(1)
            .any(|rest| !rest.trim().is_empty())
    }

    fn assert_mentions_path(text: &str, path: &Path) {
        assert!(
            text.contains(&path.display().to_string()),
            "警告にパスが無い: {text:?}"
        );
    }

    #[test]
    fn first_load_warns_with_path_and_error() {
        let dir = temp_dir("warn-first");
        let path = dir.join("config.toml");
        write_config(&path, BROKEN);

        let (_mgr, logs) = capture_warnings(|| ConfigManager::from_path(path.clone()));

        assert!(logs.contains("WARN"), "WARN で出ていない: {logs:?}");
        assert_mentions_path(&logs, &path);
        assert!(has_error_detail(&logs), "エラー内容が無い: {logs:?}");
        // 既定値を使ったことが分かる
        assert!(
            logs.contains("starting with defaults"),
            "既定値を使う旨が無い: {logs:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reinit_warns_with_path_and_error_on_parse_error() {
        let dir = temp_dir("warn-parse");
        let path = dir.join("config.toml");
        write_config(&path, VALID);
        let mut mgr = ConfigManager::from_path(path.clone());

        write_config(&path, BROKEN);
        let (_, logs) = capture_warnings(|| mgr.reinit());

        assert!(logs.contains("WARN"), "WARN で出ていない: {logs:?}");
        assert_mentions_path(&logs, &path);
        assert!(has_error_detail(&logs), "エラー内容が無い: {logs:?}");
        // 前の設定を保持したことが分かる
        assert!(
            logs.contains("keeping previous config"),
            "設定を保持した旨が無い: {logs:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reinit_warns_with_path_and_error_when_the_file_is_missing() {
        let dir = temp_dir("warn-missing");
        let path = dir.join("config.toml");
        write_config(&path, VALID);
        let mut mgr = ConfigManager::from_path(path.clone());

        std::fs::remove_file(&path).expect("remove");
        let (_, logs) = capture_warnings(|| mgr.reinit());

        assert!(logs.contains("WARN"), "WARN で出ていない: {logs:?}");
        assert_mentions_path(&logs, &path);
        // ファイル不在も理由が残る（OS のエラー文言に依存しないよう中身は問わない）
        assert!(has_error_detail(&logs), "エラー内容が無い: {logs:?}");
        assert!(
            logs.contains("keeping previous config"),
            "設定を保持した旨が無い: {logs:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn successful_load_does_not_warn() {
        let dir = temp_dir("warn-none");
        let path = dir.join("config.toml");
        write_config(&path, VALID);

        // 捕捉機構が生きていることを先に確かめる。
        // これが無いと、捕まえ損ねているだけの「警告なし」でも通ってしまう。
        let (_, sanity) = capture_warnings(|| tracing::warn!("capture check"));
        assert!(
            sanity.contains("capture check"),
            "警告を捕まえられていない: {sanity:?}"
        );

        // 初回と再初期化のどちらも、成功時は警告を出さない
        let (mut mgr, first) = capture_warnings(|| ConfigManager::from_path(path.clone()));
        let (_, again) = capture_warnings(|| mgr.reinit());

        for logs in [&first, &again] {
            assert!(
                !logs.contains("config.toml load failed"),
                "成功したのに警告が出た: {logs:?}"
            );
            assert!(
                !logs.contains("config.toml reload failed"),
                "成功したのに警告が出た: {logs:?}"
            );
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
