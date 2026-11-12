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
        self.info_at(text, 56.0);
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
    /// Seta desenhada agora: corre atrás do analógico a cada quadro, para andar suave.
    arrow: (f32, f32),
    /// Canto na tela e as duas linhas de ajuda, fixos enquanto o teclado fica aberto.
    at: (i32, i32),
    lines: (String, String),
    ticking: bool,
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
        Self { surface: Layered::new(hwnd), arrow: (0.0, 0.0), at: (0, 0), lines: Default::default(), ticking: false }
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
            self.at = ((mon.left + mon.right) / 2 - (KB_W * scale / 2.0) as i32, mon.bottom - ((KB_H + 48.0) * scale) as i32);
            // Sem controle na lista (ainda não leu nenhum), os nomes do Xbox.
            let f = pads::list().first().map_or(pads::Family::Xbox, |p| p.family);
            let l = |b: u32| crate::pages::pad_profiles::button_label(f, b);
            let fill = |t: &str, labels: &[String]| labels.iter().fold(crate::i18n::tr(t).into_owned(), |s, b| s.replacen("{}", b, 1));
            self.lines = (
                fill("{} apaga · {} espaço · {} Enter · direcional: setas", &[l(pads::LB), l(pads::RB), l(pads::START)]),
                fill("{} maiúsculas · {} números · os dois: acentos · {} fecha", &[l(crate::sys::remap::RT), l(crate::sys::remap::LT), l(pads::BACK)]),
            );
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

    /// Um quadro: a seta anda 45% do caminho até o analógico e só redesenha se mexeu.
    pub fn tick(&mut self) {
        if ease(&mut self.arrow, crate::sys::osk::stick()) {
            self.paint();
        }
    }

    fn paint(&mut self) {
        let v = crate::sys::osk::view();
        let (ax, ay) = self.arrow;
        let (line1, line2) = (&self.lines.0, &self.lines.1);
        let mode = match (v.sym, v.shift) {
            (true, true) => "áç",
            (true, false) => "123",
            (false, true) => "ABC",
            (false, false) => "abc",
        };
        self.surface.paint(self.at.0, self.at.1, KB_W, KB_H, |c, r| {
            c.round(r, 18.0, rgb(0x0d0f14).alpha(0.92));
            let (cx, cy, ring, pr) = (r.w / 2.0, 176.0, 116.0, 42.0);
            stick_arrow(c, cx, cy, (ax, ay));
            c.text_raw(rect(cx - 30.0, cy - 12.0, 60.0, 24.0), mode, 15.0, TEXT2, Align::Center, Weight::Semibold);
            for p in 0..8 {
                let a = (p as f32 * 45.0).to_radians();
                let (px, py) = (cx + ring * a.sin(), cy - ring * a.cos());
                let on = v.petal == Some(p);
                c.circle(px, py, pr, if on { ACCENT.alpha(0.35) } else { WHITE.alpha(0.07) });
                // Mesma posição dos botões: X à esquerda, Y em cima, B à direita, A embaixo.
                for (i, (dx, dy)) in [(-22.0, 0.0), (0.0, -22.0), (22.0, 0.0), (0.0, 22.0)].into_iter().enumerate() {
                    let (size, color, weight) = if on { (17.0, WHITE, Weight::Semibold) } else { (15.0, TEXT2, Weight::Regular) };
                    c.text_raw(rect(px + dx - 12.0, py + dy - 11.0, 24.0, 22.0), &v.char_at(p, i).to_string(), size, color, Align::Center, weight);
                }
            }
            c.text_fit(rect(16.0, 342.0, r.w - 32.0, 20.0), line1, 12.0, MUTED, Align::Center, Weight::Regular);
            c.text_fit(rect(16.0, 362.0, r.w - 32.0, 20.0), line2, 12.0, MUTED, Align::Center, Weight::Regular);
        });
    }
}

/// Radial do Prisma (`sys::radial`): oito ações em volta, no meio da tela.
pub struct Radial {
    surface: Layered,
    /// Setas do stick direito e do esquerdo.
    arrow: [(f32, f32); 2],
    at: (i32, i32),
    names: [String; crate::sys::radial::SLOTS],
    icons: [u8; crate::sys::radial::SLOTS],
    two: bool,
    left: bool,
    hint: String,
    ticking: bool,
}

