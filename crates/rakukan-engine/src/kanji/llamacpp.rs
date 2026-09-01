//! llama.cpp based GGUF inference for kanji conversion
//!
//! This module provides an alternative to Candle's GGUF implementation using
//! llama.cpp's optimized inference engine via the llama-cpp-2 crate.
//!
//! Enable with the `llamacpp` feature flag.

use super::error::KanjiError;
type Result<T> = super::error::Result<T>;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use std::collections::HashSet;
use std::num::NonZeroU32;
use std::path::Path;
use std::sync::OnceLock;

/// Global llama.cpp backend (can only be initialized once)
static LLAMA_BACKEND: OnceLock<std::result::Result<LlamaBackend, String>> = OnceLock::new();

/// 生成全体のウォールクロック上限秒数（greedy / beam 共通）。
/// GPU ハング以外の「EOS が出ずに max_new_tokens まで走り続ける」ケースを打ち切る。
/// GPU ハング（ctx.decode がブロッキングになる）はここでは防げないが、
/// その場合は TSF 側のウォッチドッグ (bg_timeout_watchdog) が engine_reload で対処する。
const GEN_TIMEOUT_SECS: u64 = 15;

/// Get or initialize the global llama.cpp backend
fn get_backend() -> Result<&'static LlamaBackend> {
    let result = LLAMA_BACKEND.get_or_init(|| {
        let mut backend = LlamaBackend::init().map_err(|e| e.to_string())?;
        backend.void_logs();
        Ok(backend)
    });
    match result {
        Ok(backend) => Ok(backend),
        Err(e) => Err(KanjiError::ModelLoad(
            format!("Failed to initialize llama.cpp backend: {}", e).into(),
        )),
    }
}

/// Convert bytes to hex display format for partial UTF-8 sequences
fn bytes_to_hex_display(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("<{:02X}>", b)).collect()
}

/// Load and configure an external HuggingFace tokenizer from a `tokenizer.json` file.
fn load_tokenizer<P: AsRef<Path>>(path: P) -> Result<tokenizers::Tokenizer> {
    let mut tokenizer =
        tokenizers::Tokenizer::from_file(path.as_ref()).map_err(KanjiError::TokenizerLoad)?;
    // Disable padding and truncation — we handle sequence length ourselves
    // and padding tokens would corrupt the model input.
    tokenizer.with_padding(None);
    tokenizer.with_truncation(None).ok();
    Ok(tokenizer)
}

/// `<0xNN>` 形式のバイトフォールバックトークンなら true。
fn is_byte_fallback_token(content: &str) -> bool {
    content.len() == 6
        && content.starts_with("<0x")
        && content.ends_with('>')
        && u8::from_str_radix(&content[3..5], 16).is_ok()
}

/// skip 対象にする special トークン ID を収集する（バイトフォールバックは除外）。
///
/// jinen v2 (Qwen3) の tokenizer.json はバイトフォールバックトークン
/// (`<0xNN>`) を `special: true` で登録しているため、tokenizers クレートの
/// `decode(_, skip_special_tokens=true)` に任せると UTF-8 復元前に
/// バイトトークンごと捨てられ、語彙に単独トークンが無い文字
/// （Ψ・€・絵文字など）が出力から無言で消える。
/// skip は本モジュール側で ID フィルタとして行い、tokenizers には常に
/// `skip_special_tokens=false` で渡す。
fn non_byte_special_token_ids(tokenizer: &tokenizers::Tokenizer) -> HashSet<u32> {
    tokenizer
        .get_added_tokens_decoder()
        .into_iter()
        .filter(|(_, tok)| tok.special && !is_byte_fallback_token(&tok.content))
        .map(|(id, _)| id)
        .collect()
}

/// A beam candidate with generated tokens and cumulative score
#[derive(Clone)]
struct BeamState {
    tokens: Vec<LlamaToken>,
    score: f32,
}

/// llama.cpp based GPT-2 model for GGUF inference
#[allow(dead_code)]
pub struct LlamaCppModel {
    model: LlamaModel,
    n_ctx: u32,
    /// External HuggingFace tokenizer (always required).
    /// `tokenize()` and `decode()` use this instead of llama.cpp's built-in tokenizer.
    external_tokenizer: tokenizers::Tokenizer,
    /// special トークンのうちバイトフォールバック (`<0xNN>`) を除いた ID 集合。
    /// `decode(skip_special_tokens=true)` はこの集合で自前フィルタする。
    special_token_ids: HashSet<u32>,
    /// Number of threads for inference (0 = use llama.cpp default)
    n_threads: u32,
    /// Number of layers to offload to GPU (0 = CPU only, u32::MAX = all layers)
    n_gpu_layers: u32,
    /// GPU index (0 = first GPU)
    main_gpu: i32,
}

impl LlamaCppModel {
    /// Load a GGUF model using llama.cpp with an external tokenizer.
    ///
    /// GPT-2 models use CPU only (Metal has issues with GPT-2).
    pub fn from_file<P: AsRef<Path>, T: AsRef<Path>>(path: P, tokenizer_json: T) -> Result<Self> {
        Self::from_file_with_gpu_layers(path, tokenizer_json, 0, 0)
    }

