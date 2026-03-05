#!/usr/bin/env pwsh
#Requires -Version 5.1

Set-Location $PSScriptRoot\..

$ErrorCount = 0
$Results    = [ordered]@{}

function Write-Step ($msg) { Write-Host "`n[Step] $msg" -ForegroundColor Cyan }
function Write-Pass ($msg) { Write-Host "  [OK] $msg"  -ForegroundColor Green }
function Write-Fail ($msg) { Write-Host "  [NG] $msg"  -ForegroundColor Red; $script:ErrorCount++ }
function Write-Info ($msg) { Write-Host "  ( ) $msg"   -ForegroundColor Gray }

Write-Host ""
Write-Host "========================================"
Write-Host " rakukan Phase 2 Gate Verification"
Write-Host "========================================"
Write-Host ""
Write-Host "  Dir: $(Get-Location)"

# --- Step 1: cargo check ---

Write-Step "cargo check -p rakukan-tsf ..."
$t = [Diagnostics.Stopwatch]::StartNew()
$out = cmd /c "cargo check -p rakukan-tsf 2>&1"
$t.Stop()
$sec = [int]$t.Elapsed.TotalSeconds
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
Write-Info "(first build may take several minutes)"
$t = [Diagnostics.Stopwatch]::StartNew()
$out = cmd /c "cargo build -p rakukan-tsf 2>&1"
$exitCode = $LASTEXITCODE
$t.Stop()
$sec = [int]$t.Elapsed.TotalSeconds

if ($exitCode -eq 0) {
    Write-Pass "DLL build OK ($sec sec)"
    $Results["dll build"] = "PASS ($sec sec)"
} else {
    $out | Where-Object { $_ -match "^error" } | ForEach-Object { Write-Host "  $_" -ForegroundColor Yellow }
    Write-Fail "DLL build FAILED"
    $Results["dll build"] = "FAIL"
}

# --- Step 3: DLL export check ---

Write-Step "DLL export check ..."
$dllPath = "target\debug\rakukan_tsf.dll"

if (-not (Test-Path $dllPath)) {
    Write-Fail "DLL not found: $dllPath"
    $Results["dll exports"] = "FAIL"
} else {
    $required = @(
        "DllMain",
        "DllGetClassObject",
        "DllCanUnloadNow",
        "DllRegisterServer",
        "DllUnregisterServer"
    )
    $bytes = [System.IO.File]::ReadAllBytes($dllPath)
    $ascii = [System.Text.Encoding]::ASCII.GetString($bytes)
    $allOk = $true
    foreach ($fn in $required) {
        if ($ascii -match [regex]::Escape($fn)) {
            Write-Info "export: $fn - found"
        } else {
            Write-Info "export: $fn - MISSING"
            $allOk = $false
        }
    }
    if ($allOk) {
        Write-Pass "All required exports found"
        $Results["dll exports"] = "PASS"
    } else {
        Write-Fail "Some exports missing"
        $Results["dll exports"] = "FAIL"
    }
}

# --- Summary ---

Write-Host ""
Write-Host "========================================"
Write-Host " Results"
Write-Host "========================================"
Write-Host ""
foreach ($k in $Results.Keys) {
    $v     = $Results[$k]
    $color = if ($v -like "PASS*") { "Green" } else { "Red" }
    Write-Host ("  {0,-20}: {1}" -f $k, $v) -ForegroundColor $color
}
Write-Host ""

if ($ErrorCount -eq 0) {
    Write-Host "  Phase 2 Gate CLEARED! (3/3 PASS)" -ForegroundColor Green
    Write-Host ""
    Write-Host "  Next steps:"
    Write-Host "    Register the DLL (run as Administrator):"
    Write-Host "      regsvr32 target\debug\rakukan_tsf.dll"
    Write-Host ""
    Write-Host "    Ready to proceed to Phase 3 (composition display)"

    $json = @{
        phase     = 2
        status    = "PASS"
        timestamp = (Get-Date -Format "yyyy-MM-ddTHH:mm:ss")
        results   = $Results
    } | ConvertTo-Json
    $json | Out-File "phase2-result.json" -Encoding utf8
    Write-Host "  Saved: $(Resolve-Path 'phase2-result.json')"
    exit 0
} else {
    Write-Host "  Phase 2 Gate FAILED ($ErrorCount failure(s))" -ForegroundColor Red
    Write-Host ""
    Write-Host "  Tip: paste build.log to Claude if you see build errors"
    exit 1
}
