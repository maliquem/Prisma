//! Prisma em foco: o controle navega pela janela principal. O direcional leva o cursor de controle
//! em controle, A escolhe, B volta, LB/RB trocam de página, LT/RT de aba e Y esconde o Prisma; como
//! no perfil Desktop, o analógico esquerdo é o mouse (X é o clique) e o direito rola. O mapeamento só
//! vê a parada de emergência e o botão do radial.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::pads::{self, Input};
use super::remap::{self, Act, Out};

/// Janela principal do Prisma (0: fechada).
static MAIN: AtomicIsize = AtomicIsize::new(0);
/// Uma tela do Prisma lê o controle (campo de tecla, gravação de macro, medição): não navega.
static PAUSED: AtomicBool = AtomicBool::new(false);
static STATE: Mutex<Option<HashMap<String, Pad>>> = Mutex::new(None);
/// Menu do HDR aberto por cima do jogo: o controle navega nele (`menu_feed`), e o mapeamento não vê nada.
/// Menu do HDR: fechado, aberto, ou fechado esperando soltar o que o fechou (`MENU_*`).
static MENU: AtomicU8 = AtomicU8::new(MENU_CLOSED);
const MENU_CLOSED: u8 = 0;
const MENU_OPEN: u8 = 1;
const MENU_DRAIN: u8 = 2;
/// O último relatório com o menu aberto tinha botão ou gatilho apertado.
static MENU_HELD: AtomicBool = AtomicBool::new(false);
static MENU_PADS: Mutex<Option<HashMap<String, Pad>>> = Mutex::new(None);

/// Repetição de quem segura uma direção.
const FIRST: Duration = Duration::from_millis(400);
const REPEAT: Duration = Duration::from_millis(110);
/// Analógicos: zona morta, velocidade máxima do cursor (pixels por segundo) e da rolagem (cliques da
/// roda por segundo), as mesmas do perfil Desktop.
const DEAD: f32 = 0.15;
const SPEED: f32 = 1600.0;
const SCROLL: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    /// 0 esquerda, 1 cima, 2 direita, 3 baixo.
    Move(u8),
    Ok,
    Back,
    Page(i8),
    Tab(i8),
    Hide,
    /// Controle em uso (clique, mouse ou rolagem pelos analógicos): mostra as dicas.
    Wake,
}

#[derive(Default)]
struct Pad {
    buttons: pads::Buttons,
    lt: bool,
    rt: bool,
    dir: Option<u8>,
    next: Option<Instant>,
    last: Option<Instant>,
    acc: (f32, f32),
    wheel: f32,
    moving: bool,
}

impl Pad {
    /// O que já estava apertado quando o Prisma ganhou o foco não conta como aperto.
    fn new(inp: &Input) -> Self {
        Self { buttons: inp.buttons, lt: inp.lt > 0.5, rt: inp.rt > 0.5, dir: dir_of(inp), ..Default::default() }
    }

