//! Otimizar: decide o que pode ser fechado. Nada fecha sem confirmação.
//! Blacklist protege. Whitelist sempre fecha ao otimizar.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{data_dir, Config};
use crate::sys::monitor::Group;
use crate::sys::procs;

const SYSTEM: &[&str] = &[
    "system", "registry", "memory compression", "secure system", "smss.exe", "csrss.exe", "wininit.exe",
    "winlogon.exe", "services.exe", "lsass.exe", "svchost.exe", "dwm.exe", "explorer.exe", "sihost.exe",
    "fontdrvhost.exe", "ctfmon.exe", "runtimebroker.exe", "shellexperiencehost.exe",
    "startmenuexperiencehost.exe", "searchhost.exe", "searchindexer.exe", "textinputhost.exe",
    "taskhostw.exe", "audiodg.exe", "spoolsv.exe", "wudfhost.exe", "dllhost.exe", "conhost.exe",
    "applicationframehost.exe", "securityhealthservice.exe", "securityhealthsystray.exe", "msmpeng.exe",
    "nissrv.exe", "wmiprvse.exe", "nvcontainer.exe", "nvdisplay.container.exe", "nvsphelper64.exe",
    "lsaiso.exe", "wlanext.exe", "dashost.exe", "sgrmbroker.exe", "unsecapp.exe", "prisma.exe",
    "widgets.exe", "widgetservice.exe", "phoneexperiencehost.exe", "smartscreen.exe", "userinit.exe",
];

const WORK_TOOLS: &[&str] = &[
    "claude.exe", "windowsterminal.exe", "wt.exe", "powershell.exe", "pwsh.exe", "cmd.exe", "code.exe",
    "devenv.exe", "cargo.exe", "rustc.exe", "python.exe", "node.exe", "git.exe", "idea64.exe",
];

pub fn is_system(exe: &str) -> bool {
    let low = exe.to_ascii_lowercase();
    SYSTEM.contains(&low.as_str())
}

fn contains(list: &[String], exe: &str) -> bool {
    list.iter().any(|v| v.eq_ignore_ascii_case(exe))
}

pub struct Candidate {
    pub exe: String,
    pub pids: Vec<u32>,
    pub why: String,
    pub ram: u64,
    pub cpu: f32,
    pub vram: u64,
    pub on: bool,
    pub white: bool,
}

fn visible_pids() -> Vec<u32> {
    procs::top_windows().into_iter().map(procs::window_pid).collect()
}

fn in_windows_dir(pid: u32) -> bool {
    procs::image_path(pid)
        .map(|p| p.to_ascii_lowercase().starts_with("c:\\windows\\"))
        .unwrap_or(true)
}

pub fn candidates(cfg: &Config, usage: &Usage, groups: &[Group], fg_pid: u32, game_pid: u32) -> Vec<Candidate> {
    let vis = visible_pids();
    let mut out = Vec::new();
    for g in groups {
        if is_system(&g.exe) || contains(&cfg.black, &g.exe) {
            continue;
        }
        if g.pids.iter().any(|p| *p == fg_pid || *p == game_pid) {
            continue;
        }
        let lower = g.exe.to_ascii_lowercase();
        if WORK_TOOLS.contains(&lower.as_str()) {
            continue;
        }
        let white = contains(&cfg.white, &g.exe);
        let heavy = g.ram > 150 << 20 || g.cpu > 1.0 || g.vram > 100 << 20;
        if !white && !heavy {
            continue;
        }
        if !white && g.pids.first().is_none_or(|p| in_windows_dir(*p)) {
            continue;
        }
        let has_window = g.pids.iter().any(|p| vis.contains(p));
        let why = if white {
            "Sempre fechar (whitelist)".to_string()
        } else if has_window {
            "Aberto em segundo plano".to_string()
        } else {
            "Sem janela aberta".to_string()
        };
        out.push(Candidate {
            exe: g.exe.clone(),
            pids: g.pids.clone(),
            why,
            ram: g.ram,
            cpu: g.cpu,
            vram: g.vram,
            on: white || (!has_window && !usage.used_today(&g.exe)),
            white,
        });
    }
    out.sort_by(|a, b| b.white.cmp(&a.white).then(b.ram.cmp(&a.ram)));
    out
}

