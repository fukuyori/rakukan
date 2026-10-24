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

`docs/Installer_Redesign_Plan.md` 5 章の Step 1 の完了条件と、`docs/Log_Redesign_Plan.md` 7 章の「ETW」「ログのファイル」の行を確かめる。0 の準備（開発ビルドの導入を含む）の後に行う。`cargo make build-engine` で cpu・vulkan・cuda の 3 つをビルドしておく。

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

## 2026-10-10 の結果（T1〜T3: 開発ビルド `7354de5-dirty`、T4: `rakukan-0.12.0-setup.exe`）

- T1: 合格。F9 → F10 が「KANJI」になった件は、大文字・小文字を保つ方式に直した（同日）。
- T2-1 / T2-1b / T2-2 / T2-4: 合格。
- T2-3: 文字は失われないが、復元後の表示と状態が食い違う（上記）。#69 の資料。
- T3: 合格（rev=2 で `Create`、戻した後は rev=3 で sha256 が元の値）。
- T4: 合格（`rakukan-0.12.0-setup.exe`、署名あり。ビルドは `b9a6de4-dirty` = #72 の修正を含む作業ツリー）。
  - T4-1: 旧版として `output\rakukan-0.11.9-setup.exe`（同日朝の開発ビルド。リリースした 0.11.9 ではないが、`DllMain` でスレッドを起動する旧い作り）を入れ、0.12.0 で更新。インストーラーは成功、登録の終了コード 0、`register_debug.log` は 3 段階とも OK、ホストと DLL は 0.12.0 で一致、利用者のデータは残った。0.12.0 のインストーラーの開始直後（ファイルの置き換え前）に regsvr32 が 1 回落ちた（旧版の DLL での登録解除と見られる。※推測。#33 の 4.3a）。
  - T4-2: 別の IME への切り替え・サインアウトをせずにアンインストールしたため「いくつかの項目が削除できませんでした」と表示され、使用中のファイル（TSF DLL・ホスト・engine DLL・トレイ）と `rakukan-dict-builder.exe` が残った。登録の解除は 3 段階とも OK、CLSID は消えた。README にアンインストールの節を追加。サインアウト → サインイン → 再インストールで、登録 OK・変換できる・利用者のデータは残った。
- 試験中に見たログ: フォーカスが外れたときに毎回 `composition change failed: timer or irreversible request cannot acquire ownership` が出る（`OnCompositionTerminated` が `rpc_composition::close()` を `reset_all()` より先に呼ぶため。次の入力は空の状態から始まり、表示の害は見つかっていない）。
- #72（DllMain を空にし Activate で初期化）の確認（2026-10-10、開発ビルド 05:02:54）: `sudo cargo make install` が手順 5 まで完了。regsvr32 の登録・解除を 5 回繰り返して終了コードはすべて 0、クラッシュなし、Activate されない読み込みでログのファイル・`.lock` を作らない。変換・F9 → F10・設定の反映を確認。

## 2026-10-22 の結果（入力処理の見直しのフェーズ 2: 開発ビルド `317ae85`、engine ABI 12）

`cargo make build-engine` → `cargo make build-tsf` → サインアウト → サインイン → `sudo cargo make install` で導入。ホストのログで、ホストと engine DLL がどちらも `317ae85`・ABI 12 で読み込まれたことを確かめた。設定はライブ変換の `min_chars = 4`。

- T1 相当（`kanji` → Space → Enter、`kanji` → Backspace × 2 → Enter）: 合格。Space で変換・確定でき、Backspace 2 回で「か」が確定する。「かんじ」（3 文字）がライブ変換されないのは `min_chars = 4` の設定どおり。
- 部分確定の残りの F10（docs/Input_Pipeline_Plan.md 4.8 の F-4。3 文字ではライブ変換が始まらず範囲指定に入れないので、`kankou` で行った）: `kankou` → Shift+← × 2（「かん」）→ Space → Enter → F10 → F6 → Esc。合格（F10 で「kou」、F6 で「こう」）。
- 範囲変換の候補表示中の F10（同 F-6）: `kankou` → Shift+← × 2 → Space → F10 → F6 → Esc。合格（F10 で「kankou」、F6 で「かんこう」）。
- T2-1b（改訂後）: 合格。止めた後の Space で、ホストを起動して繋ぎ直し、流し直した後にそのまま変換し、候補ウィンドウが出た（モデルの読み込みを待ってから候補が出る）。2 回目の Space で次の候補へ移り、Enter で確定した。
- T2-2: 合格（「かんじか」）。
- T2-3: 合格。流し直した後も候補を選んでいる状態が保たれ、↓ で入力行が次の候補（「漢字」）に変わり、Enter で「漢字」が確定した（2026-10-10 の食い違いは解消）。
- 確かめていないもの（ホストを止めても次のキーで自動で起動し直すので、この手順では状態を作れない）: エンジンが 30 s 以上使えないときの処置（打った文字の救済・直接入力・背景の接続確認での解除）、IME をオフにした後の保留したキーの挿入、journal の上限。

