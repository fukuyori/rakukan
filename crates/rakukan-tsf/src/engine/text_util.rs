/// ひらがな → カタカナ変換（F7）
///
/// - ひらがな → 全角カタカナ
/// - 半角カタカナ → 全角カタカナ
/// - 半角英数記号(ASCII) → 全角英数記号
/// - その他はそのまま
pub fn to_katakana(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        let c = chars[i];
        let n = c as u32;
        if (0x3041..=0x3096).contains(&n) {
            // ひらがな → カタカナ
            result.push(char::from_u32(n + 0x60).unwrap_or(c));
            i += 1;
        } else if (0xFF65..=0xFF9F).contains(&n) {
            // 半角カタカナ → 全角カタカナ（濁点・半濁点結合を処理）
            let next = chars.get(i + 1).copied();
            if let Some(kata) = half_kata_to_full(c, next) {
                result.push(kata.0);
                i += 1 + kata.1 as usize; // 結合文字を消費したら+1
            } else {
                result.push(c);
                i += 1;
            }
        } else if (0x21..=0x7E).contains(&n) {
            // 半角ASCII印字可能文字 → 全角
            result.push(char::from_u32(n - 0x21 + 0xFF01).unwrap_or(c));
            i += 1;
        } else {
            result.push(c);
            i += 1;
        }
    }
    result
}

/// ひらがな変換（F6）
///
/// - 全角カタカナ → ひらがな
/// - 半角カタカナ → ひらがな
/// - 半角英数記号(ASCII) → 全角英数記号
/// - ひらがなはそのまま
/// - その他はそのまま
pub fn to_hiragana(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let len = chars.len();
    let mut i = 0;
    while i < len {
        let c = chars[i];
        let n = c as u32;
        if (0x30A1..=0x30F6).contains(&n) {
            // 全角カタカナ → ひらがな
            result.push(char::from_u32(n - 0x60).unwrap_or(c));
            i += 1;
        } else if (0xFF65..=0xFF9F).contains(&n) {
            // 半角カタカナ → 全角カタカナ → ひらがな
            let next = chars.get(i + 1).copied();
            if let Some(kata) = half_kata_to_full(c, next) {
                let n2 = kata.0 as u32;
                if (0x30A1..=0x30F6).contains(&n2) {
                    result.push(char::from_u32(n2 - 0x60).unwrap_or(kata.0));
                } else {
                    result.push(kata.0);
                }
                i += 1 + kata.1 as usize;
            } else {
                result.push(c);
                i += 1;
            }
        } else if (0x21..=0x7E).contains(&n) {
            // 半角ASCII印字可能文字 → 全角
            result.push(char::from_u32(n - 0x21 + 0xFF01).unwrap_or(c));
            i += 1;
        } else {
            result.push(c);
            i += 1;
        }
    }
    result
}

