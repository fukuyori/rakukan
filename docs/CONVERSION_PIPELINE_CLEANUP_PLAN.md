# Conversion Pipeline Cleanup Plan

バージョン: draft-1
作成日: 2026-05-01
前提: v0.8.7 時点の rakukan コードベース

## 目的

Space 変換、ライブ変換、候補表示、後追い候補更新の処理を整理し、最終的な変換方式の見直しに進めるための段階計画を定義する。

現状の rakukan には、ライブ変換、Space 変換、Waiting timer、`llm_pending` 更新、RangeSelect、同期 fallback、過去の分節変換試行の痕跡が並存している。これらをいきなり置き換えると候補ウィンドウや composition 更新を壊す危険が高いため、まず現行経路を監査し、責務境界を整理する。

## 基本方針

- 候補ウィンドウは変換を開始しない。
- 候補ウィンドウは `SessionState` の表示に徹する。
- Space 1回目で候補選択状態に入ることを必須条件にする。
- 重い変換は Space 押下時に初めて始めるのではなく、事前計算または後追い更新に寄せる。
- `Waiting` は候補表を出せない特殊状態に限定し、通常は `Selecting { llm_pending: true }` に寄せる。
- `convert_sync` fallback は通常経路から隔離し、最終手段として扱う。
- 分節変換や形態素解析の導入は、現行経路整理後の別フェーズで判断する。

## 最終形態の責務分離

```text
input handling
  reading / preedit を更新する

candidate producer
  辞書候補、学習候補、LLM候補を生成する

candidate snapshot
  reading ごとの候補状態を保持する

selection state
  候補選択、ページング、pending 状態を管理する

candidate window
  現在の選択状態を描画する

composition updater
  TSF composition text を更新する
```

## Phase 1: 現行変換経路の監査

目的: 現在存在する変換経路をすべて列挙し、責務、重複、危険箇所を明確にする。

対象経路:

- 通常入力中のライブ変換
- Space による通常変換
- `Waiting` timer による候補取得
- `Selecting { llm_pending: true }` の後追い更新
- `engine_convert_sync_multi` による同期 fallback
- RangeSelect の部分変換
- 句読点入力時の変換
- LiveConv から Space / Enter / Backspace / Esc への遷移

確認する API:

- `bg_start`
- `bg_take_candidates`
- `bg_peek_top_candidate`
- `bg_reclaim`
- `bg_wait_ms`
- `convert_sync`
- `merge_candidates`
- `candidate_window::show_with_status`
- `SessionState::set_waiting`
- `SessionState::activate_selecting`

成果物:

- 各入力イベントから候補表示までの経路図
- 各経路が使う engine/RPC API の一覧
- `Waiting` に落ちる条件
- `sync_multi_fallback` に落ちる条件
- `bg_take_candidates None` の原因分類
- 重複している候補生成、待機、マージ、表示処理の一覧
- Phase 2 以降で残す経路、縮小する経路、削除候補の整理

このフェーズでは原則として挙動を変更しない。

### Phase 1 監査結果

対象コード:

- `crates/rakukan-tsf/src/tsf/factory/on_input.rs`
- `crates/rakukan-tsf/src/tsf/factory/on_convert.rs`
- `crates/rakukan-tsf/src/tsf/factory/edit_ops.rs`
- `crates/rakukan-tsf/src/tsf/factory/dispatch.rs`
- `crates/rakukan-tsf/src/tsf/candidate_window.rs`
- `crates/rakukan-tsf/src/engine/state.rs`
- `crates/rakukan-engine/src/lib.rs`
- `crates/rakukan-engine/src/conv_cache.rs`
- `crates/rakukan-engine-rpc/src/client.rs`
- `crates/rakukan-engine-rpc/src/server.rs`
- `crates/rakukan-engine-rpc/src/protocol.rs`

#### 状態モデル

`SessionState` は現在、以下の変換関連状態を持つ。

