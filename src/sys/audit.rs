//! Auditoria: lê o estado do PC e não altera nada. É a primeira tela antes de qualquer ajuste.

use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use super::disks::{Health, Level};
use super::reg::{self, Hive};
use super::{events, procs};

#[link(name = "ntdll")]
extern "system" {
    fn NtQuerySystemInformation(class: u32, info: *mut c_void, len: u32, ret: *mut u32) -> i32;
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fix {
    None,
    CoreIsolation,
    Disks,
    Tools,
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub level: Level,
    pub title: String,
    pub detail: String,
    pub fix: Fix,
}

fn f(level: Level, title: impl Into<String>, detail: impl Into<String>, fix: Fix) -> Finding {
    Finding { level, title: title.into(), detail: detail.into(), fix }
}

/// True quando a Integridade da Memória está ativa agora.
pub fn hvci_running() -> bool {
    #[repr(C)]
    struct Info {
        len: u32,
        options: u32,
    }
    let mut i = Info { len: 8, options: 0 };
    let mut ret = 0;
    let st = unsafe { NtQuerySystemInformation(103, &mut i as *mut _ as *mut _, 8, &mut ret) };
    st == 0 && i.options & 0x400 != 0
}

fn hags_on() -> Option<bool> {
    reg::dword(Hive::Machine, "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers", "HwSchMode").map(|v| v == 2)
}

fn pagefile_on() -> bool {
    use windows::Win32::System::SystemInformation::*;
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe {
        let _ = GlobalMemoryStatusEx(&mut m);
    }
    m.ullTotalPageFile > m.ullTotalPhys + (256 << 20)
}

fn image_path_ok() -> bool {
    reg::text(Hive::Machine, "SYSTEM\\CurrentControlSet\\Services\\TrustedInstaller", "ImagePath")
        .is_some_and(|p| p.to_ascii_lowercase().contains("trustedinstaller.exe"))
}

fn reboot_pending() -> bool {
    reg::exists(Hive::Machine, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Component Based Servicing\\RebootPending")
        || reg::exists(Hive::Machine, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\WindowsUpdate\\Auto Update\\RebootRequired")
}

pub fn build() -> (u32, String) {
    let k = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    let b = reg::text(Hive::Machine, k, "CurrentBuildNumber").and_then(|s| s.parse().ok()).unwrap_or(0);
    let name = reg::text(Hive::Machine, k, "DisplayVersion").unwrap_or_default();
    (b, name)
}

/// Mais nova que o último teste do catálogo: escreve, mas com rótulo.
pub const TESTED_UP_TO: u32 = 26100;

pub fn run(disks: &[(u32, String, Vec<char>)], health: &[(u32, Health)]) -> Vec<Finding> {
    let mut out = Vec::new();

    for (n, name, letters) in disks {
        let Some((_, h)) = health.iter().find(|v| v.0 == *n) else { continue };
        if matches!(h.level, Level::Warn | Level::Risk) {
            let l: Vec<String> = letters.iter().map(|c| format!("{c}:")).collect();
            let lv = h.level;
            out.push(f(lv, format!("Disco {} ({name}) pede cuidado", l.join(" ")), format!("{}. Copie o que importa antes de qualquer reparo.", h.summary), Fix::Disks));
        }
    }

    if hvci_running() {
        out.push(f(Level::Good, "Integridade da Memória ligada", "Proteção ativa. Em jogos limitados pela CPU custa cerca de 6%; em 4K, cerca de 2%. A escolha é sua, na tela oficial.", Fix::CoreIsolation));
    } else {
        out.push(f(Level::Warn, "Integridade da Memória desligada", "O PC fica com menos proteção contra drivers maliciosos. A Microsoft pode religar em atualizações.", Fix::CoreIsolation));
    }

    match hags_on() {
        Some(true) => out.push(f(Level::Good, "Agendamento de GPU ligado", "Necessário para a geração de quadros do DLSS.", Fix::None)),
        Some(false) => out.push(f(Level::Warn, "Agendamento de GPU desligado", "O DLSS Frame Generation exige ele ligado, em Configurações, Sistema, Tela, Gráficos.", Fix::None)),
        None => {}
    }

    if !pagefile_on() {
        out.push(f(Level::Risk, "Arquivo de paginação desligado", "Jogos e compilações podem falhar sem aviso. Ligue em Sistema, Avançado, Desempenho.", Fix::None));
    }

    let dep = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Appx\\AppxAllUserStore\\Deprovisioned";
    let count = reg::subkey_count(Hive::Machine, dep);
    if count > 0 {
        out.push(f(Level::Warn, format!("{count} apps do Windows foram retirados antes"), "Alguma ferramenta ou atualização marcou esses apps. Sem Game Pass isso não costuma atrapalhar.", Fix::None));
    }

    let tel = ["SOFTWARE\\Policies\\Microsoft\\Windows\\DataCollection", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\DataCollection"]
        .iter()
        .find_map(|k| reg::dword(Hive::Machine, k, "AllowTelemetry"));
    if tel == Some(0) {
        out.push(f(Level::Warn, "Telemetria em 0, valor que o Pro não suporta", "Na prática o Windows trata como Necessários. O perfil grava o valor correto.", Fix::None));
    }

    if !image_path_ok() {
        out.push(f(Level::Risk, "Serviço TrustedInstaller alterado", "Já houve caso em que isso quebrou o Windows Update. Veja antes de aplicar qualquer ajuste.", Fix::None));
    }
    if reboot_pending() {
        out.push(f(Level::Warn, "Reinício pendente", "Reinicie antes de aplicar ajustes ou reparar o Windows.", Fix::None));
    }

    let crashes = events::crashes(90);
    if !crashes.is_empty() {
        let whea = crashes.iter().filter(|e| e.provider.contains("WHEA")).count();
        let level = if whea > 0 { Level::Warn } else { Level::Good };
        out.push(f(level, format!("{} quedas ou erros graves em 90 dias", crashes.len()), format!("{whea} de hardware. Veja em Ferramentas, Histórico de falhas."), Fix::Tools));
    }

    if procs::parent_map().is_empty() {
        out.push(f(Level::Unknown, "Lista de processos indisponível", "", Fix::None));
    }
    if std::path::Path::new("C:\\Windows.old").is_dir() {
        out.push(f(Level::Good, "Pasta Windows.old presente", "Permite voltar de versão. Apagar libera espaço, mas encerra essa volta.", Fix::None));
    }
    let (b, name) = build();
    if b > TESTED_UP_TO {
        out.push(f(Level::Good, format!("Windows build {b} ({name})"), "Mais nova que os testes do catálogo. Os ajustes funcionam, com o rótulo Não testado nesta build.", Fix::None));
    }
    out.sort_by_key(|v| match v.level {
        Level::Risk => 0,
        Level::Warn => 1,
        Level::Unknown => 2,
        Level::Good => 3,
    });
    out
}

pub type Shared = Arc<Mutex<Option<Vec<Finding>>>>;
