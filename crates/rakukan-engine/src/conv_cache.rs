//! 投機的変換キャッシュ — 常駐ワーカースレッド方式
//!
//! # 概要
//! `rakukan-tsf` 層から呼ばれる非同期変換 API。
//! `KanaKanjiConverter`（LLM 変換器）は DLL 境界を越えられないため、
//! バックグラウンド変換処理をエンジン内部に閉じ込める。
//!
//! # 状態遷移
//! ```text
//!  start() で pending に積む
//!       │
//!       ▼
//!  Idle（pending=Some）
//!       │ ワーカーが pending を取り出す
//!       ▼
//!  Running { key }
//!       │ 変換完了
//!       ▼
//!  Done { key, conv, candidates }
//!       │ take_ready() または reclaim()
//!       ▼
//!  Idle（pending=None）
//! ```
//!
//! # ロック規則
//! - `CACHE.inner` は `Mutex<Inner>` で保護。
//! - `try_lock()` は input 経路など Done 見落としが許容できる箇所のみ使用。
//! - `wait_done_timeout()` / `take_ready()` は `blocking lock` を使用。
//!
//! # bg_start 直後のレース
//! `start()` は `pending` に積んで即リターンするため、呼び出し直後は
//! `State::Idle && pending=Some` という中間状態になる。
//! `wait_done_timeout()` はこの状態でも Condvar で待機し、
//! ワーカーが `Running` → `Done` になったら `true` を返す。

use std::sync::{Arc, Condvar, LazyLock, Mutex};

use crate::kanji::KanaKanjiConverter;

// ─── リクエスト ────────────────────────────────────────────────────────────────

/// ワーカーへの変換リクエスト（single-slot 上書き式キュー）
struct Request {
    hiragana:  String,
    committed: String,
    converter: KanaKanjiConverter,
    n:         usize,
}

// ─── キャッシュ状態 ────────────────────────────────────────────────────────────

enum State {
    /// 変換中でも結果待ちでもない
    Idle,
    /// ワーカーが変換処理を実行中
    Running { key: String },
    /// 変換完了。`take_ready()` または `reclaim()` で回収するまで保持
    Done { key: String, converter: KanaKanjiConverter, candidates: Vec<String> },
}

struct Inner {
    state:   State,
    /// 上書き式 single-slot キュー。`start()` が積み、ワーカーが取り出す。
    /// Running 中に新リクエストが来た場合も上書きし、
    /// ワーカーは変換完了時に pending があれば warm-up 済み converter を再利用する。
    pending: Option<Request>,
}

struct Cache {
    inner: Mutex<Inner>,
    /// ワーカー ↔ 待機側（`wait_done_timeout`）の通知用 Condvar
    cond:  Condvar,
}

// KanaKanjiConverter は raw pointer を含むが、常駐ワーカースレッドを
// 単一スレッドで動かし、TSF 側の engine guard で排他制御するため安全。
unsafe impl Send for Cache {}
unsafe impl Sync for Cache {}

static CACHE: LazyLock<Arc<Cache>> = LazyLock::new(|| {
    let cache = Arc::new(Cache {
        inner: Mutex::new(Inner { state: State::Idle, pending: None }),
        cond:  Condvar::new(),
    });
    let worker = Arc::clone(&cache);
    std::thread::Builder::new()
        .name("rakukan-conv-worker".into())
        .spawn(move || worker_loop(worker))
        .expect("conv worker spawn failed");
    cache
});

// ─── ワーカーループ ────────────────────────────────────────────────────────────

fn worker_loop(cache: Arc<Cache>) {
    loop {
        // pending が来るまで Condvar で待機
        let req = {
            let mut inner = cache.inner.lock().unwrap();
            loop {
                if let Some(req) = inner.pending.take() {
                    inner.state = State::Running { key: req.hiragana.clone() };
                    // Idle → Running の遷移を wait_done_timeout（pending 待機中）に通知。
                    // この notify がないと bg_start 直後に wait_done_timeout を呼んだ側が
                    // Condvar に入れず、Running 状態のチェックが走る前に即タイムアウトする。
                    cache.cond.notify_all();
                    break req;
                }
                inner = cache.cond.wait(inner).unwrap();
            }
        };

        let key       = req.hiragana.clone();
        let committed = req.committed.clone();
        let n         = req.n;
        let converter = req.converter;

        let t = std::time::Instant::now();
        let (converter, candidates) = match std::panic::catch_unwind(
            std::panic::AssertUnwindSafe(|| converter.convert(&key, &committed, n))
        ) {
            Ok(Ok(cands)) => {
                tracing::trace!("conv-worker: {}ms {} cands key={:?}", t.elapsed().as_millis(), cands.len(), key);
                (converter, cands)
            }
            Ok(Err(e)) => {
                tracing::warn!("conv-worker error: {e}");
                (converter, vec![])
            }
            Err(_) => {
                tracing::error!("conv-worker PANIC");
                (converter, vec![])
            }
        };

        let mut inner = cache.inner.lock().unwrap();
        if let Some(pending) = inner.pending.as_mut() {
            // 変換中に新しいリクエストが来ていた場合、
            // warm-up 済み converter を次のリクエストに引き渡す（再初期化コストを節約）
            pending.converter = converter;
        } else {
            inner.state = State::Done { key, converter, candidates };
        }
        // Done 遷移を wait_done_timeout に通知
        cache.cond.notify_all();
    }
}

// ─── 公開 API ─────────────────────────────────────────────────────────────────

