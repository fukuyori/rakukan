//! rakukan-dict-builder
//!
//! mozc の dictionary_oss TSV ファイルを rakukan 独自バイナリ形式に変換する。
//!
//! # 使い方
//! ```
//! rakukan-dict-builder \
//!   --input  path/to/mozc_dict.tsv \   # 複数指定可
//!   --output %APPDATA%\rakukan\dict\rakukan.dict
//! ```
//!
//! # mozc TSV フォーマット
//! ```
//! 読み TAB 表記 TAB 品詞名 TAB lid TAB rid TAB cost
//! にほん  日本    名詞-固有名詞-地名-一般  1849  1849  3394
//! ```
//!
//! # 出力バイナリフォーマット（rakukan.dict）
//!
//! ```text
//! ┌─ Header (16 bytes)
//! │   magic[4]       = b"RKND"
//! │   version[4]     = 1u32 LE
//! │   n_entries[4]   = 全エントリ数 u32 LE
//! │   n_readings[4]  = ユニーク読み数 u32 LE
//! │
//! ├─ Index (n_readings × 12 bytes, 読み仮名の辞書順ソート済)
//! │   reading_off[4]   = reading_heap 内バイトオフセット u32 LE
//! │   reading_len[2]   = 読みバイト長 u16 LE
//! │   entries_start[4] = entries 内の開始インデックス u32 LE
//! │   n_tokens[2]      = この読みのエントリ数 u16 LE
//! │
//! ├─ Reading heap  (UTF-8 文字列の連続、ヌル終端なし)
//! │
//! ├─ Entries (n_entries × 8 bytes, 各読みごとに cost 昇順ソート済)
//! │   surface_off[4]  = surface_heap 内バイトオフセット u32 LE
//! │   surface_len[2]  = 表記バイト長 u16 LE
//! │   cost[2]         = mozc cost (小=高頻度) u16 LE
//! │
//! └─ Surface heap (UTF-8 文字列の連続、ヌル終端なし)
//! ```

use std::collections::HashMap;
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

// ─── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "rakukan-dict-builder",
    about = "mozc TSV → rakukan.dict binary converter"
)]
struct Args {
    /// Input TSV files (mozc dictionary format, multiple allowed)
    #[arg(short, long = "input", required = true)]
    inputs: Vec<PathBuf>,

    /// Input symbol TSV files (mozc symbol/symbol.tsv format, multiple allowed)
    #[arg(long = "symbol")]
    symbols: Vec<PathBuf>,

    /// Input emoji TSV files (mozc emoji/emoji_data.tsv format, multiple allowed)
    #[arg(long = "emoji")]
    emojis: Vec<PathBuf>,

    /// Output binary file
    #[arg(short, long)]
    output: PathBuf,

    /// Max candidates per reading (default: 50)
    #[arg(long, default_value = "50")]
    max_per_reading: usize,

    /// Max cost threshold (default: no limit)
    #[arg(long, default_value = "65535")]
    max_cost: u16,

    /// Cost for emoji entries (default: 6000 — 一般語より下、symbol より下で候補末尾寄り)
    #[arg(long, default_value = "6000")]
    emoji_cost: u16,
}

// ─── TSV パーサー ─────────────────────────────────────────────────────────────

/// 1エントリ
#[derive(Debug)]
struct Entry {
    reading: String,
    surface: String,
    cost: u16,
}

/// Windows 11 標準フォント + 既定 font linking で描画できない仮名ブロックを
/// 含むかを判定する。
///
/// 対象範囲 U+1AFF0..=U+1B16F は以下 4 ブロック:
/// - Kana Extended-B  (U+1AFF0–U+1AFFF)
/// - Kana Supplement  (U+1B000–U+1B0FF) — 変体仮名
/// - Kana Extended-A  (U+1B100–U+1B12F) — 変体仮名追加
/// - Small Kana Extension (U+1B130–U+1B16F)
///
/// これらの surface は候補ウィンドウで「‥」相当のフォールバック字形になるため、
/// 辞書ビルド時に恒久的に除外する。絵文字 (U+1F000+) や CJK 漢字
/// (U+4E00 / U+20000+) は範囲が重ならないので誤爆しない。
fn has_unrenderable_kana(s: &str) -> bool {
    s.chars().any(|c| {
        let n = c as u32;
        (0x1AFF0..=0x1B16F).contains(&n)
    })
}

