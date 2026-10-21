//! 確定待ちのキーの保留（入力処理の見直しの段 5 の 5c、docs/Input_Pipeline_Plan.md 4.2）。
//!
//! 確定の EditSession が後で実行される場合、その結果が出るまで（`commit_txn::commit_pending`）
//! に届いたキーをここに積み、結果の後処理を送ってから、届いた順に 1 つずつ実行する。
//! 確定を待たずにキーを処理すると、遅れて届く後処理（`reset_preedit` など）が後のキーの
//! 編集状態を閉じ、文字が失われるため。
//!
//! 再生の契機は 3 つ: 確定の完了の通知（`WM_APP_COMMIT_DONE`）・保留している間だけ動く
//! タイマー（[`TICK_MS`]）・次のキーの入口。どれでも [`pump`] を呼ぶ。
//!
//! 保留するのは、確定の後の状態（[`input_after_pending`]）で IME が食べるキーだけ。食べない
//! キー（全体の確定の後の Home など）は、保留せずその場でアプリへ渡す。今のキーも列の末尾に
//! 積んでから再生し、今のキーまで実行できたら、その結果で食べるかを決める。
//!
//! この段の制限（設計書 7 節）: 宛先は「キーが届いた文書（DM）」だけで照合し、フォーカスが
//! 別の文書へ移っていたキーは捨てて WARN を残す。前に保留したキーが、実行の時点で IME が
//! オフだった・IME が使わなかった場合は、アプリへ渡せずに失われる（打った文字を元の文書へ
//! 入れる扱い（前提の決定 2）は段 6）。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use windows::Win32::UI::TextServices::{ITfCompositionSink, ITfContext};
use windows_core::ComObject;

use crate::engine::user_action::UserAction;
use crate::tsf::factory::TextServiceFactory;

/// 再生のタイマーの間隔（設計書 4.2）。
pub(crate) const TICK_MS: u32 = 50;
/// 保留できるキーの数と、最初のキーからの時間の上限（設計書 4.2）。超えたキーは捨てる。
const MAX_KEYS: usize = 256;
const MAX_AGE: Duration = Duration::from_secs(30);

/// 保留したキー。
#[derive(Clone)]
pub(crate) struct HeldKey {
    pub factory: ComObject<TextServiceFactory>,
    pub action: UserAction,
    pub vk: u16,
    pub ctx: ITfContext,
    pub tid: u32,
    pub sink: ITfCompositionSink,
    /// キーが届いた文書（DM のポインタ）。
    pub dm: usize,
    pub at: Instant,
}

/// 列の中のキー（通し番号付き）。
struct Queued {
    seq: u64,
    key: HeldKey,
}

thread_local! {
    static QUEUE: RefCell<VecDeque<Queued>> = const { RefCell::new(VecDeque::new()) };
    static REPLAYING: Cell<bool> = const { Cell::new(false) };
    /// OnKeyDown の入口に入り、まだ受け付け（[`admit`]）を終えていないキーの数。
    static ADMITTING: Cell<u32> = const { Cell::new(0) };
    static NEXT_SEQ: Cell<u64> = const { Cell::new(1) };
    /// 直前の `pump` で実行したキーの結果（通し番号と、食べたか）。
    static OUTCOMES: RefCell<Vec<(u64, bool)>> = const { RefCell::new(Vec::new()) };
    /// 処理中のキーが、確定の結果を待つために止まった（[`suspend_current`]）。
    static SUSPENDED: Cell<bool> = const { Cell::new(false) };
    /// 保留したキーを再生している（[`pump`] の中で `replay_key` を呼んでいる間）。
    static IN_REPLAY: Cell<bool> = const { Cell::new(false) };
}

/// 保留したキーを再生している最中か。再生したキーがエンジンを取れないときは、捨てずに
/// 止めて（[`suspend_current`]）列に戻すために使う。
pub(crate) fn in_replay() -> bool {
    IN_REPLAY.with(|r| r.get())
}

/// 処理中のキーを、確定の結果を待つために止める（読みの長さの上限で確定した場合など）。
/// キーを処理した側（OnKeyDown・[`pump`]）が、そのキーを列に戻し、結果の後で実行する。
pub(crate) fn suspend_current() {
    SUSPENDED.with(|s| s.set(true));
}

