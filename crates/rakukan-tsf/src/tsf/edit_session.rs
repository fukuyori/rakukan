//! TSF EditSession: DoEditSession コールバックで安全にテキストを操作する
//!
//! TSF では `ITfContext` のテキストを変更するには必ず
//! `RequestEditSession` → `DoEditSession` 経由で行う必要がある。
//! ここでは `FnOnce(u32) -> Result<()>` を持つ汎用セッションを提供する。

use windows::{
    Win32::UI::TextServices::{ITfEditSession, ITfEditSession_Impl},
    core::implement,
};

// Safety: TSF スレッド (STA) からのみ呼ばれる
unsafe impl Send for EditSession {}
unsafe impl Sync for EditSession {}

type EditFn = Box<dyn FnOnce(u32) -> windows::core::Result<()>>;

/// 要求から実行までがこの時間を超えたら WARN を出す（Step 13-2）。
///
/// 同期実行なら 1ms 未満で来る。アプリが編集権をすぐ渡さない（非同期になる）と
/// 「変換したのに画面が変わらない」の原因になるため、区間として観測できるようにする。
const EDIT_SESSION_SLOW_MS: u128 = 100;

/// 任意のクロージャを DoEditSession として実行する汎用セッション
#[implement(ITfEditSession)]
pub struct EditSession {
    func: std::cell::RefCell<Option<EditFn>>,
    /// セッション生成時刻。`RequestEditSession` は生成直後に呼ばれるので、
    /// 要求から `DoEditSession` 実行までの遅延の近似として使う（Step 13-2）。
    requested_at: std::time::Instant,
}

impl EditSession {
    // COM の生成パターン: Self ではなくインターフェース (ITfEditSession) を返す
    #[allow(clippy::new_ret_no_self)]
    pub fn new<F>(f: F) -> ITfEditSession
    where
        F: FnOnce(u32) -> windows::core::Result<()> + 'static,
    {
        let session = EditSession {
            func: std::cell::RefCell::new(Some(Box::new(f))),
            requested_at: std::time::Instant::now(),
        };
        session.into()
    }
}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> windows::core::Result<()> {
        // 要求 → 実行の遅延（Step 13-2）。同期実行なら 1ms 未満。
        let waited_us = self.requested_at.elapsed().as_micros();
        if waited_us / 1000 >= EDIT_SESSION_SLOW_MS {
            tracing::warn!("edit_session: SLOW grant waited_us={waited_us}");
        } else {
            tracing::trace!("edit_session: granted waited_us={waited_us}");
        }
        let f = self.func.try_borrow_mut().ok().and_then(|mut g| g.take());

        if let Some(func) = f { func(ec) } else { Ok(()) }
    }
}
