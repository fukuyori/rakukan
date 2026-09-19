//! 数字保存の検証で、かな run の読みが正当化する大字（`参` / `拾`）を数えない判定（#53）
//!
//! `verify_digits_preserved` は候補の中の大字を数値として数えるので、かな run の
//! 変換結果に `参考` や `拾う` が出ると、入力に無い `3` / `10` が増えたとみなして
//! 正しい候補を捨てる（`2まいめをさんこう` → `2枚目を参考` が拒否される）。
//!
//! ここでは、候補に現れた `参` / `拾` が、入力の読みに対応する辞書の語の一部だと
//! 確かめられたときだけ、数値として数えない。入力に根拠の無い数の書き換え
//! （`2まい` → `2参枚` など）は、現行どおり拒否する。
//!
//! # 判定
//!
//! 除外の対象（`is_exclusion_target`）ごとに、「数える」か「対象を覆う組を割り当てて
//! 数えない」かを選ぶ。組は「かな run の読みの範囲 → 辞書の表記 → 候補中の出力の範囲」。
//! 使う組どうしが、読み・出力の範囲で重ならず、同じかな run の中では読みの順と出力の
//! 順が一致し、数えた分の数字列が入力の数字列と一致する割当てが存在すれば通す。
//!
//! 存在判定は動的計画法で行う（`judge_segments`）。状態は
//! `(入力の数字列の照合済みの長さ, 直前に割り当てた組)`。
//!
//! # 準備は必要になった時点で作る
//!
//! 照合表（かな run の読みの部分文字列を辞書で引いた結果）と出力位置の照合（run 内の
//! 候補ごとの、組の位置）は、現行の判定で拒否され、かつ除外の対象がある連結候補の
//! 判定で初めて必要になったときに作り、変換 1 回の中で再利用する。変換をまたいで
//! 保持しない。
//!
//! # 既知の制限: 読みの長さの上限
//!
//! 照合表で引く読みの部分文字列は `MAX_READING_CHARS` 文字までに限る。上限が無いと
//! 長さ n の かな run で n(n+1)/2 件を引くことになり、ライブ変換は打鍵ごとに照合表を
//! 作り直すので、長文で遅延が 2 乗で増える。上限を入れると引く件数は約 n·L になる。
//!
//! 読みが上限を超える語（`だんじょきょうどうさんかく` → `男女共同参画` など）は除外の
//! 根拠に使わず、その `参` / `拾` は従来どおり数字保存の検証で数える。システム辞書の
//! 生成元（`dictionary00.txt`〜`dictionary09.txt`）で数えると、`参` / `拾` を含む
//! 2 文字以上の表記を持つ組 766 件のうち、読みが 12 文字を超えるのは 13 件で、
//! すべて固有名詞や複合語（`ロシア連邦軍参謀本部情報総局` など）。

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use crate::digits::{Combined, NumericToken, PartRef, Run};

/// 除外の対象になる大字
const TARGET_CHARS: [char; 2] = ['参', '拾'];

/// 照合表で辞書を引く、読みの部分文字列の長さの上限（文字数）。
///
/// これより長い読みの語は除外の根拠に使わない（モジュール冒頭の「既知の制限」）。
pub(crate) const MAX_READING_CHARS: usize = 12;

fn target_index(c: char) -> Option<usize> {
    TARGET_CHARS.iter().position(|&t| t == c)
}

/// 読み（かなの部分文字列）から、辞書の表記を引く手段。
///
/// 本番ではシステム辞書（`DictStore::lookup_system_normal`）、テストでは固定の表。
pub trait SurfaceSource {
    /// 読みに完全一致する表記の一覧
    fn surfaces(&self, reading: &str) -> Vec<String>;
}

impl SurfaceSource for rakukan_dict::DictStore {
    fn surfaces(&self, reading: &str) -> Vec<String> {
        self.lookup_system_normal(reading)
    }
}

