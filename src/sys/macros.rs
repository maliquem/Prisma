//! Macros dos perfis: cada uma roda numa thread própria, com os passos e as esperas dela, e para no
//! fim (uma vez), ao soltar o botão (enquanto segura), no próximo aperto (até apertar de novo) ou
//! quando o mapeamento solta tudo (parada de emergência, troca de perfil, teclado do Prisma).
//! O gravador pega teclado, cliques e roda do mouse e os botões e gatilhos de todo controle, lidos
//! direto do leitor (`pad_input`), em qualquer modo. Começa pelo editor (grava na macro escolhida)
//! ou pela ação liga/desliga (atalho, radial, combinação: grava numa macro nova do perfil em uso).
//! Na reprodução, o botão gravado é apertado no controle que rodou a macro, antes do perfil e da
//! ponte Xbox: o motor e o Xbox virtual o veem como se fosse do controle.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::pads::{Buttons, Family, Input};
use super::remap::{self, Act, Macro, Mode, Out, Profile, Step};

/// Botões do controle que a macro grava e aperta: os físicos com nome e os dois gatilhos.
/// shortcut: sticks e gyro não gravam (só botões e gatilhos); gravar eixos pede passo com valor.
pub const PAD_BITS: Buttons = (super::pads::PHYSICAL & ((1 << 62) - 1)) | remap::LT | remap::RT;

/// Macros rodando: (controle, número) → modo e o sinal de parar.
static RUNNING: Mutex<Option<HashMap<(String, u8), (Mode, Arc<AtomicBool>)>>> = Mutex::new(None);

