# クラッシュ / race 不具合 調査プロトコル

<!-- markdownlint-disable MD024 -->

最終更新: 2026-04-24  
位置づけ: Explorer / IME クライアントプロセスの `0xc0000005` crash、および TSF hot path の race / ハング / 文字消失を解析するときの定型手順。過去に実行して成果を上げたフローを標準化したもの。

関連資料:

- [EXPLORER_CRASH_HISTORY.md](EXPLORER_CRASH_HISTORY.md) — これまでの crash 対策の年表と学習
- [ROADMAP.md](ROADMAP.md) — M5 の開封条件
- [handoff.md](handoff.md) — 現在の状態と既知の問題

---

## 1. 調査の大原則

1. **症状の再現条件を最優先で絞り込む**: 発生する操作シーケンス・タイミング・環境（アプリ、入力速度、モード）を書き起こしてから着手する。憶測で修正しない
2. **WinDbg `!analyze -v` の `Failure.Bucket` を最初に見る**: call stack が一見不完全でも bucket 名だけで方向が決まることが多い（例: `<module>!Unloaded` → unload race）
3. **tracing ログを time-correlate する**: `rakukan.log` と `rakukan-engine-host.log` の両方を時刻で並べて race の順序を組み立てる
4. **仮説は 1 つずつ検証する**: 同時に複数の変更を入れると root cause が分からない。再現性が低い bug ほど「1 変更 → 実機テスト」で確かめる
5. **`0xc0000005` は unload race を最初に疑う**（0.6.6 の教訓）。TSF DLL / engine DLL / Windows API の順で当たる

---

## 2. クラッシュ dump 解析フロー

### 2.1 dump 取得設定（1 度だけ実行）

Explorer / IME クライアントプロセスでフルダンプが自動保存されるようにする:

```powershell
# 管理者 PowerShell で
$reg = 'HKLM:\SOFTWARE\Microsoft\Windows\Windows Error Reporting\LocalDumps'
New-Item -Path $reg -Force | Out-Null
Set-ItemProperty -Path $reg -Name 'DumpType' -Value 2 -Type DWord          # 2 = Full
Set-ItemProperty -Path $reg -Name 'DumpCount' -Value 10 -Type DWord
Set-ItemProperty -Path $reg -Name 'DumpFolder' -Value '%LOCALAPPDATA%\CrashDumps' -Type ExpandString
```

発生場所: `%LOCALAPPDATA%\CrashDumps\<exe>.<pid>.dmp`

### 2.2 WinDbg による解析

Windows SDK 付属の WinDbg (x64) を使う:

```powershell
& "C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\windbg.exe" "$env:LOCALAPPDATA\CrashDumps\explorer.exe.3124.dmp"
```

WinDbg コマンドプロンプトで:

```
.symfix
.reload
!analyze -v
```

必要に応じて追加:

```
lmv             # ロード済みモジュール一覧
k               # callstack
kp              # パラメータ付き callstack
!teb            # Thread Environment Block
!peb            # Process Environment Block
!address <addr> # 特定アドレスのマッピング確認
```

### 2.3 `!analyze -v` の読み方

重要フィールドを優先順位順で確認:

| フィールド | 意味 | 注目理由 |
|---|---|---|
| `Failure.Bucket` | 既知の不具合カテゴリ | `<module>!Unloaded` なら unload race。`BAD_INSTRUCTION_PTR_*` は命令ポインタ破壊 |
| `STACK_TEXT` | callstack | `<Unloaded_*>` があれば DLL が既に unmap されている |
| `FAULTING_IP` | 命令ポインタ | unmapped 領域なら unload race の確定証拠 |
| `EXCEPTION_CODE` | 例外コード | `c0000005` = AV、`c0000409` = security cookie 破壊 |
| `MODULE_NAME` | 関与モジュール | rakukan_tsf / rakukan_engine_* / msctf 等 |

### 2.4 過去事例の bucket → 対策の対応

