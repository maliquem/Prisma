use std::ffi::c_void;
use std::fs::File;
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::core::HSTRING;

use crate::pipe::current_user_sid;
use crate::{PROTOCOL, Reply, Request, read_frame, write_frame};

const BUFFER: u32 = 64 * 1024;

pub struct ServerInfo {
    pub app_version: String,
    pub caps: Vec<String>,
}

/// Cria o pipe `name` (só o usuário atual tem acesso) e atende um cliente por
/// vez, para sempre. Só retorna se não conseguir criar o pipe.
pub fn serve(
    name: &str,
    info: ServerInfo,
    mut handler: impl FnMut(Request) -> Option<Reply>,
) -> io::Result<()> {
    let pipe = create_pipe(name)?;
    let handle = windows::Win32::Foundation::HANDLE(pipe.as_raw_handle());
    loop {
        match unsafe { ConnectNamedPipe(handle, None) } {
            Ok(()) => {}
            Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => {}
            Err(_) => {
                // Cliente que entrou e saiu antes do Connect, ou erro passageiro.
                let _ = unsafe { DisconnectNamedPipe(handle) };
                thread::sleep(Duration::from_millis(50));
                continue;
            }
        }
        let _ = attend(&pipe, &info, &mut handler);
        let _ = unsafe { DisconnectNamedPipe(handle) };
    }
}

/// Atende um cliente até fim, erro de leitura ou de escrita.
fn attend(
    mut pipe: &File,
    info: &ServerInfo,
    handler: &mut impl FnMut(Request) -> Option<Reply>,
) -> io::Result<()> {
    let mut greeted = false;
    while let Some(req) = read_frame::<_, Request>(&mut pipe)? {
        let reply = match req {
            Request::Hello { .. } => {
                greeted = true;
                Some(Reply::Capabilities {
                    protocol: PROTOCOL,
                    app_version: info.app_version.clone(),
                    caps: info.caps.clone(),
                })
            }
            _ if !greeted => return Ok(()),
            Request::Unknown => None,
            other => handler(other),
        };
        if let Some(reply) = reply {
            write_frame(&mut pipe, &reply)?;
        }
    }
    Ok(())
}

fn create_pipe(name: &str) -> io::Result<File> {
    let sid = current_user_sid()?;
    let sddl = format!("D:P(A;;GA;;;{sid})S:(ML;;NW;;;ME)");
    let sd = SecurityDescriptor::from_sddl(&sddl)?;
    let sa = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0.0,
        bInheritHandle: false.into(),
    };
    let h = unsafe {
        CreateNamedPipeW(
            &HSTRING::from(name),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_FIRST_PIPE_INSTANCE,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            BUFFER,
            BUFFER,
            0,
            Some(&sa),
        )
    };
    if h.is_invalid() {
        return Err(io::Error::last_os_error());
    }
    // O File passa a ser o dono do handle e o fecha no drop.
    Ok(unsafe { File::from_raw_handle(h.0) })
}

/// Descritor alocado pelo sistema; liberado com LocalFree.
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl SecurityDescriptor {
    fn from_sddl(sddl: &str) -> io::Result<Self> {
        let mut sd = PSECURITY_DESCRIPTOR::default();
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &HSTRING::from(sddl),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
        }?;
        Ok(Self(sd))
    }
}

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        unsafe { LocalFree(Some(HLOCAL(self.0.0.cast::<c_void>()))) };
    }
}
