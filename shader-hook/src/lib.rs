//! Shaders do Prisma dentro do jogo (modo Injeção): a mesma cadeia do motor por cima, aplicada no back buffer
//! antes do Present, sem janela por cima. Entra como o limitador (`hook/`): `SetWindowsHookEx` só na linha da
//! janela do jogo, dispositivo falso para achar as tabelas de funções, e se prende até o jogo fechar.
//! - DXGI com D3D11: `Present`, `Present1`, `ResizeBuffers`. Com D3D10, a cadeia roda num D3D11 nosso no mesmo
//!   adaptador (`Bridge`). D3D12 só avisa a API (`api`) e passa.
//! - D3D9 e 9Ex: `Present`, `PresentEx`, `Reset`, `ResetEx`.
//! O motor (`prisma.exe --shader-profile`) conversa pela memória de `prisma_shader::link`: perfil, sinal de vida,
//! `compare`. O perfil é relido a cada 250 ms, como no motor por cima.

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::*};
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;

use librashader::runtime::d3d9::FilterChain as Chain9;
use librashader::runtime::{Size, Viewport};
use prisma_shader::chain::{self, Engine, Tracker};
use prisma_shader::link::*;
use prisma_shader::{blocks, rtv, say, texture, FullPass, Res, PS_COPY};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D10::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D9::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Memory::*;
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const SLOT_PRESENT: usize = 8;
const SLOT_RESIZE: usize = 13;
const SLOT_PRESENT1: usize = 22;
const SLOT_RESIZE1: usize = 39;
const SLOT_D9_RESET: usize = 16;
const SLOT_D9_PRESENT: usize = 17;
const SLOT_D9_PRESENT_EX: usize = 121;
const SLOT_D9_RESET_EX: usize = 132;
/// Erros seguidos no desenho até desligar o efeito (o jogo segue sem ele).
const MAX_ERRORS: u32 = 10;

type Present = unsafe extern "system" fn(*mut c_void, u32, u32) -> HRESULT;
type Present1 = unsafe extern "system" fn(*mut c_void, u32, u32, *const c_void) -> HRESULT;
type Resize = unsafe extern "system" fn(*mut c_void, u32, u32, u32, DXGI_FORMAT, u32) -> HRESULT;
type Resize1 = unsafe extern "system" fn(*mut c_void, u32, u32, u32, DXGI_FORMAT, u32, *const u32, *const *mut c_void) -> HRESULT;
type D9Present = unsafe extern "system" fn(*mut c_void, *const c_void, *const c_void, HWND, *const c_void) -> HRESULT;
type D9PresentEx = unsafe extern "system" fn(*mut c_void, *const c_void, *const c_void, HWND, *const c_void, u32) -> HRESULT;
type D9Reset = unsafe extern "system" fn(*mut c_void, *mut c_void) -> HRESULT;
type D9ResetEx = unsafe extern "system" fn(*mut c_void, *mut c_void, *mut c_void) -> HRESULT;

static STARTED: AtomicBool = AtomicBool::new(false);
static SHARED: AtomicUsize = AtomicUsize::new(0);
/// (tabela, entrada, função original): o D3D9 comum e o 9Ex podem ter tabelas diferentes.
static PATCHES: Mutex<Vec<(usize, usize, usize)>> = Mutex::new(Vec::new());
static DX11: Mutex<Option<Dx11>> = Mutex::new(None);
static D9: Mutex<Option<D9>> = Mutex::new(None);
static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);
/// Perfil (`seq` e data do arquivo) com que a cadeia não abriu: tentar de novo a cada quadro só enchia o log.
/// Outro perfil, ou o mesmo gravado de novo, tenta.
static FAIL: Mutex<Option<(u32, Option<std::time::SystemTime>)>> = Mutex::new(None);

fn fail_key(s: &Shared) -> (u32, Option<std::time::SystemTime>) {
    (s.seq.load(Acquire), std::fs::metadata(s.path()).and_then(|m| m.modified()).ok())
}

thread_local! {
    /// Já dentro de um gancho nesta linha: o nosso desenho não passa de novo pelo efeito.
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn log(s: &str) {
    if let Some(f) = lock(&LOG).as_mut() {
        let _ = writeln!(f, "{s}");
    }
}

fn shared() -> Option<&'static Shared> {
    let at = SHARED.load(Relaxed);
    (at != 0).then(|| unsafe { &*(at as *const Shared) })
}

/// O motor deu sinal há pouco: sem ele (fechou ou caiu), o jogo volta a ser mostrado como era.
fn alive(s: &Shared) -> bool {
    (unsafe { GetTickCount64() }) <= s.beat.load(Relaxed) + BEAT_MS
}

/// Só a janela do jogo que o motor achou recebe o efeito (não um lançador ou janela de ferramenta do mesmo exe).
fn is_game(s: &Shared, hwnd: HWND) -> bool {
    let want = s.hwnd.load(Relaxed);
    want == 0 || hwnd.0 as u64 == want || unsafe { GetAncestor(hwnd, GA_ROOT) }.0 as u64 == want
}

/// Roda `f` uma vez por chamada do jogo, sem repetir dentro do nosso próprio desenho. Um pânico fica aqui: o jogo
/// não cai por causa do efeito.
fn guard(f: impl FnOnce()) {
    if BUSY.get() {
        return;
    }
    BUSY.set(true);
    if std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err() {
        log("shader: erro interno no gancho (pânico), efeito desligado neste quadro");
    }
    BUSY.set(false);
}

/// A função original de `slot` na tabela de `this`.
unsafe fn orig(this: *mut c_void, slot: usize) -> usize {
    let table = *(this as *const usize);
    let list = lock(&PATCHES);
    list.iter().find(|p| p.0 == table && p.1 == slot).or_else(|| list.iter().find(|p| p.1 == slot)).map_or(0, |p| p.2)
}

