# インストール・更新・削除の再設計計画

作成: 2026-09-07

更新: 2026-10-10（更新の方式を「使用中のファイルは再起動時に置き換え、再起動で完了」に改めた。旧ホスト停止の先行実装は取りやめ。経緯は 1 章の末尾と #33 のコメント https://github.com/fukuyori/rakukan/issues/33#issuecomment-6092071876 ）

着手時期: **[October_Plan.md](October_Plan.md) の段 C として実施する**（段 A #71 → 段 B #69 / #70 の後。2026-10-10 決定）。6 章の更新時の検証（文字・キーを失わない）は、段 B で #69 / #70 を直した後に行う。**本計画の実施までは、現在のインストール方法を変えない**（9 章）

対象: 0.12.0（2026-10-10、タグ `0.12.0`）の後の版で行う

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
- `ArchitecturesAllowed=x64compatible` と `ArchitecturesInstallIn64BitMode=x64compatible` を足す（現行は 32 ビットのモードで動き、`{sys}` が SysWOW64、`{autopf}` が `Program Files (x86)` になる。7 章）。
- `[Files]` のファイルに `ignoreversion restartreplace uninsrestartdelete` を付ける。`config.toml` の初期配置（`onlyifdoesntexist uninsneveruninstall`）は維持。
- `AlwaysRestart=yes`、`UninstallRestartComputer=yes`。
- 完了の案内を 2 章の文言にする。英語版も同様。再起動が要るときの完了の画面は `FinishedRestartLabel`、無人の実行（`/SILENT`）での確認は `FinishedRestartMessage`、`/NORESTART` のときは `FinishedLabelNoIcons`（`[Icons]` が無いため）が使われる（Inno Setup のソース `Setup.MainForm.pas`）ので、3 つとも替える。
- 旧ホストを止める処理は作らない。
- 独自の戻し（`CheckDllLock`、`InstallFail` とそのサインアウト案内、`.backup`）を削除し、Inno の動作に任せる（7 章）。
- 登録: Inno の `regserver` フラグは使わず、`[Run]` の `regsvr32` で登録する。`ssInstall` での旧 DLL の登録解除は、失敗しても更新を止めない形で残す（7 章、4.3a）。
- キーボードリストへの追加: `register-tip.ps1` を `postinstall` から外し、`runasoriginaluser` を付けてインストールの終わりに実行する。HKCU への `reg COPY` も `register-tip.ps1` の中で行い、写すのは rakukan の CLSID の分だけにする（今までは TIP 全体を写し、アンインストールで HKCU の TIP 全体を消していた）。トレイがサインインのときに `register-tip.ps1` を実行して、無ければ足す。行うのは**版ごとに 1 回**で、済んだ版を HKCU の `Software\rakukan` の `TipListedVersion` に記録する（2026-10-23 決定。利用者が一覧から外した場合に、同じ版では足し直さない）。`unregister-tip.ps1` は一覧からの削除・HKCU の写しの削除・この記録の削除を行う（7 章）。
- `[Dirs]` で `{commonappdata}\rakukan\models` に `Permissions: users-modify` を付ける（7 章）。
- 移行処理: `%LOCALAPPDATA%\rakukan\rakukan_tsf.dll` が存在すれば、登録解除し、旧配置のファイルは再起動時の削除を予約する。旧ディレクトリの `dict\`（`registered.txt` も同様）も同じ。
  - 行う時期は `ssInstall`（新しい DLL を登録する `[Run]` より前）。CLSID が新旧で同じなので、後で旧 DLL の登録を解除すると、新しい登録まで消える。
  - 消すのは、旧版のインストーラーと `install.ps1` が置いた名前のファイル（TSF DLL・engine DLL・ホスト・トレイ・`rakukan-dict-builder.exe`・アイコン・TIP のスクリプト・`registered.txt`・ライセンスの文書・旧版のアンインストーラー `unins000.*` など）と、`settings-ui\`・`dict\` だけ。ログ・`etw\`・`register_debug.log`・`config.toml` など、それ以外のものは残す。消せないもの（使用中）は `RestartReplace` で次の起動時に消す。フォルダーは中身の後に予約し、空になっていれば消える。
  - `install.ps1` が HKCU の Run に書いた `rakukan-tray` は、旧配置を指していれば消す（トレイは HKLM の Run から起動する）。
  - 旧版と AppId が同じなので、`UsePreviousAppDir=no` にする（既定の yes では、前回のインストール先の `%LOCALAPPDATA%\rakukan` へ入れてしまう。ヘルプ `[Setup]: UsePreviousAppDir`）。
  - 対象は、インストーラーを実行したユーザーの `%LOCALAPPDATA%` だけ（`{localappdata}`）。別のユーザーが `install.ps1` で入れた旧配置は扱わない。
- `[UninstallRun]` は unregister-tip → regsvr32 /u。`[UninstallRun]` では `runasoriginaluser` を使えない（Inno Setup のヘルプ `[Run]`）ので、アンインストーラーの権限で実行する。ファイルには `uninsrestartdelete` を付ける。
- 登録の状態（CLSID の InprocServer32 が `{app}\rakukan_tsf.dll` を指し、TIP のキーがある）を確かめられなかったときは、エラーを表示し（`/SUPPRESSMSGBOXES` では出さない）、終了コード 20 を返す（`GetCustomSetupExitCode`）。`postinstall` でない `[Run]` は `CurStepChanged(ssPostInstall)` より前に実行される（`Setup.MainForm.pas`）ので、確かめるのは `ssPostInstall`。
- サイレント実行ではダイアログに依存せず、失敗を終了コードとログで通知する。
- 使用許諾の画面（`LicenseFile`）を出す（2026-10-23 決定）。設定アプリが同梱する Windows App SDK の実行時の部品は、配布するときに「配布者・利用者に、Microsoft をこの契約と同等以上に保護する条件へ同意させる」ことが条件になっている（パッケージの `license.txt` の 3(b)）。設定アプリが同梱する Windows SDK の .NET 向けの部品（`Microsoft.Windows.SDK.NET.dll`・`WinRT.Runtime.dll`）の条項にも、同じ種類の条件（ほかに自分の著作権の表示を出すこと）がある。中身は 1 つの文書にまとめる: 見出しの文（`docs/licenses/license-agreement-header.txt`）、rakukan 本体の MIT License（著作権の表示を含む）、Windows App SDK と Windows SDK の Microsoft Software License Terms の全文と、CUDA 版の engine DLL が静的にリンクする CUDA ランタイムの読み込み用のライブラリ（CUDA 13 の hybrid。ランタイムの本体はディスプレイドライバから読み込む）の NVIDIA の EULA の全文（`docs/licenses/`。EULA の配布の条件は、配布の条件を EULA と食い違わないものにすること・NVIDIA の知的財産の保護を含む）。`build-installer.ps1` が `dist\LICENSE_AGREEMENT.txt` に組み立てる。この形で条件を満たせるかの法的な判断は行っておらず、Microsoft を補償する義務は残る。部品そのものを配布物から無くす方法は #88 で扱う。

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
- **リリース向けの署名経路を開発用と分けて定める**（同レビューで追加）。`cargo make package`（仮称）を新設し、`sign`（`sign-artifacts.ps1`。`C:\rb\release` の DLL / EXE に署名）→ `build-installer.ps1 -Sign`（セットアップ本体と、`SignedUninstaller=yes` によるアンインストーラーに署名）の順に実行する。`build-installer` が署名済みの成果物を集めるよう、`sign` を先にする。**パッケージは `-Sign` 付きで作る**。開発用の `install` は署名しないサイレント導入で、配布物には使わない。
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

- **トレイ**: 現行の `.iss` は `rakukan-tray.exe` を配布・起動・Run 登録していない（`scripts/install.ps1` だけが HKCU の Run に登録して起動する）。インストーラーで配布し、利用者の権限で起動し、**HKLM の Run** に登録し（2026-10-23 決定。マシン単位の配置に合わせ、別のユーザーのサインインでも起動する。アンインストーラーがそのまま消せる）、アンインストールで解除する。トレイはセッションごとに 1 つだけ動く（名前付きの mutex）。旧配置の `install.ps1` が HKCU の Run に書いた値は、Step 3 の移行で消す。ログの掃除（Log_Redesign_Plan.md の段 3）はトレイが行うので、これが無いと `.iss` で入れた環境ではログが掃除されない。
- **登録の成否の判定**: regsvr32 の終了コード 0 は、登録の成功も解除の成功も示さない（`DllRegisterServer` の後に DLL を外す段階で落ちうる。`unregister_server()` は各解除のエラーを捨てて常に成功を返す）。インストーラーは、終了コードだけでなく登録の状態（CLSID / TIP のレジストリ）を確かめて成否を決める。
- **旧版での登録解除**: 更新時の旧 DLL での `/u` は旧版の `DllMain` で動くので、0.12.0 以前の DLL ではスレッドの寿命の問題（解除後に落ちる）を持ち込む。旧版の解除が失敗・クラッシュしても更新を止めない扱いと、その確認を含める。

### 4.3b ログの見直し（段 2〜5）を組み込む（2026-10-10）

`docs/Log_Redesign_Plan.md` の段 2〜5 は、この計画（#33）の対応の中で行う。段 1（#72、`DllMain` を空にし Activate で初期化する）は 0.12.0 で先に行う。

| 段 | Issue | 内容 |
|---|---|---|
| 2 | #73 | ログの書き手を 1 つに限り（`.lock` を取れたときだけ書く）、ヘッダーと各行で発生元を分かるようにする |
| 3 | #74 | ログの掃除をトレイへ移す（トレイの起動時、90 日・30000 ファイル、容量の上限なし、旧 `rakukan.log*` も対象）。4.3a のトレイの配布・起動と合わせて行う |
| 4 | #75 | ホストのログを書き込み時ローテーションに揃え、stderr を同じ writer へ。engine DLL のログを C ABI の callback でホストへ渡す（無くても動く関数として足し、engine ABI は 12 のまま） |
| 5 | #76 | ログの書き込みを別スレッドへ移す。ETW を調査用に併用する |

5 章の作業ステップへの割り当て（2026-10-10 決定）: #73・#75・#76 は Step 1（コードの変更。#75 は engine ABI を変えない）、#74 は Step 2（トレイを配布・起動するのと合わせる）。

### 4.4 winget

- `winget-pkgs` に manifest を登録する。InstallerType は `inno`、サイレント引数は winget の既定（`/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART`）、InstallerUrl は GitHub Releases の exe。
- `Scope: machine`。アンインストールは ARP 登録（`AppId`）経由。引数に `/RESTARTEXITCODE=N` を足し、`ExpectedReturnCodes` で N を `rebootRequiredToFinish` に結び付ける（7 章）。
- `docs/version-update-checklist.md` に「リリース後に winget manifest を更新する」を追加する。

## 5. 作業ステップ

各ステップは「変更 → 検証 → 完了判定」の順に進め、未解決の回帰を次へ持ち越さない。単体テストやスクリプトで確かめられるものは、各 Step の中で済ませる。実機での確認は、原因の切り分けが広がらない範囲でまとめ、次の 3 回にする（リリース作業の配布物での試験は別）。

- Step 1 の後: Step 1 だけ（今の配置と `%ProgramFiles%` への手での配置）。ログと調べ方そのものを変えるので、インストーラーの確認の前に単独で確かめる。
- Step 3 の後: Step 2 と 3 をまとめて（6 章の更新・削除と、旧版からの移行）。どちらもインストーラーの動作で、旧版を入れた状態からの更新・削除という手順が重なる。
- Step 4 の後: Step 4 だけ（開発の経路）。配布物の作り方を変えるので分ける。
削除（アンインストール）を含む検証（Step 2 の削除、Step 6 の試験、Step 7 の `winget uninstall`、6 章）の前には、[October_Plan.md](October_Plan.md) の 4 節「アンインストールを含む確認の前に、ログを退避する」の手順でログを写す（アンインストールで `rakukan-engine-host.log` が消されるため）。

| Step | 内容 | 完了条件 |
|---|---|---|
| 1 | コードのパス解決を配置非依存にする（4.2 の `install_dir()`、辞書ディレクトリ、モデルディレクトリ、RPC 版不一致の扱い）。ログの見直しの #73（書き手を 1 つに・発生元）・#75（ホストと engine DLL のログ）・#76（書き込みを別スレッドへ・ETW）も行う（4.3b）。ホストが読み込んだ engine DLL をピン止めする（`GetModuleHandleExW` の PIN。#64 の方針、[October_Plan.md](October_Plan.md) の 4 節の開始の条件） | 現行の `%LOCALAPPDATA%` 配置のまま全テストと実機動作が変わらない。`%ProgramFiles%` に手動配置しても動く。設定が合わない `Create` で backend が替わっても、古い engine DLL が外れずホストが動き続ける |
| 2 | インストーラーを新方式にする（4.1）。トレイの配布・起動・Run 登録と、登録の成否の判定・旧版での登録解除の扱い（4.3a）、ログの掃除をトレイへ移す #74（4.3b）も行う | 新規 PC 相当（旧配置なし）で初回インストール → 再起動後に変換。旧ホスト・旧 TSF が動いたまま更新（事前のサインアウトなしでインストーラーが完走） → 再起動後に新版で動作。再起動までの間に、アプリが異常終了せず、直接入力のときに文字・キーを失わず、設定・学習履歴が壊れない。削除で登録とファイルが消える。サイレント実行でも結果（再起動の要否を含む）が判別できる |
| 3 | 既存利用者の移行 | `%LOCALAPPDATA%\rakukan` に旧版がある状態から新インストーラーで更新し、再起動後に旧配置の登録・ファイルが残らず、新配置で動作する |
| 4 | 経路一本化（4.3）。`build-installer.ps1` の入力をビルド出力へ、`install.ps1` / `uninstall.ps1` の削除、`-TsfOnly` の `dev-swap-tsf.ps1` への切り出し、Makefile（`install` / `package` / `full-install` / `quick-install` / 故障試験 2 タスク）、check-env / setup-env、README、CLAUDE.md、Stop hook | `cargo make build-engine` → `build-tsf` → `install` → 再起動が**インストール先が空の状態から**完走し、README の手順どおりに反映できる。`install-tsf-fault-test` / `install-tsf-normal-restore` が新経路で動き、SHA256 の一致確認と再登録が従来どおり働く。`cargo make package` を実行し、成果物・セットアップ本体・アンインストーラーの 3 つに署名が付くこと（`signtool verify`）を確認する |
| 5 | 配布前の文書。`handoff.md` と `DESIGN.md` の配置記述を更新、CHANGELOG | 文書が実装と一致 |
| 6 | リリース作業（[October_Plan.md](October_Plan.md) の 4 節「リリース作業」） | 配布物で試験に合格し、リリースする |
| 7 | 公開後: winget 登録（4.4）。manifest を提出 | `winget install` / `upgrade` / `uninstall` が通る |
| 8 | 公開後: PR #30 に結論コメント | 投稿が済む |

## 6. 検証項目

アンインストールを含む項目（再起動前の再インストール／アンインストール、削除後の確認、トレイの解除など）の前には、[October_Plan.md](October_Plan.md) の 4 節「アンインストールを含む確認の前に、ログを退避する」の手順でログを写す。

- 更新時: 旧 TSF・旧ホストの状態の組み合わせ（旧 TSF が読み込まれている／いない × ホストが動いている／止まっている）ごとに更新し、再起動までの間に、アプリが異常終了しない、直接入力のときに文字・キーを失わない、設定・学習履歴が壊れないことを確かめる。protocol / engine ABI の版が違う組み合わせは直接入力だけになり、版が同じ組み合わせはそのまま動く。再起動後に新版に揃い、変換できる。
- `PROTOCOL_VERSION` と Hello の項目が異なるビルドで更新し、旧 TSF DLL → 新 host と新 TSF DLL → 旧 host の両方向で Hello が失敗する経路を確認する。該当プロセスの TSF ログに得られた理由が残り、異なる版の要求を適用しないことを確認する。版番号を受け取れずデコード失敗・切断となる場合も含める。
- 再起動前にホストが終わった場合（設定の変更、手動の「エンジン再起動」、自己終了）と、バックエンドの切り替え（旧ホスト＋新 engine DLL）の挙動。
- `PendingFileRenameOperations` に置き換え・削除が登録され、再起動後に反映されている。
- 再起動の前にもう一度インストール／アンインストールした場合（同じ版、別の新版、ダウングレード、削除後の再インストール）の最終状態。
- `%LOCALAPPDATA%` から `%ProgramFiles%` への移行と、別ユーザーが host を使っている状態で更新を試す。
- 別ユーザーアカウントでサインインし、「キーボードの追加」で rakukan を選べ、変換できる（モデルは共有ディレクトリから読める）。
- 削除後にレジストリの CLSID / TIP 登録と言語リストの項目が残っていない。ユーザーデータは残っている。
- Explorer 等の異常終了がイベントビューアーに出ていない（更新・再起動の各段階）。
- サイレント実行（`/VERYSILENT`）で対話が一切出ず、終了コードが期待どおり（winget の引数では再起動が要る成功で N、`cargo make install` では 0）。
- TSF DLL を使用中にしたまま更新し、再起動の後に CLSID / TIP のレジストリが新版の登録の内容と合っている（7 章の登録）。
- 昇格した端末から（winget を含む）インストールし、インストーラーを起動したユーザーのキーボードの一覧に rakukan が入る。入らなかった場合は、次のサインインでトレイが足す（7 章）。
- 途中で失敗させた更新の後に、置き換え・削除の予約がどう残るか（7 章の失敗と戻し）。
- 登録・解除の成否を、regsvr32 の終了コードではなくレジストリの状態で確かめる。0.12.0 以前の版からの更新で、旧 DLL の `/u` が失敗・クラッシュしても更新が完了する（4.3a）。
- トレイが配布・起動・Run 登録され、アンインストールで解除される（4.3a）。
- ホストの中で backend を切り替えた後（設定が合わない `Create` で別の engine DLL を読み込み、ピン止めした古い DLL も残っている状態。Step 1）に、更新・アンインストールする。どちらの DLL も使用中として扱われ、インストーラーが止まらずに完走し、再起動の後に置き換え・削除されている。
- 第三者のライセンス（4.1 の使用許諾の画面、`THIRD_PARTY_LICENSES.md`）:
  - 配布物を作るときに、`build-installer.ps1` の確認（`gen-third-party-licenses.ps1 -Check`、`check-bundled-licenses.ps1 -Files`）が通る。
  - 使用許諾の画面が出て、日本語と英語の見出しの文と [1]〜[4]（rakukan の MIT License、Windows App SDK、Windows SDK、NVIDIA CUDA の EULA）が崩れずに読め、最後までスクロールできる。「同意しない」では先へ進めない。
  - サイレント実行（`/VERYSILENT`）で、使用許諾の画面に止まらずに完走する。
  - インストール先に `LICENSE`・`LICENSE_AGREEMENT.txt`・`NOTICE`・`THIRD_PARTY_LICENSES.md` がある。モデル（`.gguf`）が同梱されていない。
  - 設定アプリの左の欄の下と「補足」の「バージョン情報」に、版と著作権の表示（`Copyright (c) 2024-2026 rakukan Contributors`）が出る。「LICENSE を開く」「第三者のライセンスを開く」でインストール先の文書が開く。`rakukan-settings.exe` のプロパティの著作権の欄に同じ表示がある（#88）。
  - CUDA 版の engine DLL（`cudart_static.lib` を使う形、#90）で変換が動く。DLL の import に `cudart64_*.dll` が無い。
  - push の後に、CI の License Check（`cargo deny check licenses`）と Third-Party Licenses（一覧の `-Check` と NuGet のパッケージの確認）が通る。

## 7. 着手前に確認すること

2026-10-23 に決めたことと確かめた結果。資料は、Inno Setup 6.4.0 のヘルプ、Inno Setup のソース（jrsoftware/issrc の main）、winget-cli のソース（microsoft/winget-cli の master）とマニフェストのスキーマ 1.10.0、Microsoft の RunOnce の資料。

- **64 ビットのモード**
  - 確かめた結果: 現行の `.iss` には `ArchitecturesInstallIn64BitMode` が無く、32 ビットのモードで動く。`{sys}` は SysWOW64、`{autopf}` は `Program Files (x86)` を指す（ヘルプ「32-bit and 64-bit Installations」）。
  - 決めたこと: `ArchitecturesAllowed=x64compatible` と `ArchitecturesInstallIn64BitMode=x64compatible` を足す（4.1）。
- **登録**
  - 確かめた結果: `regserver` を付けたファイルは、`AlwaysRestart=yes` のとき、または `restartreplace` のファイルがあるとき、次の再起動の後に登録される（ヘルプ `[Files]` の Remarks）。管理者で実行すると HKLM の RunOnce に書かれる（`Setup.Install.pas`）。HKLM の RunOnce は Administrators のメンバーがログオンしたときだけ実行され、値は実行の前に消される（Microsoft の資料）。標準ユーザーが別の管理者の資格情報でインストールし、再起動後に標準ユーザーだけがログオンすると、登録が終わらない。`postinstall` でない `[Run]` の項目は、再起動が要る場合もインストールの終わりに実行される（ヘルプ「Installation Order」）。
  - 決めたこと: `regserver` は使わず、`[Run]` の `regsvr32` で登録する。成否は終了コードでなくレジストリの状態で決める（4.3a）。
  - 残る点: 置き換えが再起動まで延びた場合は、古い DLL で登録される。パス・CLSID・登録の内容が新旧で同じなら結果は同じと見込む（※推測。6 章で確かめる）。登録の内容を変える版では、別の手当てが要る。
- **キーボードリストへの追加**
  - 確かめた結果: `postinstall` の項目は、再起動が要る場合に実行されない（ヘルプ `[Run]`）。`runasoriginaluser` は、Setup を昇格した状態で起動した場合は効かない。現行の HKCU への `reg COPY` は管理者の権限で動く。
  - 決めたこと: `register-tip.ps1` を `postinstall` から外し、`runasoriginaluser` を付けてインストールの終わりに実行する。HKCU への `reg COPY` も同じ扱いにする。トレイがサインインのときに一覧を確かめ、無ければ足す（版ごとに 1 回。4.1）。
  - 確かめた結果（2026-10-23）: `[UninstallRun]` では `runasoriginaluser` を使えない（ヘルプ `[Run]`）。
- **失敗と戻し**
  - 確かめた結果: アンインストーラーを保存した後のエラーでは戻さない（ヘルプ「Installation Order」）。それより前の段階で何が戻るか、`restartreplace` の予約が戻るかは、ヘルプに書かれていない。現行の `InstallFail` / `.backup` / `CheckDllLock` は `%LOCALAPPDATA%\rakukan` 固定で、サインアウトを案内し、`FileExists` による成功判定は旧 DLL が残っていても成功とみなす。`CheckDllLock` はどこからも呼ばれていない。
  - 決めたこと: 独自の戻しを外し、Inno の動作に任せる。成否は終了コードとレジストリの状態で決める。旧 DLL の `/u` は、失敗しても更新を止めない形で残す（4.3a）。予約がどう残るかは 6 章で確かめる。
- **前提**
  - 確かめた結果: `restartreplace` / `uninsrestartdelete` は管理者の権限が要る。置き換えを 1 秒おきに 4 回まで試し、失敗すると使用中と判断する（ヘルプ `[Files]`）。予約の成功は、起動時の置き換えの成功を保証しない。
  - 決めたこと: `PrivilegesRequired=admin` を保つ。
- **終了コード**
  - 確かめた結果: `AlwaysRestart=yes` では、成功のたびに再起動が要るとされる（`Setup.MainFunc.pas`、`Setup.MainForm.pas`）。既定では 0 を返し、`/RESTARTEXITCODE=N` を付けると N を返す（ヘルプ「Setup Command Line Parameters」）。`/VERYSILENT` で `/NORESTART` が無いと、確認なしに再起動する。winget は Inno に `/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART` を付け（`ManifestCommon.cpp`）、0 なら再起動の案内を出さない。`ExpectedReturnCodes` で N を `rebootRequiredToFinish` に結び付けると、成功として扱い、再起動が要ると警告する。`rebootRequiredForInstall` は失敗の扱いになる（`InstallFlow.cpp`、スキーマ 1.10.0）。
  - 決めたこと: winget の経路では `/RESTARTEXITCODE=N` を付け、N を `rebootRequiredToFinish` に結び付ける（4.4）。`cargo make install` には付けず、0 なら再起動を案内する。
  - 残る点: 引数を足すマニフェストの項目、N の値、winget の配布版でも同じ扱いかは Step 7 で確かめる。winget の要件（公開 URL、ARP 登録、署名の要否）も同じ。
- **`%ProgramData%\rakukan\models` の ACL**
  - 確かめた結果: `[Dirs]` の `Permissions: users-modify` で付けられる。ヘルプは、プログラムのファイルを置くフォルダーには使わないよう、データのフォルダーなら差し支えないとしている。
  - 決めたこと: `{commonappdata}\rakukan\models` に `users-modify` を付ける（4.1）。
- **`rakukan-dict-builder.exe`**
  - 確かめた結果: 現行の `.iss` の `[Files]` には無い。`{app}` へ写すのは `scripts/install.ps1` だけで、他のプログラムからは起動していない。
  - 決めたこと: 辞書のビルドを `build-installer.ps1` の dist 準備段階へ移す（4.3）。`{app}` には置かない。
- **既存の `.cache\huggingface` のモデル**
  - 確かめた結果: `hf_download` は、まず `%USERPROFILE%\.cache\huggingface\hub\models--<repo>\snapshots\main\<file>` を見て、無ければ `hf_hub` の既定のキャッシュに取得する。キャッシュはユーザーのプロファイルごとにあり、他のアプリと共用される。
  - 決めたこと: コピーせず、読むだけにする。探す順は `%ProgramData%\rakukan\models` → 既存の HF キャッシュ。新たに取得するときは `%ProgramData%` に書く。`.cache\huggingface` には書き込みも削除もしない（4.2）。

## 8. 対象外

- Microsoft Store（MSIX）への対応。TSF IME を MSIX で配布できるかの調査のみ行い、結果を本書に追記する。
- ユーザー単位（管理者不要）インストール。要望が出た時点で HKCU 登録の検証を条件に検討する。
- 電子署名の必須化。ストア対応を判断する段階で再検討する。
- サインアウト・サインインでの切り替え（ローダー方式）。必要になった時点で再検討する。

## 9. 現行手順（本計画の実施まで）

本計画の実施までは現行の手順を変えない。リリースノートで手順を守るよう案内する。手順を守らずに一部のファイルが置き換わらなかった場合、新旧で protocol や engine ABI の版が違うと日本語入力が使えなくなり、インストールし直すまで直らない見込み（現行のインストーラーは、置き換えられなかったファイルを再起動の後に置き換えない）。

ソースから:

1. `cargo make build-engine` / `cargo make build-tsf`（変更箇所に応じて）
2. サインアウト
3. サインイン
4. `sudo cargo make install`

パッケージから: 別の IME に切り替え → サインアウト → サインイン → インストーラー実行。
