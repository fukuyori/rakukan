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

#[cfg(feature = "config-watch-fault-test")]
#[path = "config_watch_fault.rs"]
mod fault_control;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FaultFlags {
    suppress_notifications: bool,
    fail_save_event: bool,
    fail_request_event: bool,
    fail_dir_watch: bool,
    fail_rearm_once: bool,
}

/// 連続通知をまとめる待ち（最後の通知から）。
pub const DEBOUNCE_MS: u64 = 300;
/// 通知が途切れなくても読む上限（最初の通知から）。
pub const MAX_BURST_MS: u64 = 2_000;
/// 定期確認の間隔（最後の読込確認の完了から）。
pub const PERIODIC_MS: u64 = 30_000;

/// 読込の契機と期限を管理する（Win32 に依存しない純粋な部分）。
#[derive(Debug)]
pub struct WatchScheduler {
    periodic_ms: u64,
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
    #[cfg(test)]
    pub fn new(now_ms: u64) -> Self {
        Self::with_periodic(now_ms, PERIODIC_MS)
    }

    fn with_periodic(now_ms: u64, periodic_ms: u64) -> Self {
        Self {
            periodic_ms,
            next_periodic_at: now_ms.saturating_add(periodic_ms),
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
        self.next_periodic_at = now_ms.saturating_add(self.periodic_ms);
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

    #[cfg(test)]
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
    use super::{FaultFlags, REQUEST_EVENT, WatchScheduler, now_ms};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use windows::Win32::Foundation::CloseHandle;
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
    fn register_dir_watch(dir: Option<&Path>, log_failure: bool) -> Option<HANDLE> {
        let dir = dir?;
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

    struct LoopOptions {
        dir: Option<PathBuf>,
        event_name: String,
        stop: Option<usize>,
        publish_request: bool,
        periodic_ms: u64,
        control_poll_ms: Option<u64>,
    }

    fn retry_missing(
        options: &LoopOptions,
        faults: FaultFlags,
        named: &mut Option<HANDLE>,
        request: &mut Option<HANDLE>,
        dir: &mut Option<HANDLE>,
        logged_missing: &mut (bool, bool, bool),
    ) {
        let before = (named.is_some(), request.is_some(), dir.is_some());
        if named.is_none() && !faults.fail_save_event {
            *named = create_event(Some(&options.event_name), !logged_missing.0);
            logged_missing.0 = named.is_none();
        }
        if request.is_none() && !faults.fail_request_event {
            *request = create_event(None, !logged_missing.1);
            if options.publish_request
                && let Some(h) = request
            {
                REQUEST_EVENT.store(h.0 as usize, Ordering::Release);
            }
            logged_missing.1 = request.is_none();
        }
        if dir.is_none() && !faults.fail_dir_watch {
            *dir = register_dir_watch(options.dir.as_deref(), !logged_missing.2);
            logged_missing.2 = dir.is_none();
        }
        let after = (named.is_some(), request.is_some(), dir.is_some());
        if before != after {
            tracing::info!(
                "config_watch: sources save_event={} request={} dir_watch={}, periodic={}ms",
                after.0,
                after.1,
                after.2,
                options.periodic_ms
            );
        }
    }

    pub(super) fn run_loop() {
        #[cfg(feature = "config-watch-fault-test")]
        let mut control = super::fault_control::FileFaultControl::from_env();
        #[cfg(feature = "config-watch-fault-test")]
        let poll_ms = control.as_ref().map(|_| 1_000);
        #[cfg(not(feature = "config-watch-fault-test"))]
        let poll_ms = None;
        tracing::info!(
            "config_watch: build_kind={} pid={} module={}",
            if cfg!(feature = "config-watch-fault-test") {
                "fault-test"
            } else {
                "normal"
            },
            std::process::id(),
            crate::globals::DllModule::get_path().unwrap_or_else(|e| format!("unavailable: {e}"))
        );
        let options = LoopOptions {
            dir: watch_dir(),
            event_name: RELOAD_EVENT_NAME.to_owned(),
            stop: None,
            publish_request: true,
            periodic_ms: super::PERIODIC_MS,
            control_poll_ms: poll_ms,
        };
        run_loop_with(
            options,
            |reason| format!("{:?}", super::super::config::reload_config(reason)),
            super::super::state::engine_reload,
            move || {
                #[cfg(feature = "config-watch-fault-test")]
                {
                    control
                        .as_mut()
                        .map_or_else(FaultFlags::default, |c| c.poll())
                }
                #[cfg(not(feature = "config-watch-fault-test"))]
                {
                    FaultFlags::default()
                }
            },
        );
    }

    fn run_loop_with<R, A, C>(options: LoopOptions, mut read: R, mut apply: A, mut control: C)
    where
        R: FnMut(&'static str) -> String,
        A: FnMut(),
        C: FnMut() -> FaultFlags,
    {
        let mut faults = control();
        let mut named = (!faults.fail_save_event)
            .then(|| create_event(Some(&options.event_name), true))
            .flatten();
        let mut request = (!faults.fail_request_event)
            .then(|| create_event(None, true))
            .flatten();
        if options.publish_request
            && let Some(h) = request
        {
            REQUEST_EVENT.store(h.0 as usize, Ordering::Release);
        }
        let mut dir = (!faults.fail_dir_watch)
            .then(|| register_dir_watch(options.dir.as_deref(), true))
            .flatten();
        let mut logged_missing = (named.is_none(), request.is_none(), dir.is_none());
        let mut rearm_failed_once = false;
        let mut sched = WatchScheduler::with_periodic(now_ms(), options.periodic_ms);
        tracing::info!(
            "config_watch: sources save_event={} request={} dir_watch={}, periodic={}ms",
            named.is_some(),
            request.is_some(),
            dir.is_some(),
            options.periodic_ms
        );

        loop {
            let set = super::WaitSet::new(named.is_some(), request.is_some(), dir.is_some());
            let mut handles: Vec<HANDLE> = set
                .sources()
                .iter()
                .map(|s| match s {
                    super::WatchSource::SaveEvent => named.expect("present"),
                    super::WatchSource::Request => request.expect("present"),
                    super::WatchSource::DirChange => dir.expect("present"),
                })
                .collect();
            let stop_offset = if let Some(raw) = options.stop {
                handles.insert(0, HANDLE(raw as *mut core::ffi::c_void));
                1
            } else {
                0
            };
            let wait = sched
                .wait_ms(now_ms())
                .min(options.control_poll_ms.unwrap_or(u64::MAX))
                .min(u32::MAX as u64 - 1) as u32;
            if handles.is_empty() {
                std::thread::sleep(std::time::Duration::from_millis(wait as u64));
            } else {
                let result = unsafe { WaitForMultipleObjects(&handles, false, wait) };
                let now = now_ms();
                match result {
                    WAIT_TIMEOUT => {}
                    WAIT_FAILED => {
                        tracing::error!("config_watch: WaitForMultipleObjects failed ({result:?})");
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                    WAIT_EVENT(i) => {
                        let index = i.wrapping_sub(WAIT_OBJECT_0.0);
                        if stop_offset == 1 && index == 0 {
                            break;
                        }
                        match set.source_at(index.saturating_sub(stop_offset)) {
                            Some(super::WatchSource::SaveEvent)
                                if !faults.suppress_notifications =>
                            {
                                tracing::info!("config_watch: reload event received");
                                sched.on_save_event(now);
                            }
                            Some(super::WatchSource::Request) if !faults.suppress_notifications => {
                                sched.on_notify(now)
                            }
                            Some(super::WatchSource::DirChange) => {
                                if !faults.suppress_notifications {
                                    sched.on_notify(now);
                                }
                                if let Some(d) = dir {
                                    let rearm = if faults.fail_rearm_once && !rearm_failed_once {
                                        rearm_failed_once = true;
                                        Err(windows::core::Error::new(
                                            windows::Win32::Foundation::E_FAIL,
                                            "injected rearm failure",
                                        ))
                                    } else {
                                        unsafe { FindNextChangeNotification(d) }
                                    };
                                    if let Err(e) = rearm {
                                        tracing::warn!(
                                            "config_watch: FindNextChangeNotification failed: {e}; will re-register at the next check"
                                        );
                                        close_dir_watch(d);
                                        dir = None;
                                        logged_missing.2 = true;
                                    }
                                }
                            }
                            Some(_) => {}
                            None => {
                                tracing::warn!("config_watch: unexpected wait result {i}");
                                std::thread::sleep(std::time::Duration::from_millis(100));
                            }
                        }
                    }
                }
            }
            let previous = faults;
            faults = control();
            if previous != FaultFlags::default() && faults == FaultFlags::default() {
                retry_missing(
                    &options,
                    faults,
                    &mut named,
                    &mut request,
                    &mut dir,
                    &mut logged_missing,
                );
            }
            if let Some(job) = sched.take_due(now_ms()) {
                let outcome = read(job.reason);
                sched.on_read_done(now_ms());
                tracing::debug!(
                    "config_watch: read ({}) -> {} save_event={}",
                    job.reason,
                    outcome,
                    job.save_event
                );
                retry_missing(
                    &options,
                    faults,
                    &mut named,
                    &mut request,
                    &mut dir,
                    &mut logged_missing,
                );
                if job.save_event {
                    apply();
                }
            }
        }
        if let Some(h) = dir {
            close_dir_watch(h);
        }
        if let Some(h) = request {
            let _ = unsafe { CloseHandle(h) };
        }
        if let Some(h) = named {
            let _ = unsafe { CloseHandle(h) };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::{Arc, Mutex, mpsc};
        use std::time::Duration;
        use windows::Win32::System::Threading::SetEvent;

        struct TestRun {
            stop: HANDLE,
            thread: std::thread::JoinHandle<()>,
            reads: mpsc::Receiver<(&'static str, String)>,
            applies: mpsc::Receiver<()>,
        }

        fn fixture(tag: &str) -> (PathBuf, PathBuf, String) {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir()
                .join(format!("rakukan-watch-{tag}-{}-{id}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("test directory");
            let path = dir.join("config.toml");
            std::fs::write(&path, "A").expect("initial config");
            let event = format!("Local\\rakukan.watch.test.{}.{id}", std::process::id());
            (dir, path, event)
        }

        fn start(
            dir: PathBuf,
            path: PathBuf,
            event_name: String,
            periodic_ms: u64,
            faults: Arc<Mutex<FaultFlags>>,
        ) -> TestRun {
            let stop = create_event(None, true).expect("stop event");
            let stop_raw = stop.0 as usize;
            let (reads_tx, reads_rx) = mpsc::channel();
            let (apply_tx, apply_rx) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                run_loop_with(
                    LoopOptions {
                        dir: Some(dir),
                        event_name,
                        stop: Some(stop_raw),
                        publish_request: false,
                        periodic_ms,
                        control_poll_ms: Some(20),
                    },
                    move |reason| {
                        let body = std::fs::read_to_string(&path).expect("read config");
                        reads_tx.send((reason, body)).expect("send read");
                        "ok".to_owned()
                    },
                    move || {
                        apply_tx.send(()).expect("send apply");
                    },
                    move || *faults.lock().expect("fault lock"),
                );
            });
            TestRun {
                stop,
                thread,
                reads: reads_rx,
                applies: apply_rx,
            }
        }

        fn stop(run: TestRun, dir: PathBuf) {
            unsafe { SetEvent(run.stop) }.expect("signal stop");
            run.thread.join().expect("watch thread");
            let _ = unsafe { CloseHandle(run.stop) };
            std::fs::remove_dir_all(dir).expect("remove test directory");
        }

        #[test]
        fn real_wait_loop_reads_periodically_without_notifications_then_recovers() {
            let (dir, path, event_name) = fixture("suppressed");
            let faults = Arc::new(Mutex::new(FaultFlags {
                suppress_notifications: true,
                ..Default::default()
            }));
            let run = start(dir.clone(), path.clone(), event_name, 700, faults.clone());
            std::thread::sleep(Duration::from_millis(80));
            std::fs::write(&path, "B").expect("replace config");
            let first = run
                .reads
                .recv_timeout(Duration::from_secs(3))
                .expect("periodic read");
            assert_eq!(first, ("periodic", "B".to_owned()));

            *faults.lock().expect("fault lock") = FaultFlags::default();
            std::thread::sleep(Duration::from_millis(80));
            std::fs::write(&path, "C").expect("replace config again");
            let mut notified = false;
            for _ in 0..4 {
                let (reason, body) = run
                    .reads
                    .recv_timeout(Duration::from_secs(2))
                    .expect("next read");
                if reason == "notify" && body == "C" {
                    notified = true;
                    break;
                }
            }
            assert!(notified, "directory notification did not resume");
            stop(run, dir);
        }

        #[test]
        fn real_wait_loop_survives_all_source_creation_failures_and_recreates_them() {
            let (dir, path, event_name) = fixture("sources");
            let event = create_event(Some(&event_name), true).expect("named event");
            let faults = Arc::new(Mutex::new(FaultFlags {
                fail_save_event: true,
                fail_request_event: true,
                fail_dir_watch: true,
                ..Default::default()
            }));
            let run = start(dir.clone(), path, event_name, 600, faults.clone());
            assert_eq!(
                run.reads
                    .recv_timeout(Duration::from_secs(3))
                    .expect("periodic read")
                    .0,
                "periodic"
            );
            *faults.lock().expect("fault lock") = FaultFlags::default();
            std::thread::sleep(Duration::from_millis(100));
            unsafe { SetEvent(event) }.expect("signal save event");
            run.applies
                .recv_timeout(Duration::from_secs(3))
                .expect("apply after recovery");
            stop(run, dir);
            let _ = unsafe { CloseHandle(event) };
        }

        #[test]
        fn real_wait_loop_rearms_after_one_notification_registration_failure() {
            let (dir, path, event_name) = fixture("rearm");
            let faults = Arc::new(Mutex::new(FaultFlags {
                fail_rearm_once: true,
                ..Default::default()
            }));
            let run = start(dir.clone(), path.clone(), event_name, 2_000, faults);
            std::thread::sleep(Duration::from_millis(80));
            std::fs::write(&path, "B").expect("first change");
            assert_eq!(
                run.reads
                    .recv_timeout(Duration::from_secs(2))
                    .expect("first read"),
                ("notify", "B".to_owned())
            );
            std::thread::sleep(Duration::from_millis(80));
            std::fs::write(&path, "C").expect("second change");
            assert_eq!(
                run.reads
                    .recv_timeout(Duration::from_secs(2))
                    .expect("second read"),
                ("notify", "C".to_owned())
            );
            stop(run, dir);
        }

        #[test]
        fn real_wait_loop_keeps_a_save_event_received_during_read() {
            let (dir, path, event_name) = fixture("save-during-read");
            let event = create_event(Some(&event_name), true).expect("named event");
            let stop_event = create_event(None, true).expect("stop event");
            let stop_raw = stop_event.0 as usize;
            let (entered_tx, entered_rx) = mpsc::channel();
            let (resume_tx, resume_rx) = mpsc::channel();
            let (reads_tx, reads_rx) = mpsc::channel();
            let (apply_tx, apply_rx) = mpsc::channel();
            let watch_dir = dir.clone();
            let thread = std::thread::spawn(move || {
                let mut reads = 0;
                run_loop_with(
                    LoopOptions {
                        dir: Some(watch_dir),
                        event_name,
                        stop: Some(stop_raw),
                        publish_request: false,
                        periodic_ms: 2_000,
                        control_poll_ms: None,
                    },
                    move |reason| {
                        if reads == 0 {
                            entered_tx.send(()).expect("entered read");
                            resume_rx
                                .recv_timeout(Duration::from_secs(3))
                                .expect("resume read");
                        }
                        reads += 1;
                        reads_tx.send(reason).expect("send reason");
                        "ok".to_owned()
                    },
                    move || {
                        apply_tx.send(()).expect("send apply");
                    },
                    FaultFlags::default,
                );
            });
            std::thread::sleep(Duration::from_millis(80));
            std::fs::write(&path, "B").expect("change config");
            entered_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("read started");
            unsafe { SetEvent(event) }.expect("signal during read");
            resume_tx.send(()).expect("finish first read");
            assert_eq!(
                reads_rx
                    .recv_timeout(Duration::from_secs(2))
                    .expect("first read"),
                "notify"
            );
            assert_eq!(
                reads_rx
                    .recv_timeout(Duration::from_secs(2))
                    .expect("second read"),
                "notify"
            );
            apply_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("apply after second read");
            assert!(
                apply_rx.recv_timeout(Duration::from_millis(100)).is_err(),
                "save event applied twice"
            );
            unsafe { SetEvent(stop_event) }.expect("signal stop");
            thread.join().expect("watch thread");
            let _ = unsafe { CloseHandle(event) };
            let _ = unsafe { CloseHandle(stop_event) };
            std::fs::remove_dir_all(dir).expect("remove test directory");
        }

        #[test]
        fn real_wait_loop_keeps_last_good_config_after_parse_failure_and_recovers() {
            use crate::engine::config::{ConfigManager, LoadOutcome};
            let (dir, path, event_name) = fixture("parse-recovery");
            std::fs::write(&path, "[conversion]\nnum_candidates = 7\n").expect("valid config");
            let mut manager = ConfigManager::from_path(path.clone());
            let stop_event = create_event(None, true).expect("stop event");
            let stop_raw = stop_event.0 as usize;
            let (results_tx, results_rx) = mpsc::channel();
            let thread = std::thread::spawn(move || {
                run_loop_with(
                    LoopOptions {
                        dir: Some(dir.clone()),
                        event_name,
                        stop: Some(stop_raw),
                        publish_request: false,
                        periodic_ms: 2_000,
                        control_poll_ms: None,
                    },
                    move |_| {
                        let result = manager.reinit();
                        let candidates = manager.app_config().effective_num_candidates();
                        results_tx
                            .send((result, candidates))
                            .expect("send config result");
                        format!("{result:?}")
                    },
                    || {},
                    FaultFlags::default,
                );
            });
            std::thread::sleep(Duration::from_millis(80));
            std::fs::write(&path, "[conversion\nnum_candidates = ").expect("broken config");
            assert_eq!(
                results_rx
                    .recv_timeout(Duration::from_secs(2))
                    .expect("failed read"),
                (LoadOutcome::Failed, 7)
            );
            std::fs::remove_file(&path).expect("remove config");
            assert_eq!(
                results_rx
                    .recv_timeout(Duration::from_secs(2))
                    .expect("missing file read"),
                (LoadOutcome::Failed, 7)
            );
            std::fs::write(&path, "[conversion]\nnum_candidates = 4\n").expect("fixed config");
            let mut recovered = false;
            for _ in 0..4 {
                if results_rx
                    .recv_timeout(Duration::from_secs(2))
                    .expect("recovery read")
                    == (LoadOutcome::Updated, 4)
                {
                    recovered = true;
                    break;
                }
            }
            assert!(recovered, "last good config was not replaced after repair");
            unsafe { SetEvent(stop_event) }.expect("signal stop");
            thread.join().expect("watch thread");
            let _ = unsafe { CloseHandle(stop_event) };
            std::fs::remove_dir_all(path.parent().expect("parent")).expect("remove test directory");
        }

        #[test]
        fn unrelated_directory_writes_do_not_delay_periodic_check_or_publish_config() {
            use crate::engine::config::{ConfigManager, LoadOutcome};
            let (dir, path, event_name) = fixture("unrelated-writes");
            std::fs::write(&path, "[conversion]\nnum_candidates = 7\n").expect("valid config");
            let mut manager = ConfigManager::from_path(path);
            let stop_event = create_event(None, true).expect("stop event");
            let stop_raw = stop_event.0 as usize;
            let (results_tx, results_rx) = mpsc::channel();
            let watch_dir = dir.clone();
            let thread = std::thread::spawn(move || {
                run_loop_with(
                    LoopOptions {
                        dir: Some(watch_dir),
                        event_name,
                        stop: Some(stop_raw),
                        publish_request: false,
                        periodic_ms: 800,
                        control_poll_ms: None,
                    },
                    move |reason| {
                        let result = manager.reinit();
                        results_tx.send((reason, result)).expect("send result");
                        format!("{result:?}")
                    },
                    || {},
                    FaultFlags::default,
                );
            });
            let start = std::time::Instant::now();
            std::thread::sleep(Duration::from_millis(80));
            for i in 0..12 {
                std::fs::write(dir.join("learn_history.bin"), [i as u8]).expect("unrelated write");
                std::thread::sleep(Duration::from_millis(80));
            }
            let mut saw_periodic = false;
            while let Ok((reason, result)) = results_rx.recv_timeout(Duration::from_millis(200)) {
                assert_eq!(
                    result,
                    LoadOutcome::Unchanged,
                    "unrelated file republished config"
                );
                if reason == "periodic" {
                    saw_periodic = true;
                    break;
                }
            }
            assert!(
                saw_periodic,
                "continuous notifications delayed the periodic read"
            );
            assert!(start.elapsed() < Duration::from_secs(2));
            assert_eq!(
                results_rx
                    .recv_timeout(Duration::from_secs(2))
                    .expect("debounced read"),
                ("notify", LoadOutcome::Unchanged)
            );
            unsafe { SetEvent(stop_event) }.expect("signal stop");
            thread.join().expect("watch thread");
            let _ = unsafe { CloseHandle(stop_event) };
            std::fs::remove_dir_all(dir).expect("remove test directory");
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
