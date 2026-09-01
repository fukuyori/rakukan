//! リテラル保護レイヤー
//!
//! reading を「数字ラン」「アルファベットラン」「記号ラン」「かなラン」に分割し、
//! LLM にはかな部分だけを渡す。数字・アルファベット・記号は原文を保持し、
//! 半角・全角の両方を候補として提示する。

use crate::kanji::KanaKanjiConverter;
#[cfg(test)]
use crate::segments::{Candidate, CandidateSource};
use crate::{DigitCandidateKind, default_digit_candidates_order};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    Digit(String),
    Alpha(String),
    Symbol(String),
    Kana(String),
}

impl Run {
    pub fn text(&self) -> &str {
        match self {
            Run::Digit(s) | Run::Alpha(s) | Run::Symbol(s) | Run::Kana(s) => s,
        }
    }

    pub fn is_literal(&self) -> bool {
        matches!(self, Run::Digit(_) | Run::Alpha(_) | Run::Symbol(_))
    }

    pub fn is_digit(&self) -> bool {
        matches!(self, Run::Digit(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharKind {
    Digit,
    Alpha,
    Symbol,
    Kana,
}

fn classify_char(c: char) -> CharKind {
    if c.is_ascii_digit() || ('０'..='９').contains(&c) {
        CharKind::Digit
    } else if c.is_ascii_alphabetic() || ('Ａ'..='Ｚ').contains(&c) || ('ａ'..='ｚ').contains(&c)
    {
        CharKind::Alpha
    } else if is_convertible_symbol(c) {
        CharKind::Symbol
    } else {
        CharKind::Kana
    }
}

fn is_convertible_symbol(c: char) -> bool {
    (c.is_ascii_graphic() && !c.is_ascii_alphanumeric())
        || (('\u{ff01}'..='\u{ff5e}').contains(&c)
            && !('０'..='９').contains(&c)
            && !('Ａ'..='Ｚ').contains(&c)
            && !('ａ'..='ｚ').contains(&c))
        // かなルール由来の和文記号（「 」 ・）は FF01-FF5E 範囲外だが
        // 変換対象外の記号として扱う
        || matches!(c, '「' | '」' | '・')
}

fn to_halfwidth_digits(s: &str) -> String {
    s.chars()
        .map(|c| {
            if ('０'..='９').contains(&c) {
                char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

fn to_fullwidth_digits(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_digit() {
                char::from_u32(c as u32 - '0' as u32 + '０' as u32).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

fn normalize_numeric_char(c: char) -> Option<char> {
    if c.is_ascii_digit() {
        Some(c)
    } else if ('０'..='９').contains(&c) {
        Some(char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c))
    } else {
        match c {
            ',' | '，' => Some(','),
            '.' | '．' => Some('.'),
            _ => None,
        }
    }
}

fn normalize_numeric_literal(s: &str) -> Option<String> {
    let normalized: String = s
        .chars()
        .map(normalize_numeric_char)
        .collect::<Option<_>>()?;
    if normalized.chars().any(|c| c.is_ascii_digit()) {
        Some(normalized)
    } else {
        None
    }
}

fn digit_to_per_digit_kanji(c: char) -> Option<char> {
    match c {
        '0' | '０' => Some('〇'),
        '1' | '１' => Some('一'),
        '2' | '２' => Some('二'),
        '3' | '３' => Some('三'),
        '4' | '４' => Some('四'),
        '5' | '５' => Some('五'),
        '6' | '６' => Some('六'),
        '7' | '７' => Some('七'),
        '8' | '８' => Some('八'),
        '9' | '９' => Some('九'),
        _ => None,
    }
}

fn kanji_to_digit(c: char) -> Option<char> {
    match c {
        '〇' | '零' => Some('0'),
        '一' | '壱' => Some('1'),
        '二' | '弐' => Some('2'),
        '三' | '参' => Some('3'),
        '四' => Some('4'),
        '五' => Some('5'),
        '六' => Some('6'),
        '七' => Some('7'),
        '八' => Some('8'),
        '九' => Some('9'),
        _ => None,
    }
}

fn kanji_digit_value(c: char) -> Option<u64> {
    kanji_to_digit(c).and_then(|d| d.to_digit(10).map(u64::from))
}

fn small_kanji_unit(c: char) -> Option<u64> {
    match c {
        '十' | '拾' => Some(10),
        '百' => Some(100),
        '千' => Some(1000),
        _ => None,
    }
}

fn large_kanji_unit(c: char) -> Option<u64> {
    match c {
        '万' => Some(10_000),
        '億' => Some(100_000_000),
        '兆' => Some(1_000_000_000_000),
        '京' => Some(10_000_000_000_000_000),
        _ => None,
    }
}

fn is_kanji_number_char(c: char) -> bool {
    kanji_digit_value(c).is_some()
        || small_kanji_unit(c).is_some()
        || large_kanji_unit(c).is_some()
        || c == '点'
}

fn parse_kanji_integer_digits(s: &str) -> Option<String> {
    let mut total = 0u64;
    let mut group = 0u64;
    let mut pending_digit: Option<u64> = None;
    let mut saw_unit = false;

    for c in s.chars() {
        if let Some(digit) = kanji_digit_value(c) {
            pending_digit = Some(digit);
        } else if let Some(unit) = small_kanji_unit(c) {
            saw_unit = true;
            let digit = pending_digit.take().unwrap_or(1);
            group = group.checked_add(digit.checked_mul(unit)?)?;
        } else {
            let unit = large_kanji_unit(c)?;
            saw_unit = true;
            let mut group_value = group;
            if let Some(digit) = pending_digit.take() {
                group_value = group_value.checked_add(digit)?;
            }
            if group_value == 0 {
                group_value = 1;
            }
            total = total.checked_add(group_value.checked_mul(unit)?)?;
            group = 0;
        }
    }

    if !saw_unit {
        return Some(s.chars().filter_map(kanji_to_digit).collect());
    }

    if let Some(digit) = pending_digit {
        group = group.checked_add(digit)?;
    }
    total.checked_add(group).map(|n| n.to_string())
}

fn parse_kanji_number_digits(s: &str) -> Option<String> {
    let (integer, decimal) = s.split_once('点').unwrap_or((s, ""));
    let mut out = parse_kanji_integer_digits(integer)?;
    if !decimal.is_empty() {
        if !decimal.chars().all(|c| kanji_digit_value(c).is_some()) {
            return None;
        }
        out.push_str(
            &decimal
                .chars()
                .filter_map(kanji_to_digit)
                .collect::<String>(),
        );
    }
    Some(out)
}

fn digit_to_daiji(c: char) -> Option<&'static str> {
    match c {
        '0' => Some("零"),
        '1' => Some("壱"),
        '2' => Some("弐"),
        '3' => Some("参"),
        '4' => Some("四"),
        '5' => Some("五"),
        '6' => Some("六"),
        '7' => Some("七"),
        '8' => Some("八"),
        '9' => Some("九"),
        _ => None,
    }
}

fn to_per_digit_kanji(s: &str) -> String {
    s.chars()
        .map(|c| digit_to_per_digit_kanji(c).unwrap_or(c))
        .collect()
}

fn digit_to_kanji(c: char) -> Option<&'static str> {
    match c {
        '0' => Some("零"),
        '1' => Some("一"),
        '2' => Some("二"),
        '3' => Some("三"),
        '4' => Some("四"),
        '5' => Some("五"),
        '6' => Some("六"),
        '7' => Some("七"),
        '8' => Some("八"),
        '9' => Some("九"),
        _ => None,
    }
}

fn to_per_digit_kanji_normalized(s: &str) -> String {
    s.chars()
        .filter_map(|c| match c {
            ',' => None,
            '.' => Some("点"),
            d if d.is_ascii_digit() => digit_to_kanji(d),
            _ => None,
        })
        .collect()
}

fn to_kanji_under_10000(n: u16, omit_leading_one: bool) -> String {
    debug_assert!(n < 10_000);
    if n == 0 {
        return String::new();
    }
    let units = [(1000, "千"), (100, "百"), (10, "十"), (1, "")];
    let mut rest = n;
    let mut out = String::new();
    for (unit, label) in units {
        let digit = rest / unit;
        rest %= unit;
        if digit == 0 {
            continue;
        }
        if unit == 1 {
            out.push_str(digit_to_kanji(char::from_digit(digit as u32, 10).unwrap()).unwrap());
        } else {
            if digit != 1 || !omit_leading_one {
                out.push_str(digit_to_kanji(char::from_digit(digit as u32, 10).unwrap()).unwrap());
            }
            out.push_str(label);
        }
    }
    out
}

fn to_daiji_under_10000(n: u16) -> String {
    debug_assert!(n < 10_000);
    if n == 0 {
        return String::new();
    }
    let units = [(1000, "千"), (100, "百"), (10, "拾"), (1, "")];
    let mut rest = n;
    let mut out = String::new();
    for (unit, label) in units {
        let digit = rest / unit;
        rest %= unit;
        if digit == 0 {
            continue;
        }
        out.push_str(digit_to_daiji(char::from_digit(digit as u32, 10).unwrap()).unwrap());
        out.push_str(label);
    }
    out
}

fn to_kanji_integer(n: u64) -> Option<String> {
    if n == 0 {
        return Some("零".into());
    }

    let groups = [
        (1_0000_0000_0000_0000_u64, "京"),
        (1_0000_0000_0000_u64, "兆"),
        (1_0000_0000_u64, "億"),
        (1_0000_u64, "万"),
        (1_u64, ""),
    ];
    let mut rest = n;
    let mut out = String::new();
    for (base, label) in groups {
        let group = rest / base;
        rest %= base;
        if group == 0 {
            continue;
        }
        if base != 1 && group == 1 {
            out.push('一');
        } else {
            out.push_str(&to_kanji_under_10000(group as u16, true));
        }
        out.push_str(label);
    }
    Some(out)
}

fn to_daiji_integer(n: u64) -> Option<String> {
    if n == 0 {
        return Some("零".into());
    }

    let groups = [
        (1_0000_0000_0000_0000_u64, "京"),
        (1_0000_0000_0000_u64, "兆"),
        (1_0000_0000_u64, "億"),
        (1_0000_u64, "万"),
        (1_u64, ""),
    ];
    let mut rest = n;
    let mut out = String::new();
    for (base, label) in groups {
        let group = rest / base;
        rest %= base;
        if group == 0 {
            continue;
        }
        if base != 1 && group == 1 {
            out.push('壱');
        } else {
            out.push_str(&to_daiji_under_10000(group as u16));
        }
        out.push_str(label);
    }
    Some(out)
}

fn to_kanji_positional(s: &str) -> Option<String> {
    let normalized = normalize_numeric_literal(s)?;
    if normalized.matches('.').count() > 1 {
        return None;
    }

    let (integer, decimal) = normalized.split_once('.').unwrap_or((&normalized, ""));
    let integer_digits: String = integer.chars().filter(|c| *c != ',').collect();
    if integer_digits.is_empty() || !integer_digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let integer_value = integer_digits.parse::<u64>().ok()?;
    let mut out = to_kanji_integer(integer_value)?;
    if !decimal.is_empty() {
        if !decimal.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        out.push('点');
        out.push_str(&to_per_digit_kanji_normalized(decimal));
    }
    Some(out)
}

fn to_daiji_positional(s: &str) -> Option<String> {
    let normalized = normalize_numeric_literal(s)?;
    if normalized.matches('.').count() > 1 {
        return None;
    }

    let (integer, decimal) = normalized.split_once('.').unwrap_or((&normalized, ""));
    let integer_digits: String = integer.chars().filter(|c| *c != ',').collect();
    if integer_digits.is_empty() || !integer_digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }

    let integer_value = integer_digits.parse::<u64>().ok()?;
    let mut out = to_daiji_integer(integer_value)?;
    if !decimal.is_empty() {
        if !decimal.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        out.push('点');
        for d in decimal.chars() {
            out.push_str(digit_to_daiji(d)?);
        }
    }
    Some(out)
}

fn push_unique(candidates: &mut Vec<String>, value: String) {
    if !candidates.contains(&value) {
        candidates.push(value);
    }
}

fn effective_digit_candidates_order(order: &[DigitCandidateKind]) -> Vec<DigitCandidateKind> {
    if order.is_empty() {
        default_digit_candidates_order()
    } else {
        order.to_vec()
    }
}

fn digit_candidates(s: &str, order: &[DigitCandidateKind]) -> Vec<String> {
    let normalized = normalize_numeric_literal(s).unwrap_or_else(|| s.to_string());
    let half = to_halfwidth_digits(&normalized);
    let full = to_fullwidth_digits(&normalized);
    let kanji = to_per_digit_kanji(&normalized);
    let mut candidates = Vec::new();
    for kind in effective_digit_candidates_order(order) {
        match kind {
            DigitCandidateKind::Arabic => push_unique(&mut candidates, half.clone()),
            DigitCandidateKind::Fullwidth => push_unique(&mut candidates, full.clone()),
            DigitCandidateKind::Positional => {
                if let Some(positional) = to_kanji_positional(&normalized) {
                    push_unique(&mut candidates, positional);
                }
            }
            DigitCandidateKind::PerDigit => push_unique(&mut candidates, kanji.clone()),
            DigitCandidateKind::Daiji => {
                if let Some(daiji) = to_daiji_positional(&normalized) {
                    push_unique(&mut candidates, daiji);
                }
            }
        }
    }
    candidates
}

#[cfg(test)]
fn digit_candidate_structs(s: &str, order: &[DigitCandidateKind]) -> Vec<Candidate> {
    let normalized = normalize_numeric_literal(s).unwrap_or_else(|| s.to_string());
    let half = to_halfwidth_digits(&normalized);
    let full = to_fullwidth_digits(&normalized);
    let kanji = to_per_digit_kanji(&normalized);
    let mut candidates = Vec::new();
    for kind in effective_digit_candidates_order(order) {
        let (surface, annotation) = match kind {
            DigitCandidateKind::Arabic => (Some(half.clone()), "半角"),
            DigitCandidateKind::Fullwidth => (Some(full.clone()), "全角"),
            DigitCandidateKind::Positional => (to_kanji_positional(&normalized), "漢数字"),
            DigitCandidateKind::PerDigit => (Some(kanji.clone()), "桁並び漢数字"),
            DigitCandidateKind::Daiji => (to_daiji_positional(&normalized), "大字"),
        };
        if let Some(surface) = surface
            && !candidates.iter().any(|c: &Candidate| c.surface == surface)
        {
            candidates.push(Candidate {
                surface,
                source: CandidateSource::Digit,
                annotation: Some(annotation.into()),
            });
        }
    }
    candidates
}

fn to_halfwidth_alpha(s: &str) -> String {
    s.chars()
        .map(|c| {
            if ('Ａ'..='Ｚ').contains(&c) {
                char::from_u32(c as u32 - 'Ａ' as u32 + 'A' as u32).unwrap_or(c)
            } else if ('ａ'..='ｚ').contains(&c) {
                char::from_u32(c as u32 - 'ａ' as u32 + 'a' as u32).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

fn to_fullwidth_alpha(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                char::from_u32(c as u32 - 'A' as u32 + 'Ａ' as u32).unwrap_or(c)
            } else if c.is_ascii_lowercase() {
                char::from_u32(c as u32 - 'a' as u32 + 'ａ' as u32).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

fn alpha_candidates(s: &str, fullwidth_first: bool) -> Vec<String> {
    let half = to_halfwidth_alpha(s);
    let full = to_fullwidth_alpha(s);
    if half == full {
        vec![half]
    } else if fullwidth_first {
        vec![full, half]
    } else {
        vec![half, full]
    }
}

#[cfg(test)]
fn alpha_candidate_structs(s: &str) -> Vec<Candidate> {
    let half = to_halfwidth_alpha(s);
    let full = to_fullwidth_alpha(s);
    if half == full {
        vec![Candidate {
            surface: half,
            source: CandidateSource::Literal,
            annotation: None,
        }]
    } else {
        vec![
            Candidate {
                surface: half,
                source: CandidateSource::Literal,
                annotation: Some("半角".into()),
            },
            Candidate {
                surface: full,
                source: CandidateSource::Literal,
                annotation: Some("全角".into()),
            },
        ]
    }
}

fn to_halfwidth_symbol(s: &str) -> String {
    s.chars()
        .map(|c| {
            if ('\u{ff01}'..='\u{ff5e}').contains(&c) {
                char::from_u32(c as u32 - 0xfee0).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

fn to_fullwidth_symbol(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_graphic() && !c.is_ascii_alphanumeric() {
                char::from_u32(c as u32 + 0xfee0).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

fn symbol_candidates(s: &str, fullwidth_first: bool) -> Vec<String> {
    let half = to_halfwidth_symbol(s);
    let full = to_fullwidth_symbol(s);
    if half == full {
        vec![half]
    } else if fullwidth_first {
        vec![full, half]
    } else {
        vec![half, full]
    }
}

#[cfg(test)]
fn symbol_candidate_structs(s: &str) -> Vec<Candidate> {
    let half = to_halfwidth_symbol(s);
    let full = to_fullwidth_symbol(s);
    if half == full {
        vec![Candidate {
            surface: half,
            source: CandidateSource::Literal,
            annotation: None,
        }]
    } else {
        vec![
            Candidate {
                surface: half,
                source: CandidateSource::Literal,
                annotation: Some("半角".into()),
            },
            Candidate {
                surface: full,
                source: CandidateSource::Literal,
                annotation: Some("全角".into()),
            },
        ]
    }
}

fn literal_candidates(
    run: &Run,
    digit_candidates_order: &[DigitCandidateKind],
    alpha_fullwidth_first: bool,
    symbol_fullwidth_first: bool,
) -> Vec<String> {
    match run {
        Run::Digit(s) => digit_candidates(s, digit_candidates_order),
        Run::Alpha(s) => alpha_candidates(s, alpha_fullwidth_first),
        Run::Symbol(s) => symbol_candidates(s, symbol_fullwidth_first),
        Run::Kana(_) => unreachable!(),
    }
}

fn half_full_literal_candidates(
    run: &Run,
    alpha_fullwidth_first: bool,
    symbol_fullwidth_first: bool,
) -> Vec<String> {
    let (half, full, fullwidth_first) = match run {
        Run::Digit(s) => (to_halfwidth_digits(s), to_fullwidth_digits(s), false),
        Run::Alpha(s) => (
            to_halfwidth_alpha(s),
            to_fullwidth_alpha(s),
            alpha_fullwidth_first,
        ),
        Run::Symbol(s) => (
            to_halfwidth_symbol(s),
            to_fullwidth_symbol(s),
            symbol_fullwidth_first,
        ),
        Run::Kana(_) => unreachable!(),
    };
    if half == full {
        vec![half]
    } else if fullwidth_first {
        vec![full, half]
    } else {
        vec![half, full]
    }
}

#[cfg(test)]
#[allow(dead_code)]
fn literal_candidate_structs(run: &Run) -> Vec<Candidate> {
    match run {
        Run::Digit(s) => digit_candidate_structs(s, &default_digit_candidates_order()),
        Run::Alpha(s) => alpha_candidate_structs(s),
        Run::Symbol(s) => symbol_candidate_structs(s),
        Run::Kana(_) => unreachable!(),
    }
}

pub fn split_by_digits(reading: &str) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut current = String::new();
    let mut current_kind = CharKind::Kana;

    for c in reading.chars() {
        let kind = classify_char(c);
        if current.is_empty() {
            current_kind = kind;
            current.push(c);
        } else if kind == current_kind {
            current.push(c);
        } else {
            let text = std::mem::take(&mut current);
            runs.push(make_run(current_kind, text));
            current_kind = kind;
            current.push(c);
        }
    }
    if !current.is_empty() {
        runs.push(make_run(current_kind, current));
    }
    runs
}

fn make_run(kind: CharKind, text: String) -> Run {
    match kind {
        CharKind::Digit => Run::Digit(text),
        CharKind::Alpha => Run::Alpha(text),
        CharKind::Symbol => Run::Symbol(text),
        CharKind::Kana => Run::Kana(text),
    }
}

/// 数字だけで構成された文字列か（算用数字・全角数字・漢数字）。
///
/// かな run の候補が数そのものになっていることがある（「じゅう」→「10」）。
/// こうした候補は、入力側の数字と一致しないかぎり数字保存の検証で落ちる。
fn is_numeric_text(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_digit() || ('０'..='９').contains(&c) || is_kanji_number_char(c))
}

/// 位取りの単位（十・百・千・万・億・兆・京）だけで構成された漢字 run か。
///
/// 「万」「千万」のように単位だけの run は、それ自体では数を表さない。
/// 数字の直後に現れた場合は「その数字に付いた単位」であって、独立した
/// 数値ではない（`5万` の `万` は 10000 という別の数ではない）。
fn is_unit_only_kanji_run(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| small_kanji_unit(c).is_some() || large_kanji_unit(c).is_some())
}

/// 入力・出力から「含まれる数」を数字列として取り出す。
///
/// 漢数字は算用数字へ正規化して比較できるようにする（`二〇二四` → `2024`）。
///
/// ただし **数字 run の直後に単位だけの漢字 run が続く場合、その単位は数値として
/// 数えない**。`5まん` → `5万` を検証するとき、素朴に数えると入力の `5` に対して
/// 出力が `5` + `10000` になり、数字が改変されたと誤判定して正しい候補を捨てて
/// しまうため。`5万円`・`5万5千`・`10万以上` のように単位のあとに語が続く場合も
/// 同じ。
///
/// 数字に隣接していない漢数字 run は従来どおり数値として解釈する。
/// `2024ねん` → `二千二十四年` が `2024` に正規化されて一致する挙動は変わらない。
fn extract_digits(s: &str) -> String {
    let mut out = String::new();
    let mut kanji_run = String::new();
    // 直前に出力した文字が数字だったか（漢字 run が数字に隣接しているかの判定用）
    let mut after_digit = false;

    let flush_kanji_run = |out: &mut String, kanji_run: &mut String, after_digit: bool| {
        if kanji_run.is_empty() {
            return;
        }
        // 数字の直後の「万」「千」などは、その数字に付いた単位として読み飛ばす
        if !(after_digit && is_unit_only_kanji_run(kanji_run))
            && let Some(digits) = parse_kanji_number_digits(kanji_run)
        {
            out.push_str(&digits);
        }
        kanji_run.clear();
    };

    for c in s.chars() {
        if c.is_ascii_digit() {
            flush_kanji_run(&mut out, &mut kanji_run, after_digit);
            out.push(c);
            after_digit = true;
        } else if ('０'..='９').contains(&c) {
            flush_kanji_run(&mut out, &mut kanji_run, after_digit);
            out.push(char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c));
            after_digit = true;
        } else if is_kanji_number_char(c) {
            kanji_run.push(c);
        } else {
            flush_kanji_run(&mut out, &mut kanji_run, after_digit);
            after_digit = false;
        }
    }
    flush_kanji_run(&mut out, &mut kanji_run, after_digit);
    out
}

pub fn verify_digits_preserved(input: &str, output: &str) -> bool {
    extract_digits(input) == extract_digits(output)
}

/// 数字 run の直後に単独で現れたとき、数詞（位取りの単位）として解釈すべき読み。
///
/// 辞書は「まん → 万」を正しく持っているが、数字が混ざる読みは
/// `convert_with_digit_protection` が run 単位で `KanaKanjiConverter::convert()`
/// を呼ぶ経路になり、そこは LLM のみで辞書を参照しない。その結果
/// 「5まん」が「5満」「5マン」になる。数字の直後という限定した条件でのみ、
/// 数詞の漢字を先頭に差し込んで救済する。
///
/// 戻り値の `bool` は「第 1 候補に置いてよいか」。
///
/// `true` は数字の直後という条件下でほぼ一意な単位。`false` は同音異義語が
/// あるもので、候補には入れるが順位は LLM の判断に譲る
/// （「3せん」= 3戦/3選、「1ちょう」= 1丁、「5じゅう」= 5重 など）。
/// 構造的に候補へ出てこないことだけを補い、どれを既定にするかは文脈を見た
/// 変換器に任せる、という切り分け。
///
/// このルールが発動するのは、**直前の run が数字 run** で、かつ かな run が
/// 数詞に完全一致する場合だけ。`5まん` のほか `だい5まん`・`3.5まん` のように
/// 前に別の run があっても効く。「5せんのしはらい」は かな run が
/// `"せんのしはらい"` になるため対象外で、文脈ごと変換器が扱う。
///
/// 完全一致のみを見るのは前方一致による破壊を避けるため。前方一致にすると
/// 「3まんが」のような読みを「3万が」に固定してしまう。
///
/// この候補は かな run の候補リストへ足して `combine_runs` に通す。以前は
/// `verify_digits_preserved` が「万」を数値 10000 と読んで「5万」を数字改変と
/// みなすため、フィルタを迂回して後から差し込む必要があったが、
/// `extract_digits()` が数字直後の単位を独立した数値として数えなくなったので
/// その回避は不要になった。
fn numeric_unit_kanji(reading: &str) -> Option<(&'static str, bool)> {
    NUMERIC_UNITS
        .iter()
        .find(|(r, _, _)| *r == reading)
        .map(|(_, kanji, promote)| (*kanji, *promote))
}

/// (読み, 漢数詞, 第 1 候補に置いてよいか)
///
/// 連濁形（`3ぜん` = 3千、`3びゃく` / `3ぴゃく` = 3百）も入れる。数字の直後に
/// 現れるこれらは数詞以外に読みようが無いが、同音語の有無は清音形に合わせる。
const NUMERIC_UNITS: [(&str, &str, bool); 9] = [
    ("まん", "万", true),
    ("おく", "億", true),
    ("ちょう", "兆", false),
    ("せん", "千", false),
    ("ぜん", "千", false),
    ("ひゃく", "百", false),
    ("びゃく", "百", false),
    ("ぴゃく", "百", false),
    ("じゅう", "十", false),
];

fn build_local_context(runs: &[Run], kana_index: usize, global_context: &str) -> String {
    let mut ctx = String::from(global_context);
    if kana_index > 0
        && let Some(run) = runs.get(kana_index - 1)
        && run.is_literal()
    {
        if !ctx.is_empty() {
            ctx.push('…');
        }
        ctx.push_str(run.text());
    }
    ctx
}

pub fn convert_with_digit_protection(
    converter: &KanaKanjiConverter,
    reading: &str,
    context: &str,
    num_candidates: usize,
    digit_candidates_order: &[DigitCandidateKind],
    alpha_fullwidth_first: bool,
    symbol_fullwidth_first: bool,
) -> crate::kanji::error::Result<Vec<String>> {
    let runs = split_by_digits(reading);

    if runs.iter().all(|r| !r.is_literal()) {
        return converter.convert(reading, context, num_candidates);
    }

    if runs.iter().all(|r| r.is_literal()) {
        let literal_str: String = runs.iter().map(|r| r.text()).collect();
        if runs.iter().all(|r| r.is_digit()) || normalize_numeric_literal(&literal_str).is_some() {
            return Ok(digit_candidates(&literal_str, digit_candidates_order));
        }
        if runs.iter().all(|r| matches!(r, Run::Alpha(_))) {
            return Ok(alpha_candidates(&literal_str, alpha_fullwidth_first));
        }
        if runs.iter().all(|r| matches!(r, Run::Symbol(_))) {
            return Ok(symbol_candidates(&literal_str, symbol_fullwidth_first));
        }
        // 数字+アルファベット+記号混在のリテラルのみ。
        // 数字の漢数字化は「数字だけ」の時に限定し、混在時は半角/全角候補を合成する。
        let run_candidates: Vec<Vec<String>> = runs
            .iter()
            .map(|r| half_full_literal_candidates(r, alpha_fullwidth_first, symbol_fullwidth_first))
            .collect();
        return Ok(combine_runs(&run_candidates, num_candidates));
    }

    let mut run_candidates: Vec<Vec<String>> = Vec::with_capacity(runs.len());
    for (i, run) in runs.iter().enumerate() {
        if run.is_literal() {
            run_candidates.push(literal_candidates(
                run,
                digit_candidates_order,
                alpha_fullwidth_first,
                symbol_fullwidth_first,
            ));
        } else if let Run::Kana(s) = run {
            let local_context = build_local_context(&runs, i, context);
            let mut cands = converter.convert(s, &local_context, num_candidates)?;
            // 数字 run の直後のかな run が数詞そのものなら、漢数詞を候補に足す。
            //
            // かな run は数字 run と切り離して変換器へ渡るため、「まん」単独では
            // 数詞と判断する手掛かりが無く「満」「マン」しか返らない。構造的に
            // 候補へ出てこない分だけをここで補い、数字表記との組み合わせ・重複
            // 排除・件数制限は combine_runs 以降の通常の経路に任せる。
            //
            // 足すのは単位の漢字 1 つだけ。数字表記（1/１/一/壱…）との総当たりを
            // ここで作ると「一十」「壱十」のような候補まで生成してしまう。
            // 組み合わせは combine_runs が第 1 表記から順に作るので、先頭に来るのは
            // 「1十」であって「一十」ではない。
            if i > 0 && runs[i - 1].is_digit() {
                if let Some((unit, promote)) = numeric_unit_kanji(s) {
                    cands.retain(|c| c != unit);
                    // promote=false（同音異義語あり）は変換器の第 1 候補を既定のまま
                    // 残し、数詞はその次に置く。候補として存在させることだけを保証する。
                    //
                    // ただし単純に 1 番目へ入れると、変換器の第 1 候補が数字だけの
                    // 表記だったときに数詞が先頭へ繰り上がってしまう。「じゅう」は
                    // 変換器が「10」を第 1 候補に返すが、これは数字保存の検証で
                    // 落ちるため（入力の数字は「1」だけ）、結果として「1十」が
                    // 先頭に立つ。落ちると分かっている候補は数えず、実際に残る
                    // 最初の候補の後ろへ置く。
                    let at = if promote {
                        0
                    } else {
                        cands
                            .iter()
                            .position(|c| !is_numeric_text(c))
                            .map(|p| p + 1)
                            .unwrap_or(0)
                            .min(cands.len())
                    };
                    cands.insert(at, unit.to_string());
                }
            }
            run_candidates.push(cands);
        }
    }

    let combined = combine_runs(&run_candidates, num_candidates);

    let mut verified: Vec<String> = combined
        .into_iter()
        .filter(|c| verify_digits_preserved(reading, c))
        .collect();

    // 数詞（「5まん」→「5万」）の救済は、かな run の候補へ漢数詞を足す形で
    // 上の run ループが行う。`extract_digits()` が数字直後の単位を独立した
    // 数値として数えなくなったため、この経路の候補も verify を素通りできる。
    // フィルタを迂回して後から差し込む必要は無くなった。

    // 重複排除と件数制限を最後に通す。
    let mut seen = std::collections::HashSet::new();
    verified.retain(|c| seen.insert(c.clone()));
    verified.truncate(num_candidates);

    if verified.is_empty() {
        Ok(vec![reading.to_string()])
    } else {
        Ok(verified)
    }
}

fn combine_runs(run_candidates: &[Vec<String>], limit: usize) -> Vec<String> {
    if run_candidates.is_empty() {
        return vec![];
    }

    let mut results: Vec<String> = vec![String::new()];

    for cands in run_candidates {
        if cands.is_empty() {
            continue;
        }
        if cands.len() == 1 {
            for r in &mut results {
                r.push_str(&cands[0]);
            }
        } else {
            let mut new_results = Vec::with_capacity(results.len() * cands.len());
            for r in &results {
                for c in cands {
                    let mut combined = r.clone();
                    combined.push_str(c);
                    new_results.push(combined);
                    if new_results.len() >= limit * 2 {
                        break;
                    }
                }
                if new_results.len() >= limit * 2 {
                    break;
                }
            }
            results = new_results;
        }
    }

    results.truncate(limit);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_no_digits() {
        let runs = split_by_digits("ねんがつにち");
        assert_eq!(runs, vec![Run::Kana("ねんがつにち".into())]);
    }

    #[test]
    fn split_only_digits() {
        let runs = split_by_digits("２０２４");
        assert_eq!(runs, vec![Run::Digit("２０２４".into())]);
    }

    #[test]
    fn split_mixed() {
        let runs = split_by_digits("２０２４ねん４がつ１０にち");
        assert_eq!(
            runs,
            vec![
                Run::Digit("２０２４".into()),
                Run::Kana("ねん".into()),
                Run::Digit("４".into()),
                Run::Kana("がつ".into()),
                Run::Digit("１０".into()),
                Run::Kana("にち".into()),
            ]
        );
    }

    #[test]
    fn split_ascii_digits() {
        let runs = split_by_digits("2024ねん");
        assert_eq!(
            runs,
            vec![Run::Digit("2024".into()), Run::Kana("ねん".into()),]
        );
    }

    #[test]
    fn split_trailing_digits() {
        let runs = split_by_digits("でんわ０９０１２３４５６７８");
        assert_eq!(
            runs,
            vec![
                Run::Kana("でんわ".into()),
                Run::Digit("０９０１２３４５６７８".into()),
            ]
        );
    }

    #[test]
    fn split_alpha_only() {
        let runs = split_by_digits("ＰＣ");
        assert_eq!(runs, vec![Run::Alpha("ＰＣ".into())]);
    }

    #[test]
    fn split_alpha_ascii() {
        let runs = split_by_digits("USB");
        assert_eq!(runs, vec![Run::Alpha("USB".into())]);
    }

    #[test]
    fn split_alpha_with_kana() {
        let runs = split_by_digits("ＰＣをかう");
        assert_eq!(
            runs,
            vec![Run::Alpha("ＰＣ".into()), Run::Kana("をかう".into()),]
        );
    }

    #[test]
    fn split_digit_alpha_kana() {
        let runs = split_by_digits("3Dぷりんたー");
        assert_eq!(
            runs,
            vec![
                Run::Digit("3".into()),
                Run::Alpha("D".into()),
                Run::Kana("ぷりんたー".into()),
            ]
        );
    }

    #[test]
    fn split_alpha_symbol_alpha() {
        let runs = split_by_digits("USB-C");
        assert_eq!(
            runs,
            vec![
                Run::Alpha("USB".into()),
                Run::Symbol("-".into()),
                Run::Alpha("C".into()),
            ]
        );
    }

    #[test]
    fn split_fullwidth_symbol() {
        let runs = split_by_digits("（test）");
        assert_eq!(
            runs,
            vec![
                Run::Symbol("（".into()),
                Run::Alpha("test".into()),
                Run::Symbol("）".into()),
            ]
        );
    }

    #[test]
    fn verify_preserved_ok() {
        assert!(verify_digits_preserved("２０２４ねん", "２０２４年"));
        assert!(verify_digits_preserved("２０２４ねん", "2024年"));
        assert!(verify_digits_preserved("２０２４ねん", "二〇二四年"));
        assert!(verify_digits_preserved("２０２４ねん", "二千二十四年"));
        assert!(verify_digits_preserved("２０２４ねん", "弐千弐拾四年"));
        assert!(verify_digits_preserved("２４００えん", "弐千四百円"));
        assert!(verify_digits_preserved("２．５", "弐点五"));
    }

    #[test]
    fn verify_preserved_ng() {
        assert!(!verify_digits_preserved("２０２４ねん", "2025年"));
        assert!(!verify_digits_preserved("１００えん", "1000円"));
    }

    #[test]
    fn verify_no_digits() {
        assert!(verify_digits_preserved("ねんがつ", "年月"));
    }

    #[test]
    fn combine_single_run() {
        let runs = vec![vec!["年".into(), "ねん".into()]];
        let result = combine_runs(&runs, 5);
        assert_eq!(result, vec!["年", "ねん"]);
    }

    #[test]
    fn combine_digit_and_kana() {
        let runs = vec![
            vec!["2024".into(), "２０２４".into(), "二〇二四".into()],
            vec!["年".into(), "ねん".into()],
        ];
        let result = combine_runs(&runs, 5);
        assert_eq!(
            result,
            vec![
                "2024年",
                "2024ねん",
                "２０２４年",
                "２０２４ねん",
                "二〇二四年"
            ]
        );
    }

    #[test]
    fn combine_multi_kana_runs() {
        let runs = vec![
            vec!["2024".into(), "２０２４".into()],
            vec!["年".into()],
            vec!["4".into(), "４".into()],
            vec!["月".into(), "がつ".into()],
        ];
        let result = combine_runs(&runs, 5);
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], "2024年4月");
    }

    #[test]
    fn digit_candidates_halfwidth_input() {
        let cands = digit_candidates("2024", &default_digit_candidates_order());
        assert_eq!(
            cands,
            vec!["2024", "２０２４", "二千二十四", "二〇二四", "弐千弐拾四"]
        );
    }

    #[test]
    fn digit_candidates_fullwidth_input() {
        let cands = digit_candidates("２０２４", &default_digit_candidates_order());
        assert_eq!(
            cands,
            vec!["2024", "２０２４", "二千二十四", "二〇二四", "弐千弐拾四"]
        );
    }

    #[test]
    fn positional_kanji_basic_numbers() {
        assert_eq!(to_kanji_positional("0").as_deref(), Some("零"));
        assert_eq!(to_kanji_positional("10").as_deref(), Some("十"));
        assert_eq!(to_kanji_positional("100").as_deref(), Some("百"));
        assert_eq!(to_kanji_positional("1000").as_deref(), Some("千"));
        assert_eq!(to_kanji_positional("10000").as_deref(), Some("一万"));
        assert_eq!(to_kanji_positional("100000").as_deref(), Some("十万"));
        assert_eq!(to_kanji_positional("1000000").as_deref(), Some("百万"));
        assert_eq!(to_kanji_positional("101").as_deref(), Some("百一"));
        assert_eq!(to_kanji_positional("1234").as_deref(), Some("千二百三十四"));
    }

    #[test]
    fn positional_kanji_with_separators() {
        assert_eq!(to_kanji_positional("2,400").as_deref(), Some("二千四百"));
        assert_eq!(to_kanji_positional("2.5").as_deref(), Some("二点五"));
        assert_eq!(
            to_kanji_positional("２，４００．５").as_deref(),
            Some("二千四百点五")
        );
    }

    #[test]
    fn daiji_kanji_basic_numbers() {
        assert_eq!(to_daiji_positional("0").as_deref(), Some("零"));
        assert_eq!(to_daiji_positional("10").as_deref(), Some("壱拾"));
        assert_eq!(to_daiji_positional("100").as_deref(), Some("壱百"));
        assert_eq!(to_daiji_positional("1000").as_deref(), Some("壱千"));
        assert_eq!(to_daiji_positional("10000").as_deref(), Some("壱万"));
        assert_eq!(
            to_daiji_positional("1234").as_deref(),
            Some("壱千弐百参拾四")
        );
    }

    #[test]
    fn digit_candidates_order_can_be_customized() {
        let order = [
            DigitCandidateKind::Daiji,
            DigitCandidateKind::Arabic,
            DigitCandidateKind::PerDigit,
        ];
        let cands = digit_candidates("1234", &order);
        assert_eq!(cands, vec!["壱千弐百参拾四", "1234", "一二三四"]);
    }

    #[test]
    fn numeric_literal_candidates_with_symbols() {
        let cands = digit_candidates("2,400.5", &default_digit_candidates_order());
        assert_eq!(
            cands,
            vec![
                "2,400.5",
                "２,４００.５",
                "二千四百点五",
                "二,四〇〇.五",
                "弐千四百点五"
            ]
        );
    }

    #[test]
    fn numeric_literal_candidates_normalize_fullwidth_punctuation() {
        let cands = digit_candidates("２，４００．５", &default_digit_candidates_order());
        assert_eq!(
            cands,
            vec![
                "2,400.5",
                "２,４００.５",
                "二千四百点五",
                "二,四〇〇.五",
                "弐千四百点五"
            ]
        );
    }

    #[test]
    fn digit_candidate_structs_has_annotations() {
        let cands = digit_candidate_structs("100", &default_digit_candidates_order());
        assert_eq!(cands.len(), 5);
        assert_eq!(cands[0].surface, "100");
        assert_eq!(cands[0].annotation.as_deref(), Some("半角"));
        assert_eq!(cands[1].surface, "１００");
        assert_eq!(cands[1].annotation.as_deref(), Some("全角"));
        assert_eq!(cands[2].surface, "百");
        assert_eq!(cands[2].annotation.as_deref(), Some("漢数字"));
        assert_eq!(cands[3].surface, "一〇〇");
        assert_eq!(cands[3].annotation.as_deref(), Some("桁並び漢数字"));
        assert_eq!(cands[4].surface, "壱百");
        assert_eq!(cands[4].annotation.as_deref(), Some("大字"));
    }

    #[test]
    fn alpha_candidates_halfwidth_first() {
        let cands = alpha_candidates("PC", false);
        assert_eq!(cands, vec!["PC", "ＰＣ"]);
    }

    #[test]
    fn alpha_candidates_fullwidth_first() {
        let cands = alpha_candidates("PC", true);
        assert_eq!(cands, vec!["ＰＣ", "PC"]);
    }

    #[test]
    fn alpha_candidate_structs_has_annotations() {
        let cands = alpha_candidate_structs("USB");
        assert_eq!(cands.len(), 2);
        assert_eq!(cands[0].surface, "USB");
        assert_eq!(cands[0].annotation.as_deref(), Some("半角"));
        assert_eq!(cands[1].surface, "ＵＳＢ");
        assert_eq!(cands[1].annotation.as_deref(), Some("全角"));
    }

    #[test]
    fn alpha_lowercase_halfwidth_first() {
        let cands = alpha_candidates("abc", false);
        assert_eq!(cands, vec!["abc", "ａｂｃ"]);
    }

    #[test]
    fn alpha_lowercase_fullwidth_first() {
        let cands = alpha_candidates("abc", true);
        assert_eq!(cands, vec!["ａｂｃ", "abc"]);
    }

    #[test]
    fn symbol_candidates_halfwidth_first() {
        let cands = symbol_candidates("+-*/", false);
        assert_eq!(cands, vec!["+-*/", "＋－＊／"]);
    }

    #[test]
    fn symbol_candidates_fullwidth_first() {
        let cands = symbol_candidates("+-*/", true);
        assert_eq!(cands, vec!["＋－＊／", "+-*/"]);
    }

    #[test]
    fn symbol_candidate_structs_has_annotations() {
        let cands = symbol_candidate_structs("@");
        assert_eq!(cands.len(), 2);
        assert_eq!(cands[0].surface, "@");
        assert_eq!(cands[0].annotation.as_deref(), Some("半角"));
        assert_eq!(cands[1].surface, "＠");
        assert_eq!(cands[1].annotation.as_deref(), Some("全角"));
    }

    #[test]
    fn combine_alpha_symbol_runs() {
        let runs = vec![
            alpha_candidates("USB", false),
            symbol_candidates("-", false),
            alpha_candidates("C", false),
        ];
        let result = combine_runs(&runs, 6);
        assert_eq!(
            result,
            vec![
                "USB-C",
                "USB-Ｃ",
                "USB－C",
                "USB－Ｃ",
                "ＵＳＢ-C",
                "ＵＳＢ-Ｃ"
            ]
        );
    }

    #[test]
    fn combine_mixed_literal_runs_without_kanji_digits() {
        let runs = split_by_digits("3D-C");
        let run_candidates: Vec<Vec<String>> = runs
            .iter()
            .map(|r| half_full_literal_candidates(r, false, false))
            .collect();
        let result = combine_runs(&run_candidates, 6);
        assert_eq!(
            result,
            vec!["3D-C", "3D-Ｃ", "3D－C", "3D－Ｃ", "3Ｄ-C", "3Ｄ-Ｃ"]
        );
        assert!(!result.iter().any(|s| s.contains('三')));
    }

    #[test]
    fn combine_respects_limit() {
        let runs = vec![
            vec!["A".into(), "B".into(), "C".into()],
            vec!["1".into(), "2".into(), "3".into()],
        ];
        let result = combine_runs(&runs, 3);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn digit_followed_by_unit_kanji_passes_verification() {
        // 数字の直後の「万」は、その数字に付いた単位であって独立した 10000 では
        // ない。以前はここで 10000 を加算してしまい、正しい候補を数字改変として
        // 捨てていた。
        assert!(verify_digits_preserved("5まん", "5万"));
        assert!(verify_digits_preserved("5まんえん", "5万円"));
        assert!(verify_digits_preserved("だい5まん", "第5万"));
        assert!(verify_digits_preserved("3.5まん", "3.5万"));
        assert!(verify_digits_preserved("3ぜん", "3千"));
        assert!(verify_digits_preserved("1じゅう", "1十"));
        assert!(verify_digits_preserved("5まん5せん", "5万5千"));
        assert!(verify_digits_preserved("10まんいじょう", "10万以上"));
        // 単位を伴わない通常の候補も従来どおり通る
        assert!(verify_digits_preserved("5まん", "5マン"));
    }

    #[test]
    fn digit_tampering_is_still_rejected_around_units() {
        // 単位を読み飛ばすようにしても、数字そのものの改変は捕まえる
        assert!(!verify_digits_preserved("5まん", "50万"));
        assert!(!verify_digits_preserved("5まん", "6万"));
        assert!(!verify_digits_preserved("5まんえん", "5万5円"));

        // 既知の限界: `extract_digits()` は小数点を拾わないので、`3.5` と `35` は
        // 区別できない。この検証は「数字の並びが保存されているか」だけを見る。
        // 単位の読み飛ばしとは独立した以前からの性質。
        assert!(verify_digits_preserved("3.5まん", "35万"));
    }

    #[test]
    fn kanji_number_not_adjacent_to_digit_is_still_a_number() {
        // 数字に隣接していない漢数字 run は従来どおり数値として解釈する。
        // 既存の数値表現変換（2024ねん → 二千二十四年）を壊さないこと。
        assert!(verify_digits_preserved("2024ねん", "二千二十四年"));
        assert!(verify_digits_preserved("2024ねん", "2024年"));
        assert!(!verify_digits_preserved("2024ねん", "二千二十五年"));
        // 単位だけの run でも、数字に続いていなければ数値として読む
        assert!(!verify_digits_preserved("まん", "5万"));
    }

    #[test]
    fn numeric_units_cover_rendaku_forms() {
        assert_eq!(numeric_unit_kanji("ぜん").map(|(k, _)| k), Some("千"));
        assert_eq!(numeric_unit_kanji("びゃく").map(|(k, _)| k), Some("百"));
        assert_eq!(numeric_unit_kanji("ぴゃく").map(|(k, _)| k), Some("百"));
        // 第 1 候補に置いてよいのは同音異義語の無い「万」「億」だけ
        assert_eq!(numeric_unit_kanji("まん").map(|(_, p)| p), Some(true));
        assert_eq!(numeric_unit_kanji("おく").map(|(_, p)| p), Some(true));
        assert_eq!(numeric_unit_kanji("せん").map(|(_, p)| p), Some(false));
        assert_eq!(numeric_unit_kanji("じゅう").map(|(_, p)| p), Some(false));
        // 数詞でない読みは拾わない
        assert_eq!(numeric_unit_kanji("まい"), None);
        assert_eq!(numeric_unit_kanji("まんが"), None);
    }

    #[test]
    fn unit_only_kanji_run_detection() {
        assert!(is_unit_only_kanji_run("万"));
        assert!(is_unit_only_kanji_run("千"));
        assert!(is_unit_only_kanji_run("千万"));
        // 数字を含む run は「単位だけ」ではない
        assert!(!is_unit_only_kanji_run("二千"));
        assert!(!is_unit_only_kanji_run("五"));
        assert!(!is_unit_only_kanji_run(""));
    }

    #[test]
    fn numeric_text_detection() {
        assert!(is_numeric_text("10"));
        assert!(is_numeric_text("１０"));
        assert!(is_numeric_text("十"));
        assert!(is_numeric_text("二千"));
        assert!(!is_numeric_text("重"));
        assert!(!is_numeric_text("10円"));
        assert!(!is_numeric_text(""));
    }

    /// 数字直後の数詞を実モデルで確認する診断用プローブ。
    ///
    /// `convert_with_digit_protection()` は変換器を通るため純粋なユニットテストでは
    /// 押さえられない。受入条件の読みを実際に流し、候補に何が並ぶかを目で見るための
    /// もの。モデルのロードが入るので既定では走らせない。
    ///
    /// 実行: `cargo test -p rakukan-engine --lib probe_numeric_unit_candidates -- --ignored --nocapture`
    #[test]
    #[ignore = "実モデルのロードが必要"]
    fn probe_numeric_unit_candidates() {
        use crate::kanji::Backend;

        let backend =
            Backend::from_variant_id("jinen-v1-small-q5").expect("Failed to load default model");
        let converter = KanaKanjiConverter::new(backend).expect("Failed to create converter");
        let order = default_digit_candidates_order();

        for reading in [
            "5まん",
            "5まんえん",
            "だい5まん",
            "3.5まん",
            "3ぜん",
            "1じゅう",
            "10まん",
            "3せん",
        ] {
            let cands =
                convert_with_digit_protection(&converter, reading, "", 9, &order, false, false)
                    .unwrap_or_else(|e| panic!("{reading}: {e:?}"));
            println!("{reading:>12} -> {cands:?}");
        }
    }

    #[test]
    fn split_by_digits_splits_digit_and_kana() {
        let runs = split_by_digits("5まん");
        assert_eq!(runs.len(), 2);
        assert!(runs[0].is_digit());
        assert_eq!(runs[0].text(), "5");
        assert_eq!(runs[1].text(), "まん");
    }
    #[test]
    fn numeric_unit_promotion_depends_on_ambiguity() {
        // 数字の直後でほぼ一意 → 第 1 候補に置いてよい
        assert_eq!(numeric_unit_kanji("まん"), Some(("万", true)));
        assert_eq!(numeric_unit_kanji("おく"), Some(("億", true)));

        // 同音異義語あり → 候補には入れるが順位は LLM に譲る
        // 3せん=3戦/3選, 1ちょう=1丁, 5じゅう=5重
        assert_eq!(numeric_unit_kanji("せん"), Some(("千", false)));
        assert_eq!(numeric_unit_kanji("ちょう"), Some(("兆", false)));
        assert_eq!(numeric_unit_kanji("じゅう"), Some(("十", false)));
        assert_eq!(numeric_unit_kanji("ひゃく"), Some(("百", false)));

        // 前方一致では拾わない
        assert_eq!(numeric_unit_kanji("まんが"), None);
        assert_eq!(numeric_unit_kanji("せんち"), None);
        assert_eq!(numeric_unit_kanji("かわ"), None);
    }

    #[test]
    fn numeric_unit_rule_does_not_apply_when_context_follows() {
        // 「5せんのしはらい」「5せんをかちぬいた」は かな run が数詞に
        // 完全一致しないので、このルールは発動せず文脈ごと LLM に渡る。
        for reading in ["5せんのしはらい", "5せんをかちぬいた", "5まんえん"] {
            let runs = split_by_digits(reading);
            assert_eq!(runs.len(), 2, "{reading}");
            let Run::Kana(k) = &runs[1] else {
                panic!("{reading}: 2 番目が Kana ではない")
            };
            assert_eq!(
                numeric_unit_kanji(k),
                None,
                "{reading}: かな run \"{k}\" で発動してはいけない"
            );
        }
    }
}
