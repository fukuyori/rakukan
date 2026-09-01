//! host 側（rakukan-engine-host が本 crate を経由して参照する）の build 識別子を埋め込む。
//! 実体は `build-support/git_info.rs`（engine DLL の build.rs と共通）。

include!("../../build-support/git_info.rs");

fn main() {
    emit_git_sha();
}
