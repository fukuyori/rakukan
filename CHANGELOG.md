# Changelog

## [0.3.4] - 2026-03-18

### Fixed

- **変換後に次の入力をするとカーソルが飛ぶ問題を修正**（`factory.rs`）
  - 候補選択中に次の文字を打つと新しい composition が文末または文頭に開始されていた
  - `EndComposition` 前に確定テキストの range 末尾位置を保存し、その位置から新 composition を開始するよう修正
  - 従来は `ctx.GetEnd(ec)`（ドキュメント末尾）を使用していたため文章途中の編集でカーソルが末尾に飛んでいた

- **候補品質の改善**（`backend.rs`）
  - `generate_beam_search_d1_greedy`（深さ1ビームサーチ）に切り替えていたが、読みと無関係な候補が生成される問題があった
  - 真のビームサーチ（`generate_beam_search`）に戻し、`beam_size` を最大3にキャップして速度と品質のバランスを改善

- **辞書が参照できない問題を修正**（`rakukan-dict/src/lib.rs`）
  - 辞書配置先を `%LOCALAPPDATA%\rakukan\dict\` に明確化（フォールバックを削除）
  - `rakukan_installer.iss` / `install.ps1` / `download-skk.ps1` / `build-installer.ps1` の辞書パスを統一

### Changed

- **`-`（ハイフン）のコンテキスト判定を拡張**（`lib.rs`）
  - `,` → `、`（ひらがな直後）/ `，`（全角文字直後）/ `,`（半角直後）
  - `.` → `。`（ひらがな直後）/ `．`（全角文字直後）/ `.`（半角直後・小数点）
  - `=`, `@`, `(`, `¥` 等の ASCII 記号も全角コンテキストで全角記号に変換

- **`UserAction::InputRaw` を追加**（`user_action.rs`, `keymap.rs`, `factory.rs`, `lib.rs`, `ffi.rs`, `abi/lib.rs`）
  - テンキー記号（`/ * - + .`）がかなルール経由で誤変換される問題を根本解決
  - ローマ字変換を完全バイパスして `hiragana_buf` に直接書き込む経路を新設

- **F8（半角カタカナ）変換を完全実装**（`text_util.rs`）
  - ひらがな・全角カタカナ → 半角カタカナ（濁音・半濁音は結合文字2文字に展開）
  - 全角英数記号も半角に変換

- **F6/F7 に全角変換・半角カタカナ対応を追加**（`text_util.rs`）
  - 半角英数記号 → 全角英数記号
  - 半角カタカナ → 全角カタカナ/ひらがな（F8後にF7/F6でサイクル可能）

- **コンテキストトリミング改善**（`lib.rs`）
  - `commit()` の200文字トリミングを文境界（`。！？`等）で行うよう改善

- **バックスペース素通り修正**（`lib.rs`）
  - `preedit_is_empty()` が `pending_romaji_buf` を無視していた問題を修正

- **`build-installer.ps1` のバージョンを 0.3.4 に更新**

### Notes

- 辞書ファイルの配置先は `%LOCALAPPDATA%\rakukan\dict\` に統一
- `config.toml` / `keymap.toml` / `user_dict.toml` は `%APPDATA%\rakukan\` に配置


## [0.3.3] - 2026-03-18

### Fixed

- **バックスペースが子音単体入力中に素通りする問題を修正**（`lib.rs`）
  - `preedit_is_empty()` が `hiragana_buf` しか見ておらず `pending_romaji_buf` を無視していた
  - `k`, `s`, `t` 等の子音だけ打った状態でバックスペースが効かなかった

- **複数候補生成時のクラッシュを修正**（`llamacpp.rs`）
  - `generate_beam_search_d1_greedy_batch` で `n_batch` が `n_ctx`（128）を大幅に超え
    llama.cpp 内部の `GGML_ASSERT` が `abort()` を呼びプロセスが異常終了していた
  - `n_ctx` を `max(128, batch_size)` に動的拡張し `beam_size` を最大5にキャップして解消

- **テンキー記号（`/ * - + .`）がかな変換される問題を修正**（`keymap.rs`, `factory.rs`, `lib.rs`, `ffi.rs`, `abi/lib.rs`）
  - `ToUnicode` 経由でかなルール（`/`→`・`, `-`→`ー`, `.`→`。`）が適用されていた
  - `UserAction::InputRaw` と `push_raw()` を新設してローマ字変換を完全バイパス

### Changed

- **変換速度の改善**（`backend.rs`）
  - 複数候補生成を `generate_beam_search`（毎回 `new_context()` を生成）から
    `generate_beam_search_d1_greedy_batch`（KVキャッシュ共有）に切り替え

- **F8（半角カタカナ）変換を完全実装**（`text_util.rs`）
  - 従来はスタブで全角カタカナをそのまま返していた
  - ひらがな・全角カタカナ → 半角カタカナ（濁音・半濁音は結合文字2文字に展開）
  - 全角英数記号（`ＭＳーＩＭＥ` の英字部分等）も半角に変換

- **F6/F7 に全角変換を追加**（`text_util.rs`）
  - 半角英数記号 → 全角英数記号（`abc` → `ａｂｃ`）
  - 半角カタカナ → 全角カタカナ/ひらがな（F8後にF7/F6でサイクル可能）

- **`-`（ハイフン）のコンテキスト判定を実装**（`lib.rs`）
  - 直前の文字種に応じて自動的に適切な文字を選択
  - ひらがな・全角カタカナ直後 → `ー`（全角長音符）
  - 半角カタカナ直後 → `ｰ`（半角長音符）
  - 全角英数・全角記号直後 → `－`（全角ハイフン）
  - 半角英数・空・未確定ローマ字あり → `-`（半角ハイフン）

- **ASCII 記号のコンテキスト判定を実装**（`lib.rs`）
  - `=`, `@`, `(`, `¥` 等の記号を直前の文字種に応じて全角・半角に自動変換
  - 全角コンテキスト（ひらがな・全角文字の直後）→ 全角記号（`＝`, `＠`, `（`, `￥`）
  - 半角コンテキスト → ローマ字ルールに委ねる（従来通り）

- **コンテキストトリミング改善**（`lib.rs`）
  - `commit()` の200文字トリミングを文境界（`。！？`等）で行うよう改善
  - LLMに渡すコンテキストが文の途中で切れなくなった

### Notes

- `backend.rs` のテストで `std::env::set_var` / `remove_var` を `unsafe` ブロックで囲むよう修正
  （Rust 1.85 / edition 2024 の変更に対応）


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
