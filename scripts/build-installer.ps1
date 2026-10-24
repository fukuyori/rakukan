# ============================================================================
#  [AI ASSISTANT NOTICE / Claude Code 向け注意]
# ============================================================================
#  このスクリプトをユーザーに直接実行させるよう案内しないこと。
#  正規の入口は以下の cargo make コマンドです:
#
#      sudo cargo make install           (開発の導入: 署名なしで作り、サイレント実行する)
#      cargo make package                (配布物: cargo make sign の後に -Sign で作る)
#
#  このファイルは Makefile.toml から呼び出される内部実装です。
# ============================================================================
#
# scripts\build-installer.ps1 - rakukan の Inno Setup インストーラーを作る
#
# 入力はビルドの出力（docs/Installer_Redesign_Plan.md の 4.3）:
#   $BuildDir\$Profile\  rakukan_tsf.dll、rakukan_engine_{cpu,vulkan,cuda}.dll、
#                        rakukan-engine-host.exe、rakukan-tray.exe、rakukan-dict-builder.exe
#   apps\rakukan-settings-winui\bin\x64\<Config>\net8.0-windows10.0.19041.0\win-x64\  設定アプリ
# 辞書は、ここで mozc の TSV を取得して rakukan-dict-builder で作る（$BuildDir\dict に置き、
# 取得元の版と cost 帯の版が同じなら作り直さない）。モデルは同梱しない（engine-host が初回の変換で取得する）。
#
# 前提:
#   - cargo make build-engine / cargo make build-tsf が済んでいること
#   - Inno Setup 6 がインストールされていること
#   - -Sign を使う場合: 環境変数 CODESIGN_CERT に証明書の Subject CN を設定済みで、
#     Windows SDK の signtool.exe が使えること。成果物（DLL / EXE）は cargo make sign で先に署名しておく
#
# -Sign について:
#   ISCC の SignTool 機能 (/S スイッチ) 経由で signtool を呼び出す。
#   .iss 側の SignedUninstaller=yes により、セットアップ本体だけでなく
#   インストール先に展開される **アンインストーラー (unins000.exe)** にも
#   同じ証明書で署名が付く。dist\ に詰める DLL/EXE に署名が付いているかもここで確かめる。
#
# -Install について:
#   作ったインストーラーを /VERYSILENT /SUPPRESSMSGBOXES /NORESTART で実行する（開発の導入）。
#   完了には再起動が要る。セットアップのログは実行ごとに output\logs\ へ別のファイルで残す。
#
# 環境変数 RAKUKAN_FORCE_DICT=1: 辞書を作り直す（TSV も取り直す）
# =============================================================================

