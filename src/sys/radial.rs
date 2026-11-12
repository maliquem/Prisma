//! Radial do Prisma: um círculo com até 8 ações do Prisma. O stick direito aponta, e
//! ele fica aberto até Confirmar (A) disparar ou Voltar (B) fechar.
//! Enquanto ele está aberto, o controle serve só a ele, como no teclado do Prisma.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::pads::{self, Input};

/// Lugar vazio no radial.
pub const EMPTY: u8 = u8::MAX;
/// Lugar que abre um programa (o caminho vem de `set_programs`).
pub const PROGRAM: u8 = u8::MAX - 1;
/// Lugares: 8 do stick direito e, com dois círculos, mais 8 do esquerdo.
pub const SLOTS: usize = 16;

static PROGRAMS: Mutex<[String; SLOTS]> = Mutex::new([const { String::new() }; SLOTS]);

/// Caminho do programa de cada lugar marcado com `PROGRAM`.
pub fn set_programs(paths: [String; SLOTS]) {
    *PROGRAMS.lock().unwrap() = paths;
}

/// O que vem da configuração (aba Controles, cartão Radial do Prisma).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Config {
    /// Ações em cada direção, de cima no sentido do relógio: 0 a 7 no stick direito, 8 a 15 no esquerdo.
    pub slots: [u8; SLOTS],
    /// Dois círculos, um em cada stick (16 lugares). Sempre fica aberto.
    pub two: bool,
    /// Com um círculo só, o stick esquerdo aponta em vez do direito.
    pub left: bool,
    /// Botão que abre (`pads::BACK`, `pads::GUIDE`), ou 0 para só a ação ligada no perfil.
    pub button: u32,
    pub hold: Duration,
    /// Toque abre o radial e segurar faz a função normal do botão (o padrão é o contrário).
    pub tap_opens: bool,
    /// Com o stick de volta ao centro, o lugar escolhido continua escolhido.
    pub keep: bool,
    /// Botão que dispara (aberto para ficar) e botão que fecha sem escolher.
    pub ok: u32,
    pub back: u32,
    /// O Prisma faz a função normal do Guia (Win + G), com a Game Bar sem o botão do controle.
    pub guide_normal: bool,
}

pub const DEFAULT: Config = Config {
    slots: [EMPTY; SLOTS],
    two: false,
    left: false,
    button: 0,
    hold: Duration::from_millis(400),
    tap_opens: false,
    keep: true,
    ok: pads::A,
    back: pads::B,
    guide_normal: false,
};

/// Botões que dá para escolher para confirmar e voltar.
pub const PICK_BUTTONS: [u32; 6] = [pads::A, pads::B, pads::X, pads::Y, pads::LB, pads::RB];

static CFG: Mutex<Config> = Mutex::new(DEFAULT);

pub fn configure(mut c: Config) {
    // O mesmo botão nos dois: volta ao padrão, senão nada fecharia.
    if c.ok == c.back {
        (c.ok, c.back) = (pads::A, pads::B);
    }
    *CFG.lock().unwrap() = c;
}

struct Radial {
    slots: [u8; SLOTS],
    two: bool,
    left: bool,
    pad: Option<String>,
    sel: Option<usize>,
    /// Para onde cada stick apontava no relatório anterior: direito, esquerdo.
    last: [Option<usize>; 2],
    /// Stick direito e esquerdo.
    stick: [(f32, f32); 2],
    keep: bool,
    ok: u32,
    back: u32,
    /// Botão que abriu: apertado de novo, fecha. 0: aberto por uma ligação do perfil.
    watch: u32,
    prev: u32,
}

enum State {
    Closed,
    Open(Radial),
    /// Fechado com botão ainda apertado: nada vai para o perfil até soltar.
    Drain,
}

static STATE: Mutex<State> = Mutex::new(State::Closed);

/// O que a janela mostra.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct View {
    pub open: bool,
    pub sel: Option<usize>,
    pub two: bool,
    pub left: bool,
    pub watch: u32,
}

pub fn view() -> View {
    match &*STATE.lock().unwrap() {
        State::Open(r) => View {
            open: true,
            sel: r.sel,
            two: r.two,
            left: r.left,
            watch: r.watch,
        },
        _ => View::default(),
    }
}

