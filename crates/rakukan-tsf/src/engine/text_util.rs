/// ASCII 記号 → 全角記号マッピング（F6/F7 用）
///
/// `,` `[` `]` `\` は和文対応の全角記号に変換。
/// `.` は `。` ではなく `．`（全角ピリオド）に変換（F6/F7 は文字種変換であり句読点変換ではない）。
/// `-` は `－`（全角ハイフン）に変換（長音符は文脈依存なので F6/F7 では扱わない）。
/// その他の ASCII 印字可能文字は U+FF01–U+FF5E の全角対応文字に変換。
pub(crate) fn ascii_to_fullwidth_symbol(c: char) -> char {
    match c {
        ',' => '、',
        '.' => '。',
        '[' => '「',
        ']' => '」',
        '\x5C' | '\u{A5}' => '\u{FFE5}',
        '-' => '－',
        _ => {
            let n = c as u32;
            if (0x21..=0x7E).contains(&n) {
                char::from_u32(n - 0x21 + 0xFF01).unwrap_or(c)
            } else {
                c
            }
        }
    }
}

/// 入力時に再変換を起動せず、未確定 composition へ直接追加する記号。
///
/// `-` は長音符入力としてローマ字ルールに委ねるため、ここでは扱わない。
pub(crate) fn direct_input_symbol(c: char) -> Option<char> {
    match c {
        ',' => Some('、'),
        '.' => Some('。'),
        '/' => Some('・'),
        '[' => Some('「'),
        ']' => Some('」'),
        '?' => Some('？'),
        '!' => Some('！'),
        '~' => Some('〜'),
        '\x5C' | '\u{A5}' => Some('\u{FFE5}'),
        '、' | '。' | '「' | '」' | '・' | '？' | '！' | '〜' | '\u{FFE5}' => Some(c),
        '-' => None,
        _ if c.is_ascii_graphic() && !c.is_ascii_alphanumeric() => {
            Some(ascii_to_fullwidth_symbol(c))
        }
        _ if is_kuten(c) => Some(c),
        _ => None,
    }
}

/// 全角記号 → ASCII 記号マッピング（F8/F10 用）
pub(crate) fn fullwidth_symbol_to_ascii(c: char) -> char {
    match c {
        '、' => ',',
        '。' => '.',
        '・' => '/',
        '「' => '[',
        '」' => ']',
        '\u{FFE5}' => '\x5C',
        '－' => '-',
        'ー' => '-', // F10: 長音符 → 半角ハイフン
        '，' => ',',
        '．' => '.',
        '［' => '[',
        '］' => ']',
        _ => {
            let n = c as u32;
            if (0xFF01..=0xFF5E).contains(&n) {
                char::from_u32(n - 0xFF01 + 0x21).unwrap_or(c)
            } else {
                c
            }
        }
    }
}

/// 全角記号 → 半角カタカナ対応記号マッピング（F8 用）
pub(crate) fn fullwidth_symbol_to_hankaku(c: char) -> char {
    match c {
        '、' => '､',
        '。' => '｡',
        '「' => '｢',
        '」' => '｣',
        '\u{FFE5}' => '\x5C',
        '－' => '-',
        'ー' => 'ｰ', // F8: 長音符 → 半角長音符
        _ => {
            let n = c as u32;
            if (0xFF01..=0xFF5E).contains(&n) {
                char::from_u32(n - 0xFF01 + 0x21).unwrap_or(c)
            } else {
                c
            }
        }
    }
}

/// `full = prefix + suffix` を前提に suffix 部分を返す。
///
/// ライブ変換の preedit は `reading + pending_romaji` 構成なので `strip_prefix`
/// を優先して使う。前提が崩れていた場合は panic せず空文字へ倒し、
/// 観測用に debug ログだけ残す。
pub(crate) fn suffix_after_prefix_or_empty<'a>(full: &'a str, prefix: &str, op: &str) -> &'a str {
    if prefix.is_empty() {
        return full;
    }

    if let Some(suffix) = full.strip_prefix(prefix) {
        return suffix;
    }

    tracing::debug!(
        "{op}: prefix mismatch while extracting suffix full={full:?} prefix={prefix:?}"
    );
    ""
}

