# scripts\uninstall-amakana-legacy.ps1
#
# 旧 Amakana IME の登録を完全にクリアするスクリプト。
# rakukan に移行する前に実行する。
#
# 対象:
#   - %LOCALAPPDATA%\Amakana\ 以下の amakana_tsf_*.dll を regsvr32 /u で登録解除
#   - HKCR\CLSID\{C0DDF8B0-...}  (InProcServer32 含む)
#   - HKCU\Software\Microsoft\CTF\Assemblies\0x00000411\{C0DDF8B0-...}
#   - HKLM\SOFTWARE\Microsoft\CTF\TIP\{C0DDF8B0-...}
#   - HKCU\SOFTWARE\Microsoft\CTF\TIP\{C0DDF8B0-...}
#   - %LOCALAPPDATA%\Amakana\ ディレクトリ
#   - %APPDATA%\amakana\ ディレクトリ (keymap.toml / backend.json 等)
#
# 使い方:
#   PowerShell を「管理者として実行」して:
#   .\scripts\uninstall-amakana-legacy.ps1
#
# 注意:
#   管理者権限がないと HKCR / HKLM の削除に失敗することがある。
#   その場合は [手動削除手順] セクションを参照。

$ErrorActionPreference = "Continue"

# ── 管理者チェック ────────────────────────────────────────────────────────────
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole(
    [Security.Principal.WindowsBuiltInRole]::Administrator
)
if (-not $isAdmin) {
    Write-Host ""
    Write-Host "  [ERROR] このスクリプトは管理者権限が必要です。" -ForegroundColor Red
    Write-Host "  PowerShell を「管理者として実行」して再実行してください。" -ForegroundColor Yellow
    Write-Host ""
    exit 1
}

# ── 定数 ─────────────────────────────────────────────────────────────────────
$CLSID        = "{C0DDF8B0-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"
$CLSID_LOWER  = "c0ddf8b0-1f1e-4c2d-a9e3-5f7b8d6e2a4c"
$PROFILE_GUID = "{C0DDF8B1-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"
$LANGID_HEX   = "0x00000411"   # 日本語

$installDir   = "$env:LOCALAPPDATA\Amakana"
$appdataDir   = "$env:APPDATA\amakana"

$ok    = 0
$warn  = 0
$fail  = 0

function Write-Ok($msg)   { Write-Host "  [OK]   $msg" -ForegroundColor Green;  $script:ok++   }
function Write-Warn($msg) { Write-Host "  [SKIP] $msg" -ForegroundColor Yellow; $script:warn++ }
function Write-Fail($msg) { Write-Host "  [FAIL] $msg" -ForegroundColor Red;    $script:fail++ }

Write-Host ""
Write-Host "=== Amakana Legacy Uninstall ===" -ForegroundColor Cyan
Write-Host "  CLSID : $CLSID"
Write-Host "  Dir   : $installDir"
Write-Host ""

# ────────────────────────────────────────────────────────────────────────────
# Step 1: DLL を regsvr32 /u で登録解除（DllUnregisterServer 経由で TSF 登録も削除）
# ────────────────────────────────────────────────────────────────────────────
Write-Host "[Step 1] DLL の登録解除 (regsvr32 /u)" -ForegroundColor Cyan

$dlls = @()
if (Test-Path $installDir) {
    $dlls = @(Get-ChildItem "$installDir\amakana_tsf_*.dll" -ErrorAction SilentlyContinue)
}

# registered.txt に記載された DLL も対象
$regFile = "$installDir\registered.txt"
if (Test-Path $regFile) {
    $regDll = Get-Content $regFile -ErrorAction SilentlyContinue
    if ($regDll -and (Test-Path $regDll)) {
        $dlls += Get-Item $regDll -ErrorAction SilentlyContinue
    }
}

# 重複排除
$dlls = $dlls | Sort-Object FullName -Unique

