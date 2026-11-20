//! HDR do jogo: uma escolha por programa, com um só HDR ligado de cada vez. Auto HDR é uma chave no
//! registro do usuário, RTX HDR fica no perfil do driver NVIDIA e o HDR do Prisma vai pelo `--hdr`.

use super::reg::{self, Hive};
use crate::sys::drs;

const ROOT: &str = "Software\\Microsoft\\Direct3D";
const PREFIX: &str = "Prisma ";

/// Números gravados no config. 0, 1 e 2 são os do formato antigo (padrão, forçar, desligar).
pub const DEFAULT: u8 = 0;
pub const AUTO: u8 = 1;
pub const OFF: u8 = 2;
pub const AUTO10: u8 = 3;
pub const RTX: u8 = 4;
pub const PRISMA: u8 = 5;
/// Ordem na tela, com o nome longo (aba HDR) e o curto (Medidor).
pub const MODES: [(u8, &str, &str); 6] = [
    (DEFAULT, "Padrão do Windows", "Padrão"),
    (OFF, "Desligado", "Desligado"),
    (AUTO, "Auto HDR", "Auto HDR"),
    (AUTO10, "Auto HDR 10 bits", "Auto 10 bits"),
    (RTX, "RTX HDR", "RTX HDR"),
    (PRISMA, "HDR do Prisma", "Prisma"),
];

const RTX_ON: u32 = 0x00DD_48FB;
/// Liga pelo perfil, sem o app da NVIDIA; 2 é a suavização de faixas máxima.
const RTX_FLAGS: u32 = 0x0043_2F84;
/// Permite o RTX HDR nos jogos que a NVIDIA bloqueia.
const RTX_ALLOW: u32 = 0x1077_A11A;

fn text(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

/// Placa NVIDIA com driver: só então o RTX HDR aparece.
pub fn nvidia() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| drs::driver_version().is_some())
}

/// Grava a escolha. Vale na próxima vez que o jogo abrir.
pub fn apply(exe: &str, mode: u8) -> bool {
    const REG_SZ: u32 = 1;
    if nvidia() {
        let rtx = match mode {
            RTX => [(RTX_ON, Some(1)), (RTX_FLAGS, Some(2)), (RTX_ALLOW, Some(1))],
            DEFAULT => [(RTX_ON, None), (RTX_FLAGS, None), (RTX_ALLOW, None)],
            // O app da NVIDIA pode ligar o RTX HDR em tudo: o 0 no perfil do jogo desliga só neste.
            _ => [(RTX_ON, Some(0)), (RTX_FLAGS, None), (RTX_ALLOW, None)],
        };
        // O driver só aceita estas opções de quem roda como administrador. Fora do RTX HDR, gravar
        // o 0 é só proteção: sem ele a escolha vale do mesmo jeito.
        if !drs::apply(exe, &rtx) && mode == RTX {
            return false;
        }
    }
    let key = format!("{ROOT}\\{PREFIX}{exe}");
    let behavior = match mode {
        AUTO => "BufferUpgradeOverride=1",
        AUTO10 => "BufferUpgradeOverride=1;BufferUpgradeEnable10Bit=1",
        // Desligado, RTX HDR e Prisma: o Auto HDR fica de fora para não somar dois HDR.
        OFF | RTX | PRISMA => "DisableBufferUpgrade=1",
        _ => "",
    };
    if behavior.is_empty() {
        reg::delete_key(Hive::User, &key)
    } else {
        reg::write(Hive::User, &key, "Name", REG_SZ, &text(exe)) && reg::write(Hive::User, &key, "D3DBehaviors", REG_SZ, &text(behavior))
    }
}

/// Tira todas as escolhas feitas pelo Prisma na chave Direct3D.
pub fn clear_all() {
    for k in reg::subkeys(Hive::User, ROOT).iter().filter(|k| k.starts_with(PREFIX)) {
        reg::delete_key(Hive::User, &format!("{ROOT}\\{k}"));
    }
}

