# Step 10 詳細設計: ローマ字入力状態を入力文字列から再構築する

作成: 2026-09-07
位置づけ: `September_Revised_Plan.md` の Step 10（G-5、P1）の詳細。計画本文の「作業 / 検証 / 完了条件」を、現行コード（main `6c8a2fe` 時点）に即して具体化する
対象クレート: `rakukan-engine`（engine DLL 内に閉じる）。Space 変換時の接尾辞表示だけ `rakukan-tsf` に及ぶ
着手: 未定。Issue #18 の不具合修正フェーズ（C / H / I / F の一部）の後

## 1. 背景と経緯

- G-5: 誤入力を Backspace で消した後、残ったローマ字と次の文字が結合しない。`kt` → Backspace → `a` が「か」ではなく「kあ」になる。打ち直せば回避できるので P1
- Issue #18 の B（Backspace 後の子音戻し）は、フォーク側の実装が「入力ログからの再生」ではなく Backspace 専用の後処理（`reclaim_pending_consonant()`）だった。計画どおり Step 10 はこちらで「再生」構造として実装し、先方に rebase してもらう。参考差分 `6a1b1cc` のテスト期待値（`kt` → BS → `a` = 「か」等）は受け入れテストに使う（差分自体はローカルに無い。fork 側）
- Issue #18 の F のうち「Shift+英字が未確定ローマ字を追い越す」は Step 10 に合流。PR #26 は `romaji_input_log` を再生する実装で Step 10 と二重になるため、Step 10 完了後に出し直してもらう。PR #26 の観察（復元する / しないの判定表 `google` / `claude` / `seedreamtsukau`、「境界は未確定バッファを引いた位置で取る」「読みの途中の英単語は対象外」）は本設計の材料
- Step 1（PR #10）は `hiragana_text()` を reading として扱う前提で完了しており、未変換接尾辞（「たt」の `t`）の分離・表示・確定・学習除外は本 Step に持ち越し
- Step 5（L-1）の Backspace テストは「`kt` → Backspace で reading が `k` から空に変わる」を期待している。本 Step で期待値を再確認する

## 2. 現状の仕組み（`crates/rakukan-engine/src/lib.rs`）

### 2.1 状態

| フィールド | 役割 |
|---|---|
| `romaji: RomajiConverter` | trie による変換器。`output`（変換済み）と `buffer`（未確定）を持つ |
| `hiragana_buf` | 表示・変換キーになる確定済み文字列。かな以外（数字・記号・全角英字・素通し ASCII）も入る |
| `pending_romaji_buf` | 未確定ローマ字。常に `romaji.buffer()` と同じ内容 |
| `romaji_input_log: Vec<String>` | ユーザーが打った文字の原本。`RomajiConverter::Converted` 単位で 1 要素。F6〜F10 の復元に使う |

不変条件（既存テスト `romaji_log_matches_typed_input_for_qwrty` で保証）:
`romaji_input_log.concat() + pending_romaji_buf == ユーザーが打った文字列`

### 2.2 入力経路（`push_char` の 5 経路 + 2 関数）

`push_char` は先頭から順に判定し、いずれも `romaji_input_log` に打鍵文字を積む。

1. 数字直後の `,` / `.` → 数値区切り（`digit_separator_auto`、pending が空のときのみ）
2. 英字・記号直後の `,` / `.` → 欧文句読点（`alpha_width` / `symbol_width` に追従、pending が空のときのみ）
3. 数字 0–9 → `digit_width` に従って全角 / 半角（pending が空のときのみ）
4. `,./[]\-` 以外の印字可能 ASCII 記号 → `symbol_width` に従って全角 / 半角（pending が空のときのみ）
5. 英字と `,./[]\-` → trie。`pending_romaji_buf` に積んで `romaji.push`、`output` / `buffer` の差分から確定分と未確定分を判定する

`push_raw`（テンキー記号など。かなルール登録文字を直接）と `push_fullwidth_alpha`（Shift+英字。`alpha_width` に従う。log には ASCII 大文字を記録）は **pending の有無を見ずに** `hiragana_buf` へ追記する。

`flush_pending_n` は Convert / CommitRaw の直前に末尾の `n` を「ん」にする。log に `"n"` を積み、変換器を作り直す。
`force_preedit`（F6〜F10、RangeSelect、BlockSelecting）は `hiragana_buf` を差し替え、pending と変換器を捨て、**log は保持する**。

### 2.3 Backspace（現行）

`RomajiConverter::backspace` の結果で 3 分岐する。

