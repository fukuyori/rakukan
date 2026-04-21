# Rakukan 引き継ぎ資料 (v0.6.3)

更新日: 2026-04-21

## 現在の状態

- **バージョン:** v0.6.3
- **位置づけ:** 変換パイプライン改修完了（数値保護 + 範囲指定変換 + vibrato/SplitPreedit 完全削除）。v0.6.0 で OnSetFocus の安定性修正、v0.6.1 でライブ変換の挙動修正（停止不具合・候補ウィンドウ残留・`num_candidates` 漏洩回帰・ユーザー辞書優先）を反映
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
- `rakukan_installer.iss` / `scripts/build-installer.ps1` / `scripts/install.ps1`: `rakukan-engine-host.exe` を配置

## 確認コマンド

```powershell
# TSF 層 (tsf/tray/host/dict-builder/WinUI) のみビルド
cargo make build-tsf

# engine DLL のみビルド
cargo make build-engine

# ビルド成果物に電子署名 (任意)
cargo make sign

# 実機反映 (コピー + 登録 + tray 起動、★管理者必要)
cargo make install

# 開発時の高速再インストール (build-tsf + install、engine 使いまわし、署名なし)
cargo make quick-install

# リリースフル (build-engine + build-tsf + sign + install を一括)
cargo make full-install

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

## 既知の問題

### Explorer の稀な異常終了（0.6.0 以降頻度低下、残存）

**症状**: Explorer (`explorer.exe`) が `MSCTF.dll` 関連のアクセス違反 (`0xc0000005`) で異常終了することがある。

**現状**:
- 0.6.0 の OnSetFocus 安定性修正（TSF 通知ストーム対策、`prev_dm == next_dm` 早期 return、null DM 処理）で **発生頻度は大幅に低下**
- ただし完全に根絶できておらず、Explorer 使用中にごく稀に再現する

**根本原因の推定**:
- `WM_TIMER` から呼ばれる Phase1A (`RequestEditSession` 直呼び) が、DM が再生成される Explorer のシェル領域で stale な `ITfContext` を掴む競合が残存している可能性

**2026-04 再調査メモ**:
- `OnSetFocus` 本体はすでに `WM_APP_FOCUS_CHANGED` へのキュー積みへ移されており、`msctf!_NotifyCallbacks` 直下で COM 再入しない方針は入っている
- 一方で live conversion の Phase1A は `candidate_window.rs` の `TL_LIVE_CTX` に保持した `ITfContext` を `WM_TIMER` から直接使って `RequestEditSession` を試行している
- `process_focus_change()` では `stop_live_timer()` を呼んでいるが、Explorer 側で DocumentMgr が短時間に再生成されると、フォーカス遷移通知より先に stale な context を掴んだ timer tick が残る可能性がある
- `OnUninitDocumentMgr` は現在 `doc_mode_remove()` のみで、live timer が保持している context / DM の失効判定には使っていない

**優先度付きの対策候補**:

| 対策 | 優先度 | ねらい / メモ |
|------|--------|---------------|
| live context に DM 世代ガードを追加 | 最優先 | `live_input_notify()` で `ctx.GetDocumentMgr()` の ptr も保持し、Phase1A 実行直前に `ITfThreadMgr::GetFocus()` と一致する場合だけ `RequestEditSession` を許可する。`OnUninitDocumentMgr` でも一致 DM の live cache を失効させる |
| `WM_TIMER` 直呼びをやめ、`WM_APP_LIVE_APPLY` 経由で Phase1A を遅延実行 | 高 | `WM_APP_FOCUS_CHANGED` と同じ流儀に寄せ、timer callback 中の COM 呼び出しを減らす。Phase1A の race 窓をさらに狭める |
| Explorer シェルクラスのみ Phase1A を無効化 | 中 | `Shell_TrayWnd` / `Progman` / `WorkerW` 等では Queue fallback に倒す。局所回避としては有効だが、Explorer 専用分岐なので第一選択にはしない |
| `COMPOSITION` のスコープ縮小（`thread_local!` 化） | 中 | 既存保留案。効果はあり得るが変更範囲が大きく、まずは Phase1A 側の stale context ガードを先に入れる |

**推奨方針**:
1. まずは Explorer 専用分岐を増やさず、live timer Phase1A に共通の DM 整合性チェックを入れる
2. それでも再発する場合のみ、`WM_APP_LIVE_APPLY` 化または Explorer シェルクラスでの Phase1A 無効化を追加で検討する
3. live conversion が `enabled = false` の状態でも Explorer crash が出るなら、本件は別経路の可能性が高いため切り分けをやり直す

**検討済み / 見送り対策**:

| 対策 | 状態 | 理由 |
|------|------|------|
| Phase1A 無効化（Phase1B キュー方式に一本化） | 見送り | ライブ変換が機能しなくなる（composition 更新がキー入力まで遅延） |
| Explorer シェルクラスでライブ変換無効化 | 見送り | `Shell_TrayWnd` / `Progman` 等を `GetClassNameW` で判定する方針は妥当だが、処理分岐が微妙で今回は外した |
| `COMPOSITION` のスコープ縮小（`thread_local!` 化） | 保留 | 呼び出し箇所が `factory.rs` 全体に散在、変更量大。次回バージョンで検討 |
| `hwnd_modes` の Explorer 無効化 | 保留 | 上記の COMPOSITION 修正と合わせて次回検討 |

**当面の対応方針**: 現状の頻度は許容範囲として、再発が増悪した場合に見送り対策を再検討する。

## 残タスク（優先度順）

### 完了済み

- ~~**[Num-1] 数字プレースホルダ**~~ → v0.5.0 で解決（`digits.rs` 数値保護レイヤー）
- ~~**Segment ベースの文節管理**~~ → RangeSelect 方式に転換。vibrato / SplitPreedit を完全削除
- ~~**数字・助数詞の構造対応**~~ → 数値保護で解決。助数詞結合は不要（分節しない方式のため）
- ~~**[TSF-1] OnSetFocus 安定性**~~ → v0.6.0 で解決（TSF 通知ストーム対策、null DM 処理改善、フォーカス変化時の候補ウィンドウ閉じを条件付きに）
- ~~**[Live-1] ライブ変換の停止不具合**~~ → v0.6.1 で解決（`on_live_timer` の engine 一時ロック競合を busy 判定せず次回 tick を待つよう修正）
- ~~**[TSF-2] 候補ウィンドウのアプリ切替時残留**~~ → v0.6.1 で解決（`ITfThreadFocusSink` を登録し、Alt+Tab 等の非 TSF アプリへのフォーカス遷移で `hide()` / `stop_live_timer()` / `stop_waiting_timer()` を実行）
- ~~**[Live-3] `num_candidates` 漏洩によるライブ変換遅延**~~ → v0.6.1 で解決（バッチ RPC 経路の prefetch 用 `bg_start(n)` を `live_conv_beam_size` に戻した）
- ~~**[Dict-1] ライブ変換でユーザー辞書が優先されない**~~ → v0.6.1 で解決（`bg_take_candidates` がユーザー辞書候補を LLM 結果の先頭にマージ、読み完全一致のみ）

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

- TSF だけ変えた場合は `cargo make quick-install` で OK (= `build-tsf` + `install`)
- engine / ABI を変えた場合は `cargo make build-engine` → `cargo make quick-install` が必要
- **engine-host を変えた場合も `cargo make quick-install`** (`build-tsf` が rakukan-engine-host も同時ビルドする)
