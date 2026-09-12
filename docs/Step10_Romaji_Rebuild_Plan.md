# Step 10 詳細設計: ローマ字入力状態を入力文字列から再構築する

作成: 2026-09-07（2026-09-09 更新: Issue #18 の 9/8 回答と、TSF 記号経路の追い越しを反映。2026-09-10 更新: 10-0 の調査結果、実施段階の分割案、10-1 の設計案を反映。§5 の判断は未定のまま）

位置づけ: `September_Revised_Plan.md` の Step 10（G-5、P1）の詳細。計画本文の「作業 / 検証 / 完了条件」を、現行コード（main `6c8a2fe` 時点）に即して具体化する

対象クレート: `rakukan-engine`（engine DLL 内に閉じる）。Space 変換時の接尾辞表示だけ `rakukan-tsf` に及ぶ

関連 Issue: #34（本 Step の問題点の記録）、#38（英単語の破壊・右端。PR #26 の置き場）、#35（`z` 系キー列の扱い）、#18（B / F）

着手: 検討中（2026-09-10 に 10-0 の調査を完了。実施段階の案は §9。実装は未着手）

## 1. 背景と経緯

- G-5: 誤入力を Backspace で消した後、残ったローマ字と次の文字が結合しない。`kt` → Backspace → `a` が「か」ではなく「kあ」になる。打ち直せば回避できるので P1
- Issue #18 の B（Backspace 後の子音戻し）は、フォーク側の実装が「入力ログからの再生」ではなく Backspace 専用の後処理（`reclaim_pending_consonant()`）だった。計画どおり Step 10 はこちらで「再生」構造として実装し、先方に rebase してもらう。参考差分 `6a1b1cc` のテスト期待値（`kt` → BS → `a` = 「か」等）は受け入れテストに使う（差分自体はローカルに無い。fork 側）
- Issue #18 の F のうち「Shift+英字が未確定ローマ字を追い越す」は Step 10 に合流。PR #26 は `romaji_input_log` を再生する実装で Step 10 と二重になるため、Step 10 完了後に出し直してもらう。PR #26 の観察（復元する / しないの判定表 `google` / `claude` / `seedreamtsukau`、「境界は未確定バッファを引いた位置で取る」「読みの途中の英単語は対象外」）は本設計の材料
- Step 1（PR #10）は `hiragana_text()` を reading として扱う前提で完了しており、未変換接尾辞（「たt」の `t`）の分離・表示・確定・学習除外は本 Step に持ち越し
- Step 5（L-1）の Backspace テストが「`kt` → Backspace で reading が `k` から空に変わる」を期待している、と旧版に書いていたが、2026-09-10 に確認したところ該当するテストは main に無い（Step 5 のコミット `d7d582f` が追加したのは `live_continuation_*` 6 件のみ）。期待値の更新ではなく、新規テストとして追加する（§7.2）
- 2026-09-08: nick20002005 から本計画書 §5 への回答（Issue #18 コメント）。5-2 / 5-4 は推奨どおりで合意、5-1 は逆引き表を 2 段にする提案、5-3 は **§4.4 の記述がコードと食い違う**との指摘（→ §4.4 で訂正済み）。B の参考差分はテスト期待値だけ使えば十分とのこと
- 2026-09-09: TSF が `.` `,` `/` `[` `]` を記号入力経路へ先に回すため、未確定ローマ字がある状態でこれらを打つと記号が先に確定して未確定分が後ろに残る（`z` + `,` → 「、z」、`j` + `.` → 「j。」。ログで確認）。§3.3 の追い越し経路の 3 つ目として本 Step に取り込む。trie の `z.` `z,` `z/` `z[` `z]` が到達不能になっている原因でもある。リーダー記号の方式そのものは `docs/Symbol_Leader_Input_Plan.md` で別途検討中
- 2026-09-10: 課題を 20 件に洗い出し、依存関係で 5 段階（10-1〜10-5）に分ける案を作った（§9）。10-0（現状確定の調査）を実施し、§2 / §3 の記述を現行コードに合わせて訂正した。主な訂正は、TSF 記号経路の範囲（5 文字ではなく `-` 以外の ASCII 記号すべて）、LiveConv が pending を保持すること、Waiting の表示と engine のずれ、`closes_run` が必要な経路の特定

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

