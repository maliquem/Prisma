//! HDR no DirectX 9, só em janela. O D3D9 comum não divide buffer com outra API, então o dispositivo do
//! jogo nasce pelo 9On12 do Windows, sobre um D3D12 nosso. No `Present` do jogo, o buffer de trás dele é
//! copiado para o buffer do jogo numa cadeia DXGI nossa, que o proxy do D3D12 (`hdr12.rs`) já transforma:
//! a curva, a calibração, o Analisar e o medidor são os mesmos. O `Present` do 9On12 nunca roda.
//! shortcut: D3D9Ex, MSAA no buffer de trás e tela cheia exclusiva passam direto, sem HDR.

use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicUsize, Ordering::*};
use std::sync::{Mutex, MutexGuard};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Direct3D9::*;
use windows::Win32::Graphics::Direct3D9on12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

use crate::hdr::{self, INSIDE};
use crate::{export, patch, Ptr};

const SLOT_CREATE_DEVICE: usize = 16;
const SLOT_RESET: usize = 16;
const SLOT_PRESENT: usize = 17;
const SLOT_SWAP_PRESENT: usize = 3;
/// Listas em giro: a cópia de um quadro não espera a do anterior.
const RING: usize = 3;

type CreateDevice = unsafe extern "system" fn(Ptr, u32, D3DDEVTYPE, HWND, u32, *mut D3DPRESENT_PARAMETERS, *mut Ptr) -> HRESULT;
type Reset = unsafe extern "system" fn(Ptr, *mut D3DPRESENT_PARAMETERS) -> HRESULT;
type Present = unsafe extern "system" fn(Ptr, *const RECT, *const RECT, HWND, Ptr) -> HRESULT;
type SwapPresent = unsafe extern "system" fn(Ptr, *const RECT, *const RECT, HWND, Ptr, u32) -> HRESULT;
type Create9 = unsafe extern "system" fn(u32) -> Ptr;
type Create9On12 = unsafe extern "system" fn(u32, *mut D3D9ON12_ARGS, u32) -> Ptr;
type CreateD3D12 = unsafe extern "system" fn(Ptr, D3D_FEATURE_LEVEL, *const GUID, *mut Ptr) -> HRESULT;

static OLD_CREATE_DEVICE: AtomicUsize = AtomicUsize::new(0);
static OLD_RESET: AtomicUsize = AtomicUsize::new(0);
static OLD_PRESENT: AtomicUsize = AtomicUsize::new(0);
static OLD_SWAP_PRESENT: AtomicUsize = AtomicUsize::new(0);
static DEVICES: Mutex<Vec<Device>> = Mutex::new(Vec::new());

struct Slot {
    alloc: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    done: u64,
}

/// Um dispositivo do jogo feito pelo 9On12. Os endereços do jogo ficam sem referência: segurar o
/// dispositivo impediria o jogo de soltá-lo.
struct Device {
    device: usize,
    swap: usize,
    hwnd: HWND,
    sync: u32,
    /// Sem HDR agora (tela cheia, ou a cadeia nossa falhou): o `Present` do 9On12 segue normal.
    off: bool,
    queue: ID3D12CommandQueue,
    fence: ID3D12Fence,
    value: u64,
    ring: Vec<Slot>,
    chain: Option<IDXGISwapChain3>,
}

// Os objetos COM só são usados com a trava pega.
unsafe impl Send for Device {}

fn devices() -> MutexGuard<'static, Vec<Device>> {
    DEVICES.lock().unwrap_or_else(|e| e.into_inner())
}

/// O intervalo do D3D9 vira o do DXGI: o padrão espera um retraço, o imediato nenhum.
fn interval(v: u32) -> u32 {
    match v {
        0 => 1,
        v if v == D3DPRESENT_INTERVAL_IMMEDIATE as u32 => 0,
        v => v.min(4),
    }
}

fn transition(res: &ID3D12Resource, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            // Sem AddRef: a barreira só vive durante a chamada.
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { std::mem::transmute_copy(res) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    }
}

/// O pedaço do buffer que o `Present` mostra, dentro do buffer; sem pedaço (ou vazio), o buffer todo.
fn area(src: Option<&RECT>, width: u32, height: u32) -> D3D12_BOX {
    let full = D3D12_BOX { left: 0, top: 0, front: 0, right: width, bottom: height, back: 1 };
    let Some(r) = src else { return full };
    let x = |v: i32| v.clamp(0, width as i32) as u32;
    let y = |v: i32| v.clamp(0, height as i32) as u32;
    let b = D3D12_BOX { left: x(r.left), top: y(r.top), front: 0, right: x(r.right), bottom: y(r.bottom), back: 1 };
    if b.right <= b.left || b.bottom <= b.top { full } else { b }
}

