# =============================================================================
#  [AI ASSISTANT NOTICE / Claude Code 向け注意]
# =============================================================================
#  このファイルは単体で実行するものではない。
#  scripts\sign-artifacts.ps1 / scripts\build-installer.ps1 から
#  dot-source される共通ライブラリ。
# =============================================================================
#
# scripts\signtool-common.ps1 - コード署名まわりの共通ヘルパー
#
#   Find-SignTool       : signtool.exe を Windows SDK から検出
#   Resolve-CertSubject : 署名証明書の Subject CN を解決 (環境変数 CODESIGN_CERT)
#
# 署名証明書は環境変数 CODESIGN_CERT で指定する。
#   setx CODESIGN_CERT "<CN>"

# --- signtool.exe を検出 ---
function Find-SignTool {
    param([string]$SigntoolPath)

    if ($SigntoolPath -and (Test-Path -LiteralPath $SigntoolPath)) {
        return $SigntoolPath
    }

    $candidates = @()
    $appCertKit = "${env:ProgramFiles(x86)}\Windows Kits\10\App Certification Kit\signtool.exe"
    if (Test-Path -LiteralPath $appCertKit) { $candidates += $appCertKit }

    $binRoot = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    if (Test-Path -LiteralPath $binRoot) {
        Get-ChildItem -Path $binRoot -Directory -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending |
            ForEach-Object {
                $p = Join-Path $_.FullName "x64\signtool.exe"
                if (Test-Path -LiteralPath $p) { $candidates += $p }
            }
    }

    $found = $candidates | Select-Object -First 1
    if (-not $found) {
        throw "signtool.exe not found. Install Windows 10/11 SDK or pass -SigntoolPath."
    }
    return $found
}

# --- 署名証明書の Subject CN を解決 ---
#
# /a (自動選択) は「有効期限が最も長い証明書」を選ぶため、WDK テスト証明書などが
# ストアに入ると本来の証明書から勝手に乗り換わる (2026-08 に実際に発生。
# パスワード保護のないテスト証明書が選ばれ、プロンプトなしで署名されていた)。
# そのため CN 未指定時はエラーとし、意図的に /a を使う場合のみ -AutoSelectCert を指定する。
#
# 戻り値: CN 文字列。/a 自動選択を使う場合は空文字列。
function Resolve-CertSubject {
    param(
        [string]$CertSubject,
        [switch]$AutoSelectCert
    )

    if ($AutoSelectCert) {
        if ($CertSubject) {
            Write-Warning "[sign] -AutoSelectCert が指定されたため CertSubject '$CertSubject' を無視して /a を使います"
        }
        return ""
    }

    if (-not $CertSubject) {
        throw @"
Code-signing certificate subject is not set.
環境変数 CODESIGN_CERT に証明書の Subject CN を設定してください。例:

    setx CODESIGN_CERT "<証明書の Subject CN>"      # 永続化 (新しいシェルから有効)
    `$env:CODESIGN_CERT = "<証明書の Subject CN>"   # 現在のセッションのみ

CN は証明書ストアで確認できます:
    Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert | Select-Object Subject, NotAfter

または明示的に -CertSubject "<CN>" を渡してください。
signtool /a の自動選択を意図的に使う場合は -AutoSelectCert を指定します
(テスト証明書に乗り換わる恐れがあるため非推奨)。
"@
    }

    return $CertSubject
}
