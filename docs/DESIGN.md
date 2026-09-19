# Rakukan 詳細設計書

対象: main（0.11.7 以降）。節ごとに変更に合わせて更新しており、全節を一度に照合した版ではない  
最終更新: 2026-09-17（6.1 節を protocol v5 と現行のホスト再起動経路に合わせた）

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
- ライブ変換と範囲指定変換 UI を含む日本語入力フロー

---

## 2. クレート構成

```text
rakukan/
├── crates/
│   ├── rakukan-tsf/          TSF レイヤー（Windows IME 本体、DLL）
│   ├── rakukan-engine/       変換エンジン（LLM + 辞書、DLL 実体）
│   ├── rakukan-engine-abi/   エンジン DLL の動的ローダー（ABI ブリッジ）
│   ├── rakukan-engine-rpc/   Named Pipe + postcard RPC レイヤー
│   ├── rakukan-engine-host/  rakukan-engine-host.exe（エンジン常駐プロセス）
│   ├── rakukan-engine-cli/   エンジン単体テスト用 CLI
│   ├── rakukan-dict/         辞書ライブラリ（mozc / SKK / ユーザー辞書）
│   ├── rakukan-dict-builder/ 辞書ビルドツール
│   └── rakukan-tray/         システムトレイ常駐プロセス（モード表示）
├── config/
│   ├── config.toml           設定ファイルテンプレート
│   └── keymap.toml           キーバインドテンプレート
└── scripts/                  ビルド・インストールスクリプト
```

### プロセス構成とロード関係 (v0.4.4 以降)

```text
[任意のアプリケーションプロセス]                [rakukan-engine-host.exe]
(Zoom.exe / Dropbox.exe / notepad.exe / ...)    (ユーザーセッションで 1 個、常駐)

 rakukan_tsf.dll                                 rakukan_engine-abi（静的リンク）
   └── rakukan-engine-rpc（client）                └── rakukan_engine_{cuda|vulkan|cpu}.dll
          │                                              └── llama.cpp（静的リンク）
          │                                              └── rakukan-dict（静的リンク）
          │
          │  Named Pipe
          └─────────────────────────────────────▶ rakukan-engine-rpc（server）
             \\.\pipe\rakukan-engine-<user-sid>
```

**重要: 0.4.4 以降、TSF DLL は `rakukan_engine_*.dll` を一切 LoadLibrary しない。**

0.4.3 までは `rakukan_tsf.dll → rakukan-engine-abi → rakukan_engine_{backend}.dll`
を対象アプリケーションプロセス内で直接読み込んでいたため、llama.cpp とその C++
ランタイム（`msvcp140.dll`）がすべての Windows アプリに持ち込まれ、
Zoom / Dropbox 等で `msvcp140.dll` のクロスロード由来の `0xc0000005` を誘発していた。

0.4.4 ではエンジン DLL を専用バイナリ `rakukan-engine-host.exe` に集約し、
TSF 側は Named Pipe 越しに RPC するクライアントとしてのみ振る舞う。
対象アプリケーションプロセスには llama.cpp 由来のコードは一切入らない。

rakukan-tsf と rakukan-engine を **別 DLL** として分離する理由は引き続き:
llama.cpp のビルドが重く（数分〜十数分）、TSF 層の変更ごとに再ビルドしたくないため。
ABI 境界は引き続き C の FFI（`extern "C"` 関数ポインタのテーブル）で構成されるが、
この ABI は現在 **`rakukan-engine-host.exe` 内部でのみ使用** される。

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
  │ IME オン（ImeMode::On）                              │
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
| `engine/ime_mode.rs` | IME オン/オフ列挙型（`ImeMode`） |
| `engine/user_action.rs` | ユーザーアクション列挙型（`UserAction`） |
| `engine/text_util.rs` | 文字種変換（F6〜F10 用） |
| `diagnostics.rs` | ログイベント集約（`DiagEvent`） |

### 4.2 グローバル状態（state.rs）

TSF 層の状態は複数のグローバル静的変数で管理される。  
ホットパス（OnTestKeyDown / OnKeyDown）では**ブロックを避ける**ことが最優先制約であり、  
Mutex の取得はすべて `try_lock()` を使用する。

