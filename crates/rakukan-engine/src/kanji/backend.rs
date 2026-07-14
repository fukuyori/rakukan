//! Backend interface for kanji conversion using llama.cpp

use super::error::KanjiError;
use super::hf_download::{get_tokenizer_path, get_variant_path};
use super::llamacpp::LlamaCppModel;
use super::model_config::{ModelFamily, VariantConfig, registry};
use super::{CONTEXT_TOKEN, INPUT_START_TOKEN, OUTPUT_START_TOKEN};
use crate::kana::{hiragana_to_katakana, katakana_to_hiragana};

type Result<T> = super::error::Result<T>;

/// Configuration for kanji conversion
#[derive(Debug, Clone)]
pub struct ConversionConfig {
    /// Maximum number of new tokens to generate
    pub max_new_tokens: usize,
    /// Space 変換時のビーム幅の**上限**（num_candidates と併せて min をとる）。
    /// デフォルト 30 では実質無制限で、num_candidates がそのまま beam 幅になる。
    /// 変換速度を抑えたいユーザは小さく設定する（例: 3）。ランタイムで [1, 30]。
    pub beam_size: usize,
    /// 異常変換の棄却に使う「最良候補からの平均 log-prob 差」の許容幅 (nats/token)。
    /// beam 候補は長さ正規化した平均 log-prob (1 トークンあたりの自信度) で評価し、
    /// 最良候補より `margin` 以上低い候補は外れ値として捨てる。`None` で無効。
    /// 既定 3.0 は寛容（最良候補比で 1 トークンあたり e^3≈20 倍も不確かな候補だけを
    /// 落とす）で、通常の代替候補には影響しない。値を小さくすると棄却が強まる。
    pub confidence_margin: Option<f32>,
    /// 最良候補の平均 log-prob (nats/token) の絶対下限。最良候補すらこれを下回る変換は
    /// 幻覚の可能性が高いため全候補を捨て、かな（元の読み）にフォールバックする。
    /// 適切な閾値は実地のスコア分布に依存するため既定 `None`（無効）。有効化する場合は
    /// まず `confidence_margin` のデバッグログで実際の平均 log-prob を観測してから設定する。
    pub min_top_confidence: Option<f32>,
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            max_new_tokens: 15,
            beam_size: 30,
            confidence_margin: Some(3.0),
            min_top_confidence: None,
        }
    }
}

fn generation_budget(reading: &str, config_max_new_tokens: usize) -> usize {
    let reading_chars = reading.chars().count();
    // 長めの文でも途中で切れにくいよう、固定値ではなく読み長に応じて伸ばす。
    // jinen 系では 1 文字あたり 1 token 未満になることもあるが、かなり長い文では
    // 15 token では不足しやすいため、余裕を持って 2 倍 + 8 を上限付きで使う。
    // M1.5 T-BUG1 (a): 上限を 128 → 256 に引き上げ。20 文字超の長文 reading で
    // budget が頭打ちになる前に EOS が出るパターン (尻切れ) を抑制する。
    // KV cache は変換時のみ確保するためメモリ圧は無視できる。
    config_max_new_tokens
        .max(reading_chars.saturating_mul(2).saturating_add(8))
        .min(256)
}

/// 反復検出の最小周期（文字数）。「ますます」「きらきら」のような正当な畳語は
/// 周期 2〜3 なので、4 以上の周期のみを退化とみなす。
const REPEAT_MIN_PERIOD: usize = 4;

/// エコー源マスキングを適用する読みの最小文字数。「は」「が」など短い読みで
/// context を切り捨てると誤爆だらけになるため、短い読みには適用しない。
const ECHO_MIN_READING_CHARS: usize = 4;

/// エコー源検索に使う読みプレフィックスの長さ（文字数）。長めに取るほど
/// 「というこ」等の一般的なかな列との偶然一致（正当な context の切り捨て）が減る。
const ECHO_NEEDLE_CHARS: usize = 6;