/// Largura de um círculo; com dois, a janela tem os dois lado a lado.
const RD_W: f32 = 600.0;
const RD_H: f32 = 460.0;

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
        Self { surface: Layered::new(hwnd), arrow: [(0.0, 0.0); 2], at: (0, 0), names: Default::default(), icons: [0; crate::sys::radial::SLOTS], two: false, left: false, hint: String::new(), ticking: false }
    }

    /// Mostra o estado de agora, ou esconde se fechou. As ações e os programas vêm da configuração.
    pub fn update(&mut self, cfg: &crate::config::Config) {
        use crate::sys::pads;
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
            self.at = ((mon.left + mon.right) / 2 - (self.width() * scale / 2.0) as i32, (mon.top + mon.bottom) / 2 - (RD_H * scale / 2.0) as i32);
            for (i, &a) in cfg.pad_radial.iter().enumerate() {
                self.names[i] = if a == crate::sys::radial::PROGRAM {
                    cfg.pad_radial_prog[i].0.clone()
                } else {
                    crate::sys::remap::ACTIONS.get(a as usize).map_or(String::new(), |n| crate::i18n::tr(n).into_owned())
                };
            }
            self.icons = cfg.pad_radial;
            let f = pads::list().first().map_or(pads::Family::Xbox, |p| p.family);
            let label = |b| crate::pages::pad_profiles::button_label(f, b);
            self.hint = if v.stay {
                crate::i18n::tr("{} dispara · {} fecha").replacen("{}", &label(pads::A), 1).replacen("{}", &label(pads::B), 1)
            } else if v.watch == 0 {
                crate::i18n::tr("Solte o botão para escolher · com o stick no centro, cancela").into_owned()
            } else {
                crate::i18n::tr("Solte {} para escolher · com o stick no centro, cancela").replacen("{}", &label(v.watch), 1)
            };
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
        if self.two { RD_W * 2.0 - 40.0 } else { RD_W }
    }

    fn paint(&mut self) {
        let sel = crate::sys::radial::view().sel;
        let (names, icons, hint, arrow, two, left) = (&self.names, &self.icons, &self.hint, self.arrow, self.two, self.left);
        self.surface.paint(self.at.0, self.at.1, self.width(), RD_H, |c, r| {
            c.round(r, 18.0, rgb(0x0d0f14).alpha(0.92));
            let cy = 210.0;
            // Com dois círculos, o do stick esquerdo (lugares 8 a 15) fica à esquerda.
            let rings: &[(f32, usize)] = if two { &[(RD_W / 2.0 - 10.0, 8), (r.w - RD_W / 2.0 + 10.0, 0)] } else { &[(RD_W / 2.0, 0)] };
            for &(cx, first) in rings {
            stick_arrow(c, cx, cy, arrow[if left { 1 } else { first / 8 }]);
            for (i, name) in names.iter().enumerate().skip(first).take(8) {
                let a = ((i % 8) as f32 * 45.0).to_radians();
                let (px, py) = (cx + 210.0 * a.sin(), cy - 160.0 * a.cos());
                let b = rect(px - 70.0, py - 40.0, 140.0, 80.0);
                if name.is_empty() {
                    c.round_line(b, 12.0, WHITE.alpha(0.12), 1.0);
                    continue;
                }
                let on = sel == Some(i);
                c.round(b, 12.0, if on { ACCENT.alpha(0.35) } else { WHITE.alpha(0.07) });
                // Ícone em cima, nome embaixo (até três linhas).
                c.icon(px - 10.0, b.y + 7.0, 20.0, crate::pages::action_icon(icons[i]), if on { WHITE } else { ACCENT });
                c.text_wrap_center(rect(b.x + 6.0, b.y + 31.0, b.w - 12.0, 46.0), name, 11.0, if on { WHITE } else { TEXT2 }, if on { Weight::Semibold } else { Weight::Regular });
            }
            }
            c.text_fit(rect(16.0, 426.0, r.w - 32.0, 20.0), hint, 12.0, MUTED, Align::Center, Weight::Regular);
        });
    }
}
