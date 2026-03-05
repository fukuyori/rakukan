//! SKK辞書パーサー
//!
//! # SKK辞書フォーマット（送りなし）
//! ```text
//! ;; comment lines start with semicolons
//! ;; okuri-ari entries.
//! あk /開/空/明/飽/.../ ← 送りあり（無視）
//! ;; okuri-nasi entries.
//! にほんご /日本語/
//! とうきょう /東京/都京/
//! ```
//!
//! # パース方針
//! - 送りあり（`*k` のようにローマ字が続く）エントリは無視
//! - アノテーション（`;` 以降）は除去
//! - 候補は `/` で区切られた文字列
//! - 空候補は除去

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};

use anyhow::Result;
use tracing::{debug, warn};

/// ひらがな読み → 候補リストのマップ
pub type SkkDict = HashMap<String, Vec<String>>;

/// SKK辞書をパースして HashMap に展開する
pub fn parse<R: Read>(reader: R) -> Result<SkkDict> {
    let mut dict: SkkDict = HashMap::new();
    let reader = BufReader::new(reader);

    let mut in_okuri_nasi = false;
    let mut skipped_okuri_ari = 0usize;
    let mut parsed = 0usize;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                warn!("SKK parse: line read error: {e}");
                continue;
            }
        };
        let line = line.trim();

        // コメント行
        if line.starts_with(';') {
            if line.contains("okuri-nasi") {
                in_okuri_nasi = true;
                debug!("SKK: okuri-nasi section started");
            } else if line.contains("okuri-ari") {
                in_okuri_nasi = false;
            }
            continue;
        }

        if line.is_empty() {
            continue;
        }

        // エントリ行: "よみ /候補1/候補2/..."
        let Some(space_pos) = line.find(' ') else { continue };
        let reading = &line[..space_pos];
        let rest = &line[space_pos + 1..];

        // 送りありエントリはスキップ（読みの最後がASCII小文字）
        if !in_okuri_nasi || reading.ends_with(|c: char| c.is_ascii_lowercase()) {
            skipped_okuri_ari += 1;
            continue;
        }

        let candidates = parse_candidates(rest);
        if candidates.is_empty() {
            continue;
        }

        let reading_hira = to_hiragana(reading);
        dict.entry(reading_hira).or_default().extend(candidates);
        parsed += 1;
    }

    debug!(
        "SKK: parsed {} entries, skipped {} okuri-ari",
        parsed, skipped_okuri_ari
    );
    Ok(dict)
}

/// `/候補1/候補2/;comment/` 形式から候補リストを取り出す
fn parse_candidates(s: &str) -> Vec<String> {
    s.split('/')
        .filter(|t| !t.is_empty())
        .map(|t| {
            // アノテーション除去: "候補;注釈" → "候補"
            if let Some(pos) = t.find(';') { &t[..pos] } else { t }
        })
        .filter(|t| !t.is_empty())
        // ASCII制御文字のみのゴミエントリを除く
        // ※ 全角記号（括弧・句読点等）は通す
        .filter(|t| !t.chars().all(|c| c.is_ascii_control()))
        .map(|t| t.to_string())
        .collect()
}

/// カタカナ → ひらがな変換（読みの正規化）
fn to_hiragana(s: &str) -> String {
    s.chars().map(|c| {
        let code = c as u32;
        if (0x30A1..=0x30F6).contains(&code) {
            char::from_u32(code - 0x60).unwrap_or(c)
        } else {
            c
        }
    }).collect()
}

/// EUC-JP バイト列を UTF-8 文字列に変換する（encoding_rs 使用）
///
/// SKK-JISYO.L は EUC-JP エンコードで配布されているため、
/// ロード時にこの関数で UTF-8 に変換してからパースする。
pub fn decode_eucjp(bytes: &[u8]) -> String {
    let (text, _, had_errors) = encoding_rs::EUC_JP.decode(bytes);
    if had_errors {
        warn!("SKK decode_eucjp: encoding errors detected (non-EUC-JP bytes replaced with U+FFFD)");
    }
    text.into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_basic() {
        let input = b"\
;; okuri-ari entries.\n\
\xe3\x81\x82k /\xe9\x96\x8b/\n\
;; okuri-nasi entries.\n\
\xe3\x81\xab\xe3\x81\xbb\xe3\x82\x93\xe3\x81\x94 /\xe6\x97\xa5\xe6\x9c\xac\xe8\xaa\x9e/\n\
\xe3\x81\xa8\xe3\x81\x86\xe3\x81\x8d\xe3\x82\x87\xe3\x81\x86 /\xe6\x9d\xb1\xe4\xba\xac/\n\
";
        let dict = parse(input.as_slice()).unwrap();
        assert!(dict.contains_key("にほんご"), "にほんご not found: {:?}", dict.keys().collect::<Vec<_>>());
        assert_eq!(dict["にほんご"], vec!["日本語"]);
        assert_eq!(dict["とうきょう"], vec!["東京"]);
        assert!(!dict.contains_key("あ"), "okuri-ari should be skipped");
    }

    #[test]
    fn test_annotation_stripped() {
        let input = b"\
;; okuri-nasi entries.\n\
\xe3\x81\xa8\xe3\x81\x86\xe3\x81\x8d\xe3\x82\x87\xe3\x81\x86 /\xe6\x9d\xb1\xe4\xba\xac;\xe9\x83\xbd\xe9\x81\x93\xe5\xba\x9c\xe7\x9c\x8c/\n\
";
        let dict = parse(input.as_slice()).unwrap();
        assert_eq!(dict["とうきょう"], vec!["東京"]);
    }

    #[test]
    fn test_kakko_symbol() {
        // かっこ → （ etc.（全角記号は通過すること）
        let input = b"\
;; okuri-nasi entries.\n\
\xe3\x81\x8b\xe3\x81\xa3\xe3\x81\x93 /\xef\xbc\x88/\xef\xbc\x89/\n\
";
        let dict = parse(input.as_slice()).unwrap();
        // 「（」「）」が候補として含まれること
        let cands = dict.get("かっこ").expect("かっこ not found");
        assert!(cands.contains(&"（".to_string()), "（ not in candidates: {:?}", cands);
    }

    #[test]
    fn test_eucjp_decode() {
        // EUC-JP で "漢字" = 0xB4C1 0xBBFA
        let eucjp: &[u8] = &[0xB4, 0xC1, 0xBB, 0xFA];
        let result = decode_eucjp(eucjp);
        assert_eq!(result, "漢字", "EUC-JP decode failed: {:?}", result);
    }

    #[test]
    fn test_to_hiragana() {
        assert_eq!(to_hiragana("アイウ"), "あいう");
        assert_eq!(to_hiragana("にほんご"), "にほんご");
    }

    #[test]
    fn test_parse_candidates_keeps_symbols() {
        // 全角括弧は残す
        let cands = parse_candidates("/（/）/");
        assert_eq!(cands, vec!["（", "）"]);
    }
}

