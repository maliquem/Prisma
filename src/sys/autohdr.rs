//! HDR do jogo: uma escolha por programa, com um só HDR ligado de cada vez. Auto HDR é uma chave no
//! registro do usuário, RTX HDR fica no perfil do driver NVIDIA e o HDR do Prisma vai pelo `--hdr`.

use super::reg::{self, Hive};
use crate::config::HdrGame;
use crate::pages::hdr::{knob, KNOBS};
use crate::sys::drs;
use crate::sys::inject::{Hdr, HdrLive};

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

/// Se o jogo tem HDR próprio (`HdrGame::native`). Visto: o jogo saiu em HDR com o HDR do Prisma. Não visto:
/// só saiu em SDR até agora e vira visto se sair em HDR. Sim e não: marcados à mão na aba, valem sobre o visto.
pub const NATIVE_UNKNOWN: u8 = 0;
pub const NATIVE_NOT_SEEN: u8 = 1;
pub const NATIVE_SEEN: u8 = 2;
pub const NATIVE_YES: u8 = 3;
pub const NATIVE_NO: u8 = 4;
/// Quadros em SDR até o jogo contar como não visto: uns 10 s a 60 fps, passando das telas de abertura.
const NOT_SEEN_FRAMES: u64 = 600;

/// Medidas do Analisar que o Prisma liga sozinho num buffer de 10 bits sem espaço de cor, para saber se
/// o jogo manda HDR10 sem dizer (o ELDEN RING faz isso). Duas, alternadas: só um número novo zera a soma.
pub const GUESS_SCAN: u32 = 0x5051_0000;
const GUESS_SCANS: [u32; 2] = [GUESS_SCAN, GUESS_SCAN + 1];
/// Lido como SDR, o branco do jogo chega a ~100 nits. O HDR10 lido assim fica abaixo de 65 até o sinal
/// de ~2500 nits: o ELDEN RING dá 40. Abaixo de 10 é tela escura (carregando), que não diz nada.
/// shortcut: jogo SDR que passa 600 quadros com imagem e sem nada acima de 82% do sinal passa por HDR10;
/// a aba marca "não" à mão.
const GUESS_FRAMES: u32 = 600;
const GUESS_PEAK: std::ops::Range<f32> = 10.0..65.0;

/// Se 600 quadros que o Analisar leu como SDR parecem HDR10; nada enquanto mede.
pub fn guess_pq(frames: u32, peak: f32) -> Option<bool> {
    (frames >= GUESS_FRAMES).then(|| GUESS_PEAK.contains(&peak))
}

/// Uma passada num jogo aberto com o HDR do Prisma: o buffer de 10 bits sem espaço de cor do jogo com HDR
/// próprio é lido como HDR10. Sem saber ainda, o Analisar mede em ciclos de 600 quadros até ver HDR10: o
/// ELDEN RING sai SDR no menu e só manda HDR10 em jogo. O estado novo do HDR próprio, se mudou.
/// shortcut: num jogo SDR de 10 bits a medida não para; custa ~0 de GPU (medido no DQ7R).
pub fn watch(live: &HdrLive, native: u8) -> Option<u8> {
    live.set_pq(has_native(native));
    let (mode, frames) = live.native();
    let mut seen = detect(native, mode, frames);
    if live.untold() && native != NATIVE_NO && !has_native(native) {
        match live.scanning() {
            0 => live.scan_set(GUESS_SCAN),
            g if GUESS_SCANS.contains(&g) => {
                if let Some(pq) = live.scan(g).and_then(|s| guess_pq(s.frames, s.peak)) {
                    // O outro número recomeça a medida do zero; achado o HDR10, para.
                    live.scan_set(if pq { 0 } else { g ^ 1 });
                    if pq {
                        seen = Some(NATIVE_SEEN);
                    }
                }
            }
            _ => {}
        }
    }
    seen
}

pub fn has_native(n: u8) -> bool {
    n == NATIVE_SEEN || n == NATIVE_YES
}

pub fn lacks_native(n: u8) -> bool {
    n == NATIVE_NOT_SEEN || n == NATIVE_NO
}

/// O estado novo pelo que o gancho viu: saída do último quadro (`mode`, 0 SDR) e quadros desenhados.
pub fn detect(old: u8, mode: u32, frames: u64) -> Option<u8> {
    match old {
        NATIVE_SEEN | NATIVE_YES | NATIVE_NO => None,
        _ if mode != 0 => Some(NATIVE_SEEN),
        NATIVE_UNKNOWN if frames >= NOT_SEEN_FRAMES => Some(NATIVE_NOT_SEEN),
        _ => None,
    }
}

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

