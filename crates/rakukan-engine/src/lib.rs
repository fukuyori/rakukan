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

// cdylib のリンク時に link.exe が stdout へ出す情報行（「ライブラリ ... を作成中」）を
// rustc の linker_messages lint が warning 表示するため、crate 単位で allow する
//（rakukan-tsf と同じ対処）。注意: この allow により cuda variant の LNK4098
//（LIBCMT と動的 CRT の混在。CUDA 静的ランタイム由来の既知の警告で、動作実績あり）も
// 表示されなくなる。CRT 統一で根本対処する場合はこの allow を外して確認すること。
#![allow(linker_messages)]

// ── 統合した karukan-engine モジュール ────────────────────────────────────────
pub mod kana;
pub mod kanji;
mod latin_run;
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

/// context への追加を拒否する、ひらがな（+長音・中点）の最小文字数。
/// 変換時の strip（backend.rs の `ECHO_RUN_MIN_CHARS` = 8）より低く設定する:
/// 「きもちは、」のような短いひらがな確定も、同じ読みの再変換でエコーを誘発するため。
const CONTEXT_ECHO_MIN_HIRAGANA_CHARS: usize = 4;

/// context に入れると LLM のエコーアトラクタ（変換ではなく context からの
/// コピー）を誘発するテキストか判定する。
///
/// 対象は「未変換のまま確定された」ひらがな文: 句読点・空白を除いた全文字が
/// ひらがな（+長音・中点）で、その数が `CONTEXT_ECHO_MIN_HIRAGANA_CHARS` 以上。
/// 漢字・カタカナ・英数字を 1 文字でも含むテキストは変換済みとみなして通す
/// （カタカナ確定はエコーしても正しい出力になるため対象外。混在汚染は
/// backend.rs の `strip_echo_context` が保険として捕捉する）。
fn is_context_echo_risk(text: &str) -> bool {
    let mut hiragana_count = 0usize;
    for c in text.chars() {
        let n = c as u32;
        if (0x3041..=0x3096).contains(&n) || c == 'ー' || c == '・' {
            hiragana_count += 1;
        } else if matches!(
            c,
            '、' | '。'
                | '！'
                | '？'
                | '!'
                | '?'
                | '.'
                | '．'
                | '，'
                | ','
                | '\n'
                | ' '
                | '\u{3000}'
                | '「'
                | '」'
                | '『'
                | '』'
                | '（'
                | '）'
                | '('
                | ')'
        ) {
            // 句読点・括弧・空白は無視
        } else {
            // 漢字・カタカナ・英数字等を含む → 変換済みテキストとみなす
            return false;
        }
    }
    hiragana_count >= CONTEXT_ECHO_MIN_HIRAGANA_CHARS
}

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
#[derive(Default)]
pub enum DigitWidth {
    Fullwidth,
    #[default]
    Halfwidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AlphaWidth {
    #[default]
    Fullwidth,
    Halfwidth,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum SymbolWidth {
    #[default]
    Fullwidth,
    Halfwidth,
}

fn default_digit_separator_auto() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitCandidateKind {
    Arabic,
    Fullwidth,
    Positional,
    PerDigit,
    Daiji,
}

pub fn default_digit_candidates_order() -> Vec<DigitCandidateKind> {
    vec![
        DigitCandidateKind::Arabic,
        DigitCandidateKind::Fullwidth,
        DigitCandidateKind::Positional,
        DigitCandidateKind::PerDigit,
        DigitCandidateKind::Daiji,
    ]
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
    /// 英字の入力幅: "fullwidth" = 全角 (ＡＢＣ), "halfwidth" = 半角 (ABC)
    #[serde(default)]
    pub alpha_width: AlphaWidth,
    /// 記号の入力幅: "fullwidth" = 全角 (＠＃), "halfwidth" = 半角 (@#)
    #[serde(default)]
    pub symbol_width: SymbolWidth,
    /// 数字直後の句読点を数値区切りとして扱う。
    #[serde(default = "default_digit_separator_auto")]
    pub digit_separator_auto: bool,
    /// 数字だけの reading に対して提示する候補種別と順序。
    #[serde(default = "default_digit_candidates_order")]
    pub digit_candidates_order: Vec<DigitCandidateKind>,
    /// ライブ変換時の候補数（beam 幅に影響）。1 = greedy（高速）、3 = beam（高品質）
    pub live_conv_beam_size: usize,
    /// Space 変換時のビーム幅の**上限**（num_candidates と併せて min をとる）。
    /// デフォルト 30 では実質上限なし、num_candidates がそのまま beam 幅になる。
    pub convert_beam_size: usize,
    /// 異常変換の棄却に使う「最良候補からの平均 log-prob 差」の許容幅 (nats/token)。
    /// `null` で無効。既定 3.0 は寛容で、明らかな外れ値候補のみ落とす。
    /// 詳細は `kanji::ConversionConfig::confidence_margin` を参照。
    #[serde(default = "default_confidence_margin")]
    pub confidence_margin: Option<f32>,
    /// 最良候補の平均 log-prob (nats/token) の絶対下限。これを下回る変換は幻覚の
    /// 可能性が高いため全候補を捨て、かなにフォールバックする。`null`（既定）で無効。
    /// 詳細は `kanji::ConversionConfig::min_top_confidence` を参照。
    #[serde(default)]
    pub min_top_confidence: Option<f32>,
}

fn default_confidence_margin() -> Option<f32> {
    Some(3.0)
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
            alpha_width: AlphaWidth::default(),
            symbol_width: SymbolWidth::default(),
            digit_separator_auto: true,
            digit_candidates_order: default_digit_candidates_order(),
            live_conv_beam_size: 3,
            convert_beam_size: 30,
            confidence_margin: default_confidence_margin(),
            min_top_confidence: None,
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

/// 変換器に 1 文字流し、このステップで確定した (打鍵, 出力) を返す。
/// `push_char` 経路 5 と Backspace 再生（`replay_romaji_run`）で共用する。
/// `output` / `buffer` の差分で判定するので、PassThrough の連鎖で複数文字が
/// 確定するケースも 1 エントリにまとまる。
fn romaji_step(
    conv: &mut RomajiConverter,
    pending: &mut String,
    c: char,
) -> Option<(String, String)> {
    pending.push(c);
    let prev_output_len = conv.output().len();
    let _ = conv.push(c);
    let added = conv.output()[prev_output_len..].to_string();
    let new_buffer_len = conv.buffer().len();
    debug_assert!(new_buffer_len <= pending.len());
    let consumed_len = pending.len() - new_buffer_len;
    (consumed_len > 0).then(|| (pending.drain(..consumed_len).collect(), added))
}

/// ローマ字区間の打鍵列を先頭から新しい変換器に流し、
/// (エントリ列, 未確定ローマ字, 変換器) を返す（Step 10-3 の Backspace 再生用）。
fn replay_romaji_run(typed: &str) -> (Vec<InputEntry>, String, RomajiConverter) {
    let mut conv = RomajiConverter::new();
    let mut entries = Vec::new();
    let mut pending = String::new();
    for c in typed.chars() {
        if let Some((entry_typed, output)) = romaji_step(&mut conv, &mut pending, c) {
            entries.push(InputEntry {
                typed: entry_typed,
                output,
                kind: InputKind::Romaji,
                closes_run: false,
            });
        }
    }
    (entries, pending, conv)
}

/// Step 10-4 の 1 段目: `run_typed` の接頭辞を再生して、出力が `target` に一致する
/// エントリ列を探す。未確定が残らない一致（`kata` → `ka`）を優先し、無ければ未確定を
/// 末尾エントリの打鍵に含めた形（`tta` → `tt` = 「っ」）を返す。いずれも最長を採る。
fn replay_prefix_for(run_typed: &str, target: &str) -> Option<Vec<InputEntry>> {
    let mut with_pending: Option<Vec<InputEntry>> = None;
    let cuts: Vec<usize> = run_typed
        .char_indices()
        .map(|(i, _)| i)
        .skip(1)
        .chain(std::iter::once(run_typed.len()))
        .collect();
    for &cut in cuts.iter().rev() {
        let (mut entries, pending, _) = replay_romaji_run(&run_typed[..cut]);
        let output: String = entries.iter().map(|e| e.output.as_str()).collect();
        if output != target {
            continue;
        }
        if pending.is_empty() {
            return Some(entries);
        }
        if with_pending.is_none()
            && let Some(last) = entries.last_mut()
        {
            last.typed.push_str(&pending);
            with_pending = Some(entries);
        }
    }
    with_pending
}

/// Step 10-4 の 2 段目: 残るかな `remaining` を出す綴りを逆引きする。
/// 母音字 1 文字（`a` `i` `u` `e` `o`）が候補にあればそれ（`wi` → 「う」は `u`）。
/// それ以外は元の綴り `original` と共有する接頭辞が最長のもの（`sha` → 「し」は `shi`）、
/// 同点なら短いもの、さらに同点なら辞書順。
fn reverse_spelling(remaining: &str, original: &str) -> Option<String> {
    let candidates = romaji::spellings_for(remaining);
    if let Some(vowel) = candidates
        .iter()
        .find(|c| matches!(c.as_str(), "a" | "i" | "u" | "e" | "o"))
    {
        return Some(vowel.clone());
    }
    fn common_prefix_len(a: &str, b: &str) -> usize {
        a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
    }
    candidates
        .iter()
        .max_by(|x, y| {
            common_prefix_len(x, original)
                .cmp(&common_prefix_len(y, original))
                .then(y.len().cmp(&x.len()))
                .then(y.cmp(x))
        })
        .cloned()
}

/// 候補リストを優先順位どおりに 1 本へマージする（Step 12-1）。
///
/// 順序は 学習履歴 → ユーザー辞書 → システム辞書 → LLM。重複は先に積んだ側を残す。
/// システム辞書は `limit` まで埋めず、LLM 候補（`merged` にまだ無いもの）の分だけ
/// 枠を残す。従来は辞書候補が 40 件ある読みで LLM 候補が 1 件も入らなかった
/// （Issue #42 の問題点 4）。LLM の枠を残しても辞書候補が少なければ LLM が
/// その分を使うので、合計は `limit` を超えない。
fn merge_candidate_lists(
    learn: &[String],
    user: &[String],
    dict: &[String],
    llm: Vec<String>,
    limit: usize,
) -> Vec<String> {
    let mut merged: Vec<String> = Vec::new();
    let push_unique = |merged: &mut Vec<String>, c: &str, cap: usize| {
        if merged.len() < cap && !merged.iter().any(|m| m == c) {
            merged.push(c.to_string());
        }
    };

    // 1. 学習履歴（スコア順。最近・頻繁に選んだものが先頭）
    for c in learn {
        push_unique(&mut merged, c, limit);
    }
    // 2. ユーザー辞書
    for c in user {
        push_unique(&mut merged, c, limit);
    }
    // 3. システム辞書。LLM の未登場候補ぶんは枠を残す
    let mut llm_unique: Vec<String> = Vec::new();
    for c in llm {
        if !merged.contains(&c) && !llm_unique.contains(&c) {
            llm_unique.push(c);
        }
    }
    let dict_cap = limit.saturating_sub(llm_unique.len()).max(merged.len());
    for c in dict {
        push_unique(&mut merged, c, dict_cap);
    }
    // 4. LLM 候補（文脈考慮。辞書と重複するものは辞書側の位置に残る）
    for c in &llm_unique {
        push_unique(&mut merged, c, limit);
    }
    // 5. LLM 候補が辞書と重複して枠が余ったら、辞書の残りで埋め戻す
    for c in dict {
        push_unique(&mut merged, c, limit);
    }
    merged
}

/// 記号を差し込む位置: 通常候補の上位この件数の後ろ（mozc の SymbolRewriter と同じ 3）
const SYMBOL_INSERT_AFTER: usize = 3;
/// 前方に差し込む記号の最大数（mozc と同じ 15）。残りは末尾
const SYMBOL_PROMOTE: usize = 15;

/// 記号・絵文字を通常候補の列に差し込む（Step 12-2、Issue #42）。
///
/// mozc の SymbolRewriter に倣い、通常候補の上位 `SYMBOL_INSERT_AFTER` 件の後ろに
/// 記号を最大 `SYMBOL_PROMOTE` 個入れ、残りの記号は末尾、絵文字はさらにその後ろに
/// 置く。「みぎ」なら 右 → みぎ → ミギ → 記号 15 個 → 残りの記号 → 絵文字。
/// 通常候補と重複する記号・絵文字は入れない。戻りの件数は `limit` を超えてよい
/// （候補ウィンドウはページ送りを持つ）。
fn place_symbol_candidates(
    base: Vec<String>,
    symbols: Vec<String>,
    emoji: Vec<String>,
) -> Vec<String> {
    if symbols.is_empty() && emoji.is_empty() {
        return base;
    }
    let mut seen: Vec<String> = base.clone();
    let mut unique = |items: Vec<String>| -> Vec<String> {
        let mut out = Vec::new();
        for c in items {
            if !seen.contains(&c) {
                seen.push(c.clone());
                out.push(c);
            }
        }
        out
    };
    let mut symbols = unique(symbols);
    let emoji = unique(emoji);
    let rest = if symbols.len() > SYMBOL_PROMOTE {
        symbols.split_off(SYMBOL_PROMOTE)
    } else {
        Vec::new()
    };
    let at = SYMBOL_INSERT_AFTER.min(base.len());
    let mut out = Vec::with_capacity(base.len() + symbols.len() + rest.len() + emoji.len());
    out.extend_from_slice(&base[..at]);
    out.extend(symbols);
    out.extend_from_slice(&base[at..]);
    out.extend(rest);
    out.extend(emoji);
    out
}

/// `push_char` で trie（ローマ字ルール）に委ねる文字か。
/// 英字と `,./[]\-`（、。・「」￥ー等のルールがある記号）。
fn is_trie_input_char(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, ',' | '.' | '/' | '[' | ']' | '\\' | '-')
}

fn is_alpha_char(c: char) -> bool {
    c.is_ascii_alphabetic() || ('Ａ'..='Ｚ').contains(&c) || ('ａ'..='ｚ').contains(&c)
}

fn is_symbol_char(c: char) -> bool {
    let n = c as u32;
    // ASCII printable 記号（英数字除く）
    if (0x21..=0x7E).contains(&n) && !c.is_ascii_alphanumeric() {
        return true;
    }
    // 全角記号 (U+FF01..=U+FF5E)、ただし全角英数字を除く
    if (0xFF01..=0xFF5E).contains(&n)
        && !('０'..='９').contains(&c)
        && !('Ａ'..='Ｚ').contains(&c)
        && !('ａ'..='ｚ').contains(&c)
    {
        return true;
    }
    false
}

/// `,` / `.` / `、` / `。` を、直前文字の種類と幅設定に応じて
/// Western 句読点（全角 ， ． or 半角 , .）として返す。
/// 直前が英字でも記号でもなければ `None`（変換せず trie に委ねる）。
fn alpha_symbol_separator_auto(
    prev: Option<char>,
    c: char,
    alpha_width: AlphaWidth,
    symbol_width: SymbolWidth,
) -> Option<char> {
    let prev = prev?;
    let fullwidth = if is_alpha_char(prev) {
        matches!(alpha_width, AlphaWidth::Fullwidth)
    } else if is_symbol_char(prev) {
        matches!(symbol_width, SymbolWidth::Fullwidth)
    } else {
        return None;
    };
    match (c, fullwidth) {
        (',' | '、', true) => Some('，'), // U+FF0C 全角コンマ
        (',' | '、', false) => Some(','),
        ('.' | '。', true) => Some('．'), // U+FF0E 全角ピリオド
        ('.' | '。', false) => Some('.'),
        _ => None,
    }
}

/// 入力ログのエントリ種別（どの経路で入力されたか）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputKind {
    /// `push_char` 経路 5。trie で変換する通常ローマ字
    Romaji,
    /// `push_char` 経路 3。数字（`digit_width` で幅が決まる）
    Digit,
    /// `push_char` 経路 4。ASCII 記号（`symbol_width` で幅が決まる）
    Symbol,
    /// `push_char` 経路 1・2。自動置換された区切り（数値区切り・欧文句読点）
    Separator,
    /// `push_raw`。かなルール登録文字などの直接入力
    Raw,
    /// `push_fullwidth_alpha`。Shift+英字（`typed` は ASCII 大文字）
    ShiftAlpha,
}

/// 入力ログの 1 エントリ。
///
/// 不変条件（Backspace / force_preedit を除く経路で成立）:
/// - `typed` の連結 + `pending_romaji_buf` == ユーザーが打った文字列
/// - `output` の連結 == `hiragana_buf`
#[derive(Debug, Clone, PartialEq, Eq)]
struct InputEntry {
    /// ユーザーが打った文字そのまま（"kya" / "1" / "A" / "。"）
    typed: String,
    /// このエントリが `hiragana_buf` に足した文字列（"きゃ" / "１" / "Ａ" / "。"）
    output: String,
    kind: InputKind,
    /// このエントリでローマ字区間が閉じた（`flush_pending_n`）。
    /// Backspace 再生（Step 10-3）で区間の境界として使う。
    closes_run: bool,
}

pub struct RakunEngine {
    romaji: RomajiConverter,
    kanji: Option<KanaKanjiConverter>,
    config: EngineConfig,
    hiragana_buf: String,
    pending_romaji_buf: String,
    /// 入力ログ。`hiragana_buf` に文字が足されるたびに 1 エントリ積む
    /// （ローマ字は `RomajiConverter::Converted` 単位）。
    /// 未確定分（`pending_romaji_buf`）はログに入らない。
    /// F6〜F10 の文字種変換で元の表示・ローマ字を復元するのに使う。
    input_log: Vec<InputEntry>,
    /// `force_preedit` で `hiragana_buf` を差し替えた時点の `input_log.len()`。
    /// これより前のエントリは `hiragana_buf` と対応しないので、Backspace 再生の対象外。
    /// log をクリアしたら 0 に戻す。
    log_detached_at: usize,
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
            input_log: Vec::new(),
            log_detached_at: 0,
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
            confidence_margin: config.confidence_margin,
            min_top_confidence: config.min_top_confidence,
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

