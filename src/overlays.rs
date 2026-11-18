//! Janelas flutuantes: widget, aviso rápido e pergunta de otimização.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::Config;
use crate::sys::monitor::Snapshot;
use crate::ui::gfx::*;
use crate::ui::layered::{create_overlay, Layered};
use crate::ui::{Ui, UiState};

const HOT: f32 = 90.0;

fn fmt_gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / (1u64 << 30) as f64).replace('.', crate::i18n::comma())
}

/// Itens que o widget pode mostrar, na ordem dos bits de `widget_items`.
pub const ITEMS: [&str; 9] = ["Uso da CPU", "Uso da GPU", "RAM", "VRAM", "Temperatura da CPU", "Temperatura da GPU", "Plano de energia", "HDR", "VRR"];
pub const ALL_ITEMS: u16 = (1 << ITEMS.len()) - 1;
const CPU_TEMP: usize = 4;
const GPU_TEMP: usize = 5;
const PLAN: usize = 6;
const HDR: usize = 7;
const VRR: usize = 8;

const fn has(items: u16, i: usize) -> bool {
    items & (1 << i) != 0
}

/// Valor do config.ini; sem nenhum item conhecido, volta ao padrão.
pub fn parse_items(v: &str) -> u16 {
    match v.trim().parse::<u16>() {
        Ok(m) if m & ALL_ITEMS != 0 => m & ALL_ITEMS,
        _ => ALL_ITEMS,
    }
}

/// Liga ou desliga um item. O último ligado não sai: o widget nunca fica vazio.
pub fn toggle_item(items: u16, i: usize) -> u16 {
    let n = (items ^ (1 << i)) & ALL_ITEMS;
    if n == 0 {
        items
    } else {
        n
    }
}

/// Leituras do widget que não vêm do monitor de desempenho.
#[derive(Clone, Default)]
pub struct Status {
    pub plan: String,
    pub hdr: Option<bool>,
    pub vrr: Option<bool>,
}

/// Um item na tela: os medidores têm `gauge` (0 a 100); os de estado, só texto.
struct Item {
    name: &'static str,
    gauge: Option<f32>,
    /// Número grande dos anéis e do instrumento.
    big: String,
    /// Valor da pílula.
    small: String,
    /// Valor das barras e das linhas de estado.
    detail: String,
}

fn on_off(v: Option<bool>) -> String {
    match v {
        Some(true) => "Ligado",
        Some(false) => "Desligado",
        None => "—",
    }
    .to_string()
}

fn items(s: &Snapshot, st: &Status, mask: u16) -> Vec<Item> {
    let pct = |name, v: f32, detail: String| Item { name, gauge: Some(v), big: format!("{v:.0}"), small: format!("{v:.0}%"), detail };
    let temp = |name, t: Option<f32>| {
        let big = t.map_or("—".to_string(), |t| format!("{t:.0}"));
        Item { name, gauge: Some(t.unwrap_or(0.0)), detail: t.map_or("—".to_string(), |t| format!("{t:.0} °C")), small: big.clone(), big }
    };
    let text = |name, v: String| Item { name, gauge: None, big: String::new(), small: v.clone(), detail: v };
    (0..ITEMS.len())
        .filter(|i| has(mask, *i))
        .map(|i| match i {
            0 => pct("CPU", s.cpu, format!("{:.0}%", s.cpu)),
            1 => pct("GPU", s.gpu.util, format!("{:.0}%", s.gpu.util)),
            2 => pct("RAM", s.ram_pct(), fmt_gb(s.ram_used)),
            3 => pct("VRAM", s.vram_pct(), fmt_gb(s.gpu.used)),
            CPU_TEMP => temp("CPU °C", s.cpu_temp),
            GPU_TEMP => temp("GPU °C", (s.gpu.temp > 0).then_some(s.gpu.temp as f32)),
            PLAN => text("Plano", if st.plan.is_empty() { "—".into() } else { st.plan.clone() }),
            HDR => text("HDR", on_off(st.hdr)),
            _ => text("VRR", on_off(st.vrr)),
        })
        .collect()
}

fn hot(v: f32) -> Color {
    if v >= HOT {
        WARN
    } else {
        WHITE
    }
}

pub struct Widget {
    pub surface: Layered,
    pub visible: bool,
    pub status: Status,
    status_at: Option<std::time::Instant>,
    status_items: u16,
    /// G-SYNC lido numa thread: 0 sem leitura, 1 desligado, 2 ligado.
    vrr: std::sync::Arc<std::sync::atomic::AtomicU8>,
    vrr_at: Option<std::time::Instant>,
    /// Filho da barra de tarefas (estilo Pílula com "Prender na barra").
    docked: bool,
    panel: Option<Panel>,
}

/// O widget preso não se arrasta: o clique abre o painel.
static DOCKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static PRESSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

