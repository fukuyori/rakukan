//! rakukan-dict — 辞書パーサー・ユーザー辞書管理
//!
//! # 辞書検索の優先順位
//! 1. ユーザー登録語（user_dict.toml）
//! 2. mozc バイナリ辞書（rakukan.dict, インストール時にビルド）
//! 3. SKK辞書（SKK-JISYO.L 等）
//!
//! # ファイル配置
//! 辞書ファイルは %LOCALAPPDATA%\rakukan\dict\ に配置する。

pub mod skk;
pub mod user_dict;
pub mod store;
pub mod mozc_dict;

pub use store::DictStore;

use std::path::PathBuf;

/// 辞書ディレクトリ（%LOCALAPPDATA%\rakukan\dict）
pub fn dict_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let localappdata = std::env::var("LOCALAPPDATA").ok()?;
        Some(PathBuf::from(localappdata).join("rakukan").join("dict"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(".config").join("rakukan").join("dict"))
    }
}

/// ユーザー辞書ファイルパス（%APPDATA%\rakukan\user_dict.toml）
pub fn user_dict_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").ok()?;
        Some(PathBuf::from(appdata).join("rakukan").join("user_dict.toml"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(".config").join("rakukan").join("user_dict.toml"))
    }
}

/// rakukan.dict のパス（%LOCALAPPDATA%\rakukan\dict\rakukan.dict）
pub fn find_mozc_dict() -> Option<PathBuf> {
    let p = dict_dir()?.join("rakukan.dict");
    if p.exists() { Some(p) } else { None }
}

/// インストール済みの SKK-JISYO ファイルを優先度順に探す（L > ML > M > S）
pub fn find_skk_jisyo() -> Vec<PathBuf> {
    let Some(dir) = dict_dir() else { return vec![] };
    ["SKK-JISYO.L", "SKK-JISYO.ML", "SKK-JISYO.M", "SKK-JISYO.S"]
        .iter()
        .map(|name| dir.join(name))
        .filter(|p| p.exists())
        .collect()
}
