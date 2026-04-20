# scripts/refresh-models.ps1
#
# HuggingFace で公開されているモデルファイルの最新状況を確認し、
# crates/rakukan-engine/models.toml に未登録の .gguf があれば検出する。
#
# 使い方:
#   pwsh scripts/refresh-models.ps1          # チェックのみ: 差分レポートを表示 (書き換えなし)
#   pwsh scripts/refresh-models.ps1 -Apply   # 新規 variant を models.toml の末尾に追記
#   pwsh scripts/refresh-models.ps1 -Quiet   # 新規 variant が無ければ何も出力しない
#
# 終了コード:
#   0: 更新なし (最新)、または -Apply で正常に適用完了
#   2: -Apply なしで新規 variant を検出 (CI での「更新必要」シグナルに使える)
#   1: エラー (models.toml が見つからない等)
#
# 出力:
#   - 各 repo の既知ファイル / 新規ファイル
#   - 新規ファイルがあれば、models.toml に追記するための TOML スニペット
#
# 注意:
#   - -Apply は models.toml **のみ** を更新します。
#     variant 一覧が重複している以下 2 ファイルは手動更新が必要です:
#       - scripts/install.ps1                           ($modelMap)
#       - apps/rakukan-settings-winui/MainWindow.xaml   (ModelVariantCombo)

[CmdletBinding()]
param(
    [switch]$Apply,
    [switch]$Quiet
)

$ErrorActionPreference = "Stop"

$modelsTomlPath = Join-Path $PSScriptRoot "..\crates\rakukan-engine\models.toml"
if (-not (Test-Path -LiteralPath $modelsTomlPath)) {
    Write-Error "models.toml not found: $modelsTomlPath"
    exit 1
}

$modelsTomlText = Get-Content -LiteralPath $modelsTomlPath -Raw

# ── models.toml から repo_id と既知 filename を抽出 ───────────────────────────
# 形式:
#   [models.<family>]
#   repo_id = "..."
#   [models.<family>.variants.<q>]
#   filename = "..."

$repoIds  = [ordered]@{}   # family(short name) -> repo_id
$knownFiles = @{}          # repo_id -> list of known filenames

$currentFamily = $null
foreach ($line in ($modelsTomlText -split "`r?`n")) {
    $trim = $line.Trim()

    # セクション判定
    if ($trim -match '^\[models\.([^\.\]]+)\]$') {
        $currentFamily = $Matches[1]
        if (-not $repoIds.Contains($currentFamily)) {
            $repoIds[$currentFamily] = $null
        }
        continue
    }
    if ($trim -match '^\[models\.([^\.\]]+)\.variants\.[^\]]+\]$') {
        $currentFamily = $Matches[1]
        continue
    }

    # repo_id / filename
    if ($trim -match '^repo_id\s*=\s*"([^"]+)"' -and $currentFamily) {
        $repoIds[$currentFamily] = $Matches[1]
    }
    elseif ($trim -match '^filename\s*=\s*"([^"]+)"' -and $currentFamily) {
        $repo = $repoIds[$currentFamily]
        if ($repo) {
            if (-not $knownFiles.ContainsKey($repo)) {
                $knownFiles[$repo] = @()
            }
            $knownFiles[$repo] += $Matches[1]
        }
    }
}

if ($repoIds.Count -eq 0) {
    Write-Error "no [models.*] sections parsed from models.toml"
    exit 1
}

if (-not $Quiet) {
    Write-Host ""
    Write-Host "Scanning HuggingFace for new .gguf files..." -ForegroundColor Cyan
    Write-Host "  models.toml: $modelsTomlPath"
    Write-Host ""
}

# ── HuggingFace API で各 repo のファイル一覧を取得 ────────────────────────────

$anyNew = $false
$snippets = New-Object System.Collections.Generic.List[string]

