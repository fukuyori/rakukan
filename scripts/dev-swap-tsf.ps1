# ============================================================================
#  [AI ASSISTANT NOTICE / Claude Code 向け注意]
# ============================================================================
#  このスクリプトをユーザーに直接実行させるよう案内しないこと。
#  正規の入口は以下の cargo make コマンドです (#65 の故障試験専用):
#
#      cargo make install-tsf-fault-test
#      cargo make install-tsf-normal-restore
#
#  このファイルは Makefile.toml から呼び出される内部実装です。
# ============================================================================
#
# scripts\dev-swap-tsf.ps1 - 開発専用: インストール済みの TSF DLL だけを別のビルドに差し替えて登録し直す
#
# インストーラーでは別ビルドの DLL 1 本だけを入れ替えられないため、この経路を残す
# （docs/Installer_Redesign_Plan.md の 4.3）。インストーラーで入れた %ProgramFiles%\rakukan が前提。
# DLL をコピー → SHA256 の一致を確かめる → 登録し直す。トレイ・ホスト・設定アプリは止めない。

param(
    [ValidateSet("debug","release")] [string]$Profile = "release",
    [Parameter(Mandatory = $true)] [string]$BuildDir,
    [switch]$NoElevate      # 自動昇格をスキップ (内部利用)
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Console encoding: UTF-8 (Windows PowerShell 5.1 で日本語出力の文字化けを防ぐ)
try {
    [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
    $OutputEncoding = [System.Text.UTF8Encoding]::new()
} catch {}

# --- Auto-elevate to Administrator ---
# TSF DLL 登録 (regsvr32 → HKLM 書き込み)、%ProgramFiles% への書き込み、ctfmon 等の停止のため
# 管理者権限が必須。非管理者セッションから呼ばれた場合は UAC で昇格して再実行する。
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin -and -not $NoElevate) {
    Write-Host "[dev-swap-tsf] Requesting administrator privileges (UAC)..." -ForegroundColor Yellow
    # 昇格後は作業フォルダが変わるので、相対の BuildDir はここで絶対パスにする
    $absBuildDir = [System.IO.Path]::GetFullPath((Join-Path (Get-Location).Path $BuildDir))
    $argList = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "`"$PSCommandPath`"", "-NoElevate",
                 "-Profile", $Profile, "-BuildDir", "`"$absBuildDir`"")
    # Note: Start-Process -Wait は UAC 昇格 (-Verb RunAs) と組み合わせた場合
    # Windows PowerShell 5.1 で正しく待機しないことがあるため、PassThru で
    # Process オブジェクトを受け取り WaitForExit() で明示的に待機する。
    try {
        $proc = Start-Process -FilePath "powershell.exe" -Verb RunAs -ArgumentList $argList -PassThru
        if (-not $proc) {
            Write-Error "[dev-swap-tsf] Failed to launch elevated PowerShell"
            exit 1
        }
        $proc.WaitForExit()
        exit $proc.ExitCode
    } catch {
        Write-Error "[dev-swap-tsf] Elevation failed: $_"
        exit 1
    }
}

# --- Log file setup ---
$LogFile = Join-Path (Get-Location).Path ("rakukan_install_tsf_{0:yyyyMMdd_HHmmss}_{1}.log" -f (Get-Date), $PID)
Start-Transcript -Path $LogFile -Force | Out-Null
Write-Host "Log: $LogFile"

Set-Location (Split-Path $PSScriptRoot)

$TsfClsid = "{C0DDF8B0-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"

function Stop-ProcSilent([string]$name) {
    Get-Process -Name $name -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

# DLL をロック解放待ちのリトライ付きでコピーする。
#
# 直前に kill した ctfmon / TextInputHost がファイルを解放するまで
# 1.2 秒では足りないことがあり、その場合は同じコマンドをもう一度実行するだけで
# 通る（2026-09-06 実機）。1 回目で通るように、IOException なら少し待って
# 数回やり直す。最後まで失敗したら例外をそのまま投げ、ロックの案内へ落とす。
function Copy-DllWithRetry([string]$Source, [string]$Destination, [int]$Attempts = 5, [int]$WaitMs = 1000) {
    for ($i = 1; $i -le $Attempts; $i++) {
        try {
            Copy-Item -LiteralPath $Source -Destination $Destination -Force
            return
        } catch [System.IO.IOException] {
            if ($i -ge $Attempts) { throw }
            Write-Host "  (locked) $([IO.Path]::GetFileName($Destination)) - retry $i/$($Attempts - 1) in ${WaitMs}ms" -ForegroundColor DarkGray
            Start-Sleep -Milliseconds $WaitMs
        }
    }
}

# Get-FileHash は使わない。管理者の PowerShell 7 から cargo make 経由で起動すると、昇格し直さずに
# PowerShell 7 のモジュールの場所を引き継ぎ、5.1 の Get-FileHash（モジュールのスクリプトで定義）が見つからない
function Get-Sha256([string]$Path) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($Path)
    try { return [System.BitConverter]::ToString($sha.ComputeHash($stream)) -replace '-', '' }
    finally { $stream.Dispose(); $sha.Dispose() }
}

function Invoke-Regsvr32([string]$Arguments) {
    $regsvr64 = Join-Path $env:WINDIR "System32\regsvr32.exe"
    $p = Start-Process -FilePath $regsvr64 -ArgumentList $Arguments -Wait -PassThru
    return $p.ExitCode
}

# 登録の状態: CLSID の InprocServer32 が指す DLL（無ければ $null）
function Get-RegisteredTsfDll() {
    $key = "Registry::HKEY_LOCAL_MACHINE\SOFTWARE\Classes\CLSID\$TsfClsid\InprocServer32"
    $value = (Get-ItemProperty -LiteralPath $key -ErrorAction SilentlyContinue).'(default)'
    if ([string]::IsNullOrWhiteSpace($value)) { return $null }
    return $value
}

# ─────────────────────────────────────────────────────────────────────────────
# Pre-flight
# ─────────────────────────────────────────────────────────────────────────────

$installDir = Join-Path $env:ProgramFiles "rakukan"
$dst        = Join-Path $installDir "rakukan_tsf.dll"
$profileDir = if ($Profile -eq "release") { "release" } else { "debug" }
$srcDll     = Join-Path ([System.IO.Path]::GetFullPath($BuildDir)) "$profileDir\rakukan_tsf.dll"

if (-not (Test-Path -LiteralPath $srcDll)) {
    throw "[dev-swap-tsf] $srcDll not found. Build it first (cargo make build-tsf-fault-test / build-tsf-normal-restore)."
}
$registeredDll = Get-RegisteredTsfDll
if (-not (Test-Path -LiteralPath $dst) -or -not $registeredDll) {
    throw "[dev-swap-tsf] requires an existing registered installation: $dst (run sudo cargo make install and restart first)"
}
if (-not $registeredDll.Equals($dst, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "[dev-swap-tsf] registered DLL differs from install destination: $registeredDll"
}
$srcHash = Get-Sha256 $srcDll

# ─────────────────────────────────────────────────────────────────────────────
# [1/2] Copy
# ─────────────────────────────────────────────────────────────────────────────

Write-Host "[1/2] Copying $srcDll -> $dst ..."
try {
    # 解除の結果は使わない（解除後に DLL が外れるのを待つため）。成否は登録の後に状態で確かめる
    $null = Invoke-Regsvr32 "/s /u `"$dst`""
    Stop-ProcSilent "ctfmon"
    Stop-ProcSilent "TextInputHost"
    Start-Sleep -Milliseconds 1200
    Copy-DllWithRetry -Source $srcDll -Destination $dst
} catch [System.IO.IOException] {
    Write-Host ""
    Write-Host "[dev-swap-tsf] ファイルがロックされていてコピーできません:" -ForegroundColor Red
    Write-Host "  $($_.Exception.Message)" -ForegroundColor Red
    Write-Host ""
    Write-Host "  対処: 一旦サインアウト→再ログオンしてから、同じ install-tsf-* タスクを再実行してください。" -ForegroundColor Yellow
    Write-Host "        (TSF DLL は再ログオンで解放されます)" -ForegroundColor Yellow
    # 解除したままにしない
    $null = Invoke-Regsvr32 "/s `"$dst`""
    Start-Process ctfmon | Out-Null
    Stop-Transcript | Out-Null
    exit 1
}

$dstHash = Get-Sha256 $dst
Write-Host "  TSF SHA256 source:    $srcHash"
Write-Host "  TSF SHA256 installed: $dstHash"
if ($srcHash -ne $dstHash) {
    throw "[dev-swap-tsf] TSF DLL SHA256 mismatch after copy"
}

# ─────────────────────────────────────────────────────────────────────────────
# [2/2] Register
# ─────────────────────────────────────────────────────────────────────────────

Write-Host "[2/2] Registering $dst ..."
$code = Invoke-Regsvr32 "/s `"$dst`""
# regsvr32 の終了コード 0 は登録の成功を示さないので、登録の状態で確かめる
$registeredDll = Get-RegisteredTsfDll
$tipExists = Test-Path -LiteralPath "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$TsfClsid"
if (-not $registeredDll -or -not $registeredDll.Equals($dst, [System.StringComparison]::OrdinalIgnoreCase) -or -not $tipExists) {
    throw "[dev-swap-tsf] registration check failed (regsvr32 exit=$code, InprocServer32=$registeredDll, TIP=$tipExists)"
}
Write-Host "  Registered: $dst (regsvr32 exit=$code)"

# HKCU への TIP の写し（Windows 11 の設定の表示のため。register-tip.ps1 と同じく rakukan の CLSID の分だけ）
& reg.exe COPY "HKLM\Software\Microsoft\CTF\TIP\$TsfClsid" "HKCU\Software\Microsoft\CTF\TIP\$TsfClsid" /s /f | Out-Null
if ($LASTEXITCODE -ne 0) { Write-Warning "reg COPY to HKCU failed (exit $LASTEXITCODE)" }

Start-Process ctfmon | Out-Null

Write-Host "Installed TSF DLL only: $dst"
Write-Host "TSF SHA256: $dstHash"
Stop-Transcript | Out-Null
Write-Host "Log saved: $LogFile"
exit 0
