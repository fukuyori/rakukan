# 入力処理の見直し（#69 / #70）設計書 第 9 版

作成: 2026-10-21（第 9 版: 第 8 版に対する 8 回目のレビューの残り 1 件（Y4。composition が無い場合の挿入位置）を受け、composition が無い場合は挿入せず記録と通知にした（4.2 の救済の表、試験 38）。8 回目のレビューの結果: Y1〜Y3 は解消、Y4 は一部解消で中 1 件、新たな二重書き込み・通知なしの欠落は見つからない。第 8 版の経緯: 第 7 版に対する Codex CLI の 7 回目のレビュー「修正してから進める」（Y1〜Y4。救済の再試行）を受け、回復できない場合の救済を 3 つの原則で簡素化した（4.2）。第 5 版までの変更は 0 節と 10 節。段 1 は A 節のとおり作業ツリーに実装済み・未コミット。A 節は中身を変えず、注記だけを足した）
対象: [October_Plan.md](October_Plan.md) 3 節（フェーズ 2）の B-1 定義・B-2 設計・B-3 実装順。
前提:
- 行番号は main `eee267e`（#71・ログの絞り込みをコミットした状態）で読み直したもの。
- 実機では確かめていない。コードからの推定・未検証の API の挙動は ※推測 と書く。
- 決定済み（利用者）: フェーズ 1 を土台にする。RPC protocol は v7 に上げる。回復できないホストでは、保留した文字キーを打った文字のまま文書へ入れ、文字以外のキーは捨て、回復まで直接入力にして表示を出す。未確認のキーは仮に表示し（例「かんじa」）、Enter は表示どおりに確定する。未確認の操作はすべての種類を仮表示に反映する。
- 決定済み（2026-10-21）: (1) フェーズ 2 はフェーズ 3 と同じリリースにする（engine ABI の変更は 1 回。protocol v7 は古いホストを止めるインストーラーと一緒に出す）。(2) 保留中にフォーカスが別の文書へ移ったら、元の文書が生きていれば文字キーを打った文字のまま入れ、文字以外は捨てる。生きていなければ捨てて WARN。(3) 正確に復元できたときは Space で通常どおり変換する（T2-1b の期待値が変わる）。(4) Enter で末尾の未確定 `n` は `ん` にして確定する（「表示どおり」の承認済みの例外。今の挙動）。

## A. 実装の段 1 の仕様（すぐ実装できる形）

目的: 今の挙動を変えずに、キーの解決と「食べるか」の判定を、試験できる関数に取り出す。以後の段の差分が、どの挙動を変えたかを試験で見えるようにする。

**対象のファイル**（これ以外は変えない）:
- `crates/rakukan-tsf/src/engine/keymap.rs`
- `crates/rakukan-tsf/src/tsf/factory.rs`

`dispatch.rs`・`on_*.rs`・`edit_ops.rs`・`state.rs`・`rpc_composition.rs` は変えない（dispatch.rs:27 のエンジンの取得と、その後の復元・初期化の順序をそのまま残す）。

**1. keymap.rs: キーの状態を読む口を取り出す**
- `trait KeyStateSource` を足す。メソッドと、今のコードで呼んでいる場所:
  - `modifiers(&self) -> Modifiers` … 今の `Modifiers::current()`（keymap.rs:181-204）をそのまま呼ぶ。中で左右のキーを読み、左右とも押されていない場合だけ汎用のキーを読む（keymap.rs:188-195）。この条件付きの読み方を作り直さない
  - `space_down(&self) -> bool` … `GetKeyState(0x20)`（keymap.rs:440）
  - `selecting(&self) -> bool` … `session_is_selecting_fast()`（keymap.rs:465）
  - `to_unicode(&self, vk: u16) -> Option<(i32, [u16; 2])>` … `GetKeyboardState` と `ToUnicode`（keymap.rs:493-500）。`GetKeyboardState` が失敗したら `None`（今の `?` と同じく、`resolve_action` 全体が `None` を返す。④ の Tab にも進まない）。
- 本番用の `Win32KeyState` は、上の Win32 API を今と同じ引数で呼ぶだけ。
- `pub fn resolve_action(&self, vk: u16) -> Option<UserAction>` は `self.resolve_action_with(vk, &Win32KeyState)` を返すだけにする。
- `resolve_action_with` の本体は今の `resolve_action`（keymap.rs:435-522）と同じ順序・同じ分岐にする: `modifiers()` → `space_down()` → `normalize_key_event` → `masked` → ① `resolve_mods` → ①.5 `essential_fallback_action` → ② `!ctrl && !alt` のときだけ `selecting()` → ② テンキー記号 → ③ `to_unicode()` → ④ Tab。各メソッドは今の API 呼び出しの位置でだけ呼ぶ（`ToUnicode` を前に動かさない。今は ① 〜 ② で戻る場合に `GetKeyboardState` / `ToUnicode` を呼ばない: keymap.rs:451-491）。

**2. factory.rs: 判定を関数に取り出す**（呼び出し側は今の位置のまま関数を呼ぶ）
- `fn is_ime_mode_action(action: &UserAction) -> bool` … `ImeToggle | ImeOn | ImeOff`。`OnTestKeyDown`（factory.rs:753-759）と `OnKeyDown`（factory.rs:857-870）の 2 か所の `matches!` を置き換える。
- `fn test_resolve(resolved: Option<UserAction>, vk: u16) -> Option<UserAction>` … `resolved.or_else(|| essential_fallback_action(vk))`。今の `OnTestKeyDown` は、keymap の借用に失敗した場合も `resolve_action` が `None` を返した場合も fallback に進む（factory.rs:730-745）。この両方を `resolved = None` として写す。
- `fn test_key_eat(action: &UserAction, ime_on: bool, has_preedit: impl FnOnce() -> bool, is_selecting: impl FnOnce() -> bool) -> bool`:
  - `ime_on` が false なら `is_ime_mode_action(action)` を返し、2 つの関数を呼ばない（今は IME オフで `engine_try_get_or_create` を呼ばない: factory.rs:753-759）。
  - true なら `has_preedit()` を呼び、次に `is_selecting()` を呼び（今の順: factory.rs:761-773）、`key_should_eat(action, hp || sel)` を返す。両方を必ず 1 回ずつ呼んでから組み合わせる（今は両方を計算してから `||` している）。
  - `OnTestKeyDown` は今の `has_preedit` の式（`engine_try_get_or_create` → `preedit_is_empty` → 失敗時は `composition_clone`: factory.rs:761-768）をそのままクロージャにして渡す。
- `fn keydown_ime_off_ignores(action: &UserAction, ime_on: bool) -> bool` … `!ime_on && !is_ime_mode_action(action)`。factory.rs:857-870 の条件を置き換える（`diag::event` とログはそのまま）。
- `fn keydown_return(result: &anyhow::Result<bool>) -> bool` … `Ok(ate)` は `ate`、`Err` は false（factory.rs:871-887）。`diag::event` と `tracing::warn!` の文言・順序はそのまま残す。
- `normalize_key_event_vk`（factory.rs:916-925。`GetKeyState` を 4 回読む）は、両方の入口で今の位置のまま先に呼ぶ。
- `OnKeyDown` の「keymap が取れなければ unmapped で FALSE」（factory.rs:806-827）は変えない。fallback を揃えるのは段 2。

**3. 変えてはいけないこと**
- Win32 API の呼び出しの有無・順序・引数、`engine_try_get_or_create` の呼び出しの有無と順序、ログ・`diag::event` の文言と順序、返す TRUE / FALSE。
- `key_should_eat`（factory.rs:1077-1110）の中身。

**4. 今の挙動を固定する試験**（`cargo test -p rakukan-tsf --lib` を PowerShell から実行）
- keymap.rs（偽の `KeyStateSource` が呼ばれたメソッドを順に記録する）:
  - 前提: 1・2・4〜9 は、対象のキーに割り当ての無い keymap（`Keymap::placeholder()`: keymap.rs:531。割り当てが空）で試す。プリセットを含む keymap では ① の割り当て（keymap.rs:452）が先に効き、`to_unicode` まで届かないことがある。
  1. 割り当ての無い Enter（vk 0x0D）→ `CommitRaw`。`selecting` と `to_unicode` は呼ばれない。
  2. Ctrl+Enter（割り当て無し）→ `CommitRaw`（①.5 は修飾キーを見ない）。
  3. 割り当てのあるキー（既存の試験 keymap.rs:995 と同じ作り方の keymap）→ その操作。①.5 以降のメソッドは呼ばれない。
  4. `1`（0x31）、修飾キー無し、`selecting = true` → `CandidateSelect(1)`、`to_unicode` は呼ばれない。`selecting = false` → `to_unicode` が呼ばれ `Input('1')`。Ctrl+`1` では `selecting` を呼ばない。
  5. テンキー 0x6F → `InputRaw('/')`、`to_unicode` は呼ばれない。
  6. Shift+A（`to_unicode` が `A`）→ `Input('A')`。
  7. `to_unicode` が制御文字・0 文字 → vk 0x09 は `Tab`、それ以外は `None`。
  8. `to_unicode` が `None`（`GetKeyboardState` の失敗）→ vk 0x09 でも `None`。
  9. 呼び出しの順序が `modifiers` → `space_down` →（必要なら）`selecting` → `to_unicode` であること。
- factory.rs:
  10. `key_should_eat` の表: `UserAction`（user_action.rs:4 の全 variant）× `has_preedit` の全組み合わせを、今の `match` を写した期待値の表と比べる（既存の `key_should_eat_tests`: factory.rs:1512-1541 に足す）。
  11. `test_key_eat`: `ime_on = false` で 2 つのクロージャが呼ばれず、モードキーだけ true。`ime_on = true` で `has_preedit` → `is_selecting` の順に 1 回ずつ呼ばれる。
  12. `test_resolve(None, 0x0D) == Some(CommitRaw)`、`test_resolve(None, 0x41) == None`、`test_resolve(Some(x), _) == Some(x)`。KeyDown は `None` で unmapped（fallback 無し）のまま、という今の相違をこの試験の名前と注記で固定する（段 2 で期待値を変える）。
  13. `keydown_ime_off_ignores` と `keydown_return` の表。

**5. 完了の条件**: 上の試験が通る。`cargo make check` が通る。差分が 2 つのファイルに収まり、呼び出しの順序の比較（変更前後の関数を並べて読む）で、API・エンジン取得・ログの順序が同じと確かめられる。試験 12 は関数だけを確かめるので、実際の借用の失敗と KeyDown の unmapped の分岐は差分のレビューで確かめる: factory.rs:730-745（`OnTestKeyDown` の `try_borrow` の範囲と fallback への分岐）と factory.rs:807-824（`OnKeyDown` の `try_borrow` の範囲・unmapped の早期 return・`tracing::debug!` と `diag::event` の文言と順序）が変更前と同じであること。規模の目安は試験を含めて 250〜450 行（※推測）。

## 0. 版ごとの主な変更

第 4 版（第 3 版から。3 回目のレビュー N1〜N6）:

| 項目 | 第 3 版 | 第 4 版 |
|---|---|---|
| 時間切れの要求（N1） | 時間切れ後に遅れて実行されたら文書の確定だけ行う | 要求ごとの状態の箱（`ReqCell`）を、EditSession の先頭と時間切れの処理が 1 回ずつ読み書きし、「実行開始」と「失効」のどちらが先かを一意に決める。失効した要求は文書に触れない。実行した要求の結果は必ず回収する。`InsertTxn`・表示の要求も同じ（4.6） |
| 宛先の違うキー（N2） | 所有スレッドに列が 1 本 | 宛先（文脈 + `focus_epoch`）ごとの列を、着いた順に並べる。先頭の列だけを実行し、後の列は待つ。A → B → A は 3 本の列（4.2） |
| Full 確定（N3） | 「SetText は要らない」 | 「表示どおり」と「指定の置き換えの後に確定」（末尾の `ん`・CycleKana のカタカナ）を分け、置き換えの SetText が失敗したら今と同じく表示どおりに確定する（4.6） |
| 読みの長さの自動確定（N4） | 表示どおりに確定 | 自動の確定には「表示どおり」を適用しない。表示の要求の完了を待ち、範囲の文字列が計画と一致したときだけ確定する（4.6 の確定の理由） |
| 確定の理由（N5） | ― | `CommitReason` を記録し、理由ごとに文字列の決め方・失敗時の処置・終わり方を決める。回復できない場合は最初の確定が失敗しても有限の手順で直接入力に入る（4.2, 4.6） |
| 段 1・段 4（N6） | 段 1 で入口に `KeyEvent` を作る、段 4 で上限を有効化 | 段 1 は今の評価順と副作用をそのまま残す（A 節）。各段に「追加」と「有効化」を分けて書き、上限・縮約の有効化は依存する段の後に移す（7 節） |
| 代替期間の F | 「F は今より悪くならない」 | 縮約を有効にするのは ABI の段（段 7）の後。それまで代替の `SeedPreedit` を使うのは Rebased のときだけで、そのときの F の制限を明記（4.8） |
| 付記 | ― | `HostLink` は通知を受けたら最新の状態を読み直してから直接入力を外す。`INPUT_OWNER` は Deactivate・終了通知・各タイマーにも適用する。写しを作らない経路は `push_char` の早期 return を含む全 `current_preedit()` を外す |
| 事実 | `ensure_connected` が最大 5 s | 5 s は spawn 後のパイプ接続待ち 1 回の上限。`ensure_connected` と `call_with_retry_inner` が再試行するので、全体の上限は 5 s ではない（1 節） |

第 3 版（第 2 版から。R1〜R10）: 今のキーの未実行の部分（`KeyTask`）、確定待ちのキーの引き継ぎ、直接挿入のトランザクション、範囲選択中のログの保持、`EngineAccess` と `HostLink` の分離、Enter と表示の修復の順、写しを作らない経路と上限、実装順、保留したモードキーの破棄、所有スレッド、`checkpoint_edit` / `final_edit`、再開のタイマー。

第 2 版（草案から）: 照合点と末尾の常時適用、確定トランザクション、表示の要求値・成功値、保留キーの受け付けと実行の分離、`SeedPreedit` / `RetainRange`、Codex の順の実装計画。

## 1. 現状の要点（コード上の事実）

キーの入口
- `handle_action` は冒頭で `engine_try_get_or_create()?` を呼ぶ（dispatch.rs:27）。エンジンが無ければ初期化を始めて Err、ロックが取れなければ `engine busy` の Err（state.rs:379-392）。IME のオン/オフのキーもここで Err になる。この関数が確かめるのは、ロックが取れることと手元のエンジンの口（`RpcEngine`）があることだけで、ホストとの接続は確かめない。
- 背景の初期化は、エンジンの口が既にあれば何もしない（state.rs:302-304）。
- `OnKeyDown` は `handle_action` の Err を常に FALSE（アプリへ渡す）にする（factory.rs:880-887）。
- keymap が取れないとき、`OnTestKeyDown` は `essential_fallback_action(vk)` を使い（factory.rs:737-745）、`OnKeyDown` は unmapped として FALSE を返す（factory.rs:806-827）。今は Test と KeyDown で扱いが違う。
- `OnTestKeyDown` の `has_preedit` は、エンジンが取れないと false（factory.rs:761-768）。RPC を送るのは、IME オンで、割り当てがあり、エンジンが取れ、手元に編集状態がある場合だけ（rpc_composition.rs:521-524）。
- `essential_fallback_action` は修飾キーを見ない（keymap.rs:977-988）。`resolve_action` の ①.5 も同じ関数を使う（keymap.rs:455-461）。候補番号は `session_is_selecting_fast()` に依存（keymap.rs:465）。修飾キーと文字は解決時に `GetKeyState` / `GetKeyboardState` / `ToUnicode` で読む（keymap.rs:435-447, 493-500。関数全体は keymap.rs:435-522）。

手元の復元元・台帳
- `RemoteComposition { owner, generation, edit, pending, bg_reading, tsf_comp, created_under }`（rpc_composition.rs:28-41）。鍵は DM のポインタだけ（rpc_composition.rs:64-68）。
- `pending` は 1 件で、`apply_via`（rpc_composition.rs:268-309）の 278, 292, 297, 304 行で上書き。未解決に阻まれた変更は保存しない（303-305 行）。`reset_preedit` は `close()` で手元の状態を消す（rpc_composition.rs:681-685）。
- 結果不明の要求はプロセスで 1 件（client.rs:164, 1034）。`Restore` は未解決があっても送れ、`Restored` で台帳を空にする（client.rs:1084, 1101）。
- `engine_health` は失敗を `ok` に置き換える（client.rs:910-913）。接続の確認には使えない。

ホスト
- `Restore` は保存済み応答の照合を先に行う（server.rs:984）。適用は `reset_preedit` → `force_preedit` → `push_char` → BG 回収 → `bg_owner = None` → `owner` の変更 → `then`（server.rs:996-1007）。過去の打鍵ログは消える（lib.rs:696-697）が、`push_char` と `then` で新しいログは作られる。
- `apply_change` は各操作の結果に `preedit_display`・`hiragana_text`・`EditState` の写しを作る（server.rs:1113 以降）。
- 照合の順は「世代 → 番号 → 所有者」。同じ番号の再送には、所有者が替わっていても当時の応答を返す（server.rs:222-241）。記録は TSF の ID ごと（server.rs:75-79）。

エンジン
- `push_char` は毎回 `current_preedit()`（読みと未確定ローマ字の写し）を返す（lib.rs:809, 1118-1122）。FFI はその戻り値を捨てるが、写しは作られる（ffi.rs:293-299）。
- `force_preedit` は表示を置き換え、ログは残し、`log_detached_at` をログの長さにする（lib.rs:832-837）。`hiragana_from_romaji_log` / `romaji_log_str` はログ全体を連結する（lib.rs:1130-1142）。`on_latin_convert` は表示がログ由来のかなで始まらないと、頭を除いたローマ字を作る（edit_ops.rs:129-134）。
- かな → 綴りの逆引き `spellings_for`（romaji/rules.rs:403）。engine ABI は 11（rakukan-engine-abi/src/lib.rs:23、rakukan-engine/src/ffi.rs:20）。

表示と確定
- `update_composition` は `COMPOSITION_APPLY_LOCK` が取れないと SetText を飛ばし（on_compose.rs:201-208）、`RequestEditSession` の結果を捨てる（on_compose.rs:267-270）。
- `commit_then_start_composition` は EditSession の冒頭で composition を取り出し（on_compose.rs:325）、確定部分の末尾の位置を `insert_after_commit` に控え（on_compose.rs:351-366）、縮める SetText の失敗は WARN だけで EndComposition に進む（on_compose.rs:355-362, 372-374）。要求の結果は `TF_S_ASYNC` などを見て印を消すだけ（on_compose.rs:474-490）。
- `end_composition` も冒頭で取り出し（on_compose.rs:769）、EndComposition の失敗は `?` で抜ける（on_compose.rs:831-837）。取り出した composition は戻さない。
- Enter の各経路は、ホストの `commit` と `reset_preedit` を文書の確定より先に送る（on_convert.rs:1880-1893, 1939-1942）。
- Backspace で空になると `end_composition("")` で閉じるが、手元の編集状態は閉じない（on_convert.rs:2047-2068）。

状態の置き場所
- プロセス共通: `SESSION_STATE`（state.rs:1620）、`COMPOSITION`（state.rs:1211。`dm_ptr` 付き）、`COMPOSITIONS`（rpc_composition.rs:25）、`RAKUKAN_ENGINE`。
- スレッドローカル: 候補ウィンドウの HWND（candidate_window.rs:157-159）、今の DM（candidate_window.rs:184）、DM の台帳（dm_registry.rs:253）、フォーカスの遅延処理の列など。TSF の TIP はスレッドごとに Activate される（factory.rs:379）。※推測: 1 つのプロセスで複数のスレッドが TSF を使うと、スレッドローカルの値とプロセス共通の composition・セッションが食い違う。今のコードに、1 つのスレッドだけが使うことを保証する仕組みは見当たらない。
- `Preedit{text}` の中身は経路で違う（通常の入力は読みだけ: on_input.rs:329-331、復元は読み + 未確定ローマ字: dispatch.rs:63-66）。
- `PENDING_KEYS`（state.rs:231-266）に積むのは engine=None の分岐だけ（on_input.rs:93-112, 363-374）。dispatcher が先に Err で抜けるので届かない。
- フォーカス通知は遅延処理に送る（factory.rs:1362-1399 → candidate_window.rs:1015, 1052）。`OnPushContext` / `OnPopContext` は何もしない（factory.rs:1401-1406）。
- 範囲変換は、エンジンの読みを選択部分に縮める（on_convert.rs:395）。取消では全体の読みに戻す（on_convert.rs:289-305）。ログは縮めない。
- ロックを長く持つ処理: 初期化のスレッド（state.rs:300-313。spawn 後のパイプ接続待ちは 1 回あたり最大 5 s: client.rs:43。`ensure_connected` は失敗すると 200 ms 後にもう 1 回試し: client.rs:1186-1207、`call_with_retry_inner` も 2 回試す: client.rs:1144-1170 ので、全体の上限は 5 s ではない）、`engine_reload_impl`（state.rs:742, 785, 788）。

## 2. B-1 定義

### 2.1 キーの段階と、1 打鍵に含まれる複数の操作

キーの段階はキーに付け、編集要求・表示・確定はそれぞれ別の記録で持つ。1 打鍵は「以前の分の復元」「表示の修復」「今のキーの編集要求（0〜n 件）」「表示の要求」「確定トランザクション（0〜1 件）」「ホストの副作用」を含みうる。

| 段階 | 意味 | 持ち主 |
|---|---|---|
| K0 受け取った | `KeyEvent` を作り（4.1）、食べるかを決めた | 受け付け |
| K1 保留した | 実行を待っている | 入力の列（4.2） |
| K1' 続きを待っている | 実行を始めたが、未実行の部分（`KeyTask`）を残して止まった | 入力の列の先頭（4.1） |
| K2 記録した | 編集要求を journal に入れた。ホストの適用は未確認 | journal の末尾（4.3） |
| K3 確かめた | そのキーの編集要求がすべて確認済みの接頭部に入った | journal |
| K4 文書操作を頼んだ | 表示か確定の EditSession を要求した | 表示の記録・確定トランザクション |
| K5 文書操作が終わった | EditSession の結果が記録された | 同上 |

規則:
- 列から取り出すことと、journal への追加は TSF のスレッドの同じ処理の中で行う。同じ編集要求が列と journal の両方にあることはない。
- 列へ戻すのは `KeyTask`（未実行の部分）だけ。実行済みの部分（journal に入れた編集要求・要求した EditSession）は戻さない。
- 文字を生むキー（4.1 の定義）は、文書が編集を受け付けない場合（4.6）を除き、捨てない。

### 2.2 読み・未確定ローマ字・表示

| 値 | 正の置き場所 | 手元の写し |
|---|---|---|
| 読み・未確定ローマ字 | journal。ホストの状態はその適用結果 | 照合点の `EditState`（`checkpoint_edit`） |
| 打鍵ログ | ホストのエンジン。journal を最初から適用すると同じものができる | 持たない |
| 表示要求値 `requested` | 最後に頼んだ表示（表示要求 ID 付き） | 入力セッション |
| 表示成功値 `shown` | EditSession の中で SetText が成功した文字列（ID 付き） | 入力セッション |
| 仮表示 | 照合点の状態に未確認の末尾を手元で当てた文字列（4.5） | 計算 |
| 範囲の元の文字列 `original_text` / 確定した文字列 `committed_text` | 確定の EditSession の中で `GetText` で読んだ範囲全体の文字列（照合・元に戻す用）と、実際に文書に確定した文字列（学習・文脈に使う）（4.6） | 確定トランザクション |