/// context 汚染（エコーアトラクタ）対策: 読みの先頭かな列と一致する部分が
/// context 内にあれば、その最初の出現位置で context を切り捨てて返す。
///
/// 未変換のまま確定されたテキスト（例:「きだじゅんいちろう氏は、」）が context に
/// 残っていると、小型 LLM は変換ではなく context からのコピー（エコー）を選び、
/// 全ビームがエコー系に収束して漢字候補が消える。漢字に変換済みの文はかな列として
/// 一致しないため削られない。カタカナ確定（F7 等）由来の汚染も検出する。
///
/// 純粋関数。llama 非依存で単体テスト可能。
fn strip_echo_context<'a>(context: &'a str, reading: &str) -> &'a str {
    if context.is_empty() {
        return context;
    }
    let reading_chars = reading.chars().count();
    if reading_chars < ECHO_MIN_READING_CHARS {
        return context;
    }
    let needle: String = reading.chars().take(ECHO_NEEDLE_CHARS).collect();
    let kata_needle = hiragana_to_katakana(&needle);
    let pos = match (context.find(&needle), context.find(&kata_needle)) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    match pos {
        Some(p) => &context[..p],
        None => context,
    }
}

/// かなのみで読みの真のプレフィックス（読み全体は除く）になっている候補を検出する。
///
/// 「きだじゅん」「キダジュン」のような途中で切れたエコー候補は、読み全体を
/// カバーしない未変換断片であり正当な変換結果ではありえないため棄却する。
/// 読み全体と一致する候補（無変換・カタカナ変換のフォールバック）は残す。
fn is_kana_prefix_echo(candidate: &str, reading: &str) -> bool {
    if candidate.is_empty() {
        return false;
    }
    let hira = katakana_to_hiragana(candidate);
    if hira == reading {
        return false;
    }
    if !hira.chars().all(is_kana_or_prolonged) {
        return false;
    }
    reading.starts_with(&hira)
}

/// 候補長の上限安全網。かな→漢字変換で文字数は通常縮むため、読みの
/// 1.5 倍 + 2 を超える候補は反復生成などの異常出力とみなして棄却する。
/// （下限 33% の安全網と対になる。同じ文が 2 度続く候補は長さ約 2 倍に
/// なるため、この上限で捕捉できる。）
fn max_candidate_chars(reading_chars: usize) -> usize {
    reading_chars.saturating_mul(3) / 2 + 2
}

/// 候補内のタンデム反復（同一部分列 X が「XX」と連続する、|X| >= min_period）を
/// 検出する。小型 LLM が EOS を出せず同じ句を繰り返す退化パターンの検出用。
///
/// 純粋関数。呼び出し側は「読み自身が反復を含む場合」（ユーザーが実際に
/// 繰り返し表現を入力したケース）には適用しないこと。
fn has_tandem_repeat(text: &str, min_period: usize) -> bool {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    for p in min_period..=n / 2 {
        for i in 0..=(n - 2 * p) {
            if chars[i..i + p] == chars[i + p..i + 2 * p] {
                return true;
            }
        }
    }
    false
}

/// 長さ上限・反復検出による異常候補の棄却（症状: 途中切れ・同文 2 度出力）。
///
/// 読み自身にタンデム反復が含まれる場合は反復検出を無効化する
/// （「わかったわかった」のような入力を正当に変換できるようにする）。
fn is_degenerate_candidate(candidate: &str, reading_chars: usize, reading_repeats: bool) -> bool {
    let len = candidate.chars().count();
    if len > max_candidate_chars(reading_chars) {
        return true;
    }
    if !reading_repeats && has_tandem_repeat(candidate, REPEAT_MIN_PERIOD) {
        return true;
    }
    false
}

/// beam の累積 log-prob (score) を「1 トークンあたりの平均 log-prob」に正規化する。
///
/// `score` は生成トークンの log-softmax の総和 (≤ 0) で、系列が長いほど負に大きくなる
/// 長さ依存量。トークン数で割ることで候補間で比較可能な「自信度」になる。
fn avg_logprob(score: f32, n_tokens: usize) -> f32 {
    score / (n_tokens.max(1) as f32)
}

