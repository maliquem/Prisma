//! Limitador de FPS e HDR dentro do jogo. O limitador só espera antes de cada apresentação. O HDR
//! (`hdr.rs`) só entra quando o Prisma abre o jogo com ele pedido. Sem o Prisma por perto, ou com o
//! limite zerado, deixa tudo passar direto.

mod hdr;
mod hdr12;
mod hdrvk;
mod scan;

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};

pub(crate) type Ptr = *mut c_void;
type Present = unsafe extern "system" fn(Ptr, u32, u32) -> i32;
type Present1 = unsafe extern "system" fn(Ptr, u32, u32, *const c_void) -> i32;

const MAGIC: u32 = 0x5052_534d;
/// Erro anotado quando outro mod trocou o FP16 da cadeia e o proxy saiu do caminho (o mesmo em `inject::OTHER_FORMAT`).
pub(crate) const OTHER_FORMAT: u32 = 0xE0E0_0F16;
const STATE_LOADED: u32 = 1;
const STATE_READY: u32 = 2;
const STATE_FAILED: u32 = 3;
/// Sem sinal do Prisma por este tempo, o limite deixa de valer.
const BEAT_MS: u64 = 5000;
pub(crate) const PRESENT_TEST: u32 = 1;
const SLOT_PRESENT: usize = 8;
const SLOT_PRESENT1: usize = 22;
const R8G8B8A8_UNORM: u32 = 28;
const USAGE_TARGET: u32 = 0x20;
const SDK_VERSION: u32 = 7;
const DRIVER_HARDWARE: u32 = 1;
const DRIVER_WARP: u32 = 5;
const PAGE_READWRITE: u32 = 4;
const FILE_MAP_ALL: u32 = 0xF001F;
const PIN_FROM_ADDRESS: u32 = 1 | 4;
const TIMER_HIGH_RES: u32 = 2;
const TIMER_ALL: u32 = 0x1F0003;

/// Mesmo formato de `src/sys/inject.rs`.
#[repr(C)]
pub(crate) struct Shared {
    magic: AtomicU32,
    state: AtomicU32,
    /// Milésimos de FPS. Zero desliga.
    fps_milli: AtomicU32,
    pad: u32,
    beat: AtomicU64,
    presents: AtomicU64,
    waits: AtomicU64,
    /// O Prisma pede o proxy antes de o jogo rodar; só cadeias criadas com isto ligado viram HDR.
    pub hdr_proxy: AtomicU32,
    /// Curva HDR ligada. Desligada, o proxy mostra o SDR como o Windows mostraria.
    pub hdr_on: AtomicU32,
    /// Nits, em bits de f32. Gama, saturação e intensidade dos brilhos são números puros.
    pub hdr_peak: AtomicU32,
    pub hdr_paper: AtomicU32,
    pub hdr_gamma: AtomicU32,
    pub hdr_sat: AtomicU32,
    pub hdr_boost: AtomicU32,
    pub hdr_black: AtomicU32,
    /// Escritos pelo jogo: cadeias com proxy, último erro (HRESULT) e quadros desenhados.
    pub hdr_chains: AtomicU32,
    pub hdr_error: AtomicU32,
    pub hdr_frames: AtomicU64,
    /// Brilho da interface em nits (bits de f32). Zero desliga a separação entre cena e interface.
    pub hdr_ui: AtomicU32,
    /// Escritos pelo jogo: como saiu o último quadro (`hdr::SPLIT_*`), a saída do jogo (`hdr::MODE_*`)
    /// e quantos quadros saíram com a interface separada.
    pub hdr_split: AtomicU32,
    pub hdr_native: AtomicU32,
    /// Escrito pelo jogo: tempo de GPU do passe do HDR em µs (média), para o medidor do Prisma.
    pub hdr_gpu_us: AtomicU32,
    pub hdr_split_frames: AtomicU64,
    /// O que o jogo entrega, medido pelo Analisar (bits de f32, 0 = não medido): pico, preto e paper white.
    pub hdr_src_peak: AtomicU32,
    pub hdr_src_black: AtomicU32,
    pub hdr_src_paper: AtomicU32,
    /// Analisar: número da medida pedida pelo Prisma; zero não mede.
    pub hdr_scan: AtomicU32,
    /// Escritos pelo jogo durante o Analisar (`scan::Stats`): a medida a que pertencem, quadros aceitos,
    /// nits (bits de f32) e a saída do jogo (`hdr::MODE_*`).
    pub scan_gen: AtomicU32,
    pub scan_frames: AtomicU32,
    pub scan_peak: AtomicU32,
    pub scan_p999: AtomicU32,
    pub scan_p99: AtomicU32,
    pub scan_med: AtomicU32,
    pub scan_black: AtomicU32,
    pub scan_ui: AtomicU32,
    pub scan_white: AtomicU32,
    pub scan_mode: AtomicU32,
    /// Escrito pelo Prisma: o jogo manda HDR10 no buffer de 10 bits sem dizer o espaço de cor (`hdr::Want::untold`).
    pub hdr_pq: AtomicU32,
    /// Escrito pelo jogo: o último quadro veio desse buffer, que pode ser SDR ou HDR10.
    pub hdr_untold: AtomicU32,
}