| 変換器の結果 | engine の処理 |
|---|---|
| `RemovedBuffer`（buffer から 1 文字） | `pending_romaji_buf.pop()`。log は触らない |
| `RemovedOutput`（output から 1 文字） | `hiragana_buf.pop()`、`romaji_input_log.pop()` |
| `Empty` | `hiragana_buf` が空でなければ `pop()` と `log.pop()` |

### 2.4 復元（F6〜F10）

- `romaji_log_str()`: log を連結して返す。F9 / F10 はこれに `pending` 相当の接尾辞を足してラテン文字に変換する（`edit_ops.rs`）
- `hiragana_from_romaji_log()`: log を連結し、新しい変換器に 1 文字ずつ流してかなを作り直す。最後に `flush`

## 3. 問題の分析

### 3.1 G-5 の再現機構（`kt`）

1. `k`: trie 上の有効な経路なので `Buffered`。`pending = "k"`、log は空
2. `t`: buffer `"kt"` は一致なし。先頭 `k` は子を持つが `kt` は経路上にないので `k` を素通し（PassThrough）。`hiragana_buf = "k"`、`log = ["k"]`、`pending = "t"`
3. Backspace: 変換器は buffer の `t` を消して `RemovedBuffer`。engine は `pending` を空にするだけ。`hiragana_buf = "k"`、`log = ["k"]` のまま
4. `a`: `pending = "a"` → 「あ」。結果 `"kあ"`

素通しした `k` が「確定済み」側に固定され、Backspace が入力原本（`ka`）から状態を作り直さないのが原因。`nt`（`n` が「ん」に確定）、`tt`（「っ」に確定）も同じ構造。

### 3.2 現行 Backspace が壊す不変条件

- `RemovedOutput` で `hiragana_buf` を 1 文字、log を 1 要素消す。log 要素は 1 文字とは限らない（`"kya"` → 「きゃ」）。「きゃ」から「ゃ」だけ消すと log から `"kya"` 全体が消え、`hiragana_buf = "き"` に対して log は空になる。この状態で F9 を押すと `k i` が復元されない（※コード上の読み。実機未確認）

### 3.3 未確定ローマ字を追い越す入力（Issue #18 F）

`push_raw` と `push_fullwidth_alpha` は pending を見ない。`k` → Shift+`A` で `hiragana_buf = "A"`、`pending = "k"` となり、表示は `"Ak"`、原本は `kA`。順序が入れ替わる。

### 3.4 復元関数の入力種別の欠落

`hiragana_from_romaji_log` は log の全文字を変換器に流す。変換器は大文字を小文字化するので、Shift+英字で積んだ `"A"` が「あ」になる。数字・記号は trie に無ければ素通しで戻るが、`,./[]\-` は trie ルール（、。・「」￥ー）に当たるため、数値区切りとして打った `,`（log には `","` が入る）が「、」に戻る。いずれも log が「何として入力されたか」を持たないため（※コード上の読み。実機未確認）。

### 3.5 Space 変換時の未変換接尾辞（「たt」）

`hiragana_text()` = 「た」、`pending` = `t`。ライブ変換は `ends_with_pending_romaji` で起動を抑止している。Space 変換では reading「た」で候補を引き、接尾辞 `t` を表示・確定に残し、学習キーには含めない、という扱いを本 Step で確定する。TSF 側の候補ビューには `corresponding_reading_len` / `suffix_len` が既にあり（Step 1）、接尾辞を表示する土台はある。末尾 `n` は `flush_pending_n` で「ん」にしてから変換している。

## 4. 設計

### 4.1 入力原本に種別を持たせる

`romaji_input_log: Vec<String>` を `Vec<InputEntry>` に変える。

```rust
struct InputEntry {
    /// ユーザーが打った文字（そのまま）
    typed: String,
    kind: InputKind,
    /// このエントリでローマ字区間が閉じた（flush_pending_n / 区間終端）
    closes_run: bool,
}

enum InputKind {
    /// 経路 5。trie で変換する通常ローマ字
    Romaji,
    /// 経路 3。数字
    Digit,
    /// 経路 4。ASCII 記号（幅設定で確定）
    Symbol,
    /// 経路 1・2。自動置換された区切り（数値区切り・欧文句読点）
    Separator,
    /// push_raw。かなルール登録文字の直接入力
    Raw,
    /// push_fullwidth_alpha。Shift+英字（typed は ASCII 大文字）
    ShiftAlpha,
}
```

