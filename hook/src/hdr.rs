//! HDR próprio: o jogo desenha num buffer SDR nosso, e a cadeia de troca de verdade vira FP16 scRGB.
//! No `Present`, um passe de tela cheia passa o quadro do jogo pela nossa curva (gama 2.2, paper
//! white, brilhos até o pico, saturação). Só D3D11 e só janela: o resto passa direto.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering::*};
use std::sync::{Mutex, MutexGuard};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::Fxc::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::WindowsAndMessaging::IsWindow;

use crate::{patch, unpatch, Ptr, Shared};

const SLOT_CREATE: usize = 10;
const SLOT_CREATE_HWND: usize = 15;
const SLOT_GET_BUFFER: usize = 9;
const SLOT_GET_DESC: usize = 12;
const SLOT_RESIZE: usize = 13;
const SLOT_GET_DESC1: usize = 18;
const SLOT_SET_COLOR_SPACE: usize = 38;
const SLOT_RESIZE1: usize = 39;
const SLOT_CREATE_SRV: usize = 7;
const SLOT_CREATE_UAV: usize = 8;
const SLOT_CREATE_RTV: usize = 9;
const GDI_COMPATIBLE: u32 = 4;
const USAGE_UAV: u32 = 0x400;

type Create = unsafe extern "system" fn(Ptr, Ptr, *const DXGI_SWAP_CHAIN_DESC, *mut Ptr) -> HRESULT;
type CreateHwnd = unsafe extern "system" fn(Ptr, Ptr, HWND, *const DXGI_SWAP_CHAIN_DESC1, *const DXGI_SWAP_CHAIN_FULLSCREEN_DESC, Ptr, *mut Ptr) -> HRESULT;
type GetBuffer = unsafe extern "system" fn(Ptr, u32, *const GUID, *mut Ptr) -> HRESULT;
type GetDesc = unsafe extern "system" fn(Ptr, *mut DXGI_SWAP_CHAIN_DESC) -> HRESULT;
type GetDesc1 = unsafe extern "system" fn(Ptr, *mut DXGI_SWAP_CHAIN_DESC1) -> HRESULT;
type Resize = unsafe extern "system" fn(Ptr, u32, u32, u32, DXGI_FORMAT, u32) -> HRESULT;
type Resize1 = unsafe extern "system" fn(Ptr, u32, u32, u32, DXGI_FORMAT, u32, *const u32, *const Ptr) -> HRESULT;
type SetColorSpace = unsafe extern "system" fn(Ptr, DXGI_COLOR_SPACE_TYPE) -> HRESULT;
type CreateView = unsafe extern "system" fn(Ptr, Ptr, *const c_void, *mut Ptr) -> HRESULT;

static OLD_CREATE: AtomicUsize = AtomicUsize::new(0);
static OLD_CREATE_HWND: AtomicUsize = AtomicUsize::new(0);
static OLD_GET_BUFFER: AtomicUsize = AtomicUsize::new(0);
static OLD_GET_DESC: AtomicUsize = AtomicUsize::new(0);
static OLD_GET_DESC1: AtomicUsize = AtomicUsize::new(0);
static OLD_RESIZE: AtomicUsize = AtomicUsize::new(0);
static OLD_RESIZE1: AtomicUsize = AtomicUsize::new(0);
static OLD_SET_COLOR_SPACE: AtomicUsize = AtomicUsize::new(0);
static OLD_CREATE_SRV: AtomicUsize = AtomicUsize::new(0);
static OLD_CREATE_UAV: AtomicUsize = AtomicUsize::new(0);
static OLD_CREATE_RTV: AtomicUsize = AtomicUsize::new(0);
static PROXIES: Mutex<Vec<Proxy>> = Mutex::new(Vec::new());

thread_local! {
    /// Nosso código rodando nesta linha (criar, redimensionar, desenhar, o resto do Present): os
    /// ganchos deixam passar direto. Evita trava dupla e mostra a cadeia de verdade a overlays.
    static INSIDE: Cell<bool> = const { Cell::new(false) };
}

