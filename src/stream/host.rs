//! Quem compartilha: captura a tela principal, codifica e manda para quem assiste.

use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::*;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::Media::timeBeginPeriod;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::WinRT::Direct3D11::*;

use super::*;
use crate::capture::encoder::Encoder;
use crate::capture::engine::{bgra_texture, bitrate, even, Converter};
use crate::capture::hdr::ToneMap;
use crate::capture::mux::{Codec, Pkt};
use crate::capture::{d3d, monitor_color, monitor_item, D3d};

/// Sem notícia de quem assiste por este tempo, a sessão acaba.
const SILENCE: Duration = Duration::from_secs(5);

enum End {
    Gone,
    Again(SocketAddr, u32, u32, u32, u8),
}

pub fn run(port: u16) {
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
    eprintln!("Prisma compartilhando: esperando quem assiste na porta UDP {port}");
    let mut buf = vec![0u8; 2048];
    let mut next: Option<(SocketAddr, u32, u32, u32, u8)> = None;
    loop {
        let hello = match next.take() {
            Some(h) => h,
            None => {
                let _ = sock.set_nonblocking(false);
                let _ = sock.set_read_timeout(None);
                let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                match read(&buf[..n]) {
                    Some(Msg::Hello { w, h, hz, codecs }) => (from, w, h, hz, codecs),
                    _ => continue,
                }
            }
        };
        eprintln!("{} chegou: tela {}x{} a {} Hz", hello.0, hello.1, hello.2, hello.3);
        match unsafe { session(&d, &sock, hello) } {
            Ok(End::Gone) => eprintln!("{} saiu", hello.0),
            Ok(End::Again(a, w, h, hz, c)) => next = Some((a, w, h, hz, c)),
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

unsafe fn session(d: &D3d, sock: &UdpSocket, hello: (SocketAddr, u32, u32, u32, u8)) -> Result<End> {
    let (peer, cw, ch, chz, codecs) = hello;
    let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
    let item = monitor_item(mon)?;
    let color = monitor_color(mon);
    let size = item.Size()?;
    let (src_w, src_h) = (even(size.Width as u32), even(size.Height as u32));
    // Automático: a altura da tela de quem assiste, sem passar da daqui.
    let out_h = if ch == 0 || ch >= src_h { src_h } else { even(ch) };
    let out_w = even((src_w as u64 * out_h as u64 / src_h as u64) as u32);
    // Automático: o menor entre os Hz dos dois lados.
    let fps = monitor_hz(mon).min(if chz > 1 { chz } else { 60 }).clamp(10, 240);
    let _ = cw;

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
        if codecs & codec_bit(c) == 0 || wanted.is_some_and(|w| w != c) {
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

    let _ = sock.set_nonblocking(true);
    let mut out = Vec::with_capacity(MTU);
    let mut buf = vec![0u8; 2048];
    let info = Msg::Info { codec, w: out_w, h: out_h, fps };
    write(&mut out, &info);
    send(sock, peer, &out);

    let interval = 10_000_000i64 / fps as i64;
    let mut last_fed = i64::MIN / 2;
    let mut pending: Option<i64> = None;
    let mut pkts: Vec<Pkt> = Vec::new();
    let mut frame_id = 0u32;
    let mut heard = Instant::now();
    let mut result = End::Gone;

    'run: loop {
        let _ = WaitForSingleObject(event, 1);
        // O que quem assiste mandou.
        loop {
            match sock.recv_from(&mut buf) {
                Ok((n, from)) => {
                    let Some(m) = read(&buf[..n]) else { continue };
                    if let Msg::Hello { w, h, hz, codecs } = m {
                        // Quem assiste reabriu, ou outro chegou: recomeça com a tela dele.
                        result = End::Again(from, w, h, hz, codecs);
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
    let _ = pool.RemoveFrameArrived(token);
    let _ = capture.Close();
    let _ = pool.Close();
    let _ = CloseHandle(event);
    Ok(result)
}
