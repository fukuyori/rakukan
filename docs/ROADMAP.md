# 作業ロードマップ — 0.7.x / 0.8.x シリーズ

<!-- markdownlint-disable MD024 -->
<!-- MD024: マイルストーンごとに「目的」「作業」「完了条件」「リスク」等を繰り返す構造のため無効化 -->

最終更新: 2026-04-24  
位置づけ: v0.6.7 までで Explorer crash の unload race と変換中レスポンスを収束させた地点からの次期計画。0.7.x で安定性向上・保守性改善を消化し、0.8.x では低リスクなユーザ可視機能から順次追加する。

**v0.7.0 リリース済み（2026-04-24）**: 以下 5 件 + 候補幅の user-facing bug fix を同梱。

- ✅ M1.5 T-BUG2: preview 長サニティチェック（尻切れ抑止）
- ✅ M1.7 T-MODE1: `doc_mode_remove` で破棄前に HWND 退避（ブラウザモード保持）
- ✅ M1.7 T-MODE2: `IMEState::set_mode` から `doc_mode_remember_current` でモード変更を即時 store へ反映（Firefox 反転対策）
- ✅ M1.7 T-MODE3: `GetForegroundWindow()` を `GA_ROOT` 正規化
- ✅ M1.8 T-MID1: Phase1B キュー + Phase1A EditSession 両経路に `LIVE_CONV_GEN` + reading スナップショットを付与し stale 結果を discard（中間文字消失）
- ✅ 候補ウィンドウ幅を動的計算（`GetTextExtentPoint32W` 実測、260〜900px でクランプ）

**v0.7.1 リリース済み（2026-04-24）**: 以下 6 件のホスト crash 根絶 + 基盤整理を同梱。

- ✅ M1.6 T-HOST1: `Request::Shutdown` 追加 + `engine_reload()` を shutdown + 自動 re-spawn 経路化（host crash 根絶）
- ✅ M1.6 T-HOST2: reload 時間計測ログ（`dict ready: X ms` / `model ready: X ms`）
- ✅ M1.6 T-HOST3: エンジン読込中のキャレット近傍視覚フィードバック（`⏳` / `⌛` / `⚠` / `✕` の段階表示）
- ✅ M1.6 T-HOST4: 読込中のキー握り潰し撤去（`PENDING_KEYS` バッファに積んで engine 復帰後 replay）
- ✅ M1 T3-A: `engine_get_or_create()` 削除（dead code）
- ✅ M1 T3-B: `dispose_dm_resources()` ヘルパに 3 つの cleanup を集約
- ✅ M1 T1-D: `docs/EXPLORER_CRASH_HISTORY.md` / `docs/INVESTIGATION_GUIDE.md` 新設

**v0.7.2 リリース済み（2026-04-28）**: 以下 2 件の race 修正 + 診断強化を同梱。

- ✅ M1.6 T-HOST5: `engine_reload` 直後の reconnect race を解消（`ensure_connected` を `try_connect_once` + 200ms sleep + 1 回リトライに分離 / `engine_reload` の `eng.shutdown()` 後に mutex を握ったまま 100ms sleep してハンドル drop）
- ✅ M1.6 T-HOST6: engine-host のサイレント死を捕捉する診断強化（Rust panic hook で `PANIC at <loc>: ...` をログ / Win32 `SetStdHandle` で stderr を log ファイルへ redirect / `engine_reload()` に `#[track_caller]` を付け呼出元 location をログ / langbar メニュー由来の reload に明示ログ追加）

**v0.7.3 リリース済み（2026-04-28）**: 以下 3 件の bug fix を同梱。

- ⚠️ M1.5 T-BUG1 (部分採用): (a) `generation_budget` 上限 128→256 と (c) 出力 candidates のエンジン側フィルタは採用。(b) `min_new_tokens` 機構は実装後 token / char 単位ミスマッチで `〜` 混入の regression が観測されたため**同バージョン内で revert**。長文尻切れ本命修正は logit bias API が整備された段階で再設計予定
- ✅ M1.8 T-MID2: `update_composition` / `update_composition_candidate_parts` の EditSession クロージャ先頭で composition pointer の stale check を再実行
- ✅ M1.8 T-MID3: `COMPOSITION_APPLY_LOCK: Mutex<()>` を導入し Phase1A / `update_composition` 系の SetText を try_lock 排他化（busy なら skip）

**v0.7.5 リリース済み（2026-04-29）**: M3 + M2 §5.1/§5.2 + WinUI 改行コード修正 + Stop hook を同梱。

- ✅ M3 T1-A: `factory.rs` (4816 行) を 6 ファイルに分割（`factory.rs` 核 / `dispatch.rs` / `on_input.rs` / `on_convert.rs` / `on_compose.rs` / `edit_ops.rs`）。**動作変更なし** (純粋切り出し)
- ✅ M2 §5.1 / T1-B: `on_live_timer` (298 行) を 6 サブ関数 (`pass_debounce` / `probe_engine` / `ensure_bg_running` / `fetch_preview` / `build_apply_snapshot` / `try_apply_phase1a` + `queue_phase1b`) に分解。orchestrator は 16 行に縮小
- ✅ M2 §5.2: `bg_peek_top_candidate` 新設で live preview を非破壊化。conv_cache を進めず、表示前に `merge_candidates` を通してユーザー辞書と学習履歴を反映するため、commit 経路 (`bg_take_candidates`) と干渉しない。engine / FFI / engine-abi / engine-rpc / TSF の 5 層に追加 (out-of-process)
- ✅ WinUI 設定 UI で保存した `config.toml` の改行コード LF → CRLF 統一 (`SettingsStore.WriteIfDifferent` に `NormalizeToCrlf` 挟む)
- ✅ Claude Code 用 Stop hook (`.claude/settings.json` + `scripts/check-install-instruction.ps1`) で「`cargo make install` の前に `cargo make build-tsf` 案内が無い」「install 後にサインアウト」のような誤案内を構造的に block

**v0.7.6 リリース済み（2026-04-29）**: M4 LiveConvSession 集約の Phase 1 を同梱。

- ✅ M4 / T2 段階 c の **Phase 1**: TSF スレッドローカルに閉じる 5 種のグローバル状態を `LiveConvSession` 構造体に集約。新ファイル `crates/rakukan-tsf/src/tsf/live_session.rs`。**動作変更なし** (純粋リファクタ)
  - 集約: 旧 `TL_LIVE_CTX` / `TL_LIVE_TID` / `TL_LIVE_DM_PTR` (thread_local Cell/RefCell × 3) + 旧 `LIVE_TIMER_FIRED_ONCE_STATIC` / `LIVE_LAST_INPUT_MS` (static AtomicBool/AtomicU64 × 2) → `LiveConvSession.{ctx, tid, composition_dm_ptr, fired_once, last_input_ms}`
  - `LIVE_DEBOUNCE_CFG_MS` は設定値のため static のまま残す
  - candidate_window.rs の callsite 8 箇所を helper (`set_context_snapshot` / `clear_context_snapshot` / `context_snapshot` / `invalidate_dm_ptr` / `swap_fired_once` / `reset_fired_once` / `store_last_input_ms` / `load_last_input_ms`) 経由に置換

**v0.7.7 リリース済み（2026-04-29）**: M4 LiveConvSession 集約の Phase 2 + M2 §5.3 を同梱。

- ✅ M4 / T2 段階 c の **Phase 2**: cross-thread を含む 4 種のグローバル状態を `LiveShared` 構造体に集約。**動作変更なし** (純粋リファクタ)
  - 集約: 旧 `LIVE_PREVIEW_QUEUE` (`Mutex<Option<PreviewEntry>>`) + 旧 `LIVE_PREVIEW_READY` (`AtomicBool`) + 旧 `SUPPRESS_LIVE_COMMIT_ONCE` (`AtomicBool`) + 旧 `LIVE_CONV_GEN` (`AtomicU32`) → `LiveShared.{preview_queue, preview_ready, suppress_commit_once, conv_gen}`
  - 個別の sync primitive は据え置き (`Mutex<LiveShared>` で一括包むと既存 `COMPOSITION_APPLY_LOCK` との順序関係が複雑化するため)。構造体は名前空間として機能、helper 関数で更新を集約
  - 公開 helper: `queue_preview_set` / `queue_preview_consume` / `queue_preview_clear` / `suppress_commit_arm` / `suppress_commit_clear` / `suppress_commit_take` / `conv_gen_bump` / `conv_gen_snapshot`
  - callsite 14 箇所を helper 経由に置換 (`queue_phase1b` / `dispatch` Phase1B 消費 / `on_input` x4 / `on_convert` x5 / `edit_ops` x2 / `candidate_window` x2)
  - `PreviewEntry` 定義も `engine::state` から `tsf::live_session` に移設
- ✅ **M2 §5.3 `session_nonce`** (composition 開始ごとの identity 識別子): Phase 1B キュー消費時の stale 判定を (gen + reading) 二重から (gen + reading + session_nonce) **三重**防壁に強化
  - `LiveShared.session_nonce: AtomicU64` 追加。`composition_set_with_dm(Some(...), _)` 経路で `fetch_add(1)` (3 callsite — `StartComposition` 成功直後)
  - `PreviewEntry.session_nonce_at_request` 追加。`queue_phase1b` で要求時のスナップショットを格納
  - `dispatch` の Phase1B 消費時に現在値と比較し、不一致ならログ出して破棄。composition が破棄→再生成された後に古い preview がキューに残って次の composition に紛れ込む race を断つ
  - 公開 helper: `session_nonce_advance()` / `session_nonce_snapshot() -> u64`

**v0.8.0 リリース済み（2026-04-29）**: 0.8.x 新機能候補 M6 のうち、低リスクな **M6.2 桁並び漢数字候補** から開始し、実機確認済み。

- ✅ M6.2: 数字だけの reading で半角 / 全角候補に加えて `200` → `二〇〇` のような桁並び漢数字候補を追加
- ✅ 数字保護検証を `〇一二三四五六七八九` / `零` の復元に対応

**v0.8.1 リリース済み（2026-04-29）**: 0.8.x 新機能候補 M6 のうち、同じ literal 保護レイヤーで完結する **M6.4 記号の半角 / 全角候補** を追加。

- ✅ M6.4: ASCII 記号 / 全角記号を `Symbol` run として分割し、半角 / 全角候補を追加
- ✅ `USB-C` / `A+B` / `(test)` のような数字・アルファベット・記号混在 reading を literal として保護し、既存の候補合成経路で提示

**v0.8.2 リリース済み（2026-04-29）**: 0.8.x 新機能候補 M6 のうち、**M6.3 位取り漢数字候補** の通常漢数字部分を追加。大字候補と候補順設定は v0.8.4 で追加。

- ✅ M6.3: `1234` → `千二百三十四`、`10000` → `一万` の位取り漢数字候補を追加
- ✅ `2,400` → `二千四百`、`2.5` → `二点五` のカンマ・小数付き数値に対応
- ✅ 大字候補 (`壱弐参...`) と `[input] digit_candidates_order` は v0.8.4 で追加

**v0.8.3 リリース済み（2026-04-29）**: **M6.1 数字間の区切り文字自動変換** を追加。

- ✅ 数字直後の `、` / `。` を `,` / `.` としてプリエディットに追加
- ✅ `[input] digit_separator_auto = true` を追加（デフォルト true）

**v0.8.4 リリース済み（2026-04-29）**: **M6.3 大字候補 + 数字候補順設定** を追加。

- ✅ `1234` → `壱千弐百参拾四`、`10000` → `壱万` の大字候補を追加
- ✅ `[input] digit_candidates_order = ["arabic", "fullwidth", "positional", "per_digit", "daiji"]` で候補種別と順序を設定可能

**v0.8.5 リリース済み（2026-05-01）**: リリース表記とパッケージメタデータの更新のみ。

- ✅ 動作変更なし

**現状認識（2026-04-23 時点）**: v0.6.6 以降の実機運用で **Explorer の異常終了は 1 度も観測されていない**。crash root cause（DLL unload race）はほぼ収束したと判断し、**0.7.x の主目的を「新機能追加」ではなく「安定性向上 / 保守性改善」** に置く。未発火の crash 対策（M5）に先行投資せず、既に観測されている不具合（M1.5 尻切れ / M1.6 host crash）と、今後の変更を安全に進めるための土台整備（M1 / M2 / M3 / M4）を優先する。

関連資料:

- [handoff.md](handoff.md) — 現在の状態と既知の問題
- [LIVE_CONV_REDESIGN_REVISED.md](LIVE_CONV_REDESIGN_REVISED.md) — ライブ変換再設計案（§18 で採否仕分け済み）
- [GPU_MEMORY_LIFECYCLE.md](GPU_MEMORY_LIFECYCLE.md) — engine-host 多重起動時の GPU 実態

---

## 1. 全体方針

1. **0.7.x の目的は「安定性向上」**: Explorer crash は実機で未再発、機能的にも一通り揃っている。新機能追加ではなく、**観測済みの不具合を潰し、将来の変更を安全に進める土台を整備する**のが主目的
2. **0.7.x シリーズとしてリリース**: crash 根絶 (M1.6) と user-facing bug 修正 (M1.5) を含むため minor bump に値する。M1 以降の refactor を 0.7.x の patch/minor で順次出す
3. **純リファクタリングと機能変更を分離**: 同一コミットに混ぜない。git blame と diff レビューが壊れる
4. **低リスク・小さな変更から**: ウォームアップで開発フローと CI を確認、その後に大きな整理へ進む
5. **未発火の crash 対策には投資しない**: LIVE_CONV_REDESIGN_REVISED.md §18.3 の「採用検討」のうち、M5（WM_TIMER → PostMessage / Explorer シェル分岐）は**実機再発が観測されるまで着手しない**。過剰な先行投資より、現状の安定動作を回帰させないことを優先

