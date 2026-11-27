//! HDR no OpenGL, só em janela. O `wglSwapBuffers` ganha um salto para cá: o quadro do jogo sai do
//! buffer de trás do GL para uma textura do D3D11 dividida pelo `WGL_NV_DX_interop2` (NVIDIA, AMD e
//! Intel têm), vai para o buffer do jogo numa cadeia DXGI nossa e o proxy do D3D11 (`hdr.rs`) faz o
//! resto: a curva, a calibração, o Analisar e o medidor são os mesmos. A troca do GL nunca roda.
//! Janela sem HDR, MSAA no buffer do GL ou qualquer falha: o salto sai e o jogo segue como era.
//! shortcut: uma janela por vez; um jogo que apresente em duas ao mesmo tempo fica trocando entre elas.

use std::sync::{Mutex, MutexGuard};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC, WindowFromDC, HDC};
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

use crate::hdr::{self, INSIDE};
use crate::{export, Ptr, VirtualProtect};

const READ_FRAMEBUFFER: u32 = 0x8CA8;
const DRAW_FRAMEBUFFER: u32 = 0x8CA9;
const READ_FRAMEBUFFER_BINDING: u32 = 0x8CAA;
const DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const COLOR_ATTACHMENT0: u32 = 0x8CE0;
const FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;
const TEXTURE_2D: u32 = 0x0DE1;
const COLOR_BUFFER_BIT: u32 = 0x4000;
const NEAREST: u32 = 0x2600;
const SCISSOR_TEST: u32 = 0x0C11;
const FRAMEBUFFER_SRGB: u32 = 0x8DB9;
const READ_BUFFER: u32 = 0x0C02;
const BACK: u32 = 0x0405;
const SAMPLE_BUFFERS: u32 = 0x80A8;
const ACCESS_WRITE_DISCARD: u32 = 2;
const DRIVER_HARDWARE: u32 = 1;
const SDK_VERSION: u32 = 7;
const EXECUTE_READWRITE: u32 = 0x40;
/// `mov rax, alvo; jmp rax`: sem endereço relativo, outro gancho que copie a entrada (o da Steam) não se perde.
const JUMP: usize = 12;

type Swap = unsafe extern "system" fn(HDC) -> BOOL;
type CreateD3D11 = unsafe extern "system" fn(Ptr, u32, Ptr, u32, *const u32, u32, u32, *mut Ptr, *mut u32, *mut Ptr) -> HRESULT;

#[link(name = "kernel32")]
extern "system" {
    fn FlushInstructionCache(process: Ptr, at: Ptr, size: usize) -> i32;
    fn GetCurrentProcess() -> Ptr;
}

/// As funções do GL que o passe usa. As do GL 1.1 vêm do `opengl32.dll`; as outras, do driver.
#[derive(Clone, Copy)]
struct Gl {
    get_integer: unsafe extern "system" fn(u32, *mut i32),
    is_enabled: unsafe extern "system" fn(u32) -> u8,
    enable: unsafe extern "system" fn(u32),
    disable: unsafe extern "system" fn(u32),
    read_buffer: unsafe extern "system" fn(u32),
    gen_textures: unsafe extern "system" fn(i32, *mut u32),
    delete_textures: unsafe extern "system" fn(i32, *const u32),
    gen_framebuffers: unsafe extern "system" fn(i32, *mut u32),
    delete_framebuffers: unsafe extern "system" fn(i32, *const u32),
    bind_framebuffer: unsafe extern "system" fn(u32, u32),
    framebuffer_texture: unsafe extern "system" fn(u32, u32, u32, u32, i32),
    check_framebuffer: unsafe extern "system" fn(u32) -> u32,
    blit: unsafe extern "system" fn(i32, i32, i32, i32, i32, i32, i32, i32, u32, u32),
    open: unsafe extern "system" fn(Ptr) -> HANDLE,
    close: unsafe extern "system" fn(HANDLE) -> i32,
    register: unsafe extern "system" fn(HANDLE, Ptr, u32, u32, u32) -> HANDLE,
    unregister: unsafe extern "system" fn(HANDLE, HANDLE) -> i32,
    lock: unsafe extern "system" fn(HANDLE, i32, *mut HANDLE) -> i32,
    unlock: unsafe extern "system" fn(HANDLE, i32, *mut HANDLE) -> i32,
    interval: Option<unsafe extern "system" fn() -> i32>,
}