### 2.3 アプリへの確定の段階

確定トランザクション（4.6）・直接挿入トランザクション（4.2）・表示の要求に共通の段階:

| 段階 | 状態 |
|---|---|
| T0 計画した | 手元で決めた。ホストにも文書にも何もしていない |
| T1 要求した | `RequestEditSession` を受け付け、まだ実行されていない（`ReqCell = Pending`） |
| T2 実行中 | EditSession の先頭で `ReqCell` を `Pending` → `Running` にした |
| T3 文書の結果あり | 結果（下）を記録し、`ReqCell = Done(結果)` にした |
| T4 ホストの処理済み | 副作用・新しい composition の状態を送った（成否は副作用ごと） |
| T5 閉じた | 結び付いたキーを次の入力世代へ渡した、または再試行・破棄を決めた |
| TV 失効した | T1 のまま 2 s 経ち、時間切れの処理が `ReqCell` を `Pending` → `Void` にした。以後この要求が実行されても文書に触れない |

文書の結果: `Committed`（C2）/ `CommittedNoNew`（C1）/ `Aborted`（C0）/ `AbortedChanged`（C0'。文字列が計画と違うまま残った）/ `NotRun`（要求の失敗・拒否・古い要求・失効）。直接挿入は `Inserted` / `NotInserted`。

## 3. 識別子

| 識別子 | 内容 | 進める契機 |
|---|---|---|
| `DmRef` | DM のポインタ + 世代（dm_registry.rs:34-40） | 既存 |
| 文脈の識別 | `ITfContext` の `IUnknown` のポインタ値。列は `ITfContext` の参照も持つ | ― |
| `focus_epoch` | スレッドローカルの `u64` | `OnSetFocus`（ThreadMgr）・`OnPushContext` / `OnPopContext`・`OnSetThreadFocus` / `OnKillThreadFocus`・`OnUninitDocumentMgr`・Deactivate の中で、同期で +1（加算だけ。msctf に再入しない） |
| `input_gen` | 入力セッションごとの `u64` | 入力セッションの開始 |
| 入力の系列 `lineage` | 同じ文脈で、確定をまたいで続く入力のまとまり。`focus_epoch` と文脈が同じ間は同じ値 | 文脈が替わったとき |
| 確定・挿入トランザクション ID | `u64` | 計画ごと |
| 表示要求 ID と表示の失効番号 `display_epoch` | 要求ごとの ID と、入力セッションの `u64` | `display_epoch` は確定トランザクションを作るとき・入力セッションを閉じるときに +1 |

失効と世代の更新を分ける:
- 失効（キーを元の宛先へ届けない）: `focus_epoch`・`DmRef`・文脈のどれかが違う。
- 世代の更新（正常な確定で新しい入力セッションになる）: `input_gen` が進むだけで、`lineage` は同じ。キーは確定トランザクションを通して新しい世代へ渡す（4.6）。`input_gen` が違うことだけでは失効にしない。

## 4. 設計

### 4.1 受け付け・食べる判定・`KeyTask`

**`KeyEvent`**（段 5 から。段 1 では今の評価順のまま: A 節）: 入口で vk、左右込みの修飾キー、Space の押下、`ToUnicode` の文字（制御文字を除く）、時刻、宛先（`DmRef`・文脈・`focus_epoch`・`lineage`）を固定する。解決時に `GetKeyState` 等を読み直さない。

**文字を生むキー**: `KeyEvent` に文字があり、Ctrl / Alt が無いもの（Space を含む）。

**解決**（この形は段 5 から。段 1 は A 節の `resolve_action_with` と `test_resolve` の抽出だけ）: `resolve(event, keymap: Option<&Keymap>, logical: LogicalState) -> Option<UserAction>`（純関数）。今の `resolve_action`（keymap.rs:435）はこれを呼ぶだけにする。keymap が取れないときの扱いは、7 節の 1 では今の相違（Test は `essential_fallback_action`、KeyDown は unmapped）をそのまま `resolve_test` / `resolve_keydown` の 2 つで写す。7 節の 2 で、両方を ①.5 と同じ扱い（修飾キーを見ない `essential_fallback_action`）に揃える。

**判定 `decide_eat(event, action, view) -> bool`**（純関数）。`view` は所有スレッド（4.4）のスレッドローカルの写し `InputView`。

| `InputView` | 意味 |
|---|---|
| `ime_on` | 実行済みのモード（アトミック） |
| `has_input` | その文脈の入力セッションに今の入力がある: composition を保持（stale でない）、`requested` か `shown` が空でない、仮表示が空でない、セッションが Idle でない、のどれか。journal の有無は使わない。入力セッションの DM が今の DM と違えば false |
| `selecting` | 同じ入力セッションの変換状態 |
| `queue_nonempty` / `commit_pending` / `replaying` | 4.2・4.6 |
| `direct_fallback` | 4.2 |
| `owner_here` | このスレッドがプロセス共通の状態を持っているか（4.4） |

| 条件 | 判定 |
|---|---|
| `owner_here` でない | 4.4 の規則（引き継げなければ食べない） |
| `direct_fallback` | IME のモードキーだけ食べる |
| IME オフ | モードキーだけ食べる（今と同じ） |
| 列・確定待ち・再生中 | アプリ用のショートカット（Ctrl / Alt 付きで、IME の割り当ても文字も無い）以外は食べる |
| 上以外 | `key_should_eat(action, has_input \|\| selecting)`（factory.rs:1077-1110）と、各ハンドラーが今 `Ok(false)` を返す条件 |

ロック: `InputView` は所有スレッドだけが読み書きし、ロックを使わない。状態を変える処理は直後に写しを更新する。

**`KeyOutcome`**: `Applied` / `Retained`（末尾が未確認）/ `DocPending`（確定トランザクションが T1）/ `DisplayStale` / `NoEffect{reason}` / `Suspended(KeyTask)`（未実行の部分を残して止まった）。食べると決めたキーは、どの結果でも TRUE を返す（7 節の 6 で有効にする）。

**`KeyTask`（未実行の部分）と続きの規則**（R1）:
- `KeyTask { event, action, step }`。`step` は「どこから続けるか」: `Start`（何もしていない）/ `AfterRepair`（入口の復元と表示の修復が済んだ）/ `AfterCommit{txn}`（確定の後の入力が残っている）。
- 入口の復元が Rebased（4.7）: セッションと表示を修復し、`step = AfterRepair` で同じキーを続ける。
  - 文字を生むキー（`Input` / `InputRaw` / `FullWidthSpace` / `Punctuate`）: 修復後の状態で 1 回だけ実行する（例: 「かんじ」+ `a` →「かんじあ」）。
  - 見えていた表示に依存する操作（`Convert`・候補の操作・F キー・Backspace・Esc・カーソル）: 修復だけで終える（`NoEffect(restored)`）。押した時点の表示がもう無いため。Enter は入口の復元を通らない（4.5）。
- 確定の後に入力が続くキー（CommitThenInput: 変換中の文字・BlockSelecting 中の文字）: 確定が `Aborted` / `AbortedChanged` / `NotRun` なら、`step = AfterCommit` の `KeyTask` を列の先頭に残す。表示を作り直してから、確定を新しいトランザクションで再試行する（4.6 の再試行）。確定が成功したら新しい文字を 1 回だけ実行する。
- 続きは 1 回だけ: `KeyTask` は列の先頭にしか置かず、実行を始めた時点で取り出す。止まった場合は、未実行の部分だけを持つ新しい `KeyTask` を置く。

### 4.2 保留キー・自動再開・回復できない場合（#70）

**エンジンとホストの状態を分ける**（R5）:
- `EngineAccess`（UI のスレッドで、待たずに分かる値）: `Ready(guard)`（ロックが取れ、エンジンの口がある）/ `Starting` / `Busy` / `NoEngine(reason)`（初期化が失敗した。`reason` は 4.9 の `ConnectError`）。
- `HostLink`（プロセス共通）: `{ state: Connected | Disconnected{kind, since}, link_epoch: u64 }`。RPC の成功・失敗（`ensure_connected` の結果を含む）で更新し、更新のたびに `link_epoch` を進める。`kind` は 4.9 の `ConnectError` の分類（`VersionMismatch` を区別）。
- `Ready` でも `HostLink` が `Disconnected` のことがある（口はあるが繋がっていない）。

**宛先ごとの列**（N2）:
- 宛先 `Dest = { dm: DmRef, ctx, focus_epoch }`。`focus_epoch` が違えば、同じ文脈でも別の宛先。
- `InputQueues`（所有スレッドに 1 つ）: `VecDeque<DestQueue>` を着いた順に並べる。`DestQueue = { dest, lineage, keys: VecDeque<KeyEvent | KeyTask>, bound_txn: Option<txn_id>, first_at, reason }`。1 本の列の中のキーは同じ宛先を持つ。
- 実行するのは先頭の列だけ。後ろの列は、先頭の列が空になり、先頭の列に結び付いたトランザクションが閉じるまで待つ。
- 新しい宛先のキーは、最後の列の宛先と違えば、新しい列を末尾に足して積む。古い列の処理を待たずに置き場所がある。
- 先頭の列の宛先が今のフォーカスと違う（`focus_epoch` が今の値と違う）とき、その列は前提の決定 2 で処理する: 元の文書が生きていれば、列の文字キーを直接挿入トランザクションで元の文脈へ打った文字のまま入れ、文字以外は捨てる。生きていなければ捨てて WARN。結び付いたトランザクションが T1 なら、その完了か失効を先に待つ。
- 例 A → B → A（`focus_epoch` 1 → 2 → 3）: 列は `[A@1, B@2, A@3]`。A@1 は元の文書 A へ文字を挿入（決定 2）、B@2 も今のフォーカスではないので同じく B へ挿入、A@3 は通常どおり実行する。A@1 と A@3 は同じ文書でも混ぜない。

**受け付け `accept` と実行 `execute` を分ける**:
- `accept`: `decide_eat` が false → アプリへ渡す。true → 列がある・確定待ち・再生中・エンジンが `Ready` でない、のどれかなら、宛先の列に積む。それ以外は列を先に実行し、このキーを `execute` する。
- `execute`: 列の判定をしない。実行時の `LogicalState` で `resolve` してハンドラーを呼ぶ。再生でも同じ `execute` を使う。
- 再生中は `replaying` を立て、届いたキーは列に積む。

**固定と解決**: vk・修飾キー・文字・宛先は受け取り時に固定（段 5 から。段 1 では今の評価順のまま: A 節）。`UserAction` は実行時に解決。

**モードキー**: 列があれば積む。積んだ時点ではモードを変えない。実行時に IME オフになっていた文字キーは、直接挿入トランザクションで文書へ入れる。実行時に意味を失った文字以外のキーは捨てて WARN。列が無くエンジンも無いときのモードキーは、エンジンを取らずに実行する。

**アプリ用のショートカット**は積まずにすぐ渡す。保留中の文字より先にアプリに届く（6 節の試験で固定する）。

**宛先の照合**（実行の直前）: `focus_epoch`・`GetFocus` の DM（生存）・文脈が同じ。`input_gen` は使わない（3 節）。違えば上の決定 2 の処理。

**再生の契機（必須）**:
1. `accept` の入口。
2. 再開のタイマー: 列があるか、トランザクションか表示の要求が T1 の間だけ動かす。間隔 50 ms。毎回、宛先の照合・上限・トランザクションの時間切れと完了の確認（通知の取りこぼしの補い）・`EngineAccess` の取得を行い、条件がそろえば再生する。
3. 初期化の完了・確定の完了・接続の回復の通知（`PostMessage`）。届かなくても 2 で拾う。
- タイマーの窓: Activate の中で `ensure_hwnd`（candidate_window.rs:881）を呼び、最初の候補表示より前に HWND を作る。HWND かタイマーを作れなかった場合は、列を使わない（キーは今と同じくアプリへ渡す。WARN を残し、次の Activate で作り直す）。Deactivate ではタイマーを止め、列は決定 2 の扱い（Deactivate の中では文書へ書けないので、捨てて WARN）。
- タイマーからの再生は「フォーカス中の文脈へのキー入力の続き」で、宛先の照合が通ったときだけ所有権を取り直す（rpc_composition.rs:1-2 の方針は、キーを伴わない読み取り・変更にだけ適用）。
- タイマーの処理は所有スレッドでだけ動く（4.4）。

**再生の途中**: 先頭の列のキーを 1 つずつ `execute` する。`Suspended` が返ったら止める（`KeyTask` は先頭）。それ以外の結果のキーは列から外す。`DocPending` の後は、そのトランザクションが閉じるまで止める。

**上限**: 列全体で 256 キー、または最初のキーから 30 s（根拠: spawn の抑止が 30 s: client.rs:39。毎秒 8 キー程度の打鍵で 30 s 分が約 240 キー）。`HostLink` が `Disconnected{VersionMismatch}` なら待たない。稼働中の切断（口はあるがホストが応えない）では、キーは実行されて journal の末尾が伸びる。末尾が 30 s 以上未確認、または 256 件を超えたら、同じく回復できない場合に進む（有効にするのは段 6）。

**回復できない場合の処置（救済）**（R3・N5・V1・V2・W1〜W3・W5。第 8 版で簡素化）:

救済はまれな経路なので、再試行の分岐を増やさず、次の 3 つの原則で決める（第 8 版の設計判断）:
- **二重にしない**: 同じ文字を文書に 2 回書く可能性のある操作をしない。
- **確かめられない位置に入れない**: 文字を入れる位置は composition の範囲そのものか、締め切りの後に利用者が何も操作していないと分かる選択位置だけ。
- **結果が不確かな書き込みを再試行しない**: 再試行してよいのは、前の試行が文書を変えていないと分かる場合（要求が実行されなかった `Void`、または SetText の失敗の後に範囲を読み直して元のままと確かめた場合）だけ。

結果が不確か、または位置を確かめられないときは、その文字について救済をやめる: それ以上書かず、救済できなかった文字列を記録し（WARN に文字数と本文。今のログも確定する本文を出している: on_compose.rs:334-338 など）、`mode_indicator` に「入力の一部を文書へ入れられませんでした」を出す。利用者の決定（保留した文字キーを打った文字のまま入れる）は、結果の確かな通常の経路で守る。

入口の条件: 回復できない状態（上の上限、または `VersionMismatch`）になったとき、所有スレッドに「受理したが確定していない入力」があること。対象は (a) 入力セッションの journal から作る未確定の文字列 `pending_text`（照合点の状態に未確認の末尾を当てた仮表示。変換の状態ならセッションの表示）と、(b) 列の文字キー。composition に表示があるかどうかは条件にしない（表示の要求が未実行・失敗で、composition が無い・空のこともある: on_compose.rs:193 で保持した後、201-212 で SetText が省略・失敗しうる）。どちらも無ければ、救済はせず手順 6 に進む。

救済の進み具合 `RescueState`（W2）: `Idle` / `Active` / `Finishing`。接続の状態 `HostLink` とは別に持つ。`direct_fallback` を外せるのは `RescueState = Idle` のときだけ（下の「直接入力」）。

1. **締め切り**（同期に行う。EditSession は使わない）:
   - 対象を固定する: `target = pending_text ++ 列の文字キーの文字`。以後は増やさない。
   - `shown_at_cutoff` を控える: その時点の表示成功値 `shown`（最後に SetText が成功した文字列。表示の要求が一度も成功していなければ空）。
   - 未実行の表示の要求をすべて失効させる（W1）: `display_epoch` を進め、`Pending` の表示の `ReqCell` をすべて `Void` にする。以後、失効した表示の要求が実行されても composition を作らず、SetText もしない。
   - 通常の再生と入口の表示の修復を止める: `RescueState = Active` の間、列の再生・`execute`・表示の修復・ライブ変換と待機のタイマーによる表示の更新を行わない。
   - `passed_since_cutoff = 0` にする（締め切りの後にアプリへ渡したキーの数。手順 2 で数える。ログに残すための値で、挿入の可否の判断には使わない）。
   - `direct_fallback` に入る。
2. **締め切りの後のキー**: モードキー以外はアプリへ渡し、`passed_since_cutoff` を数える。救済の対象には加えない。
3. 確定トランザクションが T1 なら、新しい要求を出さずに、その完了か失効（TV）を待つ（重複させない）。完了して確定済みの部分は対象と `shown_at_cutoff` から除く。
4. **救済の要求 `RescueTxn`**（理由 `Unrecoverable`。ホストには何も送らない）。1 つの EditSession の中で、実行の時点の状態で 1 回だけ判断する。先頭で `ReqCell` が `Void` なら何もしない。結果は `RescueOutcome` として記録する。

   | 実行の時点 | 処置 | 結果 `RescueOutcome` |
   |---|---|---|
   | composition を保持し、範囲の文字列が `shown_at_cutoff` と同じ | 範囲を `target` に SetText し（`Full(Overwrite)`）、EndComposition | 両方成功: `Rescued`（全部入った） |
   | 同上で SetText が失敗 | 範囲を読み直す。`shown_at_cutoff` のままなら何も変わっていない | `Retryable`（予算の中で、新しい `RescueTxn` をもう一度要求する）。読めない・違う: `Stopped{ unrescued: target }`（composition には触れずに残す） |
   | 同上で SetText は成功し EndComposition が失敗 | 元に戻さない。再試行もしない（EndComposition の結果が不確か） | `WrittenNotEnded`（`target` は composition の中にある。アプリがフォーカスの移動などで確定する。救済できなかった文字は無いが、確定していないことを WARN と通知に出す） |
   | composition を保持し、範囲の文字列が `shown_at_cutoff` と違う（締め切りの後にアプリが範囲を編集した） | 上書きしない。EndComposition だけを行う（利用者の編集を残す） | `PartlyKept{ unrescued: target のうち表示に出ていなかった部分 }`（`shown_at_cutoff` が `target` の先頭なら `target[len(shown_at_cutoff)..]`、そうでなければ `target` 全体を「確かめられない」として記録）。EndComposition が失敗しても再試行しない（同じ記録） |
   | composition を保持していない。締め切りの後に終了通知を受けた（アプリが確定した） | 書かない | `PartlyKept{ unrescued: 上と同じ計算 }` |
   | composition を保持していない（もともと無かった） | 書かない。選択位置が締め切りの時点から変わっていないことを確かめる手段が無い（`passed_since_cutoff` はキーしか数えず、マウスでの移動を検出できない） | `Stopped{ unrescued: target }`（記録と通知） |

   - 救済の `Overwrite` は、4.6 の共通の確定の「元に戻す」規則の対象外（4.6 の S1''・S4）。救済が書いた後に元に戻すことはない（Y1）。
   - `Retryable` だけを再試行する。再試行は新しい `id` と `ReqCell` で同じ表の判断をやり直す（前の試行で文書が変わっていないので、二重にならない）。
   - 挿入する位置は、composition の範囲そのものか、利用者が操作していないと分かる選択位置だけ。保存した範囲（`comp_end_anchor`）を使う挿入はしない（第 7 版の「確定済み・挿入待ち」と `anchor` は削除した。Y3）。
5. **予算を使い切ったとき**（`Retryable` が 4 回続いたとき）: composition を保持し、範囲が `shown_at_cutoff` のままなら、EndComposition だけを最後の 1 回試す（`UnrecoverableAsDisplayed`。表示どおりの確定）。成功しても救済全体の成功ではない: 結果は `PartlyKept{ unrescued: target のうち表示に出ていなかった部分 }` で、残りを別の位置へ挿入はしない（記録と通知）。失敗したら `Stopped{ unrescued: target }`。composition が無ければ `Stopped{ unrescued: target }`（Y2）。
6. `Rescued` 以外の結果は、救済できなかった文字列を記録し通知する（上）。列の文字以外のキーは、モードキーを含めて捨てる（件数を WARN）（R9）。対象・列・トランザクションを片づけ、`RescueState = Idle` にする。
7. 直接入力のまま、回復の確認を待つ。救済の間に接続の回復の通知を受けていた場合は、ここで `HostLink` を読み直して判断する（下の「直接入力」）。

**予算と時間**（V2・W5）: 救済の要求は「`Retryable` の 4 回 + 最後の 1 回（手順 5）」。これに手順 3 の既存の要求の待ちが 1 回加わる。各要求は `Pending` のまま 2 s で失効させる（4.6 の `ReqCell`。失効は文書を変えていないので `Retryable`）。`Pending` の待ち枠の合計の目安は (1 + 4 + 1) × 2 s ≒ 12 s。実際の所要時間には、これに `Running` の EditSession の実行時間と、タイマー（50 ms 間隔）・通知の処理の遅れが加わる。`Running` には時間の上限を設けない（TSF の呼び出しが戻ることを前提にする。戻らなければ所有スレッドごと止まっており、この設計では扱えない）。無条件の時間の保証はしない。

**直接挿入トランザクション `InsertTxn`**（救済以外: 実行時に IME オフになっていた文字キー、前提の決定 2）: `{ id, dest, text, keys: その文字を出した列の範囲, position, req: ReqCell, stage, retries }`。
- 挿入位置: その時点の選択位置。
- 列の文字は `Inserted` が記録されるまで列から消さない。この間に届いたキーは列に積む（追い越さない）。
- `NotInserted`・失効: 再開のタイマーで最大 3 回、新しい `InsertTxn`（新しい `id` と `ReqCell`）で再試行する。失効した前の要求は、後から実行されても文書に触れない（4.6 の `ReqCell`）。3 回とも失敗したら、文書が編集を受け付けないとみなし、その文字を捨てて WARN。
- 宛先の照合に失敗したら、決定 2 の処理（元の文書が生きていれば元の文脈へ、生きていなければ捨てて WARN）。

**直接入力 `direct_fallback`**:
- モードキー以外は食べない。`mode_indicator` と言語バーのツールチップに「エンジン停止中（直接入力）」を出す。
- 直接入力に入った後に新しく受けたモードキーは通常どおり処理する（モードの値は変わるが、`direct_fallback` の間は IME の入力はしない）。
- 外す条件（R5）: 背景の接続確認の成功。確認用のスレッドが 5 s ごとに `RAKUKAN_ENGINE` を `try_lock` で取り（取れなければその回は飛ばす）、口が無ければ初期化を促し、口があれば失敗を返す確認の RPC（新しい `RpcEngine::probe()`: `EngineHealth` を送り、`Hello` の版の照合を含めて成否を返す。今の `engine_health` は失敗を隠す: client.rs:910-913）を送る。成功したら `HostLink` を `Connected` にし（`link_epoch` が進む）、その `link_epoch` を付けて UI のスレッドへ `PostMessage` する。
- UI のスレッドは通知を受けたら、まず `RescueState` を見る。`Active` / `Finishing`（救済の途中）なら外さず、「回復の通知を受けた」印だけを残す（W2）。`Idle` なら `HostLink` を読み直し、`Connected` で `link_epoch` が通知の値と同じときだけ `direct_fallback` を外して表示を消す。違えば（その間に切断した）外さず、次の確認を待つ。
- 救済が終わった時点（手順 7）で印があれば、そのときの `HostLink` を読み直し、`Connected` なら外す。印の時点の値は使わない。
- UI のスレッドでは待たない。

**届かないコードの削除**: on_input.rs:93-126, 363-387 の engine=None の分岐と drain、state.rs:222-266 の `PENDING_KEYS`。

**回復しないと分かったときの早い切り替えと知らせ（第 9 版の案、2026-10-22。#70）**

2026-10-22 の実機確認（G: ホストの実行ファイルを起動できない状態）で、文字は失わないが、利用者から見ると「約 40 s 固まり（最初のキーの繋ぎ直しの待ち約 11 s と保留の 30 s）、その後は日本語が打てない」状態で、壊れたとしか受け取れないことが分かった。待っても戻らないと確かに分かる場合は待たずに切り替え、何が起きたかと戻し方を知らせる。

