# Changelog

## [0.4.2] - 2026-03-31

### Changed

- **GPU 使用時の診断ログを追加**
  - `debug` ログ時のみ、低頻度で GPU メモリ使用量を記録するよう改善

### Fixed

- **`F6` 後に `Enter` を押すと再変換される問題を修正**
  - 文字種変換直後はライブ変換 fallback を 1 回抑止するよう変更

- **変換後に `Enter` を押さず次の文を入力したとき、前文が確定されない問題を修正**
  - split / 変換中の内容を確定してから次入力へ進むよう整理

- **`F9` / `F10` で英字化すると末尾子音が欠けることがある問題を修正**
  - pending ローマ字を含めて復元するよう改善

## [0.4.1] - 2026-03-29

### Added

- **`n_gpu_layers` 設定を追加**
  - `%APPDATA%\rakukan\config.toml` から GPU オフロード量を調整可能にした
  - README と設定テンプレートに `model_variant` / `n_gpu_layers` の目安を追記

### Changed

- **分節再変換を辞書寄りに調整**
  - 分節対象では候補数を増やし、辞書候補を先に見やすくした

- **設定テンプレートのモデル ID 表記を修正**
  - `small` / `xsmall` の旧表記を `jinen-v1-small-q5` / `jinen-v1-xsmall-q5` に更新

### Fixed

- **長文変換で後半が欠ける問題を修正**
  - 読み長に応じて LLM の生成予算を伸ばすよう変更

- **分節再変換で `Esc` を押しても読みへ戻らない問題を修正**
  - `かっこ -> （ -> Esc` で `かっこ` に戻るよう修正

- **入力モードに応じたスペース入力へ修正**
  - ひらがな / カタカナ入力中は全角スペース
  - 英数モードでは半角スペース

## [0.4.0] - 2026-03-28

### Added

- **ライブ変換 Phase 1 を追加**
  - ひらがな入力後、短い停止でトップ候補を自動表示
  - `Enter` でプレビュー確定、`Space` で通常の再変換操作へ遷移

- **分節ベースの再変換 UI を追加**
  - `Space` 後に文節単位の選択状態へ入る
  - `Left/Right` で選択文節を移動
  - `Shift+Left/Right` で選択範囲を縮小・拡張

- **Vibrato ベースの分節 API を追加**
  - engine / ABI / TSF を通して `surface` から文節候補を取得可能にした
  - `assets/vibrato/system.dic` を同梱対象に追加

- **engine ABI バージョンチェックを追加**
  - 古い engine DLL を読み込んだとき、更新漏れが分かるようにした

### Changed

- **ライブ変換後の編集フローを整理**
  - `F6` 後の `Enter` で古い変換結果へ戻る問題を修正
  - ライブ変換中の追加入力・`Space`・`Enter`・`ESC`・`Backspace` の状態遷移を整理

- **分節選択中の composition 表示を 3 分割化**
  - `prefix / selected / suffix` を保持し、中間文節だけ再変換できるようにした

- **インジケータ初期表示を実際の入力モードへ同期**
  - IME 起動直後に設定と無関係に `"あ"` 表示になる問題を修正

### Fixed

- **ライブ変換中の追加入力で前半が勝手に確定する問題を修正**
- **分節選択中にライブ変換タイマーが割り込んで状態が崩れる問題を修正**
- **`Right` と `Shift+Right` が同一動作になる問題を修正**
- **文節境界が効かず、再変換対象の読みが崩れるケースを複数修正**

## [0.3.8] - 2026-03-23

### Changed

- **`[candidate]` / `[conversion]` セクションを config.toml から削除**（`config.rs`）
  - 未実装のまま残っていた `page_size` / `use_number_selection` / `show_numbers` / `engine` /
    `commit_raw_with_enter` / `cancel_behavior` を設定ファイルおよび構造体から除去
  - `CandidateConfig` / `ConversionConfig` / `CancelBehavior` 構造体を削除
  - `effective_num_candidates()` を `num_candidates.unwrap_or(9).clamp(1, 9)` に単純化
  - `num_candidates` キー（旧互換）はコメントアウト例として残存

