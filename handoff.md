# Rakukan 引き継ぎ資料 (v0.4.1)

更新日: 2026-03-28

## 現在の状態

- **バージョン:** v0.4.1
- **位置づけ:** ライブ変換 Phase 1 と分節再変換 UI が実用段階
- **ソース:** `C:\Users\n_fuk\source\rust\rakukan`
- **インストール先:** `%LOCALAPPDATA%\rakukan\`
- **設定:** `%APPDATA%\rakukan\config.toml`
- **ログ:** `%LOCALAPPDATA%\rakukan\rakukan.log`

## 0.4.1 時点でできていること

### ライブ変換

- ひらがな入力後、短い停止でトップ候補を自動表示
- `Enter` でライブ変換結果をそのまま確定
- `Space` で通常の再変換操作へ移行
- `F6` 後の `Enter` で元の漢字に戻る問題は解消済み

### 分節再変換

- `Space` 後に分節選択へ入れる
- `Left/Right` で選択文節を移動
- `Shift+Left/Right` で選択範囲を縮小・拡張
- 選択中の文節だけ再変換し、前後の表示は保持
- `Enter` で全文を安定して確定

### 分節境界

- Phase 1 として `Vibrato` を導入済み
- `engine_segment_surface` を engine / ABI / TSF に追加済み
- 同梱辞書は `assets/vibrato/system.dic`
- 辞書が無い場合は従来ヒューリスティックへフォールバック

### 開発運用

- engine ABI バージョンチェックあり
- 古い engine DLL を読んだ場合、更新漏れがログで分かる

## 主な変更ファイル

- `crates/rakukan-tsf/src/engine/state.rs`
  - `LiveConv`
  - `SplitPreedit` の分節選択状態
- `crates/rakukan-tsf/src/tsf/factory.rs`
  - ライブ変換遷移
  - 分節移動・拡張・再変換・確定
- `crates/rakukan-tsf/src/tsf/candidate_window.rs`
  - ライブ変換タイマー
- `crates/rakukan-engine/src/segmenter.rs`
  - Vibrato 分節補助
- `crates/rakukan-engine/src/ffi.rs`
- `crates/rakukan-engine-abi/src/lib.rs`

## 確認コマンド

```powershell
# TSF 層のみビルド
cargo make build-tsf

# engine DLL を含むビルド
cargo make build-engine

# 実機反映
cargo make reinstall

# ログ確認
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 40
```

## 現在の確認ポイント

1. ライブ変換表示が自然か
2. `Space` 後の分節移動が `Left/Right` で素直に動くか
3. `Shift+Left/Right` で範囲調整できるか
4. 再変換後も状態が崩れないか
5. `Enter` で全文が安定確定するか

## 残タスク

### 優先度: 高

- **[Live-2] display_attr 拡張**
  - Preedit / LiveConv / Selecting の見た目をさらに分ける

- **[Live-3] config フラグ整理**
  - `live_conversion.enabled`
  - `live_conversion.debounce_ms`

- **[Num-1] 数字プレースホルダ応急処置**
  - `200えん -> 2000円` 系の誤変換抑制

### 優先度: 中

- **速度改善**
  - 現状の体感遅延は分節化そのものより `Space` 時の同期待ちが主因
  - `bg_wait_ms(...)` の待ち方見直しや `peek` 系 API が本命

- **長文・句読点混じりでの分節精度確認**

- **`bg_peek_top1` 系 API**
  - Space 後の再取得コストを下げる候補

### 優先度: 低

- **Segment ベースの本格文節管理**
  - TSF 推定を減らし、engine 主導へ寄せる

- **数字・助数詞の構造対応**

## 補足

- TSF だけ変えた場合は `build-tsf` でよい
- engine / ABI を変えた場合は `build-engine` と `reinstall` が必要
- 公開 API だけで「標準 IME トレイの隣に必ず表示」は難しく、今後の課題として保留
