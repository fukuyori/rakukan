# rakukan をこのユーザーのキーボードの一覧に足す（インストーラーとトレイが、利用者の権限で実行する）。
#
# 1. HKLM の TIP の登録（rakukan の CLSID の分だけ）を HKCU へ写す（Windows 11 の設定の表示のため）
# 2. 日本語の入力方式の一覧に rakukan が無ければ足す
#
# 失敗したら 0 以外で終わる（トレイは成功したときだけ、この版で済んだと記録する）。

$ErrorActionPreference = 'Stop'

$tipClsid    = "{C0DDF8B0-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"
$tipProfile  = "{C0DDF8B1-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"
$tipEntry    = "0411:$tipClsid$tipProfile"

& reg.exe COPY "HKLM\Software\Microsoft\CTF\TIP\$tipClsid" "HKCU\Software\Microsoft\CTF\TIP\$tipClsid" /s /f | Out-Null
if ($LASTEXITCODE -ne 0) {
    Write-Error "reg COPY failed (exit $LASTEXITCODE)"
}

$list = Get-WinUserLanguageList
$ja   = $list | Where-Object { $_.LanguageTag -like "ja*" } | Select-Object -First 1

if (-not $ja) {
    # 日本語が一覧に無いユーザーには足さない
    exit 0
}

if ($ja.InputMethodTips -notcontains $tipEntry) {
    $ja.InputMethodTips.Add($tipEntry)
    Set-WinUserLanguageList $list -Force
}
exit 0
