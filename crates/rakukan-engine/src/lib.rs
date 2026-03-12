//! rakukan 変換エンジン
//!
//! karukan-engine のコードを直接統合したクレート。
//! 外部 git 依存なし。
//!
//! ```text
//! ローマ字 → RomajiConverter → ひらがな → (1) 辞書引き（同期）
//!                                          (2) KanaKanjiConverter（LLM, 非同期）
//!                                          → 候補マージ → 返却
//! ```

// ── 統合した karukan-engine モジュール ────────────────────────────────────────
pub mod kana;
pub mod kanji;
pub mod romaji;

pub use kana::{hiragana_to_halfwidth_katakana, hiragana_to_katakana, katakana_to_hiragana, normalize_nfkc};
pub use kanji::{Backend, KanaKanjiConverter};
pub use romaji::{BackspaceResult, ConversionEvent, RomajiConverter};

// ── rakukan 独自モジュール ────────────────────────────────────────────────────
pub mod backend;
pub mod conv_cache;
pub mod ffi;
pub use backend::{BackendSelection, GpuInfo, select_backend};
// Backend は kanji::Backend と名前が被るため、rakukan の Backend は別名でエクスポート
pub use backend::Backend as RakunBackend;

pub use rakukan_dict::{DictStore, find_skk_jisyo, find_mozc_dict, user_dict_path};

use kanji::{registry, Backend as KarukanBackend};
use thiserror::Error;
use tracing::{debug, info};

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("エンジン初期化失敗: {0}")]
    InitFailed(String),
    #[error("変換エラー: {0}")]
    ConversionFailed(String),
    #[error("モデル未初期化（init_kanji() を先に呼んでください）")]
    ModelNotInitialized,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    pub model_variant: Option<String>,
    pub num_candidates: usize,
    pub n_threads: u32,
    /// GPU レイヤー数 (u32::MAX = 全レイヤー, 0 = CPU のみ)
    pub n_gpu_layers: u32,
    /// 使用する GPU インデックス (0 = 最初の GPU, -1 = 自動)
    pub main_gpu: i32,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self { model_variant: None, num_candidates: 5, n_threads: 0, n_gpu_layers: 0u32, main_gpu: 0 }
    }
}

#[derive(Debug, Clone, Default)]
pub struct PreeditState {
    pub hiragana: String,
    pub pending_romaji: String,
}

impl PreeditState {
    pub fn display(&self) -> String {
        format!("{}{}", self.hiragana, self.pending_romaji)
    }
    pub fn is_empty(&self) -> bool {
        self.hiragana.is_empty() && self.pending_romaji.is_empty()
    }
}

pub struct RakunEngine {
    romaji: RomajiConverter,
    kanji: Option<KanaKanjiConverter>,
    config: EngineConfig,
    hiragana_buf: String,
    pending_romaji_buf: String,
    committed: String,
    dict_store: Option<DictStore>,
}

impl RakunEngine {
    pub fn new(config: EngineConfig) -> Self {
        Self {
            romaji: RomajiConverter::new(),
            kanji: None,
            config,
            hiragana_buf: String::new(),
            pending_romaji_buf: String::new(),
            committed: String::new(),
            dict_store: None,
        }
    }

    pub fn init_kanji(&mut self) -> Result<(), EngineError> {
        let converter = Self::build_converter(&self.config)?;
        self.kanji = Some(converter);
        Ok(())
    }

    pub fn build_converter(config: &EngineConfig) -> Result<KanaKanjiConverter, EngineError> {
        let variant_id = config
            .model_variant
            .clone()
            .unwrap_or_else(|| registry().default_model.clone());
        info!("モデル初期化中: {} (n_gpu_layers={}, main_gpu={})", variant_id, config.n_gpu_layers, config.main_gpu);
        let backend = KarukanBackend::from_variant_id(&variant_id)
            .map_err(|e| EngineError::InitFailed(e.to_string()))?
            .with_n_gpu_layers(config.n_gpu_layers)
            .with_main_gpu(config.main_gpu);
        let mut converter = KanaKanjiConverter::new(backend)
            .map_err(|e| EngineError::InitFailed(e.to_string()))?;
        if config.n_threads > 0 {
            converter.set_n_threads(config.n_threads);
        }
        info!("モデル初期化完了: {}", converter.model_display_name());
        Ok(converter)
    }

