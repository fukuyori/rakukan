//! DictStore — 辞書統合ルックアップ
//!
//! 優先順位: ユーザー辞書 > 学習履歴 (mozc 候補の押し上げ) > LLM候補 > mozc バイナリ辞書
//!
//! # スレッド安全性
//! `user` / `learn_history` は `RwLock<HashMap>` で保護する。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::mozc_dict::MozcDict;
use crate::user_dict::UserDict;

/// 学習履歴に保持するエントリ数の上限。mozc の kLruCacheSize に合わせて 30,000 件。
/// これを超える場合は `last_access_time` が最古のエントリから削除する。
const LEARN_LRU_CAPACITY: usize = 30_000;

/// 学習履歴の stale 判定閾値（日）。最終確定からこの日数を超えたエントリは
/// 起動時の `load_learn_history_file` で削除される。
///
/// azooKey の 128 日（無使用で除外）を参考にしつつ、rakukan の 30 日半減期スコアと
/// 組み合わせて 180 日を採用。180 日 = 6 半減期 → 元の重みの約 1.6% まで減衰
/// しており、実質的な可視性はほぼ失われた状態でのハードカット。
const STALE_ENTRY_MAX_AGE_DAYS: u64 = 180;

/// bincode ファイルのフォーマットバージョン。破壊的変更時にインクリメントする。
const LEARN_HISTORY_FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct DictResult {
    pub candidates: Vec<String>,
    pub source: DictSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DictSource {
    User,
    Mozc,
    Merged,
    None,
}

/// 学習履歴 1 エントリ。`(reading, surface)` ペアごとに 1 つ。
///
/// スコアは mozc の `UserHistoryPredictor::GetScore` を参考に、
/// `last_access_time` を軸に頻度と文字数で微調整する。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnEntry {
    pub surface: String,
    /// 最終確定時刻 (unix 秒)
    pub last_access_time: u64,
    /// 確定回数。時間減衰と組み合わせて頻度ボーナスに使う。
    pub suggestion_freq: u32,
    /// 候補ウィンドウで表示された回数（Phase 2c の未選択エントリ削除用、当面 0 のまま）
    pub shown_freq: u32,
}

/// 半減期 (日)。確定からこの日数経過するたびに `suggestion_freq` の重みが半分になる。
const LEARN_HALF_LIFE_DAYS: f64 = 30.0;
/// 頻度項の重み。`1 freq` を「1 日分の last_access_time」に換算。
const LEARN_W_FREQ: f64 = 86_400.0;

