//! Windows Named Pipe の最小ラッパー。
//!
//! ランタイム依存を増やしたくないので tokio は使わず、
//! 同期 I/O の `std::io::Read`/`Write` を提供する `PipeStream` だけを持つ。

use std::ffi::OsStr;
use std::io::{self, Read, Write};
use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, INVALID_HANDLE_VALUE,
    LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows::Win32::Security::{
    GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX,
    ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE,
    PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, WaitNamedPipeW,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, PWSTR};

/// 現在ユーザーごとのパイプ名を構築する。
///
/// 例: `\\.\pipe\rakukan-engine-n_fuk`
pub fn pipe_name_for_current_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "default".into());
    let sanitized: String = user
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!(
        "\\\\.\\pipe\\{}-{}",
        crate::protocol::PIPE_BASE_NAME,
        sanitized
    )
}

fn to_wide_z(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

// ─── Security Descriptor ヘルパ ────────────────────────────────────────────────
//
// Named Pipe をデフォルト DACL で作ると Windows は「作成ユーザー + Administrators
// + SYSTEM」に全許可を与える。IME のサービス用としてはもう少し絞り、
// **現在のログインユーザーと SYSTEM だけ** に GENERIC_ALL を与える SDDL を
// 明示的に構築する。これにより、万一同一マシンに別ユーザーが存在する場合でも
// そのユーザー（および Administrators の別アカウント）からは接続できなくなる。

/// `LocalFree` で解放する必要がある PSECURITY_DESCRIPTOR を RAII で所有する。
/// pipe インスタンス（= 1 接続）を使い終えたら drop して OK。
pub struct OwnedSecurityDescriptor {
    psd: PSECURITY_DESCRIPTOR,
    sa: SECURITY_ATTRIBUTES,
}

// Safety: SECURITY_ATTRIBUTES は単なる POD（ポインタを持つ）。
// 構造体を他スレッドへ送ること自体は安全で、使う側が寿命を守る責任を負う。
unsafe impl Send for OwnedSecurityDescriptor {}
unsafe impl Sync for OwnedSecurityDescriptor {}

impl OwnedSecurityDescriptor {
    /// 現在のログインユーザー SID と SYSTEM に全許可を与える DACL を構築する。
    pub fn current_user_only() -> Result<Self> {
        let sid_str = current_user_sid_string().context("fetch current user SID")?;
        // D:  = DACL指定
        // P   = Protected（親からの継承を受けない）
        // A   = Allow
        // GA  = GENERIC_ALL
        // SY  = SYSTEM (well-known SID)
        let sddl = format!("D:P(A;;GA;;;{sid_str})(A;;GA;;;SY)");
        Self::from_sddl(&sddl)
    }

    fn from_sddl(sddl: &str) -> Result<Self> {
        let wsddl = to_wide_z(sddl);
        let mut psd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(wsddl.as_ptr()),
                1, // SDDL_REVISION_1
                &mut psd,
                None,
            )
            .map_err(|e| anyhow::anyhow!("ConvertStringSecurityDescriptor failed: {e}"))?;
        }
        let sa = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: psd.0,
            bInheritHandle: false.into(),
        };
        Ok(Self { psd, sa })
    }

    /// `CreateNamedPipeW` の lpSecurityAttributes 引数に渡すポインタ。
    pub fn as_ptr(&self) -> *const SECURITY_ATTRIBUTES {
        &self.sa as *const _
    }
}

impl Drop for OwnedSecurityDescriptor {
    fn drop(&mut self) {
        unsafe {
            if !self.psd.0.is_null() {
                let _ = LocalFree(HLOCAL(self.psd.0 as _));
                self.psd = PSECURITY_DESCRIPTOR::default();
            }
        }
    }
}

/// 現在のプロセスの token から `TokenUser` を取得して SID 文字列（例: `S-1-5-21-…`）を返す。
fn current_user_sid_string() -> Result<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .map_err(|e| anyhow::anyhow!("OpenProcessToken: {e}"))?;

        // 必要なバッファサイズを問い合わせる（最初の GetTokenInformation は
        // ERROR_INSUFFICIENT_BUFFER を返すので ok は無視する）
        let mut needed: u32 = 0;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        if needed == 0 {
            let _ = CloseHandle(token);
            bail!("GetTokenInformation returned zero needed-size");
        }

        let mut buf = vec![0u8; needed as usize];
        let res = GetTokenInformation(
            token,
            TokenUser,
            Some(buf.as_mut_ptr() as *mut _),
            needed,
            &mut needed,
        );
        let _ = CloseHandle(token);
        res.map_err(|e| anyhow::anyhow!("GetTokenInformation(TokenUser): {e}"))?;

        let tu = &*(buf.as_ptr() as *const TOKEN_USER);
        let sid_ptr = tu.User.Sid;
        if sid_ptr.0.is_null() {
            bail!("TOKEN_USER.Sid is null");
        }

        let mut sid_str = PWSTR::null();
        ConvertSidToStringSidW(sid_ptr, &mut sid_str)
            .map_err(|e| anyhow::anyhow!("ConvertSidToStringSidW: {e}"))?;
        if sid_str.0.is_null() {
            bail!("ConvertSidToStringSidW returned null");
        }

        // PWSTR を String に変換
        let mut len = 0usize;
        while *sid_str.0.add(len) != 0 {
            len += 1;
        }
        let slice = std::slice::from_raw_parts(sid_str.0, len);
        let s = String::from_utf16_lossy(slice);

        let _ = LocalFree(HLOCAL(sid_str.0 as _));
        Ok(s)
    }
}

