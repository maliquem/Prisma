//! HDR próprio: o jogo desenha num buffer nosso, e a cadeia de troca de verdade vira FP16 scRGB.
//! No `Present`, um passe de tela cheia passa o quadro do jogo pela nossa curva: no SDR, gama 2.2,
//! paper white, brilhos até o pico e saturação; no HDR do próprio jogo (HDR10 ou scRGB), o reajuste
//! pela calibração (paper white, saturação, pico com ombro suave e preto). A interface pode sair
//! separada da cena, com brilho próprio. D3D11 aqui, D3D12 em `hdr12.rs` (a cadeia, a calibração e a
//! separação são as mesmas), Vulkan em `hdrvk.rs`. Só janela: o resto passa direto.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
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

use crate::{hdr12, patch, scan, unpatch, Ptr, Shared};

const SLOT_CREATE: usize = 10;
const SLOT_CREATE_HWND: usize = 15;
const SLOT_GET_BUFFER: usize = 9;
const SLOT_GET_DESC: usize = 12;
const SLOT_RESIZE: usize = 13;
const SLOT_GET_DESC1: usize = 18;
const SLOT_CHECK_COLOR_SPACE: usize = 37;
const SLOT_SET_COLOR_SPACE: usize = 38;
const SLOT_RESIZE1: usize = 39;
const SLOT_CREATE_SRV: usize = 7;
const SLOT_CREATE_UAV: usize = 8;
const SLOT_CREATE_RTV: usize = 9;
const SLOT_DRAW_INDEXED: usize = 12;
const SLOT_DRAW: usize = 13;
const SLOT_DRAW_INDEXED_INSTANCED: usize = 20;
const SLOT_DRAW_INSTANCED: usize = 21;
const SLOT_OM_SET: usize = 33;
const SLOT_OM_SET_UAV: usize = 34;
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
type CheckColorSpace = unsafe extern "system" fn(Ptr, DXGI_COLOR_SPACE_TYPE, *mut u32) -> HRESULT;
type OmSet = unsafe extern "system" fn(Ptr, u32, *const Ptr, Ptr);
type OmSetUav = unsafe extern "system" fn(Ptr, u32, *const Ptr, Ptr, u32, u32, *const Ptr, *const u32);
type Draw = unsafe extern "system" fn(Ptr, u32, u32);
type DrawIndexed = unsafe extern "system" fn(Ptr, u32, u32, i32);
type DrawInstanced = unsafe extern "system" fn(Ptr, u32, u32, u32, u32);
type DrawIndexedInstanced = unsafe extern "system" fn(Ptr, u32, u32, u32, i32, u32);
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
static OLD_CHECK_COLOR_SPACE: AtomicUsize = AtomicUsize::new(0);
static OLD_OM_SET: AtomicUsize = AtomicUsize::new(0);
static OLD_OM_SET_UAV: AtomicUsize = AtomicUsize::new(0);
static OLD_DRAW: AtomicUsize = AtomicUsize::new(0);
static OLD_DRAW_INDEXED: AtomicUsize = AtomicUsize::new(0);
static OLD_DRAW_INSTANCED: AtomicUsize = AtomicUsize::new(0);
static OLD_DRAW_INDEXED_INSTANCED: AtomicUsize = AtomicUsize::new(0);
static PROXIES: Mutex<Vec<Proxy>> = Mutex::new(Vec::new());

/// Como saiu o último quadro, em `Shared::hdr_split`.
pub const SPLIT_NONE: u32 = 0; // nada desenhado depois de um passe de tela cheia: tudo junto, como antes
pub const SPLIT_ON: u32 = 1; // cena guardada e interface separada
pub const SPLIT_LATE: u32 = 2; // passe de tela cheia depois da interface: tudo junto
pub const SPLIT_OFF: u32 = 3; // brilho da interface em zero
/// O que o jogo entrega, em `Shared::hdr_native`.
pub const MODE_SDR: u32 = 0;
pub const MODE_HDR10: u32 = 1;
pub const MODE_SCRGB: u32 = 2;

/// Separação da interface no quadro da vez: nada visto, cena guardada, interface por cima, ou um passe
/// de tela cheia depois da interface (a separação falhou neste quadro).
pub(crate) const PHASE_NONE: u32 = 0;
pub(crate) const PHASE_SCENE: u32 = 1;
pub(crate) const PHASE_UI: u32 = 2;
pub(crate) const PHASE_LATE: u32 = 3;
/// shortcut: um proxy só separa a interface (o mais novo); jogo com duas janelas D3D11 fica sem na outra.
static CTX: AtomicUsize = AtomicUsize::new(0);
/// O buffer do jogo que separa, e o tamanho dele (largura << 32 | altura). Zero: nenhum.
static TEX: AtomicUsize = AtomicUsize::new(0);
static SIZE: AtomicU64 = AtomicU64::new(0);
static ON_BB: AtomicBool = AtomicBool::new(false);
static PHASE: AtomicU32 = AtomicU32::new(PHASE_NONE);
/// Pico que a tela do jogo informa ao Windows (bits de f32), lido quando a cadeia nasce.
static SCREEN_PEAK: AtomicU32 = AtomicU32::new(0);

thread_local! {
    /// Nosso código rodando nesta linha (criar, redimensionar, desenhar, o resto do Present): os
    /// ganchos deixam passar direto. Evita trava dupla e mostra a cadeia de verdade a overlays.
    pub(crate) static INSIDE: Cell<bool> = const { Cell::new(false) };
}

