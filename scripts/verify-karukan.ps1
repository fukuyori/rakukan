# scripts/verify-karukan.ps1
# Phase 0 最終確認: karukan-core が Windows でビルドできるか検証する
# 実行: powershell -ExecutionPolicy Bypass -File scripts/verify-karukan.ps1

$ErrorActionPreference = "Continue"
$workspaceRoot = Split-Path $PSScriptRoot -Parent
$testDir = Join-Path $env:TEMP "rakukan-karukan-test"

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " karukan-core Windows ビルド検証" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# ── Step 1: karukan をクローン ───────────
Write-Host "[Step 1] karukan リポジトリをクローン中..." -ForegroundColor Cyan

if (Test-Path $testDir) {
    Write-Host "  既存ディレクトリを削除中: $testDir"
    Remove-Item -Recurse -Force $testDir
}

# --depth 1 を外す: 浅いクローンだとサブモジュールが取得できない
git clone https://github.com/togatoga/karukan $testDir
if ($LASTEXITCODE -ne 0) {
    Write-Host "  [NG] クローン失敗" -ForegroundColor Red
    exit 1
}
Write-Host "  [OK] クローン完了: $testDir" -ForegroundColor Green
Set-Location $testDir

Write-Host ""

# ── Step 2: サブモジュールを明示的に取得 ──
Write-Host "[Step 2] サブモジュール（llama.cpp など）を取得中..." -ForegroundColor Cyan
Write-Host "  (初回は数分かかります)" -ForegroundColor Gray

git submodule update --init --recursive
if ($LASTEXITCODE -ne 0) {
    Write-Host "  [警告] サブモジュール取得でエラーが発生しましたが続行します" -ForegroundColor Yellow
}

# llama.cpp の存在を確認
$llamaFiles = Get-ChildItem -Recurse -Filter "llama.h" -ErrorAction SilentlyContinue
if ($llamaFiles) {
    Write-Host "  [OK] llama.cpp サブモジュール確認 (llama.h を検出)" -ForegroundColor Green
} else {
    Write-Host "  [警告] llama.h が見つかりません。ビルドが失敗する可能性があります" -ForegroundColor Yellow
}

Write-Host ""

# ── Step 3: ワークスペース内のパッケージ名を動的に取得 ──
Write-Host "[Step 3] ワークスペースのパッケージ構成を確認..." -ForegroundColor Cyan

# cargo metadata でパッケージ一覧を取得
$metaJson = cargo metadata --no-deps --format-version 1 2>&1 | Out-String
if ($LASTEXITCODE -ne 0) {
    Write-Host "  [NG] cargo metadata 失敗。Rust 環境を確認してください" -ForegroundColor Red
    exit 1
}
$meta = $metaJson | ConvertFrom-Json
$packages = $meta.packages | Select-Object -ExpandProperty name
Write-Host "  検出されたパッケージ: $($packages -join ', ')" -ForegroundColor Gray

# 変換エンジンのパッケージを特定（"core" または "engine" を含むもの優先、なければ最初の1つ）
$enginePkg = $packages | Where-Object { $_ -match "core|engine" } | Select-Object -First 1
if (-not $enginePkg) {
    $enginePkg = $packages | Select-Object -First 1
}
Write-Host "  ビルド対象パッケージ: $enginePkg" -ForegroundColor Green

Write-Host ""

# ── Step 4: GPU 検出 & ビルド順序を決定 ──
Write-Host "[Step 4] GPU を検出してビルド順序を決定中..." -ForegroundColor Cyan

$detectScript = Join-Path $PSScriptRoot "detect-gpu.ps1"
. $detectScript
$recommended = $env:RAKUKAN_BACKEND

$buildOrder = switch ($recommended) {
    "cuda"   { @("cuda", "vulkan", "cpu") }
    "vulkan" { @("vulkan", "cpu") }
    default  { @("cpu") }
}

Write-Host "  ビルド試行順序: $($buildOrder -join ' -> ')" -ForegroundColor Gray
Write-Host ""