### 0.7.x バージョニング方針

| バージョン | 含むマイルストーン | リリース種別 | 狙い |
| --- | --- | --- | --- |
| **v0.7.0** ✅ 2026-04-24 | ✅ M1.5 T-BUG2 + ✅ M1.7 T-MODE1/2/3 + ✅ M1.8 T-MID1（Phase1A/1B 両経路）+ ✅ 候補ウィンドウ幅の動的計算 | minor | 尻切れ / ブラウザモード喪失 / 中間文字消失の即効対策 + 表示改善 |
| **v0.7.1** ✅ 2026-04-24 | ✅ M1.6 T-HOST1〜4（host 再起動化 + 読込中 UI + 握り潰し撤去 + 時間計測）+ ✅ M1 T3-A/T3-B/T1-D（基盤整理 + docs 整備） | minor | host crash 根絶 + 読込中体感改善 + dead code 削減 + 調査資料整備 |
| **v0.7.2** ✅ 2026-04-28 | ✅ M1.6 T-HOST5（`engine_reload` reconnect race の解消、`ensure_connected` リトライ + `engine_reload` 100ms sleep）+ ✅ M1.6 T-HOST6（engine-host 診断強化: panic hook / stderr→log redirect / `#[track_caller]` ベース呼出元ログ） | patch | 観測済み race の即時修正 + サイレント死診断 |
| **v0.7.3** | M1.5 T-BUG1（早期 EOS 抑制、繰り延べ）+ M1.8 T-MID2/3（stale check + SetText 排他、繰り延べ） | patch | engine 品質改善 + race 対策堅牢化 |
| ~~**v0.7.4**~~ | ~~M3（factory.rs 分割）~~ | — | v0.7.5 に統合 (リリース skip) |
| **v0.7.5** ✅ 2026-04-29 | ✅ M3 T1-A (factory.rs 分割) + ✅ M2 §5.1 (on_live_timer 6 分解) + ✅ M2 §5.2 (bg_peek_top_candidate 新設) + WinUI config.toml CRLF 統一 + Claude Code Stop hook 追加 | minor | リファクタ + preview 経路の非破壊化 |
| **v0.7.6** ✅ 2026-04-29 | ✅ M4 Phase 1 (LiveConvSession 集約 — TSF スレッドローカルに閉じる 5 種を構造体化) | patch | ライブ変換中枢の再設計 (Phase 1) |
| **v0.7.7** ✅ 2026-04-29 | ✅ M4 Phase 2 (LiveShared 集約 — cross-thread 状態 `LIVE_PREVIEW_QUEUE` / `LIVE_PREVIEW_READY` / `SUPPRESS_LIVE_COMMIT_ONCE` / `LIVE_CONV_GEN` を構造体化) + ✅ **M2 §5.3 session_nonce** (Phase 1B キューの stale 判定を gen + reading + session_nonce の三重防壁に強化) | minor | ライブ変換中枢の再設計 (Phase 2) + composition 跨ぎ stale 防壁 |
| **v0.8.0** ✅ 2026-04-29 | ✅ M6.2 桁並び漢数字候補 (`200` → `二〇〇`) | minor | 0.8.x 新機能候補 M6 の最初の低リスク機能 |
| **v0.8.1** ✅ 2026-04-29 | ✅ M6.4 記号の半角 / 全角候補 (`USB-C` → `USB－C`) | minor | literal 保護レイヤーの記号対応 |
| **v0.8.2** ✅ 2026-04-29 | ✅ M6.3 位取り漢数字候補の通常漢数字部分 (`1234` → `千二百三十四`) | minor | 数字候補の日本語表記拡張 |
| **v0.8.3** ✅ 2026-04-29 | ✅ M6.1 数字間の区切り文字自動変換 (`2、4` → `2,4`) | minor | 数値入力の直接編集改善 |
| **v0.8.4** ✅ 2026-04-29 | ✅ M6.3 大字候補 + `digit_candidates_order` | minor | 数字候補の仕上げ |
| **v0.8.5** ✅ 2026-05-01 | リリース表記とパッケージメタデータの更新 | patch | 動作変更なし |
| **v0.7.x patch** | M5（再発時のみ） | patch | 条件付き |

原則:

- **v0.7.0 をできるだけ早く出す**: crash 直撃のユーザがいるため bug fix 集中型の初版で引き上げ
- **minor bump は「ユーザから見た動作変化が見える」or「API 互換破り」**: M1.6/M2/M4 は前者、M1/M3 は後者なし → patch
- **M5 は再発時のみで例外扱い**、シリーズに組み込まない

---

## 2. マイルストーン全体像

```text
M0: v0.6.6 実機 PASS 確認         （継続観察）  ← 暫定 PASS 済（2026-04-23）
   │
   ├─ M1: 基盤整理（並行可能）    （3〜5 日）
   │   ├─ T3-A: engine accessor 整理
   │   ├─ T3-B: dispose 集約
   │   └─ ドキュメント / コメント cleanup
   │
   ├─ M1.5: ライブ変換尻切れ修正  （1〜2 日） ★ user-facing bug
   │   ├─ T-BUG1: 早期 EOS 抑制 / max_new_tokens 上限引き上げ
   │   └─ T-BUG2: preview 長さサニティチェック & fallback
   │
   ├─ M1.6: 設定反映時の host 再起動 （2 日前後） ★ host crash 根絶
   │   ├─ T-HOST1: Request::Shutdown 追加 / engine_reload を restart 経路化
   │   ├─ T-HOST2: 再起動時間計測
   │   ├─ T-HOST3: 読込中 UI（段階的エスカレーション、10s/30s/60s）
   │   └─ T-HOST4: 読込中の入力握り潰し対策（かな直接コミットへフォールバック）
   │
   ├─ M1.7: ブラウザ入力モード保持  （1 日前後） ★ user-facing bug
   │   ├─ T-MODE1: doc_mode_remove で破棄前に HWND へ退避（最優先）
   │   ├─ T-MODE2: モード変更ごとに store を即時更新
   │   ├─ T-MODE3: HWND を GA_ROOT へ正規化
   │   ├─ T-MODE4: exe 名フォールバック段の追加（任意）
   │   └─ T-MODE5: mode_store の永続化（%APPDATA%）（任意）
   │
   ├─ M1.8: ライブ変換中の中間文字消失  （1〜2 日） ★ user-facing bug
   │   ├─ T-MID1: Phase1B キューに gen タグを付与し stale 結果を discard
   │   ├─ T-MID2: EditSession 入口で composition stale check
   │   └─ T-MID3: Phase1A / Phase1B の SetText 二重適用を排他化
   │
   ├─ M2: ライブ変換可読性        （3〜5 日）
   │   ├─ T1-B: on_live_timer 分解
   │   ├─ §18.3 採用: bg_peek/take 分離
   │   └─ §18.3 採用: session_nonce + gen
   │
   ├─ M3: factory.rs 分割         （1〜2 日）
   │   └─ T1-A: 純粋なファイル切り出し
   │
   └─ M4: ライブ変換状態集約      （3〜5 日）
       └─ T2: LiveConvSession 構造体導入

M5 (着手しない前提): 追加対策    （実機再発時のみ開封）
   ├─ WM_TIMER → PostMessage 化
   └─ Explorer シェル分岐
```

**M0 が暫定 PASS 済のため、ゲートとしての「通過待ち」は実質不要**。M1 以降に着手してよい。運用中に Explorer crash が再発した時点で M5 を開封する運用に切り替える。

---

## 2a. フェーズ別負荷一覧

各フェーズの見積もり工数・リスク・性質を一表にまとめる。工数は該当セクション内の「想定工数」記述と一致させる。

| フェーズ | タスク | 工数 | リスク | 性質 | 並行可能 | リリース |
| --- | --- | --- | --- | --- | --- | --- |
| **M0** | v0.6.6 実機 PASS 確認（**暫定 PASS: 再発なし**） | 継続観察のみ（作業工数 ≒ 0） | — | ゲート / 観察 | 全フェーズと並行可 | (ゲート通過扱い) |
| **M1** | ✅ T3-A: engine accessor 整理 | 30 分 | 極小 | dead code 削除 | ✓ | **v0.7.1 済** |
| **M1** | ✅ T3-B: dispose 系関数集約 | 1 時間 | 小 | 純粋な集約 | ✓ | **v0.7.1 済** |
| **M1** | ✅ T1-D: docs / コメント cleanup | 1〜2 時間 | なし | 文書のみ | ✓ (M0 と並行可) | **v0.7.1 済** |
| **M1 合計** | 基盤整理（低リスク先行） | **3〜5 日**（実作業 ≒ 4 時間、レビュー/実機含む） | 小 | リファクタ | — | **v0.7.1 済** |
| **M1.5** | ✅ T-BUG2: preview 長サニティチェック | 半日 | 小 | bug fix (TSF 側防壁) | ✓ (M1.6 と並行可) | **v0.7.0 済** |
| **M1.5** | T-BUG1: 早期 EOS 抑制 / budget 拡大 | 1 日 | 中（LLM 品質影響要確認） | bug fix (engine 側本命) | ✓ | v0.7.3（繰り延べ） |
| **M1.5 合計** | ライブ変換尻切れ修正 | **1〜2 日** | 中 | ★ user-facing bug | — | v0.7.0 + v0.7.3 |
| **M1.6** | ✅ T-HOST1: Request::Shutdown + restart 経路化 | 半日 | 小 | RPC 設計変更 | ✓ (M1.5 と並行可) | **v0.7.1 済** |
| **M1.6** | ✅ T-HOST2: 再起動時間計測 | 30 分 | なし | 計測のみ | ✓ | **v0.7.1 済** |
| **M1.6** | ✅ T-HOST3: 段階表示 UI (10s/30s/60s) | 半日 | 小 | UI 追加（MVP: 記号のみ） | ✓ | **v0.7.1 済** |
| **M1.6** | ✅ T-HOST4: 読込中の入力握り潰し対策 | 半日 | 中（hot path 触る） | bug fix + UX | ✓ | **v0.7.1 済** |
| **M1.6** | ✅ T-HOST5: reconnect race 解消（ensure_connected リトライ + engine_reload sleep） | 半日 | 小 | race fix | ✓ | **v0.7.2 済** |
| **M1.6** | ✅ T-HOST6: サイレント死診断（panic hook / stderr→log / `#[track_caller]`） | 半日 | なし | 診断強化 | ✓ | **v0.7.2 済** |
| **M1.6 合計** | 設定反映時の host 再起動化 + race 修正 | **3 日前後** | 小〜中 | ★ host crash 根絶 | — | **v0.7.1 + v0.7.2 済** |
| **M1.7** | ✅ T-MODE1: doc_mode_remove で破棄前に HWND へ退避 | 15 分 | 極小 | bug fix（最優先） | ✓ (M1.5/M1.6 と並行可) | **v0.7.0 済** |
| **M1.7** | ✅ T-MODE2: モード変更ごとに store を即時更新 | 半日 | 小（TL 追加 + 変更経路フック） | bug fix + 設計改善 | ✓ | **v0.7.0 済**（前倒し） |
| **M1.7** | ✅ T-MODE3: HWND を GA_ROOT へ正規化 | 15 分 | 極小 | bug fix | ✓ | **v0.7.0 済**（前倒し） |
| **M1.7** | T-MODE4: exe 名フォールバック段（任意） | 半日 | 小 | bug fix（保険） | ✓ | v0.7.2+ 様子見 |
| **M1.7** | T-MODE5: mode_store 永続化（任意） | 半日 | 小 | 機能改善 | ✓ | v0.7.2+ 様子見 |
| **M1.7 合計** | ブラウザ入力モード保持 | **1 日前後**（T-MODE1〜3） | 小 | ★ user-facing bug | — | **v0.7.0 済** |
| **M1.8** | ✅ T-MID1: Phase1B キューに gen タグを付与 + Phase1A EditSession race fix | 半日 | 小〜中（race 再現確認含む） | bug fix（最優先） | ✓ (M1.5〜M1.7 と並行可) | **v0.7.0 済** |
| **M1.8** | T-MID2: EditSession 入口で composition stale check | 半日 | 小 | bug fix | ✓ | v0.7.3（繰り延べ） |
| **M1.8** | T-MID3: Phase1A / Phase1B の SetText 二重適用排他化 | 半日 | 中（ロック/順序設計要） | bug fix | ✓ | v0.7.3（繰り延べ） |
| **M1.8 合計** | ライブ変換中の中間文字消失 | **1〜2 日** | 中 | ★ user-facing bug | M2 と根本原因共有（先行バックポート） | v0.7.0 + v0.7.3 |
| **M2** | ✅ T1-B: on_live_timer 分解 (6 サブ関数) | 半日 | 中（ロック順保持） | リファクタ | — | **v0.7.5 済** |
| **M2** | ✅ bg_peek/take API 分離 (§18.3) | 1 日 | 中（API 変更 ~10 箇所 → 実際は preview 経路 1 箇所のみ置換） | リファクタ + 機能 | — | **v0.7.5 済** |
| **M2** | ✅ session_nonce + gen (§18.3) | 1 日 | 中 | 機能追加 | — | **v0.7.7 済** (M4 Phase 2 と同梱) |
| **M2 合計** | ライブ変換ロジック可読性向上 | **3〜5 日** | 中 | リファクタ + 機能 | M1.6 後推奨 | **v0.7.5 + v0.7.7 済** |
| **M3** | ✅ T1-A: factory.rs 分割 (6 ファイル化) | **1〜2 日** | 小（純粋切り出し） | リファクタ | — | **v0.7.5 済** |
| **M4** | ✅ T2 Phase 1: LiveConvSession 構造体集約 (TSF スレッドローカル限定の 5 種) | 半日 | 中（純粋リファクタ） | リファクタ | 段階 PR で安全に | **v0.7.6 済** |
| **M4** | ✅ T2 Phase 2: LiveShared 構造体集約 (cross-thread 状態 4 種) + M2 §5.3 `session_nonce` 統合 | 1〜2 日 | 中〜大（ライブ変換中枢） | リファクタ + 機能 | Phase 1 完了後 | **v0.7.7 済** |
| **M5.1** | WM_TIMER → PostMessage 化（条件付き） | 1〜2 日 | 中 | crash 対策 | 実機再発時のみ | v0.7.x patch |
| **M5.2** | Explorer シェル分岐（条件付き） | 半日 | 小（UX 劣化あり） | crash 局所回避 | 実機再発時のみ | v0.7.x patch |

