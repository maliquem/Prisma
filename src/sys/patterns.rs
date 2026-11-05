//! Padrões de teste em tela cheia, em 10 bits PQ: cada cor é um código exato do sinal HDR.

use std::cell::RefCell;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{CreateRectRgn, SetWindowRgn, HRGN};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::ui::gfx::{rect, rgb, Align, Canvas, Weight};

/// Fundo no máximo do sinal: a tela corta nele, e o quadrado some ao chegar nesse corte.
const TOP: i32 = 1023;
const PANEL_H: u32 = 130;
const SETTLE_MS: u32 = 1200;
const TIMER: usize = 1;

#[derive(Clone, Copy, Debug, Default)]
pub struct Measured {
    pub black: f32,
    /// Pico em janelas de 2, 10, 25, 50 e 100% da tela.
    pub windows: [f32; 5],
    /// Vermelho, verde e azul sozinhos, em janela de 10%.
    pub channels: [f32; 3],
}

pub const WINDOWS: [u32; 5] = [2, 10, 25, 50, 100];

#[derive(Clone, Copy)]
enum Kind {
    Black,
    Window(usize),
    Channel(usize),
}

struct Step {
    kind: Kind,
    code: i32,
}

/// Nits de um código de 10 bits.
pub fn nits(code: i32) -> f32 {
    let (m1, m2, c1, c2, c3) = (0.159_301_76_f32, 78.843_75_f32, 0.835_937_5_f32, 18.851_563_f32, 18.687_5_f32);
    let e = (code as f32 / 1023.0).powf(1.0 / m2);
    10000.0 * ((e - c1).max(0.0) / (c2 - c3 * e)).powf(1.0 / m1)
}

/// Valor na tela, com casas só onde um passo mudaria menos que uma unidade.
fn shown(v: f32) -> String {
    let s = if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 10.0 {
        format!("{v:.1}")
    } else if v >= 0.1 {
        format!("{v:.2}")
    } else {
        format!("{v:.4}")
    };
    s.replace('.', crate::i18n::comma())
}

fn code_of(v: f32) -> i32 {
    (0..=1023).find(|c| nits(*c) >= v).unwrap_or(1023)
}

fn steps() -> Vec<Step> {
    let start = code_of(300.0);
    let mut list = vec![Step { kind: Kind::Black, code: code_of(0.4) }];
    list.extend([1, 0, 2, 3, 4].map(|i| Step { kind: Kind::Window(i), code: start }));
    list.extend((0..3).map(|c| Step { kind: Kind::Channel(c), code: start }));
    list
}

struct Run {
    swap: IDXGISwapChain3,
    ctx: ID3D11DeviceContext1,
    rtv: ID3D11RenderTargetView,
    back: ID3D11Texture2D,
    panel: ID3D11Texture2D,
    canvas: Canvas,
    target: IWICBitmap,
    size: (i32, i32),
    panel_size: (u32, u32),
    steps: Vec<Step>,
    now: usize,
    surround: bool,
    hidden: bool,
    composed: bool,
    settling: bool,
    done: Option<bool>,
}

thread_local! {
    static RUN: RefCell<Option<Run>> = const { RefCell::new(None) };
}

type Fill = (RECT, [i32; 3]);

fn centered(w: i32, h: i32, cx: i32, cy: i32) -> RECT {
    RECT { left: cx - w / 2, top: cy - h / 2, right: cx + w / 2, bottom: cy + h / 2 }
}

impl Run {
    fn fills(&self) -> Vec<Fill> {
        let (w, h) = self.size;
        let (cx, cy) = (w / 2, h / 2);
        let step = &self.steps[self.now];
        let mut out = Vec::new();
        if self.settling {
            return out;
        }
        let window = |pct: u32| {
            let k = (pct as f32 / 100.0).sqrt();
            centered((w as f32 * k) as i32, (h as f32 * k) as i32, cx, cy)
        };
        match step.kind {
            Kind::Black => {
                if self.surround {
                    // Em mini-LED, preto puro em volta apaga as zonas e esconde a forma.
                    const FRAME: i32 = 130;
                    let b = h / 20;
                    for r in [RECT { left: 0, top: 0, right: w, bottom: b }, RECT { left: 0, top: h - b, right: w, bottom: h }, RECT { left: 0, top: 0, right: b, bottom: h }, RECT { left: w - b, top: 0, right: w, bottom: h }] {
                        out.push((r, [FRAME; 3]));
                    }
                }
                let s = h / 10;
                for (dx, dy) in [(0, 0), (-2, 0), (2, 0), (0, -2), (0, 2)] {
                    out.push((centered(s, s, cx + dx * s, cy + dy * s), [step.code; 3]));
                }
            }
            Kind::Window(i) => {
                let r = window(WINDOWS[i]);
                out.push((r, [TOP; 3]));
                let size = ((r.right - r.left).min(r.bottom - r.top) / 3).min(h / 4);
                out.push((centered(size, size, cx, cy), [step.code; 3]));
            }
            Kind::Channel(c) => {
                let paint = |v: i32| {
                    let mut p = [0; 3];
                    p[c] = v;
                    p
                };
                let r = window(10);
                out.push((r, paint(TOP)));
                let size = (r.bottom - r.top) / 3;
                out.push((centered(size, size, cx, cy), paint(step.code)));
            }
        }
        out
    }

