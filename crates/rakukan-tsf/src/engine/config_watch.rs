//! `config.toml` の変更を各 TSF プロセスが自分で検出する（Issue #65）。
//!
//! 設定アプリの保存通知（名前付きイベント `Local\rakukan.engine.reload`）は auto-reset で、
//! 1 回の `SetEvent` で起きるのは待機中の 1 プロセスだけ。ここでは、それに加えて
//! `%APPDATA%\rakukan` のディレクトリ変更通知と定期確認を組み合わせ、通知を取りこぼしても
//! 定期確認で追いつくようにする。
//!
//! - ディレクトリ変更通知は各プロセスが独立に登録し、通知のたびに
//!   `FindNextChangeNotification` で再設定する。配送の保証ではない
//! - 変更の判定は本文の比較で行う（`config::reload_config`）。mtime・size で読み取りを省略しない
//! - 期限は 3 つ: デバウンス（最後の通知から）、連続通知の最大待ち（最初の通知から）、
//!   定期確認（最後の読込確認の完了から。通知では延ばさない）
//! - 読込からエンジンへの RPC は呼ばない。保存イベントを受けていたときだけ、読込後に
//!   `state::engine_reload()` へ反映待ちの処理を依頼する（監視スレッドは RPC の完了を待たない）
//! - 監視ディレクトリの不在やハンドルの作成・再設定失敗でも定期確認を続け、その機会に
//!   監視の復旧を試みる

use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// 連続通知をまとめる待ち（最後の通知から）。
pub const DEBOUNCE_MS: u64 = 300;
/// 通知が途切れなくても読む上限（最初の通知から）。
pub const MAX_BURST_MS: u64 = 2_000;
/// 定期確認の間隔（最後の読込確認の完了から）。
pub const PERIODIC_MS: u64 = 30_000;

/// 読込の契機と期限を管理する（Win32 に依存しない純粋な部分）。
#[derive(Debug)]
pub struct WatchScheduler {
    /// 次の定期確認時刻。読込確認の完了後から `PERIODIC_MS`。通知では延ばさない
    next_periodic_at: u64,
    /// 連続通知の開始時刻（最大待ちの基準）。要求が無ければ `None`
    burst_start_at: Option<u64>,
    /// 最後の通知時刻（デバウンスの基準）
    last_notify_at: Option<u64>,
    /// 保存イベント（エンジン反映を求める契機）を受けた。読込後に処理するまで保持する
    save_event: bool,
}

/// 期限が来た読込の内容。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadJob {
    /// この読込の前に保存イベントを受けていた（読込後に反映待ちを処理する）
    pub save_event: bool,
    /// ログ用: 通知によるものか定期確認か
    pub reason: &'static str,
}

impl WatchScheduler {
    pub fn new(now_ms: u64) -> Self {
        Self {
            next_periodic_at: now_ms.saturating_add(PERIODIC_MS),
            burst_start_at: None,
            last_notify_at: None,
            save_event: false,
        }
    }

    /// ディレクトリ変更通知、または背景読込の要求を受けた。
    pub fn on_notify(&mut self, now_ms: u64) {
        if self.burst_start_at.is_none() {
            self.burst_start_at = Some(now_ms);
        }
        self.last_notify_at = Some(now_ms);
    }

    /// 名前付きイベント（設定アプリの保存・トレイ操作）を受けた。
    /// 読込要求に加えて、読込後にエンジンへの反映待ちを処理する契機として記録する。
    pub fn on_save_event(&mut self, now_ms: u64) {
        self.save_event = true;
        self.on_notify(now_ms);
    }

    /// 要求があるときの読込期限（デバウンスと最大待ちの早い方）。
    fn request_deadline(&self) -> Option<u64> {
        match (self.burst_start_at, self.last_notify_at) {
            (Some(burst), Some(last)) => Some(
                last.saturating_add(DEBOUNCE_MS)
                    .min(burst.saturating_add(MAX_BURST_MS)),
            ),
            _ => None,
        }
    }

    /// 次に読むべき時刻（絶対時刻、ms）。
    pub fn next_deadline(&self) -> u64 {
        match self.request_deadline() {
            Some(d) => d.min(self.next_periodic_at),
            None => self.next_periodic_at,
        }
    }