/// 自信度 (平均 log-prob) に基づいて異常変換候補を棄却する。
///
/// 入力 `cands` は `(表層, 平均 log-prob)` のリスト（スコア降順を想定）。
/// - `margin`: 最良候補よりこれ以上低い候補を外れ値として捨てる (相対棄却)。
/// - `min_top`: 最良候補すらこれを下回るなら全候補を捨て、空を返す (絶対フロア／フォールバック)。
///
/// 純粋関数。llama 非依存で単体テスト可能。
fn filter_by_confidence(
    cands: Vec<(String, f32)>,
    margin: Option<f32>,
    min_top: Option<f32>,
) -> Vec<String> {
    if cands.is_empty() {
        return Vec::new();
    }
    // 最良 (= 平均 log-prob 最大) を基準にする。入力はスコア降順想定だが念のため算出。
    let top = cands
        .iter()
        .map(|(_, lp)| *lp)
        .fold(f32::NEG_INFINITY, f32::max);

    // 絶対フロア: 最良候補すら自信が低すぎる → 全棄却してフォールバックさせる。
    if let Some(floor) = min_top {
        if top < floor {
            return Vec::new();
        }
    }

    cands
        .into_iter()
        .filter(|(_, lp)| match margin {
            Some(m) => *lp >= top - m,
            None => true,
        })
        .map(|(s, _)| s)
        .collect()
}

/// Build a prompt in jinen format
pub fn build_jinen_prompt(katakana: &str, context: &str) -> String {
    format!(
        "{}{}{}{}{}",
        CONTEXT_TOKEN, context, INPUT_START_TOKEN, katakana, OUTPUT_START_TOKEN
    )
}

/// Clean model output by trimming whitespace and removing spurious furigana.
///
/// Special tokens (BOS/EOS) are handled at the decode level via
/// `skip_special_tokens` rather than string replacement.
///
/// # Furigana removal
/// LLM が「健診(けんしん)や」のようにルビ形式で読みを付けることがある。
/// 全角・半角括弧内がひらがな・カタカナのみで構成される場合は除去する。
/// 意図的な括弧（(笑)、(注)、(英数字)）はカナ以外の文字を含むため保持される。
pub fn clean_model_output(text: &str) -> String {
    strip_furigana(text.trim())
}

/// 括弧内がひらがな・カタカナのみで構成される場合に括弧ごと除去する。
fn strip_furigana(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let close = match c {
            '（' => Some('）'),
            '(' => Some(')'),
            _ => None,
        };
        if let Some(close_ch) = close {
            // 閉じ括弧を探す（同一行内のみ、最大30文字先まで）
            let lookahead = chars[i + 1..].iter().take(30);
            let end_pos = lookahead
                .enumerate()
                .find(|&(_, &x)| x == close_ch)
                .map(|(j, _)| j);
            if let Some(end) = end_pos {
                let inner: String = chars[i + 1..i + 1 + end].iter().collect();
                // 内容がひらがな・カタカナ（長音符含む）のみなら除去
                let is_kana_only = !inner.is_empty() && inner.chars().all(is_kana_or_prolonged);
                if is_kana_only {
                    i = i + 1 + end + 1; // 括弧全体をスキップ
                    continue;
                }
            }
        }
        result.push(c);
        i += 1;
    }
    result
}

/// ひらがな・カタカナ・長音符・中点のいずれかか判定する。
#[inline]
fn is_kana_or_prolonged(c: char) -> bool {
    let n = c as u32;
    (0x3041..=0x3096).contains(&n)   // ひらがな（ぁ〜ゖ）
    || (0x30A1..=0x30FC).contains(&n) // カタカナ（ァ〜ー、ー含む）
    || c == 'ー' || c == '・' || c == 'ｰ'
}

/// Inference backend configuration (llama.cpp GGUF format with external tokenizer)
#[derive(Debug, Clone)]
pub struct Backend {
    gguf_path: String,
    tokenizer_json_path: String,
    /// Display name for the model (variant id for registry models, "custom" for GGUF paths)
    display_name: String,
    /// Number of layers to offload to GPU (0 = CPU only, u32::MAX = all layers)
    pub n_gpu_layers: u32,
    /// GPU index to use (0 = first GPU, -1 = auto)
    pub main_gpu: i32,
}

