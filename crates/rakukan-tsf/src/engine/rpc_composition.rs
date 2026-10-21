//! composition ごとの所有者と復元元（Issue #56）。`RpcEngine` の再接続をまたいで保持する。
//! タイマーからの読み取り・変更では所有権を取り返さない。復元は、フォーカス中の文脈へのキー入力からだけ行う。
use rakukan_engine_rpc::{
    BgStartOutcome, BgTakeOutcome, BgView, ChangeOutcome, ChangeReply, ChangeRequest, Checkpoint,
    EditState, EngineGen, Expect, InputCharKind, Owner, ReadError, ReadRequest, Reason, Replay,
    Response, RestoreJournalReply, RpcEngine,
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
/// 編集状態の鍵: DM のポインタと世代（入力処理の見直しの段 3）。
///
/// ポインタだけでは、破棄された DM と同じアドレスに作られた別の DM の状態を取り違える。
type DmKey = crate::engine::dm_registry::DmRef;
/// フォーカス中の DM が無いときの鍵。
const NO_DM: DmKey = DmKey {
    ptr: 0,
    generation: 0,
};
static COMPOSITIONS: LazyLock<Mutex<HashMap<DmKey, RemoteComposition>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
pub struct RemoteComposition {
    owner: Owner,
    generation: Option<EngineGen>,
    /// ホストが確かめた最新の編集状態。
    edit: EditState,
    /// composition の開始から送った編集要求（入力処理の見直しの段 4、設計書 4.3）。
    /// 再送できる要求の置き場所はここだけ。ホストの適用を確かめていない末尾を含む。
    journal: Vec<ChangeRequest>,
    /// journal の先頭 `.0` 件を適用した状態として、ホストが確かめた値（照合点）。
    checkpoint: Option<(usize, EditState)>,
    bg_reading: Option<String>,
    /// この編集状態を表示している TSF の composition（`composition_identity`）。
    /// 0 = まだ結び付いていない。終了の通知の照合に使う（Issue #71）。
    tsf_comp: usize,
    /// この状態を作ったときに保持していた composition（0 = 無し）。まだ結び付いていない
    /// 状態が、どの composition の続きとして作られたかの照合に使う（Issue #71）。
    created_under: usize,
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
            journal: Vec::new(),
            checkpoint: None,
            bg_reading: None,
            tsf_comp: 0,
            created_under: crate::engine::state::current_composition_identity_for(dm_key().ptr),
        }
    }
    fn expect(&self) -> Option<Expect> {
        self.generation.map(|engine_gen| Expect {
            engine_gen,
            owner: self.owner,
        })
    }
    /// ホストの適用を確かめた journal の件数。
    fn confirmed_len(&self) -> usize {
        self.checkpoint.as_ref().map_or(0, |(len, _)| *len)
    }
}
fn dm_key() -> DmKey {
    crate::tsf::candidate_window::current_dm_hwnd()
        .0
        .unwrap_or(NO_DM)
}
fn lock_states() -> std::sync::MutexGuard<'static, HashMap<DmKey, RemoteComposition>> {
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
/// その DM（ポインタと世代）の状態を外す（Deactivate で失効させた DM）。
pub fn dispose_ref(dm: DmKey) {
    lock_states().remove(&dm);
}
/// DM（ポインタ `dm`）が破棄された。その DM の状態を、世代にかかわらずすべて外す。
pub fn dispose(dm: usize) {
    lock_states().retain(|key, _| key.ptr != dm);
}
/// composition の表示を要求した時点の、編集状態の位置と所有者（Issue #71）。
///
/// EditSession は遅れて実行されることがあり、その間にフォーカスや所有者が変わりうる。
/// 要求の時点で控え、実行時に同じ状態のままなら結び付ける。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindTarget {
    dm: DmKey,
    owner: Owner,
}
/// 今の文脈の編集状態の [`BindTarget`]。状態が無ければ `None`。
pub fn bind_target() -> Option<BindTarget> {
    let dm = dm_key();
    lock_states().get(&dm).map(|state| BindTarget {
        dm,
        owner: state.owner,
    })
}
/// 要求の時点で控えた編集状態を、表示している TSF の composition に結び付ける（Issue #71）。
/// その位置の状態が別の所有者に替わっていれば、何もしない。
///
/// `comp_dm` は composition を始めた文書（`composition_set_with_dm` に渡した値、
/// 新しく始める場合はその文脈の文書）。控えた文書と違えば結び付けない。共有の
/// composition が、フォーカスの移った別の文書の状態と対応していない場合があるため。
pub fn bind_composition(target: Option<BindTarget>, tsf_comp: usize, comp_dm: usize) {
    if let Some(target) = target {
        bind_in(&mut lock_states(), target, tsf_comp, comp_dm);
    }
}
fn bind_in(
    states: &mut HashMap<DmKey, RemoteComposition>,
    target: BindTarget,
    tsf_comp: usize,
    comp_dm: usize,
) {
    if comp_dm != target.dm.ptr {
        tracing::debug!(
            "composition bind skipped: composition dm={comp_dm:#x} state dm={:#x}",
            target.dm.ptr
        );
        return;
    }
    if let Some(state) = states.get_mut(&target.dm)
        && state.owner == target.owner
    {
        state.tsf_comp = tsf_comp;
    }
}
/// アプリが終わらせた composition の編集状態を、手元から外して返す（Issue #71）。
///
/// 外すのは次の状態。遅れて届いた通知で、別の composition の状態を消さない。
/// - その composition に結び付いた状態
/// - まだ結び付いていない状態のうち、その composition を保持している間に作られたもの
///   （表示の前に失敗した、または表示の EditSession がまだ実行されていない続きの入力）。
///   ただし `replacing`（部分確定などで、その composition を新しいものへ置き換える
///   EditSession を要求済み）の場合は、新しい composition のための状態なので外さない
pub fn take_terminated(tsf_comp: usize, replacing: bool) -> Vec<RemoteComposition> {
    let taken = take_terminated_from(&mut lock_states(), tsf_comp, replacing);
    if !taken.is_empty() {
        crate::tsf::candidate_window::reset_worker_busy_clock();
    }
    taken
}
fn take_terminated_from(
    states: &mut HashMap<DmKey, RemoteComposition>,
    tsf_comp: usize,
    replacing: bool,
) -> Vec<RemoteComposition> {
    if tsf_comp == 0 {
        return Vec::new();
    }
    let dms: Vec<DmKey> = states
        .iter()
        .filter(|(_, state)| {
            state.tsf_comp == tsf_comp
                || (!replacing && state.tsf_comp == 0 && state.created_under == tsf_comp)
        })
        .map(|(dm, _)| *dm)
        .collect();
    dms.into_iter()
        .filter_map(|dm| states.remove(&dm))
        .collect()
}
/// 今の文脈の編集状態を、手元から外して返す（Deactivate 用）。
pub fn take_current() -> Option<RemoteComposition> {
    crate::tsf::candidate_window::reset_worker_busy_clock();
    lock_states().remove(&dm_key())
}
fn save(c: RemoteComposition) {
    lock_states().insert(dm_key(), c);
}
/// journal に入れる編集要求か（設計書 4.3）。
fn is_journal_request(request: &ChangeRequest) -> bool {
    matches!(
        request,
        ChangeRequest::InputChar { .. }
            | ChangeRequest::PushChar(_)
            | ChangeRequest::PushRaw(_)
            | ChangeRequest::PushFullwidthAlpha(_)
            | ChangeRequest::Backspace
            | ChangeRequest::FlushPendingN
            | ChangeRequest::ForcePreedit { .. }
            | ChangeRequest::SeedPreedit { .. }
            | ChangeRequest::RetainRange { .. }
    )
}
/// 所有権を取り直してでも送る要求か（キー入力の中だけ）。journal の編集要求と、
/// composition を閉じるリセット。
fn may_acquire_ownership(request: &ChangeRequest) -> bool {
    is_journal_request(request)
        || matches!(
            request,
            ChangeRequest::ResetPreedit | ChangeRequest::ResetAll
        )
}
/// journal に記録する形。BG の開始は流し直さない（`bg_start_n_cands` は `None`）。
fn journal_entry(request: &ChangeRequest) -> ChangeRequest {
    match request {
        ChangeRequest::InputChar { c, kind, .. } => ChangeRequest::InputChar {
            c: *c,
            kind: *kind,
            bg_start_n_cands: None,
        },
        other => other.clone(),
    }
}
/// journal の費用（設計書 4.3 の `cost`）の近似: 文字の要求は 1、文字列の要求はそのバイト数。
fn journal_cost(journal: &[ChangeRequest]) -> usize {
    journal
        .iter()
        .map(|request| match request {
            ChangeRequest::ForcePreedit { text } | ChangeRequest::SeedPreedit { text } => {
                text.len().max(1)
            }
            _ => 1,
        })
        .sum()
}
/// journal の上限（設計書 4.3）。段 4 では判定して DEBUG に残すだけで、有効にしない。
const JOURNAL_SOFT_COUNT: usize = 512;
const JOURNAL_SOFT_COST: usize = 32 * 1024;
const JOURNAL_HARD_COUNT: usize = 1024;
const JOURNAL_HARD_COST: usize = 128 * 1024;
fn note_journal_limits(journal: &[ChangeRequest]) {
    let count = journal.len();
    let cost = journal_cost(journal);
    if count >= JOURNAL_HARD_COUNT || cost >= JOURNAL_HARD_COST {
        tracing::debug!(
            "journal over the hard limit (not enforced yet): count={count} cost={cost}"
        );
    } else if count >= JOURNAL_SOFT_COUNT || cost >= JOURNAL_SOFT_COST {
        tracing::debug!(
            "journal over the soft limit (not enforced yet): count={count} cost={cost}"
        );
    }
}
/// 変更の後に、手元の状態を保存するかどうか。
///
/// 新しい composition の最初の変更が、何も送らないまま失敗した場合（`Commit` などの
/// journal に入らない要求は所有権を取れない）は保存しない。世代の無い状態を残すと、次の復元まで
/// 読み取りが `NoComposition` で失敗し続ける。journal に入れた要求は、再送のために保存する。
fn keep_after_change(fresh: bool, state: &RemoteComposition, ok: bool) -> bool {
    ok || !fresh || state.generation.is_some() || !state.journal.is_empty()
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
    fn restore_journal(
        &self,
        owner: Owner,
        journal: Vec<ChangeRequest>,
        checkpoint: Option<Checkpoint>,
    ) -> Result<RestoreJournalReply, rakukan_engine_rpc::ChangeError>;
    /// `owner` の結果不明の要求を断念する。断念したら `true`。
    fn abandon_for(&self, owner: Owner) -> bool;
}
impl CompositionRpc for RpcEngine {
    fn abandon_for(&self, owner: Owner) -> bool {
        self.abandon_unresolved_for(owner).is_some()
    }
    fn change(
        &self,
        expect: Expect,
        request: ChangeRequest,
    ) -> Result<ChangeReply, rakukan_engine_rpc::ChangeError> {
        self.change(expect, request)
    }
    fn restore_journal(
        &self,
        owner: Owner,
        journal: Vec<ChangeRequest>,
        checkpoint: Option<Checkpoint>,
    ) -> Result<RestoreJournalReply, rakukan_engine_rpc::ChangeError> {
        self.restore_journal(owner, Vec::new(), journal, checkpoint)
    }
}
/// journal の先頭 `upto` 件をホストで流し直し、所有権を取る（設計書 4.3）。
///
/// 照合点が `upto` 以内なら送る。`Exact` なら照合点を `upto` 件に進める。`Rebased` なら、
/// 送った照合点の値を種にした形（`SeedPreedit` + 未確定ローマ字の `PushChar`）に journal の
/// 先頭を書き換える（応答の最終値で作ると末尾が二重になる）。失敗したら何も変えない。
fn restore_journal_via(
    rpc: &impl CompositionRpc,
    state: &mut RemoteComposition,
    upto: usize,
) -> anyhow::Result<()> {
    let sent_checkpoint = state
        .checkpoint
        .as_ref()
        .filter(|(len, _)| *len <= upto)
        .map(|(len, edit)| Checkpoint {
            len: *len as u32,
            checkpoint_edit: edit.clone(),
        });
    match rpc.restore_journal(
        state.owner,
        state.journal[..upto].to_vec(),
        sent_checkpoint.clone(),
    )? {
        RestoreJournalReply::Restored {
            engine_gen,
            replay,
            final_edit,
            ..
        } => {
            let confirmed = match (replay, sent_checkpoint) {
                (Replay::Rebased, Some(checkpoint)) => {
                    let base = checkpoint.checkpoint_edit;
                    let mut rebased = vec![ChangeRequest::SeedPreedit { text: base.reading }];
                    rebased.extend(
                        base.pending_romaji
                            .chars()
                            .map(|c| ChangeRequest::PushChar(c as u32)),
                    );
                    let tail_start = checkpoint.len as usize;
                    rebased.extend(state.journal[tail_start..upto].iter().cloned());
                    let len = rebased.len();
                    rebased.extend(state.journal[upto..].iter().cloned());
                    tracing::info!(
                        "journal rebased on the checkpoint: {} -> {} request(s)",
                        state.journal.len(),
                        rebased.len()
                    );
                    state.journal = rebased;
                    len
                }
                _ => upto,
            };
            state.generation = Some(engine_gen);
            state.bg_reading = None;
            state.checkpoint = Some((confirmed, final_edit.clone()));
            state.edit = final_edit;
            Ok(())
        }
        RestoreJournalReply::Rejected(reason) => anyhow::bail!("restore rejected: {reason:?}"),
    }
}
/// 変更要求を送る（設計書 4.3 の送り方）。
///
/// journal の編集要求は、送る前に journal の末尾に入れる。所有権が無い、または確かめていない
/// 末尾が前にある場合は、キー入力の中でだけ、それまでの journal を `RestoreJournal` で流し直して
/// から、この要求を `Change` で送る（結果を呼び出し側へ返すため）。失敗しても journal は
/// そのまま（次のキーで流し直す）。
fn apply_via(
    rpc: &impl CompositionRpc,
    state: &mut RemoteComposition,
    request: ChangeRequest,
    foreground: bool,
) -> anyhow::Result<ChangeOutcome> {
    let journal_request = is_journal_request(&request);
    if journal_request {
        state.journal.push(journal_entry(&request));
        note_journal_limits(&state.journal);
    }
    // この要求より前の journal の件数
    let prior = if journal_request {
        state.journal.len() - 1
    } else {
        state.journal.len()
    };
    let mut restored = false;
    if state.expect().is_none() || state.confirmed_len() < prior {
        if !foreground || !may_acquire_ownership(&request) {
            anyhow::bail!("timer or irreversible request cannot acquire ownership");
        }
        restore_journal_via(rpc, state, prior)?;
        restored = true;
    }
    loop {
        let expect = state
            .expect()
            .ok_or_else(|| anyhow::anyhow!("no ownership after restore"))?;
        match rpc.change(expect, request.clone()) {
            Ok(ChangeReply::Changed(outcome)) => {
                if journal_request {
                    state.checkpoint = Some((state.journal.len(), outcome.edit().clone()));
                }
                state.edit = outcome.edit().clone();
                return Ok(outcome);
            }
            Ok(ChangeReply::Rejected(reason)) => {
                let error = ReadError::Rejected(reason);
                note_read_error(&error);
                // 所有権を失った: キー入力の中なら、それまでの journal を流し直して 1 回だけ送り直す
                if error.ownership_lost()
                    && foreground
                    && may_acquire_ownership(&request)
                    && !restored
                {
                    let prior = if journal_request {
                        state.journal.len() - 1
                    } else {
                        state.journal.len()
                    };
                    restore_journal_via(rpc, state, prior)?;
                    restored = true;
                    continue;
                }
                return Err(error.into());
            }
            // 通信失敗・未解決に阻まれた: journal に残した要求は、次のキーで流し直す
            Err(error) => return Err(error.into()),
        }
    }
}
/// 終わった composition の後始末で送った 1 件の要求の結果（Issue #71）。
#[derive(Debug, PartialEq, Eq)]
enum FinishResult {
    Applied,
    Rejected(Reason),
    /// 別の composition の結果不明の要求に阻まれ、送っていない。
    BlockedByOther,
    /// 送ったが応答を受け取れなかった。この composition の要求として断念した。
    LostAndAbandoned,
    /// 接続できず、送っていない（台帳に結果不明の要求は残っていない）。
    NotSent(String),
    Host(String),
}
/// アプリが終わらせた composition の、ホスト側の後始末（Issue #71）。
///
/// 手元にある既存の `Expect` に対して `BgReclaim` と、`reset_all` なら `ResetAll` を送る。
/// 所有権は取り直さない（Restore は送らない）。別の composition がホストの所有者に
/// なっていれば `OwnerMismatch` で拒否され、その composition の状態は変わらない。
/// 結果不明の要求は、この composition のものだけを断念する（再送の機会はもう無い）。
fn finish_via(
    rpc: &impl CompositionRpc,
    state: &RemoteComposition,
    reset_all: bool,
) -> Vec<(ChangeRequest, FinishResult)> {
    // 送る前の、この composition の結果不明の要求（応答を失った打鍵・初回の Restore など）は
    // 再送しない。世代の無い状態（初回の Restore の応答を失った）でも断念する
    rpc.abandon_for(state.owner);
    let Some(expect) = state.expect() else {
        return Vec::new();
    };
    let mut requests = vec![ChangeRequest::BgReclaim];
    if reset_all {
        requests.push(ChangeRequest::ResetAll);
    }
    requests
        .into_iter()
        .map(|request| {
            let result = match rpc.change(expect, request.clone()) {
                Ok(ChangeReply::Changed(_)) => FinishResult::Applied,
                Ok(ChangeReply::Rejected(reason)) => {
                    // ResultUnavailable は結果不明として台帳に残るので、ここで断念する
                    rpc.abandon_for(expect.owner);
                    FinishResult::Rejected(reason)
                }
                Err(rakukan_engine_rpc::ChangeError::Unresolved(_)) => FinishResult::BlockedByOther,
                // 接続の失敗は採番・送信の前にも起きる。台帳に残ったかで区別する
                Err(rakukan_engine_rpc::ChangeError::Transport(error)) => {
                    if rpc.abandon_for(expect.owner) {
                        FinishResult::LostAndAbandoned
                    } else {
                        FinishResult::NotSent(error.to_string())
                    }
                }
                Err(rakukan_engine_rpc::ChangeError::Host(error)) => FinishResult::Host(error),
            };
            (request, result)
        })
        .collect()
}
/// `recover_on_input` の結果。`bg` は、入口の読み取りで得た BG 状態（復元した場合は `None`）。
pub struct InputEntry {
    pub restored: bool,
    pub bg: Option<BgView>,
}
pub struct DynEngine(RpcEngine);
impl std::ops::Deref for DynEngine {
    type Target = RpcEngine;
    fn deref(&self) -> &RpcEngine {
        &self.0
    }
}
impl DynEngine {
    pub fn connect_or_spawn_with_source(
        source: rakukan_engine_rpc::ConfigSource,
    ) -> anyhow::Result<Self> {
        Ok(Self(RpcEngine::connect_or_spawn_with_source(source)?))
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

    /// フォーカス中の有効な文脈へのキー入力からだけ呼ぶ。
    ///
    /// 所有権の確認に使う読み取りは `BgStatus` で、その値を `InputEntry::bg` で返す。
    /// 呼び出し側の診断ログはこの値を使い、同じ打鍵で `bg_status()` を読み直さない。
    pub fn recover_on_input(&self) -> anyhow::Result<InputEntry> {
        let Some(mut state) = snapshot() else {
            return Ok(InputEntry {
                restored: false,
                bg: Some(BgView::Idle),
            });
        };
        let mut bg = None;
        let lost = match state.expect() {
            Some(expect) => match self.0.bg_status(expect) {
                Ok(view) => {
                    if view != BgView::WorkerBusy {
                        crate::tsf::candidate_window::reset_worker_busy_clock();
                    }
                    bg = Some(view);
                    false
                }
                Err(error) if error.ownership_lost() => true,
                Err(error) => {
                    note_read_error(&error);
                    return Err(error.into());
                }
            },
            None => true,
        };
        let unconfirmed = state.confirmed_len() < state.journal.len();
        if !lost && !unconfirmed && self.0.unresolved().is_none() {
            return Ok(InputEntry {
                restored: false,
                bg,
            });
        }
        // journal 全体を流し直す。台帳の未解決のうち編集要求は、どこかの composition の journal に
        // 確かめていない末尾として残っているので、`RestoredJournal` で台帳が空になっても失われない
        // （その composition の次のキーで流し直す）。副作用（BG・確定・学習）の結果不明の記録は
        // 段 5 で入れる。それまでは commit / learn / reset が今までどおり台帳全体を断念する
        let upto = state.journal.len();
        let result = restore_journal_via(&self.0, &mut state, upto);
        save(state);
        result?;
        // 復元の後の BG 状態は読んでいない。
        Ok(InputEntry {
            restored: true,
            bg: None,
        })
    }
    fn change_local(&self, request: ChangeRequest) -> anyhow::Result<ChangeOutcome> {
        let existing = snapshot();
        let fresh = existing.is_none();
        let mut state = match existing {
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
        if keep_after_change(fresh, &state, result.is_ok()) {
            save(state);
        }
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
    /// 後ろに表示の読み取りが続くので、失敗を返す。呼び出し側は、そのキーの処理を中断する。
    pub fn push_raw(&self, c: char) -> anyhow::Result<()> {
        self.change_local(ChangeRequest::PushRaw(c as u32))
            .map(|_| ())
    }
    /// `push_raw` と同じく、失敗を返す。
    pub fn force_preedit(&self, text: String) -> anyhow::Result<()> {
        self.change_local(ChangeRequest::ForcePreedit { text })
            .map(|_| ())
    }
    /// アプリが終わらせた composition（`take_terminated` / `take_current` で外した状態）の
    /// ホスト側の後始末。所有権は取り直さない（Issue #71）。
    pub fn finish_composition(&self, state: &RemoteComposition, reset_all: bool) {
        let results = finish_via(&self.0, state, reset_all);
        if results.is_empty() {
            tracing::debug!(
                "composition finish: nothing sent (no ownership) owner={:?}",
                state.owner
            );
        }
        for (request, result) in results {
            tracing::debug!(
                "composition finish: {:?} -> {result:?} owner={:?}",
                request.kind(),
                state.owner
            );
        }
    }
    pub fn bg_reclaim(&self) {
        // 所有権を持つ composition が無ければ、回収する相手も無い。送らずに戻る。
        if snapshot().and_then(|state| state.expect()).is_none() {
            return;
        }
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
        Restore(Owner, Vec<ChangeRequest>, Option<Checkpoint>),
        Change(Expect, ChangeRequest),
    }
    struct FakeRpc {
        sent: RefCell<Vec<Sent>>,
        changes: RefCell<VecDeque<Result<ChangeReply, rakukan_engine_rpc::ChangeError>>>,
        restores: RefCell<VecDeque<Result<RestoreJournalReply, rakukan_engine_rpc::ChangeError>>>,
        /// 断念を試みた owner。
        abandoned: RefCell<Vec<Owner>>,
        /// 断念の結果の台本（尽きたら `false` = 台帳に該当なし）。
        abandon_results: RefCell<VecDeque<bool>>,
    }
    impl CompositionRpc for FakeRpc {
        fn abandon_for(&self, owner: Owner) -> bool {
            self.abandoned.borrow_mut().push(owner);
            self.abandon_results
                .borrow_mut()
                .pop_front()
                .unwrap_or(false)
        }
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
        fn restore_journal(
            &self,
            owner: Owner,
            journal: Vec<ChangeRequest>,
            checkpoint: Option<Checkpoint>,
        ) -> Result<RestoreJournalReply, rakukan_engine_rpc::ChangeError> {
            self.sent
                .borrow_mut()
                .push(Sent::Restore(owner, journal, checkpoint));
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
    fn restored(replay: Replay, final_edit: EditState) -> RestoreJournalReply {
        RestoreJournalReply::Restored {
            engine_gen: generation(),
            replay,
            final_edit,
            bg: BgView::Idle,
            effects: vec![],
        }
    }
    fn reading(reading: &str, pending: &str) -> EditState {
        EditState {
            reading: reading.into(),
            pending_romaji: pending.into(),
        }
    }
    fn changed(edit: EditState) -> Result<ChangeReply, rakukan_engine_rpc::ChangeError> {
        Ok(ChangeReply::Changed(ChangeOutcome::Unit { edit }))
    }
    fn rpc(
        changes: Vec<Result<ChangeReply, rakukan_engine_rpc::ChangeError>>,
        restores: Vec<Result<RestoreJournalReply, rakukan_engine_rpc::ChangeError>>,
    ) -> FakeRpc {
        FakeRpc {
            sent: RefCell::new(vec![]),
            changes: RefCell::new(changes.into()),
            restores: RefCell::new(restores.into()),
            abandoned: RefCell::new(vec![]),
            abandon_results: RefCell::new(VecDeque::new()),
        }
    }
    fn owned_state() -> RemoteComposition {
        let mut state = RemoteComposition::new();
        state.generation = Some(generation());
        state.edit = edit();
        state
    }
    fn unit() -> Result<ChangeReply, rakukan_engine_rpc::ChangeError> {
        Ok(ChangeReply::Changed(ChangeOutcome::Unit {
            edit: EditState::default(),
        }))
    }
    #[test]
    fn finish_sends_reclaim_and_reset_all_with_the_existing_expect_and_never_restores() {
        let fake = rpc(vec![unit(), unit()], vec![]);
        let state = owned_state();
        let results = finish_via(&fake, &state, true);
        assert_eq!(
            results,
            vec![
                (ChangeRequest::BgReclaim, FinishResult::Applied),
                (ChangeRequest::ResetAll, FinishResult::Applied),
            ]
        );
        let sent = fake.sent.borrow();
        assert_eq!(sent.len(), 2);
        for s in sent.iter() {
            assert!(
                matches!(s, Sent::Change(expect, _) if expect.owner == state.owner && expect.engine_gen == generation())
            );
        }
        // 送る前に、この composition の結果不明の要求だけを断念する
        assert_eq!(fake.abandoned.borrow().first(), Some(&state.owner));
    }
    #[test]
    fn finish_on_deactivate_sends_only_reclaim() {
        let fake = rpc(vec![unit()], vec![]);
        let results = finish_via(&fake, &owned_state(), false);
        assert_eq!(
            results,
            vec![(ChangeRequest::BgReclaim, FinishResult::Applied)]
        );
    }
    #[test]
    fn finish_without_ownership_sends_nothing_but_abandons_its_lost_restore() {
        // 初回の Restore の応答を失った composition は世代を持たない。何も送らないが、
        // 台帳に残ったその Restore は断念する
        let fake = rpc(vec![], vec![]);
        let state = RemoteComposition::new();
        assert!(finish_via(&fake, &state, true).is_empty());
        assert!(fake.sent.borrow().is_empty());
        assert_eq!(*fake.abandoned.borrow(), vec![state.owner]);
    }
    #[test]
    fn finish_rejected_by_another_owner_does_not_restore() {
        // 別の composition が所有者なら拒否される。所有権を取り直さない
        let fake = rpc(
            vec![
                Ok(ChangeReply::Rejected(Reason::OwnerMismatch)),
                Ok(ChangeReply::Rejected(Reason::OwnerMismatch)),
            ],
            vec![],
        );
        let results = finish_via(&fake, &owned_state(), true);
        assert_eq!(
            results,
            vec![
                (
                    ChangeRequest::BgReclaim,
                    FinishResult::Rejected(Reason::OwnerMismatch)
                ),
                (
                    ChangeRequest::ResetAll,
                    FinishResult::Rejected(Reason::OwnerMismatch)
                ),
            ]
        );
        assert!(
            fake.sent
                .borrow()
                .iter()
                .all(|s| matches!(s, Sent::Change(..)))
        );
    }
    #[test]
    fn finish_after_lost_reclaim_reply_abandons_it_and_still_sends_reset_all() {
        let fake = rpc(
            vec![
                Err(rakukan_engine_rpc::ChangeError::Transport(anyhow::anyhow!(
                    "disconnected"
                ))),
                unit(),
            ],
            vec![],
        );
        // 送る前は台帳に該当なし、応答を失った後は該当あり
        *fake.abandon_results.borrow_mut() = VecDeque::from([false, true]);
        let state = owned_state();
        let results = finish_via(&fake, &state, true);
        assert_eq!(
            results,
            vec![
                (ChangeRequest::BgReclaim, FinishResult::LostAndAbandoned),
                (ChangeRequest::ResetAll, FinishResult::Applied),
            ]
        );
        // 送る前・応答を失った後の 2 回、この composition の分だけを断念する
        assert_eq!(*fake.abandoned.borrow(), vec![state.owner, state.owner]);
    }
    #[test]
    fn finish_distinguishes_not_sent_from_lost_reply() {
        // 接続できず送っていない場合は、台帳に何も残っていない
        let fake = rpc(
            vec![
                Err(rakukan_engine_rpc::ChangeError::Transport(anyhow::anyhow!(
                    "connect failed"
                ))),
                Err(rakukan_engine_rpc::ChangeError::Transport(anyhow::anyhow!(
                    "connect failed"
                ))),
            ],
            vec![],
        );
        let results = finish_via(&fake, &owned_state(), true);
        assert!(
            results.iter().all(
                |(_, r)| matches!(r, FinishResult::NotSent(e) if e.contains("connect failed"))
            )
        );
    }
    #[test]
    fn finish_after_lost_reset_all_or_result_unavailable_abandons_it() {
        let fake = rpc(
            vec![
                Ok(ChangeReply::Rejected(Reason::ResultUnavailable)),
                Err(rakukan_engine_rpc::ChangeError::Transport(anyhow::anyhow!(
                    "disconnected"
                ))),
            ],
            vec![],
        );
        // 送る前: なし、ResultUnavailable の後: あり、ResetAll の応答消失の後: あり
        *fake.abandon_results.borrow_mut() = VecDeque::from([false, true, true]);
        let state = owned_state();
        let results = finish_via(&fake, &state, true);
        assert_eq!(
            results,
            vec![
                (
                    ChangeRequest::BgReclaim,
                    FinishResult::Rejected(Reason::ResultUnavailable)
                ),
                (ChangeRequest::ResetAll, FinishResult::LostAndAbandoned),
            ]
        );
        assert_eq!(fake.abandoned.borrow().len(), 3);
    }
    #[test]
    fn finish_blocked_by_another_unresolved_request_keeps_it() {
        let other = RemoteComposition::new();
        let unresolved = rakukan_engine_rpc::Unresolved {
            seq: 9,
            kind: rakukan_engine_rpc::ChangeKind::PushChar,
            owner: other.owner,
            engine_gen: generation(),
        };
        let fake = rpc(
            vec![
                Err(rakukan_engine_rpc::ChangeError::Unresolved(unresolved)),
                Err(rakukan_engine_rpc::ChangeError::Unresolved(unresolved)),
            ],
            vec![],
        );
        let state = owned_state();
        let results = finish_via(&fake, &state, true);
        assert!(
            results
                .iter()
                .all(|(_, r)| *r == FinishResult::BlockedByOther)
        );
        // 断念を試みるのは、終わった composition の分だけ
        assert!(
            fake.abandoned
                .borrow()
                .iter()
                .all(|owner| *owner == state.owner)
        );
    }
    /// (DM, 結び付いた composition, 作ったときに保持していた composition)
    fn dm(ptr: usize) -> DmKey {
        DmKey { ptr, generation: 1 }
    }
    fn states_with(entries: &[(usize, usize, usize)]) -> HashMap<DmKey, RemoteComposition> {
        entries
            .iter()
            .map(|&(dm_ptr, tsf_comp, created_under)| {
                let mut state = owned_state();
                state.tsf_comp = tsf_comp;
                state.created_under = created_under;
                (dm(dm_ptr), state)
            })
            .collect()
    }
    #[test]
    fn terminated_state_is_found_by_its_composition_even_after_focus_moved() {
        // A（DM 0xA、composition 0x1）の通知が、B（DM 0xB）にフォーカスが移った後に届く
        let mut states = states_with(&[(0xA, 0x1, 0), (0xB, 0x2, 0)]);
        let taken = take_terminated_from(&mut states, 0x1, false);
        assert_eq!(taken.len(), 1);
        assert_eq!(taken[0].tsf_comp, 0x1);
        assert!(states.contains_key(&dm(0xB)), "B の状態を消さない");
        assert!(!states.contains_key(&dm(0xA)));
    }
    #[test]
    fn late_notification_for_an_unknown_composition_changes_nothing() {
        // 同じ DM で新しい composition（0x3）が始まった後に、古い通知（0x1）が届く
        let mut states = states_with(&[(0xA, 0x3, 0x1)]);
        assert!(take_terminated_from(&mut states, 0x1, false).is_empty());
        assert!(states.contains_key(&dm(0xA)));
    }
    #[test]
    fn unbound_state_created_under_the_composition_is_taken_unless_replacing() {
        // 0x1 を保持している間に作った、まだ表示していない状態（表示の前に失敗した等）
        let mut states = states_with(&[(0xA, 0, 0x1)]);
        // 部分確定で置き換え中: 新しい composition のための状態なので外さない
        assert!(take_terminated_from(&mut states, 0x1, true).is_empty());
        assert!(states.contains_key(&dm(0xA)));
        // 置き換え中でなければ、終わった composition の続きとして外す
        assert_eq!(take_terminated_from(&mut states, 0x1, false).len(), 1);
        assert!(states.is_empty());
    }
    #[test]
    fn unbound_state_created_without_a_composition_is_never_taken() {
        // 最初の打鍵（composition が無い間）に作った状態は、どの終了通知でも外さない
        let mut states = states_with(&[(0xA, 0, 0)]);
        assert!(take_terminated_from(&mut states, 0x1, false).is_empty());
        assert!(take_terminated_from(&mut states, 0, false).is_empty());
        assert!(states.contains_key(&dm(0xA)));
    }
    #[test]
    fn binding_uses_the_dm_and_owner_captured_at_request_time() {
        let mut states = states_with(&[(0xA, 0, 0), (0xB, 0, 0)]);
        let target_a = BindTarget {
            dm: dm(0xA),
            owner: states[&dm(0xA)].owner,
        };
        // 共有の composition が別の文書（B）のものなら結び付けない
        bind_in(&mut states, target_a, 0x1, 0xB);
        assert_eq!(states[&dm(0xA)].tsf_comp, 0);
        // 実行時にフォーカスが B に移っていても、要求の時点の A に結び付ける
        bind_in(&mut states, target_a, 0x1, 0xA);
        assert_eq!(states[&dm(0xA)].tsf_comp, 0x1);
        assert_eq!(states[&dm(0xB)].tsf_comp, 0);
        // 同じ DM の状態が別の所有者に替わっていれば結び付けない
        let mut replaced = owned_state();
        replaced.tsf_comp = 0;
        states.insert(dm(0xA), replaced);
        bind_in(&mut states, target_a, 0x2, 0xA);
        assert_eq!(states[&dm(0xA)].tsf_comp, 0);
    }
    #[test]
    fn states_of_a_reused_dm_address_are_kept_apart_by_generation() {
        // 破棄された DM と同じアドレスに新しい DM ができても、世代で区別する
        let old = DmKey {
            ptr: 0xA,
            generation: 1,
        };
        let new = DmKey {
            ptr: 0xA,
            generation: 2,
        };
        let mut states = HashMap::new();
        states.insert(old, owned_state());
        let target_new = BindTarget {
            dm: new,
            owner: states[&old].owner,
        };
        // 新しい DM の鍵では、古い DM の状態に結び付けない
        bind_in(&mut states, target_new, 0x1, 0xA);
        assert_eq!(states[&old].tsf_comp, 0);
        assert!(!states.contains_key(&new));
    }
    #[test]
    fn first_key_acquires_ownership_with_an_empty_journal_then_sends_the_key_as_a_change() {
        // 最初の打鍵: それまでの journal（空）を流し直して所有権を取り、打鍵は Change で送る
        let fake = rpc(
            vec![changed(reading("", "t")), changed(reading("", ""))],
            vec![Ok(restored(Replay::Exact, reading("", "")))],
        );
        let mut state = RemoteComposition::new();
        apply_via(&fake, &mut state, ChangeRequest::PushChar(116), true).unwrap();
        assert_eq!(state.journal, vec![ChangeRequest::PushChar(116)]);
        assert_eq!(state.checkpoint, Some((1, reading("", "t"))));
        apply_via(&fake, &mut state, ChangeRequest::Backspace, true).unwrap();
        assert_eq!(state.checkpoint, Some((2, reading("", ""))));
        let sent = fake.sent.borrow();
        assert!(
            matches!(&sent[0], Sent::Restore(owner, journal, None) if *owner == state.owner && journal.is_empty())
        );
        assert!(matches!(
            &sent[1],
            Sent::Change(_, ChangeRequest::PushChar(116))
        ));
        assert!(
            matches!(&sent[2], Sent::Change(expect, ChangeRequest::Backspace) if expect.engine_gen == generation())
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
    fn lost_reply_keeps_the_key_in_the_journal_and_the_next_key_replays_it() {
        // `a` の応答を失った: journal に残り、照合点は進まない
        let fake = rpc(
            vec![
                Err(rakukan_engine_rpc::ChangeError::Transport(anyhow::anyhow!(
                    "disconnected"
                ))),
                changed(reading("かk", "")),
            ],
            vec![Ok(restored(Replay::Exact, reading("か", "")))],
        );
        let mut state = RemoteComposition::new();
        state.generation = Some(generation());
        state.journal = vec![ChangeRequest::PushChar('k' as u32)];
        state.checkpoint = Some((1, reading("", "k")));
        assert!(apply_via(&fake, &mut state, ChangeRequest::PushChar('a' as u32), true).is_err());
        assert_eq!(state.journal.len(), 2);
        assert_eq!(state.confirmed_len(), 1);
        // 次の打鍵 `k`: 確かめていない `a` までを照合点つきで流し直し、`k` を Change で送る
        apply_via(&fake, &mut state, ChangeRequest::PushChar('k' as u32), true).unwrap();
        let sent = fake.sent.borrow();
        assert!(matches!(
            &sent[1],
            Sent::Restore(_, journal, Some(cp)) if journal.len() == 2 && cp.len == 1 && cp.checkpoint_edit == reading("", "k")
        ));
        assert!(matches!(
            &sent[2],
            Sent::Change(_, ChangeRequest::PushChar(107))
        ));
        assert_eq!(state.journal.len(), 3);
        assert_eq!(state.checkpoint, Some((3, reading("かk", ""))));
    }
    #[test]
    fn rebased_restore_rewrites_the_journal_head_from_the_sent_checkpoint() {
        let fake = rpc(
            vec![changed(reading("カき", ""))],
            vec![Ok(restored(Replay::Rebased, reading("カ", "k")))],
        );
        let mut state = RemoteComposition::new();
        state.generation = None;
        state.journal = vec![
            ChangeRequest::PushChar('k' as u32),
            ChangeRequest::PushChar('a' as u32),
            ChangeRequest::PushChar('k' as u32),
        ];
        state.checkpoint = Some((2, reading("カ", "")));
        apply_via(&fake, &mut state, ChangeRequest::PushChar('i' as u32), true).unwrap();
        // 先頭 2 件が照合点の値（「カ」、未確定なし）を種にした形になり、確かめていない `k` と
        // 今の `i` が後ろに続く。応答の最終値（未確定 `k`）では作らない（末尾が二重になる）
        assert_eq!(
            state.journal,
            vec![
                ChangeRequest::SeedPreedit { text: "カ".into() },
                ChangeRequest::PushChar('k' as u32),
                ChangeRequest::PushChar('i' as u32),
            ]
        );
        assert_eq!(state.checkpoint, Some((3, reading("カき", ""))));
    }
    #[test]
    fn foreground_ownership_loss_replays_the_journal_and_resends_the_key_once() {
        let fake = rpc(
            vec![
                Ok(ChangeReply::Rejected(Reason::GenMismatch)),
                Ok(ChangeReply::Changed(ChangeOutcome::Bool {
                    value: true,
                    edit: reading("", ""),
                })),
            ],
            vec![Ok(restored(Replay::Exact, reading("", "t")))],
        );
        let mut state = RemoteComposition::new();
        state.generation = Some(generation());
        state.journal = vec![ChangeRequest::PushChar(116)];
        state.checkpoint = Some((1, reading("", "t")));
        apply_via(&fake, &mut state, ChangeRequest::Backspace, true).unwrap();
        let sent = fake.sent.borrow();
        assert!(matches!(
            &sent[0],
            Sent::Change(_, ChangeRequest::Backspace)
        ));
        assert!(matches!(&sent[1], Sent::Restore(_, journal, Some(_)) if journal.len() == 1));
        assert!(matches!(
            &sent[2],
            Sent::Change(_, ChangeRequest::Backspace)
        ));
        assert_eq!(state.checkpoint, Some((2, reading("", ""))));
    }
    #[test]
    fn unresolved_block_keeps_the_key_in_the_journal() {
        let mut state = RemoteComposition::new();
        state.generation = Some(generation());
        state.journal = vec![ChangeRequest::PushChar(116)];
        state.checkpoint = Some((1, edit()));
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
        assert_eq!(
            state.journal,
            vec![ChangeRequest::PushChar(116), ChangeRequest::PushChar(97)]
        );
        assert_eq!(state.confirmed_len(), 1);
        assert_eq!(state.edit, EditState::default());
    }
    #[test]
    fn bg_timer_cannot_create_ownership_and_irreversible_requests_are_not_journaled() {
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
            assert!(!may_acquire_ownership(&request));
            assert!(!is_journal_request(&request));
        }
    }
    #[test]
    fn key_lost_after_a_rebase_is_applied_exactly_once_by_the_next_recovery() {
        // 流し直しは Rebased で成功し、続く今のキー（`i`）の Change の応答を失う
        let fake = rpc(
            vec![
                Err(rakukan_engine_rpc::ChangeError::Transport(anyhow::anyhow!(
                    "disconnected"
                ))),
                changed(reading("カきあ", "")),
            ],
            vec![
                Ok(restored(Replay::Rebased, reading("カ", "k"))),
                Ok(restored(Replay::Exact, reading("カき", ""))),
            ],
        );
        let mut state = RemoteComposition::new();
        state.journal = vec![
            ChangeRequest::PushChar('k' as u32),
            ChangeRequest::PushChar('a' as u32),
            ChangeRequest::PushChar('k' as u32),
        ];
        state.checkpoint = Some((2, reading("カ", "")));
        assert!(apply_via(&fake, &mut state, ChangeRequest::PushChar('i' as u32), true).is_err());
        // 書き換えた先頭（種 + 確かめていない `k`）までが確認済み、`i` は未確認
        assert_eq!(state.confirmed_len(), 2);
        assert_eq!(state.journal.len(), 3);
        // 次のキー `a`: `i` までを流し直し、`a` を Change で送る。`i` は 1 回だけ流し直される
        apply_via(&fake, &mut state, ChangeRequest::PushChar('a' as u32), true).unwrap();
        let sent = fake.sent.borrow();
        let Sent::Restore(_, journal, Some(cp)) = &sent[2] else {
            panic!("second restore: {:?}", sent[2]);
        };
        assert_eq!(
            journal,
            &vec![
                ChangeRequest::SeedPreedit { text: "カ".into() },
                ChangeRequest::PushChar('k' as u32),
                ChangeRequest::PushChar('i' as u32),
            ]
        );
        assert_eq!(cp.len, 2);
        assert_eq!(
            journal
                .iter()
                .filter(|r| **r == ChangeRequest::PushChar('i' as u32))
                .count(),
            1
        );
        assert_eq!(state.checkpoint, Some((4, reading("カきあ", ""))));
    }
    #[test]
    fn after_a_full_recovery_the_next_key_is_a_plain_change() {
        let fake = rpc(
            vec![changed(reading("かき", ""))],
            vec![Ok(restored(Replay::Exact, reading("か", "k")))],
        );
        let mut state = RemoteComposition::new();
        state.journal = vec![
            ChangeRequest::PushChar('k' as u32),
            ChangeRequest::PushChar('a' as u32),
            ChangeRequest::PushChar('k' as u32),
        ];
        state.checkpoint = Some((1, reading("", "k")));
        // recover_on_input と同じく journal 全体を流し直す
        let upto = state.journal.len();
        restore_journal_via(&fake, &mut state, upto).unwrap();
        assert_eq!(state.confirmed_len(), 3);
        // 次のキーは流し直さない
        apply_via(&fake, &mut state, ChangeRequest::PushChar('i' as u32), true).unwrap();
        let sent = fake.sent.borrow();
        assert_eq!(sent.len(), 2);
        assert!(matches!(
            &sent[1],
            Sent::Change(_, ChangeRequest::PushChar(105))
        ));
    }
    #[test]
    fn composition_a_recovers_its_tail_after_b_cleared_the_ledger() {
        // A の `a` は応答を失って未確認。B のキーが流し直しで台帳を空にした後、A に戻ると
        // A の次のキーが `a` を流し直す（台帳を消しても A の journal に残っている）
        let fake = rpc(
            vec![
                Err(rakukan_engine_rpc::ChangeError::Transport(anyhow::anyhow!(
                    "disconnected"
                ))),
                changed(reading("", "s")),
                changed(reading("かk", "")),
            ],
            vec![
                Ok(restored(Replay::Exact, reading("", ""))),
                Ok(restored(Replay::Exact, reading("か", ""))),
            ],
        );
        let mut a = RemoteComposition::new();
        a.generation = Some(generation());
        a.journal = vec![ChangeRequest::PushChar('k' as u32)];
        a.checkpoint = Some((1, reading("", "k")));
        let mut b = RemoteComposition::new();
        assert!(apply_via(&fake, &mut a, ChangeRequest::PushChar('a' as u32), true).is_err());
        apply_via(&fake, &mut b, ChangeRequest::PushChar('s' as u32), true).unwrap();
        // A に戻る: A は所有権を失っている（B が所有者）ので、確かめていない `a` までを流し直す
        a.generation = None;
        apply_via(&fake, &mut a, ChangeRequest::PushChar('k' as u32), true).unwrap();
        let sent = fake.sent.borrow();
        assert!(matches!(
            &sent[3],
            Sent::Restore(owner, journal, Some(_)) if *owner == a.owner && journal.len() == 2
        ));
        assert_eq!(a.checkpoint, Some((3, reading("かk", ""))));
    }
    #[test]
    fn journal_cost_counts_text_bytes_below_the_count_limit() {
        // 件数が少なくても、文字列の大きさで費用が上限を超えうる
        let journal = vec![ChangeRequest::ForcePreedit {
            text: "あ".repeat(20_000),
        }];
        assert!(journal_cost(&journal) >= JOURNAL_SOFT_COST);
    }
    #[test]
    fn journal_records_input_without_starting_bg_conversion() {
        let entry = journal_entry(&ChangeRequest::InputChar {
            c: 'a' as u32,
            kind: InputCharKind::Char,
            bg_start_n_cands: Some(4),
        });
        assert_eq!(
            entry,
            ChangeRequest::InputChar {
                c: 'a' as u32,
                kind: InputCharKind::Char,
                bg_start_n_cands: None,
            }
        );
    }
    #[test]
    fn first_change_that_sends_nothing_is_not_saved() {
        // journal に入らない最初の要求: 何も送らずに失敗し、状態も残さない。
        let fake = rpc(vec![], vec![]);
        let mut state = RemoteComposition::new();
        let request = ChangeRequest::Commit {
            text: "text".into(),
        };
        assert!(apply_via(&fake, &mut state, request, true).is_err());
        assert!(fake.sent.borrow().is_empty());
        assert!(!keep_after_change(true, &state, false));
        // journal に入れた要求は、送れなくても保存する（次のキーで流し直す）。
        let fake = rpc(
            vec![],
            vec![Err(rakukan_engine_rpc::ChangeError::Transport(
                anyhow::anyhow!("disconnected"),
            ))],
        );
        let mut state = RemoteComposition::new();
        assert!(apply_via(&fake, &mut state, ChangeRequest::PushChar(116), true).is_err());
        assert_eq!(state.journal, vec![ChangeRequest::PushChar(116)]);
        assert!(keep_after_change(true, &state, false));
        // 既存の composition は、失敗しても保存する。
        assert!(keep_after_change(false, &RemoteComposition::new(), false));
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
