# rakukan の登録を完全にクリアする（管理者権限が必要）
# キーボードの追加から消えた場合にこれを実行する

$clsid  = "{C0DDF8B0-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"
$clsid2 = "c0ddf8b0-1f1e-4c2d-a9e3-5f7b8d6e2a4c"  # 小文字版

Write-Host "=== rakukan Registry Cleanup ===" -ForegroundColor Cyan

# 1. 登録済み DLL を登録解除
$regFile = "$env:LOCALAPPDATA\rakukan\registered.txt"
if (Test-Path $regFile) {
    $dll = Get-Content $regFile -ErrorAction SilentlyContinue
    if ($dll -and (Test-Path $dll)) {
        Write-Host "Unregistering: $dll"
        regsvr32 /s /u $dll
    }
}

# 2. HKCR\CLSID エントリを削除
foreach ($key in @(
    "HKCR:\CLSID\$clsid",
    "HKCR:\CLSID\$clsid2"
)) {
    if (Test-Path $key) {
        Remove-Item -Path $key -Recurse -Force
        Write-Host "Removed: $key"
    }
}

# 3. HKCU の CTF アセンブリを削除
$ctfPath = "HKCU:\Software\Microsoft\CTF\Assemblies\0x00000411\$clsid"
if (Test-Path $ctfPath) {
    Remove-Item -Path $ctfPath -Recurse -Force
    Write-Host "Removed CTF assembly: $ctfPath"
}

# 4. HKLM の TIP エントリを削除（CategoryMgr が書いたもの）
$tipPath = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System"  # dummy
$catPaths = @(
    "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$clsid",
    "HKCU:\SOFTWARE\Microsoft\CTF\TIP\$clsid"
)
foreach ($p in $catPaths) {
    if (Test-Path $p) {
        Remove-Item -Path $p -Recurse -Force
        Write-Host "Removed TIP: $p"
    }
}

# 5. registered.txt を削除
if (Test-Path $regFile) {
    Remove-Item $regFile -Force
    Write-Host "Removed: $regFile"
}

Write-Host ""
Write-Host "Cleanup complete. Now run: cargo make full-install" -ForegroundColor Green
