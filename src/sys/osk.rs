//! Teclado do Prisma para controle. O analógico esquerdo escolhe um dos 8 grupos e A, B, X ou Y
//! digita a letra do grupo que está na mesma posição do botão. As letras vão como texto (Unicode):
//! não dependem do idioma do teclado do Windows.

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
pub const FACE: [u32; 4] = [pads::X, pads::Y, pads::B, pads::A];
/// Teclas que repetem enquanto o botão fica apertado: apagar e as setas.
const REPEATS: [(u32, u16); 5] = [(pads::LB, 0x08), (pads::UP, 0x26), (pads::DOWN, 0x28), (pads::LEFT, 0x25), (pads::RIGHT, 0x27)];
const REPEAT_AFTER: Duration = Duration::from_millis(400);
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
    pub shift: bool,
    pub sym: bool,
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
}

struct Osk {
    view: View,
    /// Só o controle que mexeu primeiro digita: dois controles misturariam os botões.
    pad: Option<String>,
    prev: u32,
    repeat: Option<(u32, u16, Instant)>,
    /// Posição do analógico, para a seta. Fica fora do `View`: mudar não pede aviso à janela,
    /// que lê a seta no próprio relógio dela.
    stick: (f32, f32),
    /// Botões que chamam o teclado (ligação do perfil, botão do radial): apertados de novo, fecham.
    close_on: u32,
}

/// Botões que o teclado usa: nunca fecham, mesmo ligados a ele.
const USED: u32 = pads::A | pads::B | pads::X | pads::Y | pads::LB | pads::RB | pads::START | pads::BACK | pads::UP | pads::DOWN | pads::LEFT | pads::RIGHT;

impl Osk {
    fn new(close_on: u32) -> Self {
        // Tudo conta como já apertado: o botão que abriu o teclado só vale depois de solto.
        Osk { view: View { open: true, ..Default::default() }, pad: None, prev: u32::MAX, repeat: None, stick: (0.0, 0.0), close_on: close_on & !USED }
    }

