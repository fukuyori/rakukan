# rakukan handoff — 2026-03-18

## 現在のバージョン
v0.3.4（`Cargo.toml` / `rakukan_installer.iss` / `README.md` / `CHANGELOG.md` 更新済み）

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
# ABI変更を含む場合
cargo make build-engine
cargo make reinstall

# TSF側のみ変更の場合
cargo make reinstall
```

> **注意**: DLL の更新が反映されない場合は `C:\rb\release\` の該当 DLL を削除してから再実行する。

## v0.3.4 で修正・変更した内容

### [FIX] 変換後のカーソルジャンプ修正
候補選択中に次の文字を打つと composition が文末または文頭に開始される問題を修正。
`EndComposition` 前に確定テキストの range 末尾位置を保存し、その位置から新 composition を開始する。

### [FIX] 候補品質の改善
`d1_greedy` → 真のビームサーチ（`beam_size` 最大3）に戻す。

### [FIX] 辞書参照できない問題
辞書配置先を `%LOCALAPPDATA%\rakukan\dict\` に明確化（フォールバック削除）。

### [CHANGED] ASCII 記号のコンテキスト判定
`-` / `,` / `.` / `=` / `@` / `¥` 等を直前の文字種に応じて全角・半角・長音符に自動変換。

### [CHANGED] テンキー記号の根本修正
`UserAction::InputRaw` + `push_raw()` を新設してローマ字変換を完全バイパス。

### [CHANGED] F6/F7/F8 文字種変換を完全実装

### [CHANGED] バックスペース素通り修正

## 残課題

- [ ] `,` `.` の残課題（記号入力の継続検討）
- [ ] 括弧のルール検討
- [ ] F6/F7/F8/F9/F10 の動作詳細検討
- [ ] F9/F10 の `romaji_input_log` 実装
- [ ] ライブ変換（Phase 4）
- [ ] SplitPreedit の複数文節連続変換

## ログ収集コマンド

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 50 |
  Select-String "on_convert|bg_wait|completed|engine-init|dict|Input|Backspace"
```