/// Sem AddRef, como a barreira: o local só vive durante a chamada.
fn location(res: &ID3D12Resource) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        pResource: unsafe { std::mem::transmute_copy(res) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
    }
}

impl Device {
    unsafe fn idle(&self) {
        if self.fence.GetCompletedValue() < self.value {
            let _ = self.fence.SetEventOnCompletion(self.value, HANDLE::default());
        }
    }

    /// Solta a cadeia nossa e o proxy dela; a próxima apresentação em janela faz outra.
    unsafe fn close(&mut self) {
        self.idle();
        if let Some(c) = self.chain.take() {
            hdr::forget(c.as_raw());
        }
    }

    /// A cadeia nasce pela fábrica, fora do `INSIDE`: o gancho dela a faz FP16 com o proxy do D3D12.
    unsafe fn open(&mut self, width: u32, height: u32, format: DXGI_FORMAT) -> Result<IDXGISwapChain3> {
        let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width,
            Height: height,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ..Default::default()
        };
        let chain = factory.CreateSwapChainForHwnd(&self.queue, self.hwnd, &desc, None, None)?;
        // Sem proxy a cadeia saiu comum: o HDR não pegou nesta tela.
        if !hdr::ours(chain.as_raw()) {
            return Err(E_FAIL.into());
        }
        // O Alt+Enter é do jogo (ele chama o `Reset`), não do DXGI.
        let _ = factory.MakeWindowAssociation(self.hwnd, DXGI_MWA_NO_ALT_ENTER);
        let chain: IDXGISwapChain3 = chain.cast()?;
        self.chain = Some(chain.clone());
        Ok(chain)
    }

    /// Grava e manda a cópia do buffer de trás (ou do pedaço que o `Present` mostra) para o buffer do jogo
    /// da vez na cadeia nossa.
    unsafe fn copy(&mut self, res: &ID3D12Resource, src: Option<&RECT>) -> Result<()> {
        let d = res.GetDesc();
        let b = area(src, d.Width as u32, d.Height);
        let (width, height) = (b.right - b.left, b.bottom - b.top);
        let chain = match self.chain.clone() {
            Some(c) => c,
            None => self.open(width, height, d.Format)?,
        };
        // O `GetDesc1` do gancho mostra o tamanho do buffer do jogo; o `ResizeBuffers` passa pelo proxy.
        let cd = chain.GetDesc1()?;
        if (cd.Width, cd.Height) != (width, height) {
            self.idle();
            chain.ResizeBuffers(0, width, height, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
        }
        let game: ID3D12Resource = chain.GetBuffer(chain.GetCurrentBackBufferIndex())?;
        let s = &mut self.ring[(self.value + 1) as usize % RING];
        if self.fence.GetCompletedValue() < s.done {
            let _ = self.fence.SetEventOnCompletion(s.done, HANDLE::default());
        }
        s.alloc.Reset()?;
        s.list.Reset(&s.alloc, None)?;
        // O buffer do 9On12 chega em COMMON e sobe sozinho para fonte de cópia; volta a COMMON no fim da lista.
        s.list.ResourceBarrier(&[transition(&game, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_COPY_DEST)]);
        s.list.CopyTextureRegion(&location(&game), 0, 0, 0, &location(res), Some(&b));
        s.list.ResourceBarrier(&[transition(&game, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PRESENT)]);
        s.list.Close()?;
        self.queue.ExecuteCommandLists(&[Some(s.list.cast()?)]);
        s.done = self.value + 1;
        Ok(())
    }

    /// O quadro do jogo vai para a cadeia nossa. O buffer do 9On12 volta a ele mesmo com erro.
    unsafe fn frame(&mut self, src: Option<&RECT>) -> Result<()> {
        let raw = self.device as Ptr;
        let device = IDirect3DDevice9::from_raw_borrowed(&raw).ok_or(Error::from(E_POINTER))?;
        let on: IDirect3DDevice9On12 = device.cast()?;
        let bb = device.GetBackBuffer(0, 0, D3DBACKBUFFER_TYPE_MONO)?;
        let mut res: Option<ID3D12Resource> = None;
        on.UnwrapUnderlyingResource(&bb, &self.queue, &ID3D12Resource::IID, &mut res as *mut _ as *mut *mut c_void)?;
        let made = match &res {
            Some(r) => self.copy(r, src),
            None => Err(E_FAIL.into()),
        };
        drop(res);
        self.value += 1;
        let signal = self.queue.Signal(&self.fence, self.value);
        let (mut v, mut f) = (self.value, [Some(self.fence.clone())]);
        on.ReturnUnderlyingResource(&bb, 1, &mut v, f.as_mut_ptr())?;
        signal.and(made)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        unsafe { self.close() };
    }
}

