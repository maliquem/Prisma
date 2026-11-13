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

fn metrics(s: &Snapshot) -> [(&'static str, f32, String); 4] {
    [
        ("CPU", s.cpu, format!("{:.0}%", s.cpu)),
        ("GPU", s.gpu.util, format!("{:.0}%", s.gpu.util)),
        ("RAM", s.ram_pct(), fmt_gb(s.ram_used)),
        ("VRAM", s.vram_pct(), fmt_gb(s.gpu.used)),
    ]
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
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

impl Widget {
    pub fn new() -> Self {
        register(w!("PrismaWidget"), widget_proc);
        let hwnd = create_overlay(w!("PrismaWidget"), false);
        crate::capture::hide_from_capture(hwnd);
        Self { surface: Layered::new(hwnd), visible: false }
    }

    pub fn set_visible(&mut self, on: bool) {
        self.visible = on;
        unsafe {
            let _ = ShowWindow(self.surface.hwnd, if on { SW_SHOWNOACTIVATE } else { SW_HIDE });
        }
    }

    pub fn update(&mut self, cfg: &Config, s: &Snapshot) {
        if !self.visible {
            return;
        }
        let m = metrics(s);
        let (w, h) = match cfg.widget_style {
            0 => (316.0, 36.0),
            1 => (220.0, 150.0),
            2 => (304.0, 100.0),
            _ => (316.0, 70.0),
        };
        let style = cfg.widget_style;
        self.surface.paint(cfg.widget_x, cfg.widget_y, w, h, |c, r| match style {
            0 => draw_pill(c, r, &m),
            1 => draw_bars(c, r, &m),
            2 => draw_rings(c, r, &m),
            _ => draw_instrument(c, r, &m),
        });
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

fn draw_pill(c: &Canvas, r: Rect, m: &[(&str, f32, String); 4]) {
    c.round(r, r.h / 2.0, PANEL);
    let mut x = 16.0;
    for (n, v, _) in m {
        let t = format!("{v:.0}%");
        c.text(rect(x, 0.0, 40.0, r.h), n, 13.0, rgb(0xc5cad6), Align::Left, Weight::Regular);
        let nw = measure(n, 13.0, Weight::Regular, None).0 + 5.0;
        c.text(rect(x + nw, 0.0, 40.0, r.h), &t, 13.0, hot(*v), Align::Left, Weight::Semibold);
        x += nw + measure(&t, 13.0, Weight::Semibold, None).0 + 14.0;
    }
}

fn draw_bars(c: &Canvas, r: Rect, m: &[(&str, f32, String); 4]) {
    c.round(r, 12.0, PANEL);
    let mut y = 10.0;
    for (n, v, d) in m {
        c.text(rect(14.0, y, 90.0, 18.0), n, 13.0, rgb(0xc5cad6), Align::Left, Weight::Regular);
        c.text(rect(100.0, y, 106.0, 18.0), d, 13.0, WHITE, Align::Right, Weight::Semibold);
        c.round(rect(14.0, y + 20.0, 192.0, 3.0), 1.5, WHITE.alpha(0.16));
        c.round(rect(14.0, y + 20.0, 192.0 * v / 100.0, 3.0), 1.5, hot(*v));
        y += 33.0;
    }
}

fn draw_rings(c: &Canvas, r: Rect, m: &[(&str, f32, String); 4]) {
    c.round(r, 16.0, PANEL);
    for (i, (n, v, _)) in m.iter().enumerate() {
        let cx = 42.0 + i as f32 * 73.0;
        c.circle_line(cx, 40.0, 23.0, WHITE.alpha(0.16), 5.0);
        c.arc(cx, 40.0, 23.0, v / 100.0, if *v >= HOT { WARN } else { ACCENT }, 5.0);
        let t = format!("{v:.0}");
        c.text(rect(cx - 23.0, 17.0, 46.0, 46.0), &t, 14.0, WHITE, Align::Center, Weight::Semibold);
        c.text(rect(cx - 30.0, 70.0, 60.0, 20.0), n, 12.0, rgb(0xc5cad6), Align::Center, Weight::Regular);
    }
}

fn draw_instrument(c: &Canvas, r: Rect, m: &[(&str, f32, String); 4]) {
    let w = r.w / 4.0;
    for (i, (n, v, _)) in m.iter().enumerate() {
        let x = i as f32 * w;
        let t = format!("{v:.0}");
        let color = if *v >= HOT { rgb(0xffb000) } else { WHITE };
        let shadow = rgb(0).alpha(0.75);
        c.text(rect(x + 1.0, 1.0, w, 18.0), n, 12.0, shadow, Align::Left, Weight::Semibold);
        c.text(rect(x, 0.0, w, 18.0), n, 12.0, WHITE, Align::Left, Weight::Semibold);
        c.text(rect(x + 1.0, 19.0, w, 50.0), &t, 44.0, shadow, Align::Left, Weight::Regular);
        c.text(rect(x, 18.0, w, 50.0), &t, 44.0, color, Align::Left, Weight::Regular);
    }
}

/// Desenha um estilo dentro de um cartão da tela Widget.
pub fn preview(c: &Canvas, style: u8, area: Rect, s: &Snapshot) {
    let m = metrics(s);
    let (w, h) = match style {
        0 => (316.0, 36.0),
        1 => (220.0, 150.0),
        2 => (304.0, 100.0),
        _ => (316.0, 70.0),
    };
    let k = (area.w / w).min(area.h / h).min(1.0);
    c.scale_at(k, area.x, area.y);
    let r = rect(0.0, 0.0, w, h);
    match style {
        0 => draw_pill(c, r, &m),
        1 => draw_bars(c, r, &m),
        2 => draw_rings(c, r, &m),
        _ => draw_instrument(c, r, &m),
    }
    c.translate(0.0, 0.0);
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

    fn show(&mut self, w: f32, h: f32, bottom: f32, draw: impl FnOnce(&Canvas, Rect)) {
        let mon = focus_monitor();
        let hwnd = self.surface.hwnd;
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(hwnd) } as f32 / 96.0;
        let x = (mon.left + mon.right) / 2 - (w * scale / 2.0) as i32;
        let y = mon.bottom - ((h + bottom) * scale) as i32;
        self.surface.paint(x, y, w, h, draw);
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            self.timer = SetTimer(Some(hwnd), 1, 1500, None);
        }
    }

    pub fn volume(&mut self, name: &str, vol: f32, muted: bool) {
        let name = name.to_string();
        self.show(400.0, 62.0, 56.0, move |c, r| {
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
        crate::report::note(text);
        let text = text.to_string();
        let tw = measure(&text, 14.0, Weight::Regular, None).0 + 32.0;
        self.show(tw.max(120.0), 40.0, bottom, move |c, r| {
            c.round(r, 10.0, PANEL);
            c.text(r, &text, 14.0, WHITE, Align::Center, Weight::Regular);
        });
    }
}

/// Pergunta sobre o jogo: oferece fechar programas sem tirar o foco dele.
pub struct Prompt {
    surface: Layered,
    ui: UiState,
    title: String,
    body: String,
    /// Distância do canto superior direito, para não cobrir o medidor.
    gap: (f32, f32),
    pub open: bool,
}

pub enum PromptAnswer {
    Close,
    List,
    Dismiss,
}

const PROMPT_SIZE: (f32, f32) = (300.0, 110.0);
const PROMPT_MS: u32 = 10_000;

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
        Self { surface: Layered::new(hwnd), ui: UiState::default(), title: String::new(), body: String::new(), gap: (24.0, 24.0), open: false }
    }

    pub fn ask(&mut self, title: String, body: String, gap: (f32, f32)) {
        self.title = title;
        self.body = body;
        self.gap = gap;
        self.open = true;
        self.redraw();
        unsafe {
            let _ = ShowWindow(self.surface.hwnd, SW_SHOWNOACTIVATE);
            SetTimer(Some(self.surface.hwnd), 1, PROMPT_MS, None);
        }
    }

    pub fn hide(&mut self) {
        self.open = false;
        unsafe {
            let _ = ShowWindow(self.surface.hwnd, SW_HIDE);
        }
    }

    pub fn mouse(&mut self, msg: u32, x: f32, y: f32) -> Option<PromptAnswer> {
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

    fn redraw(&mut self) -> Option<PromptAnswer> {
        let mon = focus_monitor();
        let scale = unsafe { windows::Win32::UI::HiDpi::GetDpiForWindow(self.surface.hwnd) } as f32 / 96.0;
        let (w, h) = PROMPT_SIZE;
        let (x, y) = (mon.right - ((w + self.gap.0) * scale) as i32, mon.top + (self.gap.1 * scale) as i32);
        let mut answer = None;
        let (title, body) = (self.title.clone(), self.body.clone());
        let ui = &mut self.ui;
        self.surface.paint(x, y, w, h, |c, r| {
            c.round(r, 14.0, rgb(0x14161c).alpha(0.96));
            let mut u = Ui::begin(c, ui);
            let room = r.w - 28.0;
            let mut title = crate::i18n::tr(&title).into_owned();
            while measure(&title, 14.0, Weight::Semibold, None).0 > room && title.chars().count() > 1 {
                title.pop();
                title.pop();
                title.push('…');
            }
            c.text(rect(14.0, 10.0, room, 22.0), &title, 14.0, WHITE, Align::Left, Weight::Semibold);
            c.text(rect(14.0, 32.0, room, 20.0), &body, 13.0, rgb(0xd3d7df), Align::Left, Weight::Regular);
            let bw = 84.0;
            if u.button("pclose", rect(14.0, 64.0, bw, 34.0), "Fechar", true) {
                answer = Some(PromptAnswer::Close);
            }
            if u.button("plist", rect(108.0, 64.0, bw, 34.0), "Ver lista", false) {
                answer = Some(PromptAnswer::List);
            }
            if u.button("pnot", rect(202.0, 64.0, bw, 34.0), "Agora não", false) {
                answer = Some(PromptAnswer::Dismiss);
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
    lines: [Vec<(Vec<u32>, String)>; 2],
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
    hint: Vec<(Vec<u32>, String)>,
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
            self.groups = cfg.pad_radial[..crate::sys::radial::SLOTS].contains(&crate::sys::radial::GROUP);
            self.at = ((mon.left + mon.right) / 2 - (self.width() * scale / 2.0) as i32, (mon.top + mon.bottom) / 2 - (self.height() * scale / 2.0) as i32);
            for (i, &a) in cfg.pad_radial.iter().enumerate() {
                self.names[i] = if a == crate::sys::radial::PROGRAM {
                    cfg.pad_radial_prog[i].0.clone()
                } else {
                    crate::sys::remap::ACTIONS.get(a as usize).map_or(String::new(), |n| crate::i18n::tr(n).into_owned())
                };
            }
            self.icons = cfg.pad_radial;
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
        use crate::sys::radial::{sub, EMPTY, GROUP};
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
            if let Some(i) = sel.filter(|&i| groups && icons[i] == GROUP) {
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
