//! Shaders do Prisma dentro do jogo (modo Injeção): a mesma cadeia do motor por cima, aplicada no back buffer
//! antes do Present, sem janela por cima. Entra como o limitador (`hook/`): `SetWindowsHookEx` só na linha da
//! janela do jogo, dispositivo falso para achar as tabelas de funções, e se prende até o jogo fechar.
//! - DXGI com D3D11: `Present`, `Present1`, `ResizeBuffers`. Com D3D10, a cadeia roda num D3D11 nosso no mesmo
//!   adaptador (`Bridge`). Com D3D12, num D3D11 sobre a fila do jogo (11on12).
//! - D3D9 e 9Ex: `Present`, `PresentEx`, `Reset`, `ResetEx`.
//! - OpenGL: `wglSwapBuffers`, com a cadeia num contexto GL nosso no mesmo HDC (`Gl`).
//! - Vulkan: `vkQueuePresentKHR`, `vkCreateSwapchainKHR` e as de destruir, desviadas no código do driver, com a
//!   cadeia gravada num comando nosso na fila do jogo (`VkDev`).
//! O motor (`prisma.exe --shader-profile`) conversa pela memória de `prisma_shader::link`: perfil, sinal de vida,
//! `compare`. O perfil é relido a cada 250 ms, como no motor por cima.

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use ash::vk;
use glow::HasContext;

use librashader::runtime::d3d9::FilterChain as Chain9;
use librashader::runtime::gl::{FilterChain as ChainGl, GLImage as ChainGlImage};
use librashader::runtime::vk::{FilterChain as ChainVk, VulkanImage as ChainVkImage};
use librashader::runtime::{Size, Viewport};
use prisma_shader::chain::{self, Engine, Tracker};
use prisma_shader::link::*;
use prisma_shader::{blocks, ema, rtv, say, texture, FullPass, GpuTimer, Res, PS_COPY};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D10::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Direct3D11on12::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Direct3D9::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC, WindowFromDC, HDC, WGLSWAP};
use windows::Win32::Graphics::OpenGL::*;
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
/// (tabela, entrada, função original): o D3D9 comum e o 9Ex podem ter tabelas diferentes. Tabela 0: o código da
/// função foi desviado e a original é o trampolim, valendo para qualquer tabela.
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

/// Chama a original com `BUSY` ligado: um Present feito dentro dela (o driver por baixo de outra API) não passa de
/// novo pelo efeito.
fn inner<R>(f: impl FnOnce() -> R) -> R {
    let was = BUSY.replace(true);
    let r = f();
    BUSY.set(was);
    r
}

/// A função original de `slot` na tabela de `this`.
unsafe fn orig(this: *mut c_void, slot: usize) -> usize {
    let table = *(this as *const usize);
    let list = lock(&PATCHES);
    let any = |t: usize| list.iter().find(|p| p.0 == t && p.1 == slot);
    any(0).or_else(|| any(table)).or_else(|| list.iter().find(|p| p.1 == slot)).map_or(0, |p| p.2)
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

/// O DXGI é desviado no código da função (como o D3D9): com a tabela trocada, um gancho que a troque de novo por
/// cima do nosso (o overlay da Steam no Halls of Torment) chama a gente e a gente a ele, até estourar a pilha.
/// Se a tabela já não aponta para o dxgi.dll (alguém a trocou antes), fica a troca na tabela.
unsafe fn hook_dxgi(table: *mut usize, slot: usize, new: usize) -> bool {
    let at = *table.add(slot);
    let mut m = HMODULE::default();
    let flags = GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT | GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS;
    let inside = GetModuleHandleExW(flags, PCWSTR(at as *const u16), &mut m).is_ok() && GetModuleHandleW(w!("dxgi.dll")).is_ok_and(|d| d == m);
    if inside {
        if detour_with(at, new, |t| lock(&PATCHES).push((0, slot, t))).is_ok() {
            return true;
        }
        // O trampolim anotado antes de o desvio falhar já não existe.
        lock(&PATCHES).retain(|p| !(p.0 == 0 && p.1 == slot));
    }
    patch(table, slot, new)
}

/// (função original, trampolim que a chama): desvios no código, para quando a tabela não serve.
static DETOURS: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

/// Desvia a função em `at` para `new`, uma vez por função; o desvio fica até o jogo fechar (a DLL está presa).
/// `before` recebe o trampolim antes de o desvio ligar: o gancho pode ser chamado já na instrução seguinte.
unsafe fn detour_with(at: usize, new: usize, before: impl FnOnce(usize)) -> Res<()> {
    let mut list = lock(&DETOURS);
    if let Some(d) = list.iter().find(|d| d.0 == at) {
        before(d.1);
        return Ok(());
    }
    if at == 0 {
        return Ok(());
    }
    let d = retour::RawDetour::new(at as *const (), new as *const ()).map_err(|e| e.to_string())?;
    before(d.trampoline() as *const () as usize);
    list.reserve(1);
    enable(&d, at)?;
    list.push((at, d.trampoline() as *const () as usize));
    std::mem::forget(d);
    Ok(())
}

/// Liga o desvio com as outras linhas do jogo paradas e nenhuma delas nos bytes trocados (uma linha ali no meio
/// executaria metade da instrução velha e metade do salto). Com as linhas paradas nada aloca: uma delas pode estar
/// com a trava do heap.
unsafe fn enable(d: &retour::RawDetour, at: usize) -> Res<()> {
    use windows::Win32::System::Diagnostics::Debug::*;
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    let (me, pid) = (GetCurrentThreadId(), GetCurrentProcessId());
    let mut held: Vec<HANDLE> = Vec::with_capacity(4096);
    for _ in 0..100 {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?;
        let mut te = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
        let mut inside = false;
        let mut more = Thread32First(snap, &mut te).is_ok();
        while more {
            if te.th32OwnerProcessID == pid && te.th32ThreadID != me && held.len() < held.capacity() {
                if let Ok(t) = OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT, false, te.th32ThreadID) {
                    if SuspendThread(t) == u32::MAX {
                        let _ = CloseHandle(t);
                    } else {
                        let mut c = CONTEXT::default();
                        #[cfg(target_arch = "x86_64")]
                        let ip = {
                            c.ContextFlags = CONTEXT_CONTROL_AMD64;
                            GetThreadContext(t, &mut c).map(|_| c.Rip as usize)
                        };
                        #[cfg(target_arch = "x86")]
                        let ip = {
                            c.ContextFlags = CONTEXT_CONTROL_X86;
                            GetThreadContext(t, &mut c).map(|_| c.Eip as usize)
                        };
                        inside |= ip.is_ok_and(|ip| (at..at + 16).contains(&ip));
                        held.push(t);
                    }
                }
            }
            more = Thread32Next(snap, &mut te).is_ok();
        }
        let r = (!inside).then(|| d.enable());
        for t in held.drain(..) {
            ResumeThread(t);
            let _ = CloseHandle(t);
        }
        let _ = CloseHandle(snap);
        if let Some(r) = r {
            return r.map_err(|e| e.to_string().into());
        }
        Sleep(1);
    }
    Err("uma linha do jogo não saiu da função a desviar".into())
}

unsafe fn vtable<T: Interface>(obj: &T) -> *mut usize {
    *(obj.as_raw() as *const *mut usize)
}

// ---------- DXGI / D3D11 ----------

unsafe extern "system" fn on_present(this: *mut c_void, sync: u32, flags: u32) -> HRESULT {
    guard(|| dx11_present(this, flags));
    let f: Present = std::mem::transmute(orig(this, SLOT_PRESENT));
    lost(this, inner(|| f(this, sync, flags)))
}

unsafe extern "system" fn on_present1(this: *mut c_void, sync: u32, flags: u32, params: *const c_void) -> HRESULT {
    guard(|| dx11_present(this, flags));
    let f: Present1 = std::mem::transmute(orig(this, SLOT_PRESENT1));
    lost(this, inner(|| f(this, sync, flags, params)))
}

/// Dispositivo removido ou reiniciado (driver atualizado, placa travou): o jogo vai refazer tudo, e o nosso sai já.
fn lost(this: *mut c_void, hr: HRESULT) -> HRESULT {
    if hr == DXGI_ERROR_DEVICE_REMOVED || hr == DXGI_ERROR_DEVICE_RESET {
        guard(|| release(&mut lock(&DX11), |x| x.sc == this as usize, true));
    }
    hr
}

/// Solta o que é nosso do back buffer (com `all`, o `Dx11` inteiro) quando `which` bate, e faz o Flush: o D3D11
/// adia a liberação, e o back buffer (ou o do D3D12, pelo 11on12) só fica livre de verdade depois dele.
fn release(g: &mut Option<Dx11>, which: impl FnOnce(&Dx11) -> bool, all: bool) {
    let Some(x) = g.as_mut().filter(|x| which(x)) else { return };
    x.bb = None;
    unsafe {
        x.ctx.Flush();
        if let Some(d10) = &x.d10 {
            d10.Flush();
        }
    }
    if all {
        *g = None;
    }
}

/// O ResizeBuffers falha com o back buffer ainda preso: o que é nosso dele sai antes.
fn drop_bb(this: *mut c_void) {
    guard(|| release(&mut lock(&DX11), |x| x.sc == this as usize, false));
}

const SLOT_CREATE_SC: usize = 10;
const SLOT_CREATE_SC_HWND: usize = 15;
type CreateSc = unsafe extern "system" fn(*mut c_void, *mut c_void, *const c_void, *mut *mut c_void) -> HRESULT;
type CreateScHwnd = unsafe extern "system" fn(*mut c_void, *mut c_void, HWND, *const c_void, *const c_void, *mut c_void, *mut *mut c_void) -> HRESULT;

/// Swapchain nova na janela do jogo (Alt+Enter em alguns, troca de modo): a velha só some quando nada mais a
/// segura, e a nova não nasce enquanto ela existir. O nosso sai antes.
fn forget_window(hwnd: HWND) {
    guard(|| release(&mut lock(&DX11), |x| x.hwnd == hwnd.0 as usize, true));
}

unsafe extern "system" fn on_create_sc(this: *mut c_void, dev: *mut c_void, desc: *const c_void, out: *mut *mut c_void) -> HRESULT {
    if let Some(d) = (desc as *const DXGI_SWAP_CHAIN_DESC).as_ref() {
        forget_window(d.OutputWindow);
    }
    let f: CreateSc = std::mem::transmute(orig(this, SLOT_CREATE_SC));
    inner(|| f(this, dev, desc, out))
}

