# 設定の反映とホストの起動し直し（フェーズ 4: #64・#66・#49・#82）

作成: 2026-10-24
対象: [October_Plan.md](October_Plan.md) の 5 節（フェーズ 4）。Issue #64・#66・#49・#82。
根拠のコード: main `458027d`。

## 1. 目的

config.toml で変えた設定（モデル・backend・ログの水準を含む）が、ホストが動いている間も、次の変換から確実に反映されるようにする。そのために、1 つのホストの中では engine を作り直さず、設定が変わったらホストを起動し直す。

## 2. 決めたこと（2026-10-24）

1. 1 つのホストの中では engine DLL を替えない・外さない。engine も作り直さない。設定が違えば、ホストが終わり、TSF が新しいホストを起動する（#64 の方針。2026-10-23 決定を、モデルの変更にも当てはめる）。
2. モデルの変更（`model_variant`）は、1 の起動し直しで反映する（#49）。
3. `gpu_backend` を、TSF がホストに送る設定（EngineConfig の JSON）に加える。ホストはその値で engine DLL を選ぶ。`gpu_backend` だけの変更でもホストを起動し直す。
4. `log_level` も設定の JSON に加え、変わればホストを起動し直す（ホストの起動し直しの負荷は小さい）。
5. #66 の設定の版の照合（`config_version`）を、この段で入れる。config.toml が壊れている間も変換を続けられるように、ホストが最後に採用した設定を保存し、壊れている間はそれで起動する（4.3a）。config.toml が無いときは、既定値で採用する。待ちと上限の値は 7 章の案のとおり。
6. engine の側でも、変換の部品（converter）を使い回すときに、モデルにかかわる設定を照合する（守りの層。engine ABI は変えない）。
7. `Request::Reload` と、使われていない `Request::AvailableModelsJson` をなくす。プロトコルの版を 8 に上げる。
8. #82 は、モデルの定義を config.toml へ移さない（82-3・82-4 はやめる）。
   - 使えるモデルの一覧は、engine に埋め込む `crates/rakukan-engine/models.toml` だけに書く。モデルを足すときは models.toml に書いてビルドし直す（開発者がモデルを試すときも同じ）。
   - 選んでいるモデルは、config.toml の `model_variant` に書く（書き方は今のまま）。
   - 既定のモデルは `jinen-v1-xsmall-q5`。
   - 設定アプリのモデルの選択肢は、models.toml を読んで作る。
   - 使っていない `pre_tokenizer_override` を消す。
   - engine ABI は変えない（12 のまま）。

## 3. 今のコード（main `458027d`）

| 場所 | 今の動き | この設計で変える点 |
|---|---|---|
| ホストの `Create`（`crates/rakukan-engine-rpc/src/server.rs:727-738`） | エンジンがあって設定が同じなら `Unit`。違えば `load_engine_into` で、同じホストの中で作り直す。`config_version` は捨てる | 違えば作り直さず、終わる（4.2） |
| `load_engine_into`（server.rs:1599-1624） | `DynEngine::load_auto` → 辞書・モデルの読み込み開始 → `slot.engine = Some(eng)`（古いエンジンを落とす）→ `set_config` | エンジンが無いときだけ呼ぶ。DLL は JSON の `gpu_backend` で選ぶ |
| ホストの `Reload`（server.rs:739-751） | エンジンを落として作り直す。クライアントの `RpcEngine::reload()`（client.rs:788-799）の呼び出し元は 0 件 | なくす（4.4） |
| `ShutdownIfConfigDiffers`（server.rs:799-823） | `host_id` が違えば `ShutdownSkipped`。同じなら設定の文字列を比べ、違えば `Bool(true)` で終わる。`config_version` は捨てる | 版の照合を加える（4.3） |
| 応答の後の終了（server.rs:246-252, 580-618） | `ShutdownAccepted` と、条件付き終了の `Bool(true)` のときに終わる | `Create` の新しい応答でも終わる |
| クライアントの接続（client.rs:1527-1703） | 試行ごとに `config_source` から設定を取り直す（#65）。`Create` の応答は `Unit` 以外を失敗とし、`HostSpawnGuard` の失敗に数える。`config_version` は常に `None` | 新しい応答・拒否の扱いを加える（4.5） |
| ホストの多重起動 | `CreateMutexW` で 2 つ目のホストは起動しない（`crates/rakukan-engine-host/src/main.rs:180-224`） | 変えない |
| TSF の設定の JSON（`crates/rakukan-tsf/src/engine/config.rs:1005-1047` の `engine_config_json`） | `gpu_backend` と `log_level` を含まない | 2 つを加える（4.1） |
| TSF の設定の出どころ（config.rs:401-417） | `ConfigSource::File { bytes, sha256 }` で、本文のハッシュを持つ。既定値で始めたときは `Defaults`（ハッシュなし） | ハッシュを `config_version` として送る |
| DLL の選択（`crates/rakukan-engine-abi/src/lib.rs:529-533, 1099-1230`） | `load_auto` が config.toml を自分で読んで backend を決める | JSON の値で選ぶ（4.1） |
| engine の converter（`crates/rakukan-engine/src/ffi.rs:736-789`） | `try_reclaim_done()` で回収した converter を、設定と比べずに使う | モデルにかかわる設定を照合する（4.6） |
| models.toml（`crates/rakukan-engine/models.toml:1`） | `default_model = "jinen-v1-small-q5"`（配布する config.toml・ひな形・設定アプリ・CLI の説明は xsmall） | xsmall にする |
| 設定アプリ（`apps/rakukan-settings-winui/MainWindow.xaml:137-145`） | 8 項目を手で書いている | models.toml から作る（4.7） |