| 状態 | 主な意味 | 候補表 | 注意点 |
|---|---|---:|---|
| `Idle` | 入力なし | なし | 変換経路の起点ではない |
| `Preedit` | 未変換の入力あり | なし | Space で通常変換へ進む |
| `LiveConv` | ライブ変換 preview を composition に表示中 | なし | Space で通常変換へ戻り、Enter で preview 確定 |
| `Waiting` | BG 候補待ち | あり/なしが経路で混在 | 通常変換と RangeSelect の両方で使われる |
| `Selecting` | 候補選択中 | あり | `llm_pending` で後追い候補待ちも表す |
| `RangeSelect` | 変換範囲選択中 | なし | Space で選択範囲だけ候補表示へ進む |

`Waiting` と `Selecting { llm_pending: true }` はどちらも「候補待ち」を表せるため、責務が重なっている。最終形態では、通常の Space 変換は `Selecting { llm_pending: true }` に寄せ、`Waiting` は候補表をまだ出せない特殊ケースへ縮小するのが自然。

#### Engine / RPC API の意味

| API | 現在の意味 | 注意点 |
|---|---|---|
| `InputChar` | 入力反映、preedit/hiragana/bg_status 取得、任意で `bg_start` | TSF 側では現在 `bg_start_n_cands=None` で呼ぶ経路が中心 |
| `bg_start(n)` | 現在の hiragana を key に BG 変換を開始 | `bg_start` 冒頭で Done converter を回収するため、既存候補を捨てることがある |
| `bg_status()` | `idle/running/done` などを返す | key は返さないため、呼び出し側が key mismatch を別途処理する |
| `bg_peek_top_candidate(key)` | Done 候補の先頭だけを見る | cache 状態を進めない。ライブ preview 向け |
| `bg_take_candidates(key)` | Done 候補を取り出し converter を engine に戻す | key mismatch / 未完了 / 空候補 / RPC 失敗が TSF 側では区別しにくい |
| `bg_reclaim()` | Done converter を回収し、候補は破棄 | 安全回収と候補破棄が同じ操作になっている |
| `bg_wait_ms(ms)` | BG 完了を短時間待つ | Space hot path に待機を持ち込む |
| `convert_sync()` | 同期変換 | fallback として残っており、体感ラグの原因になりうる |
| `merge_candidates(llm, limit)` | user/learn/LLM/dict を統合 | 辞書即時候補生成と LLM 結果のマージを兼ねる |

#### イベント別経路監査

