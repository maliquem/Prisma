//! PyroWave (de Hans-Kristian Arntzen, licença MIT): codec de wavelet só intra, em Vulkan compute.
//! Feito para a rede local: perto de 200 Mbit/s em 1080p a 60 quadros, poucos milissegundos de
//! atraso e nenhum quadro dependendo de outro (um pacote perdido só borra um pedaço daquele quadro).
//!
//! O Vulkan entra só com este codec: a `libpyrowave-shared-1.dll` é carregada quando ele é escolhido
//! e cria o próprio Vulkan na placa do D3D11 (pelo LUID). As imagens passam sem cópia: texturas D3D11
//! compartilhadas (NT handle) viram imagens Vulkan, e uma fence D3D11 compartilhada vira um semáforo
//! timeline que ordena os dois lados na GPU, sem a CPU esperar.
//!
//! No host, a captura (BGRA, ou PQ de 10 bits no HDR10) vai direto para o PyroWave, que reduz e
//! converte para YCbCr 4:2:0 ou 4:4:4 (BT.709, ou BT.2020 no HDR10). Em quem assiste, o PyroWave
//! escreve os três planos em texturas D3D11 (de 16 bits no HDR10), e a janela continua sendo a de
//! sempre (D3D11, com o mesmo menu e o mesmo medidor): um shader converte para RGB ao desenhar.

use std::ffi::c_void;
use std::sync::OnceLock;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::capture::hdr::{compile, VS};
use crate::capture::live_enc::VideoFmt;
use crate::capture::D3d;

/// A biblioteca do PyroWave, ao lado do `prisma.exe` (ou onde `PRISMA_PYROWAVE` disser).
pub const DLL: &str = "libpyrowave-shared-1.dll";

/// Bits por pixel por quadro do automático: 1080p a 60 quadros dá 200 Mbit/s, o mínimo que o autor
/// recomenda para a imagem ficar limpa.
const BITS_PER_PIXEL: f64 = 200e6 / (1920.0 * 1080.0 * 60.0);
/// Teto do automático: o que uma rede de 1 Gbit/s aguenta com folga para o resto.
const MAX_BITS: f64 = 700e6;

/// Bits por segundo do automático para este tamanho, ritmo e formato. O 4:4:4 leva metade a mais,
/// como no NVENC: o dobro de amostras de cor, que comprimem bem.
pub fn auto_bits(w: u32, h: u32, fps: u32, vf: VideoFmt) -> u64 {
    let k = if vf.yuv444 { 1.5 } else { 1.0 };
    (w as f64 * h as f64 * fps as f64 * BITS_PER_PIXEL * k).min(MAX_BITS) as u64
}

/// O formato como aparece nas mensagens e no menu.
pub fn fmt_name(vf: VideoFmt) -> &'static str {
    match (vf.hdr10, vf.yuv444) {
        (true, true) => "4:4:4 HDR10",
        (true, false) => "4:2:0 HDR10",
        (false, true) => "4:4:4",
        (false, false) => "4:2:0",
    }
}

// ---------- Vulkan: só o que a API do PyroWave pede ----------

const VK_IMAGE_CREATE_INFO: u32 = 14;
const VK_IMAGE_TYPE_2D: u32 = 1;
const VK_FORMAT_R8_UNORM: u32 = 9;
const VK_FORMAT_B8G8R8A8_UNORM: u32 = 44;
const VK_FORMAT_A2B10G10R10_UNORM: u32 = 64;
const VK_FORMAT_R16_UNORM: u32 = 70;
const VK_IMAGE_USAGE_SAMPLED: u32 = 0x4;
const VK_IMAGE_USAGE_STORAGE: u32 = 0x8;
const VK_IMAGE_USAGE_COLOR_ATTACHMENT: u32 = 0x10;
const VK_IMAGE_ASPECT_COLOR: u32 = 1;
const VK_MEMORY_D3D11_TEXTURE: u32 = 0x8;
const VK_SEMAPHORE_D3D11_FENCE: u32 = 0x8;
const VK_SEMAPHORE_TYPE_TIMELINE: u32 = 1;
const VK_QUEUE_FAMILY_EXTERNAL: u32 = !1;
const VK_QUEUE_GLOBAL_PRIORITY_MEDIUM: u32 = 256;
const VK_COLOR_SPACE_SRGB_NONLINEAR: u32 = 0;
const VK_COLOR_SPACE_HDR10_ST2084: u32 = 1000104008;
const CHROMA_420: u32 = 0;
const CHROMA_444: u32 = 1;

#[repr(C)]
struct VkImageCreateInfo {
    s_type: u32,
    p_next: *const c_void,
    flags: u32,
    image_type: u32,
    format: u32,
    extent: [u32; 3],
    mip_levels: u32,
    array_layers: u32,
    samples: u32,
    tiling: u32,
    usage: u32,
    sharing_mode: u32,
    queue_family_index_count: u32,
    p_queue_family_indices: *const u32,
    initial_layout: u32,
}

// ---------- A API em C do PyroWave (`pyrowave.h`, versão 1) ----------

type Dev = *mut c_void;
type Enc = *mut c_void;
type Dec = *mut c_void;
type Sync = *mut c_void;
type Img = *mut c_void;
type MsgCb = unsafe extern "C" fn(*mut c_void, *const std::ffi::c_char);

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ImageView {
    image: u64,
    width: u32,
    height: u32,
    image_format: u32,
    view_format: u32,
    mip_level: u32,
    layer: u32,
    aspect: u32,
    swizzle: u32,
    layout: u32,
}

#[repr(C)]
struct SyncInfo {
    device: Dev,
    external_handle: usize,
    handle_type: u32,
    semaphore_type: u32,
    import_flags: u32,
}

#[repr(C)]
struct ImageInfo {
    device: Dev,
    external_handle: usize,
    handle_type: u32,
    image_create_info: *const VkImageCreateInfo,
}