## 2026-10-22 の結果（続き: ホストを起動できないとき。開発ビルド `bafb8d3`）

試験 G: メモ帳で `kanji` と打ちかけのまま、ホストの実行ファイル（`%LOCALAPPDATA%\rakukan\rakukan-engine-host.exe`）の名前を変えて起動できなくし、ホストを止める。`a` を打ち、少し待ってから `ka` などを打つ。最後に名前を戻す。

- 1 回目〜3 回目（`317ae85` とその後の開発ビルド）: 入力できなくなった。保留したキーの再生と、言語バーのツールチップ・タイマーなどの状態の問い合わせが、UI のスレッドでそのたびに繋ぎ直しを待ち（1 回 3.6〜12 s）、メモ帳とターミナルが反応しなくなった。最初のキーの待ちは約 11 s、その後の保留は 30 s。原因は足した診断のログ（`rpc SLOW … thread=`、`key_diag:`）で確かめ、`bafb8d3` で直した（docs/Input_Pipeline_Plan.md 4.2 の第 9 版の案・6c の注記 (b)）。
- 4 回目（`bafb8d3`）: 合格。
  - 最初のキーで止まったのは 1.8 s。その時点で起動できない（`HostMissing`）と判別し、30 s を待たずに composition の救済（「かんじa」の確定）と直接入力に切り替わった。
  - 「日本語入力を使えません。PC を再起動してください。直らない場合は、Rakukan を再インストールしてください。」の知らせがキャレットの近くに出た。
  - 直接入力の間、キーはアプリへ渡り、固まらなかった。
  - 名前を戻すと、背景の接続確認で約 10 s で日本語入力が戻り、「日本語入力が使えるようになりました。」の知らせが出た。試験の途中で起動したもう 1 つのアプリでも同じだった。
- 残る制限: 切断に気づく最初のキーの待ち（今回 1.8 s）。保留中に押した文字以外のキー（Enter など）は救済で捨てる。

## 2026-10-22 の結果（続き: 部分確定の文書先行 5c-3。開発ビルド `2822e88` + 未コミットの 5c-3）

`cargo make build-tsf` → サインアウト → サインイン → `sudo cargo make install` で導入（engine は変えていない）。メモ帳で、部分確定の 6 か所（docs/Input_Pipeline_Plan.md 4.6 の「部分確定の文書先行（5c-3）」）をそれぞれ通した。TSF のログで、どれも `commit_then_start[session]` を通り、引き継ぎ（`then=`）が想定どおりであることを確かめた。直近の TSF のログに、引き継ぎ・復元・確定の WARN は無かった。

- Selecting 中の文字（`kanji` → Space → `a` → `i` → Enter）: 合格。候補が確定し、続けて「あ」が未確定で出た（`then=InputChar`）。
- Selecting の Enter の残りあり（`kankou` → Shift+← × 2 → Space → Enter → Enter）: 合格。選んだ候補だけが確定し、「こう」が未確定で残った（`then=SeedPreedit{"こう"}`）。
- 番号キーの残りあり（`kankou` → Shift+← × 2 → Space → `2` → Enter）: 合格（`then=SeedPreedit{"こう"}`）。
- RangeSelect の部分確定（`kankou` → Shift+← × 2 → Enter → Enter）: 合格。「かん」がひらがなのまま確定し、「こう」が未確定で残った（`then=SeedPreedit{"こう"}`）。
- BlockSelecting 中の文字（`kyouha,ameda` → Space → `a` → Enter）: 合格。表示どおり全体が確定し、続けて「あ」が出た（`then=InputChar`）。Space の後、区間ごとの候補の先頭がひらがな（`merge_candidates` の並び「きょうは」「あめだ」）だったので全体がひらがなで確定したが、5c-3 では変えていない部分で、利用者の判断で問題なしとした。
- `on_input_raw` の Selecting（`kanji` → Space → テンキーの `-` → Enter）: 合格（`then=PushRaw(45)`）。テンキーの `*` `+` も問題なし。テンキーの `/` は記号の経路（`on_punctuate`）で「・」になり、部分確定の経路を通らなかった（次の節で直した）。
- 確定しなかった場合・ホストへの引き継ぎに失敗した場合の経路は、狙って起こせないので確かめていない（未確認）。