    fn log_push(&mut self, typed: impl Into<String>, output: impl Into<String>, kind: InputKind) {
        self.input_log.push(InputEntry {
            typed: typed.into(),
            output: output.into(),
            kind,
            closes_run: false,
        });
    }

    fn log_pop(&mut self) {
        self.input_log.pop();
        self.log_detached_at = self.log_detached_at.min(self.input_log.len());
    }

    fn log_clear(&mut self) {
        self.input_log.clear();
        self.log_detached_at = 0;
    }

    /// 未確定ローマ字を閉じる（Step 10-2）。
    ///
    /// ローマ字以外の入力（記号・Shift+英字・数字）が来たとき、先に pending を確定側へ
    /// 移してから追記することで、表示順と打鍵順が一致する（`k` + `。` = 「k。」）。
    /// 閉じ方は `flush_pending_n`（`n` だけなら「ん」）→ `RomajiConverter::flush`
    /// （残りは trie 一致か素通し）の順。閉じたエントリには `closes_run` を立てる。
    fn close_pending(&mut self) {
        if self.pending_romaji_buf.is_empty() {
            return;
        }
        if self.flush_pending_n() {
            return;
        }
        let typed = std::mem::take(&mut self.pending_romaji_buf);
        let output = self.romaji.flush();
        self.hiragana_buf.push_str(&output);
        debug!("engine::close_pending: {:?} → {:?}", typed, output);
        self.input_log.push(InputEntry {
            typed,
            output,
            kind: InputKind::Romaji,
            closes_run: true,
        });
        self.romaji = RomajiConverter::new();
    }

