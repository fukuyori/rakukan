include!("../../build-support/git_info.rs");

fn main() {
    // build 識別子（git sha）。host 側と突き合わせて別ビルドの組み合わせを検出する（Issue #8）。
    emit_git_sha();

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
        let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
        let yd = if leap { 366 } else { 365 };
        if d < yd {
            break;
        }
        d -= yd;
        y += 1;
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
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
