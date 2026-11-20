//! HDR no D3D12: o mesmo proxy do D3D11 (`hdr.rs`). O jogo desenha num buffer nosso por buffer da cadeia,
//! e no `Present` uma lista nossa, mandada na fila do jogo, passa o buffer da vez pela curva até a cadeia
//! FP16 de verdade. O índice da vez é o da cadeia de verdade, que o jogo pergunta a ela mesma.

use std::cell::Cell;
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::{Mutex, MutexGuard};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;

use crate::hdr::{self, Params, Want, INSIDE};
use crate::{patch, scan, Ptr, Shared};

const SLOT_CREATE_SRV: usize = 18;
const SLOT_CREATE_UAV: usize = 19;
const SLOT_CREATE_RTV: usize = 20;
const SLOT_DRAW_INSTANCED: usize = 12;
const SLOT_DRAW_INDEXED_INSTANCED: usize = 13;
const SLOT_RS_SET_VIEWPORTS: usize = 21;
const SLOT_OM_SET: usize = 46;
const USAGE_UAV: u32 = 0x400;
/// O DXGI aceita até 16 buffers; a separação da interface acompanha os 8 primeiros.
const MAX: usize = 8;
const BUFFERS: usize = 16;
/// O jogo e a cena são lidos pelo passe e pelo `cs` do Analisar.
const READ: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE;

type CreateView = unsafe extern "system" fn(Ptr, Ptr, *const c_void, D3D12_CPU_DESCRIPTOR_HANDLE);
type CreateUav = unsafe extern "system" fn(Ptr, Ptr, Ptr, *const c_void, D3D12_CPU_DESCRIPTOR_HANDLE);
type OmSet = unsafe extern "system" fn(Ptr, u32, *const D3D12_CPU_DESCRIPTOR_HANDLE, BOOL, *const D3D12_CPU_DESCRIPTOR_HANDLE);
type SetViewports = unsafe extern "system" fn(Ptr, u32, *const D3D12_VIEWPORT);
type DrawInstanced = unsafe extern "system" fn(Ptr, u32, u32, u32, u32);
type DrawIndexedInstanced = unsafe extern "system" fn(Ptr, u32, u32, u32, i32, u32);

static OLD_CREATE_SRV: AtomicUsize = AtomicUsize::new(0);
static OLD_CREATE_UAV: AtomicUsize = AtomicUsize::new(0);
static OLD_CREATE_RTV: AtomicUsize = AtomicUsize::new(0);
static OLD_OM_SET: AtomicUsize = AtomicUsize::new(0);
static OLD_SET_VIEWPORTS: AtomicUsize = AtomicUsize::new(0);
static OLD_DRAW_INSTANCED: AtomicUsize = AtomicUsize::new(0);
static OLD_DRAW_INDEXED_INSTANCED: AtomicUsize = AtomicUsize::new(0);
static PROXIES: Mutex<Vec<Proxy>> = Mutex::new(Vec::new());

/// Separação da interface, só no proxy mais novo (como no D3D11): os buffers do jogo, as vistas de alvo
/// que o jogo fez neles (duas por buffer: comum e _SRGB), a cena guardada, o tamanho e a fase por buffer.
static GAME: [AtomicUsize; MAX] = [const { AtomicUsize::new(0) }; MAX];
static HANDLES: [AtomicUsize; MAX * 2] = [const { AtomicUsize::new(0) }; MAX * 2];
static SCENE: AtomicUsize = AtomicUsize::new(0);
static SIZE: AtomicU64 = AtomicU64::new(0);
static PHASES: [AtomicU32; MAX] = [const { AtomicU32::new(hdr::PHASE_NONE) }; MAX];

thread_local! {
    /// Lista sendo gravada nesta linha e o buffer do jogo preso nela (índice + 1; zero: outro alvo).
    static BB: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
    /// Lista e se a área de desenho dela cobre a tela toda.
    static VP: Cell<(usize, bool)> = const { Cell::new((0, false)) };
}