/// mozc TSV を読み込んでエントリ列を返す
fn parse_tsv(path: &PathBuf, max_cost: u16) -> Result<Vec<Entry>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("TSV 読み込み失敗: {}", path.display()))?;

    let mut entries = Vec::new();
    let mut skipped = 0usize;
    let mut skipped_unrenderable = 0usize;

    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        // コメント・空行スキップ
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let cols: Vec<&str> = line.splitn(5, '\t').collect();
        // mozc format: reading TAB lid TAB rid TAB cost TAB surface
        if cols.len() < 5 {
            tracing::warn!(
                "{}:{} カラム不足 ({} cols): {:?}",
                path.display(),
                lineno + 1,
                cols.len(),
                line
            );
            skipped += 1;
            continue;
        }

        let reading = cols[0].to_string();
        let surface = cols[4].to_string();
        let cost_str = cols[3];
        let cost: u16 = match cost_str.parse::<u32>() {
            Ok(c) if c <= 65535 => c as u16,
            Ok(c) => {
                // cost が u16 を超える場合は上限にクランプ
                tracing::trace!("cost クランプ: {} → 65535", c);
                65535u16
            }
            Err(_) => {
                tracing::warn!(
                    "{}:{} cost パース失敗: {:?}",
                    path.display(),
                    lineno + 1,
                    cost_str
                );
                skipped += 1;
                continue;
            }
        };

        if cost > max_cost {
            skipped += 1;
            continue;
        }

        // 読みが空・表記が空のエントリを除外
        if reading.is_empty() || surface.is_empty() {
            skipped += 1;
            continue;
        }

        // 変体仮名等の描画不可文字を含む surface を除外
        if has_unrenderable_kana(&surface) {
            skipped_unrenderable += 1;
            continue;
        }

        entries.push(Entry {
            reading,
            surface,
            cost,
        });
    }

    tracing::info!(
        "{}: {} エントリ読み込み、{} スキップ (うち描画不可仮名 {})",
        path.display(),
        entries.len(),
        skipped + skipped_unrenderable,
        skipped_unrenderable
    );
    Ok(entries)
}

// ─── バイナリビルダー ─────────────────────────────────────────────────────────

/// 読みごとにまとめたグループ
struct ReadingGroup {
    reading: String,
    /// cost 昇順にソートされた (surface, cost) リスト
    tokens: Vec<(String, u16)>,
}

/// symbol.tsv パーサー
///
/// フォーマット: POS TAB CHAR TAB Readings(space-sep) TAB description ...
/// Readings フィールドのうちひらがなのみのトークンを読みとして採用する。
/// cost は固定値（symbol は優先度を高めにする）。
fn parse_symbol_tsv(path: &PathBuf) -> Result<Vec<Entry>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("symbol TSV read failed: {}", path.display()))?;

    let mut entries = Vec::new();
    let mut skipped = 0usize;
    let mut skipped_unrenderable = 0usize;

    for (_lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        // ヘッダ・コメント・空行スキップ
        if line.is_empty() || line.starts_with('#') || line.starts_with("POS\t") {
            continue;
        }

        let cols: Vec<&str> = line.splitn(4, '\t').collect();
        if cols.len() < 3 {
            skipped += 1;
            continue;
        }

        let surface = cols[1].trim().to_string();
        let readings_raw = cols[2];

        if surface.is_empty() {
            skipped += 1;
            continue;
        }

        // 変体仮名等の描画不可文字を含む surface を除外
        if has_unrenderable_kana(&surface) {
            skipped_unrenderable += 1;
            continue;
        }

        // readings フィールドはスペース区切りの複数トークン
        // ひらがな（U+3041–U+309F）と長音符（U+30FC）のみで構成されるトークンを読みとして採用
        let hira_readings: Vec<&str> = readings_raw
            .split(' ')
            .filter(|t| {
                !t.is_empty()
                    && t.chars().all(|c| {
                        let n = c as u32;
                        (0x3041..=0x309F).contains(&n) || c == 'ー'
                    })
            })
            .collect();

        if hira_readings.is_empty() {
            skipped += 1;
            continue;
        }

        // symbol エントリは cost=3000 固定（mozc 通常エントリの平均的な値）
        let cost: u16 = 3000;

        for reading in hira_readings {
            entries.push(Entry {
                reading: reading.to_string(),
                surface: surface.clone(),
                cost,
            });
        }
    }

    tracing::info!(
        "{}: {} symbol entries, {} skipped (うち描画不可仮名 {})",
        path.display(),
        entries.len(),
        skipped + skipped_unrenderable,
        skipped_unrenderable
    );
    Ok(entries)
}

