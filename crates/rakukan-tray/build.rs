include!("../../build-support/git_info.rs");

fn main() {
    // build 識別子（git sha）。ログのヘッダーと、キーボードの一覧を補った版の記録に使う。
    emit_git_sha();
}
