# 変換パイプライン / 文節編集 再設計メモ

バージョン: draft-1
作成日: 2026-04-13
前提: v0.4.4 時点の rakukan コードベース
関連資料:
- [SEGMENT_EDIT_REDESIGN.md](SEGMENT_EDIT_REDESIGN.md) — 分節編集の基本方針（Segment 列を正とする）
- [VIBRATO_PHASE1.md](VIBRATO_PHASE1.md) — vibrato 形態素解析器の導入
- [DESIGN.md](DESIGN.md) — v0.4.4 時点の全体設計書
- Mozc: `src/converter/segments.h`, `src/converter/converter.cc`, `src/session/session_converter.cc`

---

## 1. 目的

ライブ変換後の文章を**文節単位で再変換**できる、破綻しない編集モデルを構築する。
設計思想は Mozc（Google 日本語入力の OSS 版）の `Segments` / `Converter` を参考にする。

### 実現する機能

| # | 機能 | 優先度 |
|---|---|---|
| 1 | ライブ変換後に Space で文節分割へ遷移 | A |
| 2 | 矢印キーで文節移動 | A |
| 3 | 選択文節の候補表示と再変換 | A |
| 4 | Shift+矢印で文節境界の伸縮（右側に伸びる/縮む） | A |
| 5 | 部分確定（focused 文節まで確定、以降はプリエディットに残す） | A |
| 6 | 文節単位の学習（選んだ候補を次回優先） | A |
| 7 | 数値保護（LLM が数字を改変しない） | A |
| 8 | ライブ変換は beam=1（greedy）、Space 時に beam=3 で候補拡張 | A |
| 9 | undo after commit（確定直後の Backspace で戻す） | B |
| 10 | 候補ウィンドウの一覧展開（Tab で N → 3N 候補） | B |
| 11 | F6〜F10 の文節単位版 | B |
| 12 | ユーザー辞書登録 | C |

優先度 A を本設計の主対象、B を後続フェーズ、C は将来検討。

### 非目標

- Suggestion / Prediction / Conversion の 3 分割（Mozc 流の完全パイプライン）
- 完全な Mozc 互換（rakukan は LLM 変換主体、Mozc は辞書 + 統計主体）
- 既存のキーマップ / config 互換を壊さないこと（移行期は旧 API も残す）

---

## 2. 現状分析

### 2.1 既にあるもの

#### engine 側

**`rakukan-engine/src/segmenter.rs`**

```rust
pub struct SegmentBlock {
    pub surface: String,
    pub reading: String,
}

pub struct SegmentCandidate {
    pub surface: String,
    pub segments: Vec<SegmentBlock>,
}

pub fn segment_candidate(surface: &str, reading: &str) -> Vec<SegmentBlock>;
pub fn segment_candidates(reading: &str, candidates: &[String]) -> Vec<SegmentCandidate>;
```

- vibrato + DP で分節化
- ライブ変換結果（surface）と読み（reading）から文節列を生成可能
- RPC (`rakukan-engine-rpc`) でも `bg_take_segmented_candidates` / `segment_candidate` / `convert_sync_segmented` として公開済み

**`rakukan-engine/src/kanji/backend.rs`**

- `convert(reading, context, num_candidates)` が `num_candidates == 1` で greedy、`> 1` で beam（`beam_size = num_candidates.min(3)`）に自動分岐
- ライブ変換の n=1 提案はこの分岐を活かすだけで済む

#### TSF 側

**`rakukan-tsf/src/engine/state.rs::SessionState`**

```rust
enum SessionState {
    Idle,
    Preedit { text: String },
    Waiting { text, pos_x, pos_y },
    SplitPreedit { conversion: ConversionState },
    Selecting {
        original_preedit, candidates, structured_candidates,
        selected, page_size, llm_pending, pos_x, pos_y,
        punct_pending, prefix, prefix_reading, remainder, remainder_reading,
        split_prefix_blocks, split_suffix_blocks,
    },
    LiveConv { reading: String, preview: String },
}

struct ConversionState {
    segments: Vec<ConversionSegment>,  // reading + surface
    focused_index: usize,
    candidate_view: Option<ConversionCandidateView>,  // セッション全体に 1 つ
}

struct SplitBlock { reading: String, display: String }  // 旧データ構造
```

- `SplitPreedit` / `LiveConv` / `Selecting` が並存
- `ConversionState` は Mozc の `Segments` に近いが、候補リストと選択状態がセッション全体で 1 つ（文節ごとに持たない）
- `SplitBlock` と `ConversionSegment` がほぼ同じ内容で二重管理

### 2.2 問題点（SEGMENT_EDIT_REDESIGN.md で既に指摘）

1. **文字列再構築が複数経路に散在**
   - `build_split_blocks_from_surface(engine, reading, surface, suffix)` を `LiveConv → SplitPreedit` 遷移で毎回呼ぶ
   - `rebuild_split_blocks_from_selection(...)` を `Selecting → SplitPreedit` 遷移でまた呼ぶ
   - 2 経路が別々のロジックで文字列を切り直す

2. **データ構造の二重管理**
   - `SplitBlock { reading, display }` と `ConversionSegment { reading, surface }` が並存
   - 相互変換コードが散らばる

3. **`Selecting` と `SplitPreedit` で別経路**
   - `Selecting` は `split_prefix_blocks` / `split_suffix_blocks` を抱えて文字列的に前後を復元
   - `SplitPreedit` は `ConversionState` を使う
   - 行き来のたびに変換コードが走る

4. **候補リストが文節ごとに持てない**
   - `ConversionState.candidate_view` はセッション全体で 1 つ
   - 矢印キーで文節を移動しても、各文節の選択状態が保存されない

5. **境界伸縮が TSF 側で文字列を切り直している**
   - `on_segment_grow` / `on_segment_shrink` が TSF 側で `build_split_blocks_from_surface` を再呼び出し
   - 左側固定の不変条件がコード上で保証されない

6. **ライブ変換で 9 候補を無駄に生成**
   - `bg_start(n_cands)` が毎キーストローク 9 候補要求
   - 実際の表示は 1 位のみ
   - LLM の計算量が 3 倍（beam_size = min(9,3) = 3）

