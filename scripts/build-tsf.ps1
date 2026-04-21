# ============================================================================
#  [AI ASSISTANT NOTICE / Claude Code 向け注意]
# ============================================================================
#  このスクリプトをユーザーに直接実行させるよう案内しないこと。
#  正規の入口は以下の cargo make コマンドです:
#
#      cargo make build-tsf
#
#  このファイルは Makefile.toml から呼び出される内部実装です。
#  PS1 を直接叩く手順を提示すると cargo make と二重化してユーザーが混乱します。
# ============================================================================
#
# scripts\build-tsf.ps1 - TSF 系バイナリのビルド (engine DLL は別)
#
# ビルド対象:
#   rakukan-tsf         (TSF DLL)
#   rakukan-tray        (tray アプリ)
#   rakukan-engine-host (out-of-process RPC サーバ)
#   rakukan-dict-builder (Mozc 辞書ビルダー)
#   WinUI 設定アプリ (rakukan-settings)
#
# 管理者不要。インストールは行わない。

param(
    [ValidateSet("debug","release")] [string]$Profile = "release",
    [string]$BuildDir = "C:\rb"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

Set-Location (Split-Path $PSScriptRoot)

function Invoke-CargoBuild {
    param([string]$Package, [string]$Profile)
    $argList = @("build", "-p", $Package)
    if ($Profile -eq "release") { $argList += "--release" }
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & cargo @argList 2>&1 | ForEach-Object {
        if ($_ -is [System.Management.Automation.ErrorRecord]) {
            Write-Host $_.Exception.Message
        } else {
            Write-Host $_
        }
    }
    $ErrorActionPreference = $prev
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$env:CARGO_TARGET_DIR = $BuildDir
$profileDir = if ($Profile -eq "release") { "release" } else { "debug" }
$cfgName    = if ($Profile -eq "release") { "Release" } else { "Debug" }

# rakukan-tsf does NOT depend on rakukan-engine features (uses DynEngine loader)
Write-Host "[build-tsf] Building rakukan-tsf..."
Invoke-CargoBuild -Package "rakukan-tsf"          -Profile $Profile
Write-Host "[build-tsf] Building rakukan-tray..."
Invoke-CargoBuild -Package "rakukan-tray"         -Profile $Profile
Write-Host "[build-tsf] Building rakukan-engine-host..."
Invoke-CargoBuild -Package "rakukan-engine-host"  -Profile $Profile
Write-Host "[build-tsf] Building rakukan-dict-builder..."
Invoke-CargoBuild -Package "rakukan-dict-builder" -Profile $Profile

Write-Host "[build-tsf] Building WinUI settings ($cfgName)..."
& "$PSScriptRoot\build-settings-winui.ps1" -Configuration $cfgName
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$env:CARGO_TARGET_DIR = $null

# Sanity check
$expected = @(
    (Join-Path $BuildDir "$profileDir\rakukan_tsf.dll")
    (Join-Path $BuildDir "$profileDir\rakukan-tray.exe")
    (Join-Path $BuildDir "$profileDir\rakukan-engine-host.exe")
    (Join-Path $BuildDir "$profileDir\rakukan-dict-builder.exe")
)
foreach ($p in $expected) {
    if (-not (Test-Path -LiteralPath $p)) { throw "[build-tsf] Missing build output: $p" }
}

$winuiBin = Join-Path $PSScriptRoot "..\apps\rakukan-settings-winui\bin\x64\$cfgName\net8.0-windows10.0.19041.0\win-x64"
if (-not (Test-Path -LiteralPath (Join-Path $winuiBin "rakukan-settings.exe"))) {
    throw "[build-tsf] Missing WinUI build output: $winuiBin\rakukan-settings.exe"
}

Write-Host ""
Write-Host "[build-tsf] Done."
Write-Host "  Cargo outputs: $BuildDir\$profileDir\"
Write-Host "  WinUI output:  $winuiBin\"
