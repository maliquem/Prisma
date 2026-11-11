//! Planos de energia e detecção de carga para a troca automática.

use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Power::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{fullscreen, procs};
use crate::config::Config;

const SUB_PROCESSOR: GUID = GUID::from_u128(0x54533251_82be_4824_96c1_47b60b740d00);
const MIN_STATE: GUID = GUID::from_u128(0x893dee8e_2bef_41e0_89c6_b55d0929964c);
const MAX_STATE: GUID = GUID::from_u128(0xbc5038f7_23e0_4960_96da_33abaf5935ec);
const BOOST_MODE: GUID = GUID::from_u128(0xbe337238_0d82_4146_a960_4f3749d470c7);
const EPP: GUID = GUID::from_u128(0x36687f9e_e3a5_4dbf_b1dc_15eb381c6863);
const PARK_MIN: GUID = GUID::from_u128(0x0cc5b647_c1df_4637_891a_dec35c318583);
const BALANCED: GUID = GUID::from_u128(0x381b4222_f694_41f0_9685_ff5bb260df2e);
const HIGH_PERF: GUID = GUID::from_u128(0x8c5e7fda_e8bf_4a96_9a85_a6e23a8c635c);

pub const NAME_REST: &str = "Prisma Repouso";
pub const NAME_GAME: &str = "Prisma Jogo";

#[derive(Clone)]
pub struct Plan {
    pub guid: GUID,
    pub id: String,
    pub name: String,
}

pub fn guid_str(g: &GUID) -> String {
    format!("{g:?}").to_ascii_lowercase()
}

pub fn parse_guid(s: &str) -> Option<GUID> {
    let hex: String = s.chars().filter(|c| *c != '-').collect();
    u128::from_str_radix(&hex, 16).ok().map(GUID::from_u128)
}

fn friendly_name(g: &GUID) -> String {
    unsafe {
        let mut size = 0u32;
        let _ = PowerReadFriendlyName(None, Some(g), None, None, None, &mut size);
        let mut buf = vec![0u16; size as usize / 2 + 1];
        if PowerReadFriendlyName(None, Some(g), None, None, Some(buf.as_mut_ptr() as *mut u8), &mut size).0 != 0 {
            return guid_str(g);
        }
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }
}

pub fn list() -> Vec<Plan> {
    let mut out = Vec::new();
    unsafe {
        let mut i = 0;
        loop {
            let mut g = GUID::zeroed();
            let mut size = std::mem::size_of::<GUID>() as u32;
            let r = PowerEnumerate(
                None,
                None,
                None,
                ACCESS_SCHEME,
                i,
                Some(&mut g as *mut GUID as *mut u8),
                &mut size,
            );
            if r.0 != 0 {
                break;
            }
            out.push(Plan { guid: g, id: guid_str(&g), name: friendly_name(&g) });
            i += 1;
        }
    }
    out
}

pub fn active() -> Option<String> {
    unsafe {
        let mut p: *mut GUID = std::ptr::null_mut();
        if PowerGetActiveScheme(None, &mut p).0 != 0 || p.is_null() {
            return None;
        }
        let g = *p;
        let _ = LocalFree(Some(HLOCAL(p as *mut _)));
        Some(guid_str(&g))
    }
}

pub fn set_active(id: &str) -> bool {
    let Some(g) = parse_guid(id) else { return false };
    unsafe { PowerSetActiveScheme(None, Some(&g)).0 == 0 }
}

#[derive(Clone, Default)]
pub struct PlanValues {
    pub name: String,
    pub min: u32,
    pub epp: u32,
    pub boost: u32,
    pub park: u32,
}

fn read_ac(scheme: &GUID, setting: &GUID) -> u32 {
    let mut v = 0;
    unsafe {
        let _ = PowerReadACValueIndex(None, Some(scheme), Some(&SUB_PROCESSOR), Some(setting), &mut v);
    }
    v
}

