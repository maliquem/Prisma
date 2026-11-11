//! Na janela de quem assiste: o medidor de latência, o aviso de conexão ruim e o menu da conexão.
//!
//! O medidor e o aviso ficam numa camada transparente presa à janela (some com ela e não recebe
//! cliques). O menu é desenhado como o resto do Prisma e funciona com o mouse e com o teclado.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT, VK_SPACE, VK_TAB, VK_UP};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::i18n::tr;
use crate::ui::gfx::{measure, rect, rgb, Align, Weight, MUTED, TEXT, WARN};
use crate::ui::layered::Layered;

/// O que a janela mediu no último segundo.
#[derive(Default, Clone)]
pub struct Sample {
    pub rtt_ms: f64,
    pub e2e_ms: f64,
    pub decode_ms: f64,
    pub fps: f64,
    pub mbps: f64,
    /// Quadros perdidos neste segundo e no total.
    pub lost: u32,
    pub lost_total: u32,
    pub shown: u32,
    /// Ex.: `1920x1080 AV1 HDR10`.
    pub video: String,
    pub route: String,
}

/// Quanto tempo o aviso fica na tela.
const WARN_FOR: Duration = Duration::from_secs(6);

pub struct Hud {
    layer: Layered,
    visible: bool,
    warn: Option<(&'static str, Instant)>,
    rtts: VecDeque<f64>,
    rising: u32,
}

fn green() -> crate::ui::gfx::Color {
    rgb(0x6bd38a)
}

impl Hud {
    pub fn new(owner: HWND) -> Option<Self> {
        unsafe {
            let inst: HINSTANCE = GetModuleHandleW(None).ok()?.into();
            let class = w!("PrismaAssistirMedidor");
            unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
                DefWindowProcW(h, m, w, l)
            }
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst, lpszClassName: class, ..Default::default() };
            RegisterClassW(&wc);
            let ex = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
            // Dona é a janela de quem assiste: a camada fica sempre logo acima dela, e só dela.
            let h = CreateWindowExW(ex, class, w!(""), WS_POPUP, 0, 0, 1, 1, Some(owner), None, Some(inst), None).ok()?;
            Some(Hud { layer: Layered::new(h), visible: false, warn: None, rtts: VecDeque::new(), rising: 0 })
        }
    }

    /// Confere a conexão no segundo que passou. Devolve o aviso novo, se houver.
    fn check(&mut self, s: &Sample) -> Option<&'static str> {
        let frames = s.shown + s.lost;
        if frames >= 10 && s.lost as f64 / frames as f64 > 0.02 {
            return Some("Conexão instável: a rede está perdendo quadros");
        }
        if s.rtt_ms > 0.0 {
            self.rtts.push_back(s.rtt_ms);
            if self.rtts.len() > 5 {
                self.rtts.pop_front();
            }
            let (lo, hi) = self.rtts.iter().fold((f64::MAX, 0.0f64), |(l, h), &v| (l.min(v), h.max(v)));
            let prev = self.rtts.iter().rev().nth(1).copied();
            self.rising = match prev {
                Some(p) if s.rtt_ms > p + 1.0 => self.rising + 1,
                _ => 0,
            };
            if self.rtts.len() >= 4 && hi - lo > 10.0 {
                return Some("Conexão instável: a latência está variando");
            }
            if self.rising >= 3 {
                self.rising = 0;
                return Some("Conexão piorando: a latência está subindo");
            }
        }
        None
    }

    /// Uma vez por segundo, e quando a janela muda.
    pub fn update(&mut self, owner: HWND, meter: u8, warn_on: bool, s: Option<&Sample>) {
        if let Some(s) = s {
            if let Some(w) = self.check(s) {
                if warn_on {
                    self.warn = Some((w, Instant::now()));
                }
            }
        }
        if self.warn.is_some_and(|(_, t)| t.elapsed() > WARN_FOR) || !warn_on {
            self.warn = None;
        }
        let iconic = unsafe { IsIconic(owner).as_bool() };
        let mut lines: Vec<(String, crate::ui::gfx::Color, f32, Weight)> = Vec::new();
        if let Some(s) = s.filter(|_| meter > 0) {
            let ms = if s.e2e_ms > 0.0 { s.e2e_ms } else { s.rtt_ms };
            if meter == 1 {
                lines.push((format!("{ms:.0} ms"), TEXT, 15.0, Weight::Semibold));
            } else {
                let c = crate::i18n::comma();
                let n = |v: f64, d: usize| format!("{v:.d$}").replace('.', c);
                lines.push((format!("{} ms {} · {} {} ms", n(s.e2e_ms, 1), tr("ponta a ponta"), tr("rede"), n(s.rtt_ms, 1)), TEXT, 14.0, Weight::Semibold));
                // Quadros por segundo mostrados aqui, em linha própria.
                lines.push((format!("{:.0} FPS · {} Mbit/s", s.fps, n(s.mbps, 1)), TEXT, 14.0, Weight::Semibold));
                lines.push((s.video.clone(), TEXT, 13.0, Weight::Regular));
                lines.push((format!("{} {} ms · {} {}", tr("decodificação"), n(s.decode_ms, 2), tr("perdidos"), s.lost_total), MUTED, 13.0, Weight::Regular));
                lines.push((s.route.clone(), MUTED, 13.0, Weight::Regular));
            }
        }
        if let Some((w, _)) = self.warn {
            lines.push((tr(w).into_owned(), WARN, 14.0, Weight::Semibold));
        }
        if lines.is_empty() || iconic {
            if self.visible {
                unsafe {
                    let _ = ShowWindow(self.layer.hwnd, SW_HIDE);
                }
                self.visible = false;
            }
            return;
        }
        let dot = meter == 1;
        let w = lines.iter().map(|(t, _, size, wt)| measure(t, *size, *wt, None).0).fold(0.0f32, f32::max) + 28.0 + if dot { 18.0 } else { 0.0 };
        let h = lines.iter().map(|l| l.2 + 8.0).sum::<f32>() + 16.0;
        let mut p = POINT { x: 12, y: 12 };
        unsafe {
            let _ = ClientToScreen(owner, &mut p);
        }
        let ms = s.map_or(0.0, |s| if s.e2e_ms > 0.0 { s.e2e_ms } else { s.rtt_ms });
        let dot_color = if ms < 30.0 {
            green()
        } else if ms < 60.0 {
            WARN
        } else {
            rgb(0xff5a52)
        };
        self.layer.paint(p.x, p.y, w, h, |c, r| {
            c.round(r, 10.0, rgb(0x101317).alpha(0.82));
            let mut y = r.y + 8.0;
            for (i, (t, col, size, wt)) in lines.iter().enumerate() {
                let lh = size + 8.0;
                let mut x = r.x + 14.0;
                if dot && i == 0 {
                    c.circle(x + 5.0, y + lh / 2.0, 5.0, dot_color);
                    x += 18.0;
                }
                c.text_raw(rect(x, y, r.right() - x - 10.0, lh), t, *size, *col, Align::Left, *wt);
                y += lh;
            }
        });
        if !self.visible {
            unsafe {
                let _ = ShowWindow(self.layer.hwnd, SW_SHOWNOACTIVATE);
            }
            self.visible = true;
        }
    }
}

