# 8月ログ分析にもとづく改善計画 第2版

作成: 2026-09-01

対象: v0.10.5 以降

位置づけ: `AUGUST_LOG_IMPROVEMENT_PLAN.md` の内容を保持し、2026-08-30 時点の GitHub Issue / PR とコードレビュー結果を統合した実行計画

方針決定反映: 2026-09-01（学習優先順位、旧学習履歴の破棄、auto backend fallback）

## 1. この版の目的

第1版は、2026年8月の運用ログから判明した次の3件を中心に整理した。

- `live_continuation_guard` の誤発動
- echo strip のカタカナ語誤爆
- モード切替時の同期 keymap 再読込

第2版では、第1版の分析と改善案を引き継いだうえで、同時期に報告された GitHub Issue と未マージPRを統合する。ログ由来の改善だけを先行すると、ユーザー辞書がSpace変換で反映されない問題や、`gpu_backend = "auto"` でIMEが起動不能になる問題が残るため、ユーザー影響と修正の依存関係にもとづいて実施順を組み直す。

この文書は計画書であり、Issueへの回答、PRへのレビュー投稿、マージ、バージョン変更は含まない。

### 開発段階の後方互換方針

現在はテスト開発中のため、旧バージョンとの後方互換性は原則として完了条件にしない。既存API、protocol、設定形式、永続データ形式は、現在の設計を単純かつ検証可能にするために変更または削除してよい。

ただし、旧形式を暗黙に新形式として読み込んで誤動作させない。旧形式を破棄する場合は形式を明確に判別し、破棄した事実をログへ残して、安全な初期状態から継続する。正式な互換性保証を開始する時点で、対象versionと移行方針を別途定義する。

## 2. 第1版からの主な変更

1. GitHub Issue #1 / #2 / #3 / #6 / #8 / #9 / #11 / #13 を計画対象に追加した。
2. PR #4 / #5 / #7 / #10 のレビュー結果と、PR #12 の要レビュー状態を記録した。
3. PR #10、#5、#7 に残る問題を、マージ前の受入条件として具体化した。
4. 既存のABI検査とDLLログを踏まえ、Issue #8を「新規実装」ではなく「現行診断経路の再現確認と不足分の補強」に変更した。
5. 変更範囲が異なる修正を一つのバージョンへ詰め込まず、TSF入力、起動安全性、変換品質、候補UI、学習のリリース単位に分離した。
6. バージョン番号は実装開始時に決定し、変更時は `docs/version-update-checklist.md` を作成して更新対象を管理する方針にした。
7. 2026-09-01 にPR #4 / #5 / #10 / #12 の差分と現行コードを照合し、R-1 / R-2 / R-3 / R-5 / G-1 / L-3 へ補足を追記した（PR #4 / #5 / #10 の head は 2026-08-30 時点から変更なし）。
8. G-5（ローマ字入力状態の再構築）をP0からP1へ下げ、Step 1からStep 10へ移動した。PR #10（新 Step 1）が必要とする reading / 未変換接尾辞の分離は現行の `hiragana_text()` / `pending_romaji_buf` で既に成立しており、G-5 に依存しないため。各Stepに修正負荷の見積りを付記した。

## 3. 根拠と現状

### 3.1 8月ログの再計測

2026年8月の運用ログ（`rakukan.log` 6世代、`rakukan-engine-host.log`、`rakukan-engine-dll.log` 2世代、計約100MB）を分析した。ERROR・パニック・クラッシュは0件。host再起動は60回/月（約2回/日）で、再起動ストームは再発していない。

0.10.xの実機反映は8月7日頃であり、導入前後の比較は次のとおり。

| 指標 | 7月 | 8/1–7 | 8/7–31 | 判定 |
|---|---:|---:|---:|---|
| `engine::init: loading model` | 800回/月 | 132回 | 37回（約1.5回/日） | 目標達成 |
| `take_ready MISMATCH` | 409回/月 | 15回 | 0回 | 目標達成 |
| `learn: dict_store not initialized` | 未集計 | 206回 | 0回 | 学習ロス解消 |
| `engine busy` / `not ready` | 6回/月 | 未分離 | 9回/月 | 横ばい |
| echo strip発動 | 3,182回/月 | 1,212回 | 1,689回 | 改善せず |
| `live_continuation_guard` fallback | 330回/月 | 未分離 | 160回/月 | 半減したが残存 |
| SLOW OnKeyDown >200ms | 14回/月 | 未分離 | 15回/月 | 横ばい |

追加の観測:

- `end_composition: SetText failed`（`TS_E_READONLY`）は5件で、すべてFirefox。0.10.2の保全パスが機能しているため、現時点では追加修正を行わず監視を継続する。
- SLOW OnKeyDownは420件、p50=9ms、p95=146ms、最大931ms。
- Convertは258件、p50=14ms、p95=171ms、最大502ms。
- エンジン側beam変換は8月7日以降20,983件、p50=50ms、p95=88ms、p99=126ms、最大817ms。

### 3.2 GitHubの現状

2026-09-01確認時点で、対象Issue 8件とPR 5件はすべてOPEN。

| Issue | 内容 | 対応PR | 現在の判断 |
|---|---|---|---|
| #1 | JIS配列の半角/全角キーが動作しない | #4 | 差分上のブロッカーなし。テスト追加後に受入可能 |
| #2 | auto選択したCUDA DLLがロード不能でもfallbackしない | なし | 最優先で新規修正 |
| #3 | 候補フォントサイズを変更できない | #5 | 要修正。表示中レイアウトと縦方向の収容が未解決 |
| #6 | `5まん` が `5満` になる | #7 | 要再設計。限定テーブルでは根本原因を解消しない |
| #8 | DLL差し替え時に辞書停止を診断できない | なし | 現行のABI検査・DLLログを含めて再現確認が必要 |
| #9 | Space変換でユーザー辞書・学習が反映されない | #10 | 要修正。成功した読みを失う経路が1件残存 |
| #11 | 変換中のHome/Endがアプリへ素通し | なし | 仕様を固定して実装 |
| #13 | ユーザー辞書と学習履歴の優先順位 | なし | 学習候補をユーザー辞書より上位にできる方針で確定 |

PR #12は学習頻度の減衰と学習履歴v2を扱う。旧学習履歴は移行せず破棄する方針に変更する。Issue #13と同じ学習領域に影響するため、単独レビューと設計整合の確認を終えるまでマージ対象にしない。

## 4. 優先順位

| 優先度 | 項目 | 理由 |
|---|---|---|
| P0 | Issue #2: auto backend fallback | 対象環境でIME全体が使用不能になる |
| P0 | PR #10 / Issue #9 | ユーザー辞書と学習履歴が主要なSpace変換経路から落ちる |
| P1 | ログA: live continuation guard | 正しいライブ変換がかなへ巻き戻る |
| P1 | ログC: keymap mtime gate | キー処理を最大931msブロックする |
| P1 | Issue #11: Home/End | 未確定文字列とアプリ側キャレットの状態が分離する |
| P1 | PR #4 / Issue #1 | JIS配列の標準キーでIME切替ができない |
| P1 | ログB: echo strip | 正しいカタカナ文脈を失い変換品質を下げる |
| P1 | PR #7 / Issue #6 | 数字混在時に辞書を迂回し、頻出表現を誤変換する |
| P1 | G-5: ローマ字入力状態の再構築 | 誤入力をBackspaceで直せない。打ち直せば回避できるためP0にはしない |
| P2 | PR #5 / Issue #3 | 高解像度環境で候補表示が小さく、設定手段がない |
| P2 | Issue #8 | 障害時の原因特定が困難になる |
| P2 | PR #12 / Issue #13 | 方針は確定。永続形式変更と候補順位の回帰確認が必要 |
| P3 | 初回変換リトライ・遅延計測 | 発生頻度が低い、または計測強化のみ |

P0はログ由来のPhaseより先に着手してよい。P1以降は変更するcrateと回帰範囲に応じてリリースを分ける。

修正負荷の目安は次のとおり（コード読解にもとづく見積り）。

- 小: 数十行、1〜2ファイル、設計判断なし
- 中: 数百行、複数ファイル、既存テストの調整あり
- 大: 設計判断を伴い、回帰範囲が広い

## 5. Track R: 既存PRの受入条件

### R-1. PR #10: 読みを明示した候補マージ

#### 現状

`merge_candidates()` はengine-host内部の `hiragana_buf` を参照するため、out-of-process構成では空または古い読みを使うことがある。PR #10はTSF側の呼び出しを `merge_candidates_for_reading()` へ移行している。

初回レビューで指摘された `candidate_window.rs` の2経路は、候補取得に成功したキーを `matched_key` として保持するよう修正済み。

#### 残る問題

`on_convert.rs` のphase 3は、候補を `hiragana_key2` で試し、失敗時に `preedit` で再試行する。しかし最新差分は、どちらで成功してもマージに `preedit` を渡す。再試行後の `hira3` / `preedit` でも同じ情報が失われる。

未確定ローマ字を含む「たt」（reading「た」と接尾辞「t」の区別、接尾辞の表示・確定、末尾 n の扱い）は未確定ローマ字の扱いとして G-5 と同じ領域なので、Step 10 でまとめて扱う。本Stepでは `flush_pending_n` と接尾辞の現状挙動を変更しない。

#### 修正方針