unsafe extern "system" fn on_create_sc_hwnd(this: *mut c_void, dev: *mut c_void, hwnd: HWND, desc: *const c_void, full: *const c_void, output: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    forget_window(hwnd);
    let f: CreateScHwnd = std::mem::transmute(orig(this, SLOT_CREATE_SC_HWND));
    inner(|| f(this, dev, hwnd, desc, full, output, out))
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
    if flags & DXGI_PRESENT_TEST.0 != 0 || GetTickCount64() < VK_LIVE.load(Relaxed) + 1000 {
        return;
    }
    s.presents.fetch_add(1, Relaxed);
    let Some(sc) = IDXGISwapChain::from_raw_borrowed(&this) else { return };
    let Ok(desc) = sc.GetDesc() else { return };
    if !is_game(s, desc.OutputWindow) {
        return;
    }
    let d11 = sc.GetDevice::<ID3D11Device>().ok();
    // DXGI sem D3D11 nem D3D10 por trás: D3D12.
    let d12 = if d11.is_none() && sc.GetDevice::<ID3D10Device>().is_err() { sc.GetDevice::<ID3D12Device>().ok() } else { None };
    if d12.is_some() {
        s.api.store(API_DX12, Relaxed);
    } else if s.api.load(Relaxed) != API_DX10 {
        // Quem já caiu na ponte fica D3D10 (o D3D10 do Windows também atende como D3D11).
        s.api.store(if d11.is_some() { API_DX11 } else { API_DX10 }, Relaxed);
    }
    if !alive(s) {
        return;
    }
    let mut g = lock(&DX11);
    if let Some(x) = g.as_ref().filter(|x| x.sc != this as usize) {
        // Duas swapchains se alternando na janela (um vídeo, um painel): fica a que já tem efeito, enquanto ela apresentar.
        if x.last.elapsed().as_millis() < 1000 {
            return;
        }
        say!("shader: outra swapchain, recomeçando");
        *g = None;
    }
    if g.is_none() {
        let key = fail_key(s);
        if *lock(&FAIL) == Some(key) {
            return;
        }
        let queue = d12.as_ref().map(|d| lock(&QUEUE).clone().filter(|q| {
            let mut qd: Option<ID3D12Device> = None;
            q.GetDevice(&mut qd).is_ok() && qd.as_ref() == Some(d)
        }));
        if queue.as_ref().is_some_and(|q| q.is_none()) {
            // O jogo ainda não mandou nada pela fila desde o desvio: fica para o próximo quadro.
            return;
        }
        // O D3D10 atende como D3D11 mas recusa o estado de contexto nosso: aí a cadeia vai pela ponte (`Bridge`).
        let made = match (d12, queue.flatten()) {
            (Some(d12), Some(q)) => on12_device(&d12, &q).and_then(|(dev, on12)| Dx11::new(this as usize, dev, None, Some(on12), s)),
            _ => match d11.map(|dev| Dx11::new(this as usize, dev, None, None, s)) {
            Some(Ok(x)) => Ok(x),
            first => match sc.GetDevice::<ID3D10Device>() {
                Ok(d10) => {
                    if let Some(Err(e)) = first {
                        say!("shader: D3D11 do jogo recusou ({e}), indo como D3D10");
                    }
                    s.api.store(API_DX10, Relaxed);
                    own_device(&d10).and_then(|dev| Dx11::new(this as usize, dev, Some(d10), None, s))
                }
                Err(_) => first.unwrap(),
            },
            },
        };
        match made {
            Ok(mut x) => {
                x.hwnd = desc.OutputWindow.0 as usize;
                *g = Some(x);
            }
            Err(e) => {
                *lock(&FAIL) = Some(key);
                say!("shader: D3D11 não abriu: {e}");
                return;
            }
        }
    }
    let x = g.as_mut().unwrap();
    x.last = Instant::now();
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
    /// Um por back buffer no D3D12 (o da vez sai do `GetCurrentBackBufferIndex`); um só nos outros.
    out: Vec<ID3D11RenderTargetView>,
    /// D3D12: os back buffers embrulhados pelo 11on12, na ordem de `out`.
    wrapped: Vec<ID3D11Texture2D>,
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

/// D3D12: um D3D11 por cima do dispositivo e da fila do jogo (11on12). A cadeia, o detector e a redução são os do D3D11.
unsafe fn on12_device(d12: &ID3D12Device, q: &ID3D12CommandQueue) -> Res<(ID3D11Device, ID3D11On12Device)> {
    let mut dev = None;
    D3D11On12CreateDevice(d12, D3D11_CREATE_DEVICE_BGRA_SUPPORT.0, None, Some(&[Some(q.cast::<IUnknown>()?)]), 0, Some(&mut dev), None, None)?;
    let dev = dev.ok_or("sem dispositivo 11on12")?;
    say!("shader: D3D12, a cadeia roda num D3D11 sobre a fila do jogo (11on12)");
    let on12 = dev.cast()?;
    Ok((dev, on12))
}

/// Espera a GPU passar da consulta (100 ms no máximo: travar o jogo é pior que um quadro sem efeito).
fn finished(mut ready: impl FnMut() -> bool) -> Res<()> {
    let t = Instant::now();
    let mut spins = 0;
    while !ready() {
        if t.elapsed().as_millis() >= 100 {
            return Err("a GPU não terminou a cópia em 100 ms".into());
        }
        // Umas voltas rápidas e depois cede a CPU de verdade (o `yield_now` volta na hora se nada mais quer rodar).
        spins += 1;
        unsafe { Sleep(if spins < 50 { 0 } else { 1 }) };
    }
    Ok(())
}

struct Dx11 {
    sc: usize,
    /// Janela da swapchain (`forget_window`).
    hwnd: usize,
    /// Último Present desta swapchain (`dx11_present` ignora outra enquanto esta apresenta).
    last: Instant,
    dev: ID3D11Device,
    /// O dispositivo do jogo, quando ele é D3D10 (o `dev` então é nosso).
    d10: Option<ID3D10Device>,
    /// D3D12: o `dev` é um 11on12 sobre a fila do jogo.
    on12: Option<ID3D11On12Device>,
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
    load: Load,
}

/// Tempo de GPU da cadeia para o menu do motor (`Shared::gpu_us`). Sem consultas de carimbo fica sem medida.
struct Load {
    timer: Option<GpuTimer>,
    n: usize,
    ms: f64,
}

impl Load {
    fn begin(&mut self, ctx: &ID3D11DeviceContext) {
        if let Some(ms) = self.timer.as_mut().and_then(|t| t.begin(ctx, self.n)) {
            self.ms = ema(self.ms, ms);
        }
    }

    fn end(&mut self, ctx: &ID3D11DeviceContext, s: &Shared) {
        if let Some(t) = &self.timer {
            t.end(ctx, self.n);
        }
        self.n += 1;
        s.gpu_us.store((self.ms * 1000.0) as u32, Relaxed);
    }

    /// Efeito parado: a medida some do menu e recomeça do zero.
    fn stop(&mut self, s: &Shared) {
        self.ms = 0.0;
        s.gpu_us.store(0, Relaxed);
    }
}

impl Dx11 {
    unsafe fn new(sc: usize, dev: ID3D11Device, d10: Option<ID3D10Device>, on12: Option<ID3D11On12Device>, s: &Shared) -> Res<Self> {
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
        let load = Load { timer: GpuTimer::new(&dev).ok(), n: 0, ms: 0.0 };
        Ok(Self { sc, hwnd: 0, last: Instant::now(), dev, d10, on12, ctx, state, eng, copy, polled: Instant::now(), seq, bb: None, warned: false, errors: 0, load })
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
            self.load.stop(s);
            return Ok(());
        }
        let (w, h, fmt, samples) = (desc.BufferDesc.Width, desc.BufferDesc.Height, desc.BufferDesc.Format, desc.SampleDesc.Count);
        // Cor do back buffer: só 8 bits por canal. HDR (16 bits ou 10 bits) fica sem efeito.
        let (base, rgba) = match fmt {
            DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => (DXGI_FORMAT_B8G8R8A8_UNORM, false),
            DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => (DXGI_FORMAT_R8G8B8A8_UNORM, true),
            other => {
                self.load.stop(s);
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
            let mut wrapped = Vec::new();
            let (raw, out, bridge) = match (&self.d10, &self.on12) {
                (Some(d10), _) => {
                    let (br, v) = Bridge::new(&self.dev, d10, w, h, base)?;
                    ((br.inp.0.clone(), v), vec![rtv(&self.dev, &br.out.0)?], Some(br))
                }
                (None, Some(on12)) => {
                    let flags = D3D11_RESOURCE_FLAGS { BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default() };
                    let mut out = Vec::new();
                    for i in 0..desc.BufferCount {
                        let mut t: Option<ID3D11Texture2D> = None;
                        on12.CreateWrappedResource(&sc.GetBuffer::<ID3D12Resource>(i)?, &flags, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_PRESENT, &mut t)?;
                        let t = t.ok_or("sem back buffer embrulhado")?;
                        out.push(rtv(&self.dev, &t)?);
                        wrapped.push(t);
                    }
                    (texture(&self.dev, w, h, base, false)?, out, None)
                }
                (None, None) => (texture(&self.dev, w, h, base, false)?, vec![rtv(&self.dev, &sc.GetBuffer::<ID3D11Texture2D>(0)?)?], None),
            };
            let bgra = if rgba {
                let (t, v) = texture(&self.dev, w, h, DXGI_FORMAT_B8G8R8A8_UNORM, true)?;
                let r = rtv(&self.dev, &t)?;
                Some((t, v, r))
            } else {
                None
            };
            // shortcut: back buffer _SRGB sai com a saída em sRGB de novo (fica mais claro); raro em flip model.
            self.bb = Some(Bb { key, raw, bgra, out, wrapped, bridge });
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
                Self::chain_pass(&mut self.eng, &mut self.load, s, &self.copy, &self.dev, &self.ctx, b, &b.out[0], w, h)?;
                self.ctx.End(&br.q11);
                self.ctx.Flush();
                let c: &ID3D11DeviceContext = &self.ctx;
                finished(|| (Interface::vtable(c).GetData)(c.as_raw(), br.q11.as_raw(), std::ptr::null_mut(), 0, 0) == S_OK)?;
                d10.CopyResource(&back, &br.out.1);
                s.drawn.fetch_add(1, Relaxed);
                return Ok(());
            }
            _ if !b.wrapped.is_empty() => {
                // D3D12 (flip model, sem MSAA): o 11on12 põe tudo na fila do jogo antes do Present.
                let on12 = self.on12.as_ref().unwrap();
                let i = sc.cast::<IDXGISwapChain3>()?.GetCurrentBackBufferIndex() as usize;
                let back = b.wrapped.get(i).ok_or("back buffer fora da conta")?;
                let res = [Some(back.cast::<ID3D11Resource>()?)];
                on12.AcquireWrappedResources(&res);
                self.ctx.CopyResource(&b.raw.0, back);
                let r = Self::chain_pass(&mut self.eng, &mut self.load, s, &self.copy, &self.dev, &self.ctx, b, &b.out[i], w, h);
                on12.ReleaseWrappedResources(&res);
                self.ctx.Flush();
                r?;
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
        Self::chain_pass(&mut self.eng, &mut self.load, s, &self.copy, &self.dev, &self.ctx, b, &b.out[0], w, h)?;
        s.drawn.fetch_add(1, Relaxed);
        Ok(())
    }

    /// Da cópia do back buffer (`raw`) até `out`, passando para BGRA quando precisa.
    /// Medido do começo ao fim (o tempo de GPU que o menu mostra).
    unsafe fn chain_pass(eng: &mut Engine, load: &mut Load, s: &Shared, copy: &FullPass, dev: &ID3D11Device, ctx: &ID3D11DeviceContext1, b: &Bb, out: &ID3D11RenderTargetView, w: u32, h: u32) -> Res<()> {
        load.begin(ctx);
        let r = Self::chain_inner(eng, copy, dev, ctx, b, out, w, h);
        load.end(ctx, s);
        if r.is_err() {
            load.stop(s);
        }
        r
    }

    unsafe fn chain_inner(eng: &mut Engine, copy: &FullPass, dev: &ID3D11Device, ctx: &ID3D11DeviceContext1, b: &Bb, out: &ID3D11RenderTargetView, w: u32, h: u32) -> Res<()> {
        let input = match &b.bgra {
            Some((t, v, r)) => {
                copy.draw(ctx, &b.raw.1, r, [0.0, 0.0, w as f32, h as f32], [0.0; 4])?;
                (t, v)
            }
            None => (&b.raw.0, &b.raw.1),
        };
        eng.render(dev, ctx, input, (w, h), out, [0.0, 0.0, w as f32, h as f32])
    }
}

const SLOT_D12_EXECUTE: usize = 10;
type ExecuteCommandLists = unsafe extern "system" fn(*mut c_void, u32, *const *mut c_void);

/// A última fila DIRECT do jogo: a swapchain do D3D12 não diz com que fila nasceu, e o 11on12 precisa dela.
static QUEUE: Mutex<Option<ID3D12CommandQueue>> = Mutex::new(None);
static QUEUE_RAW: AtomicUsize = AtomicUsize::new(0);
/// Trampolim do `ExecuteCommandLists` (chamado muitas vezes por quadro: sem procurar na lista de desvios).
static D12_EXECUTE: AtomicUsize = AtomicUsize::new(0);

/// Outra fila DIRECT do mesmo dispositivo, já vista: não toma o lugar da primeira (e não é olhada de novo).
static QUEUE_OTHER: AtomicUsize = AtomicUsize::new(0);

unsafe extern "system" fn on_execute(this: *mut c_void, n: u32, lists: *const *mut c_void) {
    if QUEUE_RAW.load(Relaxed) != this as usize && QUEUE_OTHER.load(Relaxed) != this as usize {
        guard(|| {
            let Some(q) = ID3D12CommandQueue::from_raw_borrowed(&this).filter(|q| q.GetDesc().Type == D3D12_COMMAND_LIST_TYPE_DIRECT) else { return };
            let mut g = lock(&QUEUE);
            // shortcut: com duas filas DIRECT no mesmo dispositivo fica a primeira que mandou trabalho; um jogo que apresenta pela segunda tem o efeito fora de ordem com ela.
            let dev = |q: &ID3D12CommandQueue| {
                let mut d: Option<ID3D12Device> = None;
                q.GetDevice(&mut d).ok().and(d)
            };
            if g.as_ref().is_some_and(|old| dev(old) == dev(q)) {
                QUEUE_OTHER.store(this as usize, Relaxed);
                return;
            }
            *g = Some(q.clone());
            QUEUE_RAW.store(this as usize, Relaxed);
        });
    }
    let f: ExecuteCommandLists = std::mem::transmute(D12_EXECUTE.load(Relaxed));
    f(this, n, lists)
}

unsafe fn install_d12() -> Res<()> {
    let mut dev: Option<ID3D12Device> = None;
    D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev)?;
    let q: ID3D12CommandQueue = dev.ok_or("sem dispositivo")?.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() })?;
    let at = *vtable(&q).add(SLOT_D12_EXECUTE);
    detour_with(at, on_execute as *const () as usize, |t| D12_EXECUTE.store(t, Relaxed))
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
        let mut ok = hook_dxgi(t, SLOT_PRESENT, on_present as *const () as usize) && hook_dxgi(t, SLOT_RESIZE, on_resize as *const () as usize);
        if let Ok(sc1) = sc.cast::<IDXGISwapChain1>() {
            ok &= hook_dxgi(vtable(&sc1), SLOT_PRESENT1, on_present1 as *const () as usize);
        }
        // Só a tabela de quem é IDXGISwapChain3 chega à entrada 39.
        if let Ok(sc3) = sc.cast::<IDXGISwapChain3>() {
            ok &= hook_dxgi(vtable(&sc3), SLOT_RESIZE1, on_resize1 as *const () as usize);
        }
        // A fábrica: swapchain nova na mesma janela (`forget_window`). As entradas não se cruzam com as da swapchain.
        if let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() {
            ok &= hook_dxgi(vtable(&f), SLOT_CREATE_SC, on_create_sc as *const () as usize);
            if let Ok(f2) = f.cast::<IDXGIFactory2>() {
                ok &= hook_dxgi(vtable(&f2), SLOT_CREATE_SC_HWND, on_create_sc_hwnd as *const () as usize);
            }
        }
        if ok { Ok(()) } else { Err("VirtualProtect".to_string()) }
    });
    drop(dev);
    let _ = DestroyWindow(window);
    Ok(done?)
}

