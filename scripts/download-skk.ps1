$dictDir = Join-Path $PSScriptRoot "dict"
New-Item -ItemType Directory -Force -Path $dictDir | Out-Null
$dest = Join-Path $dictDir "SKK-JISYO.L"
if (-not (Test-Path $dest)) {
    $ProgressPreference = "SilentlyContinue"
    Invoke-WebRequest -Uri "https://raw.githubusercontent.com/skk-dev/dict/master/SKK-JISYO.L" `
        -OutFile $dest -UseBasicParsing -TimeoutSec 120
}
