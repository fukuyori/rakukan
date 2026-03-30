# Rakukan 詳細設計書

バージョン: v0.4.2  
最終更新: 2026-03-30

---

## 1. プロジェクト概要

Rakukan は Windows 向け日本語 IME（Input Method Editor）である。  
Rust で実装され、Windows Text Services Framework（TSF）を通じてシステムに統合される。
変換エンジンに llama.cpp ベースの小型 LLM（jinen モデル）と mozc 辞書を組み合わせ、
高品質な漢字変換を実現することを目標とする。

### 主要特徴

- TSF（Text Services Framework）による Windows 標準 IME 統合
- LLM（llama.cpp）によるかな漢字変換
- mozc / SKK 辞書との組み合わせによる候補マージ
- GPU（CUDA / Vulkan）アクセラレーション対応
- バックグラウンド非同期変換（UI スレッドをブロックしない）
- ライブ変換と分節再変換 UI を含む日本語入力フロー

---

## 2. クレート構成

```
rakukan/
├── crates/
│   ├── rakukan-tsf/          TSF レイヤー（Windows IME 本体、DLL）
│   ├── rakukan-engine/       変換エンジン（LLM + 辞書、DLL）
│   ├── rakukan-engine-abi/   エンジン DLL の動的ローダー（ABI ブリッジ）
│   ├── rakukan-dict/         辞書ライブラリ（mozc / SKK / ユーザー辞書）
│   ├── rakukan-dict-builder/ 辞書ビルドツール
│   ├── rakukan-engine-cli/   エンジン単体テスト用 CLI
│   └── rakukan-tray/         システムトレイ常駐プロセス
├── config/
│   ├── config.toml           設定ファイルテンプレート
│   └── keymap.toml           キーバインドテンプレート
└── scripts/                  ビルド・インストールスクリプト
```

### DLL 構成とロード関係

```
rakukan_tsf.dll
  └── rakukan-engine-abi（静的リンク）
        └── rakukan_engine_{cuda|vulkan|cpu}.dll（実行時動的ロード）
              └── llama.cpp（静的リンク）
              └── rakukan-dict（静的リンク）
```

rakukan-tsf と rakukan-engine は **別 DLL** として分離されている。  
理由: llama.cpp のビルドが重く（数分〜十数分）、TSF 層の変更ごとに再ビルドしたくないため。  
ABI 境界は C の FFI（`extern "C"` 関数ポインタのテーブル）で構成される。

---

## 3. アーキテクチャ概要

### 処理フロー全体図

```
ユーザーキー入力
    │
    ▼
[Windows TSF]
  OnTestKeyDown / OnKeyDown
    │
    ▼
[rakukan-tsf: factory.rs]
  keymap.resolve_action(vk)
    │
    ▼
  UserAction（Input / Convert / CommitRaw / ...）
    │
  ┌─────────────────────────────────────────────────────┐
  │ IME オン（Hiragana / Katakana モード）               │
  │                                                     │
  │  Input(c) ──▶ engine.push_char(c)                  │
  │                 │                                   │
  │                 ▼                                   │
  │         RomajiConverter（trie ルール）               │
  │           Buffered / Converted / PassThrough        │
  │                 │                                   │
  │                 ▼                                   │
  │         hiragana_buf に蓄積                         │
  │                 │                                   │
  │  Convert ──▶ bg_start() ──▶ 変換ワーカースレッド    │
  │                 │             LLM + 辞書             │
  │                 │             候補マージ             │
  │                 │                                   │
  │            ポーリング                               │
  │         bg_take_candidates()                        │
  │                 │                                   │
  │                 ▼                                   │
  │         候補ウィンドウ表示                           │
  │                 │                                   │
  │  Enter / 数字 ──▶ engine.commit() ──▶ TSF 確定      │
  └─────────────────────────────────────────────────────┘
```

---

## 4. rakukan-tsf 詳細

### 4.1 モジュール構成

