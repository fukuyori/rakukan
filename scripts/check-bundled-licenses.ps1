<#
.SYNOPSIS
    設定アプリ（settings-ui）が使う NuGet のパッケージと、配布するファイルのライセンスを確かめる。

.DESCRIPTION
    許可の一覧は docs/licenses/bundled-components.json。

    いつも行うこと（CI でも行う）:
      - 設定アプリの obj/project.assets.json にある NuGet のパッケージ（間接の依存を含む）が、
        許可の一覧に同じ版で載っているか
      - 配布物に入るパッケージの見出し（section）が THIRD_PARTY_LICENSES.md にあるか

    -Files を付けたとき（配布物を作るとき。build-installer.ps1 が呼ぶ）:
      - .NET の参照パック・apphost（downloadDependencies）が許可の一覧にあるか
      - 配布する各ファイルを、NuGet のパッケージの中のファイルと名前とハッシュで突き合わせ、
        どのパッケージから来たかを決める。自前のファイル（ownFiles）を除き、来たところが
        分からないファイルや、配布してよいとしていないパッケージのファイルがあれば失敗する

    前提: 設定アプリを restore 済みであること（scripts/build-settings-winui.ps1、または
    dotnet restore apps/rakukan-settings-winui/Rakukan.Settings.WinUI.csproj）。

.PARAMETER Files
    配布する settings-ui のディレクトリ。

.EXAMPLE
    .\scripts\check-bundled-licenses.ps1
    .\scripts\check-bundled-licenses.ps1 -Files dist\settings-ui
