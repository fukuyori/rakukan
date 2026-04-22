# 作業ロードマップ (post v0.6.6)

最終更新: 2026-04-22  
位置づけ: v0.6.6 で Explorer crash の真因対策が完了した時点での、コード整理・追加対策の作業計画書。  
関連資料:

- [handoff.md](handoff.md) — 現在の状態と既知の問題
- [LIVE_CONV_REDESIGN_REVISED.md](LIVE_CONV_REDESIGN_REVISED.md) — ライブ変換再設計案（§18 で採否仕分け済み）
- [GPU_MEMORY_LIFECYCLE.md](GPU_MEMORY_LIFECYCLE.md) — engine-host 多重起動時の GPU 実態

---

## 1. 全体方針

1. **安定性が最優先**: v0.6.6 の Explorer crash 対策の効果を実機で確認するまで、TSF 周辺の挙動変更は最小化する
2. **純リファクタリングと機能変更を分離**: 同一コミットに混ぜない。git blame と diff レビューが壊れる
3. **低リスク・小さな変更から**: ウォームアップで開発フローと CI を確認、その後に大きな整理へ進む
4. **保留している採用候補は実機 PASS 後に取り込む**: LIVE_CONV_REDESIGN_REVISED.md §18.3 の「採用検討」リストを段階的に消化する

---

## 2. マイルストーン全体像

```text
M0: v0.6.6 実機 PASS 確認         （1〜7 日）  ← ゲート
   │
   ├─ M1: 基盤整理（並行可能）    （3〜5 日）
   │   ├─ T3-A: engine accessor 整理
   │   ├─ T3-B: dispose 集約
   │   └─ ドキュメント / コメント cleanup
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

M5 (条件付き): 追加対策          （実機再発時のみ）
   ├─ WM_TIMER → PostMessage 化
   └─ Explorer シェル分岐
```

---

## 3. M0 — v0.6.6 安定性確認（ゲート）

### 目的

v0.6.6 の `DllCanUnloadNow` → `S_FALSE` 固定が真の root cause 対策だったかを実機で検証する。後続作業はすべてここを通過した後に着手する。

### 作業内容

1. WerFault フルダンプ設定（[handoff.md §既知の問題 §1](handoff.md) のコマンド参照）
2. `cargo make build-engine && cargo make build-tsf && cargo make sign && cargo make install`
3. サインアウト → 再ログオン
4. Explorer 主体で 30 分以上連続使用（リネーム / アドレスバー / フォルダ移動 / Alt+Tab）
5. `%LOCALAPPDATA%\CrashDumps\explorer.exe.*.dmp` の発生有無を確認

### 完了条件

- 30 分連続で crash 0 件 → **暫定 PASS**（M1 着手可）
- 1 日 (8 時間) 連続で crash 0 件 → **正式 PASS**

### 失敗時の対応

1. 新しい dump を WinDbg で `!analyze -v` 解析
2. `Failure.Bucket` が前回と異なる → 別経路の root cause、再仕分け
3. 同じ `rakukan_tsf.dll!Unloaded` → v0.6.6 の修正不足、追加調査

### M0 と並行可能な作業

- M1-D（ドキュメント / コメント cleanup） — TSF コード本体に触れないので並行 OK

---

## 4. M1 — 基盤整理（低リスク先行）

### 4.1 T3-A: engine accessor の重複削減

**現状**: 4 種類のアクセサが共存

```rust
pub fn engine_try_get()           // hot path 用
pub fn engine_get()                // blocking 用
pub fn engine_get_or_create()      // 既に #[allow(dead_code)]
pub fn engine_try_get_or_create()  // hot path + lazy spawn
```

**作業**:

- `engine_get_or_create()` を完全削除
- 関連 import の整理

**完了条件**: `cargo check` PASS、`cargo test` PASS

**リスク**: 極小（dead code 削除）

**想定工数**: 30 分

### 4.2 T3-B: dispose 系関数の集約

**現状**: `OnUninitDocumentMgr` から 3 つの cleanup を手で呼んでいる

```rust
fn OnUninitDocumentMgr(&self, pdim: Option<&ITfDocumentMgr>) -> Result<()> {
    let ptr = dm.as_raw() as usize;
    doc_mode_remove(ptr);
    invalidate_live_context_for_dm(ptr);
    invalidate_composition_for_dm(ptr);  // 1 つでも忘れたら不整合
    Ok(())
}
```

**作業**:

- `dispose_dm_resources(dm_ptr: usize)` ヘルパを `engine/state.rs` に追加
- 内部で 3 関数を順に呼ぶ
- `OnUninitDocumentMgr` から 1 行呼び出しに変更

**完了条件**: `cargo check` PASS、目視で漏れがないこと確認

**リスク**: 小（純粋な集約）

**想定工数**: 1 時間

### 4.3 T1-D: ドキュメント / コメント cleanup（M0 と並行可）

**目的**: 旧仮説（Phase1A の stale ITfContext race が主因）に基づくコメント・ドキュメント表現を v0.6.6 の真因判明後の現実に合わせる