| イベント | 開始状態 | 主な経路 | 候補表示タイミング | composition 更新 | fallback / 問題点 | 判定 |
|---|---|---|---|---|---|---|
| 通常文字入力 | `Preedit` / `LiveConv` / `RangeSelect` | `input_char(..., None)` 後に `start_live_bg_if_ready` | 候補表なし。ライブ preview は timer 後 | `update_composition` | 入力時 prefetch はライブ用。Space 用候補とは明示的に共有されていない | 残す。ただし snapshot 化で Space と共有 |
| ライブ変換 preview | `Preedit` | `start_live_bg_if_ready` → live timer → `bg_peek_top_candidate` | 候補表なし | timer/edit session 経由で preview 反映 | `bg_peek` は先頭のみ。Space 候補全体とは別物 | 残す。ただし preview と Space 候補の関係を整理 |
| Space 通常変換 | `Preedit` / `LiveConv` | `bg_reclaim` → `bg_start(num_candidates)` → `bg_wait_ms(250)` → `bg_take_candidates` → `merge_candidates` | 完了すれば `activate_selecting` 後に表示。未完了なら `Waiting` + status 表示 | 最後に `update_composition(first)` | live BG を捨てて fresh 変換し直す可能性。`bg_take None` で `reclaim+restart+wait`。最後に `convert_sync` fallback | 最優先で整理 |
| Space 中の前 BG running | `Preedit` | `Waiting` 表示 → 既存 BG を待つ → `bg_reclaim` → 新 key で `bg_start` | 待機 status を先に表示 | 完了後のみ composition 更新 | 前の変換が converter を持っていると新変換開始まで待つ | snapshot/worker 所有権整理が必要 |
| `bg_take_candidates None` 再試行 | 通常 Space / `llm_pending` | `bg_reclaim` → `bg_start` → `bg_wait_ms` → 再 `bg_take` | 再試行完了まで遅れる | 再試行完了後 | None の原因が曖昧なまま重い再変換に進む | Phase 3 で分類必須、Phase 4 で表示優先へ変更 |
| `Waiting` timer | `Waiting` | timer → `bg_status == done` → `bg_take_candidates` → `merge_candidates` → `activate_selecting` | timer で候補表更新 | timer では composition 更新しない | 候補表だけ更新され、composition 更新は次のキー入力など別経路に依存 | 縮小対象 |
| dispatch poll: `llm_pending` | `Selecting { llm_pending: true }` | poll → `bg_take_candidates` → `merge_candidates` → candidates 更新 | 既存候補表を更新 | `update_composition_candidate_parts` | 成功時に `selected=0` へ戻す経路があり、ユーザー選択を壊す可能性 | 残すが更新ルール要整理 |
| on_convert: `llm_pending` | `Selecting { llm_pending: true }` | Space 再押下で最大 500ms 待機し、Done なら候補更新 | 既存候補表を維持または更新 | 候補更新後に composition 更新 | None なら `reclaim+restart` し、最大 1500ms 待つ経路がある | 待機と再起動を削減 |
| 句読点入力 | `Preedit` / `Selecting` | 選択中は `punct_pending` 更新。Preedit では `bg_start` / `bg_take` / sync fallback | 候補表を出し、status に確定後句読点を表示 | first candidate で composition 更新 | 通常 Space と似た候補生成ロジックが別実装 | 統合候補 |
| RangeSelect Space | `RangeSelect` | selected を `force_preedit` → `Waiting` → `bg_start` → inline wait/timer | 待機 status または候補表 | selected 部分のみ更新 | 通常 Space と似た待機・表示処理を別に持つ | 後で通常 Space と共通化 |
| LiveConv Enter | `LiveConv` | preview を commit | 候補表なし | `end_composition` | preview が空なら false | 残す |
| LiveConv Backspace/Esc | `LiveConv` | reading に戻す / 1文字削除 / cancel | 候補表なし | preedit 表示へ戻す | `bg_reclaim` を伴う経路が複数 | 残すが reclaim 意味を整理 |

#### 重複している処理

- `bg_start` → 短時間待機 → `bg_take_candidates` → `merge_candidates` → `activate_selecting` → `candidate_window::show_with_status` が、通常 Space、RangeSelect、句読点、timer/poll 系に分散している。
- `bg_take_candidates None` の扱いが複数あり、ある経路では待機継続、別経路では `reclaim+restart`、別経路では sync fallback へ進む。
- `Waiting` から候補表を出す経路と、`Selecting { llm_pending: true }` で候補表を出したまま後追いする経路が共存している。
- `engine_convert_sync_multi` は通常 Space と句読点経路の fallback に残っており、非同期設計と同期設計が混ざっている。
- Space 用の `num_candidates` とライブ変換用の `live_conv_beam_size` が、入力時 prefetch と Space 時 fresh 変換で分かれており、候補再利用の設計が明確でない。

#### 危険箇所

- `bg_reclaim` は converter 回収と候補破棄を同時に行うため、Space 冒頭で呼ぶと利用可能な Done 候補を失う可能性がある。
- `bg_status == done` だけでは key が合っているか分からない。
- `bg_take_candidates` が `None` を返す原因が、未完了、key mismatch、空候補、RPC 失敗のどれか TSF 側で区別しにくい。
- timer からは composition text を直接更新しないため、候補表だけが新しくなり composition が古いまま残る経路がある。
- 後追い候補更新で候補配列を差し替えると、選択中インデックスやページ位置を壊す可能性がある。
- `convert_sync` fallback は候補表示の遅延を隠す一方で、Space hot path に重い同期処理を戻してしまう。

#### Phase 1 結論