    /// 次の期限までの待ち時間。
    pub fn wait_ms(&self, now_ms: u64) -> u64 {
        self.next_deadline().saturating_sub(now_ms)
    }

    /// 期限が来ていれば読込の内容を返し、要求と契機を取り出す。
    ///
    /// 要求はここで消すので、読込中に届いた通知は次の `on_notify` で新しい要求になる
    /// （取り落とさない）。保存イベントも同様に、読込後に処理するまで残る。
    pub fn take_due(&mut self, now_ms: u64) -> Option<ReadJob> {
        let request_due = self.request_deadline().is_some_and(|d| now_ms >= d);
        let periodic_due = now_ms >= self.next_periodic_at;
        if !request_due && !periodic_due {
            return None;
        }
        self.burst_start_at = None;
        self.last_notify_at = None;
        let save_event = std::mem::take(&mut self.save_event);
        Some(ReadJob {
            save_event,
            reason: if request_due { "notify" } else { "periodic" },
        })
    }

    /// 読込確認が終わった（成功・失敗を問わない）。定期確認の期限をここから数え直す。
    /// 読込失敗時にも次の確認時刻を置くので、期限超過による連続再試行にならない。
    pub fn on_read_done(&mut self, now_ms: u64) {
        self.next_periodic_at = now_ms.saturating_add(PERIODIC_MS);
    }

    #[cfg(test)]
    fn has_request(&self) -> bool {
        self.burst_start_at.is_some()
    }
}

/// 監視に使う入力源。ハンドルが作れなかった源は待たず、定期確認で作り直しを試みる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchSource {
    /// 名前付きイベント（設定アプリの保存・トレイ操作）
    SaveEvent,
    /// 背景読込の要求（候補表示など）
    Request,
    /// ディレクトリ変更通知
    DirChange,
}

/// 今そろっている入力源の並び。`WaitForMultipleObjects` に渡す順と、起きた番号の対応を 1 か所で決める。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitSet {
    order: Vec<WatchSource>,
}

impl WaitSet {
    pub fn new(save_event: bool, request: bool, dir_change: bool) -> Self {
        let mut order = Vec::new();
        if save_event {
            order.push(WatchSource::SaveEvent);
        }
        if request {
            order.push(WatchSource::Request);
        }
        if dir_change {
            order.push(WatchSource::DirChange);
        }
        Self { order }
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// 起きたハンドルの番号（`WAIT_OBJECT_0` からの差）を入力源に戻す。範囲外は `None`。
    pub fn source_at(&self, index: u32) -> Option<WatchSource> {
        self.order.get(index as usize).copied()
    }

    pub fn sources(&self) -> &[WatchSource] {
        &self.order
    }
}

static CLOCK: LazyLock<Instant> = LazyLock::new(Instant::now);

fn now_ms() -> u64 {
    CLOCK.elapsed().as_millis() as u64
}

/// 背景読込の要求用イベント（無名、auto-reset）。0 = 未作成。
static REQUEST_EVENT: AtomicUsize = AtomicUsize::new(0);

/// 候補表示などの経路から、背景での読込を要求する。同期読込はしない。
/// 監視スレッドがまだ無ければ何もしない（次の定期確認で拾われる）。
pub fn request_reload() {
    #[cfg(windows)]
    {
        let h = REQUEST_EVENT.load(Ordering::Acquire);
        if h != 0 {
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::System::Threading::SetEvent;
            let _ = unsafe { SetEvent(HANDLE(h as *mut core::ffi::c_void)) };
        }
    }
}

/// 監視スレッドを起動する（DllMain から 1 回）。
pub fn start_watcher() {
    #[cfg(windows)]
    {
        let _ = std::thread::Builder::new()
            .name("rakukan-config-watch".into())
            .spawn(win32::run_loop);
    }
}

#[cfg(windows)]
mod win32 {
    use super::{REQUEST_EVENT, WatchScheduler, now_ms};
    use std::sync::atomic::Ordering;
    use windows::Win32::Foundation::{
        HANDLE, INVALID_HANDLE_VALUE, WAIT_EVENT, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows::Win32::Storage::FileSystem::{
        FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE, FILE_NOTIFY_CHANGE_SIZE,
        FindCloseChangeNotification, FindFirstChangeNotificationW, FindNextChangeNotification,
    };
    use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects};
    use windows::core::PCWSTR;

