//! Janelas flutuantes: widget, aviso rápido e pergunta de otimização.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
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
}

unsafe extern "system" fn widget_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_NCHITTEST => LRESULT(HTCAPTION as isize),
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
        Self { surface: Layered::new(hwnd), visible: false, status: Status::default(), status_at: None, status_items: 0, vrr: Default::default(), vrr_at: None }
    }

    pub fn set_visible(&mut self, on: bool) {
        self.visible = on;
        self.status_at = None;
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
        crate::sys::cputemp::WIDGET.store(self.visible && has(mask, CPU_TEMP), Relaxed);
        crate::sys::monitor::GPU_TEMP.store(self.visible && has(mask, GPU_TEMP), Relaxed);
        if !self.visible {
            return;
        }
        self.refresh(mask);
        let m = items(s, &self.status, mask);
        let style = cfg.widget_style;
        let (w, h) = size(style, &m);
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

    /// Aviso que fica na tela até `hide` (gravação de macro).
    pub fn keep(&mut self, text: &str) {
        self.info(text);
        unsafe {
            let _ = KillTimer(Some(self.surface.hwnd), 1);
        }
    }

    pub fn hide(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.surface.hwnd), 1);
            let _ = ShowWindow(self.surface.hwnd, SW_HIDE);
        }
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
        crate::capture::hide_from_capture(hwnd);
        Self { surface: Layered::new(hwnd), ui: UiState::default(), title: String::new(), body: Vec::new(), buttons: Vec::new(), gap: (24.0, 24.0), open: false }
    }

    /// `limit`: some sozinha depois desses ms (`prompt_hide`); `None` fica até a resposta.
    pub fn ask(&mut self, title: String, body: Vec<String>, buttons: &[(&'static str, bool)], gap: (f32, f32), limit: Option<u32>) {
        self.title = title;
        self.body = body;
        self.buttons = buttons.to_vec();
        self.gap = gap;
        self.open = true;
        self.ui = UiState::default();
        self.redraw();
        unsafe {
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

/// Círculo do meio e a seta para onde o stick aponta, mais comprida quanto mais inclinado.
fn stick_arrow(c: &Canvas, cx: f32, cy: f32, (ax, ay): (f32, f32)) {
    let len = (ax * ax + ay * ay).sqrt();
    let m = len.min(1.0);
    c.circle(cx, cy, 30.0, WHITE.alpha(0.08));
    c.circle_line(cx, cy, 30.0, ACCENT.alpha(0.15 + 0.6 * m), 2.0);
    if m > 0.08 {
        let (ux, uy) = (ax / len, -ay / len);
        let (r0, r1) = (32.0, 34.0 + 34.0 * m);
        let (tx, ty) = (cx + ux * r1, cy + uy * r1);
        c.line(cx + ux * r0, cy + uy * r0, tx, ty, ACCENT, 4.0);
        for side in [1.0, -1.0] {
            c.line(tx, ty, tx - ux * 10.0 - uy * 7.0 * side, ty - uy * 10.0 + ux * 7.0 * side, ACCENT, 4.0);
        }
    }
}

pub struct Keyboard {
    surface: Layered,
    /// Setas desenhadas agora (esquerdo, direito): correm atrás dos analógicos a cada quadro, para
    /// andar suave.
    arrow: [(f32, f32); 2],
    /// Canto na tela, as duas linhas de ajuda e os dois sticks, fixos enquanto o teclado fica aberto.
    at: (i32, i32),
    lines: [Vec<(Vec<crate::sys::pads::Buttons>, String)>; 2],
    style: crate::ui::glyph::Style,
    two: bool,
    ticking: bool,
    /// Fugir do cursor: altura em pixels embaixo e no alto da tela, e a de agora, que corre atrás
    /// da escolhida.
    rows: (i32, i32),
    y: f32,
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

impl Keyboard {
    pub fn new() -> Self {
        register(w!("PrismaKeyboard"), keyboard_proc);
        let hwnd = create_overlay(w!("PrismaKeyboard"), true);
        Self { surface: Layered::new(hwnd), arrow: [(0.0, 0.0); 2], at: (0, 0), lines: Default::default(), style: crate::ui::glyph::Style::Xbox, two: false, ticking: false, rows: (0, 0), y: 0.0 }
    }

    /// Mostra o estado de agora do teclado, ou esconde se ele fechou. Devolve `Some` quando ele
    /// acabou de abrir (`true`) ou de fechar (`false`).
    pub fn update(&mut self) -> Option<bool> {
        use crate::sys::pads;
        let open = crate::sys::osk::view().open;
        let hwnd = self.surface.hwnd;
        if !open {
            unsafe {
                let _ = KillTimer(Some(hwnd), 1);
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            return std::mem::take(&mut self.ticking).then_some(false);
        }
        let opened = !self.ticking;
        if opened {
            let mon = focus_monitor();
            let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
            self.two = crate::sys::osk::view().two;
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
            unsafe {
                SetTimer(Some(hwnd), 1, KB_FRAME, None);
            }
            self.ticking = true;
        }
        self.paint();
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        opened.then_some(true)
    }

    /// Um quadro: cada seta anda 45% do caminho até o analógico, o teclado anda 35% do caminho até
    /// a altura escolhida (embaixo, ou no alto com o cursor de texto atrás dele), e só redesenha se
    /// algo mexeu.
    pub fn tick(&mut self) {
        let [l, r] = crate::sys::osk::stick();
        let (a, b) = (ease(&mut self.arrow[0], l), ease(&mut self.arrow[1], r));
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(self.surface.hwnd) } as f32 / 96.0;
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
        if a || b || moved {
            self.paint();
        }
    }

    /// Com os dois sticks, os dois círculos lado a lado.
    fn width(&self) -> f32 {
        if self.two { KB_W * 2.0 } else { KB_W }
    }

    fn paint(&mut self) {
        let v = crate::sys::osk::view();
        let (arrow, two, style) = (self.arrow, self.two, self.style);
        let lines = &self.lines;
        let mode = match (v.sym, v.shift) {
            (true, true) => "áç",
            (true, false) => "123",
            (false, true) => "ABC",
            (false, false) => "abc",
        };
        self.surface.paint(self.at.0, self.at.1, self.width(), KB_H, |c, r| {
            c.round(r, 18.0, rgb(0x0d0f14).alpha(0.92));
            let (cy, ring, pr) = (176.0, 116.0, 42.0);
            // Esquerdo (A, B, X, Y digitam) à esquerda; com os dois sticks, o direito (o direcional
            // digita) à direita, com um traço entre os dois.
            let wheels: &[(f32, bool)] = if two { &[(KB_W / 2.0, false), (KB_W * 1.5, true)] } else { &[(KB_W / 2.0, false)] };
            if two {
                c.line(KB_W, 40.0, KB_W, 312.0, WHITE.alpha(0.10), 1.0);
            }
            for &(cx, right) in wheels {
            stick_arrow(c, cx, cy, arrow[right as usize]);
            let numbers = two && right != v.letters_right;
            // No centro, os botões que digitam neste círculo; o modo logo abaixo.
            if right {
                crate::ui::glyph::dpad(c, cx, cy, 26.0, None, TEXT2);
            } else {
                crate::ui::glyph::face(c, cx, cy, 40.0, style, TEXT2);
            }
            c.text_raw(rect(cx - 30.0, cy + 34.0, 60.0, 20.0), if numbers { "123" } else { mode }, 13.0, MUTED, Align::Center, Weight::Semibold);
            let chosen = if right { v.right } else { v.petal };
            for p in 0..8 {
                let a = (p as f32 * 45.0).to_radians();
                let (px, py) = (cx + ring * a.sin(), cy - ring * a.cos());
                let on = chosen == Some(p);
                c.circle(px, py, pr, if on { ACCENT.alpha(0.35) } else { WHITE.alpha(0.07) });
                // Mesma posição dos botões (X à esquerda, Y em cima, B à direita, A embaixo) e
                // das setas do direcional.
                for (i, (dx, dy)) in [(-22.0, 0.0), (0.0, -22.0), (22.0, 0.0), (0.0, 22.0)].into_iter().enumerate() {
                    let (size, color, weight) = if on { (17.0, WHITE, Weight::Semibold) } else { (15.0, TEXT2, Weight::Regular) };
                    c.text_raw(rect(px + dx - 12.0, py + dy - 11.0, 24.0, 22.0), &v.wheel_char(right, p, i).to_string(), size, color, Align::Center, weight);
                }
            }
            }
            crate::ui::glyph::hints(c, rect(16.0, 336.0, r.w - 32.0, 22.0), style, &lines[0], 12.0, MUTED);
            crate::ui::glyph::hints(c, rect(16.0, 362.0, r.w - 32.0, 22.0), style, &lines[1], 12.0, MUTED);
        });
    }
}

/// Radial do Prisma (`sys::radial`): oito ações em volta, no meio da tela.
pub struct Radial {
    surface: Layered,
    /// Setas do stick direito e do esquerdo.
    arrow: [(f32, f32); 2],
    at: (i32, i32),
    names: [String; crate::sys::radial::ALL],
    icons: [u8; crate::sys::radial::ALL],
    two: bool,
    /// Algum lugar tem 4 opções: a janela cresce para a faixa delas, embaixo do círculo.
    groups: bool,
    left: bool,
    hint: Vec<(Vec<crate::sys::pads::Buttons>, String)>,
    style: crate::ui::glyph::Style,
    ticking: bool,
}

/// Largura de um círculo; com dois, a janela tem os dois lado a lado.
const RD_W: f32 = 600.0;
const RD_H: f32 = 460.0;
/// Faixa das 4 opções do direcional, entre o círculo e a dica.
const RD_SUB: f32 = 96.0;

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
        Self { surface: Layered::new(hwnd), arrow: [(0.0, 0.0); 2], at: (0, 0), names: std::array::from_fn(|_| String::new()), icons: [0; crate::sys::radial::ALL], two: false, groups: false, left: false, hint: Vec::new(), style: crate::ui::glyph::Style::Xbox, ticking: false }
    }

    /// Mostra o estado de agora, ou esconde se fechou. As ações e os programas vêm da configuração.
    pub fn update(&mut self, cfg: &crate::config::Config) {
        let hwnd = self.surface.hwnd;
        let v = crate::sys::radial::view();
        if !v.open {
            unsafe {
                let _ = KillTimer(Some(hwnd), 1);
                let _ = ShowWindow(hwnd, SW_HIDE);
            }
            self.ticking = false;
            return;
        }
        if !self.ticking {
            let mon = focus_monitor();
            let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
            self.two = v.two;
            self.left = v.left;
            self.groups = cfg.pad_radial[..crate::sys::radial::SLOTS].iter().any(|&a| a == crate::sys::radial::GROUP || a == crate::sys::radial::PROFILES);
            self.at = ((mon.left + mon.right) / 2 - (self.width() * scale / 2.0) as i32, (mon.top + mon.bottom) / 2 - (self.height() * scale / 2.0) as i32);
            for (i, &a) in cfg.pad_radial.iter().enumerate() {
                self.names[i] = if a == crate::sys::radial::PROGRAM {
                    cfg.pad_radial_prog[i].0.clone()
                } else if a == crate::sys::radial::PROFILES {
                    crate::i18n::tr("Trocar perfil").into_owned()
                } else {
                    crate::i18n::tr(crate::sys::remap::action_name(a)).into_owned()
                };
            }
            self.icons = cfg.pad_radial;
            // Lugar de perfis: as opções são os perfis lidos ao abrir.
            let choices = crate::sys::radial::choices();
            for i in (0..crate::sys::radial::SLOTS).filter(|&i| cfg.pad_radial[i] == crate::sys::radial::PROFILES) {
                for d in 0..4 {
                    let k = crate::sys::radial::sub(i, d);
                    self.names[k] = choices.get(d).cloned().unwrap_or_default();
                    self.icons[k] = if d < choices.len() { crate::sys::radial::PROFILES } else { crate::sys::radial::EMPTY };
                }
            }
            self.style = crate::ui::glyph::Style::current();
            let b = |i: u8| crate::sys::radial::PICK_BUTTONS[i.min(5) as usize];
            let t = |s: &str| crate::i18n::tr(s).into_owned();
            self.hint = vec![(vec![b(cfg.pad_radial_ok)], t("dispara")), (vec![b(cfg.pad_radial_back)], t("fecha"))];
            if self.groups {
                self.hint.insert(1, (vec![crate::ui::glyph::DPAD_ANY], t("escolhe entre as 4 opções")));
            }
            self.arrow = crate::sys::radial::stick();
            unsafe {
                SetTimer(Some(hwnd), 1, KB_FRAME, None);
            }
            self.ticking = true;
        }
        self.paint();
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub fn tick(&mut self) {
        let [r, l] = crate::sys::radial::stick();
        let (a, b) = (ease(&mut self.arrow[0], r), ease(&mut self.arrow[1], l));
        if a || b {
            self.paint();
        }
    }

    fn width(&self) -> f32 {
        if self.two { RD_W * 2.0 + 40.0 } else { RD_W }
    }

    fn height(&self) -> f32 {
        if self.groups { RD_H + RD_SUB } else { RD_H }
    }

    fn paint(&mut self) {
        use crate::sys::radial::{sub, EMPTY, GROUP, PROFILES};
        let v = crate::sys::radial::view();
        let sel = v.sel;
        let (names, icons, hint, arrow, two, left, groups, style) = (&self.names, &self.icons, &self.hint, self.arrow, self.two, self.left, self.groups, self.style);
        self.surface.paint(self.at.0, self.at.1, self.width(), self.height(), |c, r| {
            c.round(r, 18.0, rgb(0x0d0f14).alpha(0.92));
            let cy = 210.0;
            // Com dois círculos, o do stick esquerdo (lugares 8 a 15) fica à esquerda.
            let rings: &[(f32, usize)] = if two { &[(RD_W / 2.0, 8), (r.w - RD_W / 2.0, 0)] } else { &[(RD_W / 2.0, 0)] };
            // Traço entre os dois círculos, para não parecerem um só.
            if two {
                c.line(r.w / 2.0, 40.0, r.w / 2.0, 380.0, WHITE.alpha(0.10), 1.0);
            }
            for &(cx, first) in rings {
            let stick = if left { 1 } else { first / 8 };
            stick_arrow(c, cx, cy, arrow[stick]);
            // No centro, o stick que aponta neste círculo.
            crate::ui::glyph::draw(c, cx - 11.0, cy, 22.0, style, if stick == 1 { crate::sys::pads::LS } else { crate::sys::pads::RS }, TEXT2);
            for (i, name) in names.iter().enumerate().skip(first).take(8) {
                let a = ((i % 8) as f32 * 45.0).to_radians();
                let (px, py) = (cx + 210.0 * a.sin(), cy - 160.0 * a.cos());
                let b = rect(px - 70.0, py - 40.0, 140.0, 80.0);
                let on = sel == Some(i);
                if icons[i] == GROUP {
                    // As 4 opções em cruz, na posição do direcional, sem nome: os nomes ficam na faixa.
                    c.round(b, 12.0, if on { ACCENT.alpha(0.35) } else { WHITE.alpha(0.07) });
                    for (d, (dx, dy)) in [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)].into_iter().enumerate() {
                        let k = icons[sub(i, d)];
                        if k != EMPTY {
                            c.icon(px - 9.0 + dx * 26.0, py - 9.0 + dy * 22.0, 18.0, crate::pages::action_icon(k), if on { WHITE } else { ACCENT });
                        }
                    }
                    continue;
                }
                if name.is_empty() {
                    c.round_line(b, 12.0, WHITE.alpha(0.12), 1.0);
                    continue;
                }
                c.round(b, 12.0, if on { ACCENT.alpha(0.35) } else { WHITE.alpha(0.07) });
                // Ícone em cima, nome embaixo (até três linhas).
                c.icon(px - 10.0, b.y + 7.0, 20.0, crate::pages::action_icon(icons[i]), if on { WHITE } else { ACCENT });
                c.text_wrap_center(rect(b.x + 6.0, b.y + 31.0, b.w - 12.0, 46.0), name, 11.0, if on { WHITE } else { TEXT2 }, if on { Weight::Semibold } else { Weight::Regular });
            }
            }
            // Faixa das 4 opções do lugar escolhido, embaixo do círculo dele: seta do direcional, ícone e nome.
            if let Some(i) = sel.filter(|&i| groups && (icons[i] == GROUP || icons[i] == PROFILES)) {
                let cx = if two && i < 8 { r.w - RD_W / 2.0 } else { RD_W / 2.0 };
                let (w, gap) = (134.0, 8.0);
                let x0 = cx - (4.0 * w + 3.0 * gap) / 2.0;
                for d in 0..4 {
                    let k = sub(i, d);
                    let b = rect(x0 + d as f32 * (w + gap), RD_H - 40.0, w, 80.0);
                    if icons[k] == EMPTY {
                        c.round_line(b, 12.0, WHITE.alpha(0.12), 1.0);
                        continue;
                    }
                    let on = v.sub == Some(d);
                    c.round(b, 12.0, if on { ACCENT.alpha(0.35) } else { WHITE.alpha(0.07) });
                    crate::ui::glyph::dpad(c, b.x + 17.0, b.y + 17.0, 18.0, Some(d), if on { WHITE } else { MUTED });
                    c.icon(b.x + b.w / 2.0 - 10.0, b.y + 7.0, 20.0, crate::pages::action_icon(icons[k]), if on { WHITE } else { ACCENT });
                    c.text_wrap_center(rect(b.x + 6.0, b.y + 31.0, b.w - 12.0, 46.0), &names[k], 11.0, if on { WHITE } else { TEXT2 }, if on { Weight::Semibold } else { Weight::Regular });
                }
            }
            crate::ui::glyph::hints(c, rect(16.0, r.h - 36.0, r.w - 32.0, 22.0), style, hint, 12.0, MUTED);
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
