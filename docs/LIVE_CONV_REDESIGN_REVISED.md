# ライブ変換再設計書（改訂版）

対象バージョン: 次期ライブ変換再設計  
作成日: 2026-04-22  
位置づけ: Explorer などで観測される TSF / MSCTF 系異常終了の再発防止を主目的とした、ライブ変換の安全側再設計  
関連資料: `docs/DESIGN.md`, `docs/CONVERTER_REDESIGN.md`, `docs/handoff.md`

---

## 1. 概要

現行のライブ変換は動作しているが、Explorer を中心に `ITfContext` / `ITfComposition` / `DocumentMgr` の寿命境界と競合したときに、stale な TSF オブジェクトへ触れる経路が残っている。

とくに現行実装では次の要素が組み合わさり、異常終了リスクを高めている。

1. ライブ変換状態が `TL_LIVE_CTX` / `TL_LIVE_DM_PTR` / `LIVE_PREVIEW_QUEUE` / `LIVE_PREVIEW_READY` / `SessionState::LiveConv` / `COMPOSITION(stale)` に分散している
2. `OnSetFocus` / `OnUninitDocumentMgr` / `OnKillThreadFocus` / `OnEndComposition` と、ライブ変換 preview 適用の責務が複数箇所に分断されている
3. `WM_TIMER` ベースの定常ポーリングが stale 状態の後追い発火を起こしうる
4. `bg_take_candidates()` が「結果取得」と「converter 所有権返却」を兼ねており、live preview と通常変換が結合している
5. 旧セッションの結果と新セッションの入力状態を分離する `session nonce` が存在しない

本書は、上記を構造的に解消しつつ、ライブ変換 UX を維持するための改訂版設計である。

なお、初稿で提案されていた「TSF callback 中に `LiveConvSession` を即 `drop` する」「`static Mutex` に COM オブジェクトを保持する」「`Weak<ITfComposition>` で composition 寿命を判定する」方針は、本改訂版では採用しない。これらは現行の hardening 方針と衝突し、かえって Explorer crash を再導入するおそれがあるためである。

---

## 2. 設計原則

| # | 原則 | 内容 | 効果 |
|---|---|---|---|
| P1 | 単一所有 | ライブ変換のランタイム状態は `LiveConvSession` に集約する | 状態分散の縮小 |
| P2 | thread-local 所有 | `ITfContext` / `ITfDocumentMgr` / `ITfComposition` を含むライブ状態は `thread_local!` に保持し、`Send/Sync` 前提のグローバル `Mutex` に置かない | STA / COM 所有モデルを維持 |
| P3 | 遅延破棄 | TSF callback 中は stale mark のみ行い、COM object の `Release` を伴う実破棄は安全な後段に遅延する | `msctf!_NotifyCallbacks` 直下の再入 / 解放を回避 |
| P4 | イベント駆動 | `WM_TIMER` による 50ms 定常ポーリングを廃止し、ワーカー完了時の `PostMessage` で apply をスケジュールする | stale timer 発火を排除 |
| P5 | 二重世代管理 | `session_nonce` と `generation` を併用し、旧セッション結果も旧入力結果も適用しない | stale 結果混入の排除 |
| P6 | 副作用分離 | preview 参照は副作用なし API、確定 / 通常変換でのみ consume API を使う | live preview と通常変換の競合低減 |
| P7 | 多層防御 | message 受信時と EditSession 実行直前の両方で整合性を再検証する | TOCTOU の抑制 |
| P8 | 失敗は静かに | 整合性違反は warn / debug ログと `return` のみ。ユーザー入力は継続可能に保つ | データ損失なき縮退 |
| P9 | kill-switch を残す | Explorer など特定条件で deferred apply を無効化できる設定 / ガードを残す | 実機再発時の被害局所化 |

---

## 3. 初稿からの主な変更点

### 3.1 採用しない方針

以下は初稿から除外する。

- `static LIVE: OnceLock<Mutex<Option<LiveConvSession>>>`
- `LiveConvSession::Drop` に全クリーンアップ責務を集中
- `OnSetFocus` / `OnUninitDocumentMgr` / `OnEndComposition` / `Deactivate` での即 `LIVE.take()`
- `Weak<ITfComposition>` を composition 失効検出の主手段とする設計
- `RESULT_QUEUE.clear()` による stale 結果の一括消去

### 3.2 改訂版で採る方針

