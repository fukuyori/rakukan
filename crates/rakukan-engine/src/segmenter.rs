use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};
use vibrato::{Dictionary, Tokenizer};

use rakukan_dict::dict_dir;

const VIBRATO_DICT_ENV: &str = "RAKUKAN_VIBRATO_DIC";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SegmentBlock {
    pub surface: String,
    pub reading: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SegmentCandidate {
    pub surface: String,
    pub segments: Vec<SegmentBlock>,
}

#[derive(Debug, Clone)]
struct TokenHint {
    surface: String,
    preferred_reading: Option<String>,
}

static TOKENIZER: LazyLock<Result<Tokenizer, String>> = LazyLock::new(|| match load_tokenizer() {
    Ok(tokenizer) => {
        info!("[vibrato] tokenizer ready");
        Ok(tokenizer)
    }
    Err(err) => {
        warn!("[vibrato] fallback to heuristic: {err:#}");
        Err(err.to_string())
    }
});

pub fn segment_surface(surface: &str) -> Vec<String> {
    segment_token_hints(surface)
        .into_iter()
        .map(|token| token.surface)
        .collect()
}

pub fn segment_candidate(surface: &str, reading: &str) -> Vec<SegmentBlock> {
    if surface.is_empty() {
        return Vec::new();
    }

    let tokens = segment_token_hints(surface);
    if tokens.is_empty() {
        return vec![SegmentBlock {
            surface: surface.to_string(),
            reading: reading.to_string(),
        }];
    }

    let blocks = align_segment_blocks(reading, &tokens).unwrap_or_else(|| {
        let runs = tokens
            .into_iter()
            .map(|token| token.surface)
            .collect::<Vec<_>>();
        let reading_parts = split_reading_evenly(reading, runs.len());
        runs.into_iter()
            .zip(reading_parts)
            .map(|(surface, reading)| SegmentBlock { surface, reading })
            .collect()
    });
    debug!(
        "[vibrato] segment_candidate reading={reading:?} surface={surface:?} blocks={}",
        debug_blocks(&blocks)
    );
    blocks
}

pub fn segment_candidates(reading: &str, candidates: &[String]) -> Vec<SegmentCandidate> {
    candidates
        .iter()
        .map(|surface| SegmentCandidate {
            surface: surface.clone(),
            segments: segment_candidate(surface, reading),
        })
        .collect()
}

fn segment_token_hints(surface: &str) -> Vec<TokenHint> {
    if surface.is_empty() {
        return Vec::new();
    }

    match &*TOKENIZER {
        Ok(tokenizer) => {
            let mut worker = tokenizer.new_worker();
            let normalized = hiragana_to_katakana(surface);
            let boundaries = char_boundaries(surface);
            worker.reset_sentence(&normalized);
            worker.tokenize();

            let mut out = Vec::with_capacity(worker.num_tokens());
            for i in 0..worker.num_tokens() {
                let token = worker.token(i);
                let range = token.range_char();
                let start = *boundaries.get(range.start).unwrap_or(&0);
                let end = *boundaries.get(range.end).unwrap_or(&surface.len());
                let piece = &surface[start..end];
                if !piece.is_empty() {
                    out.push(TokenHint {
                        surface: piece.to_string(),
                        preferred_reading: parse_feature_reading(token.feature()),
                    });
                }
            }

            if out.is_empty() {
                debug!("[vibrato] empty token stream for surface={surface:?}");
                vec![TokenHint {
                    surface: surface.to_string(),
                    preferred_reading: None,
                }]
            } else {
                debug!(
                    "[vibrato] token_hints surface={surface:?} tokens={}",
                    debug_token_hints(&out)
                );
                out
            }
        }
        Err(_) => vec![TokenHint {
            surface: surface.to_string(),
            preferred_reading: None,
        }],
    }
}

fn hiragana_to_katakana(text: &str) -> String {
    text.chars()
        .map(|c| {
            if ('\u{3041}'..='\u{3096}').contains(&c) {
                char::from_u32(c as u32 + 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

pub fn vibrato_dict_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(VIBRATO_DICT_ENV) {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    let primary = dict_dir()?.join("vibrato").join("system.dic");
    if primary.exists() {
        return Some(primary);
    }

    None
}

fn load_tokenizer() -> Result<Tokenizer> {
    let path = vibrato_dict_path()
        .context("system.dic not found in %LOCALAPPDATA%\\rakukan\\dict\\vibrato")?;
    let file = File::open(&path)
        .with_context(|| format!("failed to open Vibrato dictionary: {}", path.display()))?;
    let reader = BufReader::new(file);
    let dict = Dictionary::read(reader)
        .with_context(|| format!("failed to read Vibrato dictionary: {}", path.display()))?;
    info!("[vibrato] dictionary loaded path={}", path.display());
    Ok(Tokenizer::new(dict))
}

fn char_boundaries(s: &str) -> Vec<usize> {
    let mut out = s.char_indices().map(|(i, _)| i).collect::<Vec<_>>();
    out.push(s.len());
    out
}

fn is_kana_surface(text: &str) -> bool {
    !text.is_empty()
        && text.chars().all(|c| {
            matches!(
                c as u32,
                0x3041..=0x3096 | 0x30A1..=0x30FA | 0x30FC | 0xFF66..=0xFF9F
            )
        })
}

fn parse_feature_reading(feature: &str) -> Option<String> {
    let reading = feature.split(',').nth(7)?;
    if reading.is_empty() || reading == "*" {
        None
    } else {
        Some(katakana_to_hiragana(reading))
    }
}

fn katakana_to_hiragana(text: &str) -> String {
    text.chars()
        .map(|c| {
            if ('ァ'..='ヶ').contains(&c) {
                char::from_u32(c as u32 - 0x60).unwrap_or(c)
            } else {
                c
            }
        })
        .collect()
}

fn preferred_reading(token: &TokenHint) -> Option<&str> {
    token.preferred_reading.as_deref().or_else(|| {
        if is_kana_surface(&token.surface) {
            Some(token.surface.as_str())
        } else {
            None
        }
    })
}

fn score_segment(token: &TokenHint, reading: &str, is_last: bool) -> Option<i32> {
    if reading.is_empty() {
        return None;
    }

    let mut score = -(reading.chars().count() as i32);
    if let Some(preferred) = preferred_reading(token) {
        if reading == preferred {
            score += 10_000;
        } else if !is_last {
            return None;
        } else {
            score -= 2_000;
        }
    }

    if token.surface == reading {
        score += 300;
    }

    Some(score)
}

fn align_segment_blocks(reading: &str, tokens: &[TokenHint]) -> Option<Vec<SegmentBlock>> {
    let boundaries = char_boundaries(reading);
    let mut best: Vec<Vec<Option<(i32, Vec<SegmentBlock>)>>> =
        vec![vec![None; boundaries.len()]; tokens.len()];

    fn solve(
        token_idx: usize,
        boundary_idx: usize,
        reading: &str,
        boundaries: &[usize],
        tokens: &[TokenHint],
        best: &mut [Vec<Option<(i32, Vec<SegmentBlock>)>>],
    ) -> Option<(i32, Vec<SegmentBlock>)> {
        if let Some(cached) = &best[token_idx][boundary_idx] {
            return Some(cached.clone());
        }

        let start = boundaries[boundary_idx];
        let token = &tokens[token_idx];
        let is_last = token_idx + 1 == tokens.len();
        let remaining_tokens = tokens.len() - token_idx - 1;
        let max_end_idx = boundaries.len().saturating_sub(remaining_tokens + 1);

        let mut best_candidate: Option<(i32, Vec<SegmentBlock>)> = None;
        for end_idx in (boundary_idx + 1)..=max_end_idx {
            let end = boundaries[end_idx];
            let current = &reading[start..end];
            let Some(mut score) = score_segment(token, current, is_last) else {
                continue;
            };

            let mut blocks = vec![SegmentBlock {
                surface: token.surface.clone(),
                reading: current.to_string(),
            }];

            if !is_last {
                let Some((rest_score, mut rest_blocks)) =
                    solve(token_idx + 1, end_idx, reading, boundaries, tokens, best)
                else {
                    continue;
                };
                score += rest_score;
                blocks.append(&mut rest_blocks);
            } else if end != reading.len() {
                continue;
            }

            let replace = best_candidate
                .as_ref()
                .map(|(best_score, _)| score > *best_score)
                .unwrap_or(true);
            if replace {
                best_candidate = Some((score, blocks));
            }
        }

        best[token_idx][boundary_idx] = best_candidate.clone();
        best_candidate
    }

    solve(0, 0, reading, &boundaries, tokens, &mut best).map(|(_, blocks)| blocks)
}

fn split_reading_evenly(reading: &str, count: usize) -> Vec<String> {
    if count == 0 {
        return Vec::new();
    }
    let chars: Vec<char> = reading.chars().collect();
    if chars.is_empty() {
        return vec![String::new(); count];
    }
    let mut parts = Vec::with_capacity(count);
    let mut start = 0usize;
    for idx in 0..count {
        let remaining_chars = chars.len().saturating_sub(start);
        let remaining_parts = count - idx;
        let take = if remaining_parts <= 1 {
            remaining_chars
        } else {
            (remaining_chars / remaining_parts).max(1)
        };
        let end = (start + take).min(chars.len());
        parts.push(chars[start..end].iter().collect());
        start = end;
    }
    parts
}

fn debug_token_hints(tokens: &[TokenHint]) -> String {
    tokens
        .iter()
        .map(|token| match &token.preferred_reading {
            Some(reading) => format!("{}<{}>", token.surface, reading),
            None => format!("{}<*>", token.surface),
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn debug_blocks(blocks: &[SegmentBlock]) -> String {
    blocks
        .iter()
        .map(|block| format!("{}<{}>", block.surface, block.reading))
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::{
        SegmentBlock, TokenHint, align_segment_blocks, katakana_to_hiragana, parse_feature_reading,
    };

    #[test]
    fn parse_feature_reading_extracts_ipadic_reading() {
        let feature = "東京都,名詞,固有名詞,地名,一般,*,*,トウキョウト,東京都,*,B,5/9,*,5/9,*";
        assert_eq!(
            parse_feature_reading(feature).as_deref(),
            Some("とうきょうと")
        );
    }

    #[test]
    fn katakana_to_hiragana_converts_basic_kana() {
        assert_eq!(katakana_to_hiragana("ワサビ"), "わさび");
    }

    #[test]
    fn align_segment_blocks_prefers_token_readings() {
        let tokens = vec![
            TokenHint {
                surface: "刺身".into(),
                preferred_reading: Some("さしみ".into()),
            },
            TokenHint {
                surface: "と".into(),
                preferred_reading: Some("と".into()),
            },
            TokenHint {
                surface: "山葵".into(),
                preferred_reading: Some("わさび".into()),
            },
            TokenHint {
                surface: "と".into(),
                preferred_reading: Some("と".into()),
            },
            TokenHint {
                surface: "辛子".into(),
                preferred_reading: Some("からし".into()),
            },
        ];

        let blocks = align_segment_blocks("さしみとわさびとからし", &tokens).unwrap();
        assert_eq!(
            blocks
                .into_iter()
                .map(|b| (b.surface, b.reading))
                .collect::<Vec<_>>(),
            vec![
                ("刺身".into(), "さしみ".into()),
                ("と".into(), "と".into()),
                ("山葵".into(), "わさび".into()),
                ("と".into(), "と".into()),
                ("辛子".into(), "からし".into()),
            ]
        );
    }

    #[test]
    fn align_segment_blocks_uses_last_segment_as_fallback() {
        let tokens = vec![
            TokenHint {
                surface: "吾輩".into(),
                preferred_reading: Some("わがはい".into()),
            },
            TokenHint {
                surface: "は".into(),
                preferred_reading: Some("は".into()),
            },
            TokenHint {
                surface: "猫である".into(),
                preferred_reading: None,
            },
        ];

        let blocks = align_segment_blocks("わがはいはねこである", &tokens).unwrap();
        assert_eq!(
            blocks,
            vec![
                SegmentBlock {
                    surface: "吾輩".into(),
                    reading: "わがはい".into(),
                },
                SegmentBlock {
                    surface: "は".into(),
                    reading: "は".into(),
                },
                SegmentBlock {
                    surface: "猫である".into(),
                    reading: "ねこである".into(),
                },
            ]
        );
    }
}
