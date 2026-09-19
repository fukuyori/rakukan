<#
.SYNOPSIS
    TSF のプロセス別ログを時刻順に統合する（Issue #60）。

.DESCRIPTION
    TSF はアプリごとに別プロセスで動くため、ログは起動インスタンスごとの
    rakukan-tsf-<PID>-<起動識別子>.log に分かれる。ある時刻に何が起きたかを
    追うには、複数のファイルを時刻順に並べ直す必要がある。

    このスクリプトは対象のログを読み、各行の先頭にある UTC タイムスタンプで
    並べ替えて 1 本にする。どの行がどのプロセスの、どのファイルから来たかが
    分かるよう、行の先頭に [#索引:PID] を付け、索引と元ファイル名の対応を
    先頭の凡例に出す。

    タイムスタンプを持たない行（パニックのバックトレースなど）は、直前の行の
    時刻を引き継いで同じ位置に残す。

.PARAMETER LogDir
    ログのあるディレクトリ。既定は %LOCALAPPDATA%\rakukan。

.PARAMETER OutFile
    書き出し先。省略すると標準出力へ流す。

.PARAMETER Since
    この時刻以降の行だけを出す（UTC）。例: '2026-09-18T01:00:00'

.PARAMETER Until
    この時刻以前の行だけを出す（UTC）。

.PARAMETER ProcessId
    この PID のログだけを対象にする。複数指定できる。

.PARAMETER IncludeLegacy
    旧方式の rakukan.log / rakukan.log.N も対象に含める。

.EXAMPLE
    .\scripts\merge-logs.ps1 -OutFile merged.log

.EXAMPLE
    .\scripts\merge-logs.ps1 -Since '2026-09-18T01:27' -Until '2026-09-18T06:21'
#>
[CmdletBinding()]
param(
    [string]$LogDir = "$env:LOCALAPPDATA\rakukan",
    [string]$OutFile,
    [string]$Since,
    [string]$Until,
    [int[]]$ProcessId,
    [switch]$IncludeLegacy
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path -LiteralPath $LogDir)) {
    throw "ログのディレクトリが見つかりません: $LogDir"
}

# 対象ファイルを集める。.lock は除く。
$patterns = @('rakukan-tsf-*.log', 'rakukan-tsf-*.log.*')
if ($IncludeLegacy) { $patterns += @('rakukan.log', 'rakukan.log.*') }

$files = foreach ($p in $patterns) {
    Get-ChildItem -LiteralPath $LogDir -Filter $p -File -ErrorAction SilentlyContinue
}
$files = $files | Sort-Object -Property Name -Unique

if ($ProcessId) {
    $files = $files | Where-Object {
        if ($_.Name -match '^rakukan-tsf-(\d+)-') { [int]$Matches[1] -in $ProcessId } else { $false }
    }
}

if (-not $files) {
    throw "対象のログがありません: $LogDir"
}

# 行の先頭の UTC タイムスタンプ（例: 2026-09-18T01:27:52.424529Z）
$stampPattern = '^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?)Z?'

# 比較は小数部を 6 桁へ揃えた固定幅の文字列で行う。
# 文字列のまま比べると、'...:52.000000' と '...:52Z' のように桁数が違う場合に
# 大小が入れ替わり、指定した範囲の行が落ちる。
function ConvertTo-LogKey([string]$stamp) {
    $dot = $stamp.IndexOf('.')
    if ($dot -lt 0) { return $stamp + '.000000' }
    $frac = $stamp.Substring($dot + 1)
    if ($frac.Length -gt 6) { $frac = $frac.Substring(0, 6) }
    return $stamp.Substring(0, $dot) + '.' + $frac.PadRight(6, '0')
}

# -Since / -Until は日時として解釈し、UTC に揃えてから同じ形にする。
# 'Z' 付き・時分まで・ローカル時刻のオフセット付き、いずれも受け付ける。
function ConvertTo-BoundKey([string]$text) {
    $styles = [System.Globalization.DateTimeStyles]::AssumeUniversal -bor `
              [System.Globalization.DateTimeStyles]::AdjustToUniversal
    $dt = [datetime]::Parse($text, [cultureinfo]::InvariantCulture, $styles)
    return $dt.ToString('yyyy-MM-ddTHH:mm:ss.ffffff', [cultureinfo]::InvariantCulture)
}

$sinceKey = if ($Since) { ConvertTo-BoundKey $Since } else { $null }
$untilKey = if ($Until) { ConvertTo-BoundKey $Until } else { $null }

$records = [System.Collections.Generic.List[object]]::new()
$legend = [System.Collections.Generic.List[object]]::new()
$index = 0

foreach ($f in $files) {
    $index++
    $filePid = if ($f.Name -match '^rakukan-tsf-(\d+)-') { $Matches[1] } else { '-' }
    $first = $null
    $last = $null
    $count = 0
    # タイムスタンプを持たない行は直前の行の時刻を引き継ぐ。
    # 同じ時刻の中での元の並びを保つため、ファイル内の行番号を第 2 キーにする。
    $carried = '0000-00-00T00:00:00.000000'
    $lineNo = 0
    # 稼働中のプロセスが書き込み中でも読めるよう、共有を許して開く。
    $stream = [System.IO.FileStream]::new(
        $f.FullName,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        ([System.IO.FileShare]::ReadWrite -bor [System.IO.FileShare]::Delete))
    $reader = [System.IO.StreamReader]::new($stream, [System.Text.Encoding]::UTF8)
    try {
    while ($null -ne ($line = $reader.ReadLine())) {
        $lineNo++
        if ($line -match $stampPattern) { $carried = ConvertTo-LogKey $Matches[1] }
        if ($sinceKey -and $carried -lt $sinceKey) { continue }
        if ($untilKey -and $carried -gt $untilKey) { continue }
        $records.Add([pscustomobject]@{
            Key    = $carried
            Source = $index
            Line   = $lineNo
            Text   = "[#${index}:${filePid}] $line"
        })
        $count++
        if (-not $first) { $first = $carried }
        $last = $carried
    }
    } finally {
        $reader.Dispose()
        $stream.Dispose()
    }
    $legend.Add([pscustomobject]@{
        Index = "#$index"
        Pid   = $filePid
        File  = $f.Name
        Lines = $count
        From  = $first
        To    = $last
    })
}

$sorted = $records | Sort-Object -Property Key, Source, Line

$header = @()
$header += "# rakukan ログ統合  生成: $(Get-Date -Format 'yyyy-MM-ddTHH:mm:ssK')"
$header += "# 元ディレクトリ: $LogDir"
if ($sinceKey) { $header += "# Since: $sinceKey (UTC)" }
if ($untilKey) { $header += "# Until: $untilKey (UTC)" }
$header += "#"
$header += "# 索引  PID     行数      範囲（UTC）                              元ファイル"
foreach ($l in ($legend | Where-Object { $_.Lines -gt 0 })) {
    $from = if ($l.From) { $l.From } else { '-' }
    $to = if ($l.To) { $l.To } else { '-' }
    $header += ("# {0,-4} {1,-7} {2,8}  {3} .. {4}  {5}" -f `
        $l.Index, $l.Pid, $l.Lines, $from, $to, $l.File)
}
$header += "#"

$out = @($header) + @($sorted | ForEach-Object { $_.Text })

if ($OutFile) {
    $out | Set-Content -LiteralPath $OutFile -Encoding utf8
    Write-Host "書き出しました: $OutFile （$($sorted.Count) 行 / $(($legend | Where-Object { $_.Lines -gt 0 }).Count) ファイル）"
} else {
    $out
}