- **待っても戻らないと確かに分かる状態**（`HostLink` の `Disconnected` の分類で判断する）:
  - `VersionMismatch`（版の違うホストが動いている）。
  - 新しい分類 `HostMissing`: 起動を試みて、実行ファイルが無い・起動できないで失敗した（`spawn_host` の失敗）。
  - `Suppressed`: 起動の失敗が続き、起動の抑止に入った（短い時間に 3 回失敗）。
  - それ以外（`Transport`・`HelloError`・`CreateError`）は一時的な切断もありうるので、今までどおり保持の上限（30 s）まで待つ。
  - 受け入れた危険（利用者の決定、2026-10-22）: `Suppressed`・`HostMissing` は一時的な原因でも入りうる（ホストの起動が遅れている間に抑止に入った、別のプロセスがホストを起動している途中で 0.5 s の接続の試行が終わった、など）。その場合も直接入力に切り替わるが、背景の接続確認（5 s ごと）で繋がれば自動で戻り、「日本語入力が使えるようになりました」を出す。その間に保留していた文字は打ったまま入り、文字以外のキーは失われる。「起動はするがすぐ落ちる」場合を主に `Suppressed` で見分けるので、外して 30 s 待つより、この危険を受け入れる。
- **早い切り替え**: 保留したキーがあり、上の状態なら、30 s を待たずに回復できない場合の処置（救済と直接入力）に進む。
- **最初のキーの待ちを短くする**: 起動を試みて失敗した（実行ファイルが無い）ときは、起動の後の接続の待ち（5 s）をせず、短い接続の試行（抑止中と同じ 0.5 s）だけにする（別のプロセスが起動したホストへ繋がる場合は今までどおり成功扱い）。見込み: 最初のキーの待ちが約 11 s から約 1〜2 s になる（※推測。実機で測る）。
- **知らせ**: 回復できない場合の処置で直接入力に入るとき（早い切り替えでも、30 s の上限でも）、次の文面を出す。原因ごとに文面を分けない（PC の再起動でほぼ戻り、戻らない場合は再インストールで戻るため）。
  - 文面: 「日本語入力を使えません。PC を再起動してください。直らない場合は、Rakukan を再インストールしてください。」
  - 出す場所: (a) キャレットの近くに出す、文を表示できる知らせの窓（新しく作る。`tsf/notice.rs`。`mode_indicator` と同じく TSF の窓で、キャレットの下に出す。キャレットの位置が取れなければ前面の窓の下寄りの中央に出す。使えないときの知らせは 10 s、戻ったときの知らせは 3 s で消える。打鍵の途中に出るので、キーでは消さない）。(b) 言語バーのツールチップを同じ文面にする（直接入力の間。先頭に `rakukan: ` を付ける）。トレイの通知は、トレイが通知の仕組みを持たないので、この版では使わない（インストールの見直し #33 でトレイを扱うときに考える）。
  - ログ（WARN）には分類（`HostMissing` など）と最後のエラーを残す。
  - 直接入力が外れたとき（背景の接続確認で回復した）は、知らせの窓に「日本語入力が使えるようになりました」を出す。
- **変えないこと**: 打った文字を打ったまま入れる救済、文字以外のキーを捨てること、背景の接続確認で自動で戻ること。

### 4.3 journal と復元

**持つもの**（`RemoteComposition` に追加。`pending` は廃止）:
- `journal: Vec<ChangeRequest>`: 編集要求（`InputChar`（`bg_start_n_cands` は `None` で記録。今の呼び出しはすべて `None`: on_input.rs:124, 148, 224, 291, 320, 384）・`PushChar`・`PushRaw`・`PushFullwidthAlpha`・`Backspace`・`FlushPendingN`・`ForcePreedit`・`SeedPreedit`・`RetainRange`）。
- `checkpoint: (len, checkpoint_edit)`: journal の先頭 `len` 件を適用した状態として、ホストが確かめた値。
- `generation`、副作用の記録 `effects`（4.7）、費用の合計 `cost`（下）。

**送り方**:
1. 上限を判定してから（下）、編集要求を journal の末尾に入れる（送る前）。
2. 末尾が今の要求だけで、所有権があり、台帳に未解決が無ければ `Change`。`Changed` なら `checkpoint = (len, outcome.edit)`。
3. それ以外は `RestoreJournal`。
4. `Change` の失敗: 通信失敗・`ResultUnavailable` は結果不明、`OwnerMismatch` / `GenMismatch` は未適用、`Unresolved` は未送信。journal はそのまま、照合点も進めない。所有権を失った場合は同じキーの中で 3 を行う。

**`RestoreJournal`**（ホスト、1 回のエンジンロックの中）:
1. 番号の照合（`Restore` と同じ。同じ番号には保存した応答）。
2. `pre_effects`（4.6）を順に適用。
3. `reset_preedit`。
4. `checkpoint = Some{len, checkpoint_edit}` なら先頭 `len` 件を適用し、`EditState` を `checkpoint_edit` と比べる。一致なら残りを適用（`Exact`）。不一致なら `reset_preedit` → `SeedPreedit(checkpoint_edit.reading)` → `checkpoint_edit.pending_romaji` の各文字を `push_char` → `journal[len..]` を適用（`Rebased`）。`None` なら全体を適用（`Exact`）。
5. BG を回収し、`bg_owner = None`、`owner = owner`。
6. `RestoredJournal { engine_gen, replay, final_edit, bg, effects }` を記録して返す。
- エンジンの操作のエラー（BG の mutex の poison だけ。配布ビルドは `panic = "abort"`）: `reset_preedit`・`owner = None` にして `Error`。

**TSF 側**:

| 応答 | 照合点 | journal | 所有権 |
|---|---|---|---|
| `Exact` | `(journal の長さ, final_edit)` | そのまま | `generation = engine_gen` |
| `Rebased` | 書き換えた journal の長さと `final_edit` | `[SeedPreedit(checkpoint_edit.reading), PushChar(checkpoint_edit の未確定ローマ字)…] + journal[len..]`（送った照合点の値で作る。`final_edit` で作ると末尾が二重になる） | 同上 |
| `ResultUnavailable` / 通信失敗 | 進めない | そのまま | 不明（次は必ず `RestoreJournal`） |
| その他の拒否 / `Error` | 進めない | そのまま | 無し |

- 今のキーは journal の末尾（`then` は使わない）。
- 照合点の不一致は、確定済みの表示を基点に末尾を当てる。主な原因は設定の変更と推定する（※推測）。

**台帳との関係**:
- 不変条件: 台帳に未解決として残りうる要求は、(a) どこかの composition の journal にある編集要求、(b) 副作用の記録で「結果不明」と記録済みの副作用・BG 操作、のどちらか。
- この条件のもとで、`RestoredJournal` が台帳を空にしても失われない。同じプロセスの別の composition の未解決は、その composition の次のキーで流し直される。別プロセスの台帳は独立で、各プロセスの次のキーで流し直される。
- 保存した応答の再送（client.rs:1144-1170 の 2 回目）で受けた `RestoredJournal` は「その時点で適用し所有者になった」ことの証拠で、今の所有権の証拠ではない。照合点は進め、所有権は楽観的に持ち、次の `Change` が `OwnerMismatch` なら全体を流し直す。
- 副作用は journal に入れない（4.7）。

**費用**（R7）:
- エンジン内部に、写しを作らない操作の経路を作る。`push_char` の本体を戻り値なしの内部関数 `push_char_inner` に分け、今 `current_preedit()` を返している 5 か所（早期 return の lib.rs:744, 763, 775, 794 と末尾の 809）をすべて内部関数からは外す。公開の `push_char` は `push_char_inner` の後に `current_preedit()` を 1 回返す包みとして残す（既存の試験はこちらを使う）。FFI（ffi.rs:293-299）と流し直しは `push_char_inner` を呼ぶ。DLL 内部の変更で、engine ABI は変えない。
- ホストの流し直しは `apply_edit`（操作だけを行い、結果を作らない）を使う。BG の確認は最初に 1 回。`EditState` を作るのは照合点と最後の 2 回。
- 費用 `cost` は「処理する文字列の総量」で数える: 各操作の大きさ（`InputChar` 等は 1 文字、`ForcePreedit` / `SeedPreedit` は文字列のバイト数）に、その操作がエンジンで触る文字列の長さの上限（`Backspace` は打鍵ログの末尾の区間の長さ、`ForcePreedit` / `SeedPreedit` / `RetainRange` は結果の読みの長さ）を足したもの。通常の打鍵の写しをなくすことと、Backspace などを含む総費用を見積もることは分けて扱う。※推測: 1 回の復元の処理量は `cost` にほぼ比例する。試験で測る。

**上限**（追加の前に判定する。有効にする段を右端に書く。それまでは判定して DEBUG に残すだけ）:

| 上限 | 値 | 達したとき | 有効にする段 |
|---|---|---|---|
| 読みの長さ | 2,000 文字 | そのキーを `KeyTask`（`AfterCommit`）にして止め、理由 `LengthLimit` の確定を要求する（4.6。表示の要求の完了を待ち、範囲の文字列が計画どおりのときだけ確定する）。成功したら、そのキーを新しい入力セッションで実行する。受理した文字で表示に出ていないものは、確定せず次の入力に残す（4.6） | 5 |
| 軟上限 | 512 件、または `cost` 32 KiB | 末尾が空なら、journal を `[SeedPreedit(読み), PushChar(未確定ローマ字)…]` に縮める（F は種の綴りになる: 4.8）。末尾が空でなければ縮めずに続ける | 7（engine ABI の `seed_preedit` と同時） |
| 硬上限 | 1,024 件、または `cost` 128 KiB | 末尾が空なら縮めてから追加する（段 7 より前は縮めないので、そのまま追加する）。末尾が空でなければ、そのキーを `Suspended`（`JournalFull`）にする。ホストが応えない状態なので、4.2 の上限で回復できない場合に進む | 6 |
| 1 件の大きさ | `ForcePreedit` / `SeedPreedit` の文字列は読みの長さの上限以下 | 超える要求は作らない（読みの長さの上限が先に効く） | 5 |

- 読みの長さの上限があるので、縮めた journal（`SeedPreedit` 1 件）の `cost` も軟上限以下に収まる（2,000 文字 × 最大 4 バイト）。縮めても収まらない入力は、読みの長さの上限で先に確定されるので起きない。
- 段 4〜6 の間は縮めないので、長い composition では journal が伸びる。同じリリース（前提の決定 1）の中の中間の状態で、配布はしない。
- フレームの上限 8 MiB（codec.rs:15）より十分小さい。
- 目安: ホストは流し直しの時間を測り、20 ms を超えたら WARN（判定には使わない）。
- 所有権が毎打鍵入れ替わる場合（2 アプリが交互に打つ）は毎回全体を送り、累積は O(n²) になる。上限で抑える。
- `Restore`（v6）は protocol から消さない。TSF は使わない。

### 4.4 スレッドと入力セッション（R10）

- プロセス共通の状態（`SESSION_STATE`・`COMPOSITION`・`COMPOSITIONS`・台帳）は、その時点で入力を持つ 1 つのスレッドが所有する。所有者はプロセス共通のアトミック `INPUT_OWNER: (thread_id, busy)`。`busy` は、そのスレッドに入力セッション・列・未完了のトランザクション・表示の要求のどれかがある間 true。
- 所有の移動: 別のスレッドで食べるべきキーを受けたとき、所有者が `busy` でなければ、そのスレッドが所有者になる（比較交換）。前の所有者のスレッドローカルの値は、そのスレッドが次に動くときに「所有者でない」と分かり、写しを空にする。
- 所有者が `busy` の間に別のスレッドで受けたキーは食べない（今の IME オフと同じ扱い）。WARN を 1 回残す。※推測: アプリはフォーカスの移動で composition を終わらせるので、通常は前の所有者が `busy` でなくなってからキーが届く。
- 所有者の確認はキーの入口だけでなく、プロセス共通の状態に触るすべての入口で行う:
  - Deactivate: 所有者でないスレッドの Deactivate は、自分のスレッドローカルの値（候補ウィンドウ・タイマー・列・トランザクションの表）だけを片づけ、プロセス共通の状態には触れない。今の Deactivate は `composition_set(None)`・`take_current`・`set_idle` を無条件に行う（factory.rs:640, 643, 657）。所有者のスレッドの Deactivate は今と同じ後始末を行い、所有者を空にする。
  - 終了通知（`OnCompositionTerminated`: factory.rs:667-711）: 通知の composition が所有スレッドの保持しているものでなければ、プロセス共通の状態に触れない（今の `composition_is_current` の照合に加え、スレッドを確かめる）。
  - タイマー（再開・ライブ変換・待機）と遅延処理（フォーカス・モード）: 所有スレッドでだけプロセス共通の状態に触る。
- スレッドローカルの値（`InputView`・列・`focus_epoch`・候補ウィンドウ・タイマー・トランザクションの表）は所有スレッドの値だけを使う。
- 入力セッションは所有スレッドに 1 つ。`RemoteComposition` の鍵を `DmRef` にする（rpc_composition.rs:64-68）。
- 別の文脈へのキー（同じスレッド）: 4.2 の宛先ごとの列で扱う。新しい宛先の列は、古い宛先の列とそのトランザクションが終わるまで待つ。古い列の処理は:
  - 確定トランザクションが T1 なら、その完了か失効を待つ。
  - 確定待ちが無く、古い composition を保持していて古い文脈が生きていれば、理由 `ContextSwitch` で確定する（4.6）。その完了を待つ。
  - 生きていなければ手元の状態を外し、#71 と同じ後始末（`finish_composition(…, true)`）を送る。
  - その後、新しい宛先の列を先頭にし、新しい入力セッションを始める。
- 規模に含める（7 節の 3）。

### 4.5 仮表示と Enter（B）

**仮表示**（照合点の状態に未確認の末尾を手元で当てる。ローマ字の変換は手元ではしない）:

| 未確認の操作 | 仮表示への反映 |
|---|---|
| `InputChar{c, Char}` / `PushChar(c)` | 未確定部分の末尾に `c` を足す（「かんじ」+`a` →「かんじa」） |
| `InputChar{c, FullwidthAlpha}` / `PushFullwidthAlpha(c)` | `c`（ASCII 大文字）を足す |
| `InputChar{c, Raw}` / `PushRaw(c)` | `c` を足す |
| `Backspace` | 末尾の 1 文字を消す（足した文字、無ければ確認済みの未確定ローマ字、無ければ読みの末尾） |
| `FlushPendingN` | 未確定部分がちょうど `n` なら `ん` に置き換える |
| `ForcePreedit{text}` / `SeedPreedit{text}` | 全体を `text` にし、未確定部分を空にする |
| `RetainRange{start, end}` | 読みの `start..end` 文字を残し、未確定部分を空にする |

- 確認できた時点で表示を作り直す。変換の状態の表示はセッションの候補から作る。LiveConv で今のキーが未確認になったら Preedit（仮表示）に戻し、ライブのタイマーと preview の列を止める。

**表示要求の失効**（R6）:
- 表示の EditSession は、要求時の表示要求 ID と `display_epoch` を持つ。実行時に `display_epoch` が進んでいれば SetText せずに終える（`shown` も変えない）。
- 確定トランザクションを作るとき `display_epoch` を進める。確定より前に要求され、まだ実行されていない表示は、確定の後に適用されない。

**Enter（`CommitRaw`）**（R6。利用者の決定「表示どおり」）:
- Enter は入口の復元も表示の修復も行わない（今も入口の復元を通らない: dispatch.rs:46-48）。押した時点で見えている文字を確定する。
- 計画は手元の値だけで作る（`flush_pending_n` も読み取りも送らない）。確定トランザクションを作り、`display_epoch` を進める。
- EditSession の中で composition の範囲を `GetText` で読み、`original_text` として記録する。文書に確定した文字列は `committed_text`（4.6）として記録し、学習の判定（`committed_text` が計画の文字列と同じときだけ学習）と文脈（`Commit`）には `committed_text` だけを使う。
- 末尾の未確定 `n`（承認済みの例外。前提の決定 4）: 計画の表示の未確定部分がちょうど `n`（照合点の `pending_romaji == "n"`、または仮表示で足した部分が `n` だけ）なら、確定の種類を `Full(Replace{ text: 末尾を ん にした文字列, rule: TrailingN })` にする（4.6）。範囲の文字列が計画の表示と同じなら、文書の文字列を `ん` に SetText してから確定する。今の `flush_pending_n`（on_convert.rs:1907）と同じ結果。確定・学習・文脈に使う実績値は置き換えの後の文字列。範囲の文字列が計画と違う場合と、置き換えの SetText が失敗した場合は、範囲の文字列のまま確定する（4.6）。
- 範囲の文字列が `expected_display` と違う場合（表示の要求が省略・失効していた）: Full はそのまま `original_text` を確定する。Partial は分け方が分からないので確定せず（`Aborted`）、表示を作り直す。Enter は文字を生まないので、`KeyTask` は残さない（もう一度 Enter を押すと確定する）。
- 入力の無い Enter は食べない。入力があって `original_text` が空なら、composition を閉じて食べる。

**入口の表示の修復**: Enter 以外のキーは、実行の最初に「表示が古い」印（`requested != shown`・省略・失敗）を見て、セッションと journal から表示を作り直してから処理する。

### 4.6 確定トランザクション（C）と部分確定（A）

**順序**: 文書を先に確定し、ホストの副作用は文書の結果を見てから送る。EditSession の中では RPC を送らない。ただし部分確定（Partial / CommitThenInput）は、EditSession の中で EndComposition が成功した後に `RestoreJournal` を 1 回と、成功したときの表示の読み出しを 1 回送る（下の「部分確定の文書先行（5c-3）」。2026-10-22 改訂）。

**記録**: `CommitTxn { id, reason: CommitReason, kind, dm, ctx, lineage, input_gen, old_comp, plan, req: ReqCell, stage, doc_result, original_text, committed_text, post_commit_range, effects, bound_keys, retries }`。所有スレッドの表に置き、EditSession は `id` と `ReqCell` を持つ。

**種類 `kind`**（N3）:

| 種類 | 文書への操作 | 今の関数 |
|---|---|---|
| `Full(AsDisplayed)` | 範囲の文字列をそのまま確定する（SetText しない） | `end_composition`（on_compose.rs:751） |
| `Full(Replace{ text, rule })` | 範囲の文字列が `expected_display` と同じなら `text` に SetText してから確定する。`rule` は `TrailingN`（末尾の未確定 `n` を `ん`）と `Katakana`（CycleKana: edit_ops.rs:181 で `to_katakana`、今は on_compose.rs:795 で SetText してから確定） | 同上 |
| `Full(Overwrite{ text, expect })` | 範囲の文字列が `expect`（`shown_at_cutoff`）と同じときだけ `text` に SetText してから確定する。救済だけで使う。失敗の扱いは 4.2 の救済の表で決め、共通の「元に戻す」規則の対象外 | 同上 |
| `Partial` | 確定部分に縮めて確定し、残りで新しい composition を始める | `commit_then_start_composition`（on_compose.rs:280） |
| `CommitThenInput` | 変換の全体を確定し、新しい文字で新しい composition を始める | 同上 |

- `Replace` の SetText が失敗したとき（同じ EditSession で 1 回再試行しても失敗）: 今の `end_composition` と同じく、置き換えずに範囲の文字列のまま確定する（on_compose.rs:795-816 の「SetText できなくても EndComposition までは進める」）。結果は `Committed`、`committed_text` は確定した文字列（置き換え前）、`replace_failed = true` を記録し WARN。学習は `committed_text` が計画どおりのときだけなので、この場合は学習しない。`Overwrite`（救済だけで使う）の失敗の扱いは 4.2 の救済の表だけで決め、この節の共通の規則（元に戻す・`Aborted` / `AbortedChanged`）は適用しない。
- `Replace` の SetText が成功し、EndComposition が失敗したとき（V3）: 同じ EditSession で範囲を `original_text` に戻す SetText を試す。戻せたら `Aborted`（文書は元のまま）。戻せなければ `AbortedChanged`（文書は置き換え後の文字列のまま composition に残る。`shown` を置き換え後の文字列にし、表示が古い印を立てる）。
- `Replace` で範囲の文字列が `expected_display` と違うとき: 置き換えない。`rule = TrailingN` は `AsDisplayed` と同じに確定する（Enter は表示どおりが基本。見えている `n` が未確定のものか分からない）。`rule = Katakana` は確定しない（`Aborted`。表示を作り直す。CycleKana は文字を生まないので再試行しない）。

**確定の理由 `CommitReason`**（N4・N5）:

| 理由 | 起点 | 種類 | 範囲の文字列が計画と違う・表示が古い場合 | 失敗したとき |
|---|---|---|---|---|
| `Enter` | 利用者 | `Full(AsDisplayed)` / `Full(Replace{TrailingN})` / `Partial` | Full は見えている文字を確定（利用者の決定）。Partial は `Aborted` | 再試行しない（Enter は文字を生まない） |
| `CycleKana` | 利用者 | `Full(Replace{Katakana})` | `Aborted` | 再試行しない |
| `ImeSwitch`（`commit_visible_composition`: edit_ops.rs:469） | 利用者 | `Full(AsDisplayed)` | 見えている文字を確定 | 再試行しない |
| `CommitThenInput` | 利用者の文字キー | `CommitThenInput` | `Aborted` | 表示を作り直してから最大 3 回（R1）。その後は「文書が編集を受け付けない」 |
| `LengthLimit`（4.3） | 自動 | `Full(AsDisplayed)` | 確定しない（下の「自動の確定」） | 最大 3 回。その後は「文書が編集を受け付けない」 |
| `ContextSwitch`（4.4） | 自動 | `Full(AsDisplayed)` | 確定しない | 最大 3 回。その後は範囲の文字列をそのまま確定（`ContextSwitchAsDisplayed`、表示に出ていない部分の件数を WARN）。それも失敗したら手元の状態を外す |
| `Unrecoverable`（4.2） | 自動 | `RescueTxn`（実行の時点で composition があり範囲が締め切りの時点の表示と同じなら `Full(Overwrite)`。無ければ、利用者が操作していないときだけ挿入） | 表示の完了は待たない。範囲が違えば上書きしない（W3） | 4.2 の救済の表。再試行は文書が変わっていないと分かる `Retryable` だけ、4 回 + 最後の 1 回。結果が不確かなら救済をやめて記録と通知 |

**自動の確定**（`LengthLimit` / `ContextSwitch`。`Unrecoverable` は 4.2 の手順）: Enter の「見えている文字を確定」は適用しない（N4）。
1. 表示が古ければ表示の要求を出し、その完了（`ReqCell = Done` で SetText 成功、`shown == requested`）を待つ。表示の要求が失効・失敗したら作り直して待つ（最大 3 回。その後は「失敗したとき」の列）。
2. 確定トランザクションの `expected_display` を、その時点の `requested`（受理したすべての操作を反映した表示）にする。
3. EditSession の中で範囲の文字列が `expected_display` と同じときだけ確定する。違えば `Aborted`（受理した文字が表示に出ていないので、確定しない）。
4. 成功したら、確定した文字列に対応しない受理済みの操作（確定トランザクションを作った後に届いたキー）は、`bound_keys` として次の入力に残す。journal は確定した読みまでで閉じ、残りは新しい入力世代の journal へ引き継ぐ。