**TSF 側の記号経路**（`crates/rakukan-tsf/src/tsf/factory/dispatch.rs`）: キーボードの記号は `Input(c)` として届いたあと、engine に渡る前に `text_util::direct_input_symbol` で判定される。**`-` 以外の ASCII 記号はすべて `Some` を返し**（`.` `,` `/` `[` `]` は `。` `、` `・` `「` `」`、`?` `!` `~` `\` は `？` `！` `〜` `￥`、それ以外は U+FF01〜 の全角記号）、`on_punctuate` → `push_raw` で積まれる。つまり **キーボードの記号は経路 1・2・4・5 のいずれにも届かない**（2026-09-10 確認）。

キーボードからの経路を整理すると次のとおり。

| 入力 | TSF の経路 | engine の関数 |
|---|---|---|
| 英小文字、数字、`-` | `Input` → `on_input` → RPC `InputChar(Char)` | `push_char` |
| Shift+英字 | `Input` → `on_input` → RPC `InputChar(FullwidthAlpha)` | `push_fullwidth_alpha` |
| `-` 以外の ASCII 記号 | `Input` → `direct_input_symbol` → `on_punctuate` | `push_raw`（全角化済み） |
| テンキー `/` `*` `+` `.` | `InputRaw` → `direct_input_symbol` → `on_punctuate` | `push_raw` |
| テンキー `-` | `InputRaw` → `on_input_raw` | `push_raw('-')` |

この結果、次が成り立つ。

- 経路 1（数値区切り）は engine では到達しない。数値区切りは TSF の `on_punctuate` が `edit_ops.rs` の同名関数 `numeric_separator_after_digit` で判定し、`digit_separator_auto` が有効なら **半角の `,` `.`** を `push_raw` に渡す。engine と TSF に同じ関数が二重にある
- 経路 2（欧文句読点）と経路 4（記号幅）も到達しない。`direct_input_symbol` は常に全角化するので、`symbol_width = halfwidth` はキーボード入力に効いていない可能性がある（※推測。実機未確認）
- 経路 3（数字）は pending が空のときだけ通る。pending がある状態の数字（`k` → `1`）は経路 5 に落ち、`k` `1` とも素通しで **`digit_width` を無視して半角**になる
- 通常入力はすべてこの経路を通るので、`push_raw` の「pending を見ない」性質がキーボード入力に直接現れる。

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

### 2.5 TSF の各状態と pending（2026-09-10 確認）

| 状態 | pending | 記号入力（`on_punctuate`）の表示の組み方 |
|---|---|---|
| Idle / Preedit | 保持 | `engine.preedit_display()` を取り直す |
| LiveConv | **保持する**（`on_input` の LiveConv 分岐が `preview + 接尾辞 + pending` を表示。テスト `live_continuation_appends_pending_romaji_only_to_shown_text`） | `preview + symbol`、読みは `reading + symbol` を TSF 側で組む。engine の読みを取り直さない。同じ状況の `on_input_raw` は `engine.hiragana_text()` を取り直している |
| Waiting | Space 直後で `flush_pending_n` 済み。末尾 `n` は無いが `t` などの子音は残る（`text` = 「たt」） | `text + symbol` を TSF 側で組む |
| Selecting / BlockSelecting / RangeSelect | 空（`force_preedit` で捨てる） | `force_preedit` → `push_raw` |

ライブ変換の起動抑止（`ends_with_pending_romaji`）は BG 変換の開始を止めるだけで、状態は LiveConv のまま残る。旧版の「LiveConv では pending が空」は誤り。

`flush_pending_n` の後に `force_preedit` も `commit` も経ずに Preedit へ戻る経路がある: Space → Waiting → Esc、または Waiting 中に次の文字を打つ経路（`prepare_for_direct_input` が Preedit に戻す）。このとき log には `"n"` が積まれ、変換器は新品、`hiragana_buf` は「…ん」のまま入力が続く。

`force_preedit` の後も入力は続けられ、log は伸び続ける（`hiragana_buf` は log から導出できないまま、後ろに新しいエントリが足される）。

## 3. 問題の分析

### 3.1 G-5 の再現機構（`kt`）

1. `k`: trie 上の有効な経路なので `Buffered`。`pending = "k"`、log は空
2. `t`: buffer `"kt"` は一致なし。先頭 `k` は子を持つが `kt` は経路上にないので `k` を素通し（PassThrough）。`hiragana_buf = "k"`、`log = ["k"]`、`pending = "t"`
3. Backspace: 変換器は buffer の `t` を消して `RemovedBuffer`。engine は `pending` を空にするだけ。`hiragana_buf = "k"`、`log = ["k"]` のまま
4. `a`: `pending = "a"` → 「あ」。結果 `"kあ"`

素通しした `k` が「確定済み」側に固定され、Backspace が入力原本（`ka`）から状態を作り直さないのが原因。`nt`（`n` が「ん」に確定）、`tt`（「っ」に確定）も同じ構造。

### 3.2 現行 Backspace が壊す不変条件

- `RemovedOutput` で `hiragana_buf` を 1 文字、log を 1 要素消す。log 要素は 1 文字とは限らない（`"kya"` → 「きゃ」）。「きゃ」から「ゃ」だけ消すと log から `"kya"` 全体が消え、`hiragana_buf = "き"` に対して log は空になる。この状態で F9 を押すと `k i` が復元されない（2026-09-10 にコードで確認。`push_char` が `kya` を 1 エントリで積み、`RemovedOutput` で 1 エントリ pop する。実機未確認）。同じ理由で `sha` → BS → F9 の現行結果は `si` ではなく「し」のまま（復元が空文字になり `pending_suffix` に「し」がそのまま入る）

### 3.3 未確定ローマ字を追い越す入力（Issue #18 F、および TSF 記号経路）

`push_raw` と `push_fullwidth_alpha` は pending を見ない。`k` → Shift+`A` で `hiragana_buf = "A"`、`pending = "k"` となり、表示は `"Ak"`、原本は `kA`。順序が入れ替わる。

キーボードの `.` `,` `/` `[` `]` も §2.2 の TSF 記号経路で `push_raw` に落ちるため同じ構造になる。ログで確認した実例（2026-09-09）:

| 打鍵 | 表示 | 期待 |
|---|---|---|
| `z` `,` | 「、z」 | 「‥」（trie の `z,`）または「z、」 |
| `z` `.` | 「。z」 | 「…」（trie の `z.`）または「z。」 |
| `j` `.` | 「j。」ではなく「。j」 | 「j。」 |

`n` + `.` の実例はログに無く、「ん。」になるか「。n」になるかは未確認。§4.4 の閉じ方で「ん。」に確定させる。

2026-09-10 の確認で分かった同系統の問題:

- **pending がある状態の数字**は追い越さないが、経路 5 に落ちて `digit_width` を無視する（§2.2）。`k` → `1` = 「k1」（全角設定でも半角）
- **LiveConv 中の記号**: `on_punctuate` の LiveConv 分岐は `preview + symbol` で表示を組むため、pending の `k` が表示から落ちる。engine 側には pending が残るので、次の母音で「か」として現れる。engine で pending を閉じるようにすると、TSF 側が組む `reading + symbol` と engine の読みがずれるので、`on_input_raw` と同様に engine の読みを取り直す必要がある
- **Waiting 中の記号**: TSF は `text + symbol` = 「たt。」を表示するが、現行 engine は `push_raw` で「た。」+ pending `t` となり engine の表示は「た。t」。**今もずれている**。engine で pending を閉じれば「たt。」で一致する

### 3.4 復元関数の入力種別の欠落

`hiragana_from_romaji_log` は log の全文字を変換器に流す。log が「何として入力されたか」を持たないため、次が起きる（2026-09-10 にコードで確認。実機未確認）。

- Shift+英字で積んだ `"A"` は変換器が小文字化するので「あ」になる。後続の母音と結合すれば `Bi` → 「び」のように 2 文字が消える。例: 「るーむＡしつ」→ F9 → F6 で「るーむあしつ」
- 数字は trie に無く素通しなので、`digit_width = fullwidth` でも半角に落ちる。例: 「１,０００.０」→ F9 → F6 で「1、000。0」
- 桁区切りとして半角のまま積んだ `,` `.`（TSF の `on_punctuate` 経由）は trie の「、」「。」になる。engine の経路 1 で積んだ `","` も同じ（テスト経路のみ）
- `flush_pending_n` で積んだ `"n"` は、再生時に `n` 単独の規則が無く素通しになる。例: `kon` → F9 → F6 で「こn」
- 全角化されて `push_raw` に渡った「、」「。」「？」は trie に規則が無く素通しなので影響しない。例: 「かれは、おとこだ。」は現状も正しく戻る

### 3.5 Space 変換時の未変換接尾辞（「たt」）

`hiragana_text()` = 「た」、`pending` = `t`。ライブ変換は `ends_with_pending_romaji` で起動を抑止している。Space 変換では reading「た」で候補を引き、接尾辞 `t` を表示・確定に残し、学習キーには含めない、という扱いを本 Step で確定する。TSF 側の候補ビューには `corresponding_reading_len` / `suffix_len` が既にあり（Step 1）、接尾辞を表示する土台はある。末尾 `n` は `flush_pending_n` で「ん」にしてから変換している。

## 4. 設計

### 4.1 入力原本に種別を持たせる

`romaji_input_log: Vec<String>` を `Vec<InputEntry>` に変える。

```rust
struct InputEntry {
    /// ユーザーが打った文字（そのまま）。"kya" / "1" / "A" / "。"
    typed: String,
    /// このエントリが hiragana_buf に足した文字列。"きゃ" / "１" / "Ａ" / "。"（2026-09-10 の案で追加）
    output: String,
    kind: InputKind,
    /// このエントリでローマ字区間が閉じた（flush_pending_n / §4.4 で閉じたとき）
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

- 不変条件 1 は `entries.iter().map(typed).concat() + pending == 打鍵文字列` のまま
- 不変条件 2（案）: detach していない状態で `entries.iter().map(output).concat() == hiragana_buf`
- `romaji_log_str()` の戻り値（`typed` の連結）は変えない。F9 / F10 の呼び出し側は無変更
- `hiragana_from_romaji_log()` は **`output` の連結**にする案（2026-09-10）。変換器に流し直さない。旧版の「種別ごとに幅規則を再計算する」案は、`push_char` の各経路が追記した文字列をその場で持っているので不要になり、設定がセッション中に変わっても記録時の結果を保てる。これで 3.4 が解消する。採否は未定
- `output` は 10-4 の逆引き（`kya` が「きゃ」を出したことを知る）にも使える
- 種別は 6 種のまま残す案。キーボードから来るのは `Romaji` / `Digit` / `Raw` / `ShiftAlpha` の 4 種（§2.2）だが、`Separator` / `Symbol` は `push_char` に経路と単体テストがある。4 種に絞る選択もあり、未定

### 4.2 再生関数を分離する

Backspace 専用の分岐ではなく、入力原本から状態を作るテスト可能な純粋関数を置く。§4.1 の `output` 連結案を採る場合、復元（F6〜F10）にはこの関数は不要で、Backspace 再生（10-3）のためだけに使う。

```rust
/// Romaji 区間の打鍵列を先頭から流し、(確定かな, 未確定ローマ字) を返す
fn replay_romaji_run(typed: &str) -> (String, String, RomajiConverter)
```

- 呼び出し側は「最後の非 Romaji エントリより後ろ」の区間だけを再生し、それより前の `hiragana_buf` はそのまま残す。数字・記号・Shift 英字・直接入力を通常ローマ字として再解釈しない（計画の要件）
- `closes_run == true` のエントリでは変換器を `flush` して区間を閉じる。`flush_pending_n` で「ん」にした `n` が、再生時に未確定 `n` に戻らないようにする。**必要性は確認済み**（§2.5）: Space → Waiting → Esc で Preedit に戻り `k` を打って Backspace すると、`closes_run` が無ければ `kon` を再生して「こ」+ pending `n` になり「こん」に戻らない
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

**pending が空のとき**: 表示上の 1 文字を消す現行動作を維持する（「きゃ」→「き」）。ただし log を要素ごと捨てず、消した後に残るかなに対応する打鍵に書き換える（`"kya"` → `"ki"`）。逆引きできない場合（直接入力の記号など）は typed をそのまま 1 文字削る。

残骸の打鍵列の作り方は 2 段にする（2026-09-08 の提案を採用候補とする）:

1. **typed の接頭辞を再生して残骸に一致するものを探す。** 促音由来（`tta` → 「った」→ BS → 「っ」）は `tt` を再生すると output が「っ」になるので、打鍵に沿った綴りが得られる。逆引きだと `xtu` / `ltu` になり打鍵と無関係になる
2. **見つからなければ かな → ローマ字の逆引き表を引く。** 拗音由来（`kya` → 「きゃ」→ BS → 「き」）は typed の接頭辞（`k` / `ky`）が出力を持たないので逆引きが要る。`rules.rs` は多対一（`ci` / `si` / `shi` → 「し」）なので、**元の綴りと接頭辞を共有する候補を優先する**（`sha` → BS → `shi`。`si` にしない）

この方式では §2.1 の不変条件は「打鍵原本」から「現在の表示を再生できる打鍵列」へ意味が変わり、**F9 / F10 が打った綴りをそのまま返さなくなる**場合がある（`sha` → BS → F9 は `shi`）。挙動変更なのでテストで固定する（§7.1）。

この分岐が本 Step で決める最大の UX 判断（5. の 1 参照）。「1 打鍵戻す」に統一すると「きゃ」→ BS が `ky`（表示 `ky`）になり、MS-IME の「ゃ」だけ消える挙動から離れる。

**force_preedit 後**: F7 でカタカナにした後などは `hiragana_buf` が log から導出できない。`force_preedit` の後も入力は続けられ log は伸びる（§2.5）ので、フラグではなく **detach の境界位置**（`entries.len()` を記録するか、境界エントリを積む）で表し、再生は境界より後ろの区間に限る案（2026-09-10）。境界より前に Backspace が及ぶときは現行の単純 pop に留める。F6〜F10 のサイクルは既存の復元関数が log から作り直すので影響しない。RangeSelect / BlockSelecting からの復帰も同じ扱い。

### 4.4 未確定ローマ字を追い越す入力（`push_raw` / `push_fullwidth_alpha`）

pending が空でない状態でこれらが呼ばれたら、先に pending を閉じてから追記する。閉じ方は次の順（エントリに `closes_run = true` を付ける）。

1. `flush_pending_n`（pending がちょうど `n` なら「ん」）
2. 残った pending を `RomajiConverter::flush` で閉じる（`k` → `k` 素通し）
3. 記号 / Shift+英字を追記

**訂正（2026-09-08 指摘）**: 旧版は「`flush` が `n` を「ん」にする」と書いていたが誤り。`rules.rs` に `n` 単独の規則は無く（`nn` / `n'` / `xn` のみ）、`flush()` は一致しない先頭文字をそのまま素通しするので、`flush` だけで閉じると `n` → Shift+`A` は「nA」になる。「ん」にしているのは engine の `flush_pending_n()` だけ。上記 1 → 2 の順で「んA」「ん。」にする（§5 の 3）。

これで表示順と原本の順が一致する（`k` → Shift+`A` = `kA`、`k` + `.` = 「k。」、`j` + `.` = 「j。」）。

対象は `push_raw` / `push_fullwidth_alpha` に加え、**`push_char` の経路 3（数字）**（2026-09-10 追加。§2.2 の「pending 中の数字が `digit_width` を無視する」を直すため。経路 1・2・4 も同じ形にしておく）。いずれも engine 内部で行う（TSF 記号経路も `push_raw` に落ちるので自動的に直る）。engine の公開 API は `flush_pending_n` しか無く、素通しで閉じる API を TSF 側から呼ぶには ABI 追加が要るため、engine 内部に置く方が小さい（§4.6）。

**TSF 側で必要な追随**（2026-09-10 追加）: `on_punctuate` の LiveConv 分岐は `reading + symbol` を TSF 側で組んでいる（§2.5）。engine が pending を閉じると読みが `reading + k + symbol` になり食い違うので、`on_input_raw` と同じく `push_raw` 後に `engine.hiragana_text()` を取り直す形に変える。PR #31 の `tail_delta` / `apply_tail_delta`（engine の読みの末尾差分を表示に当てる部品）も候補だが、取り直し方式で足りる見込み（§10）。Waiting 分岐（`text + symbol`）は閉じた後の engine と一致するので変更不要。

**`z` + 記号の例外**: trie の `z.` `z,` `z/` `z[` `z]` を生かす場合は、pending がちょうど `z` で `.` `,` `/` `[` `]` が続くときだけ、閉じずに trie へ流す（TSF `dispatch.rs` で `on_punctuate` ではなく `on_input` に回す）。採用するかは `docs/Symbol_Leader_Input_Plan.md` §5 の判断に従う。採用しない場合は `z` も上記 1 → 3 で「z、」になる。

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

2026-09-08 の nick20002005 の回答（Issue #18）を併記。2026-09-12 時点: **1 は確定**（下記）。2〜4 は推奨値で実装済み（10-2 / 10-3）だが判断の記録は未。5 は「例外を入れない」で実装（10-2。テスト 1 件の差し替えで変更可）。6 は「10-2 に含める」で実施済み。

**§5-1 の判断（2026-09-12）**: A「表示 1 文字を消す＋log 書き換え」を採用。逆引きの優先順は a′「元の綴りと共有する接頭辞が最長のもの（同点は短い方）。ただし残るかなが母音 1 文字なら母音字 1 文字」。a（母音の例外なし）だと `wi` → BS → F9 が `wu` になり、F9 → BS → `i` で「ｗい」が残る実害があるため例外を足した。閉じる範囲は i「pending 空の Backspace を通った区間の末尾エントリを常に閉じる」（10-3 の再生が `t` → 「っ」のような再生不能エントリを流し直して崩れるのを防ぐ）。

1. **pending が空のときの Backspace**: 「表示 1 文字を消す（現行、log は書き換え）」か「1 打鍵戻す（`きゃ` → `ky`）」か。推奨は前者（4.3）。**先方も前者に賛成**。打鍵列の作り方は「typed の接頭辞再生 → 逆引き」の 2 段（4.3）を提案。F9 / F10 の挙動変更をテストで固定することも提案
2. **末尾 `n`**: `flush_pending_n` で「ん」に確定する現行維持（推奨）か、接尾辞として残すか。**先方も現行維持に賛成**
3. **`push_raw` / `push_fullwidth_alpha` で pending を閉じるとき** `n` を「ん」にするか素通し `n` にするか。推奨は `flush_pending_n` → `flush` の順で「ん」（4.4 で訂正済み。旧記述の「`flush` で「ん」」はコードと不一致）。**先方も「んA」を推奨**
4. **`force_preedit` 後の Backspace**: 現行の単純 pop で据え置く（推奨）か、log から再構築するか。**先方も据え置きに賛成**
5. **`z` + 記号を trie に流す例外（4.4）を入れるか**: `docs/Symbol_Leader_Input_Plan.md` §5 の 1 に従う。未定
6. **TSF 記号経路の修正を Step 10 本体と切り離して先行させるか**: `push_raw` 内で閉じる変更（4.4 の 1 → 3）は engine の 1 関数で済み、ABI 変更なし。`j` + `.` → 「。j」の実害があるため先行も可。未定

## 6. 変更対象

- `crates/rakukan-engine/src/lib.rs`: `InputEntry` / `InputKind` の導入、`push_char` 5 経路と `push_raw` / `push_fullwidth_alpha` / `flush_pending_n` / `force_preedit` の log 更新、`backspace` の書き直し、`replay_romaji_run` の追加、`hiragana_from_romaji_log` の種別対応、かな → ローマ字逆引き表（5. の 1 で前者を採る場合）
- `crates/rakukan-engine/src/romaji/converter.rs`: 変更なしの見込み。必要なら区間再生用のヘルパーを足す
- `crates/rakukan-tsf/src/tsf/factory/on_convert.rs`: 接尾辞の表示・確定・学習除外（4.5）
- `crates/rakukan-tsf/src/tsf/factory/dispatch.rs`: `z` + 記号の例外（5. の 5 で採用した場合のみ）
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
- `k` → Shift+`A` の表示が `kA`（順序が入れ替わらない）。`n` → Shift+`A` は「んA」（5. の 3）
- `k` + `.` = 「k。」、`j` + `.` = 「j。」、`n` + `.` = 「ん。」（TSF 記号経路。§3.3 の実例）。`z` + `,` は 5. の 5 に従い「‥」または「z、」
- `sha` → BS → F9 = `shi`（`si` にしない。4.3 の接頭辞優先）、`tta` → BS → F9 = `tt`（逆引きの `xtu` にしない）
- 数字・記号・数値区切り・欧文句読点・直接入力の後の Backspace が現行どおり 1 文字消え、log と一致する
- Shift+英字 `A` と数値区切り `,` を含む log から `hiragana_from_romaji_log` が「あ」「、」を作らない
- `force_preedit` 後の Backspace が現行どおり 1 文字消す
- 全経路で `log + pending == 打鍵列` を保つ性質テスト（ランダムな英数記号列を流して確認）

### 7.2 TSF 単体（`cargo test -p rakukan-tsf --lib`、PowerShell から）

- `kt` → BS で reading が空、pending が `k` になるテストを新規に追加する（Step 5 由来の既存テストは無い。§1）
- Step 1 の「たt」「やまのたn」: reading と接尾辞の分離が変わらないこと、接尾辞が候補表示・確定に残り学習キーに含まれないこと

### 7.3 実機

- `kt` / `nt` / `tt` の打ち直しが Backspace 1 回で直る
- F6〜F10 のサイクル（Shift+英字、数字、記号を含む文字列）が崩れない
- RangeSelect / BlockSelecting / F7 から戻った後の Backspace が現行どおり

## 8. 完了条件（計画本文の再掲）

- Backspace 後の表示結果、変換器、入力ログ、未確定ローマ字が同じ入力列を表す
- 誤入力を削除した後の次の文字が、削除前の派生出力に妨げられず、残ったローマ字と結合される

## 9. 実施段階と着手順（案、2026-09-10）

課題 20 件を依存関係で 5 段階に分けた案。全段階が「入力ログに種別を持たせる」を土台にするため 10-1 を先に置き、実害が出ている追い越しを次に、UX 判断が最大の pending 空時の Backspace は再生の仕組みができてから載せる。Space 接尾辞は TSF 側で完結し独立なので最後。段階分けと順序は案で、着手の判断は未定。

| 段階 | 内容 | 主な課題 | 完了条件 | 状態 |
|---|---|---|---|---|
| 10-0 | 現状確定（調査のみ） | 経路の実態、各状態の pending、Waiting の表示、Step 5 テストの所在、`closes_run` の要否 | §2 / §3 が実態に合い、10-1〜10-3 の設計に必要な事実がそろう | 完了（本更新で反映） |
| 10-1 | 入力ログの種別化と復元 | §3.4、F9 の接尾辞切り出し、性質テスト | 挙動変更なし（F6〜F10 の復元の修正のみ）。`hiragana_from_romaji_log` が detach していない状態で常に `hiragana_buf` と一致 | 実装済み（2026-09-12。engine 187 / tsf 99 件通過、check・clippy・fmt クリーン。実機未確認、未コミット） |
| 10-2 | 追い越し防止 | §3.3、pending 中の数字、`on_punctuate` LiveConv 分岐、TSF / engine の区切り判定の二重実装 | `k` + `.` = 「k。」、`n` + `.` = 「ん。」、`k` + `1` が `digit_width` に従う | 実装済み（2026-09-12。engine 202 / tsf 99 件通過、clippy・fmt クリーン。実機未確認、未コミット） |
| 10-3 | Backspace 再生（pending あり） | §3.1、`force_preedit` 後の据え置き、`closes_run`、TSF テスト追加 | `kt` / `nt` / `tt` → BS → `a` が「か」「な」「た」 | 実装済み（2026-09-12。engine 218 / tsf 99 件通過、clippy・fmt クリーン。実機未確認、未コミット） |
| 10-4 | Backspace（pending なし）の log 書き換え | §3.2、2 段方式の具体化、F9 の回復、回帰テスト | `kya` → BS → F9 = `ki`、`sha` → BS → F9 = `shi`、`tta` → BS → F9 = `tt` | 実装済み（2026-09-12。engine 231 / tsf 99 件通過、clippy・fmt クリーン。実機未確認、未コミット） |
| 10-5 | Space 変換の未変換接尾辞 | §4.5 | 「たt」の `t` が候補と確定に残り、学習キーに入らない | 実装済み（2026-09-12。tsf 102 件通過、clippy・fmt・check クリーン。実機未確認、未コミット） |

10-0 と 10-1〜10-4 は engine 内で閉じ、ABI 変更は不要な見込み。10-2 で `edit_ops.rs`（`on_punctuate`）、10-5 で `on_convert.rs` に及ぶ。

### 9.1 10-1 の計画（2026-09-12 実装済み。`output` 案と 6 種別を採用）

範囲は `crates/rakukan-engine/src/lib.rs` の入力ログとその周辺関数のみ。`RomajiConverter`、TSF、RPC、ABI は触らない。Backspace の挙動は現行のまま。

変更点:

| 箇所 | 現状 | 10-1 後 |
|---|---|---|
| ログの型 | `romaji_input_log: Vec<String>` | `Vec<InputEntry>`（§4.1） |
| `push_char` 経路 1〜4 | `log.push(c)` | `typed = c`、`output = 追記した文字`、`kind` は経路ごと |
| `push_char` 経路 5 | `log.push(entry)` | `typed = entry`、`output = added`、`kind = Romaji` |
| `push_raw` | `log.push(c)` | `typed = output = c`、`kind = Raw` |
| `push_fullwidth_alpha` | `log.push(c)` | `typed = c`、`output = 幅適用後の文字`、`kind = ShiftAlpha` |
| `flush_pending_n` | `log.push("n")` | `typed = "n"`、`output = "ん"`、`closes_run = true` |
| `backspace` | `log.pop()` | `entries.pop()`（分岐と回数は同じ） |
| `romaji_log_str` | `log.concat()` | `typed` の連結（結果は同じ） |
| `hiragana_from_romaji_log` | 変換器に流し直し | `output` の連結 |

操作上変わるのは F6〜F10 で文字種を変えたあと元に戻す場面だけ:

| 操作 | 現状 | 10-1 後 |
|---|---|---|
| 「るーむＡしつ」→ F9 → F6 | るーむあしつ | るーむＡしつ |
| 「１,０００.０」（`digit_separator_auto` 有効）→ F9 → F6 | 1、000。0 | １,０００.０ |
| 「１、０００。０」（無効）→ F9 → F6 | 1、000。0 | １、０００。０ |
| `kon` → F9 → F6 | こn | こん |
| 「かれは、おとこだ。」→ F9 → F6 | かれは、おとこだ。 | 変わらない |

現状側はいずれもコード上の読みで、実機未確認。F9 / F10 の 1 回目の変換結果とサイクルは変わらない。区切り文字の種類（`,` と「、」）は入力時に TSF が決めたものをそのまま戻すだけで、10-1 では変えない。

作業手順:

1. 性質テストを先に書き、現行コードで不変条件 2 が落ちることを確認する
2. 個別テスト: Shift+英字の復元、桁区切りの復元、`kon` + `flush_pending_n` 後の復元が「こん」、既存の `qwrty` 2 件の維持
3. データ構造を置き換え、各関数の log 操作を移す
4. 復元関数を `output` 連結に置き換える
5. `cargo make check`、`cargo clippy -p rakukan-engine -p rakukan-tsf --all-targets`、`cargo test -p rakukan-engine --lib` と `cargo test -p rakukan-tsf --lib`（PowerShell から）
6. 実機: ビルド → サインアウト → サインイン → install の順で入れ、Shift+英字・数字・桁区切りを含む文字列で F9 → F6 の往復が元に戻ること、通常入力と Backspace が現行どおりであることを確認
7. 本計画書と `September_Revised_Plan.md` に完了を記録

10-1 完了時点でも Backspace の `RemovedOutput` 後は不変条件 2 が崩れる（10-4 までの既知事項）。性質テストは Backspace を除く経路で固定し、10-4 で Backspace を加える。

実装メモ（2026-09-12）: 性質テスト 1（typed + pending == 打鍵列）は、pending がある間の Raw / Shift 入力で現状も崩れる（追い越し。10-2 の対象）ため、生成器でその組み合わせを除いて固定した。10-2 でこの制限を外す。テストは `input_log_tests` モジュール（性質 2 件、個別 7 件）。

着手前に決めること: `output` を持たせる案の採否（§4.1）、`Separator` / `Symbol` を種別として残すか。

### 9.2 10-2 の実装メモ（2026-09-12）

- engine: `close_pending()` を追加（`flush_pending_n` → `RomajiConverter::flush` → 変換器リセット。閉じたエントリは `closes_run = true`）。`push_raw` と `push_fullwidth_alpha` の先頭、および `push_char` で **数字と trie 外の ASCII 記号** が来たときに呼ぶ。英字と `,./[]\-` は従来どおり trie に委ねる（`k` + `-` = 「kー」、`n` + `,` = 「ん、」。`z,` 系の例外を将来入れられるよう engine の trie 経路は残した）
- 前提（§5 は未決のまま）: §5-3 は「ん」（`n` + `.` = 「ん。」）、§5-5 は例外なし（`z` + `、` = 「z、」）。テスト `raw_symbol_after_z_has_no_leader_exception` で固定しているので、§5-5 で例外を採る場合はこのテストを差し替える
- TSF: `on_punctuate` の LiveConv 分岐を `on_input_raw` と同じ形にし、`push_raw` 後に `engine.hiragana_text()` を取り直して `live_continuation_display` で表示を組む（`on_input.rs` の同関数を `pub(super)` に変更）。Waiting / Idle / Preedit / Selecting / BlockSelecting の分岐は変更なし。PR #31 の `tail_delta` は使わなかった
- TSF / engine の区切り判定の二重実装（`numeric_separator_after_digit`）は整理していない。`1` `k` `,` が「1k,」になる件（TSF が pending を含めずに「数字の直後」と判定）は残る
- テスト: `close_pending_tests` モジュール 15 件。`input_log_tests` の性質テスト 1 から 10-1 の制限を外した（pending 中の Raw / Shift を含めても `typed + pending == 打鍵列` が成立）
- 実機での確認項目: `k` `.`、`j` `.`、`n` `.`、`k` Shift+`A`、`k` `1`、ライブ変換中の `k` `.`、Waiting 中（Space 直後）の `.`、テンキー `-`

### 9.3 10-3 の実装メモ（2026-09-12）

- engine のみ。`backspace()` の先頭で、pending が空でなければ `replay_backspace()` を試み、成功したら現行の 3 分岐に入らない
- `replay_backspace()`: 末尾のローマ字区間（`romaji_run_start()`: 最後の非 `Romaji` エントリ・`closes_run`・detach 境界の直後から）の `typed` + pending から末尾 1 打鍵を消し、`replay_romaji_run()` で新しい変換器に流し直して `input_log` / `hiragana_buf` / `pending_romaji_buf` / 変換器を置き換える。区間の `output` 連結が `hiragana_buf` の末尾と一致しないとき（10-4 未対応の「きゃ」→ BS 後など、log と表示がずれている状態）は何もせず false を返し、現行の単純 pop に落ちる
- `romaji_step()`: `push_char` 経路 5 の 1 文字分の処理（`output` / `buffer` の差分でエントリを切る）を切り出し、再生と共用する
- detach 境界は `log_detached_at: usize`（`force_preedit` 時の `input_log.len()`）。`log_pop()` で clamp、`log_clear()` で 0 に戻す。§4.3 の「境界の位置で持つ」案を採用
- TSF 側の変更なし。`kt` → BS で reading が空、pending が `k` になる確認は engine テスト `kt_bs_leaves_k_pending` で行う（TSF 単体テストは engine を持たないため）
- テスト: `backspace_replay_tests` 16 件。性質テスト 2 件に「pending があるときの Backspace」を加え、`typed + pending == 打鍵列` と `output 連結 == hiragana_buf` が Backspace 再生後も成り立つことを確認
- 実機での確認項目: `kt` / `nt` / `tt` → BS → `a`、`kanakq` → BS → `a`、F7 の後の Backspace、Space → Esc → 入力 → BS、ライブ変換中の BS

### 9.4 10-4 の実装メモ（2026-09-12）

- engine のみ。`backspace()` の pending 空分岐を `pop_display_char()` に置き換え。表示末尾 1 文字を消したあと、末尾エントリが表示と対応していれば（非 detach かつ `output` が消した文字で終わる）log を書き換える。対応していなければ現行どおり 1 エントリ消す
- 1 段目 `replay_prefix_for()`: 区間の打鍵列の接頭辞を長い順に再生し、出力が「区間の出力 − 1 文字」に一致するものを探す。未確定が残らない一致（`kata` → `ka`）を優先し、無ければ未確定を末尾エントリの打鍵に含める（`tta` → `tt`、`nta` → `nt`、`nna` → `nn`）
- 2 段目 `reverse_spelling()`: `romaji::spellings_for()`（trie を走査して作る逆引き表、`rules.rs`）から候補を取り、母音字 1 文字があればそれ、無ければ共有接頭辞最長 → 短い方 → 辞書順（`sha` → `shi`、`sya` → `si`、`tya` → `ti`、`fa` → `fu`、`wi` → `u`）
- 3 段目: 末尾エントリの打鍵から 1 文字削る（逆引きも無い場合。テストでは到達していない）
- 書き換えた区間の末尾は `closes_run = true`。1 文字エントリを丸ごと消した場合も、残る区間の末尾を閉じる。pending 空の Backspace 後は変換器を新品にする
- 10-3 の既知の崩れ（`tta` → BS → `k` → BS が `t`）はこの閉じる処理で解消。テスト `rewritten_entry_is_closed_so_replay_does_not_reopen_it`
- F9 の綴りが打鍵と変わる例: `sha` → BS → F9 = `shi`（旧版は復元不能で「し」のまま）、`nta` → BS → F9 = `nt`
- テスト: `log_rewrite_tests` 12 件、`rules.rs` の逆引き表 1 件。性質テスト 2（`output` 連結 == `hiragana_buf`）の Backspace を「常に」に広げた。性質テスト 1（`typed` + pending == 打鍵列）は書き換え後に打鍵原本と一致しなくなるので、pending があるときの Backspace に限ったまま
- 実機での確認項目: `kya` → BS → F9、`sha` → BS → F9、`tta` → BS → F9、`wi` → BS → F9 → BS → `i`、`tta` → BS → `k` → BS、長い読みを Backspace で全部消す

### 9.5 10-5 の実装メモ（2026-09-12）

- TSF のみ。engine は変更なし
- **現状の挙動（コード上の読み）**: Space 時の `original_preedit` に `preedit_display()`（「たt」）を渡していたため、候補は reading「た」で引かれるのに Selecting の読みは「たt」になり、(1) 候補表示で `t` が消える、(2) Enter の確定文字列からも `t` が消える、(3) 学習キーが「たt」になる、(4) Waiting からの復帰（dispatch の waiting-poll）で `bg_take_candidates("たt")` がキー不一致になる、という 4 つの問題があった
- 変更: `on_convert` の Space 経路で `conv_reading = hiragana_text()` と `pending_suffix = pending_suffix_display(preedit, reading)` を求め、Selecting の `original_preedit` を reading、`remainder` を接尾辞（`remainder_reading` は空）として起動する。既存の描画（`update_composition_candidate_parts(prefix, 候補, remainder)`）、候補送り、Enter（`confirmed + remainder`）、Esc（`prefix + original + remainder`）がそのまま接尾辞を扱う。学習は `original_preedit` = reading で行われる
- 接尾辞の幅: `text_util::pending_suffix_for_display(preedit, reading, fullwidth)`（純関数、テスト 3 件）と `state::pending_suffix_display`（`[input] alpha_width` を読む）。reading が空、または preedit が reading で始まらないときは接尾辞なし
- 起動箇所の変更: `on_convert` の Space 経路 5 か所（`activate_selecting_snapshot_*` に `suffix` 引数を追加、フォールバック候補 `vec![preedit]` は `vec![reading]` に）、`dispatch.rs` の waiting-poll（hiragana_text でのキー再試行を追加し、取れたキーを読みにする）、`candidate_window.rs` の `on_waiting_timer`（取れたキーを読みにし、接尾辞を remainder の前に置く）
- `on_backspace` の Selecting 分岐は `prefix + original + remainder` を表示するよう on_cancel と揃えた（従来は original だけで、接尾辞が消えていた）
- `SessionState::activate_selecting`（affix 無し版）は使われなくなったので削除し、テスト 2 件を `activate_selecting_with_affixes` に変更
- 末尾 `n` は従来どおり `flush_pending_n` で「ん」にしてから変換する（§5-2）
- 実機での確認項目: 「た」+ `t` → Space（候補の後ろに `ｔ` が付く）、Enter（「田ｔ」が確定し、学習は「た」→「田」）、Esc（「たｔ」に戻る）、Backspace、辞書候補が無い読みで Waiting → 候補到着、`alpha_width = halfwidth` での接尾辞

## 10. 関連 PR との関係（2026-09-10）

- **PR #31**（記号連打の畳み込み）: #35 の方式待ちで保留。nick20002005 から「`tail_delta` / `apply_tail_delta` を #34 §4.4 で使うなら切り出して単体で出す」と申し出あり。10-2 の `on_punctuate` LiveConv 分岐（§4.4）の部品候補だが、`on_input_raw` と同じ取り直し方式で足りる見込みなので、10-2 の設計時に要否を決める。返答は未
- **PR #41**（推論失敗で Space が固まる、GPU デバイス消失からの復帰）: 変更ファイルは `conv_cache.rs`、`state.rs`、`candidate_window.rs`、`on_convert.rs`。10-1〜10-4 の変更箇所（engine `lib.rs`、`edit_ops.rs`）とは重ならない。10-5 で `on_convert.rs` に及ぶ時点で衝突の可能性がある