/// 処理したキーが止まったかを返し、印を消す。
pub(crate) fn take_suspended() -> bool {
    SUSPENDED.with(|s| s.replace(false))
}

/// 止まったキーを、通し番号の順の位置に戻す（上限は数えない）。
pub(crate) fn put_back(seq: u64, key: HeldKey) {
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let at = insert_position(q.iter().map(|e| e.seq), seq);
        q.insert(at, Queued { seq, key });
    });
    crate::tsf::candidate_window::start_key_queue_timer();
}

/// 確定の結果待ちの間に届いたキーで、確定の後に入力が残ると見なすか。キーの処理中か、
/// 保留したキーがあるか、結果待ちの最後の確定が部分確定なら残る（入力が残らないなら、
/// カーソル移動などはアプリのキーになる）。処理中のキーが入力を残すかは分からないので、
/// 残るものとして扱う（後続のキーが処理中のキーを追い越してアプリへ届かないように）。
pub(crate) fn input_after_pending() -> bool {
    input_after(
        REPLAYING.with(|r| r.get())
            || ADMITTING.with(|a| a.get()) > 0
            || QUEUE.with(|q| !q.borrow().is_empty()),
        crate::tsf::commit_txn::pending_leaves_input(),
    )
}

fn input_after(busy_or_queued: bool, pending_leaves_input: Option<bool>) -> bool {
    busy_or_queued || pending_leaves_input.unwrap_or(false)
}

/// キーの受け付けの結果（[`admit`]）。
pub(crate) enum Admit {
    /// 保留せずに、すぐ処理する（`run_exclusive` の中で）。
    Direct,
    /// 保留せずに、アプリへ渡す（確定の後の状態で IME が食べないキー）。
    PassToApp,
    /// 列に積んだ（上限を超えて捨てたら `None`）。
    Held(Option<u64>),
}

/// OnKeyDown の入口に入ったキー（届いた順の通し番号）。受け付け（[`admit`]）を終えるか、
/// 受け付けずに戻る（Drop）までの間は、後から届いたキーを保留させ、再生（[`pump`]）を
/// 止める。入口の処理の途中で別のキーが割り込んでも、先に入ったキーを追い越させない。
pub(crate) struct Entry {
    seq: u64,
    open: bool,
}

impl Drop for Entry {
    fn drop(&mut self) {
        if self.open {
            ADMITTING.with(|a| a.set(a.get().saturating_sub(1)));
        }
    }
}

impl Entry {
    /// 届いた順の通し番号。
    pub(crate) fn seq(&self) -> u64 {
        self.seq
    }
}

/// OnKeyDown の最初（他の処理より前）に呼ぶ。
pub(crate) fn enter() -> Entry {
    ADMITTING.with(|a| a.set(a.get() + 1));
    Entry {
        seq: NEXT_SEQ.with(|n| n.replace(n.get() + 1)),
        open: true,
    }
}

/// キーを受け付ける。保留すべきか（キーの処理中・再生中・保留したキーがある・確定の結果
/// 待ち）を判定し、保留するなら、確定の後の状態で IME が食べるキー（`eats(入力が残るか)`）
/// だけを列に積む。判定と積むまでの間は再生・後処理を止める。キーの処理中・再生中で
/// なければ、判定の前に失効と後処理（[`settle`]）を行う。
pub(crate) fn admit(
    entry: &mut Entry,
    eats: impl FnOnce(bool) -> bool,
    make_key: impl FnOnce() -> HeldKey,
) -> Admit {
    let seq = entry.seq;
    // 先に入口に入ったキーが受け付けを終えていなければ、処理中と同じに扱う
    let was_replaying = REPLAYING.with(|r| r.replace(true));
    let busy = was_replaying || ADMITTING.with(|a| a.get()) > 1;
    if !busy {
        settle();
    }
    let queued = QUEUE.with(|q| !q.borrow().is_empty());
    let result = if !(busy || queued || crate::tsf::commit_txn::commit_pending()) {
        Admit::Direct
    } else if !eats(input_after(
        busy || queued,
        crate::tsf::commit_txn::pending_leaves_input(),
    )) {
        Admit::PassToApp
    } else {
        Admit::Held(hold(seq, make_key()))
    };
    REPLAYING.with(|r| r.set(was_replaying));
    entry.open = false;
    ADMITTING.with(|a| a.set(a.get().saturating_sub(1)));
    result
}

