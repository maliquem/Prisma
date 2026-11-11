//! Mouse e teclado entre PCs: o mouse passa da borda desta tela para a tela de um PC pareado, e o
//! teclado vai junto, sem vídeo (os dois PCs têm tela própria). Vale entre dois Prismas no
//! Windows, pela mesma conexão do Compartilhar tela, com as permissões de mouse e teclado que o
//! outro PC deu a este.
//!
//! Quem controla vigia o próprio mouse e teclado com ganchos do Windows. Ao passar da borda
//! escolhida, o cursor daqui fica parado a uns pixels da borda e cada movimento vira posição na
//! tela do outro PC. Voltar pela borda de lá, ou apertar Scroll Lock, devolve o cursor.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON, VK_SCROLL};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::*;
use crate::capture::mux::Codec;

/// Quanto o cursor daqui fica para dentro da borda enquanto o mouse está no outro PC.
const PARK_IN: i32 = 200;
const SILENCE: Duration = Duration::from_secs(5);

/// De que lado desta tela fica a tela do outro PC.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Side {
    Right,
    Left,
    Up,
    Down,
}

impl Side {
    fn from(n: u8) -> Self {
        match n {
            1 => Side::Left,
            2 => Side::Up,
            3 => Side::Down,
            _ => Side::Right,
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "direita" | "right" => Side::Right,
            "esquerda" | "left" => Side::Left,
            "acima" | "up" => Side::Up,
            "abaixo" | "down" => Side::Down,
            _ => return s.parse::<u8>().ok().filter(|n| *n < 4).map(Side::from),
        })
    }

    fn name(self) -> &'static str {
        match self {
            Side::Right => "direita",
            Side::Left => "esquerda",
            Side::Up => "de cima",
            Side::Down => "de baixo",
        }
    }
}

/// O que os ganchos e a conexão dividem.
struct State {
    side: Side,
    perms: u8,
    /// Tamanho do monitor do outro PC, em pixels.
    rw: i32,
    rh: i32,
    /// O mouse está no outro PC.
    away: bool,
    /// Onde o cursor daqui fica parado enquanto isso, e o monitor por onde ele saiu.
    park: POINT,
    mon: RECT,
    /// Posição do mouse lá.
    rx: i32,
    ry: i32,
    /// Eventos para mandar.
    pending: Vec<Input>,
    /// Teclas apertadas aqui antes de o mouse sair: soltam aqui.
    local_keys: HashSet<u32>,
    /// O que ficou apertado lá, para soltar na volta.
    remote_keys: HashSet<(u16, bool)>,
    remote_buttons: u8,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);

fn virtual_screen() -> RECT {
    unsafe {
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        RECT { left: x, top: y, right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN), bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN) }
    }
}

fn monitor_rect(mon: HMONITOR) -> RECT {
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let _ = GetMonitorInfoW(mon, &mut mi);
    }
    mi.rcMonitor
}

fn monitor_at(p: POINT) -> RECT {
    monitor_rect(unsafe { MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST) })
}

/// Os testes não mexem no cursor de verdade.
fn set_cursor(x: i32, y: i32) {
    #[cfg(not(test))]
    unsafe {
        let _ = SetCursorPos(x, y);
    }
    #[cfg(test)]
    let _ = (x, y);
}

/// Fração de `v` entre `a` e `b` (os dois inclusos).
fn frac(v: i32, a: i32, b: i32) -> f64 {
    ((v - a) as f64 / (b - a).max(1) as f64).clamp(0.0, 1.0)
}

impl State {
    fn new(side: Side, rw: u32, rh: u32, perms: u8) -> Self {
        Self {
            side,
            perms,
            rw: rw.max(2) as i32,
            rh: rh.max(2) as i32,
            away: false,
            park: POINT::default(),
            mon: RECT::default(),
            rx: 0,
            ry: 0,
            pending: Vec::new(),
            local_keys: HashSet::new(),
            remote_keys: HashSet::new(),
            remote_buttons: 0,
        }
    }