Issue の記述のうち、今のコードでは済んでいるもの:
- #66 の「再接続で保存済みの古い JSON を送る」: #65（`05c2ca3`）で、接続試行ごとに config.toml を読み直すようになった。
- #66 の「`ShutdownIfConfigDiffers` の再送で `host_id` を照合する」: `shutdown_target()`（client.rs:1152-1156）とホストの照合（server.rs:806-812）で済んでいる。
- #66 の `ConfigUnavailable` は、protocol.rs では `Reason::ConfigUnusable`（`Rejected` の理由。v7 では予約だけで、ホストは返さない）。
- #56（`host_id`・`engine_gen`・入力中の状態の復元）と #65 は実装済み。
- #64 のワーカーと DLL の寿命は、フェーズ 3 のピン止め（abi lib.rs:447-454, 1266-1298）で、DLL を外さないことで避けている。ピン止めはこの設計でも残す。

## 4. 設計

### 4.1 設定の JSON と DLL の選択

- `engine_config_json` に `gpu_backend`（`"auto"` / `"cuda"` / `"vulkan"` / `"cpu"`）と `log_level` を加える。EngineConfig は `#[serde(default)]` で、未知のキーを拒否しない（`crates/rakukan-engine/src/lib.rs:217-219`）ので、engine DLL は 2 つのキーを無視する。engine ABI は変わらない。
- 2 つのキーが JSON に入るので、変えれば `engine_json_changed=true` になり、反映待ち（`pending_apply`）が立つ（config.rs:684-700）。保存の通知・モードの切り替えの契機で `ShutdownIfConfigDiffers` が送られ、違えばホストが起動し直す。
- ホストの DLL の選択: `load_engine_into` は JSON の `gpu_backend` を読み、`load_with_selection`（abi lib.rs:1134-1180）に渡す。明示の指定は他へ切り替えず（今と同じ）、`auto` は cuda → vulkan → cpu の順に試す。JSON に `gpu_backend` が無い・読めないときは `auto`。`load_auto` が config.toml を自分で読む経路（`detect_backend`）は、ホストからは使わない。
  - 指定した backend を読み込めないときは、今と同じく `Create` が失敗する（TSF はエンジン無しとして扱い、`HostSpawnGuard` の失敗に数える）。
- `log_level`: 新しいホストを起動するときの `RAKUKAN_LOG`（client.rs:1809-1835 の `spawn_log_env`）は、今と同じく TSF の現在の設定から作る。ホストは JSON の `log_level` を照合にだけ使う。
  - 優先の順は今のまま: TSF のプロセスの環境に `RAKUKAN_LOG` が手で設定されていれば、それをホストに引き継ぎ、config.toml の `log_level` は使わない（client.rs:76-90）。この場合も `log_level` を変えればホストは起動し直すが、水準は手で設定した値のまま。