| 変数 | 型 | 用途 |
|------|----|------|
| `IME_MODE_ATOMIC` | `AtomicU8` | 現在の IME オン/オフ（ロックなし高速読み取り用） |
| `IME_STATE` | `Mutex<IMEState>` | IME オン/オフの正式な保持場所 |
| `RAKUKAN_ENGINE` | `Mutex<EngineWrapper>` | エンジン DLL インスタンス |
| `ENGINE_INIT_STARTED` | `AtomicBool` | BG 初期化の二重スポーン防止フラグ |
| `SESSION_STATE` | `Mutex<SessionState>` | TSF 層の論理状態（Idle / Preedit / Waiting / ...） |
| `SESSION_SELECTING` | `AtomicBool` | 候補選択中フラグ（ホットパス用） |
| `COMPOSITION` | `Mutex<CompositionWrapper>` | 現在の `ITfComposition` オブジェクト |
| `CARET_RECT` | `Mutex<CaretRect>` | キャレット矩形（候補ウィンドウ位置計算用） |
| `LANGBAR_UPDATE_PENDING` | `AtomicBool` | BG 初期化完了後の言語バー更新フラグ |
| `DOC_MODE_STORE` | `Mutex<HashMap<usize, ImeMode>>` | DocumentManager ごとの IME オン/オフ記憶 |
| `CONFIG_MANAGER` | `Mutex<ConfigManager>` | 設定ファイルの読み込み・キャッシュ |

#### IME 状態（ImeMode）

```rust
pub enum ImeMode {
    On,   // 0 (AtomicU8) かな漢字変換
    Off,  // 1            直接入力
}
```

入力状態はこの 2 値だけで表す。「ひらがなモード」「カタカナモード」「英数モード」という
独立した概念は持たない（v0.11.4 で廃止。カタカナは F7 / 無変換 cycle_kana で入力する）。

`Off` ではキーをそのまま素通りさせ（`OnTestKeyDown` が `FALSE` を返す）、アプリが直接処理する。

**唯一の正は内部状態**（`IME_STATE.ime_mode` とそのアトミック鏡）。TSF コンパートメント
`KEYBOARD_OPENCLOSE` は内部状態から導出して書くだけで、表示（言語バー・インジケーター・
トレイ通知）もキー処理もコンパートメントは読まない。状態変更はすべて
`tsf/ime_sync::apply` を通る（キー操作 / 言語バーメニュー / フォーカス移動時の復元 /
Activate 初期化 / 外部アプリによるコンパートメント変更の取り込み）。経路ごとに片方だけ
更新して表示がずれる不具合（v0.11.3 で「A」表示のまま かな入力になる）を構造的に防ぐ。

### 4.3 セッション状態（SessionState）

TSF 層の論理状態機械。`SESSION_STATE` Mutex で保護される。

```
Idle
  │ キー入力（Input）
  ▼
Preedit { text }          ─ ひらがな入力中
  │ BG タイマー発火
  ▼
LiveConv {                ─ ライブ変換表示中
  reading, preview,         preview = 辞書/学習/LLM をマージしたトップ候補
  preview_for               preview を生成した reading。追加入力での継続表示
}                           （preview + かな接尾辞）は、preview_for と直前の reading が
                            ともに現在の reading の prefix のときだけ行い、
                            そうでなければ生の読みへ戻す（v0.11.0）
  │ Space（Convert）
  ▼
Waiting { text, pos }     ─ LLM 変換中（⏳ 表示）
  │ bg_take_candidates 成功
  ▼
Selecting {               ─ 候補選択中
  candidates, selected,
  remainder, ...
}
  │ Enter / 数字キー → Idle
  │ Shift+Left/Right ↓
  ▼
RangeSelect {             ─ 範囲指定変換モード
  full_reading,             全文ひらがなを表示
  select_end,               先頭から select_end 文字を選択
  original_preview          ESC で LiveConv に復帰するための元 preview
}
  │ Space → 選択範囲を変換（Selecting へ）
  │ Enter → 選択範囲を確定、残りで LiveConv 再開
  │ ESC   → LiveConv に復帰
  ▼
Idle
```

