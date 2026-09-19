# 辞書と build.json の差し替え・中断復旧（Issue #62）。install.ps1 から読み込む。
# journal の置換を確定点にする。バックアップは復元中にも消費しない。

function Get-DictionarySwapPaths([string]$DictionaryPath) {
    $dict = [IO.Path]::GetFullPath($DictionaryPath)
    return @{
        Dict = $dict; Info = "$dict.build.json"
        NewDict = "$dict.new"; NewInfo = "$dict.new.build.json"
        BakDict = "$dict.bak"; BakInfo = "$dict.build.json.bak"
        RestoreDict = "$dict.rollback"; RestoreInfo = "$dict.build.json.rollback"
        Journal = "$dict.swap.json"; JournalTemp = "$dict.swap.json.tmp"
        Lock = "$dict.swap.lock"
    }
}

function Move-DictionarySwapFile([string]$Source, [string]$Destination) {
    # 既存ファイルを先に消さずに置換する。.NET Framework / PowerShell 5.1 でも使える。
    if (Test-Path -LiteralPath $Destination) {
        [IO.File]::Replace($Source, $Destination, [NullString]::Value)
    } else {
        [IO.File]::Move($Source, $Destination)
    }
}

function Write-DictionarySwapJournal($Paths, $State) {
    $bytes = [Text.Encoding]::UTF8.GetBytes(($State | ConvertTo-Json -Compress))
    $stream = [IO.FileStream]::new(
        $Paths.JournalTemp, [IO.FileMode]::Create, [IO.FileAccess]::Write,
        [IO.FileShare]::None, 4096, [IO.FileOptions]::WriteThrough)
    try {
        $stream.Write($bytes, 0, $bytes.Length)
        $stream.Flush($true)
    } finally {
        $stream.Dispose()
    }
    Move-DictionarySwapFile $Paths.JournalTemp $Paths.Journal
}

function Clear-DictionarySwapFiles($Paths) {
    # journal は最後まで残す。途中で削除に失敗しても、次回は同じ確定済み状態で掃除する。
    foreach ($key in @('BakDict', 'BakInfo', 'NewDict', 'NewInfo', 'RestoreDict', 'RestoreInfo', 'JournalTemp')) {
        [IO.File]::Delete($Paths[$key])
    }
    [IO.File]::Delete($Paths.Journal)
}

function Restore-DictionarySwapCore($Paths) {
    if (-not (Test-Path -LiteralPath $Paths.Journal)) {
        if ((Test-Path -LiteralPath $Paths.BakDict) -or (Test-Path -LiteralPath $Paths.BakInfo)) {
            throw 'Dictionary backups exist without a journal; cannot determine whether the swap committed. Keeping all files; inspect before retrying.'
        }
        return 'none'
    }
    $state = Get-Content -LiteralPath $Paths.Journal -Raw -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop
    if ($state.version -ne 1 -or $state.had_dict -isnot [bool] -or $state.had_info -isnot [bool] -or
        $state.phase -notin @('preparing', 'prepared', 'committed', 'rolled_back')) {
        throw "Invalid dictionary swap journal: $($Paths.Journal). Keeping all files."
    }
    $result = $state.phase
    if ($state.phase -eq 'prepared') {
        # 復元を始める前に、必要な旧ファイルが両方揃っていることを確認する。
        if (($state.had_dict -and -not (Test-Path -LiteralPath $Paths.BakDict -PathType Leaf)) -or
            ($state.had_info -and -not (Test-Path -LiteralPath $Paths.BakInfo -PathType Leaf))) {
            throw 'Dictionary swap backup is missing; keeping the journal and remaining files.'
        }
        foreach ($entry in @(
            @{ Had = $state.had_dict; Backup = $Paths.BakDict; Temp = $Paths.RestoreDict; Target = $Paths.Dict },
            @{ Had = $state.had_info; Backup = $Paths.BakInfo; Temp = $Paths.RestoreInfo; Target = $Paths.Info }
        )) {
            if ($entry.Had) {
                # バックアップはコピーして復元する。片方の復元後に中断しても再実行できる。
                [IO.File]::Copy($entry.Backup, $entry.Temp, $true)
                Move-DictionarySwapFile $entry.Temp $entry.Target
            } else {
                # 初回インストールや旧形式（メタデータなし）の元の状態も復元する。
                [IO.File]::Delete($entry.Target)
            }
        }
    }
    if ($state.phase -in @('preparing', 'prepared')) {
        # preparing の間は本体を変更していないので、復元せず退避の掃除だけでよい。
        $state.phase = 'rolled_back'
        Write-DictionarySwapJournal $Paths $state
        $result = 'rolled_back'
    }
    Clear-DictionarySwapFiles $Paths
    return $result
}

function Restore-DictionarySwap([string]$DictionaryPath) {
    $paths = Get-DictionarySwapPaths $DictionaryPath
    $guard = [IO.File]::Open($paths.Lock, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        return Restore-DictionarySwapCore $paths
    } finally {
        $guard.Dispose()
    }
}

function Install-DictionaryPair([string]$DictionaryPath) {
    $paths = Get-DictionarySwapPaths $DictionaryPath
    $guard = [IO.File]::Open($paths.Lock, [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        # 復旧は生成前に済ませる。未解決の journal / backup を新たな差し替えで消さない。
        foreach ($key in @('Journal', 'BakDict', 'BakInfo')) {
            if (Test-Path -LiteralPath $paths[$key]) {
                throw "Unresolved dictionary swap file: $($paths[$key]). Run recovery before building."
            }
        }
        if (-not (Test-Path -LiteralPath $paths.NewDict -PathType Leaf) -or
            -not (Test-Path -LiteralPath $paths.NewInfo -PathType Leaf)) {
            throw 'Both the staged dictionary and build.json are required.'
        }
        $state = [pscustomobject]@{
            version = 1; phase = 'preparing'
            had_dict = [bool](Test-Path -LiteralPath $paths.Dict -PathType Leaf)
            had_info = [bool](Test-Path -LiteralPath $paths.Info -PathType Leaf)
        }
        Write-DictionarySwapJournal $paths $state
        try {
            # コピー中の中断では本体をそのまま使える。両方揃ってから prepared にする。
            if ($state.had_dict) { [IO.File]::Copy($paths.Dict, $paths.BakDict, $false) }
            if ($state.had_info) { [IO.File]::Copy($paths.Info, $paths.BakInfo, $false) }
            $state.phase = 'prepared'
            Write-DictionarySwapJournal $paths $state
            Move-DictionarySwapFile $paths.NewDict $paths.Dict
            Move-DictionarySwapFile $paths.NewInfo $paths.Info
            $state.phase = 'committed'
            Write-DictionarySwapJournal $paths $state
        } catch {
            $swapError = $_
            try {
                $recovered = Restore-DictionarySwapCore $paths
            } catch {
                throw "Dictionary swap failed: $swapError; recovery also failed: $_. Keeping backups and journal; installation stopped."
            }
            if ($recovered -eq 'committed') {
                throw "Dictionary swap reported an error after commit: $swapError; kept the committed pair. Installation stopped."
            }
            throw "Dictionary swap failed: $swapError; restored the previous dictionary state. Installation stopped."
        }
        # 確定後の掃除失敗は rollback しない。committed の記録を残して停止する。
        Clear-DictionarySwapFiles $paths
    } finally {
        $guard.Dispose()
    }
}