const SHADER: &str = r#"
Texture2D<float4> tex : register(t0);
cbuffer C : register(b0) { float paper, peak, gamma, sat, boost, black; uint srgb, on; };
float4 vs(uint id : SV_VertexID) : SV_Position {
    float2 t = float2((id << 1) & 2, id & 2);
    return float4(t * float2(2, -2) + float2(-1, 1), 0, 1);
}
float3 encode_srgb(float3 l) { return l <= 0.0031308 ? l * 12.92 : 1.055 * pow(l, 1 / 2.4) - 0.055; }
float3 decode_srgb(float3 v) { return v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4); }
float4 ps(float4 p : SV_Position) : SV_Target {
    float3 c = tex.Load(int3(p.xy, 0)).rgb;
    // Buffer _SRGB: a leitura já decodificou pela curva por partes; volta ao valor que o jogo gravou.
    c = saturate(srgb ? encode_srgb(saturate(c)) : c);
    // Desligado: o mesmo que o Windows faz com SDR, curva sRGB por partes no paper white.
    if (!on) return float4(decode_srgb(c) * paper / 80, 1);
    // Gama 2.2 pura: a curva por partes levanta os pretos.
    float3 l = pow(c, gamma);
    float y = dot(l, float3(0.2126, 0.7152, 0.0722));
    l = max(y + (l - y) * sat, 0);
    // Só os brilhos sobem: o ganho cresce com y^3 e chega ao pico no branco.
    float gain = 1 + (peak / paper - 1) * boost * y * y * y;
    float3 n = l * paper * gain;
    n = n * (1 - black / peak) + black;
    return float4(min(n, peak) / 80, 1);
}
"#;

#[repr(C)]
#[derive(Clone, Copy)]
struct Params {
    paper: f32,
    peak: f32,
    gamma: f32,
    sat: f32,
    boost: f32,
    black: f32,
    srgb: u32,
    on: u32,
}

/// O que o jogo pediu: o buffer dele segue isso, não a cadeia de verdade.
#[derive(Clone, Copy)]
struct Want {
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    samples: u32,
    usage: u32,
}

struct Proxy {
    chain: usize,
    hwnd: usize,
    want: Want,
    device: ID3D11Device,
    ctx: ID3D11DeviceContext1,
    state: ID3DDeviceContextState,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    cb: ID3D11Buffer,
    /// O buffer que o jogo recebe no `GetBuffer`.
    tex: Option<ID3D11Texture2D>,
    /// Com MSAA, a cópia resolvida que o shader lê.
    resolved: Option<ID3D11Texture2D>,
    srv: Option<ID3D11ShaderResourceView>,
    /// O buffer 0 da cadeia de verdade: no modelo flip do D3D11 ele sempre aponta o buffer da vez.
    rtv: Option<ID3D11RenderTargetView>,
}

// Os objetos COM só são usados com a trava pega.
unsafe impl Send for Proxy {}

fn proxies() -> MutexGuard<'static, Vec<Proxy>> {
    PROXIES.lock().unwrap_or_else(|e| e.into_inner())
}

fn shared() -> Option<&'static Shared> {
    let at = crate::SHARED.load(Relaxed);
    (at != 0).then(|| unsafe { &*(at as *const Shared) })
}

/// A pessoa abriu o jogo pelo Prisma com o HDR pedido.
pub fn wanted() -> bool {
    shared().is_some_and(|s| s.hdr_proxy.load(Relaxed) != 0)
}

fn note(hr: HRESULT) {
    if let Some(s) = shared() {
        s.hdr_error.store(hr.0 as u32, Relaxed);
    }
}

/// O pedido do jogo para esta cadeia, quando ela é nossa e o chamador não é o nosso código.
fn want_of(chain: Ptr) -> Option<Want> {
    if INSIDE.get() {
        return None;
    }
    proxies().iter().find(|p| p.chain == chain as usize).map(|p| p.want)
}

/// Roda `f` marcado como nosso: o resto do `Present` (e overlays pendurados nele) vê a cadeia de verdade.
pub fn passthrough<T>(f: impl FnOnce() -> T) -> T {
    let was = INSIDE.replace(true);
    let r = f();
    INSIDE.set(was);
    r
}

unsafe fn compile(target: PCSTR, entry: PCSTR) -> Result<ID3DBlob> {
    let mut blob = None;
    D3DCompile(SHADER.as_ptr() as *const _, SHADER.len(), None, None, None, entry, target, D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &mut blob, None)?;
    blob.ok_or_else(|| E_FAIL.into())
}

