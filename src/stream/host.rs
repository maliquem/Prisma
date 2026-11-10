//! Quem compartilha: captura a tela principal e o som, codifica e manda para quem assiste.
//! Várias pessoas podem assistir juntas (co-op): cada uma com as permissões do aparelho dela,
//! o mouse e o teclado aplicados aqui e os controles dela virando controles virtuais.

use std::collections::{HashSet, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::*;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::Audio::eRender;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::Media::timeBeginPeriod;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::WinRT::Direct3D11::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::pads::{Bridge, VirtualPad};
use super::*;
use crate::capture::live_enc::{LiveEncoder, VideoFmt};
use crate::capture::engine::{bgra_texture, bitrate, even, ConvOut, Prep};
use crate::capture::hdr::ToneMap;
use crate::capture::mux::{Codec, Pkt};
use crate::capture::sound;
use crate::capture::{d3d, monitor_color, monitor_item, D3d};

/// Sem notícia de quem assiste por este tempo, ela sai da sessão.
const SILENCE: Duration = Duration::from_secs(5);
/// Controle sem notícia por este tempo fica parado (solto), sem sair.
const PAD_STALE: Duration = Duration::from_secs(1);
/// Vagas do XInput: quatro controles no Windows, contando os de verdade do host.
const MAX_PADS: usize = 4;

#[derive(Clone, Copy)]
pub(super) struct Hello {
    w: u32,
    h: u32,
    hz: u32,
    codecs: u8,
    pid: u32,
    flags: u8,
}

impl Hello {
    pub(super) fn from_msg(m: &Msg) -> Option<Self> {
        match *m {
            Msg::Hello { w, h, hz, codecs, pid, flags } => Some(Hello { w, h, hz, codecs, pid, flags }),
            _ => None,
        }
    }
}

/// Quem chegou e vai entrar na sessão.
pub(super) struct Joiner {
    pub link: Arc<dyn Link>,
    pub hello: Hello,
    pub perms: u8,
    pub name: String,
}

/// Um controle de quem assiste, ligado aqui.
struct Slot {
    dev: Box<dyn VirtualPad>,
    seq: u16,
    heard: Instant,
    /// Parado por falta de notícia.
    idle: bool,
    rumble: (u8, u8),
    rumble_sent: Instant,
}

impl Slot {
    fn player(&self, fallback: usize) -> u8 {
        self.dev.slot().map_or(fallback as u8 + 1, |s| s + 1)
    }
}

/// Uma pessoa assistindo.
struct Guest {
    link: Arc<dyn Link>,
    hello: Hello,
    perms: u8,
    name: String,
    input: Option<Injector>,
    pads: [Option<Slot>; PADS],
    /// Último motivo de recusa avisado para cada controle, para não repetir a cada pacote.
    refused: [u8; PADS],
    heard: Instant,
    gone: bool,
    clip_in: super::clip::Pieces,
}

impl Guest {
    fn new(j: Joiner) -> Self {
        Guest { link: j.link, hello: j.hello, perms: j.perms, name: j.name, input: None, pads: Default::default(), refused: [PAD_OK; PADS], heard: Instant::now(), gone: false, clip_in: Default::default() }
    }

    fn send(&self, m: &Msg, out: &mut Vec<u8>) {
        write(out, m);
        self.link.send(out);
    }
}

/// Quem está na sala e a ponte dos controles. Vive entre uma sessão e outra, para os controles não
/// caírem quando o vídeo recomeça.
struct Room {
    guests: Vec<Guest>,
    bridge: Option<Bridge>,
    /// Por que a ponte não abriu, e quando tentar de novo.
    bridge_err: Option<(String, Instant)>,
    /// O jogo em primeiro plano tem anti-cheat: os controles virtuais saem.
    anti_cheat: bool,
    checked: Instant,
    /// A área de transferência do host, aberta quando entra alguém com a permissão `COPY`.
    clip: Option<super::clip::Clip>,
    clip_id: u16,
}

impl Room {
    fn pads(&self) -> usize {
        self.guests.iter().map(|g| g.pads.iter().flatten().count()).sum()
    }

    /// Vagas do XInput dos controles virtuais daqui.
    fn virtual_slots(&self) -> u8 {
        self.guests.iter().flat_map(|g| g.pads.iter().flatten()).filter_map(|s| s.dev.slot()).fold(0, |m, s| m | 1 << s)
    }

    fn bridge(&mut self) -> std::result::Result<&Bridge, String> {
        if self.bridge.is_none() {
            if let Some((e, t)) = &self.bridge_err {
                if t.elapsed() < Duration::from_secs(10) {
                    return Err(e.clone());
                }
            }
            match Bridge::open() {
                Ok(b) => {
                    eprintln!("controles: ponte {} aberta", b.name);
                    self.bridge = Some(b);
                    self.bridge_err = None;
                }
                Err(e) => {
                    eprintln!("controles: {e}");
                    self.bridge_err = Some((e.clone(), Instant::now()));
                    return Err(e);
                }
            }
        }
        Ok(self.bridge.as_ref().unwrap())
    }
}

pub(super) enum End {
    /// Todo mundo saiu.
    Gone,
    /// Recomeça com quem está na sala (alguém reabriu, ou chegou alguém que pede outro vídeo).
    Again,
    /// Outro chegou pelo UDP (só na rede local, uma pessoa por vez).
    Stranger(Hello, SocketAddr),
}

/// Prepara o Windows e o Direct3D para compartilhar.
pub(super) fn prepare() -> Option<D3d> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        let _ = timeBeginPeriod(1);
    }
    let d = match d3d() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("sem Direct3D: {e:?}");
            return None;
        }
    };
    unsafe {
        if let Ok(mt) = d.ctx.cast::<ID3D11Multithread>() {
            let _ = mt.SetMultithreadProtected(true);
        }
    }
    Some(d)
}