    fn at_edge(&self, p: POINT) -> bool {
        let v = virtual_screen();
        match self.side {
            Side::Right => p.x >= v.right - 1,
            Side::Left => p.x <= v.left,
            Side::Up => p.y <= v.top,
            Side::Down => p.y >= v.bottom - 1,
        }
    }

    fn send_move(&mut self) {
        let x = (self.rx as i64 * 65535 / (self.rw - 1) as i64) as u16;
        let y = (self.ry as i64 * 65535 / (self.rh - 1) as i64) as u16;
        self.pending.push(Input::Move { x, y });
    }

    /// O mouse passa para o outro PC, entrando pela borda oposta, na mesma altura relativa.
    fn leave(&mut self, p: POINT) {
        let m = monitor_at(p);
        let fx = frac(p.x, m.left, m.right - 1);
        let fy = frac(p.y, m.top, m.bottom - 1);
        let (rmx, rmy) = (self.rw - 1, self.rh - 1);
        (self.rx, self.ry, self.park) = match self.side {
            Side::Right => (0, (fy * rmy as f64) as i32, POINT { x: p.x - PARK_IN, y: p.y }),
            Side::Left => (rmx, (fy * rmy as f64) as i32, POINT { x: p.x + PARK_IN, y: p.y }),
            Side::Up => ((fx * rmx as f64) as i32, rmy, POINT { x: p.x, y: p.y + PARK_IN }),
            Side::Down => ((fx * rmx as f64) as i32, 0, POINT { x: p.x, y: p.y - PARK_IN }),
        };
        self.park.x = self.park.x.clamp(m.left + 1, m.right - 2);
        self.park.y = self.park.y.clamp(m.top + 1, m.bottom - 2);
        self.mon = m;
        self.away = true;
        set_cursor(self.park.x, self.park.y);
        self.send_move();
    }

    /// O mouse volta: solta o que ficou apertado lá e põe o cursor na borda daqui, na altura de lá.
    fn come_back(&mut self) {
        for (scan, extended) in self.remote_keys.drain().collect::<Vec<_>>() {
            self.pending.push(Input::Key { scan, extended, down: false });
        }
        for b in 0..5u8 {
            if self.remote_buttons & 1 << b != 0 {
                self.pending.push(Input::Button { button: b, down: false });
            }
        }
        self.remote_buttons = 0;
        self.away = false;
        let m = self.mon;
        let fx = frac(self.rx, 0, self.rw - 1);
        let fy = frac(self.ry, 0, self.rh - 1);
        let x = m.left + (fx * (m.right - m.left - 1) as f64) as i32;
        let y = m.top + (fy * (m.bottom - m.top - 1) as f64) as i32;
        let (x, y) = match self.side {
            Side::Right => (m.right - 3, y),
            Side::Left => (m.left + 2, y),
            Side::Up => (x, m.top + 2),
            Side::Down => (x, m.bottom - 3),
        };
        set_cursor(x, y);
    }

