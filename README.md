# rakukan v0.11.3

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
- **GPU アクセラレーション**: CUDA / Vulkan バックエンド対応。CUDA 版 DLL は CUDA ランタイムが別途必要（無い環境では `gpu_backend = "auto"` が Vulkan / CPU へ自動で切り替わる）
- **out-of-process 構成**: TSF DLL と engine-host を分離し、GPU リソースや LLM 実行をホストプロセス側で管理

## 最新の変更

v0.11.3 は候補ウィンドウのフォントサイズ変更に対応したリリースです（[Issue #3](https://github.com/fukuyori/rakukan/issues/3) / [PR #5](https://github.com/fukuyori/rakukan/pull/5)）。`config.toml` の `[appearance]` `candidate_font_height`（10〜72px、既定 17）または設定アプリの「候補表示」から変更でき、行間や余白も同じ比率で拡大されます。画面に収まらない場合は自動で縮小されます。あわせて、設定の保存が一部のアプリの IME に反映されないことがある問題を修正しました。

- v0.11.2: **数字とかなが混在する読みの変換品質を修正**。「5まん」が「5満」「5マン」になり「5万」が候補に出ない問題（Issue #6 / PR #7）を修正。

- v0.11.1: **CI 崩壊の解消（メンテナンス、挙動変更なし）**。ツールチェーンの更新（rustfmt 1.9.0 / 新しい clippy）で main の CI が落ち、すべての PR のチェックが赤くなっていた問題（Issue #14）を修正。

- v0.11.0: **8月の運用ログと GitHub Issue にもとづく修正のまとめ**。Space 変換でユーザー辞書・学習履歴が反映されない問題（Issue #9）、JIS 配列の半角/全角キー（Issue #1）、`gpu_backend = "auto"` の Vulkan / CPU への自動切替（Issue #2）、変換中の Home / End の素通し（Issue #11）、ライブ変換プレビューのかな表示への巻き戻り、変換済みカタカナ語を含む文の文脈破棄、モード切替時の keymap 同期再読込によるキーストールを修正。host / engine DLL の別ビルド検出ログ（Issue #8）を追加。

- v0.10.4: **語彙外文字（Ψ・€・絵文字など）が変換候補から消える問題を修正**。jinen v2 のバイトフォールバックトークンがデコード時に破棄されていたもので、「さいきくすおのさいなん」→「斉木楠雄のΨ難」が正しく出るようになった。
- v0.10.3: **jinen-v2 モデル（Qwen3 ベース）を追加**。`config.toml` の `model_variant` を `jinen-v2-xsmall-q5`（約 28 MB）/ `jinen-v2-small-q5`（約 81 MB）などに書き換えるだけで切り替え可能（f16 variant もあり。デフォルトは v1 のまま）。
- v0.10.2: **確定テキスト消失を修正**（TS_E_READONLY 時の再試行と表示中テキストのままの確定）、**無駄なバックグラウンド変換を削減**（末尾が未確定ローマ字の間はライブ変換を起動しない）。
- v0.10.1: **echo strip（context 汚染対策）の誤爆を削減**。エコー源判定を「8 文字以上のかな連続 run」に絞り、除去も該当文のみに限定。ひらがなのみの確定文は最初から文脈に入れない。
- v0.10.0: **エンジン（LLM モデル）の二重ロード乱発を修正**（7月の運用ログで月 800 回発生）。

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
- エンジンホスト側: `%LOCALAPPDATA%\rakukan\rakukan-engine-host.log`（起動時に host / engine DLL の version・git sha を記録し、別ビルドの組み合わせなら WARN）
- エンジン DLL 側: `%LOCALAPPDATA%\rakukan\rakukan-engine-dll.log`（辞書ロード失敗 `dict load failed at [...]` や LLM 変換の警告はこちらに出る）

> 各ステップはそれぞれ独立に実行できます。ビルド (`build-engine` / `build-tsf`) は管理者不要、`install` のみ管理者権限が必要です。

## 設定の目安

`%APPDATA%\rakukan\config.toml` では `model_variant` と `n_gpu_layers` を調整できます。

- `jinen-v1-xsmall-q5` は比較的軽く、`n_gpu_layers = 16` 前後から試しやすい
- `jinen-v1-small-q5` は `n_gpu_layers = 8` か `16` くらいから始めるのが安全
- `jinen-v2-xsmall-q5` / `jinen-v2-small-q5` は v2 世代（Qwen3 ベース）。プロンプト形式は v1 と共通で、`model_variant` を書き換えるだけで切り替えられる
- `n_gpu_layers = 0` は CPU のみ
- 未指定は全レイヤー GPU オフロード
- `gpu_backend = "auto"`（既定）は cuda → vulkan → cpu の順に実際にロードを試みる。`cuda` / `vulkan` / `cpu` を明示した場合はその DLL だけを使い、失敗しても他へ切り替えない（結果は `rakukan-engine-host.log` に出る）

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
| Home / End | 変換中は IME が受け取り、アプリ側のキャレットを動かさない（範囲指定中は選択範囲を先頭 / 末尾へ） |
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