7. **数値がLLM に改変される**
   - `2024ねん` → `2025年` のように学習データ由来の hallucination
   - 現状はプロンプトでもガードがない

---

## 3. 新データモデル

Mozc の `Segments` / `Segment` / `Candidate` を参考に、rakukan 用に最小限に簡略化する。

### 3.1 型定義

```rust
// rakukan-engine-abi にも同型を export し、rakukan-engine-rpc の protocol にも載せる

/// 1 つの候補。Mozc の Candidate に相当。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    /// 表記（漢字 or ひらがな or カタカナ or 英数）
    pub surface: String,
    /// 候補のソース（辞書 / LLM / 履歴 / 記号 など）
    pub source: CandidateSource,
    /// 注釈（同音異義語の用途説明など、将来拡張）
    pub annotation: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CandidateSource {
    Llm,
    Dict,
    History,
    Digit,    // 数値保護経由で挿入された候補
    Literal,  // 読みそのまま
}

/// 1 つの文節。Mozc の Segment に相当。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segment {
    /// 読み（ひらがな）
    pub reading: String,
    /// この文節の候補リスト（先頭がデフォルト）
    pub candidates: Vec<Candidate>,
    /// 現在選択中の候補インデックス
    pub selected: usize,
    /// 固定済みフラグ（true の場合は再変換しない）
    pub fixed: bool,
}

/// 文節列全体。Mozc の Segments に相当。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Segments {
    /// 文節列。履歴領域 (history) + 変換領域 (conversion) を 1 本で持つ
    pub segments: Vec<Segment>,
    /// 履歴領域の長さ。`segments[..history_size]` は確定済みで touch しない
    pub history_size: usize,
    /// 矢印キーで移動する対象の index（conversion 領域内）
    pub focused: usize,
}
```

### 3.2 不変条件

実装中、以下を常に守る:

1. `segments[..history_size]` は **読み込み専用**。再変換・再分節しない。
2. `focused >= history_size` かつ `focused < segments.len()`
3. 各 `Segment.candidates` は少なくとも 1 つの要素を持つ（空にしない）
4. `Segment.selected < Segment.candidates.len()`
5. 表示される全体 surface は `segments.iter().map(|s| s.candidates[s.selected].surface.as_str()).collect::<String>()`
6. `Segments` 全体の `reading` は `segments.iter().map(|s| s.reading.as_str()).collect::<String>()`

### 3.3 Mozc との差分

| Mozc | rakukan | 備考 |
|---|---|---|
| `Segments` | `Segments` | ほぼ同じ |
| `Segment::key_` | `Segment::reading` | 名前だけ変更 |
| `Segment::candidates_` | `Segment::candidates` | 同じ |
| `Segment::segment_type_` (FREE / FIXED_BOUNDARY / FIXED_VALUE / ...) | `Segment::fixed: bool` のみ | rakukan はシンプルに |
| `Candidate::value_` | `Candidate::surface` | 名前だけ変更 |
| `Candidate::content_key/value_` | なし | 付属語分離は rakukan では不要 |
| `Candidate::attributes_` (USER_HISTORY, SPELLING_CORRECTION, ...) | `CandidateSource` enum | シンプル化 |
| `Segments::history_segments_size_` | `Segments::history_size` | 同じ |
| `ConversionRequest` | 引数の組 | ラッパー構造体は作らない |

---

## 4. 機能別フロー

### 4.1 ライブ変換（機能 8）

**現状**: `on_input` で `bg_start(num_candidates)` → BG タイマーで `bg_take_candidates` → preview として表示

**新設計**:

```
on_input (キーストローク)
  ↓
engine.input_char(c, kind, bg_start_n_cands=Some(1))
  ↓ (ホスト側)
eng.push_char(c)
eng.bg_start(1)  // ← beam=1 で greedy 決定
  ↓ (BG 推論完了)
WM_TIMER → bg_take_candidates(hiragana) → 1 個の surface
  ↓
candidate_window::on_live_timer → Segments 生成
  Segments {
    segments: vec![  // ライブ変換段階では vibrato で分節化済み
      Segment { reading, candidates: [surface], selected: 0, fixed: false },
      ...
    ],
    history_size: 0,
    focused: 0,
  }
  ↓
SessionState::LiveConv { segments }  ← 文字列 preview ではなく Segments を持つ
  ↓
update_composition(表示用 surface)
```

**ポイント**:
- ライブ変換中に既に vibrato で文節分割した `Segments` を保持
- Space 押下時に文字列からの再分節は不要になる
- `bg_start(1)` により LLM は greedy decoding、レイテンシ大幅減

### 4.2 Space で文節分割へ遷移（機能 1）

**現状**: `on_convert` が `LiveConv` を検出 → `build_split_blocks_from_surface` で再分節 → `SplitPreedit`

**新設計**:

```
on_convert (Space 押下)
  ↓
if let LiveConv { segments } = current state {
    // 各文節の候補を拡張（デフォルトは 1 つだけ持っている）
    for seg in &mut segments.segments {
        if seg.candidates.len() < num_candidates {
            // engine から追加候補を取得
            let extra = engine.segment_candidates(&seg.reading, num_candidates);
            merge_candidates(&mut seg.candidates, extra);
        }
    }
    segments.focused = segments.history_size;  // 先頭文節を focused に
    SessionState::SplitPreedit { segments }
}
```

**ポイント**:
- ライブ変換時に持っていた `Segments` をそのまま move するだけ
- 追加候補取得は文節ごとに個別に `engine.segment_candidates(reading, N)` を呼ぶ
- 全文節を並列取得できる（engine 側で batch 化）

### 4.3 矢印キーで文節移動（機能 2）

```
on_move_segment_right:
  if segments.focused < segments.segments.len() - 1 {
    segments.focused += 1;
    // 候補ウィンドウを新 focused 文節用に更新
    show_candidates_for(&segments.segments[segments.focused]);
  }

on_move_segment_left:
  if segments.focused > segments.history_size {
    segments.focused -= 1;
    show_candidates_for(&segments.segments[segments.focused]);
  }
```