    /// Devolve verdadeiro para o evento não chegar a este PC.
    fn mouse(&mut self, msg: u32, m: &MSLLHOOKSTRUCT) -> bool {
        match msg {
            WM_MOUSEMOVE => self.moved(m.pt),
            WM_LBUTTONDOWN | WM_LBUTTONUP => self.button(0, msg == WM_LBUTTONDOWN),
            WM_RBUTTONDOWN | WM_RBUTTONUP => self.button(1, msg == WM_RBUTTONDOWN),
            WM_MBUTTONDOWN | WM_MBUTTONUP => self.button(2, msg == WM_MBUTTONDOWN),
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                let b = if (m.mouseData >> 16) as u16 == XBUTTON2 { 4 } else { 3 };
                self.button(b, msg == WM_XBUTTONDOWN)
            }
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL if self.away => {
                self.pending.push(Input::Wheel { delta: (m.mouseData >> 16) as u16 as i16, horizontal: msg == WM_MOUSEHWHEEL });
                true
            }
            _ => false,
        }
    }

    fn moved(&mut self, p: POINT) -> bool {
        if !self.away {
            // Arrastando algo, o mouse não passa.
            let held = unsafe { [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|v| GetAsyncKeyState(v.0 as i32) as u16 & 0x8000 != 0) };
            if !held && self.at_edge(p) {
                self.leave(p);
                return true;
            }
            return false;
        }
        let (dx, dy) = (p.x - self.park.x, p.y - self.park.y);
        if dx == 0 && dy == 0 {
            return true;
        }
        self.rx += dx;
        self.ry += dy;
        let back = match self.side {
            Side::Right => self.rx < 0,
            Side::Left => self.rx > self.rw - 1,
            Side::Up => self.ry > self.rh - 1,
            Side::Down => self.ry < 0,
        };
        if back {
            self.come_back();
            return true;
        }
        self.rx = self.rx.clamp(0, self.rw - 1);
        self.ry = self.ry.clamp(0, self.rh - 1);
        self.send_move();
        true
    }

    fn button(&mut self, b: u8, down: bool) -> bool {
        if !self.away {
            return false;
        }
        if down {
            self.remote_buttons |= 1 << b;
        } else {
            self.remote_buttons &= !(1 << b);
        }
        self.pending.push(Input::Button { button: b, down });
        true
    }

    fn key(&mut self, k: &KBDLLHOOKSTRUCT) -> bool {
        let up = k.flags.0 & LLKHF_UP.0 != 0;
        let vk = k.vkCode;
        if !self.away {
            if up {
                self.local_keys.remove(&vk);
            } else {
                self.local_keys.insert(vk);
            }
            return false;
        }
        if vk == VK_SCROLL.0 as u32 {
            if !up {
                self.come_back();
            }
            return true;
        }
        // Apertada aqui antes de o mouse sair: a repetição e o soltar ficam aqui.
        if self.local_keys.contains(&vk) {
            if up {
                self.local_keys.remove(&vk);
            }
            return false;
        }
        if self.perms & KEYBOARD == 0 {
            return false;
        }
        let key = (k.scanCode as u16, k.flags.0 & LLKHF_EXTENDED.0 != 0);
        if up {
            self.remote_keys.remove(&key);
        } else {
            self.remote_keys.insert(key);
        }
        self.pending.push(Input::Key { scan: key.0, extended: key.1, down: !up });
        true
    }
}

unsafe extern "system" fn mouse_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let m = &*(l.0 as *const MSLLHOOKSTRUCT);
        if let Ok(mut g) = STATE.lock() {
            if g.as_mut().is_some_and(|s| s.mouse(w.0 as u32, m)) {
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, w, l)
}

unsafe extern "system" fn key_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let k = &*(l.0 as *const KBDLLHOOKSTRUCT);
        if let Ok(mut g) = STATE.lock() {
            if g.as_mut().is_some_and(|s| s.key(k)) {
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, w, l)
}

/// Os ganchos precisam de uma thread com fila de mensagens; WM_QUIT encerra.
fn hooks() {
    unsafe {
        let mut msg = MSG::default();
        // Cria a fila antes de dizer quem é, para o WM_QUIT não se perder.
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        HOOK_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        let m = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0);
        let k = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_hook), None, 0);
        if m.is_err() || k.is_err() {
            eprintln!("não deu para vigiar o mouse e o teclado deste PC");
        }
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        if let Ok(h) = m {
            let _ = UnhookWindowsHookEx(h);
        }
        if let Ok(h) = k {
            let _ = UnhookWindowsHookEx(h);
        }
    }
}

// ---------- Quem controla ----------

