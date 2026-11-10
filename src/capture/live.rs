//! Bancada da transmissão (fase 0 do compartilhamento de tela). Mede, quadro a quadro,
//! quanto cada etapa leva, da composição da tela até o quadro decodificado pronto na GPU,
//! com o codificador ajustado para transmitir. Só existe na versão de teste.
//!
//! `--test-stream=saida.txt|segundos|codecs|altura|mbps|fps|quadrado|captura`, com codecs `all` ou uma lista
//! como `h264,hevc,av1`. Um quadrado colorido pisca no canto da tela principal enquanto mede,
//! para a tela ter o que mandar; com `0` no sétimo campo ele não aparece e mede o que estiver na tela.
//! `PRISMA_POOL` (quadros na fila da captura) e `PRISMA_MININT` (intervalo mínimo, em 100 ns) servem
//! para testar a captura; nenhum dos dois mudou o teto de cerca de 64 quadros por segundo.
//! A captura é `wgc` (Windows.Graphics.Capture, a da gravação) ou `dd` (Desktop Duplication).
//! `PRISMA_ENC=mf` ou `nvenc` escolhe o codificador (o NVENC direto só com `--features stream`), e
//! `PRISMA_NO_WAKE=1` não mantém a tela acesa.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::*;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED};
use windows::Win32::System::Threading::*;
use windows::Win32::System::WinRT::Direct3D11::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::decoder::{Decoded, Decoder};
use super::engine::ConvOut;
use super::live_enc::{LiveEncoder, VideoFmt};
use super::engine::{bgra_texture, even, Prep};
use super::hdr::ToneMap;
use super::mux::{Codec, Pkt};
use super::{d3d, monitor_color, monitor_item, D3d};

/// Agora, na mesma régua do `SystemRelativeTime` da captura (100 ns do QPC).
fn now() -> i64 {
    unsafe {
        let (mut c, mut f) = (0i64, 0i64);
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
        (c as i128 * 10_000_000 / f.max(1) as i128) as i64
    }
}

fn label(c: Codec) -> &'static str {
    match c {
        Codec::Av1 => "AV1",
        Codec::Hevc => "HEVC",
        Codec::H264 => "H.264",
    }
}

/// Estado da tela que o Windows avisa: -1 sem aviso, 0 apagada, 1 acesa, 2 escurecida.
static DISPLAY: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    use windows::Win32::System::Power::POWERBROADCAST_SETTING;
    use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
    if m == WM_POWERBROADCAST && w.0 as u32 == PBT_POWERSETTINGCHANGE && l.0 != 0 {
        let ps = &*(l.0 as *const POWERBROADCAST_SETTING);
        if ps.PowerSetting == GUID_CONSOLE_DISPLAY_STATE {
            DISPLAY.store(ps.Data[0] as i32, Ordering::Relaxed);
        }
        return LRESULT(1);
    }
    DefWindowProcW(h, m, w, l)
}

/// Quadrado que muda de cor a cada composição da tela, para a captura ter quadros novos.
/// Devolve quantas vezes por segundo o quadrado mudou.
fn animate(stop: Arc<AtomicBool>, x: i32, y: i32) -> std::thread::JoinHandle<f64> {
    std::thread::spawn(move || unsafe {
        let inst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let class = w!("PrismaBancadaTransmissao");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst, lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Prisma"),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            160,
            160,
            None,
            None,
            Some(inst),
            None,
        ) else {
            return 0.0;
        };
        // Ao registrar, o Windows já manda o estado atual da tela.
        let _ = windows::Win32::System::Power::RegisterPowerSettingNotification(
            HANDLE(hwnd.0),
            &windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE,
            DEVICE_NOTIFY_WINDOW_HANDLE,
        );
        let began = Instant::now();
        let mut i = 0u32;
        let mut msg = MSG::default();
        while !stop.load(Ordering::Relaxed) {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let dc = GetDC(Some(hwnd));
            let c = i.wrapping_mul(37) % 256;
            let br = CreateSolidBrush(COLORREF(c | ((255 - c) << 8) | ((i.wrapping_mul(11) % 256) << 16)));
            FillRect(dc, &RECT { left: 0, top: 0, right: 160, bottom: 160 }, br);
            let _ = DeleteObject(br.into());
            ReleaseDC(Some(hwnd), dc);
            let _ = DwmFlush();
            i = i.wrapping_add(1);
        }
        let _ = DestroyWindow(hwnd);
        i as f64 / began.elapsed().as_secs_f64()
    })
}