/// Troca uma entrada da tabela de funções. O original fica guardado antes de a nova valer.
unsafe fn patch(table: *mut usize, slot: usize, new: usize) -> bool {
    let at = table.add(slot);
    if *at == new {
        return true;
    }
    let mut old = PAGE_PROTECTION_FLAGS(0);
    if VirtualProtect(at as *const c_void, 8, PAGE_READWRITE, &mut old).is_err() {
        return false;
    }
    lock(&PATCHES).push((table as usize, slot, *at));
    (*(at as *const AtomicUsize)).store(new, SeqCst);
    let _ = VirtualProtect(at as *const c_void, 8, old, &mut old);
    true
}

/// (função original, trampolim que a chama): desvios no código, para quando a tabela não serve.
static DETOURS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// Desvia a função em `at` para `new`, uma vez por função. O desvio fica até o jogo fechar (a DLL está presa).
unsafe fn detour(at: usize, new: usize) -> Res<()> {
    let mut list = lock(&DETOURS);
    if at == 0 || list.iter().any(|d| d.0 == at) {
        return Ok(());
    }
    let d = retour::RawDetour::new(at as *const (), new as *const ()).map_err(|e| e.to_string())?;
    d.enable().map_err(|e| e.to_string())?;
    list.push((at, d.trampoline() as *const () as usize));
    std::mem::forget(d);
    Ok(())
}

/// A original de uma função desviada: a tabela de `this` ainda aponta para ela, o trampolim a chama sem o desvio.
unsafe fn tramp(this: *mut c_void, slot: usize) -> usize {
    let at = *(*(this as *const *const usize)).add(slot);
    lock(&DETOURS).iter().find(|d| d.0 == at).map_or(at, |d| d.1)
}

unsafe fn vtable<T: Interface>(obj: &T) -> *mut usize {
    *(obj.as_raw() as *const *mut usize)
}

// ---------- DXGI / D3D11 ----------

unsafe extern "system" fn on_present(this: *mut c_void, sync: u32, flags: u32) -> HRESULT {
    guard(|| dx11_present(this, flags));
    let f: Present = std::mem::transmute(orig(this, SLOT_PRESENT));
    f(this, sync, flags)
}

unsafe extern "system" fn on_present1(this: *mut c_void, sync: u32, flags: u32, params: *const c_void) -> HRESULT {
    guard(|| dx11_present(this, flags));
    let f: Present1 = std::mem::transmute(orig(this, SLOT_PRESENT1));
    f(this, sync, flags, params)
}

/// O ResizeBuffers falha se sobrar referência ao back buffer: as nossas saem antes.
/// O ResizeBuffers falha com o back buffer ainda preso: o que é nosso dele sai antes.
fn drop_bb(this: *mut c_void) {
    guard(|| {
        if let Some(x) = lock(&DX11).as_mut().filter(|x| x.sc == this as usize) {
            x.bb = None;
        }
    });
}

unsafe extern "system" fn on_resize(this: *mut c_void, n: u32, w: u32, h: u32, fmt: DXGI_FORMAT, flags: u32) -> HRESULT {
    drop_bb(this);
    let f: Resize = std::mem::transmute(orig(this, SLOT_RESIZE));
    f(this, n, w, h, fmt, flags)
}

unsafe extern "system" fn on_resize1(this: *mut c_void, n: u32, w: u32, h: u32, fmt: DXGI_FORMAT, flags: u32, mask: *const u32, queue: *const *mut c_void) -> HRESULT {
    drop_bb(this);
    let f: Resize1 = std::mem::transmute(orig(this, SLOT_RESIZE1));
    f(this, n, w, h, fmt, flags, mask, queue)
}

unsafe fn dx11_present(this: *mut c_void, flags: u32) {
    let Some(s) = shared() else { return };
    if flags & DXGI_PRESENT_TEST.0 != 0 {
        return;
    }
    s.presents.fetch_add(1, Relaxed);
    let Some(sc) = IDXGISwapChain::from_raw_borrowed(&this) else { return };
    let Ok(desc) = sc.GetDesc() else { return };
    if !is_game(s, desc.OutputWindow) {
        return;
    }
    let d11 = sc.GetDevice::<ID3D11Device>().ok();
    if d11.is_none() && sc.GetDevice::<ID3D10Device>().is_err() {
        // DXGI sem D3D11 nem D3D10 por trás: D3D12. O motor lê `api` e volta para a janela por cima.
        if s.api.load(Relaxed) != API_DX12 {
            s.api.store(API_DX12, Relaxed);
            say!("shader: o jogo apresenta em dx12, a injeção não cobre");
        }
        return;
    }
    // Quem já caiu na ponte fica D3D10 (o D3D10 do Windows também atende como D3D11).
    if s.api.load(Relaxed) != API_DX10 {
        s.api.store(if d11.is_some() { API_DX11 } else { API_DX10 }, Relaxed);
    }
    if !alive(s) {
        return;
    }
    let mut g = lock(&DX11);
    if g.as_ref().is_some_and(|x| x.sc != this as usize) {
        say!("shader: outra swapchain, recomeçando");
        *g = None;
    }
    if g.is_none() {
        let key = fail_key(s);
        if *lock(&FAIL) == Some(key) {
            return;
        }
        // O D3D10 atende como D3D11 mas recusa o estado de contexto nosso: aí a cadeia vai pela ponte (`Bridge`).
        let made = match d11.map(|dev| Dx11::new(this as usize, dev, None, s)) {
            Some(Ok(x)) => Ok(x),
            first => match sc.GetDevice::<ID3D10Device>() {
                Ok(d10) => {
                    if let Some(Err(e)) = first {
                        say!("shader: D3D11 do jogo recusou ({e}), indo como D3D10");
                    }
                    s.api.store(API_DX10, Relaxed);
                    own_device(&d10).and_then(|dev| Dx11::new(this as usize, dev, Some(d10), s))
                }
                Err(_) => first.unwrap(),
            },
        };
        match made {
            Ok(x) => *g = Some(x),
            Err(e) => {
                *lock(&FAIL) = Some(key);
                say!("shader: D3D11 não abriu: {e}");
                return;
            }
        }
    }
    let x = g.as_mut().unwrap();
    if x.errors >= MAX_ERRORS {
        return;
    }
    match x.frame(sc, &desc, s) {
        Ok(()) => x.errors = 0,
        Err(e) => {
            x.errors += 1;
            x.bb = None;
            say!("shader: {e}");
            if x.errors >= MAX_ERRORS {
                say!("shader: {MAX_ERRORS} erros seguidos, efeito desligado até o jogo fechar");
            }
        }
    }
}

