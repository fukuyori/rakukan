# rakukan をこのユーザーのキーボードの一覧から外す（アンインストーラーが実行する）。
#
# 1. 日本語の入力方式の一覧から rakukan を外す
# 2. HKCU に写した TIP の登録（rakukan の CLSID の分だけ）を消す
# 3. トレイが一覧を補った版の記録を消す（入れ直したときに、また補えるように）

$tipClsid   = "{C0DDF8B0-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"
$tipProfile = "{C0DDF8B1-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"
$tipEntry   = "0411:$tipClsid$tipProfile"

$list = Get-WinUserLanguageList
$ja   = $list | Where-Object { $_.LanguageTag -like "ja*" } | Select-Object -First 1

if ($ja -and $ja.InputMethodTips -contains $tipEntry) {
    $ja.InputMethodTips.Remove($tipEntry) | Out-Null
    Set-WinUserLanguageList $list -Force
}

& reg.exe DELETE "HKCU\Software\Microsoft\CTF\TIP\$tipClsid" /f 2>$null | Out-Null
& reg.exe DELETE "HKCU\Software\rakukan" /v TipListedVersion /f 2>$null | Out-Null
exit 0
