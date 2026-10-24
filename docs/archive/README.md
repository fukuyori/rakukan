# docs/archive — 過去の設計書・計画書

役目を終えた設計書・計画書の置き場。内容は作成当時のままで、現行コードとは乖離している。
現行の資料は [DESIGN.md](../DESIGN.md)、[handoff.md](../handoff.md)、[October_Plan.md](../October_Plan.md) を参照。
移した文書の中のリンクは、移す前の場所（`docs/`）を前提にしたままで、直していない。

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
| [September_Revised_Plan.md](September_Revised_Plan.md) | 9 月の作業計画（第 2 版。Step 1〜13、8 月のログ分析の引き継ぎ） | 2026-09-01〜09-15 / 0.11.0〜0.11.7 | September_Late_Plan.md に引き継ぎ |
| [Step10_Romaji_Rebuild_Plan.md](Step10_Romaji_Rebuild_Plan.md) | Step 10（ローマ字入力の状態を打鍵ログから作り直す、#34）の詳細設計 | 2026-09-07〜09-12 / 0.11.5 | 0.11.5 で実装 |
| [September_Late_Plan.md](September_Late_Plan.md) | 9 月後半の作業計画（#56 / #65 / #72、v6 の条件） | 2026-09-17〜10-10 / 0.11.9〜0.12.0 | October_Plan.md に引き継ぎ |
| [Issue65_Fault_Test_Design.md](Issue65_Fault_Test_Design.md) | #65（再接続で config.toml の設定を使う）の故障試験の設計 | 2026-09-24 / 0.12.0 | #65 は 0.12.0 でクローズ |
| [handoff_to_2026-10-22.md](handoff_to_2026-10-22.md) | 2026-10-22 までの引き継ぎ資料（日付ごとの更新の積み重ね） | 2026-04-17〜10-22 / 〜0.13.0 | 2026-10-23 に handoff.md を作り直した |
| [Test_Results_to_2026-10-24.md](Test_Results_to_2026-10-24.md) | 試験の結果の記録（Test_Procedure.md の末尾から移した。0.12.0・フェーズ 1〜3・0.13.0） | 2026-10-10〜10-24 / 0.12.0〜0.13.1 | 0.13.1 の結果からは Test_Procedure.md に書く |
| [Log_Redesign_Plan.md](Log_Redesign_Plan.md) | ログの見直し（#72〜#76。段 1〜5） | 2026-10-10〜10-24 / 0.12.0〜0.13.1 | 段 1 は 0.12.0、段 2〜5 は 0.13.1（フェーズ 3）で実装 |