    fn step(&mut self, inp: &Input, now: Instant) -> (Vec<Cmd>, Vec<Act>) {
        let pressed = inp.buttons & !self.buttons;
        let released = self.buttons & !inp.buttons;
        self.buttons = inp.buttons;
        let mut cmds: Vec<Cmd> = [(pads::A, Cmd::Ok), (pads::B, Cmd::Back), (pads::LB, Cmd::Page(-1)), (pads::RB, Cmd::Page(1)), (pads::Y, Cmd::Hide)]
            .into_iter()
            .filter(|(b, _)| pressed & b != 0)
            .map(|(_, c)| c)
            .collect();
        let (lt, rt) = (inp.lt > 0.5, inp.rt > 0.5);
        if lt && !self.lt {
            cmds.push(Cmd::Tab(-1));
        }
        if rt && !self.rt {
            cmds.push(Cmd::Tab(1));
        }
        (self.lt, self.rt) = (lt, rt);
        let mut acts = vec![];
        if pressed & pads::X != 0 {
            cmds.push(Cmd::Wake);
            acts.push(Act::Down(Out::Mouse(1)));
        }
        if released & pads::X != 0 {
            acts.push(Act::Up(Out::Mouse(1)));
        }
        let dir = dir_of(inp);
        match dir {
            Some(d) if self.dir != Some(d) => {
                cmds.push(Cmd::Move(d));
                self.next = Some(now + FIRST);
            }
            Some(d) if self.next.is_some_and(|t| now >= t) => {
                cmds.push(Cmd::Move(d));
                self.next = Some(now + REPEAT);
            }
            _ => {}
        }
        self.dir = dir;
        let dt = self.last.map_or(0.0, |l| now.saturating_duration_since(l).as_secs_f32().min(0.05));
        self.last = Some(now);
        let m = inp.lx.hypot(inp.ly);
        let scroll = inp.ry.abs() > DEAD;
        if m > DEAD || scroll {
            if !self.moving {
                cmds.push(Cmd::Wake);
                self.moving = true;
            }
        } else {
            self.moving = false;
        }
        if m > DEAD {
            // Curva: devagar perto do centro, rápido no fim.
            let k = ((m - DEAD) / (1.0 - DEAD)).min(1.0);
            let v = SPEED * k * k / m * dt;
            self.acc.0 += inp.lx * v;
            self.acc.1 -= inp.ly * v;
            let (dx, dy) = (self.acc.0.trunc(), self.acc.1.trunc());
            self.acc.0 -= dx;
            self.acc.1 -= dy;
            if dx != 0.0 || dy != 0.0 {
                acts.push(Act::Move(dx as i32, dy as i32));
            }
        } else {
            self.acc = (0.0, 0.0);
        }
        if scroll {
            let k = (inp.ry.abs() - DEAD) / (1.0 - DEAD);
            self.wheel += inp.ry.signum() * k * SCROLL * 120.0 * dt;
            let w = self.wheel.trunc();
            if w != 0.0 {
                acts.push(Act::Wheel(w as i32));
                self.wheel -= w;
            }
        } else {
            self.wheel = 0.0;
        }
        (cmds, acts)
    }
}

/// Direção do direcional.
fn dir_of(inp: &Input) -> Option<u8> {
    [pads::LEFT, pads::UP, pads::RIGHT, pads::DOWN].iter().position(|&b| inp.buttons & b != 0).map(|i| i as u8)
}

/// A janela principal abriu (`h`) ou fechou (0). Com ela aberta o XInput é lido a cada 8 ms.
pub fn set_window(h: isize) {
    if MAIN.swap(h, Ordering::Relaxed) != h {
        pads::poke();
    }
}

/// Chamado a cada desenho da janela principal.
pub fn pause(on: bool) {
    PAUSED.store(on, Ordering::Relaxed);
}

pub fn window_open() -> bool {
    MAIN.load(Ordering::Relaxed) != 0
}

/// O menu do HDR abriu ou fechou. Aberto, o XInput é lido a cada 8 ms, como com o Prisma aberto.
/// Ao abrir, solta o que o mapeamento segurava no jogo; ao fechar com algo apertado (o B ou o A que
/// fechou), segura o controle até soltar, como o teclado e o radial, para o jogo não receber o toque.
pub fn set_menu(on: bool) {
    *MENU_PADS.lock().unwrap() = None;
    let next = if on { MENU_OPEN } else if MENU_HELD.load(Ordering::Relaxed) { MENU_DRAIN } else { MENU_CLOSED };
    let was = MENU.swap(next, Ordering::Relaxed);
    if on && was != MENU_OPEN {
        MENU_HELD.store(false, Ordering::Relaxed);
        remap::release_held();
    }
    if (was == MENU_CLOSED) != (next == MENU_CLOSED) {
        pads::poke();
    }
}

pub fn menu_open() -> bool {
    MENU.load(Ordering::Relaxed) != MENU_CLOSED
}