    fn text(&self) -> (String, String, String) {
        let step = &self.steps[self.now];
        let n = self.now + 1;
        let total = self.steps.len();
        let path = if self.composed { "pelo Windows" } else { "direto" };
        let keys = format!("← → ajustam · ↑ ↓ ajustam mais rápido · Enter confirma · Backspace volta · H esconde este texto · T troca o caminho (agora {path}) · Esc sai");
        match step.kind {
            Kind::Black => (
                format!("Passo {n} de {total}: preto"),
                format!("Abaixe até as formas sumirem e pare no primeiro valor em que somem. Agora: {} nits. S liga ou desliga a moldura.", shown(nits(step.code))),
                keys.to_string(),
            ),
            Kind::Window(i) => (
                format!("Passo {n} de {total}: pico em janela de {}%", WINDOWS[i]),
                format!("Aumente até o quadrado escuro do centro sumir no fundo claro e pare no primeiro valor em que some. Agora: {} nits.", shown(nits(step.code))),
                keys.to_string(),
            ),
            Kind::Channel(c) => (
                format!("Passo {n} de {total}: canal {}", ["vermelho", "verde", "azul"][c]),
                format!("Aumente até o quadrado escuro do centro sumir no fundo claro e pare no primeiro valor em que some. Agora: {} nits.", shown(nits(step.code))),
                keys.to_string(),
            ),
        }
    }

