# CLAUDE.md

このファイルは Claude Code がこのプロジェクトで作業するときに毎回読み込むメモ。

## ビルド / インストール

**常に `cargo make` を使うこと。** `scripts/*.ps1` を直接叩くよう案内してはいけない。
PS1 は Makefile.toml から呼ばれる内部実装で、ユーザーには `cargo make <task>` のみを提示する。

### 標準ビルド手順（4 ステップ）

この順番で実行する。ユーザーへの案内もこの 4 コマンドで行う。

```sh
# 1. engine DLL (cpu / vulkan / cuda) をビルド
cargo make build-engine

# 2. TSF / tray / engine-host / dict-builder / WinUI 設定をビルド
cargo make build-tsf

# 3. ビルド成果物に電子署名（任意、signtool 環境が必要）
cargo make sign

# 4. %LOCALAPPDATA%\rakukan\ にコピー + TSF 登録 + tray 起動
#    ★ 管理者権限が必要（UAC 自動昇格あり）
cargo make install
```

一括実行したい場合:

- `cargo make full-install` — 1〜4 を順に実行（リリース向け）
- `cargo make quick-install` — 2→4 のみ（エンジン使い回し・署名なし、開発時の高速再インストール）

### 補助コマンド

- `cargo make build-engine-full` — エンジンをフルクリーンビルド（llama CUDA キャッシュも削除、低速）
- `cargo make uninstall` — アンインストール（管理者権限必要）
- `cargo make check` — 型チェックのみ（高速）
- `cargo make test` — エンジン単体テスト
- `cargo test --workspace --lib` — workspace 全体のテスト
- `cargo test -p rakukan-dict --lib` — rakukan-dict のテストのみ

### WinUI 設定アプリ単体ビルド

`dotnet build` を直接叩くと `MrtCore.PriGen.targets` の DLL 解決に失敗することがある。WinUI だけを反復検証するときは専用スクリプトを使う:

```
powershell -ExecutionPolicy Bypass -File scripts/build-settings-winui.ps1 -Configuration Debug
```

（このスクリプトは `cargo make build-tsf` の一部として内部的に呼ばれる）

## プロジェクト構成の注意点

- **out-of-process 化済み**: TSF DLL ↔ engine-host (RPC)。GPU リソースは engine-host が管理し、TSF 側は触らない。engine-host が複数起動していても GPU メモリは変換時のみ確保される。
- **エンジン DLL は 3 variant (`cpu` / `vulkan` / `cuda`)**: `config.toml` の `gpu_backend` で選択。`"auto"` だと順に検出。
- **ユーザー辞書は WinUI 設定が直接管理**: `user_dict.toml` は手動登録専用。engine は読み取りのみ。
- **学習履歴は別ファイル**: `%APPDATA%\rakukan\learn_history.bin` (bincode)。user_dict とは分離。
- **設定ファイル**:
  - `%APPDATA%\rakukan\config.toml` — 一般設定
  - `%APPDATA%\rakukan\keymap.toml` — キーバインド
  - `%APPDATA%\rakukan\user_dict.toml` — ユーザー辞書
  - `%APPDATA%\rakukan\learn_history.bin` — 学習履歴
  - `%LOCALAPPDATA%\rakukan\dict\rakukan.dict` — MOZC バイナリ辞書
