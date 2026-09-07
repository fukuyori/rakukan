/// IME のオン/オフ。
///
/// rakukan の入力状態はこの 2 値だけで表す。「ひらがなモード」「英数モード」という
/// 独立した概念は持たない。
///
/// - `On`  : かな漢字変換。TSF コンパートメント `KEYBOARD_OPENCLOSE` は「開」。
/// - `Off` : 直接入力（キーをアプリへそのまま渡す）。同コンパートメントは「閉」。
///
/// 内部状態（`IMEState::ime_mode` とそのアトミック鏡）が唯一の正で、
/// コンパートメントは常にここから導出して書く。表示（言語バー、インジケーター、
/// トレイ通知）もコンパートメントではなく内部状態を見る。
#[derive(Default, Copy, Clone, PartialEq, Eq, Debug)]
pub enum ImeMode {
    #[default]
    On,
    Off,
}

impl ImeMode {
    #[inline]
    pub fn is_on(self) -> bool {
        matches!(self, Self::On)
    }

    /// コンパートメント値（開=true）から変換する。
    #[inline]
    pub fn from_open(open: bool) -> Self {
        if open { Self::On } else { Self::Off }
    }

    #[inline]
    pub fn toggled(self) -> Self {
        match self {
            Self::On => Self::Off,
            Self::Off => Self::On,
        }
    }

    /// 言語バー / インジケーターの表示文字。
    pub fn label(self) -> &'static str {
        match self {
            Self::On => "あ",
            Self::Off => "A",
        }
    }

    /// 診断イベント用の名前。
    pub fn name(self) -> &'static str {
        match self {
            Self::On => "On",
            Self::Off => "Off",
        }
    }
}