- `LiveConvSession` は `thread_local! { RefCell<Option<...>> }` で保持する
- callback 中は `stale = true` と `dispose_requested = true` を立てるだけにする
- 実破棄は `WM_APP_FOCUS_CHANGED` 後段、または安全な live apply / key handling の入口で行う
- worker 結果には `session_nonce` と `gen` を付与し、古い結果は queue から取り出しても apply しない
- `SessionState::LiveConv` は型としては残す。ただし payload を最小化し、ランタイム所有の正本は `LiveConvSession` に寄せる

---

## 4. 全体アーキテクチャ

### 4.1 データフロー

```text
[ユーザー打鍵]
      │
      ▼
[on_input]
      │
      ├─ preedit 更新
      ├─ LiveConvSession.ensure()
      ├─ gen += 1
      └─ bg_start_with_ticket(session_nonce, gen)
                        │
                        ▼
                 [変換ワーカー]
                        │
                        ▼
      ResultQueue.push(BgResult { session_nonce, gen, ... })
      PostMessage(hwnd, WM_RAKUKAN_LIVE_READY, ...)
                        │
                        ▼
                 [candidate WndProc]
                        │
                        ├─ session 存在確認
                        ├─ stale / dispose_requested 確認
                        ├─ session_nonce / gen 照合
                        └─ apply_requested = true
                                │
                                ▼
                    [deferred live apply]
                                │
                                ├─ 再度 session_nonce / gen 照合
                                ├─ composition stale 確認
                                ├─ current focus DM 照合
                                └─ RequestEditSession
                                        │
                                        ▼
                                [apply_live_preview]
```

### 4.2 役割分担

| レイヤ | 責務 | 触らないこと |
|---|---|---|
| `engine/*` | 非同期推論、`session_nonce + gen` 付き結果の保持 / 参照 | TSF API |
| `tsf/live_session.rs` | `LiveConvSession` の生成、stale mark、遅延 dispose | 推論実行 |
| `tsf/live_apply.rs` | apply 前整合性確認、`RequestEditSession` と `DoEditSession` | セッション生成 / 破棄 |
| `tsf/candidate_window.rs` | `WM_RAKUKAN_LIVE_READY` 受信、ready フラグ化、Explorer 向け kill-switch | preedit ロジック本体 |
| `tsf/factory.rs` | `on_input` / `on_convert` / `on_cancel` / `on_commit_raw` と session state 連携 | タイマー管理 |

### 4.3 重要な前提

`WndProc` は「セッションを破棄する場」ではなく、「ready を受けて安全な apply を予約する場」とする。  
破棄と apply の本体は、必ず再検証を伴う別関数で行う。

---

## 5. コアデータ構造

### 5.1 `LiveConvSession`

```rust
pub struct LiveConvSession {
    // 対象コンテキストと DM。thread-local 所有のみ許可する。
    ctx: ITfContext,
    dm: ITfDocumentMgr,
    tid: u32,
    hwnd: HWND,

    // セッション単位の一意識別子。新しい composition 開始ごとに更新。
    session_nonce: u64,

    // 1 セッション内の入力世代。on_input ごとに増加。
    gen: u64,

    // composition の生存確認は Weak ではなく DM / stale flag / clone 可否で行う。
    composition_dm_ptr: usize,

    // callback 中は true にするだけで drop しない。
    stale: bool,
    dispose_requested: bool,

    // worker 通知を受けた結果、apply が必要か。
    apply_requested: bool,

    // 最後に表示した preview の世代
    applied_gen: u64,

    created_at: Instant,
}
```

### 5.2 thread-local 状態

```rust
thread_local! {
    static TL_LIVE_SESSION: RefCell<Option<LiveConvSession>> = RefCell::new(None);
}
```

理由:

- `ITfContext` / `ITfDocumentMgr` / `ITfComposition` は STA 前提で扱う
- 現行 `candidate_window.rs` も HWND や live context を thread-local で管理している
- `static Mutex` 化すると COM object を `Send/Sync` 前提に載せる方向へ寄り、現行 hardening 方針と逆行する

### 5.3 グローバル

```rust
pub static RESULT_QUEUE: OnceLock<Mutex<VecDeque<BgResult>>> = OnceLock::new();
pub static WM_RAKUKAN_LIVE_READY: OnceLock<u32> = OnceLock::new();
```

### 5.4 `BgResult`

```rust
pub struct BgResult {
    pub session_nonce: u64,
    pub gen: u64,
    pub reading: String,
    pub candidates: Vec<String>,
}
```

