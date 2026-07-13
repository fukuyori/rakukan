# Conversion Anomaly Fix Plan（変換停止・異常変換の修正計画）

バージョン: draft-1
作成日: 2026-07-03
前提: v0.9.12 時点の rakukan コードベース

## 背景と症状

ユーザー報告により以下の 3 症状が確認されている。

1. **変換の停止**: 変換が時折止まり、LLM 候補がいつまでも表示されない。
2. **途中切れ**: 変換結果が文章の途中で切れる。
3. **重複出力**: 同じ文章が 2 度出るなどの異常変換が時折発生する。

v0.9.12 で自信度（平均 log-prob）ベースの異常検出を導入済みだが、上記症状は解消していない。

## 調査結果（2026-07-03 実施）

コードレビューと実機ログ（`%LOCALAPPDATA%\rakukan\rakukan.log` / `rakukan-engine-host.log`）の突き合わせで、以下を特定した。

### 症状 1: 変換の停止

| # | 原因 | 場所 |
|---|------|------|
| 1a | **true beam search の計算量爆発**。毎ステップ × 毎ビームで `eval_sequence` が新しい LlamaContext を生成しプロンプト全体を再デコードする。読み 30 文字 × beam 18 で 1,000 回超のコンテキスト生成になる | `crates/rakukan-engine/src/kanji/llamacpp.rs:597`（`eval_sequence`）、`llamacpp.rs:532-539`（呼び出し元ループ） |
| 1b | **beam 経路にウォールクロックタイムアウトがない**。15 秒制限（`GEN_TIMEOUT_SECS`）は greedy 経路（候補数 1）のみ | `llamacpp.rs:695-708`（greedy のみ）、`llamacpp.rs:475-591`（beam・制限なし） |
| 1c | **RPC 呼び出しにタイムアウトがない**。engine-host が応答しないと TSF 側は読み取りブロックで固まり、30 秒ウォッチドッグまで復帰不能 | `crates/rakukan-engine-rpc/src/client.rs:427-452`（`call_with_retry`）、`crates/rakukan-tsf/src/engine/state.rs:374-398`（ウォッチドッグ） |
| 1d | **engine-host の多重起動ガードがない**。同一 pipe 名で 2 プロセスが 0.6 秒差で起動した実ログあり（2026-07-02 12:03:52、pid 30800 / 24740） | `crates/rakukan-engine-host/src/main.rs`、`crates/rakukan-engine-rpc/src/server.rs:46-66` |

実測: 直近ログの Space 変換 97 回のうち、LLM 候補が即座に間に合ったのは 2 回のみ（`result=shown`）。残りは辞書候補（60 回）または「変換中」プレースホルダ（35 回）で `llm_pending=true` のまま後追い待ちだった。

### 症状 2: 途中切れ

| # | 原因 | 場所 |
|---|------|------|
| 2a | **EOS 未到達の未完了ビームがそのまま候補として返る**。`finished_beams` が空のとき active ビームを返す（コード内コメントも「truncated に見える」と自認） | `llamacpp.rs:576-580` |
| 2b | **生成予算（読み長×2+8、上限 256 トークン）不足時に全ビームが途中切れ**になる | `crates/rakukan-engine/src/kanji/backend.rs:45-56`（`generation_budget`） |
| 2c | greedy 経路の 15 秒タイムアウト打ち切りも部分出力をそのまま返す | `llamacpp.rs:702-708` |
| 2d | 既存安全網では捕捉不能: 長さ**下限**チェック（読みの 33% 以上）は軽微な尻切れを素通しし、confidence フィルタも途中まで正しい文は平均 log-prob が高く棄却できない | `backend.rs:334`（下限）、`backend.rs:344-348`（confidence） |

### 症状 3: 重複出力

| # | 原因 | 場所 |
|---|------|------|
| 3a | **繰り返しペナルティ・反復検出が一切ない greedy/beam サンプリング**。小型モデルが EOS を出さず同一文を反復する典型的退化。反復は局所的に高確率のため confidence フィルタで棄却できない | `llamacpp.rs:408`（`LlamaSampler::greedy()`）、`llamacpp.rs:263` |
| 3b | **候補長の上限チェックが存在しない**（下限 33% のみ）。「文が 2 回続いた候補」（長さ約 2 倍）が素通りする | `backend.rs:329-334` |
| 3c | （副次・TSF 層）`commit_then_start_composition` が `COMPOSITION_APPLY_LOCK` を取らず、確定系アクションが `conv_gen_bump()` を呼ばないため、遅延実行された Phase1A EditSession が確定後の新 composition に古いプレビューを書き込む競合が理論上可能 | `crates/rakukan-tsf/src/tsf/factory/on_compose.rs:236-375`、`crates/rakukan-tsf/src/tsf/candidate_window.rs:1622-1695`、`on_convert.rs:1410`（`on_commit_raw`、gen bump なし） |

