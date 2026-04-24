# Explorer 異常終了 対策年表

<!-- markdownlint-disable MD024 -->
<!-- MD024: 各バージョンで「症状」「真因」「対策」を繰り返すため無効化 -->

最終更新: 2026-04-24  
位置づけ: `0xc0000005` 系 Explorer / Zoom / Dropbox 等の IME ホストプロセス異常終了に対し、0.4.x から 0.6.x にかけて実施した対策の年表と学習。**v0.6.6 で unload race の真因を掴むまで数段階の試行錯誤があり**、その過程で得た知見を将来の再発時に即座に引き出せるようにまとめる。

関連資料:

- [INVESTIGATION_GUIDE.md](INVESTIGATION_GUIDE.md) — クラッシュ dump 解析プロトコル
- [CHANGELOG.md](../CHANGELOG.md) — 各バージョンの詳細
- [handoff.md](handoff.md) — 現在の状態と既知の問題

---

## 1. 全体像

```text
0.4.3 以前: 単一プロセス構成
    └─ TSF DLL が engine DLL (llama.cpp 同梱) を直接 LoadLibrary
       Zoom / Dropbox / explorer で `msvcp140.dll` クロスロード起因の AV

0.4.4: out-of-process 化
    └─ エンジンを `rakukan-engine-host.exe` に分離、RPC (Named Pipe) 化
       Zoom / Dropbox は完全回避。Explorer は別経路で残存

0.6.3: ローマ字入力時の未確定文字消失修正
    └─ crash とは別だが、TSF hot path の整合性改善

0.6.4: Phase 1〜3 hardening
    └─ Phase 1: COMPOSITION の stale フラグ化
    └─ Phase 2: Phase1A EditSession の DM 再検証
    └─ Phase 3: panic 直結箇所の Result 化

0.6.5: 学習履歴ファイルを bincode 分離 + BG スレッド撤去
    └─ cdylib × reload で engine-host が壊れる問題の副次対策

0.6.6: unload race の真因対策
    └─ DllCanUnloadNow を常に S_FALSE 固定（TSF DLL のプロセス常駐化）
       WinDbg !analyze で確定、以降実機で crash 0 件
```

---

## 2. 0.4.3 以前 — 単一プロセス起因の `msvcp140.dll` クロスロード AV

### 症状

- Zoom / Dropbox / Explorer が IME アクティブ化後に不定期で `0xc0000005` で落ちる
- 再現率は環境依存で、アプリごとに発生しないものもあった

### 真因

TSF DLL がエンジン DLL（`rakukan_engine_*.dll`、llama.cpp 同梱）を直接 `LoadLibrary` していたため、IME クライアントプロセスに `llama.cpp` と VC ランタイム（`msvcp140.dll` 等）が持ち込まれた。クライアントプロセスが既にロードしている同名 DLL とバージョンが衝突すると、関数ポインタ不整合で call 時に AV。

### 対策: 0.4.4 の out-of-process 化

- `rakukan-engine-host.exe` を新設し、エンジン実体をホストプロセスに集約
- TSF 側は `rakukan-engine-rpc` クレート経由で Named Pipe (`\\.\pipe\rakukan-engine-<user-sid>`) + postcard フレーミングで呼び出す
- `RpcEngine` は `DynEngine` と同じメソッドシグネチャを露出するため、既存コードは型 import 差し替えのみで追従
- Activate 時点では engine DLL には触れず、**最初の実入力**（`engine_try_get_or_create()` が呼ばれる瞬間）まで RPC 接続もホスト spawn も一切発生しない
- Zoom / Dropbox のように IME を使わないアプリではホストプロセスも起動しない

### 学び

- **外部 DLL をクライアントプロセスに持ち込むな**。VC ランタイム衝突は検知が難しく、再現条件が環境依存
- 同じ COM インタフェースを露出する RPC プロキシで既存コードを置換する手法は、TSF 側の書き換えを最小化できた

---

## 3. 0.6.4 — Phase 1〜3 hardening（単一プロセス時代の残存 race の封じ込め）

### 症状

Explorer で `OnUninitDocumentMgr` → Phase1A `EditSession` callback が stale な composition / DM を掴むケースで時折 `0xc0000005` が発生。0.4.x で msvcp140 クロスロードは解消したが、それとは別経路の AV が残っていた。