最初に整理すべき対象は候補ウィンドウではなく、通常 Space 経路である。

優先順位:

1. Space 通常変換の `bg_reclaim` / `bg_start` / `bg_wait_ms` / `bg_take_candidates` / `convert_sync` の流れを分類する。
2. `Waiting` と `Selecting { llm_pending: true }` の使い分けを決める。
3. `bg_take_candidates None` の原因をログ上で区別できるようにする。
4. 句読点と RangeSelect の候補生成を通常 Space の共通経路へ寄せる。
5. ライブ preview の BG 結果を Space 初期候補へ昇格できるか検討する。

Phase 2 では、上記をもとに責務境界を定義する。特に、`candidate_window` は表示専用、Space は snapshot 昇格、候補生成は producer/snapshot 側、という分担を具体化する。

## Phase 2: 責務境界の定義

目的: 最終形態で各部品が担当する処理を明確にする。

決めること:

- `on_input` が担う範囲
- `on_convert` が担う範囲
- `candidate_window` が担う範囲
- `SessionState` が保持すべき状態
- engine-host 側に置くべき候補生成状態
- TSF 側に残すべき UI 状態

目標:

- Space ハンドラが候補生成と候補表示の両方を抱えない。
- 候補ウィンドウが状態遷移を主導しない。
- timer は候補状態の進行を通知するだけに近づける。

### Phase 2 詳細: 最終責務境界

Phase 1 の結論として、現在の混乱は「候補を作る」「候補を待つ」「候補を表示する」「composition を更新する」が複数の経路に分散していることにある。Phase 2 では、まず最終形態の責務境界を以下のように定義する。

#### 責務一覧

| 責務 | 所有者 | やること | やらないこと |
|---|---|---|---|
| Input handling | `on_input.rs` / `on_input_raw` | キー入力を engine に反映し、preedit と reading を更新する | 候補表を直接制御しない。Space 用候補の完成を待たない |
| Conversion trigger | `on_convert.rs` | Space を「候補選択開始」イベントとして扱う | 重い変換完了を必ず待たない。候補表の描画詳細を持たない |
| Candidate producer | engine-host / `rakukan-engine` | 辞書候補、学習候補、LLM候補を生成する | TSF の選択状態や候補ウィンドウを知らない |
| Candidate snapshot | engine-host 側を第一候補 | reading ごとの候補状態、generation、pending/done を保持する | UI 表示状態を持たない |
| Selection state | `SessionState::Selecting` | 候補一覧、選択 index、page、pending、prefix/remainder を保持する | 新規候補生成を開始しない |
| Waiting state | `SessionState::Waiting` | 候補表をまだ出せない例外状態を表す | 通常 Space 変換の標準状態にしない |
| Candidate window | `candidate_window.rs` | `SessionState` から渡された page candidates を描画する | `bg_start` / `bg_take_candidates` / `merge_candidates` を直接主導しない |
| Composition updater | `update_composition*` 呼び出し側 | 選択中候補を TSF composition に反映する | 候補生成や候補選択を決めない |
| Timer / poll | `candidate_window` timer / `dispatch.rs` | pending 状態の進行を確認し、必要なら候補更新を要求する | 通常経路の主制御を持たない |
| Learning | engine-host / `DictStore` | 確定した reading/surface を学習する | 候補表示順の UI 状態を直接変えない |

#### Space の最終責務

Space は「変換を始めるキー」ではなく、最終的には「現在 reading の候補 snapshot を選択状態へ昇格するキー」として扱う。

```text
Space
  -> current reading を確定
  -> snapshot を取得または作成
  -> 最小候補セットで Selecting に入る
  -> 候補表を表示
  -> full candidates は後追い更新
```

Space が直接持つべき処理:

- `Preedit` / `LiveConv` / `RangeSelect` から変換対象 reading を決める。
- 表示可能な候補セットを受け取り、`SessionState::Selecting` を開始する。
- composition に最初の候補を反映する。

Space が持つべきでない処理:

