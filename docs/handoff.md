# Rakukan 引き継ぎ資料

更新日: 2026-09-17（前版は v0.9.12 / 2026-06-24 時点。履歴は git と [CHANGELOG.md](../CHANGELOG.md) を参照）

この文書は「次のセッションが何を知っていれば作業を続けられるか」だけを書く。
設計の全体は [DESIGN.md](DESIGN.md)、9 月の作業記録と判断の経緯は [September_Revised_Plan.md](September_Revised_Plan.md) にある。

## 1. 現在の状態（2026-09-17、GitHub で確認）

| 項目 | 値 |
|---|---|
| main | `e6415cc`（origin と一致、作業ツリーはクリーン） |
| 最新リリース | **0.11.7**（2026-09-14、タグ・GitHub Release あり） |
| 次のリリース番号 | **0.11.9**。0.11.8 は取り消して欠番（タグもリリースも作っていない） |
| engine ABI | 9（`crates/rakukan-engine/src/ffi.rs` と `crates/rakukan-engine-abi/src/lib.rs` の 2 か所） |
| RPC protocol | 5（`crates/rakukan-engine-rpc/src/protocol.rs`） |
| `dict_schema` | 2 |
| 開いている PR | なし |
| CHANGELOG | `[Unreleased]` 節は無い。次のリリース作業で作る |

### 0.11.7 以降に main へ入った未リリースの変更

- **アプリごとの IME 初期状態を config で設定する**（#51 の段 1、PR #52、`8940bb0` / `928e066`）
  - `[input] ime_off_apps`: アプリがアクティブになったら IME をオフにする。既定値は `conhost.exe` / `WindowsTerminal.exe` / `mintty.exe` / `wezterm-gui.exe` / `ghostty.exe`
  - `[input] ime_on_apps`: アクティブになった後、アプリ本体とは別の入力先に入ったときに 1 回だけオンにする。既定値は空
  - どちらも、利用者が IME を操作したらその状態を保ち、インアクティブになったら破棄する。対象アプリでは入力先ごとのモード記憶を使わない
  - ウィンドウクラス名によるターミナル判定（`is_terminal_hwnd`）は削除し、config を唯一の情報源にした
- PR #47（`input.text_field_mode`）のマージと 0.11.8 を revert（`7920e1f`）。理由は計画書の「PR #47 のマージと 0.11.8 を取り消し」節

リリース時の更新対象は [version-update-checklist.md](version-update-checklist.md)。
パッケージはレモンが `-Sign` を付けて作成する（Claude 側では作らない）。

## 2. 作業の進め方

### ビルドとインストール

```powershell
cargo make build-engine
cargo make build-tsf
# サインアウト → サインイン
sudo cargo make install
```

- ビルドとインストールはレモンが行う。進言する前に状態を確認する
- `cargo test` は PowerShell から実行する（Git Bash 経由では ConPTY テストがハングする）
- 型チェックだけなら `cargo make check`、全体テストは `cargo test --workspace --lib`
- `PROTOCOL_VERSION` を変えたリリースでは、古いホストが残っていると新しい TSF が接続できない。サインアウト／サインインで解消する

### ファイルの場所