`SESSION_SELECTING` AtomicBool は `Selecting` と `RangeSelect` 時に `true` になり、  
`OnTestKeyDown` の判定（キーを IME が消費するか）に使用される。  
これにより `SESSION_STATE` の Mutex を取らずにホットパスの判定が可能。  

### 4.3.1 範囲指定変換の実装原則

範囲指定変換では、文章ごとの例外処理やリテラル文字列に依存した実装を入れない。  
目標は「特定の語をうまく切ること」ではなく、「どの文章でも同じ操作感で範囲指定・変換・確定できること」である。

- 範囲指定は reading（ひらがな）の文字位置ベースで行う。vibrato 等の形態素解析には依存しない
- TSF 側で特定の助詞・単語・文字列をハードコードして切り分けない
- `Shift+Right/Left`、`Space`、`Enter` の意味は文章に依らず固定する
- 確定した部分は commit して composition から消え、残りの reading で LiveConv を再開する
- 先頭から順に確定していく方式で、確定済み部分の再編集は行わない

禁止事項:

- 「`わさび` を含む時はこう切る」のような語彙依存の分岐
- 「`と` `は` `が` ならこう扱う」のような文字依存の runtime ルール
- 一時的なデバッグやテスト用のつもりで、特定文章向けの分節補正を本実装に残すこと

### 4.4 キー処理フロー（OnKeyDown）

```
OnKeyDown(wparam=VK)
  │
  ├─ IME オフ → キーを素通り（FALSE）
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
       ├─ ImeToggle / ImeOn / ImeOff → switch_ime → ime_sync::apply（内部状態 → KEYBOARD_OPENCLOSE）
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
  │              ※ ime_off_apps / ime_on_apps（exe 名）の対象アプリは記憶を通さず
  │                アクティブ化ごとに設定値から始める（Issue #51）
  │
  └─ ime_sync::apply(new_mode)  ← 内部状態と KEYBOARD_OPENCLOSE を同時に揃える
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

### 5.5 候補マージ（merge_candidates_for_reading）

```
merge_candidates_for_reading(reading, llm_cands, limit)
  │
  ├─ ユーザー辞書候補（lookup_user）       最優先
  ├─ 学習履歴候補（learn_history）
  ├─ mozc/SKK 辞書候補（lookup_dict）
  └─ LLM 候補（llm_cands）
  
  重複除去して返却（先着順）
