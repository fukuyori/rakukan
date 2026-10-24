# Rakukan 引き継ぎ資料

作成: 2026-10-23（2026-10-22 までの引き継ぎ資料は [archive/handoff_to_2026-10-22.md](archive/handoff_to_2026-10-22.md) に移した）

この文書には、次のセッションが作業を続けるために知っておくことだけを書く。
後から変わらない事実（コミット、タグ、含まれる版、日付）で書く。リリースされたかどうかは書かない（GitHub の Releases で確かめる）。
main・タグ・Issue の今の状態は、作業を始めるときに `git status -sb` / `git log --oneline -3` / `git ls-remote origin` / `gh release list` / `gh issue list` で確かめる。

## 0. 現在の位置と次の開始点

### 位置

- 作業計画は [October_Plan.md](October_Plan.md)（フェーズ 1〜5）。フェーズ 1（#71）・フェーズ 2（#69 / #70）は 2026-10-22 に完了した。どちらも 0.13.0（タグ `0.13.0` = `7c76649`）に含まれる。#69・#70・#71 は同日にクローズした。
- 0.13.0 の変更点は CHANGELOG.md の `[0.13.0]`。RPC protocol v7、engine ABI 12。
- フェーズ 2 は、計画ではフェーズ 3 と同じ版で出す予定だった。2026-10-22 に改め、単独で 0.13.0 に含めた（理由と条件は [Input_Pipeline_Plan.md](Input_Pipeline_Plan.md) 9 節の決定 1）。古いホストとの組み合わせは、0.13.0 のインストールの手順（別の IME に切り替え → サインアウト → サインイン → rakukan を使わずにインストール）で避けた。
- フェーズ 2 の試験のうち、故障を起こす口が要るもの（保留中の操作、257 キー目、`VersionMismatch`、ロック競合など）は #85（試験の自動化の基盤）に回した。
- フェーズ 3（#33・#73〜#76）の Step 1〜5 は 0.13.1（タグ `0.13.1` = `84ff900`）に含まれる。変更点は CHANGELOG.md の `[0.13.1]`。RPC protocol v7・engine ABI 12 のまま。配布物 `rakukan-0.13.1-setup.exe` のビルドの識別子は `7a04bef972ff`（版の変更をコミットする前の作業ツリーから作った。中身はタグと同じ）。

### 今の作業: フェーズ 3（段 C、#33。#73〜#76 を含む）

[October_Plan.md](October_Plan.md) の 4 節。作業は [Installer_Redesign_Plan.md](Installer_Redesign_Plan.md) の 5 章の Step に従い、ログの見直しは [Log_Redesign_Plan.md](Log_Redesign_Plan.md)。

フェーズ 3 は専用のブランチ `phase3-installer` で作業し、その間は main を更新しない。ブランチから main への pull request（#87）を開いたままにして CI を動かし、配布前の完了の後にマージコミットでマージする。リリースはフェーズ 3 の終了後に 1 回、マージした後の main で行う（October_Plan.md 4 節、2026-10-23 決定）。

Step の進み（2026-10-24。進捗は tque の #6 の下に登録している）:

