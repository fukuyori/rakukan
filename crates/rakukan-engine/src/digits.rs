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
        } else if let Some(unit) = large_kanji_unit(c) {
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
        } else {
            return None;
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
        if let Some(surface) = surface {
            if !candidates.iter().any(|c: &Candidate| c.surface == surface) {
                candidates.push(Candidate {
                    surface,
                    source: CandidateSource::Digit,
                    annotation: Some(annotation.into()),
                });
            }
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

fn alpha_candidates(s: &str) -> Vec<String> {
    let half = to_halfwidth_alpha(s);
    let full = to_fullwidth_alpha(s);
    if half == full {
        vec![half]
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

fn symbol_candidates(s: &str) -> Vec<String> {
    let half = to_halfwidth_symbol(s);
    let full = to_fullwidth_symbol(s);
    if half == full {
        vec![half]
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

fn literal_candidates(run: &Run, digit_candidates_order: &[DigitCandidateKind]) -> Vec<String> {
    match run {
        Run::Digit(s) => digit_candidates(s, digit_candidates_order),
        Run::Alpha(s) => alpha_candidates(s),
        Run::Symbol(s) => symbol_candidates(s),
        Run::Kana(_) => unreachable!(),
    }
}

fn half_full_literal_candidates(run: &Run) -> Vec<String> {
    let (half, full) = match run {
        Run::Digit(s) => (to_halfwidth_digits(s), to_fullwidth_digits(s)),
        Run::Alpha(s) => (to_halfwidth_alpha(s), to_fullwidth_alpha(s)),
        Run::Symbol(s) => (to_halfwidth_symbol(s), to_fullwidth_symbol(s)),
        Run::Kana(_) => unreachable!(),
    };
    if half == full {
        vec![half]
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

fn extract_digits(s: &str) -> String {
    let mut out = String::new();
    let mut kanji_run = String::new();

    let flush_kanji_run = |out: &mut String, kanji_run: &mut String| {
        if kanji_run.is_empty() {
            return;
        }
        if let Some(digits) = parse_kanji_number_digits(kanji_run) {
            out.push_str(&digits);
        }
        kanji_run.clear();
    };

    for c in s.chars() {
        if c.is_ascii_digit() {
            flush_kanji_run(&mut out, &mut kanji_run);
            out.push(c);
        } else if ('０'..='９').contains(&c) {
            flush_kanji_run(&mut out, &mut kanji_run);
            out.push(char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c));
        } else if is_kanji_number_char(c) {
            kanji_run.push(c);
        } else {
            flush_kanji_run(&mut out, &mut kanji_run);
        }
    }
    flush_kanji_run(&mut out, &mut kanji_run);
    out
}

pub fn verify_digits_preserved(input: &str, output: &str) -> bool {
    extract_digits(input) == extract_digits(output)
}

fn build_local_context(runs: &[Run], kana_index: usize, global_context: &str) -> String {
    let mut ctx = String::from(global_context);
    if kana_index > 0 {
        if let Some(run) = runs.get(kana_index - 1) {
            if run.is_literal() {
                if !ctx.is_empty() {
                    ctx.push_str("…");
                }
                ctx.push_str(run.text());
            }
        }
    }
    ctx
}

pub fn convert_with_digit_protection(
    converter: &KanaKanjiConverter,
    reading: &str,
    context: &str,
    num_candidates: usize,
    digit_candidates_order: &[DigitCandidateKind],
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
            return Ok(alpha_candidates(&literal_str));
        }
        if runs.iter().all(|r| matches!(r, Run::Symbol(_))) {
            return Ok(symbol_candidates(&literal_str));
        }
        // 数字+アルファベット+記号混在のリテラルのみ。
        // 数字の漢数字化は「数字だけ」の時に限定し、混在時は半角/全角候補を合成する。
        let run_candidates: Vec<Vec<String>> =
            runs.iter().map(half_full_literal_candidates).collect();
        return Ok(combine_runs(&run_candidates, num_candidates));
    }

    let mut run_candidates: Vec<Vec<String>> = Vec::with_capacity(runs.len());
    for (i, run) in runs.iter().enumerate() {
        if run.is_literal() {
            run_candidates.push(literal_candidates(run, digit_candidates_order));
        } else if let Run::Kana(s) = run {
            let local_context = build_local_context(&runs, i, context);
            let cands = converter.convert(s, &local_context, num_candidates)?;
            run_candidates.push(cands);
        }
    }

    let combined = combine_runs(&run_candidates, num_candidates);

    let verified: Vec<String> = combined
        .into_iter()
        .filter(|c| verify_digits_preserved(reading, c))
        .collect();

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
    fn alpha_candidates_halfwidth() {
        let cands = alpha_candidates("PC");
        assert_eq!(cands, vec!["PC", "ＰＣ"]);
    }

    #[test]
    fn alpha_candidates_fullwidth() {
        let cands = alpha_candidates("ＰＣ");
        assert_eq!(cands, vec!["PC", "ＰＣ"]);
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
    fn alpha_lowercase() {
        let cands = alpha_candidates("abc");
        assert_eq!(cands, vec!["abc", "ａｂｃ"]);
    }

    #[test]
    fn symbol_candidates_halfwidth() {
        let cands = symbol_candidates("+-*/");
        assert_eq!(cands, vec!["+-*/", "＋－＊／"]);
    }

    #[test]
    fn symbol_candidates_fullwidth() {
        let cands = symbol_candidates("（！）");
        assert_eq!(cands, vec!["(!)", "（！）"]);
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
            alpha_candidates("USB"),
            symbol_candidates("-"),
            alpha_candidates("C"),
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
        let run_candidates: Vec<Vec<String>> =
            runs.iter().map(half_full_literal_candidates).collect();
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
}