/// mozc emoji_data.tsv パーサー
///
/// フォーマット (タブ区切り、7 カラム):
/// 1. unicode code point (空白区切り hex、例: "23E9 FE0F")
/// 2. 実データ (UTF-8 文字、例: "⏩️")
/// 3. 読み (空白区切り、例: "はやおくり ばいそく ぼたん")
/// 4. unicode name (空の場合あり)
/// 5. 日本語名 (例: "早送り")
/// 6. 説明語 (空白区切り)
/// 7. emoji version (例: "E0.6")
///
/// 読み (カラム 3) のうち「ひらがな + 長音符」のみで構成されるトークンを reading として採用。
/// surface は カラム 2 をそのまま使う。
/// 変体仮名は `has_unrenderable_kana` で surface 単位で除外するが、emoji は U+1F000 以上
/// もしくは BMP 内の Misc Technical 系で、そもそもフィルタ範囲と重ならない。
fn parse_emoji_tsv(path: &PathBuf, cost: u16) -> Result<Vec<Entry>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("emoji TSV read failed: {}", path.display()))?;

    let mut entries = Vec::new();
    let mut skipped = 0usize;
    let mut skipped_no_reading = 0usize;

    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            skipped += 1;
            continue;
        }

        let surface = cols[1].trim().to_string();
        let readings_raw = cols[2];

        if surface.is_empty() {
            skipped += 1;
            continue;
        }

        // surface に変体仮名等が混ざっていないか念のため確認
        if has_unrenderable_kana(&surface) {
            skipped += 1;
            continue;
        }

        // readings: 空白区切り、ひらがな (U+3041–U+309F) + 長音符 (U+30FC) のみのトークンを採用
        let hira_readings: Vec<&str> = readings_raw
            .split(' ')
            .filter(|t| {
                !t.is_empty()
                    && t.chars().all(|c| {
                        let n = c as u32;
                        (0x3041..=0x309F).contains(&n) || c == 'ー'
                    })
            })
            .collect();

        if hira_readings.is_empty() {
            skipped_no_reading += 1;
            continue;
        }

        for reading in hira_readings {
            entries.push(Entry {
                reading: reading.to_string(),
                surface: surface.clone(),
                cost,
            });
        }
    }

    tracing::info!(
        "{}: {} emoji entries, {} skipped (読み無し {})",
        path.display(),
        entries.len(),
        skipped + skipped_no_reading,
        skipped_no_reading
    );
    Ok(entries)
}

fn build_groups(entries: Vec<Entry>, max_per_reading: usize) -> Vec<ReadingGroup> {
    // 読み → Vec<(surface, cost)>
    let mut map: HashMap<String, Vec<(String, u16)>> = HashMap::new();
    for e in entries {
        map.entry(e.reading).or_default().push((e.surface, e.cost));
    }

    let mut groups: Vec<ReadingGroup> = map
        .into_iter()
        .map(|(reading, mut tokens)| {
            // cost 昇順ソート（同コストは surface 昇順で安定化）
            tokens.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
            // 重複表記除去（コスト最小を残す）
            tokens.dedup_by(|a, b| a.0 == b.0);
            // 上限カット
            tokens.truncate(max_per_reading);
            ReadingGroup { reading, tokens }
        })
        .collect();

    // 読みを辞書順ソート（二分探索のため必須）
    groups.sort_by(|a, b| a.reading.cmp(&b.reading));

    groups
}