unsafe fn bytes(b: &ID3DBlob) -> &[u8] {
    std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize())
}

fn is_srgb(f: DXGI_FORMAT) -> bool {
    matches!(f, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8X8_UNORM_SRGB)
}

/// O buffer de uma cadeia aceita vista _SRGB sobre formato UNORM (o Unity faz isso); textura comum só
/// aceita se nascer sem tipo. Vista sem descrição ganha o formato pedido em `view`.
fn typeless(f: DXGI_FORMAT) -> DXGI_FORMAT {
    match f {
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => DXGI_FORMAT_R8G8B8A8_TYPELESS,
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => DXGI_FORMAT_B8G8R8A8_TYPELESS,
        DXGI_FORMAT_B8G8R8X8_UNORM | DXGI_FORMAT_B8G8R8X8_UNORM_SRGB => DXGI_FORMAT_B8G8R8X8_TYPELESS,
        DXGI_FORMAT_R10G10B10A2_UNORM => DXGI_FORMAT_R10G10B10A2_TYPELESS,
        f => f,
    }
}

impl Proxy {
    unsafe fn new(chain: Ptr, hwnd: HWND, device: ID3D11Device, want: Want) -> Result<Self> {
        let d1: ID3D11Device1 = device.cast()?;
        let ctx: ID3D11DeviceContext1 = device.GetImmediateContext()?.cast()?;
        // Um estado de contexto só nosso: o do jogo volta intacto depois do passe.
        let single = device.GetCreationFlags() & D3D11_CREATE_DEVICE_SINGLETHREADED.0 as u32;
        let mut state = None;
        d1.CreateDeviceContextState(single, &[device.GetFeatureLevel()], D3D11_SDK_VERSION, &ID3D11Device::IID, None, Some(&mut state))?;
        let (vsb, psb) = (compile(s!("vs_5_0"), s!("vs"))?, compile(s!("ps_5_0"), s!("ps"))?);
        let (mut vs, mut ps, mut cb) = (None, None, None);
        device.CreateVertexShader(bytes(&vsb), None, Some(&mut vs))?;
        device.CreatePixelShader(bytes(&psb), None, Some(&mut ps))?;
        let bd = D3D11_BUFFER_DESC { ByteWidth: std::mem::size_of::<Params>() as u32, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32, ..Default::default() };
        device.CreateBuffer(&bd, None, Some(&mut cb))?;
        let mut me = Self {
            chain: chain as usize,
            hwnd: hwnd.0 as usize,
            want,
            device,
            ctx,
            state: state.ok_or(Error::from(E_FAIL))?,
            vs: vs.ok_or(Error::from(E_FAIL))?,
            ps: ps.ok_or(Error::from(E_FAIL))?,
            cb: cb.ok_or(Error::from(E_FAIL))?,
            tex: None,
            resolved: None,
            srv: None,
            rtv: None,
        };
        me.rebuild(chain)?;
        Ok(me)
    }

    /// Solta tudo que segura buffers: o `ResizeBuffers` falha com algum preso.
    fn drop_buffers(&mut self) {
        (self.tex, self.resolved, self.srv, self.rtv) = (None, None, None, None);
    }

