//! Segments モデル型定義
//!
//! rakukan-engine-abi と同じ型を engine 側でも定義する。
//! engine は cdylib としてビルドされるため engine-abi には依存しない。
//! RPC レイヤーで JSON を介して相互変換される。

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum CandidateSource {
    Llm,
    Dict,
    History,
    Digit,
    Literal,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Candidate {
    pub surface: String,
    pub source: CandidateSource,
    pub annotation: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Segment {
    pub reading: String,
    pub candidates: Vec<Candidate>,
    pub selected: usize,
    pub fixed: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Segments {
    pub segments: Vec<Segment>,
    pub history_size: usize,
    pub focused: usize,
}