## 2026-10-22 の結果（続き: テンキーの記号。開発ビルド `01a5925` + 未コミットのテンキーの修正）

テンキーの `/` が「・」、`.` が「。」になっていた（`InputRaw` に、テンキー以外の記号の対応 `direct_input_symbol` を当てていた）。利用者の決定で、テンキーの `/` は全角「／」、`.` は全角「．」にした（`*` `+` `-` とメインのキーボードの `/` `.` は変えない。数字の後のテンキーの `.` は、小数点の機能が有効なら今までどおり `.`）。`cargo make build-tsf` → サインアウト → サインイン → `sudo cargo make install` で導入し、メモ帳で確かめた。

- テンキーの `/` `.` → Enter: 合格（「／．」）。
- `kanji` → Space → テンキーの `/` → Enter: 合格（候補が確定し、「／」が続く）。
- テンキーの `3` `.` `1` `4` → Enter: 合格（「3.14」）。
- メインのキーボードの `/` `.` → Enter: 合格（今までどおり「・。」）。

## 2026-10-22 の結果（続き: CommitThenInput の確定の再試行 5c-2 と表示の世代 `display_epoch`。開発ビルド `bdc6130` + 未コミットの 2 つ）

`cargo make build-tsf` → サインアウト → サインイン → `sudo cargo make install` で導入し、メモ帳で 3 つの試験を行った。TSF のログで、確定のやり直し（`retrying the key`）・表示の出し直し（`requested again`）・古い表示の飛ばし（`newer display`）・composition の保持し直しが出ていないことを確かめた。

- 試験 1（通常の入力と Selecting 中の文字）: `kyouhaiitenkidesu` → Enter、`kanji` → Space → `a` `i` を速めに → Enter。合格（`commit_then_start[session]: commit="感じ" then=InputChar`）。
- 試験 2（部分確定の残りの経路）: BlockSelecting 中の文字・Selecting の Enter の残りあり・番号キーの残りあり・RangeSelect の部分確定・テンキーの `-`。合格（引き継ぎはそれぞれ `InputChar` / `SeedPreedit{"こう"}` × 3 / `PushRaw(45)`）。
- 試験 3（確定・取り消し・フォーカスの移動の直後）: `k` → Enter × 3（「k」が 3 個。`commit_full` 3 回）、`ka` → Esc × 3（何も残らない。`end_composition` 3 回）、`kanji` のまま別のアプリをクリック → 戻って `a` → Enter（メモ帳がフォーカスを失った時点で composition を終わらせ、表示中の「かんじ」を確定した。`OnCompositionTerminated current=true`。今までと同じアプリ側の動作。戻った後の入力は通常どおり）、テンキーの `/` `.` `3` `.` `1` `4` → Enter（「／．3.14」）。合格。
- メモ帳では表示の要求がその場で実行されるため、`display_epoch` が古い要求を失効させる場面そのもの、と確定に失敗してやり直す経路は、起こせていない（未確認）。

## 2026-10-22 の結果（続き: フェーズ 2 の完了条件の残りとフェーズ 1 の確認。開発ビルド `3a2bc78` + 未コミットの修正）

`cargo make build-tsf` → サインアウト → サインイン → `sudo cargo make install` で導入し、メモ帳で行った。途中で見つかった不具合を直し、導入し直してから再試験した（直し方は Input_Pipeline_Plan.md の 7 節、2026-10-22 の実機試験の注記 (a)〜(d)）。

