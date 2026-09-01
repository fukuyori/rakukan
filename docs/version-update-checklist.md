# バージョン更新チェックリスト

バージョン番号を変更するときに更新するファイルの一覧。`X.Y.Z` を新バージョンに読み替える。
2026-09-01（0.10.4 → 0.11.0）の洗い出し結果。項目が増減したらこの文書を更新する。

## 1. ソースの単一情報源

| ファイル | 箇所 | 備考 |
|---|---|---|
| `VERSION` | 1 行目 | `scripts/build-installer.ps1` が `-Version` 未指定時に読む。インストーラの版数の元 |
| `Cargo.toml` | `[workspace.package] version` | 全 crate が `version.workspace = true` で継承。`env!("CARGO_PKG_VERSION")`（host / DLL の build 識別子、Issue #8）もここから |
| `Cargo.lock` | `rakukan-*` の `version` | 手で書かず `cargo check --workspace` で更新する |

## 2. リリースメタデータ

| ファイル | 箇所 | 備考 |
|---|---|---|
| `rakukan_installer.iss` | `#define MyAppVersion` | ビルド時に `build-installer.ps1` が `VERSION` の値で置換するが、ソースも揃えておく |
| `apps/rakukan-settings-winui/Rakukan.Settings.WinUI.csproj` | `<Version>` | WinUI 設定アプリのアセンブリ版数。`bin/` `obj/` `dist/` 配下の `.deps.json` 等は生成物なので触らない |

## 3. ドキュメント

| ファイル | 箇所 | 備考 |
|---|---|---|
| `CHANGELOG.md` | `## [Unreleased]` → `## [X.Y.Z] - YYYY-MM-DD` | Keep a Changelog 形式。次の開発開始時に `[Unreleased]` を再作成する |
| `README.md` | 1 行目 `# rakukan vX.Y.Z` と「## 最新の変更」 | 最新版の要約を段落に、前版を箇条書きへ繰り下げる |

## 4. Windows 固有の version 面

- DLL / EXE の VERSIONINFO リソース（`winres` / `.rc`）は 2026-09-01 時点で**使っていない**。追加した場合はここに加える。
- `engine_abi_version()` / `EXPECTED_ENGINE_ABI_VERSION`（ABI 番号）はバージョン番号とは独立。FFI シグネチャを変えたときだけ **2 か所同時に**上げる（`crates/rakukan-engine/src/ffi.rs` と `crates/rakukan-engine-abi/src/lib.rs`）。

## 5. 変更しないもの

- `crates/rakukan-engine-abi/src/lib.rs` の `build_id_tests` にある `"0.10.4"` / `"0.10.5"` はテスト用の固定値であり、実バージョンに追随させない。
- `docs/*.md` の過去の実施記録に出てくる旧バージョン番号は履歴なので書き換えない。

## 6. 手順

```powershell
# 1. 上記 1〜3 を編集
# 2. Cargo.lock を更新し、ビルドが通ることを確認
cargo check --workspace --all-targets
# 3. 表記漏れの確認（旧バージョン番号が残っていないか。生成物とテスト固定値は除く）
git grep -n "0\.10\.4" -- ':!*.deps.json' ':!Cargo.lock' ':!docs/*' ':!CHANGELOG.md'
# 4. 空白チェック
git diff --check
```

リリース手順（ビルド → インストーラ作成 → タグ）はこの文書の範囲外。
