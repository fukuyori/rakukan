# 10 月改修計画: インストール・更新・削除の再設計

作成: 2026-09-07
着手: 2026 年 10 月（`September_Revised_Plan.md` の Step 10〜13 完了後）
対象: 0.12 系
位置づけ: 2026-09-07 の検討（PR #30 の再検討から派生）で決めた方針を、実装計画として記述する
関連 Issue: #33（問題点の記録）、PR #30、#8

## 1. 背景と目的

rakukan の更新は、TSF DLL が IME を使った各アプリに読み込まれたままになるため、
利用者に「IME を切り替え → サインアウト → サインイン → インストーラー実行」という
準備を求めてきた。この手順は IME 固有で分かりにくく、サイレント実行（winget など）では成立しない。
また、TSF の登録は HKLM（管理者必須）なのにファイルは `%LOCALAPPDATA%\rakukan`（ユーザー別）に
置いており、権限と配置が一致していない。ソースからの `cargo make install`（`scripts/install.ps1`）と
パッケージ（`rakukan_installer.iss`）でロック対処が別実装になっている点も保守負担になっている。

初期の rakukan で Windows が不安定になった経験からサインアウト前提の手順を採ってきたが、
当時は原因が切り分けられておらず、差し替え方式が原因だと確認されたわけではない。

目的:

- 初回インストール・更新・削除を、一般的な Windows アプリと同じ手順にする。
- 更新前の準備を不要にし、更新後の案内を「一度サインアウトして再サインイン（再起動不要）」の 1 行にする。
- ロック対処をインストーラー 1 か所に集め、ソースからの install も同じ経路にする。
- winget から導入・更新・削除できるようにする。

## 2. 決定事項

| 項目 | 決定 | 理由 |
|---|---|---|
| 更新時の DLL 差し替え | 使用中の DLL は `<name>.locked-<日時>` へ改名退避し、新 DLL を本来の名前で置く（PR #30 の方式） | 改名はマップ済みでも通る。新規起動プロセスは即座に新版を読む |
| 退避ファイルの後始末 | 次回起動時の遅延削除（`MoveFileEx` の `MOVEFILE_DELAY_UNTIL_REBOOT`）を予約する | 自前の掃除処理を持たない。利用者が意識しなくても消える |
| 更新後の案内 | 「一度サインアウトして再サインインしてください（再起動は不要）」 | 起動中のアプリを新版に揃えるにはサインアウトが必要。再起動は不要 |
| 配置 | `%ProgramFiles%\rakukan`（マシン単位、管理者） | TSF 登録が HKLM である以上、ファイルもマシン単位が一貫する。別ユーザーでも動く |
| ユーザー単位インストール | 採らない | TSF が HKCU 登録だけで動くか未検証。コードが 2 通りの配置を扱うことになる |
| モデル | `%ProgramData%\rakukan\models` で全ユーザー共有 | 数百 MB をユーザーごとに持つ理由がない |
| ユーザーデータ | `%APPDATA%\rakukan`（設定・keymap・ユーザー辞書・学習履歴）、`%LOCALAPPDATA%\rakukan`（ログ）のまま | 変更不要 |
| 既存利用者の移行 | 新インストーラーが `%LOCALAPPDATA%\rakukan` の旧 DLL を登録解除・退避し、遅延削除を予約する | 手動アンインストールを案内すると「他のアプリと同じ」にならない |
| ソースからの install | `cargo make install` = インストーラーをビルドしてサイレント実行 | 利用者と開発者の経路を同じにし、install.ps1 固有のバグをなくす |
| Inno Setup | 開発環境の必須前提に加える | 経路一本化の帰結。`check-env` / `setup-env` に追加 |
| 削除 | 「アプリと機能」または `winget uninstall`。ユーザーデータは残す | 一般アプリの慣習どおり |
| 旧 DLL と新 host の混在 | 旧 TSF DLL の Hello が `PROTOCOL_VERSION` 不一致で失敗したら「エンジン未準備」と同じ扱い（無変換、ログ出力） | 混在はサインアウトまでの短時間。文字入力は壊さない |
| 開発専用の差し替え経路 | 持たない | 改名方式が本線なので不要 |
| 配布 | GitHub Releases の exe を継続し、winget に登録 | winget は Releases の exe を参照する |
| Microsoft Store | 対象外（調査のみ） | MSIX は配置・登録の前提が異なる。TSF IME を MSIX で配布できるか未確認 |
| PR #30 | 方式採用の形で結論をコメントする | close 理由だった「初期に試して駄目だった」は根拠にならない |

## 3. 目指す構造

配布物は `rakukan-<version>-setup.exe`（Inno Setup）の 1 つ。経路は 1 本。ロック対処はインストーラーだけが持つ。