**`ReqCell`（要求の状態の箱）**（N1）:
- 確定・直接挿入・表示の EditSession ごとに `Rc<Cell<ReqState>>` を作る。`ReqState = Pending | Running | Done(結果) | Void`。所有スレッドの値で、EditSession も時間切れの処理も同じスレッドで動くので、読みと書きは 1 回の処理の中で終わる。
- EditSession の先頭: `Pending` なら `Running` にして続ける。`Void` なら文書にも手元にも触れずにすぐ戻る（`composition_take` もしない）。それ以外はありえない（WARN）。
- 時間切れの処理（再開のタイマー、T1 のまま 2 s）: `Pending` なら `Void` にし、トランザクションを `NotRun(timeout)` にする。`Running` / `Done` なら失効させない（実行済みの結果を使う）。
- 再試行は、前の要求が `Void` か `Done` になってからだけ、新しい `id` と `ReqCell` で要求する。同じ composition に 2 つの確定の要求が同時に `Pending` になることはない。
- 実行した要求の結果は必ず回収する: EditSession の最後で `Done(結果)` にし、`PostMessage(WM_APP_COMMIT_DONE, id)` を送る。通知が届かなくても、再開のタイマーが表の `Done` を拾う。
- 終了通知: 自分の EditSession の EndComposition（`Running` の間）に届いた `OnCompositionTerminated`（factory.rs:667）は、その確定トランザクションのものとして扱い、セッションの片づけをしない（#71 の `set_pending_replacement`: on_compose.rs:308 を、`ReqCell` が `Running` の間だけ有効な印に広げる）。※推測: TIP 自身の EndComposition では通知は来ない。来た場合もこの規則で二重に片づけない。`Void` の要求は文書に触れないので、遅れた実行が終了通知を起こすことはない。

**EditSession の手順と結果**:
- 先頭で `ReqCell` を確かめた後、`COMPOSITION` の composition が `old_comp` と同じかを確かめる。違えば `NotRun(stale)`。
- 取り出した composition は、確定しなかった結果で必ず `composition_set_with_dm` で戻す（今は戻さない: on_compose.rs:325, 769）。

| 手順 | 失敗したとき | 結果 |
|---|---|---|
| S1 範囲の取得・`GetText` → `original_text`（照合と元に戻す用） | 何もしない | `Aborted` |
| S1' `original_text` と `expected_display` の照合（Partial / CommitThenInput / `Replace` / 自動の確定） | 4.6 の種類と理由の表のとおり | `Aborted` など |
| S1'' `Replace` / `Overwrite` の SetText（`Replace` は 1 回再試行） | `Replace` は置き換えずに進む（`replace_failed`）。`Overwrite` は 4.2 の救済の表（読み直して元のままなら `Retryable`、そうでなければ救済をやめる） | ― / 4.2 |
| S2 範囲を `commit_text` に縮める（Partial / CommitThenInput。1 回再試行） | 何もしない | `Aborted` |
| S2' 縮めた範囲を `GetText` で読み直し、`commit_text` と比べる（W4） | 読めない: 範囲を `original_text` に戻す SetText を試す。戻せたら `Aborted`。戻せなければ `AbortedChanged` で、範囲の今の文字列が分からないので `shown` を「不明」にする（確定値として更新しない。次の入口で表示を全体から作り直す）。違う: `original_text` に戻す SetText を試す。戻せたら `Aborted`、戻せなければ `AbortedChanged` で `shown` を読めた文字列にする | `Aborted` / `AbortedChanged` |
| S3 確定部分の末尾の範囲を控える（`post_commit_range`。今の `insert_after_commit`: on_compose.rs:351-367） | 元の文字列に戻す。戻せなければ `AbortedChanged` | `Aborted` / `AbortedChanged` |
| S4 EndComposition | 範囲を `original_text` に戻す（Partial は縮める前、`Replace` は置き換える前の文字列）。戻せなければ composition は縮めた・置き換えた文字列のまま。救済の `Overwrite` は元に戻さない（4.2 の救済の表の `WrittenNotEnded`） | `Aborted` / `AbortedChanged` / 4.2 |
| S5 新しい composition の開始（`post_commit_range`） | ― | `CommittedNoNew` |
| S6 新しい composition の範囲の取得 | composition の保持と結び付けは S5 の直後に済ませる | `CommittedNoNew` |
| S7 残り（または新しい文字の仮表示）の SetText | 同上 | `CommittedNoNew` |
| S8 属性・選択位置・キャレット矩形 | 無視（WARN） | 影響なし |

- Full は S1・S1'・S1''・S3・S4 だけを通る（S3 は直接挿入の位置に使う）。
- 値を 2 つに分ける（V4）: `original_text` は S1 で読んだ範囲全体の文字列で、照合と元に戻すことにだけ使う。`committed_text` は文書に確定した文字列で、Full は `original_text`（`Replace` / `Overwrite` の SetText が成功した場合は置き換え後）、Partial / CommitThenInput は S2 で縮めた確定部分（S2' で読み直して `commit_text` と同じことを確かめた値）。学習の判定と文脈（`Commit` / `CommitContext`）には `committed_text` だけを使う。例: 全体「漢字かな」で確定部分「漢字」なら、`original_text` =「漢字かな」、`committed_text` =「漢字」、学習・文脈は「漢字」だけ。
- `AbortedChanged`: 表示が古い印を立てる。入力セッション・journal・セッションはそのまま。
- `CommittedNoNew`: 残りを、`post_commit_range` から始める新しい composition に表示するよう 1 回だけ要求する（`update_composition` に開始位置を渡す形を足す。空の composition があればそれを使う）。※推測: TSF の範囲は文書の変更に追随するので、確定直後の位置として使える。範囲が使えなければ今の選択位置から始め、WARN を残す。それも失敗したら表示が古い印（次のキーの入口で作り直す）。
- 確定した部分は再送しない。

**後続のキーと入力世代**（R2）:
- `commit_pending` の間のキーは、その宛先の列に積み、`bound_txn = id` にする。
- 成功（`Committed` / `CommittedNoNew`）: T4 の後、新しい入力世代を作り、列の `bound_txn` を外して再生する。`lineage` は同じなので宛先の照合は通る。
- 失敗: 結び付いたキーは古い入力世代のまま列に残す。CommitThenInput の `KeyTask` は先頭。再試行は理由の表のとおり。
- 「文書が編集を受け付けない」: 入力セッションは残し、列の文字キーは 4.2 の上限まで保持し、上限で捨てて WARN（ホストは正常なので `direct_fallback` には入らない。`mode_indicator` に「文書へ書き込めません」）。
- フォーカスの移動（`focus_epoch` の変化）は失効として扱い、前提の決定 2 に従う（4.2）。
- 古い完了: 完了の処理は `id` がその入力セッションの今のトランザクションで、`lineage` が同じときだけ状態を変える。違えば WARN だけ。

**ホストの処理（T4）**:

| 結果 | Full | Partial / CommitThenInput |
|---|---|---|
| 成功 | 学習（`committed_text` が計画どおりのときだけ）→ `Commit(committed_text)` → 古い journal を閉じる | `RestoreJournal(new_owner, pre_effects=[学習（`committed_text` が計画の確定部分と同じときだけ）, CommitContext(committed_text)], journal=引き継ぎ)` を 1 回。古い journal を閉じる |
| 失敗 | 何も送らない | 何も送らない。新しい journal は作らない |

- 理由が `Unrecoverable` / `UnrecoverableAsDisplayed` のときはホストに何も送らない（断念として記録）。
- Full の副作用: 所有権があり末尾が空なら `Change(Learn)`・`Change(Commit)`。そうでなければ `RestoreJournal(自分, pre_effects, journal=[])`。
- `engine.commit` は読みと打鍵ログを消す（lib.rs:1075-1110）ので、Partial / CommitThenInput の文脈は、新しい composition の状態を作る要求の `pre_effects` として同じロックの中で先に適用する。

**新しい composition の journal（引き継ぎ）**:
- Partial: `old_journal + [ForcePreedit(全体の読み), RetainRange(確定した読みの文字数, 全体の文字数)]`。照合点は、古い journal の末尾が空なら `(len(old_journal), old.checkpoint_edit)`。
- CommitThenInput: 空から。新しい文字は `KeyTask` の続きとして、新しい入力世代で 1 回実行する。表示は S7 で仮表示（例 `a`）を書き、確認後に作り直す（例 `あ`）。
- 今のエンジンの取り直し（on_input.rs:214, 279, 465）はやめる。
- 末尾の未確定 `n` は Selecting に入る前の `flush_pending_n`（on_convert.rs:326）で閉じている（今と同じ）。RangeSelect の部分確定（on_convert.rs:1757-1793）、番号キーの部分確定（edit_ops.rs:356-370）も同じ計画を通す。

**部分確定の文書先行（5c-3、2026-10-22 改訂。上の S5〜S7 の仮表示と、Partial の journal の作り方を置き換える）**:

5b の試行（新しい composition に仮の表示を書き、ホストへ送った後で作り直す）は、作り直しの失効の端の経路で Codex のレビューが続けて「直してから」になり、取りやめた。代わりに、EditSession の中でホストへ送り、その応答の表示で新しい composition を始める（利用者の決定、2026-10-22。9 節）。

対象: `commit_then_start_composition` を使う 6 か所（HEAD 2822e88）。`on_input`（on_input.rs:204 BlockSelecting 中の文字、:272 Selecting 中の文字）、`on_input_raw`（on_input.rs:432 Selecting 中の文字）、`on_commit_raw`（on_convert.rs:1809 RangeSelect の部分確定、:1922 Selecting の Enter の残りあり）、`on_candidate_select`（edit_ops.rs:393 番号キーの残りあり）。今はどれも、文脈（`commit`）・`reset_preedit`・残りの引き継ぎ（`input_char` / `push_raw` / `seed_preedit`）を EditSession の前に別々の RPC で送っている。

EditSession の手順:

| 手順 | 失敗したとき | 結果 | ホスト |
|---|---|---|---|
| S0 `ReqCell` が `Pending` でない | 文書に触れない | 記録済みの `NotRun` | 送らない |
| S0' 今の文脈の DM と composition（`composition_key`）が要求の時点と同じか | 文書に触れない | `NotRun("stale")` | 送らない |
| E エンジンのロック（`engine_try_get`。H の後まで持つ） | 文書に触れない | `NotRun("engine busy")` | 送らない |
| S1 `GetRange`・S1' `original_text` を読む | composition を保持し直す | `Aborted` | 送らない |
| S2 確定部分に縮める SetText | 保持し直す（今は WARN だけで全体を確定している） | `Aborted` | 送らない |
| S2' 読み直して確定部分と比べる | `original_text` に戻してから保持し直す。戻せなければ `changed` | `Aborted{changed}` | 送らない |
| S3 確定部分の末尾の範囲を控える | 新しい composition は選択位置から（今と同じ） | ― | ― |
| S4 EndComposition | `original_text` に戻してから保持し直す（今は手放して `Failed`） | `Aborted{changed}` | 送らない |
| H 新しい編集状態を作り、`RestoreJournal` を 1 回送る | 下の H の結果の表 | `Committed{host}` | 下の表 |
| finish | `ReqCell` を `Done` にする。H の結果にかかわらず、S5 の前に必ず行う | ― | ― |
| S5〜S7 新しい composition の開始・範囲・表示 | `new_comp=false`（下の後処理） | 記録済み | ― |
| S8 属性・選択・キャレット | 無視 | ― | ― |

- composition が無い（直接の挿入）: 今と同じく直接の挿入を行い、成功したら H へ進む。失敗は `Failed`。
- `Aborted` / `NotRun` の後処理は 5c-1 と同じ（ホストへ何も送らず、確定を要求した時点の composition がまだ保持されているときだけ、キーの入口のセッションに戻す）。部分確定の印（`leaves_input`）は「キーを保留するか」と「ホストへ文書より先に送ったか」の 2 つに分け、6 か所では後者を立てない。
- 保持し直しに失敗したら `Failed`（5c-1 と同じ）。

H（ホストへの送信）:
- `RestoreJournal { owner: 新しい所有者, pre_effects: [Learn（学習するとき）, CommitContext(文脈の文字列)], journal: [InputChar(c, kind)] / [PushRaw(c)] / [SeedPreedit(残りの読み)] }`。ホストは 1 回のエンジンのロックの中で `pre_effects` → `reset_preedit` → journal の順に適用し、所有権を移す（server.rs:1205-1268）。トランザクションではない: journal の流し直しが失敗すると、ホストは `reset_preedit` して所有者を外し `Error` を返すが、`pre_effects` は取り消さない。
- 新しい編集状態は S4 の成功の後（`composition_take` の後）に作り、`created_under` と `tsf_comp` を 0 にして、その DM に登録する（`take_terminated` は `tsf_comp == 0` の通知も、`created_under == 0` の状態も外さないので、古い composition の終了通知で外されない）。まだ送っていない `pre_effects` を新しい欄 `pending_effects` に持ち、`restore_journal_via` はそれが残っている状態の流し直しでだけ送る。TSF 側の `CompositionRpc::restore_journal` に `pre_effects` の引数を足す。
- `Restored` の後、新しい所有者と世代を保存してから `preedit_display` を 1 回読み、その表示で S5〜S7 を行う。学習は H の中で済むので、BG の開始（`bg_start`、RangeSelect の `start_live_bg_if_ready` を含む）は EditSession の後の後処理にする（5b の「学習 → BG」の順を保つ）。

H の結果:

| 結果 | `pre_effects` | journal | 新しい編集状態 | 後処理 |
|---|---|---|---|---|
| `Restored` | 適用した | 適用した | 世代を保存、`pending_effects` を消す | 表示して `bg_start` |
| `Rejected(GenMismatch / OwnerMismatch)` | 適用していない | 適用していない | 残す（`pending_effects` も残す） | 下の「H が成功しなかったとき」 |
| `Rejected(ResultUnavailable)`・`Error`・送受信の失敗 | 分からない | 分からない（`Error` は適用していない） | 残す。`pending_effects` は消す（再送しない。利用者の決定） | 同上 |

H が成功しなかったとき（文書の確定部分は取り消さない）:
- ホストが動いている: セッションを Preedit（新しい読み）にする。新しい編集状態は journal と（残っていれば）`pending_effects` を持ったまま。次のキーの `apply_via` が `restore_journal_via` で流し直し、その表示で composition を始める。キーが来ないうちは、引き継ぎの文字は表示されない（利用者の決定）。
- ホストが回復できない（`link_unrecoverable`）: 新しい編集状態の journal の文字（`SeedPreedit` の読みを含む）を、第 9 版の救済（`insert_text_direct`）で文書へ入れる。

`Restored` の後の S5〜S7 の失敗（`new_comp=false`）: 結果は記録済み。EditSession の後の後処理（`finish_one`）で表示の要求を 1 回だけ出す（composition が無ければ S3 の範囲から始め、あれば今の composition に書く。表示を読めていなければ読み直す）。失効・再要求の仕組みは持たない。後のキーの表示より後に実行されて上書きしないことは、同じ文脈の EditSession が要求の順に実行されることに頼る（今のキーの表示と同じ前提。※TSF の仕様としては未確認）。保留したキーの再生は、今どおり後処理の後（key_queue）なので、表示の要求を出した後になる。

待ちの危険（利用者が受け入れた）: H の RPC は同期の `ReadFile` / `WriteFile` で期限が無い（8 節）。EditSession が UI のスレッドで実行される場合（今の EditSession の前提。TSF の仕様は未確認）、ホストが応答しない間、そのスレッドとアプリの文書の編集が待つ。回復できない切断（第 9 版）では RPC はすぐ失敗する。H の RPC はメッセージを処理しないので、待っている間にこのスレッドへ通知・タイマー・別の EditSession は入らない（※推測）。

CommitThenInput の確定の再試行（4.1 の `AfterCommit`）は 5c-2 で、この後に入れる。

### 4.7 副作用と、復元で戻るもの・戻らないもの

`Exact` が戻すもの: 読み・未確定ローマ字・ローマ字変換器の状態・打鍵ログ・`log_detached_at`。

| 対象 | 復元後 | TSF の処置 |
|---|---|---|
| BG 変換の所有者 | ホストが消す（server.rs:1002 と同じ） | ライブ・待機のタイマーを止め、preview の列を空にし、変換の世代を進め、モデル待ちを消す。Preedit / LiveConv なら `start_live_bg_if_ready` で開始し直す |
| LiveConv の preview | TSF 側の値 | Exact なら保つ。Rebased なら Preedit に戻す |
| Selecting の `llm_pending` | ― | false。候補はそのまま |
| 文脈 `committed` | `reset_preedit` は残す。ホストが入れ替われば空 | 戻さない |
| 学習履歴 | ホストのファイル | ― |

副作用の記録: 学習・文脈・BG の開始・候補の取り出しを `Pending` / `Applied` / `Unknown` / `Abandoned` / `Skipped` で記録する。`Unknown` は再送しない。文書の確定とは別に記録する。影響は「次の変換の文脈が短くなる」「その確定が学習されない」で、入力の文字は失わない。

**復元直後の処置**（入口の復元の後。Enter は通らない）:

| 結果 | セッション | 処置 |
|---|---|---|
| Exact | Preedit / LiveConv | BG の処置。表示を `final_edit`（と preview）で作り直す。今のキーは通常どおり |
| Exact | Selecting / BlockSelecting / RangeSelect | セッションと候補ウィンドウを保つ。`llm_pending=false`。今のキーは通常どおり（↓ は次の候補へ） |
| Exact | Waiting | Preedit に戻し、候補ウィンドウを隠す。今のキーは通常どおり |
| Rebased | どれでも | セッションが持つ全体の読みでセッションを Preedit にし、候補ウィンドウを隠し、表示を作り直す。今のキーは 4.1 の `AfterRepair` の規則（文字なら続ける、表示に依存する操作なら修復だけ） |

- **復元直後の Space（決定済み。前提の決定 3）**: Exact なら通常どおり変換する。Rebased なら修復だけ（4.1）。V6_Test_Procedure.md の T2-1b の「見る点」を「3 で変換され、候補ウィンドウが出る。4 の Space は次の候補へ移る」に改める（実装の 7 で、試験手順の文書も直す）。

### 4.8 F: F9 / F10 と打鍵ログの対応（R4）

**原則**: 変換中のエンジンの読み（範囲変換の選択部分・記号を除いた本体・ブロックの読み）は一時的な値で、打鍵ログは常に composition 全体の打鍵を保つ。ログを永久に切るのは、確定の引き継ぎの `RetainRange` と、ログに無い文字列を入れる `SeedPreedit` だけ。範囲選択の間に `RetainRange` は使わない。

**すべての呼び出し箇所の分類**（`eee267e`）:

| 箇所 | 今 | 第 3 版以降 | 理由 |
|---|---|---|---|
| edit_ops.rs:73, 148（F6〜F10） | `ForcePreedit` | そのまま | 表示だけを替え、ログを保つ |
| edit_ops.rs:568（BlockSelecting 中の記号） | `ForcePreedit(全体)` + `PushRaw` | そのまま | 全体に戻してから記号の Raw 項目を足すので、ログは正しい |
| edit_ops.rs:605（Selecting 中の記号） | `ForcePreedit(記号入りの読み)` | `SeedPreedit(記号入りの読み)` | 記号が途中に入ると打鍵ログと対応しない。F は種の綴り。末尾に足す場合にログを保つ方法は、未確定ローマ字の接尾辞（edit_ops.rs:590-592 のコメント）を確かめてから決める（実装時の課題） |
| edit_ops.rs:751, 893（RangeSelect に入る） | `ForcePreedit(全体)` | そのまま | 全体に戻す |
| on_convert.rs:303（Selecting から戻る） | `ForcePreedit(全体)` | そのまま | 同上 |
| on_convert.rs:395（範囲変換） | `ForcePreedit(選択部分)` | そのまま | 一時的な値。取消で全体に戻る（on_convert.rs:289-305）。ログは全体を保つ |
| on_convert.rs:892（記号を除いた本体の変換） | `ForcePreedit(本体)` | そのまま | 一時的な値 |
| on_convert.rs:952, 969（ブロックの読み） | `ForcePreedit` | そのまま | 一時的な値 |
| on_convert.rs:2022, 2107（BlockSelecting から戻る） | `ForcePreedit(全体)` | そのまま | 全体に戻す |
| on_input.rs:190（RangeSelect の取消） | `ForcePreedit(全体)` | そのまま | 同上 |
| 部分確定の残り（on_convert.rs:1782, 1883、edit_ops.rs:361、on_input.rs:470, 481） | `reset_preedit` + `push_raw` の列 | 4.6 の引き継ぎ（`ForcePreedit(全体)` + `RetainRange`） | 確定で永久に切る |
| 変換中の F6〜F10（edit_ops.rs:49-51, 110-111） | エンジンの今の読み（一時的な値のことがある）で変換 | 先に `ForcePreedit(全体の読み)` で全体に戻してから変換 | 一時的な値のまま F キーを使うと、選択部分だけが変換される |

**エンジンに足すもの（engine ABI 11 → 12）**:
- この設計書の「ABI 12」は「次の engine ABI の版」を指す。前提の決定 1 によりフェーズ 3（#75）の変更と同じ 1 回の版上げに含めるので、実際の番号は #75 と合わせて決める。
- `engine_seed_preedit(text)`: 読み・未確定・ログを消し、`text` を種の項目（`InputKind::Seeded`）で入れる。かなは `spellings_for` の最長一致で分け、`typed` に綴り、`output` にかな。逆引きの無い文字は `typed = output` の Raw 項目。綴りの選び方: 優先表（し=shi, ち=chi, つ=tsu, ふ=fu, じ=ji, しゃ=sha, しゅ=shu, しょ=sho, ちゃ=cha, ちゅ=chu, ちょ=cho, じゃ=ja, じゅ=ju, じょ=jo, を=wo）にあればそれ、無ければ最短、同じ長さなら辞書順の先頭。`っ` は次の綴りが子音で始まればその子音を重ね、それ以外は `xtu`。`ん` は次の綴りが母音・`y`・`n` で始まれば `nn`、それ以外は `n`。
- `engine_retain_range(start, end) -> bool`: 読みの `start..end` 文字を残し、未確定を消す。ログの出力の連結が読みと一致していれば、区間に入る項目を残し、境界にかかる項目は残る部分を種にして `true`。一致していなければ `seed_preedit(残す文字列)` と同じにして `false`。
- 既存の関数の意味は変えない。版は 2 か所で同時に上げる（rakukan-engine-abi/src/lib.rs:23、rakukan-engine/src/ffi.rs:20）。
- protocol v7 の `SeedPreedit` / `RetainRange` は、ABI 12 の前（7 節の 4〜6 の間）はホストが `reset_preedit` + 各文字の `push_raw` で代える。代替期間の制限: (a) 部分確定の残りは今と同じ結果（F10 で「かん」のまま）。(b) 照合点の不一致（Rebased）で作り直した部分は、打鍵ログが Raw のかなになり、F9 / F10 でかなのまま残る（今の `Restore` の後はログが空で頭が消えるので、消えない分だけ違う）。(c) 軟上限の縮約は段 7 まで有効にしないので、通常の入力のログは変わらない。この中間の状態は配布しない（前提の決定 1）。

**期待する文字列**（F9 は 小 → 大 → 先頭大 と回り、F10 は大文字・小文字の状態を保って幅を替える: V6_Test_Procedure.md T1-4）:

