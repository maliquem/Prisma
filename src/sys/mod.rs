pub mod audio;
pub mod audit;
pub mod autohdr;
pub mod autostart;
pub mod avsync;
pub mod catalog;
pub mod cputemp;
pub mod stores;
pub mod disks;
pub mod drs;
pub mod nvset;
pub mod etw;
pub mod events;
pub mod fileinfo;
pub mod frames;
pub mod fullscreen;
pub mod gamelog;
pub mod inject;
pub mod hdr;
pub mod hdrcal;
pub mod index;
pub mod keepalive;
pub mod kmt;
pub mod mft;
pub mod monitor;
pub mod nets;
pub mod nvml;
pub mod patterns;
pub mod power;
pub mod powerplan;
pub mod procs;
pub mod quick;
pub mod cmds;
pub mod reg;
pub mod services;
pub mod tools;
pub mod tweaks;
pub mod update;
pub mod uninstall;

use std::path::PathBuf;

fn folder(get: unsafe fn(Option<&mut [u16]>) -> u32) -> PathBuf {
    let mut buf = [0u16; 260];
    let n = unsafe { get(Some(&mut buf)) } as usize;
    PathBuf::from(String::from_utf16_lossy(&buf[..n.min(buf.len())]))
}

/// System32 pelo Windows, e não por `%SystemRoot%`, que qualquer programa troca para o usuário.
pub fn system_dir() -> PathBuf {
    folder(windows::Win32::System::SystemInformation::GetSystemDirectoryW)
}

pub fn windows_dir() -> PathBuf {
    folder(windows::Win32::System::SystemInformation::GetWindowsDirectoryW)
}

/// Caminho completo de um programa ou console do Windows. Só pelo nome, o Rust procura antes na
/// pasta do Prisma, e na versão portátil qualquer download pode deixar lá um `reg.exe` falso.
pub fn system_tool(name: &str) -> PathBuf {
    let file = if name.contains('.') { name.to_string() } else { format!("{name}.exe") };
    match file.to_ascii_lowercase().as_str() {
        "explorer.exe" => windows_dir().join(file),
        "powershell.exe" => system_dir().join("WindowsPowerShell\\v1.0").join(file),
        // Atalho dos aplicativos da Loja, onde fica o winget.
        "winget.exe" => known_folder(&windows::Win32::UI::Shell::FOLDERID_LocalAppData).join("Microsoft\\WindowsApps").join(file),
        _ => system_dir().join(file),
    }
}

/// Pasta do Windows pelo próprio Windows. `%ProgramFiles%` e `%APPDATA%` são texto que qualquer
/// programa troca para o usuário, e a cópia de administrador herda a troca.
pub fn known_folder(id: &windows::core::GUID) -> PathBuf {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::*;
    let path = unsafe {
        SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok().and_then(|p| {
            let text = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            text
        })
    };
    PathBuf::from(path.unwrap_or_default())
}
