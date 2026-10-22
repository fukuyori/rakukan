//! 確定トランザクション（入力処理の見直しの段 5、docs/Input_Pipeline_Plan.md 4.6）。
//!
//! 文書を先に確定し、ホストへの後処理（BG の回収・学習・文脈・編集状態を閉じる）は、
//! 文書の結果を見てから送る。EditSession の中では RPC を送らない。ただし部分確定は、
//! EditSession の中で文書の確定部分を確定した後に、引き継ぎ（学習・文脈・残りの読み）を
//! 1 回送る（5c-3、設計書 4.6 の部分確定の文書先行）。部分確定の後処理（表示の要求・BG の
//! 開始・確定しなかったときに閉じる処理）は、要求に持たせた [`set_after`] の処理で行う。
//!
//! EditSession は同期で実行されることも、後で実行されることもある。どちらの場合も
//! EditSession の最後で結果を [`ReqState::Done`] に記録し、`WM_APP_COMMIT_DONE` を送る。
//! 要求した側も、要求の直後に [`finish_completed`] を呼ぶ（同期で実行されていれば
//! そこで後処理する）。後処理は 1 回だけ行う。
//!
//! 結果を待つ間（[`commit_pending`]）に届いたキーは、`key_queue` が保留し、後処理の後で
//! 実行する（段 5 の 5c）。要求が [`COMMIT_TIMEOUT`] たっても実行されなければ失効させ
//! （[`expire_overdue`]）、実行されなかったものとして扱う。
//!
//! 確定しなかったとき（[`DocResult::Aborted`]・[`DocResult::NotRun`]）は、composition を保持し
//! 直し（EditSession の中）、ホストには何も送らずに読みを残し、キーの入口で控えたセッションの
//! 状態（候補・選択・句読点の保留）に戻す（段 5 の 5c の残り、設計書 4.6）。確定の操作が
//! 途中まで進んで composition を手放した（[`DocResult::Failed`]）ときは今までどおり、
//! BG の回収と編集状態を閉じる処理だけを送る。

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::engine::state::LearnAction;

/// 確定の要求が実行されるのを待つ時間。過ぎたら失効させる（設計書 4.6 の `ReqCell`）。
pub(crate) const COMMIT_TIMEOUT: Duration = Duration::from_secs(2);

/// 確定の後に送る学習。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LearnPlan {
    pub reading: String,
    pub surface: String,
    pub action: LearnAction,
}

/// 文書を確定できたときに、ホストへ送る後処理。
#[derive(Debug, Clone, Default)]
pub(crate) struct CommitEffects {
    /// BG 変換の converter を回収する。
    pub bg_reclaim: bool,
    /// 学習。確定した文字列が計画の文字列と同じときだけ送る。
    pub learn: Option<LearnPlan>,
    /// 確定した文字列を LLM の文脈に足す（`commit`）。
    pub context: bool,
    /// 編集状態を閉じる（`reset_preedit`）。
    pub reset_preedit: bool,
    /// 最後に、この読みで BG 変換を始める（`start_live_bg_if_ready`。部分確定の残り）。
    pub bg_start: Option<String>,
    /// 確定しなかったときも composition を手放し、編集状態を閉じる（今までの扱い）。IME の
    /// 切り替えの前の確定で使う（切り替えた後に composition を残さないため）。
    pub close_on_abort: bool,
}

