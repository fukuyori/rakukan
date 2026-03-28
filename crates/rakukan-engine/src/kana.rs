use unicode_normalization::UnicodeNormalization;

/// Apply NFKC normalization to text.
///
/// This is needed for models whose tokenizer does NOT support full-width ASCII
/// characters in its vocabulary. Without NFKC normalization, characters like
/// `（`, `）`, `！`, `？` are incorrectly tokenized as EOS tokens, causing
/// generation to stop prematurely.
///
/// NFKC normalization converts:
/// - Full-width ASCII → Half-width: `（` → `(`, `！` → `!`, `？` → `?`
/// - Full-width digits → Half-width: `０` → `0`, `１` → `1`
/// - Compatibility characters → Canonical forms
///
/// Note: Hiragana, Katakana, and Kanji are NOT affected by NFKC normalization.
/// The special jinen tokens (U+EE00-U+EE02) in Private Use Area are also preserved.
pub fn normalize_nfkc(text: &str) -> String {
    text.nfkc().collect()
}

/// Convert hiragana to katakana
pub fn hiragana_to_katakana(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            // Hiragana range (U+3041-U+3096) -> Katakana (U+30A1-U+30F6)
            '\u{3041}'..='\u{3096}' => std::char::from_u32(c as u32 + 0x60).unwrap_or(c),
            _ => c,
        })
        .collect()
}

/// Convert hiragana to half-width katakana (半角カタカナ)
pub fn hiragana_to_halfwidth_katakana(text: &str) -> String {
    // First convert to full-width katakana, then to half-width
    let katakana = hiragana_to_katakana(text);
    fullwidth_katakana_to_halfwidth(&katakana)
}

/// Convert full-width katakana to half-width katakana (半角カタカナ)
fn fullwidth_katakana_to_halfwidth(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            'ァ' => result.push('ｧ'),
            'ア' => result.push('ｱ'),
            'ィ' => result.push('ｨ'),
            'イ' => result.push('ｲ'),
            'ゥ' => result.push('ｩ'),
            'ウ' => result.push('ｳ'),
            'ェ' => result.push('ｪ'),
            'エ' => result.push('ｴ'),
            'ォ' => result.push('ｫ'),
            'オ' => result.push('ｵ'),
            'カ' => result.push('ｶ'),
            'キ' => result.push('ｷ'),
            'ク' => result.push('ｸ'),
            'ケ' => result.push('ｹ'),
            'コ' => result.push('ｺ'),
            'サ' => result.push('ｻ'),
            'シ' => result.push('ｼ'),
            'ス' => result.push('ｽ'),
            'セ' => result.push('ｾ'),
            'ソ' => result.push('ｿ'),
            'タ' => result.push('ﾀ'),
            'チ' => result.push('ﾁ'),
            'ッ' => result.push('ｯ'),
            'ツ' => result.push('ﾂ'),
            'テ' => result.push('ﾃ'),
            'ト' => result.push('ﾄ'),
            'ナ' => result.push('ﾅ'),
            'ニ' => result.push('ﾆ'),
            'ヌ' => result.push('ﾇ'),
            'ネ' => result.push('ﾈ'),
            'ノ' => result.push('ﾉ'),
            'ハ' => result.push('ﾊ'),
            'ヒ' => result.push('ﾋ'),
            'フ' => result.push('ﾌ'),
            'ヘ' => result.push('ﾍ'),
            'ホ' => result.push('ﾎ'),
            'マ' => result.push('ﾏ'),
            'ミ' => result.push('ﾐ'),
            'ム' => result.push('ﾑ'),
            'メ' => result.push('ﾒ'),
            'モ' => result.push('ﾓ'),
            'ヤ' => result.push('ﾔ'),
            'ャ' => result.push('ｬ'),
            'ユ' => result.push('ﾕ'),
            'ュ' => result.push('ｭ'),
            'ヨ' => result.push('ﾖ'),
            'ョ' => result.push('ｮ'),
            'ラ' => result.push('ﾗ'),
            'リ' => result.push('ﾘ'),
            'ル' => result.push('ﾙ'),
            'レ' => result.push('ﾚ'),
            'ロ' => result.push('ﾛ'),
            'ワ' => result.push('ﾜ'),
            'ヲ' => result.push('ｦ'),
            'ン' => result.push('ﾝ'),
            'ー' => result.push('ｰ'),
            // Dakuten (゛) decomposition: ガ→ｶﾞ etc.
            'ガ' => {
                result.push('ｶ');
                result.push('ﾞ');
            }
            'ギ' => {
                result.push('ｷ');
                result.push('ﾞ');
            }
            'グ' => {
                result.push('ｸ');
                result.push('ﾞ');
            }
            'ゲ' => {
                result.push('ｹ');
                result.push('ﾞ');
            }
            'ゴ' => {
                result.push('ｺ');
                result.push('ﾞ');
            }
            'ザ' => {
                result.push('ｻ');
                result.push('ﾞ');
            }
            'ジ' => {
                result.push('ｼ');
                result.push('ﾞ');
            }
            'ズ' => {
                result.push('ｽ');
                result.push('ﾞ');
            }
            'ゼ' => {
                result.push('ｾ');
                result.push('ﾞ');
            }
            'ゾ' => {
                result.push('ｿ');
                result.push('ﾞ');
            }
            'ダ' => {
                result.push('ﾀ');
                result.push('ﾞ');
            }
            'ヂ' => {
                result.push('ﾁ');
                result.push('ﾞ');
            }
            'ヅ' => {
                result.push('ﾂ');
                result.push('ﾞ');
            }
            'デ' => {
                result.push('ﾃ');
                result.push('ﾞ');
            }
            'ド' => {
                result.push('ﾄ');
                result.push('ﾞ');
            }
            'バ' => {
                result.push('ﾊ');
                result.push('ﾞ');
            }
            'ビ' => {
                result.push('ﾋ');
                result.push('ﾞ');
            }
            'ブ' => {
                result.push('ﾌ');
                result.push('ﾞ');
            }
            'ベ' => {
                result.push('ﾍ');
                result.push('ﾞ');
            }
            'ボ' => {
                result.push('ﾎ');
                result.push('ﾞ');
            }
            'ヴ' => {
                result.push('ｳ');
                result.push('ﾞ');
            }
            // Handakuten (゜) decomposition: パ→ﾊﾟ etc.
            'パ' => {
                result.push('ﾊ');
                result.push('ﾟ');
            }
            'ピ' => {
                result.push('ﾋ');
                result.push('ﾟ');
            }
            'プ' => {
                result.push('ﾌ');
                result.push('ﾟ');
            }
            'ペ' => {
                result.push('ﾍ');
                result.push('ﾟ');
            }
            'ポ' => {
                result.push('ﾎ');
                result.push('ﾟ');
            }
            // Katakana punctuation
            '。' => result.push('｡'),
            '「' => result.push('｢'),
            '」' => result.push('｣'),
            '、' => result.push('､'),
            '・' => result.push('･'),
            // Non-katakana: pass through
            _ => result.push(c),
        }
    }
    result
}