    pub fn set_kanji_converter(&mut self, converter: KanaKanjiConverter) {
        self.kanji = Some(converter);
    }

    pub fn take_kanji_converter(&mut self) -> Option<KanaKanjiConverter> {
        self.kanji.take()
    }

    pub fn hiragana_text(&self) -> &str {
        &self.hiragana_buf
    }

    pub fn push_char(&mut self, c: char) -> PreeditState {
        self.pending_romaji_buf.push(c);
        match self.romaji.push(c) {
            ConversionEvent::Converted(hiragana) => {
                debug!("romaji -> hiragana: {:?}", hiragana);
                self.hiragana_buf.push_str(&hiragana);
                self.pending_romaji_buf.clear();
            }
            ConversionEvent::Buffered => {}
            ConversionEvent::PassThrough(ch) => {
                self.hiragana_buf.push(ch);
                self.pending_romaji_buf.clear();
            }
        }
        self.current_preedit()
    }

    /// 末尾の未確定 "n" を「ん」として確定する（Convert / CommitRaw 前に呼ぶ）
    pub fn flush_pending_n(&mut self) -> bool {
        if self.pending_romaji_buf == "n" {
            self.pending_romaji_buf.clear();
            self.hiragana_buf.push('ん');
            self.romaji = RomajiConverter::new();
            true
        } else {
            false
        }
    }

    /// プリエディット文字列を強制置換する（F6〜F10 の文字種変換用）
    pub fn force_preedit(&mut self, text: String) {
        self.hiragana_buf = text;
        self.pending_romaji_buf.clear();
        self.romaji = RomajiConverter::new();
    }

    pub fn backspace(&mut self) -> bool {
        use romaji::BackspaceResult;
        match self.romaji.backspace() {
            BackspaceResult::RemovedBuffer(_) => {
                self.pending_romaji_buf.pop();
                true
            }
            BackspaceResult::RemovedOutput(_) => {
                self.hiragana_buf.pop();
                true
            }
            BackspaceResult::Empty => {
                if self.hiragana_buf.is_empty() {
                    false
                } else {
                    self.hiragana_buf.pop();
                    true
                }
            }
        }
    }

    pub fn convert(&self, num_candidates: usize) -> Result<Vec<String>, EngineError> {
        if self.hiragana_buf.is_empty() {
            return Ok(vec![]);
        }
        let kanji = self.kanji.as_ref().ok_or(EngineError::ModelNotInitialized)?;
        kanji
            .convert(&self.hiragana_buf, &self.committed, num_candidates)
            .map_err(|e| EngineError::ConversionFailed(e.to_string()))
    }

    pub fn convert_default(&self) -> Result<Vec<String>, EngineError> {
        self.convert(self.config.num_candidates)
    }

    pub fn commit(&mut self, text: &str) {
        debug!("確定: {:?}", text);
        self.committed.push_str(text);
        if self.committed.chars().count() > 200 {
            let start = self
                .committed
                .char_indices()
                .rev()
                .nth(199)
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.committed = self.committed[start..].to_string();
        }
        self.hiragana_buf.clear();
        self.romaji = RomajiConverter::new();
    }

    pub fn commit_as_hiragana(&mut self) {
        let text = self.hiragana_buf.clone();
        if !text.is_empty() { self.commit(&text); }
    }

    pub fn current_preedit(&self) -> PreeditState {
        PreeditState {
            hiragana: self.hiragana_buf.clone(),
            pending_romaji: self.pending_romaji_buf.clone(),
        }
    }

    pub fn preedit_is_empty(&self) -> bool { self.hiragana_buf.is_empty() }
    pub fn get_config(&self) -> &EngineConfig { &self.config }
    pub fn committed_text(&self) -> &str { &self.committed }
    pub fn is_kanji_ready(&self) -> bool { self.kanji.is_some() }

    pub fn set_dict_store(&mut self, store: DictStore) {
        info!("DictStore セット: SKK={} entries, user={} entries",
            store.skk_entry_count(), store.user_entry_count());
        self.dict_store = Some(store);
    }