/// 文書の結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DocResult {
    /// 確定した。`text` は文書に確定した文字列。
    Committed { text: String },
    /// 確定の操作は通ったが、確定した文字列が分からない（置き換えにも元の文字列の
    /// 読み取りにも失敗した）。
    CommittedUnknown,
    /// 確定の操作に失敗した。composition は手放した（今までと同じ）。
    Failed(&'static str),
    /// 確定しなかった。composition は保持し直した。`changed` なら、範囲を元の文字列に戻せず、
    /// 置き換えた文字列のまま残っている（表示が古い）。
    Aborted { why: &'static str, changed: bool },
    /// 実行されなかった（要求の失敗・拒否・時間切れ）。
    NotRun(&'static str),
}

/// 要求の状態。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReqState {
    Pending,
    Running,
    Done(DocResult),
}

pub(crate) type ReqCell = Rc<RefCell<ReqState>>;

struct PendingCommit {
    cell: ReqCell,
    /// 計画の文字列（学習を送るかの判定に使う）。
    planned: String,
    effects: CommitEffects,
    /// 要求した時刻（失効の判定に使う）。
    at: Instant,
    /// 部分確定（確定の後に新しい composition の入力が残る）。キーを保留するかの判定に使う。
    leaves_input: bool,
    /// 部分確定の「composition の置き換え待ち」の印（`set_pending_replacement`）。失効させる
    /// ときに消す。
    replacement: Option<u64>,
    /// 確定しなかったときに戻すセッションの状態（キーの入口で控えたもの。キーの処理の外の
    /// 確定では `None`）。
    restore: Option<crate::engine::state::SessionState>,
    /// 確定を要求した時点の composition（`composition_key`。文書と同一性）と、その参照。
    /// 確定しなかったときに、この composition がまだ保持されている場合だけ読みを残す。
    /// 参照は、終えるまでの間に同一性が別の composition に使い直されないために持つ。
    target: Option<(
        (usize, usize),
        windows::Win32::UI::TextServices::ITfComposition,
    )>,
    /// 後処理の最後に行う処理（部分確定。[`set_after`]）。引数は結果と、composition と
    /// ホストの読みを残すか（`keep`）。
    after: Option<AfterCommit>,
}

/// 確定の後処理の最後に行う処理（[`set_after`]）。
pub(crate) type AfterCommit = Box<dyn FnOnce(&DocResult, bool)>;

thread_local! {
    static PENDING: RefCell<BTreeMap<u64, PendingCommit>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
    /// 処理中のキーの入口のセッションの状態（[`KeySession`]）。
    static KEY_SESSION: RefCell<Option<crate::engine::state::SessionState>> = const { RefCell::new(None) };
}

/// キーの処理の間、入口のセッションの状態を控える（確定しなかったときに戻すため）。
/// 処理を抜けたら（Drop）前の値に戻す（再生の中の入れ子にも対応する）。
pub(crate) struct KeySession(Option<crate::engine::state::SessionState>);

impl KeySession {
    pub(crate) fn enter() -> Self {
        let now = crate::engine::state::session_get().ok().map(|s| s.clone());
        Self(KEY_SESSION.with(|k| k.replace(now)))
    }
}

impl Drop for KeySession {
    fn drop(&mut self) {
        let prev = self.0.take();
        KEY_SESSION.with(|k| *k.borrow_mut() = prev);
    }
}

/// 確定を計画し、EditSession に渡す状態の箱を返す。
pub(crate) fn begin(planned: String, effects: CommitEffects) -> ReqCell {
    let cell: ReqCell = Rc::new(RefCell::new(ReqState::Pending));
    let id = NEXT_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    PENDING.with(|p| {
        p.borrow_mut().insert(
            id,
            PendingCommit {
                cell: cell.clone(),
                planned,
                effects,
                at: Instant::now(),
                leaves_input: false,
                replacement: None,
                restore: KEY_SESSION.with(|k| k.borrow().clone()),
                target: crate::engine::state::composition_key_and_ref(),
                after: None,
            },
        )
    });
    cell
}

/// 要求そのものが失敗した（EditSession は実行されない）。
pub(crate) fn mark_not_run(cell: &ReqCell, why: &'static str) {
    let mut state = cell.borrow_mut();
    if *state == ReqState::Pending {
        *state = ReqState::Done(DocResult::NotRun(why));
    }
}

/// 部分確定として記録する（確定の後に入力が残る。失効させるときに置き換え待ちの印を消す）。
pub(crate) fn mark_partial(cell: &ReqCell, replacement: Option<u64>) {
    PENDING.with(|p| {
        if let Some(commit) = p
            .borrow_mut()
            .values_mut()
            .find(|c| Rc::ptr_eq(&c.cell, cell))
        {
            commit.leaves_input = true;
            commit.replacement = replacement;
        }
    });
}

/// 後処理の最後に行う処理を持たせる（部分確定。結果が出た後、後処理を送った後に 1 回だけ
/// 呼ぶ。保留したキーの再生はその後）。
pub(crate) fn set_after(cell: &ReqCell, after: AfterCommit) {
    PENDING.with(|p| {
        if let Some(commit) = p
            .borrow_mut()
            .values_mut()
            .find(|c| Rc::ptr_eq(&c.cell, cell))
        {
            commit.after = Some(after);
        }
    });
}

/// 結果を待っている最後の確定が、確定の後に入力を残すか（部分確定か）。結果を待って
/// いる確定が無ければ `None`。
pub(crate) fn pending_leaves_input() -> Option<bool> {
    PENDING.with(|p| {
        p.borrow()
            .values()
            .rev()
            .find(|c| !matches!(*c.cell.borrow(), ReqState::Done(_)))
            .map(|c| c.leaves_input)
    })
}

/// 結果を待っている確定がある（後で実行される EditSession が、まだ終わっていない）。
pub(crate) fn commit_pending() -> bool {
    PENDING.with(|p| {
        p.borrow()
            .values()
            .any(|c| !matches!(*c.cell.borrow(), ReqState::Done(_)))
    })
}

/// `timeout` を過ぎてもまだ実行されていない確定の要求を失効させる（実行されなかった
/// ものとして扱う）。失効した要求の EditSession が後で実行されても、文書には触れずに
/// 戻る（`Pending` でないため）。実行中（`Running`）のものは失効させない。
pub(crate) fn expire_overdue(now: Instant, timeout: Duration) {
    PENDING.with(|p| {
        for (id, commit) in p.borrow().iter() {
            let mut state = commit.cell.borrow_mut();
            if *state == ReqState::Pending && now.duration_since(commit.at) >= timeout {
                tracing::warn!(
                    "commit {id}: edit session not run within {timeout:?}; expired planned={:?}",
                    commit.planned
                );
                *state = ReqState::Done(DocResult::NotRun("timeout"));
                // 置き換えはもう行わないので、終了通知の片づけを止める印を消す
                crate::engine::state::clear_pending_replacement(commit.replacement);
            }
        }
    });
}

/// 結果が出た確定の後処理を、要求した順に行う。結果の無い要求があれば、そこで止める
/// （後の要求の後処理が先の要求を追い越さない）。
pub(crate) fn finish_completed() {
    let done: Vec<(u64, PendingCommit, DocResult)> = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let mut done = Vec::new();
        while let Some((&id, commit)) = p.iter().next() {
            let ReqState::Done(result) = commit.cell.borrow().clone() else {
                break;
            };
            let commit = p.remove(&id).expect("present");
            done.push((id, commit, result));
        }
        done
    });
    for (id, commit, result) in done {
        finish_one(id, commit, &result);
    }
}

