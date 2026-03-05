//! 投機的変換キャッシュ — 常駐ワーカースレッド方式
//!
//! rakukan-tsf から rakukan-engine へ移動。
//! KanaKanjiConverter が DLL 境界を越えないよう、background 変換を
//! エンジン内部に閉じ込める。
//!
//! # 状態遷移
//! ```
//! Idle ──start()──► Running { key }
//!                        │ ワーカー完了
//!                        ▼
//!                 Done { key, conv, candidates }
//!                        │ take_ready() / reclaim()
//!                        ▼
//!                       Idle
//! ```

use std::sync::{Arc, Condvar, LazyLock, Mutex};

use crate::kanji::KanaKanjiConverter;

// ─── リクエスト ────────────────────────────────────────────────────────────────

struct Request {
    hiragana:  String,
    committed: String,
    converter: KanaKanjiConverter,
    n:         usize,
}

// ─── キャッシュ状態 ────────────────────────────────────────────────────────────

enum State {
    Idle,
    Running { key: String },
    Done    { key: String, converter: KanaKanjiConverter, candidates: Vec<String> },
}

struct Inner {
    state:   State,
    /// 上書き式 single-slot キュー
    pending: Option<Request>,
}

struct Cache {
    inner: Mutex<Inner>,
    cond:  Condvar,
}

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

fn worker_loop(cache: Arc<Cache>) {
    loop {
        let req = {
            let mut inner = cache.inner.lock().unwrap();
            loop {
                if let Some(req) = inner.pending.take() {
                    inner.state = State::Running { key: req.hiragana.clone() };
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
            // 新しいリクエストが来ていたら warm-up 済み converter を再利用
            pending.converter = converter;
        } else {
            inner.state = State::Done { key, converter, candidates };
        }
        cache.cond.notify_all();
    }
}

// ─── 公開 API ─────────────────────────────────────────────────────────────────

/// BG 変換を起動する。
/// 戻り値: `None` = ワーカーに渡した / `Some(conv)` = 渡せなかった（caller が保持）
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

/// Space 押下時。key が一致すれば `Some((conv, candidates))` を返す。
pub fn take_ready(key: &str) -> Option<(KanaKanjiConverter, Vec<String>)> {
    let cache = &**CACHE;
    let mut inner = cache.inner.try_lock().ok()?;

    let State::Done { .. } = &inner.state else { return None; };
    let State::Done { key: k, converter, candidates } =
        std::mem::replace(&mut inner.state, State::Idle) else { unreachable!() };

    let matched = k == key;
    tracing::trace!("conv-cache: take_ready key={:?} match={}", k, matched);
    Some((converter, if matched { candidates } else { vec![] }))
}

/// Done 状態の converter を返す（Input 前に呼んで engine に戻す）
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

/// 非ブロッキングで converter を回収（Done のみ）
pub fn reclaim_nonblocking() -> Option<KanaKanjiConverter> {
    try_reclaim_done()
}

/// BG 変換の完了を最大 `timeout` 待つ。
/// Done になれば `true`、タイムアウトなら `false`。
/// Running / Idle でない（そもそも起動していない）場合も即 `false` を返す。
pub fn wait_done_timeout(timeout: std::time::Duration) -> bool {
    let cache = &**CACHE;
    let Ok(inner) = cache.inner.lock() else { return false; };

    // すでに Done なら即 true
    if matches!(&inner.state, State::Done { .. }) { return true; }
    // Running でなければ待っても完了しない
    if !matches!(&inner.state, State::Running { .. }) { return false; }

    // Condvar でワーカー完了通知を待つ
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
        if timed_out.timed_out() { return false; }
    }
}

/// 状態名（診断・FFI 用）
pub fn status() -> &'static str {
    match CACHE.inner.try_lock() {
        Ok(s) => match &s.state {
            State::Idle           => "idle",
            State::Running { .. } => "running",
            State::Done    { .. } => "done",
        },
        Err(_) => "locked",
    }
}