**ポイント**:
- `Segment.selected` は各文節ごとに保持されているので、移動しても前の選択は残る
- 候補ウィンドウ表示は `Segment.candidates` と `Segment.selected` をそのまま渡す

### 4.4 選択文節の候補表示と再変換（機能 3）

```
on_candidate_select (数字キー / 矢印下 / 矢印上):
  let seg = &mut segments.segments[segments.focused];
  seg.selected = new_index;
  // fixed は false のまま（ユーザーが別候補を選んだだけ）
  update_composition(render_segments(&segments));
```

**拡張候補取得**:

```
on_expand_candidates (Tab or 候補ウィンドウ末尾到達):
  let seg = &mut segments.segments[segments.focused];
  if seg.candidates.len() < MAX_CANDIDATES {
    let more = engine.segment_candidates(&seg.reading, MAX_CANDIDATES);
    merge_candidates(&mut seg.candidates, more);
  }
```

### 4.5 Shift+矢印で境界伸縮（機能 4）

**基本仕様**:

- `Shift+Right` で対象文節の **右端の境界を 1 文字右に動かす**（= 対象文節を 1 文字伸ばす）
- `Shift+Left` で対象文節の **右端の境界を 1 文字左に動かす**（= 対象文節を 1 文字縮める）
- 単位は常に **読み（ひらがな）の 1 文字ずつ**。文節単位ではない
- 連打すれば 2 文字、3 文字と伸縮する。次の文節の境界を跨いでもそのまま 1 文字ずつ進む
- 動くのは「対象文節の右端」のみ。左端（= 対象文節の開始位置）は動かない
- 左側の文節列 `segments[..focused]` は一切変化しない（不変条件）
- 伸ばす場合は次文節の先頭 1 文字を食う。次文節が 1 文字しか残っていないなら次文節は消滅し、さらにその次の文節の先頭 1 文字を食うようになる
- 縮める場合は対象文節の末尾 1 文字が外れ、次文節の先頭に付加される（または新しい 1 文字文節として挿入される）

**具体例**: `やまがき` と入力してライブ変換した結果が `やま / が / き` の 3 文節に分かれてしまい、ユーザーは `やまがき`（1 つの固有名詞）として扱いたい場合

```
初期状態:
  segments:  [ やま(2文字) | が(1文字) | き(1文字) ]
  focused:   0   ← "やま" が選択されている

Shift+Right 1 回目:
  対象文節 "やま" の右端を 1 文字右に動かす
  → "やま" の読みが 3 文字になる = 次文節 "が" の先頭 "が" を食う
  → 次文節 "が" は 0 文字になったので消滅
  segments:  [ やまが(3文字) | き(1文字) ]
  focused:   0
  (engine は "やまが" + "き" を新しい reading として再変換)

Shift+Right 2 回目:
  対象文節 "やまが" の右端をさらに 1 文字右に動かす
  → "やまが" の読みが 4 文字になる = 次文節 "き" の "き" を食う
  → 次文節 "き" は 0 文字になったので消滅
  segments:  [ やまがき(4文字) ]
  focused:   0
  (engine は "やまがき" を 1 つの文節として再変換 → "山柿" など)
```

縮める場合:

```
初期状態:
  segments:  [ やまがき(4文字) ]
  focused:   0

Shift+Left 1 回目:
  対象文節 "やまがき" の右端を 1 文字左に動かす
  → "やまがき" の読みが 3 文字になる = 末尾 "き" が外れる
  → 外れた "き" は新しい文節として挿入
  segments:  [ やまが(3文字) | き(1文字) ]
  focused:   0
```

**要点**:

- この動作は文節単位ではなく **ひらがな 1 文字単位**
- 連打するとスムーズに境界が右へ（または左へ）動く
- 次文節が 1 文字だったら連鎖的に消滅するし、対象文節が長くなれば vibrato の再分節で別の形に切り直されることもある
- この「次文節の境界を跨ぐ」挙動のおかげで、`やま / が / き` → `やまがき` のような過剰分割の修正が Shift+矢印だけでできる

**現状の問題**: `on_segment_grow` / `on_segment_shrink` が TSF 側で文字列レベル再分節をしていて、左側の文節が巻き込まれて壊れるケースがある

**新設計**: engine 側に `resize_segment` API を追加し、伸縮ロジックを 1 箇所に集約

```rust
// rakukan-engine
pub fn resize_segment(
    &self,
    segments: &Segments,
    index: usize,   // 対象文節のインデックス（focused）
    offset: i32,    // +1 = 1 文字伸ばす / -1 = 1 文字縮める
    num_candidates: usize,
) -> Segments {
    // 1. segments[..index] はそのままコピー（不変条件）
    // 2. segments[index] の新しい reading を算出
    //    - offset=+1: 次文節 reading の先頭 1 文字を対象の末尾に付け足す
    //    - offset=-1: 対象 reading の末尾 1 文字を外す
    // 3. segments[index..] の「右側全体の reading」を再連結
    //    - new_right_reading = new_focused_reading + 次文節以降の残り reading
    // 4. new_right_reading を vibrato + 数値保護 + LLM で再分節 + 再変換
    // 5. 左側（touch しない） + 新右側を結合して新しい Segments を返す
}
```

**境界条件**:

- `offset=+1` で次文節が存在しない、または次文節が 1 文字だけ → そのまま結合して次文節が消える（merge 動作）
- `offset=-1` で対象文節が 1 文字だけ → 何もしない（これ以上縮められない）
- 対象文節が数字ラン由来で `fixed: true` → 何もしない
- 対象文節の次が数字ラン（`fixed: true`）で食おうとする → 何もしない（数字は動かさない）

**TSF 側**:

```rust
on_segment_grow:  // Shift+Right
  if segments.segments[segments.focused].fixed { return; }
  let new_segments = engine.resize_segment(
      &segments,
      segments.focused,
      +1,
      convert_beam_size,
  );
  segments = new_segments;
  update_composition(render_segments(&segments));

on_segment_shrink:  // Shift+Left
  if segments.segments[segments.focused].fixed { return; }
  let new_segments = engine.resize_segment(
      &segments,
      segments.focused,
      -1,
      convert_beam_size,
  );
  segments = new_segments;
  update_composition(render_segments(&segments));
```

