# rakukan handoff — 2026-03-20

## 現在のバージョン
v0.3.6（`Cargo.toml` / `VERSION` / `CHANGELOG.md` / `handoff.md` 更新済み）

## ファイル配置

| ファイル | 配置先 |
|---------|--------|
| DLL・EXE | `%LOCALAPPDATA%\rakukan\` |
| `rakukan.log` | `%LOCALAPPDATA%\rakukan\` |
| `rakukan.dict` | `%LOCALAPPDATA%\rakukan\dict\` |
| `config.toml` / `keymap.toml` | `%APPDATA%\rakukan\` |
| `user_dict.toml` | `%APPDATA%\rakukan\` |
| LLMモデル | `~\.cache\huggingface\...` |

## ビルドコマンド

```powershell
# ABI変更を含む場合（engine DLL の変更）
cargo make build-engine
cargo make reinstall

# TSF側のみ変更の場合
cargo make reinstall

# llama-cpp-sys-2 のバージョン更新等、llama キャッシュごと全削除したい場合
cargo make build-engine-full
cargo make reinstall
```

> **注意**: `build-engine` は `rakukan-engine` と `rakukan-dict` の両方をクリーンしてから DLL をビルドする。
> ABI 変更後は必ず `build-engine` → `reinstall` の順で実行すること。

## v0.3.6 で修正・変更した内容

### [FIXED] mozc 辞書がロードされない問題（`mozc_dict.rs`）
- `from_mmap` 内の `reading_heap_size()` に `reading_heap_off` を渡していた（正しくは `index_off`）
- reading heap の UTF-8 バイト列をインデックスとして解釈し `entries_off` が約 3.8GB に誤算
- `rakukan.dict`（45MB）が常に「ファイルサイズ不足」で失敗していた

### [ADDED] 辞書ロードを `dict/loader.rs` に分離
- `load_dict()` が 4 ステップに分解: `resolve_paths` → `probe_mozc` → `open_mozc` → `load_store`
- 失敗時のログ: `failed at [open_mozc]: ...` のようにステップ名付きで原因が明確

### [ADDED] エンジン DLL にビルド時刻を埋め込み（`build.rs`）
- `dict_status` 経由で `rakukan.log` にエンジン DLL のビルド時刻が出力される
- ビルドが反映されているか確認するコマンド:
  ```powershell
  Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" |
      Select-String "dict_status" | Select-Object -Last 3
  # -> dict_status="starting: build=2026-03-20 ..."
  ```

### [ADDED] 辞書診断ツール `dict_check`
- `cargo run -p rakukan-dict --bin dict_check` で `rakukan.dict` を直接検証

### [BUILD] `build-engine.ps1` に `rakukan-dict` クリーンを追加
- インクリメンタルクリーン時に `cargo clean -p rakukan-dict` も実行
- `rakukan-dict` ソース変更がキャッシュに遮られる問題を防止

## 残課題

- [ ] Shift+アルファベット入力の改善（Shift+R → 英字モード自動移行）
- [ ] `/` キーの挙動（ひらがな入力時→`・`、F9→`/`、F10→`/`）
- [ ] ライブ変換（Phase 4a〜4c）
- [ ] SplitPreedit の複数文節連続変換
- [ ] 数字・かな混在入力（例: `400じ` → `400字`）

## ログ収集コマンド

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 50 |
  Select-String "on_convert|bg_wait|completed|engine-init|dict|Input|Backspace"
```

## 重要な技術的制約

- **TSF スレッド制約**: `RequestEditSession` は `WndProc` から呼べない（ライブ変換実装の最大障壁）
- **engine DLL のログ**: `tracing::info!` は `rakukan.log` に出ない。`set_dict_status` 経由でのみ TSF ログに届く
- **TOML セクション順序**: トップレベルキーはセクションヘッダより前に書く必要がある
- **install.ps1 / build-engine.ps1 エンコーディング**: ASCII のみ（日本語コメント不可）
- **ABI 変更時のビルド順序**: 必ず `build-engine` → `reinstall` の順で実行
- **Cargo キャッシュと ZIP 配布**: ZIP 展開ではタイムスタンプが変わらず Cargo が変更検知できない場合がある。`C:\rb\release\.fingerprint\rakukan-dict-*` を直接削除すれば解決
