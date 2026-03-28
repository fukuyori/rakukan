param(
    [string]$InputDir = "$PSScriptRoot\..\assets\vibrato\mecab-ipadic-2.7.0-20070801",
    [string]$Output = "$PSScriptRoot\..\assets\vibrato\system.dic"
)

$ErrorActionPreference = "Stop"

Write-Host "[1/2] Building Vibrato dictionary..."
cargo run -p rakukan-vibrato-builder --release -- `
    --input-dir $InputDir `
    --output $Output

Write-Host "[2/2] Done"
Get-Item $Output | Select-Object FullName, Length, LastWriteTime