// ---------- D3D9 ----------

/// Funções do D3D9 desviadas: Reset, Present, PresentEx, ResetEx (a ordem de `D9_NEXT`).
const D9_SLOTS: [usize; 4] = [SLOT_D9_RESET, SLOT_D9_PRESENT, SLOT_D9_PRESENT_EX, SLOT_D9_RESET_EX];
/// Cópias de cada gancho: o comum e o 9Ex (e cada jeito de processar vértices) podem ter código diferente.
const D9_N: usize = 4;
/// Trampolim de cada cópia. O gancho sabe o seu sem olhar a tabela do dispositivo: um overlay que troque a
/// entrada dela por cima do nosso desvio fazia o trampolim achado pela tabela ser o dele, que voltava a nós sem fim.
static D9_NEXT: [[AtomicUsize; D9_N]; 4] = [const { [const { AtomicUsize::new(0) }; D9_N] }; 4];

unsafe fn d9_next(f: usize, n: usize) -> usize {
    D9_NEXT[f][n].load(Relaxed)
}

/// MAKE_D3DHRESULT(2160); o crate windows não traz.
const D3DERR_DEVICEREMOVED: HRESULT = HRESULT(0x8876_0870_u32 as i32);

/// D3DERR_DEVICEREMOVED (só no 9Ex): o jogo vai criar outro dispositivo, e o nosso sai já.
fn d9_lost(this: *mut c_void, hr: HRESULT) -> HRESULT {
    if hr == D3DERR_DEVICEREMOVED {
        guard(|| {
            let mut g = lock(&D9);
            if g.as_ref().is_some_and(|x| x.dev == this as usize) {
                *g = None;
            }
        });
    }
    hr
}

unsafe extern "system" fn on_d9_present<const N: usize>(this: *mut c_void, a: *const c_void, b: *const c_void, w: HWND, r: *const c_void) -> HRESULT {
    guard(|| d9_present(this, w, a as *const RECT));
    let f: D9Present = std::mem::transmute(d9_next(1, N));
    d9_lost(this, inner(|| f(this, a, b, w, r)))
}

unsafe extern "system" fn on_d9_present_ex<const N: usize>(this: *mut c_void, a: *const c_void, b: *const c_void, w: HWND, r: *const c_void, flags: u32) -> HRESULT {
    guard(|| d9_present(this, w, a as *const RECT));
    let f: D9PresentEx = std::mem::transmute(d9_next(2, N));
    d9_lost(this, inner(|| f(this, a, b, w, r, flags)))
}

/// O Reset do D3D9 só funciona sem nada nosso em D3DPOOL_DEFAULT: a cadeia inteira sai e volta no próximo Present.
unsafe extern "system" fn on_d9_reset<const N: usize>(this: *mut c_void, pp: *mut c_void) -> HRESULT {
    guard(|| drop(lock(&D9).take()));
    let f: D9Reset = std::mem::transmute(d9_next(0, N));
    f(this, pp)
}

unsafe extern "system" fn on_d9_reset_ex<const N: usize>(this: *mut c_void, pp: *mut c_void, mode: *mut c_void) -> HRESULT {
    guard(|| drop(lock(&D9).take()));
    let f: D9ResetEx = std::mem::transmute(d9_next(3, N));
    f(this, pp, mode)
}

/// Desvia as funções da tabela `t` (as `count` primeiras de `D9_SLOTS`), cada endereço novo com a sua cópia do gancho.
unsafe fn d9_hook(t: *const usize, count: usize) -> Res<()> {
    let hooks: [[usize; D9_N]; 4] = [
        [on_d9_reset::<0> as *const () as usize, on_d9_reset::<1> as *const () as usize, on_d9_reset::<2> as *const () as usize, on_d9_reset::<3> as *const () as usize],
        [on_d9_present::<0> as *const () as usize, on_d9_present::<1> as *const () as usize, on_d9_present::<2> as *const () as usize, on_d9_present::<3> as *const () as usize],
        [on_d9_present_ex::<0> as *const () as usize, on_d9_present_ex::<1> as *const () as usize, on_d9_present_ex::<2> as *const () as usize, on_d9_present_ex::<3> as *const () as usize],
        [on_d9_reset_ex::<0> as *const () as usize, on_d9_reset_ex::<1> as *const () as usize, on_d9_reset_ex::<2> as *const () as usize, on_d9_reset_ex::<3> as *const () as usize],
    ];
    for f in 0..count {
        let at = *t.add(D9_SLOTS[f]);
        if lock(&DETOURS).iter().any(|d| d.0 == at) {
            continue;
        }
        let n = D9_NEXT[f].iter().position(|a| a.load(Relaxed) == 0).ok_or("versões demais da mesma função do D3D9")?;
        detour_with(at, hooks[f][n], |t| D9_NEXT[f][n].store(t, SeqCst))?;
    }
    Ok(())
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
    if let Some(x) = g.as_ref().filter(|x| x.dev != this as usize) {
        // Dois dispositivos se alternando: fica o que já tem efeito, enquanto ele apresentar.
        if x.last.elapsed().as_millis() < 1000 {
            return;
        }
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
    x.last = Instant::now();
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
    /// Último Present deste dispositivo.
    last: Instant,
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
        let mut x = Self { dev: dev_ptr, last: Instant::now(), t: Tracker::open(&s.path())?, chain: None, defaults: HashMap::new(), polled: Instant::now(), seq, input: None, small: None, detect: Default::default(), strips: None, sb: None, frame: 0, warned: false, errors: 0 };
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
                d9_hook(vtable(&dev), 2)?;
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
            d9_hook(vtable(&dev), 4)?;
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

// ---------- OpenGL ----------

type SwapBuffers = unsafe extern "system" fn(HDC) -> BOOL;
type SwapLayer = unsafe extern "system" fn(HDC, u32) -> BOOL;
type SwapMulti = unsafe extern "system" fn(u32, *const WGLSWAP) -> u32;
static GL: Mutex<Option<Gl>> = Mutex::new(None);
/// Trampolins do `wglSwapBuffers` (o `SwapBuffers` do GDI também passa por ele), `wglSwapLayerBuffers` e
/// `wglSwapMultipleBuffers`. Se um chama o outro por dentro, o `inner` deixa o efeito numa vez só.
static GL_SWAP: [AtomicUsize; 3] = [const { AtomicUsize::new(0) }; 3];
const WGL_SWAP_MAIN_PLANE: u32 = 1;

unsafe extern "system" fn on_swap(hdc: HDC) -> BOOL {
    guard(|| gl_present(hdc));
    let f: SwapBuffers = std::mem::transmute(GL_SWAP[0].load(Relaxed));
    inner(|| f(hdc))
}

unsafe extern "system" fn on_swap_layer(hdc: HDC, planes: u32) -> BOOL {
    if planes & WGL_SWAP_MAIN_PLANE != 0 {
        guard(|| gl_present(hdc));
    }
    let f: SwapLayer = std::mem::transmute(GL_SWAP[1].load(Relaxed));
    inner(|| f(hdc, planes))
}

unsafe extern "system" fn on_swap_multi(n: u32, list: *const WGLSWAP) -> u32 {
    if !list.is_null() {
        for w in std::slice::from_raw_parts(list, n as usize) {
            if w.uiFlags & WGL_SWAP_MAIN_PLANE != 0 {
                guard(|| gl_present(w.hdc));
            }
        }
    }
    let f: SwapMulti = std::mem::transmute(GL_SWAP[2].load(Relaxed));
    inner(|| f(n, list))
}

/// A cadeia roda num contexto GL nosso no mesmo HDC: o back buffer é da janela, não do contexto, e o estado do
/// jogo (programa, texturas, alvos) fica intocado sem precisar guardar e repor.
unsafe fn gl_present(hdc: HDC) {
    let Some(s) = shared() else { return };
    s.presents.fetch_add(1, Relaxed);
    let hwnd = WindowFromDC(hdc);
    if !is_game(s, hwnd) {
        return;
    }
    s.api.store(API_GL, Relaxed);
    if !alive(s) {
        return;
    }
    let (dc, rc) = (wglGetCurrentDC(), wglGetCurrentContext());
    let mut g = lock(&GL);
    // Pela janela, não pelo HDC: sem CS_OWNDC o jogo pode apresentar cada quadro com um HDC diferente.
    if g.as_ref().is_some_and(|x| x.hwnd != hwnd.0 as usize) {
        *g = None;
    }
    if g.is_none() {
        let key = fail_key(s);
        if *lock(&FAIL) != Some(key) {
            match Gl::new(hdc, hwnd, s) {
                Ok(x) => *g = Some(x),
                Err(e) => {
                    *lock(&FAIL) = Some(key);
                    say!("shader: OpenGL não abriu: {e}");
                }
            }
        }
    }
    if let Some(x) = g.as_mut().filter(|x| x.errors < MAX_ERRORS) {
        let r = match wglMakeCurrent(hdc, x.rc) {
            Ok(()) => x.frame(s, hwnd),
            Err(e) => Err(e.into()),
        };
        match r {
            Ok(()) => x.errors = 0,
            Err(e) => {
                x.errors += 1;
                say!("shader: {e}");
                if x.errors >= MAX_ERRORS {
                    say!("shader: {MAX_ERRORS} erros seguidos, efeito desligado nesta janela");
                }
            }
        }
    }
    drop(g);
    let _ = wglMakeCurrent(dc, rc);
}

/// Textura com o seu alvo de desenho.
struct Tex {
    size: (u32, u32),
    tex: glow::Texture,
    fbo: glow::Framebuffer,
}

impl Tex {
    /// Refaz `slot` quando o tamanho muda.
    unsafe fn fit(gl: &glow::Context, slot: &mut Option<Tex>, w: u32, h: u32) -> Res<()> {
        if slot.as_ref().is_some_and(|t| t.size == (w, h)) {
            return Ok(());
        }
        if let Some(t) = slot.take() {
            gl.delete_framebuffer(t.fbo);
            gl.delete_texture(t.tex);
        }
        let tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_storage_2d(glow::TEXTURE_2D, 1, glow::RGBA8, w as i32, h as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::NEAREST as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::NEAREST as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);
        let fbo = gl.create_framebuffer().inspect_err(|_| gl.delete_texture(tex))?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        *slot = Some(Tex { size: (w, h), tex, fbo });
        Ok(())
    }

    fn image(&self) -> ChainGlImage {
        ChainGlImage { handle: Some(self.tex), format: glow::RGBA8, size: Size::new(self.size.0, self.size.1) }
    }
}

/// Copia de `src` para `dst` (o alvo de cada um, ou o back buffer com `None`), por ponto.
unsafe fn blit(gl: &glow::Context, src: Option<glow::Framebuffer>, from: [i32; 4], dst: Option<glow::Framebuffer>, to: [i32; 4]) {
    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, src);
    gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, dst);
    if src.is_none() {
        gl.read_buffer(glow::BACK);
    }
    if dst.is_none() {
        gl.draw_buffer(glow::BACK);
    }
    gl.blit_framebuffer(from[0], from[1], from[2], from[3], to[0], to[1], to[2], to[3], glow::COLOR_BUFFER_BIT, glow::NEAREST);
}

