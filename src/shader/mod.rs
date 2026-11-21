//! Shaders do RetroArch (.slangp) por cima de um jogo, sem entrar nele: captura a janela do jogo, passa pela
//! cadeia do perfil (librashader) e mostra numa janela por cima que deixa os cliques passarem. Processos à parte,
//! que o app abre e fecha:
//! - `prisma.exe --shader-profile=<perfil.txt>`: espera o jogo do perfil, aplica e acompanha o arquivo.
//! - `prisma.exe --shader-preview=<perfil.txt>,<scene|bars|imagem>`: a mesma cadeia numa janela de prévia.
//! O formato do perfil e de `teclas.txt` mora em `shader_profile.rs`.

pub mod cost;
pub mod files;
mod inject;
mod menu;
mod preview;

pub use prisma_shader::chain::slangp;
use prisma_shader::{chain, ema, rtv, texture, FullPass, GpuTimer, Res};

use std::time::Instant;

use std::path::Path;

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

const HK_NEXT: i32 = 2;

pub fn run_args() -> bool {
    let arg = |k: &str| std::env::args().find_map(|a| a.strip_prefix(k).map(|v| v.trim_matches('"').to_string()));
    let r = match (arg("--shader-profile="), arg("--shader-preview=")) {
        (Some(p), _) => run(Path::new(&p)),
        (_, Some(a)) => preview::run(&a),
        _ => return false,
    };
    // Erro (anti-cheat, perfil ilegível) vai para a saída de erro, que o app manda para o log, e sai com 1.
    if let Err(e) = r {
        eprintln!("shader: {e}");
        std::process::exit(1);
    }
    true
}