const _: () = assert!(std::mem::size_of::<Shared>() == 176);

#[repr(C)]
#[derive(Clone, Copy)]
struct Guid(u32, u16, u16, [u8; 8]);

const IID_SWAPCHAIN1: Guid = Guid(0x790a45f7, 0x0d42, 0x4876, [0x98, 0x3a, 0x0a, 0x55, 0xcf, 0xe6, 0xf4, 0xaa]);

#[repr(C)]
struct SwapDesc {
    width: u32,
    height: u32,
    rate: [u32; 2],
    format: u32,
    scanline: u32,
    scaling: u32,
    sample: [u32; 2],
    usage: u32,
    count: u32,
    window: Ptr,
    windowed: i32,
    effect: u32,
    flags: u32,
}

type CreateDevice = unsafe extern "system" fn(Ptr, u32, Ptr, u32, *const u32, u32, u32, *const SwapDesc, *mut Ptr, *mut Ptr, *mut u32, *mut Ptr) -> i32;

#[link(name = "kernel32")]
extern "system" {
    fn GetCurrentProcessId() -> u32;
    fn CreateFileMappingW(file: isize, attrs: Ptr, protect: u32, high: u32, low: u32, name: *const u16) -> Ptr;
    fn MapViewOfFile(map: Ptr, access: u32, high: u32, low: u32, size: usize) -> Ptr;
    fn CreateThread(attrs: Ptr, stack: usize, start: unsafe extern "system" fn(Ptr) -> u32, arg: Ptr, flags: u32, id: *mut u32) -> Ptr;
    fn CloseHandle(h: Ptr) -> i32;
    fn CreateWaitableTimerExW(attrs: Ptr, name: *const u16, flags: u32, access: u32) -> Ptr;
    fn SetWaitableTimer(h: Ptr, due: *const i64, period: i32, routine: Ptr, arg: Ptr, resume: i32) -> i32;
    fn WaitForSingleObject(h: Ptr, ms: u32) -> u32;
    fn QueryPerformanceCounter(v: *mut i64) -> i32;
    fn QueryPerformanceFrequency(v: *mut i64) -> i32;
    fn GetTickCount64() -> u64;
    fn LoadLibraryW(name: *const u16) -> Ptr;
    fn GetProcAddress(module: Ptr, name: *const u8) -> Ptr;
    fn VirtualProtect(at: Ptr, size: usize, protect: u32, old: *mut u32) -> i32;
    fn GetModuleHandleExW(flags: u32, name: *const c_void, module: *mut Ptr) -> i32;
    fn GetModuleFileNameW(module: Ptr, name: *mut u16, size: u32) -> u32;
}

#[link(name = "user32")]
extern "system" {
    fn CallNextHookEx(hook: Ptr, code: i32, w: usize, l: isize) -> isize;
    fn CreateWindowExW(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: Ptr, menu: Ptr, inst: Ptr, param: Ptr) -> Ptr;
    fn DestroyWindow(h: Ptr) -> i32;
}

static STARTED: AtomicBool = AtomicBool::new(false);
pub(crate) static SHARED: AtomicUsize = AtomicUsize::new(0);
static FREQ: AtomicI64 = AtomicI64::new(0);
static NEXT: AtomicI64 = AtomicI64::new(0);
static TIMER: AtomicUsize = AtomicUsize::new(0);
static OLD_PRESENT: AtomicUsize = AtomicUsize::new(0);
static OLD_PRESENT1: AtomicUsize = AtomicUsize::new(0);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn now() -> i64 {
    let mut v = 0;
    unsafe { QueryPerformanceCounter(&mut v) };
    v
}

/// Dorme a maior parte e gira no último milissegundo: o relógio do Windows não acerta sozinho.
unsafe fn wait_until(deadline: i64, freq: i64) {
    let left = deadline - now();
    let sleep = (left * 10_000_000 / freq) - 10_000;
    let timer = TIMER.load(Relaxed) as Ptr;
    if sleep > 0 && !timer.is_null() {
        let due = -sleep;
        if SetWaitableTimer(timer, &due, 0, std::ptr::null_mut(), std::ptr::null_mut(), 0) != 0 {
            WaitForSingleObject(timer, 200);
        }
    }
    while now() < deadline {
        std::hint::spin_loop();
    }
}

