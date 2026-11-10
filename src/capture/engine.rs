//! Motor de vídeo: captura, converte na GPU, codifica e entrega ao arquivo e ao replay.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use windows::core::*;
use windows::Foundation::TypedEventHandler;
use windows::Graphics::Capture::*;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::WinRT::Direct3D11::*;

use super::encoder::Encoder;
use super::hdr::ToneMap;
use super::mux::{Codec, Mp4, Pkt};
use super::{d3d, monitor_color, monitor_item, sound, window_item};

#[derive(Clone)]
pub struct Settings {
    pub target: Target,
    pub out_h: u32,
    pub fps: u32,
    pub codec: Codec,
    pub cursor: bool,
    pub sys_audio: bool,
    /// Programa cujo som entra sozinho na gravação; 0 quando não há.
    pub audio_pid: u32,
    pub mic: bool,
    pub hdr10: bool,
    pub replay_secs: u32,
}

#[derive(Clone, Copy)]
pub enum Target {
    Monitor(isize),
    Window(isize),
}

pub enum Cmd {
    Record(String),
    StopRecord,
    SaveReplay(String),
    Quit,
}

#[derive(Default)]
pub struct Status {
    pub recording: AtomicBool,
    pub running: AtomicBool,
    pub bytes: AtomicU64,
    pub frames: AtomicU64,
    pub dropped: AtomicU64,
    pub failed: AtomicBool,
}

pub struct Handle {
    pub tx: Sender<Cmd>,
    pub status: Arc<Status>,
    pub info: Arc<std::sync::Mutex<String>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Handle {
    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
    pub fn stop(mut self) {
        let _ = self.tx.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn start(cfg: Settings) -> Handle {
    let (tx, rx) = channel();
    let status = Arc::new(Status::default());
    let info = Arc::new(std::sync::Mutex::new(String::new()));
    let (s, i) = (status.clone(), info.clone());
    let thread = std::thread::Builder::new()
        .name("video".into())
        .spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
            s.running.store(true, Ordering::Relaxed);
            if let Err(e) = run(cfg, rx, &s, &i) {
                eprintln!("vídeo: {e:?}");
                s.failed.store(true, Ordering::Relaxed);
            }
            s.running.store(false, Ordering::Relaxed);
            s.recording.store(false, Ordering::Relaxed);
            let _ = MFShutdown();
        })
        .expect("thread de vídeo");
    Handle { tx, status, info, thread: Some(thread) }
}

/// Memória do replay: pacotes recentes, a partir de um quadro-chave.
#[derive(Default)]
struct Ring {
    video: VecDeque<Pkt>,
    audio: VecDeque<Pkt>,
}

impl Ring {
    fn push_video(&mut self, p: Pkt, secs: u32) {
        let limit = p.pts - secs as i64 * 10_000_000;
        self.video.push_back(p);
        while self.video.len() > 1 {
            let next_key = self.video.iter().skip(1).position(|k| k.key);
            match next_key {
                Some(i) if self.video[i + 1].pts <= limit => {
                    self.video.drain(..=i);
                }
                _ => break,
            }
        }
        if let Some(first) = self.video.front() {
            let t = first.pts;
            while self.audio.front().is_some_and(|a| a.pts < t) {
                self.audio.pop_front();
            }
        }
    }

    fn save(&self, path: &str, cfg: &Settings, w: u32, h: u32) -> std::io::Result<()> {
        let audio = !self.audio.is_empty();
        let mut m = Mp4::create(path, cfg.codec, w, h, cfg.fps, cfg.hdr10, audio)?;
        for a in &self.audio {
            m.push_audio(a);
        }
        for v in &self.video {
            m.push_video(v)?;
        }
        m.finish()
    }
}

pub(crate) struct Converter {
    vd: ID3D11VideoDevice,
    vc: ID3D11VideoContext1,
    en: ID3D11VideoProcessorEnumerator,
    vp: ID3D11VideoProcessor,
    outputs: Vec<(ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    next: usize,
}

/// Saída do conversor: NV12 (comum), P010 (HDR10) ou AYUV (4:4:4, feito num shader, ver [`Prep`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ConvOut {
    Nv12,
    P010,
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    Ayuv,
}

/// Prepara o quadro para o codificador: o processador de vídeo para NV12 e P010, um shader para AYUV.
#[cfg(debug_assertions)]
pub(crate) enum Prep {
    Vp(Converter, ID3D11VideoProcessorInputView),
    Ayuv(super::hdr::ToAyuv),
}

#[cfg(debug_assertions)]
impl Prep {
    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn new(dev: &ID3D11Device, ctx: &ID3D11DeviceContext, src: &ID3D11Texture2D, in_w: u32, in_h: u32, w: u32, h: u32, fps: u32, out: ConvOut) -> Result<Self> {
        if out == ConvOut::Ayuv {
            return super::hdr::ToAyuv::new(dev, src, w, h).map(Prep::Ayuv);
        }
        let c = Converter::with(dev, ctx, in_w, in_h, w, h, fps, out)?;
        let v = c.input_view(src)?;
        Ok(Prep::Vp(c, v))
    }