fn finish_one(id: u64, mut commit: PendingCommit, result: &DocResult) {
    if !matches!(result, DocResult::Committed { .. }) {
        tracing::warn!(
            "commit {id}: document result {result:?}; learning and context skipped planned={:?}",
            commit.planned
        );
    }
    // 確定しなかった: ホストの読みを残し、セッションの状態を入口の値に戻す。部分確定も、
    // ホストへの引き継ぎは文書の確定部分を確定した後なので戻せる（5c-3）
    let keep = kept_composition(
        result,
        commit.target.as_ref().map(|(key, _)| *key),
        crate::engine::state::composition_key(),
    ) && !commit.effects.close_on_abort;
    if keep {
        restore_session(id, commit.restore.as_ref());
    }
    let plan = plan_for_result(&commit.effects, &commit.planned, result, keep);
    send_effects(id, &plan);
    if let Some(after) = commit.after.take() {
        after(result, keep);
    }
}

/// 確定しなかったので、確定の対象だった composition が残っている結果か。
/// `target` は要求した時点の composition、`current` は今の composition（`composition_key`）。
/// 一致するときだけ残っている扱い。composition が無かった（直接の挿入）・別の文書や別の
/// composition に替わった・確かめられないときは、残っていない扱い（今までどおり閉じる）。
fn kept_composition(
    result: &DocResult,
    target: Option<(usize, usize)>,
    current: Option<(usize, usize)>,
) -> bool {
    matches!(result, DocResult::Aborted { .. } | DocResult::NotRun(_))
        && target.is_some()
        && target == current
}

