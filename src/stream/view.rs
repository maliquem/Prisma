//! Quem assiste: recebe o vídeo, decodifica na GPU e mostra numa janela.
//! F11 alterna tela cheia sem borda. Fechar a janela avisa o host.

use std::collections::VecDeque;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::timeBeginPeriod;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::VK_F11;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::*;
use crate::capture::decoder::{can_decode, Decoded, Decoder};
use crate::capture::mux::{Codec, Pkt};
use crate::capture::{d3d, D3d};

struct Frame {
    key: bool,
    pts: i64,
    data: Vec<u8>,
}

enum Event {
    Info { codec: Codec, w: u32, h: u32, fps: u32 },
    Frame(Frame),
}

/// O que a linha de recepção divide com a janela.
struct Shared {
    queue: Mutex<VecDeque<Event>>,
    wake: isize,
    quit: AtomicBool,
    got_info: AtomicBool,
    /// Ida e volta da rede, em 100 ns, da melhor medida recente.
    rtt: AtomicI64,
    /// Relógio do host menos o daqui, em 100 ns.
    offset: AtomicI64,
    lost: AtomicU32,
    bytes: AtomicU64,
}

/// Junta os pedaços de um quadro.
struct Asm {
    frame: u32,
    count: u16,
    got: u16,
    key: bool,
    pts: i64,
    parts: Vec<Option<Vec<u8>>>,
}

fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