### 5.5 `SessionState::LiveConv`

`SessionState::LiveConv` は完全廃止しない。  
ただし、正本の所有は `LiveConvSession` に寄せ、`SessionState` 側は UI 分岐に必要な最小情報のみ持つ。

```rust
LiveConv {
    reading: String,
    preview: String,
}
```

残す理由:

- `on_input` の suffix 合成
- `on_convert` の `LiveConv -> Preedit` 遷移
- `on_commit_raw` fallback commit
- Esc / Backspace / selecting 遷移時の明確な状態分岐

---

## 6. エンジン API 改訂方針

### 6.1 現状の問題

現行 `bg_take_candidates(key)` は以下を同時に行う。

- Done 結果の取り出し
- converter 所有権の engine への返却
- ユーザー辞書候補とのマージ

このため live preview が結果を読むだけで engine 側の状態が進み、通常変換や commit fallback と干渉する。

### 6.2 新 API

```rust
pub fn bg_start_with_ticket(&mut self, n_cands: usize, session_nonce: u64, gen: u64) -> bool;

// preview 用: 状態を進めずに結果を読む
pub fn bg_peek_result(&self) -> Option<&BgResult>;

// commit / convert 用: 結果を consume し、converter を engine に戻す
pub fn bg_take_result(&mut self) -> Option<BgResult>;

// 既存 merge ロジックは別関数として維持
pub fn merge_candidates_for_preview(&self, reading: &str, llm: &[String], limit: usize) -> Vec<String>;
pub fn merge_candidates_for_commit(&self, reading: &str, llm: Vec<String>, limit: usize) -> Vec<String>;
```

### 6.3 整合性ルール

- live preview は `bg_peek_result()` のみを使う
- Space 変換、Enter fallback、commit 時のみ `bg_take_result()` を使う
- `RESULT_QUEUE` は notify 専用であり、候補の正本は engine / conv_cache に置く
- stale result は `session_nonce` と `gen` の不一致で黙って捨てる

### 6.4 queue の役割

`RESULT_QUEUE` は「結果の保存場所」ではなく、「どの ticket が ready になったかを知らせる通知帯域」と位置づける。

これにより以下を避ける。

- queue clear による新セッション結果の巻き添え破棄
- engine 側正本と queue 側コピーの二重管理
- preview と commit の参照元の不一致

---

## 7. ライフサイクル

### 7.1 セッション生成

```text
初回 composition 開始
  └─ ensure_live_session(ctx, dm, tid, hwnd, dm_ptr)
         ├─ 既存 session が None         → 新規生成
         ├─ 既存 session が stale        → dispose 後に新規生成
         └─ 既存 session が同一 DM / 生存 → 再利用
```

### 7.2 破棄トリガー

以下のイベントでは「即 drop」せず、`stale = true` と `dispose_requested = true` を立てる。

| イベント | 発火元 | 即時に行うこと | 後段で行うこと |
|---|---|---|---|
| Focus 変更 | `OnSetFocus` | queue に積む | 安全文脈で dispose |
| DM 破棄 | `OnUninitDocumentMgr` | stale mark | 安全文脈で dispose |
| composition 終了 | `OnEndComposition` | stale mark | 安全文脈で dispose |
| IME 非アクティブ | `Deactivate` / `OnKillThreadFocus` | stale mark | 安全文脈で dispose |
| 確定 | `on_commit_raw` | dispose_requested | 終了後 dispose |
| キャンセル | `on_cancel` | dispose_requested | 終了後 dispose |

### 7.3 dispose 実行点

dispose は以下のいずれかで実行する。

- `process_focus_change()` の後段
- `handle_live_ready()` の入口
- `on_input` / `on_convert` / `on_cancel` / `on_commit_raw` の入口
- `try_apply_live_preview()` の入口

### 7.4 dispose の内容

```rust
fn dispose_live_session(reason: &str) {
    TL_LIVE_SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(sess) = slot.as_ref() {
            tracing::info!(
                "[Live] drop: {} (session_nonce={}, gen={}, age={}ms)",
                reason,
                sess.session_nonce,
                sess.gen,
                sess.created_at.elapsed().as_millis(),
            );
        }
        *slot = None; // ここで COM object を release
    });
}
```

`Drop` にクリーンアップを詰め込まず、明示的な dispose 関数で実行理由をログに残す。

