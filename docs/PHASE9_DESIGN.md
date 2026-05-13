# Phase 9: 分節解析を含む変換方式の見直し — 設計ドラフト

バージョン: draft-1  
作成日: 2026-05-12  
前提: v0.9.0 時点の rakukan コードベース（Phase 6b 完結後）  
ステータス: **設計検討段階**。実装着手前。

## 関連資料

- [CONVERSION_PIPELINE_CLEANUP_PLAN.md](CONVERSION_PIPELINE_CLEANUP_PLAN.md) — Phase 1〜8 の現行整理計画（Phase 9 が本書で扱う対象）
- [CONVERTER_REDESIGN.md](CONVERTER_REDESIGN.md) — 2026-04 起草の Mozc 流 Segments 設計（Phase A のみ実装、Phase B〜E は **vibrato 削除により orphaned**）
- [SEGMENT_EDIT_REDESIGN.md](SEGMENT_EDIT_REDESIGN.md) — Segment 列を正とする編集モデルの基礎方針
- [DESIGN.md](DESIGN.md) — 現行アーキテクチャ全体図
- `docs/azookey-analysis/`（ローカル参考資料、リポジトリ未収載）— azooKey 内部構造の分析メモ 10 章

## 1. 目的とスコープ

### Phase 9 が解こうとする問題

| # | 問題 | 現状の対処 | 限界 |
|---|---|---|---|
| 1 | 長い読み全体を毎回 LLM 一発変換する | beam=1 のライブ変換 + 後追い更新 | 中間で文節境界を意識せず、誤変換の局所修正が難しい |
| 2 | 句読点「、」「。」を含む長文の即時確定 | v0.8.12 で暫定対策を入れたが revert（破壊的） | 句読点直前まで自然に切れる仕組みが無い |
| 3 | RangeSelect 以外で部分確定する手段がない | RangeSelect (Shift+矢印) で手動指定 | ユーザー側の操作負担、自動化のヒントが無い |
| 4 | 確定後の文脈 (`committed`) が文単位のヒューリスティック切断のみ | 200 文字超で `。！？` 等で切断 | 文節レベルの構造情報が無い |
| 5 | 学習が「reading 全体 → surface 全体」のペア単位 | mozc-dict に existing な (reading, surface) のみ学習 | 文節単位学習ができず、汚染リスクは抑えられているが恩恵も限定 |

### スコープに含めるもの

- 長い読みの**論理的な文節分割**機構（読み側 + surface 側の境界対応）
- 文節境界に基づく**部分変換 / 部分確定**経路
- v0.8.12 で revert された **句読点即時確定の根本対応**
- **CandidateView の拡張** — 単一文字列から Segment 列への進化
- 既存の LLM 一発変換と**並存する移行戦略**（feature flag）

### スコープに含めないもの

- LLM 自体の差し替え（llama.cpp / model_variant は不変）
- symbolic conversion への全面回帰（Mozc 流の Viterbi + N-best を主軸にしない）
- 予測変換 (Prediction) / 英語予測 / Zenzai 風ニューラル再評価
- UI 全面刷新（候補ウィンドウは現行のまま延長利用）

### 非目標（azooKey 由来でも採用しない）

- LOUDS 辞書の独自実装（既に mozc バイナリ辞書を使用、同等機能）
- CID/MID 接続コスト + Lattice 探索（LLM が代替）
- typo correction generator（範囲外）
- 学習メモリの LOUDS merge（現行 bincode で十分）

## 2. 現状認識サマリ

### 2.1 すでにある資産

- **CandidateView** (Phase 6b) — `text` / `suffix` / `corresponding_reading_len` / `source` を保持し、azooKey の `Suggestion` 構造に近い
- **RangeSelect 経路** — 部分変換の UI/UX は既存。`prefix` + `selected` + `remainder` の 3 区間に分かれている
- **数値保護レイヤー** (`digits.rs`) — 数字 / 記号 / アルファベットの literal 保護で部分的な「分節」は既に存在（ただし表面的な文字種分割のみ）
- **`is_dict_surface` 学習ガード** — mozc-dict 由来のペアのみ学習、literal 候補を自動除外