**Mozc 参考**: `converter/converter.cc::Converter::ResizeSegment` の設計思想（左側固定・右側再計算・offset は読み文字数ベース）を参考にしつつ、Rust で独自実装する。

### 4.6 部分確定（機能 5）

Ctrl+Enter または Shift+Enter で「現在の focused までを確定、それ以降をライブ変換の新プリエディットに戻す」:

```rust
on_partial_commit:
  let commit_end = segments.focused + 1;
  let commit_text: String = segments.segments[..commit_end]
      .iter()
      .map(|s| s.candidates[s.selected].surface.as_str())
      .collect();
  let remaining_reading: String = segments.segments[commit_end..]
      .iter()
      .map(|s| s.reading.as_str())
      .collect();

  // 学習
  for seg in &segments.segments[..commit_end] {
      let cand = &seg.candidates[seg.selected];
      if cand.source != CandidateSource::Literal {
          engine.learn(&seg.reading, &cand.surface);
      }
  }

  engine.commit(&commit_text);
  engine.reset_preedit();
  // remaining_reading をエンジンに再入力してライブ変換に戻す
  for c in remaining_reading.chars() {
      engine.push_raw(c);
  }
  // ライブ変換を再起動
  engine.bg_start(1);
  SessionState::LiveConv { ... }
```

**ポイント**:
- Mozc の `CommitSegments` は確定分を `history_size` に移すだけだが、rakukan は engine を常にリセット前提にするほうがシンプル
- 残りをライブ変換に戻すことで、ユーザーは「直した所より後ろ」をそのまま続けて直せる

### 4.7 文節単位の学習（機能 6）

```rust
// 全体確定時
on_commit:
  for seg in &segments.segments {
      let cand = &seg.candidates[seg.selected];
      if cand.source != CandidateSource::Literal {
          engine.learn(&seg.reading, &cand.surface);
      }
  }
```

**ポイント**:
- 現状の `engine.learn(reading, surface)` をそのまま使う
- rakukan-engine 側の学習データが「reading → surface → 頻度」の形になっているので、次回同じ reading がライブ変換されたときにその surface がトップに来る
- 学習データの優先度は既存の `merge_candidates` に組み込む

---

## 5. 分節化レイヤー

vibrato の生出力をそのまま Segments に流し込むと形態素単位（過剰分割）になってしまうため、**文節構成ルール**で助詞・助動詞を自立語に結合し、さらに**数値保護**で数字ランを固定文節として切り出す。この 2 つの前処理を経て初めて Segments が作られる。

### 5.1 文節構成ルール（bunsetsu composition）

#### 5.1.1 背景

vibrato は **形態素解析器** なので、出力は morpheme 単位で細かく切れる:

```
入力: "わたしはがっこうにいきます"
vibrato 出力: [わたし | は | がっこう | に | いき | ます]   ← 6 形態素
```

一方、ユーザーが期待する分節（bunsetsu）は:

```
[わたしは | がっこうに | いきます]   ← 3 文節
```

形態素単位のまま Segments にすると:

- **文節数が多すぎて矢印移動が面倒**（6 文節を移動するのと 3 文節を移動するのではユーザー体感が大きく違う）
- **助詞単体に focus が当たっても再変換の意味がない**（「は」「が」「に」に候補を出しても無意味）
- **文節の意味的な塊がユーザーの直感と合わない**
- **Mozc など他の主要 IME と挙動が違う**ので、既存の日本語入力ユーザーが戸惑う

したがって、vibrato の morpheme 出力を bunsetsu 単位に結合する後処理が必要。

#### 5.1.2 結合ルール

文節 (bunsetsu) は以下の構造を持つ:

```
文節 = 自立語(1 個) + 付属語(0 個以上)
```

- **自立語 (jiritsugo)**: 名詞 / 動詞 / 形容詞 / 形容動詞 / 副詞 / 連体詞 / 接続詞 / 感動詞
- **付属語 (fuzokugo)**: 助詞 / 助動詞
- **接頭辞 / 接尾辞**: 隣接する自立語に結合する（接頭辞は後続、接尾辞は前方）

アルゴリズム:

```rust
fn compose_bunsetsu(morphemes: &[Morpheme]) -> Vec<Bunsetsu> {
    let mut result = Vec::new();
    let mut current: Option<Bunsetsu> = None;

    for m in morphemes {
        match m.pos {
            Pos::Particle | Pos::AuxVerb => {
                // 付属語: 現 bunsetsu に追加
                if let Some(ref mut b) = current {
                    b.push(m);
                } else {
                    // 先頭が付属語（通常は起こらない、エラー系）
                    current = Some(Bunsetsu::new(m));
                }
            }
            Pos::Suffix => {
                // 接尾辞: 前の形態素に付ける
                if let Some(ref mut b) = current {
                    b.push(m);
                } else {
                    current = Some(Bunsetsu::new(m));
                }
            }
            Pos::Prefix => {
                // 接頭辞: 新しい bunsetsu を開始し、次の自立語と結合させる
                if let Some(b) = current.take() {
                    result.push(b);
                }
                current = Some(Bunsetsu::new(m));
                // 次のトークンが自立語ならそれもこの bunsetsu に結合
            }
            _ => {
                // 自立語: 新しい bunsetsu を開始
                if let Some(b) = current.take() {
                    result.push(b);
                }
                current = Some(Bunsetsu::new(m));
            }
        }
    }
    if let Some(b) = current {
        result.push(b);
    }
    result
}
```

#### 5.1.3 vibrato の POS タグへのマッピング

vibrato は UniDic 系の辞書を使っているので、POS は `品詞大分類-品詞中分類-...-` の階層形式で得られる。rakukan 側で使う簡略化マッピング:

| vibrato の POS 大分類 | rakukan の `Pos` | 備考 |
|---|---|---|
| 名詞 | `Noun`（自立語扱い）| 数詞は別途数値保護で拾う |
| 動詞 | `Verb`（自立語扱い）| |
| 形容詞 | `Adjective`（自立語扱い）| |
| 副詞 | `Adverb`（自立語扱い）| |
| 連体詞 | `Adnominal`（自立語扱い）| |
| 接続詞 | `Conjunction`（自立語扱い）| |
| 感動詞 | `Interjection`（自立語扱い）| |
| 助詞 | `Particle`（付属語）| |
| 助動詞 | `AuxVerb`（付属語）| |
| 接頭辞 | `Prefix` | 次の自立語に結合 |
| 接尾辞 | `Suffix` | 前の形態素に結合 |
| 記号 | `Symbol` | 独立した bunsetsu |
| その他 | `Other` | 独立した bunsetsu 扱い |

