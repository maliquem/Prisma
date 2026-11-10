//! Decodificador HEVC 4:4:4 pelo NVDEC, só em placa NVIDIA: o Media Foundation não decodifica 4:4:4.
//! O quadro vai para uma textura R8 com os três planos empilhados (Y, U e V, cada um com a altura do
//! vídeo), e um shader converte para RGB ao desenhar. Com a janela na mesma placa, o CUDA copia direto
//! para a textura; senão (notebook com duas placas), o quadro passa pela memória da CPU.

use std::ffi::c_void;
use std::os::raw::{c_int, c_ulong};

use mini_cuda::{
    require_driver, CUDA_ARRAY3D_DESCRIPTOR_st, CUDA_EXTERNAL_MEMORY_HANDLE_DESC_st, CUDA_EXTERNAL_MEMORY_HANDLE_DESC_st__bindgen_ty_1,
    CUDA_EXTERNAL_MEMORY_HANDLE_DESC_st__bindgen_ty_1__bindgen_ty_1, CUDA_EXTERNAL_MEMORY_MIPMAPPED_ARRAY_DESC_st, CUDA_MEMCPY2D_st, CUarray,
    CUarray_format, CUcontext, CUexternalMemory, CUexternalMemoryHandleType, CUmemorytype, CUmipmappedArray, CUresult, ContextScope,
};
use mini_nvcodec::cuvid_sys::*;
use mini_nvcodec::{require_cuvid, CuvidApi};
use windows::core::*;
use windows::Win32::Foundation::{E_FAIL, RECT};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

use super::hdr::{compile, VS};

fn cuda_err(what: &str, e: impl std::fmt::Display) -> Error {
    Error::new(E_FAIL, format!("NVDEC: {what}: {e}"))
}

fn ok(what: &str, r: CUresult) -> Result<()> {
    if r == CUresult::CUDA_SUCCESS {
        Ok(())
    } else {
        Err(Error::new(E_FAIL, format!("NVDEC: {what} falhou ({})", r.0)))
    }
}

/// Esta placa decodifica HEVC 4:4:4 de 8 bits.
pub fn can_444() -> bool {
    let Ok(api) = require_cuvid() else { return false };
    let Ok(_scope) = ContextScope::primary(0) else { return false };
    unsafe {
        let mut caps: CUVIDDECODECAPS = std::mem::zeroed();
        caps.eCodecType = cudaVideoCodec::HEVC;
        caps.eChromaFormat = cudaVideoChromaFormat::YUV444;
        caps.nBitDepthMinus8 = 0;
        (api.cuvidGetDecoderCaps)(&mut caps) == CUresult::CUDA_SUCCESS
            && caps.bIsSupported != 0
            && caps.nOutputFormatMask & (1 << cudaVideoSurfaceFormat::YUV444.0) != 0
    }
}

type HostRegister = unsafe extern "system" fn(*mut c_void, usize, u32) -> CUresult;
type HostUnregister = unsafe extern "system" fn(*mut c_void) -> CUresult;

/// Memória do quadro na CPU, travada para o CUDA quando dá: a cópia da placa chega bem mais rápido
/// do que em memória comum. `cuMemHostRegister` vem direto do `nvcuda.dll`, que já está carregado.
struct Host {
    ptr: *mut u8,
    len: usize,
    unreg: Option<HostUnregister>,
}

impl Host {
    const EMPTY: Host = Host { ptr: std::ptr::null_mut(), len: 0, unreg: None };

    fn layout(len: usize) -> std::alloc::Layout {
        std::alloc::Layout::from_size_align(len.max(1), 4096).unwrap()
    }

    /// Garante `len` bytes (só realoca se o tamanho mudar). Chamar com o contexto do CUDA ativo.
    unsafe fn ensure(&mut self, len: usize) -> bool {
        if self.len == len && !self.ptr.is_null() {
            return true;
        }
        self.free();
        let ptr = std::alloc::alloc_zeroed(Self::layout(len));
        if ptr.is_null() {
            return false;
        }
        self.ptr = ptr;
        self.len = len;
        let m = GetModuleHandleW(w!("nvcuda.dll")).ok();
        let reg = m.and_then(|m| GetProcAddress(m, s!("cuMemHostRegister_v2")));
        let unreg = m.and_then(|m| GetProcAddress(m, s!("cuMemHostUnregister")));
        if let (Some(r), Some(u)) = (reg, unreg) {
            let r = std::mem::transmute::<unsafe extern "system" fn() -> isize, HostRegister>(r);
            if r(ptr as *mut c_void, len, 0) == CUresult::CUDA_SUCCESS {
                self.unreg = Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, HostUnregister>(u));
            }
        }
        true
    }

    unsafe fn free(&mut self) {
        if self.ptr.is_null() {
            return;
        }
        if let Some(u) = self.unreg.take() {
            let _ = u(self.ptr as *mut c_void);
        }
        std::alloc::dealloc(self.ptr, Self::layout(self.len));
        *self = Host::EMPTY;
    }
}

