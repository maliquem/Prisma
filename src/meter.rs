//! Medidor sobre o jogo: janela própria, sem nada dentro do processo do jogo.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::{Config, MeterGame};
use crate::i18n::comma;
use crate::overlays::{focus_monitor, register};
use crate::sys::frames::{self, Stats, Tracker};
use crate::sys::gamelog::{self, Log};
use crate::sys::inject::{self, Limiter, Status};
use crate::sys::{drs, fileinfo, hdr, monitor, power};
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
pub const ITEM_LOG: u32 = 1 << 14;
pub const ITEM_POWER: u32 = 1 << 15;
pub const ITEMS_DEFAULT: u32 = ITEM_FPS | ITEM_FRAME | ITEM_LATENCY | ITEM_LIMIT | ITEM_UPSCALE | ITEM_GEN | ITEM_CPU | ITEM_GPU | ITEM_MEM | ITEM_LOG | ITEM_POWER;

/// Minutos de jogo que o limitador automático mede antes de decidir.
pub const AUTO_MINUTES: usize = 5;
const STEPS: [u32; 9] = [240, 165, 144, 120, 100, 90, 75, 60, 45];
const COMPACT_W: f32 = 760.0;
const COMPACT_H: f32 = 40.0;
const COMPACT_TOP: f32 = 7.0;
const FULL_W: f32 = 340.0;
const EDGE: f32 = 24.0;
/// A linha do tempo de quadro. Abaixo disto é ruído de quando o jogo chama a apresentação, e ela fica parada.
const STRIP_QUIET: f32 = 1.0;
/// Variação em ms que enche meia altura, igual em qualquer FPS.
const STRIP_RANGE: f32 = 12.0;
/// Espera depois de abrir o jogo: carregar cedo é a causa mais comum de queda.
const INJECT_AFTER: u64 = 8;
const VRAM_NEAR: f32 = 90.0;
const WARN_RED: Color = REC;
/// Segundos com o jogo em foco e nenhum programa apresentando antes de religar o rastreamento.
const SILENT_SECS: u32 = 6;
const RETRY_SECS: u64 = 5;

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
    /// Limite com que o jogo abriu: o driver só lê nessa hora.
    pub opened: Option<u32>,
    opened_preset: u8,
    inject_tried: bool,
    /// Por que a injeção não está valendo neste jogo agora.
    inject_note: Option<&'static str>,
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
    /// Havia quadros na última olhada: a mudança vai para o registro uma vez só.
    live: bool,
    by_flip: bool,
    silent: u32,
    restarted: Option<Instant>,
    /// Jogo com limite trocado pelos atalhos, gravado quando as teclas param.
    limit_write: Option<(String, Instant)>,
    pub log: Option<Log>,
    /// Pasta do registro que acabou de fechar, para avisar.
    pub saved: Option<std::path::PathBuf>,
    /// Largura do completo, que dobra com duas colunas.
    full_w: f32,
    limiter: Option<Limiter>,
    limiter_exe: String,
    /// Janela que estava em foco antes de abrir os controles.
    back: HWND,
    /// Linha escolhida pelo teclado.
    row: usize,
    rows: Vec<Row>,
    hover: Option<(Row, usize)>,
    hits: Vec<(Rect, Row, usize)>,
    /// Opção marcada em cada linha.
    chosen: Vec<(Row, usize)>,
    /// Número sendo digitado para o limite.
    typing: Option<String>,
    /// Planos de energia (id, nome) e o ativo, lidos de tempos em tempos.
    plans: Vec<(String, String)>,
    plan_active: String,
    plans_at: Option<Instant>,
}

/// Opção do campo do limite. Valores acima dela são o número digitado.
const TYPE_LIMIT: usize = 2;

/// Controles abertos: só aí o medidor recebe mouse e teclado.
static CONTROLS: AtomicBool = AtomicBool::new(false);
/// Janela do completo, para o gancho do teclado mandar as teclas.
static FULL_HWND: AtomicIsize = AtomicIsize::new(0);
/// Tecla que o gancho pegou, entregue ao painel.
const WM_METER_KEY: u32 = WM_APP + 41;

/// Com os controles abertos, as teclas deles vão para o medidor e não para o jogo. O jogo
/// continua em foco: em tela cheia com outra resolução, tirar o foco dele faria a tela trocar.
unsafe extern "system" fn controls_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 && CONTROLS.load(Relaxed) {
        let k = &*(l.0 as *const KBDLLHOOKSTRUCT);
        let vk = k.vkCode;
        let ours = matches!(vk, 0x08 | 0x0D | 0x1B | 0x25..=0x28 | 0x30..=0x39 | 0x60..=0x69);
        let mods = GetAsyncKeyState(VK_CONTROL.0 as i32) < 0 || GetAsyncKeyState(VK_MENU.0 as i32) < 0;
        if ours && !mods {
            if w.0 as u32 == WM_KEYDOWN || w.0 as u32 == WM_SYSKEYDOWN {
                let h = HWND(FULL_HWND.load(Relaxed) as *mut _);
                let _ = PostMessageW(Some(h), WM_METER_KEY, WPARAM(vk as usize), LPARAM(0));
            }
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, w, l)
}

static HOOK: std::sync::Mutex<Option<isize>> = std::sync::Mutex::new(None);

/// Pastas de jogo já olhadas: tem o DLSS (a escolha do perfil só vale com ele)?
static DLSS: std::sync::Mutex<Option<HashMap<String, Option<bool>>>> = std::sync::Mutex::new(None);

