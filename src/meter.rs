//! Medidor sobre o jogo: janela própria, sem nada dentro do processo do jogo.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{Config, MeterGame};
use crate::i18n::comma;
use crate::overlays::{focus_monitor, register};
use crate::sys::frames::{self, Stats, Tracker};
use crate::sys::{drs, fileinfo, hdr, monitor};
use crate::ui::gfx::*;
use crate::ui::layered::{create_overlay, Layered};

pub const VIEW_OFF: u8 = 0;
pub const VIEW_COMPACT: u8 = 1;
pub const VIEW_FULL: u8 = 2;

pub const ITEM_FPS: u32 = 1 << 0;
pub const ITEM_FRAME: u32 = 1 << 1;
pub const ITEM_LATENCY: u32 = 1 << 2;
pub const ITEM_LIMIT: u32 = 1 << 3;
pub const ITEM_UPSCALE: u32 = 1 << 4;
pub const ITEM_GEN: u32 = 1 << 5;
pub const ITEM_REFLEX: u32 = 1 << 6;
pub const ITEM_HDR: u32 = 1 << 7;
pub const ITEM_CPU: u32 = 1 << 8;
pub const ITEM_GPU: u32 = 1 << 9;
pub const ITEM_MEM: u32 = 1 << 10;
pub const ITEM_IO: u32 = 1 << 11;
pub const ITEM_MODE: u32 = 1 << 12;
pub const ITEM_TIME: u32 = 1 << 13;
pub const ITEMS_DEFAULT: u32 = ITEM_FPS | ITEM_FRAME | ITEM_LATENCY | ITEM_LIMIT | ITEM_UPSCALE | ITEM_GEN | ITEM_CPU | ITEM_GPU | ITEM_MEM;

/// Minutos de jogo que o limitador automático mede antes de decidir.
pub const AUTO_MINUTES: usize = 5;
const STEPS: [u32; 9] = [240, 165, 144, 120, 100, 90, 75, 60, 45];
const COMPACT_W: f32 = 760.0;
const COMPACT_H: f32 = 40.0;
const FULL_W: f32 = 340.0;
const VRAM_NEAR: f32 = 90.0;
const WARN_RED: Color = REC;

/// O que o jogo carregou. Carregado não prova que está ligado, e a tela diz isso.
#[derive(Clone, Default, Debug)]
pub struct Found {
    pub upscaler: String,
    pub version: String,
    pub frame_gen: String,
    pub reflex: bool,
}

#[derive(Default)]
pub struct Session {
    pub exe: String,
    pub pid: u32,
    pub started: Option<Instant>,
    pub anti_cheat: bool,
    /// Limite gravado para a próxima abertura, depois de medir nesta.
    pub pending: Option<u32>,
    pub measured: VecDeque<f32>,
}

pub struct Meter {
    compact: Layered,
    full: Layered,
    tracker: Option<Tracker>,
    pub view: u8,
    /// A pessoa escondeu pelo atalho: o automático respeita até o jogo fechar.
    hidden: bool,
    pub session: Session,
    pub found: Arc<Mutex<Found>>,
    pub stats: Stats,
    hdr_before: Option<bool>,
    tag: Option<(String, Instant)>,
    target: u32,
    scanned: Option<Instant>,
}

unsafe extern "system" fn meter_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(h, m, w, l),
    }
}

fn surface() -> Layered {
    let hwnd = create_overlay(w!("PrismaMeter"), true);
    crate::capture::hide_from_capture(hwnd);
    Layered::new(hwnd)
}

fn num(v: f32, digits: usize) -> String {
    format!("{v:.digits$}").replace('.', comma())
}

fn gb(b: u64) -> f32 {
    b as f32 / (1u64 << 30) as f32
}

/// Limite seguro para taxa variável: abaixo da taxa da tela, onde o limite do Reflex fica.
pub fn safe_cap(hz: f32) -> u32 {
    (hz * 3600.0 / (hz + 3600.0)).floor() as u32
}