```

TSF / host からは常に `merge_candidates_for_reading(reading, llm_cands, limit)` で読みを明示する。
`reading` は「実際に候補が取れたキー」（`bg_take_candidates` が成功した `hiragana_text()` または
preedit、同期 fallback では現在の `hiragana_text()`）で、weak merge 判定・同期 fallback まで
同じ値を使う。engine 内部の `hiragana_buf` を読みとして使う旧 `merge_candidates()` は、
ホストを複数アプリで共有する構成で別の読みで辞書を引いてユーザー辞書・学習履歴が落ちる
（Issue #9）ため v0.11.0 で RPC / host から削除した（engine DLL の ABI シンボルとエンジン内の
テストにだけ残る）。これにより `かっことじ` → `』` のようなユーザー辞書候補を、LLM 結果が
まだ無い段階でも preview や Space 変換に反映できる。

### 5.6 コンテキスト管理（committed）

LLM に渡す前文（`committed`）は 200 文字を超えたら文境界でトリミング。  
文境界は `。！？!?.\n` 等の直後として判定（`last_n_sentences_start()` 関数）。

---

## 6. rakukan-engine-abi（ABI ブリッジ）

`DynEngine` は `rakukan_engine_{backend}.dll` を実行時ロードし、  
`EngineVTable`（関数ポインタのテーブル）経由で DLL の機能を呼び出す。

**重要: 0.4.4 以降、`DynEngine` を直接使うのは `rakukan-engine-host.exe` のプロセス内部のみ。**
TSF DLL 側は `rakukan-engine-rpc` 経由で Named Pipe ごしに呼び出す（6.1 節参照）。

### 主要 ABI 関数

| カテゴリ | 関数 | 説明 |
|----------|------|------|
| ライフサイクル | `create`, `destroy` | エンジン生成・破棄 |
| 文字入力 | `push_char`, `push_raw`, `push_fullwidth_alpha`, `backspace` | 入力バッファ操作 |
| プリエディット | `preedit_display`, `preedit_is_empty`, `hiragana_text` | 表示用テキスト取得 |
| BG 変換 | `bg_start`, `bg_status`, `bg_take_candidates`, `bg_reclaim`, `bg_wait_ms` | 非同期変換制御 |
| 確定 | `commit`, `reset_preedit`, `force_preedit`, `reset_all` | 状態リセット |
| 辞書 | `merge_candidates_for_reading` | 辞書＋LLM 候補マージ（読みを明示。旧 `merge_candidates` は host から呼ばない） |
| 診断 | `build_info`（任意シンボル `engine_build_info`） | version / git sha / ビルド時刻 / ABI / DLL 内ログ初期化結果。host が起動時に自分の識別子と突き合わせ、別ビルドなら WARN（Issue #8） |
| 初期化 | `start_load_model`, `start_load_dict`, `is_kanji_ready`, `is_dict_ready` | 非同期ロード |
| 学習 | `learn` | 学習履歴 (`learn_history.bin`) への記録。MOZC/ユーザー辞書由来 surface のみ対象 |

### バックエンド選択

```text
load_auto(dir, config_json)
  │
  ├─ config.toml の gpu_backend が cuda / vulkan / cpu（明示指定）
  │    → その DLL だけをロード。失敗しても他の backend へ fallback しない
  └─ 未指定 / "auto"
       → cuda → vulkan → cpu の順に「実際にロード」を試み、最初に成功したものを採用
         失敗理由は WARN（backend::auto: <b> failed: ...; trying next）
         全て失敗したら各 backend の理由をまとめたエラー
```

v0.11.0 まではファイルの有無だけで backend を決めていたため、CUDA ランタイム未導入の環境で
`rakukan_engine_cuda.dll` が存在するとロード失敗のまま先へ進めず IME が無反応になった（Issue #2）。
`LoadLibrary` が `ERROR_MOD_NOT_FOUND`（126）で失敗した場合は依存 DLL 不足のヒントを付ける。

---

## 6.1 rakukan-engine-rpc / rakukan-engine-host（v0.4.4〜）

### RPC レイヤー構成

```text
[TSF プロセス]                              [rakukan-engine-host.exe]

RpcEngine (client)                          serve() (server)
  ├─ Mutex<Connection>                        ├─ Arc<Mutex<Option<DynEngine>>>
  ├─ config_json (再接続時用)                  └─ 接続ごとに 1 スレッド
  └─ Named Pipe 1 本                                │
        │                                           ▼
        │    [u32 LE len][postcard payload]     dispatch(Request) → DynEngine メソッド
        └───────────────────────────────────────▶
