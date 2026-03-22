# rakukan handoff — 2026-03-22

## 現在のバージョン
v0.3.7（`Cargo.toml` / `VERSION` / `CHANGELOG.md` / `handoff.md` 更新済み）

## ファイル配置

| ファイル | 配置先 |
|---------|--------|
| DLL・EXE | `%LOCALAPPDATA%\rakukan\` |
| `rakukan.log` | `%LOCALAPPDATA%\rakukan\` |
| `rakukan.dict` | `%LOCALAPPDATA%\rakukan\dict\` |
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

> **注意**: `build-engine` は `rakukan-engine` と `rakukan-dict` の両方をクリーンしてから DLL をビルドする。
> ABI 変更後は必ず `build-engine` → `reinstall` の順で実行すること。

## v0.3.7 で修正・変更した内容

### [FIXED] `/` キーで `・` が入力されない問題
- `symbol_fixed` が `/` を全角 `／` に変換していたためローマ字ルールが機能していなかった
- F9/F10 変換時も `/` を正しく出力するよう対応

### [FIXED] LLM 出力にふりがなが混入する問題
- `clean_model_output` に `strip_furigana` を追加
- 括弧内がひらがな・カタカナのみの場合に除去（例: 「健診(けんしん)や」→「健診や」）

### [FIXED] F9 変換で `、。` が `，．` に変換されない問題
- `ascii_to_fullwidth` が和文句読点を返していた
- F9/F10（全角英数モード）では `，．` を返すよう修正

### [FIXED] 候補ウィンドウが画面下部で見えなくなる問題
- `MonitorFromPoint` + `GetMonitorInfoW` で作業領域を取得し、はみ出す場合にキャレット上側へ反転表示
- 下表示時は +1px、上表示時は -4px のオフセット調整

### [ADDED] Shift+A–Z で全角大文字を入力（F9/F10 サイクル対応）
- `push_fullwidth_alpha()` を追加: `hiragana_buf` に `Ａ`、`romaji_input_log` に ASCII `A` を記録
- F9（`ａ`→`Ａ`→…）/ F10（`A`→`a`→…）のサイクル変換に対応
- ABI 変更あり（`engine_push_fullwidth_alpha` FFI 追加）

### [CHANGED] `symbol_fixed` 関数を削除・トライに統合
- `,./[]\-` の変換をローマ字トライルール（`rules.rs`）に統合
- その他 ASCII 記号（`@#$%` 等）は `push_char` 内でインライン全角変換
- `-` の文脈依存ロジック廃止。トライで常に `ー`
- `¥`（JIS キー U+00A5）→ `￥` をトライに追加

### [CHANGED] インストーラーから SKK 辞書ダウンロード機能を削除
- `[Tasks]` / `[Files]` / `[Run]` の SKK 関連エントリを除去

## 残課題

- [ ] ライブ変換（Phase 4a〜4c）
- [ ] SplitPreedit の複数文節連続変換
- [ ] 数字・かな混在入力（例: `400じ` → `400字`）
- [ ] LLM候補数増加（min(3)→min(5) 検討中。レイテンシ確認後に実施）
- [ ] LLM出力に読み仮名が混入する問題（ふりがな除去で部分対応済み、プロンプト抑制は未対応）

## ログ収集コマンド

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 50 |
  Select-String "on_convert|bg_wait|completed|engine-init|dict|Input|Backspace"
```

## 重要な技術的制約

- **TSF スレッド制約**: `RequestEditSession` は `WndProc` から呼べない（ライブ変換実装の最大障壁）
- **engine DLL のログ**: `tracing::info!` は `rakukan.log` に出ない。`set_dict_status` 経由でのみ TSF ログに届く
- **TOML セクション順序**: トップレベルキーはセクションヘッダより前に書く必要がある
- **install.ps1 / build-engine.ps1 エンコーディング**: ASCII のみ（日本語コメント不可）
- **ABI 変更時のビルド順序**: 必ず `build-engine` → `reinstall` の順で実行
- **Cargo キャッシュと ZIP 配布**: ZIP 展開ではタイムスタンプが変わらず Cargo が変更検知できない場合がある。`C:\rb\release\.fingerprint\rakukan-dict-*` を直接削除すれば解決
