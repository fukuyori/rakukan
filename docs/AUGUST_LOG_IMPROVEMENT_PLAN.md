# 8月ログ分析にもとづく改善計画

作成: 2026-09-01
対象バージョン: v0.10.5 以降（Phase 単位で分割リリース可）

## 背景

2026年8月の運用ログ（rakukan.log ×6 世代 / rakukan-engine-host.log / rakukan-engine-dll.log ×2 世代、計約 100MB）を分析した。
ERROR・パニック・クラッシュは 0 件。host 再起動は 60 回/月（≈2 回/日、サインイン相当）で再起動ストームは再発していない。

0.10.x（7月改善計画 Phase A〜D）の実機反映は **8/7 頃**（rakukan-engine-dll.log の世代切り替わりで確認。
dll.log.1 は 7/22〜8/7、dll.log は 8/7 以降をカバー）。効果測定は導入前後で分けて集計した。

### Phase E 再計測（7月計画の効果測定）

| 指標 | 7月 | 8/1–7（旧版稼働） | 8/7–31（0.10.x） | 判定 |
|------|-----|-------------------|-------------------|------|
| `engine::init: loading model` | 800 回/月 | 132 回 | **37 回（≈1.5 回/日）** | ✅ 目標達成（初回 + config 変更時のみ） |
| `take_ready MISMATCH`（WARN） | 409 回/月 | 15 回（8/1・8/2・8/4） | **0 回** | ✅ 目標達成 |
| `learn: dict_store not initialized` | — | 206 回（8/4–5 のみ） | **0 回** | ✅ 二重ロード修正の副次効果で学習ロス解消 |
| `engine busy` / `not ready` | 6 回/月 | — | 9 回/月 | ⚠️ 横ばい。host 起動直後の初回変換のみで実害小 |
| echo strip 発動 | 3,182 回/月 | 1,212 回 | 1,689 回 | ❌ 件数横ばい。主因はカタカナ語誤爆（Phase B） |
| `live_continuation_guard` fallback | 330 回/月 | — | 160 回/月 | ⚠️ 半減したが残存。原因特定済み（Phase A） |
| SLOW OnKeyDown >200ms | 14 回/月 | — | 15 回/月 | ⚠️ 横ばい。内訳判明（Phase C） |

その他の観測:

- **確定テキスト消失系**: `end_composition: SetText failed`（TS_E_READONLY）×5 回、全件 firefox.exe。
  0.10.2 の保全パス（同一 session 内 retry → 失敗しても EndComposition まで進めて preedit を確定）が機能しており、
  retry も失敗したケースでもテキストは保全されている。追加対応不要、発生アプリの監視のみ継続。
- **SLOW 集計**（閾値 5ms、`diagnostics.rs:191`）: OnKeyDown n=420 p50=9ms p95=146ms max=931ms /
  Convert n=258 p50=14ms p95=171ms max=502ms / Activate n=55 p50=6.6ms max=107ms。
- **エンジン側 beam 変換**（8/7 以降 n=20,983）: p50=50ms p95=88ms p99=126ms max=817ms。
  TSF 側 Convert p95（171ms）との差 ≈80ms は RPC + edit session のオーバーヘッド（Phase D-2 で計測強化）。

### 8月に判明した問題

| # | 問題 | 8月の実測 | 影響 |
|---|------|-----------|------|
| A | `live_continuation_guard` が正常な漢字圧縮で誤発動 | ×160 回/月（全件 event=fallback） | 正しいプレビューがかな表示に巻き戻るフリッカー |
| B | echo strip のカタカナ語誤爆 | 8/7 以降 ×1,689 回。needle 上位はすべてカタカナ語（いんすとーら ×221、どきゅめんと ×53、あぷりけーし ×36、えみゅれーた ×33…） | 変換済みカタカナ語を含む文を context から不要に除去 → 変換品質の低下 |
| C | モード切替ごとの同期 keymap / config 再読込 | SLOW OnKeyDown の直前ログ `keymap loaded` ×53。最悪例 8/20 の ImeToggle で **931ms** | キースレッドのブロック（ファイル I/O + TOML parse がホットパスに載っている） |
| D | 小粒 2 件 | `engine not ready`/`busy` ×9、Convert の RPC オーバーヘッド内訳が不明 | 起動直後の初回変換が 1 回スキップされる / 遅延分析の分解能不足 |

実施順は **A → B →（C 同乗）→ D（任意）→ E 再計測**。
A はユーザー可視のフリッカーで影響が最も大きく、修正も局所的。B は変換品質に直結。C は独立した小修正。

---

## Phase A: `live_continuation_guard` の誤発動修正（最優先）

### 原因（特定済み）