/// Grade fixa de horários: um quadro atrasado não empurra os seguintes.
unsafe fn pace(flags: u32) {
    let at = SHARED.load(Relaxed);
    if flags & PRESENT_TEST != 0 || at == 0 {
        return;
    }
    let s = &*(at as *const Shared);
    s.presents.fetch_add(1, Relaxed);
    let milli = s.fps_milli.load(Relaxed);
    if milli == 0 || GetTickCount64() > s.beat.load(Relaxed) + BEAT_MS {
        NEXT.store(0, Relaxed);
        return;
    }
    let freq = FREQ.load(Relaxed);
    let period = freq * 1000 / milli as i64;
    let (time, last) = (now(), NEXT.load(Relaxed));
    let next = last + period;
    if last == 0 || next + period < time {
        NEXT.store(time, Relaxed);
        return;
    }
    if next > time {
        s.waits.fetch_add(1, Relaxed);
        wait_until(next, freq);
    }
    NEXT.store(next, Relaxed);
}

unsafe extern "system" fn on_present(this: Ptr, sync: u32, flags: u32) -> i32 {
    hdr::present(this, flags);
    pace(flags);
    let old: Present = std::mem::transmute(OLD_PRESENT.load(Relaxed));
    let hr = hdr::passthrough(|| old(this, sync, flags));
    hdr::rehook();
    hr
}

unsafe extern "system" fn on_present1(this: Ptr, sync: u32, flags: u32, params: *const c_void) -> i32 {
    hdr::present(this, flags);
    pace(flags);
    let old: Present1 = std::mem::transmute(OLD_PRESENT1.load(Relaxed));
    let hr = hdr::passthrough(|| old(this, sync, flags, params));
    hdr::rehook();
    hr
}

/// Troca uma entrada da tabela de funções. O original fica guardado antes de a nova valer.
pub(crate) unsafe fn patch(table: *mut usize, slot: usize, new: usize, keep: &AtomicUsize) -> bool {
    let at = table.add(slot);
    if *at == new {
        return true;
    }
    let mut old = 0;
    if VirtualProtect(at as Ptr, 8, PAGE_READWRITE, &mut old) == 0 {
        return false;
    }
    keep.store(*at, SeqCst);
    (*(at as *const AtomicUsize)).store(new, SeqCst);
    VirtualProtect(at as Ptr, 8, old, &mut old);
    true
}

pub(crate) unsafe fn release(obj: Ptr) {
    if !obj.is_null() {
        let table = *(obj as *const *const usize);
        let f: unsafe extern "system" fn(Ptr) -> u32 = std::mem::transmute(*table.add(2));
        f(obj);
    }
}

/// Liga `Present` e `Present1` na tabela da cadeia.
pub(crate) unsafe fn hook_present(chain: Ptr) -> bool {
    let table = *(chain as *const *mut usize);
    let mut done = patch(table, SLOT_PRESENT, on_present as usize, &OLD_PRESENT);
    let mut newer = std::ptr::null_mut();
    let ask: unsafe extern "system" fn(Ptr, *const Guid, *mut Ptr) -> i32 = std::mem::transmute(*table);
    if ask(chain, &IID_SWAPCHAIN1, &mut newer) >= 0 {
        done &= patch(*(newer as *const *mut usize), SLOT_PRESENT1, on_present1 as usize, &OLD_PRESENT1);
        release(newer);
    }
    done
}

