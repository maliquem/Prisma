//! Fase 8: ponte opcional para o jogo ver um controle Xbox. Usa o VIIPER (Alia5, servidor GPL, que
//! instala o driver usbip-win2), programa de terceiros baixado da release oficial só depois do
//! consentimento na tela; nada dele vai dentro do Prisma. O controle virtual é o mesmo do co-op
//! (`stream::pads::Bridge`, cliente `viiper-client`, MIT). Sem a ponte instalada, nada muda.
//!
//! - Cada controle vira um Xbox, ou todos juntos viram um só (`merge`).
//! - A vibração que o jogo manda volta ao controle físico: no DualSense, pelo háptico (`rumble`).
//! - Para com anti-cheat em foco e com a parada de emergência.
//! - Remove tudo pelo próprio Prisma (`start_remove`). Pesquisa:
//!   `research_notes/Controles e macros no Prisma/fase8-viiper.md`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use super::pads::{self, Buttons, Input};
use crate::stream::pads::{Bridge, VirtualPad};
use crate::stream::PadState;

const VERSION: &str = "v0.9.0";
const ASSET: &str = "viiper-windows-amd64.exe";

// ---------- Instalação ----------

/// Onde a ponte está. `Reboot`: instalada, mas o servidor não respondeu (o driver pede reinício).
#[derive(Clone, PartialEq, Debug)]
pub enum Stage {
    Absent,
    Downloading,
    Installing,
    Reboot,
    Ready,
    Removing,
    Failed(String),
}

impl Stage {
    pub fn busy(&self) -> bool {
        matches!(self, Stage::Downloading | Stage::Installing | Stage::Removing)
    }
}

/// O sistema por trás da instalação; nos testes, um falso.
pub trait Host {
    fn viiper_installed(&self) -> bool;
    fn usbip_installed(&self) -> bool;
    /// O servidor responde em até `secs` segundos.
    fn server_up(&self, secs: u64) -> bool;
    fn download(&self) -> Result<PathBuf, String>;
    /// `viiper install --quiet`: copia, cria o início automático e instala o usbip-win2 se faltar.
    fn install(&self, exe: &Path) -> Result<(), String>;
    fn uninstall_viiper(&self) -> Result<(), String>;
    fn uninstall_usbip(&self) -> Result<(), String>;
    /// Apaga as pastas do VIIPER e o que foi baixado.
    fn clean(&self);
}

pub fn detect(h: &dyn Host) -> Stage {
    match (h.viiper_installed(), h.server_up(0)) {
        (true, true) => Stage::Ready,
        (true, false) => Stage::Reboot,
        _ => Stage::Absent,
    }
}

/// Instala. Devolve o estado final e se o usbip-win2 chegou com esta instalação (só então a
/// remoção tira ele).
pub fn install(h: &dyn Host, step: &mut dyn FnMut(Stage)) -> (Stage, bool) {
    let had = h.usbip_installed();
    step(Stage::Downloading);
    let exe = match h.download() {
        Ok(p) => p,
        Err(e) => return (Stage::Failed(e), false),
    };
    step(Stage::Installing);
    let r = h.install(&exe);
    let _ = std::fs::remove_file(&exe);
    let ours = !had && h.usbip_installed();
    if let Err(e) = r {
        return (Stage::Failed(e), ours);
    }
    if !h.viiper_installed() {
        return (Stage::Failed("o VIIPER não ficou instalado".into()), ours);
    }
    (if h.server_up(15) { Stage::Ready } else { Stage::Reboot }, ours)
}

/// Remove tudo: o VIIPER, o usbip-win2 se veio com ele, e as pastas. Diz o que sobrou.
pub fn remove(h: &dyn Host, usbip_ours: bool, step: &mut dyn FnMut(Stage)) -> Stage {
    step(Stage::Removing);
    let mut errs = vec![];
    if h.viiper_installed() {
        if let Err(e) = h.uninstall_viiper() {
            errs.push(e);
        }
    }
    if usbip_ours && h.usbip_installed() {
        if let Err(e) = h.uninstall_usbip() {
            errs.push(e);
        }
    }
    h.clean();
    let mut left = vec![];
    if h.viiper_installed() {
        left.push("VIIPER");
    }
    if usbip_ours && h.usbip_installed() {
        left.push("usbip-win2");
    }
    if left.is_empty() {
        Stage::Absent
    } else {
        errs.insert(0, format!("ficou instalado: {}", left.join(", ")));
        Stage::Failed(errs.join("; "))
    }
}