/// `--stream-mouse=nome[|lado]`: usa o mouse e o teclado daqui no PC pareado `nome`. O lado
/// (direita, esquerda, acima, abaixo) vem das opções do Cliente quando não vem aqui.
pub fn control(arg: &str) {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let mut parts = arg.split('|');
    let target = parts.next().unwrap_or("").to_string();
    let side = parts.next().and_then(Side::parse).unwrap_or_else(|| Side::from(ClientOpts::load().mouse_side));
    super::net::with_host(&target, |link, name| run(link, name, side));
}

fn run(link: Arc<dyn Link>, name: &str, side: Side) {
    let mut out = Vec::with_capacity(64);
    let bye = |out: &mut Vec<u8>| {
        write(out, &Msg::Bye);
        link.send(out);
        link.send(out);
    };
    // Um host de versão anterior trata isto como quem assiste: vai a tela de verdade daqui,
    // para ele não estranhar, e ele se revela mandando vídeo (quadros por segundo acima de zero).
    let mon = monitor_rect(unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) });
    let mut hello = Vec::new();
    let (w, h) = ((mon.right - mon.left) as u32, (mon.bottom - mon.top) as u32);
    write(&mut hello, &Msg::Hello { w, h, hz: 60, codecs: codec_bit(Codec::H264), pid: std::process::id(), flags: NO_VIDEO });
    let mut buf = vec![0u8; 2048];
    let start = Instant::now();
    let mut last_hello = start - Duration::from_secs(2);
    let (rw, rh, perms) = loop {
        if start.elapsed() > Duration::from_secs(15) {
            eprintln!("«{name}» não respondeu ao pedido de mouse e teclado");
            return bye(&mut out);
        }
        if last_hello.elapsed() > Duration::from_secs(1) {
            link.send(&hello);
            last_hello = Instant::now();
        }
        match link.recv(&mut buf, Duration::from_millis(100)) {
            Poll::Got(n) => match read(&buf[..n]) {
                Some(Msg::Info { fps, .. }) if fps != 0 => {
                    eprintln!("«{name}» tem uma versão do Prisma sem mouse e teclado entre PCs: atualize o Prisma lá");
                    return bye(&mut out);
                }
                Some(Msg::Info { w, h, perms, .. }) => break (w, h, perms),
                Some(Msg::Bye) => return eprintln!("«{name}» recusou: este PC não tem a permissão de mouse lá"),
                _ => {}
            },
            Poll::Closed => return eprintln!("a conexão com «{name}» fechou"),
            _ => {}
        }
    };
    if perms & MOUSE == 0 {
        eprintln!("«{name}» não deu a este PC a permissão de mouse");
        return bye(&mut out);
    }
    eprintln!("pronto: passe o mouse pela borda {} da tela para usar «{name}» ({rw}×{rh}); Scroll Lock volta", side.name());
    if perms & KEYBOARD == 0 {
        eprintln!("sem a permissão de teclado lá: o teclado fica neste PC");
    }
    *STATE.lock().unwrap() = Some(State::new(side, rw, rh, perms));
    let hook_thread = std::thread::Builder::new().name("mouse-teclado-ganchos".into()).spawn(hooks).ok();

    let mut history: VecDeque<Input> = VecDeque::new();
    let mut input_first = 0u32;
    let mut input_pkt = Vec::with_capacity(INPUT_SIZE * INPUT_KEEP + 16);
    let (mut last_input, mut last_send) = (Instant::now(), Instant::now());
    let mut last_ping = Instant::now() - Duration::from_secs(1);
    let mut heard = Instant::now();
    let mut clip: Option<super::clip::Clip> = None;
    let mut clip_in = super::clip::Pieces::default();
    let mut clip_id = 0u16;
    let mut perms = perms;
    let flush = |history: &mut VecDeque<Input>, input_first: &mut u32, fresh: Vec<Input>| {
        for e in fresh {
            history.push_back(e);
            if history.len() > INPUT_KEEP {
                history.pop_front();
                *input_first = input_first.wrapping_add(1);
            }
        }
    };
    loop {
        if last_ping.elapsed() > Duration::from_millis(500) {
            write(&mut out, &Msg::Ping { t: now() });
            link.send(&out);
            last_ping = Instant::now();
        }
        if clip.is_none() && perms & COPY != 0 {
            clip = Some(super::clip::start());
        }
        if let Some(c) = &clip {
            while let Ok(t) = c.changed.try_recv() {
                clip_id = clip_id.wrapping_add(1);
                let pk = super::clip::packets(&t, clip_id);
                for p in pk.iter().chain(pk.iter()) {
                    link.send(p);
                }
            }
        }
        // Os eventos novos vão na hora; os recentes vão de novo por um instante, contra perda.
        let fresh = STATE.lock().ok().and_then(|mut g| g.as_mut().map(|s| std::mem::take(&mut s.pending))).unwrap_or_default();
        if !fresh.is_empty() {
            flush(&mut history, &mut input_first, fresh);
            last_input = Instant::now();
        }
        if !history.is_empty() && (last_input > last_send || (last_input.elapsed() < Duration::from_millis(300) && last_send.elapsed() > Duration::from_millis(30))) {
            let list: Vec<Input> = history.iter().copied().collect();
            write_input(&mut input_pkt, input_first, &list);
            link.send(&input_pkt);
            last_send = Instant::now();
        }
        match link.recv(&mut buf, Duration::from_millis(2)) {
            Poll::Got(n) => {
                heard = Instant::now();
                match read(&buf[..n]) {
                    Some(Msg::Bye) => {
                        eprintln!("«{name}» encerrou a conexão");
                        break;
                    }
                    Some(Msg::Info { perms: p, .. }) => {
                        perms = p;
                        if let Some(s) = STATE.lock().unwrap().as_mut() {
                            s.perms = p;
                        }
                    }
                    Some(Msg::Clip { id, part, parts, data }) => {
                        if let (Some(c), Some(t)) = (&clip, clip_in.add(id, part, parts, data)) {
                            let _ = c.set.send(t);
                        }
                    }
                    _ => {}
                }
            }
            Poll::Closed => {
                eprintln!("a conexão com «{name}» fechou");
                break;
            }
            _ => {}
        }
        if heard.elapsed() > SILENCE {
            eprintln!("«{name}» parou de responder");
            break;
        }
    }
    // O cursor volta para cá e o que ficou apertado lá é solto.
    let last = STATE.lock().unwrap().take().map(|mut s| {
        if s.away {
            s.come_back();
        }
        s.pending
    });
    if let Some(fresh) = last.filter(|f| !f.is_empty()) {
        flush(&mut history, &mut input_first, fresh);
        let list: Vec<Input> = history.iter().copied().collect();
        write_input(&mut input_pkt, input_first, &list);
        link.send(&input_pkt);
        link.send(&input_pkt);
    }
    if let Some(t) = hook_thread {
        while HOOK_THREAD.load(Ordering::SeqCst) == 0 && !t.is_finished() {
            std::thread::sleep(Duration::from_millis(5));
        }
        unsafe {
            let _ = PostThreadMessageW(HOOK_THREAD.load(Ordering::SeqCst), WM_QUIT, WPARAM(0), LPARAM(0));
        }
        let _ = t.join();
    }
    bye(&mut out);
}

