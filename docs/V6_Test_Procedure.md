# v6 統合試験・実機試験の手順（2026-10-10、main ce7ae67）

v6 のリリース条件（`docs/handoff.md` 0 章「2026-10-10 の更新」）の試験。各試験は「操作 → 見る点 → 戻す操作 → 戻した後に見る点」の順。ログは `%LOCALAPPDATA%\rakukan\` の `rakukan-tsf-<PID>-<起動識別子>.log`（アプリごと）と `rakukan-engine-host.log`。時刻順にまとめるときは `scripts/merge-logs.ps1`。

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
3. **開発ビルドの導入**（main `ce7ae67` 以降）: `cargo make build-engine` → `cargo make build-tsf` → サインアウト → サインイン → `sudo cargo make install`。
   見る点: `rakukan-engine-host.log` の起動時のビルドの一致の検査が通っている。メモ帳で変換できる。
4. 試験に使うアプリ: メモ帳、EmEditor（2 つ以上の別プロセス）。

## T1. 通常の入力（実 DLL を通した `Change` / `Read`）

操作（メモ帳と EmEditor の両方で）:
1. `kanjiwohenkan` と打ち、Space で変換、候補を選んで Enter で確定。
2. ライブ変換が出る長さまで打ち、Enter で確定。
3. Backspace で未確定文字を消す、Esc で取り消す。
4. `kanji` と打って F9 / F10 → F6 で戻す。
5. Shift+矢印で範囲を選び、部分確定（Enter）。残りがそのまま未確定で残る。

見る点: 期待どおり入力・変換・確定できる。TSF ログに `composition change failed` / `composition mutation failed` / `composition read failed` が出ていない。

戻す操作: なし（確定した文字は文書ごと閉じて保存しない）。

## T2. ホストの交換中の composition の復元（#56）

### T2-1 未確定文字があるときにホストを止める
1. メモ帳で `kanji` と打つ（「かんじ」が未確定）。
2. PowerShell でホストを止める:
   ```powershell
   Get-Process rakukan-engine-host | Format-Table Id, StartTime
   Get-Process rakukan-engine-host | Stop-Process -Force
   ```
3. メモ帳に戻り、`a` を打つ。

見る点: 未確定文字が「かんじあ」になる（消えない・二重にならない）。TSF ログに `composition read failed: Rejected(GenMismatch)`（または通信の失敗）の後、`host connected: Hello/Create ok config=(rev=… sha256=…)` が出る。`Get-Process rakukan-engine-host` の Id と StartTime が変わっている。

4. Space で変換し、Enter で確定する（復元は 3 の `a` で済んでいるので、Space はそのまま変換になる）。

### T2-1b 復元のきっかけが Space のとき
1. メモ帳で `kanji` と打つ。
2. ホストを止める。
3. Space を押す。

見る点: 1 回目の Space では変換せず、「かんじ」のまま未確定で残る（復元した直後の Space は Preedit に戻すだけ）。2 回目の Space で変換される。

戻す操作: なし（ホストは次の入力で起動し直されている）。戻した後に見る点: そのまま普通に入力・変換できる。

### T2-2 未確定のローマ字があるとき
1. メモ帳で `kanjik` と打つ（「かんじk」）。
2. T2-1 の 2 と同じくホストを止める。
3. `a` を打つ。

見る点: 「かんじか」になる（`k` が失われない）。

### T2-3 候補を選んでいるとき
1. `kanji` → Space で候補ウィンドウを出す。
2. ホストを止める。
3. ↓ で次の候補、または Enter。

見る点: アプリが落ちない。確定できる、または未確定に戻る（文字列が消えない）。何が起きたかを記録する（#69 の経路の確認を兼ねる）。

### T2-4 言語バーの「エンジン再起動」
1. メモ帳で `kanji` と打つ。
2. 言語バー（またはトレイ）の「エンジン再起動」。
3. `a` を打つ。

見る点: T2-1 と同じ。TSF ログに `rpc: Shutdown acknowledged by host`。

### T2-5 2 つのアプリで交互に
1. メモ帳で `kanji`、EmEditor で `henkan` と打ち、どちらも未確定のままにする。
2. ホストを止める。
3. メモ帳で `a`、EmEditor で `a` を打つ。

見る点: メモ帳は「かんじあ」、EmEditor は「へんかんあ」。互いの文字が混ざらない。

戻す操作（T2 全体）: 未確定文字を Esc で消し、文書を保存せずに閉じる。

## T3. 設定を変えてからホストを入れ替える（#65 の再接続）

1. 現在の候補数（`num_candidates`）を控える（例: 19）。
2. 設定アプリで候補数を別の値（例: 9）にして保存する。
   見る点: 各 TSF ログに `config published: rev=… engine_json_changed=true pending_apply=true`。
3. ホストを止める（T2-1 の 2）。
4. メモ帳で `kanji` → Space。
   見る点: TSF ログの `host connected: Hello/Create ok config=(rev=… sha256=…)` の rev が 2 の `config published` の rev と同じかそれ以降。候補ウィンドウの候補数が 9 になっている。

戻す操作: 設定アプリで候補数を 1 の値に戻して保存 → ホストを止める → `kanji` → Space。
戻した後に見る点: 候補数が元に戻っている。`Hello/Create ok` の rev が戻した後の `config published` の rev。

## T4. パッケージのインストール・更新・アンインストール（現在の手順）

パッケージはレモンが `-Sign` 付きで作る（`docs/Installer_Redesign_Plan.md` 4.3）。

### T4-1 0.11.9 から v6 への更新
1. 0.11.9 のリリースの exe をインストールした状態にする（現在の手順: 別の IME に切り替え → サインアウト → サインイン → インストーラー実行 → サインアウト → サインイン）。T1 の 1 で動作を確かめる。
2. v6 のパッケージで更新する（同じ手順）。
3. 見る点: `rakukan-engine-host.log` のビルドの一致の検査が通る。T1 と T2-1 が通る。`%APPDATA%\rakukan` の設定・ユーザー辞書・学習履歴が残っている（0 の 1 のハッシュと比べ、学習履歴は試験で変わりうる）。

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
