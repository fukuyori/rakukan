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
    about = "mozc TSV → rakukan.dict バイナリ変換ツール"
)]
struct Args {
    /// 入力 TSV ファイル（複数指定可）
    #[arg(short, long = "input", required = true)]
    inputs: Vec<PathBuf>,

    /// 出力バイナリファイル
    #[arg(short, long)]
    output: PathBuf,

    /// 1 読みあたりの最大候補数（デフォルト: 50）
    #[arg(long, default_value = "50")]
    max_per_reading: usize,

    /// cost 上限（これより大きいエントリを除外。デフォルト: 無制限）
    #[arg(long, default_value = "65535")]
    max_cost: u16,
}

// ─── TSV パーサー ─────────────────────────────────────────────────────────────

/// 1エントリ
#[derive(Debug)]
struct Entry {
    reading: String,
    surface: String,
    cost:    u16,
}

/// mozc TSV を読み込んでエントリ列を返す
fn parse_tsv(path: &PathBuf, max_cost: u16) -> Result<Vec<Entry>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("TSV 読み込み失敗: {}", path.display()))?;

    let mut entries = Vec::new();
    let mut skipped = 0usize;

    for (lineno, line) in text.lines().enumerate() {
        let line = line.trim();
        // コメント・空行スキップ
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let cols: Vec<&str> = line.splitn(5, '\t').collect();
        // mozc format: reading TAB lid TAB rid TAB cost TAB surface
        if cols.len() < 5 {
            tracing::warn!("{}:{} カラム不足 ({} cols): {:?}", path.display(), lineno + 1, cols.len(), line);
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
                tracing::warn!("{}:{} cost パース失敗: {:?}", path.display(), lineno + 1, cost_str);
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

        entries.push(Entry { reading, surface, cost });
    }

    tracing::info!(
        "{}: {} エントリ読み込み、{} スキップ",
        path.display(), entries.len(), skipped
    );
    Ok(entries)
}

// ─── バイナリビルダー ─────────────────────────────────────────────────────────

/// 読みごとにまとめたグループ
struct ReadingGroup {
    reading: String,
    /// cost 昇順にソートされた (surface, cost) リスト
    tokens:  Vec<(String, u16)>,
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
const VERSION: u32     = 1;

fn write_dict(groups: &[ReadingGroup], output: &PathBuf) -> Result<()> {
    // ── ヒープ構築 ──────────────────────────────────────────────────────────
    let mut reading_heap: Vec<u8> = Vec::new();
    let mut surface_heap: Vec<u8> = Vec::new();

    // Index エントリ（後でバイナリに書く）
    struct IndexEntry {
        reading_off:   u32,
        reading_len:   u16,
        entries_start: u32,
        n_tokens:      u16,
    }

    struct EntryRecord {
        surface_off: u32,
        surface_len: u16,
        cost:        u16,
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
    let n_entries  = entry_records.len() as u32;

    tracing::info!(
        "書き込み: {} 読み、{} エントリ、reading_heap={} bytes、surface_heap={} bytes",
        n_readings, n_entries, reading_heap.len(), surface_heap.len()
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

    tracing_subscriber::fmt()
        .with_env_filter("info")
        .init();

    // 全入力 TSV を読み込んでマージ
    let mut all_entries: Vec<Entry> = Vec::new();
    for path in &args.inputs {
        let entries = parse_tsv(path, args.max_cost)
            .with_context(|| format!("TSV パース失敗: {}", path.display()))?;
        all_entries.extend(entries);
    }

    tracing::info!("合計 {} エントリ", all_entries.len());

    // 読みごとにグループ化・ソート
    let groups = build_groups(all_entries, args.max_per_reading);
    tracing::info!("ユニーク読み数: {}", groups.len());

    // バイナリ書き出し
    write_dict(&groups, &args.output)?;

    println!(
        "完了: {} 読み → {}",
        groups.len(),
        args.output.display()
    );
    Ok(())
}

// ─── テスト ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

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
    fn test_group_cost_sort() {
        let entries = vec![
            Entry { reading: "にほん".into(), surface: "二本".into(), cost: 7800 },
            Entry { reading: "にほん".into(), surface: "日本".into(), cost: 3394 },
        ];
        let groups = build_groups(entries, 50);
        assert_eq!(groups[0].tokens[0].0, "日本");   // cost 3394 が先頭
        assert_eq!(groups[0].tokens[1].0, "二本");
    }

    #[test]
    fn test_group_dedup() {
        let entries = vec![
            Entry { reading: "tes".into(), surface: "X".into(), cost: 100 },
            Entry { reading: "tes".into(), surface: "X".into(), cost: 200 }, // 重複
        ];
        let groups = build_groups(entries, 50);
        assert_eq!(groups[0].tokens.len(), 1); // 重複除去
    }

    #[test]
    fn test_roundtrip() {
        let entries = vec![
            Entry { reading: "にほん".into(),   surface: "日本".into(),   cost: 3394 },
            Entry { reading: "にほん".into(),   surface: "二本".into(),   cost: 7800 },
            Entry { reading: "にほんご".into(), surface: "日本語".into(), cost: 4000 },
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
