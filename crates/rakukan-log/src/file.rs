//! ログのファイル: 起動インスタンスごとの名前、書き込み時のローテーション、掃除（Issue #60）。
//!
//! 旧方式は `rakukan.log` 1 本を全プロセスで共有し、ローテーションを `DllMain` の
//! 中だけで行っていた。Windows では開いたハンドルが rename に追随するため、
//! 別プロセスがローテーションした後も、既に動いていたプロセスは退避された世代へ
//! 書き続けていた。サイズ判定もプロセス起動時にしか走らず、上限を超えて伸びた。
//!
//! ここではファイルをプロセス間で共有せず、起動インスタンスごとに
//! `<接頭辞><PID>-<起動識別子>.log` を持つ。PID の再利用は起動識別子で区別する。
//! サイズ確認・退避・開き直しは書き込み処理の中で行う。書き手は 1 つに限る（#73）。

use std::path::{Path, PathBuf};

pub const LOG_ROTATE_MAX_BYTES: u64 = 16 * 1024 * 1024;
pub const LOG_ROTATE_GENERATIONS: usize = 5;

/// ログ全体の保持目安。
///
/// **厳密な上限ではない。** 使用中のログは消さないので、使用中の分だけで
/// これを超えることがある（その場合は超過を許容する）。
// 掃除は #74 でトレイへ移し、条件を日数・件数に改める。それまで呼び出し元は無い。
pub const LOG_TOTAL_BUDGET_BYTES: u64 = 256 * 1024 * 1024;

/// TSF のログの接頭辞。旧方式の `rakukan.log` / `.1`〜`.5` は接頭辞が違うので
/// 掃除の対象にならない（自動削除は新方式のログだけに適用する）。
pub const TSF_PREFIX: &str = "rakukan-tsf-";

/// エンジンホストのログの接頭辞（#75）。
pub const HOST_PREFIX: &str = "rakukan-engine-host-";

/// `.lock` を排他で取れなかったときに、起動識別子を変えて試し直す回数（#73）。
pub const LOCK_RETRIES: u128 = 4;

/// 起動インスタンスの識別子。PID の再利用を区別する。
///
/// UNIX epoch のミリ秒。名前順がおおむね起動順になり、期間の判定にも使える
/// （ファイルの作成日時は NTFS のトンネリングで当てにならない）。
pub fn startup_id() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// `<PID>-<起動識別子>`。ログ本体とロックの両方の名前に使う。
pub fn instance_stem(pid: u32, id: u128) -> String {
    format!("{pid}-{id}")
}

pub fn instance_log_path(prefix: &str, dir: &Path, stem: &str) -> PathBuf {
    dir.join(format!("{prefix}{stem}.log"))
}

pub fn instance_lock_path(prefix: &str, dir: &Path, stem: &str) -> PathBuf {
    dir.join(format!("{prefix}{stem}.lock"))
}

/// 書き手として確保した起動インスタンス。`lock` をプロセスが終わるまで保持する。
///
/// ロックはログ本体とは別のファイルにする。ローテーションで本体を一時的に閉じている間も
/// 「使用中」を保てるようにするため。
pub struct Instance {
    pub stem: String,
    pub log_path: PathBuf,
    pub lock: std::fs::File,
}

/// `.lock` を排他で取れた起動インスタンスを確保する（#73）。
///
/// 取れなければ起動識別子を変えて [`LOCK_RETRIES`] 回まで試し直し、それでも取れなければ
/// `None`（呼び出し側はログなしで続ける）。ロックはログ本体より先に取る。掃除側はロックを
/// 排他で取れたときだけ消すので、先に取っておかないと起動途中のログが消されうる。
pub fn acquire_instance(prefix: &str, dir: &Path, pid: u32) -> Option<Instance> {
    acquire_instance_from(prefix, dir, pid, startup_id())
}

fn acquire_instance_from(prefix: &str, dir: &Path, pid: u32, base: u128) -> Option<Instance> {
    let _ = std::fs::create_dir_all(dir);
    (0..=LOCK_RETRIES).find_map(|attempt| {
        let stem = instance_stem(pid, base + attempt);
        let lock = open_exclusive(&instance_lock_path(prefix, dir, &stem), true).ok()?;
        Some(Instance {
            log_path: instance_log_path(prefix, dir, &stem),
            stem,
            lock,
        })
    })
}

/// 共有を許さずに開く。開ければ「誰も使っていない」と判断できる。
///
/// `create` は呼び出し側が決める。掃除側は `create=false` にして、ロックが無い
/// インスタンスは判定不能として残す。
pub fn open_exclusive(path: &Path, create: bool) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .share_mode(0)
        .open(path)
}

