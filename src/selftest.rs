//! Teste das partes que exigem administrador. Só existe na versão de depuração.

use std::fmt::Write;

pub fn run(path: &str, _shared: std::sync::Arc<std::sync::Mutex<crate::sys::monitor::Snapshot>>) {
    use std::path::Path;
    let mut s = String::new();
    let pf = std::env::var("ProgramFiles").unwrap();
    let exe = format!("{pf}\\Prisma\\prisma.exe");
    let lnk = format!("{}\\Microsoft\\Windows\\Start Menu\\Programs\\Prisma.lnk", std::env::var("ProgramData").unwrap());
    let key = "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Prisma";
    let reg = |k: &str| std::process::Command::new("reg").args(["query", k]).output().map(|o| o.status.success()).unwrap_or(false);
    let _ = writeln!(s, "admin {} antes: exe {} atalho {} registro {}", crate::startup::is_elevated(), Path::new(&exe).exists(), Path::new(&lnk).exists(), reg(key));
    let _ = writeln!(s, "instalou: {}", crate::startup::install(false));
    let _ = writeln!(s, "depois: exe {} atalho {} registro {}", Path::new(&exe).exists(), Path::new(&lnk).exists(), reg(key));
    crate::startup::uninstall();
    std::thread::sleep(std::time::Duration::from_secs(6));
    let _ = writeln!(s, "removido: exe {} atalho {} registro {}", Path::new(&exe).exists(), Path::new(&lnk).exists(), reg(key));
    let _ = std::fs::write(path, s);
}

/// Inicialização, assinatura e serviços, sem administrador.
pub fn misc(path: &str) {
    use crate::sys::{autostart as a, fileinfo as f, services as sv};
    let mut s = String::new();
    let exe = std::env::current_exe().unwrap().display().to_string();
    let ok = a::add(&exe);
    let find = || a::list().into_iter().find(|i| i.path.eq_ignore_ascii_case(&exe));
    let it = find().expect("item");
    let _ = writeln!(s, "adicionou {ok}: {} ativo {} origem {}", it.name, it.enabled, it.source.label());
    let off = a::set_enabled(&it, false);
    let _ = writeln!(s, "desligou {off}: ativo {}", find().unwrap().enabled);
    let on = a::set_enabled(&it, true);
    let _ = writeln!(s, "ligou {on}: ativo {}", find().unwrap().enabled);
    let rm = a::remove(&it);
    let _ = writeln!(s, "removeu {rm}: existe {}", find().is_some());
    let (rk, name) = ("HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\StartupApproved\\Run", it.name.clone());
    let _ = std::process::Command::new("reg").args(["delete", rk, "/v", &name, "/f"]).output();
    for p in ["C:\\Program Files\\Zen Browser\\zen.exe", "C:\\Windows\\explorer.exe", "C:\\Program Files\\WizTree\\WizTree64.exe"] {
        let _ = writeln!(s, "{p}: {} | {:?}", f::check(p).label(), f::description(p));
    }
    let _ = writeln!(s, "{}: {:?}", exe, f::check(&exe).label());
    for p in ["C:\\Windows\\explorer.exe", "C:\\pagefile.sys", "N:\\$MFT", "N:\\Videos\\a.mkv", "C:\\Users", "C:\\Users\\teste\\a.iso", "K:\\"] {
        let _ = writeln!(s, "protegido {p}: {}", crate::sys::mft::protected(p));
    }
    let list = sv::list();
    let _ = writeln!(s, "servicos {}", list.len());
    let (d, b) = sv::details("Schedule");
    let _ = writeln!(s, "Schedule: {} | {b}", d.chars().take(120).collect::<String>());
    let _ = std::fs::write(path, s);
}