### 真因

- `COMPOSITION` 構造体が `dm_ptr` を持たず、DM 破棄時に自動無効化されない
- Phase1A の `EditSession` callback は `RequestEditSession` 時点の context を握ったまま実行されるが、非同期実行される間に focus DM が切り替わっていることがある
- `EditSession` 内部で `unwrap()` が panic すると、`panic = "abort"` 下では TSF DLL 全体がプロセスを落とす

### 対策（3 段階）

**Phase 1**: `COMPOSITION` 構造体に `dm_ptr` / `stale` フィールドを追加。`OnUninitDocumentMgr` で破棄される DM に紐づく `COMPOSITION` に stale フラグを立てる。msctf コールバック中に即 drop せず後続の安全な文脈で無効化。

**Phase 2**: Phase1A `EditSession` callback 冒頭で `current_focus_dm_ptr()` を再検証。`live_input_notify()` 時点の DM と一致しなければ `E_FAIL` で中断。

**Phase 3**: `EditSession` 経路の panic 直結箇所を `Result` 化:

- `get_insert_range_or_end()` / `get_document_end_range()` で `unwrap()` 撤去
- `suffix_after_prefix_or_empty()` で byte index 依存の panic 抑止

検証スクリプト `scripts/verify-phase3.ps1` で hardening 完了を機械的に検証可能。

### 学び

- TSF の非同期 `EditSession` は **実行時に再検証**するのが原則（登録時点の context は信用しない）
- `panic = "abort"` 下では TSF DLL 内の panic が即プロセス停止になるので、hot path の panic 経路は全て `Result` に変換する
- stale フラグによる後延し無効化は、msctf コールバック再入を避ける上で重要

### 結果

Phase 1〜3 で Explorer の Phase1A 周辺の crash は大幅に減少。ただし根本的な unload race は残存（0.6.6 で判明）。

---

## 4. 0.6.5 — 学習履歴の BG スレッド撤去

### 症状

WinUI 設定保存後の `engine_reload` で `rakukan-engine-host.exe` が高確率で crash。変換不能になり、IME モード切替や再ログオンで復旧。

### 真因

`learn_history` の常駐 worker スレッド、および `engine_start_load_model` / `engine_start_load_dict` の初期化スレッドが engine DLL 内で実行されている状態で、reload 経路が:

1. `*g = None` で `DynEngine` を drop → `Arc<Library>` refcount 1→0 → `FreeLibrary` → engine DLL が unmap
2. 直後に `load_engine_into` で `Library::new`（新 `LoadLibrary`）

「1 と 2 の間」に DLL がプロセスから完全に消える瞬間があり、実行中のスレッドが unmapped な命令ポインタを指して AV。

### 対策

- `learn_history` を独立ファイル (`%APPDATA%\rakukan\learn_history.bin`) に分離し、BG スレッドを撤去
- 学習書き込みは `learn()` 内で同期実行（アトミック書き込み `.bin.tmp` → rename）
- write lock は in-memory 更新中のみ、I/O は snapshot に対して lock 外で実行

### 学び（重要、auto-memory にも登録済み）

- **engine DLL 内で BG スレッド / Drop I/O 禁止** — `cdylib` × reload で engine-host が壊れる
- 学習系は確定時同期保存で十分な性能が出る（UserHistoryPredictor 準拠のスコア式でも同期 write は 1ms 未満）

### 結果

reload 時の engine-host crash は大幅に減少。ただし **他の BG スレッド経由の unload race は M1.6（v0.7.1 予定）で host プロセス再起動化することで根絶予定**。

---

## 5. 0.6.6 — 真因特定と決着: `DllCanUnloadNow` の常時 `S_FALSE`

### 症状

0.6.4 / 0.6.5 の対策後も、Explorer で `0xc0000005` が再発。WinUI 設定を開閉しただけで Explorer が固まり、タスクバーが再起動するケースもあった。

### 解析

2026-04-22 07:23 (UTC 22:23) のクラッシュダンプ `explorer.exe.3124.dmp` を WinDbg で `!analyze -v` 解析:

