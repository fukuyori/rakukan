//! composition ごとの所有者と復元元（Issue #56）。`RpcEngine` の再接続をまたいで保持する。
//! タイマーからの読み取り・変更では所有権を取り返さない。復元は、フォーカス中の文脈へのキー入力からだけ行う。
use rakukan_engine_rpc::{
    BgStartOutcome, BgTakeOutcome, BgView, ChangeOutcome, ChangeReply, ChangeRequest, EditState,
    EngineGen, Expect, InputCharKind, Owner, ReadError, ReadRequest, Reason, Response,
    RestoreReply, RpcEngine,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

thread_local! { static FOREGROUND_INPUT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
pub struct InputScope(bool);
impl InputScope {
    pub fn enter(focused: bool) -> Self {
        Self(FOREGROUND_INPUT.with(|flag| flag.replace(focused)))
    }
}
impl Drop for InputScope {
    fn drop(&mut self) {
        FOREGROUND_INPUT.with(|flag| flag.set(self.0));
    }
}
static NEXT_COMPOSITION: AtomicU64 = AtomicU64::new(1);
static COMPOSITIONS: LazyLock<Mutex<HashMap<usize, RemoteComposition>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
struct RemoteComposition {
    owner: Owner,
    generation: Option<EngineGen>,
    edit: EditState,
    pending: Option<ChangeRequest>,
    bg_reading: Option<String>,
}
impl RemoteComposition {
    fn new() -> Self {
        Self {
            owner: Owner {
                tsf_id: rakukan_engine_rpc::tsf_id(),
                composition: NEXT_COMPOSITION.fetch_add(1, Ordering::Relaxed),
            },
            generation: None,
            edit: EditState::default(),
            pending: None,
            bg_reading: None,
        }
    }
    fn expect(&self) -> Option<Expect> {
        self.generation.map(|engine_gen| Expect {
            engine_gen,
            owner: self.owner,
        })
    }
}
fn dm_key() -> usize {
    crate::tsf::candidate_window::current_dm_hwnd()
        .0
        .map_or(0, |dm| dm.ptr)
}
fn lock_states() -> std::sync::MutexGuard<'static, HashMap<usize, RemoteComposition>> {
    COMPOSITIONS.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("composition snapshot mutex poisoned; preserving its contents");
        poisoned.into_inner()
    })
}
fn snapshot() -> Option<RemoteComposition> {
    lock_states().get(&dm_key()).cloned()
}
pub fn local_edit() -> Option<EditState> {
    snapshot().map(|c| c.edit)
}
pub fn close() {
    crate::tsf::candidate_window::reset_worker_busy_clock();
    lock_states().remove(&dm_key());
}
pub fn dispose(dm: usize) {
    lock_states().remove(&dm);
}
fn save(c: RemoteComposition) {
    lock_states().insert(dm_key(), c);
}
fn replayable(request: &ChangeRequest) -> bool {
    matches!(
        request,
        ChangeRequest::InputChar { .. }
            | ChangeRequest::PushChar(_)
            | ChangeRequest::PushRaw(_)
            | ChangeRequest::PushFullwidthAlpha(_)
            | ChangeRequest::Backspace
            | ChangeRequest::FlushPendingN
            | ChangeRequest::ForcePreedit { .. }
            | ChangeRequest::ResetPreedit
            | ChangeRequest::ResetAll
    )
}
pub fn note_read_error(error: &ReadError) {
    tracing::warn!("composition read failed: {error}");
    if error.ownership_lost() {
        crate::tsf::candidate_window::stop_waiting_timer();
        crate::tsf::candidate_window::stop_live_timer();
        crate::tsf::candidate_window::clear_model_wait();
    }
}
trait CompositionRpc {
    fn change(
        &self,
        expect: Expect,
        request: ChangeRequest,
    ) -> Result<ChangeReply, rakukan_engine_rpc::ChangeError>;
    fn restore(
        &self,
        owner: Owner,
        reading: String,
        pending: String,
        then: Option<ChangeRequest>,
    ) -> Result<RestoreReply, rakukan_engine_rpc::ChangeError>;
}
impl CompositionRpc for RpcEngine {
    fn change(
        &self,
        expect: Expect,
        request: ChangeRequest,
    ) -> Result<ChangeReply, rakukan_engine_rpc::ChangeError> {
        self.change(expect, request)
    }
    fn restore(
        &self,
        owner: Owner,
        reading: String,
        pending: String,
        then: Option<ChangeRequest>,
    ) -> Result<RestoreReply, rakukan_engine_rpc::ChangeError> {
        self.restore(owner, reading, pending, then)
    }
}
fn restore_via(
    rpc: &impl CompositionRpc,
    state: &mut RemoteComposition,
    then: Option<ChangeRequest>,
) -> anyhow::Result<Option<ChangeOutcome>> {
    match rpc.restore(
        state.owner,
        state.edit.reading.clone(),
        state.edit.pending_romaji.clone(),
        then,
    )? {
        RestoreReply::Restored { engine_gen, then } => {
            state.generation = Some(engine_gen);
            state.bg_reading = None;
            if let Some(outcome) = &then {
                state.edit = outcome.edit().clone();
            }
            state.pending = None;
            Ok(then)
        }
        RestoreReply::Rejected(reason) => anyhow::bail!("restore rejected: {reason:?}"),
    }
}
fn apply_via(
    rpc: &impl CompositionRpc,
    state: &mut RemoteComposition,
    request: ChangeRequest,
    foreground: bool,
) -> anyhow::Result<ChangeOutcome> {
    let Some(expect) = state.expect() else {
        if !foreground || !replayable(&request) {
            anyhow::bail!("timer or irreversible request cannot acquire ownership");
        }
        state.pending = Some(request.clone());
        return restore_via(rpc, state, Some(request))?
            .ok_or_else(|| anyhow::anyhow!("missing Restore.then"));
    };
    match rpc.change(expect, request.clone()) {
        Ok(ChangeReply::Changed(outcome)) => {
            state.edit = outcome.edit().clone();
            state.pending = None;
            Ok(outcome)
        }
        Ok(ChangeReply::Rejected(reason)) => {
            let error = ReadError::Rejected(reason);
            note_read_error(&error);
            if error.ownership_lost() && replayable(&request) && foreground {
                state.pending = Some(request.clone());
                return restore_via(rpc, state, Some(request))?
                    .ok_or_else(|| anyhow::anyhow!("missing Restore.then"));
            }
            if reason == Reason::ResultUnavailable {
                state.pending = Some(request);
            }
            Err(error.into())
        }
        Err(error) => {
            // 未解決の要求に阻まれた変更は送っていないので、再送用の要求を置き換えない。
            if matches!(error, rakukan_engine_rpc::ChangeError::Transport(_)) {
                state.pending = Some(request);
            }
            Err(error.into())
        }
    }
}
pub struct DynEngine(RpcEngine);
impl std::ops::Deref for DynEngine {
    type Target = RpcEngine;
    fn deref(&self) -> &RpcEngine {
        &self.0
    }
}
impl DynEngine {
    pub fn connect_or_spawn(config: Option<String>) -> anyhow::Result<Self> {
        Ok(Self(RpcEngine::connect_or_spawn(config)?))
    }
    pub fn shutdown(
        &self,
        config: Option<String>,
    ) -> anyhow::Result<rakukan_engine_rpc::ShutdownOutcome> {
        self.0.shutdown(config)
    }
    pub fn shutdown_if_config_differs(
        &self,
        config: Option<String>,
    ) -> anyhow::Result<rakukan_engine_rpc::ConditionalShutdown> {
        self.0.shutdown_if_config_differs(config)
    }
    pub fn convert_sync(&self) -> Result<Vec<String>, ReadError> {
        match self.read_local(ReadRequest::ConvertSync)? {
            Response::Strings(c) => Ok(c),
            other => Err(ReadError::Host(format!("unexpected conversion: {other:?}"))),
        }
    }
    fn restore_state(
        &self,
        state: &mut RemoteComposition,
        then: Option<ChangeRequest>,
    ) -> anyhow::Result<Option<ChangeOutcome>> {
        let result = restore_via(&self.0, state, then);
        save(state.clone());
        result
    }