/// Quantas pessoas assistem juntas: a opção da página (padrão 3), ou `PRISMA_GUESTS` por cima.
fn max_guests() -> usize {
    std::env::var("PRISMA_GUESTS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| Opts::load().guests).clamp(1, 8)
}

/// HDR opção B: com a opção ligada, o HDR do Windows desliga enquanto alguém sem tela HDR assiste,
/// e volta quando a sala esvazia. A marca no disco faz ele voltar no próximo início do Prisma,
/// mesmo se o Prisma cair no meio.
struct HdrOff(Option<(windows::Win32::Foundation::LUID, u32)>);

fn hdr_mark() -> std::path::PathBuf {
    super::dir().join("hdr_desligado.txt")
}

impl HdrOff {
    fn new(first: &Hello) -> Self {
        let off = std::env::var("PRISMA_HDR").map(|v| v == "desligar").unwrap_or_else(|_| Opts::load().hdr_off);
        if !off || first.flags & SEES_HDR != 0 {
            return HdrOff(None);
        }
        let Some((adapter, output, true)) = crate::sys::hdr::hdr_output() else { return HdrOff(None) };
        let mark = format!("{} {} {}\n", adapter.LowPart, adapter.HighPart, output);
        if std::fs::create_dir_all(super::dir()).and_then(|_| std::fs::write(hdr_mark(), mark)).is_err() {
            eprintln!("HDR: não deu para gravar a marca de volta; o HDR fica ligado");
            return HdrOff(None);
        }
        if crate::sys::hdr::set_hdr(adapter, output, false) {
            eprintln!("HDR do Windows desligado enquanto alguém assiste");
            HdrOff(Some((adapter, output)))
        } else {
            let _ = std::fs::remove_file(hdr_mark());
            eprintln!("HDR: o Windows não desligou; seguindo com a conversão");
            HdrOff(None)
        }
    }
}

impl Drop for HdrOff {
    fn drop(&mut self) {
        if let Some((adapter, output)) = self.0 {
            if crate::sys::hdr::set_hdr(adapter, output, true) {
                eprintln!("HDR do Windows ligado de novo");
            }
            let _ = std::fs::remove_file(hdr_mark());
        }
    }
}

/// Quem está conectado agora, para a página Compartilhar tela (um nome por linha; vazio: ninguém).
pub fn room_path() -> std::path::PathBuf {
    super::dir().join("sala.txt")
}

fn write_room(names: &str) {
    let _ = std::fs::write(room_path(), names);
}

/// Tecla de emergência do host: Ctrl+Alt+Shift+Q desconecta todo mundo na hora, fechando o
/// processo que compartilha (com o serviço, ele abre outro, sem ninguém conectado).
#[cfg_attr(not(feature = "stream"), allow(dead_code))]
pub fn emergency_key() {
    std::thread::Builder::new()
        .name("compartilhar-emergencia".into())
        .spawn(|| unsafe {
            use windows::Win32::UI::Input::KeyboardAndMouse::*;
            if RegisterHotKey(None, 1, MOD_CONTROL | MOD_ALT | MOD_SHIFT | MOD_NOREPEAT, b'Q' as u32).is_err() {
                eprintln!("a tecla de emergência (Ctrl+Alt+Shift+Q) já está em uso por outro programa");
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                if msg.message == WM_HOTKEY {
                    eprintln!("tecla de emergência: todo mundo desconectado");
                    crate::config::log("compartilhar: tecla de emergência apertada, todo mundo desconectado");
                    write_room("");
                    restore_hdr();
                    std::process::exit(0);
                }
            }
        })
        .ok();
}

/// No início do Prisma: se uma sessão caiu com o HDR desligado, liga de volta.
pub fn restore_hdr() {
    for path in [super::user_dir().join("hdr_desligado.txt"), super::service_dir().join("hdr_desligado.txt")] {
        let Ok(s) = std::fs::read_to_string(&path) else { continue };
        let v: Vec<i64> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
        if let [low, high, output] = v[..] {
            let adapter = windows::Win32::Foundation::LUID { LowPart: low as u32, HighPart: high as i32 };
            if crate::sys::hdr::set_hdr(adapter, output as u32, true) {
                crate::config::log("compartilhar: HDR ligado de volta depois de uma sessão interrompida");
            }
        }
        let _ = std::fs::remove_file(&path);
    }
}

/// Uma sala, do primeiro que chega até o último sair. Quem chega pelo `joins` entra junto.
/// Devolve o endereço de alguém novo que chegou pelo UDP, se for o caso.
pub(super) fn serve(d: &D3d, first: Joiner, joins: Option<&mpsc::Receiver<Joiner>>) -> Option<(Hello, SocketAddr)> {
    let _hdr = HdrOff::new(&first.hello);
    let mut room = Room { guests: vec![Guest::new(first)], bridge: None, bridge_err: None, anti_cheat: false, checked: Instant::now() - Duration::from_secs(10), clip: None, clip_id: 0 };
    let g = &room.guests[0];
    eprintln!("chegou «{}»: tela {}x{} a {} Hz, {} ({})", g.name, g.hello.w, g.hello.h, g.hello.hz, perms_text(g.perms), g.link.route());
    loop {
        match unsafe { session(d, &mut room, joins) } {
            Ok(End::Gone) => {
                write_room("");
                eprintln!("todo mundo saiu");
                return None;
            }
            Ok(End::Again) => {}
            Ok(End::Stranger(h, from)) => return Some((h, from)),
            Err(e) => {
                eprintln!("a sessão parou: {e:?}");
                return None;
            }
        }
    }
}

pub fn run(port: u16, control: bool) {
    let sock = match UdpSocket::bind(("0.0.0.0", port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("não deu para abrir a porta {port}: {e}");
            return;
        }
    };
    let Some(d) = prepare() else { return };
    let mode = if control { "com mouse, teclado e controles de quem assiste" } else { "só para ver" };
    eprintln!("Prisma compartilhando ({mode}): esperando quem assiste na porta UDP {port}");
    let mut buf = vec![0u8; 2048];
    let mut next: Option<(Hello, SocketAddr)> = None;
    loop {
        let (hello, from) = match next.take() {
            Some(h) => h,
            None => {
                let _ = sock.set_nonblocking(false);
                let _ = sock.set_read_timeout(None);
                let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                match read(&buf[..n]).as_ref().and_then(Hello::from_msg) {
                    Some(h) => (h, from),
                    None => continue,
                }
            }
        };
        let Ok(s) = sock.try_clone() else { continue };
        let j = Joiner { link: Arc::new(Udp::new(s, from)), hello, perms: if control { TOTAL } else { SOUND }, name: from.to_string() };
        next = serve(&d, j, None);
    }
}

/// Som do PC em blocos de 5 ms, para quem tem a permissão de som. Com quem assiste no mesmo PC,
/// tira o som da janela dele, senão ele voltaria em eco.
fn audio(links: Arc<Mutex<Vec<Arc<dyn Link>>>>, exclude: Option<u32>, stop: Arc<AtomicBool>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let src = match exclude {
            Some(pid) => sound::open_process_mode(pid, true),
            None => sound::open(eRender, true),
        };
        let src = match src {
            Ok(s) => s,
            Err(e) => {
                eprintln!("sem som: {e:?}");
                return;
            }
        };
        let mut pcm: Vec<f32> = Vec::new();
        let mut block = Vec::with_capacity(AUDIO_BLOCK * 4);
        let mut out = Vec::with_capacity(MTU);
        let mut seq = 0u32;
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(2));
            sound::read(&src, &mut pcm);
            while pcm.len() >= AUDIO_BLOCK * 2 {
                block.clear();
                for v in pcm.drain(..AUDIO_BLOCK * 2) {
                    block.extend_from_slice(&((v.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                }
                write(&mut out, &Msg::Audio { seq, pts: now(), data: &block });
                for l in links.lock().unwrap().iter() {
                    l.send(&out);
                }
                seq = seq.wrapping_add(1);
            }
        }
        let _ = src.client.Stop();
    }
}

/// Aplica no PC o mouse e o teclado de quem assiste, só o que as permissões dela deixam.
struct Injector {
    /// Monitor transmitido e área de todos os monitores, em pixels.
    mon: RECT,
    virt: RECT,
    perms: u8,
    next: Option<u32>,
    keys: HashSet<(u16, bool)>,
    buttons: [bool; 5],
}

impl Injector {
    fn new(mon: HMONITOR, perms: u8) -> Self {
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            let _ = GetMonitorInfoW(mon, &mut mi);
        }
        let virt = unsafe {
            let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
            RECT { left: x, top: y, right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN), bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN) }
        };
        Self { mon: mi.rcMonitor, virt, perms, next: None, keys: HashSet::new(), buttons: [false; 5] }
    }

    fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
        }
    }

    fn key(scan: u16, extended: bool, down: bool) -> INPUT {
        let mut f = KEYEVENTF_SCANCODE;
        if extended {
            f |= KEYEVENTF_EXTENDEDKEY;
        }
        if !down {
            f |= KEYEVENTF_KEYUP;
        }
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: scan, dwFlags: f, time: 0, dwExtraInfo: 0 } },
        }
    }

    fn button(b: u8, down: bool) -> Option<INPUT> {
        let (f, data) = match (b, down) {
            (0, true) => (MOUSEEVENTF_LEFTDOWN, 0),
            (0, false) => (MOUSEEVENTF_LEFTUP, 0),
            (1, true) => (MOUSEEVENTF_RIGHTDOWN, 0),
            (1, false) => (MOUSEEVENTF_RIGHTUP, 0),
            (2, true) => (MOUSEEVENTF_MIDDLEDOWN, 0),
            (2, false) => (MOUSEEVENTF_MIDDLEUP, 0),
            (3, true) => (MOUSEEVENTF_XDOWN, XBUTTON1 as i32),
            (3, false) => (MOUSEEVENTF_XUP, XBUTTON1 as i32),
            (4, true) => (MOUSEEVENTF_XDOWN, XBUTTON2 as i32),
            (4, false) => (MOUSEEVENTF_XUP, XBUTTON2 as i32),
            _ => return None,
        };
        Some(Self::mouse(f, 0, 0, data))
    }

    fn one(&mut self, e: Input) -> Option<INPUT> {
        let allowed = match e {
            Input::Key { .. } => self.perms & KEYBOARD != 0,
            _ => self.perms & MOUSE != 0,
        };
        if !allowed {
            return None;
        }
        Some(match e {
            Input::Move { x, y } => {
                // Da imagem para o monitor, e do monitor para a escala de 0 a 65535 da área toda.
                let mw = (self.mon.right - self.mon.left) as i64;
                let mh = (self.mon.bottom - self.mon.top) as i64;
                let px = self.mon.left as i64 + x as i64 * (mw - 1) / 65535;
                let py = self.mon.top as i64 + y as i64 * (mh - 1) / 65535;
                let vw = (self.virt.right - self.virt.left).max(2) as i64;
                let vh = (self.virt.bottom - self.virt.top).max(2) as i64;
                let dx = ((px - self.virt.left as i64) * 65535 / (vw - 1)) as i32;
                let dy = ((py - self.virt.top as i64) * 65535 / (vh - 1)) as i32;
                Self::mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, dx, dy, 0)
            }
            Input::Button { button, down } => {
                if let Some(s) = self.buttons.get_mut(button as usize) {
                    *s = down;
                }
                Self::button(button, down)?
            }
            Input::Wheel { delta, horizontal } => Self::mouse(if horizontal { MOUSEEVENTF_HWHEEL } else { MOUSEEVENTF_WHEEL }, 0, 0, delta as i32),
            Input::Key { scan, extended, down } => {
                if down {
                    self.keys.insert((scan, extended));
                } else {
                    self.keys.remove(&(scan, extended));
                }
                Self::key(scan, extended, down)
            }
        })
    }

    /// Aplica só os eventos novos do pacote: os repetidos de pacotes anteriores são pulados.
    fn apply(&mut self, first: u32, events: &[u8]) {
        let mut list = Vec::new();
        for (i, chunk) in events.chunks(INPUT_SIZE).enumerate() {
            let seq = first.wrapping_add(i as u32);
            if self.next.is_some_and(|n| (seq.wrapping_sub(n) as i32) < 0) {
                continue;
            }
            if let Some(e) = Input::get(chunk) {
                if let Some(inp) = self.one(e) {
                    list.push(inp);
                }
            }
            self.next = Some(seq.wrapping_add(1));
        }
        if !list.is_empty() {
            unsafe {
                SendInput(&list, std::mem::size_of::<INPUT>() as i32);
            }
        }
    }

    /// Solta tudo que ficou apertado, para nenhuma tecla ficar presa quando quem assiste sai.
    fn release(&mut self) {
        let mut list: Vec<INPUT> = self.keys.drain().map(|(s, e)| Self::key(s, e, false)).collect();
        for (i, down) in self.buttons.iter_mut().enumerate() {
            if std::mem::take(down) {
                list.extend(Self::button(i as u8, false));
            }
        }
        if !list.is_empty() {
            unsafe {
                SendInput(&list, std::mem::size_of::<INPUT>() as i32);
            }
        }
    }
}