#### 5.1.4 具体例

```
入力: "わたしはがっこうにいきます"

Step 1 (vibrato 形態素):
  [わたし(名詞) | は(助詞) | がっこう(名詞) | に(助詞) | いき(動詞) | ます(助動詞)]

Step 2 (文節構成):
  [わたし + は] → "わたしは" (自立語 + 付属語)
  [がっこう + に] → "がっこうに"
  [いき + ます] → "いきます"

結果: [わたしは | がっこうに | いきます]   ← 3 bunsetsu
```

```
入力: "やまがきがすきだ"

Step 1 (vibrato 形態素):
  [やまがき(名詞・固有名詞)? | が(助詞) | すき(名詞) | だ(助動詞)]
  もしくは
  [やま(名詞) | が(助詞) | き(名詞) | が(助詞) | すき(名詞) | だ(助動詞)]
  ← vibrato が固有名詞として認識するかは辞書次第

Step 2 (文節構成):
  前者の場合: [やまがき + が | すき + だ] = [やまがきが | すきだ]
  後者の場合: [やま + が | き + が | すき + だ] = [やまがが | きが | すきだ]

後者の場合でも、Shift+Right を 2 回押せば "やまがが" → "やまがきが" → "やまがきが"（自立語 "やまがき" + 助詞 "が"）と修正できる
```

#### 5.1.5 Shift+矢印との関係

§4.5 で述べた「ひらがな 1 文字単位の伸縮」は、この文節構成ルールの **あと** に適用される。つまり:

1. vibrato で形態素分割
2. 文節構成ルールで bunsetsu に結合
3. 数値保護で数字ランを独立 bunsetsu に
4. この結果が `Segments` となる
5. ユーザーが Shift+Right/Left を押したら、1 文字ずつ境界を動かし、**その新しい境界で再度 vibrato + 文節構成を実行**して `Segments` を更新

つまり `resize_segment` の内部では、新しい right-side reading を vibrato に通し直し、再度 bunsetsu 構成を適用する。ユーザーの「1 文字伸ばす」操作が結果的に「次の助詞まで飲み込む」動作につながる。

#### 5.1.6 結合を抑制するケース

常に助詞を結合すると「文節境界を助詞の前で切りたい」ユーザーの意図を無視してしまうが、その場合は **Shift+Left で境界を戻せる**ので結合側をデフォルトにする。これは Mozc と同じ方針。

### 5.2 数値保護レイヤー

#### 5.2.1 背景

LLM は学習データの頻出パターンに引きずられ、数字を改変することがある:

- `2024ねん` → `2025年`（年号ドリフト）
- `3じ15ふん` → `3時30分`（時刻の正規化）
- `500えん` → `５００円`（全角化は意図的だが数字は合う、OK）
- `100％` → `千％`（桁変化、論外）

#### 5.2.2 対策: 入力分割 + 非数字部だけを LLM に

reading を「数字ラン」と「非数字ラン」に分割し、**LLM には非数字部分だけを渡す**。LLM 出力後に元の数字ランを再挿入する。

```
reading: "2024ねん4がつ10にち"
         ↓ split_by_digits
[Digit("2024"), Kana("ねん"), Digit("4"), Kana("がつ"), Digit("10"), Kana("にち")]
         ↓ 非数字ランのみ LLM に渡す
         ↓ ただしコンテキストとして周辺の数字ランも参考情報として渡す
         ↓
[Digit("2024"), Converted("年"), Digit("4"), Converted("月"), Digit("10"), Converted("日")]
         ↓ concat
"2024年4月10日"
```

#### 5.2.2 API

```rust
// rakukan-engine
pub fn convert_with_digit_protection(
    &self,
    reading: &str,
    context: &str,
    num_candidates: usize,
) -> Result<Vec<String>> {
    let runs = split_by_digits(reading);
    if runs.iter().all(|r| !r.is_digit()) {
        // 数字なし: 既存パス
        return self.convert(reading, context, num_candidates);
    }
    // 数字あり: 非数字ランだけ LLM、数字ランは原文そのまま
    let mut run_candidates: Vec<Vec<String>> = Vec::with_capacity(runs.len());
    for (i, run) in runs.iter().enumerate() {
        match run {
            Run::Digit(s) => {
                // 数字は 1 候補のみ（半/全/漢数字の変換はユーザーが F 系キーで別途）
                run_candidates.push(vec![s.clone()]);
            }
            Run::Kana(s) => {
                // 周辺 run をプロンプトのコンテキストに含める
                let local_context = build_local_context(&runs, i, context);
                let cands = self.convert(s, &local_context, num_candidates)?;
                run_candidates.push(cands);
            }
        }
    }
    Ok(combine_runs(&run_candidates, num_candidates))
}

enum Run {
    Digit(String),  // [0-9]+
    Kana(String),   // それ以外（ひらがな/カタカナ/記号）
}

fn split_by_digits(reading: &str) -> Vec<Run>;
fn combine_runs(runs: &[Vec<String>], limit: usize) -> Vec<String>;  // デカルト積 → 上位 limit 個
```

#### 5.2.3 後処理検証（保険）

A 案の実装バグに備えて、最終出力の数字一致を検証:

```rust
fn verify_digits_preserved(input: &str, output: &str) -> bool {
    let in_digits: String = input.chars().filter(|c| c.is_ascii_digit()).collect();
    let out_digits: String = output.chars().filter(|c| c.is_ascii_digit()).collect();
    in_digits == out_digits
}

// convert_with_digit_protection の末尾で:
let verified: Vec<String> = candidates
    .into_iter()
    .filter(|c| verify_digits_preserved(reading, c))
    .collect();
```

検証で落ちた候補はリストから除外。全部落ちたらフォールバックで原文を返す。