impl Drop for Hud {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.layer.hwnd);
        }
    }
}

/// O que o menu mostra.
pub struct MenuState {
    pub fullscreen: bool,
    /// Modo imersivo agora (0 desligado, 1 mouse, 2 teclado, 3 os dois).
    pub immersive: u8,
    /// Quadros por segundo pedidos (0 automático) e os Hz desta tela, o teto da lista.
    pub fps: u32,
    pub max_hz: u32,
    pub meter: u8,
    pub warn: bool,
    pub sound: bool,
    pub keyboard: bool,
    pub video: String,
    /// Os atalhos, já escritos: menu, tela cheia, modo imersivo, medidor, som, Ctrl+Alt+Del, desconectar.
    pub keys: [String; 7],
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Pick {
    Fullscreen,
    Immersive(u8),
    ImmersiveToggle,
    Fps(u32),
    Meter(u8),
    MeterCycle,
    Warn,
    Sound,
    CtrlAltDel,
    Disconnect,
}

/// Uma área que se clica no menu.
struct Hit {
    r: crate::ui::gfx::Rect,
    pick: Pick,
    enabled: bool,
}

/// O menu aberto: o que foi desenhado, onde está o mouse e o que foi escolhido.
struct Open {
    layer: Layered,
    hits: Vec<Hit>,
    hover: Option<usize>,
    done: bool,
    pick: Option<Pick>,
    pos: (i32, i32),
    size: (f32, f32),
}

thread_local! {
    static MENU: std::cell::RefCell<Option<Open>> = const { std::cell::RefCell::new(None) };
    static MENU_STATE: std::cell::RefCell<Option<MenuState>> = const { std::cell::RefCell::new(None) };
    /// A janela do menu aberto (0: fechado).
    static MENU_HWND: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
}

const MENU_W: f32 = 460.0;
const PAD: f32 = 16.0;
const ROW: f32 = 34.0;
const SEG: f32 = 30.0;

/// As opções de cada linha com escolhas: (texto, escolha, marcada).
fn segments(st: &MenuState, row: usize) -> Vec<(String, Pick, bool)> {
    match row {
        0 => ["Desligado", "Mouse", "Teclado", "Os dois"].iter().enumerate().map(|(i, t)| (tr(t).into_owned(), Pick::Immersive(i as u8), st.immersive == i as u8)).collect(),
        1 => crate::stream::CLIENT_FPS
            .iter()
            .filter(|&&f| f == 0 || f <= st.max_hz.max(30))
            .map(|&f| (if f == 0 { tr("Automático").into_owned() } else { f.to_string() }, Pick::Fps(f), st.fps == f))
            .collect(),
        _ => ["Desligado", "Simples", "Detalhado"].iter().enumerate().map(|(i, t)| (tr(t).into_owned(), Pick::Meter(i as u8), st.meter == i as u8)).collect(),
    }
}

/// Altura do menu para o que ele mostra.
fn menu_height(st: &MenuState) -> f32 {
    let head = if st.video.is_empty() { 44.0 } else { 64.0 };
    let segs = 3.0 * (26.0 + SEG + 10.0);
    head + ROW + segs + 3.0 * ROW + 13.0 + ROW + PAD
}

/// Desenha o menu e devolve as áreas que se clicam.
fn draw_menu(c: &crate::ui::gfx::Canvas, r: crate::ui::gfx::Rect, st: &MenuState, hover: Option<usize>) -> Vec<Hit> {
    use crate::ui::gfx::{rect as rr, ACCENT, BORDER, BORDER2, CARD, CARD2, LINE, ON_ACCENT, REC, TEXT2};
    let mut hits = Vec::new();
    c.round(r, 12.0, CARD);
    c.round_line(r, 12.0, BORDER, 1.0);
    let x = r.x + PAD;
    let w = r.w - 2.0 * PAD;
    c.text(rr(x, r.y + 12.0, w, 22.0), "Conexão", 15.0, TEXT, Align::Left, Weight::Semibold);
    c.text_raw(rr(x, r.y + 12.0, w, 22.0), &st.keys[0], 12.0, MUTED, Align::Right, Weight::Regular);
    let mut y = r.y + 38.0;
    if !st.video.is_empty() {
        c.text_raw(rr(x, y, w, 18.0), &st.video, 12.0, MUTED, Align::Left, Weight::Regular);
        y += 20.0;
    }
    y += 6.0;
    let hovered = |hits: &Vec<Hit>| hover == Some(hits.len());
    // Linha que liga e desliga, com a caixa de marcar.
    let toggle = |hits: &mut Vec<Hit>, y: f32, label: &str, key: &str, on: bool, pick: Pick| {
        let row = rr(r.x + 6.0, y, r.w - 12.0, ROW);
        if hovered(hits) {
            c.round(row, 8.0, CARD2);
        }
        let b = rr(x, y + (ROW - 16.0) / 2.0, 16.0, 16.0);
        if on {
            c.round(b, 4.0, ACCENT);
            c.line(b.x + 3.5, b.y + 8.5, b.x + 6.5, b.y + 11.5, ON_ACCENT, 2.0);
            c.line(b.x + 6.5, b.y + 11.5, b.x + 12.5, b.y + 4.5, ON_ACCENT, 2.0);
        } else {
            c.round_line(b, 4.0, BORDER2, 1.5);
        }
        c.text(rr(x + 26.0, y, w - 26.0, ROW), label, 14.0, TEXT, Align::Left, Weight::Regular);
        c.text_raw(rr(x, y, w, ROW), key, 12.0, MUTED, Align::Right, Weight::Regular);
        hits.push(Hit { r: row, pick, enabled: true });
    };
    // Linha de ação, sem caixa.
    let action = |hits: &mut Vec<Hit>, y: f32, label: &str, key: &str, enabled: bool, color: crate::ui::gfx::Color, pick: Pick| {
        let row = rr(r.x + 6.0, y, r.w - 12.0, ROW);
        if enabled && hovered(hits) {
            c.round(row, 8.0, CARD2);
        }
        let col = if enabled { color } else { MUTED.alpha(0.6) };
        c.text(rr(x, y, w, ROW), label, 14.0, col, Align::Left, Weight::Regular);
        c.text_raw(rr(x, y, w, ROW), key, 12.0, MUTED, Align::Right, Weight::Regular);
        hits.push(Hit { r: row, pick, enabled });
    };

    toggle(&mut hits, y, "Tela cheia", &st.keys[1], st.fullscreen, Pick::Fullscreen);
    y += ROW;
    for (i, (label, key)) in [("Modo imersivo", st.keys[2].as_str()), ("Quadros por segundo", ""), ("Medidor de latência", st.keys[3].as_str())].iter().enumerate() {
        c.text(rr(x, y + 4.0, w, 22.0), label, 13.0, TEXT2, Align::Left, Weight::Regular);
        c.text_raw(rr(x, y + 4.0, w, 22.0), key, 12.0, MUTED, Align::Right, Weight::Regular);
        y += 26.0;
        let segs = segments(st, i);
        let n = segs.len() as f32;
        let gap = 6.0;
        let sw = ((w - gap * (n - 1.0)) / n).floor();
        for (j, (text, pick, on)) in segs.into_iter().enumerate() {
            let s = rr(x + j as f32 * (sw + gap), y, sw, SEG);
            let hov = hovered(&hits);
            let (bg, fg) = if on {
                (ACCENT, ON_ACCENT)
            } else if hov {
                (CARD2, TEXT)
            } else {
                (LINE.alpha(0.6), TEXT2)
            };
            c.round(s, 7.0, bg);
            if hov && on {
                c.round_line(s, 7.0, TEXT.alpha(0.5), 1.0);
            }
            c.text_raw(s, &text, 13.0, fg, Align::Center, if on { Weight::Semibold } else { Weight::Regular });
            hits.push(Hit { r: s, pick, enabled: true });
        }
        y += SEG + 10.0;
    }
    toggle(&mut hits, y, "Avisar quando a conexão piorar", "", st.warn, Pick::Warn);
    y += ROW;
    toggle(&mut hits, y, "Som", &st.keys[4], st.sound, Pick::Sound);
    y += ROW;
    action(&mut hits, y, "Mandar Ctrl+Alt+Del", &st.keys[5], st.keyboard, TEXT, Pick::CtrlAltDel);
    y += ROW + 6.0;
    c.line(x, y, x + w, y, LINE, 1.0);
    y += 7.0;
    action(&mut hits, y, "Desconectar", &st.keys[6], true, REC, Pick::Disconnect);
    hits
}

fn repaint(o: &mut Open) {
    MENU_STATE.with(|s| {
        let s = s.borrow();
        let Some(st) = s.as_ref() else { return };
        let hover = o.hover;
        let mut hits = Vec::new();
        o.layer.paint(o.pos.0, o.pos.1, o.size.0, o.size.1, |c, r| hits = draw_menu(c, r, st, hover));
        o.hits = hits;
    });
}

unsafe extern "system" fn menu_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let handled = MENU.with(|cell| {
        let Ok(mut b) = cell.try_borrow_mut() else { return false };
        let Some(o) = b.as_mut() else { return false };
        let scale = o.layer.scale.max(0.5);
        let at = |l: LPARAM| ((l.0 & 0xFFFF) as i16 as f32 / scale, ((l.0 >> 16) & 0xFFFF) as i16 as f32 / scale);
        match m {
            WM_MOUSEMOVE => {
                let (x, y) = at(l);
                let hover = o.hits.iter().position(|t| t.enabled && t.r.contains(x, y));
                if hover != o.hover {
                    o.hover = hover;
                    repaint(o);
                }
                true
            }
            WM_LBUTTONUP => {
                let (x, y) = at(l);
                if let Some(t) = o.hits.iter().find(|t| t.enabled && t.r.contains(x, y)) {
                    o.pick = Some(t.pick);
                    o.done = true;
                }
                true
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                let n = o.hits.len();
                let step = |from: Option<usize>, fwd: bool| -> Option<usize> {
                    let mut i = from.unwrap_or(if fwd { n - 1 } else { 0 });
                    for _ in 0..n {
                        i = if fwd { (i + 1) % n } else { (i + n - 1) % n };
                        if o.hits[i].enabled {
                            return Some(i);
                        }
                    }
                    None
                };
                match VIRTUAL_KEY(w.0 as u16) {
                    VK_ESCAPE => o.done = true,
                    VK_DOWN | VK_RIGHT | VK_TAB if n > 0 => {
                        o.hover = step(o.hover, true);
                        repaint(o);
                    }
                    VK_UP | VK_LEFT if n > 0 => {
                        o.hover = step(o.hover, false);
                        repaint(o);
                    }
                    VK_RETURN | VK_SPACE => {
                        if let Some(t) = o.hover.and_then(|i| o.hits.get(i)) {
                            o.pick = Some(t.pick);
                            o.done = true;
                        }
                    }
                    _ => {}
                }
                true
            }
            WM_ACTIVATE if (w.0 & 0xFFFF) as u32 == WA_INACTIVE => {
                o.done = true;
                true
            }
            _ => false,
        }
    });
    if handled {
        LRESULT(0)
    } else {
        DefWindowProcW(h, m, w, l)
    }
}

