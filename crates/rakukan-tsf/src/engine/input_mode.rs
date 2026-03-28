/// IME 入力モード
#[derive(Default, Copy, Clone, PartialEq, Debug)]
pub enum InputMode {
    /// ひらがな入力（かなモード）
    #[default]
    Hiragana,
    /// カタカナ入力（全角）
    Katakana,
    /// 英数入力（IME パススルー）
    Alphanumeric,
}

#[allow(dead_code)]
impl InputMode {
    pub fn is_kana(&self) -> bool {
        matches!(self, Self::Hiragana | Self::Katakana)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Hiragana => "あ",
            Self::Katakana => "ア",
            Self::Alphanumeric => "A",
        }
    }
}
