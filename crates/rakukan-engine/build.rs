include!("../../build-support/git_info.rs");

/// CUDA 版（Windows）の CUDA ランタイムの読み込み用のライブラリを `cudart_static.lib` から取る（#90）。
///
/// CUDA 13 の Windows では、`cudart.lib` も `cudart_static.lib` も、ランタイムをディスプレイドライバ
/// から読み込む小さな読み込み用のライブラリ（hybrid）で、同じ名前のオブジェクトを持つ。
/// `llama-cpp-sys-2` は `cudart.lib` を指定するが、CUDA の EULA の Attachment A に名前で挙がって
/// いるのは `cudart_static.lib` なので、こちらを先に並べ、未解決の記号をここから解決させる。
/// リンカーは参照元のライブラリより後ろから記号を探すので、`cudart_static.lib` を別に指定しても
/// 使われない。そこで `cudart_static.lib` をこの crate の `OUT_DIR` に `cudart.lib` の名前で写し、
/// 検索先に足す。llama-cpp-sys-2 の `cudart` の指定は、検索先の並びでこちらに解決される。
/// 実際にどちらが使われたかは、リンカーのコマンドライン（`--print link-args`）と MAP で確かめる。
fn link_cudart_static_on_windows() {
    let cuda = std::env::var_os("CARGO_FEATURE_CUDA").is_some();
    let windows = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    if !(cuda && windows) {
        return;
    }
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    let Some(root) = std::env::var_os("CUDA_PATH") else {
        println!(
            "cargo:warning=CUDA_PATH is not set; linking cudart.lib as llama-cpp-sys-2 requests"
        );
        return;
    };
    let src = std::path::PathBuf::from(root)
        .join("lib")
        .join("x64")
        .join("cudart_static.lib");
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    println!("cargo:rerun-if-changed={}", src.display());
    match std::fs::copy(&src, out.join("cudart.lib")) {
        Ok(_) => println!("cargo:rustc-link-search=native={}", out.display()),
        Err(e) => println!(
            "cargo:warning=cannot copy {} ({e}); linking cudart.lib as llama-cpp-sys-2 requests",
            src.display()
        ),
    }
}

fn main() {
    // build 識別子（git sha）。host 側と突き合わせて別ビルドの組み合わせを検出する（Issue #8）。
    emit_git_sha();
    link_cudart_static_on_windows();

    // RAKUKAN_ENGINE_BUILD_TIME: ビルド時刻。
    // （git_info.rs が HEAD への rerun-if-changed を出すため、以前の「毎ビルド更新」ではなく
    //   HEAD かパッケージ内ファイルが変わったときに更新される）
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86400;
    let rem = secs % 86400;
    let h = rem / 3600;
    let m = (rem % 3600) / 60;
    let s = rem % 60;
    let mut y = 1970u64;
    let mut d = days;
    loop {
        let leap = (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
        let yd = if leap { 366 } else { 365 };
        if d < yd {
            break;
        }
        d -= yd;
        y += 1;
    }
    let leap = (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    let month_days: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut mo = 1u64;
    for &md in &month_days {
        if d < md {
            break;
        }
        d -= md;
        mo += 1;
    }
    let build_time = format!("{y:04}-{mo:02}-{:02} {h:02}:{m:02}:{s:02} UTC", d + 1);
    println!("cargo:rustc-env=RAKUKAN_ENGINE_BUILD_TIME={build_time}");
}