    /// Load a GGUF model with explicit GPU layer count.
    /// * `n_gpu_layers = 0`  — CPU only
    /// * `n_gpu_layers = u32::MAX` — offload all layers to GPU (CUDA / Vulkan)
    pub fn from_file_with_gpu_layers<P: AsRef<Path>, T: AsRef<Path>>(
        path: P,
        tokenizer_json: T,
        n_gpu_layers: u32,
        main_gpu: i32,
    ) -> Result<Self> {
        let backend = get_backend()?;
        let model_params = LlamaModelParams::default()
            .with_n_gpu_layers(n_gpu_layers)
            .with_main_gpu(main_gpu);
        let model = LlamaModel::load_from_file(backend, path.as_ref(), &model_params)
            .map_err(|e| KanjiError::ModelLoad(e.into()))?;
        let external_tokenizer = load_tokenizer(tokenizer_json)?;
        let special_token_ids = non_byte_special_token_ids(&external_tokenizer);
        Ok(Self {
            model,
            n_ctx: 128,
            external_tokenizer,
            special_token_ids,
            n_threads: 0,
            n_gpu_layers,
            main_gpu,
        })
    }

    /// Load a GGUF model with a pre-tokenizer type override.
    ///
    /// Some models use custom pre-tokenizer types (e.g., `gpt2-small-japanese-char`)
    /// that llama.cpp doesn't recognize. This method overrides the `tokenizer.ggml.pre`
    /// metadata key to a compatible type before loading.
    pub fn from_file_with_pre_tokenizer_override<P: AsRef<Path>, T: AsRef<Path>>(
        path: P,
        tokenizer_json: T,
        pre_tokenizer: &str,
    ) -> Result<Self> {
        use llama_cpp_2::model::params::kv_overrides::ParamOverrideValue;
        use std::ffi::CString;
        use std::pin::pin;

        let backend = get_backend()?;

        let mut params = pin!(LlamaModelParams::default().with_n_gpu_layers(0));

        let key =
            CString::new("tokenizer.ggml.pre").map_err(|e| KanjiError::ModelLoad(e.into()))?;
        let mut str_value: [std::os::raw::c_char; 128] = [0; 128];
        for (i, &byte) in pre_tokenizer.as_bytes().iter().enumerate() {
            if i >= 127 {
                break;
            }
            str_value[i] = byte as std::os::raw::c_char;
        }
        params
            .as_mut()
            .append_kv_override(&key, ParamOverrideValue::Str(str_value));

        let model = LlamaModel::load_from_file(backend, path.as_ref(), &params)
            .map_err(|e| KanjiError::ModelLoad(e.into()))?;
        let external_tokenizer = load_tokenizer(tokenizer_json)?;
        let special_token_ids = non_byte_special_token_ids(&external_tokenizer);

        Ok(Self {
            model,
            n_ctx: 128,
            external_tokenizer,
            special_token_ids,
            n_threads: 0,
            n_gpu_layers: 0,
            main_gpu: 0,
        })
    }

    /// Load a GGUF model with explicit context window size
    pub fn from_file_with_n_ctx<P: AsRef<Path>, T: AsRef<Path>>(
        path: P,
        tokenizer_json: T,
        n_ctx: u32,
    ) -> Result<Self> {
        let backend = get_backend()?;

        let model_params = LlamaModelParams::default().with_n_gpu_layers(0);

        let model = LlamaModel::load_from_file(backend, path.as_ref(), &model_params)
            .map_err(|e| KanjiError::ModelLoad(e.into()))?;
        let external_tokenizer = load_tokenizer(tokenizer_json)?;
        let special_token_ids = non_byte_special_token_ids(&external_tokenizer);

        Ok(Self {
            model,
            n_ctx,
            external_tokenizer,
            special_token_ids,
            n_threads: 0,
            n_gpu_layers: 0,
            main_gpu: 0,
        })
    }

    /// Set the number of threads for inference.
    /// 0 means use llama.cpp default (typically all cores).
    pub fn set_n_threads(&mut self, n: u32) {
        self.n_threads = n;
    }

    /// Build LlamaContextParams with configured n_threads
    fn context_params(&self) -> LlamaContextParams {
        let params = LlamaContextParams::default().with_n_ctx(Some(
            NonZeroU32::new(self.n_ctx).expect("n_ctx must be non-zero"),
        ));
        if self.n_threads > 0 {
            params
                .with_n_threads(self.n_threads as i32)
                .with_n_threads_batch(self.n_threads as i32)
        } else {
            params
        }
    }

    /// Tokenize a string using the external tokenizer
    pub fn tokenize(&self, text: &str) -> Result<Vec<LlamaToken>> {
        let encoding = self
            .external_tokenizer
            .encode(text, false)
            .map_err(KanjiError::Inference)?;
        let tokens: Vec<LlamaToken> = encoding
            .get_ids()
            .iter()
            .map(|&id| LlamaToken(id as i32))
            .collect();
        Ok(tokens)
    }

