use std::fs::{File, OpenOptions};
use std::io::{self, ErrorKind};
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::AsRawHandle;
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Pipes::PeekNamedPipe;

use crate::{Capabilities, PROTOCOL, Reply, Request, read_frame, write_frame};

/// Quanto `connect` espera pelas `Capabilities` depois do `Hello`.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Intervalo entre consultas ao pipe enquanto se espera uma resposta.
const POLL: Duration = Duration::from_millis(20);

/// SECURITY_ANONYMOUS: o servidor não pode se passar pelo cliente.
const SECURITY_ANONYMOUS: u32 = 0;

pub struct Client {
    pipe: File,
    caps: Capabilities,
}

impl Client {
    /// Abre o pipe, manda `Hello` e espera `Capabilities` por até `CONNECT_TIMEOUT`
    /// (`ErrorKind::TimedOut` depois disso). Aceita qualquer protocolo do servidor;
    /// o que vale são as capacidades anunciadas.
    pub fn connect(name: &str, app_version: &str) -> io::Result<Client> {
        let mut pipe = OpenOptions::new()
            .read(true)
            .write(true)
            .security_qos_flags(SECURITY_ANONYMOUS)
            .open(name)?;
        write_frame(
            &mut pipe,
            &Request::Hello {
                protocol: PROTOCOL,
                app_version: app_version.to_owned(),
            },
        )?;
        wait_readable(&pipe, Instant::now() + CONNECT_TIMEOUT)?;
        match read_frame::<_, Reply>(&mut pipe)? {
            Some(Reply::Capabilities {
                protocol,
                app_version,
                caps,
            }) if protocol >= 1 => Ok(Client {
                pipe,
                caps: Capabilities {
                    protocol,
                    app_version,
                    caps,
                },
            }),
            Some(_) => Err(io::Error::new(
                ErrorKind::InvalidData,
                "esperava Capabilities",
            )),
            None => Err(ErrorKind::UnexpectedEof.into()),
        }
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.caps
    }

    pub fn send(&mut self, req: &Request) -> io::Result<()> {
        write_frame(&mut self.pipe, req)
    }

    /// Próxima resposta conhecida; `Reply::Unknown` é pulada.
    pub fn recv(&mut self) -> io::Result<Reply> {
        loop {
            match read_frame::<_, Reply>(&mut self.pipe)? {
                Some(Reply::Unknown) => continue,
                Some(r) => return Ok(r),
                None => return Err(ErrorKind::UnexpectedEof.into()),
            }
        }
    }

    /// Como `recv`, mas desiste com `ErrorKind::TimedOut` se nada chegar em `limit`.
    /// Um quadro que começou a chegar é lido até o fim.
    pub fn recv_timeout(&mut self, limit: Duration) -> io::Result<Reply> {
        let deadline = Instant::now() + limit;
        loop {
            wait_readable(&self.pipe, deadline)?;
            match read_frame::<_, Reply>(&mut self.pipe)? {
                Some(Reply::Unknown) => continue,
                Some(r) => return Ok(r),
                None => return Err(ErrorKind::UnexpectedEof.into()),
            }
        }
    }

    /// `false` quando o servidor fechou o pipe.
    pub fn alive(&self) -> bool {
        let h = HANDLE(self.pipe.as_raw_handle());
        unsafe { PeekNamedPipe(h, None, 0, None, None, None) }.is_ok()
    }
}

/// Espera haver bytes para ler no pipe até `deadline`. Erro do pipe (servidor
/// fechou) sai na hora; prazo vencido vira `ErrorKind::TimedOut`.
fn wait_readable(pipe: &File, deadline: Instant) -> io::Result<()> {
    let h = HANDLE(pipe.as_raw_handle());
    loop {
        let mut avail = 0u32;
        unsafe { PeekNamedPipe(h, None, 0, None, Some(&mut avail), None) }?;
        if avail > 0 {
            return Ok(());
        }
        let now = Instant::now();
        if now >= deadline {
            return Err(ErrorKind::TimedOut.into());
        }
        thread::sleep(POLL.min(deadline - now));
    }
}
