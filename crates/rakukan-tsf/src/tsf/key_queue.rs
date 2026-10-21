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
use std::rc::Rc;
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

/// 処置の途中に届いた文字（届いた順の通し番号・文書・文脈・クライアント ID・文字）。
type ExtraChar = (u64, usize, ITfContext, u32, char);
/// 同じ文書へ入れる文字列（文書・文脈・クライアント ID・文字列）。
type TextBatch = (usize, ITfContext, u32, String);

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
    /// 回復できない場合の処置の後の直接入力（[`direct_fallback`]）。
    static DIRECT_FALLBACK: Cell<bool> = const { Cell::new(false) };
    /// 回復できない場合の処置で、文書へ入れている文字列。
    static RESCUES: RefCell<Vec<Rescue>> = const { RefCell::new(Vec::new()) };
    /// 回復できない場合の処置の途中（文字を入れ終わるまで）。この間のキーはアプリへ渡さず、
    /// 文字キーは打った文字として後で同じように入れる（[`absorb_into_rescue`]）。
    static RESCUE_ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// 処置の途中に届いた文字キーの文字（届いた順の通し番号・文書・文字）。
    static RESCUE_EXTRA: RefCell<Vec<ExtraChar>> = const { RefCell::new(Vec::new()) };
    /// 文字を入れる要求の失効の世代（Deactivate で進める）。取り出し中の要求にも届く。
    static RESCUE_EPOCH: Cell<u64> = const { Cell::new(0) };
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
    check_rescues();
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
                if engine_unavailable() {
                    // エンジンが使えないまま保持の上限を過ぎた: 回復できない場合の処置
                    // （列の文字キーを打った文字のまま入れ、直接入力に切り替える）
                    give_up(kept);
                    break;
                }
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
    if QUEUE.with(|q| q.borrow().is_empty())
        && !crate::tsf::commit_txn::commit_pending()
        && RESCUES.with(|r| r.borrow().is_empty())
    {
        crate::tsf::candidate_window::stop_key_queue_timer();
    } else {
        crate::tsf::candidate_window::start_key_queue_timer();
    }
}

/// 保留したキーを再生している最中か（再生するキーは、食べたキーなのでアプリへは渡せない）。
pub(crate) fn in_replay() -> bool {
    IN_REPLAY.with(|r| r.get())
}

/// エンジンが使えない（まだ無い・ロックが取れない）。
fn engine_unavailable() -> bool {
    crate::engine::state::engine_try_get().map_or(true, |g| g.0.is_none())
}

/// 文字キーが出す文字（打った文字のまま。回復できない場合の処置で文書へ入れる）。
fn typed_text(action: &UserAction) -> Option<char> {
    match action {
        UserAction::Input(c) | UserAction::InputRaw(c) | UserAction::Punctuate(c) => Some(*c),
        UserAction::FullWidthSpace => Some(' '),
        _ => None,
    }
}

/// 回復できない場合の処置で、文書へ入れている文字列（[`give_up`]）。
struct Rescue {
    dm: usize,
    ctx: ITfContext,
    tid: u32,
    text: String,
    /// 要求した回数（3 回まで）。
    attempts: u32,
    state: Rc<Cell<crate::tsf::factory::InsertState>>,
    at: Instant,
}

/// 文字を入れる要求の上限の回数と、1 回の要求が実行されるのを待つ時間。
const RESCUE_ATTEMPTS: u32 = 3;
const RESCUE_TIMEOUT: Duration = Duration::from_secs(2);

/// 回復できない場合の処置（入力処理の見直しの段 6、前提の決定 3-B）: 先頭の `first` と
/// 列のキーのうち、文字キーを打った文字のまま、そのキーが届いた文書の選択位置へ入れる
/// （同じ文書へ続く文字は 1 回で。届いた文書が破棄されていれば入れない。前提の決定 2）。
/// 文字以外のキーは捨てる。その後、入れ終わり、エンジンが使えるようになるまで直接入力に
/// する（[`direct_fallback`]）。入れた結果は [`pump`] で確かめる（[`check_rescues`]）。
fn give_up(first: HeldKey) {
    let mut keys = vec![first];
    keys.extend(QUEUE.with(|q| q.borrow_mut().drain(..).map(|e| e.key).collect::<Vec<_>>()));
    let mut dropped = 0usize;
    let mut batches: Vec<TextBatch> = Vec::new();
    for key in keys {
        let Some(c) = typed_text(&key.action) else {
            dropped += 1;
            continue;
        };
        match batches.last_mut() {
            Some((dm, _, _, text)) if *dm == key.dm => text.push(c),
            _ => batches.push((key.dm, key.ctx.clone(), key.tid, c.to_string())),
        }
    }
    tracing::warn!(
        "key_queue: engine unavailable for {MAX_AGE:?}; inserting {} typed character batch(es) as is, {dropped} other key(s) dropped; direct input until the engine is back",
        batches.len()
    );
    // 状態と表示を先に決める（入れられなかった通知を、この表示で上書きしない）
    DIRECT_FALLBACK.with(|d| d.set(true));
    RESCUE_ACTIVE.with(|a| a.set(true));
    crate::tsf::factory::show_engine_waiting();
    crate::tsf::candidate_window::start_key_queue_timer();
    let epoch = RESCUE_EPOCH.with(|e| e.get());
    for (dm, ctx, tid, text) in batches {
        request_rescue(new_rescue(dm, ctx, tid, text), epoch);
    }
    // 同期で入れ終わっていれば、ここで片づける
    check_rescues();
}

