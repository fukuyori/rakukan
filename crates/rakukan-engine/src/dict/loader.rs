//! 辞書ロードの各ステップを分離したモジュール。
//!
//! 各ステップが独立した関数になっており、どこで失敗したかを
//! `dict_status` に記録することで原因を絞り込める。
//!
//! # ステップ
//! 1. `step_resolve_paths`  — インストール先（engine DLL の場所）から mozc/user パスを解決
//! 2. `step_probe_mozc`     — ファイル存在・サイズ・マジック確認
//! 3. `step_open_mozc`      — MozcDict::open（mmap + ヘッダー検証）
//! 4. `step_load_store`     — DictStore::load（user 辞書込み）

use rakukan_dict::mozc_dict::MozcDict;
use rakukan_dict::{DictStore, find_mozc_dict, learn_history_path, user_dict_path};
use std::path::PathBuf;

/// ローダーの各ステップ結果
pub enum LoadResult {
    /// 全ステップ成功
    Ok(DictStore),
    /// 失敗したステップと理由
    Failed { step: &'static str, reason: String },
}

/// 辞書のフルロードを試みる。
/// 各ステップで失敗した場合は `Failed` を返し、`reason` に詳細を記録する。
pub fn load_dict() -> LoadResult {
    // ── Step 1: パス解決 ──────────────────────────────────────────────────────
    let (mozc_path, user_path) = step_resolve_paths();

    // ── Step 2: mozc ファイルの事前プローブ ───────────────────────────────────
    if let Some(ref p) = mozc_path {
        if let Err(reason) = step_probe_mozc(p) {
            return LoadResult::Failed {
                step: "probe_mozc",
                reason,
            };
        }
    } else {
        return LoadResult::Failed {
            step: "resolve_paths",
            reason: "find_mozc_dict() returned None (engine DLL path not found?)".to_string(),
        };
    }

    // ── Step 3: MozcDict::open ────────────────────────────────────────────────
    let mozc_path_ref = mozc_path.as_deref().unwrap();
    if let Err(reason) = step_open_mozc(mozc_path_ref) {
        return LoadResult::Failed {
            step: "open_mozc",
            reason,
        };
    }

    // ── Step 4: DictStore::load ───────────────────────────────────────────────
    let user_ref = user_path.as_deref();
    let learn_history = learn_history_path();
    let learn_ref = learn_history.as_deref();
    match DictStore::load(user_ref, Some(mozc_path_ref), learn_ref) {
        Err(e) => LoadResult::Failed {
            step: "load_store",
            reason: format!("{e}"),
        },
        Ok(store) => {
            if !store.is_mozc_loaded() {
                // Step 3 で成功したのに store が mozc=None → 内部で再度失敗
                LoadResult::Failed {
                    step: "load_store",
                    reason: format!(
                        "DictStore::load succeeded but mozc=None \
                         (path={:?} size={}B)",
                        mozc_path_ref,
                        std::fs::metadata(mozc_path_ref)
                            .map(|m| m.len())
                            .unwrap_or(0),
                    ),
                }
            } else {
                tracing::info!("dict build info: {}", describe_build_info(mozc_path_ref));
                LoadResult::Ok(store)
            }
        }
    }
}

// ─── 各ステップ実装 ───────────────────────────────────────────────────────────

/// Step 1: 環境変数からパスを解決する
fn step_resolve_paths() -> (Option<PathBuf>, Option<PathBuf>) {
    let mozc = find_mozc_dict();
    let user = user_dict_path();
    (mozc, user)
}

/// Step 2: ファイルの存在・サイズ・マジックバイトを確認する（open しない）
fn step_probe_mozc(path: &std::path::Path) -> Result<(), String> {
    // 存在確認
    if !path.exists() {
        return Err(format!("file not found: {}", path.display()));
    }

    // サイズ確認
    let size = std::fs::metadata(path)
        .map(|m| m.len())
        .map_err(|e| format!("metadata failed: {e}"))?;

    if size < 16 {
        return Err(format!("file too small: {size}B"));
    }

    // マジックバイト確認（最初の4バイトを読む）
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("File::open failed: {e}"))?;
    let mut magic = [0u8; 4];
    f.read_exact(&mut magic)
        .map_err(|e| format!("read_exact failed: {e}"))?;

