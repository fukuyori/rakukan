# scripts\build-engine.ps1 - rakukan-engine DLL builder (cpu / vulkan / cuda)
#
# Usage (standalone):
#   powershell -ExecutionPolicy Bypass -File scripts\build-engine.ps1 [-Profile release|debug]
#
# Usage (from install.ps1):
#   & "$PSScriptRoot\build-engine.ps1" -Profile $Profile -BuildDir $buildDir
#
# Outputs (in $BuildDir\<profile>\):
#   rakukan_engine_cpu.dll    -- always built
#   rakukan_engine_vulkan.dll -- if VULKAN_SDK is set
#   rakukan_engine_cuda.dll   -- if nvcc found

param(
    [ValidateSet("debug","release")] [string]$Profile  = "release",
    [string]$BuildDir = "C:\rb"
)

$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot)

# --- Log setup ---
# Standalone: write own transcript. Called from install.ps1: skip (PS 5.1 no nested transcript).
$LogFile       = $null
$OwnTranscript = $false
try { $null = Get-Variable -Name TRANSCRIPT_STARTED -Scope Global -ErrorAction Stop }
catch {
    $LogFile       = Join-Path (Get-Location).Path "rakukan_build_engine.log"
    Start-Transcript -LiteralPath $LogFile -Force | Out-Null
    $OwnTranscript = $true
    Write-Host "Log: $LogFile"
}

# --- Compute all paths BEFORE vcvarsall.bat (vcvarsall may clobber env vars) ---
$profileDir     = if ($Profile -eq "release") { "release" } else { "debug" }
$cpuDll         = Join-Path $BuildDir "$profileDir\rakukan_engine.dll"
$llamaGlob      = Join-Path $BuildDir "$profileDir\build\llama-cpp-sys-2-*"
$ninjaStamp     = Join-Path $BuildDir "ninja_generator.stamp"

$env:CARGO_TARGET_DIR = $BuildDir
$null = New-Item -ItemType Directory -Force -Path $BuildDir