/// O programa em primeiro plano tem sinal de anti-cheat. `PRISMA_PADS_ANTICHEAT=1` ignora (por conta dela).
fn foreground_anti_cheat() -> bool {
    if std::env::var("PRISMA_PADS_ANTICHEAT").as_deref() == Ok("1") {
        return false;
    }
    let pid = crate::sys::procs::window_pid(unsafe { GetForegroundWindow() });
    pid != 0 && pid != std::process::id() && crate::sys::procs::image_path(pid).is_some_and(|p| crate::meter::anti_cheat(&p))
}

/// O que um controle de quem assiste mandou.
struct PadMsg {
    guest: usize,
    pad: u8,
    seq: u16,
    on: bool,
    state: PadState,
}

/// Liga, atualiza ou solta o controle virtual de um controle de quem assiste.
fn pad_msg(room: &mut Room, m: PadMsg, out: &mut Vec<u8>) {
    let i = m.pad as usize;
    if i >= PADS {
        return;
    }
    let refuse = |room: &mut Room, why: u8, out: &mut Vec<u8>| {
        let g = &mut room.guests[m.guest];
        g.pads[i] = None;
        if g.refused[i] != why {
            g.refused[i] = why;
            eprintln!("controle {} de «{}» recusado: {}", i + 1, g.name, pad_why(why));
            g.send(&Msg::PadInfo { pad: m.pad, player: 0, slot: 255, why }, out);
        }
    };
    if !m.on {
        let g = &mut room.guests[m.guest];
        if g.pads[i].take().is_some() {
            eprintln!("controle {} de «{}» saiu", i + 1, g.name);
        }
        g.refused[i] = PAD_OK;
        return;
    }
    if room.guests[m.guest].perms & PAD == 0 {
        return refuse(room, PAD_NO_PERMISSION, out);
    }
    if room.anti_cheat {
        return refuse(room, PAD_ANTI_CHEAT, out);
    }
    // No mesmo PC, quem assiste também enxerga os controles virtuais daqui: um deles voltando
    // viraria outro controle, sem fim.
    if room.guests[m.guest].link.same_pc() && i < 4 && room.virtual_slots() & (1 << i) != 0 {
        return refuse(room, PAD_LOOP, out);
    }
    if let Some(s) = &mut room.guests[m.guest].pads[i] {
        if (m.seq.wrapping_sub(s.seq) as i16) > 0 || s.idle {
            s.seq = m.seq;
            s.heard = Instant::now();
            s.idle = false;
            if !s.dev.update(&m.state) {
                room.guests[m.guest].pads[i] = None;
            }
        }
        return;
    }
    if room.pads() >= MAX_PADS {
        return refuse(room, PAD_FULL, out);
    }
    let dev = match room.bridge().and_then(|b| b.plug()) {
        Ok(d) => d,
        Err(e) => {
            if room.guests[m.guest].refused[i] != PAD_NO_DRIVER {
                eprintln!("controles: {e}");
            }
            return refuse(room, PAD_NO_DRIVER, out);
        }
    };
    let count = room.pads();
    let g = &mut room.guests[m.guest];
    let mut s = Slot { dev, seq: m.seq, heard: Instant::now(), idle: false, rumble: (0, 0), rumble_sent: Instant::now() };
    s.dev.update(&m.state);
    let player = s.player(count);
    eprintln!("controle {} de «{}» ligado: jogador {player}", i + 1, g.name);
    g.refused[i] = PAD_OK;
    g.send(&Msg::PadInfo { pad: m.pad, player, slot: s.dev.slot().unwrap_or(255), why: PAD_OK }, out);
    g.pads[i] = Some(s);
}