fn scan(pid: u32) -> Option<Found> {
    let mut out = Found::default();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid).ok()?;
        let mut m = MODULEENTRY32W { dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
        let mut more = Module32FirstW(snap, &mut m).is_ok();
        while more {
            let end = m.szModule.iter().position(|c| *c == 0).unwrap_or(m.szModule.len());
            let name = String::from_utf16_lossy(&m.szModule[..end]).to_ascii_lowercase();
            let end = m.szExePath.iter().position(|c| *c == 0).unwrap_or(m.szExePath.len());
            let path = String::from_utf16_lossy(&m.szExePath[..end]);
            let found = |label: &str| (label.to_string(), fileinfo::version(&path).unwrap_or_default());
            if name == "nvngx_dlss.dll" {
                (out.upscaler, out.version) = found("DLSS");
            } else if name.starts_with("libxess") && out.upscaler.is_empty() {
                (out.upscaler, out.version) = found("XeSS");
            } else if (name.starts_with("ffx_fsr") || name.starts_with("amd_fidelityfx")) && out.upscaler.is_empty() {
                (out.upscaler, out.version) = found("FSR");
            }
            if name == "nvngx_dlssg.dll" || name == "sl.dlss_g.dll" {
                out.frame_gen = "DLSS FG".to_string();
            } else if name.starts_with("ffx_frameinterpolation") && out.frame_gen.is_empty() {
                out.frame_gen = "FSR FG".to_string();
            }
            if name == "sl.reflex.dll" || name == "nvlowlatencyvk.dll" {
                out.reflex = true;
            }
            more = Module32NextW(snap, &mut m).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    Some(out)
}

/// Sinais de anti-cheat na pasta do jogo. Achar nada não prova que não tem.
pub fn anti_cheat(path: &str) -> bool {
    let Some(dir) = std::path::Path::new(path).parent() else { return false };
    const NAMES: [&str; 6] = ["easyanticheat", "battleye", "start_protected_game.exe", "beclient", "vgc", "eac"];
    let check = |d: &std::path::Path| {
        std::fs::read_dir(d).is_ok_and(|rd| rd.flatten().any(|e| {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            NAMES.iter().any(|k| n.starts_with(k))
        }))
    };
    check(dir) || dir.parent().is_some_and(check)
}

fn entry<'a>(cfg: &'a Config, exe: &str) -> Option<&'a MeterGame> {
    cfg.meter_games.iter().find(|g| g.exe.eq_ignore_ascii_case(exe))
}

/// Valor que vai para o driver, que sempre conta quadros reais.
pub fn driver_limit(g: &MeterGame, multiplier: u32) -> Option<u32> {
    let value = match g.limit_mode {
        0 => return None,
        _ => g.limit_fps,
    };
    Some(if g.limit_real { value } else { value / multiplier.max(1) })
}

/// Grava no perfil do jogo. O driver lê ao abrir o jogo, então vale na próxima abertura.
pub fn write_limit(g: &MeterGame) -> bool {
    drs::apply(&g.exe, &[(drs::FRAME_LIMIT, driver_limit(g, 1))])
}

pub fn write_preset(g: &MeterGame) -> bool {
    let preset = match g.preset {
        1 => Some(0x00FF_FFFF),
        2 => Some(11),
        3 => Some(12),
        4 => Some(13),
        _ => None,
    };
    drs::apply(&g.exe, &[(drs::DLSS_PRESET, preset), (drs::DLSS_OVERRIDE, preset.map(|_| 1))])
}

impl Meter {
    pub fn new() -> Self {
        register(w!("PrismaMeter"), meter_proc);
        Self {
            compact: surface(),
            full: surface(),
            tracker: None,
            view: VIEW_OFF,
            hidden: false,
            session: Session::default(),
            found: Default::default(),
            stats: Stats::default(),
            hdr_before: None,
            tag: None,
            target: 0,
            scanned: None,
        }
    }

    fn hide_all(&self) {
        unsafe {
            let _ = ShowWindow(self.compact.hwnd, SW_HIDE);
            let _ = ShowWindow(self.full.hwnd, SW_HIDE);
        }
    }