    pub(crate) unsafe fn convert(&mut self, ctx: &ID3D11DeviceContext) -> Result<ID3D11Texture2D> {
        match self {
            Prep::Vp(c, v) => c.convert(v),
            Prep::Ayuv(a) => a.convert(ctx),
        }
    }
}

impl Converter {
    pub(crate) unsafe fn new(dev: &ID3D11Device, ctx: &ID3D11DeviceContext, in_w: u32, in_h: u32, w: u32, h: u32, fps: u32, ten: bool) -> Result<Self> {
        Self::with(dev, ctx, in_w, in_h, w, h, fps, if ten { ConvOut::P010 } else { ConvOut::Nv12 })
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) unsafe fn with(dev: &ID3D11Device, ctx: &ID3D11DeviceContext, in_w: u32, in_h: u32, w: u32, h: u32, fps: u32, out: ConvOut) -> Result<Self> {
        let vd: ID3D11VideoDevice = dev.cast()?;
        let vc: ID3D11VideoContext1 = ctx.cast()?;
        let desc = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
            InputWidth: in_w,
            InputHeight: in_h,
            OutputFrameRate: DXGI_RATIONAL { Numerator: fps, Denominator: 1 },
            OutputWidth: w,
            OutputHeight: h,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        let en = vd.CreateVideoProcessorEnumerator(&desc)?;
        let vp = vd.CreateVideoProcessor(&en, 0)?;
        let (in_cs, out_cs) = match out {
            ConvOut::P010 => (DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020, DXGI_COLOR_SPACE_YCBCR_STUDIO_G2084_LEFT_P2020),
            ConvOut::Nv12 => (DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709, DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709),
            ConvOut::Ayuv => return Err(Error::new(E_INVALIDARG, "o processador de vídeo não grava AYUV; use Prep")),
        };
        vc.VideoProcessorSetStreamColorSpace1(&vp, 0, in_cs);
        vc.VideoProcessorSetOutputColorSpace1(&vp, out_cs);
        vc.VideoProcessorSetStreamFrameFormat(&vp, 0, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE);
        let fmt = match out {
            ConvOut::Nv12 => DXGI_FORMAT_NV12,
            ConvOut::P010 => DXGI_FORMAT_P010,
            ConvOut::Ayuv => DXGI_FORMAT_AYUV,
        };
        let mut outputs = Vec::new();
        for _ in 0..16 {
            let td = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: fmt,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: if out == ConvOut::Nv12 {
                    (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_VIDEO_ENCODER.0) as u32
                } else {
                    D3D11_BIND_RENDER_TARGET.0 as u32
                },
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut t = None;
            dev.CreateTexture2D(&td, None, Some(&mut t)).map_err(|e| Error::new(e.code(), format!("conversor: criar a textura {fmt:?}: {}", e.message())))?;
            let t = t.unwrap();
            let vd_desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
            };
            let mut v = None;
            vd.CreateVideoProcessorOutputView(&t, &en, &vd_desc, Some(&mut v)).map_err(|e| Error::new(e.code(), format!("conversor: saída em {fmt:?}: {}", e.message())))?;
            outputs.push((t, v.unwrap()));
        }
        Ok(Self { vd, vc, en, vp, outputs, next: 0 })
    }

    pub(crate) unsafe fn input_view(&self, tex: &ID3D11Texture2D) -> Result<ID3D11VideoProcessorInputView> {
        let d = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
            },
        };
        let mut v = None;
        self.vd.CreateVideoProcessorInputView(tex, &self.en, &d, Some(&mut v))?;
        Ok(v.unwrap())
    }

    pub(crate) unsafe fn convert(&mut self, view: &ID3D11VideoProcessorInputView) -> Result<ID3D11Texture2D> {
        let (tex, out) = &self.outputs[self.next];
        self.next = (self.next + 1) % self.outputs.len();
        let stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(view.clone())),
            ..Default::default()
        };
        let streams = [stream];
        let r = self.vc.VideoProcessorBlt(&self.vp, out, 0, &streams);
        let mut streams = streams;
        std::mem::ManuallyDrop::drop(&mut streams[0].pInputSurface);
        r?;
        Ok(tex.clone())
    }
}