/// A escolha do jogo `exe` (caminho ou nome) é o HDR do Prisma.
pub fn uses_prisma(cfg: &crate::config::Config, exe: &str) -> bool {
    game_hdr(cfg, &super::procs::exe_name(exe)).is_some()
}

/// A calibração da tela com este nome GDI: a mesma que a aba HDR mostra para ela.
pub fn calibrated(cfg: &crate::config::Config, gdi: &str) -> Hdr {
    let screen = super::hdr::screen_name(gdi);
    Hdr::from_profile(cfg.hdr_profiles.iter().rfind(|p| screen.as_deref() == Some(p.screen.as_str())))
}

/// Nome GDI da tela principal, onde o jogo abre.
pub fn primary_gdi() -> String {
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
    super::hdr::gdi_name(unsafe { MonitorFromPoint(Default::default(), MONITOR_DEFAULTTOPRIMARY) })
}

/// O que o jogo em HDR entrega, se o Analisar mediu com ele saindo em HDR.
fn source(h: &mut Hdr, g: &HdrGame) {
    if let Some(s) = g.scan.filter(|s| s.mode != 0) {
        (h.src_peak, h.src_black, h.src_paper) = (s.peak, s.black, s.ui);
    }
}

/// Base do Automático: a calibração `cal` da tela. Jogo com HDR próprio fica sem realce dos claros (a curva
/// só leva o que ele entrega até a tela); no SDR, cena com muito branco de papel ganha menos realce.
pub fn auto_base(cal: Hdr, g: &HdrGame) -> Hdr {
    let mut h = cal;
    if has_native(g.native) || g.scan.is_some_and(|s| s.mode != 0) {
        h.boost = 0.0;
    } else if let Some(s) = g.scan {
        // Inteiro com até 2% da cena em branco de papel, 40% com 12% ou mais.
        h.boost = cal.boost * (1.0 - 0.6 * ((s.white - 0.02) / 0.10).clamp(0.0, 1.0));
    }
    source(&mut h, g);
    h
}

/// `base` com o ajuste fino por cima, dentro das faixas dos ajustes.
pub fn tuned(base: Hdr, tune: &[f32; KNOBS.len()]) -> Hdr {
    let mut h = base;
    for (i, (_, lo, hi)) in KNOBS.iter().enumerate().filter(|(i, _)| tune[*i] != 0.0) {
        let v = knob(&mut h, i);
        *v = (*v + tune[i]).clamp(*lo, *hi);
    }
    h
}

/// O ajuste fino que leva `base` a `h`.
pub fn tune_of(base: Hdr, h: Hdr) -> [f32; KNOBS.len()] {
    let (mut a, mut b) = (h, base);
    std::array::from_fn(|i| *knob(&mut a, i) - *knob(&mut b, i))
}

/// O que vale no jogo: no Manual os ajustes salvos, no Automático a base com o ajuste fino.
pub fn resolve(cal: Hdr, g: &HdrGame) -> Hdr {
    match g.prisma {
        Some(mut h) => {
            source(&mut h, g);
            h
        }
        None => tuned(auto_base(cal, g), &g.tune),
    }
}

fn game_hdr(cfg: &crate::config::Config, name: &str) -> Option<Hdr> {
    let g = cfg.hdr_games.iter().find(|g| g.exe.eq_ignore_ascii_case(name) && g.mode == PRISMA)?;
    Some(resolve(calibrated(cfg, &primary_gdi()), g))
}

/// Processos de anti-cheat que só rodam com um jogo protegido aberto.
const AC_RUNNING: [&str; 4] = ["start_protected_game", "easyanticheat", "beservice", "bedaisy"];

/// Se o HDR do Prisma pode entrar no jogo `path`. O anti-cheat achado na pasta barra, a não ser que a pessoa
/// tenha marcado o jogo para ignorar (offline, mods); com um anti-cheat rodando agora, nunca.
pub fn allowed(cfg: &crate::config::Config, path: &str) -> Result<(), String> {
    let name = super::procs::exe_name(path);
    let running = super::procs::running_exes();
    if running.iter().any(|n| AC_RUNNING.iter().any(|a| n.starts_with(a))) || name.to_ascii_lowercase().starts_with(AC_RUNNING[0]) {
        return Err("anti-cheat rodando".into());
    }
    let off = cfg.hdr_games.iter().any(|g| g.ac_off && g.exe.eq_ignore_ascii_case(&name));
    if !off && crate::meter::anti_cheat(path) {
        return Err("anti-cheat detectado".into());
    }
    Ok(())
}