---

## 8. メッセージング

### 8.1 メッセージ定義

```rust
unsafe {
    let id = RegisterWindowMessageW(w!("rakukan_live_ready"));
    WM_RAKUKAN_LIVE_READY.set(id).ok();
}
```

### 8.2 `WPARAM` / `LPARAM`

初稿の「下位 32bit gen のみ照合」は採用しない。  
代わりに、`RESULT_QUEUE` に `BgResult { session_nonce, gen, ... }` を push し、`PostMessage` 自体は wakeup 通知としてのみ使う。

```rust
WPARAM: unused
LPARAM: unused
```

理由:

- platform 依存の `WPARAM` 幅に設計を縛られない
- `session_nonce + gen` の完全比較を queue 側で行える
- rollover を考慮した下位 32bit 比較より簡潔で誤りにくい

---

## 9. 主要フロー

### 9.1 `on_input`

```rust
fn on_input(...) -> Result<()> {
    dispose_live_session_if_requested("on_input");

    let sess = ensure_live_session(...)?;
    sess.gen += 1;
    let ticket = (sess.session_nonce, sess.gen);

    let preedit = append_hiragana(ch)?;
    update_composition(ctx, tid, sink, &preedit)?;

    if !preedit.is_empty() && engine_ready() {
        with_engine(|e| {
            e.bg_start_with_ticket(live_beam, ticket.session_nonce, ticket.gen);
        });
    }
    Ok(())
}
```

### 9.2 worker 完了

```rust
fn worker_done(result: BgResult, hwnd: HWND) {
    RESULT_QUEUE.lock().push_back(result);
    let _ = unsafe { PostMessageW(hwnd, live_ready_msg, WPARAM(0), LPARAM(0)) };
}
```

失敗時の扱い:

- `PostMessage` 失敗は「通知先消失」とみなし、結果は queue に残す
- 次の安全な入口で queue を掃除する
- queue clear はしない。stale 判定で個別破棄する

### 9.3 `handle_live_ready`

```rust
fn handle_live_ready() {
    dispose_live_session_if_requested("live_ready");

    let Some(sess) = live_session_clone_minimal() else { return };
    if sess.stale {
        return;
    }

    let Some(result) = pop_latest_matching_result(sess.session_nonce, sess.gen) else {
        return;
    };

    mark_apply_requested(sess.session_nonce, sess.gen, result.preview);
    try_apply_live_preview();
}
```

重要:

- `handle_live_ready` 自体は session の破棄責務を持たない
- queue は先頭から順に見て、古い `session_nonce` や `gen` は debug ログを出して捨てる
- lock 競合時は「pending を残したまま return」し、次のメッセージまたは入力で再試行する

### 9.4 `try_apply_live_preview`

```rust
fn try_apply_live_preview() {
    dispose_live_session_if_requested("try_apply");

    let Some(snapshot) = take_pending_apply_snapshot() else { return };
    if !snapshot.is_valid_now() {
        return;
    }

    if explorer_kill_switch_enabled_for(snapshot.hwnd) {
        tracing::info!("[Live] apply skipped by kill-switch");
        return;
    }

    let es = EditSession::new(move |ec| unsafe {
        apply_live_preview(ec, snapshot)
    });
    let _ = unsafe { snapshot.ctx.RequestEditSession(snapshot.tid, &es, TF_ES_READWRITE) };
}
```

### 9.5 `apply_live_preview`

```rust
unsafe fn apply_live_preview(ec: TfEditCookie, snapshot: PendingApply) -> Result<()> {
    // 再検証
    let Some(sess) = live_session_ref() else { return Ok(()); };
    if sess.stale || sess.dispose_requested {
        return Ok(());
    }
    if sess.session_nonce != snapshot.session_nonce || sess.gen != snapshot.gen {
        return Ok(());
    }
    if current_focus_dm_ptr() != Some(sess.composition_dm_ptr) {
        return Ok(());
    }

    let Some(comp) = composition_clone()? else {
        return Ok(());
    };

    let range = comp.GetRange()?;

    // pending romaji を消さない
    let display_text = build_live_display_text(snapshot.preview.as_str())?;

    let text_w: Vec<u16> = display_text.encode_utf16().collect();
    range.SetText(ec, 0, &text_w)?;

    set_display_attribute(...)?;
    set_selection_to_end(...)?;

    sess.applied_gen = snapshot.gen;
    Ok(())
}
```

注意:

