# rakukan v0.11.6

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

v0.11.6 は候補の順位を見直したリリースです。順位を「学習履歴 → ユーザー辞書 → システム辞書 → LLM」にし、ユーザー辞書に登録した表記を別の候補の選択で上書きできるようにしました（Issue #13）。記号・絵文字は通常語の後ろに mozc の優先順で並び、「みぎ」で「右」が矢印記号に埋もれる問題を直しました（Issue #42。辞書の再生成が要ります。インストーラーは再生成済みの辞書を同梱し、`install.ps1` は古い辞書を自動で作り直します）。候補から読みそのものを明示的に選んだ確定も学習するようになりました。ひらがな入力中に打った英単語が後ろの日本語ごと壊れる問題（Issue #38、PR #44）と、辞書候補が多い読みで LLM 候補が入らない問題、エンジンホストの再起動直後に Space が固まる問題も修正しています。

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

[Releases](https://github.com/fukuyori/rakukan/releases) の `rakukan-<version>-setup.exe` を実行します。管理者権限（UAC）が求められます。インストール先は `%LOCALAPPDATA%\rakukan\` で、終了時に言語リストへ rakukan が追加されます。

**すでに旧版が入っている場合** は、DLL を掴んでいるプロセスを無くしてから実行してください:

1. 言語バーで rakukan 以外の IME（例: Microsoft IME）に切り替える
2. サインアウトする
3. サインインする
4. インストーラーを実行する

インストーラーは旧 DLL が使用中かを確認し、使用中なら上の手順を案内します。上書きに失敗した場合は前のバージョンに戻したうえで同じ手順を表示します。インストール後に言語バーへ表示されない場合は、一度サインアウトして再度サインインしてください。PC の再起動は不要です。

## ビルド前提

ソースからビルドする場合に必要なもの。`cargo make check-env` で一括確認でき、不足分は `cargo make setup-env`（winget 使用、管理者推奨）で導入できます。

| 必須 | 用途 |
|---|---|
| Rust 1.85 以上（`x86_64-pc-windows-msvc`）と cargo-make | 全クレートのビルド、`Makefile.toml` の実行 |
| Visual Studio 2022 Build Tools（C++ ツール、CMake / Ninja、MSBuild） | llama.cpp のビルド、設定アプリのビルド |
| LLVM（libclang） | `llama-cpp-sys-2` の bindgen |
| .NET SDK 8 以上 | 設定アプリ `apps/rakukan-settings-winui`（Windows App SDK は NuGet で自動取得） |
| Git | ソース取得 |

| 任意 | 用途 |
|---|---|
| CUDA Toolkit（nvcc） | `rakukan_engine_cuda.dll` を作る場合。無ければ cuda variant はスキップ |
| Vulkan SDK（環境変数 `VULKAN_SDK`） | `rakukan_engine_vulkan.dll` を作る場合。無ければ vulkan variant はスキップ |
| Inno Setup 6、Windows SDK の signtool | 配布パッケージ作成と署名（`scripts/build-installer.ps1`、`cargo make sign`） |

## インストール（ソースから）

ビルド → 署名 → インストールを **4 ステップ** に分離しています。**DLL を掴んでいるプロセスがあると ④ が失敗する** ため、ビルドを済ませてからサインアウト→サインインし、IME を使う前に ④ を実行します:

```powershell
# ① engine DLL をビルド (cpu/vulkan/cuda)
cargo make build-engine

# ② tsf + tray + host + dict-builder + WinUI settings をビルド
cargo make build-tsf

# ③ 電子署名 (任意; 配布用)
cargo make sign

# --- ここでサインアウト → サインイン (TSF DLL を全プロセスから解放する) ---

# ④ %LOCALAPPDATA%\rakukan\ にコピー + TSF 登録 + tray 起動 (★管理者権限)
cargo make install
```

手順の順序は **ビルド → サインアウト → サインイン → install** です。`cargo make install` はビルドを行わないので、ビルドを飛ばすと古い成果物が再インストールされるだけになります。サインアウトせずに ④ を実行すると、DLL のコピーで「ファイルがロックされています」と表示されて中断することがあります（直前に停止したプロセスの解放待ちによる失敗は、スクリプト内で数回リトライして吸収します）。

まとめ実行:

```powershell
# ①〜④ を一括 (リリース向け)
cargo make full-install

# 開発時の高速再インストール (engine 使いまわし、署名なし)
cargo make quick-install
```

まとめ実行はビルドと install の間にサインアウトを挟めないため、DLL がどこにもロードされていない状態（初回インストール、またはサインイン直後に IME を使う前）でのみ通ります。通常の更新では上の分割手順を使ってください。

インストール先: `%LOCALAPPDATA%\rakukan\`  
設定: `%APPDATA%\rakukan\config.toml`  
ログ:

- TSF 側: `%LOCALAPPDATA%\rakukan\rakukan.log`
- エンジンホスト側: `%LOCALAPPDATA%\rakukan\rakukan-engine-host.log`（起動時に host / engine DLL の version・git sha を記録し、別ビルドの組み合わせなら WARN）
- エンジン DLL 側: `%LOCALAPPDATA%\rakukan\rakukan-engine-dll.log`（辞書ロード失敗 `dict load failed at [...]` や LLM 変換の警告はこちらに出る）

> 各ステップはそれぞれ独立に実行できます。ビルド (`build-engine` / `build-tsf`) は管理者不要、`install` のみ管理者権限が必要です。

## 設定の目安

`%APPDATA%\rakukan\config.toml` では `model_variant` と `n_gpu_layers` を調整できます。

- `jinen-v1-xsmall-q5` は比較的軽く、`n_gpu_layers = 16` 前後から試しやすい
- `jinen-v1-small-q5` は `n_gpu_layers = 8` か `16` くらいから始めるのが安全
- `jinen-v2-xsmall-q5` / `jinen-v2-small-q5` は v2 世代（Qwen3 ベース）。プロンプト形式は v1 と共通で、`model_variant` を書き換えるだけで切り替えられる
- `n_gpu_layers = 0` は CPU のみ
- 未指定は全レイヤー GPU オフロード
- `gpu_backend = "auto"`（既定）は cuda → vulkan → cpu の順に実際にロードを試みる。`cuda` / `vulkan` / `cpu` を明示した場合はその DLL だけを使い、失敗しても他へ切り替えない（結果は `rakukan-engine-host.log` に出る）

`n_gpu_layers` と `model_variant` は config.toml を編集したあと IME をオン/オフするだけで即時反映されます（`rakukan-engine-host.exe` 内部の DynEngine が新設定で作り直されます）。

`[input]` セクションでは起動時の IME 状態を指定できます。

- `default_mode = "off"`（既定）は直接入力で開始、`"on"` はかな漢字変換で開始。旧値 `"alphanumeric"` / `"hiragana"` も同じ意味で受け付けます
- `remember_last_kana_mode = true`（既定）はアプリ（ウィンドウ）ごとに前回の IME オン/オフを記憶して復元します。ターミナル系アプリは設定に関わらず IME オフで開始します

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

- TSF 層だけの変更確認: `cargo make build-tsf` → サインアウト → サインイン → `cargo make install`
- engine DLL を含む変更確認: `cargo make build-engine` → `cargo make build-tsf` → サインアウト → サインイン → `cargo make install`
- 生成ログ確認:

```powershell
Get-Content "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 40
```

## 課題リスト

### 主要設計書

- [DESIGN.md](docs/DESIGN.md) — v0.4.4 時点の全体設計書（クレート構成・RPC プロトコル・スレッドモデル・辞書システムなど）
- [handoff.md](docs/handoff.md) — v0.9.3 引き継ぎ資料 + 残タスクリスト

### 独立した技術課題

- [ ] `rakukan-engine-host.exe` の idle 自死（長時間アイドル時のメモリ解放）
- [ ] ホストプロセスのヘルスチェックとクラッシュカウント
- [ ] Preedit / LiveConv / Selecting の display_attr 拡張

### 過去の設計書・計画書

役目を終えた設計書・計画書は [docs/archive/](docs/archive/README.md) に移してあります。どの作業で使用したかは同ディレクトリの README.md にまとめています。現在進行中のタスクではありません。

## ライセンス

rakukan 本体のコードは **MIT ライセンス** です。  
辞書・モデルなどの同梱物や取得物には、それぞれ個別のライセンス条件が適用されます。