    /// フォーカス中の有効な文脈へのキー入力からだけ呼ぶ。
    pub fn recover_on_input(&self) -> anyhow::Result<bool> {
        let Some(mut state) = snapshot() else {
            return Ok(false);
        };
        let lost = match state.expect() {
            Some(expect) => match self.0.bg_status(expect) {
                Ok(_) => false,
                Err(error) if error.ownership_lost() => true,
                Err(error) => {
                    note_read_error(&error);
                    return Err(error.into());
                }
            },
            None => true,
        };
        if !lost && self.0.unresolved().is_none() {
            return Ok(false);
        }
        let pending = state.pending.clone().filter(replayable);
        if self.0.unresolved().is_some() && pending.is_none() {
            self.0.abandon_unresolved();
        }
        self.restore_state(&mut state, pending)?;
        Ok(true)
    }
    fn change_local(&self, request: ChangeRequest) -> anyhow::Result<ChangeOutcome> {
        let mut state = match snapshot() {
            Some(state) => state,
            None if matches!(
                request,
                ChangeRequest::BgStart { .. }
                    | ChangeRequest::BgTakeCandidates { .. }
                    | ChangeRequest::BgReclaim
                    | ChangeRequest::Learn { .. }
                    | ChangeRequest::LearnForce { .. }
            ) =>
            {
                anyhow::bail!("no active composition")
            }
            None => RemoteComposition::new(),
        };
        let result = apply_via(
            &self.0,
            &mut state,
            request,
            FOREGROUND_INPUT.with(|flag| flag.get()),
        );
        save(state);
        if let Err(error) = &result {
            tracing::warn!("composition change failed: {error}");
        }
        result
    }