/// Sticks direito e esquerdo agora (Y para cima positivo), para as setas.
pub fn stick() -> [(f32, f32); 2] {
    match &*STATE.lock().unwrap() {
        State::Open(r) => r.stick,
        _ => [(0.0, 0.0); 2],
    }
}

/// Abre. O que o perfil segurava é solto antes. Não chamar com o mapeamento travado (`remap::RUN`).
fn open(watch: u32) {
    let (slots, two, left, keep, ok, back) = { let c = CFG.lock().unwrap(); (c.slots, c.two, c.left && !c.two, c.keep, c.ok, c.back) };
    let mut s = STATE.lock().unwrap();
    if !matches!(*s, State::Open(_)) {
        *s = State::Open(Radial {
            slots,
            two,
            left,
            pad: None,
            sel: None,
            last: [None; 2],
            stick: [(0.0, 0.0); 2],
            keep,
            ok,
            back,
            watch,
            prev: u32::MAX,
        });
        drop(s);
        // Aberto, a soltura do botão vai para o radial e não para o gatilho: ele recomeça do zero.
        TRIG.lock().unwrap().take();
        super::remap::release_held();
    }
}

/// Botão que abre o radial (0: nenhum).
pub fn button() -> u32 {
    CFG.lock().unwrap().button
}

/// Aberto pela ação "Radial do Prisma" ligada a um botão do perfil (segurar).
pub fn open_action() {
    open(0);
}

/// Só para as fotos do manual: aberto, apontando para `sel`.
pub fn show_for_test(sel: usize) {
    open(pads::BACK);
    if let State::Open(r) = &mut *STATE.lock().unwrap() {
        r.sel = Some(sel);
        let a = ((sel % 8) as f32 * 45.0).to_radians();
        r.stick[if r.left { 1 } else { sel / 8 }] = (a.sin() * 0.9, a.cos() * 0.9);
    }
}

/// Um relatório: a ação a disparar, se fechou e se a janela muda.
fn step(r: &mut Radial, inp: &Input) -> (Option<u8>, bool, bool) {
    r.stick = [(inp.rx, inp.ry), (inp.lx, inp.ly)];
    // Um círculo só: os lugares 0 a 7 seguem o stick escolhido.
    let (px, py) = if r.left { (inp.lx, inp.ly) } else { (inp.rx, inp.ry) };
    let right = super::osk::petal(px, py).filter(|&i| r.slots[i] != EMPTY);
    let left = if r.two { super::osk::petal(inp.lx, inp.ly).map(|i| i + 8).filter(|&i| r.slots[i] != EMPTY) } else { None };
    // Vale o stick que acabou de apontar para outro lugar; parado, o que já estava escolhido.
    let sel = if right.is_some() && right != r.last[0] {
        right
    } else if left.is_some() && left != r.last[1] {
        left
    } else if r.sel.is_some() && (r.sel == right || r.sel == left) {
        r.sel
    } else {
        // Stick no centro: com `keep`, o lugar de antes continua escolhido.
        right.or(left).or(if r.keep { r.sel } else { None })
    };
    r.last = [right, left];
    let changed = sel != r.sel;
    r.sel = sel;
    let down = inp.buttons & !r.prev;
    r.prev = inp.buttons;
    if down & r.ok != 0 && sel.is_some() {
        return (sel.map(|i| r.slots[i]), true, true);
    }
    if down & (r.back | r.watch) != 0 {
        return (None, true, true);
    }
    (None, false, changed)
}

/// Relatório do controle `key`. Devolve se o radial ficou com ele.
pub fn feed(key: &str, inp: &Input, _now: Instant) -> bool {
    let (pick, slot, changed) = {
        let mut s = STATE.lock().unwrap();
        match &mut *s {
            State::Closed => return false,
            State::Drain => {
                if inp.buttons == 0 && inp.lt < 0.3 && inp.rt < 0.3 {
                    *s = State::Closed;
                }
                return true;
            }
            State::Open(r) => {
                if r.pad.get_or_insert_with(|| key.to_string()) != key {
                    return true;
                }
                let (pick, close, changed) = step(r, inp);
                let slot = r.sel;
                if close {
                    *s = if inp.buttons == 0 {
                        State::Closed
                    } else {
                        State::Drain
                    };
                }
                (pick, slot, changed)
            }
        }
    };
    if changed {
        pads::emit(pads::Event::Radial);
    }
    if pick == Some(PROGRAM) {
        let path = slot.map(|i| PROGRAMS.lock().unwrap()[i].clone()).unwrap_or_default();
        if !path.is_empty() {
            pads::emit(pads::Event::Run { path });
        }
    } else if let Some(id) = pick {
        pads::emit(pads::Event::Action {
            id,
            pad: key.to_string(),
        });
    }
    true
}

