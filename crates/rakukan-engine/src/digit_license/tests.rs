use super::*;
use crate::digits::{
    combine_and_verify, combine_runs_with_origin, digits_of_tokens, scan_numeric_tokens,
    split_by_digits, verify_digits_preserved,
};
use std::cell::Cell;

/// テスト用の辞書（固定の表）。引いた回数を数える。
struct FixedSource {
    map: HashMap<&'static str, Vec<&'static str>>,
    calls: Cell<usize>,
}

impl SurfaceSource for FixedSource {
    fn surfaces(&self, reading: &str) -> Vec<String> {
        self.calls.set(self.calls.get() + 1);
        self.map
            .get(reading)
            .map(|v| v.iter().map(|s| s.to_string()).collect())
            .unwrap_or_default()
    }
}

/// 計画書の模擬検証で根拠になった語（`rakukan.dict` の通常語）に、1 文字の表記と
/// 紛らわしい同音語を足した表。
fn fixed_source() -> FixedSource {
    let entries: [(&'static str, &[&'static str]); 16] = [
        ("さんこう", &["参考", "山行"]),
        ("さんか", &["参加", "産科", "傘下"]),
        ("じさん", &["持参", "自賛"]),
        ("ひろう", &["拾う", "披露", "疲労"]),
        ("ひろい", &["広い", "拾い"]),
        ("まいり", &["参り"]),
        ("まいる", &["参る"]),
        ("じゅうばん", &["十番", "拾番"]),
        ("さんかい", &["参会", "散会", "山海"]),
        // 1 文字の表記は根拠にしない（`たくさん` の `さん` で `2参枚で沢山` を通さない）
        ("さん", &["参", "三", "山"]),
        ("まい", &["参", "枚", "毎"]),
        ("じゅう", &["十", "拾", "銃"]),
        ("たくさん", &["沢山"]),
        ("みほん", &["見本"]),
        ("かい", &["回", "会", "階"]),
        ("ばん", &["番", "晩"]),
    ];
    FixedSource {
        map: entries.iter().map(|(r, s)| (*r, s.to_vec())).collect(),
        calls: Cell::new(0),
    }
}

/// run ごとの候補を 1 つずつ与えて、連結した候補が通るか
fn passes(reading: &str, run_cands: &[&str], source: Option<&dyn SurfaceSource>) -> bool {
    passes_with_stats(reading, run_cands, source).0
}

fn passes_with_stats(
    reading: &str,
    run_cands: &[&str],
    source: Option<&dyn SurfaceSource>,
) -> (bool, LicenseStats) {
    let runs = split_by_digits(reading);
    assert_eq!(
        runs.len(),
        run_cands.len(),
        "run の数と候補の数が合わない: {reading:?} {runs:?}"
    );
    let run_candidates: Vec<Vec<String>> = run_cands.iter().map(|c| vec![c.to_string()]).collect();
    let candidate: String = run_cands.concat();
    let (out, stats) = combine_and_verify(reading, &runs, &run_candidates, 9, source);
    (out == vec![candidate], stats)
}

type Case = (&'static str, &'static str, &'static [&'static str], bool);