unsafe extern "system" fn widget_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_NCHITTEST if DOCKED.load(std::sync::atomic::Ordering::Relaxed) => LRESULT(HTCLIENT as isize),
        WM_NCHITTEST => LRESULT(HTCAPTION as isize),
        // Clique inteiro no widget: um botão solto que começou fora (arrasto na barra) não abre nada.
        WM_LBUTTONDOWN => {
            PRESSED.store(true, std::sync::atomic::Ordering::Relaxed);
            LRESULT(0)
        }
        WM_LBUTTONUP if PRESSED.swap(false, std::sync::atomic::Ordering::Relaxed) => {
            crate::app::with(click);
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_EXITSIZEMOVE => {
            let mut r = RECT::default();
            let _ = GetWindowRect(h, &mut r);
            crate::app::with(|a| {
                a.cfg.widget_x = r.left;
                a.cfg.widget_y = r.top;
                a.cfg.save();
                // Pode ter ido para outra tela: HDR e VRR são lidos de novo, e o VRR da tela antiga sai.
                a.widget.status_at = None;
                a.widget.vrr_at = None;
                a.widget.vrr.store(0, std::sync::atomic::Ordering::Relaxed);
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

/// Nome GDI do monitor onde a janela está.
fn monitor_name(h: HWND) -> String {
    unsafe {
        let m = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let _ = GetMonitorInfoW(m, &mut mi as *mut _ as *mut MONITORINFO);
        String::from_utf16_lossy(&mi.szDevice[..mi.szDevice.iter().position(|c| *c == 0).unwrap_or(0)])
    }
}

impl Widget {
    pub fn new() -> Self {
        register(w!("PrismaWidget"), widget_proc);
        let hwnd = create_overlay(w!("PrismaWidget"), false);
        crate::capture::hide_from_capture(hwnd);
        Self {
            surface: Layered::new(hwnd),
            visible: false,
            status: Status::default(),
            status_at: None,
            status_items: 0,
            vrr: Default::default(),
            vrr_at: None,
            docked: false,
            panel: None,
        }
    }

    /// O Explorer reiniciou e levou a janela junto: cria outra.
    fn recreate(&mut self) {
        let hwnd = create_overlay(w!("PrismaWidget"), false);
        crate::capture::hide_from_capture(hwnd);
        self.surface = Layered::new(hwnd);
        self.docked = false;
        DOCKED.store(false, std::sync::atomic::Ordering::Relaxed);
        if self.visible {
            unsafe {
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
        }
    }

    /// Põe o widget dentro da barra (`Some`) ou de volta solto na tela (`None`).
    fn set_parent(&mut self, bar: Option<HWND>) {
        let h = self.surface.hwnd;
        let float = |vis: u32| unsafe {
            SetWindowLongPtrW(h, GWL_STYLE, (WS_POPUP.0 | vis) as isize);
            SetWindowLongPtrW(h, GWL_EXSTYLE, (WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0) as isize);
            let _ = SetParent(h, None);
            crate::capture::hide_from_capture(h);
        };
        unsafe {
            let vis = GetWindowLongPtrW(h, GWL_STYLE) as u32 & WS_VISIBLE.0;
            if let Some(b) = bar {
                // Preso, faz parte da barra, que aparece em capturas (Parsec, compartilhamento); fora delas saía
                // um retângulo preto. A afinidade só vale em janela de cima: troca antes de prender.
                let _ = SetWindowDisplayAffinity(h, WDA_NONE);
                SetWindowLongPtrW(h, GWL_STYLE, (WS_CHILD.0 | WS_CLIPSIBLINGS.0 | vis) as isize);
                SetWindowLongPtrW(h, GWL_EXSTYLE, (WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as isize);
                let _ = SetParent(h, Some(b));
                if ok_docked(h, bar) {
                    // SetParent junta as filas de entrada das duas threads; separadas, um travamento do Prisma não trava a barra.
                    let _ = windows::Win32::System::Threading::AttachThreadInput(
                        windows::Win32::System::Threading::GetCurrentThreadId(),
                        GetWindowThreadProcessId(b, None),
                        false,
                    );
                } else {
                    // Recusado (versão nova do Windows, por exemplo): continua solto.
                    float(vis);
                }
            } else {
                float(vis);
            }
            self.docked = ok_docked(h, bar);
            let z = if self.docked { HWND_TOP } else { HWND_TOPMOST };
            let _ = SetWindowPos(h, Some(z), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED);
        }
        DOCKED.store(self.docked, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set_visible(&mut self, on: bool) {
        self.visible = on;
        self.status_at = None;
        if !on {
            self.close_panel();
        }
        unsafe {
            let _ = ShowWindow(self.surface.hwnd, if on { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    /// Plano, HDR e VRR: só os itens ligados, no máximo a cada 5 s (VRR fora desta thread).
    pub fn refresh(&mut self, items: u16) {
        use std::sync::atomic::Ordering::Relaxed;
        self.status.vrr = match self.vrr.load(Relaxed) {
            1 => Some(false),
            2 => Some(true),
            _ => None,
        };
        if items == self.status_items && self.status_at.is_some_and(|t| t.elapsed().as_secs() < 5) {
            return;
        }
        self.status_at = Some(std::time::Instant::now());
        self.status_items = items;
        if has(items, PLAN) {
            let id = crate::sys::power::active();
            self.status.plan = crate::sys::power::list().into_iter().find(|p| Some(&p.id) == id.as_ref()).map(|p| p.name).unwrap_or_default();
        }
        if has(items, HDR) {
            self.status.hdr = crate::sys::hdr::hdr_on(&monitor_name(self.surface.hwnd));
        }
        // Carregar a NVAPI leva um tempo; outras placas nem tentam e ficam em —.
        if has(items, VRR) && self.vrr_at.is_none_or(|t| t.elapsed().as_secs() >= 5) && crate::sys::drs::driver_version().is_some() {
            self.vrr_at = Some(std::time::Instant::now());
            let (out, gdi) = (self.vrr.clone(), monitor_name(self.surface.hwnd));
            let _ = std::thread::Builder::new().name("vrr".into()).spawn(move || {
                out.store(crate::sys::drs::vrr_active(&gdi).map_or(0, |on| 1 + on as u8), Relaxed);
            });
        }
    }

    pub fn update(&mut self, cfg: &Config, s: &Snapshot) {
        use std::sync::atomic::Ordering::Relaxed;
        let mask = cfg.widget_items;
        let panel = self.panel.as_ref().is_some_and(|p| p.open);
        crate::sys::cputemp::WIDGET.store(self.visible && (panel || has(mask, CPU_TEMP)), Relaxed);
        crate::sys::monitor::GPU_TEMP.store(self.visible && (panel || has(mask, GPU_TEMP)), Relaxed);
        if !self.visible {
            return;
        }
        self.refresh(mask);
        let mut m = items(s, &self.status, mask);
        let style = cfg.widget_style;
        if unsafe { !IsWindow(Some(self.surface.hwnd)).as_bool() } {
            self.recreate();
        }
        let bar = if cfg.widget_dock && style == 0 { taskbar() } else { None };
        // Preso: tira itens do fim até caber no espaço livre, para não cobrir os ícones.
        if let Some(room) = bar.as_ref().and_then(|tb| tb.room(cfg.widget_dock_left)) {
            let k = unsafe { GetDpiForWindow(self.surface.hwnd) } as f32 / 96.0;
            while m.len() > 1 && size(style, &m).0 * k > room as f32 {
                m.pop();
            }
        }
        let (w, h) = size(style, &m);
        match &bar {
            // Explorer reiniciado traz outra barra: prende de novo.
            Some(tb) if !ok_docked(self.surface.hwnd, Some(tb.bar)) => self.set_parent(Some(tb.bar)),
            None if self.docked => self.set_parent(None),
            _ => {}
        }
        match bar.filter(|_| self.docked) {
            Some(tb) => {
                let k = unsafe { GetDpiForWindow(self.surface.hwnd) } as f32 / 96.0;
                let (pw, ph) = ((w * k).ceil() as i32, (h * k).ceil() as i32);
                let x = if cfg.widget_dock_left { tb.rect.left + tb.gap } else { tb.tray_left - tb.gap - pw };
                let mut p = POINT { x, y: tb.rect.top + (tb.rect.bottom - tb.rect.top - ph) / 2 };
                unsafe {
                    let _ = ScreenToClient(tb.bar, &mut p);
                }
                self.surface.paint(p.x, p.y, w, h, |c, r| draw(c, style, r, &m));
                unsafe {
                    // Acima da parte XAML da barra; assíncrono para uma barra travada não segurar o Prisma.
                    let _ = SetWindowPos(self.surface.hwnd, Some(HWND_TOP), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                }
            }
            None => {
                self.surface.paint(cfg.widget_x, cfg.widget_y, w, h, |c, r| draw(c, style, r, &m));
                unsafe {
                    let _ = SetWindowPos(
                        self.surface.hwnd,
                        Some(HWND_TOPMOST),
                        0,
                        0,
                        0,
                        0,
                        SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                    );
                }
            }
        }
        if !self.docked {
            self.close_panel();
        }
        if let Some(p) = self.panel.as_mut().filter(|p| p.open) {
            crate::sys::nets::sample(&mut p.nets);
            self.paint_panel(s);
        }
    }
}

fn ok_docked(h: HWND, bar: Option<HWND>) -> bool {
    bar.is_some_and(|b| unsafe { GetAncestor(h, GA_PARENT) } == b)
}

/// Barra de tarefas principal, deitada, e onde começa a área de notificação, em pixels de tela.
struct Taskbar {
    bar: HWND,
    rect: RECT,
    tray_left: i32,
    /// Começo do botão Iniciar, quando a barra o expõe como janela.
    start_left: Option<i32>,
    /// Folga entre o widget e o que está ao lado, em pixels.
    gap: i32,
}

impl Taskbar {
    /// Largura livre, em pixels, no lado escolhido. `None` quando não dá para medir.
    /// shortcut: supõe os ícones centralizados e simétricos em volta do meio; não mede com UI Automation.
    fn room(&self, left: bool) -> Option<i32> {
        let start = self.start_left?;
        if start - self.rect.left < 4 * self.gap {
            // Ícones à esquerda: o lado do relógio fica livre e o canto esquerdo não tem espaço.
            return if left { Some(0) } else { None };
        }
        let icons_right = self.rect.left + self.rect.right - start;
        Some(if left { start - self.rect.left - 2 * self.gap } else { self.tray_left - icons_right - 2 * self.gap })
    }
}

fn taskbar() -> Option<Taskbar> {
    unsafe {
        let bar = FindWindowW(w!("Shell_TrayWnd"), None).ok()?;
        let mut rect = RECT::default();
        GetWindowRect(bar, &mut rect).ok()?;
        if rect.right - rect.left <= rect.bottom - rect.top {
            return None;
        }
        let mut rt = RECT::default();
        let tray = FindWindowExW(Some(bar), None, w!("TrayNotifyWnd"), None).ok();
        let dpi = GetDpiForWindow(bar) as i32;
        let tray_left = match tray {
            Some(t) if GetWindowRect(t, &mut rt).is_ok() && rt.left > rect.left && rt.right > rt.left => rt.left,
            _ => rect.right - 160 * dpi / 96,
        };
        let mut rs = RECT::default();
        let start_left = FindWindowExW(Some(bar), None, w!("Start"), None)
            .ok()
            .filter(|s| GetWindowRect(*s, &mut rs).is_ok() && rs.right > rs.left)
            .map(|_| rs.left);
        Some(Taskbar { bar, rect, tray_left, start_left, gap: 12 * dpi / 96 })
    }
}

/// Clique no widget preso: painel detalhado, se ligado; senão, abre o Prisma no Monitor.
fn click(a: &mut crate::app::App) {
    if !a.widget.docked {
        return;
    }
    if a.cfg.widget_panel {
        let (w, s) = (&mut a.widget, &a.snap);
        w.toggle_panel(s);
    } else {
        a.page = crate::pages::Page::Monitor;
        a.show_main();
    }
}

// ---------- Painel detalhado ----------

struct Panel {
    surface: Layered,
    open: bool,
    shown_at: std::time::Instant,
    closed_at: Option<std::time::Instant>,
    nets: Vec<crate::sys::nets::Net>,
}

const WM_PANEL_CLOSE: u32 = WM_APP + 1;
const ANIM_MS: f32 = 180.0;
const PANEL_W: f32 = 340.0;

unsafe extern "system" fn panel_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_TIMER => {
            crate::app::with(|a| {
                let (wd, s) = (&mut a.widget, &a.snap);
                wd.paint_panel(s);
            });
            LRESULT(0)
        }
        // Fecha depois, fora desta mensagem: a ativação pode chegar com o estado em uso.
        WM_ACTIVATE if w.0 & 0xffff == WA_INACTIVE as usize => {
            let _ = PostMessageW(Some(h), WM_PANEL_CLOSE, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        WM_KEYDOWN if w.0 == VK_ESCAPE.0 as usize => {
            let _ = PostMessageW(Some(h), WM_PANEL_CLOSE, WPARAM(0), LPARAM(0));
            LRESULT(0)
        }
        WM_PANEL_CLOSE => {
            crate::app::with(|a| a.widget.close_panel());
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

impl Widget {
    fn toggle_panel(&mut self, s: &Snapshot) {
        if self.panel.is_none() {
            register(w!("PrismaPanel"), panel_proc);
            let h = create_overlay(w!("PrismaPanel"), false);
            unsafe {
                // Este recebe o foco: perder o foco fecha o painel.
                SetWindowLongPtrW(h, GWL_EXSTYLE, (WS_EX_LAYERED.0 | WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0) as isize);
            }
            crate::capture::hide_from_capture(h);
            self.panel = Some(Panel { surface: Layered::new(h), open: false, shown_at: std::time::Instant::now(), closed_at: None, nets: Vec::new() });
        }
        let Some(p) = self.panel.as_mut() else { return };
        // O clique no widget já fechou o painel (perdeu o foco): não reabre no mesmo clique.
        if p.open || p.closed_at.is_some_and(|t| t.elapsed().as_millis() < 300) {
            self.close_panel();
            return;
        }
        p.open = true;
        p.shown_at = std::time::Instant::now();
        p.nets = crate::sys::nets::list();
        crate::sys::nets::sample(&mut p.nets);
        crate::sys::monitor::PANEL.store(true, std::sync::atomic::Ordering::Relaxed);
        let h = p.surface.hwnd;
        self.paint_panel(s);
        unsafe {
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
            SetTimer(Some(h), 1, 15, None);
        }
    }

    fn close_panel(&mut self) {
        let Some(p) = self.panel.as_mut().filter(|p| p.open) else { return };
        p.open = false;
        p.closed_at = Some(std::time::Instant::now());
        crate::sys::monitor::PANEL.store(false, std::sync::atomic::Ordering::Relaxed);
        unsafe {
            let _ = KillTimer(Some(p.surface.hwnd), 1);
            let _ = ShowWindow(p.surface.hwnd, SW_HIDE);
        }
    }

    /// Desenha o painel acima do widget; nos primeiros quadros ele sobe e aparece.
    fn paint_panel(&mut self, s: &Snapshot) {
        let wh = self.surface.hwnd;
        let Some(p) = self.panel.as_mut().filter(|p| p.open) else { return };
        let t = (p.shown_at.elapsed().as_secs_f32() * 1000.0 / ANIM_MS).min(1.0);
        if t >= 1.0 {
            unsafe {
                let _ = KillTimer(Some(p.surface.hwnd), 1);
            }
        }
        let e = 1.0 - (1.0 - t).powi(3);
        let d = detail(s, &p.nets);
        let h = panel_h(&d);
        let mut wr = RECT::default();
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let k = unsafe {
            let _ = GetWindowRect(wh, &mut wr);
            let _ = GetMonitorInfoW(MonitorFromWindow(wh, MONITOR_DEFAULTTOPRIMARY), &mut mi);
            GetDpiForWindow(wh) as f32 / 96.0
        };
        let (pw, ph) = ((PANEL_W * k).ceil() as i32, (h * k).ceil() as i32);
        let bar_top = taskbar().map_or(wr.top, |tb| tb.rect.top);
        let m = mi.rcMonitor;
        let gap = (8.0 * k) as i32;
        let x = (wr.right - pw).min(m.right - pw - gap).max(m.left + gap);
        let y = bar_top - ph - (12.0 * k) as i32 + ((1.0 - e) * 24.0 * k) as i32;
        p.surface.alpha = (e * 255.0) as u8;
        p.surface.paint(x, y, PANEL_W, h, |c, r| draw_panel(c, r, &d));
    }
}

/// Um bloco do painel: título, valor em destaque, barra opcional e linhas de uma ou duas células.
struct Section {
    title: String,
    value: String,
    gauge: Option<f32>,
    lines: Vec<Vec<(String, String)>>,
}

fn cell(k: &str, v: String) -> (String, String) {
    (k.to_string(), v)
}

/// Mínimo, média e máximo do histórico do monitor (último minuto).
fn mma(h: &std::collections::VecDeque<f32>) -> String {
    if h.is_empty() {
        return "—".into();
    }
    let (lo, hi) = h.iter().fold((f32::MAX, 0.0f32), |(a, b), v| (a.min(*v), b.max(*v)));
    format!("{lo:.0}% / {:.0}% / {hi:.0}%", h.iter().sum::<f32>() / h.len() as f32)
}

fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86400, secs / 3600 % 24, secs / 60 % 60);
    if d > 0 {
        format!("{d} d {h} h {m} min")
    } else {
        format!("{h} h {m} min")
    }
}

fn detail(s: &Snapshot, nets: &[crate::sys::nets::Net]) -> Vec<Section> {
    use crate::pages::{fmt_mem, fmt_rate};
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};
    let mut pi = windows::Win32::System::ProcessStatus::PERFORMANCE_INFORMATION::default();
    let pi_ok = unsafe { windows::Win32::System::ProcessStatus::GetPerformanceInfo(&mut pi, std::mem::size_of_val(&pi) as u32) }.is_ok();
    let page = pi.PageSize as u64;
    let up = unsafe { windows::Win32::System::SystemInformation::GetTickCount64() } / 1000;
    let dash = || "—".to_string();
    let mut out = Vec::new();

    let mut cpu = vec![vec![
        cell("Velocidade", format!("{:.2} GHz", s.ghz).replace('.', crate::i18n::comma())),
        cell("Temperatura", s.cpu_temp.map_or_else(dash, |t| format!("{t:.0} °C"))),
    ]];
    if pi_ok {
        cpu.push(vec![cell("Processos", pi.ProcessCount.to_string()), cell("Threads", pi.ThreadCount.to_string())]);
        cpu.push(vec![cell("Handles", pi.HandleCount.to_string()), cell("Ligado há", uptime(up))]);
    }
    cpu.push(vec![cell("Mín / méd / máx (1 min)", mma(&s.hist_cpu))]);
    out.push(Section { title: "CPU".into(), value: format!("{:.0}%", s.cpu), gauge: Some(s.cpu), lines: cpu });

    let mut ram = vec![vec![cell("Em uso", format!("{} de {}", fmt_mem(s.ram_used), fmt_mem(s.ram_total)))]];
    ram.push(vec![cell("Disponível", fmt_mem(s.ram_total.saturating_sub(s.ram_used)))]);
    if pi_ok {
        ram.push(vec![cell("Memória comprometida", format!("{} de {}", fmt_mem(pi.CommitTotal as u64 * page), fmt_mem(pi.CommitLimit as u64 * page)))]);
    }
    ram.push(vec![cell("Mín / méd / máx (1 min)", mma(&s.hist_ram))]);
    out.push(Section { title: "RAM".into(), value: format!("{:.0}%", s.ram_pct()), gauge: Some(s.ram_pct()), lines: ram });

    let g = &s.gpu;
    let mut gpu = Vec::new();
    if !s.gpu_name.is_empty() {
        gpu.push(vec![cell(&s.gpu_name, String::new())]);
    }
    gpu.push(vec![
        cell("Temperatura", if g.temp > 0 { format!("{} °C", g.temp) } else { dash() }),
        cell("Ventoinha", if g.fan > 0 { format!("{}%", g.fan) } else { dash() }),
    ]);
    if g.total > 0 {
        gpu.push(vec![cell("Memória dedicada", format!("{} de {}", fmt_mem(g.used), fmt_mem(g.total)))]);
    }
    if g.mhz > 0 {
        gpu.push(vec![cell("Clock", format!("{} MHz", g.mhz)), cell("Memória", format!("{} MHz", g.mem_mhz))]);
    }
    if g.watts > 0.0 {
        gpu.push(vec![cell("Energia", format!("{:.0} W", g.watts)), cell("Limite", format!("{:.0} W", g.power_limit))]);
    }
    gpu.push(vec![cell("Mín / méd / máx (1 min)", mma(&s.hist_gpu))]);
    out.push(Section { title: "GPU".into(), value: format!("{:.0}%", g.util), gauge: Some(g.util), lines: gpu });

    let mut disks = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u8 {
        let letter = (b'A' + i) as char;
        let root = HSTRING::from(format!("{letter}:\\"));
        // Só discos fixos: leitor de cartão vazio e unidade de rede podem demorar.
        if mask & (1 << i) == 0 || unsafe { GetDriveTypeW(&root) } != 3 {
            continue;
        }
        let (mut free, mut total) = (0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(&root, None, Some(&mut total), Some(&mut free)) }.is_ok() && total > 0 {
            disks.push(vec![cell(&format!("{letter}:"), format!("{} livres de {}", fmt_mem(free), fmt_mem(total)))]);
        }
    }
    if !disks.is_empty() {
        out.push(Section { title: "Discos".into(), value: String::new(), gauge: None, lines: disks });
    }

    if let Some(n) = nets.iter().find(|n| n.up && !n.ip.is_empty()) {
        let lines = vec![vec![cell(&n.desc, String::new())], vec![cell("Recebido", fmt_mem(n.total_down)), cell("Enviado", fmt_mem(n.total_up))]];
        out.push(Section { title: "Rede".into(), value: format!("↓ {}  ↑ {}", fmt_rate(n.down_rate), fmt_rate(n.up_rate)), gauge: None, lines });
    }
    out
}

const LINE_H: f32 = 20.0;
const PAD: f32 = 14.0;
const SEP: f32 = 17.0;

fn section_h(sec: &Section) -> f32 {
    26.0 + if sec.gauge.is_some() { 10.0 } else { 0.0 } + sec.lines.len() as f32 * LINE_H
}

fn panel_h(d: &[Section]) -> f32 {
    PAD * 2.0 + d.iter().map(section_h).sum::<f32>() + d.len().saturating_sub(1) as f32 * SEP
}

fn draw_panel(c: &Canvas, r: Rect, d: &[Section]) {
    c.round(r, 12.0, Color { r: 0.11, g: 0.12, b: 0.15, a: 0.97 });
    c.round_line(r, 12.0, WHITE.alpha(0.1), 1.0);
    let w = r.w - PAD * 2.0;
    let mut y = PAD;
    for (i, sec) in d.iter().enumerate() {
        if i > 0 {
            c.fill(rect(PAD, y + SEP / 2.0, w, 1.0), WHITE.alpha(0.08));
            y += SEP;
        }
        let vc = sec.gauge.map_or(ACCENT, |v| if v >= HOT { WARN } else { ACCENT });
        c.text(rect(PAD, y, w, 24.0), &sec.title, 14.0, WHITE, Align::Left, Weight::Semibold);
        c.text(rect(PAD, y, w, 24.0), &sec.value, 14.0, vc, Align::Right, Weight::Semibold);
        y += 26.0;
        if let Some(v) = sec.gauge {
            c.round(rect(PAD, y, w, 4.0), 2.0, WHITE.alpha(0.12));
            c.round(rect(PAD, y, w * v.clamp(0.0, 100.0) / 100.0, 4.0), 2.0, vc);
            y += 10.0;
        }
        for line in &sec.lines {
            let gap = 16.0;
            let cw = (w - gap * (line.len() - 1) as f32) / line.len() as f32;
            for (j, (k, v)) in line.iter().enumerate() {
                let x = PAD + j as f32 * (cw + gap);
                c.text(rect(x, y, cw, LINE_H), k, 12.0, rgb(LABEL), Align::Left, Weight::Regular);
                c.text(rect(x, y, cw, LINE_H), v, 12.0, WHITE, Align::Right, Weight::Semibold);
            }
            y += LINE_H;
        }
    }
}

const PANEL: Color = Color { r: 0.05, g: 0.06, b: 0.08, a: 0.8 };
const LABEL: u32 = 0xc5cad6;

fn tw(s: &str, size: f32, weight: Weight) -> f32 {
    measure(s, size, weight, None).0
}

/// Largura de uma linha "nome ... valor" de 13 px, sem as margens.
fn row_w(m: &Item) -> f32 {
    tw(m.name, 13.0, Weight::Regular) + 12.0 + tw(&m.detail, 13.0, Weight::Semibold)
}

fn texts(m: &[Item]) -> impl Iterator<Item = &Item> {
    m.iter().filter(|i| i.gauge.is_none())
}

fn gauges(m: &[Item]) -> impl Iterator<Item = &Item> {
    m.iter().filter(|i| i.gauge.is_some())
}

fn rows_w(m: &[Item]) -> f32 {
    texts(m).map(row_w).fold(0.0, f32::max) + 28.0
}

fn ring_col(m: &[Item]) -> f32 {
    gauges(m).map(|i| tw(i.name, 12.0, Weight::Regular) + 12.0).fold(73.0, f32::max)
}

fn inst_col(m: &[Item]) -> f32 {
    gauges(m).map(|i| tw(i.name, 12.0, Weight::Semibold).max(tw(&i.big, 44.0, Weight::Regular)) + 10.0).fold(79.0, f32::max)
}

/// Tamanho do widget: cresce com os itens e com o texto, para nada sair cortado.
fn size(style: u8, m: &[Item]) -> (f32, f32) {
    let (ng, nt) = (gauges(m).count() as f32, texts(m).count() as f32);
    match style {
        0 => {
            let sum: f32 = m.iter().map(|i| tw(i.name, 13.0, Weight::Regular) + 5.0 + tw(&i.small, 13.0, Weight::Semibold) + 14.0).sum();
            (sum + 18.0, 36.0)
        }
        1 => (m.iter().map(row_w).fold(192.0, f32::max) + 28.0, 10.0 + ng * 33.0 + nt * 24.0 + 8.0),
        2 => {
            let top = if ng > 0.0 { 100.0 } else { 10.0 };
            ((12.0 + ng * ring_col(m)).max(rows_w(m)), top + nt * 22.0 + if nt > 0.0 { 8.0 } else { 0.0 })
        }
        _ => {
            let top = if ng > 0.0 { 70.0 } else { 0.0 };
            ((ng * inst_col(m)).max(rows_w(m) - 28.0), top + nt * 20.0)
        }
    }
}

fn draw(c: &Canvas, style: u8, r: Rect, m: &[Item]) {
    match style {
        0 => draw_pill(c, r, m),
        1 => draw_bars(c, r, m),
        2 => draw_rings(c, r, m),
        _ => draw_instrument(c, r, m),
    }
}

/// Linha de estado: nome à esquerda, valor à direita.
fn draw_row(c: &Canvas, x: f32, y: f32, w: f32, h: f32, m: &Item) {
    c.text(rect(x, y, w, h), m.name, 13.0, rgb(LABEL), Align::Left, Weight::Regular);
    c.text(rect(x, y, w, h), &m.detail, 13.0, WHITE, Align::Right, Weight::Semibold);
}

fn draw_pill(c: &Canvas, r: Rect, m: &[Item]) {
    c.round(r, r.h / 2.0, PANEL);
    let mut x = 16.0;
    for i in m {
        let nw = tw(i.name, 13.0, Weight::Regular) + 5.0;
        let vw = tw(&i.small, 13.0, Weight::Semibold);
        c.text(rect(x, 0.0, nw + 2.0, r.h), i.name, 13.0, rgb(LABEL), Align::Left, Weight::Regular);
        c.text(rect(x + nw, 0.0, vw + 2.0, r.h), &i.small, 13.0, i.gauge.map_or(WHITE, hot), Align::Left, Weight::Semibold);
        x += nw + vw + 14.0;
    }
}

fn draw_bars(c: &Canvas, r: Rect, m: &[Item]) {
    c.round(r, 12.0, PANEL);
    let w = r.w - 28.0;
    let mut y = 10.0;
    for i in m {
        draw_row(c, 14.0, y, w, 18.0, i);
        if let Some(v) = i.gauge {
            c.round(rect(14.0, y + 20.0, w, 3.0), 1.5, WHITE.alpha(0.16));
            c.round(rect(14.0, y + 20.0, w * v.clamp(0.0, 100.0) / 100.0, 3.0), 1.5, hot(v));
            y += 33.0;
        } else {
            y += 24.0;
        }
    }
}

fn draw_rings(c: &Canvas, r: Rect, m: &[Item]) {
    c.round(r, 16.0, PANEL);
    let col = ring_col(m);
    for (i, it) in gauges(m).enumerate() {
        let v = it.gauge.unwrap_or(0.0);
        let cx = 6.0 + col * (i as f32 + 0.5);
        c.circle_line(cx, 40.0, 23.0, WHITE.alpha(0.16), 5.0);
        c.arc(cx, 40.0, 23.0, v.clamp(0.0, 100.0) / 100.0, if v >= HOT { WARN } else { ACCENT }, 5.0);
        c.text(rect(cx - 23.0, 17.0, 46.0, 46.0), &it.big, 14.0, WHITE, Align::Center, Weight::Semibold);
        c.text(rect(cx - col / 2.0, 70.0, col, 20.0), it.name, 12.0, rgb(LABEL), Align::Center, Weight::Regular);
    }
    let mut y = if gauges(m).next().is_some() { 100.0 } else { 10.0 };
    for it in texts(m) {
        draw_row(c, 14.0, y, r.w - 28.0, 22.0, it);
        y += 22.0;
    }
}

fn draw_instrument(c: &Canvas, r: Rect, m: &[Item]) {
    let w = inst_col(m);
    let shadow = rgb(0).alpha(0.75);
    for (i, it) in gauges(m).enumerate() {
        let v = it.gauge.unwrap_or(0.0);
        let x = i as f32 * w;
        let color = if v >= HOT { rgb(0xffb000) } else { WHITE };
        c.text(rect(x + 1.0, 1.0, w, 18.0), it.name, 12.0, shadow, Align::Left, Weight::Semibold);
        c.text(rect(x, 0.0, w, 18.0), it.name, 12.0, WHITE, Align::Left, Weight::Semibold);
        c.text(rect(x + 1.0, 19.0, w, 50.0), &it.big, 44.0, shadow, Align::Left, Weight::Regular);
        c.text(rect(x, 18.0, w, 50.0), &it.big, 44.0, color, Align::Left, Weight::Regular);
    }
    let mut y = if gauges(m).next().is_some() { 70.0 } else { 0.0 };
    for it in texts(m) {
        let nw = tw(it.name, 13.0, Weight::Semibold) + 12.0;
        for (d, col) in [(1.0, shadow), (0.0, WHITE)] {
            c.text(rect(d, y + d, nw, 20.0), it.name, 13.0, col, Align::Left, Weight::Semibold);
            c.text(rect(nw + d, y + d, r.w - nw, 20.0), &it.detail, 13.0, col, Align::Left, Weight::Semibold);
        }
        y += 20.0;
    }
}

/// Desenha um estilo dentro de um cartão da tela Widget.
pub fn preview(c: &Canvas, style: u8, area: Rect, s: &Snapshot, st: &Status, mask: u16) {
    let m = items(s, st, mask);
    let (w, h) = size(style, &m);
    let k = (area.w / w).min(area.h / h).min(1.0);
    c.scale_at(k, area.x, area.y);
    draw(c, style, rect(0.0, 0.0, w, h), &m);
    c.translate(0.0, 0.0);
}

#[cfg(test)]
mod widget_tests {
    use super::*;

    #[test]
    fn taskbar_room_beside_icons() {
        // 4K a 200%: Iniciar em 1258, relógio em 3206 (medido na barra real).
        let tb = |start| Taskbar { bar: HWND::default(), rect: RECT { left: 0, top: 2064, right: 3840, bottom: 2160 }, tray_left: 3206, start_left: start, gap: 24 };
        assert_eq!(tb(Some(1258)).room(true), Some(1258 - 48));
        assert_eq!(tb(Some(1258)).room(false), Some(3206 - (3840 - 1258) - 48));
        // Ícones à esquerda: canto esquerdo sem espaço, lado do relógio sem limite.
        assert_eq!(tb(Some(0)).room(true), Some(0));
        assert_eq!(tb(Some(0)).room(false), None);
        assert_eq!(tb(None).room(true), None);
    }

    #[test]
    fn widget_items_choice_and_file() {
        // Padrão: os quatro de antes e os novos.
        assert_eq!(crate::config::Config::default().widget_items, ALL_ITEMS);
        assert_eq!(parse_items(""), ALL_ITEMS);
        assert_eq!(parse_items("abc"), ALL_ITEMS);
        assert_eq!(parse_items("0"), ALL_ITEMS);
        // O que foi gravado volta igual; bit que não existe sai.
        assert_eq!(parse_items(&(1u16 << PLAN | 1).to_string()), 1 << PLAN | 1);
        assert_eq!(parse_items(&(1u16 << 15 | 1 << HDR).to_string()), 1 << HDR);
        // Liga e desliga; o último não sai.
        let m = toggle_item(ALL_ITEMS, VRR);
        assert!(!has(m, VRR) && has(m, HDR));
        assert_eq!(toggle_item(m, VRR), ALL_ITEMS);
        assert_eq!(toggle_item(1 << GPU_TEMP, GPU_TEMP), 1 << GPU_TEMP);
        // Só os itens escolhidos aparecem, na ordem da lista.
        let s = Snapshot::default();
        let names: Vec<_> = items(&s, &Status::default(), 1 << 0 | 1 << CPU_TEMP | 1 << HDR).iter().map(|i| i.name).collect();
        assert_eq!(names, ["CPU", "CPU °C", "HDR"]);
        let none = items(&s, &Status::default(), 1 << GPU_TEMP | 1 << VRR);
        assert_eq!((none[0].big.as_str(), none[1].detail.as_str()), ("—", "—"));
    }
}

pub fn register(name: PCWSTR, proc_: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) {
    unsafe {
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc_),
            lpszClassName: name,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
    }
}

/// Monitor em que está a janela em foco, em pixels.
pub fn focus_monitor() -> RECT {
    unsafe {
        let m = MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(m, &mut mi);
        mi.rcMonitor
    }
}

pub struct Toast {
    surface: Layered,
    timer: usize,
}

unsafe extern "system" fn toast_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_TIMER {
        let _ = KillTimer(Some(h), 1);
        let _ = ShowWindow(h, SW_HIDE);
        return LRESULT(0);
    }
    DefWindowProcW(h, m, w, l)
}

impl Toast {
    pub fn new() -> Self {
        register(w!("PrismaToast"), toast_proc);
        let hwnd = create_overlay(w!("PrismaToast"), true);
        crate::capture::hide_from_capture(hwnd);
        Self { surface: Layered::new(hwnd), timer: 0 }
    }

    fn show(&mut self, w: f32, h: f32, bottom: f32, ms: u32, draw: impl FnOnce(&Canvas, Rect)) {
        let mon = focus_monitor();
        let hwnd = self.surface.hwnd;
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
        let x = (mon.left + mon.right) / 2 - (w * scale / 2.0) as i32;
        let y = mon.bottom - ((h + bottom) * scale) as i32;
        self.surface.paint(x, y, w, h, draw);
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            self.timer = SetTimer(Some(hwnd), 1, ms, None);
        }
    }

    pub fn volume(&mut self, name: &str, vol: f32, muted: bool) {
        let name = name.to_string();
        self.show(400.0, 62.0, 56.0, 1500, move |c, r| {
            c.round(r, 14.0, PANEL);
            c.text(rect(16.0, 10.0, 300.0, 22.0), &name, 14.0, WHITE, Align::Left, Weight::Regular);
            let t = if muted { "Mudo".to_string() } else { format!("{:.0}%", vol * 100.0) };
            c.text(rect(300.0, 10.0, 84.0, 22.0), &t, 14.0, WHITE, Align::Right, Weight::Semibold);
            c.round(rect(16.0, 42.0, 368.0, 4.0), 2.0, WHITE.alpha(0.18));
            let (fill, color) = if muted { (1.0, WARN.alpha(0.5)) } else { (vol, ACCENT) };
            c.round(rect(16.0, 42.0, 368.0 * fill, 4.0), 2.0, color);
        });
    }

    pub fn info(&mut self, text: &str) {
        self.info_at(text, 56.0);
    }

    /// Aviso a `bottom` DIPs do pé da tela, para não cobrir outra janela flutuante.
    pub fn info_at(&mut self, text: &str, bottom: f32) {
        self.info_for(text, bottom, 1500);
    }

    /// Aviso que fica `ms` na tela.
    pub fn info_for(&mut self, text: &str, bottom: f32, ms: u32) {
        crate::report::note(text);
        let text = text.to_string();
        let tw = measure(&text, 14.0, Weight::Regular, None).0 + 32.0;
        self.show(tw.max(120.0), 40.0, bottom, ms, move |c, r| {
            c.round(r, 10.0, PANEL);
            c.text(r, &text, 14.0, WHITE, Align::Center, Weight::Regular);
        });
    }
}

/// Pergunta com botões no canto da tela, sem tirar o foco do jogo: a de fechar programas e a de
/// quem pede para entrar no compartilhamento de tela.
pub struct Prompt {
    surface: Layered,
    ui: UiState,
    title: String,
    /// Uma linha por item; linha longa quebra, nada é cortado.
    body: Vec<String>,
    /// Texto e destaque de cada botão, da esquerda para a direita.
    buttons: Vec<(&'static str, bool)>,
    /// Distância do canto superior direito, para não cobrir o medidor.
    gap: (f32, f32),
    pub open: bool,
}

/// Sem resposta em 10 s, a pergunta de fechar programas some.
pub const PROMPT_MS: u32 = 10_000;
const PROMPT_W: (f32, f32) = (300.0, 440.0);

unsafe extern "system" fn prompt_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let pt = || {
        let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(h) as f32 / 96.0;
        ((l.0 & 0xffff) as i16 as f32 / scale, ((l.0 >> 16) & 0xffff) as i16 as f32 / scale)
    };
    match m {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP => {
            let (x, y) = pt();
            crate::app::with(|a| a.prompt_mouse(m, x, y));
            return LRESULT(0);
        }
        WM_TIMER => {
            let _ = KillTimer(Some(h), 1);
            crate::app::with(|a| a.prompt_hide());
            return LRESULT(0);
        }
        _ => {}
    }
    DefWindowProcW(h, m, w, l)
}

impl Prompt {
    pub fn new() -> Self {
        register(w!("PrismaPrompt"), prompt_proc);
        let hwnd = create_overlay(w!("PrismaPrompt"), false);
        Self { surface: Layered::new(hwnd), ui: UiState::default(), title: String::new(), body: Vec::new(), buttons: Vec::new(), gap: (24.0, 24.0), open: false }
    }

    /// `limit`: some sozinha depois desses ms (`prompt_hide`); `None` fica até a resposta.
    /// `captured`: aparece em capturas (Parsec, compartilhamento), para quem usa este PC de longe poder
    /// responder; sem ele, fica fora de fotos e vídeos.
    pub fn ask(&mut self, title: String, body: Vec<String>, buttons: &[(&'static str, bool)], gap: (f32, f32), limit: Option<u32>, captured: bool) {
        self.title = title;
        self.body = body;
        self.buttons = buttons.to_vec();
        self.gap = gap;
        self.open = true;
        self.ui = UiState::default();
        self.redraw();
        unsafe {
            let _ = SetWindowDisplayAffinity(self.surface.hwnd, if captured { WDA_NONE } else { WDA_EXCLUDEFROMCAPTURE });
            let _ = KillTimer(Some(self.surface.hwnd), 1);
            let _ = ShowWindow(self.surface.hwnd, SW_SHOWNOACTIVATE);
            if let Some(ms) = limit {
                SetTimer(Some(self.surface.hwnd), 1, ms, None);
            }
        }
    }

    pub fn hide(&mut self) {
        self.open = false;
        unsafe {
            let _ = KillTimer(Some(self.surface.hwnd), 1);
            let _ = ShowWindow(self.surface.hwnd, SW_HIDE);
        }
    }

    /// O botão clicado, pela posição em `buttons`.
    pub fn mouse(&mut self, msg: u32, x: f32, y: f32) -> Option<usize> {
        self.ui.mx = x;
        self.ui.my = y;
        match msg {
            WM_LBUTTONDOWN => {
                self.ui.down = true;
                self.ui.pressed = true;
                unsafe { SetCapture(self.surface.hwnd) };
            }
            WM_LBUTTONUP => {
                self.ui.down = false;
                self.ui.released = true;
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            _ => {}
        }
        self.redraw()
    }

    fn redraw(&mut self) -> Option<usize> {
        const PAD: f32 = 14.0;
        let mon = focus_monitor();
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(self.surface.hwnd) } as f32 / 96.0;
        // Botões do tamanho do texto no idioma atual; a caixa cresce até PROMPT_W.1 e o texto quebra.
        let widths: Vec<f32> = self.buttons.iter().map(|(t, _)| (measure(t, 13.0, Weight::Semibold, None).0 + 28.0).max(84.0)).collect();
        let row = widths.iter().sum::<f32>() + 10.0 * widths.len().saturating_sub(1) as f32;
        let widest = std::iter::once(measure(&self.title, 14.0, Weight::Semibold, None).0).chain(self.body.iter().map(|l| measure(l, 13.0, Weight::Regular, None).0)).fold(row, f32::max);
        let w = (widest + 2.0 * PAD).clamp(PROMPT_W.0, PROMPT_W.1);
        let room = w - 2.0 * PAD;
        let th = measure(&self.title, 14.0, Weight::Semibold, Some(room)).1.max(22.0);
        let lines: Vec<f32> = self.body.iter().map(|l| measure(l, 13.0, Weight::Regular, Some(room)).1.max(18.0)).collect();
        let h = 10.0 + th + lines.iter().sum::<f32>() + 12.0 + 34.0 + 12.0;
        let (x, y) = (mon.right - ((w + self.gap.0) * scale) as i32, mon.top + (self.gap.1 * scale) as i32);
        let mut answer = None;
        let (title, body, buttons) = (&self.title, &self.body, &self.buttons);
        let ui = &mut self.ui;
        self.surface.paint(x, y, w, h, |c, r| {
            c.round(r, 14.0, rgb(0x14161c).alpha(0.96));
            let mut u = Ui::begin(c, ui);
            c.text_wrap(rect(PAD, 10.0, room, th), title, 14.0, WHITE, Weight::Semibold);
            let mut ty = 10.0 + th;
            for (l, lh) in body.iter().zip(&lines) {
                c.text_wrap(rect(PAD, ty, room, *lh), l, 13.0, rgb(0xd3d7df), Weight::Regular);
                ty += lh;
            }
            let mut bx = PAD;
            for (i, ((label, primary), bw)) in buttons.iter().zip(&widths).enumerate() {
                if u.button(&format!("p{i}"), rect(bx, h - 46.0, *bw, 34.0), label, *primary) {
                    answer = Some(i);
                }
                bx += bw + 10.0;
            }
            u.end();
        });
        answer
    }
}

/// Aviso de gravação no canto superior direito. Fica fora do vídeo.
pub struct Hud {
    surface: Layered,
}

const HUD_W: f32 = 430.0;
const HUD_H: f32 = 48.0;

unsafe extern "system" fn hud_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONUP => {
            let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(h) as f32 / 96.0;
            if (l.0 & 0xffff) as i16 as f32 / scale > HUD_W - 48.0 {
                crate::app::with(|a| a.hud_stop());
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

impl Hud {
    pub fn new() -> Self {
        register(w!("PrismaHud"), hud_proc);
        let hwnd = create_overlay(w!("PrismaHud"), false);
        crate::capture::hide_from_capture(hwnd);
        Self { surface: Layered::new(hwnd) }
    }

    pub fn hide(&mut self) {
        unsafe {
            let _ = ShowWindow(self.surface.hwnd, SW_HIDE);
        }
    }

    pub fn update(&mut self, secs: u64, info: &str, bytes: u64) {
        let mon = focus_monitor();
        let hwnd = self.surface.hwnd;
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
        let x = mon.right - ((HUD_W + 24.0) * scale) as i32;
        let y = mon.top + (24.0 * scale) as i32;
        let time = format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
        let line = if bytes > 0 { format!("{info} · {}", fmt_gb(bytes)) } else { info.to_string() };
        self.surface.paint(x, y, HUD_W, HUD_H, |c, r| {
            c.round(r, r.h / 2.0, rgb(0x0d0f14).alpha(0.86));
            c.circle(20.0, r.h / 2.0, 5.0, REC);
            c.text(rect(32.0, 0.0, 70.0, r.h), &time, 14.0, WHITE, Align::Left, Weight::Semibold);
            c.text(rect(106.0, 0.0, 276.0, r.h), &line, 13.0, rgb(0xc5cad6), Align::Left, Weight::Regular);
            let (bx, by) = (r.w - 42.0, 6.0);
            c.circle(bx + 18.0, by + 18.0, 18.0, CARD2);
            c.round(rect(bx + 12.0, by + 12.0, 12.0, 12.0), 2.0, WHITE);
        });
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }
}

/// Aviso de gravação de macro no alto da tela, no meio: o tempo e o último botão ou tecla; ao
/// parar, que a macro foi salva. Não pega o mouse nem o foco, e fica fora das capturas.
pub struct MacroHud {
    surface: Layered,
}

impl MacroHud {
    pub fn new() -> Self {
        register(w!("PrismaMacroHud"), toast_proc);
        let hwnd = create_overlay(w!("PrismaMacroHud"), true);
        crate::capture::hide_from_capture(hwnd);
        Self { surface: Layered::new(hwnd) }
    }

    /// `ms` > 0: some sozinho depois desse tempo.
    fn show(&mut self, w: f32, ms: u32, draw: impl FnOnce(&Canvas, Rect)) {
        let mon = focus_monitor();
        let hwnd = self.surface.hwnd;
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
        let x = (mon.left + mon.right) / 2 - (w * scale / 2.0) as i32;
        let y = mon.top + (24.0 * scale) as i32;
        self.surface.paint(x, y, w, 48.0, draw);
        unsafe {
            let _ = KillTimer(Some(hwnd), 1);
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            if ms > 0 {
                SetTimer(Some(hwnd), 1, ms, None);
            }
        }
    }

    /// Gravando: o tempo, e o último aperto (`icon`: o botão no desenho do controle usado).
    pub fn recording(&mut self, secs: u64, last: &str, icon: Option<(crate::ui::glyph::Style, crate::sys::pads::Buttons)>) {
        const TITLE: &str = "Gravando macro";
        let time = format!("{:02}:{:02}", secs / 60, secs % 60);
        let last = last.to_string();
        let tw = measure(TITLE, 13.0, Weight::Regular, None).0;
        let iw = icon.map_or(0.0, |(_, b)| crate::ui::glyph::width(b, 20.0) + 6.0);
        // Nome longo (outro idioma, tecla com modificadores): a caixa cresce até 640 e a letra encolhe.
        let lw = measure_raw(&last, 14.0, Weight::Semibold);
        let w = (84.0 + tw + 16.0 + iw + lw + 20.0).clamp(220.0, 640.0);
        self.show(w, 0, move |c, r| {
            c.round(r, r.h / 2.0, rgb(0x0d0f14).alpha(0.86));
            c.circle(20.0, r.h / 2.0, 5.0, REC);
            c.text_raw(rect(32.0, 0.0, 48.0, r.h), &time, 14.0, WHITE, Align::Left, Weight::Semibold);
            c.text(rect(84.0, 0.0, tw + 4.0, r.h), TITLE, 13.0, rgb(0xc5cad6), Align::Left, Weight::Regular);
            let mut x = 84.0 + tw + 16.0;
            if let Some((s, b)) = icon {
                x += crate::ui::glyph::draw(c, x, r.h / 2.0, 20.0, s, b, WHITE) + 6.0;
            }
            c.text_fit(rect(x, 0.0, r.w - x - 16.0, r.h), &last, 14.0, WHITE, Align::Left, Weight::Semibold);
        });
    }

    /// Fim da gravação: o texto fica 3 s.
    pub fn done(&mut self, text: &str) {
        crate::report::note(text);
        let text = crate::i18n::tr(text).into_owned();
        let w = (measure_raw(&text, 14.0, Weight::Regular) + 40.0).clamp(160.0, 640.0);
        self.show(w, 3000, move |c, r| {
            c.round(r, r.h / 2.0, rgb(0x0d0f14).alpha(0.86));
            c.text_fit(rect(20.0, 0.0, r.w - 40.0, r.h), &text, 14.0, WHITE, Align::Center, Weight::Regular);
        });
    }

    pub fn hide(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.surface.hwnd), 1);
            let _ = ShowWindow(self.surface.hwnd, SW_HIDE);
        }
    }
}

/// Teclado do Prisma para controle (`sys::osk`): só desenha, quem digita é o controle.
/// A seta anda 45% do caminho até o stick a cada quadro; devolve se mexeu o bastante para redesenhar.
fn ease(arrow: &mut (f32, f32), (tx, ty): (f32, f32)) -> bool {
    let (ax, ay) = *arrow;
    let (nx, ny) = (ax + (tx - ax) * 0.45, ay + (ty - ay) * 0.45);
    if (nx - ax).abs() + (ny - ay).abs() < 0.002 {
        return false;
    }
    *arrow = (nx, ny);
    true
}

/// Centro do círculo: disco, e um arco no aro que aponta para onde o stick vai, mais forte quanto
/// mais inclinado.
fn stick_hub(c: &Canvas, cx: f32, cy: f32, r: f32, (ax, ay): (f32, f32)) {
    let m = (ax * ax + ay * ay).sqrt().min(1.0);
    c.circle(cx, cy, r, WHITE.alpha(0.06));
    c.circle_line(cx, cy, r, WHITE.alpha(0.08), 1.0);
    if m > 0.08 {
        let a = ax.atan2(ay).to_degrees();
        c.arc_span(cx, cy, r - 3.0, a - 24.0, a + 24.0, ACCENT.alpha(0.25 + 0.75 * m), 3.0);
    }
}

/// Valor animado de `from` a `to` em `ms`, pelo relógio: o timer de 16 ms oscila, contar quadros
/// deixaria o passo irregular. Curva cúbica: suave na chegada, ou na saída para fechar (`ease_in`).
#[derive(Clone, Copy)]
struct Tw {
    from: f32,
    to: f32,
    t0: std::time::Instant,
    ms: f32,
    ease_in: bool,
}

impl Tw {
    fn new(v: f32) -> Self {
        Self { from: v, to: v, t0: std::time::Instant::now(), ms: 0.0, ease_in: false }
    }
    fn k(&self) -> f32 {
        let e = std::time::Instant::now().saturating_duration_since(self.t0).as_secs_f32() * 1000.0;
        if self.ms <= 0.0 { 1.0 } else { (e / self.ms).min(1.0) }
    }
    fn v(&self) -> f32 {
        let k = self.k();
        let e = if self.ease_in { k * k * k } else { 1.0 - (1.0 - k).powi(3) };
        self.from + (self.to - self.from) * e
    }
    /// Vai até `to` em `ms`, depois de `wait` ms, a partir de onde está agora.
    fn go(&mut self, to: f32, ms: f32, wait: f32, ease_in: bool) {
        if to != self.to {
            *self = Self { from: self.v(), to, t0: std::time::Instant::now() + std::time::Duration::from_secs_f32(wait / 1000.0), ms, ease_in };
        }
    }
    fn set(&mut self, to: f32, ms: f32) {
        self.go(to, ms, 0.0, false)
    }
    fn busy(&self) -> bool {
        self.k() < 1.0
    }
}

/// Ângulo de chegada pelo caminho mais curto, para o destaque girar pelo lado certo.
fn short_to(from: f32, to: f32) -> f32 {
    from + ((to - from) % 360.0 + 540.0) % 360.0 - 180.0
}

/// Destaque que gira até `ang` (em graus); aparece de onde já está se estava apagado.
fn turn_to(ang: &mut Tw, on: &mut Tw, to: f32) {
    if on.v() < 0.05 {
        *ang = Tw::new(to);
    } else {
        let a = ang.v();
        ang.set(short_to(a, to), 140.0);
    }
    on.set(1.0, 140.0);
}

/// Cascata ao abrir: cada item entra 8 ms depois do anterior, em 160 ms.
fn cascade(since_ms: f32, i: usize) -> f32 {
    let k = ((since_ms - i as f32 * 8.0) / 160.0).clamp(0.0, 1.0);
    1.0 - (1.0 - k).powi(3)
}

/// Margem da janela para a sombra em volta do painel.
const SHADOW: f32 = 18.0;

/// Painel das janelas do controle: sombra em anéis (sem desfoque, que o Direct2D em software
/// não faz a 60 quadros), fundo escuro translúcido e borda fina.
fn pad_panel(c: &Canvas, r: Rect) {
    // 3 anéis de 6 px: cada contorno custa no Direct2D em software, mais que a largura dele.
    for i in 0..3 {
        let g = 3.0 + i as f32 * 6.0;
        c.round_line(r.inset(-g), 22.0 + g, rgb(0).alpha(0.16 - 0.05 * i as f32), 6.0);
    }
    c.round(r, 22.0, rgb(0x0d0f14).alpha(0.88));
    c.round_line(r, 22.0, WHITE.alpha(0.09), 1.0);
}

pub struct Keyboard {
    surface: Layered,
    /// Sticks desenhados agora (esquerdo, direito): correm atrás dos analógicos a cada quadro, para
    /// andar suave.
    arrow: [(f32, f32); 2],
    /// Canto do painel na tela, as duas linhas de ajuda e os dois sticks, fixos enquanto o teclado
    /// fica aberto.
    at: (i32, i32),
    lines: [Vec<(Vec<crate::sys::pads::Buttons>, String)>; 2],
    style: crate::ui::glyph::Style,
    two: bool,
    ticking: bool,
    /// Fugir do cursor: altura em pixels embaixo e no alto da tela, e a de agora, que corre atrás
    /// da escolhida.
    rows: (i32, i32),
    y: f32,
    /// Abre (0 a 1) e fecha (1 a 0, com `closing`); o relógio segue até acabar de sumir.
    show: Tw,
    closing: bool,
    opened: std::time::Instant,
    /// Estado desenhado (fica o último enquanto fecha) e o de antes da troca de modo, para as
    /// letras trocarem em fade (`swap`).
    last: crate::sys::osk::View,
    prev: crate::sys::osk::View,
    swap: Tw,
    /// Destaque de cada círculo (esquerdo, direito): ângulo e quanto aparece.
    hl: [(Tw, Tw); 2],
    /// Letra digitada: pulso de 0 a 1.
    pulse: Tw,
}

/// Cursor de texto do programa em foco, na tela, lido por `watch_caret`.
static CARET: std::sync::Mutex<Option<RECT>> = std::sync::Mutex::new(None);
static WATCHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Cursor de texto do programa em foco: o do Windows (Bloco de Notas, campos clássicos) ou, sem
/// ele, o da acessibilidade (navegadores e a maioria dos programas atuais).
/// shortcut: programas que só têm UI Automation (alguns WinUI) não mostram o cursor e o teclado
/// fica embaixo; ler o TextPattern2 se faltar em algum programa usado.
fn caret() -> Option<RECT> {
    use windows::Win32::UI::Accessibility::{AccessibleObjectFromWindow, IAccessible};
    unsafe {
        let fg = GetForegroundWindow();
        let tid = GetWindowThreadProcessId(fg, None);
        let mut gi = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        if tid != 0 && GetGUIThreadInfo(tid, &mut gi).is_ok() {
            let r = gi.rcCaret;
            if !gi.hwndCaret.is_invalid() && r.bottom > r.top {
                let mut pts = [POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom }];
                MapWindowPoints(Some(gi.hwndCaret), None, &mut pts);
                return Some(RECT { left: pts[0].x, top: pts[0].y, right: pts[1].x, bottom: pts[1].y });
            }
        }
        let h = if gi.hwndFocus.is_invalid() { fg } else { gi.hwndFocus };
        let mut p: *mut core::ffi::c_void = std::ptr::null_mut();
        AccessibleObjectFromWindow(h, OBJID_CARET.0 as u32, &IAccessible::IID, &mut p).ok()?;
        let acc = IAccessible::from_raw(p);
        let (mut x, mut y, mut w, mut hh) = (0, 0, 0, 0);
        acc.accLocation(&mut x, &mut y, &mut w, &mut hh, &windows::Win32::System::Variant::VARIANT::from(0i32)).ok()?;
        (hh > 0 && (x, y) != (0, 0)).then_some(RECT { left: x, top: y, right: x + w.max(1), bottom: y + hh })
    }
}

/// Lê o cursor de texto 5 vezes por segundo enquanto o teclado está aberto, numa thread à parte:
/// um programa travado prende só ela.
fn watch_caret() {
    *CARET.lock().unwrap() = None;
    if WATCHING.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    std::thread::spawn(|| {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        while crate::sys::osk::view().open {
            let c = caret();
            *CARET.lock().unwrap() = c;
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        *CARET.lock().unwrap() = None;
        WATCHING.store(false, std::sync::atomic::Ordering::Relaxed);
    });
}

/// Sobe para o alto da tela se o cursor `c` está atrás do teclado embaixo (`x`, `bottom`, `w` × `h`).
fn dodge_up(c: RECT, x: i32, bottom: i32, w: i32, h: i32) -> bool {
    let m = 16;
    c.right > x - m && c.left < x + w + m && c.bottom > bottom - m && c.top < bottom + h + m
}

const KB_W: f32 = 520.0;
const KB_H: f32 = 392.0;
/// Altura, do pé da tela, de um aviso que não cobre o teclado aberto.
pub const KB_LIFT: f32 = KB_H + 60.0;
/// Quadro da seta, em ms (cerca de 60 por segundo, só com o teclado aberto).
const KB_FRAME: u32 = 16;

unsafe extern "system" fn keyboard_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_TIMER {
        crate::app::with(|a| a.keyboard_tick());
        return LRESULT(0);
    }
    DefWindowProcW(h, m, w, l)
}

/// Texto que troca: o velho sobe 3 px e some, o novo sobe de 3 px abaixo e aparece (`k` de 0 a 1).
fn swap_text(c: &Canvas, r: Rect, old: &str, new: &str, k: f32, size: f32, col: Color, w: Weight) {
    if k >= 1.0 || old == new {
        c.text_raw(r, new, size, col, Align::Center, w);
        return;
    }
    c.text_raw(rect(r.x, r.y - 3.0 * k, r.w, r.h), old, size, col.alpha(col.a * (1.0 - k)), Align::Center, w);
    c.text_raw(rect(r.x, r.y + 3.0 * (1.0 - k), r.w, r.h), new, size, col.alpha(col.a * k), Align::Center, w);
}

fn kb_mode(v: &crate::sys::osk::View) -> &'static str {
    match (v.sym, v.shift) {
        (true, true) => "áç",
        (true, false) => "123",
        (false, true) => "ABC",
        (false, false) => "abc",
    }
}

/// Escala da janela pelo DPI do monitor dela.
fn dpi_scale(hwnd: HWND) -> f32 {
    unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 }
}

impl Keyboard {
    pub fn new() -> Self {
        register(w!("PrismaKeyboard"), keyboard_proc);
        let hwnd = create_overlay(w!("PrismaKeyboard"), true);
        Self {
            surface: Layered::new(hwnd),
            arrow: [(0.0, 0.0); 2],
            at: (0, 0),
            lines: Default::default(),
            style: crate::ui::glyph::Style::Xbox,
            two: false,
            ticking: false,
            rows: (0, 0),
            y: 0.0,
            show: Tw::new(0.0),
            closing: false,
            opened: std::time::Instant::now(),
            last: Default::default(),
            prev: Default::default(),
            swap: Tw::new(1.0),
            hl: [(Tw::new(0.0), Tw::new(0.0)); 2],
            pulse: Tw::new(1.0),
        }
    }

    /// Mostra o estado de agora do teclado, ou começa a esconder se ele fechou. Devolve `Some`
    /// quando ele acabou de abrir (`true`) ou de fechar (`false`).
    pub fn update(&mut self) -> Option<bool> {
        use crate::sys::pads;
        let v = crate::sys::osk::view();
        let hwnd = self.surface.hwnd;
        if !v.open {
            if !self.ticking || self.closing {
                return None;
            }
            // Some em 140 ms; o `tick` esconde a janela no fim.
            self.closing = true;
            self.show.go(0.0, 140.0, 0.0, true);
            return Some(false);
        }
        // Reabrir enquanto some conta como abrir de novo.
        let opened = !self.ticking || self.closing;
        if opened {
            let mon = focus_monitor();
            let scale = dpi_scale(hwnd);
            self.two = v.two;
            self.at = ((mon.left + mon.right) / 2 - (self.width() * scale / 2.0) as i32, mon.bottom - ((KB_H + 48.0) * scale) as i32);
            self.rows = (self.at.1, mon.top + (48.0 * scale) as i32);
            self.y = self.at.1 as f32;
            if crate::sys::osk::dodge() {
                watch_caret();
            }
            // Sem controle na lista (ainda não leu nenhum), os ícones do Xbox.
            self.style = crate::ui::glyph::Style::current();
            let t = |s: &str| crate::i18n::tr(s).into_owned();
            let (lt, rt) = (crate::sys::remap::LT, crate::sys::remap::RT);
            self.lines = [
                vec![
                    (vec![pads::LB], t("apaga")),
                    (vec![pads::RB], t("espaço")),
                    (vec![pads::START], "Enter".into()),
                    (vec![crate::ui::glyph::DPAD_ANY], t(if self.two { "digita no círculo da direita; com o stick direito solto, setas" } else { "setas" })),
                ],
                if self.two {
                    vec![(vec![rt], t("maiúsculas")), (vec![lt], t("acentos")), (vec![pads::BACK], t("fecha"))]
                } else {
                    vec![(vec![rt], t("maiúsculas")), (vec![lt], t("números")), (vec![lt, rt], t("acentos")), (vec![pads::BACK], t("fecha"))]
                },
            ];
            self.arrow = crate::sys::osk::stick();
            self.closing = false;
            self.show = Tw::new(0.0);
            self.show.set(1.0, 180.0);
            self.opened = std::time::Instant::now();
            self.prev = v;
            self.swap = Tw::new(1.0);
            self.pulse = Tw::new(1.0);
            self.hl = [(Tw::new(0.0), Tw::new(0.0)); 2];
            unsafe {
                SetTimer(Some(hwnd), 1, KB_FRAME, None);
            }
            self.ticking = true;
        } else {
            let l = self.last;
            if (v.shift, v.sym, v.letters_right) != (l.shift, l.sym, l.letters_right) {
                self.prev = l;
                self.swap = Tw::new(0.0);
                self.swap.set(1.0, 140.0);
            }
            if v.hits != l.hits {
                self.pulse = Tw::new(0.0);
                self.pulse.set(1.0, 200.0);
            }
        }
        let before = if opened { [None, None] } else { [self.last.petal, self.last.right] };
        for (side, p) in [v.petal, v.right].into_iter().enumerate() {
            if p == before[side] {
                continue;
            }
            let (ang, on) = &mut self.hl[side];
            match p {
                Some(p) => turn_to(ang, on, p as f32 * 45.0),
                None => on.set(0.0, 120.0),
            }
        }
        self.last = v;
        self.paint();
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        opened.then_some(true)
    }

    /// Um quadro: cada stick anda 45% do caminho até o analógico, o teclado anda 35% do caminho até
    /// a altura escolhida (embaixo, ou no alto com o cursor de texto atrás dele), as animações
    /// seguem o relógio, e só redesenha se algo mexeu. No fim do fechar, esconde e para o relógio.
    pub fn tick(&mut self) {
        let hwnd = self.surface.hwnd;
        if self.closing && !self.show.busy() {
            unsafe {
                let _ = KillTimer(Some(hwnd), 1);
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            self.ticking = false;
            self.closing = false;
            return;
        }
        let [l, r] = crate::sys::osk::stick();
        let (a, b) = (ease(&mut self.arrow[0], l), ease(&mut self.arrow[1], r));
        let scale = dpi_scale(hwnd);
        let (w, h) = ((self.width() * scale) as i32, (KB_H * scale) as i32);
        let up = CARET.lock().unwrap().is_some_and(|c| dodge_up(c, self.at.0, self.rows.0, w, h));
        let goal = if up { self.rows.1 } else { self.rows.0 } as f32;
        let moved = (goal - self.y).abs() >= 1.0;
        if moved {
            self.y += (goal - self.y) * 0.35;
            if (goal - self.y).abs() < 2.0 {
                self.y = goal;
            }
            self.at.1 = self.y.round() as i32;
        }
        if a || b || moved || self.animating() {
            self.paint();
        }
    }

    fn animating(&self) -> bool {
        // 8 pétalas em cascata: a última entra aos 56 ms e leva 160.
        self.opened.elapsed().as_millis() < 240 || self.show.busy() || self.swap.busy() || self.pulse.busy() || self.hl.iter().any(|(a, o)| a.busy() || o.busy())
    }

    /// Com os dois sticks, os dois círculos lado a lado.
    fn width(&self) -> f32 {
        if self.two { KB_W * 2.0 } else { KB_W }
    }

    fn paint(&mut self) {
        let (v, pv) = (self.last, self.prev);
        let (arrow, two, style) = (self.arrow, self.two, self.style);
        let lines = &self.lines;
        let s = self.show.v();
        // Abre subindo 12 px; fecha descendo 8.
        let dy = if self.closing { 8.0 } else { 12.0 } * (1.0 - s);
        let since = self.opened.elapsed().as_secs_f32() * 1000.0;
        let (mk, pk) = (self.swap.v(), self.pulse.v());
        let hl = self.hl.map(|(a, o)| (a.v(), o.v()));
        let w = self.width();
        self.surface.alpha = (s.clamp(0.0, 1.0) * 255.0).round() as u8;
        let m = (SHADOW * dpi_scale(self.surface.hwnd)).round() as i32;
        self.surface.paint(self.at.0 - m, self.at.1 - m, w + 2.0 * SHADOW, KB_H + 2.0 * SHADOW, |c, _| {
            let (bx, by) = (SHADOW, SHADOW + dy);
            c.translate(bx, by);
            let r = rect(0.0, 0.0, w, KB_H);
            pad_panel(c, r);
            let (cy, ring, pr, hub) = (176.0, 116.0, 42.0, 62.0);
            // Esquerdo (A, B, X, Y digitam) à esquerda; com os dois sticks, o direito (o direcional
            // digita) à direita, com um traço entre os dois.
            let wheels: &[(f32, bool)] = if two { &[(KB_W / 2.0, false), (KB_W * 1.5, true)] } else { &[(KB_W / 2.0, false)] };
            if two {
                c.line(KB_W, 40.0, KB_W, 312.0, WHITE.alpha(0.10), 1.0);
            }
            for &(cx, right) in wheels {
                let side = right as usize;
                let numbers = |v: &crate::sys::osk::View| two && right != v.letters_right;
                let chosen = if right { v.right } else { v.petal };
                let (ha, hon) = hl[side];
                let hit = pk < 1.0 && v.hit.0 == right;
                stick_hub(c, cx, cy, hub, arrow[side]);
                if hit {
                    c.circle(cx, cy, hub, ACCENT.alpha(0.15 * (1.0 - pk)));
                }
                // No centro, os botões que digitam neste círculo; o modo logo abaixo.
                if right {
                    crate::ui::glyph::dpad(c, cx, cy, 26.0, None, TEXT2);
                } else {
                    crate::ui::glyph::face(c, cx, cy, 40.0, style, TEXT2);
                }
                let mode = |v: &crate::sys::osk::View| if numbers(v) { "123" } else { kb_mode(v) };
                swap_text(c, rect(cx - 30.0, cy + 34.0, 60.0, 20.0), mode(&pv), mode(&v), mk, 13.0, MUTED, Weight::Semibold);
                let pos = |a: f32| (cx + ring * a.to_radians().sin(), cy - ring * a.to_radians().cos());
                // Cada pétala entra crescendo de 0,9 em cascata; a escolhida cresce mais 5%.
                let grow = |p: usize| {
                    let k = cascade(since, p);
                    // 5%: com mais, a escolhida encosta nas vizinhas.
                    let sc = (0.9 + 0.1 * k) * if chosen == Some(p) { 1.0 + 0.05 * hon } else { 1.0 };
                    let (px, py) = pos(p as f32 * 45.0);
                    c.scale_at(sc, bx + px * (1.0 - sc), by + py * (1.0 - sc));
                    (k, px, py)
                };
                for p in 0..8 {
                    let (k, px, py) = grow(p);
                    c.circle(px, py, pr, WHITE.alpha(0.07 * k));
                }
                c.translate(bx, by);
                // O destaque gira pelo anel até a pétala escolhida.
                if hon > 0.01 {
                    let (hx, hy) = pos(ha);
                    let hr = pr * (1.0 + 0.05 * hon);
                    c.circle(hx, hy, hr, ACCENT.alpha(0.24 * hon));
                    c.circle_line(hx, hy, hr, ACCENT.alpha(hon), 1.5);
                }
                // As outras pétalas apagam um pouco enquanto uma está escolhida.
                let dim = if chosen.is_some() { 1.0 - 0.3 * hon } else { 1.0 };
                for p in 0..8 {
                    let (k, px, py) = grow(p);
                    let on = chosen == Some(p);
                    // Mesma posição dos botões (X à esquerda, Y em cima, B à direita, A embaixo) e
                    // das setas do direcional.
                    for (i, (dx, dy)) in [(-22.0, 0.0), (0.0, -22.0), (22.0, 0.0), (0.0, 22.0)].into_iter().enumerate() {
                        let (x, y) = (px + dx, py + dy);
                        let (mut size, col, weight) = if on { (17.0, WHITE.alpha(k), Weight::Semibold) } else { (15.0, TEXT2.alpha(k * dim), Weight::Regular) };
                        // A letra digitada pulsa: maior e na cor de destaque, voltando em 200 ms.
                        let pulse = pk < 1.0 && v.hit == (right, p, i);
                        if pulse {
                            size *= 1.4 - 0.4 * pk;
                        }
                        let tr = rect(x - 18.0, y - 15.0, 36.0, 30.0);
                        let now = v.wheel_char(right, p, i).to_string();
                        swap_text(c, tr, &pv.wheel_char(right, p, i).to_string(), &now, mk, size, col, weight);
                        if pulse {
                            c.text_raw(tr, &now, size, ACCENT.alpha(1.0 - pk), Align::Center, weight);
                            c.circle_line(x, y, 8.0 + 14.0 * pk, ACCENT.alpha(0.6 * (1.0 - pk)), 1.5);
                        }
                    }
                }
                c.translate(bx, by);
            }
            crate::ui::glyph::hints(c, rect(16.0, 336.0, r.w - 32.0, 22.0), style, &lines[0], 12.0, MUTED);
            crate::ui::glyph::hints(c, rect(16.0, 362.0, r.w - 32.0, 22.0), style, &lines[1], 12.0, MUTED);
            c.translate(0.0, 0.0);
        });
    }
}

/// Radial do Prisma (`sys::radial`): oito ações em gomos de um anel, no meio da tela.
pub struct Radial {
    surface: Layered,
    /// Sticks direito e esquerdo desenhados agora.
    arrow: [(f32, f32); 2],
    /// Canto do painel na tela.
    at: (i32, i32),
    names: [String; crate::sys::radial::ALL],
    icons: [u8; crate::sys::radial::ALL],
    /// Nome do lugar cabe em duas linhas no gomo; senão, uma com reticências.
    wraps: [bool; crate::sys::radial::SLOTS],
    two: bool,
    /// Algum lugar tem 4 opções: a janela cresce para a faixa delas, embaixo do círculo.
    groups: bool,
    left: bool,
    hint: Vec<(Vec<crate::sys::pads::Buttons>, String)>,
    style: crate::ui::glyph::Style,
    ticking: bool,
    /// Abre (0 a 1) e fecha (1 a 0, com `closing`); o relógio segue até acabar de sumir.
    show: Tw,
    closing: bool,
    opened: std::time::Instant,
    /// Estado desenhado; enquanto fecha, fica o último.
    last: crate::sys::radial::View,
    /// Quanto cada lugar está escolhido: o gomo cresce e acende.
    pop: [Tw; crate::sys::radial::SLOTS],
    /// Destaque de cada círculo: ângulo e quanto aparece.
    hl: [(Tw, Tw); 2],
    /// Nome no centro: troca (0 a 1) a partir do lugar de antes.
    label: Tw,
    label_prev: Option<usize>,
    /// Faixa das 4 opções: aparece (0 a 1), de que lugar é, posição e brilho do destaque.
    strip: Tw,
    strip_slot: Option<usize>,
    strip_x: Tw,
    strip_on: Tw,
    /// Lugar disparado: pisca antes de a janela sumir.
    flash: Tw,
    flash_slot: Option<usize>,
}

/// Largura de um círculo; com dois, a janela tem os dois lado a lado.
const RD_W: f32 = 600.0;
const RD_H: f32 = 480.0;
/// Faixa das 4 opções do direcional, entre o círculo e a dica.
const RD_SUB: f32 = 100.0;
/// Anel dos gomos (raio de dentro e de fora) e disco do centro.
const RD_R0: f32 = 96.0;
const RD_R1: f32 = 198.0;
const RD_HUB: f32 = 78.0;

unsafe extern "system" fn radial_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_TIMER {
        crate::app::with(|a| a.radial_tick());
        return LRESULT(0);
    }
    DefWindowProcW(h, m, w, l)
}

impl Radial {
    pub fn new() -> Self {
        register(w!("PrismaRadial"), radial_proc);
        let hwnd = create_overlay(w!("PrismaRadial"), true);
        Self {
            surface: Layered::new(hwnd),
            arrow: [(0.0, 0.0); 2],
            at: (0, 0),
            names: std::array::from_fn(|_| String::new()),
            icons: [0; crate::sys::radial::ALL],
            wraps: [false; crate::sys::radial::SLOTS],
            two: false,
            groups: false,
            left: false,
            hint: Vec::new(),
            style: crate::ui::glyph::Style::Xbox,
            ticking: false,
            show: Tw::new(0.0),
            closing: false,
            opened: std::time::Instant::now(),
            last: Default::default(),
            pop: [Tw::new(0.0); crate::sys::radial::SLOTS],
            hl: [(Tw::new(0.0), Tw::new(0.0)); 2],
            label: Tw::new(1.0),
            label_prev: None,
            strip: Tw::new(0.0),
            strip_slot: None,
            strip_x: Tw::new(0.0),
            strip_on: Tw::new(0.0),
            flash: Tw::new(0.0),
            flash_slot: None,
        }
    }

    /// Mostra o estado de agora, ou começa a esconder se fechou. As ações e os programas vêm da
    /// configuração.
    pub fn update(&mut self, cfg: &crate::config::Config) {
        use crate::sys::radial::{GROUP, PROFILES, SLOTS};
        let hwnd = self.surface.hwnd;
        let v = crate::sys::radial::view();
        if !v.open {
            if !self.ticking || self.closing {
                return;
            }
            let fired = crate::sys::radial::fired();
            // Captura: some na hora, como antes, para o radial não sair na foto.
            if fired.is_some_and(|a| a <= 5) {
                unsafe {
                    let _ = KillTimer(Some(hwnd), 1);
                    let _ = ShowWindow(hwnd, SW_HIDE);
                }
                self.ticking = false;
                return;
            }
            // Fechou por Confirmar: o gomo pisca 220 ms e a janela some a partir dos 120.
            self.closing = true;
            self.flash_slot = self.last.sel.filter(|_| fired.is_some());
            let wait = if self.flash_slot.is_some() {
                self.flash = Tw::new(1.0);
                self.flash.set(0.0, 220.0);
                120.0
            } else {
                0.0
            };
            self.show.go(0.0, 140.0, wait, true);
            return;
        }
        // Reabrir enquanto some conta como abrir de novo.
        let opened = !self.ticking || self.closing;
        if opened {
            let mon = focus_monitor();
            let scale = dpi_scale(hwnd);
            self.two = v.two;
            self.left = v.left;
            self.groups = cfg.pad_radial[..SLOTS].iter().any(|&a| a == GROUP || a == PROFILES);
            self.at = ((mon.left + mon.right) / 2 - (self.width() * scale / 2.0) as i32, (mon.top + mon.bottom) / 2 - (self.height() * scale / 2.0) as i32);
            for (i, &a) in cfg.pad_radial.iter().enumerate() {
                self.names[i] = if a == crate::sys::radial::PROGRAM {
                    cfg.pad_radial_prog[i].0.clone()
                } else if a == PROFILES {
                    crate::i18n::tr("Trocar perfil").into_owned()
                } else {
                    crate::i18n::tr(crate::sys::remap::action_name(a)).into_owned()
                };
            }
            self.icons = cfg.pad_radial;
            // Lugar de perfis: as opções são os perfis lidos ao abrir.
            let choices = crate::sys::radial::choices();
            for i in (0..SLOTS).filter(|&i| cfg.pad_radial[i] == PROFILES) {
                for d in 0..4 {
                    let k = crate::sys::radial::sub(i, d);
                    self.names[k] = choices.get(d).cloned().unwrap_or_default();
                    self.icons[k] = if d < choices.len() { PROFILES } else { crate::sys::radial::EMPTY };
                }
            }
            // Medido uma vez, em negrito (a largura maior): medir a cada quadro custa.
            self.wraps = std::array::from_fn(|i| measure(&self.names[i], 11.0, Weight::Semibold, Some(86.0)).1 <= 30.0);
            self.style = crate::ui::glyph::Style::current();
            let b = |i: u8| crate::sys::radial::PICK_BUTTONS[i.min(5) as usize];
            let t = |s: &str| crate::i18n::tr(s).into_owned();
            self.hint = vec![(vec![b(cfg.pad_radial_ok)], t("dispara")), (vec![b(cfg.pad_radial_back)], t("fecha"))];
            if self.groups {
                self.hint.insert(1, (vec![crate::ui::glyph::DPAD_ANY], t("escolhe entre as 4 opções")));
            }
            self.arrow = crate::sys::radial::stick();
            self.closing = false;
            self.show = Tw::new(0.0);
            self.show.set(1.0, 180.0);
            self.opened = std::time::Instant::now();
            self.pop = [Tw::new(0.0); SLOTS];
            self.hl = [(Tw::new(0.0), Tw::new(0.0)); 2];
            self.label = Tw::new(1.0);
            self.label_prev = None;
            self.strip = Tw::new(0.0);
            self.strip_slot = None;
            self.strip_on = Tw::new(0.0);
            self.flash = Tw::new(0.0);
            self.flash_slot = None;
            unsafe {
                SetTimer(Some(hwnd), 1, KB_FRAME, None);
            }
            self.ticking = true;
        }
        let before = if opened { Default::default() } else { self.last };
        if v.sel != before.sel {
            if let Some(i) = before.sel.filter(|&i| i < SLOTS) {
                self.pop[i].set(0.0, 120.0);
            }
            if let Some(i) = v.sel.filter(|&i| i < SLOTS) {
                self.pop[i].set(1.0, 160.0);
            }
            let two = self.two;
            let ring_of = |i: usize| (two && i < 8) as usize;
            for ring in 0..2 {
                let (now, was) = (v.sel.filter(|&i| ring_of(i) == ring), before.sel.filter(|&i| ring_of(i) == ring));
                if now == was {
                    continue;
                }
                let (ang, on) = &mut self.hl[ring];
                match now {
                    Some(i) => turn_to(ang, on, (i % 8) as f32 * 45.0),
                    None => on.set(0.0, 120.0),
                }
            }
            self.label_prev = before.sel;
            self.label = Tw::new(0.0);
            self.label.set(1.0, 120.0);
            // A faixa sobe 10 px e aparece com um lugar de 4 opções; ao sair dele, some.
            match v.sel.filter(|&i| self.groups && (self.icons[i] == GROUP || self.icons[i] == PROFILES)) {
                Some(i) => {
                    self.strip_slot = Some(i);
                    self.strip.set(1.0, 160.0);
                }
                None => self.strip.set(0.0, 120.0),
            }
        }
        if v.sub != before.sub {
            match v.sub {
                Some(d) => {
                    if self.strip_on.v() < 0.05 {
                        self.strip_x = Tw::new(d as f32);
                    } else {
                        self.strip_x.set(d as f32, 140.0);
                    }
                    self.strip_on.set(1.0, 140.0);
                }
                None => self.strip_on.set(0.0, 120.0),
            }
        }
        self.last = v;
        self.paint();
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }

    /// Um quadro: sticks e animações; no fim do fechar, esconde e para o relógio.
    pub fn tick(&mut self) {
        if self.closing && !self.show.busy() {
            unsafe {
                let _ = KillTimer(Some(self.surface.hwnd), 1);
                let _ = ShowWindow(self.surface.hwnd, SW_HIDE);
            }
            self.ticking = false;
            self.closing = false;
            return;
        }
        let [r, l] = crate::sys::radial::stick();
        let (a, b) = (ease(&mut self.arrow[0], r), ease(&mut self.arrow[1], l));
        if a || b || self.animating() {
            self.paint();
        }
    }

    fn animating(&self) -> bool {
        // 8 gomos em cascata: o último entra aos 56 ms e leva 160.
        self.opened.elapsed().as_millis() < 240
            || [self.show, self.flash, self.label, self.strip, self.strip_x, self.strip_on].iter().any(Tw::busy)
            || self.pop.iter().any(Tw::busy)
            || self.hl.iter().any(|(a, o)| a.busy() || o.busy())
    }

    fn width(&self) -> f32 {
        if self.two { RD_W * 2.0 + 40.0 } else { RD_W }
    }

    fn height(&self) -> f32 {
        if self.groups { RD_H + RD_SUB } else { RD_H }
    }

    fn paint(&mut self) {
        use crate::sys::radial::{sub, EMPTY, GROUP, SLOTS};
        let v = self.last;
        let sel = v.sel;
        let s = self.show.v();
        // Abre de 95% do tamanho; fecha até 97%, ambos pelo meio.
        let k = if self.closing { 0.97 + 0.03 * s } else { 0.95 + 0.05 * s };
        let since = self.opened.elapsed().as_secs_f32() * 1000.0;
        let pops: [f32; SLOTS] = std::array::from_fn(|i| self.pop[i].v());
        let hl = self.hl.map(|(a, o)| (a.v(), o.v()));
        let (lk, lprev) = (self.label.v(), self.label_prev);
        let (sv, sslot, sx, son) = (self.strip.v(), self.strip_slot, self.strip_x.v(), self.strip_on.v());
        let (fl, fslot) = (self.flash.v(), self.flash_slot);
        let (names, icons, wraps, hint, arrow, two, left, style) = (&self.names, &self.icons, &self.wraps, &self.hint, self.arrow, self.two, self.left, self.style);
        let (w, h) = (self.width(), self.height());
        self.surface.alpha = (s.clamp(0.0, 1.0) * 255.0).round() as u8;
        let m = (SHADOW * dpi_scale(self.surface.hwnd)).round() as i32;
        self.surface.paint(self.at.0 - m, self.at.1 - m, w + 2.0 * SHADOW, h + 2.0 * SHADOW, |c, _| {
            c.scale_at(k, SHADOW + w / 2.0 * (1.0 - k), SHADOW + h / 2.0 * (1.0 - k));
            let r = rect(0.0, 0.0, w, h);
            pad_panel(c, r);
            let cy = 226.0;
            // Com dois círculos, o do stick esquerdo (lugares 8 a 15) fica à esquerda.
            let rings: &[(f32, usize)] = if two { &[(RD_W / 2.0, 8), (w - RD_W / 2.0, 0)] } else { &[(RD_W / 2.0, 0)] };
            let ring_of = |i: usize| (two && i < 8) as usize;
            // Traço entre os dois círculos, para não parecerem um só.
            if two {
                c.line(w / 2.0, 40.0, w / 2.0, 420.0, WHITE.alpha(0.10), 1.0);
            }
            for (ring, &(cx, first)) in rings.iter().enumerate() {
                let stick = if left { 1 } else { first / 8 };
                for i in first..first + 8 {
                    let a = (i % 8) as f32 * 45.0;
                    let (a0, a1) = (a - 22.5 + 0.6, a + 22.5 - 0.6);
                    // Ao abrir, cada gomo cresce para fora em cascata; o escolhido sai 7 px.
                    let ck = cascade(since, i % 8);
                    let pop = pops[i];
                    let r1 = RD_R0 + (0.7 + 0.3 * ck) * (RD_R1 - RD_R0) + 7.0 * pop;
                    let r0 = RD_R0 + 2.0 * pop;
                    if icons[i] != GROUP && names[i].is_empty() {
                        c.sector(cx, cy, r0, r1, a0, a1, &[], Some((WHITE.alpha(0.12 * ck), 1.0)));
                        continue;
                    }
                    let flash = if fslot == Some(i) { fl } else { 0.0 };
                    let fills = [WHITE.alpha(0.055 * ck), ACCENT.alpha(0.24 * pop), ACCENT.alpha(0.6 * flash)];
                    let n = if flash > 0.0 { 3 } else if pop > 0.0 { 2 } else { 1 };
                    c.sector(cx, cy, r0, r1, a0, a1, &fills[..n], Some((WHITE.alpha(0.07 * ck), 1.0)));
                    let rm = (r0 + r1) / 2.0;
                    let (px, py) = (cx + rm * a.to_radians().sin(), cy - rm * a.to_radians().cos());
                    let lit = pop > 0.5;
                    let ic = if lit { WHITE } else { ACCENT }.alpha(ck);
                    if icons[i] == GROUP {
                        // As 4 opções em cruz, na posição do direcional, sem nome: os nomes ficam na faixa.
                        for (d, (dx, dy)) in [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)].into_iter().enumerate() {
                            let kk = icons[sub(i, d)];
                            if kk != EMPTY {
                                c.icon(px - 8.0 + dx * 20.0, py - 8.0 + dy * 18.0, 16.0, crate::pages::action_icon(kk), ic);
                            }
                        }
                        continue;
                    }
                    // Ícone em cima, nome embaixo: duas linhas se couber, senão uma com reticências.
                    c.icon(px - 10.5, py - 22.0, 21.0, crate::pages::action_icon(icons[i]), ic);
                    let (tc, wt) = if lit { (WHITE.alpha(ck), Weight::Semibold) } else { (TEXT2.alpha(ck), Weight::Regular) };
                    if wraps[i] {
                        c.text_wrap_center(rect(px - 43.0, py + 2.0, 86.0, 30.0), &names[i], 11.0, tc, wt);
                    } else {
                        c.text_raw(rect(px - 43.0, py + 2.0, 86.0, 16.0), &names[i], 11.0, tc, Align::Center, wt);
                    }
                }
                // Destaque: arco por fora do gomo, que gira até o escolhido; três traços fazem o brilho.
                let (ha, hon) = hl[ring];
                if hon > 0.01 {
                    let rr = RD_R1 + 7.0 * hon + 3.0;
                    for (lw, al) in [(16.0, 0.06), (8.0, 0.12), (3.0, 1.0)] {
                        c.arc_span(cx, cy, rr, ha - 22.5 + 0.6, ha + 22.5 - 0.6, ACCENT.alpha(al * hon), lw);
                    }
                }
                // No centro, o nome inteiro do escolhido (troca em fade, 4 px) e o stick deste círculo.
                stick_hub(c, cx, cy, RD_HUB, arrow[stick]);
                // Lugar de 4 opções não tem nome: mostra o da opção do direcional.
                let mine = |j: Option<usize>| {
                    let j = j.filter(|&i| ring_of(i) == ring)?;
                    let j = match v.sub {
                        Some(d) if icons[j] == GROUP || icons[j] == crate::sys::radial::PROFILES => sub(j, d),
                        _ => j,
                    };
                    (!names[j].is_empty()).then_some(j)
                };
                let label = |j: usize, al: f32, dy: f32| c.text_wrap_center(rect(cx - 60.0, cy - 34.0 + dy, 120.0, 36.0), &names[j], 13.0, TEXT.alpha(al), Weight::Semibold);
                if let Some(j) = mine(sel) {
                    label(j, lk, 4.0 * (1.0 - lk));
                }
                if let Some(j) = mine(lprev).filter(|_| lk < 1.0) {
                    label(j, 1.0 - lk, -4.0 * lk);
                }
                crate::ui::glyph::draw(c, cx - 11.0, cy + 22.0, 22.0, style, if stick == 1 { crate::sys::pads::LS } else { crate::sys::pads::RS }, TEXT2);
            }
            // Faixa das 4 opções do lugar escolhido, embaixo do círculo dele: seta do direcional,
            // ícone e nome; o destaque corre entre elas.
            if let Some(i) = sslot.filter(|_| sv > 0.01) {
                let cx = if two && i < 8 { w - RD_W / 2.0 } else { RD_W / 2.0 };
                let (bw, gap) = (134.0, 8.0);
                let x0 = cx - (4.0 * bw + 3.0 * gap) / 2.0;
                let y = RD_H - 32.0 + 10.0 * (1.0 - sv);
                let bx = |d: f32| rect(x0 + d * (bw + gap), y, bw, 80.0);
                for d in 0..4 {
                    if icons[sub(i, d)] == EMPTY {
                        c.round_line(bx(d as f32), 12.0, WHITE.alpha(0.12 * sv), 1.0);
                    } else {
                        c.round(bx(d as f32), 12.0, WHITE.alpha(0.07 * sv));
                    }
                }
                if son > 0.01 {
                    c.round(bx(sx), 12.0, ACCENT.alpha(0.24 * son * sv));
                    c.round_line(bx(sx), 12.0, ACCENT.alpha(son * sv), 1.5);
                }
                for d in 0..4 {
                    let kk = sub(i, d);
                    if icons[kk] == EMPTY {
                        continue;
                    }
                    let b = bx(d as f32);
                    let on = sel == Some(i) && v.sub == Some(d);
                    crate::ui::glyph::dpad(c, b.x + 17.0, b.y + 17.0, 18.0, Some(d), if on { WHITE } else { MUTED }.alpha(sv));
                    c.icon(b.x + b.w / 2.0 - 10.0, b.y + 7.0, 20.0, crate::pages::action_icon(icons[kk]), if on { WHITE } else { ACCENT }.alpha(sv));
                    c.text_wrap_center(rect(b.x + 6.0, b.y + 31.0, b.w - 12.0, 46.0), &names[kk], 11.0, if on { WHITE } else { TEXT2 }.alpha(sv), if on { Weight::Semibold } else { Weight::Regular });
                }
            }
            crate::ui::glyph::hints(c, rect(16.0, r.h - 36.0, r.w - 32.0, 22.0), style, hint, 12.0, MUTED);
            c.translate(0.0, 0.0);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dodge_only_when_behind() {
        let r = |l, t| RECT { left: l, top: t, right: l + 2, bottom: t + 20 };
        // Teclado em x 700, de y 600 a 992, 520 de largura.
        assert!(dodge_up(r(900, 700), 700, 600, 520, 392));
        assert!(dodge_up(r(690, 590), 700, 600, 520, 392));
        assert!(!dodge_up(r(900, 300), 700, 600, 520, 392));
        assert!(!dodge_up(r(100, 700), 700, 600, 520, 392));
    }

    /// Custo de um quadro do radial e do teclado, os dois círculos e tudo aceso:
    /// `cargo test --release frame_cost -- --ignored --nocapture`. O relógio é de 16 ms.
    #[test]
    #[ignore]
    fn frame_cost() {
        use crate::sys::radial::{sub, SLOTS};
        let ago = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let time = |name: &str, f: &mut dyn FnMut()| {
            f();
            let t = std::time::Instant::now();
            for _ in 0..120 {
                f();
            }
            println!("{name}: {:.2} ms por quadro", t.elapsed().as_secs_f32() * 1000.0 / 120.0);
        };
        let mut r = Radial::new();
        for i in 0..SLOTS {
            r.names[i] = format!("Ação de teste número {i}");
            r.icons[i] = 1;
            for d in 0..4 {
                r.names[sub(i, d)] = format!("Opção {d}");
                r.icons[sub(i, d)] = 1;
            }
        }
        r.wraps = [true; SLOTS];
        (r.two, r.groups, r.opened) = (true, true, ago);
        r.last = crate::sys::radial::View { open: true, sel: Some(3), sub: Some(1), two: true, ..Default::default() };
        r.pop[3] = Tw::new(1.0);
        r.hl[1] = (Tw::new(135.0), Tw::new(1.0));
        (r.strip, r.strip_slot, r.strip_x, r.strip_on) = (Tw::new(1.0), Some(3), Tw::new(1.0), Tw::new(1.0));
        r.arrow = [(0.5, 0.5); 2];
        time("radial", &mut || r.paint());
        let mut k = Keyboard::new();
        (k.two, k.opened) = (true, ago);
        k.last = crate::sys::osk::View { open: true, two: true, petal: Some(2), right: Some(5), ..Default::default() };
        k.hl = [(Tw::new(90.0), Tw::new(1.0)), (Tw::new(225.0), Tw::new(1.0))];
        k.arrow = [(0.5, 0.5); 2];
        time("teclado", &mut || k.paint());
        // Letra pulsando e letras trocando de modo, o quadro mais cheio.
        k.prev.shift = true;
        (k.swap, k.pulse) = (Tw::new(0.5), Tw::new(0.5));
        k.swap.set(1.0, 1e9);
        k.pulse.set(1.0, 1e9);
        time("teclado animando", &mut || k.paint());
    }

    /// Fotos do radial e do teclado, já abertos (build de debug):
    /// `PRISMA_LAYER_DUMP=<pasta> cargo test layer_shots -- --ignored`.
    #[test]
    #[ignore]
    fn layer_shots() {
        use crate::sys::radial::{sub, EMPTY, GROUP, PROGRAM, SLOTS};
        let dir = std::env::var("PRISMA_LAYER_DUMP").expect("PRISMA_LAYER_DUMP");
        let ago = std::time::Instant::now() - std::time::Duration::from_secs(1);
        let keep = |name: &str| {
            let newest = std::fs::read_dir(&dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().starts_with("camada_")).max_by_key(|e| e.metadata().unwrap().modified().unwrap()).unwrap();
            std::fs::rename(newest.path(), format!("{dir}\\{name}.png")).unwrap();
        };
        for (two, file) in [(false, "radial_um"), (true, "radial_dois")] {
            let mut r = Radial::new();
            for i in 0..SLOTS {
                let a = [0u8, 6, 15, GROUP, PROGRAM, 20, EMPTY, 29][i % 8];
                r.icons[i] = a;
                r.names[i] = match a {
                    PROGRAM => "Visual Studio Code".into(),
                    EMPTY => String::new(),
                    _ => crate::i18n::tr(crate::sys::remap::action_name(a)).into_owned(),
                };
                for d in 0..4 {
                    let s = [1u8, 7, 16, 21][d];
                    r.icons[sub(i, d)] = s;
                    r.names[sub(i, d)] = crate::i18n::tr(crate::sys::remap::action_name(s)).into_owned();
                }
                r.wraps[i] = measure(&r.names[i], 11.0, Weight::Semibold, Some(86.0)).1 <= 30.0;
            }
            r.hint = vec![(vec![crate::sys::pads::A], "dispara".into()), (vec![crate::ui::glyph::DPAD_ANY], "escolhe entre as 4 opções".into()), (vec![crate::sys::pads::B], "fecha".into())];
            (r.two, r.groups, r.opened, r.show) = (two, true, ago, Tw::new(1.0));
            r.last = crate::sys::radial::View { open: true, sel: Some(3), sub: Some(1), two, ..Default::default() };
            r.pop[3] = Tw::new(1.0);
            r.hl[(two) as usize] = (Tw::new(135.0), Tw::new(1.0));
            (r.strip, r.strip_slot, r.strip_x, r.strip_on) = (Tw::new(1.0), Some(3), Tw::new(1.0), Tw::new(1.0));
            r.arrow = [(0.6, -0.6); 2];
            r.paint();
            keep(file);
        }
        for (two, file) in [(false, "teclado_um"), (true, "teclado_dois")] {
            let mut k = Keyboard::new();
            (k.two, k.opened, k.show) = (two, ago, Tw::new(1.0));
            let t = |s: &str| s.to_string();
            k.lines = [
                vec![(vec![crate::sys::pads::LB], t("apaga")), (vec![crate::sys::pads::RB], t("espaço")), (vec![crate::sys::pads::START], t("Enter")), (vec![crate::ui::glyph::DPAD_ANY], t("setas"))],
                vec![(vec![crate::sys::remap::RT], t("maiúsculas")), (vec![crate::sys::remap::LT], t("números")), (vec![crate::sys::pads::BACK], t("fecha"))],
            ];
            k.last = crate::sys::osk::View { open: true, two, petal: Some(2), right: two.then_some(5), ..Default::default() };
            k.prev = k.last;
            k.hl = [(Tw::new(90.0), Tw::new(1.0)), (Tw::new(225.0), Tw::new(1.0))];
            k.arrow = [(0.7, 0.0), (-0.5, -0.5)];
            k.paint();
            keep(file);
        }
    }

    /// Lê o cursor de texto da janela em foco agora: `cargo test caret_now -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn caret_now() {
        let c = caret();
        let mut name = [0u16; 128];
        let n = unsafe { GetClassNameW(GetForegroundWindow(), &mut name) } as usize;
        println!("janela: {} cursor: {:?}", String::from_utf16_lossy(&name[..n]), c.map(|r| (r.left, r.top, r.right, r.bottom)));
    }
}