    /// Texto em tom baixo, para não acender zonas perto das formas.
    unsafe fn panel(&self) -> Result<()> {
        let (w, h) = self.panel_size;
        let k = self.size.1 as f32 / 1080.0;
        let (a, b, c) = self.text();
        let rt = &self.canvas.rt;
        rt.BeginDraw();
        rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }));
        let white = rgb(0xffffff);
        let full = w as f32;
        self.canvas.text(rect(0.0, 6.0 * k, full, 34.0 * k), &a, 24.0 * k, white, Align::Center, Weight::Semibold);
        self.canvas.text(rect(0.0, 46.0 * k, full, 28.0 * k), &b, 19.0 * k, white, Align::Center, Weight::Regular);
        self.canvas.text(rect(0.0, 84.0 * k, full, 28.0 * k), &c, 17.0 * k, white, Align::Center, Weight::Regular);
        rt.EndDraw(None, None)?;
        let mut px = vec![0u8; (w * h * 4) as usize];
        self.target.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
        let top = code_of(if matches!(self.steps[self.now].kind, Kind::Black) { 6.0 } else { 40.0 }) as u32;
        let packed: Vec<u32> = px.chunks_exact(4).map(|p| {
            let v = p[1] as u32 * top / 255;
            v | (v << 10) | (v << 20) | (3 << 30)
        }).collect();
        self.ctx.UpdateSubresource(&self.panel, 0, None, packed.as_ptr() as *const _, w * 4, 0);
        Ok(())
    }

    unsafe fn draw(&self) -> Result<()> {
        let black = [0.0, 0.0, 0.0, 1.0];
        self.ctx.ClearView(&self.rtv, &black, None);
        for (r, c) in self.fills() {
            let color = [c[0] as f32 / 1023.0, c[1] as f32 / 1023.0, c[2] as f32 / 1023.0, 1.0];
            self.ctx.ClearView(&self.rtv, &color, Some(&[r]));
        }
        if !self.hidden && !self.settling {
            self.panel()?;
            let (w, h) = self.panel_size;
            let (x, y) = ((self.size.0 as u32 - w) / 2, self.size.1 as u32 - h - self.size.1 as u32 / 40);
            self.ctx.CopySubresourceRegion(&self.back, 0, x, y, 0, &self.panel, 0, None);
        }
        self.swap.Present(1, DXGI_PRESENT(0)).ok()
    }

    /// Tirar uma linha da janela faz o Windows compor a imagem, como no app de calibração.
    /// Direto na tela, a TV não recebe as mesmas informações e pode nem cortar.
    fn apply_path(&self, hwnd: HWND) {
        unsafe {
            let region = if self.composed { CreateRectRgn(0, 0, self.size.0, self.size.1 - 1) } else { HRGN::default() };
            SetWindowRgn(hwnd, Some(region), true);
        }
    }

    fn nudge(&mut self, by: i32) {
        let step = &mut self.steps[self.now];
        step.code = (step.code + by).clamp(0, TOP);
    }

    fn settle(&mut self, hwnd: HWND) {
        self.settling = true;
        unsafe { SetTimer(Some(hwnd), TIMER, SETTLE_MS, None) };
    }

    fn key(&mut self, hwnd: HWND, vk: u16) {
        // Enquanto a tela assenta, um Enter a mais não pode pular o passo seguinte.
        if self.settling && VIRTUAL_KEY(vk) != VK_ESCAPE {
            return;
        }
        match VIRTUAL_KEY(vk) {
            VK_ESCAPE => self.done = Some(false),
            VK_RETURN if self.now + 1 == self.steps.len() => self.done = Some(true),
            VK_RETURN => {
                // Recomeça abaixo do corte que a tela acabou de mostrar, com o quadrado à vista.
                let start = (self.steps[self.now].code - 60).max(0);
                let first_channel = matches!(self.steps[self.now + 1].kind, Kind::Channel(0));
                let reference = self.steps.iter().find(|s| matches!(s.kind, Kind::Window(1))).map_or(start, |s| s.code - 60);
                self.now += 1;
                if !matches!(self.steps[self.now - 1].kind, Kind::Black) {
                    self.steps[self.now].code = if first_channel { reference } else { start };
                }
                self.settle(hwnd);
            }
            VK_BACK if self.now > 0 => {
                self.now -= 1;
                self.settle(hwnd);
            }
            VK_LEFT => self.nudge(-1),
            VK_RIGHT => self.nudge(1),
            VK_DOWN => self.nudge(-8),
            VK_UP => self.nudge(8),
            k if k.0 == b'H' as u16 => self.hidden = !self.hidden,
            k if k.0 == b'S' as u16 => self.surround = !self.surround,
            k if k.0 == b'T' as u16 => {
                self.composed = !self.composed;
                self.apply_path(hwnd);
            }
            _ => return,
        }
    }

    fn result(&self) -> Measured {
        let mut m = Measured::default();
        for s in &self.steps {
            match s.kind {
                Kind::Black => m.black = nits(s.code),
                Kind::Window(i) => m.windows[i] = nits(s.code),
                Kind::Channel(c) => m.channels[c] = nits(s.code),
            }
        }
        m
    }
}

unsafe extern "system" fn proc(h: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let act = |f: &mut dyn FnMut(&mut Run)| {
        RUN.with(|r| {
            if let Some(run) = r.borrow_mut().as_mut() {
                f(run);
                // Com a tecla presa, desenha só o último valor em vez de acumular atraso.
                let mut next = MSG::default();
                let waiting = PeekMessageW(&mut next, Some(h), WM_KEYDOWN, WM_KEYDOWN, PM_NOREMOVE).as_bool();
                if !waiting {
                    let _ = run.draw();
                }
            }
        });
    };
    match msg {
        WM_KEYDOWN => {
            act(&mut |run| run.key(h, w.0 as u16));
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let up = ((w.0 >> 16) & 0xffff) as i16 > 0;
            act(&mut |run| run.nudge(if up { 3 } else { -3 }));
            LRESULT(0)
        }
        WM_TIMER => {
            let _ = KillTimer(Some(h), TIMER);
            act(&mut |run| run.settling = false);
            LRESULT(0)
        }
        WM_SETCURSOR => {
            SetCursor(None);
            LRESULT(1)
        }
        WM_CLOSE => {
            act(&mut |run| run.done = Some(false));
            LRESULT(0)
        }
        _ => DefWindowProcW(h, msg, w, l),
    }
}

/// Área da saída na área de trabalho, e a saída em si.
unsafe fn find_output(gdi: &str) -> Result<(IDXGIAdapter1, RECT)> {
    let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
    let mut a = 0;
    while let Ok(adapter) = factory.EnumAdapters1(a) {
        a += 1;
        let mut o = 0;
        while let Ok(output) = adapter.EnumOutputs(o) {
            o += 1;
            let d = output.GetDesc()?;
            if String::from_utf16_lossy(&d.DeviceName).trim_end_matches('\0') == gdi {
                return Ok((adapter, d.DesktopCoordinates));
            }
        }
    }
    Err(E_FAIL.into())
}