/// 失効した確定を片づけ、結果の出た確定の後処理を送る。
fn settle() {
    crate::tsf::commit_txn::expire_overdue(Instant::now(), crate::tsf::commit_txn::COMMIT_TIMEOUT);
    crate::tsf::commit_txn::finish_completed();
}

/// 今届いたキーを保留すべきか（確定の結果待ち・保留中のキーがある・キーの処理中・再生中）。
/// OnTestKeyDown の判定に使う（OnKeyDown は [`admit`]）。
pub(crate) fn should_hold() -> bool {
    REPLAYING.with(|r| r.get())
        || ADMITTING.with(|a| a.get()) > 0
        || QUEUE.with(|q| !q.borrow().is_empty())
        || crate::tsf::commit_txn::commit_pending()
}

/// キーを通し番号の順の位置に積み、通し番号を返す。上限を超えたら捨てて WARN（`None`）。
fn hold(seq: u64, key: HeldKey) -> Option<u64> {
    let over = QUEUE.with(|q| {
        let q = q.borrow();
        q.len() >= MAX_KEYS
            || q.front()
                .is_some_and(|first| key.at.duration_since(first.key.at) >= MAX_AGE)
    });
    if over {
        tracing::warn!(
            "key_queue: limit reached; key dropped vk={:#04x} action={:?}",
            key.vk,
            key.action
        );
        return None;
    }
    tracing::debug!(
        "key_queue: held vk={:#04x} action={:?} (commit pending)",
        key.vk,
        key.action
    );
    QUEUE.with(|q| {
        let mut q = q.borrow_mut();
        let at = insert_position(q.iter().map(|e| e.seq), seq);
        q.insert(at, Queued { seq, key });
    });
    crate::tsf::candidate_window::start_key_queue_timer();
    Some(seq)
}

/// 通し番号の順に並んだ列で、`seq` を入れる位置（後から届いた番号より前）。
fn insert_position(seqs: impl Iterator<Item = u64>, seq: u64) -> usize {
    let mut len = 0;
    for (i, s) in seqs.enumerate() {
        if s > seq {
            return i;
        }
        len = i + 1;
    }
    len
}

/// 直前の `pump` で、通し番号 `seq` のキーを実行したなら、食べたかを返す（実行して
/// いなければ `None`。まだ列にある）。
pub(crate) fn take_outcome(seq: u64) -> Option<bool> {
    OUTCOMES.with(|o| {
        let mut o = o.borrow_mut();
        let i = o.iter().position(|(s, _)| *s == seq)?;
        Some(o.remove(i).1)
    })
}