pub fn rotated_log_path(path: &Path, generation: usize) -> Option<PathBuf> {
    let mut file_name = path.file_name()?.to_os_string();
    file_name.push(format!(".{generation}"));
    Some(path.with_file_name(file_name))
}

/// 退避の途中で本体を置く一時名。`<本体>.log.tmp`。
pub fn staged_log_path(path: &Path) -> Option<PathBuf> {
    let mut file_name = path.file_name()?.to_os_string();
    file_name.push(".tmp");
    Some(path.with_file_name(file_name))
}

/// 退避で次に実行する処理。成功した処理は再試行時に繰り返さない。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RotationStep {
    RemoveOldest,
    MoveTo(usize),
}

/// 既存の退避先を上書きせずに移す。書き手はこのインスタンスだけ。
fn rename_log_to_empty(src: &Path, dst: &Path) -> std::io::Result<()> {
    if dst.try_exists()? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "log rotation destination already exists",
        ));
    }
    std::fs::rename(src, dst)
}

/// 本体を退避し、`.1`〜`.5` を 1 つずつ送る。
///
/// **本体を先に一時名へ移す。** ここで失敗したら世代には一切触らずに諦める。
/// 途中で失敗したら、その処理と `.tmp` を保持して止める。次回は同じ処理から
/// 再開し、既に送った世代や新しく書き始めた本体を動かさない。
/// 状態を持たない既存の `.tmp` は、内容を守るため上書きも削除もしない。
fn shift_generations(path: &Path, pending: &mut Option<RotationStep>) -> std::io::Result<()> {
    let invalid_path = || std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid log path");
    let staged = staged_log_path(path).ok_or_else(invalid_path)?;
    if pending.is_none() {
        rename_log_to_empty(path, &staged)?;
        *pending = Some(RotationStep::RemoveOldest);
    }
    while let Some(step) = *pending {
        match step {
            RotationStep::RemoveOldest => {
                let oldest =
                    rotated_log_path(path, LOG_ROTATE_GENERATIONS).ok_or_else(invalid_path)?;
                match std::fs::remove_file(oldest) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                *pending = Some(RotationStep::MoveTo(LOG_ROTATE_GENERATIONS));
            }
            RotationStep::MoveTo(1) => {
                let dst = rotated_log_path(path, 1).ok_or_else(invalid_path)?;
                rename_log_to_empty(&staged, &dst)?;
                *pending = None;
            }
            RotationStep::MoveTo(generation) => {
                let dst = rotated_log_path(path, generation).ok_or_else(invalid_path)?;
                let src = rotated_log_path(path, generation - 1).ok_or_else(invalid_path)?;
                if src.try_exists()? {
                    rename_log_to_empty(&src, &dst)?;
                }
                *pending = Some(RotationStep::MoveTo(generation - 1));
            }
        }
    }
    Ok(())
}

/// 退避に失敗したあと、次に退避を試すまでに本体が伸びてよい量。
///
/// 失敗するたびに毎回の書き込みで試すと、失敗が続く間ずっと rename を叩く。
const ROTATE_RETRY_STEP_BYTES: u64 = 1024 * 1024;

/// 開き直しに失敗したあと、次に試すまでの間隔。
const REOPEN_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// 書き込みのたびにサイズを見て、上限を超えるならローテーションするログ。
///
/// 書き込み用のスレッド（[`crate::AsyncLog`]）が持つ。サイズ確認・ハンドルを
/// 閉じる・退避・開き直しは、すべて 1 回の `write` の中で行う。
///
/// 空の本体を開いたときは、最初にヘッダーの行を書く（ローテーションの後も、#73）。
pub struct RotatingLog {
    path: PathBuf,
    /// 空の本体を開いたときに 1 行目に書く行（改行なし）。
    header: Option<String>,
    file: Option<std::fs::File>,
    written: u64,
    max_bytes: u64,
    /// `.tmp` の退避が未完了なら、次回ここから再開する。
    rotation_step: Option<RotationStep>,
    /// 退避に失敗した場合の、次に試すサイズ。`None` なら上限超過で即試す。
    rotate_retry_at: Option<u64>,
    /// 開き直しに失敗した場合の、次に試す時刻。
    reopen_retry_at: Option<std::time::Instant>,
    /// 開き直しの再試行間隔。テストから縮められるよう定数を持たせている。
    reopen_retry: std::time::Duration,
}

impl RotatingLog {
    /// 本体を開く。`header` は空の本体を開くたびに 1 行目に書く。
    pub fn open(path: PathBuf, header: Option<String>) -> Self {
        Self::open_with(path, LOG_ROTATE_MAX_BYTES, header)
    }

    #[cfg(test)]
    pub(crate) fn open_with_cap(path: PathBuf, max_bytes: u64) -> Self {
        Self::open_with(path, max_bytes, None)
    }