/// O que depende do back buffer: some no ResizeBuffers.
struct Bb {
    key: (u32, u32, DXGI_FORMAT, u32),
    /// Cópia do back buffer (o back buffer não tem leitura por shader).
    raw: (ID3D11Texture2D, ID3D11ShaderResourceView),
    /// Em BGRA, quando o back buffer é RGBA: o detector de blocos e a redução leem BGRA.
    bgra: Option<(ID3D11Texture2D, ID3D11ShaderResourceView, ID3D11RenderTargetView)>,
    out: ID3D11RenderTargetView,
    /// D3D10: entrada e saída compartilhadas com o dispositivo do jogo.
    bridge: Option<Bridge>,
}

/// D3D10 ↔ D3D11 nosso. Compartilhamento simples (o D3D10.0 não abre textura com chave): cada lado espera a GPU
/// terminar o seu (consulta de evento) antes de o outro ler.
struct Bridge {
    inp: (ID3D11Texture2D, ID3D10Texture2D),
    out: (ID3D11Texture2D, ID3D10Texture2D),
    q10: ID3D10Query,
    q11: ID3D11Query,
}

impl Bridge {
    unsafe fn new(dev: &ID3D11Device, d10: &ID3D10Device, w: u32, h: u32, fmt: DXGI_FORMAT) -> Res<(Self, ID3D11ShaderResourceView)> {
        let (inp, out) = (shared_tex(dev, d10, w, h, fmt)?, shared_tex(dev, d10, w, h, fmt)?);
        let mut v = None;
        dev.CreateShaderResourceView(&inp.0, None, Some(&mut v))?;
        let (mut q10, mut q11) = (None, None);
        d10.CreateQuery(&D3D10_QUERY_DESC { Query: D3D10_QUERY_EVENT, MiscFlags: 0 }, Some(&mut q10))?;
        dev.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut q11))?;
        Ok((Self { inp, out, q10: q10.ok_or("sem consulta")?, q11: q11.ok_or("sem consulta")? }, v.ok_or("sem vista")?))
    }
}

/// Textura compartilhada entre o D3D10 do jogo e o D3D11 nosso. Nasce no D3D10: ele não abre a que nasce no D3D11.
unsafe fn shared_tex(dev: &ID3D11Device, d10: &ID3D10Device, w: u32, h: u32, fmt: DXGI_FORMAT) -> Res<(ID3D11Texture2D, ID3D10Texture2D)> {
    let desc = D3D10_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D10_USAGE_DEFAULT,
        BindFlags: (D3D10_BIND_SHADER_RESOURCE.0 | D3D10_BIND_RENDER_TARGET.0) as u32,
        CPUAccessFlags: 0,
        MiscFlags: D3D10_RESOURCE_MISC_SHARED.0 as u32,
    };
    let t10 = d10.CreateTexture2D(&desc, None).map_err(|e| format!("textura {w}x{h} {fmt:?}: {e}"))?;
    let mut t11: Option<ID3D11Texture2D> = None;
    dev.OpenSharedResource(t10.cast::<IDXGIResource>()?.GetSharedHandle()?, &mut t11)?;
    Ok((t11.ok_or("sem textura aberta")?, t10))
}

/// D3D10: a cadeia (feita para D3D11) roda num dispositivo nosso no mesmo adaptador do jogo.
unsafe fn own_device(d10: &ID3D10Device) -> Res<ID3D11Device> {
    let adapter = d10.cast::<IDXGIDevice>()?.GetAdapter()?;
    let mut dev = None;
    D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut dev), None, None)?;
    say!("shader: D3D10, a cadeia roda num D3D11 próprio no mesmo adaptador");
    Ok(dev.ok_or("sem dispositivo D3D11")?)
}

/// Espera a GPU passar da consulta (100 ms no máximo: travar o jogo é pior que um quadro sem efeito).
fn finished(mut ready: impl FnMut() -> bool) -> Res<()> {
    let t = Instant::now();
    while !ready() {
        if t.elapsed().as_millis() >= 100 {
            return Err("a GPU não terminou a cópia em 100 ms".into());
        }
        std::thread::yield_now();
    }
    Ok(())
}

struct Dx11 {
    sc: usize,
    dev: ID3D11Device,
    /// O dispositivo do jogo, quando ele é D3D10 (o `dev` então é nosso).
    d10: Option<ID3D10Device>,
    ctx: ID3D11DeviceContext1,
    /// Estado do contexto só nosso: o do jogo fica guardado do jeito que estava (`SwapDeviceContextState`).
    state: ID3DDeviceContextState,
    eng: Engine,
    copy: FullPass,
    polled: Instant,
    seq: u32,
    bb: Option<Bb>,
    warned: bool,
    errors: u32,
}