/// O mesmo passe nas três APIs: o D3D11 e o D3D12 compilam pelo fxc, o Vulkan pelo glslang (`hdr.spv`,
/// gerado no teste `spirv_matches_hlsl`). No D3D12 o `RS` é a assinatura raiz: constantes em b0, imagens em t0 e t1.
pub(crate) const SHADER: &str = r#"
#ifdef VULKAN
[[vk::binding(0)]] Texture2D<float4> tex;
[[vk::binding(1)]] Texture2D<float4> scene;
[[vk::push_constant]] cbuffer C { float paper, peak, gamma, sat, boost, black, ui; uint srgb, on, split, mode; float src_peak, src_black, src_paper; uint pad0, pad1; };
#ifdef SCAN
[[vk::binding(2)]] RWByteAddressBuffer hist;
#endif
#else
Texture2D<float4> tex : register(t0);
Texture2D<float4> scene : register(t1);
cbuffer C : register(b0) { float paper, peak, gamma, sat, boost, black, ui; uint srgb, on, split, mode; float src_peak, src_black, src_paper; uint pad0, pad1; };
#ifdef SCAN
RWByteAddressBuffer hist : register(u0);
#endif
#define RS "RootConstants(num32BitConstants=16, b0), DescriptorTable(SRV(t0, numDescriptors=2)), UAV(u0)"
#endif
static const float3 LUM = float3(0.2126, 0.7152, 0.0722);
static const float3x3 BT2020_TO_709 = { 1.6605, -0.5876, -0.0728, -0.1246, 1.1329, -0.0083, -0.0182, -0.1006, 1.1187 };
float4 vs(uint id : SV_VertexID) : SV_Position {
    float2 t = float2((id << 1) & 2, id & 2);
    return float4(t * float2(2, -2) + float2(-1, 1), 0, 1);
}
float3 encode_srgb(float3 l) { return l <= 0.0031308 ? l * 12.92 : 1.055 * pow(l, 1 / 2.4) - 0.055; }
float3 decode_srgb(float3 v) { return v <= 0.04045 ? v / 12.92 : pow((v + 0.055) / 1.055, 2.4); }
float3 pq_nits(float3 v) {
    float3 e = pow(saturate(v), 1 / 78.84375);
    return 10000 * pow(max(e - 0.8359375, 0) / (18.8515625 - 18.6875 * e), 1 / 0.1593017578125);
}
float pq_of(float n) {
    float y = pow(saturate(n / 10000), 0.1593017578125);
    return pow((0.8359375 + 18.8515625 * y) / (1 + 18.6875 * y), 78.84375);
}
float nits_of(float e) { return pq_nits(e.xxx).x; }
// BT.2390: o que o jogo entrega, de lb a lw nits, cabe no preto e no pico da tela. Abaixo do joelho fica
// igual; acima, uma curva de Hermite encosta no pico; o preto da tela entra pelo termo (1-e)^4.
float eetf(float n, float lb, float lw) {
    float pb = pq_of(lb), range = max(pq_of(lw) - pb, 1e-4);
    float e = max(pq_of(n) - pb, 0) / range;
    float lo = max(pq_of(black) - pb, 0) / range, hi = (pq_of(peak) - pb) / range;
    float ks = 1.5 * hi - 0.5;
    if (e > ks && ks < 1) {
        float t = saturate((e - ks) / (1 - ks)), t2 = t * t, t3 = t2 * t;
        e = (2 * t3 - 3 * t2 + 1) * ks + (t3 - 2 * t2 + t) * (1 - ks) + (-2 * t3 + 3 * t2) * hi;
    }
    e += lo * pow(saturate(1 - e), 4);
    return nits_of(e * range + pb);
}
// HDR do jogo: os nits que ele pediu, reajustados para a tela. Sem medida, o jogo supõe paper white de
// 203 nits e pico igual ao da tela.
float3 game_nits(float3 raw) { return mode == 1 ? mul(BT2020_TO_709, pq_nits(raw)) : raw * 80; }
float4 native(float3 raw, bool hud) {
    float3 n = game_nits(raw);
    if (!on) return float4(n / 80, 1);
    if (hud) return float4(min(n * ui / src_paper, peak) / 80, 1);
    float k = paper / src_paper, lw = src_peak * k;
    n *= k;
    float y = dot(n, LUM);
    n = y + (n - y) * sat;
    // Jogo que para abaixo do pico da tela: os brilhos sobem até ele, como no SDR.
    float m = max(max(n.r, n.g), n.b), t = saturate(m / lw), up = max(peak / lw - 1, 0) * boost;
    float o = eetf(m * (1 + up * t * t * t), src_black * k, lw * (1 + up));
    n = m > 1e-6 ? n * (o / m) : float3(o, o, o);
    return float4(min(n, peak) / 80, 1);
}
float4 ps(float4 p : SV_Position) : SV_Target {
    int3 at = int3(p.xy, 0);
    float3 raw = tex.Load(at).rgb;
    // Interface: o que mudou desde a cena guardada logo depois do passe de tela cheia.
    bool hud = split && any(abs(raw - scene.Load(at).rgb) > 0.004);
    if (mode) return native(raw, hud);
    // Buffer _SRGB: a leitura já decodificou pela curva por partes; volta ao valor que o jogo gravou.
    float3 c = saturate(srgb ? encode_srgb(saturate(raw)) : raw);
    // Desligado: o mesmo que o Windows faz com SDR, curva sRGB por partes no paper white.
    if (!on) return float4(decode_srgb(c) * paper / 80, 1);
    // Gama 2.2 pura: a curva por partes levanta os pretos.
    float3 l = pow(c, gamma);
    if (hud) return float4(min(l * ui, peak) / 80, 1);
    float y = dot(l, LUM);
    l = max(y + (l - y) * sat, 0);
    // Só os brilhos sobem: o ganho cresce com y^3 e chega ao pico no branco.
    float gain = 1 + (peak / paper - 1) * boost * y * y * y;
    float3 n = l * paper * gain;
    n = n * (1 - black / peak) + black;
    return float4(min(n, peak) / 80, 1);
}
#ifdef SCAN
// Analisar: histograma do quadro do jogo antes da nossa curva, em faixas de 1/8 de oitava de 1/256 a
// 15 mil nits. Cena (Y e maior canal) e interface (Y) à parte; a CPU soma e tira os percentis.
groupshared uint bins[528];
uint bin_of(float n) { return n <= 0 ? 0 : (uint)clamp((log2(n) + 8) * 8, 0, 175); }
[numthreads(16, 16, 1)]
void cs(uint3 id : SV_DispatchThreadID, uint gi : SV_GroupIndex) {
    for (uint i = gi; i < 528; i += 256) bins[i] = 0;
    GroupMemoryBarrierWithGroupSync();
    uint w, h;
    tex.GetDimensions(w, h);
    uint2 at = id.xy * 2;
    if (at.x < w && at.y < h) {
        float3 raw = tex.Load(int3(at, 0)).rgb;
        bool hud = split && any(abs(raw - scene.Load(int3(at, 0)).rgb) > 0.004);
        float3 c = saturate(srgb ? encode_srgb(saturate(raw)) : raw);
        float3 n = max(mode ? game_nits(raw) : pow(c, 2.2) * 100, 0);
        float y = dot(n, LUM);
        uint dummy;
        if (hud) {
            InterlockedAdd(bins[352 + bin_of(y)], 1, dummy);
        } else {
            InterlockedAdd(bins[bin_of(y)], 1, dummy);
            InterlockedAdd(bins[176 + bin_of(max(max(n.r, n.g), n.b))], 1, dummy);
        }
    }
    GroupMemoryBarrierWithGroupSync();
    for (uint j = gi; j < 528; j += 256) {
        if (bins[j]) hist.InterlockedAdd(j * 4, bins[j]);
    }
}
#endif
"#;

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Params {
    paper: f32,
    peak: f32,
    gamma: f32,
    sat: f32,
    boost: f32,
    black: f32,
    ui: f32,
    srgb: u32,
    on: u32,
    split: u32,
    mode: u32,
    /// O que o jogo entrega no HDR dele: pico, preto e paper white em nits.
    src_peak: f32,
    src_black: f32,
    src_paper: f32,
    pad: [u32; 2],
}