fn write_ac(scheme: &GUID, setting: &GUID, v: u32) {
    unsafe {
        let _ = PowerWriteACValueIndex(None, scheme, Some(&SUB_PROCESSOR), Some(setting), v);
    }
}

pub fn values(p: &Plan) -> PlanValues {
    PlanValues {
        name: p.name.clone(),
        min: read_ac(&p.guid, &MIN_STATE),
        epp: read_ac(&p.guid, &EPP),
        boost: read_ac(&p.guid, &BOOST_MODE),
        park: read_ac(&p.guid, &PARK_MIN),
    }
}

pub fn boost_name(v: u32) -> &'static str {
    match v {
        0 => "Desligado",
        1 => "Ativado",
        2 => "Agressivo",
        3 => "Eficiente",
        4 => "Efic. agressivo",
        5 => "Agr. garantido",
        6 => "Efic. garantido",
        _ => "Outro",
    }
}

fn create_plan(base: &GUID, name: &str, vals: &[(GUID, u32)]) -> Option<String> {
    unsafe {
        let mut p: *mut GUID = std::ptr::null_mut();
        if PowerDuplicateScheme(None, base, &mut p).0 != 0 || p.is_null() {
            return None;
        }
        let g = *p;
        let _ = LocalFree(Some(HLOCAL(p as *mut _)));
        let w: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 2);
        let _ = PowerWriteFriendlyName(None, &g, None, None, bytes);
        for (setting, v) in vals {
            write_ac(&g, setting, *v);
        }
        Some(guid_str(&g))
    }
}

/// Cria os planos do Prisma que faltam e devolve (repouso, jogo).
pub fn ensure_prisma_plans() -> (Option<String>, Option<String>) {
    let plans = list();
    let find = |n: &str| plans.iter().find(|p| p.name == n).map(|p| p.id.clone());
    let rest = find(NAME_REST).or_else(|| {
        create_plan(
            &BALANCED,
            NAME_REST,
            &[(MIN_STATE, 5), (MAX_STATE, 100), (EPP, 50), (BOOST_MODE, 2), (PARK_MIN, 10)],
        )
    });
    let game = find(NAME_GAME).or_else(|| {
        create_plan(
            &HIGH_PERF,
            NAME_GAME,
            &[(MIN_STATE, 100), (MAX_STATE, 100), (EPP, 0), (BOOST_MODE, 2), (PARK_MIN, 100)],
        )
    });
    (rest, game)
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Rest,
    Load,
    Game,
}

/// Trecho da linha do tempo da tela Energia.
#[derive(Clone)]
pub struct Span {
    pub mode: Mode,
    pub label: String,
    pub start: Instant,
    pub end: Option<Instant>,
}

pub struct Detector {
    pub mode: Mode,
    pub game_exe: String,
    pub game_pid: u32,
    pub since: Instant,
    pub timeline: Vec<Span>,
    gpu_since: Option<Instant>,
    cpu_since: Option<Instant>,
    last_load: Option<Instant>,
    /// Jogos abertos pelo launcher: (pid, nome do exe).
    launched: Vec<(u32, String)>,
}

const LIBRARIES: &[&str] = &[
    "\\steamapps\\common\\",
    "\\epic games\\",
    "\\gog games\\",
    "\\gog galaxy\\games\\",
    "\\xboxgames\\",
    "\\ubisoft game launcher\\games\\",
    "\\ea games\\",
];

