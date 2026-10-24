# rakukan v0.14.0

> ⚠️ **注意：現在テスト動作中です**
>
> rakukan は開発途中のソフトウェアです。インストールによって **Windows の動作が不安定になる可能性があります**。
> ライブ変換は、非常にクセのある動きが見られ、現在まだバグが残っているので使用には我慢が必要になります。
> TSF（Text Services Framework）DLL をシステムに登録するため、インストール・アンインストールの操作は
> **自己責任** で行ってください。重要な作業環境への適用は推奨しません。

Windows 向け日本語 IME。  
[karukan](https://github.com/togatoga/karukan) の LLM ベース変換エンジンを中核とし、
[azooKey-Windows](https://github.com/fkunn1326/azooKey-Windows) の TSF 層実装を参考に構築しています。

rakukan は、ローカルで動く小型 LLM と Mozc 系辞書を組み合わせ、従来のかな漢字変換とは少し違う候補の出し方を試すための実験的な IME です。入力中の読みから候補を先読みするライブ変換、数字やアルファベットを壊さない literal 保護、ユーザー辞書・学習履歴による候補の優先順位調整を中心にしています。

設計上の大きな特徴は、TSF DLL と変換エンジンを別プロセスに分けていることです。Windows の入力フレームワーク側には軽いクライアントだけを置き、LLM や GPU バックエンドは `rakukan-engine-host.exe` 側で管理します。これにより、CPU / Vulkan / CUDA の engine DLL を設定で切り替えながら、IME 側の安定性をできるだけ保つ構成にしています。

現時点では、日常利用向けの完成品というより、LLM 変換・ライブ変換・Windows TSF 実装を実機で検証するためのプロトタイプです。挙動を観察しながら改善していく前提で使ってください。

## 主な機能

- **ライブ変換**: ひらがな入力後、短い停止でトップ候補を自動表示
- **範囲指定変換**: `Shift+Right/Left` で先頭から変換範囲を指定 → `Space` で変換 → `Enter` で確定、残りで LiveConv 再開
- **区読点分割変換**: `、` `。` や全角記号 `（）～` 、和文記号 `「」・` など記号を含む読みを入力すると記号位置でブロックへ自動分割。`Space` で各ブロックを変換、`Left` / `Right` でブロックを移動 → `Enter` で全ブロックをまとめて確定
- **数値保護**: LLM が数字を改変しない（`2024ねん → 2024年`）。数字・アルファベットは半角/全角の両方を候補として提示
- **LLM + 辞書変換**: jinen モデルと Mozc 系辞書を併用
- **ユーザー辞書学習**: 確定した変換結果を即時反映
- **文字種変換**: `F6`〜`F10` でひらがな・カタカナ・英数を往復
- **GPU アクセラレーション**: CUDA / Vulkan バックエンド対応。CUDA 版 DLL は CUDA ランタイムが別途必要（無い環境では `gpu_backend = "auto"` が Vulkan / CPU へ自動で切り替わる）
- **out-of-process 構成**: TSF DLL と engine-host を分離し、GPU リソースや LLM 実行をホストプロセス側で管理

## 最新の変更

v0.14.0 は、設定アプリで変えた設定（モデル・GPU の backend・ログの水準を含む）が、エンジンのホストが動いている間も次の変換から反映されるようにしたリリースです（[Issue #49](https://github.com/fukuyori/rakukan/issues/49) / [#64](https://github.com/fukuyori/rakukan/issues/64) / [#66](https://github.com/fukuyori/rakukan/issues/66)）。設定が変わると、ホストは自分の中でエンジンを作り直さずに起動し直します（ほかのアプリの接続もいったん切れ、入力中の文字は戻されます）。ホストは、送られた設定が config.toml の今の内容から作られているかを確かめてから採用するので、古い設定を持ったアプリの接続で設定が戻りません。config.toml を読めない間は、最後に採用した設定で動きます。既定のモデルを `jinen-v1-xsmall-q5` にそろえ、設定アプリのモデルの選択肢を models.toml から作るようにしました（Issue #82）。IME をオンにした直後の表示が「ー」になる問題も直しました。ホストとの通信の版（プロトコル）が 8 に上がったので、更新した後は PC を再起動するまで日本語入力が使えないことがあります。

- v0.13.1: **インストール・更新・アンインストールを、一般的な Windows のアプリと同じ手順にした**（Issue #33）。インストール先を `%ProgramFiles%\rakukan` にし、インストーラーの実行の後の PC の再起動で完了する（事前の IME の切り替えやサインアウトは要らない）。0.13.0 までの配置からの更新では旧配置を片付ける。モデルは `%ProgramData%\rakukan\models` に置く。ログの見直し（Issue #73〜#76）、トレイの配布、使用許諾の画面と第三者のライセンスの文書の同梱。

- v0.13.0: **エンジンホストとの通信が途切れたときやエンジンの準備中に、打ったキーと確定する文字を失わない**（Issue #69 / #70）。準備中・応答待ちの間のキーは保留して打った順に処理し、ホストが戻らないときは確定して直接入力に切り替えて知らせる。確定は文書への書き込みを先に済ませる。フォーカス移動で LLM の文脈が消えない問題（Issue #71）、打っただけの読みで範囲選択が効かない問題、メモ帳で言語バーのメニューが出ない問題を修正。テンキーの `/` `.` は「／」「．」。engine ABI 12、通信のプロトコル v7。

- v0.12.0: **エンジンホストが入れ替わっても未確定の文字を失わない**（Issue #56、nick20002005 さんの PR #67 / PR #68）。ホストへの再接続でその時点の `config.toml` を使う（Issue #65）。Space → Backspace → Space で表示されていない候補が確定される問題、F9 / F10 で大文字に変わる問題を修正。エンジン DLL のログを約 1 か月分残す。

- v0.11.9: **ログ・設定・IME 状態の扱いを修正**。アプリごとの IME 初期状態を `config.toml` の `[input] ime_off_apps` / `ime_on_apps` で設定（Issue #51 の段 1）。設定の変更をすべてのアプリに反映（Issue #65）。TSF のログをプロセスごとのファイルに分け（Issue #60）、ホストとの通信も記録（Issue #54）。閉じた入力先の IME 状態の誤復元（Issue #50）、`config.toml` が一時的に読めないと既定値に戻る問題（Issue #61）、起動の繰り返し（Issue #55）、変換の詰まりの誤判定（Issue #57）、「参」「拾」の誤判定（Issue #53）を修正。MOZC 辞書の取得元を SHA で固定（Issue #62）。

- v0.11.7: **エンジンの異常からの復帰を中心に修正**。エンジンホストが入れ替わったときに辞書・ユーザー辞書・学習履歴が黙って無効になる問題、推論失敗で「⏳ 変換中...」のまま止まる問題（Issue #43）を修正し、連続して失敗した場合はエンジンが自分で再起動して復帰。モデル読み込み中の変換は読み込み後に自動でやり直す（Issue #39）。候補ウィンドウがタスクバーと Alt+Tab に出る問題（PR #48）、エンジン DLL のログレベルが `config.toml` に追随しない問題も修正。

- v0.11.6: **候補の順位を「学習履歴 → ユーザー辞書 → システム辞書 → LLM」にした**（Issue #13）。記号・絵文字を通常語の後ろに mozc の優先順で並べ、「みぎ」で「右」が矢印記号に埋もれる問題を修正（Issue #42。辞書の再生成が要る）。読みそのものを明示的に選んだ確定も学習。ひらがな入力中の英単語が後ろの日本語ごと壊れる問題（Issue #38、PR #44）、辞書候補が多い読みで LLM 候補が入らない問題、ホスト再起動直後に Space が固まる問題も修正。

- v0.11.5: **ローマ字入力の状態を打鍵ログから再構築**（Issue #34）。Backspace 後のローマ字の結合、記号・Shift+英字・数字の追い越し、F9 / F10 → F6 の復元、「きゃ」の削除後の F9、Space 変換の末尾ローマ字を修正。推論失敗時に Space が固まる問題も修正（PR #41 の一部）。

- v0.11.4: **入力状態を「IME オン / IME オフ」の 2 値に統一**。入力モード 3 値（ひらがな / カタカナ / 英数）を廃止し、カタカナは `F7` / 無変換キーで入力。IME 切替経路を 1 か所に集約し、メニューで英数にした後の表示ずれを修正。修飾キーの左右指定、設定アプリの「主要ショートカット」再構成とキー名ヘルプ、単一インスタンス化を追加。keymap の旧アクション名と `default_mode` の旧値は引き続き読み込める。

- v0.11.3: **候補ウィンドウのフォントサイズ変更に対応**（Issue #3 / PR #5）。`config.toml` の `[appearance]` `candidate_font_height` または設定アプリの「候補表示」から変更可能。設定の保存が一部のアプリの IME に反映されないことがある問題も修正。

- v0.11.2: **数字とかなが混在する読みの変換品質を修正**。「5まん」が「5満」「5マン」になり「5万」が候補に出ない問題（Issue #6 / PR #7）を修正。

- v0.11.1: **CI 崩壊の解消（メンテナンス、挙動変更なし）**。ツールチェーンの更新（rustfmt 1.9.0 / 新しい clippy）で main の CI が落ち、すべての PR のチェックが赤くなっていた問題（Issue #14）を修正。

- v0.11.0: **8月の運用ログと GitHub Issue にもとづく修正のまとめ**。Space 変換でユーザー辞書・学習履歴が反映されない問題（Issue #9）、JIS 配列の半角/全角キー（Issue #1）、`gpu_backend = "auto"` の Vulkan / CPU への自動切替（Issue #2）、変換中の Home / End の素通し（Issue #11）、ライブ変換プレビューのかな表示への巻き戻り、変換済みカタカナ語を含む文の文脈破棄、モード切替時の keymap 同期再読込によるキーストールを修正。host / engine DLL の別ビルド検出ログ（Issue #8）を追加。

- v0.10.4: **語彙外文字（Ψ・€・絵文字など）が変換候補から消える問題を修正**。jinen v2 のバイトフォールバックトークンがデコード時に破棄されていたもので、「さいきくすおのさいなん」→「斉木楠雄のΨ難」が正しく出るようになった。
- v0.10.3: **jinen-v2 モデル（Qwen3 ベース）を追加**。`config.toml` の `model_variant` を `jinen-v2-xsmall-q5`（約 28 MB）/ `jinen-v2-small-q5`（約 81 MB）などに書き換えるだけで切り替え可能（f16 variant もあり。デフォルトは v1 のまま）。
- v0.10.2: **確定テキスト消失を修正**（TS_E_READONLY 時の再試行と表示中テキストのままの確定）、**無駄なバックグラウンド変換を削減**（末尾が未確定ローマ字の間はライブ変換を起動しない）。
- v0.10.1: **echo strip（context 汚染対策）の誤爆を削減**。エコー源判定を「8 文字以上のかな連続 run」に絞り、除去も該当文のみに限定。ひらがなのみの確定文は最初から文脈に入れない。
- v0.10.0: **エンジン（LLM モデル）の二重ロード乱発を修正**（7月の運用ログで月 800 回発生）。

過去の変更履歴は [CHANGELOG.md](CHANGELOG.md) を参照してください。

## インストール（パッケージから）

[Releases](https://github.com/fukuyori/rakukan/releases) の `rakukan-<version>-setup.exe` を実行します。管理者権限（UAC）が求められます。インストール先は `%ProgramFiles%\rakukan\` で、インストールの終わりに言語リストへ rakukan が追加されます。

インストールを完了するには、**PC を再起動** してください。旧版が入っている場合も、事前の準備（IME の切り替えやサインアウト）は要りません。使用中のファイルは再起動のときに置き換わります。再起動するまでは、Rakukan による日本語入力ができないことがあります。

`%LOCALAPPDATA%\rakukan\` に入っていた旧版（0.13.0 までの配置）は、インストーラーが登録を解除し、旧版のファイルを消します（使用中のものは再起動のときに消えます）。設定・ログは残ります。

## アンインストール

「設定」→「アプリ」から Rakukan IME をアンインストールし、PC を再起動してください。使用中のファイルは再起動のときに消えます。

設定・ユーザー辞書・学習履歴（`%APPDATA%\rakukan\`）、ログ（`%LOCALAPPDATA%\rakukan\` の `*.log`）、モデル（`%ProgramData%\rakukan\models\`）はアンインストールしても残ります。不要なら手で削除してください。

## ビルド前提

ソースからビルドする場合に必要なもの。`cargo make check-env` で一括確認でき、不足分は `cargo make setup-env`（winget 使用、管理者推奨）で導入できます。

| 必須 | 用途 |
|---|---|
| Rust 1.85 以上（`x86_64-pc-windows-msvc`）と cargo-make | 全クレートのビルド、`Makefile.toml` の実行 |
| Visual Studio 2022 Build Tools（C++ ツール、CMake / Ninja、MSBuild） | llama.cpp のビルド、設定アプリのビルド |
| LLVM（libclang） | `llama-cpp-sys-2` の bindgen |
| .NET SDK 8 以上 | 設定アプリ `apps/rakukan-settings-winui`（Windows App SDK は NuGet で自動取得） |
| Git | ソース取得 |
| Inno Setup 6 | インストーラーの作成（`cargo make install` / `cargo make package`） |

| 任意 | 用途 |
|---|---|
| CUDA Toolkit（nvcc） | `rakukan_engine_cuda.dll` を作る場合。無ければ cuda variant はスキップ |
| Vulkan SDK（環境変数 `VULKAN_SDK`） | `rakukan_engine_vulkan.dll` を作る場合。無ければ vulkan variant はスキップ |
| Windows SDK の signtool | 電子署名を行う場合（`cargo make package`） |

## インストール（ソースから）

ソースからのインストールも、パッケージと同じインストーラーを使います。`cargo make install` はビルドの出力からインストーラーを署名なしで作り、サイレント実行します:

```powershell
# ① engine DLL をビルド (cpu/vulkan/cuda)
cargo make build-engine

# ② tsf + tray + host + dict-builder + WinUI settings をビルド
cargo make build-tsf

# ③ インストーラーを作ってサイレント実行 (★管理者権限)
sudo cargo make install

# ④ PC を再起動 (インストールの完了)
```

`cargo make install` はビルドを行わないので、ビルドを飛ばすと古い成果物が再インストールされるだけになります。事前のサインアウトは要りません。使用中のファイルは再起動のときに置き換わるので、再起動するまでは日本語入力ができないことがあります。辞書は初回に mozc の辞書を取得して作り（`C:\rb\dict\`）、取得元の版が変わるまで作り直しません。セットアップのログは実行ごとに `output\logs\` に残ります。

まとめ実行（開発用、署名なし）:

```powershell
# ①〜③ を一括
cargo make full-install

# engine を使いまわして ②〜③
cargo make quick-install
```

電子署名を行える環境では、`cargo make package` で、成果物・セットアップ本体・アンインストーラーに署名したインストーラー（`output\rakukan-<version>-setup.exe`）を作れます（環境変数 `CODESIGN_CERT` に証明書の Subject CN を設定しておく）。

アンインストール: `sudo cargo make uninstall`（アンインストーラーをサイレント実行。完了には再起動）。

インストール先: `%ProgramFiles%\rakukan\`  
設定: `%APPDATA%\rakukan\config.toml`  
ログ:

- TSF 側: `%LOCALAPPDATA%\rakukan\rakukan-tsf-<PID>-<起動識別子>.log`（アプリごとのプロセス単位。統合は `scripts/merge-logs.ps1`）
- エンジンホスト側: `%LOCALAPPDATA%\rakukan\rakukan-engine-host-<PID>-<起動識別子>.log`（起動時に host / engine DLL の version・git sha を記録し、別ビルドの組み合わせなら WARN）。エンジン DLL のログ（辞書ロード失敗 `dict load failed at [...]` や LLM 変換の警告。target `rakukan_engine_dll`）もここに入る

各ファイルの 1 行目は発生元を示すヘッダー（実行ファイル・PID・ビルドなど）。書き込み時に 16 MiB で退避し、5 世代を残す。

> 各ステップはそれぞれ独立に実行できます。ビルド (`build-engine` / `build-tsf`) と `package` は管理者不要（署名は昇格しないセッションで行う）、`install` / `uninstall` は管理者権限が必要です。

## 設定の目安

`%APPDATA%\rakukan\config.toml`（設定アプリの各ページでも変えられます）では `model_variant` と `n_gpu_layers` を調整できます。

- 使えるモデルは設定アプリの「モデル ID」で選べます（一覧は `crates/rakukan-engine/models.toml`）。既定は `jinen-v1-xsmall-q5`
- `jinen-v1-xsmall-q5` は比較的軽く、`n_gpu_layers = 16` 前後から試しやすい
- `jinen-v1-small-q5` は `n_gpu_layers = 8` か `16` くらいから始めるのが安全
- `jinen-v2-xsmall-q5` / `jinen-v2-small-q5` は v2 世代（Qwen3 ベース）。プロンプト形式は v1 と共通で、`model_variant` を書き換えるだけで切り替えられる
- `n_gpu_layers = 0` は CPU のみ
- 未指定は全レイヤー GPU オフロード
- `gpu_backend = "auto"`（既定）は cuda → vulkan → cpu の順に実際にロードを試みる。`cuda` / `vulkan` / `cpu` を明示した場合はその DLL だけを使い、失敗しても他へ切り替えない（結果はエンジンホストのログに出る）

エンジンにかかわる設定（`model_variant`・`gpu_backend`・`n_gpu_layers`・候補数・`log_level` など）を変えると、`rakukan-engine-host.exe` が起動し直して新しい設定で動きます。設定アプリで保存したときは、次の変換から反映されます。config.toml を直接編集したときは、変更はフォルダの変更通知と 30 秒ごとの定期確認で読み込まれ、次の IME の切り替え（`reload_on_mode_switch = true` のとき）で反映されます。すぐに反映したい場合は言語バーの「エンジン再起動」を使います。config.toml を読めない（書き間違いなど）間は、直前の正しい設定のまま動きます。

`[input]` セクションでは起動時の IME 状態を指定できます。

- `default_mode = "off"`（既定）は直接入力で開始、`"on"` はかな漢字変換で開始。旧値 `"alphanumeric"` / `"hiragana"` も同じ意味で受け付けます
- `remember_last_kana_mode = true`（既定）はアプリ（ウィンドウ）ごとに前回の IME オン/オフを記憶して復元します
- `ime_off_apps` に挙げた exe は、アクティブになるたび IME オフで始まります（既定はターミナル 5 種）。`ime_on_apps` に挙げた exe は、アプリ本体とは別の入力先に入ったとき IME をオンにします。どちらも操作中に IME を変えればその状態が続き、インアクティブになると捨てられます（次にアクティブにすると設定値から始まります）

  ```toml
  [input]
  ime_off_apps = ["conhost.exe", "WindowsTerminal.exe", "mintty.exe", "wezterm-gui.exe", "ghostty.exe"]
  ime_on_apps = ["msedge.exe", "chrome.exe", "firefox.exe"]
  ```

> v0.4.4 より、Zoom / Dropbox 等の他アプリが異常終了する問題は別プロセス化で解消済みです。`n_gpu_layers` を下げる回避策は不要になりました。

## キー操作

rakukan の入力状態は **IME オン**（かな漢字変換）と **IME オフ**（直接入力）の 2 つだけです。「ひらがなモード」「カタカナモード」「英数モード」といった独立したモードはありません。カタカナは `F7` や無変換キーで変換して入力します。言語バーのアイコン（「あ」/「A」）をクリックするとメニューから切り替えられます。

| キー | 動作 |
| ---- | ---- |
| 半角/全角 / Ctrl+Space | IME オン/オフの切替 |
| ひらがな / Ctrl+Caps（US 配列: Ctrl+J） | IME オン |
| 英数（US 配列: Ctrl+L） | IME オフ |
| カタカナ / Alt+Caps（US 配列: Ctrl+K） | 入力中の読みをカタカナに変換（F7 と同じ） |
| 無変換 | ひらがな → カタカナ → 半角カタカナ の循環 |
| Space / 変換 | 変換開始 / 次候補 / 選択中分節の再変換 |
| Enter | 表示中の内容を確定（区読点分割変換中は全ブロックをまとめて確定） |
| ESC | 変換キャンセル |
| Backspace | 1文字削除 |
| Left / Right | 区読点分割変換中は選択ブロックの移動（それ以外の変換中は IME が受け取り、アプリ側のキャレットを動かさない） |
| Home / End | 変換中は IME が受け取り、アプリ側のキャレットを動かさない（範囲指定中は選択範囲を先頭 / 末尾へ） |
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

> **区読点分割変換について**: 読みに `、` `。` `！` `？` などの区読点・記号（全角記号 `（）～` / ASCII 記号 `@#()` / 和文記号 `「」・` など）が含まれると自動的にブロック分割変換へ移行します。Space でブロック内の候補を選択し、Left / Right でブロックを移動します。Enter で全ブロックをまとめて確定し、そのときに学習が行われます。

### キー割り当ての変更

キー割り当ては設定アプリ（言語バーのメニュー「設定...」）の「キー設定」ページ、または `%APPDATA%\rakukan\keymap.toml` で変更できます。

- 設定アプリの「主要ショートカット」では IME 切替 / IME ON / IME OFF / 変換開始 / ひらがな確定 / 取消 / 全取消 の 7 項目をキー名で指定します。「キー名の一覧と注意」ボタンで使えるキー名と注意事項を確認できます
- 1 つのアクションに複数のキーを割り当てたい場合や、上記以外のアクションは `keymap.toml` の `[[bindings]]` に `key` と `action` を書きます。IME 制御のアクション名は `ime_toggle` / `ime_on` / `ime_off` です。旧名 `mode_hiragana` / `mode_alphanumeric` は `ime_on` / `ime_off` として、`mode_katakana` は `katakana`（F7 変換）として読み込まれます
- 修飾キーは `Ctrl+` / `Shift+` / `Alt+`（左右どちらでも一致）のほか、`LCtrl+` / `RCtrl+` / `LShift+` / `RShift+` / `LAlt+` / `RAlt+` で左右を区別できます。左右指定と汎用指定の両方があるときは左右指定が優先されます
- 数字キー（候補選択に使用）、Win キーを含む組み合わせ、修飾キー単独は割り当てできません。Alt+Tab のように Windows が先に処理するキーや、ブラウザの Ctrl+L のようにアプリが IME より先に処理するキーは、設定しても効きません

## 開発メモ

- TSF 層だけの変更確認: `cargo make build-tsf` → `sudo cargo make install` → 再起動
- engine DLL を含む変更確認: `cargo make build-engine` → `cargo make build-tsf` → `sudo cargo make install` → 再起動
- 生成ログ確認:

```powershell
.\scripts\merge-logs.ps1 | Select-Object -Last 40
```

## 課題リスト

### 主要設計書

- [DESIGN.md](docs/DESIGN.md) — 全体設計書（クレート構成・RPC プロトコル・スレッドモデル・辞書システムなど）
- [handoff.md](docs/handoff.md) — 引き継ぎ資料（現在の位置・次の開始点・作業の決まり・文書の地図）

### 独立した技術課題

- [ ] `rakukan-engine-host.exe` の idle 自死（長時間アイドル時のメモリ解放）
- [x] 推論失敗が続いたときのホストの自己終了と自動復帰（0.11.7、[Issue #43](https://github.com/fukuyori/rakukan/issues/43)）
- [ ] ホストが異常終了を繰り返したときの検知と抑止（接続直後の失敗が続いたときに再起動を一時停止する仕組みはあるが、起動に成功した後の異常終了は数えていない）
- [ ] Preedit / LiveConv / Selecting の display_attr 拡張

### 過去の設計書・計画書

役目を終えた設計書・計画書は [docs/archive/](docs/archive/README.md) に移してあります。どの作業で使用したかは同ディレクトリの README.md にまとめています。現在進行中のタスクではありません。

## ライセンス

rakukan 本体のコードは **MIT ライセンス** です。  
辞書・モデルなどの同梱物や取得物には、それぞれ個別のライセンス条件が適用されます。
