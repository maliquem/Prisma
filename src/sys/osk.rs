//! Teclado do Prisma para controle. O analógico esquerdo escolhe um dos 8 grupos e A, B, X ou Y
//! digita a letra do grupo que está na mesma posição do botão. As letras vão como texto (Unicode):
//! não dependem do idioma do teclado do Windows.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::pads::{self, Input};
use super::remap::{Act, Out};

/// Grupos no sentido do relógio, começando em cima. Em cada grupo: esquerda (X), cima (Y),
/// direita (B) e baixo (A), a mesma posição dos botões no controle. As três primeiras tabelas são
/// as do antigo teclado em roda (daisywheel) do Big Picture do Steam, para quem já está acostumado.
pub const LETTERS: [[char; 4]; 8] = [
    ['a', 'b', 'c', 'd'],
    ['e', 'f', 'g', 'h'],
    ['i', 'j', 'k', 'l'],
    ['m', 'n', 'o', 'p'],
    ['q', 'r', 's', 't'],
    ['u', 'v', 'w', 'x'],
    ['y', 'z', '?', '!'],
    [';', '\\', '&', '-'],
];
pub const CAPS: [[char; 4]; 8] = [
    ['A', 'B', 'C', 'D'],
    ['E', 'F', 'G', 'H'],
    ['I', 'J', 'K', 'L'],
    ['M', 'N', 'O', 'P'],
    ['Q', 'R', 'S', 'T'],
    ['U', 'V', 'W', 'X'],
    ['Y', 'Z', '+', '.'],
    ['@', '#', '$', '%'],
];
/// O último grupo não existia no Steam: o Prisma pôs ali o que faltava.
pub const NUMBERS: [[char; 4]; 8] = [
    ['0', '1', '2', '3'],
    ['4', '5', '6', '7'],
    ['8', '9', '*', ','],
    ['_', '=', '"', '\''],
    ['(', ')', '[', ']'],
    ['{', '}', ':', '~'],
    ['^', '<', '>', '|'],
    ['/', '`', '€', '£'],
];
/// Só do Prisma (os dois gatilhos juntos): letras com acento.
pub const ACCENTS: [[char; 4]; 8] = [
    ['á', 'à', 'â', 'ã'],
    ['é', 'ê', 'í', 'ó'],
    ['ô', 'õ', 'ú', 'ç'],
    ['ü', 'ñ', 'è', 'ò'],
    ['Á', 'À', 'Â', 'Ã'],
    ['É', 'Ê', 'Í', 'Ó'],
    ['Ô', 'Õ', 'Ú', 'Ç'],
    ['Ü', 'Ñ', 'È', 'Ò'],
];
/// Botões na ordem das letras de cada grupo.
pub const FACE: [pads::Buttons; 4] = [pads::X, pads::Y, pads::B, pads::A];
/// Com os dois sticks, o direcional digita no círculo do stick direito, na mesma ordem.
pub const DPAD: [pads::Buttons; 4] = [pads::LEFT, pads::UP, pads::RIGHT, pads::DOWN];
/// Apagar e as setas: repetem enquanto o botão fica apertado mesmo com "Repetir ao segurar" desligado.
const REPEATS: [(pads::Buttons, u16); 5] = [(pads::LB, 0x08), (pads::UP, 0x26), (pads::DOWN, 0x28), (pads::LEFT, 0x25), (pads::RIGHT, 0x27)];
const REPEAT_EVERY: Duration = Duration::from_millis(60);

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Key {
    Char(char),
    Vk(u16),
}

/// O que a janela desenha.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct View {
    pub open: bool,
    pub petal: Option<usize>,
    /// Como no Steam: RT apertado, maiúsculas (`shift`); LT, números e símbolos (`sym`). Os dois: acentos.
    /// Com os dois sticks os números já têm o círculo deles, e LT sozinho dá os acentos.
    pub shift: bool,
    pub sym: bool,
    /// Dois sticks: um círculo para as letras e outro para os números e símbolos.
    pub two: bool,
    /// Com os dois sticks: grupo apontado pelo stick direito, que o direcional digita.
    pub right: Option<usize>,
    /// Com os dois sticks: as letras no stick direito e os números no esquerdo.
    pub letters_right: bool,
}

