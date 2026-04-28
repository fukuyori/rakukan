//! グローバル IME 状態
//!
//! # ロック戦略
//! TSF OnKeyDown はホットパス（UIスレッド）のため、**絶対にブロックしない**。
//! - ホットパス: `try_lock()` のみ使用。取れなければ即リターン。
//! - 非ホットパス（Activate, BG スレッド): `lock()` を使用可。

use super::input_mode::InputMode;
// RpcEngine は DynEngine と同じメソッドシグネチャを露出するので、
// 既存コードの大部分が触らないよう `DynEngine` の名前で re-export する。
// 実体は `rakukan-engine-rpc` を通じて `rakukan-engine-host.exe` へ Named Pipe で
// 通信するクライアント。TSF プロセス内に `rakukan_engine_*.dll` はロードされない。
pub use rakukan_engine_rpc::InputCharKind;
pub use rakukan_engine_rpc::RpcEngine as DynEngine;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering as AO};
use std::sync::{LazyLock, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::core::{Interface, GUID};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIAdapter3, IDXGIFactory1,
    DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
};

// ─── INPUT_MODE_ATOMIC ────────────────────────────────────────────────────────
// IMEState.input_mode の鏡。ロックなしでホットパス（OnTestKeyDown / OnKeyDown）
// から安全に読み取れるよう AtomicU8 で持つ。
// 値: 0=Hiragana, 1=Katakana, 2=Alphanumeric
// IMEState::set_mode が呼ばれるたびに同期更新される。

static INPUT_MODE_ATOMIC: AtomicU8 = AtomicU8::new(0);

pub fn input_mode_set_atomic(mode: InputMode) {
    let v = match mode {
        InputMode::Hiragana => 0u8,
        InputMode::Katakana => 1u8,
        InputMode::Alphanumeric => 2u8,
    };
    INPUT_MODE_ATOMIC.store(v, AO::Release);
}

/// ロックなし高速読み取り（ホットパス用）
#[inline]
pub fn input_mode_get_atomic() -> InputMode {
    match INPUT_MODE_ATOMIC.load(AO::Acquire) {
        1 => InputMode::Katakana,
        2 => InputMode::Alphanumeric,
        _ => InputMode::Hiragana,
    }
}

// ─── EngineWrapper ────────────────────────────────────────────────────────────
// Safety: TSF は STA で動作し、Mutex で保護するため Send/Sync を許容する。
// ただし ホットパスでは try_lock() しか使わないことを必ず守ること。

pub struct EngineWrapper(pub Option<DynEngine>);
unsafe impl Send for EngineWrapper {}
unsafe impl Sync for EngineWrapper {}

pub static RAKUKAN_ENGINE: LazyLock<Mutex<EngineWrapper>> =
    LazyLock::new(|| Mutex::new(EngineWrapper(None)));

/// バックグラウンドエンジン初期化が既に起動済みかどうかのフラグ。
/// Activate ごとに重複スポーンしないために使う。
static ENGINE_INIT_STARTED: AtomicBool = AtomicBool::new(false);

/// 辞書ロード完了のラッチ。
///
/// `poll_dict_ready()` は一度 true を返したら以降ずっと true のため、
/// 毎キーストロークごとに RPC を往復させる必要はない。
/// ラッチが立っている間は RPC をスキップする。
/// `engine_reload()` でリセットされる。
static DICT_READY_LATCH: AtomicBool = AtomicBool::new(false);

/// モデルロード完了のラッチ（`DICT_READY_LATCH` と同じ方針）。
static MODEL_READY_LATCH: AtomicBool = AtomicBool::new(false);

/// M1.6 T-HOST2: ラッチリセット時刻（UNIX epoch ms）。false → true 遷移で
/// 経過時間をログする。0 の間は計測無効（起動直後など）。
static READY_RESET_AT_MS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 辞書 ready 状態を取得。未 ready のうちだけ RPC で poll する。
///
/// ホットパス（`on_input` 等）から 1 キーストロークごとに呼ばれる前提。
#[inline]
pub fn poll_dict_ready_cached(eng: &DynEngine) -> bool {
    if DICT_READY_LATCH.load(AO::Acquire) {
        return true;
    }
    let r = eng.poll_dict_ready();
    if r {
        // M1.6 T-HOST2: false → true の遷移で計測
        if !DICT_READY_LATCH.swap(true, AO::AcqRel) {
            let reset_at = READY_RESET_AT_MS.load(AO::Acquire);
            if reset_at != 0 {
                let elapsed = now_ms().saturating_sub(reset_at);
                tracing::info!("dict ready: {} ms since reload reset", elapsed);
            }
        }
    }
    r
}

/// モデル ready 状態を取得（`poll_dict_ready_cached` と同じ方針）。
#[inline]
pub fn poll_model_ready_cached(eng: &DynEngine) -> bool {
    if MODEL_READY_LATCH.load(AO::Acquire) {
        return true;
    }
    let r = eng.poll_model_ready();
    if r {
        if !MODEL_READY_LATCH.swap(true, AO::AcqRel) {
            let reset_at = READY_RESET_AT_MS.load(AO::Acquire);
            if reset_at != 0 {
                let elapsed = now_ms().saturating_sub(reset_at);
                tracing::info!("model ready: {} ms since reload reset", elapsed);
            }
        }
    }
    r
}

/// reload 時など、ready 状態を強制的に再評価させたいときに呼ぶ。
/// M1.6 T-HOST2: リセット時刻を記録し、以降の ready 遷移で経過時間をログする。
#[inline]
pub fn reset_ready_latches() {
    DICT_READY_LATCH.store(false, AO::Release);
    MODEL_READY_LATCH.store(false, AO::Release);
    READY_RESET_AT_MS.store(now_ms(), AO::Release);
}

/// M1.6 T-HOST3: 読込中 UI で「経過時間」を表示するため、直近の reset からの
/// 経過 ms を返す。reset されていない（= 初期状態 or 既に ready）なら `None`。
pub fn ready_reset_elapsed_ms() -> Option<u64> {
    let reset_at = READY_RESET_AT_MS.load(AO::Acquire);
    if reset_at == 0 {
        return None;
    }
    Some(now_ms().saturating_sub(reset_at))
}

/// M1.6 T-HOST4: engine が未 ready（None）の期間に打鍵されたキーを積むバッファ。
///
/// 旧実装では [factory::on_input] / [factory::on_input_raw] が `guard.as_mut()`
/// で `None` を受けると `return Ok(true)` で入力を握り潰していた。reload 中や
/// 初回起動中に打鍵した文字が全部消える原因。
///
/// T-HOST4 ではここに積んでおき、engine 復帰後の最初の on_input で先に drain
/// してから現在のキーを処理する。これによりユーザ体感は「入力が一瞬遅れるが
/// 消えない」状態になる。
static PENDING_KEYS: LazyLock<Mutex<Vec<(char, InputCharKind, bool)>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// engine 未 ready 時の「握り潰さない」代替: 後で engine に流し込むため積む。
/// `raw` = true なら `input_char` ではなく `push_raw` 経路を使いたい旨のフラグ。
pub fn push_pending_key(c: char, kind: InputCharKind, raw: bool) {
    if let Ok(mut v) = PENDING_KEYS.lock() {
        v.push((c, kind, raw));
        tracing::debug!("pending_keys: buffered c={:?} kind={:?} raw={} (total={})", c, kind, raw, v.len());
    }
}