#[repr(C)]
struct CodecInfo {
    device: Dev,
    width: i32,
    height: i32,
    chroma: u32,
    /// Só o decodificador lê (caminho de fragment shader, para GPU de celular).
    fragment_path: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ExtRef {
    image: Img,
    queue_family_index: u32,
}

#[repr(C)]
struct SyncOp {
    images: *const ExtRef,
    num_images: usize,
    semaphore: u64,
    value: u64,
}

#[repr(C)]
struct ScaledInfo {
    view: ImageView,
    input_color_space: u32,
    output_color_space: u32,
    intermediate_plane_format: u32,
    ycbcr_chroma_midpoint: f32,
    force_linear_filtering: bool,
    skip_dither: bool,
    crop_rect: *const c_void,
    ycbcr_range: u32,
    ycbcr_range_bit_depth: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Packet {
    offset: usize,
    size: usize,
}

macro_rules! api {
    ($($name:ident: fn($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        #[allow(non_snake_case)]
        struct Api {
            $($name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
        }

        impl Api {
            unsafe fn load(lib: HMODULE) -> std::result::Result<Self, String> {
                Ok(Self {
                    $($name: match GetProcAddress(lib, PCSTR(concat!("pyrowave_", stringify!($name), "\0").as_ptr())) {
                        Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(f),
                        None => return Err(format!("a DLL não tem pyrowave_{}", stringify!($name))),
                    },)*
                })
            }
        }
    };
}

api! {
    get_api_version: fn(*mut u32, *mut u32, *mut u32);
    create_device_by_compat: fn(u32, u32, *const c_void, *const c_void, *const [u8; 8], u32, *mut Dev) -> i32;
    device_destroy: fn(Dev);
    device_report_performance_stats: fn(Dev, MsgCb, *mut c_void, bool);
    sync_object_create: fn(*const SyncInfo, *mut Sync) -> i32;
    sync_object_get_semaphore: fn(Sync) -> u64;
    sync_object_destroy: fn(Sync);
    image_create: fn(*const ImageInfo, *mut Img) -> i32;
    image_get_image_view: fn(Img, u32, u32, *mut ImageView) -> i32;
    image_destroy: fn(Img);
    encoder_create: fn(*const CodecInfo, *mut Enc) -> i32;
    encoder_encode_gpu_scaled: fn(Enc, *const SyncOp, *const SyncOp, *const ScaledInfo, *const usize) -> i32;
    encoder_compute_num_packets: fn(Enc, usize, *mut usize) -> i32;
    encoder_compute_num_critical_packets: fn(Enc, i32, usize, usize, *mut usize) -> i32;
    encoder_packetize: fn(Enc, *mut Packet, usize, *mut usize, *mut c_void, usize) -> i32;
    encoder_get_mapped_raw_bitstream: fn(Enc, *mut *const c_void, *mut usize, *mut *const c_void, *mut usize) -> i32;
    encoder_destroy: fn(Enc);
    decoder_create: fn(*const CodecInfo, *mut Dec) -> i32;
    decoder_push_packet: fn(Dec, *const c_void, usize) -> i32;
    decoder_decode_is_ready_with_sideband: fn(Dec, bool, i32, f32, *const u32, usize) -> bool;
    decoder_decode_gpu_buffer: fn(Dec, *const SyncOp, *const SyncOp, *const [ImageView; 3]) -> i32;
    decoder_clear: fn(Dec);
    decoder_destroy: fn(Dec);
}

// SAFETY: a tabela só tem ponteiros de função da DLL, que fica carregada até o processo acabar.
unsafe impl Send for Api {}
unsafe impl std::marker::Sync for Api {}

fn dll_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("PRISMA_PYROWAVE") {
        return p.into();
    }
    std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join(DLL))).unwrap_or_else(|| DLL.into())
}

/// A DLL está aqui? Só olha o arquivo: o Vulkan só entra quando o codec for usado de fato.
pub fn available() -> bool {
    dll_path().is_file()
}

fn api() -> Result<&'static Api> {
    static API: OnceLock<std::result::Result<Api, String>> = OnceLock::new();
    let r = API.get_or_init(|| unsafe {
        let path = dll_path();
        let lib = LoadLibraryW(&HSTRING::from(path.as_os_str())).map_err(|e| format!("{} não abriu: {}", path.display(), e.message()))?;
        let api = Api::load(lib)?;
        let (mut major, mut minor, mut patch) = (0, 0, 0);
        (api.get_api_version)(&mut major, &mut minor, &mut patch);
        if major != 1 {
            return Err(format!("{} tem a API {major}.{minor}.{patch}; este Prisma fala a 1.x", path.display()));
        }
        Ok(api)
    });
    r.as_ref().map_err(|e| Error::new(E_FAIL, format!("PyroWave: {e}")))
}

fn check(r: i32, what: &str) -> Result<()> {
    match r {
        0 => Ok(()),
        _ => Err(Error::new(E_FAIL, format!("PyroWave: {what} falhou ({r})"))),
    }
}

/// O Vulkan do PyroWave na placa do D3D11, com uma fence D3D11 que os dois lados enxergam.
/// O valor da fence só sobe: `t` é o último pedido aos dois lados.
struct Gpu {
    api: &'static Api,
    dev: Dev,
    fence: ID3D11Fence,
    ctx4: ID3D11DeviceContext4,
    sync: Sync,
    sem: u64,
    t: u64,
    /// Tempo de abrir a DLL, o Vulkan e a ponte, para a medição.
    open_ms: f64,
}

impl Gpu {
    unsafe fn new(d: &D3d) -> Result<Self> {
        let start = std::time::Instant::now();
        let api = api()?;
        let adapter = d.dev.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()?;
        let luid: [u8; 8] = std::mem::transmute(adapter.AdapterLuid);
        let mut dev = std::ptr::null_mut();
        check((api.create_device_by_compat)(0, 0, std::ptr::null(), std::ptr::null(), &luid, VK_QUEUE_GLOBAL_PRIORITY_MEDIUM, &mut dev), "abrir o Vulkan nesta placa")?;
        let fail = |e: Error| {
            (api.device_destroy)(dev);
            e
        };
        let ctx4: ID3D11DeviceContext4 = d.ctx.cast().map_err(fail)?;
        let mut fence: Option<ID3D11Fence> = None;
        d.dev.cast::<ID3D11Device5>().and_then(|d5| d5.CreateFence(0, D3D11_FENCE_FLAG_SHARED, &mut fence)).map_err(fail)?;
        let fence = fence.unwrap();
        let h = fence.CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null()).map_err(fail)?;
        let info = SyncInfo { device: dev, external_handle: h.0 as usize, handle_type: VK_SEMAPHORE_D3D11_FENCE, semaphore_type: VK_SEMAPHORE_TYPE_TIMELINE, import_flags: 0 };
        let mut sync = std::ptr::null_mut();
        // Importada, a fence fica com o PyroWave, que fecha o handle.
        if let Err(e) = check((api.sync_object_create)(&info, &mut sync), "importar a fence do D3D11") {
            let _ = CloseHandle(h);
            return Err(fail(e));
        }
        let sem = (api.sync_object_get_semaphore)(sync);
        Ok(Self { api, dev, fence, ctx4, sync, sem, t: 0, open_ms: start.elapsed().as_secs_f64() * 1000.0 })
    }

