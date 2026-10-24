//! 設定の版の照合と、最後に採用した設定の保存（docs/Engine_Config_Apply_Plan.md 4.3 / 4.3a）。
//!
//! ホストは設定を採用するかを決めるときにだけ `config.toml` を読み、本文の SHA-256 を
//! TSF が送った `config_version` と照らし合わせる（TOML の解析はしない）。照合が通った
//! 設定だけを採用するので、古い設定を持ったプロセスの要求でホストの設定が戻らない。
//!
//! `config.toml` を TSF が解析できない間は、TSF が知らせる `broken_version`（解析できな
//! かった本文のハッシュ）とディスクの本文が一致すれば「壊れている」とみなし、ホストが
//! 最後に採用した設定（`host-config-last.json`）を使う。どのプロセスから繋いでも設定は
//! この 1 つなので、プロセスごとに設定が行き来しない。

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::protocol::ConfigVersion;

/// ディスクの `config.toml` を読んだ結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskConfig {
    /// 読めた。値は本文の SHA-256。
    Read(ConfigVersion),
    /// ファイルが無い。
    Missing,
    /// ほかの理由で読めない（書き込み中・権限など）。場所が分からないときも含む。
    Unreadable,
}

/// `Unreadable` のときに読み直す回数と間隔（設計書 7 章）。
const READ_ATTEMPTS: u32 = 3;
const READ_RETRY_DELAY: Duration = Duration::from_millis(50);

fn read_once(path: &Path) -> DiskConfig {
    match std::fs::read(path) {
        Ok(bytes) => DiskConfig::Read(sha256(&bytes)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DiskConfig::Missing,
        Err(e) => {
            tracing::debug!("config_check: cannot read {}: {e}", path.display());
            DiskConfig::Unreadable
        }
    }
}

/// `config.toml` を読む。一時的に読めないときは、少し待って読み直す。
pub fn read_disk(path: Option<&Path>) -> DiskConfig {
    let Some(path) = path else {
        return DiskConfig::Unreadable;
    };
    let mut result = read_once(path);
    for _ in 1..READ_ATTEMPTS {
        if result != DiskConfig::Unreadable {
            break;
        }
        std::thread::sleep(READ_RETRY_DELAY);
        result = read_once(path);
    }
    result
}

/// 照合の結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 送られた設定はディスクの今の本文から作られている。採用してよい。
    Adopt,
    /// ディスクの本文は、TSF が解析できなかった本文と同じ（壊れている間）。
    Broken,
    /// 送られた設定は古い、またはディスクを読めない。採用しない。
    Stale,
}

/// 設計書 4.3 の照合の表。
pub fn judge(
    config_version: Option<ConfigVersion>,
    broken_version: Option<ConfigVersion>,
    disk: DiskConfig,
) -> Verdict {
    match disk {
        DiskConfig::Read(h) if config_version == Some(h) => Verdict::Adopt,
        DiskConfig::Read(h) if broken_version == Some(h) => Verdict::Broken,
        DiskConfig::Read(_) => Verdict::Stale,
        // ファイルが無いときは、既定値（`None`）で作った設定だけを採用する
        DiskConfig::Missing if config_version.is_none() => Verdict::Adopt,
        DiskConfig::Missing | DiskConfig::Unreadable => Verdict::Stale,
    }
}

/// `Create` で何をするか（設計書 4.2・4.3・4.3a）。エンジンがあって設定が同じ場合は
/// 照合の前に `Unit` で返すので、ここには来ない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateDecision {
    /// エンジンがあり、新しい設定が採用できる: ホストは作り直さずに終わる（`CreateRestart`）。
    Restart,
    /// エンジンがあり、設定は採用しないが、今のまま使える（`CreateKept`）。
    Keep,
    /// エンジンが無い: 送られた設定で作る（`Unit`）。
    LoadRequested,
    /// エンジンが無く壊れている間: 最後に採用した設定で作る（`CreateKept`）。
    LoadLastAdopted,
    /// エンジンが無く、設定を採用できない（`Rejected(ConfigUnusable)`）。
    Unusable,
}

pub fn decide_create(has_engine: bool, verdict: Verdict) -> CreateDecision {
    match (has_engine, verdict) {
        (true, Verdict::Adopt) => CreateDecision::Restart,
        (true, Verdict::Broken | Verdict::Stale) => CreateDecision::Keep,
        (false, Verdict::Adopt) => CreateDecision::LoadRequested,
        (false, Verdict::Broken) => CreateDecision::LoadLastAdopted,
        (false, Verdict::Stale) => CreateDecision::Unusable,
    }
}