/// O jogo traz o DLSS? Olha os arquivos da pasta dele uma vez, fora da tela; até saber, None.
pub fn has_dlss(path: &str) -> Option<bool> {
    if path.is_empty() {
        return Some(false);
    }
    let mut guard = DLSS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    if let Some(v) = map.get(path) {
        return *v;
    }
    map.insert(path.to_string(), None);
    let key = path.to_string();
    let _ = std::thread::Builder::new().name("dlss".into()).spawn(move || {
        let found = dlss_in(&game_root(&key));
        if let Some(m) = DLSS.lock().unwrap().as_mut() {
            m.insert(key, Some(found));
        }
    });
    None
}

/// A pasta do jogo: a da biblioteca da Steam, ou acima das pastas de binários.
fn game_root(exe: &str) -> std::path::PathBuf {
    let path = std::path::Path::new(exe);
    let low = exe.to_ascii_lowercase();
    if let Some(i) = low.find("\\steamapps\\common\\") {
        let rest = &exe[i + "\\steamapps\\common\\".len()..];
        if let Some(name) = rest.split('\\').next() {
            return std::path::PathBuf::from(&exe[..i + "\\steamapps\\common\\".len()]).join(name);
        }
    }
    let mut dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    for _ in 0..4 {
        let name = dir.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if !matches!(name.as_str(), "bin" | "binaries" | "win64" | "win32" | "x64" | "x86" | "game" | "retail") {
            break;
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => break,
        }
    }
    dir
}

fn dlss_in(root: &std::path::Path) -> bool {
    let mut stack = vec![(root.to_path_buf(), 0)];
    let mut seen = 0;
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 40_000 {
                return false;
            }
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() && depth < 7 {
                stack.push((e.path(), depth + 1));
            } else if t.is_file() && e.file_name().to_string_lossy().eq_ignore_ascii_case("nvngx_dlss.dll") {
                return true;
            }
        }
    }
    false
}

/// O que o medidor recebeu com os controles abertos.
pub enum Input {
    Move(f32, f32),
    Click(f32, f32),
    Key(u32),
    Lost,
}

/// Uma linha de controles do medidor completo.
#[derive(Clone, Copy, PartialEq)]
pub enum Row {
    Mode,
    Fps,
    Real,
    Method,
    Preset,
    Power,
}

unsafe extern "system" fn meter_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let open = CONTROLS.load(Relaxed);
    let point = || {
        let scale = GetDpiForWindow(h) as f32 / 96.0;
        ((l.0 & 0xffff) as i16 as f32 / scale, ((l.0 >> 16) & 0xffff) as i16 as f32 / scale)
    };
    match m {
        WM_NCHITTEST => LRESULT(if open { HTCLIENT as isize } else { HTTRANSPARENT as isize }),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_METER_KEY if open => {
            crate::app::with(|a| a.meter_input(Input::Key(w.0 as u32)));
            LRESULT(0)
        }
        WM_MOUSEMOVE if open => {
            let (x, y) = point();
            crate::app::with(|a| a.meter_input(Input::Move(x, y)));
            LRESULT(0)
        }
        WM_LBUTTONUP if open => {
            let (x, y) = point();
            crate::app::with(|a| a.meter_input(Input::Click(x, y)));
            LRESULT(0)
        }
        WM_KEYDOWN if open => {
            crate::app::with(|a| a.meter_input(Input::Key(w.0 as u32)));
            LRESULT(0)
        }
        WM_ACTIVATE if open && w.0 & 0xffff == 0 => {
            crate::app::with(|a| a.meter_input(Input::Lost));
            LRESULT(0)
        }
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

fn clock(secs: u32) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
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

/// `pid` foi aberto por `parent`? Só um filho do jogo pode responder pelos quadros dele:
/// qualquer outro programa desenhando na tela daria um número que parece certo.
fn child_of(pid: u32, parent: u32) -> bool {
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return false };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snap, &mut e).is_ok();
        let mut found = false;
        while more && !found {
            found = e.th32ProcessID == pid && e.th32ParentProcessID == parent;
            more = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
        found
    }
}

