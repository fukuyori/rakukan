# 7月ログ分析にもとづく改善計画

作成: 2026-08-04
対象バージョン: v0.9.16 以降（Phase 単位で分割リリース可）

## 背景

2026年7月の運用ログ（rakukan.log ×5 世代 / rakukan-engine-host.log / rakukan-engine-dll.log ×2 世代、計約 110MB）を分析した。
ERROR・パニック・クラッシュは 0 件で、v0.9.15 の context 汚染修正後は「きだじゅん」型の変換崩壊は再発していない。
一方で WARN 集計から以下の問題が判明した。

| # | 問題 | 7月の実測 | 影響 |
|---|------|-----------|------|
| A | エンジン（モデル）再 init の乱発 | `engine::init` ×800 回（多い日 75〜80 回）。10ms 差のペア発動 ×179、commit 直後 ×185、beam 完了直後 ×178 | 無駄なモデルロード（各 ~0.45s）、その間 engine busy/not ready（×6）、散発的な入力ストールの一因 |
| B | echo strip の過剰発動 | `echo source stripped` ×3,182 回。平均 209 バイト切り捨て、233 回は context が空に。発動の 36% が読み 4〜5 文字 | 正当な context の喪失 → 変換品質の低下 |
| C | BG 変換の無駄打ちと WARN ノイズ | `conv-cache: take_ready MISMATCH` ×409 回。`"ぁr"` `"c"` `"cd"` などローマ字未確定キーで変換起動 | 捨てられる LLM 変換で GPU/CPU 消費、WARN ノイズ |
| D | 確定テキスト消失 | `end_composition: SetText failed`（0x80040209）×2 回（7/17「え」、8/3「ええ」） | 低頻度だがデータロス。発生アプリが特定できない |
| E | live_continuation_guard fallback | ×330 回（全件 event=fallback） | ガードは機能しているが頻度が高い。A/C 修正後に再計測 |

実施順は **A → B → C →（D 同乗）→ E 再計測**。
A は独立したバグ修正で効果測定が容易、B は変換品質に直結、C は A の後の方が効果を観測しやすい。

---

## Phase A: エンジン再 init 乱発の修正（最優先）

**状態: 実装済み（2026-08-04）**。`conv_cache::has_converter()` 追加、`engine_start_load_model` の
ロード要否判定 + `MODEL_LOADING` ガード、`engine_poll_model_ready` の config フィンガープリント照合
（計画に対する追加: Reload 直後に古い config の converter を注入しない安全策）。実機での効果測定は未実施。

### 原因（特定済み）

**バグ 1: converter が conv_cache に出張中だと「モデル未ロード」と誤認する**

- `RakunEngine::is_kanji_ready()` は `self.kanji.is_some()` のみを見る（`crates/rakukan-engine/src/lib.rs:644`）。
- BG 変換の Running / Done 中は converter の所有権が conv_cache 側にあり `engine.kanji = None`。
- この状態で TSF の Activate → `engine_start_bg_init`（`crates/rakukan-tsf/src/engine/state.rs:225-227`）→ RPC `StartLoadModel` → `engine_start_load_model`（`crates/rakukan-engine/src/ffi.rs:410`）が走り、**モデルが存在するのに新たにビルド**する。
- ログの「commit 直後 init ×185」「beam conversion done 直後 init ×178」はこの経路（変換中/直後 = converter 出張中にフォーカス切替が来たケース）。

**バグ 2: `engine_start_load_model` に多重起動ガードがない**

- 辞書側には `DICT_LOADING: AtomicBool` がある（`ffi.rs:459`）が、モデル側にはない。
- 短時間に 2 回呼ばれると 2 スレッドが並行でモデルを 2 個ビルドする。ログの「10ms 差のペア init ×179」がこれ。

### 実装