    /// Uma textura D3D11 compartilhada vista como imagem Vulkan, na mesma memória.
    unsafe fn import(&self, tex: &ID3D11Texture2D, format: u32, usage: u32) -> Result<(Img, ImageView)> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        tex.GetDesc(&mut desc);
        let h = tex.cast::<IDXGIResource1>()?.CreateSharedHandle(None, GENERIC_ALL.0, PCWSTR::null())?;
        let ci = VkImageCreateInfo {
            s_type: VK_IMAGE_CREATE_INFO,
            p_next: std::ptr::null(),
            flags: 0,
            image_type: VK_IMAGE_TYPE_2D,
            format,
            extent: [desc.Width, desc.Height, 1],
            mip_levels: 1,
            array_layers: 1,
            samples: 1,
            tiling: 0,
            usage,
            sharing_mode: 0,
            queue_family_index_count: 0,
            p_queue_family_indices: std::ptr::null(),
            initial_layout: 0,
        };
        let info = ImageInfo { device: self.dev, external_handle: h.0 as usize, handle_type: VK_MEMORY_D3D11_TEXTURE, image_create_info: &ci };
        let mut img = std::ptr::null_mut();
        if let Err(e) = check((self.api.image_create)(&info, &mut img), "importar a textura do D3D11") {
            let _ = CloseHandle(h);
            return Err(e);
        }
        let mut view = ImageView::default();
        let use_bit = if usage & VK_IMAGE_USAGE_STORAGE != 0 { VK_IMAGE_USAGE_STORAGE } else { VK_IMAGE_USAGE_SAMPLED };
        if let Err(e) = check((self.api.image_get_image_view)(img, VK_IMAGE_ASPECT_COLOR, use_bit, &mut view), "vista da imagem") {
            (self.api.image_destroy)(img);
            return Err(e);
        }
        Ok((img, view))
    }

    /// O D3D11 avisa a GPU que terminou o que pediu até aqui; o Vulkan espera por este valor.
    unsafe fn d3d_done(&mut self, ctx: &ID3D11DeviceContext) -> Result<u64> {
        self.t += 1;
        self.ctx4.Signal(&self.fence, self.t)?;
        // Sem o flush, o sinal fica na fila do D3D11 e o Vulkan espera à toa.
        ctx.Flush();
        Ok(self.t)
    }

    /// Tempos de GPU do PyroWave desde a última chamada, um por linha.
    fn stats(&self) -> String {
        unsafe extern "C" fn add(user: *mut c_void, msg: *const std::ffi::c_char) {
            let s = &mut *(user as *mut String);
            s.push_str(&std::ffi::CStr::from_ptr(msg).to_string_lossy());
        }
        let mut s = String::new();
        unsafe { (self.api.device_report_performance_stats)(self.dev, add, &mut s as *mut String as *mut c_void, true) };
        s
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            (self.api.sync_object_destroy)(self.sync);
            (self.api.device_destroy)(self.dev);
        }
    }
}

/// A CPU espera o D3D11 terminar tudo o que foi pedido até aqui.
unsafe fn cpu_wait(ctx: &ID3D11DeviceContext, q: &ID3D11Query) {
    ctx.End(q);
    ctx.Flush();
    let mut done = BOOL(0);
    while !(ctx.GetData(q, Some(&mut done as *mut _ as *mut c_void), std::mem::size_of::<BOOL>() as u32, 0).is_ok() && done.as_bool()) {
        std::hint::spin_loop();
    }
}

/// Textura que o Vulkan também enxerga (NT handle).
unsafe fn shared_texture(dev: &ID3D11Device, w: u32, h: u32, fmt: DXGI_FORMAT, bind: D3D11_BIND_FLAG) -> Result<ID3D11Texture2D> {
    let d = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: bind.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32,
    };
    let mut t = None;
    dev.CreateTexture2D(&d, None, Some(&mut t))?;
    Ok(t.unwrap())
}

// ---------- Pedaços para a rede ----------

/// Bandas mais baixas (a imagem em miniatura e o primeiro nível de detalhe): sem elas o quadro não
/// se monta, então os pacotes delas vão em dobro.
const CRITICAL_BANDS: i32 = 2;
/// Parte dos blocos que precisa chegar para um quadro incompleto ir para a tela; o que falta sai
/// com menos detalhe naquele pedaço.
const MIN_BLOCKS: f32 = 0.8;

/// Um quadro em pedaços que cabem num datagrama. Um pacote do PyroWave maior que o limite (um bloco
/// cheio de detalhe passa de 1,5 KB) vai em pedaços seguidos, com `more` ligado em todos menos no
/// último. Os `critical` primeiros pedaços (as bandas mais baixas, sem as quais o quadro não sai) vão
/// duas vezes pela rede.
#[derive(Default)]
pub struct Chunks {
    pub parts: Vec<Vec<u8>>,
    pub more: Vec<bool>,
    pub critical: usize,
}

impl Chunks {
    /// Um bloco qualquer cortado em pedaços de até `space` bytes, sem continuação (o do NVENC: o
    /// quadro só vale inteiro).
    pub fn plain(data: &[u8], space: usize) -> Self {
        let mut parts: Vec<Vec<u8>> = data.chunks(space).map(<[u8]>::to_vec).collect();
        if parts.is_empty() {
            parts.push(Vec::new());
        }
        let more = vec![false; parts.len()];
        Self { parts, more, critical: 0 }
    }

    /// Um pacote do PyroWave, em quantos pedaços precisar.
    fn push(&mut self, data: &[u8], space: usize) {
        let n = data.len().div_ceil(space).max(1);
        for (i, c) in data.chunks(space).enumerate() {
            self.parts.push(c.to_vec());
            self.more.push(i + 1 < n);
        }
    }
}

/// Em quem assiste: junta de volta os pacotes do PyroWave que chegaram inteiros. Um pacote com
/// algum pedaço faltando fica de fora (o decodificador lida com a falta dele).
pub fn join(parts: Vec<Option<Vec<u8>>>, more: &[bool]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut cur: Option<Vec<u8>> = Some(Vec::new());
    for (i, p) in parts.into_iter().enumerate() {
        match (cur.as_mut(), p) {
            (Some(c), Some(p)) => c.extend_from_slice(&p),
            _ => cur = None,
        }
        if !more.get(i).copied().unwrap_or(false) {
            if let Some(c) = cur.take().filter(|c| !c.is_empty()) {
                out.push(c);
            }
            cur = Some(Vec::new());
        }
    }
    out
}

// ---------- Host ----------

/// Codificador do host. A captura escreve em [`WaveEncoder::input`] (BGRA, ou R10G10B10A2 em PQ e
/// BT.2020 no HDR10, no tamanho da tela) e cada quadro sai em pacotes do PyroWave, cada um cabendo
/// num datagrama.
pub struct WaveEncoder {
    pub vf: VideoFmt,
    input: ID3D11Texture2D,
    img: Img,
    view: ImageView,
    enc: Enc,
    /// Teto de bytes por quadro.
    budget: usize,
    packets: Vec<Packet>,
    bits: Vec<u8>,
    query: ID3D11Query,
    gpu: Gpu,
}