**作業**:

1. `rg "Phase1A.*race|stale ITfContext|Explorer crash 主因"` で grep
2. ヒット箇所のコメントを「DM 個別破棄に対する保険（v0.6.6 で DLL unload race は別途解消）」のような表現に修正
3. `docs/INVESTIGATION_GUIDE.md`（新規）の作成 — クラッシュ調査プロトコル明文化（dump → WinDbg → root cause → fix の順）
4. `docs/EXPLORER_CRASH_HISTORY.md`（新規）の作成 — 0.4.4 から 0.6.6 までの crash 対策の年表 + 学んだこと

**完了条件**: 旧仮説に基づく表現が残っていないこと、新規 2 ドキュメントが作成済みであること

**リスク**: なし（コード本体には触れない）

**想定工数**: 1〜2 時間

### M1 完了条件

- T3-A, T3-B, T1-D 全完了
- `cargo make build-engine && cargo make build-tsf` PASS
- 機能テスト: ローマ字入力、変換、確定、F6-F10 が引き続き動作

---

## 5. M2 — ライブ変換ロジックの可読性向上

### 5.1 T1-B: `on_live_timer` (248 行) の分解

**現状**: [candidate_window.rs:987-1219](../crates/rakukan-tsf/src/tsf/candidate_window.rs#L987) の `on_live_timer` が 6 段階の処理を 1 関数で抱えている

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

**現状**: `bg_take_candidates(key)` が「結果取り出し」「converter 返却」「ユーザー辞書マージ」を兼ねており、live preview と通常変換が干渉する

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

**作業**:

- `EngineConfig` 相当に `session_nonce` カウンタを追加（composition 開始ごとに increment）
- `BgResult` に `(session_nonce, gen)` を付与
- preview 取得側で session_nonce / gen 不一致なら破棄

**完了条件**: stale result が apply されないこと（debug ログで確認）

**リスク**: 中

**想定工数**: 1 日

### M2 完了条件

- T1-B, §6.2 採用, session_nonce 採用 完了
- 機能テスト: ライブ変換、Space 変換、Enter fallback すべて動作
- 1 時間程度の使用テスト で回帰なし

---

## 6. M3 — factory.rs 分割（T1-A）

### 目的

4583 行の god file を機能別ファイルに分割し、可読性と保守性を向上させる。  
**機能変更は一切行わない**。純粋なファイル切り出しのみ。

### 推奨分割

```text
crates/rakukan-tsf/src/tsf/factory.rs              核 + COM impl     (~500 行)
                  ├── factory/activate.rs          Activate/Deactivate (~250 行)
                  ├── factory/dispatch.rs          handle_action       (~300 行)
                  ├── factory/on_input.rs          Input + LiveConv    (~400 行)
                  ├── factory/on_convert.rs        Convert + Selecting (~600 行)
                  ├── factory/on_compose.rs        update_composition  (~500 行)
                  ├── factory/on_kana_latin.rs     F6-F10              (~300 行)
                  └── factory/on_segment.rs        CursorL/R + Range   (~400 行)
```

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

### 集約対象

| 現状の場所 | 移動先 |
|---|---|
| `TL_LIVE_CTX` / `TL_LIVE_TID` / `TL_LIVE_DM_PTR` | `LiveConvSession.{ctx, tid, dm}` |
| `LIVE_PREVIEW_QUEUE` / `LIVE_PREVIEW_READY` | 削除（pull モデルに変更） |
| `SUPPRESS_LIVE_COMMIT_ONCE` | `LiveConvSession.suppress_next_commit` |
| `LIVE_TIMER_FIRED_ONCE_STATIC` / `LIVE_LAST_INPUT_MS` / `LIVE_DEBOUNCE_CFG_MS` | `LiveConvSession.{fired_once, last_input_ms}` + 設定値はそのまま |
| `SessionState::LiveConv { reading, preview }` | `LiveConvSession.last_preview` + `SessionState::LiveConv` の payload は最小化（reading のみ残す） |

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

## 9. リファクタリング不要と判断したもの

| ファイル | 行数 | 理由 |
|---|---|---|
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
6. **v0.6.6 安定確認まで TSF 側のロジック改修は止める** — M0 PASS が大前提

---

## 11. 想定タイムライン

```text
Week 1: M0 (v0.6.6 確認) + M1 (基盤整理) 並行
Week 2: M2 (ライブ変換可読性)
Week 3: M3 (factory.rs 分割)
Week 4-5: M4 (LiveConvSession 集約)
Week 6+: M5 (再発時のみ)
```

並行作業を許容すれば 4〜5 週で M1〜M4 完了見込み。  
個人開発の余裕状況によって優先度を調整可能。

---

## 12. 進捗トラッキング

各マイルストーン完了時に handoff.md の「残タスク（優先度順）」を更新する。  
本 ROADMAP.md も適宜更新し、完了項目には ✅ を付ける。