    /// 確定した候補をユーザー辞書に学習して保存する
    pub fn learn(&mut self, reading: &str, surface: &str) {
        use rakukan_dict::user_dict::UserDict;
        let Some(path) = rakukan_dict::user_dict_path() else { return };
        let mut ud = UserDict::load(&path).unwrap_or_default();
        ud.add(reading, surface);
        if let Err(e) = ud.save(&path) {
            tracing::warn!("user_dict save failed: {e}");
        } else {
            tracing::info!("learned: {:?} -> {:?}", reading, surface);
        }
    }

    pub fn is_dict_ready(&self) -> bool {
        self.dict_store.as_ref().map(|d| d.is_skk_loaded()).unwrap_or(false)
    }

    pub fn merge_candidates(&self, llm_candidates: Vec<String>, limit: usize) -> Vec<String> {
        let hiragana = &self.hiragana_buf;
        let dict_cands: Vec<String> = self.dict_store
            .as_ref()
            .map(|d| d.lookup(hiragana, limit).candidates)
            .unwrap_or_default();
        let mut merged = dict_cands;
        for c in llm_candidates {
            if !merged.contains(&c) {
                merged.push(c);
            }
            if merged.len() >= limit {
                break;
            }
        }
        if merged.is_empty() {
            vec![hiragana.clone()]
        } else {
            merged
        }
    }

    pub fn backend_label(&self) -> String {
        match &self.kanji {
            Some(conv) => conv.model_display_name().to_string(),
            None       => "初期化中...".to_string(),
        }
    }

    // ─── Background 変換 API ──────────────────────────────────────────────────
    // conv_cache が engine 内部に移動したことで、TSF 側は converter を直接触らない。

    /// バックグラウンド変換を起動する。
    /// is_kanji_ready() == true の場合にのみ converter をキャッシュに渡す。
    /// False: kanji 未準備 or ひらがなが空。
    pub fn bg_start(&mut self, n_cands: usize) -> bool {
        // is_kanji_ready() チェックの前に Done 状態の converter を回収する。
        // キー不一致で take_ready が None を返した場合、converter は Done に戻るが
        // engine.kanji=None のまま → is_kanji_ready()=false → bg_start が永遠にスキップ
        // されてしまう。回収を先に行うことでこの問題を解消する。
        if let Some(old) = conv_cache::try_reclaim_done() {
            tracing::trace!("bg_start: reclaimed converter from Done state");
            self.kanji = Some(old);
        }

        let hiragana  = self.hiragana_buf.clone();
        let committed = self.committed.clone();
        if hiragana.is_empty() { return false; }
        if !self.is_kanji_ready() { return false; }

        if let Some(conv) = self.kanji.take() {
            match conv_cache::start(hiragana, committed, conv, n_cands) {
                Some(returned) => { self.kanji = Some(returned); false }
                None           => true,
            }
        } else {
            false
        }
    }

    /// BG 変換の状態文字列（診断用）
    pub fn bg_status(&self) -> &'static str {
        conv_cache::status()
    }

    /// key が一致する BG 変換結果を取得し、converter を engine に戻す。
    /// None = まだ完了していない / キー不一致
    pub fn bg_take_candidates(&mut self, key: &str) -> Option<Vec<String>> {
        let (conv, cands) = conv_cache::take_ready(key)?;
        self.kanji = Some(conv);
        Some(cands)
    }

    /// Done 状態の converter を engine に戻す（commit/cancel 時に呼ぶ）
    pub fn bg_reclaim(&mut self) {
        if let Some(conv) = conv_cache::reclaim_nonblocking() {
            self.kanji = Some(conv);
        }
    }

    pub fn reset_preedit(&mut self) {
        self.hiragana_buf.clear();
        self.romaji = RomajiConverter::new();
        self.pending_romaji_buf.clear();
    }

    pub fn reset_all(&mut self) {
        self.hiragana_buf.clear();
        self.committed.clear();
        self.romaji = RomajiConverter::new();
        self.pending_romaji_buf.clear();
    }

    pub fn available_models() -> Vec<ModelInfo> {
        let reg = registry();
        let mut models: Vec<ModelInfo> = reg
            .models
            .values()
            .flat_map(|family| {
                family.variants.values().map(|v| ModelInfo {
                    id: v.id.clone(),
                    display_name: v.display_name.clone(),
                    is_default: v.id == reg.default_model,
                })
            })
            .collect();
        models.sort_by(|a, b| a.id.cmp(&b.id));
        models
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: String,
    pub is_default: bool,
}