    fn open_with(path: PathBuf, max_bytes: u64, header: Option<String>) -> Self {
        let mut log = Self {
            path,
            header,
            file: None,
            written: 0,
            max_bytes,
            rotation_step: None,
            rotate_retry_at: None,
            reopen_retry_at: None,
            reopen_retry: REOPEN_RETRY_INTERVAL,
        };
        log.reopen();
        log
    }

    fn reopen(&mut self) {
        self.file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .ok();
        if self.file.is_some() {
            self.reopen_retry_at = None;
        }
        self.written = self
            .file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map(|m| m.len())
            .unwrap_or(0);
        if self.written == 0
            && let (Some(f), Some(header)) = (self.file.as_mut(), self.header.as_deref())
        {
            use std::io::Write;
            let line = format!("{header}\n");
            if f.write_all(line.as_bytes()).is_ok() {
                self.written = line.len() as u64;
            }
        }
    }

    /// 閉じたままなら、間隔を空けて開き直す。
    ///
    /// これが無いと、一時的に開けなかっただけで以後ずっと書き込みを捨て続ける。
    fn ensure_open(&mut self) {
        if self.file.is_some() {
            return;
        }
        let now = std::time::Instant::now();
        if let Some(at) = self.reopen_retry_at
            && now < at
        {
            return;
        }
        self.reopen();
        if self.file.is_none() {
            self.reopen_retry_at = Some(now + self.reopen_retry);
        }
    }

    fn should_rotate(&self, incoming: usize) -> bool {
        if self.written == 0 {
            return false;
        }
        if self.written.saturating_add(incoming as u64) <= self.max_bytes {
            return false;
        }
        // 退避に失敗した直後は、しばらく試さない
        match self.rotate_retry_at {
            Some(next) => self.written >= next,
            None => true,
        }
    }

    /// ハンドルを閉じてから退避し、開き直す。
    fn rotate(&mut self) {
        self.file = None; // 退避の前に必ず閉じる
        let rotated = shift_generations(&self.path, &mut self.rotation_step).is_ok();
        self.reopen();
        if rotated {
            self.rotate_retry_at = None;
        } else {
            // 本体を一時名へ移した場合、開き直した本体のサイズから間隔を数える。
            self.rotate_retry_at = Some(self.written.saturating_add(ROTATE_RETRY_STEP_BYTES));
        }
    }
}

impl std::io::Write for RotatingLog {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.should_rotate(buf.len()) {
            self.rotate();
        }
        self.ensure_open();
        match self.file.as_mut() {
            Some(f) => {
                let n = f.write(buf)?;
                self.written = self.written.saturating_add(n as u64);
                Ok(n)
            }
            // 開けないときは捨てる。ログのために IME を止めない。
            None => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

/// ファイル名 `rakukan-tsf-<PID>-<起動識別子>.log[.N|.tmp]` から `<stem>` を取り出す。
///
/// **このコードが作る名前だけを受け付ける。** 末尾を「空でなければ何でも可」に
/// すると、`rakukan-tsf-1-2.log.backup` のような別のファイルまで掃除の削除対象に
/// なってしまう。
pub fn instance_stem_of<'a>(prefix: &str, file_name: &'a str) -> Option<&'a str> {
    let rest = file_name.strip_prefix(prefix)?;
    let (stem, tail) = rest.split_once(".log")?;
    // 本体（空）、退避世代（`.1`〜`.5`）、退避の途中の一時名（`.tmp`）だけ。
    // `.lock` は `.log` を含まないのでここには来ない。
    if !tail.is_empty() {
        let suffix = tail.strip_prefix('.')?;
        if suffix != "tmp" {
            let generation: usize = suffix.parse().ok()?;
            if generation == 0 || generation > LOG_ROTATE_GENERATIONS {
                return None;
            }
        }
    }
    // `<PID>-<起動識別子>` はどちらも数字
    let (pid, id) = stem.rsplit_once('-')?;
    let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (numeric(pid) && numeric(id)).then_some(stem)
}

/// `<stem>` の中の起動識別子。並べ替えに使う。取れなければ 0（最も古い扱い）。
pub fn startup_id_of(stem: &str) -> u128 {
    stem.rsplit_once('-')
        .and_then(|(_, id)| id.parse::<u128>().ok())
        .unwrap_or(0)
}

/// 掃除で数える起動インスタンス 1 件分。
struct LogInstance {
    startup_id: u128,
    bytes: u64,
    files: Vec<PathBuf>,
}

impl LogInstance {
    fn new(startup_id: u128) -> Self {
        Self {
            startup_id,
            bytes: 0,
            files: Vec::new(),
        }
    }
}