- **`enable_jis_keys` を削除し `layout = "jis"` に統合**（`config.rs`）
  - `KeyboardConfig` から `enable_jis_keys: bool` フィールドを削除
  - JIS キー判定は `layout = "jis"` → `KeyboardLayout::Jis` → `KeymapPreset::MsImeJis` の
    既存パスで完結しており、独立フラグは不要だった

- **キーボードレイアウトのデフォルトを `jis` に変更**（`config.rs`）
  - `default_keyboard_layout()` の戻り値を `KeyboardLayout::Jis` に変更
  - `config.toml` / `default_config_text()` の `layout` も `"jis"` に統一

- **`DefaultInputMode::Katakana` を廃止**（`config.rs`）
  - `DefaultInputMode` を `Hiragana` / `Alphanumeric` の 2 択に縮小
  - カタカナモードへの切り替えは F7 / `ModeKatakana` アクションで引き続き動作

- **`default_mode = "alphanumeric"` を有効化**（`config.rs`, `state.rs`）
  - `doc_mode_on_focus_change()` が初回フォーカス時に `config.input.default_mode` を参照するよう改修
  - ターミナル（Windows Terminal / ConHost 等）は config に関わらず常に `Alphanumeric`

- **`remember_last_kana_mode` を有効化**（`state.rs`）
  - `false` に設定した場合、ウィンドウ切り替え時にモードを保存せず毎回デフォルトを適用
  - `true`（デフォルト）では従来通り DocumentManager ごとに前回モードを復元

- **`default_config_text()` を `config/config.toml` に完全同期**（`config.rs`）
  - 初回起動時に生成されるテンプレートを開発用 `config.toml` と一致させた

### Fixed

- **keymap: `Ctrl+J` / `Ctrl+K` / `Ctrl+L` が parse できない問題を修正**（`keymap.rs`）
  - `name_to_vk()` に単一アルファベット（`a`–`z`）のフォールバックを追加
  - `is_ascii_alphabetic()` を `to_ascii_uppercase()` して VK コード 0x41–0x5A に変換
  - これにより `Ctrl+A` ～ `Ctrl+Z` が keymap.toml で全て記述可能になった

- **keymap: 全角/半角キー（`Zenkaku`）の VK コードが誤っていた問題を修正**（`keymap.rs`）
  - `"zenkaku"` / `"hankaku"` / `"kanji"` のマッピングを `0xF3`（VK_DBE_ROMAN）から
    `0x19`（VK_KANJI）に修正
  - 従来は `factory.rs` のハードコードフォールバック（`0x19 => ImeToggle`）のみで動作していた
  - 修正後はキーマップ経由で正常に処理され、`keymap.toml` でのリマップも有効になる

- **確定時に前の文章が消えるバグを修正**（`factory.rs`）
  - `end_composition` / `commit_then_start_composition` の `composition_take()` をセッション外側からセッション内側へ移動
  - 旧コードでは `COMPOSITION=None` になった直後に次キー入力が来ると `update_composition` が
    `existing=None` を見て誤った位置から新 composition を開始し、`SetText` が既存テキストを上書きしていた
  - `get_cursor_range` の `Collapse` 失敗もログ付きで処理するよう変更

- **`remember_last_kana_mode` が機能しない根本バグを修正**（`factory.rs`）
  - `OnSetFocus` / `OnUninitDocumentMgr` / `Activate` で DocumentManager のポインタ取得が誤っていた
  - `d as *const _ as usize`（ローカル参照のスタックアドレス）→
    `*(d as *const ITfDocumentMgr as *const usize)`（COM オブジェクトの内側ポインタ値）に修正
  - 旧コードでは呼び出しごとに異なるキーが生成され `DOC_MODE_STORE` のルックアップが常にミスしていた

- **`default_mode = "alphanumeric"` が反映されない問題を修正**（`factory.rs`）
  - `Activate` 末尾で `tm.GetFocus()` で現在フォーカス中の DM を取得し
    `doc_mode_on_focus_change` で初期モードを即時適用するよう変更
  - `ITfThreadMgrEventSink` 登録前にフォーカス済みの DM には `OnSetFocus` が呼ばれないため
