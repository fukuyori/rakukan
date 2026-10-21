//! 直接入力の間の接続の確認（入力処理の見直しの段 6、docs/Input_Pipeline_Plan.md 4.2 の
//! 「直接入力」の外す条件）。
//!
//! 回復できない場合の処置の後、直接入力にしたスレッドの窓を登録する（[`watch`]）。確認の
//! スレッドが 5 s ごとに `RAKUKAN_ENGINE` を `try_lock` で取り（取れなければその回は飛ばす）、
//! エンジンの口が無ければ初期化を促し、あれば `probe()` を送る。成功したら、その時点の
//! `HostLink` の世代と登録の番号を付けて、登録した窓へ `WM_APP_LINK_UP` を送る。外すか
//! どうかは UI のスレッドが決める（`key_queue::on_link_up`）。登録が無くなったらスレッドを
//! 終える。
//!
//! 送信は登録と同じロックの中で行い、送る直前に登録が残っていることを確かめる。登録を
//! 外した（[`unwatch`] が戻った）後は、その窓へ送らない（Deactivate は外してから窓を壊す）。
//! 受け取る側は登録の番号を照合し、自分の今の登録でない通知を捨てる。

use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// 確認の間隔（設計書 4.2）。
const INTERVAL: Duration = Duration::from_secs(5);

struct Probe {
    /// 確認の結果を知らせる窓と登録の番号（直接入力の間のスレッドの窓）。
    targets: Vec<(isize, u64)>,
    /// 確認のスレッドが動いている。
    running: bool,
    /// 最後に振った登録の番号。
    last_token: u64,
}

static PROBE: Mutex<Probe> = Mutex::new(Probe {
    targets: Vec::new(),
    running: false,
    last_token: 0,
});

fn probe_state() -> MutexGuard<'static, Probe> {
    PROBE.lock().unwrap_or_else(|e| e.into_inner())
}

/// 窓 `hwnd` を登録し、確認のスレッドが無ければ起こす。登録の番号を返す（通知に付く）。
/// スレッドを起こせなければ `None`（登録もしない）。
pub(crate) fn watch(hwnd: isize) -> Option<u64> {
    let mut p = probe_state();
    if !p.running {
        if let Err(e) = std::thread::Builder::new()
            .name("rakukan-host-probe".into())
            .spawn(run)
        {
            tracing::warn!("host_probe: failed to start the probe thread: {e}");
            return None;
        }
        p.running = true;
    }
    p.last_token += 1;
    let token = p.last_token;
    p.targets.retain(|(h, _)| *h != hwnd);
    p.targets.push((hwnd, token));
    Some(token)
}

/// 登録を外す（直接入力を外した・窓を壊す）。戻った後は、その登録へ通知を送らない。
pub(crate) fn unwatch(token: u64) {
    probe_state().targets.retain(|(_, t)| *t != token);
}

fn run() {
    tracing::info!("host_probe: started");
    loop {
        std::thread::sleep(INTERVAL);
        {
            let mut p = probe_state();
            if p.targets.is_empty() {
                p.running = false;
                tracing::info!("host_probe: stopped");
                return;
            }
        }
        let Some(epoch) = check() else {
            continue;
        };
        // 登録と同じロックの中で、今の登録へだけ送る（PostMessageW は待たない）
        let p = probe_state();
        for &(hwnd, token) in &p.targets {
            unsafe {
                let _ = PostMessageW(
                    HWND(hwnd as *mut _),
                    crate::tsf::candidate_window::WM_APP_LINK_UP,
                    WPARAM(epoch as usize),
                    LPARAM(token as isize),
                );
            }
        }
    }
}

/// 1 回の確認。成功したら、その時点の `HostLink` の世代を返す。
fn check() -> Option<u64> {
    let guard = match super::state::RAKUKAN_ENGINE.try_lock() {
        Ok(g) => g,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        // 誰かが使っている: この回は飛ばす
        Err(TryLockError::WouldBlock) => return None,
    };
    let Some(engine) = guard.0.as_ref() else {
        // 口が無い: 初期化を促す（次の回で確かめる）
        drop(guard);
        super::state::engine_start_bg_init();
        return None;
    };
    match engine.probe() {
        Ok(health) => {
            let link = rakukan_engine_rpc::host_link();
            tracing::info!(
                "host_probe: host answered (health={health}, link={:?} epoch={})",
                link.state,
                link.epoch
            );
            Some(link.epoch)
        }
        Err(e) => {
            tracing::debug!("host_probe: probe failed: {e:#}");
            None
        }
    }
}
