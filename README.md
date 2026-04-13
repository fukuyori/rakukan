# rakukan v0.4.4

> ⚠️ **注意：現在テスト動作中です**
>
> rakukan は開発途中のソフトウェアです。インストールによって **Windows の動作が不安定になる可能性があります**。
> TSF（Text Services Framework）DLL をシステムに登録するため、インストール・アンインストールの操作は
> **自己責任** で行ってください。重要な作業環境への適用は推奨しません。

Windows 向け日本語 IME。  
[karukan](https://github.com/togatoga/karukan) の LLM ベース変換エンジンを中核とし、
[azooKey-Windows](https://github.com/fkunn1326/azooKey-Windows) の TSF 層実装を参考に構築しています。

## 主な機能

- **ライブ変換**: ひらがな入力後、短い停止でトップ候補を自動表示
- **分節編集**: `Space` 後に文節単位で再変換し、`Left/Right` で移動、`Shift+Left/Right` で範囲調整
- **LLM + 辞書変換**: jinen モデルと Mozc 系辞書を併用
- **ユーザー辞書学習**: 確定した変換結果を即時反映
- **文字種変換**: `F6`〜`F10` でひらがな・カタカナ・英数を往復
- **GPU アクセラレーション**: CUDA / Vulkan バックエンド対応
- **Vibrato 分節補助**: `system.dic` を同梱し、分節境界の初期推定に利用

## 最近の 0.4.4 変更点

- **エンジンを別プロセス化**: `rakukan_engine_*.dll`（llama.cpp 同梱）は専用の
  `rakukan-engine-host.exe` でのみロードし、TSF DLL 側は Named Pipe + postcard で
  RPC するクライアントになりました。これにより Zoom / Dropbox などで発生していた
  `msvcp140.dll` クロスロード起因のクラッシュを根本解決しています。
- Activate 時には engine DLL に一切触れず、最初の入力まで完全に遅延ロード
- IME モード切替時の `config.toml` 自動再反映を out-of-process でも動くように修正
  （`Request::Reload` でホスト側 DynEngine を新 config で作り直す）
- Named Pipe には明示的な DACL を設定（現在ログインユーザー + SYSTEM のみ）
- `rakukan-engine-cli` の既存ビルドエラーを修正

## インストール

```powershell
# 初回: esaxx-rs パッチのセットアップ
cargo fetch
.\scripts\setup-esaxx-patch.ps1

# engine に変更がある場合
cargo make build-engine

# TSF/engine をインストール先へ反映
cargo make reinstall
```

インストール先: `%LOCALAPPDATA%\rakukan\`  
設定: `%APPDATA%\rakukan\config.toml`  
ログ:

- TSF 側: `%LOCALAPPDATA%\rakukan\rakukan.log`
- エンジンホスト側: `%LOCALAPPDATA%\rakukan\rakukan-engine-host.log`

> `cargo make build-tsf` はビルドのみです。実機確認には `cargo make reinstall` を使ってください。

## 設定の目安

`%APPDATA%\rakukan\config.toml` では `model_variant` と `n_gpu_layers` を調整できます。

- `jinen-v1-xsmall-q5` は比較的軽く、`n_gpu_layers = 16` 前後から試しやすい
- `jinen-v1-small-q5` は `n_gpu_layers = 8` か `16` くらいから始めるのが安全
- `n_gpu_layers = 0` は CPU のみ
- 未指定は全レイヤー GPU オフロード

`n_gpu_layers` と `model_variant` は config.toml を編集したあと IME モードを切り替えるだけで即時反映されます（`rakukan-engine-host.exe` 内部の DynEngine が新設定で作り直されます）。

> v0.4.4 より、Zoom / Dropbox 等の他アプリが異常終了する問題は別プロセス化で解消済みです。`n_gpu_layers` を下げる回避策は不要になりました。

## キー操作

| キー | 動作 |
|------|------|
| Space / 変換 | 変換開始 / 次候補 / 選択中分節の再変換 |
| Enter | 表示中の内容を確定 |
| ESC | 変換キャンセル |
| Backspace | 1文字削除 |
| Left / Right | 分節選択の移動 |
| Shift+Left / Shift+Right | 分節選択の縮小 / 拡張 |
| ↑ / ↓ | 候補を前後に移動 |
| 1〜9 | 候補を番号で選択 |
| Tab / PageDown | 次ページ |
| Shift+Tab / PageUp | 前ページ |
| F6 | ひらがな |
| F7 | カタカナ |
| F8 | 半角カタカナ |
| F9 | 全角英数 |
| F10 | 半角英数 |

## 開発メモ

- TSF 層だけの変更確認: `cargo make build-tsf`
- engine DLL を含む変更確認: `cargo make build-engine` → `cargo make reinstall`
- 同梱 Vibrato 辞書: `assets/vibrato/system.dic`
- 生成ログ確認:

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 40
```

## ライセンス

rakukan 本体のコードは **MIT ライセンス** です。  
辞書・モデルなどの同梱物や取得物には、それぞれ個別のライセンス条件が適用されます。