unsafe fn open(gdi: &str, area: Option<RECT>) -> Result<(HWND, Run)> {
    let (adapter, full) = find_output(gdi)?;
    let area = area.unwrap_or(full);
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    let inst = GetModuleHandleW(None)?;
    let class = w!("PrismaPadrao");
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
    const FORMAT: DXGI_FORMAT = DXGI_FORMAT_R10G10B10A2_UNORM;
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
    let swap: IDXGISwapChain3 = factory.CreateSwapChainForHwnd(&dev, hwnd, &desc, None, None)?.cast()?;
    swap.SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020)?;
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
    let rt = factory_d2d().CreateWicBitmapRenderTarget(&target, &props)?;
    let run = Run {
        swap,
        ctx,
        rtv: rtv.ok_or(Error::from(E_FAIL))?,
        back,
        panel: panel.ok_or(Error::from(E_FAIL))?,
        canvas: Canvas::new(rt.cast()?),
        target,
        size: (w, h),
        panel_size: (pw, ph),
        steps: steps(),
        now: 0,
        surround: true,
        hidden: false,
        composed: true,
        settling: false,
        done: None,
    };
    Ok((hwnd, run))
}

fn factory_d2d() -> ID2D1Factory {
    crate::ui::gfx::factory()
}

/// `Ok`: terminou todos os passos. `Err(None)`: a pessoa saiu. `Err(Some)`: falhou.
pub fn run(gdi: &str) -> std::result::Result<Measured, Option<&'static str>> {
    const FAILED: &str = "Não consegui abrir os padrões em HDR.";
    unsafe {
        #[cfg(debug_assertions)]
        let area = std::env::var("PRISMA_PATTERN_SMALL").ok().map(|_| RECT { left: 40, top: 40, right: 680, bottom: 400 });
        #[cfg(not(debug_assertions))]
        let area = None;
        let (hwnd, run) = open(gdi, area).map_err(|_| Some(FAILED))?;
        run.apply_path(hwnd);
        let _ = run.draw();
        RUN.with(|r| *r.borrow_mut() = Some(run));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        let mut msg = MSG::default();
        let mut out = Err(None);
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
            let finished = RUN.with(|r| r.borrow().as_ref().and_then(|x| x.done.map(|d| (d, x.result()))));
            if let Some((done, m)) = finished {
                if done {
                    out = Ok(m);
                }
                break;
            }
        }
        RUN.with(|r| *r.borrow_mut() = None);
        let _ = DestroyWindow(hwnd);
        out
    }
}

/// Teste: desenha cada passo numa janela pequena e confere os códigos que saem.
#[cfg(debug_assertions)]
pub fn exact_test(gdi: &str) -> String {
    let mut out = String::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let area = RECT { left: 40, top: 40, right: 680, bottom: 400 };
        let (hwnd, mut run) = match open(gdi, Some(area)) {
            Ok(v) => v,
            Err(e) => return format!("abrir falhou: {e:?}"),
        };
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let (_, dev) = (0, run.swap.GetDevice::<ID3D11Device>().unwrap());
        let mut d = D3D11_TEXTURE2D_DESC::default();
        run.back.GetDesc(&mut d);
        d.Usage = D3D11_USAGE_STAGING;
        d.BindFlags = 0;
        d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        dev.CreateTexture2D(&d, None, Some(&mut staging)).unwrap();
        let staging = staging.unwrap();
        for i in 0..run.steps.len() {
            run.now = i;
            let fills = run.fills();
            let black = [0.0, 0.0, 0.0, 1.0];
            run.ctx.ClearView(&run.rtv, &black, None);
            for (r, c) in &fills {
                let color = [c[0] as f32 / 1023.0, c[1] as f32 / 1023.0, c[2] as f32 / 1023.0, 1.0];
                run.ctx.ClearView(&run.rtv, &color, Some(&[*r]));
            }
            run.panel().unwrap();
            run.ctx.CopyResource(&staging, &run.back);
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            run.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map)).unwrap();
            let at = |x: i32, y: i32| *((map.pData as *const u8).add(y as usize * map.RowPitch as usize + x as usize * 4) as *const u32);
            let (cx, cy) = (run.size.0 / 2, run.size.1 / 2);
            let p = at(cx, cy);
            let want = fills.last().unwrap().1;
            let got = [(p & 1023) as i32, ((p >> 10) & 1023) as i32, ((p >> 20) & 1023) as i32];
            out += &format!("passo {i}: pedido {want:?} saiu {got:?} {}\n", want == got);
            run.ctx.Unmap(&staging, 0);
            let _ = run.swap.Present(1, DXGI_PRESENT(0));
        }
        let _ = DestroyWindow(hwnd);
    }
    out
}