    pub fn push_char(&mut self, c: char) -> PreeditState {
        // 数字と trie 外の ASCII 記号は pending を閉じてから経路 3・4 で扱う。
        // `,./[]\-` と英字は trie に委ねる（pending と結合しうるため）。
        if !self.pending_romaji_buf.is_empty() && !is_trie_input_char(c) {
            let n = c as u32;
            if c.is_ascii_digit() || ((0x21..=0x7E).contains(&n) && !c.is_ascii_alphabetic()) {
                self.close_pending();
            }
        }

        if self.config.digit_separator_auto
            && self.pending_romaji_buf.is_empty()
            && let Some(separator) =
                numeric_separator_after_digit(self.hiragana_buf.chars().last(), c)
        {
            self.hiragana_buf.push(separator);
            self.log_push(c, separator, InputKind::Separator);
            debug!("engine::push: numeric separator {:?} → {:?}", c, separator);
            return self.current_preedit();
        }

        // 英字・記号後の `,` / `.` を Western 句読点 (， / ． or , / .) へ自動置換
        // 幅設定 (alpha_width / symbol_width) に追従する。
        if self.pending_romaji_buf.is_empty()
            && let Some(separator) = alpha_symbol_separator_auto(
                self.hiragana_buf.chars().last(),
                c,
                self.config.alpha_width,
                self.config.symbol_width,
            )
        {
            self.hiragana_buf.push(separator);
            self.log_push(c, separator, InputKind::Separator);
            debug!(
                "engine::push: alpha/symbol separator {:?} → {:?}",
                c, separator
            );
            return self.current_preedit();
        }

        // 数字 0–9（pending_romaji がない場合のみ）
        if self.pending_romaji_buf.is_empty() && c.is_ascii_digit() {
            let out = match self.config.digit_width {
                DigitWidth::Fullwidth => char::from_u32(c as u32 - 0x30 + 0xFF10).unwrap_or(c),
                DigitWidth::Halfwidth => c,
            };
            self.hiragana_buf.push(out);
            self.log_push(c, out, InputKind::Digit);
            debug!("engine::push: digit {:?} → {:?}", c, out);
            return self.current_preedit();
        }

        // ASCII 記号の処理（pending_romaji がない場合のみ）
        // ,./[]\- はトライのルール（、。・「」￥ー等）に委ねる。
        // それ以外の印字可能 ASCII 記号（@#$%^&*()+=_:"~!? 等）は
        // symbol_width に従って全角 or 半角で即確定する。
        if self.pending_romaji_buf.is_empty() {
            let n = c as u32;
            let is_ascii_printable = (0x21..=0x7E).contains(&n);
            let is_trie_symbol = matches!(c, ',' | '.' | '/' | '[' | ']' | '\\' | '-');
            if is_ascii_printable && !is_trie_symbol && !c.is_ascii_alphanumeric() {
                let out = match self.config.symbol_width {
                    SymbolWidth::Fullwidth => char::from_u32(n - 0x21 + 0xFF01).unwrap_or(c),
                    SymbolWidth::Halfwidth => c,
                };
                self.hiragana_buf.push(out);
                self.log_push(c, out, InputKind::Symbol);
                debug!("engine::push: symbol {:?} → {:?}", c, out);
                return self.current_preedit();
            }
        }

        // ,./[]\- および英字 → ローマ字ルール（trie）に委ねる
        // pending_romaji_buf と romaji.buffer は常に同じ状態を保つ。
        // ConversionEvent variant ではなく romaji.output / romaji.buffer の差分から
        // 「確定したひらがな」と「未確定として残っているローマ字」を判定する。
        // （PassThrough の連鎖で複数文字が確定するケースを正しく扱うため）
        if let Some((entry, added)) = romaji_step(&mut self.romaji, &mut self.pending_romaji_buf, c)
        {
            self.hiragana_buf.push_str(&added);
            debug!("engine::push: romaji {:?} → {:?}", entry, added);
            self.log_push(entry, added, InputKind::Romaji);
        }
        self.current_preedit()
    }

    /// 末尾の未確定 "n" を「ん」として確定する（Convert / CommitRaw 前に呼ぶ）
    pub fn flush_pending_n(&mut self) -> bool {
        if self.pending_romaji_buf == "n" {
            self.hiragana_buf.push('ん');
            let entry = std::mem::take(&mut self.pending_romaji_buf);
            self.input_log.push(InputEntry {
                typed: entry,
                output: "ん".to_string(),
                kind: InputKind::Romaji,
                closes_run: true,
            });
            self.romaji = RomajiConverter::new();
            true
        } else {
            false
        }
    }

    /// プリエディット文字列を強制置換する（F6〜F10 の文字種変換用）
    /// input_log は保持する（F9/F10 サイクル中に再度ローマ字に戻せるよう）
    pub fn force_preedit(&mut self, text: String) {
        self.hiragana_buf = text;
        self.pending_romaji_buf.clear();
        self.romaji = RomajiConverter::new();
        self.log_detached_at = self.input_log.len();
    }

    /// ローマ字変換を経由せず hiragana_buf に直接1文字追加する。
    /// テンキー記号など、かなルールに登録されている文字をそのまま入力する場合に使用する。
    pub fn push_raw(&mut self, c: char) {
        self.close_pending();
        self.hiragana_buf.push(c);
        self.log_push(c, c, InputKind::Raw);
    }

    /// Shift+アルファベット用: alpha_width 設定に従って全角 or 半角の大文字を hiragana_buf に追加。
    /// `input_log` の `typed` には ASCII 大文字を記録する。
    ///
    /// F9/F10 のサイクル変換は input_log の ASCII 文字を元に動作するため、
    /// `typed` には元の ASCII 文字（'A'–'Z'）を保持する必要がある。
    /// `c` には ASCII 大文字（'A'–'Z'）を渡すこと。
    pub fn push_fullwidth_alpha(&mut self, c: char) {
        debug_assert!(c.is_ascii_uppercase());
        self.close_pending();
        let out = match self.config.alpha_width {
            AlphaWidth::Fullwidth => char::from_u32(c as u32 - 0x41 + 0xFF21).unwrap_or(c),
            AlphaWidth::Halfwidth => c,
        };
        self.hiragana_buf.push(out);
        self.log_push(c, out, InputKind::ShiftAlpha);
    }

    pub fn backspace(&mut self) -> bool {
        if !self.pending_romaji_buf.is_empty() {
            if self.replay_backspace() {
                return true;
            }
            // 再生できない（log と表示がずれている）ときは未確定 1 文字を消すだけ
            self.pending_romaji_buf.pop();
            let _ = self.romaji.backspace();
            return true;
        }
        self.pop_display_char()
    }

    /// pending が空のときの Backspace（Step 10-4）。
    ///
    /// 表示の末尾 1 文字を消し、log を「残った表示を再生できる打鍵列」に書き換える。
    /// 書き換えは §4.3 の 2 段（`replay_prefix_for` → `reverse_spelling`）。どちらも
    /// 効かなければ末尾エントリの打鍵から 1 文字削る。書き換えた区間の末尾エントリは
    /// `closes_run` で閉じ、以後の Backspace 再生（10-3）で流し直さない。
    ///
    /// detach 後（`force_preedit` の後）や log と表示が対応しないときは、現行どおり
    /// 表示 1 文字とエントリ 1 つを消す。
    fn pop_display_char(&mut self) -> bool {
        let Some(removed) = self.hiragana_buf.pop() else {
            return false;
        };
        // 区間は閉じるので変換器の履歴は要らない
        self.romaji = RomajiConverter::new();

        let len = self.input_log.len();
        let detached = len <= self.log_detached_at;
        let Some(last) = self.input_log.last() else {
            return true;
        };
        if detached || !last.output.ends_with(removed) {
            self.log_pop();
            return true;
        }
        let start = if last.kind != InputKind::Romaji {
            len
        } else if last.closes_run {
            len - 1
        } else {
            self.romaji_run_start()
        };
        if start == len {
            // 1 文字出力の非ローマ字エントリ
            self.log_pop();
            return true;
        }

        let run = &self.input_log[start..];
        let run_typed: String = run.iter().map(|e| e.typed.as_str()).collect();
        let mut target: String = run.iter().map(|e| e.output.as_str()).collect();
        target.pop();
        let last_typed = last.typed.clone();
        let mut last_remaining = last.output.clone();
        last_remaining.pop();

        if target.is_empty() {
            self.input_log.truncate(start);
            return true;
        }
        if let Some(mut entries) = replay_prefix_for(&run_typed, &target) {
            self.input_log.truncate(start);
            if let Some(e) = entries.last_mut() {
                e.closes_run = true;
            }
            debug!(
                "engine::backspace: rewrote run {:?} → {:?}",
                run_typed,
                entries.iter().map(|e| e.typed.as_str()).collect::<String>()
            );
            self.input_log.extend(entries);
            return true;
        }
        if last_remaining.is_empty() {
            // 末尾エントリを丸ごと消し、残る区間の末尾を閉じる
            self.log_pop();
            if let Some(e) = self.input_log.last_mut() {
                e.closes_run = true;
            }
            return true;
        }
        let typed = reverse_spelling(&last_remaining, &last_typed).unwrap_or_else(|| {
            let mut t = last_typed.clone();
            t.pop();
            t
        });
        debug!(
            "engine::backspace: rewrote entry {:?} → {:?} ({:?})",
            last_typed, typed, last_remaining
        );
        let e = self.input_log.last_mut().expect("checked above");
        e.typed = typed;
        e.output = last_remaining;
        e.closes_run = true;
        true
    }

