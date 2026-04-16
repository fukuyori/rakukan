//! リテラル保護レイヤー
//!
//! reading を「数字ラン」「アルファベットラン」「かなラン」に分割し、
//! LLM にはかな部分だけを渡す。数字・アルファベットは原文を保持し、
//! 半角・全角の両方を候補として提示する。

use crate::kanji::KanaKanjiConverter;
#[cfg(test)]
use crate::segments::{Candidate, CandidateSource};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    Digit(String),
    Alpha(String),
    Kana(String),
}

impl Run {
    pub fn text(&self) -> &str {
        match self {
            Run::Digit(s) | Run::Alpha(s) | Run::Kana(s) => s,
        }
    }

    pub fn is_literal(&self) -> bool {
        matches!(self, Run::Digit(_) | Run::Alpha(_))
    }

    pub fn is_digit(&self) -> bool {
        matches!(self, Run::Digit(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharKind {
    Digit,
    Alpha,
    Kana,
}

fn classify_char(c: char) -> CharKind {
    if c.is_ascii_digit() || ('０'..='９').contains(&c) {
        CharKind::Digit
    } else if c.is_ascii_alphabetic()
        || ('Ａ'..='Ｚ').contains(&c)
        || ('ａ'..='ｚ').contains(&c)
    {
        CharKind::Alpha
    } else {
        CharKind::Kana
    }
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

fn digit_candidates(s: &str) -> Vec<String> {
    let half = to_halfwidth_digits(s);
    let full = to_fullwidth_digits(s);
    if half == full {
        vec![half]
    } else {
        vec![half, full]
    }
}

#[cfg(test)]
fn digit_candidate_structs(s: &str) -> Vec<Candidate> {
    let half = to_halfwidth_digits(s);
    let full = to_fullwidth_digits(s);
    if half == full {
        vec![Candidate {
            surface: half,
            source: CandidateSource::Digit,
            annotation: None,
        }]
    } else {
        vec![
            Candidate {
                surface: half,
                source: CandidateSource::Digit,
                annotation: Some("半角".into()),
            },
            Candidate {
                surface: full,
                source: CandidateSource::Digit,
                annotation: Some("全角".into()),
            },
        ]
    }
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

fn literal_candidates(run: &Run) -> Vec<String> {
    match run {
        Run::Digit(s) => digit_candidates(s),
        Run::Alpha(s) => alpha_candidates(s),
        Run::Kana(_) => unreachable!(),
    }
}

#[cfg(test)]
fn literal_candidate_structs(run: &Run) -> Vec<Candidate> {
    match run {
        Run::Digit(s) => digit_candidate_structs(s),
        Run::Alpha(s) => alpha_candidate_structs(s),
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
        CharKind::Kana => Run::Kana(text),
    }
}

fn extract_digits(s: &str) -> String {
    s.chars()
        .filter_map(|c| {
            if c.is_ascii_digit() {
                Some(c)
            } else if ('０'..='９').contains(&c) {
                Some(char::from_u32(c as u32 - '０' as u32 + '0' as u32).unwrap_or(c))
            } else {
                None
            }
        })
        .collect()
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
) -> crate::kanji::error::Result<Vec<String>> {
    let runs = split_by_digits(reading);

    if runs.iter().all(|r| !r.is_literal()) {
        return converter.convert(reading, context, num_candidates);
    }

    if runs.iter().all(|r| r.is_literal()) {
        let literal_str: String = runs.iter().map(|r| r.text()).collect();
        if runs.iter().all(|r| r.is_digit()) {
            return Ok(digit_candidates(&literal_str));
        }
        if runs.iter().all(|r| matches!(r, Run::Alpha(_))) {
            return Ok(alpha_candidates(&literal_str));
        }
        // 数字+アルファベット混在のリテラルのみ
        let half: String = runs
            .iter()
            .map(|r| match r {
                Run::Digit(s) => to_halfwidth_digits(s),
                Run::Alpha(s) => to_halfwidth_alpha(s),
                Run::Kana(s) => s.clone(),
            })
            .collect();
        let full: String = runs
            .iter()
            .map(|r| match r {
                Run::Digit(s) => to_fullwidth_digits(s),
                Run::Alpha(s) => to_fullwidth_alpha(s),
                Run::Kana(s) => s.clone(),
            })
            .collect();
        return if half == full {
            Ok(vec![half])
        } else {
            Ok(vec![half, full])
        };
    }

    let mut run_candidates: Vec<Vec<String>> = Vec::with_capacity(runs.len());
    for (i, run) in runs.iter().enumerate() {
        if run.is_literal() {
            run_candidates.push(literal_candidates(run));
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
    fn verify_preserved_ok() {
        assert!(verify_digits_preserved("２０２４ねん", "２０２４年"));
        assert!(verify_digits_preserved("２０２４ねん", "2024年"));
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
            vec!["2024".into(), "２０２４".into()],
            vec!["年".into(), "ねん".into()],
        ];
        let result = combine_runs(&runs, 5);
        assert_eq!(
            result,
            vec!["2024年", "2024ねん", "２０２４年", "２０２４ねん"]
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
        let cands = digit_candidates("2024");
        assert_eq!(cands, vec!["2024", "２０２４"]);
    }

    #[test]
    fn digit_candidates_fullwidth_input() {
        let cands = digit_candidates("２０２４");
        assert_eq!(cands, vec!["2024", "２０２４"]);
    }

    #[test]
    fn digit_candidate_structs_has_annotations() {
        let cands = digit_candidate_structs("100");
        assert_eq!(cands.len(), 2);
        assert_eq!(cands[0].surface, "100");
        assert_eq!(cands[0].annotation.as_deref(), Some("半角"));
        assert_eq!(cands[1].surface, "１００");
        assert_eq!(cands[1].annotation.as_deref(), Some("全角"));
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
    fn combine_respects_limit() {
        let runs = vec![
            vec!["A".into(), "B".into(), "C".into()],
            vec!["1".into(), "2".into(), "3".into()],
        ];
        let result = combine_runs(&runs, 3);
        assert_eq!(result.len(), 3);
    }
}