| 用途 | 場所 |
|---|---|
| インストール先 | `%LOCALAPPDATA%\rakukan\`（`%ProgramFiles%` への移行は #33、10 月予定） |
| 設定 | `%APPDATA%\rakukan\config.toml` / `keymap.toml` / `user_dict.toml` |
| 学習履歴 | `%APPDATA%\rakukan\learn_history.bin` |
| 辞書 | `%LOCALAPPDATA%\rakukan\dict\rakukan.dict`（隣に `rakukan.dict.build.json`） |
| ログ | `%LOCALAPPDATA%\rakukan\rakukan.log`（TSF）/ `rakukan-engine-host.log` / `rakukan-engine-dll.log` |
| 復帰マーカー | `%LOCALAPPDATA%\rakukan\engine-recovery.txt`（`<exited_at_ms> <attempt>`、#43） |

### GitHub

- `origin` = fukuyori/rakukan、`nick` = nick20002005 のフォーク（PR と Issue の主な提案者）
- コミット・リリースの状態を話す前に `git fetch` / `gh` で GitHub 側を確認する（ローカルの追跡参照は古いことがある）
- **コミットメッセージと PR 本文に、完了キーワードと Issue 番号を並べて書かない。** 引用でも GitHub が解釈する。2026-09-15 に #51 がこれで 2 回自動クローズされた。段階的な対応は `Refs` や「#NN の段 1」と書き、Issue は手でクローズする

## 3. 開いている Issue と状態

| Issue | 内容 | 状態 |
|---|---|---|
| #55 | ホストを spawn した後の接続失敗が `HostSpawnGuard` の失敗回数に入らない（`client.rs:651-652` の `?`、0.6.0 から） | 未着手（2026-09-17 起票）。ホストが起動できない状態では抑止が働かず、キー入力の処理中に RPC 1 回あたり spawn 最大 4 回・待ち約 21 秒と見積もり（経路はコードで確認、停止時間は未確認）。#54 を先に直して発生状況を見てから直す案 |
| #54 | RPC クライアント（`rakukan-engine-rpc`）のログが `rakukan.log` に記録されない | 未着手（2026-09-17 起票）。TSF のログ filter が `rakukan_tsf=<level>` だけ（`lib.rs:117-122`）。`rpc SLOW` / `spawn_host failed` など 12 か所が出ない。filter に `rakukan_engine_rpc` を足す案 |
| #53 | 数字と「参考」を同じ変換単位で打つと候補が捨てられる（`参` `拾` を大字 1 文字として数える） | **未読・未回答**（2026-09-15、nick）。原因と回避案（1 文字だけの大字 run を数値にしない）、PR の用意ありと書かれている。大字の扱いの方針判断が要る |
| #51 | アプリごとの IME 初期状態 | 段 1 は main に入った（未リリース）。**段 2 = 設定アプリ（WinUI）での編集画面が残る**。実行中のアプリから選ばせたい。#33 と時期を調整 |
| #50 | 破棄済み DocumentManager のポインタが `dm_modes` に入り直し、アドレス再利用で前のモードが復元される | 未着手。#51 の対象外アプリでは残る |
| #49 | `model_variant` を変えてもホストが続く限り古いモデルのまま | 未着手。`start_load_model` が converter を config と照合しない |
| #46 | 学習履歴を個別に削除する手段が無い | 10 月改修（#33）と合わせる予定 |
| #45 | 速く打つとライブ変換のプレビューが更新されない | nick の PR 待ち |
| #40 | 候補・予測の品質（統合ブランチ側の報告 7 件） | 1・3 は再現せず、7 は #45、2 は縮小版の PR 待ち、4 は見送り |
| #35 | 区読点だけの読みとリーダー記号 | 判断待ち。[Symbol_Leader_Input_Plan.md](Symbol_Leader_Input_Plan.md)。PR #31 は保留 |
| #33 | インストーラー再設計（`%ProgramFiles%` 移行、更新は実行 → 再起動で完了） | **10 月改修**。[October_Install_Plan.md](October_Install_Plan.md) |
| #32 | 文節変換 | 判断待ち。[Segment_Edit_Plan.md](Segment_Edit_Plan.md) |
| #29 | ユーザー辞書の `priority = "low"` | 保留 |
| #18 | 統合ブランチの棚卸し | 各項目は個別 Issue へ分割済み。記録として残している |
| #16 | 数字混在の読みで、かな run が辞書を参照できない | 判断待ち。`はんは` のように助詞を含む読みも辞書に届かない（2026-09-14 確認） |

計画書のステップ表（`September_Revised_Plan.md` 末尾）では、Step 10・12・13 は完了。残りは Step 14（リリースと再計測）と 10 月改修。

## 4. 実機で未確認の項目

| 項目 | 補足 |
|---|---|
| `ime_off_apps` の既定値 `conhost.exe` / `WindowsTerminal.exe` / `mintty.exe` | 一致したログの記録が無い。wezterm-gui.exe / ghostty.exe は確認済み |
| `ime_on_apps` で Photoshop の文字ツール | 環境が無い。firefox / msedge / chrome は確認済み |
| Step 13-1「モデルが ready になったら自動で変換をやり直す」 | モデル読み込みが 157ms で終わり手で狙えない。コールドスタート時に確認する |
| Step 13-2 `edit_session: SLOW grant` | 閾値を超えたときだけ出る（0 件は遅延が起きていないことを示す） |
| Step 13-2 `rpc SLOW` | **記録されない状態**（#54）。`rakukan_engine_rpc` の行は 6 世代で 0 件で、0 件は遅延が無いことを示さない。#54 の修正後に確認する |

## 5. 既知の問題（Issue 化していないもの）

- **起動時に config のパースに失敗すると、警告なしで全設定が既定値に戻る**（`ConfigManager::new` / `init_config_manager` の `unwrap_or_default()`）。IME 切替時の再読込は警告を出して前の設定を保つので、挙動が非対称
- **config の再読込がアプリごとにばらばら**。reload イベント `Local\rakukan.engine.reload` が auto-reset のため、1 回の Set で 1 プロセスにしか届かない
- 入力先ごとのモード記憶で、手動変更の即時保存（`doc_mode_remember_current`）は `trace!`、`DOC_MODE_STORE.try_lock()` の失敗は無言で抜けるため、この 2 経路はログから確認できない
- ブラウザで「入力項目が変わると英数に変わる」という症状は、調査を途中で打ち切った（`default=` のイベント 42 件を見た段階で、原因は未特定）

## 6. 検証と調査の知見

- **モデル未準備の状態を作る**: `model_variant` を存在しない値にする。その後ホストを終了する（#49 のため config を変えただけではモデルが切り替わらない）
- **推論失敗を作る**: `[diagnostics] force_inference_failure = true`。converter に焼き込まれるので、有効化にはホストの作り直しが要る
- **全アプリに config を反映させる**: reload イベントを連続で Set する（上記の auto-reset のため）
- **エンジン DLL のログレベル**は `config.toml` の `log_level` に追随する（ホスト spawn 時に `RAKUKAN_LOG` として渡す。環境変数を明示していればそちらが優先）
- **ホストの健全性**: 連続 3 回の推論失敗でホストが自己終了し、5 分以内に 2 回自己終了した後にまた失敗すると `unrecoverable`。判断はホスト側（`crates/rakukan-engine-rpc/src/health.rs`）
- **辞書・モデルの注入**はホストの `dispatch_engine` 冒頭で毎回試みる。TSF のラッチやホストが入れ替わった理由に依存しない
- Git Bash のヒアドキュメントで Python にファイルを書かせると `\\` が潰れ、Windows パス中の `\r` が CR になる。書いた後に CR の混入を確認する
- 過去の Issue コメントのうち Claude が起案したもの（例: #18 の 9/4 返信）を、レモンの決定として扱わない

## 7. 関連資料

| 資料 | 内容 |
|---|---|
| [DESIGN.md](DESIGN.md) | 全体設計（プロセス構成、RPC、ホストのライフサイクル、設定、辞書） |
| [September_Revised_Plan.md](September_Revised_Plan.md) | 9 月の実行計画と、Step ごと・リリースごとの記録 |
| [October_Install_Plan.md](October_Install_Plan.md) | 10 月改修（#33） |
| [Segment_Edit_Plan.md](Segment_Edit_Plan.md) | 文節変換の論点（#32） |
| [Symbol_Leader_Input_Plan.md](Symbol_Leader_Input_Plan.md) | リーダー記号の検討（#35） |
| [Step10_Romaji_Rebuild_Plan.md](Step10_Romaji_Rebuild_Plan.md) | ローマ字入力の再構築（完了） |
| [version-update-checklist.md](version-update-checklist.md) | バージョン変更で更新するファイル |
| [GPU_MEMORY_LIFECYCLE.md](GPU_MEMORY_LIFECYCLE.md) | ホスト多重起動時の GPU メモリ（「GPU 浪費」と論じない根拠） |
| [EXPLORER_CRASH_HISTORY.md](EXPLORER_CRASH_HISTORY.md) / [INVESTIGATION_GUIDE.md](INVESTIGATION_GUIDE.md) | Explorer 異常終了の対策年表と dump 解析の手順（前版の「既知の問題」はこちらに集約されている） |
| [archive/](archive/README.md) | 完了・破棄した計画書 |
