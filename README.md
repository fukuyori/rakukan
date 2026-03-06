# rakukan v0.2.0

> ⚠️ **注意：現在テスト動作中です**
>
> rakukan は開発途中のソフトウェアです。インストールによって **Windows の動作が不安定になる可能性があります**。
> TSF（Text Services Framework）DLL をシステムに登録するため、インストール・アンインストールの操作は
> **自己責任** で行ってください。重要な作業環境への適用は推奨しません。

---

Windows 向け日本語 IME。  
[karukan](https://github.com/togatoga/karukan)（Hitoshi Togasaki 氏）の LLM ベース変換エンジンを中核とし、
[azooKey-Windows](https://github.com/fkunn1326/azooKey-Windows)（fkunn1326 氏）の TSF 層実装を参考に構築した IME です。

## v0.2.0 の位置づけ

v0.2.0 は、Phase 1 の設定・キーマップ整理を完了し、Phase 2 の状態機械導入を開始した **開発スナップショット版** です。
この時点では `SessionState` を中心に TSF 層の状態整理を進めており、候補操作・変換開始・確定・取消の主要経路は新しい状態層へ段階移行しています。
一方で、`SelectionState` はまだ互換レイヤとして一部に残っており、Phase 2 は継続中です。

### この版で入っている主な内容

- `config.toml` / `keymap.toml` の構造化と再読込
- US / JIS 配列を意識したキーマップ基盤
- システムトレイ常駐と入力モード表示
- Rust 2024 警告の整理
- `SessionState` 導入による TSF 状態管理の段階移行
- Phase 2 に向けた候補選択・待機状態の整理

### 同梱ドキュメント

- `CHANGELOG.md` — リリース履歴
- `PHASE1_SUMMARY.md` — Phase 1 の要点
- `PHASE2_PREP.md` — Phase 2 着手前の整理
- `PHASE2_STATUS.md` — v0.2.0 時点の Phase 2 状況
- `WARNING_FIXES.md` — 直近の warning 修正内容
- `THIRD_PARTY_LICENSES.md` — サードパーティライセンス

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

**SKK-JISYO.L** — SKK 開発チーム  
https://github.com/skk-dev/dict  
インストール時に自動ダウンロードし、Mozc 辞書に候補がない場合のフォールバックとして使用します。  
ライセンス：GNU General Public License v2

### 主要依存ライブラリ

| ライブラリ | 用途 | ライセンス |
|-----------|------|-----------|
| [llama-cpp-2](https://github.com/utilityai/llama-cpp-rs) | llama.cpp Rust バインディング | MIT |
| [llama.cpp](https://github.com/ggml-org/llama.cpp) | LLM 推論バックエンド | MIT |
| [yada](https://github.com/takuyaa/yada) | ダブル配列辞書 | MIT |
| [memmap2](https://github.com/RazrFalcon/memmap2) | バイナリ辞書の mmap 読み取り | MIT / Apache 2.0 |
| [windows-rs](https://github.com/microsoft/windows-rs) | Windows API バインディング | MIT / Apache 2.0 |
| [tokenizers](https://github.com/huggingface/tokenizers) | トークナイザー | Apache 2.0 |
| [encoding_rs](https://github.com/hsivonen/encoding_rs) | EUC-JP デコード（SKK-JISYO.L 対応） | MIT / Apache 2.0 |
| [anyhow](https://github.com/dtolnay/anyhow) / [thiserror](https://github.com/dtolnay/thiserror) | エラーハンドリング | MIT / Apache 2.0 |
| [tracing](https://github.com/tokio-rs/tracing) | 構造化ログ | MIT |

---

## ライセンス

rakukan 本体のコードは **MIT ライセンス** で配布します。

ただし、インストール時に取得する辞書ファイルには異なるライセンスが適用されます。

- **Mozc 辞書**（`rakukan.dict`）：Apache License 2.0
- **SKK-JISYO.L**：GNU General Public License v2

これらの辞書ファイルを再配布する場合は、それぞれのライセンス条件に従ってください。  
rakukan のインストーラーは辞書を GitHub から直接取得するため、rakukan のリポジトリ自体には辞書ファイルを含みません。

---

## 動作要件

- Windows 10 / 11（x64）
- [Rust](https://rustup.rs/)（MSVC ツールチェーン）
- Visual Studio 2022（C++ ワークロード）
- CMake 3.x 以上
- （オプション）CUDA Toolkit — GPU アクセラレーション用
- （オプション）Vulkan SDK — Vulkan バックエンド用

---

## インストール

```powershell
# エンジン DLL のビルド（初回または更新時）
cargo make build-engine

# インストール（管理者権限が必要）
sudo cargo make install
```

インストール先：`%LOCALAPPDATA%\rakukan\`

ログ：`%LOCALAPPDATA%\rakukan\rakukan.log`

### アンインストール

```powershell
sudo cargo make uninstall
```

---

## プロジェクト構成

```
rakukan/
├── Cargo.toml                   ワークスペース定義
├── Makefile.toml                cargo-make タスク
├── scripts/
│   ├── install.ps1              インストールスクリプト
│   └── build-engine.ps1         エンジン DLL ビルド
└── crates/
    ├── rakukan-engine/          変換エンジン（karukan 統合）
    ├── rakukan-engine-abi/      エンジン DLL FFI インターフェース
    ├── rakukan-tsf/             TSF IME DLL 本体
    ├── rakukan-tray/            システムトレイアプリ
    ├── rakukan-dict/            辞書パーサー（Mozc / SKK / ユーザー辞書）
    └── rakukan-dict-builder/    Mozc TSV → バイナリ辞書変換ツール
```

---

## 開発コマンド

```powershell
cargo make check          # ビルドエラー確認（高速）
cargo make test           # 全テスト
cargo make build-engine   # エンジン DLL ビルド
sudo cargo make install   # インストール
sudo cargo make uninstall # アンインストール
```

---

## ビルド要件

### 必須ツール

| ツール | 最低バージョン | 入手先 | 備考 |
|--------|--------------|--------|------|
| **Rust**（MSVC ツールチェーン） | 1.85 以上 | https://rustup.rs/ | `rustup default stable-x86_64-pc-windows-msvc` |
| **Visual Studio 2022** | Community 以上 | https://visualstudio.microsoft.com/ | C++ によるデスクトップ開発 ワークロード必須 |
| **CMake** | 3.21 以上 | https://cmake.org/ | PATH に追加すること |
| **Ninja** | 1.11 以上 | VS 同梱 または https://ninja-build.org/ | VS 環境から自動取得される |
| **cargo-make** | 最新 | `cargo install cargo-make` | ビルドタスク管理 |
| **gsudo**（sudo） | 最新 | `winget install gerardog.gsudo` | `sudo cargo make install` に必要 |
| **Git** | 2.x 以上 | https://git-scm.com/ | ソース取得に使用 |

> **注意：** Rust は MSVC ツールチェーン（`x86_64-pc-windows-msvc`）を使用してください。  
> GNU ツールチェーンでは Windows API バインディング（windows-rs）がビルドできません。

### GPU バックエンド（任意）

GPU を使用することで LLM 変換が大幅に高速化されます。いずれか一方、または両方をインストールできます。

#### CUDA（NVIDIA GPU 推奨）

| 項目 | 内容 |
|------|------|
| 対象 GPU | NVIDIA GeForce / RTX / Quadro シリーズ |
| 必要なもの | **CUDA Toolkit 12.x** |
| 入手先 | https://developer.nvidia.com/cuda-downloads |
| 確認コマンド | `nvcc --version` |
| インストール後 | `config.toml` に `gpu_backend = "cuda"` を設定 |

> CUDA Toolkit をインストールすると `nvcc` が PATH に追加されます。  
> ビルドスクリプトが自動検出し、`rakukan_engine_cuda.dll` をビルドします。

#### Vulkan（AMD / Intel / NVIDIA 共通）

| 項目 | 内容 |
|------|------|
| 対象 GPU | Vulkan 対応の GPU 全般（AMD / Intel / NVIDIA） |
| 必要なもの | **Vulkan SDK** |
| 入手先 | https://vulkan.lunarg.com/sdk/home |
| 確認方法 | 環境変数 `VULKAN_SDK` が設定されていること |
| インストール後 | `config.toml` に `gpu_backend = "vulkan"` を設定 |

> Vulkan SDK をインストールすると `VULKAN_SDK` 環境変数が自動設定されます。  
> ビルドスクリプトがこれを検出し、`rakukan_engine_vulkan.dll` をビルドします。

#### CPU のみ（フォールバック）

GPU がない場合や、GPU SDK をインストールしない場合は CPU のみで動作します。  
変換速度は GPU 使用時より遅くなりますが、辞書変換は高速に動作します。

---

## LLM モデル

rakukan の LLM 変換には [jinen](https://huggingface.co/togatogah)（karukan 向けかな漢字変換モデル）を使用します。  
モデルは `config.toml` に `model_variant` を設定すると、`cargo make install` 実行時に Hugging Face から自動ダウンロードされます。

### 利用可能なモデル

| model_variant | モデル名 | サイズ | 推奨環境 |
|--------------|---------|--------|---------|
| `jinen-v1-small-q5` | jinen-v1-small Q5_K_M | 約 500 MB | GPU 推奨 |
| `jinen-v1-xsmall-q5` | jinen-v1-xsmall Q5_K_M | 約 250 MB | CPU でも動作可 |

### 設定方法

`%APPDATA%\rakukan\config.toml`（初回インストール後に自動生成）に以下を追加します：

```toml
# LLM モデルを指定（インストール時に Hugging Face から自動ダウンロード）
model_variant = "jinen-v1-small-q5"

# GPU バックエンド（cuda / vulkan / cpu）
gpu_backend = "cuda"

# 使用する GPU のインデックス（複数 GPU がある場合）
# main_gpu = 0

# 候補ウィンドウの表示候補数（1–9）
num_candidates = 9
```

> `model_variant` を設定しない場合、LLM 変換は無効になります。  
> 辞書変換（Mozc + SKK）のみで動作します。

---

## ビルド手順

### 初回セットアップ

```powershell
# 1. cargo-make と gsudo をインストール
cargo install cargo-make
winget install gerardog.gsudo

# 2. リポジトリをクローン
git clone https://github.com/yourname/rakukan.git
cd rakukan

# 3. エンジン DLL をビルド（初回のみ時間がかかります）
cargo make build-engine

# 4. インストール（管理者権限が必要）
sudo cargo make install
```

### 更新時

```powershell
# TSF DLL・トレイのみ更新（エンジンは再ビルドしない）
sudo cargo make install

# エンジンも含めてフルビルド・インストール
sudo cargo make install-full
```

### ビルド時間の目安

| 対象 | 初回 | 差分ビルド |
|------|------|----------|
| エンジン DLL（CPU） | 約 5〜15 分 | 約 20 秒 |
| エンジン DLL（Vulkan/CUDA 追加） | 追加 15〜30 分 | 約 20 秒 |
| TSF DLL・トレイ | 約 1〜3 分 | 数秒 |

> 初回ビルド時は llama.cpp のコンパイルに時間がかかります。  
> ビルドディレクトリは `C:\rb\`（短いパスで nvcc の誤認識を防ぐため固定）。
