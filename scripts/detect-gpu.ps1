# scripts/detect-gpu.ps1
# GPU を自動検出し、最適な llama.cpp バックエンドを提案する
# 単体でも実行可能、verify-karukan.ps1 からも呼び出される
#
# 出力: $env:RAKUKAN_BACKEND に選択結果をセットする
# 戻り値: "cpu" | "vulkan" | "cuda"

param([switch]$SaveResult)

function Read-ConfigTomlBackend {
    # config.toml の gpu_backend = "..." を読んでユーザー指定を取得する。
    # コメント行・空白行は無視。値がなければ $null を返す。
    $configPath = Join-Path $env:APPDATA "rakukan\config.toml"
    if (-not (Test-Path $configPath)) { return $null }
    foreach ($line in (Get-Content $configPath -ErrorAction SilentlyContinue)) {
        $line = $line.Trim()
        if ($line -match '^#') { continue }                     # コメント行
        if ($line -match '^gpu_backend\s*=\s*"([^"]+)"') {
            return $Matches[1].ToLower()                         # "cuda" / "vulkan" / "cpu"
        }
    }
    return $null
}

function Detect-Gpus {
    # WMI でインストール済みの GPU を取得
    $gpus = Get-CimInstance -ClassName Win32_VideoController |
        Where-Object { $_.Name -notmatch "Microsoft Basic" } |
        Select-Object Name, AdapterRAM, DriverVersion,
            @{N="VRAM_MB"; E={ [math]::Round($_.AdapterRAM / 1MB) }}
    return $gpus
}

function Test-VulkanSupport {
    # vulkaninfoコマンドで確認（Vulkan SDK or ドライバー付属）
    $vulkaninfo = Get-Command "vulkaninfo" -ErrorAction SilentlyContinue
    if ($vulkaninfo) {
        # stderr をエラーとして扱わないよう一時的に SilentlyContinue に変更
        $prev = $ErrorActionPreference
        $ErrorActionPreference = "SilentlyContinue"
        $result = & vulkaninfo --summary 2>&1
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $prev
        $text = ($result | Out-String)
        # 終了コード0 か、出力に Vulkan の有効性を示す文字列があれば OK
        if ($exitCode -eq 0 -or ($text -match "Vulkan Instance Version|GPU id\s*=\s*0")) {
            return $true
        }
        return $false
    }

    # vulkaninfoがなくてもドライバーが対応しているかレジストリで確認
    $vulkanKey = "HKLM:\SOFTWARE\Khronos\Vulkan\Drivers"
    if (Test-Path $vulkanKey) {
        $drivers = Get-ItemProperty $vulkanKey -ErrorAction SilentlyContinue
        return ($drivers -ne $null)
    }

    # DX12対応なら高確率でVulkanも動く（Windows 10以降の目安）
    $os = [System.Environment]::OSVersion.Version
    return ($os.Major -ge 10)
}

function Test-CudaSupport {
    # nvidia-smi があればCUDA対応GPUが存在する
    $nvidiaSmi = Get-Command "nvidia-smi" -ErrorAction SilentlyContinue
    if (-not $nvidiaSmi) { return $false, $null, $null }

    $smiOutput = nvidia-smi --query-gpu=name,compute_cap --format=csv,noheader 2>&1
    if ($LASTEXITCODE -ne 0) { return $false, $null, $null }

    # CUDA Toolkit がインストールされているか確認
    $nvcc = Get-Command "nvcc" -ErrorAction SilentlyContinue
    $cudaToolkitInstalled = ($nvcc -ne $null)

    # 最初のGPUの情報を取得
    $firstGpu = ($smiOutput | Select-Object -First 1) -split ","
    $gpuName = $firstGpu[0].Trim()
    $computeCap = $firstGpu[1].Trim()

    return $true, $gpuName, $cudaToolkitInstalled
}

function Select-Backend {
    param($gpus, $hasVulkan, $hasCuda, $cudaGpuName, $hasCudaToolkit)

    # GPU が一切ない場合
    if (-not $gpus) {
        return "cpu", "GPU が検出されませんでした。CPU バックエンドを使用します。"
    }

    # CUDA GPU があり Toolkit もある → CUDA が最速
    if ($hasCuda -and $hasCudaToolkit) {
        return "cuda", "Nvidia GPU ($cudaGpuName) + CUDA Toolkit を検出。CUDA バックエンドを推奨します。"
    }

    # CUDA GPU はあるが Toolkit がない → Vulkan にフォールバック
    if ($hasCuda -and -not $hasCudaToolkit) {
        if ($hasVulkan) {
            return "vulkan", "Nvidia GPU を検出しましたが CUDA Toolkit がありません。Vulkan バックエンドを推奨します。"
        }
    }

    # Vulkan が使える → 汎用的で推奨
    if ($hasVulkan) {
        return "vulkan", "GPU を検出。Vulkan バックエンドを推奨します（汎用・追加インストール不要）。"
    }

    # どちらも使えない → CPU
    return "cpu", "GPU はありますが Vulkan/CUDA が利用できません。CPU バックエンドを使用します（低速）。"
}

