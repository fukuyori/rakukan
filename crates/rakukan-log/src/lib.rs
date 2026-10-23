//! rakukan のログのファイル（ログの見直しの段 2・4・5、#73・#75・#76）。
//!
//! TSF DLL・エンジンホスト（・トレイ）で共有する。subscriber はそれぞれのモジュールが
//! 自分で持つ（EXE と DLL で tracing の static は共有されない）。
//!
//! - [`file`] — 起動インスタンスごとの名前と `.lock`、書き込み時のローテーション、掃除
//! - [`header`] — 各ファイルの 1 行目（発生元を特定する項目）
//! - [`AsyncLog`] — 呼び出し元は行をキューに入れるだけにし、書き込みは専用のスレッドで行う
//! - [`etw`] — 調査用の ETW provider（セッションは手で開始する）
//!
//! 各行に付く項目（時刻 UTC・レベル・スレッド ID・target）と、ヘッダーにしか無い項目
//! （実行ファイル・パッケージファミリー名・PID・起動インスタンス・ビルド）は分けて持つ。

pub mod etw;
pub mod file;
pub mod header;
mod queue;

pub use queue::{AsyncLog, Flusher, QUEUE_CAPACITY};