### 調査中に棄却した仮説

- beam ループの `for _step in 0..(max_new_tokens - 1)` は off-by-one では**ない**。初期トークン 1 個（`get_top_k_tokens` 由来）+ ループ `max_new_tokens - 1` 回で合計はちょうど `max_new_tokens`。

## 修正計画

優先度順。Phase 1 は低リスク・即効の候補フィルタ強化、Phase 2 は生成側の打ち切り対策、Phase 3 は性能・インフラの根治。

### Phase 1: 候補後処理の安全網強化（症状 2・3 の緩和）

**F1. 候補長の上限安全網**

- `backend.rs` の下限チェック（`c.chars().count() * 3 >= reading_chars`）の隣に上限を追加する。
- 目安: `候補文字数 <= 読み文字数 × 1.5 + 2`（かな→漢字で文字数は通常縮むため 1.5 倍あれば十分。要係数チューニング。`digit_width` 等で伸びるケースに +2 の余裕）。
- 「同じ文が 2 度」の候補は長さ約 2 倍になるため、これで大半が捕捉できる。

**F2. 反復 n-gram 検出フィルタ**

- 候補文字列に対する純粋関数として実装（llama 非依存・単体テスト可能。`filter_by_confidence` と同じ流儀）。
- 検出条件の案:
  - 候補の前半と後半が同一（完全 2 重化）。
  - 同一の 4-gram（文字ベース）が候補内で 3 回以上出現、または近距離（8 文字以内）で反復。
- 該当候補は棄却。全候補が棄却された場合は既存どおり読みフォールバック。

### Phase 2: 生成打ち切りの扱い改善（症状 2 の根治）

**F3. EOS 未到達ビームを候補にしない**

- `generate_beam_search_impl`（`llamacpp.rs:576-580`）で `finished_beams` が空の場合、active ビームを返す現行動作を変更する。
- 案 A（シンプル）: 未完了のみなら空を返し、`convert` 側で読みフォールバック。
- 案 B（有益）: 戻り値に `finished: bool` を付け、TSF 側はライブプレビューには使ってよいが候補ウィンドウ・確定には使わない。
- `generate_beam_search_d1_greedy_batch`（`llamacpp.rs:451`）も同様に `beam_finished` を結果に反映する。
- 注意: ABI/RPC の型変更を伴う場合は Engine ABI バージョンを engine-abi と engine/ffi.rs の**両方**で更新すること。

**F4. beam 経路にウォールクロックタイムアウト**

- greedy の `GEN_TIMEOUT_SECS = 15` と同じ仕組みを `generate_beam_search_impl` / `generate_beam_search_d1_greedy_batch` のステップループに追加。
- タイムアウト時はその時点の `finished_beams` のみ返す（F3 と整合）。

### Phase 3: 性能・インフラ根治（症状 1 の根治）

**F5. true beam search の KV キャッシュ共有**

- `eval_sequence` の「毎ステップ fresh context + フル再デコード」をやめ、1 つのコンテキストで `n_seq` を beam 数分確保し、`llama_kv_cache_seq_cp` でビーム分岐をコピー共有する実装へ書き換える。
- 期待効果: ステップあたりの計算がフルプロンプト再評価 → 新トークン 1 個のデコードになり、体感 10〜100 倍。
- 参考: `generate_beam_search_d1_greedy_batch`（`llamacpp.rs:321-454`）は既に 1 コンテキスト複数 seq で動いており、同じ構造を拡張する。
- 暫定ワークアラウンド（コード変更前に即適用可）: `config.toml` の `[conversion] beam_size` / `num_candidates` を 18 → 6〜9 に下げる。計算量はほぼビーム数に比例する。

**F6. RPC 呼び出しタイムアウト + engine-host シングルトン化**

