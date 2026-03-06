# Rakukan v0.2.0 に先行する Phase 2 着手前メモ

このスナップショットでは、全面的な Phase 2 再設計に入る前の足場として、
TSF 層へ `SessionState` を追加した。

## 到達点

- `engine/state.rs` に `SessionState` を追加
  - `Idle`
  - `Preedit { text }`
  - `Selecting { ... }`
- 既存の `SelectionState` は温存
- `SelectionState -> SessionState` の mirror 同期を追加
- `keymap.rs` の高速選択判定を `session_is_selecting_fast()` に変更
- `factory.rs` の候補操作系を `SessionState` ベースに変更
  - `Convert` の「すでに選択中」分岐
  - `CandidateNext`
  - `CandidatePrev`
  - `CandidatePageDown`
  - `CandidatePageUp`
  - `CandidateSelect`

## この段階で残していたもの

- `SelectionState` の書き込み側
- `CommitRaw` / `Backspace` / `Cancel` の選択中分岐
- LLM 待機中 (`llm_wait_preedit`) の一部処理

## 位置づけ

この文書は、v0.2.0 に至るまでの「Phase 2 着手前の整理内容」を示す補助メモである。
