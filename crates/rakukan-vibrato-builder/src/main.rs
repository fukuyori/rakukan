use std::fs::{self, File};
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use encoding_rs::EUC_JP;
use vibrato::SystemDictionaryBuilder;

#[derive(Parser, Debug)]
#[command(
    name = "rakukan-vibrato-builder",
    about = "MeCab IPADIC から Vibrato system.dic を生成する"
)]
struct Args {
    #[arg(long)]
    input_dir: PathBuf,

    #[arg(long)]
    output: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let input_dir = args.input_dir;
    if !input_dir.is_dir() {
        bail!("input_dir is not a directory: {}", input_dir.display());
    }

    let lexicon = build_lexicon_csv(&input_dir)?;
    let matrix = decode_text_file(&input_dir.join("matrix.def"))?;
    let char_def = decode_text_file(&input_dir.join("char.def"))?;
    let unk_def = decode_text_file(&input_dir.join("unk.def"))?;

    let dict = SystemDictionaryBuilder::from_readers(
        Cursor::new(lexicon.into_bytes()),
        Cursor::new(matrix.into_bytes()),
        Cursor::new(char_def.into_bytes()),
        Cursor::new(unk_def.into_bytes()),
    )
    .context("failed to build Vibrato dictionary from IPADIC")?;

    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output dir: {}", parent.display()))?;
    }

    let mut file = File::create(&args.output)
        .with_context(|| format!("failed to create output: {}", args.output.display()))?;
    let written = dict
        .write(&mut file)
        .with_context(|| format!("failed to write output: {}", args.output.display()))?;
    file.flush()
        .with_context(|| format!("failed to flush output: {}", args.output.display()))?;

    println!(
        "built Vibrato dictionary: input={} output={} bytes={} csv_files={}",
        input_dir.display(),
        args.output.display(),
        written,
        count_lexicon_csvs(&input_dir)?,
    );
    Ok(())
}

fn build_lexicon_csv(input_dir: &Path) -> Result<String> {
    let mut csv_paths = lexicon_csv_paths(input_dir)?;
    csv_paths.sort();

    if csv_paths.is_empty() {
        bail!("no lexicon csv files found in {}", input_dir.display());
    }

    let mut merged = String::new();
    for path in &csv_paths {
        let text = decode_text_file(path)?;
        merged.push_str(text.trim_end_matches(['\r', '\n']));
        merged.push('\n');
    }
    Ok(merged)
}

fn lexicon_csv_paths(input_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(input_dir)
        .with_context(|| format!("failed to read directory: {}", input_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("csv") {
            continue;
        }
        out.push(path);
    }
    Ok(out)
}

fn count_lexicon_csvs(input_dir: &Path) -> Result<usize> {
    Ok(lexicon_csv_paths(input_dir)?.len())
}

fn decode_text_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("failed to read: {}", path.display()))?;
    if let Ok(text) = String::from_utf8(bytes.clone()) {
        return Ok(text);
    }

    let (decoded, _, had_errors) = EUC_JP.decode(&bytes);
    if had_errors {
        bail!("failed to decode {} as UTF-8 or EUC-JP", path.display());
    }
    Ok(decoded.into_owned())
}
