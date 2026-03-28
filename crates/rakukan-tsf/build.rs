fn main() {
    // rerun-if-changed を一切書かないことで毎ビルド時にこのスクリプトが実行され、
    // RAKUKAN_BUILD_TIME が常に最新のビルド時刻に更新される。

    // ビルド時刻を埋め込む（DLL差し替え確認用）
    // rerun-if-changed を書かないことで毎ビルド時に更新される
    {
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
        println!("cargo:rustc-env=RAKUKAN_BUILD_TIME={build_time}");
    }

    // ICO を DLL リソースとして埋め込む
    embed_resource::compile("rakukan.rc", embed_resource::NONE);
}