/// O `digest` que a API do GitHub dá ao arquivo `asset` da release, em hexa.
pub fn asset_digest(json: &str, asset: &str) -> Option<String> {
    let at = json.find(&format!("\"name\":\"{asset}\"")).or_else(|| json.find(&format!("\"name\": \"{asset}\"")))?;
    let rest = &json[at..];
    // O digest vem dentro do mesmo objeto, antes do próximo arquivo.
    let end = rest[1..].find("\"name\"").map_or(rest.len(), |e| e + 1);
    let obj = &rest[..end];
    let d = obj.find("sha256:")? + 7;
    let hex: String = obj[d..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
    (hex.len() == 64).then(|| hex.to_lowercase())
}

/// Separa um comando do Windows em programa e argumentos.
pub fn split_cmd(cmd: &str) -> (String, String) {
    let cmd = cmd.trim();
    if let Some(rest) = cmd.strip_prefix('"') {
        if let Some(e) = rest.find('"') {
            return (rest[..e].to_string(), rest[e + 1..].trim().to_string());
        }
    }
    let low = cmd.to_ascii_lowercase();
    match low.find(".exe") {
        Some(e) => (cmd[..e + 4].to_string(), cmd[e + 4..].trim().to_string()),
        None => (cmd.to_string(), String::new()),
    }
}

struct Win;

fn viiper_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default()).join("VIIPER")
}

fn usbip_program() -> Option<super::uninstall::Program> {
    super::uninstall::list().into_iter().find(|p| p.name.to_ascii_lowercase().starts_with("usbip"))
}

/// Roda e espera, pelo Shell: um programa que pede administrador mostra a confirmação do Windows.
fn shell_wait(file: &str, args: &str, admin: bool) -> Result<(), String> {
    use windows::core::*;
    use windows::Win32::Foundation::*;
    use windows::Win32::System::Threading::*;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let (f, a) = (HSTRING::from(file), HSTRING::from(args));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: if admin { w!("runas") } else { PCWSTR::null() },
        lpFile: PCWSTR(f.as_ptr()),
        lpParameters: PCWSTR(a.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).map_err(|_| "o Windows recusou ou a permissão foi negada".to_string())?;
        if info.hProcess.is_invalid() {
            return Ok(());
        }
        let _ = WaitForSingleObject(info.hProcess, 600_000);
        let mut code = 1;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        if code == 0 {
            Ok(())
        } else {
            Err(format!("saiu com o código {code}"))
        }
    }
}

impl Host for Win {
    fn viiper_installed(&self) -> bool {
        viiper_dir().join("viiper.exe").exists()
    }
    fn usbip_installed(&self) -> bool {
        usbip_program().is_some()
    }
    fn server_up(&self, secs: u64) -> bool {
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 3242));
        let t = std::time::Instant::now();
        loop {
            if std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).is_ok() {
                return true;
            }
            if t.elapsed() >= Duration::from_secs(secs) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    fn download(&self) -> Result<PathBuf, String> {
        use super::update;
        let api = update::download(&format!("https://api.github.com/repos/Alia5/VIIPER/releases/tags/{VERSION}")).map_err(|_| "a release oficial do VIIPER não respondeu".to_string())?;
        let want = asset_digest(&String::from_utf8_lossy(&api), ASSET).ok_or("a release do VIIPER não traz a soma do arquivo")?;
        let data = update::download(&format!("https://github.com/Alia5/VIIPER/releases/download/{VERSION}/{ASSET}")).map_err(|_| "o download do VIIPER falhou".to_string())?;
        if update::sha256(&data).as_deref() != Some(want.as_str()) {
            return Err("o arquivo baixado não confere com a release oficial".into());
        }
        let dir = std::env::temp_dir().join("prisma-viiper");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join(ASSET);
        std::fs::write(&p, data).map_err(|e| e.to_string())?;
        Ok(p)
    }
    fn install(&self, exe: &Path) -> Result<(), String> {
        shell_wait(&exe.to_string_lossy(), "install --quiet --no-update", false)
    }
    fn uninstall_viiper(&self) -> Result<(), String> {
        shell_wait(&viiper_dir().join("viiper.exe").to_string_lossy(), "uninstall --quiet", false)
    }
    fn uninstall_usbip(&self) -> Result<(), String> {
        let p = usbip_program().ok_or("usbip-win2 não achado")?;
        let (f, a) = split_cmd(&super::uninstall::command(&p, true));
        shell_wait(&f, &a, true)
    }
    fn clean(&self) {
        let _ = std::fs::remove_dir_all(viiper_dir());
        let _ = std::fs::remove_dir_all(PathBuf::from(std::env::var_os("APPDATA").unwrap_or_default()).join("VIIPER"));
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("prisma-viiper"));
    }
}

