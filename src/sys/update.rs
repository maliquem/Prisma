//! Atualização pelo GitHub: procura a versão mais recente, confere e troca o executável.
//! Nada é instalado sem a pessoa pedir. A busca só envia o pedido da página de versões.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Cryptography::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const REPO: &str = "maliquem/Prisma";
const ASSET: &str = "Prisma.exe";
const MAX_SIZE: usize = 64 << 20;

#[derive(Clone, Default, PartialEq, Debug)]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    Latest,
    Available,
    Downloading(u32),
    Installing,
    Failed(&'static str),
}

#[derive(Clone, Default)]
pub struct State {
    pub phase: Phase,
    pub version: String,
    pub notes: String,
}

pub type Shared = Arc<Mutex<State>>;

pub fn current() -> String {
    #[cfg(debug_assertions)]
    if let Some(v) = std::env::args().find_map(|a| a.strip_prefix("--fake-version=").map(str::to_string)) {
        return v;
    }
    env!("CARGO_PKG_VERSION").to_string()
}

fn wide_host(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    Some((host.to_string(), format!("/{path}")))
}

struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

/// Baixa por HTTPS. O WinHTTP segue os redirecionamentos e recusa voltar para HTTP.
fn get(url: &str, mut progress: impl FnMut(usize, usize)) -> Result<(u32, Vec<u8>), ()> {
    let (host, path) = wide_host(url).ok_or(())?;
    let agent = HSTRING::from(format!("Prisma/{}", env!("CARGO_PKG_VERSION")));
    unsafe {
        let session = Handle(WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return Err(());
        }
        let conn = Handle(WinHttpConnect(session.0, &HSTRING::from(host), 443, 0));
        if conn.0.is_null() {
            return Err(());
        }
        let req = Handle(WinHttpOpenRequest(conn.0, w!("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE));
        if req.0.is_null() {
            return Err(());
        }
        let accept: Vec<u16> = "Accept: application/vnd.github+json\r\n".encode_utf16().collect();
        WinHttpAddRequestHeaders(req.0, &accept, WINHTTP_ADDREQ_FLAG_ADD).map_err(|_| ())?;
        WinHttpSendRequest(req.0, None, None, 0, 0, 0).map_err(|_| ())?;
        WinHttpReceiveResponse(req.0, std::ptr::null_mut()).map_err(|_| ())?;
        let (mut status, mut total) = (0u32, 0u32);
        let flag = WINHTTP_QUERY_FLAG_NUMBER;
        let mut size = 4u32;
        WinHttpQueryHeaders(req.0, WINHTTP_QUERY_STATUS_CODE | flag, PCWSTR::null(), Some(&mut status as *mut u32 as *mut _), &mut size, std::ptr::null_mut()).map_err(|_| ())?;
        let mut size = 4u32;
        let _ = WinHttpQueryHeaders(req.0, WINHTTP_QUERY_CONTENT_LENGTH | flag, PCWSTR::null(), Some(&mut total as *mut u32 as *mut _), &mut size, std::ptr::null_mut());
        let mut body = Vec::with_capacity(total as usize);
        let mut buf = vec![0u8; 64 << 10];
        loop {
            let mut read = 0u32;
            WinHttpReadData(req.0, buf.as_mut_ptr() as *mut _, buf.len() as u32, &mut read).map_err(|_| ())?;
            if read == 0 {
                break;
            }
            body.extend_from_slice(&buf[..read as usize]);
            if body.len() > MAX_SIZE {
                return Err(());
            }
            progress(body.len(), total as usize);
        }
        Ok((status, body))
    }
}

/// Lê um texto do JSON da API. A resposta é conhecida: não vale carregar um analisador por isso.
fn json_text(json: &str, key: &str) -> Option<String> {
    let start = json.find(&format!("\"{key}\":"))? + key.len() + 3;
    let rest = json[start..].trim_start().strip_prefix('"')?;
    let mut out = String::new();
    let mut it = rest.chars();
    while let Some(c) = it.next() {
        match c {
            '"' => return Some(out),
            '\\' => match it.next()? {
                'n' => out.push('\n'),
                'r' | 't' => out.push(' '),
                'u' => {
                    let hex: String = it.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?).unwrap_or(' '));
                }
                x => out.push(x),
            },
            _ => out.push(c),
        }
    }
    None
}

fn parts(v: &str) -> Option<[u32; 3]> {
    let mut it = v.trim_start_matches('v').split('.');
    Some([it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?])
}

pub fn is_newer(found: &str, have: &str) -> bool {
    matches!((parts(found), parts(have)), (Some(a), Some(b)) if a > b)
}

fn sha256(data: &[u8]) -> Option<String> {
    let mut out = [0u8; 32];
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out) }.ok().ok()?;
    Some(out.iter().map(|b| format!("{b:02x}")).collect())
}