#[derive(Default, Clone, Copy)]
struct Times {
    comp: i64,
    got: i64,
    fed: i64,
    enc: i64,
    dec: i64,
    gpu: i64,
    size: usize,
    key: bool,
}

fn stat(name: &str, mut v: Vec<i64>) -> String {
    if v.is_empty() {
        return format!("  {name:<16} sem medida\n");
    }
    v.sort_unstable();
    let ms = |x: i64| x as f64 / 10_000.0;
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    format!(
        "  {name:<16} mediana {:>6.2} ms · p95 {:>6.2} ms · máx {:>6.2} ms · mín {:>6.2} ms\n",
        ms(at(0.5)),
        ms(at(0.95)),
        ms(*v.last().unwrap()),
        ms(v[0])
    )
}

struct Run {
    secs: u32,
    out_h: u32,
    mbps: u32,
    fps: u32,
    dd: bool,
    fmt: VideoFmt,
}

/// De onde vêm os quadros da tela.
enum Source {
    Wgc { pool: Direct3D11CaptureFramePool, session: GraphicsCaptureSession, event: HANDLE, token: i64 },
    Dd { dup: IDXGIOutputDuplication, merged: u32, mouse_only: u32 },
}

enum Held {
    Wgc(Direct3D11CaptureFrame),
    Dd,
}

fn qpc_100ns(c: i64) -> i64 {
    let mut f = 0i64;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut f);
    }
    (c as i128 * 10_000_000 / f.max(1) as i128) as i64
}

/// Desktop Duplication da saída de vídeo onde está o monitor, no mesmo formato que a tela usa.
unsafe fn duplicate(d: &D3d, mon: HMONITOR, hdr: bool) -> Result<IDXGIOutputDuplication> {
    let ad = d.dev.cast::<IDXGIDevice>()?.GetAdapter()?;
    let mut i = 0;
    loop {
        let o = ad.EnumOutputs(i)?;
        if o.GetDesc()?.Monitor == mon {
            let fmts = [if hdr { DXGI_FORMAT_R16G16B16A16_FLOAT } else { DXGI_FORMAT_B8G8R8A8_UNORM }];
            if let Ok(o5) = o.cast::<IDXGIOutput5>() {
                return o5.DuplicateOutput1(&d.dev, 0, &fmts);
            }
            return o.cast::<IDXGIOutput1>()?.DuplicateOutput(&d.dev);
        }
        i += 1;
    }
}

impl Source {
    /// Espera até 1 ms e entrega os quadros novos: textura, hora do quadro e hora em que chegou.
    unsafe fn grab(&mut self, out: &mut Vec<(ID3D11Texture2D, i64, i64, Held)>) -> Result<()> {
        match self {
            Source::Wgc { pool, event, .. } => {
                let _ = WaitForSingleObject(*event, 1);
                while let Ok(f) = pool.TryGetNextFrame() {
                    let got = now();
                    let pts = f.SystemRelativeTime()?.Duration;
                    let access: IDirect3DDxgiInterfaceAccess = f.Surface()?.cast()?;
                    out.push((access.GetInterface()?, pts, got, Held::Wgc(f)));
                }
            }
            Source::Dd { dup, merged, mouse_only } => {
                let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
                let mut res = None;
                match dup.AcquireNextFrame(1, &mut info, &mut res) {
                    Ok(()) => {}
                    Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(()),
                    Err(e) => return Err(e),
                }
                let got = now();
                // Só o mouse mexeu: a imagem é a mesma.
                if info.LastPresentTime == 0 {
                    *mouse_only += 1;
                    dup.ReleaseFrame()?;
                    return Ok(());
                }
                *merged += info.AccumulatedFrames.saturating_sub(1);
                let tex: ID3D11Texture2D = res.unwrap().cast()?;
                out.push((tex, qpc_100ns(info.LastPresentTime), got, Held::Dd));
            }
        }
        Ok(())
    }