- 候補取得結果を `(matched_reading, candidates)` の組で保持する。
- 初回取得、fallback、reclaim後の再試行、候補マージ、weak merge判定、sync fallbackまで同じ `matched_reading` を渡す。
- `immediate_dict_candidates()` も現在は `preedit` で辞書を引いているため、同じ `matched_reading`（未変換接尾辞を除いた読み）を渡す対象に含める。
- 前提: `hiragana_text()` を reading として扱う。未変換接尾辞（`pending_romaji_buf`）の分離と表示・確定・学習からの除外は Step 10 で行う。したがって本Stepは G-5 に依存しない。
- TSF側から旧 `merge_candidates()` を呼べないよう、RPCクライアントAPIと不要になったprotocol要求を削除する。現在のテストで必要な場合だけ用途を限定して残す。

#### 受入テスト

- 完全な読みでユーザー辞書候補がSpace変換に出る。
- 学習履歴についても同じ結果になる。
- 初回取得とreclaim後の再試行をそれぞれ通す。
- ライブ変換で出た辞書候補がSpace押下後に消えない。
- `rakukan-tsf` に旧 `merge_candidates()` 呼び出しが残らない。

### R-2. PR #4: JIS半角/全角キー

`VK_DBE_SBCSCHAR`（0xF3）と `VK_DBE_DBCSCHAR`（0xF4）を、`OnTestKeyDown` と `OnKeyDown` が共通利用する `normalize_key_event_vk()` で `VK_KANJI`（0x19）へ正規化する方針は妥当。既定keymapの `Zenkaku = 0x19` も変更せずに利用できる。

受入前に、次の純粋な正規化テストを追加する。

- 0xF3 → 0x19
- 0xF4 → 0x19
- 0x19 → 0x19
- 無関係なVKは変更しない

JIS実機では、IMEのON/OFF両方向、`OnTestKeyDown` / `OnKeyDown` の整合、1回の押下で二重に切り替わらないことを確認する。PR #5がPR #4のコミットを含むため、#4を先にマージする。

#### 差分外だが受入時に整合を取る点

- `OnTestKeyDown` は keymap 解決に失敗した場合の fallback に `0x19 → ImeToggle` を持つが、`OnKeyDown` と `Keymap::resolve_action()` の fallback には `0x19` がない。keymap に `Zenkaku` binding がない場合、TestKeyDown=TRUE / KeyDown=FALSE の不一致になる。既定プリセットは `Zenkaku` を持つため通常は発現しないが、両者の fallback を一致させる（どちらかに揃える）ことを受入条件に含める。
- `0xF3` / `0xF4 → 0x19` の正規化は修飾キーの状態に関係なく行われ、その後の `resolve()` は修飾キー付きで照合する。Shift や Ctrl を併用した半角/全角キーは unmapped として扱われる。これは意図した仕様として明記し、正規化テストでは「修飾キーの判断は keymap 側に委ねる」ことを固定する。

### R-3. PR #5: 候補フォントサイズ

#### 解決済み

初回差分にあった横方向の画面外表示は、作業領域幅へのクランプとX座標補正で対応済み。

#### 残る問題1: 表示中レイアウトの不整合

設定再読込はバックグラウンドスレッドからも実行される。PRのAtomic値が即時に変わると、`draw()` と `window_height()` は新しい寸法を読み始める一方、既存HWNDと `TL_WIN_WIDTH` は前回の `show_with_status()` で決めた寸法のままになる。再描画時に文字や行がクリップされる可能性がある。

#### 残る問題2: 縦方向の画面外表示

`candidate_font_height = 72`、候補9件、status行とpager行ありの場合、現在の比例計算では高さが約1,210pxになる。横幅のクランプだけでは、一般的な作業領域へ縦に収まらない。

#### 修正方針

- `show_with_status()` の開始時に設定値を読み、表示中ウィンドウ用のレイアウトをスナップショット化する。
- `draw()`、幅計測、`window_height()`、`reposition()` は同じスナップショットを使う。
- 候補数、status/pagerの有無、現在モニターの作業領域高から、今回表示できる実効フォント高さを算出する。
- 設定値を超えず、ウィンドウ全体が作業領域へ収まる最大値を採用する。縮小した場合はDEBUGログへ設定値と実効値を記録する。
- 新しい設定は次回の候補表示から反映する。表示中にAtomic値だけを差し替えない。
- PR #4マージ後にrebaseし、#4由来の差分とmerge commitをPR #5のレビュー対象から外す。

#### このPRの対象外とする点

- WinUI 設定アプリ（`SettingsStore.cs`）には `[appearance]` の UI がない。設定アプリは既存 TOML を Tomlyn で読み込んで差分更新するため、手書きした `[appearance]` は保存で消えないことを確認済み。設定 UI への追加は別 Issue として扱う。
- `mode_indicator.rs` とランゲージバーの表示はスケールしない。候補ウィンドウのみを対象とする。
- DPI 対応は既存コードも未対応であり、このPRでは扱わない。

#### 受入テスト

- 設定なしで従来の17px表示と同じ寸法になる。
- 10 / 17 / 24 / 72のscale計算。
- 右端、左端、上端、下端、負座標を持つ複数モニター。
- 9候補 + status + pagerが作業領域へ収まる。
- 候補表示中にconfigを再読込しても、次の `show_with_status()` まで寸法が混在しない。
- 次回候補表示では新設定が反映される。

### R-4. PR #7: 数字直後の数詞

#### 現状の問題

PR #7は特定の読みを数詞テーブルで補い、検証後に候補を直接挿入する。これは「5まん」の一例は改善するが、根本原因である `verify_digits_preserved()` と辞書迂回を残す。

確認済みの問題:

- `5まんえん`、`だい5まん`、`3.5まん`、`3ぜん`などへ一般化されない。
- `extract_digits("5万円")` が、ASCII数字の5と独立した漢数字単位10000を連結して扱い、正しい候補を数字改変として落とす。
- `digit_candidates()` の全表記と単位を組み合わせ、「一十」「壱十」「十十」などを生成できる。
- `num_candidates` 上限を超える。
- 生の読みを確定する退避候補が消える場合がある。
- `#[test]` の二重付与がある。

#### 修正方針

- 数詞表の後挿しを主修正にしない。
- `verify_digits_preserved()` が「入力に含まれた数字が変更されていないこと」を検証しつつ、数字直後に変換された「万」「億」などを独立した数字列として誤加算しないよう、数字runと後続かなrunの境界を考慮する。
- かなrunでも辞書候補を参照できる構造を検討し、LLMだけに依存しない。
- 生成候補は通常の重複排除、順位付け、件数制限、生読みfallbackを通す。
- 既存の数値表現変換（`2024ねん` → `二千二十四年`など）を壊さない。

#### 受入テスト

- `5まん`、`5まんえん`、`だい5まん`、`3.5まん`、`3ぜん`。
- `1じゅう`、`10まん`で不自然な漢数字候補を先頭生成しない。
- `3せん`、`1ちょう`、`5じゅう`でLLMの文脈順位を不必要に上書きしない。
- 設定した `num_candidates` を超えない。
- 生の読み候補を残す。
- 既存の数字保持テストをすべて維持する。

### R-5. PR #12: 学習頻度の減衰

PR #12は `suggestion_freq` を `u32` から `f32` へ変更し、確定時にも経過時間分を減衰させる。現在のPRは学習履歴をv1からv2へ移行し、2コミット目でv1高頻度候補の順位を保つ単調な圧縮を行う。

採用方針は、**旧学習履歴を移行せず破棄し、v2を空の状態から開始する**。したがって、v1移行と高頻度カウンタ圧縮はPRから外す。v1を検出した場合は空のv2として扱い、破棄したことをWARNログへ記録する。次回保存時にv2形式で新規保存する。

2026-09-01 に差分を確認した結果、PR は方針と逆に v1 → v2 移行（対数圧縮）を含む。削除対象は次のとおり。

- `LearnEntryV1` / `LearnHistoryFileV1` / `LearnHistoryFileV1Ser`
- `migrate_v1_freq()` / `learn_freq_daily_equilibrium()` / `impl From<LearnEntryV1> for LearnEntry`
- `load_learn_history_file()` の `version == 1` 分岐
- 移行専用テスト `test_learn_history_migrates_v1_to_v2` / `test_migrate_v1_freq_is_monotonic`

現行 main は既に「version 不一致 → WARN + 空の履歴」の経路を持つため、移行分岐を消して `LEARN_HISTORY_FORMAT_VERSION = 2` にすれば「v1 検出時は空 + WARN」は自動的に満たされる。保存は常に現行 version を書くため、次回保存で v2 になる。

PR は bincode から読んだ `f32` を検証せず `score()` に渡す。読み込み時に有限かつ非負でない値（NaN、Infinity、負値）は `0.0` または `1.0` へ丸め、ログに残す処理を追加する。

このPRは未レビューであり、次を確認するまで受け入れない。

- v1を検出した場合に候補を読み込まず、空の学習履歴で継続する。
- v1破棄をログから確認できる。
- v1破棄後の最初の学習と保存で、正常なv2ファイルが作成される。
- `f32` のNaN、Infinity、負値、極端値を読み込んだ場合の扱い。
- 同一時刻、時刻巻き戻り、非常に長い経過時間での計算。
- 保存と再読込を繰り返して順位が不必要に変わらない。
- 学習候補をユーザー辞書より上位にできる、確定済みの優先順位と整合する。
- 明示的なv1 fixtureを使い、旧履歴が候補へ混入しないことを固定する。

## 6. Track L: 8月ログ由来の改善

### L-1. `live_continuation_guard` の誤発動

#### 原因

現行判定は、表示長が読み長の60%未満になるとfallbackする。

```rust
display_base_len * 5 < new_reading_len * 3
```

