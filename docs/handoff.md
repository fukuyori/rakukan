# Rakukan 引き継ぎ資料

更新日: 2026-09-21（前版は main `2e7e55b` 時点。履歴は git と [CHANGELOG.md](../CHANGELOG.md) を参照）

この文書は「次のセッションが何を知っていれば作業を続けられるか」だけを書く。
設計の全体は [DESIGN.md](DESIGN.md)、現在の作業計画と判断の経緯は [September_Late_Plan.md](September_Late_Plan.md)、9 月前半の作業記録は [September_Revised_Plan.md](September_Revised_Plan.md) にある。

## 1. 現在の状態（2026-09-21、GitHub で確認）

| 項目 | 値 |
|---|---|
| main | `cce2f17`（2026-09-21 に `git pull --ff-only` で origin と一致。#58 / #59 のマージで `2e7e55b` から 4 コミット進んだ）。作業ツリーの変更はこの記録の追記だけ |
| 最新リリース | **0.11.7**（2026-09-14、タグ・GitHub Release あり） |
| 次のリリース番号 | **0.11.9**。0.11.8 は取り消して欠番（タグもリリースも作っていない） |
| engine ABI | **main は 10**（PR #59 のマージ `fb656e1` で 9 → 10。`crates/rakukan-engine/src/ffi.rs` と `crates/rakukan-engine-abi/src/lib.rs` の 2 か所）。**次のインストールでは engine と tsf の両方をビルドし直す**（片方だけだと不一致でエンジンが起動しない） |
| RPC protocol | 5（`crates/rakukan-engine-rpc/src/protocol.rs`）。#56 の実装で上げる見込み |
| `dict_schema` | 2 |
| 開いている PR | 無し。**#58（#53）と #59（#57）は 2026-09-21 に rebase でマージした**。下の「マージした PR と状態」を参照 |
| CHANGELOG | `[Unreleased]` 節は無い。次のリリース作業で作る |
| 作業計画 | [September_Late_Plan.md](September_Late_Plan.md)（段 1〜4。判断事項 J-1〜J-9、9 節に実施記録） |

### 0.11.7 以降に main へ入った未リリースの変更

- **アプリごとの IME 初期状態を config で設定する**（#51 の段 1、PR #52、`8940bb0` / `928e066`）
  - `[input] ime_off_apps`: アプリがアクティブになったら IME をオフにする。既定値は `conhost.exe` / `WindowsTerminal.exe` / `mintty.exe` / `wezterm-gui.exe` / `ghostty.exe`
  - `[input] ime_on_apps`: アクティブになった後、アプリ本体とは別の入力先に入ったときに 1 回だけオンにする。既定値は空
  - どちらも、利用者が IME を操作したらその状態を保ち、インアクティブになったら破棄する。対象アプリでは入力先ごとのモード記憶を使わない
  - ウィンドウクラス名によるターミナル判定（`is_terminal_hwnd`）は削除し、config を唯一の情報源にした
- **RPC クライアントのログを TSF のログに記録する**（#54、`a981b07`。当時のファイル名は `rakukan.log`。#60 でプロセス別に変わった）
  - TSF のログ filter を `LOG_TARGETS`（`rakukan_tsf` / `rakukan_engine_rpc`）にまとめ、ファイル出力と出力先が無い場合で同じ filter を使う。`RAKUKAN_LOG` の優先は維持
  - これで `rpc SLOW`、`spawn_host failed`、`host spawn suppressed`、`Shutdown` の応答などが記録される
- **TSF のログをプロセス別に分け、ローテーションを書き込み処理へ移す**（#60、`e37c645`）
  - `rakukan-tsf-<PID>-<起動識別子>.log` になり、**旧 `rakukan.log` は書かれなくなった**
  - 1 ファイル 16 MiB × 5 世代、TSF ログ全体の保持目安 256 MiB（使用中は消さないので厳密な上限ではない）
  - 調査用に `scripts/merge-logs.ps1` を追加（複数のログを時刻順に統合する）