- preview 単体ではなく `preview + pending_romaji` を適用する
- composition の生死確認は `composition_clone()` と focus DM 照合で行う
- `Weak<ITfComposition>` は使わない

---

## 10. 現行 hardening との接続

### 10.1 `COMPOSITION(stale)` は残す

現行の `CompositionWrapper { comp, dm_ptr, stale }` は、Explorer crash hardening として有効に機能している。  
改訂版でもこの仕組みは残し、`LiveConvSession` はこれを利用する。

方針:

- `COMPOSITION` の stale mark は継続
- `LiveConvSession` は `composition_dm_ptr` を持つが、composition 本体の所有正本にはならない
- composition 失効検出は `COMPOSITION.stale` と `current_focus_dm_ptr()` の二段で行う

### 10.2 `OnSetFocus` 遅延処理は維持

`OnSetFocus` は引き続き `WM_APP_FOCUS_CHANGED` に積み、callback 直下で COM 再入しない。  
改訂版はこの路線を前提とし、そこへ live session dispose を統合する。

### 10.3 `SessionState::LiveConv` の縮約

`SessionState::LiveConv` を丸ごと消すのではなく、

- UI 分岐
- suffix 合成
- Enter fallback
- Esc / Space 遷移

に必要な最小状態だけ残す。

---

## 11. Explorer 向け kill-switch

### 11.1 目的

deferred live apply の本設計でも Explorer で再発が残る可能性を考慮し、設定またはコード上で対象クラス / 対象ホストに対して live auto-apply を止められるようにする。

### 11.2 方針

```toml
[live_conversion]
enabled = true
deferred_apply_enabled = true
disable_auto_apply_for_explorer = true
```

### 11.3 動作

- Explorer / shell 系クラスでは `WM_RAKUKAN_LIVE_READY` を受けても composition apply を行わず、preview 結果を session にだけ保持する
- 次のキー入力、Space、Enter、Esc で通常フローに合流する
- これにより UX は一部劣化するが、被害は Explorer に局所化できる

### 11.4 対象候補

- `CabinetWClass`
- `ExploreWClass`
- `Progman`
- `WorkerW`
- `Shell_TrayWnd`

---

## 12. エラーハンドリング

原則: 整合性違反は静かに return し、session を即破棄しない。  
ただし composition / DM 失効が確定している場合は `dispose_requested = true` を立てる。

| 失敗箇所 | 対応 | 備考 |
|---|---|---|
| session 不在 | return | 次回入力で再生成 |
| `session_nonce` 不一致 | stale result を破棄 | 古い worker 結果 |
| `gen` 不一致 | stale result を破棄 | 古い入力結果 |
| `composition_clone()` = None | `dispose_requested = true` | composition 終了後 |
| focus DM 不一致 | return | 遷移途中 |
| engine busy | pending を残して return | 次回再試行 |
| `RequestEditSession` Err | warn ログ + pending 残し | kill-switch 判定材料 |
| `SetText` Err | warn ログ | session は維持 |

---

## 13. 移行計画

### Phase R1: 基盤追加

- `docs/LIVE_CONV_REDESIGN_REVISED.md` を基準文書とする
- `WM_RAKUKAN_LIVE_READY` を追加
- `LiveConvSession` の thread-local スケルトンを追加
- `session_nonce` を生成するカウンタを追加

### Phase R2: stale / dispose モデル導入

- `OnSetFocus` / `OnUninitDocumentMgr` / `OnKillThreadFocus` / `OnEndComposition` で即 drop せず stale mark のみに変更
- `dispose_live_session_if_requested()` を安全な入口に追加

### Phase R3: engine API 分離

- `bg_start_with_ticket`
- `bg_peek_result`
- `bg_take_result`
- preview 用 merge と commit 用 merge の分離

### Phase R4: `WM_TIMER` 廃止

- `LIVE_TIMER_ID` / `LIVE_POLL_MS` / `TL_LIVE_CTX` / `LIVE_PREVIEW_QUEUE` / `LIVE_PREVIEW_READY` を段階削除
- worker 完了時 `PostMessage(WM_RAKUKAN_LIVE_READY)` に切替

### Phase R5: apply 経路置換

- `handle_live_ready` / `try_apply_live_preview` / `apply_live_preview` を導入
- pending romaji 合成を維持したまま Phase 1A / 1B の二重経路を一本化

### Phase R6: kill-switch と検証

