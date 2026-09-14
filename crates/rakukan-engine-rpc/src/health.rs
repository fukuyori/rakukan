//! 推論が即時失敗する壊れ方からの復帰（Issue #43）。
//!
//! GPU ドライバの更新・スリープ復帰・TDR で、ホストが掴んでいるデバイスだけが
//! 無効になることがある。以後の推論は詰まらず**即座に失敗**して idle に戻るため、
//! 「running が続いたら再起動」という既存の watchdog では拾えない。
//!
//! 判断と抑止はホストに置く。TSF DLL はアプリごとに別プロセスで動くので、
//! TSF 側に再起動の判断を持たせると、障害が続く間は各アプリが順に共有ホストを
//! 撃つ（0.9.13 の再起動ストームと同じ構造）。ホスト 1 プロセスで数えれば
//! プロセスごとのクールダウンは要らない。
//!
//! # 段階
//!
//! 1. 連続 [`FAILURE_THRESHOLD`] 回失敗 → ホストが自分で終了し、次の呼び出しで
//!    クライアントが新しいホストを spawn する（既存の透過再接続）。新しい
//!    プロセスなら llama のコンテキストも GPU デバイスも取り直される
//! 2. 終了マーカーが新しい（[`MARKER_WINDOW_MS`] 以内）状態で起動して、また
//!    [`UNRECOVERABLE_ATTEMPTS`] 回目に達した → `unrecoverable`。ホストは生き続け、
//!    状態だけ返す（TSF は文言を出し、辞書候補のみで使い続けられる）
//!
//! 推論が 1 回成功したら失敗の数もマーカーも捨てる。
//!
//! # プロセス内でモデルだけ作り直さない理由
//!
//! `engine_start_load_dict` / `engine_start_load_model` は「エンジンが converter を
//! 持っていないとき」にだけ働く。推論が失敗しても converter そのものは生きて
//! いるため、作り直しを頼んでも `start_load_model` が早期 return し、仮に新しい
//! converter を作っても `poll_model_ready` が「注入済み」として捨てる。
//! 差し替えるには ABI の追加が要るので、今はプロセスごと作り直す。

use std::path::PathBuf;

/// この回数だけ連続で失敗したら次の段階へ進む。
pub const FAILURE_THRESHOLD: u32 = 3;
/// 終了マーカーを「新しい」とみなす時間（ミリ秒）。
pub const MARKER_WINDOW_MS: u64 = 5 * 60 * 1000;
/// マーカー上の試行回数がこれ以上なら、次の失敗で `unrecoverable` にする。
pub const UNRECOVERABLE_ATTEMPTS: u32 = 2;

/// ホストが返す健全性。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// 正常、または一時的な失敗（復帰を試していない）。
    Ok,
    /// 復帰の途中（モデル作り直し / 自己終了）。
    Recovering,
    /// 復帰できなかった。GPU が使えないまま。
    Unrecoverable,
}

impl Health {
    pub fn as_str(self) -> &'static str {
        match self {
            Health::Ok => "ok",
            Health::Recovering => "recovering",
            Health::Unrecoverable => "unrecoverable",
        }
    }

    pub fn from_str_lossy(s: &str) -> Self {
        match s {
            "recovering" => Health::Recovering,
            "unrecoverable" => Health::Unrecoverable,
            _ => Health::Ok,
        }
    }
}

/// 観測の結果、ホストが取るべき動作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 何もしない。
    None,
    /// ホストを終了する（マーカーを書いてから）。
    ExitHost,
    /// 復帰を諦める。
    MarkUnrecoverable,
    /// 推論が成功した。失敗の記録とマーカーを捨てる。
    Recovered,
}

/// 失敗の連続回数と復帰の段階を数える。
///
/// `observe` には `bg_status()` の値をそのまま渡す。ポーリングのたびに呼ばれても
/// よいように、**状態が変わったときだけ**数える。
pub struct HealthTracker {
    last_status: String,
    failures: u32,
    /// 起動時のマーカーから読んだ、直近の自己終了の回数。
    prior_attempts: u32,
    health: Health,
}

impl HealthTracker {
    pub fn new(prior_attempts: u32) -> Self {
        Self {
            last_status: String::new(),
            failures: 0,
            prior_attempts,
            health: Health::Ok,
        }
    }

    pub fn health(&self) -> Health {
        self.health
    }

    /// 次に自己終了するときにマーカーへ書く試行回数。
    pub fn next_attempt(&self) -> u32 {
        self.prior_attempts + 1
    }

    pub fn observe(&mut self, status: &str) -> Action {
        if self.last_status == status {
            return Action::None;
        }
        self.last_status = status.to_string();

        match status {
            "error" => {
                self.failures += 1;
                if self.failures < FAILURE_THRESHOLD {
                    return Action::None;
                }
                self.failures = 0;
                if self.prior_attempts >= UNRECOVERABLE_ATTEMPTS {
                    self.health = Health::Unrecoverable;
                    Action::MarkUnrecoverable
                } else {
                    self.health = Health::Recovering;
                    Action::ExitHost
                }
            }
            "done" => {
                let was_degraded =
                    self.failures > 0 || self.health != Health::Ok || self.prior_attempts > 0;
                self.failures = 0;
                self.prior_attempts = 0;
                self.health = Health::Ok;
                if was_degraded {
                    Action::Recovered
                } else {
                    Action::None
                }
            }
            _ => Action::None,
        }
    }
}