| Step | 内容 | コミット | 実機の試験 |
|---|---|---|---|
| 1 | 配置に依存しないパス、ログ（#73・#75・#76、ETW）、engine DLL のピン止め、第三者のライセンス、#88・#90。レビューの指摘の修正 | `2a628b5`・`500d161`・`b93532a` | S1 合格（Test_Procedure.md の 2026-10-23 の結果） |
| 2 | 新しいインストーラー（64 ビット、`{autopf}`、`restartreplace`、`AlwaysRestart`、登録の成否をレジストリで判定・終了コード 20、HKLM の Run のトレイ）、トレイ（多重起動の抑止・ログ・掃除・キーボードの一覧の補いを版ごとに 1 回）、ログの掃除を 90 日・30000 ファイルに（#74） | `7c6f570` | S2 合格（Step 3 とまとめて） |
| 3 | 旧配置（`%LOCALAPPDATA%\rakukan`）からの移行（`UsePreviousAppDir=no`、旧 DLL の登録の解除、HKCU の Run の削除、旧版のファイルの削除） | `f74a9c7` | 同上 |
| 4 | 経路の一本化（`build-installer.ps1` の入力をビルドの出力に・辞書のビルドを移す、`install.ps1` / `uninstall.ps1` の削除、`dev-swap-tsf.ps1`、Makefile（`install` / `package` / `uninstall` / 故障試験の 2 タスク）、check-env / setup-env、README、CLAUDE.md、Stop hook）。移行で 0.13.0 の 32 ビット側のアンインストール情報を消す処理、`gen-third-party-licenses.ps1` のソートを序数比較に（Windows PowerShell 5.1 で `-Check` が通らなかった）。S3 の中で `check-bundled-licenses.ps1`・`dev-swap-tsf.ps1` の `Get-FileHash` を .NET の SHA256 に（cargo make から起動した Windows PowerShell 5.1 で見つからない） | — | S3 合格（Test_Procedure.md の 2026-10-24 の結果） |
| 5 | 配布前の文書（CHANGELOG の `[Next Release]` の節、DESIGN.md・handoff.md の配置と導入の手順、version-update-checklist.md の CHANGELOG の見出し） | `cc189b3`（Step 4 と同じ） | 文書だけ |
| 6 | リリース作業: PR #87 をマージコミット `7a04bef` でマージ、版を 0.13.1 に上げ・ビルドの識別子から `-dirty` を外す（`84ff900`）、タグ `0.13.1`、配布物で T1〜T4・試験 30 | `7a04bef`・`84ff900` | 合格（Test_Procedure.md の 0.13.1 の結果） |

**次の開始点**: 公開後の完了の Step 7（PR #30 への結論のコメント）は 2026-10-24 に投稿した（tque #54）。winget への登録は #91 で扱う（時期は未定）。#33・#73〜#76 は 2026-10-24 にクローズした。フェーズ 3 は完了。フェーズ 4 も 2026-10-24 に完了した（下の「フェーズ 4」）。10 月の計画で残っているのはフェーズ 5（#83・IPC の名前）と、計画の終わりの `phase3-installer` のブランチの削除。

決まったこと・分かったこと（フェーズ 3 の中で）:
- #75 は engine ABI を変えない（12 のまま）。callback を受ける関数は、無くても動く関数（`load_sym_opt!`）として足した。
- #64: 1 つのホストの中では engine DLL を替えない。フェーズ 4（下）で実装した。engine DLL のピン止めはそのまま残す。
- モデルや `gpu_backend` を替えて保存しても、ホストを起動し直すまで反映されなかった（#49 のコメント）。フェーズ 4 で直した。
- 著作権の表示は `rakukan Contributors` のまま（`LICENSE`・`NOTICE`・設定アプリの csproj。2026-10-23 に利用者が確認）。
- 0.13.0 の配布物は `output\oldversion\rakukan-0.13.0-release-setup.exe` に写してある。
- 今の PC には 0.13.1 の配布物が入っている（`%ProgramFiles%\rakukan`）。開発の導入（ビルド → `sudo cargo make install` → 再起動）で同じ場所に上書きする。`%LOCALAPPDATA%\rakukan` に残っているのはログ・`etw`・`config.toml` など。
- `install-tsf-*`（故障試験の TSF の差し替え）は、入力方式が Rakukan だと explorer とターミナルが TSF DLL を読み込んでいてロックで止まる。サインアウト → サインインの後、入力方式を Rakukan にせずに実行する。charmap で確かめる前には入力方式を Rakukan にする（Test_Procedure.md の S3-3）。
- 0.13.0 までのインストーラー（32 ビットのモード）が書いたアンインストール情報（`HKLM\SOFTWARE\WOW6432Node\…\Uninstall\{B7C4E2A1-…}_is1`）が、移行の後も残っていた（2026-10-24、S2 の後のこの PC で発見）。旧版のアンインストーラーは移行で消えるので、「アプリ」の一覧に削除できない旧版が残る。旧配置を指すものを移行で消す処理（`RemoveOldUninstallEntry`）を足し、S3-1 と、0.13.0 の配布物からの更新（T4-1）で消えることを確かめた。

### フェーズ 4（#64・#66・#49・#82）