/// Usam a GPU em tela cheia sem ser jogo: navegador, vídeo, acesso remoto, gravação, edição e
/// os próprios lançadores. Também os programas de ferramenta que moram na pasta da Steam.
const NOT_GAMES: &[&str] = &[
    "chrome.exe", "msedge.exe", "firefox.exe", "zen.exe", "brave.exe", "opera.exe", "vivaldi.exe", "arc.exe", "librewolf.exe", "floorp.exe", "waterfox.exe",
    "vlc.exe", "mpc-hc64.exe", "mpc-hc.exe", "mpc-be64.exe", "mpv.exe", "potplayermini64.exe", "potplayermini.exe", "kodi.exe", "plex.exe", "stremio.exe", "photos.exe", "applicationframehost.exe",
    "parsecd.exe", "moonlight.exe", "anydesk.exe", "teamviewer.exe", "mstsc.exe", "msrdc.exe", "rustdesk.exe",
    "discord.exe", "obs64.exe", "spotify.exe", "powerpnt.exe", "resolve.exe", "blender.exe", "photoshop.exe", "afterfx.exe", "adobe premiere pro.exe",
    "steam.exe", "steamwebhelper.exe", "epicgameslauncher.exe", "galaxyclient.exe", "eadesktop.exe", "upc.exe", "battle.net.exe", "xboxpcapp.exe", "gamebar.exe", "playnite.fullscreenapp.exe", "playnite.desktopapp.exe",
    "losslessscaling.exe", "wallpaper64.exe", "wallpaper32.exe", "nvidia overlay.exe", "explorer.exe", "prisma.exe",
];

fn in_library(path: &str) -> bool {
    let low = path.to_ascii_lowercase();
    LIBRARIES.iter().any(|l| low.contains(l))
}

pub enum Event {
    None,
    Enter { game: Option<String> },
    Leave,
}

impl Detector {
    pub fn new() -> Self {
        Self {
            mode: Mode::Rest,
            game_exe: String::new(),
            game_pid: 0,
            since: Instant::now(),
            timeline: vec![Span { mode: Mode::Rest, label: "Repouso".into(), start: Instant::now(), end: None }],
            gpu_since: None,
            cpu_since: None,
            last_load: None,
            launched: Vec::new(),
        }
    }

    /// O launcher abriu este jogo: quando for o primeiro plano, entra no modo jogo na hora.
    pub fn hint(&mut self, pid: u32, exe: &str) {
        self.launched.retain(|(p, _)| *p != pid);
        self.launched.push((pid, procs::exe_name(&exe.replace('/', "\\"))));
    }

    pub fn unhint(&mut self, pid: u32) {
        self.launched.retain(|(p, _)| *p != pid);
    }

    /// Esquece os jogos avisados cujo processo já saiu (o aviso de parada pode ter se perdido).
    fn prune(&mut self, alive: impl Fn(u32) -> bool) {
        self.launched.retain(|(p, _)| alive(*p));
    }

    fn hinted(&self, pid: u32, exe: &str) -> bool {
        self.launched.iter().any(|(p, n)| *p == pid || n.eq_ignore_ascii_case(exe))
    }

    fn set_mode(&mut self, mode: Mode, label: &str) {
        let now = Instant::now();
        if let Some(l) = self.timeline.last_mut() {
            l.end = Some(now);
        }
        self.timeline.push(Span { mode, label: label.into(), start: now, end: None });
        let cut = now.checked_sub(Duration::from_secs(2 * 3600));
        if let Some(cut) = cut {
            self.timeline.retain(|s| s.end.map_or(true, |e| e > cut));
        }
        self.mode = mode;
        self.since = now;
    }

