# esaxx-rs の build.rs を確認する
$cache = "$env:USERPROFILE\.cargo\registry\src"
$buildrs = Get-ChildItem -Recurse -Path $cache -Filter "build.rs" | 
    Where-Object { $_.FullName -match "esaxx" }
if ($buildrs) {
    Write-Host "=== esaxx-rs build.rs ==="
    Get-Content $buildrs[0].FullName
} else {
    Write-Host "esaxx-rs build.rs not found in cache"
}
