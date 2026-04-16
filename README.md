# rakukan v0.5.1

> ⚠️ **注意：現在テスト動作中です**
>
> rakukan は開発途中のソフトウェアです。インストールによって **Windows の動作が不安定になる可能性があります**。
> TSF（Text Services Framework）DLL をシステムに登録するため、インストール・アンインストールの操作は
> **自己責任** で行ってください。重要な作業環境への適用は推奨しません。

Windows 向け日本語 IME。  
[karukan](https://github.com/togatoga/karukan) の LLM ベース変換エンジンを中核とし、
[azooKey-Windows](https://github.com/fkunn1326/azooKey-Windows) の TSF 層実装を参考に構築しています。

## 主な機能

- **ライブ変換**: ひらがな入力後、短い停止でトップ候補を自動表示
- **範囲指定変換**: `Shift+Right/Left` で先頭から変換範囲を指定 → `Space` で変換 → `Enter` で確定、残りで LiveConv 再開
- **数値保護**: LLM が数字を改変しない（`2024ねん → 2024年`）。数字・アルファベットは半角/全角の両方を候補として提示
- **LLM + 辞書変換**: jinen モデルと Mozc 系辞書を併用
- **ユーザー辞書学習**: 確定した変換結果を即時反映
- **文字種変換**: `F6`〜`F10` でひらがな・カタカナ・英数を往復
- **GPU アクセラレーション**: CUDA / Vulkan バックエンド対応

## 0.5.0 変更点

- **数値保護レイヤー**: LLM が数字を改変する問題を根本解決。reading を数字ラン / 非数字ランに分割し、LLM には非数字部分だけを渡す
- **アルファベット保護**: 半角/全角の両方を候補として提示
- **範囲指定変換 (RangeSelect)**: `Shift+矢印` で先頭から変換範囲を指定する新しい部分変換方式。分節アライメント問題が発生しない
- **数字入力の半角/全角設定**: `config.toml` の `[input] digit_width = "halfwidth"` で制御（デフォルト: 半角）
- **vibrato / SplitPreedit の完全削除**: 形態素解析ベースの分節分割を廃止し、reading 文字位置ベースの範囲指定に置換

> 0.5.0 では engine ABI が v7 に更新されています。`cargo make build-engine && cargo make reinstall` で全コンポーネントを更新してください。

## 0.4.4 変更点

- **エンジンを別プロセス化**: `rakukan_engine_*.dll`（llama.cpp 同梱）は専用の
  `rakukan-engine-host.exe` でのみロードし、TSF DLL 側は Named Pipe + postcard で
  RPC するクライアントになりました。これにより Zoom / Dropbox などで発生していた
  `msvcp140.dll` クロスロード起因のクラッシュを根本解決しています。
- Activate 時には engine DLL に一切触れず、最初の入力まで完全に遅延ロード
- IME モード切替時の `config.toml` 自動再反映を out-of-process でも動くように修正
  （`Request::Reload` でホスト側 DynEngine を新 config で作り直す）
- Named Pipe には明示的な DACL を設定（現在ログインユーザー + SYSTEM のみ）
- `rakukan-engine-cli` の既存ビルドエラーを修正

## インストール

```powershell
# 初回: esaxx-rs パッチのセットアップ
cargo fetch
.\scripts\setup-esaxx-patch.ps1

# engine に変更がある場合
cargo make build-engine

# TSF/engine をインストール先へ反映
cargo make reinstall
```

インストール先: `%LOCALAPPDATA%\rakukan\`  
設定: `%APPDATA%\rakukan\config.toml`  
ログ:

- TSF 側: `%LOCALAPPDATA%\rakukan\rakukan.log`
- エンジンホスト側: `%LOCALAPPDATA%\rakukan\rakukan-engine-host.log`

> `cargo make build-tsf` はビルドのみです。実機確認には `cargo make reinstall` を使ってください。

## 設定の目安

`%APPDATA%\rakukan\config.toml` では `model_variant` と `n_gpu_layers` を調整できます。

- `jinen-v1-xsmall-q5` は比較的軽く、`n_gpu_layers = 16` 前後から試しやすい
- `jinen-v1-small-q5` は `n_gpu_layers = 8` か `16` くらいから始めるのが安全
- `n_gpu_layers = 0` は CPU のみ
- 未指定は全レイヤー GPU オフロード

`n_gpu_layers` と `model_variant` は config.toml を編集したあと IME モードを切り替えるだけで即時反映されます（`rakukan-engine-host.exe` 内部の DynEngine が新設定で作り直されます）。

> v0.4.4 より、Zoom / Dropbox 等の他アプリが異常終了する問題は別プロセス化で解消済みです。`n_gpu_layers` を下げる回避策は不要になりました。

## キー操作

| キー | 動作 |
|------|------|
| Space / 変換 | 変換開始 / 次候補 / 選択中分節の再変換 |
| Enter | 表示中の内容を確定 |
| ESC | 変換キャンセル |
| Backspace | 1文字削除 |
| Left / Right | 分節選択の移動 |
| Shift+Left / Shift+Right | 分節選択の縮小 / 拡張 |
| ↑ / ↓ | 候補を前後に移動 |
| 1〜9 | 候補を番号で選択 |
| Tab / PageDown | 次ページ |
| Shift+Tab / PageUp | 前ページ |
| F6 | ひらがな |
| F7 | カタカナ |
| F8 | 半角カタカナ |
| F9 | 全角英数 |
| F10 | 半角英数 |

## 開発メモ

- TSF 層だけの変更確認: `cargo make build-tsf`
- engine DLL を含む変更確認: `cargo make build-engine` → `cargo make reinstall`
- 同梱 Vibrato 辞書: `assets/vibrato/system.dic`
- 生成ログ確認:

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 40
```

## 課題リスト

現在進行中の設計書・残タスクは以下の資料にまとめています。

### 主要設計書

- [DESIGN.md](DESIGN.md) — v0.4.4 時点の全体設計書（クレート構成・RPC プロトコル・スレッドモデル・辞書システムなど）
- [CONVERTER_REDESIGN.md](CONVERTER_REDESIGN.md) — **進行中**: ライブ変換・文節再変換・境界伸縮・数値保護・用法辞書の全面改修設計（Phase A〜F）
- [SEGMENT_EDIT_REDESIGN.md](SEGMENT_EDIT_REDESIGN.md) — 分節編集モデルの基礎設計（`CONVERTER_REDESIGN.md` に継承済み）
- [VIBRATO_PHASE1.md](VIBRATO_PHASE1.md) — Vibrato 形態素解析器の導入メモ
- [handoff.md](handoff.md) — v0.4.4 引き継ぎ資料 + 残タスクリスト

### 進行中の主要課題

**変換パイプライン再設計**（[CONVERTER_REDESIGN.md](CONVERTER_REDESIGN.md)）

- [ ] **Phase A**: 新データモデル（`Segments` / `Segment` / `Candidate`）と engine 基盤、数値保護レイヤー
- [ ] **Phase B**: ライブ変換を beam=1 (greedy) 化、`Segments` 保持
- [ ] **Phase C**: `SplitPreedit` を新モデルに置換、文節ごとの候補管理
- [ ] **Phase D**: 境界伸縮を engine の `resize_segment` に集約、文字列再分節の撤廃
- [ ] **Phase E**: 部分確定・学習・Selecting 統合・候補一覧 Tab 展開
- [ ] **Phase F**（独立）: Candidate 注釈（用法辞書） — Mozc `usage_dict.tsv` の取り込み

**独立した技術課題**（[handoff.md §残タスク](handoff.md#残タスク優先度順)）

- [ ] `rakukan-engine-host.exe` の idle 自死（長時間アイドル時のメモリ解放）
- [ ] ホストプロセスのヘルスチェックとクラッシュカウント
- [ ] Preedit / LiveConv / Selecting の display_attr 拡張
- [ ] RPC レイテンシの実測（0.4.5 バッチ化後の計測）

### 過去のスナップショット

v0.2.0 の状態を記録した以下の資料は **過去のスナップショット** であり、現在進行中のタスクではありません。

- [PHASE1_SUMMARY.md](PHASE1_SUMMARY.md) — v0.2.0 時点の Phase 1 要約
- [PHASE2_PREP.md](PHASE2_PREP.md) — v0.2.0 先行の Phase 2 着手前メモ
- [PHASE2_STATUS.md](PHASE2_STATUS.md) — v0.2.0 時点の Phase 2 状況
- [WARNING_FIXES.md](WARNING_FIXES.md) — v0.2.0 に含まれる warning 修正メモ

## ライセンス

rakukan 本体のコードは **MIT ライセンス** です。  
辞書・モデルなどの同梱物や取得物には、それぞれ個別のライセンス条件が適用されます。