    /// 末尾のローマ字区間の開始 index。
    /// 区間は、最後の非ローマ字エントリ・区間終端（`closes_run`）・detach 境界のいずれかの
    /// 直後から始まる。
    fn romaji_run_start(&self) -> usize {
        let floor = self.log_detached_at.min(self.input_log.len());
        let mut start = self.input_log.len();
        while start > floor {
            let e = &self.input_log[start - 1];
            if e.kind != InputKind::Romaji || e.closes_run {
                break;
            }
            start -= 1;
        }
        start
    }

    /// pending があるときの Backspace（Step 10-3）。
    ///
    /// 末尾のローマ字区間と pending を合わせた打鍵列から末尾 1 打鍵を消し、区間を
    /// 新しい変換器で再生して `input_log` / `hiragana_buf` / `pending_romaji_buf` /
    /// 変換器を同じ打鍵列を表す状態にする。素通しで確定側に固定されていた `k`（`kt`）や
    /// 次の子音で確定した「ん」「っ」（`nt` / `tt`）が打鍵どおりの未確定に戻る。
    ///
    /// 区間の出力が `hiragana_buf` の末尾と一致しない（log と表示がずれている）ときは
    /// 何もせず false を返し、呼び出し側が現行の単純 pop に落とす。
    fn replay_backspace(&mut self) -> bool {
        let start = self.romaji_run_start();
        let run_output: String = self.input_log[start..]
            .iter()
            .map(|e| e.output.as_str())
            .collect();
        if !self.hiragana_buf.ends_with(&run_output) {
            return false;
        }
        let mut typed: String = self.input_log[start..]
            .iter()
            .map(|e| e.typed.as_str())
            .collect();
        typed.push_str(&self.pending_romaji_buf);
        typed.pop();

        self.input_log.truncate(start);
        let keep = self.hiragana_buf.len() - run_output.len();
        self.hiragana_buf.truncate(keep);
        let (entries, pending, conv) = replay_romaji_run(&typed);
        for e in &entries {
            self.hiragana_buf.push_str(&e.output);
        }
        self.input_log.extend(entries);
        debug!(
            "engine::backspace: replayed {:?} → hira {:?} pending {:?}",
            typed, self.hiragana_buf, pending
        );
        self.pending_romaji_buf = pending;
        self.romaji = conv;
        true
    }

    /// 変換器へ渡す読み。
    ///
    /// ひらがなモードのまま打った先頭の英単語は、読みの上ではローマ字が潰れた姿
    /// （`せえdれあm`）になっている。これをそのままリテラル保護レイヤーへ渡すと
    /// 素の `d` / `m` だけがアルファベット run になり、日本語がぶつ切りで LLM に
    /// 渡って壊れる。打鍵ログから英単語を復元して `seedreamのぺーすはどう` の形に
    /// してから変換へ回す。
    ///
    /// 復元できない読み（普通の日本語 / 読み全体が英単語 / 日本語の前置きがある /
    /// F9-F10 で force_preedit した後）はそのまま返す。
    pub fn conv_reading(&self) -> String {
        if let Some(normalized) = latin_run::normalize_leading_latin(
            &self.input_log,
            self.log_detached_at,
            &self.hiragana_buf,
        ) {
            info!(
                "engine::conv_reading: latin run restored {:?} -> {:?}",
                self.hiragana_buf, normalized
            );
            normalized
        } else {
            self.hiragana_buf.clone()
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
            &self.conv_reading(),
            &self.committed,
            num_candidates,
            &self.config.digit_candidates_order,
            matches!(self.config.alpha_width, AlphaWidth::Fullwidth),
            matches!(self.config.symbol_width, SymbolWidth::Fullwidth),
        )
        .map_err(|e| EngineError::ConversionFailed(e.to_string()))
    }

    pub fn convert_default(&self) -> Result<Vec<String>, EngineError> {
        self.convert(self.config.num_candidates)
    }