struct Frame {
    /// O buffer que o jogo recebe no `GetBuffer` e o da cadeia de verdade, no mesmo índice.
    game: ID3D12Resource,
    real: ID3D12Resource,
    alloc: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    /// Valor da cerca que marca o fim do último passe deste índice.
    done: u64,
}

/// Carimbos de tempo do passe, dois por índice, lidos quando a cerca do índice volta.
struct Timer {
    heap: ID3D12QueryHeap,
    read: ID3D12Resource,
    freq: f64,
    ms: f64,
}

pub(crate) struct Proxy {
    pub chain: usize,
    pub hwnd: usize,
    pub want: Want,
    queue: ID3D12CommandQueue,
    device: ID3D12Device,
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    fence: ID3D12Fence,
    value: u64,
    frames: Vec<Frame>,
    /// Duas vistas por índice (buffer do jogo e cena) e uma de alvo por buffer de verdade.
    srv: Option<ID3D12DescriptorHeap>,
    rtv: Option<ID3D12DescriptorHeap>,
    scene: Option<ID3D12Resource>,
    timer: Option<Timer>,
    /// Sem ele o Analisar não mede neste jogo.
    scan: Option<Scan>,
}

/// O histograma do Analisar: o `cs`, o buffer que ele soma e uma vaga de leitura por índice da cadeia,
/// lida quando a cerca do índice volta.
struct Scan {
    pso: ID3D12PipelineState,
    buf: ID3D12Resource,
    read: ID3D12Resource,
    ring: scan::Ring,
}

unsafe fn buffer(device: &ID3D12Device, kind: D3D12_HEAP_TYPE, size: u64, flags: D3D12_RESOURCE_FLAGS, state: D3D12_RESOURCE_STATES) -> Result<ID3D12Resource> {
    let props = D3D12_HEAP_PROPERTIES { Type: kind, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: flags,
        ..Default::default()
    };
    let mut out = None;
    device.CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut out)?;
    out.ok_or_else(|| E_FAIL.into())
}

impl Scan {
    unsafe fn new(device: &ID3D12Device, root: &ID3D12RootSignature) -> Result<Self> {
        let cs = hdr::compile(s!("cs_5_0"), s!("cs"))?;
        let pd = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(root),
            CS: D3D12_SHADER_BYTECODE { pShaderBytecode: cs.GetBufferPointer(), BytecodeLength: cs.GetBufferSize() },
            ..Default::default()
        };
        let pso = device.CreateComputePipelineState(&pd)?;
        let buf = buffer(device, D3D12_HEAP_TYPE_DEFAULT, scan::BYTES, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)?;
        let read = buffer(device, D3D12_HEAP_TYPE_READBACK, scan::BYTES * BUFFERS as u64, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST)?;
        Ok(Self { pso, buf, read, ring: scan::Ring::new(BUFFERS) })
    }

    /// A soma que o índice `i` copiou da última vez, se é desta medida.
    unsafe fn take(&mut self, s: &Shared, i: usize, mode: u32) {
        if !self.ring.ready(i) {
            return;
        }
        let at = i * scan::BYTES as usize;
        let range = D3D12_RANGE { Begin: at, End: at + scan::BYTES as usize };
        let mut ptr = std::ptr::null_mut();
        if self.read.Map(0, Some(&range), Some(&mut ptr)).is_ok() {
            let cur = std::slice::from_raw_parts((ptr as *const u8).add(at) as *const u32, scan::WORDS).to_vec();
            self.read.Unmap(0, Some(&D3D12_RANGE::default()));
            self.ring.feed(s, mode, &cur);
        }
    }
}

// Os objetos COM só são usados com a trava pega.
unsafe impl Send for Proxy {}

pub(crate) fn proxies() -> MutexGuard<'static, Vec<Proxy>> {
    PROXIES.lock().unwrap_or_else(|e| e.into_inner())
}

/// A fila do D3D12 que o jogo deu ao criar a cadeia (no D3D11 é o dispositivo).
pub(crate) unsafe fn queue(device: Ptr) -> Option<ID3D12CommandQueue> {
    IUnknown::from_raw_borrowed(&device)?.cast().ok()
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

fn srv_desc(format: DXGI_FORMAT) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: format,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MipLevels: 1, ..Default::default() } },
    }
}

