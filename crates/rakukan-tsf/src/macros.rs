#[allow(dead_code)]
/// anyhow::Error を windows::core::Error に変換
pub fn to_win_err(e: anyhow::Error) -> windows::core::Error {
    windows::core::Error::new(
        windows::Win32::Foundation::E_FAIL,
        e.to_string(),
    )
}

#[macro_export]
macro_rules! check_err {
    ($result:expr) => {
        match $result {
            Ok(_) => ::windows::Win32::Foundation::S_OK,
            Err(e) => { tracing::error!("{:?}", e); ::windows::Win32::Foundation::E_FAIL }
        }
    };
}

#[macro_export]
macro_rules! win_result {
    ($e:expr) => { $e.map_err($crate::macros::to_win_err) };
}

#[macro_export]
macro_rules! check_win32_err {
    ($result:expr) => {
        if $result == 0 { Ok(()) } else { Err(::windows::core::Error::from_win32()) }
    };
    ($result:expr, $ok_val:expr) => {
        if $result == 0 { Ok($ok_val) } else { Err(::windows::core::Error::from_win32()) }
    };
}