    pub fn commit(&mut self, text: &str) {
        info!("engine::commit: {:?}", text);
        if is_context_echo_risk(text) {
            // 未変換のまま確定されたひらがな文を context に入れると、同じ読みの
            // 変換で LLM がコピー（エコー）に収束する（v0.9.15 のエコーアトラクタ）。
            // 確定自体は成立させ、context にだけ入れない。
            info!("engine::commit: hiragana-only text excluded from context");
            self.hiragana_buf.clear();
            self.log_clear();
            self.romaji = RomajiConverter::new();
            return;
        }
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
        self.log_clear();
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

    /// 入力ログの打鍵文字を結合した文字列を返す（F9/F10 のローマ字復元用）
    pub fn romaji_log_str(&self) -> String {
        self.input_log.iter().map(|e| e.typed.as_str()).collect()
    }

    /// input_log から F9/F10 を押す前の表示を復元する（F6/F7/F8 でかなに戻す用）。
    /// F9/F10 で force_preedit した後でも log は保持されているため復元可能。
    ///
    /// 各エントリが入力時に `hiragana_buf` へ足した文字列（`output`）を連結する。
    /// 変換器に流し直さないので、Shift+英字・数字・区切り記号・`flush_pending_n` の
    /// 「ん」が入力時の幅と文字のまま戻る。
    pub fn hiragana_from_romaji_log(&self) -> String {
        self.input_log.iter().map(|e| e.output.as_str()).collect()
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

    pub fn learn_force(&mut self, reading: &str, surface: &str) {
        if let Some(store) = &self.dict_store {
            store.learn_force(reading, surface);
        } else {
            tracing::warn!("learn_force: dict_store not initialized");
        }
    }

    pub fn is_dict_ready(&self) -> bool {
        self.dict_store.is_some()
    }

    pub fn dict_store_ref(&self) -> Option<&DictStore> {
        self.dict_store.as_ref()
    }

    pub fn merge_candidates_for_reading(
        &self,
        hiragana: &str,
        llm_candidates: Vec<String>,
        limit: usize,
    ) -> Vec<String> {
        // 優先順位（Step 12-1、Issue #13 / #37）: 学習履歴 → ユーザー辞書 → システム辞書 → LLM
        // 学習履歴を先頭に置くので、ユーザー辞書の表記を別の候補で上書きできる。
        // LLM は末尾だが、辞書候補が上限まで埋めても LLM の枠は確保する（#42 の 4）。
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

        // 記号・絵文字は通常候補と別に引き、マージ後に位置を決める（Step 12-2、#42）
        let symbol_cands: Vec<String> = self
            .dict_store
            .as_ref()
            .map(|d| d.lookup_symbols(hiragana))
            .unwrap_or_default();
        let emoji_cands: Vec<String> = self
            .dict_store
            .as_ref()
            .map(|d| d.lookup_emoji(hiragana))
            .unwrap_or_default();

        debug!(
            "engine::merge: reading={:?} dict_store={} user_cands={:?} learn_cands={:?} dict_cands={:?} llm_cands={:?}",
            hiragana,
            if self.dict_store.is_some() {
                "Some"
            } else {
                "None"
            },
            user_cands,
            learn_cands,
            dict_cands,
            llm_candidates
        );

        let merged = merge_candidate_lists(
            &learn_cands,
            &user_cands,
            &dict_cands,
            llm_candidates,
            limit,
        );
        let mut merged = place_symbol_candidates(merged, symbol_cands, emoji_cands);

        // 候補不足時は元の読みを末尾に追加（変換せず確定する退避路）
        let desired_visible = self.config.num_candidates.min(limit);
        if merged.len() < desired_visible && !merged.iter().any(|c| c == hiragana) {
            merged.push(hiragana.to_string());
        }

        if merged.is_empty() {
            vec![hiragana.to_string()]
        } else {
            merged
        }
    }

    /// エンジン内部の `hiragana_buf` を reading として辞書をマージする。
    ///
    /// engine DLL の ABI シンボル（`engine_merge_candidates`）と単体テストからのみ使う。
    /// host / TSF からは `merge_candidates_for_reading` を使う（Issue #9）。
    pub fn merge_candidates(&self, llm_candidates: Vec<String>, limit: usize) -> Vec<String> {
        self.merge_candidates_for_reading(&self.hiragana_buf, llm_candidates, limit)
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
        let conv_reading = self.conv_reading();
        let committed = self.committed.clone();
        if hiragana.is_empty() {
            return false;
        }
        if !self.is_kanji_ready() {
            return false;
        }

        if let Some(conv) = self.kanji.take() {
            match conv_cache::start(
                hiragana,
                conv_reading,
                committed,
                conv,
                n_cands,
                self.config.digit_candidates_order.clone(),
                matches!(self.config.alpha_width, AlphaWidth::Fullwidth),
                matches!(self.config.symbol_width, SymbolWidth::Fullwidth),
            ) {
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
        self.log_clear();
    }

    pub fn reset_all(&mut self) {
        self.hiragana_buf.clear();
        self.committed.clear();
        self.romaji = RomajiConverter::new();
        self.pending_romaji_buf.clear();
        self.log_clear();
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
mod context_echo_risk_tests {
    use super::{RakunEngine, is_context_echo_risk};

    #[test]
    fn hiragana_only_text_is_echo_risk() {
        // 実機事例と同型: 未変換のまま確定されたひらがな文
        assert!(is_context_echo_risk("きだじゅんいちろうしは、"));
        // 短いひらがな確定（4 文字）も対象
        assert!(is_context_echo_risk("きもちは、"));
        // 句読点・括弧・空白は無視して判定
        assert!(is_context_echo_risk("「よろしくおねがいします。」"));
    }

    #[test]
    fn converted_or_short_text_is_not_echo_risk() {
        // 漢字を含む = 変換済み
        assert!(!is_context_echo_risk("木田純一郎氏は、"));
        // カタカナはエコーしても正しい出力になるため対象外
        assert!(!is_context_echo_risk("コーヒー"));
        // 英数字を含む
        assert!(!is_context_echo_risk("2024ねん"));
        // ひらがな 4 文字未満
        assert!(!is_context_echo_risk("です。"));
        assert!(!is_context_echo_risk(""));
    }

    #[test]
    fn commit_excludes_hiragana_only_text_from_context() {
        let mut e = RakunEngine::new(crate::EngineConfig::default());
        e.commit("今日は晴れ。");
        e.commit("きだじゅんいちろうしは、");
        // ひらがなのみの確定は context（committed）に入らない
        assert_eq!(e.committed_text(), "今日は晴れ。");
        e.commit("紀田順一郎氏は、");
        assert_eq!(e.committed_text(), "今日は晴れ。紀田順一郎氏は、");
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

    #[test]
    fn symbol_width_halfwidth_keeps_ascii() {
        let config = crate::EngineConfig {
            symbol_width: crate::SymbolWidth::Halfwidth,
            ..Default::default()
        };
        let mut e = RakunEngine::new(config);
        e.push_char('@');
        assert_eq!(e.hiragana_text(), "@");
    }

    #[test]
    fn alpha_width_halfwidth_keeps_ascii() {
        let config = crate::EngineConfig {
            alpha_width: crate::AlphaWidth::Halfwidth,
            ..Default::default()
        };
        let mut e = RakunEngine::new(config);
        e.push_fullwidth_alpha('U');
        e.push_fullwidth_alpha('S');
        e.push_fullwidth_alpha('B');
        assert_eq!(e.hiragana_text(), "USB");
    }

    #[test]
    fn alpha_width_fullwidth_converts() {
        let config = crate::EngineConfig {
            alpha_width: crate::AlphaWidth::Fullwidth,
            ..Default::default()
        };
        let mut e = RakunEngine::new(config);
        e.push_fullwidth_alpha('U');
        e.push_fullwidth_alpha('S');
        e.push_fullwidth_alpha('B');
        assert_eq!(e.hiragana_text(), "ＵＳＢ");
    }

    #[test]
    fn comma_after_alpha_with_fullwidth_uses_zenkaku_comma() {
        let config = crate::EngineConfig {
            alpha_width: crate::AlphaWidth::Fullwidth,
            ..Default::default()
        };
        let mut e = RakunEngine::new(config);
        e.push_fullwidth_alpha('A');
        e.push_char(',');
        assert_eq!(e.hiragana_text(), "Ａ，");
    }

    #[test]
    fn comma_after_alpha_with_halfwidth_uses_ascii_comma() {
        let config = crate::EngineConfig {
            alpha_width: crate::AlphaWidth::Halfwidth,
            ..Default::default()
        };
        let mut e = RakunEngine::new(config);
        e.push_fullwidth_alpha('A');
        e.push_char(',');
        assert_eq!(e.hiragana_text(), "A,");
    }

    #[test]
    fn period_after_symbol_with_fullwidth_uses_zenkaku_period() {
        let config = crate::EngineConfig {
            symbol_width: crate::SymbolWidth::Fullwidth,
            ..Default::default()
        };
        let mut e = RakunEngine::new(config);
        e.push_char('@');
        e.push_char('.');
        assert_eq!(e.hiragana_text(), "＠．");
    }

    #[test]
    fn period_after_symbol_with_halfwidth_uses_ascii_period() {
        let config = crate::EngineConfig {
            symbol_width: crate::SymbolWidth::Halfwidth,
            ..Default::default()
        };
        let mut e = RakunEngine::new(config);
        e.push_char('@');
        e.push_char('.');
        assert_eq!(e.hiragana_text(), "@.");
    }

    #[test]
    fn comma_after_kana_stays_touten() {
        // 直前が kana のときは従来通り `、` になる
        let config = crate::EngineConfig {
            alpha_width: crate::AlphaWidth::Fullwidth,
            symbol_width: crate::SymbolWidth::Fullwidth,
            ..Default::default()
        };
        let mut e = RakunEngine::new(config);
        e.force_preedit("あ".to_string());
        e.push_char(',');
        assert_eq!(e.hiragana_text(), "あ、");
    }
}

#[cfg(test)]
mod digit_width_tests {
    use super::{DigitCandidateKind, DigitWidth, EngineConfig, RakunEngine};

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

    #[test]
    fn engine_config_deserialize_uses_new_digit_defaults() {
        let cfg: EngineConfig = serde_json::from_str(r#"{"num_candidates":5}"#).unwrap();
        assert!(cfg.digit_separator_auto);
        assert_eq!(
            cfg.digit_candidates_order,
            vec![
                DigitCandidateKind::Arabic,
                DigitCandidateKind::Fullwidth,
                DigitCandidateKind::Positional,
                DigitCandidateKind::PerDigit,
                DigitCandidateKind::Daiji,
            ]
        );
    }
}

#[cfg(test)]
mod candidate_merge_tests {
    use super::{EngineConfig, RakunEngine};
    use rakukan_dict::DictStore;
    use std::fs;

    #[test]
    fn merge_candidates_pads_short_list_with_original_reading() {
        let mut engine = RakunEngine::new(EngineConfig {
            num_candidates: 9,
            ..Default::default()
        });
        engine.force_preedit("てすと".to_string());

        let llm_candidates = (1..=8).map(|n| format!("候補{n}")).collect();
        let merged = engine.merge_candidates(llm_candidates, 40);

        assert_eq!(merged.len(), 9);
        assert_eq!(merged.last().map(String::as_str), Some("てすと"));
    }

    #[test]
    fn merge_candidates_does_not_duplicate_original_reading() {
        let mut engine = RakunEngine::new(EngineConfig {
            num_candidates: 9,
            ..Default::default()
        });
        engine.force_preedit("てすと".to_string());

        let mut llm_candidates: Vec<String> = (1..=7).map(|n| format!("候補{n}")).collect();
        llm_candidates.push("てすと".to_string());
        let merged = engine.merge_candidates(llm_candidates, 40);

        assert_eq!(merged.iter().filter(|c| c.as_str() == "てすと").count(), 1);
    }

    #[test]
    fn learned_surface_outranks_user_dict_entry() {
        // Issue #13: ユーザー辞書「杜野」を登録した読みで「森の」を選んで確定すると、
        // 次回は学習した「森の」が先頭になる（旧順序では永久に「杜野」が先頭だった）。
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("user_dict.toml");
        fs::write(
            &user_path,
            r#"
[[entries]]
reading = "もりの"
surfaces = ["杜野"]
"#,
        )
        .unwrap();
        let store = DictStore::load(Some(&user_path), None, None).unwrap();
        let mut engine = RakunEngine::new(EngineConfig {
            num_candidates: 9,
            ..Default::default()
        });
        engine.set_dict_store(store);

        // 登録直後（学習なし）はユーザー辞書が先頭
        let before = engine.merge_candidates_for_reading("もりの", vec!["森の".into()], 40);
        assert_eq!(before.first().map(String::as_str), Some("杜野"));

        engine.learn_force("もりの", "森の");
        let after = engine.merge_candidates_for_reading("もりの", vec!["森の".into()], 40);
        assert_eq!(after.first().map(String::as_str), Some("森の"));
        assert_eq!(after.get(1).map(String::as_str), Some("杜野"));
    }

    #[test]
    fn merge_lists_orders_learn_user_dict_llm() {
        let merged = super::merge_candidate_lists(
            &["学".into()],
            &["ユ".into()],
            &["辞1".into(), "辞2".into()],
            vec!["L1".into()],
            40,
        );
        assert_eq!(merged, ["学", "ユ", "辞1", "辞2", "L1"]);
    }

    #[test]
    fn merge_lists_keeps_slots_for_llm_when_dict_fills_limit() {
        // Issue #42 の 4: 辞書候補が上限まであっても LLM 候補が落ちない
        let dict: Vec<String> = (1..=40).map(|n| format!("辞{n}")).collect();
        let merged =
            super::merge_candidate_lists(&[], &[], &dict, vec!["L1".into(), "L2".into()], 40);
        assert_eq!(merged.len(), 40);
        assert_eq!(&merged[38..], ["L1", "L2"]);
        assert!(!merged.iter().any(|c| c == "辞39"));
    }

    #[test]
    fn merge_lists_llm_duplicate_of_dict_stays_at_dict_position() {
        let dict: Vec<String> = (1..=40).map(|n| format!("辞{n}")).collect();
        let merged =
            super::merge_candidate_lists(&[], &[], &dict, vec!["辞3".into(), "L1".into()], 40);
        assert_eq!(merged.len(), 40);
        assert_eq!(merged[2], "辞3");
        assert_eq!(merged.iter().filter(|c| c.as_str() == "辞3").count(), 1);
        // LLM の重複ぶんは辞書の残りで埋め戻す（L1 の後ろに 辞39）
        assert_eq!(&merged[38..], ["L1", "辞39"]);
    }

    #[test]
    fn merge_lists_llm_fills_remaining_when_dict_is_short() {
        let merged = super::merge_candidate_lists(
            &[],
            &[],
            &["辞1".into()],
            vec!["L1".into(), "L2".into(), "L1".into()],
            3,
        );
        assert_eq!(merged, ["辞1", "L1", "L2"]);
    }

    #[test]
    fn merge_lists_learn_and_user_never_exceed_limit() {
        let learn: Vec<String> = (1..=5).map(|n| format!("学{n}")).collect();
        let user: Vec<String> = (1..=5).map(|n| format!("ユ{n}")).collect();
        let merged = super::merge_candidate_lists(&learn, &user, &[], vec!["L1".into()], 4);
        assert_eq!(merged, ["学1", "学2", "学3", "学4"]);
    }

    fn syms(prefix: &str, n: usize) -> Vec<String> {
        (0..n).map(|i| format!("{prefix}{i}")).collect()
    }

    #[test]
    fn symbols_are_inserted_after_top_three_and_rest_go_last() {
        let base: Vec<String> = ["右", "みぎ", "ミギ", "語3", "語4"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = super::place_symbol_candidates(base, syms("記", 20), syms("絵", 2));
        let expect: Vec<String> = ["右", "みぎ", "ミギ"]
            .iter()
            .map(|s| s.to_string())
            .chain(syms("記", 15))
            .chain(["語3".to_string(), "語4".to_string()])
            .chain((15..20).map(|i| format!("記{i}")))
            .chain(syms("絵", 2))
            .collect();
        assert_eq!(out, expect);
    }

    #[test]
    fn symbols_follow_short_base_and_dedup_against_it() {
        let base: Vec<String> = vec!["→".into(), "右".into()];
        let out = super::place_symbol_candidates(
            base,
            vec!["→".into(), "⇒".into(), "⇒".into()],
            vec!["👉".into(), "右".into()],
        );
        assert_eq!(out, ["→", "右", "⇒", "👉"]);
    }

    #[test]
    fn symbols_only_reading_lists_symbols_in_order() {
        let out = super::place_symbol_candidates(vec![], syms("記", 17), vec![]);
        assert_eq!(out, syms("記", 17));
    }

    #[test]
    fn no_symbols_returns_base_unchanged() {
        let base: Vec<String> = vec!["右".into()];
        assert_eq!(
            super::place_symbol_candidates(base.clone(), vec![], vec![]),
            base
        );
    }

    #[test]
    fn merge_candidates_uses_user_dict_even_without_llm_candidates() {
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("user_dict.toml");
        fs::write(
            &user_path,
            r#"
[[entries]]
reading = "かっことじ"
surfaces = ["』"]
"#,
        )
        .unwrap();

        let store = DictStore::load(Some(&user_path), None, None).unwrap();
        let mut engine = RakunEngine::new(EngineConfig {
            num_candidates: 9,
            ..Default::default()
        });
        engine.set_dict_store(store);
        engine.force_preedit("かっことじ".to_string());

        let merged = engine.merge_candidates(vec![], 40);

        assert_eq!(merged.first().map(String::as_str), Some("』"));
        assert!(merged.iter().any(|candidate| candidate == "かっことじ"));
    }

    #[test]
    fn merge_candidates_for_reading_uses_given_reading_not_internal_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("user_dict.toml");
        fs::write(
            &user_path,
            r#"
[[entries]]
reading = "かっことじ"
surfaces = ["』"]
"#,
        )
        .unwrap();

        let store = DictStore::load(Some(&user_path), None, None).unwrap();
        let mut engine = RakunEngine::new(EngineConfig {
            num_candidates: 9,
            ..Default::default()
        });
        engine.set_dict_store(store);
        engine.force_preedit("べつのよみ".to_string());

        let merged = engine.merge_candidates_for_reading("かっことじ", vec![], 40);

        assert_eq!(merged.first().map(String::as_str), Some("』"));
        assert!(merged.iter().any(|candidate| candidate == "かっことじ"));
        assert!(!merged.iter().any(|candidate| candidate == "べつのよみ"));
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

#[cfg(test)]
mod input_log_tests {
    //! Step 10-1: 入力ログの種別化と復元。
    //!
    //! 不変条件 1: `romaji_log_str() + pending == 打鍵列`
    //! 不変条件 2: `hiragana_from_romaji_log() == hiragana_text()`（Backspace / force_preedit を除く経路）
    use super::{AlphaWidth, DigitWidth, EngineConfig, RakunEngine};

    /// 依存を増やさないための決定的な擬似乱数（LCG）
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 33
        }

        fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
            &xs[(self.next() as usize) % xs.len()]
        }
    }

    #[derive(Clone, Copy)]
    enum Op {
        /// キーボード英数字・記号（`push_char`）
        Char(char),
        /// TSF が全角化して渡す記号（`push_raw`）
        Raw(char),
        /// Shift+英字（`push_fullwidth_alpha`）
        Shift(char),
        /// Space / Enter 直前の末尾 n 確定
        FlushN,
        /// Backspace（pending があるときだけ打鍵列から 1 文字消す。空なら何もしない）
        BackspacePending,
    }

    fn apply(e: &mut RakunEngine, op: Op, typed: &mut String) {
        apply_opts(e, op, typed, false);
    }

    /// `full_backspace` が true なら pending が空でも Backspace を押す
    /// （打鍵列は追跡しない。不変条件 2 の検査用）。
    fn apply_opts(e: &mut RakunEngine, op: Op, typed: &mut String, full_backspace: bool) {
        match op {
            Op::Char(c) => {
                e.push_char(c);
                typed.push(c);
            }
            Op::Raw(c) => {
                e.push_raw(c);
                typed.push(c);
            }
            Op::Shift(c) => {
                e.push_fullwidth_alpha(c);
                typed.push(c);
            }
            Op::FlushN => {
                e.flush_pending_n();
            }
            Op::BackspacePending => {
                if !e.current_preedit().pending_romaji.is_empty() {
                    assert!(e.backspace());
                    typed.pop();
                } else if full_backspace {
                    let _ = e.backspace();
                }
            }
        }
    }

    fn random_op(rng: &mut Lcg) -> Op {
        const CHARS: &[char] = &[
            'a', 'i', 'u', 'e', 'o', 'k', 's', 't', 'n', 'h', 'm', 'y', 'r', 'w', 'g', 'z', 'j',
            'd', 'b', 'p', 'c', 'x', 'l', 'q', 'v', 'f', '0', '1', '9', ',', '.', '-', '\'', '!',
            '?', '@',
        ];
        const RAWS: &[char] = &['、', '。', '・', 'ー', '！', ',', '.', '-'];
        const SHIFTS: &[char] = &['A', 'B', 'N', 'Z'];
        match rng.next() % 12 {
            0 => Op::Raw(*rng.pick(RAWS)),
            1 => Op::Shift(*rng.pick(SHIFTS)),
            2 => Op::FlushN,
            3 | 4 => Op::BackspacePending,
            _ => Op::Char(*rng.pick(CHARS)),
        }
    }

    fn configs() -> Vec<EngineConfig> {
        vec![
            EngineConfig::default(),
            EngineConfig {
                digit_width: DigitWidth::Fullwidth,
                alpha_width: AlphaWidth::Halfwidth,
                digit_separator_auto: false,
                ..Default::default()
            },
            EngineConfig {
                digit_width: DigitWidth::Halfwidth,
                alpha_width: AlphaWidth::Fullwidth,
                digit_separator_auto: true,
                ..Default::default()
            },
        ]
    }

    #[test]
    fn property_typed_concat_plus_pending_equals_keystrokes() {
        for (ci, config) in configs().into_iter().enumerate() {
            let mut rng = Lcg(0x5EED_0000 + ci as u64);
            for case in 0..200 {
                let mut e = RakunEngine::new(config.clone());
                let mut typed = String::new();
                let len = 1 + (rng.next() % 12) as usize;
                for _ in 0..len {
                    apply(&mut e, random_op(&mut rng), &mut typed);
                    let pending = e.current_preedit().pending_romaji.clone();
                    assert_eq!(
                        format!("{}{}", e.romaji_log_str(), pending),
                        typed,
                        "config {ci} case {case}: log + pending != typed"
                    );
                }
            }
        }
    }

    #[test]
    fn property_output_concat_equals_hiragana_buf() {
        for (ci, config) in configs().into_iter().enumerate() {
            let mut rng = Lcg(0x0B5E_0000 + ci as u64);
            for case in 0..200 {
                let mut e = RakunEngine::new(config.clone());
                let mut typed = String::new();
                let len = 1 + (rng.next() % 12) as usize;
                for _ in 0..len {
                    apply_opts(&mut e, random_op(&mut rng), &mut typed, true);
                    assert_eq!(
                        e.hiragana_from_romaji_log(),
                        e.hiragana_text(),
                        "config {ci} case {case}: typed={typed:?}"
                    );
                }
            }
        }
    }

    fn type_all(e: &mut RakunEngine, s: &str) {
        for c in s.chars() {
            e.push_char(c);
        }
    }

    #[test]
    fn shift_alpha_restores_with_alpha_width_fullwidth() {
        let mut e = RakunEngine::new(EngineConfig {
            alpha_width: AlphaWidth::Fullwidth,
            ..Default::default()
        });
        type_all(&mut e, "ru-mu");
        e.push_fullwidth_alpha('A');
        type_all(&mut e, "situ");
        assert_eq!(e.hiragana_text(), "るーむＡしつ");
        assert_eq!(e.romaji_log_str(), "ru-muAsitu");
        assert_eq!(e.hiragana_from_romaji_log(), "るーむＡしつ");
    }

    #[test]
    fn shift_alpha_restores_with_alpha_width_halfwidth() {
        let mut e = RakunEngine::new(EngineConfig {
            alpha_width: AlphaWidth::Halfwidth,
            ..Default::default()
        });
        type_all(&mut e, "ka");
        e.push_fullwidth_alpha('B');
        type_all(&mut e, "i");
        assert_eq!(e.hiragana_text(), "かBい");
        assert_eq!(e.hiragana_from_romaji_log(), "かBい");
    }

    #[test]
    fn digit_separator_restores_as_recorded() {
        let mut e = RakunEngine::new(EngineConfig {
            digit_width: DigitWidth::Fullwidth,
            digit_separator_auto: true,
            ..Default::default()
        });
        type_all(&mut e, "1,000.0");
        assert_eq!(e.hiragana_text(), "１,０００.０");
        assert_eq!(e.hiragana_from_romaji_log(), "１,０００.０");
    }

    #[test]
    fn raw_kuten_after_digit_restores_as_recorded() {
        // TSF の on_punctuate は digit_separator_auto が無効なら「、」「。」を push_raw で渡す
        let mut e = RakunEngine::new(EngineConfig {
            digit_width: DigitWidth::Fullwidth,
            digit_separator_auto: false,
            ..Default::default()
        });
        e.push_char('1');
        e.push_raw('、');
        type_all(&mut e, "000");
        e.push_raw('。');
        e.push_char('0');
        assert_eq!(e.hiragana_text(), "１、０００。０");
        assert_eq!(e.hiragana_from_romaji_log(), "１、０００。０");
    }

    #[test]
    fn flushed_n_restores_as_nn() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "kon");
        assert!(e.flush_pending_n());
        assert_eq!(e.hiragana_text(), "こん");
        assert_eq!(e.romaji_log_str(), "kon");
        assert_eq!(e.hiragana_from_romaji_log(), "こん");
    }

    #[test]
    fn kuten_via_raw_restores_unchanged() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "kareha");
        e.push_raw('、');
        type_all(&mut e, "otokoda");
        e.push_raw('。');
        assert_eq!(e.hiragana_text(), "かれは、おとこだ。");
        assert_eq!(e.hiragana_from_romaji_log(), "かれは、おとこだ。");
    }

    #[test]
    fn restore_is_empty_when_nothing_typed() {
        let e = RakunEngine::new(EngineConfig::default());
        assert_eq!(e.hiragana_from_romaji_log(), "");
        assert_eq!(e.romaji_log_str(), "");
    }
}

#[cfg(test)]
mod close_pending_tests {
    //! Step 10-2: 未確定ローマ字を追い越す入力の防止。
    //!
    //! `push_raw` / `push_fullwidth_alpha` / 数字・記号の `push_char` は、先に pending を
    //! 閉じてから追記する（`flush_pending_n` → `flush` → 追記）。
    use super::{AlphaWidth, DigitWidth, EngineConfig, RakunEngine};

    fn engine() -> RakunEngine {
        RakunEngine::new(EngineConfig {
            alpha_width: AlphaWidth::Fullwidth,
            digit_width: DigitWidth::Fullwidth,
            ..Default::default()
        })
    }

    fn type_all(e: &mut RakunEngine, s: &str) {
        for c in s.chars() {
            e.push_char(c);
        }
    }

    #[test]
    fn raw_symbol_after_consonant_keeps_order() {
        let mut e = engine();
        e.push_char('k');
        e.push_raw('。');
        assert_eq!(e.current_preedit().display(), "k。");
        assert_eq!(e.hiragana_text(), "k。");
        assert!(e.current_preedit().pending_romaji.is_empty());
        assert_eq!(e.romaji_log_str(), "k。");
    }

    #[test]
    fn raw_symbol_after_j_keeps_order() {
        let mut e = engine();
        e.push_char('j');
        e.push_raw('。');
        assert_eq!(e.current_preedit().display(), "j。");
    }

    #[test]
    fn raw_symbol_after_two_consonants_keeps_order() {
        let mut e = engine();
        type_all(&mut e, "ky");
        e.push_raw('。');
        assert_eq!(e.current_preedit().display(), "ky。");
    }

    #[test]
    fn raw_symbol_after_n_makes_nn() {
        let mut e = engine();
        e.push_char('n');
        e.push_raw('。');
        assert_eq!(e.current_preedit().display(), "ん。");
        assert_eq!(e.romaji_log_str(), "n。");
        assert_eq!(e.hiragana_from_romaji_log(), "ん。");
    }

    #[test]
    fn raw_symbol_after_z_has_no_leader_exception() {
        // §5-5 は未決。例外を入れない前提で「z、」に固定する
        let mut e = engine();
        e.push_char('z');
        e.push_raw('、');
        assert_eq!(e.current_preedit().display(), "z、");
    }

    #[test]
    fn shift_alpha_after_consonant_keeps_order() {
        let mut e = engine();
        e.push_char('k');
        e.push_fullwidth_alpha('A');
        assert_eq!(e.current_preedit().display(), "kＡ");
        assert_eq!(e.romaji_log_str(), "kA");
    }

    #[test]
    fn shift_alpha_after_n_makes_nn() {
        let mut e = engine();
        e.push_char('n');
        e.push_fullwidth_alpha('A');
        assert_eq!(e.current_preedit().display(), "んＡ");
    }

    #[test]
    fn digit_after_consonant_follows_digit_width() {
        let mut e = engine();
        e.push_char('k');
        e.push_char('1');
        assert_eq!(e.current_preedit().display(), "k１");
        assert_eq!(e.romaji_log_str(), "k1");
    }

    #[test]
    fn digit_after_n_follows_digit_width() {
        let mut e = engine();
        e.push_char('n');
        e.push_char('1');
        assert_eq!(e.current_preedit().display(), "ん１");
    }

    #[test]
    fn ascii_symbol_after_consonant_follows_symbol_width() {
        let mut e = engine();
        e.push_char('k');
        e.push_char('@');
        assert_eq!(e.current_preedit().display(), "k＠");
    }

    #[test]
    fn trie_symbol_after_consonant_still_goes_through_trie() {
        // `,./[]\-` は trie に委ねたまま（`z,` 系の例外を将来入れられるよう）
        let mut e = engine();
        e.push_char('k');
        e.push_char('-');
        assert_eq!(e.current_preedit().display(), "kー");
    }

    #[test]
    fn closed_consonant_stays_confirmed_after_backspace() {
        let mut e = engine();
        e.push_char('k');
        e.push_raw('。');
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "k");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "kあ");
    }

    #[test]
    fn vowel_after_symbol_does_not_merge_with_earlier_consonant() {
        let mut e = engine();
        e.push_char('k');
        e.push_raw('。');
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "k。あ");
    }