/// O menu da conexão, no meio da janela de quem assiste. Fecha com um clique fora, Esc ou uma
/// escolha; as setas e o Enter também escolhem.
/// Abre o menu sem parar a janela de quem assiste: o laço dela segue mostrando os quadros
/// e entrega as mensagens do menu, e `menu_poll` traz a escolha quando ele fecha.
pub fn menu_open(owner: HWND, st: MenuState) -> bool {
    if menu_is_open() {
        return false;
    }
    unsafe {
        let Ok(inst) = GetModuleHandleW(None) else { return false };
        let inst: HINSTANCE = inst.into();
        let class = w!("PrismaAssistirMenu");
        let wc = WNDCLASSW { lpfnWndProc: Some(menu_proc), hInstance: inst, lpszClassName: class, hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(), ..Default::default() };
        RegisterClassW(&wc);
        let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
        let Ok(h) = CreateWindowExW(ex, class, w!(""), WS_POPUP, 0, 0, 1, 1, Some(owner), None, Some(inst), None) else { return false };
        let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(owner) as f32 / 96.0;
        let size = (MENU_W, menu_height(&st));
        let mut rc = RECT::default();
        let _ = GetWindowRect(owner, &mut rc);
        let (pw, ph) = ((size.0 * scale) as i32, (size.1 * scale) as i32);
        let x = (rc.left + rc.right - pw) / 2;
        let y = ((rc.top + rc.bottom - ph) / 2).max(rc.top);
        MENU_STATE.with(|s| *s.borrow_mut() = Some(st));
        MENU.with(|cell| {
            let mut o = Open { layer: Layered::new(h), hits: Vec::new(), hover: None, done: false, pick: None, pos: (x, y), size };
            repaint(&mut o);
            *cell.borrow_mut() = Some(o);
        });
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetForegroundWindow(h);
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(h));
        MENU_HWND.with(|c| c.set(h.0 as isize));
        true
    }
}

