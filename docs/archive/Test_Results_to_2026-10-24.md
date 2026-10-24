# 試験の結果（2026-10-10〜2026-10-24、0.13.1 より前）

[Test_Procedure.md](../Test_Procedure.md) の末尾にあった、0.13.1 の結果より前の記録を移した（2026-10-24）。内容は記録したときのまま。手順は Test_Procedure.md を見る。

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