| Failure.Bucket | 真因 | 対策バージョン |
|---|---|---|
| `BAD_INSTRUCTION_PTR_c0000005_rakukan_tsf.dll!Unloaded` | TSF DLL の unload race（unregister 漏れ window class への in-flight message） | 0.6.6 で `DllCanUnloadNow=S_FALSE` 固定 |
| `msvcp140.dll!*` のクラッシュ | VC ランタイム クロスロード | 0.4.4 で engine out-of-process 化 |
| `rakukan_engine_*.dll!*` で reload 周辺 | engine DLL drop → 新規 load の間の unmap race | 0.6.5 で BG スレッド撤去（M1.6 で host 再起動化予定） |

---

## 3. race / ハング / 文字消失の解析フロー

TSF hot path の不具合（文字が消える、候補が出ない、モードが戻る、など）は crash dump が残らないため、tracing ログが主な手掛かり。

### 3.1 ログ有効化

`%APPDATA%\rakukan\config.toml`:

```toml
[logging]
level = "debug"
# 必要なら個別モジュール
# level = "rakukan_tsf::tsf::candidate_window=trace,rakukan_tsf=debug"
```

ログ出力先:

- TSF 側: `%LOCALAPPDATA%\rakukan\rakukan.log`
- Host 側: `%LOCALAPPDATA%\rakukan\rakukan-engine-host.log`

### 3.2 ログの tail（再現中）

別窓で tail して再現:

```powershell
Get-Content -Wait -Path "$env:LOCALAPPDATA\rakukan\rakukan.log" -Tail 0
```

再現後は tail を止めてファイル全体を確認。

### 3.3 既知のログパターン

| ログパターン | 示すもの |
|---|---|
| `[Live] Phase1A: discarded stale SetText captured_gen=X current_gen=Y` | Phase1A race を検出（0.7.0 で追加） |
| `[Live] Phase1B: discarded stale preview entry_gen=X cur_gen=Y ...` | Phase1B キューの stale 検出（0.7.0 で追加） |
| `[Live] on_live_timer: preview discarded (too short) reading_len=X preview_len=Y` | 尻切れ防壁が効いた（0.7.0 で追加） |
| `doc_mode: retained mode=M for hwnd=H before removing dm=D` | DM 破棄前 HWND 退避が効いた（0.7.0 で追加） |
| `doc_mode: saved mode=M for dm=D hwnd=H` | focus-out 時のモード保存 |
| `doc_mode: restored mode=M from dm=D` / `from hwnd=H` | focus-in 時のモード復元 |
| `rpc: Reload requested, dropping current engine` | engine_reload 経路（v0.7.1 で host 再起動化予定） |
| `OnUninitDocumentMgr: removed dm=D` | DM 破棄 |
| `OnSetFocus(deferred): prev_dm=A next_dm=B hwnd=H` | フォーカス遷移 |

### 3.4 時系列組み立て

