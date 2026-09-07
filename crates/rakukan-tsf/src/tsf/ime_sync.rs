//! IME オン/オフ切替の共通経路。
//!
//! 内部状態（`IMEState::ime_mode`）を唯一の正とし、TSF コンパートメント
//! `KEYBOARD_OPENCLOSE` はここから導出して書く。キー操作・言語バーメニュー・
//! フォーカス移動時の復元・Activate 時の初期化・外部アプリによるコンパートメント
//! 変更の取り込み、すべてがこの関数を通る。
//!
//! 経路ごとに片方だけ更新して状態がずれる（例: 内部はオンなのに表示は「A」）
//! ことを防ぐのが目的。

use windows::Win32::UI::TextServices::ITfThreadMgr;

use crate::diagnostics::{self as diag, DiagEvent};
use crate::engine::ime_mode::ImeMode;
use crate::tsf::{language_bar, tray_ipc};

/// 内部状態を `new` にし、コンパートメントと表示通知を同期する。
///
/// - `write_compartment = false` は「外部がコンパートメントを書いたのを取り込む」経路
///   専用。それ以外は必ず `true`（内部状態 → コンパートメントの向きで書く）。
/// - 言語バーの `OnUpdate` は呼び出し側の責務（`TextServiceFactory_Impl::notify_langbar_update`
///   が必要。ここでは次のキー入力で拾われるフラグだけ立てる）。
///
/// 戻り値: 変更前の状態。`ime_state` のロックが取れなかった場合は `None`
/// （その場合も内部状態以外の同期は行う）。
pub fn apply(
    tm: Option<&ITfThreadMgr>,
    tid: u32,
    new: ImeMode,
    write_compartment: bool,
    source: &'static str,
) -> Option<ImeMode> {
    let from = match crate::engine::state::ime_state_get() {
        Ok(mut st) => {
            let from = st.ime_mode;
            if from != new {
                st.set_ime_mode(new);
            }
            Some(from)
        }
        Err(e) => {
            tracing::warn!("ime_sync({source}): ime_state locked, mode not updated: {e}");
            None
        }
    };

    if write_compartment
        && let Some(tm) = tm
        && let Err(e) = unsafe { language_bar::set_open_close(tm, tid, new.is_on()) }
    {
        tracing::warn!(
            "ime_sync({source}): set_open_close({}) failed: {e}",
            new.is_on()
        );
        diag::event(DiagEvent::Error {
            site: "ime_sync/set_open_close",
            msg: e.to_string(),
        });
    }

    if let Some(from) = from
        && from != new
    {
        diag::event(DiagEvent::ModeChange {
            from: from.name().to_string(),
            to: new.name(),
        });
    }

    // 言語バーは呼び出し側で即時更新する。ここでは取りこぼし防止のフラグだけ立てる。
    crate::engine::state::langbar_update_set();
    tray_ipc::publish(new);
    from
}
