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

/// 直前の文字種から判定したコンテキスト幅。
/// ASCII 記号の全角・半角変換に使用する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextWidth {
    /// ひらがな・全角カタカナ（長音符 → ー、記号 → 全角）
    Kana,
    /// 半角カタカナ（長音符 → ｰ、記号 → 半角）
    HankakuKana,
    /// 全角英数・全角記号（記号 → 全角）
    Full,
    /// 半角英数・半角記号・空（記号 → 半角）
    Half,
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
        // ASCII 記号のコンテキスト判定
        // ローマ字ルールに登録されていない ASCII 記号（U+0021–U+007E のうち英数字以外）を
        // 直前の文字種に応じて全角・半角・長音符に変換する。
        // 未確定ローマ字がある場合は英数字コンテキストとして半角のまま渡す。
        let is_ascii_symbol = (c as u32) < 0x80
            && !c.is_ascii_alphanumeric()
            && c != ' ';
        if is_ascii_symbol && self.pending_romaji_buf.is_empty() {
            if let Some(out) = Self::symbol_for_context(&self.hiragana_buf, c) {
                self.hiragana_buf.push(out);
                debug!("symbol context {:?} → {:?}", c, out);
                return self.current_preedit();
            }
            // None の場合はローマ字ルールに任せる（句読点変換等）
        }

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

    /// ASCII 記号のコンテキスト変換。
    ///
    /// 直前の文字種を見て、入力された ASCII 記号を適切な文字に変換する。
    /// `None` を返した場合はローマ字ルールに委ねる（`/`→`・` 等）。
    ///
    /// | 直前の文字種               | `-`        | `,`   | `.`   | その他 ASCII 記号 |
    /// |--------------------------|------------|-------|-------|-----------------|
    /// | ひらがな・全角カタカナ      | `ー`（長音）| `、`  | `。`  | 全角記号          |
    /// | 半角カタカナ               | `ｰ`（長音）| `、`  | `。`  | 半角のまま（None）|
    /// | 全角英数・全角記号          | `－`       | `，`  | `．`  | 全角記号          |
    /// | 全角句読点（、。，．）      | `－`       | `、`  | `。`  | 全角記号          |
    /// | 半角英数・半角記号・空      | `-`        | `,`   | `.`   | None（ローマ字）  |
    pub(crate) fn symbol_for_context(s: &str, c: char) -> Option<char> {
        let prev = s.chars().last();
        let width = Self::context_width(prev);

        if c == '-' {
            return Some(match width {
                ContextWidth::Kana        => 'ー',
                ContextWidth::HankakuKana => 'ｰ',
                ContextWidth::Full        => '－',
                ContextWidth::Half        => '-',
            });
        }

        if c == ',' {
            return Some(match width {
                ContextWidth::Kana | ContextWidth::HankakuKana => '、',
                ContextWidth::Full  => '，',
                ContextWidth::Half  => ',',
            });
        }

        if c == '.' {
            return Some(match width {
                ContextWidth::Kana | ContextWidth::HankakuKana => '。',
                ContextWidth::Full  => '．',
                ContextWidth::Half  => '.',
            });
        }

        // ¥（U+00A5）は全角コンテキストで ￥（U+FFE5）に変換
        if c == '¥' {
            return match width {
                ContextWidth::Kana | ContextWidth::Full => Some('￥'),
                _ => None,
            };
        }

        match width {
            ContextWidth::Kana | ContextWidth::Full => {
                let n = c as u32;
                if (0x21..=0x7E).contains(&n) {
                    Some(char::from_u32(n - 0x21 + 0xFF01).unwrap_or(c))
                } else {
                    None
                }
            }
            ContextWidth::HankakuKana | ContextWidth::Half => {
                None
            }
        }
    }

    /// 直前の文字からコンテキスト幅を判定する。
    fn context_width(prev: Option<char>) -> ContextWidth {
        match prev {
            None => ContextWidth::Half,
            Some(c) => {
                let n = c as u32;
                // ひらがな・全角カタカナ・全角長音符
                if (0x3041..=0x3096).contains(&n)
                    || (0x30A1..=0x30F6).contains(&n)
                    || c == 'ー'
                    || matches!(c, '、' | '。')  // 和文句読点の直後はかなコンテキスト
                {
                    return ContextWidth::Kana;
                }
                // 半角カタカナ・半角長音符
                if (0xFF65..=0xFF9F).contains(&n) || c == 'ｰ' {
                    return ContextWidth::HankakuKana;
                }
                // 全角ASCII範囲（全角英数記号）U+FF01–U+FF5E
                if (0xFF01..=0xFF5E).contains(&n) {
                    return ContextWidth::Full;
                }
                // 全角括弧・記号（CJK記号）
                if matches!(c,
                    '）'|'］'|'】'|'〕'|'〉'|'》'|'」'|'〟'|
                    '（'|'「'|'【'|'〔'|'〈'|'《'|'『'|'』'|'〝'|
                    '・'|'…'|'—'|'～'
                ) {
                    return ContextWidth::Full;
                }
                ContextWidth::Half
            }
        }
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

    /// ローマ字変換を経由せず hiragana_buf に直接1文字追加する。
    /// テンキー記号など、かなルールに登録されている文字をそのまま入力する場合に使用する。
    pub fn push_raw(&mut self, c: char) {
        // 未確定ローマ字（pending_romaji_buf）がある場合はそのまま残す
        self.hiragana_buf.push(c);
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

#[cfg(test)]
mod context_trim_tests {
    use super::last_n_sentences_start;

    #[test]
    fn empty_text() {
        assert_eq!(last_n_sentences_start("", 2), 0);
    }

    #[test]
    fn no_boundary() {
        let text = "文境界のないテキスト";
        assert_eq!(last_n_sentences_start(text, 2), 0);
    }

    #[test]
    fn single_boundary_want_two() {
        let text = "最初の文。二番目の文";
        // 境界が1個しかない → 先頭を返す
        assert_eq!(last_n_sentences_start(text, 2), 0);
    }

    #[test]
    fn two_boundaries_want_two() {
        let text = "最初の文。二番目の文。三番目の文";
        // 境界が2個 [「二番目」先頭, 「三番目」先頭]、n=2 → 先頭から2個目の境界 = 「二番目」先頭
        let start = last_n_sentences_start(text, 2);
        assert_eq!(&text[start..], "二番目の文。三番目の文");
    }

    #[test]
    fn multiple_punctuation() {
        let text = "A！？B。C";
        // 境界2個 [「B」先頭, 「C」先頭]、n=2 → 「B」先頭
        let start = last_n_sentences_start(text, 2);
        assert_eq!(&text[start..], "B。C");
    }

    #[test]
    fn linebreak_as_boundary() {
        let text = "一行目\n二行目\n三行目";
        // 境界2個 [「二行目」先頭, 「三行目」先頭]、n=2 → 「二行目」先頭
        let start = last_n_sentences_start(text, 2);
        assert_eq!(&text[start..], "二行目\n三行目");
    }

    #[test]
    fn want_one_sentence() {
        let text = "文A。文B。文C";
        // n=1 → 最後の境界 = 「文C」先頭
        let start = last_n_sentences_start(text, 1);
        assert_eq!(&text[start..], "文C");
    }
}

#[cfg(test)]
mod minus_context_tests {
    use super::RakunEngine;

    fn sym(s: &str, c: char) -> Option<char> {
        RakunEngine::symbol_for_context(s, c)
    }

    // ─── '-' のテスト ────────────────────────────────────────────────────
    #[test]
    fn minus_after_hiragana() {
        assert_eq!(sym("あ", '-'), Some('ー'));
        assert_eq!(sym("かいぎ", '-'), Some('ー'));
    }

    #[test]
    fn minus_after_zenkaku_katakana() {
        assert_eq!(sym("ア", '-'), Some('ー'));
        assert_eq!(sym("ラー", '-'), Some('ー'));
    }

    #[test]
    fn minus_after_hankaku_katakana() {
        assert_eq!(sym("ｶｲｷﾞ", '-'), Some('ｰ'));
    }

    #[test]
    fn minus_after_zenkaku_latin() {
        assert_eq!(sym("ＭＳ", '-'), Some('－'));
        assert_eq!(sym("１２３", '-'), Some('－'));
    }

    #[test]
    fn minus_after_ascii() {
        assert_eq!(sym("MS", '-'), Some('-'));
        assert_eq!(sym("090", '-'), Some('-'));
    }

    #[test]
    fn minus_repeat() {
        assert_eq!(sym("-", '-'), Some('-'));    // --- → ---
        assert_eq!(sym("－", '-'), Some('－'));  // 全角連続
    }

    #[test]
    fn minus_at_start() {
        assert_eq!(sym("", '-'), Some('-'));
    }

    // ─── その他記号のテスト ───────────────────────────────────────────────
    #[test]
    fn symbol_after_hiragana() {
        assert_eq!(sym("あ", '='), Some('＝'));
        assert_eq!(sym("あ", '@'), Some('＠'));
        assert_eq!(sym("あ", '('), Some('（'));
    }

    #[test]
    fn symbol_after_zenkaku() {
        assert_eq!(sym("ＭＳ", '='), Some('＝'));
        assert_eq!(sym("ａｂｃ", '('), Some('（'));
    }

    #[test]
    fn symbol_after_ascii() {
        // 半角コンテキスト → ローマ字ルールに任せる（None）
        assert_eq!(sym("MS", '='), None);
        assert_eq!(sym("MS", '/'), None);
    }

    #[test]
    fn symbol_at_start() {
        assert_eq!(sym("", '='), None);
        assert_eq!(sym("", '/'), None);
    }

    // ─── ',' のテスト ────────────────────────────────────────────────────
    #[test]
    fn comma_after_hiragana() {
        assert_eq!(sym("あ", ','), Some('、'));
        assert_eq!(sym("かいぎ", ','), Some('、'));
    }

    #[test]
    fn comma_after_katakana() {
        assert_eq!(sym("ア", ','), Some('、'));
        assert_eq!(sym("ｶｲｷﾞ", ','), Some('、')); // 半角カタカナも読点
    }

    #[test]
    fn comma_after_zenkaku() {
        assert_eq!(sym("ＡＢＣ", ','), Some('，'));
        assert_eq!(sym("１２３", ','), Some('，'));
    }

    #[test]
    fn comma_after_kuten() {
        assert_eq!(sym("あ。", ','), Some('、')); // 句点の直後も読点
        assert_eq!(sym("あ、", ','), Some('、')); // 読点の直後も読点
    }

    #[test]
    fn comma_after_ascii() {
        assert_eq!(sym("abc", ','), Some(','));
        assert_eq!(sym("123", ','), Some(','));
        assert_eq!(sym("", ','), Some(','));
    }

    // ─── '.' のテスト ────────────────────────────────────────────────────
    #[test]
    fn period_after_hiragana() {
        assert_eq!(sym("あ", '.'), Some('。'));
    }

    #[test]
    fn period_after_zenkaku() {
        assert_eq!(sym("ＡＢＣ", '.'), Some('．'));
        assert_eq!(sym("１２３", '.'), Some('．'));
    }

    #[test]
    fn period_after_ascii() {
        assert_eq!(sym("abc", '.'), Some('.'));
        assert_eq!(sym("3", '.'), Some('.')); // 小数点 3.14
        assert_eq!(sym("", '.'), Some('.'));
    }
}