if ($dlls.Count -eq 0) {
    Write-Warn "登録解除対象の DLL が見つかりませんでした ($installDir)"
} else {
    foreach ($dll in $dlls) {
        Write-Host "  -> $($dll.FullName)"
        $proc = Start-Process regsvr32 -ArgumentList "/s /u `"$($dll.FullName)`"" -Wait -PassThru -NoNewWindow
        if ($proc.ExitCode -eq 0) {
            Write-Ok "登録解除: $($dll.Name)"
        } else {
            Write-Warn "登録解除失敗 (exit $($proc.ExitCode)): $($dll.Name)  ← 手動削除へ進む"
        }
    }
}

Write-Host ""

# ────────────────────────────────────────────────────────────────────────────
# Step 2: HKCR\CLSID の手動削除（regsvr32 が失敗した場合のフォールバック）
# ────────────────────────────────────────────────────────────────────────────
Write-Host "[Step 2] HKCR\CLSID エントリ削除" -ForegroundColor Cyan

foreach ($clsidVariant in @($CLSID, $CLSID_LOWER)) {
    $key = "HKCR:\CLSID\$clsidVariant"
    if (Test-Path $key) {
        try {
            Remove-Item -Path $key -Recurse -Force -ErrorAction Stop
            Write-Ok "削除: $key"
        } catch {
            Write-Fail "削除失敗: $key  ($_)"
        }
    } else {
        Write-Warn "存在しない: $key"
    }
}

Write-Host ""

# ────────────────────────────────────────────────────────────────────────────
# Step 3: TSF プロファイル (CTF\Assemblies) の削除
# ────────────────────────────────────────────────────────────────────────────
Write-Host "[Step 3] CTF Assemblies エントリ削除" -ForegroundColor Cyan

$ctfAssemblyBases = @(
    "HKCU:\Software\Microsoft\CTF\Assemblies\$LANGID_HEX",
    "HKCU:\Software\Microsoft\CTF\Assemblies\0x0411"
)

$deletedAny = $false
foreach ($base in $ctfAssemblyBases) {
    if (-not (Test-Path $base)) { continue }
    # CLSID が含まれるサブキーを探す（大文字小文字混在に対応）
    $subkeys = Get-ChildItem $base -ErrorAction SilentlyContinue
    foreach ($sub in $subkeys) {
        if ($sub.Name -match [regex]::Escape($CLSID_LOWER) -or
            $sub.Name -match [regex]::Escape($CLSID.Trim('{}').ToLower())) {
            try {
                Remove-Item -Path $sub.PSPath -Recurse -Force -ErrorAction Stop
                Write-Ok "削除: $($sub.Name)"
                $deletedAny = $true
            } catch {
                Write-Fail "削除失敗: $($sub.Name)  ($_)"
            }
        }
    }
}
if (-not $deletedAny) {
    Write-Warn "CTF Assemblies に該当エントリなし"
}

Write-Host ""

# ────────────────────────────────────────────────────────────────────────────
# Step 4: TSF TIP エントリの削除
# ────────────────────────────────────────────────────────────────────────────
Write-Host "[Step 4] CTF TIP エントリ削除" -ForegroundColor Cyan

$tipKeys = @(
    "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID",
    "HKCU:\SOFTWARE\Microsoft\CTF\TIP\$CLSID",
    "HKLM:\SOFTWARE\Microsoft\CTF\TIP\$CLSID_LOWER",
    "HKCU:\SOFTWARE\Microsoft\CTF\TIP\$CLSID_LOWER"
)

foreach ($key in $tipKeys) {
    if (Test-Path $key) {
        try {
            Remove-Item -Path $key -Recurse -Force -ErrorAction Stop
            Write-Ok "削除: $key"
        } catch {
            Write-Fail "削除失敗: $key  ($_)"
        }
    } else {
        Write-Warn "存在しない: $key"
    }
}

Write-Host ""

# ────────────────────────────────────────────────────────────────────────────
# Step 5: インストールディレクトリの削除
# ────────────────────────────────────────────────────────────────────────────
Write-Host "[Step 5] インストールディレクトリ削除" -ForegroundColor Cyan

if (Test-Path $installDir) {
    # DLL は使用中のことがあるため個別に確認
    $remainingDlls = @(Get-ChildItem "$installDir\*.dll" -ErrorAction SilentlyContinue)
    foreach ($dll in $remainingDlls) {
        try {
            Remove-Item $dll.FullName -Force -ErrorAction Stop
            Write-Ok "削除: $($dll.Name)"
        } catch {
            Write-Warn "DLL 使用中のためスキップ: $($dll.Name) (再起動後に手動削除)"
        }
    }
    # その他のファイル（ログ、registered.txt 等）
    try {
        Remove-Item "$installDir\*" -Force -Recurse -Exclude "*.dll" -ErrorAction SilentlyContinue
        $remaining = @(Get-ChildItem $installDir -ErrorAction SilentlyContinue)
        if ($remaining.Count -eq 0) {
            Remove-Item $installDir -Force -Recurse -ErrorAction SilentlyContinue
            Write-Ok "削除: $installDir"
        } else {
            Write-Warn "残存ファイルあり（DLL 使用中の可能性）: $installDir"
        }
    } catch {
        Write-Warn "ディレクトリ削除失敗: $installDir ($_)"
    }
} else {
    Write-Warn "存在しない: $installDir"
}

Write-Host ""

# ────────────────────────────────────────────────────────────────────────────
# Step 6: APPDATA 設定ディレクトリの削除
# ────────────────────────────────────────────────────────────────────────────
Write-Host "[Step 6] APPDATA 設定ディレクトリ削除" -ForegroundColor Cyan

if (Test-Path $appdataDir) {
    try {
        Remove-Item $appdataDir -Force -Recurse -ErrorAction Stop
        Write-Ok "削除: $appdataDir"
    } catch {
        Write-Fail "削除失敗: $appdataDir ($_)"
    }
} else {
    Write-Warn "存在しない: $appdataDir"
}

Write-Host ""

# ────────────────────────────────────────────────────────────────────────────
# 結果サマリ
# ────────────────────────────────────────────────────────────────────────────
Write-Host "=== 完了 ===" -ForegroundColor Cyan
Write-Host "  成功: $ok  スキップ: $warn  失敗: $fail"

if ($fail -gt 0) {
    Write-Host ""
    Write-Host "  一部の削除に失敗しました。" -ForegroundColor Yellow
    Write-Host "  以下を手動で確認してください:" -ForegroundColor Yellow
    Write-Host "    regedit → HKEY_CLASSES_ROOT\CLSID\$CLSID"
    Write-Host "    regedit → HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\CTF\TIP\$CLSID"
    Write-Host "    regedit → HKEY_CURRENT_USER\Software\Microsoft\CTF\TIP\$CLSID"
}

Write-Host ""
if ($fail -eq 0) {
    Write-Host "  Amakana の登録が完全にクリアされました。" -ForegroundColor Green
    Write-Host "  rakukan のインストールを実行できます: cargo make install" -ForegroundColor Green
} else {
    Write-Host "  失敗項目を手動で対処後、cargo make install を実行してください。" -ForegroundColor Yellow
}
Write-Host ""