    unsafe fn release(&self, h: Held) {
        match (self, h) {
            (_, Held::Wgc(f)) => {
                let _ = f.Close();
            }
            (Source::Dd { dup, .. }, Held::Dd) => {
                let _ = dup.ReleaseFrame();
            }
            _ => {}
        }
    }

    fn stop(&self) {
        if let Source::Wgc { session, .. } = self {
            let _ = session.Close();
        }
    }

    unsafe fn finish(self) {
        if let Source::Wgc { pool, event, token, .. } = self {
            let _ = pool.RemoveFrameArrived(token);
            let _ = pool.Close();
            let _ = CloseHandle(event);
        }
    }
}

unsafe fn one(d: &D3d, mon: HMONITOR, codec: Codec, r: &Run) -> Result<String> {
    let item = monitor_item(mon)?;
    let color = monitor_color(mon);
    let size = item.Size()?;
    let (src_w, src_h) = (even(size.Width as u32), even(size.Height as u32));
    let out_h = if r.out_h == 0 || r.out_h >= src_h { src_h } else { even(r.out_h) };
    let out_w = even((src_w as u64 * out_h as u64 / src_h as u64) as u32);
    let bits = r.mbps * 1_000_000;

    let mut src = if r.dd {
        Source::Dd { dup: duplicate(d, mon, color.hdr)?, merged: 0, mouse_only: 0 }
    } else {
        let fmt = if color.hdr { DirectXPixelFormat::R16G16B16A16Float } else { DirectXPixelFormat::B8G8R8A8UIntNormalized };
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d.rt, fmt, std::env::var("PRISMA_POOL").ok().and_then(|v| v.parse().ok()).unwrap_or(2), size)?;
        let event = CreateEventW(None, false, false, None)?;
        let ev = event.0 as isize;
        let token = pool.FrameArrived(&TypedEventHandler::new(move |_, _| {
            let _ = SetEvent(HANDLE(ev as *mut _));
            Ok(())
        }))?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);
        let min = std::env::var("PRISMA_MININT").ok().and_then(|v| v.parse().ok()).unwrap_or(10_000_000 / r.fps as i64);
        let _ = session.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: min });
        session.StartCapture()?;
        Source::Wgc { pool, session, event, token }
    };

    let hdr10 = r.fmt.hdr10;
    let mid = bgra_texture(&d.dev, src_w, src_h, if hdr10 { DXGI_FORMAT_R10G10B10A2_UNORM } else { DXGI_FORMAT_B8G8R8A8_UNORM })?;
    // Tela em HDR vira SDR antes de codificar (a opção A do estudo). Com HDR10 pedido, sai PQ; numa
    // tela SDR as cores ficam erradas, mas o caminho de 10 bits inteiro é exercitado.
    let tone = if color.hdr || hdr10 { Some(ToneMap::new(&d.dev, &mid, hdr10, color.white_nits)?) } else { None };
    let out_fmt = if r.fmt.yuv444 {
        ConvOut::Ayuv
    } else if hdr10 {
        ConvOut::P010
    } else {
        ConvOut::Nv12
    };
    let mut conv = Prep::new(&d.dev, &d.ctx, &mid, src_w, src_h, out_w, out_h, r.fps, out_fmt)?;
    let mut enc = LiveEncoder::open(&d.dev, codec, out_w, out_h, r.fps, bits, r.fmt)?;
    let mut dec_err = None;
    // O 4:4:4 vai para o NVDEC (o Media Foundation não decodifica 4:4:4).
    let mut dec = if r.fmt.yuv444 {
        None
    } else {
        match Decoder::new(&d.dev, codec, out_w, out_h, r.fps, enc.header().as_deref(), r.fmt.hdr10) {
            Ok(x) => Some(x),
            Err(e) => {
                dec_err = Some(format!("{e:?}"));
                None
            }
        }
    };
    #[cfg(feature = "stream")]
    let mut nv = if r.fmt.yuv444 && codec == Codec::Hevc {
        match super::nvdec::Nvdec::new(&d.dev, out_w, out_h) {
            Ok(x) => Some(x),
            Err(e) => {
                dec_err = Some(format!("{e:?}"));
                None
            }
        }
    } else {
        None
    };
    if r.fmt.yuv444 && codec != Codec::Hevc {
        dec_err = Some("o 4:4:4 só se decodifica em HEVC (NVDEC)".into());
    }
    let (dec_name, dec_gpu) = match &dec {
        Some(x) => (x.name.clone(), x.gpu),
        #[cfg(feature = "stream")]
        None if nv.is_some() => (
            format!("NVDEC 4:4:4, {}", if nv.as_ref().is_some_and(|n| n.direct) { "cópia direta na placa" } else { "cópia pela memória" }),
            true,
        ),
        None => Default::default(),
    };
    let mut q = None;
    d.dev.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut q))?;
    let q = q.unwrap();

    let mut times: HashMap<i64, Times> = HashMap::new();
    let mut pkts: Vec<Pkt> = Vec::new();
    let mut decoded: Vec<Decoded> = Vec::new();
    let (mut frames, mut dropped, mut skipped) = (0u32, 0u32, 0u32);
    // Perda simulada: `PRISMA_DROP=N` joga fora um pacote a cada N, e o codificador só fica sabendo
    // `PRISMA_DROP_LATE` pacotes depois (como a ida e volta da rede), invalidando os que dependiam dele.
    let drop_every: u32 = std::env::var("PRISMA_DROP").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let drop_late: usize = std::env::var("PRISMA_DROP_LATE").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    let (mut pkt_n, mut lost_n, mut keys_asked, mut dec_errors) = (0u32, 0u32, 0u32, 0u32);
    // Hora dos pacotes perdidos ainda não avisados, com os que vieram depois deles.
    let mut since_loss: Option<Vec<i64>> = None;
    let interval = 10_000_000i64 / r.fps as i64;
    let mut last_fed = i64::MIN / 2;
    let mut got_frames: Vec<(ID3D11Texture2D, i64, i64, Held)> = Vec::new();
    // Quadro mais novo já copiado, esperando o codificador: hora do quadro e hora em que chegou.
    let mut pending: Option<(i64, i64)> = None;
    let start = Instant::now();
    // O primeiro segundo aquece e não conta.
    let count_from = now() + 10_000_000;
    let total = Duration::from_secs(r.secs as u64 + 1);
    let mut tail = None;

    loop {
        if tail.is_none() {
            src.grab(&mut got_frames)?;
            for (tex, pts, got, held) in got_frames.drain(..) {
                if pts - last_fed < interval * 9 / 10 {
                    skipped += 1;
                    src.release(held);
                    continue;
                }
                if let Some(t) = &tone {
                    t.draw(&d.ctx, &d.dev, &tex, src_w, src_h)?;
                } else {
                    let bx = D3D11_BOX { left: 0, top: 0, front: 0, right: src_w, bottom: src_h, back: 1 };
                    d.ctx.CopySubresourceRegion(&mid, 0, 0, 0, 0, &tex, 0, Some(&bx));
                }
                src.release(held);
                // Um quadro que ainda esperava o codificador dá lugar ao mais novo.
                if pending.is_some() {
                    dropped += 1;
                }
                pending = Some((pts, got));
            }
            if start.elapsed() >= total {
                src.stop();
                tail = Some(Instant::now());
            }
        } else if tail.is_some_and(|t| t.elapsed() > Duration::from_millis(300)) {
            break;
        }
        enc.pump(&mut pkts)?;
        if enc.ready() {
            if let Some((pts, got)) = pending.take() {
                let t = conv.convert(&d.ctx)?;
                // A hora vem antes de entregar: o NVENC codifica dentro da chamada (e espera a GPU
                // terminar a conversão), então esse tempo é da codificação, não do preparo.
                let fed = now();
                enc.feed(&t, pts)?;
                last_fed = pts;
                if pts >= count_from {
                    frames += 1;
                    times.insert(pts, Times { comp: pts, got, fed, ..Default::default() });
                }
            }
        }
        let t_enc = now();
        for p in pkts.drain(..) {
            pkt_n += 1;
            if let Some(v) = &mut since_loss {
                v.push(p.pts);
                if v.len() > drop_late {
                    // O aviso "chegou" ao host: invalida o perdido e os que vieram depois.
                    let ok = v.iter().fold(false, |a, &t| enc.invalidate(t) || a);
                    if !ok {
                        enc.force_key();
                        keys_asked += 1;
                    }
                    since_loss = None;
                }
            }
            if drop_every > 0 && pkt_n % drop_every == 0 && !p.key && since_loss.is_none() && pkt_n > 10 {
                lost_n += 1;
                since_loss = Some(vec![p.pts]);
                times.remove(&p.pts);
                continue;
            }
            if let Some(t) = times.get_mut(&p.pts) {
                t.enc = t_enc;
                t.size = p.data.len();
                t.key = p.key;
            }
            if let Some(dc) = &mut dec {
                decoded.clear();
                if let Err(e) = dc.decode(Some(&p), &mut decoded) {
                    if drop_every > 0 {
                        // Com perda simulada, um erro pede quadro-chave em vez de parar.
                        dec_errors += 1;
                        enc.force_key();
                        keys_asked += 1;
                        continue;
                    }
                    dec_err = Some(format!("{e:?}"));
                    dec = None;
                    continue;
                }
                mark(d, &q, &decoded.iter().map(|f| f.pts).collect::<Vec<_>>(), &mut times);
            }
            #[cfg(feature = "stream")]
            if let Some(n) = &mut nv {
                match n.decode(&d.ctx, &p.data, p.pts) {
                    Ok(true) => mark(d, &q, &[p.pts], &mut times),
                    Ok(false) => {}
                    Err(_) if drop_every > 0 => {
                        dec_errors += 1;
                        enc.force_key();
                        keys_asked += 1;
                    }
                    Err(e) => {
                        dec_err = Some(format!("{e:?}"));
                        nv = None;
                    }
                }
            }
        }
        if let Some(dc) = &mut dec {
            decoded.clear();
            if let Err(e) = dc.decode(None, &mut decoded) {
                dec_err = Some(format!("{e:?}"));
                dec = None;
            } else {
                mark(d, &q, &decoded.iter().map(|f| f.pts).collect::<Vec<_>>(), &mut times);
            }
        }
    }
    // 4:4:4: confere a cor que voltou do NVDEC contra a da tela, em pontos parados da tela.
    #[cfg(feature = "stream")]
    let color_note = match &nv {
        Some(_) if (out_w, out_h) != (src_w, src_h) => "  cor: só se confere sem mudar o tamanho\n".to_string(),
        Some(n) => probe_444(d, &mid, n, out_w, out_h).unwrap_or_else(|e| format!("  cor: não deu para conferir ({e:?})\n")),
        None => String::new(),
    };
    #[cfg(not(feature = "stream"))]
    let color_note = String::new();
    let dd_note = match &src {
        Source::Dd { merged, mouse_only, .. } => {
            format!("  captura: Desktop Duplication, {merged} quadros juntados em outro, {mouse_only} só com o mouse\n")
        }
        Source::Wgc { .. } => "  captura: Windows.Graphics.Capture\n".to_string(),
    };
    src.finish();

    let mut v: Vec<Times> = times.into_values().filter(|t| t.enc > 0).collect();
    v.sort_by_key(|t| t.comp);
    let n = v.len();
    let bytes: usize = v.iter().map(|t| t.size).sum();
    let keys = v.iter().filter(|t| t.key).count();
    let biggest = v.iter().map(|t| t.size).max().unwrap_or(0);
    let decoded_n = v.iter().filter(|t| t.dec > 0).count();
    let mut s = String::new();
    s += &format!("== {} ==\n", label(codec));
    s += &dd_note;
    s += &color_note;
    s += &format!("  codificador: {}\n", enc.name());
    if !enc.refused().is_empty() {
        s += &format!("  ajustes recusados: {}\n", enc.refused().join(", "));
    }
    match &dec_err {
        Some(e) if dec_name.is_empty() => s += &format!("  decodificador: nenhum ({e})\n"),
        _ => s += &format!("  decodificador: {} ({})\n", dec_name, if dec_gpu { "GPU" } else { "processador" }),
    }
    if let (Some(e), false) = (&dec_err, dec_name.is_empty()) {
        s += &format!("  erro ao decodificar: {e}\n");
    }
    s += &format!(
        "  {}x{} de {}x{}, tela {}, {} Mbit/s pedidos, {} quadros/s pedidos\n",
        out_w,
        out_h,
        src_w,
        src_h,
        if color.hdr { "em HDR (convertida para SDR)" } else { "em SDR" },
        r.mbps,
        r.fps
    );
    let secs = r.secs as f64;
    s += &format!(
        "  quadros: {frames} capturados ({:.1}/s), {n} codificados, {decoded_n} decodificados, {dropped} trocados por um mais novo com o codificador ocupado, {skipped} pulados por chegarem cedo\n",
        frames as f64 / secs
    );
    s += &format!(
        "  pacotes: média {:.1} KB, maior {:.1} KB, {keys} quadros-chave, {:.1} Mbit/s de fato\n",
        if n > 0 { bytes as f64 / n as f64 / 1024.0 } else { 0.0 },
        biggest as f64 / 1024.0,
        bytes as f64 * 8.0 / secs / 1e6
    );
    if drop_every > 0 {
        s += &format!(
            "  perda simulada: {lost_n} pacotes jogados fora (1 a cada {drop_every}), aviso {drop_late} pacotes depois; {} quadros-chave pedidos, {dec_errors} erros do decodificador
",
            keys_asked
        );
    }
    s += &stat("captura", v.iter().map(|t| t.got - t.comp).collect());
    s += &stat("preparo", v.iter().map(|t| t.fed - t.got).collect());
    s += &stat("codificação", v.iter().map(|t| t.enc - t.fed).collect());
    s += &stat("decodificação", v.iter().filter(|t| t.dec > 0).map(|t| t.dec - t.enc).collect());
    s += &stat("GPU pronta", v.iter().filter(|t| t.gpu > 0).map(|t| t.gpu - t.dec).collect());
    s += &stat("até codificado", v.iter().map(|t| t.enc - t.comp).collect());
    s += &stat("total", v.iter().filter(|t| t.gpu > 0).map(|t| t.gpu - t.comp).collect());
    Ok(s)
}

