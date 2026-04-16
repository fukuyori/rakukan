//! 数値保護レイヤー
//!
//! reading を「数字ラン」と「非数字ラン」に分割し、LLM には非数字部分だけを渡す。
//! LLM 出力後に元の数字ランを再挿入することで、LLM が数字を改変する問題を防ぐ。

use crate::kanji::KanaKanjiConverter;
use crate::segmenter;
use crate::segments::{Candidate, CandidateSource, Segment};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Run {
    Digit(String),
    Kana(String),
}

impl Run {
    pub fn text(&self) -> &str {
        match self {
            Run::Digit(s) | Run::Kana(s) => s,
        }
    }

    pub fn is_digit(&self) -> bool {
        matches!(self, Run::Digit(_))
    }
}

fn is_digit_char(c: char) -> bool {
    c.is_ascii_digit() || ('０'..='９').contains(&c)
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

pub fn split_by_digits(reading: &str) -> Vec<Run> {
    let mut runs = Vec::new();
    let mut current = String::new();
    let mut in_digit = false;

    for c in reading.chars() {
        let c_is_digit = is_digit_char(c);
        if current.is_empty() {
            in_digit = c_is_digit;
            current.push(c);
        } else if c_is_digit == in_digit {
            current.push(c);
        } else {
            let run = if in_digit {
                Run::Digit(std::mem::take(&mut current))
            } else {
                Run::Kana(std::mem::take(&mut current))
            };
            runs.push(run);
            in_digit = c_is_digit;
            current.push(c);
        }
    }
    if !current.is_empty() {
        runs.push(if in_digit {
            Run::Digit(current)
        } else {
            Run::Kana(current)
        });
    }
    runs
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
        if let Some(Run::Digit(d)) = runs.get(kana_index - 1) {
            if !ctx.is_empty() {
                ctx.push_str("…");
            }
            ctx.push_str(d);
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

    if runs.iter().all(|r| !r.is_digit()) {
        return converter.convert(reading, context, num_candidates);
    }

    if runs.iter().all(|r| r.is_digit()) {
        let digit_str: String = runs.iter().map(|r| r.text()).collect();
        return Ok(digit_candidates(&digit_str));
    }

    let mut run_candidates: Vec<Vec<String>> = Vec::with_capacity(runs.len());
    for (i, run) in runs.iter().enumerate() {
        match run {
            Run::Digit(s) => {
                run_candidates.push(digit_candidates(s));
            }
            Run::Kana(s) => {
                let local_context = build_local_context(&runs, i, context);
                let cands = converter.convert(s, &local_context, num_candidates)?;
                run_candidates.push(cands);
            }
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

pub fn segment_with_digit_protection(reading: &str, surface: &str) -> Vec<Segment> {
    let reading_runs = split_by_digits(reading);

    if reading_runs.iter().all(|r| !r.is_digit()) {
        let blocks = segmenter::segment_candidate(surface, reading);
        return blocks
            .into_iter()
            .map(|b| Segment {
                reading: b.reading,
                candidates: vec![Candidate {
                    surface: b.surface,
                    source: CandidateSource::Llm,
                    annotation: None,
                }],
                selected: 0,
                fixed: false,
            })
            .collect();
    }

    let surface_runs = split_by_digits(surface);

    if reading_runs.len() != surface_runs.len() {
        let blocks = segmenter::segment_candidate(surface, reading);
        return blocks
            .into_iter()
            .map(|b| {
                let is_digit = split_by_digits(&b.reading)
                    .iter()
                    .all(|r| r.is_digit());
                Segment {
                    reading: b.reading,
                    candidates: vec![Candidate {
                        surface: b.surface,
                        source: if is_digit {
                            CandidateSource::Digit
                        } else {
                            CandidateSource::Llm
                        },
                        annotation: None,
                    }],
                    selected: 0,
                    fixed: is_digit,
                }
            })
            .collect();
    }

    let mut segments = Vec::new();
    for (r_run, s_run) in reading_runs.iter().zip(surface_runs.iter()) {
        match r_run {
            Run::Digit(d) => {
                segments.push(Segment {
                    reading: d.clone(),
                    candidates: digit_candidate_structs(d),
                    selected: 0,
                    fixed: true,
                });
            }
            Run::Kana(k) => {
                let sub_blocks =
                    segmenter::segment_candidate(s_run.text(), k);
                for b in sub_blocks {
                    segments.push(Segment {
                        reading: b.reading,
                        candidates: vec![Candidate {
                            surface: b.surface,
                            source: CandidateSource::Llm,
                            annotation: None,
                        }],
                        selected: 0,
                        fixed: false,
                    });
                }
            }
        }
    }
    segments
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
    fn combine_respects_limit() {
        let runs = vec![
            vec!["A".into(), "B".into(), "C".into()],
            vec!["1".into(), "2".into(), "3".into()],
        ];
        let result = combine_runs(&runs, 3);
        assert_eq!(result.len(), 3);
    }
}