「だいとうりょうからこく」→「大統領から酷」のような正常な漢字圧縮でも成立するため、正しいpreviewが生のかなへ巻き戻る。

#### 修正方針

- `SessionState::LiveConv` にpreview生成時のreadingを `preview_for` として保持する。
- `preview_for` が現在readingのprefixならpreviewを維持する。
- prefixでなければ生の読みにfallbackする。
- 長さ比は最終判定から撤去する。
- fallbackログへ `preview_for` と現在readingを出す。

#### テスト

- 8月21日の「大統領から酷」の実例でfallbackしない。
- preview由来readingと現在readingが不整合ならfallbackする。
- Backspace、未確定ローマ字、連続入力で古いpreviewを残さない。

### L-2. echo stripのカタカナ語誤爆

8月のecho strip上位は「いんすとーら」「どきゅめんと」「あぷりけーし」「えみゅれーた」などのカタカナ語だった。正しく確定したカタカナ文をecho源としてcontextから削除している。

#### 修正方針

- `sentence_has_echo_run` で、`kata_needle` だけが一致し、対象runが純カタカナの場合は削除対象にしない。
- ひらがなneedleが一致する未変換確定の汚染は従来どおり除去する。
- 候補側の `is_kana_prefix_echo` は維持する。

#### テスト

- 「インストーラを起動する。」をcontextに持つ「いんすとーら」でstripしない。
- ひらがな汚染は従来どおりstripする。
- `repro_context.rs` の全パターンで第1候補を維持する。

### L-3. keymap再読込のmtime gate

モード切替ごとに `Keymap::load()` が同期ファイルI/OとTOML parseを行い、SLOW OnKeyDownの直前に `keymap loaded` が53件、最大931msを記録した。

#### 修正方針

- keymapのpath、最終更新時刻、現在値を管理する。
- mtime不変なら再読込しない。
- `Keymap::load()` は Activate 時（`factory.rs` の初期化）にも呼ばれる。初回ロードは gate を通さず必ず読み込み、gate はモード切替時の再読込だけに適用する。
- 更新、作成、削除を区別し、削除時は既定keymapへ戻す。
- config側の変更検出と共通化する場合も、configとkeymapの失敗を互いに巻き込まない。

#### テスト

- mtime不変で読み直さない。
- 更新時に新しいbindingを反映する。
- 削除時に既定値へ戻る。
- parse失敗時に直前の有効なkeymapを維持する。

### L-4. 低頻度の残件

#### 初回変換のnot ready / busy

月9回で、host起動直後の最初の変換に集中する。P0/P1完了後に、ready待機と1回限りの再試行を検討する。キースレッドを長時間ブロックせず、既存のtimer fallbackへ接続する。

#### Convert遅延の内訳

TSF Convert p95=171ms、engine beam p95=88msの差を分解するため、RPC要求、RPC応答、merge、edit session、candidate window更新を個別計測する。動作変更を伴わない計測だけを先行可能とする。

## 7. Track G: PR未提出Issue

### G-1. Issue #2: auto backend fallback

#### 原因

`detect_best_installed_backend()` は `cuda → vulkan → cpu` の順にDLLファイルの存在だけを確認する。`load_backend()` もファイルが存在すれば `from_dll()` の失敗を上位へ返すため、CUDA runtime不足でDLLロードが失敗しても次候補へ進まない。

#### 修正方針

- fallbackは `gpu_backend = "auto"` の場合だけ行う。この方針は確定済み。
- autoでは `cuda → vulkan → cpu` を順に実ロードし、失敗理由を記録して次へ進む。
- 明示指定した `cuda` / `vulkan` / `cpu` はfallbackせず、ロード失敗をエラーとして返す。現行の `load_backend()` は明示指定でも DLL ファイルが無ければ `cpu` へ落ちる（`vulkan` は経由しない）ため、これは既存挙動の変更になる。CHANGELOG に明記する。
- 最終的に全候補が失敗した場合は、各backendの失敗理由をまとめて返す。
- ログへ試行backend、DLL path、失敗理由、採用backendを残す。
- ファイル存在確認と実ロードを別々の選択ロジックにしない。autoロード処理を一か所に集約する。

#### テスト

実DLLやCUDA環境に依存しないよう、ロード関数を注入可能にして次を検証する。

- CUDA失敗、Vulkan成功。
- CUDA/Vulkan失敗、CPU成功。
- 全件失敗で集約エラー。
- 明示CUDA失敗時にVulkanへfallbackしない。
- CUDA成功時に後続を試さない。

### G-2. Issue #11: Home / End

#### 事前に固定する仕様

- 未確定文字列があるとき、Home/Endはアプリへ渡さない。
- Homeは変換対象の先頭、Endは変換対象の末尾へ移動する。
- 候補選択中、block選択中、通常preedit中の各状態で、どの内部位置を変更するかをテストで固定する。
- 未確定文字列がないときはアプリへ渡す。

#### 実装範囲

- `UserAction::CursorHome` / `CursorEnd`。
- `KeyAction`、変換、既定keymap。
- dispatchとaction名ログ。
- `key_should_eat()`。
- 既存のsegment/block移動APIを調査し、アプリ側キャレットだけを動かしてIME状態を残す経路を作らない。

「preedit中の未定義キーをすべて握り潰す」変更は副作用が大きいため、このIssueには含めない。

### G-3. Issue #8: DLL差し替え時の診断

現行コードには次がすでに存在する。

- host側の期待ABIとDLL側 `engine_abi_version()` の一致検査。
- engine DLL自身の `rakukan-engine-dll.log` 初期化。

ただし、同じABI番号を持つ異なるビルドの組み合わせはABI検査を通る。またIssue報告ではDLL内警告がログへ出なかったため、まず現行mainを同一手順で再現し、どの初期化段階でログが失われるかを確認する。

#### 修正候補

- hostログにDLL path、ABI version、製品versionまたはbuild identifierを必ず記録する。
- ABIが同じでも、互換性を保証しないbuildの組み合わせを検出できる識別子を追加する。
- DLLログ初期化自体の失敗をhost側へ返せる診断APIを追加する。
- 辞書ロード失敗のstepとreasonをRPCの `dict_status` から取得できるようにする。

再現ログを得る前に、tracing基盤全体を推測で変更しない。

### G-4. Issue #13: ユーザー辞書と学習の優先順位

採用方針は、**ユーザーが明示的に選択して学習した候補を、ユーザー辞書候補より上位にできる**ものとする。

候補の基本順序は次のとおり。

1. 学習履歴
2. ユーザー辞書
3. システム辞書
4. LLM

登録直後は学習履歴がないため、従来どおりユーザー辞書候補が先頭になる。別候補を明示的に選択した後は、学習候補がユーザー辞書候補より上位になれる。旧学習履歴は破棄するため、この順序はv2で新たに学習した候補だけに適用する。

固有名詞登録、一般語との読み衝突、誤学習からの回復、学習履歴削除時にユーザー辞書順位へ戻ることをテストする。PR #12の減衰方式もこの優先順位で評価する。

### G-5. ローマ字誤入力をBackspace後に修正できない

#### 原因

ユーザーが入力した文字列は、確定済みの変換単位を `romaji_input_log`、未確定部分を `pending_romaji_buf` に保持している。既存テストも、この2つを連結すると入力文字列を復元できることを確認している。

一方、現在のBackspaceは変換結果と未確定バッファを部分的に削除するだけで、保持済みの入力文字列から変換状態を作り直さない。`kt`では `k` が既に出力側へ移り、`t`だけが未確定バッファにあるため、`t`を削除してから`a`を入力すると「か」ではなく「kあ」になる。

#### 修正方針

- 新しい入力履歴を並行して追加せず、既存の `romaji_input_log` と `pending_romaji_buf` を入力の原本として利用する。
- Backspaceでは最後に入力した1文字を原本から削除し、残った通常ローマ字入力区間を先頭から再生して、`RomajiConverter`、`hiragana_buf`、`pending_romaji_buf`、`romaji_input_log` の整合した状態を作り直す。
- `romaji_input_log` の1要素は1キーとは限らないため、単純に最後の要素を削除する実装にはしない。
- 直接入力、全角英字、数字、記号を通常ローマ字として再解釈しない。既存ログだけでは入力種別の区別が不足する場合は、別履歴を追加するのではなく既存ログの要素へ入力種別またはローマ字区間境界を持たせる。
- F9/F10が利用する入力文字列の復元結果を維持する。
- Space変換時の未変換接尾辞（「たt」の `t`）は、reading「た」で辞書・学習候補を取得し、接尾辞を候補表示と確定で失わず、学習キーにも含めない。接尾辞は英字幅設定に従って付加する。末尾 `n` は現状 `flush_pending_n` で「ん」に確定してから変換しており、これを維持するか接尾辞として残すかは本Stepの着手時に決める。

#### テスト

- `kt` → Backspaceで、表示済みの `k` が未確定ローマ字 `k` に戻り、その後の `a` で「か」になる。
- `nt` → Backspace → `a` で「な」になる。
- `tt` → Backspace → `a` で「た」になり、促音が残らない。
- `kanakq` → Backspaceで従来どおり「かなk」になる。
- `romaji_input_log + pending_romaji_buf` がユーザーの入力文字列と一致する既存の不変条件を維持する。
- 直接入力、全角英字、数字、記号、およびF9/F10の既存テストを維持する。
- 「たt」で「た」に登録したユーザー辞書候補が出て、未変換の `t` が表示・確定結果に残り、学習キーに含まれない。
- 「やまのたn」の扱いは末尾 `n` の方針決定に従う。