/// engine が復帰した時点で呼び、積んだキーを返す（所有権を奪う）。
/// 呼び出し元は返却された各要素に対して engine メソッドを呼んで replay する。
pub fn drain_pending_keys() -> Vec<(char, InputCharKind, bool)> {
    if let Ok(mut v) = PENDING_KEYS.lock() {
        if v.is_empty() {
            return Vec::new();
        }
        let out = std::mem::take(&mut *v);
        tracing::info!("pending_keys: draining {} buffered keys for replay", out.len());
        out
    } else {
        Vec::new()
    }
}

static LAST_GPU_MEMORY_LOG_MS: AtomicU64 = AtomicU64::new(0);
const GPU_MEMORY_LOG_INTERVAL_MS: u64 = 30_000;

/// エンジン DLL のロードをバックグラウンドスレッドで開始する。
///
/// Activate（UIスレッド）からの呼び出し専用。
/// DLL ロードは重いため（CUDA 初期化で数秒かかることがある）UIスレッドをブロックしない。
/// 既に起動済みの場合は何もしない（二重スポーン防止）。
/// ロード完了後、辞書・モデルのバックグラウンドロードを開始し、
/// `LANGBAR_UPDATE_PENDING` をセットして言語バー表示を更新する。
pub fn engine_start_bg_init() {
    // すでに起動済みなら何もしない（エンジンが既に存在する場合も不要）
    if ENGINE_INIT_STARTED.swap(true, AO::AcqRel) {
        // 既存エンジンで辞書・モデルがまだ未ロードなら起動する
        if let Ok(mut g) = RAKUKAN_ENGINE.try_lock() {
            if let Some(eng) = g.0.as_mut() {
                if !eng.is_dict_ready() {
                    eng.start_load_dict();
                }
                if !eng.is_kanji_ready() {
                    eng.start_load_model();
                }
            }
        }
        tracing::debug!("engine_start_bg_init: already started, skipping DLL load");
        return;
    }
    tracing::info!("engine_start_bg_init: spawning background engine init thread");
    std::thread::Builder::new()
        .name("rakukan-engine-init".into())
        .spawn(|| {
            tracing::info!("engine-init: starting DLL load");
            let load_result = {
                match RAKUKAN_ENGINE.lock() {
                    Ok(mut g) => {
                        if g.0.is_some() {
                            tracing::debug!("engine-init: engine already present, skipping");
                            return;
                        }
                        match create_engine() {
                            Ok(e) => {
                                g.0 = Some(e);
                                Ok(())
                            }
                            Err(e) => Err(e),
                        }
                    }
                    Err(_) => Err(anyhow::anyhow!("engine mutex poisoned")),
                }
            };
            match load_result {
                Ok(()) => {
                    // 辞書・モデルのバックグラウンドロードを起動
                    if let Ok(mut g) = RAKUKAN_ENGINE.lock() {
                        if let Some(eng) = g.0.as_mut() {
                            tracing::debug!(
                                "engine-init: is_dict_ready={} is_kanji_ready={}",
                                eng.is_dict_ready(),
                                eng.is_kanji_ready()
                            );
                            if !eng.is_dict_ready() {
                                tracing::info!("engine-init: calling start_load_dict");
                                eng.start_load_dict();
                            }
                            if !eng.is_kanji_ready() {
                                eng.start_load_model();
                            }
                        }
                    }
                    tracing::info!("engine-init: engine created successfully");
                    // 言語バーのアイコン・ツールチップを更新するよう通知
                    langbar_update_set();
                }
                Err(e) => {
                    tracing::error!("engine-init: DLL load failed: {e}");
                    // 次回 Activate で再試行できるようフラグをリセット
                    ENGINE_INIT_STARTED.store(false, AO::Release);
                }
            }
        })
        .ok();
}

/// ホットパス用: ブロックしない。取れなければ Err を返す。
#[inline]
pub fn engine_try_get() -> anyhow::Result<MutexGuard<'static, EngineWrapper>> {
    RAKUKAN_ENGINE
        .try_lock()
        .map_err(|_| anyhow::anyhow!("engine busy"))
}

/// 非ホットパス用（Activate, BG スレッド）: ブロックあり。poison 回復あり。
pub fn engine_get() -> anyhow::Result<MutexGuard<'static, EngineWrapper>> {
    match RAKUKAN_ENGINE.lock() {
        Ok(g) => Ok(g),
        Err(p) => {
            tracing::warn!("engine mutex poisoned, recovering");
            Ok(p.into_inner())
        }
    }
}

/// ホットパス用: エンジンを取得するだけ（DLL ロードしない）。
/// ただしエンジンが未ロードなら、ここで初めて bg init を「1 回だけ」スポーンする。
///
/// これは Zoom / Dropbox / explorer 等のホストプロセスで
/// **実際に入力が行われるまで engine DLL を一切ロードしない** ための仕掛け。
/// Activate の時点ではエンジン DLL に触れないので、IME を使わないアプリで
/// `rakukan_engine_*.dll` と `msvcp140.dll` のクロスロードによる AV を避けられる。
///
/// 初回呼び出し時は bg init がまだ完了していないため Err を返す。
/// ホットパス側は既に Err を握りつぶして動くようになっているので支障はない。
pub fn engine_try_get_or_create() -> anyhow::Result<MutexGuard<'static, EngineWrapper>> {
    // まず普通に try_lock。ここでエンジンが既に存在しロックも取れれば即返せる。
    if let Ok(g) = RAKUKAN_ENGINE.try_lock() {
        if g.0.is_some() {
            return Ok(g);
        }
        // エンジンがまだ無い: ロックを離してから bg init をスポーン
        drop(g);
        engine_start_bg_init();
        return Err(anyhow::anyhow!("engine not ready: bg init triggered"));
    }
    // ロック取れず: 誰かが作業中。busy で返す（ホットパスはこれを無視する）。
    Err(anyhow::anyhow!("engine busy"))
}

/// エンジンを強制的に破棄し、次回アクセス時に再生成されるようにする。
/// 現状は `engine_reload()` が RPC Reload を使うため呼ばれない。
/// 診断 / 緊急回避用に残す。
#[allow(dead_code)]
pub fn engine_force_recreate() {
    match RAKUKAN_ENGINE.lock() {
        Ok(mut g) => {
            tracing::debug!("engine_force_recreate: dropping engine");
            g.0 = None;
        }
        Err(p) => {
            p.into_inner().0 = None;
            tracing::warn!("engine_force_recreate: mutex was poisoned, cleared anyway");
        }
    }
}