#>
[CmdletBinding()]
param(
    [string]$Files
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$allowPath = Join-Path $repoRoot 'docs/licenses/bundled-components.json'
$thirdParty = Join-Path $repoRoot 'docs/THIRD_PARTY_LICENSES.md'
$assetsPath = Join-Path $repoRoot 'apps/rakukan-settings-winui/obj/project.assets.json'

if (-not (Test-Path -LiteralPath $assetsPath)) {
    throw "project.assets.json がありません。先に設定アプリを restore してください: $assetsPath"
}
$allow = Get-Content -LiteralPath $allowPath -Raw -Encoding utf8 | ConvertFrom-Json
$assets = Get-Content -LiteralPath $assetsPath -Raw -Encoding utf8 | ConvertFrom-Json
$docLines = Get-Content -LiteralPath $thirdParty -Encoding utf8
$errors = [System.Collections.Generic.List[string]]::new()

function Find-Allowed($list, [string]$id, [string]$version) {
    return $list | Where-Object { $_.id -ieq $id -and ($_.version -eq '*' -or $_.version -eq $version) } | Select-Object -First 1
}

# NuGet のパッケージ（間接の依存を含む）
$libraries = foreach ($key in $assets.libraries.PSObject.Properties.Name) {
    $id, $version = $key -split '/', 2
    [pscustomobject]@{ Id = $id; Version = $version }
}
foreach ($lib in $libraries) {
    if (-not (Find-Allowed $allow.packages $lib.Id $lib.Version)) {
        $errors.Add("許可の一覧に無いパッケージ・版: $($lib.Id) $($lib.Version)（ライセンスを確かめて docs/licenses/bundled-components.json に足す）")
    }
}

# 配布物に入るものの見出しが THIRD_PARTY_LICENSES.md にあるか
foreach ($entry in @($allow.packages) + @($allow.frameworkPackages)) {
    if ($entry.section -and -not ($docLines -contains $entry.section)) {
        $errors.Add("THIRD_PARTY_LICENSES.md に見出しがありません: '$($entry.section)'（$($entry.id)）")
    }
}

if ($Files) {
    # .NET の参照パック・apphost
    $framework = foreach ($fw in $assets.project.frameworks.PSObject.Properties.Value) {
        foreach ($dep in @($fw.downloadDependencies)) {
            if (-not $dep) { continue }
            $version = ($dep.version -replace '[\[\]\s]', '' -split ',')[0]
            [pscustomobject]@{ Id = $dep.name; Version = $version }
        }
    }
    foreach ($fp in $framework) {
        if (-not (Find-Allowed $allow.frameworkPackages $fp.Id $fp.Version)) {
            $errors.Add("許可の一覧に無い .NET のパック・版: $($fp.Id) $($fp.Version)")
        }
    }

    # 配布する各ファイルの名前。突き合わせは、この名前のものだけに絞る
    $root = (Resolve-Path -LiteralPath $Files).Path
    $shipped = Get-ChildItem -LiteralPath $root -Recurse -File
    $wanted = @{}
    foreach ($f in $shipped) { $wanted[$f.Name.ToLowerInvariant()] = $true }

    # パッケージの中のファイルを、名前で引けるようにする。自己完結型の Windows App SDK の部品は
    # パッケージの中の .msix（zip）から取り出されるので、.msix の中身も対象にする
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $folders = @($assets.packageFolders.PSObject.Properties.Name) + @(
        (Join-Path $repoRoot '.nuget-packages'), $env:NUGET_PACKAGES
    ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -Unique
    $byName = @{}
    # Get-FileHash は使わない。cargo make 経由で Windows PowerShell 5.1 を起動すると、PowerShell 7 の
    # モジュールの場所を先に引き継ぎ、5.1 の Get-FileHash（モジュールのスクリプトで定義）が見つからない
    function Get-Sha256([string]$path) {
        $stream = [System.IO.File]::OpenRead($path)
        try { return [System.BitConverter]::ToString($sha.ComputeHash($stream)) -replace '-', '' }
        finally { $stream.Dispose() }
    }
    function Add-Candidate([string]$name, $pkg, [string]$path, [string]$hash) {
        $key = $name.ToLowerInvariant()
        if (-not $byName.ContainsKey($key)) { $byName[$key] = [System.Collections.Generic.List[object]]::new() }
        $byName[$key].Add([pscustomobject]@{ Id = $pkg.Id; Version = $pkg.Version; Path = $path; Hash = $hash })
    }
    foreach ($pkg in @($libraries) + @($framework)) {
        $dir = $folders | ForEach-Object { Join-Path $_ "$($pkg.Id.ToLowerInvariant())/$($pkg.Version)" } |
            Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
        if (-not $dir) { continue }
        foreach ($f in Get-ChildItem -LiteralPath $dir -Recurse -File) {
            if ($wanted.ContainsKey($f.Name.ToLowerInvariant())) { Add-Candidate $f.Name $pkg $f.FullName $null }
            if ($f.Extension -ieq '.msix') {
                $zip = [System.IO.Compression.ZipFile]::OpenRead($f.FullName)
                try {
                    foreach ($e in $zip.Entries) {
                        $leaf = [System.Uri]::UnescapeDataString($e.Name)
                        if (-not $leaf -or -not $wanted.ContainsKey($leaf.ToLowerInvariant())) { continue }
                        $stream = $e.Open()
                        try { $hash = [System.BitConverter]::ToString($sha.ComputeHash($stream)) -replace '-', '' }
                        finally { $stream.Dispose() }
                        Add-Candidate $leaf $pkg "$($f.FullName)!$($e.FullName)" $hash
                    }
                } finally { $zip.Dispose() }
            }
        }
    }

    $counts = @{}
    foreach ($f in $shipped) {
        $rel = $f.FullName.Substring($root.Length).TrimStart('\', '/')
        if ($allow.ownFiles | Where-Object { $f.Name -like $_ }) {
            $counts['(rakukan)'] = 1 + [int]$counts['(rakukan)']
            continue
        }
        $candidates = $byName[$f.Name.ToLowerInvariant()]
        $origin = $null
        if ($candidates) {
            $hash = Get-Sha256 $f.FullName
            foreach ($c in $candidates) {
                if (-not $c.Hash) { $c.Hash = Get-Sha256 $c.Path }
                if ($c.Hash -eq $hash) { $origin = $c; break }
            }
        }
        if (-not $origin) {
            $errors.Add("来たところが分からないファイル: $rel")
            continue
        }
        $entry = Find-Allowed (@($allow.packages) + @($allow.frameworkPackages)) $origin.Id $origin.Version
        if (-not $entry -or -not $entry.section) {
            $errors.Add("配布してよいとしていないパッケージのファイル: $rel（$($origin.Id) $($origin.Version)）")
            continue
        }
        $counts["$($origin.Id) $($origin.Version)"] = 1 + [int]$counts["$($origin.Id) $($origin.Version)"]
    }
    foreach ($k in ($counts.Keys | Sort-Object)) { Write-Host ("  {0,-45} {1,5} ファイル" -f $k, $counts[$k]) }
}

if ($errors.Count -gt 0) {
    $errors | ForEach-Object { Write-Host "NG: $_" }
    exit 1
}
Write-Host "OK: 設定アプリの NuGet のパッケージ $(@($libraries).Count) 個$(if ($Files) { '・配布するファイル' }) は許可の一覧と合っています。"
exit 0
