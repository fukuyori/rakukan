//! Token type used by the kanji module.
//!
//! When `llama-runtime` feature is enabled, this is re-exported from `llama-cpp-2`.
//! When disabled, we provide a lightweight stand-in so the rest of the engine can
//! compile without building llama.cpp.

#[cfg(feature = "llama-runtime")]
pub use llama_cpp_2::token::LlamaToken;

#[cfg(not(feature = "llama-runtime"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LlamaToken(pub i32);