- LLM 変換完了を必ず待つ。
- `bg_take_candidates None` の原因を推測して、その場で複雑な再変換を組み立てる。
- 候補生成、辞書候補取得、LLM 候補取得、マージの詳細を個別に持つ。
- 候補ウィンドウの内部描画仕様を知る。

#### 候補生成の最終責務

候補生成は engine-host 側の責務に寄せる。

候補生成側が持つべき情報:

- reading
- committed context
- generation
- dict candidates
- learned/user candidates
- LLM candidates
- merged candidates
- status: `empty` / `dict_ready` / `llm_running` / `llm_done` / `error`

TSF 側が直接持つ候補生成ロジックは、当面の移行期間を除いて縮小する。

現状の `merge_candidates` は engine 側にあるため、候補統合の責務は engine-host に置くのが自然。ただし、`SessionState` の選択 index や page は TSF 側に残す。

#### CandidateSnapshot の責務

Phase 6 で導入する snapshot は、Phase 2 の責務定義上は以下の契約を持つ。

```text
CandidateSnapshot {
  reading,
  generation,
  status,
  immediate_candidates,
  full_candidates,
  selected_default,
}
```

`immediate_candidates` は Space 1回目で表示できる候補である。辞書候補、学習候補、既存 BG 候補、ひらがな fallback のいずれかを含む。

`full_candidates` は後追いで増える候補である。LLM 候補やより広い beam の候補がここに入る。

TSF 側は snapshot の内部生成方法を知らない。TSF 側は `reading` と `generation` が現在の入力と合っているかだけを確認する。

#### SessionState の責務

`SessionState` は UI/選択状態を表す。候補生成状態そのものではない。

残す責務:

- 現在の論理状態
- original preedit / reading
- 表示中 candidates
- selected index
- page size
- `llm_pending`
- prefix / remainder
- punctuation pending
- candidate window position

縮小する責務:

- `Waiting` を通常 Space 変換の標準状態として使うこと。
- `Waiting` から候補生成を再開すること。

将来の整理方向:

```text
Preedit
  入力中

LiveConv
  preview 表示中

Selecting { pending: bool }
  候補表表示中。pending=true なら後追い候補待ち

RangeSelect
  範囲選択中

Waiting
  候補表を出せない例外状態
```

#### Candidate window の責務

`candidate_window.rs` は描画と lightweight timer に限定する。

残す責務:

- page candidates の描画
- selected row の描画
- status line の描画
- caret 近傍への配置
- waiting/live timer の発火口

縮小する責務:

- timer 内で候補取得、マージ、`activate_selecting` を直接行うこと。
- `bg_take_candidates` の key mismatch recovery を timer 側で持つこと。

移行期間では timer 内の既存処理は残してよいが、最終的には「pending snapshot の更新通知」に近づける。

#### Timer / poll の責務

timer と poll は「状態が進んだか」を確認するだけにする。

望ましい流れ:

```text
timer/poll
  -> snapshot status を確認
  -> done なら Selecting candidates を更新
  -> candidate_window を再描画
```

避ける流れ:

```text
timer/poll
  -> bg_take_candidates が失敗
  -> bg_reclaim
  -> bg_start
  -> 再待機
```

これは通常 Space 経路と同じ複雑さを timer 側に複製するため、最終形態では避ける。

#### Composition 更新の責務

composition text の更新は TSF edit session 文脈に依存するため、候補生成とは分ける。

原則:

- `Selecting` 開始時は最初の候補を composition に反映する。
- 候補選択変更時は現在候補を composition に反映する。
- 後追い候補更新時は、ユーザーが選択中の候補を不必要に変更しない。
- timer から composition を直接更新できない場合は、候補表だけ更新する経路と composition 更新経路を明示的に分ける。

#### 当面の移行ルール

Phase 3 以降の実装では、以下を守る。