- 古いホスト・古い DLL との組み合わせ: TSF とホストと engine DLL は同じインストーラーで入れ替わり、完了は再起動なので、混ざるのは更新の後、再起動までの間だけ。プロトコルの版が違えば `Hello` で拒否される（server.rs:701-705。`Hello` は先頭の variant なので、版が違っても読める）。engine DLL は、ABI の版が同じ（12）なので、新しい JSON の 2 つのキーを無視する。

### 4.2 `Create` で設定が違うとき

ホストは、エンジンがあって設定が違う `Create` を受けたら、作り直さずに終わる。

1. 4.3 の版の照合が通ったときだけ、終わる（古い設定の `Create` でホストを止めない）。
2. 応答に、新しい `Response::CreateRestart`（仮の名前）を末尾に加える（postcard は宣言順の番号で符号化するので、追加は末尾。protocol.rs:4）。`exits_after_response` に加える。
3. **終わり方（ほかのセッションの要求を途中で切らない）**: 今の終了（server.rs:611-618）は、応答を書いてから 50ms 待って `process::exit` するだけで、ほかのセッションが処理中・送信中の要求の完了を待たない。ホストを終わらせる経路（`CreateRestart`・`ShutdownAccepted`・条件付き終了の `Bool(true)`）を、次の形にそろえる。
   1. 終わると決めたら、ホストは「終了中」になる。終了中に届いた新しい要求には、処理せず `Error("host exiting")` を返す（TSF は通信失敗として扱い、今の再接続の流れで新しいホストに繋がる。`Change` などは #56 の未解決の要求として、新しいホストで復元・断念する）。
   2. 処理中の要求（記録表の `in_flight`、server.rs:128-140, 504-520）が 0 になるか、上限（案: 2 秒）に達するまで待ってから終わる。上限に達したら WARN を出して終わる。
   3. 今の 50ms の待ち（応答がパイプで届くまでの待ち）は、最後に残す。
4. エンジンが無いときの `Create` は、今と同じく `load_engine_into` で作る。
5. 設定が同じなら `Unit`（今と同じ）。
6. **既知の影響**: ホストは全アプリで 1 つなので、設定の変更で起動し直すと、同じホストに繋がっているほかのアプリの接続も切れる。入力中の文字は #56 の復元で、切れている間に打ったキーはフェーズ 2 の保留キーで扱う。変換は、新しいホストがモデルを読み込むまで待たされる。

### 4.3 設定の版の照合（`config_version`、#66）

- TSF は、`Create` と `ShutdownIfConfigDiffers` に、送る JSON を作った読み取りの本文のハッシュ（`ConfigSource::File` の `sha256`。正規化前のバイト列から計算、config.rs:401-417）を `config_version` として載せる。既定値で始めた（`Defaults`）ときは `None`。
- ホストは、設定を採用するかを決めるとき（エンジンがあって設定が違う `Create`、エンジンが無い `Create`、設定が違う `ShutdownIfConfigDiffers`）にだけ、TSF と同じ場所の config.toml（`%APPDATA%\rakukan\config.toml`。TSF とホストは同じ利用者で動く）を読み、本文の SHA-256 を取る（TOML の解析はしない）。設定が同じ `Create` では読まない。読んだ結果は 3 つに分ける。
  - `Read(hash)`: 読めた。
  - `Missing`: ファイルが無い。
  - `Unreadable`: ほかの理由で読めない（書き込み中・権限など）。
- 照合:

  | TSF の `config_version` | ホストが読んだ結果 | 判定 |
  |---|---|---|
  | `Some(h)` | `Read(h)`（同じ） | 採用する |
  | `Some(h)` | `Read(h2)`（違う） | 古い（`ConfigStale`） |
  | `None`（既定値） | `Missing` | 採用する（既定値で動く） |
  | どれでも | `Read(h)` で、`h` が TSF の知らせた「壊れた本文のハッシュ」と同じ | 壊れている間の扱い（4.3a） |
  | `None`（既定値） | `Read(_)` | 古い（`ConfigStale`） |
  | どれでも | `Unreadable` | 判定しない（下の「一時的に読めない」） |
  | `Some(h)` | `Missing` | 古い（`ConfigStale`） |

  - `Create`: 古いときは何も変えない。エンジンがあれば、新しい応答 `Response::CreateKept`（ホストは今の設定のまま。下の「接続は続ける」）を返し、無ければ `Rejected(ConfigUnusable)` を返す。
  - `ShutdownIfConfigDiffers`: 古いときは `Rejected(ConfigStale)` を返し、終わらない。
  - 一時的に読めない（`Unreadable`）: 少し待って（50ms・3 回）読み直す。それでも読めなければ、`Create` はエンジンがあれば `CreateKept`、無ければ `Rejected(ConfigUnusable)`、`ShutdownIfConfigDiffers` は `Rejected(ConfigStale)` とし、何も変えない。
