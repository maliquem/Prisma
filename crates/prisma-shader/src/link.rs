//! Memória compartilhada entre o motor (`prisma.exe --shader-profile`, modo Injeção) e a DLL dentro do jogo
//! (`shader-hook`): `Local\PrismaShader-<pid do jogo>`. Os dois chamam `CreateFileMappingW` com esse nome; quem
//! chega primeiro cria, e ela dura enquanto a DLL estiver no jogo, então o motor reaberto só reconecta.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU16, AtomicU32, AtomicU64, Ordering::*};

pub const MAGIC: u32 = 0x5052_5348;
pub const STATE_LOADED: u32 = 1;
pub const STATE_READY: u32 = 2;
pub const STATE_FAILED: u32 = 3;
/// Sem sinal do motor por esse tempo, a DLL para de desenhar: o jogo volta como era.
pub const BEAT_MS: u64 = 5000;
/// `flags`: `compare` segurado, o jogo sem efeito.
pub const FLAG_COMPARE: u32 = 1;
pub const PATH_LEN: usize = 520;

/// API que a DLL viu no primeiro Present do jogo.
pub const API_DX11: u32 = 1;
pub const API_D3D9: u32 = 2;
pub const API_DX12: u32 = 3;
pub const API_DX10: u32 = 4;
pub const API_GL: u32 = 5;
pub const API_VK: u32 = 6;

#[repr(C)]
pub struct Shared {
    pub magic: AtomicU32,
    pub state: AtomicU32,
    pub api: AtomicU32,
    pub flags: AtomicU32,
    pub beat: AtomicU64,
    pub presents: AtomicU64,
    /// Quadros que saíram com o efeito.
    pub drawn: AtomicU64,
    /// Janela do jogo: só a swapchain dela recebe o efeito.
    pub hwnd: AtomicU64,
    /// Sobe quando o motor troca o perfil (`path`).
    pub seq: AtomicU32,
    pad: u32,
    path: [AtomicU16; PATH_LEN],
}

impl Shared {
    /// Grava o caminho do perfil e avisa (`seq`). Caminho maior que `PATH_LEN` não cabe: fica o de antes.
    pub fn set_path(&self, p: &Path) -> bool {
        use std::os::windows::ffi::OsStrExt;
        let w: Vec<u16> = p.as_os_str().encode_wide().collect();
        if w.len() >= PATH_LEN {
            return false;
        }
        for (i, slot) in self.path.iter().enumerate() {
            slot.store(w.get(i).copied().unwrap_or(0), Relaxed);
        }
        self.seq.fetch_add(1, Release);
        true
    }

    pub fn path(&self) -> PathBuf {
        use std::os::windows::ffi::OsStringExt;
        let w: Vec<u16> = self.path.iter().map(|c| c.load(Relaxed)).take_while(|c| *c != 0).collect();
        std::ffi::OsString::from_wide(&w).into()
    }
}

pub fn map_name(pid: u32) -> String {
    format!("Local\\PrismaShader-{pid}")
}

/// Nome gravado em `api=` no perfil.
pub fn api_name(api: u32) -> &'static str {
    match api {
        API_DX11 => "dx11",
        API_D3D9 => "d3d9",
        API_DX12 => "dx12",
        API_DX10 => "dx10",
        API_GL => "opengl",
        API_VK => "vulkan",
        _ => "",
    }
}

/// O modo Injeção cobre estas (`api=` do perfil).
pub fn api_supported(api: &str) -> bool {
    matches!(api, "dx11" | "dx10" | "dx12" | "d3d9" | "opengl" | "vulkan")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caminho_ida_e_volta() {
        let s: Shared = unsafe { std::mem::zeroed() };
        assert!(s.set_path(Path::new(r"C:\Prisma\shaders\perfis\jogo-crt.txt")));
        assert_eq!(s.path(), PathBuf::from(r"C:\Prisma\shaders\perfis\jogo-crt.txt"));
        assert!(s.set_path(Path::new("b.txt")));
        assert_eq!(s.path(), PathBuf::from("b.txt"));
        assert!(!s.set_path(&PathBuf::from("x".repeat(PATH_LEN))));
        assert_eq!(s.seq.load(Relaxed), 2);
    }
}