#### 5.2.4 文節分割との統合

`segment_candidate(surface, reading)` は既に vibrato ベースだが、数字ランは vibrato の結果に関係なく **独立した 1 文節として強制的に扱う**:

```rust
pub fn segment_with_digit_protection(reading: &str, surface: &str) -> Vec<Segment> {
    let reading_runs = split_by_digits(reading);
    let surface_runs = split_by_digits(surface);
    // reading と surface の run 数は一致するはず（数字保護で数字は改変されないため）
    assert_eq!(reading_runs.len(), surface_runs.len());

    let mut segments = Vec::new();
    for (r_run, s_run) in reading_runs.iter().zip(surface_runs.iter()) {
        match r_run {
            Run::Digit(d) => {
                // 数字ランは必ず 1 文節
                segments.push(Segment {
                    reading: d.clone(),
                    candidates: vec![Candidate {
                        surface: s_run.as_str().to_string(),
                        source: CandidateSource::Digit,
                        annotation: None,
                    }],
                    selected: 0,
                    fixed: true,  // 数字文節は固定
                });
            }
            Run::Kana(k) => {
                // 非数字ランは vibrato でさらに分節化
                let sub_segments = vibrato_segment(k, s_run.as_str());
                segments.extend(sub_segments);
            }
        }
    }
    segments
}
```

**ポイント**: 数字文節は `fixed: true` にして Shift+矢印での境界伸縮の対象外にする。

---

## 6. engine / RPC / TSF の API 変更

### 6.1 rakukan-engine-abi

新規型:
- `Segments`, `Segment`, `Candidate`, `CandidateSource`
- `Run` は engine 内部のみ（公開しない）

### 6.2 rakukan-engine

新規関数:
- `fn convert_with_digit_protection(&self, reading, context, num_candidates) -> Result<Vec<String>>`
- `fn segment_with_digit_protection(reading, surface) -> Vec<Segment>`
- `fn to_segments(&self, reading, context, num_candidates) -> Result<Segments>` — 読み全体から Segments を生成（ライブ変換結果の 1 位 + 非数字ランごとの候補）
- `fn segment_candidates(&self, reading, context, num_candidates) -> Result<Vec<Candidate>>` — 1 文節分の候補だけ取得
- `fn resize_segment(&self, segments, index, offset, num_candidates) -> Segments` — 境界伸縮

既存 `convert(reading, context, num_candidates)` は内部的に `convert_with_digit_protection` を呼ぶように変更（後方互換）。

### 6.3 rakukan-engine-rpc

`PROTOCOL_VERSION` を 3 に bump。新規 Request / Response:

```rust
// Request 末尾に追加
ConvertToSegments { reading: String, context: String, num_candidates: u32 },
ResizeSegment { segments_json: String, index: u32, offset: i32, num_candidates: u32 },
SegmentCandidatesRequest { reading: String, context: String, num_candidates: u32 },

// Response 末尾に追加
Segments(Vec<SegmentDto>),  // DTO は SegmentBlock と互換の型
```

`Segments` / `Segment` / `Candidate` は serde 対応済みなので postcard でそのまま送れる（JSON にする必要はない）。

### 6.4 rakukan-tsf

`SessionState` の再編:

```rust
enum SessionState {
    Idle,
    Preedit { text: String },
    Waiting { ... },
    LiveConv { segments: Segments },           // ← 構造化
    SplitPreedit { segments: Segments },       // ← 構造化
    // Selecting は廃止（SplitPreedit に統合、文節 1 個のケースも SplitPreedit で扱う）
}
```

旧 `ConversionState` / `SplitBlock` / `ConversionSegment` は Phase D で削除。

---

## 7. Mozc 参考箇所の対応表

| rakukan での対応 | Mozc の参考箇所 | 備考 |
|---|---|---|
| `Segments` 型 | `converter/segments.h::Segments` | `history_segments_size_` → `history_size` |
| `Segment` 型 | `converter/segments.h::Segment` | `key_` → `reading`、`candidates_` → `candidates`、`selected_index_` → `selected` |
| `Candidate` 型 | `converter/segments.h::Segment::Candidate` | `value_` → `surface`、`attributes_` → `source` |
| `convert_to_segments` | `converter/converter.cc::Converter::StartConversion` | LLM 主体なので rakukan は簡略化 |
| `resize_segment` | `converter/converter.cc::Converter::ResizeSegment` | offset は読みの文字数、左側固定 |
| 部分確定 | `converter/converter.cc::Converter::CommitSegments` | rakukan は engine を毎回リセットする方針 |
| 文節単位の学習 | `prediction/user_history_predictor.cc::UserHistoryPredictor::Finish` | 既存の `engine.learn` を流用 |
| 矢印キー処理 | `session/session_converter.cc::SessionConverter::SegmentFocusRight` | focused インデックスの移動のみ |
| Shift+矢印 | `session/session_converter.cc::SessionConverter::SegmentWidthExpand` | ResizeSegment を呼ぶだけ |
| 数値保護 | Mozc には専用レイヤーなし | LLM 特有の問題、rakukan 独自 |

Mozc は Apache 2.0 + BSD。参考コードとして読むのは自由、ロジック移植時は `THIRD_PARTY_LICENSES.md` に追記。

---

## 8. 段階的移行計画

### Phase A: 新データモデルと engine 基盤（1〜2 週）

**目的**: 新型を導入し、既存コードと並存させる。破壊的変更は行わない。

作業:
1. `rakukan-engine-abi` に `Segments` / `Segment` / `Candidate` / `CandidateSource` 型を追加
2. `rakukan-engine` に下記を実装:
   - `split_by_digits(reading) -> Vec<Run>`
   - `convert_with_digit_protection(reading, context, n) -> Vec<String>`
   - `verify_digits_preserved(input, output) -> bool`
   - 既存 `convert` を `convert_with_digit_protection` に差し替え
3. 数値保護のユニットテスト（年月日 / 時刻 / 金額 / 単位 / 連続数字 / 境界ケース）
4. `segment_with_digit_protection(reading, surface) -> Vec<Segment>` を実装
5. `convert_to_segments(reading, context, n) -> Segments` を実装（ライブ変換結果を文節分割して Segments 化）
6. RPC に `Request::ConvertToSegments` / `Response::Segments` を追加、`PROTOCOL_VERSION` を 3 に bump