- **接続は続ける（`CreateKept`）**: エンジンがあるホストに古い設定（または壊れている間、4.3a）の `Create` が届いても、ホストの設定は照合で確かめた新しいもの（または壊れている間の、最後に採用したもの）なので、そのホストに繋いで使うのが正しい。そこで `CreateKept` を、接続の成功として扱う。
  - クライアントは `HostSpawnGuard` の失敗に数えず、ストリームを使い続ける。
  - 「`Create` が成功した組」（client.rs:1730 の `created_with`）には、送った組ではなく「ホストの設定のまま」と記録する（ログで分かるように）。
  - TSF の反映待ちは残す。次の契機で、読み直した組の `ShutdownIfConfigDiffers` を送る。TSF の組がディスクと一致していれば、ホストの設定と比べて、違えば起動し直す。
  - 旧来の `rejection_may_have_applied`（client.rs:231-241）は `Change` などの拒否の判定なので、`Create` の応答には使わない。
- **エンジンが無いホストでの拒否（`ConfigUnusable`）**: TSF は config.toml を読み直してから 1 回だけ送り直す。
  - 送り直しても拒否なら、その試行は終える。ただし `HostSpawnGuard` の失敗に数えない（ホストは生きている）。WARN を 1 回出し、反映待ちを残す。
  - その間はエンジン無しとして扱う（候補ウィンドウの状態行は、モデル未準備と同じ仕組みで「設定を読めない」と出す）。打ったキーは、エンジン未準備と同じく保留キーの仕組みで扱う。
  - 次の試行は、今の再接続の契機（次のキー・変更の通知・定期の確認（#65）・背景の接続確認）で始める。これで、無限の送り直しにも、黙ったままの放置にもならない。
- **`ShutdownIfConfigDiffers` の `ConfigStale`**: TSF は読み直してから 1 回だけ送り直す。それでも `ConfigStale` なら、反映待ちを残して、次の契機を待つ。無条件の `Shutdown` には流さない（`request_config_apply`、state.rs:467-506 の Err の扱いと分ける）。
### 4.3a config.toml が壊れている間

TSF は、config.toml を読めても設定として解析できない（TOML の誤り・型の誤りなど）と、直前の正常な設定を使い続ける（#61）。そのハッシュはディスクの本文と合わないので、4.3 の照合だけでは、壊れている間は新しいホストを起動できない。かといって、TSF の直前の設定をそのまま受け入れると、アプリごとに持っている「直前の設定」が違うことがあるので、起動し直すたびに設定が行き来する。そこで、ホストが最後に採用した設定を 1 つだけ保存し、壊れている間はそれを使う。

- **保存**: ホストは、設定を採用してエンジンを作るたびに、`%LOCALAPPDATA%\rakukan\host-config-last.json` に書く（一時ファイルに書いてから名前を替える）。書けなくても WARN だけで続ける。ホストは多重起動しない（3 章）ので、書くのは 1 つのホストだけ。
  - 形: `{ "format": 1, "config_json": "<EngineConfig の JSON>", "config_sha256": "<採用した config.toml の本文のハッシュ>", "json_sha256": "<config_json のハッシュ>" }`。
  - 読むとき: `format` が 1 で、`json_sha256` が `config_json` のハッシュと合い、`config_json` が JSON として読めるときだけ使う。合わない・読めないときは WARN を出して使わず、既定値（JSON 無し）にする。
  - 信頼の範囲: このファイルは config.toml と同じく利用者が書き換えられる場所にある。ハッシュは壊れの検出のためで、改ざんを防ぐものではない（config.toml と同じ信頼の範囲として扱う）。