- 設計書は [Engine_Config_Apply_Plan.md](Engine_Config_Apply_Plan.md)（Codex CLI のレビューを 2 回受けた）。結果は [October_Plan.md](October_Plan.md) の 5 節。2026-10-24 に実装と実機の試験を終え、完了とした。
- コミット: `c0dcaf6`・`542d9c1`・`36cd6a9`・`23749e6`。RPC protocol v8、engine ABI 12。変更点は CHANGELOG.md の `[Next Release]`。
- 設定が変わると、ホストは自分の中でエンジンを作り直さず起動し直す。送られた設定は config.toml の本文のハッシュと照合し、一致したものだけを採用する。config.toml を解析できない間は、`%LOCALAPPDATA%\rakukan\host-config-last.json`（ホストが最後に採用した設定）で動く。
- #82 は、モデルの定義を config.toml へ移さず、engine に埋め込む models.toml を一覧の元にする形に改めた（設計書 2 章）。設定アプリは同じ models.toml を出力に写して読む。Issue #82 の題と本文は、まだ今の決定に合わせていない。
- 保存した設定で起動したホスト（config.toml が壊れている間）は、`host-config-last.json` を書き直すときに `config_sha256` を `null` にする。正しく読めた config.toml から採用したのではないので、これでよいとした（2026-10-24。照合には使わない記録の値）。

### 並行して行うこと

- #57 の運用確認、#50 の照合と段 2、1 か月分のログで再計測、#83 の計測と小さな改善は、11 月に行う（[2026-11_Plan.md](2026-11_Plan.md) の 2 節）。

## 1. 作業の決まり

### ビルドと導入

```powershell
cargo make build-engine   # エンジンを変えたときだけ
cargo make build-tsf
sudo cargo make install   # インストーラーを署名なしで作り、サイレント実行（%ProgramFiles%\rakukan）
# PC を再起動（インストールの完了。事前のサインアウトは要らない）
```

- 手順を示すときは、この順序で示す。
- `sudo cargo make uninstall` はアンインストーラーをサイレント実行する（完了には再起動）。`cargo make install-tsf-fault-test` / `install-tsf-normal-restore` は #65 の故障試験用に TSF DLL だけを差し替える。入力方式が Rakukan だと explorer とターミナルが DLL を読み込んでいてロックで止まるので、サインアウト → サインインの後、入力方式を Rakukan にせずに実行する。
- `cargo test` は PowerShell から実行する（Git Bash 経由では ConPTY テストがハングする）。全体は `cargo test --workspace --lib`。バイナリの crate（`rakukan-dict-builder`）は `cargo test -p rakukan-dict-builder` を別に実行し、件数を分けて報告する。
- CI は Format Check（`cargo fmt --all -- --check`）と Build & Test (Windows)。コミットの前に `cargo fmt --all -- --check` と clippy を通す（2026-10-22 に fmt の漏れで CI が 2 回落ちた）。
- 配布物（インストーラー）は `cargo make package`（sign → `build-installer.ps1 -Sign`）で、利用者が作る。開発の導入と同じビルドの出力から、同じインストーラーを作る（`scripts/build-installer.ps1`）。cargo make が起動するのは Windows PowerShell 5.1 なので、スクリプトは 5.1 で動く書き方にする（`Get-FileHash` は PowerShell 7 から起動すると見つからない。Sort-Object の既定の比較は 5.1 と 7 で順が違う）。

### 実機の試験

- 手順と、どの試験をいつ行うかは [Test_Procedure.md](Test_Procedure.md)（T1 通常の入力、T2 ホストの入れ替え、T3 設定、T4 インストール・更新・アンインストール、S1・S2 フェーズ 3 の Step 1〜3）。
- 手順は「1. 操作（コマンド）」と「見る点:」の番号付きの一覧で示す。管理者の PowerShell では作業フォルダがリポジトリにならないので、パスはフルパスで書く。
- 修正はまとめてから 1 回で試験する。試験は 1 つずつ示し、操作と見る点を同じ行に書き、戻す操作まで含める。
- 結果は Test_Procedure.md の末尾に、日付・版・ビルドを書いた節として足す。

### 文書

- 文書に「未リリース」「リリース済み」「最新リリースは」を書かない。コミットと、含まれる版（`git tag --contains`）で書く。版を上げるときは [version-update-checklist.md](version-update-checklist.md) の 6 章の確認を流す。
- フェーズの完了時は、October_Plan.md の該当の節に結果を書き、この文書の 0 章と [Issue_Status.md](Issue_Status.md) を更新する。Issue の状態を変えたら Issue_Status.md を同じコミットで更新する。
- 指示の無いファイルをリポジトリに作らない。作業用はスクラッチパッド。役目を終えた計画書は `docs/archive/` に移し、[archive/README.md](archive/README.md) の表に足す。