param(
    [string]$Version,
    [ValidateSet("debug","release")] [string]$Profile = "release",
    [string]$BuildDir = "C:\rb",
    [string]$InstallerScript = "$PSScriptRoot\..\rakukan_installer.iss",
    # インストーラー / アンインストーラーに電子署名を付与する
    [switch]$Sign,
    # 作ったインストーラーをサイレント実行する（開発の導入）
    [switch]$Install,
    # 署名に使う証明書の Subject CN (既定: 環境変数 CODESIGN_CERT)
    [string]$CertSubject = $env:CODESIGN_CERT,
    [string]$SigntoolPath = $null,
    [string]$TimestampUrl = "http://timestamp.digicert.com",
    # signtool /a の自動選択を使う (非推奨。理由は scripts\signtool-common.ps1)
    [switch]$AutoSelectCert
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# Console encoding: UTF-8 (Windows PowerShell 5.1 で日本語出力の文字化けを防ぐ)
try {
    [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new()
    $OutputEncoding = [System.Text.UTF8Encoding]::new()
} catch {}

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
    Write-Error "Inno Setup 6 が見つかりません。cargo make setup-env で導入するか、https://jrsoftware.org/isinfo.php からインストールしてください。"
    exit 1
}

# --- ビルドの出力 ---
# 相対パス（#65 の故障試験の出力先など）はリポジトリを基準にする
if (-not [System.IO.Path]::IsPathRooted($BuildDir)) { $BuildDir = Join-Path $repoRoot $BuildDir }
$BuildDir = [System.IO.Path]::GetFullPath($BuildDir)
$profileDir = if ($Profile -eq "release") { "release" } else { "debug" }
$cfgName    = if ($Profile -eq "release") { "Release" } else { "Debug" }
$outDir     = Join-Path $BuildDir $profileDir
$winuiBin   = Join-Path $repoRoot "apps\rakukan-settings-winui\bin\x64\$cfgName\net8.0-windows10.0.19041.0\win-x64"
$dictBuilderExe = Join-Path $outDir "rakukan-dict-builder.exe"

$required = @(
    @{ Path = (Join-Path $outDir "rakukan_tsf.dll");          Task = "cargo make build-tsf" }
    @{ Path = (Join-Path $outDir "rakukan-engine-host.exe");  Task = "cargo make build-tsf" }
    @{ Path = (Join-Path $outDir "rakukan-tray.exe");         Task = "cargo make build-tsf" }
    @{ Path = $dictBuilderExe;                                Task = "cargo make build-tsf" }
    @{ Path = (Join-Path $winuiBin "rakukan-settings.exe");   Task = "cargo make build-tsf" }
    @{ Path = (Join-Path $outDir "rakukan_engine_cpu.dll");   Task = "cargo make build-engine" }
)
foreach ($r in $required) {
    if (-not (Test-Path -LiteralPath $r.Path)) {
        throw "ビルドの出力がありません: $($r.Path)。先に $($r.Task) を実行してください。"
    }
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

# ─────────────────────────────────────────────────────────────────────────────
# 辞書（mozc の TSV から rakukan-dict-builder で作る。Apache 2.0）
# ─────────────────────────────────────────────────────────────────────────────

Write-Host "[1/4] 辞書を準備中..."
$dictDir     = Join-Path $BuildDir "dict"
$mozcDictOut = Join-Path $dictDir "rakukan.dict"
New-Item -ItemType Directory -Force -Path $dictDir | Out-Null
$forceDict = $env:RAKUKAN_FORCE_DICT -eq "1"

$mozcTsvFiles = @(
    "dictionary00.txt"
    "dictionary01.txt"
    "dictionary02.txt"
    "dictionary03.txt"
    "dictionary04.txt"
    "dictionary05.txt"
    "dictionary06.txt"
    "dictionary07.txt"
    "dictionary08.txt"
    "dictionary09.txt"
)
# 取得元は完全なコミット SHA で固定する（Issue #62）。
# ブランチ参照（refs/heads/master）だと、同じ版の rakukan でも作った時期によって
# 辞書の中身が変わり、変換品質の差を実装の違いと切り分けられない。
# upstream を取り込むときはこの値を上げる。辞書は SHA の不一致で自動的に作り直される
# （RAKUKAN_FORCE_DICT=1 を手で指定する必要はない）。
$mozcRev     = "cbbb6e1bd181cb9f3b409622d916a53a35400ec7"
$mozcRepoUrl = "https://raw.githubusercontent.com/google/mozc/$mozcRev"
$mozcBaseUrl = "$mozcRepoUrl/src/data/dictionary_oss"
$symbolUrl   = "$mozcRepoUrl/src/data/symbol/symbol.tsv"
$emojiUrl    = "$mozcRepoUrl/src/data/emoji/emoji_data.tsv"

# 取得済み TSV は SHA ごとの作業フォルダに置く。別のリビジョンで取ったものを
# 新しい SHA のものとして再利用しないため。
$mozcTsvDir  = Join-Path $dictDir ("mozc_tsv_" + $mozcRev.Substring(0, 12))

# cost 帯の版（rakukan-dict の cost_band::DICT_SCHEMA と同じ値にする）。
# 辞書の隣の rakukan.dict.build.json にビルダーが書く。無い・古い辞書は作り直す。
$dictSchemaExpected = 2
$dictBuildInfo = "$mozcDictOut.build.json"

# 復旧不能ならここで停止し、再生成判定や別の差し替えへ進まない（Issue #62）。
. (Join-Path $PSScriptRoot 'dictionary-swap.ps1')
$recovery = Restore-DictionarySwap -DictionaryPath $mozcDictOut
if ($recovery -eq 'rolled_back') {
    Write-Host "  -> restored the previous dictionary state after an interrupted swap"
} elseif ($recovery -eq 'committed') {
    Write-Host "  -> kept the committed dictionary pair and finished backup cleanup"
}

function Test-DictUpToDate {
    if (-not (Test-Path -LiteralPath $mozcDictOut)) { return "no dictionary" }
    if (-not (Test-Path -LiteralPath $dictBuildInfo)) { return "no build info (built before dict_schema 2)" }
    try {
        $info = Get-Content -LiteralPath $dictBuildInfo -Raw | ConvertFrom-Json
    } catch {
        return "build info could not be read"
    }
    if ([int]$info.dict_schema -lt $dictSchemaExpected) {
        return "dict_schema " + $info.dict_schema + " < " + $dictSchemaExpected
    }
    if ([string]$info.mozc_rev -ne $mozcRev) {
        $haveRev = if ($info.mozc_rev) { $info.mozc_rev } else { "unknown" }
        return "mozc_rev " + $haveRev + " != " + $mozcRev
    }
    return $null
}

$dictRebuildReason = Test-DictUpToDate
if ((-not $dictRebuildReason) -and (-not $forceDict)) {
    $sizeMB = [math]::Round((Get-Item $mozcDictOut).Length / 1048576, 1)
    Write-Host ("  -> rakukan.dict already built (" + $sizeMB + " MB, mozc " + $mozcRev.Substring(0, 12) + "), skipping.")
    Write-Host "     (To rebuild, set RAKUKAN_FORCE_DICT=1 and re-run)"
} else {
    if ($forceDict) { $dictRebuildReason = "RAKUKAN_FORCE_DICT=1" }
    Write-Host ("  -> building rakukan.dict: " + $dictRebuildReason)

    # 別のリビジョンで取得した TSV を残さない
    Get-ChildItem -LiteralPath $dictDir -Directory -Filter "mozc_tsv*" -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -ne $mozcTsvDir } |
        ForEach-Object {
            Write-Host ("    Removing TSV from another revision: " + $_.Name)
            Remove-Item -LiteralPath $_.FullName -Recurse -Force -ErrorAction SilentlyContinue
        }
    New-Item -ItemType Directory -Force -Path $mozcTsvDir | Out-Null

    # 必須入力。1 つでも欠けたら再生成を中止する（一部を欠いた辞書を作らない）。
    $requiredInputs = @()
    foreach ($tsv in $mozcTsvFiles) {
        $requiredInputs += [pscustomobject]@{
            Name = $tsv
            Url  = ($mozcBaseUrl + "/" + $tsv)
            Path = (Join-Path $mozcTsvDir $tsv)
        }
    }
    $requiredInputs += [pscustomobject]@{
        Name = "symbol.tsv"; Url = $symbolUrl; Path = (Join-Path $mozcTsvDir "symbol.tsv")
    }
    $requiredInputs += [pscustomobject]@{
        Name = "emoji_data.tsv"; Url = $emojiUrl; Path = (Join-Path $mozcTsvDir "emoji_data.tsv")
    }

    $missingInputs = [System.Collections.Generic.List[string]]::new()
    foreach ($item in $requiredInputs) {
        # 同じ SHA のフォルダに既にあるものは取り直さない（中断からの再開）
        if ((Test-Path -LiteralPath $item.Path) -and (-not $forceDict)) { continue }
        $tmpPath = $item.Path + ".tmp"
        try {
            Invoke-WebRequest -Uri $item.Url -OutFile $tmpPath -UseBasicParsing -TimeoutSec 120
            Move-Item -LiteralPath $tmpPath -Destination $item.Path -Force
            Write-Host ("    Downloaded: " + $item.Name)
        } catch {
            if (Test-Path -LiteralPath $tmpPath) { Remove-Item -LiteralPath $tmpPath -Force -ErrorAction SilentlyContinue }
            Write-Host ("    [WARNING] Failed: " + $item.Name + " - " + $_)
            $missingInputs.Add($item.Name)
        }
    }

    if ($missingInputs.Count -gt 0) {
        # 一部の入力を欠いた辞書を、正常に生成できたものとして扱わない
        Write-Host ("  [WARNING] " + $missingInputs.Count + " required input(s) missing: " + ($missingInputs -join ", "))
        Write-Host "  -> skipping rebuild; keeping the existing rakukan.dict and build.json"
    } else {
        Write-Host ("  Building rakukan.dict from " + $mozcTsvFiles.Count + " TSV files + symbol.tsv + emoji_data.tsv (mozc " + $mozcRev.Substring(0, 12) + ")...")

        # いったん別名へ生成し、成功したときだけ差し替える。
        # 途中で失敗しても既存の辞書と build.json を壊さない。
        $newDictOut   = $mozcDictOut + ".new"
        $newBuildInfo = "$newDictOut.build.json"
        Remove-Item -LiteralPath $newDictOut -Force -ErrorAction SilentlyContinue
        Remove-Item -LiteralPath $newBuildInfo -Force -ErrorAction SilentlyContinue

        $inputArgs = @()
        foreach ($tsv in $mozcTsvFiles) {
            $inputArgs += "--input"
            $inputArgs += (Join-Path $mozcTsvDir $tsv)
        }
        $inputArgs += "--symbol"
        $inputArgs += (Join-Path $mozcTsvDir "symbol.tsv")
        $inputArgs += "--emoji"
        $inputArgs += (Join-Path $mozcTsvDir "emoji_data.tsv")
        $inputArgs += "--mozc-rev"
        $inputArgs += $mozcRev
        $inputArgs += "--mozc-source"
        $inputArgs += $mozcRepoUrl
        $inputArgs += "--output"
        $inputArgs += $newDictOut

        $buildSucceeded = $false
        try {
            & $dictBuilderExe @inputArgs
            $buildSucceeded = $LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $newDictOut) -and (Test-Path -LiteralPath $newBuildInfo)
            if (-not $buildSucceeded) {
                Write-Host ("  [WARNING] rakukan-dict-builder failed (exit " + $LASTEXITCODE + "); keeping the existing rakukan.dict")
            }
        } catch {
            Write-Host ("  [WARNING] rakukan-dict-builder error: " + $_ + "; keeping the existing rakukan.dict")
        }
        if ($buildSucceeded) {
            # 復元・掃除の失敗をビルダーの catch で握りつぶさない。
            Install-DictionaryPair -DictionaryPath $mozcDictOut
            $sizeMB = [math]::Round((Get-Item $mozcDictOut).Length / 1048576, 1)
            Write-Host ("  -> " + $mozcDictOut + " (" + $sizeMB + " MB, mozc " + $mozcRev.Substring(0, 12) + ")")
            Remove-Item -LiteralPath $mozcTsvDir -Recurse -Force -ErrorAction SilentlyContinue
        } else {
            Remove-Item -LiteralPath $newDictOut -Force -ErrorAction SilentlyContinue
            Remove-Item -LiteralPath $newBuildInfo -Force -ErrorAction SilentlyContinue
        }
    }

    $dictRebuildReason = Test-DictUpToDate
}