fn at(base: D3D12_CPU_DESCRIPTOR_HANDLE, i: usize, step: u32) -> D3D12_CPU_DESCRIPTOR_HANDLE {
    D3D12_CPU_DESCRIPTOR_HANDLE { ptr: base.ptr + i * step as usize }
}

unsafe fn texture(device: &ID3D12Device, w: Want, flags: D3D12_RESOURCE_FLAGS, state: D3D12_RESOURCE_STATES) -> Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: w.width as u64,
        Height: w.height,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: hdr::typeless(w.format),
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Flags: flags,
        ..Default::default()
    };
    let mut out = None;
    device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut out)?;
    out.ok_or_else(|| E_FAIL.into())
}

impl Proxy {
    pub unsafe fn new(chain: Ptr, hwnd: HWND, queue: ID3D12CommandQueue, want: Want) -> Result<Self> {
        let mut device: Option<ID3D12Device> = None;
        queue.GetDevice(&mut device)?;
        let device = device.ok_or(Error::from(E_FAIL))?;
        // A cadeia só aceita fila direta: o passe desenha nela.
        let (vs, ps, rs) = (hdr::compile(s!("vs_5_0"), s!("vs"))?, hdr::compile(s!("ps_5_0"), s!("ps"))?, hdr::compile(s!("rootsig_1_0"), s!("RS"))?);
        let root: ID3D12RootSignature = device.CreateRootSignature(0, hdr::bytes(&rs))?;
        let mut blend = D3D12_BLEND_DESC::default();
        blend.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
            SrcBlend: D3D12_BLEND_ONE,
            DestBlend: D3D12_BLEND_ZERO,
            BlendOp: D3D12_BLEND_OP_ADD,
            SrcBlendAlpha: D3D12_BLEND_ONE,
            DestBlendAlpha: D3D12_BLEND_ZERO,
            BlendOpAlpha: D3D12_BLEND_OP_ADD,
            LogicOp: D3D12_LOGIC_OP_NOOP,
            RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
            ..Default::default()
        };
        let face = D3D12_DEPTH_STENCILOP_DESC { StencilFailOp: D3D12_STENCIL_OP_KEEP, StencilDepthFailOp: D3D12_STENCIL_OP_KEEP, StencilPassOp: D3D12_STENCIL_OP_KEEP, StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS };
        let code = |b: &ID3DBlob| D3D12_SHADER_BYTECODE { pShaderBytecode: b.GetBufferPointer(), BytecodeLength: b.GetBufferSize() };
        let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
        formats[0] = DXGI_FORMAT_R16G16B16A16_FLOAT;
        let pd = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            VS: code(&vs),
            PS: code(&ps),
            BlendState: blend,
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC { FillMode: D3D12_FILL_MODE_SOLID, CullMode: D3D12_CULL_MODE_NONE, DepthClipEnable: true.into(), ..Default::default() },
            DepthStencilState: D3D12_DEPTH_STENCIL_DESC { DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ZERO, DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS, FrontFace: face, BackFace: face, ..Default::default() },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            RTVFormats: formats,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..Default::default()
        };
        let pso = device.CreateGraphicsPipelineState(&pd)?;
        let fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
        let scan = Scan::new(&device, &root).ok();
        let mut me = Self { chain: chain as usize, hwnd: hwnd.0 as usize, want, queue, device, root, pso, fence, value: 0, frames: Vec::new(), srv: None, rtv: None, scene: None, timer: None, scan };
        me.rebuild(chain)?;
        me.timer = me.timer().ok();
        hook_device(me.device.as_raw());
        hook_list(me.frames[0].list.as_raw());
        Ok(me)
    }

    unsafe fn timer(&self) -> Result<Timer> {
        let n = 2 * MAX as u32 * 2;
        let mut heap = None;
        self.device.CreateQueryHeap(&D3D12_QUERY_HEAP_DESC { Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: n, NodeMask: 0 }, &mut heap)?;
        let props = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: n as u64 * 8,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut read = None;
        self.device.CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut read)?;
        let freq = self.queue.GetTimestampFrequency()? as f64;
        Ok(Timer { heap: heap.ok_or(Error::from(E_FAIL))?, read: read.ok_or(Error::from(E_FAIL))?, freq, ms: 0.0 })
    }

    /// Espera a GPU passar do valor `v` da nossa cerca. Dispositivo perdido devolve tudo pronto.
    unsafe fn wait(&self, v: u64) {
        if self.fence.GetCompletedValue() < v {
            let _ = self.fence.SetEventOnCompletion(v, HANDLE::default());
        }
    }

    /// Solta tudo que segura buffers, depois de a GPU terminar o nosso passe: o `ResizeBuffers` falha com algum preso.
    pub fn drop_buffers(&mut self) {
        unsafe { self.wait(self.value) };
        if let Some(f) = self.frames.first() {
            if GAME[0].load(Relaxed) == f.game.as_raw() as usize {
                GAME.iter().chain(HANDLES.iter()).for_each(|g| g.store(0, Relaxed));
                SCENE.store(0, Relaxed);
            }
        }
        (self.frames, self.srv, self.rtv, self.scene) = (Vec::new(), None, None, None);
    }

    /// Refaz os buffers do jogo no tamanho e na quantidade atuais da cadeia de verdade. Só com `INSIDE` ligado.
    pub unsafe fn rebuild(&mut self, chain: Ptr) -> Result<()> {
        self.drop_buffers();
        let real = IDXGISwapChain::from_raw_borrowed(&chain).ok_or(Error::from(E_POINTER))?.cast::<IDXGISwapChain3>()?;
        let d = real.GetDesc1()?;
        let n = (d.BufferCount as usize).max(1);
        (self.want.width, self.want.height) = (d.Width, d.Height);
        let w = self.want;
        let heap = |kind, count, flags| self.device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&D3D12_DESCRIPTOR_HEAP_DESC { Type: kind, NumDescriptors: count as u32, Flags: flags, NodeMask: 0 });
        let srv = heap(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 2 * n, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE)?;
        let rtv = heap(D3D12_DESCRIPTOR_HEAP_TYPE_RTV, n, D3D12_DESCRIPTOR_HEAP_FLAG_NONE)?;
        let (s0, r0) = (srv.GetCPUDescriptorHandleForHeapStart(), rtv.GetCPUDescriptorHandleForHeapStart());
        let (ss, rs) = (self.device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV), self.device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV));
        // A cena descansa como destino de cópia: o passe e a cópia da separação trocam o estado e voltam.
        let scene = texture(&self.device, w, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST)?;
        let mut flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
        if w.usage & USAGE_UAV != 0 {
            flags |= D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
        }
        let rd = D3D12_RENDER_TARGET_VIEW_DESC { Format: DXGI_FORMAT_R16G16B16A16_FLOAT, ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D, ..Default::default() };
        let mut frames = Vec::with_capacity(n);
        for i in 0..n {
            let real_i: ID3D12Resource = real.GetBuffer(i as u32)?;
            // O jogo recebe o buffer no estado de apresentação, como o da cadeia.
            let game = texture(&self.device, w, flags, D3D12_RESOURCE_STATE_PRESENT)?;
            self.device.CreateRenderTargetView(&real_i, Some(&rd), at(r0, i, rs));
            self.device.CreateShaderResourceView(&game, Some(&srv_desc(w.format)), at(s0, 2 * i, ss));
            self.device.CreateShaderResourceView(&scene, Some(&srv_desc(w.format)), at(s0, 2 * i + 1, ss));
            let alloc: ID3D12CommandAllocator = self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
            let list: ID3D12GraphicsCommandList = self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None)?;
            list.Close()?;
            frames.push(Frame { game, real: real_i, alloc, list, done: 0 });
        }
        real.SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709)?;
        // Esta passa a ser a cadeia que separa a interface.
        GAME.iter().chain(HANDLES.iter()).for_each(|g| g.store(0, Relaxed));
        PHASES.iter().for_each(|p| p.store(hdr::PHASE_NONE, Relaxed));
        for (g, f) in GAME.iter().zip(&frames) {
            g.store(f.game.as_raw() as usize, Relaxed);
        }
        SCENE.store(scene.as_raw() as usize, Relaxed);
        SIZE.store((w.width as u64) << 32 | w.height as u64, Relaxed);
        (self.frames, self.srv, self.rtv, self.scene) = (frames, Some(srv), Some(rtv), Some(scene));
        Ok(())
    }

    fn splits(&self) -> bool {
        self.frames.first().is_some_and(|f| GAME[0].load(Relaxed) == f.game.as_raw() as usize)
    }

    pub unsafe fn buffer(&self, index: u32, riid: *const GUID, out: *mut Ptr) -> HRESULT {
        match self.frames.get(index as usize) {
            Some(f) => f.game.query(riid, out),
            None => DXGI_ERROR_INVALID_CALL,
        }
    }

    /// Grava e manda o passe do índice `i`; devolve a média do tempo de GPU em ms.
    unsafe fn draw(&mut self, i: usize, p: Params, s: &Shared) -> Result<f64> {
        let (Some(srv), Some(rtv), Some(scene)) = (&self.srv, &self.rtv, &self.scene) else { return Ok(0.0) };
        let f = self.frames.get(i).ok_or(Error::from(E_FAIL))?;
        self.wait(f.done);
        if let Some(t) = self.timer.as_mut().filter(|_| f.done > 0) {
            let range = D3D12_RANGE { Begin: i * 16, End: i * 16 + 16 };
            let mut ptr = std::ptr::null_mut();
            if t.read.Map(0, Some(&range), Some(&mut ptr)).is_ok() {
                let v = std::slice::from_raw_parts((ptr as *const u64).add(i * 2), 2);
                let ms = v[1].saturating_sub(v[0]) as f64 * 1000.0 / t.freq;
                t.ms = if t.ms <= 0.0 { ms } else { t.ms * 0.9 + ms * 0.1 };
                t.read.Unmap(0, Some(&D3D12_RANGE::default()));
            }
        }
        let mut scan = self.scan.as_mut().and_then(|sc| sc.ring.start(s).then_some(sc));
        if let Some(sc) = scan.as_mut() {
            sc.take(s, i, self.want.mode());
        }
        f.alloc.Reset()?;
        let l = &f.list;
        l.Reset(&f.alloc, &self.pso)?;
        l.ResourceBarrier(&[
            transition(&f.game, D3D12_RESOURCE_STATE_PRESENT, READ),
            transition(&f.real, D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_RENDER_TARGET),
            transition(scene, D3D12_RESOURCE_STATE_COPY_DEST, READ),
        ]);
        l.SetDescriptorHeaps(&[Some(srv.clone())]);
        let consts = (std::mem::size_of::<Params>() / 4) as u32;
        let step = self.device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) as u64;
        let mut table = srv.GetGPUDescriptorHandleForHeapStart();
        table.ptr += 2 * i as u64 * step;
        let q = (2 * i) as u32;
        if let Some(t) = &self.timer {
            l.EndQuery(&t.heap, D3D12_QUERY_TYPE_TIMESTAMP, q);
        }
        let (w, h) = (self.want.width, self.want.height);
        if let Some(sc) = scan {
            l.SetComputeRootSignature(&self.root);
            l.SetComputeRoot32BitConstants(0, consts, &p as *const Params as *const c_void, 0);
            l.SetComputeRootDescriptorTable(1, table);
            l.SetComputeRootUnorderedAccessView(2, sc.buf.GetGPUVirtualAddress());
            l.SetPipelineState(&sc.pso);
            l.Dispatch(w.div_ceil(scan::TILE), h.div_ceil(scan::TILE), 1);
            l.ResourceBarrier(&[transition(&sc.buf, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            l.CopyBufferRegion(&sc.read, i as u64 * scan::BYTES, &sc.buf, 0, scan::BYTES);
            l.ResourceBarrier(&[transition(&sc.buf, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)]);
            l.SetPipelineState(&self.pso);
            sc.ring.wrote(i);
        }
        l.SetGraphicsRootSignature(&self.root);
        l.SetGraphicsRoot32BitConstants(0, consts, &p as *const Params as *const c_void, 0);
        l.SetGraphicsRootDescriptorTable(1, table);
        l.RSSetViewports(&[D3D12_VIEWPORT { Width: w as f32, Height: h as f32, MaxDepth: 1.0, ..Default::default() }]);
        l.RSSetScissorRects(&[RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 }]);
        let target = at(rtv.GetCPUDescriptorHandleForHeapStart(), i, self.device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV));
        l.OMSetRenderTargets(1, Some(&target), false, None);
        l.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        l.DrawInstanced(3, 1, 0, 0);
        if let Some(t) = &self.timer {
            l.EndQuery(&t.heap, D3D12_QUERY_TYPE_TIMESTAMP, q + 1);
            l.ResolveQueryData(&t.heap, D3D12_QUERY_TYPE_TIMESTAMP, q, 2, &t.read, q as u64 * 8);
        }
        l.ResourceBarrier(&[
            transition(&f.game, READ, D3D12_RESOURCE_STATE_PRESENT),
            transition(&f.real, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_PRESENT),
            transition(scene, READ, D3D12_RESOURCE_STATE_COPY_DEST),
        ]);
        l.Close()?;
        self.queue.ExecuteCommandLists(&[Some(l.cast()?)]);
        self.value += 1;
        self.queue.Signal(&self.fence, self.value)?;
        self.frames[i].done = self.value;
        Ok(self.timer.as_ref().map_or(0.0, |t| t.ms))
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.drop_buffers();
    }
}