/// セッションの状態を戻す。ロックが取れなければ戻さない（WARN）。
fn restore_session(id: u64, snapshot: Option<&crate::engine::state::SessionState>) {
    let Some(snapshot) = snapshot else {
        tracing::warn!("commit {id}: not committed; no session state to restore");
        return;
    };
    match crate::engine::state::session_try_get() {
        Ok(mut sess) => {
            sess.restore_from(snapshot.clone());
            tracing::info!("commit {id}: not committed; session state restored");
        }
        Err(_) => tracing::warn!("commit {id}: not committed; session busy, state not restored"),
    }
}

/// 文書の結果に対して送る後処理を決める。確定した文字列が分かるときだけ学習と文脈を
/// 送る。BG の回収・編集状態を閉じる処理・新しい読みの BG 変換の開始は、結果によらず
/// 送る（composition は手放しているので、ホストの読みを残すと次の入力が古い読みに
/// 続いてしまう。BG の開始は今までどおり）。
/// `keep` は、composition とホストの読みを残す（全体の確定で確定しなかった）とき。
fn plan_for_result(
    effects: &CommitEffects,
    planned: &str,
    result: &DocResult,
    keep: bool,
) -> EffectPlan {
    match result {
        DocResult::Committed { text } => plan_effects(effects, planned, text),
        // 確定しなかった: composition もホストの読みも残すので、何も送らない
        _ if keep => EffectPlan {
            bg_reclaim: false,
            learn: None,
            context: None,
            reset_preedit: false,
            bg_start: None,
        },
        _ => EffectPlan {
            bg_reclaim: effects.bg_reclaim,
            learn: None,
            context: None,
            reset_preedit: effects.reset_preedit,
            bg_start: effects.bg_start.clone(),
        },
    }
}

fn send_effects(id: u64, plan: &EffectPlan) {
    if !plan.bg_reclaim
        && plan.learn.is_none()
        && plan.context.is_none()
        && !plan.reset_preedit
        && plan.bg_start.is_none()
    {
        return;
    }
    deliver(id, plan);
}

/// 後処理を送る先（試験では偽物）。
trait EffectTarget {
    fn bg_reclaim(&mut self);
    fn learn(&mut self, reading: &str, surface: &str);
    fn learn_force(&mut self, reading: &str, surface: &str);
    fn commit(&mut self, text: &str);
    fn reset_preedit(&mut self);
    /// 新しい読みの BG 変換を始める。始めたかを返す。
    fn start_live_bg(&mut self, reading: &str) -> bool;
}

impl EffectTarget for crate::engine::state::DynEngine {
    fn bg_reclaim(&mut self) {
        crate::engine::state::DynEngine::bg_reclaim(self);
    }
    fn learn(&mut self, reading: &str, surface: &str) {
        crate::engine::state::DynEngine::learn(self, reading, surface);
    }
    fn learn_force(&mut self, reading: &str, surface: &str) {
        crate::engine::state::DynEngine::learn_force(self, reading, surface);
    }
    fn commit(&mut self, text: &str) {
        crate::engine::state::DynEngine::commit(self, text);
    }
    fn reset_preedit(&mut self) {
        crate::engine::state::DynEngine::reset_preedit(self);
    }
    fn start_live_bg(&mut self, reading: &str) -> bool {
        crate::engine::state::start_live_bg_if_ready(self, reading)
    }
}

/// 後処理を決まった順に送る。BG 変換の開始は学習の後（BG の要求が結果不明で台帳に
/// 残ると、後の学習の要求が断られるため）。BG 変換を始めたかを返す。
fn apply_plan(target: &mut impl EffectTarget, plan: &EffectPlan) -> bool {
    if plan.bg_reclaim {
        target.bg_reclaim();
    }
    if let Some(learn) = &plan.learn {
        match learn.action {
            LearnAction::Learn => target.learn(&learn.reading, &learn.surface),
            LearnAction::LearnForce => target.learn_force(&learn.reading, &learn.surface),
            LearnAction::Skip => {}
        }
    }
    if let Some(context) = &plan.context {
        target.commit(context);
    }
    if plan.reset_preedit {
        target.reset_preedit();
    }
    plan.bg_start
        .as_deref()
        .is_some_and(|reading| target.start_live_bg(reading))
}

