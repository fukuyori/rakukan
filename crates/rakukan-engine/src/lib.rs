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

pub use kana::{
    hiragana_to_halfwidth_katakana, hiragana_to_katakana, katakana_to_hiragana, normalize_nfkc,
};
pub use kanji::{Backend, KanaKanjiConverter};
pub use romaji::{BackspaceResult, ConversionEvent, RomajiConverter};

// ── rakukan 独自モジュール ────────────────────────────────────────────────────
pub mod backend;
pub mod conv_cache;
pub mod dict;
pub mod digits;
pub mod ffi;
pub mod segments;
pub use backend::{BackendSelection, GpuInfo, select_backend};
// Backend は kanji::Backend と名前が被るため、rakukan の Backend は別名でエクスポート
pub use backend::Backend as RakunBackend;

pub use segments::{Candidate, CandidateSource, Segment, Segments};

pub use rakukan_dict::mozc_dict::MozcDict;
pub use rakukan_dict::{DictStore, find_mozc_dict, user_dict_path};

use kanji::{Backend as KarukanBackend, registry};
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
        if matches!(
            ch,
            '\u{3002}' | '\u{FF01}' | '\u{FF1F}' | '!' | '?' | '.' | '\u{FF0E}' | '\n'
        ) {
            // 句読点・空白が連続する場合はまとめてスキップ
            let mut j = i + 1;
            while j < len
                && matches!(
                    chars[j].1,
                    '\u{3002}'
                        | '\u{FF01}'
                        | '\u{FF1F}'
                        | '!'
                        | '?'
                        | '.'
                        | '\u{FF0E}'
                        | ' '
                        | '\u{3000}'
                        | '\n'
                )
            {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
    /// 数字の入力幅: "fullwidth" = 全角 (０１２), "halfwidth" = 半角 (012)
    pub digit_width: DigitWidth,
    /// 数字直後の句読点を数値区切りとして扱う。
    pub digit_separator_auto: bool,
    /// ライブ変換時の候補数（beam 幅に影響）。1 = greedy（高速）、3 = beam（高品質）
    pub live_conv_beam_size: usize,
    /// Space 変換時のビーム幅の**上限**（num_candidates と併せて min をとる）。
    /// デフォルト 30 では実質上限なし、num_candidates がそのまま beam 幅になる。
    pub convert_beam_size: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            model_variant: None,
            num_candidates: 5,
            n_threads: 0,
            n_gpu_layers: 0u32,
            main_gpu: 0,
            digit_width: DigitWidth::default(),
            digit_separator_auto: true,
            live_conv_beam_size: 3,
            convert_beam_size: 30,
        }
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

fn is_numeric_digit(c: char) -> bool {
    c.is_ascii_digit() || ('０'..='９').contains(&c)
}

fn numeric_separator_after_digit(prev: Option<char>, c: char) -> Option<char> {
    if !prev.is_some_and(is_numeric_digit) {
        return None;
    }
    match c {
        ',' | '、' => Some(','),
        '.' | '。' => Some('.'),
        _ => None,
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
        info!(
            "engine::init: loading model={} gpu_layers={} main_gpu={}",
            variant_id, config.n_gpu_layers, config.main_gpu
        );
        let backend = KarukanBackend::from_variant_id(&variant_id)
            .map_err(|e| EngineError::InitFailed(e.to_string()))?
            .with_n_gpu_layers(config.n_gpu_layers)
            .with_main_gpu(config.main_gpu);
        let conv_cfg = kanji::ConversionConfig {
            beam_size: config.convert_beam_size,
            ..Default::default()
        };
        let mut converter = KanaKanjiConverter::with_config(backend, conv_cfg)
            .map_err(|e| EngineError::InitFailed(e.to_string()))?;
        if config.n_threads > 0 {
            converter.set_n_threads(config.n_threads);
        }
        info!(
            "engine::init: model ready name={}",
            converter.model_display_name()
        );
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
        if self.config.digit_separator_auto && self.pending_romaji_buf.is_empty() {
            if let Some(separator) =
                numeric_separator_after_digit(self.hiragana_buf.chars().last(), c)
            {
                self.hiragana_buf.push(separator);
                self.romaji_input_log.push(c.to_string());
                debug!("engine::push: numeric separator {:?} → {:?}", c, separator);
                return self.current_preedit();
            }
        }

        // 数字 0–9（pending_romaji がない場合のみ）
        if self.pending_romaji_buf.is_empty() && c.is_ascii_digit() {
            let out = match self.config.digit_width {
                DigitWidth::Fullwidth => char::from_u32(c as u32 - 0x30 + 0xFF10).unwrap_or(c),
                DigitWidth::Halfwidth => c,
            };
            self.hiragana_buf.push(out);
            self.romaji_input_log.push(c.to_string());
            debug!("engine::push: digit {:?} → {:?}", c, out);
            return self.current_preedit();
        }

        // ASCII 記号の処理（pending_romaji がない場合のみ）
        // ,./[]\- はトライのルール（、。・「」￥ー等）に委ねる。
        // それ以外の印字可能 ASCII 記号（@#$%^&*()+=_:"~!? 等）は
        // 全角に変換して即確定する（以前の symbol_fixed catch-all と同等）。
        if self.pending_romaji_buf.is_empty() {
            let n = c as u32;
            let is_ascii_printable = (0x21..=0x7E).contains(&n);
            let is_trie_symbol = matches!(c, ',' | '.' | '/' | '[' | ']' | '\\' | '-');
            if is_ascii_printable && !is_trie_symbol && !c.is_ascii_alphanumeric() {
                let fw = char::from_u32(n - 0x21 + 0xFF01).unwrap_or(c);
                self.hiragana_buf.push(fw);
                self.romaji_input_log.push(c.to_string());
                debug!("engine::push: symbol {:?} → {:?}", c, fw);
                return self.current_preedit();
            }
        }

        // ,./[]\- および英字 → ローマ字ルール（trie）に委ねる
        // pending_romaji_buf と romaji.buffer は常に同じ状態を保つ。
        // ConversionEvent variant ではなく romaji.output / romaji.buffer の差分から
        // 「確定したひらがな」と「未確定として残っているローマ字」を判定する。
        // （PassThrough の連鎖で複数文字が確定するケースを正しく扱うため）
        self.pending_romaji_buf.push(c);
        let prev_output_len = self.romaji.output().len();
        let _ = self.romaji.push(c);

        let added = self.romaji.output()[prev_output_len..].to_string();
        let new_buffer_len = self.romaji.buffer().len();
        debug_assert!(new_buffer_len <= self.pending_romaji_buf.len());
        let consumed_len = self.pending_romaji_buf.len() - new_buffer_len;
        if consumed_len > 0 {
            let entry: String = self.pending_romaji_buf.drain(..consumed_len).collect();
            self.hiragana_buf.push_str(&added);
            debug!("engine::push: romaji {:?} → {:?}", entry, added);
            self.romaji_input_log.push(entry);
        }
        self.current_preedit()
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

    /// Shift+アルファベット用: hiragana_buf に全角大文字を、romaji_input_log に ASCII 大文字を記録する。
    ///
    /// F9/F10 のサイクル変換は romaji_input_log の ASCII 文字を元に動作するため、
    /// log には元の ASCII 文字（'A'–'Z'）を保持する必要がある。
    /// `c` には ASCII 大文字（'A'–'Z'）を渡すこと。
    pub fn push_fullwidth_alpha(&mut self, c: char) {
        debug_assert!(c.is_ascii_uppercase());
        let fw = char::from_u32(c as u32 - 0x41 + 0xFF21).unwrap_or(c);
        self.hiragana_buf.push(fw);
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
        let kanji = self
            .kanji
            .as_ref()
            .ok_or(EngineError::ModelNotInitialized)?;
        digits::convert_with_digit_protection(
            kanji,
            &self.hiragana_buf,
            &self.committed,
            num_candidates,
        )
        .map_err(|e| EngineError::ConversionFailed(e.to_string()))
    }

    pub fn convert_default(&self) -> Result<Vec<String>, EngineError> {
        self.convert(self.config.num_candidates)
    }

    pub fn commit(&mut self, text: &str) {
        info!("engine::commit: {:?}", text);
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
        if !text.is_empty() {
            self.commit(&text);
        }
    }

    pub fn current_preedit(&self) -> PreeditState {
        PreeditState {
            hiragana: self.hiragana_buf.clone(),
            pending_romaji: self.pending_romaji_buf.clone(),
        }
    }

    pub fn preedit_is_empty(&self) -> bool {
        self.hiragana_buf.is_empty() && self.pending_romaji_buf.is_empty()
    }

    /// ローマ字入力ログを結合した文字列を返す（F9/F10 のローマ字復元用）
    pub fn romaji_log_str(&self) -> String {
        self.romaji_input_log.concat()
    }

    /// romaji_input_log からひらがなを復元する（F6/F7/F8 でかなに戻す用）
    /// F9/F10 で force_preedit した後でも log は保持されているため復元可能。
    pub fn hiragana_from_romaji_log(&self) -> String {
        let romaji = self.romaji_input_log.concat();
        if romaji.is_empty() {
            return String::new();
        }
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
    pub fn get_config(&self) -> &EngineConfig {
        &self.config
    }
    pub fn committed_text(&self) -> &str {
        &self.committed
    }
    pub fn is_kanji_ready(&self) -> bool {
        self.kanji.is_some()
    }

    pub fn set_dict_store(&mut self, store: DictStore) {
        info!(
            "engine::dict: store set user_entries={}",
            store.user_entry_count()
        );
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
        self.dict_store.is_some()
    }

    pub fn dict_store_ref(&self) -> Option<&DictStore> {
        self.dict_store.as_ref()
    }

    pub fn merge_candidates(&self, llm_candidates: Vec<String>, limit: usize) -> Vec<String> {
        let hiragana = &self.hiragana_buf;

        // 優先順位: ユーザー辞書 → 学習履歴 (mozc 候補の押し上げ) → LLM → mozc
        let user_cands: Vec<String> = self
            .dict_store
            .as_ref()
            .map(|d| d.lookup_user(hiragana))
            .unwrap_or_default();

        let learn_cands: Vec<String> = self
            .dict_store
            .as_ref()
            .map(|d| d.lookup_learn(hiragana))
            .unwrap_or_default();

        let dict_cands: Vec<String> = self
            .dict_store
            .as_ref()
            .map(|d| d.lookup_dict(hiragana, limit))
            .unwrap_or_default();

        debug!(
            "engine::merge: reading={:?} dict_store={} dict_cands={:?} learn_cands={:?} llm_cands={:?}",
            hiragana,
            if self.dict_store.is_some() {
                "Some"
            } else {
                "None"
            },
            dict_cands,
            learn_cands,
            llm_candidates
        );

        let mut merged: Vec<String> = Vec::new();

        // 1. ユーザー辞書候補（最優先）
        for c in &user_cands {
            if merged.len() >= limit {
                break;
            }
            if !merged.contains(c) {
                merged.push(c.clone());
            }
        }

        // 2. 学習履歴: mozc/user に既にある surface のうち、最近/よく選ばれたものを前に出す。
        //    学習履歴にしかない surface (辞書ガードで弾かれているはずだが防御的に確認) は追加しない。
        for c in &learn_cands {
            if merged.len() >= limit {
                break;
            }
            if merged.contains(c) {
                continue;
            }
            if dict_cands.contains(c) || user_cands.contains(c) {
                merged.push(c.clone());
            }
        }

        // 辞書候補に確保するスロット数（limit の約 2/3、最低 5 件）。
        // 辞書ルックアップは binary search + 固定バイト読みで LLM より圧倒的に軽いため、
        // 性能を落とさずに候補数を増やす第一の手段として dict 優先の配分にしている。
        let dict_slots = (limit * 2 / 3).max(5);
        // LLM は残りのスロットを使用
        let llm_limit = limit.saturating_sub(dict_slots).max(1);

        // 3. LLM候補（文脈考慮、上限 llm_limit）
        let mut llm_count = 0;
        for c in llm_candidates {
            if llm_count >= llm_limit {
                break;
            }
            if merged.len() >= limit {
                break;
            }
            if !merged.contains(&c) {
                merged.push(c);
                llm_count += 1;
            }
        }

        // 4. mozc候補（残りスロットを全て使用）
        for c in dict_cands {
            if merged.len() >= limit {
                break;
            }
            if !merged.contains(&c) {
                merged.push(c);
            }
        }

        if merged.is_empty() {
            vec![hiragana.clone()]
        } else {
            merged
        }
    }

    pub fn backend_label(&self) -> String {
        compiled_backend_label().to_string()
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

        let hiragana = self.hiragana_buf.clone();
        let committed = self.committed.clone();
        if hiragana.is_empty() {
            return false;
        }
        if !self.is_kanji_ready() {
            return false;
        }

        if let Some(conv) = self.kanji.take() {
            match conv_cache::start(hiragana, committed, conv, n_cands) {
                Some(returned) => {
                    self.kanji = Some(returned);
                    false
                }
                None => true,
            }
        } else {
            false
        }
    }

    /// BG 変換の状態文字列（診断用）
    pub fn bg_status(&self) -> &'static str {
        conv_cache::status()
    }

    /// ライブ変換 preview 用にトップ候補だけを覗き見する (M2 §5.2)。
    ///
    /// `bg_take_candidates` と異なり cache 状態を進めず、converter は cache に
    /// 残す。dict マージも行わないため、preview の純度が上がり commit 経路と
    /// 干渉しない。状態を進めない=複数回 peek しても結果は同じ。
    ///
    /// 次回 `bg_start` で別キーが来たときは、`bg_start` 内部で
    /// `conv_cache::reclaim_nonblocking()` が Done state から converter を
    /// 回収するため、converter を engine.kanji に戻す手間は不要。
    pub fn bg_peek_top_candidate(&self, key: &str) -> Option<String> {
        conv_cache::peek_top_candidate(key)
    }

    /// key が一致する BG 変換結果を取得し、converter を engine に戻す。
    /// None = まだ完了していない / キー不一致
    ///
    /// ユーザー辞書ヒットは LLM 結果より優先するため先頭にマージする。
    /// ライブ変換 preview (先頭候補表示) でユーザー辞書が勝つ必要があるため。
    pub fn bg_take_candidates(&mut self, key: &str) -> Option<Vec<String>> {
        let (conv, cands) = conv_cache::take_ready(key)?;
        self.kanji = Some(conv);
        let user_cands: Vec<String> = self
            .dict_store
            .as_ref()
            .map(|d| d.lookup_user(key))
            .unwrap_or_default();
        if user_cands.is_empty() {
            return Some(cands);
        }
        let mut merged = user_cands;
        for c in cands {
            if !merged.contains(&c) {
                merged.push(c);
            }
        }
        Some(merged)
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

fn compiled_backend_label() -> &'static str {
    #[cfg(feature = "cuda")]
    {
        "CUDA"
    }
    #[cfg(all(not(feature = "cuda"), feature = "vulkan"))]
    {
        "Vulkan"
    }
    #[cfg(all(not(feature = "cuda"), not(feature = "vulkan")))]
    {
        "CPU"
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
        let text =
            "\u{6587}\u{5883}\u{754C}\u{306E}\u{306A}\u{3044}\u{30C6}\u{30AD}\u{30B9}\u{30C8}";
        assert_eq!(last_n_sentences_start(text, 2), 0);
    }

    #[test]
    fn single_boundary_want_two() {
        let text =
            "\u{6700}\u{521D}\u{306E}\u{6587}\u{3002}\u{4E8C}\u{756A}\u{76EE}\u{306E}\u{6587}";
        // \u{5883}\u{754C}\u{304C}1\u{500B}\u{3057}\u{304B}\u{306A}\u{3044} \u{2192} \u{5148}\u{982D}\u{3092}\u{8FD4}\u{3059}
        assert_eq!(last_n_sentences_start(text, 2), 0);
    }

    #[test]
    fn two_boundaries_want_two() {
        let text = "\u{6700}\u{521D}\u{306E}\u{6587}\u{3002}\u{4E8C}\u{756A}\u{76EE}\u{306E}\u{6587}\u{3002}\u{4E09}\u{756A}\u{76EE}\u{306E}\u{6587}";
        // \u{5883}\u{754C}\u{304C}2\u{500B} [\u{300C}\u{4E8C}\u{756A}\u{76EE}\u{300D}\u{5148}\u{982D}, \u{300C}\u{4E09}\u{756A}\u{76EE}\u{300D}\u{5148}\u{982D}]\u{3001}n=2 \u{2192} \u{5148}\u{982D}\u{304B}\u{3089}2\u{500B}\u{76EE}\u{306E}\u{5883}\u{754C} = \u{300C}\u{4E8C}\u{756A}\u{76EE}\u{300D}\u{5148}\u{982D}
        let start = last_n_sentences_start(text, 2);
        assert_eq!(
            &text[start..],
            "\u{4E8C}\u{756A}\u{76EE}\u{306E}\u{6587}\u{3002}\u{4E09}\u{756A}\u{76EE}\u{306E}\u{6587}"
        );
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
        assert_eq!(
            &text[start..],
            "\u{4E8C}\u{884C}\u{76EE}\n\u{4E09}\u{884C}\u{76EE}"
        );
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
mod symbol_input_tests {
    use super::RakunEngine;

    fn push(buf_init: &str, c: char) -> String {
        let mut e = RakunEngine::new(crate::EngineConfig::default());
        // hiragana_buf に初期値をセット
        e.force_preedit(buf_init.to_string());
        e.push_char(c);
        e.hiragana_text().to_string()
    }

    #[test]
    fn comma_to_kuten() {
        assert!(push("", ',').ends_with('、'));
        assert!(push("あ", ',').ends_with('、'));
    }

    #[test]
    fn comma_after_digit_stays_numeric_separator() {
        assert_eq!(push("2", ','), "2,");
        assert_eq!(push("２", '、'), "２,");
    }

    #[test]
    fn period_to_maru() {
        assert!(push("", '.').ends_with('。'));
    }

    #[test]
    fn period_after_digit_stays_numeric_separator() {
        assert_eq!(push("2", '.'), "2.");
        assert_eq!(push("２", '。'), "２.");
    }

    #[test]
    fn digit_separator_auto_can_be_disabled() {
        let config = crate::EngineConfig {
            digit_separator_auto: false,
            ..Default::default()
        };
        let mut e = RakunEngine::new(config);
        e.force_preedit("2".to_string());
        e.push_char(',');
        assert_eq!(e.hiragana_text(), "2、");
    }

    #[test]
    fn slash_to_nakaten() {
        assert!(push("", '/').ends_with('・'));
    }

    #[test]
    fn bracket_open() {
        assert!(push("", '[').ends_with('「'));
    }

    #[test]
    fn bracket_close() {
        assert!(push("", ']').ends_with('」'));
    }

    #[test]
    fn backslash_to_yen() {
        assert!(push("", '\\').ends_with('￥'));
    }

    #[test]
    fn minus_always_choon() {
        // 文脈依存ロジック廃止 → 常に ー
        assert!(push("", '-').ends_with('ー'));
        assert!(push("あ", '-').ends_with('ー'));
        assert!(push("abc", '-').ends_with('ー'));
    }

    #[test]
    fn other_symbols_fullwidth() {
        assert!(push("", '=').ends_with('＝'));
        assert!(push("", '@').ends_with('＠'));
        assert!(push("", '(').ends_with('（'));
        assert!(push("", ')').ends_with('）'));
    }
}

#[cfg(test)]
mod digit_width_tests {
    use super::{DigitWidth, EngineConfig, RakunEngine};

    fn push_digit(width: DigitWidth, c: char) -> String {
        let config = EngineConfig {
            digit_width: width,
            ..Default::default()
        };
        let mut e = RakunEngine::new(config);
        e.push_char(c);
        e.hiragana_text().to_string()
    }

    #[test]
    fn halfwidth_keeps_ascii() {
        assert_eq!(push_digit(DigitWidth::Halfwidth, '0'), "0");
        assert_eq!(push_digit(DigitWidth::Halfwidth, '5'), "5");
        assert_eq!(push_digit(DigitWidth::Halfwidth, '9'), "9");
    }

    #[test]
    fn fullwidth_converts() {
        assert_eq!(push_digit(DigitWidth::Fullwidth, '0'), "０");
        assert_eq!(push_digit(DigitWidth::Fullwidth, '5'), "５");
        assert_eq!(push_digit(DigitWidth::Fullwidth, '9'), "９");
    }

    #[test]
    fn halfwidth_sequence() {
        let config = EngineConfig {
            digit_width: DigitWidth::Halfwidth,
            ..Default::default()
        };
        let mut e = RakunEngine::new(config);
        for c in "2024".chars() {
            e.push_char(c);
        }
        assert_eq!(e.hiragana_text(), "2024");
    }

    #[test]
    fn default_is_halfwidth() {
        assert_eq!(DigitWidth::default(), DigitWidth::Halfwidth);
        assert_eq!(push_digit(DigitWidth::default(), '3'), "3");
    }
}

#[cfg(test)]
mod passthrough_sync_tests {
    //! pending_romaji_buf と romaji.buffer の同期を検証する。
    //! PassThrough 連鎖で複数文字が確定する場合に、未確定ローマ字が
    //! 表示から落ちないことを保証する（旧バグ: "qwrty" → "qwry" 表示）。
    use super::{EngineConfig, RakunEngine};

    fn type_string(input: &str) -> RakunEngine {
        let mut e = RakunEngine::new(EngineConfig::default());
        for c in input.chars() {
            e.push_char(c);
        }
        e
    }

    #[test]
    fn qwrty_shows_all_typed_chars() {
        let e = type_string("qwrty");
        assert_eq!(e.current_preedit().display(), "qwrty");
    }

    #[test]
    fn kana_then_kq_shows_pending_q() {
        let e = type_string("kanakq");
        assert_eq!(e.current_preedit().display(), "かなkq");
    }

    #[test]
    fn kana_then_kq_then_bs_removes_q_only() {
        let mut e = type_string("kanakq");
        e.backspace();
        assert_eq!(e.current_preedit().display(), "かなk");
    }

    #[test]
    fn romaji_log_matches_typed_input_for_qwrty() {
        // F9/F10 復元のため、log + pending = ユーザーが入力したローマ字列 を保つ。
        let e = type_string("qwrty");
        let log = e.romaji_log_str();
        let pending = e.current_preedit().pending_romaji.clone();
        assert_eq!(format!("{}{}", log, pending), "qwrty");
    }
}
