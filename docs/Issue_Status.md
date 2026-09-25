# 開いている Issue の状況

2026-09-25 時点。GitHub の一覧（`gh issue list --state open`、18 件）と [handoff.md](handoff.md) の記録を照合して作成した。
最新リリースは 0.11.9（2026-09-24、main `4399279`）。作業計画は [September_Late_Plan.md](September_Late_Plan.md)、
インストーラー再設計は [Installer_Redesign_Plan.md](Installer_Redesign_Plan.md)。

Issue の状態を更新したら、この表も同じコミットで更新する。クローズしたら行を削除し、末尾の「クローズの記録」に移す。

## 1. 対応中・待ち

| Issue | 概要 | 対応状況 | 次の動き |
|---|---|---|---|
| [#56](https://github.com/fukuyori/rakukan/issues/56) | ホスト再起動後、未確定文字が残ると Space / Backspace が効かず、Esc で消すしかない | nick が実装中。2026-09-25 に Draft [PR #67](https://github.com/fukuyori/rakukan/pull/67)（(a) の最初のコミット、要求・応答の形だけ、プロトコル v6）が出て、同日に 5 点への回答と追加 3 点（`Restored` に `then` の結果を返す型、`ShutdownIfConfigDiffers` の `host_id` 不一致は `ShutdownSkipped`、`config_version: Option<[u8; 32]>` を `Create` / `ShutdownIfConfigDiffers` / `Change` に今回足す）を[返信](https://github.com/fukuyori/rakukan/pull/67#issuecomment-5828464616) | nick の反映を確認して形を確定 → (a) の残り → (b)。(a) 完成後にマージ、(b) も入ってから次版で 1 回リリース |
| [#65](https://github.com/fukuyori/rakukan/issues/65) | 設定変更を全 TSF プロセスへ反映する（通知漏れ・同一 mtime の変更に対応） | 実装・故障試験（[Issue65_Fault_Test_Design.md](Issue65_Fault_Test_Design.md)）まで完了し、0.11.9 に含まれる | 残件は再接続時に公開済みの設定で `Create` する組込み（`ShutdownSkipped` を「同じ設定」と区別して反映待ちを残す扱いを含む）。#67 の (a) の形が固まってから着手 |
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
| [#33](https://github.com/fukuyori/rakukan/issues/33) | インストーラー再設計（`%ProgramFiles%` 移行、実行後に再起動またはサインアウト・サインインで完了） | [Installer_Redesign_Plan.md](Installer_Redesign_Plan.md)（2026-09-25 に改名し、ビルド出力からの収集・故障試験タスクの移行先・署名経路を補完、`7cef4a7`）。旧ホストの停止方針も組み込み済み | 着手は 9 月計画の後（10 月） |
| [#46](https://github.com/fukuyori/rakukan/issues/46) | 学習履歴を個別に削除する手段が無い | #33 と合わせる予定 | #33 と同時 |
| [#32](https://github.com/fukuyori/rakukan/issues/32) | 文節変換: 区読点を含まない文で変換対象を文節単位に分けて選び直せない | [Segment_Edit_Plan.md](Segment_Edit_Plan.md)。9〜10 月は対応しない | 計画とインストーラー改修の後 |
| [#63](https://github.com/fukuyori/rakukan/issues/63) | 文節変換: 読みと変換結果を文節単位で対応付ける方式の検討 | 方式 1（かなアンカー）は不採用、2（辞書ラティス）/ 3（形態素解析）/ 4（jinen スコアリング）を比較して記録。実装予定なし | 進める前に文節の定義・発火タイミング・実験の順番を決める |
| [#18](https://github.com/fukuyori/rakukan/issues/18) | ローカル統合ブランチの未上流化の修正・機能の棚卸し | 各項目は個別 Issue へ分割済み。記録として残している | なし |

## 4. クローズ候補

- #57 と #50 は修正が 0.11.9 で出ている。運用ログの確認が済めばクローズできる。
- #65 は再接続の組込みまで開けておく。

## 5. クローズの記録（2026-09 以降）

| Issue | 内容 | クローズ日 |
|---|---|---|
| #53 | 読みが正当化する参・拾を数字保存の検証で数えない（PR #58、nick） | 2026-09-21 |
| #54 | RPC クライアントのログが TSF のログに記録されない | 2026-09-24 |
| #55 | spawn 後の接続失敗を `HostSpawnGuard` に数える | 2026-09-22 |
| #60 / #61 / #62 | プロセス別ログ / config 読込失敗時に既定値へ戻さない / MOZC 辞書の取得元固定 | 2026-09-19 |