/// O que o jogo pediu: o buffer dele segue isso, não a cadeia de verdade.
#[derive(Clone, Copy)]
pub(crate) struct Want {
    pub width: u32,
    pub height: u32,
    pub format: DXGI_FORMAT,
    samples: u32,
    pub usage: u32,
    /// O espaço de cor que o jogo deu à cadeia: G2084 no buffer de 10 bits é HDR10.
    space: DXGI_COLOR_SPACE_TYPE,
}

impl Want {
    fn new(width: u32, height: u32, format: DXGI_FORMAT, samples: u32, usage: u32) -> Self {
        Self { width, height, format, samples, usage, space: DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709 }
    }

    pub fn mode(&self) -> u32 {
        match (self.format, self.space) {
            (DXGI_FORMAT_R16G16B16A16_FLOAT, _) => MODE_SCRGB,
            (DXGI_FORMAT_R10G10B10A2_UNORM, DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020) => MODE_HDR10,
            _ => MODE_SDR,
        }
    }
}

/// Os espaços de cor que a cadeia aceita do jogo: o que ele aceitaria numa cadeia do formato que pediu.
fn takes(format: DXGI_FORMAT, space: DXGI_COLOR_SPACE_TYPE) -> bool {
    match space {
        DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709 => format != DXGI_FORMAT_R16G16B16A16_FLOAT,
        DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020 => format == DXGI_FORMAT_R10G10B10A2_UNORM,
        DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709 => format == DXGI_FORMAT_R16G16B16A16_FLOAT,
        _ => false,
    }
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
    /// A cena guardada logo depois do passe de tela cheia: o que difere dela é interface. Sem MSAA só.
    scene: Option<ID3D11Texture2D>,
    scene_srv: Option<ID3D11ShaderResourceView>,
    /// O buffer 0 da cadeia de verdade: no modelo flip do D3D11 ele sempre aponta o buffer da vez.
    rtv: Option<ID3D11RenderTargetView>,
    /// Sem consultas na placa, o passe roda sem medida.
    timer: Option<Timer>,
    /// Sem compute (placa antiga), o Analisar não mede neste jogo.
    scan: Option<Scan>,
}

/// O histograma do Analisar: o `cs`, o buffer que ele soma e três cópias lidas dois quadros depois.
struct Scan {
    cs: ID3D11ComputeShader,
    buf: ID3D11Buffer,
    uav: ID3D11UnorderedAccessView,
    staging: Vec<ID3D11Buffer>,
    n: usize,
    ring: scan::Ring,
}