/// 使用の終わったログを古い順に消して、全体を保持目安へ近づける（Issue #60）。
///
/// 削除してよいかは、起動インスタンスごとのロックを排他で取得できるかだけで決める。
/// PID の生存では判断しない（PID の再利用と、確認してから消すまでの状態変化を
/// 避けるため）。取得できない場合と、ロックが無くて判定できない場合は残す。
/// 削除が終わるまでロックのハンドルを保持する。
///
/// 使用中のログだけで目安を超える場合は何も消さない。目安は厳密な上限ではない。
pub fn cleanup_logs(prefix: &str, dir: &Path, own_stem: &str) {
    cleanup_logs_with_budget(prefix, dir, own_stem, LOG_TOTAL_BUDGET_BYTES);
}

pub fn cleanup_logs_with_budget(prefix: &str, dir: &Path, own_stem: &str, budget: u64) {
    use std::collections::HashMap;

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut total: u64 = 0;
    let mut instances: HashMap<String, LogInstance> = HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(stem) = path
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|name| instance_stem_of(prefix, name))
        else {
            continue;
        };
        let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
        total = total.saturating_add(len);
        let slot = instances
            .entry(stem.to_string())
            .or_insert_with(|| LogInstance::new(startup_id_of(stem)));
        slot.bytes = slot.bytes.saturating_add(len);
        slot.files.push(path);
    }
    if total <= budget {
        return;
    }

    let mut victims: Vec<(String, LogInstance)> = instances
        .into_iter()
        .filter(|(stem, _)| stem != own_stem)
        .collect();
    victims.sort_by_key(|(_, inst)| inst.startup_id); // 古い順

    for (stem, inst) in victims {
        if total <= budget {
            break;
        }
        let lock = instance_lock_path(prefix, dir, &stem);
        // 排他で取れたときだけ「使用が終わっている」と判断する。
        // ロックが無い（create=false で開けない）インスタンスは判定不能として残す。
        let Ok(guard) = open_exclusive(&lock, false) else {
            continue;
        };
        let mut removed: u64 = 0;
        let mut all_removed = true;
        for f in &inst.files {
            let len = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(f).is_ok() {
                removed = removed.saturating_add(len);
            } else {
                all_removed = false;
            }
        }
        drop(guard); // 削除が終わってから手放す
        // **ログが残ったらロックも残す。** 先にロックだけ消すと、次回は
        // 「ロックが無いので判定できない」分岐に入り、消せる状態になっても
        // 二度と回収されなくなる。
        if all_removed {
            let _ = std::fs::remove_file(&lock);
        }
        total = total.saturating_sub(removed);
        tracing::debug!(
            "log cleanup: removed instance {stem} ({} bytes)",
            inst.bytes
        );
    }
}

#[cfg(test)]
mod log_rotation_tests {
    //! プロセス別ログとローテーション・掃除（Issue #60）
    use super::*;
    use std::io::Write;

    /// テスト用の作業ディレクトリ。名前が衝突しないよう時刻と連番で分ける。
    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rakukan-log-test-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    pub(crate) fn write_file(path: &Path, len: usize) {
        std::fs::write(path, vec![b'x'; len]).expect("write");
    }