impl Backend {
    /// Create a backend from a `(ModelFamily, VariantConfig)` pair.
    ///
    /// Downloads the GGUF and the external tokenizer from HuggingFace.
    pub fn from_variant(family: &ModelFamily, variant: &VariantConfig) -> Result<Self> {
        let path = get_variant_path(family, variant)?;
        let tokenizer_path = get_tokenizer_path(family)?;
        Ok(Backend {
            gguf_path: path.to_string_lossy().to_string(),
            tokenizer_json_path: tokenizer_path.to_string_lossy().to_string(),
            display_name: variant.id.clone(),
            n_gpu_layers: 0,
            main_gpu: 0,
        })
    }

    /// Set the number of GPU layers to offload. -1 = all layers, 0 = CPU only.
    pub fn with_n_gpu_layers(mut self, n: u32) -> Self {
        self.n_gpu_layers = n;
        self
    }

    /// Set the GPU index to use (0 = first GPU, -1 = auto).
    pub fn with_main_gpu(mut self, gpu: i32) -> Self {
        self.main_gpu = gpu;
        self
    }

    /// Create a backend by looking up a variant id in the global registry.
    ///
    /// E.g. `Backend::from_variant_id("jinen-v1-xsmall-q5")`
    pub fn from_variant_id(variant_id: &str) -> Result<Self> {
        let (family, variant) = registry()
            .find_variant(variant_id)
            .ok_or_else(|| KanjiError::UnknownVariant(variant_id.to_string()))?;
        Self::from_variant(family, variant)
    }
}

/// Kanji converter using llama.cpp backend
pub struct KanaKanjiConverter {
    model: LlamaCppModel,
    config: ConversionConfig,
    display_name: String,
}

impl KanaKanjiConverter {
    /// Create a new converter with the specified backend
    pub fn new(backend: Backend) -> Result<Self> {
        Self::with_config(backend, ConversionConfig::default())
    }

    /// Create a new converter with the specified backend and configuration
    pub fn with_config(backend: Backend, config: ConversionConfig) -> Result<Self> {
        let model = LlamaCppModel::from_file_with_gpu_layers(
            &backend.gguf_path,
            &backend.tokenizer_json_path,
            backend.n_gpu_layers,
            backend.main_gpu,
        )?;
        Ok(KanaKanjiConverter {
            model,
            config,
            display_name: backend.display_name,
        })
    }

    /// Set the number of threads for inference (0 = default).
    pub fn set_n_threads(&mut self, n: u32) {
        self.model.set_n_threads(n);
    }

