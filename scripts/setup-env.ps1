# ============================================================================
#  [AI ASSISTANT NOTICE / Claude Code 向け注意]
# ============================================================================
#  このスクリプトをユーザーに直接実行させるよう案内しないこと。
#  正規の入口は以下の cargo make コマンドです:
#
#      cargo make setup-env              (管理者推奨: VS Build Tools の導入に必要)
#
#  このファイルは Makefile.toml から呼び出される内部実装です。
# ============================================================================
#
# scripts\setup-env.ps1 - rakukan のビルドに必要な開発環境を winget で導入する
#
# 導入するもの (必須。既に入っていればスキップ):
#   - Rust (rustup) + target x86_64-pc-windows-msvc + cargo-make
#   - Visual Studio 2022 Build Tools: C++ ツール、CMake/Ninja、MSBuild
#   - LLVM (libclang; llama-cpp-sys-2 の bindgen が使う)
#   - .NET SDK 8 (設定アプリ apps\rakukan-settings-winui)
#   - Git
#
# 導入しないもの (任意。案内のみ):
#   - CUDA Toolkit (cuda variant)、Vulkan SDK (vulkan variant)、Inno Setup 6 / signtool (配布用)
#
# 注意: ソフトウェアをインストールする。内容を確認してから実行すること。
#       対話入力はしない (非対話環境からも実行できる)。

$ErrorActionPreference = "Stop"

function Install-WingetPackage([string]$name, [string]$id, [scriptblock]$isInstalled, [string[]]$extraArgs = @()) {
    if (& $isInstalled) {
        Write-Host "  [スキップ] $name は既にインストール済み" -ForegroundColor Gray
        return
    }
    Write-Host "  [インストール] $name ($id) ..." -ForegroundColor Cyan
    $wingetArgs = @("install", "--id", $id, "--exact", "--silent", "--accept-package-agreements", "--accept-source-agreements") + $extraArgs
    & winget @wingetArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  [警告] $name のインストールに失敗 (winget exit=$LASTEXITCODE)。手動で導入してください" -ForegroundColor Yellow
    } else {
        Write-Host "  [完了] $name" -ForegroundColor Green
    }
}