### 縦割り合計

| カテゴリ | 総工数 | 備考 |
| --- | --- | --- |
| 必達（M1〜M4） | **12〜20 日**（**全消化済**） | v0.7.0〜v0.7.7 で M1 全タスク / M1.5〜M1.8 / M2 全タスク / M3 / M4 全 Phase を消化 |
| 必達のうち bug fix 枠（M1.5 + M1.6 + M1.7 + M1.8） | **5〜7 日**（消化済） | M1.5 T-BUG1 と M1.8 T-MID2/3 は v0.7.3 で消化済 |
| 必達のうち refactor 枠（M1 + M2 + M3 + M4） | **7〜13 日**（**消化済**） | v0.7.5（M3 + M2 §5.1/§5.2）/ v0.7.6（M4 Phase 1）/ v0.7.7（M4 Phase 2 + M2 §5.3）に分配 |
| 条件付き（M5） | 1.5〜2.5 日 | **着手しない前提**。再発時のみ開封 |

### 優先度と並行度のヒント

- **M0 は暫定 PASS 済**（v0.6.6 以降 Explorer crash 未観測、2026-04-23 時点）。formal PASS を待たずに M1 系に着手してよい。実機観察は運用で継続
- **M1.5 と M1.6 は触る領域が別**（M1.5: engine + TSF LiveConv 周辺 / M1.6: host + RPC protocol）なので同時進行可
- **M1.7 は M1.5 / M1.6 とも触る領域が別**（M1.7: TSF doc_mode store / focus 処理）で並行可能。特に T-MODE1 は 15 分で入るので v0.7.0 に相乗り推奨
- **M1.8 は M2 と根本原因を共有**（session_nonce / gen）。M2 の対象タスクを v0.7.0 / v0.7.1 に先行バックポートする形で進める。M1.8 完了後 M2 の当該タスクは縮小できる可能性あり
- **M1 の 3 タスクはいずれも独立**で、15 分〜2 時間の単位で隙間時間に消化可
- **M2 以降は M1.6 後が望ましい**: M2 は on_live_timer 等を触るため、M1.6 で入れる T-HOST3 UI とコード位置が重なる可能性がある
- **M5 は着手しない前提**でスケジュールする（再発時のみ開封）。必達工数見積もりから外す

---

## 3. M0 — v0.6.6 安定性確認（継続観察、ゲートとしては通過扱い）

### 現状（2026-04-23）

**Explorer crash は v0.6.6 以降 1 度も観測されていない**（ユーザ実機）。DLL unload race が真の root cause だったとの診断は妥当だったと暫定的に判断し、**このゲートは通過扱い**とする。M1 以降のタスクに順次着手してよい。

formal PASS（1 日連続 crash 0 件）は**運用の中で自然に積み上がる観察期間**として扱い、着手を止める条件ではない。

### 継続観察項目

1. `%LOCALAPPDATA%\CrashDumps\explorer.exe.*.dmp` の新規発生がないか、たまに確認
2. `%LOCALAPPDATA%\rakukan\rakukan.log` に `Phase1A` 周辺の異常ログが出ないか
3. 万一再発したら直ちに ROADMAP に差し戻し（後述 "失敗時の対応" 参照）

### 失敗時の対応（再発時のみ）

1. 新しい dump を WinDbg で `!analyze -v` 解析
2. `Failure.Bucket` が前回と異なる → 別経路の root cause、再仕分け
3. 同じ `rakukan_tsf.dll!Unloaded` → v0.6.6 の修正不足、追加調査
4. 必要なら M5（WM_TIMER → PostMessage / Explorer シェル分岐）を開封

### 作業ガイドライン（再現テストを改めて走らせる場合）

1. WerFault フルダンプ設定（[handoff.md §既知の問題 §1](handoff.md) のコマンド参照）
2. `cargo make build-engine && cargo make build-tsf && cargo make sign && cargo make install`
3. サインアウト → 再ログオン
4. Explorer 主体で 30 分以上連続使用（リネーム / アドレスバー / フォルダ移動 / Alt+Tab）
5. dump 発生有無を確認

---

## 4. M1 — 基盤整理（低リスク先行）

### 4.1 T3-A: engine accessor の重複削減

**現状（2026-04-24 コード照合）**: 4 種類のアクセサが共存