    /// Refaz o buffer do jogo no tamanho atual da cadeia de verdade. Só com `INSIDE` ligado.
    unsafe fn rebuild(&mut self, chain: Ptr) -> Result<()> {
        self.drop_buffers();
        let real = IDXGISwapChain::from_raw_borrowed(&chain).ok_or(Error::from(E_POINTER))?;
        let bb: ID3D11Texture2D = real.GetBuffer(0)?;
        let mut bd = D3D11_TEXTURE2D_DESC::default();
        bb.GetDesc(&mut bd);
        (self.want.width, self.want.height) = (bd.Width, bd.Height);
        let rd = D3D11_RENDER_TARGET_VIEW_DESC { Format: DXGI_FORMAT_R16G16B16A16_FLOAT, ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D, ..Default::default() };
        let mut rtv = None;
        self.device.CreateRenderTargetView(&bb, Some(&rd), Some(&mut rtv))?;
        let w = self.want;
        let mut bind = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
        if w.usage & USAGE_UAV != 0 {
            bind |= D3D11_BIND_UNORDERED_ACCESS.0 as u32;
        }
        let mut td = D3D11_TEXTURE2D_DESC {
            Width: w.width,
            Height: w.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: typeless(w.format),
            SampleDesc: DXGI_SAMPLE_DESC { Count: w.samples.max(1), Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: bind,
            ..Default::default()
        };
        let mut tex = None;
        self.device.CreateTexture2D(&td, None, Some(&mut tex))?;
        let tex = tex.ok_or(Error::from(E_FAIL))?;
        let read = if w.samples > 1 {
            td.SampleDesc.Count = 1;
            td.BindFlags = D3D11_BIND_SHADER_RESOURCE.0 as u32;
            let mut r = None;
            self.device.CreateTexture2D(&td, None, Some(&mut r))?;
            self.resolved = r;
            self.resolved.clone().ok_or(Error::from(E_FAIL))?
        } else {
            tex.clone()
        };
        let mut srv = None;
        self.device.CreateShaderResourceView(&read, Some(&srv_desc(w.format, false)), Some(&mut srv))?;
        (self.srv, self.rtv, self.tex) = (srv, rtv, Some(tex));
        if let Ok(c3) = real.cast::<IDXGISwapChain3>() {
            c3.SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709)?;
        }
        Ok(())
    }

    unsafe fn draw(&self, p: Params) {
        let (Some(tex), Some(srv), Some(rtv)) = (&self.tex, &self.srv, &self.rtv) else { return };
        if let Some(r) = &self.resolved {
            self.ctx.ResolveSubresource(r, 0, tex, 0, self.want.format);
        }
        let mut prev = None;
        self.ctx.SwapDeviceContextState(&self.state, Some(&mut prev));
        self.ctx.UpdateSubresource(&self.cb, 0, None, &p as *const _ as *const c_void, 0, 0);
        self.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        self.ctx.VSSetShader(&self.vs, None);
        self.ctx.PSSetShader(&self.ps, None);
        self.ctx.PSSetShaderResources(0, Some(&[Some(srv.clone())]));
        self.ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
        self.ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
        let vp = D3D11_VIEWPORT { Width: self.want.width as f32, Height: self.want.height as f32, MaxDepth: 1.0, ..Default::default() };
        self.ctx.RSSetViewports(Some(&[vp]));
        self.ctx.Draw(3, 0);
        // Solta o buffer do jogo e o de verdade antes de devolver o estado: nada nosso fica preso.
        self.ctx.PSSetShaderResources(0, Some(&[None]));
        self.ctx.OMSetRenderTargets(None, None);
        if let Some(prev) = prev {
            self.ctx.SwapDeviceContextState(&prev, None);
        }
    }
}

fn srv_desc(format: DXGI_FORMAT, ms: bool) -> D3D11_SHADER_RESOURCE_VIEW_DESC {
    let mut d = D3D11_SHADER_RESOURCE_VIEW_DESC { Format: format, ViewDimension: D3D11_SRV_DIMENSION_TEXTURE2DMS, ..Default::default() };
    if !ms {
        d.ViewDimension = D3D11_SRV_DIMENSION_TEXTURE2D;
        d.Anonymous.Texture2D = D3D11_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1 };
    }
    d
}

fn params(s: &Shared, srgb: bool) -> Params {
    let f = |a: &std::sync::atomic::AtomicU32| f32::from_bits(a.load(Relaxed));
    let peak = f(&s.hdr_peak).max(80.0);
    Params {
        paper: f(&s.hdr_paper).clamp(80.0, peak),
        peak,
        gamma: f(&s.hdr_gamma).clamp(1.0, 3.0),
        sat: f(&s.hdr_sat).clamp(0.0, 2.0),
        boost: f(&s.hdr_boost).clamp(0.0, 1.0),
        black: f(&s.hdr_black).clamp(0.0, 1.0),
        srgb: srgb as u32,
        on: (s.hdr_on.load(Relaxed) != 0) as u32,
    }
}