fn set(s: &Shared, phase: Phase) {
    s.lock().unwrap().phase = phase;
}

/// Procura em segundo plano. Devolve na hora; o resultado aparece em `shared`.
pub fn check(shared: Shared) {
    set(&shared, Phase::Checking);
    let _ = std::thread::Builder::new().name("atualizacao".into()).spawn(move || {
        let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
        let phase = match get(&url, |_, _| {}) {
            Ok((200, body)) => {
                let json = String::from_utf8_lossy(&body);
                let tag = json_text(&json, "tag_name").unwrap_or_default();
                if parts(&tag).is_none() {
                    Phase::Failed("Não consegui acessar o GitHub.")
                } else if is_newer(&tag, &current()) {
                    let mut s = shared.lock().unwrap();
                    s.version = tag.trim_start_matches('v').to_string();
                    s.notes = json_text(&json, "body").unwrap_or_default();
                    Phase::Available
                } else {
                    Phase::Latest
                }
            }
            Ok((404, _)) => Phase::Failed("Ainda não há versão publicada."),
            _ => Phase::Failed("Não consegui acessar o GitHub."),
        };
        set(&shared, phase);
    });
}

fn side_paths() -> Option<(PathBuf, PathBuf, PathBuf)> {
    let exe = std::env::current_exe().ok()?;
    Some((exe.with_extension("new"), exe.with_extension("old"), exe))
}

fn replace(data: &[u8]) -> Result<(), &'static str> {
    const DENIED: &str = "Sem permissão nesta pasta. Reabra como administrador.";
    const STUCK: &str = "Não consegui trocar o programa. Nada foi alterado.";
    let (new, old, exe) = side_paths().ok_or(STUCK)?;
    std::fs::write(&new, data).map_err(|_| DENIED)?;
    let _ = std::fs::remove_file(&old);
    // O Windows deixa renomear o executável em uso, mas não sobrescrever.
    if std::fs::rename(&exe, &old).is_err() {
        let _ = std::fs::remove_file(&new);
        return Err(DENIED);
    }
    if std::fs::rename(&new, &exe).is_err() {
        let _ = std::fs::rename(&old, &exe);
        return Err(STUCK);
    }
    let dir = exe.parent().map(PathBuf::from).unwrap_or_default();
    let started = std::process::Command::new(&exe)
        .arg(format!("--updated={}", std::process::id()))
        .current_dir(dir)
        .creation_flags(0x0000_0008)
        .spawn();
    if started.is_err() {
        let _ = std::fs::remove_file(&exe);
        let _ = std::fs::rename(&old, &exe);
        return Err(STUCK);
    }
    Ok(())
}

/// Baixa, confere e troca. Se der certo, fecha este processo pela mensagem `quit` em `hub`.
pub fn install(shared: Shared, hub: isize, quit: u32) {
    let tag = format!("v{}", shared.lock().unwrap().version);
    set(&shared, Phase::Downloading(0));
    let _ = std::thread::Builder::new().name("atualizacao".into()).spawn(move || {
        let base = format!("https://github.com/{REPO}/releases/download/{tag}/{ASSET}");
        const BAD: &str = "O arquivo baixado não confere. Nada foi alterado.";
        const OFFLINE: &str = "Não consegui acessar o GitHub.";
        let want = match get(&format!("{base}.sha256"), |_, _| {}) {
            Ok((200, b)) => String::from_utf8_lossy(&b).split_whitespace().next().unwrap_or_default().to_ascii_lowercase(),
            _ => return set(&shared, Phase::Failed(OFFLINE)),
        };
        let show = shared.clone();
        let data = match get(&base, move |got, total| {
            if total > 0 {
                set(&show, Phase::Downloading((got * 100 / total) as u32));
            }
        }) {
            Ok((200, b)) => b,
            _ => return set(&shared, Phase::Failed(OFFLINE)),
        };
        if want.len() != 64 || sha256(&data).as_deref() != Some(want.as_str()) || !data.starts_with(b"MZ") {
            return set(&shared, Phase::Failed(BAD));
        }
        set(&shared, Phase::Installing);
        match replace(&data) {
            Ok(()) => unsafe {
                let _ = PostMessageW(Some(HWND(hub as *mut _)), quit, WPARAM(0), LPARAM(0));
            },
            Err(e) => set(&shared, Phase::Failed(e)),
        }
    });
}

/// A nova versão espera a antiga sair antes de assumir, e depois apaga o arquivo velho.
pub fn after_update(pid: u32) {
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, 15_000);
            let _ = CloseHandle(h);
        }
    }
    if let Some((_, old, _)) = side_paths() {
        for _ in 0..20 {
            if std::fs::remove_file(&old).is_ok() || !old.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}
