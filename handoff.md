# rakukan handoff — 2026-03-18

## 現在のバージョン
v0.3.3（`Cargo.toml` / `rakukan_installer.iss` / `README.md` / `CHANGELOG.md` 更新済み）

## プロジェクト構成

| パス | 役割 |
|------|------|
| `crates/rakukan-engine/src/lib.rs` | エンジン本体（`push_char` / `push_raw` / コンテキスト判定） |
| `crates/rakukan-engine/src/conv_cache.rs` | BG変換キャッシュ（状態機械＋Condvar） |
| `crates/rakukan-engine/src/kanji/backend.rs` | LLM変換バックエンド（d1_greedy_batch） |
| `crates/rakukan-engine/src/kanji/llamacpp.rs` | llama.cpp推論（n_ctx動的拡張） |
| `crates/rakukan-engine/src/ffi.rs` | エンジン DLL FFI（push_raw追加） |
| `crates/rakukan-engine-abi/src/lib.rs` | エンジン DLL ABI（push_raw追加） |
| `crates/rakukan-tsf/src/tsf/factory.rs` | TSF メインロジック（InputRaw対応） |
| `crates/rakukan-tsf/src/tsf/candidate_window.rs` | 候補ウィンドウ（GDI描画 + WM_TIMERポーリング） |
| `crates/rakukan-tsf/src/engine/state.rs` | SessionState 定義・エンジン初期化 |
| `crates/rakukan-tsf/src/engine/config.rs` | config.toml 読み込み・デフォルト生成 |
| `crates/rakukan-tsf/src/engine/keymap.rs` | キーマップ（テンキー早期処理・InputRaw） |
| `crates/rakukan-tsf/src/engine/user_action.rs` | UserAction定義（InputRaw追加） |
| `crates/rakukan-tsf/src/engine/text_util.rs` | 文字種変換（F6/F7/F8完全実装） |

## ビルドコマンド

```powershell
# ABI変更を含む場合（ffi.rs / abi/lib.rs / llamacpp.rs / backend.rs）
cargo make build-engine
cargo make reinstall

# TSF側のみ変更の場合（factory.rs / keymap.rs / text_util.rs 等）
cargo make reinstall
```

> **注意**: エンジン DLL の変更を `reinstall` だけで反映しようとすると
> IME がランゲージバーで選択不能になる。

## v0.3.3 で修正・変更した内容

### [FIX] バックスペース素通り（子音単体入力中）
`preedit_is_empty()` が `hiragana_buf` のみ確認し `pending_romaji_buf` を無視していた。

### [FIX] 複数候補生成時のクラッシュ
`d1_greedy_batch` の `n_batch > n_ctx` により llama.cpp の `GGML_ASSERT` が `abort()`。
`n_ctx` を動的拡張し `beam_size` を最大5にキャップして解消。

### [FIX] テンキー記号がかな変換される問題
`UserAction::InputRaw` と `push_raw()` を新設してローマ字変換を完全バイパス。

### [CHANGED] 変換速度改善
`generate_beam_search` → `generate_beam_search_d1_greedy_batch`（KVキャッシュ共有）

### [CHANGED] F8 半角カタカナ変換を完全実装
ひらがな・全角カタカナ → 半角カタカナ（濁音/半濁音は結合文字2文字）
全角英数記号も半角に変換。

### [CHANGED] F6/F7 に全角変換を追加
半角英数記号 → 全角、半角カタカナ → 全角カタカナ/ひらがな対応。

### [CHANGED] `-` のコンテキスト判定
直前文字種に応じて `ー` / `ｰ` / `－` / `-` を自動選択。

### [CHANGED] ASCII 記号のコンテキスト判定
全角コンテキスト直後の `=`, `@`, `(`, `¥` 等を全角記号に自動変換。

### [CHANGED] コンテキストトリミング改善
`commit()` の200文字トリミングを文境界（`。！？`等）で行うよう改善。

## 残課題

- [ ] F9/F10 のローマ字バッファ実装（`romaji_input_log` の導入）
  - `Vec<String>` でひらがな1文字単位にまとめて蓄積
  - バックスペース時は `pop()` で対応
  - F9: `romaji_input_log` を結合して全角化
  - F10: `romaji_input_log` を結合して半角化
- [ ] CUDA DLL（`cublas64_13.dll` 等）の System32 コピーをインストーラーに組み込む
- [ ] ライブ変換（Phase 4）
- [ ] SplitPreedit の複数文節連続変換

## ログ収集コマンド

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 50 |
  Select-String "on_convert|bg_wait|completed|engine-init|engine created|DLL load|SLOW|Input|Backspace"
```