/// Relatório do controle `key` com o menu do HDR aberto: vira `Event::Menu` (direcional, A, B) e o
/// mapeamento não vê nada, como no teclado e no radial. Fechado, false.
pub fn menu_feed(key: &str, inp: &Input, now: Instant) -> bool {
    let held = inp.buttons != 0 || inp.lt >= 0.3 || inp.rt >= 0.3;
    match MENU.load(Ordering::Relaxed) {
        MENU_CLOSED => return false,
        MENU_DRAIN => {
            if !held {
                MENU.store(MENU_CLOSED, Ordering::Relaxed);
                MENU_HELD.store(false, Ordering::Relaxed);
            }
            return true;
        }
        _ => MENU_HELD.store(held, Ordering::Relaxed),
    }
    let inp = &stick_as_dpad(inp);
    let (cmds, _) = MENU_PADS.lock().unwrap().get_or_insert_with(HashMap::new).entry(key.to_string()).or_insert_with(|| Pad::new(inp)).step(inp, now);
    // Os analógicos e o X não mexem o mouse aqui: o foco é do jogo.
    for c in cmds.into_iter().filter(|c| *c != Cmd::Wake) {
        pads::emit(pads::Event::Menu(c));
    }
    true
}

/// O analógico esquerdo vira o direcional (a direção mais forte, passada a metade do curso) e deixa de
/// mexer o mouse: no menu do HDR ele anda como as setas.
fn stick_as_dpad(inp: &Input) -> Input {
    let mut i = *inp;
    if i.lx.abs().max(i.ly.abs()) > 0.5 {
        i.buttons |= if i.lx.abs() > i.ly.abs() {
            if i.lx < 0.0 { pads::LEFT } else { pads::RIGHT }
        } else if i.ly > 0.0 {
            pads::UP
        } else {
            pads::DOWN
        };
    }
    (i.lx, i.ly) = (0.0, 0.0);
    i
}

/// A janela principal do Prisma está em foco.
pub fn front() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let h = MAIN.load(Ordering::Relaxed);
    h != 0 && unsafe { GetForegroundWindow() }.0 as isize == h
}

/// O controle `key` saiu: esquece o estado dele e solta o clique se o X estava apertado.
pub fn forget(key: &str) {
    if let Some(m) = MENU_PADS.lock().unwrap().as_mut() {
        m.remove(key);
    }
    let held = STATE.lock().unwrap().as_mut().and_then(|m| m.remove(key)).is_some_and(|p| p.buttons & pads::X != 0);
    if held {
        remap::send(&[Act::Up(Out::Mouse(1))]);
    }
}

/// Relatório do controle `key`. Com o Prisma em foco, faz a navegação e devolve o que o mapeamento
/// ainda vê; fora dele, None.
pub fn feed(key: &str, inp: &Input, now: Instant) -> Option<Input> {
    let front = front();
    let mut g = STATE.lock().unwrap();
    let map = g.get_or_insert_with(HashMap::new);
    if !front || PAUSED.load(Ordering::Relaxed) {
        // Saiu do Prisma (ou parou de navegar) com X apertado: solta o clique.
        if map.remove(key).is_some_and(|p| p.buttons & pads::X != 0) {
            remap::send(&[Act::Up(Out::Mouse(1))]);
        }
        // Pausado: o mapeamento só vê a parada de emergência.
        return front.then(|| Input { buttons: inp.buttons & remap::STOP_COMBO, ..Default::default() });
    }
    let (cmds, acts) = map.entry(key.to_string()).or_insert_with(|| Pad::new(inp)).step(inp, now);
    drop(g);
    remap::send(&acts);
    for c in cmds {
        pads::emit(pads::Event::Nav(c));
    }
    Some(rest(inp))
}

/// O que o mapeamento ainda vê com o Prisma na frente: a parada de emergência, o botão do radial,
/// e o gyro e o touchpad inteiros. A navegação não usa nenhum dos dois, e sem isso eles deixavam de
/// mexer o mouse sempre que a janela do Prisma estava na frente.
fn rest(inp: &Input) -> Input {
    Input {
        buttons: inp.buttons & (remap::STOP_COMBO | super::radial::button()),
        gyro: inp.gyro,
        accel: inp.accel,
        motion: inp.motion,
        touch: inp.touch,
        ..Default::default()
    }
}