struct Gl {
    hwnd: usize,
    rc: HGLRC,
    ctx: Arc<glow::Context>,
    t: Tracker,
    chain: Option<ChainGl>,
    defaults: HashMap<String, f32>,
    polled: Instant,
    seq: u32,
    input: Option<Tex>,
    small: Option<Tex>,
    out: Option<Tex>,
    detect: blocks::Detect,
    /// Faixas do meio para o detector (horizontal e vertical), copiadas num quadro e lidas no seguinte.
    strips: [Option<Tex>; 2],
    pending: bool,
    frame: usize,
    errors: u32,
}

// O contexto GL só é usado na linha que apresenta, sempre debaixo do `Mutex`.
unsafe impl Send for Gl {}

impl Drop for Gl {
    fn drop(&mut self) {
        unsafe {
            // A cadeia apaga os objetos dela no contexto atual: tem de ser o nosso, nunca o do jogo.
            let hwnd = HWND(self.hwnd as _);
            let dc = GetDC(Some(hwnd));
            if !dc.is_invalid() && wglMakeCurrent(dc, self.rc).is_ok() {
                self.chain = None;
            } else {
                std::mem::forget(self.chain.take());
            }
            let _ = wglMakeCurrent(HDC::default(), HGLRC::default());
            let _ = wglDeleteContext(self.rc);
            if !dc.is_invalid() {
                ReleaseDC(Some(hwnd), dc);
            }
        }
    }
}

/// As funções do contexto atual. As do GL 1.1 só saem da opengl32.dll; as outras, do driver pelo wglGetProcAddress.
unsafe fn glow_context() -> glow::Context {
    let lib = GetModuleHandleW(w!("opengl32.dll")).unwrap_or_default();
    glow::Context::from_loader_function_cstr(|name| {
        let n = PCSTR(name.as_ptr() as *const u8);
        match wglGetProcAddress(n) {
            Some(f) if !matches!(f as usize, 1 | 2 | 3 | usize::MAX) => f as *const c_void,
            _ => GetProcAddress(lib, n).map_or(std::ptr::null(), |f| f as *const c_void),
        }
    })
}

impl Gl {
    unsafe fn new(hdc: HDC, hwnd: HWND, s: &Shared) -> Res<Self> {
        let rc = wglCreateContext(hdc)?;
        if let Err(e) = wglMakeCurrent(hdc, rc) {
            let _ = wglDeleteContext(rc);
            return Err(e.into());
        }
        let ctx = glow_context();
        let seq = s.seq.load(Acquire);
        let t = Tracker::open(&s.path()).inspect_err(|_| {
            let _ = wglMakeCurrent(HDC::default(), HGLRC::default());
            let _ = wglDeleteContext(rc);
        })?;
        let mut x = Self { hwnd: hwnd.0 as usize, rc, ctx: Arc::new(ctx), t, chain: None, defaults: HashMap::new(), polled: Instant::now(), seq, input: None, small: None, out: None, detect: Default::default(), strips: [None, None], pending: false, frame: 0, errors: 0 };
        x.build();
        say!("shader: OpenGL pronto, perfil {}", x.t.path.display());
        Ok(x)
    }

    fn build(&mut self) {
        self.chain = None;
        self.defaults.clear();
        let paths = self.t.on_paths();
        if paths.is_empty() {
            return;
        }
        let t = Instant::now();
        match chain::join(&paths).and_then(|p| unsafe { ChainGl::load_from_preset(p, self.ctx.clone(), None) }.map_err(Into::into)) {
            Ok(c) => {
                self.defaults = chain::defaults(&c);
                chain::apply(&c, &self.defaults, &self.t.profile);
                self.chain = Some(c);
                say!("shader: cadeia OpenGL de {} preset(s) carregada em {} ms", paths.len(), t.elapsed().as_millis());
            }
            Err(e) => say!("shader: {e}"),
        }
    }

    /// Com o nosso contexto atual no HDC do jogo.
    unsafe fn frame(&mut self, s: &Shared, hwnd: HWND) -> Res<()> {
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
                self.pending = false;
            }
            if c.chain {
                self.build();
            } else if let Some(ch) = self.chain.as_ref().filter(|_| c.params) {
                chain::apply(ch, &self.defaults, &self.t.profile);
            }
        }
        let p = &self.t.profile;
        if !p.on || !p.inject || s.flags.load(Relaxed) & FLAG_COMPARE != 0 || self.chain.is_none() {
            return Ok(());
        }
        let mut r = RECT::default();
        GetClientRect(hwnd, &mut r)?;
        let (w, h) = (r.right as u32, r.bottom as u32);
        if w == 0 || h == 0 {
            return Ok(());
        }
        let gl = &*self.ctx.clone();
        if self.input.as_ref().is_none_or(|t| t.size != (w, h)) {
            self.detect = Default::default();
            self.pending = false;
            say!("shader: back buffer {w}x{h} (OpenGL)");
        }
        Tex::fit(gl, &mut self.input, w, h)?;
        Tex::fit(gl, &mut self.out, w, h)?;
        let (wi, hi) = (w as i32, h as i32);
        let input = self.input.as_ref().unwrap().fbo;
        blit(gl, None, [0, 0, wi, hi], Some(input), [0, 0, wi, hi]);
        if self.t.profile.input == "auto" {
            self.detect_blocks(w, h)?;
        }
        let mut src = self.input.as_ref().unwrap().image();
        if let Some(k) = blocks::reduce_for(&self.t.profile.input, h, self.detect.found) {
            let size = blocks::reduced_size(w, h, k);
            if self.small.as_ref().is_none_or(|t| t.size != size) {
                say!("shader: entrada {w}x{h} → {}x{}", size.0, size.1);
            }
            Tex::fit(gl, &mut self.small, size.0, size.1)?;
            let small = self.small.as_ref().unwrap();
            // Um ponto do centro de cada bloco, como o PS_REDUCE; o GL conta as linhas de baixo para cima.
            let (x0, y1) = (k[2], h as f32 - k[3]);
            let from = [x0, y1 - size.1 as f32 * k[1], x0 + size.0 as f32 * k[0], y1].map(|v| v.round() as i32);
            blit(gl, Some(input), from, Some(small.fbo), [0, 0, size.0 as i32, size.1 as i32]);
            src = small.image();
        }
        let out = self.out.as_ref().unwrap();
        let vp = Viewport { x: 0.0, y: 0.0, mvp: None, output: &out.image(), size: Size::new(w, h) };
        self.chain.as_mut().unwrap().frame(&src, &vp, self.frame, None)?;
        blit(gl, Some(out.fbo), [0, 0, wi, hi], None, [0, 0, wi, hi]);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        gl.flush();
        self.frame += 1;
        s.drawn.fetch_add(1, Relaxed);
        Ok(())
    }

    /// O mesmo detector de blocos do D3D11, com as faixas lidas no quadro seguinte ao da cópia: o `glReadPixels`
    /// espera a GPU, mas a cópia já terminou no quadro anterior e a espera é curta.
    unsafe fn detect_blocks(&mut self, w: u32, h: u32) -> Res<()> {
        let gl = &*self.ctx.clone();
        let st = 32.min(w).min(h);
        if self.strips[0].as_ref().is_none_or(|t| t.size != (w, st)) {
            self.pending = false;
        }
        Tex::fit(gl, &mut self.strips[0], w, st)?;
        Tex::fit(gl, &mut self.strips[1], st, h)?;
        if !self.detect.due() {
            return Ok(());
        }
        let (a, b) = (self.strips[0].as_ref().unwrap(), self.strips[1].as_ref().unwrap());
        if self.pending {
            self.pending = false;
            let found = blocks::detect(&read_gl(gl, a), w as usize, &read_gl(gl, b), h as usize, st as usize);
            self.detect.vote(found);
            return Ok(());
        }
        let input = self.input.as_ref().unwrap().fbo;
        let (y0, x0) = (((h - st) / 2) as i32, ((w - st) / 2) as i32);
        let (wi, hi, si) = (w as i32, h as i32, st as i32);
        blit(gl, Some(input), [0, y0, wi, y0 + si], Some(a.fbo), [0, 0, wi, si]);
        blit(gl, Some(input), [x0, 0, x0 + si, hi], Some(b.fbo), [0, 0, si, hi]);
        self.pending = true;
        Ok(())
    }
}

/// A textura em pixels BGRA sem alfa, de cima para baixo (o GL guarda de baixo para cima).
unsafe fn read_gl(gl: &glow::Context, t: &Tex) -> Vec<u32> {
    let (w, h) = t.size;
    let mut buf = vec![0u8; (w * h * 4) as usize];
    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(t.fbo));
    gl.read_pixels(0, 0, w as i32, h as i32, glow::BGRA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut buf)));
    buf.chunks_exact((w * 4) as usize).rev().flat_map(|row| row.chunks_exact(4).map(|p| u32::from_le_bytes([p[0], p[1], p[2], 0]))).collect()
}

unsafe fn install_gl() -> Res<()> {
    let lib = GetModuleHandleW(w!("opengl32.dll"))?;
    let hooks = [on_swap as *const () as usize, on_swap_layer as *const () as usize, on_swap_multi as *const () as usize];
    for (i, name) in [s!("wglSwapBuffers"), s!("wglSwapLayerBuffers"), s!("wglSwapMultipleBuffers")].into_iter().enumerate() {
        let r = GetProcAddress(lib, name).ok_or_else(|| "função do wgl faltando".into()).and_then(|at| detour_with(at as usize, hooks[i], |t| GL_SWAP[i].store(t, SeqCst)));
        // Só o wglSwapBuffers é indispensável: as outras duas quase nenhum jogo usa.
        match r {
            Err(e) if i == 0 => return Err(e),
            Err(e) => say!("shader: OpenGL, {} sem desvio: {e}", ["", "wglSwapLayerBuffers", "wglSwapMultipleBuffers"][i]),
            Ok(()) => {}
        }
    }
    Ok(())
}

// ---------- Vulkan ----------

/// Drivers diferentes (placa e vídeo integrado) têm funções em endereços diferentes; cada um ganha a sua cópia
/// do desvio (`N`), que sabe qual original chamar.
const VK_N: usize = 4;
const VK_PRESENT: usize = 0;
const VK_CREATE_SC: usize = 1;
const VK_DESTROY_SC: usize = 2;
const VK_DESTROY_DEV: usize = 3;
const VK_NAMES: [&CStr; 4] = [c"vkQueuePresentKHR", c"vkCreateSwapchainKHR", c"vkDestroySwapchainKHR", c"vkDestroyDevice"];
/// Quadros em voo: cada um com o seu comando, cerca e semáforo.
const VK_SLOTS: usize = 4;

/// Trampolins por função e por driver.
static VK_NEXT: [[AtomicUsize; VK_N]; 4] = [const { [const { AtomicUsize::new(0) }; VK_N] }; 4];
/// A nossa instância (só para consultas da placa), e por placa os endereços das funções do driver dela e o par
/// (fabricante, modelo).
static VK_INST: Mutex<Option<(ash::Instance, Vec<VkCard>)>> = Mutex::new(None);
type VkCard = ([usize; 4], vk::PhysicalDevice, (u32, u32));
/// Por dispositivo do jogo criado depois da injeção, o par (fabricante, modelo) da placa dele: com duas placas no
/// mesmo driver, os endereços das funções não dizem qual é.
static VK_IDS: Mutex<Vec<(usize, (u32, u32))>> = Mutex::new(Vec::new());
/// Trampolim do `vkCreateDevice` da vulkan-1.dll e o `vkGetPhysicalDeviceProperties` dela.
static VK_CREATE_DEV: AtomicUsize = AtomicUsize::new(0);
static VK_PROPS: AtomicUsize = AtomicUsize::new(0);
/// Por dispositivo do jogo, pela chave de despacho (a primeira palavra do objeto, a mesma nas filas dele).
static VK_DEVS: Mutex<Vec<(usize, VkDev)>> = Mutex::new(Vec::new());
/// Swapchain de antes da DLL já avisada com OUT_OF_DATE. Uma vez só: se a nova também não passar por nós, o
/// jogo não fica refazendo para sempre.
static VK_NUDGED: AtomicBool = AtomicBool::new(false);
/// Último Present em Vulkan (`GetTickCount64`): a swapchain DXGI que o driver da NVIDIA monta por baixo (Vulkan
/// sobre DXGI) fica de fora enquanto o jogo apresenta em Vulkan, e só então.
static VK_LIVE: AtomicU64 = AtomicU64::new(0);