| # | 操作 | 期待 |
|---|---|---|
| F-1 | `kanji` → ホストを止める → `a`（Exact）→ F9 → F9 → F9 → F10 → F6 | 「かんじあ」→「ｋａｎｊｉａ」→「ＫＡＮＪＩＡ」→「Ｋａｎｊｉａ」→「Kanjia」→「かんじあ」（止めない場合と同じ） |
| F-2 | `kanjik` → ホストを止める → `a` → F10 | 「かんじか」→「kanjika」 |
| F-3 | `sikann`（「しかん」）→ Shift+← を 2 回（「し」を選択）→ Space → Esc（全体に戻る）→ F10 → F9 → F6 | 「sikann」→「ｓｉｋａｎｎ」→「しかん」。第 2 版の `RetainRange` だと「siかん」になる経路（R4 の例）。2026-10-22 に手順を `sikan` から改めた（下の注） |
| F-4 | `sikan` → Shift+← を 2 回 → Space →（「し」の候補を選んで）Enter → 残り「かん」で F10 → F6（ABI 12） | 「kan」→「かん」。ABI 12 の前は F10 で「かん」のまま（0.12.0 と同じ） |
| F-5 | `sikan` → Shift+← を 2 回 → Enter（「し」をひらがなで確定）→ F10（ABI 12） | 「kan」 |
| F-6 | `sikan` → Shift+← を 2 回 → Space → F10（範囲変換の候補表示中） | 「sikan」（全体に戻してから変換する） |
| F-7 | `kyouhaiitenki` → Space → 先頭の文節だけを選んで Enter。残りが「いいてんき」の場合 → F10 → F9 → F6（ABI 12） | 「iitenki」→「ｉｉｔｅｎｋｉ」→「いいてんき」（文節の切れ目はモデル次第） |
| F-8 | エンジンの単体: `seed_preedit("か")` → `s` → `a` | `romaji_log_str` = `"kasa"`、`hiragana_from_romaji_log` = `"かさ"`。TSF の F9 で「ｋａｓａ」、F6 で「かさ」 |
| F-9 | Rebased・縮めた journal: 打鍵 `sinbun` の「しんぶん」→ F10 | 「shinbun」（種の綴り。打鍵の `sinbun` には戻らない） |
| F-10 | エンジンの単体: 打鍵 `kyo` の「きょ」に `retain_range(1, 2)` | 読み「ょ」、戻り値 true、ログは種の項目 1 つ（綴りは実装時に `spellings_for("ょ")` で確かめる） |
| F-11 | `kanji` → F9 → `a` → F9 | 「ｋａｎｊｉ」→「ｋａｎｊｉあ」→「ｋａｎｊｉａ」（今と同じ） |

試験は流し直しの前後が同じことだけでなく、この表の文字列を比べる。

注（2026-10-22 の実機試験）: `sikan` と打つと末尾の `n` はかなにならず「しかｎ」で残り、未変換のローマ字が残っている間の Shift+← / Shift+→ は無視される（利用者の決定。範囲の選択はかなになった読みだけで始める）。F-3 は `sikann` と打つ。F-4〜F-6 の `sikan` も、範囲を選ぶには `sikann` と打つ必要がある。

### 4.9 拡張: #66 との関係と、接続の失敗の型

- 接続の失敗を型で返す: `ConnectError { stage: AttemptStage, kind: Transport | Suppressed | VersionMismatch{server} | HelloError | CreateError }`（今は `anyhow` の文字列: client.rs:555-560, 1363）。`HostLink` と `EngineAccess::NoEngine` はこれを使う。必要な部分（`kind` の分類と `HostLink` の更新、`probe()`）は 7 節の 3 で入れる。
- `Reason` に `ConfigStale` / `ConfigUnusable` を v7 で末尾に足す。意味は #66 で決め、v7 のホストは送らない。TSF は `_` を使わない `match` で、両方とも「結果不明」（通信失敗と同じ）として扱う。

## 5. RPC protocol v7 と engine ABI 12

| 追加 | 位置 | 内容 |
|---|---|---|
| `ChangeRequest::SeedPreedit { text }` / `RetainRange { start: u32, end: u32 }` | `ChangeRequest` の末尾。`ChangeKind` にも同名 | 4.8 |
| `SideEffect { Learn{reading, surface}, LearnForce{reading, surface}, CommitContext{text} }` | 新しい型 | 4.6 |
| `Request::RestoreJournal { owner, seq, pre_effects, journal, checkpoint: Option<Checkpoint{ len: u32, checkpoint_edit: EditState }>, config_version }` | `Request` の末尾 | 4.3 |
| `Response::RestoredJournal { engine_gen, replay: Exact \| Rebased, final_edit: EditState, bg: BgView, effects: Vec<EffectResult> }` | `Response` の末尾 | 4.3。途中結果は含めない |
| `Reason::ConfigStale` / `ConfigUnusable` | `Reason` の末尾 | 4.9 |

- `PROTOCOL_VERSION` を 7 にする（protocol.rs:22）。Hello で照合する（server.rs:697-700、client.rs:1361-1363）。v7 の TSF と v6 のホストは繋がらない（逆も同じ）。古いホストが動いている間は `HostLink = Disconnected{VersionMismatch}` で、4.2 の回復できない場合（直接入力）になる。フェーズ 3 のインストーラー（古いホストを止める）と同じリリースで出す（前提の決定 1）ので、利用者にサインアウトを別に案内する必要は無い。開発中に v7 を試すときは、古いホストを止めてから入れる。
- 台帳: `RestoreJournal` は未解決があっても送れ、`RestoredJournal` で台帳を空にする。`pre_effects` を含む要求の結果不明は、副作用を `Unknown` とし、再送しない（次の `RestoreJournal` には `pre_effects` を入れない）。
- engine ABI 12（4.8 の 2 関数。番号はフェーズ 3 の #75 と同じ 1 回の版上げで決める）は 7 節の 7 で入れる。写しを作らない操作の経路（4.3）は DLL 内部の変更で ABI を変えない。
- `Restore`（v6）は残す。

## 6. 試験の表

COM の部分を `DocOps`（EditSession の各手順の成否・`GetText` の結果を返す口）、`CompositionRpc`（FakeHost: 2 つの TSF ID と所有者を持ち、本物の `RakunEngine` を使う）、`EngineAccess` / `HostLink` の台本、時計、スレッドの ID の外に出す。各試験は、キーが抜けない・二重にならない・別の文書に届かないことに加え、表示・確定した文字列（`committed_text`）・確定の回数・確定の段階を確かめる。

判定の列（2026-10-22）: 自動の試験と実機の記録（V6_Test_Procedure.md の 2026-10-22 の節）に照らした状態。「自動で確認」「自動と実機で確認」は期待どおりであることを確かめた。「一部」は行の一部だけを確かめた。「未確認」は確かめていない（故障を起こす口 `DocOps` / FakeHost / 時計の外出しを作っていないものを含む）。「対象外」はフェーズ 2 の完了条件から外した（利用者の決定。28・31 は所有スレッドを作っていないため、#85 と書いた行は故障を起こす口（`DocOps` / FakeHost / 時計）が要るため。2026-10-22）。「単体の試験を足す」「実機で確かめる」は、フェーズ 2 の残りの作業。「設計と違う実装」は 7 節の実装時の変更で作り方を変えたもので、試験は今の実装に合わせて書き直す。

| # | 境界 | 起こす故障 | 期待 | 判定（2026-10-22） |
|---|---|---|---|---|
| 1 | 受け取り | 判定の表（IME × 割り当て × 修飾キー × keymap の取得失敗 × `has_input` × 列 × `direct_fallback`） | 7 節の 1 では今の動作と一致（Test と KeyDown の今の相違を含む）。7 節の 2 の後は両方が同じ | 自動で確認（入口の判定 `entry_gate` を OnTestKeyDown と OnKeyDown で共通にし、表の試験 `the_entry_gate_is_the_same_table_for_test_and_keydown` を足した。保留した文字がある間の直接入力で、Test は食べて KeyDown はアプリへ渡していた食い違いを直した） |
| 2 | K0→K1 | 起動中（`Starting` 5 キー → `Ready`）に `kanji` | アプリへ渡るキー 0。「かんじ」。確定 0 回 | 実機で確認（2026-10-22、EmEditor。言語バーの「エンジン再起動」の直後に打ったキーが `engine not ready … Input held` で保留され、エンジンの準備の後に打った順に再生された。アプリへ素通りしたキーは無い。保留は約 0.5 s） |
| 3 | K1 再生 | `Ready` 2 キー → `Busy` | 残り 3 キーが順に残り、`Ready` で「かんじ」 | 実機で確認（2026-10-22、EmEditor。言語バーの「エンジン再起動」の直後に打ったキーが `engine not ready … Input held` で保留され、エンジンの準備の後に打った順に再生された。アプリへ素通りしたキーは無い。保留は約 0.5 s） |
| 4 | K1 自動再開 | 起動中に `ka` で手を止める | 次のキーなしで「か」。HWND を作れない場合はキーがアプリへ渡る（今と同じ） | 実機で確認（2026-10-22、EmEditor。言語バーの「エンジン再起動」の直後に打ったキーが `engine not ready … Input held` で保留され、エンジンの準備の後に打った順に再生された。アプリへ素通りしたキーは無い。保留は約 0.5 s） |
| 5 | K1 解決 | 保留中に Space → `1` | Space の実行後に選択中なら候補番号、でなければ文字 | 修正した（利用者の決定で、届いた時点は文字で再生の時点に候補を選んでいる向きだけ。届いた時点で数字キーの規則（②）に従ったキー（Ctrl・Alt なし、キーマップに割り当てが無い 1–9 / テンキー 1–9。Shift は ② と同じく問わない: `Keymap::follows_digit_rule`）を、再生の時点で候補を選んでいれば候補番号にする `reresolve_digit_for_replay`。試験 `only_unmodified_unmapped_digits_follow_the_digit_rule`・`a_held_digit_is_resolved_again_for_the_state_at_replay`）。逆向きは 7 節の制限。実機は #85（保留が約 0.5 s で終わり、手で Space と `1` を保留の間に打てない。利用者の決定で単体の試験で確認とした）。自動で確認 |
| 6 | K1 モード | 保留中に `k` `a` → IME オフ → `x` | 「か」の確定（IME オフの既存の処理）の後、`x` を直接挿入。アプリへ渡したキー 0 | 対象外（保留は約 0.5 s で終わり、保留中の操作を手で作れない。#85。利用者の決定。照合の時点では 未確認） |
| 7 | K1 宛先 | 再生前に `focus_epoch` を進める（A → B → A）/ 元の文書を閉じる | 新しい宛先へは入らない。元の文書が生きていれば文字キーだけが元の文書に 1 回入る / 閉じていれば捨てて WARN | 対象外（保留は約 0.5 s で終わり、保留中の操作を手で作れない。#85。利用者の決定。照合の時点では 設計と違う実装（`focus_epoch`・宛先ごとの列を作っていない。7 節 6b-3 (d)。試験は今の実装に合わせて書き直す）） |
| 8 | 回復できない | 257 キー目 / 30 s / `VersionMismatch` / 稼働中の切断で末尾 30 s | 締め切りと同時に直接入力に入る。確定待ちがあれば重複させずに待つ。composition があれば `target`（journal の未確定の文字列 + 列の文字）で 1 回上書きして確定、無ければ 1 回挿入。Enter・Backspace・モードキーは入らない。締め切りの後のキーはアプリへ渡る | 一部。30 s の締め切りと稼働中の切断（ホストを起動できない状態）は実機で確認（2026-10-22、メモ帳。「かんじ」の composition を「かんじk」で 1 回上書きして確定し、直接入力に入った。UI のスレッドが止まっている間に押した `a` と Enter は、`a` を打ったとおりに入れ、Enter は捨てた。締め切りの 7 s 後の Enter はアプリへ渡った。7 節の 2026-10-22 の実機試験の注記 (d)）。257 キー目・`VersionMismatch` は対象外（故障を起こす口が要る。#85。利用者の決定、2026-10-22） |
| 9 | 挿入の失敗 | `InsertTxn` が 2 回失敗して 3 回目に成功 / 3 回とも失敗 | 文字は 1 回だけ入る / 捨てて WARN | 対象外（故障を起こす口が要る。#85。照合の時点では 未確認） |
| 10 | 回復 | 口はあるが `probe()` が失敗 → 成功 | 失敗の間は直接入力のまま。成功で外れる | 自動と実機で確認 |
| 11 | K2 応答消失 | `k` は確認、`a` の応答を失う → `n` | 仮表示「ka」→「かn」。`a` が抜けない | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 12 | K2 Enter | 11 の「ka」で Enter（ホスト不通） | 「ka」は打ったとおりに入り、Enter は救済で捨てる（2026-10-22 に期待を改めた。利用者の決定。直接入力の間に Enter をアプリへ渡すと別の動作になりうるため） | 実機で確認（2026-10-22、メモ帳。「かんじka」が入り、改行は入らなかった。救済で UI のスレッドが止まっている間に押した Enter は、救済の後に届いても捨てる。7 節の 2026-10-22 の実機試験の注記 (d)） |
| 13 | K2 交錯（同じプロセス・別プロセス） | 未解決・保存応答の再送・所有権の移動 | 各読みが打鍵どおり。二重にならない | 自動で確認（TSF 側の `apply_via`・`restore_journal_via` は実物、ホスト側は 1 つのホストの模型 `HostSim` で、2 つの composition が交互に打ち、応答を失った打鍵も含めて各読みが打鍵どおり: `two_compositions_typing_in_turn_keep_their_own_readings`）。別プロセス（2 つの TsfId）は #85 |
| 14 | 照合点の不一致 | 設定を変えた偽のエンジン | `Rebased`。確認済みの表示＋末尾。書き換えは `checkpoint_edit` で作り、末尾は 1 回だけ | 自動で確認 |
| 15 | R1 Rebased | Rebased の入口で `a` / ↓ / Space | `a` は修復後に 1 回入る。↓・Space は修復だけ | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 16 | 上限 | 軟上限 / 硬上限（末尾あり）/ 読み 2,000 文字 / 大きな `ForcePreedit` | 縮めても読みは同じ / `Suspended` / 表示どおり確定してから続ける（文字は抜けない） / 作らない | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 17 | 費用 | 長い読みでの流し直し | `cost` と時間がほぼ比例（測定） | 測った（`replay_time_grows_with_the_journal_length`、#[ignore]、release、2026-10-22）: 打鍵の流し直しは 500〜4,000 キーで 0.22〜0.50 µs/キー、4,000 キーで約 0.95 ms。種は 2,000 文字で約 0.62 ms。時間は費用（キー数）にほぼ比例し、上限の範囲で 1 ms 前後 |
| 18 | 表示の省略・失効 | 表示の要求を省略 / 非同期の表示の前に Enter | Enter は見えている文字（`original_text` = `committed_text`）を確定。失効した表示は確定の後に適用されない | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 19 | 部分確定 | S1〜S7 の失敗 | 2.3 の結果。確定は 0 か 1 回。`CommittedNoNew` は `post_commit_range` から 1 回だけ表示 | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 20 | CommitThenInput の失敗 | 変換中に `a`、確定が `Aborted` 2 回 → 成功 | `a` は捨てられず、成功後の新しい composition に 1 回だけ「あ」 | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 21 | 確定の非同期 | `TF_S_ASYNC` → 完了前に `a` | `a` はトランザクションに結び付き、成功後に新しい入力世代で「あ」 | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 22 | 古い完了・失効 | 完了の前に終了通知 / 失効（`Void`）の後に実行 | 新しい journal・セッション・列に触れない / 文書に触れない（試験 32） | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 23 | 文脈の切り替え | 古い文脈の確定が `TF_S_ASYNC` のまま新しい文脈で打つ | 新しい文脈のキーは完了（または時間切れ）まで列に残り、その後に新しい入力セッションで入る | 対象外（故障を起こす口が要る。#85。照合の時点では 未確認） |
| 24 | 副作用 | 文書は確定、`RestoreJournal` の応答を失う | 文脈・学習は `Unknown`。再送しない。`Commit` には `committed_text` | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 25 | Enter の `n` | `kan` → Enter / F10 の後の「kan」→ Enter / 仮表示「かn」→ Enter / 表示の省略で範囲が計画と違う状態の「かn」→ Enter | 「かん」/「kan」/「かん」/「かn」（置き換えない） | 対象外（故障を起こす口が要る。#85。照合の時点では 未確認） |
| 26 | T2-3 | Selecting / BlockSelecting / Waiting で入れ替わり（Exact・Rebased） | 4.7 の表。見えている候補と確定する文字が一致 | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 27 | F | 4.8 の F-1〜F-11 | 表の文字列 | 一部。F-4・F-6 は実機、F-8・F-9・F-10 と F-1・F-2・F-7（`a_replayed_typing_keeps_the_typed_spelling_for_the_f_keys`、`a_partial_commit_remainder_is_spelled_from_its_seed`。エンジンの打鍵ログまで）は自動で確認。F-1・F-2・F-3（`sikann` に改めた手順）・F-11 は実機で確認（2026-10-22、メモ帳。F-1・F-2 はホストを止めた後の F9・F10・F6 の表示） |
| 28 | スレッド | スレッド 2 で打つ（スレッド 1 が `busy` / `busy` でない） | 食べない（WARN）/ 所有者が移る | 対象外（所有スレッド `INPUT_OWNER` を作っていない。#86） |
| 29 | 1 スレッドの 2 文脈 | 文脈 1 の composition を保持したまま文脈 2 で打つ | 文脈 1 は表示どおりに 1 回確定し、完了後に文脈 2 の入力 | 対象外（故障を起こす口が要る。#85。照合の時点では 未確認） |
| 30 | 実機 | V6_Test_Procedure.md の T1・T2（T2-1b は 4.7 の新しい期待値）、再起動直後に打鍵する補助の試験。フェーズ 3 と合わせた配布物での統合試験 | 計画書の完了条件 | 一部（統合試験はフェーズ 3）。T1 相当・T2-1・T2-1b・T2-2・T2-3 は実機で合格（T2-1 は 2026-10-22 の開発ビルド `3a2bc78` + 未コミットの修正）。T2-4 も実機で合格（2026-10-22、メモ帳。再起動の直後に打ったキーは保留され、アプリへ渡らなかった）。ロック競合を起こす補助の試験は対象外（#85。利用者の決定、2026-10-22）。統合試験はフェーズ 3 |
| 31 | スレッド（N1 付記） | 所有者でないスレッドの Deactivate | 所有スレッドの composition・セッション・`COMPOSITIONS` が残る | 対象外（所有スレッド `INPUT_OWNER` を作っていない。#86） |
| 32 | 時間切れ（N1） | 確定の要求が T1 のまま 2 s → `Void` → 再試行の要求 → 古い要求が後から実行 | 古い要求は文書に触れない。確定は再試行の 1 回だけ。`InsertTxn` も同じ | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 33 | 宛先ごとの列（N2） | A の確定が `TF_S_ASYNC` のまま B へ移って `a` → A へ戻って `b` | 列は `[A@1, B@2, A@3]`。B の `a` は B へ 1 回（決定 2）、A の `b` は A で通常どおり | 対象外（保留は約 0.5 s で終わり、保留中の操作を手で作れない。#85。利用者の決定。照合の時点では 設計と違う実装（宛先ごとの列・`focus_epoch` を作っていない。7 節 6b-3 (d)。試験は今の実装に合わせて書き直す）） |
| 34 | Full の置き換え（N3・V3） | `kan` → Enter で置き換えの SetText が失敗 / CycleKana で範囲が計画と違う / 置き換えの SetText は成功し EndComposition が失敗、元に戻す SetText が成功 / 同じで元に戻す SetText も失敗 | 「かn」を確定（`replace_failed`、学習しない）/ 確定しない / `Aborted`・文書は「かn」の composition / `AbortedChanged`・文書は「かん」の composition、`shown` =「かん」、表示が古い印 | 対象外（故障を起こす口が要る。#85。照合の時点では 一部（`Full(AsDisplayed)`・`replace_failed`・`shown` は作っていない。7 節 5a (d)・5c-1 (e)）） |
| 35 | 自動の確定（N4） | 読み 1,999 文字で表示の要求が未完了のまま 2,000 文字目 | 表示の完了を待ってから確定。受理した文字は失われず、2,000 文字目は新しい入力に入る | 自動で確認（今の実装に合わせた期待: 上限で確定した後、確定の結果待ちのときだけキーを止めて列に戻し、確定できなかった・実行されなかったときはキーをそのまま実行する。`after_limit_commit`、試験 `the_key_after_the_limit_commit_waits_only_for_a_pending_commit`） |
| 36 | 回復できない場合の確定の失敗（N5・V2・W5・Y2） | `RescueTxn` が 4 回とも `Retryable`（失効、または SetText の失敗で範囲が元のまま）/ 表示「a」、対象「abc」で 4 回失敗の後、最後の EndComposition が成功 | 最後の 1 回の表示どおりの確定 → 直接入力のまま。救済の要求は 4 回 + 最後の 1 回 / 「a」が確定し、結果は `PartlyKept{ unrescued: "bc" }`。「bc」は挿入せず記録と通知。救済全体の成功として扱わない | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 37 | 回復の通知（付記・W2） | `probe()` 成功の通知の後、UI が受ける前に切断 / 救済の途中で成功の通知 | 直接入力を外さない / 救済の途中は外さず、救済の終わりに最新の `HostLink` を読み直して外す。救済の途中に受けたキーはアプリへ渡っており、列には入らない | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 38 | 回復できない場合の入口（V1・W1・Y4） | 最初の文字を journal に入れた後、表示の要求が未実行のまま回復できない状態になる（composition 無し / 空の composition）/ 失効させた古い表示の要求を `RescueTxn` の前に実行 / 後に実行 / composition 無しで救済の要求が遅れる・失敗する → その間に利用者がカーソルを動かす（キー / マウス） → 再試行 | 受理した文字が挿入で 1 回入る / `Overwrite` で 1 回確定する / 古い要求は composition を作らず、`RescueTxn` が挿入で 1 回入れる / 古い要求は何もしない。どの順でも二重にならない / composition が無いので挿入せず、`Stopped` として記録と通知（キーで動かした場合もマウスで動かした場合も同じ）。今の選択位置には入れない | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 39 | 締め切り（V2・W3） | 救済の途中で `xyz` と打つ / composition の中の文字をアプリが編集・削除する / アプリが composition を終わらせる | `xyz` はアプリへ渡り、対象は増えない（並び順は実機で確かめる）/ 範囲が `shown_at_cutoff` と違うので上書きせずに確定し、表示に出ていなかった残りは挿入せず記録と通知（`PartlyKept`）/ 確定はアプリに任せ、残りは挿入せず記録と通知 | 対象外（故障を起こす口が要る。#85。照合の時点では 一部） |
| 40 | 部分確定の実績値（V4） | 全体「漢字かな」で「漢字」を部分確定 | `committed_text` =「漢字」。学習と `CommitContext` は「漢字」だけ。残り「かな」は新しい composition | 自動と実機で確認 |
| 41 | 救済の書き込みの後の失敗（X1・Y1） | `shown_at_cutoff` =「a」、`target` =「abc」。SetText「abc」は成功、EndComposition が失敗 | 元に戻さない（4.6 の共通の元に戻す処理を通らないことを確かめる）。再試行もしない。結果は `WrittenNotEnded`、文書の composition は「abc」が 1 回。「abcbc」にも「a」にもならない | 対象外（故障を起こす口が要る。#85。照合の時点では 未確認） |
| 42 | 不確かな書き込み（第 8 版の原則） | SetText「abc」が失敗し、読み直した範囲が「a」と違う / 読めない | `Stopped{ unrescued: "abc" }`。再試行しない。composition には触れない。記録と通知 | 対象外（故障を起こす口が要る。#85。照合の時点では 未確認） |
| 43 | 終了の後の位置（X3・Y3） | アプリが composition を終わらせる → 利用者がカーソルを別の場所へ動かす → 救済が実行される | 書かない。表示に出ていなかった残りを記録と通知（`PartlyKept`）。カーソルの位置にも保存した範囲にも入れない（保存した範囲による挿入は第 8 版で削除。Clone・Collapse・SetGravity・GetText の失敗を扱う必要も無くなった） | 対象外（故障を起こす口が要る。#85。照合の時点では 未確認） |