pub fn sha256(bytes: &[u8]) -> ConfigVersion {
    Sha256::digest(bytes).into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `host-config-last.json` の形（設計書 4.3a）。
#[derive(Debug, Serialize, Deserialize)]
struct LastAdoptedFile {
    format: u32,
    /// 採用した EngineConfig の JSON。既定値のときは `None`。
    config_json: Option<String>,
    /// 採用した `config.toml` の本文の SHA-256（16 進）。既定値・不明のときは `None`。
    config_sha256: Option<String>,
    /// `config_json` の SHA-256（16 進。`None` のときは空の本文のハッシュ）。壊れの検出用。
    json_sha256: String,
}

const LAST_ADOPTED_FORMAT: u32 = 1;

fn json_hash(config_json: Option<&str>) -> String {
    hex(&sha256(config_json.unwrap_or("").as_bytes()))
}

/// 最後に採用した設定を書く。一時ファイルに書いてから名前を替える。
pub fn save_last_adopted(
    path: &Path,
    config_json: Option<&str>,
    config_sha256: Option<ConfigVersion>,
) -> anyhow::Result<()> {
    let file = LastAdoptedFile {
        format: LAST_ADOPTED_FORMAT,
        config_json: config_json.map(str::to_string),
        config_sha256: config_sha256.map(|h| hex(&h)),
        json_sha256: json_hash(config_json),
    };
    let body = serde_json::to_vec_pretty(&file)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 最後に採用した設定を読む。無い・形が違う・ハッシュが合わないときは `None`
/// （呼び出し側は既定値で作る）。読めたときは、その JSON（既定値なら `None`）。
pub fn load_last_adopted(path: &Path) -> Option<Option<String>> {
    let body = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("config_check: cannot read {}: {e}", path.display());
            return None;
        }
    };
    let file: LastAdoptedFile = match serde_json::from_slice(&body) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("config_check: {} is not valid: {e}", path.display());
            return None;
        }
    };
    if file.format != LAST_ADOPTED_FORMAT {
        tracing::warn!(
            "config_check: {} has unknown format {}",
            path.display(),
            file.format
        );
        return None;
    }
    if file.json_sha256 != json_hash(file.config_json.as_deref()) {
        tracing::warn!("config_check: {} failed its hash check", path.display());
        return None;
    }
    if let Some(json) = &file.config_json
        && serde_json::from_str::<serde_json::Value>(json).is_err()
    {
        tracing::warn!("config_check: {} holds invalid JSON", path.display());
        return None;
    }
    Some(file.config_json)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H1: ConfigVersion = [1; 32];
    const H2: ConfigVersion = [2; 32];

    #[test]
    fn judge_follows_the_table() {
        use DiskConfig::*;
        use Verdict::*;
        assert_eq!(judge(Some(H1), None, Read(H1)), Adopt);
        assert_eq!(judge(Some(H1), None, Read(H2)), Stale);
        assert_eq!(judge(None, None, Missing), Adopt);
        assert_eq!(judge(None, None, Read(H1)), Stale);
        assert_eq!(judge(Some(H1), None, Missing), Stale);
        assert_eq!(judge(Some(H1), None, Unreadable), Stale);
        assert_eq!(judge(None, None, Unreadable), Stale);
        // 壊れている間: 解析できなかった本文がディスクのものと同じ
        assert_eq!(judge(Some(H1), Some(H2), Read(H2)), Broken);
        assert_eq!(judge(None, Some(H2), Read(H2)), Broken);
        // 別の壊れた本文に替わっていれば古い
        assert_eq!(judge(Some(H1), Some(H2), Read([3; 32])), Stale);
        // 送った設定がディスクと一致すれば、壊れの知らせより採用が先
        assert_eq!(judge(Some(H1), Some(H1), Read(H1)), Adopt);
    }

    #[test]
    fn create_decisions() {
        use CreateDecision::*;
        assert_eq!(decide_create(true, Verdict::Adopt), Restart);
        assert_eq!(decide_create(true, Verdict::Stale), Keep);
        assert_eq!(decide_create(true, Verdict::Broken), Keep);
        assert_eq!(decide_create(false, Verdict::Adopt), LoadRequested);
        assert_eq!(decide_create(false, Verdict::Broken), LoadLastAdopted);
        assert_eq!(decide_create(false, Verdict::Stale), Unusable);
    }

    #[test]
    fn read_disk_distinguishes_missing_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert_eq!(read_disk(Some(&path)), DiskConfig::Missing);
        std::fs::write(&path, b"[general]\n").unwrap();
        assert_eq!(
            read_disk(Some(&path)),
            DiskConfig::Read(sha256(b"[general]\n"))
        );
        // 場所が分からない
        assert_eq!(read_disk(None), DiskConfig::Unreadable);
        // フォルダは本文として読めない
        assert_eq!(read_disk(Some(dir.path())), DiskConfig::Unreadable);
    }

    #[test]
    fn last_adopted_roundtrip_and_validation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("host-config-last.json");
        assert_eq!(load_last_adopted(&path), None, "無ければ None");

        save_last_adopted(&path, Some(r#"{"num_candidates":9}"#), Some(H1)).unwrap();
        assert_eq!(
            load_last_adopted(&path),
            Some(Some(r#"{"num_candidates":9}"#.to_string()))
        );
        // 既定値（JSON 無し）も保存できる
        save_last_adopted(&path, None, None).unwrap();
        assert_eq!(load_last_adopted(&path), Some(None));

        // ハッシュが合わない
        let mut file: LastAdoptedFile =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        file.config_json = Some("{}".into());
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert_eq!(load_last_adopted(&path), None);

        // JSON として読めない設定
        let bad = "not json";
        let file = LastAdoptedFile {
            format: LAST_ADOPTED_FORMAT,
            config_json: Some(bad.into()),
            config_sha256: None,
            json_sha256: json_hash(Some(bad)),
        };
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert_eq!(load_last_adopted(&path), None);

        // 形の違うファイル・壊れたファイル
        std::fs::write(&path, b"{").unwrap();
        assert_eq!(load_last_adopted(&path), None);
        let file = LastAdoptedFile {
            format: 99,
            config_json: None,
            config_sha256: None,
            json_sha256: json_hash(None),
        };
        std::fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();
        assert_eq!(load_last_adopted(&path), None);
    }
}
