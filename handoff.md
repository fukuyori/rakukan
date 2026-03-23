# Rakukan 引き継ぎ資料 (v0.3.8)

更新日: 2026-03-23

---

## 現在の状態

**バージョン:** v0.3.8
**ビルドパス:** `C:\Users\n_fuk\source\rust\rakukan`（または `D:\home\source\rust\rakukan`）
**インストール先:** `%LOCALAPPDATA%\rakukan\`
**ログ:** `%LOCALAPPDATA%\rakukan\rakukan.log`

---

## v0.3.8 で完了した変更

### config.toml 整理（`config.rs`）
- `[candidate]` / `[conversion]` セクションを削除（未実装のため）
- `CandidateConfig` / `ConversionConfig` / `CancelBehavior` 構造体を削除
- `effective_num_candidates()` を `num_candidates.unwrap_or(9).clamp(1, 9)` に単純化
- `enable_jis_keys` フィールドを削除（`layout = "jis"` に統合）
- キーボードレイアウトのデフォルトを `"jis"` に変更
- `default_config_text()` を `config/config.toml` と完全同期

### 入力モード初期化（`config.rs`, `state.rs`）
- `DefaultInputMode::Katakana` を廃止（F7 変換は引き続き動作）
- `default_mode = "alphanumeric"` が初回フォーカス時に有効になるよう実装
- `remember_last_kana_mode = false` 時、毎回デフォルトモードを適用するよう実装
- ターミナル（Windows Terminal / ConHost）は config に関わらず常に `Alphanumeric`

### keymap バグ修正（`keymap.rs`）
- `Ctrl+J/K/L` 等が parse できない問題を修正（`name_to_vk` に a-z フォールバックを追加）
- 全角/半角キー（`Zenkaku`）の VK コードを `0xF3` → `0x19`（VK_KANJI）に修正

---

## 動作確認ポイント

```powershell
cargo make build-tsf
cargo make reinstall
```

確認項目:
1. `rakukan.log` に `debug` レベルのログが出ること
2. 一般アプリ初回フォーカス時に Alphanumeric モードで起動すること
3. 別ウィンドウに切り替えて戻ったとき前回モードが復元されること
4. 全角/半角キーで IME オン/オフが切り替わること
5. keymap ロード時に `cannot parse` ワーニングが出ないこと

---

## 既知バグ（未対応）

### B-1: LLM 出力に読み仮名が混入
- 症状: `健診(けんしん)や` のようにルビ形式の読みが付く
- 対策候補: 出力後処理での strip、またはプロンプト側での抑制
- 優先度: 中

---

## 次の実装候補

### 優先度: 高（メイン機能）

#### Phase 4a: LiveConv 基盤
- `SessionState::LiveConv` バリアント追加
- `bg_take_top1` API 実装
- タイマー汎化・config フラグ追加

#### Phase 4b: キーストローク統合
- キーストローク毎の `bg_start`
- `on_live_timer` を `WM_TIMER` 経由で実装
- 重要制約: `RequestEditSession` は `WndProc` から呼べない → `PostMessage` で TSF スレッドに委譲

#### Phase 4c: 仕上げ
- デバウンス処理・ビジュアルポリッシュ・アプリ互換性テスト

### 優先度: 中
- 各モジュールへのログ追加（ライブ変換実装前の整備）
- SplitPreedit 多文節チェーン改善
- LLM 候補数増加（`num_candidates.min(3)` → `.min(5)`、レイテンシ要確認）
- 数字・かな混在入力（`123abc` 混在ケース）

---

## アーキテクチャ早見表

```
rakukan-tsf          TSF レイヤー（Windows IME 登録・UI）
rakukan-engine       変換エンジン（LLM + 辞書）
rakukan-engine-abi   ABI ブリッジ（FFI 境界）
rakukan-dict-builder 辞書ビルド
```

重要制約:
- TSF スレッドからのみ `RequestEditSession` 呼び出し可（WndProc 不可）
- エンジン DLL 変更時は `cargo make build-engine` 必須
- `generate_beam_search_d1_greedy_batch` は `n_batch > n_ctx` で C レベル abort()
- CUDA ランタイム DLL は System32 に配置必須

---

## ファイルパス早見表

| 用途 | パス |
|------|------|
| ソース | `C:\Users\n_fuk\source\rust\rakukan` |
| ビルド成果物 | `C:\rb\release` |
| インストール先 | `%LOCALAPPDATA%\rakukan\` |
| config.toml | `%APPDATA%\rakukan\config.toml` |
| keymap.toml | `%APPDATA%\rakukan\keymap.toml` |
| ログ | `%LOCALAPPDATA%\rakukan\rakukan.log` |
| 辞書 | `%APPDATA%\rakukan\dict\rakukan.dict` |
| LLM モデル | `snapshots/main/*.gguf` |

---

## クイックデバッグコマンド

```powershell
# ログ末尾確認
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 20

# 再インストール（TSF のみ）
cargo make reinstall

# エンジン DLL 変更後
cargo make build-engine
cargo make reinstall
```