- クライアント側 `read_frame` にタイムアウトを導入（named pipe を overlapped I/O 化、または読み取り専用スレッド + チャネルで待ち時間制限）。超過時は接続破棄 → 再接続 → ホスト再 spawn の既存経路に乗せる。
- engine-host 起動時に named mutex（例: `Global\rakukan-engine-host-<user>`）で多重起動を防止。既に保持されていれば即終了。
- TSF 側ウォッチドッグ閾値 30 秒 → 10 秒程度に短縮（`state.rs:374-398`）。

**F7. TSF 確定パスの競合封鎖（症状 3 の副次経路）**

- `commit_then_start_composition`（`on_compose.rs`）の SetText 2 箇所（旧 composition 縮小・新 composition 書き込み）を `COMPOSITION_APPLY_LOCK` で直列化する（Phase1A / `update_composition` と同じ try_lock 流儀）。
- 確定系アクション（`on_commit_raw` の各経路、BlockSelecting の逐次確定）で `conv_gen_bump()` を呼び、遅延 Phase1A / Phase1B の stale 書き込みを gen 不一致で棄却させる。

### Phase 4: 診断強化（再発時の切り分け）

**F8. engine-host の変換ログ**

- 現状ホスト側ログは起動時 5 行のみで、変換 1 件の実行時間・打ち切り理由が残らない。
- 変換ごとに INFO で 1 行出す: 読み文字数 / beam 数 / 生成トークン数 / 所要 ms / EOS 到達ビーム数 / タイムアウト有無。
- これにより「止まった」「切れた」報告時にホスト側ログだけで原因を分類できる。

## 検証方法

- **単体テスト**: F1（上限網）・F2（反復検出）は純粋関数として境界値テストを追加。F3 は EOS 未到達シーケンスを模したフィクスチャで検証。
- **回帰確認**: `cargo make test` および `cargo test --workspace --lib`。
- **実機確認**: `cargo make build-engine` → `cargo make build-tsf` → サインアウト → サインイン → `sudo cargo make install` の順（DLL 使用中の install 失敗を回避）。
- **観測**: F8 のログを有効にした状態で長文（読み 25 文字以上、区読点入り）を継続入力し、
  - Space 変換の `result=shown` 比率が改善すること（現状 2/97）、
  - 反復・途中切れ候補が候補ウィンドウに出ないこと、
  を確認する。

## 実施状況（2026-07-03 実装完了）

| 項目 | 状態 | 備考 |
|------|------|------|
| F1 候補長上限 | ✅ 実装済 | `max_candidate_chars` = 読み×1.5+2。greedy / beam 両経路に適用 |
| F2 反復検出 | ✅ 実装済 | `has_tandem_repeat`（周期 4 文字以上のタンデム反復）。読み自身が反復を含む場合は適用しない |
| F3 未完了ビーム棄却 | ✅ 実装済 | 案 A 採用: finished_beams が空なら空を返し、convert の読みフォールバックに委ねる（ABI 変更なし） |
| F4 beam タイムアウト | ✅ 実装済 | `GEN_TIMEOUT_SECS`（15 秒）をモジュールレベルに昇格し、beam 2 経路にも適用 |
| F5 beam 性能改善 | ✅ 実装済 | KV seq コピーは採用せず「1 コンテキスト + 毎ステップ clear_kv_cache + 全 beam batched decode」方式。fresh context 生成が beam×step 回 → 1 回になった |
| F6 シングルトン + ウォッチドッグ | ✅ 一部実装 | named mutex 多重起動防止（engine_reload の世代交代を考慮し 2 秒リトライ付き）。ウォッチドッグは 30→20 秒（10 秒は GEN_TIMEOUT 15 秒より短く誤発動するため不採用）。**RPC read タイムアウトは未実装**（overlapped I/O 化が必要、別途対応） |
| F7 TSF 確定パス直列化 | ✅ 実装済 | `commit_then_start_composition` / `end_composition` に COMPOSITION_APPLY_LOCK（確定はブロッキング取得）+ `conv_gen_bump()` |
| F8 ホスト側変換ログ | ✅ 実装済 | (1) DLL 内 tracing subscriber 初期化 → `%LOCALAPPDATA%\rakukan\rakukan-engine-dll.log`（従来 DLL 内ログはどこにも出ていなかった）。(2) convert ごとの INFO ログ（reading_chars / beam_size / budget / finished_beams / elapsed_ms）。(3) host dispatch の 1 秒超要求ログ |

### 実装時の主な判断