# ── Step 5: ビルド実行 ────────────────────
Write-Host "[Step 5] $enginePkg をビルド中..." -ForegroundColor Cyan
Write-Host "  (llama.cpp の C++ コンパイルで数分かかります)" -ForegroundColor Gray
Write-Host ""

$env:RUST_LOG = "warn"
# Windows でパス長制限に当たる場合の回避策
$env:TrackFileAccess = "false"
$stopwatch = [System.Diagnostics.Stopwatch]::StartNew()
$buildSuccess = $null

foreach ($backend in $buildOrder) {
    Write-Host "  試行: $backend バックエンド"

    if ($backend -eq "cpu") {
        # CPU: features 指定なし
        cargo build -p $enginePkg 2>&1
    } else {
        # GPU: -F (--features の短縮形) を使う
        # -p と --features を同時に使うと workspace 外エラーになる場合があるため
        # 対象クレートのディレクトリに移動してビルドする
        $pkgDir = $meta.packages |
            Where-Object { $_.name -eq $enginePkg } |
            Select-Object -ExpandProperty manifest_path |
            Split-Path -Parent
        Push-Location $pkgDir
        cargo build --features $backend 2>&1
        Pop-Location
    }

    if ($LASTEXITCODE -eq 0) {
        $stopwatch.Stop()
        Write-Host ""
        Write-Host "  [OK] $backend ビルド成功！ ($([int]$stopwatch.Elapsed.TotalSeconds) 秒)" -ForegroundColor Green
        $buildSuccess = $backend
        break
    } else {
        Write-Host "  [--] $backend ビルド失敗。次を試みます..." -ForegroundColor Yellow
        Write-Host ""
    }
}

Write-Host ""

# ── Step 6: 結果と次のアクション ─────────
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 結果" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

if ($buildSuccess) {
    Write-Host "  Phase 0 ゲート クリア！" -ForegroundColor Green
    Write-Host "  パッケージ名: $enginePkg" -ForegroundColor Green
    Write-Host "  バックエンド: $buildSuccess" -ForegroundColor Green
    Write-Host ""
    Write-Host "  次にやること:" -ForegroundColor Cyan
    Write-Host "  1. rakukan/Cargo.toml に以下を追加する:"
    Write-Host "     [workspace.dependencies]"
    Write-Host "     $enginePkg = { git = `"https://github.com/togatoga/karukan`" }"
    Write-Host ""
    Write-Host "  2. rakukan-engine/Cargo.toml のコメントを外す:"
    Write-Host "     $enginePkg = { workspace = true }"
    Write-Host ""
    Write-Host "  3. cargo test -p rakukan-engine でテストを実行する"
    Write-Host ""

    $result = @{
        engine_package   = $enginePkg
        all_packages     = $packages
        backend          = $buildSuccess
        buildTimeSeconds = [int]$stopwatch.Elapsed.TotalSeconds
        timestamp        = Get-Date -Format "yyyy-MM-dd HH:mm:ss"
    }
    $result | ConvertTo-Json | Set-Content (Join-Path $workspaceRoot "phase0-result.json")
    Write-Host "  結果を phase0-result.json に保存しました" -ForegroundColor Gray

} else {
    Write-Host "  Phase 0 ゲート 未通過" -ForegroundColor Red
    Write-Host ""
    Write-Host "  よくある原因:" -ForegroundColor Yellow
    Write-Host "  A) MSVC が見つからない -> setup-env.ps1 を再実行してシェルを再起動"
    Write-Host "  B) CMake が PATH に通っていない -> cmake --version で確認"
    Write-Host "  C) llama.cpp のサブモジュール取得失敗 -> git submodule update --init --recursive を手動実行"
    Write-Host "  D) Windows パス長制限 -> レジストリで LongPathsEnabled を有効化"
    Write-Host "     reg add HKLM\SYSTEM\CurrentControlSet\Control\FileSystem /v LongPathsEnabled /t REG_DWORD /d 1 /f"
    Write-Host ""
    exit 1
}

Set-Location $workspaceRoot
