use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use windows::{
    Win32::UI::WindowsAndMessaging::{GetForegroundWindow, MB_ICONERROR, MB_OK, MessageBoxW},
    core::PCWSTR,
};

use crate::globals::DllModule;

const SETTINGS_EXE_NAME: &str = "rakukan-settings.exe";
const SETTINGS_DIR_NAME: &str = "settings-ui";

pub fn launch_settings_app() {
    if let Err(err) = launch_settings_app_inner() {
        show_error(&format!(
            "設定アプリを起動できませんでした。\n{}\n\n{} をインストール先に配置してください。",
            err, SETTINGS_EXE_NAME
        ));
    }
}

fn launch_settings_app_inner() -> Result<()> {
    let dll_path = PathBuf::from(DllModule::get_path()?);
    let install_dir = dll_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("install directory not found"))?;
    let exe_path = install_dir.join(SETTINGS_DIR_NAME).join(SETTINGS_EXE_NAME);
    let fallback_path = install_dir.join(SETTINGS_EXE_NAME);
    let exe_path = if exe_path.exists() {
        exe_path
    } else {
        fallback_path
    };
    if !exe_path.exists() {
        bail!("missing file: {}", exe_path.display());
    }

    std::process::Command::new(&exe_path)
        .current_dir(exe_path.parent().unwrap_or(install_dir))
        .spawn()
        .with_context(|| format!("spawn {}", exe_path.display()))?;
    Ok(())
}

fn show_error(message: &str) {
    unsafe {
        let text = to_wide_z(message);
        let caption = to_wide_z("rakukan");
        let _ = MessageBoxW(
            GetForegroundWindow(),
            PCWSTR(text.as_ptr()),
            PCWSTR(caption.as_ptr()),
            MB_OK | MB_ICONERROR,
        );
    }
}

fn to_wide_z(text: &str) -> Vec<u16> {
    let mut wide: Vec<u16> = text.encode_utf16().collect();
    wide.push(0);
    wide
}