### 2.2 すでに無いもの（撤去済み）

- vibrato 形態素解析器（v0.5.1 で完全削除）
- `rakukan-engine/src/segmenter.rs`（vibrato 依存だった、削除済み）
- Mozc 流 `Segments` / `Segment` / `Candidate` 型（CONVERTER_REDESIGN で起草、未実装）

### 2.3 「分節解析」の意味の再定義が必要

CONVERTER_REDESIGN は vibrato + DP による「形態素レベル分節」を想定していた。本 Phase 9 では vibrato が無い前提で、**より粗い「論理文節」レベル**から始める。

```text
読み: わたしはがっこうへいきます
細粒度（vibrato 流）: わたし / は / がっこう / へ / いき / ます
粗粒度（Phase 9 仮）: わたしは / がっこうへ / いきます
```

粗粒度は実装が容易で、ユーザー体感の「文節編集」には十分。細粒度は精度を要するため後段で検討。

## 3. 段階的アプローチ案

Phase 9 を **3 つのサブフェーズ**に分割する。

### Phase 9.1: 「句読点境界 + 助詞境界」の symbolic 検出（小〜中スコープ）

**目的**: 句読点と助詞だけを文節境界として検出し、それより前を確定可能にする。

**根拠**: 句読点（、。！？）と主要助詞（は / が / を / に / で / と / へ / も / の）の境界は ASCII / mozc-dict 簡易 lookup で判定可能。LLM や Viterbi なしで動く。

**入力**: 読み文字列  
**出力**: 境界位置のリスト `Vec<usize>`（reading 上の文字位置）

**実装案**:
- `rakukan-engine` に `find_logical_boundaries(reading: &str) -> Vec<usize>` を追加
- 助詞の検出は HashSet ベース（コスト最小）
- 句読点直後で必ず境界
- 助詞の直後で候補境界（後段で確定するか判定）

**ユースケース**:
- v0.8.12 で revert した「句読点即時確定」の代替: 境界検出 → 境界より前を確定 → 残りを継続入力
- 長文ライブ変換で内部に視覚的なヒント（境界マーカー）を出すかは検討対象

**リスク**: 助詞リストの過不足（例: 「に」が助詞か / 「日に」の「に」か）。境界検出は確率的なヒントとして扱い、必ずユーザー操作で確定するなら破壊性ゼロ。

### Phase 9.2: Segment 列を正とする CandidateView の拡張（中スコープ）

**目的**: 1 つの `CandidateView` を「単一文字列」から「Segment 列」へ拡張。

**新型（azooKey + 既存 SEGMENT_EDIT_REDESIGN 流）**:
```rust
pub struct CandidateSegment {
    pub reading: String,      // この文節の読み
    pub surface: String,      // 表示文字列
    pub fixed: bool,          // 数字 / 記号 literal、または部分確定済み
    pub source: CandidateViewSource,  // 既存と同じ
}

pub struct CandidateView {  // 既存型を拡張
    pub text: String,                          // 既存。互換のため残す（segments の連結）
    pub segments: Vec<CandidateSegment>,       // 新規。Phase 9.1 の境界検出結果を反映
    pub corresponding_reading_len: usize,      // 既存
    pub suffix: String,                        // 既存（remainder）
    pub source: CandidateViewSource,           // 既存
}
```

**不変条件**:
- `segments.iter().map(|s| &s.surface).collect::<String>() == text`
- `segments.iter().map(|s| &s.reading).collect::<String>().len() == corresponding_reading_len + suffix.len()` （ただし suffix は別途）

**移行**:
- 初期は `segments` を 1 要素（全体）のみで埋める。挙動は従来通り。
- Phase 9.1 完了後、境界検出を `activate_selecting_*` で適用して `segments` を多要素化。
- 既存の `current_candidate()` / `page_candidates()` は `text` を返し続けて互換維持。
- 新規 API `current_segments() -> &[CandidateSegment]` を追加。

### Phase 9.3: 部分確定の経路統合（中スコープ）