    #[test]
    fn closing_keeps_both_invariants() {
        let mut e = engine();
        type_all(&mut e, "tat");
        e.push_raw('。');
        e.push_char('n');
        e.push_fullwidth_alpha('B');
        e.push_char('k');
        e.push_char('9');
        let pending = e.current_preedit().pending_romaji.clone();
        assert_eq!(format!("{}{}", e.romaji_log_str(), pending), "tat。nBk9");
        assert_eq!(e.hiragana_from_romaji_log(), e.hiragana_text());
        assert_eq!(e.hiragana_text(), "たt。んＢk９");
    }

    #[test]
    fn empty_pending_is_unchanged() {
        let mut e = engine();
        type_all(&mut e, "ka");
        e.push_raw('。');
        assert_eq!(e.current_preedit().display(), "か。");
        e.push_fullwidth_alpha('A');
        assert_eq!(e.current_preedit().display(), "か。Ａ");
        e.push_char('1');
        assert_eq!(e.current_preedit().display(), "か。Ａ１");
    }
}

#[cfg(test)]
mod backspace_replay_tests {
    //! Step 10-3: pending があるときの Backspace は、末尾のローマ字区間を打鍵列から再生する。
    use super::{EngineConfig, RakunEngine};

    fn type_all(e: &mut RakunEngine, s: &str) {
        for c in s.chars() {
            e.push_char(c);
        }
    }

