//! DictStore — 辞書統合ルックアップ
//!
//! 優先順位: ユーザー辞書 > mozc バイナリ辞書(cost昇順) > SKK辞書

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

use crate::skk;
use crate::user_dict::UserDict;
use crate::mozc_dict::MozcDict;

#[derive(Debug, Clone)]
pub struct DictResult {
    pub candidates: Vec<String>,
    pub source: DictSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictSource {
    User,
    Mozc,
    Skk,
    Merged,
    None,
}

struct DictStoreInner {
    user:       HashMap<String, Vec<String>>,
    mozc:       Option<MozcDict>,
    skk:        HashMap<String, Vec<String>>,
    skk_loaded: bool,
}

unsafe impl Send for DictStoreInner {}
unsafe impl Sync for DictStoreInner {}

#[derive(Clone)]
pub struct DictStore {
    inner: Arc<DictStoreInner>,
}

impl DictStore {
    /// 各辞書を読み込んで DictStore を構築する
    pub fn load(
        user_path: Option<&Path>,
        mozc_path: Option<&Path>,
        skk_paths: &[&Path],
    ) -> Result<Self> {
        let user = if let Some(p) = user_path {
            UserDict::load(p)?.to_map()
        } else {
            HashMap::new()
        };

        let mozc = if let Some(p) = mozc_path {
            if p.exists() {
                match MozcDict::open(p) {
                    Ok(d) => {
                        info!("MozcDict ロード: {} 読み, {} エントリ", d.n_readings(), d.n_entries());
                        Some(d)
                    }
                    Err(e) => {
                        warn!("MozcDict ロード失敗 {}: {}", p.display(), e);
                        None
                    }
                }
            } else {
                info!("rakukan.dict が見つからないため SKK のみ使用: {}", p.display());
                None
            }
        } else {
            None
        };

        let mut skk_map: HashMap<String, Vec<String>> = HashMap::new();
        let mut skk_loaded = false;
        for path in skk_paths {
            match load_skk_file(path) {
                Ok(dict) => {
                    for (k, mut v) in dict {
                        skk_map.entry(k).or_default().append(&mut v);
                    }
                    skk_loaded = true;
                }
                Err(e) => warn!("SKK辞書読み込み失敗 {}: {}", path.display(), e),
            }
        }

        info!(
            "DictStore: user={} entries, mozc={}, SKK={} entries",
            user.len(),
            if mozc.is_some() { "有効" } else { "なし" },
            skk_map.len()
        );

        Ok(Self {
            inner: Arc::new(DictStoreInner { user, mozc, skk: skk_map, skk_loaded }),
        })
    }

    pub fn empty() -> Self {
        Self {
            inner: Arc::new(DictStoreInner {
                user: HashMap::new(),
                mozc: None,
                skk: HashMap::new(),
                skk_loaded: false,
            }),
        }
    }

    /// ひらがな読みから候補リストを引く（優先順位: user > mozc > skk）
    pub fn lookup(&self, reading: &str, limit: usize) -> DictResult {
        let user_cands = self.inner.user.get(reading);

        let mozc_cands: Vec<String> = self.inner.mozc
            .as_ref()
            .map(|d| d.lookup(reading, limit).into_iter().map(|(s, _)| s).collect())
            .unwrap_or_default();

        let skk_cands = self.inner.skk.get(reading);

        let has_user = user_cands.is_some();
        let has_mozc = !mozc_cands.is_empty();
        let has_skk  = skk_cands.is_some();

        if !has_user && !has_mozc && !has_skk {
            return DictResult { candidates: vec![], source: DictSource::None };
        }

        let mut merged: Vec<String> = Vec::new();

        if let Some(u) = user_cands {
            for s in u {
                if !merged.contains(s) { merged.push(s.clone()); }
            }
        }

        for s in &mozc_cands {
            if merged.len() >= limit { break; }
            if !merged.contains(s) { merged.push(s.clone()); }
        }

        if merged.len() < limit {
            if let Some(skk) = skk_cands {
                for s in skk {
                    if merged.len() >= limit { break; }
                    if !merged.contains(s) { merged.push(s.clone()); }
                }
            }
        }

        merged.truncate(limit);

        let source = match (has_user, has_mozc, has_skk) {
            (true, false, false) => DictSource::User,
            (false, true, false) => DictSource::Mozc,
            (false, false, true) => DictSource::Skk,
            _                    => DictSource::Merged,
        };

        DictResult { candidates: merged, source }
    }

    pub fn is_mozc_loaded(&self) -> bool { self.inner.mozc.is_some() }
    pub fn is_skk_loaded(&self) -> bool  { self.inner.skk_loaded }
    pub fn user_entry_count(&self) -> usize { self.inner.user.len() }
    pub fn skk_entry_count(&self) -> usize  { self.inner.skk.len() }
}

fn load_skk_file(path: &Path) -> Result<skk::SkkDict> {
    let bytes = std::fs::read(path)?;
    let text = if std::str::from_utf8(&bytes).is_ok() {
        String::from_utf8(bytes).unwrap()
    } else {
        skk::decode_eucjp(&bytes)
    };
    let dict = skk::parse(text.as_bytes())?;
    info!("SKK dict loaded: {} ({} entries)", path.display(), dict.len());
    Ok(dict)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_skk_store(user: &[(&str, &str)], skk: &[(&str, &str)]) -> DictStore {
        let user_map: HashMap<String, Vec<String>> = user.iter()
            .map(|(r, s)| (r.to_string(), vec![s.to_string()]))
            .collect();
        let skk_map: HashMap<String, Vec<String>> = skk.iter()
            .map(|(r, s)| (r.to_string(), vec![s.to_string()]))
            .collect();
        DictStore {
            inner: Arc::new(DictStoreInner {
                user: user_map, mozc: None,
                skk: skk_map, skk_loaded: !skk.is_empty(),
            }),
        }
    }

    #[test]
    fn test_user_priority() {
        let store = make_skk_store(&[("きむら", "金村")], &[("きむら", "木村")]);
        let r = store.lookup("きむら", 10);
        assert_eq!(r.candidates[0], "金村");
        assert_eq!(r.source, DictSource::Merged);
    }

    #[test]
    fn test_skk_only() {
        let store = make_skk_store(&[], &[("にほんご", "日本語")]);
        let r = store.lookup("にほんご", 10);
        assert_eq!(r.candidates, vec!["日本語"]);
        assert_eq!(r.source, DictSource::Skk);
    }

    #[test]
    fn test_no_hit() {
        let store = make_skk_store(&[], &[]);
        let r = store.lookup("zzz", 10);
        assert!(r.candidates.is_empty());
        assert_eq!(r.source, DictSource::None);
    }
}
