//! Shaders do RetroArch (.slangp) por cima de um jogo, sem entrar nele (Fase 0 do estudo de shaders,
//! caminho B): captura a janela do jogo, passa pelo librashader e mostra numa janela por cima que deixa
//! os cliques passarem. Processo à parte:
//! `prisma.exe --shader=<exe ou hwnd>,<preset.slangp>[,nome=valor...]`
//! Ctrl+Alt+S liga e desliga o shader; Ctrl+Alt+Q sai. A cada 5 s escreve FPS, GPU e atraso.

use std::time::Instant;

use librashader::presets::{get_parameter_meta, ShaderFeatures, ShaderPreset};
use librashader::runtime::d3d11::FilterChain;
use librashader::runtime::{FilterChainParameters, Size, Viewport};
use windows::core::*;
use windows::Graphics::Capture::*;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::*;
use windows::Win32::System::Performance::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

type Res<T> = std::result::Result<T, Box<dyn std::error::Error>>;

const HK_TOGGLE: i32 = 1;
const HK_QUIT: i32 = 2;

pub fn run_args() -> bool {
    let Some(arg) = std::env::args().find_map(|a| a.strip_prefix("--shader=").map(str::to_string)) else { return false };
    unsafe {
        // No release o exe não tem console: escreve no do terminal que chamou.
        let _ = windows::Win32::System::Console::AttachConsole(windows::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
    if let Err(e) = run(&arg) {
        eprintln!("shader: {e}");
    }
    true
}

/// Janela do jogo: um número é o hwnd; senão, a primeira janela visível do exe com esse nome.
fn find_window(target: &str) -> Option<HWND> {
    if let Ok(n) = target.parse::<usize>() {
        return Some(HWND(n as *mut _));
    }
    let want = target.trim_end_matches(".exe").to_ascii_lowercase();
    crate::sys::procs::top_windows().into_iter().find(|h| {
        crate::sys::procs::image_path(crate::sys::procs::window_pid(*h)).is_some_and(|p| {
            std::path::Path::new(&p).file_stem().is_some_and(|s| s.to_string_lossy().to_ascii_lowercase() == want)
        })
    })
}

/// Área cliente do jogo: posição na tela e o recorte dentro do quadro capturado (que inclui a moldura).
fn client_area(game: HWND) -> Option<(POINT, RECT)> {
    unsafe {
        let mut rc = RECT::default();
        GetClientRect(game, &mut rc).ok()?;
        let mut pt = POINT::default();
        ClientToScreen(game, &mut pt).ok().ok()?;
        let mut fb = RECT::default();
        DwmGetWindowAttribute(game, DWMWA_EXTENDED_FRAME_BOUNDS, &mut fb as *mut _ as *mut _, std::mem::size_of::<RECT>() as u32).ok()?;
        let (x, y) = ((pt.x - fb.left).max(0), (pt.y - fb.top).max(0));
        Some((pt, RECT { left: x, top: y, right: x + rc.right, bottom: y + rc.bottom }))
    }
}

fn qpc_100ns() -> i64 {
    let (mut t, mut f) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut t);
        let _ = QueryPerformanceFrequency(&mut f);
    }
    (t as i128 * 10_000_000 / f.max(1) as i128) as i64
}

/// Tempo de GPU do preset por consultas de carimbo: lidas alguns quadros depois, sem esperar a GPU.
struct GpuTimer {
    slots: Vec<[ID3D11Query; 3]>,
    used: Vec<bool>,
}

impl GpuTimer {
    fn new(dev: &ID3D11Device) -> Res<Self> {
        let make = |q| -> Res<ID3D11Query> {
            let mut out = None;
            unsafe { dev.CreateQuery(&D3D11_QUERY_DESC { Query: q, MiscFlags: 0 }, Some(&mut out))? };
            Ok(out.unwrap())
        };
        let mut slots = Vec::new();
        for _ in 0..4 {
            slots.push([make(D3D11_QUERY_TIMESTAMP_DISJOINT)?, make(D3D11_QUERY_TIMESTAMP)?, make(D3D11_QUERY_TIMESTAMP)?]);
        }
        Ok(Self { used: vec![false; slots.len()], slots })
    }