## 7. B-3 実装の順序と規模

規模は「追加・変更した行（試験を含む。移動だけの行は数えない）」で見積もる。各段で「追加」（コードと試験を入れるが、今の経路からは使わない、または判定してログに残すだけ）と「有効化」（今の経路の動作を変える）を分ける。

| # | コミット | 追加 | 有効化（動作の変化） | 規模の目安 |
|---|---|---|---|---|
| 1 | 抽出（A 節） | `KeyStateSource`・`resolve_action_with`・`test_resolve`・`test_key_eat`・`keydown_ime_off_ignores`・`keydown_return` と試験 | なし。今の経路が実際に取り出した関数を通る。評価順・副作用・Test と KeyDown の相違はそのまま | 250〜450 行 |
| 2 | fallback を揃える | ― | KeyDown の keymap 取得失敗時に `essential_fallback_action` を使う（試験 12 の期待値を変える） | 50〜150 行 |
| 3 | 識別・記録・接続の型・所有スレッド | `focus_epoch`、`lineage` / `input_gen`、表示要求 ID と `display_epoch`、`ReqCell`、確定トランザクションの記録、`ConnectError` の必要部分・`HostLink`・`probe()`、`INPUT_OWNER` | `DmRef` の鍵。確定の失敗時に composition を戻す。失効した表示を適用しない。所有者でないスレッドの Deactivate・終了通知・タイマーがプロセス共通の状態に触れない | 1,000〜1,700 行 |
| 4 | journal と protocol v7 | `RestoreJournal`、照合点、FakeHost、`SeedPreedit` / `RetainRange` の代替（`push_raw`）、`apply_edit`、エンジン内部の写しを作らない経路、`cost` の計算と上限の判定（DEBUG に残すだけ） | `pending` を journal に置き換え、復元で末尾が落ちなくなる。代替の `SeedPreedit` を使うのは Rebased のときだけ（4.8 の代替期間の制限） | 1,300〜2,200 行 |
| 5 | 確定トランザクションと列の核（A・B・C） | `CommitReason`、`Full(AsDisplayed / Replace)`、`KeyTask`、`accept` / `execute`、宛先ごとの列、再開のタイマー、副作用の記録 | 文書を先にする順序、`GetText` の実績値、手順ごとの結果、時間切れ・再試行、確定待ちのキーの保留と引き継ぎ、部分確定の引き継ぎ、4.4 の切り替え。読みの長さの上限と 1 件の大きさの上限 | 1,400〜2,400 行 |
| 6 | 保留キーと回復できない場合 | `InsertTxn`、`RescueTxn` と `RescueState`、`direct_fallback`、回復の確認 | エンジンの未準備・ロック競合での保留、列の上限、硬上限（`JournalFull`）、回復できない場合の手順。最後に「受理したキーをアプリへ戻さない」（`KeyOutcome`） | 800〜1,400 行 |
| 7 | F と T2-3 | engine ABI の 2 関数（4.8） | `SeedPreedit` / `RetainRange` を engine の関数に切り替え、軟上限の縮約、edit_ops.rs:605 の置き換え、変換中の F キー、復元後の処置の表、復元直後の Space、V6_Test_Procedure.md の T2-1b の改訂 | 600〜1,000 行 |
| 8 | 拡張 | `Reason` の追加と処置、`ConnectError` の残り | 分類だけ | 150〜300 行 |