- **F3 で案 A（ABI 変更なし）を採用**: 未完了 beam は candidate にも preview にも出さない。読みフォールバックの頻度は F5 の高速化 + F4 のタイムアウトで budget 到達自体が稀になる想定。
- **F5 で seq_cp 共有を見送り**: beam 選択は同一親から複数の子を残すため KV の seq 分岐が必要になるが、旧コメントに「GPT-2 モデルで KV コピーに問題」とあり再現リスクを避けた。毎ステップのフル re-decode は残るが 1 回の batched decode に集約され、context 生成（KV 確保）は 1 変換 1 回になる。
- **検証**: `cargo test --workspace --lib` 全パス（238 tests）。beam 経路の実モデル回帰テスト `test_default_model_beam_conversion`（9 候補）を追加しパス確認済み。

### 残課題

- RPC read タイムアウト（F6 後半）: named pipe の overlapped I/O 化または読み取り専用スレッド + チャネル化が必要。engine-host が完全ハングした場合、現状は TSF 側ウォッチドッグ（20 秒）が最終防衛線。
- `has_tandem_repeat` / `max_candidate_chars` の係数チューニング: F8 のログで実運用の棄却状況を観測して調整する。

## 追加調査と修正（2026-07-13 実機テスト）

v0.9.13 の実機テストでユーザー報告「長文変換中に正しい変換ができなくなる」
「変換が異常な状態になり、前の変換内容を表示する」をログ解析で調査した。

### 調査結果

| # | 事象 | 原因 |
|---|------|------|
| A | live 変換が無言で停止（実ログ: 09:02〜09:07 UTC に host 4 回再起動、再起動中は発火せず） | **reload storm**。TSF DLL はアプリごとに別プロセスで動き、各プロセスが独立に config 変更を検出して `engine_reload()` → `Shutdown` を共有シングルトンホストへ送る。設定保存 1 回がプロセス数ぶんの連続再起動になる。再起動中（辞書・モデル各 4 秒ロード）は `bg_start`=false かつ辞書マージ空で `live_input_notify` が呼ばれない |
| B | 状態機械の乖離（実ログ 09:05:19: state=`Preedit("いまわのきわ")` のまま hira="いまは"） | LiveConv → Esc（Cancel）で `Preedit` に落ちた後、Backspace・文字入力が state のテキストを更新しない。`Waiting` 確定（`on_commit_raw`）は state テキストをそのまま commit するため「前の変換内容」が出うる |
| - | 「6 文字でしか変換しない」報告 | バグではない。当時 settings UI で `min_chars=6` に設定されており、しきい値は設定どおり追従していた |
| - | 「変換が橋りょうに」誤変換報告 | 入力自体が「はしりょう」（タイプミス）。エンジンは正常 |

### 修正（F9 / F10、2026-07-13 実装完了）

| 項目 | 状態 | 備考 |
|------|------|------|
| F9 reload storm 対策 | ✅ 実装済 | `Request::ShutdownIfConfigDiffers` を追加（postcard 互換のため enum 末尾に追加、プロトコルバージョン据え置き）。ホスト側は config を engine mutex と別ロックに分離し、変換中でも即応答できる。config 同一なら `Bool(false)` で再起動スキップ。旧ホスト・half-dead ホストで比較不能なら無条件 `Shutdown` にフォールバック。ウォッチドッグ・langbar メニューは `engine_reload_force()` で従来どおり無条件再起動 |
| F10 Preedit 追随 | ✅ 実装済 | `SessionState::sync_preedit_reading()` を追加し、`on_input`（通常/raw）と `on_backspace`（一般/LiveConv 分岐）の 4 経路で実際の読みに同期。読みが空になったら Idle へ。`Preedit` 以外の状態には触らない |

検証: `cargo test --workspace --lib` 全パス（244 tests）。既存の
`is_live_conversion_reading_ready` テスト 2 件が実環境の config.toml
（min_chars=4）を読んで落ちる環境依存だったため、純粋関数
`reading_ready_with_min_chars` に切り出してテストを config 非依存化した。

### 設計上の残リスク（未対応）

- **シングルトンホストのエンジン全クライアント共有**: hiragana_buf / committed
  文脈 / BG converter が全アプリ横断で 1 個（server.rs 冒頭コメントに明記）。
  汚染ガードはフォーカス変化時の ResetAll のみ。中期的にはモデル・辞書は共有
  したままセッション別状態に分離するのが望ましい。
