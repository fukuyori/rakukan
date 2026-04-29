//! ライブ変換セッション状態を集約する構造体 (M4 / T2)。
//!
//! # Phase 1 (v0.7.6) の集約スコープ
//! TSF スレッドローカルに閉じる状態 (Phase1A 用 ITfContext / DM ポインタ /
//! タイマー fired_once / last_input_ms) を `LiveConvSession` に集約。
//!
//! # Phase 2 (v0.7.7) の集約スコープ
//! cross-thread を含むグローバル状態を `LiveShared` に集約:
//! - `LIVE_PREVIEW_QUEUE` / `LIVE_PREVIEW_READY` (Phase 1B キュー)
//! - `SUPPRESS_LIVE_COMMIT_ONCE` (確定後の preview 抑制)
//! - `LIVE_CONV_GEN` (世代カウンタ)
//! - **M2 §5.3 `session_nonce`** (composition 開始ごとの identity)
//!
//! 個別の sync primitive は据え置く (`Mutex<LiveShared>` で一括包むと
//! `COMPOSITION_APPLY_LOCK` や engine ロックとの順序関係が複雑化するため)。
//! 構造体は名前空間として使い、helper 関数で更新を集約する。
//!
//! `LIVE_DEBOUNCE_CFG_MS` は設定値なので static のまま残す。

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering as AO};
use std::sync::{LazyLock, Mutex};

use windows::Win32::UI::TextServices::ITfContext;

// ═══════════════════════════════════════════════════════════════════════════
// Phase 1: TSF スレッドローカル状態
// ═══════════════════════════════════════════════════════════════════════════

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

// ═══════════════════════════════════════════════════════════════════════════
// Phase 2: cross-thread を含むグローバル状態
// ═══════════════════════════════════════════════════════════════════════════

/// Phase 1B キューに積む 1 件分の preview。
///
/// # stale 判定の二重 (M1.8 T-MID1) + 三重 (M2 §5.3) 防壁
/// - `gen_when_requested`: 要求時の `LIVE_CONV_GEN` スナップショット。
///   reading が進んでいるのに古い preview で中間を上書きする race を防ぐ
/// - `reading`: 要求時の reading。世代一致でも reading 不一致なら破棄 (M1.8)
/// - `session_nonce_at_request`: 要求時の `session_nonce` スナップショット。
///   composition が破棄→再生成された後に古い preview がキューに残って次の
///   composition に紛れ込む経路を断つ (M2 §5.3)
#[derive(Debug, Clone)]
pub struct PreviewEntry {
    pub preview: String,
    pub reading: String,
    pub gen_when_requested: u32,
    pub session_nonce_at_request: u64,
}

/// cross-thread を含む共有状態。
///
/// 個別の sync primitive (Atomic / Mutex) を据え置く方針。
/// 構造体は名前空間として機能し、helper 関数経由でアクセスする。
pub(crate) struct LiveShared {
    /// Phase 1B 用キュー (WM_TIMER → handle_action 橋渡し)。
    /// busy なら try_lock で skip し、warn ログを出す。
    preview_queue: Mutex<Option<PreviewEntry>>,
    /// Phase 1B キューに新規 entry が入った合図。
    /// `dispatch` 入口で `swap(false)` してから queue を try_lock する。
    preview_ready: AtomicBool,
    /// F6-F10 など fallback 経路の確定後に live preview コミットを 1 度だけ抑制。
    suppress_commit_once: AtomicBool,
    /// reading を変更する全経路で fetch_add される世代カウンタ (M1.8 T-MID1)。
    conv_gen: AtomicU32,
    /// composition 開始ごとに fetch_add される identity 識別子 (M2 §5.3)。
    /// PreviewEntry に snapshot を添えておき、消費時に現在値と比較する。
    session_nonce: AtomicU64,
}

pub(crate) static LIVE_SHARED: LazyLock<LiveShared> = LazyLock::new(|| LiveShared {
    preview_queue: Mutex::new(None),
    preview_ready: AtomicBool::new(false),
    suppress_commit_once: AtomicBool::new(false),
    conv_gen: AtomicU32::new(0),
    session_nonce: AtomicU64::new(0),
});