impl Scan {
    unsafe fn new(dev: &ID3D11Device) -> Result<Self> {
        let csb = compile(s!("cs_5_0"), s!("cs"))?;
        let mut cs = None;
        dev.CreateComputeShader(bytes(&csb), None, Some(&mut cs))?;
        let bd = D3D11_BUFFER_DESC {
            ByteWidth: scan::BYTES as u32,
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_UNORDERED_ACCESS.0 as u32,
            MiscFlags: D3D11_RESOURCE_MISC_BUFFER_ALLOW_RAW_VIEWS.0 as u32,
            ..Default::default()
        };
        let mut buf = None;
        dev.CreateBuffer(&bd, None, Some(&mut buf))?;
        let buf = buf.ok_or(Error::from(E_FAIL))?;
        let mut ud = D3D11_UNORDERED_ACCESS_VIEW_DESC { Format: DXGI_FORMAT_R32_TYPELESS, ViewDimension: D3D11_UAV_DIMENSION_BUFFER, ..Default::default() };
        ud.Anonymous.Buffer = D3D11_BUFFER_UAV { FirstElement: 0, NumElements: scan::WORDS as u32, Flags: D3D11_BUFFER_UAV_FLAG_RAW.0 as u32 };
        let mut uav = None;
        dev.CreateUnorderedAccessView(&buf, Some(&ud), Some(&mut uav))?;
        let sd = D3D11_BUFFER_DESC { ByteWidth: scan::BYTES as u32, Usage: D3D11_USAGE_STAGING, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..Default::default() };
        let staging = (0..3)
            .map(|_| {
                let mut b = None;
                dev.CreateBuffer(&sd, None, Some(&mut b))?;
                b.ok_or(Error::from(E_FAIL))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { cs: cs.ok_or(Error::from(E_FAIL))?, buf, uav: uav.ok_or(Error::from(E_FAIL))?, staging, n: 0, ring: scan::Ring::new(3) })
    }

    /// Soma o quadro no buffer, copia para a vaga da vez e lê a de dois quadros atrás, se a placa já acabou.
    unsafe fn run(&mut self, ctx: &ID3D11DeviceContext1, s: &Shared, w: Want, views: [Option<ID3D11ShaderResourceView>; 2], cb: &ID3D11Buffer) {
        if !self.ring.start(s) {
            return;
        }
        ctx.CSSetShader(&self.cs, None);
        ctx.CSSetShaderResources(0, Some(&views));
        ctx.CSSetConstantBuffers(0, Some(&[Some(cb.clone())]));
        ctx.CSSetUnorderedAccessViews(0, 1, Some(&Some(self.uav.clone())), None);
        ctx.Dispatch(w.width.div_ceil(scan::TILE), w.height.div_ceil(scan::TILE), 1);
        ctx.CSSetUnorderedAccessViews(0, 1, Some(&None), None);
        ctx.CSSetShaderResources(0, Some(&[None, None]));
        let (put, get) = (self.n % 3, (self.n + 1) % 3);
        ctx.CopyResource(&self.staging[put], &self.buf);
        self.ring.wrote(put);
        self.n += 1;
        if !self.ring.ready(get) {
            return;
        }
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        if ctx.Map(&self.staging[get], 0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32, Some(&mut m)).is_ok() {
            let cur = std::slice::from_raw_parts(m.pData as *const u32, scan::WORDS).to_vec();
            ctx.Unmap(&self.staging[get], 0);
            self.ring.feed(s, w.mode(), &cur);
        }
    }
}

/// Tempo de GPU do passe, como `prisma_shader::GpuTimer` (o gancho não puxa o librashader): carimbos lidos
/// alguns quadros depois, sem esperar a GPU.
struct Timer {
    slots: Vec<[ID3D11Query; 3]>,
    n: usize,
    ms: f64,
}

impl Timer {
    unsafe fn new(dev: &ID3D11Device) -> Option<Self> {
        let make = |q| {
            let mut out = None;
            dev.CreateQuery(&D3D11_QUERY_DESC { Query: q, MiscFlags: 0 }, Some(&mut out)).ok()?;
            out
        };
        let slots = (0..4).map(|_| Some([make(D3D11_QUERY_TIMESTAMP_DISJOINT)?, make(D3D11_QUERY_TIMESTAMP)?, make(D3D11_QUERY_TIMESTAMP)?])).collect::<Option<Vec<_>>>()?;
        Some(Self { slots, n: 0, ms: 0.0 })
    }

    /// Mede `f` na vaga da vez, depois de ler o que ela mediu da última vez; devolve a média em ms (0 sem medida ainda).
    unsafe fn time(&mut self, ctx: &ID3D11DeviceContext, f: impl FnOnce()) -> f64 {
        let i = self.n % self.slots.len();
        if self.n >= self.slots.len() {
            if let Some(ms) = self.read(ctx, i) {
                self.ms = if self.ms <= 0.0 { ms } else { self.ms * 0.9 + ms * 0.1 };
            }
        }
        let [dis, a, b] = &self.slots[i];
        ctx.Begin(dis);
        ctx.End(a);
        f();
        ctx.End(b);
        ctx.End(dis);
        self.n += 1;
        self.ms
    }

    // O GetData do windows-rs junta S_FALSE (ainda não pronto) com S_OK: a vtable devolve o HRESULT cru.
    unsafe fn get<T: Default>(ctx: &ID3D11DeviceContext, q: &ID3D11Query) -> Option<T> {
        let mut v = T::default();
        let hr = (Interface::vtable(ctx).GetData)(Interface::as_raw(ctx), Interface::as_raw(q), &mut v as *mut T as *mut _, std::mem::size_of::<T>() as u32, D3D11_ASYNC_GETDATA_DONOTFLUSH.0 as u32);
        (hr == S_OK).then_some(v)
    }

    unsafe fn read(&self, ctx: &ID3D11DeviceContext, i: usize) -> Option<f64> {
        let [dis, a, b] = &self.slots[i];
        let d: D3D11_QUERY_DATA_TIMESTAMP_DISJOINT = Self::get(ctx, dis)?;
        let (t0, t1): (u64, u64) = (Self::get(ctx, a)?, Self::get(ctx, b)?);
        (!d.Disjoint.as_bool() && d.Frequency > 0).then(|| t1.saturating_sub(t0) as f64 * 1000.0 / d.Frequency as f64)
    }
}

// Os objetos COM só são usados com a trava pega.
unsafe impl Send for Proxy {}

fn proxies() -> MutexGuard<'static, Vec<Proxy>> {
    PROXIES.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn shared() -> Option<&'static Shared> {
    let at = crate::SHARED.load(Relaxed);
    (at != 0).then(|| unsafe { &*(at as *const Shared) })
}

/// A pessoa abriu o jogo pelo Prisma com o HDR pedido.
pub fn wanted() -> bool {
    shared().is_some_and(|s| s.hdr_proxy.load(Relaxed) != 0)
}

pub(crate) fn note(hr: HRESULT) {
    if let Some(s) = shared() {
        s.hdr_error.store(hr.0 as u32, Relaxed);
    }
}

/// O pedido do jogo para esta cadeia, quando ela é nossa e o chamador não é o nosso código.
fn want_of(chain: Ptr) -> Option<Want> {
    if INSIDE.get() {
        return None;
    }
    proxies().iter().find(|p| p.chain == chain as usize).map(|p| p.want).or_else(|| hdr12::proxies().iter().find(|p| p.chain == chain as usize).map(|p| p.want))
}