- **TSF の知らせ**: TSF は、直近の読み込みで本文を読めたが設定として解析できなかったとき（UTF-8 として読めない・TOML の誤り・型の誤り）、その回に読めた本文のハッシュを `broken_version` として `Create` と `ShutdownIfConfigDiffers` に載せる（新しいフィールド。正常に読めているとき・ファイルを読めなかったときは `None`）。ホストは TOML を解析しない（TSF と同じ基準で「壊れている」を判断するため）。
  - そのために、`ConfigManager` の読み込みの失敗の記録を変える。今は `read_and_build`（config.rs:561-575）が読み取りの失敗と解析の失敗を同じ `Err` で返し、`note_failure` / `last_load_failure`（config.rs:504-522, 657-671）はエラーの文字列だけを持つ。これを、失敗の種類（読めない／解析できない）と、解析できなかったときにその回に読めた本文のハッシュを持つ形にする。壊れた本文が別の壊れた本文に替わったときも、ハッシュが替わるように、毎回の読み込みで更新する（WARN を繰り返さない今の扱いは、エラーの文字列とは別に残す）。
- **ホストの判断**: ディスクの本文のハッシュが、TSF の `broken_version` と同じなら「壊れている」とみなす。
  - エンジンがある `Create`: 設定を比べず、今のエンジンを使い続ける（`CreateKept`。4.3 の「接続は続ける」と同じ扱い）。ホストは終わらない。
  - エンジンが無い `Create`: 保存してある設定（`host-config-last.json`）でエンジンを作る（`CreateKept`）。保存が無い・使えないときは既定値（JSON 無し）で作る。TSF の送った JSON は使わない。ログに INFO で「config.toml が壊れているので、最後に採用した設定で起動した」と出す。
  - `ShutdownIfConfigDiffers`: 終わらない。`Rejected(ConfigStale)` を返す（TSF は反映待ちを残す）。
  - どのアプリから繋いでも、ホストの設定は 1 つ（保存した設定）なので、アプリごとに設定が行き来しない。TSF は `CreateKept` で、ホストの設定が自分の組と違うかもしれないことを知る（ログに残る）。
- **直ったとき**: TSF は次の通知か定期の確認（#65）で読み直し、正常な設定で反映待ちを立てる。`ShutdownIfConfigDiffers` は通常の照合に戻り、設定が違えばホストが起動し直す。
- 壊れている間に TSF の候補ウィンドウの状態行は変えない（今も #61 の WARN をログに出している）。変換は続けられる。

### 4.3b 照合で防げる順序

- これで次の順序が防げる。
  - 古い設定のプロセスの `Create` が先に着いて、新しいホストが古い設定で起動する。
  - 送信の時点で固定した古い設定 A の `ShutdownIfConfigDiffers` が、再接続で新しい設定 B を `Create` したばかりのホストを止める（#66 の 2026-10-09 のコメント）。

### 4.4 `Reload` をなくす

- ホストの `Request::Reload` の処理と、クライアントの `RpcEngine::reload()` を消す。
- 通信の上の `Request` は、variant の並びで符号化しているので、今までの決まり（`_Reserved…`）に合わせて `_ReservedReload` として位置を残し、届いたら `Error("unsupported")` を返す。
- 使われていない `Request::AvailableModelsJson`（protocol.rs:474、server.rs:1913、client.rs:1088）も同じく `_ReservedAvailableModelsJson` にし、ホストとクライアントの処理を消す。位置を残すのは `Request` だけ（応答は既存の `Response::String` を使っていて、専用の `Response` の variant は無い）。engine DLL の関数 `engine_available_models_json` は残す（消すと engine ABI が変わる。CLI は engine を直接使っている）。
- プロトコルの版を 8 にする（`Response::CreateRestart` と `Response::CreateKept` の追加、`Create` / `ShutdownIfConfigDiffers` の `broken_version`、`config_version` の照合、`Reload` と `AvailableModelsJson` の廃止）。`protocol.rs` の版の履歴のコメントに書く。

### 4.5 TSF とクライアントの扱い

