/// ひらがな → カタカナ変換（F7）
pub fn to_katakana(s: &str) -> String {
    s.chars()
        .map(|c| {
            let n = c as u32;
            // ひらがな: U+3041–U+3096 → カタカナ: U+30A1–U+30F6
            if (0x3041..=0x3096).contains(&n) {
                char::from_u32(n + 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// カタカナ → ひらがな変換
#[allow(dead_code)]
pub fn to_hiragana(s: &str) -> String {
    s.chars()
        .map(|c| {
            let n = c as u32;
            if (0x30A1..=0x30F6).contains(&n) {
                char::from_u32(n - 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// 全角英数字変換（F9）
pub fn to_full_latin(s: &str) -> String {
    s.chars()
        .map(|c| {
            let n = c as u32;
            // ASCII 印字可能文字 U+0021–U+007E → 全角 U+FF01–U+FF5E
            if (0x21..=0x7E).contains(&n) {
                char::from_u32(n - 0x21 + 0xFF01).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// 半角英数字変換（F10）
pub fn to_half_latin(s: &str) -> String {
    s.chars()
        .map(|c| {
            let n = c as u32;
            if (0xFF01..=0xFF5E).contains(&n) {
                char::from_u32(n - 0xFF01 + 0x21).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// 半角カタカナ変換（F8）- Phase 3 で完全実装
pub fn to_half_katakana(s: &str) -> String {
    // 簡易実装: 全角カタカナのまま返す
    // Phase 3: 各文字を半角カタカナ + 濁点/半濁点結合文字にマッピング
    to_katakana(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_katakana() {
        assert_eq!(to_katakana("あいう"), "アイウ");
    }

    #[test]
    fn test_to_hiragana() {
        assert_eq!(to_hiragana("アイウ"), "あいう");
    }

    #[test]
    fn test_to_full_latin() {
        assert_eq!(to_full_latin("abc"), "ａｂｃ");
    }
}
