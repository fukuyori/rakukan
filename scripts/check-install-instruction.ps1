# Stop hook: install/build 手順の誤案内を検出して block する。
#
# Claude Code の Stop hook として呼ばれる。stdin に session 情報 JSON が来るので
# transcript_path を読み、直近のアシスタント応答テキストを取り出して検査する。
#
# 検出する違反:
#  1. "cargo make install" が応答に含まれているのに、同じ応答に
#     "cargo make build-tsf" または "cargo make build-engine" が含まれない
#     (CLAUDE.md の build → install 手順違反)
#  2. "install 後にサインアウト" / "install 後にサインイン" 等の誤った順序の案内
#     (正しい順序: sign-out → sign-in → build → install)
#
# 違反があれば {"decision": "block", "reason": "..."} を出力して再考させる。
# 違反なしなら exit 0 (静かに通す)。

$ErrorActionPreference = 'Stop'

# stdout / stdin を UTF-8 (BOM なし) に固定。Stop hook の出力 JSON が
# 日本語 reason を含むため、既定の Shift-JIS では Claude Code 側の JSON parser が
# 化けて block が効かない。
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
[Console]::OutputEncoding = $utf8NoBom
$OutputEncoding = $utf8NoBom

# stdin を全部読む
$inputData = [System.IO.StreamReader]::new([System.Console]::OpenStandardInput()).ReadToEnd()
if (-not $inputData) { exit 0 }

try {
    $obj = $inputData | ConvertFrom-Json -ErrorAction Stop
} catch {
    exit 0
}

$path = $obj.transcript_path
if (-not $path -or -not (Test-Path -LiteralPath $path)) { exit 0 }

# 直近の assistant message のテキストを取り出す
$lastAssistant = $null
Get-Content -LiteralPath $path -Encoding UTF8 | ForEach-Object {
    if (-not $_) { return }
    try {
        $entry = $_ | ConvertFrom-Json -ErrorAction Stop
    } catch { return }
    if ($entry.type -ne 'assistant') { return }
    if (-not $entry.message) { return }
    if (-not $entry.message.content) { return }
    $textParts = @()
    foreach ($c in $entry.message.content) {
        if ($c.type -eq 'text' -and $c.text) {
            $textParts += [string]$c.text
        }
    }
    if ($textParts.Count -gt 0) {
        $script:lastAssistant = ($textParts -join "`n")
    }
}

if (-not $lastAssistant) { exit 0 }

# インラインコードスパン (`...`) のみ除去する。テーブルや本文中で
# 「検出するパターンの例」として `install 後にサインアウト` のように
# バッククォート 1 つで囲った文字列を誤検知しないため。
# **コードブロック (```...```) は剥がさない**: ユーザに実行を促す本物の
# コマンドが入っているケースが多いので、`cargo make install` がブロック内に
# だけ書かれていても検出対象に含める。
$lastAssistant = [regex]::Replace($lastAssistant, '`[^`\r\n]+`', ' ')

$violations = @()

# 検出 1: cargo make install を案内しているが build-* の案内がない
$hasInstall      = $lastAssistant -match 'cargo make install'
$hasBuildTsf     = $lastAssistant -match 'cargo make build-tsf'
$hasBuildEngine  = $lastAssistant -match 'cargo make build-engine'
if ($hasInstall -and -not ($hasBuildTsf -or $hasBuildEngine)) {
    $violations += '`cargo make install` を案内したのに `cargo make build-tsf` / `cargo make build-engine` の案内が同じ応答内にない。CLAUDE.md の手順 (build → install) を必ず併記すること。'
}

# 検出 2: install の後にサインアウト/サインインを置いた誤った順序
$wrongOrder = @(
    'install\s*後にサインアウト',
    'install\s*後にサインイン',
    'インストール\s*後にサインアウト',
    'インストール\s*後にサインイン',
    'install\s*→\s*サインアウト',
    'install\s*→\s*サインイン',
    'インストール\s*→\s*サインアウト',
    'インストール\s*→\s*サインイン'
)
foreach ($pat in $wrongOrder) {
    if ($lastAssistant -match $pat) {
        $violations += 'install の後にサインアウト/サインインを案内している。正しい順序は「サインアウト → サインイン → build (build-tsf / build-engine) → install」(memory/feedback_install_order.md 参照)。'
        break
    }
}

if ($violations.Count -eq 0) { exit 0 }

$reason = "反映手順の案内が CLAUDE.md および memory/feedback_install_order.md と矛盾しています:`n- " + ($violations -join "`n- ") + "`n応答を訂正してから返答してください。"

$out = @{
    decision = 'block'
    reason   = $reason
} | ConvertTo-Json -Compress

[Console]::Out.Write($out)
exit 0
