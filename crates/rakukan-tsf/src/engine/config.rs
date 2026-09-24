use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

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

// ─── 設定の組（Issue #65）─────────────────────────────────────────────
//
// 設定の読込・公開、反映待ちの管理、エンジンへの送信を分ける。
// 「設定が変わったこと」と「エンジンへの反映を依頼されたこと」は別に持つ。
// - 読込は全経路を `reload_config` に集約する（読込専用ロック → 読取・本文比較・解析・
//   JSON 生成 → 設定状態のロックで一括公開）。設定状態のロックはファイル I/O 中に持たない
// - 読込からエンジンへの RPC は呼ばない。反映は既存の契機（保存イベント・条件付き
//   モード切替・手動再起動）が `state::engine_reload*` で行う
// - 反映待ち（`pending_apply`）の解除は、送った組と現在の反映待ちが一致し、成功応答を
//   得た場合だけ。通信失敗では保持する

/// 設定の出どころ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// `config.toml` を読んで解析できた。`bytes` は読み取った元の本文（次回の比較に使う）、
    /// `sha256` はそのハッシュ（正規化前のバイト列から計算する。#66 でホストへ渡す識別子）。
    File { bytes: Arc<[u8]>, sha256: [u8; 32] },
    /// 初回に正常な本文を得られず、既定値で始めた（Issue #61）。
    /// 実在する本文のハッシュを持つ扱いにはしない。
    Defaults,
}

impl ConfigSource {
    pub fn sha256(&self) -> Option<[u8; 32]> {
        match self {
            ConfigSource::File { sha256, .. } => Some(*sha256),
            ConfigSource::Defaults => None,
        }
    }
}

/// 同じ読み取りから作った、変更後に書き換えない設定の組。
#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    /// プロセス内の更新連番（応答の照合用）。
    pub revision: u64,
    pub source: ConfigSource,
    pub app_config: AppConfig,
    /// `app_config` だけから生成した EngineConfig JSON。
    pub engine_json: String,
}

impl ConfigSnapshot {
    pub fn apply_id(&self) -> ApplyId {
        ApplyId {
            revision: self.revision,
            sha256: self.source.sha256(),
        }
    }
}

/// 反映待ち・送信中の組の識別子。既定値由来は `sha256 = None`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApplyId {
    pub revision: u64,
    pub sha256: Option<[u8; 32]>,
}

impl std::fmt::Display for ApplyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.sha256 {
            Some(h) => write!(f, "rev={} sha256={}", self.revision, hex_prefix(&h)),
            None => write!(f, "rev={} sha256=none(defaults)", self.revision),
        }
    }
}

fn hex_prefix(h: &[u8; 32]) -> String {
    h[..6].iter().map(|b| format!("{b:02x}")).collect()
}

/// エンジンへの反映を求める契機。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyTrigger {
    /// 名前付きイベント（設定アプリの保存・トレイの「エンジン再起動」）
    SaveEvent,
    /// IME モード切替（`reload_on_mode_switch = true`）
    ModeSwitch,
    /// 言語バーの「エンジン再起動」（反映待ちが無くても送る）
    ManualRestart,
}

/// ホストへ送った結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    /// `ShutdownIfConfigDiffers` が `Bool(false)`（ホストは同じ設定で動作中）
    SameConfig,
    /// `ShutdownIfConfigDiffers` が `Bool(true)`（再起動の受理。新設定での起動確認ではない）
    RestartAccepted,
    /// `Shutdown` の `Unit` を受信した。`fallback` は比較に失敗して無条件終了へ回った場合
    ShutdownAcknowledged { fallback: bool },
    /// 通信失敗・異常応答（`Ok(())` に潰した通信失敗を含む）。反映待ちは保持する
    CommFailure,
}

/// 送信中の組と契機。
#[derive(Debug, Clone)]
pub struct InFlight {
    pub id: ApplyId,
    pub trigger: ApplyTrigger,
}