/// 除外の対象か。
///
/// 連結後の数字走査で 1 文字の漢数字の並びになる `参` / `拾` のうち、かな run の
/// 候補に属し、単位の読み飛ばしに当たらないもの。`参千` のような複数文字の並びや、
/// 数字 run 由来の大字（`3まい` → `参枚`）は対象にしない。
pub(crate) fn is_exclusion_target(
    tok: &NumericToken,
    parts: &[PartRef],
    kana_runs: &[bool],
) -> bool {
    match tok {
        NumericToken::KanjiRun {
            start,
            len: 1,
            first,
            digits: Some(_),
            unit_skipped: false,
        } if target_index(*first).is_some() => {
            part_at(parts, *start).is_some_and(|p| kana_runs.get(p.run).copied().unwrap_or(false))
        }
        _ => false,
    }
}

/// 文字位置 `pos` を含む区間
fn part_at(parts: &[PartRef], pos: usize) -> Option<&PartRef> {
    parts.get(part_index_at(parts, pos)?)
}

/// 文字位置 `pos` を含む区間の添字
fn part_index_at(parts: &[PartRef], pos: usize) -> Option<usize> {
    let i = parts
        .partition_point(|p| p.char_start <= pos)
        .checked_sub(1)?;
    let p = parts.get(i)?;
    (pos < p.char_start + p.char_len).then_some(i)
}

// ─── 計測 ─────────────────────────────────────────────────────────────────────

/// 変換 1 回分の計測値。計測点は (0)〜(3) の 4 つ。
#[derive(Debug, Default, Clone)]
pub struct LicenseStats {
    /// (0) 生成元の区間の生成・入力の数字列・トークン列の作成と対象の有無の判定
    pub origin_time: Duration,
    /// (1) 照合表の作成
    pub table_time: Duration,
    /// (2) 出力位置の照合
    pub match_time: Duration,
    /// (3) 動的計画法（区切りの列と組の一覧を作る時間を含む）
    pub dp_time: Duration,

    /// 連結候補 1 件のトークン数の最大
    pub tokens_max: usize,
    /// 連結候補 1 件の区間数の最大
    pub parts_max: usize,

    /// (1) 照合表を作った run の数
    pub tables_built: usize,
    /// (1) 読みの部分文字列の数（`MAX_READING_CHARS` 文字以下のもの）
    pub substrings: usize,
    /// (1) 実際に辞書を引いた回数（同じ文字列は 1 回）
    pub lookups: usize,
    /// (1) 辞書から返った表記の数（絞り込み前、引いた分の合計）
    pub surfaces_seen: usize,
    /// (1) 照合表の組（読みの範囲と表記）の数
    pub table_entries: usize,
    /// (1) 照合表に保持した表記の文字数の合計
    pub table_chars: usize,
    /// (1) 辞書検索の結果の保持: 読みの文字列のバイト数の合計
    pub cache_key_bytes: usize,
    /// (1) 辞書検索の結果の保持: 絞り込み後の表記の文字数の合計
    pub cache_surface_chars: usize,
    /// (2) 出力位置の照合をした run 内の候補の数
    pub cands_matched: usize,
    /// (2) 出力位置の照合をした候補の文字数の合計
    pub match_chars: usize,
    /// (2) 見つかった組の数
    pub pairs_found: usize,
    /// (3) 動的計画法にかけた連結候補の数
    pub dp_candidates: usize,
    /// (3) 状態数の合計
    pub dp_states: usize,
    /// (3) 1 回の判定での、1 つの区切りの後の状態数の最大
    pub dp_states_max: usize,
    /// (3) 遷移数の合計
    pub dp_transitions: usize,
    /// (3) 通した連結候補の数
    pub dp_accepted: usize,
}

impl LicenseStats {
    /// 変換 1 回分の追加時間（(0)〜(3) の合計）
    pub fn total_time(&self) -> Duration {
        self.origin_time + self.table_time + self.match_time + self.dp_time
    }