# ── メイン処理 ────────────────────────────

Write-Host ""
Write-Host "========================================" -ForegroundColor Cyan
Write-Host " GPU 検出 & バックエンド選択" -ForegroundColor Cyan
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# GPU 一覧を取得
Write-Host "[1] GPU を検出中..." -ForegroundColor Cyan
$gpus = Detect-Gpus

if ($gpus) {
    Write-Host "  検出された GPU:" -ForegroundColor Green
    $gpus | ForEach-Object {
        $vram = if ($_.VRAM_MB -gt 0) { "$($_.VRAM_MB) MB" } else { "不明" }
        Write-Host "    - $($_.Name)  VRAM: $vram  ドライバー: $($_.DriverVersion)"
    }
} else {
    Write-Host "  GPU は検出されませんでした（統合グラフィックスのみ、または仮想マシン）" -ForegroundColor Yellow
}

Write-Host ""

# Vulkan サポート確認
Write-Host "[2] Vulkan サポートを確認中..." -ForegroundColor Cyan
$hasVulkan = Test-VulkanSupport
if ($hasVulkan) {
    Write-Host "  [OK] Vulkan が利用可能です" -ForegroundColor Green
} else {
    Write-Host "  [--] Vulkan が利用できません" -ForegroundColor Yellow
}

# CUDA サポート確認
Write-Host "[3] CUDA サポートを確認中..." -ForegroundColor Cyan
$hasCuda, $cudaGpuName, $hasCudaToolkit = Test-CudaSupport
if ($hasCuda) {
    Write-Host "  [OK] Nvidia GPU を検出: $cudaGpuName" -ForegroundColor Green
    if ($hasCudaToolkit) {
        Write-Host "  [OK] CUDA Toolkit がインストールされています" -ForegroundColor Green
    } else {
        Write-Host "  [--] CUDA Toolkit が見つかりません（nvcc がない）" -ForegroundColor Yellow
        Write-Host "       https://developer.nvidia.com/cuda-downloads でインストール可能" -ForegroundColor Gray
    }
} else {
    Write-Host "  [--] Nvidia GPU または nvidia-smi が見つかりません" -ForegroundColor Gray
}

Write-Host ""

# [3.5] config.toml でユーザー指定があるか確認
$configPreference = Read-ConfigTomlBackend
if ($configPreference) {
    Write-Host "[3.5] config.toml に gpu_backend = \"$configPreference\" が指定されています" -ForegroundColor Cyan
} else {
    Write-Host "[3.5] config.toml に gpu_backend 指定なし -> 自動選択します" -ForegroundColor Gray
}

# バックエンド選択
Write-Host "[4] 最適なバックエンドを選択..." -ForegroundColor Cyan
$selected, $reason = Select-Backend $gpus $hasVulkan $hasCuda $cudaGpuName $hasCudaToolkit

Write-Host ""
Write-Host "  推奨バックエンド: " -NoNewline
switch ($selected) {
    "cuda"   { Write-Host $selected.ToUpper() -ForegroundColor Green }
    "vulkan" { Write-Host $selected -ForegroundColor Cyan }
    "cpu"    { Write-Host $selected.ToUpper() -ForegroundColor Yellow }
}
Write-Host "  理由: $reason" -ForegroundColor Gray
Write-Host ""

# インタラクティブ選択（-SaveResult 時はスキップして自動選択）
if ($SaveResult) {
    # config.toml 指定があればそちらを優先、なければ自動選択
    if ($configPreference -and $configPreference -in @("cuda","vulkan","cpu")) {
        $finalBackend = $configPreference
        Write-Host "  config.toml 優先: $finalBackend" -ForegroundColor Green
    } else {
        $finalBackend = $selected
        Write-Host "  自動選択: $finalBackend" -ForegroundColor Green
    }
} else {
    # config.toml 指定があればデフォルト候補として提示
    $defaultChoice = if ($configPreference) { $configPreference } else { $selected }
    if ($configPreference) {
        Write-Host "  config.toml に gpu_backend = \"$configPreference\" が指定されています" -ForegroundColor Cyan
    }
    Write-Host "  このバックエンドで確定しますか？" -ForegroundColor Cyan
    Write-Host "  [Enter] $defaultChoice で確定 / [c] CPU / [v] Vulkan / [u] CUDA"
    $input = Read-Host "  選択"
    $finalBackend = switch ($input.ToLower()) {
        "c"     { "cpu" }
        "v"     { "vulkan" }
        "u"     { "cuda" }
        default { $defaultChoice }
    }
}

Write-Host ""
Write-Host "  確定: $finalBackend" -ForegroundColor Green

# 環境変数にセット（呼び出し元スクリプトで参照可能）
$env:RAKUKAN_BACKEND = $finalBackend

# backend.json には保存しない。設定の正は config.toml とする。
Write-Host "  backend.json には保存しません。gpu_backend は config.toml で管理します。" -ForegroundColor Gray
Write-Host ""

# 戻り値
return $finalBackend
