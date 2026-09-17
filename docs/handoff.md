# Rakukan 引き継ぎ資料

更新日: 2026-09-17（同日の前版は main `e6415cc` 時点。履歴は git と [CHANGELOG.md](../CHANGELOG.md) を参照）

この文書は「次のセッションが何を知っていれば作業を続けられるか」だけを書く。
設計の全体は [DESIGN.md](DESIGN.md)、現在の作業計画と判断の経緯は [September_Late_Plan.md](September_Late_Plan.md)、9 月前半の作業記録は [September_Revised_Plan.md](September_Revised_Plan.md) にある。

## 1. 現在の状態（2026-09-17、GitHub で確認）

| 項目 | 値 |
|---|---|
| main | `ba96e7e`（2026-09-17T04:35Z 時点で origin と一致、作業ツリーはクリーン。この引継書の更新はその後のコミット） |
| 最新リリース | **0.11.7**（2026-09-14、タグ・GitHub Release あり） |
| 次のリリース番号 | **0.11.9**。0.11.8 は取り消して欠番（タグもリリースも作っていない） |
| engine ABI | 9（`crates/rakukan-engine/src/ffi.rs` と `crates/rakukan-engine-abi/src/lib.rs` の 2 か所） |
| RPC protocol | 5（`crates/rakukan-engine-rpc/src/protocol.rs`） |
| `dict_schema` | 2 |
| 開いている PR | なし |
| CHANGELOG | `[Unreleased]` 節は無い。次のリリース作業で作る |
| 作業計画 | [September_Late_Plan.md](September_Late_Plan.md)（段 1〜4。判断事項 J-1〜J-9、9 節に実施記録） |

### 0.11.7 以降に main へ入った未リリースの変更

- **アプリごとの IME 初期状態を config で設定する**（#51 の段 1、PR #52、`8940bb0` / `928e066`）
  - `[input] ime_off_apps`: アプリがアクティブになったら IME をオフにする。既定値は `conhost.exe` / `WindowsTerminal.exe` / `mintty.exe` / `wezterm-gui.exe` / `ghostty.exe`
  - `[input] ime_on_apps`: アクティブになった後、アプリ本体とは別の入力先に入ったときに 1 回だけオンにする。既定値は空
  - どちらも、利用者が IME を操作したらその状態を保ち、インアクティブになったら破棄する。対象アプリでは入力先ごとのモード記憶を使わない
  - ウィンドウクラス名によるターミナル判定（`is_terminal_hwnd`）は削除し、config を唯一の情報源にした
- **RPC クライアントのログを `rakukan.log` に記録する**（#54、`a981b07`）
  - TSF のログ filter を `LOG_TARGETS`（`rakukan_tsf` / `rakukan_engine_rpc`）にまとめ、ファイル出力と出力先が無い場合で同じ filter を使う。`RAKUKAN_LOG` の優先は維持
  - これで `rpc SLOW`、`spawn_host failed`、`host spawn suppressed`、`Shutdown` の応答などが記録される
- PR #47（`input.text_field_mode`）のマージと 0.11.8 を revert（`7920e1f`）。理由は `September_Revised_Plan.md` の「PR #47 のマージと 0.11.8 を取り消し」節

このほかの 0.11.7 以降のコミットは、計画書・引継書などの文書だけ。

リリース時の更新対象は [version-update-checklist.md](version-update-checklist.md)。
パッケージはレモンが `-Sign` を付けて作成する（Claude 側では作らない）。

### 作業計画の進み具合（September_Late_Plan.md）

| 段 | 内容 | 状態 |
|---|---|---|
| 1 | #53 の影響確認と方針回答、#54 のログ修正、#45 は PR 待ち | #54 はコミット済み・実ログ出力も確認済みで、**1 日分のログ量の確認だけ残る**。#53 は方針を決めて nick に PR を依頼済み。#45 は PR 待ち |
| 2 | 0.11.9 リリースと 1 週間の再計測（Step 14） | 未着手 |
| 3 | 状態の正しさに関わる修正（#55 / #50 / reload / #49 / config 読込失敗） | 未着手 |
| 4 | 判断済みの機能（#35 本線 / #16）と nick の PR 受け入れ（#45 / #40-2 / #53） | #53 の PR を依頼済み。ほかは判断待ち・PR 待ち |

