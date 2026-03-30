# 分節編集ロジック再設計メモ

更新日: 2026-03-30

## 目的

ライブ変換後の分節編集について、次の 2 点を満たす。

- `Right` / `Shift+Right` / `Space` / `Enter` を繰り返しても破綻しない
- 部分再変換中に別の文節を掴んだり、前後の文節が壊れたりしない

ここでは「高精度」より先に、「破綻しない状態機械」を正とする。

## 現状の問題

現状の破綻は、主に次の構造から来ている。

- 分節編集状態を文字列 `surface + reading` から毎回復元している
- `SplitPreedit`、`Selecting`、候補採用後で、それぞれ別の復元経路を持っている
- `surface` の分節と `reading` の対応付けを後付けで推定している
- 後続再変換で prefix 側まで事実上再解釈してしまう

この方式では、一度 `surface` と `reading` の対応がずれると、

- 違う文節を選ぶ
- `Shift+Right` で不自然に伸びる
- 部分採用後に前後が壊れる

という破綻が起こる。

## 基本方針

分節編集は「文字列編集」ではなく「Segment 列の編集」として扱う。

- TSF は `Vec<Segment>` を正として持つ
- 各 `Segment` は `surface` と `reading` を必ず対で持つ
- 左側の確定済み Segment は再推定しない
- 再変換対象だけを差し替える
- 変更後は「右側だけ」を再変換・再分節する

## 新しいデータモデル

```rust
struct Segment {
    reading: String,
    surface: String,
    fixed: bool,
}

struct SegmentBuffer {
    segments: Vec<Segment>,
}

struct SegmentSelection {
    start: usize,
    end: usize,
}

enum SegmentEditMode {
    SplitEdit,
    CandidateSelecting {
        candidates: Vec<Candidate>,
        selected: usize,
    },
}

struct SegmentEditState {
    buffer: SegmentBuffer,
    selection: SegmentSelection,
    mode: SegmentEditMode,
}

struct Candidate {
    surface: String,
    segments: Vec<Segment>,
}
```

## 不変条件

実装中は次の条件を常に守る。

1. `buffer.segments[..selection.start]` は左側固定領域であり、再解釈しない
2. `buffer.segments[selection.start..selection.end]` が現在の編集対象
3. `buffer.segments[selection.end..]` が後続領域
4. `Segment.reading` は後から推定しない
5. `Candidate.segments` は engine 側で確定済みであること

## 各操作の意味

### `Right`

- 選択範囲を 1 文節右へ移動する
- 候補再取得はしない
- 文節列は変更しない

### `Left`

- 選択範囲を 1 文節左へ移動する
- 候補再取得はしない
- 文節列は変更しない

### `Shift+Right`

- `selection.end += 1`
- ただし単なる表示更新ではなく、拡張後の選択範囲を新しい編集対象として扱う
- 右側残りの `reading` 全体を再変換する
- 右側だけを再分節する
- 左側 `[..selection.start]` はそのまま維持する

期待動作:

```text
刺身 / と / わ / さ / びとおでんとからし
      ^

Shift+Right

刺身 / と / わさ / び / と / おでん / と / からし
      ^^^

Shift+Right

刺身 / と / わさび / と / おでん / と / からし
      ^^^^^
```

### `Shift+Left`

- `selection.end -= 1`
- 右側は再変換しない
- 既存 Segment をそのまま使う

### `Space`

- 現在の選択範囲の `reading` だけを使って候補取得する
- `CandidateSelecting` へ入る
- 前後 Segment は保持する

### 候補採用

- 左側 Segment はそのまま
- 選択範囲を `candidate.segments` で置換
- 後続の `reading` 全体を再変換
- 後続を再分節
- 次の選択位置は、原則として置換された範囲の直後

### `Enter`

- `buffer.segments` 全体の `surface` を連結して確定

### `Esc`

- 選択範囲だけ `reading` 表示へ戻す
- 左右 Segment は保持

## 必要な engine / ABI 変更

現状の `Vec<String>` 候補では不十分。最終的に engine は次を返す必要がある。

```rust
struct Candidate {
    surface: String,
    segments: Vec<Segment>,
}
```

必要 API:

- `convert_sync_candidates(reading) -> Vec<Candidate>`
- `bg_take_candidates_structured(reading) -> Vec<Candidate>`
- `segment_surface(surface)` は補助 API として残してよい

重要なのは、`surface` を返すだけでなく、同時に `segments` を返すこと。

## TSF 側でやめること

- `surface + reading` から毎回 `SplitBlock` を再構築する
- prefix 側の `reading` を後から再推定する
- `Selecting` と `SplitPreedit` で別々の復元ルールを持つ
- 候補採用後に全文を再分節する

## 移行手順

### Phase A: 状態モデルの入れ替え

- `SessionState::SplitPreedit` を `SegmentEditState` ベースへ変更
- `Selecting` を `CandidateSelecting` として統合
- 文字列ベースの `prefix / remainder` を段階的に廃止

### Phase B: 操作の一本化

- `Right` / `Left`
- `Shift+Right` / `Shift+Left`
- `Space`
- 候補採用

をすべて `SegmentEditState` に対する操作へ統一する

### Phase C: engine 候補 ABI の拡張

- `Vec<String>` ではなく `Vec<Candidate>` を返す ABI を追加
- TSF はそれをそのまま保持

### Phase D: 旧ロジック撤去

- `build_split_blocks_from_surface`
- `surface + reading` 再対応付け
- 文字列連結ベースの復元処理

を削除する

## ログ方針

再設計後も、次のログは残す。

- 現在の Segment 列
- 選択範囲 `start/end`
- `Shift+Right` 前後の Segment 列
- 候補採用時の置換範囲
- 後続再変換の入力 `reading`
- 後続再変換の出力 `Candidate.segments`

ログは「文字列」ではなく `surface<reading>` の列で出す。

## 完了条件

次が満たされたら、この再設計は完了とみなす。

1. `Shift+Right` で意図した文節にだけ伸びる
2. 部分再変換後も左側が壊れない
3. 後続だけが再変換・再分節される
4. `Space` / 候補採用 / `Enter` を繰り返しても、別の文節を掴まない
5. `surface` と `reading` の対応を TSF 側で再推定しない

## 要約

破綻ゼロを狙うには、

- 文字列をつなぎ直す設計ではなく
- `Segment` 列を正として編集する設計

へ切り替える必要がある。

この文書の方針は、ライブ変換と分節再変換を「壊れない編集モデル」に寄せるための基礎設計である。