## 8. 実施ステップとリリース単位

各ステップは、原則として「変更 → 対象テスト → 共通検証 → 完了判定」の順に進める。前のステップで見つかった未解決の回帰を、次のステップへ持ち越さない。

### Step 0: ベースラインを固定する

#### 作業

- 作業開始時のmainのcommit IDとworktree状態を記録する。
- 既存の未追跡ファイルとユーザー変更を確認し、対象外の変更へ触れない。
- 現在の主要テスト結果を記録する。
- 実機確認に使う設定、GPU backend、ユーザー辞書、学習履歴の状態を記録する。

#### 検証

- PowerShellから `cargo make check` と `cargo test --workspace --lib` を実行する。
- 既存失敗がある場合は、新しい修正と混同しないようログを保存して作業を止める。

#### 完了条件

- 修正前の成功・失敗状態を再現できる。
- 以降の差分が今回の対象だけに限定されている。

### Step 1: Space変換の辞書・学習候補を修正する

対象: R-1 / PR #10 / Issue #9

負荷: 中〜大 — `on_convert.rs` phase 3の2段取得と再試行、`immediate_dict_candidates`、`candidate_window.rs` 2経路、`dispatch.rs` の全経路で `matched_reading` を持ち回る。表示用preeditとreadingの分離、接尾辞の候補表示・確定時付加、旧 `merge_candidates` のRPC / protocol削除（3 crate）。

#### 作業

- 候補取得結果を `(matched_reading, candidates)` の組で保持する。
- 初回取得、fallback、reclaim後の再試行、merge、weak merge、sync fallbackで同じreadingを使う。
- TSF側の旧 `merge_candidates()` 呼び出しを除去し、再利用を防ぐ。

#### 検証

- 通常の読みでユーザー辞書と学習候補を確認する。
- ライブ変換で表示された辞書候補がSpace押下後に消えないことを確認する。
- 初回取得とreclaim後の再試行を別々にテストする。

#### 完了条件

- Issue #9の再現手順が解消する。
- `rakukan-tsf` に旧APIの呼び出しが残らない。

### Step 2: JIS半角/全角キーを修正する

対象: R-2 / PR #4 / Issue #1

負荷: 小 — 差分9行に純粋関数テスト4件と、`OnKeyDown` / `resolve_action` のfallback整合を追加するのみ。実機確認が主。

#### 作業

- 0xF3 / 0xF4を0x19へ正規化する。
- 正規化を純粋関数としてテスト可能にする。
- PR #4をPR #5より先に受け入れる。

#### 検証

- 0xF3、0xF4、0x19、無関係なVKの単体テスト。
- JIS実機でIMEのON/OFF両方向を確認する。
- 1回の押下で二重切替が起きないことを確認する。

#### 完了条件

- 既定の `Zenkaku` bindingがJIS実機で動作する。
- Ctrl+Spaceなど既存bindingを壊さない。

### Step 3: auto backendのfallbackを修正する

対象: G-1 / Issue #2

負荷: 中 — `engine-abi/lib.rs` の選択とロードを順次実ロードへ変更し、ロード関数を注入可能にしてテスト5件。明示指定時の挙動変更を伴う。

#### 作業

- `auto` のロード処理を `cuda → vulkan → cpu` の順次試行へ変更する。
- DLLの存在だけで採用backendを確定しない。
- 各失敗理由を保持し、全件失敗時に集約して返す。
- 明示指定したbackendではfallbackせず、ロード失敗をエラーにする。

#### 検証

- CUDA失敗→Vulkan成功。
- CUDA/Vulkan失敗→CPU成功。
- 全件失敗。
- 明示CUDA失敗時にVulkanを試さない。
- CUDA成功時に後続backendを試さない。

#### 完了条件

- CUDA runtime未導入環境でも、利用可能なVulkanまたはCPUへ1回でfallbackする。
- 明示指定の意味が維持される。

### Step 4: DLL / host診断を再確認する

対象: G-3 / Issue #8

負荷: 小〜中 — 再現作業が先。コード追加はbuild identifierのログ出力と `dict_status` の理由追加程度。再現結果次第で変動する。

#### 作業

- 現行mainでIssue #8のDLL差し替え手順を再現する。
- hostログ、DLLログ、ABI検査、`dict_status` のどこまで情報が残るか記録する。
- 再現ログにもとづき、build identifier、ログ初期化結果、辞書失敗理由のうち不足しているものだけを追加する。

#### 検証

- ABI不一致と、ABIは同じだがbuildが異なる場合を区別する。
- ログ初期化失敗時にもhost側で原因を確認できる。
- 同一ビルドのhost/DLLでは警告を出さない。

#### 完了条件

- 辞書がreadyにならない理由をhost側またはDLL側のログだけで特定できる。
- 再現結果なしの推測実装が残っていない。

### Step 5: live previewの巻き戻りを修正する

対象: L-1

負荷: 小〜中 — `SessionState::LiveConv` へ `preview_for` を追加し判定を置換。LiveConv状態を作るすべての箇所（live timer、Backspace経路）で `preview_for` を更新する必要がある。

#### 作業

- `LiveConv` にpreview生成元の `preview_for` を保持する。
- 長さ比判定をprefix整合判定へ置き換える。
- fallbackログへpreview生成元と現在readingを追加する。

#### 検証

- 「だいとうりょうからこく」の実例でfallbackしない。
- preview生成元と現在readingが不整合ならfallbackする。
- Backspaceと未確定ローマ字で古いpreviewを残さない。

#### 完了条件

- 正常な漢字圧縮でかな表示へ巻き戻らない。
- 真の不整合は引き続き防止される。

### Step 6: keymap再読込のキーストールを除去する

対象: L-3

負荷: 小 — `keymap.rs` にpath / mtime / 現在値を持たせ、`maybe_reload_runtime_config` からgate経由で呼ぶ。config側 `reload_if_changed` と同型。

#### 作業

- keymapのpath、mtime、現在値を管理する。
- mtime不変時はファイル読込とTOML parseを行わない。
- 更新、作成、削除、parse失敗を区別する。

#### 検証

- mtime不変、更新、削除、parse失敗の単体テスト。
- モード切替ごとに `keymap loaded` が出ないことを実機ログで確認する。

#### 完了条件

- keymap未変更時のモード切替から同期ファイルI/Oが消える。
- 最後に有効だったkeymapをparse失敗時に維持する。

### Step 7: Home / EndをIME内で処理する

対象: G-2 / Issue #11

負荷: 中 — `UserAction` / `KeyAction` / 既定keymap / dispatch / `key_should_eat` への追加は機械的。通常preedit・候補選択・block選択の3状態で内部位置を動かす部分に既存API調査と状態別テストが要る。

#### 作業

- `CursorHome` / `CursorEnd` を `UserAction`、`KeyAction`、既定keymap、dispatch、ログへ追加する。
- 未確定文字列がある場合だけHome/Endを消費する。
- 通常preedit、候補選択、block選択の各状態で、先頭・末尾の内部位置へ移動する。
- preeditがない場合はアプリへ渡す。

#### 検証

- 各session状態でHome/Endを確認する。
- 未確定文字列がない状態ではアプリ側のHome/Endが動作する。
- Left/Right、segment伸縮、候補移動を壊さない。

#### 完了条件

- 未確定文字列を残したままアプリ側キャレットだけが移動する状態を作らない。

### Step 8: echo stripのカタカナ語誤爆を修正する

対象: L-2

負荷: 小 — `sentence_has_echo_run` の一致条件に「`kata_needle` のみ一致かつ純カタカナrunは除外」を足すのみ。`repro_context.rs` の回帰確認が主。

#### 作業

- `kata_needle` だけが一致する純カタカナrunをstrip対象から外す。
- ひらがな汚染と候補側のprefix echo防壁は維持する。

#### 検証

- 正常なカタカナ文脈を保持するテスト。
- ひらがな汚染を除去する既存テスト。
- `repro_context.rs` の全パターン。

#### 完了条件

- カタカナneedleによるstripが発生しない。
- 既存のecho防止効果を壊さない。

### Step 9: 数字混在変換を根本修正する

対象: R-4 / PR #7 / Issue #6

負荷: 大 — `verify_digits_preserved` / `extract_digits` の境界判定の再設計に加え、かなrunでの辞書参照は `convert_with_digit_protection` の流れ自体の変更。既存の数字保持テスト群を維持しながらの改修で、最も設計が重い。

#### 作業

- PR #7の限定的な数詞候補後挿しを採用しない。
- 数字runと後続かなrunの境界を考慮して `verify_digits_preserved()` を修正する。
- かなrunでも辞書候補を参照できる構造を検討する。
- 通常の重複排除、順位付け、件数制限、生読みfallbackを通す。

#### 検証

- `5まん`、`5まんえん`、`だい5まん`、`3.5まん`、`3ぜん`。
- `1じゅう`、`10まん`で不自然な候補を作らない。
- `num_candidates`、生読み候補、既存の数字表現変換を維持する。

#### 完了条件

- 個別読みの例外表追加ではなく、数字保持と辞書参照の共通経路で解決する。

### Step 10: ローマ字入力状態を入力文字列から再構築する

対象: G-5（P1。Step 8–9 と同じ engine DLL の変更としてまとめる）

負荷: 中〜大 — Backspaceの再生自体は小さいが、`romaji_input_log` の要素に入力種別（通常ローマ字 / 数字 / 記号 / 区切り / Shift英字）を持たせる表現変更が本体。`push_char` の5経路、`flush_pending_n`、`force_preedit`、F6〜F10の復元関数（`hiragana_from_romaji_log` / `romaji_log_str`）が対象。engine内に閉じる。

