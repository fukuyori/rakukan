# 実インストール先・ネットワークに触れず、差し替えヘルパーの中断復旧を検証する。
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'dictionary-swap.ps1')

$workspace = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$testRoot = Join-Path $workspace ('target\dictionary-swap-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $testRoot | Out-Null
$script:passed = 0

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Assert-Throws([scriptblock]$Action) {
    $failed = $false
    try { $null = & $Action } catch { $failed = $true }
    Assert-True $failed 'Expected the operation to stop with an error'
}
function Write-Text([string]$Path, [string]$Text) {
    [IO.File]::WriteAllText($Path, $Text)
}
function Assert-Text([string]$Path, [string]$Text) {
    Assert-True ([IO.File]::ReadAllText($Path) -eq $Text) "Unexpected contents: $Path"
}
function New-Case([string]$Name) {
    $dir = Join-Path $testRoot $Name
    New-Item -ItemType Directory -Path $dir | Out-Null
    return Get-DictionarySwapPaths (Join-Path $dir 'rakukan.dict')
}
function Set-Journal($Paths, [string]$Phase, [bool]$HadDict = $true, [bool]$HadInfo = $true) {
    Write-DictionarySwapJournal $Paths ([pscustomobject]@{
        version = 1; phase = $Phase; had_dict = $HadDict; had_info = $HadInfo
    })
}
function Set-OldPair($Paths) {
    Write-Text $Paths.Dict 'old-dict'
    Write-Text $Paths.Info 'old-info'
}
function Set-NewPair($Paths) {
    Write-Text $Paths.NewDict 'new-dict'
    Write-Text $Paths.NewInfo 'new-info'
}
function Set-Backups($Paths) {
    Write-Text $Paths.BakDict 'old-dict'
    Write-Text $Paths.BakInfo 'old-info'
}
function Assert-Clean($Paths) {
    foreach ($key in @('BakDict', 'BakInfo', 'NewDict', 'NewInfo', 'RestoreDict', 'RestoreInfo', 'Journal', 'JournalTemp')) {
        Assert-True (-not (Test-Path -LiteralPath $Paths[$key])) "Unexpected leftover: $($Paths[$key])"
    }
}
function Hold-File([string]$Path) {
    return [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
}
function Pass([string]$Name) {
    $script:passed++
    Write-Host "PASS $Name"
}

try {
    $p = New-Case 'normal'
    Set-OldPair $p; Set-NewPair $p
    Install-DictionaryPair $p.Dict
    Assert-Text $p.Dict 'new-dict'; Assert-Text $p.Info 'new-info'; Assert-Clean $p
    Pass 'normal swap'

    $p = New-Case 'first-install'
    Set-NewPair $p
    Install-DictionaryPair $p.Dict
    Assert-Text $p.Dict 'new-dict'; Assert-Text $p.Info 'new-info'; Assert-Clean $p
    Pass 'first install without old pair'

    $p = New-Case 'preparing'
    Set-OldPair $p; Set-NewPair $p
    Set-Journal $p 'preparing'
    Write-Text $p.BakDict 'partial-backup'
    $result = Restore-DictionarySwap $p.Dict
    Assert-True ($result -eq 'rolled_back') 'Preparation must be abandoned'
    Assert-Text $p.Dict 'old-dict'; Assert-Text $p.Info 'old-info'; Assert-Clean $p
    Pass 'interrupted backup preparation'

    $p = New-Case 'prepared'
    Set-Backups $p; Set-Journal $p 'prepared'
    Write-Text $p.Dict 'new-dict'; Write-Text $p.Info 'old-info'
    $null = Restore-DictionarySwap $p.Dict
    Assert-Text $p.Dict 'old-dict'; Assert-Text $p.Info 'old-info'; Assert-Clean $p
    Pass 'interrupted installation before commit'

    $p = New-Case 'fresh-interrupted'
    Set-Journal $p 'prepared' $false $false
    Write-Text $p.Dict 'new-dict'
    $null = Restore-DictionarySwap $p.Dict
    Assert-True (-not (Test-Path -LiteralPath $p.Dict)) 'Partial first install remains'
    Assert-True (-not (Test-Path -LiteralPath $p.Info)) 'Unexpected metadata'
    Assert-Clean $p
    Pass 'rollback restores absence of old files'

    $p = New-Case 'legacy-interrupted'
    Set-Journal $p 'prepared' $true $false
    Write-Text $p.BakDict 'old-dict'
    Write-Text $p.Dict 'new-dict'; Write-Text $p.Info 'new-info'
    $null = Restore-DictionarySwap $p.Dict
    Assert-Text $p.Dict 'old-dict'
    Assert-True (-not (Test-Path -LiteralPath $p.Info)) 'New metadata survived rollback to legacy dictionary'
    Assert-Clean $p
    Pass 'rollback to legacy dictionary without metadata'

    foreach ($remaining in @('BakDict', 'BakInfo')) {
        $p = New-Case "committed-$remaining"
        Write-Text $p.Dict 'new-dict'; Write-Text $p.Info 'new-info'
        Set-Journal $p 'committed'
        Write-Text $p[$remaining] 'old-backup'
        $result = Restore-DictionarySwap $p.Dict
        Assert-True ($result -eq 'committed') 'Committed pair must not be rolled back'
        Assert-Text $p.Dict 'new-dict'; Assert-Text $p.Info 'new-info'; Assert-Clean $p
        Pass "interrupted committed cleanup with only $remaining remaining"
    }

    $p = New-Case 'cleanup-locked'
    Write-Text $p.Dict 'new-dict'; Write-Text $p.Info 'new-info'
    Set-Backups $p; Set-Journal $p 'committed'
    $held = Hold-File $p.BakInfo
    try {
        Assert-Throws { Restore-DictionarySwap $p.Dict }
        Assert-True (-not (Test-Path -LiteralPath $p.BakDict)) 'First backup should already be deleted'
        Assert-Text $p.BakInfo 'old-info'
        Assert-True ((Get-Content $p.Journal -Raw | ConvertFrom-Json).phase -eq 'committed') 'Commit marker lost'
    } finally { $held.Dispose() }
    $null = Restore-DictionarySwap $p.Dict
    Assert-Text $p.Dict 'new-dict'; Assert-Text $p.Info 'new-info'; Assert-Clean $p
    Pass 'cleanup failure preserves commit marker and new pair'

    $p = New-Case 'rollback-locked'
    Write-Text $p.Dict 'new-dict'; Write-Text $p.Info 'new-info'
    Set-Backups $p; Set-Journal $p 'prepared'
    $held = Hold-File $p.Info
    try {
        # 辞書の復元だけ成功して停止。再度復旧しても backup を消費しない。
        foreach ($attempt in 1..2) {
            Assert-Throws { Restore-DictionarySwap $p.Dict }
            Assert-Text $p.Dict 'old-dict'; Assert-Text $p.Info 'new-info'
            Assert-Text $p.BakDict 'old-dict'; Assert-Text $p.BakInfo 'old-info'
            Assert-True ((Get-Content $p.Journal -Raw | ConvertFrom-Json).phase -eq 'prepared') 'Recovery marker lost'
        }
        Set-NewPair $p
        Assert-Throws { Install-DictionaryPair $p.Dict }
        Assert-Text $p.BakDict 'old-dict'; Assert-Text $p.BakInfo 'old-info'
    } finally { $held.Dispose() }
    $null = Restore-DictionarySwap $p.Dict
    Assert-Text $p.Dict 'old-dict'; Assert-Text $p.Info 'old-info'; Assert-Clean $p
    Pass 'repeated interruption during rollback and refusal of a new swap'

    $p = New-Case 'actual-swap-failure'
    Set-OldPair $p; Set-NewPair $p
    $held = Hold-File $p.Info
    try {
        Assert-Throws { Install-DictionaryPair $p.Dict }
        Assert-Text $p.BakDict 'old-dict'; Assert-Text $p.BakInfo 'old-info'
        Assert-True (Test-Path -LiteralPath $p.Journal) 'Failed rollback lost journal'
    } finally { $held.Dispose() }
    $null = Restore-DictionarySwap $p.Dict
    Assert-Text $p.Dict 'old-dict'; Assert-Text $p.Info 'old-info'; Assert-Clean $p
    Pass 'actual replacement and rollback sharing failures'

    $p = New-Case 'rolled-back-cleanup'
    Set-OldPair $p; Set-Backups $p; Set-Journal $p 'rolled_back'
    [IO.File]::Delete($p.BakDict)
    $null = Restore-DictionarySwap $p.Dict
    Assert-Text $p.Dict 'old-dict'; Assert-Text $p.Info 'old-info'; Assert-Clean $p
    Pass 'interrupted cleanup after completed rollback'

    $p = New-Case 'orphan-backup'
    Set-OldPair $p; Write-Text $p.BakInfo 'ambiguous-info'
    Assert-Throws { Restore-DictionarySwap $p.Dict }
    Assert-Text $p.Dict 'old-dict'; Assert-Text $p.Info 'old-info'; Assert-Text $p.BakInfo 'ambiguous-info'
    Pass 'ambiguous backups without journal are preserved'

    $p = New-Case 'corrupt-journal'
    Set-OldPair $p; Set-Backups $p; Write-Text $p.Journal '{broken'
    Assert-Throws { Restore-DictionarySwap $p.Dict }
    Assert-Text $p.Dict 'old-dict'; Assert-Text $p.Info 'old-info'; Assert-Text $p.BakDict 'old-dict'
    Pass 'corrupt journal stops recovery without deleting files'

    Write-Host "All $script:passed dictionary swap checks passed."
} catch {
    Write-Host "Test files retained for investigation: $testRoot"
    throw
}

# このテストが作ったディレクトリだけを消す。最終パスを workspace 配下と確認する。
$resolved = [IO.Path]::GetFullPath($testRoot)
$allowed = [IO.Path]::GetFullPath((Join-Path $workspace 'target')) + [IO.Path]::DirectorySeparatorChar
if (-not $resolved.StartsWith($allowed, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Unexpected test cleanup path: $resolved"
}
Remove-Item -LiteralPath $resolved -Recurse -Force