/// Programa falso completo: instala no registro do usuário, deixa restos e passa
/// pelo fluxo real de desinstalar e limpar. Não toca em nada que já existia.
pub fn fake_uninstall(path: &str) {
    use crate::sys::uninstall as u;
    let mut s = String::new();
    let local = std::env::var("LOCALAPPDATA").unwrap();
    let dir = format!("{local}\\PrismaFalsoTeste");
    let keep = format!("{local}\\PrismaFalsoOutro");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(&keep).unwrap();
    std::fs::write(format!("{dir}\\dados.txt"), "resto").unwrap();
    std::fs::write(format!("{keep}\\dados.txt"), "nao apagar").unwrap();
    let key = "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\PrismaFalsoTeste";
    let reg = |a: &[&str]| std::process::Command::new("reg").args(a).output().unwrap().status.success();
    reg(&["add", key, "/v", "DisplayName", "/d", "Prisma Falso Teste 1.2 (x64)", "/f"]);
    reg(&["add", key, "/v", "Publisher", "/d", "Empresa Falsa Inc.", "/f"]);
    reg(&["add", key, "/v", "UninstallString", "/d", "cmd.exe /c echo fim", "/f"]);
    reg(&["add", "HKCU\\Software\\PrismaFalsoTeste", "/v", "x", "/d", "1", "/f"]);
    reg(&["add", "HKCU\\Software\\PrismaFalsoOutro", "/v", "x", "/d", "1", "/f"]);
    let all = u::list();
    let Some(p) = all.iter().find(|p| p.name.starts_with("Prisma Falso")).cloned() else {
        let _ = std::fs::write(path, "programa falso nao apareceu");
        return;
    };
    let _ = writeln!(s, "listou: {} de {} programas, comando: {}", p.name, all.len(), u::command(&p, true));
    let job: u::Shared = Default::default();
    u::start(vec![p], true, false, job.clone());
    for _ in 0..600 {
        if !job.lock().unwrap().running {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let j = job.lock().unwrap().clone();
    let _ = writeln!(s, "rodou: {:?}", j.done);
    for l in &j.leftovers {
        let _ = writeln!(s, "resto marcado={} {:?} {}", l.on, l.kind, l.label);
    }
    let back = std::path::PathBuf::from(std::env::var("TEMP").unwrap()).join("prisma_backup_teste");
    for l in j.leftovers.iter().filter(|l| l.on) {
        let _ = writeln!(s, "removeu {}: {}", l.label, u::remove(l, &back));
    }
    let _ = writeln!(s, "pasta do programa existe: {}", std::path::Path::new(&dir).exists());
    let _ = writeln!(s, "pasta parecida existe: {}", std::path::Path::new(&keep).exists());
    let q = |k: &str| reg(&["query", k]);
    let _ = writeln!(s, "chave do programa existe: {}", q("HKCU\\Software\\PrismaFalsoTeste"));
    let _ = writeln!(s, "chave parecida existe: {}", q("HKCU\\Software\\PrismaFalsoOutro"));
    let _ = writeln!(s, "chave de desinstalar existe: {}", q(key));
    let _ = writeln!(s, "backups: {:?}", std::fs::read_dir(&back).map(|r| r.count()));
    let _ = std::fs::remove_dir_all(&keep);
    reg(&["delete", "HKCU\\Software\\PrismaFalsoOutro", "/f"]);
    reg(&["delete", key, "/f"]);
    let _ = std::fs::remove_dir_all(&back);
    let _ = std::fs::write(path, s);
}

/// Motor de reversão com chaves falsas: nenhum ajuste real do Windows é aplicado.
pub fn tweaks(path: &str) {
    use crate::sys::reg::{self, Hive};
    use crate::sys::tweaks::{self as tw, Act};
    let mut s = String::new();
    let k: &'static str = "Software\\PrismaTesteReversao";
    let _ = std::process::Command::new("reg").args(["delete", "HKCU\\Software\\PrismaTesteReversao", "/f"]).output();
    reg::write_dword(Hive::User, k, "Existia", 7);
    reg::write(Hive::User, k, "Texto", 1, &[65, 0, 66, 0, 0, 0]);
    reg::write_dword(Hive::User, k, "Mexido", 1);
    const ACTS: &[Act] = &[
        Act::Dw(Hive::User, "Software\\PrismaTesteReversao", "Existia", 1),
        Act::Dw(Hive::User, "Software\\PrismaTesteReversao", "NaoExistia", 1),
        Act::Dw(Hive::User, "Software\\PrismaTesteReversao", "Texto", 5),
        Act::Dw(Hive::User, "Software\\PrismaTesteReversao", "Mexido", 2),
    ];
    let mut j = Vec::new();
    let mut lines = Vec::new();
    let bad = tw::apply_acts("teste", ACTS, &mut j, &mut |l| lines.push(l));
    let _ = writeln!(s, "aplicou: falhas {bad}, entradas {}", j.len());
    let _ = writeln!(s, "depois: existia {:?} naoexistia {:?}", reg::dword(Hive::User, k, "Existia"), reg::dword(Hive::User, k, "NaoExistia"));
    // Outro programa muda um valor depois do Prisma.
    reg::write_dword(Hive::User, k, "Mexido", 99);
    let r = tw::undo("teste", &mut j);
    for l in lines.iter().chain(&r.lines) {
        let _ = writeln!(s, "{l}");
    }
    let _ = writeln!(s, "desfez: {} restaurados, intocados {:?}, diario {}", r.restored, r.skipped, j.len());
    let _ = writeln!(s, "existia volta a 7: {:?}", reg::dword(Hive::User, k, "Existia"));
    let _ = writeln!(s, "naoexistia sumiu: {:?}", reg::read(Hive::User, k, "NaoExistia"));
    let _ = writeln!(s, "texto volta: {:?}", reg::text(Hive::User, k, "Texto"));
    let _ = writeln!(s, "mexido preservado em 99: {:?}", reg::dword(Hive::User, k, "Mexido"));
    let _ = std::process::Command::new("reg").args(["delete", "HKCU\\Software\\PrismaTesteReversao", "/f"]).output();
    for p in ["Microsoft.WindowsStore", "Microsoft.XboxIdentityProvider", "Microsoft.Paint", "Microsoft.BingNews"] {
        let _ = writeln!(s, "pode remover {p}: {}", tw::may_remove(p));
    }
    for t in tw::CATALOG {
        for a in t.acts {
            if let Act::App(n) = a {
                assert!(tw::may_remove(n), "{n}");
            }
        }
    }
    let empty: Vec<tw::Entry> = Vec::new();
    let apps = tw::installed_apps();
    let _ = writeln!(s, "apps instalados {}", apps.len());
    for t in tw::CATALOG {
        let _ = writeln!(s, "{}: {:?}", t.id, tw::status(t, &empty, &apps));
    }
    let _ = std::fs::write(path, s);
}

/// Auditoria real, só leitura.
pub fn audit(path: &str) {
    use crate::sys::{audit as au, disks, events};
    let mut s = String::new();
    let list = disks::list();
    let map: disks::HealthMap = Default::default();
    disks::spawn_health(&list, map.clone());
    for _ in 0..80 {
        if map.lock().unwrap().len() >= list.len() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    let info: Vec<(u32, String, Vec<char>)> = list.iter().map(|d| (d.number, d.name.clone(), d.volumes.iter().map(|v| v.letter).collect())).collect();
    let h: Vec<(u32, disks::Health)> = map.lock().unwrap().iter().map(|(k, v)| (*k, v.clone())).collect();
    for f in au::run(&info, &h) {
        let _ = writeln!(s, "{:?} | {} | {}", f.level, f.title, f.detail);
    }
    let _ = writeln!(s, "blocos ruins: {:?}", events::bad_blocks(30));
    let _ = writeln!(s, "falhas: {}", events::crashes(90).len());
    for e in events::crashes(90).iter().take(4) {
        let _ = writeln!(s, "  {} {} {}", e.time, e.id, e.provider);
    }
    let _ = writeln!(s, "memoria: {:?}", events::memory_result().map(|e| e.id));
    let _ = std::fs::write(path, s);
}

/// Saída real de ferramentas só de leitura, para conferir acentos, e o veredito do SFC.
pub fn toolrun(path: &str) {
    use crate::sys::tools::{self as t, cmd, neutral, Enc};
    let mut s = String::new();
    let run: t::Shared = Default::default();
    let steps = vec![
        cmd("sfc", "sfc", &["/?"], Enc::Utf16),
        cmd("powercfg", "powercfg", &["/a"], Enc::Oem),
        cmd("pnputil", "pnputil", &["/enum-devices", "/problem"], Enc::Ansi),
        cmd("chkdsk", "chkdsk", &["/?"], Enc::Ansi),
        cmd("netsh", "netsh", &["interface", "ipv4", "show", "dnsservers"], Enc::Utf8),
        cmd("ipconfig", "ipconfig", &["/?"], Enc::Oem),
    ];
    t::start(steps, neutral("fim"), run.clone());
    for _ in 0..200 {
        if !run.lock().unwrap().running {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let r = run.lock().unwrap().clone();
    for l in r.lines.iter().filter(|l| l.starts_with('▸') || !l.is_ascii()).take(44) {
        let _ = writeln!(s, "{}", l.chars().take(100).collect::<String>());
    }
    let _ = writeln!(s, "total de linhas {} veredito {:?}", r.lines.len(), r.verdict);
    let log = "2026-11-04 10:00:00, Info CSI [SR] Repairing 1 components\n2026-11-04 12:00:00, Info CSI [SR] Verify complete\n";
    let _ = writeln!(s, "antes ignorado: {:?}", t::sfc_verdict("2026-11-04 11:00:00", log));
    let _ = writeln!(s, "reparou: {:?}", t::sfc_verdict("2026-11-04 09:00:00", log));
    let bad = "2026-11-04 12:00:00, Info CSI [SR] Cannot repair member file [l:9]x\n";
    let _ = writeln!(s, "falhou: {:?}", t::sfc_verdict("2026-11-04 09:00:00", bad));
    let _ = writeln!(s, "sem log: {:?}", t::sfc_verdict("2026-11-04 09:00:00", ""));
    let _ = std::fs::write(path, s);
}

/// Passo a passo real sobre uma chave de teste, para conferir o painel de log sem tocar no Windows.
pub fn tweak_log() -> Vec<String> {
    use crate::sys::tweaks::{self as tw, Act};
    use crate::sys::reg::{self, Hive};
    const ACTS: &[Act] = &[
        Act::Dw(Hive::User, "Software\\PrismaTesteReversao\\Politicas\\Microsoft\\Windows\\Otimizacao de Entrega", "ValorDeTesteComNomeBemComprido", 1),
        Act::Dw(Hive::User, "Software\\PrismaTesteReversao", "Existia", 1),
    ];
    reg::write_dword(Hive::User, "Software\\PrismaTesteReversao", "Existia", 7);
    let mut lines = vec!["▸ 12:00:00  Ajuste de demonstração".to_string()];
    let mut j = Vec::new();
    tw::apply_acts("teste", ACTS, &mut j, &mut |l| lines.push(l));
    lines.push("▸ 12:00:01  Desfazendo".to_string());
    lines.extend(tw::undo("teste", &mut j).lines);
    let _ = std::process::Command::new("reg").args(["delete", "HKCU\\Software\\PrismaTesteReversao", "/f"]).output();
    lines
}
