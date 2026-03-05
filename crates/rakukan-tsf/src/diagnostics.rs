//! rakukan 診断モジュール
//!
//! # 設計方針
//! - ホットパス（OnKeyDown）に I/O・ロック待ちを置かない
//! - try_lock で取れないときは無視（IME をブロックしない）
//! - Ctrl+Shift+F12 でリングバッファ全体 + 現在状態をログに一括ダンプ
//!
//! # ログレベルの使い分け
//! | レベル | 用途                                       |
//! |--------|--------------------------------------------|
//! | ERROR  | 回復不可能なエラー（panic捕捉など）        |
//! | WARN   | 想定外だが継続可能（SetValue失敗など）     |
//! | INFO   | ライフサイクルイベント（Activate等）       |
//! | DEBUG  | キー1つの処理（変換結果など）              |
//! | TRACE  | OnTestKeyDown 等の超高頻度イベント         |
//!
//! # 使い方
//! ```rust
//! // タイミング計測（Drop時に自動記録）
//! let _t = diag::span("Convert");
//!
//! // イベント記録（ファイルI/Oなし）
//! diag::event(DiagEvent::Convert { preedit: "...", result: "..." });
//!
//! // Ctrl+Shift+F12 から呼ぶ
//! diag::dump_snapshot();
//! ```

use std::sync::{LazyLock, Mutex};
use std::time::Instant;

