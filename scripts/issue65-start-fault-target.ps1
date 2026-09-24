# scripts\issue65-start-fault-target.ps1 - Issue #65 故障試験の対象アプリを起動する
#
# 試験用ビルド（cargo make install-tsf-fault-test で導入した rakukan_tsf.dll）だけが
# 故障制御を読む。通常版では何も起きない（build_kind=normal のまま）。
#
# 流れ:
#   1. 固有の識別子と制御ファイルのパスを環境変数に載せて対象アプリを起動する
#   2. 対象の入力プロセス（パッケージ版 Notepad は起動プロセスとは別 PID）を待つ
#   3. 識別子・PID・故障フラグを制御ファイルへ書く（TSF 側は DLL 読込から最大 5 秒待つ）
#   4. 対象 PID の TSF ログに "fault control armed" が出るかを待ち、結果を表示する
#
# 解除: 表示された CONTROL のファイルを削除する（TSF 側は 1 秒間隔で確認）。
#       放置しても有効化から 90 秒で自動解除される。
#
# 対象アプリの既定は charmap.exe（System32 の従来型 Win32 アプリ。文字入力欄があり TSF DLL が読み込まれる）。
# ストア版 Notepad / EmEditor などのパッケージアプリは、この方法（exe を直接 Process.Start）では
# 2026-09-24 の確認で窓が作られず、TSF DLL も読み込まれなかった。パッケージアプリを対象にする
# 場合は起動経路が別に要る（環境変数の継承も未確認）。
#
# 例:
#   .\scripts\issue65-start-fault-target.ps1                        # charmap.exe、通知遮断
#   .\scripts\issue65-start-fault-target.ps1 -Fault fail_dir_watch
#   .\scripts\issue65-start-fault-target.ps1 -AppPath 'C:\path\to\classic-app.exe'
param(
    [ValidateSet('suppress_notifications', 'fail_save_event', 'fail_request_event', 'fail_dir_watch', 'fail_rearm_once')]
    [string]$Fault = 'suppress_notifications',
    # 起動するアプリ（従来型 Win32 アプリ）。省略時は System32 の charmap.exe
    [string]$AppPath = '',
    # 対象の入力プロセス名（拡張子なし）。省略時は AppPath のファイル名
    [string]$ProcessName = '',
    # 入力プロセスの出現を待つ秒数
    [int]$ProcessWaitSeconds = 5,
    # TSF ログで有効化を待つ秒数（0 で待たない）
    [int]$ArmedWaitSeconds = 15
)

$ErrorActionPreference = 'Stop'

if (-not $AppPath) {
    $AppPath = Join-Path $env:WINDIR 'System32\charmap.exe'
}
if (-not (Test-Path -LiteralPath $AppPath)) { throw "App not found: $AppPath" }
if (-not $ProcessName) { $ProcessName = [IO.Path]::GetFileNameWithoutExtension($AppPath) }

# 既存の同名プロセスがあると、どれが対象か決められない
if (Get-Process -Name $ProcessName -ErrorAction SilentlyContinue) {
    throw "Close all '$ProcessName' processes before starting the trial"
}

$id = [guid]::NewGuid().ToString('N')
$control = Join-Path $env:TEMP "rakukan-watch-$id.toml"

$start = [Diagnostics.ProcessStartInfo]::new($AppPath)
$start.UseShellExecute = $false
$start.EnvironmentVariables['RAKUKAN_CONFIG_WATCH_TEST_CONTROL'] = $control
$start.EnvironmentVariables['RAKUKAN_CONFIG_WATCH_TEST_ID'] = $id
$launcher = [Diagnostics.Process]::Start($start)
if (-not $launcher) { throw 'Target app did not start' }

# 入力プロセスを決める。従来型のアプリは起動プロセス自身が対象になる。
# 起動プロセスが別の同名プロセスへ引き継いで終了する形のアプリに備えて、
# 待ち時間内に他の同名プロセスが現れればそれを使う。
$deadline = [DateTime]::UtcNow.AddSeconds($ProcessWaitSeconds)
$target = $null
do {
    Start-Sleep -Milliseconds 100
    $others = @(Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Where-Object Id -ne $launcher.Id)
    if ($others.Count -gt 0) { $target = $others[0]; break }
    if ($launcher.HasExited) { break }
} until ([DateTime]::UtcNow -ge $deadline)
if (-not $target) {
    if (-not $launcher.HasExited -and $launcher.ProcessName -eq $ProcessName) {
        $target = $launcher
    } else {
        throw "Input process '$ProcessName' did not appear within $ProcessWaitSeconds s"
    }
}

$body = "id = '$id'`npid = $($target.Id)`n$Fault = true`n"
[IO.File]::WriteAllText($control, $body, [Text.UTF8Encoding]::new($false))
Write-Host "PID=$($target.Id) CONTROL=$control FAULT=$Fault"
Write-Host "Release: Remove-Item -LiteralPath '$control'"

if ($ArmedWaitSeconds -le 0) { exit 0 }

# 対象 PID の TSF ログ（rakukan-tsf-<PID>-<起動識別子>.log）で有効化を待つ。
# 稼働中のログは読み取り共有で開く。一覧のサイズ・更新日時では判断しない。
$logDir = Join-Path $env:LOCALAPPDATA 'rakukan'
$deadline = [DateTime]::UtcNow.AddSeconds($ArmedWaitSeconds)
$armed = $false
$buildKind = $null
$logPath = $null
do {
    Start-Sleep -Milliseconds 500
    $log = Get-ChildItem -LiteralPath $logDir -Filter "rakukan-tsf-$($target.Id)-*.log" -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | Select-Object -First 1
    if ($log) {
        $logPath = $log.FullName
        $stream = [IO.File]::Open($logPath, [IO.FileMode]::Open, [IO.FileAccess]::Read,
            [IO.FileShare]::ReadWrite -bor [IO.FileShare]::Delete)
        try {
            $reader = [IO.StreamReader]::new($stream)
            $text = $reader.ReadToEnd()
        } finally {
            $stream.Dispose()
        }
        if (-not $buildKind -and $text -match 'config_watch: build_kind=(\S+)') { $buildKind = $Matches[1] }
        if ($text -match "fault control armed pid=$($target.Id)\b") { $armed = $true; break }
        if ($text -match 'fault control startup timed out') { break }
    }
} until ([DateTime]::UtcNow -ge $deadline)

if ($logPath) { Write-Host "LOG=$logPath" } else { Write-Host "LOG=(not found for PID $($target.Id) within $ArmedWaitSeconds s)" }
if ($buildKind) { Write-Host "BUILD_KIND=$buildKind" }
if ($armed) {
    Write-Host 'ARMED=yes'
} else {
    Write-Host 'ARMED=no'
    if ($buildKind -eq 'normal') {
        Write-Host '  The installed DLL is the normal build; run cargo make install-tsf-fault-test first.'
    } elseif ($text -match 'fault control startup timed out') {
        Write-Host '  The DLL waited 5 s for the control file and gave up; the file may have been written too late.'
    } else {
        Write-Host '  No armed line yet. If the environment variables did not reach this process (packaged app),'
        Write-Host '  try a non-packaged app with -AppPath.'
    }
}
