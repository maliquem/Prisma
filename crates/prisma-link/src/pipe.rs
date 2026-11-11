use std::io;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::PWSTR;

/// `\\.\pipe\prisma-link-{sid}`.
pub fn pipe_name(sid: &str) -> String {
    format!(r"\\.\pipe\prisma-link-{sid}")
}

/// Nome do pipe para o usuário do processo atual.
pub fn default_pipe_name() -> io::Result<String> {
    current_user_sid().map(|sid| pipe_name(&sid))
}

/// SID do usuário do token do processo, como texto (`S-1-5-21-...`).
pub fn current_user_sid() -> io::Result<String> {
    let mut token = HANDLE::default();
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }?;
    let token = Token(token);

    let mut len = 0u32;
    // A primeira chamada só informa o tamanho; falha com ERROR_INSUFFICIENT_BUFFER.
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut len) };
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    // u64 para alinhar o TOKEN_USER.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buf.as_mut_ptr().cast()),
            len,
            &mut len,
        )
    }?;
    let user = unsafe { &*buf.as_ptr().cast::<TOKEN_USER>() };

    let mut s = PWSTR::null();
    unsafe { ConvertSidToStringSidW(user.User.Sid, &mut s) }?;
    let out = unsafe { s.to_string() };
    unsafe { LocalFree(Some(HLOCAL(s.0.cast()))) };
    out.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}