/// Estado do botão que abre, por controle.
#[derive(Default)]
struct Trig {
    since: Option<Instant>,
    fired: bool,
    /// Virou a parada de emergência: não abre nem faz a função normal até soltar.
    aborted: bool,
    /// A função normal está "segurada" (segurar faz a função normal).
    normal_down: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Normal {
    None,
    Tap,
    Down,
    Up,
}

/// Um relatório no botão que abre: abre o radial agora, e o que fazer com a função normal.
fn trig_step(t: &mut Trig, c: &Config, buttons: u32, now: Instant) -> (bool, Normal) {
    let held = buttons & c.button != 0;
    if held && buttons & super::remap::STOP_COMBO == super::remap::STOP_COMBO {
        t.aborted = true;
    }
    match (held, t.since) {
        (true, None) => {
            *t = Trig {
                since: Some(now),
                ..Default::default()
            };
            (false, Normal::None)
        }
        (true, Some(s)) if !t.fired && !t.aborted && now.saturating_duration_since(s) >= c.hold => {
            t.fired = true;
            if c.tap_opens {
                t.normal_down = true;
                (false, Normal::Down)
            } else {
                (true, Normal::None)
            }
        }
        (false, Some(_)) => {
            let t0 = std::mem::take(t);
            match (t0.aborted, t0.fired, c.tap_opens) {
                (true, ..) => (
                    false,
                    if t0.normal_down {
                        Normal::Up
                    } else {
                        Normal::None
                    },
                ),
                (false, false, true) => (true, Normal::None),
                (false, false, false) => (false, Normal::Tap),
                (false, true, true) => (false, Normal::Up),
                (false, true, false) => (false, Normal::None),
            }
        }
        _ => (false, Normal::None),
    }
}

static TRIG: Mutex<Option<HashMap<String, Trig>>> = Mutex::new(None);

/// O que o mapeamento faz com um relatório por causa do botão que abre o radial.
pub struct Trigger {
    /// Abrir o radial (depois de soltar a trava do mapeamento).
    pub open: Option<u32>,
    /// Botões que o perfil vê, um passo por item (o toque vira aperto e soltura).
    pub steps: Vec<u32>,
    /// Mandar Win + G: a função normal do Guia feita pelo Prisma.
    pub win_g: bool,
}

/// Botão que abre o radial no controle `key`. `game`: o perfil é o Base, e o Select fica só com o jogo.
pub fn trigger(key: &str, buttons: u32, now: Instant, game: bool) -> Trigger {
    let c = *CFG.lock().unwrap();
    if c.button == 0 || (game && c.button != pads::GUIDE) {
        return Trigger {
            open: None,
            steps: vec![buttons],
            win_g: false,
        };
    }
    let mut g = TRIG.lock().unwrap();
    let t = g
        .get_or_insert_with(HashMap::new)
        .entry(key.to_string())
        .or_default();
    let (open, normal) = trig_step(t, &c, buttons, now);
    let guide = c.button == pads::GUIDE;
    // O perfil não vê o botão que abre, menos quando a função normal está segurada.
    let rest = if t.normal_down {
        buttons
    } else {
        buttons & !c.button
    };
    let steps = match normal {
        Normal::Tap if !guide => vec![rest | c.button, rest],
        _ => vec![rest],
    };
    let win_g = guide && c.guide_normal && matches!(normal, Normal::Tap | Normal::Down);
    let open = open.then_some(c.button);
    Trigger { open, steps, win_g }
}

/// Abre o radial pedido por `trigger`.
pub fn open_from(button: u32) {
    self::open(button);
    pads::emit(pads::Event::Radial);
}

/// A Game Bar deixa de abrir pelo botão do controle; devolve o valor de antes (-1: não havia).
pub fn gamebar_take() -> i64 {
    use super::reg::{self, Hive};
    let prev = reg::dword(Hive::User, GAMEBAR_KEY, GAMEBAR_NAME).map_or(-1, |v| v as i64);
    reg::write_dword(Hive::User, GAMEBAR_KEY, GAMEBAR_NAME, 0);
    prev
}

/// Devolve a opção da Game Bar como estava antes de `gamebar_take`.
pub fn gamebar_restore(prev: i64) {
    use super::reg::{self, Hive};
    if prev < 0 {
        reg::delete(Hive::User, GAMEBAR_KEY, GAMEBAR_NAME);
    } else {
        reg::write_dword(Hive::User, GAMEBAR_KEY, GAMEBAR_NAME, prev as u32);
    }
}

/// "Abrir a Xbox Game Bar com este botão no controle", em Configurações > Jogos.
const GAMEBAR_KEY: &str = r"Software\Microsoft\GameBar";
const GAMEBAR_NAME: &str = "UseNexusForGameBarEnabled";

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(buttons: u32, rx: f32, ry: f32) -> Input {
        Input {
            buttons,
            rx,
            ry,
            ..Default::default()
        }
    }