    fn typed_then_bs(s: &str) -> RakunEngine {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, s);
        assert!(e.backspace());
        e
    }

    #[test]
    fn kt_bs_leaves_k_pending() {
        let e = typed_then_bs("kt");
        assert_eq!(e.hiragana_text(), "");
        assert_eq!(e.current_preedit().pending_romaji, "k");
        assert_eq!(e.current_preedit().display(), "k");
        assert_eq!(e.romaji_log_str(), "");
    }

    #[test]
    fn kt_bs_a_is_ka() {
        let mut e = typed_then_bs("kt");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "か");
        assert_eq!(e.romaji_log_str(), "ka");
    }

    #[test]
    fn nt_bs_a_is_na() {
        let mut e = typed_then_bs("nt");
        assert_eq!(e.current_preedit().display(), "n");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "な");
    }

    #[test]
    fn tt_bs_a_is_ta() {
        let mut e = typed_then_bs("tt");
        assert_eq!(e.current_preedit().display(), "t");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "た");
    }

    #[test]
    fn kk_bs_a_is_ka() {
        let mut e = typed_then_bs("kk");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "か");
    }

    #[test]
    fn kyt_bs_a_is_kya() {
        let mut e = typed_then_bs("kyt");
        assert_eq!(e.current_preedit().display(), "ky");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "きゃ");
    }

    #[test]
    fn sht_bs_i_is_shi() {
        let mut e = typed_then_bs("sht");
        e.push_char('i');
        assert_eq!(e.current_preedit().display(), "し");
        assert_eq!(e.romaji_log_str(), "shi");
    }

    #[test]
    fn kanakq_bs_keeps_display_and_makes_k_pending() {
        let mut e = typed_then_bs("kanakq");
        assert_eq!(e.current_preedit().display(), "かなk");
        assert_eq!(e.hiragana_text(), "かな");
        assert_eq!(e.current_preedit().pending_romaji, "k");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "かなか");
    }

    #[test]
    fn replay_stops_at_flushed_n() {
        // Space → Waiting → Esc で Preedit に戻った後の入力を想定
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "kon");
        assert!(e.flush_pending_n());
        e.push_char('k');
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "こん");
        assert!(e.current_preedit().pending_romaji.is_empty());
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "こんあ");
    }

    #[test]
    fn replay_stops_at_raw_entry() {
        let mut e = RakunEngine::new(EngineConfig::default());
        e.push_char('k');
        e.push_raw('。');
        e.push_char('k');
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "k。");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "k。あ");
    }

    #[test]
    fn replay_stops_at_digit_entry() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "ka1kt");
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "か1k");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "か1か");
    }

    #[test]
    fn after_force_preedit_backspace_is_simple_pop() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "ka");
        e.force_preedit("カ".to_string());
        e.push_char('k');
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "カ");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "カあ");
        // 境界より前は単純 pop
        assert!(e.backspace());
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "");
        assert!(!e.backspace());
    }

    #[test]
    fn replay_after_force_preedit_covers_only_new_input() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "ka");
        e.force_preedit("カ".to_string());
        type_all(&mut e, "kt");
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "カk");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "カか");
    }

    #[test]
    fn multiple_backspaces_walk_back_through_typed() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "kanakt");
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "かなk");
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "かな");
        assert!(e.current_preedit().pending_romaji.is_empty());
        // pending が空になったら現行の 1 文字削除
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "か");
    }

    #[test]
    fn empty_pending_backspace_is_unchanged() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "kya");
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "き");
    }

    #[test]
    fn invariants_hold_after_pending_backspace() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "watasit");
        assert!(e.backspace());
        let pending = e.current_preedit().pending_romaji.clone();
        assert_eq!(format!("{}{}", e.romaji_log_str(), pending), "watasi");
        assert_eq!(e.hiragana_from_romaji_log(), e.hiragana_text());
        assert_eq!(e.hiragana_text(), "わたし");
    }
}

