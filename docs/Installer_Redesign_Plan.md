# インストール・更新・削除の再設計計画

作成: 2026-09-07

更新: 2026-10-10（更新の方式を「使用中のファイルは再起動時に置き換え、再起動で完了」に改めた。旧ホスト停止の先行実装は取りやめ。経緯は 1 章の末尾と #33 のコメント https://github.com/fukuyori/rakukan/issues/33#issuecomment-6092071876 ）

着手時期: 未定（v6 のリリースの後）。**本計画の実施までは、現在のインストール方法を変えない**（9 章）

対象: 0.12 系

位置づけ: 2026-09-07 の検討（PR #30 の再検討から派生）で決めた方針を、実装計画として記述する

関連 Issue: #33（問題点の記録と方式の決定）、PR #30、#8、#56（プロトコル版上げ時の旧ホスト残存）

## 1. 背景と目的

rakukan の更新は、TSF DLL が IME を使った各アプリに読み込まれたままになるため、
利用者に「IME を切り替え → サインアウト → サインイン → インストーラー実行」という
準備を求めてきた。この手順は IME 固有で分かりにくく、サイレント実行（winget など）では成立しない。
また、TSF の登録は HKLM（管理者必須）なのにファイルは `%LOCALAPPDATA%\rakukan`（ユーザー別）に
置いており、権限と配置が一致していない。ソースからの `cargo make install`（`scripts/install.ps1`）と
パッケージ（`rakukan_installer.iss`）でロック対処が別実装になっている点も保守負担になっている。

2026-09-25 に #56 の Draft PR #67 を確認した際、現行の配布インストーラーは `CloseApplications=no` で、
`[Code]` に `rakukan-engine-host.exe` の停止処理が無いことを確認した。#56 で `PROTOCOL_VERSION` を 5 → 6 に
更新したため、更新時に旧ホストと新 TSF（またはその逆）が混在しうる。

2026-09-30 には、v6 の配布前に旧ホストの停止処理を現行インストーラーへ先行実装する方針とした。
2026-10-08〜10 にその設計を進めたが、他の IME（Mozc、CorvusSKK）との比較と要件の見直しの結果、
**旧ホストの停止は行わず、CorvusSKK と共通の方針（使用中のファイルは再起動時に置き換え、完了時に再起動を求める）を
採る**ことにした（2026-10-10）。先行実装は取りやめ、v6 は現在のインストール方法で配布する。

- 旧ホストを止めると、動いているアプリの旧 TSF が新しいホストを起動して接続できない。止める相手の特定・中断・戻しの設計も大きい。
- 当初の「使用中の DLL を改名退避して新版をその場に置き、再起動またはサインアウト・サインインで完了」は、サインアウトまでに新しく起動したアプリが新しい TSF を読み込み旧ホストと混在するため、「切り替えまで旧状態で動く」ことを満たさない。満たすにはローダー DLL で版を切り替える仕組みが要り、規模が大きい。
- 完了を「再起動」に限り、再起動までは Rakukan が使えないことがあるのを制限として受け入れる。

初期の rakukan で Windows が不安定になった経験からサインアウト前提の手順を採ってきたが、
当時は原因が切り分けられておらず、差し替え方式が原因だと確認されたわけではない。

目的:

- 初回インストール・更新・削除を、一般的な Windows アプリと同じ手順にする。
- 更新前の準備を不要にし、インストーラー実行後の**再起動**でインストールを完了する。
- ロック対処をインストーラー 1 か所に集め、ソースからの install も同じ経路にする。
- winget から導入・更新・削除できるようにする。

## 2. 決定事項