    const RELOAD_EVENT_NAME: &str = "Local\\rakukan.engine.reload";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 監視するディレクトリ（`config.toml` のある場所）。
    fn watch_dir() -> Option<std::path::PathBuf> {
        super::super::config::config_path()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    }

    /// ディレクトリ変更通知を登録する。失敗は `None`（呼び出し側が定期確認で復旧を試みる）。
    fn register_dir_watch(log_failure: bool) -> Option<HANDLE> {
        let dir = watch_dir()?;
        let name = wide(&dir.to_string_lossy());
        let r = unsafe {
            FindFirstChangeNotificationW(
                PCWSTR(name.as_ptr()),
                false,
                FILE_NOTIFY_CHANGE_LAST_WRITE
                    | FILE_NOTIFY_CHANGE_SIZE
                    | FILE_NOTIFY_CHANGE_FILE_NAME,
            )
        };
        match r {
            Ok(h) if h != INVALID_HANDLE_VALUE && !h.0.is_null() => {
                tracing::info!("config_watch: watching {}", dir.display());
                Some(h)
            }
            Ok(_) => {
                if log_failure {
                    tracing::warn!(
                        "config_watch: FindFirstChangeNotificationW returned an invalid handle for {}; periodic check only",
                        dir.display()
                    );
                }
                None
            }
            Err(e) => {
                if log_failure {
                    tracing::warn!(
                        "config_watch: FindFirstChangeNotificationW failed for {}: {e}; periodic check only",
                        dir.display()
                    );
                }
                None
            }
        }
    }

    fn close_dir_watch(h: HANDLE) {
        let _ = unsafe { FindCloseChangeNotification(h) };
    }

    /// イベントを作る。失敗しても監視は続け、定期確認のたびに作り直しを試みる。
    fn create_event(named: Option<&str>, log_failure: bool) -> Option<HANDLE> {
        let name = named.map(wide);
        let pname = name
            .as_ref()
            .map(|n| PCWSTR(n.as_ptr()))
            .unwrap_or(PCWSTR::null());
        match unsafe { CreateEventW(None, false, false, pname) } {
            Ok(h) if !h.0.is_null() => Some(h),
            Ok(_) => {
                if log_failure {
                    tracing::warn!(
                        "config_watch: CreateEventW({}) returned a null handle; continuing with the remaining sources",
                        named.unwrap_or("<request>")
                    );
                }
                None
            }
            Err(e) => {
                if log_failure {
                    tracing::warn!(
                        "config_watch: CreateEventW({}) failed: {e}; continuing with the remaining sources",
                        named.unwrap_or("<request>")
                    );
                }
                None
            }
        }
    }