    /// Decode tokens to string using the external tokenizer
    ///
    /// When `skip_special_tokens` is true, special tokens (BOS, EOS, EOG) are
    /// excluded from the output. バイトフォールバックトークン (`<0xNN>`) は
    /// special 扱いでも捨てずにデコーダへ渡す（Ψ など語彙外文字の復元に必要）。
    pub fn decode(&self, tokens: &[LlamaToken], skip_special_tokens: bool) -> Result<String> {
        let ids: Vec<u32> = tokens
            .iter()
            .map(|t| t.0 as u32)
            .filter(|id| !(skip_special_tokens && self.special_token_ids.contains(id)))
            .collect();
        // tokenizers 側の skip_special_tokens は常に false。true にすると
        // special フラグ付きのバイトフォールバックトークンが UTF-8 復元前に
        // 破棄され、Ψ・€・絵文字などが出力から消える（karukan PR #91 と同件）。
        let text = self
            .external_tokenizer
            .decode(&ids, false)
            .map_err(KanjiError::Inference)?;
        Ok(text)
    }

    /// Decode a single token for display purposes.
    ///
    /// For byte-level BPE tokens that represent partial UTF-8 sequences,
    /// this returns a hex representation like `<0xE3>` instead of replacement characters.
    pub fn decode_token_for_display(&self, token: LlamaToken) -> String {
        match self.model.token_to_piece_bytes(token, 32, true, None) {
            Ok(bytes) => {
                if let Ok(s) = std::str::from_utf8(&bytes) {
                    // Valid UTF-8, return as-is (escape control chars)
                    if s.chars().all(|c| !c.is_control() || c == ' ' || c == '\n') {
                        s.to_string()
                    } else {
                        // Has control characters, show hex
                        bytes_to_hex_display(&bytes)
                    }
                } else {
                    // Invalid UTF-8 (partial sequence), show hex
                    bytes_to_hex_display(&bytes)
                }
            }
            Err(_) => format!("<{}>", token.0),
        }
    }

    /// Generate tokens with greedy decoding
    pub fn generate(
        &self,
        input_tokens: &[LlamaToken],
        max_new_tokens: usize,
        eos_token_id: Option<i32>,
    ) -> Result<Vec<LlamaToken>> {
        self.generate_with_sampler(
            input_tokens,
            max_new_tokens,
            eos_token_id,
            LlamaSampler::greedy(),
        )
    }

    /// Generate multiple candidates using true beam search algorithm
    ///
    /// This implements proper beam search that tracks cumulative probabilities
    /// at every step and keeps the globally best beam_size candidates.
    ///
    /// # Arguments
    /// * `input_tokens` - Input token sequence
    /// * `max_new_tokens` - Maximum new tokens to generate per candidate
    /// * `eos_token_id` - Optional EOS token ID to stop generation
    /// * `beam_size` - Number of candidates to keep at each step
    ///
    /// Returns candidates sorted by cumulative probability (highest first)
    pub fn generate_beam_search(
        &self,
        input_tokens: &[LlamaToken],
        max_new_tokens: usize,
        eos_token_id: Option<i32>,
        beam_size: usize,
    ) -> Result<Vec<(Vec<LlamaToken>, f32)>> {
        self.generate_beam_search_impl(input_tokens, max_new_tokens, eos_token_id, beam_size)
    }

    /// Generate multiple candidates using depth-1 beam selection followed by greedy decoding
    ///
    /// This is a simplified approach: select top-k initial tokens based on probability,
    /// then generate the rest of each sequence using greedy decoding independently.
    /// This is faster than true beam search but may miss globally optimal candidates.
    ///
    /// # Arguments
    /// * `input_tokens` - Input token sequence
    /// * `max_new_tokens` - Maximum new tokens to generate per candidate
    /// * `eos_token_id` - Optional EOS token ID to stop generation
    /// * `beam_size` - Number of candidates to generate
    ///
    /// Returns candidates sorted by initial token probability (highest first)
    pub fn generate_beam_search_d1_greedy(
        &self,
        input_tokens: &[LlamaToken],
        max_new_tokens: usize,
        eos_token_id: Option<i32>,
        beam_size: usize,
    ) -> Result<Vec<(Vec<LlamaToken>, f32)>> {
        self.generate_beam_search_d1_greedy_batch(
            input_tokens,
            max_new_tokens,
            eos_token_id,
            beam_size,
        )
    }