| 項目 | 決定 | 理由 |
|---|---|---|
| 更新時のファイル差し替え | `[Files]` のファイルに `ignoreversion restartreplace uninsrestartdelete` を付ける。置き換え・削除に失敗し、Inno が使用中の可能性があると判断したファイルは、次の Windows の起動時に置き換える（2026-10-10 に改名退避から変更） | 旧ホストを止めずに済む。CorvusSKK（`installer/installer.iss`）と共通の方針 |
| 旧ホスト | 停止しない（2026-10-10 に変更） | 止めると動いているアプリの旧 TSF が接続できない。プロセスの特定・中断・戻しが不要になる |
| インストール完了の案内 | 「インストールを完了するには、PC を再起動してください。再起動するまでは、Rakukan による日本語入力ができないことがあります」。`AlwaysRestart=yes` / `UninstallRestartComputer=yes`（2026-10-10 に変更。サインアウト・サインインは完了の条件にしない） | `restartreplace` は再起動時に働く |
| 再起動までの制限 | 使用中のファイルは再起動まで旧版、それ以外はその場で新版になるため、再起動までは Rakukan が使えないことがある（旧 TSF＋新ホスト、新 TSF＋旧ホスト、旧ホスト＋新 engine DLL）。これは受け入れる。文字の取りこぼし・データの破損などの許容範囲は別に定めて確かめる | 要件を「再起動で完了」に限ったため |
| 配置 | `%ProgramFiles%\rakukan`（マシン単位、管理者） | TSF 登録が HKLM である以上、ファイルもマシン単位が一貫する。別ユーザーでも動く |
| ユーザー単位インストール | 採らない | TSF が HKCU 登録だけで動くか未検証。コードが 2 通りの配置を扱うことになる |
| モデル | `%ProgramData%\rakukan\models` で全ユーザー共有 | 数百 MB をユーザーごとに持つ理由がない |
| ユーザーデータ | `%APPDATA%\rakukan`（設定・keymap・ユーザー辞書・学習履歴）、`%LOCALAPPDATA%\rakukan`（ログ）のまま | 変更不要 |
| 既存利用者の移行 | 新インストーラーが `%LOCALAPPDATA%\rakukan` の旧 DLL を登録解除し、旧配置のファイルは再起動時の削除を予約する | 手動アンインストールを案内すると「他のアプリと同じ」にならない |
| ソースからの install | `cargo make install` = インストーラーをビルドしてサイレント実行 | 利用者と開発者の経路を同じにし、install.ps1 固有のバグをなくす |
| Inno Setup | 開発環境の必須前提に加える | 経路一本化の帰結。`check-env` / `setup-env` に追加 |
| 削除 | 「アプリと機能」または `winget uninstall`。ユーザーデータは残す。完了時に再起動を求める | 一般アプリの慣習どおり。ロード中の DLL は再起動時に削除 |
| 旧 TSF と新 host、新 TSF と旧 host の混在 | Hello を完了できない場合は「エンジン未準備」と同じ扱い（無変換、ログ出力）。再起動までの制限として受け入れる | 異なる通信形式を適用しない |
| 開発専用の差し替え経路 | 持たない（故障試験用の TSF DLL 差し替えは 4.3 のとおり残す） | インストーラーが本線 |
| 配布 | GitHub Releases の exe を継続し、winget に登録 | winget は Releases の exe を参照する |
| Microsoft Store | 対象外（調査のみ） | MSIX は配置・登録の前提が異なる。TSF IME を MSIX で配布できるか未確認 |
| PR #30 | 結論をコメントする（改名退避は採らず、再起動時の置き換えにした経緯） | close 理由だった「初期に試して駄目だった」は根拠にならない |

### 採らなかった方式（2026-10-10）

| 方式 | 採らない理由 |
|---|---|
| 旧ホストを止めてから置き換える（9/30 の先行実装の方針） | 動いているアプリの旧 TSF が新しいホストを起動して接続できない。止める相手の特定（パス・セッション・PID の再利用）、中断と戻しの設計が大きい |
| 使用中のファイルを改名退避して、その場で新版を置く（当初の方式。PR #30） | サインアウトまでに新しく起動したアプリが新しい TSF を読み込み、旧ホストに接続できない |
| すべてのファイルを待機用フォルダに置き、再起動時に一括で置き換える | 再起動まで旧版のまま動かせるが、上の制限を受け入れるので採らない |
| 版ごとのフォルダとローダー DLL で、ログオンの境界で切り替える | サインアウトでも切り替えられるが、ローダー・版の割り当て・複数セッション・昇格・32 ビット・AppContainer などの設計が大きい |
| Mozc の方式（MSI の更新処理に加え、実行時にクライアントが古いサーバーを検出して終了させ、再接続する） | MSI と実行時の自己修復が前提。rakukan の TSF には古いホストを見分けて止める仕組みが無い |