/// (区分, 読み, run ごとの候補, 期待)。期待は計画書 9 節「対照試験と 47 組の回帰確認」
#[rustfmt::skip]
const CASES_47: &[Case] = &[
    ("required", "2まいをさんこうに", &["2", "参枚を参考に"], false),
    ("required", "3まいをさんこうに", &["参", "枚を参考に"], true),
    ("issue", "2まいめをさんこう", &["2", "枚目を参考"], true),
    ("issue", "おとこは2まいめをさんこうにする", &["男は", "2", "枚目を参考にする"], true),
    ("daily", "3にんがさんか", &["3", "人が参加"], true),
    ("daily", "5こじさん", &["5", "個持参"], true),
    ("daily", "100えんひろう", &["100", "円拾う"], true),
    ("daily", "さんこうに2まい", &["参考に", "2", "枚"], true),
    ("mixed1", "3まい", &["3", "枚"], true),
    ("mixed1", "3まい", &["三", "枚"], true),
    ("mixed1", "3まい", &["参", "枚"], true),
    ("mixed1", "1まい", &["壱", "枚"], true),
    ("mixed1", "2まい", &["弐", "枚"], true),
    ("mixed1", "4まい", &["四", "枚"], true),
    ("unit", "10えん", &["壱拾", "円"], true),
    ("unit", "3000えん", &["参千", "円"], true),
    ("unit", "13にち", &["壱拾参", "日"], true),
    ("unit", "5じゅう", &["5", "拾"], true),
    ("decimal", "12.3えん", &["壱拾弐", ".", "参", "円"], true),
    ("decimal", "12.3えん", &["十二", ".", "三", "円"], true),
    ("decimal", "2.5ばい", &["弐", ".", "五", "倍"], true),
    ("multi", "1まいと2まい", &["壱", "枚と", "弐", "枚"], true),
    ("multi", "1まいと2まい", &["1", "枚と", "弐", "枚"], true),
    ("multi", "5まん5せん", &["5", "万", "5", "千"], true),
    ("multi", "3まいと2まいをさんこう", &["参", "枚と", "2", "枚を参考"], true),
    ("kana-num", "2まいとさんまい", &["2", "枚と参枚"], false),
    ("kana-num", "2まいとさんまい", &["2", "枚と三枚"], false),
    ("other-rd", "2じにまいります", &["2", "時に参ります"], true),
    ("reject", "2まい", &["2", "参枚"], false),
    ("reject", "2まい", &["2", "3枚"], false),
    ("reject", "10えん", &["10", "拾円"], false),
    ("reject", "5えん", &["5", "万円"], false),
    ("reject", "さんこうに2まい", &["参考に", "2", "参枚"], false),
    ("added", "2まいをさんこうに", &["2", "参枚をみほんに"], false),
    ("added", "2まいでたくさん", &["2", "参枚で沢山"], false),
    ("added", "10えんでひろい", &["10", "拾円で広い"], false),
    ("added-ok", "2まいでたくさん", &["2", "枚で沢山"], true),
    ("added-ok", "10えんでひろい", &["10", "円で広い"], true),
    ("same-rd", "さんこうとさんこうに2まい", &["参考と参考に", "2", "枚"], true),
    ("same-rd", "さんこうに2まい", &["参考参考に", "2", "枚"], false),
    ("same-rd", "さんこうとさんこうに2まい", &["参考と参考に", "2", "参枚"], false),
    ("overlap", "2こじさんか", &["2", "個持参加"], true),
    ("overlap", "2こじさんか", &["2", "持参と参加"], false),
    ("reorder", "さんこうとさんかに2まい", &["参加と参考に", "2", "枚"], false),
    ("num-entry", "2ばんとじゅうばん", &["2", "番と拾番"], true),
    ("num-entry", "2ばんとじゅうばん", &["2", "番と十番"], false),
    ("misplace", "2かいのさんかい", &["2", "参会の散会"], true),
];

/// 対照 3 組
#[rustfmt::skip]
const CONTROLS: &[Case] = &[
    ("control", "2ばんとじゅうばん", &["2", "番と拾番"], true),
    ("control", "2ばん", &["2", "番と拾番"], false),
    ("control", "2ばんとじゅうばん", &["2", "番と拾番と拾番"], false),
];

/// 順序と探索、`1十` / `壱十`
#[rustfmt::skip]
const EXTRA: &[Case] = &[
    ("order", "さんこうとさんこうに2まい", &["参考と参考に", "2", "枚"], true),
    ("order", "さんこうとさんかに2まい", &["参加と参考に", "2", "枚"], false),
    ("unit", "1じゅう", &["1", "十"], true),
    ("unit", "1じゅう", &["壱", "十"], false),
];

