//! Installed-device fault control. This module is absent from normal builds.

use super::FaultFlags;
use serde::Deserialize;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const CONTROL_PATH_ENV: &str = "RAKUKAN_CONFIG_WATCH_TEST_CONTROL";
const CONTROL_ID_ENV: &str = "RAKUKAN_CONFIG_WATCH_TEST_ID";
const STARTUP_WAIT: Duration = Duration::from_secs(5);
const MAX_ACTIVE: Duration = Duration::from_secs(90);

#[derive(Deserialize)]
struct ControlFile {
    id: String,
    pid: u32,
    #[serde(default)]
    suppress_notifications: bool,
    #[serde(default)]
    fail_save_event: bool,
    #[serde(default)]
    fail_request_event: bool,
    #[serde(default)]
    fail_dir_watch: bool,
    #[serde(default)]
    fail_rearm_once: bool,
}

pub(super) struct FileFaultControl {
    path: PathBuf,
    id: String,
    started: Instant,
    active: bool,
}

impl FileFaultControl {
    pub(super) fn from_env() -> Option<Self> {
        let path = PathBuf::from(std::env::var_os(CONTROL_PATH_ENV)?);
        let id = std::env::var(CONTROL_ID_ENV).ok()?;
        if id.is_empty() {
            return None;
        }
        let deadline = Instant::now() + STARTUP_WAIT;
        let mut control = Self {
            path,
            id,
            started: Instant::now(),
            active: false,
        };
        loop {
            if let Some(flags) = control.read() {
                control.started = Instant::now();
                control.active = true;
                tracing::info!(
                    "config_watch: fault control armed pid={} flags={flags:?}",
                    std::process::id()
                );
                return Some(control);
            }
            if Instant::now() >= deadline {
                tracing::warn!("config_watch: fault control startup timed out; disabled");
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn read(&self) -> Option<FaultFlags> {
        let body = std::fs::read_to_string(&self.path).ok()?;
        let file: ControlFile = toml::from_str(&body).ok()?;
        if file.id != self.id || file.pid != std::process::id() {
            return None;
        }
        Some(FaultFlags {
            suppress_notifications: file.suppress_notifications,
            fail_save_event: file.fail_save_event,
            fail_request_event: file.fail_request_event,
            fail_dir_watch: file.fail_dir_watch,
            fail_rearm_once: file.fail_rearm_once,
        })
    }

    pub(super) fn poll(&mut self) -> FaultFlags {
        if !self.active {
            return FaultFlags::default();
        }
        if self.started.elapsed() < MAX_ACTIVE
            && let Some(flags) = self.read()
        {
            return flags;
        }
        let reason = if self.started.elapsed() >= MAX_ACTIVE {
            "deadline"
        } else {
            "control missing or mismatched"
        };
        self.active = false;
        tracing::info!("config_watch: fault control released reason={reason}");
        FaultFlags::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_control_and_deadline_release_without_rearming() {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "rakukan-watch-control-{}-{}.toml",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let write = |id: &str| {
            std::fs::write(
                &path,
                format!(
                    "id = '{id}'\npid = {}\nsuppress_notifications = true\n",
                    std::process::id()
                ),
            )
            .expect("write control");
        };
        write("matching");
        let mut control = FileFaultControl {
            path: path.clone(),
            id: "matching".to_owned(),
            started: Instant::now(),
            active: true,
        };
        assert!(control.poll().suppress_notifications);
        write("different");
        assert_eq!(control.poll(), FaultFlags::default());
        write("matching");
        assert_eq!(
            control.poll(),
            FaultFlags::default(),
            "released control cannot rearm"
        );

        control.active = true;
        control.started = Instant::now();
        std::fs::remove_file(&path).expect("remove control");
        assert_eq!(
            control.poll(),
            FaultFlags::default(),
            "deletion releases control"
        );
        assert!(!control.active);

        write("matching");
        control.active = true;
        control.started = Instant::now() - MAX_ACTIVE - Duration::from_millis(1);
        assert_eq!(control.poll(), FaultFlags::default());
        assert!(!control.active);
        std::fs::remove_file(path).expect("remove control");
    }
}