#### 作業

- `romaji_input_log` と `pending_romaji_buf` から、通常ローマ字区間のユーザー入力を復元する。
- Backspaceで最後の入力文字を削除し、残った区間を再生して変換済み文字と未確定ローマ字を再構築する。
- 既存ログで入力種別を区別できない箇所は、ログ要素へ種別または区間境界を追加する。
- 再構築処理をBackspace専用の場当たり的な分岐にせず、入力文字列からローマ字状態を再生するテスト可能な処理として分離する。
- Space変換で表示用 `preedit` と辞書検索用 `reading` を分離し、未変換接尾辞を候補表示と確定で失わず、学習キーへ含めない。

#### 検証

- `kt` → Backspace → `a` が「か」になる。
- `nt` → Backspace → `a` が「な」になる。
- `tt` → Backspace → `a` が「た」になる。
- `kanakq`、直接入力、全角英字、数字、記号、F9/F10の既存動作を維持する。
- 入力ログと未確定部分を連結した値が、ユーザーの入力文字列と一致する。
- Step 5（L-1）のBackspaceテストの期待値を再確認する（`kt` → Backspace で reading が「k」から空に変わる）。
- `hiragana_text()` と `pending_romaji_buf` の意味を変えていないことを、Step 1 の「たt」「やまのたn」テストで確認する。

#### 完了条件

- Backspace後の表示結果、変換器、入力ログ、未確定ローマ字が同じ入力列を表す。
- 誤入力を削除した後の次の文字が、削除前の派生出力に妨げられず、残ったローマ字と結合される。

### Step 11: 候補フォントサイズを安全に変更可能にする

対象: R-3 / PR #5 / Issue #3

負荷: 中 — PR #4後のrebase、`show_with_status` 開始時のレイアウトスナップショット化、`draw` / `window_height` / `reposition` / 幅計測の参照先変更、作業領域高さからの実効フォント高さ算出。目視確認が必須。

#### 作業

- PR #4マージ後のmainへrebaseする。
- 表示開始時に候補ウィンドウのレイアウトをスナップショット化する。
- モニター作業領域と表示行数から実効フォント高さを算出する。
- 幅、高さ、X/Y位置、描画が同じスナップショットを使うようにする。

#### 検証

- 10 / 17 / 24 / 72px。
- 候補9件 + status + pager。
- 複数モニター、負座標、各画面端。
- 表示中のconfig再読込と次回表示への反映。
- 実際の候補ウィンドウを目視する。

#### 完了条件

- どの許容設定でも作業領域からはみ出さず、文字や行をクリップしない。
- 設定なしの表示が従来と一致する。

### Step 12: 学習順位と学習履歴v2を実装する

対象: R-5 / G-4 / PR #12 / Issue #13

負荷: 中 — PRから移行コード一式を削除、`f32` の有限値検証、`merge_candidates_for_reading` の順序入れ替え。永続形式変更のためfixtureを使った破棄テストが要る。

#### 作業

- 候補順を `学習履歴 → ユーザー辞書 → システム辞書 → LLM` にする。
- PR #12の頻度減衰を独立レビューする。
- v1移行と高頻度カウンタ圧縮を削除する。
- v1検出時は旧履歴を候補へ使わず、空のv2から開始してWARNを残す。
- 次回学習時にv2形式で保存する。

#### 検証

- 登録直後はユーザー辞書が先頭になる。
- 別候補を明示選択すると学習候補が上位になる。
- 学習履歴削除後はユーザー辞書順位へ戻る。
- v1 fixtureを破棄し、最初の学習で正常なv2を作成する。
- NaN、Infinity、負値、時刻巻き戻りを安全に扱う。

#### 完了条件

- 確定した優先順位が一貫して適用される。
- 旧履歴が新しい順位へ影響しない。
- 永続データ変更を他の修正へ同乗させない。

### Step 13: 低頻度改善と計測を追加する

対象: L-4

負荷: 小〜中 — not ready時の1回リトライを既存timer fallbackへ接続、Convert区間の計測ログ追加。動作変更は最小。

#### 作業

- 初回 `not ready` 時のready待機と1回限りの再試行を、既存timer fallbackへ接続する。
- Convert処理をRPC要求、RPC応答、merge、edit session、candidate window更新へ分解して計測する。

#### 検証

- キースレッドを長時間ブロックしない。
- retry stormを起こさない。
- 計測ログだけで各区間の遅延を集計できる。

#### 完了条件

- 初回変換失敗が減少するか、残る原因をログで特定できる。

### Step 14: リリースと再計測

#### リリース候補の分割

1. Step 1–2: 辞書候補とJISキー（rakukan-tsf）。Step 2 は小さいため Step 1 の途中でも先行して出せる。
2. Step 3–4: backend起動安全性と診断（engine-abi / host）。
3. Step 5–7: TSF入力品質（rakukan-tsf）。Step 6 は独立して小さい。
4. Step 8–10: engine変換品質とローマ字入力（rakukan-engine）。Step 8 は独立して小さい。
5. Step 11: 候補UI。
6. Step 12: 学習と永続形式。
7. Step 13: 低頻度改善と計測。

各リリースのバージョン番号は実装時に決める。バージョンを変更する場合は、先に `docs/version-update-checklist.md` を作成する。

#### 完了条件

- 対応するステップのテストと実機確認が完了している。
- 導入後1週間以上のログを、次節の指標で再計測できる。

## 9. 共通検証

テストはPowerShellから実行する。Git Bash経由の `cargo test` はConPTYテストがハングするため使用しない。

変更内容に応じて次を実行する。

```powershell
cargo make check
cargo test -p rakukan-dict --lib
cargo test --workspace --lib
```

engine変更では必要に応じて次も実行する。

```powershell
cargo make test
```

追加条件:

- テストやスクリプト自身の確認を除き、パッケージ作成は行わない。
- TSFキー処理はJIS実機と主要アプリで確認する。
- 候補ウィンドウは実際の表示を確認し、サイズ、位置、クリップを目視する。
- 永続データ変更は、採用する現行形式の読み書き、明示した旧形式の破棄、失敗時の保全を確認する。
- 変更したcrateだけでなく、関連するworkspaceのlibテストを通す。
- `git diff --check` で空白エラーを確認する。

## 10. 導入後の再計測

各Batch導入後、最低1週間のログを収集する。

| 指標 | 8月実測 | 目標 |
|---|---:|---:|
| `live_continuation_guard` fallback | 160回/月 | 数回/月以下 |
| カタカナneedleによるecho strip | 上位10件がすべてカタカナ語 | 0件 |
| echo strip全体 | 約1,700回/月 | 数回/日以下 |
| SLOW OnKeyDownの直前が `keymap loaded` | 53回/月 | 0件 |
| Space変換で辞書候補が落ちる | Issue #9で再現 | 0件 |
| auto backendの同一backend再試行ループ | Issue #2で再現 | 0件、利用可能backendへ1回でfallback |
| `engine::init: loading model` | 約1.5回/日 | 悪化しない |
| `take_ready MISMATCH` | 0回 | 0回を維持 |
| 初回 `engine not ready` / `busy` | 9回/月 | Step 13実施時のみ削減目標を設定 |

## 11. 完了条件

各項目は次を満たした時点で完了とする。

1. 原因と修正対象がコード上で特定されている。
2. 正常系と報告された再現ケースのテストがある。
3. 関連する既存テストが通る。
4. 実機依存項目は実際の表示またはハードウェア構成で確認されている。
5. ログで導入後の効果を区別できる。
6. 後方互換性は原則として完了条件にしない。永続データや設定形式を変える場合は、採用形式、旧形式の扱い、失敗時保全が確認されている。
7. バージョンを変更する場合は `docs/version-update-checklist.md` を作成し、コード、リリースメタデータ、README、CHANGELOG、Windows固有のversion面を確認している。

## 12. 今回の対象外

- Firefoxの `TS_E_READONLY` に対する新しい回避策。現行保全パスが動作しているため監視のみ。
- preedit中の未定義キーを一律に握り潰す変更。
- PR #7の限定的な数詞テーブルを、そのまま拡張して一般解とすること。
- Issue #13で確定した「学習履歴 → ユーザー辞書」の順序を、別の優先順位へ変更すること。
- ログや再現結果を得ずにIssue #8のtracing基盤を推測で置き換えること。

## 13. 実施記録

### Step 0（2026-09-01）

- ベースライン: main `e5d086a`、作業ツリーはクリーン（未追跡なし）。
- `cargo make check` OK。`cargo test --workspace --lib`: dict 33 / engine 165（ignored 1）/ abi 0 / rpc 7 / tsf 61、失敗 0。
- 既知の不安定テスト: `rakukan-engine` の `backend::tests::test_env_override_{cuda,cpu,unknown_falls_back_to_cpu}` は同じ環境変数 `RAKUKAN_BACKEND` をプロセス全体で並列に set / remove するため、稀に競合して失敗する（Step 1 の作業中に 1 回発生、再実行で通過）。修正するなら 3 テストを mutex で直列化する。

### Step 1（2026-09-01）