    fn radial(watch: u32) -> Radial {
        Radial {
            slots: [10, 11, EMPTY, 13, 14, 15, 16, 17, 20, 21, 22, 23, 24, 25, 26, EMPTY],
            two: false,
            left: false,
            pad: None,
            sel: None,
            last: [None; 2],
            stick: [(0.0, 0.0); 2],
            keep: false,
            ok: pads::A,
            back: pads::B,
            watch,
            prev: u32::MAX,
        }
    }

    #[test]
    fn kept_choice_and_chosen_confirm_and_back_buttons() {
        // Fica aberto, com X confirmando e Y voltando: A e B não fazem nada.
        let mut r = Radial { keep: true, ok: pads::X, back: pads::Y, ..radial(pads::GUIDE) };
        step(&mut r, &inp(0, 0.0, 1.0));
        // Stick de volta ao centro: a escolha continua.
        assert_eq!(step(&mut r, &inp(0, 0.0, 0.0)), (None, false, false));
        assert_eq!(r.sel, Some(0));
        assert_eq!(step(&mut r, &inp(pads::A, 0.0, 0.0)).1, false);
        assert_eq!(step(&mut r, &inp(pads::B, 0.0, 0.0)).1, false);
        assert_eq!(step(&mut r, &inp(pads::X, 0.0, 0.0)), (Some(10), true, true));
        let mut r = Radial { ok: pads::X, back: pads::Y, ..radial(pads::GUIDE) };
        step(&mut r, &inp(0, 0.0, 1.0));
        assert_eq!(step(&mut r, &inp(pads::Y, 0.0, 1.0)), (None, true, true));
        // Sem `keep`, o stick no centro desfaz a escolha e Confirmar não faz nada.
        let mut r = radial(pads::GUIDE);
        step(&mut r, &inp(0, 0.0, 1.0));
        step(&mut r, &inp(0, 0.0, 0.0));
        assert_eq!(step(&mut r, &inp(pads::A, 0.0, 0.0)).1, false);
    }

    #[test]
    fn fires_with_confirm_and_closes_with_back_or_the_button() {
        let mut r = radial(pads::GUIDE);
        // O Guia ainda segurado de quando abriu não fecha.
        assert_eq!(step(&mut r, &inp(pads::GUIDE, 0.0, 0.0)).1, false);
        assert_eq!(step(&mut r, &inp(0, 0.0, 0.0)).1, false);
        // A sem nada apontado não faz nada; apontado, dispara.
        assert_eq!(step(&mut r, &inp(pads::A, 0.0, 0.0)).1, false);
        step(&mut r, &inp(0, 0.0, 1.0));
        assert_eq!(
            step(&mut r, &inp(pads::A, 0.0, 1.0)),
            (Some(10), true, true)
        );
        let mut r = radial(pads::GUIDE);
        step(&mut r, &inp(0, 0.0, 1.0));
        assert_eq!(step(&mut r, &inp(pads::B, 0.0, 1.0)), (None, true, true));
        let mut r = radial(pads::GUIDE);
        step(&mut r, &inp(0, 0.0, 1.0));
        assert_eq!(
            step(&mut r, &inp(pads::GUIDE, 0.0, 1.0)),
            (None, true, true)
        );
    }