/// O que os retornos do analisador mexem. Fica numa caixa: o endereço vai para o NVDEC.
struct State {
    api: &'static CuvidApi,
    dec: CUvideodecoder,
    w: u32,
    h: u32,
    frame: Host,
    pitch: u32,
    fresh: bool,
    failed: Option<String>,
    /// Texturas que o CUDA escreve direto, alternadas (a janela ainda pode estar lendo a anterior).
    /// Vazio: o quadro passa pela memória.
    gpu: Vec<CUarray>,
    /// A textura do quadro mais novo.
    shown: usize,
}

unsafe extern "C" fn on_sequence(user: *mut c_void, f: *mut CUVIDEOFORMAT) -> c_int {
    let s = &mut *(user as *mut State);
    let f = &*f;
    let n = (f.min_num_decode_surfaces as c_ulong).max(1) + 3;
    if !s.dec.is_null() {
        return n as c_int;
    }
    if f.chroma_format != cudaVideoChromaFormat::YUV444 || f.bit_depth_luma_minus8 != 0 {
        s.failed = Some("o vídeo não é 4:4:4 de 8 bits".into());
        return 0;
    }
    let mut ci: CUVIDDECODECREATEINFO = std::mem::zeroed();
    ci.ulWidth = f.coded_width as c_ulong;
    ci.ulHeight = f.coded_height as c_ulong;
    ci.ulNumDecodeSurfaces = n;
    ci.CodecType = f.codec;
    ci.ChromaFormat = cudaVideoChromaFormat::YUV444;
    ci.ulCreationFlags = cudaVideoCreate_PreferCUVID;
    ci.ulMaxWidth = f.coded_width as c_ulong;
    ci.ulMaxHeight = f.coded_height as c_ulong;
    let a = &f.display_area;
    ci.display_area = CUVIDRECT { left: a.left as i16, top: a.top as i16, right: a.right as i16, bottom: a.bottom as i16 };
    ci.OutputFormat = cudaVideoSurfaceFormat::YUV444;
    ci.DeinterlaceMode = cudaVideoDeinterlaceMode::WEAVE;
    ci.ulTargetWidth = s.w as c_ulong;
    ci.ulTargetHeight = s.h as c_ulong;
    ci.ulNumOutputSurfaces = 2;
    let r = (s.api.cuvidCreateDecoder)(&mut s.dec, &mut ci);
    if r != CUresult::CUDA_SUCCESS {
        s.failed = Some(format!("criar o decodificador falhou ({})", r.0));
        return 0;
    }
    n as c_int
}

unsafe extern "C" fn on_decode(user: *mut c_void, pic: *mut CUVIDPICPARAMS) -> c_int {
    let s = &mut *(user as *mut State);
    let r = (s.api.cuvidDecodePicture)(s.dec, pic);
    if r != CUresult::CUDA_SUCCESS {
        s.failed = Some(format!("decodificar falhou ({})", r.0));
        return 0;
    }
    1
}

unsafe extern "C" fn on_display(user: *mut c_void, d: *mut CUVIDPARSERDISPINFO) -> c_int {
    let s = &mut *(user as *mut State);
    let mut vpp: CUVIDPROCPARAMS = std::mem::zeroed();
    vpp.progressive_frame = 1;
    let (mut ptr, mut pitch) = (0u64, 0u32);
    let r = (s.api.cuvidMapVideoFrame64)(s.dec, (*d).picture_index, &mut ptr, &mut pitch, &mut vpp);
    if r != CUresult::CUDA_SUCCESS {
        s.failed = Some(format!("ler o quadro falhou ({})", r.0));
        return 0;
    }
    // Os três planos vêm em seguida, cada um com a altura do vídeo.
    if !s.gpu.is_empty() {
        let next = (s.shown + 1) % s.gpu.len();
        let copied = copy_to_array(ptr, pitch, s.gpu[next], s.w, s.h * 3);
        let _ = (s.api.cuvidUnmapVideoFrame64)(s.dec, ptr);
        if let Err(e) = copied {
            s.failed = Some(e);
            return 0;
        }
        s.shown = next;
        s.fresh = true;
        return 1;
    }
    let bytes = pitch as usize * s.h as usize * 3;
    let copied = if s.frame.ensure(bytes) {
        mini_cuda::memcpy_dtoh(std::slice::from_raw_parts_mut(s.frame.ptr, bytes), ptr, std::ptr::null_mut()).map_err(|e| e.to_string())
    } else {
        Err("sem memória".to_string())
    };
    let _ = (s.api.cuvidUnmapVideoFrame64)(s.dec, ptr);
    if let Err(e) = copied {
        s.failed = Some(format!("copiar o quadro falhou ({e})"));
        return 0;
    }
    s.pitch = pitch;
    s.fresh = true;
    1
}

