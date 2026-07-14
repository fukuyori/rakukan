//! 「きだじゅんいちろうし(は)」変換異常の再現実験。
//! context にひらがな確定文「きだじゅんいちろう氏は、」が入った場合の
//! ビーム出力を、context なし・通常 context と比較する。
//!
//! cargo run -p rakukan-engine --example repro_context --release

use rakukan_engine::{Backend, KanaKanjiConverter};

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("rakukan_engine=debug")
        .with_writer(std::io::stderr)
        .init();

    let backend = Backend::from_variant_id("jinen-v1-small-q5")?;
    let conv = KanaKanjiConverter::new(backend)?;

    let reading_full = "きだじゅんいちろうしは";
    let reading_short = "きだじゅんいちろうし";

    let contexts: Vec<(&str, String)> = vec![
        ("context なし", String::new()),
        (
            "通常 context（10:23:05 相当）",
            "あらゆるコレクターの涙する場面だ。場面はがの共感を呼び、".to_string(),
        ),
        (
            "汚染 context（10:23:10 相当: ひらがな確定文入り）",
            "あらゆるコレクターの涙する場面だ。場面はがの共感を呼び、きだじゅんいちろう氏は、"
                .to_string(),
        ),
        (
            "汚染 context 最小形（きだじゅんいちろう氏は、のみ）",
            "きだじゅんいちろう氏は、".to_string(),
        ),
        (
            "実機 context 再現（10:23:10 の committed 相当）",
            "ぐにゃりとこのきもちは、あらゆるコレクターの涙する場面だ。場面はがの共感を呼び、きだじゅんいちろう氏は、"
                .to_string(),
        ),
    ];

    for reading in [reading_short, reading_full] {
        println!("=== reading: {reading} ===");
        for (label, ctx) in &contexts {
            let cands = conv.convert(reading, ctx, 19)?;
            println!("  [{label}]");
            for (i, c) in cands.iter().enumerate() {
                println!("    {}: {}", i + 1, c);
            }
        }
        println!();
    }
    Ok(())
}