- **config.toml が読めないときに設定を既定値へ戻さない**（#61、`b31ef5c`）
  - 再初期化は失敗時に直前の設定を保持、初回だけ既定値。どちらもパスとエラー内容を警告に残す
  - ファイル不在も同じ扱い（削除を暗黙の「設定リセット」にしない）
- **MOZC 辞書の取得元を SHA で固定し、差し替えを中断に耐えるようにする**（#62、`fbe7848`）
  - `mozc_rev` = `cbbb6e1bd181cb9f3b409622d916a53a35400ec7`。`build.json` に取得元と SHA を記録する
  - SHA が違う辞書と `mozc_rev` が無い辞書は、`RAKUKAN_FORCE_DICT=1` なしで作り直される
  - 差し替えは `scripts/dictionary-swap.ps1`。回帰テストは `scripts/test-dictionary-swap.ps1`
- **`参` / `拾` が入力の読みで正当化されるときは数字保存の検証で数えない**（#53、PR #58、`9d6ba20` / `5bad674`、nick）
  - 辞書による語と読みの対応付け + 動的計画法による存在判定。照合表で引く読みの長さは `MAX_READING_CHARS = 12` まで、上限超の語は従来の検証に戻す
  - 発動時は `rakukan-engine-dll.log` に `digits::license:` のログ（DEBUG）が出る。**2026-09-21 に実機で発動と変換結果を確認し、#53 をクローズした**
- **変換の詰まりの監視をホストへ移し、実行番号で数える**（#57、PR #59、`fb656e1` / `cce2f17`、nick）
  - TSF 側の `bg_timeout_watchdog` を撤去。エンジン DLL が実行番号と `Running` に入った時刻を持ち、ホストの監視スレッドが 30 秒（`STALL_THRESHOLD`）で確定して自己終了する。口（`StallProbe`）は世代を持ち、確定は口のロック内で世代を照合する
  - engine ABI 9 → 10。復帰マーカーに理由欄（`stall` / `inference_failed`）が付いた
  - 詰まりの検出は**実機未確認**（意図的に詰まりを起こす手段が無い）
- PR #47（`input.text_field_mode`）のマージと 0.11.8 を revert（`7920e1f`）。理由は `September_Revised_Plan.md` の「PR #47 のマージと 0.11.8 を取り消し」節

このほかの 0.11.7 以降のコミットは、計画書・引継書などの文書だけ。

リリース時の更新対象は [version-update-checklist.md](version-update-checklist.md)。
パッケージはレモンが `-Sign` を付けて作成する（Claude 側では作らない）。

### 作業計画の進み具合（September_Late_Plan.md）

| 段 | 内容 | 状態 |
|---|---|---|
| 1 | #53 の影響確認と方針回答、#54 のログ修正、#45 は PR 待ち | #54 はコミット済み・実ログ出力も確認済みで、**1 日分のログ量の確認だけ残る**。**#53 は PR #58 をマージし、実機確認のうえクローズ済み（2026-09-21）**。#45 は PR 待ち |
| 2 | 0.11.9 リリースと 1 週間の再計測（Step 14） | 未着手 |
| 3 | 状態の正しさに関わる修正（#55 / #50 / reload / #49 / config 読込失敗） | **#57 は PR #59 をマージ済み（2026-09-21）。実機未確認のため Issue は開いたまま、運用ログで確認する**。**#56 は nick の設計回答に対して確認事項 5 点を返信し、回答待ち**（実装着手は未承認）。**config 読込失敗（J-5 の (1)）は #61 として完了**。#55 / #50 / #49 と、reload 通知の全プロセス反映（J-5 の (2)、着手予定）は未着手 |
| 4 | 判断済みの機能（#35 本線 / #16）と nick の PR 受け入れ（#45 / #40-2 / #53） | #53 は完了（クローズ済み）。ほかは判断待ち・PR 待ち |

判断事項 J-1〜J-9 のうち、決定済みは J-1（#53。方式・PR の依頼・上限 L = 12 の採用まで）と **J-5**（2 件とも起票する。(1) は #61 として完了、(2) が次の着手予定）。J-2（#45 は依頼済みの PR を待つ）ほかは、計画書の推奨のままレモンの判断待ち。