/// Cópia dentro da placa, do quadro decodificado para a textura, e espera terminar.
unsafe fn copy_to_array(src: u64, pitch: u32, dst: CUarray, w: u32, rows: u32) -> std::result::Result<(), String> {
    let drv = require_driver().map_err(|e| e.to_string())?;
    let cp = CUDA_MEMCPY2D_st {
        srcXInBytes: 0,
        srcY: 0,
        srcMemoryType: CUmemorytype::CU_MEMORYTYPE_DEVICE,
        srcHost: std::ptr::null(),
        srcDevice: src,
        srcArray: std::ptr::null_mut(),
        srcPitch: pitch as usize,
        dstXInBytes: 0,
        dstY: 0,
        dstMemoryType: CUmemorytype::CU_MEMORYTYPE_ARRAY,
        dstHost: std::ptr::null_mut(),
        dstDevice: 0,
        dstArray: dst,
        dstPitch: 0,
        WidthInBytes: w as usize,
        Height: rows as usize,
    };
    let r = (drv.cuMemcpy2DAsync_v2)(&cp, std::ptr::null_mut());
    if r != CUresult::CUDA_SUCCESS {
        return Err(format!("copiar para a textura falhou ({})", r.0));
    }
    let r = (drv.cuStreamSynchronize)(std::ptr::null_mut());
    if r != CUresult::CUDA_SUCCESS {
        return Err(format!("esperar a cópia falhou ({})", r.0));
    }
    Ok(())
}

/// O CUDA da placa 0 é a placa desta janela.
unsafe fn same_gpu(dev: &ID3D11Device) -> bool {
    let Ok(drv) = require_driver() else { return false };
    let Ok(desc) = dev.cast::<IDXGIDevice>().and_then(|d| d.GetAdapter()).and_then(|a| a.GetDesc()) else {
        return false;
    };
    let (mut luid, mut mask) = ([0u8; 8], 0u32);
    if (drv.cuDeviceGetLuid)(luid.as_mut_ptr(), &mut mask, 0) != CUresult::CUDA_SUCCESS {
        return false;
    }
    luid[..4] == desc.AdapterLuid.LowPart.to_le_bytes() && luid[4..] == desc.AdapterLuid.HighPart.to_le_bytes()
}

/// Textura que o CUDA também enxerga.
struct Shared {
    ext: CUexternalMemory,
    mip: CUmipmappedArray,
    arr: CUarray,
}