/// 半角カタカナ1文字（+ 後続の濁点/半濁点）→ 全角カタカナ1文字
/// 戻り値: (全角カタカナ, 結合文字を消費したか)
fn half_kata_to_full(c: char, next: Option<char>) -> Option<(char, bool)> {
    let dakuten  = next == Some('\u{FF9E}');
    let handaku  = next == Some('\u{FF9F}');
    let r = match c {
        'ｦ' => ('ヲ', false),
        'ｧ' => ('ァ', false), 'ｨ' => ('ィ', false), 'ｩ' => ('ゥ', false),
        'ｪ' => ('ェ', false), 'ｫ' => ('ォ', false),
        'ｬ' => ('ャ', false), 'ｭ' => ('ュ', false), 'ｮ' => ('ョ', false),
        'ｯ' => ('ッ', false), 'ｰ' => ('ー', false),
        'ｱ' => ('ア', false), 'ｲ' => ('イ', false),
        'ｳ' => if dakuten { ('ヴ', true) } else { ('ウ', false) },
        'ｴ' => ('エ', false), 'ｵ' => ('オ', false),
        'ｶ' => if dakuten { ('ガ', true) } else { ('カ', false) },
        'ｷ' => if dakuten { ('ギ', true) } else { ('キ', false) },
        'ｸ' => if dakuten { ('グ', true) } else { ('ク', false) },
        'ｹ' => if dakuten { ('ゲ', true) } else { ('ケ', false) },
        'ｺ' => if dakuten { ('ゴ', true) } else { ('コ', false) },
        'ｻ' => if dakuten { ('ザ', true) } else { ('サ', false) },
        'ｼ' => if dakuten { ('ジ', true) } else { ('シ', false) },
        'ｽ' => if dakuten { ('ズ', true) } else { ('ス', false) },
        'ｾ' => if dakuten { ('ゼ', true) } else { ('セ', false) },
        'ｿ' => if dakuten { ('ゾ', true) } else { ('ソ', false) },
        'ﾀ' => if dakuten { ('ダ', true) } else { ('タ', false) },
        'ﾁ' => if dakuten { ('ヂ', true) } else { ('チ', false) },
        'ﾂ' => if dakuten { ('ヅ', true) } else { ('ツ', false) },
        'ﾃ' => if dakuten { ('デ', true) } else { ('テ', false) },
        'ﾄ' => if dakuten { ('ド', true) } else { ('ト', false) },
        'ﾅ' => ('ナ', false), 'ﾆ' => ('ニ', false), 'ﾇ' => ('ヌ', false),
        'ﾈ' => ('ネ', false), 'ﾉ' => ('ノ', false),
        'ﾊ' => if dakuten { ('バ', true) } else if handaku { ('パ', true) } else { ('ハ', false) },
        'ﾋ' => if dakuten { ('ビ', true) } else if handaku { ('ピ', true) } else { ('ヒ', false) },
        'ﾌ' => if dakuten { ('ブ', true) } else if handaku { ('プ', true) } else { ('フ', false) },
        'ﾍ' => if dakuten { ('ベ', true) } else if handaku { ('ペ', true) } else { ('ヘ', false) },
        'ﾎ' => if dakuten { ('ボ', true) } else if handaku { ('ポ', true) } else { ('ホ', false) },
        'ﾏ' => ('マ', false), 'ﾐ' => ('ミ', false), 'ﾑ' => ('ム', false),
        'ﾒ' => ('メ', false), 'ﾓ' => ('モ', false),
        'ﾔ' => ('ヤ', false), 'ﾕ' => ('ユ', false), 'ﾖ' => ('ヨ', false),
        'ﾗ' => ('ラ', false), 'ﾘ' => ('リ', false), 'ﾙ' => ('ル', false),
        'ﾚ' => ('レ', false), 'ﾛ' => ('ロ', false),
        'ﾜ' => ('ワ', false), 'ﾝ' => ('ン', false),
        '｡' => ('。', false), '｢' => ('「', false), '｣' => ('」', false),
        '､' => ('、', false), '･' => ('・', false),
        _ => return None,
    };
    Some(r)
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

/// 半角カタカナ変換（F8）
///
/// ひらがな・全角カタカナ → 半角カタカナ（U+FF65–U+FF9F）に変換する。
/// 濁音・半濁音は基底文字 + 結合文字（ﾞ/ﾟ）の2文字に展開する。
/// 変換対象外の文字（長音符以外）はそのまま残す。
pub fn to_half_katakana(s: &str) -> String {
    // 全角カタカナ → 半角カタカナの対応テーブル
    // (全角カタカナ, 半角基底, 結合文字 or None)
    // 結合文字: ﾞ(U+FF9E) / ﾟ(U+FF9F)
    const DAKUTEN:  char = '\u{FF9E}';
    const HANDAKU:  char = '\u{FF9F}';

    fn kata_to_half(c: char) -> (char, Option<char>) {
        match c {
            'ァ' => ('ｧ', None), 'ア' => ('ｱ', None),
            'ィ' => ('ｨ', None), 'イ' => ('ｲ', None),
            'ゥ' => ('ｩ', None), 'ウ' => ('ｳ', None),
            'ェ' => ('ｪ', None), 'エ' => ('ｴ', None),
            'ォ' => ('ｫ', None), 'オ' => ('ｵ', None),
            'カ' => ('ｶ', None), 'ガ' => ('ｶ', Some(DAKUTEN)),
            'キ' => ('ｷ', None), 'ギ' => ('ｷ', Some(DAKUTEN)),
            'ク' => ('ｸ', None), 'グ' => ('ｸ', Some(DAKUTEN)),
            'ケ' => ('ｹ', None), 'ゲ' => ('ｹ', Some(DAKUTEN)),
            'コ' => ('ｺ', None), 'ゴ' => ('ｺ', Some(DAKUTEN)),
            'サ' => ('ｻ', None), 'ザ' => ('ｻ', Some(DAKUTEN)),
            'シ' => ('ｼ', None), 'ジ' => ('ｼ', Some(DAKUTEN)),
            'ス' => ('ｽ', None), 'ズ' => ('ｽ', Some(DAKUTEN)),
            'セ' => ('ｾ', None), 'ゼ' => ('ｾ', Some(DAKUTEN)),
            'ソ' => ('ｿ', None), 'ゾ' => ('ｿ', Some(DAKUTEN)),
            'タ' => ('ﾀ', None), 'ダ' => ('ﾀ', Some(DAKUTEN)),
            'チ' => ('ﾁ', None), 'ヂ' => ('ﾁ', Some(DAKUTEN)),
            'ッ' => ('ｯ', None), 'ツ' => ('ﾂ', None), 'ヅ' => ('ﾂ', Some(DAKUTEN)),
            'テ' => ('ﾃ', None), 'デ' => ('ﾃ', Some(DAKUTEN)),
            'ト' => ('ﾄ', None), 'ド' => ('ﾄ', Some(DAKUTEN)),
            'ナ' => ('ﾅ', None), 'ニ' => ('ﾆ', None), 'ヌ' => ('ﾇ', None),
            'ネ' => ('ﾈ', None), 'ノ' => ('ﾉ', None),
            'ハ' => ('ﾊ', None), 'バ' => ('ﾊ', Some(DAKUTEN)), 'パ' => ('ﾊ', Some(HANDAKU)),
            'ヒ' => ('ﾋ', None), 'ビ' => ('ﾋ', Some(DAKUTEN)), 'ピ' => ('ﾋ', Some(HANDAKU)),
            'フ' => ('ﾌ', None), 'ブ' => ('ﾌ', Some(DAKUTEN)), 'プ' => ('ﾌ', Some(HANDAKU)),
            'ヘ' => ('ﾍ', None), 'ベ' => ('ﾍ', Some(DAKUTEN)), 'ペ' => ('ﾍ', Some(HANDAKU)),
            'ホ' => ('ﾎ', None), 'ボ' => ('ﾎ', Some(DAKUTEN)), 'ポ' => ('ﾎ', Some(HANDAKU)),
            'マ' => ('ﾏ', None), 'ミ' => ('ﾐ', None), 'ム' => ('ﾑ', None),
            'メ' => ('ﾒ', None), 'モ' => ('ﾓ', None),
            'ャ' => ('ｬ', None), 'ヤ' => ('ﾔ', None),
            'ュ' => ('ｭ', None), 'ユ' => ('ﾕ', None),
            'ョ' => ('ｮ', None), 'ヨ' => ('ﾖ', None),
            'ラ' => ('ﾗ', None), 'リ' => ('ﾘ', None), 'ル' => ('ﾙ', None),
            'レ' => ('ﾚ', None), 'ロ' => ('ﾛ', None),
            'ヮ' => ('ﾜ', None), 'ワ' => ('ﾜ', None), 'ヲ' => ('ｦ', None),
            'ン' => ('ﾝ', None),
            'ヴ' => ('ｳ', Some(DAKUTEN)),
            'ー' => ('ｰ', None),
            '。' => ('｡', None), '「' => ('｢', None), '」' => ('｣', None),
            '、' => ('､', None), '・' => ('･', None),
            _ => (c, None),
        }
    }

    // ひらがな → 全角カタカナ → 半角カタカナ の2段変換
    // 全角英数記号(U+FF01–U+FF5E) → 半角ASCII も同時に処理
    let mut result = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        let n = c as u32;
        // 全角英数記号 → 半角ASCII（ＭＳーＩＭＥ → MS-IME の英数部分）
        if (0xFF01..=0xFF5E).contains(&n) {
            result.push(char::from_u32(n - 0xFF01 + 0x21).unwrap_or(c));
            continue;
        }
        // ひらがな(U+3041–U+3096)は先に全角カタカナに変換
        let kata = if (0x3041..=0x3096).contains(&n) {
            char::from_u32(n + 0x60).unwrap_or(c)
        } else {
            c
        };
        let (base, combining) = kata_to_half(kata);
        result.push(base);
        if let Some(d) = combining {
            result.push(d);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_katakana_hiragana() {
        assert_eq!(to_katakana("あいう"), "アイウ");
    }

    #[test]
    fn test_to_katakana_ascii_to_fullwidth() {
        assert_eq!(to_katakana("abc123"), "ａｂｃ１２３");
    }

    #[test]
    fn test_to_katakana_half_kata_to_full() {
        assert_eq!(to_katakana("ｶｲｷﾞ"), "カイギ");
    }

    #[test]
    fn test_to_hiragana_katakana() {
        assert_eq!(to_hiragana("アイウ"), "あいう");
    }

    #[test]
    fn test_to_hiragana_ascii_to_fullwidth() {
        assert_eq!(to_hiragana("abc123"), "ａｂｃ１２３");
    }

    #[test]
    fn test_to_hiragana_half_kata() {
        assert_eq!(to_hiragana("ｶｲｷﾞ"), "かいぎ");
    }

    #[test]
    fn test_to_full_latin() {
        assert_eq!(to_full_latin("abc"), "ａｂｃ");
    }

    #[test]
    fn test_to_half_katakana_hiragana() {
        assert_eq!(to_half_katakana("あいう"), "ｱｲｳ");
    }

    #[test]
    fn test_to_half_katakana_dakuten() {
        assert_eq!(to_half_katakana("がぎぐげご"), "ｶﾞｷﾞｸﾞｹﾞｺﾞ");
    }

    #[test]
    fn test_to_half_katakana_handakuten() {
        assert_eq!(to_half_katakana("ぱぴぷぺぽ"), "ﾊﾟﾋﾟﾌﾟﾍﾟﾎﾟ");
    }

    #[test]
    fn test_to_half_katakana_small() {
        assert_eq!(to_half_katakana("ぁっゃゅょ"), "ｧｯｬｭｮ");
    }

    #[test]
    fn test_to_half_katakana_from_kata() {
        assert_eq!(to_half_katakana("カイギ"), "ｶｲｷﾞ");
    }

    #[test]
    fn test_to_half_katakana_choon() {
        assert_eq!(to_half_katakana("ラーメン"), "ﾗｰﾒﾝ");
    }

    #[test]
    fn test_to_half_katakana_fullwidth_latin() {
        // 全角英数記号も半角に変換される
        assert_eq!(to_half_katakana("ＭＳーＩＭＥ"), "MSｰIME"); // ーは半角長音符ｰ(U+FF70)
        assert_eq!(to_half_katakana("ａｂｃ１２３"), "abc123");
    }
}