/// Pede para fechar, inclusive janelas escondidas de programas na bandeja.
/// Devolve false quando não há janela: aí só fecha à força.
pub fn close(c: &Candidate) -> bool {
    let mut any = false;
    for h in procs::all_windows() {
        if c.pids.contains(&procs::window_pid(h)) {
            unsafe {
                let _ = PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            any = true;
        }
    }
    any
}

pub fn force(pids: &[u32]) {
    for p in pids {
        unsafe {
            if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, *p) {
                let _ = TerminateProcess(h, 1);
                let _ = CloseHandle(h);
            }
        }
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[derive(Clone, Copy, Default)]
struct Use {
    first_seen: u64,
    last_fg: u64,
    fg_secs: u64,
}

/// Histórico de uso por programa, base da detecção automática das listas.
#[derive(Default)]
pub struct Usage {
    map: HashMap<String, Use>,
    dirty: bool,
}

pub enum Suggestion {
    White(String, u64),
    Black(String),
}

const DAY: u64 = 86400;

impl Usage {
    fn path() -> std::path::PathBuf {
        data_dir().join("usage.ini")
    }

    pub fn load() -> Self {
        let mut u = Usage::default();
        if let Ok(t) = std::fs::read_to_string(Self::path()) {
            for l in t.lines() {
                let p: Vec<&str> = l.split('|').collect();
                if p.len() == 4 {
                    u.map.insert(
                        p[0].into(),
                        Use {
                            first_seen: p[1].parse().unwrap_or(0),
                            last_fg: p[2].parse().unwrap_or(0),
                            fg_secs: p[3].parse().unwrap_or(0),
                        },
                    );
                }
            }
        }
        u
    }

    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let mut s = String::new();
        for (k, v) in &self.map {
            s.push_str(&format!("{k}|{}|{}|{}\n", v.first_seen, v.last_fg, v.fg_secs));
        }
        let _ = std::fs::create_dir_all(data_dir());
        let _ = std::fs::write(Self::path(), s);
        self.dirty = false;
    }

    pub fn note(&mut self, groups: &[Group], fg_exe: &str, secs: u64) {
        let t = now();
        for g in groups {
            if is_system(&g.exe) {
                continue;
            }
            self.map.entry(g.exe.to_ascii_lowercase()).or_insert_with(|| {
                self.dirty = true;
                Use { first_seen: t, ..Default::default() }
            });
        }
        if !fg_exe.is_empty() {
            let e = self.map.entry(fg_exe.to_ascii_lowercase()).or_insert(Use { first_seen: t, ..Default::default() });
            e.last_fg = t;
            e.fg_secs += secs;
            self.dirty = true;
        }
    }

    pub fn used_today(&self, exe: &str) -> bool {
        self.map.get(&exe.to_ascii_lowercase()).is_some_and(|u| now() - u.last_fg < DAY)
    }

    /// Uma sugestão por vez, para a tela não virar lista de pendências.
    pub fn suggest(&self, cfg: &Config, groups: &[Group]) -> Option<Suggestion> {
        let t = now();
        let known = |e: &str| contains(&cfg.black, e) || contains(&cfg.white, e) || contains(&cfg.ignored, e);
        for g in groups {
            if is_system(&g.exe) || known(&g.exe) {
                continue;
            }
            let Some(u) = self.map.get(&g.exe.to_ascii_lowercase()) else { continue };
            if u.fg_secs > 3600 && t - u.last_fg < 3 * DAY {
                return Some(Suggestion::Black(g.exe.clone()));
            }
            let days = (t - u.first_seen.max(u.last_fg)) / DAY;
            if days >= 7 && g.ram > 100 << 20 {
                return Some(Suggestion::White(g.exe.clone(), days));
            }
        }
        None
    }
}