- PR #10 の 3 コミット（`bf370eb` `9f1d02a` `50dc1cc`）を `cherry-pick -n` で取り込み、その上で次を修正。
  - `on_convert.rs` phase 3: `bg_take_candidates` が成功したキー（`hiragana_key2` / `preedit` / 再試行後の `hira3` / `preedit`）を `matched_reading` として保持し、`merge_candidates_for_reading`、weak merge 判定（`is_weak_merge`）、`sync_after_weak_merge` / `sync_no_bg` の同期 fallback まで同じ reading を渡す。
  - `immediate_dict_candidates`: `hiragana_text()` を reading にし、「変換候補あり」判定は preedit と reading の両方と異なる候補に限定。
  - `engine_convert_sync_multi`: `reading`（辞書キー）と `preedit`（空のときの表示文字列）を分離。
  - RPC: `Request::MergeCandidates` を `_ReservedMergeCandidates`（deprecated、スロット維持）にし、host は `Error` を返す。`RpcEngine::merge_candidates()` と `engine-abi` の `DynEngine::merge_candidates()` を削除（vtable のシンボルは ABI 維持のため読み込みのみ）。
- 追加テスト: `on_convert::tests`（`is_weak_merge` 4 件）、`codec::tests::removed_merge_candidates_keeps_its_slot`。
- 検証: `cargo check --workspace --all-targets` 警告 0、`git diff --check` OK、`cargo test --workspace --lib`: dict 33 / engine 165 / rpc 8 / tsf 65、失敗 0。
- 未実施（実機）: 完全な読みでユーザー辞書・学習候補が Space 変換に出る、初回取得と reclaim 後の再試行、ライブ変換候補が Space 後に消えない。
- 接尾辞（「たt」）と末尾 `n` の扱いは Step 10 へ移した（4 節・R-1・Step 10 に反映済み）。

### Step 2（2026-09-01）

- PR #4 のコミット（`5c2a238`）を `cherry-pick -n` で取り込み、VK の読み替え規則を `keymap.rs` の純粋関数 `normalize_key_event` に集約（`factory.rs` の `normalize_key_event_vk` は修飾キー状態を読んで委譲するだけにした）。0xF3 / 0xF4 → 0x19 の正規化は修飾キーを変更せず、Shift / Ctrl 併用時の扱いは keymap 側の照合に委ねる。
- R-2「差分外だが受入時に整合を取る点」: keymap 解決失敗時の fallback を `essential_fallback_action()` に一本化し、`resolve_action` ①.5（keymap に binding が無い場合）と `OnTestKeyDown` の keymap 取得失敗時で同じ集合（Enter / Space / BS / Esc / IME_OFF / IME_ON / 半角全角）を使うようにした。これにより `OnKeyDown` 側も `resolve_action` 経由で半角/全角の fallback を得る。
- `docs/DESIGN.md` の VK 対照表に 0xF3 / 0xF4 の正規化を追記。
- 追加テスト（`keymap::tests`）: 0xF3 / 0xF4 / 0x19 → 0x19、無関係な VK は不変、修飾キーの素通し、既定プリセットで正規化後の 0x19 が `ImeToggle` に解決される、fallback 集合。
- 検証: `cargo check -p rakukan-tsf --all-targets` 警告 0、`git diff --check` OK、`cargo test --workspace --lib` は Step 1 と合わせて再実行（結果は本節末尾）。
- 未実施（JIS 実機）: IME の ON/OFF 両方向、1 回の押下で二重に切り替わらないこと、Ctrl+Space など既存 binding を壊さないこと。
- テスト結果（Step 1 + 2）: dict 33 / engine 165 / abi 0 / rpc 8 / tsf 69、失敗 0。

### Step 12 に向けた観察（2026-09-01）

- Step 1・2 導入後の実機で、同じ読みにユーザー辞書の登録があると学習した候補より常にユーザー辞書が先頭に来ることを確認（Issue #13 の再現。現行の順序 ユーザー辞書 → 学習履歴 の仕様どおり）。順序入れ替えの前倒しは行わず、計画どおり Step 12（学習履歴 v2 と同時）で対応する。

### Step 3（2026-09-01）

- `engine-abi/lib.rs`: ファイル存在だけで backend を決めていた `detect_best_installed_backend` を廃止し、`load_with_selection`（ロード処理を注入できる純粋な選択ロジック）に置き換えた。`auto` は `cuda` → `vulkan` → `cpu` の順に実ロードを試み、失敗理由を WARN（`backend::auto: <backend> failed: ...; trying next`）で残して次へ進む。全て失敗した場合は `all backends failed (auto): cuda: ...; vulkan: ...; cpu: ...` を返す。採用時は `Selected backend (auto|explicit): <backend> path=<dll>` を INFO で出す。
- 明示指定（`cuda` / `vulkan` / `cpu`）は fallback せず、DLL が無い・ロードできない場合はエラー（`backend <b> (explicit, no fallback) failed`）。従来の「ファイルが無ければ cpu」の暗黙 fallback は廃止（CHANGELOG の Changed に明記）。
- `from_dll` のロード失敗メッセージに、`ERROR_MOD_NOT_FOUND`（126）の場合だけ「依存 DLL が見つからない（CUDA ランタイム）」のヒントを付加。
- `CHANGELOG.md` に `[Unreleased]` を追加（Issue #2 / #9 / #1 の Fixed、明示指定の Changed）。`README.md` に CUDA ランタイムが別途必要な旨を追記（Issue #2 提案 3）。
- 追加テスト（`backend_selection_tests`、8 件）: CUDA 失敗→Vulkan 成功、CUDA/Vulkan 失敗→CPU 成功、全件失敗の集約エラー、CUDA 成功時に後続を試さない、明示 CUDA 失敗時に Vulkan を試さない、明示 Vulkan は Vulkan だけ、明示指定で DLL 欠落は cpu へ落ちない、DLL パス組み立て。
- 検証: `cargo check --workspace --all-targets` 警告 0、`git diff --check` OK、`cargo test -p rakukan-engine-abi --lib` 8 件通過。
- 未実施（実機）: CUDA ランタイム未導入環境で `auto` が Vulkan へ 1 回で切り替わること（host ログの `backend::auto` 行で確認）。

### Step 4（2026-09-01）

- 事前調査: Issue #8 の組み合わせ（0.10.4 host + 2026-08-21 main の DLL）は、git 履歴上 8/9〜8/21 に engine / dict / abi / rpc / host を変更したコミットが無く、ABI も両方 9。辞書コードは同一で、違いは報告者のビルド方法（`cargo build -p rakukan-engine --release --features vulkan` を直接実行）のみ。DLL ログ初期化は 0.9.9 から存在し、通常環境の `rakukan-engine-dll.log` に `rakukan_dict` / `rakukan_engine` の INFO が出ていることを確認済み。Issue 本文は DLL ログファイルに言及しておらず README にも記載が無かった。実機再現は行わず（方針決定 2026-09-01）、再現結果に依存しない診断の欠落だけを実装した。
- host / DLL の build 識別子: `build-support/git_info.rs`（両 build.rs が `include!`）で `RAKUKAN_GIT_SHA`（短縮 12 桁 + `-dirty`、git 不明なら `unknown`）を埋め込む。engine DLL に任意シンボル `engine_build_info`（JSON: version / git sha / build time / ABI / DLL ログ初期化結果）を追加（ABI 番号は据え置き。無い DLL は `load_sym_opt` で `None`）。
- host（`engine-abi::from_dll`）: DLL 生成後に `engine DLL loaded: path=... abi=... dll_version=... dll_git=... host_version=... host_git=... dll_log=...` を INFO で記録。`build_mismatch()` が version 不一致、または両方の sha が判明していて不一致なら WARN（`unknown` は警告しない）。DLL ログ初期化が `ok` でなければ WARN。`engine_build_info` の無い古い DLL も WARN。host 起動時に自分の version / git sha を INFO で出す。
- DLL（`ffi.rs`）: `init_dll_logging` の結果（`ok path=` / `open failed (...)` / `subscriber already set (...)`）を `LOG_STATUS` に保持し `engine_build_info` で返す。
- TSF（`state.rs`）: 辞書 ready 待ちが 30 秒を超えたら `dict_status`（`failed at [step]: reason` 等）を `rakukan.log` に WARN で 1 回出す（`reset_ready_latches` で再武装）。
- README にログ 3 種（TSF / host / DLL）を記載。CHANGELOG `[Unreleased]` に Added。
- 追加テスト: `build_id_tests`（同一ビルド / version 違い / 同 version 別 sha / dirty / unknown / 旧 JSON 互換の 6 件）、`build_info_tests`（1 件）、`dict_wait_tests`（1 件）。
- 未実施（実機）: 別ビルドの DLL を差し替えたときに host ログへ WARN が出ること、`dict_status` の WARN が 30 秒後に出ること。
- 検証: `cargo check --workspace --all-targets` 警告 0、`git diff --check` OK、両 build.rs が同じ `RAKUKAN_GIT_SHA`（`3878393b9cd2-dirty`）を出力することを確認。`cargo test --workspace --lib`: dict 33 / engine 166 / abi 14 / rpc 8 / tsf 70、失敗 0（engine は既知の `test_env_override_*` 競合で 1 回失敗、再実行で通過）。
- 既知の不安定テスト `backend::tests::test_env_override_*` を修正: 3 テストが共有する環境変数 `RAKUKAN_BACKEND` の set / remove を static Mutex（`EnvGuard`）で直列化し、Drop で必ず remove する。Step 1〜4 のテスト実行 4 回中 2 回で発生していた競合を解消（ユーザー指示 2026-09-01）。

### Step 5（2026-09-01）

