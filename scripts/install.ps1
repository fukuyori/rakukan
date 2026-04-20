# scripts\install.ps1 - rakukan インストール (コピー + 登録 + tray 起動)
#
# ★ このスクリプトはビルドを行いません。
#    先に以下を実行してビルド成果物を用意してください:
#      cargo make build-engine
#      cargo make build-tsf
#      cargo make sign         (署名が必要な場合)
#
# ★ 管理者権限 (elevated PowerShell) で実行してください。
#
# 使い方:
#   cargo make install
#   powershell -ExecutionPolicy Bypass -File scripts\install.ps1 [-Profile release|debug]

param(
    [ValidateSet("debug","release")] [string]$Profile = "release",
    [string]$BuildDir = "C:\rb"
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# --- Log file setup ---
$LogFile  = Join-Path (Get-Location).Path "rakukan_install.log"
Start-Transcript -Path $LogFile -Force | Out-Null
Write-Host "Log: $LogFile"

Set-Location (Split-Path $PSScriptRoot)

# ─────────────────────────────────────────────────────────────────────────────
# Helpers
# ─────────────────────────────────────────────────────────────────────────────

function Assert-NotEmpty([string]$name, [string]$value) {
    if ([string]::IsNullOrWhiteSpace($value)) { throw "$name is empty" }
}

function Get-KnownFolderSafe([Environment+SpecialFolder]$folder) {
    try {
        $p = [Environment]::GetFolderPath($folder)
        if ([string]::IsNullOrWhiteSpace($p)) { return $null }
        return $p
    } catch { return $null }
}

function Stop-ProcSilent([string]$name) {
    Get-Process -Name $name -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

function Invoke-Regsvr32Strict([string]$DllPath) {
    Assert-NotEmpty "DllPath" $DllPath
    $regsvr64 = Join-Path $env:WINDIR "System32\regsvr32.exe"
    $regsvr32 = Join-Path $env:WINDIR "SysWOW64\regsvr32.exe"

    $p = Start-Process -FilePath $regsvr64 -ArgumentList "/s `"$DllPath`"" -Wait -PassThru
    if ($p.ExitCode -eq 0) { return "x64" }

    $p2 = Start-Process -FilePath $regsvr32 -ArgumentList "/s `"$DllPath`"" -Wait -PassThru
    if ($p2.ExitCode -eq 0) { return "x86" }

    throw "regsvr32 failed. x64 exit=$($p.ExitCode), x86 exit=$($p2.ExitCode)"
}

function Invoke-Regsvr32UnregisterBestEffort([string]$DllPath) {
    if ([string]::IsNullOrWhiteSpace($DllPath)) { return }
    if (-not (Test-Path -LiteralPath $DllPath)) { return }

    $regsvr64 = Join-Path $env:WINDIR "System32\regsvr32.exe"
    $regsvr32 = Join-Path $env:WINDIR "SysWOW64\regsvr32.exe"

    try { Start-Process -FilePath $regsvr64 -ArgumentList "/s /u `"$DllPath`"" -Wait -PassThru | Out-Null } catch {}
    try { Start-Process -FilePath $regsvr32 -ArgumentList "/s /u `"$DllPath`"" -Wait -PassThru | Out-Null } catch {}
}

function Assert-ComRegistered([string]$DllPath) {
    Assert-NotEmpty "DllPath" $DllPath
    $out = & reg.exe query "HKCR\CLSID" /s /f $DllPath 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $out) {
        throw "COM registration not found in HKCR\CLSID for: $DllPath"
    }
}

function Setup-RunTray([string]$TrayExe) {
    if ([string]::IsNullOrWhiteSpace($TrayExe)) { return }
    if (-not (Test-Path -LiteralPath $TrayExe)) { return }
    $runKey  = "HKCU\Software\Microsoft\Windows\CurrentVersion\Run"
    $trayCmd = "`"$TrayExe`""
    & reg.exe ADD $runKey /v "rakukan-tray" /t REG_SZ /d $trayCmd /f | Out-Null
}

function Promote-TrayIcon() {
    try {
        $trayGuid = "{9C8B5A79-9F7F-4D6A-BF87-2E50B5D7A2C1}"
        $key = "HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\TrayNotify\NotifyIconSettings\$trayGuid"
        & reg.exe ADD $key /v "IsPromoted" /t REG_DWORD /d 1 /f | Out-Null
    } catch {}
}

# ─────────────────────────────────────────────────────────────────────────────
# Admin / folder setup
# ─────────────────────────────────────────────────────────────────────────────

$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin) { throw "Administrator privileges are required." }

$local = Get-KnownFolderSafe ([Environment+SpecialFolder]::LocalApplicationData)
if (-not $local) { $local = $env:LOCALAPPDATA }
if (-not $local) { $local = Join-Path $HOME "AppData\Local" }
Assert-NotEmpty "LocalAppData" $local

$installDir  = Join-Path $local "rakukan"
$regFile     = Join-Path $installDir "registered.txt"
$trayExe     = Join-Path $installDir "rakukan-tray.exe"
$settingsDir = Join-Path $installDir "settings-ui"

$profileDir     = if ($Profile -eq "release") { "release" } else { "debug" }
$cfgName        = if ($Profile -eq "release") { "Release" } else { "Debug" }
$srcDll         = Join-Path $BuildDir "$profileDir\rakukan_tsf.dll"
$srcTray        = Join-Path $BuildDir "$profileDir\rakukan-tray.exe"
$srcHost        = Join-Path $BuildDir "$profileDir\rakukan-engine-host.exe"
$srcBuilder     = Join-Path $BuildDir "$profileDir\rakukan-dict-builder.exe"
$srcSettingsDir = Join-Path $PSScriptRoot "..\apps\rakukan-settings-winui\bin\x64\$cfgName\net8.0-windows10.0.19041.0\win-x64"
$engineDlls = @("cpu","vulkan","cuda") | ForEach-Object {
    $p = Join-Path $BuildDir "$profileDir\rakukan_engine_$_.dll"
    if (Test-Path $p) { $p } else { $null }
} | Where-Object { $_ }

# ─────────────────────────────────────────────────────────────────────────────
# Pre-flight: required build outputs
# ─────────────────────────────────────────────────────────────────────────────

if (-not (Test-Path -LiteralPath $srcDll)) {
    throw "[install] $srcDll not found. Run 'cargo make build-tsf' first."
}

if ($engineDlls.Count -eq 0) {
    # engine DLL がビルド出力になくても、既存インストール先にあれば使いまわす。
    # どちらも無ければ Activate() が失敗するのでエラー停止。
    $existingCpuDll = Join-Path $installDir "rakukan_engine_cpu.dll"
    if (-not (Test-Path -LiteralPath $existingCpuDll)) {
        throw "[install] rakukan_engine_cpu.dll not found in $BuildDir\$profileDir\ nor in $installDir\. Run 'cargo make build-engine' first."
    }
    Write-Host "[install] [WARN] Engine DLL not rebuilt; reusing existing: $existingCpuDll"
}

# ─────────────────────────────────────────────────────────────────────────────
# [1/5] Copy to LocalAppData
# ─────────────────────────────────────────────────────────────────────────────

Write-Host "[1/5] Installing to $installDir ..."
New-Item -ItemType Directory -Force -Path $installDir | Out-Null

# Unregister old DLL to release engine DLL file locks
if (Test-Path -LiteralPath $regFile) {
    $oldDllEarly = Get-Content -LiteralPath $regFile -ErrorAction SilentlyContinue
    if ($oldDllEarly) { Invoke-Regsvr32UnregisterBestEffort $oldDllEarly }
}
Stop-ProcSilent "rakukan-tray"
Stop-ProcSilent "rakukan-engine-host"
Stop-ProcSilent "rakukan-settings"
Stop-ProcSilent "ctfmon"
Stop-ProcSilent "TextInputHost"
Start-Sleep -Milliseconds 1200

# TSF DLL
$dst = Join-Path $installDir "rakukan_tsf.dll"
Copy-Item -LiteralPath $srcDll -Destination $dst -Force
Write-Host "  -> $dst"

# 古いタイムスタンプ付き DLL を削除
Get-ChildItem -Path $installDir -Filter "rakukan_tsf_????????_??????.dll" -ErrorAction SilentlyContinue |
    ForEach-Object {
        try {
            Invoke-Regsvr32UnregisterBestEffort $_.FullName
            Remove-Item -LiteralPath $_.FullName -Force
            Write-Host "  Removed old: $($_.Name)"
        } catch {
            Write-Host "  Could not remove: $($_.Name) (in use?)"
        }
    }

# Engine DLLs
foreach ($engineDll in $engineDlls) {
    $dllName = [IO.Path]::GetFileName($engineDll)
    $engineDst = Join-Path $installDir $dllName
    Copy-Item -LiteralPath $engineDll -Destination $engineDst -Force
    Write-Host "  -> $engineDst"
}

# tray.exe
if (Test-Path -LiteralPath $srcTray) {
    try {
        Copy-Item -LiteralPath $srcTray -Destination $trayExe -Force
    } catch {
        $tmp = "$trayExe.new"
        Copy-Item -LiteralPath $srcTray -Destination $tmp -Force
        Move-Item -LiteralPath $tmp -Destination $trayExe -Force
    }
    Write-Host "  -> $trayExe"
}

# engine-host.exe
if (Test-Path -LiteralPath $srcHost) {
    $hostExe = Join-Path $installDir "rakukan-engine-host.exe"
    try {
        Copy-Item -LiteralPath $srcHost -Destination $hostExe -Force
    } catch {
        $tmp = "$hostExe.new"
        Copy-Item -LiteralPath $srcHost -Destination $tmp -Force
        Move-Item -LiteralPath $tmp -Destination $hostExe -Force
    }
    Write-Host "  -> $hostExe"
}

# dict-builder.exe
if (Test-Path -LiteralPath $srcBuilder) {
    $builderDest = Join-Path $installDir "rakukan-dict-builder.exe"
    Copy-Item -LiteralPath $srcBuilder -Destination $builderDest -Force
    Write-Host "  -> $builderDest"
}

# WinUI settings UI (folder)
if (Test-Path -LiteralPath $srcSettingsDir) {
    Remove-Item -LiteralPath $settingsDir -Recurse -Force -ErrorAction SilentlyContinue
    New-Item -ItemType Directory -Force -Path $settingsDir | Out-Null
    Copy-Item -Path (Join-Path $srcSettingsDir "*") -Destination $settingsDir -Recurse -Force
    Write-Host "  -> $settingsDir"
}

# config.toml (first install only)
$configDir  = Join-Path $env:APPDATA "rakukan"
$configDest = Join-Path $configDir "config.toml"
$configSrc  = Join-Path $PSScriptRoot "..\config\config.toml"
New-Item -ItemType Directory -Force -Path $configDir | Out-Null
if (-not (Test-Path -LiteralPath $configDest)) {
    if (Test-Path -LiteralPath $configSrc) {
        Copy-Item -LiteralPath $configSrc -Destination $configDest
        Write-Host "  -> $configDest"
    }
} else {
    Write-Host "  -> config.toml already exists, skipping"
}

# ─────────────────────────────────────────────────────────────────────────────
# [2/5] Unregister old / Register new TSF DLL
# ─────────────────────────────────────────────────────────────────────────────

Write-Host "[2/5] Unregistering old version..."
if (Test-Path -LiteralPath $regFile) {
    $oldDll = Get-Content -LiteralPath $regFile -ErrorAction SilentlyContinue
    if ($oldDll) { Invoke-Regsvr32UnregisterBestEffort $oldDll }
}
Stop-ProcSilent "ctfmon"
Stop-ProcSilent "TextInputHost"
Start-Sleep -Milliseconds 400

Write-Host "[3/5] Registering new TSF DLL..."
$arch = Invoke-Regsvr32Strict $dst
Assert-ComRegistered $dst
Write-Host "  Registered ($arch): $dst"
$dst | Set-Content -LiteralPath $regFile

# HKCU TIP mirror (Windows 11 Settings 表示対応)
try {
    $hit = (reg.exe query "HKCR\CLSID" /s /f $dst 2>$null |
        Select-String -Pattern 'HKEY_CLASSES_ROOT\\CLSID\\\{[0-9A-Fa-f-]+\}\\InProcServer32' |
        Select-Object -First 1)
    if ($hit) {
        $clsid = ($hit.ToString() -replace '.*\\CLSID\\(\{[0-9A-Fa-f-]+\})\\InProcServer32','$1')
        reg.exe COPY "HKLM\Software\Microsoft\CTF\TIP\$clsid" "HKCU\Software\Microsoft\CTF\TIP\$clsid" /s /f | Out-Null
    }
} catch {}

Start-Process ctfmon | Out-Null

# ─────────────────────────────────────────────────────────────────────────────
# [4/5] Dictionary & LLM model setup
# ─────────────────────────────────────────────────────────────────────────────

Write-Host "[4/5] Setting up dictionaries..."
$dictDir   = Join-Path $env:LOCALAPPDATA "rakukan\dict"
New-Item -ItemType Directory -Force -Path $dictDir | Out-Null
$forceDict = $env:RAKUKAN_FORCE_DICT -eq "1"

# mozc dictionary (Apache 2.0)
Write-Host "  [4a] mozc dictionary (Apache 2.0)..."
$mozcDictOut    = Join-Path $dictDir "rakukan.dict"
$mozcTsvDir     = Join-Path $dictDir "mozc_tsv"
$dictBuilderExe = Join-Path $installDir "rakukan-dict-builder.exe"

$mozcTsvFiles = @(
    "dictionary00.txt"
    "dictionary01.txt"
    "dictionary02.txt"
    "dictionary03.txt"
    "dictionary04.txt"
    "dictionary05.txt"
    "dictionary06.txt"
    "dictionary07.txt"
    "dictionary08.txt"
    "dictionary09.txt"
)
$mozcBaseUrl = "https://raw.githubusercontent.com/google/mozc/refs/heads/master/src/data/dictionary_oss"

if ((Test-Path -LiteralPath $mozcDictOut) -and (-not $forceDict)) {
    $sizeMB = [math]::Round((Get-Item $mozcDictOut).Length / 1048576, 1)
    Write-Host ("  -> rakukan.dict already built (" + $sizeMB + " MB), skipping.")
    Write-Host "     (To rebuild, set RAKUKAN_FORCE_DICT=1 and re-run)"
} elseif (-not (Test-Path -LiteralPath $dictBuilderExe)) {
    Write-Host "  [WARNING] rakukan-dict-builder.exe not found, skipping mozc dict."
} else {
    New-Item -ItemType Directory -Force -Path $mozcTsvDir | Out-Null
    $downloadedTsvs = [System.Collections.Generic.List[string]]::new()
    $ProgressPreference = "SilentlyContinue"

    foreach ($tsv in $mozcTsvFiles) {
        $tsvPath = Join-Path $mozcTsvDir $tsv
        if ((-not (Test-Path -LiteralPath $tsvPath)) -or $forceDict) {
            try {
                $url     = $mozcBaseUrl + "/" + $tsv
                $tmpPath = $tsvPath + ".tmp"
                Invoke-WebRequest -Uri $url -OutFile $tmpPath -UseBasicParsing -TimeoutSec 120
                Move-Item -LiteralPath $tmpPath -Destination $tsvPath -Force
                Write-Host ("    Downloaded: " + $tsv)
            } catch {
                $tmpPath = $tsvPath + ".tmp"
                if (Test-Path -LiteralPath $tmpPath) { Remove-Item -LiteralPath $tmpPath -Force -ErrorAction SilentlyContinue }
                Write-Host ("    [WARNING] Failed: " + $tsv + " - " + $_)
            }
        }
        if (Test-Path -LiteralPath $tsvPath) { $downloadedTsvs.Add($tsvPath) }
    }

    if ($downloadedTsvs.Count -eq 0) {
        Write-Host "  [WARNING] No mozc TSV files downloaded. rakukan.dict will not be built."
    } else {
        # symbol.tsv (Apache 2.0)
        $symbolTsvPath = Join-Path $mozcTsvDir "symbol.tsv"
        $symbolUrl     = "https://raw.githubusercontent.com/google/mozc/refs/heads/master/src/data/symbol/symbol.tsv"
        if ((-not (Test-Path -LiteralPath $symbolTsvPath)) -or $forceDict) {
            try {
                $tmpPath = $symbolTsvPath + ".tmp"
                Invoke-WebRequest -Uri $symbolUrl -OutFile $tmpPath -UseBasicParsing -TimeoutSec 60
                Move-Item -LiteralPath $tmpPath -Destination $symbolTsvPath -Force
                Write-Host "    Downloaded: symbol.tsv"
            } catch {
                $tmpPath = $symbolTsvPath + ".tmp"
                if (Test-Path -LiteralPath $tmpPath) { Remove-Item -LiteralPath $tmpPath -Force -ErrorAction SilentlyContinue }
                Write-Host ("    [WARNING] Failed to download symbol.tsv: " + $_)
            }
        }

        Write-Host ("  Building rakukan.dict from " + $downloadedTsvs.Count + " TSV files + symbol.tsv...")
        $inputArgs = @()
        foreach ($f in $downloadedTsvs) {
            $inputArgs += "--input"
            $inputArgs += $f
        }
        if (Test-Path -LiteralPath $symbolTsvPath) {
            $inputArgs += "--symbol"
            $inputArgs += $symbolTsvPath
        }
        $inputArgs += "--output"
        $inputArgs += $mozcDictOut
        try {
            & $dictBuilderExe @inputArgs
            if ($LASTEXITCODE -eq 0) {
                $sizeMB = [math]::Round((Get-Item $mozcDictOut).Length / 1048576, 1)
                Write-Host ("  -> " + $mozcDictOut + " (" + $sizeMB + " MB)")
                Remove-Item -LiteralPath $mozcTsvDir -Recurse -Force -ErrorAction SilentlyContinue
            } else {
                Write-Host ("  [WARNING] rakukan-dict-builder failed (exit " + $LASTEXITCODE + ")")
            }
        } catch {
            Write-Host ("  [WARNING] rakukan-dict-builder error: " + $_)
        }
    }
}

# LLM model pre-download
Write-Host ""
Write-Host "  [4b] LLM model pre-download..."

$userConfigToml = Join-Path $env:APPDATA "rakukan\config.toml"
$modelVariant   = $null
if (Test-Path -LiteralPath $userConfigToml) {
    foreach ($line in (Get-Content $userConfigToml -Encoding UTF8)) {
        $line = $line.Trim()
        if ($line.StartsWith('#')) { continue }
        if ($line -match '^model_variant\s*=\s*"([^"]+)"') {
            $modelVariant = $Matches[1]
            break
        }
    }
}

if (-not $modelVariant) {
    Write-Host "  model_variant not set in config.toml - skipping model download."
} else {
    # 注: この表は crates/rakukan-engine/models.toml と同期が必要
    # (scripts/refresh-models.ps1 で最新の variant を検出できる)
    $modelMap = @{
        "jinen-v1-small-q5"   = @{ repo = "togatogah/jinen-v1-small.gguf";   file = "jinen-v1-small-Q5_K_M.gguf";   tok = "tokenizer.json" }
        "jinen-v1-xsmall-q5"  = @{ repo = "togatogah/jinen-v1-xsmall.gguf";  file = "jinen-v1-xsmall-Q5_K_M.gguf";  tok = "tokenizer.json" }
        "jinen-v1-small-f16"  = @{ repo = "togatogah/jinen-v1-small.gguf";   file = "jinen-v1-small-f16.gguf";      tok = "tokenizer.json" }
        "jinen-v1-xsmall-f16" = @{ repo = "togatogah/jinen-v1-xsmall.gguf";  file = "jinen-v1-xsmall-f16.gguf";     tok = "tokenizer.json" }
    }
    if (-not $modelMap.ContainsKey($modelVariant)) {
        Write-Host ("  Unknown model_variant: " + $modelVariant + " - skipping.")
    } else {
        $m        = $modelMap[$modelVariant]
        $repoSlug = $m.repo -replace '/', '--'
        $cacheDir = Join-Path $env:USERPROFILE ".cache\huggingface\hub\models--$repoSlug\snapshots\main"
        New-Item -ItemType Directory -Force -Path $cacheDir | Out-Null

        foreach ($fname in @($m.file, $m.tok)) {
            $dest = Join-Path $cacheDir $fname
            if ((Test-Path -LiteralPath $dest) -and (Get-Item $dest).Length -gt 0) {
                $sizeMB = [math]::Round((Get-Item $dest).Length / 1048576, 1)
                Write-Host ("  -> " + $fname + " already cached (" + $sizeMB + " MB), skipping.")
            } else {
                $url  = "https://huggingface.co/" + $m.repo + "/resolve/main/" + $fname
                $tmp  = $dest + ".tmp"
                Write-Host ("  Downloading " + $fname + " ...")
                try {
                    $ProgressPreference = "SilentlyContinue"
                    Invoke-WebRequest -Uri $url -OutFile $tmp -UseBasicParsing -TimeoutSec 3600
                    Move-Item -LiteralPath $tmp -Destination $dest -Force
                    $sizeMB = [math]::Round((Get-Item $dest).Length / 1048576, 1)
                    Write-Host ("  -> " + $dest + " (" + $sizeMB + " MB)")
                } catch {
                    if (Test-Path -LiteralPath $tmp) { Remove-Item $tmp -Force -ErrorAction SilentlyContinue }
                    Write-Host ("  [WARNING] Failed to download " + $fname + ": " + $_)
                }
            }
        }
    }
}

# ─────────────────────────────────────────────────────────────────────────────
# [5/5] Tray
# ─────────────────────────────────────────────────────────────────────────────

Write-Host "[5/5] Setting up tray icon..."
if (Test-Path -LiteralPath $trayExe) {
    Stop-ProcSilent "rakukan-tray"
    Setup-RunTray $trayExe
    Promote-TrayIcon
    Start-Process -FilePath $trayExe | Out-Null
    Write-Host "  Tray started."
}

Write-Host ""
Write-Host "Installed: $dst"
Write-Host "Switch to rakukan in the language bar."

Stop-Transcript | Out-Null
Write-Host "Log saved: $LogFile"
