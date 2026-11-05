//! Teste de sincronia: clarão na tela e clique no som, ao mesmo tempo, em ciclos fixos.
//! A pessoa atrasa o clarão até os dois coincidirem, ou filma com o celular e conta os quadros.

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::patterns::find_output;
use crate::ui::gfx::{rect, rgb, Align, Canvas, Weight};

const PERIOD_MS: u64 = 1500;
const FLASH_MS: f64 = 100.0;
const CLICK_MS: u64 = 12;
const PANEL_H: u32 = 110;
const MAX_OFFSET: i32 = 1000;

/// Formato de som na hora da medida e o atraso achado, em ms.
#[derive(Default)]
pub struct State {
    pub running: bool,
    pub failed: bool,
    pub results: Vec<(String, i32)>,
}

pub type Shared = Arc<Mutex<State>>;

struct Keys {
    offset: i32,
    done: bool,
}

thread_local! {
    static KEYS: RefCell<Keys> = const { RefCell::new(Keys { offset: 0, done: false }) };
}

unsafe extern "system" fn proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_KEYDOWN => {
            KEYS.with(|k| {
                let mut k = k.borrow_mut();
                match VIRTUAL_KEY(w.0 as u16) {
                    VK_ESCAPE | VK_RETURN => k.done = true,
                    VK_LEFT => k.offset -= 5,
                    VK_RIGHT => k.offset += 5,
                    VK_DOWN => k.offset -= 25,
                    VK_UP => k.offset += 25,
                    _ => {}
                }
                k.offset = k.offset.clamp(0, MAX_OFFSET);
            });
            LRESULT(0)
        }
        WM_SETCURSOR => {
            SetCursor(None);
            LRESULT(1)
        }
        WM_CLOSE => {
            KEYS.with(|k| k.borrow_mut().done = true);
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

struct Sound {
    client: IAudioClient,
    render: IAudioRenderClient,
    clock: IAudioClock,
    frequency: u64,
    rate: u64,
    channels: usize,
    size: u32,
    written: u64,
}

impl Sound {
    unsafe fn open() -> Result<Sound> {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let format = client.GetMixFormat()?;
        let (rate, channels) = ((*format).nSamplesPerSec as u64, (*format).nChannels as usize);
        let opened = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 2_000_000, 0, format, None);
        CoTaskMemFree(Some(format as *const _));
        opened?;
        let render: IAudioRenderClient = client.GetService()?;
        let clock: IAudioClock = client.GetService()?;
        let frequency = clock.GetFrequency()?;
        let size = client.GetBufferSize()?;
        let mut s = Sound { client, render, clock, frequency, rate, channels, size, written: 0 };
        s.fill()?;
        s.client.Start()?;
        Ok(s)
    }

    fn period(&self) -> u64 {
        self.rate * PERIOD_MS / 1000
    }

    /// Mantém o som correndo em silêncio: o clique sai numa posição exata do fluxo.
    unsafe fn fill(&mut self) -> Result<()> {
        let free = self.size - self.client.GetCurrentPadding()?;
        if free == 0 {
            return Ok(());
        }
        let data = self.render.GetBuffer(free)? as *mut f32;
        let out = std::slice::from_raw_parts_mut(data, free as usize * self.channels);
        let length = self.rate * CLICK_MS / 1000;
        for (i, frame) in out.chunks_exact_mut(self.channels).enumerate() {
            let at = (self.written + i as u64) % self.period();
            let v = if at < length { 0.6 * (at as f32 * 2000.0 * std::f32::consts::PI / self.rate as f32).sin() } else { 0.0 };
            frame.fill(0.0);
            frame[0] = v;
            if self.channels > 1 {
                frame[1] = v;
            }
        }
        self.render.ReleaseBuffer(free, 0)?;
        self.written += free as u64;
        Ok(())
    }

    /// Instante do próximo clique, em microssegundos do relógio do sistema.
    unsafe fn next_click(&self, now_us: f64, offset_us: f64) -> Result<f64> {
        let (mut pos, mut qpc) = (0u64, 0u64);
        self.clock.GetPosition(&mut pos, Some(&mut qpc))?;
        let played = pos as f64 / self.frequency as f64 * self.rate as f64;
        let at_us = qpc as f64 / 10.0;
        let time = |frame: f64| at_us + (frame - played) / self.rate as f64 * 1_000_000.0;
        let period = self.period() as f64;
        let mut frame = (played / period).floor() * period;
        while time(frame) + offset_us + FLASH_MS * 1000.0 < now_us {
            frame += period;
        }
        Ok(time(frame))
    }
}