if (-not (Test-Path -LiteralPath $mozcDictOut)) {
    throw "辞書を作れませんでした（上の WARNING を参照）: $mozcDictOut"
}
if ($dictRebuildReason) {
    # 配布物には、取得元の版と cost 帯の版が合わない辞書を入れない
    if ($Sign) { throw "辞書が古いままです（$dictRebuildReason）。取得・ビルドの失敗を直してから作り直してください。" }
    Write-Warning "辞書が古いまま使います（$dictRebuildReason）"
}

# ─────────────────────────────────────────────────────────────────────────────
# dist の組み立て
# ─────────────────────────────────────────────────────────────────────────────

Write-Host "[2/4] dist フォルダを準備中..."
# 前回ビルドの残骸を残すと、削除済みの成果物が次回インストーラーに混入する。
Remove-PathIfExists $distDir
New-Item -ItemType Directory -Force -Path $distDir | Out-Null

# TSF DLL・ホスト・トレイ・engine DLL（cpu は必須、vulkan / cuda はビルドした場合だけ）
foreach ($name in @("rakukan_tsf.dll", "rakukan-engine-host.exe", "rakukan-tray.exe",
                    "rakukan_engine_cpu.dll", "rakukan_engine_vulkan.dll", "rakukan_engine_cuda.dll")) {
    $src = Join-Path $outDir $name
    if (Test-Path -LiteralPath $src) {
        Copy-Item -LiteralPath $src "$distDir\$name" -Force
        Write-Host "  -> $name"
    }
}

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