- T2-1: 合格（07:15:26 UTC にホストを繋ぎ直し（`GenMismatch` の後）、「かんじあ」が残った）。
- F-3: `sikan` と打つと「しかｎ」で残り、Shift+← が効かなかった（未変換のローマ字が残っている間は無視する。利用者の決定）。手順を `sikann` に改めて合格（「sikann」→「ｓｉｋａｎｎ」→「しかん」）。
- F-11: 合格（「ｋａｎｊｉ」→「ｋａｎｊｉあ」→「ｋａｎｊｉａ」）。
- 範囲の選択（R1）: 打っただけの「しかん」で Shift+← / Shift+→ が効かず、RangeSelect の Esc / Backspace で composition が空になっていた。直した後: Shift+→ で範囲を選べる、Esc / Backspace で「しかん」に戻る、「しかｎ」では矢印が無視される。合格。
- 言語バーのメニュー（M1）: メモ帳でメニューが出ず、`TrackPopupMenu` がエラー 87 で失敗していた。直した後: メニューが出て、選択と取り消しができる（ログ `from_focus=true`、失敗の WARN なし）。合格。
- 試験 8・12（ホストを起動できない状態で、未確定の「かんじ」の後に `ka` + Enter を素早く打つ）: 直す前は「かんじka」が入った後に改行が入った（UI のスレッドが止まっている間に押した Enter が、救済の後にアプリへ渡った）。直した後: 「かんじka」が確定し、改行は入らない。TSF のログで、`a`（`msg_time` が切り替えの 719 ms 前）と Enter（1500 ms 前）が `pressed_before=true` と判定され、Enter は `key pressed before direct input dropped` で捨てられた。7 s 後の Enter は改行になった。ホストの実行ファイルの名前を戻すと `host is back; direct input ended` で戻り、`a` で「あ」が出た。合格。
- フェーズ 1 の 2.0（October_Plan.md）: `log_level = "trace"` で起動し直したホストの `rakukan-engine-dll.log`（25,468 行。LLM の変換を含む）に `tokenizers::` の行は 0 件、rakukan の TRACE は 9,119 件。合格。
- フェーズ 1 の 2.1（#71）: ユーザーの環境変数 `RUST_LOG=info,rakukan_engine_rpc=debug` でサインインし直し、メモ帳で「今日はいい天気」を確定した後、`asita` を未確定のまま別の窓をクリックした。ホストのログに `change ResetAll seq=37 result=applied … committed_empty before=false after=true`（期待と今の Owner・世代が一致）。取り消して閉じたときも `result=applied … before=true after=true`。合格。試験の後に `RUST_LOG` を消した。
- 観察（調べていない）: ホストを起動できない状態で、最初のキーの待ちが 3.6 s になったことがある（`Transport` → `HostMissing` → `Suppressed`）。

## 2026-10-22 の結果（続き: F-1・F-2・T2-4。`de31d6a` と同じ内容の開発ビルド）

- F-1（Input_Pipeline_Plan.md 4.8）: ホストを止めて繋ぎ直した後（08:51:10 UTC）、「かんじあ」→ F9 × 3 → F10 → F6 で「ｋａｎｊｉａ」→「ＫＡＮＪＩＡ」→「Ｋａｎｊｉａ」→「Kanjia」→「かんじあ」。合格。
- T2-4: メモ帳で言語バーの「エンジン再起動」→ `trigger=ManualRestart force=true`・`rpc: Shutdown acknowledged by host`（08:52:38）、ホストのログに `Shutdown requested, exiting host process` の後の起動（08:52:39）。直後に打ったキーは `engine not ready … Input held` で保留され、「感じ」が確定した。合格。
- F-2: ホストを止めて繋ぎ直した後（08:54:57 UTC）、`kanjik` → `a` で「かんじか」、F10 で「kanjika」。合格。

## 0.13.0 の結果（2026-10-22。T4: `rakukan-0.13.0-setup.exe`、ビルド `7c76649`）

- T4（更新・インストール・アンインストール）: 合格（利用者の確認）。導入した後の `rakukan-engine-host.log` は `version=0.13.0 git=7c766499a275`、engine DLL は `abi=12 dll_version=0.13.0 dll_git=7c766499a275`（ホストと同じビルド、`-dirty` なし）。
- T1・T2: 0.13.0 で TSF・RPC・ホストを変えたので行った。開発ビルドで合格（この文書の 2026-10-22 の各節。T2-1〜T2-4 と、F の試験・ホストを起動できないときの試験を含む）。配布物は同じファイルを詰めたもので、インストーラーと配置は変えていないので、配布物ではやり直していない（試験を行う時期の決まり）。
- T3: 設定の読み込み・再接続の処理は変えていないので行っていない。
- 10 月の計画の試験 30（配布物での統合試験）は、フェーズ 3 の変更（配置・ログ）を含まない 0.13.0 では、T4 で足りるとした。

