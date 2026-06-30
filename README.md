# rakukan v0.9.12

> ⚠️ **注意：現在テスト動作中です**
>
> rakukan は開発途中のソフトウェアです。インストールによって **Windows の動作が不安定になる可能性があります**。
> ライブ変換は、非常にクセのある動きが見られ、現在まだバグが残っているので使用には我慢が必要になります。
> TSF（Text Services Framework）DLL をシステムに登録するため、インストール・アンインストールの操作は
> **自己責任** で行ってください。重要な作業環境への適用は推奨しません。

Windows 向け日本語 IME。  
[karukan](https://github.com/togatoga/karukan) の LLM ベース変換エンジンを中核とし、
[azooKey-Windows](https://github.com/fkunn1326/azooKey-Windows) の TSF 層実装を参考に構築しています。

rakukan は、ローカルで動く小型 LLM と Mozc 系辞書を組み合わせ、従来のかな漢字変換とは少し違う候補の出し方を試すための実験的な IME です。入力中の読みから候補を先読みするライブ変換、数字やアルファベットを壊さない literal 保護、ユーザー辞書・学習履歴による候補の優先順位調整を中心にしています。

設計上の大きな特徴は、TSF DLL と変換エンジンを別プロセスに分けていることです。Windows の入力フレームワーク側には軽いクライアントだけを置き、LLM や GPU バックエンドは `rakukan-engine-host.exe` 側で管理します。これにより、CPU / Vulkan / CUDA の engine DLL を設定で切り替えながら、IME 側の安定性をできるだけ保つ構成にしています。

現時点では、日常利用向けの完成品というより、LLM 変換・ライブ変換・Windows TSF 実装を実機で検証するためのプロトタイプです。挙動を観察しながら改善していく前提で使ってください。

## 主な機能

- **ライブ変換**: ひらがな入力後、短い停止でトップ候補を自動表示
- **範囲指定変換**: `Shift+Right/Left` で先頭から変換範囲を指定 → `Space` で変換 → `Enter` で確定、残りで LiveConv 再開
- **区読点分割変換**: `、` `。` や全角記号 `（）～` 、和文記号 `「」・` など記号を含む読みを入力すると記号位置でブロックへ自動分割。`Space` で各ブロックを変換 → `Enter` でブロックを順番に確定。候補ウィンドウは確定のたびに次のブロック直下へ追従
- **数値保護**: LLM が数字を改変しない（`2024ねん → 2024年`）。数字・アルファベットは半角/全角の両方を候補として提示
- **LLM + 辞書変換**: jinen モデルと Mozc 系辞書を併用
- **ユーザー辞書学習**: 確定した変換結果を即時反映
- **文字種変換**: `F6`〜`F10` でひらがな・カタカナ・英数を往復
- **GPU アクセラレーション**: CUDA / Vulkan バックエンド対応
- **out-of-process 構成**: TSF DLL と engine-host を分離し、GPU リソースや LLM 実行をホストプロセス側で管理

## 最新の変更

v0.9.12 では以下の変更が入りました。

- **F9/F10 の記号変換修正**: かな入力で入った `、。・ー` を、F9 では `，．／－`、F10 では `,./-` に変換するよう修正した。
- **中点と長音符の英数変換**: F10 で `・` が `/` に戻らない問題を修正し、長音符 `ー` は半角ハイフン `-` / 全角ハイフン `－` へ変換するようにした。

過去の変更履歴は [CHANGELOG.md](CHANGELOG.md) を参照してください。

## インストール

ビルド → 署名 → インストールを **4 ステップ** に分離しています:

```powershell
# 初回: esaxx-rs パッチのセットアップ
cargo fetch
.\scripts\setup-esaxx-patch.ps1

# ① engine DLL をビルド (cpu/vulkan/cuda)
cargo make build-engine

# ② tsf + tray + host + dict-builder + WinUI settings をビルド
cargo make build-tsf

# ③ 電子署名 (任意; 配布用)
cargo make sign

# ④ %LOCALAPPDATA%\rakukan\ にコピー + TSF 登録 + tray 起動 (★管理者権限)
cargo make install
```

まとめ実行:

```powershell
# ①〜④ を一括 (リリース向け)
cargo make full-install

# 開発時の高速再インストール (engine 使いまわし、署名なし)
cargo make quick-install
```

インストール先: `%LOCALAPPDATA%\rakukan\`  
設定: `%APPDATA%\rakukan\config.toml`  
ログ:

- TSF 側: `%LOCALAPPDATA%\rakukan\rakukan.log`
- エンジンホスト側: `%LOCALAPPDATA%\rakukan\rakukan-engine-host.log`

> 各ステップはそれぞれ独立に実行できます。ビルド (`build-engine` / `build-tsf`) は管理者不要、`install` のみ管理者権限が必要です。

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
| ---- | ---- |
| Space / 変換 | 変換開始 / 次候補 / 選択中分節の再変換 |
| Enter | 表示中の内容を確定（区読点分割変換中はブロックを順番に確定） |
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

> **区読点分割変換について**: 読みに `、` `。` `！` `？` などの区読点・記号（全角記号 `（）～` / ASCII 記号 `@#()` / 和文記号 `「」・` など）が含まれると自動的にブロック分割変換へ移行します。Space でブロック内の候補を選択し、Enter でそのブロックを確定して次のブロックへ進みます。全ブロック確定時に学習が行われます。

## 開発メモ

- TSF 層だけの変更確認: `cargo make quick-install` (= `build-tsf` + `install`)
- engine DLL を含む変更確認: `cargo make build-engine` → `cargo make quick-install`
- 同梱 Vibrato 辞書: `assets/vibrato/system.dic`
- 生成ログ確認:

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 40
```

## 課題リスト

### 主要設計書

- [DESIGN.md](docs/DESIGN.md) — v0.4.4 時点の全体設計書（クレート構成・RPC プロトコル・スレッドモデル・辞書システムなど）
- [handoff.md](docs/handoff.md) — v0.9.3 引き継ぎ資料 + 残タスクリスト

### 独立した技術課題

- [ ] `rakukan-engine-host.exe` の idle 自死（長時間アイドル時のメモリ解放）
- [ ] ホストプロセスのヘルスチェックとクラッシュカウント
- [ ] Preedit / LiveConv / Selecting の display_attr 拡張

### 過去のスナップショット

v0.2.0 の状態を記録した以下の資料は **過去のスナップショット** であり、現在進行中のタスクではありません。

- [PHASE1_SUMMARY.md](docs/archive/PHASE1_SUMMARY.md) — v0.2.0 時点の Phase 1 要約
- [PHASE2_PREP.md](docs/archive/PHASE2_PREP.md) — v0.2.0 先行の Phase 2 着手前メモ
- [PHASE2_STATUS.md](docs/archive/PHASE2_STATUS.md) — v0.2.0 時点の Phase 2 状況
- [WARNING_FIXES.md](docs/archive/WARNING_FIXES.md) — v0.2.0 に含まれる warning 修正メモ

## ライセンス

rakukan 本体のコードは **MIT ライセンス** です。  
辞書・モデルなどの同梱物や取得物には、それぞれ個別のライセンス条件が適用されます。