pub(crate) fn has(chain: Ptr) -> bool {
    proxies().iter().any(|p| p.chain == chain as usize)
}

/// Chamado antes de cada apresentação; `false` quando a cadeia não é um proxy do D3D12.
pub(crate) unsafe fn present(s: &Shared, chain: Ptr) -> bool {
    let mut list = proxies();
    let Some(p) = list.iter_mut().find(|p| p.chain == chain as usize) else { return false };
    let Some(real) = IDXGISwapChain::from_raw_borrowed(&chain).and_then(|c| c.cast::<IDXGISwapChain3>().ok()) else { return true };
    let i = real.GetCurrentBackBufferIndex() as usize;
    let phase = match PHASES.get(i) {
        Some(ph) if p.splits() => ph.swap(hdr::PHASE_NONE, Relaxed),
        _ => hdr::PHASE_NONE,
    };
    let split = hdr::split_of(s, phase);
    let pr = hdr::params(s, hdr::is_srgb(p.want.format), p.want.mode(), split == hdr::SPLIT_ON);
    match hdr::passthrough(|| p.draw(i, pr, s)) {
        Ok(ms) => hdr::report(s, ms, split, p.want.mode()),
        Err(e) => hdr::note(e.code()),
    }
    true
}

/// `ResizeBuffers` e `ResizeBuffers1` de um proxy do D3D12: a cadeia de verdade segue FP16 e os buffers do jogo são refeitos.
pub(crate) unsafe fn resize(this: Ptr, count: u32, format: DXGI_FORMAT, flags: u32, call: impl FnOnce(u32, DXGI_FORMAT, u32) -> HRESULT) -> HRESULT {
    let mut list = proxies();
    let Some(p) = list.iter_mut().find(|p| p.chain == this as usize) else {
        drop(list);
        return call(count, format, flags);
    };
    p.drop_buffers();
    let hr = hdr::passthrough(|| call(if count == 0 { 0 } else { count.max(2) }, DXGI_FORMAT_R16G16B16A16_FLOAT, flags));
    if format != DXGI_FORMAT_UNKNOWN {
        p.want.format = format;
    }
    if hr.is_err() {
        hdr::note(hr);
    }
    // Mesmo se falhou: sem buffers nossos, o `GetBuffer` daria ao jogo o FP16 de verdade.
    if let Err(e) = hdr::passthrough(|| p.rebuild(this)) {
        hdr::note(e.code());
    }
    hr
}