**PR と重ならない独立した 3 件（#60 / #61 / #62）を 2026-09-19 に起票・実装し、実機確認まで終えてクローズした。** 経緯は [September_Late_Plan.md](September_Late_Plan.md) 9 節の「2026-09-19 PR と重ならない 3 件を起票・実装」と「2026-09-19 #60 / #61 / #62 の実機確認」。

### マージした PR と状態（2026-09-21）

| PR | 対象 | main のコミット | 状態 |
|---|---|---|---|
| [#58](https://github.com/fukuyori/rakukan/pull/58) | #53（`参` / `拾` を数字保存の検証で数えない） | `9d6ba20` / `5bad674` | **マージ済み**（rebase）。依頼した記述の書き分け（「TSV 上では 1 件」「通常語フィルター適用後は未確認」）は PR 本文に反映済み。実機の `digits::license:` の発動と回帰テスト（インストール済み辞書、62 判定すべて期待どおり）を 2026-09-21 に確認し、**#53 はクローズ済み** |
| [#59](https://github.com/fukuyori/rakukan/pull/59) | #57（詰まりの監視をホストへ移す） | `fb656e1` / `cce2f17` | **マージ済み**（rebase）。確認事項 4 件への対応 `e94fe83` を差分・周辺コード・テスト実走で確認し、[返信](https://github.com/fukuyori/rakukan/pull/59#issuecomment-5755508624)。前回指摘 4（`last_status` の経路）は `observe("done")` が `prior_attempts` を 0 に戻すため成立せず、撤回した。実機の詰まり検出は未確認 |

- 両 PR が `conv_cache.rs` と `docs/DESIGN.md` に触れていたが、`git merge-tree` と GitHub の判定とも競合なし。**マージ後の main `cce2f17` の CI は Build & Test / Format Check とも成功**
- **`5bad674`（#58 だけを入れた時点）の main の CI は失敗していた**。`kanji::backend::tests::test_default_model_beam_conversion` が HuggingFace キャッシュのロック取得に失敗（`Download(LockAcquisition(...jinen-v1-small.gguf/blobs/....lock))`）。270 件成功・1 件失敗。**原因は未特定**。同じテストは `cce2f17` で成功している（[run](https://github.com/fukuyori/rakukan/actions/runs/35562159411)）。※推測: 同じモデルを読む 2 テスト（`backend.rs:875` / `:892`）が並列にダウンロードして競合した。main の失敗履歴は 9/1 以降 10 件あり、同じ原因かは未確認
- 過去の PR（#48 / #52）に合わせて rebase マージにした（線形履歴。コミット単位を維持し、SHA は変更）

詳細と根拠は [September_Late_Plan.md](September_Late_Plan.md) 9 節の 2026-09-18〜2026-09-21 の各小節。

### 直近起票した Issue（2026-09-20 / 2026-09-21）

- [#64](https://github.com/fukuyori/rakukan/issues/64) config 不一致の `Create` で、ワーカーが残る旧エンジン DLL をアンロードしうる（2026-09-21）。コード上の成立条件のみで起票、**クラッシュの再現は未実施**。DLL の選択はホストがディスク上の `config.toml` を読み直して行う（TSF の `config_json` に `gpu_backend` は無い）。`rakukan-conv-worker` は `CACHE` の初期化時に起動し（BG 変換・モデル読み込み準備・監視の状態取得のいずれからも）、停止・join の処理が無い。ワーカーの終了を待たずに解放しうる構造は #59 以前から存在し、#59 の `StallProbe` は保持参照と解放タイミングに影響するだけ
- [#63](https://github.com/fukuyori/rakukan/issues/63) 文節変換の土台として、読みと変換結果を文節単位で対応付ける方式の検討（辞書ラティス / 形態素解析 / jinen スコアリング）。jinen は指示追従モデルではなく出力に区切りトークンも無いので、LLM に直接「区切り」を聞くことはできない。かなアンカー法は「にわにはにわにわとりがいる」で成立せず不採用。実装の予定は無く、方式の記録と再開点

### 直近クローズした Issue（2026-09-19 / 2026-09-21）

いずれも実装・コミット・実機確認まで完了。詳細は [September_Late_Plan.md](September_Late_Plan.md) 9 節。

| Issue | 内容 | コミット |
|---|---|---|
| #53 | 数字と「参考」を同じ変換単位で打つと候補が捨てられる（PR #58、nick）。2026-09-21 に実機と回帰テストで受入確認 | `9d6ba20` / `5bad674` |
| #60 | TSF のログがローテーション後も旧世代へ書き込まれ、16 MiB を超えて増え続ける | `e37c645` |
| #61 | config.toml のパースに失敗すると、警告なしで全設定が既定値に戻る | `b31ef5c` |
| #62 | MOZC 辞書を master から取得していて、生成元のリビジョンを固定も記録もしていない | `fbe7848` |

残った未確認は「4. 実機で未確認の項目」を参照。

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
- 型チェックだけなら `cargo make check`、テストは `cargo test --workspace --lib`
- **`--workspace --lib` は lib ターゲットだけなので、バイナリ crate のテストが走らない。** `rakukan-dict-builder` は `src/main.rs` にテストを持つので `cargo test -p rakukan-dict-builder` を併せて実行し、件数を分けて報告する
- PowerShell のスクリプトを変えたら、構文チェック（`[System.Management.Automation.Language.Parser]::ParseFile`）と `scripts/test-dictionary-swap.ps1` を流す。後者は PowerShell 7 と Windows PowerShell 5.1 の両方で確認する
- `PROTOCOL_VERSION` を変えたリリースでは、古いホストが残っていると新しい TSF が接続できない。サインアウト／サインインで解消する

### ファイルの場所

| 用途 | 場所 |
|---|---|
| インストール先 | `%LOCALAPPDATA%\rakukan\`（`%ProgramFiles%` への移行は #33） |
| 設定 | `%APPDATA%\rakukan\config.toml` / `keymap.toml` / `user_dict.toml` |
| 学習履歴 | `%APPDATA%\rakukan\learn_history.bin` |
| 辞書 | `%LOCALAPPDATA%\rakukan\dict\rakukan.dict`（隣に `rakukan.dict.build.json`） |
| ログ | `%LOCALAPPDATA%\rakukan\rakukan-tsf-<PID>-<起動識別子>.log`（TSF。**アプリごとのプロセス単位で分かれる**。時刻順の統合は `scripts/merge-logs.ps1`）/ `rakukan-engine-host.log` / `rakukan-engine-dll.log`。旧方式の `rakukan.log` / `.1`〜`.5` は 0.11.x までのもので、自動削除の対象外（#60） |
| 復帰マーカー | `%LOCALAPPDATA%\rakukan\engine-recovery.txt`（`<exited_at_ms> <attempt> <reason>`、#43 / #57）。理由欄は `stall` / `inference_failed`。旧形式（2 列）は「不明」として読む |

### GitHub

- `origin` = fukuyori/rakukan、`nick` = nick20002005 のフォーク（PR と Issue の主な提案者）
- コミット・リリースの状態を話す前に `git fetch` / `gh` で GitHub 側を確認する（ローカルの追跡参照は古いことがある）
- **コミットメッセージと PR 本文に、完了キーワードと Issue 番号を並べて書かない。** 引用でも GitHub が解釈する。2026-09-15 に #51 がこれで 2 回自動クローズされた。段階的な対応は `Refs` や「#NN の段 1」と書き、Issue は手でクローズする
- Issue の依頼や判断の有無は、コメントを全件読んでから判断する（`gh issue view N --json comments`）。2026-09-17 に、#40 のコメントを途中までしか読まずに「#45 の PR 依頼が出ていない」と誤って報告した
- Issue への投稿は、本文をレモンが確認し「投稿して」の指示があってから行う

## 3. 開いている Issue と状態

| Issue | 内容 | 状態 |
|---|---|---|
| #57 | 長文を Space で変換すると、返っているのにエンジンが再起動されることがある（`bg_timeout_watchdog` の詰まり開始時刻が残る） | **確認済み・設計への意見を求めて返信済み**（2026-09-17、nick。[返信](https://github.com/fukuyori/rakukan/issues/57#issuecomment-5709038255)）。`candidate_window.rs` の `on_waiting_timer` が `bg=done` で `bg_timeout_watchdog(false)` を呼ばないため、古い時刻から数えて閾値を超え `engine_reload_force()` が走る、という報告。2026-09-16 の `rakukan.log` に `conv worker stuck 3618s` / `65s` の実例。手元の修正はフォークの `e8eeac9`（#56 と同じコミット。PR にするなら分けると書かれている）。**主経路はコードと合う**。待機タイマーの上限（10 秒）が監視の閾値（20 秒）より短く、今の詰まりの検出は記録を次の待機まで残すことで成り立っているので、UI の待機の終了で記録を消すと検出が効かなくなる。一方で、観測していない間に実行が完了して別の実行が始まることや、監視している実行・エンジンが置き換わることもあるので、TSF 側で `false` を足す案では足りない。**判断: 監視対象を「同じ実行中の変換」にし、B（エンジン側で実行番号と開始からの経過時間を持ち、ホストが監視する）を軸に設計する**（計画書 3-6）。**実装前の判断 5 点を 2026-09-18 に返信し（[コメント](https://github.com/fukuyori/rakukan/issues/57#issuecomment-5725464861)）、PR #59 をレビュー → 確認事項 4 件の対応 `e94fe83` を確認 → 2026-09-21 にマージ**（`fb656e1` / `cce2f17`、[確認結果の返信](https://github.com/fukuyori/rakukan/pull/59#issuecomment-5755508624)）。**実機で詰まりの検出は未確認**（意図的に起こす手段が無い）なので Issue は開いたまま、運用ログ（`rakukan-engine-host.log` の `conversion stalled reason=stall`、マーカーの `stall`）で確認してからクローズする。閾値 30 秒は「正当な処理でも超えうる誤発動を許容する暫定値」として採用（`GEN_TIMEOUT_SECS` は生成 1 回の上限で、変換 1 回は かな run ごとに生成を呼ぶため k × 15 秒かかりうる）。ABI は 9 → 10 |
| #56 | 未確定文字があるときにホストが再起動すると、Space も Backspace も効かなくなる | **確認済み・設計への意見を求めて返信済み**（2026-09-17、nick。[返信](https://github.com/fukuyori/rakukan/issues/56#issuecomment-5709038552)）。新しいホストの読みバッファが空になり、TSF のセッションと composition だけが未確定文字を持つ、という報告。`handle_action: "Input" state=Preedit("…") bg=idle hira=""` の実例。手元の修正はフォークの `e8eeac9`（composition が生きていてエンジンの読みが空なら、セッション側の読みを `force_preedit` で戻す。Esc 直後は対象外）。**前提（新しいエンジンは読みが空、TSF は合わせない）はコードと合う**。ただし、ホストやエンジンの世代を示す情報が無く、RPC 失敗も「空」になる。エンジンは全アプリで共有。通常入力ではセッションが `Idle` のままで復元の元が無い。`force_preedit` は未確定ローマ字を失う。自動再送で読みを変える要求が二重に適用されうる。**判断: エンジンの世代情報を追加する方向で設計する**。世代だけでは足りず、共有エンジンの所有者（どの composition の読みか）、復元元の編集状態の保持場所（TSF 側）、世代違いを適用前に検出する順序、再送の二重適用も合わせて設計する。未確定ローマ字・打鍵履歴の復元は未確定（計画書 3-7、9 節「#56 / #57 の確認」「#56 / #57 の設計」）。プロトコルの版を上げる見込み。**2026-09-18 にコード照合の結果を返信し、2 度の訂正と回収規則の確認事項を追加した**（[1](https://github.com/fukuyori/rakukan/issues/56#issuecomment-5725537904) / [2](https://github.com/fukuyori/rakukan/issues/56#issuecomment-5725557345) / [3](https://github.com/fukuyori/rakukan/issues/56#issuecomment-5725568117)）。要点: 識別子はプロセス再起動でも衝突しない形にし、世代は要求ごとに照合する／**復元用 RPC を 1 つ設ける**（既存のバッチでは不可分にできない）／判定の優先順は「世代 → 要求番号 → 所有者」で、**所有権の移動では適用済み記録を破棄しない**（破棄すると `Learn` の二重適用が残る）／`input_log` は復元せず、失われるのは「復元**前**の打鍵履歴に依存する機能」。**2026-09-21 に nick から回答**（[コメント](https://github.com/fukuyori/rakukan/issues/56#issuecomment-5754617795)。要求番号は TSF 起動インスタンスごとの単調増加でプロセス static に置く／記録は接続の無いインスタンスだけ回収し `Hello` で有無を返す／結果不明の要求は種類で分け、読みを変える要求は復元後に送り直す／変更 RPC の一覧／`Learn` `Shutdown` `Reload` は期待する世代で止める）。**同日、確認事項 5 点を返信**（[コメント](https://github.com/fukuyori/rakukan/issues/56#issuecomment-5755597587)）: (1) 未応答の要求の寿命と順序、(2) 記録回収と `Hello` の同期・未解決の要求を忘れない、(3) 読み取り RPC（`PreeditIsEmpty` など）も世代・所有者の照合が要る（`on_convert` の `FlushPendingN` → `PreeditIsEmpty` → 空白確定の経路。`preedit_is_empty` は失敗時に `true` を返す）、(4) `Shutdown` / `Reload` の世代不一致を成功扱いにする条件（エンジンの世代とホストの入れ替わりは別。`handle_session` は先に `Shutdown` を記録して応答後に終了する）、(5) 保証は「復元後の編集状態に 1 回分反映」と書く。**nick の回答待ち。#59 はマージ済みなので順序の前提は満たした。実装着手は未承認** |
| #55 | ホストを spawn した後の接続失敗が `HostSpawnGuard` の失敗回数に入らない | 未着手。計画書の段 3（3-1）。#54 は直ったので、ログで発生状況を見られる。発生が 0 件でも修正と再現テストは省略しない方針 |
| #54 | RPC クライアントのログが `rakukan.log` に記録されない | **修正済み・未リリース**（`a981b07`）。インストール版で、言語バーの「エンジン再起動」で `INFO rakukan_engine_rpc::client: rpc: Shutdown acknowledged by host` が記録されることを確認（2026-09-17T02:50:06Z）。**1 日分のログ量の確認が残る。ただし #60 で基準が使えなくなった**（旧 `rakukan.log` は 2026-09-19T10:10:05Z を最後に更新が止まり、以後はプロセス別のファイルになった。以前の基準値 73,033 行・9,736,860 バイトとは比べられない）。**プロセス別ログの合計で測り直す**必要がある。2026-09-19 の中間結果では `rakukan_engine_rpc` の出力が 2.7 日で 6 行・650 バイト、`rpc SLOW` は 0 件だった（低使用日 1 日だけの観測なので結論には足りない）。Issue は開いたまま |
| #51 | アプリごとの IME 初期状態 | 段 1 は main に入った（未リリース）。**段 2 = 設定アプリ（WinUI）での編集画面が残る**。実行中のアプリから選ばせたい。#33 と時期を調整 |
| #50 | 破棄済み DocumentManager のポインタが `dm_modes` に入り直し、アドレス再利用で前のモードが復元される | 未着手。計画書の J-4（推奨: 世代と生存状態を持たせる）と段 3（3-2）。Activate 時の直接呼出し（`factory.rs`）も対象経路 |
| #49 | `model_variant` を変えてもホストが続く限り古いモデルのまま | 未着手。計画書の J-3（推奨: モデル設定の変更でホストを終了）と段 3（3-4）。先に設定の全プロセス反映（3-3）を設計する前提 |
| #46 | 学習履歴を個別に削除する手段が無い | インストーラー改修（#33）と合わせる予定 |
| #45 | 速く打つとライブ変換のプレビューが更新されない | nick の PR 待ち（#40 の 2026-09-12 のコメントで依頼済み） |
| #40 | 候補・予測の品質（統合ブランチ側の報告 7 件） | 1・3 は再現せず、7 は #45、2 は縮小版の PR 待ち（依頼済み）、4 は見送り |
| #35 | 区読点だけの読みとリーダー記号 | 判断待ち。計画書の J-6（推奨: Space 変換の本線を先に、`z` キー列は後）。[Symbol_Leader_Input_Plan.md](Symbol_Leader_Input_Plan.md) の 3.1.1 の A〜C が未決。**PR #31 は 2026-09-13 に nick が取り下げてクローズ済み**（方針が決まったら新しく出すとのこと） |
| #33 | インストーラー再設計（`%ProgramFiles%` 移行、更新は実行 → 再起動で完了） | [October_Install_Plan.md](October_Install_Plan.md) |
| #32 | 文節変換 | 判断待ち。[Segment_Edit_Plan.md](Segment_Edit_Plan.md)。計画書では本計画とインストーラー改修の後。読みと変換結果の対応付け方式は #63 に分けた |
| #64 | config 不一致の `Create` で、ワーカーが残る旧エンジン DLL をアンロードしうる | 2026-09-21 起票。**対処方針の決定と必要な検証は未着手**。再現未実施。対処の方向（案）: 選択される DLL が変わるならホストを終了して作り直す／`Reload` は未使用なので同じ扱いか廃止／設定の全プロセス反映（J-5 の (2)）は古い `config_json` への対策候補だが DLL の寿命管理は別途 |
| #63 | 読みと変換結果を文節単位で対応付ける方式の検討 | 2026-09-20 起票。実装予定なし。方式 1（かなアンカー）は不採用、2（辞書ラティス）/ 3（形態素解析）/ 4（jinen の NLL スコアリング）を比較して記録。進める前に決めること: 文節の定義、発火タイミング、実験の順番 |
| #29 | ユーザー辞書の `priority = "low"` | 保留 |
| #18 | 統合ブランチの棚卸し | 各項目は個別 Issue へ分割済み。記録として残している |
| #16 | 数字混在の読みで、かな run が辞書を参照できない | 判断待ち。計画書の J-7（推奨: 段 4 で設計から）。変換ワーカーへ辞書を渡す配線は #53 と共通になる。`はんは` のように助詞を含む読みも辞書に届かない（2026-09-14 確認） |

#56 / #57 は作業計画の段 3 に別項目で追加した（3-6「#57 修正経路と回帰条件の確認」、3-7「#56 復旧条件・状態復元の設計」）。コードでの確認結果は計画書 9 節「#56 / #57 の確認」、設計（案）は「#56 / #57 の設計」、2026-09-18 以降の判断は同節の日付ごとの小節。**PR は分けて #57 を先に入れる順序で合意済み**。#57 は ABI を 9 → 10、#56 はプロトコルの版を上げる見込み。

Issue への投稿・レビューは、いずれも本文をレモンが確認してから送っている。#53 / #57 / #56 の返信では、**結論を「実際に確かめた範囲」に限定して書く**ことを繰り返し求められた（「TSV 上では 1 件」「同一ホスト世代内の重複のみ」「復元前に打った分だけ」「その計測条件での値」）。断定できない箇所は撤回して書き直す。

## 4. 実機で未確認の項目

| 項目 | 補足 |
|---|---|
| `ime_off_apps` の既定値 `conhost.exe` / `WindowsTerminal.exe` / `mintty.exe` | 一致したログの記録が無い。wezterm-gui.exe / ghostty.exe は確認済み |
| `ime_on_apps` で Photoshop の文字ツール | 環境が無い。firefox / msedge / chrome は確認済み |
| Step 13-1「モデルが ready になったら自動で変換をやり直す」 | モデル読み込みが 157ms で終わり手で狙えない。コールドスタート時に確認する |
| Step 13-2 `edit_session: SLOW grant` | 閾値を超えたときだけ出る。0 件は「観測期間中に閾値超過を記録しなかった」と扱う |
| Step 13-2 `rpc SLOW` | #54 の修正で**記録されるようになった**（出力経路は確認済み）。インストール直後の変換（約 380 行分）では 0 件。1 週間の再計測（段 2）で件数を見る |
| #54 の 1 日分のログ量 | **#60 で旧 `rakukan.log` が止まったため、以前の基準値とは比べられない。** プロセス別ログの合計で測り直す（段 2 の 1 週間の再計測に相乗りさせる） |
| #60 ログの 16 MiB 退避 | 主要な確認は 2026-09-19 に完了。退避だけはその量に達しておらず未確認（日常利用で自然に到達する） |
| #61 初回パース失敗時の既定値 | 主要な確認は 2026-09-19 に完了。初回だけは DLL 読み込み前に壊す必要があり未確認（単体テストでは確認済み） |
| #62 SHA 一致時のスキップ | 次回のインストールで `already built ... mozc cbbb6e1bd181, skipping` が出れば確認できる |
| #62 同時インストール全体の安全性 | ロックが守るのは復旧・差し替えの実行中だけ。取得・ビルドを含む全体は未検証 |
| #62 辞書のバイト単位の一致 | SHA の固定が保証するのは取得元データの一致まで。生成物の一致は別の検証事項 |
| #58 の上限で根拠を失う語の件数 | TSV 上は `吉祥院新田参ノ段` の 1 件。**通常語の帯（`cost_band::Class::Normal`）の絞り込みを適用した後の件数は未確認** |
| #53（PR #58）の照合表の作成時間 | 実機で初回 5〜33 ms、2 回目約 0.1 ms（`table=` の値）。**差の原因は未特定**。PR の計測（120 文字で 0.81 ms）は暖まった状態の値。発動ログと変換結果そのものは 2026-09-21 に確認済み |
| #57（PR #59）の詰まりの検出 | **マージ済み・実機未確認**。詰まりを意図的に起こす手段（#43 の `force_inference_failure` に相当する診断設定）は無い。運用ログで `conversion stalled reason=stall` が出るのを待つ。同じ DLL を `Reload` すると世代だけ進んでキャッシュと実行番号は同じなので、`unrecoverable` でホストが生きている間は同じ `run_id` の発動ログが 2 回出うる（設計どおり） |
| #57 の `last_status` が `"done"` のまま回復処理を素通りする経路 | **成立しないことをコードで確認した**（`observe("done")` が `prior_attempts` を 0 に戻すため、その後の詰まりは `ExitHost` になる）。番兵 `"<stalled>"` は保険として入った。指摘は撤回済み |

## 5. 既知の問題（Issue 化していないもの）

- **config の再読込がアプリごとにばらばら**。reload イベント `Local\rakukan.engine.reload` が auto-reset のため、1 回の Set で 1 プロセスにしか届かない。計画書の J-5 の (2)。**起票して着手することは決定済み**（`state.rs` を触る。PR #59 はマージ済みなので重複の心配は無くなった）
- **CI の `test_default_model_beam_conversion` が HuggingFace キャッシュのロック取得で落ちることがある**（`5bad674` の main で 1 回。次の run では通った）。※推測: 同じモデルを読む 2 テストの並列ダウンロード。main の失敗履歴 10 件が同じ原因かは未確認
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
| [September_Late_Plan.md](September_Late_Plan.md) | **現在の作業計画**。段 1〜4、判断事項 J-1〜J-9、9 節に実施記録（#54 の修正と確認、#53 の設計と PR #58 のレビュー・マージ、#56 / #57 の設計と PR #59 のレビュー・マージ、#60 / #61 / #62 の起票・実装・実機確認、#63 の起票、#56 の確認事項 5 点） |
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
