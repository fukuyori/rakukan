# esaxx-rs パッチのセットアップ（初回のみ）

## 問題

`esaxx-rs` の `build.rs` が `.static_crt(true)` を明示指定するため `/MT` でビルドされる。
`llama-cpp-sys-2` は `/MD` でビルドされるため、リンク時に LNK2038 が発生する。

## 解決策

`[patch.crates-io]` で `esaxx-rs` を上書きし、`static_crt(false)` に変更する。

## セットアップ手順

パッチ版のソース一式は `patches/esaxx-rs/` としてリポジトリに含まれているため、追加のセットアップは不要。
`cargo make build-engine` でそのままビルドできる。