/// Sinais de anti-cheat na pasta do jogo. Achar nada não prova que não tem.
pub fn anti_cheat(path: &str) -> bool {
    let Some(dir) = std::path::Path::new(path).parent() else { return false };
    const NAMES: [&str; 13] = ["easyanticheat", "battleye", "start_protected_game.exe", "beclient", "vgc", "eac", "xigncode", "gameguard", "nprotect", "punkbuster", "pnkbstr", "equ8", "mhyprot"];
    const ANYWHERE: [&str; 2] = ["anticheat", "anti-cheat"];
    // O VAC e outros não deixam arquivo próprio: esses jogos só dá para reconhecer pela pasta.
    const TITLES: [&str; 16] = ["counter-strike", "dota 2", "team fortress", "left 4 dead", "apex legends", "call of duty", "destiny 2", "pubgbattlegrounds", "rainbow six", "overwatch", "the finals", "delta force", "marvel rivals", "war thunder", "escape from tarkov", "dead by daylight"];
    let low = path.to_ascii_lowercase();
    if TITLES.iter().any(|t| low.contains(t)) {
        return true;
    }
    let check = |d: &std::path::Path| {
        std::fs::read_dir(d).is_ok_and(|rd| rd.flatten().any(|e| {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            NAMES.iter().any(|k| n.starts_with(k)) || ANYWHERE.iter().any(|k| n.contains(k))
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
        _ if uses_injection(g) => return None,
        _ => g.limit_fps,
    };
    Some(if g.limit_real { value } else { value / multiplier.max(1) })
}

pub fn uses_injection(g: &MeterGame) -> bool {
    g.inject && !g.inject_off
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
            live: false,
            by_flip: false,
            silent: 0,
            restarted: None,
            limit_write: None,
            log: None,
            saved: None,
            full_w: FULL_W,
            limiter: None,
            limiter_exe: String::new(),
            back: HWND::default(),
            row: 0,
            rows: Vec::new(),
            hover: None,
            hits: Vec::new(),
            chosen: Vec::new(),
            typing: None,
            plans: Vec::new(),
            plan_active: String::new(),
            plans_at: None,
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
            // Esconder recomeça a medição: FPS, médias e 1% low voltam do zero.
            self.stats = Stats::default();
            self.live = false;
            if let Some(t) = &mut self.tracker {
                t.reset();
            }
            if self.session.exe.is_empty() {
                self.tracker = None;
                monitor::METER.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            return;
        }
        monitor::METER.store(true, std::sync::atomic::Ordering::Relaxed);
        self.restart_if_none();
    }

    fn restart_if_none(&mut self) {
        if self.tracker.is_none() {
            self.tracker = Tracker::start();
            self.restarted = Some(Instant::now());
            self.target = 0;
            self.silent = 0;
        }
    }

    /// A sessão de eventos caiu, ou não entrega nada com o jogo em foco: religa em vez de deixar o número parado.
    fn keep_tracking(&mut self, silent: bool) {
        let dead = self.tracker.as_ref().is_some_and(|t| !t.alive());
        self.silent = if silent { self.silent + 1 } else { 0 };
        if dead || self.silent >= SILENT_SECS {
            crate::config::log(&format!("medidor: religou o rastreamento, sessão {}", if dead { "encerrada" } else { "sem eventos" }));
            self.tracker = None;
            self.stats = Stats::default();
            self.silent = 0;
        }
        let waited = self.restarted.map_or(true, |t| t.elapsed().as_secs() >= RETRY_SECS);
        if (self.view != VIEW_OFF || self.log.is_some()) && waited {
            self.restart_if_none();
        }
    }

    /// Liga ou desliga. Devolve true quando começou a gravar.
    pub fn toggle_log(&mut self, cfg: &Config, snap: &monitor::Snapshot) -> bool {
        if self.log.is_some() {
            self.stop_log(cfg);
            return false;
        }
        self.start_log(cfg, snap)
    }

    fn start_log(&mut self, cfg: &Config, snap: &monitor::Snapshot) -> bool {
        let gp = &snap.gpu;
        let gpu = if gp.total > 0 {
            format!(
                "GPU: driver {}, VRAM {:.1} GB, max {} MHz core / {} MHz mem, power limit {:.0} W, PCIe Gen{} x{}\n",
                snap.gpu_driver,
                gp.total as f32 / (1u64 << 30) as f32,
                gp.max_mhz,
                gp.max_mem_mhz,
                gp.power_limit,
                gp.pcie_gen,
                gp.pcie_width
            )
        } else {
            String::new()
        };
        let head = format!("{}\n{}\n{}\n{gpu}", self.session.exe, crate::sys::tools::now_stamp(), crate::report::machine(snap, 1.0));
        let limit = (cfg.meter_log_min > 0).then(|| cfg.meter_log_min * 60);
        self.log = Log::start(&self.session.exe, head, !self.session.anti_cheat, limit);
        if self.log.is_none() {
            return false;
        }
        monitor::METER.store(true, std::sync::atomic::Ordering::Relaxed);
        crate::sys::nvml::PCIE_WANTED.store(true, Relaxed);
        self.restart_if_none();
        self.set_tag("Registro ligado".to_string());
        true
    }

    /// O que só se sabe depois de o jogo rodar um pouco vai para o fim do resumo.
    fn stop_log(&mut self, cfg: &Config) {
        let Some(log) = self.log.take() else { return };
        crate::sys::nvml::PCIE_WANTED.store(false, Relaxed);
        if let Some(t) = &mut self.tracker {
            t.set_recording(false);
        }
        let found = self.found.lock().unwrap().clone();
        let game = entry(cfg, &self.session.exe);
        let limit = self.session.opened.map_or("—".to_string(), |v| format!("{v} FPS"));
        let mut tail = format!("{}: {limit}\n", crate::i18n::tr("Limite ativo"));
        tail += &format!("Upscaling: {} {}\n", found.upscaler, found.version);
        tail += &format!("{}: {}\n", crate::i18n::tr("Geração de quadros"), found.frame_gen);
        tail += &format!("Reflex: {}\n", found.reflex);
        tail += &format!("{}: {}\n", crate::i18n::tr("Perfil DLSS"), game.map_or(0, |g| g.preset));
        tail += &format!("Anti-cheat: {}\n", self.session.anti_cheat);
        self.saved = Some(log.finish(tail));
        self.set_tag("Registro salvo".to_string());
    }

    /// Atalhos: mostrar um esconde o outro.
    pub fn show(&mut self, view: u8) {
        self.set_controls(false);
        self.hidden = view == VIEW_OFF;
        self.apply_view(view);
    }

    pub fn interacting(&self) -> bool {
        CONTROLS.load(Relaxed)
    }

    /// Abre ou fecha os controles. O jogo continua em foco: as teclas dos controles chegam por
    /// um gancho, e o clique no painel não tira o foco dele.
    pub fn set_controls(&mut self, on: bool) {
        if on == self.interacting() {
            return;
        }
        CONTROLS.store(on, Relaxed);
        self.typing = None;
        let h = self.full.hwnd;
        FULL_HWND.store(h.0 as isize, Relaxed);
        unsafe {
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
            if on {
                self.back = GetForegroundWindow();
                self.row = 0;
                SetWindowLongPtrW(h, GWL_EXSTYLE, ex & !(WS_EX_TRANSPARENT.0 as isize));
                let mut hook = HOOK.lock().unwrap();
                if hook.is_none() {
                    *hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(controls_hook), None, 0).ok().map(|h| h.0 as isize);
                }
            } else {
                self.hover = None;
                SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_TRANSPARENT.0 as isize);
                if let Some(hk) = HOOK.lock().unwrap().take() {
                    let _ = UnhookWindowsHookEx(HHOOK(hk as *mut _));
                }
            }
        }
    }

    /// Devolve a opção que deve ser aplicada, se houver.
    pub fn input(&mut self, input: Input) -> Option<(Row, usize)> {
        let at = |list: &[(Rect, Row, usize)], x: f32, y: f32| list.iter().find(|h| x >= h.0.x && x < h.0.right() && y >= h.0.y && y < h.0.bottom()).map(|h| (h.1, h.2));
        match input {
            Input::Lost => self.set_controls(false),
            Input::Move(x, y) => self.hover = at(&self.hits, x, y),
            Input::Click(x, y) => {
                self.typing = None;
                let hit = at(&self.hits, x, y);
                if hit == Some((Row::Fps, TYPE_LIMIT)) {
                    self.start_typing();
                    return None;
                }
                return hit;
            }
            Input::Key(key) => return self.key(key),
        }
        None
    }

    fn start_typing(&mut self) {
        if let Some(i) = self.rows.iter().position(|r| *r == Row::Fps) {
            self.row = i;
            self.typing.get_or_insert_with(String::new);
        }
    }

    fn key(&mut self, key: u32) -> Option<(Row, usize)> {
        const BACK: u32 = 0x08;
        const ESC: u32 = 0x1B;
        const ENTER: u32 = 0x0D;
        const LEFT: u32 = 0x25;
        const UP: u32 = 0x26;
        const RIGHT: u32 = 0x27;
        const DOWN: u32 = 0x28;
        // Números do teclado de cima e do teclado numérico.
        let digit = match key {
            0x30..=0x39 => Some(key - 0x30),
            0x60..=0x69 => Some(key - 0x60),
            _ => None,
        };
        if let Some(d) = digit {
            self.start_typing();
            if let Some(t) = self.typing.as_mut().filter(|t| t.len() < 4) {
                t.push(char::from_digit(d, 10)?);
            }
            return None;
        }
        if self.typing.is_some() {
            match key {
                BACK => {
                    self.typing.as_mut()?.pop();
                    return None;
                }
                ESC => {
                    self.typing = None;
                    return None;
                }
                ENTER => {
                    let v: usize = self.typing.take()?.parse().ok()?;
                    return Some((Row::Fps, v.max(TYPE_LIMIT + 1)));
                }
                _ => self.typing = None,
            }
        }
        let last = self.rows.len().saturating_sub(1);
        match key {
            ESC | ENTER => self.set_controls(false),
            UP => self.row = self.row.saturating_sub(1),
            DOWN => self.row = (self.row + 1).min(last),
            LEFT | RIGHT => {
                let row = *self.rows.get(self.row)?;
                let forward = key == RIGHT;
                if row == Row::Fps {
                    return Some((row, forward as usize));
                }
                let (selected, count) = self.selected(row)?;
                let next = if forward { (selected + 1).min(count - 1) } else { selected.saturating_sub(1) };
                return (next != selected).then_some((row, next));
            }
            _ => {}
        }
        None
    }

    fn selected(&self, row: Row) -> Option<(usize, usize)> {
        let count = self.hits.iter().filter(|h| h.1 == row).count();
        Some((self.chosen.iter().find(|c| c.0 == row)?.1, count))
    }

    /// Aplica uma opção dos controles ao jogo aberto.
    pub fn act(&mut self, cfg: &mut Config, row: Row, option: usize) {
        if row == Row::Power {
            let n = self.plans.len();
            if n > 0 {
                let at = self.plans.iter().position(|p| p.0.eq_ignore_ascii_case(&self.plan_active)).unwrap_or(0);
                let next = if option == 0 { (at + n - 1) % n } else { (at + 1) % n };
                let (id, name) = self.plans[next].clone();
                if power::set_active(&id) {
                    self.plan_active = id;
                    crate::config::log(&format!("medidor: plano de energia {name}"));
                }
            }
            return;
        }
        if row == Row::Fps {
            if option > TYPE_LIMIT {
                self.set_limit(cfg, option as u32);
            } else {
                self.change_limit(cfg, if option == 0 { -1 } else { 1 });
            }
            return;
        }
        let protected = self.session.anti_cheat;
        let Some(g) = cfg.meter_games.iter_mut().find(|g| g.exe.eq_ignore_ascii_case(&self.session.exe)) else { return };
        match row {
            Row::Mode => {
                g.limit_mode = [1, 2, 0][option];
                if g.limit_mode == 1 {
                    g.measured = false;
                }
            }
            Row::Real => g.limit_real = option == 0,
            Row::Method => {
                g.inject = option == 0 && !protected;
                g.inject_off = false;
                self.session.inject_tried = false;
                self.session.inject_note = (option == 0 && protected).then_some("Injeção bloqueada: anti-cheat detectado.");
            }
            Row::Preset => g.preset = option as u8,
            Row::Fps | Row::Power => {}
        }
        self.limit_write = Some((g.exe.clone(), Instant::now()));
    }

    /// Quanto outra janela no canto superior direito precisa recuar para não cobrir o medidor.
    pub fn corner_gap(&self, cfg: &Config) -> (f32, f32) {
        match self.view {
            VIEW_COMPACT if cfg.meter_pos_compact == 2 => (EDGE, COMPACT_TOP + COMPACT_H + 10.0),
            VIEW_FULL if cfg.meter_pos_full == 0 => (EDGE + self.full_w + 12.0, EDGE),
            _ => (EDGE, EDGE),
        }
    }

    fn set_tag(&mut self, text: String) {
        self.tag = Some((text, Instant::now()));
    }

    /// Atalhos do limitador. Cada tecla só muda o valor: o driver é gravado depois, de uma vez.
    pub fn change_limit(&mut self, cfg: &mut Config, change: i32) -> String {
        self.apply_limit(cfg, |g| {
            if change == 0 {
                g.limit_mode = if g.limit_mode == 0 { 2 } else { 0 };
            } else {
                g.limit_mode = 2;
                g.limit_fps = (g.limit_fps as i32 + change).clamp(20, 1023) as u32;
            }
        })
    }

    /// Limite digitado no medidor completo.
    fn set_limit(&mut self, cfg: &mut Config, fps: u32) {
        self.apply_limit(cfg, |g| {
            g.limit_mode = 2;
            g.limit_fps = fps.clamp(20, 1023);
        });
    }

    fn apply_limit(&mut self, cfg: &mut Config, change: impl FnOnce(&mut MeterGame)) -> String {
        let Some(g) = cfg.meter_games.iter_mut().find(|g| g.exe.eq_ignore_ascii_case(&self.session.exe)) else { return String::new() };
        change(g);
        let text = if g.limit_mode == 0 { "Sem limite".to_string() } else { format!("Limite {}", g.limit_fps) };
        self.limit_write = Some((g.exe.clone(), Instant::now()));
        self.set_tag(text.clone());
        let value = if g.limit_mode == 0 { 0 } else { g.limit_fps };
        if let Some(l) = &mut self.limiter {
            l.update(value);
        }
        text
    }

    /// Carrega o limitador no jogo quando é seguro, passa o limite e vigia quedas.
    fn keep_limiter(&mut self, cfg: &mut Config, focused: bool, live: bool) {
        let code = self.limiter.as_ref().and_then(|l| l.exit_code());
        if let Some(code) = code {
            self.limiter = None;
            self.session.inject_tried = false;
            if inject::is_crash(code) {
                self.fall_back(cfg, "O jogo caiu. Limite vale ao reabrir.", &format!("o jogo caiu com {code:#x}"));
            }
        }
        let Some(g) = entry(cfg, &self.session.exe).cloned() else { return };
        if !uses_injection(&g) {
            self.limiter = None;
            return;
        }
        // O gancho vê cada quadro que vai à tela, gerado ou não.
        let screen = if g.limit_real { self.multiplier().unwrap_or(1) } else { 1 };
        let value = if g.limit_mode == 0 { 0 } else { g.limit_fps * screen };
        if self.session.anti_cheat {
            self.fall_back(cfg, "Anti-cheat: limite vale ao reabrir.", "anti-cheat detectado");
            return;
        }
        if self.limiter.is_none() && !self.session.inject_tried && focused && live && self.session.started.is_some_and(|t| t.elapsed().as_secs() >= INJECT_AFTER) && value > 0 {
            self.session.inject_tried = true;
            crate::config::log("medidor: carregando o limitador no jogo");
            self.limiter = Limiter::attach(self.target);
            self.limiter_exe = self.session.exe.clone();
            if self.limiter.is_none() {
                self.fall_back(cfg, "Injeção falhou. Limite vale ao reabrir.", "o módulo não carregou");
                return;
            }
        }
        if let Some(l) = &mut self.limiter {
            l.update(value);
            if l.status == Status::Failed {
                self.fall_back(cfg, "Injeção falhou. Limite vale ao reabrir.", "o limitador não engatou");
            } else if l.status == Status::Ready {
                self.session.inject_note = None;
            }
        }
    }

    fn save_limit(&mut self, cfg: &Config) {
        let ready = self.limit_write.as_ref().is_some_and(|w| w.1.elapsed().as_millis() >= 700);
        if !ready {
            return;
        }
        let Some((exe, _)) = self.limit_write.take() else { return };
        cfg.save();
        if entry(cfg, &exe).is_some_and(|g| !write_limit(g) || !write_preset(g)) {
            self.set_tag("Sem driver NVIDIA".to_string());
        }
    }

    /// Sem injeção neste jogo: o limite passa para o driver, que vale na próxima abertura.
    fn fall_back(&mut self, cfg: &mut Config, note: &'static str, why: &str) {
        self.limiter = None;
        // Depois de uma queda, a sessão já pode ser de outro programa.
        let exe = if self.limiter_exe.is_empty() { self.session.exe.clone() } else { std::mem::take(&mut self.limiter_exe) };
        self.session.inject_note = Some(note);
        crate::config::log(&format!("medidor: limite passou para o driver, {why}"));
        if let Some(g) = cfg.meter_games.iter_mut().find(|g| g.exe.eq_ignore_ascii_case(&exe)) {
            g.inject_off = true;
            write_limit(g);
        }
        cfg.save();
        self.set_tag("Limite pelo driver".to_string());
    }

    /// Chamado uma vez por segundo. Liga, desliga e mede conforme o jogo em foco.
    pub fn tick(&mut self, cfg: &mut Config, game: Option<(String, u32, String)>, fg_pid: u32, snap: &monitor::Snapshot) {
        self.save_limit(cfg);
        if self.interacting() && unsafe { GetForegroundWindow() } != self.back {
            self.set_controls(false);
        }
        if self.plans_at.map_or(true, |t| t.elapsed().as_secs() >= 5) && (self.view == VIEW_FULL || self.plans.is_empty()) {
            self.plans_at = Some(Instant::now());
            self.plans = power::list().into_iter().map(|p| (p.id, p.name)).collect();
            self.plan_active = power::active().unwrap_or_default();
        }
        match game {
            Some((exe, pid, path)) if self.session.exe != exe => self.enter(cfg, exe, pid, path, snap),
            // Mesmo jogo reaberto: é outro processo, e o antigo nunca mais manda quadros.
            Some((_, pid, _)) => self.session.pid = pid,
            None if !self.session.exe.is_empty() => self.leave(cfg),
            _ => {}
        }
        let focused = self.session.pid != 0 && fg_pid == self.session.pid;
        let (top, own) = self.tracker.as_ref().map_or((None, 0), |t| t.presenters(self.session.pid));
        self.keep_tracking(focused && top.is_none());
        let live = self.stats.frames > 0;
        self.keep_limiter(cfg, focused, live);
        let Some(t) = &mut self.tracker else { return };
        if self.view == VIEW_OFF {
            self.stats = t.stats();
        }
        if let Some(log) = &mut self.log {
            t.set_recording(true);
            log.second(gamelog::Second { stats: self.stats.clone(), snap: snap.clone(), frames: t.take_frames(), pid: self.target });
            log.seen += (self.stats.frames > 0) as u32;
            if log.limit.is_some_and(|l| log.seen >= l) {
                self.stop_log(cfg);
            }
        }
        if self.stats.by_flip && !self.by_flip {
            crate::config::log("medidor: contando pela troca de imagem");
        }
        self.by_flip |= self.stats.by_flip;
        if self.live && !live && focused {
            crate::config::log(&format!("medidor: quadros pararam, alvo {} jogo {} maior {top:?}", self.target, self.session.pid));
        }
        self.live = live;
        let game = self.session.pid;
        // Alguns jogos desenham num processo filho: vale quem está mandando quadros.
        let pid = match top {
            _ if game == 0 => fg_pid,
            _ if own > 0 => game,
            _ if live => self.target,
            Some((p, n)) if n > 15 && focused && child_of(p, game) => p,
            _ => game,
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

    /// O atalho chamou o medidor sobre este programa: ele vira o jogo da sessão.
    pub fn start(&mut self, cfg: &mut Config, exe: String, pid: u32, path: String, snap: &monitor::Snapshot) {
        if self.session.exe != exe {
            self.enter(cfg, exe, pid, path, snap);
        } else {
            self.session.pid = pid;
        }
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
        write_limit(g);
        cfg.save();
        self.set_tag(format!("Limite {value}"));
    }

    fn enter(&mut self, cfg: &mut Config, exe: String, pid: u32, path: String, snap: &monitor::Snapshot) {
        self.stop_log(cfg);
        self.session = Session { exe: exe.clone(), pid, anti_cheat: anti_cheat(&path), started: Some(Instant::now()), ..Default::default() };
        *self.found.lock().unwrap() = Found::default();
        self.scanned = None;
        self.hidden = false;
        let listed = entry(cfg, &exe).cloned();
        self.session.opened = listed.as_ref().and_then(|g| driver_limit(g, 1));
        self.session.opened_preset = listed.as_ref().map_or(0, |g| g.preset);
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
        // O medidor só aparece pelo atalho. Medir para o limitador automático não depende dele.
        if listed.as_ref().is_some_and(|g| g.limit_mode == 1 && !g.measured) {
            // Medir para o limitador não depende de o medidor estar visível.
            monitor::METER.store(true, std::sync::atomic::Ordering::Relaxed);
            if self.tracker.is_none() {
                self.tracker = Tracker::start();
                self.target = 0;
            }
        }
        if cfg.meter_log_auto {
            self.start_log(cfg, snap);
        }
    }

    /// Fim do jogo, ou do Prisma: devolve o HDR e fecha o rastreamento.
    pub fn leave(&mut self, cfg: &mut Config) {
        self.set_controls(false);
        self.stop_log(cfg);
        self.keep_limiter(cfg, false, false);
        self.limiter = None;
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
        self.live = false;
        self.by_flip = false;
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
        let tag = self.tag.as_ref().map(|t| t.0.clone()).or_else(|| self.log.as_ref().map(|l| format!("Registro {}", clock(l.seen)))).or_else(|| self.limit_tag(cfg));
        let w = if tag.is_some() { COMPACT_W + 104.0 } else { COMPACT_W };
        let x = match cfg.meter_pos_compact {
            0 => mon.left + (EDGE * scale) as i32,
            2 => mon.right - ((w + EDGE) * scale) as i32,
            _ => mon.left + ((mon.right - mon.left) as f32 - w * scale) as i32 / 2,
        };
        let y = mon.top + (COMPACT_TOP * scale) as i32;
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
            let ms_text = if s.frames > 0 { format!("{} ms", num(s.ms, 1)) } else { "— ms".to_string() };
            c.text(at(x, 44.0), &fps_text, 20.0, if s.spike { WARN } else { WHITE }, Align::Left, Weight::Semibold);
            c.text(at(x + 44.0, 28.0), "FPS", 12.0, TEXT2, Align::Left, Weight::Regular);
            x += 84.0;
            c.line(x, 10.0, x, 30.0, rgb(0x3a3f49), 1.0);
            x += 12.0;
            strip(c, rect(x, 8.0, 120.0, 24.0), &s);
            x += 128.0;
            let ms_color = if s.spike { WARN } else { TEXT };
            c.text(at(x, 60.0), &ms_text, 13.0, ms_color, Align::Left, Weight::Regular);
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
            let whole = |v: f32| if s.frames > 0 { format!("{v:.0}") } else { "—".to_string() };
            out.push(Line::Three([("Média", whole(s.avg)), ("1% low", whole(s.low1)), ("0,1% low", whole(s.low01))]));
        }
        if on(ITEM_FRAME) {
            let text = if s.frames > 0 { format!("{} ms · variação {} ms", num(s.ms, 1), num(s.dev, 1)) } else { "Sem quadros agora".to_string() };
            out.push(Line::Pair("Frame time".into(), text, if s.spike { WARN } else { TEXT }));
            out.push(Line::Strip);
        }
        if on(ITEM_LATENCY) {
            let text = s.latency.map_or("Só em jogos com Reflex".to_string(), |v| format!("{v:.0} ms"));
            out.push(Line::Pair("Latência do PC".into(), text, TEXT));
        }
        // Os botões ficam sempre à vista. O atalho só libera o mouse e o teclado para eles.
        let open = self.interacting();
        let playing = !self.session.exe.is_empty();
        if on(ITEM_LIMIT) || open || self.limit_write.is_some() {
            out.push(Line::Head("Limitador".into()));
            if playing {
                out.push(Line::Choice("Modo".into(), Row::Mode, ["Automático", "Manual", "Desligado"].iter().map(|s| s.to_string()).collect(), [2, 0, 1][game.map_or(0, |g| g.limit_mode.min(2)) as usize]));
                out.push(Line::Stepper("Limite".into(), Row::Fps, game.map_or(0, |g| g.limit_fps), self.typing.clone()));
                out.push(Line::Choice("Limitando".into(), Row::Real, vec!["FPS real".into(), "Com geração".into()], !game.map_or(true, |g| g.limit_real) as usize));
                out.push(Line::Choice("Método".into(), Row::Method, vec!["Injeção".into(), "Driver".into()], !game.map_or(true, uses_injection) as usize));
                if let Some(g) = game.filter(|g| g.limit_mode != 0) {
                    let now = if self.limiter.as_ref().is_some_and(|l| l.status == Status::Ready) {
                        format!("{} FPS · injeção", g.limit_fps)
                    } else if uses_injection(g) {
                        "Injeção, aguardando".to_string()
                    } else {
                        self.session.opened.map_or("Sem limite".to_string(), |v| format!("{v} FPS"))
                    };
                    out.push(Line::Pair("Ativo agora".into(), now, TEXT));
                }
            } else {
                out.push(Line::Pair("Modo".into(), "Desligado".to_string(), TEXT));
            }
            if let Some(note) = self.session.inject_note {
                out.push(Line::Note(note.to_string(), WARN));
            }
            if game.is_some_and(|g| g.limit_mode == 1 && !g.measured) {
                out.push(Line::Note(format!("Medindo há {} min de {AUTO_MINUTES}.", self.session.measured.len() / 60), TEXT2));
            }
            if playing && !game.is_some_and(uses_injection) && game.and_then(|g| driver_limit(g, 1)) != self.session.opened {
                out.push(Line::Note("O novo limite vale ao reabrir o jogo.".into(), ACCENT));
            }
            let hint = if self.typing.is_some() {
                "Enter aplica. Esc cancela.".to_string()
            } else if open {
                "Setas, mouse ou números. Esc fecha.".to_string()
            } else {
                format!("Controles: {}", cfg.hk_meter_keys.label())
            };
            out.push(Line::Note(hint, TEXT2));
        }
        if on(ITEM_UPSCALE) || open {
            out.push(Line::Head("Upscaling".into()));
            if found.upscaler.is_empty() {
                out.push(Line::Note("Nenhum carregado".into(), TEXT2));
            } else {
                out.push(Line::Pair(found.upscaler.clone(), format!("{} · carregado", found.version), TEXT));
            }
            let dlss = found.upscaler == "DLSS" || game.is_some_and(|g| has_dlss(&g.path) == Some(true));
            if playing && dlss {
                let preset = game.map_or(0, |g| g.preset);
                out.push(Line::Choice("Perfil DLSS".into(), Row::Preset, ["Do jogo", "Recomendado", "K", "L", "M"].iter().map(|s| s.to_string()).collect(), preset as usize));
                if preset != self.session.opened_preset {
                    out.push(Line::Note("O novo perfil vale ao reabrir o jogo.".into(), ACCENT));
                }
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
        if on(ITEM_POWER) || open {
            let name = self.plans.iter().find(|p| p.0.eq_ignore_ascii_case(&self.plan_active)).map_or("—".to_string(), |p| p.1.clone());
            out.push(Line::Cycle("Plano de energia".into(), Row::Power, name));
        }
        if on(ITEM_LOG) || self.log.is_some() {
            out.push(Line::Head("Registro de dados".into()));
            match &self.log {
                Some(l) => {
                    let time = l.limit.map_or(clock(l.seen), |v| format!("{} / {}", clock(l.seen), clock(v)));
                    out.push(Line::Pair("Gravando".into(), time, ACCENT));
                }
                None => {
                    let time = if cfg.meter_log_min == 0 { "Sem limite".to_string() } else { format!("{} min", cfg.meter_log_min) };
                    out.push(Line::Pair("Duração".into(), time, TEXT));
                }
            }
            out.push(Line::Note(format!("Ligar e desligar: {}", cfg.hk_log.label()), TEXT2));
        }
        if on(ITEM_CPU) {
            out.push(Line::Usage("CPU", snap.cpu));
            out.push(Line::Pair("Frequência".into(), format!("{} GHz", num(snap.ghz, 2)), TEXT));
            out.push(Line::Cores(snap.threads.clone()));
        }
        if on(ITEM_GPU) {
            let g = &snap.gpu;
            out.push(Line::Usage("GPU", g.util as f32));
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
        self.rows = lines.iter().filter_map(Line::row).collect();
        self.chosen = lines.iter().filter_map(Line::chosen).collect();
        let inner: f32 = lines.iter().map(Line::height).sum::<f32>() + 28.0;
        let two = inner > room;
        let (w, h) = if two { (FULL_W * 2.0 + 12.0, (inner / 2.0 + 60.0).min(room)) } else { (FULL_W, inner) };
        self.full_w = w;
        let x = if cfg.meter_pos_full == 1 { mon.left + (EDGE * scale) as i32 } else { mon.right - ((w + EDGE) * scale) as i32 };
        let y = mon.top + (EDGE * scale) as i32;
        let alpha = if self.interacting() { 0.97 } else { cfg.meter_op_full as f32 / 100.0 };
        let mut hits = Vec::new();
        let (focus, hover) = (self.interacting().then(|| self.rows.get(self.row).copied()).flatten(), self.hover);
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
                l.controls(c, cx, cy, width, focus, hover, &mut hits);
                cy += l.height();
            }
        });
        self.hits = hits;
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
    let y = |v: f32| {
        let d = v - s.ms;
        (mid - d.signum() * (d.abs() - STRIP_QUIET).max(0.0) / STRIP_RANGE * r.h / 2.0).clamp(r.y, r.bottom())
    };
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
    Choice(String, Row, Vec<String>, usize),
    Stepper(String, Row, u32, Option<String>),
    /// Nome e o valor atual entre as setas de anterior e próximo.
    Cycle(String, Row, String),
    /// Uso da CPU ou da GPU, em número grande para ler de longe.
    Usage(&'static str, f32),
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
            Line::Choice(..) | Line::Cycle(..) => 54.0,
            Line::Stepper(..) => 36.0,
            Line::Usage(..) => 48.0,
        }
    }

    fn row(&self) -> Option<Row> {
        match self {
            Line::Choice(_, row, ..) | Line::Stepper(_, row, ..) | Line::Cycle(_, row, _) => Some(*row),
            _ => None,
        }
    }

    fn chosen(&self) -> Option<(Row, usize)> {
        match self {
            Line::Choice(_, row, _, selected) => Some((*row, *selected)),
            _ => None,
        }
    }

    /// Botões da linha. Cada um guarda onde está, para o clique achar a opção.
    fn controls(&self, c: &Canvas, x: f32, y: f32, w: f32, focus: Option<Row>, hover: Option<(Row, usize)>, hits: &mut Vec<(Rect, Row, usize)>) {
        let chip = |c: &Canvas, r: Rect, text: &str, on: bool, row: Row, i: usize, hits: &mut Vec<(Rect, Row, usize)>| {
            let hot = hover == Some((row, i));
            c.round(r, 6.0, if on { ACCENT } else if hot { CARD2 } else { rgb(0x1b1d22) });
            if !on {
                c.round_line(r, 6.0, rgb(0x5a6170), 1.0);
            }
            c.text(r, text, 12.0, if on { rgb(0x10202c) } else { TEXT }, Align::Center, if on { Weight::Semibold } else { Weight::Regular });
            hits.push((r, row, i));
        };
        match self {
            Line::Choice(name, row, options, selected) => {
                c.text(rect(x, y, w, 20.0), name, 12.0, if focus == Some(*row) { ACCENT } else { TEXT2 }, Align::Left, Weight::Regular);
                let sizes: Vec<f32> = options.iter().map(|o| measure(o, 12.0, Weight::Semibold, None).0 + 16.0).collect();
                let extra = (w - sizes.iter().sum::<f32>() - 4.0 * (sizes.len() - 1) as f32).max(0.0) / sizes.len() as f32;
                let mut cx = x;
                for (i, o) in options.iter().enumerate() {
                    let r = rect(cx, y + 21.0, sizes[i] + extra, 26.0);
                    chip(c, r, o, i == *selected, *row, i, hits);
                    cx = r.right() + 4.0;
                }
            }
            Line::Cycle(name, row, value) => {
                c.text(rect(x, y, w, 20.0), name, 12.0, if focus == Some(*row) { ACCENT } else { TEXT2 }, Align::Left, Weight::Regular);
                chip(c, rect(x, y + 21.0, 34.0, 26.0), "‹", false, *row, 0, hits);
                let field = rect(x + 38.0, y + 21.0, w - 76.0, 26.0);
                c.round(field, 6.0, rgb(0x1b1d22));
                c.round_line(field, 6.0, rgb(0x3a3f49), 1.0);
                c.text_raw(field, value, 12.0, WHITE, Align::Center, Weight::Semibold);
                chip(c, rect(x + w - 34.0, y + 21.0, 34.0, 26.0), "›", false, *row, 1, hits);
            }
            Line::Stepper(name, row, value, typing) => {
                c.text(rect(x, y + 3.0, w * 0.4, 24.0), name, 12.0, if focus == Some(*row) { ACCENT } else { TEXT2 }, Align::Left, Weight::Regular);
                let right = x + w;
                chip(c, rect(right - 148.0, y + 3.0, 34.0, 26.0), "−", false, *row, 0, hits);
                let field = rect(right - 110.0, y + 3.0, 72.0, 26.0);
                let shown = match typing {
                    Some(t) => format!("{t}|"),
                    None if *value > 0 => value.to_string(),
                    None => "—".to_string(),
                };
                let hot = typing.is_some() || hover == Some((*row, TYPE_LIMIT));
                c.round(field, 6.0, rgb(0x1b1d22));
                c.round_line(field, 6.0, if hot { ACCENT } else { rgb(0x3a3f49) }, 1.0);
                c.text(field, &shown, 15.0, if typing.is_some() { ACCENT } else { WHITE }, Align::Center, Weight::Semibold);
                hits.push((field, *row, TYPE_LIMIT));
                chip(c, rect(right - 34.0, y + 3.0, 34.0, 26.0), "+", false, *row, 1, hits);
            }
            _ => {}
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
                let shown = if *fps > 0.0 { format!("{fps:.0}") } else { "—".to_string() };
                c.text(rect(tx, y, 110.0, 56.0), &shown, 44.0, WHITE, Align::Left, Weight::Semibold);
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
            Line::Choice(..) | Line::Stepper(..) | Line::Cycle(..) => {}
            Line::Usage(name, pct) => {
                c.line(x, y + 6.0, x + w, y + 6.0, rgb(0x3a3f49), 1.0);
                c.text(rect(x, y + 10.0, w * 0.5, 36.0), name, 30.0, WHITE, Align::Left, Weight::Semibold);
                c.text(rect(x + w * 0.5, y + 10.0, w * 0.5, 36.0), &format!("{pct:.0}%"), 30.0, WHITE, Align::Right, Weight::Semibold);
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