unsafe fn share(tex: &ID3D11Texture2D, w: u32, rows: u32) -> std::result::Result<Shared, String> {
    let drv = require_driver().map_err(|e| e.to_string())?;
    let handle = tex
        .cast::<IDXGIResource1>()
        .and_then(|r| r.CreateSharedHandle(None, (DXGI_SHARED_RESOURCE_READ | DXGI_SHARED_RESOURCE_WRITE).0, PCWSTR::null()))
        .map_err(|e| format!("compartilhar a textura: {}", e.message()))?;
    let hd = CUDA_EXTERNAL_MEMORY_HANDLE_DESC_st {
        type_: CUexternalMemoryHandleType::CU_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_RESOURCE,
        handle: CUDA_EXTERNAL_MEMORY_HANDLE_DESC_st__bindgen_ty_1 {
            win32: CUDA_EXTERNAL_MEMORY_HANDLE_DESC_st__bindgen_ty_1__bindgen_ty_1 { handle: handle.0, name: std::ptr::null() },
        },
        size: w as u64 * rows as u64,
        // CUDA_EXTERNAL_MEMORY_DEDICATED: obrigatório para recurso do D3D11.
        flags: 1,
        reserved: [0; 16],
    };
    let mut ext: CUexternalMemory = std::ptr::null_mut();
    let r = (drv.cuImportExternalMemory)(&mut ext, &hd);
    let _ = CloseHandle(handle);
    if r != CUresult::CUDA_SUCCESS {
        return Err(format!("o CUDA não abriu a textura ({})", r.0));
    }
    let md = CUDA_EXTERNAL_MEMORY_MIPMAPPED_ARRAY_DESC_st {
        offset: 0,
        arrayDesc: CUDA_ARRAY3D_DESCRIPTOR_st {
            Width: w as usize,
            Height: rows as usize,
            Depth: 0,
            Format: CUarray_format::CU_AD_FORMAT_UNSIGNED_INT8,
            NumChannels: 1,
            Flags: 0,
        },
        numLevels: 1,
        reserved: [0; 16],
    };
    let mut mip: CUmipmappedArray = std::ptr::null_mut();
    let r = (drv.cuExternalMemoryGetMappedMipmappedArray)(&mut mip, ext, &md);
    if r != CUresult::CUDA_SUCCESS {
        let _ = (drv.cuDestroyExternalMemory)(ext);
        return Err(format!("o CUDA não mapeou a textura ({})", r.0));
    }
    let mut arr: CUarray = std::ptr::null_mut();
    let r = (drv.cuMipmappedArrayGetLevel)(&mut arr, mip, 0);
    if r != CUresult::CUDA_SUCCESS {
        let _ = (drv.cuMipmappedArrayDestroy)(mip);
        let _ = (drv.cuDestroyExternalMemory)(ext);
        return Err(format!("o CUDA não achou a textura ({})", r.0));
    }
    Ok(Shared { ext, mip, arr })
}

unsafe fn unshare(s: &Shared) {
    if let Ok(drv) = require_driver() {
        let _ = (drv.cuMipmappedArrayDestroy)(s.mip);
        let _ = (drv.cuDestroyExternalMemory)(s.ext);
    }
}

unsafe fn plane_texture(dev: &ID3D11Device, w: u32, rows: u32, shared: bool) -> Result<(ID3D11Texture2D, ID3D11ShaderResourceView)> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: rows,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_R8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        MiscFlags: if shared { (D3D11_RESOURCE_MISC_SHARED.0 | D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0) as u32 } else { 0 },
        ..Default::default()
    };
    let mut tex = None;
    dev.CreateTexture2D(&desc, None, Some(&mut tex))?;
    let tex = tex.unwrap();
    let mut srv = None;
    dev.CreateShaderResourceView(&tex, None, Some(&mut srv))?;
    Ok((tex, srv.unwrap()))
}

pub struct Nvdec {
    api: &'static CuvidApi,
    ctx: CUcontext,
    parser: CUvideoparser,
    st: Box<State>,
    texs: Vec<(ID3D11Texture2D, ID3D11ShaderResourceView)>,
    shared: Vec<Shared>,
    pub w: u32,
    pub h: u32,
    /// O quadro vai direto da decodificação para a textura, sem passar pela CPU.
    pub direct: bool,
}

impl Nvdec {
    pub fn new(dev: &ID3D11Device, w: u32, h: u32) -> Result<Self> {
        let api = require_cuvid().map_err(|e| cuda_err("abrir nvcuvid.dll", e))?;
        let scope = ContextScope::primary(0).map_err(|e| cuda_err("abrir a placa", e))?;
        let ctx = scope.context();
        let mut st = Box::new(State {
            api,
            dec: std::ptr::null_mut(),
            w,
            h,
            frame: Host::EMPTY,
            pitch: 0,
            fresh: false,
            failed: None,
            gpu: Vec::new(),
            shown: 0,
        });
        unsafe {
            let mut pp: CUVIDPARSERPARAMS = std::mem::zeroed();
            pp.CodecType = cudaVideoCodec::HEVC;
            pp.ulMaxNumDecodeSurfaces = 1;
            pp.ulClockRate = 10_000_000;
            // Sem fila de exibição: o quadro sai na mesma chamada que o recebe.
            pp.ulMaxDisplayDelay = 0;
            pp.pUserData = &mut *st as *mut State as *mut c_void;
            pp.pfnSequenceCallback = Some(on_sequence);
            pp.pfnDecodePicture = Some(on_decode);
            pp.pfnDisplayPicture = Some(on_display);
            let mut parser: CUvideoparser = std::ptr::null_mut();
            ok("criar o analisador", (api.cuvidCreateVideoParser)(&mut parser, &mut pp))?;

            // Direto pela placa quando a janela está nela e o CUDA aceita a textura; senão, pela memória.
            let (mut texs, mut shared) = (Vec::new(), Vec::new());
            if same_gpu(dev) && std::env::var("PRISMA_NVDEC_CPU").as_deref() != Ok("1") {
                for _ in 0..2 {
                    let Ok((t, v)) = plane_texture(dev, w, h * 3, true) else { break };
                    match share(&t, w, h * 3) {
                        Ok(s) => {
                            texs.push((t, v));
                            shared.push(s);
                        }
                        Err(e) => {
                            eprintln!("4:4:4: {e}; o quadro passa pela memória");
                            break;
                        }
                    }
                }
                if shared.len() < 2 {
                    shared.iter().for_each(|s| unshare(s));
                    shared.clear();
                    texs.clear();
                }
            }
            if texs.is_empty() {
                match plane_texture(dev, w, h * 3, false) {
                    Ok(x) => texs.push(x),
                    Err(e) => {
                        let _ = (api.cuvidDestroyVideoParser)(parser);
                        return Err(e);
                    }
                }
            }
            let direct = !shared.is_empty();
            st.gpu = shared.iter().map(|s| s.arr).collect();
            Ok(Self { api, ctx, parser, st, texs, shared, w, h, direct })
        }
    }