/// Leva o cursor ao ponto (`x`, `y`) da tela, com a marca do Prisma: a janela não confunde com o mouse.
pub fn point_to(x: i32, y: i32) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (vw, vh) = (GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2), GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2));
        let n = |p: i32, o: i32, s: i32| ((p - o) as f64 * 65535.0 / (s - 1) as f64).round() as i32;
        let mi = MOUSEINPUT { dx: n(x, vx, vw), dy: n(y, vy, vh), dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, dwExtraInfo: remap::MARK, ..Default::default() };
        SendInput(&[INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi } }], std::mem::size_of::<INPUT>() as i32);
    }
}

/// Clique esquerdo onde o cursor está, com a marca do Prisma.
pub fn click() {
    remap::send(&[Act::Down(Out::Mouse(1)), Act::Up(Out::Mouse(1))]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prisma_in_front_still_passes_the_touchpad_and_the_gyro() {
        let inp = Input {
            buttons: pads::A | pads::LB | remap::STOP_COMBO,
            lx: 0.8,
            gyro: [10.0, 20.0, 30.0],
            motion: true,
            touch: [Some((0.3, 0.4)), None],
            ..Default::default()
        };
        let r = rest(&inp);
        // O stick e os botões comuns ficam com a navegação.
        assert_eq!((r.lx, r.buttons & pads::A), (0.0, 0));
        assert_eq!(r.buttons & remap::STOP_COMBO, remap::STOP_COMBO);
        // O gyro e o touchpad seguem para o mapeamento.
        assert_eq!((r.gyro, r.motion, r.touch), (inp.gyro, true, inp.touch));
    }

    #[test]
    fn edges_repeat_and_mouse() {
        let t0 = Instant::now();
        let mut p = Pad::new(&Input { buttons: pads::A, ..Default::default() });
        // A já estava apertado ao ganhar o foco: não escolhe.
        assert!(p.step(&Input { buttons: pads::A, ..Default::default() }, t0).0.is_empty());
        let (c, _) = p.step(&Input { buttons: pads::DOWN | pads::RB, lt: 0.9, ..Default::default() }, t0);
        assert_eq!(c, vec![Cmd::Page(1), Cmd::Tab(-1), Cmd::Move(3)]);
        assert!(p.step(&Input { buttons: pads::DOWN, lt: 0.9, ..Default::default() }, t0 + Duration::from_millis(300)).0.is_empty());
        assert_eq!(p.step(&Input { buttons: pads::DOWN, ..Default::default() }, t0 + Duration::from_millis(420)).0, vec![Cmd::Move(3)]);
        // Direcional para a esquerda; X aperta e solta o clique.
        let (c, a) = p.step(&Input { buttons: pads::X | pads::LEFT, ..Default::default() }, t0 + Duration::from_millis(500));
        assert_eq!(c, vec![Cmd::Wake, Cmd::Move(0)]);
        assert!(matches!(a[..], [Act::Down(Out::Mouse(1))]));
        assert!(matches!(p.step(&Input::default(), t0 + Duration::from_millis(510)).1[..], [Act::Up(Out::Mouse(1))]));
        // Analógico esquerdo para cima e à direita: o cursor sobe (y negativo) e vai para a direita,
        // e o analógico direito para cima rola para cima (roda positiva). Nenhum dos dois anda o foco.
        let s = Input { lx: 0.7, ly: 0.7, ry: 0.9, ..Default::default() };
        assert_eq!(p.step(&s, t0 + Duration::from_millis(520)).0, vec![Cmd::Wake]);
        let (c, a) = p.step(&s, t0 + Duration::from_millis(560));
        assert!(c.is_empty());
        assert!(matches!(a[..], [Act::Move(dx, dy), Act::Wheel(w)] if dx > 0 && dy < 0 && w > 0), "{}", a.len());
    }

    #[test]
    fn menu_stick_moves_like_the_dpad() {
        let at = |lx, ly| stick_as_dpad(&Input { lx, ly, ..Default::default() });
        assert_eq!(at(0.3, 0.2).buttons, 0);
        assert_eq!(at(-0.9, 0.4).buttons, pads::LEFT);
        assert_eq!(at(0.2, 0.8).buttons, pads::UP);
        assert_eq!(at(0.1, -0.7).buttons, pads::DOWN);
        let i = at(0.9, 0.0);
        assert_eq!((i.buttons, i.lx, i.ly), (pads::RIGHT, 0.0, 0.0));
    }
}