- `SessionState::LiveConv` に `preview_for`（preview を生成した reading）を追加。`set_live_conv(reading, preview, preview_for)` に変更し、全 8 呼び出しを更新: live timer 適用と Phase1B 適用は `preview_for = reading`、記号追加（LiveConv / BlockSelecting / Selecting からの遷移）と RangeSelect からの復帰は display が読み全体に対応するので `preview_for = 新 reading`、追加入力での継続は最初の BG 変換キーを引き継ぐ（fallback した場合は `new_reading`）。
- `live_continuation_display(preview_for, preview, reading, new_reading, pending)`: 長さ比（`display_base_len * 5 < new_reading_len * 3`、12 文字以上）を撤去し、「直前の reading と `preview_for` がともに `new_reading` の prefix」なら継続表示、そうでなければ生の読みへ fallback。接尾辞の計算（`strip_prefix`）も関数内に移し、prefix でない場合を `reading_not_prefix` として fallback 対象にした（従来は `unwrap_or(new_reading)` で preview の後ろに読み全体を連結していた）。
- fallback の WARN に `reason`（`preview_for_not_prefix` / `reading_not_prefix`）、`preview_for`、`reading`、`new_reading`、`preview` を出す。`LIVE_CONTINUATION_GUARD_MIN_READING_LEN` は削除。
- テスト（`on_input::tests`、8 件）: 8/21 の実例「だいとうりょうからこく」+ 'い' で fallback しない、2 文字目以降の継続、旧テストの英数 12 文字入力を維持、`preview_for` が prefix でない場合の fallback、reading が prefix でない場合の fallback、未確定ローマ字は表示にだけ付く、既存の短い preview 2 件。
- 検証: `cargo check -p rakukan-tsf --all-targets` 警告 0、`git diff --check` OK、`cargo test -p rakukan-tsf --lib` 75 件通過。
- 未実施（実機）: 1 週間運用して `live_continuation_guard event=fallback` が数回/月以下になること、Backspace / 未確定ローマ字 / 連続入力で古い preview が残らないこと。
- テスト結果（全体）: dict 33 / engine 166 / abi 14 / rpc 8 / tsf 75、失敗 0。

### Step 6（2026-09-01）

- `keymap.rs` に `KeymapReloader`（path / 最終更新時刻）と `classify_change`（Unchanged / Created / Updated / Deleted の純粋判定）を追加。グローバル `KEYMAP_RELOADER` を `Keymap::load()`（Activate）で基準化し、`Keymap::reload_if_changed()` はモード切替時に mtime が変わった場合だけ `keymap.toml` を読み直す（変化なしなら `metadata` 1 回のみ）。
- 更新・作成 → 新 keymap、削除 → 既定 keymap、parse 失敗 → WARN を出して `None`（`factory.rs` 側は直前の keymap を維持）。失敗時も mtime を更新し、壊れたファイルを切替ごとに parse し直さない。ロード処理は `reload_if_changed_with(load)` で注入可能にしてテストした。
- `factory.rs` の `maybe_reload_runtime_config`: `Keymap::load()` の無条件呼び出しを `reload_if_changed()` に置換。config 側（`maybe_reload_on_mode_switch`）とは独立に判定し、互いの失敗を巻き込まない。Activate の初回ロードは gate を通さず従来どおり。
- テスト（`keymap::tests`、6 件）: 変更種別の全遷移、mtime 不変で parse しない、更新で新 binding（F6: hiragana → katakana）を反映し 2 回目は変化なし、parse 失敗で直前を維持し再試行しない、削除で既定 keymap（Space=Convert）、path 不明なら何もしない。一時ファイルは `File::set_modified` で mtime を確実に進めている。
- 検証: `cargo check -p rakukan-tsf --all-targets` 警告 0、`git diff --check` OK、keymap テスト 13 件通過。
- 未実施（実機）: モード切替ごとに `keymap loaded` が出ないこと（Activate 時のみ）、`keymap.toml` 編集後の切替で `keymap reloaded (Updated)` が出て新 binding が効くこと、SLOW OnKeyDown の直前パターンが消えること。
- テスト結果（全体）: dict 33 / engine 166 / abi 14 / rpc 8 / tsf 81、失敗 0。

### Step 7（2026-09-01）

- 仕様の固定: rakukan は preedit 内にキャレットを持たない（既存の Left / Right も消費するだけで位置を変えない）ため、Home / End は次のとおりとした。
  - 未確定文字列なし → アプリへ渡す（`key_should_eat` は `has_preedit` で gate）。
  - Preedit / LiveConv / Waiting / Selecting / BlockSelecting → 消費して何もしない（アプリ側キャレットは動かない）。BlockSelecting は確定済みブロックへ戻れないため、先頭ブロックへの移動も定義しない。
  - RangeSelect（Shift+矢印の範囲指定）→ 選択範囲の右端を Home で先頭（1 文字）、End で末尾（全体）へ移す。
- 実装: `UserAction::CursorHome` / `CursorEnd`、`KeyAction::CursorHome` / `CursorEnd`（`cursor_home` / `cursor_end`）、`to_user_action`、既定プリセット（JIS / US）と keymap.toml テンプレートに `Home` / `End` を追加。`key_should_eat` と `action_name` に追加。dispatch から `on_cursor_jump(to_end)` を呼ぶ。`SessionState::range_select_to_start` / `range_select_to_end` を追加。
- 「preedit 中の未定義キーをすべて握り潰す」変更は計画どおり含めない。
- テスト（6 件）: 既定プリセットで 0x24 / 0x23 が `CursorHome` / `CursorEnd` に解決、keymap.toml の `cursor_home` / `cursor_end` を parse、`key_should_eat` が Home / End を preedit ありのときだけ消費し Left / Right と同じ gate、RangeSelect の Home / End で境界が先頭 / 末尾へ移り既に端なら変化なし、他状態では無変化。
- 検証: `cargo check -p rakukan-tsf --all-targets` 警告 0、`git diff --check` OK、`cargo test -p rakukan-tsf --lib` 87 件通過。
- 未実施（実機）: 変換中（候補表示中 / 通常 preedit / ライブ変換中）に Home / End を押してもアプリ側キャレットが動かないこと、未確定文字列が無いときはアプリの Home / End が動くこと、Shift+Right で範囲指定中に Home / End で範囲が先頭 / 末尾へ変わること、Left / Right・文節伸縮・候補移動が壊れていないこと。
- テスト結果（全体）: dict 33 / engine 166 / abi 14 / rpc 8 / tsf 87、失敗 0。

### Step 8（2026-09-01）

- 8月の DLL ログで `echo sentence dropped` の実例を確認したところ、捨てられた文は「分類方法がをトランプはトランプは、極めて…」「プレビューファイル名の下、および…」のように**変換済みのカタカナ語に助詞・漢字が続く正常な文**で、かな run はカタカナだけで構成されていない（`トランプはトランプは` = 10 文字の混在 run）。計画の「`kata_needle` のみ一致かつ純カタカナ run は除外」では混在 run が引き続き削られ、再計測目標「カタカナ needle による strip 0 件」を満たせないため、**カタカナ形 needle での照合自体を廃止**し、ひらがな needle のみで判定するようにした（計画 L-2 の意図「カタカナ語の echo は正しい出力」に沿う）。
- `sentence_has_echo_run(sentence, needle)` から `kata_needle` を削除。`strip_echo_context` の `hiragana_to_katakana(&needle)` も削除。候補側の `is_kana_prefix_echo` は維持。
- 既存テスト `strip_echo_context_drops_katakana_echo_sentence`（F7 カタカナ確定文を除去）は方針変更に伴い `..._keeps_katakana_echo_sentence` に反転（理由をコメントに記録）。追加テスト: 「インストーラを起動する。」を保持、長いカタカナ複合語（ギャラリーエクスポート）と混在 run（トランプはトランプは）を保持、カタカナ語の直後に続く未変換ひらがな汚染は従来どおり除去。
- 検証: `cargo check --workspace --all-targets` 警告 0、`git diff --check` OK、`strip_echo` テスト 10 件通過。
- `repro_context.rs`（`cargo run -p rakukan-engine --example repro_context --release`、jinen-v1-small-q5 が必要）は本 Step の対象パターン（ひらがな汚染）に変更が無いため単体テストで代替した。実行結果は本節末尾に追記する（実行できた場合）。
- 未実施（実機）: `echo sentence dropped` が数回/日以下になり、needle にカタカナ語が並ばなくなること（1 週間運用）。
- テスト結果（全体）: dict 33 / engine 169 / abi 14 / rpc 8 / tsf 87、失敗 0。`repro_context.rs` は本環境にモデルディレクトリ（`%LOCALAPPDATA%\rakukan\models`）が無く未実行。

### リリース 0.11.0（2026-09-01）

- Step 1〜8 をまとめて 0.11.0 とした（Step 14 の分割案は採用せず 1 リリース）。`docs/version-update-checklist.md` を作成し、`VERSION` / `Cargo.toml` / `Cargo.lock` / `rakukan_installer.iss` / WinUI `csproj` / `CHANGELOG.md`（`[Unreleased]` → `[0.11.0]`）/ `README.md` を更新。
- 設計文書の追随: `docs/DESIGN.md`（LiveConv の `preview_for`、候補マージの読み明示と旧 API 廃止、backend 選択の実ロード順次試行、RPC 表、config / keymap のリロード条件、Home / End、ログ 3 種）、`config/config.toml` の `gpu_backend` コメント、`CLAUDE.md`（backend 選択・ログ・バージョン更新手順）、README のキー操作表と設定の目安。
- 実機確認は Step 1〜8 とも未実施。0.11.0 の配布前に少なくとも Step 1（辞書候補）・Step 2（JIS キー）・Step 3（backend fallback）を確認する。
- 次の開発開始時に CHANGELOG へ `[Unreleased]` を再作成する。

