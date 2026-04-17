# Rakukan v0.2.0 に含まれる Phase 1 要約

このパッケージには、Phase 1 で整備した設定・キーマップ基盤が含まれている。

## 実装内容

- `config.toml` を serde ベースの正式構造体 (`engine/config.rs`) で管理
- `keyboard.layout = "us" | "jis" | "custom"` を追加
- `candidate.page_size` / `live_conversion.*` などの設定セクションを追加
- 旧形式 `num_candidates` との互換を維持
- 入力モード変更時に `config.toml` を再読込するキャッシュ管理を追加
- 入力モード変更時に `keymap.toml` も再読込するよう変更
- `keymap.toml` に `preset` / `inherit_preset` を追加
- `ms-ime-us` / `ms-ime-jis` プリセットを追加
- US 配列向けの記号キー名 (`BackQuote`, `Semicolon` など) を追加
- リポジトリ同梱の `config/config.toml` と `config/keymap.toml` を新形式に更新

## 主な変更ファイル

- `crates/rakukan-tsf/src/engine/config.rs`
- `crates/rakukan-tsf/src/engine/mod.rs`
- `crates/rakukan-tsf/src/engine/keymap.rs`
- `crates/rakukan-tsf/src/engine/state.rs`
- `crates/rakukan-tsf/src/tsf/factory.rs`
- `crates/rakukan-tsf/src/lib.rs`
- `config/config.toml`
- `config/keymap.toml`

## v0.2.0 での扱い

Phase 1 の整備内容は、v0.2.0 の土台として維持されている。
その上に、Phase 2 の状態機械整理が段階的に積み上がっている。