impl LearnEntry {
    /// mozc 準拠のスコア。大きいほど上位。
    ///
    /// `score = last_access_time + W_FREQ * freq * 0.5^(Δdays/30) - chars_count`
    pub fn score(&self, now: u64) -> f64 {
        let dt_secs = now.saturating_sub(self.last_access_time) as f64;
        let dt_days = dt_secs / 86_400.0;
        let decay = 0.5_f64.powf(dt_days / LEARN_HALF_LIFE_DAYS);
        let chars_penalty = self.surface.chars().count() as f64;
        self.last_access_time as f64 + LEARN_W_FREQ * (self.suggestion_freq as f64) * decay
            - chars_penalty
    }
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn user_dict_file_state(path: &Path) -> UserDictFileState {
    match std::fs::metadata(path) {
        Ok(meta) => UserDictFileState {
            modified: meta.modified().ok(),
            len: Some(meta.len()),
        },
        Err(_) => UserDictFileState::default(),
    }
}

/// `surface` の各文字が辞書ガードなしで学習を許可してよいかを判定するヘルパー。
///
/// **ひらがな**は読み相当なので surface としての学習を許可しない。
/// **CJK 漢字**は LLM 誤変換リスクがあるため辞書ガードが必要。
/// 上記以外（カタカナ・英数字・記号・半角カタカナ等）は誤変換リスクが低いため、
/// 辞書に存在しなくても学習を許可する。
///
/// これにより以下のケースが学習対象になる:
/// - F7 カタカナ変換: `はろー → ハロー`
/// - F8 半角カタカナ変換: `はろー → ﾊﾛｰ`
/// - F9 全角英数変換: `abc → ａｂｃ`
/// - F10 半角英数変換: `ＡＢＣ → ABC`
/// - 括弧・記号ペア: `『』` `《》` `«»` `【】` 等
#[inline]
fn is_learnable_without_dict(c: char) -> bool {
    let n = c as u32;
    // ひらがな (U+3041–U+309F): reading 相当のため surface として学習しない
    if (0x3041..=0x309F).contains(&n) {
        return false;
    }
    // CJK 統合漢字 (U+4E00–U+9FFF): LLM 誤変換リスクあり
    if (0x4E00..=0x9FFF).contains(&n) {
        return false;
    }
    // CJK 統合漢字拡張 A (U+3400–U+4DBF): 同上
    if (0x3400..=0x4DBF).contains(&n) {
        return false;
    }
    // 上記以外（カタカナ・英数・記号・半角カタカナ・全角英数等）は学習許可
    true
}

struct DictStoreInner {
    /// ユーザー辞書（手動登録のみ）。Phase 2b 以降は `learn()` で更新しない。
    user: RwLock<HashMap<String, Vec<String>>>,
    /// ユーザー辞書ファイル。設定画面や外部エディタで編集された場合の
    /// hot reload 判定に使う。
    user_path: Option<PathBuf>,
    user_file_state: RwLock<UserDictFileState>,
    mozc: Option<MozcDict>,
    /// 学習履歴。`learn()` で更新され、`lookup_learn` で score 降順に並べて返す。
    learn_history: RwLock<HashMap<String, Vec<LearnEntry>>>,
    /// 学習履歴ファイルパス。`None` なら永続化しない（テスト用）。
    learn_history_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct UserDictFileState {
    modified: Option<SystemTime>,
    len: Option<u64>,
}

unsafe impl Send for DictStoreInner {}
unsafe impl Sync for DictStoreInner {}

#[derive(Clone)]
pub struct DictStore {
    inner: Arc<DictStoreInner>,
}

/// 学習履歴ファイルのバイナリ形式（bincode）。
#[derive(Serialize, Deserialize, Default)]
struct LearnHistoryFile {
    version: u32,
    entries: HashMap<String, Vec<LearnEntry>>,
}

/// 書き込み用（clone 回避のため参照版を別定義）
#[derive(Serialize)]
struct LearnHistoryFileRef<'a> {
    version: u32,
    entries: &'a HashMap<String, Vec<LearnEntry>>,
}

impl DictStore {
    /// 各辞書を読み込んで DictStore を構築する
    ///
    /// - `user_path`: 手動登録ユーザー辞書 (`user_dict.toml`)
    /// - `mozc_path`: MOZC バイナリ辞書 (`rakukan.dict`)
    /// - `learn_history_path`: 学習履歴 (`learn_history.bin`)。`None` なら永続化しない
    pub fn load(
        user_path: Option<&Path>,
        mozc_path: Option<&Path>,
        learn_history_path: Option<&Path>,
    ) -> Result<Self> {
        let user_file_state = user_path.map(user_dict_file_state).unwrap_or_default();
        // ユーザー辞書: 失敗しても空で続行（パスエラー・パースエラー問わず）
        let user = if let Some(p) = user_path {
            match UserDict::load(p) {
                Ok(ud) => ud.to_map(),
                Err(e) => {
                    warn!("user_dict load failed ({}): {}", p.display(), e);
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        // mozc辞書: 失敗しても None で続行
        let mozc = if let Some(p) = mozc_path {
            if p.exists() {
                match MozcDict::open(p) {
                    Ok(d) => {
                        info!(
                            "dict::store: mozc loaded readings={} entries={}",
                            d.n_readings(),
                            d.n_entries()
                        );
                        Some(d)
                    }
                    Err(e) => {
                        warn!(
                            "dict::store: mozc load failed path={} size={}B err={}",
                            p.display(),
                            std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
                            e
                        );
                        None
                    }
                }
            } else {
                warn!("dict::store: rakukan.dict not found path={}", p.display());
                None
            }
        } else {
            warn!("dict::store: mozc_path is None (dict_dir unavailable)");
            None
        };

        // 学習履歴: ファイルが無ければ空で開始、破損していてもログだけ出して続行。
        let learn_history = if let Some(p) = learn_history_path {
            match load_learn_history_file(p) {
                Ok(map) => {
                    info!(
                        "dict::store: learn_history loaded entries={} path={}",
                        map.values().map(|v| v.len()).sum::<usize>(),
                        p.display()
                    );
                    map
                }
                Err(e) => {
                    warn!(
                        "dict::store: learn_history load failed ({}): {} — starting empty",
                        p.display(),
                        e
                    );
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

        info!(
            "dict::store: ready user_entries={} mozc={} mozc_path={:?}",
            user.len(),
            if mozc.is_some() { "loaded" } else { "none" },
            mozc_path.map(|p| p.display().to_string()),
        );

        Ok(Self {
            inner: Arc::new(DictStoreInner {
                user: RwLock::new(user),
                user_path: user_path.map(|p| p.to_path_buf()),
                user_file_state: RwLock::new(user_file_state),
                mozc,
                learn_history: RwLock::new(learn_history),
                learn_history_path: learn_history_path.map(|p| p.to_path_buf()),
            }),
        })
    }

    pub fn empty() -> Self {
        Self {
            inner: Arc::new(DictStoreInner {
                user: RwLock::new(HashMap::new()),
                user_path: None,
                user_file_state: RwLock::new(UserDictFileState::default()),
                mozc: None,
                learn_history: RwLock::new(HashMap::new()),
                learn_history_path: None,
            }),
        }
    }

    /// `user_dict.toml` が設定画面や外部エディタで更新されていれば、ユーザー辞書だけ
    /// hot reload する。parse に失敗した場合は現在の辞書を保持する。
    pub fn reload_user_if_changed(&self) -> bool {
        let Some(path) = &self.inner.user_path else {
            return false;
        };
        let current_state = user_dict_file_state(path);
        if self
            .inner
            .user_file_state
            .read()
            .is_ok_and(|state| *state == current_state)
        {
            return false;
        }

        let loaded = match UserDict::load(path) {
            Ok(ud) => ud.to_map(),
            Err(e) => {
                warn!(
                    "user_dict hot reload failed ({}): {}; keeping previous entries",
                    path.display(),
                    e
                );
                if let Ok(mut state) = self.inner.user_file_state.write() {
                    *state = current_state;
                }
                return false;
            }
        };
        let count = loaded.len();
        if let Ok(mut user) = self.inner.user.write() {
            *user = loaded;
        } else {
            warn!("user_dict hot reload failed: user lock poisoned");
            return false;
        }
        if let Ok(mut state) = self.inner.user_file_state.write() {
            *state = current_state;
        }
        info!(
            "user_dict hot reloaded entries={} path={}",
            count,
            path.display()
        );
        true
    }

    /// 確定した候補を学習履歴に記録する。
    ///
    /// 学習対象は MOZC 辞書またはユーザー辞書に `(reading → surface)` が存在する候補のみ。
    /// LLM 由来や数字/リテラル由来の surface は dict lookup にヒットせず、学習をスキップする。
    ///
    /// 動作:
    /// - `learn_history[reading]` に `LearnEntry` を追加 or 既存エントリを更新。
    /// - 既存エントリ: `last_access_time = now`, `suggestion_freq += 1`。
    /// - `user_dict.toml` には一切書き込まない（Phase 2b 以降、手動登録専用）。
    /// - 更新後に `learn_history.bin` へ同期書き込みする（確定時に数 ms 程度の I/O）。
    pub fn learn(&self, reading: &str, surface: &str) {
        if !self.is_dict_surface(reading, surface) {
            debug!(
                "dict::store: learn skipped (not in dict) reading={:?} surface={:?}",
                reading, surface
            );
            return;
        }
        self.learn_inner(reading, surface);
    }

    /// 辞書ガードなしで学習する。候補ウィンドウからの明示選択（案C）専用。
    /// `is_dict_surface` チェックをスキップし、LLM 由来の surface も記録する。
    pub fn learn_force(&self, reading: &str, surface: &str) {
        self.learn_inner(reading, surface);
    }

    fn learn_inner(&self, reading: &str, surface: &str) {
        let now = now_unix_secs();
        let snapshot = {
            let Ok(mut hist) = self.inner.learn_history.write() else {
                warn!("learn_history write lock failed");
                return;
            };
            let entries = hist.entry(reading.to_string()).or_default();
            if let Some(e) = entries.iter_mut().find(|e| e.surface == surface) {
                e.last_access_time = now;
                e.suggestion_freq = e.suggestion_freq.saturating_add(1);
            } else {
                entries.push(LearnEntry {
                    surface: surface.to_string(),
                    last_access_time: now,
                    suggestion_freq: 1,
                    shown_freq: 0,
                });
            }
            trim_learn_history_to_capacity(&mut hist, LEARN_LRU_CAPACITY);
            hist.clone()
        };
        debug!(
            "dict::store: learned reading={:?} surface={:?}",
            reading, surface
        );
        if let Some(path) = &self.inner.learn_history_path {
            if let Err(e) = save_learn_history_file(path, &snapshot) {
                warn!("learn_history save failed: {e}");
            }
        }
    }

    /// 学習履歴から指定の `(reading, surface)` を明示的に削除する。
    ///
    /// 戻り値: 削除されたエントリがあれば `true`、無ければ `false`。
    /// `reading` キーの最後のエントリだった場合はキー自体も削除する。
    /// 永続化 (`learn_history.bin` への即時書き込み) を伴う。失敗は警告ログのみ。
    pub fn forget(&self, reading: &str, surface: &str) -> bool {
        let (removed, snapshot) = {
            let Ok(mut hist) = self.inner.learn_history.write() else {
                warn!("learn_history write lock failed in forget");
                return false;
            };
            let Some(entries) = hist.get_mut(reading) else {
                return false;
            };
            let before = entries.len();
            entries.retain(|e| e.surface != surface);
            let removed = before != entries.len();
            if entries.is_empty() {
                hist.remove(reading);
            }
            (removed, if removed { Some(hist.clone()) } else { None })
        };

        if removed {
            info!(
                "dict::store: forget reading={:?} surface={:?}",
                reading, surface
            );
            if let (Some(snapshot), Some(path)) = (snapshot, &self.inner.learn_history_path) {
                if let Err(e) = save_learn_history_file(path, &snapshot) {
                    warn!("learn_history save failed after forget: {e}");
                }
            }
        }
        removed
    }

    /// 学習履歴から `reading` のエントリを score 降順で並べ、surface のリストを返す。
    ///
    /// `merge_candidates` で「mozc/user 候補のうち最近選ばれたものを先に出す」ために使う。
    /// 返り値の surface は必ずしも mozc/user に存在するとは限らないので、呼び出し側で
    /// 重複除去 + 在籍チェックを行うこと。
    pub fn lookup_learn(&self, reading: &str) -> Vec<String> {
        let Ok(hist) = self.inner.learn_history.read() else {
            return vec![];
        };
        let Some(entries) = hist.get(reading) else {
            return vec![];
        };
        let now = now_unix_secs();
        let mut scored: Vec<(f64, String)> = entries
            .iter()
            .map(|e| (e.score(now), e.surface.clone()))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        scored.into_iter().map(|(_, s)| s).collect()
    }

    /// `(reading, surface)` が MOZC 辞書またはユーザー辞書に存在するかを判定する。
    ///
    /// 学習対象を「辞書由来の候補のみ」に絞るためのガード。
    /// LLM 生成の surface は辞書外なのでここで false になり、確定時の学習がスキップされる。
    ///
    /// ただし、括弧ペア（`『』` `《》` `«»` など）のような**記号のみからなる surface** は
    /// LLM 由来であっても誤学習リスクが低いため、辞書外でも学習を許可する。
    fn is_dict_surface(&self, reading: &str, surface: &str) -> bool {
        self.reload_user_if_changed();
        if let Ok(user) = self.inner.user.read() {
            if user
                .get(reading)
                .is_some_and(|list| list.iter().any(|s| s == surface))
            {
                return true;
            }
        }
        if let Some(mozc) = &self.inner.mozc {
            // 1 読みあたり 1024 候補を超えることはまず無いので、この範囲を走査すれば十分。
            if mozc.lookup(reading, 1024).iter().any(|(s, _)| s == surface) {
                return true;
            }
        }
        // ひらがな・CJK漢字を含まない surface（カタカナ・英数・記号等）は辞書外でも学習を許可する。
        // F7カタカナ変換・F9全角英数変換・括弧ペア等が対象。漢字誤変換の学習リスクは生じない。
        !surface.is_empty() && surface.chars().all(is_learnable_without_dict)
    }

    /// ひらがな読みからユーザー辞書候補のみを返す（merge_candidates 用）
    pub fn lookup_user(&self, reading: &str) -> Vec<String> {
        self.reload_user_if_changed();
        let Ok(user) = self.inner.user.read() else {
            return vec![];
        };
        user.get(reading).cloned().unwrap_or_default()
    }

    /// ひらがな読みから mozc 候補を返す（ユーザー辞書を除く）
    pub fn lookup_dict(&self, reading: &str, limit: usize) -> Vec<String> {
        let mozc_loaded = self.inner.mozc.is_some();
        let result: Vec<String> = self
            .inner
            .mozc
            .as_ref()
            .map(|d| {
                d.lookup(reading, limit)
                    .into_iter()
                    .map(|(s, _)| s)
                    .collect()
            })
            .unwrap_or_default();
        debug!(
            "dict::store: lookup reading={:?} mozc={} n={}",
            reading,
            mozc_loaded,
            result.len()
        );
        result
    }

    /// ひらがな読みから候補リストを引く（優先順位: user > mozc）
    /// 後方互換のために残す。merge_candidates では lookup_user/lookup_dict を使う。
    pub fn lookup(&self, reading: &str, limit: usize) -> DictResult {
        let user_cands = {
            let Ok(user) = self.inner.user.read() else {
                return DictResult {
                    candidates: vec![],
                    source: DictSource::None,
                };
            };
            user.get(reading).cloned()
        };

        let mozc_cands: Vec<String> = self
            .inner
            .mozc
            .as_ref()
            .map(|d| {
                d.lookup(reading, limit)
                    .into_iter()
                    .map(|(s, _)| s)
                    .collect()
            })
            .unwrap_or_default();

        let has_user = user_cands.is_some();
        let has_mozc = !mozc_cands.is_empty();

        if !has_user && !has_mozc {
            return DictResult {
                candidates: vec![],
                source: DictSource::None,
            };
        }

        let mut merged: Vec<String> = Vec::new();

        if let Some(u) = user_cands {
            for s in u {
                if !merged.contains(&s) {
                    merged.push(s);
                }
            }
        }

        for s in &mozc_cands {
            if merged.len() >= limit {
                break;
            }
            if !merged.contains(s) {
                merged.push(s.clone());
            }
        }

        merged.truncate(limit);

        let source = match (has_user, has_mozc) {
            (true, false) => DictSource::User,
            (false, true) => DictSource::Mozc,
            _ => DictSource::Merged,
        };

        DictResult {
            candidates: merged,
            source,
        }
    }

    pub fn is_mozc_loaded(&self) -> bool {
        self.inner.mozc.is_some()
    }
    pub fn user_entry_count(&self) -> usize {
        self.inner.user.read().map(|u| u.len()).unwrap_or(0)
    }

    /// 学習履歴の合計エントリ数を返す（テスト/診断用）
    pub fn learn_entry_count(&self) -> usize {
        self.inner
            .learn_history
            .read()
            .map(|h| h.values().map(|v| v.len()).sum())
            .unwrap_or(0)
    }
}

// ─── 永続化ヘルパ ─────────────────────────────────────────────────────────────

fn load_learn_history_file(path: &Path) -> Result<HashMap<String, Vec<LearnEntry>>> {
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let bytes = std::fs::read(path)?;
    let file: LearnHistoryFile =
        bincode::deserialize(&bytes).map_err(|e| anyhow::anyhow!("bincode decode: {e}"))?;
    if file.version != LEARN_HISTORY_FORMAT_VERSION {
        warn!(
            "learn_history: version mismatch (file={}, expected={}); ignoring file",
            file.version, LEARN_HISTORY_FORMAT_VERSION
        );
        return Ok(HashMap::new());
    }
    let mut entries = file.entries;
    let pruned = prune_stale_entries(&mut entries, STALE_ENTRY_MAX_AGE_DAYS, now_unix_secs());
    if pruned > 0 {
        info!(
            "learn_history: pruned {} stale entries (max_age_days={}) on load",
            pruned, STALE_ENTRY_MAX_AGE_DAYS
        );
    }
    Ok(entries)
}

/// `last_access_time` が `now - max_age_days * 86400` より古いエントリを削除する。
/// 削除後に空になった reading キーも除去する。
/// 戻り値: 削除されたエントリ数。
fn prune_stale_entries(
    hist: &mut HashMap<String, Vec<LearnEntry>>,
    max_age_days: u64,
    now: u64,
) -> usize {
    let cutoff = now.saturating_sub(max_age_days.saturating_mul(86_400));
    let mut pruned: usize = 0;
    hist.retain(|_reading, entries| {
        let before = entries.len();
        entries.retain(|e| e.last_access_time >= cutoff);
        pruned += before - entries.len();
        !entries.is_empty()
    });
    pruned
}

fn save_learn_history_file(path: &Path, entries: &HashMap<String, Vec<LearnEntry>>) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = LearnHistoryFileRef {
        version: LEARN_HISTORY_FORMAT_VERSION,
        entries,
    };
    let bytes = bincode::serialize(&file).map_err(|e| anyhow::anyhow!("bincode encode: {e}"))?;
    // アトミック書き込み: .tmp に書いてからリネーム。crash で破損ファイルを残さない。
    let tmp = path.with_extension("bin.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 学習履歴の LRU 圧縮。合計エントリ数が `cap` を超えている場合、
/// `last_access_time` が最古のエントリから `excess` 件を削除する。
fn trim_learn_history_to_capacity(hist: &mut HashMap<String, Vec<LearnEntry>>, cap: usize) {
    let total: usize = hist.values().map(|v| v.len()).sum();
    if total <= cap {
        return;
    }
    // (reading, surface, last_access_time) のタプルに展開してソート
    let mut all: Vec<(String, String, u64)> = hist
        .iter()
        .flat_map(|(r, entries)| {
            entries
                .iter()
                .map(|e| (r.clone(), e.surface.clone(), e.last_access_time))
        })
        .collect();
    all.sort_by_key(|(_, _, t)| *t); // 昇順 (oldest first)
    let excess = total - cap;
    for (r, s, _) in all.into_iter().take(excess) {
        if let Some(entries) = hist.get_mut(&r) {
            entries.retain(|e| e.surface != s);
            if entries.is_empty() {
                hist.remove(&r);
            }
        }
    }
    debug!(
        "learn_history: trimmed {} old entries (cap={})",
        excess, cap
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_store(user: &[(&str, Vec<&str>)]) -> DictStore {
        let user_map: HashMap<String, Vec<String>> = user
            .iter()
            .map(|(r, surfaces)| {
                (
                    r.to_string(),
                    surfaces.iter().map(|s| s.to_string()).collect(),
                )
            })
            .collect();
        DictStore {
            inner: Arc::new(DictStoreInner {
                user: RwLock::new(user_map),
                user_path: None,
                user_file_state: RwLock::new(UserDictFileState::default()),
                mozc: None,
                learn_history: RwLock::new(HashMap::new()),
                learn_history_path: None,
            }),
        }
    }

    #[test]
    fn test_no_hit() {
        let store = make_store(&[]);
        let r = store.lookup("zzz", 10);
        assert!(r.candidates.is_empty());
        assert_eq!(r.source, DictSource::None);
    }

    #[test]
    fn test_user_dict_hot_reload_on_lookup() {
        let dir = tempfile::tempdir().unwrap();
        let user_path = dir.path().join("user_dict.toml");

        let mut ud = UserDict::default();
        ud.add("かっこ", "『");
        ud.save(&user_path).unwrap();

        let store = DictStore::load(Some(&user_path), None, None).unwrap();
        assert_eq!(store.lookup_user("かっこ"), vec!["『"]);

        let mut updated = UserDict::default();
        updated.add("かっこ", "《");
        updated.add("かっこ", "『");
        updated.add("かっことじ", "』");
        updated.save(&user_path).unwrap();

        assert_eq!(store.lookup_user("かっこ"), vec!["『", "《"]);
        assert_eq!(store.lookup_user("かっことじ"), vec!["』"]);
    }

    #[test]
    fn test_learn_records_in_history() {
        // user_dict に居る surface を learn すると learn_history に記録される
        // (user_dict.toml は Phase 2b 以降変更されない)。
        let store = make_store(&[("にほんご", vec!["日本語"])]);
        store.learn("にほんご", "日本語");
        let learned = store.lookup_learn("にほんご");
        assert_eq!(learned, vec!["日本語"]);
        // user_dict は変化しない
        let user = store.lookup_user("にほんご");
        assert_eq!(user, vec!["日本語"]);
    }

    #[test]
    fn test_learn_history_mru_ordering() {
        // 複数 surface を順次 learn すると、last_access_time 昇順 ≒ 後で確定したほうが上位。
        let store = make_store(&[("よみ", vec!["表記A", "表記B"])]);
        store.learn("よみ", "表記A");
        // 1 秒ずらして B を確定（テスト上は同一秒だが freq の差で順序がつく）
        store.learn("よみ", "表記B");
        let learned = store.lookup_learn("よみ");
        // B が最後に確定されたので score は A と同じ last_access_time + freq だが、
        // freq は両方 1 なので score はほぼ同じ → 順序は不定。
        // 少なくとも 2 要素が返ることを確認。
        assert_eq!(learned.len(), 2);
        assert!(learned.contains(&"表記A".to_string()));
        assert!(learned.contains(&"表記B".to_string()));
    }

    #[test]
    fn test_learn_history_freq_boost() {
        // 同じ surface を複数回 learn すると suggestion_freq が増え、score が上がる。
        let store = make_store(&[("よみ", vec!["表記A", "表記B"])]);
        store.learn("よみ", "表記A");
        store.learn("よみ", "表記A"); // freq = 2
        store.learn("よみ", "表記B"); // freq = 1
        let learned = store.lookup_learn("よみ");
        assert_eq!(learned[0], "表記A", "freq が高いほうが先頭");
    }

    #[test]
    fn test_learn_skips_non_dict_surface() {
        // MOZC 辞書にも user 辞書にも無い surface は学習されない（LLM 生成などを想定）。
        let store = make_store(&[("あいうえお", vec!["登録済み"])]);
        store.learn("あいうえお", "未登録");
        let learned = store.lookup_learn("あいうえお");
        assert!(
            learned.is_empty(),
            "未登録 surface は learn_history に入らない"
        );
        // user_dict も変化しない
        let user = store.lookup_user("あいうえお");
        assert_eq!(user, vec!["登録済み"]);
    }

    #[test]
    fn test_learn_skips_unknown_reading() {
        // reading 自体が辞書に無い場合も学習しない。
        let store = make_store(&[]);
        store.learn("にほんご", "日本語");
        assert!(store.lookup_learn("にほんご").is_empty());
        assert!(store.lookup_user("にほんご").is_empty());
    }

    #[test]
    fn test_learn_skips_kanji_digit_literal_candidates() {
        // 数字 reading (`200` 等) から生成される漢数字 surface は CJK漢字を含むため
        // 辞書ガードが効き、学習されない（LLM誤変換防止）。
        let store = make_store(&[]);
        for (reading, surface) in [
            ("200", "二百"),
            ("200", "二〇〇"),
            ("1234", "千二百三十四"),
            ("1234", "壱千弐百参拾四"),
        ] {
            store.learn(reading, surface);
            assert!(
                store.lookup_learn(reading).is_empty(),
                "漢数字 surface {:?} は辞書がなければ学習されない",
                reading,
            );
        }
    }

    #[test]
    fn test_learn_allows_fullwidth_digit_literal() {
        // 全角数字 (`２００`) はひらがな・漢字を含まないため学習対象。
        // F9 変換由来の数字表記が学習されることを確認。
        let store = make_store(&[]);
        store.learn("200", "２００");
        assert!(
            store.lookup_learn("200").contains(&"２００".to_string()),
            "全角数字 surface は学習される"
        );
    }

    #[test]
    fn test_learn_allows_alpha_symbol_via_fkey_transform() {
        // F9全角英数変換・F10半角英数変換など「変化で表示させた記号」は
        // 辞書外でも学習対象。ひらがな・CJK漢字を含まなければ許可。
        let store = make_store(&[]);
        for (reading, surface) in [
            ("USB-C", "ＵＳＢ-Ｃ"),
            ("USB-C", "USB-C"),
            ("(test)", "（test）"),
            ("A+B", "Ａ＋Ｂ"),
        ] {
            store.learn(reading, surface);
            assert!(
                store.lookup_learn(reading).contains(&surface.to_string()),
                "変換由来の英数 surface {:?} は学習される",
                surface
            );
        }
    }

    #[test]
    fn test_learn_allows_pure_symbol_surface() {
        // 記号のみからなる surface（括弧ペア等）は辞書外でも学習できる。
        // 何度も選択した『』が昇格しない問題への対処。
        let store = make_store(&[]);
        for surface in [
            "『』",
            "《》",
            "«»",
            "\u{201c}\u{201d}",
            "<>",
            "【】",
            "（）",
        ] {
            store.learn("かっこ", surface);
            let learned = store.lookup_learn("かっこ");
            assert!(
                learned.contains(&surface.to_string()),
                "記号 surface {:?} は辞書外でも学習される",
                surface
            );
        }
    }

    #[test]
    fn test_learn_allows_katakana_surface() {
        // F7カタカナ変換・F8半角カタカナ変換の結果は辞書外でも学習対象。
        let store = make_store(&[]);
        for (reading, surface) in [
            ("かっこ", "カッコ"),
            ("てすと", "テスト"),
            ("はろー", "ﾊﾛｰ"),
        ] {
            store.learn(reading, surface);
            assert!(
                store.lookup_learn(reading).contains(&surface.to_string()),
                "カタカナ surface {:?} は辞書外でも学習される",
                surface
            );
        }
    }

    #[test]
    fn test_learn_still_skips_kanji_not_in_dict() {
        // CJK漢字を含む surface は辞書ガードが必要（LLM誤変換防止）。
        // 辞書に存在しない漢字 surface は学習されない。
        let store = make_store(&[]);
        for (reading, surface) in [("かっこ", "括弧"), ("あいうえお", "未登録")] {
            store.learn(reading, surface);
            assert!(
                store.lookup_learn(reading).is_empty(),
                "漢字含み surface {:?} は辞書がなければ学習されない",
                surface
            );
        }
    }

    #[test]
    fn test_learn_allows_user_dict_override_of_literal_reading() {
        // ユーザーが意図的に user_dict に登録した場合だけ学習を許す。
        // 例: 「200」→「200円」を user_dict に登録 → 学習可能。
        let store = make_store(&[("200", vec!["200円"])]);
        store.learn("200", "200円");
        assert_eq!(
            store.lookup_learn("200"),
            vec!["200円"],
            "user_dict 登録ありなら学習される"
        );
        // 登録されていない literal surface は依然として学習されない
        store.learn("200", "二百");
        let learned = store.lookup_learn("200");
        assert!(
            !learned.contains(&"二百".to_string()),
            "user_dict に無い literal surface は学習されない"
        );
    }

    #[test]
    fn test_learn_entry_score_recency() {
        // 同じ freq でも last_access_time が新しいほうが score 高い。
        let old = LearnEntry {
            surface: "A".into(),
            last_access_time: 1_000,
            suggestion_freq: 1,
            shown_freq: 0,
        };
        let new = LearnEntry {
            surface: "B".into(),
            last_access_time: 2_000,
            suggestion_freq: 1,
            shown_freq: 0,
        };
        let now = 2_000;
        assert!(new.score(now) > old.score(now));
    }

    #[test]
    fn test_learn_entry_score_decay() {
        // 半減期 30 日で freq 項が半分になる。
        let fresh = LearnEntry {
            surface: "A".into(),
            last_access_time: 0,
            suggestion_freq: 10,
            shown_freq: 0,
        };
        let score_now = fresh.score(0);
        let score_30d = fresh.score(30 * 86_400);
        let score_60d = fresh.score(60 * 86_400);
        // freq 項: 1 freq = 86400 なので 10 freq = 864000
        // 30日経過: 半分 = 432000、60日経過: 1/4 = 216000
        // last_access_time 項: 0 (両方同じ)、文字数ペナルティ: -1 (両方同じ)
        let freq_contribution_now = score_now - (-1.0); // -(chars=1)
        let freq_contribution_30d = score_30d - (-1.0);
        let freq_contribution_60d = score_60d - (-1.0);
        assert!(
            (freq_contribution_30d / freq_contribution_now - 0.5).abs() < 0.01,
            "30 日で半分になること: {} / {} = {}",
            freq_contribution_30d,
            freq_contribution_now,
            freq_contribution_30d / freq_contribution_now
        );
        assert!(
            (freq_contribution_60d / freq_contribution_now - 0.25).abs() < 0.01,
            "60 日で 1/4 になること"
        );
    }

    #[test]
    fn test_trim_learn_history_to_capacity() {
        // cap=2 に対して 3 エントリ → 最古 1 件が削除される
        let mut hist: HashMap<String, Vec<LearnEntry>> = HashMap::new();
        hist.insert(
            "a".into(),
            vec![
                LearnEntry {
                    surface: "A".into(),
                    last_access_time: 100,
                    suggestion_freq: 1,
                    shown_freq: 0,
                },
                LearnEntry {
                    surface: "B".into(),
                    last_access_time: 300,
                    suggestion_freq: 1,
                    shown_freq: 0,
                },
            ],
        );
        hist.insert(
            "b".into(),
            vec![LearnEntry {
                surface: "C".into(),
                last_access_time: 200,
                suggestion_freq: 1,
                shown_freq: 0,
            }],
        );

        trim_learn_history_to_capacity(&mut hist, 2);

        let total: usize = hist.values().map(|v| v.len()).sum();
        assert_eq!(total, 2, "cap=2 まで削減される");
        // A (100) が最古なので削除されているはず
        assert!(
            !hist
                .get("a")
                .is_some_and(|v| v.iter().any(|e| e.surface == "A")),
            "最古 (A, 100) が削除されている"
        );
    }

    #[test]
    fn test_trim_removes_empty_reading() {
        // reading 配下が全て削除されたら reading 自体も HashMap から消す
        let mut hist: HashMap<String, Vec<LearnEntry>> = HashMap::new();
        hist.insert(
            "old".into(),
            vec![LearnEntry {
                surface: "X".into(),
                last_access_time: 100,
                suggestion_freq: 1,
                shown_freq: 0,
            }],
        );
        hist.insert(
            "new".into(),
            vec![LearnEntry {
                surface: "Y".into(),
                last_access_time: 500,
                suggestion_freq: 1,
                shown_freq: 0,
            }],
        );

        trim_learn_history_to_capacity(&mut hist, 1);

        assert!(!hist.contains_key("old"), "空になった reading は削除");
        assert!(hist.contains_key("new"));
    }

    #[test]
    fn test_learn_history_roundtrip_bincode() {
        // bincode で書き出した履歴を読み戻せることを確認。
        // load 時の prune (STALE_ENTRY_MAX_AGE_DAYS) で消されないよう、
        // last_access_time は now を使う。
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("learn_history.bin");

        let now = now_unix_secs();
        let mut entries: HashMap<String, Vec<LearnEntry>> = HashMap::new();
        entries.insert(
            "にほんご".into(),
            vec![LearnEntry {
                surface: "日本語".into(),
                last_access_time: now,
                suggestion_freq: 3,
                shown_freq: 5,
            }],
        );

        save_learn_history_file(&path, &entries).unwrap();
        let loaded = load_learn_history_file(&path).unwrap();

        assert_eq!(loaded.len(), 1);
        let e = &loaded["にほんご"][0];
        assert_eq!(e.surface, "日本語");
        assert_eq!(e.last_access_time, now);
        assert_eq!(e.suggestion_freq, 3);
        assert_eq!(e.shown_freq, 5);
    }

    #[test]
    fn test_load_prunes_stale_entries() {
        // load_learn_history_file は STALE_ENTRY_MAX_AGE_DAYS より古いエントリを除く
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("learn_history.bin");

        let now = now_unix_secs();
        let day = 86_400u64;
        let mut entries: HashMap<String, Vec<LearnEntry>> = HashMap::new();
        entries.insert(
            "fresh".into(),
            vec![LearnEntry {
                surface: "A".into(),
                last_access_time: now - 10 * day, // 残る
                suggestion_freq: 1,
                shown_freq: 0,
            }],
        );
        entries.insert(
            "stale".into(),
            vec![LearnEntry {
                surface: "B".into(),
                last_access_time: now.saturating_sub(365 * day), // 1 年前: 消える
                suggestion_freq: 1,
                shown_freq: 0,
            }],
        );

        save_learn_history_file(&path, &entries).unwrap();
        let loaded = load_learn_history_file(&path).unwrap();
        assert!(loaded.contains_key("fresh"));
        assert!(!loaded.contains_key("stale"));
    }

    #[test]
    fn test_learn_history_load_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.bin");
        let loaded = load_learn_history_file(&path).unwrap();
        assert!(loaded.is_empty(), "ファイルが無ければ空 HashMap");
    }

    #[test]
    fn test_learn_history_load_corrupted_file() {
        // 壊れたファイルは bincode エラーになる（呼び出し側で catch される前提）
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.bin");
        std::fs::write(&path, b"not a valid bincode").unwrap();
        assert!(load_learn_history_file(&path).is_err());
    }

    #[test]
    fn test_prune_stale_entries_removes_old() {
        // 180 日より古い last_access_time を持つエントリは削除される
        let mut hist: HashMap<String, Vec<LearnEntry>> = HashMap::new();
        let now: u64 = 1_700_000_000;
        let day = 86_400u64;
        hist.insert(
            "fresh".into(),
            vec![LearnEntry {
                surface: "A".into(),
                last_access_time: now - 30 * day, // 30日前: 残す
                suggestion_freq: 1,
                shown_freq: 0,
            }],
        );
        hist.insert(
            "stale".into(),
            vec![LearnEntry {
                surface: "B".into(),
                last_access_time: now - 200 * day, // 200日前: 消す
                suggestion_freq: 5,
                shown_freq: 0,
            }],
        );
        hist.insert(
            "mixed".into(),
            vec![
                LearnEntry {
                    surface: "C-keep".into(),
                    last_access_time: now - 10 * day, // 10日前: 残す
                    suggestion_freq: 2,
                    shown_freq: 0,
                },
                LearnEntry {
                    surface: "C-drop".into(),
                    last_access_time: now - 365 * day, // 1年前: 消す
                    suggestion_freq: 1,
                    shown_freq: 0,
                },
            ],
        );

        let pruned = prune_stale_entries(&mut hist, 180, now);
        assert_eq!(
            pruned, 2,
            "2 エントリが削除される (stale/B と mixed/C-drop)"
        );
        assert!(hist.contains_key("fresh"));
        assert!(
            !hist.contains_key("stale"),
            "全エントリが古い reading は削除"
        );
        assert!(hist.contains_key("mixed"));
        assert_eq!(hist["mixed"].len(), 1);
        assert_eq!(hist["mixed"][0].surface, "C-keep");
    }

    #[test]
    fn test_prune_stale_entries_no_op_when_all_fresh() {
        let mut hist: HashMap<String, Vec<LearnEntry>> = HashMap::new();
        let now: u64 = 1_700_000_000;
        hist.insert(
            "fresh".into(),
            vec![LearnEntry {
                surface: "X".into(),
                last_access_time: now,
                suggestion_freq: 1,
                shown_freq: 0,
            }],
        );
        let pruned = prune_stale_entries(&mut hist, 180, now);
        assert_eq!(pruned, 0);
        assert_eq!(hist.len(), 1);
    }

    #[test]
    fn test_forget_removes_specific_surface() {
        let store = make_store(&[("にほんご", vec!["日本語", "二本語"])]);
        store.learn("にほんご", "日本語");
        store.learn("にほんご", "二本語");
        assert_eq!(store.lookup_learn("にほんご").len(), 2);

        let removed = store.forget("にほんご", "二本語");
        assert!(removed, "対象 surface を削除した場合 true");
        let learned = store.lookup_learn("にほんご");
        assert_eq!(learned, vec!["日本語"]);

        // 全削除でキー自体が消える
        let removed = store.forget("にほんご", "日本語");
        assert!(removed);
        assert!(store.lookup_learn("にほんご").is_empty());
    }

    #[test]
    fn test_forget_returns_false_when_missing() {
        let store = make_store(&[("にほんご", vec!["日本語"])]);
        let removed = store.forget("にほんご", "見つからない");
        assert!(!removed);
        let removed = store.forget("そんざいしない", "X");
        assert!(!removed);
    }
}
