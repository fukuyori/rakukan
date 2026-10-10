# 開いている Issue の状況

2026-10-10 時点。GitHub の一覧（`gh issue list --state open`、23 件）と [handoff.md](handoff.md) の記録を照合して更新した。
最新リリースは 0.12.0（2026-10-10、タグ `0.12.0` = main `c15fefe`）。10 月の作業計画（#71 → #69 / #70 → #33）は [October_Plan.md](October_Plan.md)。作業計画は [September_Late_Plan.md](September_Late_Plan.md)、
インストーラー再設計は [Installer_Redesign_Plan.md](Installer_Redesign_Plan.md)。

Issue の状態を更新したら、この表も同じコミットで更新する。クローズしたら行を削除し、末尾の「クローズの記録」に移す。

## 1. 対応中・待ち

| Issue | 概要 | 対応状況 | 次の動き |
|---|---|---|---|
| [#71](https://github.com/fukuyori/rakukan/issues/71) | フォーカス移動で `ResetAll` が送られず、LLM の文脈が消えない（PR #68 以降） | 2026-10-10 起票。原因と直し方（照合と制約を含む）を記録、Codex CLI のレビュー済み | 0.12.0 に入れるかは未決 |
| [#69](https://github.com/fukuyori/rakukan/issues/69) | ホストとの通信失敗時に、打鍵や確定する文字列が失われる経路を見直す | 2026-10-09 起票。v6 の条件に入れない。T2-3 の表示の食い違い（2026-10-10）も資料 | 「キーをどこまで処理したか」の定義から設計 |
| [#70](https://github.com/fukuyori/rakukan/issues/70) | エンジンの準備前・ロック競合中に打ったキーがアプリへ素通りする | 2026-10-09 起票。v6 の条件に入れない | #69 と合わせて設計 |
| [#57](https://github.com/fukuyori/rakukan/issues/57) | 変換が 250 ms を超えて返ると詰まりの開始時刻が残り、後の変換でエンジンが誤って再起動される | [PR #59](https://github.com/fukuyori/rakukan/pull/59)（nick）をマージ済み。詰まりの監視をホストへ移し、実行番号で数える。engine ABI 9 → 10。0.11.9 に含まれる | 意図的に詰まりを起こす手段が無く実機未確認。運用ログで誤再起動が無いことを確認してクローズ |
| [#50](https://github.com/fukuyori/rakukan/issues/50) | 破棄済み DocumentManager のポインタが `dm_modes` に入り直し、アドレス再利用で前のモードが復元される | `c078e51` で修正（DM をポインタ + 世代で識別、Activate で既存 DM を列挙、Deactivate で全失効）。実機確認は範囲付きで済み、0.11.9 に含まれる | 運用ログで `unknown dm` / `skipped save for dead dm` の有無と回帰を見てクローズ |
| [#51](https://github.com/fukuyori/rakukan/issues/51) | アプリごとの IME 初期状態を config で設定し、GUI からも操作できるようにする | 段 1（`[input] ime_on_apps` / `ime_off_apps`、[PR #52](https://github.com/fukuyori/rakukan/pull/52)）は 0.11.9 に含まれる | 段 2 = 設定アプリ（WinUI）での編集画面。実行中のアプリから選ばせたい。#33 と時期を調整 |
| [#45](https://github.com/fukuyori/rakukan/issues/45) | 速く打つとライブ変換のプレビューが連続入力中に一度も更新されない | nick の PR 待ち（#40 の 2026-09-12 のコメントで依頼済み） | PR が来たらレビュー |
| [#40](https://github.com/fukuyori/rakukan/issues/40) | 候補・予測の品質に関する統合ブランチ側の報告 7 件の再現確認 | 1・3 は main で再現せず、7 は #45 に切り出し、2 は縮小版の PR を依頼済み、4 は見送り | 2 の PR が来たらレビュー。残りが片付いたらクローズ |

## 2. 判断待ち（レモンの決定が要る）

| Issue | 概要 | 対応状況 | 次の動き |
|---|---|---|---|
| [#66](https://github.com/fukuyori/rakukan/issues/66) | ホスト再接続時の古い設定の再送による巻き戻りを防止する | 2026-09-22 起票、設計案あり（`config_version` の照合とホスト再起動への統一）。方式採用・実装着手は未承認。#67 で `config_version` の項目だけ先取りする形を返信中 | 方式の承認。設計に「`config_version = None` は一致として受理しない」を明記する。#49 / #64 と合わせて設計 |
| [#64](https://github.com/fukuyori/rakukan/issues/64) | config 不一致の `Create` で、ワーカーが残る旧エンジン DLL をアンロードしうる | 2026-09-21 起票。対処方針は未決定、再現・実機検証は未実施。#66 に、古い設定の拒否とホスト内のエンジン置き換え・`Reload` の廃止を組み合わせる案を記録 | #66 と同時に方針を決める |
| [#49](https://github.com/fukuyori/rakukan/issues/49) | `model_variant` を変更してもホストのプロセスが続く限り古いモデルのまま動く | 未着手。計画書の J-3（推奨: モデル設定の変更でホストを終了） | #66 と合わせて設計 |
| [#35](https://github.com/fukuyori/rakukan/issues/35) | 区読点だけの読み（。。。）が変換に乗らず、リーダー記号（… ‥）の標準的な入力手段が無い | 判断待ち。計画書の J-6（推奨: Space 変換の候補を先に、`z` キー列は後）。[Symbol_Leader_Input_Plan.md](Symbol_Leader_Input_Plan.md) の 3.1.1 の A〜C が未決。PR #31 は nick が取り下げ済み | 方式と文字の割り当てを決める |
| [#16](https://github.com/fukuyori/rakukan/issues/16) | 数字混在の読みで、かな run が辞書（ユーザー辞書・学習履歴・MOZC）候補を参照できない | 判断待ち。計画書の J-7（推奨: 段 4 で設計から）。変換ワーカーへ辞書を渡す配線は #53 と共通 | 段 4 で設計 |
| [#29](https://github.com/fukuyori/rakukan/issues/29) | ユーザー辞書エントリに `priority = "low"` を追加する | 保留 | 要望があれば再検討 |

## 3. 別計画で扱う

| Issue | 概要 | 対応状況 | 次の動き |
|---|---|---|---|
| [#33](https://github.com/fukuyori/rakukan/issues/33) | インストーラー再設計（`%ProgramFiles%` 移行、実行後に再起動で完了） | [Installer_Redesign_Plan.md](Installer_Redesign_Plan.md)。2026-10-10 に方式を決定（`restartreplace`、再起動で完了、旧ホストは止めない）。ログの見直しから回した事項（4.3a）と、段 2〜5（#73〜#76、4.3b）を組み込んだ | 0.12.0 の後に着手 |
| #73〜#76 | ログ見直し 段 2〜5（書き手を 1 つに・発生元、掃除をトレイへ、ホストと engine DLL のログを揃える、書き込みを別スレッドへ・ETW） | 2026-10-10 起票。設計は [Log_Redesign_Plan.md](Log_Redesign_Plan.md) | #33 の対応の中で行う |
| [#46](https://github.com/fukuyori/rakukan/issues/46) | 学習履歴を個別に削除する手段が無い | #33 と合わせる予定 | #33 と同時 |
| [#32](https://github.com/fukuyori/rakukan/issues/32) | 文節変換: 区読点を含まない文で変換対象を文節単位に分けて選び直せない | [Segment_Edit_Plan.md](Segment_Edit_Plan.md)。9〜10 月は対応しない | 計画とインストーラー改修の後 |
| [#63](https://github.com/fukuyori/rakukan/issues/63) | 文節変換: 読みと変換結果を文節単位で対応付ける方式の検討 | 方式 1（かなアンカー）は不採用、2（辞書ラティス）/ 3（形態素解析）/ 4（jinen スコアリング）を比較して記録。実装予定なし | 進める前に文節の定義・発火タイミング・実験の順番を決める |
| [#18](https://github.com/fukuyori/rakukan/issues/18) | ローカル統合ブランチの未上流化の修正・機能の棚卸し | 各項目は個別 Issue へ分割済み。記録として残している | なし |

## 4. クローズ候補

- #57 と #50 は修正が 0.11.9 で出ている。運用ログの確認が済めばクローズできる。

## 5. クローズの記録（2026-09 以降）

| Issue | 内容 | クローズ日 |
|---|---|---|
| #56 / #65 / #72 | ホスト交換中の未確定文字の復元（PR #67 / #68、nick）/ 再接続時に config.toml の設定で Create / DllMain を空にし Activate で初期化（regsvr32 の失敗） | 2026-10-10（0.12.0） |
| #53 | 読みが正当化する参・拾を数字保存の検証で数えない（PR #58、nick） | 2026-09-21 |
| #54 | RPC クライアントのログが TSF のログに記録されない | 2026-09-24 |
| #55 | spawn 後の接続失敗を `HostSpawnGuard` に数える | 2026-09-22 |
| #60 / #61 / #62 | プロセス別ログ / config 読込失敗時に既定値へ戻さない / MOZC 辞書の取得元固定 | 2026-09-19 |