| ファイル | 役割 |
|----------|------|
| `tsf/factory.rs` | TSF COM オブジェクト実装の中心。OnKeyDown・EditSession・確定処理 |
| `tsf/candidate_window.rs` | 候補ウィンドウ（Win32 独自ウィンドウ）の表示・更新・タイマー |
| `tsf/edit_session.rs` | EditSession ラッパー（`ITfEditSession` 実装） |
| `tsf/language_bar.rs` | 言語バー（タスクバーの IME アイコン）管理 |
| `tsf/display_attr.rs` | アンダーライン属性（プリエディット・変換中表示）管理 |
| `tsf/registration.rs` | TIP 登録・登録解除 |
| `tsf/tray_ipc.rs` | トレイプロセスへの IPC（共有メモリ） |
| `engine/state.rs` | グローバル IME 状態（エンジン・セッション・モード等） |
| `engine/config.rs` | 設定ファイル管理（`AppConfig`・`ConfigManager`） |
| `engine/keymap.rs` | キーバインド管理（`Keymap`・`KeySpec`・プリセット） |
| `engine/input_mode.rs` | 入力モード列挙型（`InputMode`） |
| `engine/user_action.rs` | ユーザーアクション列挙型（`UserAction`） |
| `engine/text_util.rs` | 文字種変換（F6〜F10 用） |
| `diagnostics.rs` | ログイベント集約（`DiagEvent`） |

### 4.2 グローバル状態（state.rs）

TSF 層の状態は複数のグローバル静的変数で管理される。  
ホットパス（OnTestKeyDown / OnKeyDown）では**ブロックを避ける**ことが最優先制約であり、  
Mutex の取得はすべて `try_lock()` を使用する。

| 変数 | 型 | 用途 |
|------|----|------|
| `INPUT_MODE_ATOMIC` | `AtomicU8` | 現在の入力モード（ロックなし高速読み取り用） |
| `IME_STATE` | `Mutex<IMEState>` | 入力モードの正式な保持場所 |
| `RAKUKAN_ENGINE` | `Mutex<EngineWrapper>` | エンジン DLL インスタンス |
| `ENGINE_INIT_STARTED` | `AtomicBool` | BG 初期化の二重スポーン防止フラグ |
| `SESSION_STATE` | `Mutex<SessionState>` | TSF 層の論理状態（Idle / Preedit / Waiting / ...） |
| `SESSION_SELECTING` | `AtomicBool` | 候補選択中フラグ（ホットパス用） |
| `COMPOSITION` | `Mutex<CompositionWrapper>` | 現在の `ITfComposition` オブジェクト |
| `CARET_RECT` | `Mutex<CaretRect>` | キャレット矩形（候補ウィンドウ位置計算用） |
| `LANGBAR_UPDATE_PENDING` | `AtomicBool` | BG 初期化完了後の言語バー更新フラグ |
| `DOC_MODE_STORE` | `Mutex<HashMap<usize, InputMode>>` | DocumentManager ごとの入力モード記憶 |
| `CONFIG_MANAGER` | `Mutex<ConfigManager>` | 設定ファイルの読み込み・キャッシュ |

#### IME 状態（InputMode）

```rust
pub enum InputMode {
    Hiragana,      // 0 (AtomicU8)
    Katakana,      // 1
    Alphanumeric,  // 2
}
```

`Alphanumeric` は実質的な「IME オフ」状態。  
キーをそのまま素通りさせ（`OnTestKeyDown` が `FALSE` を返す）、アプリが直接処理する。

### 4.3 セッション状態（SessionState）

TSF 層の論理状態機械。`SESSION_STATE` Mutex で保護される。

```
Idle
  │ キー入力（Input）
  ▼
Preedit { text }          ─ ひらがな入力中
  │ Space（Convert）
  ▼
Waiting { text, pos }     ─ LLM 変換中（⏳ 表示）
  │ bg_take_candidates 成功
  ▼
Selecting {               ─ 候補選択中
  candidates, selected,
  llm_pending,
  remainder, ...
}
  │ Shift+Left/Right
  ▼
SplitPreedit {            ─ 文節分割調整中
  target,                   target: 変換対象（実線アンダーライン）
  remainder                 remainder: 残り（点線アンダーライン）
}
  │ Enter / 数字キー / Escape
  ▼
Idle
```