/// トレイから「エンジン再起動」が要求されたとき、または config.toml 変更後の
/// IME モード切替で呼ばれる。
///
/// Phase A（out-of-process 化）以降は、TSF 側のハンドル (RpcEngine) を捨てずに
/// ホストプロセスを終了させ、次回 API 呼び出しで新プロセスを自動 spawn させる
/// （M1.6 T-HOST1: host 再起動化）。
///
/// 旧実装では `Request::Reload` で DLL を drop → 再ロードしていたが、engine DLL
/// 内で bg スレッドが動いている状態で `FreeLibrary` が走ると unmapped な
/// 命令ポインタで AV を誘発していた（0.6.5 以降も learn_history 以外の経路で
/// 残存）。ここでは `Request::Shutdown` を送ってホストに自死させ、次回
/// `connect_or_spawn` が新 PID を立ち上げる。OS がプロセス終了時に全スレッドと
/// DLL マッピングをまとめて回収するため、unmap race が原理的に起きない。
///
/// `n_gpu_layers` や `model_variant` のような **エンジン生成時決定パラメータ** は
/// 新 PID の `Create { config_json }` で反映される。client 側は `shutdown()` に
/// 渡した `config_json` を保持し、再接続時に Create で再送する。
#[track_caller]
pub fn engine_reload() {
    // 診断: 誰がこの関数を呼んだかをログに残す。0.7.x で調査中の
    // 「reload event/runtime config 由来でない engine_reload」を切り分けるため。
    let caller = std::panic::Location::caller();
    tracing::info!(
        "engine_reload: invoked from {}:{}:{}",
        caller.file(),
        caller.line(),
        caller.column()
    );
    // reload 後は辞書・モデルが再ロードされるのでラッチもリセットする
    reset_ready_latches();
    // バックグラウンドで shutdown（UI スレッドをブロックしない）
    std::thread::spawn(|| {
        let t_start = std::time::Instant::now();

        // config.toml をディスクから再読み込みしてから EngineConfig JSON を生成する。
        // 設定画面の保存ボタン → SignalReload → reload_watcher 経由で呼ばれた場合、
        // CONFIG_MANAGER のキャッシュは古いままなので、ここで明示的にリロードする。
        // （モード切替経由では `maybe_reload_on_mode_switch` が先に実ファイルを読む）
        super::config::init_config_manager();
        let cfg = build_engine_config_json();

        let mut guard = match RAKUKAN_ENGINE.lock() {
            Ok(g) => g,
            Err(p) => {
                tracing::warn!("engine_reload: mutex poisoned, recovering");
                p.into_inner()
            }
        };
        match guard.0.as_mut() {
            Some(eng) => {
                // ホストに self-exit を依頼。応答は待つが失敗しても前進する
                // （相手が exit 中で応答が返らないのは想定内）。
                let r = eng.shutdown(Some(cfg));
                let elapsed = t_start.elapsed();
                match r {
                    Ok(()) => tracing::info!(
                        "engine_reload: host shutdown requested ({:?})",
                        elapsed
                    ),
                    Err(e) => tracing::warn!(
                        "engine_reload: host shutdown call returned error ({:?}): {e}",
                        elapsed
                    ),
                }
                // ホスト側は応答送信後 50ms sleep してから `process::exit(0)` する
                // (server.rs:73-77)。ここでハンドルを即 drop すると、次の
                // `engine_try_get_or_create()` がその 50ms 内に死にゆくパイプへ
                // connect → Hello で host が exit → "read length" エラーになる
                // race が発生する。RAKUKAN_ENGINE mutex を握ったまま 100ms 待つ
                // ことで、他スレッドの reconnect は古いハンドル経由で短絡され、
                // 本当のホスト exit 完了後に新しい spawn が走る。
                std::thread::sleep(std::time::Duration::from_millis(100));
                // 応答の可否に関わらずハンドルは捨てる。次回
                // `engine_try_get_or_create()` が新 PID へ自動 spawn + 再接続する。
                guard.0 = None;
                ENGINE_INIT_STARTED.store(false, AO::Release);
            }
            None => {
                // ハンドル未作成 = まだ一度も使われていない or 前回落ちた状態。
                // 通常の初回ロードパスに合流させる。
                drop(guard);
                ENGINE_INIT_STARTED.store(false, AO::Release);
                engine_start_bg_init();
            }
        }
    });
}

/// 名前付きイベント `Local\rakukan.engine.reload` を監視するバックグラウンドスレッドを起動する。
/// トレイプロセスがこのイベントを SetEvent したとき engine_reload() を呼ぶ。
pub fn start_reload_watcher() {
    std::thread::Builder::new()
        .name("rakukan-reload-watcher".into())
        .spawn(|| {
            use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};
            let name: Vec<u16> = "Local\\rakukan.engine.reload\0".encode_utf16().collect();
            let evt = unsafe {
                CreateEventW(
                    None,
                    false, // auto-reset
                    false,
                    windows::core::PCWSTR(name.as_ptr()),
                )
            };
            let evt = match evt {
                Ok(h) => h,
                Err(e) => {
                    tracing::error!("reload_watcher: CreateEventW failed: {e}");
                    return;
                }
            };
            tracing::info!("reload_watcher: listening on Local\\rakukan.engine.reload");
            loop {
                let ret = unsafe { WaitForSingleObject(evt, INFINITE) };
                if ret.0 != 0 {
                    // WAIT_ABANDONED or WAIT_FAILED
                    tracing::error!("reload_watcher: WaitForSingleObject failed ({:?})", ret);
                    break;
                }
                tracing::info!("reload_watcher: reload event received");
                engine_reload();
            }
        })
        .ok();
}

/// エンジン（= rakukan-engine-host への RPC クライアント）を生成する。
///
/// TSF プロセス内では engine DLL を一切ロードしない。代わりに
/// `rakukan-engine-host.exe` に Named Pipe で接続する。ホストが動いていなければ
/// `RpcEngine::connect_or_spawn` が `CreateProcessW` で detached 起動する。
fn create_engine() -> anyhow::Result<DynEngine> {
    let cfg = build_engine_config_json();
    let engine = rakukan_engine_rpc::RpcEngine::connect_or_spawn(Some(cfg))
        .map_err(|e| anyhow::anyhow!("engine RPC connect failed: {e}"))?;
    tracing::info!(
        "engine connected via RPC: backend={}",
        engine.backend_label()
    );
    maybe_log_gpu_memory(&engine);
    Ok(engine)
}

