# Changelog

<!-- markdownlint-disable MD024 -->
<!-- MD024: Keep-a-Changelog 形式では各バージョンで ### Added/Changed/Fixed が繰り返されるため無効化 -->

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