/// Vista pedida sem descrição sobre um buffer do jogo: ele nasce sem tipo, então ganha o formato pedido.
unsafe fn format_of(res: Ptr, desc: *const c_void) -> Option<DXGI_FORMAT> {
    if !desc.is_null() || res.is_null() || INSIDE.get() {
        return None;
    }
    proxies().iter().find(|p| p.frames.iter().any(|f| f.game.as_raw() == res)).map(|p| p.want.format)
}

unsafe extern "system" fn on_create_srv(this: Ptr, res: Ptr, desc: *const c_void, dest: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let old: CreateView = std::mem::transmute(OLD_CREATE_SRV.load(Relaxed));
    match format_of(res, desc) {
        Some(f) => old(this, res, &srv_desc(f) as *const _ as *const c_void, dest),
        None => old(this, res, desc, dest),
    }
}

unsafe extern "system" fn on_create_uav(this: Ptr, res: Ptr, counter: Ptr, desc: *const c_void, dest: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let old: CreateUav = std::mem::transmute(OLD_CREATE_UAV.load(Relaxed));
    match format_of(res, desc) {
        Some(f) => {
            let d = D3D12_UNORDERED_ACCESS_VIEW_DESC { Format: f, ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D, ..Default::default() };
            old(this, res, counter, &d as *const _ as *const c_void, dest)
        }
        None => old(this, res, counter, desc, dest),
    }
}

