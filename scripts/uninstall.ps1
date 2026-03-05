# scripts\uninstall.ps1 — rakukan アンインストール
# 必ず「管理者として実行」した PowerShell から呼び出すこと

$ErrorActionPreference = "Continue"

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) {
    Write-Host "ERROR: Administrator privileges are required." -ForegroundColor Red; exit 1
}

$installDir = "$env:LOCALAPPDATA\rakukan"
$regFile    = "$installDir\registered.txt"

# tray を停止 & 自動起動を解除（管理者でもHKCUは操作可）
Stop-Process -Name rakukan-tray -Force -ErrorAction SilentlyContinue
$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
try { Remove-ItemProperty -Path $runKey -Name "rakukan-tray" -ErrorAction SilentlyContinue } catch { }

if (Test-Path $regFile) {
    $dll = Get-Content $regFile -ErrorAction SilentlyContinue
    if ($dll -and (Test-Path $dll)) {
        $proc = Start-Process regsvr32 -ArgumentList "/s /u `"$dll`"" -Wait -PassThru
        Write-Host "Unregistered: $dll (exit $($proc.ExitCode))" -ForegroundColor Yellow
    }
    Remove-Item $regFile -Force
} else {
    Write-Host "Nothing to uninstall." -ForegroundColor Yellow
}