// ─── バイナリ書き出し ─────────────────────────────────────────────────────────

const MAGIC: &[u8; 4] = b"RKND";
const VERSION: u32 = 1;

fn write_dict(groups: &[ReadingGroup], output: &PathBuf) -> Result<()> {
    // ── ヒープ構築 ──────────────────────────────────────────────────────────
    let mut reading_heap: Vec<u8> = Vec::new();
    let mut surface_heap: Vec<u8> = Vec::new();

    // Index エントリ（後でバイナリに書く）
    struct IndexEntry {
        reading_off: u32,
        reading_len: u16,
        entries_start: u32,
        n_tokens: u16,
    }

    struct EntryRecord {
        surface_off: u32,
        surface_len: u16,
        cost: u16,
    }

    let mut index_entries: Vec<IndexEntry> = Vec::with_capacity(groups.len());
    let mut entry_records: Vec<EntryRecord> = Vec::new();

    let mut entries_cursor: u32 = 0;

    for group in groups {
        let reading_off = reading_heap.len() as u32;
        let reading_bytes = group.reading.as_bytes();
        reading_heap.extend_from_slice(reading_bytes);

        let n_tokens = group.tokens.len() as u16;

        for (surface, cost) in &group.tokens {
            let surface_off = surface_heap.len() as u32;
            let surface_bytes = surface.as_bytes();
            surface_heap.extend_from_slice(surface_bytes);

            entry_records.push(EntryRecord {
                surface_off,
                surface_len: surface_bytes.len() as u16,
                cost: *cost,
            });
        }

        index_entries.push(IndexEntry {
            reading_off,
            reading_len: reading_bytes.len() as u16,
            entries_start: entries_cursor,
            n_tokens,
        });

        entries_cursor += n_tokens as u32;
    }

    let n_readings = groups.len() as u32;
    let n_entries = entry_records.len() as u32;

    tracing::info!(
        "書き込み: {} 読み、{} エントリ、reading_heap={} bytes、surface_heap={} bytes",
        n_readings,
        n_entries,
        reading_heap.len(),
        surface_heap.len()
    );

    // ── ファイル書き込み ─────────────────────────────────────────────────────
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("ディレクトリ作成失敗: {}", parent.display()))?;
    }

    let file = std::fs::File::create(output)
        .with_context(|| format!("ファイル作成失敗: {}", output.display()))?;
    let mut w = BufWriter::new(file);

    // Header (16 bytes)
    w.write_all(MAGIC)?;
    w.write_all(&VERSION.to_le_bytes())?;
    w.write_all(&n_entries.to_le_bytes())?;
    w.write_all(&n_readings.to_le_bytes())?;

    // Index (n_readings × 12 bytes)
    for ie in &index_entries {
        w.write_all(&ie.reading_off.to_le_bytes())?;
        w.write_all(&ie.reading_len.to_le_bytes())?;
        w.write_all(&ie.entries_start.to_le_bytes())?;
        w.write_all(&ie.n_tokens.to_le_bytes())?;
    }

    // Reading heap
    w.write_all(&reading_heap)?;

    // Entries (n_entries × 8 bytes)
    for er in &entry_records {
        w.write_all(&er.surface_off.to_le_bytes())?;
        w.write_all(&er.surface_len.to_le_bytes())?;
        w.write_all(&er.cost.to_le_bytes())?;
    }

    // Surface heap
    w.write_all(&surface_heap)?;

    w.flush()?;
    let file_size = output.metadata().map(|m| m.len()).unwrap_or(0);
    tracing::info!("出力: {} ({} bytes)", output.display(), file_size);
    Ok(())
}