/// Chamado antes de cada apresentação: desenha o quadro do jogo na cadeia FP16.
pub unsafe fn present(chain: Ptr, flags: u32) {
    let Some(s) = shared() else { return };
    if flags & crate::PRESENT_TEST != 0 || INSIDE.get() {
        return;
    }
    let list = proxies();
    let Some(p) = list.iter().find(|p| p.chain == chain as usize) else { return };
    passthrough(|| p.draw(params(s, is_srgb(p.want.format))));
    s.hdr_frames.fetch_add(1, Relaxed);
}

/// Ajusta o pedido do jogo para a cadeia de verdade: FP16, troca por flip, sem MSAA.
fn flip(effect: DXGI_SWAP_EFFECT) -> DXGI_SWAP_EFFECT {
    if effect == DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL { effect } else { DXGI_SWAP_EFFECT_FLIP_DISCARD }
}

unsafe fn d3d11(device: Ptr) -> Option<ID3D11Device> {
    IUnknown::from_raw_borrowed(&device)?.cast().ok()
}

/// HDR do Windows ligado na tela da janela. Com ele desligado o FP16 cortaria o brilho: o jogo fica como está.
/// shortcut: vale na criação; ligar ou desligar o HDR com o jogo aberto pede reabrir o jogo.
unsafe fn screen_hdr(hwnd: HWND) -> bool {
    let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return false };
    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    // Todas as placas: em notebook a tela costuma estar na integrada e o jogo na dedicada.
    for a in (0..).map_while(|i| factory.EnumAdapters1(i).ok()) {
        for o in (0..).map_while(|i| a.EnumOutputs(i).ok()) {
            if let Ok(d) = o.cast::<IDXGIOutput6>().and_then(|o| o.GetDesc1()) {
                if d.Monitor == monitor {
                    return d.ColorSpace == DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
                }
            }
        }
    }
    false
}

/// Uma cadeia nova pode nascer no endereço de uma já solta, e janela fechada não volta: os proxies
/// dessas saem, senão o `GetBuffer` da cadeia nova devolveria o buffer velho.
unsafe fn created(hr: HRESULT, out: *mut Ptr, hwnd: HWND) -> HRESULT {
    if hr.is_ok() && !out.is_null() {
        let chain = *out as usize;
        proxies().retain(|p| p.chain != chain && p.hwnd != hwnd.0 as usize && IsWindow(Some(HWND(p.hwnd as *mut c_void))).as_bool());
    }
    hr
}

/// Cria a cadeia FP16 e o proxy; se algo falhar, cria a cadeia que o jogo pediu, sem HDR.
unsafe fn proxied(device: Ptr, hwnd: HWND, want: Want, out: *mut Ptr, real: impl Fn() -> HRESULT, plain: impl Fn() -> HRESULT) -> HRESULT {
    // FP16 pedido pelo jogo já é HDR dele: a curva cortaria os brilhos.
    let fit = !INSIDE.get() && !out.is_null() && want.format != DXGI_FORMAT_R16G16B16A16_FLOAT && wanted() && screen_hdr(hwnd);
    let Some(dev) = d3d11(device).filter(|_| fit) else { return created(plain(), out, hwnd) };
    INSIDE.set(true);
    let mut hr = real();
    if hr.is_ok() {
        hook_chain(*out);
        hook_device(&dev);
        match Proxy::new(*out, hwnd, dev, want) {
            Ok(p) => {
                let _ = created(hr, out, hwnd);
                proxies().push(p);
                if let Some(s) = shared() {
                    s.hdr_chains.fetch_add(1, Relaxed);
                }
            }
            Err(e) => {
                note(e.code());
                crate::release(*out);
                *out = std::ptr::null_mut();
                hr = created(plain(), out, hwnd);
            }
        }
    } else {
        note(hr);
        hr = created(plain(), out, hwnd);
    }
    INSIDE.set(false);
    hr
}