    /// Entrega um quadro do HEVC. `true` quando saiu uma imagem nova na textura.
    pub fn decode(&mut self, ctx: &ID3D11DeviceContext, data: &[u8], pts: i64) -> Result<bool> {
        let _scope = ContextScope::push(self.ctx).map_err(|e| cuda_err("usar a placa", e))?;
        unsafe {
            let mut pkt = CUVIDSOURCEDATAPACKET {
                flags: CUVID_PKT_ENDOFPICTURE | CUVID_PKT_TIMESTAMP,
                payload_size: data.len() as c_ulong,
                payload: data.as_ptr(),
                timestamp: pts,
            };
            ok("analisar", (self.api.cuvidParseVideoData)(self.parser, &mut pkt))?;
            if let Some(e) = self.st.failed.take() {
                return Err(Error::new(E_FAIL, format!("NVDEC: {e}")));
            }
            if !std::mem::take(&mut self.st.fresh) {
                return Ok(false);
            }
            if !self.direct {
                ctx.UpdateSubresource(&self.texs[0].0, 0, None, self.st.frame.ptr as *const c_void, self.st.pitch, 0);
            }
            Ok(true)
        }
    }
}

impl Nvdec {
    /// A textura com o quadro mais novo.
    pub fn srv(&self) -> &ID3D11ShaderResourceView {
        &self.texs[if self.direct { self.st.shown } else { 0 }].1
    }

    /// Y, U e V do último quadro neste ponto, lidos de volta da textura (para a bancada conferir a cor).
    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub fn yuv_at(&self, dev: &ID3D11Device, ctx: &ID3D11DeviceContext, x: u32, y: u32) -> Result<[u8; 3]> {
        let tex = &self.texs[if self.direct { self.st.shown } else { 0 }].0;
        unsafe {
            let td = D3D11_TEXTURE2D_DESC {
                Width: 1,
                Height: 1,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_R8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_STAGING,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                ..Default::default()
            };
            let mut out = [0u8; 3];
            for (k, o) in out.iter_mut().enumerate() {
                let mut st = None;
                dev.CreateTexture2D(&td, None, Some(&mut st))?;
                let st = st.unwrap();
                let row = y + k as u32 * self.h;
                let bx = D3D11_BOX { left: x, top: row, front: 0, right: x + 1, bottom: row + 1, back: 1 };
                ctx.CopySubresourceRegion(&st, 0, 0, 0, 0, tex, 0, Some(&bx));
                let mut m = D3D11_MAPPED_SUBRESOURCE::default();
                ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
                *o = *(m.pData as *const u8);
                ctx.Unmap(&st, 0);
            }
            Ok(out)
        }
    }
}

impl Drop for Nvdec {
    fn drop(&mut self) {
        let _scope = ContextScope::push(self.ctx);
        unsafe {
            let _ = (self.api.cuvidDestroyVideoParser)(self.parser);
            if !self.st.dec.is_null() {
                let _ = (self.api.cuvidDestroyDecoder)(self.st.dec);
            }
            self.st.frame.free();
            self.shared.iter().for_each(|s| unshare(s));
        }
    }
}