static STAGE: Mutex<Option<Stage>> = Mutex::new(None);
/// Resultado do usbip-win2 da última instalação para o app gravar: 0 nada, 1 não é nosso, 2 é.
static USBIP: AtomicU8 = AtomicU8::new(0);

fn set_stage(s: Stage) {
    *STAGE.lock().unwrap() = Some(s);
}

/// Estado agora (olha o sistema na primeira vez).
pub fn stage() -> Stage {
    if let Some(s) = STAGE.lock().unwrap().clone() {
        return s;
    }
    let s = detect(&Win);
    set_stage(s.clone());
    s
}

/// Olha o sistema de novo (depois de reiniciar o PC, por exemplo).
pub fn recheck() {
    if !stage().busy() {
        set_stage(detect(&Win));
    }
}

/// Começa a instalar, numa linha à parte. Só depois do consentimento na tela.
pub fn start_install() {
    if stage().busy() {
        return;
    }
    set_stage(Stage::Downloading);
    std::thread::spawn(|| {
        let (s, ours) = install(&Win, &mut set_stage);
        USBIP.store(if ours { 2 } else { 1 }, Ordering::Relaxed);
        set_stage(s);
    });
}

/// Remove tudo. `usbip_ours`: o usbip-win2 veio com a instalação do Prisma.
pub fn start_remove(usbip_ours: bool) {
    if stage().busy() {
        return;
    }
    ACTIVE.store(false, Ordering::Relaxed);
    set_stage(Stage::Removing);
    std::thread::spawn(move || {
        // Dá tempo de a linha da ponte soltar os controles virtuais.
        std::thread::sleep(Duration::from_millis(300));
        let s = remove(&Win, usbip_ours, &mut set_stage);
        set_stage(s);
    });
}

/// O usbip-win2 veio com a última instalação? Uma vez, para gravar na configuração.
pub fn take_usbip() -> Option<bool> {
    match USBIP.swap(0, Ordering::Relaxed) {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    }
}

// ---------- Controles ----------

/// Botões do Prisma e os do XInput.
const XBITS: [(Buttons, u16); 15] = [
    (pads::UP, 0x1),
    (pads::DOWN, 0x2),
    (pads::LEFT, 0x4),
    (pads::RIGHT, 0x8),
    (pads::START, 0x10),
    (pads::BACK, 0x20),
    (pads::LS, 0x40),
    (pads::RS, 0x80),
    (pads::LB, 0x100),
    (pads::RB, 0x200),
    (pads::GUIDE, 0x400),
    (pads::A, 0x1000),
    (pads::B, 0x2000),
    (pads::X, 0x4000),
    (pads::Y, 0x8000),
];

pub fn to_xbox(i: &Input) -> PadState {
    let s = |v: f32| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16;
    let t = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let buttons = XBITS.iter().filter(|(b, _)| i.buttons & b != 0).fold(0, |a, (_, x)| a | x);
    PadState { buttons, lt: t(i.lt), rt: t(i.rt), lx: s(i.lx), ly: s(i.ly), rx: s(i.rx), ry: s(i.ry) }
}

/// Vários controles em um: botões somados, gatilho mais apertado e, em cada stick, o mais inclinado.
pub fn merge(all: &[Input]) -> Input {
    let mut m = Input::default();
    let (mut l, mut r) = (0.0f32, 0.0f32);
    for i in all {
        m.buttons |= i.buttons;
        m.lt = m.lt.max(i.lt);
        m.rt = m.rt.max(i.rt);
        let (a, b) = (i.lx * i.lx + i.ly * i.ly, i.rx * i.rx + i.ry * i.ry);
        if a > l {
            (l, m.lx, m.ly) = (a, i.lx, i.ly);
        }
        if b > r {
            (r, m.rx, m.ry) = (b, i.rx, i.ry);
        }
    }
    m
}

/// Os controles virtuais ligados agora: (grupo, controles físicos dele, virtual).
pub struct Runner {
    bridge: Bridge,
    live: Vec<(String, Vec<String>, Box<dyn VirtualPad>)>,
}

impl Runner {
    pub fn new(bridge: Bridge) -> Runner {
        Runner { bridge, live: vec![] }
    }

