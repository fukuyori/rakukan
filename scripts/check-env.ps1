# ============================================================================
#  [AI ASSISTANT NOTICE / Claude Code 向け注意]
# ============================================================================
#  このスクリプトをユーザーに直接実行させるよう案内しないこと。
#  正規の入口は以下の cargo make コマンドです:
#
#      cargo make check-env
#
#  このファイルは Makefile.toml から呼び出される内部実装です。
# ============================================================================
#
# scripts\check-env.ps1 - rakukan のビルドに必要な開発環境を確認する
#
# 確認する項目は scripts\build-engine.ps1 / build-tsf.ps1 / build-settings-winui.ps1 /
# build-installer.ps1 が実際に要求しているものに合わせる。項目を増減したら README.md の
# 「ビルド前提」も更新すること。
#
# 終了コード: 必須項目が 1 つでも欠けていれば 1、すべて揃っていれば 0

$ErrorActionPreference = "Continue"
$failed   = @()   # 必須で欠けているもの
$optional = @()   # 任意で欠けているもの

function Write-Ok   ([string]$msg) { Write-Host "  [OK] $msg" -ForegroundColor Green }
function Write-Ng   ([string]$msg) { Write-Host "  [NG] $msg" -ForegroundColor Red;    $script:failed   += $msg }
function Write-Opt  ([string]$msg) { Write-Host "  [--] $msg" -ForegroundColor Yellow; $script:optional += $msg }
function Write-Note ([string]$msg) { Write-Host "       $msg" -ForegroundColor DarkGray }