    /// Um relatório do controle: as teclas a digitar e se é para fechar.
    fn step(&mut self, inp: &Input, now: Instant) -> (Vec<Key>, bool) {
        let held = inp.buttons;
        let down = held & !self.prev;
        self.prev = held;
        self.view.shift = inp.rt > 0.5;
        self.view.sym = inp.lt > 0.5;
        self.view.petal = petal(inp.lx, inp.ly);
        self.stick = (inp.lx, inp.ly);
        let mut keys = vec![];
        if down & (pads::BACK | self.close_on) != 0 || (down & pads::B != 0 && self.view.petal.is_none()) {
            return (keys, true);
        }
        if let Some(p) = self.view.petal {
            keys.extend(FACE.iter().enumerate().filter(|(_, b)| down & **b != 0).map(|(i, _)| Key::Char(self.view.char_at(p, i))));
        }
        if down & pads::RB != 0 {
            keys.push(Key::Char(' '));
        }
        if down & pads::START != 0 {
            keys.push(Key::Vk(0x0D));
        }
        if let Some((b, vk, at)) = self.repeat {
            if held & b == 0 {
                self.repeat = None;
            } else if now >= at {
                keys.push(Key::Vk(vk));
                self.repeat = Some((b, vk, now + REPEAT_EVERY));
            }
        }
        for (b, vk) in REPEATS {
            if down & b != 0 {
                keys.push(Key::Vk(vk));
                self.repeat = Some((b, vk, now + REPEAT_AFTER));
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

pub fn view() -> View {
    match &*STATE.lock().unwrap() {
        State::Open(o) => o.view,
        _ => View::default(),
    }
}

/// Analógico esquerdo agora (Y para cima positivo), ou parado com o teclado fechado.
pub fn stick() -> (f32, f32) {
    match &*STATE.lock().unwrap() {
        State::Open(o) => o.stick,
        _ => (0.0, 0.0),
    }
}

/// Abre o teclado. O que o mapeamento segurava é solto antes.
pub fn open() {
    let close_on = super::remap::keyboard_buttons() | super::radial::button();
    let mut s = STATE.lock().unwrap();
    if !matches!(*s, State::Open(_)) {
        *s = State::Open(Osk::new(close_on));
        drop(s);
        super::remap::release_held();
    }
}

/// Só para as fotos do manual: teclado aberto com o grupo `petal` escolhido.
pub fn show_for_test(petal: usize) {
    open();
    if let State::Open(o) = &mut *STATE.lock().unwrap() {
        o.view.petal = Some(petal);
        let a = (petal as f32 * 45.0).to_radians();
        o.stick = (a.sin() * 0.9, a.cos() * 0.9);
    }
}

pub fn close() {
    let mut s = STATE.lock().unwrap();
    if matches!(*s, State::Open(_)) {
        *s = State::Drain;
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
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    match *k {
        Key::Vk(vk) => super::remap::send(&[Act::Down(Out::Key(vk, 0)), Act::Up(Out::Key(vk, 0))]),
        Key::Char(c) => {
            let mut buf = [0u16; 2];
            let list: Vec<INPUT> = c
                .encode_utf16(&mut buf)
                .iter()
                .flat_map(|&u| [KEYEVENTF_UNICODE, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP].map(|f| INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wScan: u, dwFlags: f, ..Default::default() } } }))
                .collect();
            unsafe {
                SendInput(&list, std::mem::size_of::<INPUT>() as i32);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(buttons: u32, lx: f32, ly: f32) -> Input {
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
        let mut o = Osk::new(0);
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
        let mut o = Osk::new(0);
        o.step(&inp(0, 0.0, 0.0), t);
        assert_eq!(o.step(&inp(pads::RB | pads::START, 0.0, 0.0), t).0, vec![Key::Char(' '), Key::Vk(0x0D)]);
        o.step(&inp(0, 0.0, 0.0), t);
        // Apagar repete depois de 400 ms, a cada 60 ms.
        assert_eq!(o.step(&inp(pads::LB, 0.0, 0.0), t).0, vec![Key::Vk(0x08)]);
        assert_eq!(o.step(&inp(pads::LB, 0.0, 0.0), t + Duration::from_millis(300)).0, vec![]);
        assert_eq!(o.step(&inp(pads::LB, 0.0, 0.0), t + Duration::from_millis(400)).0, vec![Key::Vk(0x08)]);
        assert_eq!(o.step(&inp(pads::LB, 0.0, 0.0), t + Duration::from_millis(430)).0, vec![]);
        assert_eq!(o.step(&inp(0, 0.0, 0.0), t + Duration::from_millis(500)).0, vec![]);
        // B com o analógico apontado digita; com ele solto, fecha.
        assert_eq!(o.step(&inp(pads::B, 0.0, 1.0), t), (vec![Key::Char('c')], false));
        o.step(&inp(0, 0.0, 0.0), t);
        assert!(o.step(&inp(pads::B, 0.0, 0.0), t).1);
        let mut o = Osk::new(0);
        o.step(&inp(0, 0.0, 0.0), t);
        assert!(o.step(&inp(pads::BACK, 0.0, 1.0), t).1);
    }

    #[test]
    fn the_button_that_calls_it_closes_it_unless_the_keyboard_uses_it() {
        let t = Instant::now();
        // RS ligado ao teclado e A também: RS fecha, A continua digitando.
        let mut o = Osk::new(pads::RS | pads::A);
        assert!(!o.step(&inp(pads::RS, 0.0, 0.0), t).1, "o aperto que abriu não fecha");
        o.step(&inp(0, 0.0, 0.0), t);
        assert_eq!(o.step(&inp(pads::A, 0.0, 1.0), t), (vec![Key::Char('d')], false));
        o.step(&inp(0, 0.0, 0.0), t);
        assert!(o.step(&inp(pads::RS, 0.0, 0.0), t).1);
    }
}