    /// Um passo: liga e solta virtuais conforme os controles, manda o estado e devolve a vibração
    /// pedida pelo jogo para cada controle físico (grave, agudo).
    pub fn step(&mut self, pads: &[(String, Input)], merge_all: bool) -> Result<Vec<(String, (u8, u8))>, String> {
        let groups: Vec<(String, Vec<&(String, Input)>)> = if merge_all {
            if pads.is_empty() { vec![] } else { vec![("todos".into(), pads.iter().collect())] }
        } else {
            pads.iter().map(|p| (p.0.clone(), vec![p])).collect()
        };
        self.live.retain(|l| groups.iter().any(|g| g.0 == l.0));
        let mut out = vec![];
        for (id, members) in &groups {
            let keys: Vec<String> = members.iter().map(|m| m.0.clone()).collect();
            let i = match self.live.iter().position(|l| &l.0 == id) {
                Some(i) => i,
                None => {
                    self.live.push((id.clone(), keys.clone(), self.bridge.plug()?));
                    self.live.len() - 1
                }
            };
            let l = &mut self.live[i];
            l.1 = keys;
            let inputs: Vec<Input> = members.iter().map(|m| m.1).collect();
            if !l.2.update(&to_xbox(&merge(&inputs))) {
                let id = id.clone();
                self.live.retain(|l| l.0 != id);
                return Err("o controle virtual caiu".into());
            }
            let rumble = l.2.rumble();
            out.extend(l.1.iter().map(|k| (k.clone(), rumble)));
        }
        Ok(out)
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }
}

static ON: AtomicBool = AtomicBool::new(false);
static MERGE: AtomicBool = AtomicBool::new(false);
/// Virtuais ligados agora (a ponte age).
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// Vagas do XInput que são dos virtuais: o Prisma não lê elas como controle.
static OWN: AtomicU8 = AtomicU8::new(0);
static RUMBLE: Mutex<Vec<(String, (u8, u8))>> = Mutex::new(Vec::new());
static STARTED: AtomicBool = AtomicBool::new(false);

/// Liga ou desliga a ponte e a junção dos controles (`apply_pads`).
pub fn configure(on: bool, merge_all: bool) {
    ON.store(on, Ordering::Relaxed);
    MERGE.store(merge_all, Ordering::Relaxed);
    if on && !STARTED.swap(true, Ordering::Relaxed) {
        let _ = std::thread::Builder::new().name("ponte".into()).spawn(run);
    }
}

pub fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

/// A vaga `i` do XInput é de um controle virtual da ponte.
pub fn own_slot(i: usize) -> bool {
    OWN.load(Ordering::Relaxed) & (1 << i) != 0
}

/// Vibração que o jogo pede ao controle `key` pela ponte (grave, agudo).
pub fn rumble(key: &str) -> (u8, u8) {
    if !active() {
        return (0, 0);
    }
    RUMBLE.lock().unwrap().iter().find(|r| r.0 == key).map_or((0, 0), |r| r.1)
}

/// Esconde do jogo os controles físicos pelo HidHide (só os que a pessoa marcou): sem isso o
/// jogo pode ver o físico e o virtual juntos.
fn hide_physical(keys: &[String], on: bool) {
    for k in keys {
        if on {
            super::hidhide::esconder(k);
        } else {
            super::hidhide::mostrar(k);
        }
    }
}

/// Vibração do jogo nos controles do XInput (os HID recebem pelo `PadOut`).
fn xinput_rumble(sent: &mut Vec<(String, (u8, u8))>, now: &[(String, (u8, u8))]) {
    use windows::Win32::UI::Input::XboxController::{XInputSetState, XINPUT_VIBRATION};
    // Quem saiu da lista para de vibrar.
    let mut want = now.to_vec();
    want.extend(sent.iter().filter(|s| !now.iter().any(|n| n.0 == s.0)).map(|s| (s.0.clone(), (0, 0))));
    for (k, v) in want {
        let Some(slot) = k.strip_prefix("xinput:").and_then(|n| n.parse::<u32>().ok()) else { continue };
        if sent.iter().find(|s| s.0 == k).map_or((0, 0), |s| s.1) == v {
            continue;
        }
        let vib = XINPUT_VIBRATION { wLeftMotorSpeed: v.0 as u16 * 257, wRightMotorSpeed: v.1 as u16 * 257 };
        unsafe {
            XInputSetState(slot, &vib);
        }
        sent.retain(|s| s.0 != k);
        if v != (0, 0) {
            sent.push((k, v));
        }
    }
}