fn vk_key<T: vk::Handle>(h: T) -> usize {
    unsafe { *(h.as_raw() as usize as *const usize) }
}

unsafe fn vk_next(f: usize, n: usize) -> usize {
    VK_NEXT[f][n].load(Relaxed)
}

/// Lista do Vulkan como fatia; ponteiro nulo ou contagem zero dão vazia.
unsafe fn vk_list<'a, T>(p: *const T, n: u32) -> &'a [T] {
    if p.is_null() || n == 0 { &[] } else { std::slice::from_raw_parts(p, n as usize) }
}

unsafe fn vk_ids(pd: vk::PhysicalDevice) -> Option<(u32, u32)> {
    let f = VK_PROPS.load(Relaxed);
    if f == 0 {
        return None;
    }
    let f: vk::PFN_vkGetPhysicalDeviceProperties = std::mem::transmute(f);
    let mut p = vk::PhysicalDeviceProperties::default();
    f(pd, &mut p);
    Some((p.vendor_id, p.device_id))
}

unsafe extern "system" fn vk_create_dev(pd: vk::PhysicalDevice, ci: *const vk::DeviceCreateInfo, a: *const vk::AllocationCallbacks, out: *mut vk::Device) -> vk::Result {
    let next: vk::PFN_vkCreateDevice = std::mem::transmute(VK_CREATE_DEV.load(Relaxed));
    let r = inner(|| next(pd, ci, a, out));
    if r == vk::Result::SUCCESS {
        let d = *out;
        guard(|| {
            if let Some(id) = vk_ids(pd) {
                lock(&VK_IDS).push((vk_key(d), id));
            }
        });
    }
    r
}

/// As funções valem para qualquer dispositivo do mesmo driver: um dispositivo nosso de cada placa dá os
/// endereços, e o desvio no código pega também o jogo que já tinha os ponteiros antes de a DLL entrar.
unsafe fn install_vk() -> Res<()> {
    let lib = GetModuleHandleW(w!("vulkan-1.dll")).map_err(|_| "sem vulkan-1.dll no jogo")?;
    let gipa: vk::PFN_vkGetInstanceProcAddr = std::mem::transmute(GetProcAddress(lib, s!("vkGetInstanceProcAddr")).ok_or("sem vkGetInstanceProcAddr")?);
    let entry = ash::Entry::from_static_fn(ash::StaticFn { get_instance_proc_addr: gipa });
    let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
    let inst = entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None)?;
    let mut cards: Vec<VkCard> = Vec::new();
    for pd in inst.enumerate_physical_devices()? {
        let Some(fam) = vk_graphics_family(&inst, pd) else { continue };
        let prio = [1.0];
        let qi = [vk::DeviceQueueCreateInfo::default().queue_family_index(fam).queue_priorities(&prio)];
        let ext = [ash::khr::swapchain::NAME.as_ptr()];
        let Ok(dev) = inst.create_device(pd, &vk::DeviceCreateInfo::default().queue_create_infos(&qi).enabled_extension_names(&ext), None) else { continue };
        let gdpa = inst.fp_v1_0().get_device_proc_addr;
        let set = VK_NAMES.map(|n| gdpa(dev.handle(), n.as_ptr()).map_or(0, |f| f as usize));
        dev.destroy_device(None);
        let p = inst.get_physical_device_properties(pd);
        cards.push((set, pd, (p.vendor_id, p.device_id)));
    }
    if cards.is_empty() {
        inst.destroy_instance(None);
        return Err("nenhuma placa Vulkan com swapchain".into());
    }
    let hooks: [[usize; VK_N]; 4] = [
        [vk_present::<0> as *const () as usize, vk_present::<1> as *const () as usize, vk_present::<2> as *const () as usize, vk_present::<3> as *const () as usize],
        [vk_create_sc::<0> as *const () as usize, vk_create_sc::<1> as *const () as usize, vk_create_sc::<2> as *const () as usize, vk_create_sc::<3> as *const () as usize],
        [vk_destroy_sc::<0> as *const () as usize, vk_destroy_sc::<1> as *const () as usize, vk_destroy_sc::<2> as *const () as usize, vk_destroy_sc::<3> as *const () as usize],
        [vk_destroy_dev::<0> as *const () as usize, vk_destroy_dev::<1> as *const () as usize, vk_destroy_dev::<2> as *const () as usize, vk_destroy_dev::<3> as *const () as usize],
    ];
    // Um gancho por driver (duas placas do mesmo driver dividem as funções).
    let mut list: Vec<[usize; 4]> = Vec::new();
    for c in &cards {
        if !list.contains(&c.0) {
            list.push(c.0);
        }
    }
    let n = list.len().min(VK_N);
    *lock(&VK_INST) = Some((inst, cards));
    // O Present por último: ele já pode mandar o jogo refazer a swapchain, e a criação precisa estar desviada.
    for f in [VK_CREATE_SC, VK_DESTROY_SC, VK_DESTROY_DEV, VK_PRESENT] {
        for (n, set) in list.iter().enumerate().take(VK_N) {
            if set[f] == 0 {
                return Err(format!("o driver não deu {:?}", VK_NAMES[f]).into());
            }
            detour_with(set[f], hooks[f][n], |t| VK_NEXT[f][n].store(t, SeqCst))?;
        }
    }
    // Só para separar duas placas do mesmo driver; sem ele, o dispositivo de antes da injeção fica sem efeito nesse caso.
    VK_PROPS.store(GetProcAddress(lib, s!("vkGetPhysicalDeviceProperties")).map_or(0, |f| f as usize), Relaxed);
    if let Some(at) = GetProcAddress(lib, s!("vkCreateDevice")) {
        if let Err(e) = detour_with(at as usize, vk_create_dev as *const () as usize, |t| VK_CREATE_DEV.store(t, SeqCst)) {
            say!("shader: Vulkan, vkCreateDevice sem desvio: {e}");
        }
    }
    say!("shader: Vulkan, {n} driver(s) desviado(s)");
    Ok(())
}

unsafe fn vk_graphics_family(inst: &ash::Instance, pd: vk::PhysicalDevice) -> Option<u32> {
    inst.get_physical_device_queue_family_properties(pd).iter().position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS)).map(|i| i as u32)
}

unsafe extern "system" fn vk_create_sc<const N: usize>(d: vk::Device, ci: *const vk::SwapchainCreateInfoKHR, a: *const vk::AllocationCallbacks, out: *mut vk::SwapchainKHR) -> vk::Result {
    let next: vk::PFN_vkCreateSwapchainKHR = std::mem::transmute(vk_next(VK_CREATE_SC, N));
    // A cadeia lê a imagem do jogo por cópia e desenha nela: a swapchain precisa deixar as duas coisas.
    let mut c = *ci;
    c.image_usage |= vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::COLOR_ATTACHMENT;
    let mut r = inner(|| next(d, &c, a, out));
    let ours = r == vk::Result::SUCCESS;
    if !ours {
        // A velha já foi aposentada pela tentativa que falhou; passar ela de novo não vale.
        c = *ci;
        c.old_swapchain = vk::SwapchainKHR::null();
        r = inner(|| next(d, &c, a, out));
    }
    if r == vk::Result::SUCCESS {
        let sc = *out;
        guard(|| vk_swapchain(d, &*ci, sc, ours));
    }
    r
}

unsafe extern "system" fn vk_destroy_sc<const N: usize>(d: vk::Device, sc: vk::SwapchainKHR, a: *const vk::AllocationCallbacks) {
    guard(|| {
        if let Some((_, x)) = lock(&VK_DEVS).iter_mut().find(|x| x.0 == vk_key(d)) {
            // As nossas cópias podem estar lendo as imagens dela.
            x.wait();
            x.scs.retain(|s| s.0 != sc);
            if x.main == Some(sc) {
                x.main = None;
            }
        }
    });
    let next: vk::PFN_vkDestroySwapchainKHR = std::mem::transmute(vk_next(VK_DESTROY_SC, N));
    next(d, sc, a)
}

unsafe extern "system" fn vk_destroy_dev<const N: usize>(d: vk::Device, a: *const vk::AllocationCallbacks) {
    guard(|| {
        let mut g = lock(&VK_DEVS);
        if let Some(i) = g.iter().position(|x| x.0 == vk_key(d)) {
            drop(g.remove(i));
        }
        lock(&VK_IDS).retain(|x| x.0 != vk_key(d));
    });
    let next: vk::PFN_vkDestroyDevice = std::mem::transmute(vk_next(VK_DESTROY_DEV, N));
    next(d, a)
}

unsafe extern "system" fn vk_present<const N: usize>(q: vk::Queue, info: *const vk::PresentInfoKHR) -> vk::Result {
    let next: vk::PFN_vkQueuePresentKHR = std::mem::transmute(vk_next(VK_PRESENT, N));
    VK_LIVE.store(GetTickCount64(), Relaxed);
    let (mut sem, mut nudge) = (None, false);
    guard(|| (sem, nudge) = vk_frame(q, &*info));
    let r = match sem {
        Some(s) => {
            let w = [s];
            let mut i = *info;
            i.wait_semaphore_count = 1;
            i.p_wait_semaphores = w.as_ptr();
            inner(|| next(q, &i))
        }
        None => inner(|| next(q, info)),
    };
    // A swapchain é de antes da DLL (sem a cópia liberada): o jogo refaz ao ver OUT_OF_DATE, e aí ela passa por nós.
    if nudge && r.as_raw() >= 0 {
        // Há jogos (Godot) que olham o resultado de cada swapchain, não o da função.
        if !(*info).p_results.is_null() {
            std::slice::from_raw_parts_mut((*info).p_results, (*info).swapchain_count as usize).fill(vk::Result::ERROR_OUT_OF_DATE_KHR);
        }
        vk::Result::ERROR_OUT_OF_DATE_KHR
    } else {
        r
    }
}

unsafe fn vk_swapchain(d: vk::Device, ci: &vk::SwapchainCreateInfoKHR, sc: vk::SwapchainKHR, ours: bool) {
    let g = lock(&VK_INST);
    let Some((inst, cards)) = g.as_ref() else { return };
    let mut devs = lock(&VK_DEVS);
    let key = vk_key(d);
    if !devs.iter().any(|x| x.0 == key) {
        let Some(s) = shared() else { return };
        // A placa pelo driver que atende o dispositivo do jogo (o Present dele é o de uma das nossas).
        let present = (inst.fp_v1_0().get_device_proc_addr)(d, VK_NAMES[VK_PRESENT].as_ptr()).map_or(0, |f| f as usize);
        // Com duas placas no mesmo driver, pelo par anotado no vkCreateDevice.
        let same: Vec<&VkCard> = cards.iter().filter(|c| c.0[VK_PRESENT] == present).collect();
        let pd = match same.as_slice() {
            [] => cards[0].1,
            [one] => one.1,
            many => {
                let id = lock(&VK_IDS).iter().find(|x| x.0 == key).map(|x| x.1);
                match many.iter().find(|c| Some(c.2) == id) {
                    Some(c) => c.1,
                    None => {
                        say!("shader: Vulkan, duas placas no mesmo driver e o dispositivo do jogo é de antes da injeção; efeito desligado nele");
                        return;
                    }
                }
            }
        };
        match VkDev::new(inst, pd, d, s) {
            Ok(x) => devs.push((key, x)),
            Err(e) => {
                say!("shader: Vulkan não abriu: {e}");
                return;
            }
        }
    }
    let x = &mut devs.iter_mut().find(|x| x.0 == key).unwrap().1;
    let images = ash::khr::swapchain::Device::new(inst, &x.dev).get_swapchain_images(sc).unwrap_or_default();
    // Só 8 bits por canal, como no DXGI: HDR (10 ou 16 bits) fica sem efeito.
    let eight = matches!(ci.image_format, vk::Format::B8G8R8A8_UNORM | vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_UNORM | vk::Format::R8G8B8A8_SRGB | vk::Format::A8B8G8R8_UNORM_PACK32 | vk::Format::A8B8G8R8_SRGB_PACK32);
    if !ours {
        say!("shader: a swapchain recusou a cópia (TRANSFER_SRC), efeito desligado nela");
    } else if !eight {
        say!("shader: swapchain em {:?} (HDR), efeito desligado nela", ci.image_format);
    }
    // Uma swapchain por dispositivo recebe o efeito: a primeira, e quem a substitui (`old_swapchain`).
    if x.main.is_none_or(|m| m == ci.old_swapchain) {
        x.main = Some(sc);
    }
    x.scs.push((sc, Sc { format: ci.image_format, size: (ci.image_extent.width, ci.image_extent.height), images, ok: ours && eight }));
}

