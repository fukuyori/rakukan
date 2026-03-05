# scripts\check-amakana-registry.ps1
# Amakana の登録残存を確認する（読み取り専用・削除はしない）
# 管理者権限不要

$CLSID       = "{C0DDF8B0-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"
$CLSID_LOWER = "c0ddf8b0-1f1e-4c2d-a9e3-5f7b8d6e2a4c"
$installDir  = "$env:LOCALAPPDATA\Amakana"
$appdataDir  = "$env:APPDATA\amakana"

$found = 0

function Check-Key($path) {
    if (Test-Path $path) {
        Write-Host "  [残存] $path" -ForegroundColor Red
        $script:found++
    } else {
        Write-Host "  [なし] $path" -ForegroundColor Green
    }
}

function Check-Dir($path) {
    if (Test-Path $path) {
        $items = @(Get-ChildItem $path -ErrorAction SilentlyContinue)
        Write-Host "  [残存] $path  ($($items.Count) items)" -ForegroundColor Red
        $items | ForEach-Object { Write-Host "           $($_.Name)" -ForegroundColor DarkRed }
        $script:found++
    } else {
        Write-Host "  [なし] $path" -ForegroundColor Green
    }
}

Write-Host ""
Write-Host "=== Amakana Registry Check ===" -ForegroundColor Cyan
Write-Host "  CLSID: $CLSID"
Write-Host ""

Write-Host "[ HKCR CLSID ]"
Check-Key "HKCR:\CLSID\$CLSID"
Check-Key "HKCR:\CLSID\$CLSID_LOWER"

Write-Host ""
Write-Host "[ CTF TIP ]"
Check-Key "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID"
Check-Key "HKCU:\SOFTWARE\Microsoft\CTF\TIP\$CLSID"
Check-Key "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID_LOWER"
Check-Key "HKCU:\SOFTWARE\Microsoft\CTF\TIP\$CLSID_LOWER"

Write-Host ""
Write-Host "[ CTF Assemblies ]"
$assemblyBases = @(
    "HKCU:\Software\Microsoft\CTF\Assemblies\0x00000411",
    "HKCU:\Software\Microsoft\CTF\Assemblies\0x0411"
)
$assemblyFound = $false
foreach ($base in $assemblyBases) {
    if (Test-Path $base) {
        $subs = Get-ChildItem $base -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -match $CLSID_LOWER }
        foreach ($s in $subs) {
            Write-Host "  [残存] $($s.Name)" -ForegroundColor Red
            $found++
            $assemblyFound = $true
        }
    }
}
if (-not $assemblyFound) {
    Write-Host "  [なし] CTF Assemblies に Amakana エントリなし" -ForegroundColor Green
}

Write-Host ""
Write-Host "[ ファイル / ディレクトリ ]"
Check-Dir $installDir
Check-Dir $appdataDir

Write-Host ""
Write-Host "=== 結果 ===" -ForegroundColor Cyan
if ($found -eq 0) {
    Write-Host "  クリーンです。Amakana の残存エントリはありません。" -ForegroundColor Green
} else {
    Write-Host "  $found 件の残存エントリが見つかりました。" -ForegroundColor Red
    Write-Host "  uninstall-amakana-legacy.ps1 を管理者として実行してください。" -ForegroundColor Yellow
}
Write-Host ""