## 3. 目指す構造

配布物は `rakukan-<version>-setup.exe`（Inno Setup）の 1 つ。経路は 1 本。ロック対処はインストーラーだけが持つ。

| 操作 | 利用者がすること | 内部の動き | 事後の操作 |
|---|---|---|---|
| 初回 | exe 実行、UAC 承認 | ファイル配置、登録、TIP 登録、言語リストへ追加 | 再起動 |
| 更新 | 同じ exe 実行（または `winget upgrade`） | 旧ホストは止めない。ファイルを置き換え、使用中のものは再起動時の置き換えを予約 | 再起動 |
| 削除 | 「アプリと機能」（または `winget uninstall`） | 言語リストから除去、登録解除、ファイル削除。ロード中のものは再起動時に削除 | 再起動 |

配置:

| 場所 | 内容 |
|---|---|
| `%ProgramFiles%\rakukan` | `rakukan_tsf.dll`、`rakukan_engine_{cpu,vulkan,cuda}.dll`、`rakukan-engine-host.exe`、`rakukan-tray.exe`、`settings-ui\`、`dict\rakukan.dict`、`register-tip.ps1` / `unregister-tip.ps1`、NOTICE 類 |
| `%ProgramData%\rakukan\models` | GGUF モデル（engine-host がダウンロード・読込。全ユーザー書込可） |
| `%APPDATA%\rakukan` | `config.toml`、`keymap.toml`、`user_dict.toml`、`learn_history.bin` |
| `%LOCALAPPDATA%\rakukan` | `rakukan-tsf-<PID>-<起動識別子>.log`、`rakukan-engine-host.log`、`rakukan-engine-dll.log` |

## 4. 変更内容

### 4.1 インストーラー（`rakukan_installer.iss`）

- `DefaultDirName` を `{autopf}\rakukan` に変更。`PrivilegesRequired=admin` は維持。
- `[Files]` のファイルに `ignoreversion restartreplace uninsrestartdelete` を付ける。`config.toml` の初期配置（`onlyifdoesntexist uninsneveruninstall`）は維持。
- `AlwaysRestart=yes`、`UninstallRestartComputer=yes`。
- 完了の案内（実際に表示されるメッセージ。`[Icons]` が無い場合は `FinishedLabelNoIcons`）を 2 章の文言にする。英語版も同様。
- 旧ホストを止める処理は作らない。
- `CheckDllLock`（現行は `[Tasks]` が無く呼ばれていない）、`InstallFail` のサインアウト案内、`.backup` によるロールバックを削除または見直す（7 章）。
- 登録: 現行の `[Run]` の `regsvr32` と `ssInstall` での旧 DLL の登録解除を、`restartreplace` と組み合わせた形に改める。Inno の `regserver` フラグを使う場合は 64 ビットのインストールモードとフラグも決める（7 章）。
- キーボードリストへの追加: 現行の `register-tip.ps1` は `postinstall` で、再起動が必要な場合は実行されない。実行するユーザー・順序を決める（7 章）。
- 移行処理: `%LOCALAPPDATA%\rakukan\rakukan_tsf.dll` が存在すれば、登録解除し、旧配置のファイルは再起動時の削除を予約する。旧ディレクトリの `dict\`（`registered.txt` も同様）も同じ。
- `[UninstallRun]` は現行どおり（unregister-tip → regsvr32 /u）。ファイルには `uninsrestartdelete` を付ける。
- サイレント実行ではダイアログに依存せず、失敗を終了コードとログで通知する。

### 4.2 コード

| 箇所 | 現状 | 変更 |
|---|---|---|
| `rakukan-engine-abi::install_dir()` | `%LOCALAPPDATA%\rakukan` 固定 | `GetModuleFileNameW` で自モジュール（TSF DLL / host exe）のディレクトリを返す。コメントには元々その意図が書かれている |
| `rakukan-engine-rpc::client::spawn_host()` | `install_dir()` + `rakukan-engine-host.exe` | 変更不要（`install_dir()` の修正で追随） |
| `rakukan-dict` の辞書ディレクトリ | `%LOCALAPPDATA%\rakukan\dict` 固定 | `install_dir()\dict` に変更 |
| `rakukan-engine::kanji::hf_download` | `%USERPROFILE%\.cache\huggingface\hub` に取得 | `%ProgramData%\rakukan\models` を第一候補にし、既存の HF キャッシュは移行期間の読み取りフォールバックとして残す |
| `rakukan-tsf` の RPC 接続 | Hello の版不一致は `bail!` | Hello の失敗を「エンジン未準備」と同じ状態に倒し、該当プロセスの `rakukan-tsf-<PID>-<起動識別子>.log` に理由を残す。版番号を受け取れた場合と、Hello 自体をデコードできなかった場合を区別する |
| `settings_launcher` | DLL のディレクトリ基準 | 変更不要 |
| ログ・設定パス | `%LOCALAPPDATA%` / `%APPDATA%` | 変更不要 |

### 4.3 ビルド・開発経路

- **`scripts/build-installer.ps1` の入力をインストール先からビルド出力へ変える**（2026-09-25 のレビューで追加）。現行はTSF DLL・engine DLL 3 種・host・settings-ui・モデルを `-InstallDir`（既定 `%LOCALAPPDATA%\rakukan`）から、辞書を `%LOCALAPPDATA%\rakukan\dict` から集め、無ければ「先に `cargo make install`」と言って止まる。`install` から `build-installer` を呼ぶ形にすると循環するので、**全成果物を `-BuildDir`（既定 `C:\rb`）の `release` 配下と WinUI の publish 出力から集める**形にし、`-InstallDir` と「`cargo make install` 済み」の前提を外す。辞書は下記のとおり dist 準備段階で作る。モデルは同梱しない（現行の「存在する場合はコピー」も外す。取得は engine-host の初回変換時）。
- `Makefile.toml`: `install` タスクを `build-installer`（署名なし）→ `output\rakukan-<version>-setup.exe /VERYSILENT /SUPPRESSMSGBOXES /NORESTART` の実行に置き換える（完了には再起動が要る）。`quick-install` は `build-tsf` → `install`、`full-install` は `build-engine` → `build-tsf` → `install` の開発用に限定し、`sign` を外す。`uninstall` はインストーラーのアンインストーラーをサイレント実行する形にする。
- **リリース向けの署名経路を開発用と分けて定める**（同レビューで追加）。`cargo make package`（仮称）を新設し、`sign`（`sign-artifacts.ps1`。`C:\rb\release` の DLL / EXE に署名）→ `build-installer.ps1 -Sign`（セットアップ本体と、`SignedUninstaller=yes` によるアンインストーラーに署名）の順に実行する。`build-installer` が署名済みの成果物を集めるよう、`sign` を先にする。**パッケージ作成はレモンが `-Sign` 付きで実行する**（Claude は行わない。`CLAUDE.md` のとおり）。開発用の `install` は署名しないサイレント導入で、配布物には使わない。
- `scripts/install.ps1` を削除。担っていた処理の行き先:
  - コピー・登録・tray 自動起動登録 → インストーラー
  - mozc 辞書の取得と `rakukan-dict-builder` によるビルド（[4a]） → `scripts/build-installer.ps1` の dist 準備段階へ移す（辞書は配布物に同梱済みのため、利用者の PC では行わない）
  - モデルの事前ダウンロード（[4b]） → 廃止（engine-host が初回変換時に取得する現行動作に任せる）
  - **`-TsfOnly`（#65 の故障試験用に TSF DLL だけを差し替えて再登録する経路。`Makefile.toml` の `install-tsf-fault-test` / `install-tsf-normal-restore` が呼ぶ）** → 開発専用の `scripts/dev-swap-tsf.ps1`（仮称）として切り出す。処理は現行の `-TsfOnly` と同じ（DLL のみコピー → SHA256 一致確認 → 再登録。tray / host / 設定アプリは止めない）で、コピー先を新配置（`%ProgramFiles%\rakukan`）にする。2 タスクの `args` をこのスクリプトへ向ける。インストーラーでは別ビルドの DLL 1 本だけを入れ替えられないため、この経路は残す
- `scripts/uninstall.ps1` を削除（インストーラーのアンインストーラーに一本化）。
- `scripts/check-env.ps1` / `setup-env.ps1`: Inno Setup 6 を必須に格上げ。`setup-env` は winget で導入する。
- `README.md`: 「インストール（ソースから）」の手順を「`cargo make build-engine` → `cargo make build-tsf` → `sudo cargo make install` → 再起動」に変更。「インストール（パッケージから）」の更新手順から事前のサインアウトを削除し、実行後の再起動を案内する。「ビルド前提」の Inno Setup を必須へ。
- `.claude/CLAUDE.md` の手順も同時に更新する。`scripts/check-install-instruction.ps1` の検出条件と文言を新手順に合わせる。
- 失敗した回のインストールのログを残す（#33 の 2026-09-12 のコメント。上書きせず、実行ごとに別ファイルにするなど）。

### 4.3a ログの見直しから回した事項（2026-10-10）

`docs/Log_Redesign_Plan.md` の検討で出た、インストールの経路で扱う事項。

- **トレイ**: 現行の `.iss` は `rakukan-tray.exe` を配布・起動・Run 登録していない（`scripts/install.ps1` だけが HKCU の Run に登録して起動する）。インストーラーで配布し、利用者の権限で起動・Run 登録し、アンインストールで解除する。ログの掃除（Log_Redesign_Plan.md の段 3）はトレイが行うので、これが無いと `.iss` で入れた環境ではログが掃除されない。
- **登録の成否の判定**: regsvr32 の終了コード 0 は、登録の成功も解除の成功も示さない（`DllRegisterServer` の後に DLL を外す段階で落ちうる。`unregister_server()` は各解除のエラーを捨てて常に成功を返す）。インストーラーは、終了コードだけでなく登録の状態（CLSID / TIP のレジストリ）を確かめて成否を決める。
- **旧版での登録解除**: 更新時の旧 DLL での `/u` は旧版の `DllMain` で動くので、0.12.0 以前の DLL ではスレッドの寿命の問題（解除後に落ちる）を持ち込む。旧版の解除が失敗・クラッシュしても更新を止めない扱いと、その確認を含める。

### 4.3b ログの見直し（段 2〜5）を組み込む（2026-10-10）

`docs/Log_Redesign_Plan.md` の段 2〜5 は、この計画（#33）の対応の中で行う。段 1（#72、`DllMain` を空にし Activate で初期化する）は 0.12.0 で先に行う。

| 段 | Issue | 内容 |
|---|---|---|
| 2 | #73 | ログの書き手を 1 つに限り（`.lock` を取れたときだけ書く）、ヘッダーと各行で発生元を分かるようにする |
| 3 | #74 | ログの掃除をトレイへ移す（トレイの起動時、90 日・30000 ファイル、容量の上限なし、旧 `rakukan.log*` も対象）。4.3a のトレイの配布・起動と合わせて行う |
| 4 | #75 | ホストのログを書き込み時ローテーションに揃え、stderr を同じ writer へ。engine DLL のログを C ABI の callback でホストへ渡す（ABI の版を上げる） |
| 5 | #76 | ログの書き込みを別スレッドへ移す。ETW を調査用に併用する |

5 章の作業ステップへの割り当ては、着手時に決める。

### 4.4 winget

- `winget-pkgs` に manifest を登録する。InstallerType は `inno`、サイレント引数は `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`、InstallerUrl は GitHub Releases の exe。
- `Scope: machine`。アンインストールは ARP 登録（`AppId`）経由。再起動が必要なときの終了コードを manifest に合わせる。
- `docs/version-update-checklist.md` に「リリース後に winget manifest を更新する」を追加する。

## 5. 作業ステップ

各ステップは「変更 → 検証 → 完了判定」の順に進め、未解決の回帰を次へ持ち越さない。

| Step | 内容 | 完了条件 |
|---|---|---|
| 1 | コードのパス解決を配置非依存にする（4.2 の `install_dir()`、辞書ディレクトリ、モデルディレクトリ、RPC 版不一致の扱い） | 現行の `%LOCALAPPDATA%` 配置のまま全テストと実機動作が変わらない。`%ProgramFiles%` に手動配置しても動く |
| 2 | インストーラーを新方式にする（4.1） | 新規 PC 相当（旧配置なし）で初回インストール → 再起動後に変換。旧ホスト・旧 TSF が動いたまま更新（事前のサインアウトなしでインストーラーが完走） → 再起動後に新版で動作。再起動までの間に、文字入力が壊れず、アプリが異常終了せず、設定・学習履歴が壊れない。削除で登録とファイルが消える。サイレント実行でも結果（再起動の要否を含む）が判別できる |
| 3 | 既存利用者の移行 | `%LOCALAPPDATA%\rakukan` に旧版がある状態から新インストーラーで更新し、再起動後に旧配置の登録・ファイルが残らず、新配置で動作する |
| 4 | 経路一本化（4.3）。`build-installer.ps1` の入力をビルド出力へ、`install.ps1` / `uninstall.ps1` の削除、`-TsfOnly` の `dev-swap-tsf.ps1` への切り出し、Makefile（`install` / `package` / `full-install` / `quick-install` / 故障試験 2 タスク）、check-env / setup-env、README、CLAUDE.md、Stop hook | `cargo make build-engine` → `build-tsf` → `install` → 再起動が**インストール先が空の状態から**完走し、README の手順どおりに反映できる。`install-tsf-fault-test` / `install-tsf-normal-restore` が新経路で動き、SHA256 の一致確認と再登録が従来どおり働く。`cargo make package` をレモンが実行し、成果物・セットアップ本体・アンインストーラーの 3 つに署名が付くこと（`signtool verify`）を確認する |
| 5 | winget 登録（4.4）。リリース後に manifest を提出 | `winget install` / `upgrade` / `uninstall` が通る |
| 6 | PR #30 に結論コメント、`handoff.md` と `DESIGN.md` の配置記述を更新、CHANGELOG | 文書が実装と一致 |

## 6. 検証項目

- 更新時: 旧 TSF・旧ホストの状態の組み合わせ（旧 TSF が読み込まれている／いない × ホストが動いている／止まっている）ごとに更新し、再起動までの間に、文字入力が壊れない（Rakukan が使えない場合も、未確定の文字・確定に見える文字列・キーを失わない。#69 / #70 の経路を含む）、アプリが異常終了しない、設定・学習履歴が壊れないことを確かめる。再起動後に新版に揃い、変換できる。
- `PROTOCOL_VERSION` と Hello の項目が異なるビルドで更新し、旧 TSF DLL → 新 host と新 TSF DLL → 旧 host の両方向で Hello が失敗する経路を確認する。該当プロセスの TSF ログに得られた理由が残り、異なる版の要求を適用しないことを確認する。版番号を受け取れずデコード失敗・切断となる場合も含める。
- 再起動前にホストが終わった場合（設定の変更、手動の「エンジン再起動」、自己終了）と、バックエンドの切り替え（旧ホスト＋新 engine DLL）の挙動。
- `PendingFileRenameOperations` に置き換え・削除が登録され、再起動後に反映されている。
- 再起動の前にもう一度インストール／アンインストールした場合（同じ版、別の新版、ダウングレード、削除後の再インストール）の最終状態。
- `%LOCALAPPDATA%` から `%ProgramFiles%` への移行と、別ユーザーが host を使っている状態で更新を試す。
- 別ユーザーアカウントでサインインし、「キーボードの追加」で rakukan を選べ、変換できる（モデルは共有ディレクトリから読める）。
- 削除後にレジストリの CLSID / TIP 登録と言語リストの項目が残っていない。ユーザーデータは残っている。
- Explorer 等の異常終了がイベントビューアーに出ていない（更新・再起動の各段階）。
- サイレント実行（`/VERYSILENT`）で対話が一切出ず、終了コードが期待どおり（再起動が必要な場合を含む）。
- 登録・解除の成否を、regsvr32 の終了コードではなくレジストリの状態で確かめる。0.12.0 以前の版からの更新で、旧 DLL の `/u` が失敗・クラッシュしても更新が完了する（4.3a）。
- トレイが配布・起動・Run 登録され、アンインストールで解除される（4.3a）。

## 7. 着手前に確認すること

- **登録**: Inno の `regserver` は、再起動が必要な場合（`AlwaysRestart=yes` ではファイルをその場で置き換えられた場合も）、登録を `RunOnce` に延ばす。管理者で実行した場合は HKLM の `RunOnce` で、再起動後に Administrators のメンバーがログオンしたときに実行される（標準ユーザーが別の管理者の資格情報でインストールし、再起動後は標準ユーザーだけがログオンする場合に登録が完了しない）。現行の `[Run]` の `regsvr32` を残すか置き換えるか、`ssInstall` / `CheckDllLock` の旧 DLL の登録解除の扱い、64 ビットのモードとフラグを決める。
- **キーボードリストへの追加**: `register-tip.ps1`（`postinstall`）は再起動が必要な場合に実行されない。COM/TSF の登録・言語リストへの追加・HKCU への反映を、どのユーザーの権限で、どの順に行うか。
- **失敗と戻し**: 途中で失敗したときに、その場で置き換わったファイル・置き換えの予約・登録の予約をどう扱うか。現行の `InstallFail` は TSF DLL だけを戻し、`FileExists` による成功判定は旧 DLL が残っていても成功とみなす。
- **前提**: `restartreplace` / `uninsrestartdelete` は管理者の権限が要る。使用中かどうかは、置き換えの失敗（アクセス拒否・共有違反など）から Inno が判断する。予約の成功は起動時の置き換えの成功を保証しない。
- Inno Setup がサイレント実行時に返す終了コード（再起動が必要な場合を含む）と、winget manifest の `ReturnCodes` / `ExpectedReturnCodes` の対応。winget の要件（公開 URL、ARP 登録、署名の要否）。
- #56 で Hello の要求・応答の項目が変わるため、新旧間で版番号を交換できるかを実際の v5 / v6 のエンコードで確認する（v6 TSF → v5 host は版不一致の応答、v5 TSF → v6 host はデコード失敗・切断になる見込み）。ログと利用者向けの未接続状態をどう表すか決める。
- `%ProgramData%\rakukan\models` の ACL 設定をインストーラーで行う方法（全ユーザー書込可）。
- `%ProgramFiles%` 配置での `rakukan-dict-builder.exe` の扱い（配布物から外せるか）。
- `hf_download` が書き込む先を `%ProgramData%` に変えたときの、既存の `.cache\huggingface` にあるモデルの扱い（コピーするか、読み取りフォールバックのみか）。

## 8. 対象外

- Microsoft Store（MSIX）への対応。TSF IME を MSIX で配布できるかの調査のみ行い、結果を本書に追記する。
- ユーザー単位（管理者不要）インストール。要望が出た時点で HKCU 登録の検証を条件に検討する。
- 電子署名の必須化。ストア対応を判断する段階で再検討する。
- サインアウト・サインインでの切り替え（ローダー方式）。必要になった時点で再検討する。

## 9. 現行手順（本計画の実施まで）

本計画の実施までは現行の手順を変えない。v6 もこの手順で配布する（リリースノートで手順を守るよう案内する。手順を守らずに一部のファイルが置き換わらなかった場合、v5 と v6 の組み合わせで日本語入力が使えなくなり、インストールし直すまで直らない見込み）。

ソースから:

1. `cargo make build-engine` / `cargo make build-tsf`（変更箇所に応じて）
2. サインアウト
3. サインイン
4. `sudo cargo make install`

パッケージから: 別の IME に切り替え → サインアウト → サインイン → インストーラー実行。
