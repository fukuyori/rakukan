//! 漢字変換（llama.cpp GGUF 推論）

mod backend;
pub mod error;
pub mod hf_download;
pub mod llamacpp;
pub mod model_config;

pub use backend::{
    Backend, ConversionConfig, KanaKanjiConverter, build_jinen_prompt, clean_model_output,
};
pub use error::KanjiError;
pub use hf_download::{
    download_gguf, get_path_by_id, get_tokenizer_path, get_tokenizer_path_by_id, get_variant_path,
};
pub use llama_cpp_2::token::LlamaToken;
pub use llamacpp::{LlamaCppModel, NllScorer};
pub use model_config::{ModelFamily, ModelRegistry, VariantConfig, registry};

/// jinen フォーマット特殊トークン
pub const CONTEXT_TOKEN: char = '\u{ee02}';
pub const INPUT_START_TOKEN: char = '\u{ee00}';
pub const OUTPUT_START_TOKEN: char = '\u{ee01}';
