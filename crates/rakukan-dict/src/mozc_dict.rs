//! rakukan バイナリ辞書（mozc TSV からビルド）の読み取りモジュール
//!
//! # フォーマット（rakukan.dict）
//!
//! ```text
//! Header  (16 bytes)  magic="RKND", version=1, n_entries, n_readings
//! Index   (n_readings × 12 bytes)  reading_off:u32, reading_len:u16, entries_start:u32, n_tokens:u16
//! Reading heap  (UTF-8)
//! Entries (n_entries × 8 bytes)    surface_off:u32, surface_len:u16, cost:u16
//! Surface heap  (UTF-8)
//! ```
//!
//! # ルックアップ
//! - Index は読み仮名の辞書順ソート済 → **二分探索** O(log N)
//! - ヒット後は entries[entries_start..entries_start+n_tokens] を読む
//! - cost 昇順（ビルド時ソート済）で返す

use std::path::Path;

use anyhow::{bail, Context, Result};
use memmap2::Mmap;

// ─── フォーマット定数 ─────────────────────────────────────────────────────────

const MAGIC:   &[u8; 4] = b"RKND";
const VERSION: u32      = 1;

const HEADER_SIZE:       usize = 16;
const INDEX_ENTRY_SIZE:  usize = 12;
const ENTRY_RECORD_SIZE: usize = 8;

// ─── MozcDict ────────────────────────────────────────────────────────────────

/// メモリマップ済みバイナリ辞書
///
/// `MozcDict` は `Mmap` を内部に持ち、辞書ファイルをゼロコピーで参照する。
/// 複数スレッドからの読み取りは安全（`Mmap` は `Sync`）。
pub struct MozcDict {
    mmap:       Mmap,
    n_entries:  u32,
    n_readings: u32,
    // 各セクションの開始オフセット（mmap 内）
    index_off:   usize,
    reading_heap_off: usize,
    entries_off: usize,
    surface_heap_off: usize,
}