## 2026-10-23 の結果（S1: フェーズ 3 の Step 1。開発ビルド `500d161`）

`cargo make build-engine` → `cargo make build-tsf` → サインアウト → サインイン → `sudo cargo make install` で導入。ホストと engine DLL がどちらも `500d161acd1a`・ABI 12 で読み込まれた。

- S1-1: 合格。ホストのログは `rakukan-engine-host-<PID>-<起動識別子>.log` で 1 行目がヘッダー、`engine DLL pinned`、engine DLL の行は target `rakukan_engine_dll`。`ETW provider … not available` は無い。TSF のログのヘッダーとスレッド ID、`.lock` もある。
- S1-2: 合格。今のモデルは既存の HF のキャッシュから読まれた。任意の確認で `jinen-v2-xsmall-q5` を `C:\ProgramData\rakukan\models` に取得できた。ただし、モデルを替えて保存しただけでは、ホストが前のモデルのまま動いた（`converter reclaimed from conv_cache (no reload)` → `ShutdownIfConfigDiffers: config unchanged`）。言語バーの「エンジン再起動」で反映された（#49 のコメント）。手順に「エンジン再起動」を加えた。
- S1-3: 合格。`gpu_backend` だけを替えて保存しても反映されない（`engine_json_changed=false`。#49 のコメント）。手順に「エンジン再起動」を加えた。候補数も一緒に替えて、同じホストの中で CUDA 版から Vulkan 版に替わった場合は、ピン止めでホストは落ちなかったが、変換が 4.5〜12.4 s かかった（Vulkan 版だけのホストでは 50〜155 ms。#64 のコメント）。CUDA 版の DLL の import に `cudart64_*.dll` は無く、CUDA のものは `cublas64_13.dll` だけ（#90）。
- S1-4: 合格。`Rakukan.Tsf` 1035 件・`Rakukan.Host` 56 件、欠落 0。`tracerpt` の CSV で `level`・`target`・`message` が読めた（`target` が `rakukan_engine_dll` のものを含む）。打鍵は遅くならなかった。CSV・要約・ETL は確かめた後に消した。
- S1-5: 合格。設定アプリ・ホスト・engine DLL・辞書が `C:\Program Files\rakukan` から動き、ログは `%LOCALAPPDATA%\rakukan` に書かれた。設定アプリの左の欄の下とファイルのプロパティに「Copyright (c) 2024-2026 rakukan Contributors」がある（#88）。
  - 1 回目の戻す操作は管理者でない PowerShell で行い、登録が付け替わらないまま `%ProgramFiles%\rakukan` を消したため、IME が選べなくなった（`register_debug.log` に `create CLSID key: アクセスが拒否されました。 (0x80070005)`）。2 回目は、戻す操作の `regsvr32` が `register_debug.log` に記録を残さず（登録・解除の処理まで届いていない）、同じく選べなくなった。2 回目の原因は未確認。どちらも管理者の PowerShell で `regsvr32 "$env:LOCALAPPDATA\rakukan\rakukan_tsf.dll"` をやり直して戻った。手順を、`/s` を外し、登録先のレジストリと `register_debug.log` を確かめてから消す形に改めた。
- 後始末: `config.toml`・`keymap.toml`・`user_dict.toml` は試験の前のバックアップと一致。`learn_history.bin` は試験の確定で学習が増えて異なる。ログのレベルは `trace` のまま。

## 2026-10-23 の結果（S2: フェーズ 3 の Step 2・3。開発ビルド `f74a9c7`、そこから `-Sign` 付きで作ったインストーラー）

S2-0 の 4 で `sudo cargo make install` した開発の配置（`%LOCALAPPDATA%\rakukan`）を旧配置とした。アプリはメモ帳の代わりに EmEditor を使った。