unsafe extern "system" fn on_create_rtv(this: Ptr, res: Ptr, desc: *const c_void, dest: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let old: CreateView = std::mem::transmute(OLD_CREATE_RTV.load(Relaxed));
    if !INSIDE.get() {
        track(res as usize, dest.ptr);
    }
    match format_of(res, desc) {
        Some(f) => {
            let d = D3D12_RENDER_TARGET_VIEW_DESC { Format: f, ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D, ..Default::default() };
            old(this, res, &d as *const _ as *const c_void, dest)
        }
        None => old(this, res, desc, dest),
    }
}

/// Lembra em que buffer do jogo cada vista de alvo aponta. Uma vista refeita no mesmo lugar esquece a antiga.
fn track(res: usize, handle: usize) {
    for h in &HANDLES {
        let _ = h.compare_exchange(handle, 0, Relaxed, Relaxed);
    }
    if let Some(i) = GAME.iter().position(|g| res != 0 && g.load(Relaxed) == res) {
        let slot = if HANDLES[2 * i].load(Relaxed) == 0 { 2 * i } else { 2 * i + 1 };
        HANDLES[slot].store(handle, Relaxed);
    }
}

unsafe extern "system" fn on_om_set(this: Ptr, count: u32, rtvs: *const D3D12_CPU_DESCRIPTOR_HANDLE, single: BOOL, dsv: *const D3D12_CPU_DESCRIPTOR_HANDLE) {
    let old: OmSet = std::mem::transmute(OLD_OM_SET.load(Relaxed));
    old(this, count, rtvs, single, dsv);
    if INSIDE.get() {
        return;
    }
    let first = (count > 0 && !rtvs.is_null() && GAME[0].load(Relaxed) != 0).then(|| (*rtvs).ptr).unwrap_or(0);
    let idx = if first == 0 { 0 } else { HANDLES.iter().position(|h| h.load(Relaxed) == first).map_or(0, |j| j / 2 + 1) };
    BB.set((this as usize, idx));
}