/// Apresenta pelo caminho nosso; `None`: o jogo segue pelo `Present` dele.
unsafe fn show(this: Ptr, key: fn(&Device) -> usize, src: *const RECT) -> Option<HRESULT> {
    if INSIDE.get() {
        return None;
    }
    let mut list = devices();
    let d = list.iter_mut().find(|d| key(d) == this as usize && !d.off)?;
    if let Err(e) = d.frame(src.as_ref()) {
        hdr::note(e.code());
        d.off = true;
        d.close();
        return None;
    }
    let (chain, sync) = (d.chain.clone()?, d.sync);
    drop(list);
    // Passa pelo gancho do `Present` da cadeia: o passe HDR, o limitador e o medidor.
    let hr = chain.Present(sync, DXGI_PRESENT(0));
    if hr.is_err() {
        hdr::note(hr);
    }
    Some(S_OK)
}

unsafe extern "system" fn on_present(this: Ptr, src: *const RECT, dst: *const RECT, hwnd: HWND, dirty: Ptr) -> HRESULT {
    if let Some(hr) = show(this, |d| d.device, src) {
        return hr;
    }
    let old: Present = std::mem::transmute(OLD_PRESENT.load(Relaxed));
    old(this, src, dst, hwnd, dirty)
}

unsafe extern "system" fn on_swap_present(this: Ptr, src: *const RECT, dst: *const RECT, hwnd: HWND, dirty: Ptr, flags: u32) -> HRESULT {
    if let Some(hr) = show(this, |d| d.swap, src) {
        return hr;
    }
    let old: SwapPresent = std::mem::transmute(OLD_SWAP_PRESENT.load(Relaxed));
    old(this, src, dst, hwnd, dirty, flags)
}

/// O `Reset` troca janela, tamanho e intervalo: a cadeia nossa sai antes e volta no próximo quadro em janela.
unsafe extern "system" fn on_reset(this: Ptr, pp: *mut D3DPRESENT_PARAMETERS) -> HRESULT {
    if let Some(d) = devices().iter_mut().find(|d| d.device == this as usize) {
        d.close();
        if let Some(p) = pp.as_ref() {
            d.sync = interval(p.PresentationInterval);
            d.off = !p.Windowed.as_bool() || p.MultiSampleType != D3DMULTISAMPLE_NONE;
        }
    }
    let old: Reset = std::mem::transmute(OLD_RESET.load(Relaxed));
    old(this, pp)
}

/// O dispositivo do jogo, pelo 9On12 sobre um D3D12 e uma fila nossos.
unsafe fn make(adapter: u32, kind: D3DDEVTYPE, focus: HWND, flags: u32, pp: *mut D3DPRESENT_PARAMETERS, hwnd: HWND) -> Result<(IDirect3DDevice9, Device)> {
    let (create12, create9) = (export("d3d12.dll", b"D3D12CreateDevice\0"), export("d3d9.dll", b"Direct3DCreate9On12\0"));
    if create12.is_null() || create9.is_null() {
        return Err(E_NOINTERFACE.into());
    }
    let (create12, create9): (CreateD3D12, Create9On12) = (std::mem::transmute(create12), std::mem::transmute(create9));
    let mut raw = std::ptr::null_mut();
    create12(std::ptr::null_mut(), D3D_FEATURE_LEVEL_11_0, &ID3D12Device::IID, &mut raw).ok()?;
    let d12 = ID3D12Device::from_raw(raw);
    let queue: ID3D12CommandQueue = d12.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() })?;
    let mut args = D3D9ON12_ARGS {
        Enable9On12: TRUE,
        pD3D12Device: ManuallyDrop::new(Some(d12.cast()?)),
        ppD3D12Queues: [ManuallyDrop::new(Some(queue.cast()?)), ManuallyDrop::new(None)],
        NumQueues: 1,
        NodeMask: 0,
    };
    let d9 = create9(D3D_SDK_VERSION, &mut args, 1);
    ManuallyDrop::drop(&mut args.pD3D12Device);
    ManuallyDrop::drop(&mut args.ppD3D12Queues[0]);
    if d9.is_null() {
        return Err(E_FAIL.into());
    }
    let d9 = IDirect3D9::from_raw(d9);
    let mut device = None;
    d9.CreateDevice(adapter, kind, focus, flags, pp, &mut device)?;
    let device = device.ok_or(Error::from(E_FAIL))?;
    let swap = device.GetSwapChain(0)?;
    let fence: ID3D12Fence = d12.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
    let mut ring = Vec::with_capacity(RING);
    for _ in 0..RING {
        let alloc: ID3D12CommandAllocator = d12.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
        let list: ID3D12GraphicsCommandList = d12.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None)?;
        list.Close()?;
        ring.push(Slot { alloc, list, done: 0 });
    }
    patch(*(device.as_raw() as *const *mut usize), SLOT_RESET, on_reset as *const () as usize, &OLD_RESET);
    patch(*(device.as_raw() as *const *mut usize), SLOT_PRESENT, on_present as *const () as usize, &OLD_PRESENT);
    patch(*(swap.as_raw() as *const *mut usize), SLOT_SWAP_PRESENT, on_swap_present as *const () as usize, &OLD_SWAP_PRESENT);
    let sync = interval((*pp).PresentationInterval);
    let d = Device { device: device.as_raw() as usize, swap: swap.as_raw() as usize, hwnd, sync, off: false, queue, fence, value: 0, ring, chain: None };
    Ok((device, d))
}