1. **conv_cache に converter 存在確認 API を追加**（`crates/rakukan-engine/src/conv_cache.rs`）

   ```rust
   /// Running または Done 状態で converter を保持しているか。
   /// pending キューに積まれている場合も true（ワーカーが拾う前のレース対策）。
   pub fn has_converter() -> bool
   ```

   注意: `try_lock` 失敗時は `true` を返す（lock 中 = 誰かが converter を触っている最中であり、
   「存在しない」と誤判定してモデルを二重ロードするより安全側に倒す）。

2. **`engine_start_load_model` を修正**（`ffi.rs:410-430`）

   ```text
   engine_start_load_model:
     1. engine.is_kanji_ready()          → true なら return（現状どおり）
     2. conv_cache::try_reclaim_done()   → 回収できたら set_kanji_converter して return
     3. conv_cache::has_converter()      → true なら return（Running 中: ロード不要）
     4. PENDING_CONVERTER が Some        → return（前回のロードが完了済み・注入待ち）
     5. MODEL_LOADING.swap(true)         → 既に true なら return（多重 spawn 防止）
     6. スレッド spawn して build_converter（完了/失敗時に MODEL_LOADING を false に戻す）
   ```

   `MODEL_LOADING` は `DICT_LOADING` と同形式の `static AtomicBool`。
   **エラー時のフラグ戻し忘れに注意**（build_converter が Err でも必ず false に戻す）。

3. **`engine_poll_model_ready` の掃除**（`ffi.rs:441-454`）
   - 既に `is_kanji_ready() == true` で return する早期パスで、`PENDING_CONVERTER` に残骸があれば破棄する
     （現状は放置され、後で古い converter が注入されうる）。

### 制約・注意

- CLAUDE.md の方針どおり **engine DLL 内で新たな常駐 BG スレッドは作らない**。
  `engine_start_load_model` の一時スレッドは既存設計の範囲内（ロード完了で終了する）。
- `is_kanji_ready()` の意味は変えない（bg_start が「手元に converter があるか」の判定に使っているため）。
  ロード要否の判定だけを上記手順で賢くする。

### テスト / 検証

- 単体: `has_converter()` の状態遷移テスト（Idle/pending/Running/Done）。
- 実機: 1 日運用して `engine::init: loading model` の回数を確認。
  期待値: **初回起動 + config 変更時のみ（1日あたり数回以下）**。ペア init が消えること。
- `engine busy` / `not ready` WARN が減ること。

---

## Phase B: echo strip の誤爆削減

**状態: 実装済み（2026-08-04）**。計画からの変更点:
- run 判定は計画どおり `ECHO_RUN_MIN_CHARS = 8`。除去単位も計画どおり文単位（`split_sentences`、
  戻り値は `Cow<str>`）。発動時は `echo sentence dropped from context` ログに needle と除去文の
  先頭 20 文字を出す。
- commit 時除外の閾値は 8 ではなく **ひらがな 4 文字**（`CONTEXT_ECHO_MIN_HIRAGANA_CHARS`）にした。
  「きもちは、」のような短いひらがな確定は strip の run 条件（8 文字）に届かないため、
  commit 側で低めに拾わないと v0.9.15 より短読みエコーに弱くなる。
- commit 時除外は**ひらがなのみ**を対象とし、カタカナのみのテキストは除外しない
  （カタカナはエコーしても正しい出力になるため。混在汚染は strip が保険で捕捉）。
- 実機事例の汚染文「きだじゅんいちろう氏は、」は漢字（氏）を含むため、「文に漢字がなければ
  エコー源」という単純化は不可と確認済み。run 長判定が必須。
- repro_context.rs で汚染 context 全パターンの漢字候補 1 位維持を確認済み。実機での効果測定は未実施。

### 現状の問題

`strip_echo_context`（`crates/rakukan-engine/src/kanji/backend.rs:79`）は
「読み先頭 needle（最大 6 文字、読み 4 文字未満は対象外）が context のどこかに出現したら、その位置以降を全部切り捨て」。