/// A chave geral do Auto HDR em Configurações do Windows.
pub fn global_on() -> bool {
    let key = "Software\\Microsoft\\DirectX\\UserGpuPreferences";
    reg::text(Hive::User, key, "DirectXUserGlobalSettings").is_some_and(|v| v.contains("AutoHDREnable=1"))
}

/// O que colar nas opções de inicialização da Steam: a Steam troca `%command%` pelo jogo e abre o
/// Prisma, que abre o jogo com o HDR. Nas outras escolhas o Prisma só abre o jogo.
pub fn launch_option() -> String {
    let me = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_default();
    format!("\"{me}\" --hdr %command%")
}

/// `prisma.exe --hdr jogo.exe [argumentos]`: abre o jogo com o HDR do Prisma se ele for a escolha,
/// espera o jogo fechar (a Steam conta o tempo por este processo) e passa os ajustes ao vivo.
pub fn run_args() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|a| a != "--hdr") {
        return false;
    }
    let Some(exe) = args.next().map(|a| a.to_string_lossy().to_string()) else { return true };
    let rest: Vec<String> = args.map(|a| a.to_string_lossy().to_string()).collect();
    run(&exe, &rest);
    true
}

/// Junta os argumentos de volta numa linha de comando.
// shortcut: aspas só em volta de quem tem espaço, sem escapar `\"`; trocar se um jogo precisar.
fn join(args: &[String]) -> String {
    args.iter().map(|a| if a.is_empty() || a.contains(' ') { format!("\"{a}\"") } else { a.clone() }).collect::<Vec<_>>().join(" ")
}

fn game_hdr(cfg: &crate::config::Config, name: &str) -> Option<crate::sys::inject::Hdr> {
    let g = cfg.hdr_games.iter().find(|g| g.exe.eq_ignore_ascii_case(name) && g.mode == PRISMA)?;
    Some(g.prisma.unwrap_or_else(|| crate::sys::inject::Hdr::from_profile(cfg.hdr_profiles.last())))
}

fn run(exe: &str, args: &[String]) {
    let name = super::procs::exe_name(exe);
    let cfg = crate::config::Config::load();
    let line = join(args);
    if let Some(mut hdr) = game_hdr(&cfg, &name) {
        let launched = crate::sys::inject::Limiter::launch(exe, &line, &hdr);
        if let Err(e) = &launched {
            crate::config::log(&format!("HDR do Prisma: {name} abre sem ele ({e})"));
        }
        if let Ok(l) = launched {
            let path = crate::config::config_path();
            let mut seen = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
            while l.exit_code().is_none() {
                std::thread::sleep(std::time::Duration::from_millis(250));
                let now = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                if now != seen {
                    seen = now;
                    // Config lido no meio da gravação: fica com o último que valeu.
                    if let Some(h) = game_hdr(&crate::config::Config::load(), &name).filter(|h| *h != hdr) {
                        hdr = h;
                        l.set_hdr(&hdr);
                    }
                }
            }
            return;
        }
    }
    let dir = std::path::Path::new(exe).parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let _ = std::process::Command::new(exe).args(args).current_dir(dir).status();
}

/// "Abrir com HDR" de um jogo fora da Steam: o Prisma abre outra cópia dele com `--hdr`, como usuário comum.
pub fn open(path: &str) -> bool {
    let Ok(me) = std::env::current_exe() else { return false };
    let me = me.display().to_string();
    let dir = std::path::Path::new(path).parent().map(|p| p.display().to_string()).unwrap_or_default();
    if crate::startup::is_elevated() {
        return super::quick::run_as_user(&me, &format!("--hdr \"{path}\""), &dir, Default::default());
    }
    std::process::Command::new(me).arg("--hdr").arg(path).current_dir(dir).spawn().is_ok()
}