### リリース 0.11.1（2026-09-02）

- Issue #14（rustfmt 1.9.0 / 新しい clippy の導入で main の CI が両ジョブとも落ち、全 PR のチェックが赤くなる）への対応。fmt は PR #15（nick20002005 氏）をマージ（`4b0b7ad`）、clippy は `ff05d72` で解消（35 ファイル、`cargo clippy --fix` の機械適用＋理由コメント付き `#[allow]`、挙動変更なし）。`cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --check` / `cargo test --workspace`（dict 33 / engine 169 / abi 14 / rpc 8 / tsf 87、失敗 0）を確認し、CI run 33573317635 で両ジョブ success。
- fmt が落ちていた 2 ファイル（llamacpp.rs / on_compose.rs）は、Step 3〜8 の作業中にコミットを最小に保つため rustfmt の整形を意図的に revert し続けていたことが原因。今後は fmt の差分も含めてコミットする。
- バージョン 0.11.1: `docs/version-update-checklist.md` に従い `VERSION` / `Cargo.toml` / `Cargo.lock` / `rakukan_installer.iss` / WinUI `csproj` / `CHANGELOG.md` / `README.md` を更新。

### Step 9（2026-09-03）

- PR #7（nick20002005 氏の再設計版）を採用した。数詞表の後挿しをやめ、根本原因である `extract_digits()` の境界（数字 run の直後に単位だけの漢字 run が続く場合、その単位を独立した数値として数えない）を修正。数詞候補はかな run の候補リストへ 1 つ足して `combine_runs` の通常経路（重複排除・順位付け・件数制限・生読み fallback）を通す。発動条件は「直前の run が数字 run かつ かな run が数詞に完全一致」で、`だい5まん` / `3.5まん` にも効き、連濁形（ぜん・びゃく・ぴゃく）も対応。「一十」「壱十」は生成されない。数字に隣接しない漢数字 run は従来どおり数値として解釈し、`2024ねん` → `二千二十四年` の正規化は不変。
- レビューで verify の弱化を発見: 単位を無条件に読み飛ばすと「5えん」→「5万円」のような単位の挿入（数を 1 万倍に変える改変）を見逃す。`maintainerCanModify` を使いフォークブランチへ直接 push した `c4d9ece`（main では `dc2a96b`）で修正。`reading_licenses_units()` が単位の読み飛ばしを「入力読みに対応する数詞かなが含まれる場合」に限定する。照合は単位の値で行い大字（拾 = 十）も正当化、`NUMERIC_UNITS` に読みが無い「京」は正当化されない。
- CI（run 33700036048）両ジョブ success を確認し rebase マージ。main は `dc2a96b`（5 コミット直線）。
- 完了条件との関係: 「個別読みの例外表追加ではなく共通経路で解決」は、verify 側を `extract_digits()` の共通修正で満たした。`NUMERIC_UNITS`（9 エントリ）は位取り単位という閉集合の候補補完で、読みごとの救済表とは性質が異なると判断。作業項目「かな run でも辞書候補を参照できる構造を検討する」は検討の結果、辞書を run 単位の変換へ配線する設計が別途必要として Issue #16 へ切り出した（PR #7 のスコープ外、コントリビューターの提案どおり）。
- テスト結果（全体）: dict 33 / engine 178 / abi 14 / rpc 8 / tsf 87、失敗 0。`cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --all -- --check` もクリーン。実モデル（jinen-v1-small-q5）の実測は PR #7 コメントの表を参照（`#[ignore]` 付き診断プローブ `probe_numeric_unit_candidates` を同梱）。
- 実機確認済み（2026-09-03）: `5まん` / `5まんえん` / `だい5まん` / `3.5まん` / `3ぜん` / `1じゅう` / `10まん` / `3せん` の候補確認と、`2024ねん` 等の既存数値表現の維持確認。確認後に Issue #6 をクローズした（実装コミット引用コメント付き、構造課題は Issue #16 で追跡）。

### Step 11（2026-09-03）

- PR #5（nick20002005 氏）を採用した。`[appearance] candidate_font_height`（10〜72px、既定 17）でフォントサイズを変更でき、行高・余白・最小幅は 17px 基準の同比率で `scaled_to()` がスケールする。レビュー指摘の 2 点は対応済み: 寸法を `Layout` 構造体に集約し `show_with_status()` 開始時に 1 回だけスナップショット（表示中の設定変更で寸法が混ざらない）、`fit_font_height()` が作業領域の高さに収まる最大のフォント高さを算出（下限 9px、単調性を利用した線形探索）。幅と X 位置も `clamp_width_to_work_area` / `calc_window_x` で作業領域内に収める。ユニットテスト 6 件（既定 17px で従来寸法と完全一致、10/17/24/72 の scale 検算、9 候補 + status + pager の収まりと最大性、下限、モニター情報なし）。
- 0.11.2 の main へローカルで rebase し、`cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --check` / `cargo test --workspace`（tsf 93 passed）を確認後、rebase マージ（main `0381599`、4 コミット直線）。Issue #3 はコミットメッセージの Closes で自動クローズ。
- 追加対応（ユーザー要望）: WinUI 設定アプリの「候補表示」カードにフォントサイズ項目を追加（`SettingsStore` が `[appearance]` を読み書き、10〜72 の範囲検証付き）。表記は「フォント高さ」ではなく「フォントサイズ px」とした（config キーは `candidate_font_height` のまま）。保存後の「反映しました」InfoBar は、設定を 1 つでも変更した時点で閉じるようにした（`WireStatusBarDismissal()`）。
- 「フォントサイズ変更が反映しないときがある」の原因を特定: 保存通知の `Local\rakukan.engine.reload` は auto-reset イベントで、SetEvent が起こすのは待機スレッド 1 本だけ。TSF DLL はアプリごとに別プロセスのため、イベントを受け取れなかったプロセスでは atomic が更新されなかった。`refresh_appearance_if_changed()`（候補表示直前に config.toml の mtime を確認して appearance だけ更新、表示 1 回につき stat 1 回）で修正。CONFIG_MANAGER の mtime キャッシュは意図的に消費しない（モード切替時の `reload_if_changed` が「変更なし」と誤判定してエンジン再起動をスキップするのを防ぐ）。
- 実機確認済み（2026-09-03）: フォントサイズ変更の反映、設定 GUI からの保存、InfoBar の消去。複数モニター・画面端（大きいフォントサイズでの位置補正・作業領域への収まり）も確認し問題なし。Step 11 の検証項目はすべて完了。

### Issue #18 対応方針（2026-09-04）

Issue #18（nick20002005 氏のローカル統合ブランチ未上流化分の棚卸し、カテゴリ A〜I）への返信を投稿し、受け入れ順を指定した（https://github.com/fukuyori/rakukan/issues/18#issuecomment-5533907994）。以降の計画は次のとおり修正して運用する。依頼形式は現行 main 基準・1 テーマ 1 PR。

- **Step 10 の前に不具合修正の受け入れフェーズを挿入**。対象は次の 4 群で、いずれも TSF 側のため Step 10（engine 側）と衝突しない。まとまったところで不具合修正リリースとする。
  1. C（Enter / 確定まわり 5 件）
  2. `KEYBOARD_OPENCLOSE` コンパートメントの外部変更への追随（H から不具合修正として分離）
  3. I（数詞の挿入位置。現行 main `dc2a96b` 以降での再現確認後）
  4. F のうち未確定ローマ字の再構築に触れないもの（ひらがなモードの英単語破壊、英単語の右端）
  - 到着済みの PR #17（キャレット矩形。プロセス起動後初回打鍵時に `CARET_RECT` が初期値のまま予測ウィンドウが (0,0) に出る問題）もこのフェーズでレビューする。
- **Step 10 は B の実装構造の回答で進め方を分岐**する。B（Backspace 後に素通しした子音を未確定ローマ字へ戻す）が Step 10 想定の「入力ログからの再生」構造なら、B を土台として受けてこちらはレビューと受け入れテスト（本計画 Step 10 の検証項目）に回る。Backspace 専用の分岐なら計画どおりこちらで実装し、先方に rebase してもらう。F の「Shift+英字が未確定ローマ字を追い越す」は Step 10 に合流させる。
- **Step 12 は E（学習 3 件）を設計材料として先に受け取る**。E は学習仕様の判断そのもののため PR では受けず、実装前に設計意図と参考差分の提示を依頼した。精査のうえ取り込めるものは Step 12 の設計へ折り込み、こちらで実装する（学習履歴 v2 の永続形式を二度動かさないため）。
- **9 月計画完了後に 0.12 系の新マイルストーンを設け、機能追加を受ける**。
  - A（文節変換一式）: `DictLookup` API 単独 PR（protocol v7 / ABI 変更をここだけで消化）→ 本体の 2 段構成を依頼。`DictLookup` は Issue #16（かな run の辞書参照）への転用可能性も検証する。
  - D（候補・予測品質）: 項目ごとに個別判断。「数字＋助数詞」は Step 9 の `reading_licenses_units` との整合審査が必要。
  - G（三点リーダー畳み込み）: 設定で opt-in（既定 off）を条件に受ける。
  - H（`text_field_mode`）: 必要性は認めるが初版は簡易な形に限定 — 設定キーはグローバル 1 個のみ（アプリごとのリスト管理なし）・既定 off・設定 GUI への追加なし。
- 計画への影響: 不具合修正フェーズ以降、こちらの作業の中心は実装からレビューと受け入れ検証へ移る。Step 10 は B の構造次第で負荷が下がる可能性がある。Step 12〜13 の内容・順序は変更しない。