- S2-0: 0.13.0 の配布物を `output\oldversion\rakukan-0.13.0-release-setup.exe` に写した（ハッシュ一致）。インストーラーの署名は有効（タイムスタンプ付き）。`Show-RakukanState` の `Keyboard` が空と出たのは関数の書き方の誤り（`Get-WinUserLanguageList` は一覧を 1 つの値で返す）で、直した。`register-tip.ps1` は一覧を変数に入れてから絞るので影響しない。
- S2-1: 合格。
  - 使用許諾の画面に、日本語と英語の見出しの文と [1]〜[4] が崩れずに出て、最後（NVIDIA の EULA の Attachment B の終わり）まで読めた。「同意しない」では先へ進めなかった。サインアウトを求められずに完走した。
  - セットアップのログに、旧 DLL の登録の解除（`exit code 0`）、HKCU の Run の `rakukan-tray` の削除、旧版のファイル・`settings-ui`・`dict` の削除があり、使用中の 4 つ（TSF DLL・CUDA 版の engine DLL・ホスト・トレイ）は再起動のときの削除の予約に入った。`TSF registration check failed` は無い。`etw`・`config.toml`・`rakukan.toml`・`*.bak`・ログは残った。
  - 再起動の前も EmEditor で変換・確定でき（旧ホストのまま）、文字は失われなかった。Program Files のトレイは起動しなかった（旧配置で動いていた新しいビルドのトレイが mutex を持っていたためと見られる。※推測）。
  - 再起動の後: ホスト・トレイ・engine DLL・辞書が `C:\Program Files\rakukan` から動き、変換できた。モデルは既存の HF のキャッシュから読まれた。予約は空、旧配置の旧版のファイルは消えた。トレイのログの 1 行目と掃除の行があった（一覧の補いは旧配置のトレイが 0.13.0 の分を済ませていたので行わない）。キーボードの一覧に rakukan があった。`models` は `BUILTIN\Users:(OI)(CI)(M)`。イベントログのアプリの異常終了に rakukan・explorer・EmEditor のものは無かった。設定のハッシュは試験の前と同じ。設定アプリに版と著作権の表示が出た。
  - `TIP WOW64`（`HKLM\SOFTWARE\WOW6432Node\Microsoft\CTF\TIP\{CLSID}`）は、旧配置・移行の後・再起動の後とも `True` で、S2-4 のアンインストールで `False` になった。
- S2-2: 合格。完了の画面に「インストールを完了するには、PC を再起動してください。再起動するまでは、Rakukan による日本語入力ができないことがあります。」と「今すぐ再起動しますか?」が出た。使用中の 4 つは置き換えの予約（`is-*.tmp`）になり、更新の後も EmEditor で確定できた。再起動の後に予約と `is-*.tmp` は無くなり、登録は変わらなかった。
- S2-3: 合格。`/RESTARTEXITCODE=3010` 付きで終了コード `3010`、付けずに `0`。セットアップのログに `Need to restart Windows? Yes` と `Will not restart Windows automatically.`。再起動の後に予約は無くなり、変換できた。画面に何も出なかったかは未確認。管理者の PowerShell では作業フォルダがリポジトリにならないので、手順をフルパスに改めた。
- S2-4: 合格。アンインストールの最後に再起動を求める表示が出た。再起動の前に、CLSID・TIP（HKLM・HKCU）・HKLM の Run・`TipListedVersion`・キーボードの一覧の項目・「アプリと機能」の登録が消え、使用中の 4 つと `C:\Program Files\rakukan` が削除の予約に入った（同じファイルが 4 回ずつ登録されていた）。再起動の後に `%ProgramFiles%\rakukan` と予約が無くなった。`%ProgramData%\rakukan\models`・`%APPDATA%\rakukan`（設定のハッシュは試験の前と同じ）・ログは残った。
- S2-5: 合格。`sudo cargo make install` と `register-tip.ps1` で、登録が `%LOCALAPPDATA%\rakukan\rakukan_tsf.dll`、HKCU の Run がトレイ、キーボードの一覧に rakukan、ホストが `%LOCALAPPDATA%\rakukan` から動き、変換できた。
- S2-6: 行っていない。
- 後始末: ログは S2-0 と S2-4 の前に `%APPDATA%\rakukan\log-backup-*` へ写した。ログのレベルは `trace` のまま。

## 2026-10-24 の結果（S3: フェーズ 3 の Step 4。開発ビルド `71f8bd1` + 未コミットの Step 4、そこから `cargo make package` で作ったインストーラー）

S3-0 の時点の旧配置は、S2-5 で `install.ps1` により入れた `%LOCALAPPDATA%\rakukan`。