impl WaveEncoder {
    /// `bits` por segundo: o teto de cada quadro é `bits / fps`.
    pub fn new(d: &D3d, src_w: u32, src_h: u32, out_w: u32, out_h: u32, fps: u32, bits: u64, vf: VideoFmt) -> Result<Self> {
        unsafe {
            let gpu = Gpu::new(d)?;
            let (dxgi, vk) = if vf.hdr10 { (DXGI_FORMAT_R10G10B10A2_UNORM, VK_FORMAT_A2B10G10R10_UNORM) } else { (DXGI_FORMAT_B8G8R8A8_UNORM, VK_FORMAT_B8G8R8A8_UNORM) };
            let input = shared_texture(&d.dev, src_w, src_h, dxgi, D3D11_BIND_FLAG(D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0))?;
            let (img, view) = gpu.import(&input, vk, VK_IMAGE_USAGE_SAMPLED | VK_IMAGE_USAGE_COLOR_ATTACHMENT)?;
            let chroma = if vf.yuv444 { CHROMA_444 } else { CHROMA_420 };
            let info = CodecInfo { device: gpu.dev, width: out_w as i32, height: out_h as i32, chroma, fragment_path: false };
            let mut enc = std::ptr::null_mut();
            if let Err(e) = check((gpu.api.encoder_create)(&info, &mut enc), "criar o codificador") {
                (gpu.api.image_destroy)(img);
                return Err(e);
            }
            let mut query = None;
            d.dev.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut query))?;
            let budget = (bits / 8 / fps.max(1) as u64).max(4096) as usize;
            Ok(Self { vf, input, img, view, enc, budget, packets: Vec::new(), bits: Vec::new(), query: query.unwrap(), gpu })
        }
    }

    pub fn input(&self) -> &ID3D11Texture2D {
        &self.input
    }

    pub fn name(&self) -> String {
        format!("PyroWave {} por Vulkan, teto de {} KB por quadro", fmt_name(self.vf), self.budget / 1024)
    }

    /// Codifica o que o D3D11 escreveu em `input` e devolve o quadro em pedaços de até `space` bytes.
    pub fn encode(&mut self, ctx: &ID3D11DeviceContext, space: usize) -> Result<Chunks> {
        self.encode_with(ctx, space, true)
    }

    /// `fenced` falso é só para medir: a CPU espera o D3D11 terminar antes de pedir ao Vulkan.
    fn encode_with(&mut self, ctx: &ID3D11DeviceContext, space: usize, fenced: bool) -> Result<Chunks> {
        unsafe {
            let api = self.gpu.api;
            let r = ExtRef { image: self.img, queue_family_index: VK_QUEUE_FAMILY_EXTERNAL };
            // HDR10: o PQ entra e sai como está (sem mapear tons) e só passa para o YCbCr do BT.2020,
            // em planos de 16 bits.
            let (cs, planes, mid) = if self.vf.hdr10 {
                (VK_COLOR_SPACE_HDR10_ST2084, VK_FORMAT_R16_UNORM, 0.5)
            } else {
                (VK_COLOR_SPACE_SRGB_NONLINEAR, VK_FORMAT_R8_UNORM, 128.0 / 255.0)
            };
            let info = ScaledInfo {
                view: self.view,
                input_color_space: cs,
                output_color_space: cs,
                intermediate_plane_format: planes,
                ycbcr_chroma_midpoint: mid,
                force_linear_filtering: false,
                skip_dither: false,
                crop_rect: std::ptr::null(),
                ycbcr_range: 0,
                ycbcr_range_bit_depth: 0,
            };
            if fenced {
                let before = self.gpu.d3d_done(ctx)?;
                let acquire = SyncOp { images: &r, num_images: 1, semaphore: self.gpu.sem, value: before };
                self.gpu.t += 1;
                let release = SyncOp { images: &r, num_images: 1, semaphore: self.gpu.sem, value: self.gpu.t };
                check((api.encoder_encode_gpu_scaled)(self.enc, &acquire, &release, &info, &self.budget), "codificar")?;
                // O D3D11 só escreve de novo na entrada depois que o Vulkan a leu.
                self.gpu.ctx4.Wait(&self.gpu.fence, self.gpu.t)?;
            } else {
                cpu_wait(ctx, &self.query);
                let acquire = SyncOp { images: &r, num_images: 1, semaphore: 0, value: 0 };
                let release = SyncOp { images: &r, num_images: 1, semaphore: 0, value: 0 };
                check((api.encoder_encode_gpu_scaled)(self.enc, &acquire, &release, &info, &self.budget), "codificar")?;
            }
            // Daqui em diante a CPU espera a GPU terminar o quadro.
            let (mut n, mut crit) = (0, 0);
            check((api.encoder_compute_num_packets)(self.enc, space, &mut n), "contar os pacotes")?;
            check((api.encoder_compute_num_critical_packets)(self.enc, CRITICAL_BANDS, space, 0, &mut crit), "contar os pacotes críticos")?;
            self.packets.resize(n, Packet::default());
            let (mut bs, mut bs_size, mut meta, mut meta_size) = (std::ptr::null(), 0usize, std::ptr::null(), 0usize);
            check((api.encoder_get_mapped_raw_bitstream)(self.enc, &mut bs, &mut bs_size, &mut meta, &mut meta_size), "ler o quadro")?;
            // O empacotador não confere o tamanho da saída: cabe tudo o que a GPU escreveu, mais o cabeçalho.
            self.bits.resize(bs_size + 4096, 0);
            let mut got = 0;
            check((api.encoder_packetize)(self.enc, self.packets.as_mut_ptr(), space, &mut got, self.bits.as_mut_ptr() as *mut c_void, self.bits.len()), "empacotar")?;
            let mut out = Chunks::default();
            for (i, p) in self.packets[..got.min(n)].iter().enumerate() {
                out.push(&self.bits[p.offset..p.offset + p.size], space);
                if i < crit {
                    out.critical = out.parts.len();
                }
            }
            Ok(out)
        }
    }
}

impl Drop for WaveEncoder {
    fn drop(&mut self) {
        unsafe {
            (self.gpu.api.encoder_destroy)(self.enc);
            (self.gpu.api.image_destroy)(self.img);
        }
    }
}

// ---------- Quem assiste ----------