fn now_us(frequency: i64) -> f64 {
    let mut c = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
    }
    c as f64 / frequency as f64 * 1_000_000.0
}

struct View {
    swap: IDXGISwapChain1,
    ctx: ID3D11DeviceContext1,
    rtv: ID3D11RenderTargetView,
    back: ID3D11Texture2D,
    panel: ID3D11Texture2D,
    canvas: Canvas,
    target: IWICBitmap,
    size: (i32, i32),
    panel_size: (u32, u32),
    shown: i32,
}

impl View {
    unsafe fn text(&mut self, offset: i32) -> Result<()> {
        self.shown = offset;
        let (w, h) = self.panel_size;
        let k = self.size.1 as f32 / 1080.0;
        let rt = &self.canvas.rt;
        rt.BeginDraw();
        rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }));
        let color = rgb(0xb8b8b8);
        let full = w as f32;
        let a = format!("Atraso do som: {offset} ms");
        let b = "Ajuste até o clarão e o clique parecerem um só. Para medir com mais precisão, filme com o celular e deixe em 0 ms.";
        let c = "← → mudam 5 ms · ↑ ↓ mudam 25 ms · Enter ou Esc guarda e sai";
        self.canvas.text(rect(0.0, 4.0 * k, full, 34.0 * k), &a, 24.0 * k, color, Align::Center, Weight::Semibold);
        self.canvas.text(rect(0.0, 42.0 * k, full, 26.0 * k), b, 18.0 * k, color, Align::Center, Weight::Regular);
        self.canvas.text(rect(0.0, 72.0 * k, full, 26.0 * k), c, 17.0 * k, color, Align::Center, Weight::Regular);
        rt.EndDraw(None, None)?;
        let mut px = vec![0u8; (w * h * 4) as usize];
        self.target.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
        self.ctx.UpdateSubresource(&self.panel, 0, None, px.as_ptr() as *const _, w * 4, 0);
        Ok(())
    }

    unsafe fn draw(&self, flash: bool) -> Result<()> {
        self.ctx.ClearView(&self.rtv, &[0.0, 0.0, 0.0, 1.0], None);
        if flash {
            let (w, h) = self.size;
            let r = RECT { left: w / 4, top: h / 8, right: w * 3 / 4, bottom: h * 5 / 8 };
            self.ctx.ClearView(&self.rtv, &[1.0, 1.0, 1.0, 1.0], Some(&[r]));
        }
        let (w, h) = self.panel_size;
        self.ctx.CopySubresourceRegion(&self.back, 0, (self.size.0 as u32 - w) / 2, self.size.1 as u32 - h - self.size.1 as u32 / 30, 0, &self.panel, 0, None);
        self.swap.Present(1, DXGI_PRESENT(0)).ok()
    }
}