// ─── Phase 1B キュー ──────────────────────────────────────────────────────────

/// Phase 1B キューに preview を書き込む (`queue_phase1b` 用)。
/// - 成功時 `true` を返し、`preview_ready` を Release で立てる
/// - busy なら `false` を返す (呼び出し側が warn ログ)
pub(crate) fn queue_preview_set(entry: PreviewEntry) -> bool {
    if let Ok(mut q) = LIVE_SHARED.preview_queue.try_lock() {
        *q = Some(entry);
        LIVE_SHARED.preview_ready.store(true, AO::Release);
        true
    } else {
        false
    }
}

/// Phase 1B キューを消費 (`dispatch` 用)。
/// - `preview_ready` が立っていなければ `None`
/// - 立っていれば `swap(false)` してから try_lock + take
/// - lock busy のときは entry を取れず `None` (次回入口で再試行される)
pub(crate) fn queue_preview_consume() -> Option<PreviewEntry> {
    if !LIVE_SHARED.preview_ready.swap(false, AO::AcqRel) {
        return None;
    }
    LIVE_SHARED
        .preview_queue
        .try_lock()
        .ok()
        .and_then(|mut q| q.take())
}

/// Phase 1B キューをクリア (`on_input` / `on_convert` / `on_cancel` / `on_backspace` で
/// LiveConv 状態を抜ける時に使用)。
/// `preview_ready` を先に下ろし、try_lock 取れた場合のみ entry を破棄。
/// busy のときも ready=false で「消費しても apply されない」状態になるため安全。
pub(crate) fn queue_preview_clear() {
    LIVE_SHARED.preview_ready.store(false, AO::Release);
    if let Ok(mut q) = LIVE_SHARED.preview_queue.try_lock() {
        *q = None;
    }
}

// ─── SUPPRESS_LIVE_COMMIT_ONCE ────────────────────────────────────────────────

/// 「次の commit 時に live preview の自動 commit を 1 度だけ抑制する」を armed 状態にする
/// (F6-F10 の fallback 等)。
pub(crate) fn suppress_commit_arm() {
    LIVE_SHARED.suppress_commit_once.store(true, AO::Release);
}

/// 抑制状態を明示クリア (`on_input` 入口で確実に initial 状態にしたい場合)。
pub(crate) fn suppress_commit_clear() {
    LIVE_SHARED.suppress_commit_once.store(false, AO::Release);
}

/// armed なら 1 度だけ true を返してクリア。`on_commit_raw` の fallback 経路で使う。
pub(crate) fn suppress_commit_take() -> bool {
    LIVE_SHARED.suppress_commit_once.swap(false, AO::AcqRel)
}

// ─── LIVE_CONV_GEN (M1.8 T-MID1) ──────────────────────────────────────────────

/// reading を変更する全経路から呼ぶ。世代カウンタを前進させる。
#[inline]
pub(crate) fn conv_gen_bump() {
    LIVE_SHARED.conv_gen.fetch_add(1, AO::Release);
}

/// 現在の世代を取得。preview 要求時のスナップショットや stale 判定に使う。
#[inline]
pub(crate) fn conv_gen_snapshot() -> u32 {
    LIVE_SHARED.conv_gen.load(AO::Acquire)
}

// ─── session_nonce (M2 §5.3) ──────────────────────────────────────────────────

/// composition 開始のたびに fetch_add される identity 識別子。
/// `composition_set_with_dm` で `Some(...)` がセットされる経路から呼ぶ。
#[inline]
pub(crate) fn session_nonce_advance() {
    LIVE_SHARED.session_nonce.fetch_add(1, AO::Release);
}

/// 現在の session_nonce を取得。preview 要求時のスナップショットや
/// 消費時の stale 判定に使う。
#[inline]
pub(crate) fn session_nonce_snapshot() -> u64 {
    LIVE_SHARED.session_nonce.load(AO::Acquire)
}
