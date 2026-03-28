//! ユーザー登録語辞書
//!
//! `%APPDATA%\rakukan\user_dict.toml` に TOML 形式で保存する。
//!
//! # ファイル形式
//! ```toml
//! [[entries]]
//! reading  = "きむら"
//! surfaces = ["木村", "金村"]   # 先頭が最優先候補
//!
//! [[entries]]
//! reading  = "らくかん"
//! surfaces = ["楽漢"]
//! ```

use std::collections::HashMap;
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct UserDict {
    #[serde(default)]
    pub entries: Vec<UserEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserEntry {
    pub reading: String,
    pub surfaces: Vec<String>,
}

impl UserDict {
    /// ファイルから読み込む。ファイルが存在しない場合は空の辞書を返す。
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            debug!("user_dict: not found, using empty dict");
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)?;
        let ud: Self =
            toml::from_str(&text).map_err(|e| anyhow::anyhow!("user_dict parse error: {e}"))?;
        info!(
            "user_dict: loaded {} entries from {}",
            ud.entries.len(),
            path.display()
        );
        Ok(ud)
    }

    /// ファイルに保存する
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = toml::to_string_pretty(self)
            .map_err(|e| anyhow::anyhow!("user_dict serialize error: {e}"))?;
        std::fs::write(path, text)?;
        debug!(
            "user_dict: saved {} entries to {}",
            self.entries.len(),
            path.display()
        );
        Ok(())
    }

    /// 読み → 候補リストの HashMap に変換する（DictStore 構築用）
    pub fn to_map(&self) -> HashMap<String, Vec<String>> {
        let mut map = HashMap::new();
        for entry in &self.entries {
            map.insert(entry.reading.clone(), entry.surfaces.clone());
        }
        map
    }

    /// エントリを追加または更新する
    /// 同じ reading が既にある場合は surfaces の先頭に挿入（重複除去）
    pub fn add(&mut self, reading: &str, surface: &str) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.reading == reading) {
            e.surfaces.retain(|s| s != surface);
            e.surfaces.insert(0, surface.to_string());
        } else {
            self.entries.push(UserEntry {
                reading: reading.to_string(),
                surfaces: vec![surface.to_string()],
            });
        }
    }

    /// エントリを削除する
    pub fn remove(&mut self, reading: &str, surface: &str) {
        if let Some(e) = self.entries.iter_mut().find(|e| e.reading == reading) {
            e.surfaces.retain(|s| s != surface);
        }
        self.entries.retain(|e| !e.surfaces.is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn test_add_and_save_load() {
        let mut ud = UserDict::default();
        ud.add("きむら", "木村");
        ud.add("きむら", "金村");
        assert_eq!(ud.entries[0].surfaces, vec!["金村", "木村"]);

        let f = NamedTempFile::new().unwrap();
        ud.save(f.path()).unwrap();

        let loaded = UserDict::load(f.path()).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].surfaces[0], "金村");
    }

    #[test]
    fn test_remove() {
        let mut ud = UserDict::default();
        ud.add("きむら", "木村");
        ud.add("きむら", "金村");
        ud.remove("きむら", "木村");
        assert_eq!(ud.entries[0].surfaces, vec!["金村"]);
    }

    #[test]
    fn test_to_map() {
        let mut ud = UserDict::default();
        ud.add("らくかん", "楽漢");
        let map = ud.to_map();
        assert_eq!(map["らくかん"], vec!["楽漢"]);
    }
}