unsafe extern "system" fn on_create_device(this: Ptr, adapter: u32, kind: D3DDEVTYPE, focus: HWND, flags: u32, pp: *mut D3DPRESENT_PARAMETERS, out: *mut Ptr) -> HRESULT {
    let old: CreateDevice = std::mem::transmute(OLD_CREATE_DEVICE.load(Relaxed));
    let Some(p) = pp.as_ref().filter(|_| !INSIDE.get() && !out.is_null() && hdr::wanted()) else {
        return old(this, adapter, kind, focus, flags, pp, out);
    };
    let hwnd = if p.hDeviceWindow.is_invalid() { focus } else { p.hDeviceWindow };
    // O D3D9Ex do jogo pede o dispositivo Ex depois: o do 9On12 comum quebraria ele.
    let ex = IUnknown::from_raw_borrowed(&this).is_some_and(|u| u.cast::<IDirect3D9Ex>().is_ok());
    let fit = p.Windowed.as_bool() && p.MultiSampleType == D3DMULTISAMPLE_NONE && kind == D3DDEVTYPE_HAL && !ex && hdr::screen_hdr(hwnd);
    if !fit {
        return old(this, adapter, kind, focus, flags, pp, out);
    }
    match hdr::passthrough(|| make(adapter, kind, focus, flags, pp, hwnd)) {
        Ok((device, d)) => {
            let mut list = devices();
            // Dispositivo velho no mesmo endereço ou na mesma janela: a cadeia dele prenderia a janela.
            list.retain(|o| o.device != d.device && o.hwnd != hwnd);
            list.push(d);
            *out = device.into_raw();
            S_OK
        }
        Err(e) => {
            hdr::note(e.code());
            old(this, adapter, kind, focus, flags, pp, out)
        }
    }
}

/// Um `IDirect3D9` descartável mostra a tabela que o jogo vai usar para criar o dispositivo.
pub unsafe fn install() -> bool {
    let create = export("d3d9.dll", b"Direct3DCreate9\0");
    if create.is_null() {
        return false;
    }
    let create: Create9 = std::mem::transmute(create);
    let d9 = create(D3D_SDK_VERSION);
    if d9.is_null() {
        return false;
    }
    let d9 = IDirect3D9::from_raw(d9);
    patch(*(d9.as_raw() as *const *mut usize), SLOT_CREATE_DEVICE, on_create_device as *const () as usize, &OLD_CREATE_DEVICE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_follows_d3d9() {
        assert_eq!(interval(0), 1);
        assert_eq!(interval(1), 1);
        assert_eq!(interval(2), 2);
        assert_eq!(interval(8), 4);
        assert_eq!(interval(D3DPRESENT_INTERVAL_IMMEDIATE as u32), 0);
    }

    #[test]
    fn area_stays_inside_the_buffer() {
        let r = |left, top, right, bottom| RECT { left, top, right, bottom };
        let k = |b: D3D12_BOX| (b.left, b.top, b.right, b.bottom);
        assert_eq!(k(area(None, 640, 480)), (0, 0, 640, 480));
        assert_eq!(k(area(Some(&r(0, 0, 320, 240)), 640, 480)), (0, 0, 320, 240));
        assert_eq!(k(area(Some(&r(-5, 10, 900, 500)), 640, 480)), (0, 10, 640, 480));
        assert_eq!(k(area(Some(&r(100, 100, 100, 200)), 640, 480)), (0, 0, 640, 480));
    }
}