1. 症状発生時刻を特定（ユーザ記憶 / アプリ挙動）
2. `rakukan.log` を該当時刻 ±5 秒で抽出:

   ```powershell
   Select-String -Path "$env:LOCALAPPDATA\rakukan\rakukan.log" `
     -Pattern "2026-04-24 12:3[45]:"
   ```

3. 同時刻の `rakukan-engine-host.log` も突合
4. 「reading 変化」「preview 計算」「apply」「focus 変化」の 4 軸で時刻順テーブルを作る
5. 想定フローとズレている箇所を特定

---

## 4. 実機再現テストの手順

### 4.1 ビルド + インストール

```sh
cargo make build-tsf             # TSF のみ（engine 変更なしなら高速）
cargo make build-engine          # engine 本体。初回 / engine crate 変更後
sudo cargo make install
```

**サインアウト → 再ログオン**。TSF DLL は新 PID でないと再読み込みされない。

### 4.2 症状カテゴリ別チェックリスト

#### `0xc0000005` crash 系

1. WerFault フルダンプ設定（§2.1）を確認
2. `cargo make sign && cargo make install` → サインアウト → 再ログオン
3. 再現操作を 30 分以上継続（Explorer 主体ならアドレスバー / リネーム / フォルダ移動 / Alt+Tab）
4. `%LOCALAPPDATA%\CrashDumps\` に dump が出るか確認
5. 出たら §2.2 で解析

#### モード保持 / race 系

1. 再現手順を 3 回連続で実施（タイミング依存のため）
2. `rakukan.log` を確認し §3.3 のログパターンで経路を特定
3. 想定通りの discard / retained / saved ログが出ているか
4. 出ていない経路があれば、そこが race の穴

#### 文字消失 / 尻切れ系

1. `rakukan.log` で `reading={:?}` と `preview={:?}` の対応を追う
2. `Phase1A: discarded` / `Phase1B: discarded` / `preview discarded` のいずれが出ているか
3. どれも出ていないのに消える → 別経路の race、新規調査が要

---

## 5. 不具合切り分けのチェックリスト

症状を報告されたら、まず以下を聞く:

### 共通

- どのアプリで発生するか（Explorer / Chrome / VSCode / etc.）
- OS ビルド番号（`winver`）
- rakukan のバージョン（`%LOCALAPPDATA%\rakukan\rakukan.log` 先頭行）
- 再現率（毎回 / たまに / 1 回だけ）

### crash 系

- `%LOCALAPPDATA%\CrashDumps\` に dump があるか
- WerFault フルダンプ設定済みか（§2.1）
- 直前の操作（IME モード切替 / 設定保存 / アプリ起動直後 / etc.）

### モード系

- ブラウザなら Chrome / Edge / Firefox どれか
- タブ切替 / ページ遷移 / リンククリック / Alt+Tab のどれで発生するか
- `config.input.default_mode` の設定値
- `config.input.remember_last_kana_mode` の設定値

### 変換 / 文字消失系

- 再現する reading（できれば実例）
- 打鍵速度（普通 / 速打ち）
- `[live_conversion] debounce_ms` の設定値
- 消える位置（末尾 / 中間 / 全体）

---

## 6. ロードマップとの連携

新規に発見した crash root cause は以下の流れで処理する:

1. §2 で WinDbg 解析 → `Failure.Bucket` 確定
2. §3 でログと突合 → 再現条件と経路を特定
3. 既存 M1〜M5 でカバーされていれば該当マイルストーンに合流
4. 既存ではカバーされていなければ **新規マイルストーン**（例: M1.9 / M6）として ROADMAP に追加
5. 該当 auto-memory （`~/.claude/projects/.../memory/`）にも学習を記録

### 現在保留中の条件付き対策（M5）

実機で Explorer crash が再発した場合のみ開封:

- **M5.1** WM_TIMER → `PostMessage` 化（`WM_RAKUKAN_LIVE_READY` 導入）
- **M5.2** Explorer シェルクラスで Phase1A 無効化

詳細は [ROADMAP.md §8](ROADMAP.md) 参照。

---

## 7. よくある誤診 / アンチパターン

以下は過去に起きた勘違い。繰り返さないこと。

1. **race を「まれにしか起きないので放置」と判断する** — 速打ち / タブ切替頻度が高いユーザで必ず顕在化する
2. **Phase 1 / Phase 2 / Phase 3 の hardening で満足する** — 0.6.4 で Phase 1〜3 を入れた後も 0.6.6 で unload race が別経路で残っていた。hardening は root cause ではない
3. **「たぶんこの関数が悪い」で修正を入れる** — WinDbg / ログで真因を確定してから直す。当てずっぽう修正は別経路の race を増やすだけ
4. **BG スレッドで I/O する** — cdylib × reload で engine-host が壊れる（0.6.5 の教訓）。確定時同期保存で十分
5. **新機能を crash 修正と同じ PR に混ぜる** — git blame / diff レビューが壊れ、回帰の原因特定が困難になる
