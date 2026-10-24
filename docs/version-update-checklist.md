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
| `CHANGELOG.md` | `## [Next Release]` → `## [X.Y.Z] - YYYY-MM-DD` | Keep a Changelog 形式。版が決まるまでは `[Next Release]` の見出しで書き、版を上げるときに置き換える |
| `README.md` | 1 行目 `# rakukan vX.Y.Z` と「## 最新の変更」 | 最新版の要約を段落に、前版を箇条書きへ繰り下げる |

## 4. Windows 固有の version 面

- DLL / EXE の VERSIONINFO リソース（`winres` / `.rc`）は 2026-09-01 時点で**使っていない**。追加した場合はここに加える。
- `engine_abi_version()` / `EXPECTED_ENGINE_ABI_VERSION`（ABI 番号）はバージョン番号とは独立。FFI シグネチャを変えたときだけ **2 か所同時に**上げる（`crates/rakukan-engine/src/ffi.rs` と `crates/rakukan-engine-abi/src/lib.rs`）。

## 4.1 辞書の cost 帯（dict_schema）を変えたとき

`rakukan.dict` の cost 帯（`crates/rakukan-dict/src/lib.rs` の `cost_band`）の意味を変えたら、次を同時に行う。バージョン番号とは独立。

| 箇所 | 内容 |
|---|---|
| `cost_band::DICT_SCHEMA` | 1 上げる |
| `scripts/build-installer.ps1` の `$dictSchemaExpected` | 同じ値にする（古い辞書を再生成させる） |
| ビルドマシンの辞書 | `build-installer.ps1`（`cargo make install` / `cargo make package`）が、`C:\rb\dict\rakukan.dict.build.json` の `dict_schema`・`mozc_rev` が期待値と違えば作り直して同梱する。作り直せなかった場合、`package`（`-Sign`）は止まり、`install` は警告を出して古い辞書を使う。手で作り直すときは `RAKUKAN_FORCE_DICT=1` を付ける |

## 4.2 第三者のライセンス（依存を変えたとき）

| 対象 | 内容 |
|---|---|
| `docs/THIRD_PARTY_LICENSES.md` の「Rust Crates」の節 | `.\scripts\gen-third-party-licenses.ps1` で作り直す（cargo-about 0.9.2 が要る）。著作権者の名前の無い本文になる crate があると警告が出るので、上流の LICENSE を crate を作ったコミットで取り、`docs/licenses/<名前>-<版>.txt` に置く（同じ場所の README.md に出どころを書く）。手で書く節（karukan・azooKey・Mozc・llama.cpp・esaxx・Tomlyn・Windows App SDK）は、同梱する部品や版が変わったときに手で直す |
| llama.cpp の節 | `llama-cpp-sys-2` の版を変えたら、同梱の llama.cpp のコミット（llama-cpp-rs の `llama-cpp-sys-2/llama.cpp` のサブモジュール）を確かめ、その版の `LICENSE` と、コンパイルされるコードの表示（llamafile・YaRN・nlohmann/json・base64 など）を見直す |
| `docs/licenses/nvidia-cuda-*-eula.txt` | CUDA Toolkit の版を変えたら、その版の `EULA.txt` に差し替え、`build-installer.ps1` の使用許諾の文書の見出しと `THIRD_PARTY_LICENSES.md` の NVIDIA CUDA Runtime Loader の節を直す。CUDA のランタイムのリンクの形（hybrid の読み込み用のライブラリか、engine DLL が `cudart64_*.dll` を import しないか、`crates/rakukan-engine/build.rs` の差し替えで、llama-cpp-sys-2 が指定する `cudart.lib` が `cudart_static.lib` の写しに解決されているか。`cargo rustc ... -- --print link-args` で `cudart.lib` のパスが rakukan-engine の `OUT_DIR` になっていること）も確かめる |
| `docs/licenses/microsoft-*.txt` | Windows App SDK・Windows SDK の参照パックの版を変えたら、その版の条項に差し替え、`build-installer.ps1` の使用許諾の文書の見出し（版の番号）を直す |
| `docs/licenses/bundled-components.json` | 設定アプリの NuGet のパッケージ（間接の依存を含む）や、.NET・Windows SDK の版を変えたら、ライセンスを確かめてから版を直し、配布物に入るものは `THIRD_PARTY_LICENSES.md` に節を足す。`.\scripts\check-bundled-licenses.ps1` が確かめる（CI はパッケージ、`build-installer.ps1` は `-Files` で配布するファイルまで） |
| `deny.toml` の `allow` | 新しいライセンスの crate が入ったら、配布してよいかを確かめてから足す（CI の License Check が止める） |

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
# 4. リリースの状態の書き込みが無いか（版の番号・コミットで書く。docs/archive は除く）
git grep -n "未リリース\|最新リリース" -- ':!docs/archive/*' ':!docs/version-update-checklist.md'
# 5. 第三者のライセンスの一覧が依存と合っているか
.\scripts\gen-third-party-licenses.ps1 -Check
.\scripts\check-bundled-licenses.ps1
# 6. 空白チェック
git diff --check
```

リリース手順（ビルド → インストーラ作成 → タグ）はこの文書の範囲外。
