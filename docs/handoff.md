# Rakukan 引き継ぎ資料

作成: 2026-10-23（2026-10-22 までの引き継ぎ資料は [archive/handoff_to_2026-10-22.md](archive/handoff_to_2026-10-22.md) に移した）

この文書には、次のセッションが作業を続けるために知っておくことだけを書く。
後から変わらない事実（コミット、タグ、含まれる版、日付）で書く。リリースされたかどうかは書かない（GitHub の Releases で確かめる）。
main・タグ・Issue の今の状態は、作業を始めるときに `git status -sb` / `git log --oneline -3` / `git ls-remote origin` / `gh release list` / `gh issue list` で確かめる。

## 0. 現在の位置と次の開始点

### 位置

- 作業計画は [October_Plan.md](October_Plan.md)（フェーズ 1〜5）。フェーズ 1（#71）・フェーズ 2（#69 / #70）は 2026-10-22 に完了した。どちらも 0.13.0（タグ `0.13.0` = `7c76649`）に含まれる。#69・#70・#71 は同日にクローズした。
- 0.13.0 の変更点は CHANGELOG.md の `[0.13.0]`。RPC protocol v7、engine ABI 12。
- フェーズ 2 は、計画ではフェーズ 3 と同じ版で出す予定だった。2026-10-22 に改め、単独で 0.13.0 に含めた（理由と条件は [Input_Pipeline_Plan.md](Input_Pipeline_Plan.md) 9 節の決定 1）。古いホストとの組み合わせは、今のインストールの手順（別の IME に切り替え → サインアウト → サインイン → rakukan を使わずにインストール）で避ける。
- フェーズ 2 の試験のうち、故障を起こす口が要るもの（保留中の操作、257 キー目、`VersionMismatch`、ロック競合など）は #85（試験の自動化の基盤）に回した。

### 次の開始点: フェーズ 3（段 C、#33。#73〜#76 を含む）

[October_Plan.md](October_Plan.md) の 4 節。作業は [Installer_Redesign_Plan.md](Installer_Redesign_Plan.md) の 5 章の Step に従い、ログの見直しは [Log_Redesign_Plan.md](Log_Redesign_Plan.md)。着手の前に、次の 2 つの開始の条件を片づける。

1. Installer_Redesign_Plan.md 7 章の確認事項（登録の方式・実行するユーザー・順序・失敗時の処置・終了コード・ACL・既存モデルの扱い）を決め、確かめた結果を同 7 章に書く。→ 2026-10-23 に決め、同 7 章に書いた。
2. #64（設定が合わない `Create` で旧 engine DLL を外しうる）の DLL の寿命の扱いを決める。#75 の callback と #76 の ETW provider の解除にかかわる。→ 2026-10-23 決定: 1 つのホストの中では engine DLL を替えない。実装はフェーズ 4、それまでは Step 1 で engine DLL をピン止めする（October_Plan.md 4 節）。

#75 は engine ABI を変えない（12 のまま）。callback を受ける関数は、無くても動く関数（`load_sym_opt!`）として足し、既存の関数の形と意味は変えない（2026-10-22 決定。Log_Redesign_Plan.md 段 4 の (b)）。

フェーズ 3 は専用のブランチ `phase3-installer` で作業し、その間は main を更新しない。ブランチから main への pull request を開いたままにして CI を動かし、配布前の完了の後にマージコミットでマージする。リリースはフェーズ 3 の終了後に 1 回、マージした後の main で行う（October_Plan.md 4 節、2026-10-23 決定）。

### 並行して行うこと

- 1 か月分のログで再計測（October_Plan.md 7 節）。ログ紛失のため延期。#57・#50 の運用確認もこのとき行う。

## 1. 作業の決まり

### ビルドと導入

```powershell
cargo make build-engine   # エンジンを変えたときだけ
cargo make build-tsf
# サインアウト → サインイン
sudo cargo make install
```

- 手順を示すときは、この順序で示す。
- `cargo test` は PowerShell から実行する（Git Bash 経由では ConPTY テストがハングする）。全体は `cargo test --workspace --lib`。バイナリの crate（`rakukan-dict-builder`）は `cargo test -p rakukan-dict-builder` を別に実行し、件数を分けて報告する。
- CI は Format Check（`cargo fmt --all -- --check`）と Build & Test (Windows)。コミットの前に `cargo fmt --all -- --check` と clippy を通す（2026-10-22 に fmt の漏れで CI が 2 回落ちた）。
- 配布物（インストーラー）は `-Sign` 付きで作る。配布物は `cargo make install` で入れたファイルをそのまま詰めたもの（`scripts/build-installer.ps1`）。

### 実機の試験

- 手順と、どの試験をいつ行うかは [Test_Procedure.md](Test_Procedure.md)（T1 通常の入力、T2 ホストの入れ替え、T3 設定、T4 インストール・更新・アンインストール）。
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
| インストール先 | `%LOCALAPPDATA%\rakukan\`（`%ProgramFiles%` への移行はフェーズ 3） |
| 設定 | `%APPDATA%\rakukan\config.toml` / `keymap.toml` / `user_dict.toml` |
| 学習履歴 | `%APPDATA%\rakukan\learn_history.bin` |
| 辞書 | `%LOCALAPPDATA%\rakukan\dict\rakukan.dict`（隣に `rakukan.dict.build.json`） |
| ログ | `%LOCALAPPDATA%\rakukan\rakukan-tsf-<PID>-<起動識別子>.log`（TSF。アプリのプロセスごと。時刻順の統合は `scripts/merge-logs.ps1`）/ `rakukan-engine-host.log` / `rakukan-engine-dll.log` |
| 復帰マーカー | `%LOCALAPPDATA%\rakukan\engine-recovery.txt`（#43 / #57） |

## 3. 文書の地図

| 文書 | 内容 |
|---|---|
| [October_Plan.md](October_Plan.md) | 今の作業計画（フェーズ 1〜5、各フェーズの作業・開始と完了の条件・結果） |
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
- **インストーラー**: 今のインストーラーは動いているホストを止めない。失敗したときに戻すのは TSF の DLL だけ。どちらもフェーズ 3 で扱う。
- **運用で確かめる**: #57（詰まりの検出）・#50（DM の識別）は、ログの再計測のときに確認してクローズを判断する。

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
