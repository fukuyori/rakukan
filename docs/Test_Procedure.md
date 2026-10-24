# 統合試験・実機試験の手順（2026-10-10 に v6 用として作成。2026-10-22 に版によらない手順に改め、`V6_Test_Procedure.md` から改名）

自動のテスト（`cargo test`・CI）では確かめられない、実際のアプリ・プロセスをまたぐ動き・画面・インストーラーの動作を、手で確かめる試験。各試験は「操作 → 見る点 → 戻す操作 → 戻した後に見る点」の順。ログは `%LOCALAPPDATA%\rakukan\` の `rakukan-tsf-<PID>-<起動識別子>.log`（アプリごと）と `rakukan-engine-host-<PID>-<起動識別子>.log`（ホスト。engine DLL の行を含む。フェーズ 3 の Step 1 より前のビルドでは `rakukan-engine-host.log` と `rakukan-engine-dll.log`）。時刻順にまとめるときは `scripts/merge-logs.ps1`。

**共通の注意**: アプリからフォーカスが外れると、未確定の文字はアプリ側で確定される（PowerShell に移った時点で確定する）。未確定の文字を保ったままホストを入れ替える試験（T2・T3）は、PowerShell で**遅らせてから**ホストを止め、その間にアプリへ戻って操作する。PowerShell での確認は、アプリでの操作をすべて終えてから行う。

## 試験を行う時期（2026-10-22 に決定）

手で行うので負担が大きい。行う時期を決め、行わない場面をはっきりさせる。

| 試験 | 行うとき | 行わないとき | 使うもの |
|---|---|---|---|
| T1 通常の入力 | TSF のキー処理・表示・確定、またはエンジンの変換を変えたとき | 文書・設定アプリだけの変更 | 開発ビルド |
| T2 ホストの入れ替え | RPC・ホスト・復元・保留キーの処理を変えたとき | TSF の表示だけの変更 | 開発ビルド |
| T3 設定を変えてからの入れ替え | 設定の読み込み・再接続の処理を変えたとき | それ以外 | 開発ビルド |
| T4 インストール・更新・アンインストール | すべてのリリースの前（前の版からの更新を含む） | 開発中 | `-Sign` 付きの配布物 |
| S1 フェーズ 3 の Step 1（配置に依存しない形・ログ・ETW） | フェーズ 3 の Step 1 を終えたとき（`docs/Installer_Redesign_Plan.md` 5 章の実機確認の 1 回目） | それ以外 | 開発ビルド |
| S2 フェーズ 3 の Step 2・3（新しいインストーラー・旧配置からの移行） | フェーズ 3 の Step 3 を終えたとき（同 5 章の実機確認の 2 回目） | それ以外 | 開発ビルドから `-Sign` 付きで作ったインストーラー |
| S3 フェーズ 3 の Step 4（経路の一本化） | フェーズ 3 の Step 4 を終えたとき（同 5 章の実機確認の 3 回目） | それ以外 | 開発ビルド（`sudo cargo make install`）と `cargo make package` の配布物 |

- 変更のたびではなく、まとまった変更（段・フェーズ）が終わったときに 1 回行う。
- リリースの前: T4 は必ず行う。T1〜T3 は、前のリリースの後に当てはまる変更があり、その変更の後にまだ行っていないものだけを行う。
- 配布物で T1〜T3 をやり直すのは、インストーラーやファイルの配置を変えたとき（10 月の計画のフェーズ 3 など）だけ。開発の導入（`cargo make install`）も配布物（`cargo make package`）も、同じビルドの出力から同じインストーラー（`scripts/build-installer.ps1`）で作るので、それ以外は開発ビルドでの結果を使う。
- 故障を起こす口が要る試験（保留中の操作、257 キー目、`VersionMismatch`、ロック競合など）は、手では行わない（#85 の自動化で扱う）。
- 結果は、日付・版・ビルドを書いた節として、この文書の末尾に足していく。

## 0. 準備

1. **設定のバックアップ**（PowerShell）
   ```powershell
   $bk = "$env:APPDATA\rakukan\backup-test-$(Get-Date -Format yyyyMMdd-HHmmss)"
   New-Item -ItemType Directory $bk | Out-Null
   Copy-Item "$env:APPDATA\rakukan\config.toml","$env:APPDATA\rakukan\keymap.toml","$env:APPDATA\rakukan\user_dict.toml","$env:APPDATA\rakukan\learn_history.bin" $bk -ErrorAction SilentlyContinue
   Get-FileHash "$bk\*" | Format-Table Hash, Path
   ```
   見る点: 4 ファイル（無いものは除く）とハッシュが表示される。
2. **ログのレベル**: 設定アプリ、または `config.toml` の `[general] log_level` を `"debug"` にする（元の値を控える）。反映はホストの起動し直しの後。
3. **開発ビルドの導入**: `cargo make build-engine` → `cargo make build-tsf` → `sudo cargo make install` → PC を再起動（フェーズ 3 の Step 4 から。それより前のビルドでは、`sudo cargo make install` の前にサインアウト → サインイン）。
   見る点: ホストのログの起動時のビルドの一致の検査が通っている（`dll_git` と `host_git` が同じ）。メモ帳で変換できる。
4. 試験に使うアプリ: メモ帳、EmEditor（2 つ以上の別プロセス）。

## T1. 通常の入力（実 DLL を通した `Change` / `Read`）

操作（メモ帳と EmEditor の両方で）:
1. `kanjiwohenkan` と打ち、Space で変換、候補を選んで Enter で確定。
2. ライブ変換が出る長さまで打ち（例: `kyouhaiitenkidesu`）、Enter で確定。
3. `kanji` と打ち、Backspace で 1 文字消す（「かん」）、`ji` で戻す、Esc で取り消す。
4. `kanji` と打って F9（「ｋａｎｊｉ」）→ F10（「kanji」）→ F6（「かんじ」）、Esc で取り消す。
   F9 と F10 は大文字・小文字の状態を保ったまま幅を切り替える。同じキーを続けて押すと 小→大→先頭大 と回る。
5. ライブ変換が出る長さまで打って Space、Shift+← で先頭の文節だけを選び、Enter（部分確定）。

見る点: 期待どおり入力・変換・確定できる。5 は選んだ部分だけが確定し、残りが未確定で残る。

戻す操作: なし（確定した文字は文書ごと閉じて保存しない）。

## T2. ホストの交換中の composition の復元（#56）

### T2-1 未確定文字があるときにホストを止める
1. PowerShell で 10 秒後にホストを止める:
   ```powershell
   Get-Process rakukan-engine-host | Format-Table Id, StartTime; Start-Sleep 10; Get-Process rakukan-engine-host | Stop-Process -Force; "stopped"
   ```
2. すぐにメモ帳へ移り、`kanji` と打つ（「かんじ」が未確定）。
3. 10 秒以上待ち、`a` を打つ。
4. Space で変換し、Enter で確定する。
5. PowerShell に戻り、`Get-Process rakukan-engine-host | Format-Table Id, StartTime`。

見る点: 3 で「かんじあ」になる（消えない・二重にならない）。TSF ログに `composition read failed: Rejected(GenMismatch)`（または通信の失敗）の後、`host connected: Hello/Create ok config=(rev=… sha256=…)` が出る。5 の Id と StartTime が 1 と変わっている。

### T2-1b 復元のきっかけが Space のとき
1. PowerShell で `Start-Sleep 10; Get-Process rakukan-engine-host | Stop-Process -Force; "stopped"`。
2. すぐにメモ帳へ移り、`kanji` と打つ。
3. 10 秒以上待ち、Space を押す。
4. もう一度 Space を押し、Enter で確定する。

見る点: 3 で変換され、候補ウィンドウが出る。4 の Space は次の候補へ移る（入力処理の見直しの段 7 以降。復元の結果が照合点と一致した場合（Exact）は、復元した直後の Space も通常どおり変換する。一致しなかった場合（Rebased）は表示の修復だけで、変換は次の Space）。

### T2-2 未確定のローマ字があるとき
1. T2-1b の 1 と同じくホストを止める。
2. すぐにメモ帳へ移り、`kanjik` と打つ（「かんじk」）。
3. 10 秒以上待ち、`a` を打つ。
4. Esc で取り消す。

見る点: 3 で「かんじか」になる（`k` が失われない）。

### T2-3 候補を選んでいるとき
1. T2-1b の 1 と同じくホストを止める。
2. すぐにメモ帳へ移り、`kanji` → Space で候補ウィンドウを出す。
3. 10 秒以上待ち、↓ を押す。
4. Enter を押す。

見る点: アプリが落ちない。文字列が消えない。入力処理の見直しの段 7 以降は、3 で候補ウィンドウが残り、↓ で次の候補へ移る。4 で、選んでいる候補（入力行に見えている文字）が確定される（Exact の場合。Rebased なら 3 で Preedit に戻り、候補ウィンドウが消える）。3 と 4 で画面がどうなったか（候補ウィンドウ・入力行の文字・確定された文字）を記録する。

2026-10-10 の結果: 復元すると変換前の状態（Preedit）に戻るが、画面は変換後の文字と候補ウィンドウのまま書き換わらず、↓ は効かず、Enter でひらがなが確定した。

### T2-4 言語バーの「エンジン再起動」
言語バーのメニューを開くとアプリのフォーカスが外れ、未確定の文字は確定される。そのため、未確定の文字が無い状態で行う。トレイの「エンジン再起動」は設定アプリの保存と同じ扱い（`ApplyTrigger::SaveEvent`）で、設定が変わっていなければホストを止めないので、この試験には使わない。

1. メモ帳で、未確定の文字が無い状態にする。
2. 言語バーの Rakukan のメニューから「エンジン再起動」を選ぶ。
3. メモ帳に戻り、`kanji` → Space → Enter。

見る点: 3 で普通に変換・確定できる。TSF ログに `trigger=ManualRestart force=true` と `rpc: Shutdown acknowledged by host`、ホストのログに `Shutdown requested, exiting host process` の後の起動。

戻す操作（T2 全体）: 未確定文字を Esc で消し、文書を保存せずに閉じる。

2 つのアプリで同時に未確定の文字を持ったままホストを入れ替える試験は、アプリを移った時点で前のアプリの未確定の文字が確定されるため、その状態を作れない。試験から外した。

## T3. 設定を変えてからホストを入れ替える（#65 の再接続）

1. 設定アプリで現在の候補数（`num_candidates`）を控え（例: 19）、候補ウィンドウのページ数を見ておく（19 のとき 5 ページ）。
2. 候補数を別の値（例: 9）にして保存する。
   見る点: 各 TSF ログに `config published: rev=… engine_json_changed=true pending_apply=true`。
3. PowerShell で `Start-Sleep 10; Get-Process rakukan-engine-host | Stop-Process -Force; "stopped"`。
4. すぐにメモ帳へ移り、`kanji` と打ち、10 秒以上待ってから Space を 2 回（1 回目は復元だけ）。
5. 候補ウィンドウのページ数を見て、Esc で取り消す。

見る点: TSF ログの `host connected: Hello/Create ok config=(rev=… sha256=…)` の rev が 2 の `config published` の rev と同じかそれ以降。ページ数が変わっている（9 のとき 4 ページ）。1 ページの件数は `num_candidates` では変わらない（9 件のまま）。

戻す操作: 設定アプリで候補数を 1 の値に戻して保存 → 3〜5 と同じ操作。
戻した後に見る点: ページ数が 1 と同じに戻っている。`Hello/Create ok` の rev が戻した後の `config published` の rev で、sha256 が変える前と同じ値。

## T4. パッケージのインストール・更新・アンインストール

**版数を上げた後のパッケージで行う。** 版数を上げる前のパッケージは、入っているものと同じ版数・同じ中身に置き換わるだけで、更新の試験にならない。パッケージは `cargo make package`（`-Sign` 付き）で作る。手順はフェーズ 3 の Step 4 の後のもの（インストーラーの実行 → 再起動。事前のサインアウトは要らない）。状態は S3 の「PowerShell の準備」の関数（`Show-RakukanState`・`Show-UninstallEntry`）で見る。アンインストールの前には、ログを退避する（`docs/October_Plan.md` 4 節）。

### T4-0 前の版を入れる（前の版の状態でないとき）
1. 入っている rakukan を外す: `sudo cargo make uninstall`（または「設定」→「アプリ」）→ PC を再起動。
2. 前の版の配布物（例: `output\oldversion\rakukan-0.13.0-release-setup.exe`）をダブルクリックで実行し、その版の手順で入れる（0.13.0 までは、終わったらサインアウト → サインイン）。
   見る点: `Show-RakukanState` の `CLSID` が前の版のインストール先（0.13.0 までは `…\AppData\Local\rakukan\rakukan_tsf.dll`）。

### T4-1 前の版からの更新
1. 前の版の状態で、T1 の 1 で動作を確かめる。メモ帳は閉じない（前の版の TSF とホストが動いている状態にする）。
2. 新しい版のパッケージをダブルクリックで実行する（UAC を承認）。サインアウトはしない。
   見る点: 使用許諾の画面が出て、「同意する」で進める。完了の画面に再起動を求める文が出る。「後で再起動する」を選んで完了する。
3. メモ帳で `kanji` → Space → Enter。
   見る点: メモ帳が落ちない。打った文字は失われない（変換できなくてもよい）。
4. PC を再起動し、サインインする。
5. メモ帳で T1 の 1。PowerShell で「PowerShell の準備」を行い、`Show-RakukanState`、`Show-UninstallEntry`、ホストのログの 1 行目と `engine DLL loaded` の行（S3-1 の 8 のコマンド）。
   見る点: 変換できる。`CLSID` が `C:\Program Files\rakukan\rakukan_tsf.dll`。ホストのログの `build=` と `dll_version=` が新しい版で、`dll_git` と `host_git` が同じ（`-dirty` が付かない）。前の版が 0.13.0 までなら、`64-bit: True`・`32-bit: False`、「アプリ」の一覧の Rakukan IME が 1 つ、`%LOCALAPPDATA%\rakukan` に旧版のファイルが無い。`%APPDATA%\rakukan` の設定・ユーザー辞書・学習履歴が残っている（0 の 1 のハッシュと比べ、学習履歴は試験で変わりうる）。

### T4-2 アンインストールと再インストール
1. ログを退避する。「設定」→「アプリ」から Rakukan IME をアンインストールする。
   見る点: 完了の後に再起動を求める表示が出る。
2. PC を再起動し、サインインする。`Show-RakukanState`、`Show-UninstallEntry`。
   見る点: キーボードの一覧に Rakukan が無い。`CLSID` が空。`%ProgramFiles%\rakukan` が無い。`64-bit: False`・`32-bit: False`。`%APPDATA%\rakukan` と `%ProgramData%\rakukan\models` が残っている。
3. 戻す操作: 新しい版のパッケージをダブルクリックで実行 → PC を再起動。
4. 戻した後に見る点: T1 の 1 が通る。設定が残っている。

## S1. フェーズ 3 の Step 1（配置に依存しない形・ログ・ETW）

`docs/Installer_Redesign_Plan.md` 5 章の Step 1 の完了条件と、`docs/archive/Log_Redesign_Plan.md` 7 章の「ETW」「ログのファイル」の行を確かめる。0 の準備（開発ビルドの導入を含む）の後に行う。`cargo make build-engine` で cpu・vulkan・cuda の 3 つをビルドしておく。

### S1-1 ログのファイル（今の配置）
1. メモ帳で `kanjiwohenkan` → Space → Enter。
2. PowerShell で:
   ```powershell
   $d = "$env:LOCALAPPDATA\rakukan"
   $host_log = Get-ChildItem $d -Filter 'rakukan-engine-host-*.log' | Sort-Object LastWriteTime | Select-Object -Last 1
   $tsf_log  = Get-ChildItem $d -Filter 'rakukan-tsf-*.log' | Sort-Object LastWriteTime | Select-Object -Last 1
   $host_log.Name; Get-Content $host_log.FullName -TotalCount 1
   Select-String -LiteralPath $host_log.FullName -Pattern 'engine DLL loaded|engine DLL pinned|rakukan_engine_dll|ETW provider|STDERR' | Select-Object -First 8 | ForEach-Object { $_.Line }
   $tsf_log.Name; Get-Content $tsf_log.FullName -TotalCount 1
   Select-String -LiteralPath $tsf_log.FullName -Pattern 'ETW provider|dll pinned' | Select-Object -First 3 | ForEach-Object { $_.Line }
   Get-ChildItem $d -Filter '*.lock' | Sort-Object LastWriteTime | Select-Object -Last 3 Name
   Get-Item "$d\rakukan-engine-dll.log" -ErrorAction SilentlyContinue | Format-Table Name, LastWriteTime
   ```

見る点:
- ホストのログの名前が `rakukan-engine-host-<PID>-<起動識別子>.log` で、1 行目が `rakukan log header: component=host exe=…rakukan-engine-host.exe … pid=… instance=… build=0.13.0@…`。
- `engine DLL loaded: path=…` の後に `engine DLL pinned for the life of the host: …` がある。
- engine DLL の行が target `rakukan_engine_dll` で入っている（変換の `beam conversion done` など）。`rakukan-engine-dll.log` は、無いか、最終更新が導入より前（伸びていない）。
- `ETW provider … not available` の行が無い。
- TSF のログの 1 行目が `rakukan log header: component=tsf exe=…notepad.exe …`。各行にスレッド ID（`ThreadId(…)`）が付く。
- ログごとに同じ名前の `.lock` がある。

戻す操作: なし。

### S1-2 モデルの探す順
1. PowerShell で:
   ```powershell
   Select-String -LiteralPath $host_log.FullName -Pattern 'Using (shared|pre-downloaded|previously downloaded) model|Downloading|Downloaded to' | ForEach-Object { $_.Line }
   Test-Path "$env:ProgramData\rakukan\models"
   ```

見る点: 今のモデルは、既存の HF のキャッシュ（`Using pre-downloaded model` か `Using previously downloaded model`）から読まれ、取得し直していない（`Downloading` が無い）。`%ProgramData%\rakukan\models` は、新しく取得していなければ作られていない（`False`）。

（任意）新しく取得する先の確認: 設定アプリでモデルを、まだ取得していないもの（例: `jinen-v2-xsmall-q5`）に替えて保存し、言語バーの「エンジン再起動」を行ってから、メモ帳で `kanji` → Space（保存だけではモデルが替わらない。#49）。新しいホストのログを読むので、`$host_log = Get-ChildItem "$env:LOCALAPPDATA\rakukan" -Filter 'rakukan-engine-host-*.log' | Sort-Object LastWriteTime | Select-Object -Last 1` で取り直してから、上の `Select-String` を行う。
- 見る点: ホストのログに `Downloading …` と `Downloaded to "C:\ProgramData\rakukan\models\models--…"`。
- 戻す操作: 設定アプリでモデルを元に戻して保存し、言語バーの「エンジン再起動」を行ってから、メモ帳で `kanji` → Space。
- 戻した後に見る点: `Using … model` が元のモデルを指す。取得したファイルは `%ProgramData%\rakukan\models` に残る（消さなくてよい）。

### S1-3 backend の切り替え（engine DLL のピン止め #64・CUDA の読み込み用のライブラリ #90）
1. 設定アプリで、今の `gpu_backend` を控え、CUDA が使える PC なら `cuda` に替えて保存する（使えない PC は `vulkan`）。言語バーの「エンジン再起動」を行う（`gpu_backend` だけの変更は保存では反映されない。#49）。
2. メモ帳で `kanjiwohenkan` → Space → Enter。
3. PowerShell で:
   ```powershell
   $host_log = Get-ChildItem "$env:LOCALAPPDATA\rakukan" -Filter 'rakukan-engine-host-*.log' | Sort-Object LastWriteTime | Select-Object -Last 1
   Select-String -LiteralPath $host_log.FullName -Pattern 'engine DLL loaded|engine DLL pinned|Selected backend' | Select-Object -Last 4 | ForEach-Object { $_.Line }
   & "C:\Program Files\LLVM\bin\llvm-objdump.exe" -p "$env:LOCALAPPDATA\rakukan\rakukan_engine_cuda.dll" | Select-String 'DLL Name: (cud|cublas)'
   ```

見る点: 選んだ backend の DLL が読み込まれ、`engine DLL pinned` が出る。変換できる。CUDA の DLL の import に `cudart64_*.dll` が無く、`cublas64_*.dll` だけがある（`llvm-objdump` が無ければ、この項目は飛ばす）。

戻す操作: 設定アプリで `gpu_backend` を 1 で控えた値に戻して保存し、言語バーの「エンジン再起動」を行ってから、メモ帳で `kanji` → Space → Enter。
戻した後に見る点: ホストのログの最後の `engine DLL loaded` が元の backend の DLL を指す。変換できる。

### S1-4 ETW（管理者の PowerShell）
1. 管理者の PowerShell で、リポジトリのフォルダから `.\scripts\etw-trace.ps1 start`。
   見る点: `開始しました: rakukan-etw -> …\rakukan.etl`。
2. メモ帳で `kanjiwohenkan` → Space → Enter を 3 回。打鍵が遅くならないか見る。
3. `.\scripts\etw-trace.ps1 stop` → `.\scripts\etw-trace.ps1 export`。
4. 書き出した CSV を開き、`Rakukan.Tsf` と `Rakukan.Host` のイベントを探す。

見る点:
- 2 で打鍵と変換の速さが、セッションを始める前と変わらない。
- CSV に両方の provider のイベントがあり、`level`・`target`・`message` の値が読める（`target` が `rakukan_engine_dll` のものを含む）。フィールドは `message` の後ろに ` name=value` の形で入る。
- `export` の表示に欠落（Lost）の数が出る。
- 読めない場合（`tracerpt` が TraceLogging の項目を書き出せない場合）は、その旨を記録する（PerfView か WPA の手順に改める）。

戻す操作: `.\scripts\etw-trace.ps1 status` でセッションが動いていないことを確かめる。CSV には入力した読みが入るので、確かめた後に `%LOCALAPPDATA%\rakukan\etw` の CSV・要約（`*-summary.txt`）・ETL を消す。
戻した後に見る点: `status` が「動いていません」。

### S1-5 `%ProgramFiles%` に手で置く
今の配置（`%LOCALAPPDATA%\rakukan`）から `%ProgramFiles%\rakukan` へ写し、登録を付け替えて動くかを確かめる。ログ・設定の置き場所は変わらない。

登録の付け替えは、`regsvr32` の表示だけでなく、登録先のレジストリと `register_debug.log`（`%LOCALAPPDATA%\rakukan`。登録・解除のたびに時刻・`dll_path`・各段階の結果を追記する）で確かめる。登録先が消したフォルダを指したままになると、IME が選べなくなる（直し方は戻す操作の 2）。

1. 管理者の PowerShell で（タイトルバーに「管理者」と出ている窓）:
   ```powershell
   $src = "$env:LOCALAPPDATA\rakukan"; $dst = "$env:ProgramFiles\rakukan"
   regsvr32 /u "$src\rakukan_tsf.dll"
   New-Item -ItemType Directory $dst -Force | Out-Null
   Copy-Item "$src\rakukan_tsf.dll", "$src\rakukan-engine-host.exe" $dst
   Copy-Item "$src\rakukan_engine_*.dll" $dst
   Copy-Item "$src\dict", "$src\settings-ui" $dst -Recurse
   regsvr32 "$dst\rakukan_tsf.dll"
   Get-ChildItem $dst | Format-Table Name, Length
   (Get-ItemProperty "Registry::HKEY_CLASSES_ROOT\CLSID\{c0ddf8b0-1f1e-4c2d-a9e3-5f7b8d6e2a4c}\InprocServer32").'(default)'
   Get-Content "$src\register_debug.log" -Tail 13
   ```
   見る点: `regsvr32` の表示が 2 回出る（`…\AppData\Local\rakukan\rakukan_tsf.dll の DllUnregisterServer は成功しました。` と `C:\Program Files\rakukan\rakukan_tsf.dll の DllRegisterServer は成功しました。`）。登録先が `C:\Program Files\rakukan\rakukan_tsf.dll`。`register_debug.log` の最後に、今の時刻の `unregister OK` × 3 と、`dll_path: C:\Program Files\rakukan\rakukan_tsf.dll` の `register OK` × 3・`DllRegisterServer success` がある。
2. サインアウト → サインイン（動いているアプリの古い TSF と古いホストを外すため）。
3. メモ帳で `kanjiwohenkan` → Space → Enter。言語バーの Rakukan のメニューから設定アプリを開き、開いたままにする。
4. PowerShell で:
   ```powershell
   Get-Process rakukan-engine-host, rakukan-settings | Format-Table Id, Path
   $host_log = Get-ChildItem "$env:LOCALAPPDATA\rakukan" -Filter 'rakukan-engine-host-*.log' | Sort-Object LastWriteTime | Select-Object -Last 1
   Get-Content $host_log.FullName -TotalCount 1
   Select-String -LiteralPath $host_log.FullName -Pattern 'engine DLL loaded|dict' | Select-Object -First 5 | ForEach-Object { $_.Line }
   ```
5. 設定アプリを閉じる。

見る点: ホストの `Path` が `C:\Program Files\rakukan\rakukan-engine-host.exe`。ヘッダーの `exe=` も同じ。`engine DLL loaded: path=C:\Program Files\rakukan\…`。辞書が読めて変換できる（`dict load failed` が無い）。ログは `%LOCALAPPDATA%\rakukan` に書かれる。設定アプリが開き、その `Path` が `C:\Program Files\rakukan\settings-ui\rakukan-settings.exe`。

戻す操作:
1. 管理者の PowerShell で:
   ```powershell
   $src = "$env:LOCALAPPDATA\rakukan"; $dst = "$env:ProgramFiles\rakukan"
   regsvr32 /u "$dst\rakukan_tsf.dll"
   regsvr32 "$src\rakukan_tsf.dll"
   (Get-ItemProperty "Registry::HKEY_CLASSES_ROOT\CLSID\{c0ddf8b0-1f1e-4c2d-a9e3-5f7b8d6e2a4c}\InprocServer32").'(default)'
   Get-Content "$src\register_debug.log" -Tail 13
   ```
   見る点: `regsvr32` の成功の表示が 2 回出る。登録先が `%LOCALAPPDATA%\rakukan\rakukan_tsf.dll`（`C:\Users\<ユーザー名>\AppData\Local\rakukan\rakukan_tsf.dll`）。`register_debug.log` の最後に、今の時刻の `unregister OK` × 3 と、`dll_path: …\AppData\Local\rakukan\rakukan_tsf.dll` の `register OK` × 3・`DllRegisterServer success` がある。**登録先と記録がこのとおりのときだけ** 3 に進む。
2. 1 の見る点のとおりでない場合（登録先が `C:\Program Files\…` のまま、または記録が増えていない）は、管理者の PowerShell で `regsvr32 "$env:LOCALAPPDATA\rakukan\rakukan_tsf.dll"` をやり直し、1 の 2 行（レジストリと `register_debug.log`）で確かめる。
3. サインアウト → サインイン → 管理者の PowerShell で `Remove-Item "$env:ProgramFiles\rakukan" -Recurse -Force`。

戻した後に見る点: `Get-Process rakukan-engine-host | Format-Table Id, Path` の `Path` が `%LOCALAPPDATA%\rakukan\rakukan-engine-host.exe`。メモ帳で変換できる。`%ProgramFiles%\rakukan` が無い。

## S2. フェーズ 3 の Step 2・3（新しいインストーラー・旧配置からの移行）

`docs/Installer_Redesign_Plan.md` 5 章の Step 2・3 の完了条件と、6 章の検証項目を確かめる。今の開発の配置（`%LOCALAPPDATA%\rakukan`、`sudo cargo make install`）を旧配置として、新しいインストーラーで `%ProgramFiles%\rakukan` へ移し、上書きの更新・サイレント実行・アンインストールを行い、最後に開発の配置へ戻す。

試験の範囲の外: protocol / engine ABI の版が違う組み合わせ（旧版と新版が同じ版なので作れない。6 章の Hello の失敗の経路）、0.12.0 より前の DLL での登録の解除。別のユーザーでの確認（S2-6）は、別のアカウントがある場合だけ行う。

### S2-0 準備
1. 0 の 1（設定のバックアップ）と 2（ログのレベルを `"debug"`）。
2. **0.13.0 の配布物を写しておく**（新しいインストーラーは同じ名前 `output\rakukan-0.13.0-setup.exe` で作られ、上書きされるため）:
   ```powershell
   New-Item -ItemType Directory output\oldversion -Force | Out-Null
   Copy-Item output\rakukan-0.13.0-setup.exe output\oldversion\rakukan-0.13.0-release-setup.exe
   Get-FileHash output\rakukan-0.13.0-setup.exe, output\oldversion\rakukan-0.13.0-release-setup.exe | Format-Table Hash, Path
   ```
   見る点: 2 つのハッシュが同じ。
3. ログを退避する（`docs/October_Plan.md` 4 節の「アンインストールを含む確認の前に、ログを退避する」の手順）。新しいアンインストーラーはログを消さないが、4 で入る新しいトレイが起動時に古いログを掃除するので、その前に写しておく。
4. 0 の 3（`cargo make build-engine` → `cargo make build-tsf` → サインアウト → サインイン → `sudo cargo make install`）。これが旧配置になる。
5. 利用者が、新しいインストーラーを `-Sign` 付きで作る（`output\rakukan-0.13.0-setup.exe`）。
6. 状態を見るコマンド（以下「状態の確認」。各試験で使う）を PowerShell に貼る:
   ```powershell
   function Show-RakukanState {
     $c = '{c0ddf8b0-1f1e-4c2d-a9e3-5f7b8d6e2a4c}'
     "CLSID:      " + (Get-ItemProperty "Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Classes\CLSID\$c\InprocServer32" -ErrorAction SilentlyContinue).'(default)'
     "TIP HKLM:   " + (Test-Path "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$c")
     "TIP HKCU:   " + (Test-Path "HKCU:\Software\Microsoft\CTF\TIP\$c")
     "TIP WOW64:  " + (Test-Path "HKLM:\SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\$c")
     "Run HKLM:   " + (Get-ItemProperty 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Run' -Name rakukan-tray -ErrorAction SilentlyContinue).'rakukan-tray'
     "Run HKCU:   " + (Get-ItemProperty 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name rakukan-tray -ErrorAction SilentlyContinue).'rakukan-tray'
     "TipListed:  " + (Get-ItemProperty 'HKCU:\Software\rakukan' -Name TipListedVersion -ErrorAction SilentlyContinue).TipListedVersion
     "Keyboard:   " + ((@(Get-WinUserLanguageList) | ForEach-Object { $_ } | Where-Object { $_.LanguageTag -like 'ja*' }).InputMethodTips -join ', ')
     "--- processes"; Get-Process rakukan-engine-host, rakukan-tray, rakukan-settings -ErrorAction SilentlyContinue | Format-Table Id, ProcessName, Path, StartTime
     "--- pending (rakukan)"; (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager' -Name PendingFileRenameOperations -ErrorAction SilentlyContinue).PendingFileRenameOperations | Select-String rakukan
     "--- $env:ProgramFiles\rakukan"; Get-ChildItem "$env:ProgramFiles\rakukan" -ErrorAction SilentlyContinue | Format-Table Name, Length
     "--- $env:LOCALAPPDATA\rakukan (logs excluded)"; Get-ChildItem "$env:LOCALAPPDATA\rakukan" -ErrorAction SilentlyContinue | Where-Object { $_.Name -notmatch '\.(log|lock)(\.\d+|\.tmp)?$' } | Format-Table Mode, Name, Length
   }
   Show-RakukanState
   ```
   見る点（旧配置の状態として控える）: `CLSID` が `…\AppData\Local\rakukan\rakukan_tsf.dll`。`Run HKCU` が `…\AppData\Local\rakukan\rakukan-tray.exe`。`%ProgramFiles%\rakukan` が無い。
7. セットアップのログの見方（各インストールの後に使う）:
   ```powershell
   $setupLog = Get-ChildItem $env:TEMP -Filter 'Setup Log*.txt' | Sort-Object LastWriteTime | Select-Object -Last 1
   $setupLog.Name; Select-String -LiteralPath $setupLog.FullName -Pattern 'migration:|old TSF DLL|TSF registration|Need to restart|exit code|Exception' | ForEach-Object { $_.Line }
   ```

### S2-1 旧配置から新しいインストーラーで更新する（移行。旧 TSF・旧ホストが動いたまま）
1. メモ帳で `kanji` → Space → Enter（旧 TSF と旧ホストが動いている状態にする）。メモ帳は閉じない。
2. サインアウトせずに、`output\rakukan-0.13.0-setup.exe` を実行する（UAC を承認）。
   - 使用許諾の画面: 「同意しない」を選んで「次へ」が押せないことを確かめてから、「同意する」で進む。
   - 完了の画面: 「後で再起動する」を選んで完了する。
3. メモ帳に戻り、`kanji` → Space → Enter、続けて `abc` → Enter。
4. PowerShell で `Show-RakukanState` と、2 のセットアップのログ（S2-0 の 7）。

見る点:
- 2: 使用許諾の画面に、日本語と英語の見出しの文と [1]〜[4]（rakukan の MIT License、Windows App SDK、Windows SDK、NVIDIA CUDA の EULA）が崩れずに出て、最後までスクロールできる。「同意しない」では先へ進めない。サインアウトを求められずに完走する。完了の画面に「インストールを完了するには、PC を再起動してください。再起動するまでは、Rakukan による日本語入力ができないことがあります。」と出る。
- 3: メモ帳が落ちない。変換できない場合でも、打った文字は失われない（直接入力になる、など）。起きたことを記録する（再起動までの制限として受け入れる）。
- 4: `CLSID` が `C:\Program Files\rakukan\rakukan_tsf.dll`、`TIP HKLM` が `True`。`Run HKLM` が `"C:\Program Files\rakukan\rakukan-tray.exe"`、`Run HKCU` が空。`%ProgramFiles%\rakukan` に `rakukan-tray.exe`・`LICENSE`・`LICENSE_AGREEMENT.txt`・`NOTICE`・`THIRD_PARTY_LICENSES.md` があり、`.gguf` が無い。`%LOCALAPPDATA%\rakukan` に残っているのは、使用中で消せなかった旧版のファイル（TSF DLL・ホスト・トレイ・engine DLL など）と、ログ・`etw`・`config.toml`・`rakukan.toml`・`*.bak` などの旧版が置いたのではないもの。消せなかったものは `pending (rakukan)` に `…\AppData\Local\rakukan\…` として出る。セットアップのログに `migration: old TSF DLL unregister`、`migration: removed HKCU Run rakukan-tray`、`migration: removed` / `delete at restart` の行があり、`TSF registration check failed` が無い。
- 記録する: `TIP WOW64` の値（Step 3 の実装の時点で、旧配置で `True` だった。共有のキーか、残ったものかを見る）。

5. PC を再起動し、サインインする。
6. メモ帳で `kanjiwohenkan` → Space → Enter。言語バーのメニューから設定アプリを開き、左の欄の下の版と著作権の表示を見て、閉じる。
7. PowerShell で `Show-RakukanState`、続けて:
   ```powershell
   $host_log = Get-ChildItem "$env:LOCALAPPDATA\rakukan" -Filter 'rakukan-engine-host-*.log' | Sort-Object LastWriteTime | Select-Object -Last 1
   Get-Content $host_log.FullName -TotalCount 1
   Select-String -LiteralPath $host_log.FullName -Pattern 'engine DLL loaded|mozc_path|dict load failed' | Select-Object -First 3 | ForEach-Object { $_.Line }
   $tray_log = Get-ChildItem "$env:LOCALAPPDATA\rakukan" -Filter 'rakukan-tray-*.log' | Sort-Object LastWriteTime | Select-Object -Last 1
   Get-Content $tray_log.FullName | Select-Object -First 6
   icacls "$env:ProgramData\rakukan\models"
   Get-WinEvent -FilterHashtable @{ LogName = 'Application'; Id = 1000; StartTime = (Get-Date).AddHours(-2) } -ErrorAction SilentlyContinue | Format-Table TimeCreated, Message -Wrap
   ```

見る点:
- 6: 変換できる。設定アプリが開き、「rakukan v0.13.0」と「Copyright (c) 2024-2026 rakukan Contributors」が出る。
- 7: ホスト・トレイ・設定アプリ（開いていれば）の `Path` が `C:\Program Files\rakukan\…`。ホストのログの `exe=`・`engine DLL loaded: path=`・`mozc_path` が `C:\Program Files\rakukan\…`。`pending (rakukan)` が空。`%LOCALAPPDATA%\rakukan` に旧版のファイル・`settings-ui`・`dict` が無く、ログ・`etw`・`config.toml` などは残っている。`Keyboard` に `0411:{C0DDF8B0-…}{C0DDF8B1-…}` があり、`TipListed` が `0.13.0`。トレイのログの 1 行目が `rakukan log header: component=tray exe=C:\Program Files\rakukan\rakukan-tray.exe …` で、`log cleanup: removed_instances=… remaining_files=…` と `keyboard list checked for 0.13.0` がある。`icacls` に `BUILTIN\Users:(OI)(CI)(M)`（または `Users` の変更の権限）がある。イベントログ（Id 1000、アプリの異常終了）に、試験の時間の explorer・notepad・rakukan のものが無い。`%APPDATA%\rakukan` の設定のハッシュが S2-0 の 1 と同じ（学習履歴は変わりうる）。

戻す操作: なし（S2-2 へ続ける。最後に S2-5 で戻す）。

### S2-2 新しい配置のまま上書きで更新する（TSF・ホストが使用中）
1. メモ帳で `kanji` → Space → Enter。メモ帳は閉じない。
2. `output\rakukan-0.13.0-setup.exe` をもう一度実行し、完了の画面で「後で再起動する」を選ぶ。
3. メモ帳で `kanji` → Space → Enter。
4. PowerShell で `Show-RakukanState` とセットアップのログ。
5. PC を再起動し、サインインする。メモ帳で `kanjiwohenkan` → Space → Enter。PowerShell で `Show-RakukanState`。

見る点:
- 2: 完走する。
- 3: メモ帳が落ちない。打った文字は失われない（起きたことを記録する）。
- 4: `pending (rakukan)` に `C:\Program Files\rakukan\…`（使用中の TSF DLL・ホスト・トレイなど）が出る。`CLSID` が `C:\Program Files\rakukan\rakukan_tsf.dll`。セットアップのログに `old TSF DLL unregister: exit code …` があり、`TSF registration check failed` が無い。
- 5: 変換できる。`pending (rakukan)` が空。`CLSID`・`TIP HKLM` が 4 と同じ。

戻す操作: なし（S2-3 へ続ける）。

### S2-3 サイレント実行（winget と同じ引数）
1. 管理者の PowerShell で（作業フォルダがリポジトリにならないので、フルパスで指定する）:
   ```powershell
   $setup = "D:\home\source\rust\rakukan\output\rakukan-0.13.0-setup.exe"
   $p = Start-Process $setup -ArgumentList '/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /RESTARTEXITCODE=3010' -Wait -PassThru
   "exit code: $($p.ExitCode)"
   ```
2. 同じく `/RESTARTEXITCODE` を付けずに実行し、終了コードを見る:
   ```powershell
   $p = Start-Process $setup -ArgumentList '/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -Wait -PassThru
   "exit code: $($p.ExitCode)"
   ```
3. PowerShell で `Show-RakukanState` とセットアップのログ。
4. PC を再起動し、サインインする。メモ帳で `kanji` → Space → Enter。

見る点: 1・2 とも、画面に何も出ず（使用許諾の画面も出ない）、再起動もされない。1 の終了コードが `3010`、2 が `0`（再起動が要る成功。`AlwaysRestart=yes` なので毎回再起動が要る扱い）。3 で `CLSID` が変わらず、セットアップのログに `Need to restart Windows? Yes` がある。4 で変換できる。

戻す操作: なし（S2-4 へ続ける）。

### S2-4 アンインストール
1. ログを退避する（S2-0 の 3 と同じ）。
2. 「設定」→「アプリ」→「インストールされているアプリ」から Rakukan IME をアンインストールする。再起動を求められたら「後で」を選ぶ。
3. PowerShell で `Show-RakukanState`。
4. PC を再起動し、サインインする。PowerShell で `Show-RakukanState`、続けて `Test-Path "$env:ProgramData\rakukan\models"`、`Get-ChildItem "$env:APPDATA\rakukan" | Format-Table Name`。

見る点:
- 2: 完走し、再起動を求める表示が出る。
- 3: `CLSID` が空、`TIP HKLM`・`TIP HKCU` が `False`、`Run HKLM` が空、`TipListed` が空、`Keyboard` に rakukan が無い。`pending (rakukan)` に、使用中だった `C:\Program Files\rakukan\…` が出る。
- 4: `%ProgramFiles%\rakukan` が無い（またはファイルが残っていない）。`pending (rakukan)` が空。`%ProgramData%\rakukan\models` が残っている（`True`）。`%APPDATA%\rakukan` の設定・ユーザー辞書・学習履歴が残っている。`%LOCALAPPDATA%\rakukan` のログが残っている。

戻す操作: S2-5。

### S2-5 開発の配置に戻す
Step 4（経路の一本化）までは、開発の導入は今までの `sudo cargo make install`（`%LOCALAPPDATA%\rakukan`）で行うので、その状態に戻す。
1. サインアウト → サインイン → `sudo cargo make install`。
2. キーボードの一覧に Rakukan を戻す（S2-4 のアンインストールで外れ、`install.ps1` は一覧に足さない。移行で `%LOCALAPPDATA%` の `register-tip.ps1` も消えるので、トレイも補わない）: PowerShell で
   ```powershell
   powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\register-tip.ps1; "exit code: $LASTEXITCODE"
   ```
3. メモ帳で `kanjiwohenkan` → Space → Enter。
4. PowerShell で `Show-RakukanState`。

戻した後に見る点: 2 の終了コードが `0`。変換できる。`Keyboard` に `0411:{C0DDF8B0-…}{C0DDF8B1-…}` がある。`CLSID` が `…\AppData\Local\rakukan\rakukan_tsf.dll`、`TIP HKLM` が `True`。`Run HKCU` が `…\AppData\Local\rakukan\rakukan-tray.exe`、`Run HKLM` が空。`%ProgramFiles%\rakukan` が無い。ホストの `Path` が `%LOCALAPPDATA%\rakukan\rakukan-engine-host.exe`。`output\rakukan-0.13.0-setup.exe` は新しいインストーラーのままなので、0.13.0 の配布物が要るときは `output\oldversion\rakukan-0.13.0-release-setup.exe` を使う（ハッシュは S2-0 の 2 で控えたもの）。

### S2-6（任意）別のユーザー
別のアカウントがある場合だけ、S2-1 の後（新しい配置が入っている間）に行う。
1. 別のアカウントでサインインする。
2. 「設定」→「時刻と言語」→「言語と地域」→ 日本語の「言語のオプション」→「キーボードの追加」に Rakukan があれば足す（トレイが足していれば、すでにある）。
3. メモ帳で `kanji` → Space → Enter。新しいモデルを取得させる場合は、設定アプリでまだ取得していないモデルに替えて保存し、言語バーの「エンジン再起動」を行ってから `kanji` → Space。

見る点: キーボードの一覧に Rakukan がある（トレイが補った場合は、そのユーザーの `HKCU\Software\rakukan` の `TipListedVersion` が `0.13.0`）。変換できる。モデルが `%ProgramData%\rakukan\models` から読まれる（ホストのログに `Using shared model`）。新しく取得させた場合は `Downloaded to "C:\ProgramData\rakukan\models\…"` が出る（取得の失敗で `.cache\huggingface` へ取り直していない）。

戻す操作: 別のアカウントからサインアウトする。

## S3. フェーズ 3 の Step 4（経路の一本化）

`docs/Installer_Redesign_Plan.md` 5 章の Step 4 の完了条件を確かめる。今の開発の配置（`%LOCALAPPDATA%\rakukan`、削除した `install.ps1` で入れたもの）から、新しい `sudo cargo make install` で `%ProgramFiles%\rakukan` へ移る。以後の開発の導入はこの形になるので、最後は新しい形の開発の配置で終わる。

試験の範囲の外: 別のユーザーでの確認（S2-6 と同じ）、protocol / engine ABI の版が違う組み合わせ。

**使う PowerShell**: 管理者でない PowerShell を 1 つ開き、S3 の操作はすべてそこで行う（`sudo` と `install-tsf-*` は、そこから昇格する。S3-4 の 2 だけは管理者の PowerShell）。下の「PowerShell の準備」は、この PowerShell を開くたびに行う。**PC を再起動した後は、開き直して、もう一度行う**（関数はその PowerShell の中にだけ残る）。

**PowerShell の準備**（S3-0 の 3 で初めて行い、再起動の後のたびに行う）:
1. 作業フォルダをリポジトリにする: `Set-Location D:\home\source\rust\rakukan`
2. S2-0 の 6 の `function Show-RakukanState { … }` を貼る（末尾の `Show-RakukanState` の行は貼らなくてよい）。
3. 次の 2 つの関数を貼る:
   ```powershell
   # セットアップのログ（output\logs\ に実行ごとに残る）の、最後のものの要点
   function Show-SetupLog {
     $setupLog = Get-ChildItem D:\home\source\rust\rakukan\output\logs -Filter 'setup-*.log' | Sort-Object LastWriteTime | Select-Object -Last 1
     $setupLog.Name; Select-String -LiteralPath $setupLog.FullName -Pattern 'migration:|old TSF DLL|TSF registration|Need to restart|exit code|Exception' | ForEach-Object { $_.Line }
   }
   # アンインストール情報（64 ビット側と、0.13.0 までの 32 ビット側）
   function Show-UninstallEntry {
     $u = 'Microsoft\Windows\CurrentVersion\Uninstall\{B7C4E2A1-3F8D-4C91-B5A0-D2E6F9183047}_is1'
     "64-bit: " + (Test-Path "HKLM:\SOFTWARE\$u"); "32-bit: " + (Test-Path "HKLM:\SOFTWARE\WOW6432Node\$u")
   }
   ```
   見る点: エラーが出ない。`Get-Command Show-RakukanState, Show-SetupLog, Show-UninstallEntry` で 3 つが出る。

### S3-0 準備
1. 0 の 1（設定のバックアップ）と 2（ログのレベルを `"debug"` 以上にする。`"trace"` ならそのまま）。
2. ログを退避する（`docs/October_Plan.md` 4 節の「アンインストールを含む確認の前に、ログを退避する」の手順。S3-1 で入る新しいトレイが古いログを掃除し、S3-4 でアンインストールするため）。
3. 「PowerShell の準備」を行い、`Show-RakukanState`、`Show-UninstallEntry`、`Test-Path C:\rb\dict\rakukan.dict`。
   見る点: `%ProgramFiles%\rakukan` が無い（インストール先が空）。`CLSID` が `…\AppData\Local\rakukan\rakukan_tsf.dll`。`C:\rb\dict\rakukan.dict` の有無を記録する（無ければ S3-1 で辞書を初めて作る）。`32-bit` の値を記録する（S2 の後のこの PC では `True`）。

### S3-1 インストール先が空の状態から install → 再起動（README の手順）
1. メモ帳で `kanji` → Space → Enter（旧配置の TSF とホストが動いている状態にする）。メモ帳は閉じない。
2. S3 の PowerShell で `cargo make build-engine` → `cargo make build-tsf`。
3. 同じ PowerShell で、サインアウトせずに `sudo cargo make install`。
   見る点: `[1/4]` で mozc の TSV の `Downloaded:` が 12 行と `-> C:\rb\dict\rakukan.dict` が出る（S3-0 の 3 で `C:\rb\dict\rakukan.dict` があった場合は `already built` が出る）。`[3/4]` の ISCC が完走する。`exit code: 0`、`log: D:\home\source\rust\rakukan\output\logs\setup-….log`、「インストールしました。完了するには PC を再起動してください。」が出る。セットアップの画面（使用許諾を含む）が出ず、PC も再起動しない。
4. `git diff rakukan_installer.iss | Select-String MyAppVersion`。
   見る点: 何も出ない（版が同じなので、`build-installer.ps1` が `#define MyAppVersion` を書き換えない）。
5. メモ帳で `kanji` → Space → Enter。
   見る点: メモ帳が落ちない。打った文字は失われない（変換できなくてもよい。起きたことを記録する）。
6. `Show-RakukanState`、`Show-SetupLog`、`Show-UninstallEntry`。
   見る点: `CLSID` が `C:\Program Files\rakukan\rakukan_tsf.dll`、`TIP HKLM` が `True`。`Run HKLM` が `"C:\Program Files\rakukan\rakukan-tray.exe"`、`Run HKCU` が空。`%ProgramFiles%\rakukan` に `dict`・`settings-ui`・`rakukan-tray.exe` があり、`models` が無い。`pending (rakukan)` に、使用中だった `…\AppData\Local\rakukan\…` が出る。セットアップのログに `migration:` の行があり、`TSF registration check failed` が無い。アンインストール情報が `64-bit: True`・`32-bit: False`。32 ビット側が S3-0 の時点であった場合は、セットアップのログに `migration: removed old uninstall entry` がある。「設定」→「アプリ」→「インストールされているアプリ」の Rakukan IME が 1 つだけ。
7. PC を再起動し、サインインする。メモ帳で `kanjiwohenkan` → Space → Enter。
8. PowerShell を開き、「PowerShell の準備」を行う。`Show-RakukanState`、続けて:
   ```powershell
   $host_log = Get-ChildItem "$env:LOCALAPPDATA\rakukan" -Filter 'rakukan-engine-host-*.log' | Sort-Object LastWriteTime | Select-Object -Last 1
   Get-Content $host_log.FullName -TotalCount 1
   Select-String -LiteralPath $host_log.FullName -Pattern 'engine DLL loaded|mozc_path|dict load failed' | Select-Object -First 3 | ForEach-Object { $_.Line }
   Get-ChildItem "$env:ProgramFiles\rakukan\dict" | Format-Table Name, Length
   ```
   見る点: 変換できる。ホスト・トレイの `Path` が `C:\Program Files\rakukan\…`。ホストのログの 1 行目の `exe=`、`engine DLL loaded: path=`、`mozc_path` が `C:\Program Files\rakukan\…` で、`dict load failed` が無い。`dict` に `rakukan.dict` と `rakukan.dict.build.json` がある。`pending (rakukan)` が空。`%LOCALAPPDATA%\rakukan` に旧版のファイル・`settings-ui`・`dict` が無い。`Keyboard` に `0411:{C0DDF8B0-…}{C0DDF8B1-…}` がある。

戻す操作: なし（この形が以後の開発の配置。S3-2 へ続ける）。

### S3-2 2 回目の install（辞書を作り直さない・ログを分ける）
1. S3 の PowerShell で `sudo cargo make install`（ビルドはしない）。
   見る点: `rakukan.dict already built (…), skipping.` が出て、`Downloaded:` が無い。`exit code: 0`。`Get-ChildItem D:\home\source\rust\rakukan\output\logs` に、S3-1 とは別の名前のセットアップのログが 2 つある。
2. PC を再起動し、サインインする。**rakukan を使わずに** S3-3 へ進む（TSF DLL を読み込んだアプリがあると、S3-3 のコピーが止まる）。

戻す操作: なし。

### S3-3 故障試験の 2 タスク（`install-tsf-fault-test` / `install-tsf-normal-restore`）
1. PowerShell を開き、「PowerShell の準備」を行う（S3-2 の再起動の後なので）。`cargo make build-tsf-fault-test` → `cargo make build-tsf-normal-restore`（どちらもインストールしない）。
2. `cargo make install-tsf-fault-test`（UAC を承認）。
   見る点: `TSF SHA256 source:` と `TSF SHA256 installed:` が同じ（結果は昇格した側の作業フォルダ `C:\Windows\System32\rakukan_install_tsf_*.log` に残る）。`Registered: C:\Program Files\rakukan\rakukan_tsf.dll` と `Installed TSF DLL only:` が出る。ファイルのロックで止まった場合（`Error while executing command, exit code: 1`）は、サインアウト → サインインの後、**入力方式を Rakukan にせずに**ターミナルを 1 つだけ開いて同じタスクをやり直す（入力方式が Rakukan だと、explorer とターミナルが TSF DLL を読み込む。`tasklist /m rakukan_tsf.dll` で見られる）。
3. `Show-RakukanState`。
   見る点: `CLSID` が `C:\Program Files\rakukan\rakukan_tsf.dll`、`TIP HKLM`・`TIP HKCU` が `True`。トレイが止まっていない（`StartTime` が 2 の前のまま。ホストが動いていればホストも）。
4. ターミナルをクリックしてから Win + Space で、入力方式を **Rakukan** にする（差し替えの登録し直しで MS-IME に変わっていることがあり、そのままでは charmap が TSF DLL を読み込まず、`LOG=(not found …)`・`ARMED=no` になる）。
5. `.\scripts\issue65-start-fault-target.ps1`、表示された `Release:` のコマンドで制御のファイルを消し、文字コード表（charmap）を閉じる。
   見る点: `BUILD_KIND=fault-test`、`ARMED=yes`。

戻す操作: `cargo make install-tsf-normal-restore`（4 で入力方式を Rakukan にしたので、ロックで止まりやすい。止まった場合は 2 と同じ）。続けて入力方式を Rakukan にしてから `.\scripts\issue65-start-fault-target.ps1 -ArmedWaitSeconds 5`、`Release:` のコマンドを実行して charmap を閉じる。メモ帳で `kanji` → Space → Enter。

戻した後に見る点: `TSF SHA256 source:` と `installed:` が同じで、2 の値と違う。`issue65-start-fault-target.ps1` が `BUILD_KIND=normal`・`ARMED=no`。メモ帳で変換できる。

### S3-4 `cargo make package` の署名と、`cargo make uninstall`
1. S3 の PowerShell（**昇格していない**）で `cargo make package`（利用者が行う。環境変数 `CODESIGN_CERT` を使う）。
   見る点: `[sign] Signed: …, Skipped: …, Failed: 0`、`[sign] 成果物の署名を確認しました`、`[sign] 署名を確認しました` が出る。`output\rakukan-0.13.0-setup.exe` ができる。
2. 管理者の PowerShell を別に開き、作った配布物をサイレント実行する（終わったら、この PowerShell は閉じてよい）:
   ```powershell
   $setup = "D:\home\source\rust\rakukan\output\rakukan-0.13.0-setup.exe"
   $p = Start-Process $setup -ArgumentList '/SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART' -Wait -PassThru
   "exit code: $($p.ExitCode)"
   ```
   見る点: 終了コードが `0`。
3. PC を再起動し、サインインする。メモ帳で `kanji` → Space → Enter。PowerShell を開き、「PowerShell の準備」を行ってから:
   ```powershell
   $files = @("D:\home\source\rust\rakukan\output\rakukan-0.13.0-setup.exe", "$env:ProgramFiles\rakukan\unins000.exe") +
            @(Get-ChildItem "$env:ProgramFiles\rakukan" -File | Where-Object Extension -in '.dll', '.exe' | ForEach-Object FullName) +
            @("$env:ProgramFiles\rakukan\settings-ui\rakukan-settings.exe", "$env:ProgramFiles\rakukan\settings-ui\rakukan-settings.dll")
   Get-AuthenticodeSignature $files | Format-Table Status, @{ n = 'Signer'; e = { $_.SignerCertificate.Subject } }, Path -AutoSize
   ```
   見る点: 変換できる。すべての行の `Status` が `Valid` で、`Signer` が `CODESIGN_CERT` の証明書（セットアップ本体・アンインストーラー `unins000.exe`・TSF DLL・engine DLL・ホスト・トレイ・設定アプリ）。
4. ログを退避する（S3-0 の 2 と同じ）。同じ PowerShell で `sudo cargo make uninstall`。
   見る点: `[uninstall] C:\Program Files\rakukan\unins000.exe`、`[uninstall] exit code: 0`、`[uninstall] Done. Restart the PC to finish …` が出る。画面に何も出ない。
5. `Show-RakukanState`。
   見る点: `CLSID` が空、`TIP HKLM`・`TIP HKCU` が `False`、`Run HKLM` が空、`TipListed` が空、`Keyboard` に rakukan が無い。
6. PC を再起動し、サインインする。PowerShell を開き、「PowerShell の準備」を行ってから `Show-RakukanState`、`Show-UninstallEntry`。
   見る点: `%ProgramFiles%\rakukan` が無い（またはファイルが残っていない）。`pending (rakukan)` が空。`64-bit: False`・`32-bit: False`。`%APPDATA%\rakukan` の設定が残っている。

戻す操作: 同じ PowerShell で `sudo cargo make install` → PC を再起動し、サインインする。メモ帳で `kanjiwohenkan` → Space → Enter。PowerShell を開き、「PowerShell の準備」を行ってから `Show-RakukanState`。

戻した後に見る点: 変換できる。`CLSID` が `C:\Program Files\rakukan\rakukan_tsf.dll`。`Keyboard` に `0411:{C0DDF8B0-…}{C0DDF8B1-…}` があり、`TipListed` が `0.13.0`（インストーラーが足せなかった場合は、サインインのときにトレイが足す）。ホストの `Path` が `C:\Program Files\rakukan\rakukan-engine-host.exe`。

## 5. 後始末

1. ログのレベルを 0 の 2 で控えた値に戻す。
2. 設定を比べる:
   ```powershell
   Get-FileHash "$env:APPDATA\rakukan\config.toml", "$bk\config.toml" | Format-Table Hash, Path
   ```
   `config.toml` が一致しない場合は、差分を見て、試験で変えた項目だけが残っていれば戻す（`Copy-Item "$bk\config.toml" "$env:APPDATA\rakukan\config.toml"`）。
3. 開発ビルドで試験を終える場合は、通常の手順で導入し直す。

## 記録すること

- 各試験の結果（通った／通らない、通らない場合の画面と時刻）
- 失敗した試験の時刻の前後の TSF ログとホストのログ（`scripts/merge-logs.ps1` でまとめる）
- T2-3 で起きたこと（#69 の資料）

0.13.1 の結果より前の記録（2026-10-10〜2026-10-24 の S3 まで）は [archive/Test_Results_to_2026-10-24.md](archive/Test_Results_to_2026-10-24.md)。

## 0.13.1 の結果（2026-10-24。T1〜T4・試験 30: `rakukan-0.13.1-setup.exe`、`cargo make package` で作成、ビルド `7a04bef972ff`）

配布物は main（`7a04bef` = PR #87 のマージ）に版の変更（0.13.1）と `-dirty` の廃止を加えた作業ツリーから作った。署名はセットアップ本体（タイムスタンプ付き）と `dist\` の成果物 8 つがすべて有効。前の版は 0.13.0 の配布物（`output\oldversion\rakukan-0.13.0-release-setup.exe`）。試験のアプリはメモ帳の代わりに主に EmEditor を使った。

- 準備: 設定を `backup-test-20261024-102657`、ログを `log-backup-20261024-102658` に写した。
- T4-0: 開発版を `sudo cargo make uninstall` で外して再起動した後、0.13.0 の配布物を入れた。登録は `…\AppData\Local\rakukan\rakukan_tsf.dll`、アンインストール情報は 32 ビット側だけ。
- T4-1: 合格。0.13.0 で変換できる状態から、サインアウトせずに 0.13.1 の配布物を実行した（使用許諾の画面・完了の画面の再起動の文を確認）。再起動の前も入力できた。セットアップのログに旧 DLL の登録の解除と `migration: removed old uninstall entry … (App Path=C:\Users\n_fuk\AppData\Local\rakukan)` があり、使用中の 0.13.0 の 3 つ（TSF DLL・CUDA 版の engine DLL・ホスト）は削除の予約に入った。再起動の後: ホストのログの 1 行目が `build=0.13.1@7a04bef972ff`、`engine DLL loaded` が `dll_version=0.13.1 dll_git=7a04bef972ff … host_git=7a04bef972ff`（ビルドの一致の WARN なし、`-dirty` なし）。ホスト・トレイ・engine DLL・辞書が `C:\Program Files\rakukan` から動いた。アンインストール情報は 64 ビット側だけ（`DisplayVersion` 0.13.1）。予約は空、`%LOCALAPPDATA%\rakukan` の旧版のファイルは消えた。`TipListed` が `0.13.1`。設定（`config.toml`・`keymap.toml`・`user_dict.toml`）は準備のバックアップと一致。イベントログのアプリの異常終了は無かった。
- T1: 合格（1〜5）。
- T2: 合格。T2-1・T2-1b・T2-2・T2-3 で、ホストを止めた後に `Disconnected(Transport)` → `Hello/Create ok … spawn=ok` → `composition read failed: Rejected(GenMismatch)`（復元のきっかけ）の順の行があった。T2-3 は Enter で選んでいた候補（「感じ」）が確定した（2026-10-10 はひらがなが確定していた）。T2-1 の前に 1 回 `Rejected(OwnerMismatch)` があった（ホストを止める前。アプリの間で入力先が移ったときのものと見られる。※推測）。T2-4 は `trigger=ManualRestart force=true` と `rpc: Shutdown acknowledged by host`、ホストのログに `Shutdown requested, exiting host process` の後の起動。
- T3: 合格。ページ数は 5 → 4（候補数 9）→ 5（28 に戻した後）。繋ぎ直しは `Hello/Create ok config=(rev=2 sha256=e2fb5d0f66cf)`（変えた後の `config published` と同じ）と、戻した後の `rev=3 sha256=f7be1901bd38`（変える前と同じ sha256）。
- T4-2: 合格。ログを退避（`log-backup-20261024-113955`）してから「アプリ」でアンインストールし、再起動の後に登録・TIP（HKLM・HKCU）・HKLM の Run・`TipListed`・キーボードの一覧の項目・アンインストール情報（64・32 ビットとも）・`%ProgramFiles%\rakukan`・予約が無くなり、`%APPDATA%\rakukan` と `%ProgramData%\rakukan\models` は残った。0.13.1 の配布物を入れ直して再起動の後に変換でき、登録・キーボードの一覧・`TipListed`（`0.13.1`）・ホストとトレイの場所（`C:\Program Files\rakukan`）が戻った。
- 試験 30（配布物での統合試験）: 上の T1・T2 を 0.13.1 の配布物で行った。
- 後始末: 設定は `config.toml`・`keymap.toml`・`user_dict.toml` が準備のバックアップと一致。`learn_history.bin` は試験の確定で学習が増えて異なる。ログのレベルは `trace` のまま。今の PC は 0.13.1 の配布物が入った状態。

## 2026-10-24 の結果（フェーズ 4: 設定の反映とホストの起動し直し。開発ビルド `23749e6`、`sudo cargo make install`）

設計書は [Engine_Config_Apply_Plan.md](Engine_Config_Apply_Plan.md) の 5.2 節。最初の試験は、`cargo make build-tsf` と `sudo cargo make install` が前のビルド（16:34、ホストのログの 1 行目が `build=0.13.1@3bdfd7729091`）のまま行われていたので、数えない。入れ直した後（19:15〜19:17 のビルド、ホストのログが `build=0.13.1@23749e628031`）にやり直した。

- 準備: `config.toml` を `config.toml.phase4-backup` に写した。
- 通常の入力: 合格。新しいホストが `Selected backend (explicit): cuda`（設定の JSON の `gpu_backend` で選んだ）、`model ready name=jinen-v2-small-q5`。
- 設定アプリのモデルの選択肢: 合格（models.toml の 8 種類）。
- モデルの変更（`jinen-v2-xsmall-f16` → `jinen-v2-small-q5` に戻す）: 合格。古いホストが `Create with changed config; ending this host to restart` で終わり、新しいホストがそのモデルを読んだ（xsmall-f16 は `%ProgramData%\rakukan\models` へ取得、26 秒）。TSF は `host connect not completed (not counted)`。戻した後の `ShutdownIfConfigDiffers` は「同じ設定」。
- backend の変更（`vulkan` → `cuda` に戻す）: 合格。`gpu_backend` だけの変更で `ShutdownIfConfigDiffers: config differs, will exit`、新しいホストが `Selected backend (explicit): vulkan`（最初の変換 1251ms）。古いホストに宛てた別のアプリの要求は `for another host; keeping this host`。戻した後は CUDA 版で 106ms。
- config.toml を壊す（最後に `[general` を足す）: 合格。TSF は `TOML parse error` の WARN を出して直前の設定を使い続け、動いているホストはそのまま変換した。言語バーの「エンジン再起動」の後、新しいホストが `config.toml cannot be parsed; starting with the last adopted config (saved)` で、保存してあった設定（モデル・backend・候補数とも壊す前と同じ）で起動した。TSF の接続は `host kept its config`（`CreateKept`）。行を消した後も変換できた。
- 複数のアプリ（メモ帳・EmEditor・Firefox）: 合格。
- 全体: 起動の抑止（`host spawn suppressed`）と、起動の失敗として数えたもの（`recorded host startup failure`）は 0 件。ERROR の水準の行と panic は無かった。
- 試さなかったもの: 入力中に設定を保存する試験（設計書 5.2 の 6）。設定アプリに切り替えるとフォーカスの移動で確定するため。
- 後始末: 行を消した後の `config.toml` は、最後の改行が 1 つ多く、写しとハッシュが合わなかった（値は同じ）。写しで上書きしてハッシュの一致を確かめ、写しを消した。
- 保存した設定で起動したホストは、`host-config-last.json` を書き直すときに `config_sha256` を `null` にする。config.toml が壊れている間の起動なので、これでよいとした（照合には使わない記録の値）。
