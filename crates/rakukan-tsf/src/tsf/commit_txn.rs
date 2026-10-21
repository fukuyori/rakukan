//! 確定トランザクション（入力処理の見直しの段 5、docs/Input_Pipeline_Plan.md 4.6）。
//!
//! 文書を先に確定し、ホストへの後処理（BG の回収・学習・文脈・編集状態を閉じる）は、
//! 文書の結果を見てから送る。EditSession の中では RPC を送らない。
//!
//! EditSession は同期で実行されることも、後で実行されることもある。どちらの場合も
//! EditSession の最後で結果を [`ReqState::Done`] に記録し、`WM_APP_COMMIT_DONE` を送る。
//! 要求した側も、要求の直後に [`finish_completed`] を呼ぶ（同期で実行されていれば
//! そこで後処理する）。後処理は 1 回だけ行う。
//!
//! 確定に失敗した（文書が確定されたか分からない）ときの文書の扱いは今までと同じ
//! （composition は手放す）。ホストには BG の回収と編集状態を閉じる処理だけを送り、
//! 学習と文脈は送らない。composition を保持し直してホストの読みを残す扱いは、
//! セッションの状態（候補・選択・句読点の保留）を戻す仕組みと一緒に後の段で入れる。
//!
//! EditSession が後で実行される（要求の直後に結果が無い）場合は、後処理をその場で
//! 先に送る（[`send_early_if_pending`]。今までと同じ順）。確定を待つ間に打ったキーを
//! 保留する仕組み（段 5 の後の部分）が無い間に、遅れた後処理が後続のキーの編集状態を
//! 閉じないため。この場合は、後で文書の確定に失敗しても取り消せない（ログに残す）。

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::engine::state::LearnAction;

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
    /// 実行されなかった（要求の失敗・拒否）。
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
    /// 結果を待たずに後処理を送った（EditSession が後で実行される場合）。
    sent_early: bool,
}

thread_local! {
    static PENDING: RefCell<BTreeMap<u64, PendingCommit>> = const { RefCell::new(BTreeMap::new()) };
    static NEXT_ID: Cell<u64> = const { Cell::new(1) };
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
                sent_early: false,
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

/// EditSession が要求の直後に実行されなかった（後で実行される）場合に呼ぶ。計画の文字列で
/// 確定したものとして、後処理をその場で送る。
pub(crate) fn send_early_if_pending(cell: &ReqCell) {
    if *cell.borrow() != ReqState::Pending {
        return;
    }
    let early = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let (id, commit) = p.iter_mut().find(|(_, c)| Rc::ptr_eq(&c.cell, cell))?;
        commit.sent_early = true;
        Some((
            *id,
            plan_effects(&commit.effects, &commit.planned, &commit.planned),
        ))
    });
    if let Some((id, plan)) = early {
        tracing::debug!("commit {id}: edit session deferred; host effects sent before the result");
        send_effects(id, &plan);
    }
}

/// 結果が出た確定の後処理を、要求した順に行う。結果の無い要求があれば、そこで止める
/// （後の要求の後処理が先の要求を追い越さない）。後処理を送り済みの要求は、結果が
/// 無くても後の要求を止めない（後処理の順はすでに守られている）。
pub(crate) fn finish_completed() {
    let done: Vec<(u64, PendingCommit, DocResult)> = PENDING.with(|p| {
        let mut p = p.borrow_mut();
        let mut ids = Vec::new();
        for (&id, commit) in p.iter() {
            if matches!(*commit.cell.borrow(), ReqState::Done(_)) {
                ids.push(id);
            } else if !commit.sent_early {
                break;
            }
        }
        ids.into_iter()
            .filter_map(|id| {
                let commit = p.remove(&id)?;
                let ReqState::Done(result) = commit.cell.borrow().clone() else {
                    return None;
                };
                Some((id, commit, result))
            })
            .collect()
    });
    for (id, commit, result) in done {
        finish_one(id, &commit, &result);
    }
}