/// 双方向 Named Pipe ストリーム。Read/Write 実装つき。
pub struct PipeStream {
    handle: HANDLE,
    /// サーバ側でパイプ作成時に使った security descriptor。
    /// `CreateNamedPipeW` は内部でコピーするので厳密には直後に drop しても良いが、
    /// 安全側に倒して pipe ハンドルと寿命を合わせる。クライアント側では None。
    _sd: Option<OwnedSecurityDescriptor>,
}

// Safety: Named Pipe ハンドルはスレッド境界を越えて使えるが、同一ハンドルを
// 同時に複数スレッドから read/write するのは未定義なので Sync は付けない。
unsafe impl Send for PipeStream {}

impl PipeStream {
    /// サーバ側: Named Pipe インスタンスを作成する。
    ///
    /// 明示的な DACL（current user + SYSTEM のみ）を持つ security descriptor を構築し、
    /// `CreateNamedPipeW` の lpSecurityAttributes に渡す。
    /// SD の構築に失敗した場合はエラー（デフォルト DACL へのサイレントフォールバックはしない）。
    pub fn create_server(pipe_name: &str) -> Result<Self> {
        let wname = to_wide_z(pipe_name);
        let sd = OwnedSecurityDescriptor::current_user_only()
            .context("build security descriptor for named pipe")?;
        let sa_ptr = sd.as_ptr();
        unsafe {
            let h = CreateNamedPipeW(
                PCWSTR(wname.as_ptr()),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024, // out buf
                64 * 1024, // in buf
                0,         // default timeout
                Some(sa_ptr),
            );
            if h == INVALID_HANDLE_VALUE || h.0.is_null() {
                bail!(
                    "CreateNamedPipeW failed: {}",
                    io::Error::last_os_error()
                );
            }
            Ok(Self {
                handle: h,
                _sd: Some(sd),
            })
        }
    }

    /// サーバ側: クライアント接続を待つ（ブロッキング）。
    pub fn accept(&self) -> Result<()> {
        unsafe {
            match ConnectNamedPipe(self.handle, None) {
                Ok(()) => Ok(()),
                Err(e) => {
                    // ERROR_PIPE_CONNECTED (535) は「既に接続済み」なので成功扱い
                    if e.code().0 as u32 == 0x8007_0217 || e.code().0 as u32 == 535 {
                        Ok(())
                    } else {
                        Err(anyhow::anyhow!("ConnectNamedPipe failed: {e}"))
                    }
                }
            }
        }
    }

    /// サーバ側: 現接続を切断する。
    #[allow(dead_code)]
    pub fn disconnect(&self) {
        unsafe {
            let _ = DisconnectNamedPipe(self.handle);
        }
    }

    /// クライアント側: 指定パイプへ接続する。存在しなければ WaitNamedPipeW でリトライ。
    pub fn connect_client(pipe_name: &str, total_timeout: Duration) -> Result<Self> {
        let wname = to_wide_z(pipe_name);
        let deadline = std::time::Instant::now() + total_timeout;
        loop {
            let r = unsafe {
                CreateFileW(
                    PCWSTR(wname.as_ptr()),
                    GENERIC_READ.0 | GENERIC_WRITE.0,
                    FILE_SHARE_NONE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAGS_AND_ATTRIBUTES(0),
                    HANDLE::default(),
                )
            };
            match r {
                Ok(h) if h != INVALID_HANDLE_VALUE && !h.0.is_null() => {
                    return Ok(Self {
                        handle: h,
                        _sd: None,
                    });
                }
                _ => {
                    let err = io::Error::last_os_error();
                    let is_busy = err.raw_os_error() == Some(ERROR_PIPE_BUSY.0 as i32);
                    if std::time::Instant::now() >= deadline {
                        bail!("connect_client: timeout: {err}");
                    }
                    if is_busy {
                        unsafe {
                            let _ = WaitNamedPipeW(PCWSTR(wname.as_ptr()), 200);
                        }
                    } else {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        }
    }
}

impl Read for PipeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut read: u32 = 0;
        let ok = unsafe { ReadFile(self.handle, Some(buf), Some(&mut read), None) };
        ok.map_err(|e| io::Error::new(io::ErrorKind::Other, format!("ReadFile: {e}")))?;
        if read == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "pipe closed"));
        }
        Ok(read as usize)
    }
}

impl Write for PipeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut written: u32 = 0;
        let ok = unsafe { WriteFile(self.handle, Some(buf), Some(&mut written), None) };
        ok.map_err(|e| io::Error::new(io::ErrorKind::Other, format!("WriteFile: {e}")))?;
        Ok(written as usize)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for PipeStream {
    fn drop(&mut self) {
        unsafe {
            if self.handle != INVALID_HANDLE_VALUE && !self.handle.0.is_null() {
                let _ = CloseHandle(self.handle);
            }
        }
    }
}
