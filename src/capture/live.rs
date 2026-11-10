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

use std::collections::{HashMap, VecDeque};
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

use super::encoder::{friendly, Encoder};
use super::engine::{bgra_texture, even, Converter};
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

fn subtype(c: Codec) -> GUID {
    match c {
        Codec::Av1 => MFVideoFormat_AV1,
        Codec::Hevc => MFVideoFormat_HEVC,
        Codec::H264 => MFVideoFormat_H264,
    }
}

fn label(c: Codec) -> &'static str {
    match c {
        Codec::Av1 => "AV1",
        Codec::Hevc => "HEVC",
        Codec::H264 => "H.264",
    }
}

fn packed(a: u32, b: u32) -> u64 {
    (a as u64) << 32 | b as u64
}

/// Decodificador do Media Foundation, na GPU quando ele aceita o Direct3D 11.
struct Decoder {
    mft: IMFTransform,
    events: Option<IMFMediaEventGenerator>,
    need: u32,
    queue: VecDeque<Pkt>,
    provides: bool,
    size: u32,
    fps: u32,
    name: String,
    gpu: bool,
}

impl Decoder {
    unsafe fn new(dev: &ID3D11Device, codec: Codec, w: u32, h: u32, fps: u32, header: Option<&[u8]>) -> Result<Self> {
        let i = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: subtype(codec) };
        let o = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_NV12 };
        let mut list = std::ptr::null_mut();
        let mut n = 0u32;
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG_SYNCMFT | MFT_ENUM_FLAG_ASYNCMFT | MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
            Some(&i),
            Some(&o),
            &mut list,
            &mut n,
        )?;
        if n == 0 || list.is_null() {
            return Err(Error::new(E_FAIL, "sem decodificador"));
        }
        let items = std::slice::from_raw_parts(list, n as usize);
        let act = items[0].as_ref().unwrap();
        let name = friendly(act);
        let mft = act.ActivateObject::<IMFTransform>();
        CoTaskMemFree(Some(list as *const _));
        let mft = mft?;

        let attrs = mft.GetAttributes()?;
        let is_async = attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0;
        if is_async {
            attrs.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
        }
        let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
        let mut gpu = false;
        if attrs.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) != 0 {
            let mut token = 0;
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
            let manager = manager.unwrap();
            manager.ResetDevice(dev, token)?;
            gpu = mft.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize).is_ok();
        }

        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &subtype(codec))?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, packed(w, h))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, packed(fps, 1))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, packed(1, 1))?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        if let Some(hd) = header {
            let _ = t.SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, hd);
        }
        mft.SetInputType(0, &t, 0)?;
        let events = if is_async { Some(mft.cast::<IMFMediaEventGenerator>()?) } else { None };
        let mut s = Self { mft, events, need: 0, queue: VecDeque::new(), provides: false, size: 0, fps, name, gpu };
        // Alguns só dizem a saída depois do primeiro quadro; aí ela vem na troca de formato.
        let _ = s.pick_output();
        s.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        s.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
        Ok(s)
    }

    unsafe fn pick_output(&mut self) -> Result<()> {
        let mut i = 0;
        loop {
            let t = self.mft.GetOutputAvailableType(0, i)?;
            if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                self.mft.SetOutputType(0, &t, 0)?;
                break;
            }
            i += 1;
        }
        let info = self.mft.GetOutputStreamInfo(0)?;
        let own = (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0) as u32;
        self.provides = info.dwFlags & own != 0;
        self.size = info.cbSize;
        Ok(())
    }

    unsafe fn sample(&self, p: &Pkt) -> Result<IMFSample> {
        let len = p.data.len() as u32;
        let b = MFCreateMemoryBuffer(len)?;
        let mut ptr = std::ptr::null_mut();
        b.Lock(&mut ptr, None, None)?;
        std::ptr::copy_nonoverlapping(p.data.as_ptr(), ptr, p.data.len());
        b.Unlock()?;
        b.SetCurrentLength(len)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&b)?;
        s.SetSampleTime(p.pts)?;
        s.SetSampleDuration(10_000_000 / self.fps as i64)?;
        if p.key {
            s.SetUINT32(&MFSampleExtension_CleanPoint, 1)?;
        }
        Ok(s)
    }

    /// Tira um quadro pronto. Devolve falso quando o decodificador quer mais entrada.
    unsafe fn pull(&mut self, out: &mut Vec<i64>) -> Result<bool> {
        let mut buf = [MFT_OUTPUT_DATA_BUFFER::default()];
        if !self.provides {
            let s = MFCreateSample()?;
            s.AddBuffer(&MFCreateMemoryBuffer(self.size.max(1))?)?;
            buf[0].pSample = std::mem::ManuallyDrop::new(Some(s));
        }
        let mut status = 0;
        let r = self.mft.ProcessOutput(0, &mut buf, &mut status);
        let sample = std::mem::ManuallyDrop::take(&mut buf[0].pSample);
        let _ = std::mem::ManuallyDrop::into_inner(std::mem::replace(&mut buf[0].pEvents, std::mem::ManuallyDrop::new(None)));
        match r {
            Ok(()) => {
                if let Some(s) = sample {
                    out.push(s.GetSampleTime()?);
                }
                Ok(true)
            }
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(false),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                self.pick_output()?;
                Ok(true)
            }
            Err(e) => Err(e),
        }
    }

    /// Entrega um pacote (ou só trata o que estiver pendente) e diz quais quadros saíram.
    unsafe fn decode(&mut self, p: Option<&Pkt>, out: &mut Vec<i64>) -> Result<()> {
        if self.events.is_none() {
            if let Some(p) = p {
                let s = self.sample(p)?;
                let mut tries = 0;
                loop {
                    match self.mft.ProcessInput(0, &s, 0) {
                        Ok(()) => break,
                        Err(e) if e.code() == MF_E_NOTACCEPTING && tries < 4 => {
                            tries += 1;
                            while self.pull(out)? {}
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            while self.pull(out)? {}
            return Ok(());
        }
        if let Some(p) = p {
            self.queue.push_back(p.clone());
        }
        loop {
            while self.need > 0 {
                let Some(p) = self.queue.pop_front() else { break };
                let s = self.sample(&p)?;
                self.mft.ProcessInput(0, &s, 0)?;
                self.need -= 1;
            }
            let ev = match self.events.as_ref().unwrap().GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                Ok(e) => e,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
                Err(e) => return Err(e),
            };
            match ev.GetType()? {
                t if t == METransformNeedInput.0 as u32 => self.need += 1,
                t if t == METransformHaveOutput.0 as u32 => {
                    self.pull(out)?;
                }
                _ => {}
            }
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if let Ok(s) = self.mft.cast::<IMFShutdown>() {
                let _ = s.Shutdown();
            }
        }
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

    let mid = bgra_texture(&d.dev, src_w, src_h, DXGI_FORMAT_B8G8R8A8_UNORM)?;
    // Tela em HDR vira SDR antes de codificar (a opção A do estudo).
    let tone = if color.hdr { Some(ToneMap::new(&d.dev, &mid, false, color.white_nits)?) } else { None };
    let mut conv = Converter::new(&d.dev, &d.ctx, src_w, src_h, out_w, out_h, r.fps, false)?;
    let in_view = conv.input_view(&mid)?;
    let mut enc = Encoder::live(&d.dev, codec, out_w, out_h, r.fps, bits)?;
    let mut dec_err = None;
    let mut dec = match Decoder::new(&d.dev, codec, out_w, out_h, r.fps, enc.header().as_deref()) {
        Ok(x) => Some(x),
        Err(e) => {
            dec_err = Some(format!("{e:?}"));
            None
        }
    };
    let (dec_name, dec_gpu) = dec.as_ref().map(|x| (x.name.clone(), x.gpu)).unwrap_or_default();
    let mut q = None;
    d.dev.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut q))?;
    let q = q.unwrap();

    let mut times: HashMap<i64, Times> = HashMap::new();
    let mut pkts: Vec<Pkt> = Vec::new();
    let mut decoded: Vec<i64> = Vec::new();
    let (mut frames, mut dropped, mut skipped) = (0u32, 0u32, 0u32);
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
        if enc.need > 0 {
            if let Some((pts, got)) = pending.take() {
                let t = conv.convert(&in_view)?;
                enc.feed(&t, pts)?;
                last_fed = pts;
                if pts >= count_from {
                    frames += 1;
                    times.insert(pts, Times { comp: pts, got, fed: now(), ..Default::default() });
                }
            }
        }
        let t_enc = now();
        for p in pkts.drain(..) {
            if let Some(t) = times.get_mut(&p.pts) {
                t.enc = t_enc;
                t.size = p.data.len();
                t.key = p.key;
            }
            if let Some(dc) = &mut dec {
                decoded.clear();
                if let Err(e) = dc.decode(Some(&p), &mut decoded) {
                    dec_err = Some(format!("{e:?}"));
                    dec = None;
                    continue;
                }
                mark(d, &q, &decoded, &mut times);
            }
        }
        if let Some(dc) = &mut dec {
            decoded.clear();
            if let Err(e) = dc.decode(None, &mut decoded) {
                dec_err = Some(format!("{e:?}"));
                dec = None;
            } else {
                mark(d, &q, &decoded, &mut times);
            }
        }
    }
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
    s += &format!("  codificador: {}\n", enc.name);
    if !enc.refused.is_empty() {
        s += &format!("  ajustes recusados: {}\n", enc.refused.join(", "));
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
    let run = Run { secs, out_h, mbps, fps, dd };

    let mut s = String::from("Bancada da transmissão (fase 0)\n");
    s += "Etapas: captura = do momento em que a tela do PC mostra o quadro (o relógio da captura) até ele chegar ao Prisma, e negativo quer dizer que chegou antes de aparecer; preparo = cópia, conversão de cor e escala na GPU até entregar ao codificador, contando a espera pelo codificador; codificação = até o pacote sair; decodificação = até o quadro sair do decodificador; GPU pronta = até a GPU terminar o quadro decodificado; total = do quadro na tela do PC até o quadro decodificado pronto, sem a rede e sem mostrar na outra tela.\n";
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        let _ = timeBeginPeriod(1);
        // Tela apagada por ociosidade muda tudo: a composição desacelera e a Desktop Duplication
        // não entrega nada. Mantém a tela acesa enquanto mede e dá tempo de ela acordar.
        let _ = SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED);
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