- `candidate_window` の描画仕様は Phase 4 までは変更しない。
- Space 1回目の候補表示を壊す変更はしない。
- `Waiting` を増やす変更は避ける。
- 新規 fallback を追加する前に、既存 fallback の分類ログを追加する。
- `bg_reclaim` を呼ぶ箇所では、候補破棄が許容されるかを明示する。
- 後追い候補更新では、既存の `selected` を維持できる場合は維持する。
- 句読点、RangeSelect、通常 Space の候補生成ロジックは、最終的に共通 helper に寄せる。

#### Phase 2 結論

最終責務境界では、Space は候補生成を完了させる場所ではなく、候補 snapshot を選択 UI に昇格させる場所になる。

次に進む Phase 3 では、実装を変える前に、現在の Space 経路がどの分類で終わったかをログ上で判別できるようにする。特に `bg_take_candidates None` と `convert_sync` fallback の発生条件を可視化する。

## Phase 3: 計測と経路分類の整備

目的: Space 候補表示のラグがどの経路で起きているかをログで判別できるようにする。

分類したい結果:

```text
cache_hit
cache_miss
bg_running_wait
bg_take_key_mismatch
reclaim_restart
timer_fallback
sync_multi_fallback
shown_immediate
shown_after_wait
```

既存の `convert_timing` マーカーを活かしつつ、最終結果の分類を明示する。

### Phase 3 詳細: Space 経路分類ログ

Phase 3 では、通常 Space 変換の最終 `convert_timing result=...` ログに以下の分類フィールドを追加する。

```text
path
  新規 Space 変換が通った大枠の経路。

bg_take
  bg_take_candidates がどの key で成功/失敗したか。

candidate_source
  最終的に表示した候補の主な由来。

retry
  bg_take_candidates None 後の reclaim+restart を試したか。

sync_fallback
  convert_sync fallback を使ったか。
```

分類例:

```text
convert_timing result=shown \
  path=bg_running_wait \
  bg_take=hit_hiragana \
  candidate_source=bg \
  retry=false \
  sync_fallback=false \
  candidates=8 \
  llm_pending=false \
  total_us=...
```

`path` の主な値:

| 値 | 意味 |
|---|---|
| `new` | inline wait を必要としない通常経路 |
| `bg_running_wait` | Space 時点で現在 key の BG が running/idle 扱いになり、短時間待機した |
| `prev_bg_running_wait` | converter が前 BG に貸し出されている状態を待った |

`bg_take` の主な値:

| 値 | 意味 |
|---|---|
| `hit_hiragana` | `hiragana_text()` key で BG 候補取得に成功 |
| `hit_preedit` | `preedit` key で BG 候補取得に成功 |
| `miss_hiragana` | `hiragana_text()` key で失敗し、preedit retry は不要 |
| `miss_hiragana_preedit` | hiragana/preedit の両方で失敗 |
| `hit_after_retry` | reclaim+restart 後に取得成功 |
| `miss_after_retry` | reclaim+restart 後も取得失敗 |

`candidate_source` の主な値:

| 値 | 意味 |
|---|---|
| `bg` | BG 候補を merge して表示 |
| `bg_after_retry` | retry 後の BG 候補を merge して表示 |
| `sync_after_weak_merge` | BG 候補は取れたが merge 結果が弱く、同期 fallback を使った |
| `sync_no_bg` | BG 候補が取れず、同期 fallback を使った |
| `preedit_model_not_ready` | モデル未 ready のため preedit fallback を表示 |

このフェーズのコード変更は観測性のみを目的とし、候補表示順、待機時間、状態遷移は変更しない。

## Phase 4: Space 初回表示の安定化

目的: Space 1回目で必ず候補選択状態に入り、候補ウィンドウを表示する。

### 実機確認メモ: `にわにはにわにわとりがいる`

Phase 3 計測入りビルドの実機確認で、`にわにはにわにわとりがいる` の候補から
期待される `二羽` が消え、`庭には庭鶏がいる` 系の候補に寄ることを確認した。

辞書を直接確認した結果:

```text
にわ => 二輪, 庭, 丹羽, 二話, ...
には => には, 丹羽, 二派, ...
にわとり => 鶏, ニワトリ, ...
にわにはにわにわとりがいる => <none>
```

