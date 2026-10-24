//! ログの 1 行目（ヘッダー、#73）。
//!
//! ヘッダーにだけ書く項目: 実行ファイルのフルパス、パッケージファミリー名（パッケージで
//! 動いていなければ `none`）、PID、起動インスタンス（`<PID>-<起動識別子>`）、ビルド。
//! ローテーションで新しい本体を開くたびに書く（[`crate::file::RotatingLog`]）。

/// ヘッダーの行（改行なし）。`component` は `tsf` / `host` など。
pub fn header_line(component: &str, stem: &str, build: &str) -> String {
    let exe = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".to_string());
    let package = package_family_name().unwrap_or_else(|| "none".to_string());
    format!(
        "rakukan log header: component={component} exe={exe} package={package} pid={} instance={stem} build={build}",
        std::process::id()
    )
}

/// このプロセスのパッケージファミリー名。パッケージで動いていなければ `None`。
#[cfg(windows)]
fn package_family_name() -> Option<String> {
    use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};
    use windows::Win32::Storage::Packaging::Appx::GetCurrentPackageFamilyName;
    let mut len: u32 = 0;
    // パッケージで動いていないときは APPMODEL_ERROR_NO_PACKAGE が返る
    let first = unsafe { GetCurrentPackageFamilyName(&mut len, windows::core::PWSTR::null()) };
    if first != ERROR_INSUFFICIENT_BUFFER || len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    let second =
        unsafe { GetCurrentPackageFamilyName(&mut len, windows::core::PWSTR(buf.as_mut_ptr())) };
    if second != ERROR_SUCCESS {
        return None;
    }
    // len は終端の NUL を含む
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

#[cfg(not(windows))]
fn package_family_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_names_the_origin() {
        let line = header_line("tsf", "12-34", "0.13.0@abc");
        assert!(line.starts_with("rakukan log header: component=tsf exe="));
        assert!(line.contains(&format!(" pid={} ", std::process::id())));
        assert!(line.contains(" instance=12-34 "));
        assert!(line.ends_with(" build=0.13.0@abc"));
        // テストはパッケージの外で動く
        assert!(line.contains(" package=none "));
        assert!(!line.contains('\n'));
    }
}
