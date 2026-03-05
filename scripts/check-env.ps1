# scripts/check-env.ps1
# Phase 0: 開発環境の確認スクリプト
# 実行: powershell -ExecutionPolicy Bypass -File scripts/check-env.ps1

$ErrorActionPreference = "Continue"
$failed = @()
$warnings = @()

function Check-Command {
    param($name, $cmd, $required = $true)
    try {
        $result = Invoke-Expression $cmd 2>&1
        Write-Host "  [OK] $name" -ForegroundColor Green
        return $true
    } catch {
        if ($required) {
            Write-Host "  [NG] $name — 必須" -ForegroundColor Red
            $script:failed += $name
        } else {
            Write-Host "  [--] $name — 任意（なくても可）" -ForegroundColor Yellow
            $script:warnings += $name
        }
        return $false
    }
}

function Check-VSComponent {
    param($component)
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) { return $false }
    $result = & $vswhere -products * -requires $component -latest 2>&1
    return $result -ne ""
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " rakukan Phase 0 環境チェック" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# ── Rust ──────────────────────────────────
Write-Host "[Rust]" -ForegroundColor Cyan
Check-Command "rustc" "rustc --version"
Check-Command "cargo" "cargo --version"
Check-Command "cargo-make" "cargo make --version" -required $false

# rustup target 確認
$targets = rustup target list --installed 2>&1
if ($targets -match "x86_64-pc-windows-msvc") {
    Write-Host "  [OK] target: x86_64-pc-windows-msvc" -ForegroundColor Green
} else {
    Write-Host "  [NG] target: x86_64-pc-windows-msvc — 追加が必要" -ForegroundColor Red
    $failed += "rustup target x86_64-pc-windows-msvc"
}

Write-Host ""


# ── LLVM / libclang ──────────────────────────
Write-Host "[LLVM]" -ForegroundColor Cyan
# llama-cpp-sys-2 の bindgen が libclang.dll を必要とする
$llvmFound = $false

# 1. LIBCLANG_PATH 環境変数
if ($env:LIBCLANG_PATH) {
    $clangDll = Get-ChildItem $env:LIBCLANG_PATH -Filter "libclang.dll" -ErrorAction SilentlyContinue
    if ($clangDll) {
        Write-Host "  [OK] LIBCLANG_PATH: $($clangDll.FullName)" -ForegroundColor Green
        $llvmFound = $true
    }
}

# 2. LLVM デフォルトインストールパスを探す
if (-not $llvmFound) {
    $llvmPaths = @(
        "C:\Program Files\LLVM\bin",
        "C:\Program Files (x86)\LLVM\bin",
        "$env:LOCALAPPDATA\Programs\LLVM\bin"
    )
    foreach ($p in $llvmPaths) {
        if (Test-Path (Join-Path $p "libclang.dll")) {
            Write-Host "  [OK] libclang.dll: $p\libclang.dll" -ForegroundColor Green
            Write-Host "  [!] LIBCLANG_PATH が未設定です。以下を実行してください:" -ForegroundColor Yellow
            Write-Host "      `$env:LIBCLANG_PATH = `"$p`"" -ForegroundColor Yellow
            $llvmFound = $true
            break
        }
    }
}

if (-not $llvmFound) {
    Write-Host "  [NG] libclang.dll が見つかりません (LLVM 必須)" -ForegroundColor Red
    Write-Host "      setup-env.ps1 を実行してインストールしてください" -ForegroundColor Yellow
    $missingRequired++
}

# ── Visual Studio Build Tools ──────────────
Write-Host "[Visual Studio / Build Tools]" -ForegroundColor Cyan
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $vswhere) {
    Write-Host "  [OK] vswhere.exe 確認" -ForegroundColor Green

    if (Check-VSComponent "Microsoft.VisualStudio.Component.VC.Tools.x86.x64") {
        Write-Host "  [OK] MSVC C++ ツール" -ForegroundColor Green
    } else {
        Write-Host "  [NG] MSVC C++ ツール — VS Installer で追加が必要" -ForegroundColor Red
        $failed += "MSVC C++ Tools"
    }
} else {
    Write-Host "  [NG] Visual Studio / Build Tools が見つかりません" -ForegroundColor Red
    $failed += "Visual Studio Build Tools"
}

Write-Host ""

# ── CMake ─────────────────────────────────
Write-Host "[CMake]" -ForegroundColor Cyan
if (Check-Command "cmake" "cmake --version") {
    $cmakeVer = (cmake --version 2>&1 | Select-Object -First 1) -replace "cmake version ", ""
    $major = [int]($cmakeVer.Split(".")[0])
    if ($major -ge 3) {
        Write-Host "  [OK] CMake $cmakeVer (3.x 以上)" -ForegroundColor Green
    } else {
        Write-Host "  [NG] CMake $cmakeVer — 3.x 以上が必要" -ForegroundColor Red
        $failed += "CMake 3.x+"
    }
}

Write-Host ""

# ── Git ───────────────────────────────────
Write-Host "[Git]" -ForegroundColor Cyan
Check-Command "git" "git --version"

# Git LFS（モデルファイルのダウンロードに必要）
if (Check-Command "git-lfs" "git lfs version" -required $false) {
    Write-Host "  [OK] Git LFS" -ForegroundColor Green
} else {
    $warnings += "Git LFS（大容量モデルファイルに必要になる可能性あり）"
}

Write-Host ""

# ── grpcurl（Phase 2 のテストツール）────────
Write-Host "[テストツール]" -ForegroundColor Cyan
Check-Command "grpcurl" "grpcurl --version" -required $false

Write-Host ""

# ── 結果サマリー ──────────────────────────
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " 結果" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan

if ($failed.Count -eq 0) {
    Write-Host ""
    Write-Host "  すべての必須項目が OK です。Phase 0 に進めます！" -ForegroundColor Green
    Write-Host ""
} else {
    Write-Host ""
    Write-Host "  以下の必須項目が不足しています:" -ForegroundColor Red
    $failed | ForEach-Object { Write-Host "    - $_" -ForegroundColor Red }
    Write-Host ""
    Write-Host "  下記の setup-env.ps1 で自動インストールできます:" -ForegroundColor Yellow
    Write-Host "    powershell -ExecutionPolicy Bypass -File scripts/setup-env.ps1" -ForegroundColor Yellow
    Write-Host ""
}

if ($warnings.Count -gt 0) {
    Write-Host "  任意項目（なくても進められます）:" -ForegroundColor Yellow
    $warnings | ForEach-Object { Write-Host "    - $_" -ForegroundColor Yellow }
    Write-Host ""
}