- S3-0: 合格。設定を `backup-test-20261024-082342`、ログを `log-backup-20261024-082342` に写した（3385 ファイル）。`%ProgramFiles%\rakukan` は無く、`C:\rb\dict\rakukan.dict` はスクリプトの確認で作ったものがあった。0.13.0 の 32 ビット側のアンインストール情報が残っていた（`32-bit: True`）。
- S3-1: 合格。
  - `sudo cargo make install` は辞書を作り直さず（`already built`）、ISCC からサイレント実行まで完走した（`exit code: 0`、画面なし、再起動なし）。`#define MyAppVersion` は書き換えられなかった。
  - 再起動の前もメモ帳で変換できた。登録は `C:\Program Files\rakukan\rakukan_tsf.dll`、トレイは HKLM の Run、HKCU の Run は消えた。`%ProgramFiles%\rakukan` に `models` は無い。セットアップのログに移行の行と `migration: removed old uninstall entry … (App Path=C:\Users\n_fuk\AppData\Local\rakukan)` があり、アンインストール情報は `64-bit: True`・`32-bit: False`。「アプリ」の一覧の Rakukan IME は 1 つ。使用中の 4 つ（TSF DLL・CUDA 版の engine DLL・ホスト・トレイ）は削除の予約に入った。
  - 再起動の後: ホスト・トレイ・engine DLL・辞書（`mozc_path`）が `C:\Program Files\rakukan` から動き、変換できた。ホストと engine DLL のビルドは同じ（`71f8bd1aae78-dirty`）。予約は空、`%LOCALAPPDATA%\rakukan` の旧版のファイルは消えた。`TipListed` が `0.13.0`、トレイのログに `keyboard list checked for 0.13.0`。イベントログのアプリの異常終了に explorer・notepad・rakukan のものは無かった。
- S3-2: 合格。2 回目の `sudo cargo make install` は辞書を作り直さず（`C:\rb\dict\rakukan.dict` の更新日時が変わらない）、セットアップのログは別のファイルになった。
- S3-3: 合格。
  - `install-tsf-fault-test` の 1 回目はファイルのロックで止まった（`explorer.exe` とターミナル `ghostty.exe` が TSF DLL を読み込んでいた）。サインアウト → サインインの後の 2 回目で、SHA256 の一致（`3D3A35C8…`）と登録を確かめた。トレイは止まらなかった。
  - 起動スクリプトの 1 回目は、入力方式が MS-IME のままで charmap が TSF DLL を読み込まず、`ARMED=no`。入力方式を Rakukan にした後は `BUILD_KIND=fault-test`・`ARMED=yes`。手順に入力方式の確認を足した。
  - `install-tsf-normal-restore` も 1 回目はロックで止まり、サインアウト → サインインの後に通った（SHA256 `31DCD747…`）。`BUILD_KIND=normal`・`ARMED=no`、メモ帳で変換できた。
- S3-4: 合格。
  - 最初の `cargo make package` は、`check-bundled-licenses.ps1` で `Get-FileHash` が見つからずに止まった（cargo make から起動した Windows PowerShell 5.1 が PowerShell 7 のモジュールの場所を引き継ぐため）。`Get-FileHash` を .NET の SHA256 に置き換えて、やり直しで通った。
  - セットアップ本体（タイムスタンプ付き）、`dist\` の成果物 8 つ、サイレント実行で入れた後のアンインストーラー `unins000.exe` とインストール先の成果物が、すべて `CN=Noriaki Fukuyori` で `Valid`。配布物のサイレント実行は `exit code: 0`、再起動の後に変換できた。
  - `sudo cargo make uninstall` は `exit code: 0` で、画面は出なかった。再起動の前に登録・TIP（HKLM・HKCU）・HKLM の Run・`TipListed`・キーボードの一覧の項目・アンインストール情報（64・32 ビットとも）が消え、使用中の 4 つと `C:\Program Files\rakukan` が削除の予約に入った。再起動の後の `sudo cargo make install` では、既存のファイルが無い状態で入った。
  - 戻した後: 再起動の後に変換でき、登録・キーボードの一覧・`TipListed`（`0.13.0`）・ホストとトレイの場所が戻った。
- 後始末: 設定は `config.toml`・`keymap.toml`・`user_dict.toml` が S3-0 のバックアップと一致。`learn_history.bin` は試験の確定で学習が増えて異なる。ログは S3-0 と S3-4 の前に退避した。ログのレベルは `trace` のまま。`output\rakukan-0.13.0-setup.exe` は開発用のインストーラーに置き換わった（S3-4 の署名付きのものも、その後の install で置き換わった）。

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
