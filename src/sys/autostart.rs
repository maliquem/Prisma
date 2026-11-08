//! Aplicativos de inicialização. Liga e desliga pelo mesmo registro do Gerenciador
//! de Tarefas, então os dois sempre mostram o mesmo estado.

use std::path::PathBuf;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;

const RUN: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN32: &str = "Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run";
const APPROVED: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Source {
    UserRun,
    MachineRun,
    Machine32Run,
    UserFolder,
    CommonFolder,
}

impl Source {
    fn approved(self) -> (HKEY, String) {
        match self {
            Source::UserRun => (HKEY_CURRENT_USER, format!("{APPROVED}\\Run")),
            Source::MachineRun => (HKEY_LOCAL_MACHINE, format!("{APPROVED}\\Run")),
            Source::Machine32Run => (HKEY_LOCAL_MACHINE, format!("{APPROVED}\\Run32")),
            Source::UserFolder => (HKEY_CURRENT_USER, format!("{APPROVED}\\StartupFolder")),
            Source::CommonFolder => (HKEY_LOCAL_MACHINE, format!("{APPROVED}\\StartupFolder")),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Source::UserRun => "Registro, só este usuário",
            Source::MachineRun => "Registro, todos os usuários",
            Source::Machine32Run => "Registro, programas de 32 bits",
            Source::UserFolder => "Pasta Inicializar",
            Source::CommonFolder => "Pasta Inicializar, todos os usuários",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub name: String,
    pub command: String,
    pub path: String,
    pub source: Source,
    pub enabled: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Valores de texto e binários de uma chave.
fn values(root: HKEY, sub: &str, access: REG_SAM_FLAGS) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    unsafe {
        let mut key = HKEY::default();
        let s = wide(sub);
        if RegOpenKeyExW(root, PCWSTR(s.as_ptr()), None, KEY_READ | access, &mut key).is_err() {
            return out;
        }
        let mut i = 0;
        loop {
            let mut name = [0u16; 512];
            let mut nlen = name.len() as u32;
            let mut data = vec![0u8; 8192];
            let mut dlen = data.len() as u32;
            let r = RegEnumValueW(
                key,
                i,
                Some(PWSTR(name.as_mut_ptr())),
                &mut nlen,
                None,
                None,
                Some(data.as_mut_ptr()),
                Some(&mut dlen),
            );
            if r.is_err() {
                break;
            }
            data.truncate(dlen as usize);
            out.push((String::from_utf16_lossy(&name[..nlen as usize]), data));
            i += 1;
        }
        let _ = RegCloseKey(key);
    }
    out
}

fn reg_text(b: &[u8]) -> String {
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let end = w.iter().position(|c| *c == 0).unwrap_or(w.len());
    String::from_utf16_lossy(&w[..end])
}

fn expand(s: &str) -> String {
    let mut out = s.to_string();
    for (k, v) in std::env::vars() {
        let tag = format!("%{k}%");
        if out.to_ascii_lowercase().contains(&tag.to_ascii_lowercase()) {
            let low = out.to_ascii_lowercase();
            if let Some(i) = low.find(&tag.to_ascii_lowercase()) {
                out.replace_range(i..i + tag.len(), &v);
            }
        }
    }
    out
}

/// Tira o executável de uma linha de comando, com ou sem aspas.
pub fn exe_of(cmd: &str) -> String {
    let c = expand(cmd.trim());
    if let Some(rest) = c.strip_prefix('"') {
        return rest.split('"').next().unwrap_or("").to_string();
    }
    let low = c.to_ascii_lowercase();
    if let Some(i) = low.find(".exe") {
        return c[..i + 4].to_string();
    }
    c.split(' ').next().unwrap_or("").to_string()
}

fn state(source: Source, name: &str) -> bool {
    let (root, sub) = source.approved();
    let list = values(root, &sub, REG_SAM_FLAGS(0));
    match list.iter().find(|v| v.0.eq_ignore_ascii_case(name)) {
        Some((_, d)) if !d.is_empty() => d[0] & 1 == 0,
        _ => true,
    }
}

/// Pelo Windows, e não por `%APPDATA%`: remover apaga um arquivo desta pasta.
fn folder(source: Source) -> Option<PathBuf> {
    use windows::Win32::UI::Shell::*;
    let dir = match source {
        Source::UserFolder => super::known_folder(&FOLDERID_Startup),
        Source::CommonFolder => super::known_folder(&FOLDERID_CommonStartup),
        _ => return None,
    };
    (!dir.as_os_str().is_empty()).then_some(dir)
}

pub fn list() -> Vec<Item> {
    let mut out = Vec::new();
    for (root, sub, src, access) in [
        (HKEY_CURRENT_USER, RUN, Source::UserRun, REG_SAM_FLAGS(0)),
        (HKEY_LOCAL_MACHINE, RUN, Source::MachineRun, KEY_WOW64_64KEY),
        (HKEY_LOCAL_MACHINE, RUN32, Source::Machine32Run, KEY_WOW64_64KEY),
    ] {
        for (name, data) in values(root, sub, access) {
            let command = reg_text(&data);
            if command.is_empty() {
                continue;
            }
            out.push(Item { path: exe_of(&command), enabled: state(src, &name), name, command, source: src });
        }
    }
    for src in [Source::UserFolder, Source::CommonFolder] {
        let Some(dir) = folder(src) else { continue };
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.eq_ignore_ascii_case("desktop.ini") {
                continue;
            }
            let path = e.path().display().to_string();
            out.push(Item { enabled: state(src, &name), name, command: path.clone(), path, source: src });
        }
    }
    out.sort_by_key(|i| i.name.to_lowercase());
    out
}

fn set_binary(root: HKEY, sub: &str, name: &str, data: &[u8]) -> bool {
    unsafe {
        let mut key = HKEY::default();
        let s = wide(sub);
        let opts = REG_OPTION_NON_VOLATILE;
        if RegCreateKeyExW(root, PCWSTR(s.as_ptr()), None, None, opts, KEY_SET_VALUE, None, &mut key, None).is_err() {
            return false;
        }
        let n = wide(name);
        let ok = RegSetValueExW(key, PCWSTR(n.as_ptr()), None, REG_BINARY, Some(data)).is_ok();
        let _ = RegCloseKey(key);
        ok
    }
}

pub fn set_enabled(item: &Item, on: bool) -> bool {
    let (root, sub) = item.source.approved();
    let mut data = [0u8; 12];
    if on {
        data[0] = 2;
    } else {
        data[0] = 3;
        let ft = unsafe { GetSystemTimeAsFileTime() };
        data[4..8].copy_from_slice(&ft.dwLowDateTime.to_le_bytes());
        data[8..12].copy_from_slice(&ft.dwHighDateTime.to_le_bytes());
    }
    set_binary(root, &sub, &item.name, &data)
}

/// Adiciona um programa para abrir com este usuário.
pub fn add(path: &str) -> bool {
    let name = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let value = wide(&format!("\"{path}\""));
    let bytes = unsafe { std::slice::from_raw_parts(value.as_ptr() as *const u8, value.len() * 2) };
    unsafe {
        let mut key = HKEY::default();
        let s = wide(RUN);
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(s.as_ptr()), None, KEY_SET_VALUE, &mut key).is_err() {
            return false;
        }
        let n = wide(&name);
        let ok = RegSetValueExW(key, PCWSTR(n.as_ptr()), None, REG_SZ, Some(bytes)).is_ok();
        let _ = RegCloseKey(key);
        ok
    }
}

/// Tira da inicialização. O programa em si continua instalado.
pub fn remove(item: &Item) -> bool {
    let sub = match item.source {
        Source::UserRun | Source::MachineRun => RUN,
        Source::Machine32Run => RUN32,
        _ => return std::fs::remove_file(&item.path).is_ok(),
    };
    let root = if item.source == Source::UserRun { HKEY_CURRENT_USER } else { HKEY_LOCAL_MACHINE };
    let access = if item.source == Source::UserRun { KEY_SET_VALUE } else { KEY_SET_VALUE | KEY_WOW64_64KEY };
    unsafe {
        let mut key = HKEY::default();
        let s = wide(sub);
        if RegOpenKeyExW(root, PCWSTR(s.as_ptr()), None, access, &mut key).is_err() {
            return false;
        }
        let n = wide(&item.name);
        let ok = RegDeleteValueW(key, PCWSTR(n.as_ptr())) == ERROR_SUCCESS;
        let _ = RegCloseKey(key);
        ok
    }
}