/// ひらがな → カタカナ変換（F7）
///
/// - ひらがな → 全角カタカナ
/// - 半角カタカナ → 全角カタカナ
/// - 半角英数記号(ASCII) → 全角英数記号
/// - 全角記号はそのまま（長音符 ー は維持）
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
                i += 1 + kata.1 as usize;
            } else {
                result.push(c);
                i += 1;
            }
        } else if (0x21..=0x7E).contains(&n) {
            // 半角ASCII印字可能文字 → 全角（記号マッピング適用）
            result.push(ascii_to_fullwidth_symbol(c));
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
/// - 半角英数記号(ASCII) → 全角記号マッピング適用
/// - 全角英数 → ひらがなには変換しない（そのまま）
/// - ひらがなはそのまま
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
        } else if c == 'ー' {
            // 全角長音符 → ひらがなに対応するものはないのでそのまま
            result.push(c);
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
            // 半角ASCII印字可能文字 → 全角記号マッピング適用
            result.push(ascii_to_fullwidth_symbol(c));
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
    let dakuten = next == Some('\u{FF9E}');
    let handaku = next == Some('\u{FF9F}');
    let r = match c {
        'ｦ' => ('ヲ', false),
        'ｧ' => ('ァ', false),
        'ｨ' => ('ィ', false),
        'ｩ' => ('ゥ', false),
        'ｪ' => ('ェ', false),
        'ｫ' => ('ォ', false),
        'ｬ' => ('ャ', false),
        'ｭ' => ('ュ', false),
        'ｮ' => ('ョ', false),
        'ｯ' => ('ッ', false),
        'ｰ' => ('ー', false),
        'ｱ' => ('ア', false),
        'ｲ' => ('イ', false),
        'ｳ' => {
            if dakuten {
                ('ヴ', true)
            } else {
                ('ウ', false)
            }
        }
        'ｴ' => ('エ', false),
        'ｵ' => ('オ', false),
        'ｶ' => {
            if dakuten {
                ('ガ', true)
            } else {
                ('カ', false)
            }
        }
        'ｷ' => {
            if dakuten {
                ('ギ', true)
            } else {
                ('キ', false)
            }
        }
        'ｸ' => {
            if dakuten {
                ('グ', true)
            } else {
                ('ク', false)
            }
        }
        'ｹ' => {
            if dakuten {
                ('ゲ', true)
            } else {
                ('ケ', false)
            }
        }
        'ｺ' => {
            if dakuten {
                ('ゴ', true)
            } else {
                ('コ', false)
            }
        }
        'ｻ' => {
            if dakuten {
                ('ザ', true)
            } else {
                ('サ', false)
            }
        }
        'ｼ' => {
            if dakuten {
                ('ジ', true)
            } else {
                ('シ', false)
            }
        }
        'ｽ' => {
            if dakuten {
                ('ズ', true)
            } else {
                ('ス', false)
            }
        }
        'ｾ' => {
            if dakuten {
                ('ゼ', true)
            } else {
                ('セ', false)
            }
        }
        'ｿ' => {
            if dakuten {
                ('ゾ', true)
            } else {
                ('ソ', false)
            }
        }
        'ﾀ' => {
            if dakuten {
                ('ダ', true)
            } else {
                ('タ', false)
            }
        }
        'ﾁ' => {
            if dakuten {
                ('ヂ', true)
            } else {
                ('チ', false)
            }
        }
        'ﾂ' => {
            if dakuten {
                ('ヅ', true)
            } else {
                ('ツ', false)
            }
        }
        'ﾃ' => {
            if dakuten {
                ('デ', true)
            } else {
                ('テ', false)
            }
        }
        'ﾄ' => {
            if dakuten {
                ('ド', true)
            } else {
                ('ト', false)
            }
        }
        'ﾅ' => ('ナ', false),
        'ﾆ' => ('ニ', false),
        'ﾇ' => ('ヌ', false),
        'ﾈ' => ('ネ', false),
        'ﾉ' => ('ノ', false),
        'ﾊ' => {
            if dakuten {
                ('バ', true)
            } else if handaku {
                ('パ', true)
            } else {
                ('ハ', false)
            }
        }
        'ﾋ' => {
            if dakuten {
                ('ビ', true)
            } else if handaku {
                ('ピ', true)
            } else {
                ('ヒ', false)
            }
        }
        'ﾌ' => {
            if dakuten {
                ('ブ', true)
            } else if handaku {
                ('プ', true)
            } else {
                ('フ', false)
            }
        }
        'ﾍ' => {
            if dakuten {
                ('ベ', true)
            } else if handaku {
                ('ペ', true)
            } else {
                ('ヘ', false)
            }
        }
        'ﾎ' => {
            if dakuten {
                ('ボ', true)
            } else if handaku {
                ('ポ', true)
            } else {
                ('ホ', false)
            }
        }
        'ﾏ' => ('マ', false),
        'ﾐ' => ('ミ', false),
        'ﾑ' => ('ム', false),
        'ﾒ' => ('メ', false),
        'ﾓ' => ('モ', false),
        'ﾔ' => ('ヤ', false),
        'ﾕ' => ('ユ', false),
        'ﾖ' => ('ヨ', false),
        'ﾗ' => ('ラ', false),
        'ﾘ' => ('リ', false),
        'ﾙ' => ('ル', false),
        'ﾚ' => ('レ', false),
        'ﾛ' => ('ロ', false),
        'ﾜ' => ('ワ', false),
        'ﾝ' => ('ン', false),
        '｡' => ('。', false),
        '｢' => ('「', false),
        '｣' => ('」', false),
        '､' => ('、', false),
        '･' => ('・', false),
        _ => return None,
    };
    Some(r)
}