/// 回復できない場合の処置の途中か。この間のキーは [`absorb_into_rescue`] に渡す
/// （IME のモードキーを除く）。
pub(crate) fn rescue_active() -> bool {
    RESCUE_ACTIVE.with(|a| a.get())
}

/// 処置の途中に届いたキー: 文字キーは打った文字として、今入れている文字の後に入れる
/// （アプリへ渡すと、まだ入っていない文字を追い越すため）。届いた順の通し番号（`seq`）で
/// 並べる。文字以外は捨てる。
pub(crate) fn absorb_into_rescue(
    seq: u64,
    action: &UserAction,
    dm: usize,
    ctx: &ITfContext,
    tid: u32,
) {
    let Some(c) = typed_text(action) else {
        tracing::warn!("key_queue: key dropped while inserting typed characters action={action:?}");
        return;
    };
    RESCUE_EXTRA.with(|e| e.borrow_mut().push((seq, dm, ctx.clone(), tid, c)));
}

/// 処置の途中に届いた文字を、届いた順に並べ、同じ文書へ続く文字をまとめる。
fn batch_extra<C>(mut extra: Vec<(u64, usize, C, u32, char)>) -> Vec<(usize, C, u32, String)> {
    extra.sort_by_key(|e| e.0);
    let mut batches: Vec<(usize, C, u32, String)> = Vec::new();
    for (_, dm, ctx, tid, c) in extra {
        match batches.last_mut() {
            Some((last_dm, _, _, text)) if *last_dm == dm => text.push(c),
            _ => batches.push((dm, ctx, tid, c.to_string())),
        }
    }
    batches
}

fn new_rescue(dm: usize, ctx: ITfContext, tid: u32, text: String) -> Rescue {
    Rescue {
        dm,
        ctx,
        tid,
        text,
        attempts: 0,
        state: Rc::new(Cell::new(crate::tsf::factory::InsertState::Pending)),
        at: Instant::now(),
    }
}

/// 要求を失効させ（後で実行されても文書に触れない）、入れられなかったものとして記録する。
fn expire_rescue(rescue: &Rescue, reason: &str) {
    if rescue.state.get() != crate::tsf::factory::InsertState::Inserted {
        rescue
            .state
            .set(crate::tsf::factory::InsertState::NotInserted {
                reason: "expired",
                retryable: false,
            });
        unrescued(&rescue.text, reason);
    }
}

/// 文字を入れる要求を出して記録する。届いた文書が破棄されていれば入れずに記録だけする。
/// `epoch` は要求を始めた時点の失効の世代。要求の間に Deactivate で進んでいたら、出した
/// 要求を失効させて記録する。
fn request_rescue(mut rescue: Rescue, epoch: u64) {
    // 要求の時点の文書（ポインタと世代）。実行の時点でも同じ文書が生きているかを確かめる
    let Some(dm_ref) = crate::engine::dm_registry::lookup(rescue.dm)
        .filter(|dm| crate::engine::dm_registry::is_live(*dm))
    else {
        unrescued(&rescue.text, "document gone");
        return;
    };
    rescue.attempts += 1;
    rescue.at = Instant::now();
    // EditSession の実行の時点で、失効していない（Deactivate で世代が進んでいない）こと、
    // 文書が破棄されていないことを確かめる
    let still_valid: Box<dyn Fn() -> bool> = Box::new(move || {
        RESCUE_EPOCH.with(|e| e.get()) == epoch && crate::engine::dm_registry::is_live(dm_ref)
    });
    rescue.state =
        crate::tsf::factory::insert_text_direct(&rescue.ctx, rescue.tid, &rescue.text, still_valid);
    if RESCUE_EPOCH.with(|e| e.get()) != epoch {
        expire_rescue(&rescue, "deactivated");
        return;
    }
    RESCUES.with(|r| r.borrow_mut().push(rescue));
}

/// 入れられなかった文字列を記録し、通知する（設計書 4.2 の救済の原則）。
fn unrescued(text: &str, reason: &str) {
    tracing::warn!(
        "key_queue: typed characters could not be inserted ({reason}); chars={} text={text:?}",
        text.chars().count()
    );
    // 入力の一部を文書へ入れられなかったことを知らせる（エンジン待ちの表示とは別）
    crate::tsf::mode_indicator::show("未入力", 0, 0);
}