/// 読込の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOutcome {
    /// 本文が前回と同じ（解析・公開を省略した）
    Unchanged,
    /// 新しい組を公開した
    Updated,
    /// 読めない・解析できない。直前の組を保持した
    Failed,
}

pub struct ConfigManager {
    path: PathBuf,
    current: Arc<ConfigSnapshot>,
    next_revision: u64,
    pending_apply: Option<ApplyId>,
    in_flight: Option<InFlight>,
    /// 直近の読込失敗（エラー文）。同じ失敗が続く間は WARN を繰り返さず、
    /// 失敗の内容が変わったときと、読めるようになったときに記録する。再確認自体は続ける
    last_load_failure: Option<String>,
}

/// 読込失敗の記録。戻り値は「新しい失敗として WARN すべきか」。
fn note_load_failure(last: &mut Option<String>, error: &str) -> bool {
    if last.as_deref() == Some(error) {
        return false;
    }
    *last = Some(error.to_string());
    true
}

/// 読込成功の記録。戻り値は「失敗から回復したか」。
fn note_load_success(last: &mut Option<String>) -> bool {
    last.take().is_some()
}

fn sha256_of(bytes: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).into()
}

/// 読み取った本文から組を作る（ロックなし）。
fn build_snapshot_from_bytes(bytes: Vec<u8>, revision: u64) -> Result<ConfigSnapshot> {
    let text = std::str::from_utf8(&bytes)?;
    let cfg: AppConfig = toml::from_str(text)?;
    let engine_json = engine_config_json(&cfg);
    Ok(ConfigSnapshot {
        revision,
        source: ConfigSource::File {
            sha256: sha256_of(&bytes),
            bytes: bytes.into(),
        },
        app_config: cfg,
        engine_json,
    })
}

fn defaults_snapshot(revision: u64) -> ConfigSnapshot {
    let cfg = AppConfig::default();
    ConfigSnapshot {
        revision,
        source: ConfigSource::Defaults,
        engine_json: engine_config_json(&cfg),
        app_config: cfg,
    }
}

/// 読み取り（ロックなし）。本文が前回と同じなら `Ok(None)`（解析も公開もしない）。
fn read_and_build(
    path: &Path,
    prev: &ConfigSnapshot,
    revision: u64,
) -> Result<Option<ConfigSnapshot>> {
    let bytes = std::fs::read(path)?;
    if let ConfigSource::File {
        bytes: prev_bytes, ..
    } = &prev.source
        && prev_bytes[..] == bytes[..]
    {
        return Ok(None);
    }
    Ok(Some(build_snapshot_from_bytes(bytes, revision)?))
}

fn log_engine_config(snapshot: &ConfigSnapshot) {
    tracing::info!(
        "engine config: {} ({})",
        snapshot.engine_json,
        snapshot.apply_id()
    );
}

impl ConfigManager {
    fn new() -> Self {
        Self::from_path(config_path().unwrap_or_else(|_| PathBuf::from("config.toml")))
    }