// 試験では、送る後処理をエンジンへ送らずに記録する
#[cfg(test)]
thread_local! {
    static DELIVERED: RefCell<Vec<(u64, EffectPlan)>> = const { RefCell::new(Vec::new()) };
}

/// 試験では記録して `true` を返す（エンジンへは送らない）。
#[cfg(test)]
fn delivered_in_test(id: u64, plan: &EffectPlan) -> bool {
    DELIVERED.with(|d| d.borrow_mut().push((id, plan.clone())));
    true
}

#[cfg(not(test))]
fn delivered_in_test(_id: u64, _plan: &EffectPlan) -> bool {
    false
}

fn deliver(id: u64, plan: &EffectPlan) {
    if delivered_in_test(id, plan) {
        return;
    }
    let Ok(mut guard) = crate::engine::state::engine_get() else {
        tracing::warn!("commit {id}: engine unavailable; host effects not sent");
        return;
    };
    let Some(engine) = guard.as_mut() else {
        return;
    };
    apply_plan(engine, plan);
}

/// 送る後処理（試験のため、エンジンを使わない部分を切り出す）。
#[derive(Debug, Clone, PartialEq)]
struct EffectPlan {
    bg_reclaim: bool,
    learn: Option<LearnPlan>,
    context: Option<String>,
    reset_preedit: bool,
    bg_start: Option<String>,
}