impl View {
    /// O caractere do grupo `p`, posição `i`, na tabela dos gatilhos apertados.
    pub fn char_at(&self, p: usize, i: usize) -> char {
        match (self.sym, self.shift) {
            (true, true) => ACCENTS[p][i],
            (true, false) => NUMBERS[p][i],
            (false, true) => CAPS[p][i],
            (false, false) => LETTERS[p][i],
        }
    }

    /// O caractere do círculo do stick direito (`right`) ou do esquerdo. Com os dois sticks, o
    /// círculo dos números mostra sempre os números; o das letras segue os gatilhos.
    pub fn wheel_char(&self, right: bool, p: usize, i: usize) -> char {
        if self.two && right != self.letters_right {
            NUMBERS[p][i]
        } else {
            self.char_at(p, i)
        }
    }
}

struct Osk {
    view: View,
    /// Só o controle que mexeu primeiro digita: dois controles misturariam os botões.
    pad: Option<String>,
    prev: pads::Buttons,
    /// Botão segurado, a tecla que ele digitou e quando ela sai de novo.
    repeat: Option<(pads::Buttons, Key, Instant)>,
    /// "Repetir ao segurar": toda tecla repete (senão, só apagar e as setas), depois de `after`.
    repeat_all: bool,
    after: Duration,
    /// Posição dos analógicos (esquerdo, direito), para as setas. Fica fora do `View`: mudar não
    /// pede aviso à janela, que lê as setas no próprio relógio dela.
    stick: [(f32, f32); 2],
    /// Botões e combinações que chamam o teclado (ligações dos perfis, botão do radial):
    /// apertados de novo, fecham. Um botão sozinho que o teclado usa não fecha.
    close_on: Vec<pads::Buttons>,
}

/// Botões que o teclado usa: nunca fecham, mesmo ligados a ele.
const USED: pads::Buttons = pads::A | pads::B | pads::X | pads::Y | pads::LB | pads::RB | pads::START | pads::BACK | pads::UP | pads::DOWN | pads::LEFT | pads::RIGHT;

impl Osk {
    fn new(close_on: Vec<pads::Buttons>) -> Self {
        // Tudo conta como já apertado: o botão que abriu o teclado só vale depois de solto.
        let (two, letters_right) = (TWO.load(Ordering::Relaxed), LETTERS_RIGHT.load(Ordering::Relaxed));
        let (repeat_all, after) = (REPEAT_ALL.load(Ordering::Relaxed), Duration::from_millis(REPEAT_MS.load(Ordering::Relaxed) as u64));
        Osk { view: View { open: true, two, letters_right, ..Default::default() }, pad: None, prev: pads::Buttons::MAX, repeat: None, repeat_all, after, stick: [(0.0, 0.0); 2], close_on: close_on.into_iter().filter(|&m| m != 0 && (m.count_ones() > 1 || m & USED == 0)).collect() }
    }