**目的**: 句読点入力 / 助詞境界 / RangeSelect / 候補選択を**一つの部分確定 API**に統合。

**現状の経路分散**:
- RangeSelect → Space → 候補選択 → Enter（既存）
- 句読点入力 → v0.8.12 で「直前を確定 + 句読点もコミット」（revert 済み）
- LiveConv → Space → Selecting → Enter（既存）

**統合 API 案**:
```rust
fn commit_until_boundary(
    sess: &mut SessionState,
    boundary_idx: usize,  // segments[..boundary_idx] を確定、残りを LiveConv 継続
) -> Result<CommitOutcome>;
```

これにより:
- 句読点経路: 句読点を境界として `commit_until_boundary(after_punctuation)` を呼ぶだけ
- RangeSelect 経路: 選択範囲を境界として同じ API
- 候補選択経路: 全体を境界として同じ API（= 全確定）

## 4. LLM と segmentation の役割分担（重要決定点）

### 案 A: LLM は不変、segmentation は post-hoc

- LLM は読み全体に対して `text` を返す（現状通り）
- TSF 側で `find_logical_boundaries(reading)` を別途実行
- 読みの境界と surface の境界の対応は**ヒューリスティック** (`text.len() / reading.len()` 比例配分など)
- **メリット**: LLM 不変。RPC 変更最小。
- **デメリット**: 読み境界と surface 境界の不一致でズレが出る（例: `わたしは` → `私は` で「わたし」 = 3 char、「私」 = 1 char）

### 案 B: LLM が segmentation 情報を出力

- llama.cpp に GBNF grammar / structured output を指示し、`[{kanji}|{reading}]` 形式で出力させる
- 例: `[私|わたし][は|は][学校|がっこう][へ|へ][行きます|いきます]`
- **メリット**: 読みと surface の対応が正確
- **デメリット**: モデルの学習データ次第で精度に幅あり、推論時間増加（出力長 2 倍程度）、grammar 失敗時の fallback 必要

### 案 C: ハイブリッド（推奨）

- LLM は通常通り `text` を返す（メイン経路）
- 別途 `mozc-dict` で読みを左から右に最長一致 lookup し、surface 側の境界候補を推定
- ライブ変換時には案 A（粗い境界）を使い、Space 押下後の部分確定時に案 B（精緻な境界）に切り替えるオプション
- **メリット**: 既存経路を壊さない、必要なときだけ高コスト経路
- **デメリット**: 実装複雑度が上がる

## 5. azooKey からの取り込み方針（再確認）

[CONVERSION_PIPELINE_CLEANUP_PLAN.md](CONVERSION_PIPELINE_CLEANUP_PLAN.md) の Phase 6b で取り込んだ要素は azooKey 流 `Candidate` / `MarkedText` の枠組みに収束。Phase 9 では以下を追加で参照:

| azooKey 概念 | Phase 9 での扱い |
|---|---|
| `ComposingText.input + convertTarget` | **採用見送り**: rakukan は roman2kana を engine 側で処理済み。入力履歴の二重保持は overkill |
| `Candidate.composingCount` | **Phase 9.2 で活性化**: `corresponding_reading_len` を Segment 列の合計と整合させる |
| `Candidate.data: [DicdataElement]` | **採用見送り**: LLM 出力からは復元不可。Phase 9.2 の `Vec<CandidateSegment>` で代替 |
| `firstClauseResults` | **検討**: Phase 9.3 で `commit_until_boundary` の一部として、先頭文節候補だけを別途取得する経路を持つかは未決 |
| Zenzai 風 prefix constraint | **将来検討**: llama.cpp の `logit_bias` で実装可能だが Phase 9 のスコープ外 |
| LearningMemory 文節 bigram 学習 | **採用見送り**: 学習粒度を粗くするほうが安全（誤学習リスク） |

## 6. 段階移行プラン