    /// 初回の読み込み（Issue #61）。
    ///
    /// 保持すべき前の設定が無いので、失敗したら既定値を使う。ただし**必ず警告を
    /// 残す**。無言で既定値へ戻すと、利用者からは「設定が勝手に初期化された」と
    /// しか見えない。既定値由来の組は `ConfigSource::Defaults` で、本文のハッシュを持たない。
    pub(crate) fn from_path(path: PathBuf) -> Self {
        let snapshot = match std::fs::read(&path)
            .map_err(anyhow::Error::from)
            .and_then(|bytes| build_snapshot_from_bytes(bytes, 1))
        {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "config.toml load failed; starting with defaults: path={} error={e}",
                    path.display()
                );
                defaults_snapshot(1)
            }
        };
        publish_atomics(&snapshot.app_config);
        log_engine_config(&snapshot);
        Self {
            path,
            current: Arc::new(snapshot),
            next_revision: 2,
            pending_apply: None,
            in_flight: None,
            last_load_failure: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Arc<ConfigSnapshot> {
        self.current.clone()
    }

    #[cfg(test)]
    pub(crate) fn app_config(&self) -> &AppConfig {
        &self.current.app_config
    }

    /// 読み直し（単一スレッド版。本番の経路はすべて `reload_config` を通る）。
    ///
    /// **読めなければ直前の組を保つ**（Issue #61）。ファイルが無い場合も保持する
    /// （削除を暗黙の「設定リセット」にしない）。ここでは `config_save_default()` を
    /// 呼ばず、ファイルを作り直さない。本文が同じなら解析も公開もしない。
    #[cfg(test)]
    pub(crate) fn reinit(&mut self) -> LoadOutcome {
        let prev = self.current.clone();
        let path = self.path.clone();
        match read_and_build(&path, &prev, self.next_revision) {
            Ok(None) => {
                self.note_success();
                LoadOutcome::Unchanged
            }
            Ok(Some(snapshot)) => {
                self.note_success();
                self.publish(snapshot);
                LoadOutcome::Updated
            }
            Err(e) => {
                self.note_failure(&e, "reinit");
                LoadOutcome::Failed
            }
        }
    }

    /// 読込失敗を記録する。同じ失敗の間は WARN を繰り返さない（#65: 「WARN を 1 回」）。
    fn note_failure(&mut self, e: &anyhow::Error, reason: &str) {
        let text = e.to_string();
        if note_load_failure(&mut self.last_load_failure, &text) {
            tracing::warn!(
                "config.toml reload failed; keeping previous config: path={} error={text} ({reason})",
                self.path.display()
            );
        } else {
            tracing::debug!(
                "config.toml still unreadable; keeping previous config: path={} ({reason})",
                self.path.display()
            );
        }
    }

    fn note_success(&mut self) {
        if note_load_success(&mut self.last_load_failure) {
            tracing::info!("config.toml readable again: path={}", self.path.display());
        }
    }

    /// 新しい組を公開する。
    ///
    /// - `engine_json` が変わったら、現在の組を反映待ちにする
    /// - 変わっていなくても既に反映待ちなら、識別子を新しい組へ更新する
    ///   （コメントだけの変更でも、応答との対応を崩さない）
    fn publish(&mut self, snapshot: ConfigSnapshot) {
        let prev = std::mem::replace(&mut self.current, Arc::new(snapshot));
        self.next_revision = self.current.revision.saturating_add(1);
        publish_atomics(&self.current.app_config);
        let json_changed = prev.engine_json != self.current.engine_json;
        if json_changed {
            log_engine_config(&self.current);
        }
        let id = self.current.apply_id();
        if json_changed || self.pending_apply.is_some() {
            self.pending_apply = Some(id.clone());
        }
        tracing::info!(
            "config published: {id} engine_json_changed={json_changed} pending_apply={}",
            self.pending_apply.is_some()
        );
    }

    pub(crate) fn has_pending_apply(&self) -> bool {
        self.pending_apply.is_some()
    }

    /// 送信を始める。`force` でなければ反映待ちが無いときは `None`（送らない）。
    /// 返す JSON は送信時点の組に固定する。
    pub(crate) fn begin_apply(
        &mut self,
        trigger: ApplyTrigger,
        force: bool,
    ) -> Option<(ApplyId, String)> {
        if !force && self.pending_apply.is_none() {
            return None;
        }
        let id = self.current.apply_id();
        self.in_flight = Some(InFlight {
            id: id.clone(),
            trigger,
        });
        Some((id, self.current.engine_json.clone()))
    }

    /// 応答を反映する。反映待ちを解除するのは、送った組が現在の反映待ちと一致し、
    /// 成功応答（同じ設定・再起動受理・終了応答）を得たときだけ。通信失敗では保持する。
    /// 戻り値は解除したかどうか。
    pub(crate) fn finish_apply(&mut self, sent: &ApplyId, outcome: ApplyOutcome) -> bool {
        let trigger = match self.in_flight.take_if(|f| &f.id == sent) {
            Some(f) => Some(f.trigger),
            None => None,
        };
        let cleared = match outcome {
            ApplyOutcome::CommFailure => false,
            ApplyOutcome::SameConfig
            | ApplyOutcome::RestartAccepted
            | ApplyOutcome::ShutdownAcknowledged { .. } => {
                if self.pending_apply.as_ref() == Some(sent) {
                    self.pending_apply = None;
                    true
                } else {
                    false
                }
            }
        };
        tracing::info!(
            "config apply: sent=({sent}) trigger={trigger:?} outcome={outcome:?} cleared={cleared} pending_now={}",
            self.pending_apply
                .as_ref()
                .map(|p| p.to_string())
                .unwrap_or_else(|| "none".into())
        );
        cleared
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

/// 候補ウィンドウの表示直前に呼ぶ。同期読込はせず、背景の監視へ読込を要求するだけ。
/// 描画は公開済みの設定で行い、変更は読込完了後の次回表示から効く（Issue #65）。
pub fn request_background_reload() {
    super::config_watch::request_reload();
}

/// 読込専用ロック。古い読み取りが遅れて公開される順序を防ぐ。
/// 設定状態（`CONFIG_MANAGER`）のロックはファイル I/O 中に持たない。
static LOAD_LOCK: Mutex<()> = Mutex::new(());

static CONFIG_MANAGER: LazyLock<Mutex<ConfigManager>> =
    LazyLock::new(|| Mutex::new(ConfigManager::new()));

fn lock_manager() -> std::sync::MutexGuard<'static, ConfigManager> {
    match CONFIG_MANAGER.lock() {
        Ok(g) => g,
        Err(p) => {
            tracing::warn!("config manager poisoned, recovering");
            p.into_inner()
        }
    }
}

pub fn config_path() -> Result<PathBuf> {
    let appdata = std::env::var("APPDATA").map_err(|_| anyhow::anyhow!("APPDATA not set"))?;
    Ok(PathBuf::from(appdata).join("rakukan").join("config.toml"))
}

#[cfg(test)]
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

/// 共通の読込処理（すべての読込経路がここを通る、Issue #65）。
///
/// 読込専用ロック → ファイル読取 → 本文比較 → ハッシュ・解析・JSON 生成 →
/// 設定状態のロックで一括公開。**読めなければ直前の組を保つ**（Issue #61）。
/// この処理からエンジンへの RPC は呼ばない。
pub fn reload_config(reason: &'static str) -> LoadOutcome {
    let _load = match LOAD_LOCK.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    let (path, prev, revision) = {
        let mgr = lock_manager();
        (mgr.path.clone(), mgr.current.clone(), mgr.next_revision)
    };
    match read_and_build(&path, &prev, revision) {
        Ok(None) => {
            // 変更なしの読込は定期確認で 30 秒ごとに起きるので trace に留める
            // （debug 運用でプロセス数に比例してログが増えるのを避ける）
            tracing::trace!("config unchanged ({reason})");
            lock_manager().note_success();
            LoadOutcome::Unchanged
        }
        Ok(Some(snapshot)) => {
            let mut mgr = lock_manager();
            // 読込専用ロックの下なので、prev から進んだ組は無い
            mgr.note_success();
            mgr.publish(snapshot);
            LoadOutcome::Updated
        }
        Err(e) => {
            // 同じ失敗の間は WARN を繰り返さない（定期確認・候補表示・通知で再読込は続く）
            lock_manager().note_failure(&e, reason);
            LoadOutcome::Failed
        }
    }
}

/// config.toml を読み直す。**読めなければ直前の設定を保つ**（Issue #61）。
///
/// 呼び出し元は DllMain（初回）と言語バーの「エンジン再起動」。初回だけは、
/// 直前に `config_save_default()` がファイルを作る。
pub fn init_config_manager() {
    {
        let mut mgr = lock_manager();
        mgr.path = config_path().unwrap_or_else(|_| mgr.path.clone());
    }
    let _ = reload_config("init");
}

pub fn current_config() -> AppConfig {
    lock_manager().current.app_config.clone()
}

/// 公開済みの設定の組（変更後に書き換えない）。
pub fn current_snapshot() -> Arc<ConfigSnapshot> {
    lock_manager().current.clone()
}

pub fn effective_num_candidates() -> usize {
    current_config().effective_num_candidates()
}

pub fn keyboard_layout() -> KeyboardLayout {
    current_config().keyboard.layout
}

pub fn has_pending_apply() -> bool {
    lock_manager().has_pending_apply()
}

/// 送信を始める（`state::engine_reload*` から、`RAKUKAN_ENGINE` のロック下で呼ぶ）。
pub fn begin_apply(trigger: ApplyTrigger, force: bool) -> Option<(ApplyId, String)> {
    lock_manager().begin_apply(trigger, force)
}

/// 応答を反映する（同上）。
pub fn finish_apply(sent: &ApplyId, outcome: ApplyOutcome) -> bool {
    lock_manager().finish_apply(sent, outcome)
}

/// IME モード切替時の読み直し。`reload_on_mode_switch = true` のときだけ、共通の
/// 読込処理を同期で通す。戻り値は**エンジンへの反映待ちがあるか**（本文が変わったか
/// ではない。背景の監視が先に読んでいても、反映待ちが残っていれば処理する）。
pub fn maybe_reload_on_mode_switch() -> bool {
    if !lock_manager()
        .current
        .app_config
        .keyboard
        .reload_on_mode_switch
    {
        return false;
    }
    let outcome = reload_config("mode_switch");
    let pending = has_pending_apply();
    if outcome == LoadOutcome::Updated {
        let cfg = current_config();
        tracing::info!(
            "config.toml reloaded on mode switch: layout={:?} num_candidates={} live_conversion={} pending_apply={pending}",
            cfg.keyboard.layout,
            cfg.effective_num_candidates(),
            cfg.live_conversion.enabled,
        );
    }
    pending
}

/// `AppConfig` から EngineConfig JSON を作る（純粋。途中で `current_config()` を読まない）。
pub fn engine_config_json(cfg: &AppConfig) -> String {
    let num_candidates = cfg.effective_num_candidates();
    let main_gpu = cfg.general.main_gpu;
    let n_gpu_layers = cfg.general.n_gpu_layers.unwrap_or(u32::MAX);
    let model_variant = cfg.general.model_variant.clone();
    let digit_width = match cfg.input.digit_width {
        DigitWidth::Fullwidth => "fullwidth",
        DigitWidth::Halfwidth => "halfwidth",
    };
    let alpha_width = match cfg.input.alpha_width {
        AlphaWidth::Fullwidth => "fullwidth",
        AlphaWidth::Halfwidth => "halfwidth",
    };
    let symbol_width = match cfg.input.symbol_width {
        SymbolWidth::Fullwidth => "fullwidth",
        SymbolWidth::Halfwidth => "halfwidth",
    };
    let live_conv_beam_size = cfg.live_conversion.beam_size.clamp(1, 9);
    let convert_beam_size = cfg.conversion.beam_size.clamp(1, 30);
    let digit_separator_auto = cfg.input.digit_separator_auto;
    let digit_candidates_order = cfg
        .input
        .digit_candidates_order
        .iter()
        .map(|kind| match kind {
            DigitCandidateKind::Arabic => r#""arabic""#,
            DigitCandidateKind::Fullwidth => r#""fullwidth""#,
            DigitCandidateKind::Positional => r#""positional""#,
            DigitCandidateKind::PerDigit => r#""per_digit""#,
            DigitCandidateKind::Daiji => r#""daiji""#,
        })
        .collect::<Vec<_>>()
        .join(",");
    // 診断用の強制失敗（Issue #43）。既定 false なので通常は JSON に載らない。
    let force_inference_failure = cfg.diagnostics.force_inference_failure;
    let mv_json = match &model_variant {
        Some(v) => format!(r#","model_variant":"{}""#, v),
        None => String::new(),
    };
    format!(
        r#"{{"num_candidates":{num_candidates},"n_gpu_layers":{n_gpu_layers},"main_gpu":{main_gpu},"n_threads":0,"digit_width":"{digit_width}","alpha_width":"{alpha_width}","symbol_width":"{symbol_width}","digit_separator_auto":{digit_separator_auto},"digit_candidates_order":[{digit_candidates_order}],"live_conv_beam_size":{live_conv_beam_size},"convert_beam_size":{convert_beam_size},"force_inference_failure":{force_inference_failure}{mv_json}}}"#
    )
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
            mgr.app_config().effective_num_candidates(),
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
        assert_eq!(mgr.app_config().effective_num_candidates(), 7);

        // 壊れた TOML に書き換えて読み直しても、既定値へ戻らない
        write_config(&path, BROKEN);
        mgr.reinit();
        assert_eq!(
            mgr.app_config().effective_num_candidates(),
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
        assert_eq!(mgr.app_config().effective_num_candidates(), 7);

        // 削除を暗黙の「設定リセット」にしない
        std::fs::remove_file(&path).expect("remove");
        mgr.reinit();
        assert_eq!(
            mgr.app_config().effective_num_candidates(),
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
        assert_eq!(mgr.app_config().effective_num_candidates(), 7);

        // 直せば次の読み込みで反映される
        write_config(&path, FIXED);
        mgr.reinit();
        assert_eq!(
            mgr.app_config().effective_num_candidates(),
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
    pub(crate) fn capture_warnings<R>(f: impl FnOnce() -> R) -> (R, String) {
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

#[cfg(test)]
mod config_snapshot_tests {
    //! 設定の組と反映待ちの管理（Issue #65）
    use super::config_load_failure_tests::{BROKEN, FIXED, VALID, temp_dir, write_config};
    use super::{ApplyOutcome, ApplyTrigger, ConfigManager, ConfigSource, LoadOutcome};

    fn mgr_with(body: &str) -> (ConfigManager, std::path::PathBuf, std::path::PathBuf) {
        let dir = temp_dir("snap");
        let path = dir.join("config.toml");
        write_config(&path, body);
        (ConfigManager::from_path(path.clone()), path, dir)
    }

    #[test]
    fn first_load_from_file_has_hash_and_bytes() {
        let (mgr, _path, dir) = mgr_with(VALID);
        let snap = mgr.snapshot();
        assert_eq!(snap.revision, 1);
        match &snap.source {
            ConfigSource::File { bytes, sha256 } => {
                assert_eq!(&bytes[..], VALID.as_bytes());
                assert_eq!(*sha256, super::sha256_of(VALID.as_bytes()));
            }
            ConfigSource::Defaults => panic!("ファイル由来のはず"),
        }
        assert!(snap.apply_id().sha256.is_some());
        assert!(!mgr.has_pending_apply(), "初回は反映待ちにしない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_load_failure_is_defaults_without_hash() {
        let (mgr, _path, dir) = mgr_with(BROKEN);
        let snap = mgr.snapshot();
        assert_eq!(snap.source, ConfigSource::Defaults);
        assert_eq!(
            snap.apply_id().sha256,
            None,
            "既定値由来は本文のハッシュを持たない"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_bytes_are_not_reparsed_or_republished() {
        let (mut mgr, _path, dir) = mgr_with(VALID);
        assert_eq!(mgr.reinit(), LoadOutcome::Unchanged);
        assert_eq!(mgr.snapshot().revision, 1, "同じ本文では組を作り直さない");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_engine_json_sets_pending_apply() {
        // VALID → FIXED は num_candidates が変わるので JSON が変わる
        let (mut mgr, path, dir) = mgr_with(VALID);
        write_config(&path, FIXED);
        assert_eq!(mgr.reinit(), LoadOutcome::Updated);
        assert_eq!(mgr.snapshot().revision, 2);
        assert!(mgr.has_pending_apply());
        assert_eq!(mgr.pending_apply.as_ref().unwrap().revision, 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn comment_only_change_keeps_pending_but_moves_its_id() {
        let (mut mgr, path, dir) = mgr_with(VALID);
        // 本文が変わり JSON も変わる → 反映待ち rev=2
        write_config(&path, FIXED);
        mgr.reinit();
        assert_eq!(mgr.pending_apply.as_ref().unwrap().revision, 2);
        // コメントだけの変更（JSON は同じ）→ 反映待ちは残り、識別子は rev=3 へ
        write_config(&path, &format!("# comment\n{FIXED}"));
        assert_eq!(mgr.reinit(), LoadOutcome::Updated);
        assert_eq!(mgr.pending_apply.as_ref().unwrap().revision, 3);
        // 反映待ちが無い状態でコメントだけ変えても、反映待ちは立たない
        mgr.pending_apply = None;
        write_config(&path, &format!("# another\n{FIXED}"));
        assert_eq!(mgr.reinit(), LoadOutcome::Updated);
        assert!(!mgr.has_pending_apply());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_reload_keeps_previous_snapshot_and_pending() {
        let (mut mgr, path, dir) = mgr_with(VALID);
        write_config(&path, FIXED);
        mgr.reinit();
        let before = mgr.snapshot();
        write_config(&path, BROKEN);
        assert_eq!(mgr.reinit(), LoadOutcome::Failed);
        let after = mgr.snapshot();
        assert_eq!(
            after.revision, before.revision,
            "壊れた本文で組を入れ替えない"
        );
        assert_eq!(
            after.source, before.source,
            "新しいハッシュと以前の設定を組み合わせない"
        );
        assert!(mgr.has_pending_apply());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn begin_apply_only_when_pending_unless_forced() {
        let (mut mgr, _path, dir) = mgr_with(VALID);
        assert!(mgr.begin_apply(ApplyTrigger::SaveEvent, false).is_none());
        let (id, json) = mgr
            .begin_apply(ApplyTrigger::ManualRestart, true)
            .expect("force sends the current snapshot");
        assert_eq!(id.revision, 1);
        assert_eq!(json, mgr.snapshot().engine_json);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn response_for_a_clears_only_a_not_b() {
        // A を送信中に背景監視が B を読む → A の応答で B の反映待ちを消さない
        let (mut mgr, path, dir) = mgr_with(VALID);
        write_config(&path, FIXED);
        mgr.reinit();
        let (a, _) = mgr
            .begin_apply(ApplyTrigger::SaveEvent, false)
            .expect("A pending");
        // 送信中に B（num_candidates = 9）を検出
        write_config(&path, "[conversion]\nnum_candidates = 9\n");
        assert_eq!(mgr.reinit(), LoadOutcome::Updated);
        let b = mgr.pending_apply.clone().expect("B pending");
        assert_ne!(a, b);
        // A の成功応答
        assert!(
            !mgr.finish_apply(&a, ApplyOutcome::SameConfig),
            "A の応答では解除しない"
        );
        assert_eq!(mgr.pending_apply.as_ref(), Some(&b), "B の反映待ちが残る");
        // B を送って成功 → 解除
        let (b2, _) = mgr
            .begin_apply(ApplyTrigger::SaveEvent, false)
            .expect("B pending");
        assert_eq!(b2, b);
        assert!(mgr.finish_apply(&b2, ApplyOutcome::RestartAccepted));
        assert!(!mgr.has_pending_apply());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_to_b_to_a_does_not_let_an_old_response_clear_the_new_pending() {
        // 同じ本文へ戻る A→B→A でも識別子（revision）が違うので、古い応答は新しい反映待ちを消さない
        let (mut mgr, path, dir) = mgr_with(VALID);
        write_config(&path, FIXED);
        mgr.reinit();
        let (a1, _) = mgr
            .begin_apply(ApplyTrigger::SaveEvent, false)
            .expect("A pending");
        write_config(&path, VALID);
        mgr.reinit();
        write_config(&path, FIXED);
        mgr.reinit();
        let a3 = mgr.pending_apply.clone().expect("A again");
        assert_eq!(a1.sha256, a3.sha256, "本文は同じ");
        assert_ne!(a1.revision, a3.revision);
        assert!(!mgr.finish_apply(&a1, ApplyOutcome::SameConfig));
        assert!(mgr.has_pending_apply());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn comm_failure_keeps_pending() {
        let (mut mgr, path, dir) = mgr_with(VALID);
        write_config(&path, FIXED);
        mgr.reinit();
        let (id, _) = mgr
            .begin_apply(ApplyTrigger::SaveEvent, false)
            .expect("pending");
        assert!(!mgr.finish_apply(&id, ApplyOutcome::CommFailure));
        assert!(mgr.has_pending_apply(), "通信失敗では反映待ちを保持する");
        assert!(mgr.in_flight.is_none(), "送信中の記録は消す");
        // フォールバックの Shutdown で Unit を受信したら解除する
        let (id, _) = mgr
            .begin_apply(ApplyTrigger::SaveEvent, false)
            .expect("pending");
        assert!(mgr.finish_apply(&id, ApplyOutcome::ShutdownAcknowledged { fallback: true }));
        assert!(!mgr.has_pending_apply());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_identical_failure_warns_once_and_keeps_rechecking() {
        use super::config_load_warning_tests::capture_warnings;
        let (mut mgr, path, dir) = mgr_with(VALID);
        write_config(&path, BROKEN);
        let (first, logs1) = capture_warnings(|| mgr.reinit());
        assert_eq!(first, LoadOutcome::Failed);
        assert!(
            logs1.contains("reload failed"),
            "最初の失敗は WARN: {logs1:?}"
        );
        // 同じ失敗の再確認では WARN を繰り返さない（再確認自体は行う）
        let (again, logs2) = capture_warnings(|| mgr.reinit());
        assert_eq!(again, LoadOutcome::Failed);
        assert!(
            !logs2.contains("reload failed"),
            "同じ失敗で WARN を繰り返した: {logs2:?}"
        );
        // 失敗の内容が変わったら（欠落）改めて WARN
        std::fs::remove_file(&path).expect("remove");
        let (missing, logs3) = capture_warnings(|| mgr.reinit());
        assert_eq!(missing, LoadOutcome::Failed);
        assert!(
            logs3.contains("reload failed"),
            "別の失敗は WARN: {logs3:?}"
        );
        // 読めるようになったら回復し、次の失敗はまた WARN
        write_config(&path, FIXED);
        assert_eq!(mgr.reinit(), LoadOutcome::Updated);
        write_config(&path, BROKEN);
        let (_, logs4) = capture_warnings(|| mgr.reinit());
        assert!(
            logs4.contains("reload failed"),
            "回復後の失敗は WARN: {logs4:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changed_bytes_with_same_mtime_and_size_are_published() {
        use std::fs::{File, FileTimes};

        let (mut mgr, path, dir) = mgr_with(VALID);
        let before = std::fs::metadata(&path).expect("metadata");
        let mtime = before.modified().expect("modified");
        let revision = mgr.snapshot().revision;
        assert_eq!(VALID.len(), FIXED.len());

        write_config(&path, FIXED);
        File::options()
            .write(true)
            .open(&path)
            .expect("open")
            .set_times(FileTimes::new().set_modified(mtime))
            .expect("restore mtime");
        let after = std::fs::metadata(&path).expect("metadata after");
        assert_eq!(after.len(), before.len(), "size must be unchanged");
        assert_eq!(after.modified().expect("modified after"), mtime);

        assert_eq!(mgr.reinit(), LoadOutcome::Updated);
        assert_eq!(mgr.snapshot().revision, revision + 1);
        assert_eq!(mgr.app_config().effective_num_candidates(), 4);
        assert!(mgr.has_pending_apply());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn engine_json_is_pure_and_matches_snapshot() {
        let (mgr, _path, dir) = mgr_with(VALID);
        let snap = mgr.snapshot();
        assert_eq!(
            super::engine_config_json(&snap.app_config),
            snap.engine_json
        );
        assert!(snap.engine_json.contains(r#""num_candidates":7"#));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
