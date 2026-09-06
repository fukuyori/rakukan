# docs/archive — 過去の設計書・計画書

役目を終えた設計書・計画書の置き場。内容は作成当時のままで、現行コードとは乖離している。
現行の資料は [DESIGN.md](../DESIGN.md)、[handoff.md](../handoff.md)、[September_Revised_Plan.md](../September_Revised_Plan.md) を参照。

| ファイル | 使用した作業 | 期間 / バージョン | 結末 |
|---|---|---|---|
| [PHASE1_SUMMARY.md](PHASE1_SUMMARY.md) | Phase 1（エンジン基盤）の要約 | v0.2.0 | スナップショット |
| [PHASE2_PREP.md](PHASE2_PREP.md) | Phase 2（TSF 統合）着手前メモ | v0.2.0 | スナップショット |
| [PHASE2_STATUS.md](PHASE2_STATUS.md) | Phase 2 の進捗記録 | v0.2.0 | スナップショット |
| [WARNING_FIXES.md](WARNING_FIXES.md) | warning 修正のメモ | v0.2.0 | スナップショット |
| [VIBRATO_PHASE1.md](VIBRATO_PHASE1.md) | vibrato 形態素解析による分節 API の導入 | 2026-03-28 / v0.4.0 | v0.5.1（2026-04-16）で vibrato 完全削除 |
| [SEGMENT_EDIT_REDESIGN.md](SEGMENT_EDIT_REDESIGN.md) | 分節編集ロジック（Segment 列を正とする）の再設計 | 2026-03-30 / 0.4.x | 2026-04-13 に CONVERTER_REDESIGN.md へ継承 |
| [CONVERTER_REDESIGN.md](CONVERTER_REDESIGN.md) | 変換パイプライン / 文節編集の再設計（Phase A〜E） | 2026-04-13〜21 / v0.4.5〜0.6.4 | Phase A（数値保護・Segments 型）を v0.5.0 で実装。Phase B〜E は vibrato 削除で無効化 |
| [LIVE_CONV_REDESIGN_REVISED.md](LIVE_CONV_REDESIGN_REVISED.md) | Explorer 異常終了の再発防止を目的としたライブ変換再設計案 | 2026-04-22 / 0.6.x〜0.7.x | ROADMAP.md / handoff.md §18 で採否を仕分け |
| [ROADMAP.md](ROADMAP.md) | 0.7.x〜0.9.x の作業計画（M1〜、リファクタリングと再設計採用の段取り） | 2026-04-22〜06-24 | v0.9.12 でクローズ |
| [CONVERSION_PIPELINE_CLEANUP_PLAN.md](CONVERSION_PIPELINE_CLEANUP_PLAN.md) | 変換パイプライン整理計画（Phase 1〜9） | 2026-05-01〜06-22 / v0.8.11〜0.9.x | ROADMAP クローズと同時に更新停止 |
| [PHASE9_DESIGN.md](PHASE9_DESIGN.md) | 分節解析を含む変換方式見直しの設計ドラフト | 2026-05-12 / v0.9.1 | 設計検討段階のまま未着手 |
| [CONVERSION_ANOMALY_FIX_PLAN.md](CONVERSION_ANOMALY_FIX_PLAN.md) | 変換停止・異常変換（EOS 未到達ビームなど）の修正計画 | 2026-07-03〜13 / v0.9.12 | 2026-07-03 に実装完了 |
| [JULY_LOG_IMPROVEMENT_PLAN.md](JULY_LOG_IMPROVEMENT_PLAN.md) | 7月運用ログ分析にもとづく改善計画（エンジン再 init 乱発、echo strip 誤爆など） | 2026-08-04 / v0.9.16〜0.10.0 | Phase A〜 を 0.10.x で実装 |
| [AUGUST_LOG_IMPROVEMENT_PLAN.md](AUGUST_LOG_IMPROVEMENT_PLAN.md) | 8月運用ログ分析にもとづく改善計画 | 2026-09-01 / v0.10.5〜 | September_Revised_Plan.md（第2版）に引き継ぎ |