    /// `groups` traz a GPU de cada programa: jogo é quem desenha, não quem divide a tela com ele.
    pub fn tick(&mut self, cfg: &Config, cpu: f32, gpu: f32, groups: &[crate::sys::monitor::Group]) -> Event {
        let now = Instant::now();
        self.prune(procs::is_alive);
        let gpu_on = cfg.rule_gpu && gpu > cfg.load_gpu_pct as f32;
        let cpu_on = cfg.rule_cpu && cpu > cfg.load_cpu_pct as f32;
        self.gpu_since = if gpu_on { self.gpu_since.or(Some(now)) } else { None };
        self.cpu_since = if cpu_on { self.cpu_since.or(Some(now)) } else { None };
        let gpu_load = self.gpu_since.is_some_and(|t| now - t >= Duration::from_secs(cfg.load_gpu_secs as u64));
        let cpu_load = self.cpu_since.is_some_and(|t| now - t >= Duration::from_secs(cfg.load_cpu_secs as u64));

        crate::sys::monitor::GPU_WANTED.store(self.mode == Mode::Game, std::sync::atomic::Ordering::Relaxed);
        let fg = unsafe { GetForegroundWindow() };
        let mut game: Option<(String, u32)> = None;
        if !fg.is_invalid() && fg != unsafe { GetShellWindow() } {
            let pid = procs::window_pid(fg);
            if let Some(path) = procs::image_path(pid) {
                let exe = procs::exe_name(&path);
                let known = NOT_GAMES.iter().any(|n| exe.eq_ignore_ascii_case(n));
                let cover = cfg.rule_fullscreen && fullscreen::covers_monitor(fg);
                let lib = cfg.rule_library && in_library(&path);
                let candidate = !known && (cover || lib);
                crate::sys::monitor::GPU_WANTED.store(candidate || self.mode == Mode::Game, std::sync::atomic::Ordering::Relaxed);
                let own = groups.iter().find(|g| g.pids.contains(&pid)).map_or(0.0, |g| g.gpu);
                if self.hinted(pid, &exe) || (candidate && ((cover && own >= 10.0) || (lib && own >= 3.0))) {
                    game = Some((exe, pid));
                }
            }
        }

        let load = game.is_some() || gpu_load || cpu_load;
        if load {
            self.last_load = Some(now);
        }
        match (self.mode, &game) {
            (Mode::Rest | Mode::Load, Some((exe, pid))) => {
                self.game_exe = exe.clone();
                self.game_pid = *pid;
                let label = exe.trim_end_matches(".exe").to_string();
                self.set_mode(Mode::Game, &label);
                return Event::Enter { game: Some(exe.clone()) };
            }
            (Mode::Game, Some((exe, pid))) if *pid != self.game_pid => {
                self.game_exe = exe.clone();
                self.game_pid = *pid;
            }
            (Mode::Rest, None) if load => {
                self.set_mode(Mode::Load, "Carga alta");
                return Event::Enter { game: None };
            }
            _ => {}
        }
        if self.mode != Mode::Rest && !load {
            let idle = self.last_load.map_or(true, |t| now - t >= Duration::from_secs(60));
            if idle {
                self.game_exe.clear();
                self.game_pid = 0;
                self.set_mode(Mode::Rest, "Repouso");
                return Event::Leave;
            }
        }
        Event::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_matches_pid() {
        let mut d = Detector::new();
        d.hint(10, r"C:\G\Hades.exe");
        assert!(d.hinted(10, "other.exe"));
    }

    #[test]
    fn hint_matches_exe_name_any_case() {
        let mut d = Detector::new();
        d.hint(10, r"C:\G\Launcher.exe");
        assert!(d.hinted(99, "LAUNCHER.EXE"));
        assert!(!d.hinted(99, "game.exe"));
    }

    #[test]
    fn hint_normalises_forward_slashes() {
        let mut d = Detector::new();
        d.hint(10, "C:/G/Hades.exe");
        assert!(d.hinted(99, "hades.exe"));
    }

    #[test]
    fn prune_drops_dead_pids() {
        let mut d = Detector::new();
        d.hint(10, r"C:\G\A.exe");
        d.hint(11, r"C:\G\B.exe");
        d.prune(|p| p == 11);
        assert!(!d.hinted(10, "x.exe") && !d.hinted(99, "a.exe"));
        assert!(d.hinted(11, "x.exe"));
    }

    #[test]
    fn unhint_removes() {
        let mut d = Detector::new();
        d.hint(10, r"C:\G\Launcher.exe");
        d.unhint(10);
        assert!(!d.hinted(10, "x.exe"));
        assert!(!d.hinted(99, "launcher.exe"));
    }
}