`SESSION_SELECTING` AtomicBool は `Selecting` と `SplitPreedit` 時に `true` になり、  
`OnTestKeyDown` の判定（キーを IME が消費するか）に使用される。  
これにより `SESSION_STATE` の Mutex を取らずにホットパスの判定が可能。  

### 4.3.1 部分再変換の実装原則

部分再変換・分節編集では、文章ごとの例外処理やリテラル文字列に依存した実装を入れない。  
目標は「特定の語をうまく切ること」ではなく、「どの文章でも同じ操作感で伸長・縮小・再変換できること」である。

- 分節境界の検出は `Vibrato` と engine 側の一般ロジックに委ねる
- TSF 側で特定の助詞・単語・文字列をハードコードして切り分けない
- `Right/Left`、`Shift+Right/Left`、`Space`、`Enter` の意味は文章に依らず固定する
- 既存の分節を修正する処理では、未変更の左側を壊さないことを最優先にする
- `surface + reading` 文字列から毎回再解釈するのではなく、文節列を正として扱う方向へ寄せる

禁止事項:

- 「`わさび` を含む時はこう切る」のような語彙依存の分岐
- 「`と` `は` `が` ならこう扱う」のような文字依存の runtime ルール
- 一時的なデバッグやテスト用のつもりで、特定文章向けの分節補正を本実装に残すこと

### 4.4 キー処理フロー（OnKeyDown）

```
OnKeyDown(wparam=VK)
  │
  ├─ Alphanumeric モード → キーを素通り（FALSE）
  │
  ├─ keymap.resolve_action(vk) → UserAction
  │     ├─ keymap.toml + プリセット（MsImeUs / MsImeJis）
  │     └─ ToUnicode でフォールバック（印字可能文字）
  │
  └─ handle_user_action(action)
       ├─ Input(c)         → push_char / push_raw / push_fullwidth_alpha
       ├─ Convert          → bg_start → Waiting / Selecting
       ├─ CommitRaw        → end_composition / commit_then_start
       ├─ Backspace        → engine.backspace → update_composition
       ├─ Cancel           → reset_preedit → end_composition
       ├─ CandidateNext/Prev → SessionState を更新 → candidate_window 更新
       ├─ F6〜F10          → text_util::to_xxx → force_preedit
       ├─ ImeToggle        → InputMode 切り替え → KEYBOARD_OPENCLOSE 更新
       └─ ...
```

### 4.5 確定処理（EditSession）

TSF の `RequestEditSession` 経由でテキストの読み書きを行う。  
重要な制約: **`WndProc` から `RequestEditSession` を呼べない**（デッドロックの危険）。  
そのため WM_TIMER コールバックからの直接呼び出しは禁止。

#### `end_composition(text)`

```
EditSession 内:
  composition_take()          ← セッション内で取得（外で取ると race condition）
  comp.GetRange()
  range.SetText(text)
  range.Collapse(TF_ANCHOR_END)
  ctx.SetSelection(...)       ← EndComposition 前にカーソルを末尾へ
  comp.EndComposition()
```

#### `commit_then_start_composition(commit_text, next_preedit)`

文節分割（SplitPreedit）後の確定 + 残り部分の継続入力に使用。  
1 EditSession 内で「確定 → 新 composition 開始」を原子的に行う。

```
EditSession 内:
  composition_take()
  Step1: 既存 composition を commit_text に縮めて EndComposition
         ※ EndComposition 前に末尾 range を保存
  Step2: 保存した末尾位置から StartComposition
         new_range.SetText(next_preedit)
         display_attr_prop 設定（アンダーライン）
```

### 4.6 フォーカス変化とモード復元

`ITfThreadMgrEventSink::OnSetFocus` で DocumentManager（DM）フォーカス変化を受信。

```
OnSetFocus(prev_dm, next_dm)
  │
  ├─ DMポインタ取得:
  │   *(d as *const ITfDocumentMgr as *const usize)  ← COM内側ポインタ値
  │   ※ d as *const _ as usize はスタックアドレスになるため NG
  │
  ├─ doc_mode_on_focus_change(prev_ptr, next_ptr, hwnd)
  │    ├─ remember=true かつ prev_ptr!=0: store.insert(prev_ptr, 現在モード) で保存
  │    └─ next_ptr の復元:
  │         ├─ store に存在: 前回モードを返す
  │         └─ 初回: config.input.default_mode を返す
  │              ※ ターミナル（CASCADIA_HOSTING_WINDOW_CLASS 等）は常に Alphanumeric
  │
  └─ st.set_mode(new_mode)
     set_open_close(KEYBOARD_OPENCLOSE)
```

