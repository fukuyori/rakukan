# Vibrato Phase 1 具体案

更新日: 2026-03-28

## 目的

ライブ変換および再変換時の分節境界を、TSF 側の単純なヒューリスティックではなく、
形態素解析ベースで初期提案できるようにする。

Phase 1 では ABI 互換性を維持し、既存の `Vec<String>` 候補返却を変更しない。
Vibrato は **engine 内部の補助 API** として導入し、`SplitPreedit` の初期ブロック生成にのみ使う。

## 方針

- `rakukan-engine` に Vibrato ベースの segmenter を追加する
- `surface` 文字列を分節し、`Vec<SegmentBoundary>` を返す
- TSF は候補文字列そのものではなく、engine が返した分節候補を `SplitBlock` へ変換する
- 既存 API
  - `convert_sync() -> Vec<String>`
  - `bg_take_candidates() -> Option<Vec<String>>`
  - `merge_candidates() -> Vec<String>`
  は変更しない

## 非目標

- ABI の `Vec<String>` を `Vec<Candidate>` に広げること
- LLM の候補生成ロジックそのものを置き換えること
- reading と surface の完全な文節対応を保証すること
- 辞書学習や候補順位に Vibrato を直接反映すること

## 変更対象

### 1. `crates/rakukan-engine/Cargo.toml`

追加候補:

```toml
[dependencies]
vibrato = "..."
```

辞書の扱いを考えると、必要なら以下も検討する。

- `include_bytes!` で小さな辞書を同梱
- インストーラで辞書を `%LOCALAPPDATA%\\rakukan\\dict\\` に配置
- 将来は mozc / user dict と別管理

### 2. 新規 `crates/rakukan-engine/src/segmenter.rs`

追加する責務:

- Vibrato tokenizer の lazy 初期化
- `surface` から分節境界列を返す
- 初期化失敗時は安全にフォールバック

想定インターフェース:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SegmentBoundary {
    pub surface: String,
}

pub struct Segmenter {
    // vibrato tokenizer / dictionary handle
}

impl Segmenter {
    pub fn new(...) -> anyhow::Result<Self>;
    pub fn segment_surface(&self, surface: &str) -> Vec<SegmentBoundary>;
}

pub fn segment_surface(surface: &str) -> Vec<SegmentBoundary>;
```

Phase 1 では `surface` だけ返せばよい。
reading 側の対応は TSF で既存ヒューリスティックを併用する。

### 3. `crates/rakukan-engine/src/lib.rs`

エンジン公開メソッドを追加する。

```rust
pub fn segment_surface(&self, surface: &str) -> Vec<String>;
```

実装方針:

- segmenter が使える場合は Vibrato 結果を返す
- 使えない場合は `vec![surface.to_string()]`
- 空文字は `vec![]`

### 4. `crates/rakukan-engine/src/ffi.rs`

FFI を 1 本追加する。

```rust
#[no_mangle]
pub extern "C" fn engine_segment_surface(
    handle: *mut c_void,
    surface: *const c_char,
) -> *mut c_char;
```

返却形式:

- JSON 配列 `["変換", "が"]`

Phase 1 では surface のみを返すので ABI は単純。

### 5. `crates/rakukan-engine-abi/src/lib.rs`

vtable と wrapper にメソッドを追加する。

```rust
segment_surface: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_char,
```

wrapper:

```rust
pub fn segment_surface(&self, surface: &str) -> Vec<String>;
```

### 6. `crates/rakukan-tsf/src/tsf/factory.rs`

現在の

- `split_surface_runs`
- `build_split_blocks_from_surface`

のうち、`surface` 側の分節候補取得を engine 呼び出しに置き換える。

想定変更:

```rust
fn build_split_blocks_from_surface(
    engine: &DynEngine,
    reading: &str,
    surface: &str,
    outer_remainder: &str,
) -> Vec<SplitBlock>
```

処理の流れ:

1. `engine.segment_surface(surface)` を呼ぶ
2. `Vec<String>` を `display block` として使う
3. `reading` 側は既存の `split_reading_by_weights` で対応付ける
4. segmenter 失敗時のみ現在の `split_surface_runs` へフォールバック

### 7. `crates/rakukan-tsf/src/engine/state.rs`

`SplitBlock { reading, display }` はそのままでよい。
Phase 1 では state の ABI 変更は不要。

## 実装ステップ

### Step 1

`rakukan-engine` に segmenter モジュールを追加し、単体で `surface -> Vec<String>` が取れるようにする。

### Step 2

FFI / ABI に `segment_surface()` を追加する。

### Step 3

`factory.rs` の `build_split_blocks_from_surface()` を engine 呼び出し対応に差し替える。

### Step 4

ライブ変換の

- `Selecting -> SplitPreedit`
- `LiveConv -> SplitPreedit`

の両経路で、Vibrato 境界が使われることを確認する。

## フォールバック方針

Vibrato が使えない場合でも IME が壊れないことを最優先とする。

優先順位:

1. Vibrato の分節結果
2. 既存 `split_surface_runs()` のヒューリスティック
3. 最悪 1 ブロック

## 確認項目

### 基本

- `変換が` → `["変換", "が"]`
- `東京都に` → `["東京都", "に"]`
- `学校へ行く` → `["学校", "へ", "行く"]`

### 再変換 UI

- Space 後に Left/Right で block 単位に境界が動く
- Shift+Left/Right も従来どおり同じ操作として使える
- Enter 確定で `reading` / `remainder` が壊れない

### フォールバック

- 辞書がない / 初期化失敗でも `build-tsf` と通常入力は継続する
- ログに fallback が出る

## ログ追加案

`factory.rs`

```rust
tracing::debug!(
    "[segment] reading={:?} surface={:?} blocks={:?}",
    reading, surface, blocks
);
```

`segmenter.rs`

```rust
tracing::info!("[vibrato] tokenizer ready");
tracing::warn!("[vibrato] fallback to heuristic: {err}");
```

## リスク

- Vibrato 辞書の配布サイズ
- 起動直後の初期化コスト
- surface の形態素境界と IME 再変換文節境界がずれるケース
- reading 側はまだ重み分配なので完全一致しない

## Phase 2 へのつなぎ

Phase 1 が安定したら、次で ABI を広げる。

```rust
pub struct CandidateSegment {
    pub reading: String,
    pub surface: String,
}

pub struct Candidate {
    pub surface: String,
    pub segments: Vec<CandidateSegment>,
}
```

これに進むと、TSF 側の重み分配をやめて、
engine が返す本当の文節情報で `SplitPreedit` を構築できる。
