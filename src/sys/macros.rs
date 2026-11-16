//! Macros dos perfis: cada uma roda numa thread própria, com os passos e as esperas dela, e para no
//! fim (uma vez), ao soltar o botão (enquanto segura), no próximo aperto (até apertar de novo) ou
//! quando o mapeamento solta tudo (parada de emergência, troca de perfil, teclado do Prisma).
//! O gravador pega teclado, cliques e roda do mouse enquanto o editor grava, com o aviso na tela, e
//! os botões do controle como as teclas, cliques e roda que o perfil faz deles (`record_acts`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::remap::{self, Act, Macro, Mode, Out, Step};

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
    std::thread::spawn(move || {
        run(&steps, mode, &stop, |a| remap::send(a));
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
    }

    /// O que o motor do controle mandou: teclas, cliques e roda viram passos; o resto não entra.
    fn acts(&mut self, now: Instant, acts: &[Act]) {
        for a in acts {
            let s = match *a {
                Act::Down(o @ (Out::Key(..) | Out::Mouse(_))) => Step::Down(o),
                Act::Up(o @ (Out::Key(..) | Out::Mouse(_))) => Step::Up(o),
                Act::Wheel(n) if n / 120 != 0 => Step::Down(Out::Wheel((n / 120).clamp(-127, 127) as i8)),
                _ => continue,
            };
            self.event(now, s);
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

pub fn recording() -> bool {
    REC.lock().unwrap().is_some()
}

/// Começa a gravar. Nada do que acontece com o Prisma em foco entra, nem o que o Prisma manda.
pub fn record_start() {
    let mut g = REC.lock().unwrap();
    if g.is_some() {
        return;
    }
    *g = Some(Rec::default());
    std::thread::spawn(hooks);
}

/// Para de gravar e devolve os passos.
pub fn record_stop() -> Vec<Step> {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
    let steps = REC.lock().unwrap().take().map(Rec::finish).unwrap_or_default();
    let t = REC_THREAD.swap(0, Ordering::SeqCst);
    if t != 0 {
        unsafe {
            let _ = PostThreadMessageW(t, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
    steps
}

fn record(s: Step) {
    if let Some(r) = REC.lock().unwrap().as_mut() {
        r.event(Instant::now(), s);
    }
}

/// O controle apertou algo pelo perfil (`remap::feed`). Essas teclas saem injetadas, e o gancho
/// as ignora; sem isto o controle não gravava nada.
pub fn record_acts(now: Instant, acts: &[Act]) {
    if let Some(r) = REC.lock().unwrap().as_mut() {
        if !super::nav::front() {
            r.acts(now, acts);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn recorder_takes_controller_keys_clicks_and_wheel() {
        let t = Instant::now();
        let at = |ms| t + Duration::from_millis(ms);
        let (a, click) = (Out::Key(0x41, 0), Out::Mouse(1));
        let mut r = Rec::default();
        r.acts(at(0), &[Act::Down(a), Act::Move(5, 5), Act::Cmd(Out::Action(0))]);
        r.acts(at(50), &[Act::Up(a), Act::Down(click)]);
        r.acts(at(80), &[Act::Up(click), Act::Wheel(-240)]);
        assert_eq!(r.finish(), vec![Step::Down(a), Step::Wait(50), Step::Up(a), Step::Down(click), Step::Wait(30), Step::Up(click), Step::Down(Out::Wheel(-2))]);
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