impl Dx11 {
    unsafe fn new(sc: usize, dev: ID3D11Device, d10: Option<ID3D10Device>, s: &Shared) -> Res<Self> {
        let ctx: ID3D11DeviceContext1 = dev.GetImmediateContext()?.cast()?;
        let single = dev.GetCreationFlags() & D3D11_CREATE_DEVICE_SINGLETHREADED.0 as u32 != 0;
        let flags = if single { D3D11_1_CREATE_DEVICE_CONTEXT_STATE_SINGLETHREADED.0 as u32 } else { 0 };
        let mut state = None;
        dev.cast::<ID3D11Device1>()?.CreateDeviceContextState(flags, &[dev.GetFeatureLevel()], D3D11_SDK_VERSION, &ID3D11Device::IID, None, Some(&mut state))?;
        let state = state.ok_or("sem estado de contexto")?;
        let seq = s.seq.load(Acquire);
        // A cadeia carrega aqui (o jogo para um instante na primeira vez), já no estado nosso.
        let mut prev = None;
        ctx.SwapDeviceContextState(&state, Some(&mut prev));
        let made = Engine::open(&s.path(), &dev, false).and_then(|eng| Ok((eng, FullPass::new(&dev, PS_COPY)?)));
        ctx.ClearState();
        if let Some(p) = prev {
            ctx.SwapDeviceContextState(&p, None);
        }
        let (eng, copy) = made?;
        say!("shader: D3D11 pronto, perfil {}", eng.path.display());
        Ok(Self { sc, dev, d10, ctx, state, eng, copy, polled: Instant::now(), seq, bb: None, warned: false, errors: 0 })
    }

    unsafe fn frame(&mut self, sc: &IDXGISwapChain, desc: &DXGI_SWAP_CHAIN_DESC, s: &Shared) -> Res<()> {
        let mut prev = None;
        self.ctx.SwapDeviceContextState(&self.state, Some(&mut prev));
        let r = self.draw(sc, desc, s);
        // Nada nosso fica preso ao back buffer (o ResizeBuffers do jogo pede todas as referências soltas).
        self.ctx.ClearState();
        if let Some(p) = prev {
            self.ctx.SwapDeviceContextState(&p, None);
        }
        r
    }

    unsafe fn draw(&mut self, sc: &IDXGISwapChain, desc: &DXGI_SWAP_CHAIN_DESC, s: &Shared) -> Res<()> {
        if self.polled.elapsed().as_millis() >= 250 {
            self.polled = Instant::now();
            let seq = s.seq.load(Acquire);
            if seq != self.seq {
                self.seq = seq;
                self.eng.switch(&self.dev, &s.path());
                say!("shader: perfil {}", self.eng.path.display());
            } else {
                self.eng.poll(&self.dev);
            }
        }
        let p = &self.eng.profile;
        if !p.on || !p.inject || s.flags.load(Relaxed) & FLAG_COMPARE != 0 || !self.eng.active() {
            return Ok(());
        }
        let (w, h, fmt, samples) = (desc.BufferDesc.Width, desc.BufferDesc.Height, desc.BufferDesc.Format, desc.SampleDesc.Count);
        // Cor do back buffer: só 8 bits por canal. HDR (16 bits ou 10 bits) fica sem efeito.
        let (base, rgba) = match fmt {
            DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => (DXGI_FORMAT_B8G8R8A8_UNORM, false),
            DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => (DXGI_FORMAT_R8G8B8A8_UNORM, true),
            other => {
                if !self.warned {
                    self.warned = true;
                    let why = if matches!(other, DXGI_FORMAT_R16G16B16A16_FLOAT | DXGI_FORMAT_R10G10B10A2_UNORM) { "HDR" } else { "formato" };
                    say!("shader: back buffer em {other:?} ({why}), efeito desligado neste jogo");
                }
                return Ok(());
            }
        };
        let key = (w, h, fmt, samples);
        if self.bb.as_ref().is_none_or(|b| b.key != key) {
            self.bb = None;
            let (raw, out, bridge) = match &self.d10 {
                Some(d10) => {
                    let (br, v) = Bridge::new(&self.dev, d10, w, h, base)?;
                    ((br.inp.0.clone(), v), rtv(&self.dev, &br.out.0)?, Some(br))
                }
                None => (texture(&self.dev, w, h, base, false)?, rtv(&self.dev, &sc.GetBuffer::<ID3D11Texture2D>(0)?)?, None),
            };
            let bgra = if rgba {
                let (t, v) = texture(&self.dev, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, true)?;
                let r = rtv(&self.dev, &t)?;
                Some((t, v, r))
            } else {
                None
            };
            // shortcut: back buffer _SRGB sai com a saída em sRGB de novo (fica mais claro); raro em flip model.
            self.bb = Some(Bb { key, raw, bgra, out, bridge });
            say!("shader: back buffer {w}x{h} {fmt:?}, {samples} amostra(s)");
        }
        let b = self.bb.as_ref().unwrap();
        match (&b.bridge, &self.d10) {
            (Some(br), Some(d10)) => {
                // O jogo copia o quadro para `inp`, a cadeia desenha de `inp` em `out` e o jogo copia `out` de volta.
                let back: ID3D10Texture2D = sc.GetBuffer(0)?;
                if samples > 1 {
                    d10.ResolveSubresource(&br.inp.1, 0, &back, 0, base);
                } else {
                    d10.CopyResource(&br.inp.1, &back);
                }
                br.q10.End();
                d10.Flush();
                let q: &ID3D10Asynchronous = &br.q10;
                finished(|| (Interface::vtable(q).GetData)(q.as_raw(), std::ptr::null_mut(), 0, 0) == S_OK)?;
                Self::chain_pass(&mut self.eng, &self.copy, &self.dev, &self.ctx, b, w, h)?;
                self.ctx.End(&br.q11);
                self.ctx.Flush();
                let c: &ID3D11DeviceContext = &self.ctx;
                finished(|| (Interface::vtable(c).GetData)(c.as_raw(), br.q11.as_raw(), std::ptr::null_mut(), 0, 0) == S_OK)?;
                d10.CopyResource(&back, &br.out.1);
                s.drawn.fetch_add(1, Relaxed);
                return Ok(());
            }
            _ => {
                let back: ID3D11Texture2D = sc.GetBuffer(0)?;
                if samples > 1 {
                    self.ctx.ResolveSubresource(&b.raw.0, 0, &back, 0, base);
                } else {
                    self.ctx.CopyResource(&b.raw.0, &back);
                }
            }
        }
        Self::chain_pass(&mut self.eng, &self.copy, &self.dev, &self.ctx, b, w, h)?;
        s.drawn.fetch_add(1, Relaxed);
        Ok(())
    }

