# ============================================================================
#  [AI ASSISTANT NOTICE / Claude Code 向け注意]
# ============================================================================
#  このスクリプトをユーザーに直接実行させるよう案内しないこと。
#  正規の入口は以下の cargo make コマンドです:
#
#      cargo make install                (管理者権限が必要 / 自動昇格あり)
#      cargo make install-tsf-fault-test / install-tsf-normal-restore (#65 試験専用)
#
#  ビルド成果物が必要なため、事前に以下を実行してください:
#      cargo make build-engine
#      cargo make build-tsf
#      cargo make sign                   (署名が必要な場合のみ)
#  一括実行:
#      cargo make full-install           (build-engine + build-tsf + sign + install)
#      cargo make quick-install          (build-tsf + install、開発時の高速再インストール)
#
#  このファイルは Makefile.toml から呼び出される内部実装です。
#  PS1 を直接叩く手順を提示すると cargo make と二重化してユーザーが混乱します。
# ============================================================================
#
# scripts\install.ps1 - rakukan インストール (コピー + 登録 + tray 起動)

param(
    [ValidateSet("debug","release")] [string]$Profile = "release",
    [string]$BuildDir = "C:\rb",
    [switch]$TsfOnly,   # #65 故障試験: TSF DLL だけを差し替えて登録する
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
# TSF DLL 登録 (regsvr32 → HKLM 書き込み) とプロセス停止 (ctfmon 等) のため
# 管理者権限が必須。非管理者セッションから呼ばれた場合は UAC で昇格して再実行する。
$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $isAdmin -and -not $NoElevate) {
    Write-Host "[install] Requesting administrator privileges (UAC)..." -ForegroundColor Yellow
    $argList = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "`"$PSCommandPath`"", "-NoElevate")
    foreach ($pair in $PSBoundParameters.GetEnumerator()) {
        $name  = $pair.Key
        $value = $pair.Value
        if ($value -is [switch]) {
            if ($value.IsPresent) { $argList += "-$name" }
        } elseif ($null -ne $value -and $value -ne "") {
            $argList += "-$name"
            $argList += "`"$value`""
        }
    }
    # Note: Start-Process -Wait は UAC 昇格 (-Verb RunAs) と組み合わせた場合
    # Windows PowerShell 5.1 で正しく待機しないことがあるため、PassThru で
    # Process オブジェクトを受け取り WaitForExit() で明示的に待機する。
    try {
        $proc = Start-Process -FilePath "powershell.exe" -Verb RunAs -ArgumentList $argList -PassThru
        if (-not $proc) {
            Write-Error "[install] Failed to launch elevated PowerShell"
            exit 1
        }
        $proc.WaitForExit()
        exit $proc.ExitCode
    } catch {
        Write-Error "[install] Elevation failed: $_"
        exit 1
    }
}

# --- Log file setup ---
$logName = if ($TsfOnly) {
    "rakukan_install_tsf_{0:yyyyMMdd_HHmmss}_{1}.log" -f (Get-Date), $PID
} else {
    "rakukan_install.log"
}
$LogFile  = Join-Path (Get-Location).Path $logName
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

# DLL をロック解放待ちのリトライ付きでコピーする。
#
# 直前に kill した ctfmon / TextInputHost / engine-host がファイルを解放するまで
# 1.2 秒では足りないことがあり、その場合は同じコマンドをもう一度実行するだけで
# 通る（2026-09-06 実機）。1 回目で通るように、IOException なら少し待って
# 数回やり直す。最後まで失敗したら例外をそのまま投げ、既存のロック案内へ落とす。
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
# Folder setup (admin は冒頭で自動昇格済み)
# ─────────────────────────────────────────────────────────────────────────────

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