/// O endereço de uma função, do `opengl32.dll` ou do driver (pede o contexto do jogo ligado).
unsafe fn find(name: &[u8]) -> Ptr {
    let at = export("opengl32.dll", name);
    if !at.is_null() {
        return at;
    }
    let get = export("opengl32.dll", b"wglGetProcAddress\0");
    if get.is_null() {
        return get;
    }
    let get: unsafe extern "system" fn(*const u8) -> Ptr = std::mem::transmute(get);
    let at = get(name.as_ptr());
    // Alguns drivers devolvem 1, 2, 3 ou -1 para dizer que não têm.
    if (at as isize) < 4 { std::ptr::null_mut() } else { at }
}

impl Gl {
    unsafe fn load() -> Option<Self> {
        macro_rules! f {
            ($n:literal) => {{
                let at = find(concat!($n, "\0").as_bytes());
                if at.is_null() {
                    return None;
                }
                std::mem::transmute(at)
            }};
        }
        let interval = find(b"wglGetSwapIntervalEXT\0");
        Some(Self {
            get_integer: f!("glGetIntegerv"),
            is_enabled: f!("glIsEnabled"),
            enable: f!("glEnable"),
            disable: f!("glDisable"),
            read_buffer: f!("glReadBuffer"),
            gen_textures: f!("glGenTextures"),
            delete_textures: f!("glDeleteTextures"),
            gen_framebuffers: f!("glGenFramebuffers"),
            delete_framebuffers: f!("glDeleteFramebuffers"),
            bind_framebuffer: f!("glBindFramebuffer"),
            framebuffer_texture: f!("glFramebufferTexture2D"),
            check_framebuffer: f!("glCheckFramebufferStatus"),
            blit: f!("glBlitFramebuffer"),
            open: f!("wglDXOpenDeviceNV"),
            close: f!("wglDXCloseDeviceNV"),
            register: f!("wglDXRegisterObjectNV"),
            unregister: f!("wglDXUnregisterObjectNV"),
            lock: f!("wglDXLockObjectsNV"),
            unlock: f!("wglDXUnlockObjectsNV"),
            interval: (!interval.is_null()).then(|| std::mem::transmute(interval)),
        })
    }

    unsafe fn int(&self, name: u32) -> i32 {
        let mut v = 0;
        (self.get_integer)(name, &mut v);
        v
    }
}

/// A textura do D3D11 que o GL enxerga, presa a um framebuffer do GL.
struct Bridge {
    tex: ID3D11Texture2D,
    name: u32,
    obj: HANDLE,
    fbo: u32,
    width: u32,
    height: u32,
}

struct Window {
    hwnd: HWND,
    glrc: usize,
    gl: Gl,
    ctx: ID3D11DeviceContext,
    device: ID3D11Device,
    chain: IDXGISwapChain1,
    interop: HANDLE,
    bridge: Option<Bridge>,
}

// Os objetos COM só são usados com a trava pega.
unsafe impl Send for Window {}

static WINDOW: Mutex<Option<Window>> = Mutex::new(None);
/// O começo original do `wglSwapBuffers` e o endereço dele.
static ORIGINAL: Mutex<Option<(usize, [u8; JUMP])>> = Mutex::new(None);

fn window() -> MutexGuard<'static, Option<Window>> {
    WINDOW.lock().unwrap_or_else(|e| e.into_inner())
}

unsafe fn current() -> usize {
    let get = export("opengl32.dll", b"wglGetCurrentContext\0");
    if get.is_null() {
        return 0;
    }
    let get: unsafe extern "system" fn() -> Ptr = std::mem::transmute(get);
    get() as usize
}

/// O intervalo do GL vira o do DXGI; o adaptativo (negativo) espera um retraço.
fn sync_of(v: Option<i32>) -> u32 {
    match v {
        Some(0) => 0,
        Some(v) if v > 0 => v.min(4) as u32,
        _ => 1,
    }
}