/// Marca os quadros que saíram do decodificador e espera a GPU terminá-los.
/// Cor da tela (RGB) em alguns pontos, o YUV que o BT.709 e o BT.601 dariam e o que o NVDEC devolveu.
#[cfg(feature = "stream")]
unsafe fn probe_444(d: &D3d, mid: &ID3D11Texture2D, n: &super::nvdec::Nvdec, w: u32, h: u32) -> Result<String> {
    let td = D3D11_TEXTURE2D_DESC {
        Width: 1,
        Height: 1,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut st = None;
    d.dev.CreateTexture2D(&td, None, Some(&mut st))?;
    let st = st.unwrap();
    let yuv = |r: f64, g: f64, b: f64, kr: f64, kb: f64| {
        let y = kr * r + (1.0 - kr - kb) * g + kb * b;
        let u = (b - y) / (2.0 * (1.0 - kb));
        let v = (r - y) / (2.0 * (1.0 - kr));
        [(16.0 + 219.0 * y).round() as i32, (128.0 + 224.0 * u).round() as i32, (128.0 + 224.0 * v).round() as i32]
    };
    let mut s = String::new();
    let (mut e709, mut e601) = (0i32, 0i32);
    for (fx, fy) in [(1, 1), (3, 1), (1, 3), (3, 3), (2, 2), (5, 7), (7, 5)] {
        let (x, y) = (w * fx / 8, h * fy / 8);
        let bx = D3D11_BOX { left: x, top: y, front: 0, right: x + 1, bottom: y + 1, back: 1 };
        d.ctx.CopySubresourceRegion(&st, 0, 0, 0, 0, mid, 0, Some(&bx));
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        d.ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let px = std::slice::from_raw_parts(m.pData as *const u8, 4);
        let (b, g, r) = (px[0] as f64 / 255.0, px[1] as f64 / 255.0, px[2] as f64 / 255.0);
        d.ctx.Unmap(&st, 0);
        let got = n.yuv_at(&d.dev, &d.ctx, x, y)?;
        let (a, c) = (yuv(r, g, b, 0.2126, 0.0722), yuv(r, g, b, 0.299, 0.114));
        let got = got.map(|v| v as i32);
        e709 += (0..3).map(|i| (a[i] - got[i]).abs()).sum::<i32>();
        e601 += (0..3).map(|i| (c[i] - got[i]).abs()).sum::<i32>();
        s += &format!(
            "  cor em {x},{y}: RGB {:.0},{:.0},{:.0} · BT.709 daria {:?} · BT.601 daria {:?} · NVDEC deu {:?}\n",
            r * 255.0,
            g * 255.0,
            b * 255.0,
            a,
            c,
            got
        );
    }
    s += &format!("  cor: diferença somada {e709} contra o BT.709 e {e601} contra o BT.601\n");
    Ok(s)
}

unsafe fn mark(d: &D3d, q: &ID3D11Query, decoded: &[i64], times: &mut HashMap<i64, Times>) {
    if decoded.is_empty() {
        return;
    }
    let t_dec = now();
    d.ctx.End(q);
    let mut done = BOOL(0);
    let limit = Instant::now();
    // S_FALSE também chega como Ok: quem diz se terminou é o valor.
    while !(d.ctx.GetData(q, Some(&mut done as *mut _ as *mut _), std::mem::size_of::<BOOL>() as u32, 0).is_ok() && done.as_bool())
        && limit.elapsed() < Duration::from_millis(100)
    {
        std::hint::spin_loop();
    }
    let t_gpu = now();
    for pts in decoded {
        if let Some(t) = times.get_mut(pts) {
            t.dec = t_dec;
            t.gpu = t_gpu;
        }
    }
}

pub fn bench(arg: &str) {
    let mut p = arg.split('|');
    let out = p.next().unwrap_or("transmissao.txt").to_string();
    let secs = p.next().and_then(|v| v.parse().ok()).unwrap_or(8u32).max(2);
    let codecs: Vec<Codec> = match p.next().unwrap_or("all") {
        "all" | "" => vec![Codec::H264, Codec::Hevc, Codec::Av1],
        l => l
            .split(',')
            .filter_map(|c| match c.trim() {
                "h264" => Some(Codec::H264),
                "hevc" => Some(Codec::Hevc),
                "av1" => Some(Codec::Av1),
                _ => None,
            })
            .collect(),
    };
    let out_h = p.next().and_then(|v| v.parse().ok()).unwrap_or(0u32);
    let mbps = p.next().and_then(|v| v.parse().ok()).unwrap_or(30u32);
    let fps = p.next().and_then(|v| v.parse().ok()).unwrap_or(120u32).clamp(10, 240);
    // Sem o quadrado, mede o que estiver na tela, como um jogo ou um vídeo.
    let square = p.next() != Some("0");
    let dd = p.next() == Some("dd");
    // `PRISMA_FMT=hdr10` ou `444` mede o vídeo de 10 bits ou o 4:4:4 (só no NVENC direto).
    let fmt = match std::env::var("PRISMA_FMT").as_deref() {
        Ok("hdr10") => VideoFmt { hdr10: true, yuv444: false },
        Ok("444") => VideoFmt { hdr10: false, yuv444: true },
        _ => VideoFmt::default(),
    };
    let run = Run { secs, out_h, mbps, fps, dd, fmt };

    let mut s = String::from("Bancada da transmissão (fase 0)\n");
    s += "Etapas: captura = do momento em que a tela do PC mostra o quadro (o relógio da captura) até ele chegar ao Prisma, e negativo quer dizer que chegou antes de aparecer; preparo = cópia, conversão de cor e escala na GPU até entregar ao codificador, contando a espera pelo codificador; codificação = até o pacote sair (no NVENC direto, que codifica dentro da chamada, conta também a GPU terminando a conversão; compare os codificadores por \"até codificado\"); decodificação = até o quadro sair do decodificador; GPU pronta = até a GPU terminar o quadro decodificado; total = do quadro na tela do PC até o quadro decodificado pronto, sem a rede e sem mostrar na outra tela.\n";
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        let _ = timeBeginPeriod(1);
        // Tela apagada por ociosidade muda tudo: a composição desacelera e a Desktop Duplication
        // não entrega nada. Mantém a tela acesa enquanto mede e dá tempo de ela acordar.
        // `PRISMA_NO_WAKE=1` não mexe na tela (para medir só a codificação sem acender a tela de ninguém).
        let wake = std::env::var("PRISMA_NO_WAKE").as_deref() != Ok("1");
        if wake {
            let _ = SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED);
        }
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let mut ex = MONITORINFOEXW::default();
        ex.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let _ = GetMonitorInfoW(mon, &mut ex as *mut _ as *mut MONITORINFO);
        let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        if EnumDisplaySettingsW(PCWSTR(ex.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool() {
            s += &format!("Tela principal: {}x{} a {} Hz\n", dm.dmPelsWidth, dm.dmPelsHeight, dm.dmDisplayFrequency);
        }
        s += "\n";
        let stop = Arc::new(AtomicBool::new(false));
        let anim = square.then(|| animate(stop.clone(), mi.rcMonitor.left + 40, mi.rcMonitor.top + 40));
        // Dá tempo de o aviso do estado da tela chegar.
        std::thread::sleep(Duration::from_millis(500));
        match DISPLAY.load(Ordering::Relaxed) {
            0 => s += "ATENÇÃO: a tela está apagada. A captura sai de um relógio interno, então os números de captura não valem; os de codificação e decodificação valem.\n\n",
            1 => s += "Tela acesa.\n\n",
            2 => s += "Tela escurecida pela economia de energia.\n\n",
            _ => {}
        }
        match d3d() {
            Ok(d) => {
                if let Ok(mt) = d.ctx.cast::<ID3D11Multithread>() {
                    let _ = mt.SetMultithreadProtected(true);
                }
                // Placa de vídeo: o Media Foundation usa o que ela tiver, NVIDIA, AMD ou Intel.
                if let Some(desc) = d
                    .dev
                    .cast::<windows::Win32::Graphics::Dxgi::IDXGIDevice>()
                    .and_then(|x| x.GetAdapter())
                    .and_then(|a| a.GetDesc())
                    .ok()
                {
                    let n = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
                    s = s.replacen("\n\n", &format!("\nPlaca de vídeo: {}\n\n", String::from_utf16_lossy(&desc.Description[..n])), 1);
                }
                for c in codecs {
                    match one(&d, mon, c, &run) {
                        Ok(r) => s += &r,
                        Err(e) => s += &format!("== {} ==\n  falhou: {e:?}\n", label(c)),
                    }
                    s += "\n";
                    let _ = std::fs::write(&out, &s);
                }
            }
            Err(e) => s += &format!("sem Direct3D: {e:?}\n"),
        }
        stop.store(true, Ordering::Relaxed);
        if let Some(a) = anim {
            s += &format!("O quadrado de teste mudou {:.1} vezes por segundo.\n", a.join().unwrap_or(0.0));
        }
        let _ = SetThreadExecutionState(ES_CONTINUOUS);
        let _ = timeEndPeriod(1);
        let _ = MFShutdown();
    }
    let _ = std::fs::write(&out, s);
}