### GitHub

- `origin` = fukuyori/rakukan、`nick` = nick20002005 のフォーク（PR と Issue の主な提案者）。
- コミット・タグ・プッシュは、その都度「コミット」「プッシュ」の指示があってから行う。
- コミットメッセージと PR 本文に、完了キーワードと Issue 番号を並べて書かない（引用でも自動でクローズされる）。`Refs #NN` と書き、Issue は手でクローズする。
- Issue への投稿は、本文の確認と「投稿して」の指示があってから行う。Issue の依頼や判断は、コメントを全件読んでから判断する（`gh issue view N --json comments`）。
- 作業の前のレビューは Codex CLI（`codex exec -s read-only -C <repo> -o <出力> - < <依頼文>`）で受ける。

## 2. ファイルの場所

| 用途 | 場所 |
|---|---|
| インストール先 | `%ProgramFiles%\rakukan\`（開発の導入も配布物も同じ） |
| ビルドの出力 | `C:\rb\release`、辞書は `C:\rb\dict\`、インストーラーは `output\rakukan-<version>-setup.exe`、セットアップのログは `output\logs\`（`cargo make install` の実行ごと） |
| モデル | `%ProgramData%\rakukan\models`（無ければ既存の `%USERPROFILE%\.cache\huggingface\hub` を読む） |
| 設定 | `%APPDATA%\rakukan\config.toml` / `keymap.toml` / `user_dict.toml` |
| 学習履歴 | `%APPDATA%\rakukan\learn_history.bin` |
| 辞書 | インストール先の `dict\rakukan.dict`（隣に `rakukan.dict.build.json`） |
| ログ | `%LOCALAPPDATA%\rakukan\rakukan-tsf-<PID>-<起動識別子>.log`（TSF。アプリのプロセスごと。時刻順の統合は `scripts/merge-logs.ps1`）/ `rakukan-engine-host-<PID>-<起動識別子>.log`（engine DLL の行を含む）/ `rakukan-tray-<PID>-<起動識別子>.log`。各ファイルの 1 行目がヘッダー。トレイが起動時に 90 日・30000 ファイルを超えた分を消す |
| 登録の記録 | `%LOCALAPPDATA%\rakukan\register_debug.log`（regsvr32 の登録・解除のたびに時刻・`dll_path`・各段階の結果） |
| 調査用の ETW | `scripts/etw-trace.ps1 start / stop / export`（管理者）。provider は `Rakukan.Tsf`・`Rakukan.Host` |
| 復帰マーカー | `%LOCALAPPDATA%\rakukan\engine-recovery.txt`（#43 / #57） |

## 3. 文書の地図

| 文書 | 内容 |
|---|---|
| [October_Plan.md](October_Plan.md) | 10 月の作業計画（フェーズ 1〜5、各フェーズの作業・開始と完了の条件・結果） |
| [2026-11_Plan.md](2026-11_Plan.md) | 11 月の作業計画（候補の一覧。辞書の版の整理、10 月の計画の残り、#91）。計画書の名前は 11 月から `年-月_Plan.md` |
| [Issue_Status.md](Issue_Status.md) | 開いている Issue の概要・対応状況・次の動き、クローズの記録 |
| [DESIGN.md](DESIGN.md) | 全体の設計（プロセス構成、RPC、ホストのライフサイクル、設定、辞書） |
| [Input_Pipeline_Plan.md](Input_Pipeline_Plan.md) | フェーズ 2（#69 / #70）の入力処理の設計。6 節に試験の表と判定、7 節に実装時の変更、9 節に利用者の決定 |
| [Installer_Redesign_Plan.md](Installer_Redesign_Plan.md) | フェーズ 3 のインストーラーの見直し（#33） |
| [Log_Redesign_Plan.md](Log_Redesign_Plan.md) | フェーズ 3 のログの見直し（#73〜#76） |
| [Test_Procedure.md](Test_Procedure.md) | 実機の試験の手順・行う時期・結果 |
| [version-update-checklist.md](version-update-checklist.md) | 版を上げるときに直すファイルと確認 |
| [Segment_Edit_Plan.md](Segment_Edit_Plan.md) / [Symbol_Leader_Input_Plan.md](Symbol_Leader_Input_Plan.md) | 検討中: 文節変換（#32）/ リーダー記号（#35） |
| [GPU_MEMORY_LIFECYCLE.md](GPU_MEMORY_LIFECYCLE.md) | ホストが複数あっても GPU を圧迫しない根拠 |
| [EXPLORER_CRASH_HISTORY.md](EXPLORER_CRASH_HISTORY.md) / [INVESTIGATION_GUIDE.md](INVESTIGATION_GUIDE.md) | Explorer の異常終了の対策の年表と、dump の解析の手順 |
| [archive/](archive/README.md) | 役目を終えた計画書（9 月の計画、Step 10、#65 の故障試験の設計、2026-10-22 までの引き継ぎ資料など） |

## 4. 残っている課題と観察

- **#85（試験の自動化の基盤）**: フェーズ 2 の対象外にした試験（Input_Pipeline_Plan.md 6 節の表で「対象外」の行）は、故障を起こす口（`DocOps` / FakeHost / 時計の外出し）が要る。今後の開発の基盤として計画する。
- **#86**: 同じプロセスで複数のスレッドが TSF を使うと、composition とセッションがスレッドの間で共有される。重要度は低。起きるアプリは見つかっていない。ログのアプリ名（`app=`）で探す。
- **観察（調べていない）**: ホストを起動できない状態で、最初のキーの待ちが 3.6 s になったことがある（`Transport` → `HostMissing` → `Suppressed`。2026-10-22）。
- **フェーズ 3 で行っていない確認**: 別のユーザーでの確認（Test_Procedure.md の S2-6）。
- **観察（調べていない）**: 2026-10-23 のサインインの後、ChatGPT.exe の TSF のログが作られなかった。ChatGPT で「100%」が「10%」になることがあるという報告があったが、ログが無く確かめられていない。
- **運用で確かめる**: #57（詰まりの検出）は運用ログで確認してクローズを判断する。#50（DM の識別）は段 1 のログを入れてから照合する（どちらも 2026-11_Plan.md の 2 節）。

## 5. 調査の知見

- **ホストを起動できない状態を作る**: `%LOCALAPPDATA%\rakukan\rakukan-engine-host.exe` の名前を変え、動いているホストを止める（`Stop-Process -Name rakukan-engine-host -Force`）。ホストは常駐なので、アプリを閉じるだけでは止まらない。元に戻すのを PowerShell の `finally` に任せない（ターミナルの強制終了で実行されなかった。2026-09-22）。
- **未確定の文字を持ったままホストを入れ替える**: アプリからフォーカスが外れると未確定の文字は確定されるので、PowerShell で `Start-Sleep` で遅らせてからホストを止め、その間にアプリへ戻って操作する。待ちは 10〜15 s 程度にする（長いとフォーカスが移る）。
- **モデル未準備の状態を作る**: `model_variant` を存在しない値にしてからホストを終了する（#49 のため、設定を変えただけではモデルが切り替わらない）。
- **推論失敗を作る**: `[diagnostics] force_inference_failure = true`。有効にするにはホストの作り直しが要る。
- **ホストのログの DEBUG を出す**: ユーザーの環境変数 `RUST_LOG=info,rakukan_engine_rpc=debug` を設定し、サインインし直す（ホストは起動したアプリの環境を受け継ぐ）。試験の後に消す。engine DLL のログは `config.toml` の `log_level` に従う（`RAKUKAN_LOG` を明示すればそちらが優先）。
- **RPC のログが出ることを確かめる**: 言語バーの「エンジン再起動」で `rpc: Shutdown acknowledged by host` が出る。接続のたびに `host connected: Hello/Create ok …` が出る。
- **キーの扱いを追う**: TSF のログの `key_diag:`（キーごとの判断）、`rescue:`（救済の各段階）、`key_queue:`（保留・直接入力）、`rpc SLOW … thread=`（UI のスレッドの停止）。
- **ツールの注意**
  - Git Bash のヒアドキュメントで Python に書かせると、`\\` が潰れ、Windows のパスの `\r` が CR になる。パスを含む文字列は `chr(92)` で組み立てるか Edit ツールを使い、書いた後に CR の混入を確かめる。
  - Git Bash の `sed` に `/` で始まる式を渡すと、パスに書き換えられることがある（`MSYS_NO_PATHCONV=1`）。
  - `cargo test` の日本語の出力は、PowerShell ツール（pwsh）で読む。
- 過去の Issue コメントのうち Claude が起案したもの（例: #18 の 9/4 の返信）を、利用者の決定として扱わない。
