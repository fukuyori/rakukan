//! rakukan-dict — 辞書パーサー・ユーザー辞書管理
//!
//! 辞書の優先順位:
//! 1. ユーザー登録語（user_dict.toml）
//! 2. mozc バイナリ辞書（rakukan.dict, インストール時にビルド）
//!
//! 辞書ファイルは %LOCALAPPDATA%\rakukan\dict\ に配置する。

pub mod mozc_dict;
pub mod store;
pub mod user_dict;

/// 辞書エントリの cost 帯（Step 12-2、Issue #42）。
///
/// mozc 由来の通常語は cost < `NORMAL_MAX`（2026-09-12 の実測で最大 18,318）。
/// `symbol.tsv` の記号は `SYMBOL_BASE + 行番号`、`emoji_data.tsv` の絵文字は
/// `EMOJI_BASE + 行番号` に置く。これで通常語 → 記号 → 絵文字の順に並び、
/// 記号・絵文字は mozc の行順（優先順）を保つ。バイナリ形式（`VERSION` 1）は変えない。
///
/// 旧辞書（記号 3000 / 絵文字 6000 の固定 cost）はすべて「通常語」に分類されるので、
/// 再生成するまでは旧来の並びのまま動く。再生成の判断は builder が書く
/// `rakukan.dict.build.json` の `dict_schema` を `install.ps1` が比較して行う。
pub mod cost_band {
    /// 通常語の cost はこれ未満。builder はこれ以上の通常語を `NORMAL_MAX - 1` に丸める
    pub const NORMAL_MAX: u16 = 20_000;
    /// 記号帯の先頭。記号の cost = `SYMBOL_BASE + symbol.tsv の行番号`
    pub const SYMBOL_BASE: u16 = 30_000;
    /// 絵文字帯の先頭。絵文字の cost = `EMOJI_BASE + emoji_data.tsv の行番号`
    pub const EMOJI_BASE: u16 = 40_000;
    /// cost 帯の版。`rakukan.dict.build.json` に書き、`install.ps1` の期待値と比較する。
    /// 帯の意味を変えたら上げる（`scripts/install.ps1` の `$dictSchemaExpected` も同時に）
    pub const DICT_SCHEMA: u32 = 2;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Class {
        Normal,
        Symbol,
        Emoji,
    }

    pub fn classify(cost: u16) -> Class {
        if cost >= EMOJI_BASE {
            Class::Emoji
        } else if cost >= SYMBOL_BASE {
            Class::Symbol
        } else {
            Class::Normal
        }
    }
}

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
        return Some(
            PathBuf::from(home)
                .join(".config")
                .join("rakukan")
                .join("dict"),
        );
    }
    None
}

/// ユーザー辞書ファイルパス（%APPDATA%\rakukan\user_dict.toml）
pub fn user_dict_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        return Some(
            PathBuf::from(appdata)
                .join("rakukan")
                .join("user_dict.toml"),
        );
    }
    #[cfg(not(target_os = "windows"))]
    if let Ok(home) = std::env::var("HOME") {
        return Some(
            PathBuf::from(home)
                .join(".config")
                .join("rakukan")
                .join("user_dict.toml"),
        );
    }
    None
}

/// 学習履歴ファイルパス（%APPDATA%\rakukan\learn_history.bin）
///
/// `engine.learn()` で更新される `(reading, surface) → LearnEntry` マップを
/// bincode バイナリ形式で保存する。user_dict.toml とは別ファイル。
pub fn learn_history_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        return Some(
            PathBuf::from(appdata)
                .join("rakukan")
                .join("learn_history.bin"),
        );
    }
    #[cfg(not(target_os = "windows"))]
    if let Ok(home) = std::env::var("HOME") {
        return Some(
            PathBuf::from(home)
                .join(".config")
                .join("rakukan")
                .join("learn_history.bin"),
        );
    }
    None
}

/// rakukan.dict のパス（%LOCALAPPDATA%\rakukan\dict\rakukan.dict）
pub fn find_mozc_dict() -> Option<PathBuf> {
    let p = dict_dir()?.join("rakukan.dict");
    Some(p)
}