    /// Convert hiragana to kanji candidates
    ///
    /// # Arguments
    /// * `reading` - Input reading in hiragana
    /// * `context` - Left context (previously converted text)
    /// * `num_candidates` - Number of candidates to generate
    ///
    /// # Returns
    /// Vector of conversion candidates
    pub fn convert(
        &self,
        reading: &str,
        context: &str,
        num_candidates: usize,
    ) -> Result<Vec<String>> {
        let max_new_tokens = generation_budget(reading, self.config.max_new_tokens);

        // context 汚染対策: 読みのエコー源を含む context はその位置で切り捨てる。
        // 「変換できない」報告の切り分け用に、発動時のみ INFO で観測ログを残す。
        let stripped = strip_echo_context(context, reading);
        if stripped.len() != context.len() {
            tracing::info!(
                reading_chars = reading.chars().count(),
                context_bytes = context.len(),
                stripped_bytes = stripped.len(),
                "echo source stripped from context"
            );
        }
        let context = stripped;

        // Convert hiragana to katakana (model expects katakana input)
        let katakana = hiragana_to_katakana(reading);

        // Build prompt in jinen format
        let prompt = build_jinen_prompt(&katakana, context);

        // Tokenize
        let tokens = self.model.tokenize(&prompt)?;
        let eos = Some(self.model.eos_token_id().0);

        if num_candidates == 1 {
            // Single candidate: use greedy decoding (faster)
            let output_tokens = self.model.generate(&tokens, max_new_tokens, eos)?;
            let generated = &output_tokens[tokens.len()..];
            let text = self.model.decode(generated, true)?;
            let clean = clean_model_output(&text);

            let mut candidates = Vec::with_capacity(1);
            if !clean.is_empty() {
                candidates.push(clean);
            }

            // greedy パスは score を返さないため自信度フィルタは適用できない。
            // 長さ安全網 (下記) のみが効く。スコアによる異常検出が必要なら
            // num_candidates >= 2 (beam パス) を使う。
            let reading_chars = reading.chars().count();
            let reading_repeats = has_tandem_repeat(reading, REPEAT_MIN_PERIOD);
            candidates.retain(|c| {
                if c.chars().count() * 3 < reading_chars {
                    return false;
                }
                if is_degenerate_candidate(c, reading_chars, reading_repeats) {
                    tracing::debug!(reading = %reading, candidate = %c, "dropped degenerate candidate (greedy)");
                    return false;
                }
                if is_kana_prefix_echo(c, reading) {
                    tracing::debug!(reading = %reading, candidate = %c, "dropped kana prefix echo candidate (greedy)");
                    return false;
                }
                true
            });
            if candidates.is_empty() {
                candidates.push(reading.to_string());
            }
            return Ok(candidates);
        }

        // Multiple candidates: use true beam search for better candidate quality.
        // d1_greedy is faster but generates candidates unrelated to the reading.
        //
        // beam_size は num_candidates に等しい（ユーザが要求した候補数がそのまま
        // beam 幅になる）。`config.beam_size` は安全上限として機能し、デフォルト
        // 30 で実質上限なし。変換速度を抑えたいユーザは config.toml の
        // `[conversion] beam_size` を小さく設定して明示的に上限をかける。
        let configured_cap = self.config.beam_size.clamp(1, 30);
        let beam_size = num_candidates.min(configured_cap).clamp(1, 30);
        let gen_start = std::time::Instant::now();
        let results = self
            .model
            .generate_beam_search(&tokens, max_new_tokens, eos, beam_size)?;
        // 変換 1 件ごとの観測ログ。「止まる」「切れる」報告時に
        // rakukan-engine-dll.log だけで所要時間と EOS 到達状況を切り分けられる。
        tracing::info!(
            reading_chars = reading.chars().count(),
            beam_size,
            budget = max_new_tokens,
            finished_beams = results.len(),
            elapsed_ms = gen_start.elapsed().as_millis() as u64,
            "beam conversion done"
        );

        // (表層, 平均 log-prob) を保持。beam score は累積 log-prob (長さ依存) なので
        // トークン数で正規化して候補間で比較可能な自信度にする。
        let mut scored: Vec<(String, f32)> = Vec::with_capacity(results.len());
        for (output_tokens, score) in results {
            let text = self.model.decode(&output_tokens, true)?;
            let clean = clean_model_output(&text);
            if clean.is_empty() || scored.iter().any(|(s, _)| s == &clean) {
                continue;
            }
            scored.push((clean, avg_logprob(score, output_tokens.len())));
        }

        // M1.5 T-BUG1 (c): 出力が極端に短い候補を捨てる安全網。reading の
        // 33% 以上の長さを持つ候補だけを残す。0.7.0 で TSF 側 (T-BUG2) にも
        // 同等の防壁があるが、エンジン側で先に弾けば session に短い preview が
        // 入らず、後段の sanity check や filter に頼らず済む。
        // 加えて長さ上限（読み×1.5+2）とタンデム反復検出で「同じ文が 2 度
        // 続く」退化候補を棄却する（confidence フィルタは反復を捕捉できない）。
        let reading_chars = reading.chars().count();
        let reading_repeats = has_tandem_repeat(reading, REPEAT_MIN_PERIOD);
        scored.retain(|(c, _)| {
            if c.chars().count() * 3 < reading_chars {
                return false;
            }
            if is_degenerate_candidate(c, reading_chars, reading_repeats) {
                tracing::debug!(reading = %reading, candidate = %c, "dropped degenerate candidate (beam)");
                return false;
            }
            if is_kana_prefix_echo(c, reading) {
                tracing::debug!(reading = %reading, candidate = %c, "dropped kana prefix echo candidate (beam)");
                return false;
            }
            true
        });

        // 自信度 (平均 log-prob) の観測ログ。閾値チューニングの材料になる。
        if tracing::enabled!(tracing::Level::DEBUG) {
            for (c, lp) in &scored {
                tracing::debug!(reading = %reading, candidate = %c, avg_logprob = lp, "conv candidate confidence");
            }
        }

        // 自信度に基づく異常変換の棄却（相対外れ値＋絶対フロア）。
        let mut candidates = filter_by_confidence(
            scored,
            self.config.confidence_margin,
            self.config.min_top_confidence,
        );

        // If no candidates, return the original reading
        if candidates.is_empty() {
            candidates.push(reading.to_string());
        }

        Ok(candidates)
    }