function Refresh-Path() {
    $env:PATH = [Environment]::GetEnvironmentVariable("PATH", "Machine") + ";" + [Environment]::GetEnvironmentVariable("PATH", "User")
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
function Test-VSComponent([string]$component) {
    if (-not (Test-Path -LiteralPath $vswhere)) { return $false }
    return [bool](& $vswhere -products * -requires $component -latest -property installationPath 2>$null)
}

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " rakukan 開発環境セットアップ" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

if (-not (Get-Command "winget" -ErrorAction SilentlyContinue)) {
    Write-Host "winget が見つかりません。Microsoft Store から「アプリ インストーラー」を導入してください。" -ForegroundColor Red
    exit 1
}

# ── [1/5] Rust ───────────────────────────────────────────────────────────────
Write-Host "[1/5] Rust" -ForegroundColor Cyan
Install-WingetPackage "Rustup" "Rustlang.Rustup" { [bool](Get-Command "rustup" -ErrorAction SilentlyContinue) }
Refresh-Path
if (Get-Command "rustup" -ErrorAction SilentlyContinue) {
    rustup target add x86_64-pc-windows-msvc
    Write-Host "  [完了] target x86_64-pc-windows-msvc" -ForegroundColor Green
    if (Get-Command "cargo-make" -ErrorAction SilentlyContinue) {
        Write-Host "  [スキップ] cargo-make は既にインストール済み" -ForegroundColor Gray
    } else {
        cargo install cargo-make --quiet
        Write-Host "  [完了] cargo-make" -ForegroundColor Green
    }
} else {
    Write-Host "  [警告] rustup がまだ PATH にありません。シェルを開き直してから再実行してください" -ForegroundColor Yellow
}
Write-Host ""

# ── [2/5] Visual Studio Build Tools ─────────────────────────────────────────
# C++ ツール (VC.Tools.x86.x64)、CMake/Ninja (VC.CMake.Project)、MSBuild を揃える。
# Visual Studio 本体が入っていて不足コンポーネントだけがある場合も、Build Tools の追加導入で足りる。
Write-Host "[2/5] Visual Studio 2022 Build Tools" -ForegroundColor Cyan
$needVs = -not ((Test-VSComponent "Microsoft.VisualStudio.Component.VC.Tools.x86.x64") -and
                (Test-VSComponent "Microsoft.VisualStudio.Component.VC.CMake.Project") -and
                (Test-VSComponent "Microsoft.Component.MSBuild"))
if ($needVs) {
    Write-Host "  (数分かかります。管理者権限が必要です)" -ForegroundColor Gray
    Install-WingetPackage "VS 2022 Build Tools" "Microsoft.VisualStudio.2022.BuildTools" { $false } @(
        "--override",
        "--add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.VC.CMake.Project --includeRecommended --quiet --wait --norestart"
    )
} else {
    Write-Host "  [スキップ] C++ ツール / CMake / MSBuild は既にインストール済み" -ForegroundColor Gray
}
Write-Host ""

# ── [3/5] LLVM (libclang) ───────────────────────────────────────────────────
Write-Host "[3/5] LLVM (libclang)" -ForegroundColor Cyan
$llvmPaths = @("C:\Program Files\LLVM\bin", "C:\Program Files (x86)\LLVM\bin", "$env:LOCALAPPDATA\Programs\LLVM\bin")
function Find-Libclang() { return ($llvmPaths | Where-Object { Test-Path -LiteralPath (Join-Path $_ "libclang.dll") } | Select-Object -First 1) }
Install-WingetPackage "LLVM" "LLVM.LLVM" { [bool](Find-Libclang) }
$llvmBin = Find-Libclang
if ($llvmBin) {
    if (-not $env:LIBCLANG_PATH) {
        [Environment]::SetEnvironmentVariable("LIBCLANG_PATH", $llvmBin, "User")
        $env:LIBCLANG_PATH = $llvmBin
        Write-Host "  [完了] LIBCLANG_PATH (ユーザー環境変数) = $llvmBin" -ForegroundColor Green
    } else {
        Write-Host "  [スキップ] LIBCLANG_PATH は設定済み: $env:LIBCLANG_PATH" -ForegroundColor Gray
    }
} else {
    Write-Host "  [警告] libclang.dll が見つかりません。https://github.com/llvm/llvm-project/releases から LLVM-*-win64.exe を導入してください" -ForegroundColor Yellow
}
Write-Host ""

# ── [4/5] .NET SDK ──────────────────────────────────────────────────────────
Write-Host "[4/5] .NET SDK 8 (設定アプリ用)" -ForegroundColor Cyan
Install-WingetPackage ".NET SDK 8" "Microsoft.DotNet.SDK.8" {
    $dotnet = Get-Command "dotnet" -ErrorAction SilentlyContinue
    if (-not $dotnet) { return $false }
    [bool](@(dotnet --list-sdks 2>$null) | Where-Object { $_ -match '^(\d+)\.' -and [int]$Matches[1] -ge 8 })
}
Write-Host ""

# ── [5/5] Git ───────────────────────────────────────────────────────────────
Write-Host "[5/5] Git" -ForegroundColor Cyan
Install-WingetPackage "Git" "Git.Git" { [bool](Get-Command "git" -ErrorAction SilentlyContinue) }
Write-Host ""

# ── 任意項目の案内 ──────────────────────────────────────────────────────────
Write-Host "[任意 (自動導入しない)]" -ForegroundColor Cyan
Write-Host "  - CUDA Toolkit : cuda variant (rakukan_engine_cuda.dll) を作る場合。nvcc が PATH か CUDA_PATH にあれば build-engine が検出する" -ForegroundColor Gray
Write-Host "  - Vulkan SDK   : vulkan variant を作る場合。環境変数 VULKAN_SDK が設定されていれば build-engine が検出する" -ForegroundColor Gray
Write-Host "  - Inno Setup 6 / Windows SDK signtool : 配布パッケージ作成と署名にのみ必要" -ForegroundColor Gray
Write-Host ""

Write-Host "========================================" -ForegroundColor Green
Write-Host " セットアップ終了" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Green
Write-Host "次のステップ:" -ForegroundColor Cyan
Write-Host "  1. シェルを開き直す (PATH / LIBCLANG_PATH を反映)"
Write-Host "  2. cargo make check-env   で確認"
Write-Host "  3. cargo make build-engine -> cargo make build-tsf -> サインアウト -> サインイン -> sudo cargo make install"
Write-Host ""