    /// Da cópia do back buffer (`raw`) até `out`, passando para BGRA quando precisa.
    unsafe fn chain_pass(eng: &mut Engine, copy: &FullPass, dev: &ID3D11Device, ctx: &ID3D11DeviceContext1, b: &Bb, w: u32, h: u32) -> Res<()> {
        let input = match &b.bgra {
            Some((t, v, r)) => {
                copy.draw(ctx, &b.raw.1, r, [0.0, 0.0, w as f32, h as f32], [0.0; 4])?;
                (t, v)
            }
            None => (&b.raw.0, &b.raw.1),
        };
        eng.render(dev, ctx, input, (w, h), &b.out, [0.0, 0.0, w as f32, h as f32])
    }
}

unsafe fn install_dxgi() -> Res<()> {
    let window = dummy_window()?;
    let desc = DXGI_SWAP_CHAIN_DESC {
        BufferDesc: DXGI_MODE_DESC { Width: 2, Height: 2, Format: DXGI_FORMAT_R8G8B8A8_UNORM, ..Default::default() },
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 1,
        OutputWindow: window,
        Windowed: TRUE,
        ..Default::default()
    };
    let (mut sc, mut dev) = (None, None);
    let mut made = Err(Error::empty());
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        made = D3D11CreateDeviceAndSwapChain(None, driver, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), None, D3D11_SDK_VERSION, Some(&desc), Some(&mut sc), Some(&mut dev), None, None);
        if made.is_ok() {
            break;
        }
    }
    let done = made.map_err(|e| e.to_string()).and_then(|_| {
        let sc: IDXGISwapChain = sc.ok_or("sem swapchain")?;
        let t = vtable(&sc);
        let mut ok = patch(t, SLOT_PRESENT, on_present as *const () as usize) && patch(t, SLOT_RESIZE, on_resize as *const () as usize);
        if let Ok(sc1) = sc.cast::<IDXGISwapChain1>() {
            ok &= patch(vtable(&sc1), SLOT_PRESENT1, on_present1 as *const () as usize);
        }
        // Só a tabela de quem é IDXGISwapChain3 chega à entrada 39.
        if let Ok(sc3) = sc.cast::<IDXGISwapChain3>() {
            ok &= patch(vtable(&sc3), SLOT_RESIZE1, on_resize1 as *const () as usize);
        }
        if ok { Ok(()) } else { Err("VirtualProtect".to_string()) }
    });
    drop(dev);
    let _ = DestroyWindow(window);
    Ok(done?)
}

// ---------- D3D9 ----------

unsafe extern "system" fn on_d9_present(this: *mut c_void, a: *const c_void, b: *const c_void, w: HWND, r: *const c_void) -> HRESULT {
    guard(|| d9_present(this, w, a as *const RECT));
    let f: D9Present = std::mem::transmute(tramp(this, SLOT_D9_PRESENT));
    f(this, a, b, w, r)
}

unsafe extern "system" fn on_d9_present_ex(this: *mut c_void, a: *const c_void, b: *const c_void, w: HWND, r: *const c_void, flags: u32) -> HRESULT {
    guard(|| d9_present(this, w, a as *const RECT));
    let f: D9PresentEx = std::mem::transmute(tramp(this, SLOT_D9_PRESENT_EX));
    f(this, a, b, w, r, flags)
}

/// O Reset do D3D9 só funciona sem nada nosso em D3DPOOL_DEFAULT: a cadeia inteira sai e volta no próximo Present.
unsafe extern "system" fn on_d9_reset(this: *mut c_void, pp: *mut c_void) -> HRESULT {
    guard(|| drop(lock(&D9).take()));
    let f: D9Reset = std::mem::transmute(tramp(this, SLOT_D9_RESET));
    f(this, pp)
}

unsafe extern "system" fn on_d9_reset_ex(this: *mut c_void, pp: *mut c_void, mode: *mut c_void) -> HRESULT {
    guard(|| drop(lock(&D9).take()));
    let f: D9ResetEx = std::mem::transmute(tramp(this, SLOT_D9_RESET_EX));
    f(this, pp, mode)
}

/// `src`: o pedaço do back buffer que o jogo apresenta (o Guilty Gear XX desenha num canto e mostra só ele).
unsafe fn d9_present(this: *mut c_void, over: HWND, src: *const RECT) {
    let Some(s) = shared() else { return };
    s.presents.fetch_add(1, Relaxed);
    let Some(dev) = IDirect3DDevice9::from_raw_borrowed(&this) else { return };
    let mut hwnd = over;
    if hwnd.is_invalid() {
        let mut pp = D3DPRESENT_PARAMETERS::default();
        if let Ok(sc) = dev.GetSwapChain(0) {
            let _ = sc.GetPresentParameters(&mut pp);
        }
        hwnd = pp.hDeviceWindow;
        if hwnd.is_invalid() {
            let mut cp = D3DDEVICE_CREATION_PARAMETERS::default();
            let _ = dev.GetCreationParameters(&mut cp);
            hwnd = cp.hFocusWindow;
        }
    }
    if !is_game(s, hwnd) {
        return;
    }
    s.api.store(API_D3D9, Relaxed);
    if !alive(s) {
        return;
    }
    let mut g = lock(&D9);
    if g.as_ref().is_some_and(|x| x.dev != this as usize) {
        *g = None;
    }
    if g.is_none() {
        let key = fail_key(s);
        if *lock(&FAIL) == Some(key) {
            return;
        }
        match D9::new(this as usize, dev, s) {
            Ok(x) => *g = Some(x),
            Err(e) => {
                *lock(&FAIL) = Some(key);
                say!("shader: D3D9 não abriu: {e}");
                return;
            }
        }
    }
    let x = g.as_mut().unwrap();
    if x.errors >= MAX_ERRORS {
        return;
    }
    match x.frame(dev, s, src.as_ref()) {
        Ok(()) => x.errors = 0,
        Err(e) => {
            x.errors += 1;
            say!("shader: {e}");
            if x.errors >= MAX_ERRORS {
                say!("shader: {MAX_ERRORS} erros seguidos, efeito desligado até o próximo Reset");
            }
        }
    }
}

