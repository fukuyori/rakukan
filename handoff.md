# Rakukan 引き継ぎ資料 (v0.5.1)

更新日: 2026-04-16

## 現在の状態

- **バージョン:** v0.5.1
- **位置づけ:** 変換パイプライン改修完了（数値保護 + 範囲指定変換 + vibrato/SplitPreedit 完全削除）
- **ソース:** `C:\Users\n_fuk\source\rust\rakukan`
- **インストール先:** `%LOCALAPPDATA%\rakukan\`
- **設定:** `%APPDATA%\rakukan\config.toml`
- **ログ:**
  - TSF 側: `%LOCALAPPDATA%\rakukan\rakukan.log`
  - エンジンホスト側: `%LOCALAPPDATA%\rakukan\rakukan-engine-host.log`

## 0.4.4 の目玉: エンジン別プロセス化

### 背景

0.4.3 までは `rakukan_engine_*.dll`（llama.cpp 同梱）を TSF DLL から直接 LoadLibrary していたため、Zoom / Dropbox / explorer といった **IME を実際には使わないアプリ** のプロセスにも llama.cpp とそのランタイム（`msvcp140.dll` 等）が持ち込まれ、`msvcp140.dll` のクロスロード起因で `0xc0000005` による異常終了を誘発していた。

### 解決策

engine DLL を TSF ホストプロセスに持ち込まず、**専用の `rakukan-engine-host.exe`** に集約する。TSF 側は Windows Named Pipe で RPC するクライアントとしてのみ振る舞う。

```text
┌──────────────────────┐        Named Pipe          ┌────────────────────────┐
│ Zoom.exe / Dropbox / │  \\.\pipe\rakukan-engine- │  rakukan-engine-host   │
│ explorer / ...       │◀──────────(SID)───────────▶│  .exe (1 個、常駐)     │
│                      │                            │                        │
│  rakukan_tsf.dll     │                            │  rakukan_engine_*.dll  │
│   ├ rakukan-engine-  │                            │   ├ llama.cpp          │
│   │   rpc (client)   │                            │   ├ rakukan-dict       │
│   └ ❌ engine DLL    │                            │   └ Vulkan / CUDA 等   │
└──────────────────────┘                            └────────────────────────┘
        ↑                                                     ↑
        └─ llama.cpp を一切ロードしない                       └─ GPU バックエンドはここだけ