さらに `にわ` の上位 200 候補にも `二羽` は存在しなかった。

このため、この症状は候補ウィンドウ表示や Space 待機時間だけの問題ではない。
現行の「読み全体を LLM/辞書候補に投げ、最後に候補を merge する」方式では、
`庭には / 二羽 / 鶏がいる` のような語列候補を辞書から構成できない。

Phase 4 は初回表示の安定化に限定し、この品質問題を ad hoc な助詞推定や
`にわ` 専用補正で直さない。正しい対応は、既存辞書や外部形態素解析器を使った
ラティス生成と Viterbi 的な系列選択、または同等の既存エンジン利用を Phase 8 の
再設計対象として扱う。

### 実機確認メモ: 長文入力中に後方が消える

長文を入力していると、後ろの方から表示済みの文章が消えていく症状を確認した。

原因候補は LiveConv 継続入力時の表示合成にある。従来は LiveConv 状態で次の文字を
入力したとき、表示を以下のように組み立てていた。

```text
display = previous_preview + suffix_from_new_reading
```

この方式では、`previous_preview` が LLM の途中切れや短い候補だった場合、その preview
を次の表示の土台にしてしまう。結果として、engine 側の `hiragana_text` は残っていても、
composition 上の表示だけが後方から欠けていく。

最初の安全対策では、LiveConv 継続入力時に常に engine が持つ完全な preedit を
composition に戻した。その後、入力体験を補うために文字数比で preview の途中切れを
推定する暫定ガードも検討したが、これは理論的に弱い。

その後の確認で、入力中に毎回すべてひらがなへ戻ると入力体験が悪いことも確認した。
根本原因は表示合成そのものだけでなく、LLM の beam search が EOS 到達済みの
finished beam と、まだ EOS に到達していない active beam を同列に返していた点にもある。
active beam が高スコアで先頭に出ると、途中切れ preview が LiveConv に入る。

Phase 4 では、生成側で finished beam を優先し、finished beam が 1 件でもある場合は
active beam を候補に混ぜない。これにより、LiveConv の `previous_preview + suffix` 表示を
戻しつつ、途中切れ preview の継承リスクを下げる。

表示の原則は以下のとおり。

```text
1. 入力文字列は canonical state として保持する
2. 変換は常に canonical reading を入力として実行する
3. 表示は変換後 preview が得られた時だけ、current reading 全体に対応する preview へ更新する
4. 1-2 文字目は未変換 preedit をそのまま表示し、3 文字目からライブ変換を開始する
5. 未完了 beam 由来の preview はできるだけ LiveConv に入れない
```

これにより、長文で後方が消える問題と、最後の文字が表示合成から漏れる問題を、
表示側の文字数推定ではなく生成候補の完了性で抑える。

方針:

- 辞書候補、学習候補、既存 BG 候補、ひらがな fallback のいずれかで即表示する。
- LLM/full candidates は後追い更新にする。
- `Waiting` のまま候補表が出ない経路を減らす。
- 候補ウィンドウ自体の描画仕様は大きく変えない。

成功条件:

- Space 1回目で候補ウィンドウが表示される。
- 候補生成が未完了でも `Selecting { llm_pending: true }` として選択操作を開始できる。
- 後追い更新時に選択中インデックスを不必要にリセットしない。

### Phase 4 実装メモ: pending candidates の即時表示

通常 Space 変換で BG が running/idle の場合、従来は `Waiting` に入り、
`bg_wait_ms(250ms)` の完了を待ってから候補取得へ進んでいた。

Phase 4 では、この通常経路を以下に変更した。

```text
Space
  -> candidates = [space 時点の LiveConv preview]
     fallback: [preedit]
  -> Selecting { llm_pending: true }
  -> candidate_window を即表示
  -> waiting timer で BG 完了を後追い確認
  -> 完了後に candidates を差し替え、llm_pending=false
```

