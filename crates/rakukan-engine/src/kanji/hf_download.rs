//! HuggingFace model download utilities
//!
//! Downloads GGUF models from HuggingFace Hub and caches them locally.
//! Model definitions are loaded from `models.toml` via [`super::model_config`].

use super::error::KanjiError;
use super::model_config::{ModelFamily, VariantConfig, registry};
type Result<T> = super::error::Result<T>;
use hf_hub::{Cache, Repo, RepoType, api::sync::ApiBuilder};
use std::path::PathBuf;

/// 全ユーザーで共有するモデルの置き場所（`%ProgramData%\rakukan\models`、#33）。
/// hf-hub のキャッシュと同じ形（`models--<repo>\snapshots\...`）で置く。
fn shared_model_dir() -> Option<PathBuf> {
    let base = std::env::var_os("ProgramData")?;
    Some(PathBuf::from(base).join("rakukan").join("models"))
}

/// 以前の取得先（`%USERPROFILE%\.cache\huggingface\hub`）。読むだけにし、書き込み・削除はしない。
fn legacy_hf_cache_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(
        PathBuf::from(home)
            .join(".cache")
            .join("huggingface")
            .join("hub"),
    )
}

/// Download a GGUF model from HuggingFace Hub
///
/// Returns the local path to the downloaded file.
/// 探す順: 共有の置き場所（[`shared_model_dir`]）→ 以前の取得先（`install.ps1` が先に
/// 取得した `snapshots\main`、hf-hub のキャッシュ）。どこにも無ければ共有の置き場所へ取得する。
/// 共有の置き場所を作れない場合と、そこへの取得に失敗した場合は、以前の取得先へ取得する。
///
/// # Arguments
/// * `repo_id` - HuggingFace repository ID
/// * `filename` - Filename to download
///
/// # Environment Variables
/// * `HF_TOKEN` - HuggingFace API token (required for private repositories)
pub fn download_gguf(repo_id: &str, filename: &str) -> Result<PathBuf> {
    let hub_repo = Repo::new(repo_id.to_string(), RepoType::Model);
    let shared = shared_model_dir();
    if let Some(path) = shared
        .as_ref()
        .and_then(|dir| Cache::new(dir.clone()).repo(hub_repo.clone()).get(filename))
    {
        tracing::info!("Using shared model: {:?}", path);
        return Ok(path);
    }

    let legacy = legacy_hf_cache_dir();
    if let Some(hub) = &legacy {
        // install.ps1 が先に取得した置き方（snapshots/main/）
        let repo_slug = repo_id.replace('/', "--");
        let candidate = hub
            .join(format!("models--{}", repo_slug))
            .join("snapshots")
            .join("main")
            .join(filename);
        if candidate.exists() && candidate.metadata().map(|m| m.len() > 0).unwrap_or(false) {
            tracing::info!("Using pre-downloaded model: {:?}", candidate);
            return Ok(candidate);
        }
        if let Some(path) = Cache::new(hub.clone()).repo(hub_repo.clone()).get(filename) {
            tracing::info!("Using previously downloaded model: {:?}", path);
            return Ok(path);
        }
    }

    let shared = shared.filter(|dir| match std::fs::create_dir_all(dir) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!(
                "cannot create {:?} ({e}); downloading to the user cache",
                dir
            );
            false
        }
    });
    let Some(shared) = shared else {
        return fetch(legacy, &hub_repo, repo_id, filename);
    };
    // 共有の置き場所が既にあっても、別のユーザーが作ったものなどで書けないことがある。
    // そのときは以前の取得先へ取り直す。
    match fetch(Some(shared.clone()), &hub_repo, repo_id, filename) {
        Ok(path) => Ok(path),
        Err(e) if legacy.is_some() => {
            tracing::warn!(
                "download to {:?} failed ({e}); retrying in the user cache",
                shared
            );
            fetch(legacy, &hub_repo, repo_id, filename)
        }
        Err(e) => Err(e),
    }
}

/// `cache_dir`（`None` なら hf-hub の既定）へ取得する。
fn fetch(
    cache_dir: Option<PathBuf>,
    hub_repo: &Repo,
    repo_id: &str,
    filename: &str,
) -> Result<PathBuf> {
    let mut builder = ApiBuilder::new();
    if let Some(dir) = cache_dir {
        builder = builder.with_cache_dir(dir);
    }
    // Check for HF_TOKEN environment variable
    if let Ok(token) = std::env::var("HF_TOKEN") {
        builder = builder.with_token(Some(token));
    }
    let api = builder
        .build()
        .map_err(|e| KanjiError::Download(e.into()))?;

    let repo = api.repo(hub_repo.clone());

    tracing::info!("Downloading {} from {}...", filename, repo_id);

    let path = repo
        .get(filename)
        .map_err(|e| KanjiError::Download(e.into()))?;

    tracing::info!("Downloaded to {:?}", path);

    Ok(path)
}

/// Get local path for a variant, downloading if not cached.
pub fn get_variant_path(family: &ModelFamily, variant: &VariantConfig) -> Result<PathBuf> {
    download_gguf(&family.repo_id, &variant.filename)
}

/// Get the local path to `tokenizer.json` for a model family, downloading if necessary.
pub fn get_tokenizer_path(family: &ModelFamily) -> Result<PathBuf> {
    download_gguf(&family.repo_id, "tokenizer.json")
}

/// Convenience: look up a variant id in the global registry and return its local GGUF path.
pub fn get_path_by_id(variant_id: &str) -> Result<PathBuf> {
    let (family, variant) = registry()
        .find_variant(variant_id)
        .ok_or_else(|| KanjiError::UnknownVariant(variant_id.to_string()))?;
    get_variant_path(family, variant)
}

/// Convenience: look up a variant id and return the tokenizer path.
pub fn get_tokenizer_path_by_id(variant_id: &str) -> Result<PathBuf> {
    let (family, _variant) = registry()
        .find_variant(variant_id)
        .ok_or_else(|| KanjiError::UnknownVariant(variant_id.to_string()))?;
    get_tokenizer_path(family)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_default_variant_exists() {
        let reg = registry();
        let (family, variant) = reg.default_variant().expect("default variant must exist");
        assert!(!family.repo_id.is_empty());
        assert!(!variant.filename.is_empty());
    }

    #[test]
    fn test_find_all_variants() {
        let reg = registry();
        for id in reg.all_variant_ids() {
            assert!(
                reg.find_variant(id).is_some(),
                "find_variant should succeed for '{}'",
                id
            );
        }
    }

    #[test]
    fn test_get_path_by_id_unknown() {
        let result = get_path_by_id("nonexistent-model-id");
        assert!(result.is_err());
    }

    #[test]
    fn test_tokenizer_path_by_id_unknown() {
        let result = get_tokenizer_path_by_id("nonexistent-model-id");
        assert!(result.is_err());
    }
}
