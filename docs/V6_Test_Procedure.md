# v6 統合試験・実機試験の手順（2026-10-10 作成、同日の試験結果で改定）

v6 のリリース条件（`docs/handoff.md` 0 章「2026-10-10 の更新」）の試験。各試験は「操作 → 見る点 → 戻す操作 → 戻した後に見る点」の順。ログは `%LOCALAPPDATA%\rakukan\` の `rakukan-tsf-<PID>-<起動識別子>.log`（アプリごと）と `rakukan-engine-host.log`。時刻順にまとめるときは `scripts/merge-logs.ps1`。

**共通の注意**: アプリからフォーカスが外れると、未確定の文字はアプリ側で確定される（PowerShell に移った時点で確定する）。未確定の文字を保ったままホストを入れ替える試験（T2・T3）は、PowerShell で**遅らせてから**ホストを止め、その間にアプリへ戻って操作する。PowerShell での確認は、アプリでの操作をすべて終えてから行う。

## 0. 準備

1. **設定のバックアップ**（PowerShell）
   ```powershell
   $bk = "$env:APPDATA\rakukan\backup-v6test-$(Get-Date -Format yyyyMMdd-HHmmss)"
   New-Item -ItemType Directory $bk | Out-Null
   Copy-Item "$env:APPDATA\rakukan\config.toml","$env:APPDATA\rakukan\keymap.toml","$env:APPDATA\rakukan\user_dict.toml","$env:APPDATA\rakukan\learn_history.bin" $bk -ErrorAction SilentlyContinue
   Get-FileHash "$bk\*" | Format-Table Hash, Path
   ```
   見る点: 4 ファイル（無いものは除く）とハッシュが表示される。
2. **ログのレベル**: 設定アプリ、または `config.toml` の `[general] log_level` を `"debug"` にする（元の値を控える）。反映はホストの起動し直しの後。
3. **開発ビルドの導入**: `cargo make build-engine` → `cargo make build-tsf` → サインアウト → サインイン → `sudo cargo make install`。
   見る点: `rakukan-engine-host.log` の起動時のビルドの一致の検査が通っている（`dll_git` と `host_git` が同じ）。メモ帳で変換できる。
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

見る点: 3 では変換せず、「かんじ」のまま未確定で残る（復元した直後の Space は Preedit に戻すだけ）。4 の Space で変換される。

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

見る点: アプリが落ちない。文字列が消えない。3 と 4 で画面がどうなったか（候補ウィンドウ・入力行の文字・確定された文字）を記録する（#69 の資料）。

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

## T4. パッケージのインストール・更新・アンインストール（現在の手順）

**版数を上げた後のパッケージで行う。** 版数を上げる前のパッケージは、入っているものと同じ版数・同じ中身に置き換わるだけで、更新の試験にならない。パッケージはレモンが `-Sign` 付きで作る（`docs/Installer_Redesign_Plan.md` 4.3）。

### T4-1 0.11.9 から v6 への更新
1. 0.11.9（または試験前に入っている版）の状態で、T1 の 1 で動作を確かめる。
2. v6 のパッケージで更新する（現在の手順: 別の IME に切り替え → サインアウト → サインイン → インストーラー実行 → サインアウト → サインイン）。
3. 見る点: インストーラーがエラーなく完了する。`rakukan-engine-host.log` の起動時の版数が v6 で、ビルドの一致の検査が通る。T1 の 1 と T2-1 が通る。`%APPDATA%\rakukan` の設定・ユーザー辞書・学習履歴が残っている（0 の 1 のハッシュと比べ、学習履歴は試験で変わりうる）。

### T4-2 アンインストールと再インストール
1. 「アプリと機能」から Rakukan を削除 → サインアウト → サインイン。
2. 見る点: キーボードの一覧に Rakukan が無い。`%APPDATA%\rakukan` が残っている。
3. 戻す操作: v6 のパッケージを再インストール（現在の手順）。
4. 戻した後に見る点: T1 の 1 が通る。設定が残っている。

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