    fn read_local(&self, request: ReadRequest) -> Result<Response, ReadError> {
        let state = snapshot().ok_or(ReadError::NoComposition)?;
        let expect = state.expect().ok_or(ReadError::NoComposition)?;
        self.0.read(expect, request).inspect_err(note_read_error)
    }
    fn string(&self, request: ReadRequest) -> Result<String, ReadError> {
        // 最初の編集の前は、手元の状態が空と分かっている（RPC の失敗ではない）。
        if snapshot().is_none() {
            return Ok(String::new());
        }
        match self.read_local(request)? {
            Response::String(s) => Ok(s),
            other => Err(ReadError::Host(format!("unexpected read: {other:?}"))),
        }
    }
    pub fn preedit_display(&self) -> Result<String, ReadError> {
        self.string(ReadRequest::PreeditDisplay)
    }
    pub fn hiragana_text(&self) -> Result<String, ReadError> {
        self.string(ReadRequest::HiraganaText)
    }
    pub fn romaji_log_str(&self) -> Result<String, ReadError> {
        self.string(ReadRequest::RomajiLogStr)
    }
    pub fn hiragana_from_romaji_log(&self) -> Result<String, ReadError> {
        self.string(ReadRequest::HiraganaFromRomajiLog)
    }
    pub fn preedit_is_empty(&self) -> Result<bool, ReadError> {
        if snapshot().is_none() {
            return Ok(true);
        }
        match self.read_local(ReadRequest::PreeditIsEmpty)? {
            Response::Bool(b) => Ok(b),
            other => Err(ReadError::Host(format!("unexpected read: {other:?}"))),
        }
    }
    pub fn bg_status(&self) -> Result<BgView, ReadError> {
        if snapshot().is_none() {
            return Ok(BgView::Idle);
        }
        match self.read_local(ReadRequest::BgStatus)? {
            Response::Bg(b) => {
                if b != BgView::WorkerBusy {
                    crate::tsf::candidate_window::reset_worker_busy_clock();
                }
                Ok(b)
            }
            other => Err(ReadError::Host(format!("unexpected BG read: {other:?}"))),
        }
    }
    pub fn bg_peek_top_candidate(&self, key: &str) -> Result<Option<String>, ReadError> {
        match self.read_local(ReadRequest::BgPeekTopCandidate { key: key.into() })? {
            Response::OptionalString(s) => Ok(s),
            other => Err(ReadError::Host(format!("unexpected peek: {other:?}"))),
        }
    }
    pub fn bg_wait_ms(&self, timeout_ms: u64) -> Result<BgView, ReadError> {
        match self.read_local(ReadRequest::BgWaitMs { timeout_ms })? {
            Response::Bg(b) => {
                if b != BgView::WorkerBusy {
                    crate::tsf::candidate_window::reset_worker_busy_clock();
                }
                Ok(b)
            }
            other => Err(ReadError::Host(format!("unexpected wait: {other:?}"))),
        }
    }
    pub fn bg_start(&self, n: usize) -> anyhow::Result<BgStartOutcome> {
        match self.change_local(ChangeRequest::BgStart { n_cands: n as u32 })? {
            ChangeOutcome::BgStart { outcome, .. } => {
                if outcome == BgStartOutcome::Started
                    && let Some(mut state) = snapshot()
                {
                    state.bg_reading = Some(state.edit.reading.clone());
                    save(state);
                }
                Ok(outcome)
            }
            _ => anyhow::bail!("unexpected start outcome"),
        }
    }
    pub fn bg_reading_is_current(&self, reading: &str) -> bool {
        snapshot().is_some_and(|state| state.bg_reading.as_deref() == Some(reading))
    }
    pub fn bg_take_candidates(&self, key: &str) -> anyhow::Result<Option<Vec<String>>> {
        match self.change_local(ChangeRequest::BgTakeCandidates { key: key.into() })? {
            ChangeOutcome::BgTake {
                outcome: BgTakeOutcome::Taken(c),
                ..
            } => Ok(Some(c)),
            ChangeOutcome::BgTake {
                outcome: BgTakeOutcome::NotReady,
                ..
            } => Ok(None),
            ChangeOutcome::BgTake {
                outcome: BgTakeOutcome::NotYours,
                ..
            } => {
                crate::tsf::candidate_window::stop_waiting_timer();
                crate::tsf::candidate_window::stop_live_timer();
                anyhow::bail!("BG result belongs to a different composition")
            }
            _ => anyhow::bail!("unexpected take outcome"),
        }
    }
    pub fn input_char(
        &self,
        c: char,
        kind: InputCharKind,
        n: Option<usize>,
    ) -> anyhow::Result<(String, String, BgView)> {
        match self.change_local(ChangeRequest::InputChar {
            c: c as u32,
            kind,
            bg_start_n_cands: n.map(|n| n as u32),
        })? {
            ChangeOutcome::InputChar {
                preedit,
                hiragana,
                bg,
                ..
            } => {
                if bg != BgView::WorkerBusy {
                    crate::tsf::candidate_window::reset_worker_busy_clock();
                }
                Ok((preedit, hiragana, bg))
            }
            _ => anyhow::bail!("unexpected input outcome"),
        }
    }
    pub fn backspace(&self) -> anyhow::Result<bool> {
        match self.change_local(ChangeRequest::Backspace)? {
            ChangeOutcome::Bool { value, .. } => Ok(value),
            _ => anyhow::bail!("unexpected backspace outcome"),
        }
    }
    pub fn flush_pending_n(&self) -> anyhow::Result<bool> {
        match self.change_local(ChangeRequest::FlushPendingN)? {
            ChangeOutcome::Bool { value, .. } => Ok(value),
            _ => anyhow::bail!("unexpected flush outcome"),
        }
    }
    fn unit(&self, request: ChangeRequest) {
        if let Err(error) = self.change_local(request) {
            tracing::warn!("composition mutation failed: {error}");
        }
    }
    pub fn push_raw(&self, c: char) {
        self.unit(ChangeRequest::PushRaw(c as u32));
    }
    pub fn force_preedit(&self, text: String) {
        self.unit(ChangeRequest::ForcePreedit { text });
    }
    pub fn bg_reclaim(&self) {
        self.unit(ChangeRequest::BgReclaim);
    }
    pub fn commit(&self, text: &str) {
        self.unit(ChangeRequest::Commit { text: text.into() });
        // Commit は、結果不明になっても再送しない。
        self.0.abandon_unresolved();
    }
    pub fn reset_preedit(&self) {
        self.unit(ChangeRequest::ResetPreedit);
        self.0.abandon_unresolved();
        close();
    }
    pub fn reset_all(&self) {
        self.unit(ChangeRequest::ResetAll);
        self.0.abandon_unresolved();
        close();
    }
    pub fn learn(&self, reading: &str, surface: &str) {
        self.unit(ChangeRequest::Learn {
            reading: reading.into(),
            surface: surface.into(),
        });
        self.0.abandon_unresolved();
    }
    pub fn learn_force(&self, reading: &str, surface: &str) {
        self.unit(ChangeRequest::LearnForce {
            reading: reading.into(),
            surface: surface.into(),
        });
        self.0.abandon_unresolved();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    #[derive(Debug)]
    enum Sent {
        Restore(Owner, String, String, Option<ChangeRequest>),
        Change(Expect, ChangeRequest),
    }
    struct FakeRpc {
        sent: RefCell<Vec<Sent>>,
        changes: RefCell<VecDeque<Result<ChangeReply, rakukan_engine_rpc::ChangeError>>>,
        restores: RefCell<VecDeque<Result<RestoreReply, rakukan_engine_rpc::ChangeError>>>,
    }
    impl CompositionRpc for FakeRpc {
        fn change(
            &self,
            expect: Expect,
            request: ChangeRequest,
        ) -> Result<ChangeReply, rakukan_engine_rpc::ChangeError> {
            self.sent.borrow_mut().push(Sent::Change(expect, request));
            self.changes
                .borrow_mut()
                .pop_front()
                .expect("change script")
        }
        fn restore(
            &self,
            owner: Owner,
            reading: String,
            pending: String,
            then: Option<ChangeRequest>,
        ) -> Result<RestoreReply, rakukan_engine_rpc::ChangeError> {
            self.sent
                .borrow_mut()
                .push(Sent::Restore(owner, reading, pending, then));
            self.restores
                .borrow_mut()
                .pop_front()
                .expect("restore script")
        }
    }
    fn generation() -> EngineGen {
        EngineGen {
            host_id: rakukan_engine_rpc::HostId(7),
            generation: 8,
        }
    }
    fn edit() -> EditState {
        EditState {
            reading: "reading".into(),
            pending_romaji: "t".into(),
        }
    }
    fn rpc(
        changes: Vec<Result<ChangeReply, rakukan_engine_rpc::ChangeError>>,
        restores: Vec<Result<RestoreReply, rakukan_engine_rpc::ChangeError>>,
    ) -> FakeRpc {
        FakeRpc {
            sent: RefCell::new(vec![]),
            changes: RefCell::new(changes.into()),
            restores: RefCell::new(restores.into()),
        }
    }
    #[test]
    fn first_change_restores_then_later_change_uses_restored_generation() {
        let outcome = ChangeOutcome::Unit { edit: edit() };
        let rpc = rpc(
            vec![Ok(ChangeReply::Changed(outcome.clone()))],
            vec![Ok(RestoreReply::Restored {
                engine_gen: generation(),
                then: Some(outcome),
            })],
        );
        let mut state = RemoteComposition::new();
        apply_via(&rpc, &mut state, ChangeRequest::PushChar(116), true).unwrap();
        assert_eq!(state.edit, edit());
        apply_via(&rpc, &mut state, ChangeRequest::Backspace, true).unwrap();
        let sent = rpc.sent.borrow();
        assert!(
            matches!(&sent[0], Sent::Restore(owner, reading, pending, Some(ChangeRequest::PushChar(116))) if *owner==state.owner && reading.is_empty() && pending.is_empty())
        );
        assert!(
            matches!(&sent[1], Sent::Change(expect, ChangeRequest::Backspace) if expect.engine_gen == generation() && expect.owner==state.owner)
        );
    }
    #[test]
    fn timer_rejection_keeps_snapshot_and_never_restores() {
        let rpc = rpc(
            vec![Ok(ChangeReply::Rejected(Reason::OwnerMismatch))],
            vec![],
        );
        let mut state = RemoteComposition::new();
        state.generation = Some(generation());
        state.edit = edit();
        assert!(
            apply_via(
                &rpc,
                &mut state,
                ChangeRequest::BgStart { n_cands: 4 },
                false
            )
            .is_err()
        );
        assert_eq!(state.edit, edit());
        assert_eq!(rpc.sent.borrow().len(), 1);
        assert!(matches!(rpc.sent.borrow()[0], Sent::Change(_, _)));
    }
    #[test]
    fn foreground_rejection_restores_exact_reading_and_pending_then_applies_key() {
        let outcome = ChangeOutcome::Bool {
            value: true,
            edit: EditState {
                reading: "reading".into(),
                pending_romaji: "".into(),
            },
        };
        let rpc = rpc(
            vec![Ok(ChangeReply::Rejected(Reason::GenMismatch))],
            vec![Ok(RestoreReply::Restored {
                engine_gen: generation(),
                then: Some(outcome.clone()),
            })],
        );
        let mut state = RemoteComposition::new();
        state.generation = Some(generation());
        state.edit = edit();
        assert_eq!(
            apply_via(&rpc, &mut state, ChangeRequest::Backspace, true).unwrap(),
            outcome
        );
        assert!(
            matches!(&rpc.sent.borrow()[1], Sent::Restore(_, reading, pending, Some(ChangeRequest::Backspace)) if reading=="reading" && pending=="t")
        );
    }
    #[test]
    fn unresolved_block_does_not_overwrite_the_original_pending_key() {
        let mut state = RemoteComposition::new();
        state.generation = Some(generation());
        state.edit = edit();
        state.pending = Some(ChangeRequest::PushChar(116));
        let unresolved = rakukan_engine_rpc::Unresolved {
            seq: 5,
            kind: rakukan_engine_rpc::ChangeKind::PushChar,
            owner: state.owner,
            engine_gen: generation(),
        };
        let rpc = rpc(
            vec![Err(rakukan_engine_rpc::ChangeError::Unresolved(unresolved))],
            vec![],
        );
        assert!(apply_via(&rpc, &mut state, ChangeRequest::PushChar(97), true).is_err());
        assert_eq!(state.pending, Some(ChangeRequest::PushChar(116)));
        assert_eq!(state.edit, edit());
    }
    #[test]
    fn bg_timer_cannot_create_ownership_and_irreversible_requests_are_not_replayed() {
        let rpc = rpc(vec![], vec![]);
        let mut state = RemoteComposition::new();
        assert!(
            apply_via(
                &rpc,
                &mut state,
                ChangeRequest::BgStart { n_cands: 4 },
                false
            )
            .is_err()
        );
        assert!(rpc.sent.borrow().is_empty());
        for request in [
            ChangeRequest::Commit {
                text: "text".into(),
            },
            ChangeRequest::Learn {
                reading: "r".into(),
                surface: "s".into(),
            },
            ChangeRequest::BgTakeCandidates { key: "r".into() },
        ] {
            assert!(!replayable(&request));
        }
    }
    #[test]
    fn read_errors_distinguish_ownership_transport_host_and_no_composition() {
        assert!(ReadError::Rejected(Reason::OwnerMismatch).ownership_lost());
        assert!(ReadError::Rejected(Reason::GenMismatch).ownership_lost());
        assert!(!ReadError::Host("poison".into()).ownership_lost());
        assert!(!ReadError::Transport(anyhow::anyhow!("disconnected")).ownership_lost());
        assert!(!ReadError::NoComposition.ownership_lost());
    }
}