これにより、Space 1回目は重い候補生成の完了を待たず、候補選択状態へ入る。
LiveConv から Space へ進む場合は、入力 reading は canonical state として保持しつつ、
Space 押下時点で composition に出ていた preview を候補表の第1候補として使う。
そのため、候補表の先頭がハイライトされ、本文 composition も同じ候補を表示する。
preview がない Preedit 経路だけは preedit fallback を使う。

通常 Space 経路では、候補配列を直接候補ウィンドウへ渡す前に
`activate_selecting_snapshot` で `SessionState::Selecting` を作り、
その snapshot から以下を同時に取り出す。

```text
first
  composition に表示する現在候補

page_candidates / page_selected / page_info
  候補ウィンドウに表示する現在ページ
```

pending 表示と完了済み候補表示は、どちらもこの snapshot を表示元にする。
これにより、候補表のハイライト行と本文 composition の候補がずれないようにする。
ただし `kanji_ready=false` で前回 BG の converter 回収が必要な経路や、
`bg_take_candidates None` 後の retry 経路はまだ旧 Waiting/fallback が残る。
これらは Phase 5 で `Waiting` と `llm_pending` の責務をさらに整理する。

## Phase 5: Waiting と llm_pending の整理

目的: 候補待ち状態が複数あることによる混乱を減らす。

整理方針:

```text
Waiting
  候補表をまだ出せない特殊状態

Selecting { llm_pending: true }
  候補表は出ているが、追加候補待ちの通常状態
```

通常の Space 変換では `Waiting` ではなく `Selecting { llm_pending: true }` を基本にする。

## Phase 6: 変換 snapshot 化

目的: reading ごとの候補状態を一箇所にまとめ、ライブ変換、Space 変換、後追い更新で再利用できるようにする。

想定モデル:

```text
CandidateSnapshot {
  reading,
  generation,
  dict_candidates,
  learned_candidates,
  bg_candidates,
  merged_candidates,
  status,
}
```

検討事項:

- snapshot を engine-host 側に置くか、TSF 側に置くか
- generation mismatch の扱い
- live preview と Space candidates の共有方法
- dictionary-only snapshot の即時生成方法
- LLM 完了時の snapshot 更新方法

## Phase 7: 同期 fallback の隔離・削減

目的: `convert_sync` が候補表示の体感ラグを引き起こさないようにする。

方針:

- `engine_convert_sync_multi` を通常候補表示経路から外す。
- 表示可能な候補がある場合は同期変換を待たない。
- fallback が必要な場合も、ログ上で明確に分類する。
- fallback は候補表示後の補完処理、または明示的な最終手段に限定する。

## Phase 8: 現行変換方式の再設計

目的: 分節変換にこだわらず、整理済みの構造へ現行変換方式を移す。

検討対象:

- `live_conv_beam_size` と `convert_beam_size` の役割
- ライブ変換候補と Space 候補の共有
- 辞書候補、学習候補、LLM候補の統合順序
- 長文入力時の再変換単位
- bg worker / cache の所有権モデル

この段階では、候補生成の理論を刷新するより、既存の候補生成結果を無駄なく使うことを優先する。

## Phase 9: 長文変換・変換理論の再検討

目的: 長文変換や実用的な候補生成方式について、既存資産を正しく使う前提で再検討する。

検討対象:

- Mozc 本体利用
- Mozc 辞書の `lid` / `rid` / `cost` / connection 情報の利用
- ラティス / Viterbi 的な候補探索
- 長文 snapshot / 局所再計算
- 形態素解析器を補助として使う範囲

非方針:

- 変換後文章の形態素解析結果から入力ひらがな境界を復元する。
- 助詞や表層文字による当て推量で分割する。
- 独自理論で実用 IME 相当の変換器を作る。

## 直近の次アクション

まず Phase 1 を詳細化する。

Phase 1 の詳細化では、現行コードの各経路について以下を表にする。

```text
イベント
開始状態
呼び出す関数
engine/RPC API
候補表示タイミング
composition 更新タイミング
失敗時 fallback
問題点
残す/整理する/削除候補
```

この監査結果をもとに、Phase 2 で最終形態の責務境界を確定する。
