//! Mouse e teclado entre PCs: até três telas lado a lado (esquerda, meio, direita), cada uma num
//! PC com tela própria. O mouse passa da borda de uma tela para a vizinha e o teclado vai junto,
//! sem vídeo. Vale entre Prismas no Windows, pela mesma conexão do Compartilhar tela, com as
//! permissões de mouse e teclado que cada outro PC deu a este.
//!
//! Este PC vigia o próprio mouse e teclado com ganchos do Windows. Quando o mouse sai para outra
//! tela, o cursor daqui fica parado a uns pixels da borda e cada movimento vira posição na tela
//! de lá. Um atalho trava o mouse na tela em que ele está.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_LBUTTON, VK_MBUTTON, VK_MENU, VK_RBUTTON, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::*;
use crate::capture::mux::Codec;

/// Quanto o cursor daqui fica para dentro da borda enquanto o mouse está em outra tela.
const PARK_IN: i32 = 200;
const SILENCE: Duration = Duration::from_secs(5);

/// As posições das telas, da esquerda para a direita.
pub const SLOTS: usize = 3;

/// Lê uma posição: `esquerda`, `meio`, `direita` ou o número (0, 1, 2).
fn slot_parse(s: &str) -> Option<u8> {
    Some(match s.trim() {
        "esquerda" | "left" => 0,
        "meio" | "middle" => 1,
        "direita" | "right" => 2,
        n => return n.parse::<u8>().ok().filter(|n| (*n as usize) < SLOTS),
    })
}

fn slot_name(s: u8) -> &'static str {
    match s {
        0 => "esquerda",
        1 => "meio",
        _ => "direita",
    }
}

/// Em qual tela o mouse está.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Screen {
    Local,
    /// Um PC pareado, pelo índice na lista dos alvos.
    Remote(usize),
}

/// Um PC pareado no arranjo.
struct Remote {
    slot: u8,
    /// Tamanho do monitor de lá, em pixels, depois que a conexão abre.
    size: Option<(i32, i32)>,
    perms: u8,
    /// O que ficou apertado lá, para soltar quando o mouse sai.
    keys: HashSet<(u16, bool)>,
    buttons: u8,
}

/// O que os ganchos e as conexões dividem.
struct State {
    local_slot: u8,
    remotes: Vec<Remote>,
    at: Screen,
    /// O atalho travou o mouse na tela em que ele está.
    locked: bool,
    lock_key: Keys,
    /// A tecla do atalho desceu: a subida dela também fica aqui.
    lock_held: bool,
    /// Onde o cursor daqui fica parado enquanto o mouse está fora, e o monitor por onde ele saiu.
    park: POINT,
    mon: RECT,
    /// Posição do mouse na tela de fora.
    rx: i32,
    ry: i32,
    /// Eventos para mandar, com o índice do PC.
    pending: Vec<(usize, Input)>,
    /// Teclas apertadas aqui antes de o mouse sair: soltam aqui.
    local_keys: HashSet<u32>,
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

fn down(vk: u16) -> bool {
    unsafe { GetAsyncKeyState(vk as i32) as u16 & 0x8000 != 0 }
}

/// Fração de `v` entre `a` e `b` (os dois inclusos).
fn frac(v: i32, a: i32, b: i32) -> f64 {
    ((v - a) as f64 / (b - a).max(1) as f64).clamp(0.0, 1.0)
}

impl State {
    fn new(local_slot: u8, slots: &[u8], lock_key: Keys) -> Self {
        Self {
            local_slot,
            remotes: slots.iter().map(|&slot| Remote { slot, size: None, perms: 0, keys: HashSet::new(), buttons: 0 }).collect(),
            at: Screen::Local,
            locked: false,
            lock_key,
            lock_held: false,
            park: POINT::default(),
            mon: RECT::default(),
            rx: 0,
            ry: 0,
            pending: Vec::new(),
            local_keys: HashSet::new(),
        }
    }

    fn slot(&self, s: Screen) -> u8 {
        match s {
            Screen::Local => self.local_slot,
            Screen::Remote(i) => self.remotes[i].slot,
        }
    }