    /// 同時に使う最大のメモリ量の見積もり（確保先ごとのバイト数）。
    ///
    /// 要素数 × 要素の大きさで数え、`HashMap` / `HashSet` は制御バイトと空きを
    /// 1.25 倍で見込む。`combined`（連結候補の文字列と区間）は変換 1 回の間
    /// 生きているので `combined_parts` に区間の分だけを数える（文字列は現行にもある）。
    #[cfg(test)]
    pub(crate) fn memory_estimate(&self, combined_count: usize) -> Vec<(&'static str, usize)> {
        use std::mem::size_of;
        let hashed = |n: usize, entry: usize| n * (entry + 1) * 5 / 4;
        vec![
            (
                "(0) Vec<PartRef>（連結候補の区間）",
                combined_count * self.parts_max * size_of::<PartRef>(),
            ),
            (
                "(0) Vec<NumericToken>（1 件ずつ確保・解放）",
                self.tokens_max * size_of::<NumericToken>(),
            ),
            (
                "(1) HashMap 辞書検索の結果",
                hashed(self.lookups, size_of::<(String, Vec<Vec<char>>)>())
                    + self.cache_key_bytes
                    + self.cache_surface_chars * size_of::<char>(),
            ),
            (
                "(1) 照合表 Vec<TableEntry> と索引",
                self.table_entries * size_of::<TableEntry>()
                    + self.table_chars * size_of::<char>()
                    + self.table_chars * size_of::<(usize, usize)>(),
            ),
            (
                "(2) 出力位置の照合 HashMap<(run, cand), CandMatches>",
                hashed(
                    self.cands_matched,
                    size_of::<((usize, usize), CandMatches)>(),
                ) + self.pairs_found * (size_of::<LocalPair>() + size_of::<usize>())
                    + self.match_chars * size_of::<Vec<usize>>(),
            ),
            (
                "(3) HashSet 状態（現在と次の 2 つ）",
                2 * hashed(self.dp_states_max, size_of::<(usize, usize)>()),
            ),
        ]
    }

    pub(crate) fn log(&self, reading: &str) {
        if self.dp_candidates == 0 && self.tables_built == 0 {
            tracing::trace!(
                "digits::license: reading={:?} origin={}us (no target)",
                reading,
                self.origin_time.as_micros()
            );
            return;
        }
        tracing::debug!(
            "digits::license: reading={:?} total={}us origin={}us table={}us(runs={} substr={} lookups={} entries={}) match={}us(cands={} pairs={}) dp={}us(cands={} accepted={} states={} max={} trans={})",
            reading,
            self.total_time().as_micros(),
            self.origin_time.as_micros(),
            self.table_time.as_micros(),
            self.tables_built,
            self.substrings,
            self.lookups,
            self.table_entries,
            self.match_time.as_micros(),
            self.cands_matched,
            self.pairs_found,
            self.dp_time.as_micros(),
            self.dp_candidates,
            self.dp_accepted,
            self.dp_states,
            self.dp_states_max,
            self.dp_transitions,
        );
    }
}

// ─── 照合表と出力位置の照合 ──────────────────────────────────────────────────

/// 照合表の組: かな run の読みの範囲 `[ri, rj)` と、その読みの表記
struct TableEntry {
    ri: usize,
    rj: usize,
    surface: Vec<char>,
}

/// (1) かな run 1 つ分の照合表
struct RunTable {
    entries: Vec<TableEntry>,
    /// 対象の文字（`TARGET_CHARS` の添字）→ (組, 表記内の位置 q) の一覧
    index: [Vec<(usize, usize)>; 2],
}

/// run 内の候補の中の組（候補内の文字位置）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct LocalPair {
    ri: usize,
    rj: usize,
    oi: usize,
    oj: usize,
}

/// (2) run 内の候補 1 つ分の出力位置の照合
struct CandMatches {
    pairs: Vec<LocalPair>,
    /// 候補内の文字位置 → その位置を覆う組（`pairs` の添字）
    cover: Vec<Vec<usize>>,
}