/// YCbCr de faixa cheia (BT.709, como o PyroWave codifica) para RGB. Os planos de cor podem ter a
/// metade do tamanho (4:2:0) ou o tamanho inteiro (4:4:4): as coordenadas são as mesmas.
const PS: &str = r#"
Texture2D ty : register(t0);
Texture2D tu : register(t1);
Texture2D tv : register(t2);
SamplerState smp : register(s0);
float4 main(float4 p : SV_Position, float2 t : T) : SV_Target {
    float y = ty.Sample(smp, t).r;
    float u = tu.Sample(smp, t).r - 128.0 / 255.0;
    float v = tv.Sample(smp, t).r - 128.0 / 255.0;
    return float4(saturate(float3(y + 1.5748 * v, y - 0.1873 * u - 0.4681 * v, y + 1.8556 * u)), 1);
}
"#;

/// HDR10: YCbCr do BT.2020 (não constante) para R'G'B' ainda em PQ, que a janela em HDR10 mostra
/// como está.
const PS_HDR: &str = r#"
Texture2D ty : register(t0);
Texture2D tu : register(t1);
Texture2D tv : register(t2);
SamplerState smp : register(s0);
float4 main(float4 p : SV_Position, float2 t : T) : SV_Target {
    float y = ty.Sample(smp, t).r;
    float u = tu.Sample(smp, t).r - 0.5;
    float v = tv.Sample(smp, t).r - 0.5;
    return float4(saturate(float3(y + 1.4746 * v, y - 0.164553 * u - 0.571353 * v, y + 1.8814 * u)), 1);
}
"#;

/// Decodificador de quem assiste: os pacotes de um quadro viram três planos em texturas D3D11.
pub struct WaveDecoder {
    pub w: u32,
    pub h: u32,
    pub vf: VideoFmt,
    imgs: [Img; 3],
    views: [ImageView; 3],
    srvs: [Option<ID3D11ShaderResourceView>; 3],
    _planes: Vec<ID3D11Texture2D>,
    dec: Dec,
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    smp: ID3D11SamplerState,
    gpu: Gpu,
}

impl WaveDecoder {
    pub fn new(d: &D3d, w: u32, h: u32, vf: VideoFmt) -> Result<Self> {
        unsafe {
            let gpu = Gpu::new(d)?;
            let (dxgi, vk) = if vf.hdr10 { (DXGI_FORMAT_R16_UNORM, VK_FORMAT_R16_UNORM) } else { (DXGI_FORMAT_R8_UNORM, VK_FORMAT_R8_UNORM) };
            let (cw, ch) = if vf.yuv444 { (w, h) } else { (w / 2, h / 2) };
            let api = gpu.api;
            let mut planes = Vec::new();
            let mut imgs = [std::ptr::null_mut(); 3];
            let mut views = [ImageView::default(); 3];
            let mut srvs: [Option<ID3D11ShaderResourceView>; 3] = [None, None, None];
            let undo = |imgs: &[Img; 3]| imgs.iter().filter(|i| !i.is_null()).for_each(|&i| (api.image_destroy)(i));
            for (i, (pw, ph)) in [(w, h), (cw, ch), (cw, ch)].into_iter().enumerate() {
                let made = (|| -> Result<()> {
                    let t = shared_texture(&d.dev, pw, ph, dxgi, D3D11_BIND_FLAG(D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_UNORDERED_ACCESS.0))?;
                    (imgs[i], views[i]) = gpu.import(&t, vk, VK_IMAGE_USAGE_STORAGE | VK_IMAGE_USAGE_SAMPLED)?;
                    d.dev.CreateShaderResourceView(&t, None, Some(&mut srvs[i]))?;
                    planes.push(t);
                    Ok(())
                })();
                if let Err(e) = made {
                    undo(&imgs);
                    return Err(e);
                }
            }
            let chroma = if vf.yuv444 { CHROMA_444 } else { CHROMA_420 };
            let info = CodecInfo { device: gpu.dev, width: w as i32, height: h as i32, chroma, fragment_path: false };
            let mut dec = std::ptr::null_mut();
            if let Err(e) = check((api.decoder_create)(&info, &mut dec), "criar o decodificador") {
                undo(&imgs);
                return Err(e);
            }
            let shaders = (|| -> Result<_> {
                let (vb, pb) = (compile(VS, s!("vs_5_0"))?, compile(if vf.hdr10 { PS_HDR } else { PS }, s!("ps_5_0"))?);
                let (mut vs, mut ps, mut smp) = (None, None, None);
                d.dev.CreateVertexShader(&vb, None, Some(&mut vs))?;
                d.dev.CreatePixelShader(&pb, None, Some(&mut ps))?;
                let sd = D3D11_SAMPLER_DESC {
                    Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                    AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                    AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                    MaxLOD: f32::MAX,
                    ..Default::default()
                };
                d.dev.CreateSamplerState(&sd, Some(&mut smp))?;
                Ok((vs.unwrap(), ps.unwrap(), smp.unwrap()))
            })();
            let (vs, ps, smp) = match shaders {
                Ok(s) => s,
                Err(e) => {
                    (api.decoder_destroy)(dec);
                    undo(&imgs);
                    return Err(e);
                }
            };
            Ok(Self { w, h, vf, imgs, views, srvs, _planes: planes, dec, vs, ps, smp, gpu })
        }
    }

    pub fn open_ms(&self) -> f64 {
        self.gpu.open_ms
    }

    /// Decodifica um quadro com os pacotes que chegaram (faltando alguns, o quadro sai mais borrado
    /// onde eles estavam). Falso se não deu para montar nada. O desenho seguinte já espera o Vulkan.
    pub fn decode(&mut self, parts: &[Vec<u8>]) -> Result<bool> {
        unsafe {
            let api = self.gpu.api;
            (api.decoder_clear)(self.dec);
            for p in parts {
                // Um pacote estragado só fica de fora.
                let _ = (api.decoder_push_packet)(self.dec, p.as_ptr() as *const c_void, p.len());
            }
            // O padrão do PyroWave exige as duas bandas mais baixas inteiras, mas toma um bloco vazio
            // (comum em áreas lisas da tela) por perdido e recusa o quadro. Essas bandas já vão em
            // dobro pela rede; aqui basta ter chegado a maior parte dos blocos.
            if !(api.decoder_decode_is_ready_with_sideband)(self.dec, true, 0, MIN_BLOCKS, std::ptr::null(), 0) {
                return Ok(false);
            }
            let refs = self.imgs.map(|image| ExtRef { image, queue_family_index: VK_QUEUE_FAMILY_EXTERNAL });
            // O Vulkan só escreve nos planos depois que o último desenho do D3D11 os leu.
            let acquire = SyncOp { images: refs.as_ptr(), num_images: 3, semaphore: self.gpu.sem, value: self.gpu.t };
            self.gpu.t += 1;
            let release = SyncOp { images: refs.as_ptr(), num_images: 3, semaphore: self.gpu.sem, value: self.gpu.t };
            check((api.decoder_decode_gpu_buffer)(self.dec, &acquire, &release, &self.views), "decodificar")?;
            self.gpu.ctx4.Wait(&self.gpu.fence, self.gpu.t)?;
            Ok(true)
        }
    }

