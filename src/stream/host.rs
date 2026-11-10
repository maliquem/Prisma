//! Quem compartilha: captura a tela principal e o som, codifica e manda para quem assiste.
//! No modo de controle, aplica o mouse e o teclado de quem assiste.

use std::collections::HashSet;
use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

use super::*;
use crate::capture::encoder::Encoder;
use crate::capture::engine::{bgra_texture, bitrate, even, Converter};
use crate::capture::hdr::ToneMap;
use crate::capture::mux::{Codec, Pkt};
use crate::capture::sound;
use crate::capture::{d3d, monitor_color, monitor_item, D3d};

/// Sem notícia de quem assiste por este tempo, a sessão acaba.
const SILENCE: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
struct Hello {
    from: SocketAddr,
    w: u32,
    h: u32,
    hz: u32,
    codecs: u8,
    pid: u32,
}

enum End {
    Gone,
    Again(Hello),
}

pub fn run(port: u16, control: bool) {
    let sock = match UdpSocket::bind(("0.0.0.0", port)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("não deu para abrir a porta {port}: {e}");
            return;
        }
    };
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        let _ = timeBeginPeriod(1);
    }
    let d = match d3d() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("sem Direct3D: {e:?}");
            return;
        }
    };
    unsafe {
        if let Ok(mt) = d.ctx.cast::<ID3D11Multithread>() {
            let _ = mt.SetMultithreadProtected(true);
        }
    }
    let mode = if control { "com mouse e teclado de quem assiste" } else { "só para ver" };
    eprintln!("Prisma compartilhando ({mode}): esperando quem assiste na porta UDP {port}");
    let mut buf = vec![0u8; 2048];
    let mut next: Option<Hello> = None;
    loop {
        let hello = match next.take() {
            Some(h) => h,
            None => {
                let _ = sock.set_nonblocking(false);
                let _ = sock.set_read_timeout(None);
                let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                match read(&buf[..n]) {
                    Some(Msg::Hello { w, h, hz, codecs, pid }) => Hello { from, w, h, hz, codecs, pid },
                    _ => continue,
                }
            }
        };
        eprintln!("{} chegou: tela {}x{} a {} Hz", hello.from, hello.w, hello.h, hello.hz);
        match unsafe { session(&d, &sock, hello, control) } {
            Ok(End::Gone) => eprintln!("{} saiu", hello.from),
            Ok(End::Again(h)) => next = Some(h),
            Err(e) => eprintln!("a sessão parou: {e:?}"),
        }
    }
}

/// Manda um pacote; com a fila do Windows cheia, tenta de novo por um instante.
fn send(sock: &UdpSocket, to: SocketAddr, b: &[u8]) {
    let t = Instant::now();
    loop {
        match sock.send_to(b, to) {
            Err(e) if e.kind() == ErrorKind::WouldBlock && t.elapsed() < Duration::from_millis(20) => std::hint::spin_loop(),
            _ => return,
        }
    }
}

/// Som do PC em blocos de 5 ms. Com os dois lados no mesmo PC, tira o som de quem assiste,
/// senão ele voltaria em eco.
fn audio(sock: UdpSocket, peer: SocketAddr, exclude: Option<u32>, stop: Arc<AtomicBool>) {
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
                send(&sock, peer, &out);
                seq = seq.wrapping_add(1);
            }
        }
        let _ = src.client.Stop();
    }
}

/// Aplica no PC o mouse e o teclado de quem assiste.
struct Injector {
    /// Monitor transmitido e área de todos os monitores, em pixels.
    mon: RECT,
    virt: RECT,
    next: Option<u32>,
    keys: HashSet<(u16, bool)>,
    buttons: [bool; 5],
}

impl Injector {
    fn new(mon: HMONITOR) -> Self {
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        unsafe {
            let _ = GetMonitorInfoW(mon, &mut mi);
        }
        let virt = unsafe {
            let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
            RECT { left: x, top: y, right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN), bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN) }
        };
        Self { mon: mi.rcMonitor, virt, next: None, keys: HashSet::new(), buttons: [false; 5] }
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