`Activate` 末尾でも `tm.GetFocus()` で現在の DM を取得し初期モードを適用。  
理由: `Activate` 時点ですでにフォーカスがある場合 `OnSetFocus` は呼ばれないため。

---

## 5. rakukan-engine 詳細

### 5.1 RakunEngine の主要フィールド

```rust
pub struct RakunEngine {
    romaji:             RomajiConverter,        // ローマ字変換（trie）
    kanji:              Option<KanaKanjiConverter>, // LLM 変換器
    config:             EngineConfig,
    hiragana_buf:       String,                 // 確定前のひらがな
    pending_romaji_buf: String,                 // 未確定のローマ字（例: "sh"）
    romaji_input_log:   Vec<String>,            // F9/F10 ローマ字復元用ログ
    committed:          String,                 // LLM コンテキスト用の確定済み文章
    dict_store:         Option<DictStore>,      // mozc + SKK + ユーザー辞書
}
```

### 5.2 文字入力パイプライン（push_char）

```
push_char(c)
  │
  ├─ pending_romaji が空 かつ ASCII 数字:
  │    全角数字（０〜９）に変換して hiragana_buf へ
  │
  ├─ pending_romaji が空 かつ ASCII 記号（trie 対象外）:
  │    全角記号に変換して hiragana_buf へ（＠ → ＠、etc.）
  │
  └─ その他（英字・trie 対象記号）:
       RomajiConverter.push(c)
         ├─ Buffered:   pending_romaji に積み上げ
         ├─ Converted:  ひらがなを hiragana_buf に追加 + romaji_input_log に記録
         └─ PassThrough: 変換できずスルー（直接 hiragana_buf へ）
```

特殊ケース:
- `push_raw(c)`: ローマ字変換をバイパスして hiragana_buf に直接追加（テンキー記号用）
- `push_fullwidth_alpha(c)`: 全角大文字を hiragana_buf に追加（Shift+A〜Z 用）

### 5.3 RomajiConverter（trie ベース）

`trie.rs` にローマ字ルール trie を構築。`rules.rs` にルール定義。  
出力: `ConversionEvent::Converted(String)` / `Buffered` / `PassThrough(char)`

主なルール:
- `a`→`あ`, `ka`→`か`, `shi`→`し`, `tchi`→`っち`, etc.
- `,`→`、`, `.`→`。`, `/`→`・`, `[`→`「`, `]`→`」`, `\`→`￥`
- `-`→`ー`（長音符）

### 5.4 バックグラウンド変換（conv_cache.rs）

常駐ワーカースレッド（`rakukan-conv-worker`）が LLM 変換を非同期実行。

```
State::Idle
  │ bg_start(n) → pending に Request を積む + notify
  ▼
State::Idle（pending=Some） ← ワーカーが気づくまでの中間
  │ ワーカーが取り出し
  ▼
State::Running { key }
  │ KanaKanjiConverter.convert() 完了
  ▼
State::Done { key, converter, candidates }
  │ take_ready(key) または reclaim()
  ▼
State::Idle
```

キーは `hiragana_buf` の内容（変換対象ひらがな）。  
`bg_take_candidates(key)` でキーが一致する Done 結果を取り出す。  
キーが不一致（変換途中に入力が変わった）の場合は `None` を返す。

### 5.5 候補マージ（merge_candidates）

```
merge_candidates(llm_cands, limit)
  │
  ├─ ユーザー辞書候補（lookup_user）       最優先
  ├─ LLM 候補（llm_cands）
  └─ mozc/SKK 辞書候補（lookup_dict）    最大 limit 件
  
  重複除去して返却（先着順）
