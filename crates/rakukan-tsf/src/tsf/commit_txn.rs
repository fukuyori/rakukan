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
/// 送る。BG の回収と編集状態を閉じる処理は、結果によらず送る（composition は
/// 手放しているので、ホストの読みを残すと次の入力が古い読みに続いてしまう）。
fn plan_for_result(effects: &CommitEffects, planned: &str, result: &DocResult) -> EffectPlan {
    match result {
        DocResult::Committed { text } => plan_effects(effects, planned, text),
        _ => EffectPlan {
            bg_reclaim: effects.bg_reclaim,
            learn: None,
            context: None,
            reset_preedit: effects.reset_preedit,
        },
    }
}

fn send_effects(id: u64, plan: &EffectPlan) {
    if !plan.bg_reclaim && plan.learn.is_none() && plan.context.is_none() && !plan.reset_preedit {
        return;
    }
    let Ok(mut guard) = crate::engine::state::engine_get() else {
        tracing::warn!("commit {id}: engine unavailable; host effects not sent");
        return;
    };
    let Some(engine) = guard.as_mut() else {
        return;
    };
    if plan.bg_reclaim {
        engine.bg_reclaim();
    }
    if let Some(learn) = &plan.learn {
        match learn.action {
            LearnAction::Learn => engine.learn(&learn.reading, &learn.surface),
            LearnAction::LearnForce => engine.learn_force(&learn.reading, &learn.surface),
            LearnAction::Skip => {}
        }
    }
    if let Some(context) = &plan.context {
        engine.commit(context);
    }
    if plan.reset_preedit {
        engine.reset_preedit();
    }
}

/// 送る後処理（試験のため、エンジンを使わない部分を切り出す）。
#[derive(Debug, PartialEq)]
struct EffectPlan {
    bg_reclaim: bool,
    learn: Option<LearnPlan>,
    context: Option<String>,
    reset_preedit: bool,
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
                },
                "{result:?}"
            );
        }
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
