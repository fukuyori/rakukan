# Changelog

<!-- markdownlint-disable MD024 -->
<!-- MD024: Keep-a-Changelog 形式では各バージョンで ### Added/Changed/Fixed が繰り返されるため無効化 -->

## [0.7.7] - 2026-04-29

### Changed

- **ライブ変換セッション状態の集約 — Phase 2** (M4 / T2 段階 c の後半) — TSF cross-thread を含むグローバル状態を `LiveShared` 構造体に集約。**動作変更なし** (純粋リファクタ、既存挙動を完全保持):
  - 集約対象 4 種:
    - 旧 `LIVE_PREVIEW_QUEUE` (`LazyLock<Mutex<Option<PreviewEntry>>>`) → `LiveShared.preview_queue`
    - 旧 `LIVE_PREVIEW_READY` (static `AtomicBool`) → `LiveShared.preview_ready`
    - 旧 `SUPPRESS_LIVE_COMMIT_ONCE` (static `AtomicBool`) → `LiveShared.suppress_commit_once`
    - 旧 `LIVE_CONV_GEN` (static `AtomicU32`) → `LiveShared.conv_gen`
  - 個別の sync primitive (Atomic / 個別 Mutex) は据え置き — `Mutex<LiveShared>` で全状態を一括包むと、`COMPOSITION_APPLY_LOCK` や engine ロックとの順序関係が複雑化し、`try_apply_phase1a` 内で EditSession コールバックが走る間ロックを保持してしまう罠が出るため。構造体は名前空間として機能し、helper 関数で更新を集約 (Phase 1 の thread_local 集約と同じ流儀)
  - 公開 helper:
    - キュー: `queue_preview_set(entry) -> bool` / `queue_preview_consume() -> Option<PreviewEntry>` / `queue_preview_clear()`
    - 抑制: `suppress_commit_arm()` / `suppress_commit_clear()` / `suppress_commit_take() -> bool`
    - 世代: `conv_gen_bump()` / `conv_gen_snapshot() -> u32`
  - callsite (14 箇所) を helper 経由に置換: `queue_phase1b` / `dispatch` の Phase1B 消費 / `on_input` x2 (clear) / `on_convert` x4 (clear / commit fallback / cancel) / `edit_ops` x2 (arm) / `on_input` x2 + `on_convert` x1 (gen bump) / `candidate_window` x2 (gen snapshot)
  - `PreviewEntry` 定義も `tsf::live_session` 配下に移設 (旧 `engine::state::PreviewEntry`)

### Added

- **M2 §5.3 `session_nonce`** (composition 開始ごとの identity 識別子) — Phase 1B キューの stale 判定を世代 (`gen_when_requested`) + reading + **session_nonce** の三重防壁にして、composition が破棄→再生成された後に古い preview がキューに残って次の composition に紛れ込む経路を断つ:
  - `LiveShared.session_nonce: AtomicU64` 追加。`composition_set_with_dm(Some(...), _)` 経路で `session_nonce.fetch_add(1, Release)` を実行 (3 callsite — `StartComposition` 成功直後)
  - `PreviewEntry` に `session_nonce_at_request: u64` フィールド追加。`queue_phase1b` で要求時のスナップショットを格納
  - `dispatch` の Phase1B 消費時に現在 nonce と比較し、不一致なら `discarded stale preview entry_nonce={} cur_nonce={} ...` ログを出して破棄
  - これまでは `gen` + `reading` の二重防壁だった (M1.8 T-MID1)。`session_nonce` は composition 単位の identity を加え、reading が偶然一致する場合の race も塞ぐ
  - 公開 helper: `session_nonce_advance()` / `session_nonce_snapshot() -> u64`

## [0.7.6] - 2026-04-29

### Changed