- 変換済みの文にも送り仮名・助詞・かな語は普通に含まれるため、読み 4〜5 文字では
  正当な context に偶然一致して切り捨てる誤爆が起きやすい（7月: 発動 3,182 回、平均 209 バイト切り捨て、233 回で context 全損）。
- 一致位置以降を**全部**捨てるため、後続の正常な変換済み文まで失われる。

### 実装（3 段構え）

1. **「長いかな連続 run 内の一致」だけをエコー源とみなす**（backend.rs、純粋関数のまま）

   ```rust
   /// エコー源とみなす かな連続 run の最小長（文字数）。
   /// 本物のエコー源（未変換確定文）は長いかな列になる。
   /// 変換済みの文は数文字先に漢字・記号が現れるため run が短い。
   const ECHO_RUN_MIN_CHARS: usize = 8;
   ```

   - 一致位置から左右に「かな / 長音 / 小書き」を伸ばして run 長を測り、
     `run_len >= max(ECHO_RUN_MIN_CHARS, needle_chars + 2)` の場合のみエコー源と判定。
   - カタカナ一致（F7 確定由来）も同じ run 判定を適用。

2. **切り捨て範囲を「エコー run を含む文」に限定**

   - 現状の「一致位置以降すべて削除」をやめ、エコー源を含む文（`。` `！` `？` 区切り）だけを
     context から除去し、前後の文は温存する。
   - 戻り値が `&str` スライスでは表現できなくなるため、シグネチャを
     `fn strip_echo_context(context: &str, reading: &str) -> Cow<'_, str>` に変更
     （発動しない大多数のケースでコピーを避ける）。

3. **根本対策: commit 時点で汚染を context に入れない**（`crates/rakukan-engine/src/lib.rs` の `commit()`）

   - `committed` に追記する際、追記対象の文が「かな（+長音・句読点）のみで一定長以上」なら
     context バッファから除外する（確定自体は通常どおり行う。context に入れないだけ）。
   - 閾値は `ECHO_RUN_MIN_CHARS` と共通でよい。
   - これによりエコー源がそもそも context に入らなくなり、1・2 の strip は
     「過去バージョンからの持ち越し汚染・カタカナ確定」等に対する保険に格下げされる。

4. **観測性の改善**

   - 発動時の INFO ログに needle と一致箇所前後の断片（±10 文字程度）を含め、誤爆かどうかを
     ログだけで判定できるようにする。1〜3 の安定後に DEBUG へ降格。

### テスト / 検証

- 既存 7 テスト（`strip_echo_context_*` / `kana_prefix_echo_*`)を新仕様に更新 + 追加:
  - 変換済み文中の送り仮名・助詞への偶然一致（短い run）→ 発動しない
  - 長いかな文（本物のエコー源）→ その文だけ除去、前後の文は残る
  - commit 時のかな文除外（かな文が context に入らない / 漢字混じり文は入る）
- 回帰: `crates/rakukan-engine/examples/repro_context.rs` で汚染 context でも漢字候補が 1 位のままであること。
- 実機: `echo source stripped` の発動回数が激減すること（目安: 数回/日以下）。
  発動時のログ断片を見て誤爆が混ざっていないこと。

---

## Phase C: BG 変換の無駄打ち削減と WARN 降格

### 現状の問題

- `conv-cache: take_ready MISMATCH`（`crates/rakukan-engine/src/conv_cache.rs:293-299`）は
  「BG 変換がタイプ速度に負けて 1 文字古い」だけの想定内レースで、リカバリ処理も既にある
  （`crates/rakukan-tsf/src/tsf/candidate_window.rs:1071-1084` で正しいキーで再起動）。WARN は過剰。
- `"ぁr"` `"cd"` のように**末尾が未確定ローマ字**のキーで BG 変換（LLM 実行）を起動しており、
  次の打鍵で必ずキーが変わる = 確実に捨てられる変換に GPU/CPU を使っている。