判断事項 J-1〜J-9 のうち、決定済みは J-1（#53。方式と PR の依頼まで）。J-2（#45 は依頼済みの PR を待つ）ほか J-3〜J-9 は、計画書の推奨のままレモンの判断待ち。

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
| インストール先 | `%LOCALAPPDATA%\rakukan\`（`%ProgramFiles%` への移行は #33） |
| 設定 | `%APPDATA%\rakukan\config.toml` / `keymap.toml` / `user_dict.toml` |
| 学習履歴 | `%APPDATA%\rakukan\learn_history.bin` |
| 辞書 | `%LOCALAPPDATA%\rakukan\dict\rakukan.dict`（隣に `rakukan.dict.build.json`） |
| ログ | `%LOCALAPPDATA%\rakukan\rakukan.log`（TSF）/ `rakukan-engine-host.log` / `rakukan-engine-dll.log` |
| 復帰マーカー | `%LOCALAPPDATA%\rakukan\engine-recovery.txt`（`<exited_at_ms> <attempt>`、#43） |

### GitHub

- `origin` = fukuyori/rakukan、`nick` = nick20002005 のフォーク（PR と Issue の主な提案者）
- コミット・リリースの状態を話す前に `git fetch` / `gh` で GitHub 側を確認する（ローカルの追跡参照は古いことがある）
- **コミットメッセージと PR 本文に、完了キーワードと Issue 番号を並べて書かない。** 引用でも GitHub が解釈する。2026-09-15 に #51 がこれで 2 回自動クローズされた。段階的な対応は `Refs` や「#NN の段 1」と書き、Issue は手でクローズする
- Issue の依頼や判断の有無は、コメントを全件読んでから判断する（`gh issue view N --json comments`）。2026-09-17 に、#40 のコメントを途中までしか読まずに「#45 の PR 依頼が出ていない」と誤って報告した
- Issue への投稿は、本文をレモンが確認し「投稿して」の指示があってから行う

## 3. 開いている Issue と状態

| Issue | 内容 | 状態 |
|---|---|---|
| #57 | 長文を Space で変換すると、返っているのにエンジンが再起動されることがある（`bg_timeout_watchdog` の詰まり開始時刻が残る） | **確認済み・未回答**（2026-09-17、nick）。`candidate_window.rs` の `on_waiting_timer` が `bg=done` で `bg_timeout_watchdog(false)` を呼ばないため、古い時刻から数えて閾値を超え `engine_reload_force()` が走る、という報告。2026-09-16 の `rakukan.log` に `conv worker stuck 3618s` / `65s` の実例。手元の修正はフォークの `e8eeac9`（#56 と同じコミット。PR にするなら分けると書かれている）。**主経路はコードと合う**。待機タイマーの上限（10 秒）が監視の閾値（20 秒）より短く、今の詰まりの検出は記録を次の待機まで残すことで成り立っているので、UI の待機の終了で記録を消すと検出が効かなくなる。一方で、観測していない間に実行が完了して別の実行が始まることや、監視している実行・エンジンが置き換わることもあるので、TSF 側で `false` を足す案では足りない。**判断: 監視対象を「同じ実行中の変換」にし、B（エンジン側で実行番号と開始からの経過時間を持ち、ホストが監視する）を軸に設計する**（計画書 3-6、9 節「#56 / #57 の確認」「#56 / #57 の設計」）。ABI の版を上げる見込み。今は設計と返信の下書きまで。nick への返事はまだ |
| #56 | 未確定文字があるときにホストが再起動すると、Space も Backspace も効かなくなる | **確認済み・未回答**（2026-09-17、nick）。新しいホストの読みバッファが空になり、TSF のセッションと composition だけが未確定文字を持つ、という報告。`handle_action: "Input" state=Preedit("…") bg=idle hira=""` の実例。手元の修正はフォークの `e8eeac9`（composition が生きていてエンジンの読みが空なら、セッション側の読みを `force_preedit` で戻す。Esc 直後は対象外）。**前提（新しいエンジンは読みが空、TSF は合わせない）はコードと合う**。ただし、ホストやエンジンの世代を示す情報が無く、RPC 失敗も「空」になる。エンジンは全アプリで共有。通常入力ではセッションが `Idle` のままで復元の元が無い。`force_preedit` は未確定ローマ字を失う。自動再送で読みを変える要求が二重に適用されうる。**判断: エンジンの世代情報を追加する方向で設計する**。世代だけでは足りず、共有エンジンの所有者（どの composition の読みか）、復元元の編集状態の保持場所（TSF 側）、世代違いを適用前に検出する順序、再送の二重適用も合わせて設計する。未確定ローマ字・打鍵履歴の復元は未確定（計画書 3-7、9 節「#56 / #57 の確認」「#56 / #57 の設計」）。プロトコルの版を上げる見込み。今は設計と返信の下書きまで。nick への返事はまだ |
| #55 | ホストを spawn した後の接続失敗が `HostSpawnGuard` の失敗回数に入らない | 未着手。計画書の段 3（3-1）。#54 は直ったので、ログで発生状況を見られる。発生が 0 件でも修正と再現テストは省略しない方針 |
| #54 | RPC クライアントのログが `rakukan.log` に記録されない | **修正済み・未リリース**（`a981b07`）。インストール版で、言語バーの「エンジン再起動」で `INFO rakukan_engine_rpc::client: rpc: Shutdown acknowledged by host` が記録されることを確認（2026-09-17T02:50:06Z）。**1 日分のログ量の確認が残る**（基準: 2026-09-17T02:50:20Z 時点の `rakukan.log` が 73,033 行・9,736,860 バイト、インストール後の最初の行は 72,638 行目。ローテーションが起きたら `.1` と合わせて数える）。Issue は開いたまま |
| #53 | 数字と「参考」を同じ変換単位で打つと候補が捨てられる（`参` `拾` を大字 1 文字として数える） | **方針を決めて nick に PR を依頼済み**（[依頼コメント](https://github.com/fukuyori/rakukan/issues/53#issuecomment-5708502866)）。方式: 候補の `参` / `拾` が、入力の読みに対応する辞書の語の一部と確かめられたときだけ数えない（辞書による語と読みの対応付け + 動的計画法による存在判定）。模擬実装で全探索と照合済み。**本実装での性能評価は未完了**で、PR に比較計測を添えてもらい、受入れと数値の基準は分けて判断する。経緯・設計・受入条件・既知の制限は計画書 9 節の「#53」で始まる小節 |
| #51 | アプリごとの IME 初期状態 | 段 1 は main に入った（未リリース）。**段 2 = 設定アプリ（WinUI）での編集画面が残る**。実行中のアプリから選ばせたい。#33 と時期を調整 |
| #50 | 破棄済み DocumentManager のポインタが `dm_modes` に入り直し、アドレス再利用で前のモードが復元される | 未着手。計画書の J-4（推奨: 世代と生存状態を持たせる）と段 3（3-2）。Activate 時の直接呼出し（`factory.rs`）も対象経路 |
| #49 | `model_variant` を変えてもホストが続く限り古いモデルのまま | 未着手。計画書の J-3（推奨: モデル設定の変更でホストを終了）と段 3（3-4）。先に設定の全プロセス反映（3-3）を設計する前提 |
| #46 | 学習履歴を個別に削除する手段が無い | インストーラー改修（#33）と合わせる予定 |
| #45 | 速く打つとライブ変換のプレビューが更新されない | nick の PR 待ち（#40 の 2026-09-12 のコメントで依頼済み） |
| #40 | 候補・予測の品質（統合ブランチ側の報告 7 件） | 1・3 は再現せず、7 は #45、2 は縮小版の PR 待ち（依頼済み）、4 は見送り |
| #35 | 区読点だけの読みとリーダー記号 | 判断待ち。計画書の J-6（推奨: Space 変換の本線を先に、`z` キー列は後）。[Symbol_Leader_Input_Plan.md](Symbol_Leader_Input_Plan.md) の 3.1.1 の A〜C が未決。**PR #31 は 2026-09-13 に nick が取り下げてクローズ済み**（方針が決まったら新しく出すとのこと） |
| #33 | インストーラー再設計（`%ProgramFiles%` 移行、更新は実行 → 再起動で完了） | [October_Install_Plan.md](October_Install_Plan.md) |
| #32 | 文節変換 | 判断待ち。[Segment_Edit_Plan.md](Segment_Edit_Plan.md)。計画書では本計画とインストーラー改修の後 |
| #29 | ユーザー辞書の `priority = "low"` | 保留 |
| #18 | 統合ブランチの棚卸し | 各項目は個別 Issue へ分割済み。記録として残している |
| #16 | 数字混在の読みで、かな run が辞書を参照できない | 判断待ち。計画書の J-7（推奨: 段 4 で設計から）。変換ワーカーへ辞書を渡す配線は #53 と共通になる。`はんは` のように助詞を含む読みも辞書に届かない（2026-09-14 確認） |

#56 / #57 は作業計画の段 3 に別項目で追加した（3-6「#57 修正経路と回帰条件の確認」、3-7「#56 復旧条件・状態復元の設計」）。コードでの確認結果は計画書 9 節「#56 / #57 の確認」、設計（案）は「#56 / #57 の設計」。nick への返事は Issue ごとに下書き中（設計への意見を求める段階で、実装の依頼はしない）。PR は分けて、最新の main を基準に確認する方針。#57 は ABI、#56 はプロトコルの版を上げる見込みで、変更の順序は nick と相談する。

## 4. 実機で未確認の項目

| 項目 | 補足 |
|---|---|
| `ime_off_apps` の既定値 `conhost.exe` / `WindowsTerminal.exe` / `mintty.exe` | 一致したログの記録が無い。wezterm-gui.exe / ghostty.exe は確認済み |
| `ime_on_apps` で Photoshop の文字ツール | 環境が無い。firefox / msedge / chrome は確認済み |
| Step 13-1「モデルが ready になったら自動で変換をやり直す」 | モデル読み込みが 157ms で終わり手で狙えない。コールドスタート時に確認する |
| Step 13-2 `edit_session: SLOW grant` | 閾値を超えたときだけ出る。0 件は「観測期間中に閾値超過を記録しなかった」と扱う |
| Step 13-2 `rpc SLOW` | #54 の修正で**記録されるようになった**（出力経路は確認済み）。インストール直後の変換（約 380 行分）では 0 件。1 週間の再計測（段 2）で件数を見る |
| #54 の 1 日分のログ量 | 上の基準値から比べる |

## 5. 既知の問題（Issue 化していないもの）

- **起動時・再読込時に config のパースに失敗すると、警告なしで全設定が既定値に戻る**（`ConfigManager::new` / `init_config_manager` の `unwrap_or_default()`）。`init_config_manager` は保存通知の `engine_reload` と言語バーの「エンジン再起動」からも呼ばれ、既定値から作った設定がホストへ送られる（ホストの再起動・再生成につながるかは推測、未確認）。計画書の J-5 で起票を推奨（未決）
- **config の再読込がアプリごとにばらばら**。reload イベント `Local\rakukan.engine.reload` が auto-reset のため、1 回の Set で 1 プロセスにしか届かない。計画書の J-5 で起票を推奨（未決）
- **MOZC 辞書のリビジョンを固定・記録していない**。`scripts/install.ps1` は `refs/heads/master` から取得し、ビルド後に TSV を削除し、`rakukan.dict.build.json` にもリビジョンを書かない。インストール済み辞書（2026-09-12 作成）の生成元は、`dictionary_oss` の最終変更コミット `8be758d` と推定（14 の読みで再現が一致した裏付けのみで、確定した記録ではない）
- **`rakukan-dict-builder` の doc コメントの列の並びが誤っている**（`読み TAB 表記 TAB 品詞名 …` と書いているが、実際と実装は `読み TAB lid TAB rid TAB cost TAB 表記`）
- 入力先ごとのモード記憶で、手動変更の即時保存（`doc_mode_remember_current`）は `trace!`、`DOC_MODE_STORE.try_lock()` の失敗は無言で抜けるため、この 2 経路はログから確認できない
- ブラウザで「入力項目が変わると英数に変わる」という症状は、調査を途中で打ち切った（`default=` のイベント 42 件を見た段階で、原因は未特定）

## 6. 検証と調査の知見

- **モデル未準備の状態を作る**: `model_variant` を存在しない値にする。その後ホストを終了する（#49 のため config を変えただけではモデルが切り替わらない）
- **推論失敗を作る**: `[diagnostics] force_inference_failure = true`。converter に焼き込まれるので、有効化にはホストの作り直しが要る
- **全アプリに config を反映させる**: reload イベントを連続で Set する（上記の auto-reset のため）
- **RPC クライアントのログが出ることを確かめる**: 言語バーの「エンジン再起動」。config が同じでもホストに Shutdown を送るので、`rpc: Shutdown acknowledged by host` が必ず出る。正常な接続・変換では RPC クライアントはログを出さない
- **エンジン DLL のログレベル**は `config.toml` の `log_level` に追随する（ホスト spawn 時に `RAKUKAN_LOG` として渡す。環境変数を明示していればそちらが優先）
- **ホストの健全性**: 連続 3 回の推論失敗でホストが自己終了し、5 分以内に 2 回自己終了した後にまた失敗すると `unrecoverable`。判断はホスト側（`crates/rakukan-engine-rpc/src/health.rs`）
- **辞書・モデルの注入**はホストの `dispatch_engine` 冒頭で毎回試みる。TSF のラッチやホストが入れ替わった理由に依存しない
- **判定規則の一時的な比較**: `crates/rakukan-engine/src/digits.rs` の末尾に `#[cfg(test)]` の一時モジュールを追記して `cargo test -p rakukan-engine --lib <モジュール名> -- --nocapture` で流し、終わったら `git checkout -- crates/rakukan-engine/src/digits.rs` で撤去する。#53 の検証はすべてこの形（コミットしていない）。`cargo test … digits` のフィルタは一時モジュールの名前にも当たるので、既存テストの件数を数えるときは一時モジュールを撤去してから流す（2026-09-17 に「47 件」と誤って報告した。正しくは 46 件成功・1 件 ignored）
- **辞書ファイルの中身を見る**: `rakukan.dict` の形式は `crates/rakukan-dict/src/mozc_dict.rs` の冒頭のコメントどおり（ヘッダー 16 バイト、読みの索引 12 バイト × 件数、読みのヒープ、エントリ 8 バイト × 件数、表記のヒープ）。読み取り専用のスクリプトで引ける。元辞書の品詞は `dictionary_oss/id.def` と `src/data/rules/pos_matcher_rule.def`（`名詞,サ変接続` は `Unknown` にも使われる）
- **ツールの注意**
  - Git Bash のヒアドキュメントで Python にファイルを書かせると `\\` が潰れ、Windows パス中の `\r` が CR になる。書いた後に CR の混入を確認する
  - Bash ツールのヒアドキュメントは、長い本文で `unexpected EOF while looking for matching` と失敗することがある（何も書き込まれない）。その場合は Write ツールでスクラッチパッドにファイルを作り、`cat >>` で追記する
  - Git Bash から `powershell.exe`（Windows PowerShell 5）経由で `cargo test` を流すと、日本語の出力が化ける。PowerShell ツール（pwsh）で `[Console]::OutputEncoding = [Text.Encoding]::UTF8` を設定し、`Out-File -Encoding utf8` で保存して読む