foreach ($family in $repoIds.Keys) {
    $repo = $repoIds[$family]
    if (-not $repo) { continue }

    $apiUrl = "https://huggingface.co/api/models/$repo"
    try {
        $info = Invoke-RestMethod -Uri $apiUrl -TimeoutSec 30
    }
    catch {
        Write-Warning ("failed to query " + $apiUrl + ": " + $_)
        continue
    }

    $siblings = @($info.siblings | ForEach-Object { $_.rfilename })
    $ggufFiles = @($siblings | Where-Object { $_ -like '*.gguf' })
    $known = @($knownFiles[$repo])
    $new = @($ggufFiles | Where-Object { $known -notcontains $_ })

    if (-not $Quiet) {
        Write-Host ("[Repo] " + $repo) -ForegroundColor Yellow
        Write-Host ("  family : " + $family)
        foreach ($f in $known) {
            Write-Host ("  known  : " + $f) -ForegroundColor Gray
        }
        if ($new.Count -eq 0) {
            Write-Host "  (no new .gguf files)" -ForegroundColor DarkGray
        }
    }

    foreach ($f in $new) {
        $anyNew = $true
        if (-not $Quiet) {
            Write-Host ("  NEW    : " + $f) -ForegroundColor Green
        }

        # Q{level}_{variant} を抽出して variant キーを推定
        # 例: jinen-v1-xsmall-Q5_K_M.gguf -> q5
        #     jinen-v1-xsmall-Q4_K_M.gguf -> q4
        #     jinen-v1-xsmall-F16.gguf    -> f16
        $quantKey = $null
        if ($f -match '-(Q[0-9]+)_') {
            $quantKey = $Matches[1].ToLower()
        }
        elseif ($f -match '-(F[0-9]+)\.') {
            $quantKey = $Matches[1].ToLower()
        }
        elseif ($f -match '-([A-Za-z0-9_]+)\.gguf$') {
            $quantKey = $Matches[1].ToLower() -replace '[^a-z0-9]', ''
        }
        if (-not $quantKey) { $quantKey = "unknown" }

        $variantId = "$family-$quantKey"

        # display 用の quant ラベル (filename から末尾の -XXX.gguf を抽出)
        $quantLabel = ""
        if ($f -match '-([^-]+)\.gguf$') {
            $quantLabel = $Matches[1]
        }

        $snippet = @"

[models.$family.variants.$quantKey]
id = "$variantId"
filename = "$f"
display_name = "$family ($quantLabel)"

"@
        $snippets.Add($snippet) | Out-Null
    }

    if (-not $Quiet) { Write-Host "" }
}

# ── レポート ──────────────────────────────────────────────────────────────────

if ($anyNew) {
    if ($Apply) {
        Write-Host "================================================================" -ForegroundColor Cyan
        Write-Host " Applying new variants to models.toml..." -ForegroundColor Cyan
        Write-Host "================================================================" -ForegroundColor Cyan

        # 末尾の改行を整えてから追記 (各スニペット先頭に空行が入っている)
        $current = [System.IO.File]::ReadAllText($modelsTomlPath)
        if (-not $current.EndsWith("`n")) { $current += "`n" }
        $append = ($snippets -join "") + "`n"
        [System.IO.File]::WriteAllText($modelsTomlPath, $current + $append)

        Write-Host ("  Wrote " + $snippets.Count + " new variant section(s) to:") -ForegroundColor Green
        Write-Host ("    " + $modelsTomlPath) -ForegroundColor Gray
        Write-Host ""
        Write-Host "Also update manually (hardcoded variant tables):" -ForegroundColor Yellow
        Write-Host '  - scripts/install.ps1                               ($modelMap)' -ForegroundColor Gray
        Write-Host "  - apps/rakukan-settings-winui/MainWindow.xaml       (ModelVariantCombo)" -ForegroundColor Gray
        Write-Host ""
        exit 0
    }
    else {
        Write-Host "================================================================" -ForegroundColor Cyan
        Write-Host " New variants detected. Paste these into models.toml," -ForegroundColor Cyan
        Write-Host " or re-run with -Apply to append them automatically:" -ForegroundColor Cyan
        Write-Host "================================================================" -ForegroundColor Cyan
        foreach ($s in $snippets) {
            Write-Host $s
        }
        Write-Host ""
        Write-Host "Also update (hardcoded variant tables):" -ForegroundColor Yellow
        Write-Host '  - scripts/install.ps1                               ($modelMap)' -ForegroundColor Gray
        Write-Host "  - apps/rakukan-settings-winui/MainWindow.xaml       (ModelVariantCombo)" -ForegroundColor Gray
        Write-Host ""
        exit 2   # 差分あり (apply 未適用)
    }
}
elseif (-not $Quiet) {
    Write-Host "models.toml is up to date. No new .gguf files found." -ForegroundColor Green
}

exit 0
