# =============================================================================
# scripts\build-installer.ps1
# rakukan IME Inno Setup インストーラー作成スクリプト
#
# 使用方法:
#   cd D:\home\source\rust\rakukan
#   .\scripts\build-installer.ps1
#   .\scripts\build-installer.ps1 -Sign      # インストーラー/アンインストーラーに署名
#
# 前提:
#   - cargo make install が完了していること
#   - Inno Setup 6 がインストールされていること
#   - -Sign を使う場合: 環境変数 CODESIGN_CERT に証明書の Subject CN を設定済みで、
#     Windows SDK の signtool.exe が使えること
#
# -Sign について:
#   ISCC の SignTool 機能 (/S スイッチ) 経由で signtool を呼び出す。
#   .iss 側の SignedUninstaller=yes により、セットアップ本体だけでなく
#   インストール先に展開される **アンインストーラー (unins000.exe)** にも
#   同じ証明書で署名が付く。
#   dist\ に詰める DLL/EXE 自体の署名は cargo make sign が担当する (別レイヤ)。
# =============================================================================

param(
    [string]$Version,
    [string]$InstallDir = "$env:LOCALAPPDATA\rakukan",
    [string]$InstallerScript = "$PSScriptRoot\..\rakukan_installer.iss",
    # インストーラー / アンインストーラーに電子署名を付与する
    [switch]$Sign,
    # 署名に使う証明書の Subject CN (既定: 環境変数 CODESIGN_CERT)
    [string]$CertSubject = $env:CODESIGN_CERT,
    [string]$SigntoolPath = $null,
    [string]$TimestampUrl = "http://timestamp.digicert.com",
    # signtool /a の自動選択を使う (非推奨。理由は scripts\signtool-common.ps1)
    [switch]$AutoSelectCert
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path $PSScriptRoot -Parent
$distDir = "$PSScriptRoot\..\dist"

function Remove-PathIfExists([string]$Path) {
    if (Test-Path -LiteralPath $Path) {
        Remove-Item -LiteralPath $Path -Recurse -Force
    }
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    $versionFile = Join-Path $repoRoot "VERSION"
    if (Test-Path -LiteralPath $versionFile) {
        $Version = (Get-Content -LiteralPath $versionFile -Raw).Trim()
    }
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    throw "Version is empty. Pass -Version or create VERSION."
}

# --- ISCC.exe の場所を探す ---
$iscc = @(
    "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    "C:\Program Files\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1

if (-not $iscc) {
    Write-Error "Inno Setup 6 が見つかりません。https://jrsoftware.org/isinfo.php からインストールしてください。"
    exit 1
}

# --- 署名の準備 (dist を組み立てる前に失敗させる) ---
$isccExtraArgs = @()
$signtool = $null
if ($Sign) {
    . (Join-Path $PSScriptRoot "signtool-common.ps1")

    $signtool    = Find-SignTool -SigntoolPath $SigntoolPath
    $CertSubject = Resolve-CertSubject -CertSubject $CertSubject -AutoSelectCert:$AutoSelectCert

    Write-Host "[sign] signtool: $signtool"

    # ISCC の SignTool コマンド文字列。
    #   $q -> ダブルクォート、$f -> 署名対象ファイル (既にクォート済みなので $q で囲まない)
    # PowerShell の変数展開と衝突するため $q / $f はリテラルで組み立てる。
    $Q = '$q'
    $F = '$f'
    $certPart = if ($CertSubject) {
        Write-Host "[sign] Certificate: CN=$CertSubject (pinned)" -ForegroundColor Cyan
        "/n $Q$CertSubject$Q"
    } else {
        Write-Host "[sign] Certificate: auto-select (/a)" -ForegroundColor Yellow
        "/a"
    }
    $signCommand = "$Q$signtool$Q sign /fd SHA256 $certPart /tr $TimestampUrl /td SHA256 $F"
    $isccExtraArgs = @("/DSIGN", "/Srakukan=$signCommand")
}

Write-Host "[1/3] dist フォルダを準備中..."
# 前回ビルドの残骸を残すと、削除済みの成果物が次回インストーラーに混入する。
Remove-PathIfExists $distDir
New-Item -ItemType Directory -Force -Path $distDir | Out-Null
New-Item -ItemType Directory -Force -Path "$distDir\models" | Out-Null

# TSF DLL をコピー (固定名)
$tsfDll = Join-Path $InstallDir "rakukan_tsf.dll"
if (-not (Test-Path $tsfDll)) {
    Write-Error "rakukan_tsf.dll が $InstallDir に見つかりません。先に cargo make install を実行してください。"
    exit 1
}
Copy-Item $tsfDll "$distDir\rakukan_tsf.dll" -Force
Write-Host "  -> rakukan_tsf.dll"

# アイコン
$icoSrc = "$PSScriptRoot\..\crates\rakukan-tsf\rakukan.ico"
if (Test-Path $icoSrc) {
    Copy-Item $icoSrc "$distDir\rakukan.ico" -Force
    Write-Host "  -> rakukan.ico"
} else {
    Write-Warning "rakukan.ico が見つかりません ($icoSrc)"
}

# register-tip.ps1 (キーボードリスト登録スクリプト)
Copy-Item "$PSScriptRoot\register-tip.ps1" "$distDir\register-tip.ps1" -Force
Write-Host "  -> register-tip.ps1"

# unregister-tip.ps1 (キーボードリスト削除スクリプト)
Copy-Item "$PSScriptRoot\unregister-tip.ps1" "$distDir\unregister-tip.ps1" -Force
Write-Host "  -> unregister-tip.ps1"

# Engine DLL
foreach ($name in @("rakukan_engine_cpu.dll", "rakukan_engine_vulkan.dll", "rakukan_engine_cuda.dll")) {
    $src = Join-Path $InstallDir $name
    if (Test-Path $src) {
        Copy-Item $src "$distDir\$name" -Force
        Write-Host "  -> $name"
    }
}

# Engine Host (out-of-process RPC server)
$engineHost = Join-Path $InstallDir "rakukan-engine-host.exe"
if (Test-Path $engineHost) {
    Copy-Item $engineHost "$distDir\rakukan-engine-host.exe" -Force
    Write-Host "  -> rakukan-engine-host.exe"
} else {
    Write-Warning "rakukan-engine-host.exe が見つかりません ($engineHost) — cargo make install が古い可能性があります"
}

# Settings GUI (WinUI 3 app folder)
$settingsDir = Join-Path $InstallDir "settings-ui"
if (Test-Path $settingsDir) {
    New-Item -ItemType Directory -Force -Path "$distDir\settings-ui" | Out-Null
    Copy-Item "$settingsDir\*" "$distDir\settings-ui\" -Recurse -Force
    Write-Host "  -> settings-ui\\"
} else {
    Write-Warning "settings-ui が見つかりません ($settingsDir)"
}

# 辞書
$dict = Join-Path $env:LOCALAPPDATA "rakukan\dict\rakukan.dict"
if (Test-Path $dict) {
    Copy-Item $dict "$distDir\rakukan.dict" -Force
    Write-Host "  -> rakukan.dict"
} else {
    Write-Warning "rakukan.dict が見つかりません ($dict)"
}

# ライセンス・帰属表示
foreach ($entry in @(
    @{ Name = "NOTICE"; Source = (Join-Path $repoRoot "NOTICE") }
    @{ Name = "THIRD_PARTY_LICENSES.md"; Source = (Join-Path $repoRoot "docs\THIRD_PARTY_LICENSES.md") }
)) {
    $f = $entry.Name
    $src = $entry.Source
    if (Test-Path $src) {
        Copy-Item $src "$distDir\$f" -Force
        Write-Host "  -> $f"
    } else {
        Write-Warning "$f が見つかりません"
    }
}

# config.toml (デフォルト値が入ったもの)
$configSrc = "$PSScriptRoot\..\config\config.toml"
if (-not (Test-Path $configSrc)) {
    $configSrc = Join-Path $InstallDir "config.toml"
}
if (Test-Path $configSrc) {
    Copy-Item $configSrc "$distDir\config.toml" -Force
    Write-Host "  -> config.toml"
}

# モデル (.gguf) をコピー (存在する場合)
$modelsDir = Join-Path $InstallDir "models"
if (Test-Path $modelsDir) {
    $ggufFiles = Get-ChildItem -Path $modelsDir -Filter "*.gguf"
    foreach ($f in $ggufFiles) {
        Copy-Item $f.FullName "$distDir\models\" -Force
        Write-Host "  -> models\$($f.Name)"
    }
}

# --- バージョン番号をスクリプトに反映 ---
$issContent = Get-Content $InstallerScript -Raw
$issContent = $issContent -replace '#define MyAppVersion\s+"[^"]+"', "#define MyAppVersion   `"$Version`""
$issContent | Set-Content $InstallerScript -NoNewline -Encoding UTF8

Write-Host ""
if ($Sign) {
    Write-Host "[2/3] Inno Setup コンパイル中 (署名あり: setup + uninstaller)..."
} else {
    Write-Host "[2/3] Inno Setup コンパイル中..."
}
& $iscc @isccExtraArgs $InstallerScript
if ($LASTEXITCODE -ne 0) {
    Write-Error "ISCC.exe が失敗しました (exit code $LASTEXITCODE)"
    exit 1
}

Write-Host ""
Write-Host "[3/3] 完了!"
$outputFile = Get-ChildItem "$PSScriptRoot\..\output\rakukan-*.exe" |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($outputFile) {
    Write-Host "インストーラー: $($outputFile.FullName)"
    Write-Host "サイズ: $([math]::Round($outputFile.Length / 1MB, 1)) MB"

    if ($Sign) {
        # 署名検証 (アンインストーラーは ISCC が埋め込み済みなのでここでは検証できない)
        & $signtool verify /pa $outputFile.FullName
        if ($LASTEXITCODE -eq 0) {
            Write-Host "[sign] 署名を確認しました (アンインストーラーも同じ証明書で署名済み)" -ForegroundColor Green
        } else {
            Write-Error "[sign] 署名の検証に失敗しました (exit code $LASTEXITCODE)"
            exit 1
        }
    }
}