```text
v0.9.x (現状): Phase 6b 完結、CandidateView 安定運用、学習履歴クリーンアップ
              │
              ▼
v0.10.0: Phase 9.1 — symbolic boundary detection
        - find_logical_boundaries(reading) を engine に追加
        - ログ probe only（動作変化なし、観測のみ）
        - feature flag: [phase9] boundary_detect = false (デフォルト)
              │
              ▼
v0.10.x: Phase 9.2 — CandidateView.segments 拡張
        - CandidateView に segments フィールド追加（互換維持: 1 要素 default）
        - Phase 9.1 の境界検出を活性化（依然 ログのみ）
        - flag を true にすると visualization が出るオプション
              │
              ▼
v0.11.0: Phase 9.3 — 部分確定経路統合
        - commit_until_boundary API 導入
        - 句読点入力時の境界判定で auto-commit（v0.8.12 のリベンジ）
        - flag を true にすると句読点 auto-commit ON
              │
              ▼
v0.11.x: stability + LLM 連携深化（案 B / C への移行検討）
```

各 minor バージョンで feature flag は OFF をデフォルトに。実機で OK と判断したらデフォルト ON に切り替える minor bump を行う。

## 7. 未決事項（決定が必要）

| # | 項目 | 決定者 | 期限の目安 |
|---|---|---|---|
| Q1 | 句読点 auto-commit を Phase 9 で再導入するか、別 phase に切り出すか | 設計判断 | Phase 9.1 着手前 |
| Q2 | LLM への segmentation 情報出力（案 B）に踏み込むか | 設計判断 + 実験 | Phase 9.2 完了後 |
| Q3 | 助詞リストの確定範囲（厳密 / 緩い） | 実装時 | Phase 9.1 実装着手時 |
| Q4 | 境界検出を `rakukan-engine` 内に置くか、`rakukan-dict` 内か | アーキテクチャ | Phase 9.1 着手前 |
| Q5 | `CandidateSegment.fixed` の意味（literal? 部分確定済み? 両方?） | 設計判断 | Phase 9.2 着手前 |
| Q6 | `docs/japanese-understanding/` を公式取り込みするか、ローカル参考に留めるか | プロジェクト判断 | いつでも |
| Q7 | `docs/azookey-analysis/` 同上 | プロジェクト判断 | いつでも |
| Q8 | 文節単位学習 (azooKey の clause bigram) を Phase 9 で扱うか | 設計判断 | Phase 9.3 着手後 |

## 8. 既存文書との関係

- **CONVERTER_REDESIGN.md Phase A** は引き続き有効。Phase B〜E（vibrato 前提）は本書で代替方針を提示し orphan を解消。
- **SEGMENT_EDIT_REDESIGN.md** の不変条件（Segment 列を正とする）は本書 Phase 9.2 が継承。
- **CONVERSION_PIPELINE_CLEANUP_PLAN.md Phase 9** セクションは本書で具体化。

## 9. 完了条件（Phase 9 全体）

Phase 9.1〜9.3 すべて完了時:

- [ ] 句読点を含む長文を入力中、句読点で auto-commit するオプションが安定動作する
- [ ] RangeSelect と句読点 auto-commit が同じ `commit_until_boundary` 経路を通る
- [ ] `CandidateView.segments` が非空となり、部分確定で正しい範囲の reading/surface が確定される
- [ ] 既存の LLM 一発変換は feature flag OFF で完全に従来通り動作する（regression なし）
- [ ] 文節境界検出のログが grep 可能で、誤検出率を実機で測定できる

## 10. 要約

Phase 9 は「rakukan を symbolic 化する」ことが目的ではなく、**LLM 主軸を維持しながら、azooKey 由来の Segment ベース編集モデルと粗い境界検出を組み合わせて、長文 / 句読点 / 部分確定の UX を改善する**こと。

実装の鍵は:
1. **境界検出を symbolic / 軽量** に保つ（Phase 9.1）
2. **CandidateView の段階的拡張** で互換性を維持（Phase 9.2）
3. **部分確定経路の統合** で重複コード削減（Phase 9.3）
4. **feature flag による段階移行** で regression リスク管理

vibrato 削除以降の orphan を解消し、azooKey 分析の成果を実装可能な単位に落とし込む計画。