- 過去の Issue コメントのうち Claude が起案したもの（例: #18 の 9/4 返信）を、レモンの決定として扱わない

## 7. 関連資料

| 資料 | 内容 |
|---|---|
| [September_Late_Plan.md](September_Late_Plan.md) | **現在の作業計画**。段 1〜4、判断事項 J-1〜J-9、9 節に実施記録（#54 の修正と確認、#53 の影響確認・方式の比較・辞書の調査・動的計画法の照合・本実装の設計・PR の依頼） |
| [DESIGN.md](DESIGN.md) | 全体設計（プロセス構成、RPC、ホストのライフサイクル、設定、辞書） |
| [September_Revised_Plan.md](September_Revised_Plan.md) | 9 月前半の実行計画と、Step ごと・リリースごとの記録（Step 14 は September_Late_Plan.md に引き継いだ） |
| [October_Install_Plan.md](October_Install_Plan.md) | インストーラー改修（#33） |
| [Segment_Edit_Plan.md](Segment_Edit_Plan.md) | 文節変換の論点（#32） |
| [Symbol_Leader_Input_Plan.md](Symbol_Leader_Input_Plan.md) | リーダー記号の検討（#35） |
| [Step10_Romaji_Rebuild_Plan.md](Step10_Romaji_Rebuild_Plan.md) | ローマ字入力の再構築（完了） |
| [version-update-checklist.md](version-update-checklist.md) | バージョン変更で更新するファイル |
| [GPU_MEMORY_LIFECYCLE.md](GPU_MEMORY_LIFECYCLE.md) | ホスト多重起動時の GPU メモリ（「GPU 浪費」と論じない根拠） |
| [EXPLORER_CRASH_HISTORY.md](EXPLORER_CRASH_HISTORY.md) / [INVESTIGATION_GUIDE.md](INVESTIGATION_GUIDE.md) | Explorer 異常終了の対策年表と dump 解析の手順（前版の「既知の問題」はこちらに集約されている） |
| [archive/](archive/README.md) | 完了・破棄した計画書 |