```

### 5.6 コンテキスト管理（committed）

LLM に渡す前文（`committed`）は 200 文字を超えたら文境界でトリミング。  
文境界は `。！？!?.\n` 等の直後として判定（`last_n_sentences_start()` 関数）。

---

## 6. rakukan-engine-abi（ABI ブリッジ）

`DynEngine` は `rakukan_engine_{backend}.dll` を実行時ロードし、  
`EngineVTable`（関数ポインタのテーブル）経由で DLL の機能を呼び出す。

### 主要 ABI 関数

| カテゴリ | 関数 | 説明 |
|----------|------|------|
| ライフサイクル | `create`, `destroy` | エンジン生成・破棄 |
| 文字入力 | `push_char`, `push_raw`, `push_fullwidth_alpha`, `backspace` | 入力バッファ操作 |
| プリエディット | `preedit_display`, `preedit_is_empty`, `hiragana_text` | 表示用テキスト取得 |
| BG 変換 | `bg_start`, `bg_status`, `bg_take_candidates`, `bg_reclaim`, `bg_wait_ms` | 非同期変換制御 |
| 確定 | `commit`, `reset_preedit`, `force_preedit`, `reset_all` | 状態リセット |
| 辞書 | `merge_candidates` | 辞書＋LLM 候補マージ |
| 初期化 | `start_load_model`, `start_load_dict`, `is_kanji_ready`, `is_dict_ready` | 非同期ロード |
| 学習 | `learn` | ユーザー辞書への学習 |

### バックエンド選択

```
load_auto(dir, config_json)
  │
  ├─ config_json の gpu_backend キー
  └─ デフォルト: cpu
  
  → rakukan_engine_{cuda|vulkan|cpu}.dll をロード
```

---

## 7. 設定ファイル

### config.toml

配置先: `%APPDATA%\rakukan\config.toml`  
リロードタイミング: IME モード切り替え時（`reload_on_mode_switch = true` の場合）

```toml
[general]
log_level = "debug"         # error/warn/info/debug/trace
# gpu_backend = "cuda"      # cuda/vulkan/cpu（未指定=自動検出）
# main_gpu = 0
# model_variant = "jinen-v1-small-q5"
# model_variant = "jinen-v1-xsmall-q5"

[keyboard]
layout = "jis"              # us/jis/custom（デフォルト: jis）
reload_on_mode_switch = true

[input]
default_mode = "alphanumeric"   # alphanumeric/hiragana
remember_last_kana_mode = true  # ウィンドウごとにモードを記憶

[live_conversion]
enabled = false
debounce_ms = 80

[diagnostics]
dump_active_config = true
warn_on_unknown_key = true

# num_candidates = 9        # 旧互換キー（デフォルト: 9）
```

### keymap.toml

配置先: `%APPDATA%\rakukan\keymap.toml`  
リロードタイミング: IME オフ→オン（Activate）時のみ

```toml
preset = "ms-ime-jis"      # ms-ime-jis/ms-ime-us/custom
inherit_preset = true       # プリセットを基底として [[bindings]] で上書き

[[bindings]]
key    = "Ctrl+J"
action = "mode_hiragana"
```

プリセット `MsImeJis` の主要バインド: Space=変換、Henkan=変換、Enter=ひらがな確定、  
Escape=キャンセル、Muhenkan=CycleKana、Zenkaku=ImeToggle、Hiragana_key=ModeHiragana、etc.

#### name_to_vk の VK コード対照表

| キー名 | VK コード | 備考 |
|--------|-----------|------|
| `zenkaku`/`hankaku`/`kanji` | `0x19` | VK_KANJI（全角/半角キー）|
| `henkan` | `0x1C` | VK_CONVERT |
| `muhenkan` | `0x1D` | VK_NONCONVERT |
| `eisuu` | `0xF0` | VK_DBE_ALPHANUMERIC |
| `katakana` | `0xF1` | VK_DBE_KATAKANA |
| `hiragana_key` | `0xF2` | VK_DBE_HIRAGANA |
| `a`〜`z`（1文字） | `0x41〜0x5A` | to_ascii_uppercase() で変換 |

---

## 8. 辞書システム（rakukan-dict）

### 構成

```
DictStore
  ├─ mozc: Option<MozcDict>   mozc バイナリ辞書（mmap）
  ├─ skk:  Vec<SkkEntry>      SKK 辞書（メモリ展開）
  └─ user: RwLock<HashMap>    ユーザー辞書（リアルタイム更新可）