/// Convert katakana to hiragana
pub fn katakana_to_hiragana(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            // Katakana range (U+30A1-U+30F6) -> Hiragana (U+3041-U+3096)
            '\u{30A1}'..='\u{30F6}' => std::char::from_u32(c as u32 - 0x60).unwrap_or(c),
            _ => c,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hiragana_to_katakana() {
        assert_eq!(hiragana_to_katakana("あいうえお"), "アイウエオ");
        assert_eq!(hiragana_to_katakana("こんにちは"), "コンニチハ");
        assert_eq!(hiragana_to_katakana("きゃきゅきょ"), "キャキュキョ");
        assert_eq!(hiragana_to_katakana("がぎぐげご"), "ガギグゲゴ");
        assert_eq!(hiragana_to_katakana("ぱぴぷぺぽ"), "パピプペポ");

        // Mixed with non-hiragana should pass through
        assert_eq!(hiragana_to_katakana("abc123"), "abc123");
        assert_eq!(hiragana_to_katakana("あいうabc"), "アイウabc");
    }

    #[test]
    fn test_katakana_to_hiragana() {
        assert_eq!(katakana_to_hiragana("アイウエオ"), "あいうえお");
        assert_eq!(katakana_to_hiragana("コンニチハ"), "こんにちは");
        assert_eq!(katakana_to_hiragana("キャキュキョ"), "きゃきゅきょ");
    }

    #[test]
    fn test_round_trip() {
        let original = "こんにちは";
        let katakana = hiragana_to_katakana(original);
        let back = katakana_to_hiragana(&katakana);
        assert_eq!(original, back);
    }

    #[test]
    fn test_hiragana_to_halfwidth_katakana() {
        assert_eq!(hiragana_to_halfwidth_katakana("あいうえお"), "ｱｲｳｴｵ");
        assert_eq!(hiragana_to_halfwidth_katakana("かきくけこ"), "ｶｷｸｹｺ");
        assert_eq!(hiragana_to_halfwidth_katakana("がぎぐげご"), "ｶﾞｷﾞｸﾞｹﾞｺﾞ");
        assert_eq!(hiragana_to_halfwidth_katakana("ぱぴぷぺぽ"), "ﾊﾟﾋﾟﾌﾟﾍﾟﾎﾟ");
        assert_eq!(hiragana_to_halfwidth_katakana("っ"), "ｯ");
        assert_eq!(hiragana_to_halfwidth_katakana("ゃゅょ"), "ｬｭｮ");
        // Mixed: non-hiragana passes through
        assert_eq!(hiragana_to_halfwidth_katakana("あいabc"), "ｱｲabc");
        // Long vowel mark
        assert_eq!(hiragana_to_halfwidth_katakana("カー"), "ｶｰ");
    }

    #[test]
    fn test_normalize_nfkc() {
        // Full-width ASCII should be converted to half-width
        assert_eq!(normalize_nfkc("（）"), "()");
        assert_eq!(normalize_nfkc("！？"), "!?");
        assert_eq!(normalize_nfkc("Ａｂｃ"), "Abc");
        assert_eq!(normalize_nfkc("０１２３"), "0123");

        // Full-width punctuation
        assert_eq!(normalize_nfkc("、。"), "、。"); // These are NOT full-width ASCII
        assert_eq!(normalize_nfkc("「」"), "「」"); // Japanese brackets preserved

        // Hiragana, Katakana, Kanji should be preserved
        assert_eq!(normalize_nfkc("あいうえお"), "あいうえお");
        assert_eq!(normalize_nfkc("アイウエオ"), "アイウエオ");
        assert_eq!(normalize_nfkc("漢字"), "漢字");

        // Mixed text
        assert_eq!(normalize_nfkc("（カッコ）テスト！"), "(カッコ)テスト!");

        // Special jinen tokens (Private Use Area U+EE00-U+EE02) should be preserved
        assert_eq!(normalize_nfkc("\u{ee00}"), "\u{ee00}");
        assert_eq!(normalize_nfkc("\u{ee01}"), "\u{ee01}");
        assert_eq!(normalize_nfkc("\u{ee02}"), "\u{ee02}");
        assert_eq!(
            normalize_nfkc("\u{ee02}context\u{ee00}input\u{ee01}"),
            "\u{ee02}context\u{ee00}input\u{ee01}"
        );
    }
}
