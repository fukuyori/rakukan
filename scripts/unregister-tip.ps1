$tipClsid   = "{C0DDF8B0-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"
$tipProfile = "{C0DDF8B1-1F1E-4C2D-A9E3-5F7B8D6E2A4C}"
$tipEntry   = "0411:$tipClsid$tipProfile"

$list = Get-WinUserLanguageList
$ja   = $list | Where-Object { $_.LanguageTag -like "ja*" } | Select-Object -First 1

if ($ja -and $ja.InputMethodTips -contains $tipEntry) {
    $ja.InputMethodTips.Remove($tipEntry) | Out-Null
    Set-WinUserLanguageList $list -Force
}
