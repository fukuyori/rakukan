//! rakukan-dict — 辞書パーサー・ユーザー辞書管理
//!
//! 辞書の優先順位:
//! 1. ユーザー登録語（user_dict.toml）
//! 2. mozc バイナリ辞書（rakukan.dict, インストール時にビルド）
//!
//! 辞書ファイルはインストール先（engine DLL と同じディレクトリ）の `dict\` に配置する。

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

/// 辞書ディレクトリ（インストール先の `dict`）
///
/// この関数を含むモジュール（engine DLL）のディレクトリの `dict`。辞書はインストール先に
/// 置くので、配置（`%LOCALAPPDATA%\rakukan` / `%ProgramFiles%\rakukan`）に依存しない（#33）。
#[cfg(target_os = "windows")]
pub fn dict_dir() -> Option<PathBuf> {
    module_dir_of(dict_dir as *const ()).map(|d| d.join("dict"))
}

#[cfg(not(target_os = "windows"))]
pub fn dict_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("rakukan")
            .join("dict"),
    )
}

/// `anchor` のアドレスを含むモジュールのファイルがあるディレクトリ。
#[cfg(target_os = "windows")]
fn module_dir_of(anchor: *const ()) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleFileNameW, GetModuleHandleExW,
    };
    let mut module = HMODULE::default();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::PCWSTR(anchor as *const u16),
            &mut module,
        )
    }
    .ok()?;
    // 長いパスに備え、切り詰められたらバッファを広げて取り直す
    let mut buf = vec![0u16; 1024];
    loop {
        let len = unsafe { GetModuleFileNameW(module, &mut buf) } as usize;
        if len == 0 {
            return None;
        }
        if len < buf.len() {
            let path = PathBuf::from(std::ffi::OsString::from_wide(&buf[..len]));
            return path.parent().map(std::path::Path::to_path_buf);
        }
        if buf.len() >= 32 * 1024 {
            return None;
        }
        buf.resize(buf.len() * 2, 0);
    }
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

/// rakukan.dict のパス（インストール先の `dict\rakukan.dict`）
pub fn find_mozc_dict() -> Option<PathBuf> {
    let p = dict_dir()?.join("rakukan.dict");
    Some(p)
}