    pub(super) fn run_loop() {
        // どの入力源も、作れなくても監視を止めない。定期確認は常に動き、
        // 読込のたびに欠けている源の作り直しを試みる。
        let mut named = create_event(Some(RELOAD_EVENT_NAME), true);
        let mut request = create_event(None, true);
        if let Some(h) = request {
            REQUEST_EVENT.store(h.0 as usize, Ordering::Release);
        }
        let mut dir = register_dir_watch(true);
        // 失敗のログは状態が変わったときだけ（作り直しの試行ごとに繰り返さない）
        let mut logged_missing = (named.is_none(), request.is_none(), dir.is_none());

        let mut sched = WatchScheduler::new(now_ms());
        tracing::info!(
            "config_watch: sources save_event={} request={} dir_watch={}, periodic={}ms",
            named.is_some(),
            request.is_some(),
            dir.is_some(),
            super::PERIODIC_MS
        );

        loop {
            let set = super::WaitSet::new(named.is_some(), request.is_some(), dir.is_some());
            let handles: Vec<HANDLE> = set
                .sources()
                .iter()
                .map(|s| match s {
                    super::WatchSource::SaveEvent => named.expect("present"),
                    super::WatchSource::Request => request.expect("present"),
                    super::WatchSource::DirChange => dir.expect("present"),
                })
                .collect();
            let wait = sched.wait_ms(now_ms()).min(u32::MAX as u64 - 1) as u32;

            if set.is_empty() {
                // 待てる源が無い: 定期確認だけで動き、その機会に作り直す
                std::thread::sleep(std::time::Duration::from_millis(wait as u64));
            } else {
                let r = unsafe { WaitForMultipleObjects(&handles, false, wait) };
                let now = now_ms();
                match r {
                    WAIT_TIMEOUT => {}
                    WAIT_FAILED => {
                        tracing::error!("config_watch: WaitForMultipleObjects failed ({:?})", r);
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                    WAIT_EVENT(i) => match set.source_at(i.wrapping_sub(WAIT_OBJECT_0.0)) {
                        Some(super::WatchSource::SaveEvent) => {
                            tracing::info!("config_watch: reload event received");
                            sched.on_save_event(now);
                        }
                        Some(super::WatchSource::Request) => sched.on_notify(now),
                        Some(super::WatchSource::DirChange) => {
                            sched.on_notify(now);
                            if let Some(d) = dir
                                && let Err(e) = unsafe { FindNextChangeNotification(d) }
                            {
                                tracing::warn!(
                                    "config_watch: FindNextChangeNotification failed: {e}; will re-register at the next check"
                                );
                                close_dir_watch(d);
                                dir = None;
                                logged_missing.2 = true;
                            }
                        }
                        None => {
                            // WAIT_ABANDONED_0 など。イベントには起きないが、念のため記録して続行
                            tracing::warn!("config_watch: unexpected wait result {i}");
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    },
                }
            }

            let now = now_ms();
            if let Some(job) = sched.take_due(now) {
                let outcome = super::super::config::reload_config(job.reason);
                sched.on_read_done(now_ms());
                tracing::debug!(
                    "config_watch: read ({}) -> {:?} save_event={}",
                    job.reason,
                    outcome,
                    job.save_event
                );
                // 欠けている源の作り直し。ログは状態が変わったときだけ
                if named.is_none() {
                    named = create_event(Some(RELOAD_EVENT_NAME), !logged_missing.0);
                    logged_missing.0 = named.is_none();
                }
                if request.is_none() {
                    request = create_event(None, !logged_missing.1);
                    if let Some(h) = request {
                        REQUEST_EVENT.store(h.0 as usize, Ordering::Release);
                    }
                    logged_missing.1 = request.is_none();
                }
                if dir.is_none() {
                    dir = register_dir_watch(!logged_missing.2);
                    logged_missing.2 = dir.is_none();
                }
                if job.save_event {
                    // 反映待ちの処理はエンジン処理側に依頼する（ラッチのリセットも含む）。
                    // engine_reload は自前のスレッドで動くので、ここでは RPC を待たない
                    super::super::state::engine_reload();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_only_when_idle() {
        let mut s = WatchScheduler::new(1_000);
        assert_eq!(s.next_deadline(), 1_000 + PERIODIC_MS);
        assert!(s.take_due(1_000 + PERIODIC_MS - 1).is_none());
        let job = s.take_due(1_000 + PERIODIC_MS).expect("periodic due");
        assert_eq!(job.reason, "periodic");
        assert!(!job.save_event);
    }

    #[test]
    fn debounce_waits_for_quiet_period() {
        let mut s = WatchScheduler::new(0);
        s.on_notify(1_000);
        assert_eq!(s.next_deadline(), 1_000 + DEBOUNCE_MS);
        // 300ms 以内の追加通知でデバウンスが延びる
        s.on_notify(1_200);
        assert_eq!(s.next_deadline(), 1_200 + DEBOUNCE_MS);
        assert!(s.take_due(1_499).is_none());
        let job = s.take_due(1_500).expect("debounce due");
        assert_eq!(job.reason, "notify");
        assert!(!s.has_request(), "読込で要求は消える");
    }

    #[test]
    fn burst_is_capped_by_max_wait() {
        // 通知が途切れなくても、最初の通知から 2 秒で読む
        let mut s = WatchScheduler::new(0);
        let mut t = 1_000;
        s.on_notify(t);
        while t < 1_000 + MAX_BURST_MS {
            t += 100;
            s.on_notify(t);
            assert!(s.next_deadline() <= 1_000 + MAX_BURST_MS);
        }
        assert!(s.take_due(1_000 + MAX_BURST_MS).is_some());
    }

    #[test]
    fn notifications_do_not_extend_periodic_deadline() {
        let mut s = WatchScheduler::new(0);
        let periodic = s.next_periodic_at;
        for t in (100..PERIODIC_MS).step_by(100) {
            s.on_notify(t);
            // 読まずに通知だけ続いても、定期確認の期限は動かない
            assert_eq!(s.next_periodic_at, periodic);
        }
    }

    #[test]
    fn read_done_restarts_periodic_from_completion() {
        let mut s = WatchScheduler::new(0);
        s.on_notify(500);
        assert!(s.take_due(800).is_some());
        s.on_read_done(900);
        assert_eq!(s.next_periodic_at, 900 + PERIODIC_MS);
        // 失敗した読込でも同じ（呼び出し側が on_read_done を呼ぶ）
        s.on_read_done(1_000);
        assert_eq!(s.next_periodic_at, 1_000 + PERIODIC_MS);
    }

    #[test]
    fn notify_during_read_becomes_a_new_request() {
        let mut s = WatchScheduler::new(0);
        s.on_notify(100);
        assert!(s.take_due(400).is_some());
        // 読込中（take_due の後、on_read_done の前）に届いた通知
        s.on_notify(450);
        s.on_read_done(500);
        assert!(s.has_request(), "読込中の通知を取り落とさない");
        assert_eq!(s.next_deadline(), 450 + DEBOUNCE_MS);
    }

    #[test]
    fn save_event_is_kept_until_a_read_takes_it() {
        let mut s = WatchScheduler::new(0);
        s.on_save_event(100);
        // 保存イベントの直後に別の通知が来ても、契機は残る
        s.on_notify(200);
        let job = s.take_due(500).expect("due");
        assert!(job.save_event);
        // 取り出した後は消えている
        s.on_notify(600);
        let job = s.take_due(900).expect("due");
        assert!(!job.save_event);
    }

    #[test]
    fn save_event_during_read_is_processed_by_the_next_read() {
        let mut s = WatchScheduler::new(0);
        s.on_notify(100);
        let first = s.take_due(400).expect("due");
        assert!(!first.save_event);
        // 読込中に保存イベント（背景読込より先にイベントが届く順序の逆も含め、捨てない）
        s.on_save_event(450);
        s.on_read_done(500);
        let second = s.take_due(750).expect("due");
        assert!(second.save_event, "読込中の保存イベントを捨てない");
    }

    #[test]
    fn wait_set_maps_signaled_index_to_the_present_sources_only() {
        // 全部そろっているとき
        let all = WaitSet::new(true, true, true);
        assert_eq!(all.len(), 3);
        assert_eq!(all.source_at(0), Some(WatchSource::SaveEvent));
        assert_eq!(all.source_at(1), Some(WatchSource::Request));
        assert_eq!(all.source_at(2), Some(WatchSource::DirChange));
        assert_eq!(all.source_at(3), None);
        // 名前付きイベントが作れなかったとき: 番号 0 は要求イベント
        let no_named = WaitSet::new(false, true, true);
        assert_eq!(no_named.len(), 2);
        assert_eq!(no_named.source_at(0), Some(WatchSource::Request));
        assert_eq!(no_named.source_at(1), Some(WatchSource::DirChange));
        // 要求イベントとディレクトリ監視が無いとき: 番号 0 は名前付きイベント
        let only_named = WaitSet::new(true, false, false);
        assert_eq!(only_named.source_at(0), Some(WatchSource::SaveEvent));
        assert_eq!(only_named.source_at(1), None);
        // 何も作れなかったとき: 待つ源は無い（定期確認だけで動く）
        let none = WaitSet::new(false, false, false);
        assert!(none.is_empty());
        assert_eq!(none.source_at(0), None);
    }

    #[test]
    fn periodic_read_while_request_pending_uses_the_earlier_deadline() {
        let mut s = WatchScheduler::new(0);
        // 定期確認の直前に通知が来た: 早い方（定期確認）で読み、要求も消費する
        s.on_notify(PERIODIC_MS - 100);
        assert_eq!(s.next_deadline(), PERIODIC_MS);
        let job = s.take_due(PERIODIC_MS).expect("due");
        assert_eq!(job.reason, "periodic");
        assert!(!s.has_request());
    }
}
