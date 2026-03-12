# demote-debug-logs.ps1
# 診断用の一時 info! ログを debug! に降格する（本番ビルドでは出力されない）

$file = "crates\rakukan-tsf\src\tsf\factory.rs"
$content = Get-Content $file -Raw -Encoding UTF8

$patterns = @(
    # OnKeyDown 診断ログ
    @('tracing::info!("OnKeyDown vk={:#04x}", vk);',
      'tracing::debug!("OnKeyDown vk={:#04x}", vk);'),
    @('tracing::info!("OnKeyDown vk={:#04x} → unmapped',
      'tracing::debug!("OnKeyDown vk={:#04x} → unmapped'),
    # on_segment_shrink 診断ログ
    @('tracing::info!("on_segment_shrink:',
      'tracing::debug!("on_segment_shrink:'),
    @('tracing::info!("  Selecting: original=',
      'tracing::debug!("  Selecting: original='),
    @('tracing::info!("  → SplitPreedit: target=',
      'tracing::debug!("  → SplitPreedit: target='),
    @('tracing::info!("  after update_composition: state=',
      'tracing::debug!("  after update_composition: state='),
    @('tracing::info!("  SplitPreedit: before_target=',
      'tracing::debug!("  SplitPreedit: before_target='),
    @('tracing::info!("  → new target=',
      'tracing::debug!("  → new target='),
    @('tracing::info!("  → no matching state',
      'tracing::debug!("  → no matching state'),
    # on_convert 診断ログ
    @('tracing::info!("on_convert: preedit_empty=',
      'tracing::debug!("on_convert: preedit_empty='),
    # convert_split_target 診断ログ
    @('tracing::info!("convert_split_target: target=',
      'tracing::debug!("convert_split_target: target='),
    @('tracing::info!("convert_split_target: candidates=',
      'tracing::debug!("convert_split_target: candidates=')
)

$changed = 0
foreach ($pair in $patterns) {
    if ($content.Contains($pair[0])) {
        $content = $content.Replace($pair[0], $pair[1])
        $changed++
        Write-Host "  demoted: $($pair[0].Substring(0, [Math]::Min(60, $pair[0].Length)))"
    }
}

if ($changed -gt 0) {
    Set-Content $file -Value $content -Encoding UTF8 -NoNewline
    Write-Host "[$changed] logs demoted to debug! in $file"
} else {
    Write-Host "No diagnostic info! logs found (already demoted or not present)"
}