| 操作 | 利用者がすること | 内部の動き | 事後の操作 |
|---|---|---|---|
| 初回 | exe 実行、UAC 承認 | ファイル配置、regsvr32、TIP 登録、言語リストへ追加 | なし |
| 更新 | 同じ exe 実行（または `winget upgrade`） | tray / host / 設定アプリを停止して置換。使用中の TSF DLL は改名退避して新版を配置。退避ファイルの遅延削除を予約 | サインアウト → サインイン |
| 削除 | 「アプリと機能」（または `winget uninstall`） | 言語リストから除去、regsvr32 /u、ファイル削除。ロード中の DLL は遅延削除 | サインアウト → サインイン |

配置:

| 場所 | 内容 |
|---|---|
| `%ProgramFiles%\rakukan` | `rakukan_tsf.dll`、`rakukan_engine_{cpu,vulkan,cuda}.dll`、`rakukan-engine-host.exe`、`rakukan-tray.exe`、`settings-ui\`、`dict\rakukan.dict`、`register-tip.ps1` / `unregister-tip.ps1`、NOTICE 類 |
| `%ProgramData%\rakukan\models` | GGUF モデル（engine-host がダウンロード・読込。全ユーザー書込可） |
| `%APPDATA%\rakukan` | `config.toml`、`keymap.toml`、`user_dict.toml`、`learn_history.bin` |
| `%LOCALAPPDATA%\rakukan` | `rakukan.log`、`rakukan-engine-host.log`、`rakukan-engine-dll.log` |

## 4. 変更内容

### 4.1 インストーラー（`rakukan_installer.iss`）

- `DefaultDirName` を `{autopf}\rakukan` に変更。`PrivilegesRequired=admin` は維持。
- `CheckDllLock`、`InstallFail` のサインアウト案内、`.backup` によるロールバックを削除。
- `[Code]` で DLL 差し替えを実装: 上書きに失敗したら `RenameFile` で `<name>.locked-<日時>` へ退避し、
  退避ファイルに対して `MoveFileExW(path, NULL, MOVEFILE_DELAY_UNTIL_REBOOT)` を呼ぶ
  （`external` 宣言で Win32 API を直接呼ぶ。書き方は着手前に確認）。対象は `rakukan_tsf.dll` と
  `rakukan_engine_*.dll`。
- 移行処理: `%LOCALAPPDATA%\rakukan\rakukan_tsf.dll` が存在すれば、regsvr32 /u → 退避 → 遅延削除予約を
  一度だけ行う。旧ディレクトリの `dict\` は不要になるので削除（`registered.txt` も同様）。
- `FinishedLabel` を「更新の場合は一度サインアウトして再サインインしてください（再起動は不要）」に変更。
- `[UninstallRun]` は現行どおり（unregister-tip → regsvr32 /u）。DLL に `uninsrestartdelete` を付け、
  ロード中でも削除できるようにする。
- `[Files]` の `config.toml` 初期配置（`onlyifdoesntexist uninsneveruninstall`）は維持。

### 4.2 コード

| 箇所 | 現状 | 変更 |
|---|---|---|
| `rakukan-engine-abi::install_dir()` | `%LOCALAPPDATA%\rakukan` 固定 | `GetModuleFileNameW` で自モジュール（TSF DLL / host exe）のディレクトリを返す。コメントには元々その意図が書かれている |
| `rakukan-engine-rpc::client::spawn_host()` | `install_dir()` + `rakukan-engine-host.exe` | 変更不要（`install_dir()` の修正で追随） |
| `rakukan-dict` の辞書ディレクトリ | `%LOCALAPPDATA%\rakukan\dict` 固定 | `install_dir()\dict` に変更 |
| `rakukan-engine::kanji::hf_download` | `%USERPROFILE%\.cache\huggingface\hub` に取得 | `%ProgramData%\rakukan\models` を第一候補にし、既存の HF キャッシュは移行期間の読み取りフォールバックとして残す |
| `rakukan-tsf` の RPC 接続 | Hello の版不一致は `bail!` | 「エンジン未準備」と同じ状態に倒し、`rakukan.log` に版不一致を出す |
| `settings_launcher` | DLL のディレクトリ基準 | 変更不要 |
| ログ・設定パス | `%LOCALAPPDATA%` / `%APPDATA%` | 変更不要 |

### 4.3 ビルド・開発経路

- `Makefile.toml`: `install` タスクを `build-installer` → `output\rakukan-<version>-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART` の実行に置き換える。`full-install` / `quick-install` は
  `build-engine` / `build-tsf` との依存関係だけ残す。`uninstall` はインストーラーのアンインストーラーを
  サイレント実行する形にする。
- `scripts/install.ps1` を削除。担っていた処理の行き先:
  - コピー・登録・tray 自動起動登録 → インストーラー
  - mozc 辞書の取得と `rakukan-dict-builder` によるビルド（[4a]） → `scripts/build-installer.ps1` の dist 準備段階へ移す（辞書は配布物に同梱済みのため、利用者の PC では行わない）
  - モデルの事前ダウンロード（[4b]） → 廃止（engine-host が初回変換時に取得する現行動作に任せる）
- `scripts/uninstall.ps1` を削除（インストーラーのアンインストーラーに一本化）。
- `scripts/check-env.ps1` / `setup-env.ps1`: Inno Setup 6 を必須に格上げ。`setup-env` は winget で導入する。
- `README.md`: 「インストール（ソースから）」の手順を「`cargo make build-engine` → `cargo make build-tsf` → `sudo cargo make install` → サインアウト → サインイン」に変更。「インストール（パッケージから）」の更新手順から事前のサインアウトを削除。「ビルド前提」の Inno Setup を必須へ。
- `.claude/CLAUDE.md`（git 管理外）の手順も同時に更新する。`scripts/check-install-instruction.ps1` の検出条件と文言を新手順に合わせる。

### 4.4 winget

- `winget-pkgs` に manifest を登録する。InstallerType は `inno`、サイレント引数は `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`、InstallerUrl は GitHub Releases の exe。
- `Scope: machine`。アンインストールは ARP 登録（`AppId`）経由。
- `docs/version-update-checklist.md` に「リリース後に winget manifest を更新する」を追加する。

## 5. 作業ステップ

各ステップは「変更 → 検証 → 完了判定」の順に進め、未解決の回帰を次へ持ち越さない。

| Step | 内容 | 完了条件 |
|---|---|---|
| 1 | コードのパス解決を配置非依存にする（4.2 の `install_dir()`、辞書ディレクトリ、モデルディレクトリ、RPC 版不一致の扱い） | 現行の `%LOCALAPPDATA%` 配置のまま全テストと実機動作が変わらない。`%ProgramFiles%` に手動配置しても動く |
| 2 | インストーラーを新方式にする（4.1） | 新規 PC 相当（旧配置なし）で初回インストール → 変換 → 更新（サインアウトなしで完走） → サインアウト・サインイン後に新版で動作 → 再起動後に `.locked-*` が消えている → 削除で登録とファイルが消える |
| 3 | 既存利用者の移行 | `%LOCALAPPDATA%\rakukan` に旧版がある状態から新インストーラーで更新し、旧 DLL の登録が消え、新配置で動作する |
| 4 | 経路一本化（4.3）。install.ps1 / uninstall.ps1 の削除、Makefile、check-env / setup-env、README、CLAUDE.md、Stop hook | `cargo make build-engine` → `build-tsf` → `install` が完走し、README の手順どおりに反映できる |
| 5 | winget 登録（4.4）。リリース後に manifest を提出 | `winget install` / `upgrade` / `uninstall` が通る |
| 6 | PR #30 に結論コメント、`handoff.md` と `DESIGN.md` の配置記述を更新、CHANGELOG | 文書が実装と一致 |

## 6. 検証項目

- 更新時: 更新前から IME を使っていたアプリ（メモ帳、ブラウザ、Explorer の検索欄）が更新直後も入力を受け付け、変換不能でも文字入力は壊れない。サインアウト → サインイン後に新版で変換できる。
- `PROTOCOL_VERSION` を意図的に変えたビルドで更新し、旧 DLL のアプリが無変換になり `rakukan.log` に版不一致が出る。
- 再起動後に `.locked-*` が消えている（`PendingFileRenameOperations` に登録されていることを事前に確認）。
- 別ユーザーアカウントでサインインし、「キーボードの追加」で rakukan を選べ、変換できる（モデルは共有ディレクトリから読める）。
- 削除後にレジストリの CLSID / TIP 登録と言語リストの項目が残っていない。ユーザーデータは残っている。
- Explorer 等の異常終了がイベントビューアーに出ていない（更新・サインアウト・再起動の各段階）。
- サイレント実行（`/VERYSILENT`）で対話が一切出ず、終了コードが期待どおり。

## 7. 着手前に確認すること

- Inno Setup の `[Code]` から `MoveFileExW` を `external` 宣言で呼ぶ書き方と、`MOVEFILE_DELAY_UNTIL_REBOOT` が管理者権限下で有効なこと。
- Inno Setup がサイレント実行時に返す終了コードと、winget manifest の `ReturnCodes` / `ExpectedReturnCodes` の対応。winget の要件（公開 URL、ARP 登録、署名の要否）。
- `%ProgramData%\rakukan\models` の ACL 設定をインストーラーで行う方法（全ユーザー書込可）。
- `%ProgramFiles%` 配置での `rakukan-dict-builder.exe` の扱い（配布物から外せるか）。
- `hf_download` が書き込む先を `%ProgramData%` に変えたときの、既存の `.cache\huggingface` にあるモデルの扱い（コピーするか、読み取りフォールバックのみか）。

## 8. 対象外

- Microsoft Store（MSIX）への対応。TSF IME を MSIX で配布できるかの調査のみ行い、結果を本書に追記する。
- ユーザー単位（管理者不要）インストール。要望が出た時点で HKCU 登録の検証を条件に検討する。
- 電子署名の必須化。ストア対応を判断する段階で再検討する。

## 9. 現行手順（本計画の実施まで）

本計画の実施までは現行の手順を変えない。

1. `cargo make build-engine` / `cargo make build-tsf`（変更箇所に応じて）
2. サインアウト
3. サインイン
4. `sudo cargo make install`