impl MozcDict {
    /// ファイルを開いて mmap し、ヘッダーを検証する
    pub fn open(path: &Path) -> Result<Self> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("MozcDict 開けない: {}", path.display()))?;

        let mmap = unsafe { Mmap::map(&file) }
            .with_context(|| format!("mmap 失敗: {}", path.display()))?;

        Self::from_mmap(mmap)
    }

    fn from_mmap(mmap: Mmap) -> Result<Self> {
        if mmap.len() < HEADER_SIZE {
            bail!("rakukan.dict: ファイルが小さすぎる ({}B)", mmap.len());
        }

        // magic
        if &mmap[0..4] != MAGIC {
            bail!("rakukan.dict: マジック不一致 (expected RKND)");
        }
        // version
        let version = u32_le(&mmap, 4);
        if version != VERSION {
            bail!("rakukan.dict: バージョン不一致 (got {version}, expected {VERSION})");
        }

        let n_entries  = u32_le(&mmap, 8);
        let n_readings = u32_le(&mmap, 12);

        // セクションオフセット計算
        let index_off        = HEADER_SIZE;
        let reading_heap_off = index_off + n_readings as usize * INDEX_ENTRY_SIZE;
        let entries_off      = reading_heap_off + reading_heap_size(&mmap, n_readings, index_off)?;
        let surface_heap_off = entries_off + n_entries as usize * ENTRY_RECORD_SIZE;

        if surface_heap_off > mmap.len() {
            bail!("rakukan.dict: ファイルサイズ不足 ({}B, expected >= {}B)",
                mmap.len(), surface_heap_off);
        }

        tracing::debug!(
            "MozcDict opened: n_readings={n_readings}, n_entries={n_entries}, \
             reading_heap_off={reading_heap_off}, entries_off={entries_off}, \
             surface_heap_off={surface_heap_off}"
        );

        Ok(Self {
            mmap,
            n_entries,
            n_readings,
            index_off,
            reading_heap_off,
            entries_off,
            surface_heap_off,
        })
    }

    /// 読みで候補を検索する（cost 昇順、最大 `limit` 件）
    pub fn lookup(&self, reading: &str, limit: usize) -> Vec<(String, u16)> {
        let Some(idx) = self.binary_search(reading) else {
            return vec![];
        };

        let (entries_start, n_tokens) = self.index_entry(idx);
        let count = (n_tokens as usize).min(limit);

        let mut results = Vec::with_capacity(count);
        for i in 0..count {
            let record_off = self.entries_off
                + (entries_start as usize + i) * ENTRY_RECORD_SIZE;
            let surface_off = u32_le(&self.mmap, record_off) as usize;
            let surface_len = u16_le(&self.mmap, record_off + 4) as usize;
            let cost        = u16_le(&self.mmap, record_off + 6);

            let surface_start = self.surface_heap_off + surface_off;
            let surface_end   = surface_start + surface_len;
            if surface_end > self.mmap.len() {
                tracing::warn!("surface 範囲外: reading={reading:?}");
                break;
            }
            let surface = std::str::from_utf8(&self.mmap[surface_start..surface_end])
                .unwrap_or("")
                .to_string();
            results.push((surface, cost));
        }

        results
    }

    /// ユニーク読み数
    pub fn n_readings(&self) -> usize { self.n_readings as usize }

    /// 全エントリ数
    pub fn n_entries(&self) -> usize { self.n_entries as usize }

    // ─ 内部ヘルパー ──────────────────────────────────────────────────────────

    /// 読みを二分探索して index 内の位置を返す
    fn binary_search(&self, reading: &str) -> Option<usize> {
        let mut lo = 0usize;
        let mut hi = self.n_readings as usize;
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let r   = self.reading_at(mid);
            match r.as_deref().cmp(&Some(reading)) {
                std::cmp::Ordering::Equal   => return Some(mid),
                std::cmp::Ordering::Less    => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
            }
        }
        None
    }

    /// index[i] の reading 文字列を取得
    fn reading_at(&self, i: usize) -> Option<&str> {
        let base = self.index_off + i * INDEX_ENTRY_SIZE;
        let reading_off = u32_le(&self.mmap, base) as usize;
        let reading_len = u16_le(&self.mmap, base + 4) as usize;
        let start = self.reading_heap_off + reading_off;
        let end   = start + reading_len;
        if end > self.entries_off {
            return None;
        }
        std::str::from_utf8(&self.mmap[start..end]).ok()
    }

    /// index[i] の (entries_start, n_tokens)
    fn index_entry(&self, i: usize) -> (u32, u16) {
        let base = self.index_off + i * INDEX_ENTRY_SIZE;
        let entries_start = u32_le(&self.mmap, base + 6);
        let n_tokens      = u16_le(&self.mmap, base + 10);
        (entries_start, n_tokens)
    }
}

// ─── reading_heap のサイズ計算 ─────────────────────────────────────────────────
//
// reading_heap は index の直後に続く。
// その終端 = 最後の index エントリの reading_off + reading_len。

fn reading_heap_size(mmap: &Mmap, n_readings: u32, index_off: usize) -> Result<usize> {
    if n_readings == 0 {
        return Ok(0);
    }
    let mut max_end = 0usize;
    for i in 0..n_readings as usize {
        let base = index_off + i * INDEX_ENTRY_SIZE;
        if base + 8 > mmap.len() {
            bail!("index 範囲外");
        }
        let off = u32_le(mmap, base) as usize;
        let len = u16_le(mmap, base + 4) as usize;
        max_end = max_end.max(off + len);
    }
    Ok(max_end)
}

// ─── バイト読み取りユーティリティ ────────────────────────────────────────────

#[inline(always)]
fn u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}

#[inline(always)]
fn u16_le(buf: &[u8], off: usize) -> u16 {
    u16::from_le_bytes(buf[off..off + 2].try_into().unwrap())
}

