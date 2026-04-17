# Rakukan v0.2.0 時点の Phase 2 状況

## 今回進めた内容

- `SessionState` に `Waiting` 状態を追加した
- `SelectionState` にだけ存在していた LLM 待機中の概念を、`SessionState` でも保持できるようにした
- `factory.rs` の以下の分岐を `SessionState` 主体に寄せた
  - BG 完了時の待機解除
  - `Input`
  - `Convert`
  - `CommitRaw`
  - `Backspace`
  - `Cancel`
- `selection_sync_from_session()` を直接更新方式に修正し、相互同期中の逆流を避けた

## 現在地

- 候補操作の読む側だけでなく、主要な書き込み側も `SessionState` ベースに移行済み
- `SelectionState` は主に互換レイヤと BG 完了時の一部経路のために残っている
- v0.2.0 は、Phase 2 本体として状態中心が `SessionState` へかなり寄った段階のスナップショットである

## 次の作業

1. BG 完了時の `SelectionState` 依存を `SessionState` に寄せきる
2. `SelectionState` を mirror 専用へ縮退させる
3. `factory.rs` の状態別処理を関数分割する
4. `config.toml` の状態別キーマップ仕様を固定する