```

### 辞書ロード（dict/loader.rs）

4 ステップで辞書をロード。失敗時はステップ名付きエラーを返す。

1. `step_resolve_paths` — `%LOCALAPPDATA%\rakukan\dict\` からパス解決
2. `step_probe_mozc` — ファイル存在・サイズ・マジックバイト確認
3. `step_open_mozc` — MozcDict::open（mmap + ヘッダー検証）
4. `step_load_store` — DictStore::load（ユーザー辞書込み）

### 候補優先順位

`merge_candidates` での優先順位:
1. ユーザー辞書（`lookup_user`）
2. LLM 出力（`llm_cands`）
3. mozc/SKK 辞書（`lookup_dict`）

---

## 9. LLM 変換（kanji/）

### モデル

- モデル: jinen-v1（日本語かな漢字変換専用モデル）
  - `jinen-v1-small-Q5_K_M.gguf`（約 88MB）
  - `jinen-v1-xsmall-Q5_K_M.gguf`（約 30MB）
- 実装: llama.cpp（`llama-cpp-2` クレート経由）

### 変換フロー（backend.rs::convert）

```
convert(reading, context, num_candidates)
  │
  ├─ hiragana → katakana 変換（モデル入力形式）
  ├─ build_jinen_prompt(katakana, context) でプロンプト構築
  ├─ model.tokenize(prompt)
  │
  ├─ num_candidates == 1: greedy decoding（高速）
  └─ num_candidates > 1:  beam search（beam_size = min(n, 3)）
                           generate_beam_search() で複数候補生成
  │
  ├─ clean_model_output() でルビ等を除去
  └─ 重複除去して返却
```

### clean_model_output

- 出力末尾のノイズ除去
- ルビ形式（`健診(けんしん)`）の除去：`strip_furigana()` が括弧内がひらがな/カタカナのみの場合に削除

---

## 10. エンジン初期化フロー

```
[rakukan.dll ロード時]
  DLL_PROCESS_ATTACH
    └─ tracing subscriber 初期化
       config_path 設定
       config_save_default() ← config.toml がなければ作成
       keymap_save_default() ← keymap.toml がなければ作成

[アプリがフォーカスを得る]
  Activate(thread_mgr, tid)
    ├─ Keymap::load()           ← keymap.toml 読み込み
    ├─ engine_start_bg_init()   ← BG スレッドで DLL ロード開始
    │    └─ [BG スレッド]
    │         create_engine()   ← rakukan_engine_{backend}.dll をロード
    │         engine.start_load_dict()   ← 辞書 BG ロード
    │         engine.start_load_model()  ← LLM BG ロード
    │         langbar_update_set()       ← 言語バー更新フラグをセット
    ├─ KeyEventSink 登録
    ├─ ThreadMgrEventSink 登録  ← OnSetFocus を受け取るようになる
    └─ GetFocus() で初期モード適用 ← default_mode を即時反映