/// バックグラウンド変換を起動する。
///
/// `converter` を pending キューに積み、ワーカースレッドに通知する。
/// 同一キーが既に Running または Done の場合はスキップして `converter` を返す。
///
/// # 戻り値
/// - `None`       = ワーカーに渡した（converter の所有権はキャッシュへ）
/// - `Some(conv)` = 渡せなかった（同一キー実行中 or lock 取得失敗）
pub fn start(
    hiragana:  String,
    committed: String,
    converter: KanaKanjiConverter,
    n:         usize,
) -> Option<KanaKanjiConverter> {
    if hiragana.is_empty() { return Some(converter); }

    let cache = &**CACHE;
    let Ok(mut inner) = cache.inner.try_lock() else {
        return Some(converter);
    };

    // 同一キーが Done / Running 中なら再起動しない
    if let State::Done { key, .. } = &inner.state {
        if key == &hiragana {
            tracing::trace!("conv-cache: skip same key {:?}", hiragana);
            return Some(converter);
        }
    }
    if let State::Running { key } = &inner.state {
        if key == &hiragana {
            return Some(converter);
        }
    }

    inner.pending = Some(Request { hiragana, committed, converter, n });
    cache.cond.notify_one();
    None
}

/// バックグラウンド変換結果を取り出す。
///
/// `key` が一致する Done 状態であれば `Some((conv, candidates))` を返す。
/// キー不一致の場合は Done 状態を**復元**して `None` を返す
/// （呼び出し元が正しいキーで再試行できるよう Done を壊さない）。
///
/// # ロック
/// `blocking lock` を使用。`try_lock()` だとワーカーが Done を書いている瞬間に
/// "locked" を返し Done を見落とすため。
pub fn take_ready(key: &str) -> Option<(KanaKanjiConverter, Vec<String>)> {
    let cache = &**CACHE;
    let mut inner = cache.inner.lock().ok()?;

    let State::Done { .. } = &inner.state else { return None; };
    let State::Done { key: k, converter, candidates } =
        std::mem::replace(&mut inner.state, State::Idle) else { unreachable!() };

    let matched = k == key;
    if !matched {
        tracing::warn!(
            "conv-cache: take_ready MISMATCH cache_key={:?}({} bytes) req_key={:?}({} bytes)",
            k, k.len(), key, key.len()
        );
        // キー不一致 → Done 状態を復元し、呼び出し元には None を返す
        inner.state = State::Done { key: k, converter, candidates };
        return None;
    }
    tracing::trace!("conv-cache: take_ready MATCH key={:?}", key);
    Some((converter, candidates))
}

/// Done 状態の converter だけを回収して Idle に戻す（候補は捨てる）。
///
/// 新しい入力が始まった際に呼び、古い変換結果を破棄して engine に converter を返す。
/// `try_lock()` を使用するため、ロック競合時は `None` を返す。
pub fn try_reclaim_done() -> Option<KanaKanjiConverter> {
    let cache = &**CACHE;
    let mut inner = cache.inner.try_lock().ok()?;
    if let State::Done { .. } = &inner.state {
        let State::Done { converter, key, .. } =
            std::mem::replace(&mut inner.state, State::Idle) else { unreachable!() };
        tracing::trace!("conv-cache: reclaim Done key={:?}", key);
        Some(converter)
    } else {
        None
    }
}

/// `try_reclaim_done` の別名（`bg_reclaim` 経路用）
pub fn reclaim_nonblocking() -> Option<KanaKanjiConverter> {
    try_reclaim_done()
}

/// バックグラウンド変換の完了を最大 `timeout` 待つ。
///
/// # 戻り値
/// - `true`  = Done 状態になった
/// - `false` = タイムアウト、または完了しない状態（`Idle && pending=None`）
///
/// # bg_start 直後のレース対策
/// `start()` は `pending` に積んで即リターンするため、呼び出し直後は
/// `State::Idle && pending=Some` という中間状態になる。
/// この状態でもワーカーが `Running` → `Done` になるまで Condvar で待機する。
///
/// # ロック
/// `blocking lock` を使用。`try_lock()` だと Done 書き込み中を見落とすため。
pub fn wait_done_timeout(timeout: std::time::Duration) -> bool {
    let cache = &**CACHE;
    let Ok(inner) = cache.inner.lock() else { return false; };

    // すでに Done なら即 true
    if matches!(&inner.state, State::Done { .. }) { return true; }
    // Idle かつ pending なし → ワーカーが拾うものがない（変換が起動されていない）
    if matches!(&inner.state, State::Idle) && inner.pending.is_none() { return false; }
    // 以下は Condvar で待機:
    //   - Idle && pending=Some: bg_start 直後のレース（ワーカーがまだ拾っていない）
    //   - Running: 変換中

    let deadline = std::time::Instant::now() + timeout;
    let mut guard = inner;
    loop {
        let remaining = match deadline.checked_duration_since(std::time::Instant::now()) {
            Some(d) => d,
            None    => return false, // タイムアウト
        };
        let (g, timed_out) = cache.cond.wait_timeout(guard, remaining).unwrap();
        guard = g;
        if matches!(&guard.state, State::Done { .. }) { return true; }
        // pending も Running もなくなった（外部からキャンセル等）
        if matches!(&guard.state, State::Idle) && guard.pending.is_none() { return false; }
        if timed_out.timed_out() { return false; }
    }
}

/// キャッシュの現在状態名を返す（診断・ログ用）
///
/// `blocking lock` を使用。`try_lock()` だとワーカーが Done を書いている瞬間に
/// "locked" を返してしまい状態を見落とすため。
pub fn status() -> &'static str {
    match CACHE.inner.lock() {
        Ok(s) => match &s.state {
            State::Idle           => "idle",
            State::Running { .. } => "running",
            State::Done    { .. } => "done",
        },
        Err(_) => "idle", // poisoned — フォールバック
    }
}