impl Bridge {
    unsafe fn new(w: &Window, width: u32, height: u32) -> Result<Self> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            // O interop só registra textura com tipo.
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
            ..Default::default()
        };
        let mut tex = None;
        w.device.CreateTexture2D(&desc, None, Some(&mut tex))?;
        let tex = tex.ok_or(Error::from(E_FAIL))?;
        let gl = &w.gl;
        let mut name = 0;
        (gl.gen_textures)(1, &mut name);
        let obj = (gl.register)(w.interop, tex.as_raw(), name, TEXTURE_2D, ACCESS_WRITE_DISCARD);
        if obj.0.is_null() {
            let e = interop_error();
            (gl.delete_textures)(1, &name);
            return Err(e);
        }
        let mut fbo = 0;
        (gl.gen_framebuffers)(1, &mut fbo);
        let mut me = Self { tex, name, obj, fbo, width, height };
        // Fora do lock a textura não existe para o GL e o FBO dá FRAMEBUFFER_UNSUPPORTED.
        if (gl.lock)(w.interop, 1, &mut me.obj) == 0 {
            let e = interop_error();
            me.free(gl, w.interop);
            return Err(e);
        }
        let was = gl.int(DRAW_FRAMEBUFFER_BINDING) as u32;
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, fbo);
        (gl.framebuffer_texture)(DRAW_FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, name, 0);
        let status = (gl.check_framebuffer)(DRAW_FRAMEBUFFER);
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, was);
        (gl.unlock)(w.interop, 1, &mut me.obj);
        if status != FRAMEBUFFER_COMPLETE {
            me.free(gl, w.interop);
            // O código do GL fica no erro que o app mostra.
            return Err(HRESULT(0x8004_0000u32 as i32 | status as i32).into());
        }
        Ok(me)
    }

    unsafe fn free(&self, gl: &Gl, interop: HANDLE) {
        (gl.unregister)(interop, self.obj);
        (gl.delete_framebuffers)(1, &self.fbo);
        (gl.delete_textures)(1, &self.name);
    }
}