/// Vibração, controles parados e o aviso de vaga repetido de tempos em tempos.
fn pads_tick(room: &mut Room, out: &mut Vec<u8>, repeat_info: bool) {
    // Jogo com anti-cheat em primeiro plano: os controles virtuais saem do Windows.
    if room.checked.elapsed() > Duration::from_secs(2) {
        room.checked = Instant::now();
        let ac = room.guests.iter().any(|g| g.perms & PAD != 0) && foreground_anti_cheat();
        if ac != room.anti_cheat {
            room.anti_cheat = ac;
            if ac {
                eprintln!("controles: o jogo em primeiro plano tem anti-cheat; os controles virtuais saíram");
                for g in &mut room.guests {
                    for (i, p) in g.pads.iter_mut().enumerate() {
                        if p.take().is_some() {
                            g.refused[i] = PAD_ANTI_CHEAT;
                            write(out, &Msg::PadInfo { pad: i as u8, player: 0, slot: 255, why: PAD_ANTI_CHEAT });
                            g.link.send(out);
                        }
                    }
                }
            } else {
                eprintln!("controles: anti-cheat fora do primeiro plano; os controles voltam no próximo toque");
                for g in &mut room.guests {
                    g.refused = [PAD_OK; PADS];
                }
            }
        }
    }
    let mut n = 0;
    for g in &mut room.guests {
        for (i, p) in g.pads.iter_mut().enumerate() {
            let Some(s) = p else { continue };
            n += 1;
            if !s.idle && s.heard.elapsed() > PAD_STALE {
                s.idle = true;
                s.dev.update(&PadState::default());
            }
            let r = s.dev.rumble();
            if r != s.rumble || (r != (0, 0) && s.rumble_sent.elapsed() > Duration::from_millis(250)) {
                s.rumble = r;
                s.rumble_sent = Instant::now();
                write(out, &Msg::Rumble { pad: i as u8, low: r.0, high: r.1 });
                g.link.send(out);
            }
            if repeat_info {
                write(out, &Msg::PadInfo { pad: i as u8, player: s.player(n - 1), slot: s.dev.slot().unwrap_or(255), why: PAD_OK });
                g.link.send(out);
            }
        }
    }
}

/// O vídeo que já está saindo serve para quem chegou?
fn fits(hello: &Hello, codec: Codec, vf: VideoFmt) -> bool {
    if vf.yuv444 {
        return hello.flags & DECODES_444 != 0;
    }
    hello.codecs & codec_bit(codec) != 0 && (!vf.hdr10 || hello.flags & SEES_HDR != 0)
}

/// De onde vem a imagem: WGC (com o cursor do Windows) ou Desktop Duplication (no serviço, para
/// alcançar a tela segura; `PRISMA_CAPTURE=dda` também liga, para teste).
enum Cap {
    Wgc { item: GraphicsCaptureItem, pool: Option<(Direct3D11CaptureFramePool, GraphicsCaptureSession, HANDLE, i64)> },
    Dda(Box<super::screen::Dda>),
}