/// Os três planos empilhados viram RGB (BT.709, faixa limitada). Cada plano se lê só dentro dele
/// mesmo, para a filtragem não misturar a borda de um com a do outro.
const PS_444: &str = r#"
Texture2D tex : register(t0);
SamplerState smp : register(s0);
cbuffer C : register(b0) { float h; };
float plane(float2 t, float k) {
    float v = clamp(t.y, 0.5 / h, 1 - 0.5 / h);
    return tex.Sample(smp, float2(t.x, (k + v) / 3)).r;
}
float4 main(float4 p : SV_Position, float2 t : T) : SV_Target {
    float y = (plane(t, 0) * 255 - 16) / 219;
    float u = (plane(t, 1) * 255 - 128) / 224;
    float v = (plane(t, 2) * 255 - 128) / 224;
    return float4(saturate(float3(y + 1.5748 * v, y - 0.1873 * u - 0.4681 * v, y + 1.8556 * u)), 1);
}
"#;

/// Desenha o quadro 4:4:4 na janela, encaixado com faixas pretas.
pub struct Draw444 {
    vs: ID3D11VertexShader,
    ps: ID3D11PixelShader,
    smp: ID3D11SamplerState,
    cb: ID3D11Buffer,
}

impl Draw444 {
    pub fn new(dev: &ID3D11Device, h: u32) -> Result<Self> {
        unsafe {
            let vb = compile(VS, s!("vs_5_0"))?;
            let pb = compile(PS_444, s!("ps_5_0"))?;
            let (mut vs, mut ps) = (None, None);
            dev.CreateVertexShader(&vb, None, Some(&mut vs))?;
            dev.CreatePixelShader(&pb, None, Some(&mut ps))?;
            let sd = D3D11_SAMPLER_DESC {
                Filter: D3D11_FILTER_MIN_MAG_MIP_LINEAR,
                AddressU: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressV: D3D11_TEXTURE_ADDRESS_CLAMP,
                AddressW: D3D11_TEXTURE_ADDRESS_CLAMP,
                MaxLOD: f32::MAX,
                ..Default::default()
            };
            let mut smp = None;
            dev.CreateSamplerState(&sd, Some(&mut smp))?;
            let data = [h as f32, 0.0, 0.0, 0.0f32];
            let bd = D3D11_BUFFER_DESC {
                ByteWidth: 16,
                Usage: D3D11_USAGE_IMMUTABLE,
                BindFlags: D3D11_BIND_CONSTANT_BUFFER.0 as u32,
                ..Default::default()
            };
            let init = D3D11_SUBRESOURCE_DATA { pSysMem: data.as_ptr() as *const _, ..Default::default() };
            let mut cb = None;
            dev.CreateBuffer(&bd, Some(&init), Some(&mut cb))?;
            Ok(Self { vs: vs.unwrap(), ps: ps.unwrap(), smp: smp.unwrap(), cb: cb.unwrap() })
        }
    }

    pub fn draw(&self, dev: &ID3D11Device, ctx: &ID3D11DeviceContext, src: &Nvdec, target: &ID3D11Texture2D, ow: u32, oh: u32) -> Result<()> {
        unsafe {
            let mut rtv = None;
            dev.CreateRenderTargetView(target, None, Some(&mut rtv))?;
            let rtv = rtv.unwrap();
            ctx.ClearRenderTargetView(&rtv, &[0.0, 0.0, 0.0, 1.0]);
            let r = fit(src.w, src.h, ow, oh);
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
            ctx.PSSetShaderResources(0, Some(&[Some(src.srv().clone())]));
            ctx.PSSetSamplers(0, Some(&[Some(self.smp.clone())]));
            ctx.PSSetConstantBuffers(0, Some(&[Some(self.cb.clone())]));
            ctx.Draw(3, 0);
            ctx.PSSetShaderResources(0, Some(&[None]));
            ctx.OMSetRenderTargets(None, None);
            Ok(())
        }
    }
}

/// Retângulo do vídeo `iw`×`ih` encaixado em `ow`×`oh`, centralizado.
pub fn fit(iw: u32, ih: u32, ow: u32, oh: u32) -> RECT {
    let scale = (ow as f64 / iw as f64).min(oh as f64 / ih as f64);
    let (dw, dh) = ((iw as f64 * scale).round() as i32, (ih as f64 * scale).round() as i32);
    let (x, y) = ((ow as i32 - dw) / 2, (oh as i32 - dh) / 2);
    RECT { left: x, top: y, right: x + dw, bottom: y + dh }
}