unsafe fn session(d: &D3d, sock: &UdpSocket, hello: Hello, control: bool) -> Result<End> {
    let peer = hello.from;
    let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
    let item = monitor_item(mon)?;
    let color = monitor_color(mon);
    let size = item.Size()?;
    let (src_w, src_h) = (even(size.Width as u32), even(size.Height as u32));
    // Automático: a altura da tela de quem assiste, sem passar da daqui.
    let out_h = if hello.h == 0 || hello.h >= src_h { src_h } else { even(hello.h) };
    let out_w = even((src_w as u64 * out_h as u64 / src_h as u64) as u32);
    // Automático: o menor entre os Hz dos dois lados.
    let fps = monitor_hz(mon).min(if hello.hz > 1 { hello.hz } else { 60 }).clamp(10, 240);

    let mid = bgra_texture(&d.dev, src_w, src_h, DXGI_FORMAT_B8G8R8A8_UNORM)?;
    let tone = if color.hdr { Some(ToneMap::new(&d.dev, &mid, false, color.white_nits)?) } else { None };
    let mut conv = Converter::new(&d.dev, &d.ctx, src_w, src_h, out_w, out_h, fps, false)?;
    let in_view = conv.input_view(&mid)?;

    let wanted: Option<Codec> = match std::env::var("PRISMA_CODEC").as_deref() {
        Ok("h264") => Some(Codec::H264),
        Ok("hevc") => Some(Codec::Hevc),
        Ok("av1") => Some(Codec::Av1),
        _ => None,
    };
    let mut made = None;
    for c in [Codec::Av1, Codec::Hevc, Codec::H264] {
        if hello.codecs & codec_bit(c) == 0 || wanted.is_some_and(|w| w != c) {
            continue;
        }
        let bits = match std::env::var("PRISMA_MBPS").ok().and_then(|v| v.parse::<u32>().ok()) {
            Some(m) => m * 1_000_000,
            None => (bitrate(out_w, out_h, fps, c) as u64 * 2).min(150_000_000) as u32,
        };
        if let Ok(e) = Encoder::live(&d.dev, c, out_w, out_h, fps, bits) {
            eprintln!("vídeo: {}x{} a {} quadros/s, {}, {:.1} Mbit/s ({})", out_w, out_h, fps, codec_name(c), bits as f64 / 1e6, e.name);
            made = Some((e, c));
            break;
        }
    }
    let Some((mut enc, codec)) = made else {
        return Err(Error::new(E_FAIL, "nenhum codec em comum com quem assiste"));
    };

    let fmt = if color.hdr { DirectXPixelFormat::R16G16B16A16Float } else { DirectXPixelFormat::B8G8R8A8UIntNormalized };
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d.rt, fmt, 2, size)?;
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

    let stop_audio = Arc::new(AtomicBool::new(false));
    let audio_thread = match sock.try_clone() {
        Ok(s) => {
            // Mesmo PC: tira o som da janela que assiste.
            let exclude = (peer.ip().is_loopback() && hello.pid != 0).then_some(hello.pid);
            let stop = stop_audio.clone();
            std::thread::Builder::new().name("compartilhar-som".into()).spawn(move || audio(s, peer, exclude, stop)).ok()
        }
        Err(_) => None,
    };
    let mut input = control.then(|| Injector::new(mon));

    let _ = sock.set_nonblocking(true);
    let mut out = Vec::with_capacity(MTU);
    let mut buf = vec![0u8; 2048];
    let info = Msg::Info { codec, w: out_w, h: out_h, fps, control };
    write(&mut out, &info);
    send(sock, peer, &out);

    let interval = 10_000_000i64 / fps as i64;
    let mut last_fed = i64::MIN / 2;
    let mut pending: Option<i64> = None;
    let mut pkts: Vec<Pkt> = Vec::new();
    let mut frame_id = 0u32;
    let mut heard = Instant::now();
    let mut result = End::Gone;
    let mut failed = None;

    'run: loop {
        let _ = WaitForSingleObject(event, 1);
        // O que quem assiste mandou.
        loop {
            match sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    let Some(m) = read(&buf[..n]) else { continue };
                    if let Msg::Hello { w, h, hz, codecs, pid } = m {
                        // Quem assiste reabriu, ou outro chegou: recomeça com a tela dele.
                        result = End::Again(Hello { from, w, h, hz, codecs, pid });
                        break 'run;
                    }
                    if from != peer {
                        continue;
                    }
                    heard = Instant::now();
                    match m {
                        Msg::Ping { t } => {
                            write(&mut out, &Msg::Pong { t, host: now() });
                            send(sock, peer, &out);
                        }
                        Msg::KeyReq => enc.force_key(),
                        Msg::Input { first, events } => {
                            if let Some(inj) = &mut input {
                                inj.apply(first, events);
                            }
                        }
                        Msg::Bye => break 'run,
                        _ => {}
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                // Resposta de porta fechada do outro lado: o Windows avisa no próximo recv.
                Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
                Err(_) => break,
            }
        }
        if heard.elapsed() > SILENCE {
            break;
        }

        let step = (|| -> Result<()> {
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
            if enc.need > 0 {
                if let Some(pts) = pending.take() {
                    let t = conv.convert(&in_view)?;
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
        for p in pkts.drain(..) {
            if p.key {
                // Repete a descrição com cada quadro-chave, caso a primeira tenha se perdido.
                write(&mut out, &info);
                send(sock, peer, &out);
            }
            let room = MTU - VIDEO_HEAD;
            let count = p.data.len().div_ceil(room).max(1);
            if count > u16::MAX as usize {
                continue;
            }
            for (idx, chunk) in p.data.chunks(room).enumerate() {
                write(&mut out, &Msg::Video { key: p.key, idx: idx as u16, count: count as u16, frame: frame_id, pts: p.pts, data: chunk });
                send(sock, peer, &out);
            }
            frame_id = frame_id.wrapping_add(1);
        }
    }
    if let Some(inj) = &mut input {
        inj.release();
    }
    stop_audio.store(true, Ordering::Relaxed);
    if let Some(t) = audio_thread {
        let _ = t.join();
    }
    let _ = pool.RemoveFrameArrived(token);
    let _ = capture.Close();
    let _ = pool.Close();
    let _ = CloseHandle(event);
    match failed {
        Some(e) => Err(e),
        None => Ok(result),
    }
}