/// O semáforo que o Present passa a esperar (o fim da cadeia), e se o jogo deve refazer a swapchain.
unsafe fn vk_frame(q: vk::Queue, info: &vk::PresentInfoKHR) -> (Option<vk::Semaphore>, bool) {
    let Some(s) = shared() else { return (None, false) };
    s.presents.fetch_add(1, Relaxed);
    s.api.store(API_VK, Relaxed);
    if !alive(s) {
        return (None, false);
    }
    let scs = vk_list(info.p_swapchains, info.swapchain_count);
    let idx = vk_list(info.p_image_indices, info.swapchain_count);
    let mut devs = lock(&VK_DEVS);
    let known = |sc: &vk::SwapchainKHR| devs.iter().any(|x| x.1.scs.iter().any(|s| s.0 == *sc));
    if scs.iter().any(|sc| !known(sc)) && !VK_NUDGED.swap(true, Relaxed) {
        say!("shader: Vulkan, swapchain de antes da injeção; o jogo vai refazê-la");
        return (None, true);
    }
    // A fila tem a chave do dispositivo dela; com um dispositivo só, ele mesmo.
    let key = vk_key(q);
    let one = devs.len() == 1;
    let Some((_, x)) = devs.iter_mut().find(|x| x.0 == key || one) else { return (None, false) };
    if x.errors >= MAX_ERRORS {
        return (None, false);
    }
    let target = scs.iter().zip(idx).find_map(|(sc, i)| x.scs.iter().find(|s| s.0 == *sc && s.1.ok && x.main == Some(s.0)).and_then(|s| Some((s.1.images.get(*i as usize).copied()?, s.1.format, s.1.size))));
    let waits = vk_list(info.p_wait_semaphores, info.wait_semaphore_count);
    match x.frame(s, q, target, waits) {
        Ok(sem) => {
            x.errors = 0;
            (sem, false)
        }
        Err(e) => {
            x.errors += 1;
            say!("shader: {e}");
            if x.errors >= MAX_ERRORS {
                say!("shader: {MAX_ERRORS} erros seguidos, efeito desligado neste dispositivo");
            }
            (None, false)
        }
    }
}

struct Sc {
    format: vk::Format,
    size: (u32, u32),
    images: Vec<vk::Image>,
    /// Criada com a cópia liberada.
    ok: bool,
}

/// Imagem nossa na memória da placa.
struct VkImg {
    size: (u32, u32),
    format: vk::Format,
    image: vk::Image,
    mem: vk::DeviceMemory,
}

/// As faixas do meio para o detector, numa memória que a CPU lê.
struct VkStrips {
    size: (u32, u32),
    s: u32,
    buf: vk::Buffer,
    mem: vk::DeviceMemory,
    ptr: *const u32,
    /// Copiadas no comando deste quadro em voo; lidas quando a cerca dele abrir.
    pending: Option<usize>,
}

struct VkDev {
    inst: ash::Instance,
    pd: vk::PhysicalDevice,
    dev: ash::Device,
    mem: vk::PhysicalDeviceMemoryProperties,
    t: Tracker,
    chain: Option<ChainVk>,
    defaults: HashMap<String, f32>,
    /// Perfil com que a cadeia não abriu (não tentar de novo a cada quadro).
    failed: Option<(u32, Option<std::time::SystemTime>)>,
    polled: Instant,
    seq: u32,
    pool: vk::CommandPool,
    slots: Vec<(vk::CommandBuffer, vk::Fence, vk::Semaphore)>,
    slot: usize,
    scs: Vec<(vk::SwapchainKHR, Sc)>,
    /// A swapchain que recebe o efeito (duas janelas não dividem a mesma entrada e o mesmo detector).
    main: Option<vk::SwapchainKHR>,
    input: Option<VkImg>,
    small: Option<VkImg>,
    strips: Option<VkStrips>,
    detect: blocks::Detect,
    frame: usize,
    errors: u32,
}

// Os objetos Vulkan só são usados debaixo do `Mutex`.
unsafe impl Send for VkDev {}

impl Drop for VkDev {
    fn drop(&mut self) {
        unsafe {
            self.wait();
            self.chain = None;
            for i in [self.input.take(), self.small.take()].into_iter().flatten() {
                self.dev.destroy_image(i.image, None);
                self.dev.free_memory(i.mem, None);
            }
            if let Some(st) = self.strips.take() {
                self.dev.destroy_buffer(st.buf, None);
                self.dev.free_memory(st.mem, None);
            }
            for (_, f, s) in self.slots.drain(..) {
                self.dev.destroy_fence(f, None);
                self.dev.destroy_semaphore(s, None);
            }
            self.dev.destroy_command_pool(self.pool, None);
        }
    }
}