struct D9 {
    dev: usize,
    t: Tracker,
    chain: Option<Chain9>,
    defaults: HashMap<String, f32>,
    polled: Instant,
    seq: u32,
    /// Cópia do back buffer (o librashader lê textura em D3DPOOL_DEFAULT).
    input: Option<((u32, u32, D3DFORMAT), IDirect3DTexture9, IDirect3DSurface9)>,
    small: Option<((u32, u32), IDirect3DTexture9, IDirect3DSurface9)>,
    detect: blocks::Detect,
    strips: Option<Strips>,
    sb: Option<IDirect3DStateBlock9>,
    frame: usize,
    warned: bool,
    errors: u32,
}

// Os objetos D3D9 só são usados na linha que apresenta, sempre debaixo do `Mutex`.
unsafe impl Send for D9 {}

impl D9 {
    unsafe fn new(dev_ptr: usize, dev: &IDirect3DDevice9, s: &Shared) -> Res<Self> {
        let seq = s.seq.load(Acquire);
        let mut x = Self { dev: dev_ptr, t: Tracker::open(&s.path())?, chain: None, defaults: HashMap::new(), polled: Instant::now(), seq, input: None, small: None, detect: Default::default(), strips: None, sb: None, frame: 0, warned: false, errors: 0 };
        x.build(dev);
        say!("shader: D3D9 pronto, perfil {}", x.t.path.display());
        Ok(x)
    }

    fn build(&mut self, dev: &IDirect3DDevice9) {
        self.chain = None;
        self.defaults.clear();
        let paths = self.t.on_paths();
        if paths.is_empty() {
            return;
        }
        let t = Instant::now();
        match chain::join(&paths).and_then(|p| unsafe { Chain9::load_from_preset(p, dev, None) }.map_err(Into::into)) {
            Ok(c) => {
                self.defaults = chain::defaults(&c);
                chain::apply(&c, &self.defaults, &self.t.profile);
                self.chain = Some(c);
                say!("shader: cadeia D3D9 de {} preset(s) carregada em {} ms", paths.len(), t.elapsed().as_millis());
            }
            Err(e) => {
                // O jeito que o jogo criou o dispositivo explica a maioria das recusas do D3D9.
                let (mut cp, mut caps) = (D3DDEVICE_CREATION_PARAMETERS::default(), D3DCAPS9::default());
                unsafe {
                    let _ = dev.GetCreationParameters(&mut cp);
                    let _ = dev.GetDeviceCaps(&mut caps);
                }
                say!("shader: {e} (dispositivo {:#x}, VS {:#x}, PS {:#x})", cp.BehaviorFlags, caps.VertexShaderVersion, caps.PixelShaderVersion)
            }
        }
    }

    unsafe fn frame(&mut self, dev: &IDirect3DDevice9, s: &Shared, src: Option<&RECT>) -> Res<()> {
        if self.polled.elapsed().as_millis() >= 250 {
            self.polled = Instant::now();
            let seq = s.seq.load(Acquire);
            if seq != self.seq {
                self.seq = seq;
                self.t.switch(&s.path());
                say!("shader: perfil {}", self.t.path.display());
            }
            let c = self.t.poll();
            if c.input {
                self.detect = Default::default();
            }
            if c.chain {
                self.build(dev);
            } else if let Some(ch) = self.chain.as_ref().filter(|_| c.params) {
                chain::apply(ch, &self.defaults, &self.t.profile);
            }
        }
        let p = &self.t.profile;
        if !p.on || !p.inject || s.flags.load(Relaxed) & FLAG_COMPARE != 0 || self.chain.is_none() {
            return Ok(());
        }
        // Tela cheia sem foco: o dispositivo está perdido até o Reset do jogo; desenhar só dá erro.
        if dev.TestCooperativeLevel().is_err() {
            return Ok(());
        }
        let back = dev.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO)?;
        let mut d = D3DSURFACE_DESC::default();
        back.GetDesc(&mut d)?;
        if matches!(d.Format, D3DFMT_A2R10G10B10 | D3DFMT_A2B10G10R10 | D3DFMT_A16B16G16R16F) {
            if !self.warned {
                self.warned = true;
                say!("shader: back buffer em {:?} (HDR), efeito desligado neste jogo", d.Format);
            }
            return Ok(());
        }
        let full = RECT { left: 0, top: 0, right: d.Width as i32, bottom: d.Height as i32 };
        let r = src.map_or(full, |r| RECT { left: r.left.max(0), top: r.top.max(0), right: r.right.min(full.right), bottom: r.bottom.min(full.bottom) });
        if r.right <= r.left || r.bottom <= r.top {
            return Ok(());
        }
        let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
        let key = (w, h, d.Format);
        if self.input.as_ref().is_none_or(|i| i.0 != key) {
            let (t, sf) = target(dev, w, h, d.Format)?;
            self.input = Some((key, t, sf));
            say!("shader: back buffer {}x{} {:?}, imagem {w}x{h} em {},{}", d.Width, d.Height, d.Format, r.left, r.top);
        }
        let (_, tex, surf) = self.input.as_ref().unwrap();
        dev.StretchRect(&back, &r, surf, std::ptr::null(), D3DTEXF_NONE)?;
        let mut input = tex;
        // `auto`: o mesmo detector de blocos do D3D11, com as faixas lidas pela CPU. 16 bits por pixel fica sem.
        if self.t.profile.input == "auto" && matches!(d.Format, D3DFMT_X8R8G8B8 | D3DFMT_A8R8G8B8) {
            d9_detect(&mut self.detect, &mut self.strips, dev, surf, w, h, d.Format)?;
        }
        // Redução por StretchRect de ponto.
        if let Some(k) = blocks::reduce_for(&self.t.profile.input, h, self.detect.found) {
            let size = blocks::reduced_size(w, h, k);
            if self.small.as_ref().is_none_or(|x| x.0 != size) {
                let (t, sf) = target(dev, size.0, size.1, d.Format)?;
                self.small = Some((size, t, sf));
                say!("shader: entrada {w}x{h} → {}x{}", size.0, size.1);
            }
            let (_, t, sf) = self.small.as_ref().unwrap();
            dev.StretchRect(surf, std::ptr::null(), sf, std::ptr::null(), D3DTEXF_POINT)?;
            input = t;
        }
        // O bloco de estado guarda o do jogo; alvo e profundidade ficam fora dele e voltam à mão.
        match &self.sb {
            Some(sb) => sb.Capture()?,
            None => self.sb = Some(dev.CreateStateBlock(D3DSBT_ALL)?),
        }
        let (rt, ds) = (dev.GetRenderTarget(0).ok(), dev.GetDepthStencilSurface().ok());
        let vp = Viewport { x: r.left as f32, y: r.top as f32, mvp: None, output: &back, size: Size::new(w, h) };
        let r = self.chain.as_mut().unwrap().frame(input, &vp, self.frame, None);
        let _ = self.sb.as_ref().unwrap().Apply();
        if let Some(rt) = &rt {
            let _ = dev.SetRenderTarget(0, rt);
        }
        let _ = dev.SetDepthStencilSurface(ds.as_ref());
        r?;
        self.frame += 1;
        s.drawn.fetch_add(1, Relaxed);
        Ok(())
    }
}