    /// A tela vizinha de `from` para a direita (ou a esquerda), pulando posições vazias e PCs
    /// ainda sem conexão.
    fn next(&self, from: Screen, right: bool) -> Option<Screen> {
        let s = self.slot(from) as i32;
        let step = if right { 1 } else { -1 };
        let mut k = s + step;
        while (0..SLOTS as i32).contains(&k) {
            if self.local_slot as i32 == k {
                return Some(Screen::Local);
            }
            if let Some(i) = self.remotes.iter().position(|r| r.slot as i32 == k && r.size.is_some()) {
                return Some(Screen::Remote(i));
            }
            k += step;
        }
        None
    }

    fn size(&self, i: usize) -> (i32, i32) {
        self.remotes[i].size.unwrap_or((2, 2))
    }

    fn send_move(&mut self) {
        let Screen::Remote(i) = self.at else { return };
        let (w, h) = self.size(i);
        let x = (self.rx as i64 * 65535 / (w - 1).max(1) as i64) as u16;
        let y = (self.ry as i64 * 65535 / (h - 1).max(1) as i64) as u16;
        self.pending.push((i, Input::Move { x, y }));
    }

    /// Solta o que ficou apertado no PC de fora em que o mouse estava.
    fn release_here(&mut self) {
        let Screen::Remote(i) = self.at else { return };
        let r = &mut self.remotes[i];
        let keys: Vec<_> = r.keys.drain().collect();
        let buttons = std::mem::take(&mut r.buttons);
        for (scan, extended) in keys {
            self.pending.push((i, Input::Key { scan, extended, down: false }));
        }
        for b in 0..5u8 {
            if buttons & 1 << b != 0 {
                self.pending.push((i, Input::Button { button: b, down: false }));
            }
        }
    }

    /// O mouse vai para a tela `to`, entrando pela borda do lado de onde veio (andando para a
    /// direita, entra pela esquerda), na altura relativa `fy`.
    fn enter(&mut self, to: Screen, right: bool, fy: f64) {
        self.release_here();
        // O PC de onde o mouse sai esconde o cursor de lá.
        if let Screen::Remote(i) = self.at {
            if to != self.at {
                self.pending.push((i, Input::Away));
            }
        }
        self.at = to;
        match to {
            Screen::Local => {
                super::cursor::show();
                let v = virtual_screen();
                let x = if right { v.left + 2 } else { v.right - 3 };
                let m = monitor_at(POINT { x, y: self.park.y });
                let y = m.top + (fy * (m.bottom - m.top - 1) as f64) as i32;
                set_cursor(x, y);
            }
            Screen::Remote(i) => {
                // O cursor daqui fica parado perto da borda: some enquanto o mouse está lá.
                super::cursor::hide();
                let (w, h) = self.size(i);
                self.rx = if right { 0 } else { w - 1 };
                self.ry = (fy * (h - 1) as f64) as i32;
                self.send_move();
            }
        }
    }

    /// O mouse sai desta tela pelo ponto `p`, rumo à tela `to`.
    fn leave(&mut self, p: POINT, to: Screen, right: bool) {
        let m = monitor_at(p);
        let fy = frac(p.y, m.top, m.bottom - 1);
        let x = if right { p.x - PARK_IN } else { p.x + PARK_IN };
        self.park = POINT { x: x.clamp(m.left + 1, m.right - 2), y: p.y.clamp(m.top + 1, m.bottom - 2) };
        self.mon = m;
        set_cursor(self.park.x, self.park.y);
        self.enter(to, right, fy);
    }

    /// Volta o mouse para cá (a conexão caiu ou acabou), pela borda do lado daquele PC.
    fn come_back(&mut self) {
        if let Screen::Remote(i) = self.at {
            let (_, h) = self.size(i);
            let fy = frac(self.ry, 0, h - 1);
            let right = self.remotes[i].slot < self.local_slot;
            self.enter(Screen::Local, right, fy);
        }
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
            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => match self.at {
                Screen::Remote(i) => {
                    self.pending.push((i, Input::Wheel { delta: (m.mouseData >> 16) as u16 as i16, horizontal: msg == WM_MOUSEHWHEEL }));
                    true
                }
                Screen::Local => false,
            },
            _ => false,
        }
    }