# --- Cargo build helper ---
function Invoke-CargoBuild {
    param([string]$Package, [string]$Profile, [string]$Features = "")
    $argList = @("build", "-p", $Package)
    if ($Profile -eq "release") { $argList += "--release" }
    if ($Features)              { $argList += "--features=$Features" }
    $prev = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    & cargo @argList 2>&1 | ForEach-Object {
        if ($_ -is [System.Management.Automation.ErrorRecord]) {
            Write-Host $_.Exception.Message
        } else {
            Write-Host $_
        }
    }
    $ErrorActionPreference = $prev
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

# --- VS environment helper (sources vcvarsall x64, makes Ninja available) ---
function Invoke-VsEnv {
    $vcvars  = $null
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
        if ($vsPath) { $vcvars = Join-Path $vsPath "VC\Auxiliary\Build\vcvarsall.bat" }
    }
    if (-not $vcvars -or -not (Test-Path $vcvars)) {
        foreach ($r in (Get-ChildItem "C:\Program Files\Microsoft Visual Studio" -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending)) {
            $c = Join-Path $r.FullName "VC\Auxiliary\Build\vcvarsall.bat"
            if (Test-Path $c) { $vcvars = $c; break }
        }
    }
    if (-not $vcvars) { Write-Warning "vcvarsall.bat not found"; return $false }
    Write-Host "  Sourcing VS env: $vcvars"
    $tmp = [IO.Path]::GetTempFileName()
    cmd /c "`"$vcvars`" x64 > nul 2>&1 && set" | Out-File $tmp -Encoding ASCII
    Get-Content $tmp | ForEach-Object {
        if ($_ -match "^([^=]+)=(.*)$") {
            [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], "Process")
        }
    }
    Remove-Item $tmp -Force -ErrorAction SilentlyContinue
    return $true
}

# --- nvcc detection (PATH first, then standard install locations) ---
$nvcc = Get-Command "nvcc" -ErrorAction SilentlyContinue
if (-not $nvcc) {
    $cudaRoot = $env:CUDA_PATH
    if (-not $cudaRoot) {
        $cudaRoot = Get-Item "Env:CUDA_PATH_V*" -ErrorAction SilentlyContinue |
                    Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty Value
    }
    if (-not $cudaRoot) {
        $cudaRoot = Get-ChildItem "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA" -Directory -ErrorAction SilentlyContinue |
                    Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty FullName
    }
    if ($cudaRoot -and (Test-Path (Join-Path $cudaRoot "bin\nvcc.exe"))) {
        $env:PATH = "$cudaRoot\bin;$env:PATH"
        $nvcc = Get-Command "nvcc" -ErrorAction SilentlyContinue
        Write-Host "[engine] CUDA found at: $cudaRoot"
    }
}

# --- Prepare Ninja environment (used for both Vulkan and CUDA) ---
# MSBuild 18 parallel builds break ExternalProject step ordering (vulkan-shaders-gen).
# Ninja handles dependencies correctly. CUDA also works with Ninja + CUDACXX=nvcc.
$needNinja = ($env:VULKAN_SDK -and (Test-Path $env:VULKAN_SDK)) -or ($null -ne $nvcc)
if ($needNinja) {
    Write-Host "[engine] Preparing Ninja environment (Vulkan + CUDA)..."
    Invoke-VsEnv | Out-Null
    # Wipe llama-cpp-sys-2 cache when cmake exe path or generator changes
    $cmakeExe  = (Get-Command "cmake" -ErrorAction SilentlyContinue)
    $cmakePath = if ($cmakeExe) { $cmakeExe.Source } else { "cmake" }
    $stampVal  = "Ninja|$cmakePath"
    $lastStamp = if (Test-Path $ninjaStamp) { (Get-Content $ninjaStamp -Raw).Trim() } else { "" }
    if ($lastStamp -ne $stampVal) {
        Write-Host "  Config changed; clearing llama-cpp-sys-2 cache"
        Write-Host "    was: $lastStamp"
        Write-Host "    now: $stampVal"
        Get-Item $llamaGlob -ErrorAction SilentlyContinue | ForEach-Object {
            Write-Host "    Removing: $($_.FullName)"
            Remove-Item $_.FullName -Recurse -Force
        }
        $null = New-Item -ItemType Directory -Force -Path (Split-Path $ninjaStamp)
        $stampVal | Set-Content $ninjaStamp -NoNewline
    } else {
        Write-Host "  Generator unchanged (Ninja); skipping cache wipe"
    }
    $env:CMAKE_GENERATOR = "Ninja"
}
if ($nvcc) {
    $env:CUDACXX   = "nvcc"
    $env:CUDAFLAGS = "--allow-unsupported-compiler"
    Write-Host "[engine] CUDA: CUDACXX=nvcc CUDAFLAGS=--allow-unsupported-compiler"
    # cudart.lib / cublas.lib のリンクに必要な CUDA lib パスを LIB に追加
    $cudaLibPath = $null
    if ($env:CUDA_PATH -and (Test-Path "$env:CUDA_PATH\lib\x64")) {
        $cudaLibPath = "$env:CUDA_PATH\lib\x64"
    } elseif (Test-Path "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA") {
        $cudaLibPath = Get-ChildItem "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA" -Directory |
                       Sort-Object Name -Descending | Select-Object -First 1 |
                       ForEach-Object { "$($_.FullName)\lib\x64" }
    }
    if ($cudaLibPath -and (Test-Path $cudaLibPath)) {
        $env:LIB = "$cudaLibPath;$env:LIB"
        Write-Host "[engine] CUDA lib path added to LIB: $cudaLibPath"
    } else {
        Write-Warning "[engine] CUDA lib path not found; cudart.lib / cublas.lib may be missing"
    }
}

# --- CPU DLL ---
Write-Host "[engine] Building cpu DLL..."
Invoke-CargoBuild -Package "rakukan-engine" -Profile $Profile -Features ""
if (Test-Path $cpuDll) {
    Copy-Item $cpuDll (Join-Path $BuildDir "$profileDir\rakukan_engine_cpu.dll") -Force
    Write-Host "[engine] [OK] cpu DLL"
} else {
    Write-Warning "[engine] cpu DLL not found after build"
}

# --- Vulkan DLL ---
if ($env:VULKAN_SDK -and (Test-Path $env:VULKAN_SDK)) {
    Write-Host "[engine] Building vulkan DLL..."
    Invoke-CargoBuild -Package "rakukan-engine" -Profile $Profile -Features "rakukan-engine/vulkan"
    if (Test-Path $cpuDll) {
        Copy-Item $cpuDll (Join-Path $BuildDir "$profileDir\rakukan_engine_vulkan.dll") -Force
        Write-Host "[engine] [OK] vulkan DLL"
    }
} else {
    Write-Host "[engine] [--] VULKAN_SDK not set; skipping vulkan DLL"
}

# --- CUDA DLL ---
if ($nvcc) {
    Write-Host "[engine] Building cuda DLL..."
    Invoke-CargoBuild -Package "rakukan-engine" -Profile $Profile -Features "rakukan-engine/cuda"
    if (Test-Path $cpuDll) {
        Copy-Item $cpuDll (Join-Path $BuildDir "$profileDir\rakukan_engine_cuda.dll") -Force
        Write-Host "[engine] [OK] cuda DLL"
    }
} else {
    Write-Host "[engine] [--] nvcc not found; skipping cuda DLL"
}

$env:CMAKE_GENERATOR  = $null
$env:CARGO_TARGET_DIR = $null

if ($OwnTranscript) {
    Stop-Transcript | Out-Null
    Write-Host "[engine] Done. Log: $LogFile"
} else {
    Write-Host "[engine] Done."
}