/// O botão ligado à macro `n` foi apertado no controle `pad`.
pub fn press(pad: &str, n: u8, m: &Macro) {
    let mut g = RUNNING.lock().unwrap();
    let map = g.get_or_insert_with(HashMap::new);
    let key = (pad.to_string(), n);
    if let Some((_, stop)) = map.get(&key) {
        // Rodando: "até apertar de novo" para; as outras seguem.
        if m.mode == Mode::Toggle {
            stop.store(true, Ordering::Relaxed);
            map.remove(&key);
        }
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    map.insert(key.clone(), (m.mode, stop.clone()));
    let (steps, mode) = (m.steps.clone(), m.mode);
    let pad = pad.to_string();
    std::thread::spawn(move || {
        run(&steps, mode, &stop, |a| {
            // Botão do controle: pelo caminho do controle; o resto, ao Windows.
            let (b, rest): (Vec<Act>, Vec<Act>) = a.iter().copied().partition(|x| matches!(x, Act::Down(Out::Pad(_)) | Act::Up(Out::Pad(_))));
            remap::send(&rest);
            for x in b {
                if let Act::Down(Out::Pad(n)) | Act::Up(Out::Pad(n)) = x {
                    press_pad(&pad, 1 << n, matches!(x, Act::Down(_)));
                }
            }
        });
        let mut g = RUNNING.lock().unwrap();
        if let Some(map) = g.as_mut() {
            if map.get(&key).is_some_and(|(_, s)| Arc::ptr_eq(s, &stop)) {
                map.remove(&key);
            }
        }
    });
}

/// O botão soltou: "enquanto segura" para.
pub fn release(pad: &str, n: u8) {
    let mut g = RUNNING.lock().unwrap();
    let Some(map) = g.as_mut() else { return };
    let key = (pad.to_string(), n);
    if map.get(&key).is_some_and(|(m, _)| *m == Mode::Held) {
        map.remove(&key).unwrap().1.store(true, Ordering::Relaxed);
    }
}

/// Para todas as macros; cada uma solta o que segurava.
pub fn stop_all() {
    if let Some(map) = RUNNING.lock().unwrap().as_mut() {
        map.drain().for_each(|(_, (_, s))| s.store(true, Ordering::Relaxed));
    }
}

/// Para as macros do controle que saiu.
pub fn stop_pad(pad: &str) {
    if let Some(map) = RUNNING.lock().unwrap().as_mut() {
        map.retain(|k, (_, s)| {
            let keep = k.0 != pad;
            if !keep {
                s.store(true, Ordering::Relaxed);
            }
            keep
        });
    }
}

/// O controle saiu: para as macros dele e esquece o que segurava.
pub fn forget(pad: &str) {
    stop_pad(pad);
    if let Some(m) = HELD.lock().unwrap().as_mut() {
        m.remove(pad);
    }
}

/// Roda os passos: uma vez, ou de novo até `stop`. No fim, solta o que ficou apertado.
fn run(steps: &[Step], mode: Mode, stop: &AtomicBool, mut send: impl FnMut(&[Act])) {
    let mut held: Vec<Out> = vec![];
    'all: loop {
        let start = Instant::now();
        for s in steps {
            if stop.load(Ordering::Relaxed) {
                break 'all;
            }
            match *s {
                Step::Down(Out::Wheel(n)) => send(&[Act::Wheel(n as i32 * 120)]),
                Step::Down(o) => {
                    send(&[Act::Down(o)]);
                    held.push(o);
                }
                Step::Up(o) => {
                    send(&[Act::Up(o)]);
                    if let Some(i) = held.iter().position(|&x| x == o) {
                        held.remove(i);
                    }
                }
                Step::Wait(ms) => {
                    // Em pedaços, para parar logo.
                    let end = Instant::now() + Duration::from_millis(ms as u64);
                    while Instant::now() < end && !stop.load(Ordering::Relaxed) {
                        std::thread::sleep(end.saturating_duration_since(Instant::now()).min(Duration::from_millis(10)));
                    }
                }
            }
        }
        if mode == Mode::Once || stop.load(Ordering::Relaxed) {
            break;
        }
        // Macro sem espera: no mínimo 10 ms por volta, para não travar o PC.
        if start.elapsed() < Duration::from_millis(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let ups: Vec<Act> = held.into_iter().rev().map(Act::Up).collect();
    if !ups.is_empty() {
        send(&ups);
    }
}

// ---------- Gravador ----------

/// O que o gravador juntou até agora e o que está apertado (a repetição do Windows não conta).
#[derive(Default)]
struct Rec {
    steps: Vec<Step>,
    last: Option<Instant>,
    down: Vec<Out>,
    /// Botões de cada controle no último relatório; o primeiro só marca o ponto de partida.
    pads: HashMap<String, Buttons>,
    /// Primeiro controle que apertou algo: a macro mostra os botões dele.
    pad: Option<String>,
    /// Último aperto, para o aviso na tela.
    pressed: Option<Out>,
    /// Destino: a macro `n` do perfil (o editor); sem ele, uma macro nova.
    into: Option<(String, usize)>,
    start: Option<Instant>,
}

impl Rec {
    fn event(&mut self, now: Instant, s: Step) {
        match s {
            Step::Down(o) if !matches!(o, Out::Wheel(_)) => {
                if self.down.contains(&o) {
                    return;
                }
                self.down.push(o);
            }
            Step::Up(o) => {
                // Soltar o que não foi apertado gravando (o clique em Gravar, uma tecla de antes).
                let Some(i) = self.down.iter().position(|&x| x == o) else { return };
                self.down.remove(i);
            }
            _ => {}
        }
        if self.steps.len() + 2 > remap::MAX_STEPS {
            return;
        }
        if let Some(t) = self.last {
            // Esperas em passos de 10 ms; menos que isso fica junto.
            let ms = (now.saturating_duration_since(t).as_millis() as u32 / 10 * 10).min(remap::MAX_WAIT);
            if ms > 0 {
                self.steps.push(Step::Wait(ms));
            }
        }
        self.last = Some(now);
        self.steps.push(s);
        if let Step::Down(o) = s {
            self.pressed = Some(o);
        }
    }

    /// Relatório do controle `key`: o que mudou nos botões vira aperto ou soltura.
    fn pad(&mut self, key: &str, b: Buttons, now: Instant) {
        let Some(old) = self.pads.insert(key.to_string(), b) else { return };
        for i in 0..64u8 {
            let bit: Buttons = 1 << i;
            if (old ^ b) & bit != 0 {
                let n = self.steps.len();
                self.event(now, if b & bit != 0 { Step::Down(Out::Pad(i)) } else { Step::Up(Out::Pad(i)) });
                if self.steps.len() > n && self.pad.is_none() {
                    self.pad = Some(key.to_string());
                }
            }
        }
    }

    /// Os passos gravados. O que ainda está apertado (o clique em Parar) sai, com a espera antes dele.
    fn finish(mut self) -> Vec<Step> {
        for o in self.down {
            if let Some(i) = self.steps.iter().rposition(|s| *s == Step::Down(o)) {
                self.steps.remove(i);
                if i > 0 && matches!(self.steps.get(i - 1), Some(Step::Wait(_))) {
                    self.steps.remove(i - 1);
                }
            }
        }
        while matches!(self.steps.last(), Some(Step::Wait(_))) {
            self.steps.pop();
        }
        self.steps
    }
}

static REC: Mutex<Option<Rec>> = Mutex::new(None);
static REC_THREAD: AtomicU32 = AtomicU32::new(0);

/// Os botões que a macro grava, num relatório: os físicos e os gatilhos passando da metade.
fn pad_bits(inp: &Input) -> Buttons {
    let lt = if inp.lt >= 0.5 { remap::LT } else { 0 };
    let rt = if inp.rt >= 0.5 { remap::RT } else { 0 };
    (inp.buttons & PAD_BITS & !(remap::LT | remap::RT)) | lt | rt
}

/// Botões que as macros seguram agora, por controle, e o último relatório de verdade de cada um.
static HELD: Mutex<Option<HashMap<String, (Buttons, Input)>>> = Mutex::new(None);

/// Cada relatório de controle passa aqui antes de tudo (`sys::pads`, HID e XInput, em todo modo):
/// grava o que mudou e soma o que as macros seguram.
pub fn pad_input(key: &str, inp: &mut Input, now: Instant) {
    if let Some(r) = REC.lock().unwrap().as_mut() {
        r.pad(key, pad_bits(inp), now);
    }
    let held = {
        let mut g = HELD.lock().unwrap();
        let e = g.get_or_insert_with(HashMap::new).entry(key.to_string()).or_insert((0, *inp));
        e.1 = *inp;
        e.0
    };
    hold(inp, held);
}

/// Soma os botões seguros ao relatório; gatilho seguro vai ao fim.
fn hold(inp: &mut Input, held: Buttons) {
    inp.buttons |= held & !(remap::LT | remap::RT);
    if held & remap::LT != 0 {
        inp.lt = 1.0;
    }
    if held & remap::RT != 0 {
        inp.rt = 1.0;
    }
}

/// A macro aperta ou solta `bit` no controle `key`. Vale já: o último relatório volta pelo caminho
/// do controle, para o controle que só manda relatório quando algo muda.
fn press_pad(key: &str, bit: Buttons, down: bool) {
    let last = {
        let mut g = HELD.lock().unwrap();
        let Some(e) = g.get_or_insert_with(HashMap::new).get_mut(key) else { return };
        if down {
            e.0 |= bit;
        } else {
            e.0 &= !bit;
        }
        let mut inp = e.1;
        hold(&mut inp, e.0);
        inp
    };
    super::pads::refeed(key, &last);
}

pub fn recording() -> bool {
    REC.lock().unwrap().is_some()
}

/// A gravação em curso, para o editor e o aviso na tela.
pub struct Live {
    pub steps: Vec<Step>,
    pub secs: u64,
    pub pressed: Option<Out>,
    pub pad: Option<(Family, u16)>,
    pub into: Option<(String, usize)>,
}

pub fn live() -> Option<Live> {
    let (l, key) = {
        let g = REC.lock().unwrap();
        let r = g.as_ref()?;
        let secs = r.start.map_or(0, |t| t.elapsed().as_secs());
        (Live { steps: r.steps.clone(), secs, pressed: r.pressed, pad: None, into: r.into.clone() }, r.pad.clone())
    };
    // Fora da trava do gravador: a lista de controles tem a dela.
    Some(Live { pad: key.and_then(|k| pad_of(&k)), ..l })
}

fn pad_of(key: &str) -> Option<(Family, u16)> {
    super::pads::list().into_iter().find(|p| p.key == key).map(|p| (p.family, p.vid))
}

/// Começa a gravar na macro `into` (perfil, número) ou, sem ela, numa macro nova. Nada do que
/// acontece com o Prisma em foco entra, menos o controle, nem o que o Prisma manda.
pub fn record_start(into: Option<(String, usize)>) {
    let mut g = REC.lock().unwrap();
    if g.is_some() {
        return;
    }
    *g = Some(Rec { into, start: Some(Instant::now()), ..Default::default() });
    crate::config::log("macro: gravação começou");
    std::thread::spawn(hooks);
}

/// Para de gravar e guarda no perfil: devolve o nome da macro, ou o motivo de não ter guardado.
pub fn record_stop() -> Result<String, &'static str> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
    let rec = REC.lock().unwrap().take();
    let t = REC_THREAD.swap(0, Ordering::SeqCst);
    if t != 0 {
        unsafe {
            let _ = PostThreadMessageW(t, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
    let Some(rec) = rec else { return Err("Nenhuma macro estava sendo gravada") };
    let (pads, key, into) = (rec.pads.len(), rec.pad.clone(), rec.into.clone());
    let steps = rec.finish();
    // Fora dos testes: o registro é o do PC.
    if !cfg!(test) {
        crate::config::log(&format!("macro: gravação parou com {} passos ({pads} controles ouvidos)", steps.len()));
    }
    if steps.is_empty() && into.is_none() {
        return Err("Gravação parada: nada foi gravado");
    }
    let prof = match &into {
        Some((p, _)) => Some(p.clone()),
        None => remap::profile_for(key.as_deref()),
    };
    let mut p = prof.and_then(|n| remap::profiles().into_iter().find(|p| p.name == n)).ok_or("Não deu para salvar a macro")?;
    let name = store(&mut p, into.map(|x| x.1), steps, key.and_then(|k| pad_of(&k)), &crate::i18n::tr("Macro")).ok_or("Este perfil já tem macros demais")?;
    if !remap::save(&p) {
        return Err("Não deu para salvar a macro");
    }
    Ok(name)
}

/// Guarda os passos na macro `into` ou numa nova com o próximo nome livre. `None`: sem lugar.
fn store(p: &mut Profile, into: Option<usize>, steps: Vec<Step>, pad: Option<(Family, u16)>, word: &str) -> Option<String> {
    if let Some(m) = into.and_then(|i| p.macros.get_mut(i)) {
        m.steps = steps;
        m.pad = pad.or(m.pad);
        return Some(m.name.clone());
    }
    if p.macros.len() >= remap::MAX_MACROS {
        return None;
    }
    let name = next_name(p, word);
    p.macros.push(Macro { name: name.clone(), mode: Mode::Once, steps, pad });
    Some(name)
}

/// "Macro N" com o menor N que nenhuma macro do perfil usa.
pub fn next_name(p: &Profile, word: &str) -> String {
    (1..).map(|n| format!("{word} {n}")).find(|n| !p.macros.iter().any(|m| m.name == *n)).unwrap_or_default()
}

fn record(s: Step) {
    if let Some(r) = REC.lock().unwrap().as_mut() {
        r.event(Instant::now(), s);
    }
}

unsafe extern "system" fn key_hook(code: i32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::*;
    if code == HC_ACTION as i32 {
        let k = &*(l.0 as *const KBDLLHOOKSTRUCT);
        if k.flags.0 & LLKHF_INJECTED.0 == 0 && !super::nav::front() && (1..=254).contains(&k.vkCode) {
            let o = Out::Key(k.vkCode as u16, 0);
            match w.0 as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => record(Step::Down(o)),
                WM_KEYUP | WM_SYSKEYUP => record(Step::Up(o)),
                _ => {}
            }
        }
    }
    CallNextHookEx(None, code, w, l)
}

unsafe extern "system" fn mouse_hook(code: i32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::*;
    if code == HC_ACTION as i32 {
        let m = &*(l.0 as *const MSLLHOOKSTRUCT);
        // Clique em cima de uma janela do Prisma (o botão Parar) não entra.
        let mut pid = 0;
        GetWindowThreadProcessId(WindowFromPoint(m.pt), Some(&mut pid));
        if m.flags & LLMHF_INJECTED == 0 && pid != std::process::id() {
            let x = if (m.mouseData >> 16) as u16 == 1 { 4 } else { 5 };
            let step = match w.0 as u32 {
                WM_LBUTTONDOWN => Some(Step::Down(Out::Mouse(1))),
                WM_LBUTTONUP => Some(Step::Up(Out::Mouse(1))),
                WM_RBUTTONDOWN => Some(Step::Down(Out::Mouse(2))),
                WM_RBUTTONUP => Some(Step::Up(Out::Mouse(2))),
                WM_MBUTTONDOWN => Some(Step::Down(Out::Mouse(3))),
                WM_MBUTTONUP => Some(Step::Up(Out::Mouse(3))),
                WM_XBUTTONDOWN => Some(Step::Down(Out::Mouse(x))),
                WM_XBUTTONUP => Some(Step::Up(Out::Mouse(x))),
                WM_MOUSEWHEEL => {
                    let d = (m.mouseData >> 16) as u16 as i16 / 120;
                    (d != 0).then(|| Step::Down(Out::Wheel(d.clamp(-127, 127) as i8)))
                }
                _ => None,
            };
            if let Some(s) = step {
                record(s);
            }
        }
    }
    CallNextHookEx(None, code, w, l)
}

/// Os ganchos só existem enquanto grava, numa thread com fila de mensagens; WM_QUIT encerra.
fn hooks() {
    use windows::Win32::System::Threading::GetCurrentThreadId;
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let mut msg = MSG::default();
        // Cria a fila antes de dizer quem é, para o WM_QUIT não se perder.
        let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE);
        REC_THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        // Parou antes de a thread começar: não liga os ganchos.
        if REC.lock().unwrap().is_none() {
            REC_THREAD.store(0, Ordering::SeqCst);
            return;
        }
        let m = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0);
        let k = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_hook), None, 0);
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        if let Ok(h) = m {
            let _ = UnhookWindowsHookEx(h);
        }
        if let Ok(h) = k {
            let _ = UnhookWindowsHookEx(h);
        }
    }
}