```

### トランスポート

- パイプ名: `\\.\pipe\rakukan-engine-<USERNAME-sanitized>`
- フレーミング: `[u32 LE payload-length][postcard payload]`
- エンコード: **postcard**（serde, forward-compat, 小サイズ）
- バイトストリーム・同期 I/O（tokio 非依存）
- DACL: `D:P(A;;GA;;;<current-user-sid>)(A;;GA;;;SY)` を明示的に構築して `CreateNamedPipeW` に渡す

### プロトコル

現在の `PROTOCOL_VERSION` は **5**（v0.11.7 で `EngineHealth` を追加）。
`Hello` で一致しないと接続できないため、上げたリリースではサインアウト／サインインで
古いホストを終わらせる必要がある。

`rakukan-engine-rpc/src/protocol.rs` の `Request` enum は DynEngine の全メソッドを
1 対 1 でマップしている。代表的なバリアント:

| Request | 役割 |
| --- | --- |
| `Hello { protocol_version }` | 接続直後のバージョン交換 |
| `Create { config_json }` | DynEngine を初回生成（既存なら no-op） |
| `Reload { config_json }` | 既存 DynEngine を drop して新 config で再生成 |
| `PushChar(u32) / Backspace / ResetAll / ...` | 入力操作 |
| `BgStart / BgWaitMs / BgTakeCandidates / ...` | BG 変換 |
| `MergeCandidatesForReading` | 候補マージ。TSF 側が reading を明示する（旧 `MergeCandidates` は `_ReservedMergeCandidates` としてスロットのみ残し、host は Error を返す） |
| `ConvertSync / SegmentSurface / ...` | 同期変換 |
| `Shutdown` | ホストに自己終了を依頼する（応答後に `process::exit(0)`） |
| `ShutdownIfConfigDiffers { config_json }` | ホストの config と異なるときだけ自己終了する（`Bool(true)` = 終了する） |
| `EngineHealth` | ホストの健全性 `ok` / `recovering` / `unrecoverable` を返す（Issue #43）。エンジンのロックを取らずに即答する |
| `Bye` | クライアント切断宣言 |

`Response` は `Hello / Unit / Bool / U32 / I32 / String / Strings / SegmentsModel / Error / InputCharResult` のいずれか
（`_ReservedSegments` / `_ReservedSegmentBlocks` は ABI v7 で廃止したスロット）。
enum の discriminant は宣言順なので、`Request` / `Response` の variant は**末尾にだけ追加する**。
ホスト側で panic したり DynEngine が未生成のまま他リクエストが来た場合は
`Response::Error(String)` を返し、クライアント側は空値を返すかそのまま無視する。

### エンジン状態共有

ホスト内では **1 個の `DynEngine` を全クライアント接続で共有** する（`Arc<Mutex<...>>`）。
llama 推論は逐次なので排他で問題にならず、model/dict のロードが重複しない利点が大きい。

hiragana_buf 等のセッション状態は TSF 側がフォーカス変化で `ResetAll` を呼ぶ
既存運用でカバーされる。

### ホストプロセスのライフサイクル

1. TSF 側の `engine_try_get_or_create()` が最初の入力で bg init スレッドを spawn
2. bg init が `RpcEngine::connect_or_spawn(Some(config_json))` を呼ぶ
3. `ensure_connected()` がパイプ接続を試行 → 失敗したら `CreateProcessW`
   （`DETACHED_PROCESS | CREATE_NO_WINDOW`）で `rakukan-engine-host.exe` を起動
4. 最大 5 秒までリトライ接続、成功したら `Hello` → `Create` を送信
   （`Create` はホストの config と同じなら no-op、異なればエンジンを作り直す）
5. ホストがクラッシュして別 PID で再起動した場合、`call_with_retry` が
   パイプエラーを検知して 1 回だけ再接続し、**保存済み `config_json` で `Create` を再送**
6. 接続直後の `Hello` / `Create` の失敗が 15 秒に 3 回続くと、その TSF プロセスからの
   spawn を 30 秒止める（`HostSpawnGuard`）。起動に成功すると回数は 0 に戻るため、
   起動後の異常終了の繰り返しは数えていない
7. 辞書・モデルの注入はホストの `dispatch_engine` 冒頭で毎回試みる（v0.11.7）。
   TSF 側のラッチや、ホストが入れ替わった理由に依存しない
8. 現状ホストは常駐（idle 自死しない）

### 推論失敗からの復帰（v0.11.7、Issue #43）

GPU ドライバ更新・スリープ復帰・TDR でホストの GPU デバイスだけが無効になると、推論は
詰まらず即座に失敗し続ける。判断と抑止はホスト 1 プロセスに置く（TSF はアプリごとに別
プロセスなので、TSF 側で判断すると各アプリが共有ホストを順に再起動してしまう）。

- `dispatch_engine` の応答後に `bg_status()` を観測し、**状態が変わったときだけ**数える
  （`crates/rakukan-engine-rpc/src/health.rs` の `HealthTracker`）
- 連続 3 回失敗 → `%LOCALAPPDATA%\rakukan\engine-recovery.txt` に
  `<exited_at_ms> <attempt>` を書いてホストが自己終了し、次の呼び出しでクライアントが spawn し直す
- マーカーが 5 分以内で `attempt >= 2` の状態からまた 3 回失敗 → `unrecoverable`（ホストは
  生き続け、辞書候補のみで動作）
- 推論が 1 回成功したら失敗の回数とマーカーを捨てる
- TSF は `EngineHealth` を問い合わせて status 行の文言を決めるだけ
- 検証用に `[diagnostics] force_inference_failure` で推論を必ず失敗させられる
- プロセス内でモデルだけ作り直す方式は、現在の ABI では converter を差し替えられないため採っていない

### config.toml の即時反映

`engine_reload()`（IME モード切替時の config 変更検出、または設定アプリ保存時の名前付きイベント
`Local\rakukan.engine.reload` から呼ばれる）は、エンジンをホスト内で作り直さず**ホストプロセスごと
再起動する**（v0.7.1 M1.6 T-HOST1。DLL を drop すると BG スレッドが参照中の DLL が unmap されて
AV を誘発するため）。

1. バックグラウンドスレッドで `config.toml` を読み直し、`build_engine_config_json()` で設定 JSON を作る
2. `ShutdownIfConfigDiffers { config_json }` を送る
   - `Bool(false)`（同じ config）: ホストを維持し、ready ラッチだけ落として終わる
   - `Bool(true)`（異なる）: ホストは応答後に自己終了する
   - エラー（比較できない）: 無条件の `Shutdown` にフォールバック
3. ラッチを落とし、100ms 待ってから TSF 側のハンドルを捨てる（終了途中のパイプへ再接続する race を避ける）
4. 次の呼び出しで `connect_or_spawn` が新しいホストを spawn し、新しい config で `Create` する

`engine_reload_force()`（言語バーメニューの「エンジン再起動」と BG ウォッチドッグ）は比較せずに `Shutdown` を送る。

TSF DLL はアプリごとに別プロセスで動くため、同じ config で複数プロセスが reload しても
ホストの再起動は 1 回で済む（2 回目以降は `Bool(false)`）。ただし reload イベントは
auto-reset なので、1 回の Set で届くのは 1 プロセスだけである。

`Request::Reload`（ホスト内でエンジンを drop → 再生成）はプロトコルに残っているが、TSF からは使っていない。

---

## 7. 設定ファイル

### config.toml

配置先: `%APPDATA%\rakukan\config.toml`  
リロードタイミング: IME モード切り替え時（`reload_on_mode_switch = true` の場合）。mtime が変わっていなければ読まない

```toml
[general]
log_level = "debug"         # error/warn/info/debug/trace
# gpu_backend = "auto"      # auto（cuda→vulkan→cpu を順に実ロード）/ cuda / vulkan / cpu（明示指定は fallback しない）
# main_gpu = 0
# model_variant = "jinen-v1-small-q5"
# model_variant = "jinen-v1-xsmall-q5"