unsafe extern "system" fn on_create(this: Ptr, device: Ptr, desc: *const DXGI_SWAP_CHAIN_DESC, out: *mut Ptr) -> HRESULT {
    let old: Create = std::mem::transmute(OLD_CREATE.load(Relaxed));
    if desc.is_null() {
        return old(this, device, desc, out);
    }
    let d = *desc;
    // shortcut: tela cheia exclusiva passa direto, sem HDR; fica para a próxima fase.
    if !d.Windowed.as_bool() {
        return created(old(this, device, desc, out), out, d.OutputWindow);
    }
    let want = Want { width: d.BufferDesc.Width, height: d.BufferDesc.Height, format: d.BufferDesc.Format, samples: d.SampleDesc.Count, usage: d.BufferUsage.0 };
    let mut real = d;
    real.BufferDesc.Format = DXGI_FORMAT_R16G16B16A16_FLOAT;
    real.SampleDesc = DXGI_SAMPLE_DESC { Count: 1, Quality: 0 };
    real.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
    real.BufferCount = d.BufferCount.max(2);
    real.SwapEffect = flip(d.SwapEffect);
    real.Flags &= !GDI_COMPATIBLE;
    proxied(device, d.OutputWindow, want, out, || old(this, device, &real, out), || old(this, device, desc, out))
}

unsafe extern "system" fn on_create_hwnd(this: Ptr, device: Ptr, hwnd: HWND, desc: *const DXGI_SWAP_CHAIN_DESC1, fs: *const DXGI_SWAP_CHAIN_FULLSCREEN_DESC, output: Ptr, out: *mut Ptr) -> HRESULT {
    let old: CreateHwnd = std::mem::transmute(OLD_CREATE_HWND.load(Relaxed));
    if desc.is_null() || (!fs.is_null() && !(*fs).Windowed.as_bool()) {
        return created(old(this, device, hwnd, desc, fs, output, out), out, hwnd);
    }
    let d = *desc;
    let want = Want { width: d.Width, height: d.Height, format: d.Format, samples: d.SampleDesc.Count, usage: d.BufferUsage.0 };
    let mut real = d;
    real.Format = DXGI_FORMAT_R16G16B16A16_FLOAT;
    real.SampleDesc = DXGI_SAMPLE_DESC { Count: 1, Quality: 0 };
    real.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
    real.BufferCount = d.BufferCount.max(2);
    real.SwapEffect = flip(d.SwapEffect);
    real.Flags &= !GDI_COMPATIBLE;
    if real.Scaling == DXGI_SCALING_NONE && real.SwapEffect != d.SwapEffect {
        real.Scaling = DXGI_SCALING_STRETCH;
    }
    proxied(device, hwnd, want, out, || old(this, device, hwnd, &real, fs, output, out), || old(this, device, hwnd, desc, fs, output, out))
}

unsafe extern "system" fn on_get_buffer(this: Ptr, index: u32, riid: *const GUID, out: *mut Ptr) -> HRESULT {
    if !INSIDE.get() {
        let list = proxies();
        if let Some(tex) = list.iter().find(|p| p.chain == this as usize).and_then(|p| p.tex.as_ref()) {
            return tex.query(riid, out);
        }
    }
    let old: GetBuffer = std::mem::transmute(OLD_GET_BUFFER.load(Relaxed));
    old(this, index, riid, out)
}

/// O jogo vê o formato que pediu: quem monta a vista pelo `GetDesc` acertaria o FP16 e falharia.
unsafe extern "system" fn on_get_desc(this: Ptr, out: *mut DXGI_SWAP_CHAIN_DESC) -> HRESULT {
    let old: GetDesc = std::mem::transmute(OLD_GET_DESC.load(Relaxed));
    let hr = old(this, out);
    if let Some(w) = want_of(this).filter(|_| hr.is_ok() && !out.is_null()) {
        let d = &mut *out;
        (d.BufferDesc.Format, d.SampleDesc.Count, d.BufferUsage) = (w.format, w.samples.max(1), DXGI_USAGE(w.usage));
    }
    hr
}

unsafe extern "system" fn on_get_desc1(this: Ptr, out: *mut DXGI_SWAP_CHAIN_DESC1) -> HRESULT {
    let old: GetDesc1 = std::mem::transmute(OLD_GET_DESC1.load(Relaxed));
    let hr = old(this, out);
    if let Some(w) = want_of(this).filter(|_| hr.is_ok() && !out.is_null()) {
        let d = &mut *out;
        (d.Format, d.SampleDesc.Count, d.BufferUsage) = (w.format, w.samples.max(1), DXGI_USAGE(w.usage));
    }
    hr
}