impl Window {
    /// `None`: esta janela fica sem HDR (tela sem HDR, MSAA, sem interop).
    unsafe fn new(hwnd: HWND, glrc: usize) -> Result<Option<Self>> {
        if !hdr::screen_hdr(hwnd) {
            return Ok(None);
        }
        let Some(gl) = Gl::load() else { return Ok(None) };
        // O GL não resolve MSAA e vira de ponta-cabeça na mesma cópia.
        let was = gl.int(DRAW_FRAMEBUFFER_BINDING) as u32;
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, 0);
        let samples = gl.int(SAMPLE_BUFFERS);
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, was);
        if samples > 0 {
            return Ok(None);
        }
        let create = export("d3d11.dll", b"D3D11CreateDevice\0");
        if create.is_null() {
            return Ok(None);
        }
        let create: CreateD3D11 = std::mem::transmute(create);
        let (mut device, mut ctx, mut level) = (std::ptr::null_mut(), std::ptr::null_mut(), 0);
        // shortcut: a placa padrão; num notebook com o GL na outra placa o interop falha e o jogo fica sem HDR.
        create(std::ptr::null_mut(), DRIVER_HARDWARE, std::ptr::null_mut(), 0, std::ptr::null(), 0, SDK_VERSION, &mut device, &mut level, &mut ctx).ok()?;
        let (device, ctx) = (ID3D11Device::from_raw(device), ID3D11DeviceContext::from_raw(ctx));
        let interop = (gl.open)(device.as_raw());
        if interop.0.is_null() {
            return Ok(None);
        }
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect)?;
        let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: rect.right.max(1) as u32,
            Height: rect.bottom.max(1) as u32,
            // O mesmo formato da ponte: o `CopyResource` não converte.
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ..Default::default()
        };
        // Pela fábrica, fora do `INSIDE`: o gancho dela faz a cadeia FP16 com o proxy do D3D11.
        let chain = match factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None) {
            Ok(c) if hdr::ours(c.as_raw()) => c,
            Ok(c) => {
                drop(c);
                (gl.close)(interop);
                return Ok(None);
            }
            Err(e) => {
                (gl.close)(interop);
                return Err(e);
            }
        };
        // O Alt+Enter é do jogo, não do DXGI.
        let _ = factory.MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER);
        Ok(Some(Self { hwnd, glrc, gl, ctx, device, chain, interop, bridge: None }))
    }

    /// Copia o buffer de trás do GL para a cadeia nossa e apresenta.
    unsafe fn frame(&mut self) -> Result<()> {
        let mut rect = RECT::default();
        GetClientRect(self.hwnd, &mut rect)?;
        let (width, height) = (rect.right as u32, rect.bottom as u32);
        // Minimizada: nada a mostrar.
        if width == 0 || height == 0 {
            return Ok(());
        }
        if self.bridge.as_ref().is_none_or(|b| (b.width, b.height) != (width, height)) {
            if let Some(b) = self.bridge.take() {
                b.free(&self.gl, self.interop);
            }
            let d = self.chain.GetDesc1()?;
            if (d.Width, d.Height) != (width, height) {
                self.chain.ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
            }
            self.bridge = Some(Bridge::new(self, width, height)?);
        }
        let (gl, b) = (&self.gl, self.bridge.as_mut().ok_or(Error::from(E_FAIL))?);
        if (gl.lock)(self.interop, 1, &mut b.obj) == 0 {
            return Err(interop_error());
        }
        let (read, draw) = (gl.int(READ_FRAMEBUFFER_BINDING) as u32, gl.int(DRAW_FRAMEBUFFER_BINDING) as u32);
        let (scissor, srgb) = ((gl.is_enabled)(SCISSOR_TEST) != 0, (gl.is_enabled)(FRAMEBUFFER_SRGB) != 0);
        (gl.bind_framebuffer)(READ_FRAMEBUFFER, 0);
        let buffer = gl.int(READ_BUFFER) as u32;
        (gl.read_buffer)(BACK);
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, b.fbo);
        (gl.disable)(SCISSOR_TEST);
        (gl.disable)(FRAMEBUFFER_SRGB);
        // A linha 0 do GL é a de baixo; a do D3D, a de cima.
        let (w, h) = (width as i32, height as i32);
        (gl.blit)(0, 0, w, h, 0, h, w, 0, COLOR_BUFFER_BIT, NEAREST);
        (gl.read_buffer)(buffer);
        (gl.bind_framebuffer)(READ_FRAMEBUFFER, read);
        (gl.bind_framebuffer)(DRAW_FRAMEBUFFER, draw);
        if scissor {
            (gl.enable)(SCISSOR_TEST);
        }
        if srgb {
            (gl.enable)(FRAMEBUFFER_SRGB);
        }
        (gl.unlock)(self.interop, 1, &mut b.obj);
        // O `GetBuffer` do gancho devolve o buffer do jogo no proxy.
        let game: ID3D11Texture2D = self.chain.GetBuffer(0)?;
        self.ctx.CopyResource(&game, &b.tex);
        Ok(())
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            // O GL só solta o que é dele com o contexto dele ligado.
            if current() == self.glrc {
                if let Some(b) = self.bridge.take() {
                    b.free(&self.gl, self.interop);
                }
                (self.gl.close)(self.interop);
            }
            hdr::forget(self.chain.as_raw());
        }
    }
}

/// O jogo trocou de contexto: o `Drop` só solta os objetos do GL com o contexto dono ligado. Liga o
/// velho por um instante e volta ao do jogo (um `wglMakeCurrent` que falha desliga o atual, então a
/// volta vem sempre). Sem conseguir (janela fechada, contexto apagado ou noutra thread), eles ficam.
unsafe fn drop_in_own_context(old: Window, glrc: usize) {
    let (make, get_dc) = (export("opengl32.dll", b"wglMakeCurrent\0"), export("opengl32.dll", b"wglGetCurrentDC\0"));
    if old.glrc == glrc || make.is_null() || get_dc.is_null() {
        return drop(old);
    }
    let make: unsafe extern "system" fn(HDC, Ptr) -> i32 = std::mem::transmute(make);
    let get_dc: unsafe extern "system" fn() -> HDC = std::mem::transmute(get_dc);
    let (cur_dc, hwnd) = (get_dc(), old.hwnd);
    let dc = GetDC(Some(hwnd));
    if !dc.is_invalid() {
        make(dc, old.glrc as Ptr);
    }
    drop(old);
    make(cur_dc, glrc as Ptr);
    if !dc.is_invalid() {
        ReleaseDC(Some(hwnd), dc);
    }
}

