//! 数字保存の検証の性能の比較計測（#53）。
//!
//! ```text
//! cargo test --release -p rakukan-engine --lib digits::bench -- --ignored --nocapture --test-threads=1
//! ```
//!
//! 変更前のコミットでは、このファイルの `tail` と `full` だけを変更前の処理
//! （`combine_runs` → `verify_digits_preserved` → 重複除去、および辞書引数の無い
//! `convert_with_digit_protection`）に差し替えて同じ計測を流す。入力・候補・候補数・
//! 反復回数は共通。

use super::*;
use rakukan_dict::DictStore;
use std::time::{Duration, Instant};

const WARMUP: usize = 30;
const ITER: usize = 300;
const MODEL_ITER: usize = 100;

// ─── 差し替える部分 ───────────────────────────────────────────────────────────

/// 変換器を呼んだ後の、連結・検証・重複除去
fn tail(
    reading: &str,
    runs: &[Run],
    rc: &[Vec<String>],
    limit: usize,
    dict: Option<&DictStore>,
) -> (Vec<String>, Option<crate::digit_license::LicenseStats>) {
    let (out, stats) = combine_and_verify(
        reading,
        runs,
        rc,
        limit,
        dict.map(|d| d as &dyn SurfaceSource),
    );
    (out, Some(stats))
}

/// 変換器を含む変換 1 回
fn full(converter: &KanaKanjiConverter, reading: &str, dict: Option<&DictStore>) -> Vec<String> {
    let order = default_digit_candidates_order();
    convert_with_digit_protection(
        converter,
        reading,
        "",
        9,
        &order,
        false,
        false,
        dict.map(|d| d as &dyn SurfaceSource),
    )
    .expect("convert")
}

// ─── 共通 ─────────────────────────────────────────────────────────────────────

fn installed_dict() -> DictStore {
    let base = std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA");
    let path = std::path::Path::new(&base).join("rakukan/dict/rakukan.dict");
    DictStore::load(None, Some(&path), None).expect("load installed dictionary")
}

fn product(prefixes: &[&str], suffixes: &[&str], k: usize) -> Vec<String> {
    let mut out = Vec::new();
    for p in prefixes {
        for s in suffixes {
            out.push(format!("{p}{s}"));
        }
    }
    assert!(out.len() >= k);
    out.truncate(k);
    out
}

/// 繰り返しでない自然文のかな（辞書に当たる短い部分文字列が多い）
const NATURAL_KANA: &str = "きのうはあさからあめがふっていたのでいえでほんをよんでいたがひるすぎにはれてきたのでちかくのこうえんまであるいていきべんちにすわってしばらくそらをながめていたらともだちからでんわがかかってきてこんどのしゅうまつにみんなでうみへいこうというはなしになった";

/// 繰り返しでない かな（線形合同法。辞書に当たらない長い部分文字列が多い）
fn aperiodic_kana(n: usize) -> String {
    let kana: Vec<char> = "あいうえおかきくけこさしすせそたちつてとなにぬねのはひふへほまみむめもやゆよらりるれろわをん"
        .chars()
        .collect();
    let mut x: u32 = 12345;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            kana[(x >> 16) as usize % kana.len()]
        })
        .collect()
}

struct Scenario {
    name: String,
    reading: String,
    rc: Vec<Vec<String>>,
}

/// 変換器を使わない合成入力。数字 run は実際の `literal_candidates`、かな run は候補数 k の合成
fn scenarios() -> Vec<Scenario> {
    let order = default_digit_candidates_order();
    let digit = |s: &str| digit_candidates(s, &order);
    let mut out = Vec::new();
    for k in [5, 19, 30] {
        // 対象なし（現行で一部の候補が拒否される）
        out.push(Scenario {
            name: format!("mixed/no-target k={k}"),
            reading: "5えん".into(),
            rc: vec![
                digit("5"),
                product(
                    &["円", "縁", "艶", "宴", "園", "苑"],
                    &["", "だ", "です", "の", "万円"],
                    k,
                ),
            ],
        });
        // 現行の判定で全候補が通る
        out.push(Scenario {
            name: format!("mixed/all-pass k={k}"),
            reading: "2まいでたくさん".into(),
            rc: vec![
                digit("2"),
                product(
                    &["枚で", "毎で", "舞で", "米で", "枚でも", "まいで"],
                    &["沢山", "たくさん", "多く", "タクサン", "沢さん"],
                    k,
                ),
            ],
        });
        // 対象を含み、現行で拒否される候補がある（#53 の読み）
        out.push(Scenario {
            name: format!("mixed/target k={k}"),
            reading: "2まいめをさんこう".into(),
            rc: vec![
                digit("2"),
                product(
                    &["枚目を", "枚めを", "回目を", "枚目の", "丁目を", "毎目を"],
                    &["参考", "山行", "さんこう", "参考に", "三稿"],
                    k,
                ),
            ],
        });
        out.push(Scenario {
            name: format!("mixed/target-long k={k}"),
            reading: "おとこは2まいめをさんこうにする".into(),
            rc: vec![
                vec!["男は".into(), "おとこは".into()],
                digit("2"),
                product(
                    &["枚目を", "枚めを", "回目を", "枚目の", "丁目を", "毎目を"],
                    &[
                        "参考にする",
                        "山行にする",
                        "さんこうにする",
                        "参考する",
                        "三稿にする",
                    ],
                    k,
                ),
            ],
        });
    }
    // 繰り返しでない長い かな run（n 文字、末尾付近に `さんこう`）。照合表の読みの
    // 長さの上限（`MAX_READING_CHARS`）の効果を見る
    for (kind, source) in [
        ("random", aperiodic_kana(200)),
        ("natural", NATURAL_KANA.to_string()),
    ] {
        for n in [30, 60, 120] {
            let filler: String = source
                .chars()
                .take(n - "まいさんこうに".chars().count())
                .collect();
            out.push(Scenario {
                name: format!("aperiodic/{kind} n={n}"),
                reading: format!("2まい{filler}さんこうに"),
                rc: vec![digit("2"), vec![format!("枚{filler}参考に")]],
            });
        }
    }
    for n in [10, 20, 40] {
        for tail in ["枚", "参枚"] {
            out.push(Scenario {
                name: format!("repeat N={n} tail={tail}"),
                reading: format!("{}に2まい", "さんこうと".repeat(n)),
                rc: vec![
                    vec![format!("{}に", "参考と".repeat(n))],
                    digit("2"),
                    vec![tail.into()],
                ],
            });
        }
    }
    out
}