    /// Um relatório do controle: as teclas a digitar e se é para fechar.
    fn step(&mut self, inp: &Input, now: Instant) -> (Vec<Key>, bool) {
        let held = inp.buttons;
        let down = held & !self.prev;
        self.prev = held;
        let (lt, rt) = (inp.lt > 0.5, inp.rt > 0.5);
        self.view.sym = lt;
        self.view.shift = rt || (lt && self.view.two);
        self.view.petal = petal(inp.lx, inp.ly);
        self.view.right = if self.view.two { petal(inp.rx, inp.ry) } else { None };
        self.stick = [(inp.lx, inp.ly), (inp.rx, inp.ry)];
        let mut keys = vec![];
        let again = self.close_on.iter().any(|&m| held & m == m && down & m != 0);
        if again || down & pads::BACK != 0 || (down & pads::B != 0 && self.view.petal.is_none()) {
            return (keys, true);
        }
        // Cada botão apertado agora e a tecla que ele digita.
        let mut typed: Vec<(pads::Buttons, Key)> = vec![];
        if let Some(p) = self.view.petal {
            typed.extend(FACE.iter().enumerate().filter(|(_, b)| down & **b != 0).map(|(i, b)| (*b, Key::Char(self.view.wheel_char(false, p, i)))));
        }
        // Stick direito apontado: o direcional digita no círculo dele e deixa de ser as setas.
        let dpad_types = self.view.right.is_some();
        if let Some(p) = self.view.right {
            typed.extend(DPAD.iter().enumerate().filter(|(_, b)| down & **b != 0).map(|(i, b)| (*b, Key::Char(self.view.wheel_char(true, p, i)))));
        }
        if down & pads::RB != 0 {
            typed.push((pads::RB, Key::Char(' ')));
        }
        if down & pads::START != 0 {
            typed.push((pads::START, Key::Vk(0x0D)));
        }
        let arrow = |k: Key| matches!(k, Key::Vk(0x25..=0x28));
        typed.extend(REPEATS.iter().filter(|(b, vk)| down & b != 0 && !(dpad_types && arrow(Key::Vk(*vk)))).map(|&(b, vk)| (b, Key::Vk(vk))));
        if let Some((b, k, at)) = self.repeat {
            if held & b == 0 || (dpad_types && arrow(k)) {
                self.repeat = None;
            } else if now >= at {
                keys.push(k);
                self.repeat = Some((b, k, now + REPEAT_EVERY));
            }
        }
        for (b, k) in typed {
            keys.push(k);
            if self.repeat_all || REPEATS.iter().any(|r| r.0 == b) {
                self.repeat = Some((b, k, now + self.after));
            }
        }
        (keys, false)
    }
}

/// Grupo apontado pelo analógico: 0 em cima, no sentido do relógio. Perto do centro, nenhum.
pub(super) fn petal(x: f32, y: f32) -> Option<usize> {
    if x * x + y * y < 0.25 {
        return None;
    }
    let turn = x.atan2(y).to_degrees().rem_euclid(360.0);
    Some(((turn / 45.0).round() as usize) % 8)
}

enum State {
    Closed,
    Open(Osk),
    /// Fechado, mas o controle ainda tem botão apertado: nada vai para o mapeamento até soltar.
    Drain,
}

static STATE: Mutex<State> = Mutex::new(State::Closed);
static TWO: AtomicBool = AtomicBool::new(false);
static LETTERS_RIGHT: AtomicBool = AtomicBool::new(false);
static DODGE: AtomicBool = AtomicBool::new(true);
static REPEAT_ALL: AtomicBool = AtomicBool::new(true);
static REPEAT_MS: AtomicU32 = AtomicU32::new(500);

/// Opções da configuração: vale na próxima vez que o teclado abrir.
pub fn configure(two: bool, letters_right: bool, dodge: bool, repeat: bool, repeat_ms: u32) {
    TWO.store(two, Ordering::Relaxed);
    LETTERS_RIGHT.store(letters_right, Ordering::Relaxed);
    DODGE.store(dodge, Ordering::Relaxed);
    REPEAT_ALL.store(repeat, Ordering::Relaxed);
    REPEAT_MS.store(repeat_ms, Ordering::Relaxed);
}

/// O teclado foge do cursor de texto (`overlays::Keyboard`).
pub fn dodge() -> bool {
    DODGE.load(Ordering::Relaxed)
}

pub fn view() -> View {
    match &*STATE.lock().unwrap() {
        State::Open(o) => o.view,
        _ => View::default(),
    }
}

/// Analógicos esquerdo e direito agora (Y para cima positivo), ou parados com o teclado fechado.
pub fn stick() -> [(f32, f32); 2] {
    match &*STATE.lock().unwrap() {
        State::Open(o) => o.stick,
        _ => [(0.0, 0.0); 2],
    }
}

/// Abre o teclado. O que o mapeamento segurava é solto antes.
pub fn open() {
    super::pads::trace(|| "abre o teclado do Prisma".into());
    let mut close_on = super::remap::keyboard_buttons();
    close_on.push(super::radial::button());
    let mut s = STATE.lock().unwrap();
    if !matches!(*s, State::Open(_)) {
        *s = State::Open(Osk::new(close_on));
        drop(s);
        super::remap::release_held();
    }
}

