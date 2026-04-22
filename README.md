# rakukan v0.6.7

> ⚠️ **注意：現在テスト動作中です**
>
> rakukan は開発途中のソフトウェアです。インストールによって **Windows の動作が不安定になる可能性があります**。
> TSF（Text Services Framework）DLL をシステムに登録するため、インストール・アンインストールの操作は
> **自己責任** で行ってください。重要な作業環境への適用は推奨しません。

Windows 向け日本語 IME。  
[karukan](https://github.com/togatoga/karukan) の LLM ベース変換エンジンを中核とし、
[azooKey-Windows](https://github.com/fkunn1326/azooKey-Windows) の TSF 層実装を参考に構築しています。

## 主な機能

- **ライブ変換**: ひらがな入力後、短い停止でトップ候補を自動表示
- **範囲指定変換**: `Shift+Right/Left` で先頭から変換範囲を指定 → `Space` で変換 → `Enter` で確定、残りで LiveConv 再開
- **数値保護**: LLM が数字を改変しない（`2024ねん → 2024年`）。数字・アルファベットは半角/全角の両方を候補として提示
- **LLM + 辞書変換**: jinen モデルと Mozc 系辞書を併用
- **ユーザー辞書学習**: 確定した変換結果を即時反映
- **文字種変換**: `F6`〜`F10` でひらがな・カタカナ・英数を往復
- **GPU アクセラレーション**: CUDA / Vulkan バックエンド対応

## 0.6.7 変更点

- **辞書候補の増強**: Space 変換の `DICT_LIMIT` を 20 → 40、`merge_candidates` の辞書スロット配分を `limit/2` → `limit*2/3` に変更。LLM beam を広げずに辞書由来候補を最大 26 件程度まで提示
- **設定画面を開閉しただけで変換が止まる問題を解消**: WinUI 設定が on-disk TOML との diff を検出した時のみ engine reload を発火するよう変更（無変更クローズでは reload しない）
- **変換中のキー入力取りこぼしを軽減**: `on_convert` の inline LLM 待機を 3〜15 秒 → 250ms に短縮。超過時は既存の WM_TIMER ポーリングにフォールバックして ⏳ 表示のまま継続。hot path のロック占有時間を 1 桁以上縮める
- **範囲指定変換の二重ブロック解消**: 旧実装の `convert_sync` + `bg_wait_ms(1500)` を `bg_start` + 250ms inline + WM_TIMER fallback に統一。`SessionState::Waiting` に `remainder` フィールドを追加し、非同期で Selecting 昇格する際も残り読みを保持
- **変体仮名 (Hentaigana) を辞書ビルド時に除外**: Windows 標準フォントで描画できない Kana Extended-B / Supplement / Extended-A / Small Kana Extension (U+1AFF0–U+1B16F) を含む surface を dict-builder が恒久排除。絵文字・CJK 拡張漢字・⏩ 等の BMP 記号は誤爆せず残る
- **絵文字 (mozc emoji_data.tsv) に対応**: dict-builder に `--emoji <path>` 引数と `parse_emoji_tsv` を追加。install.ps1 が `emoji_data.tsv` を GitHub からダウンロードして辞書に統合する。「はーと」→ ♥️、「はやおくり」→ ⏩ 等の hiragana 読みで引ける（候補ウィンドウ内は GDI の制約で白黒表示、確定先アプリではカラーで入力される）

## 0.6.6 変更点

- **Explorer 異常終了の真因対策**: `DllCanUnloadNow` を常に `S_FALSE` 固定。2026-04-22 のクラッシュダンプ解析で、unload 済み TSF DLL の wnd_proc アドレスへ in-flight メッセージがディスパッチされる race（`BAD_INSTRUCTION_PTR_c0000005_rakukan_tsf.dll!Unloaded`）が真因と判明。プロセス常駐させて完全回避（メモリコスト ~2 MB/process）

## 0.6.5 変更点

- **学習機能を `learn_history.bin` に分離** — 確定時の学習を独立ファイル (`%APPDATA%\rakukan\learn_history.bin`) に記録し、MOZC の `UserHistoryPredictor` 準拠のスコア式 (`last_access_time + 86400 * freq * 0.5^(Δdays/30) - chars_count`) で優先順位を制御。半減期 30 日の頻度減衰、LRU 30,000 件上限、学習対象は MOZC 辞書・ユーザー辞書由来の surface のみ (LLM 生成は対象外)
- **`[input] auto_learn` デフォルト true + WinUI 設定にトグル追加** — 既定で学習が有効に。`user_dict.toml` は手動登録専用に戻る
- **WinUI モデル ID 保存バグ修正** — 設定画面で ModelVariant が再起動時に `jinen-v1-xsmall-q5` にリセットされていた問題を修正

## 0.6.4 変更点

- **Explorer 異常終了の hardening**: `OnUninitDocumentMgr` で破棄される DM に紐づく `COMPOSITION` も stale 扱いし、Phase1A callback では `current_focus_dm_ptr()` を実行直前に再検証する。あわせて `EditSession` 経路の `unwrap()` を `get_insert_range_or_end` / `suffix_after_prefix_or_empty` 等で `Result` 化し、panic=abort 下でのプロセス停止経路を縮小

## 0.6.3 変更点

- **ローマ字入力時の文字消失バグを修正**: `pending_romaji_buf` と `RomajiConverter` 内部 buffer の同期ズレが原因で、`PassThrough` 連鎖時に未確定ローマ字がプリエディット表示から落ちていた問題を修正（例: `qwrty` → 表示 `qwry`、`かなkq` → 表示 `かなk`）。同根原因として F9/F10 のローマ字復元ログも整合を取り戻す

## 0.6.2 変更点

- **`gpu_backend = "auto"` サポート**: インストール済みの `rakukan_engine_*.dll` を `cuda` → `vulkan` → `cpu` の順で自動選択。デフォルトで有効
- **モデル variant `f16` 追加**: `jinen-v1-xsmall-f16` / `jinen-v1-small-f16`（量子化なし FP16、高精度・大容量）を追加
- **設定デフォルト値を整理**: `log_level = "info"`、`n_gpu_layers = 16`、`main_gpu = 0`、`model_variant = "jinen-v1-xsmall-q5"` を有効化。`dump_active_config = false` に変更
- **WinUI 設定のモデル選択 UI 改善**: ドロップダウンに各 variant のファイルサイズを併記（例: `jinen-v1-xsmall-q5 (約 30 MB)`）
- **モデル登録ツール `scripts/refresh-models.ps1`**: HuggingFace 公開中の `.gguf` を走査して `models.toml` 未登録分を検出、`-Apply` で自動追記

## 0.6.1 変更点

- **ライブ変換の停止不具合を修正**: `on_live_timer` が engine ロック競合で `stop_live_timer` を呼び、入力の流れによってライブ変換が途中で止まることがあった問題を修正
- **候補ウィンドウのアプリ切替時残留を修正**: `ITfThreadFocusSink` を登録し、Alt+Tab 等の非 TSF アプリへのフォーカス遷移でも候補ウィンドウと各種タイマーを確実に閉じるよう改善
- **`num_candidates` がライブ変換を遅延させる回帰を修正**: バッチ RPC 経路の `input_char` が prefetch 用 `bg_start(n)` に `num_candidates`（最大 30）を渡していたのを `live_conv_beam_size` に戻した。Space 変換時は従来どおり `num_candidates` を使用
- **ライブ変換 preview でユーザー辞書を優先**: `bg_take_candidates` がユーザー辞書候補を LLM 結果の先頭にマージするよう変更（読み完全一致のみ）

## 0.5.0 変更点

- **数値保護レイヤー**: LLM が数字を改変する問題を根本解決。reading を数字ラン / 非数字ランに分割し、LLM には非数字部分だけを渡す
- **アルファベット保護**: 半角/全角の両方を候補として提示
- **範囲指定変換 (RangeSelect)**: `Shift+矢印` で先頭から変換範囲を指定する新しい部分変換方式。分節アライメント問題が発生しない
- **数字入力の半角/全角設定**: `config.toml` の `[input] digit_width = "halfwidth"` で制御（デフォルト: 半角）
- **vibrato / SplitPreedit の完全削除**: 形態素解析ベースの分節分割を廃止し、reading 文字位置ベースの範囲指定に置換

> 0.5.0 では engine ABI が v7 に更新されています。`cargo make full-install` で全コンポーネントを更新してください。

## 0.4.4 変更点

- **エンジンを別プロセス化**: `rakukan_engine_*.dll`（llama.cpp 同梱）は専用の
  `rakukan-engine-host.exe` でのみロードし、TSF DLL 側は Named Pipe + postcard で
  RPC するクライアントになりました。これにより Zoom / Dropbox などで発生していた
  `msvcp140.dll` クロスロード起因のクラッシュを根本解決しています。
- Activate 時には engine DLL に一切触れず、最初の入力まで完全に遅延ロード
- IME モード切替時の `config.toml` 自動再反映を out-of-process でも動くように修正
  （`Request::Reload` でホスト側 DynEngine を新 config で作り直す）
- Named Pipe には明示的な DACL を設定（現在ログインユーザー + SYSTEM のみ）
- `rakukan-engine-cli` の既存ビルドエラーを修正

## インストール

ビルド → 署名 → インストールを **4 ステップ** に分離しています:

```powershell
# 初回: esaxx-rs パッチのセットアップ
cargo fetch
.\scripts\setup-esaxx-patch.ps1

# ① engine DLL をビルド (cpu/vulkan/cuda)
cargo make build-engine

# ② tsf + tray + host + dict-builder + WinUI settings をビルド
cargo make build-tsf

# ③ 電子署名 (任意; 配布用)
cargo make sign

# ④ %LOCALAPPDATA%\rakukan\ にコピー + TSF 登録 + tray 起動 (★管理者権限)
cargo make install
```

まとめ実行:

```powershell
# ①〜④ を一括 (リリース向け)
cargo make full-install

# 開発時の高速再インストール (engine 使いまわし、署名なし)
cargo make quick-install
```

インストール先: `%LOCALAPPDATA%\rakukan\`  
設定: `%APPDATA%\rakukan\config.toml`  
ログ:

- TSF 側: `%LOCALAPPDATA%\rakukan\rakukan.log`
- エンジンホスト側: `%LOCALAPPDATA%\rakukan\rakukan-engine-host.log`

> 各ステップはそれぞれ独立に実行できます。ビルド (`build-engine` / `build-tsf`) は管理者不要、`install` のみ管理者権限が必要です。

## 設定の目安

`%APPDATA%\rakukan\config.toml` では `model_variant` と `n_gpu_layers` を調整できます。

- `jinen-v1-xsmall-q5` は比較的軽く、`n_gpu_layers = 16` 前後から試しやすい
- `jinen-v1-small-q5` は `n_gpu_layers = 8` か `16` くらいから始めるのが安全
- `n_gpu_layers = 0` は CPU のみ
- 未指定は全レイヤー GPU オフロード

`n_gpu_layers` と `model_variant` は config.toml を編集したあと IME モードを切り替えるだけで即時反映されます（`rakukan-engine-host.exe` 内部の DynEngine が新設定で作り直されます）。

> v0.4.4 より、Zoom / Dropbox 等の他アプリが異常終了する問題は別プロセス化で解消済みです。`n_gpu_layers` を下げる回避策は不要になりました。

## キー操作

| キー | 動作 |
| ---- | ---- |
| Space / 変換 | 変換開始 / 次候補 / 選択中分節の再変換 |
| Enter | 表示中の内容を確定 |
| ESC | 変換キャンセル |
| Backspace | 1文字削除 |
| Left / Right | 分節選択の移動 |
| Shift+Left / Shift+Right | 分節選択の縮小 / 拡張 |
| ↑ / ↓ | 候補を前後に移動 |
| 1〜9 | 候補を番号で選択 |
| Tab / PageDown | 次ページ |
| Shift+Tab / PageUp | 前ページ |
| F6 | ひらがな |
| F7 | カタカナ |
| F8 | 半角カタカナ |
| F9 | 全角英数 |
| F10 | 半角英数 |

## 開発メモ

- TSF 層だけの変更確認: `cargo make quick-install` (= `build-tsf` + `install`)
- engine DLL を含む変更確認: `cargo make build-engine` → `cargo make quick-install`
- 同梱 Vibrato 辞書: `assets/vibrato/system.dic`
- 生成ログ確認:

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 40
```

## 課題リスト

現在進行中の設計書・残タスクは以下の資料にまとめています。

### 主要設計書

- [DESIGN.md](docs/DESIGN.md) — v0.4.4 時点の全体設計書（クレート構成・RPC プロトコル・スレッドモデル・辞書システムなど）
- [CONVERTER_REDESIGN.md](docs/CONVERTER_REDESIGN.md) — **進行中**: ライブ変換・文節再変換・境界伸縮・数値保護・用法辞書の全面改修設計（Phase A〜F）
- [SEGMENT_EDIT_REDESIGN.md](docs/SEGMENT_EDIT_REDESIGN.md) — 分節編集モデルの基礎設計（`CONVERTER_REDESIGN.md` に継承済み）
- [VIBRATO_PHASE1.md](docs/VIBRATO_PHASE1.md) — Vibrato 形態素解析器の導入メモ
- [handoff.md](docs/handoff.md) — v0.4.4 引き継ぎ資料 + 残タスクリスト

### 進行中の主要課題

**変換パイプライン再設計**（[CONVERTER_REDESIGN.md](docs/CONVERTER_REDESIGN.md)）

- [ ] **Phase A**: 新データモデル（`Segments` / `Segment` / `Candidate`）と engine 基盤、数値保護レイヤー
- [ ] **Phase B**: ライブ変換を beam=1 (greedy) 化、`Segments` 保持
- [ ] **Phase C**: `SplitPreedit` を新モデルに置換、文節ごとの候補管理
- [ ] **Phase D**: 境界伸縮を engine の `resize_segment` に集約、文字列再分節の撤廃
- [ ] **Phase E**: 部分確定・学習・Selecting 統合・候補一覧 Tab 展開
- [ ] **Phase F**（独立）: Candidate 注釈（用法辞書） — Mozc `usage_dict.tsv` の取り込み

**独立した技術課題**（[handoff.md §残タスク](docs/handoff.md#残タスク優先度順)）

- [ ] `rakukan-engine-host.exe` の idle 自死（長時間アイドル時のメモリ解放）
- [ ] ホストプロセスのヘルスチェックとクラッシュカウント
- [ ] Preedit / LiveConv / Selecting の display_attr 拡張
- [ ] RPC レイテンシの実測（0.4.5 バッチ化後の計測）

### 過去のスナップショット

v0.2.0 の状態を記録した以下の資料は **過去のスナップショット** であり、現在進行中のタスクではありません。

- [PHASE1_SUMMARY.md](docs/archive/PHASE1_SUMMARY.md) — v0.2.0 時点の Phase 1 要約
- [PHASE2_PREP.md](docs/archive/PHASE2_PREP.md) — v0.2.0 先行の Phase 2 着手前メモ
- [PHASE2_STATUS.md](docs/archive/PHASE2_STATUS.md) — v0.2.0 時点の Phase 2 状況
- [WARNING_FIXES.md](docs/archive/WARNING_FIXES.md) — v0.2.0 に含まれる warning 修正メモ

## ライセンス

rakukan 本体のコードは **MIT ライセンス** です。  
辞書・モデルなどの同梱物や取得物には、それぞれ個別のライセンス条件が適用されます。