// ─── テスト ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// テスト用のミニ rakukan.dict をメモリ上で作る
    fn build_test_dict(entries: &[(&str, &str, u16)]) -> Vec<u8> {
        // entries: [(reading, surface, cost)]
        use std::collections::BTreeMap;

        // 読みでグループ化（BTreeMap = ソート済）
        let mut map: BTreeMap<&str, Vec<(&str, u16)>> = BTreeMap::new();
        for (r, s, c) in entries {
            map.entry(r).or_default().push((s, *c));
        }
        for v in map.values_mut() {
            v.sort_by_key(|x| x.1);
        }

        let n_readings = map.len() as u32;
        let n_entries  = entries.len() as u32;

        let mut reading_heap: Vec<u8> = Vec::new();
        let mut surface_heap: Vec<u8> = Vec::new();

        struct IdxEntry { reading_off: u32, reading_len: u16, entries_start: u32, n_tokens: u16 }
        struct EntRecord { surface_off: u32, surface_len: u16, cost: u16 }

        let mut idx_entries: Vec<IdxEntry> = Vec::new();
        let mut ent_records: Vec<EntRecord> = Vec::new();
        let mut entries_cursor = 0u32;

        for (reading, tokens) in &map {
            let reading_off = reading_heap.len() as u32;
            reading_heap.extend_from_slice(reading.as_bytes());
            for (surface, cost) in tokens {
                let surface_off = surface_heap.len() as u32;
                surface_heap.extend_from_slice(surface.as_bytes());
                ent_records.push(EntRecord {
                    surface_off,
                    surface_len: surface.len() as u16,
                    cost: *cost,
                });
            }
            idx_entries.push(IdxEntry {
                reading_off,
                reading_len: reading.len() as u16,
                entries_start: entries_cursor,
                n_tokens: tokens.len() as u16,
            });
            entries_cursor += tokens.len() as u32;
        }

        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(b"RKND");
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&n_entries.to_le_bytes());
        buf.extend_from_slice(&n_readings.to_le_bytes());
        for ie in &idx_entries {
            buf.extend_from_slice(&ie.reading_off.to_le_bytes());
            buf.extend_from_slice(&ie.reading_len.to_le_bytes());
            buf.extend_from_slice(&ie.entries_start.to_le_bytes());
            buf.extend_from_slice(&ie.n_tokens.to_le_bytes());
        }
        buf.extend_from_slice(&reading_heap);
        for er in &ent_records {
            buf.extend_from_slice(&er.surface_off.to_le_bytes());
            buf.extend_from_slice(&er.surface_len.to_le_bytes());
            buf.extend_from_slice(&er.cost.to_le_bytes());
        }
        buf.extend_from_slice(&surface_heap);
        buf
    }

    fn open_test(data: &[u8]) -> MozcDict {
        // tmpファイルに書いて open
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(data).unwrap();
        f.flush().unwrap();
        MozcDict::open(f.path()).unwrap()
    }

    #[test]
    fn test_lookup_basic() {
        let data = build_test_dict(&[
            ("にほん",   "日本",   3394),
            ("にほん",   "二本",   7800),
            ("にほんご", "日本語", 4000),
        ]);
        let dict = open_test(&data);
        let results = dict.lookup("にほん", 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "日本");    // cost 3394
        assert_eq!(results[1].0, "二本");    // cost 7800
    }

    #[test]
    fn test_lookup_limit() {
        let data = build_test_dict(&[
            ("tes", "A", 100),
            ("tes", "B", 200),
            ("tes", "C", 300),
        ]);
        let dict = open_test(&data);
        let results = dict.lookup("tes", 2);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_lookup_miss() {
        let data = build_test_dict(&[("abc", "X", 100)]);
        let dict = open_test(&data);
        assert!(dict.lookup("xyz", 10).is_empty());
    }

    #[test]
    fn test_stats() {
        let data = build_test_dict(&[
            ("a", "X", 1),
            ("b", "Y", 2),
            ("b", "Z", 3),
        ]);
        let dict = open_test(&data);
        assert_eq!(dict.n_readings(), 2);
        assert_eq!(dict.n_entries(), 3);
    }
}