/// O espaço de cor da cadeia é nosso (scRGB): o sRGB do jogo fica só de conta, e HDR10 falha como
/// numa tela sem HDR, para o jogo seguir no SDR que a curva espera.
unsafe extern "system" fn on_set_color_space(this: Ptr, space: DXGI_COLOR_SPACE_TYPE) -> HRESULT {
    if want_of(this).is_some() {
        return if space == DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709 { S_OK } else { DXGI_ERROR_UNSUPPORTED };
    }
    let old: SetColorSpace = std::mem::transmute(OLD_SET_COLOR_SPACE.load(Relaxed));
    old(this, space)
}

/// `ResizeBuffers` e `ResizeBuffers1`: a cadeia de verdade segue FP16 e o buffer do jogo é refeito.
unsafe fn resize(this: Ptr, count: u32, format: DXGI_FORMAT, flags: u32, call: impl FnOnce(u32, DXGI_FORMAT, u32) -> HRESULT) -> HRESULT {
    let mut list = proxies();
    let Some(p) = list.iter_mut().find(|p| !INSIDE.get() && p.chain == this as usize) else {
        drop(list);
        return call(count, format, flags);
    };
    p.drop_buffers();
    let hr = passthrough(|| call(if count == 0 { 0 } else { count.max(2) }, DXGI_FORMAT_R16G16B16A16_FLOAT, flags & !GDI_COMPATIBLE));
    if format != DXGI_FORMAT_UNKNOWN {
        p.want.format = format;
    }
    if hr.is_err() {
        note(hr);
    }
    // Mesmo se falhou: sem buffer nosso, o `GetBuffer` daria ao jogo o FP16 de verdade.
    if let Err(e) = passthrough(|| p.rebuild(this)) {
        note(e.code());
    }
    hr
}

unsafe extern "system" fn on_resize(this: Ptr, count: u32, w: u32, h: u32, format: DXGI_FORMAT, flags: u32) -> HRESULT {
    let old: Resize = std::mem::transmute(OLD_RESIZE.load(Relaxed));
    resize(this, count, format, flags, |c, f, fl| old(this, c, w, h, f, fl))
}

/// Máscara e filas são do D3D12, que nunca vira proxy: com D3D11 vêm nulas.
unsafe extern "system" fn on_resize1(this: Ptr, count: u32, w: u32, h: u32, format: DXGI_FORMAT, flags: u32, mask: *const u32, queues: *const Ptr) -> HRESULT {
    let old: Resize1 = std::mem::transmute(OLD_RESIZE1.load(Relaxed));
    resize(this, count, format, flags, |c, f, fl| old(this, c, w, h, f, fl, mask, queues))
}

/// Vista pedida sem descrição sobre o buffer do jogo: ele nasce sem tipo, então ganha o formato que o
/// jogo pediu, como aconteceria num buffer de cadeia.
unsafe fn view<D>(old: &AtomicUsize, this: Ptr, res: Ptr, desc: *const c_void, out: *mut Ptr, make: impl FnOnce(Want) -> D) -> HRESULT {
    let old: CreateView = std::mem::transmute(old.load(Relaxed));
    let want = (desc.is_null() && !INSIDE.get()).then(|| proxies().iter().find(|p| p.tex.as_ref().is_some_and(|t| t.as_raw() == res)).map(|p| p.want)).flatten();
    match want {
        Some(w) => {
            let d = make(w);
            old(this, res, &d as *const D as *const c_void, out)
        }
        None => old(this, res, desc, out),
    }
}

unsafe extern "system" fn on_create_srv(this: Ptr, res: Ptr, desc: *const c_void, out: *mut Ptr) -> HRESULT {
    view(&OLD_CREATE_SRV, this, res, desc, out, |w| srv_desc(w.format, w.samples > 1))
}

unsafe extern "system" fn on_create_rtv(this: Ptr, res: Ptr, desc: *const c_void, out: *mut Ptr) -> HRESULT {
    view(&OLD_CREATE_RTV, this, res, desc, out, |w| D3D11_RENDER_TARGET_VIEW_DESC {
        Format: w.format,
        ViewDimension: if w.samples > 1 { D3D11_RTV_DIMENSION_TEXTURE2DMS } else { D3D11_RTV_DIMENSION_TEXTURE2D },
        ..Default::default()
    })
}