fn finish_one(id: u64, commit: &PendingCommit, result: &DocResult) {
    if commit.sent_early {
        // 後処理は送り済み。計画どおりに確定できなかった場合は、取り消せないので残すだけ
        if *result
            != (DocResult::Committed {
                text: commit.planned.clone(),
            })
        {
            tracing::warn!(
                "commit {id}: effects were sent before the result, but the document result was {result:?} planned={:?}",
                commit.planned
            );
        }
        return;
    }
    if !matches!(result, DocResult::Committed { .. }) {
        tracing::warn!(
            "commit {id}: document result {result:?}; learning and context skipped planned={:?}",
            commit.planned
        );
    }
    let plan = plan_for_result(&commit.effects, &commit.planned, result);
    send_effects(id, &plan);
}

/// 文書の結果に対して送る後処理を決める。確定した文字列が分かるときだけ学習と文脈を
/// 送る。BG の回収・編集状態を閉じる処理・新しい読みの BG 変換の開始は、結果によらず
/// 送る（composition は手放しているので、ホストの読みを残すと次の入力が古い読みに
/// 続いてしまう。BG の開始は今までどおり）。
fn plan_for_result(effects: &CommitEffects, planned: &str, result: &DocResult) -> EffectPlan {
    match result {
        DocResult::Committed { text } => plan_effects(effects, planned, text),
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

thread_local! {
    /// 直前に送った後処理で BG 変換を始めたか（`take_bg_started`）。
    static BG_STARTED: Cell<bool> = const { Cell::new(false) };
}

/// 直前の後処理で BG 変換を始めたかを返し、印を消す（呼び出し側がライブ変換の
/// タイマーを始めるかの判定に使う）。
pub(crate) fn take_bg_started() -> bool {
    BG_STARTED.with(|b| b.replace(false))
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
    let started = apply_plan(engine, plan);
    BG_STARTED.with(|b| b.set(started));
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
    fn a_commit_whose_effects_were_sent_early_is_not_processed_again() {
        // 送り済みの印だけを確かめる（効果の送信はエンジンを使うため、ここでは
        // 後処理の無い CommitEffects::default() を使う）
        let a = begin("a".into(), CommitEffects::default());
        send_early_if_pending(&a);
        assert!(PENDING.with(|p| p.borrow().values().all(|c| c.sent_early)));
        // 結果が出ても、後処理は 2 回目を送らずに外れる
        *a.borrow_mut() = ReqState::Done(DocResult::Committed { text: "a".into() });
        finish_completed();
        assert!(remaining().is_empty());
        // 送り済みで結果待ちの要求は、後の要求を止めない
        let c = begin("c".into(), CommitEffects::default());
        send_early_if_pending(&c);
        let d = begin("d".into(), CommitEffects::default());
        *d.borrow_mut() = ReqState::Done(DocResult::Failed("test"));
        finish_completed();
        assert_eq!(remaining(), vec!["c".to_string()]);
        mark_not_run(&c, "request failed");
        finish_completed();
        assert!(remaining().is_empty());
        // 結果の出た要求は送り直さない
        let b = begin("b".into(), CommitEffects::default());
        *b.borrow_mut() = ReqState::Done(DocResult::Failed("test"));
        send_early_if_pending(&b);
        assert!(PENDING.with(|p| p.borrow().values().all(|c| !c.sent_early)));
        finish_completed();
    }

    #[test]
    fn a_failed_or_unknown_commit_closes_the_host_state_without_learning_or_context() {
        for result in [
            DocResult::Failed("EndComposition"),
            DocResult::CommittedUnknown,
            DocResult::NotRun("request failed"),
        ] {
            let plan = plan_for_result(&effects(), "漢字", &result);
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
        // 後で実行される: その場で 1 回送り、結果の後では送らない
        let d = begin("漢字".into(), learn_only());
        send_early_if_pending(&d);
        assert_eq!(delivered_learns(), vec![Some("漢字".to_string())]);
        *d.borrow_mut() = ReqState::Done(DocResult::Committed {
            text: "漢字".into(),
        });
        finish_completed();
        assert!(delivered_learns().is_empty());
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
        );
        assert!(apply_plan(&mut target, &plan));
        assert_eq!(target.calls, vec!["learn 漢字", "bg_start のこり"]);
        // 確定に失敗しても、BG 変換は今までどおり始める（学習は送らない）
        let mut target = FakeTarget::default();
        let plan = plan_for_result(&e, "漢字", &DocResult::Failed("EndComposition"));
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
        // 後で実行される: その場で 1 回送り、結果の後では送らない
        let c = begin("漢字".into(), partial());
        send_early_if_pending(&c);
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