impl VkDev {
    unsafe fn new(inst: &ash::Instance, pd: vk::PhysicalDevice, d: vk::Device, s: &Shared) -> Res<Self> {
        let t = Tracker::open(&s.path())?;
        let gdpa = inst.fp_v1_0().get_device_proc_addr;
        let dev = ash::Device::load_with(|n| gdpa(d, n.as_ptr()).map_or(std::ptr::null(), |f| f as *const c_void), d);
        // shortcut: a fila de desenho é a primeira família com gráficos da placa; um jogo que apresenta por uma família só de apresentação não ganha efeito.
        let fam = vk_graphics_family(inst, pd).ok_or("a placa não tem fila de gráficos")?;
        let pool = dev.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(fam).flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER), None)?;
        let mut slots = Vec::new();
        let made = (|| -> Res<()> {
            for c in dev.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool).command_buffer_count(VK_SLOTS as u32))? {
                let f = dev.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None)?;
                let sm = dev.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).inspect_err(|_| dev.destroy_fence(f, None))?;
                slots.push((c, f, sm));
            }
            Ok(())
        })();
        if let Err(e) = made {
            for (_, f, sm) in slots {
                dev.destroy_fence(f, None);
                dev.destroy_semaphore(sm, None);
            }
            dev.destroy_command_pool(pool, None);
            return Err(e);
        }
        let props = inst.get_physical_device_properties(pd);
        let name = std::ffi::CStr::from_ptr(props.device_name.as_ptr()).to_string_lossy().to_string();
        let x = Self { inst: inst.clone(), pd, mem: inst.get_physical_device_memory_properties(pd), dev, t, chain: None, defaults: HashMap::new(), failed: None, polled: Instant::now(), seq: s.seq.load(Acquire), pool, slots, slot: 0, scs: Vec::new(), main: None, input: None, small: None, strips: None, detect: Default::default(), frame: 0, errors: 0 };
        say!("shader: Vulkan pronto em {name}, perfil {}", x.t.path.display());
        Ok(x)
    }

    /// Espera os nossos comandos em voo.
    unsafe fn wait(&self) {
        let f: Vec<vk::Fence> = self.slots.iter().map(|s| s.1).collect();
        let _ = self.dev.wait_for_fences(&f, true, 1_000_000_000);
    }

    /// A cadeia usa a fila do jogo para subir as tabelas (LUT): só na linha que apresenta, que a tem agora.
    unsafe fn build(&mut self, q: vk::Queue, s: &Shared) {
        self.wait();
        self.chain = None;
        self.defaults.clear();
        let paths = self.t.on_paths();
        if paths.is_empty() {
            return;
        }
        let t = Instant::now();
        let opts = librashader::runtime::vk::FilterChainOptions { frames_in_flight: VK_SLOTS as u32, ..Default::default() };
        match chain::join(&paths).and_then(|p| ChainVk::load_from_preset(p, (self.pd, self.inst.clone(), self.dev.clone(), q), Some(&opts)).map_err(Into::into)) {
            Ok(c) => {
                self.defaults = chain::defaults(&c);
                chain::apply(&c, &self.defaults, &self.t.profile);
                self.chain = Some(c);
                say!("shader: cadeia Vulkan de {} preset(s) carregada em {} ms", paths.len(), t.elapsed().as_millis());
            }
            Err(e) => {
                self.failed = Some(fail_key(s));
                say!("shader: {e}");
            }
        }
    }

    unsafe fn image(&self, w: u32, h: u32, format: vk::Format, usage: vk::ImageUsageFlags) -> Res<VkImg> {
        let ci = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width: w, height: h, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage);
        let image = self.dev.create_image(&ci, None)?;
        let req = self.dev.get_image_memory_requirements(image);
        let mem = self.alloc(req, vk::MemoryPropertyFlags::DEVICE_LOCAL).inspect_err(|_| self.dev.destroy_image(image, None))?;
        if let Err(e) = self.dev.bind_image_memory(image, mem, 0) {
            self.dev.destroy_image(image, None);
            self.dev.free_memory(mem, None);
            return Err(e.into());
        }
        Ok(VkImg { size: (w, h), format, image, mem })
    }

    unsafe fn alloc(&self, req: vk::MemoryRequirements, want: vk::MemoryPropertyFlags) -> Res<vk::DeviceMemory> {
        let types = &self.mem.memory_types[..self.mem.memory_type_count as usize];
        let i = types.iter().enumerate().position(|(i, t)| req.memory_type_bits & (1 << i) != 0 && t.property_flags.contains(want)).ok_or("sem memória para a imagem")?;
        Ok(self.dev.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(i as u32), None)?)
    }

    /// Refaz `slot` quando o tamanho ou o formato mudam (depois de esperar os comandos que podem usar a antiga).
    unsafe fn fit(&mut self, small: bool, w: u32, h: u32, format: vk::Format) -> Res<()> {
        let cur = if small { &self.small } else { &self.input };
        if cur.as_ref().is_some_and(|i| i.size == (w, h) && i.format == format) {
            return Ok(());
        }
        self.wait();
        let new = self.image(w, h, format, vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)?;
        let slot = if small { &mut self.small } else { &mut self.input };
        if let Some(old) = slot.replace(new) {
            self.dev.destroy_image(old.image, None);
            self.dev.free_memory(old.mem, None);
        }
        Ok(())
    }

    unsafe fn barrier(&self, cmd: vk::CommandBuffer, image: vk::Image, from: (vk::ImageLayout, vk::AccessFlags, vk::PipelineStageFlags), to: (vk::ImageLayout, vk::AccessFlags, vk::PipelineStageFlags)) {
        let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
        let b = vk::ImageMemoryBarrier::default()
            .image(image)
            .old_layout(from.0)
            .src_access_mask(from.1)
            .new_layout(to.0)
            .dst_access_mask(to.1)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .subresource_range(range);
        self.dev.cmd_pipeline_barrier(cmd, from.2, to.2, vk::DependencyFlags::empty(), &[], &[], &[b]);
    }

    /// Com a fila do jogo, na linha que apresenta. Devolve o semáforo do fim da cadeia, se desenhou.
    unsafe fn frame(&mut self, s: &Shared, q: vk::Queue, target: Option<(vk::Image, vk::Format, (u32, u32))>, waits: &[vk::Semaphore]) -> Res<Option<vk::Semaphore>> {
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
                if let Some(st) = self.strips.as_mut() {
                    st.pending = None;
                }
            }
            if c.chain {
                self.wait();
                self.chain = None;
                self.failed = None;
            } else if let Some(ch) = self.chain.as_ref().filter(|_| c.params) {
                chain::apply(ch, &self.defaults, &self.t.profile);
            }
        }
        let p = &self.t.profile;
        if !p.on || !p.inject || s.flags.load(Relaxed) & FLAG_COMPARE != 0 {
            return Ok(None);
        }
        let Some((sc_img, format, (w, h))) = target else { return Ok(None) };
        if self.chain.is_none() && self.failed != Some(fail_key(s)) {
            self.build(q, s);
        }
        if self.chain.is_none() || w == 0 || h == 0 {
            return Ok(None);
        }
        let (cmd, fence, sem) = self.slots[self.slot];
        self.dev.wait_for_fences(&[fence], true, 1_000_000_000)?;
        self.dev.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
        // As faixas copiadas há alguns quadros: a cerca daquele quadro já abriu.
        if let Some(st) = self.strips.as_mut().filter(|st| st.pending.is_some_and(|p| self.dev.get_fence_status(self.slots[p].1) == Ok(true))) {
            st.pending = None;
            let (sw, sh, n) = (st.size.0 as usize, st.size.1 as usize, st.s as usize);
            let all = std::slice::from_raw_parts(st.ptr, sw * n + n * sh);
            let (a, b): (Vec<u32>, Vec<u32>) = (all[..sw * n].iter().map(|p| p & 0xFF_FFFF).collect(), all[sw * n..].iter().map(|p| p & 0xFF_FFFF).collect());
            self.detect.vote(blocks::detect(&a, sw, &b, sh, n));
        }
        self.dev.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))?;
        let r = self.record(cmd, sc_img, format, w, h);
        self.dev.end_command_buffer(cmd)?;
        // Com erro nada vai para a fila: o jogo apresenta como entregou e a cerca continua aberta.
        if let Err(e) = r {
            if let Some(st) = self.strips.as_mut().filter(|st| st.pending == Some(self.slot)) {
                st.pending = None;
            }
            return Err(e);
        }
        self.dev.reset_fences(&[fence])?;
        let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; waits.len()];
        let (cmds, sig) = ([cmd], [sem]);
        let submit = vk::SubmitInfo::default().wait_semaphores(waits).wait_dst_stage_mask(&stages).command_buffers(&cmds).signal_semaphores(&sig);
        if let Err(e) = self.dev.queue_submit(q, &[submit], fence) {
            // A cerca já foi fechada e nada vai abri-la: uma nova, aberta, no lugar (senão este espaço espera 1 s a cada volta).
            if let Ok(f) = self.dev.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None) {
                self.dev.destroy_fence(fence, None);
                self.slots[self.slot].1 = f;
            }
            if let Some(st) = self.strips.as_mut().filter(|st| st.pending == Some(self.slot)) {
                st.pending = None;
            }
            return Err(e.into());
        }
        self.slot = (self.slot + 1) % VK_SLOTS;
        self.frame += 1;
        s.drawn.fetch_add(1, Relaxed);
        Ok(Some(sem))
    }

    unsafe fn record(&mut self, cmd: vk::CommandBuffer, sc_img: vk::Image, format: vk::Format, w: u32, h: u32) -> Res<()> {
        use vk::{AccessFlags as A, ImageLayout as L, PipelineStageFlags as P};
        let color = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        self.barrier(cmd, sc_img, (L::PRESENT_SRC_KHR, A::MEMORY_WRITE, P::ALL_COMMANDS), (L::TRANSFER_SRC_OPTIMAL, A::TRANSFER_READ, P::TRANSFER));
        // `auto`: o detector de blocos com as faixas do meio (a swapchain já é de 8 bits).
        if self.t.profile.input == "auto" {
            self.copy_strips(cmd, sc_img, w, h)?;
        }
        let k = blocks::reduce_for(&self.t.profile.input, h, self.detect.found);
        let size = k.map_or((w, h), |k| blocks::reduced_size(w, h, k));
        if k.is_some() && self.small.as_ref().is_none_or(|i| i.size != size) {
            say!("shader: entrada {w}x{h} → {}x{}", size.0, size.1);
        }
        self.fit(k.is_some(), size.0, size.1, format)?;
        let dst = if k.is_some() { self.small.as_ref() } else { self.input.as_ref() }.unwrap().image;
        self.barrier(cmd, dst, (L::UNDEFINED, A::SHADER_READ, P::FRAGMENT_SHADER), (L::TRANSFER_DST_OPTIMAL, A::TRANSFER_WRITE, P::TRANSFER));
        match k {
            // Um ponto do centro de cada bloco, como o PS_REDUCE.
            Some(k) => {
                let (x0, y0) = (k[2].round() as i32, k[3].round() as i32);
                let (x1, y1) = (((k[2] + size.0 as f32 * k[0]).round() as i32).min(w as i32), ((k[3] + size.1 as f32 * k[1]).round() as i32).min(h as i32));
                let region = vk::ImageBlit::default()
                    .src_subresource(color)
                    .src_offsets([vk::Offset3D { x: x0, y: y0, z: 0 }, vk::Offset3D { x: x1, y: y1, z: 1 }])
                    .dst_subresource(color)
                    .dst_offsets([vk::Offset3D::default(), vk::Offset3D { x: size.0 as i32, y: size.1 as i32, z: 1 }]);
                self.dev.cmd_blit_image(cmd, sc_img, L::TRANSFER_SRC_OPTIMAL, dst, L::TRANSFER_DST_OPTIMAL, &[region], vk::Filter::NEAREST);
            }
            None => {
                let region = vk::ImageCopy::default().src_subresource(color).dst_subresource(color).extent(vk::Extent3D { width: w, height: h, depth: 1 });
                self.dev.cmd_copy_image(cmd, sc_img, L::TRANSFER_SRC_OPTIMAL, dst, L::TRANSFER_DST_OPTIMAL, &[region]);
            }
        }
        self.barrier(cmd, dst, (L::TRANSFER_DST_OPTIMAL, A::TRANSFER_WRITE, P::TRANSFER), (L::SHADER_READ_ONLY_OPTIMAL, A::SHADER_READ, P::FRAGMENT_SHADER));
        self.barrier(cmd, sc_img, (L::TRANSFER_SRC_OPTIMAL, A::TRANSFER_READ, P::TRANSFER), (L::COLOR_ATTACHMENT_OPTIMAL, A::COLOR_ATTACHMENT_WRITE, P::COLOR_ATTACHMENT_OUTPUT));
        let input = ChainVkImage { image: dst, size: Size::new(size.0, size.1), format };
        let out = ChainVkImage { image: sc_img, size: Size::new(w, h), format };
        let vp = Viewport { x: 0.0, y: 0.0, mvp: None, output: out, size: Size::new(w, h) };
        let r = self.chain.as_mut().unwrap().frame(&input, &vp, cmd, self.frame, None);
        // O jogo recebe a imagem como entregou, mesmo quando a cadeia falha no meio.
        self.barrier(cmd, sc_img, (L::COLOR_ATTACHMENT_OPTIMAL, A::COLOR_ATTACHMENT_WRITE, P::COLOR_ATTACHMENT_OUTPUT), (L::PRESENT_SRC_KHR, A::empty(), P::BOTTOM_OF_PIPE));
        Ok(r?)
    }

    /// Faixa horizontal e vertical do meio para a memória que a CPU lê, no quadro em voo atual.
    unsafe fn copy_strips(&mut self, cmd: vk::CommandBuffer, sc_img: vk::Image, w: u32, h: u32) -> Res<()> {
        if self.strips.as_ref().is_none_or(|st| st.size != (w, h)) {
            self.wait();
            self.detect = Default::default();
            if let Some(st) = self.strips.take() {
                self.dev.destroy_buffer(st.buf, None);
                self.dev.free_memory(st.mem, None);
            }
            let n = 32.min(w).min(h);
            let bytes = 4 * (w * n + n * h) as u64;
            let buf = self.dev.create_buffer(&vk::BufferCreateInfo::default().size(bytes).usage(vk::BufferUsageFlags::TRANSFER_DST), None)?;
            let req = self.dev.get_buffer_memory_requirements(buf);
            let mem = self.alloc(req, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT).inspect_err(|_| self.dev.destroy_buffer(buf, None))?;
            let ptr = match self.dev.bind_buffer_memory(buf, mem, 0).and_then(|_| self.dev.map_memory(mem, 0, bytes, vk::MemoryMapFlags::empty())) {
                Ok(p) => p as *const u32,
                Err(e) => {
                    self.dev.destroy_buffer(buf, None);
                    self.dev.free_memory(mem, None);
                    return Err(e.into());
                }
            };
            self.strips = Some(VkStrips { size: (w, h), s: n, buf, mem, ptr, pending: None });
        }
        let st = self.strips.as_mut().unwrap();
        if st.pending.is_some() || !self.detect.due() {
            return Ok(());
        }
        let n = st.s;
        let color = vk::ImageSubresourceLayers::default().aspect_mask(vk::ImageAspectFlags::COLOR).layer_count(1);
        let regions = [
            vk::BufferImageCopy::default().image_subresource(color).image_offset(vk::Offset3D { x: 0, y: ((h - n) / 2) as i32, z: 0 }).image_extent(vk::Extent3D { width: w, height: n, depth: 1 }),
            vk::BufferImageCopy::default().buffer_offset(4 * (w * n) as u64).image_subresource(color).image_offset(vk::Offset3D { x: ((w - n) / 2) as i32, y: 0, z: 0 }).image_extent(vk::Extent3D { width: n, height: h, depth: 1 }),
        ];
        self.dev.cmd_copy_image_to_buffer(cmd, sc_img, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, st.buf, &regions);
        let b = vk::BufferMemoryBarrier::default().buffer(st.buf).size(vk::WHOLE_SIZE).src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::HOST_READ).src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED);
        self.dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::HOST, vk::DependencyFlags::empty(), &[], &[b], &[]);
        st.pending = Some(self.slot);
        Ok(())
    }
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
    // Vulkan primeiro: na NVIDIA o Present dele passa por uma swapchain DXGI do driver, que o gancho DXGI deve ignorar.
    // As outras APIs vêm com a DLL (importadas por ela); a vulkan-1.dll só existe se o jogo a carregou.
    let vk_in = || GetModuleHandleW(w!("vulkan-1.dll")).is_ok();
    let late = !vk_in();
    let vk = if late { Err("a vulkan-1.dll ainda não carregou".into()) } else { install_vk() };
    let (dxgi, d12, d9, gl) = (install_dxgi(), install_d12(), install_d9(), install_gl());
    say!("shader: injeção no pid {}: DXGI {:?}, D3D12 {:?}, D3D9 {:?}, OpenGL {:?}, Vulkan {:?}", GetCurrentProcessId(), dxgi, d12, d9, gl, vk);
    s.state.store(if dxgi.is_ok() || d9.is_ok() || gl.is_ok() || vk.is_ok() || late { STATE_READY } else { STATE_FAILED }, SeqCst);
    // Jogo que abre a janela antes de carregar o Vulkan: o desvio entra quando a DLL dele aparecer (a swapchain
    // feita antes disso passa pelo OUT_OF_DATE do Present).
    if late {
        while !vk_in() {
            Sleep(500);
        }
        say!("shader: Vulkan carregou depois da injeção: {:?}", install_vk());
    }
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

    /// Swapchain de D3D12 numa janela escondida, como a de um jogo.
    unsafe fn d3d12_swapchain() -> (ID3D12Device, ID3D12CommandQueue, IDXGISwapChain3) {
        let mut dev: Option<ID3D12Device> = None;
        D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev).unwrap();
        let dev = dev.unwrap();
        let queue: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
        let factory: IDXGIFactory2 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
        let desc = DXGI_SWAP_CHAIN_DESC1 { Width: 320, Height: 240, Format: DXGI_FORMAT_B8G8R8A8_UNORM, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT, BufferCount: 2, SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD, ..Default::default() };
        let sc = factory.CreateSwapChainForHwnd(&queue, dummy_window().unwrap(), &desc, None, None).unwrap();
        (dev, queue, sc.cast().unwrap())
    }

    /// A swapchain do D3D12 dá o dispositivo mas não a fila (vem do `ExecuteCommandLists`); com os dois, o 11on12
    /// embrulha os back buffers e a cópia passa.
    #[test]
    fn d3d12_por_11on12() {
        unsafe {
            let (dev, queue, sc) = d3d12_swapchain();
            assert!(sc.GetDevice::<ID3D11Device>().is_err() && sc.GetDevice::<ID3D10Device>().is_err());
            assert!(sc.GetDevice::<ID3D12CommandQueue>().is_err());
            assert_eq!(sc.GetDevice::<ID3D12Device>().unwrap(), dev);
            let (d11, on12) = on12_device(&dev, &queue).unwrap();
            let ctx = d11.GetImmediateContext().unwrap();
            let flags = D3D11_RESOURCE_FLAGS { BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32, ..Default::default() };
            let mut back: Option<ID3D11Texture2D> = None;
            on12.CreateWrappedResource(&sc.GetBuffer::<ID3D12Resource>(sc.GetCurrentBackBufferIndex()).unwrap(), &flags, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_PRESENT, &mut back).unwrap();
            let back = back.unwrap();
            let (raw, _) = texture(&d11, 320, 240, DXGI_FORMAT_B8G8R8A8_UNORM, false).unwrap();
            let res = [Some(back.cast::<ID3D11Resource>().unwrap())];
            on12.AcquireWrappedResources(&res);
            ctx.CopyResource(&raw, &back);
            ctx.ClearRenderTargetView(&rtv(&d11, &back).unwrap(), &[0.0, 0.5, 0.0, 1.0]);
            on12.ReleaseWrappedResources(&res);
            ctx.Flush();
            sc.Present(0, DXGI_PRESENT(0)).ok().unwrap();
        }
    }

    /// O Vulkan passa pelos desvios sem tropeçar: a swapchain nasce com as nossas flags de uso, apresenta e
    /// some pelos trampolins (o desvio ligado antes do trampolim gravado derrubava o jogo). Limite: sem o motor
    /// (`Shared`) a cadeia não roda aqui; o efeito em Vulkan se confere num jogo de verdade (RetroArch em Vulkan).
    #[test]
    fn vulkan_pelos_desvios() {
        unsafe {
            let entry = ash::Entry::load().unwrap();
            install_vk().unwrap();
            let ext = [ash::khr::surface::NAME.as_ptr(), ash::khr::win32_surface::NAME.as_ptr()];
            let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
            let inst = entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app).enabled_extension_names(&ext), None).unwrap();
            let hinst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
            let class = w!("PrismaTesteVK");
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: class, ..Default::default() });
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, class, WS_OVERLAPPEDWINDOW, 0, 0, 160, 120, None, None, Some(hinst), None).unwrap();
            let si = vk::Win32SurfaceCreateInfoKHR::default().hinstance(hinst.0 as isize).hwnd(hwnd.0 as isize);
            let surface = ash::khr::win32_surface::Instance::new(&entry, &inst).create_win32_surface(&si, None).unwrap();
            let sfn = ash::khr::surface::Instance::new(&entry, &inst);
            let (pd, fam) = inst.enumerate_physical_devices().unwrap().into_iter().find_map(|pd| {
                let fam = vk_graphics_family(&inst, pd)?;
                sfn.get_physical_device_surface_support(pd, fam, surface).unwrap_or(false).then_some((pd, fam))
            }).unwrap();
            let prio = [1.0];
            let qi = [vk::DeviceQueueCreateInfo::default().queue_family_index(fam).queue_priorities(&prio)];
            let dext = [ash::khr::swapchain::NAME.as_ptr()];
            let dev = inst.create_device(pd, &vk::DeviceCreateInfo::default().queue_create_infos(&qi).enabled_extension_names(&dext), None).unwrap();
            let q = dev.get_device_queue(fam, 0);
            let caps = sfn.get_physical_device_surface_capabilities(pd, surface).unwrap();
            let format = sfn.get_physical_device_surface_formats(pd, surface).unwrap()[0];
            let ci = vk::SwapchainCreateInfoKHR::default()
                .surface(surface)
                .min_image_count(caps.min_image_count.max(2))
                .image_format(format.format)
                .image_color_space(format.color_space)
                .image_extent(caps.current_extent)
                .image_array_layers(1)
                .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
                .pre_transform(caps.current_transform)
                .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
                .present_mode(vk::PresentModeKHR::FIFO);
            let scfn = ash::khr::swapchain::Device::new(&inst, &dev);
            let sc = scfn.create_swapchain(&ci, None).unwrap();
            let fence = dev.create_fence(&vk::FenceCreateInfo::default(), None).unwrap();
            for _ in 0..3 {
                let (i, _) = scfn.acquire_next_image(sc, u64::MAX, vk::Semaphore::null(), fence).unwrap();
                dev.wait_for_fences(&[fence], true, u64::MAX).unwrap();
                dev.reset_fences(&[fence]).unwrap();
                // A imagem sai de UNDEFINED para PRESENT_SRC pela fila, como num jogo que desenhou nela.
                let pool = dev.create_command_pool(&vk::CommandPoolCreateInfo::default().queue_family_index(fam), None).unwrap();
                let cmd = dev.allocate_command_buffers(&vk::CommandBufferAllocateInfo::default().command_pool(pool).command_buffer_count(1)).unwrap()[0];
                dev.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default()).unwrap();
                let range = vk::ImageSubresourceRange::default().aspect_mask(vk::ImageAspectFlags::COLOR).level_count(1).layer_count(1);
                let img = scfn.get_swapchain_images(sc).unwrap()[i as usize];
                let b = vk::ImageMemoryBarrier::default().image(img).subresource_range(range).old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::PRESENT_SRC_KHR);
                dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::BOTTOM_OF_PIPE, vk::DependencyFlags::empty(), &[], &[], &[b]);
                dev.end_command_buffer(cmd).unwrap();
                dev.queue_submit(q, &[vk::SubmitInfo::default().command_buffers(&[cmd])], fence).unwrap();
                dev.wait_for_fences(&[fence], true, u64::MAX).unwrap();
                dev.reset_fences(&[fence]).unwrap();
                dev.destroy_command_pool(pool, None);
                let (scs, idx) = ([sc], [i]);
                scfn.queue_present(q, &vk::PresentInfoKHR::default().swapchains(&scs).image_indices(&idx)).unwrap();
            }
            dev.device_wait_idle().unwrap();
            dev.destroy_fence(fence, None);
            scfn.destroy_swapchain(sc, None);
            dev.destroy_device(None);
            sfn.destroy_surface(surface, None);
            inst.destroy_instance(None);
            let _ = DestroyWindow(hwnd);
        }
    }

    const IN: (u32, u32) = (240, 160);
    const OUT: (u32, u32) = (720, 480);

    /// Xadrez colorido em BGRA, de cima para baixo.
    fn picture() -> Vec<u8> {
        let (w, h) = IN;
        (0..h).flat_map(|y| (0..w).flat_map(move |x| [(y * 255 / h) as u8, (x * 255 / w) as u8, if (x / 8 + y / 8) % 2 == 0 { 230 } else { 40 }, 255])).collect()
    }

    unsafe fn run_d3d11(preset: &str, frames: usize) -> Vec<u32> {
        use librashader::runtime::d3d11::FilterChain;
        let (mut dev, mut ctx) = (None, None);
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), None, D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx)).unwrap();
        let (dev, ctx) = (dev.unwrap(), ctx.unwrap());
        let mut desc = D3D11_TEXTURE2D_DESC { Width: IN.0, Height: IN.1, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_B8G8R8A8_UNORM, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, CPUAccessFlags: 0, MiscFlags: 0 };
        let img = picture();
        let data = D3D11_SUBRESOURCE_DATA { pSysMem: img.as_ptr() as _, SysMemPitch: IN.0 * 4, SysMemSlicePitch: 0 };
        let (mut t, mut srv) = (None, None);
        dev.CreateTexture2D(&desc, Some(&data), Some(&mut t)).unwrap();
        dev.CreateShaderResourceView(&t.unwrap(), None, Some(&mut srv)).unwrap();
        let (out, _) = texture(&dev, OUT.0, OUT.1, DXGI_FORMAT_B8G8R8A8_UNORM, true).unwrap();
        let rt = rtv(&dev, &out).unwrap();
        let mut chain = FilterChain::load_from_preset(chain::join(&[preset.into()]).unwrap(), &dev, None).unwrap();
        let vp = Viewport { x: 0.0, y: 0.0, mvp: None, output: &rt, size: Size::new(OUT.0, OUT.1) };
        for f in 0..frames {
            chain.frame(None, srv.as_ref().unwrap(), &vp, f, None).unwrap();
        }
        (desc.Width, desc.Height, desc.Usage, desc.BindFlags, desc.CPUAccessFlags) = (OUT.0, OUT.1, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32);
        let mut st = None;
        dev.CreateTexture2D(&desc, None, Some(&mut st)).unwrap();
        let st = st.unwrap();
        ctx.CopyResource(&st, &out);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m)).unwrap();
        let px = (0..OUT.1).flat_map(|y| std::slice::from_raw_parts((m.pData as *const u8).add((y * m.RowPitch) as usize) as *const u32, OUT.0 as usize).iter().map(|p| p & 0xFF_FFFF)).collect();
        ctx.Unmap(&st, 0);
        px
    }

    unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        DefWindowProcW(h, m, w, l)
    }

    /// Como no jogo: a imagem sobe de baixo para cima (a do back buffer) e a saída é lida pelo `read_gl`.
    unsafe fn run_gl(preset: &str, frames: usize) -> Vec<u32> {
        use windows::Win32::Graphics::Gdi::{GetDC, ReleaseDC};
        let inst: HINSTANCE = GetModuleHandleW(None).unwrap().into();
        let class = w!("PrismaTesteGL");
        RegisterClassW(&WNDCLASSW { style: CS_OWNDC, lpfnWndProc: Some(wndproc), hInstance: inst, lpszClassName: class, ..Default::default() });
        let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, class, WS_OVERLAPPEDWINDOW, 0, 0, 64, 64, None, None, Some(inst), None).unwrap();
        let dc = GetDC(Some(hwnd));
        let pfd = PIXELFORMATDESCRIPTOR { nSize: size_of::<PIXELFORMATDESCRIPTOR>() as u16, nVersion: 1, dwFlags: PFD_DRAW_TO_WINDOW | PFD_SUPPORT_OPENGL | PFD_DOUBLEBUFFER, iPixelType: PFD_TYPE_RGBA, cColorBits: 32, ..Default::default() };
        SetPixelFormat(dc, ChoosePixelFormat(dc, &pfd), &pfd).unwrap();
        let rc = wglCreateContext(dc).unwrap();
        wglMakeCurrent(dc, rc).unwrap();
        let gl = Arc::new(glow_context());
        let (mut input, mut out) = (None, None);
        Tex::fit(&gl, &mut input, IN.0, IN.1).unwrap();
        Tex::fit(&gl, &mut out, OUT.0, OUT.1).unwrap();
        let (input, out) = (input.unwrap(), out.unwrap());
        let up: Vec<u8> = picture().chunks_exact(IN.0 as usize * 4).rev().flatten().copied().collect();
        gl.bind_texture(glow::TEXTURE_2D, Some(input.tex));
        gl.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, 0, IN.0 as i32, IN.1 as i32, glow::BGRA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&up)));
        let mut chain = ChainGl::load_from_preset(chain::join(&[preset.into()]).unwrap(), gl.clone(), None).unwrap();
        let vp = Viewport { x: 0.0, y: 0.0, mvp: None, output: &out.image(), size: Size::new(OUT.0, OUT.1) };
        for f in 0..frames {
            chain.frame(&input.image(), &vp, f, None).unwrap();
        }
        let px = read_gl(&gl, &out);
        drop(chain);
        let _ = wglMakeCurrent(HDC::default(), HGLRC::default());
        let _ = wglDeleteContext(rc);
        ReleaseDC(Some(hwnd), dc);
        let _ = DestroyWindow(hwnd);
        px
    }

    /// A cadeia OpenGL dá a mesma imagem que a D3D11. Precisa de um preset: `PRISMA_TEST_PRESET=<.slangp>`;
    /// com `PRISMA_TEST_OUT=<pasta>` grava as duas saídas em BMP.
    #[test]
    fn gl_igual_d3d11() {
        let Ok(preset) = std::env::var("PRISMA_TEST_PRESET") else { return };
        let frames = 8;
        let (a, b) = unsafe { (run_d3d11(&preset, frames), run_gl(&preset, frames)) };
        let ch = |v: &[u32]| v.iter().flat_map(|p| p.to_le_bytes().into_iter().take(3)).map(f64::from).collect::<Vec<_>>();
        let (ca, cb) = (ch(&a), ch(&b));
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        let diff = |x: &[f64], y: &[f64]| x.iter().zip(y).map(|(p, q)| (p - q).abs()).sum::<f64>() / x.len() as f64;
        // O GL sai uma linha acima (as linhas do quadro contam de baixo para cima); o mesmo acontece no RetroArch em GL.
        let row = OUT.0 as usize * 3;
        let shifted = diff(&ca[..ca.len() - row], &cb[row..]);
        eprintln!("média d3d11 {:.1}, gl {:.1}; diferença {:.2}, com o gl uma linha acima {:.2}", mean(&ca), mean(&cb), diff(&ca, &cb), shifted);
        if let Ok(dir) = std::env::var("PRISMA_TEST_OUT") {
            for (name, px) in [("d3d11.bmp", &a), ("gl.bmp", &b)] {
                let mut f = Vec::with_capacity(54 + px.len() * 4);
                let size = (54 + px.len() * 4) as u32;
                f.extend(b"BM".iter().copied().chain(size.to_le_bytes()).chain([0; 4]).chain(54u32.to_le_bytes()));
                f.extend(40u32.to_le_bytes().into_iter().chain((OUT.0 as i32).to_le_bytes()).chain((-(OUT.1 as i32)).to_le_bytes()).chain(1u16.to_le_bytes()).chain(32u16.to_le_bytes()).chain([0; 24]));
                f.extend(px.iter().flat_map(|p| (p | 0xFF00_0000).to_le_bytes()));
                std::fs::write(Path::new(&dir).join(name), f).unwrap();
            }
        }
        assert!((mean(&ca) - mean(&cb)).abs() < 2.0 && shifted < 8.0);
    }
}
