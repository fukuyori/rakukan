# rakukan handoff — 2026-03-17

## 現在のバージョン
v0.3.2（`VERSION` / `CHANGELOG.md` / `Cargo.toml` 更新済み）

## プロジェクト構成

| パス | 役割 |
|------|------|
| `crates/rakukan-engine/src/conv_cache.rs` | BG変換キャッシュ（状態機械＋Condvar） |
| `crates/rakukan-engine/src/lib.rs` | エンジン本体（`bg_start` / `bg_reclaim` / `merge_candidates`） |
| `crates/rakukan-engine-abi/src/lib.rs` | エンジン DLL FFI インターフェース |
| `crates/rakukan-tsf/src/tsf/factory.rs` | TSF メインロジック（キーハンドリング・変換フロー） |
| `crates/rakukan-tsf/src/tsf/candidate_window.rs` | 候補ウィンドウ（GDI描画 + WM_TIMERポーリング） |
| `crates/rakukan-tsf/src/engine/state.rs` | SessionState 定義・エンジン初期化 |
| `crates/rakukan-tsf/src/engine/config.rs` | config.toml 読み込み・デフォルト生成 |
| `config/config.toml` | サンプル設定ファイル |
| `rakukan_installer.iss` | Inno Setup インストーラースクリプト |

## ビルドコマンド

```powershell
# conv_cache.rs / lib.rs 変更時（エンジン DLL 再ビルドが必要）
cargo make build-engine
cargo make reinstall

# factory.rs / candidate_window.rs / state.rs のみ変更時
cargo make reinstall
```

> **注意**: エンジン DLL の変更を `reinstall` だけで反映しようとすると
> IME がランゲージバーで選択不能になる。FFI/vtable 変更は必ず `build-engine` から。

## v0.3.2 で修正・変更した内容

### [FIX] Activate 時の UIスレッドブロック（state.rs / factory.rs）
**症状**: アプリ切り替え時にメモ帳等が「応答なし」になる（最大 5.7 秒フリーズ）
**原因**: `Activate`（UIスレッド）内で `engine_get_or_create()` → CUDA DLL ロードを同期実行
**修正**:
- `engine_start_bg_init()` を追加（DLL ロードを別スレッドで非同期実行）
- `ENGINE_INIT_STARTED: AtomicBool` で二重スポーン防止
- `Activate` 内を `engine_start_bg_init()` 1行に置き換え
- `engine_try_get_or_create()` から同期 DLL ロードを除去

### [FIX] rakukan_engine_cuda.dll のロード失敗
**症状**: CUDA モードで `DLL load failed: rakukan_engine_cuda.dll`
**原因**:
1. `llama-cpp-sys-2 0.1.137` が `nvcudart_hybrid64.dll` を要求するが CUDA 13.x Toolkit に非同梱
2. `cublas64_13.dll` 等が System32 になく、DLL ロード時に見つからない
**修正**:
- `llama-cpp-sys-2` を 0.1.138 に更新（`cargo update`）
- `C:\rb`（ビルドキャッシュ）を削除してフルビルド（31分）
- 以下の DLL を `C:\Windows\System32` に手動コピー（管理者権限）:
  - `nvcudart_hybrid64.dll`（`cudart64_13.dll` のコピー）
  - `cublas64_13.dll`
  - `cublasLt64_13.dll`
  - `cudart64_13.dll`

### [CHANGED] config.toml の gpu_backend 説明拡充
- `cuda` / `vulkan` / `cpu` の説明と対応 GPU を明記
- `config/config.toml` と `config.rs` の `default_config_text()` 両方を更新

### [CHANGED] Inno Setup の config.toml 配置先修正
- `{app}`（LocalAppData）→ `%APPDATA%\rakukan`（Roaming）に修正
- `GetRoamingConfigDir()` 関数を追加

## CUDA 動作環境（実機確認済み）

| 項目 | 内容 |
|------|------|
| GPU | NVIDIA GeForce RTX 5070 |
| CUDA Toolkit | 13.2 |
| ドライバー | 591.86 |
| llama-cpp-sys-2 | 0.1.138 |
| 変換速度 | 127〜444ms（CPUとほぼ同等、モデルが小さいため） |

## 残課題

- [ ] CUDA DLL（`cublas64_13.dll` 等）の System32 コピーをインストーラーに組み込む
- [ ] ライブ変換（Phase 4）
- [ ] SplitPreedit の複数文節連続変換
- [ ] WM_TIMER フォールバック時の composition text 更新

## ログ収集コマンド

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 80 |
  Select-String "on_convert|bg_wait|completed|engine-init|engine created|DLL load|SLOW"
```