    /// Lê o resultado antigo da vaga (se pronto) e começa a medir nela.
    fn begin(&mut self, ctx: &ID3D11DeviceContext, n: usize) -> Option<f64> {
        let i = n % self.slots.len();
        let old = if self.used[i] { self.read(ctx, i) } else { None };
        let [dis, a, _] = &self.slots[i];
        unsafe {
            ctx.Begin(dis);
            ctx.End(a);
        }
        self.used[i] = true;
        old
    }

    fn end(&self, ctx: &ID3D11DeviceContext, n: usize) {
        let [dis, _, b] = &self.slots[n % self.slots.len()];
        unsafe {
            ctx.End(b);
            ctx.End(dis);
        }
    }

    // O GetData do windows-rs junta S_FALSE (ainda não pronto) com S_OK: a vtable devolve o HRESULT cru.
    fn get<T: Default>(ctx: &ID3D11DeviceContext, q: &ID3D11Query) -> Option<T> {
        let mut v = T::default();
        let hr = unsafe {
            (Interface::vtable(ctx).GetData)(Interface::as_raw(ctx), Interface::as_raw(q), &mut v as *mut T as *mut _, std::mem::size_of::<T>() as u32, D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32)
        };
        (hr == S_OK).then_some(v)
    }

    fn read(&self, ctx: &ID3D11DeviceContext, i: usize) -> Option<f64> {
        let [dis, a, b] = &self.slots[i];
        let d: D3D11_QUERY_DATA_TIMESTAMP_DISJOINT = Self::get(ctx, dis)?;
        let (t0, t1): (u64, u64) = (Self::get(ctx, a)?, Self::get(ctx, b)?);
        (!d.Disjoint.as_bool() && d.Frequency > 0).then(|| (t1.saturating_sub(t0)) as f64 * 1000.0 / d.Frequency as f64)
    }
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Textura com leitura pelo shader; `rt` também deixa desenhar nela.
fn texture(dev: &ID3D11Device, w: u32, h: u32, fmt: DXGI_FORMAT, rt: bool) -> Res<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | if rt { D3D11_BIND_RENDER_TARGET.0 } else { 0 }) as u32,
        ..Default::default()
    };
    unsafe {
        let mut tex = None;
        dev.CreateTexture2D(&desc, None, Some(&mut tex))?;
        let tex = tex.unwrap();
        let mut srv = None;
        dev.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
        Ok((tex, srv.unwrap()))
    }
}

/// Monitor em HDR: o quadro vem em scRGB (linear, 1.0 = 80 nits) e vira SDR com o branco do Windows, que é
/// o que os shaders do RetroArch esperam. A janela por cima é SDR e o DWM a mostra com o mesmo branco.
/// A captura em 8 bits de uma tela HDR sai com outra conversão do Windows, fria e lavada.
/// shortcut: o que passa do branco do SDR (brilho de jogo HDR) é cortado; para jogo HDR, saída em scRGB.
struct ToSdr {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    smp: ID3D11SamplerState,
    cb: ID3D11Buffer,
}

const PS_TO_SDR: &str = r#"
Texture2D tex : register(t0);
SamplerState smp : register(s0);
cbuffer C : register(b0) { float scale; };
float srgb(float v) {
    v = saturate(v);
    return v <= 0.0031308 ? 12.92 * v : 1.055 * pow(v, 1 / 2.4) - 0.055;
}
float4 main(float4 p : SV_Position, float2 t : T) : SV_Target {
    float3 c = tex.Sample(smp, t).rgb * scale;
    return float4(srgb(c.r), srgb(c.g), srgb(c.b), 1);
}
"#;