    fn moved(&mut self, p: POINT) -> bool {
        let Screen::Remote(i) = self.at else {
            // Travado, ou arrastando algo: o mouse não passa.
            if self.locked || [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|v| down(v.0)) {
                return false;
            }
            let v = virtual_screen();
            let right = if p.x >= v.right - 1 {
                true
            } else if p.x <= v.left {
                false
            } else {
                return false;
            };
            return match self.next(Screen::Local, right) {
                Some(to) => {
                    self.leave(p, to, right);
                    true
                }
                None => false,
            };
        };
        let (dx, dy) = (p.x - self.park.x, p.y - self.park.y);
        if dx == 0 && dy == 0 {
            return true;
        }
        let (w, h) = self.size(i);
        self.rx += dx;
        self.ry = (self.ry + dy).clamp(0, h - 1);
        if !self.locked && (self.rx < 0 || self.rx > w - 1) {
            let right = self.rx > w - 1;
            if let Some(to) = self.next(self.at, right) {
                let fy = frac(self.ry, 0, h - 1);
                self.enter(to, right, fy);
                return true;
            }
        }
        self.rx = self.rx.clamp(0, w - 1);
        self.send_move();
        true
    }

    fn button(&mut self, b: u8, pressed: bool) -> bool {
        let Screen::Remote(i) = self.at else { return false };
        let r = &mut self.remotes[i];
        if pressed {
            r.buttons |= 1 << b;
        } else {
            r.buttons &= !(1 << b);
        }
        self.pending.push((i, Input::Button { button: b, down: pressed }));
        true
    }

    /// O atalho de travar: liga e desliga a trava, e não vai para lugar nenhum.
    fn lock_key_event(&mut self, vk: u32, up: bool, mods: u32) -> bool {
        if vk != self.lock_key.1 {
            return false;
        }
        if up {
            return std::mem::take(&mut self.lock_held);
        }
        if self.lock_held {
            return true;
        }
        if mods != self.lock_key.0 {
            return false;
        }
        self.lock_held = true;
        self.locked = !self.locked;
        eprintln!("{}", if self.locked { "mouse travado nesta tela" } else { "mouse destravado" });
        super::notice(if self.locked { "Mouse travado nesta tela" } else { "Mouse destravado" }, "");
        true
    }

    fn key(&mut self, k: &KBDLLHOOKSTRUCT) -> bool {
        let up = k.flags.0 & LLKHF_UP.0 != 0;
        let vk = k.vkCode;
        let mods = mods_now(down(VK_CONTROL.0), down(VK_MENU.0), down(VK_SHIFT.0));
        if self.lock_key_event(vk, up, mods) {
            return true;
        }
        let Screen::Remote(i) = self.at else {
            if up {
                self.local_keys.remove(&vk);
            } else {
                self.local_keys.insert(vk);
            }
            return false;
        };
        // Apertada aqui antes de o mouse sair: a repetição e o soltar ficam aqui.
        if self.local_keys.contains(&vk) {
            if up {
                self.local_keys.remove(&vk);
            }
            return false;
        }
        let r = &mut self.remotes[i];
        if r.perms & KEYBOARD == 0 {
            return false;
        }
        let key = (k.scanCode as u16, k.flags.0 & LLKHF_EXTENDED.0 != 0);
        if up {
            r.keys.remove(&key);
        } else {
            r.keys.insert(key);
        }
        self.pending.push((i, Input::Key { scan: key.0, extended: key.1, down: !up }));
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

/// Lê o arranjo: `eu|meio,nome|direita,nome|esquerda`. Sem `eu`, este PC fica na posição
/// das opções do Cliente; um PC sem posição vai para a primeira livre ao lado deste.
fn layout(arg: &str, default_me: u8) -> (u8, Vec<(String, u8)>) {
    let mut me = None;
    let mut list: Vec<(String, Option<u8>)> = Vec::new();
    for item in arg.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, slot) = match item.rsplit_once('|') {
            Some((n, s)) => (n.trim(), slot_parse(s)),
            None => (item, None),
        };
        if name == "eu" {
            me = slot;
        } else {
            list.push((name.to_string(), slot));
        }
    }
    let me = me.unwrap_or(default_me);
    let mut taken = vec![me];
    let mut out = Vec::new();
    for (name, slot) in &list {
        if let Some(s) = slot.filter(|s| !taken.contains(s)) {
            taken.push(s);
            out.push((name.clone(), s));
        }
    }
    for (name, slot) in list {
        if out.iter().any(|o| o.0 == name) {
            continue;
        }
        if slot.is_some() {
            eprintln!("«{name}» ficou fora: a posição dele já tem outra tela");
            continue;
        }
        // Do lado deste PC primeiro: direita, esquerda, e depois o que sobrar.
        let order = [me as i32 + 1, me as i32 - 1, me as i32 + 2, me as i32 - 2];
        match order.into_iter().filter(|s| (0..SLOTS as i32).contains(s)).map(|s| s as u8).find(|s| !taken.contains(s)) {
            Some(s) => {
                taken.push(s);
                out.push((name, s));
            }
            None => eprintln!("«{name}» ficou fora: as três posições já têm tela"),
        }
    }
    (me, out)
}

