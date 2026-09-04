param(
    [ValidateSet("Debug", "Release")] [string]$Configuration = "Release"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path $PSScriptRoot -Parent
$project = Join-Path $repoRoot 'apps\rakukan-settings-winui\Rakukan.Settings.WinUI.csproj'
$nugetConfig = Join-Path $repoRoot 'apps\rakukan-settings-winui\NuGet.Config'

$env:APPDATA = Join-Path $repoRoot '.appdata'
$env:NUGET_PACKAGES = Join-Path $repoRoot '.nuget-packages'
New-Item -ItemType Directory -Force -Path $env:APPDATA | Out-Null
New-Item -ItemType Directory -Force -Path $env:NUGET_PACKAGES | Out-Null

# Locate MSBuild via vswhere first: hard-coded edition paths miss e.g. VS 2022 Community.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$msbuild = $null
if (Test-Path -LiteralPath $vswhere) {
    $msbuild = & $vswhere -latest -products * -requires Microsoft.Component.MSBuild -find 'MSBuild\**\Bin\amd64\MSBuild.exe' |
        Select-Object -First 1
}
if (-not $msbuild) {
    $msbuild = @(
        "C:\Program Files\Microsoft Visual Studio\18\Community\MSBuild\Current\Bin\amd64\MSBuild.exe",
        "C:\Program Files\Microsoft Visual Studio\2022\Professional\MSBuild\Current\Bin\amd64\MSBuild.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}

if (-not $msbuild) {
    throw "Visual Studio MSBuild (amd64) was not found."
}

& $msbuild $project /restore /p:RestoreConfigFile=$nugetConfig /p:Configuration=$Configuration /p:Platform=x64
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

$outputBaseDir = Join-Path $repoRoot 'apps\rakukan-settings-winui\bin\x64'
$outputDir = Join-Path $outputBaseDir $Configuration
$outputDir = Join-Path $outputDir 'net8.0-windows10.0.19041.0\win-x64'
if (-not (Test-Path $outputDir)) {
    throw "WinUI build output not found: $outputDir"
}

Write-Host "WinUI settings output: $outputDir"