/// %APPDATA%\rakukan\config.toml を読んで EngineConfig JSON を生成する。
fn build_engine_config_json() -> String {
    let cfg = super::config::current_config();
    let num_candidates = cfg.effective_num_candidates();
    let main_gpu = cfg.general.main_gpu;
    let n_gpu_layers = cfg.general.n_gpu_layers.unwrap_or(u32::MAX);
    let model_variant = cfg.general.model_variant.clone();
    let digit_width = match cfg.input.digit_width {
        super::config::DigitWidth::Fullwidth => "fullwidth",
        super::config::DigitWidth::Halfwidth => "halfwidth",
    };
    let live_conv_beam_size = cfg.live_conversion.beam_size.clamp(1, 9);
    let convert_beam_size = cfg.conversion.beam_size.clamp(1, 30);

    tracing::info!(
        "engine config: num_candidates={num_candidates} n_gpu_layers={n_gpu_layers} main_gpu={main_gpu} model_variant={model_variant:?} digit_width={digit_width} live_conv_beam_size={live_conv_beam_size} convert_beam_size={convert_beam_size}"
    );
    let mv_json = match &model_variant {
        Some(v) => format!(r#","model_variant":"{}""#, v),
        None => String::new(),
    };
    format!(
        r#"{{"num_candidates":{num_candidates},"n_gpu_layers":{n_gpu_layers},"main_gpu":{main_gpu},"n_threads":0,"digit_width":"{digit_width}","live_conv_beam_size":{live_conv_beam_size},"convert_beam_size":{convert_beam_size}{mv_json}}}"#
    )
}

/// config.toml から num_candidates を読む（ホットパスで使う軽量版）
pub fn get_num_candidates() -> usize {
    super::config::effective_num_candidates()
}

pub fn get_live_conv_beam_size() -> usize {
    super::config::current_config()
        .live_conversion
        .beam_size
        .clamp(1, 9)
}

/// `[input] auto_learn` 設定を返す（デフォルト: false）。
///
/// `false` のとき、確定時の `engine.learn()` 呼び出しを抑止してユーザー辞書への
/// 自動登録を止める。ユーザー辞書は設定画面からの手動登録のみで運用する。
pub fn is_auto_learn_enabled() -> bool {
    super::config::current_config().input.auto_learn
}

pub fn maybe_log_gpu_memory(engine: &DynEngine) {
    if !tracing::enabled!(tracing::Level::DEBUG) {
        return;
    }

    let backend = engine.backend_label();
    if backend.eq_ignore_ascii_case("cpu") || engine.n_gpu_layers() == 0 {
        return;
    }

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let last_ms = LAST_GPU_MEMORY_LOG_MS.load(AO::Acquire);
    if now_ms.saturating_sub(last_ms) < GPU_MEMORY_LOG_INTERVAL_MS {
        return;
    }
    if LAST_GPU_MEMORY_LOG_MS
        .compare_exchange(last_ms, now_ms, AO::AcqRel, AO::Acquire)
        .is_err()
    {
        return;
    }

    let adapter_index = super::config::current_config().general.main_gpu.max(0) as u32;
    match query_local_gpu_memory(adapter_index) {
        Ok((adapter_name, info)) => {
            let used_mb = info.CurrentUsage / (1024 * 1024);
            let budget_mb = info.Budget / (1024 * 1024);
            let available_mb = budget_mb.saturating_sub(used_mb);
            tracing::debug!(
                "gpu memory: backend={} adapter={} used={}MB budget={}MB available={}MB n_gpu_layers={}",
                backend,
                adapter_name,
                used_mb,
                budget_mb,
                available_mb,
                engine.n_gpu_layers()
            );
        }
        Err(err) => {
            tracing::debug!(
                "gpu memory: backend={} adapter_index={} unavailable: {}",
                backend,
                adapter_index,
                err
            );
        }
    }
}

fn query_local_gpu_memory(
    adapter_index: u32,
) -> anyhow::Result<(String, DXGI_QUERY_VIDEO_MEMORY_INFO)> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let adapter1: IDXGIAdapter1 = unsafe { factory.EnumAdapters1(adapter_index)? };
    let adapter_name = unsafe {
        let desc = adapter1.GetDesc1()?;
        wides_to_string(&desc.Description)
    };
    let adapter3: IDXGIAdapter3 = adapter1.cast()?;
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    unsafe { adapter3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)? };
    Ok((adapter_name, info))
}

fn wides_to_string(wides: &[u16]) -> String {
    let len = wides.iter().position(|&c| c == 0).unwrap_or(wides.len());
    String::from_utf16_lossy(&wides[..len])
}

impl std::ops::Deref for EngineWrapper {
    type Target = Option<DynEngine>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
impl std::ops::DerefMut for EngineWrapper {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// ホットパス用: EngineWrapper の MutexGuard 型エイリアス
pub type EngineGuard = std::sync::MutexGuard<'static, EngineWrapper>;

// ─── IMEState ────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct IMEState {
    pub input_mode: InputMode,
    #[allow(dead_code)]
    pub cookies: HashMap<GUID, u32>,
}

pub static IME_STATE: LazyLock<Mutex<IMEState>> = LazyLock::new(|| {
    Mutex::new(IMEState {
        input_mode: InputMode::default(),
        cookies: HashMap::new(),
    })
});

unsafe impl Send for IMEState {}
unsafe impl Sync for IMEState {}

impl IMEState {
    /// ホットパス用: ブロックしない
    #[inline]
    pub fn try_get() -> anyhow::Result<MutexGuard<'static, IMEState>> {
        IME_STATE
            .try_lock()
            .map_err(|_| anyhow::anyhow!("ime_state busy"))
    }

    pub fn set_mode(&mut self, mode: InputMode) {
        tracing::info!("input mode: {:?} → {:?}", self.input_mode, mode);
        self.input_mode = mode;
        // ホットパス用アトミックも同期更新
        input_mode_set_atomic(mode);
        // M1.7 T-MODE2: doc_mode store を即時更新。focus-out を待たずに
        // 現在の DM / HWND に mode を紐付ける。これがないと、同じ DM 内で
        // モードを変えても store 側は「前回 focus-in 時のモード」のままで、
        // 別 DM から戻ってきたときに最新モードが反映されない（Firefox の
        // タブ切替で反転する症状の原因）。
        doc_mode_remember_current(mode);
    }
}

/// ホットパス用
#[inline]
pub fn ime_state_get() -> anyhow::Result<MutexGuard<'static, IMEState>> {
    IMEState::try_get()
}

// ─── CompositionWrapper ───────────────────────────────────────────────────────

use windows::Win32::UI::TextServices::ITfComposition;

pub(crate) struct CompositionWrapper {
    comp: Option<ITfComposition>,
    dm_ptr: usize,
    stale: bool,
}
unsafe impl Send for CompositionWrapper {}
unsafe impl Sync for CompositionWrapper {}

pub static COMPOSITION: LazyLock<Mutex<CompositionWrapper>> = LazyLock::new(|| {
    Mutex::new(CompositionWrapper {
        comp: None,
        dm_ptr: 0,
        stale: false,
    })
});

/// ホットパス用: ブロックしない
#[inline]
pub fn composition_try_get() -> anyhow::Result<MutexGuard<'static, CompositionWrapper>> {
    COMPOSITION
        .try_lock()
        .map_err(|_| anyhow::anyhow!("composition busy"))
}

pub fn composition_set(comp: Option<ITfComposition>) -> anyhow::Result<()> {
    composition_set_with_dm(comp, 0)
}

pub fn composition_set_with_dm(comp: Option<ITfComposition>, dm_ptr: usize) -> anyhow::Result<()> {
    // set はホットパスでもブロックを許容（短い操作のため）
    let mut g = COMPOSITION.lock().map_err(|p| {
        let _ = p;
        anyhow::anyhow!("composition poison")
    })?;
    g.comp = comp;
    g.dm_ptr = if g.comp.is_some() { dm_ptr } else { 0 };
    g.stale = false;
    Ok(())
}

pub fn composition_take() -> anyhow::Result<Option<ITfComposition>> {
    let mut g = composition_try_get()?;
    if g.stale {
        g.dm_ptr = 0;
        g.stale = false;
        let _ = g.comp.take();
        return Ok(None);
    }
    let comp = g.comp.take();
    g.dm_ptr = 0;
    Ok(comp)
}

pub fn composition_clone() -> anyhow::Result<Option<ITfComposition>> {
    let g = composition_try_get()?;
    if g.stale {
        return Ok(None);
    }
    Ok(g.comp.clone())
}

/// M1.8 T-MID3: composition の `SetText` を直列化するための排他ロック。
///
/// Phase1A (`candidate_window.rs` の live preview EditSession) と
/// `update_composition` / `update_composition_candidate_parts`
/// (factory.rs の通常 / 候補表示 EditSession) はそれぞれ別経路で
/// 同じ composition の range に `SetText` する。TSF EditSession は STA で
/// 直列実行されるが、deferred dispatch の順序が保証されないため、
/// 古い経路の SetText が新しい経路の SetText を上書きする risk がある。
///
/// 各 SetText 直前で `try_lock` を取り、busy なら skip して return。
/// 取りこぼした apply は次のキー入力 / タイマー発火で最新 gen の SetText が
/// 走るので整合は保てる（M1.8 T-MID1 の gen 機構と組合せて機能）。
pub static COMPOSITION_APPLY_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// `OnUninitDocumentMgr` から呼ばれる。
///
/// msctf コールバック中に stale な `ITfComposition` を drop しないよう、
/// ここでは mark だけ付け、実際の破棄は後続の安全な文脈へ遅延させる。
pub fn invalidate_composition_for_dm(dm_ptr: usize) {
    if dm_ptr == 0 {
        return;
    }

    let Ok(mut g) = composition_try_get() else {
        tracing::trace!("composition invalidate skipped: busy dm={dm_ptr:#x}");
        return;
    };

    if g.comp.is_some() && g.dm_ptr == dm_ptr {
        g.stale = true;
        tracing::debug!("composition marked stale for dm={dm_ptr:#x}");
    }
}