- 実装時の変更（2026-10-21、段 1〜3 の実装と Codex のレビューを受けて）: 段 3 では、`RemoteComposition` の鍵を `DmRef` にし、Deactivate で失効させた DM の編集状態を外すようにした。所有スレッド（`INPUT_OWNER`、4.4）は、所有の移譲と処理への入場の競合・遅れたフォーカス通知・スレッドの終了の判定にレビューで問題が残り、そもそも 1 つのプロセスで複数のスレッドが TSF を使うことが確かめられていない（1 節の ※推測）ので、入れなかった。代わりに、2 つ目のスレッドが Activate したら WARN を残す記録を入れた。このログで複数スレッドが確かめられたら、4.4 を設計し直す。「確定の失敗時に composition を戻す」と「失効した表示を適用しない」（`display_epoch`）は、エンジン・セッションとの整合と最新の表示の再要求が要るので、確定トランザクションと一緒に段 5 で入れる。「追加」の列（`focus_epoch`・`lineage` / `input_gen`・`ReqCell`・確定の記録・`ConnectError` / `HostLink` / `probe()`）も、使う段（5・6）で入れる。
- 実装時の変更（2026-10-21、段 4）: (a) 所有権を取り直すときは、今のキーより前の journal を `RestoreJournal` で流し直し、今のキーは `Change` で送る（呼び出し側がキーごとの結果を必要とするため。4.3 の「今のキーも journal に入れて `RestoreJournal` で送る」から変えた）。今のキーは送る前に journal の末尾に入れるので、`Change` が失敗しても確かめていない末尾として残り、次のキーで流し直す。(b) 台帳の不変条件（4.3）のうち、副作用（BG・確定・学習）の結果不明の記録は段 5 で入れる。それまでは commit / learn / reset が今までどおり台帳全体を断念する。(c) journal に入らない `Commit` / `CommitAsHiragana` はエンジンの読みを消すので、照合点を `Change` の応答で進める前提は「確定の直後に `reset_preedit` で composition を閉じる」呼び出し順に依存する（段 5 の確定トランザクションで、この順を確定の記録に置き換える）。(d) ホストは、journal より長い照合点を受けたら journal を捨てて照合点の値を種にする（`Rebased`）。今の TSF は送る範囲の中の照合点だけを送る。(e) 上限は判定して DEBUG に残すだけ。`cost` は文字の要求を 1、文字列の要求をそのバイト数で数える近似で、エンジンが触る文字列の長さは足していない。(f) 実エンジン付きの FakeHost（6 節）は未作成。TSF 側は台本の偽の RPC、ホスト側は簡単な偽のエンジンで試験している。
- 実装時の変更（2026-10-21、段 5 の最初の部分 5a）: 全体の確定（Enter の各状態・CycleKana・番号キーの確定（残りなし）・IME 切り替え前の確定・全角スペース・ひらがな確定）を `commit_full` に通した。部分確定（`commit_then_start_composition`）と空の文字列で閉じる経路は 5b 以降。(a) EditSession が同期で実行された場合だけ、文書を先に確定し、結果を見てからホストへの後処理（BG の回収・学習・文脈・`reset_preedit`）を送る。後で実行される（`TF_S_ASYNC`）場合は、確定待ちのキーの保留（5c）が無い間は、後処理をその場で先に送る（今までと同じ順）。遅れた `reset_preedit` が確定後に打ったキーの編集状態を閉じるのを避けるため。この場合、文書の確定に失敗しても取り消せず、WARN を残すだけ。(b) 確定に失敗したときは、文書の扱いを今までと同じにする。EditSession の中の失敗（GetRange・EndComposition・直接の挿入）では composition を手放し、保持し直さない。要求が実行されなかった場合は、composition の保持の状態を変えない。どちらの場合も、ホストには BG の回収と `reset_preedit` だけを送り、学習と文脈は送らない。置き換えにも元の文字列の読み取りにも失敗して確定した文字列が分からないときも、学習と文脈を送らない。composition を保持し直してホストの読みを残す扱い（4.6 の失敗時の復元）は、呼び出し側が先に閉じたセッションの状態（Selecting の候補・選択、句読点の保留）を戻せないと、次の Enter が候補ではなく読みを確定してしまう。後処理を先に送った場合はホストの読みが無いので、次の入力が保持し直した composition の文字列を置き換えてしまう（Codex のレビュー）。このため、セッションの状態の写しと表示の再要求（`display_epoch`）と一緒に 5c で入れる。(c) 後処理は要求の順に行い、結果の無い要求の後ろは待たせる（後処理を先に送った要求は待たせない）。(d) `Full(AsDisplayed)` は、Selecting の句読点の保留と末尾の `n` が今の SetText に依存しているので入れていない。`ReqCell` の時間切れ・再試行も 5c。
- 実装時の変更（2026-10-21、段 5 の 5b。利用者の決定）: 部分確定（RangeSelect の部分確定・Selecting の Enter と番号キーの残りあり）と確定してから入力するキー（BlockSelecting・Selecting 中の文字、`on_input_raw` の Selecting）では、学習だけを文書の結果の後に回した。確定部分に縮めた後の範囲を `GetText` で読み直し（4.6 の S2'）、それが計画の確定部分と同じときだけ学習する。文脈（`commit`）・`reset_preedit`・残りの読みの引き継ぎ（`push_raw` / `input_char`）は今までどおり先に送り、1 回の EditSession で新しい composition にホストの表示を書く（`commit_then_start_composition`）。EditSession が後で実行される場合は、学習もその場で先に送る（5a と同じ）。新しい読みの BG 変換の開始は、学習の後に移した（BG の要求が結果不明で台帳に残ると、後の学習の要求が断られるため。後処理の一つ `bg_start` として、学習の後に送る順を `apply_plan` に固定した。確定に失敗しても今までどおり始める）。経緯: 後処理と引き継ぎを全部文書の後に回す作り方（新しい composition に仮の表示を書き、確定の後でホストの表示に作り直す）は、作り直しの失効（キー・ライブ変換のタイマー・終了通知・フォーカスの移動・文書の破棄）と、作り直す前に編集状態を作ってよいかの判定に端の経路が続き、Codex のレビューで 7 回続けて中の指摘が出た。部分確定の全体を文書先行にするのは、確定待ちのキーの保留と `RestoreJournal` の `pre_effects`（`CommitContext` を引き継ぎと同じロックで適用）が入る 5c で行う。その作り方の差分は手元に残してある（リポジトリには入れていない）。
- 実装時の変更（2026-10-21、段 5 の 5c の最初の部分）: 確定待ちのキーの保留を入れた（`tsf/key_queue.rs`）。(a) 確定の EditSession が後で実行され、結果を待っている間（`commit_txn::commit_pending`）に届いたキーは、IME がオンで、確定の後の状態で IME が食べるキー（`key_should_eat` に「確定の後に入力が残るか」を渡す。残るのは、保留したキーがあるか、結果待ちの最後の確定が部分確定のとき）なら、OnTestKeyDown で食べ、OnKeyDown で列に積む。食べないキー（全体の確定の後の Home など）は保留せず、その場でアプリへ渡す（4.1 の「列・確定待ちの間はアプリ用のショートカット以外を食べる」とは違う。後で実行してアプリへ渡せないキーを作らないため）。今のキーも列に積んでから再生し、今のキーまで実行できたら、その結果（IME がオフだった・IME が使わなかった）でアプリへ渡すかを決める。キーは OnKeyDown の最初（他の処理より前）で届いた順の通し番号を取り、受け付け（`admit`）を終えるまで、後から届いたキーを保留させ、再生を止める（`key_queue::enter`）。列ではその番号の順に並べる。受け付けの判定（失効・後処理・保留の判定・積む）は再生を止めた中で行う（`admit`）。キーの処理中・再生中に割り込んだキーは、処理中のキーが入力を残すものとして判定する。宛先は届いた文書（DM）だけで、`KeyEvent` の固定は行わず、`UserAction` は届いた時点で解決したものを使う（4.1 の「実行時に解決」とは違う）。(b) 結果の後処理を送った後、届いた順に 1 つずつ `handle_action` で実行する。契機は `WM_APP_COMMIT_DONE`・保留か結果待ちの間だけ動く 50 ms のタイマー・次のキーの入口（4.2 の再生の契機）。実行したキーが新たに確定を待たせたら、そこで止める。キーの処理の間は再生を止める（`run_exclusive`）。(c) これにより、5a・5b の「後で実行される場合は後処理を先に送る」扱いをやめ、後処理は常に文書の結果の後に送る。(d) 確定の要求が 2 s たっても実行されなければ失効させ（`expire_overdue`）、実行されなかったもの（`NotRun(timeout)`）として後処理する。失効した要求の EditSession が後で実行されても、文書には触れない。部分確定の「composition の置き換え待ち」の印（#71）も、失効のときに消す。(e) 上限: 256 キー、または最初のキーから 30 s。超えたキーは捨てて WARN。(f) この段の制限: 実行の時点で届いた文書にフォーカスが無いキーは捨てて WARN。前に保留したキーが、実行の時点で IME がオフだった・IME が使わなかった場合は、アプリへ渡せずに失われる（INFO を残す。前提の決定 2 の「打った文字を元の文書へ入れる」は段 6 の `InsertTxn`）。Deactivate では保留したキーを捨てる（4.2 のとおり）。確定に失敗したときのセッションの状態の復元、表示の世代、`CommitThenInput` の再試行、部分確定全体の文書先行は、5c の後の部分。BlockSelecting 中の文字で確定が後で実行された場合、新しい読みの BG 変換は結果の後で始まるが、ライブ変換のタイマー（`live_input_notify`）は始めない（次のキーで始まる）。
- 利用者の決定（2026-10-21）: 5c の残り（確定に失敗したときのセッションの状態と composition の復元、表示の世代 `display_epoch`、`CommitThenInput` の再試行、部分確定全体の文書先行）は、フェーズ 2 の後に回す。理由: 前の 3 つは EndComposition などが失敗する稀な経路の改善で、今は段 5 より前と同じ扱い。表示の世代は、今の composition の照合（`update_composition` の T-MID2）とライブ変換の世代（`conv_gen`）で大部分を防げている。部分確定は学習がすでに文書の後で、残るのは文脈の順だけ。
- 実装時の変更（2026-10-21、段 5 の 5d）: 読みの長さの上限（2,000 文字）を入れた（dispatch.rs の `handle_action` の入口）。上限に達した読みに文字を足すキー（`Input` / `InputRaw` / `FullWidthSpace` / `Punctuate`。候補の選択中は除く。選択中の文字は先に確定するため）は、先に今の composition を Enter と同じ扱い（`CommitRaw`。見えている文字を確定し、ライブ変換なら学習も Enter と同じ）で確定してから実行する。4.6 の「自動の確定」（表示の要求の完了を待ち、範囲の文字列が計画どおりのときだけ確定する。学習しない）とは違う。確定が後で実行される場合は、そのキーを止めて（`key_queue::suspend_current`）列の先頭に戻し、結果の後で実行する。判定はキーの文書へ結び付けた後で、その文書の読みの文字数（`local_reading_len`）に対して行う。確定に失敗した（エンジンを取れないなど）ときは、そのキーを今までどおり実行する（上限を少し超え、次の文字キーでもう一度確定を試す）。確定の後でエンジンを取り直せなければ、そのキーを止めて列に戻し、タイマーで再試行する（再生はそこで止める）。保留したキーの再生で `handle_action` の入口のエンジンを取れない場合も、捨てずに止めて列に戻す（`key_queue::in_replay`。直接のキーは今までどおり）。止まり続けて保持の上限（最初に保留してから 30 s）を過ぎたキーは、捨てて WARN を残し、後のキーへ進む。1 件の大きさの上限（`ForcePreedit` / `SeedPreedit` の文字列）は、読みの長さの上限が先に効くので判定を足していない。硬上限は段 6、軟上限（縮約）は段 7。
- 実装時の変更（2026-10-21、段 6 の最初の部分 6a。#70）: エンジンの準備中・ロックの競合のときのキーの保留を入れた。(a) `handle_action` の入口でエンジンを取れない（`engine_try_get_or_create` が Err）とき、IME のモードキーは、composition が無ければエンジン無しで切り替える（`switch_ime(None, ..)`。composition があれば保留する）。それ以外の直接届いたキーは、今の入力の有無（composition か候補の選択中。composition を確かめられなければ、あるものとする。OnTestKeyDown もエンジンを取れないときは同じ判定）で IME が食べるキーだけをアプリへ渡さず止めて（`suspend_current`）保留の列に入れ（読込中の表示を出す）、食べないキーは今までどおりアプリへ渡す（OnTestKeyDown と同じ判定）。再生中のキーは止めて列に戻す。エンジンが使えるようになったら、再生のタイマーで届いた順に実行する。4.2 の `EngineAccess`（`Starting` / `Busy` / `NoEngine`）は分けていない。(b) 回復できない場合: 保留したキーが最初の保留から 30 s を過ぎても、エンジンが使えない（無い・ロックが取れない）ままなら、列の文字キーを打った文字のまま、そのキーが届いた文書の選択位置へ入れる（`insert_text_direct`。同じ文書へ続く文字は 1 回で）。位置を確かめられないとき（composition が残っている・確かめられない、選択位置が取れない・長さ 0 に縮められない）と、届いた文書が破棄されていたときは入れずに記録と通知（「未入力」を表示。composition がある場合の救済は段 6 の後の部分）。文字を入れ終わるまで（処置の途中）に届いたキーは、IME のオン・オフにかかわらずアプリへ渡さず（入れていない文字を追い越さないため）、文字キーは打った文字として、届いた順の通し番号の順に、今入れている文字の後に同じように入れる。文字以外（割り当ての無いキーを含む）は捨てる（モードキーは通常どおり）。文字を入れる EditSession は、実行の時点で失効の世代と、要求の時点の文書（ポインタと世代）が生きていることを確かめ、違えば文書に触れずに失効する。選択位置はキーで動かないが、マウスで動いた場合は動いた位置に入る（確かめる手段が無い。4.2 の「確かめられない位置に入れない」の例外として残す）。Deactivate では文字を入れる要求を失効させ（失効の世代を進め、取り出し中・要求中の要求にも届かせる）、入れられなかったものとして記録する。要求の結果は状態の箱に記録し、再生の契機ごとに確かめる（`check_rescues`）。文書を変えていない失敗（要求の拒否・SetText の失敗・2 s の失効）は 3 回まで要求し直す。文字以外のキーは捨てて WARN。その後は直接入力（IME のモードキー以外はアプリへ渡す）にし、文字を入れ終わり、キーの入口でエンジンを取れたら外す。4.2 の `HostLink`・背景の接続確認（`probe()`）・`RescueState`、composition がある場合の救済（`RescueTxn`）、実行の時点で IME がオフだった文字キーの `InsertTxn` は、段 6 の後の部分。(c) 届かなくなった `PENDING_KEYS`（state.rs）と on_input.rs のエンジンが無い場合の分岐を削除した。
- 実装時の変更（2026-10-21、段 6 の 6b-1。#70）: 接続の状態と、直接入力を外す条件を入れた。(a) `HostLink`（rakukan-engine-rpc の `host_link()`。プロセスで 1 つ）: 接続の試行（`ensure_connected`）と RPC（`call_with_retry`）の成否で更新し、通信の失敗・ホストの終了で接続を捨てた時点（`drop_stream`）でも切断として記録する（再接続を待つ間に `Connected` のまま残さない）。状態は `Unknown` / `Connected` / `Disconnected(VersionMismatch | Other)`。世代は**状態が変わったときだけ**進める（4.2 の「更新のたびに進める」とは違う。繋がったままの RPC の成功で世代が進むと、確認の通知の世代と照合できなくなるため。切断を挟めば必ず進む）。`Hello` の版の違いは型の付いたエラー（`ProtocolMismatch`）で返し、`VersionMismatch` に分類する。4.9 の `ConnectError` の他の分類と `since` は入れていない。(b) `RpcEngine::probe()`: `EngineHealth` を送り、通信の失敗を隠さずに返す（健全性の値は判断に使わず、ログに残すだけ）。(c) 確認のスレッド（`engine/host_probe.rs`）: 直接入力にしたスレッドの候補ウィンドウを登録し、5 s ごとに `RAKUKAN_ENGINE` を `try_lock` で取る（取れなければ飛ばす）。口が無ければ `engine_start_bg_init` で初期化を促し、あれば `probe()` を送る。成功したら、その時点の `HostLink` の世代と登録の番号を付けて、登録した窓へ `WM_APP_LINK_UP` を送る。送信は登録と同じロックの中で行い、登録を外した後はその窓へ送らない。受け取る側は登録の番号を照合し、今の登録でない通知を捨てる。登録が無くなったらスレッドを終える。`probe()` の間はエンジンのロックを持つので、ホストが応えないと再接続の待ち（最大で約 10 s）の間、UI のスレッドの `engine_get`（Activate など）が待つ（初期化のスレッドやキーの RPC の再接続と同じ）。(d) UI のスレッド（`key_queue::on_link_up`）: 処置の途中（`RESCUE_ACTIVE`。4.2 の `RescueState` の代わり）なら外さず印だけ残す。それ以外は `HostLink` を読み直し、`Connected` で世代が通知と同じときだけ直接入力を外す。処置が終わった時点で印があれば、そのときの `HostLink` が `Connected` なら外す。(e) Deactivate では窓を壊すので登録を外し、直接入力のままなら次のキーの入口で登録し直す。窓・スレッドを作れないときは、今までどおりキーの入口でエンジンを取れたら外す。(f) 保留したキーの再生で、`HostLink` が `VersionMismatch` でエンジンを取れなければ、30 s を待たずに回復できない場合の処置へ進む。言語バーのツールチップの文言（4.2 の「エンジン停止中（直接入力）」）は変えていない。
- 実装時の変更（2026-10-21、段 6 の 6b-2。#70）: composition がある場合の救済（4.2 の手順 1〜6 の `RescueTxn`）を入れた。利用者の決定: 表示の失効は救済用の最小限だけ入れる（5c の後の部分の `display_epoch` 全体は入れない）。救済で書く文字列は「締め切りの時点の表示 ＋ 確かめていない末尾の文字 ＋ 列の文字キー」。(a) 締め切り（`give_up` の中で同期に行う）: 直接入力と処置の途中の印を先に立て、composition があれば、表示の要求の世代（`on_compose` の `DISPLAY_EPOCH`）とライブ変換の世代を進め、その時点の表示 `shown`（composition に最後に SetText が成功した文字列。composition ごとに記録し、一度も成功していなければ空）を控える。`target` = `shown` ＋ その composition の編集状態の journal のうち確かめていない末尾の文字（打った文字のまま。Backspace は末尾の文字を消し、それ以外の要求は数えて WARN に残す）＋ composition の文書へ届いた列の文字キーの文字（列から取り除く）。他の文書の文字は 6a のとおり選択位置へ入れる。(b) 表示の要求（`update_composition`・`update_composition_parts_impl`）は、処置の途中なら要求しない。要求の時点の世代を持ち、実行の時点で世代が進んでいるか処置の途中なら、composition を作らず SetText もしない。ライブ変換（Phase1A）と待機のタイマーの SetText は、締め切りで進めるライブ変換の世代で失効し、実行の時点で処置の途中なら書かない。`shown` は composition に SetText する全経路（この 4 つと、部分確定の後の新しい composition）で記録する。最新の表示の再要求はしない。(c) `rescue_composition`（EditSession）: 4.2 の手順 4 の表のとおり 1 回だけ判断する。composition が締め切りの時点のもの（`composition_identity` で照合）でなければアプリが確定したものとして書かない。範囲を読み、`shown` と違えば EndComposition だけ、同じなら `target` に SetText して EndComposition。SetText に失敗したら読み直し、`shown` のままなら `Retryable`。結果は `RescueOutcome` として箱に記録する。(d) `Retryable`（要求の拒否・2 s の失効を含む）は 4 回まで要求し直し、その後に最後の 1 回（範囲が `shown` のままなら EndComposition だけ）を要求する。手順 3（確定の T1 を待つ）は、再生が確定の結果待ちの間は止まっているので、締め切りの時点で T1 は無い。(e) 救済で composition を終えたら、候補・ライブ変換のタイマー・セッションと、その composition の編集状態を片づける（ホストへは送らない）。`WrittenNotEnded` は「未確定」を出す。救済できなかった文字は 6a と同じく WARN と「未入力」。処置の途中に届いた文字は、composition の救済が終わってから選択位置へ入れる（6a と同じ。4.2 の手順 2 の「アプリへ渡す」とは違う）。
- 実装時の変更（2026-10-21、段 6 の 6b-3。#70）: 保留したキーの直接挿入（4.2 の `InsertTxn`、前提の決定 2）を入れた。5c の制限（フォーカスが移っていたキーを捨てる・IME がオフだった前のキーが失われる）を外した。(a) 再生で、前に保留したキーが実行の時点で届いた文書にフォーカスが無い（`replay_key` が実行せずに `None`）、または IME がオフだった・IME が使わなかった（`Some(false)`）とき、文字キーは打った文字のまま、届いた文書（ポインタと世代で生きていることを確かめる）の選択位置へ入れる（`insert_text_direct`。1 キーにつき 1 要求）。文字以外のキーと、文書が破棄されていたキーは捨てて WARN。OnKeyDown で処理中の今のキー（`pump_for` に渡す通し番号）で IME が使わなかったときは、今までどおりアプリへ渡す。(b) 結果が出るまで再生を止め（`check_inserting`）、その間に届いたキーは保留する（`held_or_inserting` を保留の判定に含める。追い越さない）。保留したキーがある・文字を入れている間の文字キーは、IME のオン・オフにかかわらず保留する（`holds_typed_char`。OnTestKeyDown・OnKeyDown の IME オフの判定より前）。文字以外のキーは今までどおり（IME が食べるキーを保留し、アプリ用のショートカットなどはアプリへ渡す）。(c) 文書を変えていないと分かる失敗（要求の拒否・要求の失敗・2 s の失効・composition の状態のロックが取れずに有無を確かめられなかった場合。どれも書く前にやめている）は新しい要求で 3 回まで入れ直し、それでも入らなければ捨てて WARN。SetText の失敗は、文書が変わっていないと確かめられないので入れ直さない（6a の文字の挿入も同じ。6a の注記の「SetText の失敗は要求し直す」を改める）。位置を確かめられない失敗（composition がある・選択位置が取れない・長さ 0 に縮められない）も入れ直さずに捨てて WARN。Deactivate では失効させる（失効の世代を進め、実行されても文書に触れない）。(d) この段の制限: `insert_text_direct` はこのスレッドに composition があると入れない（文書を問わない）。フォーカスの移った先の文書で composition が始まっていれば、前の文書へ入れる文字は捨てる。4.2 の「宛先ごとの列」（`focus_epoch` で列を分ける）は入れていない。
- 実装時の変更（2026-10-21、段 6 の 6c。#70）: 稼働中の切断・journal の硬上限と、「受理したキーをアプリへ戻さない」（`KeyOutcome` の一部）を入れた。利用者の決定: 稼働中にホストが応えないときのキーは、4.2 の「キーは実行されて journal の末尾が伸びる」ではなく、保留の列に入れる。(a) `handle_action` の入口の所有権の確認・復元（`recover_on_input`）が失敗したら、エンジンの準備中（6a）と同じく、直接届いたキーで IME が食べないものはアプリへ渡し、それ以外は止めて保留の列に入れる（今までは処理全体がエラーで戻り、キーは journal にも入らずアプリへ渡っていた）。(b) ホストが応えない（`HostLink` が `Disconnected`）間は、UI のスレッドで保留したキーを再生しない（2026-10-22 の実機確認で、1 s に 1 回の再生でも、そのたびに繋ぎ直しの待ちで UI のスレッドが約 3.6 s ずつ止まり、入力できなくなったため改めた）。回復は背景の接続確認（6b-1 の `host_probe`。保留しているスレッドの窓も登録する）で知り、繋がった通知（`WM_APP_LINK_UP`）で、直接入力でなければ再生する。保持の上限（30 s）は列の先頭のキーの時刻で判断し、過ぎたら（版の違いなら待たずに）回復できない場合の処置に進む。背景の確認に登録できない（窓・スレッドを作れない）ときは、回復を知る手段がほかに無いので、5 s に 1 回だけ再生を試す。UI のスレッドが繋ぎ直しを待つのは、切断に気づいた最初のキー（と、その後に列が空のときに届いたキー）だけ。ただし、切断中の Deactivate・`OnCompositionTerminated` などはホストへの RPC（と、背景の確認が持つエンジンのロック）を待つ（6b-1 の注記と同じ。今回は扱っていない）。さらに、RPC クライアントは、要求の時点で切断中（`HostLink` が `Disconnected`）で接続が無ければ、要求の種類によらず、繋ぎ直しを試さずにすぐ失敗する（接続の状態は変えない。`call_with_retry`）。繋がっていた接続がその要求の中で切れた場合は、今までどおり繋ぎ直して再送する（通常のホストの入れ替えはその場で越える。T2-1b）。切断中の繋ぎ直しは背景の接続確認の `probe()` だけが試す（`send(.., true)`）。キーの入口の確認が失敗すればキーは保留され、保留で背景の確認に登録されるので、繋ぎ直しはそこから始まる。2026-10-22 の実機確認で、言語バーのツールチップ・表示の更新・タイマーが状態の問い合わせや読み取りを UI のスレッドから繰り返し呼び、そのたびに 3.6〜12 s 止まって、メモ帳とターミナルが反応しなくなったため（診断のログ `rpc SLOW … thread=` で確かめた）。切断に気づく最初の要求（繋がっていた接続が切れた要求）の繋ぎ直しの待ちは残る。(c) journal の末尾（確かめていない部分）ができた時刻を編集状態に持つ（`unconfirmed_since`。状態を保存するたびに更新）。末尾があって、30 s 以上確かめられない・256 件を超えた・硬上限（1,024 件または `cost` 128 KiB）に達した（`journal_stuck`）ときに入口の復元が失敗したら、そのキーを `suspend_unrecoverable` で止め、30 s を待たずに処置に進む。末尾が無ければ硬上限を超えても続ける（段 7 より前は縮めない）。composition が無いのに末尾がある場合は、末尾の文字を書かずに記録と通知（「未入力」）をし、その編集状態を外す（4.2 の手順 4 の「composition を保持していない（もともと無かった）」）。文字以外の要求だけが残っていた場合も、件数を WARN に残して「未入力」を出す。(d) `KeyOutcome`: OnKeyDown で処理が失敗した（`handle_action` が `Err`）ときは、食べると決めていたかで返す。判定は同じキー（仮想キーが同じ）の直前の OnTestKeyDown のものを使い、無ければ（OnKeyDown だけが呼ばれた）composition の有無による `key_should_eat` で決める。保留したキーの再生で失敗したキーは食べたものとする（文書へ入れ直さない）。4.1 の `KeyOutcome` の列挙型そのものは入れていない。
- 実装時の変更（2026-10-21、段 7 の 7a）: engine ABI を 12 に上げ、4.8 の 2 関数を入れた。利用者の決定: ABI は今 12 に上げ、#75 の段 4 の 3（DLL のログの callback）は同じリリースの前に同じ版 12 へ足す（途中の状態は配布しないので、利用者から見た版上げは 1 回）。(a) エンジン（rakukan-engine の `seed_preedit` / `retain_range`）: 入力ログの項目の種類に `Seeded` を足した。`seed_preedit` は 4.8 の規則どおり（綴りの最長一致、優先表、「っ」「ん」の規則、逆引きの無い文字は Raw）。綴りの選び方は、優先表に無いかなでは「最短、同じ長さなら `c` / `q` で始まらないもの、その中で辞書順の先頭」とした（4.8 の「辞書順の先頭」だと か = ca・く = cu になり、F-8 の期待「kasa」と合わないため）。「っ」は、次の綴りが `n` で始まる場合は重ねずに `xtu` とした（`nn` は「ん」になるため。例: 「っん」は `xtun`）。`retain_range` は 4.8 のとおり（出力の連結が読みと一致し、表示の差し替えの後でなければ、区間に入る項目を残し、境界の項目は残る部分を種にする）。種の項目は Backspace で 1 項目の閉じた区間として書き換える（複数文字のかなを 1 文字ずつ消せる）。(b) FFI に `engine_seed_preedit` / `engine_retain_range` を足し、版を 2 か所（rakukan-engine-abi/src/lib.rs、rakukan-engine/src/ffi.rs）で 12 にした。(c) ホストの `SeedPreedit` / `RetainRange`（journal の流し直し・照合点の種・`Change`）を、代替（`reset_preedit` と各文字の `push_raw`）からエンジンの関数に切り替えた。4.8 の代替期間の制限 (a)(b) はこれで外れる。試験: F-8・F-9（種の綴り）・F-10 と、`retain_range` の打鍵の保持・食い違いのときの種、種の項目の Backspace をエンジンの単体で確かめる。
- 実装時の変更（2026-10-21、段 7 の 7b）: TSF 側を 4.7・4.8 に合わせた。利用者の決定: 部分確定の残りは `RetainRange` ではなく `SeedPreedit(残り)` で入れる（今の部分確定は `commit`（文脈に足すと同時に読みと打鍵ログを消す）の後に残りを入れ直すので、`RetainRange` で残りの打鍵を保てない。F9 / F10 は種の綴りになり、打った綴り（`zi` など）には戻らない。F-4・F-5・F-7 の期待は種の綴りで満たす）。(a) 部分確定の残り（edit_ops.rs の番号キーでの選択、on_convert.rs の範囲指定の確定と候補の確定）の `reset_preedit` + 各文字の `push_raw` を `SeedPreedit` に替えた。(b) 選択中の記号（edit_ops.rs:605 だったもの）を `ForcePreedit` から `SeedPreedit` に替えた（記号が途中に入った読みは打鍵ログと対応しないため。F は種の綴り。4.8 の「末尾に足す場合にログを保つ方法」は扱っていない）。(c) 変換中の F6〜F10: セッションが変換中（Waiting / Selecting / BlockSelecting / RangeSelect）なら、全体の読み（`SessionState::conversion_full_reading`）がエンジンの読みと違うとき `ForcePreedit(全体)` で戻してから変換する（F-6）。(d) 軟上限: 編集要求を journal に足す前に、軟上限（512 件または `cost` 32 KiB。硬上限を超えた場合も同じ）を超え、末尾が空なら、照合点の値で `[SeedPreedit(読み), PushChar(未確定ローマ字)…]` に縮める（`shrink_journal`）。Rebased の作り直しも同じ形（`seeded_journal`）を使う。(e) 復元直後の処置（4.7 の表）: 復元の結果（`InputEntry::rebased`）とセッションで決める。Exact で Selecting / BlockSelecting / RangeSelect / LiveConv はセッションと候補ウィンドウを保ち（`llm_pending` を外す。LiveConv なら BG を開始し直す）、今のキーは通常どおり。Exact で Preedit / Waiting / それ以外は Preedit にして候補ウィンドウを隠し、今のキーは通常どおり（復元直後の Space も変換する）。Rebased は、全体の読みでセッションを Preedit にし（エンジンの読みと違えば `ForcePreedit`）、候補ウィンドウを隠す。文字を生むキーとモードキーは続け、それ以外（Space・候補・F キー・Backspace・Esc・カーソル）は表示の修復だけで終える。BG の処置（タイマーの停止・preview の列を空にする・変換の世代を進める・モデル待ちの消去）はどの場合も行う。セッションが持つ LiveConv の preview そのものは Exact なら保つ（4.7 の表）。(f) V6_Test_Procedure.md の T2-1b の「見る点」を 4.7 の決定どおりに改め、T2-3 に段 7 以降の期待を足した。
- 実装時の変更（2026-10-21、段 8）: 4.9 の分類を入れた。(a) `Reason`: `ConfigStale` / `ConfigUnusable` は段 4 で protocol に入っている。クライアントの拒否の扱い（`Change`・`Restore`・`RestoreJournal` で台帳の未解決を解決するか）を、`_` を使わない `match`（`rejection_may_have_applied`）にした。照合の拒否（`GenMismatch` / `OwnerMismatch`）は適用していない、`ResultUnavailable` と予約の 2 つは結果不明（通信失敗と同じ）。(b) 接続の失敗の分類: `DisconnectKind` を `Transport` / `Suppressed` / `VersionMismatch` / `HelloError` / `CreateError` にした（6b-1 の `Other` を分けた）。接続の試行の失敗には、失敗の段階（`AttemptStage`）から分類した `ConnectError { stage, kind }` を付け（表示は元のエラーの文言のまま）、`HostLink` はそれで分類する。版の違いは今までどおり `ProtocolMismatch` で見分ける。繋がっていた接続を捨てた場合と、分類の付いていない失敗は `Transport`。4.9 との違い: `stage` は `AttemptStage` ではなくその名前の文字列、`VersionMismatch` はホストの版の値を持たない（版の値はログの文言に残る）。4.2 の `EngineAccess::NoEngine` は入れていない（`EngineAccess` を作っていないため）。判断に使うのは今までどおり `VersionMismatch` かどうかと、切断しているかどうかだけで、他の分類はログと今後の拡張のため。
- 実装時の変更（2026-10-22、4.2 の第 9 版の案の実装と、実機確認で見つかった切断中の停止の修正。#70）: (a) 6c の (b) の改め（切断中は UI のスレッドで保留したキーを再生しない）と、RPC クライアントの規則（要求の時点で接続が無く切断中なら、要求の種類によらずすぐ失敗する。切断中に繋ぎ直すのは `probe()` だけ）。詳しくは 6c の注記 (b)。(b) `DisconnectKind` に `HostMissing`（自分で起動を試みて失敗した。起動の後の接続の待ちは 0.5 s）を足した。6b-1 の注記の `Disconnected(VersionMismatch | Other)` と段 8 の注記の 5 分類は、この時点で `Transport` / `Suppressed` / `VersionMismatch` / `HelloError` / `CreateError` / `HostMissing` の 6 分類になった。判断に使うのは、切断しているかどうかと、待っても戻らない分類（`VersionMismatch` / `HostMissing` / `Suppressed`。4.2 の第 9 版の案）。(c) 知らせの窓（`tsf/notice.rs`）と、直接入力の間の言語バーのツールチップ（知らせと同じ文面の先頭に `rakukan: ` を付けたもの）。6b-1 の注記の「ツールチップの文言は変えていない」は、この時点で改めた。(d) 診断のログ（キーの判断 `key_diag:`、救済の各段階 `rescue:`、`engine_get` の待ち、背景の確認がロックを持った時間、`rpc SLOW` のスレッド）。
- 実装時の変更（2026-10-22、5c の残りの最初の部分 5c-1）: 全体の確定（`commit_full`）で確定しなかったときに、composition・ホストの読み・セッションの状態を戻すようにした（4.6 の「取り出した composition は確定しなかった結果で必ず戻す」と S4）。(a) `DocResult::Aborted { why, changed }` を足した。EditSession の中の `GetRange` の失敗と `EndComposition` の失敗では、composition を `composition_set_with_dm` で保持し直す。保持し直せなかった（ロックの poison）ときは `Aborted` にせず、composition を手放した `Failed` として今までどおりの後処理をする。`EndComposition` の失敗のときは、置き換えた範囲を読んでおいた元の文字列に戻す SetText を試し、戻せなければ `changed`（置き換えた文字列のまま残る）。(b) 確定しなかった結果（`Aborted` と `NotRun`）で、確定を要求した時点の composition（文書と同一性、`composition_key`。終えるまでその参照を持ち、同一性のポインタ値が別の composition に使い直されないようにする）を終える時点でも保持しているときは、ホストへ何も送らない。要求した時点に composition が無かった（直接の挿入）・別の文書や別の composition に替わった・ロックが取れず確かめられないときは、今までどおり閉じる（composition が残っていれば、この段の前と同じ食い違いが残る）。保持しているときは、BG の回収・`reset_preedit` も送らずに読みを残し、キーの入口（`handle_action`）で控えたセッションの状態（`commit_txn::KeySession`。候補・選択・句読点の保留を含む）に戻す（`SessionState::restore_from`。選択中の印も合わせる。ロックが取れなければ戻さず WARN）。キーの処理の外の確定では控えが無いので、セッションは戻さない（ホストの読みと composition は残る）。(c) 対象は全体の確定だけ。部分確定（`commit_then_start_composition`）は、文脈・`reset_preedit`・残りの引き継ぎを文書より先に送っている（5b）ので、今までどおり閉じる。IME の切り替えの前の確定（`switch_ime`）は、切り替えた後に composition を残さないよう、今までどおり手放して閉じる（`CommitEffects::close_on_abort`）。(d) 直接の挿入（composition が無い）の失敗は今までどおり `Failed`。(e) この段の制限: 確定の前に呼び出し側が隠した候補ウィンドウ・止めたタイマーは戻さない（Selecting に戻った後、候補を動かすキーで再び出る）。`changed` のときの表示の作り直し（4.6 の「表示が古い印」と `display_epoch` の再要求）は入れていない（次のキーで表示を作り直す）。確定の前にホストへ送った変更（`flush_pending_n`・CycleKana の `force_preedit` など）は戻らない。IME の切り替えの前の確定（`close_on_abort`）で EditSession が実行されなかった（`NotRun`）ときは、composition が文書に残ったまま、ホストの読みとセッションへは閉じる後処理を送る（この段の前と同じ扱い。composition を手放すには EditSession が要るので、切り替えの後に確定を要求し直す仕組みと合わせて後の段で扱う）。
- 実装時の変更（2026-10-22、5c の残りの 5c-3。設計は 4.6 の「部分確定の文書先行（5c-3）」、利用者の決定は 9 節の 5〜9）: (a) `commit_then_start_composition` を `PartialPlan`（確定部分・学習・引き継ぎの要求・Preedit にするか・BG を始めるか・ライブのタイマー）で呼ぶ形にし、6 か所の呼び出し元は文脈・`reset_preedit`・引き継ぎを送らずに呼ぶ。文脈に送る文字列は 6 か所とも確定部分と同じなので、`PartialPlan` に別の欄は持たない。(b) TSF 側の口: `DynEngine::partial_handoff`（古い編集状態を外し結果不明を断念してから、`created_under` と `tsf_comp` が 0 の新しい状態に引き継ぎを入れて `RestoreJournal` を 1 回送る。成功したら引き継いだ後の読みを返す）、`RemoteComposition.pending_effects`、`CompositionRpc::restore_journal` の `pre_effects`。`restore_journal_via` は `pending_effects` を送り、照合で拒否（`GenMismatch` / `OwnerMismatch`）・別の要求の結果不明に阻まれた場合だけ残し、それ以外は消す（`effects_settled`）。(c) 部分確定の後処理（表示の要求し直し・BG の開始・ライブのタイマー・確定しなかったときに古い読みを閉じる・回復できない切断の救済）は、要求に持たせる `commit_txn::set_after` の処理（`after_partial`）で行う。`finish_one` が後処理を送った後に 1 回だけ呼ぶので、保留したキーの再生より前になる。古い読みを閉じる（`bg_reclaim`・`reset_preedit`）のは、composition を手放した `Failed` だけ。`Aborted` / `NotRun` で要求の時点の composition が今は無い・替わった場合は、今のホストの状態が替わった先のもの（終わった composition の状態は終了通知の後始末が閉じる）なので触れない（5c-1 の全体の確定は、この場合も閉じる）。5c-1 の復元から部分確定を外していた条件（`leaves_input`）は外した（`leaves_input` はキーの保留の判定だけに使う）。`CommitEffects.bg_start` は本番の呼び出し元が無くなった（試験だけが使う）。`take_bg_started` は使う所が無くなったので外した。(d) 回復できない切断で引き継げなかったときは、新しい編集状態の確かめていない末尾（`take_unconfirmed_tail`）を `insert_text_direct` で入れる。このため `typed_tail` は `SeedPreedit` を「そこまでの文字を種の読みに置き換える」として数えるようにした（今までは数えるだけ）。(e) Selecting 中の文字（`on_input`）の診断の `InputChar` は、新しい文字の後の表示が引き継ぎの後まで分からないので、`preedit_after` を空で記録する。(f) RangeSelect の部分確定の `start_live_bg_if_ready` は、文書の確定の後（`after_partial`）へ移した。
- 実装時の変更（2026-10-22、5c の残りの 5c-2。CommitThenInput の確定の再試行）: (a) 対象は `PartialPlan.retry` を立てる 3 か所（`on_input` の Selecting / BlockSelecting 中の文字、`on_input_raw` の Selecting）。Enter・番号キー・RangeSelect の部分確定は、4.6 の理由の表のとおり再試行しない。(b) 4.1 の `KeyTask`（`step = AfterCommit`）は作らず、今の key_queue の「止めて列に戻す」仕組み（`suspend_current` → `put_back`）に乗せた。キーの処理の間（OnKeyDown と再生）、そのキーの通し番号と写しを控え（`key_queue::CurrentKeyScope`）、確定を要求する時点で取る。確定しなかった（`Aborted` / `NotRun`。composition が残っているかにかかわらない）ら、そのキーを列に戻し、再生で今の状態から処理し直す（確定からやり直す）。そのキーの処理の中なら止めて処理した側が戻し、処理の後なら通し番号の順の位置に戻す。後のキーの処理の入口の後処理（`handle_action` の `finish_completed`）で戻した場合は、後のキーも止めて列に戻し、追い越させない。その印（`key_queue::yielding`）は止まった印とは別に持ち、読みの長さの上限の入れ子の確定（`CommitRaw`）が止まった印を消費しても、外側のキーが処理を続けずに戻る。フォーカスが別の文書へ移っていれば、今の再生の仕組みが元の文書へ入れる。composition が替わっていた（`keep` でない）場合も再生するのは、打ったキーを捨てない（前提の決定 2）ため。再生は、その時点の状態から同じキーを処理する。キーを捨てた・別の文書へ入れた・すべて捨てた（保持の上限・回復できない場合の処置・Deactivate）ときは、回数の表も消す。再生したキーが止まらずに処理され、確定の結果待ちが無ければ消す（状態が変わって部分確定を通らなかった場合など）。結果待ちがあれば、その後処理が成功なら消し、失敗なら数える（再生のたびに消すと、後で実行される確定の失敗で回数が数え直しになり、際限なくやり直すため）。列に戻しても受け付けた時刻は変えないので、最初の入力から保持の上限（30 s）を過ぎれば、通常の保留キーと同じく捨てる。(c) 回数は通し番号ごとに 3 回まで（R1。`COMMIT_RETRY_LIMIT`）。超えたらキーを捨てて WARN を残し、「文書へ書き込めません」を出す（`mode_indicator`。4.6 の「文書が編集を受け付けない」の文言）。(d) `Aborted{changed: true}`（範囲が縮めた文字列のまま）なら、列に戻す前に、戻したセッション（Selecting / BlockSelecting）の表示を composition に書き直す要求を出す（`update_composition_candidate_parts`）。4.1 の「表示を作り直してから」に当たる。その要求の完了は待たずに再生する。再生の確定は、範囲を確定部分に書き直して（S2）読み直して照合する（S2'）ので、直前の表示には頼らない。表示の作り直しは、やり直しがすべて失敗してキーを捨てたときに、古い表示を残さないためのもの。(e) 確定に失敗して composition を手放した（`Failed`）ときは再試行しない（今までどおり）。
- 実装時の変更（2026-10-22、5c の残りの表示の世代 `display_epoch`。利用者の決定: おすすめの案で、フォーカスの移動も含め、確定しなかったときは表示をもう一度出す）: (a) 6b-2 で救済の締め切りだけに入れた表示の世代（on_compose の `DISPLAY_EPOCH`、`display_allowed`）を、`advance_display_epoch` で次の時点にも進める: 確定トランザクションを作るとき（`commit_txn::begin`。全体の確定・部分確定）、空の文字列で閉じるとき（`end_composition`）、アプリが今の composition を終わらせたとき（`OnCompositionTerminated`。部分確定の置き換えの途中は進めない）、フォーカスが別の文書へ移ったとき（`OnSetFocus` の同期の部分。thread-local の数を進めるだけ）、Deactivate。(b) 今の照合（T-MID2）は、要求の時点と実行の時点の composition が同じかだけを見るので、要求の時点に composition が無かった表示（入力の最初の文字）が、確定・取り消し・フォーカスの移動の後に実行されると、そこで新しい composition を作って書いていた（※推測。実機では未確認）。世代が進んでいれば、composition を作らず SetText もしない。(c) 世代が進んだために実行しなかった表示の要求（最後の 1 件。救済の途中は除く）を、要求の時点の世代と一緒に控える（`VoidedDisplay`）。確定トランザクションは自分が進める前の世代を持ち、結果の後処理（`finish_one`）で、控えがその世代の要求なら、確定しなかったために composition を残すとき（`keep`）で、その確定の後にほかに世代が進んでいない（今の世代が、確定が進めた直後のまま）ときだけもう一度出し、それ以外は捨てる（`settle_voided_display`）。確定の後にフォーカスの移動・入力を閉じた・composition の終了があれば、控えの文脈と composition は古いので出さない。それより古い控えは捨て、新しい控え（後の確定の前の表示）は残す。確定の後処理より後に実行されて控えられた要求を、別の確定の結果で出さないため。世代を進める時点では控えを捨てない（`OnSetFocus` の同期の部分で COM の参照を放さない）。確定の前に要求した表示が実行されないまま、composition の表示がホストの読みより古く残らないため。
- 実装時の変更（2026-10-22、フェーズ 2 の完了の作業。6 節の試験の表の判定の列と利用者の決定による）: (a) 試験 5: キーは届いた時点で解決して列に積むので、保留中に Space → `1` と打つと `1` が文字になっていた。届いた時点で数字キーの規則（`resolve_action_with` の ②）に従ったキー（Ctrl・Alt なし、キーマップに割り当てが無い 1–9 / テンキー 1–9。Shift は ② と同じく問わない）に印を付けて列に積み（`HeldKey.digit_rule`、`Keymap::follows_digit_rule`）、再生の時点で候補を選んでいれば候補番号にする（`keymap::reresolve_digit_for_replay`、`replay_key`）。制限（利用者の決定、2026-10-22）: 逆向き（届いた時点は候補を選んでいて候補番号、再生の時点では選択が終わっている。保留中に Enter → `1` など）は直さない。文字に戻すには押した時点の文字が要り、そのために `ToUnicode` を余分に呼ぶとデッドキーの状態に触れうるため。このキーは、修正の前と同じく何も入らない。(b) 試験 1: OnTestKeyDown と OnKeyDown の入口の判定（保留した文字・IME オフ・直接入力）を `entry_gate` にまとめた。保留した文字がある間の文字キーを、直接入力の間に OnTestKeyDown は食べ、OnKeyDown はアプリへ渡していた（まだ入っていない文字を追い越しうる。※実機では未確認）ので、OnKeyDown も保留する。(c) 試験 35: 読みの長さの上限で確定した後の分かれ道を `after_limit_commit` に切り出した（動作は同じ）。(d) 試験 12: 期待を今の動作（Enter は救済で捨てる）に改めた（利用者の決定）。(e) 試験 13・17・27 の試験を足した（6 節の判定の列）。(f) 1 つのプロセスで複数のスレッドが Activate すること（メモ帳・Explorer）を確かめ、#86（重要度は低）に記録した。WARN と DLL の読み込みのログにアプリ名（`app=`）を出す。
(c') 表示の要求に通し番号を付け（`DISPLAY_SEQ`）、最後に SetText が成功した番号（`APPLIED_SEQ`）より古い要求は、後から実行されても書かない。もう一度出した要求と新しい表示の要求が要求の順に実行されない場合に、古い表示が新しい表示を上書きしないため（今までのキーの表示もこの順に頼っていた）。(d) 4.5 の表示要求 ID と `requested` / `shown`、「表示が古い印」と入口の表示の修復は入れていない。
- 実装時の変更（2026-10-22、フェーズ 2 の実機試験で見つかったもの。V6_Test_Procedure.md の 2026-10-22 の節）: (a) 範囲の選択の始まり: 打っただけの読み（セッションが Idle。ライブ変換のタイマーが動く前）で Shift+← / Shift+→ が効かなかった。Preedit と Idle のどちらからでも RangeSelect に入る（`reading_for_range_start`。表示がかなの読みと一致し、読みが Shift+← は 2 文字以上・Shift+→ は 1 文字以上のときだけ）。未変換のローマ字が残っている（「しかｎ」）間は今までどおり無視する（利用者の決定）。(b) RangeSelect の Esc / Backspace: 範囲を選んだ時点の表示（`original_preview`）が空のとき（Idle から入ったとき）に composition が空になっていた。空なら全体の読みに戻す（`range_select_return_display`）。(c) 言語バーのメニュー: メモ帳で `TrackPopupMenu` がエラー 87 で失敗した（所有の窓 `GetForegroundWindow()` が呼び出し元と別のスレッドの窓だった）。所有の窓をフォーカスのある窓（`GetFocus()`、無ければ `GetForegroundWindow()`）にし、`TPM_NONOTIFY` を付けた（Microsoft のドキュメントと mozc・CorvusSKK の実装に合わせた）。(d) 救済の間に押したキー: 救済で UI のスレッドが止まっている間に押したキーは、救済の後（直接入力の間）に OnKeyDown へ届き、モードキー以外はアプリへ渡っていた（Enter で改行が入った。試験 12 の期待に反する）。救済が終わって直接入力に入った時刻（`GetTickCount`）を控え（`mark_fallback_started`）、直接入力の間に届いたキーで、その時刻から 3 s 以内かつキーのメッセージの時刻（`GetMessageTime`）がその時刻より前のものは、文字キーなら今までどおりアプリへ渡し、文字以外（Enter・Backspace・モードキーなど）は食べて捨てる（`EntryGate::Drop`、`pressed_before_direct_input`。利用者の決定）。OnTestKeyDown と OnKeyDown で判定が食い違わないよう、同じキー・同じ時刻の判定を使い回し、OnKeyDown で消す。`GetMessageTime` が OnKeyDown の中でそのキーの時刻を返すことは、実機のログ（`key after the rescue … pressed_before=`）で確かめた（2026-10-22）。
- 規模は ※推測。1・2 は今のコードの写しなので比較的確か。3〜6 は非同期・再入・宛先・スレッドの組み合わせで幅が大きく、合計（5,550〜9,600 行）は上限として信用できる値ではない。
- 前提の決定 1 により、フェーズ 2 はフェーズ 3 と同じリリースで出す。engine ABI はフェーズ 3（#75）の変更と合わせて 1 回だけ上げる（版の番号は #75 と合わせて決め、2 か所を同時に直す）。1〜3 は動作の修正として先に main に入れてよい。4〜7 の間の中間の状態（縮約が無い・代替の `SeedPreedit`）は配布しない。
- 依存: 5 は 3（記録・`ReqCell`）と 4（引き継ぎの journal）に依存する。6 は 3（`HostLink`・`probe()`）と 5（確定の理由・列の核）に依存する。7 は 4（`SeedPreedit` / `RetainRange` の protocol）に依存する。前の段が後の段の機能を使うことはない（上限は判定だけを 4 で入れ、有効化はそれぞれ 5・6・7）。

## 8. 範囲外

- ホストへの再接続の間に UI のスレッドが待つこと。spawn 後のパイプ接続待ちは 1 回あたり最大 5 s（client.rs:43）だが、`ensure_connected` は失敗すると 200 ms 待ってもう 1 回試し（client.rs:1186-1207）、`call_with_retry_inner` も 2 回試す（client.rs:1144-1170）。さらに、各試行の書き込みと読み取りは同期の I/O で、読み取り（client.rs:1160 の `read_frame`）には期限を付けていないので、ホストが応答しないまま接続が切れなければ待ち続ける。このため 1 回の RPC の待ちに全体の上限は無い。稼働中の切断では、キーごとにこの待ちが起きうる。
- #66 の本体。
- 4.8 の表に無い F6〜F10 の制限（例: F9 の後に Backspace で消した場合のログの扱い）。

## 9. 利用者（プロジェクトの持ち主）の決定

2026-10-21 に次の 4 項目が決まった（前提にも記載）。設計に残っている、利用者の判断が要る事項は無い。

| # | 事項 | 決定 | 反映した節 |
|---|---|---|---|
| 1 | リリースのまとめ方 | フェーズ 3 と同じリリース。engine ABI の変更は 1 回。protocol v7 は古いホストを止めるインストーラーと一緒に出す | 5、7 |
| 2 | 保留中にフォーカスが移った場合のキー | 元の文書が生きていれば、文字キーを打った文字のまま入れ、文字以外は捨てる（回復できない場合と同じ）。生きていなければ捨てて WARN | 4.2、4.6、6 の 7 |
| 3 | T2-1b | 正確に復元できたときは Space で通常どおり変換する。V6_Test_Procedure.md の T2-1b の期待値を改める | 4.7、6 の 30、7 の 7 |
| 4 | Enter の末尾の未確定 `n` | `ん` にして確定する（今の挙動）。「表示どおり」の承認済みの例外 | 4.5、6 の 25 |

2026-10-22 に、部分確定の文書先行（5c-3）について次の 5 項目が決まった（Codex の設計レビュー 6 回の後、設計案のとおり）。

| # | 事項 | 決定 | 反映した節 |
|---|---|---|---|
| 5 | 部分確定の文書先行の作り方 | EditSession の中で EndComposition の成功後に `RestoreJournal` を 1 回送る（仮の表示と作り直しは作らない） | 4.6 |
| 6 | EditSession の中でホストが応答しないとき | アプリの文書のロックを持ったまま待つ危険を受け入れる | 4.6 |
| 7 | 確定部分に縮める SetText の失敗 | 確定を止める（`Aborted`）。今は全体を確定している | 4.6 |
| 8 | H の結果が分からないとき | 学習と文脈を再送しない（失いうる。二重の適用を避ける） | 4.6 |
| 9 | H が一時的に失敗したとき | 次のキーまで引き継ぎの文字を表示しない（次のキーで流し直す）。CommitThenInput の再試行（5c-2）はこの後の段 | 4.6 |

## 10. Codex の指摘との対応

1 回目のレビュー（第 2 版で対応、第 3・4 版で更新）:

| 指摘 | 内容 | 対応する節 |
|---|---|---|
| 1 | 未確認キーの消失・今のキーの位置・確認件数 | 4.3、2.1、4.1（R1） |
| 2 | 台帳・別プロセス・保存応答と所有権・同一プロセスの 2 DM | 4.3（台帳との関係）、4.4 |
| 3 | F | 4.8 |
| 4 | 非同期確定・複合失敗・要求 ID・古い完了・確定待ちのキー・`end_composition` | 2.3、4.6 |
| 5 | 表示要求値と成功値・未確認の全種類 | 2.2、4.5 |
| 6 | 保留キーの解決時期・受付と実行 | 4.1、4.2 |
| 7 | 宛先の照合 | 3、4.2 |
| 8 | 履歴と現在の入力・ロック・修飾キー | 4.1 |
| 9 | 復元の範囲・副作用 | 4.7 |
| 10 | 上限とコスト | 4.3（R7） |
| 11 | 自動再開は必須 | 4.2 |
| 12 | 事実の誤り | 1 |
| B-1 | 1 キーに複数の操作 | 2.1 |
| 順序 | 安全な分割 | 7 |
| v6 のまま進める案 | 採らない（利用者が v7 を決定。過去のログが戻らない） | 4.3 |

2 回目のレビュー:

| 指摘 | 内容 | 対応する節 | 対応 |
|---|---|---|---|
| R1 | 受理した文字を捨てる経路 | 4.1（`KeyTask`）、4.6（再試行）、4.7 | 未実行の部分を残し、修復・再試行の後に 1 回だけ続ける。捨てるのは文書が編集を受け付けない場合だけ |
| R2 | 確定待ちのキーと入力世代 | 3（失効と世代の更新）、4.6（完了・失効・再試行）、4.4 | トランザクションへの結び付けと成功時の引き継ぎ。文脈の切り替えは完了か時間切れを待つ |
| R3 | 回復できない場合の挿入 | 4.2（処置・`InsertTxn`）、4.6（`post_commit_range`） | 要求 ID・位置・結果・再試行。完了まで列から消さない。確定待ちと重複させない |
| R4 | 範囲選択と `RetainRange` | 4.8 | 一時的な値と永久の切り取りを分け、全箇所を分類。`sikan` の期待値 |
| R5 | ホストの回復の確認 | 4.2（`EngineAccess` と `HostLink`）、4.9 | 背景の `probe()` の成功で直接入力を外す。稼働中の切断・版の不一致を含む |
| R6 | Enter と表示の修復 | 4.5 | Enter は修復しない。`display_epoch` で失効。`GetText` の結果を実績値に（第 5 版で `original_text` / `committed_text` に分けた: V4） |
| R7 | 費用と上限 | 4.3（費用・上限） | エンジン内部の写しを作らない経路、文字列の総量、追加前の判定、読みの長さの上限 |
| R8 | 実装順 | 7 | 1 は動作不変・実際に経路を通す。fallback は 2。接続の型と確定の完了管理を先に |
| R9 | モードキー | 4.2 | 保留したモードキーも捨てる。直接入力の後のモードキーは別扱い |
| R10 | スレッド | 1、4.4、7 の 3 | 候補ウィンドウはスレッドローカル。所有スレッド `INPUT_OWNER` |
| 付記 | 照合点の名前 | 4.3、5 | `checkpoint_edit` / `final_edit`。書き換えは照合点の値 |
| 付記 | タイマー | 4.2（再生の契機） | Activate で `ensure_hwnd`、作れない場合・Deactivate |
| 付記 | 末尾の `n` | 4.5、9 節 | 承認済みの例外として `ん` にする（利用者の決定 4） |
| 付記 | 設計側で決める事項 | 3、4.1〜4.6 | 世代の引き継ぎ（4.6）、拒否・時間切れ・通知の取りこぼし（4.6, 4.2）、今のキーの未実行の部分（4.1）、範囲選択中のログ（4.8）、ホストの回復の条件（4.2）、Enter と修復の順（4.5）、上限を超える入力（4.3）、共通とスレッドローカルの所有（4.4） |
| 付記 | 規模の数え方 | 7 | 追加・変更した行（移動は数えない） |

3 回目のレビュー:

| 指摘 | 内容 | 対応する節 | 対応 |
|---|---|---|---|
| N1 | 時間切れの要求の遅れた実行 | 2.3（TV）、4.6（`ReqCell`）、4.2（`InsertTxn`） | 実行開始と失効のどちらが先かを `ReqCell` で一意に決める。失効した要求は文書に触れない。実行した結果は必ず回収。自分の EndComposition の間の終了通知はそのトランザクションのもの |
| N2 | 宛先の違うキーの置き場所 | 4.2（宛先ごとの列）、4.4 | 宛先（文脈 + `focus_epoch`）ごとの列を着いた順に並べる。A → B → A は 3 本 |
| N3 | Full の置き換え | 4.6（種類）、4.5 | `AsDisplayed` と `Replace{TrailingN / Katakana}`。置き換えの失敗は今と同じく表示どおりに確定 |
| N4 | 自動確定と未表示の文字 | 4.6（自動の確定）、4.3（上限） | 自動の確定には「表示どおり」を適用しない。表示の完了を待ち、範囲の文字列が計画と同じときだけ確定。後から届いた分は次の入力へ |
| N5 | 回復できない場合の最初の確定の失敗 | 4.2（処置の手順 2・5）、4.6（確定の理由） | `CommitReason` を記録。最大 3 回 → 表示どおり 1 回 → 手元の状態を外す、の後に挿入と直接入力。（第 5 版で V1・V2 により手順を改め、時間の保証は条件付きにした） |
| N6 | 段 1 の評価順・段 4 の前方参照 | A 節、7 節 | 段 1 は API・エンジン取得・ログの順序を変えない（`ToUnicode` を前に動かさない、dispatch.rs:27 に触れない）。各段の追加と有効化を分け、上限の有効化を 5・6・7 へ |
| 付記 | 代替期間の F | 4.8 | 縮約は段 7。代替の `SeedPreedit` は Rebased のときだけで、そのときの F の制限を明記 |
| 付記 | `HostLink` の通知 | 4.2（直接入力） | 通知を受けたら最新の状態と `link_epoch` を照合してから外す |
| 付記 | `INPUT_OWNER` の適用範囲 | 4.4、6 の 31 | Deactivate（factory.rs:640, 643, 657）・終了通知・タイマーにも適用。所有者でないスレッドの Deactivate の試験 |
| 付記 | 写しを作らない経路 | 4.3（費用） | 早期 return を含む 5 か所の `current_preedit()` を内部関数から外す |
| 事実 | `ensure_connected` の 5 s | 1、8 | 5 s は接続待ち 1 回の上限。再試行と、期限の無い同期の読み取り（client.rs:1160）のため全体の上限は無い（第 5 版で根拠を追加） |
| 付記 | 段 1 の仕様 | A 節 | ファイル・関数・変えてはいけないこと・試験・完了の条件 |

4 回目のレビュー:

| 指摘 | 内容 | 対応する節 | 対応 |
|---|---|---|---|
| V1 | 表示が空・composition が無い場合の受理済みの入力 | 4.2（回復できない場合の処置の入口・手順 4）、4.6（`Overwrite`）、6 の 38 | 入口の条件を「受理したが確定していない入力がある」（journal と列）にした。composition があれば `Overwrite`、無ければ `InsertTxn`。予算を使い切った場合まで定義 |
| V2 | 有限時間の保証 | 4.2（締め切り・締め切りの後のキー・再試行の予算と時間）、6 の 36・39 | 入口で対象を締め切り、以後のキーは直接入力でアプリへ。表示・確定・挿入で予算 4 回を共有。`Running` に上限が無いことと前提を明記し、無条件の時間の保証は外した |
| V3 | `Replace` の SetText 成功後の EndComposition の失敗 | 4.6（種類の注記・S4）、6 の 34 | 元に戻せたら `Aborted`、戻せなければ `AbortedChanged`（`shown` を更新し表示が古い印） |
| V4 | 部分確定の実績値 | 4.6（`original_text` / `committed_text`、T4）、6 の 40 | 照合と元に戻す用の値と、確定した値を分けた。学習・文脈は確定部分だけ |
| 低 | A 節の注記 | A 節、4.1 | 割り当ての無い keymap で試す、`Modifiers::current()` をそのまま呼ぶ、4.1 の形は段 5 から、差分のレビューで factory.rs:730-745・807-824 を確かめる。A 節の中身は変えていない |
| 低 | 行番号 | A 節、1 | `resolve_action` は keymap.rs:435-522 |
| 低 | 全体の上限が無い根拠 | 8 | 期限の無い同期の読み取り（client.rs:1160）を追加 |

5 回目のレビュー:

| 指摘 | 内容 | 対応する節 | 対応 |
|---|---|---|---|
| W1 | 古い表示の要求と挿入の二重 | 4.2（救済の手順 1・4）、6 の 38 | 締め切りで未実行の表示の要求をすべて失効させ、通常の再生と表示の修復を止める。composition の有無は救済の EditSession の実行の時点で見る |
| W2 | 救済の途中の回復の通知 | 4.2（`RescueState`・直接入力・手順 7）、6 の 37 | 接続の状態と救済の進み具合を分け、救済が終わるまで直接入力を外さない。終わりに最新の状態を読み直す |
| W3 | 締め切りの後のアプリの編集と `Overwrite` | 4.2（手順 4）、4.6（`Overwrite` の種類）、6 の 39 | 範囲を `shown_at_cutoff` と比べ、違えば上書きしない。表示に出ていない残りだけを入れる。終了通知の後も同じ |
| W4 | S2 の後の読み直しの失敗 | 4.6（S2'） | 独立の手順にし、読めない・違う・元に戻せない場合を `Aborted` / `AbortedChanged` に分けた。読めない場合は `shown` を「不明」にする |
| W5 | 12 s の意味と回数の表現 | 4.2（予算と時間）、4.6 の理由の表、6 の 36 | 12 s は `Pending` の待ち枠の合計の目安。回数は「4 回 + 最後の 1 回」に揃えた |
| W6 | 試験 8 | 6 の 8 | 締め切りで直接入力、`Overwrite` / 挿入の経路に合わせた |

6 回目のレビュー:

| 指摘 | 内容 | 対応する節 | 対応 |
|---|---|---|---|
| W3（残り） | 再試行で自分の書き込みとアプリの編集を区別できない | 4.2（`RescueRecord`・状態の表） | X1 と同じ（第 8 版で Y1〜Y4 として置き換え） |
| X1 | 上書きの後の再試行で末尾が二重になる | 4.2（`written` と `Written` の状態。救済は元に戻さない）、6 の 41 | 再試行は締め切りの時点の表示ではなく、救済が書いた `written` と比べる。`Written` からは EndComposition だけをやり直す |
| X2 | 確定済み・挿入待ちの途中結果 | 4.2（`Committed` の状態）、6 の 42 | `committed`・`remaining`・`anchor` を次の試行へ引き継ぎ、挿入だけをやり直す。確定した部分は書かない（第 8 版で削除: Y3） |
| X3 | 終了の後の挿入の位置 | 4.2（挿入の位置 `anchor`・`comp_end_anchor`・位置の確認）、6 の 43 | 今の選択位置に頼らず、表示と確定のたびに控える composition の末尾の範囲を使う。確かめられなければ入れずに WARN と通知（第 8 版で削除: Y3） |

7 回目のレビュー（第 8 版で救済を簡素化して対応）:

| 指摘 | 内容 | 対応する節 | 対応 |
|---|---|---|---|
| Y1 | 救済の「元に戻さない」と共通の確定の「元に戻す」の矛盾 | 4.2（救済の表）、4.6（`Overwrite` の種類・S1''・S4・理由の表）、6 の 41 | 救済の `Overwrite` を共通の元に戻す規則から外した。SetText の成功の後の EndComposition の失敗は `WrittenNotEnded` で、元に戻さず再試行もしない |
| Y2 | 最後の表示どおりの確定の後の残り | 4.2（手順 5）、6 の 36 | 最後の試行も結果の表の遷移にした。成功しても救済全体の成功ではなく、残りは挿入せず記録と通知 |
| Y3 | 保存した範囲の確かめ方 | 4.2（救済の表の注記）、6 の 43 | 保存した範囲（`comp_end_anchor`）による挿入をやめた。位置を確かめられない文字は記録と通知 |
| Y4 | composition が最初から無い場合の位置 | 4.2（救済の表）、6 の 38 | 第 9 版: composition が無い場合は挿入しない（選択位置の継続を確かめられないため。マウスでの移動はキーの数に出ない）。`Stopped` として記録と通知 |
| 第 7 版の X2・X3 | 「確定済み・挿入待ち」と `anchor` | 4.2 | 第 8 版で削除した（不確かな位置への挿入と、結果の不確かな再試行をしない原則のため）。X1 の二重は、救済が書いた後に再試行しないことで防ぐ |