/// 文字を入れる要求の結果を確かめる。入れた・入れられない（位置を確かめられない）なら
/// 片づけ、文書を変えていない失敗・時間切れなら上限まで要求し直す。
fn check_rescues() {
    use crate::tsf::factory::InsertState;
    let epoch = RESCUE_EPOCH.with(|e| e.get());
    let rescues = RESCUES.with(|r| std::mem::take(&mut *r.borrow_mut()));
    for rescue in rescues {
        // 取り出した後に Deactivate で失効した
        if RESCUE_EPOCH.with(|e| e.get()) != epoch {
            expire_rescue(&rescue, "deactivated");
            continue;
        }
        let mut state = rescue.state.get();
        if state == InsertState::Pending && rescue.at.elapsed() >= RESCUE_TIMEOUT {
            // 失効させる（後で実行されても文書に触れない）
            state = InsertState::NotInserted {
                reason: "timeout",
                retryable: true,
            };
            rescue.state.set(state);
        }
        match state {
            InsertState::Inserted => {}
            InsertState::Pending | InsertState::Running => {
                RESCUES.with(|r| r.borrow_mut().push(rescue));
            }
            InsertState::NotInserted {
                retryable: true, ..
            } if rescue.attempts < RESCUE_ATTEMPTS => {
                request_rescue(rescue, epoch);
            }
            InsertState::NotInserted { reason, .. } => unrescued(&rescue.text, reason),
        }
    }
    // 入れ終わったら、処置の途中に届いた文字を続けて入れる。それも無ければ処置を終える
    if rescue_active()
        && RESCUE_EPOCH.with(|e| e.get()) == epoch
        && RESCUES.with(|r| r.borrow().is_empty())
    {
        let extra = RESCUE_EXTRA.with(|e| std::mem::take(&mut *e.borrow_mut()));
        for (dm, ctx, tid, text) in batch_extra(extra) {
            request_rescue(new_rescue(dm, ctx, tid, text), epoch);
        }
        if RESCUES.with(|r| r.borrow().is_empty()) && RESCUE_EXTRA.with(|e| e.borrow().is_empty()) {
            RESCUE_ACTIVE.with(|a| a.set(false));
        }
    }
}

/// 直接入力（回復できない場合の処置の後）。IME のモードキー以外はアプリへ渡す。文字を
/// 入れ終わり、エンジンが使えるようになったら外す（キーの入口で確かめる）。直接入力の
/// 間なら `true`。
pub(crate) fn direct_fallback() -> bool {
    if !DIRECT_FALLBACK.with(|d| d.get()) {
        return false;
    }
    if rescue_active() {
        return true;
    }
    // エンジンを取れれば外す（取れなければ、初期化をもう一度促す）
    if crate::engine::state::engine_try_get_or_create().is_ok() {
        DIRECT_FALLBACK.with(|d| d.set(false));
        tracing::info!("key_queue: engine is back; direct input ended");
        return false;
    }
    true
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
    // 文字を入れる要求は失効させ（後で実行されても文書に触れない）、入れられなかった
    // ものとして記録する
    // 取り出し中・要求中の要求にも届くよう、失効の世代を進める
    RESCUE_EPOCH.with(|e| e.set(e.get() + 1));
    let rescues = RESCUES.with(|r| std::mem::take(&mut *r.borrow_mut()));
    for rescue in rescues {
        expire_rescue(&rescue, reason);
    }
    let extra = RESCUE_EXTRA.with(|e| std::mem::take(&mut *e.borrow_mut()));
    for (_, _, _, text) in batch_extra(extra) {
        unrescued(&text, reason);
    }
    RESCUE_ACTIVE.with(|a| a.set(false));
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
    fn only_character_keys_are_inserted_as_typed_when_giving_up() {
        assert_eq!(typed_text(&UserAction::Input('k')), Some('k'));
        assert_eq!(typed_text(&UserAction::InputRaw('/')), Some('/'));
        assert_eq!(typed_text(&UserAction::Punctuate('.')), Some('.'));
        assert_eq!(typed_text(&UserAction::FullWidthSpace), Some(' '));
        assert_eq!(typed_text(&UserAction::Backspace), None);
        assert_eq!(typed_text(&UserAction::CommitRaw), None);
        assert_eq!(typed_text(&UserAction::ImeToggle), None);
    }

    #[test]
    fn characters_typed_during_a_rescue_are_inserted_in_arrival_order() {
        // 割り込みで後の番号が先に届いても、番号の順に並べ、同じ文書へ続く文字をまとめる
        let extra = vec![
            (7, 1, (), 0, 'c'),
            (5, 1, (), 0, 'a'),
            (6, 1, (), 0, 'b'),
            (8, 2, (), 0, 'x'),
        ];
        let batches: Vec<(usize, String)> = batch_extra(extra)
            .into_iter()
            .map(|(dm, _, _, text)| (dm, text))
            .collect();
        assert_eq!(batches, vec![(1, "abc".to_string()), (2, "x".to_string())]);
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
