# Warning fixes included in v0.2.0

v0.2.0 には、以下の warning 修正が含まれる。

## Rust 2024 / tray 側

- `crates/rakukan-tray/src/main.rs` から `unsafe fn` 由来の曖昧な unsafe 境界を整理
- Win32 / GDI 呼び出しと raw pointer 書き込みを局所 `unsafe { ... }` に変更
- Rust 2024 の `unsafe_op_in_unsafe_fn` warning を解消

## TSF / Phase 2 移行途中

- `SessionState` 導入後に未使用になった `SelectionState` 側 API を整理
- 未使用 import / 未使用関数 / 未使用メソッドを削除
- `-BuildOnly` 構成で warning なしのビルドが通る状態を確認
