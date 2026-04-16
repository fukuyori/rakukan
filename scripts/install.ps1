# scripts\install.ps1 - rakukan installer (robust)
# Run from an elevated (Administrator) PowerShell:
#   cargo make install

param(
    [ValidateSet("debug","release")] [string]$Profile = "release",
    [switch]$SkipEngine,  # engine DLL ????????????tsf / tray / dict-builder ???????
    [switch]$BuildOnly    # build only, skip install
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# --- Log file setup ---
# stdout + stderr are both captured via Start-Transcript.
# Each run creates a timestamped log in %TEMP%.
$LogFile  = Join-Path (Get-Location).Path "rakukan_install.log"
Start-Transcript -Path $LogFile -Force | Out-Null
$global:TRANSCRIPT_STARTED = $true   # prevents build-engine.ps1 from starting its own transcript
Write-Host "Log: $LogFile"

Set-Location (Split-Path $PSScriptRoot)

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

    $regsvr64 = Join-Path $env:WINDIR "System32\regsvr32.exe"   # 64-bit
    $regsvr32 = Join-Path $env:WINDIR "SysWOW64\regsvr32.exe"  # 32-bit

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
    # Best-effort: ask Windows to show the tray icon in the main area (not overflow).
    # This is a per-user setting keyed by the NOTIFYICON GUID (must match rakukan-tray TRAY_GUID).
    try {
        $trayGuid = "{9C8B5A79-9F7F-4D6A-BF87-2E50B5D7A2C1}"
        $key = "HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\TrayNotify\NotifyIconSettings\$trayGuid"
        & reg.exe ADD $key /v "IsPromoted" /t REG_DWORD /d 1 /f | Out-Null
    } catch {
        # ignore
    }
}

# --- Administrator check (skipped for -BuildOnly) ---
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)

# --- Folders (do NOT assume env vars exist) ---
$local = Get-KnownFolderSafe ([Environment+SpecialFolder]::LocalApplicationData)
if (-not $local) { $local = $env:LOCALAPPDATA }
if (-not $local) { $local = Join-Path $HOME "AppData\Local" }
Assert-NotEmpty "LocalAppData" $local

$roaming = Get-KnownFolderSafe ([Environment+SpecialFolder]::ApplicationData)

$buildDir   = "C:\rb"  # Short path to avoid Windows MAX_PATH (260) overflow in deep CMake builds
$installDir = Join-Path $local "rakukan"
$regFile    = Join-Path $installDir "registered.txt"
$trayExe    = Join-Path $installDir "rakukan-tray.exe"

# --- 0/4 Backend (best-effort; never fail install) ---
$configToml = $null
if ($roaming) {
    $configToml = Join-Path $roaming "rakukan\config.toml"
}

$gpuBackend = $null
if ($configToml -and (Test-Path -LiteralPath $configToml)) {
    try {
        foreach ($line in Get-Content -LiteralPath $configToml) {
            if ($line -match '^\s*gpu_backend\s*=\s*"([^"]+)"') {
                $gpuBackend = $Matches[1].ToLower()
                break
            }
        }
    } catch { }
}

if ($gpuBackend -in @("cuda", "vulkan", "cpu")) {
    Write-Host "[0/4] Using config.toml backend: $gpuBackend"
} else {
    $gpuBackend = "cpu"
    Write-Host "[0/4] gpu_backend not set in config.toml -> running detect-gpu.ps1"
    try {
        $detected = & "$PSScriptRoot\detect-gpu.ps1" -SaveResult
        if ($detected -and ($detected.Trim().ToLower() -in @("cuda","vulkan","cpu"))) {
            $gpuBackend = $detected.Trim().ToLower()
        }
    } catch { }
}
Write-Host "[0/4] Cargo GPU features: (all backends built by build-engine.ps1)"

# --- 0.5/6 Clean stale CMake cache for llama-cpp-sys-2 (only on backend change) ---
# The cmake crate skips reconfiguration if the build dir exists.
# If the GPU backend changed (e.g. cpu -> cuda), the stale cache causes
# MSB1009 "install.vcxproj not found". We track the last-built backend in
# a stamp file and only wipe the cache when it changes.
$lastBackendFile = "C:\rb\last_gpu_backend.txt"
$lastBackend = if (Test-Path $lastBackendFile) {
    (Get-Content $lastBackendFile -ErrorAction SilentlyContinue) -replace '\s',''
} else { "" }