    fn apply_view(&mut self, view: u8) {
        self.view = view;
        if view == VIEW_OFF {
            self.hide_all();
            if self.session.exe.is_empty() {
                self.tracker = None;
                monitor::METER.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            return;
        }
        monitor::METER.store(true, std::sync::atomic::Ordering::Relaxed);
        if self.tracker.is_none() {
            self.tracker = Tracker::start();
            self.target = 0;
        }
    }

    /// Atalhos: mostrar um esconde o outro.
    pub fn show(&mut self, view: u8) {
        self.hidden = view == VIEW_OFF;
        self.apply_view(view);
    }

    fn set_tag(&mut self, text: String) {
        self.tag = Some((text, Instant::now()));
    }

    /// Chamado uma vez por segundo. Liga, desliga e mede conforme o jogo em foco.
    pub fn tick(&mut self, cfg: &mut Config, game: Option<(String, u32, String)>, fg_pid: u32) {
        match game {
            Some((exe, pid, path)) if self.session.exe != exe => self.enter(cfg, exe, pid, path),
            None if !self.session.exe.is_empty() => self.leave(),
            _ => {}
        }
        if self.tracker.is_none() {
            return;
        }
        let want = if self.session.pid != 0 { self.session.pid } else { fg_pid };
        let top = self.tracker.as_ref().and_then(|t| t.top_presenter());
        // Alguns jogos desenham num processo filho: vale quem está mandando quadros.
        let pid = match top {
            Some((p, n)) if n > 15 && self.stats.frames == 0 && self.session.pid != 0 => p,
            _ if self.target == 0 || self.stats.frames > 0 || self.session.pid == 0 => want,
            _ => self.target,
        };
        if pid != self.target {
            self.target = pid;
            self.scanned = None;
            let reflex = frames::reflex_pids().contains(&pid);
            if let Some(t) = &mut self.tracker {
                t.set_target(pid);
                t.set_markers(reflex);
            }
        }
        let stale = self.scanned.map_or(true, |t| t.elapsed().as_secs() >= 20);
        if stale && self.target != 0 && !self.session.anti_cheat {
            self.scanned = Some(Instant::now());
            let (slot, pid) = (self.found.clone(), self.target);
            let _ = std::thread::Builder::new().name("modulos".into()).spawn(move || {
                let mut f = scan(pid).unwrap_or_default();
                f.reflex |= frames::reflex_pids().contains(&pid);
                *slot.lock().unwrap() = f;
            });
        }
        self.measure(cfg);
    }

    /// Automático: guarda o FPS de cada segundo jogado e decide depois de 5 minutos.
    fn measure(&mut self, cfg: &mut Config) {
        let Some(g) = cfg.meter_games.iter_mut().find(|g| g.exe.eq_ignore_ascii_case(&self.session.exe)) else { return };
        if g.limit_mode != 1 || g.measured || self.stats.fps <= 0.0 || self.stats.worst > 250.0 {
            return;
        }
        self.session.measured.push_back(self.stats.fps);
        #[cfg(debug_assertions)]
        let needed = std::env::var("PRISMA_AUTO_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(AUTO_MINUTES * 60);
        #[cfg(not(debug_assertions))]
        let needed = AUTO_MINUTES * 60;
        if self.session.measured.len() < needed {
            return;
        }
        let mut list: Vec<f32> = self.session.measured.iter().copied().collect();
        list.sort_by(|a, b| a.total_cmp(b));
        let sustained = list[list.len() / 20];
        let cap = g.limit_fps;
        let value = if sustained >= cap as f32 * 0.95 { cap } else { STEPS.iter().copied().find(|s| (*s as f32) <= sustained).unwrap_or(30).min(cap) };
        g.limit_fps = value;
        g.measured = true;
        let done = write_limit(g);
        self.session.pending = done.then_some(value);
        cfg.save();
        self.set_tag(format!("Limite {value}"));
    }

    fn enter(&mut self, cfg: &mut Config, exe: String, pid: u32, path: String) {
        self.session = Session { exe: exe.clone(), pid, anti_cheat: anti_cheat(&path), started: Some(Instant::now()), ..Default::default() };
        *self.found.lock().unwrap() = Found::default();
        self.scanned = None;
        self.hidden = false;
        let listed = entry(cfg, &exe).cloned();
        let hdr_wanted = match listed.as_ref().map_or(0, |g| g.hdr) {
            1 => true,
            2 => false,
            _ => match cfg.meter_hdr {
                1 => listed.is_some(),
                2 => true,
                _ => false,
            },
        };
        if hdr_wanted {
            if let Some((adapter, output, on)) = hdr::hdr_output() {
                if !on && hdr::set_hdr(adapter, output, true) {
                    self.hdr_before = Some(false);
                }
            }
        }
        let shown = match cfg.meter_rule {
            0 => listed.is_some(),
            _ => listed.is_none() || listed.as_ref().is_some_and(|g| g.view != 2),
        };
        if shown {
            let view = if listed.as_ref().is_some_and(|g| g.view == 1) { VIEW_FULL } else { VIEW_COMPACT };
            self.apply_view(view);
        } else if listed.as_ref().is_some_and(|g| g.limit_mode == 1 && !g.measured) {
            // Medir para o limitador não depende de o medidor estar visível.
            monitor::METER.store(true, std::sync::atomic::Ordering::Relaxed);
            if self.tracker.is_none() {
                self.tracker = Tracker::start();
                self.target = 0;
            }
        }
    }

    /// Fim do jogo, ou do Prisma: devolve o HDR e fecha o rastreamento.
    pub fn leave(&mut self) {
        if self.hdr_before.take() == Some(false) {
            if let Some((adapter, output, _)) = hdr::hdr_output() {
                hdr::set_hdr(adapter, output, false);
            }
        }
        self.session = Session::default();
        self.stats = Stats::default();
        self.apply_view(VIEW_OFF);
        self.tracker = None;
        self.target = 0;
        monitor::METER.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Multiplicador da geração: quadros na tela por quadro real. Só com prova da contagem.
    fn multiplier(&self) -> Option<u32> {
        let real = self.stats.real_fps?;
        let m = (self.stats.fps / real).round() as u32;
        (real > 5.0 && (2..=4).contains(&m)).then_some(m)
    }

    /// Chamado quatro vezes por segundo com o medidor na tela.
    pub fn paint(&mut self, cfg: &Config, snap: &monitor::Snapshot, hdr_light: Option<(bool, hdr::Light)>) {
        if let Some(t) = &self.tracker {
            self.stats = t.stats();
        }
        if self.tag.as_ref().is_some_and(|t| t.1.elapsed().as_secs() >= 6) {
            self.tag = None;
        }
        match self.view {
            VIEW_COMPACT => {
                self.draw_compact(cfg, snap);
                unsafe {
                    let _ = ShowWindow(self.full.hwnd, SW_HIDE);
                }
            }
            VIEW_FULL => {
                self.draw_full(cfg, snap, hdr_light);
                unsafe {
                    let _ = ShowWindow(self.compact.hwnd, SW_HIDE);
                }
            }
            _ => {}
        }
    }

    fn draw_compact(&mut self, cfg: &Config, snap: &monitor::Snapshot) {
        let s = self.stats.clone();
        let mon = focus_monitor();
        let hwnd = self.compact.hwnd;
        let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
        let tag = self.tag.as_ref().map(|t| t.0.clone()).or_else(|| self.limit_tag(cfg));
        let w = if tag.is_some() { COMPACT_W + 104.0 } else { COMPACT_W };
        let x = mon.left + ((mon.right - mon.left) as f32 - w * scale) as i32 / 2;
        let y = mon.top + (14.0 * scale) as i32;
        let alpha = cfg.meter_op_compact as f32 / 100.0;
        let generated = self.multiplier().is_some();
        let vram = snap.vram_pct();
        let (used, total) = (gb(snap.gpu.used), gb(snap.gpu.total));
        let latency = s.latency;
        self.compact.paint(x, y, w, COMPACT_H, |c, r| {
            c.round(r, 10.0, rgb(0x17191d).alpha(alpha));
            c.round_line(r, 10.0, rgb(0x5a6170).alpha(0.55), 1.0);
            let mid = rect(0.0, 0.0, 0.0, r.h);
            let at = |x: f32, w: f32| rect(x, mid.y, w, mid.h);
            let mut x = 14.0;
            if generated {
                c.text(at(x, 16.0), "G", 21.0, ACCENT, Align::Left, Weight::Semibold);
                x += 20.0;
            }
            let fps_text = if s.frames > 0 { format!("{:.0}", s.fps) } else { "—".to_string() };
            c.text(at(x, 44.0), &fps_text, 20.0, if s.spike { WARN } else { WHITE }, Align::Left, Weight::Semibold);
            c.text(at(x + 44.0, 28.0), "FPS", 12.0, TEXT2, Align::Left, Weight::Regular);
            x += 84.0;
            c.line(x, 10.0, x, 30.0, rgb(0x3a3f49), 1.0);
            x += 12.0;
            strip(c, rect(x, 8.0, 120.0, 24.0), &s);
            x += 128.0;
            let ms_color = if s.spike { WARN } else { TEXT };
            c.text(at(x, 60.0), &format!("{} ms", num(s.ms, 1)), 13.0, ms_color, Align::Left, Weight::Regular);
            x += 66.0;
            c.line(x, 10.0, x, 30.0, rgb(0x3a3f49), 1.0);
            x += 12.0;
            let pair = |x: f32, name: &str, nw: f32, value: &str| {
                c.text(at(x, nw), name, 12.0, TEXT2, Align::Left, Weight::Regular);
                c.text(at(x + nw + 4.0, 52.0), value, 14.0, WHITE, Align::Left, Weight::Semibold);
            };
            let lat = latency.map_or("—".to_string(), |v| format!("{v:.0} ms"));
            pair(x, "Latência", 50.0, &lat);
            x += 104.0;
            pair(x, "CPU", 26.0, &format!("{:.0}%", snap.cpu));
            x += 70.0;
            pair(x, "GPU", 26.0, &format!("{:.0}%", snap.gpu.util));
            x += 70.0;
            let hot = vram >= VRAM_NEAR;
            c.text(at(x, 36.0), "VRAM", 12.0, if hot { WARN_RED } else { TEXT2 }, Align::Left, if hot { Weight::Semibold } else { Weight::Regular });
            x += 40.0;
            let bar = rect(x, 17.0, 64.0, 6.0);
            c.round(bar, 3.0, CARD2);
            c.round(rect(bar.x, bar.y, 64.0 * (vram / 100.0).min(1.0), 6.0), 3.0, if hot { WARN_RED } else { ACCENT });
            x += 70.0;
            c.text(at(x, 34.0), &num(used, 1), 14.0, WHITE, Align::Left, Weight::Semibold);
            x += 32.0;
            c.text(at(x, 50.0), &format!("/ {total:.0} GB"), 12.0, TEXT2, Align::Left, Weight::Regular);
            if let Some(t) = &tag {
                c.text(rect(r.w - 110.0, 0.0, 98.0, r.h), t, 12.0, ACCENT, Align::Right, Weight::Regular);
            }
        });
        show_topmost(hwnd);
    }

    fn limit_tag(&self, cfg: &Config) -> Option<String> {
        let g = entry(cfg, &self.session.exe)?;
        (g.limit_mode == 1 && !g.measured).then(|| format!("Medindo {} min", self.session.measured.len() / 60))
    }

    fn lines(&self, cfg: &Config, snap: &monitor::Snapshot, hdr_light: Option<(bool, hdr::Light)>) -> Vec<Line> {
        let s = &self.stats;
        let on = |bit: u32| cfg.meter_items & bit != 0;
        let found = self.found.lock().unwrap().clone();
        let game = entry(cfg, &self.session.exe);
        let mut out = Vec::new();
        let name = if self.session.exe.is_empty() { "Medidor".to_string() } else { crate::pages::app_name(&self.session.exe).to_string() };
        out.push(Line::Title(name));
        let shield = if self.session.exe.is_empty() {
            None
        } else if self.session.anti_cheat {
            Some("Anti-cheat detectado")
        } else {
            Some("Nenhum anti-cheat detectado")
        };
        let mode = if s.direct { "Flip direto" } else { "Composto pelo Windows" };
        let mut tags = shield.unwrap_or_default().to_string();
        if on(ITEM_MODE) {
            if !tags.is_empty() {
                tags += " · ";
            }
            tags += mode;
        }
        if !tags.is_empty() {
            out.push(Line::Note(tags, TEXT2));
        }
        if on(ITEM_FPS) {
            let split = self.multiplier().map(|_| {
                let real = s.real_fps.unwrap_or(0.0);
                format!("{real:.0} reais + {:.0} gerados", s.fps - real)
            });
            out.push(Line::Big(s.fps, self.multiplier().is_some(), split.unwrap_or_default()));
            out.push(Line::Three([("Média", format!("{:.0}", s.avg)), ("1% low", format!("{:.0}", s.low1)), ("0,1% low", format!("{:.0}", s.low01))]));
        }
        if on(ITEM_FRAME) {
            out.push(Line::Pair("Frame time".into(), format!("{} ms · variação {} ms", num(s.ms, 1), num(s.dev, 1)), if s.spike { WARN } else { TEXT }));
            out.push(Line::Strip);
        }
        if on(ITEM_LATENCY) {
            let text = s.latency.map_or("Só em jogos com Reflex".to_string(), |v| format!("{v:.0} ms"));
            out.push(Line::Pair("Latência do PC".into(), text, TEXT));
        }
        if on(ITEM_LIMIT) {
            out.push(Line::Head("Limitador".into()));
            let (kind, value) = match game.map_or(0, |g| g.limit_mode) {
                1 => ("Automático", game.map_or(0, |g| g.limit_fps)),
                2 => ("Manual", game.map_or(0, |g| g.limit_fps)),
                _ => ("Desligado", 0),
            };
            out.push(Line::Pair("Modo".into(), kind.to_string(), TEXT));
            if value > 0 {
                let unit = if game.is_some_and(|g| g.limit_real) { "FPS real" } else { "Com geração" };
                out.push(Line::Pair("Limite".into(), format!("{value} · {unit}"), TEXT));
                out.push(Line::Pair("Método".into(), "Limite do driver".to_string(), TEXT));
            }
            if game.is_some_and(|g| g.limit_mode == 1 && !g.measured) {
                out.push(Line::Note(format!("Medindo há {} min de {AUTO_MINUTES}.", self.session.measured.len() / 60), TEXT2));
            }
            if let Some(v) = self.session.pending {
                out.push(Line::Note(format!("Limite {v} vale na próxima abertura do jogo."), ACCENT));
            }
        }
        if on(ITEM_UPSCALE) {
            out.push(Line::Head("Upscaling".into()));
            if found.upscaler.is_empty() {
                out.push(Line::Note("Nenhum carregado".into(), TEXT2));
            } else {
                out.push(Line::Pair(found.upscaler.clone(), format!("{} · carregado", found.version), TEXT));
            }
            let forced = game.map_or(0, |g| g.preset);
            if forced > 0 {
                out.push(Line::Pair("Perfil".into(), ["", "Recomendado", "Preset K", "Preset L", "Preset M"][forced as usize].to_string(), TEXT));
            }
        }
        if on(ITEM_GEN) {
            let state = match (self.multiplier(), found.frame_gen.is_empty()) {
                (Some(m), _) => format!("Ligada · {m}x"),
                (None, false) => format!("{} carregado", found.frame_gen),
                _ => "Não detectada".to_string(),
            };
            out.push(Line::Pair("Geração de quadros".into(), state, TEXT));
        }
        if on(ITEM_REFLEX) {
            out.push(Line::Pair("Reflex".into(), if found.reflex { "Integrado no jogo" } else { "Não detectado" }.to_string(), TEXT));
        }
        if on(ITEM_HDR) {
            out.push(Line::Head("HDR".into()));
            match hdr_light {
                Some((true, l)) => {
                    out.push(Line::Pair("Tela".into(), "HDR ligado".into(), ACCENT));
                    out.push(Line::Pair("Pico informado".into(), format!("{:.0} nits", l.peak), TEXT));
                    out.push(Line::Pair("Tela cheia".into(), format!("{:.0} nits", l.full), TEXT));
                    out.push(Line::Pair("Preto".into(), format!("{} nits", num(l.black, 3)), TEXT));
                    out.push(Line::Note(format!("Sugestão no jogo: pico {:.0} nits · branco 200 nits", l.peak), TEXT2));
                }
                _ => out.push(Line::Pair("Tela".into(), "HDR desligado".into(), TEXT)),
            }
        }
        if on(ITEM_CPU) {
            out.push(Line::Head(format!("CPU {:.0}%", snap.cpu)));
            out.push(Line::Pair("Frequência".into(), format!("{} GHz", num(snap.ghz, 2)), TEXT));
            out.push(Line::Cores(snap.threads.clone()));
        }
        if on(ITEM_GPU) {
            let g = &snap.gpu;
            out.push(Line::Head(format!("GPU {:.0}%", g.util)));
            out.push(Line::Pair("Núcleo".into(), format!("{} MHz · {} °C · {:.0} W", g.mhz, g.temp, g.watts), TEXT));
            out.push(Line::Pair("Memória".into(), format!("{} MHz · ventoinha {}%", g.mem_mhz, g.fan), TEXT));
        }
        if on(ITEM_MEM) {
            out.push(Line::Bar("VRAM".into(), format!("{} / {:.0} GB", num(gb(snap.gpu.used), 1), gb(snap.gpu.total)), snap.vram_pct()));
            out.push(Line::Bar("RAM".into(), format!("{} / {:.0} GB", num(gb(snap.ram_used), 1), gb(snap.ram_total)), snap.ram_pct()));
        }
        if on(ITEM_IO) {
            let group = snap.groups.iter().find(|g| g.exe.eq_ignore_ascii_case(&self.session.exe));
            let mb = |v: u64| num(v as f32 / (1 << 20) as f32, 1);
            out.push(Line::Pair("Disco".into(), format!("{} MB/s", mb(group.map_or(0, |g| g.disk))), TEXT));
            out.push(Line::Pair("Rede".into(), format!("{} MB/s", mb(group.map_or(0, |g| g.net))), TEXT));
        }
        if on(ITEM_TIME) {
            if let Some(t) = self.session.started {
                out.push(Line::Pair("Tempo de jogo".into(), format!("{} min", t.elapsed().as_secs() / 60), TEXT));
            }
        }
        out
    }

    fn draw_full(&mut self, cfg: &Config, snap: &monitor::Snapshot, hdr_light: Option<(bool, hdr::Light)>) {
        let lines = self.lines(cfg, snap, hdr_light);
        let s = self.stats.clone();
        let mon = focus_monitor();
        let hwnd = self.full.hwnd;
        let scale = unsafe { GetDpiForWindow(hwnd) } as f32 / 96.0;
        let room = (mon.bottom - mon.top) as f32 / scale - 48.0;
        let inner: f32 = lines.iter().map(Line::height).sum::<f32>() + 28.0;
        let two = inner > room;
        let (w, h) = if two { (FULL_W * 2.0 + 12.0, (inner / 2.0 + 60.0).min(room)) } else { (FULL_W, inner) };
        let x = mon.right - ((w + 24.0) * scale) as i32;
        let y = mon.top + (24.0 * scale) as i32;
        let alpha = cfg.meter_op_full as f32 / 100.0;
        self.full.paint(x, y, w, h, |c, r| {
            c.round(r, 14.0, rgb(0x17191d).alpha(alpha));
            c.round_line(r, 14.0, rgb(0x5a6170).alpha(0.55), 1.0);
            let (mut cx, mut cy) = (16.0, 14.0);
            let width = FULL_W - 32.0;
            for l in &lines {
                if cy + l.height() > r.h - 12.0 && two && cx < FULL_W {
                    cx = FULL_W + 12.0;
                    cy = 14.0;
                }
                l.draw(c, cx, cy, width, &s);
                cy += l.height();
            }
        });
        show_topmost(hwnd);
    }
}

fn show_topmost(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
}

/// Listra de tempo de quadro: calma quando igual, laranja quando trava.
fn strip(c: &Canvas, r: Rect, s: &Stats) {
    let mid = r.y + r.h / 2.0;
    c.line(r.x, mid, r.right(), mid, rgb(0x4a5160), 1.0);
    if s.strip.len() < 2 || s.ms <= 0.0 {
        return;
    }
    let color = if s.spike { WARN } else { ACCENT };
    let step = r.w / (s.strip.len() - 1) as f32;
    let y = |v: f32| (mid - (v - s.ms) / s.ms * r.h).clamp(r.y, r.bottom());
    for (i, pair) in s.strip.windows(2).enumerate() {
        let x = r.x + i as f32 * step;
        c.line(x, y(pair[0]), x + step, y(pair[1]), color, 1.5);
    }
}

enum Line {
    Title(String),
    Note(String, Color),
    Head(String),
    Pair(String, String, Color),
    Big(f32, bool, String),
    Three([(&'static str, String); 3]),
    Strip,
    Cores(Vec<f32>),
    Bar(String, String, f32),
}

impl Line {
    fn height(&self) -> f32 {
        match self {
            Line::Title(_) => 26.0,
            Line::Note(..) => 20.0,
            Line::Head(_) => 30.0,
            Line::Pair(..) => 22.0,
            Line::Big(..) => 62.0,
            Line::Three(_) => 40.0,
            Line::Strip => 46.0,
            Line::Cores(_) => 44.0,
            Line::Bar(..) => 34.0,
        }
    }

    fn draw(&self, c: &Canvas, x: f32, y: f32, w: f32, s: &Stats) {
        match self {
            Line::Title(t) => c.text(rect(x, y, w, 24.0), t, 15.0, WHITE, Align::Left, Weight::Semibold),
            Line::Note(t, color) => c.text(rect(x, y, w, 18.0), t, 12.0, *color, Align::Left, Weight::Regular),
            Line::Head(t) => {
                c.line(x, y + 6.0, x + w, y + 6.0, rgb(0x3a3f49), 1.0);
                c.text(rect(x, y + 8.0, w, 20.0), t, 13.0, WHITE, Align::Left, Weight::Semibold);
            }
            Line::Pair(a, b, color) => {
                c.text(rect(x, y, w * 0.42, 20.0), a, 12.0, TEXT2, Align::Left, Weight::Regular);
                c.text(rect(x + w * 0.42, y, w * 0.58, 20.0), b, 13.0, *color, Align::Right, Weight::Regular);
            }
            Line::Big(fps, generated, split) => {
                let mut tx = x;
                if *generated {
                    c.text(rect(tx, y, 32.0, 56.0), "G", 44.0, ACCENT, Align::Left, Weight::Semibold);
                    tx += 36.0;
                }
                c.text(rect(tx, y, 110.0, 56.0), &format!("{fps:.0}"), 44.0, WHITE, Align::Left, Weight::Semibold);
                c.text(rect(tx + 104.0, y + 8.0, 40.0, 44.0), "FPS", 13.0, TEXT2, Align::Left, Weight::Regular);
                c.text(rect(tx + 140.0, y + 8.0, w - (tx - x) - 140.0, 44.0), split, 12.0, TEXT2, Align::Right, Weight::Regular);
            }
            Line::Three(items) => {
                for (i, (name, value)) in items.iter().enumerate() {
                    let cx = x + i as f32 * w / 3.0;
                    c.text(rect(cx, y, w / 3.0, 16.0), name, 12.0, TEXT2, Align::Left, Weight::Regular);
                    c.text(rect(cx, y + 16.0, w / 3.0, 20.0), value, 15.0, WHITE, Align::Left, Weight::Semibold);
                }
            }
            Line::Strip => strip(c, rect(x, y + 2.0, w, 38.0), s),
            Line::Cores(list) => {
                let n = list.len().max(1) as f32;
                let bw = (w - (n - 1.0) * 2.0) / n;
                for (i, v) in list.iter().enumerate() {
                    let bx = x + i as f32 * (bw + 2.0);
                    c.round(rect(bx, y + 2.0, bw, 36.0), 2.0, CARD2);
                    let h = 36.0 * (v / 100.0).clamp(0.02, 1.0);
                    c.round(rect(bx, y + 38.0 - h, bw, h), 2.0, if *v >= 90.0 { WARN } else { ACCENT });
                }
            }
            Line::Bar(name, value, pct) => {
                let hot = *pct >= VRAM_NEAR && name == "VRAM";
                c.text(rect(x, y, w * 0.4, 18.0), name, 12.0, if hot { WARN_RED } else { TEXT2 }, Align::Left, if hot { Weight::Semibold } else { Weight::Regular });
                c.text(rect(x + w * 0.4, y, w * 0.6, 18.0), value, 13.0, TEXT, Align::Right, Weight::Regular);
                let bar = rect(x, y + 21.0, w, 6.0);
                c.round(bar, 3.0, CARD2);
                c.round(rect(bar.x, bar.y, w * (pct / 100.0).min(1.0), 6.0), 3.0, if hot { WARN_RED } else { ACCENT });
            }
        }
    }
}

/// Teto inicial do automático, abaixo da taxa da tela.
pub fn first_limit(hz: f32) -> u32 {
    safe_cap(hz).max(30)
}