// ─── main ─────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt().with_env_filter("info").init();

    // 全入力 TSV を読み込んでマージ
    let mut all_entries: Vec<Entry> = Vec::new();
    for path in &args.inputs {
        let entries = parse_tsv(path, args.max_cost)
            .with_context(|| format!("TSV パース失敗: {}", path.display()))?;
        all_entries.extend(entries);
    }

    // symbol.tsv を読み込んでマージ
    for path in &args.symbols {
        let entries = parse_symbol_tsv(path)
            .with_context(|| format!("symbol TSV パース失敗: {}", path.display()))?;
        all_entries.extend(entries);
    }

    // emoji_data.tsv を読み込んでマージ
    for path in &args.emojis {
        let entries = parse_emoji_tsv(path, args.emoji_cost)
            .with_context(|| format!("emoji TSV パース失敗: {}", path.display()))?;
        all_entries.extend(entries);
    }

    tracing::info!("合計 {} エントリ", all_entries.len());

    // 読みごとにグループ化・ソート
    let groups = build_groups(all_entries, args.max_per_reading);
    tracing::info!("ユニーク読み数: {}", groups.len());

    // バイナリ書き出し
    write_dict(&groups, &args.output)?;

    println!("完了: {} 読み → {}", groups.len(), args.output.display());
    Ok(())
}

