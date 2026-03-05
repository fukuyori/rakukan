#!/usr/bin/env pwsh
Set-Location $PSScriptRoot\..

$ErrorCount = 0
$Results    = [ordered]@{}

function Write-Step ($msg) { Write-Host "`n[Step] $msg" -ForegroundColor Cyan }
function Write-Pass ($msg) { Write-Host "  [OK] $msg"  -ForegroundColor Green }
function Write-Fail ($msg) { Write-Host "  [NG] $msg"  -ForegroundColor Red; $script:ErrorCount++ }
function Write-Info ($msg) { Write-Host "  ( ) $msg"   -ForegroundColor Gray }

Write-Host ""
Write-Host "========================================"
Write-Host " rakukan Phase 3 Gate Verification"
Write-Host "========================================"

# --- Step 1: cargo check ---
Write-Step "cargo check -p rakukan-tsf ..."
$t = [Diagnostics.Stopwatch]::StartNew()
$out = cmd /c "cargo check -p rakukan-tsf 2>&1"
$t.Stop(); $sec = [int]$t.Elapsed.TotalSeconds
if ($LASTEXITCODE -eq 0) {
    Write-Pass "cargo check OK ($sec sec)"
    $Results["cargo check"] = "PASS ($sec sec)"
} else {
    $out | Where-Object { $_ -match "^error" } | ForEach-Object { Write-Host "  $_" -ForegroundColor Yellow }
    Write-Fail "cargo check FAILED"
    $Results["cargo check"] = "FAIL"
}

# --- Step 2: cargo build ---
Write-Step "cargo build -p rakukan-tsf ..."
Write-Info "(may take a few minutes)"
$t = [Diagnostics.Stopwatch]::StartNew()
$out = cmd /c "cargo build -p rakukan-tsf 2>&1"
$exitCode = $LASTEXITCODE; $t.Stop(); $sec = [int]$t.Elapsed.TotalSeconds
if ($exitCode -eq 0) {
    Write-Pass "DLL build OK ($sec sec)"
    $Results["dll build"] = "PASS ($sec sec)"
} else {
    $out | Where-Object { $_ -match "^error" } | ForEach-Object { Write-Host "  $_" -ForegroundColor Yellow }
    Write-Fail "DLL build FAILED"
    $Results["dll build"] = "FAIL"
}

# --- Step 3: DLL exports ---
Write-Step "DLL export check ..."
$dllPath = "target\debug\rakukan_tsf.dll"
if (-not (Test-Path $dllPath)) {
    Write-Fail "DLL not found: $dllPath"
    $Results["dll exports"] = "FAIL"
} else {
    $bytes = [System.IO.File]::ReadAllBytes($dllPath)
    $ascii = [System.Text.Encoding]::ASCII.GetString($bytes)
    $required = @("DllMain","DllGetClassObject","DllCanUnloadNow","DllRegisterServer","DllUnregisterServer")
    $allOk = $true
    foreach ($fn in $required) {
        if ($ascii -match [regex]::Escape($fn)) { Write-Info "export: $fn - found" }
        else { Write-Info "export: $fn - MISSING"; $allOk = $false }
    }
    if ($allOk) { Write-Pass "All exports found"; $Results["dll exports"] = "PASS" }
    else { Write-Fail "Missing exports"; $Results["dll exports"] = "FAIL" }
}

# --- Step 4: Phase 3 ソースチェック ---
Write-Step "Phase 3 source check ..."

$checks = @(
    @{ File = "crates\rakukan-tsf\src\tsf\edit_session.rs"; Pattern = "ITfEditSession_Impl"; Desc = "EditSession impl" },
    @{ File = "crates\rakukan-tsf\src\tsf\factory.rs";      Pattern = "ITfCompositionSink";  Desc = "ITfCompositionSink" },
    @{ File = "crates\rakukan-tsf\src\tsf\factory.rs";      Pattern = "StartComposition";    Desc = "StartComposition" },
    @{ File = "crates\rakukan-tsf\src\tsf\factory.rs";      Pattern = "EndComposition";      Desc = "EndComposition" },
    @{ File = "crates\rakukan-tsf\src\tsf\factory.rs";      Pattern = "RequestEditSession";  Desc = "RequestEditSession" },
    @{ File = "crates\rakukan-tsf\src\engine\state.rs";     Pattern = "COMPOSITION";         Desc = "Global COMPOSITION state" }
)

$srcOk = $true
foreach ($c in $checks) {
    if (Select-String -Path $c.File -Pattern $c.Pattern -Quiet) {
        Write-Info "$($c.Desc) - found"
    } else {
        Write-Info "$($c.Desc) - MISSING in $($c.File)"
        $srcOk = $false
    }
}
if ($srcOk) { Write-Pass "All Phase 3 components present"; $Results["phase3 source"] = "PASS" }
else { Write-Fail "Phase 3 components missing"; $Results["phase3 source"] = "FAIL" }

# --- Summary ---
Write-Host ""
Write-Host "========================================"
Write-Host " Results"
Write-Host "========================================"
foreach ($k in $Results.Keys) {
    $v = $Results[$k]
    $color = if ($v -like "PASS*") { "Green" } else { "Red" }
    Write-Host ("  {0,-20}: {1}" -f $k, $v) -ForegroundColor $color
}
Write-Host ""

if ($ErrorCount -eq 0) {
    Write-Host "  Phase 3 Gate CLEARED! (4/4 PASS)" -ForegroundColor Green
    Write-Host ""
    Write-Host "  Register and test (as Administrator):"
    Write-Host "    regsvr32 target\debug\rakukan_tsf.dll"
    Write-Host "    (switch to rakukan in language bar, open Notepad, type romaji)"
    $json = @{ phase=3; status="PASS"; timestamp=(Get-Date -Format "yyyy-MM-ddTHH:mm:ss") } | ConvertTo-Json
    $json | Out-File "phase3-result.json" -Encoding utf8
    exit 0
} else {
    Write-Host "  Phase 3 Gate FAILED ($ErrorCount failure(s))" -ForegroundColor Red
    Write-Host "  Tip: run 'cargo check -p rakukan-tsf 2>&1 | Out-File build.log' and send build.log"
    exit 1
}