impl ToSdr {
    fn new(dev: &ID3D11Device, white_nits: f32) -> Res<Self> {
        use crate::capture::hdr::{compile, VS};
        unsafe {
            let (vb, pb) = (compile(VS, s!("vs_5_0"))?, compile(PS_TO_SDR, s!("ps_5_0"))?);
            let (mut vs, mut ps, mut smp, mut cb) = (None, None, None, None);
            dev.CreateVertexShader(&vb, None, Some(&mut vs))?;
            dev.CreatePixelShader(&pb, None, Some(&mut ps))?;
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_POINT,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            dev.CreateSamplerState(&sd, Some(&mut smp))?;
            let scale = [80.0 / white_nits.max(80.0), 0.0, 0.0, 0.0];
            let bd = D3D11_BUFFER_DESC { ByteWidth: 16, Usage: D3D11_USAGE_IMMUTABLE, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() };
            let init = D3D11_SUBRESOURCE_DATA { pSysMem: scale.as_ptr() as *const _, ..Default::default() };
            dev.CreateBuffer(&bd, Some(&init), Some(&mut cb))?;
            Ok(Self { vs: vs.unwrap(), ps: ps.unwrap(), smp: smp.unwrap(), cb: cb.unwrap() })
        }
    }

    fn draw(&self, dev: &ID3D11Device, ctx: &ID3D11DeviceContext, input: &ID3D11ShaderResourceView, out: &ID3D11Texture2D, w: u32, h: u32) -> Res<()> {
        unsafe {
            let mut rtv = None;
            dev.CreateRenderTargetView(out, None, Some(&mut rtv))?;
            ctx.OMSetRenderTargets(Some(&[rtv]), None);
            ctx.RSSetViewports(Some(&[D3D11_VIEWPORT { Width: w as f32, Height: h as f32, MaxDepth: 1.0, ..Default::default() }]));
            ctx.IASetPrimitiveTopology(windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(None);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetShaderResources(0, Some(&[Some(input.clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.smp.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            ctx.Draw(3, 0);
            // Solta a entrada e a saída: o librashader liga as dele em seguida.
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
        }
        Ok(())
    }
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    DefWindowProcW(h, m, w, l)
}

fn overlay_window() -> Res<HWND> {
    unsafe {
        let class = w!("PrismaShader");
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), lpszClassName: class, ..Default::default() };
        RegisterClassW(&wc);
        // LAYERED + TRANSPARENT: os cliques e o mouse passam para o jogo; NOREDIRECTIONBITMAP: só a swapchain desenha.
        let ex = WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOREDIRECTIONBITMAP;
        let hwnd = CreateWindowExW(ex, class, w!("Prisma shader"), WS_POPUP, 0, 0, 1, 1, None, None, None, None)?;
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
        Ok(hwnd)
    }
}

fn run(arg: &str) -> Res<()> {
    let mut parts = arg.split(',');
    let target = parts.next().unwrap_or_default();
    let preset_path = parts.next().ok_or("falta o preset: --shader=<exe ou hwnd>,<preset.slangp>")?.trim_matches('"').to_string();
    let sets: Vec<(String, f32)> = parts.filter_map(|p| p.split_once('=').and_then(|(k, v)| Some((k.trim().to_string(), v.trim().parse().ok()?)))).collect();

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let game = find_window(target).ok_or_else(|| format!("janela não achada: {target}"))?;
    let path = crate::sys::procs::image_path(crate::sys::procs::window_pid(game)).unwrap_or_default();
    // Decisão do estudo: jogo com anti-cheat não recebe shader, nem por cima.
    if crate::meter::anti_cheat(&path) {
        return Err(format!("jogo com anti-cheat, o shader não entra: {path}").into());
    }
    eprintln!("jogo: {path} ({})", crate::sys::procs::window_title(game));

    let mc = crate::capture::monitor_color(unsafe { windows::Win32::Graphics::Gdi::MonitorFromWindow(game, windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST) });
    eprintln!("monitor em HDR: {} (branco do SDR {} nits)", if mc.hdr { "sim" } else { "não" }, mc.white_nits);
    let d = crate::capture::d3d()?;
    let t = Instant::now();
    let mut chain = unsafe { FilterChain::load_from_path(&preset_path, ShaderFeatures::NONE, &d.dev, None)? };
    eprintln!("preset carregado em {} ms", t.elapsed().as_millis());
    if let Ok(meta) = ShaderPreset::try_parse(&preset_path, ShaderFeatures::NONE).map_err(|e| e.to_string()).and_then(|p| get_parameter_meta(&p).map(|m| m.collect::<Vec<_>>()).map_err(|e| e.to_string())) {
        for m in meta {
            eprintln!("  parâmetro {} = {} ({} a {}, passo {}) {}", m.id, m.initial, m.minimum, m.maximum, m.step, m.description);
        }
    }
    for (k, v) in &sets {
        match chain.parameters().set_parameter_value(k, *v) {
            Some(old) => eprintln!("  {k}: {old} -> {v}"),
            None => eprintln!("  {k}: parâmetro não existe neste preset"),
        }
    }

    // Em HDR a captura vem em 16 bits por cor e passa pelo ToSdr antes do preset.
    let to_sdr = if mc.hdr { Some(ToSdr::new(&d.dev, mc.white_nits)?) } else { None };
    let (fmt, raw_fmt) = if mc.hdr {
        (DirectXPixelFormat::R16G16B16A16Float, DXGI_FORMAT_R16G16B16A16_FLOAT)
    } else {
        (DirectXPixelFormat::B8G8R8A8UIntNormalized, DXGI_FORMAT_B8G8R8A8_UNORM)
    };
    let item = crate::capture::window_item(game)?;
    let mut pool_size = item.Size()?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d.rt, fmt, 2, pool_size)?;
    let event = unsafe { CreateEventW(None, false, false, None)? };
    let ev = event.0 as isize;
    let token = pool.FrameArrived(&windows::Foundation::TypedEventHandler::new(move |_, _| {
        unsafe { SetEvent(HANDLE(ev as *mut _))? };
        Ok(())
    }))?;
    let session = pool.CreateCaptureSession(&item)?;
    let _ = session.SetIsCursorCaptureEnabled(false);
    let _ = session.SetIsBorderRequired(false);
    session.StartCapture()?;

    let overlay = overlay_window()?;
    let factory: IDXGIFactory2 = unsafe { d.dev.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()? };
    let tearing = unsafe {
        let mut ok = BOOL(0);
        factory.cast::<IDXGIFactory5>().is_ok_and(|f| f.CheckFeatureSupport(DXGI_FEATURE_PRESENT_ALLOW_TEARING, &mut ok as *mut _ as *mut _, 4).is_ok()) && ok.as_bool()
    };
    let flags = if tearing { DXGI_SWAP_CHAIN_FLAG_ALLOW_TEARING.0 as u32 } else { 0 };
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: 1,
        Height: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        AlphaMode: DXGI_ALPHA_MODE_IGNORE,
        Flags: flags,
        ..Default::default()
    };
    let swap = unsafe { factory.CreateSwapChainForHwnd(&d.dev, overlay, &desc, None, None)? };
    unsafe {
        let _ = factory.MakeWindowAssociation(overlay, DXGI_MWA_NO_ALT_ENTER);
        // Tecla já usada por outro programa não impede o teste: só fica sem o atalho.
        for (id, key, what) in [(HK_TOGGLE, 'S', "ligar e desligar"), (HK_QUIT, 'Q', "sair")] {
            if RegisterHotKey(None, id, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, key as u32).is_err() {
                eprintln!("Ctrl+Alt+{key} ({what}) já é de outro programa: sem esse atalho");
            }
        }
    }

    let mut timer = GpuTimer::new(&d.dev)?;
    // raw: o recorte da captura; src: a entrada do preset (a própria raw sem HDR).
    let mut raw: Option<(ID3D11Texture2D, ID3D11ShaderResourceView)> = None;
    let mut src: Option<(ID3D11Texture2D, ID3D11ShaderResourceView)> = None;
    let mut rtv: Option<ID3D11RenderTargetView> = None;
    let (mut size, mut pos, mut shown, mut on) = ((0u32, 0u32), (POINT { x: i32::MIN, y: 0 }, 0u32, 0u32), false, true);
    let (mut gpu, mut lat, mut frames, mut since, mut n) = (Vec::new(), Vec::new(), 0u32, Instant::now(), 0usize);

    let result: Res<()> = (|| loop {
        unsafe {
            MsgWaitForMultipleObjects(Some(&[event]), false, 250, QS_ALLINPUT);
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_HOTKEY {
                    match msg.wParam.0 as i32 {
                        HK_QUIT => return Ok(()),
                        _ => on = !on,
                    }
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if !IsWindow(Some(game)).as_bool() {
                eprintln!("o jogo fechou");
                return Ok(());
            }
            let visible = on && GetForegroundWindow() == game && !IsIconic(game).as_bool();
            if !visible && shown {
                let _ = ShowWindow(overlay, SW_HIDE);
                shown = false;
            }
            let Ok(mut frame) = pool.TryGetNextFrame() else { continue };
            // O aviso de quadro novo não acumula: sem esvaziar a fila, um soluço deixava tudo um quadro atrasado.
            while let Ok(newer) = pool.TryGetNextFrame() {
                frame.Close()?;
                frame = newer;
            }
            // O SystemRelativeTime do quadro é o horário da composição seguinte do DWM (fica depois do
            // Present), não o da captura: o atraso medido aqui é o do Prisma, de receber o quadro até o Present.
            let got = qpc_100ns();
            let content = frame.ContentSize()?;
            if content != pool_size {
                pool.Recreate(&d.rt, fmt, 2, content)?;
                pool_size = content;
            }
            if !visible {
                frame.Close()?;
                continue;
            }
            let Some((at, crop)) = client_area(game) else { continue };
            let tex: ID3D11Texture2D = frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?.GetInterface()?;
            let mut td = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut td);
            // Logo depois de o jogo mudar de tamanho, a captura ainda entrega quadros do tamanho antigo: esses não servem.
            let fits = crop.right <= content.Width.min(td.Width as i32) && crop.bottom <= content.Height.min(td.Height as i32);
            let (w, h) = ((crop.right - crop.left).max(0) as u32, (crop.bottom - crop.top).max(0) as u32);
            if !fits || w == 0 || h == 0 {
                frame.Close()?;
                continue;
            }
            // Qualquer resolução: tamanho novo refaz a textura de entrada e a swapchain; o librashader acompanha a viewport.
            if (w, h) != size {
                rtv = None;
                swap.ResizeBuffers(2, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(flags as i32))?;
                raw = Some(texture(&d.dev, w, h, raw_fmt, false)?);
                src = if mc.hdr { Some(texture(&d.dev, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, true)?) } else { raw.clone() };
                size = (w, h);
                eprintln!("tamanho {w}x{h}");
            }
            // Posição ou tamanho novos: a janela por cima acompanha a área do jogo.
            if !shown || (at, w, h) != pos {
                SetWindowPos(overlay, Some(HWND_TOPMOST), at.x, at.y, w as i32, h as i32, SWP_NOACTIVATE | SWP_SHOWWINDOW)?;
                pos = (at, w, h);
                shown = true;
            }
            let ((raw_tex, raw_srv), (src_tex, srv)) = (raw.as_ref().unwrap(), src.as_ref().unwrap());
            let bx = D3D11_BOX { left: crop.left as u32, top: crop.top as u32, front: 0, right: crop.right as u32, bottom: crop.bottom as u32, back: 1 };
            d.ctx.CopySubresourceRegion(raw_tex, 0, 0, 0, 0, &tex, 0, Some(&bx));
            frame.Close()?;
            if rtv.is_none() {
                let back: ID3D11Texture2D = swap.GetBuffer(0)?;
                let mut v = None;
                d.dev.CreateRenderTargetView(&back, None, Some(&mut v))?;
                rtv = v;
            }
            let out = rtv.as_ref().unwrap();
            if let Some(ms) = timer.begin(&d.ctx, n) {
                gpu.push(ms);
            }
            if let Some(t) = &to_sdr {
                t.draw(&d.dev, &d.ctx, raw_srv, src_tex, w, h)?;
            }
            let vp = Viewport { x: 0.0, y: 0.0, mvp: None, output: out, size: Size::new(w, h) };
            chain.frame(None, srv, &vp, n, None)?;
            timer.end(&d.ctx, n);
            let present = if tearing { DXGI_PRESENT_ALLOW_TEARING } else { DXGI_PRESENT(0) };
            swap.Present(0, present).ok()?;
            lat.push((qpc_100ns() - got) as f64 / 10_000.0);
            n += 1;
            frames += 1;
            if since.elapsed().as_secs() >= 5 {
                // O formato da captura foi escolhido ao abrir: HDR ligado ou desligado no meio erra as cores.
                let now = crate::capture::monitor_color(windows::Win32::Graphics::Gdi::MonitorFromWindow(game, windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST));
                if now.hdr != mc.hdr || now.white_nits != mc.white_nits {
                    return Err("o HDR do monitor mudou: abra o teste de novo".into());
                }
                let secs = since.elapsed().as_secs_f64();
                eprintln!(
                    "{w}x{h} {:.0} fps | GPU do preset {:.2} ms (p95 {:.2}) | quadro recebido até o Present {:.1} ms (p95 {:.1})",
                    frames as f64 / secs,
                    pct(&mut gpu, 0.5),
                    pct(&mut gpu, 0.95),
                    pct(&mut lat, 0.5),
                    pct(&mut lat, 0.95)
                );
                (frames, since) = (0, Instant::now());
                gpu.clear();
                lat.clear();
            }
        }
    })();

    unsafe {
        let _ = UnregisterHotKey(None, HK_TOGGLE);
        let _ = UnregisterHotKey(None, HK_QUIT);
        let _ = pool.RemoveFrameArrived(token);
        let _ = session.Close();
        let _ = pool.Close();
        let _ = DestroyWindow(overlay);
        let _ = CloseHandle(event);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HDR: o branco do SDR (280 nits = 3.5 em scRGB) vira 255; metade da luz vira ~188 (curva sRGB);
    /// acima do branco é cortado; vermelho e azul não trocam.
    #[test]
    fn hdr_para_sdr() {
        let d = crate::capture::d3d().unwrap();
        let px: [f32; 16] = [3.5, 3.5, 3.5, 1.0, 1.75, 1.75, 1.75, 1.0, 7.0, 7.0, 7.0, 1.0, 3.5, 0.0, 0.0, 1.0];
        let (_, srv) = unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: 4, Height: 1, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..Default::default()
            };
            let init = D3D11_SUBRESOURCE_DATA { pSysMem: px.as_ptr() as *const _, SysMemPitch: 64, ..Default::default() };
            let mut tex = None;
            d.dev.CreateTexture2D(&desc, Some(&init), Some(&mut tex)).unwrap();
            let tex = tex.unwrap();
            let mut srv = None;
            d.dev.CreateShaderResourceView(&tex, None, Some(&mut srv)).unwrap();
            (tex, srv.unwrap())
        };
        let (out, _) = texture(&d.dev, 4, 1, DXGI_FORMAT_B8G8R8A8_UNORM, true).unwrap();
        ToSdr::new(&d.dev, 280.0).unwrap().draw(&d.dev, &d.ctx, &srv, &out, 4, 1).unwrap();
        let got = unsafe {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            out.GetDesc(&mut desc);
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut st = None;
            d.dev.CreateTexture2D(&desc, None, Some(&mut st)).unwrap();
            let st = st.unwrap();
            d.ctx.CopyResource(&st, &out);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            d.ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m)).unwrap();
            let v = std::slice::from_raw_parts(m.pData as *const u8, 16).to_vec();
            d.ctx.Unmap(&st, 0);
            v
        };
        eprintln!("BGRA {got:?}");
        assert_eq!(&got[0..3], &[255, 255, 255]);
        assert!((186..=190).contains(&got[4]), "meio brilho: {}", got[4]);
        assert_eq!(&got[8..11], &[255, 255, 255]);
        assert_eq!(&got[12..15], &[0, 0, 255], "vermelho puro em BGRA");
    }

    /// Uma imagem de cor conhecida passa pelo preset e volta: confere a ordem das cores (BGRA) e as linhas.
    #[test]
    fn cores_e_linhas() {
        let dir = std::env::temp_dir().join("prisma_shader_teste");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("p.slang"), r#"#version 450
layout(push_constant) uniform Push { vec4 OutputSize; float LINES; } params;
#pragma parameter LINES "linhas" 0.5 0.0 1.0 0.05
layout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; } global;
#pragma stage vertex
layout(location = 0) in vec4 Position;
layout(location = 1) in vec2 TexCoord;
layout(location = 0) out vec2 vTexCoord;
void main() { gl_Position = global.MVP * Position; vTexCoord = TexCoord; }
#pragma stage fragment
layout(location = 0) in vec2 vTexCoord;
layout(location = 0) out vec4 FragColor;
layout(set = 0, binding = 2) uniform sampler2D Source;
void main() { float l = mod(floor(vTexCoord.y * params.OutputSize.y), 2.0); FragColor = vec4(texture(Source, vTexCoord).rgb * (1.0 - params.LINES * l), 1.0); }
"#).unwrap();
        std::fs::write(dir.join("p.slangp"), "shaders = 1\nshader0 = p.slang\nfilter_linear0 = false\nscale_type0 = viewport\n").unwrap();

        let d = crate::capture::d3d().unwrap();
        let (w, h) = (8u32, 8u32);
        // Laranja quente em BGRA: R=240, G=120, B=20.
        let px: Vec<u8> = (0..w * h).flat_map(|_| [20u8, 120, 240, 255]).collect();
        let (src, srv) = texture(&d.dev, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, false).unwrap();
        unsafe {
            d.ctx.UpdateSubresource(&src, 0, None, px.as_ptr() as *const _, w * 4, 0);
            let mut chain = FilterChain::load_from_path(dir.join("p.slangp"), ShaderFeatures::NONE, &d.dev, None).unwrap();
            chain.parameters().set_parameter_value("LINES", 0.5);
            let mut desc = D3D11_TEXTURE2D_DESC {
                Width: w, Height: h, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default()
            };
            let mut out = None;
            d.dev.CreateTexture2D(&desc, None, Some(&mut out)).unwrap();
            let out = out.unwrap();
            let mut rtv = None;
            d.dev.CreateRenderTargetView(&out, None, Some(&mut rtv)).unwrap();
            let rtv = rtv.unwrap();
            let vp = Viewport { x: 0.0, y: 0.0, mvp: None, output: &rtv, size: Size::new(w, h) };
            chain.frame(None, &srv, &vp, 0, None).unwrap();
            desc.Usage = D3D11_USAGE_STAGING;
            desc.BindFlags = 0;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut st = None;
            d.dev.CreateTexture2D(&desc, None, Some(&mut st)).unwrap();
            let st = st.unwrap();
            d.ctx.CopyResource(&st, &out);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            d.ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m)).unwrap();
            let row = |y: usize| std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), 4).to_vec();
            let (a, b) = (row(0), row(1));
            d.ctx.Unmap(&st, 0);
            eprintln!("linha 0 BGRA {a:?}, linha 1 BGRA {b:?}");
            assert!(a[2] > 230 && a[0] < 30, "vermelho e azul trocados: {a:?}");
            assert!(b[2] < 130 && b[2] > 110, "linha ímpar sem escurecer: {b:?}");
        }
    }
}