    /// Generate multiple candidates using batch inference (depth-1 beam + greedy)
    ///
    /// Uses shared KV cache for input tokens across all sequences.
    /// Selects top-k initial tokens, then generates greedily for each beam.
    fn generate_beam_search_d1_greedy_batch(
        &self,
        input_tokens: &[LlamaToken],
        max_new_tokens: usize,
        eos_token_id: Option<i32>,
        beam_size: usize,
    ) -> Result<Vec<(Vec<LlamaToken>, f32)>> {
        let backend = get_backend()?;

        // d1_greedy では候補数が多くても品質向上は限定的で、
        // beam_size が大きいほど n_batch が膨らみ n_ctx を超えてクラッシュする。
        // 実用上 5 候補あれば十分なため上限を設ける。
        let beam_size = beam_size.min(5);

        // n_batch / n_ubatch は input_len * beam_size + 生成余裕分 が必要。
        // ただし llama.cpp は n_batch > n_ctx を許容しないため、
        // n_ctx を max(self.n_ctx, batch_size) に動的に拡張する。
        let batch_size = input_tokens
            .len()
            .saturating_mul(beam_size)
            .saturating_add(max_new_tokens.saturating_add(16))
            .min(u32::MAX as usize) as u32;
        let n_ctx_needed = batch_size.max(self.n_ctx);
        let ctx_params = self
            .context_params()
            .with_n_ctx(Some(
                NonZeroU32::new(n_ctx_needed).expect("n_ctx must be non-zero"),
            ))
            .with_n_seq_max(beam_size.try_into().unwrap_or(32))
            .with_n_batch(batch_size)
            .with_n_ubatch(batch_size);

        let mut ctx = self
            .model
            .new_context(backend, ctx_params)
            .map_err(|e| KanjiError::Inference(e.into()))?;

        let model_eos = self.model.token_eos();
        let input_len = input_tokens.len();

        // Step 1: Process input tokens for ALL sequences in one batch
        // batch_size already accounts for input_len * beam_size + 64
        let mut batch = LlamaBatch::new(batch_size as usize, 1);

        for (i, token) in input_tokens.iter().enumerate() {
            for seq_id in 0..beam_size as i32 {
                let is_last = i == input_len - 1 && seq_id == 0;
                batch
                    .add(*token, i as i32, &[seq_id], is_last)
                    .map_err(|e| KanjiError::Inference(e.into()))?;
            }
        }
        ctx.decode(&mut batch)
            .map_err(|e| KanjiError::Inference(e.into()))?;

        // Step 2: Get top-k initial tokens
        let logits = ctx.get_logits();
        let (top_tokens, top_log_probs) = self.get_top_k_tokens(logits, beam_size);

        // Step 3: Initialize beam state
        let mut beam_tokens: Vec<Vec<LlamaToken>> = top_tokens.iter().map(|&t| vec![t]).collect();
        let beam_scores: Vec<f32> = top_log_probs.clone();
        let mut beam_finished: Vec<bool> = vec![false; beam_size];

        for (i, &token) in top_tokens.iter().enumerate() {
            if self.is_eos_token(token, eos_token_id, model_eos) {
                beam_finished[i] = true;
            }
        }

        // Step 4: Add initial tokens to each beam's sequence
        batch.clear();
        for (beam_idx, &token) in top_tokens.iter().enumerate() {
            if !beam_finished[beam_idx] {
                batch
                    .add(token, input_len as i32, &[beam_idx as i32], true)
                    .map_err(|e| KanjiError::Inference(e.into()))?;
            }
        }

        if batch.n_tokens() > 0 {
            ctx.decode(&mut batch)
                .map_err(|e| KanjiError::Inference(e.into()))?;
        }

        // Step 5: Generate tokens for all beams in parallel
        let mut samplers: Vec<LlamaSampler> =
            (0..beam_size).map(|_| LlamaSampler::greedy()).collect();

        let gen_start = std::time::Instant::now();

        for _step in 0..(max_new_tokens - 1) {
            if gen_start.elapsed().as_secs() >= GEN_TIMEOUT_SECS {
                tracing::warn!(
                    "d1_greedy_batch: wall-clock timeout ({:.1}s), stopping generation early",
                    gen_start.elapsed().as_secs_f32()
                );
                break;
            }
            let active_count = beam_finished.iter().filter(|&&f| !f).count();
            if active_count == 0 {
                break;
            }

            let mut active_beams: Vec<usize> = Vec::new();
            let mut new_tokens: Vec<LlamaToken> = Vec::new();

            for (beam_idx, finished) in beam_finished.iter().enumerate() {
                if *finished {
                    continue;
                }
                let logit_idx = active_beams.len() as i32;
                let new_token = samplers[beam_idx].sample(&ctx, logit_idx);
                active_beams.push(beam_idx);
                new_tokens.push(new_token);
            }

            batch.clear();
            for (i, beam_idx) in active_beams.iter().enumerate() {
                let new_token = new_tokens[i];
                if self.is_eos_token(new_token, eos_token_id, model_eos) {
                    beam_finished[*beam_idx] = true;
                } else {
                    beam_tokens[*beam_idx].push(new_token);
                    let pos = (input_len + beam_tokens[*beam_idx].len() - 1) as i32;
                    batch
                        .add(new_token, pos, &[*beam_idx as i32], true)
                        .map_err(|e| KanjiError::Inference(e.into()))?;
                }
            }

            if batch.n_tokens() > 0 {
                ctx.decode(&mut batch)
                    .map_err(|e| KanjiError::Inference(e.into()))?;
            } else {
                break;
            }
        }

        // EOS に到達した beam のみ返す（未完了 beam は途中切れテキストになる）
        let results: Vec<(Vec<LlamaToken>, f32)> = beam_tokens
            .into_iter()
            .zip(beam_scores)
            .zip(beam_finished)
            .filter_map(|((tokens, score), finished)| finished.then_some((tokens, score)))
            .collect();
        if results.is_empty() {
            tracing::warn!(
                "d1_greedy_batch: no beam reached EOS (budget={} tokens), returning no candidates",
                max_new_tokens
            );
        }
        Ok(results)
    }