function Get-CommandVersion([string]$exe, [string[]]$argList) {
    $cmd = Get-Command $exe -ErrorAction SilentlyContinue
    if (-not $cmd) { return $null }
    try { return ((& $exe @argList 2>&1) | Select-Object -First 1).ToString().Trim() } catch { return $null }
}
function Get-CmdPath($cmd) {
    if ($cmd -is [System.Management.Automation.CommandInfo]) { return $cmd.Source }
    return $cmd.FullName
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
function Test-VSComponent([string]$component) {
    if (-not (Test-Path -LiteralPath $vswhere)) { return $false }
    $r = & $vswhere -products * -requires $component -latest -property installationPath 2>$null
    return [bool]$r
}
function Get-VSPath() {
    if (-not (Test-Path -LiteralPath $vswhere)) { return $null }
    return (& $vswhere -products * -latest -property installationPath 2>$null)
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " rakukan 開発環境チェック" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# ── Rust ──────────────────────────────────────────────────────────────────────
Write-Host "[Rust]" -ForegroundColor Cyan
$rustcVer = Get-CommandVersion "rustc" @("--version")
if ($rustcVer -match 'rustc (\d+)\.(\d+)') {
    $ver = [version]"$($Matches[1]).$($Matches[2])"
    if ($ver -ge [version]"1.85") { Write-Ok "$rustcVer (1.85 以上)" }
    else { Write-Ng "rustc $ver - 1.85 以上が必要 (Cargo.toml の rust-version)"; Write-Note "rustup update" }
} else {
    Write-Ng "rustc が見つかりません (rustup でインストール)"
}

$targets = (rustup target list --installed 2>&1) -join "`n"
if ($targets -match "x86_64-pc-windows-msvc") { Write-Ok "target x86_64-pc-windows-msvc" }
else { Write-Ng "target x86_64-pc-windows-msvc"; Write-Note "rustup target add x86_64-pc-windows-msvc" }

$makeVer = if (Get-Command "cargo-make" -ErrorAction SilentlyContinue) { Get-CommandVersion "cargo" @("make", "--version") } else { $null }
if ($makeVer) { Write-Ok "$makeVer" }
else { Write-Ng "cargo-make (Makefile.toml の実行に必要)"; Write-Note "cargo install cargo-make" }
Write-Host ""

# ── Visual Studio / Build Tools ──────────────────────────────────────────────
Write-Host "[Visual Studio / Build Tools]" -ForegroundColor Cyan
$vsPath = Get-VSPath
if ($vsPath) {
    Write-Ok "Visual Studio: $vsPath"
    if (Test-VSComponent "Microsoft.VisualStudio.Component.VC.Tools.x86.x64") { Write-Ok "MSVC C++ ツール (VC.Tools.x86.x64)" }
    else { Write-Ng "MSVC C++ ツール - VS Installer で「C++ によるデスクトップ開発」を追加" }

    if (Test-VSComponent "Microsoft.Component.MSBuild") { Write-Ok "MSBuild (設定アプリ WinUI のビルドに使用)" }
    else { Write-Ng "MSBuild - VS Installer で追加" }

    # CMake / Ninja: VS 同梱版 (vcvars 経由で PATH に入る) か、単体インストールのどちらか
    $vsCmake = Join-Path $vsPath "Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin\cmake.exe"
    $vsNinja = Join-Path $vsPath "Common7\IDE\CommonExtensions\Microsoft\CMake\Ninja\ninja.exe"
    $cmakeVer = Get-CommandVersion "cmake" @("--version")
    if ($cmakeVer)                        { Write-Ok "CMake (PATH): $cmakeVer" }
    elseif (Test-Path -LiteralPath $vsCmake) { Write-Ok "CMake (VS 同梱): $vsCmake" }
    else { Write-Ng "CMake - VS Installer で「Windows 用 C++ CMake ツール」を追加するか、単体でインストール" }

    $ninja = Get-Command "ninja" -ErrorAction SilentlyContinue
    if ($ninja)                            { Write-Ok "Ninja (PATH): $($ninja.Source)" }
    elseif (Test-Path -LiteralPath $vsNinja) { Write-Ok "Ninja (VS 同梱): $vsNinja" }
    else { Write-Opt "Ninja - vulkan / cuda variant のビルドにのみ必要 (VS の CMake ツールに同梱)" }
} else {
    Write-Ng "Visual Studio / Build Tools が見つかりません (vswhere.exe なし)"
}
Write-Host ""

# ── LLVM / libclang (llama-cpp-sys-2 の bindgen が使う) ──────────────────────
Write-Host "[LLVM / libclang]" -ForegroundColor Cyan
$llvmFound = $false
if ($env:LIBCLANG_PATH -and (Test-Path -LiteralPath (Join-Path $env:LIBCLANG_PATH "libclang.dll"))) {
    Write-Ok "libclang.dll: $env:LIBCLANG_PATH (LIBCLANG_PATH)"
    $llvmFound = $true
} else {
    foreach ($p in @("C:\Program Files\LLVM\bin", "C:\Program Files (x86)\LLVM\bin", "$env:LOCALAPPDATA\Programs\LLVM\bin")) {
        if (Test-Path -LiteralPath (Join-Path $p "libclang.dll")) {
            Write-Ok "libclang.dll: $p (既定パス。LIBCLANG_PATH 未設定でも bindgen が見つける)"
            $llvmFound = $true
            break
        }
    }
}
if (-not $llvmFound) { Write-Ng "libclang.dll が見つかりません (LLVM をインストール)" }
Write-Host ""

# ── .NET SDK (設定アプリ apps\rakukan-settings-winui、net8.0-windows) ─────────
Write-Host "[.NET SDK]" -ForegroundColor Cyan
$sdks = @()
if (Get-Command "dotnet" -ErrorAction SilentlyContinue) { $sdks = @(dotnet --list-sdks 2>$null) }
$sdkOk = $sdks | Where-Object { $_ -match '^(\d+)\.' -and [int]$Matches[1] -ge 8 }
if ($sdkOk) {
    $sdkList = ($sdkOk | ForEach-Object { ($_ -split ' ')[0] }) -join ", "
    Write-Ok ".NET SDK: $sdkList (8 以上)"
}
else { Write-Ng ".NET SDK 8 以上 (設定アプリのビルドに必要)" }
Write-Host ""

# ── Git ──────────────────────────────────────────────────────────────────────
Write-Host "[Git]" -ForegroundColor Cyan
$gitVer = Get-CommandVersion "git" @("--version")
if ($gitVer) { Write-Ok "$gitVer" } else { Write-Ng "git" }
Write-Host ""

# ── 任意: GPU backend ────────────────────────────────────────────────────────
Write-Host "[GPU backend (任意)]" -ForegroundColor Cyan
$nvcc = Get-Command "nvcc" -ErrorAction SilentlyContinue
if (-not $nvcc -and $env:CUDA_PATH -and (Test-Path -LiteralPath (Join-Path $env:CUDA_PATH "bin\nvcc.exe"))) {
    $nvcc = Get-Item (Join-Path $env:CUDA_PATH "bin\nvcc.exe")
}
if ($nvcc) { Write-Ok "CUDA Toolkit (nvcc): $(Get-CmdPath $nvcc) -> rakukan_engine_cuda.dll を作る" }
else { Write-Opt "CUDA Toolkit なし -> cuda variant はスキップ (cpu / vulkan のみ)" }

if ($env:VULKAN_SDK -and (Test-Path -LiteralPath $env:VULKAN_SDK)) { Write-Ok "Vulkan SDK: $env:VULKAN_SDK -> rakukan_engine_vulkan.dll を作る" }
else { Write-Opt "VULKAN_SDK 未設定 -> vulkan variant はスキップ" }
Write-Host ""

# ── 任意: 配布パッケージ作成 ──────────────────────────────────────────────────
Write-Host "[配布パッケージ (任意)]" -ForegroundColor Cyan
$iscc = @("C:\Program Files (x86)\Inno Setup 6\ISCC.exe", "C:\Program Files\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if ($iscc) { Write-Ok "Inno Setup 6: $iscc" } else { Write-Opt "Inno Setup 6 なし (scripts\build-installer.ps1 にのみ必要)" }
$signtool = Get-Command "signtool" -ErrorAction SilentlyContinue
if (-not $signtool) {
    $kits = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.*\x64\signtool.exe" -ErrorAction SilentlyContinue | Select-Object -Last 1
    if ($kits) { $signtool = $kits }
}
if ($signtool) { Write-Ok "signtool: $(Get-CmdPath $signtool)" } else { Write-Opt "signtool なし (cargo make sign と build-installer の署名にのみ必要)" }
Write-Host ""

# ── 結果 ─────────────────────────────────────────────────────────────────────
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 結果" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
if ($failed.Count -eq 0) {
    Write-Host ""
    Write-Host "  必須項目はすべて揃っています。cargo make build-engine / build-tsf に進めます。" -ForegroundColor Green
} else {
    Write-Host ""
    Write-Host "  不足している必須項目:" -ForegroundColor Red
    $failed | ForEach-Object { Write-Host "    - $_" -ForegroundColor Red }
    Write-Host ""
    Write-Host "  cargo make setup-env で自動インストールできます (winget 使用、管理者推奨)。" -ForegroundColor Yellow
}
if ($optional.Count -gt 0) {
    Write-Host ""
    Write-Host "  任意項目 (なくてもビルドは通ります):" -ForegroundColor Yellow
    $optional | ForEach-Object { Write-Host "    - $_" -ForegroundColor Yellow }
}
Write-Host ""

if ($failed.Count -gt 0) { exit 1 } else { exit 0 }