**完了条件**:
- 既存機能が全て動く（`cargo check --workspace` OK、実機で回帰なし）
- 数値を含むライブ変換で数字が改変されない
- 新 API が RPC 経由で呼べる（テストツールで確認）

### Phase B: ライブ変換の n=1 化と Segments 保持（3〜5 日）

**目的**: ライブ変換が `Segments` を保持、LLM 計算量を削減。

作業:
1. `SessionState::LiveConv` を `{ segments: Segments }` に変更
2. `on_input` の `bg_start(n_cands)` を `bg_start(1)` に変更
3. BG タイマー完了時に `convert_to_segments` を呼んで `Segments` を生成
4. `update_composition` で Segments から surface を組み立てて表示
5. 旧 `LiveConv { reading, preview }` 参照を全撤去

**完了条件**:
- ライブ変換のキーストロークごとのレイテンシが現行の 1/3 以下になる（計測）
- ライブ変換の表示は従来通り
- Space を押さなければ SplitPreedit には入らない

### Phase C: SplitPreedit を新モデルに（1〜2 週）

**目的**: 文節再変換の中核を実装。

作業:
1. `SessionState::SplitPreedit` を `{ segments: Segments }` に変更
2. `on_convert` の `LiveConv → SplitPreedit` 遷移を、Segments を move するだけに簡略化
3. 各文節の候補拡張:
   - Space 押下時に全文節に対して `engine.segment_candidates(reading, N)` を並列呼び出し
   - `Segment.candidates` に格納
4. 矢印キー処理 (`on_move_segment_left/right`): `segments.focused` を移動
5. 候補選択処理: `segments.segments[focused].selected` を更新
6. 候補ウィンドウ表示を `Segment.candidates` / `Segment.selected` ベースに書き換え
7. 全体確定 (`on_commit`) で Segments から surface を組み立てて engine に渡し、各文節を `engine.learn`

**完了条件**:
- ライブ変換 → Space → 矢印キーで文節移動 → 候補選択 → Enter で確定、の一連の流れが動く
- 旧 `Selecting` / `SplitPreedit` が並存するが新実装は `SplitPreedit` のみ使う

### Phase D: 境界伸縮を engine に寄せる（1 週）

**目的**: Shift+矢印を engine の `resize_segment` で実装。

作業:
1. `rakukan-engine` に `resize_segment(&self, segments, index, offset, n) -> Segments` を実装
2. RPC に `Request::ResizeSegment` を追加
3. TSF の `on_segment_grow` / `on_segment_shrink` を `engine.resize_segment` 呼び出しに差し替え
4. 旧 `build_split_blocks_from_surface` / `rebuild_split_blocks_from_selection` を削除
5. 数字文節 (`fixed: true`) は境界調整の対象外にする

**完了条件**:
- Shift+Right / Shift+Left で文節境界が正しく伸縮する
- 左側の文節が絶対に壊れない（ユニットテストで検証）
- 旧文字列ベースの再分節コードが完全に削除

### Phase E: 追加機能（部分確定・学習・Selecting 統合・Tab 展開）（1〜1.5 週）

作業:
1. **部分確定**: `on_partial_commit`（Ctrl+Enter）を実装
2. **学習**: `on_commit` / `on_partial_commit` で `engine.learn` を呼ぶ
3. **Selecting 統合**: 旧 `SessionState::Selecting` を削除、`SplitPreedit` に一本化
   - 文節 1 個のケース（短い変換）も `SplitPreedit { segments }` で扱う
4. **候補一覧展開（機能 10, Tab）**: Tab キーで focused 文節の候補数を拡張
   - 通常は `num_candidates` 個（既定 9）を表示
   - Tab で `3 * num_candidates` 個まで拡張し、候補ウィンドウの表示を更新
   - 追加候補は `engine.segment_candidates(reading, 3*n)` で取得
5. **undo after commit**（B 優先度、時間があれば）: 確定直後の Backspace で直前の Segments を復元

**完了条件**:
- Ctrl+Enter で部分確定ができる
- 同じ文を 2 回ライブ変換すると、1 回目の選択が 2 回目でトップに来る
- `SessionState::Selecting` が完全削除
- Tab で候補一覧が拡張表示される

---

## 9. 回帰テスト項目

### 9.1 ライブ変換

- [ ] 短い文（5 文字以下）のライブ変換が現行と同等に動く
- [ ] 中文（10〜20 文字）のライブ変換でトップ候補の品質が greedy 化で落ちていないことを確認
- [ ] ローマ字 pending 中の表示（"tat" 入力時の "t"）が維持される
- [ ] IME モード切替（ひらがな/カタカナ/英数）が正しく動く
- [ ] BackSpace で 1 文字削除できる
- [ ] ESC で変換がキャンセルされる

### 9.2 文節分割と移動

- [ ] ライブ変換後 Space で文節分割表示に入る
- [ ] 左右矢印キーで focused が移動する
- [ ] focused 以外の文節の選択状態が保持される
- [ ] 候補ウィンドウが focused 文節の候補を表示する
- [ ] Enter で全体確定、各文節が学習される

### 9.3 境界伸縮

- [ ] Shift+Right で対象文節が 1 文字右に伸びる
- [ ] Shift+Left で対象文節が 1 文字縮む
- [ ] 左側の文節が伸縮で一切変化しない（重要）
- [ ] 伸縮後、対象文節の候補が更新される
- [ ] 右端を超える伸長は無視される
- [ ] 0 文字に縮めると 1 つ前の文節と結合される（または無視）

### 9.4 部分確定

- [ ] Ctrl+Enter で focused までが確定、以降がライブ変換に戻る
- [ ] 確定した文節が学習される
- [ ] 残りライブ変換が正しく動く

### 9.5 数値保護

- [ ] `2024ねん` → `2024年`（LLM が年を変えない）
- [ ] `3じ15ふん` → `3時15分`
- [ ] `100えん` → `100円`
- [ ] `でんわ09012345678` → `電話09012345678`
- [ ] `1にち2かい` → `1日2回`
- [ ] 数字文節が fixed になり Shift+矢印で動かない
- [ ] 数字を含まない文のライブ変換に影響しない（既存動作と同じ）