/// 連結候補の中の組（連結候補内の文字位置）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Pair {
    pub run: usize,
    pub ri: usize,
    pub rj: usize,
    pub oi: usize,
    pub oj: usize,
}

/// 動的計画法の入力の区切り
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Segment {
    /// 対象以外のトークンの数字列の連結
    Fixed(Vec<u8>),
    /// 対象の数字列と、それを覆う組（`pairs` の添字）
    Target { digits: Vec<u8>, covers: Vec<usize> },
}

/// 変換 1 回分の照合の文脈。照合表・出力位置の照合を、必要になった時点で作って持つ。
pub(crate) struct LicenseContext<'a> {
    source: &'a dyn SurfaceSource,
    runs: &'a [Run],
    run_candidates: &'a [Vec<String>],
    /// 読みの部分文字列 → 対象を含む 2 文字以上の表記（変換 1 回の中で 1 回だけ引く）
    lookup_cache: HashMap<String, Vec<Vec<char>>>,
    tables: Vec<Option<RunTable>>,
    matches: HashMap<(usize, usize), CandMatches>,
    table_time: Duration,
    match_time: Duration,
    tables_built: usize,
    substrings: usize,
    lookups: usize,
    surfaces_seen: usize,
    cands_matched: usize,
}

impl<'a> LicenseContext<'a> {
    pub(crate) fn new(
        source: &'a dyn SurfaceSource,
        runs: &'a [Run],
        run_candidates: &'a [Vec<String>],
    ) -> Self {
        Self {
            source,
            runs,
            run_candidates,
            lookup_cache: HashMap::new(),
            tables: (0..runs.len()).map(|_| None).collect(),
            matches: HashMap::new(),
            table_time: Duration::ZERO,
            match_time: Duration::ZERO,
            tables_built: 0,
            substrings: 0,
            lookups: 0,
            surfaces_seen: 0,
            cands_matched: 0,
        }
    }

    pub(crate) fn fill_stats(&self, stats: &mut LicenseStats) {
        stats.table_time += self.table_time;
        stats.match_time += self.match_time;
        stats.tables_built += self.tables_built;
        stats.substrings += self.substrings;
        stats.lookups += self.lookups;
        stats.surfaces_seen += self.surfaces_seen;
        for t in self.tables.iter().flatten() {
            stats.table_entries += t.entries.len();
            stats.table_chars += t.entries.iter().map(|e| e.surface.len()).sum::<usize>();
        }
        for (k, v) in &self.lookup_cache {
            stats.cache_key_bytes += k.len();
            stats.cache_surface_chars += v.iter().map(Vec::len).sum::<usize>();
        }
        stats.cands_matched += self.cands_matched;
        for m in self.matches.values() {
            stats.pairs_found += m.pairs.len();
            stats.match_chars += m.cover.len();
        }
    }

    /// (1) かな run の照合表を、未作成なら作る。
    ///
    /// 引くのは長さ `MAX_READING_CHARS` 以下の部分文字列だけ。
    fn ensure_table(&mut self, run: usize) {
        if self.tables[run].is_some() {
            return;
        }
        let t = Instant::now();
        let reading: Vec<char> = self.runs[run].text().chars().collect();
        let n = reading.len();
        let mut entries = Vec::new();
        let mut index: [Vec<(usize, usize)>; 2] = [Vec::new(), Vec::new()];
        for ri in 0..n {
            let mut sub = String::new();
            for (rj, c) in reading.iter().enumerate().skip(ri).take(MAX_READING_CHARS) {
                sub.push(*c);
                let rj = rj + 1;
                self.substrings += 1;
                if !self.lookup_cache.contains_key(&sub) {
                    let raw = self.source.surfaces(&sub);
                    self.lookups += 1;
                    self.surfaces_seen += raw.len();
                    let mut kept: Vec<Vec<char>> = Vec::new();
                    for s in raw {
                        let chars: Vec<char> = s.chars().collect();
                        if chars.len() >= 2
                            && chars.iter().any(|&c| target_index(c).is_some())
                            && !kept.contains(&chars)
                        {
                            kept.push(chars);
                        }
                    }
                    self.lookup_cache.insert(sub.clone(), kept);
                }
                for surface in &self.lookup_cache[&sub] {
                    let e = entries.len();
                    for (q, &c) in surface.iter().enumerate() {
                        if let Some(k) = target_index(c) {
                            index[k].push((e, q));
                        }
                    }
                    entries.push(TableEntry {
                        ri,
                        rj,
                        surface: surface.clone(),
                    });
                }
            }
        }
        self.tables[run] = Some(RunTable { entries, index });
        self.tables_built += 1;
        self.table_time += t.elapsed();
    }

