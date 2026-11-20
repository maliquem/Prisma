//! Custo dos efeitos do Prisma no jogo, para o medidor (`prisma_shader::link::Cost`): o motor de shader escreve
//! (nos dois modos) e o app lê. O HDR do Prisma usa a mesma memória com o tipo `COST_HDR`.

use std::path::Path;
use std::sync::atomic::Ordering::Relaxed;

use prisma_shader::link::{cost_name, Cost};
use windows::core::HSTRING;
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::*;
use windows::Win32::System::SystemInformation::GetTickCount64;

pub struct CostMap {
    map: HANDLE,
    view: *const Cost,
    /// Quem escreve: ao fechar, zera o sinal de vida e o medidor tira a linha na hora.
    owner: bool,
    /// Último pedido de menu já atendido.
    menu: u32,
}

impl CostMap {
    fn map(kind: &str, pid: u32, owner: bool) -> Option<Self> {
        let name = HSTRING::from(cost_name(kind, pid));
        unsafe {
            let map = if owner {
                CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, std::mem::size_of::<Cost>() as u32, &name).ok()?
            } else {
                OpenFileMappingW(FILE_MAP_ALL_ACCESS.0, false, &name).ok()?
            };
            let view = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            if view.Value.is_null() {
                let _ = CloseHandle(map);
                return None;
            }
            let view = view.Value as *const Cost;
            Some(Self { map, view, owner, menu: (*view).menu.load(Relaxed) })
        }
    }

    /// O motor do jogo `pid` cria a memória e passa a escrever nela.
    pub fn create(kind: &str, pid: u32) -> Option<Self> {
        Self::map(kind, pid, true)
    }

    fn c(&self) -> &Cost {
        unsafe { &*self.view }
    }

    /// Chamado de tempos em tempos pelo motor: publica o custo e diz se o app pediu o menu desde a última vez.
    pub fn tick(&mut self, on: bool, gpu_ms: f64, frame_ms: f64, profile: &Path) -> bool {
        let name = profile.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        self.c().publish(now(), on, gpu_ms, frame_ms, &name);
        let asked = self.c().menu.load(Relaxed);
        std::mem::replace(&mut self.menu, asked) != asked
    }
}

impl Drop for CostMap {
    fn drop(&mut self) {
        if self.owner {
            self.c().beat.store(0, Relaxed);
        }
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view as *mut _ });
            let _ = CloseHandle(self.map);
        }
    }
}

fn now() -> u64 {
    unsafe { GetTickCount64() }
}

/// (perfil, ms de GPU, ms do quadro) do efeito `kind` no jogo `pid`, com ele na tela.
pub fn read(kind: &str, pid: u32) -> Option<(String, f64, f64)> {
    CostMap::map(kind, pid, false)?.c().read(now())
}

/// Pede ao efeito `kind` do jogo `pid` que abra ou feche o menu dele. Falso sem ninguém escrevendo.
pub fn ask_menu(kind: &str, pid: u32) -> bool {
    let Some(m) = CostMap::map(kind, pid, false) else { return false };
    let alive = now().saturating_sub(m.c().beat.load(Relaxed)) <= prisma_shader::link::COST_STALE_MS;
    if alive {
        m.c().menu.fetch_add(1, Relaxed);
    }
    alive
}