[keyboard]
layout = "jis"              # us/jis/custom（デフォルト: jis）
reload_on_mode_switch = true

[input]
default_mode = "off"            # off/on（旧値 alphanumeric/hiragana も受け付ける）
remember_last_kana_mode = true  # ウィンドウごとに IME オン/オフを記憶

[live_conversion]
enabled = false
debounce_ms = 80

[diagnostics]
dump_active_config = true
warn_on_unknown_key = true

# num_candidates = 6        # 旧互換キー（デフォルト: 6）
```

### keymap.toml

配置先: `%APPDATA%\rakukan\keymap.toml`  
リロードタイミング: IME オフ→オン（Activate）時は必ず読み込む。IME オン/オフ切替時は
`keymap.toml` の mtime が変わった場合だけ読み直す（v0.11.0。以前は切替ごとに同期で
読み直しており、8月ログで最大 931ms のキーストールを起こしていた）。更新・作成は新しい
keymap、削除は既定 keymap、parse 失敗は直前の keymap を維持する。

```toml
preset = "ms-ime-jis"      # ms-ime-jis/ms-ime-us/custom
inherit_preset = true       # プリセットを基底として [[bindings]] で上書き

[[bindings]]
key    = "Ctrl+J"
action = "ime_on"
```

修飾キーは `Ctrl+` / `Shift+` / `Alt+`（左右どちらでも一致）に加え、`LCtrl+` / `RCtrl+` /
`LShift+` / `RShift+` / `LAlt+` / `RAlt+` で左右を区別できる（v0.11.4）。照合は `Keymap::resolve_mods`
が「実際の押下状態に一致しうる要求」を限定的なものから順に試すため、左右指定と汎用指定が両方
あれば左右指定が勝つ。

IME 制御アクションは `ime_toggle` / `ime_on` / `ime_off` の 3 つ。旧アクション名は互換のため
別名として読む: `mode_hiragana`→`ime_on`、`mode_alphanumeric`→`ime_off`、`mode_katakana`→`katakana`（F7 変換）。

プリセット `MsImeJis` の主要バインド: Space=変換、Henkan=変換、Enter=ひらがな確定、
Escape=キャンセル、Muhenkan=CycleKana、Zenkaku=ImeToggle、Hiragana_key=ImeOn、Eisuu=ImeOff、Katakana=Katakana(F7)、
Left/Right=CursorLeft/CursorRight、Home/End=CursorHome/CursorEnd、Shift+Left/Right=SegmentShrink/Extend、etc.

Left / Right / Home / End は未確定文字列がある間はアプリへ渡さず IME が消費する（rakukan は
preedit 内キャレットを持たないので位置は変えない。Home / End は RangeSelect 中のみ選択範囲の
右端を先頭 / 末尾へ移す。Issue #11）。未確定文字列が無いときはアプリへ渡す。

#### name_to_vk の VK コード対照表

| キー名 | VK コード | 備考 |
|--------|-----------|------|
| `zenkaku`/`hankaku`/`kanji` | `0x19` | VK_KANJI（全角/半角キー）。JIS 実機が送る `0xF3`（VK_DBE_SBCSCHAR）/ `0xF4`（VK_DBE_DBCSCHAR）は `normalize_key_event` が `0x19` に正規化する |
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
    ├─ （エンジン DLL はロードしない）  ← 0.4.4 以降、初回入力まで完全に遅延
    ├─ KeyEventSink 登録
    ├─ ThreadMgrEventSink 登録  ← OnSetFocus を受け取るようになる
    └─ GetFocus() で初期モード適用 ← default_mode を即時反映
```

