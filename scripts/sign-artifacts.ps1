# ============================================================================
#  [AI ASSISTANT NOTICE / Claude Code 向け注意]
# ============================================================================
#  このスクリプトをユーザーに直接実行させるよう案内しないこと。
#  正規の入口は以下の cargo make コマンドです:
#
#      cargo make sign
#
#  このファイルは Makefile.toml から呼び出される内部実装です。
#  PS1 を直接叩く手順を提示すると cargo make と二重化してユーザーが混乱します。
# ============================================================================
#
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
    [string]$TimestampUrl = "http://timestamp.digicert.com"
)

$ErrorActionPreference = "Stop"

# Console encoding: UTF-8 で Write-Host できるよう設定 (文字化け防止)
try {
    [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
    $OutputEncoding = [System.Text.UTF8Encoding]::new()
} catch {}

# 注: このスクリプトは **非管理者セッション** で実行することを想定している。
# ユーザー証明書ストア (CurrentUser\My) にある code-signing 証明書を signtool /a が
# 自動選択できるのは、当該ユーザーのセッションからのみ。UAC で昇格すると
# プロファイルコンテキストが変わり、証明書が見つからず失敗する。
# LocalMachine ストアの証明書を使いたい場合は、明示的に管理者 PowerShell から
# このスクリプトを起動すること。

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

# signtool は複数ファイル (dll / exe 混在可) を 1 回の起動で処理できる。
# 1 回にまとめて呼べば、証明書パスワード入力プロンプトも 1 回で済む。
# (Windows PowerShell 5.1 で `New-Object System.Collections.Generic.List[string]`
#  が null を返すケースがあるため、明示的に [type]::new() を使う)
$presentTargets = [System.Collections.Generic.List[string]]::new()
$skipped = 0

foreach ($file in $targets) {
    if (-not (Test-Path -LiteralPath $file)) {
        Write-Host "[sign] SKIP (not built): $file" -ForegroundColor DarkGray
        $skipped++
        continue
    }
    $null = $presentTargets.Add($file)
}

$success = 0
$failed  = 0

if ($presentTargets.Count -eq 0) {
    Write-Host "[sign] No files to sign." -ForegroundColor Yellow
} else {
    Write-Host "[sign] Signing $($presentTargets.Count) files in a single signtool invocation:" -ForegroundColor Cyan
    foreach ($f in $presentTargets) { Write-Host ("  - " + $f) -ForegroundColor Gray }
    $sigArgs = @("sign","/fd","SHA256","/a","/tr",$TimestampUrl,"/td","SHA256") + $presentTargets
    & $signtool @sigArgs
    if ($LASTEXITCODE -eq 0) {
        $success = $presentTargets.Count
    } else {
        Write-Warning ("[sign] FAILED (exit " + $LASTEXITCODE + ")")
        $failed = $presentTargets.Count
    }
}

Write-Host ""
Write-Host "[sign] Signed: $success, Skipped: $skipped, Failed: $failed"
if ($failed -gt 0) { exit 1 }