    /// Internal implementation of true beam search algorithm
    ///
    /// Unlike depth-1 beam methods which select top-k initial tokens
    /// and then generate greedily, this implements proper beam search that
    /// tracks cumulative probabilities at every step and keeps the globally
    /// best beam_size candidates.
    ///
    /// # Algorithm
    ///
    /// 1. Start with top-k initial tokens as beams
    /// 2. At each step:
    ///    - For each active beam, get top-k candidate next tokens
    ///    - Score each candidate: beam_score + log_prob(new_token)
    ///    - Keep only the best beam_size candidates globally
    /// 3. Repeat until all beams reach EOS or max_new_tokens
    ///
    /// True beam search implementation without KV cache sharing.
    ///
    /// beam の選択で同一親から複数の子が残るため KV cache の seq 分岐が必要に
    /// なるが、seq コピー共有は過去に GPT-2 モデルで問題が出たため使わない。
    /// 代わりに 1 コンテキストを維持し、毎ステップ `clear_kv_cache` してから
    /// 全 beam のフル系列を **1 回の batched decode** で評価する。
    /// （旧実装は beam × step ごとに fresh context を生成してフル再デコード
    /// しており、context 生成 = KV 確保が beam_size × max_new_tokens 回走って
    /// 長文 × 大 beam で分単位の遅延 =「変換が止まる」体感の主因だった。）
    fn generate_beam_search_impl(
        &self,
        input_tokens: &[LlamaToken],
        max_new_tokens: usize,
        eos_token_id: Option<i32>,
        beam_size: usize,
    ) -> Result<Vec<(Vec<LlamaToken>, f32)>> {
        let backend = get_backend()?;
        let model_eos = self.model.token_eos();
        let input_len = input_tokens.len();

        // n_ctx / n_batch は「全 beam のフル系列を 1 decode に載せる」ぶんが必要。
        let max_seq_len = input_len.saturating_add(max_new_tokens).saturating_add(1);
        let batch_size = max_seq_len.saturating_mul(beam_size).min(u32::MAX as usize) as u32;
        let n_ctx_needed = batch_size.max(self.n_ctx);
        let ctx_params = self
            .context_params()
            .with_n_ctx(Some(
                NonZeroU32::new(n_ctx_needed).expect("n_ctx must be non-zero"),
            ))
            .with_n_seq_max(beam_size.try_into().unwrap_or(32))
            .with_n_batch(batch_size)
            .with_n_ubatch(batch_size);
        let mut ctx = self
            .model
            .new_context(backend, ctx_params)
            .map_err(|e| KanjiError::Inference(e.into()))?;
        let mut batch = LlamaBatch::new(batch_size as usize, 1);

        // Step 1: Get initial logits (input を seq 0 で 1 回だけ評価)
        for (i, token) in input_tokens.iter().enumerate() {
            let is_last = i == input_len - 1;
            batch
                .add(*token, i as i32, &[0], is_last)
                .map_err(|e| KanjiError::Inference(e.into()))?;
        }
        ctx.decode(&mut batch)
            .map_err(|e| KanjiError::Inference(e.into()))?;
        let initial_logits = ctx.get_logits().to_vec();
        let (top_tokens, top_log_probs) = self.get_top_k_tokens(&initial_logits, beam_size);

        // Initialize beams, partitioning EOS tokens into finished
        let mut beams: Vec<BeamState> = Vec::with_capacity(beam_size);
        let mut finished_beams: Vec<BeamState> = Vec::new();

        for (&token, &log_prob) in top_tokens.iter().zip(top_log_probs.iter()) {
            let beam = BeamState {
                tokens: vec![token],
                score: log_prob,
            };

            if self.is_eos_token(token, eos_token_id, model_eos) {
                finished_beams.push(beam);
            } else {
                beams.push(beam);
            }
        }

        // Expansion factor
        let expand_k = beam_size.max(4);

        // ウォールクロック制限。EOS が出ないまま走り続けると「変換が止まった」
        // 体感になるため、超過時はその時点の finished_beams のみで打ち切る。
        let gen_start = std::time::Instant::now();
        let mut timed_out = false;

        // Step 2: Main beam search loop
        'step: for _step in 0..(max_new_tokens - 1) {
            if beams.is_empty() {
                break;
            }

            // Early termination check
            if finished_beams.len() >= beam_size {
                let best_finished = finished_beams
                    .iter()
                    .map(|b| b.score)
                    .fold(f32::NEG_INFINITY, f32::max);
                let best_active = beams
                    .iter()
                    .map(|b| b.score)
                    .fold(f32::NEG_INFINITY, f32::max);
                if best_active < best_finished {
                    break;
                }
            }

            // ステップ単位（= 1 batched decode）でタイムアウトを判定する
            if gen_start.elapsed().as_secs() >= GEN_TIMEOUT_SECS {
                tracing::warn!(
                    "generate_beam_search_impl: wall-clock timeout ({:.1}s), stopping with {} finished beams",
                    gen_start.elapsed().as_secs_f32(),
                    finished_beams.len()
                );
                timed_out = true;
                break 'step;
            }

            // 全 beam のフル系列（input + beam.tokens）を別 seq に載せ、
            // 1 回の decode で各 beam の最終トークン位置の logits を得る
            ctx.clear_kv_cache();
            batch.clear();
            let mut last_logit_idx: Vec<i32> = Vec::with_capacity(beams.len());
            let mut n_added: i32 = 0;
            for (beam_idx, beam) in beams.iter().enumerate() {
                let seq = beam_idx as i32;
                for (i, token) in input_tokens.iter().enumerate() {
                    batch
                        .add(*token, i as i32, &[seq], false)
                        .map_err(|e| KanjiError::Inference(e.into()))?;
                    n_added += 1;
                }
                for (j, token) in beam.tokens.iter().enumerate() {
                    let pos = (input_len + j) as i32;
                    let is_last = j == beam.tokens.len() - 1;
                    batch
                        .add(*token, pos, &[seq], is_last)
                        .map_err(|e| KanjiError::Inference(e.into()))?;
                    n_added += 1;
                }
                last_logit_idx.push(n_added - 1);
            }
            ctx.decode(&mut batch)
                .map_err(|e| KanjiError::Inference(e.into()))?;

            // Collect candidates from all beams
            let mut candidates: Vec<BeamState> = Vec::new();

            for (beam_idx, beam) in beams.iter().enumerate() {
                let logits = ctx.get_logits_ith(last_logit_idx[beam_idx]);
                let (top_tokens, top_log_probs) = self.get_top_k_tokens(logits, expand_k);

                // Create candidates
                for (&token, &log_prob) in top_tokens.iter().zip(top_log_probs.iter()) {
                    let mut new_tokens = beam.tokens.clone();
                    new_tokens.push(token);

                    candidates.push(BeamState {
                        tokens: new_tokens,
                        score: beam.score + log_prob,
                    });
                }
            }

            // Sort and keep top beam_size candidates
            candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
            candidates.truncate(beam_size);

            // Partition into finished and active beams
            beams.clear();
            for candidate in candidates {
                let last_token = match candidate.tokens.last() {
                    Some(&t) => t,
                    None => continue,
                };

                if self.is_eos_token(last_token, eos_token_id, model_eos) {
                    finished_beams.push(candidate);
                } else {
                    beams.push(candidate);
                }
            }
        }

