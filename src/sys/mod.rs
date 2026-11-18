pub mod audio;
pub mod eq;
pub mod eqapo;
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
pub mod nav;
pub mod osk;
pub mod macros;
pub mod keymap;
pub mod pads;
pub mod pad_sound;
pub mod bt_audio;
pub mod audio_haptic;
pub mod dsx;
pub mod remap;
pub mod patterns;
pub mod power;
pub mod powerplan;
pub mod procs;
pub mod radial;
pub mod quick;
pub mod cmds;
pub mod copy;
pub mod copylist;
pub mod reg;
pub mod shellmenu;
pub mod services;
pub mod tools;
pub mod tweaks;
pub mod update;
pub mod uninstall;
pub mod xbridge;
pub mod hidhide;
pub mod nefarius;

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

/// Texto do Windows: UTF-16 terminado em zero.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Texto de um buffer UTF-16 do Windows, até o primeiro zero.
pub fn from_wide(b: &[u16]) -> String {
    let n = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf16_lossy(&b[..n])
}

/// Números little-endian dentro de um bloco de bytes do Windows.
pub fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
pub fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
pub fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// Um SHA-256 do Windows (BCrypt) em andamento; solto ao sair.
pub(crate) struct Sha(windows::Win32::Security::Cryptography::BCRYPT_HASH_HANDLE);

impl Sha {
    pub(crate) fn new() -> std::io::Result<Self> {
        use windows::Win32::Security::Cryptography::*;
        let mut h = BCRYPT_HASH_HANDLE::default();
        unsafe { BCryptCreateHash(BCRYPT_SHA256_ALG_HANDLE, &mut h, None, None, 0) }.ok().map_err(|_| std::io::Error::other("BCrypt"))?;
        Ok(Sha(h))
    }

    pub(crate) fn add(&self, data: &[u8]) -> std::io::Result<()> {
        unsafe { windows::Win32::Security::Cryptography::BCryptHashData(self.0, data, 0) }.ok().map_err(|_| std::io::Error::other("BCrypt"))
    }

    pub(crate) fn finish(&self) -> std::io::Result<[u8; 32]> {
        let mut out = [0u8; 32];
        unsafe { windows::Win32::Security::Cryptography::BCryptFinishHash(self.0, &mut out, 0) }.ok().map_err(|_| std::io::Error::other("BCrypt"))?;
        Ok(out)
    }

    /// O SHA-256 de `data` de uma vez.
    pub(crate) fn digest(data: &[u8]) -> Option<[u8; 32]> {
        let s = Sha::new().ok()?;
        s.add(data).ok()?;
        s.finish().ok()
    }
}

impl Drop for Sha {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Security::Cryptography::BCryptDestroyHash(self.0);
        }
    }
}