/// Janela do jogo: a primeira janela visível do exe com esse nome.
fn find_window(target: &str) -> Option<HWND> {
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

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// Monitor em HDR: o quadro vem em scRGB (linear, 1.0 = 80 nits) e vira SDR com o branco do Windows, que é
/// o que os shaders do RetroArch esperam. A janela por cima é SDR e o DWM a mostra com o mesmo branco.
/// A captura em 8 bits de uma tela HDR sai com outra conversão do Windows, fria e lavada.
/// shortcut: o que passa do branco do SDR (brilho de jogo HDR) é cortado; para jogo HDR, saída em scRGB.
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

/// Tecla lida pelo estado (`GetAsyncKeyState`) a cada volta do laço: chega também nos jogos que bloqueiam os
/// atalhos do Windows (o Castlevania Advance Collection bloqueia). `repeat`: segurar repete, como as setas.
#[derive(Default)]
pub(crate) struct Press {
    down: bool,
    next: Option<Instant>,
}

impl Press {
    pub(crate) fn hit(&mut self, down: bool, repeat: bool) -> bool {
        let now = Instant::now();
        let was = std::mem::replace(&mut self.down, down);
        if !down {
            return false;
        }
        if !was {
            self.next = repeat.then(|| now + std::time::Duration::from_millis(400));
            return true;
        }
        match self.next {
            Some(t) if now >= t => {
                self.next = Some(now + std::time::Duration::from_millis(80));
                true
            }
            _ => false,
        }
    }
}

/// Atalho de `teclas.txt` apertado agora (para segurar: `compare`).
fn held(k: crate::config::Hotkey) -> bool {
    use crate::config::{MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};
    let down = |vk: u16| unsafe { GetAsyncKeyState(vk as i32) } < 0;
    let need = |m: u32, vk: VIRTUAL_KEY| k.mods & m == 0 || down(vk.0);
    k.vk != 0 && down(k.vk as u16) && need(MOD_CTRL, VK_CONTROL) && need(MOD_ALT, VK_MENU) && need(MOD_SHIFT, VK_SHIFT) && (k.mods & MOD_WIN == 0 || down(VK_LWIN.0) || down(VK_RWIN.0))
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

/// `toggle` e `menu` são do app (o menu também cria o perfil do jogo que não tem); aqui só o próximo perfil. O
/// RegisterHotKey só segura a tecla longe do jogo (onde o jogo deixa); quem age é o `Press`, que também pega nos
/// jogos que bloqueiam atalhos, inclusive o do menu. Atalho de outro programa: fica sem ele.
fn hold_keys(keys: &crate::shader_profile::Keys) {
    let k = keys.next;
    if k.vk != 0 && unsafe { RegisterHotKey(None, HK_NEXT, HOT_KEY_MODIFIERS(k.mods) | MOD_NOREPEAT, k.vk) }.is_err() {
        eprintln!("shader: {} já é de outro programa, sem esse atalho (próximo perfil)", k.label());
    }
}

fn free_keys() {
    unsafe {
        let _ = UnregisterHotKey(None, HK_NEXT);
    }
}

fn run(path: &Path) -> Res<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let prof = crate::shader_profile::Profile::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let stem = prof.exe.file_stem().map(|s| s.to_string_lossy().to_string()).filter(|s| !s.is_empty()).ok_or("perfil sem exe")?;
    eprintln!("esperando a janela de {stem}");
    let game = loop {
        if let Some(h) = find_window(&stem) {
            break h;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    let game_path = crate::sys::procs::image_path(crate::sys::procs::window_pid(game)).unwrap_or_default();
    // Decisão do estudo: jogo com anti-cheat não recebe shader, nem por cima. Sem o caminho não dá para saber.
    if game_path.is_empty() || crate::meter::anti_cheat(&game_path) {
        return Err(format!("jogo com anti-cheat, o shader não entra: {game_path}").into());
    }
    eprintln!("jogo: {game_path} ({})", crate::sys::procs::window_title(game));
    // Injeção: o efeito vai dentro do jogo; se não servir nele, o perfil volta para Por cima e segue aqui.
    // Sem os arquivos baixados (ou de outra versão), fica Por cima sem mexer no perfil: a página avisa e baixa.
    if prof.inject {
        match files::load() {
            Some(f) if inject::run(path, game, &f)? => return Ok(()),
            Some(_) => {}
            None => eprintln!("shader: o modo Injeção não está baixado para a versão {}; fica Por cima", files::VERSION),
        }
    }

    let mc = crate::capture::monitor_color(unsafe { windows::Win32::Graphics::Gdi::MonitorFromWindow(game, windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST) });
    eprintln!("monitor em HDR: {} (branco do SDR {} nits)", if mc.hdr { "sim" } else { "não" }, mc.white_nits);
    let d = crate::capture::d3d()?;
    let mut eng = chain::Engine::open(path, &d.dev, false)?;
    let keys = crate::shader_profile::Keys::load(&crate::shader_profile::keys_path());

    // Em HDR a captura vem em 16 bits por cor e passa pelo PS_TO_SDR antes da cadeia.
    let to_sdr = if mc.hdr { Some(FullPass::new(&d.dev, PS_TO_SDR)?) } else { None };
    let sdr_scale = [80.0 / mc.white_nits.max(80.0), 0.0, 0.0, 0.0];
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
    }
    hold_keys(&keys);
    let mut menu = menu::Menu::new();
    let mut cost = cost::CostMap::create(crate::sys::procs::window_pid(game));

    let mut timer = GpuTimer::new(&d.dev)?;
    // raw: o recorte da captura; src: a entrada da cadeia (a própria raw sem HDR).
    let mut raw: Option<(ID3D11Texture2D, ID3D11ShaderResourceView)> = None;
    let mut src: Option<(ID3D11Texture2D, ID3D11ShaderResourceView)> = None;
    let (mut size, mut pos, mut shown) = ((0u32, 0u32), (POINT { x: i32::MIN, y: 0 }, 0u32, 0u32), false);
    let (mut gpu, mut lat, mut frames, mut since, mut n) = (Vec::new(), Vec::new(), 0u32, Instant::now(), 0usize);
    let (mut polled, mut record) = (Instant::now(), None);
    let (mut fg, mut menu_key, mut next_key) = (false, Press::default(), Press::default());
    // Consumo para o menu: ms da cadeia na GPU e intervalo entre quadros, em média.
    let (mut load, mut presented) = ((0.0, 0.0), None::<Instant>);

    let result: Res<()> = (|| loop {
        unsafe {
            MsgWaitForMultipleObjects(Some(&[event]), false, if menu.open || fg { 16 } else { 250 }, QS_ALLINPUT);
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if !IsWindow(Some(game)).as_bool() {
                eprintln!("o jogo fechou");
                return Ok(());
            }
            if polled.elapsed().as_millis() >= 250 {
                polled = Instant::now();
                eng.poll(&d.dev);
                // record=1: a janela por cima entra na captura do monitor (gravar e transmitir com o shader).
                if record != Some(eng.profile.record) {
                    record = Some(eng.profile.record);
                    let _ = SetWindowDisplayAffinity(overlay, if eng.profile.record { WDA_NONE } else { WDA_EXCLUDEFROMCAPTURE });
                }
                // O medidor do app mostra o custo e pode pedir o menu (botão Shader).
                if cost.as_mut().is_some_and(|c| c.tick(shown, load.0, load.1, &eng.path)) {
                    menu.toggle();
                }
            }
            fg = GetForegroundWindow() == game && !IsIconic(game).as_bool();
            if menu_key.hit(fg && held(keys.menu), false) {
                menu.toggle();
            }
            if next_key.hit(fg && held(keys.next), false) {
                eng.next(&d.dev, 1);
            }
            menu.keys(fg, &mut eng, &d.dev);
            menu.pad(&mut eng, &d.dev);
            menu.load = Some(load);
            menu.update(fg, client_area(game).map(|c| c.0), &mut eng);
            // `compare` segurado esconde a janela por cima: aparece o jogo sem efeito.
            let visible = fg && eng.profile.on && eng.active() && !held(keys.compare);
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
                // Sem efeito na tela não há consumo: a medida recomeça quando ele volta.
                (load, presented) = ((0.0, 0.0), None);
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
                swap.ResizeBuffers(2, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(flags as i32))?;
                raw = Some(texture(&d.dev, w, h, raw_fmt, false)?);
                src = if mc.hdr { Some(texture(&d.dev, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, true)?) } else { raw.clone() };
                size = (w, h);
                eprintln!("tamanho {w}x{h}");
            }
            // Posição ou tamanho novos: a janela por cima acompanha a área do jogo, e o menu fica acima dela.
            if !shown || (at, w, h) != pos {
                SetWindowPos(overlay, Some(HWND_TOPMOST), at.x, at.y, w as i32, h as i32, SWP_NOACTIVATE | SWP_SHOWWINDOW)?;
                pos = (at, w, h);
                shown = true;
                menu.raise();
            }
            let ((raw_tex, raw_srv), (src_tex, srv)) = (raw.as_ref().unwrap(), src.as_ref().unwrap());
            let bx = D3D11_BOX { left: crop.left as u32, top: crop.top as u32, front: 0, right: crop.right as u32, bottom: crop.bottom as u32, back: 1 };
            d.ctx.CopySubresourceRegion(raw_tex, 0, 0, 0, 0, &tex, 0, Some(&bx));
            frame.Close()?;
            let out = rtv(&d.dev, &swap.GetBuffer::<ID3D11Texture2D>(0)?)?;
            if let Some(ms) = timer.begin(&d.ctx, n) {
                gpu.push(ms);
                load.0 = ema(load.0, ms);
            }
            if let Some(t) = &to_sdr {
                t.draw(&d.ctx, raw_srv, &rtv(&d.dev, src_tex)?, [0.0, 0.0, w as f32, h as f32], sdr_scale)?;
            }
            eng.render(&d.dev, &d.ctx, (src_tex, srv), (w, h), &out, [0.0, 0.0, w as f32, h as f32])?;
            timer.end(&d.ctx, n);
            let present = if tearing { DXGI_PRESENT_ALLOW_TEARING } else { DXGI_PRESENT(0) };
            swap.Present(0, present).ok()?;
            if let Some(t) = presented.replace(Instant::now()) {
                load.1 = ema(load.1, t.elapsed().as_secs_f64() * 1000.0);
            }
            lat.push((qpc_100ns() - got) as f64 / 10_000.0);
            n += 1;
            frames += 1;
            if since.elapsed().as_secs() >= 5 {
                // O formato da captura foi escolhido ao abrir: HDR ligado ou desligado no meio erra as cores.
                let now = crate::capture::monitor_color(windows::Win32::Graphics::Gdi::MonitorFromWindow(game, windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTONEAREST));
                if now.hdr != mc.hdr || now.white_nits != mc.white_nits {
                    return Err("o HDR do monitor mudou: o shader precisa abrir de novo".into());
                }
                let secs = since.elapsed().as_secs_f64();
                // No release só com PRISMA_SHADER_STATS: o log do app não ganha uma linha a cada 5 s.
                if cfg!(debug_assertions) || std::env::var_os("PRISMA_SHADER_STATS").is_some() {
                    eprintln!(
                        "{w}x{h} {:.0} fps | GPU da cadeia {:.2} ms (p95 {:.2}) | quadro recebido até o Present {:.1} ms (p95 {:.1})",
                        frames as f64 / secs,
                        pct(&mut gpu, 0.5),
                        pct(&mut gpu, 0.95),
                        pct(&mut lat, 0.5),
                        pct(&mut lat, 0.95)
                    );
                }
                (frames, since) = (0, Instant::now());
                gpu.clear();
                lat.clear();
            }
        }
    })();

    free_keys();
    unsafe {
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
    use librashader::presets::ShaderFeatures;
    use librashader::runtime::d3d11::FilterChain;
    use librashader::runtime::{FilterChainParameters, Size, Viewport};

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
        FullPass::new(&d.dev, PS_TO_SDR).unwrap().draw(&d.ctx, &srv, &rtv(&d.dev, &out).unwrap(), [0.0, 0.0, 4.0, 1.0], [80.0 / 280.0, 0.0, 0.0, 0.0]).unwrap();
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
