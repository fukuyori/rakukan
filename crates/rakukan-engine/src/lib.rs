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

// ── コンテキストトリミング ────────────────────────────────────────────────────

/// テキストから末尾 `n` 文の開始バイト位置を返す。
///
/// fast-bunkai の BasicRule / LinebreakAnnotator 相当の純 Rust 実装。
/// 文境界は `。！？!?.．\n` の直後とみなす。
/// 文境界が `n` 個未満の場合はテキスト全体の先頭（0）を返す。
fn last_n_sentences_start(text: &str, n: usize) -> usize {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let len = chars.len();
    let mut boundaries: Vec<usize> = Vec::new();
    let mut i = 0;
    while i < len {
        let ch = chars[i].1;
        if matches!(ch, '\u{3002}' | '\u{FF01}' | '\u{FF1F}' | '!' | '?' | '.' | '\u{FF0E}' | '\n') {
            // 句読点・空白が連続する場合はまとめてスキップ
            let mut j = i + 1;
            while j < len && matches!(chars[j].1,
                '\u{3002}' | '\u{FF01}' | '\u{FF1F}' | '!' | '?' | '.' | '\u{FF0E}'
                | ' ' | '\u{3000}' | '\n') {
                j += 1;
            }
            if j < len {
                boundaries.push(chars[j].0);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    // 末尾から n 個目の境界を返す。境界が足りなければ先頭。
    if boundaries.len() >= n {
        boundaries[boundaries.len() - n]
    } else {
        0
    }
}



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
    /// ローマ字入力ログ。`RomajiConverter::Converted` 単位で1エントリとして積む。
    /// 末尾エントリは pending_romaji_buf に対応する未確定分（確定時に上書き）。
    /// F9/F10 でかな→ローマ字復元に使用する。
    romaji_input_log: Vec<String>,
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
            romaji_input_log: Vec::new(),
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
        // ─── ASCII 数字・記号の全角変換 ───────────────────────────────────────
        // 未確定ローマ字がない場合のみ実施（pending 中は英数字コンテキスト）。
        // ローマ字ルールに登録されている文字（英字）は通常のローマ字変換に任せる。
        if self.pending_romaji_buf.is_empty() {
            // 数字 0–9 → 全角数字 ０–９
            if c.is_ascii_digit() {
                let fw = char::from_u32(c as u32 - 0x30 + 0xFF10).unwrap_or(c);
                self.hiragana_buf.push(fw);
                self.romaji_input_log.push(c.to_string());
                debug!("digit {:?} → {:?}", c, fw);
                return self.current_preedit();
            }
            // ASCII 記号 → 固定マッピングまたは全角変換
            let is_ascii_symbol = (c as u32) < 0x80 && !c.is_ascii_alphanumeric() && c != ' ';
            if is_ascii_symbol {
                let out = Self::symbol_fixed(c, &self.hiragana_buf);
                if let Some(out) = out {
                    self.hiragana_buf.push(out);
                    self.romaji_input_log.push(c.to_string());
                    debug!("symbol {:?} → {:?}", c, out);
                    return self.current_preedit();
                }
                // None はローマ字ルールに任せる（`/`→`・` 等）
            }
        }

        self.pending_romaji_buf.push(c);
        match self.romaji.push(c) {
            ConversionEvent::Converted(hiragana) => {
                debug!("romaji -> hiragana: {:?}", hiragana);
                self.hiragana_buf.push_str(&hiragana);
                // pending_romaji_buf 全体が今の確定分
                let entry = std::mem::take(&mut self.pending_romaji_buf);
                self.romaji_input_log.push(entry);
            }
            ConversionEvent::Buffered => {
                // pending_romaji_buf に積み上がっているだけ。
                // log への記録は Converted / PassThrough で確定時にまとめて行う。
            }
            ConversionEvent::PassThrough(ch) => {
                self.hiragana_buf.push(ch);
                let entry = std::mem::take(&mut self.pending_romaji_buf);
                self.romaji_input_log.push(entry);
            }
        }
        self.current_preedit()
    }

    /// ASCII 記号の固定マッピング変換。
    ///
    /// コンテキスト判定を廃止し、以下のルールで固定変換する：
    /// - `,` → `、`  `.` → `。`  `[` → `「`  `]` → `」`  `\` → `￥`
    /// - `-` のみ直前文字種で `ー`（かな後）/ `－`（その他）に分岐
    /// - その他 ASCII 印字可能記号 → 全角記号
    /// - `None` を返すとローマ字ルールに委ねる
    pub(crate) fn symbol_fixed(c: char, hiragana_buf: &str) -> Option<char> {
        match c {
            ',' => Some('、'),
            '.' => Some('。'),
            '[' => Some('「'),
            ']' => Some('」'),
            '\x5C' | '\u{A5}' => Some('\u{FFE5}'),
            '-' => {
                // 直前がひらがな・カタカナ → 長音符、それ以外 → 全角ハイフン
                let prev = hiragana_buf.chars().last();
                let is_kana = prev.map(|ch| {
                    let n = ch as u32;
                    (0x3041..=0x3096).contains(&n)   // ひらがな
                    || (0x30A1..=0x30FC).contains(&n) // 全角カタカナ・長音符
                    || (0xFF65..=0xFF9F).contains(&n) // 半角カタカナ
                    || ch == 'ｰ'
                }).unwrap_or(false);
                Some(if is_kana { 'ー' } else { '－' })
            }
            _ => {
                // その他 ASCII 印字可能文字 → 全角
                let n = c as u32;
                if (0x21..=0x7E).contains(&n) {
                    Some(char::from_u32(n - 0x21 + 0xFF01).unwrap_or(c))
                } else {
                    None
                }
            }
        }
    }



    /// 末尾の未確定 "n" を「ん」として確定する（Convert / CommitRaw 前に呼ぶ）
    pub fn flush_pending_n(&mut self) -> bool {
        if self.pending_romaji_buf == "n" {
            self.hiragana_buf.push('ん');
            let entry = std::mem::take(&mut self.pending_romaji_buf);
            self.romaji_input_log.push(entry);
            self.romaji = RomajiConverter::new();
            true
        } else {
            false
        }
    }

    /// プリエディット文字列を強制置換する（F6〜F10 の文字種変換用）
    /// romaji_input_log は保持する（F9/F10 サイクル中に再度ローマ字に戻せるよう）
    pub fn force_preedit(&mut self, text: String) {
        self.hiragana_buf = text;
        self.pending_romaji_buf.clear();
        self.romaji = RomajiConverter::new();
    }

    /// ローマ字変換を経由せず hiragana_buf に直接1文字追加する。
    /// テンキー記号など、かなルールに登録されている文字をそのまま入力する場合に使用する。
    pub fn push_raw(&mut self, c: char) {
        self.hiragana_buf.push(c);
        self.romaji_input_log.push(c.to_string());
    }

    pub fn backspace(&mut self) -> bool {
        use romaji::BackspaceResult;
        match self.romaji.backspace() {
            BackspaceResult::RemovedBuffer(_) => {
                self.pending_romaji_buf.pop();
                // pending_romaji_buf はまだ未確定 → romaji_input_log には記録されていない
                // log 操作は不要
                true
            }
            BackspaceResult::RemovedOutput(_) => {
                self.hiragana_buf.pop();
                // 確定済みのひらがな1文字分 → log エントリを1つ pop
                self.romaji_input_log.pop();
                true
            }
            BackspaceResult::Empty => {
                if self.hiragana_buf.is_empty() {
                    false
                } else {
                    self.hiragana_buf.pop();
                    self.romaji_input_log.pop();
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
            // 文境界でトリミング: 直近 2 文を残す。
            // 200 文字単純切りより自然な文脈を LLM に渡せる。
            let start = last_n_sentences_start(&self.committed, 2);
            if start > 0 {
                self.committed = self.committed[start..].to_string();
            } else {
                // 文境界が見つからない場合は従来通り直近 200 文字
                let fallback = self
                    .committed
                    .char_indices()
                    .rev()
                    .nth(199)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                self.committed = self.committed[fallback..].to_string();
            }
        }
        self.hiragana_buf.clear();
        self.romaji_input_log.clear();
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

    pub fn preedit_is_empty(&self) -> bool { self.hiragana_buf.is_empty() && self.pending_romaji_buf.is_empty() }

    /// ローマ字入力ログを結合した文字列を返す（F9/F10 のローマ字復元用）
    pub fn romaji_log_str(&self) -> String {
        self.romaji_input_log.concat()
    }

    /// romaji_input_log からひらがなを復元する（F6/F7/F8 でかなに戻す用）
    /// F9/F10 で force_preedit した後でも log は保持されているため復元可能。
    pub fn hiragana_from_romaji_log(&self) -> String {
        let romaji = self.romaji_input_log.concat();
        if romaji.is_empty() { return String::new(); }
        let mut conv = RomajiConverter::new();
        let mut result = String::new();
        for c in romaji.chars() {
            match conv.push(c) {
                crate::romaji::ConversionEvent::Converted(h) => result.push_str(&h),
                crate::romaji::ConversionEvent::PassThrough(ch) => result.push(ch),
                crate::romaji::ConversionEvent::Buffered => {}
            }
        }
        // pending を flush
        result.push_str(&conv.flush());
        result
    }
    pub fn get_config(&self) -> &EngineConfig { &self.config }
    pub fn committed_text(&self) -> &str { &self.committed }
    pub fn is_kanji_ready(&self) -> bool { self.kanji.is_some() }

    pub fn set_dict_store(&mut self, store: DictStore) {
        info!("DictStore セット: SKK={} entries, user={} entries",
            store.skk_entry_count(), store.user_entry_count());
        self.dict_store = Some(store);
    }

    /// 確定した候補をユーザー辞書に学習して保存する
    /// 学習語を DictStore に即時反映してファイルにも保存する。
    pub fn learn(&mut self, reading: &str, surface: &str) {
        if let Some(store) = &self.dict_store {
            store.learn(reading, surface);
        } else {
            tracing::warn!("learn: dict_store not initialized");
        }
    }

    pub fn is_dict_ready(&self) -> bool {
        self.dict_store.as_ref().map(|d| d.is_skk_loaded()).unwrap_or(false)
    }

    pub fn merge_candidates(&self, llm_candidates: Vec<String>, limit: usize) -> Vec<String> {
        let hiragana = &self.hiragana_buf;

        // 優先順位: ユーザー辞書 → LLM → mozc/skk
        let user_cands: Vec<String> = self.dict_store
            .as_ref()
            .map(|d| d.lookup_user(hiragana))
            .unwrap_or_default();

        let dict_cands: Vec<String> = self.dict_store
            .as_ref()
            .map(|d| d.lookup_dict(hiragana, limit))
            .unwrap_or_default();

        let mut merged: Vec<String> = Vec::new();

        // 1. ユーザー辞書候補（最優先）
        for c in user_cands {
            if merged.len() >= limit { break; }
            if !merged.contains(&c) { merged.push(c); }
        }

        // 2. LLM候補（文脈考慮）
        for c in llm_candidates {
            if merged.len() >= limit { break; }
            if !merged.contains(&c) { merged.push(c); }
        }

        // 3. mozc/skk候補（文脈なし辞書）
        for c in dict_cands {
            if merged.len() >= limit { break; }
            if !merged.contains(&c) { merged.push(c); }
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
        self.romaji_input_log.clear();
    }

    pub fn reset_all(&mut self) {
        self.hiragana_buf.clear();
        self.committed.clear();
        self.romaji = RomajiConverter::new();
        self.pending_romaji_buf.clear();
        self.romaji_input_log.clear();
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

#[cfg(test)]
mod context_trim_tests {
    use super::last_n_sentences_start;

    #[test]
    fn empty_text() {
        assert_eq!(last_n_sentences_start("", 2), 0);
    }

    #[test]
    fn no_boundary() {
        let text = "\u{6587}\u{5883}\u{754C}\u{306E}\u{306A}\u{3044}\u{30C6}\u{30AD}\u{30B9}\u{30C8}";
        assert_eq!(last_n_sentences_start(text, 2), 0);
    }

    #[test]
    fn single_boundary_want_two() {
        let text = "\u{6700}\u{521D}\u{306E}\u{6587}\u{3002}\u{4E8C}\u{756A}\u{76EE}\u{306E}\u{6587}";
        // \u{5883}\u{754C}\u{304C}1\u{500B}\u{3057}\u{304B}\u{306A}\u{3044} \u{2192} \u{5148}\u{982D}\u{3092}\u{8FD4}\u{3059}
        assert_eq!(last_n_sentences_start(text, 2), 0);
    }

    #[test]
    fn two_boundaries_want_two() {
        let text = "\u{6700}\u{521D}\u{306E}\u{6587}\u{3002}\u{4E8C}\u{756A}\u{76EE}\u{306E}\u{6587}\u{3002}\u{4E09}\u{756A}\u{76EE}\u{306E}\u{6587}";
        // \u{5883}\u{754C}\u{304C}2\u{500B} [\u{300C}\u{4E8C}\u{756A}\u{76EE}\u{300D}\u{5148}\u{982D}, \u{300C}\u{4E09}\u{756A}\u{76EE}\u{300D}\u{5148}\u{982D}]\u{3001}n=2 \u{2192} \u{5148}\u{982D}\u{304B}\u{3089}2\u{500B}\u{76EE}\u{306E}\u{5883}\u{754C} = \u{300C}\u{4E8C}\u{756A}\u{76EE}\u{300D}\u{5148}\u{982D}
        let start = last_n_sentences_start(text, 2);
        assert_eq!(&text[start..], "\u{4E8C}\u{756A}\u{76EE}\u{306E}\u{6587}\u{3002}\u{4E09}\u{756A}\u{76EE}\u{306E}\u{6587}");
    }

    #[test]
    fn multiple_punctuation() {
        let text = "A\u{FF01}\u{FF1F}B\u{3002}C";
        // \u{5883}\u{754C}2\u{500B} [\u{300C}B\u{300D}\u{5148}\u{982D}, \u{300C}C\u{300D}\u{5148}\u{982D}]\u{3001}n=2 \u{2192} \u{300C}B\u{300D}\u{5148}\u{982D}
        let start = last_n_sentences_start(text, 2);
        assert_eq!(&text[start..], "B\u{3002}C");
    }

    #[test]
    fn linebreak_as_boundary() {
        let text = "\u{4E00}\u{884C}\u{76EE}\n\u{4E8C}\u{884C}\u{76EE}\n\u{4E09}\u{884C}\u{76EE}";
        // \u{5883}\u{754C}2\u{500B} [\u{300C}\u{4E8C}\u{884C}\u{76EE}\u{300D}\u{5148}\u{982D}, \u{300C}\u{4E09}\u{884C}\u{76EE}\u{300D}\u{5148}\u{982D}]\u{3001}n=2 \u{2192} \u{300C}\u{4E8C}\u{884C}\u{76EE}\u{300D}\u{5148}\u{982D}
        let start = last_n_sentences_start(text, 2);
        assert_eq!(&text[start..], "\u{4E8C}\u{884C}\u{76EE}\n\u{4E09}\u{884C}\u{76EE}");
    }

    #[test]
    fn want_one_sentence() {
        let text = "\u{6587}A\u{3002}\u{6587}B\u{3002}\u{6587}C";
        // n=1 \u{2192} \u{6700}\u{5F8C}\u{306E}\u{5883}\u{754C} = \u{300C}\u{6587}C\u{300D}\u{5148}\u{982D}
        let start = last_n_sentences_start(text, 1);
        assert_eq!(&text[start..], "\u{6587}C");
    }
}

#[cfg(test)]
mod symbol_fixed_tests {
    use super::RakunEngine;

    fn sym(buf: &str, c: char) -> Option<char> {
        RakunEngine::symbol_fixed(c, buf)
    }

    #[test]
    fn comma_always_kuten() {
        assert_eq!(sym("", ','), Some('\u{3001}'));
        assert_eq!(sym("\u{3042}", ','), Some('\u{3001}'));
        assert_eq!(sym("abc", ','), Some('\u{3001}'));
        assert_eq!(sym("\u{FF21}\u{FF22}\u{FF23}", ','), Some('\u{3001}'));
    }

    #[test]
    fn period_always_maru() {
        assert_eq!(sym("", '.'), Some('\u{3002}'));
        assert_eq!(sym("\u{3042}", '.'), Some('\u{3002}'));
        assert_eq!(sym("abc", '.'), Some('\u{3002}'));
    }

    #[test]
    fn bracket_open() {
        assert_eq!(sym("", '['), Some('\u{300C}'));
        assert_eq!(sym("\u{3042}", '['), Some('\u{300C}'));
    }

    #[test]
    fn bracket_close() {
        assert_eq!(sym("", ']'), Some('\u{300D}'));
    }

    #[test]
    fn backslash_to_yen() {
        assert_eq!(sym("", '\x5C'), Some('\u{FFE5}'));
        assert_eq!(sym("\u{3042}", '\x5C'), Some('\u{FFE5}'));
    }

    #[test]
    fn minus_after_kana_is_choon() {
        assert_eq!(sym("\u{3042}", '-'), Some('\u{30FC}'));
        assert_eq!(sym("\u{30A2}", '-'), Some('\u{30FC}'));
        assert_eq!(sym("\u{FF71}", '-'), Some('\u{30FC}'));
    }

    #[test]
    fn minus_after_other_is_zenkaku_hyphen() {
        assert_eq!(sym("", '-'), Some('\u{FF0D}'));
        assert_eq!(sym("abc", '-'), Some('\u{FF0D}'));
        assert_eq!(sym("\u{FF21}\u{FF22}\u{FF23}", '-'), Some('\u{FF0D}'));
    }

    #[test]
    fn other_symbols_fullwidth() {
        assert_eq!(sym("", '='), Some('\u{FF1D}'));
        assert_eq!(sym("", '@'), Some('\u{FF20}'));
        assert_eq!(sym("", '('), Some('\u{FF08}'));
        assert_eq!(sym("", ')'), Some('\u{FF09}'));
        assert_eq!(sym("abc", '='), Some('\u{FF1D}'));
    }
}
