//! ライブ変換セッション状態を集約する thread_local 構造体 (M4 / T2)。
//!
//! # Phase 1 (v0.7.6) のスコープ
//! TSF スレッドローカルに閉じる状態 (Phase1A 用 ITfContext / DM ポインタ /
//! タイマー fired_once / last_input_ms) を集約。`LIVE_DEBOUNCE_CFG_MS` は
//! 設定値なので static のまま残す。
//!
//! ## Phase 2 (v0.7.7 以降) で吸収する予定
//! - `LIVE_PREVIEW_QUEUE` / `LIVE_PREVIEW_READY` (Phase 1B キュー)
//! - `SUPPRESS_LIVE_COMMIT_ONCE`
//! - `LIVE_CONV_GEN`
//! - M2 §5.3 `session_nonce` (composition 開始ごとの identity)

use std::cell::RefCell;

use windows::Win32::UI::TextServices::ITfContext;

#[derive(Default)]
pub(super) struct LiveConvSession {
    /// Phase1A 用 ITfContext (RequestEditSession 起点)。
    /// `live_input_notify` で `on_input` から保存され、`on_live_timer` から参照される。
    pub ctx: Option<ITfContext>,
    /// TSF client_id (RequestEditSession の引数)。
    pub tid: u32,
    /// `ctx` を取得した時点の DocumentMgr ポインタ。
    /// Explorer 等で DM が再生成されたら stale 判定に使う。
    pub composition_dm_ptr: usize,

    /// `on_live_timer` の bg=running 状態を 1 度だけログするためのフラグ。
    /// `swap_fired_once(true)` で「ログ済」に遷移、`reset_fired_once()` で戻す。
    pub fired_once: bool,
    /// 最後の `live_input_notify` 呼出時刻 (ms)。debounce 判定に使う。
    pub last_input_ms: u64,
}

thread_local! {
    pub(super) static TL_LIVE_SESSION: RefCell<LiveConvSession> =
        RefCell::new(LiveConvSession::default());
}

// ─── (ctx, tid, composition_dm_ptr) スナップショット ──────────────────────────

/// Phase1A 用の (ctx, tid, dm_ptr) を一括セット (`live_input_notify` 経由)。
pub(super) fn set_context_snapshot(ctx: ITfContext, tid: u32, dm_ptr: usize) {
    TL_LIVE_SESSION.with(|s| {
        let mut s = s.borrow_mut();
        s.ctx = Some(ctx);
        s.tid = tid;
        s.composition_dm_ptr = dm_ptr;
    });
}

/// Phase1A 用の (ctx, tid, dm_ptr) を一括クリア (`stop_live_timer` 経由)。
pub(super) fn clear_context_snapshot() {
    TL_LIVE_SESSION.with(|s| {
        let mut s = s.borrow_mut();
        s.ctx = None;
        s.tid = 0;
        s.composition_dm_ptr = 0;
    });
}

/// (ctx, tid, dm_ptr) のスナップショットを返す。`on_live_timer` 用。
pub(super) fn context_snapshot() -> (Option<ITfContext>, u32, usize) {
    TL_LIVE_SESSION.with(|s| {
        let s = s.borrow();
        (s.ctx.clone(), s.tid, s.composition_dm_ptr)
    })
}

/// 指定 dm_ptr が現在の `composition_dm_ptr` と一致するなら 0 にクリアして `true` を
/// 返す。`OnUninitDocumentMgr` 経由で DM が破棄されたとき Phase1A の stale 判定に
/// 使う。ctx / tid は触らない (DM 単位の invalidate のため)。
pub(super) fn invalidate_dm_ptr(dm_ptr: usize) -> bool {
    TL_LIVE_SESSION.with(|s| {
        let mut s = s.borrow_mut();
        if s.composition_dm_ptr == dm_ptr {
            s.composition_dm_ptr = 0;
            true
        } else {
            false
        }
    })
}

// ─── fired_once フラグ ────────────────────────────────────────────────────────

/// `fired_once` を `new` に swap し、旧値を返す。
/// 旧 `LIVE_TIMER_FIRED_ONCE_STATIC.swap(...)` と同等。
pub(super) fn swap_fired_once(new: bool) -> bool {
    TL_LIVE_SESSION.with(|s| std::mem::replace(&mut s.borrow_mut().fired_once, new))
}

/// `fired_once` を false に戻す (新サイクル開始時 / Done 状態到達時)。
pub(super) fn reset_fired_once() {
    TL_LIVE_SESSION.with(|s| s.borrow_mut().fired_once = false);
}

// ─── last_input_ms ────────────────────────────────────────────────────────────

/// `last_input_ms` を `now_ms` にセット (`live_input_notify` 入口)。
pub(super) fn store_last_input_ms(now_ms: u64) {
    TL_LIVE_SESSION.with(|s| s.borrow_mut().last_input_ms = now_ms);
}

/// `last_input_ms` を取得 (`pass_debounce` 用)。
pub(super) fn load_last_input_ms() -> u64 {
    TL_LIVE_SESSION.with(|s| s.borrow().last_input_ms)
}
