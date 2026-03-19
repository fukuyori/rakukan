//! DictStore — 辞書統合ルックアップ
//!
//! 優先順位: ユーザー辞書 > LLM候補 > mozc バイナリ辞書 > SKK辞書
//!
//! # スレッド安全性
//! `user` は `RwLock<HashMap>` で保護し、`learn()` によるリアルタイム更新に対応する。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, RwLock};

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
    /// ユーザー辞書。`learn()` でリアルタイム更新するため RwLock で保護。
    user:       RwLock<HashMap<String, Vec<String>>>,
    mozc:       Option<MozcDict>,
    skk:        HashMap<String, Vec<String>>,
    skk_loaded: bool,
}

unsafe impl Send for DictStoreInner {}
unsafe impl Sync for DictStoreInner {}

#[derive(Clone)]
pub struct DictStore {
    inner: Arc<DictStoreInner>,
    /// ユーザー辞書ファイルパス（learn 時の保存先）
    user_path: Option<std::path::PathBuf>,
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
            inner: Arc::new(DictStoreInner {
                user: RwLock::new(user),
                mozc,
                skk: skk_map,
                skk_loaded,
            }),
            user_path: user_path.map(|p| p.to_path_buf()),
        })
    }

    pub fn empty() -> Self {
        Self {
            inner: Arc::new(DictStoreInner {
                user: RwLock::new(HashMap::new()),
                mozc: None,
                skk: HashMap::new(),
                skk_loaded: false,
            }),
            user_path: None,
        }
    }

    /// ユーザー辞書に学習語を登録し、メモリとファイルの両方を更新する。
    ///
    /// - メモリ上の `user` マップをリアルタイム更新 → 次回 `lookup` に即反映
    /// - `user_path` が設定されていれば `user_dict.toml` にも保存
    pub fn learn(&self, reading: &str, surface: &str) {
        // メモリ更新
        {
            let Ok(mut user) = self.inner.user.write() else {
                warn!("user dict write lock failed");
                return;
            };
            let entry = user.entry(reading.to_string()).or_default();
            // 先頭に挿入（最新の学習語を優先）、重複は削除
            entry.retain(|s| s != surface);
            entry.insert(0, surface.to_string());
        }
        // ファイル保存
        let Some(path) = &self.user_path else { return };
        let mut ud = UserDict::load(path).unwrap_or_default();
        ud.add(reading, surface);
        if let Err(e) = ud.save(path) {
            warn!("user_dict save failed: {e}");
        } else {
            info!("learned: {:?} -> {:?}", reading, surface);
        }
    }

    /// ひらがな読みからユーザー辞書候補のみを返す（merge_candidates 用）
    pub fn lookup_user(&self, reading: &str) -> Vec<String> {
        let Ok(user) = self.inner.user.read() else { return vec![]; };
        user.get(reading).cloned().unwrap_or_default()
    }

    /// ひらがな読みから mozc/skk 候補を返す（ユーザー辞書を除く）
    pub fn lookup_dict(&self, reading: &str, limit: usize) -> Vec<String> {
        let mozc_cands: Vec<String> = self.inner.mozc
            .as_ref()
            .map(|d| d.lookup(reading, limit).into_iter().map(|(s, _)| s).collect())
            .unwrap_or_default();

        let skk_cands = self.inner.skk.get(reading);

        let mut merged: Vec<String> = mozc_cands;
        if merged.len() < limit {
            if let Some(skk) = skk_cands {
                for s in skk {
                    if merged.len() >= limit { break; }
                    if !merged.contains(s) { merged.push(s.clone()); }
                }
            }
        }
        merged.truncate(limit);
        merged
    }

    /// ひらがな読みから候補リストを引く（優先順位: user > mozc > skk）
    /// 後方互換のために残す。merge_candidates では lookup_user/lookup_dict を使う。
    pub fn lookup(&self, reading: &str, limit: usize) -> DictResult {
        let user_cands = {
            let Ok(user) = self.inner.user.read() else {
                return DictResult { candidates: vec![], source: DictSource::None };
            };
            user.get(reading).cloned()
        };

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
                if !merged.contains(&s) { merged.push(s); }
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
    pub fn user_entry_count(&self) -> usize {
        self.inner.user.read().map(|u| u.len()).unwrap_or(0)
    }
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

    fn make_store(user: &[(&str, &str)], skk: &[(&str, &str)]) -> DictStore {
        let user_map: HashMap<String, Vec<String>> = user.iter()
            .map(|(r, s)| (r.to_string(), vec![s.to_string()]))
            .collect();
        let skk_map: HashMap<String, Vec<String>> = skk.iter()
            .map(|(r, s)| (r.to_string(), vec![s.to_string()]))
            .collect();
        DictStore {
            inner: Arc::new(DictStoreInner {
                user: RwLock::new(user_map),
                mozc: None,
                skk: skk_map,
                skk_loaded: !skk.is_empty(),
            }),
            user_path: None,
        }
    }

    #[test]
    fn test_user_priority() {
        let store = make_store(&[("きむら", "金村")], &[("きむら", "木村")]);
        let r = store.lookup("きむら", 10);
        assert_eq!(r.candidates[0], "金村");
        assert_eq!(r.source, DictSource::Merged);
    }

    #[test]
    fn test_skk_only() {
        let store = make_store(&[], &[("にほんご", "日本語")]);
        let r = store.lookup("にほんご", 10);
        assert_eq!(r.candidates, vec!["日本語"]);
        assert_eq!(r.source, DictSource::Skk);
    }

    #[test]
    fn test_no_hit() {
        let store = make_store(&[], &[]);
        let r = store.lookup("zzz", 10);
        assert!(r.candidates.is_empty());
        assert_eq!(r.source, DictSource::None);
    }

    #[test]
    fn test_learn_realtime() {
        let store = make_store(&[], &[("にほんご", "日本語")]);
        // 学習前: skk のみ
        let r = store.lookup("にほんご", 10);
        assert_eq!(r.candidates[0], "日本語");
        // 学習後: メモリに即反映
        store.learn("にほんご", "日本語〔学習〕");
        let r2 = store.lookup_user("にほんご");
        assert_eq!(r2[0], "日本語〔学習〕");
    }

    #[test]
    fn test_learn_dedup() {
        let store = make_store(&[("よみ", "表記A")], &[]);
        store.learn("よみ", "表記B");
        store.learn("よみ", "表記A"); // 重複 → 先頭に移動
        let r = store.lookup_user("よみ");
        assert_eq!(r[0], "表記A");
        assert_eq!(r.len(), 2); // 重複削除で2件
    }
}