### 9.6 学習

- [ ] 同じ reading を 2 回変換した時、1 回目の選択が 2 回目でトップに来る
- [ ] 部分確定した文節も学習される
- [ ] 学習データが永続化される（プロセス再起動後も有効）

### 9.7 パフォーマンス

- [ ] キーストロークごとのライブ変換レイテンシ: 現行比 1/3 以下
- [ ] Space 押下時の文節分割: 100ms 以下（長文は別途）
- [ ] 矢印キー移動: 16ms 以下（1 フレーム相当）
- [ ] Shift+矢印伸縮: 200ms 以下

### 9.8 ストレステスト

- [ ] 20 文節以上ある長文のライブ変換 → 文節分割 → 各文節の再変換
- [ ] Shift+矢印を連打しても破綻しない
- [ ] 矢印キーと候補選択を組み合わせても前の文節の選択が維持される
- [ ] 数値を含む長文の変換と伸縮
- [ ] 候補ウィンドウの Tab 展開 → 選択 → Enter

---

## 10. リスクと対応

| リスク | 影響度 | 対応 |
|---|---|---|
| `live_conv_beam_size=1`（greedy）でライブ変換の品質が落ちる | 中 | config.toml で `live_conv_beam_size` を可変に。デフォルト 1、ユーザーは 2 / 3 に上げて品質重視にもできる |
| 数値保護レイヤーで非数字ランが短すぎて LLM の精度低下 | 中 | 隣接数字ランを「プロンプトのコンテキスト情報」として渡す（出力には使わない） |
| `resize_segment` が遅い（文節ごとに LLM 再実行） | 中 | 対象文節と右側のみ再変換。左側はキャッシュ |
| `PROTOCOL_VERSION` bump で古い host.exe と新 TSF の組み合わせが動かなくなる | 低 | インストーラ再実行で解消、互換 warning を TSF のエラーメッセージに追加 |
| Mozc の思想参考のみで独自実装するため、Mozc と挙動が微妙に異なる | 低 | 意図的な差分（LLM 主体の変換、数値保護など）として設計書に明記、回帰テストで挙動を固定 |
| 既存の `Selecting` / 旧 `SplitPreedit` の廃止で既存ユーザー設定が壊れる | 低 | config.toml / keymap.toml のキー名は維持。内部状態のみ変更 |

---

## 11. 決定事項

本設計書の起草段階で合意に至った項目を以下にまとめる。

1. **beam_size の設定方法**: config.toml に新規キーを追加する
   - `live_conv_beam_size`: ライブ変換時のビーム幅。デフォルト `1`（greedy、最速）
   - `convert_beam_size`: Space 押下時のビーム幅。デフォルト `3`（品質重視）
   - 既存 `num_candidates` は「候補ウィンドウに表示する候補数」の意味として残す（beam_size とは別概念）
   - ライブ変換中は `engine.bg_start(live_conv_beam_size)` を呼ぶ
   - Space 押下で文節分割に入るとき、各文節の候補取得は `convert_beam_size` をビーム幅として使い、候補数は `num_candidates` まで取る

2. **Mozc コードの扱い**: 思想・設計のみ参考、コードの直接コピーは行わない
   - Mozc のファイル構成・型名・関数名は参考程度に留める
   - アルゴリズム（`ResizeSegment` など）は Mozc を読んで理解したうえで、rakukan 側で独自に Rust で再実装する
   - `THIRD_PARTY_LICENSES.md` への Apache 2.0 追記は不要（コードを取り込まないため）
   - ただし「Mozc を参考にした」旨の謝辞は README または設計書に明記する

3. **文節結合・分割の扱い**: 機能 4（Shift+矢印の伸縮）のみでカバーし、専用キーは設けない
   - Mozc と同じ方針: `Shift+Right` で対象文節を伸ばす結果として次文節が縮む（= 実質 split）
   - `Shift+Left` で対象文節を縮める結果として隣接文節に統合される（= 実質 merge）
   - 専用の merge/split キーは学習コスト増の割に利点が少ないため導入しない

4. **Candidate 注釈（`annotation`）の扱い**: 用法辞書を別ファイルで追加する Phase F として対応する
   - 既存 `rakukan-dict` のフォーマットは変更しない（再ビルド不要）
   - 新規 `UsageDict`（`rakukan-engine/src/usage_dict.rs`）をランタイムロードし、Candidate 生成時に `(reading, surface)` で引いて注釈を付与
   - データソースは Mozc の `usage_dict.tsv`（BSD-3、データ部分）。`THIRD_PARTY_LICENSES.md` に追記する
   - 候補ウィンドウは候補の右側に小さく注釈を表示する描画を追加
   - インストーラに `usage_dict.rkud`（約 200KB）を 1 ファイル追加
   - Phase A〜E とは独立した追加 phase として進行可能

5. **候補ウィンドウの一覧展開（機能 10, Tab）**: Phase E に含める
   - Phase C（`SplitPreedit` の新モデル置換）では Tab 展開は実装しない
   - 部分確定・学習・Selecting 統合と同じ Phase E の追加機能として実装する

6. **パフォーマンス目標**: 数値目標は設定せず、動作確認しながら改善する
   - 目標値は「現行比で目に見えて速い」こと。厳密な ms 数は測定結果を見て随時判断
   - 各 Phase 完了時に実機で計測し、体感差を評価
   - 明確なボトルネックが見つかれば個別に最適化 Phase を挿入

---

## 12. 要約

- ライブ変換から文節再変換までを Mozc 流の `Segments` / `Segment` / `Candidate` モデルで統一する
- 文字列からの再分節を撤廃し、`Segments` を状態として持ち回す
- ライブ変換は beam=1（greedy）で高速化、Space 時に candidate を拡張
- 境界伸縮は engine 側の `resize_segment` に集約、左側固定を保証
- 数値保護レイヤーで LLM の数字改変を根絶
- 部分確定と学習で実用性を底上げ
- 段階的移行（Phase A〜E）で既存機能を壊さない
