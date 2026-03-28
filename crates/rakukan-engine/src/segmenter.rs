use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use tracing::{debug, info, warn};
use vibrato::{Dictionary, Tokenizer};

use rakukan_dict::dict_dir;

const VIBRATO_DICT_ENV: &str = "RAKUKAN_VIBRATO_DIC";

static TOKENIZER: LazyLock<Result<Tokenizer, String>> = LazyLock::new(|| match load_tokenizer() {
    Ok(tokenizer) => {
        info!("[vibrato] tokenizer ready");
        Ok(tokenizer)
    }
    Err(err) => {
        warn!("[vibrato] fallback to heuristic: {err:#}");
        Err(err.to_string())
    }
});

pub fn segment_surface(surface: &str) -> Vec<String> {
    if surface.is_empty() {
        return Vec::new();
    }

    match &*TOKENIZER {
        Ok(tokenizer) => {
            let mut worker = tokenizer.new_worker();
            worker.reset_sentence(surface);
            worker.tokenize();

            let mut out = Vec::with_capacity(worker.num_tokens());
            for i in 0..worker.num_tokens() {
                let token = worker.token(i);
                let piece = token.surface();
                if !piece.is_empty() {
                    out.push(piece.to_string());
                }
            }

            if out.is_empty() {
                debug!("[vibrato] empty token stream for surface={surface:?}");
                vec![surface.to_string()]
            } else {
                out
            }
        }
        Err(_) => vec![surface.to_string()],
    }
}

pub fn vibrato_dict_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var(VIBRATO_DICT_ENV) {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    let primary = dict_dir()?.join("vibrato").join("system.dic");
    if primary.exists() {
        return Some(primary);
    }

    None
}

fn load_tokenizer() -> Result<Tokenizer> {
    let path = vibrato_dict_path()
        .context("system.dic not found in %LOCALAPPDATA%\\rakukan\\dict\\vibrato")?;
    let file = File::open(&path)
        .with_context(|| format!("failed to open Vibrato dictionary: {}", path.display()))?;
    let reader = BufReader::new(file);
    let dict = Dictionary::read(reader)
        .with_context(|| format!("failed to read Vibrato dictionary: {}", path.display()))?;
    info!("[vibrato] dictionary loaded path={}", path.display());
    Ok(Tokenizer::new(dict))
}