/// 文字列を全角英数字にする（ひらがな等はそのまま）
/// ローマ字ログに使用するため記号は全角記号マッピングを適用
/// ASCII 文字列を全角英数記号に変換（F9/F10 用）。
///
/// `,` → `，`、`.` → `．` など、純粋な ASCII→全角対応（U+FF01–U+FF5E）を使う。
/// `ascii_to_fullwidth_symbol`（F6/F7用の和文句読点変換）は経由しない。
fn ascii_to_fullwidth(s: &str) -> String {
    s.chars()
        .map(|c| {
            let n = c as u32;
            match c {
                '、' => '，',
                '。' => '．',
                '・' => '／',
                'ー' => '－',
                _ if (0x21..=0x7E).contains(&n) => char::from_u32(n - 0x21 + 0xFF01).unwrap_or(c),
                _ => c,
            }
        })
        .collect()
}

/// 文字列を半角英数字にする
fn fullwidth_to_ascii(s: &str) -> String {
    s.chars().map(|c| fullwidth_symbol_to_ascii(c)).collect()
}

/// 全角文字列を大文字化
fn fullwidth_to_upper(s: &str) -> String {
    s.chars()
        .map(|c| {
            let n = c as u32;
            // 全角小文字 ａ–ｚ (FF41–FF5A) → 全角大文字 Ａ–Ｚ (FF21–FF3A)
            if (0xFF41..=0xFF5A).contains(&n) {
                char::from_u32(n - 0x20).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// 全角文字列を先頭だけ大文字化（Ｔｅｓｕｔｏ 形式）
fn fullwidth_to_title(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => {
            let n = first as u32;
            let upper_first = if (0xFF41..=0xFF5A).contains(&n) {
                char::from_u32(n - 0x20).unwrap_or(first)
            } else {
                first
            };
            let rest: String = chars.collect();
            format!("{}{}", upper_first, fullwidth_to_lower(&rest))
        }
    }
}

fn fullwidth_to_lower(s: &str) -> String {
    s.chars()
        .map(|c| {
            let n = c as u32;
            // 全角大文字 Ａ–Ｚ (FF21–FF3A) → 全角小文字 ａ–ｚ (FF41–FF5A)
            if (0xFF21..=0xFF3A).contains(&n) {
                char::from_u32(n + 0x20).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

/// F9 サイクル状態
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LatinCycle {
    /// 全角小文字（ｔｅｓｕｔｏ）
    FullLower,
    /// 全角大文字（ＴＥＳＵＴＯ）
    FullUpper,
    /// 全角先頭大文字（Ｔｅｓｕｔｏ）
    FullTitle,
    /// 半角小文字（tesuto）
    HalfLower,
    /// 半角大文字（TESUTO）
    HalfUpper,
    /// 半角先頭大文字（Tesuto）
    HalfTitle,
}

impl LatinCycle {
    /// 現在の文字列からサイクル状態を推定する
    pub fn detect(s: &str) -> Self {
        let is_half = s
            .chars()
            .all(|c| (c as u32) < 0x80 || !(0xFF01..=0xFF5E).contains(&(c as u32)));
        let has_alpha = s.chars().any(|c| {
            c.is_ascii_alphabetic()
                || (0xFF21..=0xFF3A).contains(&(c as u32))
                || (0xFF41..=0xFF5A).contains(&(c as u32))
        });
        if !has_alpha {
            return Self::FullLower;
        }
        if is_half {
            let all_upper = s
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .all(|c| c.is_uppercase());
            let all_lower = s
                .chars()
                .filter(|c| c.is_ascii_alphabetic())
                .all(|c| c.is_lowercase());
            if all_lower {
                Self::HalfLower
            } else if all_upper {
                Self::HalfUpper
            } else {
                Self::HalfTitle
            }
        } else {
            let all_upper = s
                .chars()
                .filter(|c| (0xFF21..=0xFF5A).contains(&(*c as u32)))
                .all(|c| (0xFF21..=0xFF3A).contains(&(c as u32)));
            let all_lower = s
                .chars()
                .filter(|c| (0xFF21..=0xFF5A).contains(&(*c as u32)))
                .all(|c| (0xFF41..=0xFF5A).contains(&(c as u32)));
            if all_lower {
                Self::FullLower
            } else if all_upper {
                Self::FullUpper
            } else {
                Self::FullTitle
            }
        }
    }

    /// 次のサイクル状態（F9: 全角サイクル、F10: 半角サイクル）
    pub fn next_full(self) -> Self {
        match self {
            Self::FullLower | Self::HalfLower | Self::HalfUpper | Self::HalfTitle => {
                Self::FullUpper
            }
            Self::FullUpper => Self::FullTitle,
            Self::FullTitle => Self::FullLower,
        }
    }

    pub fn next_half(self) -> Self {
        match self {
            Self::HalfLower | Self::FullLower | Self::FullUpper | Self::FullTitle => {
                Self::HalfUpper
            }
            Self::HalfUpper => Self::HalfTitle,
            Self::HalfTitle => Self::HalfLower,
        }
    }

    /// サイクル状態を文字列に適用する
    pub fn apply(self, s: &str) -> String {
        match self {
            Self::FullLower => fullwidth_to_lower(&ascii_to_fullwidth(s)),
            Self::FullUpper => fullwidth_to_upper(&ascii_to_fullwidth(s)),
            Self::FullTitle => fullwidth_to_title(&ascii_to_fullwidth(s)),
            Self::HalfLower => fullwidth_to_ascii(s).to_lowercase(),
            Self::HalfUpper => fullwidth_to_ascii(s).to_uppercase(),
            Self::HalfTitle => {
                let half = fullwidth_to_ascii(s);
                let mut chars = half.chars();
                match chars.next() {
                    None => String::new(),
                    Some(first) => {
                        format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase())
                    }
                }
            }
        }
    }
}

/// F9: 全角英数サイクル変換（ローマ字ログから変換 or 現在の状態から次へ）
pub fn to_full_latin(s: &str) -> String {
    let cycle = LatinCycle::detect(s);
    let next = cycle.next_full();
    next.apply(s)
}

/// F10: 半角英数サイクル変換
pub fn to_half_latin(s: &str) -> String {
    let cycle = LatinCycle::detect(s);
    let next = cycle.next_half();
    next.apply(s)
}

/// F9/F10 初回変換: ローマ字ログを全角英数（小文字）に変換する
pub fn romaji_to_fullwidth_latin(romaji: &str) -> String {
    fullwidth_to_lower(&ascii_to_fullwidth(romaji))
}

/// F9/F10 初回変換: ローマ字ログを半角英数（小文字）に変換する
pub fn romaji_to_halfwidth_latin(romaji: &str) -> String {
    fullwidth_to_ascii(romaji).to_lowercase()
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
    const DAKUTEN: char = '\u{FF9E}';
    const HANDAKU: char = '\u{FF9F}';

    fn kata_to_half(c: char) -> (char, Option<char>) {
        match c {
            'ァ' => ('ｧ', None),
            'ア' => ('ｱ', None),
            'ィ' => ('ｨ', None),
            'イ' => ('ｲ', None),
            'ゥ' => ('ｩ', None),
            'ウ' => ('ｳ', None),
            'ェ' => ('ｪ', None),
            'エ' => ('ｴ', None),
            'ォ' => ('ｫ', None),
            'オ' => ('ｵ', None),
            'カ' => ('ｶ', None),
            'ガ' => ('ｶ', Some(DAKUTEN)),
            'キ' => ('ｷ', None),
            'ギ' => ('ｷ', Some(DAKUTEN)),
            'ク' => ('ｸ', None),
            'グ' => ('ｸ', Some(DAKUTEN)),
            'ケ' => ('ｹ', None),
            'ゲ' => ('ｹ', Some(DAKUTEN)),
            'コ' => ('ｺ', None),
            'ゴ' => ('ｺ', Some(DAKUTEN)),
            'サ' => ('ｻ', None),
            'ザ' => ('ｻ', Some(DAKUTEN)),
            'シ' => ('ｼ', None),
            'ジ' => ('ｼ', Some(DAKUTEN)),
            'ス' => ('ｽ', None),
            'ズ' => ('ｽ', Some(DAKUTEN)),
            'セ' => ('ｾ', None),
            'ゼ' => ('ｾ', Some(DAKUTEN)),
            'ソ' => ('ｿ', None),
            'ゾ' => ('ｿ', Some(DAKUTEN)),
            'タ' => ('ﾀ', None),
            'ダ' => ('ﾀ', Some(DAKUTEN)),
            'チ' => ('ﾁ', None),
            'ヂ' => ('ﾁ', Some(DAKUTEN)),
            'ッ' => ('ｯ', None),
            'ツ' => ('ﾂ', None),
            'ヅ' => ('ﾂ', Some(DAKUTEN)),
            'テ' => ('ﾃ', None),
            'デ' => ('ﾃ', Some(DAKUTEN)),
            'ト' => ('ﾄ', None),
            'ド' => ('ﾄ', Some(DAKUTEN)),
            'ナ' => ('ﾅ', None),
            'ニ' => ('ﾆ', None),
            'ヌ' => ('ﾇ', None),
            'ネ' => ('ﾈ', None),
            'ノ' => ('ﾉ', None),
            'ハ' => ('ﾊ', None),
            'バ' => ('ﾊ', Some(DAKUTEN)),
            'パ' => ('ﾊ', Some(HANDAKU)),
            'ヒ' => ('ﾋ', None),
            'ビ' => ('ﾋ', Some(DAKUTEN)),
            'ピ' => ('ﾋ', Some(HANDAKU)),
            'フ' => ('ﾌ', None),
            'ブ' => ('ﾌ', Some(DAKUTEN)),
            'プ' => ('ﾌ', Some(HANDAKU)),
            'ヘ' => ('ﾍ', None),
            'ベ' => ('ﾍ', Some(DAKUTEN)),
            'ペ' => ('ﾍ', Some(HANDAKU)),
            'ホ' => ('ﾎ', None),
            'ボ' => ('ﾎ', Some(DAKUTEN)),
            'ポ' => ('ﾎ', Some(HANDAKU)),
            'マ' => ('ﾏ', None),
            'ミ' => ('ﾐ', None),
            'ム' => ('ﾑ', None),
            'メ' => ('ﾒ', None),
            'モ' => ('ﾓ', None),
            'ャ' => ('ｬ', None),
            'ヤ' => ('ﾔ', None),
            'ュ' => ('ｭ', None),
            'ユ' => ('ﾕ', None),
            'ョ' => ('ｮ', None),
            'ヨ' => ('ﾖ', None),
            'ラ' => ('ﾗ', None),
            'リ' => ('ﾘ', None),
            'ル' => ('ﾙ', None),
            'レ' => ('ﾚ', None),
            'ロ' => ('ﾛ', None),
            'ヮ' => ('ﾜ', None),
            'ワ' => ('ﾜ', None),
            'ヲ' => ('ｦ', None),
            'ン' => ('ﾝ', None),
            'ヴ' => ('ｳ', Some(DAKUTEN)),
            'ー' => ('ｰ', None),
            '。' => ('｡', None),
            '「' => ('｢', None),
            '」' => ('｣', None),
            '、' => ('､', None),
            '・' => ('･', None),
            _ => (c, None),
        }
    }

    // ひらがな → 全角カタカナ → 半角カタカナ の2段変換
    // 全角記号・長音符は fullwidth_symbol_to_hankaku で変換
    let mut result = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        let n = c as u32;
        // 全角数字 (FF10–FF19) → 半角数字
        if (0xFF10..=0xFF19).contains(&n) {
            result.push(char::from_u32(n - 0xFF10 + 0x30).unwrap_or(c));
            continue;
        }
        // 全角英数記号 (FF01–FF5E) → 半角ASCII（記号は fullwidth_symbol_to_hankaku で処理）
        if (0xFF01..=0xFF5E).contains(&n) {
            result.push(fullwidth_symbol_to_hankaku(c));
            continue;
        }
        // 全角句読点・長音符・和文記号 → 半角カタカナ対応記号
        if matches!(c, '、' | '。' | '「' | '」' | '\u{FFE5}' | '－' | 'ー') {
            result.push(fullwidth_symbol_to_hankaku(c));
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

// ─── 区読点ヘルパー ──────────────────────────────────────────────────────────

/// 区読点（変換ブロックの区切り記号）か判定する。
///
/// 対象:
/// - 日本語区読点: `、` `。`
/// - 全角記号 (U+FF01–U+FF5E、全角数字・英字を除く):
///   `！` `？` `～` `（` `）` `｛` `｝` `；` `：` `＠` `＃` `＄` `％` 等
/// - ASCII 印字可能記号（数字・英字を除く）: `@` `#` `(` `)` `?` `~` 等
/// - 和文記号（かなルール由来）: `「` `」` `・`
///
/// これらは LLM に渡さない delimiter として扱う。
#[inline]
pub fn is_kuten(c: char) -> bool {
    // 日本語区読点（FF01-FF5E 外）
    if matches!(c, '、' | '。') {
        return true;
    }
    let n = c as u32;
    // 全角記号（U+FF01–U+FF5E、全角数字・英字を除く）
    if (0xFF01..=0xFF5E).contains(&n)
        && !('０'..='９').contains(&c)
        && !('Ａ'..='Ｚ').contains(&c)
        && !('ａ'..='ｚ').contains(&c)
    {
        return true;
    }
    // ASCII 印字可能記号（数字・英字を除く）
    if c.is_ascii_graphic() && !c.is_ascii_alphanumeric() {
        return true;
    }
    // 和文記号（かなルール由来: 「」・）
    matches!(c, '「' | '」' | '・')
}

/// ひらがな文字列を区読点で分割する。
///
/// 戻り値は `Vec<(reading, trailing_punct)>`:
/// - `reading`: 区読点を除いた読み文字列（空文字になる場合もある）
/// - `trailing_punct`: ブロック末尾の区読点（末尾のブロックは `None`）
///
/// 空文字列を渡すと空の Vec を返す。
///
/// # 例
/// ```
/// use rakukan_tsf::engine::text_util::split_by_punctuation;
/// assert_eq!(
///     split_by_punctuation("きのう、あめがふった。"),
///     vec![
///         ("きのう".to_string(), Some('、')),
///         ("あめがふった".to_string(), Some('。')),
///     ]
/// );
/// ```
pub fn split_by_punctuation(s: &str) -> Vec<(String, Option<char>)> {
    if s.is_empty() {
        return Vec::new();
    }
    let mut blocks: Vec<(String, Option<char>)> = Vec::new();
    let mut current = String::new();
    for c in s.chars() {
        if is_kuten(c) {
            blocks.push((std::mem::take(&mut current), Some(c)));
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        blocks.push((current, None));
    }
    blocks
}

/// 先頭・末尾の記号を変換対象外 affix として分離する。
///
/// `「かっことじ」` は `("「", "かっことじ", "」")` を返す。
/// target 内に記号が残る場合は、通常の区読点分割変換に委ねるため `None`。
pub(crate) fn split_symbol_affixes(s: &str) -> Option<(String, String, String)> {
    let mut first_target = None;
    let mut last_target_end = 0usize;

    for (idx, c) in s.char_indices() {
        if !is_kuten(c) {
            first_target.get_or_insert(idx);
            last_target_end = idx + c.len_utf8();
        }
    }

    let first_target = first_target?;
    let target = &s[first_target..last_target_end];
    if target.chars().any(is_kuten) {
        return None;
    }

    let prefix = &s[..first_target];
    let suffix = &s[last_target_end..];
    if prefix.is_empty() && suffix.is_empty() {
        return None;
    }

    Some((prefix.to_string(), target.to_string(), suffix.to_string()))
}

/// 文字列が区読点を含むかどうか。
#[inline]
pub fn contains_kuten(s: &str) -> bool {
    s.chars().any(is_kuten)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn katakana_from_hiragana() {
        assert_eq!(
            to_katakana("\u{3042}\u{3044}\u{3046}"),
            "\u{30A2}\u{30A4}\u{30A6}"
        );
    }

    #[test]
    fn katakana_symbols_fullwidth() {
        // 入力は `,.[\\]` の 5 文字。`\x5C` は単一の backslash。
        // 過去 `"\\x5C"` と書かれていたが、それは 4 文字 `\`, `x`, `5`, `C`
        // を表す Rust 文字列リテラルで意図と異なっていた。
        assert_eq!(
            to_katakana(",.[\x5C]"),
            "\u{3001}\u{3002}\u{300C}\u{FFE5}\u{300D}"
        );
        assert_eq!(
            to_katakana("abc123"),
            "\u{FF41}\u{FF42}\u{FF43}\u{FF11}\u{FF12}\u{FF13}"
        );
    }

    #[test]
    fn katakana_from_half_kata() {
        assert_eq!(
            to_katakana("\u{FF76}\u{FF72}\u{FF77}\u{FF9E}"),
            "\u{30AB}\u{30A4}\u{30AE}"
        );
    }

    #[test]
    fn hiragana_from_katakana() {
        assert_eq!(
            to_hiragana("\u{30A2}\u{30A4}\u{30A6}"),
            "\u{3042}\u{3044}\u{3046}"
        );
    }

    #[test]
    fn hiragana_symbols_fullwidth() {
        // katakana_symbols_fullwidth と同じ理由で `\x5C` は単一 backslash。
        assert_eq!(
            to_hiragana(",.[\x5C]"),
            "\u{3001}\u{3002}\u{300C}\u{FFE5}\u{300D}"
        );
    }

    #[test]
    fn hiragana_from_half_kata() {
        assert_eq!(
            to_hiragana("\u{FF76}\u{FF72}\u{FF77}\u{FF9E}"),
            "\u{304B}\u{3044}\u{304E}"
        );
    }

    #[test]
    fn half_kata_from_hiragana() {
        assert_eq!(
            to_half_katakana("\u{3042}\u{3044}\u{3046}"),
            "\u{FF71}\u{FF72}\u{FF73}"
        );
    }

    #[test]
    fn half_kata_dakuten() {
        assert_eq!(
            to_half_katakana("\u{304C}\u{304E}\u{3050}\u{3052}\u{3054}"),
            "\u{FF76}\u{FF9E}\u{FF77}\u{FF9E}\u{FF78}\u{FF9E}\u{FF79}\u{FF9E}\u{FF7A}\u{FF9E}"
        );
    }

    #[test]
    fn half_kata_handakuten() {
        assert_eq!(
            to_half_katakana("\u{3071}\u{3074}\u{3077}\u{307A}\u{307D}"),
            "\u{FF8A}\u{FF9F}\u{FF8B}\u{FF9F}\u{FF8C}\u{FF9F}\u{FF8D}\u{FF9F}\u{FF8E}\u{FF9F}"
        );
    }

    #[test]
    fn half_kata_small() {
        assert_eq!(
            to_half_katakana("\u{3041}\u{3063}\u{3083}\u{3085}\u{3087}"),
            "\u{FF67}\u{FF6F}\u{FF6C}\u{FF6D}\u{FF6E}"
        );
    }

    #[test]
    fn half_kata_from_kata() {
        assert_eq!(
            to_half_katakana("\u{30AB}\u{30A4}\u{30AE}"),
            "\u{FF76}\u{FF72}\u{FF77}\u{FF9E}"
        );
    }

    #[test]
    fn half_kata_choon() {
        assert_eq!(
            to_half_katakana("\u{30E9}\u{30FC}\u{30E1}\u{30F3}"),
            "\u{FF97}\u{FF70}\u{FF92}\u{FF9D}"
        );
    }

    #[test]
    fn half_kata_symbols() {
        assert_eq!(
            to_half_katakana("\u{3001}\u{3002}\u{300C}\u{300D}"),
            "\u{FF64}\u{FF61}\u{FF62}\u{FF63}"
        );
        assert_eq!(
            to_half_katakana("\u{FF41}\u{FF42}\u{FF43}\u{FF11}\u{FF12}\u{FF13}"),
            "abc123"
        );
        assert_eq!(to_half_katakana("\u{FFE5}"), "\x5C");
        assert_eq!(to_half_katakana("\u{FF0D}"), "-");
    }

    #[test]
    fn full_latin_cycle() {
        let s = "tesuto";
        let s1 = to_full_latin(s);
        assert_eq!(s1, "\u{FF34}\u{FF25}\u{FF33}\u{FF35}\u{FF34}\u{FF2F}");
        let s2 = to_full_latin(&s1);
        assert_eq!(s2, "\u{FF34}\u{FF45}\u{FF53}\u{FF55}\u{FF54}\u{FF4F}");
        let s3 = to_full_latin(&s2);
        assert_eq!(s3, "\u{FF54}\u{FF45}\u{FF53}\u{FF55}\u{FF54}\u{FF4F}");
        let s4 = to_full_latin(&s3);
        assert_eq!(s4, "\u{FF34}\u{FF25}\u{FF33}\u{FF35}\u{FF34}\u{FF2F}");
    }

    #[test]
    fn half_latin_cycle() {
        let s = "tesuto";
        let s1 = to_half_latin(s);
        assert_eq!(s1, "TESUTO");
        let s2 = to_half_latin(&s1);
        assert_eq!(s2, "Tesuto");
        let s3 = to_half_latin(&s2);
        assert_eq!(s3, "tesuto");
        let s4 = to_half_latin(&s3);
        assert_eq!(s4, "TESUTO");
    }

    #[test]
    fn romaji_to_full() {
        assert_eq!(
            romaji_to_fullwidth_latin("tesuto"),
            "\u{FF54}\u{FF45}\u{FF53}\u{FF55}\u{FF54}\u{FF4F}"
        );
        assert_eq!(
            romaji_to_fullwidth_latin("schedule"),
            "\u{FF53}\u{FF43}\u{FF48}\u{FF45}\u{FF44}\u{FF55}\u{FF4C}\u{FF45}"
        );
    }

    #[test]
    fn romaji_to_half() {
        assert_eq!(romaji_to_halfwidth_latin("tesuto"), "tesuto");
        assert_eq!(romaji_to_halfwidth_latin("SCHEDULE"), "schedule");
    }

    #[test]
    fn latin_convert_japanese_symbols() {
        assert_eq!(to_full_latin("、。・ー"), "，．／－");
        assert_eq!(to_half_latin("、。・ー"), ",./-");
    }

    #[test]
    fn romaji_log_latin_convert_japanese_symbols() {
        assert_eq!(romaji_to_fullwidth_latin("、。・-"), "，．／－");
        assert_eq!(romaji_to_halfwidth_latin("、。・-"), ",./-");
    }

    // ─── split_by_punctuation ─────────────────────────────────────────────────

    #[test]
    fn split_no_punctuation() {
        assert_eq!(
            split_by_punctuation("きのう"),
            vec![("きのう".to_string(), None)]
        );
    }

    #[test]
    fn split_empty_string() {
        let result: Vec<(String, Option<char>)> = split_by_punctuation("");
        assert!(result.is_empty());
    }

    #[test]
    fn split_single_punct_middle() {
        assert_eq!(
            split_by_punctuation("きのう、あめがふった"),
            vec![
                ("きのう".to_string(), Some('、')),
                ("あめがふった".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_trailing_punct() {
        assert_eq!(
            split_by_punctuation("きのう、あめがふった。"),
            vec![
                ("きのう".to_string(), Some('、')),
                ("あめがふった".to_string(), Some('。')),
            ]
        );
    }

    #[test]
    fn split_multiple_puncts() {
        assert_eq!(
            split_by_punctuation("あ、い。う"),
            vec![
                ("あ".to_string(), Some('、')),
                ("い".to_string(), Some('。')),
                ("う".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_leading_punct() {
        // 文頭の区読点 → 空 reading のブロックが先頭に来る
        assert_eq!(
            split_by_punctuation("、あめがふった"),
            vec![
                ("".to_string(), Some('、')),
                ("あめがふった".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_symbol_affixes_for_leading_symbol() {
        assert_eq!(
            split_symbol_affixes("「かっことじ"),
            Some(("「".to_string(), "かっことじ".to_string(), "".to_string()))
        );
    }

    #[test]
    fn split_symbol_affixes_for_surrounding_symbols() {
        assert_eq!(
            split_symbol_affixes("「かっことじ」"),
            Some(("「".to_string(), "かっことじ".to_string(), "」".to_string()))
        );
    }

    #[test]
    fn split_symbol_affixes_keeps_internal_punctuation_for_block_conversion() {
        assert_eq!(split_symbol_affixes("「きょう、あした」"), None);
    }

    #[test]
    fn split_punct_only() {
        assert_eq!(
            split_by_punctuation("。"),
            vec![("".to_string(), Some('。'))]
        );
    }

    #[test]
    fn split_consecutive_puncts() {
        assert_eq!(
            split_by_punctuation("あ、、い"),
            vec![
                ("あ".to_string(), Some('、')),
                ("".to_string(), Some('、')),
                ("い".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_exclamation_question() {
        assert_eq!(
            split_by_punctuation("ほんとう！そうだ？"),
            vec![
                ("ほんとう".to_string(), Some('！')),
                ("そうだ".to_string(), Some('？')),
            ]
        );
    }

    #[test]
    fn contains_kuten_true() {
        assert!(contains_kuten("きのう、あめ"));
        assert!(contains_kuten("おわり。"));
        assert!(contains_kuten("！"));
        assert!(contains_kuten("？"));
        // 全角記号
        assert!(contains_kuten("きょう（はれ）"));
        assert!(contains_kuten("タイトル～サブタイトル"));
        assert!(contains_kuten("メール；おわり"));
        // 和文記号（かなルール由来）
        assert!(contains_kuten("「にほんご」"));
        assert!(contains_kuten("なかぐろ・てすと"));
        // ASCII 印字可能記号
        assert!(contains_kuten("user@example"));
        assert!(contains_kuten("a(b)c"));
    }

    #[test]
    fn contains_kuten_false() {
        assert!(!contains_kuten("きのう"));
        assert!(!contains_kuten(""));
        assert!(!contains_kuten("hello world"));
        // 全角数字・英字は区切りではない
        assert!(!contains_kuten("２０２４"));
        assert!(!contains_kuten("Ａｂｃ"));
    }

    #[test]
    fn split_with_fullwidth_symbol() {
        // 全角括弧が区切りとして機能するかテスト
        assert_eq!(
            split_by_punctuation("きょう（てんき）よい"),
            vec![
                ("きょう".to_string(), Some('（')),
                ("てんき".to_string(), Some('）')),
                ("よい".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_with_kana_rule_symbol() {
        // かなルール由来の記号が区切りとして機能するかテスト
        assert_eq!(
            split_by_punctuation("いろは「にほんご」まとめ"),
            vec![
                ("いろは".to_string(), Some('「')),
                ("にほんご".to_string(), Some('」')),
                ("まとめ".to_string(), None),
            ]
        );
    }

    #[test]
    fn split_with_middle_dot() {
        assert_eq!(
            split_by_punctuation("とうきょう・おおさか"),
            vec![
                ("とうきょう".to_string(), Some('・')),
                ("おおさか".to_string(), None),
            ]
        );
    }
}
