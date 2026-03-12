# esaxx-rs パッチのセットアップ（初回のみ）

## 問題

`esaxx-rs` の `build.rs` が `.static_crt(true)` を明示指定するため `/MT` でビルドされる。
`llama-cpp-sys-2` は `/MD` でビルドされるため、リンク時に LNK2038 が発生する。

## 解決策

`[patch.crates-io]` で `esaxx-rs` を上書きし、`static_crt(false)` に変更する。

## セットアップ手順（初回のみ）

```powershell
# 1. まず cargo fetch でキャッシュを確保
cargo fetch

# 2. esaxx.cpp をキャッシュからパッチディレクトリへコピー
.\scripts\setup-esaxx-patch.ps1

# 3. 通常通りビルド
.\scripts\build-engine.ps1 -Profile release
```

## 確認

`patches\esaxx-rs\src\esaxx.cpp` が存在すればセットアップ完了。
