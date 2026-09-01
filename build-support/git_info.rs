// build.rs から `include!` して使う共通処理。
//
// `RAKUKAN_GIT_SHA` に「ビルド元の git コミット（短縮 12 桁）+ 作業ツリーが
// 汚れていれば `-dirty`」を埋め込む。host（rakukan-engine-abi 経由）と
// engine DLL の両方が同じ値を持つので、実行時に「ABI は同じだが別ビルド」を
// 検出できる（Issue #8）。git が使えない・リポジトリ外でビルドした場合は
// `unknown` になり、実行時の比較では「不明」として扱う（WARN は出さない）。

fn git_output(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    Some(s.trim().to_string())
}

fn emit_git_sha() {
    let sha = match git_output(&["rev-parse", "--short=12", "HEAD"]) {
        Some(sha) if !sha.is_empty() => {
            let dirty = git_output(&["status", "--porcelain", "--untracked-files=no"])
                .map(|s| !s.is_empty())
                .unwrap_or(false);
            if dirty { format!("{sha}-dirty") } else { sha }
        }
        _ => "unknown".to_string(),
    };
    println!("cargo:rustc-env=RAKUKAN_GIT_SHA={sha}");

    // HEAD（と symbolic ref の実体）が変わったら再実行する。
    // 作業ツリーの dirty 状態までは追跡しない（毎ビルドの git status は行わない）。
    if let Some(git_dir) = git_output(&["rev-parse", "--absolute-git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        if let Some(head_ref) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
            println!("cargo:rerun-if-changed={git_dir}/{head_ref}");
        }
    }
}