`crates/rakukan-tsf/src/tsf/factory/on_input.rs:24` の `live_continuation_display` は、
LiveConv 中の追加入力で「preview + suffix」の表示が読みに対して短くなりすぎた場合に
生の読み表示へフォールバックするガードを持つ。判定は長さ比:

```rust
display_base_len * 5 < new_reading_len * 3   // 表示が読みの 60% 未満なら fallback
```

この閾値が**正常な漢字圧縮で成立してしまう**。実ログ（8/21 07:13:15）:

- state = LiveConv(reading「だいとうりょうからこく」11 字, preview「大統領から酷」6 字)
- 'i' 入力 → new_reading 12 字、display_base = preview + suffix「い」= 7 字
- 7×5 = 35 < 12×3 = 36 → **fallback 発動**
- 直前の live timer は正しい preview「大統領から酷」を表示済みなのに、表示が
  「だいとうりょうからこくい」へ巻き戻り、次の live timer 発火（100ms 超先）まで
  かな表示が続く

「だいとうりょう→大統領」は 6→3 字の圧縮であり、漢字密度の高い読みでは表示が読みの
60% を割るのはむしろ正常。ガードが本来防ぎたいのは「preview が古い reading 由来で
現在の reading と対応しない」ケースであって、長さ比はその代理指標として誤爆率が高すぎる。

### 実装

7月計画 Phase E 末尾の「preview の由来キーを保持する」案を実装する。

1. **`SessionState::LiveConv` に preview の由来 reading を保持**
   （現状 `live_conv_parts()` が返す reading は「現在の読み」で、preview がどの読みに
   対する変換結果かは持っていない）。live timer が preview を設定する際に
   `preview_for: String`（その時点の reading）を併せて記録する。
2. **ガード判定を長さ比から prefix 整合へ変更**（`live_continuation_display`）:
   - `preview_for` が `new_reading` の prefix である → preview は現在の読みの先頭部分の
     正当な変換。suffix 連結表示を維持（fallback しない）。
   - prefix でない（読みが Backspace 等で preview の由来と食い違った）→ fallback。
   - 長さ比条件は撤去する。段階導入するなら暫定で閾値を 60% → 40%
     （`display_base_len * 5 < new_reading_len * 2`）に緩めるだけでも 8/21 型の誤爆は消えるが、
     最終形は prefix 整合とする。
3. **WARN ログに `preview_for` を追加**し、fallback が発動した場合に由来キーのずれを
   ログから検証できるようにする（7月計画の宿題）。

### テスト / 検証

- 単体（`on_input.rs` の既存テストに追加）:
  - 「だいとうりょうからこく」+ preview「大統領から酷」+ 'i' 入力で fallback **しない**こと
    （現行テスト `live_continuation_falls_back_when_long_display_gets_too_short` は
    ローマ字 12 字の合成例で、漢字圧縮の実例を含んでいない）。
  - preview の由来 reading が現 reading の prefix でないケースで fallback **する**こと。
- 実機: 1 週間運用して `live_continuation_guard event=fallback` が数回/月以下になること。

---

## Phase B: echo strip のカタカナ語誤爆の抑制

### 現状の問題

`crates/rakukan-engine/src/kanji/backend.rs:97` の `sentence_has_echo_run` は
needle のひらがな形とカタカナ形（`kata_needle`）の両方を照合する。このため:

- ユーザーが「いんすとーら」を変換して「インストーラ」を確定 → context に入る
  （0.10.1 の commit 時除外はカタカナのみのテキストを**意図的に**除外していない）
- 次に同じ語を打つと、`kata_needle`「インストー…」が context 中のカタカナ run
  （「ギャラリーエクスポート」のような長いカタカナ列は `ECHO_RUN_MIN_CHARS = 8` を満たす）
  に一致し、**正当に変換済みの文が context から除去**される
- ドキュメント執筆のように同じカタカナ語を繰り返し打つ場面では毎回発動する
  （live 変換はキーストロークごとに convert を呼ぶため件数も増幅される。
  8月の needle 上位 10 件はすべてカタカナ語）

しかしカタカナ語の echo は**正しい出力そのもの**であり（いんすとーら→インストーラは
context からのコピーで正解になる）、途切れたかな断片候補は `is_kana_prefix_echo`
（`backend.rs:170` 付近）が候補側で既に棄却している。カタカナ一致による strip は
アトラクタ防止の利得がほぼなく、context を失う損失だけが残る。

### 実装

1. **`sentence_has_echo_run` からカタカナ run の除去判定を外す**:
   一致パターンが `kata_needle` で、かつ一致を含む run が**カタカナのみ**で構成される場合は
   エコー源と見なさない（= その文を残す）。
   - F7 カタカナ確定由来の汚染を残す懸念に対して: ひらがな needle が run に一致する場合は
     従来どおり除去されるため、「ひらがな混じりの未変換確定」は引き続き捕捉される。
     純カタカナ確定は echo しても正しい出力になるので除去不要（0.10.1 の commit 時除外が
     カタカナを対象外にしたのと同じ理屈）。