- Explorer 向け `disable_auto_apply_for_explorer`
- 実機で 30 分以上の rename / address bar / Alt+Tab 試験
- `docs/DESIGN.md` と `docs/handoff.md` を更新

---

## 14. テスト計画

### 14.1 機能テスト

| # | シナリオ | 期待動作 |
|---|---|---|
| F1 | 「にほんご」入力後停止 | preview が自動更新 |
| F2 | preview 表示中にさらに打鍵 | suffix + pending romaji を維持 |
| F3 | preview 表示中に Space | `LiveConv -> Preedit` 経由で通常変換へ |
| F4 | preview 表示中に Enter | fallback commit が動作 |
| F5 | preview 表示中に Esc | preedit クリア、session dispose_requested |
| F6 | Backspace | preedit 短縮、古い結果は捨てられる |

### 14.2 ストレステスト

| # | シナリオ | 期待動作 |
|---|---|---|
| S1 | Explorer リネーム連発 | crash なし |
| S2 | メモ帳 / Chrome / Explorer を往復 | stale result が適用されない |
| S3 | 打鍵と Esc の交互操作 | session が残留しない |
| S4 | モデル未ロード時連打 | crash なし、ロード後回復 |
| S5 | 同一入力中に focus 変化 | `dispose_requested` が立ち、apply は中止 |

### 14.3 ログ確認

| # | 条件 | 期待ログ |
|---|---|---|
| E1 | stale session result | `[Live] stale result: session_nonce mismatch` |
| E2 | stale gen result | `[Live] stale result: gen mismatch` |
| E3 | deferred dispose | `[Live] drop: OnSetFocus ...` |
| E4 | kill-switch 発動 | `[Live] apply skipped by kill-switch` |
| E5 | focus 不一致 | `[Live] apply skipped: focus dm changed` |

---

## 15. オープンクエスチョン

| # | 内容 | 改訂版の暫定方針 |
|---|---|---|
| Q1 | `RequestEditSession` を deferred message から呼ぶ前提を最終採用するか | 実機検証で確認。再発時は Explorer kill-switch を優先 |
| Q2 | `bg_peek_result` を conv_cache 内に置くか engine 側にラップするか | engine 側 API で隠蔽する |
| Q3 | `SessionState::LiveConv` の payload をどこまで縮めるか | まず `reading + preview` を維持 |
| Q4 | `RESULT_QUEUE` を notify 帯域に限定できるか | できる。候補の正本は engine に残す |
| Q5 | dispose 実行点の最小集合 | `focus_changed`, `live_ready`, `on_input`, `on_convert`, `on_cancel`, `on_commit_raw` |
| Q6 | Explorer 判定をクラス名で行うか process 名で行うか | まずクラス名。必要なら process 名追加 |

---

## 16. 承認チェックリスト

- [ ] `thread_local LiveConvSession` 方針で進める
- [ ] callback 中に COM object を即 drop しないことを合意
- [ ] `session_nonce + gen` の二重照合を採用
- [ ] `SessionState::LiveConv` を完全廃止しないことを合意
- [ ] `RESULT_QUEUE` は notify 帯域に限定する
- [ ] Explorer 向け kill-switch を最初から入れるか判断
- [ ] `docs/DESIGN.md` の「WndProc から RequestEditSession 禁止」との整合を実機で再確認

---

## 17. 要約

改訂版のポイントは次の 4 点である。

1. ライブ変換状態は `LiveConvSession` に集約するが、保持場所は `static Mutex` ではなく `thread_local RefCell` にする
2. `OnSetFocus` / `OnUninitDocumentMgr` / `OnEndComposition` では即破棄せず、stale mark と遅延 dispose に切り替える
3. worker 結果には `session_nonce + gen` を付与し、queue clear ではなく個別 discard で stale 結果を捨てる
4. `SessionState::LiveConv` は完全削除せず、UI 分岐に必要な最小情報だけ維持する

これにより、現行 hardening を壊さずにライブ変換の責務を整理し、Explorer 系クラッシュの主要因と考えられる stale TSF object 操作を大幅に減らせる見込みである。

---

## 18. 検討結果（2026-04-22 追記）

> ⚠️ **重要**: 本書の前提（Explorer crash の主因が stale `ITfContext` / `ITfComposition`）は、**2026-04-22 のクラッシュダンプ解析で覆った**。本章はその結果を反映した採用判断のメモである。

### 18.1 前提の変化

