# =============================================================================
# scripts\build-installer.ps1
# rakukan IME Inno Setup インストーラー作成スクリプト
#
# 使用方法:
#   cd D:\home\source\rust\rakukan
#   .\scripts\build-installer.ps1
#
# 前提:
#   - cargo make install が完了していること
#   - Inno Setup 6 がインストールされていること
# =============================================================================

param(
    [string]$Version = "0.4.0",
    [string]$InstallDir = "$env:LOCALAPPDATA\rakukan",
    [string]$BuildDir = "C:\rb\release",
    [string]$InstallerScript = "$PSScriptRoot\..\rakukan_installer.iss"
)

$ErrorActionPreference = "Stop"
$distDir = "$PSScriptRoot\..\dist"

# --- ISCC.exe の場所を探す ---
$iscc = @(
    "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    "C:\Program Files\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1

if (-not $iscc) {
    Write-Error "Inno Setup 6 が見つかりません。https://jrsoftware.org/isinfo.php からインストールしてください。"
    exit 1
}

Write-Host "[1/3] dist フォルダを準備中..."
New-Item -ItemType Directory -Force -Path $distDir | Out-Null
New-Item -ItemType Directory -Force -Path "$distDir\models" | Out-Null

# TSF DLL をコピー (固定名)
$tsfDll = Join-Path $InstallDir "rakukan_tsf.dll"
if (-not (Test-Path $tsfDll)) {
    Write-Error "rakukan_tsf.dll が $InstallDir に見つかりません。先に cargo make install を実行してください。"
    exit 1
}
Copy-Item $tsfDll "$distDir\rakukan_tsf.dll" -Force
Write-Host "  -> rakukan_tsf.dll"

# アイコン
$icoSrc = "$PSScriptRoot\..\crates\rakukan-tsf\rakukan.ico"
if (Test-Path $icoSrc) {
    Copy-Item $icoSrc "$distDir\rakukan.ico" -Force
    Write-Host "  -> rakukan.ico"
} else {
    Write-Warning "rakukan.ico が見つかりません ($icoSrc)"
}

# register-tip.ps1 (キーボードリスト登録スクリプト)
Copy-Item "$PSScriptRoot\register-tip.ps1" "$distDir\register-tip.ps1" -Force
Write-Host "  -> register-tip.ps1"

# unregister-tip.ps1 (キーボードリスト削除スクリプト)
Copy-Item "$PSScriptRoot\unregister-tip.ps1" "$distDir\unregister-tip.ps1" -Force
Write-Host "  -> unregister-tip.ps1"

# Engine DLL
foreach ($name in @("rakukan_engine_cpu.dll", "rakukan_engine_vulkan.dll", "rakukan_engine_cuda.dll")) {
    $src = Join-Path $InstallDir $name
    if (Test-Path $src) {
        Copy-Item $src "$distDir\$name" -Force
        Write-Host "  -> $name"
    }
}

# 辞書
$dict = Join-Path $env:LOCALAPPDATA "rakukan\dict\rakukan.dict"
if (Test-Path $dict) {
    Copy-Item $dict "$distDir\rakukan.dict" -Force
    Write-Host "  -> rakukan.dict"
} else {
    Write-Warning "rakukan.dict が見つかりません ($dict)"
}

$vibratoDict = Join-Path $PSScriptRoot "..\assets\vibrato\system.dic"
if (Test-Path $vibratoDict) {
    New-Item -ItemType Directory -Force -Path "$distDir\vibrato" | Out-Null
    Copy-Item $vibratoDict "$distDir\vibrato\system.dic" -Force
    Write-Host "  -> vibrato\\system.dic"
} else {
    Write-Warning "Vibrato system.dic が見つかりません ($vibratoDict)"
}

# ライセンス・帰属表示
$repoRoot = "$PSScriptRoot\.."
foreach ($f in @("NOTICE", "THIRD_PARTY_LICENSES.md")) {
    $src = Join-Path $repoRoot $f
    if (Test-Path $src) {
        Copy-Item $src "$distDir\$f" -Force
        Write-Host "  -> $f"
    } else {
        Write-Warning "$f が見つかりません"
    }
}

# config.toml (デフォルト値が入ったもの)
$configSrc = "$PSScriptRoot\..\config\config.toml"
if (-not (Test-Path $configSrc)) {
    $configSrc = Join-Path $InstallDir "config.toml"
}
if (Test-Path $configSrc) {
    Copy-Item $configSrc "$distDir\config.toml" -Force
    Write-Host "  -> config.toml"
}

# モデル (.gguf) をコピー (存在する場合)
$modelsDir = Join-Path $InstallDir "models"
if (Test-Path $modelsDir) {
    $ggufFiles = Get-ChildItem -Path $modelsDir -Filter "*.gguf"
    foreach ($f in $ggufFiles) {
        Copy-Item $f.FullName "$distDir\models\" -Force
        Write-Host "  -> models\$($f.Name)"
    }
}

# --- バージョン番号をスクリプトに反映 ---
$issContent = Get-Content $InstallerScript -Raw
$issContent = $issContent -replace '#define MyAppVersion\s+"[^"]+"', "#define MyAppVersion   `"$Version`""
$issContent | Set-Content $InstallerScript -NoNewline -Encoding UTF8

Write-Host ""
Write-Host "[2/3] Inno Setup コンパイル中..."
& $iscc $InstallerScript
if ($LASTEXITCODE -ne 0) {
    Write-Error "ISCC.exe が失敗しました (exit code $LASTEXITCODE)"
    exit 1
}

Write-Host ""
Write-Host "[3/3] 完了!"
$outputFile = Get-ChildItem "$PSScriptRoot\..\output\rakukan-*.exe" |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($outputFile) {
    Write-Host "インストーラー: $($outputFile.FullName)"
    Write-Host "サイズ: $([math]::Round($outputFile.Length / 1MB, 1)) MB"
}