- `handshake_create`（client.rs:1690-1703）が `CreateRestart` を受けたら、その試行は「失敗」ではなく「ホストが起動し直す」として終える。`HostSpawnGuard` の失敗に数えない。ストリームは捨てる。
- **古いホストが終わるのを待ってから、次の試行をする。** 古いホストが終わる前に次の試行をすると、次のどちらかになる。
  - パイプに繋がって、終了中の古いホストから `Error("host exiting")` が返る。
  - 繋がらずに spawn した新しいホストが、古いホストがまだ持っている多重起動防止のミューテックス（main.rs:180-224）を見て、すぐ終わる。
  そのため、`CreateRestart` を受けたクライアントは、古いホストのパイプに繋がらなくなるまで（案: 100ms ごとに、最長 3 秒）待ってから、通常の試行（connect → spawn → Hello → Create）に進む。
  - 待っている間の `host exiting` と接続の失敗は、`HostSpawnGuard` の失敗に数えない。
  - 3 秒を超えたら、通常の試行に進む（そこでの失敗は今までどおり数える）。
  - 新しいホストが立ち上がった後の `Create` で、また `CreateRestart` が返った（設定がさらに変わった）場合も同じ扱いにし、続けて 3 回を上限にする。上限を超えたら WARN を出して、通常の失敗として数える。
- ほかのアプリの TSF は、`CreateRestart` を受けていないので、次の要求で接続の失敗（または `host exiting`）を受け、今の再接続の流れで新しいホストに繋がる。spawn が古いホストのミューテックスとぶつかって失敗した場合は、今の `HostSpawnGuard` の数え方のまま（3 回で 15 秒の抑止に入らないかを 5 章の試験で確かめる）。
- `ShutdownIfConfigDiffers` の結果（`ConditionalShutdown`、client.rs:343-352）に `Stale` を加える。
- 起動し直しの間に打ったキーは、フェーズ 2 の保留キーの仕組み（`key_queue`）で扱い、入力中の文字は #56 の復元で戻す。新しい仕組みは足さない。試験で確かめる（5 章）。

### 4.6 engine の converter の照合（守りの層）

- `engine_start_load_model`（ffi.rs:736-789）で、`conv_cache::try_reclaim_done()` が返した converter を使う前に、その converter を作ったときのモデルの設定と、今のエンジンの設定を比べる。違えば捨てて、読み込み直す。
- 比べるのは、`build_converter`（lib.rs:766-796）が使う項目: `model_variant`（未指定なら既定）、`n_gpu_layers`、`main_gpu`、`n_threads`、`convert_beam_size`、`confidence_margin`、`min_top_confidence`、`force_inference_failure`。
- converter を作ったときの設定は、DLL の中の static に、converter と組にして持つ（engine ABI は変えない）。
- 4.2 で同じホストの中の作り直しが無くなるので、通常の流れではこの照合は一致する。違ったときは INFO を出す。
- `has_converter()` が true（converter が BG の変換に出ている: pending / Running）のときは、今のまま何もしない。変換が終わって Done に戻った後の次の `engine_start_load_model` で、上の照合を通る。処理中の converter を途中で捨てることはしない。
- `pre_tokenizer_override` は 4.7 で消すので、照合の項目には入れない（今も本番の経路では使っていない）。

### 4.7 モデルの一覧と既定（#82）