クラッシュダンプ (`explorer.exe.3124.dmp`, 2026-04-22 07:23) の WinDbg 解析結果:

```text
Failure.Bucket = BAD_INSTRUCTION_PTR_c0000005_rakukan_tsf.dll!Unloaded
スタック: explorer!CTray::_MessageLoop → PeekMessageW
        → UserCallWinProcCheckWow → <Unloaded_rakukan_tsf.dll>+0x13e70
```

- **真因**: `DllCanUnloadNow=S_OK` 後の `FreeLibrary` と、`RegisterClassW` で残った `wnd_proc` ポインタへの in-flight メッセージディスパッチが衝突
- **v0.6.6 の対策**: `DllCanUnloadNow` を常に `S_FALSE` 固定（プロセス常駐化、Microsoft 標準 IME と同パターン、メモリコスト ~2 MB/process）
- **影響**: 本書の P3 (遅延破棄) / P9 (kill-switch) など「DLL unload 中の COM release を恐れて設計した」要素は、DLL が unload されないため緊急性を失った

### 18.2 各原則の再評価

| # | 原則 | v0.6.6 後の評価 |
| --- | --- | --- |
| P1 | 単一所有 | ◯ コード可読性向上、crash には無関係 |
| P2 | thread-local | ◯ 既存方針と同じ |
| P3 | 遅延破棄 | △ DLL 常駐で COM release 自体のリスク低下 |
| P4 | イベント駆動 (WM_TIMER 廃止) | ◯ オーバーヘッド削減には有効 |
| P5 | session_nonce + gen | ◯ 正確性向上（stale 結果適用防止） |
| P6 | preview/consume API 分離 | ◎ 副作用低減で価値高 |
| P7 | 多層防御 | △ Phase 1〜2 で実装済み |
| P8 | 失敗は静かに | ◯ Phase 3 で実装済み |
| P9 | kill-switch | ✕ DLL unload race 解消で不要 |

### 18.3 採否の仕分け

#### ✅ 採用検討（独立して有用、優先度高）

| 項目 | 出典 | 理由 |
| --- | --- | --- |
| `session_nonce + gen` での stale 結果 discard | Phase R3 / P5 | crash 関係なく正確性向上 |
| `bg_peek_result` / `bg_take_result` の API 分離 | §6.2 | preview と commit の状態干渉解消 |
| `merge_candidates_for_preview` / `_for_commit` の分離 | §6.2 | 副作用境界の明確化 |

**工数**: 中（engine 側の API 追加 + 呼出元の置換）。**リスク**: 低。**効果**: 通常変換と live preview の競合バグ予防

#### 🤔 保留（v0.6.6 安定性確認後に再検討）

| 項目 | 出典 | 保留理由 |
| --- | --- | --- |
| WM_TIMER → PostMessage 化 | Phase R4 | crash と無関係。改善はオーバーヘッドのみ |
| `LiveConvSession` 構造体への状態集約 | Phase R1-R2 | コード整理として有益だが変更量大、現状動いている |
| `dispose_requested` / 遅延 dispose モデル | §7 | DLL 常駐で COM release リスク低下、現行 `invalidate_*` で十分の可能性 |

#### ❌ 見送り

| 項目 | 出典 | 見送り理由 |
| --- | --- | --- |
| Explorer kill-switch | §11 | v0.6.6 で DLL unload race 解消済み |
| Phase R1〜R6 の全面一括実施 | §13 | コスト過大、crash 主因が消えた今は cost/benefit 悪い |
| `SessionState::LiveConv` の縮約 | §5.5 | 縮約の必要性が弱い |

### 18.4 推奨アクション

1. **まず v0.6.6 の実機テスト**（30 分以上、可能なら 1 日連続使用）で Explorer crash 0 件を確認する
2. **crash 解消が確認できたら**: §18.3 の「採用検討」カテゴリを **小さな個別 PR として段階的に取り込む**
3. **crash が再発したら**: 新しいダンプを取得して別経路の root cause を特定し、本書を再評価する
4. 本書の §1〜§17 の設計案は採用条件付きの参考資料として保持する

### 18.5 結論

本書は内部整合性の高い良質な設計案だが、**主動機（Explorer crash 防止）が v0.6.6 で別経路により解決された**ため、全面採用は cost/benefit が見合わない。「副作用境界の明確化」「stale 結果の discard」など独立して価値のある部分のみ抜き出して段階導入するのが妥当。