```

### 影響

- Zoom / Dropbox の異常終了が解消（実機確認済み）
- `rakukan_engine_*.dll` は TSF プロセス（= あらゆる Windows アプリケーション）ではなく `rakukan-engine-host.exe` だけにロードされる
- `rakukan-tsf` クレートの `rakukan-engine-abi` への直接依存は削除済み

## クレート構成

```text
crates/
├── rakukan-tsf/                TSF DLL （cdylib）
│     ├ rakukan-engine-rpc だけに依存。engine-abi には依存しない
│     └ DynEngine の名前で RpcEngine を re-export しているので既存コードはそのまま
├── rakukan-engine-abi/         DynEngine: engine DLL の動的ローダー
│     └ 現在の利用者は rakukan-engine-rpc（server 側）と rakukan-engine-cli のみ
├── rakukan-engine-rpc/         Named Pipe + postcard RPC レイヤー（新設）
│     ├ protocol.rs             Request / Response enum
│     ├ codec.rs                [u32 LE len][postcard payload] フレーミング
│     ├ pipe.rs                 PipeStream + OwnedSecurityDescriptor（user-only DACL）
│     ├ server.rs               1 接続 = 1 スレッドで DynEngine へディスパッチ
│     └ client.rs               RpcEngine（DynEngine 互換 API、lazy 接続 + host 自動 spawn）
├── rakukan-engine-host/        rakukan-engine-host.exe（新設）
│     └ DynEngine::load_auto + server::serve をメインに回すだけ
├── rakukan-engine/             エンジン本体
├── rakukan-engine-cli/         動作確認用 CLI
├── rakukan-tray/               トレイ（モード表示）
└── rakukan-dict-builder/
```

## RPC プロトコル要点

- **パイプ名:** `\\.\pipe\rakukan-engine-<USERNAME-sanitized>`
- **フレーミング:** `[u32 LE payload-length][postcard payload]`
- **エンコード:** postcard（forward-compat、小サイズ）
- **ハンドシェイク:** 接続直後に `Hello { protocol_version }` を交換（現在 v1）
- **主なリクエスト:** DynEngine の全メソッドを 1:1 でマップ
  - `Create { config_json }`: 初回のみ DynEngine を生成（idempotent）
  - `Reload { config_json }`: 既存 DynEngine を drop して新 config で再生成（config.toml 編集後の反映に使用）
  - `PushChar / Backspace / BgStart / BgTakeCandidates / Commit / ResetAll / …`
- **エンジン状態共有:** ホスト内で 1 つの `Mutex<DynEngine>` を共有（llama 推論は逐次なので問題なし）

## ホストプロセスのライフサイクル

1. TSF が最初の入力で `engine_try_get_or_create()` を呼び、bg init スレッドが `RpcEngine::connect_or_spawn` を実行
2. パイプへの接続を試し、失敗したら `CreateProcessW`（DETACHED + NO_WINDOW）で `rakukan-engine-host.exe` を起動
3. 最大 5 秒までリトライ接続 → `Hello` → `Create { config_json }`
4. ホストがクラッシュした場合、次の RPC 呼び出しで `call_with_retry` が 1 回再接続し、保存済みの `config_json` で `Create` を再送する
5. 現状ホストは常駐（idle 自死はしていない）

## Named Pipe の DACL

明示的に SDDL `D:P(A;;GA;;;<current-user-sid>)(A;;GA;;;SY)` を設定済み。

- 現在のログインユーザー + SYSTEM のみに GENERIC_ALL
- Protected（親 DACL を継承しない）
- 同一マシンの別ユーザーや別セッションからの接続は拒否される

## config.toml の即時反映

IME モード切替で `reload_if_changed()` が mtime チェックを行い、実際に変更があれば `engine_reload()` を呼ぶ既存パスは生きている。0.4.4 では out-of-process 対応として:

- `engine_reload()` は TSF 側のハンドル (RpcEngine) を捨てず、`Request::Reload { config_json }` をホストに送るだけ
- ホスト側は DynEngine を drop → `DynEngine::load_auto` で新 config 再生成 → 辞書・モデルの bg ロードを再起動
- RPC reload が失敗したときだけハンドルを捨てて、次の呼び出しで再接続 & 再 Create に落とす（ホストがちょうど死んでいた場合の復旧経路）

これにより `n_gpu_layers` や `model_variant` のようなエンジン生成時決定パラメータが、config.toml 編集後の次の IME モード切替で反映される。

## 既存機能（0.4.3 までに完成済み）

### ライブ変換

- ひらがな入力後、短い停止でトップ候補を自動表示
- `Enter` でライブ変換結果をそのまま確定
- `Space` で通常の再変換操作へ移行

### 範囲指定変換（RangeSelect）

- ライブ変換中または Selecting 中に `Shift+Right/Left` で範囲指定モードに入る
- 全文がひらがなに戻り、先頭から `Shift+Right` で変換範囲を指定
- `Space` で選択範囲を LLM 変換して候補表示
- `Enter` で選択範囲を確定、残りの reading で LiveConv を再開
- `ESC` で LiveConv に戻る
- vibrato / SplitPreedit は完全削除済み（分節アライメント問題を根本解決）

### 開発運用

- engine ABI バージョンチェックあり
- 古い engine DLL を読んだ場合、更新漏れがログで分かる

## 主な変更ファイル (0.4.4)

- `crates/rakukan-engine-rpc/`（新設クレート、上記 5 ファイル）
- `crates/rakukan-engine-host/`（新設バイナリ、`src/main.rs`）
- `crates/rakukan-tsf/Cargo.toml`: `rakukan-engine-abi` への依存削除、`rakukan-engine-rpc` 追加
- `crates/rakukan-tsf/src/engine/state.rs`:
  - `DynEngine` を `RpcEngine` の re-export に変更
  - `create_engine()` は `RpcEngine::connect_or_spawn()` を呼ぶのみ
  - `engine_reload()` を Request::Reload 経由に書き換え
- `crates/rakukan-tsf/src/tsf/factory.rs`: `rakukan_engine_abi::` の直接参照を state 経由に置換
- `crates/rakukan-engine-cli/src/main.rs`: `EngineConfig` リテラルを `..Default::default()` で補完
- `rakukan_installer.iss` / `scripts/rakukan_installer.iss` / `scripts/build-installer.ps1` / `scripts/install.ps1`: `rakukan-engine-host.exe` を配置

## 確認コマンド

```powershell
# TSF 層のみビルド
cargo make build-tsf