pub(crate) fn even(v: u32) -> u32 {
    (v & !1).max(2)
}

pub(crate) unsafe fn bgra_texture(dev: &ID3D11Device, w: u32, h: u32, fmt: DXGI_FORMAT) -> Result<ID3D11Texture2D> {
    let d = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut t = None;
    dev.CreateTexture2D(&d, None, Some(&mut t))?;
    Ok(t.unwrap())
}

pub fn bitrate(w: u32, h: u32, fps: u32, codec: Codec) -> u32 {
    // Calibrado para 4K a 120 quadros dar cerca de 40 Mbit/s em AV1.
    let px = (w as f64 * h as f64) / (3840.0 * 2160.0);
    let rate = (fps as f64 / 120.0).powf(0.55);
    let base = 40_000_000.0 * px.powf(0.75) * rate;
    let k = match codec {
        Codec::Av1 => 1.0,
        Codec::Hevc => 1.3,
        Codec::H264 => 2.0,
    };
    (base * k) as u32
}

fn run(mut cfg: Settings, rx: Receiver<Cmd>, status: &Status, info: &std::sync::Mutex<String>) -> Result<()> {
    unsafe {
        let d = d3d()?;
        if let Ok(mt) = d.ctx.cast::<ID3D11Multithread>() {
            let _ = mt.SetMultithreadProtected(true);
        }
        let (item, mon) = match cfg.target {
            Target::Monitor(m) => {
                let h = windows::Win32::Graphics::Gdi::HMONITOR(m as *mut _);
                (monitor_item(h)?, h)
            }
            Target::Window(w) => {
                let h = HWND(w as *mut _);
                let m = windows::Win32::Graphics::Gdi::MonitorFromWindow(h, windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST);
                (window_item(h)?, m)
            }
        };
        #[allow(unused_mut)]
        let mut color = monitor_color(mon);
        #[cfg(debug_assertions)]
        if std::env::var("PRISMA_FORCE_HDR").is_ok() {
            color.hdr = true;
        }
        let size = item.Size()?;
        let (src_w, src_h) = (even(size.Width as u32), even(size.Height as u32));
        let out_h = if cfg.out_h == 0 || cfg.out_h >= src_h { src_h } else { even(cfg.out_h) };
        let out_w = even((src_w as u64 * out_h as u64 / src_h as u64) as u32);
        let hdr10 = cfg.hdr10 && color.hdr;
        if hdr10 && cfg.codec == Codec::H264 {
            // H.264 da GPU não tem 10 bits.
            cfg.codec = Codec::Hevc;
        }

        let fmt = if color.hdr { DirectXPixelFormat::R16G16B16A16Float } else { DirectXPixelFormat::B8G8R8A8UIntNormalized };
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d.rt, fmt, 3, size)?;
        let event = CreateEventW(None, false, false, None)?;
        let ev = event.0 as isize;
        let token = pool.FrameArrived(&TypedEventHandler::new(move |_, _| {
            let _ = SetEvent(HANDLE(ev as *mut _));
            Ok(())
        }))?;
        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(cfg.cursor);
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetMinUpdateInterval(windows::Foundation::TimeSpan { Duration: 10_000_000 / cfg.fps as i64 });

        // Quadro intermediário: BGRA em SDR, ou PQ de 10 bits ao gravar em HDR.
        let mid_fmt = if hdr10 { DXGI_FORMAT_R10G10B10A2_UNORM } else { DXGI_FORMAT_B8G8R8A8_UNORM };
        let mid = bgra_texture(&d.dev, src_w, src_h, mid_fmt)?;
        let tone = if color.hdr { Some(ToneMap::new(&d.dev, &mid, hdr10, color.white_nits)?) } else { None };
        let mut conv = Converter::new(&d.dev, &d.ctx, src_w, src_h, out_w, out_h, cfg.fps, hdr10)?;
        let in_view = conv.input_view(&mid)?;

        // Placa sem o codec escolhido cai para o melhor que ela tiver.
        let order = [cfg.codec, Codec::Av1, Codec::Hevc, Codec::H264];
        let mut made = None;
        for c in order {
            if hdr10 && c == Codec::H264 {
                continue;
            }
            let bits = bitrate(out_w, out_h, cfg.fps, c);
            if let Ok(e) = Encoder::new(&d.dev, c, out_w, out_h, cfg.fps, bits, hdr10) {
                cfg.codec = c;
                made = Some(e);
                break;
            }
        }
        let mut enc = made.ok_or_else(|| Error::new(E_FAIL, "sem codificador de vídeo na GPU"))?;
        {
            let dyn_ = if hdr10 { "HDR" } else { "SDR" };
            let codec = match cfg.codec {
                Codec::Av1 => "AV1",
                Codec::Hevc => "HEVC",
                Codec::H264 => "H.264",
            };
            let res = if out_h >= 2160 { "4K".to_string() } else { format!("{out_h}p") };
            *info.lock().unwrap() = format!("{res} · {} FPS · {codec} · {dyn_}", cfg.fps);
        }

        let stop_audio = Arc::new(AtomicBool::new(false));
        let (atx, arx) = channel::<Pkt>();
        let want_audio = cfg.sys_audio || cfg.mic || cfg.audio_pid != 0;
        if want_audio {
            sound::spawn(cfg.sys_audio, cfg.audio_pid, cfg.mic, stop_audio.clone(), atx.clone());
        }
        drop(atx);

        session.StartCapture()?;
        let interval = 10_000_000i64 / cfg.fps as i64;
        let mut last_fed = i64::MIN / 2;
        let mut ring = Ring::default();
        let mut file: Option<Mp4> = None;
        let mut out: Vec<Pkt> = Vec::new();
        let mut quit = false;
        let mut failed = None;

        while !quit {
            while let Ok(c) = rx.try_recv() {
                match c {
                    Cmd::Record(path) => {
                        match Mp4::create(&path, cfg.codec, out_w, out_h, cfg.fps, hdr10, want_audio) {
                            Ok(m) => {
                                file = Some(m);
                                enc.force_key();
                                status.recording.store(true, Ordering::Relaxed);
                                status.bytes.store(0, Ordering::Relaxed);
                            }
                            Err(_) => status.failed.store(true, Ordering::Relaxed),
                        }
                    }
                    Cmd::StopRecord => {
                        if let Some(m) = file.take() {
                            let _ = m.finish();
                        }
                        status.recording.store(false, Ordering::Relaxed);
                    }
                    Cmd::SaveReplay(path) => {
                        let _ = ring.save(&path, &cfg, out_w, out_h);
                    }
                    Cmd::Quit => quit = true,
                }
            }
            let _ = WaitForSingleObject(event, 4);

            if let Err(e) = enc.pump(&mut out) {
                failed = Some(e);
                break;
            }
            while let Ok(a) = arx.try_recv() {
                if let Some(m) = &mut file {
                    m.push_audio(&a);
                }
                if cfg.replay_secs > 0 {
                    ring.audio.push_back(a);
                }
            }
            for p in out.drain(..) {
                if let Some(m) = &mut file {
                    if m.push_video(&p).is_err() {
                        status.failed.store(true, Ordering::Relaxed);
                    }
                    status.bytes.store(m.bytes, Ordering::Relaxed);
                }
                if cfg.replay_secs > 0 {
                    ring.push_video(p, cfg.replay_secs);
                }
            }

            while let Ok(frame) = pool.TryGetNextFrame() {
                let pts = frame.SystemRelativeTime()?.Duration;
                if pts - last_fed < interval * 9 / 10 {
                    let _ = frame.Close();
                    continue;
                }
                if enc.need == 0 {
                    status.dropped.fetch_add(1, Ordering::Relaxed);
                    let _ = frame.Close();
                    continue;
                }
                let surf = frame.Surface()?;
                let access: IDirect3DDxgiInterfaceAccess = surf.cast()?;
                let tex: ID3D11Texture2D = access.GetInterface()?;
                if let Some(t) = &tone {
                    t.draw(&d.ctx, &d.dev, &tex, src_w, src_h)?;
                } else {
                    let bx = D3D11_BOX { left: 0, top: 0, front: 0, right: src_w, bottom: src_h, back: 1 };
                    d.ctx.CopySubresourceRegion(&mid, 0, 0, 0, 0, &tex, 0, Some(&bx));
                }
                let t = conv.convert(&in_view)?;
                let _ = frame.Close();
                enc.feed(&t, pts)?;
                last_fed = pts;
                status.frames.fetch_add(1, Ordering::Relaxed);
            }
        }

        stop_audio.store(true, Ordering::Relaxed);
        pool.RemoveFrameArrived(token)?;
        let _ = session.Close();
        let _ = pool.Close();
        enc.drain(&mut out);
        std::thread::sleep(Duration::from_millis(40));
        for p in out.drain(..) {
            if let Some(m) = &mut file {
                let _ = m.push_video(&p);
            }
        }
        while let Ok(a) = arx.try_recv() {
            if let Some(m) = &mut file {
                m.push_audio(&a);
            }
        }
        if let Some(m) = file.take() {
            let _ = m.finish();
        }
        let _ = CloseHandle(event);
        match failed {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