/// 確定した文字列 `committed` に対して送る後処理を決める。学習は、確定した文字列が
/// 計画の文字列と同じときだけ送る（文書が計画と違う文字列で確定した場合は学習しない）。
fn plan_effects(effects: &CommitEffects, planned: &str, committed: &str) -> EffectPlan {
    let learn = effects.learn.clone().filter(|_| committed == planned);
    if effects.learn.is_some() && learn.is_none() {
        tracing::info!(
            "learning_decision action=Skip reason=committed_text_differs planned={planned:?} committed={committed:?}"
        );
    }
    EffectPlan {
        bg_reclaim: effects.bg_reclaim,
        learn,
        context: effects.context.then(|| committed.to_string()),
        reset_preedit: effects.reset_preedit,
        bg_start: effects.bg_start.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn effects() -> CommitEffects {
        CommitEffects {
            bg_reclaim: true,
            learn: Some(LearnPlan {
                reading: "かんじ".into(),
                surface: "漢字".into(),
                action: LearnAction::Learn,
            }),
            context: true,
            reset_preedit: true,
            bg_start: None,
            close_on_abort: false,
        }
    }

    #[test]
    fn committed_as_planned_sends_learning_and_the_committed_text_as_context() {
        let plan = plan_effects(&effects(), "漢字", "漢字");
        assert!(plan.learn.is_some());
        assert_eq!(plan.context.as_deref(), Some("漢字"));
        assert!(plan.bg_reclaim && plan.reset_preedit);
    }

    #[test]
    fn committed_text_that_differs_from_the_plan_is_not_learned() {
        // 文書に計画と違う文字列が確定した: 学習しない。文脈は実際に確定した文字列
        let plan = plan_effects(&effects(), "漢字", "かんじ");
        assert!(plan.learn.is_none());
        assert_eq!(plan.context.as_deref(), Some("かんじ"));
    }

    fn remaining() -> Vec<String> {
        PENDING.with(|p| p.borrow().values().map(|c| c.planned.clone()).collect())
    }

    #[test]
    fn a_later_result_does_not_overtake_an_earlier_pending_commit() {
        // 後処理はエンジンを使うので、ここでは表の出し入れだけを確かめる
        let a = begin("a".into(), CommitEffects::default());
        let b = begin("b".into(), CommitEffects::default());
        *b.borrow_mut() = ReqState::Done(DocResult::Failed("test"));
        finish_completed();
        // a が結果待ちなので、b も処理しない
        assert_eq!(remaining(), vec!["a".to_string(), "b".to_string()]);
        mark_not_run(&a, "request failed");
        finish_completed();
        assert!(remaining().is_empty());
    }

    #[test]
    fn a_deferred_commit_waits_for_its_result_and_expires_after_the_timeout() {
        // 後で実行される要求は、結果が出るまで後処理を送らず、結果待ちとして残る
        delivered_learns();
        let a = begin("漢字".into(), learn_only());
        assert!(commit_pending());
        finish_completed();
        assert!(delivered_learns().is_empty());
        // 時間内なら失効しない
        let now = Instant::now();
        expire_overdue(now, COMMIT_TIMEOUT);
        assert_eq!(*a.borrow(), ReqState::Pending);
        // 時間を過ぎたら、実行されなかったものとして後処理する（学習は送らない）
        expire_overdue(now + COMMIT_TIMEOUT, COMMIT_TIMEOUT);
        assert_eq!(*a.borrow(), ReqState::Done(DocResult::NotRun("timeout")));
        assert!(!commit_pending());
        finish_completed();
        assert!(delivered_learns().is_empty());
        assert!(remaining().is_empty());
        // 実行中の要求は失効させない
        let b = begin("b".into(), CommitEffects::default());
        *b.borrow_mut() = ReqState::Running;
        expire_overdue(now + COMMIT_TIMEOUT * 10, COMMIT_TIMEOUT);
        assert_eq!(*b.borrow(), ReqState::Running);
        assert!(commit_pending());
        *b.borrow_mut() = ReqState::Done(DocResult::Failed("test"));
        finish_completed();
    }

    #[test]
    fn the_after_step_runs_once_in_request_order_with_the_result() {
        // 部分確定の後処理（5c-3）: 結果が出た後、要求の順に 1 回だけ呼ぶ。後の要求の結果が
        // 先に出ても、前の要求の結果を待つ
        let seen: Rc<RefCell<Vec<(String, DocResult, bool)>>> = Default::default();
        let after = |name: &'static str| -> AfterCommit {
            let seen = seen.clone();
            Box::new(move |result, keep| {
                seen.borrow_mut()
                    .push((name.to_string(), result.clone(), keep))
            })
        };
        delivered_learns();
        let a = begin("a".into(), CommitEffects::default());
        mark_partial(&a, None);
        set_after(&a, after("a"));
        let b = begin("b".into(), CommitEffects::default());
        mark_partial(&b, None);
        set_after(&b, after("b"));
        *b.borrow_mut() = ReqState::Done(DocResult::Committed { text: "b".into() });
        finish_completed();
        assert!(seen.borrow().is_empty());
        *a.borrow_mut() = ReqState::Done(DocResult::Aborted {
            why: "EndComposition",
            changed: false,
        });
        finish_completed();
        finish_completed();
        assert_eq!(
            *seen.borrow(),
            vec![
                // 要求の時点の composition が無い（試験）ので、残す扱いにはならない
                (
                    "a".to_string(),
                    DocResult::Aborted {
                        why: "EndComposition",
                        changed: false
                    },
                    false
                ),
                (
                    "b".to_string(),
                    DocResult::Committed { text: "b".into() },
                    false
                ),
            ]
        );
        // 部分確定の後処理は after が行い、commit_txn からは何も送らない
        assert!(delivered_learns().is_empty());
        assert!(remaining().is_empty());
    }

    #[test]
    fn the_last_pending_commit_tells_whether_input_remains() {
        assert_eq!(pending_leaves_input(), None);
        let full = begin("全体".into(), CommitEffects::default());
        assert_eq!(pending_leaves_input(), Some(false));
        let partial = begin("部分".into(), CommitEffects::default());
        mark_partial(&partial, None);
        assert_eq!(pending_leaves_input(), Some(true));
        // 結果の出た要求は数えない
        *partial.borrow_mut() = ReqState::Done(DocResult::Failed("test"));
        assert_eq!(pending_leaves_input(), Some(false));
        mark_not_run(&full, "request failed");
        assert_eq!(pending_leaves_input(), None);
        finish_completed();
        assert!(remaining().is_empty());
    }

    #[test]
    fn a_failed_or_unknown_commit_closes_the_host_state_without_learning_or_context() {
        for (result, keep) in [
            (DocResult::Failed("EndComposition"), false),
            (DocResult::CommittedUnknown, false),
            // 部分確定の確定しなかった結果は、今までどおり閉じる
            (DocResult::NotRun("request failed"), false),
        ] {
            let plan = plan_for_result(&effects(), "漢字", &result, keep);
            assert_eq!(
                plan,
                EffectPlan {
                    bg_reclaim: true,
                    learn: None,
                    context: None,
                    reset_preedit: true,
                    bg_start: None,
                },
                "{result:?}"
            );
        }
    }

    #[test]
    fn a_full_commit_that_did_not_happen_keeps_the_host_state() {
        // 全体の確定で確定しなかった（composition を保持し直した・要求が実行されなかった）:
        // 何も送らず、ホストの読みを残す
        for result in [
            DocResult::Aborted {
                why: "EndComposition",
                changed: false,
            },
            DocResult::NotRun("timeout"),
        ] {
            assert!(kept_composition(&result, Some((1, 10)), Some((1, 10))));
            let plan = plan_for_result(&effects(), "漢字", &result, true);
            assert_eq!(
                plan,
                EffectPlan {
                    bg_reclaim: false,
                    learn: None,
                    context: None,
                    reset_preedit: false,
                    bg_start: None,
                },
                "{result:?}"
            );
        }
        let key = Some((1, 10));
        assert!(!kept_composition(
            &DocResult::Failed("direct insert"),
            key,
            key
        ));
        let not_run = DocResult::NotRun("timeout");
        // composition が無かった（直接の挿入）: 閉じる
        assert!(!kept_composition(&not_run, None, None));
        assert!(!kept_composition(&not_run, None, key));
        // 今は無い・確かめられない: 閉じる
        assert!(!kept_composition(&not_run, key, None));
        // 別の composition・別の文書に替わった: 閉じる
        assert!(!kept_composition(&not_run, key, Some((1, 11))));
        assert!(!kept_composition(&not_run, key, Some((2, 10))));
        let aborted = DocResult::Aborted {
            why: "GetRange",
            changed: false,
        };
        assert!(!kept_composition(&aborted, key, Some((1, 11))));
    }

    fn learn_only() -> CommitEffects {
        CommitEffects {
            learn: effects().learn,
            ..Default::default()
        }
    }

    fn delivered_learns() -> Vec<Option<String>> {
        DELIVERED.with(|d| {
            d.borrow_mut()
                .drain(..)
                .map(|(_, plan)| plan.learn.map(|l| l.surface))
                .collect()
        })
    }

    #[test]
    fn learning_is_sent_once_after_the_document_result() {
        delivered_learns();
        // 同期で確定できた: 結果の後で 1 回
        let a = begin("漢字".into(), learn_only());
        assert!(delivered_learns().is_empty());
        *a.borrow_mut() = ReqState::Done(DocResult::Committed {
            text: "漢字".into(),
        });
        finish_completed();
        finish_completed();
        assert_eq!(delivered_learns(), vec![Some("漢字".to_string())]);
        // 確定に失敗した・要求が拒否された・確定した文字列が分からない: 送らない
        for result in [
            DocResult::Failed("EndComposition"),
            DocResult::CommittedUnknown,
        ] {
            let c = begin("漢字".into(), learn_only());
            *c.borrow_mut() = ReqState::Done(result);
            finish_completed();
        }
        let c = begin("漢字".into(), learn_only());
        mark_not_run(&c, "edit session refused");
        finish_completed();
        assert!(delivered_learns().is_empty());
        // 後で実行される: 結果が出るまで送らず、結果の後で 1 回
        let d = begin("漢字".into(), learn_only());
        finish_completed();
        assert!(delivered_learns().is_empty());
        *d.borrow_mut() = ReqState::Done(DocResult::Committed {
            text: "漢字".into(),
        });
        finish_completed();
        finish_completed();
        assert_eq!(delivered_learns(), vec![Some("漢字".to_string())]);
        assert!(remaining().is_empty());
    }

    /// 送った順を記録する偽のエンジン。
    #[derive(Default)]
    struct FakeTarget {
        calls: Vec<String>,
    }

    impl EffectTarget for FakeTarget {
        fn bg_reclaim(&mut self) {
            self.calls.push("bg_reclaim".into());
        }
        fn learn(&mut self, _: &str, surface: &str) {
            self.calls.push(format!("learn {surface}"));
        }
        fn learn_force(&mut self, _: &str, surface: &str) {
            self.calls.push(format!("learn_force {surface}"));
        }
        fn commit(&mut self, text: &str) {
            self.calls.push(format!("commit {text}"));
        }
        fn reset_preedit(&mut self) {
            self.calls.push("reset_preedit".into());
        }
        fn start_live_bg(&mut self, reading: &str) -> bool {
            self.calls.push(format!("bg_start {reading}"));
            true
        }
    }

    #[test]
    fn the_bg_start_for_the_remainder_is_sent_after_learning() {
        // 部分確定: 学習と、残りの読みの BG 変換の開始だけを後に送る
        let e = CommitEffects {
            learn: effects().learn,
            bg_start: Some("のこり".into()),
            ..Default::default()
        };
        let mut target = FakeTarget::default();
        let plan = plan_for_result(
            &e,
            "漢字",
            &DocResult::Committed {
                text: "漢字".into(),
            },
            false,
        );
        assert!(apply_plan(&mut target, &plan));
        assert_eq!(target.calls, vec!["learn 漢字", "bg_start のこり"]);
        // 確定に失敗しても、BG 変換は今までどおり始める（学習は送らない）
        let mut target = FakeTarget::default();
        let plan = plan_for_result(&e, "漢字", &DocResult::Failed("EndComposition"), false);
        assert!(apply_plan(&mut target, &plan));
        assert_eq!(target.calls, vec!["bg_start のこり"]);
        // 全体の確定: 学習 → 文脈 → 編集状態を閉じる
        let mut target = FakeTarget::default();
        let plan = plan_for_result(
            &effects(),
            "漢字",
            &DocResult::Committed {
                text: "漢字".into(),
            },
            false,
        );
        assert!(!apply_plan(&mut target, &plan));
        assert_eq!(
            target.calls,
            vec!["bg_reclaim", "learn 漢字", "commit 漢字", "reset_preedit"]
        );
    }

    #[test]
    fn the_bg_start_is_sent_once_through_the_transaction() {
        delivered_learns();
        let partial = || CommitEffects {
            learn: effects().learn,
            bg_start: Some("のこり".into()),
            ..Default::default()
        };
        let bg_starts = || -> Vec<(Option<String>, Option<String>)> {
            DELIVERED.with(|d| {
                d.borrow_mut()
                    .drain(..)
                    .map(|(_, plan)| (plan.learn.map(|l| l.surface), plan.bg_start))
                    .collect()
            })
        };
        let once =
            |learn: Option<&str>| vec![(learn.map(String::from), Some("のこり".to_string()))];
        // 同期で確定できた: 学習と BG の開始を 1 回
        let a = begin("漢字".into(), partial());
        *a.borrow_mut() = ReqState::Done(DocResult::Committed {
            text: "漢字".into(),
        });
        finish_completed();
        finish_completed();
        assert_eq!(bg_starts(), once(Some("漢字")));
        // 確定に失敗した: BG の開始だけを 1 回
        let b = begin("漢字".into(), partial());
        *b.borrow_mut() = ReqState::Done(DocResult::Failed("EndComposition"));
        finish_completed();
        assert_eq!(bg_starts(), once(None));
        // 後で実行される: 結果の後で 1 回
        let c = begin("漢字".into(), partial());
        finish_completed();
        assert!(bg_starts().is_empty());
        *c.borrow_mut() = ReqState::Done(DocResult::Committed {
            text: "漢字".into(),
        });
        finish_completed();
        assert_eq!(bg_starts(), once(Some("漢字")));
    }

    #[test]
    fn whole_text_with_punctuation_still_learns_the_candidate() {
        // 計画の文字列は句読点を含む全体、学習の surface は候補だけ。全体が計画どおりに
        // 確定すれば、候補を学習する
        let mut e = effects();
        e.learn.as_mut().unwrap().surface = "漢字".into();
        let plan = plan_effects(&e, "漢字。", "漢字。");
        assert_eq!(plan.learn.map(|l| l.surface), Some("漢字".into()));
    }
}