/// Uma macro "rodando" de mentira, para os testes de quem a para.
#[cfg(test)]
pub(super) fn fake_running(pad: &str, n: u8) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    RUNNING.lock().unwrap().get_or_insert_with(HashMap::new).insert((pad.into(), n), (Mode::Held, stop.clone()));
    stop
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forgetting_a_pad_stops_its_macros_and_drops_its_held_buttons() {
        let mine = fake_running("forget-a", 0);
        pad_input("forget-a", &mut Input::default(), Instant::now());
        forget("forget-a");
        assert!(mine.load(Ordering::Relaxed));
        assert!(!RUNNING.lock().unwrap().as_ref().unwrap().keys().any(|k| k.0 == "forget-a"));
        assert!(!HELD.lock().unwrap().as_ref().unwrap().contains_key("forget-a"));
    }

    #[test]
    fn recorder_keeps_waits_and_drops_repeats_and_loose_ends() {
        let t = Instant::now();
        let at = |ms| t + Duration::from_millis(ms);
        let (a, click) = (Out::Key(0x41, 0), Out::Mouse(1));
        let mut r = Rec::default();
        // Soltar o que não apertou gravando não entra; a repetição do Windows também não.
        r.event(at(0), Step::Up(click));
        r.event(at(100), Step::Down(a));
        r.event(at(130), Step::Down(a));
        r.event(at(205), Step::Up(a));
        r.event(at(300), Step::Down(Out::Wheel(-1)));
        // Clique em Parar: o aperto sem a soltura sai, com a espera antes dele.
        r.event(at(900), Step::Down(click));
        assert_eq!(r.finish(), vec![Step::Down(a), Step::Wait(100), Step::Up(a), Step::Wait(90), Step::Down(Out::Wheel(-1))]);
    }

    #[test]
    fn recorder_takes_controller_buttons_and_triggers_from_the_reader() {
        use crate::sys::pads::{A, START};
        // O leitor é o mesmo em Nativo, Teclado e mouse e Xbox: o gravador não depende do modo
        // nem do perfil. Botão virtual (direção do stick) não entra; o gatilho entra pela metade.
        let t = Instant::now();
        let at = |ms| t + Duration::from_millis(ms);
        let b = |buttons, lt| pad_bits(&Input { buttons, lt, ..Default::default() });
        let mut r = Rec::default();
        r.pad("hid#ds", b(START, 0.0), at(0));
        r.pad("hid#ds", b(START | A | remap::LSU, 0.0), at(10));
        r.pad("xinput:0", b(0, 0.0), at(20));
        r.pad("xinput:0", b(0, 0.9), at(60));
        r.pad("hid#ds", b(START, 0.0), at(110));
        r.pad("xinput:0", b(0, 0.1), at(150));
        let lt = remap::LT.trailing_zeros() as u8;
        assert_eq!(r.finish(), vec![Step::Down(Out::Pad(0)), Step::Wait(50), Step::Down(Out::Pad(lt)), Step::Wait(50), Step::Up(Out::Pad(0)), Step::Wait(40), Step::Up(Out::Pad(lt))]);
    }

    #[test]
    fn new_recordings_take_the_next_free_name_and_the_editor_keeps_its_own() {
        let mut p = remap::Profile::base();
        let m = |n: &str| Macro { name: n.into(), mode: Mode::Once, steps: vec![], pad: None };
        p.macros = vec![m("Macro 1"), m("Macro 2")];
        let a = vec![Step::Down(Out::Pad(0)), Step::Up(Out::Pad(0))];
        let ds = Some((Family::DualSense, 0x054c));
        assert_eq!(store(&mut p, None, a.clone(), ds, "Macro").as_deref(), Some("Macro 3"));
        assert_eq!((p.macros[2].steps.clone(), p.macros[2].pad), (a.clone(), ds));
        // Buraco na numeração: o primeiro livre.
        p.macros.remove(0);
        assert_eq!(next_name(&p, "Macro"), "Macro 1");
        // Pelo editor: troca os passos da macro escolhida, sem nome novo.
        assert_eq!(store(&mut p, Some(0), vec![], None, "Macro").as_deref(), Some("Macro 2"));
        assert_eq!((p.macros.len(), p.macros[0].steps.len()), (2, 0));
        p.macros = (0..remap::MAX_MACROS).map(|i| m(&i.to_string())).collect();
        assert_eq!(store(&mut p, None, a, None, "Macro"), None);
    }

    #[test]
    fn live_recording_shows_steps_as_they_come_and_the_toggle_turns_it_off() {
        use crate::sys::pads::A;
        // Liga, aperta: o passo aparece já, antes de parar; desliga: some.
        assert!(!recording());
        *REC.lock().unwrap() = Some(Rec { start: Some(Instant::now()), ..Default::default() });
        assert!(recording());
        let t = Instant::now();
        pad_input("xinput:9", &mut Input::default(), t);
        pad_input("xinput:9", &mut Input { buttons: A, ..Default::default() }, t + Duration::from_millis(30));
        let l = live().unwrap();
        assert_eq!((l.steps, l.pressed), (vec![Step::Down(Out::Pad(0))], Some(Out::Pad(0))));
        let _ = record_stop();
        assert!(!recording() && live().is_none());
    }

    #[test]
    fn held_pad_buttons_reach_the_report_and_survive_the_profile_text() {
        use crate::sys::pads::{A, B};
        let mut inp = Input { buttons: B, ..Default::default() };
        hold(&mut inp, A | remap::RT);
        assert_eq!(inp.buttons, A | B);
        assert_eq!(inp.rt, 1.0);
        let mut p = remap::Profile::base();
        p.macros.push(Macro { name: "M".into(), mode: Mode::Once, steps: vec![Step::Down(Out::Pad(0)), Step::Up(Out::Pad(17))], pad: Some((Family::DualSense, 0x054c)) });
        assert_eq!(remap::Profile::parse(&p.to_text()).unwrap().macros[0].steps, p.macros[0].steps);
    }

    #[test]
    fn runner_repeats_until_stopped_and_releases_what_it_holds() {
        let a = Out::Key(0x41, 0);
        let mut sent = vec![];
        run(&[Step::Down(a), Step::Up(a), Step::Down(Out::Wheel(1))], Mode::Once, &AtomicBool::new(false), |x| sent.extend_from_slice(x));
        assert_eq!(sent, vec![Act::Down(a), Act::Up(a), Act::Wheel(120)]);
        // Parado no meio da espera: solta a tecla que segurava.
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            s2.store(true, Ordering::Relaxed);
        });
        let mut sent = vec![];
        let t = Instant::now();
        run(&[Step::Down(a), Step::Wait(5000)], Mode::Toggle, &stop, |x| sent.extend_from_slice(x));
        assert!(t.elapsed() < Duration::from_millis(1000));
        assert_eq!(sent, vec![Act::Down(a), Act::Up(a)]);
    }
}
