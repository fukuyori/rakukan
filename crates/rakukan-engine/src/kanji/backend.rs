//! Backend interface for kanji conversion using llama.cpp

use super::error::KanjiError;
use super::hf_download::{get_tokenizer_path, get_variant_path};
use super::llamacpp::LlamaCppModel;
use super::model_config::{ModelFamily, VariantConfig, registry};
use super::{CONTEXT_TOKEN, INPUT_START_TOKEN, OUTPUT_START_TOKEN};
use crate::kana::hiragana_to_katakana;

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
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            max_new_tokens: 15,
            beam_size: 30,
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

        // Convert hiragana to katakana (model expects katakana input)
        let katakana = hiragana_to_katakana(reading);

        // Build prompt in jinen format
        let prompt = build_jinen_prompt(&katakana, context);

        // Tokenize
        let tokens = self.model.tokenize(&prompt)?;
        let eos = Some(self.model.eos_token_id().0);

        let mut candidates = Vec::with_capacity(num_candidates);

        if num_candidates == 1 {
            // Single candidate: use greedy decoding (faster)
            let output_tokens = self.model.generate(&tokens, max_new_tokens, eos)?;
            let generated = &output_tokens[tokens.len()..];
            let text = self.model.decode(generated, true)?;
            let clean = clean_model_output(&text);

            if !clean.is_empty() {
                candidates.push(clean);
            }
        } else {
            // Multiple candidates: use true beam search for better candidate quality.
            // d1_greedy is faster but generates candidates unrelated to the reading.
            //
            // beam_size は num_candidates に等しい（ユーザが要求した候補数がそのまま
            // beam 幅になる）。`config.beam_size` は安全上限として機能し、デフォルト
            // 30 で実質上限なし。変換速度を抑えたいユーザは config.toml の
            // `[conversion] beam_size` を小さく設定して明示的に上限をかける。
            let configured_cap = self.config.beam_size.clamp(1, 30);
            let beam_size = num_candidates.min(configured_cap).clamp(1, 30);
            let results =
                self.model
                    .generate_beam_search(&tokens, max_new_tokens, eos, beam_size)?;

            for (output_tokens, _score) in results {
                let text = self.model.decode(&output_tokens, true)?;
                let clean = clean_model_output(&text);

                if !clean.is_empty() && !candidates.contains(&clean) {
                    candidates.push(clean);
                }
            }
        }

        // M1.5 T-BUG1 (c): 出力が極端に短い候補を捨てる安全網。reading の
        // 33% 以上の長さを持つ候補だけを残す。0.7.0 で TSF 側 (T-BUG2) にも
        // 同等の防壁があるが、エンジン側で先に弾けば session に短い preview が
        // 入らず、後段の sanity check や filter に頼らず済む。
        let reading_chars = reading.chars().count();
        candidates.retain(|c| c.chars().count() * 3 >= reading_chars);

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
