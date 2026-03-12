# setup-esaxx-patch.ps1
# patches/esaxx-rs/src/ を Cargo キャッシュからコピーする。
# [patch.crates-io] を使うにはソースファイルも必要なため。

$ErrorActionPreference = "Stop"

$patchDir = Join-Path $PSScriptRoot "..\patches\esaxx-rs\src"

# lib.rs が本物かどうか確認（スタブは "stub" という文字列を含む）
$libRs = Join-Path $patchDir "lib.rs"
$isStub = (Test-Path $libRs) -and ((Get-Content $libRs -Raw) -match "stub")

# esaxx.cpp があり、かつ lib.rs がスタブでない場合はスキップ
if ((Test-Path (Join-Path $patchDir "esaxx.cpp")) -and -not $isStub) {
    Write-Host "[esaxx-patch] src already present, skipping."
    exit 0
}

# Cargo レジストリキャッシュから esaxx-rs のソースを探す
$cache = "$env:USERPROFILE\.cargo\registry\src"
$esaxxSrc = Get-ChildItem -Recurse -Path $cache -Filter "esaxx.cpp" -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match "esaxx-rs" } |
    Select-Object -First 1

if (-not $esaxxSrc) {
    Write-Error "[esaxx-patch] esaxx.cpp not found in cargo cache ($cache). Run 'cargo fetch' first."
    exit 1
}

Write-Host "[esaxx-patch] Copying from: $($esaxxSrc.FullName)"

# src/ 以下のファイルを全コピー（スタブ lib.rs を上書き）
$srcDir = $esaxxSrc.Directory.FullName
New-Item -ItemType Directory -Force $patchDir | Out-Null
Copy-Item "$srcDir\*" $patchDir -Recurse -Force

Write-Host "[esaxx-patch] Done. Files in patch/src:"
Get-ChildItem $patchDir | ForEach-Object { Write-Host "  $_" }