```

BG 初期化が完了するまで（辞書・モデルの両方が `ready`）、変換は辞書のみで動作する。  
辞書は数百ミリ秒、LLM は GPU 依存で 1〜10 秒程度で初期化完了。

---

## 11. スレッドモデルとロック規則

### スレッド一覧

| スレッド名 | 役割 | 制約 |
|-----------|------|------|
| TSF（STA）スレッド | OnKeyDown・EditSession・Activate | ブロック禁止。try_lock() のみ |
| `rakukan-engine-init` | エンジン DLL のロード | 一度だけ起動（AtomicBool で制御） |
| `rakukan-conv-worker` | LLM 変換（Condvar で待機） | CACHE.inner の Mutex を保持して変換実行 |
| `rakukan-reload-watcher` | エンジン再起動イベント待機 | WaitForSingleObject（INFINITE）でブロック |

### ロック規則

- **ホットパス（OnTestKeyDown / OnKeyDown）**: すべて `try_lock()`。失敗したらスキップ。
- **Activate / BG スレッド**: `lock()` を使用可（ブロック許容）。
- **`COMPOSITION.take()` は EditSession 内で行うこと**。セッション外で take すると、セッション実行前に次キー入力が来た際に `composition=None` を見て誤った位置から新 composition が開始される。

---

## 12. 文字種変換（F6〜F10）

`text_util.rs` に変換関数を実装。`factory.rs::on_kana_convert` から呼ばれる。

| キー | 変換内容 | サイクル |
|------|----------|----------|
| F6 | プリエディット → ひらがな | なし |
| F7 | プリエディット → 全角カタカナ | なし |
| F8 | プリエディット → 半角カタカナ | なし |
| F9 | プリエディット → 全角英数（romaji_input_log から復元） | 全角小→全角大→全角先頭大→全角小 |
| F10 | プリエディット → 半角英数（同上） | 半角小→半角大→半角先頭大→半角小 |

---

## 13. 既知の制約・注意事項

### TSF 制約
- `RequestEditSession` は TSF スレッドから呼ぶこと（WndProc からは呼べない）
- これがライブ変換（WM_TIMER ベース）を複雑にしている最大の要因
- 回避策は `PostMessage` で TSF スレッドに処理を委譲する方式

### エンジン DLL の ABI 制約
- エンジン DLL の API を変更した場合、`cargo make build-engine` が必要
- `-SkipEngine` フラグで TSF のみビルドした場合、ABI 変更があると IME が言語バーで選択不可になる

### llama.cpp の制約
- `generate_beam_search_d1_greedy_batch` は `n_batch > n_ctx` で C レベルの `abort()` を呼ぶ
- Rust の `catch_unwind` では捕捉できない。`beam_size` と `n_ctx` の動的管理が必要

### CUDA の制約
- CUDA ランタイム DLL は `C:\Windows\System32` に配置必須
- 対象: `cublas64_13.dll`, `cublasLt64_13.dll`, `cudart64_13.dll`

### DMポインタの取得
- `d as *const _ as usize` はスタックアドレスになるため **使用禁止**
- 正しい取得方法: `*(d as *const ITfDocumentMgr as *const usize)`

---

## 14. ビルド・開発フロー

### 通常の開発ビルド（TSF のみ変更）

```powershell
cargo make build-tsf    # TSF DLL のみビルド + インストール
cargo make reinstall    # 再インストール（ビルドなし）
```

### エンジン DLL の変更を含む場合

```powershell
cargo make build-engine  # エンジン DLL ビルド（CUDA/Vulkan 含む）
cargo make reinstall
```

### ログ確認

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 30 -Wait
```

### ビルドパス

| 種別 | パス |
|------|------|
| ソース | `C:\Users\n_fuk\source\rust\rakukan` |
| ビルド成果物 | `C:\rb\release` |
| インストール先 | `%LOCALAPPDATA%\rakukan\` |
| 設定・辞書 | `%APPDATA%\rakukan\` |

---

## 15. 今後のロードマップ

### Phase 4a — ライブ変換基盤

`SessionState::LiveConv` バリアントを追加し、入力中に随時 LLM 変換を走らせる。

- `bg_take_top1` API: 変換結果の 1 候補を非確定表示する
- タイマー汎化: WM_TIMER を `RequestEditSession` 経由で動かす仕組み
- `config.toml` の `live_conversion.enabled` フラグを有効化

### Phase 4b — キーストローク統合

- キーストロークごとに `bg_start` を呼ぶ（デバウンス付き）
- `on_live_timer` で `bg_take_top1` → compose 更新
- **最大の技術課題**: WM_TIMER の WndProc から `RequestEditSession` を呼べないため、`PostMessage` で TSF スレッドに処理を委譲するアーキテクチャが必要

### Phase 4c — 仕上げ

- デバウンス（80ms デフォルト）
- ライブ変換中のビジュアル（点線アンダーライン等）
- アプリ互換性テスト（メモ帳・Word・ブラウザ等）

### その他優先度中

- `kanji/backend.rs` と `dict/loader.rs` へのログ追加
- SplitPreedit の多文節チェーン改善
- LLM 候補数の増加（現状 `min(n, 3)`）
- 数字・かな混在入力対応
