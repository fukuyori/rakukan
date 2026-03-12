# rakukan handoff — 2026-03-12

## 現在のバージョン
v0.3.1（`VERSION` / `CHANGELOG.md` 更新済み）

## プロジェクト構成

| パス | 役割 |
|------|------|
| `crates/rakukan-engine/src/conv_cache.rs` | BG変換キャッシュ（状態機械＋Condvar） |
| `crates/rakukan-engine/src/lib.rs` | エンジン本体（`bg_start` / `bg_reclaim` / `merge_candidates`） |
| `crates/rakukan-engine-abi/src/lib.rs` | エンジン DLL FFI インターフェース |
| `crates/rakukan-tsf/src/tsf/factory.rs` | TSF メインロジック（キーハンドリング・変換フロー） |
| `crates/rakukan-tsf/src/tsf/candidate_window.rs` | 候補ウィンドウ（GDI描画 + WM_TIMERポーリング） |
| `crates/rakukan-tsf/src/engine/state.rs` | SessionState 定義 |

## ビルドコマンド

```powershell
# conv_cache.rs / lib.rs 変更時（エンジン DLL 再ビルドが必要）
cargo make build-engine
cargo make reinstall

# factory.rs / candidate_window.rs / state.rs のみ変更時
cargo make reinstall
```

> **注意**: エンジン DLL の変更を `reinstall` だけで反映しようとすると
> IME がランゲージバーで選択不能になる。FFI/vtable 変更は必ず `build-engine` から。

## 今セッションで修正したバグ（v0.3.1）

### [FIX] bg_start 直後のレース条件（conv_cache.rs）
**症状**: 長い文章や新しい文章を変換すると LLM 変換が失敗し辞書候補のみ表示  
**原因**: `bg_start()` は `pending` に積んで即リターンするため呼び出し直後は
`State::Idle && pending=Some`。`wait_done_timeout` はこの状態を「変換未起動」と
誤判定して即 `false` を返していた。  
**修正**:
- `wait_done_timeout`: `Idle && pending=Some` でも Condvar で待機
- `worker_loop`: `pending → Running` 遷移時に `notify_all` を追加

### [FIX] 前変換の converter 貸し出し中に新変換を開始した場合（factory.rs）
**症状**: 連続して異なる文章を変換すると 1 回目が失敗  
**原因**: 前の変換の converter が conv_cache に貸し出されたまま → `kanji_ready=false`
→ `bg_start` がスキップ → タイマーに任せるが前変換のキーで mismatch  
**修正**: `kanji_ready=false && bg=running` の場合、前 bg の完了を待って
`bg_reclaim` し新しいキーで `bg_start` → `bg_wait_ms`

### [FIX] bg_take_candidates キー不一致後の再試行（factory.rs）
**症状**: 変換結果が 1 候補（辞書のみ）になる  
**原因**: `bg_take_candidates` が `preedit_display()` をキーとして使うが
`bg_start` のキーは `hiragana_buf` → 不一致で None  
**修正**: `hiragana_text()` を優先キーとして使用。不一致の場合は
`bg_reclaim → bg_start → bg_wait_ms` でブロッキング再試行

### [FIX] 文節確定後 remainder が二重表示（factory.rs）
**症状**: 「ほんじつは」を確定すると「本日はいいてんきです」の後に「いいてんきです」が追加表示  
**原因**: `commit_then_start_composition` の `EndComposition` が
composition 全体（確定部 + remainder）をコミットした後に
新 composition で remainder を再追加していた  
**修正**:
- `EndComposition` 前に `SetText(commit_text)` で composition を確定テキストのみに縮小
- `EndComposition` 後の挿入点を `ctx.GetEnd(ec)` に変更（`get_cursor_range` が先頭を返す問題を回避）

### [FIX] LLM_WAIT_MAX_MS タイムアウト（factory.rs）
**症状**: 長い文章（10 文字以上）で変換が失敗  
**原因**: 固定 3000ms が LLM 推論時間を下回る場合があった  
**修正**: `基本 3 秒 + 1 文字 × 300ms、上限 15 秒` で動的に設定

## 現在の on_convert フロー（完成版）

```
Space押下
  │
  ├─ [bg=idle, kanji_ready=true]
  │     bg_start → bg_wait_ms(動的) → 完了
  │         └─ bg_take_candidates(hira_key)
  │               Some → merge → activate_selecting → 表示
  │               None → bg_reclaim → bg_start → bg_wait_ms → retry
  │
  ├─ [bg=running, kanji_ready=false]  ← 前変換の conv 貸し出し中
  │     prev bg_wait_ms → bg_reclaim → bg_start → bg_wait_ms(動的)
  │         └─ 同上
  │
  └─ bg_wait_ms タイムアウト
        WM_TIMER(80ms ポーリング) にフォールバック
        → 候補ウィンドウのみ自動更新（composition は次のキー入力で更新）
```

## 次に取り組む候補

- [ ] ライブ変換（ライブ変換）: 入力中にリアルタイムで変換候補を更新
  - MS-IME / Google IME にない Windows 独自機能になり得る差別化ポイント
  - 実装コスト大（キー入力のたびに bg_start → WM_TIMER 更新が必要）
- [ ] SplitPreedit の複数文節連続変換（「本日は」確定後に「いいてんき」を自動変換開始）
- [ ] WM_TIMER フォールバック時の composition text 更新（現状は次のキー入力まで遅延）

## ログ収集コマンド

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 80 |
  Select-String "on_convert|bg_wait|completed|MISMATCH|kanji_ready|waiting|update_comp"
```