struct Summary {
    median: f64,
    p90: f64,
    p99: f64,
    max: f64,
}

fn summarize(mut v: Vec<Duration>) -> Summary {
    v.sort();
    let at = |q: f64| {
        let i = ((v.len() as f64 * q).ceil() as usize).clamp(1, v.len()) - 1;
        v[i].as_nanos() as f64 / 1000.0
    };
    Summary {
        median: at(0.5),
        p90: at(0.9),
        p99: at(0.99),
        max: v.last().unwrap().as_nanos() as f64 / 1000.0,
    }
}

fn print_row(name: &str, s: &Summary, extra: &str) {
    println!(
        "| {name} | {:.2} | {:.2} | {:.2} | {:.2} |{extra}",
        s.median, s.p90, s.p99, s.max
    );
}

#[test]
#[ignore = "計測（インストール済み辞書が必要）"]
fn synthetic() {
    let dict = installed_dict();
    println!("| 場面 | 中央値 µs | p90 | p99 | 最大 |");
    for sc in scenarios() {
        let runs = split_by_digits(&sc.reading);
        assert_eq!(runs.len(), sc.rc.len(), "{}", sc.name);
        let mut times = Vec::with_capacity(ITER);
        let mut phase: Vec<[Duration; 4]> = Vec::with_capacity(ITER);
        let mut last = None;
        let mut out = Vec::new();
        for i in 0..WARMUP + ITER {
            let t = Instant::now();
            let (o, stats) = tail(&sc.reading, &runs, &sc.rc, 9, Some(&dict));
            let e = t.elapsed();
            if i >= WARMUP {
                times.push(e);
                if let Some(s) = &stats {
                    phase.push([s.origin_time, s.table_time, s.match_time, s.dp_time]);
                }
            }
            out = o;
            last = stats;
        }
        let mut extra = format!(" out[0]={:?} n={}", out[0], out.len());
        if let Some(s) = last {
            let med = |k: usize| {
                let mut v: Vec<Duration> = phase.iter().map(|p| p[k]).collect();
                v.sort();
                v[v.len() / 2].as_nanos() as f64 / 1000.0
            };
            let combined = combine_runs_with_origin(&sc.rc, 9).len();
            let mem: usize = s.memory_estimate(combined).iter().map(|(_, b)| b).sum();
            extra.push_str(&format!(
                " | (0)={:.2} (1)={:.2} (2)={:.2} (3)={:.2} | substr={} lookups={} entries={} pairs={} dp_cands={} states={} trans={} | mem≈{}B",
                med(0),
                med(1),
                med(2),
                med(3),
                s.substrings,
                s.lookups,
                s.table_entries,
                s.pairs_found,
                s.dp_candidates,
                s.dp_states,
                s.dp_transitions,
                mem
            ));
            if sc.name.starts_with("repeat N=40 tail=参枚") || sc.name == "mixed/target k=30" {
                for (k, b) in s.memory_estimate(combined) {
                    println!("    mem {k}: {b} B");
                }
            }
        }
        print_row(&sc.name, &summarize(times), &extra);
    }

    // 全部リテラルの経路（combine_runs のラッパー経由）
    for reading in ["USB-C", "A1-B2", "3D-2x"] {
        let runs = split_by_digits(reading);
        let rc: Vec<Vec<String>> = runs
            .iter()
            .map(|r| half_full_literal_candidates(r, false, false))
            .collect();
        let mut times = Vec::with_capacity(ITER);
        let mut out = Vec::new();
        for i in 0..WARMUP + ITER {
            let t = Instant::now();
            out = combine_runs(&rc, 9);
            if i >= WARMUP {
                times.push(t.elapsed());
            }
        }
        print_row(
            &format!("literal {reading}"),
            &summarize(times),
            &format!(" out[0]={:?} n={}", out[0], out.len()),
        );
    }
}

#[test]
#[ignore = "計測（実モデルとインストール済み辞書が必要）"]
fn with_model() {
    use crate::kanji::Backend;
    let dict = installed_dict();
    let backend = Backend::from_variant_id("jinen-v1-small-q5").expect("load model");
    let converter = KanaKanjiConverter::new(backend).expect("converter");
    println!("| 読み | 中央値 µs | p90 | p99 | 最大 |");
    for reading in [
        "2まいめをさんこう",
        "おとこは2まいめをさんこうにする",
        "3にんがさんか",
        "2024ねんのけいかく",
        "5まんえんをはらう",
        "わたしはがくせいです",
        "USB-C",
    ] {
        let mut times = Vec::with_capacity(MODEL_ITER);
        let mut out = Vec::new();
        for i in 0..5 + MODEL_ITER {
            let t = Instant::now();
            out = full(&converter, reading, Some(&dict));
            if i >= 5 {
                times.push(t.elapsed());
            }
        }
        print_row(reading, &summarize(times), &format!(" {out:?}"));
    }
}
