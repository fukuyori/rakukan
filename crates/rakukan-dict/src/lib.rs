//! rakukan-dict — 辞書パーサー・ユーザー辞書管理
//!
//! 辞書の優先順位:
//! 1. ユーザー登録語（user_dict.toml）
//! 2. mozc バイナリ辞書（rakukan.dict, インストール時にビルド）
//!
//! 辞書ファイルは %LOCALAPPDATA%\rakukan\dict\ に配置する。

pub mod user_dict;
pub mod store;
pub mod mozc_dict;

pub use store::DictStore;

use std::path::PathBuf;

/// 辞書ディレクトリ（%LOCALAPPDATA%\rakukan\dict）
pub fn dict_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    if let Ok(localappdata) = std::env::var("LOCALAPPDATA") {
        return Some(PathBuf::from(localappdata).join("rakukan").join("dict"));
    }
    #[cfg(not(target_os = "windows"))]
    if let Ok(home) = std::env::var("HOME") {
        return Some(PathBuf::from(home).join(".config").join("rakukan").join("dict"));
    }
    None
}

/// ユーザー辞書ファイルパス（%APPDATA%\rakukan\user_dict.toml）
pub fn user_dict_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        return Some(PathBuf::from(appdata).join("rakukan").join("user_dict.toml"));
    }
    #[cfg(not(target_os = "windows"))]
    if let Ok(home) = std::env::var("HOME") {
        return Some(PathBuf::from(home).join(".config").join("rakukan").join("user_dict.toml"));
    }
    None
}

/// rakukan.dict のパス（%LOCALAPPDATA%\rakukan\dict\rakukan.dict）
pub fn find_mozc_dict() -> Option<PathBuf> {
    let p = dict_dir()?.join("rakukan.dict");
    Some(p)
}
