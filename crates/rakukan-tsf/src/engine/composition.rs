#![allow(dead_code)]
/// TSF コンポジション（プリエディット）の状態
///
/// rakukan-engine 層のコンポジション状態を保持する。
/// TSF 層の実際のコンポジション管理は tsf/factory.rs が行う。
#[derive(Debug, Default)]
pub struct Composition {
    /// 現在表示中のプリエディット文字列
    pub preedit: String,
    /// 変換候補リスト（Phase 3 で使用）
    pub candidates: Vec<String>,
    /// 現在選択中の候補インデックス
    pub candidate_index: Option<usize>,
    /// TSF コンポジションが開始済みかどうか
    pub is_composing: bool,
}

impl Composition {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.preedit.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn set_preedit(&mut self, text: &str) {
        self.preedit = text.to_string();
        self.is_composing = !text.is_empty();
    }
}
