//! Error types for kanji conversion

/// Errors that can occur during kanji conversion operations.
#[derive(Debug, thiserror::Error)]
pub enum KanjiError {
    #[error("unknown model variant: '{0}'")]
    UnknownVariant(String),

    #[error("download failed")]
    Download(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("model load failed")]
    ModelLoad(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("tokenizer load failed")]
    TokenizerLoad(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("inference failed")]
    Inference(#[source] Box<dyn std::error::Error + Send + Sync>),
}

pub type Result<T> = std::result::Result<T, KanjiError>;

/// エラーを `source()` の連鎖まで含めて 1 行にする（`外側: 内側: …`）。
///
/// `KanjiError` の `Display` は分類（`inference failed` など）だけで、llama.cpp などの
/// 元のエラーは `#[source]` に入る。`{e}` でログに書くと元のエラーが消えるので、
/// 原因を残したいログではこれを使う。
pub fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        out.push_str(": ");
        out.push_str(&s.to_string());
        cur = s.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_chain_includes_the_source() {
        let e = KanjiError::Inference("llama_decode returned -3".into());
        assert_eq!(
            error_chain(&e),
            "inference failed: llama_decode returned -3"
        );
    }

    #[test]
    fn error_chain_without_source_is_the_display() {
        let e = KanjiError::UnknownVariant("x".into());
        assert_eq!(error_chain(&e), "unknown model variant: 'x'");
    }

    #[test]
    fn error_chain_follows_nested_sources() {
        #[derive(Debug, thiserror::Error)]
        #[error("outer")]
        struct Outer(#[source] std::io::Error);
        let inner = std::io::Error::other("inner");
        let e = KanjiError::Inference(Box::new(Outer(inner)));
        assert_eq!(error_chain(&e), "inference failed: outer: inner");
    }
}
