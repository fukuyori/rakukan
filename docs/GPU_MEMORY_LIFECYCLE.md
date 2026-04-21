# GPU メモリのライフサイクル

最終更新: 2026-04-21

## 結論

**rakukan の engine-host インスタンスが複数（例: TSF DLL 多重 spawn で 5 個）起動していても、GPU メモリ圧迫にはならない。**

「engine-host 多重起動 → GPU メモリ浪費」という主張は誤りなので、議論時の前提として記録しておく。

## GPU メモリのライフサイクル

| タイミング | 対象 | 場所 |
|---|---|---|
| エンジン起動時（eager） | モデル weights（`n_gpu_layers` 分） | [`crates/rakukan-engine/src/kanji/llamacpp.rs:99`](../crates/rakukan-engine/src/kanji/llamacpp.rs#L99) `LlamaModel::load_from_file` |
| 変換実行時（lazy） | **KV cache（推論コンテキスト）** | [`crates/rakukan-engine/src/kanji/llamacpp.rs:353-355`](../crates/rakukan-engine/src/kanji/llamacpp.rs#L353-L355) `model.new_context()` を `convert()` 内で呼ぶ |
| 変換完了後 | context が drop → KV cache 解放 | スコープ終了で自動 |

## 数値感覚（xsmall モデル + デフォルト設定）

- モデル weights: ~30 MB（`jinen-v1-xsmall-q5`）
- `n_gpu_layers = 16` で部分オフロード
- **起動時の GPU 占有: 数十 MB 程度**
- KV cache: **変換時のみ短時間確保**、終わったら即解放

5 インスタンス × 数十 MB ≒ 100-200 MB 程度。現代の GPU では誤差。

## 多重起動の実際の影響

GPU は問題ないが、CPU 側には以下の影響がある:

- **RAM**: 各 engine-host が weights を独自に保持（~30 MB × 5 ≒ 150 MB）
- **ディスク I/O**: 起動時に cuda DLL（~150 MB）等を 5 重ロード
- **CPU**: pipe は単一クライアントしか繋がらないので 4 インスタンスは事実上アイドル待ち

これらは「無駄ではあるがクリティカルではない」レベル。Explorer crash 等の優先課題があるならそちらを先に対処すべき。

## 議論時のルール

engine-host の多重 spawn / インスタンス数について議論する時、**「GPU メモリを圧迫する」という主張はしない**。

問題視する理由は CPU/RAM 側（weights の RAM 重複、起動時 I/O、アイドル pipe instance）に限定する。

## 経緯

- 2026-04-21: install.ps1 のロックチェックで 5 個の engine-host PID が検出された件で、Claude が「GPU メモリ浪費」と指摘 → ユーザーから「GPU は変換時のみ使用なので圧迫しない」と訂正
- 実装確認後に Claude が誤りを認めて撤回
