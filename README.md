# rakukan v0.3.7

> ⚠️ **注意：現在テスト動作中です**
>
> rakukan は開発途中のソフトウェアです。インストールによって **Windows の動作が不安定になる可能性があります**。
> TSF（Text Services Framework）DLL をシステムに登録するため、インストール・アンインストールの操作は
> **自己責任** で行ってください。重要な作業環境への適用は推奨しません。

---

Windows 向け日本語 IME。  
[karukan](https://github.com/togatoga/karukan)（Hitoshi Togasaki 氏）の LLM ベース変換エンジンを中核とし、
[azooKey-Windows](https://github.com/fkunn1326/azooKey-Windows)（fkunn1326 氏）の TSF 層実装を参考に構築した IME です。

## 主な機能

- **LLM ベース変換**：jinen モデルによる自然な変換候補
- **辞書変換**：Mozc オープンソース辞書（約 113 万エントリ）+ Mozc 記号辞書（`symbol.tsv`）
- **ユーザー辞書学習**：確定した変換を記憶し即時反映
- **変換範囲変更**：Shift+左/右で変換対象を1文字単位で調整
- **文字種変換**：F6（ひらがな）/ F7（カタカナ）/ F8（半角カタカナ）/ F9（全角英数）/ F10（半角英数）のサイクル変換、相互に戻すことも可能
- **GPU アクセラレーション**：CUDA / Vulkan バックエンド対応
- **システムトレイ**：入力モード表示とバックエンド確認

---

## クレジット・謝辞

### 変換エンジン

**karukan** — Hitoshi Togasaki 氏  
https://github.com/togatoga/karukan  
llama.cpp を用いた LLM ベースのかな漢字変換エンジン。  
rakukan の変換コアとして使用しています。  
ライセンス：MIT

### TSF 層の参考実装

**azooKey-Windows** — fkunn1326 氏  
https://github.com/fkunn1326/azooKey-Windows  
Windows TSF（Text Services Framework）を用いた日本語 IME の実装。  
TSF アーキテクチャ・キーイベント処理・候補ウィンドウの設計を参考にしました。  
ライセンス：MIT

### 辞書

**Mozc オープンソース辞書** — Google  
https://github.com/google/mozc  
インストール時に `src/data/dictionary_oss/` 以下の TSV ファイル（約 113 万エントリ）を
GitHub から自動ダウンロードし、rakukan 独自のバイナリ形式に変換して使用します。  
ライセンス：Apache License 2.0


### 主要依存ライブラリ

| ライブラリ | 用途 | ライセンス |
|-----------|------|-----------| 
| [llama-cpp-2](https://github.com/utilityai/llama-cpp-rs) | llama.cpp Rust バインディング | MIT |
| [llama.cpp](https://github.com/ggml-org/llama.cpp) | LLM 推論バックエンド | MIT |
| [yada](https://github.com/takuyaa/yada) | ダブル配列辞書 | MIT |
| [memmap2](https://github.com/RazrFalcon/memmap2) | バイナリ辞書の mmap 読み取り | MIT / Apache 2.0 |
| [windows-rs](https://github.com/microsoft/windows-rs) | Windows API バインディング | MIT / Apache 2.0 |
| [tokenizers](https://github.com/huggingface/tokenizers) | トークナイザー | Apache 2.0 |

| [anyhow](https://github.com/dtolnay/anyhow) / [thiserror](https://github.com/dtolnay/thiserror) | エラーハンドリング | MIT / Apache 2.0 |
| [tracing](https://github.com/tokio-rs/tracing) | 構造化ログ | MIT |

---

## ライセンス

rakukan 本体のコードは **MIT ライセンス** で配布します。

ただし、インストール時に取得する辞書ファイルには異なるライセンスが適用されます。

- **Mozc 辞書**（`rakukan.dict`）：Apache License 2.0

これらの辞書ファイルを再配布する場合は、それぞれのライセンス条件に従ってください。  
rakukan のインストーラーは辞書を GitHub から直接取得するため、rakukan のリポジトリ自体には辞書ファイルを含みません。

---

## 動作要件

- Windows 10 / 11（x64）
- [Rust](https://rustup.rs/)（MSVC ツールチェーン）
- Visual Studio 2022（C++ ワークロード）
- CMake 3.x 以上
- （オプション）CUDA Toolkit 13.x — GPU アクセラレーション用（RTX シリーズ推奨）
- （オプション）Vulkan SDK — Vulkan バックエンド用

---

## インストール

```powershell
# 初回：esaxx-rs パッチのセットアップ（一度だけ必要）
cargo fetch
.\scripts\setup-esaxx-patch.ps1

# エンジン DLL のビルド（初回または更新時）
cargo make build-engine

# インストール（管理者権限が必要、ログアウト後に実行）
sudo cargo make install
```

インストール先：`%LOCALAPPDATA%\rakukan\`  
ログ：`%LOCALAPPDATA%\rakukan\rakukan.log`

> **注意：** `cargo make install` は DLL をファイルロックなしで上書きするため、  
> インストール前に一度ログアウト（またはサインアウト）してから実行してください。

### CUDA を使う場合（追加手順）

CUDA Toolkit 13.x をインストール後、以下の DLL を `C:\Windows\System32` にコピーしてください（管理者権限が必要、初回のみ）：

```powershell
$cudaLib = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.2\bin\x64"

Start-Process powershell -Verb RunAs -ArgumentList "-Command `"
  Copy-Item '$cudaLib\cudart64_13.dll'   'C:\Windows\System32\cudart64_13.dll'   -Force
  Copy-Item '$cudaLib\cublas64_13.dll'   'C:\Windows\System32\cublas64_13.dll'   -Force
  Copy-Item '$cudaLib\cublasLt64_13.dll' 'C:\Windows\System32\cublasLt64_13.dll' -Force
  # nvcudart_hybrid64.dll は cudart64_13.dll のコピー（llama-cpp が要求するため）
  Copy-Item '$cudaLib\cudart64_13.dll'   'C:\Windows\System32\nvcudart_hybrid64.dll' -Force
  Write-Host Done
`""
```

その後 `%APPDATA%\rakukan\config.toml` を編集して GPU バックエンドを設定します：

```toml
gpu_backend = "cuda"
```

### アンインストール

```powershell
sudo cargo make uninstall
```

---

## キー操作

| キー | 動作 |
|------|------|
| Space / 変換 | 変換開始 / 次候補 |
| Shift+Space | 全角スペース入力 |
| Enter | 確定 |
| ESC | 変換キャンセル（プリエディットに戻る） |
| Backspace | 1文字削除 |
| **Shift+左** | **変換範囲を1文字縮小** |
| **Shift+右** | **変換範囲を1文字拡大** |
| ↑ / ↓ | 候補を前後に移動 |
| 1〜9 | 候補を番号で選択 |
| Tab / PageDown | 次ページ |
| Shift+Tab / PageUp | 前ページ |
| F6 | ひらがな変換 |
| F7 | カタカナ変換 |
| F8 | 半角カタカナ変換 |
| F9 | 全角英字変換 |
| F10 | 半角英字変換 |

---

## 設定

`%APPDATA%\rakukan\config.toml`（初回インストール後に自動生成）：

```toml
# LLM モデルを指定（インストール時に Hugging Face から自動ダウンロード）
# model_variant = "jinen-v1-small-q5"    # ~500MB、GPU 推奨
# model_variant = "jinen-v1-xsmall-q5"   # ~250MB、CPU でも動作可

# GPU バックエンド（cuda / vulkan / cpu）
# gpu_backend = "cuda"

# 使用する GPU のインデックス（複数 GPU がある場合）
# main_gpu = 0

# 候補ウィンドウの表示候補数（1–9）
num_candidates = 9
```

> `model_variant` を設定しない場合、LLM 変換は無効になります。辞書変換のみで動作します。

---

## プロジェクト構成

```
rakukan/
├── Cargo.toml                   ワークスペース定義
├── Makefile.toml                cargo-make タスク
├── patches/
│   └── esaxx-rs/                esaxx-rs CRT パッチ（LNK2038 回避）
├── scripts/
│   ├── install.ps1              インストールスクリプト
│   ├── build-engine.ps1         エンジン DLL ビルド
│   └── setup-esaxx-patch.ps1   esaxx-rs パッチセットアップ
└── crates/
    ├── rakukan-engine/          変換エンジン（karukan 統合）
    │   └── src/dict/loader.rs   辞書ロード（4ステップ分離）
    ├── rakukan-engine-abi/      エンジン DLL FFI インターフェース
    ├── rakukan-tsf/             TSF IME DLL 本体
    ├── rakukan-tray/            システムトレイアプリ
    ├── rakukan-dict/            辞書パーサー（Mozc / ユーザー辞書）
    │   └── src/bin/dict_check   辞書ファイル診断ツール
    └── rakukan-dict-builder/    Mozc TSV → バイナリ辞書変換ツール
```

---

## 開発コマンド

```powershell
cargo make check          # ビルドエラー確認（高速）
cargo make test           # 全テスト
cargo make build-engine   # エンジン DLL ビルド
sudo cargo make install   # インストール（ログアウト後に実行）
sudo cargo make uninstall # アンインストール
```

---

## ビルド要件

| ツール | 最低バージョン | 入手先 |
|--------|--------------|--------|
| **Rust**（MSVC ツールチェーン） | 1.85 以上 | https://rustup.rs/ |
| **Visual Studio 2022** | Community 以上 | https://visualstudio.microsoft.com/ |
| **CMake** | 3.21 以上 | https://cmake.org/ |
| **Ninja** | 1.11 以上 | VS 同梱 または https://ninja-build.org/ |
| **cargo-make** | 最新 | `cargo install cargo-make` |
| **gsudo** | 最新 | `winget install gerardog.gsudo` |

> Rust は MSVC ツールチェーン（`x86_64-pc-windows-msvc`）を使用してください。

### GPU バックエンド（任意）

| バックエンド | 対象 GPU | 必要なもの |
|------------|---------|-----------|
| CUDA | NVIDIA GPU | CUDA Toolkit 13.x |
| Vulkan | AMD / Intel / NVIDIA | Vulkan SDK |

### ビルド時間の目安

| 対象 | 初回 | 差分ビルド |
|------|------|----------|
| エンジン DLL（CPU） | 約 5〜15 分 | 約 20 秒 |
| エンジン DLL（Vulkan/CUDA 追加） | 追加 15〜30 分 | 約 20 秒 |
| TSF DLL・トレイ | 約 1〜3 分 | 数秒 |

---

## トラブルシューティング

### ビルドエラー：`esaxx-rs` LNK2038

`esaxx-rs` が `/MT`（静的 CRT）でビルドされ、`llama-cpp-sys-2` の `/MD` と競合します。

```powershell
cargo fetch
.\scripts\setup-esaxx-patch.ps1
cargo make build-engine
```

### CUDA DLL ロードが失敗する（DLL load failed: rakukan_engine_cuda.dll）

CUDA 動作に必要な DLL が `C:\Windows\System32` にない場合に発生します。
上記「CUDA を使う場合（追加手順）」の DLL コピーを実施してください。

また、ビルドキャッシュが古い場合は以下でリセットできます：

```powershell
# ビルドキャッシュを完全削除（C:
b が build-dir の場合）
Remove-Item -Recurse -Force C:\rb

# esaxx-rs パッチを再適用（cargo clean 後は必須）
.\scripts\setup-esaxx-patch.ps1

# フルビルド（CUDA は 15〜30 分かかります）
cargo make build-engine
```

### インストール後に古い DLL が動いている

ログの `rakukan TSF DLL loaded  build=` の日時で確認してください。古い場合：

```powershell
# ログアウト後に管理者 PowerShell で
$dll = "$env:LOCALAPPDATA\rakukan\rakukan_tsf.dll"
regsvr32 /s /u $dll
Copy-Item "C:\rb\release\rakukan_tsf.dll" $dll -Force
regsvr32 /s $dll
```

### 辞書変換が機能しない（dict_status が retry / failed のまま）

ログで状態を確認します：

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" |
    Select-String "dict_status" | Select-Object -Last 3
```

辞書ファイルを直接診断するには：

```powershell
cargo run -p rakukan-dict --bin dict_check
```

`rakukan-dict` のビルドキャッシュが古い場合は以下で解決します：

```powershell
$env:CARGO_TARGET_DIR = "C:\rb"
Remove-Item C:\rb\release\.fingerprint\rakukan-dict-* -Recurse -Force -ErrorAction SilentlyContinue
cargo make build-engine
cargo make reinstall
```