        // EOS に到達した beam だけを候補にする。未完了 beam は文の途中で
        // 切れたテキストなので、候補・確定に流れると「途中切れ」の異常変換
        // になる。1 つも完走していなければ空を返し、呼び出し側（convert）の
        // 読みフォールバックに委ねる。
        if finished_beams.is_empty() {
            tracing::warn!(
                "generate_beam_search_impl: no beam reached EOS (budget={} tokens, timed_out={}), returning no candidates",
                max_new_tokens,
                timed_out
            );
            return Ok(Vec::new());
        }
        let mut all_results: Vec<(Vec<LlamaToken>, f32)> = finished_beams
            .into_iter()
            .map(|b| (b.tokens, b.score))
            .collect();

        // Sort by score and take top beam_size
        all_results.sort_by(|a, b| b.1.total_cmp(&a.1));
        all_results.truncate(beam_size);

        Ok(all_results)
    }

    /// Get top-k tokens from logits with log probabilities
    fn get_top_k_tokens(&self, logits: &[f32], k: usize) -> (Vec<LlamaToken>, Vec<f32>) {
        // Convert logits to log probabilities using log-softmax
        let max_logit = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let log_sum_exp: f32 = logits
            .iter()
            .map(|&x| (x - max_logit).exp())
            .sum::<f32>()
            .ln()
            + max_logit;

        let mut token_scores: Vec<(usize, f32)> = logits
            .iter()
            .enumerate()
            .map(|(i, &x)| (i, x - log_sum_exp))
            .collect();
        token_scores.sort_by(|a, b| b.1.total_cmp(&a.1));
        token_scores.truncate(k);

        let tokens: Vec<LlamaToken> = token_scores
            .iter()
            .map(|(i, _)| LlamaToken(*i as i32))
            .collect();
        let log_probs: Vec<f32> = token_scores.iter().map(|(_, lp)| *lp).collect();

        (tokens, log_probs)
    }

    /// Check if a token is an EOS token.
    ///
    /// Uses the model's own EOS/EOG metadata rather than hardcoded token IDs.
    fn is_eos_token(
        &self,
        token: LlamaToken,
        eos_token_id: Option<i32>,
        model_eos: LlamaToken,
    ) -> bool {
        eos_token_id.is_some_and(|eos| token.0 == eos)
            || token == model_eos
            || self.model.is_eog_token(token)
    }

    /// Generate tokens with a custom sampler
    fn generate_with_sampler(
        &self,
        input_tokens: &[LlamaToken],
        max_new_tokens: usize,
        eos_token_id: Option<i32>,
        mut sampler: LlamaSampler,
    ) -> Result<Vec<LlamaToken>> {
        let backend = get_backend()?;
        let ctx_params = self.context_params();

        let mut ctx = self
            .model
            .new_context(backend, ctx_params)
            .map_err(|e| KanjiError::Inference(e.into()))?;

        let mut batch = LlamaBatch::new(input_tokens.len().max(64), 1);
        let mut generated = input_tokens.to_vec();
        for (i, token) in input_tokens.iter().enumerate() {
            let is_last = i == input_tokens.len() - 1;
            batch
                .add(*token, i as i32, &[0], is_last)
                .map_err(|e| KanjiError::Inference(e.into()))?;
        }

        ctx.decode(&mut batch)
            .map_err(|e| KanjiError::Inference(e.into()))?;

        let n_start = input_tokens.len();

        // Get model's EOS token for comparison
        let model_eos = self.model.token_eos();

        // ウォールクロック制限（GEN_TIMEOUT_SECS はモジュールレベルで定義）
        let gen_start = std::time::Instant::now();

        // Generate new tokens
        for n_cur in n_start..n_start + max_new_tokens {
            if gen_start.elapsed().as_secs() >= GEN_TIMEOUT_SECS {
                tracing::warn!(
                    "generate_with_sampler: wall-clock timeout ({:.1}s), stopping generation early",
                    gen_start.elapsed().as_secs_f32()
                );
                break;
            }

            let new_token = sampler.sample(&ctx, -1);

            // Check for EOS using the provided token ID
            if let Some(eos) = eos_token_id
                && new_token.0 == eos
            {
                break;
            }

            // Check against model's EOS token
            if new_token == model_eos {
                break;
            }

            // Check if model thinks it's end of generation
            if self.model.is_eog_token(new_token) {
                break;
            }

            generated.push(new_token);

            // Prepare next batch with just the new token
            batch.clear();
            batch
                .add(new_token, n_cur as i32, &[0], true)
                .map_err(|e| KanjiError::Inference(e.into()))?;

            ctx.decode(&mut batch)
                .map_err(|e| KanjiError::Inference(e.into()))?;
        }

        Ok(generated)
    }

    /// Get the EOS token ID from the model
    pub fn eos_token_id(&self) -> LlamaToken {
        self.model.token_eos()
    }
}

