# scripts/setup-env.ps1
# Phase 0: 開発環境の自動セットアップ
# 管理者権限で実行: powershell -ExecutionPolicy Bypass -File scripts/setup-env.ps1
#
# ⚠️  注意: このスクリプトはソフトウェアをインストールします。
#           内容を確認してから実行してください。

$ErrorActionPreference = "Stop"

function Install-IfMissing {
    param($name, $wingetId, $testCmd)
    try {
        Invoke-Expression $testCmd | Out-Null
        Write-Host "  [スキップ] $name は既にインストール済みです" -ForegroundColor Gray
    } catch {
        Write-Host "  [インストール中] $name ..." -ForegroundColor Cyan
        winget install --id $wingetId --silent --accept-package-agreements --accept-source-agreements
        Write-Host "  [完了] $name" -ForegroundColor Green
    }
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " rakukan Phase 0 環境セットアップ" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# winget の確認
try {
    winget --version | Out-Null
} catch {
    Write-Host "winget が見つかりません。" -ForegroundColor Red
    Write-Host "Microsoft Store から 'アプリ インストーラー' をインストールしてください。" -ForegroundColor Yellow
    exit 1
}

# ── Rust ──────────────────────────────────
Write-Host "[1/4] Rust をセットアップ中..." -ForegroundColor Cyan
Install-IfMissing "Rustup" "Rustlang.Rustup" "rustup --version"

# MSVC ターゲットの追加
Write-Host "  MSVC ターゲットを追加中..."
rustup target add x86_64-pc-windows-msvc
rustup target add aarch64-pc-windows-msvc  # ARM64（任意）
Write-Host "  [完了] Rust ターゲット設定" -ForegroundColor Green

# cargo-make のインストール
Write-Host "  cargo-make をインストール中..."
cargo install cargo-make --quiet
Write-Host "  [完了] cargo-make" -ForegroundColor Green

Write-Host ""

# ── Visual Studio Build Tools ──────────────
Write-Host "[2/4] Visual Studio Build Tools をセットアップ中..." -ForegroundColor Cyan
Write-Host "  (インストール済みの場合はスキップされます)" -ForegroundColor Gray

$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
$hasCppTools = $false
if (Test-Path $vswhere) {
    $result = & $vswhere -products * -requires "Microsoft.VisualStudio.Component.VC.Tools.x86.x64" -latest 2>&1
    $hasCppTools = $result -ne ""
}

if ($hasCppTools) {
    Write-Host "  [スキップ] MSVC C++ ツールは既にインストール済みです" -ForegroundColor Gray
} else {
    Write-Host "  MSVC Build Tools をインストール中..." -ForegroundColor Cyan
    Write-Host "  (これには数分かかります)" -ForegroundColor Gray
    winget install --id Microsoft.VisualStudio.2022.BuildTools `
        --silent `
        --accept-package-agreements `
        --accept-source-agreements `
        --override "--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended --quiet"
    Write-Host "  [完了] MSVC Build Tools" -ForegroundColor Green
}

Write-Host ""

# ── CMake ─────────────────────────────────
Write-Host "[3/4] CMake をセットアップ中..." -ForegroundColor Cyan
Install-IfMissing "CMake" "Kitware.CMake" "cmake --version"

# PATH を更新（インストール直後に認識されないことがある）
$env:PATH = [System.Environment]::GetEnvironmentVariable("PATH", "Machine") + ";" +
            [System.Environment]::GetEnvironmentVariable("PATH", "User")

Write-Host ""

# ── Git & ツール ──────────────────────────
Write-Host "[4/4] Git とツールをセットアップ中..." -ForegroundColor Cyan
Install-IfMissing "Git" "Git.Git" "git --version"
Install-IfMissing "Git LFS" "GitHub.GitLFS" "git lfs version"

# grpcurl（Phase 2 のテスト用・任意）
Write-Host ""
Write-Host "  grpcurl（Phase 2 の動作確認に使用・任意）" -ForegroundColor Gray
$installGrpcurl = Read-Host "  インストールしますか？ [y/N]"
if ($installGrpcurl -eq "y" -or $installGrpcurl -eq "Y") {
    winget install --id fullstorydev.grpcurl --silent --accept-package-agreements
    Write-Host "  [完了] grpcurl" -ForegroundColor Green
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Green
Write-Host " セットアップ完了！" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Green
Write-Host ""
Write-Host "次のステップ:" -ForegroundColor Cyan
Write-Host "  1. シェルを再起動する（PATH を反映するため）"
Write-Host "  2. 環境確認: powershell -File scripts/check-env.ps1"
Write-Host "  3. ビルド確認: cargo build"
Write-Host ""


# ── LLVM (libclang) ──────────────────────────────────────────────────────────
Write-Host "[LLVM インストール]" -ForegroundColor Cyan
Write-Host "  llama-cpp-sys-2 の bindgen が必要とします"

$llvmBin = $null

# 既にインストール済みか確認
$llvmSearchPaths = @(
    "C:\Program Files\LLVM\bin",
    "C:\Program Files (x86)\LLVM\bin",
    "$env:LOCALAPPDATA\Programs\LLVM\bin"
)
foreach ($p in $llvmSearchPaths) {
    if (Test-Path (Join-Path $p "libclang.dll")) {
        $llvmBin = $p
        break
    }
}

if ($llvmBin) {
    Write-Host "  [OK] LLVM は既にインストール済み: $llvmBin" -ForegroundColor Green
} else {
    Write-Host "  LLVM をインストール中 (winget)..."
    winget install LLVM.LLVM --silent --accept-source-agreements --accept-package-agreements
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [警告] winget でのインストールに失敗しました" -ForegroundColor Yellow
        Write-Host "  手動インストール: https://github.com/llvm/llvm-project/releases" -ForegroundColor Yellow
        Write-Host "  'LLVM-XX.X.X-win64.exe' をダウンロードして実行してください" -ForegroundColor Yellow
    } else {
        # インストール後にパスを再確認
        foreach ($p in $llvmSearchPaths) {
            if (Test-Path (Join-Path $p "libclang.dll")) {
                $llvmBin = $p
                Write-Host "  [OK] LLVM インストール完了: $llvmBin" -ForegroundColor Green
                break
            }
        }
    }
}

# LIBCLANG_PATH を現在のセッションと永続的に設定
if ($llvmBin) {
    $env:LIBCLANG_PATH = $llvmBin
    [Environment]::SetEnvironmentVariable("LIBCLANG_PATH", $llvmBin, "User")
    Write-Host "  [OK] LIBCLANG_PATH を設定しました: $llvmBin" -ForegroundColor Green
    Write-Host "  ※ 新しいシェルを開くと自動的に有効になります" -ForegroundColor Gray
} else {
    Write-Host "  [NG] LLVM が見つかりません。手動でインストールしてください" -ForegroundColor Red
    Write-Host "       https://github.com/llvm/llvm-project/releases" -ForegroundColor Yellow
}

Write-Host ""