// ─── SessionState ────────────────────────────────────────────────────────────
// TSF 層の論理状態を 1 か所に集約する。SelectionState は縮退・削除済み。

#[derive(Debug, Clone, Default)]
pub enum SessionState {
    #[default]
    Idle,
    Preedit {
        text: String,
    },
    Waiting {
        text: String,
        pos_x: i32,
        pos_y: i32,
        /// 範囲指定変換から遷移してきた場合の「変換しない残り部分」
        /// （通常の on_convert 経由では空）。
        /// Waiting → Selecting 遷移時に `activate_selecting_with_affixes` に渡す。
        remainder: String,
        remainder_reading: String,
    },
    Selecting {
        original_preedit: String,
        candidates: Vec<String>,
        selected: usize,
        page_size: usize,
        llm_pending: bool,
        pos_x: i32,
        pos_y: i32,
        /// 句読点保留（「、」「。」押下時にセット、確定時に末尾連結）
        punct_pending: Option<char>,
        prefix: String,
        #[allow(dead_code)]
        prefix_reading: String,
        /// 文節分割後に変換した場合の残り部分（確定後に次のプリエディットになる）
        remainder: String,
        /// 文節分割後に変換した場合の残り部分の読み
        remainder_reading: String,
    },
    /// 範囲指定変換モード。
    ///
    /// ライブ変換中に Shift+矢印を押すと、全文がひらがなに戻り、
    /// 先頭から Shift+Right で変換範囲を指定する。
    ///
    /// - `full_reading` : 全体のひらがな（変換前）
    /// - `select_end`   : 選択範囲の終了位置（文字数、先頭から）
    ///
    /// 表示: [selected_reading] + unselected_reading
    ///   selected_reading = full_reading[..select_end] （実線アンダーライン）
    ///   unselected_reading = full_reading[select_end..] （点線アンダーライン）
    ///
    /// 遷移:
    ///   Shift+Right  → select_end += 1
    ///   Shift+Left   → select_end -= 1 (最小 1)
    ///   Space        → selected_reading を engine.convert して候補表示（Selecting へ）
    ///   Enter        → selected_reading をそのまま確定、残りで LiveConv 再開
    ///   ESC          → LiveConv に戻る（元の preview を復元）
    RangeSelect {
        full_reading: String,
        select_end: usize,
        /// ESC で戻るための元の preview
        original_preview: String,
    },
    /// ライブ変換表示中。
    ///
    /// BG 変換が完了しトップ候補を composition に表示している状態。
    /// キーを押していないので候補ウィンドウは出ない（Preedit の視覚的な上書き）。
    ///
    /// - `reading` : エンジンの hiragana_buf（Space 押下時の変換キー）
    /// - `preview` : BG 変換のトップ候補（現在 composition に表示中）
    ///
    /// 遷移:
    ///   Enter        → preview をコミット
    ///   Space        → reading で on_convert（通常変換フロー）
    ///   Input(c)     → Preedit へ戻し新文字を処理
    ///   Backspace/ESC → Preedit へ戻し reading を再表示
    ///   IME オフ     → preview をコミット
    LiveConv {
        reading: String,
        preview: String,
    },
}

pub static SESSION_STATE: LazyLock<Mutex<SessionState>> =
    LazyLock::new(|| Mutex::new(SessionState::Idle));

pub static SESSION_SELECTING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

// ─── LIVE_PREVIEW キュー（Phase 1B: WM_TIMER → handle_action 橋渡し）─────────
//
// WM_TIMER コールバック（WndProc）から RequestEditSession が呼べない場合の
// フォールバック。タイマーが変換結果をここに書き込み、次のキー入力時に
// handle_action が読み出して composition を更新する。
//
// Phase 1A（WM_TIMER から直接 RequestEditSession）が確認できれば使わなくなる。
//
// # gen タグ（M1.8 T-MID1）
// `PreviewEntry.gen_when_requested` は preview 要求時点の `LIVE_CONV_GEN`
// スナップショット。キュー消費時に現在値と比較し、古ければ apply せず
// discard する（reading が進んでいるのに古い preview で中間を上書きする race
// を防ぐ）。reading も保持し、世代一致でも reading 不一致なら破棄する二重安全策。

#[derive(Debug, Clone)]
pub struct PreviewEntry {
    pub preview: String,
    pub reading: String,
    pub gen_when_requested: u32,
}

pub static LIVE_PREVIEW_QUEUE: LazyLock<Mutex<Option<PreviewEntry>>> =
    LazyLock::new(|| Mutex::new(None));
pub static LIVE_PREVIEW_READY: AtomicBool = AtomicBool::new(false);
pub static SUPPRESS_LIVE_COMMIT_ONCE: AtomicBool = AtomicBool::new(false);

/// ライブ変換中の reading 世代カウンタ（M1.8 T-MID1）。
///
/// reading を変更する経路（キー入力 / backspace / pending flush / commit 等）
/// で `fetch_add(1, Release)` する。Phase1B キューへの書き込み時点の値と
/// apply 時点の値を比較し、古ければ stale として discard する。
pub static LIVE_CONV_GEN: AtomicU32 = AtomicU32::new(0);

/// reading を変更する全経路から呼ぶ。世代カウンタを前進させる。
#[inline]
pub fn live_conv_gen_bump() {
    LIVE_CONV_GEN.fetch_add(1, AO::Release);
}

#[inline]
pub fn live_conv_gen_snapshot() -> u32 {
    LIVE_CONV_GEN.load(AO::Acquire)
}

pub fn session_get() -> anyhow::Result<MutexGuard<'static, SessionState>> {
    SESSION_STATE
        .lock()
        .map_err(|_| anyhow::anyhow!("session_state poisoned"))
}

#[inline]
pub fn session_is_selecting_fast() -> bool {
    SESSION_SELECTING.load(std::sync::atomic::Ordering::Acquire)
}

impl SessionState {
    pub fn set_idle(&mut self) {
        *self = SessionState::Idle;
        SESSION_SELECTING.store(false, std::sync::atomic::Ordering::Release);
    }

    pub fn set_preedit(&mut self, text: String) {
        *self = SessionState::Preedit { text };
        SESSION_SELECTING.store(false, std::sync::atomic::Ordering::Release);
    }

    /// ライブ変換表示状態へ遷移。
    /// `reading` = hiragana_buf（変換キー）、`preview` = BG トップ候補。
    pub fn set_live_conv(&mut self, reading: String, preview: String) {
        *self = SessionState::LiveConv { reading, preview };
        SESSION_SELECTING.store(false, std::sync::atomic::Ordering::Release);
    }

    pub fn is_live_conv(&self) -> bool {
        matches!(self, SessionState::LiveConv { .. })
    }

    /// LiveConv の (reading, preview) を返す。
    pub fn live_conv_parts(&self) -> Option<(&str, &str)> {
        if let SessionState::LiveConv { reading, preview } = self {
            Some((reading.as_str(), preview.as_str()))
        } else {
            None
        }
    }