    /// Desenha o último quadro em `target`, no retângulo `r`, com preto em volta.
    pub fn draw(&mut self, dev: &ID3D11Device, ctx: &ID3D11DeviceContext, target: &ID3D11Texture2D, r: RECT) -> Result<()> {
        unsafe {
            let mut rtv = None;
            dev.CreateRenderTargetView(target, None, Some(&mut rtv))?;
            let rtv = rtv.unwrap();
            ctx.ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, 1.0]);
            let vp = D3D11_VIEWPORT {
                TopLeftX: r.left as f32,
                TopLeftY: r.top as f32,
                Width: (r.right - r.left) as f32,
                Height: (r.bottom - r.top) as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            };
            ctx.OMSetRenderTargets(Some(&[Some(rtv)]), None);
            ctx.RSSetViewports(Some(&[vp]));
            ctx.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
            ctx.IASetInputLayout(None);
            ctx.VSSetShader(&self.vs, None);
            ctx.PSSetShader(&self.ps, None);
            ctx.PSSetShaderResources(0, Some(&self.srvs));
            ctx.PSSetSamplers(0, Some(&[Some(self.smp.clone())]));
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None, None, None]));
            ctx.OMSetRenderTargets(None, None);
            // O próximo quadro do Vulkan espera este desenho.
            self.gpu.d3d_done(ctx)?;
            Ok(())
        }
    }
}

impl Drop for WaveDecoder {
    fn drop(&mut self) {
        unsafe {
            (self.gpu.api.decoder_destroy)(self.dec);
            for &i in &self.imgs {
                (self.gpu.api.image_destroy)(i);
            }
        }
    }
}

// ---------- Medição ----------

/// `--test-wave=saida.txt|segundos|imagem`: mede, na mesma placa e sem rede, quanto o PyroWave leva
/// da imagem pronta no D3D11 até os pacotes, e dos pacotes até a imagem pronta de novo, contra o
/// NVENC e o Media Foundation (AV1 e HEVC) com os mesmos quadros. A imagem rola um pouco a cada
/// quadro, para o NVENC não ganhar de graça com a tela parada. Não captura a tela nem abre rede.
#[cfg(any(debug_assertions, feature = "stream"))]
pub fn bench(arg: &str) {
    let mut parts = arg.split('|');
    let out = parts.next().filter(|s| !s.is_empty()).unwrap_or("pyrowave.txt").to_string();
    let secs: u32 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(10);
    let image = parts.next().filter(|s| !s.is_empty()).unwrap_or("docs/guide/welcome-pt.png").to_string();
    // Formato do PyroWave: 420 (o padrão), 444, hdr ou hdr444.
    let f = parts.next().unwrap_or("");
    let vf = VideoFmt { hdr10: f.starts_with("hdr"), yuv444: f.ends_with("444") };
    let r = unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        let _ = windows::Win32::Media::MediaFoundation::MFStartup(windows::Win32::Media::MediaFoundation::MF_VERSION, windows::Win32::Media::MediaFoundation::MFSTARTUP_LITE);
        bench_run(secs, &image, vf)
    };
    let text = match r {
        Ok(t) => t,
        Err(e) => format!("falhou: {}\n", e.message()),
    };
    print!("{text}");
    if let Err(e) = std::fs::write(&out, &text) {
        eprintln!("{out}: {e}");
    }
}

#[cfg(any(debug_assertions, feature = "stream"))]
struct Series(Vec<f64>);

#[cfg(any(debug_assertions, feature = "stream"))]
impl Series {
    fn line(&mut self, name: &str) -> String {
        if self.0.is_empty() {
            return format!("  {name}: sem amostras\n");
        }
        self.0.sort_by(|a, b| a.total_cmp(b));
        let q = |p: f64| self.0[((self.0.len() - 1) as f64 * p).round() as usize];
        let avg = self.0.iter().sum::<f64>() / self.0.len() as f64;
        format!("  {name}: média {avg:.2} ms, mediana {:.2}, p95 {:.2}, p99 {:.2}, máx {:.2}\n", q(0.5), q(0.95), q(0.99), q(1.0))
    }
}

