/// rakukan.dict 診断ツール
/// 使い方: cargo run -p rakukan-dict --bin dict_check
fn main() {
    let path = {
        let localappdata =
            std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA が設定されていません");
        std::path::PathBuf::from(localappdata)
            .join("rakukan")
            .join("dict")
            .join("rakukan.dict")
    };

    println!("=== rakukan.dict 診断 ===");
    println!("パス: {}", path.display());

    // 1. ファイル存在確認
    if !path.exists() {
        println!("❌ ファイルが存在しません");
        return;
    }
    println!("✅ ファイル存在");

    // 2. サイズ確認
    let meta = std::fs::metadata(&path).expect("metadata 取得失敗");
    println!(
        "✅ サイズ: {} bytes ({:.1} MB)",
        meta.len(),
        meta.len() as f64 / 1_048_576.0
    );

    // 3. ファイルオープン
    let file = match std::fs::File::open(&path) {
        Ok(f) => {
            println!("✅ File::open 成功");
            f
        }
        Err(e) => {
            println!("❌ File::open 失敗: {e}");
            return;
        }
    };

    // 4. mmap
    let mmap = match unsafe { memmap2::Mmap::map(&file) } {
        Ok(m) => {
            println!("✅ mmap 成功: {} bytes", m.len());
            m
        }
        Err(e) => {
            println!("❌ mmap 失敗: {e}");
            return;
        }
    };

    // 5. ヘッダー確認
    if mmap.len() < 16 {
        println!("❌ ファイルが小さすぎる");
        return;
    }
    let magic = &mmap[0..4];
    println!("マジック: {:?} (期待値: b\"RKND\")", magic);
    if magic != b"RKND" {
        println!("❌ マジック不一致");
        return;
    }
    println!("✅ マジック一致");

    let version = u32::from_le_bytes(mmap[4..8].try_into().unwrap());
    let n_entries = u32::from_le_bytes(mmap[8..12].try_into().unwrap());
    let n_readings = u32::from_le_bytes(mmap[12..16].try_into().unwrap());
    println!("version={version}, n_entries={n_entries}, n_readings={n_readings}");

    if version != 1 {
        println!("❌ バージョン不一致 (got {version}, expected 1)");
        return;
    }
    println!("✅ バージョン一致");

    // 6. インデックス境界確認
    let index_off = 16usize;
    let index_size = n_readings as usize * 12;
    let reading_heap_off = index_off + index_size;
    println!("index_off={index_off}, index_size={index_size}, reading_heap_off={reading_heap_off}");
    if reading_heap_off > mmap.len() {
        println!("❌ インデックスがファイルサイズを超える");
        return;
    }
    println!("✅ インデックス境界 OK");

    // 7. reading_heap サイズ計算（index_off を正しく渡す）
    let mut max_end = 0usize;
    for i in 0..n_readings as usize {
        let base = index_off + i * 12;
        if base + 8 > mmap.len() {
            println!("❌ インデックス[{i}] 範囲外");
            return;
        }
        let off = u32::from_le_bytes(mmap[base..base + 4].try_into().unwrap()) as usize;
        let len = u16::from_le_bytes(mmap[base + 4..base + 6].try_into().unwrap()) as usize;
        max_end = max_end.max(off + len);
    }
    let reading_heap_size = max_end;
    let entries_off = reading_heap_off + reading_heap_size;
    let surface_heap_off = entries_off + n_entries as usize * 8;
    println!(
        "reading_heap_size={reading_heap_size}, entries_off={entries_off}, surface_heap_off={surface_heap_off}"
    );

    if surface_heap_off > mmap.len() {
        println!(
            "❌ ファイルサイズ不足 (mmap={}, needed={})",
            mmap.len(),
            surface_heap_off
        );
        return;
    }
    println!("✅ オフセット計算 OK");

    // 8. 先頭 reading を読んでサニティチェック
    if n_readings > 0 {
        let base = index_off;
        let off = u32::from_le_bytes(mmap[base..base + 4].try_into().unwrap()) as usize;
        let len = u16::from_le_bytes(mmap[base + 4..base + 6].try_into().unwrap()) as usize;
        let start = reading_heap_off + off;
        let end = start + len;
        if end <= entries_off {
            if let Ok(s) = std::str::from_utf8(&mmap[start..end]) {
                println!("先頭 reading: {:?}", s);
            }
        }
    }

    println!("\n✅✅✅ rakukan.dict は正常に読み取れます ✅✅✅");
}