/// Faixas do meio para o detector de blocos: alvo na GPU e cópia legível na CPU, a horizontal e a vertical.
struct Strips {
    size: (u32, u32),
    s: u32,
    rt: [IDirect3DSurface9; 2],
    sys: [IDirect3DSurface9; 2],
    /// Copiadas num quadro, lidas no seguinte: a GPU já terminou e a leitura não espera.
    pending: bool,
}

unsafe fn d9_detect(det: &mut blocks::Detect, st: &mut Option<Strips>, dev: &IDirect3DDevice9, src: &IDirect3DSurface9, w: u32, h: u32, fmt: D3DFORMAT) -> Res<()> {
    if st.as_ref().is_none_or(|x| x.size != (w, h)) {
        *det = Default::default();
        let s = 32.min(w).min(h);
        let mk = |a: u32, b: u32| -> Res<(IDirect3DSurface9, IDirect3DSurface9)> {
            let (mut rt, mut sys) = (None, None);
            dev.CreateRenderTarget(a, b, fmt, D3DMULTISAMPLE_NONE, 0, false, &mut rt, std::ptr::null_mut())?;
            dev.CreateOffscreenPlainSurface(a, b, fmt, D3DPOOL_SYSTEMMEM, &mut sys, std::ptr::null_mut())?;
            Ok((rt.ok_or("sem faixa")?, sys.ok_or("sem faixa")?))
        };
        let ((hr, hs), (vr, vs)) = (mk(w, s)?, mk(s, h)?);
        *st = Some(Strips { size: (w, h), s, rt: [hr, vr], sys: [hs, vs], pending: false });
    }
    if !det.due() {
        return Ok(());
    }
    let x = st.as_mut().unwrap();
    let s = x.s;
    if x.pending {
        x.pending = false;
        let (a, b) = (read9(dev, &x.rt[0], &x.sys[0], w, s)?, read9(dev, &x.rt[1], &x.sys[1], s, h)?);
        det.vote(blocks::detect(&a, w as usize, &b, h as usize, s as usize));
        return Ok(());
    }
    let (y0, x0) = ((h - s) / 2, (w - s) / 2);
    dev.StretchRect(src, &RECT { left: 0, top: y0 as i32, right: w as i32, bottom: (y0 + s) as i32 }, &x.rt[0], std::ptr::null(), D3DTEXF_NONE)?;
    dev.StretchRect(src, &RECT { left: x0 as i32, top: 0, right: (x0 + s) as i32, bottom: h as i32 }, &x.rt[1], std::ptr::null(), D3DTEXF_NONE)?;
    x.pending = true;
    Ok(())
}

/// A faixa em pixels BGRA, sem o byte de alfa (no X8R8G8B8 ele vem com qualquer coisa).
unsafe fn read9(dev: &IDirect3DDevice9, rt: &IDirect3DSurface9, sys: &IDirect3DSurface9, w: u32, h: u32) -> Res<Vec<u32>> {
    dev.GetRenderTargetData(rt, sys)?;
    let mut l = D3DLOCKED_RECT::default();
    sys.LockRect(&mut l, std::ptr::null(), D3DLOCK_READONLY as u32)?;
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h as usize {
        let row = std::slice::from_raw_parts((l.pBits as *const u8).add(y * l.Pitch as usize) as *const u32, w as usize);
        out.extend(row.iter().map(|p| p & 0xFF_FFFF));
    }
    sys.UnlockRect()?;
    Ok(out)
}

unsafe fn target(dev: &IDirect3DDevice9, w: u32, h: u32, fmt: D3DFORMAT) -> Res<(IDirect3DTexture9, IDirect3DSurface9)> {
    let mut t = None;
    dev.CreateTexture(w, h, 1, D3DUSAGE_RENDERTARGET as u32, fmt, D3DPOOL_DEFAULT, &mut t, std::ptr::null_mut())?;
    let t = t.ok_or("sem textura")?;
    let sf = t.GetSurfaceLevel(0)?;
    Ok((t, sf))
}

