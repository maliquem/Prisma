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
    format!("{:.1} GB", b as f64 / (1u64 << 30) as f64).replace('.', ",")
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
    let t = windows_numerics::Matrix3x2::scale(k, k) * windows_numerics::Matrix3x2::translation(area.x, area.y);
    unsafe { c.rt.SetTransform(&t) };
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
        let text = text.to_string();
        let tw = measure(&text, 14.0, Weight::Regular, None).0 + 32.0;
        self.show(tw.max(120.0), 40.0, 56.0, move |c, r| {
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
    pub open: bool,
}

pub enum PromptAnswer {
    Close,
    List,
    Dismiss,
}

const PROMPT_SIZE: (f32, f32) = (360.0, 150.0);

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
        Self { surface: Layered::new(hwnd), ui: UiState::default(), title: String::new(), body: String::new(), open: false }
    }

    pub fn ask(&mut self, title: String, body: String) {
        self.title = title;
        self.body = body;
        self.open = true;
        self.redraw();
        unsafe {
            let _ = ShowWindow(self.surface.hwnd, SW_SHOWNOACTIVATE);
            SetTimer(Some(self.surface.hwnd), 1, 20000, None);
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
        let (x, y) = (mon.left + (24.0 * scale) as i32, mon.top + (96.0 * scale) as i32);
        let (w, h) = PROMPT_SIZE;
        let mut answer = None;
        let (title, body) = (self.title.clone(), self.body.clone());
        let ui = &mut self.ui;
        self.surface.paint(x, y, w, h, |c, r| {
            c.round(r, 14.0, rgb(0x14161c).alpha(0.96));
            let mut u = Ui::begin(c, ui);
            c.text(rect(16.0, 12.0, 328.0, 24.0), &title, 16.0, WHITE, Align::Left, Weight::Semibold);
            c.text_wrap(rect(16.0, 40.0, 328.0, 44.0), &body, 14.0, rgb(0xd3d7df), Weight::Regular);
            let bw = 104.0;
            if u.button("pclose", rect(16.0, 92.0, bw, 44.0), "Fechar", true) {
                answer = Some(PromptAnswer::Close);
            }
            if u.button("plist", rect(128.0, 92.0, bw, 44.0), "Ver lista", false) {
                answer = Some(PromptAnswer::List);
            }
            if u.button("pnot", rect(240.0, 92.0, bw, 44.0), "Agora não", false) {
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