if ($lastBackend -ne $gpuBackend) {
    Write-Host "[0.5/6] GPU backend changed ($lastBackend -> $gpuBackend): clearing llama-cpp-sys-2 build cache"
    $llamaBuildGlob = "C:\rb\release\build\llama-cpp-sys-2-*"
    Get-Item $llamaBuildGlob -ErrorAction SilentlyContinue | ForEach-Object {
        Write-Host "  Removing: $($_.FullName)"
        Remove-Item $_.FullName -Recurse -Force
    }
    # Update stamp file
    New-Item -ItemType Directory -Force -Path (Split-Path $lastBackendFile) | Out-Null
    $gpuBackend | Set-Content -LiteralPath $lastBackendFile -NoNewline
} else {
    Write-Host "[0.5/6] GPU backend unchanged ($gpuBackend): skipping cache wipe (incremental build)"
}

# --- 1/6 Build engine DLLs (cpu / vulkan / cuda) ---
if ($SkipEngine) {
    Write-Host "[1/6] Skipping engine DLL build (-SkipEngine)"
    # Remove rakukan-engine-abi cache directly (cargo clean is unreliable with multiple target dirs).
    $prev = $ErrorActionPreference; $ErrorActionPreference = "Continue"
    foreach ($root in @($buildDir, "target")) {
        Get-ChildItem $root -Recurse -Directory -Filter "rakukan_engine_abi-*" -ErrorAction SilentlyContinue |
            ForEach-Object { Remove-Item $_.FullName -Recurse -Force -ErrorAction SilentlyContinue }
        Get-ChildItem "$root" -Recurse -Filter "librakukan_engine_abi*" -ErrorAction SilentlyContinue |
            ForEach-Object { Remove-Item $_.FullName -Force -ErrorAction SilentlyContinue }
        Get-ChildItem "$root" -Recurse -Filter "rakukan_engine_abi*" -ErrorAction SilentlyContinue |
            ForEach-Object { Remove-Item $_.FullName -Force -ErrorAction SilentlyContinue }
    }
    $ErrorActionPreference = $prev
} else {
    Write-Host "[1/6] Building rakukan-engine DLLs..."
    & "$PSScriptRoot\build-engine.ps1" -Profile $Profile -BuildDir $buildDir
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

# Build helper (tsf / tray / dict-builder)
function Invoke-CargoBuild {
    param([string]$Package, [string]$Profile, [string]$Features = "")
    $args_list = @("build", "-p", $Package)
    if ($Profile -eq "release") { $args_list += "--release" }
    if ($Features)              { $args_list += "--features=$Features" }
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & cargo @args_list 2>&1 | ForEach-Object {
        if ($_ -is [System.Management.Automation.ErrorRecord]) {
            Write-Host $_.Exception.Message
        } else {
            Write-Host $_
        }
    }
    $ErrorActionPreference = $prev
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$env:CARGO_TARGET_DIR = $buildDir
$profileDir = if ($Profile -eq "release") { "release" } else { "debug" }

# rakukan-tsf does NOT depend on rakukan-engine features (uses DynEngine loader)
if ($Profile -eq "release") {
    Invoke-CargoBuild -Package "rakukan-tsf"          -Profile "release"
    Invoke-CargoBuild -Package "rakukan-tray"         -Profile "release"
    Invoke-CargoBuild -Package "rakukan-engine-host"  -Profile "release"
    Invoke-CargoBuild -Package "rakukan-dict-builder"  -Profile "release"
    $srcDll     = Join-Path $buildDir "release\rakukan_tsf.dll"
    $srcTray    = Join-Path $buildDir "release\rakukan-tray.exe"
    $srcHost    = Join-Path $buildDir "release\rakukan-engine-host.exe"
    $srcBuilder = Join-Path $buildDir "release\rakukan-dict-builder.exe"
    $engineDlls = @("cpu","vulkan","cuda") | ForEach-Object {
        $p = Join-Path $buildDir "release\rakukan_engine_$_.dll"
        if (Test-Path $p) { $p } else { $null }
    } | Where-Object { $_ }
} else {
    Invoke-CargoBuild -Package "rakukan-tsf"          -Profile "debug"
    Invoke-CargoBuild -Package "rakukan-tray"         -Profile "debug"
    Invoke-CargoBuild -Package "rakukan-engine-host"  -Profile "debug"
    Invoke-CargoBuild -Package "rakukan-dict-builder"  -Profile "debug"
    $srcDll     = Join-Path $buildDir "debug\rakukan_tsf.dll"
    $srcTray    = Join-Path $buildDir "debug\rakukan-tray.exe"
    $srcHost    = Join-Path $buildDir "debug\rakukan-engine-host.exe"
    $srcBuilder = Join-Path $buildDir "debug\rakukan-dict-builder.exe"
    $engineDlls = @("cpu","vulkan","cuda") | ForEach-Object {
        $p = Join-Path $buildDir "debug\rakukan_engine_$_.dll"
        if (Test-Path $p) { $p } else { $null }
    } | Where-Object { $_ }
}

$env:CARGO_TARGET_DIR = $null
if (-not (Test-Path -LiteralPath $srcDll)) { throw "Build output not found: $srcDll" }

if ($BuildOnly) {
    Write-Host "[build-only] Build complete. Skipping install (-BuildOnly)."
    Stop-Transcript | Out-Null
    Write-Host "Log saved: $LogFile"
    exit 0
}

# Admin required for install/register
if (-not $isAdmin) { throw "Administrator privileges are required." }

# --- 2/6 Install (copy to LocalAppData) ---
Write-Host "[2/6] Installing..."
New-Item -ItemType Directory -Force -Path $installDir | Out-Null

# Unregister old DLL first so TSF releases the engine DLLs it holds
if (Test-Path -LiteralPath $regFile) {
    $oldDllEarly = Get-Content -LiteralPath $regFile -ErrorAction SilentlyContinue
    if ($oldDllEarly) { Invoke-Regsvr32UnregisterBestEffort $oldDllEarly }
}
# Stop IME processes after unregister to release DLL file locks
Stop-ProcSilent "rakukan-tray"
Stop-ProcSilent "ctfmon"
Stop-ProcSilent "TextInputHost"
Start-Sleep -Milliseconds 1200

$dst = Join-Path $installDir "rakukan_tsf.dll"
Copy-Item -LiteralPath $srcDll -Destination $dst -Force
Write-Host "  -> $dst"

# 古いタイムスタンプ付き DLL を削除（rakukan_tsf_YYYYMMDD_HHmmss.dll）
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

# Copy engine DLLs (rakukan_engine_cpu.dll / _vulkan.dll / _cuda.dll)
if ($engineDlls.Count -eq 0) {
    # engine DLL がビルドディレクトリに存在しない場合、インストール先の既存 DLL を確認する。
    # 既存 DLL もなければ Activate() が失敗して IME が選択不可になるため、エラーで停止する。
    $existingCpuDll = Join-Path $installDir "rakukan_engine_cpu.dll"
    if (-not (Test-Path -LiteralPath $existingCpuDll)) {
        throw "[ERROR] rakukan_engine_cpu.dll not found in build dir ($buildDir\$profileDir) " +
              "and not already installed. Run 'cargo make build-engine' first."
    }
    Write-Host "  [WARN] Engine DLL not rebuilt; using existing: $existingCpuDll"
} else {
    foreach ($engineDll in $engineDlls) {
        $dllName = [IO.Path]::GetFileName($engineDll)
        $engineDst = Join-Path $installDir $dllName
        Copy-Item -LiteralPath $engineDll -Destination $engineDst -Force
        Write-Host "  -> $engineDst"
    }
}

if (Test-Path -LiteralPath $srcTray) {
    Stop-ProcSilent "rakukan-tray"
    Start-Sleep -Milliseconds 300
    try {
        Copy-Item -LiteralPath $srcTray -Destination $trayExe -Force
    } catch {
        $tmpTray = "$trayExe.new"
        Copy-Item -LiteralPath $srcTray -Destination $tmpTray -Force
        Move-Item -LiteralPath $tmpTray -Destination $trayExe -Force
    }
    Write-Host "  -> $trayExe"
}

# rakukan-engine-host.exe をインストール（out-of-process エンジンホスト）
if (Test-Path -LiteralPath $srcHost) {
    $hostExe = Join-Path $installDir "rakukan-engine-host.exe"
    Stop-ProcSilent "rakukan-engine-host"
    Start-Sleep -Milliseconds 300
    try {
        Copy-Item -LiteralPath $srcHost -Destination $hostExe -Force
    } catch {
        $tmpHost = "$hostExe.new"
        Copy-Item -LiteralPath $srcHost -Destination $tmpHost -Force
        Move-Item -LiteralPath $tmpHost -Destination $hostExe -Force
    }
    Write-Host "  -> $hostExe"
}

# Copy rakukan-dict-builder.exe to install dir
if (Test-Path -LiteralPath $srcBuilder) {
    $builderDest = Join-Path $installDir "rakukan-dict-builder.exe"
    Copy-Item -LiteralPath $srcBuilder -Destination $builderDest -Force
    Write-Host "  -> $builderDest"
}

# Deploy config.toml only on first install (skip if already exists)
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

# --- 3/6 Unregister old (already done in step 2, repeat best-effort) ---
Write-Host "[3/6] Unregistering old version..."
if (Test-Path -LiteralPath $regFile) {
    $oldDll = Get-Content -LiteralPath $regFile -ErrorAction SilentlyContinue
    if ($oldDll) { Invoke-Regsvr32UnregisterBestEffort $oldDll }
}
Stop-ProcSilent "ctfmon"
Stop-ProcSilent "TextInputHost"
Start-Sleep -Milliseconds 400

# --- 4/6 Register new (and verify COM) ---
Write-Host "[4/6] Registering..."
$arch = Invoke-Regsvr32Strict $dst
Assert-ComRegistered $dst
Write-Host "Registered ($arch): $dst"

$dst | Set-Content -LiteralPath $regFile
# --- Some Windows 11 setups only surface TIPs in Settings when HKCU also contains the TIP keys.
#     Mirror the HKLM TIP registration into HKCU (best-effort).
try {
    $dllPathForFind = $dst

    $hit = (reg.exe query "HKCR\CLSID" /s /f $dllPathForFind 2>$null |
        Select-String -Pattern 'HKEY_CLASSES_ROOT\\CLSID\\\{[0-9A-Fa-f-]+\}\\InProcServer32' |
        Select-Object -First 1)

    if ($hit) {
        $clsid = ($hit.ToString() -replace '.*\\CLSID\\(\{[0-9A-Fa-f-]+\})\\InProcServer32','$1')
        reg.exe COPY "HKLM\Software\Microsoft\CTF\TIP\$clsid" "HKCU\Software\Microsoft\CTF\TIP\$clsid" /s /f | Out-Null
    }
} catch {
    # best-effort; ignore
}


# restart ctfmon
Start-Process ctfmon | Out-Null

# --- 5/6 Dictionary setup ---
Write-Host "[5/6] Setting up dictionaries..."
Write-Host ""

$dictDir   = Join-Path $env:LOCALAPPDATA "rakukan\dict"
New-Item -ItemType Directory -Force -Path $dictDir | Out-Null
$forceDict = $env:RAKUKAN_FORCE_DICT -eq "1"

# --- 5a: mozc dictionary (Apache 2.0) ---
Write-Host "  [5a] mozc dictionary (Apache 2.0)..."
Write-Host "  Source: https://github.com/google/mozc"

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
    Write-Host "  rakukan.dict will not be built."
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
                if (Test-Path -LiteralPath $tmpPath) {
                    Remove-Item -LiteralPath $tmpPath -Force -ErrorAction SilentlyContinue
                }
                Write-Host ("    [WARNING] Failed: " + $tsv + " - " + $_)
            }
        }
        if (Test-Path -LiteralPath $tsvPath) {
            $downloadedTsvs.Add($tsvPath)
        }
    }

    if ($downloadedTsvs.Count -eq 0) {
        Write-Host "  [WARNING] No mozc TSV files downloaded. rakukan.dict will not be built."
    } else {
        # Download symbol.tsv (Apache 2.0)
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
                if (Test-Path -LiteralPath $tmpPath) {
                    Remove-Item -LiteralPath $tmpPath -Force -ErrorAction SilentlyContinue
                }
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


# --- 5c: LLM model pre-download ---
Write-Host ""
Write-Host "  [5c] LLM model pre-download..."

$configToml   = Join-Path $env:APPDATA "rakukan\config.toml"
$modelVariant = $null
if (Test-Path -LiteralPath $configToml) {
    foreach ($line in (Get-Content $configToml -Encoding UTF8)) {
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
    Write-Host "  To enable LLM conversion, add model_variant to config.toml."
} else {
    # Map variant id to repo and filename
    $modelMap = @{
        "jinen-v1-small-q5"   = @{ repo = "togatogah/jinen-v1-small.gguf";   file = "jinen-v1-small-Q5_K_M.gguf";   tok = "tokenizer.json" }
        "jinen-v1-xsmall-q5"  = @{ repo = "togatogah/jinen-v1-xsmall.gguf";  file = "jinen-v1-xsmall-Q5_K_M.gguf";  tok = "tokenizer.json" }
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

# --- 6/6 Tray setup ---
Write-Host "[6/6] Setting up tray icon..."
if (Test-Path -LiteralPath $trayExe) {
    Stop-ProcSilent "rakukan-tray"
    Setup-RunTray $trayExe
    Promote-TrayIcon
    Start-Process -FilePath $trayExe | Out-Null
    Write-Host "Tray started."
}

Write-Host ""
Write-Host "Installed: $dst"
Write-Host "Switch to rakukan in the language bar."

Stop-Transcript | Out-Null
Write-Host "Log saved: $LogFile"