unsafe extern "system" fn on_create_uav(this: Ptr, res: Ptr, desc: *const c_void, out: *mut Ptr) -> HRESULT {
    view(&OLD_CREATE_UAV, this, res, desc, out, |w| D3D11_UNORDERED_ACCESS_VIEW_DESC { Format: w.format, ViewDimension: D3D11_UAV_DIMENSION_TEXTURE2D, ..Default::default() })
}

fn table(obj: Ptr) -> *mut usize {
    unsafe { *(obj as *const *mut usize) }
}

/// Liga os ganchos da cadeia que mudam com o proxy. A tabela é a mesma do jogo. As interfaces novas
/// (1 e 3) usam a tabela delas e são opcionais: sem elas o limitador segue valendo.
pub unsafe fn hook_chain(chain: Ptr) -> bool {
    let t = table(chain);
    let done = crate::hook_present(chain)
        & patch(t, SLOT_GET_BUFFER, on_get_buffer as usize, &OLD_GET_BUFFER)
        & patch(t, SLOT_GET_DESC, on_get_desc as usize, &OLD_GET_DESC)
        & patch(t, SLOT_RESIZE, on_resize as usize, &OLD_RESIZE);
    let Some(c) = IDXGISwapChain::from_raw_borrowed(&chain) else { return done };
    if let Ok(c1) = c.cast::<IDXGISwapChain1>() {
        patch(table(c1.as_raw()), SLOT_GET_DESC1, on_get_desc1 as usize, &OLD_GET_DESC1);
    }
    if let Ok(c3) = c.cast::<IDXGISwapChain3>() {
        let t3 = table(c3.as_raw());
        patch(t3, SLOT_SET_COLOR_SPACE, on_set_color_space as usize, &OLD_SET_COLOR_SPACE);
        patch(t3, SLOT_RESIZE1, on_resize1 as usize, &OLD_RESIZE1);
    }
    done
}

/// shortcut: só as vistas do D3D11.0; as `...1` do D3D11.3 com descrição nula falham no buffer sem
/// tipo. Ligar se aparecer jogo que use.
unsafe fn hook_device(device: &ID3D11Device) {
    let t = table(device.as_raw());
    patch(t, SLOT_CREATE_SRV, on_create_srv as usize, &OLD_CREATE_SRV);
    patch(t, SLOT_CREATE_UAV, on_create_uav as usize, &OLD_CREATE_UAV);
    patch(t, SLOT_CREATE_RTV, on_create_rtv as usize, &OLD_CREATE_RTV);
}

/// Uma fábrica descartável mostra a tabela que o jogo vai usar para criar a cadeia.
pub unsafe fn install() -> bool {
    let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return false };
    let mut done = patch(table(factory.as_raw()), SLOT_CREATE, on_create as usize, &OLD_CREATE);
    if let Ok(f2) = factory.cast::<IDXGIFactory2>() {
        done &= patch(table(f2.as_raw()), SLOT_CREATE_HWND, on_create_hwnd as usize, &OLD_CREATE_HWND);
    }
    done
}

/// O overlay da Steam desiste de ligar os ganchos dele se a tabela da fábrica já aponta para nós, e o
/// Shift+Tab some. Com o gancho dele pronto, a tabela volta ao Windows por um instante, uma fábrica
/// nossa passa por ele (que liga os dele) e a nossa troca volta por cima, chamando a dele.
// shortcut: se o jogo criar a cadeia antes disto, o overlay fica de fora nesta abertura.
pub unsafe fn let_steam_in() {
    let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return };
    let t = table(factory.as_raw());
    let t2 = factory.cast::<IDXGIFactory2>().ok().map(|f| table(f.as_raw()));
    unpatch(t, SLOT_CREATE, on_create as usize, &OLD_CREATE);
    if let Some(t2) = t2 {
        unpatch(t2, SLOT_CREATE_HWND, on_create_hwnd as usize, &OLD_CREATE_HWND);
    }
    drop(CreateDXGIFactory1::<IDXGIFactory1>());
    patch(t, SLOT_CREATE, on_create as usize, &OLD_CREATE);
    if let Some(t2) = t2 {
        patch(t2, SLOT_CREATE_HWND, on_create_hwnd as usize, &OLD_CREATE_HWND);
    }
}