// ---------- Quem é controlado ----------

/// Um PC pareado usa o mouse e o teclado daqui, sem vídeo: os eventos vão para o monitor que
/// o Compartilhar tela transmitiria.
pub(super) fn serve(link: Arc<dyn Link>, perms: u8, name: String) {
    let mut out = Vec::with_capacity(64);
    if perms & MOUSE == 0 {
        eprintln!("«{name}» pediu mouse e teclado, mas não tem a permissão de mouse aqui");
        write(&mut out, &Msg::Bye);
        link.send(&out);
        link.send(&out);
        return;
    }
    let mon = host::pick_monitor(Opts::load().monitor);
    let r = monitor_rect(mon);
    let mut info = Vec::new();
    write(
        &mut info,
        &Msg::Info { codec: Codec::H264, w: (r.right - r.left) as u32, h: (r.bottom - r.top) as u32, fps: 0, perms, recover: false, hdr10: false, yuv444: false, priority: 255 },
    );
    link.send(&info);
    let mut inj = host::Injector::new(mon, perms);
    eprintln!("«{name}» usa o mouse e o teclado daqui");
    crate::config::log(&format!("compartilhar: «{name}» usa o mouse e o teclado deste PC"));
    let mut clip = (perms & COPY != 0).then(super::clip::start);
    let mut clip_in = super::clip::Pieces::default();
    let mut clip_id = 0u16;
    let mut buf = vec![0u8; 2048];
    let mut heard = Instant::now();
    loop {
        if let Some(c) = &clip {
            while let Ok(t) = c.changed.try_recv() {
                clip_id = clip_id.wrapping_add(1);
                let pk = super::clip::packets(&t, clip_id);
                for p in pk.iter().chain(pk.iter()) {
                    link.send(p);
                }
            }
        }
        match link.recv(&mut buf, Duration::from_millis(20)) {
            Poll::Got(n) => {
                heard = Instant::now();
                match read(&buf[..n]) {
                    // Repetido contra perda: a resposta vai de novo.
                    Some(Msg::Hello { .. }) => link.send(&info),
                    Some(Msg::Ping { t }) => {
                        write(&mut out, &Msg::Pong { t, host: now() });
                        link.send(&out);
                    }
                    Some(Msg::Input { first, events }) => inj.apply(first, events),
                    Some(Msg::Clip { id, part, parts, data }) => {
                        if let (Some(c), Some(t)) = (&clip, clip_in.add(id, part, parts, data)) {
                            let _ = c.set.send(t);
                        }
                    }
                    Some(Msg::Bye) => break,
                    _ => {}
                }
            }
            Poll::Closed => break,
            _ => {}
        }
        if heard.elapsed() > SILENCE {
            break;
        }
    }
    inj.release();
    clip.take();
    eprintln!("«{name}» parou de usar o mouse e o teclado daqui");
    crate::config::log(&format!("compartilhar: «{name}» parou de usar o mouse e o teclado deste PC"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crossing_and_back() {
        let mut s = State::new(Side::Right, 1920, 1080, MOUSE | KEYBOARD);
        s.away = true;
        s.park = POINT { x: 100, y: 100 };
        s.mon = RECT { left: 0, top: 0, right: 2560, bottom: 1440 };
        s.rx = 0;
        s.ry = 540;
        assert!(s.moved(POINT { x: 150, y: 100 }));
        assert_eq!((s.rx, s.ry), (50, 540));
        assert!(matches!(s.pending.last(), Some(Input::Move { .. })));
        // Muito para a direita: para na borda de lá.
        s.moved(POINT { x: 100 + 5000, y: 100 });
        assert_eq!(s.rx, 1919);
        s.remote_buttons = 1;
        s.remote_keys.insert((30, false));
        // Para a esquerda além da borda de lá: volta, soltando o que ficou apertado.
        s.pending.clear();
        s.moved(POINT { x: 100 - 3000, y: 100 });
        assert!(!s.away);
        assert!(s.pending.contains(&Input::Button { button: 0, down: false }));
        assert!(s.pending.contains(&Input::Key { scan: 30, extended: false, down: false }));
    }

    #[test]
    fn sides() {
        assert_eq!(Side::parse("esquerda"), Some(Side::Left));
        assert_eq!(Side::parse("3"), Some(Side::Down));
        assert_eq!(Side::parse("x"), None);
    }
}