/// Só para as fotos do manual: teclado aberto com o grupo `petal` escolhido (com os dois sticks,
/// o direito aponta para o grupo oposto).
pub fn show_for_test(petal: usize) {
    open();
    if let State::Open(o) = &mut *STATE.lock().unwrap() {
        let at = |p: usize| {
            let a = (p as f32 * 45.0).to_radians();
            (a.sin() * 0.9, a.cos() * 0.9)
        };
        o.view.petal = Some(petal);
        o.stick[0] = at(petal);
        if o.view.two {
            o.view.right = Some((petal + 4) % 8);
            o.stick[1] = at((petal + 4) % 8);
        }
    }
}

pub fn close() {
    let mut s = STATE.lock().unwrap();
    if matches!(*s, State::Open(_)) {
        *s = State::Drain;
    }
}

/// O controle `key` saiu: fecha o teclado que era dele (sem ele, ninguém mais fecharia).
pub fn forget(key: &str) {
    let mut s = STATE.lock().unwrap();
    if matches!(&*s, State::Open(o) if o.pad.as_deref() == Some(key)) {
        *s = State::Closed;
        drop(s);
        pads::emit(pads::Event::Keyboard);
    }
}

/// Relatório do controle `key`. Devolve se o teclado ficou com ele.
pub fn feed(key: &str, inp: &Input, now: Instant) -> bool {
    let (keys, changed) = {
        let mut s = STATE.lock().unwrap();
        match &mut *s {
            State::Closed => return false,
            State::Drain => {
                if inp.buttons == 0 && inp.lt < 0.3 && inp.rt < 0.3 {
                    *s = State::Closed;
                }
                return true;
            }
            State::Open(o) => {
                if o.pad.get_or_insert_with(|| key.to_string()) != key {
                    return true;
                }
                let before = o.view;
                let (keys, close) = o.step(inp, now);
                let changed = close || before != o.view;
                if close {
                    *s = State::Drain;
                }
                (keys, changed)
            }
        }
    };
    keys.iter().for_each(type_key);
    if changed {
        pads::emit(pads::Event::Keyboard);
    }
    true
}

fn type_key(k: &Key) {
    match *k {
        Key::Vk(vk) => super::remap::send(&[Act::Down(Out::Key(vk, 0)), Act::Up(Out::Key(vk, 0))]),
        Key::Char(c) => type_text(&c.to_string()),
    }
}