/// `Some(sync)`: o quadro foi para a cadeia nossa. `None`: esta janela segue sem HDR.
unsafe fn swap(hdc: HDC) -> Result<Option<u32>> {
    let (hwnd, glrc) = (WindowFromDC(hdc), current());
    if hwnd.is_invalid() || glrc == 0 {
        return Ok(None);
    }
    let mut slot = window();
    if slot.as_ref().is_none_or(|w| w.hwnd != hwnd || w.glrc != glrc) {
        if let Some(old) = slot.take() {
            drop_in_own_context(old, glrc);
        }
        let Some(w) = Window::new(hwnd, glrc)? else { return Ok(None) };
        *slot = Some(w);
    }
    let w = slot.as_mut().ok_or(Error::from(E_FAIL))?;
    w.frame()?;
    Ok(Some(sync_of(w.gl.interval.map(|f| f()))))
}

/// O interop deixa o motivo no `GetLastError`; sem ele, um erro genérico.
unsafe fn interop_error() -> Error {
    match GetLastError() {
        ERROR_SUCCESS => E_FAIL.into(),
        e => e.to_hresult().into(),
    }
}

unsafe fn write(at: usize, bytes: &[u8]) -> bool {
    let mut old = 0;
    if VirtualProtect(at as Ptr, bytes.len(), EXECUTE_READWRITE, &mut old) == 0 {
        return false;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), at as *mut u8, bytes.len());
    VirtualProtect(at as Ptr, bytes.len(), old, &mut old);
    FlushInstructionCache(GetCurrentProcess(), at as Ptr, bytes.len());
    true
}

/// Tira o salto de vez e devolve o endereço do `wglSwapBuffers` original.
unsafe fn give_up() -> usize {
    *window() = None;
    let mut original = ORIGINAL.lock().unwrap_or_else(|e| e.into_inner());
    match original.take() {
        Some((at, bytes)) => {
            write(at, &bytes);
            at
        }
        None => 0,
    }
}

unsafe extern "system" fn on_swap(hdc: HDC) -> BOOL {
    if !INSIDE.get() {
        match swap(hdc) {
            Ok(Some(sync)) => {
                let chain = window().as_ref().map(|w| w.chain.clone());
                // Passa pelo gancho do `Present` da cadeia: o passe HDR, o limitador e o medidor.
                if let Some(hr) = chain.map(|c| c.Present(sync, DXGI_PRESENT(0))).filter(|hr| hr.is_err()) {
                    hdr::note(hr);
                }
                return TRUE;
            }
            Ok(None) => {}
            Err(e) => hdr::note(e.code()),
        }
    }
    let at = give_up();
    if at == 0 {
        return FALSE;
    }
    let old: Swap = std::mem::transmute(at);
    old(hdc)
}

fn jump(to: usize) -> [u8; JUMP] {
    let mut b = [0u8; JUMP];
    b[..2].copy_from_slice(&[0x48, 0xB8]);
    b[2..10].copy_from_slice(&to.to_le_bytes());
    b[10..].copy_from_slice(&[0xFF, 0xE0]);
    b
}

/// O salto entra no começo do `wglSwapBuffers`. O `SwapBuffers` do GDI passa por ele.
/// shortcut: sem trampolim: a troca original só volta tirando o salto de vez (`give_up`).
pub unsafe fn install() -> bool {
    let at = export("opengl32.dll", b"wglSwapBuffers\0") as usize;
    if at == 0 {
        return false;
    }
    let mut original = ORIGINAL.lock().unwrap_or_else(|e| e.into_inner());
    if original.is_some() {
        return true;
    }
    let mut bytes = [0u8; JUMP];
    std::ptr::copy_nonoverlapping(at as *const u8, bytes.as_mut_ptr(), JUMP);
    if !write(at, &jump(on_swap as *const () as usize)) {
        return false;
    }
    *original = Some((at, bytes));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jump_is_mov_rax_jmp_rax() {
        let b = jump(0x1122_3344_5566_7788);
        assert_eq!(b, [0x48, 0xB8, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 0xFF, 0xE0]);
    }

    #[test]
    fn gl_interval_maps_to_dxgi() {
        assert_eq!(sync_of(None), 1);
        assert_eq!(sync_of(Some(0)), 0);
        assert_eq!(sync_of(Some(1)), 1);
        assert_eq!(sync_of(Some(-1)), 1);
        assert_eq!(sync_of(Some(9)), 4);
    }
}