    /// (2) run 内の候補の出力位置の照合を、未作成なら作る
    fn ensure_matches(&mut self, run: usize, cand: usize) {
        if self.matches.contains_key(&(run, cand)) {
            return;
        }
        self.ensure_table(run);
        let t = Instant::now();
        let table = self.tables[run].as_ref().expect("table built above");
        let text: Vec<char> = self.run_candidates[run][cand].chars().collect();
        let mut pairs: Vec<LocalPair> = Vec::new();
        let mut pair_ids: HashMap<LocalPair, usize> = HashMap::new();
        let mut cover: Vec<Vec<usize>> = vec![Vec::new(); text.len()];
        for (p, &c) in text.iter().enumerate() {
            let Some(k) = target_index(c) else {
                continue;
            };
            for &(e, q) in &table.index[k] {
                let entry = &table.entries[e];
                let len = entry.surface.len();
                let Some(oi) = p.checked_sub(q) else {
                    continue;
                };
                if oi + len > text.len() || text[oi..oi + len] != entry.surface[..] {
                    continue;
                }
                let lp = LocalPair {
                    ri: entry.ri,
                    rj: entry.rj,
                    oi,
                    oj: oi + len,
                };
                let id = *pair_ids.entry(lp).or_insert_with(|| {
                    pairs.push(lp);
                    pairs.len() - 1
                });
                if !cover[p].contains(&id) {
                    cover[p].push(id);
                }
            }
        }
        self.matches
            .insert((run, cand), CandMatches { pairs, cover });
        self.cands_matched += 1;
        self.match_time += t.elapsed();
    }

    /// 連結候補の区切りの列と組の一覧を作る（(1)(2) は必要に応じて作る）
    pub(crate) fn segments(
        &mut self,
        c: &Combined,
        tokens: &[NumericToken],
        kana_runs: &[bool],
    ) -> (Vec<Segment>, Vec<Pair>) {
        let mut segments = Vec::new();
        let mut pairs: Vec<Pair> = Vec::new();
        // 区間（parts の添字）→ その区間の組の、`pairs` での先頭位置
        let mut part_base: HashMap<usize, usize> = HashMap::new();
        let mut fixed: Vec<u8> = Vec::new();

        for tok in tokens {
            if !is_exclusion_target(tok, &c.parts, kana_runs) {
                if let Some(d) = tok.counted_digits() {
                    fixed.extend_from_slice(d.as_bytes());
                }
                continue;
            }
            let NumericToken::KanjiRun {
                start,
                digits: Some(digits),
                ..
            } = tok
            else {
                unreachable!("is_exclusion_target checks the shape");
            };
            let part_idx = part_index_at(&c.parts, *start)
                .expect("is_exclusion_target checks that a part covers the target");
            let part = c.parts[part_idx];
            self.ensure_matches(part.run, part.cand);
            let m = &self.matches[&(part.run, part.cand)];
            let base = *part_base.entry(part_idx).or_insert_with(|| {
                let base = pairs.len();
                pairs.extend(m.pairs.iter().map(|lp| Pair {
                    run: part.run,
                    ri: lp.ri,
                    rj: lp.rj,
                    oi: lp.oi + part.char_start,
                    oj: lp.oj + part.char_start,
                }));
                base
            });
            let covers = m.cover[*start - part.char_start]
                .iter()
                .map(|&i| base + i)
                .collect();
            segments.push(Segment::Fixed(std::mem::take(&mut fixed)));
            segments.push(Segment::Target {
                digits: digits.as_bytes().to_vec(),
                covers,
            });
        }
        segments.push(Segment::Fixed(fixed));
        (segments, pairs)
    }

