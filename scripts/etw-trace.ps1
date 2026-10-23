<#
.SYNOPSIS
    調査用の ETW セッションを開始・停止し、記録をテキストへ書き出す（#76）。

.DESCRIPTION
    rakukan は、ファイルのログを主にし、ETW は調査のときだけ使う。常時のセッションは作らない。
    このスクリプトで、TSF（Rakukan.Tsf）とエンジンホスト（Rakukan.Host。engine DLL の
    イベントを含む）の provider を集めるセッションを、手で開始・停止する。

    セッションの開始・停止には、通常は管理者か Performance Log Users の権限が要る。
    記録は循環 ETL（既定 64 MB）で、古いものから上書きされる。

    出す範囲はファイルのログと同じ（ログレベルの指定を通ったイベント）。TSF の既定の
    レベルは debug で、入力の内容（読みなど）を含むイベントもある。記録の扱いに注意する。

    Windows に標準で入っている logman（セッション）と tracerpt（書き出し）を使う。

.PARAMETER Action
    start  : セッションを開始する。同じ名前のセッションが動いていれば止まる（-Force で作り直す）
    stop   : セッションを停止する
    status : セッションの状態を表示する
    export : ETL を CSV と要約（欠落したイベントの数を含む）へ書き出す

.PARAMETER OutDir
    ETL と書き出しの置き場所。既定は %LOCALAPPDATA%\rakukan\etw。

.PARAMETER MaxMB
    循環 ETL の大きさ（MB）。既定 64。

.PARAMETER Level
    集めるレベル。1=Critical 2=Error 3=Warning 4=Information 5=Verbose（既定 5）。

.PARAMETER Force
    start のとき、既に動いている同じ名前のセッションを止めてから開始する。

.EXAMPLE
    # 管理者の PowerShell で
    .\scripts\etw-trace.ps1 start
    # （事象を再現する）
    .\scripts\etw-trace.ps1 stop
    .\scripts\etw-trace.ps1 export
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [ValidateSet('start', 'stop', 'status', 'export')]
    [string]$Action,
    [string]$OutDir = "$env:LOCALAPPDATA\rakukan\etw",
    [ValidateRange(1, 4096)]
    [int]$MaxMB = 64,
    [ValidateRange(1, 5)]
    [int]$Level = 5,
    [switch]$Force
)

$ErrorActionPreference = 'Stop'

$SessionName = 'rakukan-etw'
# provider の GUID は名前から決まる（crates/rakukan-log/src/etw.rs の *_PROVIDER_GUID と同じ値）
$Providers = [ordered]@{
    'Rakukan.Tsf'  = '{d28ecef7-3b1a-5580-2ec2-9e71c758c5ef}'
    'Rakukan.Host' = '{f3736713-19a4-5666-aab9-bd2a1ec6513c}'
}
$EtlPath = Join-Path $OutDir 'rakukan.etl'

# logman の「Data Collector Set was not found」（PLA_E_DCS_NOT_FOUND）
$NotFound = -2144337918   # 0x80300002

# セッションが動いていれば $true、無ければ $false。それ以外の失敗（権限不足など）は止める
function Test-SessionRunning {
    $output = & logman query $SessionName -ets 2>&1
    if ($LASTEXITCODE -eq 0) { return $true }
    if ($LASTEXITCODE -eq $NotFound) { return $false }
    throw ("logman query が失敗しました（終了コード {0}）。管理者の PowerShell で実行してください。`n{1}" -f $LASTEXITCODE, ($output -join "`n"))
}

switch ($Action) {
    'start' {
        if (Test-SessionRunning) {
            if (-not $Force) {
                throw "セッション $SessionName は既に動いています。止めるには stop、作り直すには -Force を付けてください。"
            }
            & logman stop $SessionName -ets | Out-Null
            if ($LASTEXITCODE -ne 0) { throw "動いているセッションを止められませんでした（logman stop の終了コード $LASTEXITCODE）。" }
        }
        New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
        $providerFile = Join-Path $OutDir 'providers.txt'
        # 1 行に 1 provider: GUID、キーワード（すべて）、レベル
        $lines = foreach ($guid in $Providers.Values) { "$guid 0xffffffffffffffff $Level" }
        Set-Content -LiteralPath $providerFile -Value $lines -Encoding ascii
        & logman start $SessionName -pf $providerFile -o $EtlPath -mode Circular -max $MaxMB -ets
        if ($LASTEXITCODE -ne 0) {
            throw "logman start が失敗しました（終了コード $LASTEXITCODE）。管理者の PowerShell で実行してください。"
        }
        Write-Host "開始しました: $SessionName -> $EtlPath （循環 $MaxMB MB、レベル $Level）"
    }
    'stop' {
        if (-not (Test-SessionRunning)) {
            Write-Host "セッション $SessionName は動いていません。"
            return
        }
        & logman stop $SessionName -ets
        if ($LASTEXITCODE -ne 0) { throw "logman stop が失敗しました（終了コード $LASTEXITCODE）。" }
        Write-Host "停止しました: $EtlPath"
    }
    'status' {
        if (Test-SessionRunning) {
            & logman query $SessionName -ets
            if ($LASTEXITCODE -ne 0) { throw "logman query が失敗しました（終了コード $LASTEXITCODE）。" }
        } else {
            Write-Host "セッション $SessionName は動いていません。"
        }
    }
    'export' {
        if (Test-SessionRunning) {
            throw "セッション $SessionName が動いています。先に stop してください。"
        }
        if (-not (Test-Path -LiteralPath $EtlPath)) { throw "ETL がありません: $EtlPath" }
        $stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
        $csv = Join-Path $OutDir "rakukan-$stamp.csv"
        $summary = Join-Path $OutDir "rakukan-$stamp-summary.txt"
        & tracerpt $EtlPath -o $csv -of CSV -summary $summary -y
        if ($LASTEXITCODE -ne 0) { throw "tracerpt が失敗しました（終了コード $LASTEXITCODE）。" }
        Write-Host "書き出しました: $csv"
        # 欠落したイベントの数は要約に出る
        Select-String -LiteralPath $summary -Pattern 'Lost|失われ' | ForEach-Object { Write-Host $_.Line }
        Write-Host "要約: $summary"
    }
}
exit 0