#[cfg(test)]
mod log_rewrite_tests {
    //! Step 10-4: pending が空のときの Backspace は表示 1 文字を消し、log を残る表示に
    //! 合う打鍵列に書き換える（§4.3 の 2 段: 接頭辞の再生 → 逆引き。母音は母音字 1 文字）。
    use super::{EngineConfig, RakunEngine};

    fn type_all(e: &mut RakunEngine, s: &str) {
        for c in s.chars() {
            e.push_char(c);
        }
    }

    /// 打鍵 → Backspace 1 回。(表示, F9 用の打鍵列) を返す
    fn bs(s: &str) -> (String, String) {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, s);
        assert!(e.backspace());
        assert_eq!(e.hiragana_from_romaji_log(), e.hiragana_text());
        (e.current_preedit().display(), e.romaji_log_str())
    }

    #[test]
    fn youon_falls_back_to_reverse_lookup() {
        assert_eq!(bs("kya"), ("き".into(), "ki".into()));
        assert_eq!(bs("gyo"), ("ぎ".into(), "gi".into()));
        assert_eq!(bs("nyu"), ("に".into(), "ni".into()));
    }

    #[test]
    fn reverse_lookup_prefers_shared_prefix() {
        assert_eq!(bs("sha"), ("し".into(), "shi".into()));
        assert_eq!(bs("sya"), ("し".into(), "si".into()));
        assert_eq!(bs("cha"), ("ち".into(), "chi".into()));
        assert_eq!(bs("tya"), ("ち".into(), "ti".into()));
        assert_eq!(bs("ja"), ("じ".into(), "ji".into()));
        assert_eq!(bs("zya"), ("じ".into(), "zi".into()));
        assert_eq!(bs("fa"), ("ふ".into(), "fu".into()));
        assert_eq!(bs("hwa"), ("ふ".into(), "hu".into()));
        assert_eq!(bs("tsa"), ("つ".into(), "tsu".into()));
    }

    #[test]
    fn reverse_lookup_uses_bare_vowel_for_vowel_kana() {
        assert_eq!(bs("wi"), ("う".into(), "u".into()));
        assert_eq!(bs("who"), ("う".into(), "u".into()));
        assert_eq!(bs("ye"), ("い".into(), "i".into()));
    }

    #[test]
    fn prefix_replay_keeps_typed_spelling() {
        // 促音: 逆引きだと xtu になるが、打鍵の接頭辞 tt で「っ」が出る
        assert_eq!(bs("tta"), ("っ".into(), "tt".into()));
        // 撥音: nna は 1 エントリ（んあ）。nn で「ん」
        assert_eq!(bs("nna"), ("ん".into(), "nn".into()));
        // n + 子音の「ん」も打鍵の接頭辞
        assert_eq!(bs("nta"), ("ん".into(), "nt".into()));
        // 複合エントリ（んにゃ）から 1 文字消す
        assert_eq!(bs("nnnya"), ("んに".into(), "nnni".into()));
    }

    #[test]
    fn exact_prefix_is_preferred_over_prefix_with_pending() {
        // kata → BS: kat（か + 未確定 t）ではなく ka
        assert_eq!(bs("kata"), ("か".into(), "ka".into()));
        assert_eq!(bs("kana"), ("か".into(), "ka".into()));
    }

    #[test]
    fn single_char_entries_are_popped_as_before() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "ka");
        e.push_raw('。');
        e.push_fullwidth_alpha('A');
        e.push_char('1');
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "か。Ａ");
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "か。");
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "か");
        assert_eq!(e.romaji_log_str(), "ka");
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "");
        assert!(!e.backspace());
    }

    #[test]
    fn rewritten_entry_is_closed_so_replay_does_not_reopen_it() {
        // 10-3 で入った崩れ: った → BS → っ → k → BS が t になっていた
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "tta");
        assert!(e.backspace());
        e.push_char('k');
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "っ");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "っあ");
    }

    #[test]
    fn popped_run_tail_is_closed_too() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "nta");
        assert!(e.backspace());
        e.push_char('k');
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "ん");
    }

    #[test]
    fn replay_after_rewrite_starts_after_closed_entry() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "kya");
        assert!(e.backspace());
        type_all(&mut e, "kt");
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "きk");
        e.push_char('a');
        assert_eq!(e.current_preedit().display(), "きか");
        assert_eq!(e.romaji_log_str(), "kika");
    }

    #[test]
    fn rewrite_restores_through_f6_path() {
        // F9 → F6 相当: hiragana_from_romaji_log が書き換え後の表示を返す
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "kyakya");
        assert!(e.backspace());
        assert_eq!(e.hiragana_text(), "きゃき");
        assert_eq!(e.hiragana_from_romaji_log(), "きゃき");
        assert_eq!(e.romaji_log_str(), "kyaki");
    }

    #[test]
    fn detached_backspace_is_simple_pop() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "kya");
        e.force_preedit("キャ".to_string());
        assert!(e.backspace());
        assert_eq!(e.current_preedit().display(), "キ");
        // detach 後は log と表示が対応しないので、現行どおり 1 エントリ消す
        assert_eq!(e.romaji_log_str(), "");
    }

    #[test]
    fn consecutive_backspaces_keep_invariant_two() {
        let mut e = RakunEngine::new(EngineConfig::default());
        type_all(&mut e, "kyounohanashashin");
        while e.backspace() {
            assert_eq!(e.hiragana_from_romaji_log(), e.hiragana_text());
        }
        assert_eq!(e.current_preedit().display(), "");
    }
}