    /// 現行の判定で拒否され、除外の対象がある連結候補を判定する
    pub(crate) fn judge(
        &mut self,
        c: &Combined,
        tokens: &[NumericToken],
        kana_runs: &[bool],
        input: &[u8],
        stats: &mut LicenseStats,
    ) -> bool {
        let (segments, pairs) = self.segments(c, tokens, kana_runs);
        let t = Instant::now();
        let result = judge_segments(&segments, &pairs, input);
        stats.dp_time += t.elapsed();
        stats.dp_candidates += 1;
        stats.dp_states += result.states;
        stats.dp_states_max = stats.dp_states_max.max(result.states_max);
        stats.dp_transitions += result.transitions;
        if result.accepted {
            stats.dp_accepted += 1;
        }
        result.accepted
    }
}

// ─── 動的計画法 ───────────────────────────────────────────────────────────────

pub(crate) struct DpResult {
    pub accepted: bool,
    pub states: usize,
    pub states_max: usize,
    pub transitions: usize,
}

/// 直前に割り当てた組 `last` の後に、組 `m` を割り当ててよいか。
///
/// 使う組を出力の位置で並べると、非重複と順序の条件は隣り合う組どうしの条件だけで
/// 全体が満たされる。同じ組を複数の対象に割り当てるのは許す（使う組の集合は同じ）。
fn can_follow(last: Option<&Pair>, m: &Pair) -> bool {
    match last {
        None => true,
        Some(l) if l == m => true,
        Some(l) if l.run != m.run => l.oj <= m.oi,
        Some(l) => l.oj <= m.oi && l.rj <= m.ri,
    }
}

const NO_PAIR: usize = usize::MAX;

/// 区切りの列を前から処理し、入力の数字列と一致する割当てが存在するかを判定する。
///
/// 状態は `(j, last)`: `j` は入力の数字列の照合済みの長さ、`last` は直前に割り当てた
/// 組（無ければ `NO_PAIR`）。
pub(crate) fn judge_segments(segments: &[Segment], pairs: &[Pair], input: &[u8]) -> DpResult {
    let mut states: HashSet<(usize, usize)> = HashSet::from([(0, NO_PAIR)]);
    let mut total_states = 1;
    let mut states_max = 1;
    let mut transitions = 0;

    for seg in segments {
        let mut next: HashSet<(usize, usize)> = HashSet::new();
        match seg {
            Segment::Fixed(f) => {
                for &(j, last) in &states {
                    transitions += 1;
                    if input[j..].starts_with(f) {
                        next.insert((j + f.len(), last));
                    }
                }
            }
            Segment::Target { digits, covers } => {
                for &(j, last) in &states {
                    // 数える
                    transitions += 1;
                    if input[j..].starts_with(digits) {
                        next.insert((j + digits.len(), last));
                    }
                    // 覆う組を割り当てて数えない
                    let last_pair = (last != NO_PAIR).then(|| &pairs[last]);
                    for &m in covers {
                        transitions += 1;
                        if can_follow(last_pair, &pairs[m]) {
                            next.insert((j, m));
                        }
                    }
                }
            }
        }
        states = next;
        total_states += states.len();
        states_max = states_max.max(states.len());
        if states.is_empty() {
            break;
        }
    }

    DpResult {
        accepted: states.iter().any(|&(j, _)| j == input.len()),
        states: total_states,
        states_max,
        transitions,
    }
}

#[cfg(test)]
mod tests;