#[cfg(feature = "stream")]
fn run() {
    let mut runner: Option<Runner> = None;
    let mut sent = vec![];
    let mut hidden: Vec<String> = vec![];
    loop {
        let want = ON.load(Ordering::Relaxed) && stage() == Stage::Ready && !super::remap::stopped() && !super::remap::anti_cheat_front();
        if !want {
            if runner.take().is_some() {
                ACTIVE.store(false, Ordering::Relaxed);
                OWN.store(0, Ordering::Relaxed);
                RUMBLE.lock().unwrap().clear();
                xinput_rumble(&mut sent, &[]);
                hide_physical(&hidden, false);
                hidden.clear();
            }
            std::thread::sleep(Duration::from_millis(250));
            continue;
        }
        if runner.is_none() {
            // shortcut: toda vaga do XInput que estava livre ao ligar fica com os virtuais (o índice
            // que o driver dá pode chegar tarde); um Xbox físico ligado depois só entra ao religar a ponte.
            OWN.store(!crate::stream::pads::xinput_mask() & 0xF, Ordering::Relaxed);
            match Bridge::viiper() {
                Ok(b) => runner = Some(Runner::new(b)),
                Err(_) => {
                    OWN.store(0, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_secs(3));
                    continue;
                }
            }
            ACTIVE.store(true, Ordering::Relaxed);
        }
        let list: Vec<(String, Input)> = pads::list().into_iter().map(|p| (p.key, p.input)).collect();
        let keys: Vec<String> = list.iter().map(|p| p.0.clone()).collect();
        if keys != hidden {
            hide_physical(&hidden, false);
            hide_physical(&keys, true);
            hidden = keys;
        }
        match runner.as_mut().unwrap().step(&list, MERGE.load(Ordering::Relaxed)) {
            Ok(r) => {
                xinput_rumble(&mut sent, &r);
                *RUMBLE.lock().unwrap() = r;
            }
            Err(_) => {
                runner = None;
                ACTIVE.store(false, Ordering::Relaxed);
                std::thread::sleep(Duration::from_secs(2));
                continue;
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}

#[cfg(not(feature = "stream"))]
fn run() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct Fake {
        viiper: Cell<bool>,
        usbip: Cell<bool>,
        server: Cell<bool>,
        /// O install traz o usbip-win2 e sobe o servidor.
        brings_usbip: bool,
        starts: bool,
        fail_download: bool,
        /// O desinstalador do VIIPER também tira o usbip-win2.
        viiper_takes_usbip: bool,
        calls: RefCell<Vec<&'static str>>,
    }

    impl Host for Fake {
        fn viiper_installed(&self) -> bool {
            self.viiper.get()
        }
        fn usbip_installed(&self) -> bool {
            self.usbip.get()
        }
        fn server_up(&self, _: u64) -> bool {
            self.server.get()
        }
        fn download(&self) -> Result<PathBuf, String> {
            self.calls.borrow_mut().push("download");
            if self.fail_download { Err("sem rede".into()) } else { Ok(PathBuf::from("nao-existe.exe")) }
        }
        fn install(&self, _: &Path) -> Result<(), String> {
            self.calls.borrow_mut().push("install");
            self.viiper.set(true);
            self.usbip.set(self.usbip.get() || self.brings_usbip);
            self.server.set(self.starts);
            Ok(())
        }
        fn uninstall_viiper(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("uninstall_viiper");
            self.viiper.set(false);
            self.server.set(false);
            if self.viiper_takes_usbip {
                self.usbip.set(false);
            }
            Ok(())
        }
        fn uninstall_usbip(&self) -> Result<(), String> {
            self.calls.borrow_mut().push("uninstall_usbip");
            self.usbip.set(false);
            Ok(())
        }
        fn clean(&self) {
            self.calls.borrow_mut().push("clean");
        }
    }

    #[test]
    fn install_states_and_driver_ownership() {
        let h = Fake { brings_usbip: true, starts: true, ..Default::default() };
        assert_eq!(detect(&h), Stage::Absent);
        let mut seen = vec![];
        let (s, ours) = install(&h, &mut |s| seen.push(s));
        assert_eq!((s, ours), (Stage::Ready, true));
        assert_eq!(seen, vec![Stage::Downloading, Stage::Installing]);
        // O usbip-win2 que já estava não é do Prisma; sem servidor, pede reinício.
        let h = Fake { starts: false, ..Default::default() };
        h.usbip.set(true);
        assert_eq!(install(&h, &mut |_| {}), (Stage::Reboot, false));
        assert_eq!(detect(&h), Stage::Reboot);
        // Sem rede: nada é instalado.
        let h = Fake { fail_download: true, ..Default::default() };
        assert!(matches!(install(&h, &mut |_| {}).0, Stage::Failed(_)));
        assert_eq!(*h.calls.borrow(), vec!["download"]);
    }

    #[test]
    fn remove_leaves_nothing_and_keeps_what_was_there() {
        let h = Fake { brings_usbip: true, starts: true, ..Default::default() };
        let (_, ours) = install(&h, &mut |_| {});
        assert_eq!(remove(&h, ours, &mut |_| {}), Stage::Absent);
        assert!(!h.viiper.get() && !h.usbip.get());
        assert_eq!(h.calls.borrow()[2..], ["uninstall_viiper", "uninstall_usbip", "clean"]);
        // O usbip-win2 de antes fica.
        let h = Fake { starts: true, ..Default::default() };
        h.usbip.set(true);
        let (_, ours) = install(&h, &mut |_| {});
        assert_eq!(remove(&h, ours, &mut |_| {}), Stage::Absent);
        assert!(h.usbip.get());
        // Se o VIIPER já tirou o driver, não chama o desinstalador dele.
        let h = Fake { brings_usbip: true, viiper_takes_usbip: true, ..Default::default() };
        let (_, ours) = install(&h, &mut |_| {});
        assert_eq!(remove(&h, ours, &mut |_| {}), Stage::Absent);
        assert!(!h.calls.borrow().contains(&"uninstall_usbip"));
    }

    #[test]
    fn digest_and_command_parsing() {
        let json = r#"{"assets":[{"name":"viiper-linux","digest":"sha256:aaaa"},{"name":"viiper-windows-amd64.exe","size":1,"digest":"sha256:ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef0123456789"},{"name":"x"}]}"#;
        assert_eq!(asset_digest(json, ASSET).as_deref(), Some("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"));
        assert_eq!(asset_digest(r#"[{"name":"viiper-windows-amd64.exe"},{"name":"b","digest":"sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}]"#, ASSET), None);
        assert_eq!(split_cmd(r#""C:\Program Files\USBip\unins000.exe" /VERYSILENT"#), (r"C:\Program Files\USBip\unins000.exe".into(), "/VERYSILENT".into()));
        assert_eq!(split_cmd(r"C:\x\u.exe /S"), (r"C:\x\u.exe".into(), "/S".into()));
    }

    #[test]
    fn buttons_sticks_and_merge() {
        let a = Input { buttons: pads::A | pads::GUIDE | pads::UP, lx: 1.0, ly: -0.2, lt: 0.5, ..Default::default() };
        let x = to_xbox(&a);
        assert_eq!(x.buttons, 0x1000 | 0x400 | 0x1);
        assert_eq!((x.lx, x.lt), (32767, 128));
        let b = Input { buttons: pads::Y | pads::PAD, lx: 0.1, rx: -1.0, rt: 1.0, ..Default::default() };
        let m = merge(&[a, b]);
        assert_eq!(m.buttons, pads::A | pads::GUIDE | pads::UP | pads::Y | pads::PAD);
        assert_eq!((m.lx, m.ly, m.rx, m.lt, m.rt), (1.0, -0.2, -1.0, 0.5, 1.0));
        // O touchpad não tem par no Xbox.
        assert_eq!(to_xbox(&m).buttons, 0x1000 | 0x400 | 0x1 | 0x8000);
    }

    #[test]
    fn one_virtual_per_pad_or_one_for_all() {
        let log: crate::stream::pads::FakeLog = Default::default();
        let mut r = Runner::new(Bridge::fake(log.clone()));
        let a = ("ds".to_string(), Input { buttons: pads::A, ..Default::default() });
        let b = ("xinput:0".to_string(), Input { buttons: pads::B, ..Default::default() });
        let out = r.step(&[a.clone(), b.clone()], false).unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(out.iter().map(|o| o.0.as_str()).collect::<Vec<_>>(), ["ds", "xinput:0"]);
        // Juntos: os dois de antes saem e um só recebe os dois botões.
        log.lock().unwrap().clear();
        let out = r.step(&[a.clone(), b], true).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(out.len(), 2);
        assert_eq!(log.lock().unwrap().last().unwrap().1.buttons, 0x1000 | 0x2000);
        // Sem controles, sem virtual.
        r.step(&[], true).unwrap();
        assert_eq!(r.len(), 0);
    }
}
