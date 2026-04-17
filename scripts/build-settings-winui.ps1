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

$msbuild = @(
    "C:\Program Files\Microsoft Visual Studio\18\Community\MSBuild\Current\Bin\amd64\MSBuild.exe",
    "C:\Program Files\Microsoft Visual Studio\2022\Professional\MSBuild\Current\Bin\amd64\MSBuild.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1

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