```
Failure.Bucket = BAD_INSTRUCTION_PTR_c0000005_rakukan_tsf.dll!Unloaded
Stack:
  explorer!CTray::_MessageLoop
  → PeekMessageW
  → UserCallWinProcCheckWow
  → <Unloaded_rakukan_tsf.dll>+0x13e70
```

### 真因（確定）

`candidate_window.rs:166` の `RegisterClassW` で登録した window class が `UnregisterClassW` されないまま、`DllCanUnloadNow=S_OK` で `FreeLibrary` された。その後 in-flight な `WM_TIMER` / `WM_PAINT` / kernel callback continuation が、消えた wnd_proc アドレスを呼び出して AV。

過去の Phase 1〜3 対策は EditSession 側の race を潰していたが、**DLL unload 自体を TSF が許可するために、unregistered な window class への in-flight message が残る**という別経路の race は残っていた。

### 対策

`DllCanUnloadNow` で常に `S_FALSE` を返し、TSF DLL をプロセス常駐させる。

```rust
#[unsafe(no_mangle)]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    // プロセス常駐させて unload race を完全回避
    // Microsoft 標準 IME も同パターン
    S_FALSE
}
```

メモリコストは TSF クライアントプロセス毎に ~2 MB 程度で実用上無視できる（Microsoft 標準 IME も同規模）。

### 学び（最重要）

- **TSF DLL は unload させない**。Unregister 漏れは COM オブジェクトだけでなく window class / timer / hook 等にも潜在し、完全列挙は現実的に不可能
- **Microsoft 標準 IME と同じパターンに合わせる**のが安全（Microsoft が大量の実アプリで検証済みの選択は踏襲する価値がある）
- WinDbg `!analyze -v` の `Failure.Bucket` が `<module>!Unloaded` を指していたら、まず unload race を疑う。callstack 復元が一見不完全でも、bucket 名だけで方向は決まる

### 結果

v0.6.6 以降、実機運用で **Explorer の異常終了は 0 件**（2026-04-24 時点）。crash root cause はほぼ収束と判断し、v0.7.x の主目的を「新機能」ではなく「観測済み bug の除去 + 土台整備」に切り替え。

---

## 6. 将来の再発時の運用

### 再発観察

`%LOCALAPPDATA%\CrashDumps\explorer.exe.*.dmp` の新規発生を運用中にたまに確認する。

- 発生 0 件が続く場合 → M5（WM_TIMER → PostMessage / Explorer シェル分岐）を開封しない
- 新規 dump 発生 → [INVESTIGATION_GUIDE.md](INVESTIGATION_GUIDE.md) のプロトコルで解析、ROADMAP M5 の開封を検討

### M5 として保留中の対策

`docs/ROADMAP.md §8` に保留:

- **WM_TIMER → PostMessage 化**: `WM_RAKUKAN_LIVE_READY` を `RegisterWindowMessageW` で取得し、worker 完了時に `PostMessage` に変更。WM_TIMER ベースの 50ms ポーリング廃止
- **Explorer シェルクラスで Phase1A 無効化**: `GetClassNameW` で `Shell_TrayWnd` / `Progman` / `WorkerW` / `CabinetWClass` / `ExploreWClass` を検出し Phase1A をスキップ

これらは現状の v0.6.6 / v0.7.0 で crash が観測されていないため **先行投資しない**方針。再発したときだけ開封する。

---

## 7. 教訓まとめ（次の TSF DLL 書き換え者へ）

1. **外部 DLL をクライアントプロセスに持ち込むな**（0.4.4 の教訓）
2. **TSF DLL は unload させない**（0.6.6 の教訓、`DllCanUnloadNow=S_FALSE` 固定）
3. **engine DLL 内で BG スレッド / Drop I/O 禁止**（0.6.5 の教訓）
4. **TSF 非同期 EditSession は実行時に再検証**（0.6.4 の教訓）
5. **`panic = "abort"` 下では hot path の panic 経路を全て `Result` 化**（0.6.4 の教訓）
6. **WinDbg `!analyze -v` の `Failure.Bucket` を最優先で見る**（0.6.6 の教訓）
7. **Microsoft 標準 IME と挙動を揃える**のが安全（0.6.6 の教訓）