# engine DLL を含むビルド
cargo make build-engine

# 実機反映（engine-host も含めて全部）
cargo make reinstall

# TSF ログ
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 40

# ホスト側ログ
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan-engine-host.log" -Tail 40

# ホスト強制終了（自動再起動の確認用）
taskkill /f /im rakukan-engine-host.exe

# ホストが動いているか確認
tasklist /FI "IMAGENAME eq rakukan-engine-host.exe"
```

## 実機確認ポイント (v0.4.4)

1. Zoom を起動したまま IME 操作 → **クラッシュしないこと**（確認済み）
2. Dropbox / explorer / VS Code / Chrome でも同様に安定動作
3. `Process Explorer` で `rakukan_engine_*.dll` が **`rakukan-engine-host.exe` にだけ** ロードされていること（TSF アプリのプロセスには居ない）
4. `config.toml` で `n_gpu_layers` を変更 → IME モード切替 → `rakukan-engine-host.log` に `rpc: Reload requested` と新値での再ロードが記録されること
5. `taskkill /f /im rakukan-engine-host.exe` → 次の入力で自動再起動 & 変換継続

## 既知の制約

- ホストは **idle 自死しない**（一度起動すると常駐）。気になれば後日 `--idle-exit-secs` 付きで改善可能
- `rakukan-engine-host.exe` は TSF DLL と同じ install_dir（`%LOCALAPPDATA%\rakukan`）に配置される必要がある
- SDDL は現在ログインユーザー + SYSTEM に限定。同一ユーザーの別プロセス（別アプリの TSF DLL）は接続可（これが IME として期待される動作）

## 残タスク（優先度順）

### 完了済み

- ~~**[Num-1] 数字プレースホルダ**~~ → v0.5.0 で解決（`digits.rs` 数値保護レイヤー）
- ~~**Segment ベースの文節管理**~~ → RangeSelect 方式に転換。vibrato / SplitPreedit を完全削除
- ~~**数字・助数詞の構造対応**~~ → 数値保護で解決。助数詞結合は不要（分節しない方式のため）

### 優先度: 中

- **[Engine-Host-1] idle 自死**
  - 最後のクライアントが切れて N 秒経ったらホスト終了 → 次使用時に自動 spawn
- **[Engine-Host-2] ヘルスチェックとクラッシュカウント**
  - ホストが短時間に連続クラッシュしたら TSF 側で諦めて fallback する
- **[Live-2] display_attr 拡張**
  - RangeSelect の選択範囲表示の改善

### 優先度: 低

- **[Perf-1] RPC レイテンシ計測**
- **用法辞書（Candidate.annotation）** — 候補ウィンドウに同音異義語の説明を表示

## 補足

- TSF だけ変えた場合は `cargo make build-tsf` で OK
- engine / ABI を変えた場合は `cargo make build-engine` と `cargo make reinstall` が必要
- **engine-host を変えた場合は `cargo make reinstall`**（`install.ps1` が rakukan-engine-host も同時ビルドする）