- 不変条件は `entries.iter().map(typed).concat() + pending == 打鍵文字列` のまま
- `romaji_log_str()` の戻り値（連結文字列）は変えない。F9 / F10 の呼び出し側は無変更
- `hiragana_from_romaji_log()` は種別ごとに復元する。`Romaji` の連続区間だけを変換器に流し、`Digit` / `Symbol` / `Separator` / `Raw` / `ShiftAlpha` は `push_char` 時と同じ幅規則で 1 文字ずつ出力する。これで 3.4 が解消する

### 4.2 再生関数を分離する

Backspace 専用の分岐ではなく、入力原本から状態を作るテスト可能な純粋関数を置く。

```rust
/// Romaji 区間の打鍵列を先頭から流し、(確定かな, 未確定ローマ字) を返す
fn replay_romaji_run(typed: &str) -> (String, String, RomajiConverter)
```

- 呼び出し側は「最後の非 Romaji エントリより後ろ」の区間だけを再生し、それより前の `hiragana_buf` はそのまま残す。数字・記号・Shift 英字・直接入力を通常ローマ字として再解釈しない（計画の要件）
- `closes_run == true` のエントリでは変換器を `flush` して区間を閉じる。`flush_pending_n` で「ん」にした `n` が、再生時に未確定 `n` に戻らないようにする
- 再生後、`romaji`（変換器）・`hiragana_buf`・`pending_romaji_buf`・entries の 4 つが同じ入力列を表す状態にする

### 4.3 Backspace

pending の有無で分ける。

**pending が空でないとき（G-5 の対象）**: 原本の末尾 1 打鍵を消し、末尾の Romaji 区間を再生する。

| 入力 | 現行 | 変更後 |
|---|---|---|
| `kt` → BS | hira `k` / pending 空 | hira 空 / pending `k` → `a` で「か」 |
| `nt` → BS | hira 「ん」/ pending 空 | hira 空 / pending `n` → `a` で「な」 |
| `tt` → BS | hira 「っ」/ pending 空 | hira 空 / pending `t` → `a` で「た」 |
| `kanakq` → BS | 「かなk」（`k` は確定側） | 「かなk」（`k` は pending 側）。表示は同じ |

**pending が空のとき**: 表示上の 1 文字を消す現行動作を維持する（「きゃ」→「き」）。ただし log を要素ごと捨てず、消した後に残るかなに対応する打鍵に書き換える（`"kya"` → `"ki"`）。かな → ローマ字の逆引き表が要る。逆引きできない場合（直接入力の記号など）は typed をそのまま 1 文字削る。

この分岐が本 Step で決める最大の UX 判断（5. の 1 参照）。「1 打鍵戻す」に統一すると「きゃ」→ BS が `ky`（表示 `ky`）になり、MS-IME の「ゃ」だけ消える挙動から離れる。

**force_preedit 後**: F7 でカタカナにした後などは `hiragana_buf` が log から導出できない。`force_preedit` でフラグ（例: `log_detached`）を立て、その間の Backspace は現行の単純 pop に留める。F6〜F10 のサイクルは既存の復元関数が log から作り直すので影響しない。RangeSelect / BlockSelecting からの復帰も同じ扱い。

### 4.4 未確定ローマ字を追い越す入力（`push_raw` / `push_fullwidth_alpha`）

pending が空でない状態でこれらが呼ばれたら、先に pending を閉じてから追記する。閉じ方は `RomajiConverter::flush`（`n` → 「ん」、`k` → `k` 素通し）で、エントリに `closes_run = true` を付ける。これで表示順と原本の順が一致する（`k` → Shift+`A` = `kA`）。

PR #26 の観察（`google` / `claude` は英単語として復元、`seedreamtsukau` は途中の英単語を対象外）は、この「閉じる」処理ではなく F9 / F10 の復元側の話なので、本 Step では扱わず、PR #26 の出し直し時に判断する。

### 4.5 Space 変換の未変換接尾辞

engine 側は既に reading（`hiragana_text()`）と接尾辞（`pending_romaji_buf`）を分けて返している。本 Step では次を確定する。

- 辞書・学習候補は reading で引く。接尾辞は候補表示の末尾に付け、確定文字列にも残す。学習キーには含めない
- 接尾辞は `alpha_width` に従って全角 / 半角にする
- 末尾 `n` は現行どおり `flush_pending_n` で「ん」にしてから変換する（推奨。末尾の `n` は英字接尾辞より「ん」の打ち途中である場合が圧倒的に多い）。接尾辞として残す案は採らない
- TSF 側は `CandidateView` の `suffix_len` を使って表示する。変更は `on_convert.rs` の候補表示・確定経路に限る