unsafe fn install_d9() -> Res<()> {
    // A cadeia D3D9 chama o D3DX9_43 (DirectX de junho de 2010), carregado só aqui (/DELAYLOAD): nem todo PC tem.
    if LoadLibraryW(w!("D3DX9_43.dll")).is_err() {
        return Err("o D3D9 precisa do D3DX9_43.dll (DirectX de junho de 2010), que não está neste PC".into());
    }
    let window = dummy_window()?;
    let r = (|| -> Res<()> {
        let mut pp = D3DPRESENT_PARAMETERS { BackBufferWidth: 2, BackBufferHeight: 2, BackBufferCount: 1, SwapEffect: D3DSWAPEFFECT_DISCARD, hDeviceWindow: window, Windowed: TRUE, ..Default::default() };
        let mut made = 0;
        // Cada dispositivo D3D9 tem tabela própria (Windows 11): trocar a entrada do falso não chega ao do jogo.
        // O desvio vai no código da função, que é o mesmo; comum e 9Ex, e cada jeito de processar vértices.
        let soft = D3DCREATE_SOFTWARE_VERTEXPROCESSING | D3DCREATE_DISABLE_DRIVER_MANAGEMENT;
        let kinds = [soft, D3DCREATE_HARDWARE_VERTEXPROCESSING, D3DCREATE_HARDWARE_VERTEXPROCESSING | D3DCREATE_PUREDEVICE, D3DCREATE_MIXED_VERTEXPROCESSING];
        if let Some(d3d) = Direct3DCreate9(D3D_SDK_VERSION) {
            for b in kinds {
                let mut dev = None;
                if d3d.CreateDevice(D3DADAPTER_DEFAULT, D3DDEVTYPE_HAL, window, b as u32, &mut pp, &mut dev).is_err() {
                    continue;
                }
                let Some(dev) = dev else { continue };
                made += 1;
                let t = vtable(&dev);
                detour(*t.add(SLOT_D9_RESET), on_d9_reset as *const () as usize)?;
                detour(*t.add(SLOT_D9_PRESENT), on_d9_present as *const () as usize)?;
            }
        }
        let d3d = Direct3DCreate9Ex(D3D_SDK_VERSION)?;
        for b in kinds {
            let mut dev = None;
            if d3d.CreateDeviceEx(D3DADAPTER_DEFAULT, D3DDEVTYPE_HAL, window, b as u32, &mut pp, std::ptr::null_mut(), &mut dev).is_err() {
                continue;
            }
            let Some(dev) = dev else { continue };
            made += 1;
            let t = vtable(&dev);
            detour(*t.add(SLOT_D9_RESET), on_d9_reset as *const () as usize)?;
            detour(*t.add(SLOT_D9_PRESENT), on_d9_present as *const () as usize)?;
            detour(*t.add(SLOT_D9_PRESENT_EX), on_d9_present_ex as *const () as usize)?;
            detour(*t.add(SLOT_D9_RESET_EX), on_d9_reset_ex as *const () as usize)?;
        }
        say!("shader: D3D9, {made} tipo(s) de dispositivo, {} função(ões) desviada(s)", lock(&DETOURS).len());
        if made == 0 { Err("nenhum dispositivo D3D9".into()) } else { Ok(()) }
    })();
    let _ = DestroyWindow(window);
    r
}

unsafe fn dummy_window() -> Res<HWND> {
    Ok(CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), None, WINDOW_STYLE(0), 0, 0, 2, 2, None, None, None, None)?)
}

// ---------- entrada ----------

/// Log da DLL ao lado dos logs do motor: `shaders\logs\<perfil>-injecao.log`.
fn open_log(profile: &Path) {
    let Some(root) = profile.parent().and_then(Path::parent) else { return };
    let dir = root.join("logs");
    let _ = std::fs::create_dir_all(&dir);
    let stem = profile.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let path: PathBuf = dir.join(format!("{stem}-injecao.log"));
    *lock(&LOG) = std::fs::OpenOptions::new().create(true).append(true).open(path).ok();
}

unsafe extern "system" fn start(_: *mut c_void) -> u32 {
    let mut module = HMODULE::default();
    // O Windows solta o módulo quando o gancho sai: preso aqui, ele fica até o jogo fechar.
    let _ = GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_PIN | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, PCWSTR(start as *const u16), &mut module);
    let name = HSTRING::from(map_name(GetCurrentProcessId()));
    let Ok(map) = CreateFileMappingW(INVALID_HANDLE_VALUE, None, PAGE_READWRITE, 0, std::mem::size_of::<Shared>() as u32, &name) else { return 1 };
    let view = MapViewOfFile(map, FILE_MAP_ALL_ACCESS, 0, 0, 0);
    if view.Value.is_null() {
        return 1;
    }
    let s = &*(view.Value as *const Shared);
    open_log(&s.path());
    prisma_shader::set_log(log);
    s.state.store(STATE_LOADED, SeqCst);
    s.magic.store(MAGIC, SeqCst);
    SHARED.store(view.Value as usize, SeqCst);
    let (dxgi, d9) = (install_dxgi(), install_d9());
    say!("shader: injeção no pid {}: DXGI {:?}, D3D9 {:?}", GetCurrentProcessId(), dxgi, d9);
    s.state.store(if dxgi.is_ok() || d9.is_ok() { STATE_READY } else { STATE_FAILED }, SeqCst);
    0
}

/// O Windows chama isto na linha da janela do jogo. O trabalho vai para outra linha, para não segurar o jogo.
#[no_mangle]
pub unsafe extern "system" fn prisma_shader_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if !STARTED.swap(true, SeqCst) {
        if let Ok(t) = CreateThread(None, 0, Some(start), None, THREAD_CREATION_FLAGS(0), None) {
            let _ = CloseHandle(t);
        }
    }
    CallNextHookEx(None, code, w, l)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ponte do D3D10: a textura nasce no D3D10 do jogo e abre no D3D11 nosso.
    #[test]
    fn ponte_d3d10() {
        unsafe {
            let mut d10 = None;
            D3D10CreateDevice(None, D3D10_DRIVER_TYPE_HARDWARE, HMODULE::default(), 0, D3D10_SDK_VERSION, Some(&mut d10)).unwrap();
            let d10 = d10.unwrap();
            let dev = own_device(&d10).unwrap();
            // O D3D10.0 não tem BGRA: o back buffer dele é RGBA.
            Bridge::new(&dev, &d10, 720, 480, DXGI_FORMAT_R8G8B8A8_UNORM).unwrap();
        }
    }
}