fn all_cases() -> impl Iterator<Item = &'static Case> {
    CASES_47.iter().chain(CONTROLS).chain(EXTRA)
}

#[test]
fn case_table_has_47_and_3() {
    assert_eq!(CASES_47.len(), 47);
    assert_eq!(CONTROLS.len(), 3);
}

#[test]
fn fixed_table_meets_expected_results() {
    let source = fixed_source();
    let mut failures = Vec::new();
    for (kind, reading, cands, expected) in all_cases() {
        let got = passes(reading, cands, Some(&source));
        if got != *expected {
            failures.push(format!(
                "[{kind}] {reading} → {} : expected {expected}, got {got}",
                cands.concat()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn without_dictionary_matches_current_judgement() {
    for (_, reading, cands, _) in all_cases() {
        let current = verify_digits_preserved(reading, &cands.concat());
        assert_eq!(
            passes(reading, cands, None),
            current,
            "{reading} → {}",
            cands.concat()
        );
    }
}

#[test]
fn daiji_from_digit_run_is_always_counted() {
    // 数字 run 由来の `参` は対象にならない（除外の根拠となる読みを消費しない）
    let source = fixed_source();
    for (reading, cands) in [
        ("3まい", &["参", "枚"][..]),
        ("12.3えん", &["壱拾弐", ".", "参", "円"][..]),
        ("3まいをさんこうに", &["参", "枚を参考に"][..]),
    ] {
        let runs = split_by_digits(reading);
        let run_candidates: Vec<Vec<String>> = cands.iter().map(|c| vec![c.to_string()]).collect();
        let kana_runs: Vec<bool> = runs.iter().map(|r| !r.is_literal()).collect();
        let c = &combine_runs_with_origin(&run_candidates, 9)[0];
        let tokens = scan_numeric_tokens(&c.text, reading);
        let targets = tokens
            .iter()
            .filter(|t| is_exclusion_target(t, &c.parts, &kana_runs))
            .count();
        // `参枚を参考に` の対象は `参考` の `参` だけ
        let expected = usize::from(reading == "3まいをさんこうに");
        assert_eq!(targets, expected, "{reading}");
        assert!(passes(reading, cands, Some(&source)));
    }
}

// ─── 生成元の区間 ─────────────────────────────────────────────────────────────

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn origin_parts_match_text() {
    let run_candidates = vec![
        strings(&["2024", "２０２４", "二〇二四"]),
        strings(&["年", "ねん"]),
        strings(&["4"]),
        strings(&["月", "がつ"]),
    ];
    let combined = combine_runs_with_origin(&run_candidates, 5);
    assert_eq!(combined.len(), 5);
    for c in &combined {
        let chars: Vec<char> = c.text.chars().collect();
        let mut rebuilt = String::new();
        let mut pos = 0;
        for p in &c.parts {
            assert_eq!(p.char_start, pos);
            let s = &run_candidates[p.run][p.cand];
            assert_eq!(p.char_len, s.chars().count());
            let slice: String = chars[p.char_start..p.char_start + p.char_len]
                .iter()
                .collect();
            assert_eq!(&slice, s);
            rebuilt.push_str(s);
            pos += p.char_len;
        }
        assert_eq!(rebuilt, c.text);
        assert_eq!(pos, chars.len());
    }
    assert_eq!(combined[0].text, "2024年4月");
    assert_eq!(combined[1].text, "2024年4がつ");
}

#[test]
fn origin_skips_empty_runs() {
    let run_candidates = vec![strings(&["2"]), vec![], strings(&["枚", "まい"])];
    let combined = combine_runs_with_origin(&run_candidates, 5);
    assert_eq!(combined.len(), 2);
    assert_eq!(
        combined[0].parts,
        vec![
            PartRef {
                run: 0,
                cand: 0,
                char_start: 0,
                char_len: 1
            },
            PartRef {
                run: 2,
                cand: 0,
                char_start: 1,
                char_len: 1
            },
        ]
    );
    assert_eq!(combined[1].parts[1].cand, 1);
    assert_eq!(combined[1].text, "2まい");
}

#[test]
fn origin_truncates_to_limit() {
    let run_candidates = vec![
        strings(&["1", "１", "一", "壱"]),
        strings(&["a", "b", "c", "d"]),
        strings(&["x", "y"]),
    ];
    let combined = combine_runs_with_origin(&run_candidates, 3);
    // 途中の打ち切り（limit * 2）と最後の切り詰めの順序は従来の combine_runs と同じ
    let texts: Vec<&str> = combined.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, vec!["1ax", "1ay", "1bx"]);
    assert!(combine_runs_with_origin(&[], 3).is_empty());
}

// ─── 準備を始める条件 ─────────────────────────────────────────────────────────

#[test]
fn no_preparation_when_current_judgement_passes_or_no_target() {
    let source = fixed_source();
    for (reading, cands) in [
        // 現行の判定で通る（数字 run 由来の大字・単位の読み飛ばし・対象なし）
        ("3まい", &["参", "枚"][..]),
        ("5じゅう", &["5", "拾"][..]),
        ("2まいでたくさん", &["2", "枚で沢山"][..]),
        // 現行で拒否されるが対象が無い
        ("2まい", &["2", "3枚"][..]),
        ("2まいとさんまい", &["2", "枚と三枚"][..]),
        ("5えん", &["5", "万円"][..]),
    ] {
        source.calls.set(0);
        let (_, stats) = passes_with_stats(reading, cands, Some(&source));
        assert_eq!(stats.tables_built, 0, "{reading}");
        assert_eq!(stats.cands_matched, 0, "{reading}");
        assert_eq!(stats.dp_candidates, 0, "{reading}");
        assert_eq!(source.calls.get(), 0, "{reading}");
    }
}

#[test]
fn no_preparation_for_candidates_cut_by_limit() {
    // かな run の 2 番目の候補（対象を含む）は、limit = 1 の切り詰めで使われない
    let source = fixed_source();
    let reading = "2まいめをさんこう";
    let runs = split_by_digits(reading);
    let run_candidates = vec![strings(&["2"]), strings(&["枚目をさんこう", "枚目を参考"])];
    let (out, stats) = combine_and_verify(reading, &runs, &run_candidates, 1, Some(&source));
    assert_eq!(out, vec!["2枚目をさんこう"]);
    assert_eq!(stats.tables_built, 0);
    assert_eq!(source.calls.get(), 0);

    // limit = 2 なら使われ、そこで初めて作る
    let (out, stats) = combine_and_verify(reading, &runs, &run_candidates, 2, Some(&source));
    assert_eq!(out, vec!["2枚目をさんこう", "2枚目を参考"]);
    assert_eq!(stats.tables_built, 1);
    assert_eq!(stats.cands_matched, 1);
    assert_eq!(stats.dp_candidates, 1);
}

#[test]
fn table_and_matches_are_built_once_per_conversion() {
    // 同じかな run の候補を複数の連結候補が使っても、照合表は 1 回だけ作る
    let source = fixed_source();
    let reading = "2まいめをさんこう";
    let runs = split_by_digits(reading);
    let run_candidates = vec![
        strings(&["2", "２", "二"]),
        strings(&["枚目を参考", "毎目を参考"]),
    ];
    let (out, stats) = combine_and_verify(reading, &runs, &run_candidates, 9, Some(&source));
    assert_eq!(
        out,
        vec![
            "2枚目を参考",
            "2毎目を参考",
            "２枚目を参考",
            "２毎目を参考",
            "二枚目を参考",
            "二毎目を参考"
        ]
    );
    assert_eq!(stats.tables_built, 1);
    assert_eq!(stats.cands_matched, 2);
    assert_eq!(stats.dp_candidates, 6);
    let n = "まいめをさんこう".chars().count();
    assert_eq!(stats.substrings, n * (n + 1) / 2);
    // この読みの部分文字列に同じ文字列は無いので、引いた回数 = 部分文字列の数
    assert_eq!(stats.lookups, source.calls.get());
    assert_eq!(stats.lookups, stats.substrings);
}

#[test]
fn same_substring_is_looked_up_once() {
    let source = fixed_source();
    let (ok, stats) = passes_with_stats(
        "さんこうとさんこうに2まい",
        &["参考と参考に", "2", "枚"],
        Some(&source),
    );
    assert!(ok);
    assert!(stats.lookups < stats.substrings);
    assert_eq!(stats.lookups, source.calls.get());
}

// ─── 繰り返し語 ───────────────────────────────────────────────────────────────

fn repeated(n: usize, tail: &str) -> (String, Vec<String>) {
    let reading = format!("{}に2まい", "さんこうと".repeat(n));
    let cands = vec![
        format!("{}に", "参考と".repeat(n)),
        "2".into(),
        tail.to_string(),
    ];
    (reading, cands)
}

#[test]
fn repeated_words_pass_and_reject() {
    let source = fixed_source();
    for n in [4, 10, 20, 40] {
        for (tail, expected) in [("枚", true), ("参枚", false)] {
            let (reading, cands) = repeated(n, tail);
            let refs: Vec<&str> = cands.iter().map(String::as_str).collect();
            let (ok, stats) = passes_with_stats(&reading, &refs, Some(&source));
            assert_eq!(ok, expected, "N={n} tail={tail}");
            assert_eq!(stats.dp_candidates, 1);
            // 状態数は N の多項式（計画書の模擬実装: N=40 で 822）
            assert!(stats.dp_states <= (4 * n + 4) * (n + 2), "N={n}");
        }
    }
}

// ─── 全探索との照合 ───────────────────────────────────────────────────────────

/// テスト専用の全探索。対象ごとに「数える」か「覆う組を 1 つ割り当てる」かを
/// すべて試し、使う組の集合が非重複・順序の条件を満たし、数字列が入力と一致する
/// 割当てが 1 つでもあれば通す。
fn exhaustive(segments: &[Segment], pairs: &[Pair], input: &[u8]) -> bool {
    fn consistent(used: &[usize], pairs: &[Pair]) -> bool {
        for (x, &a) in used.iter().enumerate() {
            for &b in &used[x + 1..] {
                if a == b {
                    continue;
                }
                let (p, q) = (&pairs[a], &pairs[b]);
                if !(p.oj <= q.oi || q.oj <= p.oi) {
                    return false;
                }
                if p.run == q.run {
                    let read_ok = p.rj <= q.ri || q.rj <= p.ri;
                    let order_ok = (p.oi < q.oi) == (p.ri < q.ri);
                    if !read_ok || !order_ok {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn go(
        i: usize,
        segments: &[Segment],
        pairs: &[Pair],
        input: &[u8],
        digits: &mut Vec<u8>,
        used: &mut Vec<usize>,
    ) -> bool {
        let Some(seg) = segments.get(i) else {
            return digits.as_slice() == input && consistent(used, pairs);
        };
        let len = digits.len();
        match seg {
            Segment::Fixed(f) => {
                digits.extend_from_slice(f);
                let ok = go(i + 1, segments, pairs, input, digits, used);
                digits.truncate(len);
                ok
            }
            Segment::Target { digits: d, covers } => {
                digits.extend_from_slice(d);
                let counted = go(i + 1, segments, pairs, input, digits, used);
                digits.truncate(len);
                if counted {
                    return true;
                }
                for &m in covers {
                    used.push(m);
                    let ok = go(i + 1, segments, pairs, input, digits, used);
                    used.pop();
                    if ok {
                        return true;
                    }
                }
                false
            }
        }
    }

    go(0, segments, pairs, input, &mut Vec::new(), &mut Vec::new())
}

/// 連結候補 1 件について、動的計画法と全探索の結果を比べる。
/// 戻り値は (現行で通るか, 対象の数, 動的計画法の結果)
fn compare(source: &FixedSource, reading: &str, run_cands: &[String]) -> (bool, usize, bool) {
    let runs = split_by_digits(reading);
    assert_eq!(runs.len(), run_cands.len(), "{reading}");
    let run_candidates: Vec<Vec<String>> = run_cands.iter().map(|c| vec![c.clone()]).collect();
    let kana_runs: Vec<bool> = runs.iter().map(|r| !r.is_literal()).collect();
    let c = combine_runs_with_origin(&run_candidates, 9).remove(0);
    let tokens = scan_numeric_tokens(&c.text, reading);
    let input = digits_of_tokens(&scan_numeric_tokens(reading, reading));
    let current = digits_of_tokens(&tokens) == input;
    let mut ctx = LicenseContext::new(source, &runs, &run_candidates);
    let (segments, pairs) = ctx.segments(&c, &tokens, &kana_runs);
    let n_targets = segments
        .iter()
        .filter(|s| matches!(s, Segment::Target { .. }))
        .count();
    let dp = judge_segments(&segments, &pairs, input.as_bytes()).accepted;
    let ex = exhaustive(&segments, &pairs, input.as_bytes());
    assert_eq!(dp, ex, "{reading} → {}", c.text);
    // 動的計画法は「どの対象も除外しない」経路を含むので、現行で通るものは必ず通す
    if current {
        assert!(dp, "{reading} → {}", c.text);
    }
    (current, n_targets, dp)
}

#[test]
fn dp_matches_exhaustive_on_all_cases() {
    let source = fixed_source();
    for (_, reading, cands, _) in all_cases() {
        let cands: Vec<String> = cands.iter().map(|s| s.to_string()).collect();
        compare(&source, reading, &cands);
    }
    for n in 1..=6 {
        for tail in ["枚", "参枚"] {
            let (reading, cands) = repeated(n, tail);
            compare(&source, &reading, &cands);
        }
    }
}

#[test]
fn dp_matches_exhaustive_on_small_grid() {
    // 読み `2` + かな 1〜2 個 × 候補 `2` + かな出力 1〜3 個（計画書の網羅と同じ組み立て）
    let source = fixed_source();
    let kana = ["さんこう", "さんか", "じゅうばん", "と", "まい", "ばん"];
    let outs = ["参考", "参加", "拾番", "参", "拾", "と", "枚", "3"];
    let mut readings = Vec::new();
    for a in kana {
        readings.push(a.to_string());
        for b in kana {
            readings.push(format!("{a}{b}"));
        }
    }
    let mut outputs = Vec::new();
    for a in outs {
        outputs.push(a.to_string());
        for b in outs {
            outputs.push(format!("{a}{b}"));
            for c in outs {
                outputs.push(format!("{a}{b}{c}"));
            }
        }
    }
    let (mut total, mut accepted, mut rescued, mut max_targets) = (0, 0, 0, 0);
    for r in &readings {
        let reading = format!("2{r}");
        for o in &outputs {
            let (current, n_targets, dp) = compare(&source, &reading, &["2".into(), o.clone()]);
            total += 1;
            accepted += usize::from(dp);
            rescued += usize::from(dp && !current);
            max_targets = max_targets.max(n_targets);
        }
    }
    assert_eq!(total, 42 * 584);
    assert!(rescued > 0 && accepted > rescued, "{accepted} {rescued}");
    assert!(max_targets >= 3);
}

#[test]
fn dp_matches_exhaustive_on_random_inputs() {
    // 固定シードの線形合同法で、[数字, かな] と [かな, 数字, かな] を組み立てる
    let source = fixed_source();
    let mut seed: u64 = 0x5eed_0053;
    let mut next = move |n: usize| {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as usize) % n
    };
    let digits: [(&str, &[&str]); 4] = [
        ("2", &["2", "２", "二", "弐"]),
        ("3", &["3", "三", "参"]),
        ("10", &["10", "十", "壱拾"]),
        ("12", &["12", "十二", "壱拾弐"]),
    ];
    let kana = [
        "さんこう",
        "さんか",
        "じさん",
        "ひろう",
        "まいり",
        "じゅうばん",
        "さんかい",
        "たくさん",
        "みほん",
        "と",
        "に",
        "まい",
        "ばん",
    ];
    let outs = [
        "参考", "参加", "持参", "拾番", "拾う", "参り", "参会", "散会", "沢山", "見本", "参", "拾",
        "三", "3", "と", "に", "枚", "番", "を",
    ];
    let kana_seq = |next: &mut dyn FnMut(usize) -> usize| {
        let len = 1 + next(3);
        (0..len).map(|_| kana[next(kana.len())]).collect::<String>()
    };
    let out_seq = |next: &mut dyn FnMut(usize) -> usize| {
        let len = 1 + next(4);
        (0..len).map(|_| outs[next(outs.len())]).collect::<String>()
    };
    let mut max_targets = 0;
    for i in 0..5000 {
        let (d, dcands) = digits[next(digits.len())];
        let dc = dcands[next(dcands.len())].to_string();
        let (reading, cands) = if i % 2 == 0 {
            let k = kana_seq(&mut next);
            (format!("{d}{k}"), vec![dc, out_seq(&mut next)])
        } else {
            let k1 = kana_seq(&mut next);
            let k2 = kana_seq(&mut next);
            let o1 = out_seq(&mut next);
            let o2 = out_seq(&mut next);
            (format!("{k1}{d}{k2}"), vec![o1, dc, o2])
        };
        let (_, n, _) = compare(&source, &reading, &cands);
        max_targets = max_targets.max(n);
    }
    assert!(max_targets >= 4);
}

// ─── 実辞書 ───────────────────────────────────────────────────────────────────

fn installed_dict() -> rakukan_dict::DictStore {
    let base = std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA");
    let path = std::path::Path::new(&base).join("rakukan/dict/rakukan.dict");
    assert!(path.exists(), "installed dictionary not found: {path:?}");
    rakukan_dict::DictStore::load(None, Some(&path), None).expect("load dictionary")
}

#[test]
#[ignore = "インストール済み辞書（%LOCALAPPDATA%\\rakukan\\dict\\rakukan.dict）が必要"]
fn installed_dictionary_meets_expected_results() {
    let dict = installed_dict();
    let mut failures = Vec::new();
    for (kind, reading, cands, expected) in all_cases() {
        let (got, stats) = passes_with_stats(reading, cands, Some(&dict));
        println!(
            "[{kind}] {reading} → {} : {} (expected {}) lookups={} entries={} pairs={} dp_states={} total={}us",
            cands.concat(),
            if got { "pass" } else { "reject" },
            if *expected { "pass" } else { "reject" },
            stats.lookups,
            stats.table_entries,
            stats.pairs_found,
            stats.dp_states,
            stats.total_time().as_micros()
        );
        if got != *expected {
            failures.push(format!("[{kind}] {reading} → {}", cands.concat()));
        }
    }
    for n in [4, 10, 20, 40] {
        for (tail, expected) in [("枚", true), ("参枚", false)] {
            let (reading, cands) = repeated(n, tail);
            let refs: Vec<&str> = cands.iter().map(String::as_str).collect();
            let (got, stats) = passes_with_stats(&reading, &refs, Some(&dict));
            println!(
                "[repeat] N={n} tail={tail}: {} (expected {}) substr={} lookups={} entries={} pairs={} dp_states={} trans={} table={}us match={}us dp={}us",
                if got { "pass" } else { "reject" },
                if expected { "pass" } else { "reject" },
                stats.substrings,
                stats.lookups,
                stats.table_entries,
                stats.pairs_found,
                stats.dp_states,
                stats.dp_transitions,
                stats.table_time.as_micros(),
                stats.match_time.as_micros(),
                stats.dp_time.as_micros()
            );
            if got != expected {
                failures.push(format!("[repeat] N={n} tail={tail}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
