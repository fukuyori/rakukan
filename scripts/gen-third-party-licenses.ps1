<#
.SYNOPSIS
    docs/THIRD_PARTY_LICENSES.md の「Rust crates」の節を作り直す。

.DESCRIPTION
    配布物（rakukan_tsf.dll・rakukan-engine-host.exe・rakukan_engine_*.dll・rakukan-tray.exe）に
    リンクする Rust の crate のライセンスを cargo-about で集め、THIRD_PARTY_LICENSES.md の
    次の 2 行の間を置き換える。ほかの節（手で書く節）には触れない。

        <!-- BEGIN GENERATED: Rust crates -->
        <!-- END GENERATED: Rust crates -->

    集める範囲は about.toml（Windows 向け、通常の依存。ビルド用・開発用の依存は除く）。
    rakukan 自身の crate は載せない。依存を変えたら実行する（docs/version-update-checklist.md）。

    crate にライセンスのファイルが無いと、cargo-about は著作権者の名前の無い標準の本文で埋める。
    著作権の表示が条件のライセンス（MIT・BSD・ISC）でそうなる crate は、上流の LICENSE を
    crate を作ったコミットで取り、docs/licenses/<名前>-<版>.txt に置く。この本文を使う。
    置いていない crate があれば警告し、-Check では失敗にする。

    前提: cargo-about 0.9.2（cargo install cargo-about --locked --version 0.9.2 --features cli）。

.PARAMETER Check
    書き換えずに、今のファイルと生成した内容が同じかだけを確かめる（違えば終了コード 1）。