/// Reusable NLL scorer that keeps a single llama.cpp context alive.
///
/// Creating a `LlamaContext` is expensive. This struct amortizes the cost by
/// creating one context and clearing the KV cache between calls.
/// Use one `NllScorer` per thread for parallel scoring.
pub struct NllScorer<'a> {
    model: &'a LlamaCppModel,
    ctx: llama_cpp_2::context::LlamaContext<'a>,
    vocab_size: usize,
}

impl<'a> NllScorer<'a> {
    /// Create a new NLL scorer with a reusable context.
    pub fn new(model: &'a LlamaCppModel, n_ctx: u32) -> Result<Self> {
        let backend = get_backend()?;

        let ctx_params = LlamaContextParams::default().with_n_ctx(Some(
            NonZeroU32::new(n_ctx).expect("n_ctx must be non-zero"),
        ));

        let ctx = model
            .model
            .new_context(backend, ctx_params)
            .map_err(|e| KanjiError::Inference(e.into()))?;

        let vocab_size = model.model.n_vocab() as usize;

        Ok(Self {
            model,
            ctx,
            vocab_size,
        })
    }

    /// Compute per-character NLL for a single (reading, surface) pair.
    ///
    /// Reuses the internal context by clearing the KV cache between calls.
    pub fn compute_nll(&mut self, reading_katakana: &str, surface: &str) -> Result<f32> {
        use super::{CONTEXT_TOKEN, INPUT_START_TOKEN, OUTPUT_START_TOKEN};

        let prompt = format!(
            "{}{}{}{}{}",
            CONTEXT_TOKEN, "", INPUT_START_TOKEN, reading_katakana, OUTPUT_START_TOKEN
        );
        let full_text = format!("{}{}", prompt, surface);

        let prompt_tokens = self.model.tokenize(&prompt)?;
        let full_tokens = self.model.tokenize(&full_text)?;

        if full_tokens.len() <= prompt_tokens.len() {
            return Ok(100.0);
        }

        let n_tokens = full_tokens.len();

        self.ctx.clear_kv_cache();

        let mut batch = LlamaBatch::new(n_tokens.max(512), 1);
        batch
            .add_sequence(&full_tokens, 0, true)
            .map_err(|e| KanjiError::Inference(e.into()))?;

        self.ctx
            .decode(&mut batch)
            .map_err(|e| KanjiError::Inference(e.into()))?;

        let start_pos = prompt_tokens.len() - 1;
        let end_pos = n_tokens - 1;
        let mut total_nll: f32 = 0.0;
        let mut n_scored = 0;

        for pos in start_pos..end_pos {
            let logits = self.ctx.get_logits_ith(pos as i32);

            let max_logit = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            let log_sum_exp: f32 = logits
                .iter()
                .take(self.vocab_size)
                .map(|&x| (x - max_logit).exp())
                .sum::<f32>()
                .ln()
                + max_logit;

            let target = full_tokens[pos + 1].0 as usize;
            if target < self.vocab_size {
                total_nll -= logits[target] - log_sum_exp;
            }
            n_scored += 1;
        }

        if n_scored == 0 {
            return Ok(100.0);
        }

        let n_chars = surface.chars().count().max(1);
        Ok(total_nll / n_chars as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_byte_fallback_token() {
        assert!(is_byte_fallback_token("<0xCE>"));
        assert!(is_byte_fallback_token("<0x00>"));
        assert!(is_byte_fallback_token("<0xFF>"));
        assert!(!is_byte_fallback_token("<s>"));
        assert!(!is_byte_fallback_token("</s>"));
        assert!(!is_byte_fallback_token("<unk>"));
        assert!(!is_byte_fallback_token("<0xGG>"));
        assert!(!is_byte_fallback_token("<0xCE> "));
        assert!(!is_byte_fallback_token(""));
    }

    /// jinen-v2 と同じ構成のミニ tokenizer:
    /// バイトフォールバックトークンが `special: true` で登録され、
    /// decoder は ByteFallback + Fuse。
    fn byte_fallback_tokenizer() -> tokenizers::Tokenizer {
        let json = r#"{
            "version": "1.0",
            "truncation": null,
            "padding": null,
            "added_tokens": [
                {"id": 0, "content": "<s>", "single_word": false, "lstrip": false, "rstrip": false, "normalized": false, "special": true},
                {"id": 1, "content": "</s>", "single_word": false, "lstrip": false, "rstrip": false, "normalized": false, "special": true},
                {"id": 2, "content": "<0xCE>", "single_word": false, "lstrip": false, "rstrip": false, "normalized": false, "special": true},
                {"id": 3, "content": "<0xA8>", "single_word": false, "lstrip": false, "rstrip": false, "normalized": false, "special": true}
            ],
            "normalizer": null,
            "pre_tokenizer": null,
            "post_processor": null,
            "decoder": {"type": "Sequence", "decoders": [{"type": "ByteFallback"}, {"type": "Fuse"}]},
            "model": {
                "type": "BPE",
                "dropout": null,
                "unk_token": null,
                "continuing_subword_prefix": null,
                "end_of_word_suffix": null,
                "fuse_unk": false,
                "byte_fallback": true,
                "ignore_merges": false,
                "vocab": {"<s>": 0, "</s>": 1, "<0xCE>": 2, "<0xA8>": 3, "\u96e3": 4},
                "merges": []
            }
        }"#;
        tokenizers::Tokenizer::from_bytes(json.as_bytes()).expect("valid tokenizer json")
    }

    #[test]
    fn test_non_byte_special_token_ids_excludes_byte_fallback() {
        let tok = byte_fallback_tokenizer();
        let ids = non_byte_special_token_ids(&tok);
        assert!(ids.contains(&0), "<s> は skip 対象");
        assert!(ids.contains(&1), "</s> は skip 対象");
        assert!(!ids.contains(&2), "<0xCE> は skip してはいけない");
        assert!(!ids.contains(&3), "<0xA8> は skip してはいけない");
    }

    /// 修正の本体: special フィルタを自前 ID フィルタで行い、
    /// tokenizers には skip_special_tokens=false で渡すと
    /// バイトフォールバック 2 トークンから Ψ (U+03A8) が復元される。
    #[test]
    fn test_decode_restores_psi_from_byte_fallback() {
        let tok = byte_fallback_tokenizer();
        let special = non_byte_special_token_ids(&tok);
        // <s> <0xCE> <0xA8> 難 </s>  →  "Ψ難"
        let ids: Vec<u32> = [0u32, 2, 3, 4, 1]
            .iter()
            .copied()
            .filter(|id| !special.contains(id))
            .collect();
        let text = tok.decode(&ids, false).expect("decode");
        assert_eq!(text, "Ψ難");
    }

    /// 旧実装の再現: tokenizers に skip_special_tokens=true を委譲すると
    /// バイトフォールバックトークンごと捨てられて Ψ が消える。
    /// （この挙動が変わったら自前フィルタは不要になっている可能性がある）
    #[test]
    fn test_tokenizers_skip_special_drops_byte_fallback() {
        let tok = byte_fallback_tokenizer();
        let text = tok.decode(&[0u32, 2, 3, 4, 1], true).expect("decode");
        assert_eq!(text, "難");
    }
}