| 関数 | 位置 | 状態 |
| --- | --- | --- |
| `engine_try_get()` | [state.rs:197](../crates/rakukan-tsf/src/engine/state.rs#L197) | hot path 用（try_lock 失敗時 Err） |
| `engine_get()` | [state.rs:204](../crates/rakukan-tsf/src/engine/state.rs#L204) | blocking 用（poison 回復あり） |
| `engine_get_or_create()` | [state.rs:218](../crates/rakukan-tsf/src/engine/state.rs#L218) | **`#[allow(dead_code)]` 付き、実呼び出し 0 件** |
| `engine_try_get_or_create()` | [state.rs:251](../crates/rakukan-tsf/src/engine/state.rs#L251) | hot path + lazy spawn、factory.rs で 9 箇所使用（682, 930, 1028, 1378, 1518, 2532, 3035, 3107, 4321） |

**作業**:

- [state.rs:218](../crates/rakukan-tsf/src/engine/state.rs#L218) の `engine_get_or_create()` を完全削除（`#[allow(dead_code)]` 付きで既に eligible）
- 関連 import の整理（呼び出しが 0 件のため import 箇所なし想定、念のため grep 確認）
- コメント「engine_reload() 等の内部用途・将来の拡張のため残す」を削除

**完了条件**: `cargo check` PASS、`cargo test` PASS、`grep engine_get_or_create` でヒット 0 件

**リスク**: 極小（呼び出し 0 件の dead code 削除）

**想定工数**: 15〜30 分

### 4.2 T3-B: dispose 系関数の集約

**現状（2026-04-24 コード照合）**: [factory.rs:4499-4511](../crates/rakukan-tsf/src/tsf/factory.rs#L4499-L4511) の `OnUninitDocumentMgr` から 3 つの cleanup を直接呼んでいる

```rust
fn OnUninitDocumentMgr(&self, pdim: Option<&ITfDocumentMgr>) -> Result<()> {
    if let Some(dm) = pdim {
        let ptr = dm.as_raw() as usize;
        doc_mode_remove(ptr);                                   // state.rs:1425
        candidate_window::invalidate_live_context_for_dm(ptr);  // candidate_window.rs
        invalidate_composition_for_dm(ptr);                     // state.rs:636
        tracing::trace!("OnUninitDocumentMgr: removed dm={ptr:#x}");
    }
    Ok(())
}
```

**ヘルパ未実装**。`dispose_dm_resources` は codebase に存在しない。

**作業**:

1. [state.rs](../crates/rakukan-tsf/src/engine/state.rs) に `pub fn dispose_dm_resources(dm_ptr: usize)` を新設
2. 内部で `doc_mode_remove(dm_ptr)` → `candidate_window::invalidate_live_context_for_dm(dm_ptr)` → `invalidate_composition_for_dm(dm_ptr)` の順で呼ぶ（既存の順序を維持）
3. [factory.rs:4499-4511](../crates/rakukan-tsf/src/tsf/factory.rs#L4499-L4511) を 1 行呼び出しに置換
4. ⚠️ M1.7 T-MODE1 の `doc_mode_remove` 退避追加と作業位置が重なるため、**T-MODE1 完了後に T3-B を実施**するか、T3-B の helper 内で既に T-MODE1 相当の処理が回るよう最初から入れ込む

**完了条件**: `cargo check` PASS、`cargo make build-tsf` PASS、目視で呼び出し順序が同じことを確認

**リスク**: 小（純粋な集約、T-MODE1 との依存順だけ留意）

**想定工数**: 1 時間

### 4.3 T1-D: ドキュメント / コメント cleanup（M0 と並行可）

**現状（2026-04-24 コード照合）**: 旧仮説表現 (`Phase1A.*race` / `stale ITfContext` / `Explorer crash 主因`) の grep は **0 件**。既に清掃済み。`state.rs:1164` に `"stale or unfocused dm cached"` という表現はあるが別文脈（ライブ変換の stale DM）で、置換不要。

**作業**:

1. ~~旧仮説コメントの grep と置換~~ **既に完了**（2026-04-24 確認）
2. `docs/INVESTIGATION_GUIDE.md`（新規）の作成 — クラッシュ調査プロトコル明文化（dump → WinDbg → root cause → fix の順）
3. `docs/EXPLORER_CRASH_HISTORY.md`（新規）の作成 — 0.4.4 から 0.6.6 までの crash 対策の年表 + 学んだこと

**完了条件**: 新規 2 ドキュメントが作成済みであること

**リスク**: なし（コード本体には触れない）

**想定工数**: 1〜2 時間（新規ドキュメント 2 本の執筆のみ）

### M1 完了条件

- T3-A, T3-B, T1-D 全完了
- `cargo make build-engine && cargo make build-tsf` PASS
- 機能テスト: ローマ字入力、変換、確定、F6-F10 が引き続き動作

---

## 4a. M1.5 — ライブ変換プレビュー尻切れ修正

### 目的

ライブ変換中に reading が長くなると composition の末尾が欠落する不具合を修正する。
Escape で engine 側の hiragana_buf に戻るため engine に入力は残っているが、commit すると短い preview のまま確定してしまい入力内容が失われる。

### 症状

- 15〜20 文字程度のひらがな入力で発生（文長は 128 token 上限と無関係）
- 句読点「、」を含むと発生確率が上がる（jinen モデルが自然な終了点として早期 EOS を出しやすい）
- 画像例: reading `じけいれつでーたのことをさしつづいた…` に対し preview `時系列データのことをさ` で止まり、以降の入力が表示されない

### 原因（2026-04-24 コード照合）

LLM の greedy/beam 生成が reading を使い切る前に EOS を出し、[`KanaKanjiConverter::convert`](../crates/rakukan-engine/src/kanji/backend.rs#L201) (backend.rs:201) が reading より短い preview を返すケース。TSF 側は長さを検証せずに preview を session に保存するため（[candidate_window.rs:1229](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L1229) の `sess.set_live_conv(reading.clone(), preview.clone())`）、以降の `display = preview + suffix` 組み立てで中間部分が欠落する。

関連コード（実測値、2026-04-24）:

- [kanji/backend.rs:32-40](../crates/rakukan-engine/src/kanji/backend.rs#L32-L40) `generation_budget` — `.min(128)` キャップが現存
- [kanji/llamacpp.rs:689-722](../crates/rakukan-engine/src/kanji/llamacpp.rs#L689-L722) `generate_with_sampler` の main loop — EOS 早期 break が 3 箇所（693-697 `eos_token_id` 一致、699-702 `model_eos`、704-707 `is_eog_token`）
- [candidate_window.rs:1127-1148](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L1127-L1148) preview 取得と `display_shown = format!("{preview}{pending}")` 組み立て — **長さ検証なし**
- [candidate_window.rs:1229](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L1229) Phase1A 成功時 `sess.set_live_conv` 保存 — 長さ検証なし
- [factory.rs:999-1029](../crates/rakukan-tsf/src/tsf/factory.rs#L999-L1029) Phase 1B フォールバック内の `display_shown = format!("{preview}{pending}")` 組み立て — 長さ検証なし

### 作業

#### T-BUG1: エンジン側で早期 EOS を抑える

##### a) `generation_budget` の上限引き上げ

[kanji/backend.rs:37-39](../crates/rakukan-engine/src/kanji/backend.rs#L37-L39) を変更:

```rust
// 現状
config_max_new_tokens
    .max(reading_chars.saturating_mul(2).saturating_add(8))
    .min(128)

// 変更案（入力比に応じて最大 256 程度まで許容）
config_max_new_tokens
    .max(reading_chars.saturating_mul(2).saturating_add(8))
    .min(256)
```

**リスク**: 極小（単純な定数引き上げ、KV cache は変換時のみ確保なのでメモリ圧なし）

##### b) min_new_tokens の導入（本命）

[kanji/llamacpp.rs:689-707](../crates/rakukan-engine/src/kanji/llamacpp.rs#L689-L707) の `generate_with_sampler` メインループに `min_new_tokens` 引数を追加し、`generated.len() < min_new_tokens` の間は EOS break を無視する:

```rust
for i in 0..max_new_tokens {
    let new_token = sampler.sample(&ctx, -1);
    let is_eos = eos_token_id.map_or(false, |eos| new_token.0 == eos)
        || new_token == model_eos
        || self.model.is_eog_token(new_token);
    if is_eos {
        if i >= min_new_tokens { break; }
        // min 未達なら EOS を無視して続行（sampler に bias は掛けず単純 skip）
        // 次トークンは decode をやり直さずスキップして再サンプル
        continue;
    }
    // 通常処理
}
```

**注意**: llama-cpp-2 crate の `LlamaSampler` に **直接 logit bias を掛ける API は未確認**。上記の「EOS を無視して再サンプル」方式のほうが確実。ただし sampler が greedy の場合、同じ EOS を再サンプルし続けるリスクあり → その時は `is_eog_token` 判定も continue 側で wrap するなど要検証。

**引き渡し方**: `max_new_tokens` と同じく呼び出し側から渡す。[backend.rs:207](../crates/rakukan-engine/src/kanji/backend.rs#L207) で `reading.chars().count() / 2` を渡すのが妥当。

**リスク**: 中（サンプラ挙動要検証、再サンプル loop の安全性確認）

##### c) 出力が短い場合のエンジン側 fallback

[backend.rs:201 `convert`](../crates/rakukan-engine/src/kanji/backend.rs#L201) の末尾で、`output.chars().count() < reading.chars().count() / 3` の候補をフィルタし、全滅なら reading をそのまま返す:

```rust
let candidates: Vec<String> = candidates.into_iter()
    .filter(|c| c.chars().count() * 3 >= reading.chars().count())
    .collect();
if candidates.is_empty() {
    return Ok(vec![reading.to_string()]);
}
```

**リスク**: 小（副作用少、既存 beam/greedy パスに共通のフィルタ挿入）

#### T-BUG2: TSF 側のサニティチェック（防壁）

**挿入箇所（2 箇所）**:

1. [candidate_window.rs:1133-1142](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L1133-L1142) の `display_shown` 組み立て直前
2. [factory.rs:1021-1025](../crates/rakukan-tsf/src/tsf/factory.rs#L1021-L1025) の `display_shown` 組み立て直前

```rust
// 共通の防壁（candidate_window.rs 側の例）
const PREVIEW_MIN_RATIO_NUM: usize = 3;
const PREVIEW_MIN_RATIO_DEN: usize = 10;
let reading_len = reading.chars().count();
let preview_len = preview.chars().count();
let preview = if preview_len * PREVIEW_MIN_RATIO_DEN < reading_len * PREVIEW_MIN_RATIO_NUM {
    tracing::warn!(
        "[Live] preview discarded: too_short reading_len={} preview_len={}",
        reading_len, preview_len
    );
    reading.clone()  // hiragana そのまま
} else {
    preview
};
```

- `RATIO` は 0.3 固定でまず出す（必要なら設定値 `[live_conversion] preview_min_ratio` として後付け）
- Phase1A の `sess.set_live_conv` ([candidate_window.rs:1229](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L1229)) / Phase1B の `sess.set_live_conv` ([factory.rs:1016](../crates/rakukan-tsf/src/tsf/factory.rs#L1016)) も破棄後の preview で保存される（同じ変数を使うだけ）

**リスク**: 小（防壁のみ、尻切れでない変換まで破棄されないよう RATIO 調整が必要）

### 完了条件

- 画像で報告された入力（`じけいれつでーたのことをさしつづいた…`）で末尾欠落が再現しない
- 30 分程度のライブ変換使用で尻切れが観測されない（デバッグログで `reading.len()` vs `preview.len()` を監視）
- 既存のライブ変換機能テスト PASS（typing → preview → commit）
- `cargo make build-engine && cargo make build-tsf` PASS

### リスク

- **中**: エンジン側（T-BUG1）は llama.cpp サンプラー周りに触れるため変換品質への影響要確認
- **小**: TSF 側（T-BUG2）は preview を reading で置換する保険なので副作用は小さい
- M2 のリファクタ前に入れる想定（M2 で preview 周りのコードが動くため）

### 想定工数

- T-BUG2 のみ先行: 半日（即効、体感改善）
- T-BUG1 本命: 1 日（logit bias 検証含む）
- 合計 1〜2 日

### リリース戦略

**v0.7.0 に T-BUG2（TSF 側防壁）を含める** ✅ リリース済（2026-04-24）。TSF 側に限定した小変更で先行着手。T-BUG1（エンジン側本命）は llama.cpp サンプラーに触れるため品質確認の時間を取りたく、**v0.7.1 patch で別出し**する。

---

## 4b. M1.6 — 設定反映時の host 再起動化

### 目的

WinUI 設定保存や外部エディタでの `config.toml` 変更で `engine_reload` が走った際に、`rakukan-engine-host.exe` が高確率で crash し、変換不能になる問題を根絶する。

### 症状

- WinUI で設定変更 → 保存 → IME が数秒〜数十秒変換できない
- ひどい場合は戻らず、IME モード切替や再ログオンで復旧
- 現象は 0.6.5 の learn_history BG スレッド撤去後も残存

### 原因（確定・2026-04-24 コード照合）

`conv_cache` の常駐 worker スレッド、および `engine_start_load_model` / `engine_start_load_dict` の初期化スレッドが engine DLL 内で実行されている状態で、[server.rs:93-100](../crates/rakukan-engine-rpc/src/server.rs#L93-L100) の `Request::Reload` 経路が:

1. `*g = None` で `DynEngine` を drop → `Arc<Library>` refcount 1→0 → `FreeLibrary` → **engine DLL が unmap**
2. その直後に `load_engine_into` で `Library::new`（新 `LoadLibrary`）

「1 と 2 の間」に DLL がプロセスから完全に消える瞬間があり、そこで実行中のスレッドが unmapped な命令ポインタを指して `0xc0000005` でプロセス崩壊。

現状の Request enum（[protocol.rs:31-48](../crates/rakukan-engine-rpc/src/protocol.rs#L31-L48)）は `Hello` / `Create` / `Reload` / `Bye` のみで、**`Shutdown` バリアントは未実装**。クライアント側 [client.rs:116](../crates/rakukan-engine-rpc/src/client.rs#L116) も `reload()` のみで `shutdown()` 未実装。

詳細は [CLAUDE.md の auto-memory `feedback_engine_dll_bg_threads.md`](../memory/feedback_engine_dll_bg_threads.md) および [CHANGELOG 0.6.5](../CHANGELOG.md) の "Phase 2c 初版では…" を参照。

### 採用方針: host プロセス再起動

DLL 内で drop→reload を頑張らず、**host プロセスを終了させて再 spawn** する。OS がプロセス終了時に全スレッドと DLL マッピングをまとめて回収するため、unmap race が原理的に起きない。

TSF 側の「pipe 切断検知 → `CreateProcessW` で host 再 spawn → `Hello` → `Create { config_json }`」経路は 0.4.4 で実装済み（[handoff.md §ホストプロセスのライフサイクル](handoff.md)）。再利用する。

### 作業

#### T-HOST1: restart 経路の実装

1. [protocol.rs:31](../crates/rakukan-engine-rpc/src/protocol.rs#L31) の `Request` enum に `Shutdown` バリアント追加（Bye とは別。Bye は接続終了の合図で、プロセス終了は意味しない）
2. [server.rs:82-100 dispatch](../crates/rakukan-engine-rpc/src/server.rs#L82-L100) の `Request::Reload` の下に `Request::Shutdown` 分岐追加:
   - `Response::Unit` を返しつつ、呼び出し側ループで exit 処理を走らせる
   - server 側の呼び出しループ末尾で `std::process::exit(0)` が走るよう制御フローを組む（`dispatch` 内で直接 exit するか、戻り値で exit フラグを返すか要設計）
3. [client.rs:116 `reload()`](../crates/rakukan-engine-rpc/src/client.rs#L116) の隣に `shutdown()` メソッドを追加:
   - `Request::Shutdown` を送って `Response::Unit` を受信（タイムアウト 2〜3 秒）
   - タイムアウト時は `TerminateProcess` で強制終了（ProcessHandle の保持が要。現状の client.rs に PID 保持機構があるか要確認）
4. [state.rs:293 `engine_reload()`](../crates/rakukan-tsf/src/engine/state.rs#L293) を書き換え:

    ```rust
    pub fn engine_reload() {
        reset_ready_latches();  // state.rs:108 既存
        std::thread::spawn(|| {
            super::config::init_config_manager();
            let mut guard = match RAKUKAN_ENGINE.lock() { ... };
            if let Some(eng) = guard.0.as_mut() {
                let _ = eng.shutdown(); // 応答不要、host に self-exit させる
            }
            guard.0 = None;
            ENGINE_INIT_STARTED.store(false, AO::Release);
            // 次回の engine_try_get_or_create が connect_or_spawn で再起動する
        });
    }
    ```

5. `Request::Reload` は残してもいいし、未使用になるので削除してもよい（判断は実装時）。T-HOST1 完了後 `Reload` の呼び出し元 grep で 0 件になるのを確認してから削除が安全

#### T-HOST2: 再起動時間計測

- **ユーザ判断基準（2026-04-23 更新）**: **読込中であることが UI で伝われば 10 秒程度までは許容**
  - 旧基準の「2 秒以内」は破棄。T-HOST3 が表示を担保するため、絶対値より「待たされていることが分かるか」が重要
- 実測ポイント:
  - host 起動: `CreateProcessW` から `Hello` 応答まで
  - model load: `start_load_model` 開始から `is_kanji_ready=true` まで
  - dict load: `start_load_dict` 開始から `is_dict_ready=true` まで
- warm cache（2 回目以降）と cold（初回起動直後）の両方を計測してログに残す
- **10 秒を超える場合のみ**以下を検討:
  - **(a) モデル未変更スキップ**: host 側で前回 config と `model_variant` / `n_gpu_layers` / `main_gpu` を比較し、エンジン生成時パラメータが同一なら restart 不要。この場合は従来の `Request::Reload`（= DLL 再 load）経路を使う
  - **(b) 辞書のみ差分再読込**: 同様に辞書パスが同一なら dict だけ再読込、model は再利用
- **(a)(b) は「DLL を保持したまま drop を避ける」別の設計（F1 系）で、unmap race の再燃リスクがあるため採用は慎重に。通常ケースは T-HOST3 の UI 表示で十分と期待**

#### T-HOST3: 読込中 UI フィードバック

モデル再ロード中（= `is_kanji_ready = false` の期間）は変換候補が出ないが、**ユーザに「壊れたわけではなく、今ロード中」だと伝える必要がある**。単一のルートに頼らず複数チャンネルで通知する。

**mode_indicator.rs の現状（2026-04-24 コード照合）**:

- [mode_indicator.rs:55](../crates/rakukan-tsf/src/tsf/mode_indicator.rs#L55) `TL_TEXT: Cell<&'static str>` で **固定文字列専用**（`"あ"` / `"ア"` / `"A"` の 3 値のみ想定）
- [mode_indicator.rs:37-38](../crates/rakukan-tsf/src/tsf/mode_indicator.rs#L37-L38) `WIN_SIZE: 32` / `FONT_HEIGHT: 22` で**1 文字想定の小ウィンドウ**
- [mode_indicator.rs:42](../crates/rakukan-tsf/src/tsf/mode_indicator.rs#L42) `FADE_START_MS: 1500` で 1.5 秒後に自動非表示
- 段階表示タイマーや auto-hide キャンセル機構は**未実装**

T-HOST3 では以下の汎用化が必要:

1. `TL_TEXT` を `Cell<&'static str>` から `RefCell<String>` に変更（可変長テキスト対応）
2. `WIN_SIZE` / `FONT_HEIGHT` を動的サイズ化、または最大幅（例: 300px）で固定
3. `show_loading_indicator(text: &str, auto_hide_ms: Option<u32>)` API を追加（auto_hide_ms=None で手動非表示）
4. ラッチ false→true の遷移で自動 `hide()` するフック（`poll_model_ready_cached` / `poll_dict_ready_cached` から呼ぶ）

**チャンネル設計**:

| チャンネル | 既存インフラ | 表示内容 | 発火タイミング |
| --- | --- | --- | --- |
| キャレット近傍インジケータ | [mode_indicator.rs](../crates/rakukan-tsf/src/tsf/mode_indicator.rs) を汎用化（上記 1〜4） | `⏳ モデル読込中` など短いラベル | 入力開始時に `is_kanji_ready = false` を検知したら表示、ready になったら消す |
| 候補ウィンドウ status line | [show_with_status](../crates/rakukan-tsf/src/tsf/candidate_window.rs) | `LLM モデル読み込み中です。しばらくお待ちください` | Space / 変換キー押下で候補を出そうとしたが LLM 未 ready のとき |
| composition 属性 | TSF DisplayAttribute | 未確定部分を「薄いグレー + 波線」で描画 | ready になるまで継続 |

**優先順位**: キャレット近傍 > 候補ウィンドウ > composition 属性（実装コストの昇順）

**既存の `is_kanji_ready` / `is_dict_ready` を使う（2026-04-24 コード照合）**:

- [poll_model_ready_cached](../crates/rakukan-tsf/src/engine/state.rs) / [poll_dict_ready_cached](../crates/rakukan-tsf/src/engine/state.rs) が既にラッチ化済み
- [reset_ready_latches](../crates/rakukan-tsf/src/engine/state.rs#L108) 実装済み。内部で `DICT_READY_LATCH` / `MODEL_READY_LATCH` を false に戻す
- [engine_reload](../crates/rakukan-tsf/src/engine/state.rs#L293) が `reset_ready_latches()` を呼んでいるので、reload 後は自然に `false` に戻る
- インジケータ側はラッチの状態を見て表示/非表示を切り替えるだけ

**段階的エスカレーション**（ユーザ判断 2026-04-23 を反映）:

| 経過時間 | キャレット近傍 | 候補ウィンドウ status | 備考 |
| --- | --- | --- | --- |
| 0–10 秒 | `⏳ モデル読込中…` | 非表示 | baseline |
| 10–30 秒 | `⏳ モデル読込中…（12s）` 等 | 非表示 | 経過秒数を追記 |
| 30–60 秒 | `⚠️ 読込に時間がかかっています` | `rakukan-engine-host.log を確認してください` | 黄色系の視覚差分 |
| 60 秒+ | `❌ エンジン起動失敗の可能性` | `last_error` の先頭 + ログパス + トレイメニューから再起動可 | **自動リトライしない** |

**実装**:

- `reset_ready_latches()` 呼び出し時刻を TSF 側で記録
- `poll_model_ready_cached` が false の間、経過時間に応じて表示文字列を切り替え
- ready に戻った瞬間にインジケータ非表示
- 60 秒到達後も同じ表示を維持し、ユーザが手動でトレイの「エンジン再起動」を叩くまで何もしない（無限ループ回避）

**原則**:

- **自動リトライ禁止**: 60 秒で自動再起動すると、破損 GGUF 等の永続障害で無限ループになる
- **手動エスケープハッチ**: [factory.rs:158 `ID_MENU_ENGINE_RELOAD`](../crates/rakukan-tsf/src/tsf/factory.rs#L158) を stuck 状態でも呼べることを確認
- **ログ誘導**: エラー表示に `%LOCALAPPDATA%\rakukan\rakukan-engine-host.log` のパスを必ず含める

#### T-HOST4: 読込中の入力握り潰し対策

**現状の問題（2026-04-24 コード照合）**: [factory.rs:1271-1282 `on_input`](../crates/rakukan-tsf/src/tsf/factory.rs#L1271-L1282) が hot path の代表で、`guard.as_mut()` が `None` なら `return Ok(true)` で**キー入力を黙って捨てている**。reload 中や初回起動中に打鍵したキーが全部消える。

```rust
fn on_input(&self, c: char, ctx: ITfContext, tid: u32, sink: ITfCompositionSink,
            mut guard: crate::engine::state::EngineGuard) -> Result<bool> {
    let engine = match guard.as_mut() {
        Some(e) => e,
        None => return Ok(true),  // ← 握り潰し
    };
    ...
}
```

同種の握り潰しが factory.rs で多数存在（`return Ok(true)` は 30 件以上。ほとんどは合法だが、`guard.as_mut() = None` から直接 return しているパスが複数あるため全経路 grep が必要）。

**対策**:

- engine が無い間、keystroke をかな変換だけ TSF 側ローカルで処理し composition に積む
  - ローマ字 → ひらがな変換は engine なしでも可能（`rakukan-engine` の `RomajiConverter` を TSF 側でも使えるようにする、または独立した軽量実装を持つ）
- Enter: 積んだひらがなをそのままコミット（LLM 変換なし）
- Space / 変換キー: 候補ウィンドウに「⏳ エンジン起動中のため変換できません」を出して入力継続
- engine が ready に戻ったら、次のキー入力から通常経路へ

**簡略版（最低限）**:

- まずは「握り潰しをやめ、composition に積む → ready 復帰後に engine へ注入」だけ実装
- `RomajiConverter` を TSF 側に持たずとも、engine_host が ready になった時点で `push_char` の遅延適用で済む

**完了条件（T-HOST4）**:

- host 再起動中に 20 文字打っても 1 文字も失われない
- Enter でそのままひらがな確定される、または engine ready 後に変換ダイアログに入れる

**文言案**:

- キャレット近傍: `⏳ モデル読込中…` / `⏳ 辞書読込中…`（短く）
- 候補ウィンドウ: `LLM モデルを読み込み中です（○秒経過）。変換候補は少し後に表示されます。`
- エラー時: `❌ モデル読込失敗。ログを確認してください。` + エラー詳細を `engine.last_error()` から

**実装の切り出し**:

- `mode_indicator` をリネーム or 汎用化して「カーソル近傍に短いテキストを一定時間表示」できるようにする
- 既存の単一文字ロジック（「あ」「ア」「A」）はそのまま残し、新たに `show_loading_indicator(text, auto_hide_ms)` を追加する方針が無難
- ラッチ false → true の遷移をフックして `hide()` する

### 完了条件

- WinUI 保存を連打（10 回以上）しても `rakukan-engine-host.exe` の PID が毎回健全に入れ替わり、crash ダンプが出ない
- **読込中 UI インジケータがキャレット近傍に表示され、ready に戻ったら自動で消える**（T-HOST3）
- 10 秒 / 30 秒 / 60 秒の段階表示が切り替わる（手動で host kill して長時間状態を観測して確認）
- **読込中に打鍵した文字が 1 文字も消えない**（T-HOST4）
- 保存 → 変換再開までの所要時間が warm cache で **10 秒以内**（ユーザ許容枠）
- `rakukan-engine-host.log` に `rpc: Reload requested, dropping current engine` が出現しない（restart 経路に完全移行していることの確認）
- 既存のライブ変換 / Space 変換 / Enter 確定が退行しない

### リスク

- **小**: 既存の host auto-spawn 経路を再利用するだけなので、新規バグの入り込む面積が小さい
- 懸念点は graceful shutdown 時の `learn_history.bin` 保存整合性 → 既に 0.6.5 で同期書き込み（atomic rename）化済みなので追加対応不要
- `user_dict.toml` は WinUI が書き込んでから SignalReload するので順序問題なし

### 想定工数

- T-HOST1: 半日（protocol / server / client / state.rs の 4 箇所変更）
- T-HOST2: 計測 30 分 + 10 秒超えだった場合の (a)(b) 検討は別タスク化
- T-HOST3: 半日（mode_indicator 汎用化 + 経過時間タイマー分岐 + 段階表示）
- T-HOST4: 半日（hot path の握り潰し撤去 + 遅延 push バッファ）
- 合計: 2 日前後

### M1.5 との優先順位

- M1.6 は **crash を伴う** 問題で優先度が高い
- 並行作業可能（M1.5 は engine/backend + TSF、M1.6 は host + RPC protocol で触る領域が別）

### リリース戦略

**v0.7.1 の中核**（v0.7.0 から繰り延べ: 2026-04-24）。v0.7.0 リリース時には M1.5/M1.7/M1.8 の bug fix 4 件が先行着手で完成し、M1.6 は着手未了のため後送り。M1.6 全体は v0.7.1 で独立した minor リリースとして出す。

minor bump の根拠:

- ユーザから見た動作変化がある（host 再起動化、読込中 UI 表示、握り潰し対策）
- RPC protocol に `Request::Shutdown` を追加（後方互換は保たれるが protocol version bump 検討）

---

## 4c. M1.7 — ブラウザ入力モード保持の修正

### 目的

ブラウザ（Chrome / Edge / Firefox）で入力モード（ひらがな / カタカナ / 英数）を変更しても、タブ切替やページ遷移で `config.input.default_mode` へ戻ってしまう不具合を修正する。

### 症状

- ブラウザで「英数」に切り替え → 別タブへ移動 → 戻ってくると「ひらがな」（デフォルト）に戻る
- ページ内リンクで遷移しただけでも同様
- Explorer や WinUI アプリでは発生せず、ブラウザ系で顕著

### 原因（確定・2026-04-24 コード照合）

[state.rs:1314-1421](../crates/rakukan-tsf/src/engine/state.rs#L1314) の `doc_mode` ストアは 3 段階フォールバック（`dm_modes` → `hwnd_modes` → `config.default_mode`）で、モード保存は `OnSetFocus(prev=A, next=B)` の遅延処理（`WM_APP_FOCUS_CHANGED`）で行う。

ブラウザはタブ切替・ページ遷移で頻繁に DocumentManager を破棄/再作成するため、以下の race が発生する:

1. 旧 DM 破棄 → [factory.rs:4499-4511](../crates/rakukan-tsf/src/tsf/factory.rs#L4499-L4511) `OnUninitDocumentMgr(old_dm)` **同期**発火
2. [state.rs:1425-1431 `doc_mode_remove`](../crates/rakukan-tsf/src/engine/state.rs#L1425-L1431) が `dm_modes.remove(&dm_ptr)` + `dm_to_hwnd.remove(&dm_ptr)` を**無条件削除**（退避コピー無し）
3. 直後に [factory.rs:4513-4539 `OnSetFocus`](../crates/rakukan-tsf/src/tsf/factory.rs#L4513-L4539) → [candidate_window.rs:594 `post_focus_changed`](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L594) → `WM_APP_FOCUS_CHANGED` キュー → [`handle_pending_focus_changes`](../crates/rakukan-tsf/src/tsf/candidate_window.rs) → `process_focus_change` → `doc_mode_on_focus_change(prev=old, next=new, hwnd)` 処理
4. [state.rs:1361](../crates/rakukan-tsf/src/engine/state.rs#L1361) の `dm_to_hwnd.get(&old_dm)` が **None** を返す（step 2 で削除済み）
5. `hwnd_modes` への保存がスキップ → HWND フォールバックの情報がどこにも残らない
6. 新 DM が同じ HWND で作られても引ける情報が無く → `config.default_mode` へ戻る

副次要因:

- モード変更は [state.rs:553-558 `IMEState::set_mode`](../crates/rakukan-tsf/src/engine/state.rs#L553-L558) 経由で `AtomicU8` にのみ反映され、store へは focus-out タイミングでしか書かれない → 焦点を外さずに DM が破棄されると最新モードが永久に失われる
- [factory.rs:4536 `GetForegroundWindow()`](../crates/rakukan-tsf/src/tsf/factory.rs#L4536) は子 HWND を返すケースがあり、`hwnd_modes` が fragment する可能性（Chrome の `Chrome_WidgetWin_1` vs 描画子 HWND）
- Electron / CEF で OnSetFocus が来ないケース

### 作業

#### T-MODE1: `doc_mode_remove` で破棄前に HWND へ退避（最優先）

[state.rs:1425 `doc_mode_remove`](../crates/rakukan-tsf/src/engine/state.rs#L1425) で、削除前に `dm_modes[dm_ptr]` を `hwnd_modes[hwnd]` にコピーする:

```rust
pub fn doc_mode_remove(dm_ptr: usize) {
    if let Ok(mut store) = DOC_MODE_STORE.try_lock() {
        // 破棄前に HWND へ退避（F1 race 対策）
        if let (Some(&mode), Some(&hwnd)) =
            (store.dm_modes.get(&dm_ptr), store.dm_to_hwnd.get(&dm_ptr))
        {
            if hwnd != 0 {
                store.hwnd_modes.insert(hwnd, mode);
            }
        }
        store.dm_modes.remove(&dm_ptr);
        store.dm_to_hwnd.remove(&dm_ptr);
    }
}
```

**効果**: これだけでブラウザの大半のケースが救われる見込み（DM 破棄 → 新 DM 再作成時に HWND フォールバックが常に効く）。

**リスク**: 極小。既存ロジックの削除前に 1 段コピーを足すだけ。

**想定工数**: 15 分

#### T-MODE2: モード変更ごとに store を即時更新

**2026-04-24 コード照合の結果**: 全モード変更は [state.rs:553-558 `IMEState::set_mode`](../crates/rakukan-tsf/src/engine/state.rs#L553-L558) に集約されており、ここで `input_mode_set_atomic()` が呼ばれる。**唯一の収斂点で 1 箇所フックすれば全経路をカバーできる**。

`IMEState::set_mode` を呼ぶ 8 箇所:

| 呼び出し元 | 位置 | 経路名 |
| --- | --- | --- |
| 言語バー | [factory.rs:127](../crates/rakukan-tsf/src/tsf/factory.rs#L127) | `apply_langbar_mode` |
| Activate 初期化 | [factory.rs:521](../crates/rakukan-tsf/src/tsf/factory.rs#L521) | `OnActivate` |
| IME トグル | [factory.rs:3073](../crates/rakukan-tsf/src/tsf/factory.rs#L3073) | `on_ime_toggle` |
| IME Off | [factory.rs:3138](../crates/rakukan-tsf/src/tsf/factory.rs#L3138) | `on_ime_off` |
| IME On → Hiragana | [factory.rs:3165](../crates/rakukan-tsf/src/tsf/factory.rs#L3165) | `on_ime_on` |
| モード切替 Hiragana | [factory.rs:3210](../crates/rakukan-tsf/src/tsf/factory.rs#L3210) | `on_mode_hiragana` |
| モード切替 Katakana | [factory.rs:3244](../crates/rakukan-tsf/src/tsf/factory.rs#L3244) | `on_mode_katakana` |
| フォーカス変化後の自動適用 | [candidate_window.rs:665](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L665) | `process_focus_change` |

**作業**:

1. `candidate_window.rs` の thread_local に `TL_CURRENT_DM: Cell<usize>` / `TL_CURRENT_HWND: Cell<usize>` を追加
2. [handle_pending_focus_changes](../crates/rakukan-tsf/src/tsf/candidate_window.rs) の末尾（`process_focus_change` 呼び出し後）で `TL_CURRENT_DM.set(fc.next_ptr)` / `TL_CURRENT_HWND.set(fc.hwnd_val)`
3. `candidate_window.rs` に公開 getter `pub fn current_dm_hwnd() -> (usize, usize)` を追加
4. `state.rs` に `doc_mode_remember_current(mode: InputMode)` を追加:

    ```rust
    pub fn doc_mode_remember_current(mode: InputMode) {
        let (dm, hwnd) = candidate_window::current_dm_hwnd();
        if let Ok(mut store) = DOC_MODE_STORE.try_lock() {
            if dm != 0 { store.dm_modes.insert(dm, mode); }
            if hwnd != 0 { store.hwnd_modes.insert(hwnd, mode); }
        }
    }
    ```

5. [state.rs:553-558 `IMEState::set_mode`](../crates/rakukan-tsf/src/engine/state.rs#L553-L558) の末尾（`input_mode_set_atomic(mode);` の直後）で `doc_mode_remember_current(mode);` を呼ぶ — **これだけで全 8 経路がカバーされる**
6. ⚠️ process_focus_change（#8）は元々 `set_mode` → 新 focus DM へのモード適用なので、T-MODE2 の対象外（フォーカス切替直後は復元経路で既に store に反映済み）。むしろ呼び出し時点で TL_CURRENT_DM が更新済みかの順序検証が要

**効果**: モード変更の瞬間に store が最新化される → focus-out を経由しなくても失われない。

**リスク**: 小（1 箇所フック + TL 追加、既存 8 呼び出し元は書き換え不要）

**想定工数**: 3〜4 時間（配線と実機確認）

#### T-MODE3: HWND を `GA_ROOT` へ正規化

`GetForegroundWindow()` 呼び出し箇所（2026-04-24 grep 結果、3 箇所）:

- [factory.rs:4536](../crates/rakukan-tsf/src/tsf/factory.rs#L4536) `OnSetFocus` の hwnd 取得 — **最重要**
- [factory.rs:244](../crates/rakukan-tsf/src/tsf/factory.rs#L244) 付近（要確認）
- [factory.rs:503](../crates/rakukan-tsf/src/tsf/factory.rs#L503) 付近 Activate 初期化

いずれも子 HWND が返ると `hwnd_modes` が fragment する。`GetAncestor(hwnd, GA_ROOT)` でルート化する。

```rust
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GA_ROOT};
let hwnd_raw = unsafe { GetForegroundWindow() };
let hwnd_root = unsafe { GetAncestor(hwnd_raw, GA_ROOT) };
let hwnd_val = hwnd_root.0 as usize;
```

**効果**: ブラウザが内部で子 HWND を切り替えても `hwnd_modes` が安定して引ける。

**リスク**: 極小（windows crate の `GetAncestor` / `GA_ROOT` はバインド存在、副作用なし）

**想定工数**: 15〜30 分（3 箇所同時修正）

#### T-MODE4: exe 名フォールバック段の追加（任意・様子見）

DM も HWND も両方変わる最悪ケース（ブラウザプロセス再起動等）向けに、プロセス名をキーとする第 3 段フォールバックを追加する。

- `exe_modes: HashMap<String, InputMode>` を `ModeStore` に追加
- `GetWindowThreadProcessId` → `QueryFullProcessImageNameW` で exe パス取得、basename を小文字化してキー化
- 保存経路: 既存の `dm_modes` / `hwnd_modes` と同時に更新
- 復元経路: DM > HWND > Exe > default の優先順

**着手条件**: T-MODE1〜3 を入れて実機テスト後、それでも保持されないケースが残った場合のみ。

**想定工数**: 半日

#### T-MODE5: mode_store 永続化（任意・様子見）

現状は in-memory のみ。プロセス終了で忘れる。`%APPDATA%\rakukan\mode_store.toml` に exe 単位の map を保存すれば再起動を跨げる。

**着手条件**: ユーザから「再起動のたびにリセットされる」要望が出た場合のみ。

**想定工数**: 半日

### 完了条件

- ブラウザで「英数」に切り替え → タブ切替 → 戻ってきた時に「英数」が維持されている
- ページ遷移・リンククリックでもモード維持
- 既存の Explorer / WinUI / VSCode / メモ帳等でのモード保持が退行しない
- `rakukan.log` で `doc_mode: saved mode=... hwnd=...` が DM 破棄前に出ていることを確認

### リスク

- **小**（既存ストアの保存タイミング / キー正規化の改善のみ、既存のフォールバック構造は維持）
- T-MODE1 は純粋な退避追加、T-MODE3 はルート HWND 化で影響は限定的
- T-MODE2 は変更経路のフックを漏らすと一部モードだけ反映されないため、全経路を grep で列挙する

### 想定工数

- T-MODE1: 15 分
- T-MODE2: 半日
- T-MODE3: 15 分
- T-MODE4: 半日（任意）
- T-MODE5: 半日（任意）
- 必達合計（T-MODE1〜3）: **1 日前後**

### リリース戦略

- **T-MODE1 を v0.7.0 に即同梱**: 15 分で入りリスク極小、体感改善が大きい
- **T-MODE2 + T-MODE3 を v0.7.1 に**: 実装分量が少し大きく、モード変更経路の列挙が要るため一呼吸置く
- **T-MODE4 / T-MODE5 は様子見**: T-MODE1〜3 で解消しきらない場合のみ開封

### 診断のために事前取得したい情報

1. `%LOCALAPPDATA%\rakukan\rakukan.log` に `doc_mode: saved/restored` のログが出ているか
2. ブラウザで「モード変更 → タブ切替 → 戻ってきた」時のログシーケンスで、`doc_mode_remove` が `OnSetFocus` より先に走っているか
3. `hwnd_modes` に対象 HWND のエントリが残っているか（tracing を debug level で拾う）

---

## 4d. M1.8 — ライブ変換中の中間文字消失修正

### 目的

ライブ変換中に、入力文字列の**末尾ではなく中ほど**の文字が消える不具合を修正する。M1.5（LLM 早期 EOS による末尾欠落）や M1.6 T-HOST4（host ロード中の全キー握り潰し）とは別経路の bug で、stale な preview が新しい composition に適用されて中間部分が上書きされる race が根本原因。

### 症状

- 「あいうえお」と打つつもりが、preview が「あえお」のように中の「いう」が飛ぶ
- 打鍵は正しく届いているが、composition 書き換えで中ほどが消える
- 発生頻度はタイミング依存（速打ち / タイマー周期との噛み合い）

### 原因（候補、2026-04-24 コード照合）

#### F1: Phase1B キューの stale reading 再適用

[state.rs:748](../crates/rakukan-tsf/src/engine/state.rs#L748) の `LIVE_PREVIEW_QUEUE: LazyLock<Mutex<Option<String>>>` に [candidate_window.rs:1242-1249](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L1242-L1249) で preview が書き込まれ、[factory.rs:999-1026](../crates/rakukan-tsf/src/tsf/factory.rs#L999-L1026) で後続のキー入力ハンドル時に取り出して apply する。だが **reading は既に次の文字を含んでおり、preview は 1 世代前のまま**。これに対して [text_util::suffix_after_prefix_or_empty](../crates/rakukan-tsf/src/tsf/factory.rs#L1002) で suffix 計算が崩れ、中間が脱落する。

再現シーケンス例:

1. キー `a` → reading=`あ`、timer が preview=`あ` を `LIVE_PREVIEW_QUEUE` に積む
2. キー `i` 到着 → hot path が `LIVE_PREVIEW_QUEUE` からキュー消費
3. この時点で reading=`あい` だが preview=`あ`（古い世代）
4. suffix 計算ズレで display=`あ` + 不正 suffix → 中間の `い` が消える

**備考**: `LIVE_PREVIEW_QUEUE` は `Option<String>` 型で、preview 文字列しか保持していない。世代識別子を持たせるには型を拡張する必要あり（下記 T-MID1 で対応）。

#### F2: Phase1A / Phase1B の SetText 二重適用

[candidate_window.rs:1194-1197](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L1194-L1197) の Phase1A の `range.SetText(ec, 0, &text_w)` と、[factory.rs:1026](../crates/rakukan-tsf/src/tsf/factory.rs#L1026) の Phase1B 内で呼ばれる `update_composition` が独立に SetText を走らせる可能性がある。`SetText(ec, 0, ...)` は range 全体への絶対置換なので、**後発の書き込みが先発の結果を全上書き**し、間に入った新キーが消える。

再現シーケンス例:

1. Phase1A が preview=`あい` を SetText 実行開始
2. キー `う` が同時到着、reading=`あいう` へ
3. Phase1B apply が古い preview=`あい` を SetText で上書き
4. composition は `あい` のまま、`う` が消失

#### F3: EditSession 内で stale composition への書き込み

[factory.rs:3874-3914](../crates/rakukan-tsf/src/tsf/factory.rs#L3874-L3914) の `update_composition` は [composition_clone](../crates/rakukan-tsf/src/engine/state.rs#L624) の snapshot を取るが、**EditSession クロージャ内で再確認していない**。focus 変更や OnEndComposition で composition が既に破棄されていた場合、stale な range に対して SetText が走り、TSF の内部整合が崩れて中間が飛ぶ可能性。

`composition_clone` 自体は [invalidate_composition_for_dm](../crates/rakukan-tsf/src/engine/state.rs#L636) による stale フラグを持っているが、EditSession 実行中に別スレッドから stale 化された場合の再検査は未実装。

#### F4: `display = preview + suffix` 境界計算のレース

[factory.rs:1309-1325](../crates/rakukan-tsf/src/tsf/factory.rs#L1309-L1325) で `display_hira = preview + suffix` を組むが、preview は前 timer 周期、suffix は最新 reading からの差分。セッションに [session.set_live_conv(reading, preview)](../crates/rakukan-tsf/src/tsf/factory.rs#L1016) で `(reading, preview)` ペアで保存されるため、次回キー入力で古い preview が再利用される。

**補足**: F4 は実装上「reading は単調増加」のため、厳密な「中間消失」ではなく「append 遅延」に見える可能性もある。F1〜F3 が主犯の候補、F4 は副次的。

### 作業

#### T-MID1: Phase1B キューに gen タグを付与（最優先）

**目的**: F1 の race を潰す。stale な preview を apply せず discard する。

**2026-04-24 コード照合の前提**:

- [state.rs:748](../crates/rakukan-tsf/src/engine/state.rs#L748) 現状 `pub static LIVE_PREVIEW_QUEUE: LazyLock<Mutex<Option<String>>>` → 型を拡張する必要あり
- [state.rs:749](../crates/rakukan-tsf/src/engine/state.rs#L749) `LIVE_PREVIEW_READY: AtomicBool` は既存、そのまま使う

**作業**:

1. [state.rs:748](../crates/rakukan-tsf/src/engine/state.rs#L748) の型を拡張:

   ```rust
   // 旧: Mutex<Option<String>>
   // 新: Mutex<Option<PreviewEntry>>
   pub struct PreviewEntry {
       pub preview: String,
       pub reading: String,       // キュー書き込み時の reading（gen の代替として使う軽量版）
       pub gen_when_requested: u32,
   }
   pub static LIVE_PREVIEW_QUEUE: LazyLock<Mutex<Option<PreviewEntry>>>
       = LazyLock::new(|| Mutex::new(None));
   ```

2. `state.rs` に `pub static LIVE_CONV_GEN: AtomicU32 = AtomicU32::new(0);` を追加
3. [state.rs:553-558 `IMEState::set_mode`](../crates/rakukan-tsf/src/engine/state.rs#L553-L558) ではなく、**composition / reading が変わる経路で** `LIVE_CONV_GEN.fetch_add(1, Release)` を呼ぶ:
   - [factory.rs on_input](../crates/rakukan-tsf/src/tsf/factory.rs#L1271) の key push の直前または直後
   - `on_backspace`、composition commit、live_conv 終了など全ての reading 更新経路（全列挙要）
4. [candidate_window.rs:1242-1249](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L1242-L1249) の Phase1B キュー書き込みで `gen_when_requested = LIVE_CONV_GEN.load(Acquire)` を記録し `PreviewEntry` で保存
5. [factory.rs:985-1029](../crates/rakukan-tsf/src/tsf/factory.rs#L985-L1029) の apply 経路で取り出した `PreviewEntry.gen_when_requested` と現在の `LIVE_CONV_GEN.load(Acquire)` を比較:
   - `queue_gen < current_gen` なら discard（`tracing::warn!("[Live] Phase1B: discarded stale preview gen={qg} current={cg}")`）
6. 追加で `PreviewEntry.reading` と現在の `engine.hiragana_text()` を比較し不一致も discard（二重安全策）

**効果**: キュー消費時点で自分が古い世代であることを検知できるので、新キー後の stale apply が起きない。

**リスク**: 小〜中。gen increment のタイミングを漏らすと race が残るため、**全ての「reading が変わる経路」を grep で列挙してから着手**。`engine.input_char` / `engine.backspace` / `engine.flush_pending_n` / commit 経路など想定。

**想定工数**: 半日（経路列挙 1 時間 + 実装 2〜3 時間）

#### T-MID2: EditSession 入口で composition stale check

**目的**: F3 の stale composition 参照を潰す。

**2026-04-24 コード照合**:

- [state.rs:624 `composition_clone`](../crates/rakukan-tsf/src/engine/state.rs#L624) が snapshot を返す
- [state.rs:636 `invalidate_composition_for_dm`](../crates/rakukan-tsf/src/engine/state.rs#L636) が DM 単位で stale flag を立てる
- [factory.rs:3874-3914 `update_composition`](../crates/rakukan-tsf/src/tsf/factory.rs#L3874-L3914) 内の EditSession クロージャは `composition_clone` の snapshot を外側で取得し、クロージャ内では**再検査なし**

**作業**:

1. [factory.rs:3874 update_composition](../crates/rakukan-tsf/src/tsf/factory.rs#L3874) で外側取得した `comp_a` に加え、EditSession クロージャ先頭で `comp_b = composition_clone()?` を再呼び出し
2. `comp_a` と `comp_b` の `as_raw()` を `usize` で比較し、不一致（dm_ptr 変更 / None 化）なら `return Ok(())` で no-op
3. tracing で `update_composition: stale snapshot, abort SetText` をログ

**効果**: EditSession 実行までの間に composition が破棄/置換された場合に誤書き込みしない。

**リスク**: 小。単純な stale 判定追加、既存の挙動を縮退しない（no-op にするだけ）。

**想定工数**: 半日

#### T-MID3: Phase1A / Phase1B の SetText 二重適用排他化

**目的**: F2 の二重 SetText を防ぐ。

**作業（案 A: mutex 排他）**:

- `COMPOSITION_APPLY_LOCK: Mutex<()>` を state に追加
- Phase1A / Phase1B の SetText 実行を try_lock で囲み、ロック中の呼び出しは skip
- この間 preview apply は取りこぼすが、最新 gen による次回 apply が勝つので整合は保てる

**作業（案 B: apply キューに統一、単一 consumer 化）**:

- Phase1A / Phase1B を共通の `apply_composition_update(snapshot, gen)` に集約
- consumer は 1 本のスレッド（TSF STA）で WM_APP_COMPOSITION_APPLY を拾って順次処理
- 排他は message loop の自然な直列化に任せる

**推奨**: 案 B（M2 の T1-B 分解と方向が一致、後続の refactor が楽）。ただし即効性を取るなら案 A。

**効果**: 二重 SetText による中間上書きが起きない。

**リスク**: 中（案 A）〜中〜大（案 B）。ロック設計 or message 経路の引き回しを誤ると deadlock or 適用抜け。

**想定工数**: 半日（案 A）/ 1 日（案 B）

### 完了条件

- 速打ち（毎秒 10 打鍵以上）で「あいうえおかきくけこ」相当の入力を 10 回繰り返して、composition に文字脱落が 0 回
- tracing で `live_conv: discarded stale preview` が race 時に出ていることを確認
- 既存のライブ変換機能（通常速度・英数・カタカナ）が退行しない
- M1.5 / M1.6 / M1.7 で入れた修正と競合しない

### リスク

- **中**（race 修正は挙動テストで追いにくく、再現条件が速度依存）
- T-MID1 の gen increment 経路を漏らすと部分的にしか効かない
- T-MID3 の案 B は実質 M2 T1-B 分解の前倒し → 範囲を慎重に切らないとスコープが膨らむ

### 想定工数

- T-MID1: 半日
- T-MID2: 半日
- T-MID3: 半日（案 A）/ 1 日（案 B）
- 合計: 1〜2 日

### M2 との関係

M2 の §18.3 採用項目「bg_peek/take API 分離」「session_nonce + gen」と **根本原因が同一**（stale 世代 discard）。M1.8 は user-facing bug として先行バックポートする位置付け。

- M1.8 T-MID1 の gen 機構は、M2 の `session_nonce + gen` の部分実装として機能する
- M1.8 で gen を導入しておけば、M2 で `session_nonce` を追加するだけで完成度が上がる
- M2 のスコープから「gen のみ」部分を抜き、M1.8 完了後に M2 は `session_nonce` 追加 + API 分離のみに縮小できる

### 診断のために事前取得したい情報

1. `%LOCALAPPDATA%\rakukan\rakukan.log` で `live_conv:` で始まるログのシーケンス（どの順で Phase1A / Phase1B が走り、reading がどう変わっているか）
2. 中抜け発生時の `reading.len()` / `preview.len()` / 消失文字のオフセット
3. 発生デバイスの打鍵速度（毎秒何打鍵で発生するか）

### リリース戦略

- **T-MID1 を v0.7.0 に同梱** ✅ リリース済（2026-04-24）。半日の作業で最大効果。M1.5 T-BUG2 / M1.7 T-MODE1 / T-MODE3 と並んで v0.7.0 は bug fix 集中リリースの 4 本柱
- **T-MID2 + T-MID3 を v0.7.1 に**: T-MID3 案 B（apply 経路統一）を採用する場合は実装量がやや大きいため一呼吸置く
- **v0.7.1 以降も再発があれば M2 で session_nonce を追加**（完全対策）

---

## 5. M2 — ライブ変換ロジックの可読性向上

### 5.1 T1-B: `on_live_timer` (248 行) の分解

**現状（2026-04-24 コード照合）**: [candidate_window.rs:1009-1256](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L1009-L1256) の `on_live_timer` が 6 段階の処理を 1 関数で抱えている（248 行、内部の段階範囲は下表）

| 段階 | 行範囲 | 処理 |
| --- | --- | --- |
| pass_debounce | 1017-1023 | `LIVE_DEBOUNCE_CFG_MS` チェック |
| probe_engine | 1032-1051 | `engine_try_get` + hiragana 取得 |
| ensure_bg_running | 1052-1105 | `bg_status` 確認 + `bg_start` 起動 |
| fetch_preview | 1109-1136 | `bg_take_candidates` で preview 取得 |
| build_apply_snapshot | 1138-1148 | `display_shown` 組み立て |
| try_apply_phase1a + queue_phase1b | 1150-1255 | `RequestEditSession` or `LIVE_PREVIEW_QUEUE` に積む |

**作業**:

```rust
fn on_live_timer() {
    if !pass_debounce() { return; }
    let probe = match probe_engine() { Some(p) => p, None => return };
    if !ensure_bg_running(&probe) { return; }
    let preview = match fetch_preview(&probe) { Some(p) => p, None => return };
    let snapshot = match build_apply_snapshot(preview) { Some(s) => s, None => return };
    if !try_apply_phase1a(snapshot) {
        queue_phase1b(snapshot);
    }
}
```

各サブ関数を 30〜50 行に抑える。

**完了条件**:

- `cargo check` PASS
- 既存ライブ変換シナリオ（typing → preview 表示 → commit）の挙動が同一であること（手動 1 サイクル確認）

**リスク**: 中（純粋分解だがロック取得順序を保つ必要あり）

**想定工数**: 半日

### 5.2 §18.3 採用: `bg_peek_result` / `bg_take_result` API 分離

**現状（2026-04-24 コード照合）**: `bg_take_candidates(key)` は [lib.rs:640](../crates/rakukan-engine/src/lib.rs#L640) に単一メソッドで存在し、「結果取り出し」「converter 返却」「ユーザー辞書マージ」を兼ねており、live preview と通常変換が干渉する。peek/take 分離は未実装。

**作業** ([LIVE_CONV_REDESIGN_REVISED.md §6.2](LIVE_CONV_REDESIGN_REVISED.md) より):

```rust
// engine 側
pub fn bg_peek_result(&self) -> Option<&BgResult>;       // 状態を進めない
pub fn bg_take_result(&mut self) -> Option<BgResult>;    // 状態を進める
pub fn merge_candidates_for_preview(&self, ...) -> Vec<String>;
pub fn merge_candidates_for_commit(&self, ...) -> Vec<String>;
```

呼び出し側:

- live preview → `bg_peek_result`
- Space 変換 / Enter fallback → `bg_take_result`

**完了条件**: 既存テスト PASS、preview と commit の干渉が起きないこと

**リスク**: 中（engine API 変更に伴う TSF 側の置換が ~10 箇所）

**想定工数**: 1 日

### 5.3 §18.3 採用: `session_nonce + gen` で stale 結果 discard

**目的**: 旧セッション / 旧入力世代の worker 結果を確実に捨てる

**M1.8 T-MID1 との関係**: M1.8 で `LIVE_CONV_GEN: AtomicU32` と `PreviewEntry.gen_when_requested` を先行導入する。M2 で残るのは**`session_nonce`**（composition 開始ごとの identity 識別子）の追加と、engine 側 `BgResult` への nonce 伝播。

**作業**:

- `engine/state.rs` または `SessionState` に `session_nonce: AtomicU64` を追加（composition 開始ごとに `fetch_add(1, Release)`）
- `BgResult` / `bg_take_candidates` 返却型に `session_nonce: u64` フィールド追加
- preview 取得側で session_nonce / gen 不一致なら破棄（M1.8 T-MID1 の PreviewEntry に nonce も追加する拡張として扱える）
- `EngineConfig` には触らない（composition 毎の identity は state 側で管理）

**完了条件**: stale result が apply されないこと（debug ログで `session_nonce mismatch` を再現して確認）

**リスク**: 中（M1.8 で入れる gen と併用するため、組合せテストが要）

**想定工数**: 1 日（M1.8 完了済みなら 半日に縮小可能）

### M2 完了条件

- T1-B, §6.2 採用, session_nonce 採用 完了
- 機能テスト: ライブ変換、Space 変換、Enter fallback すべて動作
- 1 時間程度の使用テスト で回帰なし

---

## 6. M3 — factory.rs 分割（T1-A）

### 目的

**4625 行（2026-04-24 実測）** の god file を機能別ファイルに分割し、可読性と保守性を向上させる。  
**機能変更は一切行わない**。純粋なファイル切り出しのみ。

### 推奨分割（2026-04-24 実測値で再見積もり）

| 分割先ファイル | 対象関数群 | 現在の行位置 | 実測行数 | 旧見積もり |
| --- | --- | --- | --- | --- |
| `factory.rs`（核 + COM impl） | impl Activate / Deactivate / IClassFactory 等 | 326-595 + 1405-4625 | ~500+ | ~500 ✓ |
| `factory/activate.rs` | `fn Activate` + 補助 | 348-544 | 197 | ~250（過大） |
| `factory/deactivate.rs`（新案） | `fn Deactivate` | 545-595 | 51 | — |
| `factory/dispatch.rs` | `fn handle_action` | 923-1259 | 337 | ~300（ほぼ一致） |
| `factory/on_input.rs` | `on_input` + `on_input_raw` + `on_full_width_space` | 1271-1549 | 279 | ~400（過大） |
| `factory/on_convert.rs` | `on_convert` + `on_commit_raw` | 1573-2501 | **929** ⚠️ | ~600（大幅過少） |
| `factory/on_compose.rs` | `update_composition` + helpers | 3868-4101+ | ~400 | ~500（ほぼ一致） |
| `factory/on_kana_latin.rs` | F6-F10 kana/latin ハンドラ | 2720-2878 | 159 | ~300（過大） |
| `factory/on_segment.rs` | segment_move / shrink / extend | 3393-3655 | 263 | ~400（過大） |

**再調整が必要な項目**:

- `on_convert.rs` は実測 929 行で、記載の 600 行想定を大幅超過。`on_convert` (708 行) と `on_commit_raw` (221 行) で**さらに分割**するか、`on_convert` 単独の内部を分解するのが妥当
- `activate.rs` と `deactivate.rs` は実測 197 + 51 で単独ファイル化しない選択肢もあり（併合 248 行で 1 ファイル扱い）
- `on_kana_latin.rs` と `on_segment.rs` は合わせて ~420 行で、**統合して `factory/edit_ops.rs` に**することも検討可

### 作業手順

1. ブランチを切る (`refactor/factory-split`)
2. `factory.rs` から関数を 1 グループずつ別ファイルに移動
3. 各ステップで `cargo check` PASS を確認
4. 移動のみ、ロジック変更や名前変更は **しない**
5. visibility (`pub(crate)` など) は最小限の調整のみ

### 完了条件

- `cargo check`, `cargo test` PASS
- `cargo make build-tsf` で DLL ビルド PASS
- 既存の動作が同一（30 分の手動テスト）

### リスク

- **小**（純粋な切り出し）
- 注意点: 同時にロジック改善を入れると blame / diff が壊れる

### 想定工数

1〜2 日

---

## 7. M4 — ライブ変換状態の構造体集約（T2）

### 目的

7 ヶ所に散らばっているライブ変換状態を `LiveConvSession` 構造体（thread-local）に集約する。

[LIVE_CONV_REDESIGN_REVISED.md §5.1, §5.2, §18.3 保留](LIVE_CONV_REDESIGN_REVISED.md) の方針に沿う。

### 集約対象（2026-04-24 コード照合）

| 現状の場所 | 実在位置 | 移動先 |
| --- | --- | --- |
| `TL_LIVE_CTX` | [candidate_window.rs:82](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L82) | `LiveConvSession.ctx` |
| `TL_LIVE_TID` | [candidate_window.rs:84](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L84) | `LiveConvSession.tid` |
| `TL_LIVE_DM_PTR` | [candidate_window.rs:87](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L87) | `LiveConvSession.composition_dm_ptr` |
| `LIVE_PREVIEW_QUEUE` | [state.rs:748](../crates/rakukan-tsf/src/engine/state.rs#L748) | 削除（pull モデルに変更）。**M1.8 T-MID1 で型拡張するので M4 時点で PreviewEntry を LiveConvSession に吸収** |
| `LIVE_PREVIEW_READY` | [state.rs:749](../crates/rakukan-tsf/src/engine/state.rs#L749) | 削除（`apply_requested: bool` に変更） |
| `SUPPRESS_LIVE_COMMIT_ONCE` | [state.rs:750](../crates/rakukan-tsf/src/engine/state.rs#L750) | `LiveConvSession.suppress_next_commit` |
| `LIVE_TIMER_FIRED_ONCE_STATIC` / `LIVE_LAST_INPUT_MS` / `LIVE_DEBOUNCE_CFG_MS` | `candidate_window.rs`（static atomics） | `LiveConvSession.{fired_once, last_input_ms}` + 設定値は static のまま |
| `SessionState::LiveConv { reading, preview }` | `state.rs` 中 | `LiveConvSession.last_preview` + `SessionState::LiveConv` の payload は最小化（reading のみ残す） |
| (M1.8 で新設) `LIVE_CONV_GEN` | M1.8 で追加予定 | `LiveConvSession.gen` |

### 作業手順

1. `crates/rakukan-tsf/src/tsf/live_session.rs` 新設
2. `LiveConvSession` 定義 + `TL_LIVE_SESSION: RefCell<Option<...>>`
3. ensure / dispose ヘルパ追加
4. 既存呼出元（`live_input_notify`, `on_live_timer`, `on_input` 等）を順次置換
5. 旧 `TL_LIVE_*` / `LIVE_PREVIEW_*` を削除

### 完了条件

- `cargo check` PASS
- ライブ変換の機能テスト全 PASS（特に focus 切替、commit、cancel）
- 1 日程度の使用で回帰なし

### リスク

- **中〜大**（ライブ変換の中枢を触る）
- v0.6.6 の crash が安定して発生しないことが大前提
- 段階的な PR に分けることを推奨

### 想定工数

3〜5 日

---

## 8. M5 — 追加対策（実機再発時のみ）

v0.6.6 + M1〜M4 完了後に Explorer crash が再発した場合のみ実施する。

### 8.1 WM_TIMER → PostMessage 化

[LIVE_CONV_REDESIGN_REVISED.md §8, §9.2](LIVE_CONV_REDESIGN_REVISED.md):

- `WM_RAKUKAN_LIVE_READY` を `RegisterWindowMessageW` で取得
- worker 完了時に `PostMessage(hwnd, WM_RAKUKAN_LIVE_READY, ...)`
- `wnd_proc` に新メッセージハンドラ追加
- `WM_TIMER` ベースの 50ms ポーリング廃止

**効果**: timer fire と RequestEditSession の間に他メッセージが入る余地が生まれ race window 縮小

**想定工数**: 1〜2 日

### 8.2 Explorer シェルクラスでの Phase1A 無効化

[LIVE_CONV_REDESIGN_REVISED.md §11](LIVE_CONV_REDESIGN_REVISED.md):

- `live_input_notify()` で `GetClassNameW` で window class 取得
- `Shell_TrayWnd` / `Progman` / `WorkerW` / `CabinetWClass` / `ExploreWClass` なら Phase1A スキップ
- `[live_conversion] disable_auto_apply_for_explorer` 設定で制御可能に

**効果**: Explorer crash の局所回避（既知の race を Explorer だけで無効化）  
**副作用**: Explorer 内のライブ変換が「キー入力時のみ反映」になる UX 劣化

**想定工数**: 半日

---

## 8a. 将来機能候補（0.8.x 以降）

0.7.x は安定性向上が主目的のため、以下の新機能は 0.7.x シリーズ完走後に 0.8.x の検討対象とする。

### 8a.1 M6 — 数字・記号入力の改善

#### M6.1: 数字間の区切り文字自動変換

**目的**: `2,400.5` のような数値表記をローマ字入力から自然に打てるようにする。現状は `、` / `。` がそのまま全角句読点として確定されるため、数値入力のたびに IME OFF / 記号切替が必要になる。

**仕様**:

- 数字（半角 / 全角）同士に挟まれた `、` を `,` に、`。` を `.` に自動変換
- 発火条件は「直前の文字が数字」かつ「次の入力が数字になり得る文脈」
  - 連続入力の途中は投機的に置換し、後続が数字でなかった場合は句読点に戻す（undoable）
- 設定 `[input] digit_separator_auto = true` でオンオフ切替

**関連箇所（想定）**:

- ローマ字 → かな変換層（`rakukan-engine` の `RomajiConverter` 付近）
- または TSF 側 composition 組み立て時に後処理

**リスク**: 小（純粋な文字列置換、数値文脈判定のみ）

**想定工数**: 1〜2 日

#### M6.2: 桁並び漢数字候補（二〇〇 形式）

**目的**: `200` → `二〇〇` のような、各桁を 1:1 で漢数字に置換した候補を出す。電話番号・年号・番地など位取り読みしない用途向け。

**仕様**:

- reading が全て数字（および `,` `.`）の場合、`0123456789` を `〇一二三四五六七八九` に 1:1 置換した候補を追加
  - 例: `2024` → `二〇二四`、`090` → `〇九〇`
- カンマ・ピリオドはそのまま保持（`2,400.5` → `二,四〇〇.五`）
- 半角 / 全角どちらの入力でも同じ結果

**関連箇所（想定）**:

- [digits.rs:76 `digit_candidates()`](../crates/rakukan-engine/src/digits.rs#L76) に桁並び漢数字 variant を追加
- [digits.rs:227 `extract_digits()`](../crates/rakukan-engine/src/digits.rs#L227) に `〇一〜九` → `0〜9` の逆変換を足し、`verify_digits_preserved` を通るようにする

**リスク**: 極小（1:1 マップ、位取り解析不要）

**想定工数**: 1〜2 時間

#### M6.3: 位取り漢数字候補（二百 形式）+ 大字候補

**状態**: v0.8.2 で通常漢数字候補を追加し、v0.8.4 で大字候補と候補順設定まで完了。

**目的**: `1234` → `千二百三十四` / `壱千弐百参拾四` のような位取り表記を候補に出し、文章中の数量表記や金額向け表記を楽に入力できるようにする。

**仕様**:

- reading が全て数字の場合、以下を候補化
  - 位取り漢数字: `千二百三十四`
  - 大字: `壱千弐百参拾四`
- 位取りルール:
  - `10` → `十`（`一十` ではない）
  - `100` → `百`、`1000` → `千`
  - `10000` → `一万`（万以上は先頭の `一` を落とさない）
  - `101` → `百一`（中間ゼロは `〇` 入れない）
  - 対応範囲: `u64` まで（京以上は切り捨て、または桁並び形式にフォールバック）
- カンマ入り（`2,400`）はカンマを除去してから変換
- 小数（`2.5`）は `二点五` 形式（整数部のみ位取り、小数部は桁並び）
- 設定 `[input] digit_candidates_order = ["arabic", "fullwidth", "positional", "per_digit", "daiji"]` で候補種別と順序を指定。指定しない種別は候補に出ない

**実装箇所**:

- [digits.rs](../crates/rakukan-engine/src/digits.rs): `to_kanji_positional` / `to_daiji_positional` / `digit_candidates` / 漢数字・大字の数字復元
- [lib.rs](../crates/rakukan-engine/src/lib.rs): `DigitCandidateKind` と `EngineConfig.digit_candidates_order`
- [conv_cache.rs](../crates/rakukan-engine/src/conv_cache.rs): live prefetch / BG 変換へ候補順設定を渡す
- [config.rs](../crates/rakukan-tsf/src/engine/config.rs): TSF 側 config.toml に `digit_candidates_order` を追加
- [state.rs](../crates/rakukan-tsf/src/engine/state.rs): engine-host 用 JSON へ `digit_candidates_order` を渡す

**検証**:

- `cargo test -p rakukan-engine --lib`
- `cargo check -p rakukan-tsf`

#### M6.4: 記号の半角 / 全角候補

**目的**: 数字・アルファベットと同様に、reading に混入する ASCII 記号（`!` `?` `#` `@` `(` `)` `+` `-` `*` `/` `:` `;` `<` `>` `=` 等）に対して、半角 / 全角の両方を変換候補として提示する。

**仕様**:

- 記号 run を新設し、半角 / 全角の 2 候補を生成
  - 例: `!` → `["!", "！"]`、`(test)` → `["(test)", "（test）"]`
- 対応範囲: ASCII `0x21`〜`0x7E`（space 除く）↔ 全角 `0xFF01`〜`0xFF5E`
  - 単純な `+0xFEE0` シフトで相互変換可能
- 既存の数字・アルファベット run と混在した reading でも各 run 独立に候補生成し `combine_runs` で合成
  - 例: `USB-C` → `USB-C` / `ＵＳＢ-Ｃ` / `USB－C` / `ＵＳＢ－Ｃ`（組合せ上限は既存の `limit * 2` 制限に従う）
- 例外とする記号（設定 or ハードコード）:
  - `、` `。` `「` `」` 等の **日本語句読点・括弧**: 変換対象外（既に日本語文字として扱う）
  - space: 変換対象外（compose 境界の扱いが複雑化するため）
- 数字と隣接する `,` `.` は M6.1 の自動変換と競合しないよう、**記号単独の run になるケースに限定**して候補化する

**関連箇所（想定）**:

- [digits.rs:34 `CharKind`](../crates/rakukan-engine/src/digits.rs#L34) に `Symbol` を追加
- [digits.rs:41 `classify_char`](../crates/rakukan-engine/src/digits.rs#L41) で ASCII 記号 / 全角記号を `Symbol` に分類
- `Run::Symbol(String)` を追加し、`is_literal()` を `true` に
- `to_halfwidth_symbol` / `to_fullwidth_symbol` / `symbol_candidates` を実装
- [digits.rs:241 `verify_digits_preserved`](../crates/rakukan-engine/src/digits.rs#L241) と同等の「記号が LLM に溶かされていないこと」を保証する verify は、**記号 run を literal として扱うので元々 LLM を経由しない** → 追加 verify 不要

**リスク**: 小（構造は Digit/Alpha と完全に並行、既存ロジックの拡張）

**想定工数**: 半日

### 8a.M6 完了条件

- `200` → `二〇〇` / `二百` が候補に出る
- `1234` → `一二三四` / `千二百三十四` / `壱千弐百参拾四` が候補に出る
- `2` `、` `4` と打つと `2,4` に、`2` `。` `5` と打つと `2.5` になる
- `USB-C` / `(test)` / `A+B` 等の ASCII 記号を含む reading で半角 / 全角の両候補が出る
- 既存のひらがな → 句読点入力が退行しない（数字文脈外では従来通り `、` `。`）

---

## 9. リファクタリング不要と判断したもの

| ファイル | 行数 | 理由 |
| --- | --- | --- |
| `text_util.rs` | 897 | ほぼ kana ↔ kata マッピングデータ |
| `keymap.rs` | 707 | preset 定義 + parser、低複雑度 |
| `engine/lib.rs` | 933 | 直近の test 追加が増やしただけ、本体は妥当 |
| `kanji/llamacpp.rs` | 828 | beam search 等のドメインロジック |
| `dict/store.rs` | 775 | 学習履歴 I/O の妥当な集約 |
| `settings/main.rs` | 2274 | win32 レガシー設定 UI、WinUI 移行で段階削減予定 |

---

## 10. 安全に進めるための共通ルール

1. **1 PR は単一目的に絞る** — リファクタリングと機能追加を混ぜない
2. **毎ステップで `cargo check` / `cargo test` PASS を確認**
3. **TSF 関連の変更は実機テストを伴う** — `cargo make build-tsf && cargo make install` → サインアウト → 再ログオン → 30 分使用
4. **コミットメッセージ規約**:
   - `refactor(tsf): split factory.rs into modules` — 純粋リファクタ
   - `feat(engine): add bg_peek_result / bg_take_result split` — 機能追加
   - `fix(tsf): ...` — bug 修正
5. **大きな変更は段階的に PR 分割** — M3 と M4 は特にレビュー単位を細かく
6. **Explorer crash の再発を継続観察** — `%LOCALAPPDATA%\CrashDumps\explorer.exe.*.dmp` の有無を運用中にたまに確認。再発時は直ちに M5 を開封（M0 暫定 PASS は**formal PASS ではない**ことに留意）
7. **0.7.x 各リリースは CHANGELOG を必ず更新** — user-facing 変更は特に丁寧に（crash 修正・尻切れ修正・読込中 UI は日本語で症状込みで記述）

---

## 11. 想定タイムライン（0.7.x リリース単位）

### 実績

```text
2026-04-24: ✅ v0.7.0 リリース（bug fix 集中型）
            ✅ M1.5 T-BUG2 (preview 長防壁)
            ✅ M1.7 T-MODE1 (DM 破棄前退避)
            ✅ M1.7 T-MODE2 (set_mode から即時 store 更新) ← 前倒し
            ✅ M1.7 T-MODE3 (GA_ROOT 正規化) ← 前倒し
            ✅ M1.8 T-MID1 (Phase1B gen タグ + Phase1A EditSession race fix)
            ✅ 候補ウィンドウ幅の動的計算（予定外の即応修正）
            ─ M1.6 (host 再起動化) は工数事情で v0.7.1 へ繰り延べ

2026-04-24: ✅ v0.7.1 リリース（host crash 根絶 + 基盤整理）
            ✅ M1.6 T-HOST1 (Request::Shutdown + engine_reload 再起動経路化)
            ✅ M1.6 T-HOST2 (reload 時間計測ログ)
            ✅ M1.6 T-HOST3 (読込中のキャレット近傍記号表示、MVP)
            ✅ M1.6 T-HOST4 (PENDING_KEYS バッファで握り潰し撤去)
            ✅ M1 T3-A (engine_get_or_create 削除)
            ✅ M1 T3-B (dispose_dm_resources ヘルパ集約)
            ✅ M1 T1-D (EXPLORER_CRASH_HISTORY.md + INVESTIGATION_GUIDE.md 新設)
            ─ M1.5 T-BUG1 / M1.8 T-MID2/3 は v0.7.2 へ繰り延べ
```

### 残タスク

```text
Week 2〜3: M1.5 T-BUG1 (早期 EOS 抑制)
           M1.8 T-MID2/3 (EditSession stale check + SetText 排他)
           │
           ├─────────── 🚀 v0.7.2 patch
           ▼
Week 4:    M3 (factory.rs 分割)
           │
           ├─────────── 🚀 v0.7.3 patch
           ▼
Week 5-6:  M2 (ライブ変換可読性)
           │
           ├─────────── 🚀 v0.7.4 minor
           ▼
Week 7-8:  M4 (LiveConvSession 集約)
           │
           └─────────── 🚀 v0.7.5 minor

(開封条件付き): M5 (実機再発した場合のみ) → v0.7.x patch
```

各リリースの区切り方の根拠:

- **v0.7.0 を先頭に**: 尻切れ / ブラウザモード / 中間消失の 3 種 bug fix を最速でユーザへ（達成）
- **v0.7.1 で host crash 根絶 + 基盤整理**: M1.6 全体 + M1 基盤整理を同梱（達成）
- **v0.7.2 で残り user-facing bug**: M1.5 T-BUG1（engine 品質）+ M1.8 T-MID2/3（race 堅牢化）。patch
- **v0.7.3 = 純リファクタ**: M3 factory.rs 分割、動作変わらない PR として明示（blame/diff レビューを簡潔に）
- **v0.7.4 で機能追加**: M2 の bg_peek/take 分離が見えるため minor bump
- **v0.7.5 で中枢集約**: M4 の LiveConvSession でライブ変換 API 形状の変化を包む

並行作業を許容すれば **v0.7.0/v0.7.1 は達成（2026-04-24）、v0.7.5 まで +5〜6 週** で到達見込み。個人開発の余裕状況で各リリース間隔は伸縮する。**crash が再発しない限り M5 は開封しない** ため、残総工数は 8〜14 日（v0.7.0 + v0.7.1 で ~4 日消化済）で上限が決まる。

---

## 12. 進捗トラッキング

各マイルストーン完了時に handoff.md の「残タスク（優先度順）」を更新する。  
本 ROADMAP.md も適宜更新し、完了項目には ✅ を付ける。
