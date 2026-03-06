# Changelog

## [0.2.0] - 2026-03-06

### Added
- `SessionState` を導入し、TSF 層の論理状態を 1 か所へ寄せる土台を追加
- `Waiting` 状態を追加し、LLM 待機中の状態表現を `SessionState` 側でも保持可能にした
- Phase 2 の進捗メモとして `PHASE2_PREP.md` / `PHASE2_STATUS.md` を同梱

### Changed
- `config.toml` / `keymap.toml` の構造化と再読込を整備
- 候補操作、変換開始、確定、取消などの主要経路を `SessionState` 主体へ段階移行
- 数字キー候補選択などの高速判定を新しい状態層ベースへ変更
- README を v0.2.0 の位置づけに合わせて更新

### Fixed
- `rakukan-tray` の Rust 2024 `unsafe_op_in_unsafe_fn` warning を解消
- Phase 2 移行途中に発生した未使用コード warning を整理
- `-BuildOnly` 構成で warning なしのビルドが通る状態に調整

### Notes
- v0.2.0 は Phase 2 完了版ではなく、Phase 2 本体へ進むための整備版
- `SelectionState` はまだ一部互換レイヤとして残る