#[cfg(any(debug_assertions, feature = "stream"))]
unsafe fn bench_run(secs: u32, image: &str, vf: VideoFmt) -> Result<String> {
    use std::fmt::Write as _;
    use std::time::{Duration, Instant};

    use crate::capture::decoder::Decoder;
    use crate::capture::engine::{bgra_texture, bitrate, ConvOut, Prep};
    use crate::capture::live_enc::LiveEncoder;
    use crate::capture::mux::{Codec, Pkt};

    const W: u32 = 1920;
    const H: u32 = 1080;
    const FPS: u32 = 60;
    const BOUNDARY: usize = super::MTU - super::VIDEO_HEAD;
    let frames = secs * FPS;
    let d = crate::capture::d3d()?;
    if let Ok(mt) = d.ctx.cast::<ID3D11Multithread>() {
        let _ = mt.SetMultithreadProtected(true);
    }
    let adapter = d.dev.cast::<IDXGIDevice>()?.GetAdapter()?.GetDesc()?;
    let gpu_name = String::from_utf16_lossy(&adapter.Description).trim_end_matches('\0').to_string();
    let mut s = format!("PyroWave contra NVENC, {W}x{H} a {FPS} quadros, {frames} quadros, {gpu_name}\nimagem: {image} (rolando 7 px por quadro)\n");

    // A tela de teste: a imagem repetida numa faixa com o dobro da altura, para rolar. No HDR10, os
    // mesmos valores viram códigos PQ de 10 bits (a imagem fica escura, mas a medida é a mesma).
    let (iw, ih, px) = crate::capture::load_image(image).ok_or_else(|| Error::new(E_FAIL, format!("não abriu {image}")))?;
    let ten = |v: u8| v as u32 * 4 + v as u32 / 64;
    let mut pixels = vec![0u8; (W * H * 2 * 4) as usize];
    for y in 0..H * 2 {
        for x in 0..W {
            let (sx, sy) = (x % iw, y % ih);
            let src = ((sy * iw + sx) * 4) as usize;
            let dst = ((y * W + x) * 4) as usize;
            if vf.hdr10 {
                let (b, g, r) = (px[src], px[src + 1], px[src + 2]);
                let packed = ten(r) | ten(g) << 10 | ten(b) << 20 | 3 << 30;
                pixels[dst..dst + 4].copy_from_slice(&packed.to_le_bytes());
            } else {
                pixels[dst..dst + 4].copy_from_slice(&px[src..src + 4]);
            }
        }
    }
    let fmt = if vf.hdr10 { DXGI_FORMAT_R10G10B10A2_UNORM } else { DXGI_FORMAT_B8G8R8A8_UNORM };
    // Os três canais de cor de um pixel, e o maior valor deles.
    let rgb = |p: &[u8]| -> [f64; 3] {
        if vf.hdr10 {
            let v = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
            [(v & 1023) as f64, (v >> 10 & 1023) as f64, (v >> 20 & 1023) as f64]
        } else {
            [p[0] as f64, p[1] as f64, p[2] as f64]
        }
    };
    let peak: f64 = if vf.hdr10 { 1023.0 } else { 255.0 };
    let cd = D3D11_TEXTURE2D_DESC {
        Width: W,
        Height: H * 2,
        MipLevels: 1,
        ArraySize: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        ..Default::default()
    };
    let init = D3D11_SUBRESOURCE_DATA { pSysMem: pixels.as_ptr() as *const _, SysMemPitch: W * 4, SysMemSlicePitch: 0 };
    let mut tex = None;
    d.dev.CreateTexture2D(&cd, Some(&init), Some(&mut tex))?;
    let canvas = tex.unwrap();
    let frame_box = |i: u32| {
        let off = (i * 7) % H;
        D3D11_BOX { left: 0, top: off, front: 0, right: W, bottom: off + H, back: 1 }
    };
    let mut query = None;
    d.dev.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut query))?;
    let query: ID3D11Query = query.unwrap();
    let gpu_wait = || cpu_wait(&d.ctx, &query);
    let interval = Duration::from_secs_f64(1.0 / FPS as f64);
    let target = bgra_texture(&d.dev, W, H, fmt)?;
    let full = RECT { left: 0, top: 0, right: W as i32, bottom: H as i32 };
    // Qualidade: o quadro mostrado volta para a CPU e é comparado com a imagem original (PSNR do RGB).
    let sd = D3D11_TEXTURE2D_DESC {
        Width: W,
        Height: H,
        MipLevels: 1,
        ArraySize: 1,
        Format: fmt,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        ..Default::default()
    };
    let mut staging = None;
    d.dev.CreateTexture2D(&sd, None, Some(&mut staging))?;
    let staging: ID3D11Texture2D = staging.unwrap();
    let psnr = |i: u32| -> Result<f64> {
        d.ctx.CopyResource(&staging, &target);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        d.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let off = (i * 7) % H;
        let mut err = 0f64;
        for y in 0..H {
            let row = std::slice::from_raw_parts((m.pData as *const u8).add((y * m.RowPitch) as usize), (W * 4) as usize);
            let src = &pixels[(((off + y) * W) * 4) as usize..(((off + y) * W + W) * 4) as usize];
            for (a, b) in row.chunks_exact(4).zip(src.chunks_exact(4)) {
                let (a, b) = (rgb(a), rgb(b));
                for c in 0..3 {
                    let e = a[c] - b[c];
                    err += e * e;
                }
            }
        }
        d.ctx.Unmap(&staging, 0);
        let mse = err / (W * H * 3) as f64;
        Ok(if mse == 0.0 { 99.0 } else { 10.0 * (peak * peak / mse).log10() })
    };

    // ---- PyroWave ----
    let bits = auto_bits(W, H, FPS, vf);
    let t = Instant::now();
    let mut enc = WaveEncoder::new(&d, W, H, W, H, FPS, bits, vf)?;
    let enc_open = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let mut dec = WaveDecoder::new(&d, W, H, vf)?;
    let dec_open = t.elapsed().as_secs_f64() * 1000.0;
    let _ = writeln!(s, "\nPyroWave ({}), automático de {:.0} Mbit/s, pacotes de até {BOUNDARY} bytes", enc.name(), bits as f64 / 1e6);
    let _ = writeln!(s, "  abrir no host: {enc_open:.0} ms (Vulkan e fence {:.0} ms); em quem assiste: {dec_open:.0} ms (Vulkan e fence {:.0} ms)", enc.gpu.open_ms, dec.open_ms());
    let _ = enc.gpu.stats();
    let _ = dec.gpu.stats();
    let runs = [
        ("ponte pela fence, no ritmo de 60 quadros (como na transmissão)", true, 0u32, true),
        ("ponte pela fence, sem pausa entre os quadros", true, 0, false),
        ("sem ponte na GPU, a CPU espera o D3D11, no ritmo de 60 quadros", false, 0, true),
        ("ponte pela fence, 1 datagrama em 50 perdido ao acaso", true, 50, true),
        ("ponte pela fence, 1 datagrama em 10 perdido ao acaso", true, 10, true),
    ];
    let mut rng = 0x2545_f491_4f6c_dd1du64;
    for (name, fenced, loss, paced) in runs {
        let (mut e, mut dd, mut dr, mut tot, mut q) = (Series(Vec::new()), Series(Vec::new()), Series(Vec::new()), Series(Vec::new()), Vec::new());
        let (mut bytes, mut wire, mut pkts, mut split, mut crit, mut shown) = (0usize, 0usize, 0usize, 0usize, 0usize, 0u32);
        let mut next = Instant::now();
        for i in 0..frames {
            if paced {
                std::thread::sleep(next.saturating_duration_since(Instant::now()));
                next += interval;
            }
            d.ctx.CopySubresourceRegion(enc.input(), 0, 0, 0, 0, &canvas, 0, Some(&frame_box(i)));
            let t0 = Instant::now();
            let out = enc.encode_with(&d.ctx, BOUNDARY, fenced)?;
            let t1 = Instant::now();
            bytes += out.parts.iter().map(Vec::len).sum::<usize>();
            wire += out.parts.iter().enumerate().map(|(k, p)| (p.len() + super::VIDEO_HEAD) * if k < out.critical { 2 } else { 1 }).sum::<usize>();
            pkts += out.parts.len() + out.critical;
            split += out.more.iter().filter(|&&m| m).count();
            crit += out.critical;
            // A rede: cada datagrama some com a chance pedida; os críticos vão duas vezes.
            let mut lose = || {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                loss > 0 && rng % loss as u64 == 0
            };
            let got: Vec<Option<Vec<u8>>> = out.parts.iter().enumerate().map(|(k, p)| {
                let arrived = !lose() || (k < out.critical && !lose());
                arrived.then(|| p.clone())
            }).collect();
            let packets = join(got, &out.more);
            if dec.decode(&packets)? {
                gpu_wait();
                let t2 = Instant::now();
                dec.draw(&d.dev, &d.ctx, &target, full)?;
                gpu_wait();
                let t3 = Instant::now();
                shown += 1;
                if i % 15 == 0 {
                    q.push(psnr(i)?);
                }
                dd.0.push((t2 - t1).as_secs_f64() * 1000.0);
                dr.0.push((t3 - t2).as_secs_f64() * 1000.0);
                tot.0.push((t3 - t0).as_secs_f64() * 1000.0);
            }
            e.0.push((t1 - t0).as_secs_f64() * 1000.0);
        }
        let _ = writeln!(s, " {name}:");
        s += &e.line("codificar (imagem no D3D11 até os pacotes na CPU)");
        s += &dd.line("decodificar (pacotes até os planos prontos no D3D11)");
        s += &dr.line("converter para RGB e desenhar");
        s += &tot.line("total sem a rede");
        if !q.is_empty() {
            q.sort_by(|a: &f64, b| a.total_cmp(b));
            let _ = writeln!(s, "  qualidade dos quadros mostrados (PSNR do RGB contra a imagem original): mediana {:.1} dB, pior {:.1} dB", q[q.len() / 2], q[0]);
        }
        let per = |v: usize| v as f64 / frames as f64;
        let _ = writeln!(
            s,
            "  {:.1} Mbit/s de vídeo, {:.1} Mbit/s na rede com cabeçalhos e críticos em dobro; por quadro {:.0} KB, {:.0} datagramas, {:.1} críticos, {:.1} blocos partidos; {shown} de {frames} quadros mostrados",
            per(bytes) * 8.0 * FPS as f64 / 1e6,
            per(wire) * 8.0 * FPS as f64 / 1e6,
            per(bytes) / 1024.0,
            per(pkts),
            per(crit),
            per(split),
        );
    }
    let _ = writeln!(s, "  tempos de GPU do PyroWave no host:\n{}", enc.gpu.stats().trim_end());
    let _ = writeln!(s, "  tempos de GPU do PyroWave em quem assiste:\n{}", dec.gpu.stats().trim_end());
    drop(enc);
    drop(dec);

    // ---- NVENC e Media Foundation, como na transmissão hoje ----
    // A comparação é sempre no SDR 4:2:0; nos outros formatos só o PyroWave é medido.
    if vf != VideoFmt::default() {
        return Ok(s);
    }
    let mid = bgra_texture(&d.dev, W, H, DXGI_FORMAT_B8G8R8A8_UNORM)?;
    for codec in [Codec::Av1, Codec::Hevc] {
        let mut conv = Prep::new(&d.dev, &d.ctx, &mid, W, H, W, H, FPS, ConvOut::Nv12)?;
        // O automático da transmissão, na prioridade Equilíbrio.
        let bits = (bitrate(W, H, FPS, codec) as u64 * 4 / 2).min(150_000_000) as u32;
        let mut nv = match LiveEncoder::open(&d.dev, codec, W, H, FPS, bits, VideoFmt::default()) {
            Ok(x) => x,
            Err(e) => {
                let _ = writeln!(s, "\n{}: não abriu ({})", super::codec_name(super::VideoCodec::Std(codec)), e.message());
                continue;
            }
        };
        let mut mf = Decoder::new(&d.dev, codec, W, H, FPS, None, false)?;
        let _ = writeln!(s, "\n{} ({}), automático de {:.1} Mbit/s, decodificando pelo Media Foundation", super::codec_name(super::VideoCodec::Std(codec)), nv.name(), bits as f64 / 1e6);
        let (mut e, mut dd, mut tot) = (Series(Vec::new()), Series(Vec::new()), Series(Vec::new()));
        let (mut bytes, mut late, mut shown) = (0usize, 0u32, 0u32);
        let mut pkts: Vec<Pkt> = Vec::new();
        let mut decoded = Vec::new();
        let mut next = Instant::now();
        for i in 0..frames {
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
            next += interval;
            d.ctx.CopySubresourceRegion(&mid, 0, 0, 0, 0, &canvas, 0, Some(&frame_box(i)));
            let t0 = Instant::now();
            let pts = super::now();
            while !nv.ready() {
                nv.pump(&mut pkts)?;
            }
            let t = conv.convert(&d.ctx)?;
            nv.feed(&t, pts)?;
            while pkts.is_empty() && t0.elapsed() < Duration::from_millis(200) {
                nv.pump(&mut pkts)?;
            }
            let t1 = Instant::now();
            e.0.push((t1 - t0).as_secs_f64() * 1000.0);
            for p in pkts.drain(..) {
                bytes += p.data.len();
                decoded.clear();
                mf.decode(Some(&p), &mut decoded)?;
                if decoded.iter().any(|x| x.pts == p.pts) {
                    gpu_wait();
                    let t2 = Instant::now();
                    shown += 1;
                    dd.0.push((t2 - t1).as_secs_f64() * 1000.0);
                    tot.0.push((t2 - t0).as_secs_f64() * 1000.0);
                } else if !decoded.is_empty() || p.pts != pts {
                    late += 1;
                }
            }
        }
        s += &e.line("codificar (imagem no D3D11 até o pacote na CPU, com a conversão para NV12)");
        s += &dd.line("decodificar (pacote até a imagem pronta no D3D11)");
        s += &tot.line("total sem a rede (sem o desenho, que leva menos de 0,2 ms nos dois)");
        let _ = writeln!(s, "  {:.1} Mbit/s de fato, {:.0} KB por quadro, {shown} de {frames} quadros saíram na hora, {late} atrasados", bytes as f64 * 8.0 * FPS as f64 / frames as f64 / 1e6, bytes as f64 / frames as f64 / 1024.0);
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_split_and_join() {
        let mut c = Chunks::default();
        c.push(&[1, 2, 3], 4);
        c.push(&[4, 5, 6, 7, 8, 9, 10], 4);
        c.push(&[11], 4);
        assert_eq!(c.parts, vec![vec![1, 2, 3], vec![4, 5, 6, 7], vec![8, 9, 10], vec![11]]);
        assert_eq!(c.more, vec![false, true, false, false]);
        let all: Vec<Option<Vec<u8>>> = c.parts.iter().cloned().map(Some).collect();
        assert_eq!(join(all, &c.more), vec![vec![1, 2, 3], vec![4, 5, 6, 7, 8, 9, 10], vec![11]]);
        // Faltando um pedaço do pacote partido, ele sai inteiro; os outros ficam.
        let lost = vec![Some(vec![1, 2, 3]), None, Some(vec![8, 9, 10]), Some(vec![11])];
        assert_eq!(join(lost, &c.more), vec![vec![1, 2, 3], vec![11]]);
        assert_eq!(Chunks::plain(&[], 4).parts, vec![Vec::<u8>::new()]);
    }
}
