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
    /// Tempo de GPU da cadeia em µs (média), para o menu; 0 sem medida (só o D3D11, o D3D10 e o D3D12 medem).
    /// Ocupa o antigo enchimento: o tamanho da memória não muda e uma DLL antiga no jogo ainda conecta.
    pub gpu_us: AtomicU32,
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

/// Custo do shader do Prisma no jogo, para o medidor do app: `Local\PrismaShaderCost-<pid do jogo>` (`cost_name`).
/// O motor de shader (nos dois modos) cria a memória e escreve com `publish`; o app abre, lê com `read` e sobe
/// `menu` para pedir o menu de shader no jogo. O HDR do Prisma manda o dele pela memória do gancho (`hdr_gpu_us`).
#[repr(C)]
pub struct Cost {
    /// `GetTickCount64` da última escrita: parado há mais de `COST_STALE_MS`, o efeito saiu.
    pub beat: AtomicU64,
    /// 1 com o efeito na tela.
    pub on: AtomicU32,
    /// Tempo de GPU do efeito em µs (média); 0 sem medida.
    pub gpu_us: AtomicU32,
    /// Intervalo entre quadros em µs, para a % da GPU; 0 e o medidor usa o tempo de quadro dele.
    pub frame_us: AtomicU32,
    /// O app sobe este número para pedir o menu do efeito no jogo; quem desenha abre ou fecha a cada mudança.
    pub menu: AtomicU32,
    name: [AtomicU16; COST_NAME_LEN],
}

pub const COST_NAME_LEN: usize = 64;
pub const COST_STALE_MS: u64 = 2000;
pub fn cost_name(pid: u32) -> String {
    format!("Local\\PrismaShaderCost-{pid}")
}

impl Cost {
    /// `name`: o perfil, cortado em `COST_NAME_LEN - 1` letras UTF-16.
    pub fn publish(&self, now_ms: u64, on: bool, gpu_ms: f64, frame_ms: f64, name: &str) {
        let w: Vec<u16> = name.encode_utf16().take(COST_NAME_LEN - 1).collect();
        for (i, slot) in self.name.iter().enumerate() {
            slot.store(w.get(i).copied().unwrap_or(0), Relaxed);
        }
        self.on.store(on as u32, Relaxed);
        self.gpu_us.store((gpu_ms.max(0.0) * 1000.0) as u32, Relaxed);
        self.frame_us.store((frame_ms.max(0.0) * 1000.0) as u32, Relaxed);
        self.beat.store(now_ms, Release);
    }

    /// (perfil, ms de GPU, ms do quadro) com o efeito na tela e escrito há pouco; ms 0 é sem medida.
    pub fn read(&self, now_ms: u64) -> Option<(String, f64, f64)> {
        let beat = self.beat.load(Acquire);
        if beat == 0 || now_ms.saturating_sub(beat) > COST_STALE_MS || self.on.load(Relaxed) == 0 {
            return None;
        }
        let w: Vec<u16> = self.name.iter().map(|c| c.load(Relaxed)).take_while(|c| *c != 0).collect();
        Some((String::from_utf16_lossy(&w), self.gpu_us.load(Relaxed) as f64 / 1000.0, self.frame_us.load(Relaxed) as f64 / 1000.0))
    }
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

    #[test]
    fn custo_ida_e_volta() {
        let c: Cost = unsafe { std::mem::zeroed() };
        assert_eq!(c.read(1000), None);
        c.publish(1000, true, 1.5, 16.0, "jogo-crt");
        assert_eq!(c.read(1500), Some(("jogo-crt".into(), 1.5, 16.0)));
        // Parado há tempo demais: o motor fechou sem limpar.
        assert_eq!(c.read(1000 + COST_STALE_MS + 1), None);
        c.publish(4000, false, 1.5, 16.0, &"x".repeat(100));
        assert_eq!(c.read(4000), None);
        c.publish(5000, true, 0.0, 0.0, &"x".repeat(100));
        assert_eq!(c.read(5000).unwrap().0.len(), COST_NAME_LEN - 1);
    }
}