/// Gancho que um carregador de mods abriu com o jogo (`inject::put_native`): ele nasce sem ajustes (pico zero).
/// Passa agora e segue passando até o jogo fechar. Jogo aberto pelo Prisma já tem os ajustes e fica como está.
// shortcut: o Prisma reaberto com o jogo aberto vê o pico já gravado e não passa os ajustes ao vivo até o jogo reabrir;
// guardar os pids servidos se alguém reclamar.
pub fn adopt(cfg: &crate::config::Config, live: &crate::sys::inject::HdrLive, pid: u32, name: &str) {
    if live.get().peak != 0.0 {
        return;
    }
    let Some(hdr) = game_hdr(cfg, name) else { return };
    // Já aqui, para o próximo segundo não adotar de novo.
    live.set(&hdr);
    let name = name.to_string();
    std::thread::spawn(move || {
        if let Some(l) = crate::sys::inject::Limiter::adopt(pid) {
            crate::config::log(&format!("HDR do Prisma: {name} com o gancho do carregador de mods"));
            serve(&l, &name, hdr);
        }
    });
}

/// Teste: espera até 10 min o jogo `name` ter janela e faz o que o app faz com ele na frente (`adopt`), gravando o
/// estado do gancho a cada segundo até ele fechar.
#[cfg(debug_assertions)]
pub fn test_adopt(out: &str, name: &str) {
    use super::procs;
    let cfg = crate::config::Config::load();
    let mut log = String::new();
    for g in cfg.hdr_games.iter().filter(|g| !g.mods.is_empty()) {
        log += &format!("gancho em {}: {}
", g.mods, crate::sys::inject::put_native(&g.mods));
    }
    let found = (0..600).find_map(|_| {
        let pid = procs::top_windows().into_iter().map(procs::window_pid).find(|p| procs::image_path(*p).is_some_and(|i| procs::exe_name(&i).eq_ignore_ascii_case(name)));
        if pid.is_none() {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        pid
    });
    let Some(pid) = found else {
        let _ = std::fs::write(out, "o jogo não apareceu
");
        return;
    };
    let mut i = 0;
    while procs::is_alive(pid) {
        let live = crate::sys::inject::HdrLive::connect(pid);
        if let Some(live) = &live {
            adopt(&cfg, live, pid, name);
        }
        let l = crate::sys::inject::Limiter::adopt(pid);
        log += &format!("{i} s pid {pid}: gancho {} cadeias, quadros, erro {:?} saída, interface {:?} ajustes {:?}
", live.is_some(), l.as_ref().and_then(|l| l.hdr_stats()), l.as_ref().and_then(|l| l.hdr_split()), live.as_ref().map(|l| l.get()));
        let _ = std::fs::write(out, &log);
        std::thread::sleep(std::time::Duration::from_secs(1));
        i += 1;
    }
}

/// Passa os ajustes ao vivo até o jogo fechar.
fn serve(l: &crate::sys::inject::Limiter, name: &str, mut hdr: Hdr) {
    let path = crate::config::config_path();
    let mut seen = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    // Com a aba fechada ninguém roda `watch`: o HDR10 sem espaço de cor sai do que o config guarda.
    let live = crate::sys::inject::HdrLive::connect(l.pid);
    let pq = |cfg: &crate::config::Config| cfg.hdr_games.iter().any(|g| g.exe.eq_ignore_ascii_case(name) && has_native(g.native));
    if let Some(live) = &live {
        live.set_pq(pq(&crate::config::Config::load()));
    }
    while l.exit_code().is_none() {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let now = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if now != seen {
            seen = now;
            // Config lido no meio da gravação: fica com o último que valeu.
            let cfg = crate::config::Config::load();
            if let Some(live) = &live {
                live.set_pq(pq(&cfg));
            }
            if let Some(h) = game_hdr(&cfg, name).filter(|h| *h != hdr) {
                hdr = h;
                l.set_hdr(&hdr);
            }
        }
    }
    // A interface não saiu separada nenhuma vez: ficou junto com a cena, como antes.
    if let Some((_, how, 0)) = l.hdr_split().filter(|_| hdr.ui >= 1.0) {
        let why = if how == crate::sys::inject::SPLIT_LATE { "passe de tela cheia depois da interface" } else { "nada desenhado no buffer depois da cena" };
        crate::config::log(&format!("HDR do Prisma: {name} sem brilho da interface separado ({why})"));
    }
}

fn run(exe: &str, args: &[String]) {
    let name = super::procs::exe_name(exe);
    let cfg = crate::config::Config::load();
    let line = join(args);
    if let Some(hdr) = game_hdr(&cfg, &name) {
        match allowed(&cfg, exe).and_then(|_| crate::sys::inject::Limiter::launch(exe, &line, &hdr)) {
            Ok(l) => return serve(&l, &name, hdr),
            Err(e) => crate::config::log(&format!("HDR do Prisma: {name} abre sem ele ({e})")),
        }
    }
    let dir = std::path::Path::new(exe).parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let _ = std::process::Command::new(exe).args(args).current_dir(dir).status();
}

/// "Abrir com HDR" de um jogo fora da Steam, ou o jogo que o Prisma Launcher pediu: o Prisma abre outra
/// cópia dele com `--hdr`, como usuário comum.
pub fn open(path: &str, args: &[String]) -> bool {
    let Ok(me) = std::env::current_exe() else { return false };
    let me = me.display().to_string();
    let dir = std::path::Path::new(path).parent().map(|p| p.display().to_string()).unwrap_or_default();
    if crate::startup::is_elevated() {
        return super::quick::run_as_user(&me, &format!("--hdr \"{path}\" {}", join(args)), &dir, Default::default());
    }
    std::process::Command::new(me).arg("--hdr").arg(path).args(args).current_dir(dir).spawn().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys::inject::Scan;

    #[test]
    fn native_detection() {
        assert_eq!(detect(NATIVE_UNKNOWN, 0, 10), None);
        assert_eq!(detect(NATIVE_UNKNOWN, 0, NOT_SEEN_FRAMES), Some(NATIVE_NOT_SEEN));
        // Não visto vira visto quando sai em HDR; marcado à mão não muda.
        assert_eq!(detect(NATIVE_NOT_SEEN, 1, 5000), Some(NATIVE_SEEN));
        assert_eq!(detect(NATIVE_NO, 2, 5000), None);
        assert_eq!(detect(NATIVE_SEEN, 0, 5000), None);
        // 10 bits sem espaço de cor: o ELDEN RING lido como SDR (pico 40) é HDR10; o DQ7R (103) é SDR.
        assert_eq!(guess_pq(100, 40.0), None);
        assert_eq!(guess_pq(GUESS_FRAMES, 40.0), Some(true));
        assert_eq!(guess_pq(GUESS_FRAMES, 103.0), Some(false));
        assert_eq!(guess_pq(GUESS_FRAMES, 2.0), Some(false));
    }

    #[test]
    fn automatic_with_tune_and_scan() {
        let cal = Hdr::from_profile(None);
        let mut g = HdrGame::default();
        assert_eq!(resolve(cal, &g), cal);
        // Ajuste fino como diferença, dentro da faixa do ajuste.
        let moved = Hdr { peak: cal.peak + 100.0, sat: 1.6, ..cal };
        g.tune = tune_of(cal, moved);
        assert_eq!(resolve(cal, &g).peak, cal.peak + 100.0);
        assert_eq!(resolve(cal, &g).sat, 1.5);
        // SDR com muito branco: menos realce. HDR medido: sem realce e com o que o jogo entrega.
        g.scan = Some(Scan { frames: 100, white: 0.5, ..Default::default() });
        assert!((auto_base(cal, &g).boost - 0.4).abs() < 1e-6);
        g.scan = Some(Scan { frames: 100, peak: 900.0, black: 0.01, ui: 220.0, mode: 1, ..Default::default() });
        let h = resolve(cal, &g);
        assert_eq!((h.boost, h.src_peak, h.src_paper), (0.0, 900.0, 220.0));
        // Manual vale como foi salvo, ainda com o que o jogo entrega.
        g.prisma = Some(Hdr { peak: 700.0, ..cal });
        assert_eq!((resolve(cal, &g).peak, resolve(cal, &g).src_peak), (700.0, 900.0));
    }
}
