# rakukan handoff — 2026-03-20

## 現在のバージョン
v0.3.5（`Cargo.toml` / `CHANGELOG.md` / `handoff.md` 更新済み）

## ファイル配置

| ファイル | 配置先 |
|---------|--------|
| DLL・EXE | `%LOCALAPPDATA%\rakukan\` |
| `rakukan.log` | `%LOCALAPPDATA%\rakukan\` |
| `rakukan.dict` / `SKK-JISYO.L` | `%LOCALAPPDATA%\rakukan\dict\` |
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

> **注意**: `build-engine` は `C:\rb` と `target/` の両方の `rakukan-engine-abi`
> キャッシュを削除してから DLL をビルドする。ABI 変更後は必ず `build-engine` から実行すること。

## v0.3.5 で修正・変更した内容

### [FIXED] F9/F10 が機能しない問題
- `on_latin_convert` が `hiragana_text()`（かな文字列）をローマ字として使っていた
- `romaji_log_str()` に変更して正しいローマ字ログを参照するよう修正

### [FIXED] F9 変換結果が文字数分重複する問題
- `romaji_input_log` の `Buffered` 記録で確定済みエントリに後続の子音を誤追記していた
- 例: `らぢお` → F9 → `ｒｒａｄｄｉｏ`（正しくは `ｒａｄｉｏ`）
- `Buffered` 時はログに触らず `Converted`/`PassThrough` 時にのみ push する方式に変更

### [FIXED] F9/F10 後に F6/F7/F8 でかなに戻せない問題
- F9/F10 後の `hiragana_buf` は全角/半角ラテン文字になっており `to_hiragana` 等が処理できなかった
- `hiragana_from_romaji_log()` を新設（`engine/lib.rs`, `ffi.rs`, `engine-abi/lib.rs`）
- `on_kana_convert` でかな以外のとき `hiragana_from_romaji_log()` で復元してから変換関数を適用

### [CHANGED] 記号・数字の入力を常に全角に統一
- `,`→`、` `.`→`。` `[`→`「` `]`→`」` `\`→`￥`、その他ASCII記号→全角
- `-` のみ直前がかな→`ー`、それ以外→`－`
- 数字 `0`–`9` を常に全角 `０`–`９`（テンキー除く）

### [CHANGED] ユーザー学習語のリアルタイム反映
`DictStore::learn()` を追加、IME 再起動なしで学習内容が候補に反映される

### [CHANGED] 候補順序
`merge_candidates` の優先順位: ユーザー辞書 → LLM → mozc/skk

### [BUILD] build-engine の高速化
- llama-cpp-sys-2 の CUDA/Vulkan キャッシュを維持したまま rakukan-engine のみ再ビルド
- 通常は `cargo make build-engine`（約50秒）で完了
- `cargo make build-engine-full` でフルキャッシュ削除ビルド

### [BUILD] rakukan-engine-abi キャッシュクリアの信頼性向上
- `cargo clean` から `Remove-Item` による直接削除方式に変更
- `C:\rb` と `target/` の両方を確実に削除

## 残課題

- [ ] Shift+アルファベット入力の改善（Shift+R → 英字モード自動移行）
- [ ] `/` キーの挙動（ひらがな入力時→`・`、F9→`／`、F10→`/`）
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
- **engine DLL のログ**: `rakukan.log` には出力されない（engine DLL は独自 tracing subscriber）
- **TOML セクション順序**: トップレベルキーはセクションヘッダより前に書く必要がある
- **install.ps1 / build-engine.ps1 エンコーディング**: ASCII のみ（日本語コメント不可）
- **ABI 変更時のビルド順序**: 必ず `build-engine` → `reinstall` の順で実行
