# Changelog

## [0.3.2] - 2026-03-17

### Fixed

#### CUDA 対応・UIフリーズ修正

- **Activate 時の UIスレッドブロックを解消**（`state.rs` / `factory.rs`）
  - エンジン DLL ロード（CUDA 初期化で最大6秒かかる処理）を `Activate` から切り出し、
    バックグラウンドスレッド（`engine_start_bg_init`）で非同期に実行するよう変更
  - `OnKeyDown` の `Activate` スパンが 7ms 以下に短縮（修正前: 最大 5.7 秒）
  - アプリ切り替え時にメモ帳・エディタ等が「応答なし」になる問題を解消

- **`rakukan_engine_cuda.dll` のロード失敗を修正**
  - `llama-cpp-sys-2` を 0.1.137 → 0.1.138 に更新
    （0.1.137 が要求していた `nvcudart_hybrid64.dll` は CUDA 13.x Toolkit に非同梱）
  - CUDA 13.2 環境でのフルビルドにより `cublas64_13.dll` リンクの DLL を生成
  - 不足 CUDA DLL（`nvcudart_hybrid64.dll` / `cublas64_13.dll` / `cublasLt64_13.dll` /
    `cudart64_13.dll`）を `C:\\Windows\\System32` に手動配置することで解消

### Changed

- **`config.toml` の `gpu_backend` 説明を拡充**（`config/config.toml` / `config.rs`）
  - `cuda` / `vulkan` / `cpu` の各オプションと対応 GPU を明記
  - コメントアウトされた3行を並べて切り替えやすく整理

- **Inno Setup の `config.toml` 配置先を修正**（`rakukan_installer.iss`）
  - `{app}`（`%LOCALAPPDATA%\\rakukan`）から `%APPDATA%\\rakukan`（Roaming）に変更
  - rakukan が実際に読む場所と一致させた
  - `GetRoamingConfigDir()` 関数を追加（UAC 昇格時も正しいユーザーパスを取得）

### Notes

- CUDA 動作には CUDA 13.x Toolkit のインストールと、以下の DLL を
  `C:\\Windows\\System32` へ手動コピーする作業が必要（初回のみ）:
  - `nvcudart_hybrid64.dll`（`cudart64_13.dll` のコピー）
  - `cublas64_13.dll`
  - `cublasLt64_13.dll`
  - `cudart64_13.dll`
- これらのコピーは将来のバージョンでインストーラーに組み込む予定

## [0.3.1] - 2026-03-12

### Fixed

#### LLM変換タイミング問題（Phase 3a フォローアップ）

- **Space 1 回で変換候補が表示されない問題を修正**
  - `WM_TIMER` ベースの LLM 完了ポーリングを `candidate_window.rs` に実装
  - `bg_start` 直後（`pending` 状態）に `wait_done_timeout` が即 `false` を返すレース条件を修正
  - `worker_loop` で `pending → Running` 遷移時に `notify_all` を追加
  - `wait_done_timeout` が `Idle && pending=Some` の場合も Condvar で待機するよう変更

- **新しい文章を変換すると候補が表示されない問題を修正**（`conv_cache.rs`）
  - 前の変換の converter が conv_cache に貸し出されたまま（`kanji_ready=false && bg=running`）の場合、前 bg の完了を待って `bg_reclaim` し新しいキーで `bg_start` するよう変更

- **長い文章で変換が失敗する問題を修正**（`factory.rs`）
  - `LLM_WAIT_MAX_MS` を固定 3 秒から文字数連動（基本 3 秒 + 1 文字 300ms、上限 15 秒）に変更
  - `bg_take_candidates → None` 時に `bg_reclaim → bg_start → bg_wait_ms` でブロッキング再試行

- **文節確定後に remainder が二重表示される問題を修正**（`factory.rs`）
  - `commit_then_start_composition` で `EndComposition` 前に `SetText(commit_text)` を呼び、composition を確定テキストのみに縮めてから終了するよう変更
  - `EndComposition` 後の新 composition 開始点を `get_cursor_range` から `ctx.GetEnd(ec)` に変更

- **変換候補表示時に composition text が 1 番候補に変化しない問題を修正**（`factory.rs`）
  - `bg_take_candidates` のキーを `hiragana_text()` 優先に統一（`preedit_display()` との不一致を解消）

## [0.3.0] - 2026-03-11

### Added
- **Shift+左/右による変換範囲変更**（SplitPreedit）
  - 変換中に Shift+左で変換対象を1文字縮小、Shift+右で1文字拡大
  - target（実線）と remainder（点線）を視覚的に区別して表示
  - Space で target のみを変換、Enter で確定、ESC/Backspace で全体を未変換に戻す
- ビルド時刻を DLL に埋め込み、起動ログに出力（`build=YYYY-MM-DD HH:MM:SS UTC`）
- インストール時にタイムスタンプ付き DLL（`rakukan_tsf_YYYYMMDD_HHmmss.dll`）を自動削除

### Changed
- `rakukan_tsf.dll` を固定名で上書きインストール（タイムスタンプ付きファイルが蓄積しない）
- 診断用ログを `debug!` レベルに降格（通常ログには `info!` 以上のみ出力）
- `esaxx-rs` パッチのセットアップスクリプトがスタブ `lib.rs` を正しく上書きするよう修正

### Fixed
- `esaxx-rs` パッチの `Cargo.toml` に `[lib]` セクションが欠落していたビルドエラーを修正
- `build.rs` の `rerun-if-changed` 設定により `RAKUKAN_BUILD_TIME` が更新されなかった問題を修正
- `update_composition_candidate_split` で `prop.Clear()` を呼んでから属性を再設定するよう修正
- `on_segment_extend` の `target` の move エラーと未使用変数 `full` の警告を修正

## [0.2.0] - 2026-03-06

### Added
- `SessionState` を導入し、TSF 層の論理状態を 1 か所へ寄せる土台を追加
- `Waiting` 状態を追加し、LLM 待機中の状態表現を `SessionState` 側でも保持可能にした

### Changed
- `config.toml` / `keymap.toml` の構造化と再読込を整備
- 候補操作、変換開始、確定、取消などの主要経路を `SessionState` 主体へ段階移行
- 数字キー候補選択などの高速判定を新しい状態層ベースへ変更
- README を v0.2.0 の位置づけに合わせて更新

### Fixed
- `rakukan-tray` の Rust 2024 `unsafe_op_in_unsafe_fn` warning を解消
- Phase 2 移行途中に発生した未使用コード warning を整理