/// `--stream-mouse=eu|meio,nome|direita,...`: usa o mouse e o teclado daqui nos PCs pareados,
/// cada um na sua posição.
pub fn control(arg: &str) {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let co = ClientOpts::load();
    let (me, targets) = layout(arg, co.screens.iter().position(|s| s == "eu").unwrap_or(1) as u8);
    if targets.is_empty() {
        return eprintln!("nenhum PC para usar o mouse e o teclado daqui");
    }
    run(me, targets, co.key_lock);
}

/// Uma conexão aberta com um PC do arranjo.
struct Peer {
    link: Arc<dyn Link>,
    name: String,
    /// Avisa a linha da conexão que pode fechar.
    done: Arc<AtomicBool>,
    history: VecDeque<Input>,
    first: u32,
    last_input: Instant,
    last_send: Instant,
    last_ping: Instant,
    heard: Instant,
    perms: u8,
    lat: super::LatLog,
}

enum Event {
    Up { i: usize, link: Arc<dyn Link>, name: String, w: u32, h: u32, perms: u8, done: Arc<AtomicBool> },
    /// A linha da conexão acabou (não conectou, foi recusada ou fechou).
    Gone(usize),
}

/// Pede mouse e teclado ao host e espera o Info. Devolve tamanho e permissões.
fn handshake(link: &Arc<dyn Link>, name: &str) -> Option<(u32, u32, u8)> {
    let bye = || {
        let mut out = Vec::new();
        write(&mut out, &Msg::Bye);
        link.send(&out);
        link.send(&out);
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
    loop {
        if start.elapsed() > Duration::from_secs(15) {
            eprintln!("«{name}» não respondeu ao pedido de mouse e teclado");
            bye();
            return None;
        }
        if last_hello.elapsed() > Duration::from_secs(1) {
            link.send(&hello);
            last_hello = Instant::now();
        }
        match link.recv(&mut buf, Duration::from_millis(100)) {
            Poll::Got(n) => match read(&buf[..n]) {
                Some(Msg::Info { fps, .. }) if fps != 0 => {
                    eprintln!("«{name}» tem uma versão do Prisma sem mouse e teclado entre PCs: atualize o Prisma lá");
                    bye();
                    return None;
                }
                Some(Msg::Info { perms, .. }) if perms & MOUSE == 0 => {
                    eprintln!("«{name}» não deu a este PC a permissão de mouse");
                    bye();
                    return None;
                }
                Some(Msg::Info { w, h, perms, .. }) => return Some((w, h, perms)),
                Some(Msg::Bye) => {
                    eprintln!("«{name}» recusou: este PC não tem a permissão de mouse lá");
                    return None;
                }
                _ => {}
            },
            Poll::Closed => {
                eprintln!("a conexão com «{name}» fechou");
                return None;
            }
            _ => {}
        }
    }
}

fn run(me: u8, targets: Vec<(String, u8)>, lock_key: Keys) {
    let slots: Vec<u8> = targets.iter().map(|t| t.1).collect();
    *STATE.lock().unwrap() = Some(State::new(me, &slots, lock_key));
    // Cada PC conecta na sua linha; a conexão fica aberta até esta avisar.
    let (tx, rx) = mpsc::channel::<Event>();
    for (i, (target, _)) in targets.iter().enumerate() {
        let (tx, target) = (tx.clone(), target.clone());
        let _ = std::thread::Builder::new().name(format!("mouse-teclado-{i}")).spawn(move || {
            super::net::with_host(&target, |link, name| {
                let Some((w, h, perms)) = handshake(&link, name) else { return };
                let done = Arc::new(AtomicBool::new(false));
                let _ = tx.send(Event::Up { i, link, name: name.to_string(), w, h, perms, done: done.clone() });
                while !done.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
            });
            let _ = tx.send(Event::Gone(i));
        });
    }
    drop(tx);
    let hook_thread = std::thread::Builder::new().name("mouse-teclado-ganchos".into()).spawn(hooks).ok();
    eprintln!("este PC fica na posição {}; {} trava o mouse na tela em que ele está", slot_name(me), crate::config::Hotkey::new(lock_key.0, lock_key.1).label());

    let mut peers: Vec<Option<Peer>> = (0..targets.len()).map(|_| None).collect();
    let mut lines = targets.len();
    let mut out = Vec::with_capacity(64);
    let mut pkt = Vec::with_capacity(INPUT_SIZE * INPUT_KEEP + 16);
    let mut buf = vec![0u8; 2048];
    let mut clip: Option<super::clip::Clip> = None;
    let mut clip_in = super::clip::Pieces::default();
    let mut clip_id = 0u16;
    let send_input = |p: &mut Peer, pkt: &mut Vec<u8>| {
        let list: Vec<Input> = p.history.iter().copied().collect();
        write_input(pkt, p.first, &list);
        p.link.send(pkt);
        p.last_send = Instant::now();
    };
    // Fecha a conexão com um PC: o mouse volta se estava lá, e o que ficou apertado é solto.
    let drop_peer = |peers: &mut Vec<Option<Peer>>, i: usize, why: &str, out: &mut Vec<u8>, pkt: &mut Vec<u8>| {
        let Some(mut p) = peers[i].take() else { return };
        eprintln!("«{}» {why}", p.name);
        let fresh: Vec<Input> = {
            let mut g = STATE.lock().unwrap();
            let s = g.as_mut().unwrap();
            if s.at == Screen::Remote(i) {
                s.come_back();
            }
            s.remotes[i].size = None;
            let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut s.pending).into_iter().partition(|e| e.0 == i);
            s.pending = rest;
            mine.into_iter().map(|e| e.1).collect()
        };
        if !fresh.is_empty() {
            for e in fresh {
                push(&mut p, e);
            }
            send_input(&mut p, pkt);
            send_input(&mut p, pkt);
        }
        write(out, &Msg::Bye);
        p.link.send(out);
        p.link.send(out);
        p.done.store(true, Ordering::Relaxed);
    };
    while lines > 0 {
        while let Ok(ev) = rx.try_recv() {
            match ev {
                Event::Up { i, link, name, w, h, perms, done } => {
                    let now_ = Instant::now();
                    if let Some(s) = STATE.lock().unwrap().as_mut() {
                        let r = &mut s.remotes[i];
                        r.size = Some((w.max(2) as i32, h.max(2) as i32));
                        r.perms = perms;
                    }
                    eprintln!("pronto: «{name}» ({w}×{h}) fica na posição {}", slot_name(slots[i]));
                    if perms & KEYBOARD == 0 {
                        eprintln!("«{name}» não deu a permissão de teclado: lá só vai o mouse");
                    }
                    peers[i] = Some(Peer { lat: super::LatLog::new("").of(&name), link, name, done, history: VecDeque::new(), first: 0, last_input: now_, last_send: now_, last_ping: now_ - Duration::from_secs(1), heard: now_, perms });
                }
                Event::Gone(i) => {
                    lines -= 1;
                    drop_peer(&mut peers, i, "saiu", &mut out, &mut pkt);
                }
            }
        }
        if peers.iter().all(Option::is_none) {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let copy = peers.iter().flatten().any(|p| p.perms & COPY != 0);
        if clip.is_none() && copy {
            clip = Some(super::clip::start());
        }
        if let Some(c) = &clip {
            while let Ok(t) = c.changed.try_recv() {
                clip_id = clip_id.wrapping_add(1);
                let pk = super::clip::packets(&t, clip_id);
                for p in peers.iter().flatten().filter(|p| p.perms & COPY != 0) {
                    for k in pk.iter().chain(pk.iter()) {
                        p.link.send(k);
                    }
                }
            }
        }
        // Os eventos novos vão na hora; os recentes vão de novo por um instante, contra perda.
        let fresh = STATE.lock().ok().and_then(|mut g| g.as_mut().map(|s| std::mem::take(&mut s.pending))).unwrap_or_default();
        for (i, e) in fresh {
            if let Some(p) = peers[i].as_mut() {
                push(p, e);
                p.last_input = Instant::now();
            }
        }
        let mut gone: Vec<(usize, &str)> = Vec::new();
        for (i, slot) in peers.iter_mut().enumerate() {
            let Some(p) = slot else { continue };
            if p.last_ping.elapsed() > Duration::from_millis(500) {
                write(&mut out, &Msg::Ping { t: now() });
                p.link.send(&out);
                p.last_ping = Instant::now();
            }
            if !p.history.is_empty() && (p.last_input > p.last_send || (p.last_input.elapsed() < Duration::from_millis(300) && p.last_send.elapsed() > Duration::from_millis(30))) {
                send_input(p, &mut pkt);
            }
            match p.link.recv(&mut buf, Duration::from_millis(1)) {
                Poll::Got(n) => {
                    p.heard = Instant::now();
                    match read(&buf[..n]) {
                        Some(Msg::Pong { t, .. }) => p.lat.add((now() - t) as f64 / 10_000.0, None),
                        Some(Msg::Bye) => gone.push((i, "encerrou a conexão")),
                        Some(Msg::Info { perms, .. }) => {
                            p.perms = perms;
                            if let Some(s) = STATE.lock().unwrap().as_mut() {
                                s.remotes[i].perms = perms;
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
                Poll::Closed => gone.push((i, "fechou a conexão")),
                _ => {}
            }
            if p.heard.elapsed() > SILENCE {
                gone.push((i, "parou de responder"));
            }
        }
        for (i, why) in gone {
            drop_peer(&mut peers, i, why, &mut out, &mut pkt);
        }
    }
    *STATE.lock().unwrap() = None;
    super::cursor::show();
    if let Some(t) = hook_thread {
        while HOOK_THREAD.load(Ordering::SeqCst) == 0 && !t.is_finished() {
            std::thread::sleep(Duration::from_millis(5));
        }
        unsafe {
            let _ = PostThreadMessageW(HOOK_THREAD.load(Ordering::SeqCst), WM_QUIT, WPARAM(0), LPARAM(0));
        }
        let _ = t.join();
    }
}

fn push(p: &mut Peer, e: Input) {
    p.history.push_back(e);
    if p.history.len() > INPUT_KEEP {
        p.history.pop_front();
        p.first = p.first.wrapping_add(1);
    }
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
        &Msg::Info { codec: VideoCodec::Std(Codec::H264), w: (r.right - r.left) as u32, h: (r.bottom - r.top) as u32, fps: 0, perms, recover: false, hdr10: false, yuv444: false, priority: 255 },
    );
    link.send(&info);
    let mut inj = host::Injector::new(mon, perms);
    host::watch_host_input();
    // Desde quando o cursor daqui está escondido (o mouse foi para outro PC).
    let mut hidden_at: Option<i64> = None;
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
                    Some(Msg::Input { first, events }) => {
                        // O evento mais novo diz onde o mouse está: saiu daqui, ou voltou.
                        match events.chunks(INPUT_SIZE).filter_map(Input::get).last() {
                            Some(Input::Away) if hidden_at.is_none() => {
                                super::cursor::hide();
                                hidden_at = Some(host::ms_now());
                            }
                            Some(Input::Away) => {}
                            Some(_) if hidden_at.is_some() => {
                                super::cursor::show();
                                hidden_at = None;
                            }
                            _ => {}
                        }
                        inj.apply(first, events)
                    }
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
        // O mouse daqui se mexeu: o cursor volta para quem está neste PC.
        if hidden_at.is_some_and(|t| host::HOST_MOUSE_MS.load(Ordering::Relaxed) > t) {
            super::cursor::show();
            hidden_at = None;
        }
        if heard.elapsed() > SILENCE {
            break;
        }
    }
    super::cursor::show();
    inj.release();
    clip.take();
    eprintln!("«{name}» parou de usar o mouse e o teclado daqui");
    crate::config::log(&format!("compartilhar: «{name}» parou de usar o mouse e o teclado deste PC"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MOD_CTRL;

    fn connected(s: &mut State) {
        for r in &mut s.remotes {
            r.size = Some((1920, 1080));
            r.perms = MOUSE | KEYBOARD;
        }
    }

    #[test]
    fn back_to_this_pc() {
        // Este PC no meio, um à direita.
        let mut s = State::new(1, &[2], (0, 0x91));
        connected(&mut s);
        s.park = POINT { x: 100, y: 100 };
        s.enter(Screen::Remote(0), true, 0.5);
        assert_eq!((s.at, s.rx, s.ry), (Screen::Remote(0), 0, 539));
        assert!(s.moved(POINT { x: 150, y: 100 }));
        assert_eq!(s.rx, 50);
        // Muito para a direita: não há tela, para na borda.
        s.moved(POINT { x: 100 + 5000, y: 100 });
        assert_eq!(s.rx, 1919);
        s.remotes[0].buttons = 1;
        s.remotes[0].keys.insert((30, false));
        s.pending.clear();
        // Para a esquerda além da borda: volta para cá, soltando o que ficou apertado.
        s.moved(POINT { x: 100 - 3000, y: 100 });
        assert_eq!(s.at, Screen::Local);
        assert!(s.pending.contains(&(0, Input::Button { button: 0, down: false })));
        assert!(s.pending.contains(&(0, Input::Key { scan: 30, extended: false, down: false })));
    }

    #[test]
    fn from_screen_to_screen() {
        // Este PC à esquerda, um no meio e outro à direita: do meio passa direto para a direita.
        let mut s = State::new(0, &[1, 2], (0, 0x91));
        connected(&mut s);
        s.park = POINT { x: 100, y: 100 };
        s.enter(Screen::Remote(0), true, 0.0);
        s.remotes[0].buttons = 2;
        s.pending.clear();
        s.moved(POINT { x: 100 + 2000, y: 100 });
        assert_eq!((s.at, s.rx), (Screen::Remote(1), 0));
        assert!(s.pending.contains(&(0, Input::Button { button: 1, down: false })));
        assert!(matches!(s.pending.last(), Some((1, Input::Move { .. }))));
        // Sem conexão com o do meio, a direita fica ao lado deste.
        s.remotes[0].size = None;
        assert_eq!(s.next(Screen::Local, true), Some(Screen::Remote(1)));
        assert_eq!(s.next(Screen::Remote(1), false), Some(Screen::Local));
        assert_eq!(s.next(Screen::Local, false), None);
    }

    #[test]
    fn lock_keeps_the_mouse() {
        let mut s = State::new(1, &[0], (MOD_CTRL, 0x4C));
        connected(&mut s);
        s.park = POINT { x: 100, y: 100 };
        s.enter(Screen::Remote(0), false, 0.5);
        // Sem o Ctrl, a tecla não é o atalho.
        assert!(!s.lock_key_event(0x4C, false, 0));
        assert!(s.lock_key_event(0x4C, false, MOD_CTRL));
        assert!(s.lock_key_event(0x4C, true, MOD_CTRL));
        assert!(s.locked);
        s.moved(POINT { x: 100 + 3000, y: 100 });
        assert_eq!((s.at, s.rx), (Screen::Remote(0), 1919));
        s.lock_key_event(0x4C, false, MOD_CTRL);
        s.lock_key_event(0x4C, true, MOD_CTRL);
        s.moved(POINT { x: 100 + 10, y: 100 });
        assert_eq!(s.at, Screen::Local);
    }

    #[test]
    fn layout_and_slots() {
        assert_eq!(slot_parse("esquerda"), Some(0));
        assert_eq!(slot_parse("2"), Some(2));
        assert_eq!(slot_parse("x"), None);
        assert_eq!(layout("eu|esquerda,a|direita,b", 1), (0, vec![("a".into(), 2), ("b".into(), 1)]));
        // Sem posição: ao lado deste, primeiro à direita.
        assert_eq!(layout("a,b", 1), (1, vec![("a".into(), 2), ("b".into(), 0)]));
        // Posição repetida: o segundo fica fora.
        assert_eq!(layout("a|meio,b|meio", 0), (0, vec![("a".into(), 1)]));
        assert_eq!(layout("a,b,c", 1).1.len(), 2);
    }
}