    /// Get a human-readable model name for display
    pub fn model_display_name(&self) -> &str {
        &self.display_name
    }

    /// Count only the input (reading) tokens, excluding context and special tokens
    pub fn count_input_tokens(&self, reading: &str) -> Result<usize> {
        let katakana = hiragana_to_katakana(reading);
        let tokens = self.model.tokenize(&katakana)?;
        Ok(tokens.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_budget_grows_with_reading_length() {
        // 短い reading は config_max_new_tokens (15) で頭打ち
        assert_eq!(generation_budget("かな", 15), 15);
        // 15 文字 reading: 15*2+8 = 38。M1.5 T-BUG1 (a) で上限 256 に拡張済。
        assert_eq!(generation_budget("これはながめのへんかんぶんです", 15), 38);
    }

    #[test]
    fn tandem_repeat_detects_doubled_sentence() {
        // 同じ文がそのまま 2 度続く退化パターン
        assert!(has_tandem_repeat(
            "対応策を検討して対応策を検討して",
            REPEAT_MIN_PERIOD
        ));
        // 文中の部分反復（8 文字周期）
        assert!(has_tandem_repeat(
            "本日は晴天なり本日は晴天なりです",
            REPEAT_MIN_PERIOD
        ));
    }

    #[test]
    fn tandem_repeat_allows_normal_text() {
        assert!(!has_tandem_repeat("今日は良い天気です", REPEAT_MIN_PERIOD));
        // 畳語（周期 2〜3）は正当な日本語なので検出しない
        assert!(!has_tandem_repeat("ますます盛り上がる", REPEAT_MIN_PERIOD));
        assert!(!has_tandem_repeat("きらきら光る星", REPEAT_MIN_PERIOD));
        // 空文字・短い文字列は安全に false
        assert!(!has_tandem_repeat("", REPEAT_MIN_PERIOD));
        assert!(!has_tandem_repeat("短い", REPEAT_MIN_PERIOD));
    }

    #[test]
    fn degenerate_filter_rejects_overlong_candidate() {
        // 読み 10 文字 → 上限 17 文字。18 文字の候補は棄却。
        assert_eq!(max_candidate_chars(10), 17);
        let reading_chars = 10;
        assert!(is_degenerate_candidate(
            "あいうえおかきくけこさしすせそたちつ", // 18 文字
            reading_chars,
            false
        ));
        // 上限ちょうど（17 文字）は通す
        assert!(!is_degenerate_candidate(
            "あいうえおかきくけこさしすせそたち", // 17 文字
            reading_chars,
            false
        ));
    }

    #[test]
    fn degenerate_filter_respects_reading_repeats() {
        // 読み自身が反復を含む場合（「わかったわかった」等の実入力）は
        // 候補の反復を許容する
        let reading = "わかったわかった";
        let reading_chars = reading.chars().count();
        assert!(!is_degenerate_candidate(
            "分かった分かった",
            reading_chars,
            has_tandem_repeat(reading, REPEAT_MIN_PERIOD)
        ));
        // 読みに反復がなければ候補の反復は退化として棄却
        assert!(is_degenerate_candidate(
            "分かった分かった",
            reading_chars,
            false
        ));
    }

    #[test]
    fn strip_echo_context_cuts_at_hiragana_echo_source() {
        // 実機事例: 未変換確定「きだじゅんいちろう氏は、」が context 末尾に残ったケース
        let context = "あらゆるコレクターの涙する場面だ。場面はがの共感を呼び、きだじゅんいちろう氏は、";
        let reading = "きだじゅんいちろうしは";
        assert_eq!(
            strip_echo_context(context, reading),
            "あらゆるコレクターの涙する場面だ。場面はがの共感を呼び、"
        );
    }

    #[test]
    fn strip_echo_context_cuts_at_katakana_echo_source() {
        // F7 カタカナ確定由来の汚染も検出する
        let context = "前の文。キダジュンイチロウは、";
        let reading = "きだじゅんいちろうしは";
        assert_eq!(strip_echo_context(context, reading), "前の文。");
    }

    #[test]
    fn strip_echo_context_keeps_converted_context() {
        // 漢字に変換済みの文はかな列として一致しないので削られない
        let context = "紀田順一郎氏は、蔵書を処分した。";
        let reading = "きだじゅんいちろうしは";
        assert_eq!(strip_echo_context(context, reading), context);
    }

    #[test]
    fn strip_echo_context_ignores_short_readings() {
        // 短い読み（助詞等）では context を切り捨てない
        let context = "それはそうだが、";
        assert_eq!(strip_echo_context(context, "それ"), context);
        assert_eq!(strip_echo_context(context, "は"), context);
    }

    #[test]
    fn strip_echo_context_requires_prefix_match() {
        // 読みプレフィックス（6 文字）が丸ごと一致しない限り切らない。
        // 「ということだ。」は reading「ということで」と 5 文字しか一致しない。
        let context = "それは当然のことだ、ということだ。";
        let reading = "ということで";
        assert_eq!(strip_echo_context(context, reading), context);
    }

    #[test]
    fn kana_prefix_echo_rejects_truncated_echo() {
        let reading = "きだじゅんいちろうしは";
        // ひらがな・カタカナの尻切れエコーは棄却
        assert!(is_kana_prefix_echo("きだじゅん", reading));
        assert!(is_kana_prefix_echo("キダジュン", reading));
        assert!(is_kana_prefix_echo("きだじゅ", reading));
    }

    #[test]
    fn kana_prefix_echo_keeps_legitimate_candidates() {
        let reading = "きだじゅんいちろうしは";
        // 読み全体（無変換フォールバック）は残す
        assert!(!is_kana_prefix_echo("きだじゅんいちろうしは", reading));
        // 漢字を含む候補は対象外
        assert!(!is_kana_prefix_echo("木田純一郎氏は", reading));
        assert!(!is_kana_prefix_echo("きだ純一郎氏は", reading));
        // 読み全体のカタカナ変換（F7 相当）は残す
        assert!(!is_kana_prefix_echo("コーヒー", "こーひー"));
        // プレフィックスでないかな候補は対象外
        assert!(!is_kana_prefix_echo("だじゅん", reading));
    }

    #[test]
    fn avg_logprob_normalizes_by_length() {
        // 累積 -6.0 を 3 トークンで割れば -2.0/token
        assert_eq!(avg_logprob(-6.0, 3), -2.0);
        // 同じ累積でも長い系列ほど 1 トークンあたりは小さく（0 に近く）なる
        assert!(avg_logprob(-6.0, 6) > avg_logprob(-6.0, 3));
        // 0 除算ガード
        assert_eq!(avg_logprob(-6.0, 0), -6.0);
    }

    #[test]
    fn confidence_filter_keeps_all_when_disabled() {
        let cands = vec![("漢字".into(), -0.5), ("感じ".into(), -4.0)];
        let out = filter_by_confidence(cands, None, None);
        assert_eq!(out, vec!["漢字".to_string(), "感じ".to_string()]);
    }

    #[test]
    fn confidence_filter_drops_relative_outlier() {
        // 最良 -0.5。margin 3.0 → -3.5 未満を棄却。-4.0 の候補は外れ値として落ちる。
        let cands = vec![
            ("漢字".into(), -0.5),
            ("感じ".into(), -1.0),
            ("ゴミ".into(), -4.0),
        ];
        let out = filter_by_confidence(cands, Some(3.0), None);
        assert_eq!(out, vec!["漢字".to_string(), "感じ".to_string()]);
    }

    #[test]
    fn confidence_filter_keeps_top_under_relative_rule() {
        // 最良候補は基準そのものなので相対ルールでは決して落ちない。
        let cands = vec![("唯一".into(), -9.9)];
        let out = filter_by_confidence(cands, Some(3.0), None);
        assert_eq!(out, vec!["唯一".to_string()]);
    }

    #[test]
    fn confidence_filter_absolute_floor_rejects_all() {
        // 最良 -5.0 が フロア -3.0 を下回る → 全棄却（呼び出し側でかなフォールバック）。
        let cands = vec![("幻覚".into(), -5.0), ("別".into(), -6.0)];
        let out = filter_by_confidence(cands, Some(3.0), Some(-3.0));
        assert!(out.is_empty());
    }

    #[test]
    fn confidence_filter_absolute_floor_passes_when_confident() {
        // 最良 -1.0 はフロア -3.0 以上なので通過し、相対ルールのみ適用。
        let cands = vec![("良".into(), -1.0), ("悪".into(), -5.0)];
        let out = filter_by_confidence(cands, Some(3.0), Some(-3.0));
        assert_eq!(out, vec!["良".to_string()]);
    }

    #[test]
    fn test_default_model_beam_conversion() {
        // beam 経路（generate_beam_search_impl）の実モデル検証。
        // 1 コンテキスト × batched decode 書き換え（F5）の回帰確認と、
        // EOS 未到達 beam 棄却（F3）後も通常の読みで候補が返ることの確認。
        let backend =
            Backend::from_variant_id("jinen-v1-small-q5").expect("Failed to load default model");
        let converter = KanaKanjiConverter::new(backend).expect("Failed to create converter");

        let result = converter.convert("へんかんけっかをかくにんする", "", 9);
        assert!(result.is_ok(), "Conversion failed: {:?}", result.err());

        let candidates = result.unwrap();
        assert!(!candidates.is_empty(), "No candidates returned");
        for c in &candidates {
            assert!(!c.contains("ã"), "Output contains mojibake: '{}'", c);
        }
    }

    #[test]

    fn test_default_model_conversion() {
        let backend =
            Backend::from_variant_id("jinen-v1-small-q5").expect("Failed to load default model");
        let converter = KanaKanjiConverter::new(backend).expect("Failed to create converter");

        let result = converter.convert("かんじ", "", 1);
        assert!(result.is_ok(), "Conversion failed: {:?}", result.err());

        let candidates = result.unwrap();
        assert!(!candidates.is_empty(), "No candidates returned");

        let output = &candidates[0];
        assert!(
            !output.contains("ã"),
            "Output contains mojibake: '{}'",
            output
        );
    }

    #[test]
    #[ignore = "requires network access to download GGUF model"]
    fn test_xsmall_special_tokens() {
        use super::super::hf_download::{get_path_by_id, get_tokenizer_path_by_id};
        use super::super::{CONTEXT_TOKEN, INPUT_START_TOKEN, OUTPUT_START_TOKEN};
        let path = get_path_by_id("jinen-v1-xsmall-q5").expect("Failed to download GGUF");
        let tok_path =
            get_tokenizer_path_by_id("jinen-v1-xsmall-q5").expect("Failed to download tokenizer");
        let model = LlamaCppModel::from_file(&path, &tok_path).expect("Failed to load model");

        let prompt = build_jinen_prompt("テスト", "");
        let tokens = model.tokenize(&prompt).expect("Failed to tokenize");

        let mut found_context = false;
        let mut found_input_start = false;
        let mut found_output_start = false;

        for token in &tokens {
            let display = model.decode_token_for_display(*token);
            if display.contains(CONTEXT_TOKEN) {
                found_context = true;
            }
            if display.contains(INPUT_START_TOKEN) {
                found_input_start = true;
            }
            if display.contains(OUTPUT_START_TOKEN) {
                found_output_start = true;
            }
        }

        assert!(found_context, "CONTEXT token (U+EE02) not found");
        assert!(found_input_start, "INPUT_START token (U+EE00) not found");
        assert!(found_output_start, "OUTPUT_START token (U+EE01) not found");
    }

    #[test]

    fn test_xsmall_conversion() {
        let backend =
            Backend::from_variant_id("jinen-v1-xsmall-q5").expect("Failed to download GGUF");
        let converter = KanaKanjiConverter::new(backend).expect("Failed to create converter");

        let result = converter.convert("かんじ", "", 1);
        assert!(result.is_ok(), "Conversion failed: {:?}", result.err());

        let candidates = result.unwrap();
        assert!(!candidates.is_empty(), "No candidates returned");

        let output = &candidates[0];
        assert!(
            !output.contains("ã"),
            "Output contains mojibake (GPT-2 byte encoding leak): '{}'",
            output
        );
    }
}