pub fn menu_is_open() -> bool {
    MENU_HWND.with(|c| c.get()) != 0
}

/// Menu fechado agora: `Some(escolha)`, uma vez. Aberto ou sem menu: `None`.
pub fn menu_poll(owner: HWND) -> Option<Option<Pick>> {
    if !menu_is_open() || !MENU.with(|c| c.borrow().as_ref().is_none_or(|o| o.done)) {
        return None;
    }
    let pick = MENU.with(|c| c.borrow_mut().take()).and_then(|o| o.pick);
    MENU_STATE.with(|s| *s.borrow_mut() = None);
    let h = MENU_HWND.with(|c| c.replace(0));
    unsafe {
        let _ = DestroyWindow(HWND(h as *mut _));
        let _ = SetForegroundWindow(owner);
    }
    Some(pick)
}

/// Teste: o menu com valores de exemplo, sem nada sob o mouse e com a terceira área marcada.
#[cfg(debug_assertions)]
pub fn test_menu(dir: &str) {
    if std::env::args().any(|a| a == "--test-en") {
        crate::i18n::set_english(true);
    }
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let Ok(inst) = GetModuleHandleW(None) else { return };
        let inst: HINSTANCE = inst.into();
        let class = w!("PrismaAssistirMenuTeste");
        unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst, lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let Ok(h) = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOOLWINDOW, class, w!(""), WS_POPUP, -4000, -4000, 1, 1, None, None, Some(inst), None) else { return };
        let key = |m: u32, k: u32| crate::config::Hotkey::new(m, k).label();
        let all = crate::config::MOD_CTRL | crate::config::MOD_ALT | crate::config::MOD_SHIFT;
        for (i, hover) in [None, Some(2usize), Some(12)].into_iter().enumerate() {
            let st = MenuState {
                fullscreen: true,
                immersive: 3,
                fps: 0,
                max_hz: 120,
                meter: 1,
                warn: true,
                sound: true,
                keyboard: i != 2,
                video: "3840x2160 AV1 HDR10 · 120 q/s · direto pela internet, 12 ms".into(),
                keys: [key(all, b'P' as u32), key(all, b'F' as u32), key(all, b'I' as u32), key(all, b'M' as u32), key(all, b'S' as u32), key(crate::config::MOD_CTRL | crate::config::MOD_ALT, 0x23), key(all, b'D' as u32)],
            };
            let mut layer = Layered::new(h);
            std::env::set_var("PRISMA_LAYER_DUMP", dir);
            let size = (MENU_W, menu_height(&st));
            layer.paint(-4000, -4000, size.0, size.1, |c, r| {
                draw_menu(c, r, &st, hover);
            });
            let from = std::path::Path::new(dir).join(format!("camada_{}.png", (size.0 * layer.scale).ceil() as i32));
            let _ = std::fs::rename(&from, std::path::Path::new(dir).join(format!("menu_{i}.png")));
        }
        let _ = DestroyWindow(h);
    }
}
