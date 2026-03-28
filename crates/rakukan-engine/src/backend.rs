//! バックエンド選択モジュール
//!
//! 実行時に GPU を検出し、最適な llama.cpp バックエンドを選ぶ。
//! 優先順位: CUDA > Vulkan > CPU

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// llama.cpp の推論バックエンド
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// CUDA（Nvidia GPU 専用・最速）
    Cuda,
    /// Vulkan（AMD/Nvidia/Intel GPU 共通・推奨）
    Vulkan,
    /// CPU（低速・フォールバック）
    Cpu,
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Backend::Cuda => write!(f, "CUDA"),
            Backend::Vulkan => write!(f, "Vulkan"),
            Backend::Cpu => write!(f, "CPU"),
        }
    }
}

/// GPU の情報
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuInfo {
    pub name: String,
    pub vram_mb: Option<u64>,
}

/// バックエンド選択の結果
#[derive(Debug, Serialize, Deserialize)]
pub struct BackendSelection {
    pub backend: Backend,
    pub reason: String,
    pub detected_gpus: Vec<GpuInfo>,
}

/// バックエンドを自動選択する
///
/// 優先順位: CUDA > Vulkan > CPU
/// 環境変数 `RAKUKAN_BACKEND` で強制上書きできる
pub fn select_backend() -> BackendSelection {
    tracing::info!("backend::detect: start");

    // 環境変数による強制指定
    if let Ok(forced) = std::env::var("RAKUKAN_BACKEND") {
        let backend = match forced.to_lowercase().as_str() {
            "cuda" => Backend::Cuda,
            "vulkan" => Backend::Vulkan,
            "cpu" => Backend::Cpu,
            other => {
                tracing::warn!(
                    "backend::detect: unknown RAKUKAN_BACKEND={:?}, fallback to cpu",
                    other
                );
                Backend::Cpu
            }
        };
        tracing::info!("backend::detect: forced via RAKUKAN_BACKEND={}", backend);
        return BackendSelection {
            backend,
            reason: format!("環境変数 RAKUKAN_BACKEND='{}' で指定", forced),
            detected_gpus: vec![],
        };
    }

    // 保存済み設定の読み込み（PowerShell の detect-gpu.ps1 が生成）
    if let Some(saved) = load_saved_config() {
        tracing::info!("backend::detect: using saved backend={}", saved.backend);
        return saved;
    }

    // 実行時検出
    detect_at_runtime()
}

/// 保存済み設定を読み込む（%APPDATA%\rakukan\backend.json）
fn load_saved_config() -> Option<BackendSelection> {
    let config_path = config_dir()?.join("backend.json");
    if !config_path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&config_path).ok()?;
    let config: serde_json::Value = serde_json::from_str(&content).ok()?;

    let backend = match config["backend"].as_str()? {
        "cuda" => Backend::Cuda,
        "vulkan" => Backend::Vulkan,
        _ => Backend::Cpu,
    };

    let gpus = config["detected_gpus"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|name| GpuInfo {
                    name: name.to_string(),
                    vram_mb: None,
                })
                .collect()
        })
        .unwrap_or_default();

    Some(BackendSelection {
        backend,
        reason: "保存済み設定から読み込み".to_string(),
        detected_gpus: gpus,
    })
}

/// Rust から直接 GPU を検出する（保存済み設定がない場合のフォールバック）
fn detect_at_runtime() -> BackendSelection {
    let gpus = detect_gpus_wmi();

    // CUDA の確認（nvidia-smi の存在と nvcc の有無）
    if has_nvidia_gpu(&gpus) && nvidia_smi_available() {
        if nvcc_available() {
            return BackendSelection {
                backend: Backend::Cuda,
                reason: "Nvidia GPU + CUDA Toolkit を検出".to_string(),
                detected_gpus: gpus,
            };
        } else {
            // CUDA GPU はあるが Toolkit なし → Vulkan へ
            tracing::warn!(
                "backend::detect: NVIDIA GPU found but CUDA Toolkit missing, trying Vulkan"
            );
        }
    }

    // Vulkan の確認（レジストリ or vulkaninfo）
    if vulkan_available() {
        return BackendSelection {
            backend: Backend::Vulkan,
            reason: "Vulkan 対応 GPU を検出".to_string(),
            detected_gpus: gpus,
        };
    }

    // CPU フォールバック
    tracing::warn!("backend::detect: no GPU backend available, falling back to cpu (slow)");
    BackendSelection {
        backend: Backend::Cpu,
        reason: "Vulkan/CUDA が利用できないため CPU を使用（低速）".to_string(),
        detected_gpus: gpus,
    }
}

// ── 検出ユーティリティ ────────────────────

/// WMI 経由で GPU 一覧を取得
///
/// Windows 専用。Linux ではダミーを返す。
fn detect_gpus_wmi() -> Vec<GpuInfo> {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        // PowerShell の WMI クエリを使用
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Get-CimInstance Win32_VideoController | \
                 Where-Object { $_.Name -notmatch 'Microsoft Basic' } | \
                 Select-Object -ExpandProperty Name",
            ])
            .output();

        match output {
            Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .map(|name| GpuInfo {
                    name: name.to_string(),
                    vram_mb: None,
                })
                .collect(),
            _ => vec![],
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        vec![]
    }
}

fn has_nvidia_gpu(gpus: &[GpuInfo]) -> bool {
    gpus.iter()
        .any(|g| g.name.to_lowercase().contains("nvidia"))
}

fn nvidia_smi_available() -> bool {
    which::which("nvidia-smi").is_ok()
}

fn nvcc_available() -> bool {
    which::which("nvcc").is_ok()
}

fn vulkan_available() -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        // レジストリでVulkanドライバーを確認
        let output = Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                "Test-Path 'HKLM:\\SOFTWARE\\Khronos\\Vulkan\\Drivers'",
            ])
            .output();

        match output {
            Ok(out) => String::from_utf8_lossy(&out.stdout).trim() == "True",
            Err(_) => false,
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        which::which("vulkaninfo").is_ok()
    }
}

fn config_dir() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").ok()?;
        Some(PathBuf::from(appdata).join("rakukan"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let home = std::env::var("HOME").ok()?;
        Some(PathBuf::from(home).join(".config").join("rakukan"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_display() {
        assert_eq!(Backend::Cuda.to_string(), "CUDA");
        assert_eq!(Backend::Vulkan.to_string(), "Vulkan");
        assert_eq!(Backend::Cpu.to_string(), "CPU");
    }

    #[test]
    fn test_env_override_cuda() {
        unsafe {
            std::env::set_var("RAKUKAN_BACKEND", "cuda");
        }
        let selection = select_backend();
        assert_eq!(selection.backend, Backend::Cuda);
        unsafe {
            std::env::remove_var("RAKUKAN_BACKEND");
        }
    }

    #[test]
    fn test_env_override_cpu() {
        unsafe {
            std::env::set_var("RAKUKAN_BACKEND", "cpu");
        }
        let selection = select_backend();
        assert_eq!(selection.backend, Backend::Cpu);
        unsafe {
            std::env::remove_var("RAKUKAN_BACKEND");
        }
    }

    #[test]
    fn test_env_override_unknown_falls_back_to_cpu() {
        unsafe {
            std::env::set_var("RAKUKAN_BACKEND", "unknown_backend");
        }
        let selection = select_backend();
        assert_eq!(selection.backend, Backend::Cpu);
        unsafe {
            std::env::remove_var("RAKUKAN_BACKEND");
        }
    }
}
