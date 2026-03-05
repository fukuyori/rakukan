# verify-phase1.ps1
# rakukan Phase 1 ゲート検証スクリプト
# rakukan-engine のビルド＋ユニットテストを実行してゲートクリアを確認する

[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " rakukan Phase 1 ゲート検証"            -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# ── スクリプトのディレクトリから rakukan ルートを特定 ─────────────────────────
$scriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Path
$rakukanRoot = Split-Path -Parent $scriptDir   # scripts/ の親 = rakukan/

if (-not (Test-Path (Join-Path $rakukanRoot "Cargo.toml"))) {
    Write-Host "  [NG] rakukan の Cargo.toml が見つかりません: $rakukanRoot" -ForegroundColor Red
    exit 1
}

Set-Location $rakukanRoot
Write-Host "  作業ディレクトリ: $rakukanRoot" -ForegroundColor Gray
Write-Host ""

# ── LIBCLANG_PATH の自動設定 ──────────────────────────────────────────────────
if (-not $env:LIBCLANG_PATH) {
    foreach ($p in @(
        "C:\Program Files\LLVM\bin",
        "C:\Program Files (x86)\LLVM\bin",
        "$env:LOCALAPPDATA\Programs\LLVM\bin"
    )) {
        if (Test-Path (Join-Path $p "libclang.dll")) {
            $env:LIBCLANG_PATH = $p
            Write-Host "  [自動設定] LIBCLANG_PATH = $p" -ForegroundColor Cyan
            break
        }
    }
    if (-not $env:LIBCLANG_PATH) {
        Write-Host "  [NG] libclang.dll が見つかりません。setup-env.ps1 を実行してください。" -ForegroundColor Red
        exit 1
    }
}

$passed  = 0
$failed  = 0
$results = @()

# ── Step 1: cargo check（構文チェック）────────────────────────────────────────
Write-Host "[Step 1] cargo check -p rakukan-engine ..." -ForegroundColor Cyan
$tmpCheck = [System.IO.Path]::GetTempFileName()
cmd /c "cargo check -p rakukan-engine 2>&1" | Tee-Object -FilePath $tmpCheck | ForEach-Object {
    if ($_ -match "^error") { Write-Host "  $_" -ForegroundColor Red }
    elseif ($_ -match "^warning") { Write-Host "  $_" -ForegroundColor Yellow }
}
$checkOk = ($LASTEXITCODE -eq 0)
if (-not $checkOk) {
    Write-Host "  [NG] cargo check 失敗" -ForegroundColor Red
    $failed++
    $results += "cargo check: FAIL"
} else {
    Write-Host "  [OK] コンパイル OK" -ForegroundColor Green
    $passed++
    $results += "cargo check: PASS"
}

# ── Step 2: ユニットテスト（モデル不要）────────────────────────────────────────
Write-Host ""
Write-Host "[Step 2] cargo test -p rakukan-engine （モデル不要テスト）..." -ForegroundColor Cyan
Write-Host "  （初回は依存クレートのコンパイルで数分かかります）"

$start = Get-Date
cmd /c "cargo test -p rakukan-engine 2>&1" | ForEach-Object {
    if ($_ -match "^test .* FAILED") { Write-Host "  $_" -ForegroundColor Red }
    elseif ($_ -match "^test .* ok")  { Write-Host "  $_" -ForegroundColor Green }
    elseif ($_ -match "^error")       { Write-Host "  $_" -ForegroundColor Red }
    else                              { Write-Host "  $_" }
}
$elapsed = [int](New-TimeSpan -Start $start -End (Get-Date)).TotalSeconds
$testOk  = ($LASTEXITCODE -eq 0)
if (-not $testOk) {
    Write-Host "  [NG] テスト失敗 ($elapsed 秒)" -ForegroundColor Red
    $failed++
    $results += "unit tests: FAIL"
} else {
    Write-Host "  [OK] 全テスト PASS ($elapsed 秒)" -ForegroundColor Green
    $passed++
    $results += "unit tests: PASS ($elapsed 秒)"
}

# ── Step 3: CLI ビルド ────────────────────────────────────────────────────────
Write-Host ""
Write-Host "[Step 3] cargo build -p rakukan-engine-cli ..." -ForegroundColor Cyan
cmd /c "cargo build -p rakukan-engine-cli 2>&1" | ForEach-Object {
    if ($_ -match "^error") { Write-Host "  $_" -ForegroundColor Red }
}
if ($LASTEXITCODE -ne 0) {
    Write-Host "  [NG] CLI ビルド失敗" -ForegroundColor Red
    $failed++
    $results += "CLI build: FAIL"
} else {
    Write-Host "  [OK] CLI ビルド完了" -ForegroundColor Green
    $passed++
    $results += "CLI build: PASS"
}

# ── Step 4: モデル一覧が取得できるか（CLI smoke test）──────────────────────────
Write-Host ""
Write-Host "[Step 4] rakukan-cli --list-models ..." -ForegroundColor Cyan
$modelsOut = @()
cmd /c "cargo run -p rakukan-engine-cli -- --list-models 2>&1" | ForEach-Object {
    $modelsOut += $_
}
if ($LASTEXITCODE -ne 0) {
    Write-Host "  [NG] モデル一覧取得失敗" -ForegroundColor Red
    $failed++
    $results += "list-models: FAIL"
} else {
    Write-Host "  [OK] モデル一覧取得 OK" -ForegroundColor Green
    $modelsOut | Where-Object { $_ -notmatch "^   Compiling|^    Finished|^warning:" } | ForEach-Object {
        Write-Host "     $_"
    }
    $passed++
    $results += "list-models: PASS"
}

# ── 結果 ─────────────────────────────────────────────────────────────────────
Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 結果" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

foreach ($r in $results) {
    $color = if ($r -match "FAIL") { "Red" } else { "Green" }
    Write-Host "  $r" -ForegroundColor $color
}

Write-Host ""
if ($failed -eq 0) {
    Write-Host "  Phase 1 ゲート クリア！ ($passed/$($passed+$failed) PASS)" -ForegroundColor Green
    Write-Host ""
    Write-Host "  次のステップ:"
    Write-Host "    統合テスト（モデルDL必要、数分）:"
    Write-Host "      cargo test -p rakukan-engine -- --ignored"
    Write-Host ""
    Write-Host "    対話CLI:"
    Write-Host "      cargo run -p rakukan-engine-cli"
    Write-Host ""
    Write-Host "    Phase 2 (TSF レイヤー接続) へ進む準備が整っています"

    # 結果を JSON に保存
    $json = @{
        phase   = "phase1"
        status  = "pass"
        passed  = $passed
        failed  = $failed
        results = $results
        timestamp = (Get-Date -Format "yyyy-MM-dd HH:mm:ss")
    } | ConvertTo-Json
    $jsonPath = Join-Path $rakukanRoot "phase1-result.json"
    $json | Set-Content $jsonPath -Encoding UTF8
    Write-Host "  結果を保存しました: $jsonPath" -ForegroundColor Gray

    exit 0
} else {
    Write-Host "  Phase 1 ゲート 未通過 ($failed 失敗)" -ForegroundColor Red
    Write-Host ""
    Write-Host "  よくある原因:"
    Write-Host "  A) LLVM 未インストール    -> setup-env.ps1 を実行"
    Write-Host "  B) MSVC ツール不足        -> Visual Studio Build Tools を確認"
    Write-Host "  C) karukan-engine 取得失敗 -> git / ネットワークを確認"
    exit 1
}