/// 自己終了のマーカー。`<exited_at_ms> <attempt>` の 1 行。
///
/// JSON にしないのは、この crate に serde_json を足さずに済ませるため。
/// 障害時に人が読む前提の 1 行なので、この形式で足りる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryMarker {
    pub exited_at_ms: u64,
    pub attempt: u32,
}

impl RecoveryMarker {
    pub fn encode(&self) -> String {
        format!("{} {}\n", self.exited_at_ms, self.attempt)
    }

    pub fn decode(text: &str) -> Option<Self> {
        let mut it = text.split_whitespace();
        let exited_at_ms = it.next()?.parse().ok()?;
        let attempt = it.next()?.parse().ok()?;
        Some(Self {
            exited_at_ms,
            attempt,
        })
    }
}

/// マーカーが「新しい」なら試行回数、古い・無いなら 0。
pub fn prior_attempts(marker: Option<RecoveryMarker>, now_ms: u64) -> u32 {
    match marker {
        Some(m) if now_ms.saturating_sub(m.exited_at_ms) <= MARKER_WINDOW_MS => m.attempt,
        _ => 0,
    }
}

/// `%LOCALAPPDATA%\rakukan\engine-recovery.txt`
pub fn marker_path() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(
        PathBuf::from(base)
            .join("rakukan")
            .join("engine-recovery.txt"),
    )
}

pub fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn load_marker() -> Option<RecoveryMarker> {
    let path = marker_path()?;
    let text = std::fs::read_to_string(path).ok()?;
    RecoveryMarker::decode(&text)
}

pub fn write_marker(marker: RecoveryMarker) {
    if let Some(path) = marker_path()
        && let Err(e) = std::fs::write(&path, marker.encode())
    {
        tracing::warn!("recovery marker write failed ({}): {e}", path.display());
    }
}

pub fn clear_marker() {
    if let Some(path) = marker_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(t: &mut HealthTracker, statuses: &[&str]) -> Vec<Action> {
        statuses.iter().map(|s| t.observe(s)).collect()
    }

    #[test]
    fn polling_the_same_status_does_not_count() {
        let mut t = HealthTracker::new(0);
        // 同じ "error" を何回読んでも 1 回の失敗
        let actions = feed(&mut t, &["error", "error", "error", "error"]);
        assert!(actions.iter().all(|a| *a == Action::None));
        assert_eq!(t.health(), Health::Ok);
    }

    #[test]
    fn three_failures_exit_the_host() {
        let mut t = HealthTracker::new(0);
        // running → error を 3 往復で「連続 3 回」
        for _ in 0..2 {
            assert_eq!(t.observe("running"), Action::None);
            assert_eq!(t.observe("error"), Action::None);
        }
        assert_eq!(t.observe("running"), Action::None);
        assert_eq!(t.observe("error"), Action::ExitHost);
        assert_eq!(t.health(), Health::Recovering);
        assert_eq!(t.next_attempt(), 1);
    }

    #[test]
    fn repeated_restarts_end_as_unrecoverable() {
        // 直近に 2 回自己終了している状態で起動
        let mut t = HealthTracker::new(UNRECOVERABLE_ATTEMPTS);
        for _ in 0..2 {
            t.observe("running");
            t.observe("error");
        }
        t.observe("running");
        assert_eq!(t.observe("error"), Action::MarkUnrecoverable);
        assert_eq!(t.health(), Health::Unrecoverable);
    }

    #[test]
    fn success_clears_failures_and_marker() {
        let mut t = HealthTracker::new(1);
        t.observe("running");
        t.observe("error");
        assert_eq!(t.observe("done"), Action::Recovered);
        assert_eq!(t.health(), Health::Ok);
        assert_eq!(t.next_attempt(), 1, "マーカー由来の回数も捨てる");
        // 正常運転では毎回 Recovered を出さない
        t.observe("running");
        assert_eq!(t.observe("done"), Action::None);
    }

    #[test]
    fn marker_expires_outside_the_window() {
        let m = RecoveryMarker {
            exited_at_ms: 1_000,
            attempt: 2,
        };
        assert_eq!(prior_attempts(Some(m), 1_000 + MARKER_WINDOW_MS), 2);
        assert_eq!(prior_attempts(Some(m), 1_000 + MARKER_WINDOW_MS + 1), 0);
        assert_eq!(prior_attempts(None, 1_000), 0);
    }

    #[test]
    fn marker_round_trips() {
        let m = RecoveryMarker {
            exited_at_ms: 1_726_000_000_000,
            attempt: 2,
        };
        assert_eq!(RecoveryMarker::decode(&m.encode()), Some(m));
        assert_eq!(RecoveryMarker::decode("こわれている"), None);
        assert_eq!(RecoveryMarker::decode(""), None);
    }
}
