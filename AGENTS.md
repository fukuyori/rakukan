# AGENTS.md

このファイルは Codex がこのプロジェクトで作業するときに毎回読み込むメモ。

## ビルド / インストール

```sh
cargo make build-engine
cargo make build-tsf
sudo cargo make install
```

### 補助コマンド

- `cargo make build-engine-full` — エンジンをフルクリーンビルド（llama CUDA キャッシュも削除、低速）
- `cargo make uninstall` — アンインストール
- `cargo make check` — 型チェックのみ（高速）
- `cargo make test` — エンジン単体テスト
- `cargo test --workspace --lib` — workspace 全体のテスト
- `cargo test -p rakukan-dict --lib` — rakukan-dict のテストのみ

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