#>
[CmdletBinding()]
param(
    [switch]$Check
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$target = Join-Path $repoRoot 'docs\THIRD_PARTY_LICENSES.md'
$begin = '<!-- BEGIN GENERATED: Rust crates -->'
$end = '<!-- END GENERATED: Rust crates -->'

# 配布物になる crate（トレイは Step 2 から配布する）
$roots = @('rakukan-tsf', 'rakukan-engine-host', 'rakukan-engine', 'rakukan-tray')

# 上流の LICENSE を置く場所と、著作権の表示が条件のライセンス
$overrideDir = Join-Path $repoRoot 'docs\licenses'
$attribution = @('MIT', 'BSD-2-Clause', 'BSD-3-Clause', 'ISC')
$missing = [System.Collections.Generic.SortedSet[string]]::new()

if (-not (Get-Command cargo-about -ErrorAction SilentlyContinue)) {
    throw 'cargo-about が見つかりません: cargo install cargo-about --locked --version 0.9.2 --features cli'
}

$work = Join-Path ([System.IO.Path]::GetTempPath()) "rakukan-about-$PID"
New-Item -ItemType Directory -Force -Path $work | Out-Null
try {
    $crates = @{}    # "name version" -> @{ Name; Version; License; Repository }
    $texts = @{}     # "id`n本文" -> @{ Id; Name; Text; Users = HashSet }
    foreach ($root in $roots) {
        $json = Join-Path $work "$root.json"
        $manifest = Join-Path $repoRoot "crates\$root\Cargo.toml"
        & cargo about generate --format json -c (Join-Path $repoRoot 'about.toml') -m $manifest -o $json 2>&1 |
            Where-Object { $_ -match 'ERROR|error:' } | ForEach-Object { Write-Warning $_ }
        if ($LASTEXITCODE -ne 0) { throw "cargo about generate が失敗しました: $root" }
        $data = Get-Content -LiteralPath $json -Raw -Encoding utf8 | ConvertFrom-Json
        foreach ($c in $data.crates) {
            $p = $c.package
            if ($p.name -like 'rakukan-*') { continue }
            $key = "$($p.name) $($p.version)"
            if (-not $crates.ContainsKey($key)) {
                $crates[$key] = @{ Name = $p.name; Version = $p.version; License = $c.license; Repository = $p.repository }
            }
        }
        foreach ($l in $data.licenses) {
            $users = @($l.used_by | ForEach-Object { $_.crate } | Where-Object { $_.name -notlike 'rakukan-*' })
            if ($users.Count -eq 0) { continue }
            foreach ($u in $users) {
                $text = $l.text
                if (-not $l.source_path) {
                    # 標準の本文で埋められた: 上流の LICENSE があればそれを使う
                    $override = Join-Path $overrideDir "$($u.name)-$($u.version).txt"
                    if (Test-Path -LiteralPath $override) {
                        $text = Get-Content -LiteralPath $override -Raw -Encoding utf8
                    } elseif ($attribution -contains $l.id) {
                        [void]$missing.Add("$($u.name) $($u.version) ($($l.id))")
                    }
                }
                $key = "$($l.id)`n$text"
                if (-not $texts.ContainsKey($key)) {
                    $texts[$key] = @{ Id = $l.id; Name = $l.name; Text = $text; Users = [System.Collections.Generic.SortedSet[string]]::new() }
                }
                [void]$texts[$key].Users.Add("$($u.name) $($u.version)")
            }
        }
    }
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

$sb = [System.Text.StringBuilder]::new()
[void]$sb.AppendLine($begin)
[void]$sb.AppendLine('<!-- scripts/gen-third-party-licenses.ps1 が生成する。手で直さない -->')
[void]$sb.AppendLine()
[void]$sb.AppendLine('## Rust Crates')
[void]$sb.AppendLine()
[void]$sb.AppendLine('The following Rust crates are linked into `rakukan_tsf.dll`, `rakukan-engine-host.exe`, `rakukan_engine_*.dll` and `rakukan-tray.exe`.')
[void]$sb.AppendLine()
[void]$sb.AppendLine('| Crate | Version | License | Source |')
[void]$sb.AppendLine('|---|---|---|---|')
foreach ($c in ($crates.Values | Sort-Object { $_.Name }, { $_.Version })) {
    $repo = if ($c.Repository) { $c.Repository } else { '' }
    [void]$sb.AppendLine("| $($c.Name) | $($c.Version) | $($c.License) | $repo |")
}
[void]$sb.AppendLine()
[void]$sb.AppendLine('### License Texts')
foreach ($t in ($texts.Values | Sort-Object { $_.Id }, { ($_.Users | Select-Object -First 1) }, { $_.Text })) {
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("#### $($t.Name) ($($t.Id))")
    [void]$sb.AppendLine()
    [void]$sb.AppendLine("Used by: $([string]::Join(', ', $t.Users))")
    [void]$sb.AppendLine()
    [void]$sb.AppendLine('```')
    # 行末の空白は除く（git diff --check に掛からないように。本文の意味は変わらない）
    $body = ($t.Text.TrimEnd() -split "`r?`n" | ForEach-Object { $_.TrimEnd() }) -join "`n"
    [void]$sb.AppendLine($body)
    [void]$sb.AppendLine('```')
}
[void]$sb.AppendLine()
[void]$sb.Append($end)
$generated = $sb.ToString() -replace "`r`n", "`n"

$current = (Get-Content -LiteralPath $target -Raw -Encoding utf8) -replace "`r`n", "`n"
$bi = $current.IndexOf($begin)
$ei = $current.IndexOf($end)
if ($bi -lt 0 -or $ei -lt $bi) { throw "$target に $begin と $end がありません" }
$updated = $current.Substring(0, $bi) + $generated + $current.Substring($ei + $end.Length)

if ($missing.Count -gt 0) {
    Write-Warning ("著作権者の名前の無い本文になっている crate があります。上流の LICENSE を docs/licenses/<名前>-<版>.txt に置いてください: " + [string]::Join(', ', $missing))
}

if ($Check) {
    if ($missing.Count -gt 0) { exit 1 }
    if ($updated -ne $current) {
        Write-Host 'THIRD_PARTY_LICENSES.md が依存と合っていません。scripts/gen-third-party-licenses.ps1 を実行してください。'
        exit 1
    }
    Write-Host "THIRD_PARTY_LICENSES.md は依存と合っています（crate $($crates.Count) 個）。"
    exit 0
}
[System.IO.File]::WriteAllText($target, $updated, [System.Text.UTF8Encoding]::new($false))
Write-Host "更新しました: $target （crate $($crates.Count) 個、ライセンスの本文 $($texts.Count) 種）"