- **ライブ変換セッション状態の集約 — Phase 1** (M4 / T2 段階 c の前半) — TSF スレッドローカルに閉じる 5 種のグローバル状態を `LiveConvSession` 構造体に集約。**動作変更なし** (純粋リファクタ):
  - 新ファイル `crates/rakukan-tsf/src/tsf/live_session.rs` を追加。`LiveConvSession` 構造体 + `TL_LIVE_SESSION: thread_local RefCell<...>` を定義
  - 集約対象 5 種:
    - 旧 `TL_LIVE_CTX` (`RefCell<Option<ITfContext>>`) → `LiveConvSession.ctx`
    - 旧 `TL_LIVE_TID` (`Cell<u32>`) → `LiveConvSession.tid`
    - 旧 `TL_LIVE_DM_PTR` (`Cell<usize>`) → `LiveConvSession.composition_dm_ptr`
    - 旧 `LIVE_TIMER_FIRED_ONCE_STATIC` (static `AtomicBool`) → `LiveConvSession.fired_once`
    - 旧 `LIVE_LAST_INPUT_MS` (static `AtomicU64`) → `LiveConvSession.last_input_ms`
  - `LIVE_DEBOUNCE_CFG_MS` は設定値 (live_input_notify から書き込み、on_live_timer から読み込み) のため static のまま残す ([ROADMAP §7](docs/ROADMAP.md#L1191) のスペック通り)
  - 公開 helper: `set_context_snapshot(ctx, tid, dm_ptr)` / `clear_context_snapshot()` / `context_snapshot() -> (Option<ITfContext>, u32, usize)` / `invalidate_dm_ptr(dm_ptr) -> bool` / `swap_fired_once(new) -> old` / `reset_fired_once()` / `store_last_input_ms(now_ms)` / `load_last_input_ms() -> u64`
  - candidate_window.rs の callsite (8 箇所) を helper 経由に置換: `live_input_notify` (set_context_snapshot + reset_fired_once + store_last_input_ms) / `stop_live_timer` (clear_context_snapshot) / `pass_debounce` (load_last_input_ms) / `fetch_preview` (reset_fired_once) / `ensure_bg_running` (swap_fired_once) / `try_apply_phase1a` (context_snapshot) / `invalidate_live_context_for_dm` (invalidate_dm_ptr)
  - **Phase 2 (v0.7.7 で予定)**: cross-thread を含む状態 (`LIVE_PREVIEW_QUEUE` / `LIVE_PREVIEW_READY` / `SUPPRESS_LIVE_COMMIT_ONCE` / `LIVE_CONV_GEN`) を吸収。M2 §5.3 `session_nonce` (composition 開始ごとの identity) も同タイミングで追加

## [0.7.5] - 2026-04-29

### Fixed

- **WinUI 設定 UI で保存した `config.toml` の改行コードが LF になっていた** — `Tomlyn.Toml.FromModel(...)` の出力は LF 単独のため、Windows 既定の CRLF にならず、既存 CRLF ファイルへ書き出すと「最初の数行だけ CRLF、それ以降は LF」のような混在状態が発生していた。`SettingsStore.WriteIfDifferent` と `EnsureFile` に `NormalizeToCrlf` ヘルパーを挟み、書き出し前に CRLF に統一。比較も正規化後の文字列で行うため、CRLF→CRLF の冪等書き込みを spurious change と誤判定しない

### Changed

- **`factory.rs` を 6 ファイルに分割** (M3 T1-A) — 4816 行の god file を機能別に切り出し、可読性と保守性を向上。**ロジック変更なし** (純粋切り出し、関数本体は完全に同一)。impl ブロックは inherent impl として子モジュールで `impl super::TextServiceFactory_Impl { pub(super) fn ... }` のスタイルで分割。各メソッドは `pub(super)` で兄弟モジュールから呼び出し可能:
  - `factory.rs` 1421 行 (核: COM impl / langbar / key event sink / 構造体定義 / Activate/Deactivate / 自由関数ヘルパー)
  - `factory/dispatch.rs` 375 行 (`handle_action`: ユーザアクションを各 on_* へ振り分ける dispatcher)
  - `factory/on_input.rs` 396 行 (`on_input` / `on_input_raw` / `on_full_width_space` / `prepare_for_direct_input`)
  - `factory/on_convert.rs` 1170 行 (`on_convert` / `on_commit_raw` / `on_backspace` / `on_cancel`)
  - `factory/on_compose.rs` 637 行 (composition の EditSession ヘルパー: `update_composition` / `commit_then_start_composition` / `update_composition_candidate_parts` / `end_composition` / `commit_text` / `update_caret_rect` / キャレット/range 取得 (`get_caret_pos_from_context` / `get_cursor_range` / `get_insert_range_or_end` / `get_document_end_range`) / `set_display_attr_prop`)
  - `factory/edit_ops.rs` 952 行 (F6-F10 のかな/英数変換 / `on_cycle_kana` / 候補ナビ (`on_candidate_move` / `on_candidate_page` / `on_candidate_select`) / IME トグル (`on_ime_toggle` / `on_ime_off` / `on_ime_on`) / モード切替 (`on_mode_hiragana` / `on_mode_katakana`) / 文節操作 (`on_segment_*`) / `on_punctuate`)
  - 可視性の調整: `enum CandidateDir`, `loading_indicator_symbol`, `action_name` を `pub(super)` に変更 (子モジュールから参照するため)
- **`on_live_timer` を 6 サブ関数に分解** (M2 §5.1 / T1-B) — 298 行の god function を機能別に分割し可読性を向上。**動作変更なし** (純粋分解、ロック取得順序も保持):
  - `pass_debounce()` — `LIVE_DEBOUNCE_CFG_MS` 経過チェック (None なら早期 return)
  - `probe_engine(elapsed)` — engine ロック取得 + `hiragana_text` / `bg_status` 取得 + 「FIRED ...」ログ。busy=continue / no-preedit=stop_live_timer
  - `ensure_bg_running(&probe)` — bg=done を確認、idle なら `bg_start` 自己起動 (kanji_ready 判定込み)、running は wait
  - `fetch_preview()` — `bg_peek_top_candidate` で取得 + `sanity_check_preview` (T-BUG2 防壁)
  - `build_apply_snapshot(data)` — `display_shown = preview + pending` 組み立て
  - `try_apply_phase1a(&snapshot)` / `queue_phase1b(&snapshot)` — `RequestEditSession` or `LIVE_PREVIEW_QUEUE` 経由
  - orchestrator 本体は 16 行に縮小し、各段の責務を `let-else` で素直に並べる

### Added

- **`bg_peek_top_candidate` API を新設** (M2 §5.2) — ライブ変換 preview のために conv_cache を**非破壊**に覗き見る経路を追加。従来 `bg_take_candidates` は preview / commit の両方で使われ、毎回 cache を空にして converter を engine に戻し user dict マージまで実行していた。peek/take 分離後:
  - **preview** (`fetch_preview`) → `bg_peek_top_candidate(key)` を呼ぶ。Done state はそのまま、user dict マージなし、トップ候補だけ String で返す
  - **commit / Space 変換** (`bg_take_candidates`) → 従来通り converter を engine に戻し、user dict マージして全候補を返す
  - **converter の auto-reclaim** — preview で take しなくなる代わりに、次の `bg_start` 内で `conv_cache::try_reclaim_done()` (既存、lib.rs:603) が Done state から converter を回収するため、engine.kanji の空状態は問題にならない
  - 実装は engine / engine-host / RPC の **out-of-process 構成のため 5 層** に追加: `conv_cache::peek_top_candidate` / `RakunEngine::bg_peek_top_candidate` / `engine_bg_peek_top_candidate` (FFI) / `DynEngine::bg_peek_top_candidate` (engine-abi) / `Request::BgPeekTopCandidate` (RPC) / `RpcEngine::bg_peek_top_candidate` (client)
  - サーバ側で空文字列を返した場合は `RpcEngine` 側で `None` に正規化し、TSF からは `Option<String>` として扱う
- **install/build 手順誤案内を防ぐ Stop hook** — Claude Code 用の `.claude/settings.json` に Stop hook を追加し、AI アシスタント (Claude) が `cargo make install` を案内しているのに直前に `cargo make build-tsf` / `cargo make build-engine` の案内が無い場合、または「install 後にサインアウト」のような誤った順序を書いた場合に block して再考を促す。検査スクリプトは `scripts/check-install-instruction.ps1` (PowerShell)。CLAUDE.md に正しい手順 (sign-out → sign-in → build → install) は明記済みだが、案内のたびに見落とすケースがあったため構造的に止める仕組みを入れる

### Deferred

- **M2 §5.3 (`session_nonce` で stale 結果 discard)** を v0.7.6 (M4 LiveConvSession 集約) に繰り延べ — 観測された具体的 bug がなく、M1.8 既存防壁 (T-MID1 gen / T-MID2 stale check / T-MID3 SetText 排他) で race の大半をカバー済み。M4 で `LiveConvSession` 構造体を新設するときに nonce をメンバとして自然に組み込める

## [0.7.3] - 2026-04-28

### Fixed

- **ライブ変換 preview の尻切れをエンジン側で部分抑制** (M1.5 T-BUG1 a + c) — jinen LLM が reading を使い切る前に EOS を出して preview が極端に短くなる現象 (例: `じけいれつでーたのことをさしつづいた…` → `時系列データのことをさ` で停止) に対し、副作用のない 2 段の対策を投入:
  - **(a) `generation_budget` の上限 128 → 256** ([backend.rs:32-43](crates/rakukan-engine/src/kanji/backend.rs#L32-L43)) — 20 文字超の長文 reading で budget が頭打ちになる前に EOS が出るパターンを抑止。KV cache は変換時のみ確保するためメモリ圧は無視できる
  - **(c) 出力 candidates のエンジン側フィルタ** ([backend.rs:259-264](crates/rakukan-engine/src/kanji/backend.rs#L259-L264)) — `c.chars().count() * 3 < reading.chars().count()` の候補を破棄。全滅なら reading をそのまま返す。session に短い preview が入らず、後段の sanity check に依存しない
  - 本命の (b) `min_new_tokens` 機構 (greedy で premature EOS を次点 non-EOS トークンへ差替え / beam search で premature EOS の beam を candidates から落とす) は実装したが、トークン単位の min 判定が char 単位の reading 長と整合せず、適切に EOS した変換でも次点トークン (jinen では多くの場合 `〜`) を強制挿入する regression が観測されたため**同バージョン内で revert**。例:

    ```text
    reading="がひょうじ"        preview="が表示〜"   ← 〜混入
    reading="がひょうじされる"  preview="が表示される〜る" ← 〜混入
    ```

    本命の長文尻切れ修正は、`llama-cpp-2` の logit bias API が整備された段階で再設計する。当面は 0.7.0 の TSF 側 T-BUG2 (preview 30% 未満破棄) と (c) の二重防壁で対応
- **ライブ変換中の中間文字消失への追加防壁** (M1.8 T-MID2) — `update_composition` / `update_composition_candidate_parts` の EditSession クロージャ先頭で `composition_clone()` を再呼出し、外側 snapshot のポインタと比較。`OnUninitDocumentMgr` などで composition が破棄/置換された後に deferred EditSession が誤書き込みする経路を塞ぐ。不一致なら no-op + log
- **ライブ変換中の SetText 二重適用の race 対策** (M1.8 T-MID3) — `state.rs` に `COMPOSITION_APPLY_LOCK: LazyLock<Mutex<()>>` を追加し、Phase1A (`candidate_window.rs` の live preview SetText) / `update_composition` / `update_composition_candidate_parts` の `SetText` を `try_lock` で囲む。busy なら skip して `Ok(())` で抜け、最新 gen による次回 SetText が勝つ。0.7.0 の T-MID1 gen 機構と組合せて二重 apply 経路を堅牢化

### Documentation

- **テストの矛盾を解消** — 以下のいずれも v0.7.3 の修正範囲外で v0.6.x 以前から壊れていたものを v0.7.3 リリース時に整合化:
  - `kanji::model_config::tests::test_all_variant_ids` / `test_iter_variants` が variant 数 2 を仮定していたが、v0.6.x で f16 variant 追加後は xsmall-q5 / small-q5 / xsmall-f16 / small-f16 の計 4 になっていたためアサーションを更新
  - `engine::text_util::tests::katakana_symbols_fullwidth` / `hiragana_symbols_fullwidth` が `"\\x5C"` を「backslash 1 文字」と書いていたが、Rust の文字列リテラルでは `\`, `x`, `5`, `C` の 4 文字。意図通りの 1 文字 backslash になる `"\x5C"` に修正
- **`backend::tests::test_env_override_cpu` が並列実行で flaky** — `RAKUKAN_GPU_BACKEND` env 変数を別テストとシェアするため `cargo test --workspace` で稀に失敗する。`cargo test -- --test-threads=1` で確実に通る。本リリースでは未対応 (test-only の問題)

## [0.7.2] - 2026-04-28

### Fixed

- **`engine_reload` 直後の reconnect race による「変換中の異常終了」を解消** — 設定保存・モード切替・langbar の「エンジン再起動」などで `engine_reload()` が走った直後、TSF 側の次のキー処理が `engine_start_bg_init` → `connect_or_spawn` を経由して **死にゆくホストパイプに connect** してしまい、Hello/Create の read で `read length` エラーが発火し、エンジンハンドルが破棄されたまま次のキー入力まで復旧しないことがあった（00:26:51 のログで確認: Shutdown→62ms 後の bg_init→101ms 後に "read length"）。原因はホスト側 `server.rs:73-77` の「応答配送のため 50ms sleep してから `process::exit(0)`」窓と、クライアント側 `ensure_connected` が Hello/Create 失敗時にリトライしないことの組合せ。対策として:
  - **client.rs**: `ensure_connected` を `try_connect_once` に分離し、1 回失敗時は 200ms sleep してから 1 度だけリトライする経路を追加。死にゆくパイプに当たっても retry 側ではホストが完全 exit 済みなので `spawn_host` 経由で新ホストに繋がる
  - **state.rs**: `engine_reload()` の `eng.shutdown()` 後に `RAKUKAN_ENGINE` mutex を握ったまま 100ms sleep してからハンドルを drop。サーバ側 50ms sleep より長く待つことで、他スレッドの reconnect が dying pipe に当たる確率を大幅に低減。mutex を握っている間、他スレッドの `engine_try_get`/`_or_create` は busy 短絡されるので副作用なし

### Added

- **engine-host のサイレント死を捕捉するための診断強化**
  - `install_panic_hook()`: `panic = "abort"` 設定でも abort 前に panic hook が走ることを利用し、Rust panic を `PANIC at <loc>: <msg> (thread=..., pid=...)` 形式で `rakukan-engine-host.log` に出す。engine DLL 内の Rust panic が log に何も残さず process が消えるのを防ぐ
  - `redirect_stderr_to_log()`: Win32 `SetStdHandle(STD_ERROR_HANDLE)` でホストプロセスの stderr を `rakukan-engine-host.log` へ向ける。`windows_subsystem = "windows"` で console を持たないため stderr が捨てられていた llama.cpp の `fprintf(stderr, ...)` や Rust の `eprintln!` を log と同居させて拾う
- **`engine_reload` 呼出元トラッキング** — `engine_reload()` に `#[track_caller]` を付け、入口で `engine_reload: invoked from <file>:<line>:<col>` をログ。0.7.x で見えていた「reload event/runtime config 由来でない `engine_reload`」が `factory.rs:200` (langbar menu) なのか `factory.rs:959` (mode switch) なのか `state.rs:443` (reload_watcher) なのか即判別できるようになった
- **langbar メニュー由来 reload の明示ログ** — `ID_MENU_ENGINE_RELOAD` の入口で `langbar menu: ID_MENU_ENGINE_RELOAD selected` をログ。`#[track_caller]` と合わせて 5 系統（reload_watcher / mode-switch / langbar / 未知 / panic 経由）を切り分け可能

## [0.7.1] - 2026-04-24

### Fixed

- **設定反映時の host crash を根絶** (M1.6 T-HOST1) — WinUI 設定保存後や `config.toml` 外部編集時に `rakukan-engine-host.exe` が高確率で crash し変換不能になる問題を修正。原因は `Request::Reload` 経路で engine DLL を drop → 新規 load する間に bg スレッド（conv_cache worker / engine_start_load_model / engine_start_load_dict）が unmapped な命令ポインタを指して `0xc0000005` を発火していたこと。対策として:
  - `protocol.rs` に `Request::Shutdown` バリアントを追加（後方互換）
  - `server.rs` が `Shutdown` を受けたら `Response::Unit` を返して 50ms 後に `std::process::exit(0)`
  - `client.rs` に `shutdown(config_json)` メソッドを追加。応答 read 失敗は想定内としてログのみ
  - `state.rs::engine_reload()` を旧 Reload 経路から shutdown + 自動 re-spawn 経路に書き換え。次回 `connect_or_spawn` が新 PID を立ち上げ、保持していた `config_json` で `Create` を再送
  - OS がプロセス終了時に全スレッドと DLL マッピングをまとめて回収するため unmap race が原理的に起きない
- **エンジン読込中の入力握り潰しを解消** (M1.6 T-HOST4) — reload 中や初回起動中、`on_input` / `on_input_raw` が `guard.as_mut() = None` のときに `return Ok(true)` でキー入力を黙って捨てていた問題を修正。`PENDING_KEYS: Mutex<Vec<(char, InputCharKind, bool)>>` を追加し、None 経路では `push_pending_key` で積むだけに変更。engine 復帰後の最初の呼び出しで `drain_pending_keys()` を先に replay してから現在のキーを処理

### Added

- **エンジン読込中のキャレット近傍視覚フィードバック** (M1.6 T-HOST3) — engine 未 ready の期間に打鍵すると、`mode_indicator` を流用してキャレット近傍に記号を表示。経過時間で段階切替（0〜10s: `⏳`、10〜30s: `⌛`、30〜60s: `⚠`、60s 超: `✕`）。60 秒到達後も自動リトライはせず手動開封を待つ（破損 GGUF 等の永続障害で無限ループ回避）
- **reload 時間計測** (M1.6 T-HOST2) — `READY_RESET_AT_MS` に `reset_ready_latches` 時刻を記録。`poll_dict_ready_cached` / `poll_model_ready_cached` の false → true 遷移で `dict ready: X ms since reload reset` / `model ready: X ms since reload reset` をログ出力。warm / cold cache の実測値を取りやすくした。`ready_reset_elapsed_ms()` で UI 側から経過時間を参照できる

### Changed

- **dead code 削除 + dispose 集約** (M1 T3-A / T3-B) — `engine_get_or_create()`（実呼び出し 0 件、`#[allow(dead_code)]` 付きで保留されていた）を完全削除。`OnUninitDocumentMgr` から直接呼ばれていた 3 つの cleanup（`doc_mode_remove` / `invalidate_live_context_for_dm` / `invalidate_composition_for_dm`）を `dispose_dm_resources(dm_ptr: usize)` ヘルパに集約。追加漏れによる不整合を防ぐ

### Documentation

- **クラッシュ調査資料を整備** (M1 T1-D) —
  - `docs/EXPLORER_CRASH_HISTORY.md` 新設: 0.4.3（`msvcp140.dll` クロスロード）から 0.6.6（`DllCanUnloadNow=S_FALSE` 固定）までの Explorer crash 対策年表と 7 つの教訓（TSF DLL を unload させない / engine DLL 内で BG スレッド禁止 / 非同期 EditSession は実行時に再検証 等）
  - `docs/INVESTIGATION_GUIDE.md` 新設: WerFault フルダンプ設定、WinDbg `!analyze -v` 解析プロトコル、既知の `Failure.Bucket` → 対策対応表、race 系ログパターン一覧、症状別チェックリスト、M5（条件付き）との連携フロー

## [0.7.0] - 2026-04-24

### Fixed

- **ブラウザで入力モードが保持されない問題** (M1.7 T-MODE1 / T-MODE2 / T-MODE3) — Chrome / Edge / Firefox 等でタブ切替・ページ遷移時に入力モードが `config.input.default_mode` へ戻ってしまう race を修正。原因は 3 層で、それぞれ対応:
  - **T-MODE1** `OnUninitDocumentMgr` が `OnSetFocus` より先に同期発火し `doc_mode_remove` が `dm_to_hwnd` を削除 → 後続の focus 変化処理で HWND 退避がスキップされる経路。`doc_mode_remove` で削除前に `hwnd_modes[hwnd] = mode` をコピーするよう変更
  - **T-MODE2** 同じ DM 内でモードを変えても store は focus-out スナップショット依存のため未反映。Firefox のタブ切替で「直前タブのモード」が他タブへ流出して反転する原因。`IMEState::set_mode` から `doc_mode_remember_current` を呼び、`dm_modes` / `hwnd_modes` を即時更新。`TL_CURRENT_DM` / `TL_CURRENT_HWND` は `process_focus_change` 入口で更新
  - **T-MODE3** `GetForegroundWindow()` が子 HWND を返すケースに対応し、`GetAncestor(GA_ROOT)` でルート HWND に正規化する `foreground_root_hwnd()` ヘルパを導入。doc_mode 経路（Activate 初期化 / `OnSetFocus`）で使用
- **ライブ変換 preview の尻切れによる誤確定** (M1.5 T-BUG2) — LLM の greedy/beam 生成が reading を使い切る前に EOS を出すケースで、preview が極端に短くなり中間部分が欠落する問題に対する防壁を追加。reading との char 数比が 30% 未満なら preview を破棄し reading をそのまま表示する `sanity_check_preview()` を Phase 1A / Phase 1B 両経路に挿入
- **ライブ変換中の中間文字消失** (M1.8 T-MID1) — 速打ち時に「あいうえおかきくけこさしすせそ」入力が「あいうえおかきくけこさし」のように中間〜末尾の文字が消える race を修正。原因は 2 経路で両方に対策:
  - **Phase 1B キュー経路**: `LIVE_PREVIEW_QUEUE` の型を `Option<String>` → `Option<PreviewEntry { preview, reading, gen_when_requested }>` に拡張し、世代カウンタ `LIVE_CONV_GEN: AtomicU32` と reading スナップショットを付与。apply 時点で世代 / reading 不一致なら stale として discard
  - **Phase 1A EditSession 経路**: `TF_ES_READWRITE`（非 SYNC）で遅延実行される EditSession callback に `captured_gen` を渡し、実行時点の世代と比較。不一致なら `E_FAIL` を返し、Phase 1B へ落とす（Phase 1B 側も stale なら discard されるので最終的に no-op）
  - `on_input` / `on_input_raw` / `on_backspace` の入口で `live_conv_gen_bump()` を呼び、reading 変化ごとに世代を前進
- **候補ウィンドウが長い候補に対して狭すぎる問題** — 固定幅 `WIN_WIDTH = 260` を廃止し、`compute_needed_width()` で GDI 実測（`GetTextExtentPoint32W` + Meiryo UI 17px）した幅を `WIN_WIDTH_MIN = 260` / `WIN_WIDTH_MAX = 900` にクランプして使用。`TL_WIN_WIDTH: Cell<i32>` で描画時にも参照。status 行・pager 行も測定対象に含める

### Changed

- **バージョン 0.6.x → 0.7.x シリーズへ移行** — v0.6.6 で Explorer crash の DLL unload race を解消した地点から、安定性向上と user-facing bug fix を中心とした 0.7.x シリーズに移行。0.7.0 は bug fix 集中リリース

## [0.6.7] - 2026-04-22

### Added

- **絵文字辞書 (`mozc emoji_data.tsv`) 対応** — dict-builder に `--emoji <path>` / `--emoji-cost <u16>` 引数と `parse_emoji_tsv()` を追加。install.ps1 が `emoji_data.tsv` を GitHub からダウンロードして辞書に統合。mozc 由来の hiragana 読み（例: 「はーと」→ ♥️、「はやおくり」→ ⏩、「ろけっと」→ 🚀）で引ける。cost デフォルト 6000 で一般語より下位に配置される。候補ウィンドウ内は GDI の制約でモノクロ表示だが、確定先アプリ（Chrome / VSCode / Slack 等の DirectWrite 系）ではカラーで入力される
- **`SessionState::Waiting` に `remainder` / `remainder_reading` フィールドを追加** — WM_TIMER fallback で Selecting 昇格する際に、範囲指定変換の残り読みを正しく引き継げるようになった

### Changed

- **辞書スロット配分を dict 優先化** — `merge_candidates` の `dict_slots` 算出を `(limit/2).max(3)` → `(limit*2/3).max(5)` に変更。辞書ルックアップは mmap binary search で LLM より圧倒的に軽く、性能ペナルティなしで候補密度が上がる
- **Space 変換の `DICT_LIMIT` を 20 → 40 に拡張** — `merge_candidates` に渡す上限を倍増。`num_candidates=9` のままでも辞書由来候補が最大 26 件程度まで並ぶ
- **`on_convert` の inline LLM 待機を 3〜15 秒 → 250ms に短縮** — `LLM_WAIT_MAX_MS` を廃止して `LLM_WAIT_INLINE_MS = 250` に統一。タイムアウト時は既存の WM_TIMER ポーリング経路（`start_waiting_timer`）に即委譲し、hot path の `RAKUKAN_ENGINE` / RpcEngine Connection ミューテックス占有時間を 1 桁以上縮める。⏳ 表示は維持したまま、他のキー入力が待たされない
- **範囲指定変換 (RangeSelect → Space) の二重ブロックを解消** — 旧実装の `convert_sync` + `bg_wait_ms(1500)` を `bg_start` + 250ms inline + WM_TIMER fallback に統一。`on_convert[new]` と同じパターンに合わせて重複 LLM 推論を排除

### Fixed

- **設定画面を開いて閉じただけで変換が止まる問題** — WinUI の `SettingsStore.Save()` が 3 ファイル（`config.toml` / `keymap.toml` / `user_dict.toml`）について on-disk 内容との diff を取り、**実際に書き換わったときだけ `true` を返す**ように変更。`MainWindow.TrySaveAndApply()` は戻り値 `true` の時のみ `SignalReload()` を発火する。これにより内容未変更のクローズでは engine reload（RAKUKAN_ENGINE ミューテックスを数秒占有する経路）が走らず、直後の変換がブロックされない
- **変体仮名の「‥」表示問題** — Windows 標準フォント + 既定 font linking で描画できない Kana Extended-B (U+1AFF0–U+1AFFF) / Kana Supplement (U+1B000–U+1B0FF、変体仮名) / Kana Extended-A (U+1B100–U+1B12F) / Small Kana Extension (U+1B130–U+1B16F) を含む surface を dict-builder が恒久排除。範囲指定型フィルタなので、絵文字 (U+1F000+) や CJK 拡張漢字 (U+20000+) や ⏩ 等の BMP 記号は誤爆せず残る

## [0.6.6] - 2026-04-22

### Fixed

- **Explorer 異常終了の真因対策（DLL unload race）** — `DllCanUnloadNow` を常に `S_FALSE` 固定し、TSF DLL をプロセス常駐させる。
  - **解析**: 2026-04-22 07:23 (UTC 22:23) のクラッシュダンプ (`explorer.exe.3124.dmp`) を WinDbg で解析した結果、`Failure.Bucket = BAD_INSTRUCTION_PTR_c0000005_rakukan_tsf.dll!Unloaded` と判明。スタックは `explorer!CTray::_MessageLoop` → `PeekMessageW` → `UserCallWinProcCheckWow` → `<Unloaded_rakukan_tsf.dll>+0x13e70`。
  - **真因**: `candidate_window.rs:166` の `RegisterClassW` で登録した window class が `UnregisterClassW` されないまま `DllCanUnloadNow=S_OK` で `FreeLibrary` され、in-flight な WM_TIMER / WM_PAINT / kernel callback continuation が消えた wnd_proc アドレスを呼び出して AV。
  - **対策**: `DllCanUnloadNow` で常に `S_FALSE` を返すことで unload race を完全回避。Microsoft 標準 IME も同パターン。メモリコストは TSF クライアントプロセス毎に ~2 MB 程度で実用上無視できる。
  - **位置付け**: v0.6.4 で入れた Phase 1〜3 hardening は別経路の race（Phase1A の stale ITfContext）を想定した preventive defense であり、今回の root cause とは独立。残置する。

## [0.6.5] - 2026-04-21

### Added

- **学習履歴の永続化** (`%APPDATA%\rakukan\learn_history.bin`) — 確定した候補ごとに `(reading → surface, last_access_time, suggestion_freq)` を bincode 形式で記録。IME プロセスの再起動後も学習結果が保持される。
- **WinUI 設定に「学習」トグル** — 「入力」ページに `変換確定時に学習する` トグルを追加。`[input] auto_learn` の on/off を GUI から制御できる
- `DictStore::flush_learn_history()` — 明示的に学習履歴を同期書き出しする API（プロセス終了時やテスト用）
- `DictStore::learn_entry_count()` — 診断用の統合エントリ数取得

### Changed

- **`[input] auto_learn` のデフォルトを `true` に** — 既定で学習が有効に。`user_dict.toml` は手動登録専用に戻り、学習履歴は独立した `learn_history.bin` に書き出される（user_dict.toml が学習で肥大化する問題を解消）
- **学習ロジックを MOZC UserHistoryPredictor 準拠に刷新**
  - 学習対象は **MOZC 辞書またはユーザー辞書に存在する surface** のみ。LLM 由来 / 数字変換 / リテラル候補は学習されない（`DictStore::is_dict_surface` ガード）
  - スコア式 = `last_access_time + 86400 * suggestion_freq * 0.5^(Δdays/30) - chars_count(surface)`。半減期 30 日で頻度ボーナスが減衰する
  - LRU 上限 30,000 件（mozc の `kLruCacheSize` 準拠）、超過時は `last_access_time` 最古から削除
  - `merge_candidates` の優先順位を `user_dict → 学習履歴 (mozc 候補の押し上げ) → LLM → mozc` に変更
- **学習書き込みは `learn()` 内で同期実行** — アトミック書き込み (`.bin.tmp` → rename) で crash 時の破損を防止。write lock は in-memory 更新中のみ、I/O は snapshot に対して lock 外で実行。
  *（Phase 2c 初版では BG スレッド + Drop flush の非同期方式を採用したが、engine DLL 内で BG スレッドを spawn する構成が engine reload 経路 (`SignalReload`) でデッドロック／パニックを誘発し、WinUI 設定画面を開閉するたびに LLM 変換が止まる回帰が発生。hotfix で同期保存に変更し、DLL 側に BG スレッドや Drop I/O を置かない方針に統一）*
- **`user_dict.toml` は学習で更新されなくなった** — `DictStore::learn()` は `learn_history` のみを更新し、`user_dict.toml` には一切書き込まない。ユーザー辞書は設定画面から手動管理する仕様に統一

### Fixed

- **WinUI 設定: モデル ID (ModelVariant) 保存バグ** — 設定画面を開いて閉じる（または再起動）すると `model_variant` キーが `config.toml` から消失し、次回起動時に placeholder (`jinen-v1-xsmall-q5`) に戻る問題を修正。`ApplyModelVariantToCombo()` ヘルパーで `ComboBox.SelectedItem` を明示的に Tag 一致の `ComboBoxItem` に設定するようにし、`IsEditable=True` ComboBox の `Text` だけ代入する旧実装が内部で失効していた挙動を回避

## [0.6.4] - 2026-04-21

### Fixed

- **Explorer 異常終了対策の hardening (Phase 1〜3)**:
  - **Phase 1**: `OnUninitDocumentMgr` で破棄される DM に紐づく `COMPOSITION` も stale フラグを立てる。`COMPOSITION` 構造体に `dm_ptr` / `stale` フィールドを追加。msctf コールバック中に即 drop せず後続の安全な文脈で無効化することで、Phase1A callback が stale な composition を掴むレースを縮小
  - **Phase 2**: Phase1A の `EditSession` callback 冒頭で `current_focus_dm_ptr()` を再検証し、`live_input_notify()` 時点の DM と一致しなければ `E_FAIL` で中断。`RequestEditSession` から callback 実行までの間に focus DM が切り替わるレースを完全にカバー
  - **Phase 3**: `EditSession` 経路の panic 直結箇所を `Result` 化。`get_insert_range_or_end()` / `get_document_end_range()` で `unwrap()` を撤去、`suffix_after_prefix_or_empty()` で byte index 依存の panic を抑止。`panic = "abort"` 下で TSF DLL 内の panic が Explorer プロセスを停止させる経路を縮小
- **Phase 3 ゲート検証スクリプト**: `scripts/verify-phase3.ps1` で hardening 完了を機械的に検証可能

## [0.6.3] - 2026-04-21

### Fixed

- **ローマ字入力時の未確定文字消失** — `RakunEngine::push_char` で engine 側 `pending_romaji_buf` と `RomajiConverter` 内部 `buffer` がズレ、`PassThrough` 連鎖時に未確定ローマ字がプリエディット表示から落ちていた問題を修正。`romaji.output` / `romaji.buffer` の差分から「確定したひらがな」と「未確定ローマ字」を判定する方式に変更
  - `qwrty` 入力時に `t` が表示から消えていた
  - `かなkq` 入力時に `q` が表示から消えていた
  - 同根原因として F9/F10 サイクル変換のローマ字復元ログ (`romaji_input_log`) も整合を取り戻す

## [0.6.2] - 2026-04-20

### Added

- **`gpu_backend = "auto"` サポート** — `config.toml` で `"auto"` を明示できるように（従来はキー未指定時のみ自動検出）。実行時にインストール済みの `rakukan_engine_*.dll` を `cuda` → `vulkan` → `cpu` の順で探索して選択する
- **モデル variant `f16` 追加** — `jinen-v1-xsmall-f16` / `jinen-v1-small-f16`（量子化なし FP16、高精度・大容量）を `models.toml` / `install.ps1 $modelMap` / WinUI ComboBox に追加
- **`scripts/refresh-models.ps1`** — HuggingFace API で公開中の `.gguf` を走査し、`models.toml` 未登録分を検出する開発用ツール。`-Apply` で `models.toml` 末尾に自動追記可能
- **WinUI 設定のモデル選択 UI** — TextBox → 編集可能 ComboBox に変更。ドロップダウンにファイルサイズを併記（例: `jinen-v1-xsmall-q5 (約 30 MB)`）。Tag/Content 分離で config.toml には variant ID のみ書き出す

### Changed

- **設定デフォルト値を 3 config テンプレートで統一**
  - `log_level = "info"`（テンプレート内の `"debug"` を修正し、Rust 側の構造体デフォルトと一致）
  - `gpu_backend = "auto"` を有効化（旧: コメントアウト）
  - `n_gpu_layers = 16` / `main_gpu = 0` / `model_variant = "jinen-v1-xsmall-q5"` を有効化（旧: コメントアウト）
  - `dump_active_config = false`（旧: `true`、通常運用では不要なため）
- **`config.toml` の `model_variant` コメント拡充** — 4 variant それぞれのサイズ・用途を併記（約 30 / 84 / 138 / 423 MB）
- **WinUI 設定: `gpu_backend = "auto"` を文字列として保存** — Win32 設定と挙動を統一（旧仕様では `"auto"` 選択時にキー自体を削除していた）
- **WinUI 設定: `log_level` 未設定時のフォールバックを `"info"` に** — Rust 側デフォルトと一致

## [0.6.1] - 2026-04-19

### Added

- **ユーザー辞書 管理 UI**（WinUI 設定アプリ）— 「ユーザー辞書」ナビゲーション項目を追加。読みと変換候補の追加・編集・削除、`user_dict.toml` を notepad で開くボタンを提供
- **候補数の上限拡張** — Space 変換の候補数 (`num_candidates`) の上限を 1-9 → 1-30 に拡張。WinUI 設定 / 従来設定ダイアログ双方の UI バリデーションも追従
- **`[conversion] beam_size` 設定** — Space 変換の beam 幅上限（`num_candidates` と min をとる）。デフォルト 30（実質無制限）。変換速度を抑えたい場合に小さく設定することで beam 幅を制限できる
- **`[input] auto_learn` フラグ** — 確定時のユーザー辞書自動登録を制御する設定を追加。デフォルト `false`（`user_dict.toml` の肥大化を抑止、ユーザー辞書は手動登録のみで運用）

### Fixed

- **ライブ変換の停止不具合** — `on_live_timer` が `engine_try_get` の一時的ロック競合で `has_preedit=false` と誤判定し `stop_live_timer` を呼んでいたのを修正。busy のときはタイマーを止めず次回 tick を待つ
- **候補ウィンドウのアプリ切替時残留** — `ITfThreadFocusSink` を登録、`OnKillThreadFocus` で `hide()` / `stop_live_timer()` / `stop_waiting_timer()` を実行（Alt+Tab 等の非 TSF アプリへのフォーカス遷移に対応）
- **`num_candidates` がライブ変換を遅延させる回帰** — バッチ RPC 経路の `input_char` が prefetch 用 `bg_start(n)` に `num_candidates`（最大 30）を渡していたのを `live_conv_beam_size` に修正。Space 変換時は従来どおり `num_candidates` を使用
- **設定画面からの reload で config.toml が古いまま適用される問題** — `engine_reload` の冒頭で `config::init_config_manager` を呼び、ディスクから最新 `config.toml` を読み直してから EngineConfig JSON を生成するよう修正

### Changed

- **ライブ変換 preview でユーザー辞書を優先** — `bg_take_candidates` がユーザー辞書候補を先頭にマージするよう変更（読み完全一致のみ）
- **`ConversionConfig::beam_size` を engine 側で尊重** — `KanaKanjiConverter` の `beam_size` を `num_candidates.min(config.beam_size).clamp(1, 30)` として計算し、従来のハードコード上限 3 を撤廃

## [0.6.0] - 2026-04-17

### Changed

- Phase1A の冗長ログ削除 — `on_live_timer` の Phase1A ブロックから `tracing::info!` のログ出力を削除（動作は維持）
- OnSetFocus の早期 return — `prev_dm == next_dm` で即 return（TSF 通知ストーム対策）
- OnSetFocus の `next_dm == 0` 処理改善 — モード変更はしないが、前の DM のモードは保存する（アプリ切替でモードが失われる問題の修正）
- 候補ウィンドウのフォーカス変化時自動閉じ — OnSetFocus で別コンテキストに移る場合のみ `hide()` / `stop_live_timer()` を実行

## [0.5.1] - 2026-04-16

### Added

- **数値保護レイヤー** (`digits.rs`)
  - reading を数字ラン / 非数字ランに分割し、LLM には非数字部分だけを渡す
  - `convert_with_digit_protection` で既存の `convert` パスを置換
  - `verify_digits_preserved` による出力検証（桁一致しない候補を除外）
  - 数字のみの変換では半角・全角の両方を候補として提示

- **アルファベット保護**
  - アルファベットランも数字と同様に半角・全角の両方を候補として提示

- **数字入力の半角/全角設定**
  - `config.toml` の `[input] digit_width = "halfwidth" | "fullwidth"` で制御
  - デフォルトを半角に変更

- **範囲指定変換 (RangeSelect)**
  - `Shift+Right/Left` で全文をひらがなに戻し、先頭から変換範囲を指定
  - `Space` で選択範囲を LLM 変換、`Enter` で確定、残りで LiveConv 再開
  - 先頭から順に確定していく方式で、分節アライメント問題が発生しない
  - Preedit / LiveConv / Selecting いずれの状態からも Shift+矢印で開始可能

- **ライブ変換 beam_size 設定**
  - `config.toml` の `[live_conversion] beam_size = 3` で制御（デフォルト: 3）

### Changed

- **engine ABI v7** に bump
- フォーカス変化時に候補ウィンドウを自動で閉じるようにした
- Space 押下時の文節分割を廃止、全文候補選択 (Selecting) のみに簡略化
- Selecting 確定後に remainder がある場合、旧 SplitPreedit ではなく LiveConv を再開

### Removed

- **vibrato 完全削除** — 形態素解析器 vibrato とその辞書 (`assets/vibrato/`)、
  `rakukan-vibrato-builder` クレート、`segmenter.rs` モジュールを全て削除。
  reading/surface のアライメント問題を根本解決
- **SplitPreedit 完全削除** — `SessionState::SplitPreedit`、`ConversionState`、
  `SplitBlock`、関連メソッド・ヘルパ関数を全て削除。RangeSelect に置換
- **convert_to_segments / segment_with_digit_protection** — 分節不要のため削除
- **SegmentBlock / SegmentCandidate** — engine-abi から削除
- RPC の旧 Request/Response バリアントを予約化（postcard 互換維持）

## [0.4.5] - 2026-04-13

### Changed

- **打鍵時の RPC を 1 往復にバッチ化**
  - 0.4.4 までは 1 キーストロークあたり `push_char` / `preedit_display` /
    `hiragana_text` / `bg_status` / `bg_start` 等で 8〜9 回の Named Pipe 往復が
    発生していた
  - 0.4.5 では `Request::InputChar { c, kind, bg_start_n_cands }` を新設し、
    ホスト側で push → `preedit_display` → `hiragana_text` → `bg_status` →
    条件付き `bg_start` までを 1 リクエストで処理
  - レスポンスは `Response::InputCharResult { preedit, hiragana, bg_status }`
  - `PROTOCOL_VERSION` を 2 に bump（古い `rakukan-engine-host.exe` との
    組み合わせでは Hello で弾かれる。インストーラ再適用が必要）
  - TSF の `on_input` 4 分岐（通常 / live_conv / split_preedit / selecting）を
    すべて新 API に置換

- **辞書・モデル ready 状態のラッチ化**
  - `poll_dict_ready` / `poll_model_ready` は一度 true を返したら以降ずっと
    true なので、`DICT_READY_LATCH` / `MODEL_READY_LATCH`（AtomicBool）を
    `rakukan-tsf/src/engine/state.rs` に追加
  - `poll_dict_ready_cached` / `poll_model_ready_cached` ヘルパ関数経由で呼び、
    ready 以降は RPC をスキップ
  - `engine_reload()` でラッチをリセット
  - TSF の `on_input` / `on_convert` / `candidate_window::on_live_timer` の
    該当箇所を cached 版に置換

- **ライブ変換中に debug ログで毎打鍵 2 RPC が走っていた問題を解消**
  - `tracing::debug!` の引数に `is_dict_ready()` と `dict_status()` を渡していた
    ため、log_level=debug（デフォルト）の環境で毎打鍵 2 RPC が発生していた
  - debug ログ自体を削除

### Fixed

- **ライブ変換中に pending ローマ字が表示されない問題**
  - 「tat」と入力したとき、末尾の "t" が一瞬表示された後 BG タイマー発火で
    消えてしまう問題を修正
  - `on_input` の live_conv 分岐で `preedit_display` から pending を切り出し、
    表示文字列に付加（セッションに保存する preview はひらがなのみ）
  - BG タイマー（`candidate_window::on_live_timer`）の Phase 1A 直接 `SetText`
    経路でも pending を末尾に付加するよう修正
  - Phase 1B キュー消費側（`factory.rs`）では、キュー取り出し時の engine から
    最新 pending を付け直す方式に統一（キューには pending 無しの preview を
    格納することで二重付加を回避）

### Added

- **変換パイプライン再設計の設計書** [CONVERTER_REDESIGN.md](docs/CONVERTER_REDESIGN.md)
  - ライブ変換・文節再変換・境界伸縮・数値保護・用法辞書の全面改修設計
  - Mozc の `Segments` / `Segment` / `Candidate` モデルを参考にした新データモデル
  - Phase A〜F の段階的移行計画
  - 決定事項: `live_conv_beam_size` / `convert_beam_size` の config 追加、
    Mozc コードは思想参考のみ・コピーなし、Shift+矢印の伸縮で merge/split 兼用、
    Candidate 注釈は Phase F として独立追加、候補一覧 Tab 展開は Phase E
  - 実装は 0.4.6 以降の Phase A から順次

- **README に課題リスト / 設計書リンクを集約**
  - `## 課題リスト` セクションを追加
  - 主要設計書・進行中の主要課題（Phase A〜F）・独立した技術課題・過去のスナップ
    ショットの 4 カテゴリで整理

- **handoff.md の残タスクに CONVERTER_REDESIGN への紐付けを追加**
  - `[Num-1]` / `Segment ベースの本格文節管理` / `数字・助数詞の構造対応` /
    `長文・句読点混じりでの分節精度確認` に該当節のリンクを追記

## [0.4.4] - 2026-04-13

### Changed

- **エンジンを別プロセス化（out-of-process 化）**
  - `rakukan_engine_*.dll`（llama.cpp 同梱）を TSF DLL からロードせず、
    専用バイナリ `rakukan-engine-host.exe` に集約
  - TSF 側は新設クレート `rakukan-engine-rpc` 経由で Windows Named Pipe
    (`\\.\pipe\rakukan-engine-<user-sid>`) + postcard フレーミングでエンジンを呼ぶ
  - `RpcEngine` は `DynEngine` と同じメソッドシグネチャを露出するため、
    TSF 側の既存コードは型 import 差し替えのみで追従
  - ホストプロセスは TSF 側が必要に応じて `CreateProcessW`
    （DETACHED + NO_WINDOW）で自動 spawn、最大 5 秒までリトライ接続
  - `rakukan-tsf` クレートの `rakukan-engine-abi` への直接依存を削除

- **Activate 時のエンジン DLL ロードを完全に除去**
  - 0.4.3 までは `Activate` 中に engine DLL を bg スレッドでロードしていた
  - 0.4.4 では **最初の実入力**（`engine_try_get_or_create()` が呼ばれる瞬間）
    まで RPC 接続もホスト spawn も一切発生しない
  - Zoom / Dropbox のように IME を使わないアプリでは `rakukan-engine-host.exe`
    も起動しない

- **Named Pipe に明示的な DACL を設定**
  - SDDL `D:P(A;;GA;;;<current-user-sid>)(A;;GA;;;SY)` を動的に構築し
    `CreateNamedPipeW` の lpSecurityAttributes に渡す
  - 現在のログインユーザー + SYSTEM のみに GENERIC_ALL を許可
  - 同一マシンの別ユーザーや別セッションからの接続を拒否

- **`config.toml` の即時反映を out-of-process 対応**
  - IME モード切替時の `engine_reload()` が新しい `Request::Reload { config_json }`
    を送信するよう変更
  - ホスト側は既存 DynEngine を drop → `DynEngine::load_auto` で新 config 再生成
  - クライアント側は `config_json` を内部に保持し、パイプ切断からの再接続時にも
    直近の設定で `Create` を再送する
  - `n_gpu_layers` / `model_variant` の変更が IME モード切替だけで反映される
    挙動を復活（0.4.4 の RPC 化直前に一時的に失われていた経路を修復）

### Fixed

- **Zoom / Dropbox / explorer 等での異常終了（`0xc0000005`）を根治**
  - 0.4.3 まで `msvcp140.dll` のクロスロード起因で再現していた
  - TSF プロセスに `rakukan_engine_*.dll` を一切持ち込まなくなったことで解消
  - Zoom 実機で確認済み

- **`rakukan-engine-cli` の既存ビルドエラーを修正**
  - `EngineConfig` リテラル構築に `..Default::default()` を追加
  - `n_gpu_layers` / `main_gpu` フィールドが欠けていたためビルドが通らなかった
  - 今後 `EngineConfig` にフィールドが増えても CLI 側は自動追従する

### Added

- **新クレート `rakukan-engine-rpc`**
  - `protocol.rs` / `codec.rs` / `pipe.rs` / `server.rs` / `client.rs`
  - DynEngine の全 API を 1:1 で Request / Response にマップ
  - `Hello { protocol_version }` によるハンドシェイク
  - `OwnedSecurityDescriptor` で SID 取得 + SDDL パース + LocalFree を RAII 管理

- **新バイナリ `rakukan-engine-host.exe`**
  - `#![windows_subsystem = "windows"]` でコンソール非表示
  - ログは `%LOCALAPPDATA%\rakukan\rakukan-engine-host.log`
  - インストーラ（`rakukan_installer.iss` / `install.ps1` / `build-installer.ps1`）
    に配置エントリを追加

## [0.4.3] - 2026-04-10

### Added

- **フローティングモードインジケータ** (`mode_indicator.rs`)
  - キャレット近傍に `あ / ア / A` を短時間表示する補助ウィンドウ
  - モード切替時に視認性を上げるためのもの

### Changed

- **言語バー関連のレイアウトとアイコン処理を整理** (`language_bar.rs`)
- **トレイプロセスを簡素化** (`rakukan-tray/src/main.rs`)
  - 共有メモリ + Event ベースのモード受信に特化

## [0.4.2] - 2026-03-31

### Changed

- **GPU 使用時の診断ログを追加**
  - `debug` ログ時のみ、低頻度で GPU メモリ使用量を記録するよう改善

### Fixed

- **`F6` 後に `Enter` を押すと再変換される問題を修正**
  - 文字種変換直後はライブ変換 fallback を 1 回抑止するよう変更

- **変換後に `Enter` を押さず次の文を入力したとき、前文が確定されない問題を修正**
  - split / 変換中の内容を確定してから次入力へ進むよう整理

- **`F9` / `F10` で英字化すると末尾子音が欠けることがある問題を修正**
  - pending ローマ字を含めて復元するよう改善

## [0.4.1] - 2026-03-29

### Added

- **`n_gpu_layers` 設定を追加**
  - `%APPDATA%\rakukan\config.toml` から GPU オフロード量を調整可能にした
  - README と設定テンプレートに `model_variant` / `n_gpu_layers` の目安を追記

### Changed

- **分節再変換を辞書寄りに調整**
  - 分節対象では候補数を増やし、辞書候補を先に見やすくした

- **設定テンプレートのモデル ID 表記を修正**
  - `small` / `xsmall` の旧表記を `jinen-v1-small-q5` / `jinen-v1-xsmall-q5` に更新

### Fixed

- **長文変換で後半が欠ける問題を修正**
  - 読み長に応じて LLM の生成予算を伸ばすよう変更

- **分節再変換で `Esc` を押しても読みへ戻らない問題を修正**
  - `かっこ -> （ -> Esc` で `かっこ` に戻るよう修正

- **入力モードに応じたスペース入力へ修正**
  - ひらがな / カタカナ入力中は全角スペース
  - 英数モードでは半角スペース

## [0.4.0] - 2026-03-28

### Added

- **ライブ変換 Phase 1 を追加**
  - ひらがな入力後、短い停止でトップ候補を自動表示
  - `Enter` でプレビュー確定、`Space` で通常の再変換操作へ遷移

- **分節ベースの再変換 UI を追加**
  - `Space` 後に文節単位の選択状態へ入る
  - `Left/Right` で選択文節を移動
  - `Shift+Left/Right` で選択範囲を縮小・拡張

- **Vibrato ベースの分節 API を追加**
  - engine / ABI / TSF を通して `surface` から文節候補を取得可能にした
  - `assets/vibrato/system.dic` を同梱対象に追加

- **engine ABI バージョンチェックを追加**
  - 古い engine DLL を読み込んだとき、更新漏れが分かるようにした

### Changed

- **ライブ変換後の編集フローを整理**
  - `F6` 後の `Enter` で古い変換結果へ戻る問題を修正
  - ライブ変換中の追加入力・`Space`・`Enter`・`ESC`・`Backspace` の状態遷移を整理

- **分節選択中の composition 表示を 3 分割化**
  - `prefix / selected / suffix` を保持し、中間文節だけ再変換できるようにした

- **インジケータ初期表示を実際の入力モードへ同期**
  - IME 起動直後に設定と無関係に `"あ"` 表示になる問題を修正

### Fixed

- **ライブ変換中の追加入力で前半が勝手に確定する問題を修正**
- **分節選択中にライブ変換タイマーが割り込んで状態が崩れる問題を修正**
- **`Right` と `Shift+Right` が同一動作になる問題を修正**
- **文節境界が効かず、再変換対象の読みが崩れるケースを複数修正**

## [0.3.8] - 2026-03-23

### Changed

- **`[candidate]` / `[conversion]` セクションを config.toml から削除**（`config.rs`）
  - 未実装のまま残っていた `page_size` / `use_number_selection` / `show_numbers` / `engine` /
    `commit_raw_with_enter` / `cancel_behavior` を設定ファイルおよび構造体から除去
  - `CandidateConfig` / `ConversionConfig` / `CancelBehavior` 構造体を削除
  - `effective_num_candidates()` を `num_candidates.unwrap_or(9).clamp(1, 9)` に単純化
  - `num_candidates` キー（旧互換）はコメントアウト例として残存

- **`enable_jis_keys` を削除し `layout = "jis"` に統合**（`config.rs`）
  - `KeyboardConfig` から `enable_jis_keys: bool` フィールドを削除
  - JIS キー判定は `layout = "jis"` → `KeyboardLayout::Jis` → `KeymapPreset::MsImeJis` の
    既存パスで完結しており、独立フラグは不要だった

- **キーボードレイアウトのデフォルトを `jis` に変更**（`config.rs`）
  - `default_keyboard_layout()` の戻り値を `KeyboardLayout::Jis` に変更
  - `config.toml` / `default_config_text()` の `layout` も `"jis"` に統一

- **`DefaultInputMode::Katakana` を廃止**（`config.rs`）
  - `DefaultInputMode` を `Hiragana` / `Alphanumeric` の 2 択に縮小
  - カタカナモードへの切り替えは F7 / `ModeKatakana` アクションで引き続き動作

- **`default_mode = "alphanumeric"` を有効化**（`config.rs`, `state.rs`）
  - `doc_mode_on_focus_change()` が初回フォーカス時に `config.input.default_mode` を参照するよう改修
  - ターミナル（Windows Terminal / ConHost 等）は config に関わらず常に `Alphanumeric`

- **`remember_last_kana_mode` を有効化**（`state.rs`）
  - `false` に設定した場合、ウィンドウ切り替え時にモードを保存せず毎回デフォルトを適用
  - `true`（デフォルト）では従来通り DocumentManager ごとに前回モードを復元

- **`default_config_text()` を `config/config.toml` に完全同期**（`config.rs`）
  - 初回起動時に生成されるテンプレートを開発用 `config.toml` と一致させた

### Fixed

- **keymap: `Ctrl+J` / `Ctrl+K` / `Ctrl+L` が parse できない問題を修正**（`keymap.rs`）
  - `name_to_vk()` に単一アルファベット（`a`–`z`）のフォールバックを追加
  - `is_ascii_alphabetic()` を `to_ascii_uppercase()` して VK コード 0x41–0x5A に変換
  - これにより `Ctrl+A` ～ `Ctrl+Z` が keymap.toml で全て記述可能になった

- **keymap: 全角/半角キー（`Zenkaku`）の VK コードが誤っていた問題を修正**（`keymap.rs`）
  - `"zenkaku"` / `"hankaku"` / `"kanji"` のマッピングを `0xF3`（VK_DBE_ROMAN）から
    `0x19`（VK_KANJI）に修正
  - 従来は `factory.rs` のハードコードフォールバック（`0x19 => ImeToggle`）のみで動作していた
  - 修正後はキーマップ経由で正常に処理され、`keymap.toml` でのリマップも有効になる

- **確定時に前の文章が消えるバグを修正**（`factory.rs`）
  - `end_composition` / `commit_then_start_composition` の `composition_take()` をセッション外側からセッション内側へ移動
  - 旧コードでは `COMPOSITION=None` になった直後に次キー入力が来ると `update_composition` が
    `existing=None` を見て誤った位置から新 composition を開始し、`SetText` が既存テキストを上書きしていた
  - `get_cursor_range` の `Collapse` 失敗もログ付きで処理するよう変更

- **`remember_last_kana_mode` が機能しない根本バグを修正**（`factory.rs`）
  - `OnSetFocus` / `OnUninitDocumentMgr` / `Activate` で DocumentManager のポインタ取得が誤っていた
  - `d as *const _ as usize`（ローカル参照のスタックアドレス）→
    `*(d as *const ITfDocumentMgr as *const usize)`（COM オブジェクトの内側ポインタ値）に修正
  - 旧コードでは呼び出しごとに異なるキーが生成され `DOC_MODE_STORE` のルックアップが常にミスしていた

- **`default_mode = "alphanumeric"` が反映されない問題を修正**（`factory.rs`）
  - `Activate` 末尾で `tm.GetFocus()` で現在フォーカス中の DM を取得し
    `doc_mode_on_focus_change` で初期モードを即時適用するよう変更
  - `ITfThreadMgrEventSink` 登録前にフォーカス済みの DM には `OnSetFocus` が呼ばれないため