    fn total_len(dir: &Path) -> u64 {
        std::fs::read_dir(dir)
            .expect("read_dir")
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .and_then(|n| instance_stem_of(TSF_PREFIX, n))
                    .is_some()
            })
            .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
            .sum()
    }

    #[test]
    fn instance_stem_is_taken_from_log_files_only() {
        assert_eq!(
            instance_stem_of(TSF_PREFIX, "rakukan-tsf-1234-999.log"),
            Some("1234-999")
        );
        assert_eq!(
            instance_stem_of(TSF_PREFIX, "rakukan-tsf-1234-999.log.3"),
            Some("1234-999")
        );
        // ロックは対象外（掃除のサイズ集計に入れない）
        assert_eq!(
            instance_stem_of(TSF_PREFIX, "rakukan-tsf-1234-999.lock"),
            None
        );
        // 旧方式と他コンポーネントのログは接頭辞が違うので対象外
        assert_eq!(instance_stem_of(TSF_PREFIX, "rakukan.log"), None);
        assert_eq!(instance_stem_of(TSF_PREFIX, "rakukan.log.1"), None);
        assert_eq!(
            instance_stem_of(TSF_PREFIX, "rakukan-engine-host.log"),
            None
        );
        assert_eq!(instance_stem_of(TSF_PREFIX, "rakukan-engine-dll.log"), None);
    }

    #[test]
    fn startup_id_orders_instances() {
        assert_eq!(startup_id_of("1234-999"), 999);
        // PID が同じでも起動識別子で区別できる
        assert!(startup_id_of("1234-1000") > startup_id_of("1234-999"));
        // 取れなければ最も古い扱い
        assert_eq!(startup_id_of("こわれている"), 0);
    }

    #[test]
    fn rotating_log_rotates_during_writes() {
        let dir = temp_dir("rotate");
        let path = instance_log_path(TSF_PREFIX, &dir, "1-1");
        let mut log = RotatingLog::open_with_cap(path.clone(), 64);

        // 上限までは本体に積む
        log.write_all(&[b'a'; 40]).expect("write");
        assert!(!rotated_log_path(&path, 1).expect("gen").exists());

        // 上限を超える書き込みで退避してから書く
        log.write_all(&[b'b'; 40]).expect("write");
        let gen1 = rotated_log_path(&path, 1).expect("gen");
        assert!(gen1.exists(), "退避されていない");
        assert_eq!(std::fs::metadata(&gen1).expect("meta").len(), 40);
        assert_eq!(std::fs::metadata(&path).expect("meta").len(), 40);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_header_starts_every_new_body() {
        let dir = temp_dir("header");
        let path = instance_log_path(TSF_PREFIX, &dir, "1-1");
        let mut log = RotatingLog::open_with(path.clone(), 64, Some("HEADER".to_string()));
        log.write_all(&[b'a'; 40]).expect("write");
        log.write_all(&[b'b'; 40]).expect("write");
        // 退避した世代と、新しく開いた本体の両方の 1 行目がヘッダー
        let gen1 = std::fs::read(rotated_log_path(&path, 1).expect("gen")).expect("read");
        let body = std::fs::read(&path).expect("read");
        assert!(
            gen1.starts_with(
                b"HEADER
"
            ),
            "{gen1:?}"
        );
        assert!(
            body.starts_with(
                b"HEADER
"
            ),
            "{body:?}"
        );
        assert!(body.ends_with(&[b'b'; 40]));
        drop(log);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_held_lock_is_skipped_by_changing_the_startup_id() {
        let dir = temp_dir("acquire");
        let first = acquire_instance_from(TSF_PREFIX, &dir, 7, 100).expect("first");
        let second = acquire_instance_from(TSF_PREFIX, &dir, 7, 100).expect("second");
        assert_eq!(first.stem, "7-100");
        assert_eq!(second.stem, "7-101");
        assert_eq!(
            second.log_path,
            instance_log_path(TSF_PREFIX, &dir, "7-101")
        );
        drop((first, second));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_instance_when_every_retry_is_held() {
        let dir = temp_dir("acquire-full");
        let held: Vec<Instance> = (0..=LOCK_RETRIES)
            .map(|_| acquire_instance_from(TSF_PREFIX, &dir, 7, 100).expect("held"))
            .collect();
        assert_eq!(held.len() as u128, LOCK_RETRIES + 1);
        // 書き手を 2 つにしない: 取れなければログなし
        assert!(acquire_instance_from(TSF_PREFIX, &dir, 7, 100).is_none());
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_closes_the_handle_before_renaming() {
        let dir = temp_dir("handle");
        let path = instance_log_path(TSF_PREFIX, &dir, "1-1");
        let mut log = RotatingLog::open_with_cap(path.clone(), 16);
        log.write_all(&[b'a'; 16]).expect("write");
        log.write_all(&[b'b'; 16]).expect("write");
        // 退避後の本体は新しいハンドルで、退避側は誰も掴んでいない
        // （掴んだままなら share_mode(0) で開けない）
        let gen1 = rotated_log_path(&path, 1).expect("gen");
        assert!(open_exclusive(&gen1, false).is_ok(), "退避側が開いたまま");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_does_nothing_under_the_budget() {
        let dir = temp_dir("under");
        write_file(&instance_log_path(TSF_PREFIX, &dir, "1-100"), 100);
        cleanup_logs_with_budget(TSF_PREFIX, &dir, "9-999", 1_000);
        assert!(instance_log_path(TSF_PREFIX, &dir, "1-100").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_keeps_instances_whose_lock_is_held() {
        let dir = temp_dir("held");
        // 使用中のインスタンス（ロックを保持したまま）
        write_file(&instance_log_path(TSF_PREFIX, &dir, "1-100"), 400);
        let held =
            open_exclusive(&instance_lock_path(TSF_PREFIX, &dir, "1-100"), true).expect("lock");

        cleanup_logs_with_budget(TSF_PREFIX, &dir, "9-999", 100);

        // 使用中なので消えない。目安を超えたままでも消さない
        assert!(instance_log_path(TSF_PREFIX, &dir, "1-100").exists());
        assert!(total_len(&dir) > 100);
        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_keeps_instances_without_a_lock() {
        let dir = temp_dir("nolock");
        // ロックが無い = 判定できない → 残す
        write_file(&instance_log_path(TSF_PREFIX, &dir, "1-100"), 400);
        cleanup_logs_with_budget(TSF_PREFIX, &dir, "9-999", 100);
        assert!(instance_log_path(TSF_PREFIX, &dir, "1-100").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_removes_finished_instances_oldest_first() {
        let dir = temp_dir("oldest");
        for (stem, len) in [("1-100", 200), ("1-200", 200), ("1-300", 200)] {
            write_file(&instance_log_path(TSF_PREFIX, &dir, stem), len);
            // 使用が終わったインスタンスはロックだけ残る
            drop(open_exclusive(&instance_lock_path(TSF_PREFIX, &dir, stem), true).expect("lock"));
        }
        // 退避世代も同じインスタンスの分として数える
        write_file(
            &rotated_log_path(&instance_log_path(TSF_PREFIX, &dir, "1-100"), 1).expect("gen"),
            100,
        );

        // 合計 700 を 450 以下にする → 古い 1-100（300）だけ消せば足りる
        cleanup_logs_with_budget(TSF_PREFIX, &dir, "9-999", 450);

        assert!(
            !instance_log_path(TSF_PREFIX, &dir, "1-100").exists(),
            "最も古い分が残った"
        );
        assert!(
            !rotated_log_path(&instance_log_path(TSF_PREFIX, &dir, "1-100"), 1)
                .expect("gen")
                .exists(),
            "退避世代が残った"
        );
        assert!(
            !instance_lock_path(TSF_PREFIX, &dir, "1-100").exists(),
            "ロックが残った"
        );
        assert!(
            instance_log_path(TSF_PREFIX, &dir, "1-200").exists(),
            "消しすぎている"
        );
        assert!(
            instance_log_path(TSF_PREFIX, &dir, "1-300").exists(),
            "消しすぎている"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_never_touches_its_own_instance_or_legacy_logs() {
        let dir = temp_dir("own");
        write_file(&instance_log_path(TSF_PREFIX, &dir, "9-999"), 400);
        drop(open_exclusive(&instance_lock_path(TSF_PREFIX, &dir, "9-999"), true).expect("lock"));
        // 旧方式のログ（接頭辞が違う）
        write_file(&dir.join("rakukan.log"), 400);
        write_file(&dir.join("rakukan.log.1"), 400);

        cleanup_logs_with_budget(TSF_PREFIX, &dir, "9-999", 10);

        // 自分のインスタンスはロックが空いていても対象外
        assert!(instance_log_path(TSF_PREFIX, &dir, "9-999").exists());
        // 旧方式は自動削除の対象外
        assert!(dir.join("rakukan.log").exists());
        assert!(dir.join("rakukan.log.1").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod log_rotation_failure_tests {
    //! 異常系（レビュー指摘 1〜4 の再発防止）
    use super::log_rotation_tests::{temp_dir, write_file};
    use super::*;
    use std::io::Write;

    /// 削除共有なしで掴む。この状態のファイルは rename も削除もできない。
    fn hold_without_delete_share(path: &Path) -> std::fs::File {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ_WRITE: u32 = 0x0000_0001 | 0x0000_0002;
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ_WRITE)
            .open(path)
            .expect("hold")
    }

    #[test]
    fn rotation_failure_keeps_every_generation() {
        let dir = temp_dir("failkeep");
        let path = instance_log_path(TSF_PREFIX, &dir, "1-1");
        for g in 1..=LOG_ROTATE_GENERATIONS {
            write_file(&rotated_log_path(&path, g).expect("gen"), 10);
        }
        write_file(&path, 10);
        // 本体を rename できない状態にする
        let held = hold_without_delete_share(&path);

        let mut log = RotatingLog::open_with_cap(path.clone(), 16);
        for _ in 0..5 {
            log.write_all(&[b'x'; 16]).expect("write");
        }

        // 退避世代が 1 つも失われていないこと（先に世代を送ると 5 回で全部消える）
        for g in 1..=LOG_ROTATE_GENERATIONS {
            assert!(
                rotated_log_path(&path, g).expect("gen").exists(),
                "退避世代 .{g} が失われた"
            );
            assert_eq!(
                std::fs::metadata(rotated_log_path(&path, g).expect("gen"))
                    .expect("meta")
                    .len(),
                10,
                "退避世代 .{g} が繰り上がっている"
            );
        }
        // 一時名も残していない
        assert!(!staged_log_path(&path).expect("staged").exists());

        drop(held);
        drop(log);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_is_not_retried_on_every_write_after_a_failure() {
        let dir = temp_dir("backoff");
        let path = instance_log_path(TSF_PREFIX, &dir, "1-1");
        write_file(&path, 10);
        let held = hold_without_delete_share(&path);

        let mut log = RotatingLog::open_with_cap(path.clone(), 16);
        log.write_all(&[b'x'; 16]).expect("write");
        // 1 回目の失敗で、次に試すサイズが先に置かれる
        let first = log.rotate_retry_at.expect("失敗を覚えていない");
        log.write_all(&[b'x'; 16]).expect("write");
        assert_eq!(
            log.rotate_retry_at,
            Some(first),
            "毎回の書き込みで試している"
        );

        drop(held);
        drop(log);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_resumes_at_each_locked_generation_without_losing_history() {
        for locked_generation in 1..=LOG_ROTATE_GENERATIONS {
            let dir = temp_dir(&format!("generation-{locked_generation}"));
            let path = instance_log_path(TSF_PREFIX, &dir, "1-1");
            for g in 1..=LOG_ROTATE_GENERATIONS {
                std::fs::write(
                    rotated_log_path(&path, g).expect("gen"),
                    format!("generation-{g}"),
                )
                .expect("write generation");
            }
            let mut log = RotatingLog::open_with_cap(path.clone(), 16);
            log.write_all(b"original-content").expect("write");
            let held = hold_without_delete_share(
                &rotated_log_path(&path, locked_generation).expect("gen"),
            );
            log.write_all(b"new").expect("write after failure");

            let step = log.rotation_step.expect("must retain failed step");
            for _ in 0..3 {
                // 間隔が経過したときの再試行を直接呼ぶ。進んだ世代を再度動かさない。
                log.rotate();
                assert_eq!(log.rotation_step, Some(step));
                assert_eq!(
                    std::fs::read(staged_log_path(&path).expect("staged")).expect("read"),
                    b"original-content"
                );
                for g in 1..LOG_ROTATE_GENERATIONS {
                    let at = if g <= locked_generation { g } else { g + 1 };
                    assert_eq!(
                        std::fs::read(rotated_log_path(&path, at).expect("gen")).expect("read"),
                        format!("generation-{g}").as_bytes(),
                        "locked={locked_generation}, original generation={g}"
                    );
                }
                if locked_generation == LOG_ROTATE_GENERATIONS {
                    assert_eq!(
                        std::fs::read(
                            rotated_log_path(&path, LOG_ROTATE_GENERATIONS).expect("gen")
                        )
                        .expect("read"),
                        b"generation-5"
                    );
                }
            }

            drop(held);
            log.rotate();
            assert_eq!(log.rotation_step, None);
            assert!(!staged_log_path(&path).expect("staged").exists());
            assert_eq!(std::fs::read(&path).expect("read current"), b"new");
            assert_eq!(
                std::fs::read(rotated_log_path(&path, 1).expect("gen")).expect("read"),
                b"original-content"
            );
            for g in 2..=LOG_ROTATE_GENERATIONS {
                assert_eq!(
                    std::fs::read(rotated_log_path(&path, g).expect("gen")).expect("read"),
                    format!("generation-{}", g - 1).as_bytes()
                );
            }
            drop(log);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn staged_log_survives_failed_final_rename_and_is_recovered() {
        let dir = temp_dir("staged-recovery");
        let path = instance_log_path(TSF_PREFIX, &dir, "1-1");
        let gen1 = rotated_log_path(&path, 1).expect("gen");
        std::fs::write(&gen1, b"older").expect("write");
        let held_generation = hold_without_delete_share(&gen1);
        let mut log = RotatingLog::open_with_cap(path.clone(), 16);
        log.write_all(b"original-content").expect("write");
        log.write_all(b"new").expect("write after failure");

        let staged = staged_log_path(&path).expect("staged");
        let held_staged = hold_without_delete_share(&staged);
        drop(held_generation);
        log.rotate(); // 世代の移動は終わるが、最後の .tmp → .1 が失敗する。
        assert_eq!(log.rotation_step, Some(RotationStep::MoveTo(1)));
        let next = log.rotate_retry_at.expect("retry threshold");
        let fill = (next - log.written) as usize;
        log.write_all(&vec![b'x'; fill]).expect("grow current");
        log.write_all(b"retry").expect("retry rotation");
        assert_eq!(log.rotation_step, Some(RotationStep::MoveTo(1)));
        assert_eq!(std::fs::read(&staged).expect("read"), b"original-content");
        assert_eq!(
            std::fs::read(rotated_log_path(&path, 2).expect("gen")).expect("read"),
            b"older"
        );

        let current = std::fs::read(&path).expect("read current");
        drop(held_staged);
        log.rotate();
        assert_eq!(log.rotation_step, None);
        assert!(!staged.exists());
        assert_eq!(std::fs::read(&gen1).expect("read"), b"original-content");
        assert_eq!(std::fs::read(&path).expect("read current"), current);
        drop(log);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_preserves_a_staged_log_without_progress_state() {
        let dir = temp_dir("unknown-stage");
        let path = instance_log_path(TSF_PREFIX, &dir, "1-1");
        let staged = staged_log_path(&path).expect("staged");
        let gen1 = rotated_log_path(&path, 1).expect("gen");
        std::fs::write(&staged, b"unfinished").expect("write staged");
        std::fs::write(&gen1, b"older").expect("write generation");
        let mut log = RotatingLog::open_with_cap(path.clone(), 16);
        log.write_all(b"original-content").expect("write");
        log.write_all(b"new").expect("write after failure");
        log.rotate();
        assert_eq!(log.rotation_step, None);
        assert_eq!(std::fs::read(&staged).expect("read"), b"unfinished");
        assert_eq!(std::fs::read(&gen1).expect("read"), b"older");
        assert_eq!(std::fs::read(&path).expect("read"), b"original-contentnew");
        drop(log);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_keeps_the_lock_when_a_log_cannot_be_removed() {
        let dir = temp_dir("lockkeep");
        let stem = "1-100";
        let log = instance_log_path(TSF_PREFIX, &dir, stem);
        write_file(&log, 400);
        drop(open_exclusive(&instance_lock_path(TSF_PREFIX, &dir, stem), true).expect("lock"));
        // 削除できない状態にする
        let held = hold_without_delete_share(&log);

        cleanup_logs_with_budget(TSF_PREFIX, &dir, "9-999", 100);

        assert!(log.exists(), "消えないはずのログが消えた");
        // ロックを消すと、次回は「ロックが無いので判定できない」に落ちて
        // 消せる状態になっても二度と回収されなくなる
        assert!(
            instance_lock_path(TSF_PREFIX, &dir, stem).exists(),
            "ログが残ったのにロックを消している"
        );

        drop(held);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_generated_names_are_targeted() {
        // 生成しない名前は掃除の対象にしない
        assert_eq!(
            instance_stem_of(TSF_PREFIX, "rakukan-tsf-1-2.log.backup"),
            None
        );
        assert_eq!(instance_stem_of(TSF_PREFIX, "rakukan-tsf-1-2.log.0"), None);
        assert_eq!(instance_stem_of(TSF_PREFIX, "rakukan-tsf-1-2.log.6"), None);
        assert_eq!(
            instance_stem_of(TSF_PREFIX, "rakukan-tsf-1-2.log.old"),
            None
        );
        // PID・起動識別子が数字でないものも対象外
        assert_eq!(instance_stem_of(TSF_PREFIX, "rakukan-tsf-abc-2.log"), None);
        assert_eq!(instance_stem_of(TSF_PREFIX, "rakukan-tsf-1-x.log"), None);
        assert_eq!(instance_stem_of(TSF_PREFIX, "rakukan-tsf-1.log"), None);
        // 生成する名前は受け付ける（退避の途中の一時名を含む）
        assert_eq!(
            instance_stem_of(TSF_PREFIX, "rakukan-tsf-1-2.log"),
            Some("1-2")
        );
        assert_eq!(
            instance_stem_of(TSF_PREFIX, "rakukan-tsf-1-2.log.5"),
            Some("1-2")
        );
        assert_eq!(
            instance_stem_of(TSF_PREFIX, "rakukan-tsf-1-2.log.tmp"),
            Some("1-2")
        );
    }

    #[test]
    fn cleanup_does_not_remove_unrelated_files() {
        let dir = temp_dir("unrelated");
        let keep = dir.join("rakukan-tsf-1-100.log.backup");
        write_file(&keep, 400);
        write_file(&instance_log_path(TSF_PREFIX, &dir, "1-100"), 400);
        drop(open_exclusive(&instance_lock_path(TSF_PREFIX, &dir, "1-100"), true).expect("lock"));

        cleanup_logs_with_budget(TSF_PREFIX, &dir, "9-999", 10);

        assert!(keep.exists(), ".log.backup が消された");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn writes_resume_after_the_file_can_be_opened_again() {
        let dir = temp_dir("resume");
        let missing = dir.join("missing");
        let path = instance_log_path(TSF_PREFIX, &missing, "1-1");
        let mut log = RotatingLog::open_with_cap(path.clone(), 1024);
        log.reopen_retry = std::time::Duration::ZERO; // 待たずに試す

        // 開けないので捨てるが、呼び出し側にはエラーを返さない
        log.write_all(b"dropped\n").expect("write");
        assert!(!path.exists());

        // 開ける状態に戻したら、次の書き込みで復旧する
        std::fs::create_dir_all(&missing).expect("mkdir");
        log.write_all(b"recovered\n").expect("write");
        drop(log);
        let body = std::fs::read_to_string(&path).expect("read");
        assert!(body.contains("recovered"), "復旧していない: {body:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
