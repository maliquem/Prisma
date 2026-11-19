//! Prévia com imagem: `--shader-preview=<perfil.txt>,<scene|bars|imagem>`. Janela normal, redimensionável,
//! com a imagem pela mesma cadeia do jogo e o mesmo recarregamento ao vivo. Segurar o clique mostra sem efeito.

use std::path::Path;
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use prisma_shader::{chain::Engine, rtv, Res};

/// Cena 2D de 320×240 feita aqui: céu em faixas, sol, morros, chão de tijolos e um boneco.
fn scene() -> (u32, u32, Vec<u8>) {
    let (w, h) = (320usize, 240usize);
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let hill = 150.0 + 18.0 * (x as f32 / 28.0).sin() + 8.0 * (x as f32 / 11.0).cos();
            let (dx, dy) = (x as i32 - 250, y as i32 - 44);
            let c = if (200..224).contains(&y) && (72..88).contains(&x) && (y < 206 || (76..84).contains(&x)) {
                if y < 206 { 0xf2c094 } else { 0xd83a3a } // boneco: cabeça e corpo
            } else if y >= 208 {
                if y % 8 == 0 || (x + if (y / 8) % 2 == 0 { 0 } else { 8 }) % 16 == 0 { 0x3a2416 } else { 0xa0522d }
            } else if y as f32 > hill {
                if (x / 4 + y / 4) % 7 == 0 { 0x2e7d32 } else { 0x43a047 }
            } else if dx * dx + dy * dy < 18 * 18 {
                0xffe066
            } else {
                [0x1e3a8a, 0x2549a8, 0x2d5bc4, 0x3b6fd8, 0x4f86e6, 0x6aa0f0, 0x8ab8f5, 0xa9cdf8][(y / 19).min(7)]
            };
            px[(y * w + x) * 4..][..4].copy_from_slice(&[c as u8, (c >> 8) as u8, (c >> 16) as u8, 255]);
        }
    }
    (w as u32, h as u32, px)
}

/// Barras de cor e uma escada de cinzas, 320×240.
fn bars() -> (u32, u32, Vec<u8>) {
    let (w, h) = (320usize, 240usize);
    let top = [0xc0c0c0, 0xc0c000, 0x00c0c0, 0x00c000, 0xc000c0, 0xc00000, 0x0000c0];
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let c = if y < 180 { top[x * 7 / w] } else { (x * 8 / w) as u32 * 0x242424 + 0x0f0f0f };
            px[(y * w + x) * 4..][..4].copy_from_slice(&[c as u8, (c >> 8) as u8, (c >> 16) as u8, 255]);
        }
    }
    (w as u32, h as u32, px)
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    DefWindowProcW(h, m, w, l)
}

pub fn run(arg: &str) -> Res<()> {
    // O perfil sempre termina em .txt: a imagem depois da vírgula pode ter vírgulas no caminho.
    let cut = arg.to_ascii_lowercase().find(".txt,").ok_or("uso: --shader-preview=<perfil.txt>,<scene|bars|imagem>")? + 4;
    let (path, what) = (Path::new(&arg[..cut]), arg[cut + 1..].trim_matches('"'));
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let (iw, ih, px) = match what {
        "scene" => scene(),
        "bars" => bars(),
        f => crate::ui::icons::decode_png(&std::fs::read(f).map_err(|e| format!("{f}: {e}"))?).ok_or_else(|| format!("{f}: imagem que não abre"))?,
    };
    let d = crate::capture::d3d()?;
    let (img, srv) = super::texture(&d.dev, iw, ih, DXGI_FORMAT_B8G8R8A8_UNORM, false)?;
    unsafe { d.ctx.UpdateSubresource(&img, 0, None, px.as_ptr() as *const _, iw * 4, 0) };
    let mut eng = Engine::open(path, &d.dev, false)?;

    let hwnd = unsafe {
        crate::overlays::register(w!("PrismaShaderPreview"), wndproc);
        // Cerca de 3× a imagem, sem passar da tela.
        let k = (900.0 / iw.max(ih) as f32).clamp(1.0, 4.0);
        let mut r = RECT { left: 0, top: 0, right: (iw as f32 * k) as i32, bottom: (ih as f32 * k) as i32 };
        let _ = AdjustWindowRect(&mut r, WS_OVERLAPPEDWINDOW, false);
        let title = HSTRING::from(format!("Prisma · prévia · {}", path.file_stem().unwrap_or_default().to_string_lossy()));
        CreateWindowExW(WINDOW_EX_STYLE(0), w!("PrismaShaderPreview"), &title, WS_OVERLAPPEDWINDOW | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT, r.right - r.left, r.bottom - r.top, None, None, None, None)?
    };
    let factory: IDXGIFactory2 = unsafe { d.dev.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()? };
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
        ..Default::default()
    };
    let swap = unsafe { factory.CreateSwapChainForHwnd(&d.dev, hwnd, &desc, None, None)? };
    let (mut size, mut polled) = ((0u32, 0u32), Instant::now());
    loop {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return Ok(());
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if polled.elapsed().as_millis() >= 250 {
                polled = Instant::now();
                eng.poll(&d.dev);
            }
            let mut rc = RECT::default();
            GetClientRect(hwnd, &mut rc)?;
            let (cw, ch) = (rc.right.max(1) as u32, rc.bottom.max(1) as u32);
            if IsIconic(hwnd).as_bool() {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            if (cw, ch) != size {
                swap.ResizeBuffers(2, cw, ch, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SWAP_CHAIN_FLAG(0))?;
                size = (cw, ch);
            }
            // A imagem inteira no meio da janela, na proporção dela.
            let k = (cw as f32 / iw as f32).min(ch as f32 / ih as f32);
            let (vw, vh) = ((iw as f32 * k).round(), (ih as f32 * k).round());
            let vp = [((cw as f32 - vw) / 2.0).floor(), ((ch as f32 - vh) / 2.0).floor(), vw, vh];
            let out = rtv(&d.dev, &swap.GetBuffer::<ID3D11Texture2D>(0)?)?;
            d.ctx.ClearRenderTargetView(&out, &[0.0, 0.0, 0.0, 1.0]);
            let mut cur = POINT::default();
            let _ = GetCursorPos(&mut cur);
            let pressed = GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 && GetForegroundWindow() == hwnd && WindowFromPoint(cur) == hwnd;
            if pressed || !eng.profile.on {
                eng.bypass(&d.ctx, &srv, &out, vp)?;
            } else {
                eng.render(&d.dev, &d.ctx, (&img, &srv), (iw, ih), &out, vp)?;
            }
            swap.Present(1, DXGI_PRESENT(0)).ok()?;
        }
    }
}