/// Digita como texto, não como teclas: vale com qualquer idioma do teclado do Windows.
pub fn type_text(s: &str) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let list: Vec<INPUT> = s
        .encode_utf16()
        .flat_map(|u| [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP].map(|f| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wScan: u, dwFlags: f, dwExtraInfo: super::remap::MARK, ..Default::default() } } }))
        .collect();
    unsafe {
        SendInput(&list, std::mem::size_of::<INPUT>() as i32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(buttons: pads::Buttons, lx: f32, ly: f32) -> Input {
        Input { buttons, lx, ly, ..Default::default() }
    }

    #[test]
    fn open_keyboard_owns_the_controller_until_released_after_closing() {
        let t = Instant::now();
        assert!(!feed("p", &inp(0, 0.0, 0.0), t), "fechado: o controle vai para o perfil");
        open();
        // A segurado desde antes: nada é digitado e o perfil (Desktop ou outro) não recebe nada.
        assert!(feed("p", &inp(pads::A, 0.0, 0.0), t));
        close();
        assert!(feed("p", &inp(pads::A, 0.0, 0.0), t), "fechou com A apertado: segura até soltar");
        assert!(feed("p", &inp(0, 0.0, 0.0), t));
        assert!(!feed("p", &inp(0, 0.0, 0.0), t), "solto: volta ao perfil que estava carregado");
        // O dono saiu: o teclado fecha; outro controle saindo não fecha.
        open();
        assert!(feed("p", &inp(0, 0.0, 0.0), t));
        forget("q");
        assert!(feed("p", &inp(0, 0.0, 0.0), t));
        forget("p");
        assert!(!feed("p", &inp(0, 0.0, 0.0), t));
    }

    #[test]
    fn stick_picks_group_clockwise_from_up() {
        assert_eq!(petal(0.0, 0.1), None);
        assert_eq!(petal(0.0, 1.0), Some(0));
        assert_eq!(petal(0.7, 0.7), Some(1));
        assert_eq!(petal(1.0, 0.0), Some(2));
        assert_eq!(petal(0.0, -1.0), Some(4));
        assert_eq!(petal(-1.0, 0.0), Some(6));
        assert_eq!(petal(-0.7, 0.7), Some(7));
    }

    #[test]
    fn types_only_after_the_opening_button_is_released() {
        let t = Instant::now();
        let mut o = Osk::new(vec![]);
        // A segurado desde antes de abrir: não digita.
        assert_eq!(o.step(&inp(pads::A, 0.0, 1.0), t).0, vec![]);
        assert_eq!(o.step(&inp(0, 0.0, 1.0), t).0, vec![]);
        // Grupo de cima, A embaixo: "d". Com RT: "D". Com LT: "3". Com os dois: "ã".
        assert_eq!(o.step(&inp(pads::A, 0.0, 1.0), t).0, vec![Key::Char('d')]);
        o.step(&inp(0, 0.0, 1.0), t);
        assert_eq!(o.step(&Input { buttons: pads::A, ly: 1.0, rt: 1.0, ..Default::default() }, t).0, vec![Key::Char('D')]);
        o.step(&inp(0, 0.0, 1.0), t);
        assert_eq!(o.step(&Input { buttons: pads::A, ly: 1.0, lt: 1.0, ..Default::default() }, t).0, vec![Key::Char('3')]);
        o.step(&inp(0, 0.0, 1.0), t);
        assert_eq!(o.step(&Input { buttons: pads::A, ly: 1.0, lt: 1.0, rt: 1.0, ..Default::default() }, t).0, vec![Key::Char('ã')]);
    }

    #[test]
    fn space_enter_backspace_repeat_and_close() {
        let t = Instant::now();
        let mut o = Osk::new(vec![]);
        o.step(&inp(0, 0.0, 0.0), t);
        assert_eq!(o.step(&inp(pads::RB | pads::START, 0.0, 0.0), t).0, vec![Key::Char(' '), Key::Vk(0x0D)]);
        o.step(&inp(0, 0.0, 0.0), t);
        // Apagar repete depois da espera (500 ms), a cada 60 ms.
        assert_eq!(o.step(&inp(pads::LB, 0.0, 0.0), t).0, vec![Key::Vk(0x08)]);
        assert_eq!(o.step(&inp(pads::LB, 0.0, 0.0), t + Duration::from_millis(400)).0, vec![]);
        assert_eq!(o.step(&inp(pads::LB, 0.0, 0.0), t + Duration::from_millis(500)).0, vec![Key::Vk(0x08)]);
        assert_eq!(o.step(&inp(pads::LB, 0.0, 0.0), t + Duration::from_millis(530)).0, vec![]);
        assert_eq!(o.step(&inp(0, 0.0, 0.0), t + Duration::from_millis(600)).0, vec![]);
        // Repetir ao segurar: espaço e letras também repetem; desligado, não.
        let at = |ms| t + Duration::from_millis(ms);
        assert_eq!(o.step(&inp(pads::RB, 0.0, 1.0), at(1000)).0, vec![Key::Char(' ')]);
        assert_eq!(o.step(&inp(pads::RB, 0.0, 1.0), at(1500)).0, vec![Key::Char(' ')]);
        o.step(&inp(0, 0.0, 1.0), at(1510));
        assert_eq!(o.step(&inp(pads::A, 0.0, 1.0), at(1520)).0, vec![Key::Char('d')]);
        assert_eq!(o.step(&inp(pads::A, 0.0, 1.0), at(2020)).0, vec![Key::Char('d')]);
        o.step(&inp(0, 0.0, 1.0), at(2030));
        o.repeat_all = false;
        o.step(&inp(pads::RB, 0.0, 0.0), at(3000));
        assert_eq!(o.step(&inp(pads::RB, 0.0, 0.0), at(3600)).0, vec![]);
        o.step(&inp(0, 0.0, 0.0), at(3610));
        // B com o analógico apontado digita; com ele solto, fecha.
        assert_eq!(o.step(&inp(pads::B, 0.0, 1.0), t), (vec![Key::Char('c')], false));
        o.step(&inp(0, 0.0, 0.0), t);
        assert!(o.step(&inp(pads::B, 0.0, 0.0), t).1);
        let mut o = Osk::new(vec![]);
        o.step(&inp(0, 0.0, 0.0), t);
        assert!(o.step(&inp(pads::BACK, 0.0, 1.0), t).1);
    }

    #[test]
    fn with_two_sticks_the_dpad_types_on_the_right_wheel_and_is_arrows_when_it_is_centered() {
        let t = Instant::now();
        let mut o = Osk::new(vec![]);
        o.view.two = true;
        o.step(&inp(0, 0.0, 0.0), t);
        let right_up = |b: pads::Buttons| Input { buttons: b, ry: 1.0, ..Default::default() };
        // Direito em cima, direcional para baixo (posição do A): "3" do círculo dos números.
        assert_eq!(o.step(&right_up(pads::DOWN), t).0, vec![Key::Char('3')]);
        // Segurar repete o caractere, não vira seta.
        assert_eq!(o.step(&right_up(pads::DOWN), t + Duration::from_millis(500)).0, vec![Key::Char('3')]);
        o.step(&inp(0, 0.0, 0.0), t);
        // Stick direito solto: o direcional volta a ser as setas.
        assert_eq!(o.step(&inp(pads::DOWN, 0.0, 0.0), t).0, vec![Key::Vk(0x28)]);
        o.step(&inp(0, 0.0, 0.0), t);
        // A com o esquerdo em cima continua digitando letras; LT sozinho dá os acentos.
        assert_eq!(o.step(&inp(pads::A, 0.0, 1.0), t).0, vec![Key::Char('d')]);
        o.step(&inp(0, 0.0, 1.0), t);
        assert_eq!(o.step(&Input { buttons: pads::A, ly: 1.0, lt: 1.0, ..Default::default() }, t).0, vec![Key::Char('ã')]);
        // Trocados: letras no direito, números no esquerdo.
        o.view.letters_right = true;
        o.step(&inp(0, 0.0, 0.0), t);
        assert_eq!(o.step(&inp(pads::A, 0.0, 1.0), t).0, vec![Key::Char('3')]);
        o.step(&inp(0, 0.0, 0.0), t);
        assert_eq!(o.step(&right_up(pads::DOWN), t).0, vec![Key::Char('d')]);
    }

    #[test]
    fn the_button_that_calls_it_closes_it_unless_the_keyboard_uses_it() {
        let t = Instant::now();
        // RS ligado ao teclado e A também: RS fecha, A continua digitando.
        let mut o = Osk::new(vec![pads::RS, pads::A, pads::GUIDE | pads::START]);
        assert!(!o.step(&inp(pads::RS, 0.0, 0.0), t).1, "o aperto que abriu não fecha");
        o.step(&inp(0, 0.0, 0.0), t);
        assert_eq!(o.step(&inp(pads::A, 0.0, 1.0), t), (vec![Key::Char('d')], false));
        o.step(&inp(0, 0.0, 0.0), t);
        assert!(o.step(&inp(pads::RS, 0.0, 0.0), t).1);
        // Combinação ligada ao teclado (PS + Start): fecha, mesmo com Start sendo do teclado.
        let mut o = Osk::new(vec![pads::GUIDE | pads::START]);
        o.step(&inp(0, 0.0, 0.0), t);
        assert!(!o.step(&inp(pads::GUIDE, 0.0, 0.0), t).1);
        assert!(o.step(&inp(pads::GUIDE | pads::START, 0.0, 0.0), t).1);
    }
}