unsafe fn open(gdi: &str) -> Result<(HWND, View)> {
    let (adapter, full) = find_output(gdi)?;
    #[cfg(debug_assertions)]
    let area = if std::env::var("PRISMA_PATTERN_SMALL").is_ok() { RECT { left: 40, top: 40, right: 680, bottom: 400 } } else { full };
    #[cfg(not(debug_assertions))]
    let area = full;
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    let inst = GetModuleHandleW(None)?;
    let class = w!("PrismaSincronia");
    let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(proc), hInstance: inst.into(), lpszClassName: class, ..Default::default() };
    RegisterClassExW(&wc);
    let hwnd = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW, class, w!("Prisma"), WS_POPUP, area.left, area.top, w, h, None, None, Some(inst.into()), None)?;

    let mut dev = None;
    let mut ctx = None;
    D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), None, D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx))?;
    let dev: ID3D11Device = dev.ok_or(Error::from(E_FAIL))?;
    let ctx: ID3D11DeviceContext1 = ctx.ok_or(Error::from(E_FAIL))?.cast()?;
    let dxgi: IDXGIDevice = dev.cast()?;
    let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
    const FORMAT: DXGI_FORMAT = DXGI_FORMAT_B8G8R8A8_UNORM;
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: w as u32,
        Height: h as u32,
        Format: FORMAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        ..Default::default()
    };
    let swap = factory.CreateSwapChainForHwnd(&dev, hwnd, &desc, None, None)?;
    factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)?;
    let back: ID3D11Texture2D = swap.GetBuffer(0)?;
    let mut rtv = None;
    dev.CreateRenderTargetView(&back, None, Some(&mut rtv))?;

    let k = h as f32 / 1080.0;
    let (pw, ph) = ((w as f32 * 0.9) as u32, ((PANEL_H as f32 * k) as u32).min(h as u32 / 2));
    let tex = D3D11_TEXTURE2D_DESC {
        Width: pw,
        Height: ph,
        MipLevels: 1,
        ArraySize: 1,
        Format: FORMAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        ..Default::default()
    };
    let mut panel = None;
    dev.CreateTexture2D(&tex, None, Some(&mut panel))?;
    let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
    let target = wic.CreateBitmap(pw, ph, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand)?;
    let props = D2D1_RENDER_TARGET_PROPERTIES {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.0,
        dpiY: 96.0,
        ..Default::default()
    };
    let rt = crate::ui::gfx::factory().CreateWicBitmapRenderTarget(&target, &props)?;
    let view = View { swap, ctx, rtv: rtv.ok_or(Error::from(E_FAIL))?, back, panel: panel.ok_or(Error::from(E_FAIL))?, canvas: Canvas::new(rt.cast()?), target, size: (w, h), panel_size: (pw, ph), shown: -1 };
    Ok((hwnd, view))
}

unsafe fn measure(gdi: &str, hz: f32) -> Result<i32> {
    KEYS.with(|k| *k.borrow_mut() = Keys { offset: 0, done: false });
    let (hwnd, mut view) = open(gdi)?;
    let mut sound = Sound::open()?;
    let _ = ShowWindow(hwnd, SW_SHOW);
    let _ = SetForegroundWindow(hwnd);
    let _ = SetFocus(Some(hwnd));
    let mut frequency = 0i64;
    QueryPerformanceFrequency(&mut frequency)?;
    let frame_us = 1_000_000.0 / hz.max(30.0) as f64;
    loop {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let (offset, done) = KEYS.with(|k| {
            let k = k.borrow();
            (k.offset, k.done)
        });
        if done {
            break;
        }
        if view.shown != offset {
            view.text(offset)?;
        }
        sound.fill()?;
        let now = now_us(frequency);
        let start = sound.next_click(now, offset as f64 * 1000.0)? + offset as f64 * 1000.0;
        // O quadro desenhado agora só aparece no próximo ciclo da tela.
        let shown_at = now + frame_us;
        view.draw(shown_at >= start && shown_at < start + FLASH_MS * 1000.0)?;
    }
    let _ = sound.client.Stop();
    let _ = DestroyWindow(hwnd);
    Ok(KEYS.with(|k| k.borrow().offset))
}

/// Mede em segundo plano e guarda o resultado com o formato de som usado.
pub fn run(shared: Shared, gdi: String, format: String, hz: f32, window: isize) {
    {
        let mut s = shared.lock().unwrap();
        s.running = true;
        s.failed = false;
    }
    let _ = std::thread::Builder::new().name("sincronia".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let result = measure(&gdi, hz);
        CoUninitialize();
        let mut s = shared.lock().unwrap();
        s.running = false;
        #[cfg(debug_assertions)]
        if let Err(e) = &result {
            let _ = std::fs::write(std::env::temp_dir().join("prisma_sincronia.txt"), format!("{e:?}"));
        }
        match result {
            Ok(ms) => {
                s.results.retain(|r| r.0 != format);
                s.results.push((format, ms));
            }
            Err(_) => s.failed = true,
        }
        drop(s);
        let _ = InvalidateRect(Some(HWND(window as *mut _)), None, false);
    });
}