# Settings GUI (WinUI 3 app folder)
New-Item -ItemType Directory -Force -Path "$distDir\settings-ui" | Out-Null
Copy-Item "$winuiBin\*" "$distDir\settings-ui\" -Recurse -Force
Write-Host "  -> settings-ui\\"
# 設定アプリが同梱する部品のライセンス（docs/licenses/bundled-components.json と突き合わせる）
& (Join-Path $repoRoot "scripts\check-bundled-licenses.ps1") -Files "$distDir\settings-ui"
if ($LASTEXITCODE -ne 0) { throw "settings-ui に、ライセンスを確かめていない部品があります（上の NG を参照）" }

# 辞書
Copy-Item -LiteralPath $mozcDictOut "$distDir\rakukan.dict" -Force
Write-Host "  -> rakukan.dict"
if (Test-Path -LiteralPath $dictBuildInfo) {
    Copy-Item -LiteralPath $dictBuildInfo "$distDir\rakukan.dict.build.json" -Force
    Write-Host "  -> rakukan.dict.build.json"
}

# ライセンス・帰属表示。Rust の crate の一覧が依存と合っているかを先に確かめる
& (Join-Path $repoRoot "scripts\gen-third-party-licenses.ps1") -Check
if ($LASTEXITCODE -ne 0) { throw "THIRD_PARTY_LICENSES.md が依存と合っていません（scripts\gen-third-party-licenses.ps1 を実行）" }
foreach ($entry in @(
    @{ Name = "LICENSE"; Source = (Join-Path $repoRoot "LICENSE") }
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

# 使用許諾の画面（rakukan_installer.iss の LicenseFile）。見出しの文、rakukan の MIT License、
# 設定アプリが同梱する Microsoft の部品の条項をつないで 1 つの文書にする（docs/licenses）
$agreementParts = @(
    @{ Title = $null; Path = (Join-Path $repoRoot "docs\licenses\license-agreement-header.txt") }
    @{ Title = "[1] rakukan - MIT License"; Path = (Join-Path $repoRoot "LICENSE") }
    @{ Title = "[2] Microsoft Windows App SDK 1.6.250205002 - Microsoft Software License Terms"; Path = (Join-Path $repoRoot "docs\licenses\microsoft-windows-app-sdk-1.6.250205002.txt") }
    @{ Title = "[3] Microsoft Windows SDK (Microsoft.Windows.SDK.NET.Ref 10.0.19041.57) - Microsoft Software License Terms"; Path = (Join-Path $repoRoot "docs\licenses\microsoft-windows-sdk.txt") }
    @{ Title = "[4] NVIDIA CUDA Toolkit 13.3 - End User License Agreement"; Path = (Join-Path $repoRoot "docs\licenses\nvidia-cuda-13.3-eula.txt") }
)
$agreement = [System.Text.StringBuilder]::new()
foreach ($part in $agreementParts) {
    if (-not (Test-Path $part.Path)) { throw "使用許諾の文書の部品がありません: $($part.Path)" }
    if ($part.Title) {
        [void]$agreement.AppendLine()
        [void]$agreement.AppendLine(('=' * 78))
        [void]$agreement.AppendLine($part.Title)
        [void]$agreement.AppendLine(('=' * 78))
        [void]$agreement.AppendLine()
    }
    [void]$agreement.AppendLine((Get-Content -LiteralPath $part.Path -Raw -Encoding utf8).TrimEnd())
}
[System.IO.File]::WriteAllText("$distDir\LICENSE_AGREEMENT.txt", ($agreement.ToString() -replace "`r?`n", "`r`n"), [System.Text.UTF8Encoding]::new($true))
Write-Host "  -> LICENSE_AGREEMENT.txt"

# config.toml (デフォルト値が入ったもの)
Copy-Item "$PSScriptRoot\..\config\config.toml" "$distDir\config.toml" -Force
Write-Host "  -> config.toml"

# 署名付きで作るときは、詰めた成果物（cargo make sign の対象）に署名が付いているかを確かめる
if ($Sign) {
    $signed = @(Get-ChildItem -LiteralPath $distDir -File | Where-Object { $_.Extension -in @(".dll", ".exe") } | ForEach-Object FullName) +
              @("$distDir\settings-ui\rakukan-settings.exe", "$distDir\settings-ui\rakukan-settings.dll")
    & $signtool verify /pa /q @signed
    if ($LASTEXITCODE -ne 0) {
        & $signtool verify /pa @signed
        throw "[sign] 署名の無い成果物があります。先に cargo make sign を実行してください（cargo make package はこの順で行う）"
    }
    Write-Host "[sign] 成果物の署名を確認しました ($($signed.Count) files)" -ForegroundColor Green
}

# --- バージョン番号をスクリプトに反映 ---
# UTF-8 (BOM なし) のまま読み書きし、版が変わらなければ書かない
$issContent = [System.IO.File]::ReadAllText((Resolve-Path $InstallerScript))
$issNew = $issContent -replace '#define MyAppVersion\s+"[^"]+"', "#define MyAppVersion   `"$Version`""
if ($issNew -ne $issContent) {
    [System.IO.File]::WriteAllText((Resolve-Path $InstallerScript), $issNew, [System.Text.UTF8Encoding]::new($false))
}

Write-Host ""
if ($Sign) {
    Write-Host "[3/4] Inno Setup コンパイル中 (署名あり: setup + uninstaller)..."
} else {
    Write-Host "[3/4] Inno Setup コンパイル中..."
}
& $iscc @isccExtraArgs $InstallerScript
if ($LASTEXITCODE -ne 0) {
    Write-Error "ISCC.exe が失敗しました (exit code $LASTEXITCODE)"
    exit 1
}

$outputFile = Get-Item -LiteralPath (Join-Path $repoRoot "output\rakukan-$Version-setup.exe")
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

if (-not $Install) {
    Write-Host ""
    Write-Host "[4/4] 完了!"
    exit 0
}

# ─────────────────────────────────────────────────────────────────────────────
# サイレント実行（開発の導入）
# ─────────────────────────────────────────────────────────────────────────────

Write-Host ""
Write-Host "[4/4] インストーラーをサイレント実行中..."
$logDir = Join-Path $repoRoot "output\logs"
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$setupLog = Join-Path $logDir ("setup-{0:yyyyMMdd-HHmmss}-{1}.log" -f (Get-Date), $PID)
$setupArgs = @("/SP-", "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/LOG=`"$setupLog`"")
# Windows PowerShell 5.1 で ExitCode を読めるよう、Handle を先に取っておく
$proc = Start-Process -FilePath $outputFile.FullName -ArgumentList $setupArgs -PassThru
$null = $proc.Handle
$proc.WaitForExit()
$code = $proc.ExitCode
Write-Host "  exit code: $code"
Write-Host "  log: $setupLog"

if ($code -eq 0) {
    Write-Host ""
    Write-Host "インストールしました。完了するには PC を再起動してください。" -ForegroundColor Green
    Write-Host "再起動するまでは、Rakukan による日本語入力ができないことがあります。" -ForegroundColor Yellow
    exit 0
}
if ($code -eq 20) {
    Write-Host "rakukan の登録を確かめられませんでした（終了コード 20）。PC を再起動してから、もう一度実行してください。" -ForegroundColor Red
} else {
    Write-Host "インストーラーが失敗しました（終了コード $code）。ログを確認してください。" -ForegroundColor Red
}
exit $code