- `crates/rakukan-engine/models.toml` の `default_model` を `jinen-v1-xsmall-q5` にする。`model_config.rs` のテスト（既定を固定値で確かめている）を直す。
- `pre_tokenizer_override` を消す: `model_config.rs:25` の項目、`llamacpp.rs:157` の `from_file_with_pre_tokenizer_override`。
- 設定アプリ:
  - 配置: `apps/rakukan-settings-winui/Rakukan.Settings.WinUI.csproj` に、`..\..\crates\rakukan-engine\models.toml` を `Content`（`CopyToOutputDirectory=PreserveNewest`、出力のファイル名は `models.toml`）として加える。今の csproj には無い。インストーラーは設定アプリの出力のフォルダを `settings-ui\` に入れているので、`settings-ui\models.toml` として入る（`scripts/build-installer.ps1` の設定アプリの写しと `rakukan_installer.iss` の `[Files]` で確かめる。足りなければ加える）。
  - 読む場所: 設定アプリの実行ファイルのフォルダ（`AppContext.BaseDirectory`）の `models.toml`。
  - 読み方（Tomlyn。すでに使っている）: models.toml の形は、先頭の `default_model = "<id>"` と、`[models.<ファミリー>]`（`repo_id`・`display_name`）の下の `[models.<ファミリー>.variants.<名前>]`（`id`・`filename`・`display_name`）。各 variant の `id` と `display_name` を、ファイルに書かれた順に選択肢にする。`id` が無い variant は飛ばす。`display_name` が無ければ `id` を出す。`default_model` は、何も選ばれていないときの薄い字に使う。
  - 読めないとき（ファイルが無い・TOML として壊れている）は、選択肢を空にし、名前を手で書けるようにする（今の欄は手で書ける）。設定アプリのログに WARN を出す。
  - config.toml の `model_variant` が一覧に無い値でも、その値を欄に出して保存できるようにする（今と同じ）。
  - `MainWindow.xaml` に手で書いた 8 項目を消す。
- config.toml のコメントのモデルの一覧（`config/config.toml:31-39` と、ひな形の config.rs:1077-1086）を消し、「設定アプリで選ぶ。一覧は models.toml」と書く。
- `AvailableModelsJson` の RPC は消す（4.4）。
- `scripts/refresh-models.ps1` は今のまま（models.toml を対象にしている）。

### 4.8 レビュー（2026-10-24、Codex CLI）

2 回目のレビューで、次を直した: 古い設定・壊れている間の `Create` で、エンジンがあるのに接続が失敗扱いになり、そのアプリだけ変換できなくなる（`CreateKept` を加えた。4.3）、壊れている間に TSF が「送った組で成功した」と記録する食い違い（同）、`broken_version` を作るための失敗の記録（4.3a）、保存するファイルの形と検証（4.3a）、`AvailableModelsJson` の位置を残すのは `Request` だけ（4.4）。

設計の最初の版へのレビューで、次を直した: ホストの終了でほかのセッションの要求を途中で切る（4.2 の 3）、版の照合で「既定値」と「読めない」を同じ `None` にしていた（4.3 の表）、`CreateRestart` の後に古いホストのミューテックスとぶつかる（4.5）、`RAKUKAN_LOG` を手で設定したときの優先（4.1）、設定アプリの models.toml の配置と読み方（4.7）、BG に出ている converter の扱い（4.6）、実装の順序（6 章）、試験の追加（5 章）。

## 5. 試験

### 5.1 単体の試験

- ホストの `Create` の分岐を、DLL を読み込まずに試せるように、`load_engine_into` の前で判断する部分を純粋な関数に切り出す（エンジンの有無、設定が同じか、版の照合の結果、壊れているか → `Unit` / 作る / 保存した設定で作る / `CreateRestart` / `CreateKept` / `ConfigUnusable`）。
- 版の照合: 本文のハッシュの一致・不一致・ファイルが無い・既定値（`None`）。
- `exits_after_response` が `CreateRestart` で終わること。
- クライアント: `CreateRestart` を失敗に数えないこと、上限を超えたら数えること、`ConfigUnusable` と `ShutdownIfConfigDiffers` の `ConfigStale` で 1 回だけ送り直すこと、`ConfigStale` が無条件の `Shutdown` に流れないこと。
- `engine_config_json` に `gpu_backend` と `log_level` が入り、変えると `pending_apply` が立つこと。
- engine の converter の照合（一致なら使う、違えば捨てる、BG に出ている間は何もしない）。
- models.toml の既定が xsmall であること。
- ホストの終了中: 新しい要求に `host exiting` を返すこと、処理中の要求の完了を待つこと、上限で終わること（終了の判断と待ちを、プロセスを終わらせずに試せる形に切り出す）。
- 版の照合の表の全部の行（`Unreadable` の読み直しを含む）。
- 壊れている間（4.3a）: `broken_version` が一致するときの `Create`（エンジンあり・なし）と `ShutdownIfConfigDiffers`、保存した設定の書き込み・読み込み（無い・`json_sha256` が合わない・JSON として読めない）。TSF の `broken_version` が、解析の失敗のときだけ付き、読めないときは付かないこと。壊れた本文が別の壊れた本文に替わると、`broken_version` も替わること。
- `CreateKept`: 古い設定の `Create` でも接続が続き、`HostSpawnGuard` の失敗に数えず、反映待ちが残ること。`ConfigUnusable` の後に、次の契機で接続し直すこと。
- 既存の試験の期待値の直し: `engine_json_is_pure_and_matches_snapshot`・`changed_engine_json_sets_pending_apply`（config.rs）など、JSON の中身を見ている試験。
- `CreateRestart` の後の待ち: 古いホストが残っている間の `host exiting`・接続失敗を数えないこと、3 秒で通常の試行に進むこと、続けて 3 回の上限。

### 5.2 実機の試験

1. モデルの変更: 設定アプリで `model_variant` を変えて保存 → 次の変換で新しいモデル（ホストのログの `engine::init: loading model=…`、新しい `host_id`）。戻す。
2. backend の変更: `gpu_backend` だけを変えて保存 → 新しいホストが指定の DLL を読む。変換の速さが、その backend だけのホストと同じ。戻す。
3. `log_level` の変更 → ホストが起動し直し、新しい水準でログを書く。戻す。
4. 候補数などの変更 → ホストが起動し直す（同じホストの中の作り直しが出ない）。
5. 複数のアプリ（メモ帳・EmEditor・Firefox）で入力しながら保存 → どのアプリも変換を続けられる。ホストの起動し直しは 1 回。
6. 入力中（未確定の文字がある）に保存 → 文字が失われない。
7. config.toml を壊して保存 → 動いているホストは変換を続ける。言語バーの「エンジン再起動」でホストを起動し直す → 最後に採用した設定（`host-config-last.json`）で起動し、変換できる。直して保存 → 直した設定でホストが起動し直す。
8. 設定アプリのモデルの選択肢が、models.toml の 8 種類と同じ。
9. 長い読みの変換（Space）の最中に、別のアプリで設定を保存 → 変換は完了するか、切れても再接続で回復する（処理中の要求の完了を待つ）。
10. 保存を短い間に 2 回続ける（設定アプリで 2 回保存）→ ホストの起動し直しが続いても、最後の設定で落ち着く。`HostSpawnGuard` の抑止（`host spawn suppressed`）に入らない。
11. 起動し直しの直後、まだ古い設定を読んでいるアプリがある状態（保存の直後に、別のアプリで変換）→ 古い設定でホストを止めない（ホストのログに、古い設定の `Create` に `CreateKept`、または `ShutdownIfConfigDiffers` に `ConfigStale` を返した記録）。

## 6. 実装の段取り（tque #106 を分ける案）

先に通信とホストの側を揃え、TSF の送り直し・状態の扱いを入れてから、JSON の新しいキーで起動し直しの契機を増やす（JSON を先に入れると、`gpu_backend` の変更で同じホストの中の作り直しが増え、#64 の経路を通る）。

1. プロトコルの版 8: `CreateRestart` の追加、`Reload` を `_ReservedReload` に、ホストの終了中の扱いと処理中の要求の待ち（4.2 の 3、4.4）
2. ホストの版の照合（4.3）、壊れている間の扱いと保存（4.3a）、`Create` で設定が違うときに終わる（4.2）
3. クライアントと TSF: `CreateRestart` の後の待ち、`CreateKept` を接続の成功として扱う、`ConfigStale` / `ConfigUnusable` の送り直しと状態行、`broken_version` と失敗の記録（4.3・4.3a・4.5）
4. 設定の JSON に `gpu_backend`・`log_level` を加え、ホストは JSON で DLL を選ぶ（4.1）
5. engine の converter の照合（4.6）
6. models.toml の既定・`pre_tokenizer_override`・設定アプリの選択肢・config.toml のコメント（4.7）
7. DESIGN.md・CHANGELOG の更新

## 7. 決めた値と扱い（2026-10-24）

1. config.toml が壊れている間: ホストが最後に採用した設定で動く（4.3a）。
2. config.toml が無いとき: TSF が既定値（`None`）で、ホストが `Missing` なら採用する（既定値で動く）。`Unreadable` は一致とみなさない。
3. 待ちと上限の値: 終了のときの処理中の要求の待ちは 2 秒、`CreateRestart` の後の待ちは 100ms ごとに最長 3 秒、続けての `CreateRestart` の上限は 3 回、`Unreadable` の読み直しは 50ms・3 回。
4. `AvailableModelsJson` の RPC は消す（4.4）。