/// Uma cadeia de troca descartável mostra onde a tabela do Windows está, a mesma que o jogo usa.
unsafe fn install() -> bool {
    let module = LoadLibraryW(wide("d3d11.dll").as_ptr());
    if module.is_null() {
        return false;
    }
    let create = GetProcAddress(module, b"D3D11CreateDeviceAndSwapChain\0".as_ptr());
    if create.is_null() {
        return false;
    }
    let create: CreateDevice = std::mem::transmute(create);
    let window = CreateWindowExW(0, wide("STATIC").as_ptr(), std::ptr::null(), 0, 0, 0, 2, 2, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
    if window.is_null() {
        return false;
    }
    let desc = SwapDesc { width: 2, height: 2, rate: [0, 0], format: R8G8B8A8_UNORM, scanline: 0, scaling: 0, sample: [1, 0], usage: USAGE_TARGET, count: 1, window, windowed: 1, effect: 0, flags: 0 };
    let (mut chain, mut device, mut context) = (std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut());
    let mut level = 0;
    let mut done = false;
    for driver in [DRIVER_HARDWARE, DRIVER_WARP] {
        if create(std::ptr::null_mut(), driver, std::ptr::null_mut(), 0, std::ptr::null(), 0, SDK_VERSION, &desc, &mut chain, &mut device, &mut level, &mut context) >= 0 {
            done = true;
            break;
        }
    }
    if done {
        done = hook_present(chain);
        if hdr::wanted() {
            done &= hdr::hook_chain(chain) & hdr::install();
        }
    }
    release(chain);
    release(context);
    release(device);
    DestroyWindow(window);
    done
}

/// O overlay da Steam entra no jogo depois que ele começa a rodar e liga os ganchos dele pondo um salto
/// no `CreateDXGIFactory1`. Espera até 10 s por isso; fora da Steam, só gasta a espera nesta linha.
unsafe fn steam_overlay_hooked() -> bool {
    let f = GetProcAddress(LoadLibraryW(wide("dxgi.dll").as_ptr()), b"CreateDXGIFactory1 ".as_ptr()) as *const u8;
    if f.is_null() {
        return false;
    }
    let name = wide("gameoverlayrenderer64.dll");
    for _ in 0..10_000 {
        let mut overlay = std::ptr::null_mut();
        let jump = *f == 0xE9 || (*f == 0xFF && *f.add(1) == 0x25);
        if jump && GetModuleHandleExW(0, name.as_ptr() as *const c_void, &mut overlay) != 0 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    false
}

/// Devolve a entrada original, se ela ainda for a nossa.
pub(crate) unsafe fn unpatch(table: *mut usize, slot: usize, ours: usize, keep: &AtomicUsize) {
    let at = table.add(slot);
    let mut old = 0;
    if *at == ours && VirtualProtect(at as Ptr, 8, PAGE_READWRITE, &mut old) != 0 {
        (*(at as *const AtomicUsize)).store(keep.load(SeqCst), SeqCst);
        VirtualProtect(at as Ptr, 8, old, &mut old);
    }
}

/// Cópia na pasta de DLLs de um carregador de mods (me3 do ERR), com o nome de `inject::NATIVE_DLL`: ninguém do
/// Prisma abriu o jogo, e estar ali já é o pedido do HDR. Os ajustes chegam depois, ao vivo.
unsafe fn native(module: Ptr) -> bool {
    let mut buf = [0u16; 1024];
    let n = GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())]).to_ascii_lowercase().ends_with("\\prisma_hdr.dll")
}

unsafe extern "system" fn start(_: Ptr) -> u32 {
    let mut module = std::ptr::null_mut();
    // O Windows solta o módulo quando o gancho sai: preso aqui, ele fica até o jogo fechar.
    GetModuleHandleExW(PIN_FROM_ADDRESS, start as *const c_void, &mut module);
    let name = wide(&format!("Local\\PrismaLimiter-{}", GetCurrentProcessId()));
    let map = CreateFileMappingW(-1, std::ptr::null_mut(), PAGE_READWRITE, 0, std::mem::size_of::<Shared>() as u32, name.as_ptr());
    if map.is_null() {
        return 1;
    }
    let view = MapViewOfFile(map, FILE_MAP_ALL, 0, 0, 0);
    if view.is_null() {
        return 1;
    }
    let s = &*(view as *const Shared);
    let mut freq = 0;
    QueryPerformanceFrequency(&mut freq);
    FREQ.store(freq, Relaxed);
    TIMER.store(CreateWaitableTimerExW(std::ptr::null_mut(), std::ptr::null(), TIMER_HIGH_RES, TIMER_ALL) as usize, Relaxed);
    if native(module) {
        s.hdr_proxy.store(1, SeqCst);
    }
    s.state.store(STATE_LOADED, SeqCst);
    s.magic.store(MAGIC, SeqCst);
    SHARED.store(view as usize, SeqCst);
    // A camada do Vulkan entra antes de o jogo seguir; o carregador lê as variáveis na primeira instância.
    if hdr::wanted() {
        hdrvk::setup(module);
    }
    s.state.store(if install() { STATE_READY } else { STATE_FAILED }, SeqCst);
    if hdr::wanted() && steam_overlay_hooked() {
        hdr::let_steam_in();
    }
    0
}

/// Aberto pelo Prisma (`LoadLibraryW` com o jogo ainda parado): começa sozinho.
#[no_mangle]
pub unsafe extern "system" fn DllMain(_: Ptr, reason: u32, _: Ptr) -> i32 {
    const PROCESS_ATTACH: u32 = 1;
    if reason == PROCESS_ATTACH {
        begin();
    }
    1
}

/// Uma vez só, em outra linha: o trabalho não pode rodar com a trava do carregador pega.
unsafe fn begin() {
    if !STARTED.swap(true, SeqCst) {
        let t = CreateThread(std::ptr::null_mut(), 0, start, std::ptr::null_mut(), 0, std::ptr::null_mut());
        if !t.is_null() {
            CloseHandle(t);
        }
    }
}

/// O Windows chama isto na linha da janela do jogo. O trabalho vai para outra linha, para não segurar o jogo.
#[no_mangle]
pub unsafe extern "system" fn prisma_hook(code: i32, w: usize, l: isize) -> isize {
    begin();
    CallNextHookEx(std::ptr::null_mut(), code, w, l)
}