最初の入力（`engine_try_get_or_create()` が呼ばれる瞬間）で初めて bg init スレッドが
spawn され、`RpcEngine::connect_or_spawn` がホストプロセスへ接続する:

```text
[最初の入力キー]
  │
  ▼
engine_try_get_or_create()
  │
  └─ [engine-init BG スレッド]
        RpcEngine::connect_or_spawn(Some(config_json))
          │
          ├─ パイプ接続を試行
          │    └─ 失敗 → CreateProcessW(rakukan-engine-host.exe, DETACHED|NO_WINDOW)
          │
          ├─ Hello → Create { config_json } を送信
          │    └─ ホスト側で DynEngine::load_auto が動き、
          │       rakukan_engine_{backend}.dll の LoadLibrary は **ホスト内で発生**
          │
          └─ langbar_update_set()  ← 言語バー更新フラグをセット
```

BG 初期化が完了するまで（辞書・モデルの両方が `ready`）、変換は辞書のみで動作する。  
辞書は数百ミリ秒、LLM は GPU 依存で 1〜10 秒程度で初期化完了。

**重要: Activate 時点ではエンジン DLL に一切触れない。**
これにより Zoom / Dropbox のような IME を使わないアプリでは、
`rakukan-engine-host.exe` も起動しない（ユーザーが実際に入力するまで何も走らない）。

---

## 11. スレッドモデルとロック規則

### スレッド一覧