/// 失効した確定を片づけ、結果の出た確定の後処理を送り、確定の結果待ちが無ければ保留した
/// キーを届いた順に実行する。実行したキーが新たに確定を待たせたら、そこで止める。
///
/// キーの処理の途中（`run_exclusive`）・再生の途中に呼ばれたら、何もしない。
pub(crate) fn pump() {
    // 受け付けを終えていないキーがあれば、それを追い越さないよう再生しない（そのキーの
    // 受け付けの後の pump か、タイマーで再生する）
    if ADMITTING.with(|a| a.get()) > 0 {
        return;
    }
    if REPLAYING.with(|r| r.replace(true)) {
        return;
    }
    OUTCOMES.with(|o| o.borrow_mut().clear());
    settle();
    while !crate::tsf::commit_txn::commit_pending() {
        let Some(Queued { seq, key }) = QUEUE.with(|q| q.borrow_mut().pop_front()) else {
            break;
        };
        let kept = key.clone();
        let HeldKey {
            factory,
            action,
            vk,
            ctx,
            tid,
            sink,
            dm,
            ..
        } = key;
        IN_REPLAY.with(|r| r.set(true));
        let ate = factory.replay_key(action, vk, ctx, tid, sink, dm);
        IN_REPLAY.with(|r| r.set(false));
        if take_suspended() {
            if kept.at.elapsed() >= MAX_AGE {
                // 保持の上限を過ぎても実行できない: 捨てて後のキーへ進む
                tracing::warn!(
                    "key_queue: held key could not run within {MAX_AGE:?}; dropped seq={seq} vk={vk:#04x}"
                );
                continue;
            }
            // 止まった（確定の結果待ち・エンジンを取れない）: 先頭に戻し、この回の再生は
            // ここでやめる（結果の通知かタイマーで続ける）
            put_back(seq, kept);
            break;
        }
        if !ate {
            // 今のキー（OnKeyDown の中）ならアプリへ渡す。前に保留したキーは渡せない
            tracing::info!("key_queue: replayed key not used by the IME seq={seq} vk={vk:#04x}");
        }
        OUTCOMES.with(|o| o.borrow_mut().push((seq, ate)));
        crate::tsf::commit_txn::finish_completed();
    }
    REPLAYING.with(|r| r.set(false));
    if QUEUE.with(|q| q.borrow().is_empty()) && !crate::tsf::commit_txn::commit_pending() {
        crate::tsf::candidate_window::stop_key_queue_timer();
    } else {
        crate::tsf::candidate_window::start_key_queue_timer();
    }
}

/// キーの処理（`f`）の間、再生を止める。処理の途中でタイマーや通知が届いても、保留した
/// キーを割り込ませない（その間に届いたキーは保留される）。
pub(crate) fn run_exclusive<R>(f: impl FnOnce() -> R) -> R {
    let was = REPLAYING.with(|r| r.replace(true));
    let result = f();
    REPLAYING.with(|r| r.set(was));
    result
}

/// 保留したキーをすべて捨てる（Deactivate）。捨てた数を返す。
pub(crate) fn discard_all(reason: &str) -> usize {
    let dropped = QUEUE.with(|q| q.borrow_mut().drain(..).count());
    if dropped > 0 {
        tracing::warn!("key_queue: {dropped} held key(s) dropped ({reason})");
    }
    crate::tsf::candidate_window::stop_key_queue_timer();
    dropped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_remains_after_a_partial_commit_or_held_keys() {
        // 全体の確定の結果待ちで、保留したキーも無い: 入力は残らない（Home などはアプリへ）
        assert!(!input_after(false, Some(false)));
        // 部分確定の結果待ち: 残りの読みがあるので、IME のキーとして保留する
        assert!(input_after(false, Some(true)));
        // 保留したキーがある: それが入力を作るので、後のキーも保留する
        assert!(input_after(true, Some(false)));
        assert!(input_after(true, None));
        assert!(!input_after(false, None));
    }

    #[test]
    fn a_key_entering_while_another_is_being_admitted_is_not_handled_directly() {
        // 何も待っていなければ、すぐ処理する（キーは作らない）
        let mut a = enter();
        assert!(should_hold());
        assert!(matches!(
            admit(&mut a, |_| true, || unreachable!()),
            Admit::Direct
        ));
        assert!(!should_hold());
        // 先に入ったキーが受け付けを終えていない間に割り込んだキーは、処理中と同じに扱う
        // （入力が残るものとして判定し、ここでは食べないのでアプリへ渡す）
        let first = enter();
        let mut second = enter();
        assert!(matches!(
            admit(&mut second, |input_after| !input_after, || unreachable!()),
            Admit::PassToApp
        ));
        assert!(should_hold());
        drop(first);
        assert!(!should_hold());
        // 受け付けずに戻ったキーは数えない
        drop(enter());
        assert!(!should_hold());
    }

    #[test]
    fn a_key_admitted_late_is_placed_by_its_arrival_number() {
        // 入口で番号 5 を取ったキーが、割り込んだ 6 より後に積まれても、6 の前に入る
        assert_eq!(insert_position([3, 6].into_iter(), 5), 1);
        assert_eq!(insert_position([3, 4].into_iter(), 5), 2);
        assert_eq!(insert_position(std::iter::empty(), 5), 0);
        assert_eq!(insert_position([6, 7].into_iter(), 5), 0);
    }
}
