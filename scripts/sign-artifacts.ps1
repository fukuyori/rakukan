# scripts\sign-artifacts.ps1 - ビルド成果物に電子署名を付与
#
# インストール前の段階で signtool を走らせるため、%LOCALAPPDATA% 配下の
# 実行中プロセスとの競合を回避できる (ロックが起きない)。
#
# 署名対象 (存在するものだけ、未ビルドはスキップ):
#   $BuildDir\$Profile\rakukan_engine_cpu.dll
#   $BuildDir\$Profile\rakukan_engine_vulkan.dll
#   $BuildDir\$Profile\rakukan_engine_cuda.dll
#   $BuildDir\$Profile\rakukan_tsf.dll
#   $BuildDir\$Profile\rakukan-tray.exe
#   $BuildDir\$Profile\rakukan-engine-host.exe
#   $BuildDir\$Profile\rakukan-dict-builder.exe
#   apps\rakukan-settings-winui\bin\x64\$Config\net8.0-windows10.0.19041.0\win-x64\rakukan-settings.exe
#   apps\rakukan-settings-winui\bin\x64\$Config\net8.0-windows10.0.19041.0\win-x64\rakukan-settings.dll
#
# 使い方:
#   cargo make sign
#   powershell -ExecutionPolicy Bypass -File scripts\sign-artifacts.ps1 [-Profile release|debug]

param(
    [ValidateSet("debug","release")] [string]$Profile = "release",
    [string]$BuildDir = "C:\rb",
    [string]$SigntoolPath = $null,
    [string]$TimestampUrl = "http://timestamp.digicert.com",
    [switch]$NoElevate      # 管理者昇格をスキップ (既に昇格済みの場合の内部利用)
)

$ErrorActionPreference = "Stop"

# --- 管理者権限チェック + 自動昇格 ---
# 証明書が LocalMachine ストアにある場合など、signtool は admin 権限を要求することがある。
# 非管理者セッションから cargo make sign を呼んだ場合は UAC で昇格して再実行する。
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin -and -not $NoElevate) {
    Write-Host "[sign] 管理者権限で再起動します (UAC)..." -ForegroundColor Yellow
    $argList = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "`"$PSCommandPath`"", "-NoElevate")
    foreach ($pair in $PSBoundParameters.GetEnumerator()) {
        $name = $pair.Key
        $value = $pair.Value
        if ($value -is [switch]) {
            if ($value.IsPresent) { $argList += "-$name" }
        } elseif ($null -ne $value -and $value -ne "") {
            $argList += "-$name"
            $argList += "`"$value`""
        }
    }
    try {
        $proc = Start-Process -FilePath "powershell.exe" -Verb RunAs -ArgumentList $argList -Wait -PassThru
        exit $proc.ExitCode
    } catch {
        Write-Error "[sign] 管理者昇格に失敗しました: $_"
        exit 1
    }
}

Set-Location (Split-Path $PSScriptRoot)

# --- signtool.exe を検出 ---
if ($SigntoolPath -and (Test-Path -LiteralPath $SigntoolPath)) {
    $signtool = $SigntoolPath
} else {
    $candidates = @()
    $appCertKit = "${env:ProgramFiles(x86)}\Windows Kits\10\App Certification Kit\signtool.exe"
    if (Test-Path -LiteralPath $appCertKit) { $candidates += $appCertKit }

    $binRoot = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    if (Test-Path -LiteralPath $binRoot) {
        Get-ChildItem -Path $binRoot -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending |
            ForEach-Object {
                $p = Join-Path $_.FullName "x64\signtool.exe"
                if (Test-Path -LiteralPath $p) { $candidates += $p }
            }
    }

    $signtool = $candidates | Select-Object -First 1
    if (-not $signtool) {
        throw "signtool.exe not found. Install Windows 10/11 SDK or pass -SigntoolPath."
    }
}
Write-Host "[sign] signtool: $signtool"

$profileDir = if ($Profile -eq "release") { "release" } else { "debug" }
$cfgName    = if ($Profile -eq "release") { "Release" } else { "Debug" }

$winuiBin = Join-Path $PSScriptRoot "..\apps\rakukan-settings-winui\bin\x64\$cfgName\net8.0-windows10.0.19041.0\win-x64"

$targets = @(
    (Join-Path $BuildDir "$profileDir\rakukan_engine_cpu.dll")
    (Join-Path $BuildDir "$profileDir\rakukan_engine_vulkan.dll")
    (Join-Path $BuildDir "$profileDir\rakukan_engine_cuda.dll")
    (Join-Path $BuildDir "$profileDir\rakukan_tsf.dll")
    (Join-Path $BuildDir "$profileDir\rakukan-tray.exe")
    (Join-Path $BuildDir "$profileDir\rakukan-engine-host.exe")
    (Join-Path $BuildDir "$profileDir\rakukan-dict-builder.exe")
    (Join-Path $winuiBin "rakukan-settings.exe")
    (Join-Path $winuiBin "rakukan-settings.dll")
)

$success = 0
$skipped = 0
$failed  = 0

foreach ($file in $targets) {
    if (-not (Test-Path -LiteralPath $file)) {
        Write-Host "[sign] SKIP (not built): $file" -ForegroundColor DarkGray
        $skipped++
        continue
    }
    Write-Host "[sign] Signing: $file" -ForegroundColor Cyan
    & $signtool sign /fd SHA256 /a /tr $TimestampUrl /td SHA256 $file
    if ($LASTEXITCODE -eq 0) {
        $success++
    } else {
        Write-Warning ("[sign] FAILED: " + $file + " (exit " + $LASTEXITCODE + ")")
        $failed++
    }
}

Write-Host ""
Write-Host "[sign] Signed: $success, Skipped: $skipped, Failed: $failed"
if ($failed -gt 0) { exit 1 }