fn use_dda() -> bool {
    SERVICE.load(Ordering::Relaxed) || std::env::var("PRISMA_CAPTURE").as_deref() == Ok("dda")
}

/// Tamanho da tela principal, pela saída de vídeo (a Desktop Duplication não usa o WGC).
unsafe fn output_size(d: &D3d, mon: HMONITOR) -> Result<(u32, u32)> {
    let ad = d.dev.cast::<windows::Win32::Graphics::Dxgi::IDXGIDevice>()?.GetAdapter()?;
    let mut i = 0;
    loop {
        let o = ad.EnumOutputs(i)?;
        let desc = o.GetDesc()?;
        if desc.Monitor == mon {
            let r = desc.DesktopCoordinates;
            return Ok(((r.right - r.left) as u32, (r.bottom - r.top) as u32));
        }
        i += 1;
    }
}

unsafe fn session(d: &D3d, room: &mut Room, joins: Option<&mpsc::Receiver<Joiner>>) -> Result<End> {
    let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
    let color = monitor_color(mon);
    let mut cap = if use_dda() {
        None
    } else {
        Some(monitor_item(mon)?)
    };
    let (src_w, src_h) = match &cap {
        Some(item) => {
            let s = item.Size()?;
            (even(s.Width as u32), even(s.Height as u32))
        }
        None => {
            let (w, h) = output_size(d, mon)?;
            (even(w), even(h))
        }
    };
    // O tamanho e os Hz seguem quem chegou primeiro; os codecs e formatos, o que todos aceitam.
    let lead = room.guests[0].hello;
    let codecs = room.guests.iter().fold(0xFF, |m, g| m & g.hello.codecs);
    let flags = room.guests.iter().fold(0xFF, |m, g| m & g.hello.flags);
    // Automático: a altura da tela de quem assiste, sem passar da daqui.
    let out_h = if lead.h == 0 || lead.h >= src_h { src_h } else { even(lead.h) };
    let out_w = even((src_w as u64 * out_h as u64 / src_h as u64) as u32);
    // Automático: o menor entre os Hz dos dois lados.
    let fps = monitor_hz(mon).min(if lead.hz > 1 { lead.hz } else { 60 }).clamp(10, 240);

    let wanted: Option<Codec> = match std::env::var("PRISMA_CODEC").as_deref() {
        Ok("h264") => Some(Codec::H264),
        Ok("hevc") => Some(Codec::Hevc),
        Ok("av1") => Some(Codec::Av1),
        _ => None,
    };
    // Formato do vídeo, do melhor ao comum. HDR10 quando as telas estão em HDR; 4:4:4 quando quem
    // assiste pede, porque gasta mais rede. Se o NVENC daqui não fizer, cai para o próximo.
    let want444 = flags & DECODES_444 != 0;
    let want_hdr = color.hdr && flags & SEES_HDR != 0 && std::env::var("PRISMA_HDR10").as_deref() != Ok("0");
    let full = VideoFmt { hdr10: false, yuv444: true };
    let ten = VideoFmt { hdr10: true, yuv444: false };
    let mut tries: Vec<(Codec, VideoFmt)> = Vec::new();
    if want444 {
        tries.push((Codec::Hevc, full));
    }
    if want_hdr {
        tries.extend([(Codec::Av1, ten), (Codec::Hevc, ten)]);
    }
    tries.extend([Codec::Av1, Codec::Hevc, Codec::H264].map(|c| (c, VideoFmt::default())));
    let mut made = None;
    for (c, vf) in tries {
        // O 4:4:4 vai para o NVDEC de quem assiste, não para o Media Foundation da lista de codecs.
        if (!vf.yuv444 && codecs & codec_bit(c) == 0) || wanted.is_some_and(|w| w != c) {
            continue;
        }
        let bits = match std::env::var("PRISMA_MBPS").ok().and_then(|v| v.parse::<u32>().ok()) {
            Some(m) => m * 1_000_000,
            None => (bitrate(out_w, out_h, fps, c) as u64 * 2).min(150_000_000) as u32,
        };
        // O 4:4:4 leva cerca de metade a mais de dados pela mesma nitidez.
        let bits = if vf.yuv444 { (bits as u64 * 3 / 2).min(150_000_000) as u32 } else { bits };
        match LiveEncoder::open(&d.dev, c, out_w, out_h, fps, bits, vf) {
            Ok(e) => {
                let extra = match (vf.hdr10, vf.yuv444) {
                    (true, _) => ", HDR10",
                    (_, true) => ", 4:4:4",
                    _ => "",
                };
                eprintln!("vídeo: {}x{} a {} quadros/s, {}{}, {:.1} Mbit/s ({})", out_w, out_h, fps, codec_name(c), extra, bits as f64 / 1e6, e.name());
                made = Some((e, c, vf));
                break;
            }
            Err(e) if vf != VideoFmt::default() => eprintln!("vídeo: {}{} recusado ({}); tentando o próximo", codec_name(c), if vf.hdr10 { " HDR10" } else { " 4:4:4" }, e.message()),
            Err(_) => {}
        }
    }
    let Some((mut enc, codec, vf)) = made else {
        return Err(Error::new(E_FAIL, "nenhum codec em comum com quem assiste"));
    };

    // HDR10 sai em PQ de 10 bits; senão a tela em HDR vira SDR antes de codificar.
    let mid = bgra_texture(&d.dev, src_w, src_h, if vf.hdr10 { DXGI_FORMAT_R10G10B10A2_UNORM } else { DXGI_FORMAT_B8G8R8A8_UNORM })?;
    // Na Desktop Duplication, a conversão de cor e o cursor ficam com ela.
    let tone = if color.hdr && cap.is_some() { Some(ToneMap::new(&d.dev, &mid, vf.hdr10, color.white_nits)?) } else { None };
    let out_fmt = if vf.yuv444 {
        ConvOut::Ayuv
    } else if vf.hdr10 {
        ConvOut::P010
    } else {
        ConvOut::Nv12
    };
    let mut conv = Prep::new(&d.dev, &d.ctx, &mid, src_w, src_h, out_w, out_h, fps, out_fmt)?;

    let mut cap = match cap.take() {
        Some(item) => {
            let fmt = if color.hdr { DirectXPixelFormat::R16G16B16A16Float } else { DirectXPixelFormat::B8G8R8A8UIntNormalized };
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d.rt, fmt, 2, item.Size()?)?;
            let event = CreateEventW(None, false, false, None)?;
            let ev = event.0 as isize;
            let token = pool.FrameArrived(&TypedEventHandler::new(move |_, _| {
                let _ = SetEvent(HANDLE(ev as *mut _));
                Ok(())
            }))?;
            let capture = pool.CreateCaptureSession(&item)?;
            let _ = capture.SetIsCursorCaptureEnabled(true);
            let _ = capture.SetIsBorderRequired(false);
            let _ = capture.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: 10_000_000 / fps as i64 });
            capture.StartCapture()?;
            Cap::Wgc { item, pool: Some((pool, capture, event, token)) }
        }
        None => {
            let x = super::screen::Dda::new(d, mon, color.hdr, color.white_nits, &mid, vf.hdr10, src_w, src_h)?;
            eprintln!("captura: Desktop Duplication, área de trabalho «{}»", x.desk_name);
            Cap::Dda(Box::new(x))
        }
    };
    let mut fed_at = Instant::now() - Duration::from_secs(1);
    let mut was_secure = false;

    // Som para quem tem a permissão. Mesmo PC: tira o som da janela que assiste.
    let hearing = |guests: &[Guest]| -> Vec<Arc<dyn Link>> { guests.iter().filter(|g| g.perms & SOUND != 0).map(|g| g.link.clone()).collect() };
    let audio_links = Arc::new(Mutex::new(hearing(&room.guests)));
    let exclude = room.guests.iter().find(|g| g.link.same_pc() && g.hello.pid != 0).map(|g| g.hello.pid);
    let stop_audio = Arc::new(AtomicBool::new(false));
    let audio_thread = {
        let (l, stop) = (audio_links.clone(), stop_audio.clone());
        std::thread::Builder::new().name("compartilhar-som".into()).spawn(move || audio(l, exclude, stop)).ok()
    };
    for g in &mut room.guests {
        g.input = (g.perms & (KEYBOARD | MOUSE) != 0).then(|| Injector::new(mon, g.perms));
        g.heard = Instant::now();
    }

    let mut out = Vec::with_capacity(MTU);
    let mut buf = vec![0u8; 2048];
    let info = |perms: u8| Msg::Info { codec, w: out_w, h: out_h, fps, perms, recover: enc.recovers(), hdr10: vf.hdr10, yuv444: vf.yuv444 };
    let infos: Vec<Vec<u8>> = (0..=255u8).map(|p| {
        let mut b = Vec::new();
        write(&mut b, &info(p));
        b
    }).collect();
    // Quadros enviados há pouco, com a hora de cada um, para invalidar os que se perderam.
    let mut sent: VecDeque<(u32, i64)> = VecDeque::new();
    for g in &room.guests {
        g.link.send(&infos[g.perms as usize]);
    }

    let interval = 10_000_000i64 / fps as i64;
    let mut last_fed = i64::MIN / 2;
    let mut pending: Option<i64> = None;
    let mut pkts: Vec<Pkt> = Vec::new();
    let mut frame_id = 0u32;
    let mut result = End::Gone;
    let mut failed = None;
    let mut pad_msgs: Vec<PadMsg> = Vec::new();
    let mut info_tick = Instant::now();

    let mut room_names = String::from("\u{0}");
    'run: loop {
        let names: String = room.guests.iter().map(|g| format!("{}\n", g.name)).collect();
        if names != room_names {
            write_room(&names);
            room_names = names;
        }
        let secure = match &cap {
            Cap::Wgc { pool, .. } => {
                if let Some((_, _, event, _)) = pool {
                    let _ = WaitForSingleObject(*event, 1);
                }
                false
            }
            Cap::Dda(x) => x.secure,
        };
        if secure != was_secure {
            was_secure = secure;
            eprintln!("{}", if secure { "tela segura do Windows (UAC, bloqueio ou PIN)" } else { "de volta à área de trabalho" });
        }
        // Quem chegou entra; se o vídeo atual não serve para ela, recomeça com todo mundo.
        if let Some(rx) = joins {
            while let Ok(j) = rx.try_recv() {
                if room.guests.len() >= max_guests() {
                    eprintln!("«{}» não entrou: a sala já tem {} pessoas (PRISMA_GUESTS)", j.name, room.guests.len());
                    write(&mut out, &Msg::Bye);
                    j.link.send(&out);
                    continue;
                }
                eprintln!("chegou «{}»: tela {}x{} a {} Hz, {} ({})", j.name, j.hello.w, j.hello.h, j.hello.hz, perms_text(j.perms), j.link.route());
                let echo = j.link.same_pc() && exclude.is_none() && j.perms & SOUND != 0;
                let ok = fits(&j.hello, codec, vf) && !echo;
                let mut g = Guest::new(j);
                if ok {
                    g.input = (g.perms & (KEYBOARD | MOUSE) != 0).then(|| Injector::new(mon, g.perms));
                    g.link.send(&infos[g.perms as usize]);
                    enc.force_key();
                    room.guests.push(g);
                    *audio_links.lock().unwrap() = hearing(&room.guests);
                } else {
                    room.guests.push(g);
                    result = End::Again;
                    break 'run;
                }
            }
        }
        // Texto copiado no host vai para quem tem a permissão (duas vezes, contra perda).
        if room.clip.is_none() && room.guests.iter().any(|g| g.perms & COPY != 0) {
            room.clip = Some(super::clip::start());
        }
        if let Some(c) = &room.clip {
            while let Ok(t) = c.changed.try_recv() {
                room.clip_id = room.clip_id.wrapping_add(1);
                let pk = super::clip::packets(&t, room.clip_id);
                for g in room.guests.iter().filter(|g| g.perms & COPY != 0) {
                    for p in pk.iter().chain(pk.iter()) {
                        g.link.send(p);
                    }
                }
            }
        }
        // O que cada pessoa mandou.
        for (gi, g) in room.guests.iter_mut().enumerate() {
            loop {
                match g.link.recv(&mut buf, Duration::ZERO) {
                    Poll::Got(n) => {
                        let Some(m) = read(&buf[..n]) else { continue };
                        if let Some(h) = Hello::from_msg(&m) {
                            // Reabriu: recomeça com a tela nova dela.
                            g.hello = h;
                            result = End::Again;
                            break 'run;
                        }
                        g.heard = Instant::now();
                        match m {
                            Msg::Ping { t } => g.send(&Msg::Pong { t, host: now() }, &mut out),
                            Msg::KeyReq => enc.force_key(),
                            Msg::Nack { frame } => {
                                // Para de usar o quadro perdido e os enviados depois dele, que dependiam dele.
                                let mut ok = false;
                                let mut gone = 0;
                                for &(id, pts) in sent.iter() {
                                    if id == frame || (id.wrapping_sub(frame) as i32) > 0 {
                                        gone += 1;
                                        ok = enc.invalidate(pts) || ok;
                                    }
                                }
                                // Sem como invalidar, ou perda antiga demais: quadro-chave.
                                if !ok || gone == 0 || gone > 16 {
                                    enc.force_key();
                                }
                            }
                            Msg::Input { first, events } => {
                                // Na tela segura, só quem tem a permissão de sistema.
                                if let Some(inj) = g.input.as_mut().filter(|_| !secure || g.perms & SYSTEM != 0) {
                                    inj.apply(first, events);
                                }
                            }
                            Msg::Clip { id, part, parts, data } if g.perms & COPY != 0 => {
                                if let (Some(c), Some(t)) = (&room.clip, g.clip_in.add(id, part, parts, data)) {
                                    let _ = c.set.send(t);
                                }
                            }
                            Msg::Sas => {
                                if g.perms & SYSTEM == 0 {
                                    eprintln!("Ctrl+Alt+Del de «{}» recusado: sem a permissão de sistema", g.name);
                                } else {
                                    #[cfg(feature = "stream")]
                                    super::service::request_sas(&g.name);
                                    #[cfg(not(feature = "stream"))]
                                    eprintln!("Ctrl+Alt+Del só existe com o serviço");
                                }
                            }
                            Msg::Pad { pad, seq, on, state } => pad_msgs.push(PadMsg { guest: gi, pad, seq, on, state }),
                            Msg::Bye => {
                                g.gone = true;
                                break;
                            }
                            _ => {}
                        }
                    }
                    Poll::Stranger(n, from) => {
                        // Outro chegou pelo UDP: passa a vez para ele.
                        if let Some(h) = read(&buf[..n]).as_ref().and_then(Hello::from_msg) {
                            result = End::Stranger(h, from);
                            break 'run;
                        }
                    }
                    Poll::Empty => break,
                    Poll::Closed => {
                        g.gone = true;
                        break;
                    }
                }
            }
            if g.heard.elapsed() > SILENCE {
                g.gone = true;
            }
        }
        for m in pad_msgs.drain(..) {
            pad_msg(room, m, &mut out);
        }
        let repeat = info_tick.elapsed() > Duration::from_secs(2);
        if repeat {
            info_tick = Instant::now();
        }
        pads_tick(room, &mut out, repeat);
        if room.guests.iter().any(|g| g.gone) {
            for g in room.guests.iter_mut().filter(|g| g.gone) {
                if let Some(inj) = &mut g.input {
                    inj.release();
                }
                eprintln!("«{}» saiu", g.name);
            }
            room.guests.retain(|g| !g.gone);
            *audio_links.lock().unwrap() = hearing(&room.guests);
            if room.guests.is_empty() {
                break;
            }
        }

        let mut changed = false;
        let step = (|| -> Result<()> {
            let pool = match &mut cap {
                Cap::Wgc { pool: Some((pool, ..)), .. } => pool,
                Cap::Wgc { .. } => return Ok(()),
                Cap::Dda(x) => {
                    match x.poll(&mid)? {
                        super::screen::Got::Frame(pts) => pending = Some(pts),
                        super::screen::Got::Changed => changed = true,
                        super::screen::Got::Nothing => {}
                    }
                    enc.pump(&mut pkts)?;
                    // A Desktop Duplication entrega cada mudança (o mouse também): o ritmo fica aqui.
                    if enc.ready() && fed_at.elapsed() >= Duration::from_nanos(interval as u64 * 90) {
                        if let Some(pts) = pending.take() {
                            let t = conv.convert(&d.ctx)?;
                            enc.feed(&t, pts)?;
                            last_fed = pts;
                            fed_at = Instant::now();
                        }
                    }
                    return Ok(());
                }
            };
            while let Ok(f) = pool.TryGetNextFrame() {
                let pts = f.SystemRelativeTime()?.Duration;
                if pts - last_fed < interval * 9 / 10 {
                    let _ = f.Close();
                    continue;
                }
                let access: IDirect3DDxgiInterfaceAccess = f.Surface()?.cast()?;
                let tex: ID3D11Texture2D = access.GetInterface()?;
                if let Some(t) = &tone {
                    t.draw(&d.ctx, &d.dev, &tex, src_w, src_h)?;
                } else {
                    let bx = D3D11_BOX { left: 0, top: 0, front: 0, right: src_w, bottom: src_h, back: 1 };
                    d.ctx.CopySubresourceRegion(&mid, 0, 0, 0, 0, &tex, 0, Some(&bx));
                }
                let _ = f.Close();
                pending = Some(pts);
            }
            enc.pump(&mut pkts)?;
            if enc.ready() {
                if let Some(pts) = pending.take() {
                    let t = conv.convert(&d.ctx)?;
                    enc.feed(&t, pts)?;
                    last_fed = pts;
                }
            }
            Ok(())
        })();
        if let Err(e) = step {
            failed = Some(e);
            break;
        }
        if changed {
            eprintln!("a tela mudou de tamanho: recomeçando");
            result = End::Again;
            break;
        }
        // O mesmo vídeo para todo mundo, em pedaços que cabem no caminho mais estreito.
        let room_mtu = room.guests.iter().map(|g| g.link.mtu()).min().unwrap_or(MTU);
        for p in pkts.drain(..) {
            if p.key {
                // Repete a descrição com cada quadro-chave, caso a primeira tenha se perdido.
                for g in &room.guests {
                    g.link.send(&infos[g.perms as usize]);
                }
            }
            let space = room_mtu - VIDEO_HEAD;
            let count = p.data.len().div_ceil(space).max(1);
            if count > u16::MAX as usize {
                continue;
            }
            for (idx, chunk) in p.data.chunks(space).enumerate() {
                write(&mut out, &Msg::Video { key: p.key, idx: idx as u16, count: count as u16, frame: frame_id, pts: p.pts, data: chunk });
                for g in &room.guests {
                    g.link.send(&out);
                }
            }
            sent.push_back((frame_id, p.pts));
            if sent.len() > 32 {
                sent.pop_front();
            }
            frame_id = frame_id.wrapping_add(1);
        }
    }
    for g in &mut room.guests {
        if let Some(inj) = &mut g.input {
            inj.release();
        }
        g.input = None;
    }
    stop_audio.store(true, Ordering::Relaxed);
    if let Some(t) = audio_thread {
        let _ = t.join();
    }
    if let Cap::Wgc { item, pool: Some((pool, capture, event, token)) } = cap {
        let _ = pool.RemoveFrameArrived(token);
        let _ = capture.Close();
        let _ = pool.Close();
        let _ = CloseHandle(event);
        drop(item);
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(result),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeLink {
        sent: Mutex<Vec<Vec<u8>>>,
        local: bool,
    }

    impl Link for FakeLink {
        fn send(&self, b: &[u8]) {
            self.sent.lock().unwrap().push(b.to_vec());
        }
        fn recv(&self, _: &mut [u8], _: Duration) -> Poll {
            Poll::Empty
        }
        fn same_pc(&self) -> bool {
            self.local
        }
    }

    fn guest(perms: u8, local: bool) -> (Guest, Arc<FakeLink>) {
        let link = Arc::new(FakeLink { sent: Mutex::new(Vec::new()), local });
        let hello = Hello { w: 1920, h: 1080, hz: 60, codecs: 7, pid: 0, flags: 0 };
        (Guest::new(Joiner { link: link.clone(), hello, perms, name: "teste".into() }), link)
    }

    /// Os avisos de controle que a pessoa recebeu: (controle, jogador, motivo).
    fn infos(l: &FakeLink) -> Vec<(u8, u8, u8)> {
        l.sent.lock().unwrap().iter().filter_map(|b| match read(b) {
            Some(Msg::PadInfo { pad, player, why, .. }) => Some((pad, player, why)),
            _ => None,
        }).collect()
    }

    fn room(guests: Vec<Guest>) -> (Room, super::super::pads::FakeLog) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let r = Room { guests, bridge: Some(Bridge::fake(log.clone())), bridge_err: None, anti_cheat: false, checked: Instant::now(), clip: None, clip_id: 0 };
        (r, log)
    }

    fn msg(guest: usize, pad: u8, seq: u16, buttons: u16) -> PadMsg {
        PadMsg { guest, pad, seq, on: true, state: PadState { buttons, ..Default::default() } }
    }

    #[test]
    fn pads_need_permission() {
        let (g, l) = guest(SOUND | KEYBOARD | MOUSE, false);
        let (mut r, _) = room(vec![g]);
        let mut out = Vec::new();
        pad_msg(&mut r, msg(0, 0, 1, 1), &mut out);
        pad_msg(&mut r, msg(0, 0, 2, 1), &mut out);
        assert_eq!(r.pads(), 0);
        // Um aviso só, não um por pacote.
        assert_eq!(infos(&l), vec![(0, 0, PAD_NO_PERMISSION)]);
    }

    #[test]
    fn pads_plug_in_order_and_ignore_old_packets() {
        let (g, l) = guest(SOUND | PAD, false);
        let (mut r, log) = room(vec![g]);
        let mut out = Vec::new();
        pad_msg(&mut r, msg(0, 2, 10, 0x1000), &mut out);
        assert_eq!(infos(&l), vec![(2, 1, PAD_OK)]);
        pad_msg(&mut r, msg(0, 2, 9, 0x2000), &mut out);
        pad_msg(&mut r, msg(0, 2, 11, 0x4000), &mut out);
        let seen: Vec<u16> = log.lock().unwrap().iter().map(|x| x.1.buttons).filter(|&b| b != 0xFFFF).collect();
        assert_eq!(seen, vec![0x1000, 0x4000]);
        // A sequência dá a volta sem confundir o mais novo.
        r.guests[0].pads[2].as_mut().unwrap().seq = 65535;
        pad_msg(&mut r, msg(0, 2, 0, 0x8000), &mut out);
        assert_eq!(log.lock().unwrap().last().unwrap().1.buttons, 0x8000);
        // Controle que saiu solta a vaga.
        pad_msg(&mut r, PadMsg { guest: 0, pad: 2, seq: 1, on: false, state: PadState::default() }, &mut out);
        assert_eq!(r.pads(), 0);
    }

    #[test]
    fn four_pads_at_most() {
        let (a, _) = guest(PAD, false);
        let (b, lb) = guest(PAD, false);
        let (mut r, _) = room(vec![a, b]);
        let mut out = Vec::new();
        for p in 0..3 {
            pad_msg(&mut r, msg(0, p, 1, 0), &mut out);
        }
        pad_msg(&mut r, msg(1, 0, 1, 0), &mut out);
        pad_msg(&mut r, msg(1, 1, 1, 0), &mut out);
        assert_eq!(r.pads(), 4);
        assert_eq!(infos(&lb), vec![(0, 4, PAD_OK), (1, 0, PAD_FULL)]);
    }

    #[test]
    fn same_pc_does_not_loop() {
        let (g, l) = guest(PAD, true);
        let (mut r, _) = room(vec![g]);
        let mut out = Vec::new();
        // O controle de verdade (vaga 1 lá) vira o virtual da vaga 0; o virtual, visto como
        // controle 0 por quem assiste, não pode virar outro.
        pad_msg(&mut r, msg(0, 1, 1, 0), &mut out);
        pad_msg(&mut r, msg(0, 0, 1, 0), &mut out);
        assert_eq!(r.pads(), 1);
        assert_eq!(infos(&l), vec![(1, 1, PAD_OK), (0, 0, PAD_LOOP)]);
    }

    #[test]
    fn joiner_fits_current_video() {
        let h = |codecs, flags| Hello { w: 0, h: 0, hz: 0, codecs, pid: 0, flags };
        let plain = VideoFmt::default();
        assert!(fits(&h(2, 0), Codec::Hevc, plain));
        assert!(!fits(&h(1, 0), Codec::Hevc, plain));
        assert!(!fits(&h(7, 0), Codec::Av1, VideoFmt { hdr10: true, yuv444: false }));
        assert!(fits(&h(7, SEES_HDR), Codec::Av1, VideoFmt { hdr10: true, yuv444: false }));
        assert!(!fits(&h(7, SEES_HDR), Codec::Hevc, VideoFmt { hdr10: false, yuv444: true }));
        assert!(fits(&h(0, DECODES_444), Codec::Hevc, VideoFmt { hdr10: false, yuv444: true }));
    }
}