2. ログはそのまま（`echo sentence dropped from context` / `echo source stripped from context`）。
   発動が「ひらがな run のみ」に絞られることを 8月ログの needle 分布と比較して確認する。

### テスト / 検証

- 単体: カタカナ語（「インストーラを起動する。」を context に持ち「いんすとーら」を変換）で
  strip **されない**こと。ひらがな汚染（「きだじゅんいちろう氏は、」）で従来どおり strip されること。
- `repro_context.rs` の全パターンで漢字候補 1 位維持を確認。
- 実機: `echo sentence dropped` が数回/日以下になり、needle にカタカナ語が並ばなくなること。

---

## Phase C: モード切替時の同期 keymap 再読込に変更検出を追加（小粒、A か B に同乗）

### 現状の問題

`crates/rakukan-tsf/src/tsf/factory.rs:984` の `maybe_reload_runtime_config` は
入力モード切替（ImeToggle 等。呼び出し元は `edit_ops.rs` ×5 箇所 + `factory.rs:188`）のたびに:

1. `config::maybe_reload_on_mode_switch()` — こちらは `reload_if_changed()` で**変更検出あり**
2. `Keymap::load()`（`keymap.rs:232`）— **無条件**にファイル読み + TOML parse

を実行する。2 がキースレッド上の同期 I/O であり、ディスクが遅い瞬間（スリープ復帰直後・
ウイルススキャン中など）に OnKeyDown 全体をブロックする。8月の SLOW OnKeyDown 420 件中、
直前ログが `keymap loaded` のものが 53 件。最悪例は 8/20 12:05 の ImeToggle で 931ms
（`keymap loaded` の完了と同時に SLOW 記録）。

### 実装

- `Keymap::load` の呼び出し側（または keymap モジュール内）に config 側と同様の
  **mtime ゲート**を追加する: `keymap.toml` の最終更新時刻を保持し、変わっていなければ
  再読込しない。ファイルが消えた場合はデフォルトへフォールバック（現状踏襲）。
- config 側 `reload_if_changed` の実装を流用できるならヘルパー化して共通化する。

### テスト / 検証

- 単体: mtime 不変で再パースが走らないこと、mtime 更新で新バインドが反映されること。
- 実機: `keymap loaded` の INFO がモード切替のたびに出なくなること（keymap.toml 編集時のみ）。
  SLOW OnKeyDown の `keymap loaded` 直前パターンが消えること。

---

## Phase D: 小粒の残件（任意）

### D-1: host 起動直後の初回変換スキップ（`engine not ready` ×9/月）

host のライフサイクル上、起動直後の初回変換要求が `bg init triggered` で 1 回スキップされる。
Activate 時点で `engine_start_bg_init` は既に呼ばれているため、残るのはモデルロード
（~0.5s）との競走のみ。頻度・実害とも小さいので、対応するなら「not ready 時に
ロード完了ポーリング後の自動リトライ」程度。優先度低。

### D-2: Convert 遅延の内訳計測

TSF 側 Convert p95 171ms に対しエンジン beam p95 88ms。差分（RPC 往復・edit session
取得・candidate window 更新）の内訳が現状のログでは分解できない。診断ログに
RPC 送受信時刻を追加し、次回分析で切り分けられるようにする。計測のみで動作変更なし。

---

## Phase E: 再計測（A〜C 導入後)

1 週間程度運用した後、同じ集計を行い効果を確認する:

| 指標 | 8月実測 | 目標 |
|------|---------|------|
| `live_continuation_guard` fallback | 160 回/月 | 数回/月以下（由来キー不整合の実発生のみ） |
| echo strip 発動（カタカナ needle） | needle 上位 10 がすべてカタカナ語 | カタカナ needle での発動 0 |
| echo strip 発動（全体） | ≈1,700 回/月 | 数回/日以下 |
| SLOW OnKeyDown（`keymap loaded` 直前） | 53 回/月 | 0 |
| `engine::init: loading model` | ≈1.5 回/日 | 維持（悪化していないこと） |
| `take_ready MISMATCH`（WARN） | 0 回 | 維持 |

---

## リリース方針

- Phase A + C で 1 リリース（v0.10.5）: TSF DLL のみの変更。ユーザー可視のフリッカーと
  入力ストールの改善で効果測定が明確。
- Phase B で 1 リリース（v0.10.6）: engine DLL の変更。変換品質（context 保全）の改善。
  `repro_context.rs` の回帰確認を必須とする。
- いずれもインストールは `cargo make build-engine` → `cargo make build-tsf` →
  サインアウト → サインイン → `cargo make install` の順。