// ─── イベント型 ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum DiagEvent {
    // ライフサイクル
    Activate         { tid: u32 },
    Deactivate,

    // コンパートメント（KEYBOARD_OPENCLOSE）
    /// set_open_close の結果
    CompartmentSet   { open: bool, ok: bool, err: Option<String> },
    /// get_open_close で読んだ値
    CompartmentRead  { value: i32 },

    // 言語バー
    LangbarAdd       { ok: bool, err: Option<String> },

    // キーイベント
    /// OnKeyDown でアクションを処理した
    KeyHandled       { vk: u16, action: &'static str, ate: bool },
    /// OnKeyDown でアクションが解決されなかった / 前提条件を満たさず無視
    KeyIgnored       { vk: u16, reason: &'static str },

    // 入力・変換
    InputChar        { ch: char, preedit_after: String },
    Backspace        { preedit_after: String },
    Convert          { preedit: String, kanji_ready: bool, result: String },
    CommitRaw        { preedit: String },
    ModeChange       { from: String, to: &'static str },

    // エラー
    Error            { site: &'static str, msg: String },
    Panic            { site: &'static str },

    // タイミング（Span の Drop で自動挿入）
    Timing           { label: &'static str, elapsed_us: u64 },
}

// ─── リングバッファ ──────────────────────────────────────────────────────────

const RING_CAP: usize = 256;

struct Ring {
    buf:   Vec<(u64 /* elapsed_us */, DiagEvent)>,
    head:  usize,
    full:  bool,
    epoch: Instant,
}

impl Ring {
    fn new() -> Self {
        Self {
            buf:   Vec::with_capacity(RING_CAP),
            head:  0,
            full:  false,
            epoch: Instant::now(),
        }
    }

    fn push(&mut self, ev: DiagEvent) {
        let ts = self.epoch.elapsed().as_micros() as u64;
        if self.full {
            self.buf[self.head] = (ts, ev);
        } else {
            self.buf.push((ts, ev));
            if self.buf.len() == RING_CAP {
                self.full = true;
            }
        }
        self.head = (self.head + 1) % RING_CAP;
    }

    /// 古い順に収集（DoubleEndedIterator が不要になる）
    fn collect_asc(&self) -> Vec<&(u64, DiagEvent)> {
        let start = if self.full { self.head } else { 0 };
        let len   = self.buf.len();
        (0..len).map(|i| &self.buf[(start + i) % len]).collect()
    }
}

static RING: LazyLock<Mutex<Ring>> = LazyLock::new(|| Mutex::new(Ring::new()));

// ─── 公開 API ────────────────────────────────────────────────────────────────

/// イベントをリングバッファに記録（ファイルI/Oなし、ブロックしない）
#[inline]
pub fn event(ev: DiagEvent) {
    // try_lock: 取れなければ捨てる（ホットパスを止めない）
    if let Ok(mut r) = RING.try_lock() {
        r.push(ev);
    }
}

/// RAII タイミング計測。Drop 時に DiagEvent::Timing を記録する。
pub struct Span {
    label:   &'static str,
    start:   Instant,
    active:  bool,
}

impl Span {
    pub fn new(label: &'static str) -> Self {
        Self { label, start: Instant::now(), active: true }
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if self.active {
            let us = self.start.elapsed().as_micros() as u64;
            event(DiagEvent::Timing { label: self.label, elapsed_us: us });
            // 5ms を超えたら WARN（体感で遅いと感じる閾値）
            if us > 5_000 {
                tracing::warn!("⚠ SLOW [{label}] {us}µs", label = self.label);
            }
        }
    }
}

/// タイミング計測スパンを開始する
pub fn span(label: &'static str) -> Span {
    Span::new(label)
}

// ─── スナップショットダンプ ──────────────────────────────────────────────────
// Ctrl+Shift+F12 で呼ぶ。ログファイルに全状態 + 直近イベントを出力する。

pub fn dump_snapshot() {
    tracing::info!("═══════════════════════════════════════════");
    tracing::info!("  rakukan Diagnostics Snapshot");
    tracing::info!("═══════════════════════════════════════════");
    dump_state();
    dump_events();
    dump_timing();
    tracing::info!("═══════════════════════════════════════════");
}

fn dump_state() {
    tracing::info!("─── Current State ───────────────────────");

    // InputMode
    match crate::engine::state::ime_state_get() {
        Ok(s)  => tracing::info!("  input_mode  = {:?}", s.input_mode),
        Err(e) => tracing::warn!("  input_mode  = LOCK ERROR: {e}"),
    }

    // Engine
    match crate::engine::state::engine_get() {
        Ok(g) => match g.as_ref() {
            Some(e) => {
                let preedit = e.preedit_display();
                tracing::info!("  preedit     = {:?}", preedit);
                tracing::info!("  kanji_ready = {}", e.is_kanji_ready());
            }
            None => tracing::warn!("  engine      = None"),
        },
        Err(e) => tracing::warn!("  engine      = LOCK ERROR: {e}"),
    }

    // Composition
    match crate::engine::state::composition_clone() {
        Ok(Some(_)) => tracing::info!("  composition = active"),
        Ok(None)    => tracing::info!("  composition = None"),
        Err(e)      => tracing::warn!("  composition = LOCK ERROR: {e}"),
    }
}

fn dump_events() {
    // 直近 60 件を新しい順で出力
    let events: Vec<_> = {
        let Ok(r) = RING.lock() else { return; };
        r.collect_asc()
            .into_iter().rev()
            .take(60)
            .map(|(ts, ev)| (*ts, format!("{ev:?}")))
            .collect()
    };

    tracing::info!("─── Recent Events (newest first, max 60) ─");
    if events.is_empty() {
        tracing::info!("  (empty)");
    }
    for (ts, desc) in &events {
        tracing::info!("  +{ts:>9}µs  {desc}");
    }
}

fn dump_timing() {
    use std::collections::HashMap;

    let Ok(r) = RING.lock() else { return; };
    let mut stats: HashMap<&'static str, (u64, u64, u64, u32)> = HashMap::new();

    for (_, ev) in r.collect_asc() {
        if let DiagEvent::Timing { label, elapsed_us } = ev {
            let e = stats.entry(label).or_insert((u64::MAX, 0, 0, 0));
            e.0 = e.0.min(*elapsed_us);
            e.1 = e.1.max(*elapsed_us);
            e.2 += elapsed_us;
            e.3 += 1;
        }
    }

    if stats.is_empty() { return; }

    tracing::info!("─── Timing Stats ────────────────────────");
    let mut rows: Vec<_> = stats.into_iter().collect();
    rows.sort_by_key(|(_, (_, max, _, _))| std::cmp::Reverse(*max));

    for (label, (min, max, sum, n)) in rows {
        let avg = sum / n as u64;
        tracing::info!(
            "  {label:<20}  n={n:>3}  avg={avg:>6}µs  min={min:>6}µs  max={max:>6}µs"
        );
    }
}
