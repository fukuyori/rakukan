//! TSF EditSession: DoEditSession コールバックで安全にテキストを操作する
//!
//! TSF では `ITfContext` のテキストを変更するには必ず
//! `RequestEditSession` → `DoEditSession` 経由で行う必要がある。
//! ここでは `FnOnce(u32) -> Result<()>` を持つ汎用セッションを提供する。

use windows::{
    core::implement,
    Win32::UI::TextServices::{ITfEditSession, ITfEditSession_Impl},
};

// Safety: TSF スレッド (STA) からのみ呼ばれる
unsafe impl Send for EditSession {}
unsafe impl Sync for EditSession {}

type EditFn = Box<dyn FnOnce(u32) -> windows::core::Result<()>>;

/// 任意のクロージャを DoEditSession として実行する汎用セッション
#[implement(ITfEditSession)]
pub struct EditSession {
    func: std::cell::RefCell<Option<EditFn>>,
}

impl EditSession {
    pub fn new<F>(f: F) -> ITfEditSession
    where
        F: FnOnce(u32) -> windows::core::Result<()> + 'static,
    {
        let session = EditSession {
            func: std::cell::RefCell::new(Some(Box::new(f))),
        };
        session.into()
    }
}

impl ITfEditSession_Impl for EditSession_Impl {
    fn DoEditSession(&self, ec: u32) -> windows::core::Result<()> {
        let f = self
            .func
            .try_borrow_mut()
            .ok()
            .and_then(|mut g| g.take());

        if let Some(func) = f {
            func(ec)
        } else {
            Ok(())
        }
    }
}