### 4.6 ABI / RPC

`engine_backspace`（bool 戻り）、`engine_push_*`、`engine_romaji_log_str`、`engine_hiragana_from_romaji_log`、`PreeditState`（hiragana + pending）のシグネチャは変えない。TSF は Backspace の後に `preedit_display()` と `hiragana_text()` を取り直しているので（`on_convert.rs` の `on_backspace`）、engine 内の状態の作り直しはそのまま反映される。ABI バージョンの更新は不要な見込み。

## 5. 着手前に決めること

1. **pending が空のときの Backspace**: 「表示 1 文字を消す（現行、log は逆引きで書き換え）」か「1 打鍵戻す（`きゃ` → `ky`）」か。推奨は前者（4.3）
2. **末尾 `n`**: `flush_pending_n` で「ん」に確定する現行維持（推奨）か、接尾辞として残すか
3. **`push_raw` / `push_fullwidth_alpha` で pending を閉じるとき** `n` を「ん」にするか素通し `n` にするか。推奨は `flush`（「ん」）
4. **`force_preedit` 後の Backspace**: 現行の単純 pop で据え置く（推奨）か、log から再構築するか

## 6. 変更対象

- `crates/rakukan-engine/src/lib.rs`: `InputEntry` / `InputKind` の導入、`push_char` 5 経路と `push_raw` / `push_fullwidth_alpha` / `flush_pending_n` / `force_preedit` の log 更新、`backspace` の書き直し、`replay_romaji_run` の追加、`hiragana_from_romaji_log` の種別対応、かな → ローマ字逆引き表（5. の 1 で前者を採る場合）
- `crates/rakukan-engine/src/romaji/converter.rs`: 変更なしの見込み。必要なら区間再生用のヘルパーを足す
- `crates/rakukan-tsf/src/tsf/factory/on_convert.rs`: 接尾辞の表示・確定・学習除外（4.5）
- `docs/September_Revised_Plan.md`: Step 10 の進捗記録

## 7. テスト

### 7.1 engine 単体（`cargo test -p rakukan-engine --lib`）

受け入れ（計画 + 参考差分 `6a1b1cc` の期待値）:

- `kt` → BS → `a` = 「か」
- `nt` → BS → `a` = 「な」
- `tt` → BS → `a` = 「た」（促音が残らない）
- `kanakq` → BS = 「かなk」（既存テスト `kana_then_kq_then_bs_removes_q_only` を維持。内部では `k` が pending 側）
- `qwrty` の表示と、log + pending == 打鍵列の不変条件（既存 2 件を維持）

追加:

- `kya` → BS = 「き」、log が `ki` 相当になり F9 で `ki` が出る（5. の 1 で前者を採る場合）
- `kon` → `flush_pending_n` → BS = 「こ」（「ん」を消す。再生で `n` が未確定に戻らない）
- `k` → Shift+`A` の表示が `kA`（順序が入れ替わらない）。`n` → Shift+`A` は 5. の 3 に従う
- 数字・記号・数値区切り・欧文句読点・直接入力の後の Backspace が現行どおり 1 文字消え、log と一致する
- Shift+英字 `A` と数値区切り `,` を含む log から `hiragana_from_romaji_log` が「あ」「、」を作らない
- `force_preedit` 後の Backspace が現行どおり 1 文字消す
- 全経路で `log + pending == 打鍵列` を保つ性質テスト（ランダムな英数記号列を流して確認）

### 7.2 TSF 単体（`cargo test -p rakukan-tsf --lib`、PowerShell から）

- Step 5（L-1）の Backspace テスト: `kt` → BS で reading が空、pending が `k` になる期待値に更新
- Step 1 の「たt」「やまのたn」: reading と接尾辞の分離が変わらないこと、接尾辞が候補表示・確定に残り学習キーに含まれないこと

### 7.3 実機

- `kt` / `nt` / `tt` の打ち直しが Backspace 1 回で直る
- F6〜F10 のサイクル（Shift+英字、数字、記号を含む文字列）が崩れない
- RangeSelect / BlockSelecting / F7 から戻った後の Backspace が現行どおり

## 8. 完了条件（計画本文の再掲）

- Backspace 後の表示結果、変換器、入力ログ、未確定ローマ字が同じ入力列を表す
- 誤入力を削除した後の次の文字が、削除前の派生出力に妨げられず、残ったローマ字と結合される