    if &magic != b"RKND" {
        return Err(format!(
            "magic mismatch: got {:?} (expected b\"RKND\") — dict may be stale or corrupt",
            magic
        ));
    }

    Ok(())
}

/// Step 3: MozcDict::open を呼ぶ（mmap + ヘッダー全検証）
fn step_open_mozc(path: &std::path::Path) -> Result<(), String> {
    MozcDict::open(path).map(|_| ()).map_err(|e| format!("{e}"))
}

// ─── 辞書のビルド情報（rakukan.dict.build.json）─────────────────────────────

/// `rakukan-dict-builder` が辞書の隣に書く `<辞書>.build.json` の、ログに出す項目。
/// どの項目も無いことがある（古いビルダーの辞書、取得元を固定しない呼び出し）。
#[derive(serde::Deserialize, Debug, Default, PartialEq)]
struct DictBuildInfo {
    mozc_rev: Option<String>,
    dict_schema: Option<u32>,
    format_version: Option<u32>,
    builder_version: Option<String>,
}

/// 辞書のパスから `build.json` のパスを作る（`rakukan.dict` → `rakukan.dict.build.json`）。
fn build_info_path(dict: &std::path::Path) -> PathBuf {
    let mut p = dict.as_os_str().to_owned();
    p.push(".build.json");
    PathBuf::from(p)
}

/// `build.json` の中身を 1 行にする。無い・読めないときも、その旨を返す（辞書の読み込みは止めない）。
fn describe_build_info(dict: &std::path::Path) -> String {
    let path = build_info_path(dict);
    match std::fs::read_to_string(&path) {
        Err(e) => format!("not available ({e}) path={}", path.display()),
        Ok(text) => describe_build_info_text(&text),
    }
}

fn describe_build_info_text(text: &str) -> String {
    match serde_json::from_str::<DictBuildInfo>(text) {
        Err(e) => format!("unreadable ({e})"),
        Ok(info) => {
            let num = |v: Option<u32>| v.map_or_else(|| "-".to_string(), |n| n.to_string());
            format!(
                "mozc_rev={} dict_schema={} format_version={} builder_version={}",
                info.mozc_rev.as_deref().unwrap_or("-"),
                num(info.dict_schema),
                num(info.format_version),
                info.builder_version.as_deref().unwrap_or("-"),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_path_appends_the_suffix() {
        let p = build_info_path(std::path::Path::new(r"C:\x\dict\rakukan.dict"));
        assert_eq!(p, PathBuf::from(r"C:\x\dict\rakukan.dict.build.json"));
    }

    #[test]
    fn describe_lists_the_four_items() {
        // rakukan-dict-builder が書く形（mozc_source は出さない）
        let text = r#"{
  "dict_schema": 2,
  "format_version": 1,
  "builder_version": "0.13.0",
  "mozc_rev": "cbbb6e1bd181cb9f3b409622d916a53a35400ec7",
  "mozc_source": "https://raw.githubusercontent.com/google/mozc/cbbb6e1"
}"#;
        assert_eq!(
            describe_build_info_text(text),
            "mozc_rev=cbbb6e1bd181cb9f3b409622d916a53a35400ec7 dict_schema=2 format_version=1 builder_version=0.13.0"
        );
    }

    #[test]
    fn describe_marks_missing_items() {
        assert_eq!(
            describe_build_info_text(r#"{"dict_schema": 2}"#),
            "mozc_rev=- dict_schema=2 format_version=- builder_version=-"
        );
    }

    #[test]
    fn describe_reports_unreadable_json() {
        assert!(describe_build_info_text("{").starts_with("unreadable ("));
    }

    #[test]
    fn describe_reports_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let s = describe_build_info(&dir.path().join("rakukan.dict"));
        assert!(s.starts_with("not available ("), "{s}");
        assert!(s.contains("rakukan.dict.build.json"), "{s}");
    }
}