/// Roda `f` marcado como nosso: o resto do `Present` (e overlays pendurados nele) vê a cadeia de verdade.
pub fn passthrough<T>(f: impl FnOnce() -> T) -> T {
    let was = INSIDE.replace(true);
    let r = f();
    INSIDE.set(was);
    r
}

pub(crate) unsafe fn compile(target: PCSTR, entry: PCSTR) -> Result<ID3DBlob> {
    let mut blob = None;
    // O buffer do histograma só existe no `cs`: o passe de sempre não o declara.
    let scan = [D3D_SHADER_MACRO { Name: s!("SCAN"), Definition: s!("1") }, D3D_SHADER_MACRO::default()];
    let defines = (entry.as_bytes() == b"cs").then_some(scan.as_ptr());
    D3DCompile(SHADER.as_ptr() as *const _, SHADER.len(), None, defines, None, entry, target, D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &mut blob, None)?;
    blob.ok_or_else(|| E_FAIL.into())
}

pub(crate) unsafe fn bytes(b: &ID3DBlob) -> &[u8] {
    std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize())
}

pub(crate) fn is_srgb(f: DXGI_FORMAT) -> bool {
    matches!(f, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8X8_UNORM_SRGB)
}

/// O buffer de uma cadeia aceita vista _SRGB sobre formato UNORM (o Unity faz isso); textura comum só
/// aceita se nascer sem tipo. Vista sem descrição ganha o formato pedido em `view`.
pub(crate) fn typeless(f: DXGI_FORMAT) -> DXGI_FORMAT {
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
        let imm = device.GetImmediateContext()?;
        CTX.store(imm.as_raw() as usize, Relaxed);
        hook_context(imm.as_raw());
        let ctx: ID3D11DeviceContext1 = imm.cast()?;
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
        let timer = Timer::new(&device);
        let scan = Scan::new(&device).ok();
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
            scene: None,
            scene_srv: None,
            rtv: None,
            timer,
            scan,
        };
        me.rebuild(chain)?;
        Ok(me)
    }

    /// Solta tudo que segura buffers: o `ResizeBuffers` falha com algum preso.
    fn drop_buffers(&mut self) {
        if let Some(t) = &self.tex {
            let _ = TEX.compare_exchange(t.as_raw() as usize, 0, Relaxed, Relaxed);
        }
        (self.tex, self.resolved, self.srv, self.rtv, self.scene, self.scene_srv) = (None, None, None, None, None, None);
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
        if w.samples <= 1 {
            // Sem a cópia da cena o jogo segue com tudo junto: falhar aqui não derruba o HDR.
            let sd = D3D11_TEXTURE2D_DESC { BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, ..td };
            let (mut scene, mut ssrv) = (None, None);
            if self.device.CreateTexture2D(&sd, None, Some(&mut scene)).is_ok() {
                if let Some(s) = &scene {
                    if self.device.CreateShaderResourceView(s, Some(&srv_desc(w.format, false)), Some(&mut ssrv)).is_ok() {
                        (self.scene, self.scene_srv) = (scene, ssrv);
                        SIZE.store((w.width as u64) << 32 | w.height as u64, Relaxed);
                        TEX.store(tex.as_raw() as usize, Relaxed);
                    }
                }
            }
        }
        (self.srv, self.rtv, self.tex) = (srv, rtv, Some(tex));
        if let Ok(c3) = real.cast::<IDXGISwapChain3>() {
            c3.SetColorSpace1(DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709)?;
        }
        Ok(())
    }

    unsafe fn draw(&mut self, p: Params, s: &Shared) {
        let (Some(tex), Some(srv), Some(rtv)) = (&self.tex, &self.srv, &self.rtv) else { return };
        if let Some(r) = &self.resolved {
            self.ctx.ResolveSubresource(r, 0, tex, 0, self.want.format);
        }
        let mut prev = None;
        self.ctx.SwapDeviceContextState(&self.state, Some(&mut prev));
        self.ctx.UpdateSubresource(&self.cb, 0, None, &p as *const _ as *const c_void, 0, 0);
        if let Some(sc) = &mut self.scan {
            sc.run(&self.ctx, s, self.want, [Some(srv.clone()), self.scene_srv.clone()], &self.cb);
        }
        self.ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        self.ctx.VSSetShader(&self.vs, None);
        self.ctx.PSSetShader(&self.ps, None);
        self.ctx.PSSetShaderResources(0, Some(&[Some(srv.clone()), self.scene_srv.clone()]));
        self.ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
        self.ctx.OMSetRenderTargets(Some(&[Some(rtv.clone())]), None);
        let vp = D3D11_VIEWPORT { Width: self.want.width as f32, Height: self.want.height as f32, MaxDepth: 1.0, ..Default::default() };
        self.ctx.RSSetViewports(Some(&[vp]));
        self.ctx.Draw(3, 0);
        // Solta o buffer do jogo e o de verdade antes de devolver o estado: nada nosso fica preso.
        self.ctx.PSSetShaderResources(0, Some(&[None, None]));
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

fn ui_of(s: &Shared) -> f32 {
    // Abaixo de 1 nit a aba mostra "Junto": a interface fica com a cena.
    Some(f32::from_bits(s.hdr_ui.load(Relaxed))).filter(|v| *v >= 1.0).unwrap_or(0.0)
}

pub(crate) fn params(s: &Shared, srgb: bool, mode: u32, split: bool) -> Params {
    let f = |a: &AtomicU32| f32::from_bits(a.load(Relaxed));
    let peak = f(&s.hdr_peak).max(80.0);
    // Pico do jogo sem medida: o que a tela diz ao Windows, que é o que o jogo lê; sem isso, 1000.
    let measured = f(&s.hdr_src_peak);
    let src_peak = [measured, f32::from_bits(SCREEN_PEAK.load(Relaxed))].into_iter().find(|v| *v >= 80.0).unwrap_or(1000.0).min(10_000.0);
    // Sem medida o jogo já ocupa a tela até o pico: subir os brilhos só com o pico dele medido.
    let boost = if mode != MODE_SDR && measured < 80.0 { 0.0 } else { f(&s.hdr_boost).clamp(0.0, 1.0) };
    Params {
        paper: f(&s.hdr_paper).clamp(80.0, peak),
        peak,
        gamma: f(&s.hdr_gamma).clamp(1.0, 3.0),
        sat: f(&s.hdr_sat).clamp(0.0, 2.0),
        boost,
        black: f(&s.hdr_black).clamp(0.0, 1.0),
        ui: ui_of(s).min(peak),
        srgb: srgb as u32,
        on: (s.hdr_on.load(Relaxed) != 0) as u32,
        split: split as u32,
        mode,
        src_peak,
        src_black: f(&s.hdr_src_black).clamp(0.0, src_peak / 2.0),
        src_paper: Some(f(&s.hdr_src_paper)).filter(|v| *v >= 40.0).unwrap_or(203.0).min(src_peak),
        pad: [0; 2],
    }
}

/// Como sai o quadro, pela fase que a separação viu nele.
pub(crate) fn split_of(s: &Shared, phase: u32) -> u32 {
    match phase {
        _ if ui_of(s) <= 0.0 => SPLIT_OFF,
        PHASE_UI => SPLIT_ON,
        PHASE_LATE => SPLIT_LATE,
        _ => SPLIT_NONE,
    }
}

/// O que a aba e o menu do jogo mostram do último quadro.
pub(crate) fn report(s: &Shared, ms: f64, split: u32, mode: u32) {
    s.hdr_gpu_us.store((ms * 1000.0) as u32, Relaxed);
    s.hdr_frames.fetch_add(1, Relaxed);
    s.hdr_split.store(split, Relaxed);
    s.hdr_native.store(mode, Relaxed);
    if split == SPLIT_ON {
        s.hdr_split_frames.fetch_add(1, Relaxed);
    }
}

/// Chamado antes de cada apresentação: desenha o quadro do jogo na cadeia FP16.
pub unsafe fn present(chain: Ptr, flags: u32) {
    let Some(s) = shared() else { return };
    if flags & crate::PRESENT_TEST != 0 || INSIDE.get() {
        return;
    }
    if hdr12::present(s, chain) {
        return;
    }
    let mut list = proxies();
    let Some(p) = list.iter_mut().find(|p| p.chain == chain as usize) else { return };
    let mine = p.tex.as_ref().is_some_and(|t| t.as_raw() as usize == TEX.load(Relaxed));
    let phase = if mine { PHASE.swap(PHASE_NONE, Relaxed) } else { PHASE_NONE };
    let split = split_of(s, phase);
    let pr = params(s, is_srgb(p.want.format), p.want.mode(), split == SPLIT_ON);
    // A vaga sai do proxy enquanto mede: `draw` empresta o proxy inteiro.
    let mut timer = p.timer.take();
    let ctx = p.ctx.clone();
    let ms = passthrough(|| match timer.as_mut() {
        Some(t) => t.time(&ctx, || p.draw(pr, s)),
        None => {
            p.draw(pr, s);
            0.0
        }
    });
    p.timer = timer;
    report(s, ms, split, p.want.mode());
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
pub(crate) unsafe fn screen_hdr(hwnd: HWND) -> bool {
    let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() else { return false };
    let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
    // Todas as placas: em notebook a tela costuma estar na integrada e o jogo na dedicada.
    for a in (0..).map_while(|i| factory.EnumAdapters1(i).ok()) {
        for o in (0..).map_while(|i| a.EnumOutputs(i).ok()) {
            if let Ok(d) = o.cast::<IDXGIOutput6>().and_then(|o| o.GetDesc1()) {
                if d.Monitor == monitor {
                    SCREEN_PEAK.store(d.MaxLuminance.to_bits(), Relaxed);
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
        let keep = |c: usize, h: usize| c != chain && h != hwnd.0 as usize && IsWindow(Some(HWND(h as *mut c_void))).as_bool();
        let mut list = proxies();
        list.retain(|p| keep(p.chain, p.hwnd));
        let mut list12 = hdr12::proxies();
        list12.retain(|p| keep(p.chain, p.hwnd));
        // Sem proxy nenhum o passe não roda mais: o medidor deixa de mostrar o custo dele.
        if let Some(s) = shared().filter(|_| list.is_empty() && list12.is_empty()) {
            s.hdr_gpu_us.store(0, Relaxed);
        }
    }
    hr
}

/// Cria a cadeia FP16 e o proxy; se algo falhar, cria a cadeia que o jogo pediu, sem HDR.
unsafe fn proxied(device: Ptr, hwnd: HWND, want: Want, out: *mut Ptr, real: impl Fn() -> HRESULT, plain: impl Fn() -> HRESULT) -> HRESULT {
    let fit = !INSIDE.get() && !out.is_null() && wanted() && screen_hdr(hwnd);
    // No D3D11 o jogo dá o dispositivo; no D3D12, a fila de comandos.
    let (dev, queue) = if fit { (d3d11(device), hdr12::queue(device)) } else { (None, None) };
    if dev.is_none() && queue.is_none() {
        return created(plain(), out, hwnd);
    }
    INSIDE.set(true);
    let mut hr = real();
    if hr.is_ok() {
        hook_chain(*out);
        // Antes de entrar na lista: tira os proxies velhos deste endereço e desta janela.
        let _ = created(hr, out, hwnd);
        let made = match (dev, queue) {
            (Some(dev), _) => {
                hook_device(&dev);
                Proxy::new(*out, hwnd, dev, want).map(|p| proxies().push(p))
            }
            (_, Some(q)) => hdr12::Proxy::new(*out, hwnd, q, want).map(|p| hdr12::proxies().push(p)),
            _ => Err(E_FAIL.into()),
        };
        match made {
            Ok(()) => {
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
    let want = Want::new(d.BufferDesc.Width, d.BufferDesc.Height, d.BufferDesc.Format, d.SampleDesc.Count, d.BufferUsage.0);
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
    let want = Want::new(d.Width, d.Height, d.Format, d.SampleDesc.Count, d.BufferUsage.0);
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
        drop(list);
        if let Some(p) = hdr12::proxies().iter().find(|p| p.chain == this as usize) {
            return p.buffer(index, riid, out);
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

/// O espaço de cor da cadeia de verdade é nosso (scRGB); o do jogo fica de conta e diz como ler o
/// buffer dele: HDR10 no de 10 bits, scRGB no FP16, SDR no resto.
unsafe extern "system" fn on_set_color_space(this: Ptr, space: DXGI_COLOR_SPACE_TYPE) -> HRESULT {
    if !INSIDE.get() {
        let set = |w: &mut Want| if takes(w.format, space) { w.space = space; S_OK } else { DXGI_ERROR_UNSUPPORTED };
        if let Some(p) = proxies().iter_mut().find(|p| p.chain == this as usize) {
            return set(&mut p.want);
        }
        if let Some(p) = hdr12::proxies().iter_mut().find(|p| p.chain == this as usize) {
            return set(&mut p.want);
        }
    }
    let old: SetColorSpace = std::mem::transmute(OLD_SET_COLOR_SPACE.load(Relaxed));
    old(this, space)
}

/// A cadeia de verdade é FP16 e só aceitaria scRGB: o jogo ouve o que valeria no formato dele.
unsafe extern "system" fn on_check_color_space(this: Ptr, space: DXGI_COLOR_SPACE_TYPE, out: *mut u32) -> HRESULT {
    if let Some(w) = want_of(this).filter(|_| !out.is_null()) {
        *out = if takes(w.format, space) { DXGI_SWAP_CHAIN_COLOR_SPACE_SUPPORT_FLAG_PRESENT.0 as u32 } else { 0 };
        return S_OK;
    }
    let old: CheckColorSpace = std::mem::transmute(OLD_CHECK_COLOR_SPACE.load(Relaxed));
    old(this, space, out)
}

/// O alvo 0 é o buffer do jogo que separa a interface. Só no contexto imediato e com a separação ligada.
unsafe fn track(this: Ptr, count: u32, rtvs: *const Ptr) {
    if INSIDE.get() || this as usize != CTX.load(Relaxed) {
        return;
    }
    let tex = TEX.load(Relaxed);
    let on = tex != 0
        && shared().is_some_and(|s| ui_of(s) > 0.0)
        && count > 0
        && !rtvs.is_null()
        && ID3D11View::from_raw_borrowed(&*rtvs).and_then(|v| v.GetResource().ok()).is_some_and(|r| r.as_raw() as usize == tex);
    ON_BB.store(on, Relaxed);
    // O D3D11 troca as versões do `Draw` conforme o alvo preso: os ganchos voltam por cima.
    if on {
        hook_context(this);
    }
}

/// Um triângulo ou um quadrado na tela toda lendo uma imagem do tamanho da tela: a cena indo para o
/// buffer, depois do pós-processamento.
unsafe fn fullscreen(ctx: &ID3D11DeviceContext) -> bool {
    let size = SIZE.load(Relaxed);
    let (w, h) = ((size >> 32) as f32, (size & 0xffff_ffff) as f32);
    let (mut n, mut vp) = (1u32, D3D11_VIEWPORT::default());
    ctx.RSGetViewports(&mut n, Some(&mut vp));
    if n == 0 || h == 0.0 || vp.TopLeftX > 0.5 || vp.TopLeftY > 0.5 || vp.Width < w - 1.0 || vp.Height < h - 1.0 {
        return false;
    }

    let mut srvs: [Option<ID3D11ShaderResourceView>; 4] = Default::default();
    ctx.PSGetShaderResources(0, Some(&mut srvs));
    srvs.iter().flatten().any(|v| {
        let Ok(t) = v.GetResource().and_then(|r| r.cast::<ID3D11Texture2D>()) else { return false };
        let mut d = D3D11_TEXTURE2D_DESC::default();
        t.GetDesc(&mut d);
        let (tw, th) = (d.Width as f32, d.Height as f32);

        th > 0.0 && tw >= w / 2.0 && (tw / th - w / h).abs() < 0.02 * w / h
    })
}

/// Guarda a cena: o que vier depois no buffer é interface.
unsafe fn snap(ctx: &ID3D11DeviceContext) {
    let tex = TEX.load(Relaxed);
    let list = proxies();
    if let Some(p) = list.iter().find(|p| p.tex.as_ref().is_some_and(|t| t.as_raw() as usize == tex)) {
        if let (Some(t), Some(s)) = (&p.tex, &p.scene) {
            ctx.CopyResource(s, t);
        }
    }
}

/// Depois de cada desenho no buffer do jogo: o passe de tela cheia guarda a cena, o resto é interface.
/// Um passe de tela cheia depois da interface desfaz a separação neste quadro.
unsafe fn drawn(this: Ptr, count: u32, instances: u32) {
    if INSIDE.get() || !ON_BB.load(Relaxed) || this as usize != CTX.load(Relaxed) {
        return;
    }
    let phase = PHASE.load(Relaxed);
    if phase == PHASE_LATE {
        return;
    }
    let Some(ctx) = ID3D11DeviceContext::from_raw_borrowed(&this) else { return };
    let full = count <= 6 && instances <= 1 && passthrough(|| fullscreen(ctx));
    let (next, take) = step(phase, full);
    if take {
        passthrough(|| snap(ctx));
    }
    PHASE.store(next, Relaxed);
}

/// A fase depois de um desenho no buffer do jogo, e se a cena deve ser guardada agora. Vale nas três APIs.
pub(crate) fn step(phase: u32, full: bool) -> (u32, bool) {
    match (full, phase) {
        (_, PHASE_LATE) => (PHASE_LATE, false),
        (true, PHASE_UI) => (PHASE_LATE, false),
        (true, _) => (PHASE_SCENE, true),
        (false, PHASE_SCENE) => (PHASE_UI, false),
        (_, p) => (p, false),
    }
}

unsafe extern "system" fn on_om_set(this: Ptr, count: u32, rtvs: *const Ptr, dsv: Ptr) {
    let old: OmSet = std::mem::transmute(OLD_OM_SET.load(Relaxed));
    old(this, count, rtvs, dsv);
    track(this, count, rtvs);
}

unsafe extern "system" fn on_om_set_uav(this: Ptr, count: u32, rtvs: *const Ptr, dsv: Ptr, start: u32, uavs: u32, views: *const Ptr, counts: *const u32) {
    let old: OmSetUav = std::mem::transmute(OLD_OM_SET_UAV.load(Relaxed));
    old(this, count, rtvs, dsv, start, uavs, views, counts);
    if count != D3D11_KEEP_RENDER_TARGETS_AND_DEPTH_STENCIL {
        track(this, count, rtvs);
    }
}

unsafe extern "system" fn on_draw(this: Ptr, count: u32, start: u32) {
    let old: Draw = std::mem::transmute(OLD_DRAW.load(Relaxed));
    old(this, count, start);
    drawn(this, count, 1);
}

unsafe extern "system" fn on_draw_indexed(this: Ptr, count: u32, start: u32, base: i32) {
    let old: DrawIndexed = std::mem::transmute(OLD_DRAW_INDEXED.load(Relaxed));
    old(this, count, start, base);
    drawn(this, count, 1);
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

/// `ResizeBuffers` e `ResizeBuffers1`: a cadeia de verdade segue FP16 e o buffer do jogo é refeito.
unsafe fn resize(this: Ptr, count: u32, format: DXGI_FORMAT, flags: u32, call: impl FnOnce(u32, DXGI_FORMAT, u32) -> HRESULT) -> HRESULT {
    let mut list = proxies();
    let Some(p) = list.iter_mut().find(|p| !INSIDE.get() && p.chain == this as usize) else {
        drop(list);
        if INSIDE.get() || !hdr12::has(this) {
            return call(count, format, flags);
        }
        return hdr12::resize(this, count, format, flags & !GDI_COMPATIBLE, call);
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

/// Máscara e filas são do D3D12 e passam como vieram: o proxy guarda os buffers do jogo um por índice.
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
        patch(t3, SLOT_CHECK_COLOR_SPACE, on_check_color_space as usize, &OLD_CHECK_COLOR_SPACE);
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

/// Depois de cada `Present`: ele devolve ao contexto as versões do D3D11.
pub unsafe fn rehook() {
    let ctx = CTX.load(Relaxed);
    if ctx != 0 {
        hook_context(ctx as Ptr);
    }
}

/// A tabela do contexto imediato fica dentro do objeto, e o D3D11 reescreve entradas dela no `Present`,
/// no `SwapDeviceContextState` e ao prender o alvo: os ganchos voltam por cima nesses pontos, guardando
/// a versão que o D3D11 pôs por último.
/// shortcut: a interface só é vista pelos `Draw*` comuns do contexto imediato; contexto adiado,
/// `DrawAuto` e os indiretos não contam. Ligar se aparecer jogo que desenhe a interface assim.
unsafe fn hook_context(ctx: Ptr) {
    let t = table(ctx);
    patch(t, SLOT_DRAW_INDEXED, on_draw_indexed as usize, &OLD_DRAW_INDEXED);
    patch(t, SLOT_DRAW, on_draw as usize, &OLD_DRAW);
    patch(t, SLOT_DRAW_INDEXED_INSTANCED, on_draw_indexed_instanced as usize, &OLD_DRAW_INDEXED_INSTANCED);
    patch(t, SLOT_DRAW_INSTANCED, on_draw_instanced as usize, &OLD_DRAW_INSTANCED);
    patch(t, SLOT_OM_SET, on_om_set as usize, &OLD_OM_SET);
    patch(t, SLOT_OM_SET_UAV, on_om_set_uav as usize, &OLD_OM_SET_UAV);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_compiles_and_params_match() {
        unsafe {
            compile(s!("vs_5_0"), s!("vs")).unwrap();
            compile(s!("ps_5_0"), s!("ps")).unwrap();
            compile(s!("cs_5_0"), s!("cs")).unwrap();
        }
        // cbuffer C: 7 floats e 5 uints.
        assert_eq!(std::mem::size_of::<Params>(), 64);
    }

    #[test]
    fn split_phases() {
        // Cena, interface por cima; um passe de tela cheia depois da interface desfaz a separação.
        assert_eq!(step(PHASE_NONE, true), (PHASE_SCENE, true));
        assert_eq!(step(PHASE_SCENE, false), (PHASE_UI, false));
        assert_eq!(step(PHASE_UI, false), (PHASE_UI, false));
        assert_eq!(step(PHASE_UI, true), (PHASE_LATE, false));
        assert_eq!(step(PHASE_LATE, true), (PHASE_LATE, false));
        assert_eq!(step(PHASE_NONE, false), (PHASE_NONE, false));
        // Dois passes de tela cheia seguidos: a cena é o último.
        assert_eq!(step(PHASE_SCENE, true), (PHASE_SCENE, true));
    }

    #[test]
    fn color_space_by_format() {
        let mut w = Want::new(1920, 1080, DXGI_FORMAT_R10G10B10A2_UNORM, 1, 0);
        assert_eq!(w.mode(), MODE_SDR);
        assert!(takes(w.format, DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020));
        w.space = DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020;
        assert_eq!(w.mode(), MODE_HDR10);
        assert!(!takes(DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_COLOR_SPACE_RGB_FULL_G2084_NONE_P2020));
        assert!(takes(DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709));
        let f = Want::new(1920, 1080, DXGI_FORMAT_R16G16B16A16_FLOAT, 1, 0);
        assert_eq!(f.mode(), MODE_SCRGB);
        assert!(takes(f.format, DXGI_COLOR_SPACE_RGB_FULL_G10_NONE_P709));
    }
}