    pub fn set_range_select(
        &mut self,
        full_reading: String,
        select_end: usize,
        original_preview: String,
    ) {
        *self = SessionState::RangeSelect {
            full_reading,
            select_end,
            original_preview,
        };
        SESSION_SELECTING.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn is_range_select(&self) -> bool {
        matches!(self, SessionState::RangeSelect { .. })
    }

    /// RangeSelect の選択範囲を 1 文字伸ばす。戻り値: 成功したか。
    pub fn range_select_extend(&mut self) -> bool {
        if let SessionState::RangeSelect {
            full_reading,
            select_end,
            ..
        } = self
        {
            let max = full_reading.chars().count();
            if *select_end < max {
                *select_end += 1;
                return true;
            }
        }
        false
    }

    /// RangeSelect の選択範囲を 1 文字縮める。戻り値: 成功したか。
    pub fn range_select_shrink(&mut self) -> bool {
        if let SessionState::RangeSelect { select_end, .. } = self {
            if *select_end > 1 {
                *select_end -= 1;
                return true;
            }
        }
        false
    }

    /// RangeSelect の (selected_reading, unselected_reading) を返す。
    pub fn range_select_parts(&self) -> Option<(String, String)> {
        if let SessionState::RangeSelect {
            full_reading,
            select_end,
            ..
        } = self
        {
            let chars: Vec<char> = full_reading.chars().collect();
            let end = (*select_end).min(chars.len());
            let selected: String = chars[..end].iter().collect();
            let unselected: String = chars[end..].iter().collect();
            Some((selected, unselected))
        } else {
            None
        }
    }

    /// RangeSelect の元の preview を返す（ESC で復帰用）。
    #[allow(dead_code)]
    pub fn range_select_original_preview(&self) -> Option<&str> {
        if let SessionState::RangeSelect {
            original_preview, ..
        } = self
        {
            Some(original_preview.as_str())
        } else {
            None
        }
    }

    pub fn set_waiting(&mut self, text: String, pos_x: i32, pos_y: i32) {
        self.set_waiting_with_affixes(text, pos_x, pos_y, String::new(), String::new());
    }

    /// 範囲指定変換 → WM_TIMER 経由のための Waiting 遷移。
    /// `remainder` / `remainder_reading` は Waiting → Selecting 昇格時に
    /// `activate_selecting_with_affixes` へ渡される。
    pub fn set_waiting_with_affixes(
        &mut self,
        text: String,
        pos_x: i32,
        pos_y: i32,
        remainder: String,
        remainder_reading: String,
    ) {
        *self = SessionState::Waiting {
            text,
            pos_x,
            pos_y,
            remainder,
            remainder_reading,
        };
        SESSION_SELECTING.store(false, std::sync::atomic::Ordering::Release);
    }

    pub fn activate_selecting(
        &mut self,
        candidates: Vec<String>,
        original_preedit: String,
        pos_x: i32,
        pos_y: i32,
        llm_pending: bool,
    ) {
        self.activate_selecting_with_affixes(
            candidates,
            original_preedit,
            pos_x,
            pos_y,
            llm_pending,
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        );
    }

    pub fn activate_selecting_with_affixes(
        &mut self,
        candidates: Vec<String>,
        original_preedit: String,
        pos_x: i32,
        pos_y: i32,
        llm_pending: bool,
        prefix: String,
        prefix_reading: String,
        remainder: String,
        remainder_reading: String,
    ) {
        *self = SessionState::Selecting {
            original_preedit,
            candidates,
            selected: 0,
            page_size: 9,
            llm_pending,
            pos_x,
            pos_y,
            punct_pending: None,
            prefix,
            prefix_reading,
            remainder,
            remainder_reading,
        };
        SESSION_SELECTING.store(true, std::sync::atomic::Ordering::Release);
    }

    pub fn is_selecting(&self) -> bool {
        matches!(self, SessionState::Selecting { .. })
    }

    pub fn is_candidate_list_active(&self) -> bool {
        self.is_selecting()
    }

    pub fn is_waiting(&self) -> bool {
        matches!(self, SessionState::Waiting { .. })
    }

    pub fn preedit_text(&self) -> Option<&str> {
        match self {
            SessionState::Preedit { text } => Some(text.as_str()),
            SessionState::Waiting { text, .. } => Some(text.as_str()),
            SessionState::Selecting {
                original_preedit, ..
            } => Some(original_preedit.as_str()),
            SessionState::LiveConv { preview, .. } => Some(preview.as_str()),
            SessionState::RangeSelect { full_reading, .. } => Some(full_reading.as_str()),
            SessionState::Idle => None,
        }
    }

    pub fn waiting_info(&self) -> Option<(&str, i32, i32)> {
        match self {
            SessionState::Waiting {
                text, pos_x, pos_y, ..
            } => Some((text.as_str(), *pos_x, *pos_y)),
            _ => None,
        }
    }


    pub fn current_candidate(&self) -> Option<&str> {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                ..
            } => candidates.get(*selected).map(|s| s.as_str()),
            _ => None,
        }
    }

    pub fn original_preedit(&self) -> Option<&str> {
        match self {
            SessionState::Selecting {
                original_preedit, ..
            } => Some(original_preedit.as_str()),
            SessionState::Preedit { text } => Some(text.as_str()),
            SessionState::Waiting { text, .. } => Some(text.as_str()),
            SessionState::LiveConv { reading, .. } => Some(reading.as_str()),
            SessionState::RangeSelect { full_reading, .. } => Some(full_reading.as_str()),
            SessionState::Idle => None,
        }
    }

    /// Selecting 状態の remainder を取り出す（空文字列の場合は空 String）
    pub fn take_selecting_remainder(&mut self) -> String {
        if let SessionState::Selecting { remainder, .. } = self {
            std::mem::take(remainder)
        } else {
            String::new()
        }
    }

    /// Selecting 状態の remainder を参照する（コピーを返す）
    pub fn selecting_remainder_clone(&self) -> String {
        if let SessionState::Selecting { remainder, .. } = self {
            remainder.clone()
        } else {
            String::new()
        }
    }

    pub fn selecting_remainder_reading_clone(&self) -> String {
        if let SessionState::Selecting {
            remainder_reading, ..
        } = self
        {
            remainder_reading.clone()
        } else {
            String::new()
        }
    }

    pub fn selecting_prefix_clone(&self) -> String {
        if let SessionState::Selecting { prefix, .. } = self {
            prefix.clone()
        } else {
            String::new()
        }
    }

    #[allow(dead_code)]
    pub fn selecting_prefix_reading_clone(&self) -> String {
        if let SessionState::Selecting { prefix_reading, .. } = self {
            prefix_reading.clone()
        } else {
            String::new()
        }
    }

    pub fn current_page(&self) -> usize {
        match self {
            SessionState::Selecting {
                selected,
                page_size,
                ..
            } => selected / page_size,
            _ => 0,
        }
    }

    pub fn total_pages(&self) -> usize {
        match self {
            SessionState::Selecting {
                candidates,
                page_size,
                ..
            } => {
                if candidates.is_empty() {
                    0
                } else {
                    (candidates.len() + page_size - 1) / page_size
                }
            }
            _ => 0,
        }
    }

    pub fn page_candidates(&self) -> Vec<String> {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                page_size,
                ..
            } => {
                if candidates.is_empty() {
                    return Vec::new();
                }
                let start = (selected / page_size) * page_size;
                let end = (start + page_size).min(candidates.len());
                candidates[start..end].to_vec()
            }
            _ => Vec::new(),
        }
    }

    pub fn page_selected(&self) -> usize {
        match self {
            SessionState::Selecting {
                selected,
                page_size,
                ..
            } => selected % page_size,
            _ => 0,
        }
    }

    pub fn page_info(&self) -> String {
        let total = self.total_pages();
        if total <= 1 {
            String::new()
        } else {
            format!("{}/{}", self.current_page() + 1, total)
        }
    }

    pub fn next_with_page_wrap(&mut self) {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                page_size,
                ..
            } => {
                if candidates.is_empty() {
                    return;
                }
                let next_idx = (*selected + 1) % candidates.len();
                let cur_page = *selected / *page_size;
                let next_page = next_idx / *page_size;
                *selected = if next_page != cur_page {
                    next_page * *page_size
                } else {
                    next_idx
                };
            }
            _ => {}
        }
    }

    pub fn prev(&mut self) {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                ..
            } => {
                if candidates.is_empty() {
                    return;
                }
                *selected = if *selected == 0 {
                    candidates.len() - 1
                } else {
                    *selected - 1
                };
            }
            _ => {}
        }
    }

    pub fn next_page(&mut self) {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                page_size,
                ..
            } => {
                if candidates.is_empty() {
                    return;
                }
                let total_pages = candidates.len().div_ceil(*page_size);
                let cur = *selected / *page_size;
                let next = (cur + 1) % total_pages;
                *selected = next * *page_size;
            }
            _ => {}
        }
    }

    pub fn prev_page(&mut self) {
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                page_size,
                ..
            } => {
                if candidates.is_empty() {
                    return;
                }
                let total_pages = candidates.len().div_ceil(*page_size);
                let cur = *selected / *page_size;
                let prev = if cur == 0 { total_pages - 1 } else { cur - 1 };
                *selected = prev * *page_size;
            }
            _ => {}
        }
    }

    pub fn select_nth_in_page(&mut self, n: usize) -> bool {
        if n < 1 {
            return false;
        }
        match self {
            SessionState::Selecting {
                candidates,
                selected,
                page_size,
                ..
            } => {
                let idx = (*selected / *page_size) * *page_size + (n - 1);
                if idx < candidates.len() {
                    *selected = idx;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// 句読点保留をセット（確定時に変換テキスト末尾に連結する）
    pub fn set_punct_pending(&mut self, c: char) {
        if let SessionState::Selecting { punct_pending, .. } = self {
            *punct_pending = Some(c);
        }
    }

    /// 句読点保留を取り出す
    pub fn take_punct_pending(&mut self) -> Option<char> {
        if let SessionState::Selecting { punct_pending, .. } = self {
            punct_pending.take()
        } else {
            None
        }
    }

    /// Selecting 状態での pos_x, pos_y を返す
    pub fn selecting_pos(&self) -> Option<(i32, i32)> {
        if let SessionState::Selecting { pos_x, pos_y, .. } = self {
            Some((*pos_x, *pos_y))
        } else {
            None
        }
    }
}

// ─── CARET_RECT ──────────────────────────────────────────────────────────────
// GetTextExt で取得したキャレット矩形をEditSession→handlerに渡す橋渡し用。
// RECT は外部クレートの型なので newtype でラップして Send/Sync を実装する。

use windows::Win32::Foundation::RECT;

pub(crate) struct CaretRect(RECT);
unsafe impl Send for CaretRect {}
unsafe impl Sync for CaretRect {}

pub static CARET_RECT: LazyLock<Mutex<CaretRect>> =
    LazyLock::new(|| Mutex::new(CaretRect(RECT::default())));

pub fn caret_rect_set(r: RECT) {
    if let Ok(mut g) = CARET_RECT.lock() {
        g.0 = r;
    }
}

pub fn caret_rect_get() -> RECT {
    CARET_RECT.lock().map(|g| g.0).unwrap_or_default()
}

// ─── LangBar 更新通知 ─────────────────────────────────────────────────────────
// バックグラウンドスレッドでエンジン初期化が完了したとき、
// 言語バー表示を更新するためのフラグ。
// STA スレッドが次回キー入力時にこれを確認して OnUpdate を呼ぶ。

use std::sync::atomic::Ordering as AtomicOrdering;

pub static LANGBAR_UPDATE_PENDING: AtomicBool = AtomicBool::new(false);

#[allow(dead_code)]
pub fn langbar_update_set() {
    LANGBAR_UPDATE_PENDING.store(true, AtomicOrdering::Release);
}
pub fn langbar_update_take() -> bool {
    LANGBAR_UPDATE_PENDING.swap(false, AtomicOrdering::AcqRel)
}

// ─── DocumentManager モードストア ────────────────────────────────────────────
//
// MS-IME準拠: アプリ（DocumentManager）ごとに InputMode を記憶する。
//
// # キー戦略
// Edge・Firefox 等のブラウザはページ遷移やタブ切り替えのたびに
// DocumentManager を新規作成・破棄する。DM ポインタだけをキーにすると
// DM が再作成されるたびにモードがリセットされる。
//
// そのため DM ポインタと HWND の 2 段階フォールバックを採用する:
//   1. dm_modes: DM ポインタ → モード（正確なマッチ）
//   2. hwnd_modes: HWND → モード（DM 再作成時のフォールバック）
//   3. dm_to_hwnd: DM ポインタ → HWND（保存時に HWND も更新するために必要）
//
// モードの保存タイミング（focus が離れるとき）:
//   - dm_modes[prev_dm_ptr] = 現在モード
//   - dm_to_hwnd で prev_dm_ptr の HWND を引いて hwnd_modes[hwnd] = 現在モード
//
// モードの復元タイミング（focus が来るとき）:
//   - dm_modes に next_dm_ptr が存在: それを返す
//   - なければ hwnd_modes に next_hwnd が存在: それを返す（ブラウザの DM 再作成対応）
//   - なければ config.input.default_mode を返す

struct ModeStore {
    dm_modes: HashMap<usize, InputMode>,   // DM ptr → mode
    hwnd_modes: HashMap<usize, InputMode>, // HWND → mode（DM 再作成時フォールバック）
    dm_to_hwnd: HashMap<usize, usize>,     // DM ptr → HWND（保存時の HWND 特定用）
}

static DOC_MODE_STORE: LazyLock<Mutex<ModeStore>> = LazyLock::new(|| {
    Mutex::new(ModeStore {
        dm_modes: HashMap::new(),
        hwnd_modes: HashMap::new(),
        dm_to_hwnd: HashMap::new(),
    })
});

/// DocumentManager のフォーカス変化時に呼ぶ。
///
/// - `prev_dm_ptr`: フォーカスを失った DocumentManager のポインタ（0 = なし）
/// - `next_dm_ptr`: フォーカスを得た DocumentManager のポインタ（0 = なし）
/// - `next_hwnd`: フォーカス先ウィンドウの HWND（ターミナル判定用）
///
/// 返り値: フォーカス先に適用すべき InputMode
pub fn doc_mode_on_focus_change(
    prev_dm_ptr: usize,
    next_dm_ptr: usize,
    next_hwnd: usize,
) -> Option<InputMode> {
    use super::config::{current_config, DefaultInputMode};

    let cfg = current_config();
    let remember = cfg.input.remember_last_kana_mode;

    // config.input.default_mode → InputMode へ変換
    let config_default = match cfg.input.default_mode {
        DefaultInputMode::Alphanumeric => InputMode::Alphanumeric,
        DefaultInputMode::Hiragana => InputMode::Hiragana,
    };

    let mut store = match DOC_MODE_STORE.try_lock() {
        Ok(g) => g,
        Err(_) => return None,
    };

    // 前の DocumentManager のモードを保存
    if prev_dm_ptr != 0 && remember {
        let mode = input_mode_get_atomic();
        store.dm_modes.insert(prev_dm_ptr, mode);
        // HWND も更新（ブラウザが DM を再作成しても HWND 経由で復元できるように）
        if let Some(&hwnd) = store.dm_to_hwnd.get(&prev_dm_ptr) {
            if hwnd != 0 {
                store.hwnd_modes.insert(hwnd, mode);
                tracing::debug!(
                    "doc_mode: saved mode={mode:?} for dm={prev_dm_ptr:#x} hwnd={hwnd:#x}"
                );
            }
        } else {
            tracing::debug!("doc_mode: saved mode={mode:?} for dm={prev_dm_ptr:#x} (hwnd unknown)");
        }
    }

    if next_dm_ptr == 0 {
        return None;
    }

    // DM→HWND マッピングを更新（フォーカスが来るたびに記録）
    if next_hwnd != 0 {
        store.dm_to_hwnd.insert(next_dm_ptr, next_hwnd);
    }

    // 初回フォーカス時のデフォルトモードを決定
    // ターミナルは config に関わらず常に Alphanumeric
    let resolve_default = |hwnd: usize| -> InputMode {
        if is_terminal_hwnd(hwnd) {
            tracing::debug!("doc_mode: terminal detected (hwnd={hwnd:#x}), default=Alphanumeric");
            InputMode::Alphanumeric
        } else {
            tracing::debug!("doc_mode: default={config_default:?} (config.input.default_mode)");
            config_default
        }
    };

    let mode = if remember {
        if let Some(&saved) = store.dm_modes.get(&next_dm_ptr) {
            // 既知の DM → 前回モードを復元
            tracing::debug!("doc_mode: restored mode={saved:?} from dm={next_dm_ptr:#x}");
            saved
        } else if let Some(&saved) = store.hwnd_modes.get(&next_hwnd) {
            // DM は新規だが同じ HWND → HWND 経由で復元（ブラウザの DM 再作成対応）
            tracing::debug!(
                "doc_mode: restored mode={saved:?} from hwnd={next_hwnd:#x} (dm={next_dm_ptr:#x} is new)"
            );
            store.dm_modes.insert(next_dm_ptr, saved);
            saved
        } else {
            // 完全初回 → デフォルトモードを記録して返す
            let m = resolve_default(next_hwnd);
            store.dm_modes.insert(next_dm_ptr, m);
            if next_hwnd != 0 {
                store.hwnd_modes.insert(next_hwnd, m);
            }
            m
        }
    } else {
        // remember=false: 毎回デフォルトモードを適用
        resolve_default(next_hwnd)
    };

    Some(mode)
}

/// M1.7 T-MODE2: モード変更が起きた瞬間に現在フォーカス中の DM / HWND を
/// キーに store を即時更新する。従来は focus-out 時にしか `dm_modes` /
/// `hwnd_modes` が書かれなかったため、同じ DM 内でモードを変えた直後に
/// DM が破棄されると最新モードが永久に失われていた（特に Firefox で DM が
/// 頻繁に再作成されるケースで、タブ切替時にモードが反転する原因）。
///
/// 呼び出し元は [IMEState::set_mode]。TL_CURRENT_DM / TL_CURRENT_HWND は
/// focus 切替の deferred 処理で更新される。
///
/// TSF スレッド以外（例: WinUI → 設定反映）からの呼び出しでは TL が 0 を返すため
/// save を skip する。
pub fn doc_mode_remember_current(mode: InputMode) {
    let (dm, hwnd) = crate::tsf::candidate_window::current_dm_hwnd();
    if dm == 0 && hwnd == 0 {
        return;
    }
    if let Ok(mut store) = DOC_MODE_STORE.try_lock() {
        if dm != 0 {
            store.dm_modes.insert(dm, mode);
            if hwnd != 0 {
                store.dm_to_hwnd.insert(dm, hwnd);
            }
        }
        if hwnd != 0 {
            store.hwnd_modes.insert(hwnd, mode);
        }
        tracing::trace!(
            "doc_mode: remembered mode={mode:?} for dm={dm:#x} hwnd={hwnd:#x}"
        );
    }
}

/// DocumentManager が破棄されたとき（OnUninitDocumentMgr）にエントリを削除する。
/// hwnd_modes は残す（同じ HWND で DM が再作成されたとき復元に使うため）。
///
/// ブラウザ（Chrome / Edge / Firefox）はタブ切替で DM を破棄→再作成するが、
/// その際 OnUninitDocumentMgr が OnSetFocus より先に同期発火するため、
/// 通常の focus-out 経路では `dm_to_hwnd` が削除済みになっていて HWND 退避が
/// 走らない。ここで破棄前に HWND へコピーしておくことで、同じ HWND で
/// 新しい DM が作られたときに hwnd_modes から復元できる。
pub fn doc_mode_remove(dm_ptr: usize) {
    if let Ok(mut store) = DOC_MODE_STORE.try_lock() {
        if let (Some(&mode), Some(&hwnd)) =
            (store.dm_modes.get(&dm_ptr), store.dm_to_hwnd.get(&dm_ptr))
            && hwnd != 0
        {
            store.hwnd_modes.insert(hwnd, mode);
            tracing::debug!(
                "doc_mode: retained mode={mode:?} for hwnd={hwnd:#x} before removing dm={dm_ptr:#x}"
            );
        }
        store.dm_modes.remove(&dm_ptr);
        store.dm_to_hwnd.remove(&dm_ptr);
        tracing::trace!("doc_mode: removed dm={dm_ptr:#x}");
    }
}

/// DocumentManager 破棄時 (`OnUninitDocumentMgr`) の後片付けを集約する (M1 T3-B)。
///
/// かつては `OnUninitDocumentMgr` から 3 つの関数を個別に呼んでいたが、
/// どれか 1 つを忘れると DM ごとの状態がリークして不整合になるため、
/// 追加先を 1 箇所に寄せておく。呼び出し順は既存のままで、モード退避 →
/// ライブ変換 context の無効化 → composition の無効化。
pub fn dispose_dm_resources(dm_ptr: usize) {
    doc_mode_remove(dm_ptr);
    crate::tsf::candidate_window::invalidate_live_context_for_dm(dm_ptr);
    invalidate_composition_for_dm(dm_ptr);
}

/// HWND がターミナル系ウィンドウかどうかを判定する。
///
/// 判定対象:
/// - Windows Terminal: `CASCADIA_HOSTING_WINDOW_CLASS`
/// - 旧来の ConHost:   `ConsoleWindowClass`
/// - VSCode 統合ターミナル等は親が上記クラスを持つ場合あり（簡易判定のみ）
fn is_terminal_hwnd(hwnd_val: usize) -> bool {
    if hwnd_val == 0 {
        return false;
    }

    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

    let hwnd = HWND(hwnd_val as *mut _);
    let mut buf = [0u16; 256];
    let len = unsafe { GetClassNameW(hwnd, &mut buf) } as usize;
    if len == 0 {
        return false;
    }

    let class_name = String::from_utf16_lossy(&buf[..len]);
    tracing::trace!("doc_mode: hwnd={hwnd_val:#x} class={class_name:?}");

    matches!(
        class_name.as_str(),
        "CASCADIA_HOSTING_WINDOW_CLASS"  // Windows Terminal
        | "ConsoleWindowClass"           // conhost.exe
        | "VirtualConsoleClass"          // mintty 等
        | "mintty"
    )
}