fn receive(sock: UdpSocket, host: SocketAddr, hello: Vec<u8>, sh: Arc<Shared>) {
    let _ = sock.set_read_timeout(Some(Duration::from_millis(20)));
    let mut buf = vec![0u8; 2048];
    let mut out = Vec::with_capacity(64);
    let mut cur: Option<Asm> = None;
    let mut expect: Option<u32> = None;
    let mut waiting_key = true;
    let mut last_req = Instant::now() - Duration::from_secs(1);
    let mut last_ping = Instant::now() - Duration::from_secs(1);
    let mut last_hello = Instant::now() - Duration::from_secs(2);
    // Medidas recentes de ida e volta, com a diferença de relógio de cada uma.
    let mut samples: VecDeque<(i64, i64)> = VecDeque::new();

    let push = |e: Event| {
        sh.queue.lock().unwrap().push_back(e);
        unsafe {
            let _ = SetEvent(HANDLE(sh.wake as *mut _));
        }
    };

    while !sh.quit.load(Ordering::Relaxed) {
        if !sh.got_info.load(Ordering::Relaxed) && last_hello.elapsed() > Duration::from_secs(1) {
            let _ = sock.send_to(&hello, host);
            last_hello = Instant::now();
        }
        if last_ping.elapsed() > Duration::from_millis(500) {
            write(&mut out, &Msg::Ping { t: now() });
            let _ = sock.send_to(&out, host);
            last_ping = Instant::now();
        }
        if waiting_key && sh.got_info.load(Ordering::Relaxed) && last_req.elapsed() > Duration::from_millis(150) {
            write(&mut out, &Msg::KeyReq);
            let _ = sock.send_to(&out, host);
            last_req = Instant::now();
        }
        let n = match sock.recv_from(&mut buf) {
            Ok((n, from)) if from == host => n,
            _ => continue,
        };
        sh.bytes.fetch_add(n as u64, Ordering::Relaxed);
        match read(&buf[..n]) {
            Some(Msg::Info { codec, w, h, fps }) => {
                sh.got_info.store(true, Ordering::Relaxed);
                push(Event::Info { codec, w, h, fps });
            }
            Some(Msg::Pong { t, host }) => {
                let back = now();
                let rtt = back - t;
                samples.push_back((rtt, host - (t + back) / 2));
                if samples.len() > 20 {
                    samples.pop_front();
                }
                if let Some(&(r, o)) = samples.iter().min_by_key(|s| s.0) {
                    sh.rtt.store(r, Ordering::Relaxed);
                    sh.offset.store(o, Ordering::Relaxed);
                }
            }
            Some(Msg::Video { key, idx, count, frame, pts, data }) => {
                if count == 0 || idx >= count || expect.is_some_and(|e| newer(e, frame)) {
                    continue;
                }
                if let Some(a) = &cur {
                    if a.frame != frame {
                        if !newer(frame, a.frame) {
                            continue;
                        }
                        // Chegou pedaço de um quadro mais novo com este incompleto: perdeu-se algo.
                        sh.lost.fetch_add(1, Ordering::Relaxed);
                        waiting_key = true;
                        cur = None;
                    }
                }
                let a = cur.get_or_insert_with(|| Asm { frame, count, got: 0, key, pts, parts: vec![None; count as usize] });
                if a.count != count {
                    continue;
                }
                let slot = &mut a.parts[idx as usize];
                if slot.is_none() {
                    *slot = Some(data.to_vec());
                    a.got += 1;
                }
                if a.got == a.count {
                    let a = cur.take().unwrap();
                    if expect.is_some_and(|e| e != a.frame) {
                        // Um quadro inteiro sumiu no caminho.
                        sh.lost.fetch_add(1, Ordering::Relaxed);
                        waiting_key = true;
                    }
                    expect = Some(a.frame.wrapping_add(1));
                    if a.key {
                        waiting_key = false;
                    }
                    if !waiting_key {
                        let data = a.parts.into_iter().flatten().flatten().collect();
                        push(Event::Frame(Frame { key: a.key, pts: a.pts, data }));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Converte o quadro decodificado (NV12) para a tela e encaixa com faixas pretas.
struct Scaler {
    vd: ID3D11VideoDevice,
    vc: ID3D11VideoContext1,
    en: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
    size: (u32, u32, u32, u32),
}

impl Scaler {
    unsafe fn new(d: &D3d, iw: u32, ih: u32, ow: u32, oh: u32, fps: u32) -> Result<Self> {
        let vd: ID3D11VideoDevice = d.dev.cast()?;
        let vc: ID3D11VideoContext1 = d.ctx.cast()?;
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
            InputWidth: iw,
            InputHeight: ih,
            OutputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
            OutputWidth: ow,
            OutputHeight: oh,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        let en = vd.CreateVideoProcessorEnumerator(&desc)?;
        let vp = vd.CreateVideoProcessor(&en, 0)?;
        vc.VideoProcessorSetStreamColorSpace1(&vp, 0, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709);
        vc.VideoProcessorSetOutputColorSpace1(&vp, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709);
        vc.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        vc.VideoProcessorSetStreamAutoProcessingMode(&vp, 0, false);
        let black = D3D11_VIDEO_COLOR { Anonymous: D3D11_VIDEO_COLOR_0 { RGBA: D3D11_VIDEO_COLOR_RGBA { R: 0.0, G: 0.0, B: 0.0, A: 1.0 } } };
        vc.VideoProcessorSetOutputBackgroundColor(&vp, false, &black);
        // Tamanho certo do vídeo: o decodificador pode entregar a textura com sobra.
        let src = RECT { left: 0, top: 0, right: iw as i32, bottom: ih as i32 };
        vc.VideoProcessorSetStreamSourceRect(&vp, 0, true, Some(&src));
        let scale = (ow as f64 / iw as f64).min(oh as f64 / ih as f64);
        let (dw, dh) = ((iw as f64 * scale).round() as i32, (ih as f64 * scale).round() as i32);
        let (x, y) = ((ow as i32 - dw) / 2, (oh as i32 - dh) / 2);
        vc.VideoProcessorSetStreamDestRect(&vp, 0, true, Some(&RECT { left: x, top: y, right: x + dw, bottom: y + dh }));
        vc.VideoProcessorSetOutputTargetRect(&vp, true, Some(&RECT { left: 0, top: 0, right: ow as i32, bottom: oh as i32 }));
        Ok(Self { vd, vc, en, vp, size: (iw, ih, ow, oh) })
    }

    unsafe fn blt(&self, tex: &ID3D11Texture2D, slice: u32, target: &ID3D11Texture2D) -> Result<()> {
        let iv = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: slice } },
        };
        let mut input = None;
        self.vd.CreateVideoProcessorInputView(tex, &self.en, &iv, Some(&mut input))?;
        let ov = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
        };
        let mut output = None;
        self.vd.CreateVideoProcessorOutputView(target, &self.en, &ov, Some(&mut output))?;
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: std::mem::ManuallyDrop::new(input),
            ..Default::default()
        };
        let mut streams = [stream];
        let r = self.vc.VideoProcessorBlt(&self.vp, output.as_ref().unwrap(), 0, &streams);
        std::mem::ManuallyDrop::drop(&mut streams[0].pInputSurface);
        r
    }
}

static RESIZED: AtomicBool = AtomicBool::new(false);
static SAVED: Mutex<Option<(isize, RECT)>> = Mutex::new(None);

/// Tela cheia sem borda e volta.
unsafe fn toggle_full(h: HWND) {
    let mut saved = SAVED.lock().unwrap();
    if let Some((style, r)) = saved.take() {
        SetWindowLongPtrW(h, GWL_STYLE, style);
        let _ = SetWindowPos(h, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_FRAMECHANGED | SWP_NOZORDER);
    } else {
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        *saved = Some((GetWindowLongPtrW(h, GWL_STYLE), r));
        let mon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let m = mi.rcMonitor;
        SetWindowLongPtrW(h, GWL_STYLE, (WS_POPUP | WS_VISIBLE).0 as isize);
        let _ = SetWindowPos(h, None, m.left, m.top, m.right - m.left, m.bottom - m.top, SWP_FRAMECHANGED | SWP_NOZORDER);
    }
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_SIZE => {
            RESIZED.store(true, Ordering::Relaxed);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_KEYDOWN if w.0 as u16 == VK_F11.0 => {
            toggle_full(h);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

fn client_size(h: HWND) -> (u32, u32) {
    let mut r = RECT::default();
    unsafe {
        let _ = GetClientRect(h, &mut r);
    }
    (((r.right - r.left).max(1)) as u32, ((r.bottom - r.top).max(1)) as u32)
}

#[derive(Default)]
struct Stats {
    shown: u32,
    e2e: Vec<i64>,
    decode: Vec<i64>,
    rtt: Vec<i64>,
}

fn pct(v: &mut [i64], q: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    v[((v.len() - 1) as f64 * q).round() as usize] as f64 / 10_000.0
}

pub fn run(arg: &str) {
    let mut parts = arg.split('|');
    let target = parts.next().unwrap_or("127.0.0.1");
    let out_file = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    let secs: Option<u64> = parts.next().and_then(|s| s.parse().ok());
    let host = if target.contains(':') { target.to_socket_addrs().ok().and_then(|mut a| a.next()) } else { (target, PORT).to_socket_addrs().ok().and_then(|mut a| a.next()) };
    let Some(host) = host else {
        eprintln!("endereço inválido: {target}");
        return;
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        let _ = timeBeginPeriod(1);
        if let Err(e) = watch(host, out_file.as_deref(), secs) {
            eprintln!("parou: {e:?}");
        }
        let _ = MFShutdown();
    }
}

unsafe fn watch(host: SocketAddr, out_file: Option<&str>, secs: Option<u64>) -> Result<()> {
    let d = d3d()?;
    if let Ok(mt) = d.ctx.cast::<ID3D11Multithread>() {
        let _ = mt.SetMultithreadProtected(true);
    }

    // Janela no monitor onde está o mouse, com 70% do tamanho dele.
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let _ = GetMonitorInfoW(mon, &mut mi);
    let m = mi.rcMonitor;
    let (mw, mh) = ((m.right - m.left) as u32, (m.bottom - m.top) as u32);
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<u32>().ok());
    // Para testar como outra tela: PRISMA_VIEW_H e PRISMA_VIEW_HZ.
    let screen_h = env("PRISMA_VIEW_H").unwrap_or(mh);
    let screen_w = if screen_h == mh { mw } else { screen_h * 16 / 9 };
    let hz = env("PRISMA_VIEW_HZ").unwrap_or_else(|| monitor_hz(mon));

    let inst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
    let class = w!("PrismaAssistir");
    let wc = WNDCLASSW {
        lpfnWndProc: Some(wndproc),
        hInstance: inst,
        lpszClassName: class,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        ..Default::default()
    };
    RegisterClassW(&wc);
    let (ww, wh) = ((mw * 7 / 10) as i32, (mh * 7 / 10) as i32);
    let hwnd = CreateWindowExW(
        WINDOW_EX_STYLE(0),
        class,
        w!("Prisma · conectando"),
        WS_OVERLAPPEDWINDOW | WS_VISIBLE,
        m.left + (mw as i32 - ww) / 2,
        m.top + (mh as i32 - wh) / 2,
        ww,
        wh,
        None,
        None,
        Some(inst),
        None,
    )?;

    // Cadeia de troca com tearing permitido: mostra o quadro assim que ele fica pronto.
    let factory: IDXGIFactory2 = d.dev.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()?;
    let mut allow = BOOL(0);
    let tearing = factory
        .cast::<IDXGIFactory5>()
        .and_then(|f5| f5.CheckFeatureSupport(DXGI_FEATURE_PRESENT_ALLOW_TEARING, &mut allow as *mut _ as *mut _, std::mem::size_of::<BOOL>() as u32))
        .is_ok()
        && allow.as_bool();
    let flags = if tearing { DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32 } else { 0 };
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: 0,
        Height: 0,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        Stereo: false.into(),
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        AlphaMode: DXGI_ALPHA_MODE_UNSPECIFIED,
        Flags: flags,
    };
    let swap = factory.CreateSwapChainForHwnd(&d.dev, hwnd, &desc, None, None)?;
    let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
    let present_flags = if tearing { DXGI_PRESENT_ALLOW_TEARING } else { DXGI_PRESENT(0) };

    let mut codecs = 0u8;
    for c in [Codec::H264, Codec::Hevc, Codec::Av1] {
        if can_decode(c) {
            codecs |= codec_bit(c);
        }
    }
    let mut hello = Vec::new();
    write(&mut hello, &Msg::Hello { w: screen_w, h: screen_h, hz, codecs });
    let sock = UdpSocket::bind(("0.0.0.0", 0)).map_err(|e| Error::new(E_FAIL, e.to_string()))?;
    let wake = CreateEventW(None, false, false, None)?;
    let sh = Arc::new(Shared {
        queue: Mutex::new(VecDeque::new()),
        wake: wake.0 as isize,
        quit: AtomicBool::new(false),
        got_info: AtomicBool::new(false),
        rtt: AtomicI64::new(0),
        offset: AtomicI64::new(0),
        lost: AtomicU32::new(0),
        bytes: AtomicU64::new(0),
    });
    let rx = {
        let (s2, sh2, hello2) = (sock.try_clone().map_err(|e| Error::new(E_FAIL, e.to_string()))?, sh.clone(), hello.clone());
        std::thread::Builder::new().name("assistir-rede".into()).spawn(move || receive(s2, host, hello2, sh2)).unwrap()
    };

    let mut dec: Option<Decoder> = None;
    let mut info: Option<(Codec, u32, u32, u32)> = None;
    let mut scaler: Option<Scaler> = None;
    let mut decoded: Vec<Decoded> = Vec::new();
    let start = Instant::now();
    let warm = Duration::from_secs(2);
    let mut total = Stats::default();
    let mut second = Stats::default();
    let mut tick = Instant::now();
    let mut bytes_mark = 0u64;
    let mut msg = MSG::default();

    'ui: loop {
        let _ = MsgWaitForMultipleObjects(Some(&[wake]), false, 100, QS_ALLINPUT);
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                break 'ui;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if secs.is_some_and(|s| start.elapsed() > Duration::from_secs(s)) {
            let _ = DestroyWindow(hwnd);
            continue;
        }
        if RESIZED.swap(false, Ordering::Relaxed) {
            scaler = None;
            let _ = swap.ResizeBuffers(0, 0, 0, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(flags as i32));
        }

        let events: Vec<Event> = sh.queue.lock().unwrap().drain(..).collect();
        let mut last: Option<Decoded> = None;
        for e in events {
            match e {
                Event::Info { codec, w, h, fps } => {
                    if info != Some((codec, w, h, fps)) {
                        info = Some((codec, w, h, fps));
                        scaler = None;
                        dec = match Decoder::new(&d.dev, codec, w, h, fps, None) {
                            Ok(x) => Some(x),
                            Err(e) => {
                                eprintln!("sem decodificador para {}: {e:?}", codec_name(codec));
                                None
                            }
                        };
                    }
                }
                Event::Frame(f) => {
                    let Some(dc) = &mut dec else { continue };
                    let t = now();
                    decoded.clear();
                    let p = Pkt { data: f.data, pts: f.pts, key: f.key };
                    if dc.decode(Some(&p), &mut decoded).is_err() {
                        continue;
                    }
                    second.decode.push(now() - t);
                    // Só o mais novo vai para a tela.
                    if let Some(x) = decoded.pop() {
                        last = Some(x);
                    }
                }
            }
        }
        if let (Some(f), Some((_, w, h, fps))) = (last, info) {
            if let Some((tex, slice)) = f.texture() {
                let (ow, oh) = client_size(hwnd);
                if scaler.as_ref().is_none_or(|s| s.size != (w, h, ow, oh)) {
                    scaler = Scaler::new(&d, w, h, ow, oh, fps).ok();
                }
                if let Some(s) = &scaler {
                    let bb: ID3D11Texture2D = swap.GetBuffer(0)?;
                    if s.blt(&tex, slice, &bb).is_ok() && swap.Present(0, present_flags).is_ok() {
                        let shown = now();
                        second.shown += 1;
                        second.e2e.push(shown + sh.offset.load(Ordering::Relaxed) - f.pts);
                    }
                }
            }
        }

        if tick.elapsed() >= Duration::from_secs(1) {
            let bytes = sh.bytes.load(Ordering::Relaxed);
            let mbps = (bytes - bytes_mark) as f64 * 8.0 / tick.elapsed().as_secs_f64() / 1e6;
            bytes_mark = bytes;
            let rtt = sh.rtt.load(Ordering::Relaxed);
            second.rtt.push(rtt);
            let fps_now = second.shown as f64 / tick.elapsed().as_secs_f64();
            let title = match info {
                Some((c, w, h, _)) => format!(
                    "Prisma · {}x{} {} · {:.0} q/s · {:.1} Mbit/s · rede {:.1} ms · ponta a ponta {:.1} ms · perdidos {}",
                    w,
                    h,
                    codec_name(c),
                    fps_now,
                    mbps,
                    rtt as f64 / 10_000.0,
                    pct(&mut second.e2e.clone(), 0.5),
                    sh.lost.load(Ordering::Relaxed)
                ),
                None => format!("Prisma · conectando a {host}"),
            };
            let _ = SetWindowTextW(hwnd, &HSTRING::from(title));
            if start.elapsed() > warm {
                total.shown += second.shown;
                total.e2e.append(&mut second.e2e);
                total.decode.append(&mut second.decode);
                total.rtt.append(&mut second.rtt);
            }
            second = Stats::default();
            tick = Instant::now();
        }
    }

    sh.quit.store(true, Ordering::Relaxed);
    let mut bye = Vec::new();
    write(&mut bye, &Msg::Bye);
    let _ = sock.send_to(&bye, host);
    let _ = rx.join();
    let _ = CloseHandle(wake);

    if let Some(path) = out_file {
        let secs = start.elapsed().saturating_sub(warm).as_secs_f64().max(0.001);
        let mut s = String::from("Assistir pela rede (fase 1)\n");
        if let Some((c, w, h, fps)) = info {
            s += &format!("vídeo: {w}x{h} a {fps} quadros/s, {}\n", codec_name(c));
        }
        s += &format!("tela daqui: {screen_w}x{screen_h} a {hz} Hz, tearing {}\n", if tearing { "permitido" } else { "não" });
        s += &format!("quadros mostrados: {:.1}/s, perdidos: {}\n", total.shown as f64 / secs, sh.lost.load(Ordering::Relaxed));
        s += &format!("recebido: {:.1} Mbit/s em média\n", sh.bytes.load(Ordering::Relaxed) as f64 * 8.0 / start.elapsed().as_secs_f64() / 1e6);
        s += &format!("rede (ida e volta): mediana {:.2} ms\n", pct(&mut total.rtt, 0.5));
        s += &format!("decodificação: mediana {:.2} ms, p95 {:.2} ms\n", pct(&mut total.decode, 0.5), pct(&mut total.decode, 0.95));
        s += &format!(
            "ponta a ponta (da tela do host até mostrar aqui): mediana {:.2} ms, p95 {:.2} ms, máx {:.2} ms\n",
            pct(&mut total.e2e, 0.5),
            pct(&mut total.e2e, 0.95),
            pct(&mut total.e2e, 1.0)
        );
        let _ = std::fs::write(path, s);
    }
    Ok(())
}