unsafe extern "system" fn on_set_viewports(this: Ptr, count: u32, vps: *const D3D12_VIEWPORT) {
    let old: SetViewports = std::mem::transmute(OLD_SET_VIEWPORTS.load(Relaxed));
    old(this, count, vps);
    if INSIDE.get() || count == 0 || vps.is_null() {
        return;
    }
    let size = SIZE.load(Relaxed);
    let (w, h) = ((size >> 32) as f32, (size & 0xffff_ffff) as f32);
    let v = &*vps;
    VP.set((this as usize, h > 0.0 && v.TopLeftX <= 0.5 && v.TopLeftY <= 0.5 && v.Width >= w - 1.0 && v.Height >= h - 1.0));
}

/// Depois de cada desenho num buffer do jogo: o passe de tela cheia guarda a cena (uma cópia gravada na
/// própria lista do jogo), o resto é interface.
/// shortcut: tela cheia é um triângulo ou um quadrado na área toda, sem olhar as imagens lidas (o D3D12 não
/// deixa ler a tabela de descritores); `BeginRenderPass`, pacotes e desenhos indiretos não contam.
unsafe fn drawn(list: Ptr, count: u32, instances: u32) {
    let (l, idx) = BB.get();
    if l != list as usize || idx == 0 || INSIDE.get() {
        return;
    }
    let i = idx - 1;
    let full = count <= 6 && instances <= 1 && VP.get() == (list as usize, true);
    let (next, snap) = hdr::step(PHASES[i].load(Relaxed), full);
    if snap {
        let (game, scene) = (GAME[i].load(Relaxed), SCENE.load(Relaxed));
        if let (Some(l), Some(g), Some(s)) = (ID3D12GraphicsCommandList::from_raw_borrowed(&list), ID3D12Resource::from_raw_borrowed(&(game as Ptr)), ID3D12Resource::from_raw_borrowed(&(scene as Ptr))) {
            l.ResourceBarrier(&[transition(g, D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
            l.CopyResource(s, g);
            l.ResourceBarrier(&[transition(g, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_RENDER_TARGET)]);
        }
    }
    PHASES[i].store(next, Relaxed);
}

unsafe extern "system" fn on_draw_instanced(this: Ptr, count: u32, instances: u32, start: u32, first: u32) {
    let old: DrawInstanced = std::mem::transmute(OLD_DRAW_INSTANCED.load(Relaxed));
    old(this, count, instances, start, first);
    drawn(this, count, instances);
}

unsafe extern "system" fn on_draw_indexed_instanced(this: Ptr, count: u32, instances: u32, start: u32, base: i32, first: u32) {
    let old: DrawIndexedInstanced = std::mem::transmute(OLD_DRAW_INDEXED_INSTANCED.load(Relaxed));
    old(this, count, instances, start, base, first);
    drawn(this, count, instances);
}

fn table(obj: Ptr) -> *mut usize {
    unsafe { *(obj as *const *mut usize) }
}

unsafe fn hook_device(device: Ptr) {
    let t = table(device);
    patch(t, SLOT_CREATE_SRV, on_create_srv as *const () as usize, &OLD_CREATE_SRV);
    patch(t, SLOT_CREATE_UAV, on_create_uav as *const () as usize, &OLD_CREATE_UAV);
    patch(t, SLOT_CREATE_RTV, on_create_rtv as *const () as usize, &OLD_CREATE_RTV);
}

/// As listas diretas do dispositivo dividem a tabela: a da nossa vale para as do jogo.
unsafe fn hook_list(list: Ptr) {
    let t = table(list);
    patch(t, SLOT_OM_SET, on_om_set as *const () as usize, &OLD_OM_SET);
    patch(t, SLOT_RS_SET_VIEWPORTS, on_set_viewports as *const () as usize, &OLD_SET_VIEWPORTS);
    patch(t, SLOT_DRAW_INSTANCED, on_draw_instanced as *const () as usize, &OLD_DRAW_INSTANCED);
    patch(t, SLOT_DRAW_INDEXED_INSTANCED, on_draw_indexed_instanced as *const () as usize, &OLD_DRAW_INDEXED_INSTANCED);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_signature_compiles() {
        unsafe { hdr::compile(s!("rootsig_1_0"), s!("RS")).unwrap() };
    }

    #[test]
    fn rtv_tracking_follows_reuse() {
        GAME[0].store(0x1000, Relaxed);
        GAME[1].store(0x2000, Relaxed);
        track(0x1000, 0xa);
        track(0x1000, 0xb);
        track(0x2000, 0xc);
        assert_eq!([HANDLES[0].load(Relaxed), HANDLES[1].load(Relaxed), HANDLES[2].load(Relaxed)], [0xa, 0xb, 0xc]);
        // A vista 0xa foi refeita para outra textura: deixa de ser do buffer 0.
        track(0x9999, 0xa);
        assert_eq!(HANDLES[0].load(Relaxed), 0);
        GAME.iter().chain(HANDLES.iter()).for_each(|g| g.store(0, Relaxed));
    }
}