    #[test]
    fn tap_and_hold_split_between_radial_and_normal_function() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let c = Config {
            button: pads::BACK,
            ..DEFAULT
        };
        // Padrão: toque faz a função normal, segurar 400 ms abre.
        let mut t = Trig::default();
        assert_eq!(
            trig_step(&mut t, &c, pads::BACK, ms(0)),
            (false, Normal::None)
        );
        assert_eq!(trig_step(&mut t, &c, 0, ms(100)), (false, Normal::Tap));
        assert_eq!(
            trig_step(&mut t, &c, pads::BACK, ms(200)),
            (false, Normal::None)
        );
        assert_eq!(
            trig_step(&mut t, &c, pads::BACK, ms(599)),
            (false, Normal::None)
        );
        assert_eq!(
            trig_step(&mut t, &c, pads::BACK, ms(600)),
            (true, Normal::None)
        );
        assert_eq!(trig_step(&mut t, &c, 0, ms(700)), (false, Normal::None));
        // Ao contrário: toque abre, segurar faz a função normal e solta junto.
        let c = Config {
            tap_opens: true,
            ..c
        };
        let mut t = Trig::default();
        trig_step(&mut t, &c, pads::BACK, ms(0));
        assert_eq!(trig_step(&mut t, &c, 0, ms(100)), (true, Normal::None));
        trig_step(&mut t, &c, pads::BACK, ms(200));
        assert_eq!(
            trig_step(&mut t, &c, pads::BACK, ms(600)),
            (false, Normal::Down)
        );
        assert_eq!(trig_step(&mut t, &c, 0, ms(900)), (false, Normal::Up));
        // A parada de emergência (Guia + Select + Start) não abre nem faz a função normal.
        let c = Config {
            tap_opens: false,
            ..c
        };
        let mut t = Trig::default();
        let stop = super::super::remap::STOP_COMBO;
        trig_step(&mut t, &c, stop, ms(0));
        assert_eq!(trig_step(&mut t, &c, stop, ms(2000)), (false, Normal::None));
        assert_eq!(trig_step(&mut t, &c, 0, ms(2100)), (false, Normal::None));
    }

    #[test]
    fn trigger_hides_the_button_from_the_profile_and_replays_a_tap() {
        let t0 = Instant::now();
        configure(Config {
            button: pads::BACK,
            slots: [1; SLOTS],
            ..DEFAULT
        });
        let key = "teste-gatilho";
        assert_eq!(
            trigger(key, pads::BACK | pads::A, t0, false).steps,
            vec![pads::A]
        );
        assert_eq!(
            trigger(key, pads::A, t0 + Duration::from_millis(50), false).steps,
            vec![pads::A | pads::BACK, pads::A]
        );
        // No jogo (perfil Base) o Select fica só com o jogo.
        assert_eq!(trigger(key, pads::BACK, t0, true).steps, vec![pads::BACK]);
        configure(DEFAULT);
    }

    #[test]
    fn two_circles_follow_the_stick_that_moved_last() {
        let mut r = radial(pads::GUIDE);
        r.two = true;
        let both = |lx, ly, rx, ry, b| Input { buttons: b, lx, ly, rx, ry, ..Default::default() };
        // Esquerdo para cima: lugar 8.
        step(&mut r, &both(0.0, 1.0, 0.0, 0.0, 0));
        assert_eq!(r.sel, Some(8));
        // O direito aponta para a direita (lugar 2 vazio) e depois para baixo (lugar 4): vale o direito.
        step(&mut r, &both(0.0, 1.0, 1.0, 0.0, 0));
        assert_eq!(r.sel, Some(8));
        step(&mut r, &both(0.0, 1.0, 0.0, -1.0, 0));
        assert_eq!(r.sel, Some(4));
        // O direito volta ao centro: fica o esquerdo, que ainda aponta.
        step(&mut r, &both(0.0, 1.0, 0.0, 0.0, 0));
        assert_eq!(r.sel, Some(8));
        assert_eq!(step(&mut r, &both(0.0, 1.0, 0.0, 0.0, pads::A)), (Some(20), true, true));
        // Um círculo só: o esquerdo não escolhe nada.
        let mut r = radial(pads::GUIDE);
        step(&mut r, &both(0.0, 1.0, 0.0, 0.0, 0));
        assert_eq!(r.sel, None);
        // Um círculo no stick esquerdo: ele escolhe os lugares 0 a 7 e o direito não faz nada.
        r.left = true;
        step(&mut r, &both(0.0, 1.0, 0.0, -1.0, 0));
        assert_eq!(r.sel, Some(0));
    }
}