| スレッド名 | 役割 | 制約 |
|-----------|------|------|
| TSF（STA）スレッド | OnKeyDown・EditSession・Activate | ブロック禁止。try_lock() のみ |
| `rakukan-engine-init` | RPC 経由のエンジン接続（初回のみ） | 一度だけ起動（AtomicBool で制御）。ホスト自動 spawn を含む |
| `rakukan-conv-worker` | LLM 変換（Condvar で待機） | CACHE.inner の Mutex を保持して変換実行 |
| `rakukan-reload-watcher` | エンジン再起動イベント待機 | WaitForSingleObject（INFINITE）でブロック |

別プロセスの `rakukan-engine-host.exe` 側のスレッド:

| スレッド名 | 役割 |
| --- | --- |
| main スレッド | `serve()` でパイプ待受ループ |
| `rakukan-engine-rpc-session` | 1 クライアント接続につき 1 本。`DynEngine` を `Mutex` で排他して逐次処理 |
| llama.cpp 内部ワーカー | 推論実行（backend 依存） |

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

### 4 ステップに分離された標準フロー

```powershell
cargo make build-engine   # ① engine DLL (cpu/vulkan/cuda) ビルド
cargo make build-tsf      # ② tsf/tray/host/dict-builder/WinUI ビルド
cargo make sign           # ③ ビルド成果物に電子署名 (任意)
cargo make install        # ④ コピー + TSF 登録 + tray 起動 (★管理者)
```

### まとめ実行

```powershell
cargo make full-install    # ①〜④ を一括 (リリース向け)
cargo make quick-install   # ②+④ のみ (engine 使いまわし、署名なし、開発用)
```

### ログ確認

| ファイル（`%LOCALAPPDATA%\rakukan\`） | 出所 | 主な内容 |
|------|------|------|
| `rakukan-tsf-<PID>-<起動識別子>.log` | TSF DLL（アプリごとのプロセス） | キー処理、変換経路、`dict not ready after 30s: dict_status=...`（辞書が ready にならない場合） |
| `rakukan-engine-host.log` | `rakukan-engine-host.exe` | backend 選択（`backend::auto` / `Selected backend`）、`engine DLL loaded: ... dll_version=... dll_git=... host_git=...`（host と DLL が別ビルドなら WARN）、RPC の遅延 |
| `rakukan-engine-dll.log` | engine DLL 内の tracing（cdylib は host と subscriber を共有しない） | `dict load failed at [step]: reason`、LLM 変換（`beam conversion done`）、echo strip（`echo sentence dropped`） |

TSF ログは書き込み時に 16 MiB を基準として退避し、5 世代を保持する。
本体を `.log.tmp` に移してから世代を送る。途中で削除・移動に失敗した場合は
`.tmp` と次の操作を `RotatingLog` に保持し、成功済みの操作を繰り返さずに再開する。
失敗後は、開き直した本体がさらに 1 MiB 増えるまで再試行を控えるため、
共有違反などが続く間はサイズ上限を超えることがある。
進行状態を持たない既存の `.tmp` は上書き・削除せず、新たな退避を中止する。

```powershell
.\scripts\merge-logs.ps1 -OutFile merged.log; Get-Content merged.log -Tail 30
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan-engine-host.log" -Tail 30
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan-engine-dll.log" -Tail 30
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

### 完了済み

- ライブ変換（LiveConv）— デバウンス付きタイマー + Phase1A/1B で composition 更新
- 数値保護 — LLM が数字を改変しない仕組み（`digits.rs`）
- アルファベット保護 — 半角/全角の両方を候補として提示
- 範囲指定変換（RangeSelect）— Shift+矢印で先頭から順に確定
- vibrato / SplitPreedit の完全削除 — 分節アライメント問題の根本解決

### 優先度: 中

- **[Engine-Host-1] idle 自死** — 長時間アイドルのメモリ占有削減
- **[Engine-Host-2] ヘルスチェックとクラッシュカウント**
- **[Live-2] display_attr 拡張** — RangeSelect の選択範囲表示の改善
- **用法辞書（Candidate.annotation）** — 候補ウィンドウに同音異義語の用途説明を表示

### 優先度: 低

- **RPC レイテンシ計測**
- **LLM 候補数の増加**（現状 `min(n, 3)`）