if ($TsfOnly) {
    $srcHash = (Get-FileHash -LiteralPath $srcDll -Algorithm SHA256).Hash
    $installedDll = Join-Path $installDir "rakukan_tsf.dll"
    if (-not (Test-Path -LiteralPath $installedDll) -or -not (Test-Path -LiteralPath $regFile)) {
        throw "[install] -TsfOnly requires an existing registered installation: $installedDll"
    }
    $registeredDll = (Get-Content -LiteralPath $regFile -Raw).Trim()
    if (-not $registeredDll.Equals($installedDll, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "[install] registered DLL differs from install destination: $registeredDll"
    }
}

if (-not $TsfOnly -and $engineDlls.Count -eq 0) {
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

try {

# Unregister old DLL to release engine DLL file locks
if (Test-Path -LiteralPath $regFile) {
    $oldDllEarly = Get-Content -LiteralPath $regFile -ErrorAction SilentlyContinue
    if ($oldDllEarly) { Invoke-Regsvr32UnregisterBestEffort $oldDllEarly }
}
if (-not $TsfOnly) {
    Stop-ProcSilent "rakukan-tray"
    Stop-ProcSilent "rakukan-engine-host"
    Stop-ProcSilent "rakukan-settings"
}
Stop-ProcSilent "ctfmon"
Stop-ProcSilent "TextInputHost"
Start-Sleep -Milliseconds 1200

# TSF DLL
$dst = Join-Path $installDir "rakukan_tsf.dll"
Copy-DllWithRetry -Source $srcDll -Destination $dst
Write-Host "  -> $dst"
if ($TsfOnly) {
    $dstHash = (Get-FileHash -LiteralPath $dst -Algorithm SHA256).Hash
    Write-Host "  TSF SHA256 source:    $srcHash"
    Write-Host "  TSF SHA256 installed: $dstHash"
    if ($srcHash -ne $dstHash) {
        throw "[install] TSF DLL SHA256 mismatch after copy"
    }
}

if (-not $TsfOnly) {
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
    Copy-DllWithRetry -Source $engineDll -Destination $engineDst
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
}

} catch [System.IO.IOException] {
    Write-Host ""
    Write-Host "[install] ファイルがロックされていてコピーできません:" -ForegroundColor Red
    Write-Host "  $($_.Exception.Message)" -ForegroundColor Red
    Write-Host ""
    $retryTask = if ($TsfOnly) { "対応する install-tsf-* タスク" } else { "sudo cargo make install" }
    Write-Host "  対処: 一旦サインアウト→再ログオンしてから '$retryTask' を再実行してください。" -ForegroundColor Yellow
    Write-Host "        (TSF DLL / engine DLL は再ログオンで自動解放されます)" -ForegroundColor Yellow
    Stop-Transcript | Out-Null
    exit 1
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

if ($TsfOnly) {
    Write-Host "Installed TSF DLL only: $dst"
    Write-Host "TSF SHA256: $dstHash"
    Stop-Transcript | Out-Null
    Write-Host "Log saved: $LogFile"
    exit 0
}

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
# 取得元は完全なコミット SHA で固定する（Issue #62）。
# ブランチ参照（refs/heads/master）だと、同じ版の rakukan でも作った時期によって
# 辞書の中身が変わり、変換品質の差を実装の違いと切り分けられない。
# upstream を取り込むときはこの値を上げる。辞書は SHA の不一致で自動的に作り直される
# （RAKUKAN_FORCE_DICT=1 を手で指定する必要はない）。
$mozcRev     = "cbbb6e1bd181cb9f3b409622d916a53a35400ec7"
$mozcRepoUrl = "https://raw.githubusercontent.com/google/mozc/$mozcRev"
$mozcBaseUrl = "$mozcRepoUrl/src/data/dictionary_oss"
$symbolUrl   = "$mozcRepoUrl/src/data/symbol/symbol.tsv"
$emojiUrl    = "$mozcRepoUrl/src/data/emoji/emoji_data.tsv"

# 取得済み TSV は SHA ごとの作業フォルダに置く。別のリビジョンで取ったものを
# 新しい SHA のものとして再利用しないため。
$mozcTsvDir  = Join-Path $dictDir ("mozc_tsv_" + $mozcRev.Substring(0, 12))

# cost 帯の版（rakukan-dict の cost_band::DICT_SCHEMA と同じ値にする）。
# 辞書の隣の rakukan.dict.build.json にビルダーが書く。無い・古い辞書は再生成する。
$dictSchemaExpected = 2
$dictBuildInfo = "$mozcDictOut.build.json"

# 復旧不能ならここで停止し、再生成判定や別の差し替えへ進まない（Issue #62）。
. (Join-Path $PSScriptRoot 'dictionary-swap.ps1')
$recovery = Restore-DictionarySwap -DictionaryPath $mozcDictOut
if ($recovery -eq 'rolled_back') {
    Write-Host "  -> restored the previous dictionary state after an interrupted swap"
} elseif ($recovery -eq 'committed') {
    Write-Host "  -> kept the committed dictionary pair and finished backup cleanup"
}

$dictUpToDate = $false
$dictRebuildReason = $null
if (Test-Path -LiteralPath $mozcDictOut) {
    if (Test-Path -LiteralPath $dictBuildInfo) {
        try {
            $info = Get-Content -LiteralPath $dictBuildInfo -Raw | ConvertFrom-Json
            if ([int]$info.dict_schema -lt $dictSchemaExpected) {
                $dictRebuildReason = "dict_schema " + $info.dict_schema + " < " + $dictSchemaExpected
            } elseif ([string]$info.mozc_rev -ne $mozcRev) {
                $haveRev = if ($info.mozc_rev) { $info.mozc_rev } else { "unknown" }
                $dictRebuildReason = "mozc_rev " + $haveRev + " != " + $mozcRev
            } else {
                $dictUpToDate = $true
            }
        } catch {
            $dictRebuildReason = "build info could not be read"
        }
    } else {
        $dictRebuildReason = "no build info (built before dict_schema 2)"
    }
}
if ($dictRebuildReason) {
    Write-Host ("  -> rebuilding rakukan.dict: " + $dictRebuildReason)
}

if ($dictUpToDate -and (-not $forceDict)) {
    $sizeMB = [math]::Round((Get-Item $mozcDictOut).Length / 1048576, 1)
    Write-Host ("  -> rakukan.dict already built (" + $sizeMB + " MB, dict_schema " + $info.dict_schema + ", mozc " + $mozcRev.Substring(0, 12) + "), skipping.")
    Write-Host "     (To rebuild, set RAKUKAN_FORCE_DICT=1 and re-run)"
} elseif (-not (Test-Path -LiteralPath $dictBuilderExe)) {
    Write-Host "  [WARNING] rakukan-dict-builder.exe not found, skipping mozc dict."
} else {
    # 別のリビジョンで取得した TSV を残さない
    Get-ChildItem -LiteralPath $dictDir -Directory -Filter "mozc_tsv*" -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -ne $mozcTsvDir } |
        ForEach-Object {
            Write-Host ("    Removing TSV from another revision: " + $_.Name)
            Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue
        }
    New-Item -ItemType Directory -Force -Path $mozcTsvDir | Out-Null

    # 必須入力。1 つでも欠けたら再生成を中止する（一部を欠いた辞書を作らない）。
    $requiredInputs = @()
    foreach ($tsv in $mozcTsvFiles) {
        $requiredInputs += [pscustomobject]@{
            Name = $tsv
            Url  = ($mozcBaseUrl + "/" + $tsv)
            Path = (Join-Path $mozcTsvDir $tsv)
        }
    }
    $requiredInputs += [pscustomobject]@{
        Name = "symbol.tsv"; Url = $symbolUrl; Path = (Join-Path $mozcTsvDir "symbol.tsv")
    }
    $requiredInputs += [pscustomobject]@{
        Name = "emoji_data.tsv"; Url = $emojiUrl; Path = (Join-Path $mozcTsvDir "emoji_data.tsv")
    }

    $ProgressPreference = "SilentlyContinue"
    $missingInputs = [System.Collections.Generic.List[string]]::new()

    foreach ($item in $requiredInputs) {
        # 同じ SHA のフォルダに既にあるものは取り直さない（中断からの再開）
        if ((Test-Path -LiteralPath $item.Path) -and (-not $forceDict)) { continue }
        $tmpPath = $item.Path + ".tmp"
        try {
            Invoke-WebRequest -Uri $item.Url -OutFile $tmpPath -UseBasicParsing -TimeoutSec 120
            Move-Item -LiteralPath $tmpPath -Destination $item.Path -Force
            Write-Host ("    Downloaded: " + $item.Name)
        } catch {
            if (Test-Path -LiteralPath $tmpPath) { Remove-Item -LiteralPath $tmpPath -Force -ErrorAction SilentlyContinue }
            Write-Host ("    [WARNING] Failed: " + $item.Name + " - " + $_)
            $missingInputs.Add($item.Name)
        }
    }

    if ($missingInputs.Count -gt 0) {
        # 一部の入力を欠いた辞書を、正常に生成できたものとして扱わない
        Write-Host ("  [WARNING] " + $missingInputs.Count + " required input(s) missing: " + ($missingInputs -join ", "))
        Write-Host "  -> skipping rebuild; keeping the existing rakukan.dict and build.json"
    } else {
        Write-Host ("  Building rakukan.dict from " + $mozcTsvFiles.Count + " TSV files + symbol.tsv + emoji_data.tsv (mozc " + $mozcRev.Substring(0, 12) + ")...")

        # いったん別名へ生成し、成功したときだけ差し替える。
        # 途中で失敗しても既存の辞書と build.json を壊さない。
        $newDictOut   = $mozcDictOut + ".new"
        $newBuildInfo = "$newDictOut.build.json"
        Remove-Item -LiteralPath $newDictOut -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $newBuildInfo -Force -ErrorAction SilentlyContinue

        $inputArgs = @()
        foreach ($tsv in $mozcTsvFiles) {
            $inputArgs += "--input"
            $inputArgs += (Join-Path $mozcTsvDir $tsv)
        }
        $inputArgs += "--symbol"
        $inputArgs += (Join-Path $mozcTsvDir "symbol.tsv")
        $inputArgs += "--emoji"
        $inputArgs += (Join-Path $mozcTsvDir "emoji_data.tsv")
        $inputArgs += "--mozc-rev"
        $inputArgs += $mozcRev
        $inputArgs += "--mozc-source"
        $inputArgs += $mozcRepoUrl
        $inputArgs += "--output"
        $inputArgs += $newDictOut

        $buildSucceeded = $false
        try {
            & $dictBuilderExe @inputArgs
            $buildSucceeded = $LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $newDictOut) -and (Test-Path -LiteralPath $newBuildInfo)
            if (-not $buildSucceeded) {
                Write-Host ("  [WARNING] rakukan-dict-builder failed (exit " + $LASTEXITCODE + "); keeping the existing rakukan.dict")
            }
        } catch {
            Write-Host ("  [WARNING] rakukan-dict-builder error: " + $_ + "; keeping the existing rakukan.dict")
        }
        if ($buildSucceeded) {
            # 復元・掃除の失敗をビルダーの catch で握りつぶさない。
            Install-DictionaryPair -DictionaryPath $mozcDictOut
            $sizeMB = [math]::Round((Get-Item $mozcDictOut).Length / 1048576, 1)
            Write-Host ("  -> " + $mozcDictOut + " (" + $sizeMB + " MB, mozc " + $mozcRev.Substring(0, 12) + ")")
            Remove-Item -LiteralPath $mozcTsvDir -Recurse -Force -ErrorAction SilentlyContinue
        } else {
            Remove-Item -LiteralPath $newDictOut -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $newBuildInfo -Force -ErrorAction SilentlyContinue
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
        "jinen-v2-small-q5"   = @{ repo = "togatogah/jinen-v2-small.gguf";   file = "jinen-v2-small-Q5_K_M.gguf";   tok = "tokenizer.json" }
        "jinen-v2-xsmall-q5"  = @{ repo = "togatogah/jinen-v2-xsmall.gguf";  file = "jinen-v2-xsmall-Q5_K_M.gguf";  tok = "tokenizer.json" }
        "jinen-v2-small-f16"  = @{ repo = "togatogah/jinen-v2-small.gguf";   file = "jinen-v2-small-f16.gguf";      tok = "tokenizer.json" }
        "jinen-v2-xsmall-f16" = @{ repo = "togatogah/jinen-v2-xsmall.gguf";  file = "jinen-v2-xsmall-f16.gguf";     tok = "tokenizer.json" }
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