### 実装

1. **WARN の降格**（conv_cache.rs:293）
   - `cache_key` が `req_key` の prefix（またはその逆）の不一致 → 想定内レースとして `trace!` に降格。
   - prefix 関係にない不一致のみ `warn!` を維持（本当に想定外のキー混線の検出用）。

2. **末尾未確定ローマ字での bg_start 抑止**（`crates/rakukan-engine/src/lib.rs:794` の `bg_start`）
   - `hiragana_buf` の末尾文字が ASCII アルファベットの場合は起動しない（`return false`）。
   - ただし **読み全体が ASCII の場合（英字入力モード相当）は除外しない**か、そもそも
     ライブ変換の対象外なので影響なしを確認する。判定は純粋関数
     `fn ends_with_pending_romaji(reading: &str) -> bool` に切り出して単体テストを書く。

3. **（任意・様子見）デバウンス**
   - 1・2 の効果を実機で見てから判断。効果が不十分なら、打鍵直後の即時 bg_start に
     80〜150ms のデバウンスを検討（ライブ変換の体感遅延とのトレードオフ）。

### テスト / 検証

- 単体: `ends_with_pending_romaji` のテスト（`"きょう"` false / `"ぁr"` true / `"cd"` true / `""` false）。
- 実機: MISMATCH WARN がほぼ 0 になること。ライブ変換の体感が悪化しないこと。

---

## Phase D: 確定テキスト消失への手当（小粒、B か C に同乗）

### 現状の問題

`end_composition` の SetText が `0x80040209`（「イベントを開始するメソッドがインターフェイスに多すぎます」）で
失敗し、確定文字が消えた（7月〜8月頭で 2 回、いずれも「え」「ええ」の短文）。
どのアプリで発生したかログから特定できない。

### 実装

1. **リトライ**: `crates/rakukan-tsf/src/tsf/factory/on_compose.rs` の `end_composition` で
   SetText 失敗時に別の edit session で 1 回だけ再試行する。
2. **診断情報**: それでも失敗した場合、フォーカス中ウィンドウのプロセス実行ファイル名を
   WARN ログに含める（恒久対策は発生アプリの特定後に判断）。

### テスト / 検証

- 失敗経路の再現は困難なため、リトライ分岐の単体テストは形だけになる。実機で悪化がないことの確認が主。
- 次回発生時のログにアプリ名が残ることがゴール。

---

## Phase E: 再計測（A〜C 導入後）

1 週間程度運用した後、7月分析と同じ集計を行い効果を確認する:

| 指標 | 7月実測 | 目標 |
|------|---------|------|
| `engine::init: loading model` | 800 回/月 | 数回/日以下（初回 + config 変更時のみ） |
| `echo source stripped` | 3,182 回/月 | 数回/日以下、誤爆なし |
| `take_ready MISMATCH`（WARN） | 409 回/月 | ほぼ 0（trace 降格分を除く） |
| `engine busy` / `not ready` | 6 回/月 | 0 |
| `live_continuation_guard` fallback | 330 回/月 | 再計測して判断（A/C の副次効果を確認） |
| SLOW OnKeyDown >200ms | 14 回/月 | 減少傾向を確認 |

live_continuation_guard は A/C 導入で BG 変換の回転が改善すれば頻度が下がる見込み。
下がらなければ、WARN に preview の由来キー（どの reading に対する preview か）を追加して
ずれの発生条件を分析する Phase を別途立てる。

---

## リリース方針

- Phase A 単独で 1 リリース（v0.9.16）: バグ修正のみで効果測定が明確。
- Phase B + C（+ D）で 1 リリース(v0.9.17): 変換品質・効率の改善。
- いずれも engine DLL / TSF DLL の変更を含むため、インストールは
  `cargo make build-engine` → `cargo make build-tsf` → サインアウト → サインイン → `cargo make install` の順。