// ─── テスト ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tsv(lines: &[&str]) -> String {
        lines.join("\n")
    }

    #[test]
    fn test_parse_basic() {
        // tmpファイルに書き込んでパースする
        let content = make_tsv(&[
            "にほん\t日本\t名詞\t1849\t1849\t3394",
            "にほん\t二本\t名詞\t1234\t1234\t7800",
            "にほんご\t日本語\t名詞\t1849\t1849\t4000",
        ]);
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), &content).unwrap();
        let entries = parse_tsv(&tmp.path().to_path_buf(), 65535).unwrap();
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn test_has_unrenderable_kana() {
        // ── 描画不可（フィルタ対象）──
        assert!(has_unrenderable_kana("\u{1B0B3}"));       // HENTAIGANA LETTER HU-1
        assert!(has_unrenderable_kana("\u{1B0B5}"));       // HENTAIGANA LETTER HU-3
        assert!(has_unrenderable_kana("\u{1B000}"));       // Kana Supplement 先頭
        assert!(has_unrenderable_kana("\u{1B16F}"));       // Small Kana Extension 末尾
        assert!(has_unrenderable_kana("\u{1AFF0}"));       // Kana Extended-B 先頭
        assert!(has_unrenderable_kana("ふ\u{1B0B3}る"));   // 混在していても hit

        // ── 描画可（フィルタ対象外、絵文字・漢字・通常仮名）──
        assert!(!has_unrenderable_kana("日本"));           // CJK 基本
        assert!(!has_unrenderable_kana("にほん"));          // ひらがな
        assert!(!has_unrenderable_kana("ニホン"));          // カタカナ
        assert!(!has_unrenderable_kana("\u{23E9}"));       // ⏩ Misc Technical
        assert!(!has_unrenderable_kana("\u{1F389}"));      // 🎉 絵文字
        assert!(!has_unrenderable_kana("\u{1F680}"));      // 🚀 絵文字
        assert!(!has_unrenderable_kana("\u{20000}"));      // CJK Ext B 先頭（保持）
        assert!(!has_unrenderable_kana("\u{1AFEF}"));      // フィルタ範囲の 1 つ下
        assert!(!has_unrenderable_kana("\u{1B170}"));      // フィルタ範囲の 1 つ上
    }

    #[test]
    fn test_parse_emoji_tsv() {
        // mozc emoji_data.tsv 形式（7 カラム、タブ区切り）
        // header コメント行 + データ行 3 つ
        let content = [
            "# This is a comment",
            "# The data format is tab separated fields",
            // ⏩: はやおくり / ばいそく / ぼたん などが hiragana-only → 有効
            "23E9 FE0F\t\u{23E9}\u{FE0F}\tはやおくり ばいそく ぼたん\t\t早送り\tボタン 倍速\tE0.6",
            // 1️⃣: 数字 "1" や "１" は filter で落ちる、"いち" は採用
            "31 FE0F 20E3\t1\u{FE0F}\u{20E3}\t1 いち\t\t絵文字\t1 一\tE0.6",
            // 読み無し（ASCII のみ）→ skip
            "30\t0\t0\t\t絵文字\t\tE0.6",
        ]
        .join("\n");
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), &content).unwrap();
        let entries = parse_emoji_tsv(&tmp.path().to_path_buf(), 6000).unwrap();

        // ⏩ → 3 readings (はやおくり, ばいそく, ぼたん)
        // 1️⃣ → 1 reading (いち)
        // 3 行目 → 0 readings、skip
        assert_eq!(entries.len(), 4);

        // surface にすべて filter に引っかかる文字が含まれていないこと
        for e in &entries {
            assert!(!has_unrenderable_kana(&e.surface));
        }

        // cost は引数値
        assert!(entries.iter().all(|e| e.cost == 6000));

        // ⏩ が hiragana 読みで引けること
        let hayaokuri: Vec<&Entry> = entries.iter().filter(|e| e.reading == "はやおくり").collect();
        assert_eq!(hayaokuri.len(), 1);
        assert_eq!(hayaokuri[0].surface, "\u{23E9}\u{FE0F}");

        let itchi: Vec<&Entry> = entries.iter().filter(|e| e.reading == "いち").collect();
        assert_eq!(itchi.len(), 1);
    }

    #[test]
    fn test_parse_filters_unrenderable_kana() {
        // mozc TSV 形式: reading TAB lid TAB rid TAB cost TAB surface
        // 通常エントリ 2 + 変体仮名 surface 1 → 2 件のみ残る
        let content = make_tsv(&[
            "にほん\t1849\t1849\t3394\t日本",
            "ふ\t1234\t1234\t5000\t\u{1B0B3}",       // surface = HENTAIGANA HU-1
            "にほんご\t1849\t1849\t4000\t日本語",
        ]);
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), &content).unwrap();
        let entries = parse_tsv(&tmp.path().to_path_buf(), 65535).unwrap();
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| !has_unrenderable_kana(&e.surface)));
        // 絵文字・通常漢字は残る確認
        let content2 = make_tsv(&[
            "はやおくり\t1849\t1849\t3000\t\u{23E9}",  // ⏩
            "にほん\t1849\t1849\t3394\t日本",
        ]);
        std::fs::write(tmp.path(), &content2).unwrap();
        let entries2 = parse_tsv(&tmp.path().to_path_buf(), 65535).unwrap();
        assert_eq!(entries2.len(), 2);
    }

    #[test]
    fn test_group_cost_sort() {
        let entries = vec![
            Entry {
                reading: "にほん".into(),
                surface: "二本".into(),
                cost: 7800,
            },
            Entry {
                reading: "にほん".into(),
                surface: "日本".into(),
                cost: 3394,
            },
        ];
        let groups = build_groups(entries, 50);
        assert_eq!(groups[0].tokens[0].0, "日本"); // cost 3394 が先頭
        assert_eq!(groups[0].tokens[1].0, "二本");
    }

    #[test]
    fn test_group_dedup() {
        let entries = vec![
            Entry {
                reading: "tes".into(),
                surface: "X".into(),
                cost: 100,
            },
            Entry {
                reading: "tes".into(),
                surface: "X".into(),
                cost: 200,
            }, // 重複
        ];
        let groups = build_groups(entries, 50);
        assert_eq!(groups[0].tokens.len(), 1); // 重複除去
    }

    #[test]
    fn test_roundtrip() {
        let entries = vec![
            Entry {
                reading: "にほん".into(),
                surface: "日本".into(),
                cost: 3394,
            },
            Entry {
                reading: "にほん".into(),
                surface: "二本".into(),
                cost: 7800,
            },
            Entry {
                reading: "にほんご".into(),
                surface: "日本語".into(),
                cost: 4000,
            },
        ];
        let groups = build_groups(entries, 50);
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        write_dict(&groups, &path).unwrap();
        let data = std::fs::read(&path).unwrap();
        // magic 確認
        assert_eq!(&data[0..4], b"RKND");
        // version = 1
        let ver = u32::from_le_bytes(data[4..8].try_into().unwrap());
        assert_eq!(ver, 1);
        // n_entries
        let n_entries = u32::from_le_bytes(data[8..12].try_into().unwrap());
        assert_eq!(n_entries, 3);
        // n_readings
        let n_readings = u32::from_le_bytes(data[12..16].try_into().unwrap());
        assert_eq!(n_readings, 2); // "にほん" と "にほんご"
    }
}
