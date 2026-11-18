//! Codificador AMF direto, para transmitir em placas AMD.
//!
//! Como no NVENC direto, o Media Foundation da AMD não deixa escolher a memória do codificador (VBV)
//! nem o modo de latência ultrabaixa. Aqui o AMF é chamado pela DLL que vem com o driver
//! (`amfrt64.dll`, só da pasta do sistema), pela interface em C dos cabeçalhos públicos do AMF
//! (GPUOpen, MIT): cada objeto é um ponteiro para a tabela de funções, e as posições abaixo são as
//! dos `...Vtbl` de `core/Context.h`, `components/Component.h`, `core/Surface.h` e `core/Buffer.h`.
//! - uso de latência ultrabaixa, taxa constante e memória de um quadro;
//! - sem quadro B e quadro-chave só a cada minuto ou quando quem assiste pede;
//! - modo síncrono: cada quadro sai antes do próximo entrar, e a textura do conversor pode ser
//!   reaproveitada sem cópia.
//!
//! Qualquer falha aqui volta para o Media Foundation (veja `live_enc.rs`).

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::OnceLock;

use windows::core::{s, w, Error, Interface, Result, GUID, PCWSTR};
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

use super::mux::{Codec, Pkt};

type Res = i32;
const OK: Res = 0;
const EOF: Res = 23;
const REPEAT: Res = 24;
const INPUT_FULL: Res = 25;
/// `AMF_DX11_0` em `core/Data.h`.
const DX11_0: i32 = 110;
/// `AMF_SURFACE_NV12` em `core/Surface.h`.
const NV12: i32 = 1;
/// `IID_AMFBuffer` em `core/Buffer.h`.
const IID_BUFFER: GUID = GUID::from_values(0xb04b7248, 0xb6f0, 0x4321, [0xb6, 0x91, 0xba, 0xa4, 0x74, 0x0f, 0x9f, 0xcb]);

// Posições nas tabelas de funções (`AMFContextVtbl`, `AMFComponentVtbl`, `AMFSurfaceVtbl`, `AMFBufferVtbl`).
const RELEASE: usize = 1;
const QUERY_INTERFACE: usize = 2;
const SET_PROPERTY: usize = 3;
const GET_PROPERTY: usize = 4;
const CTX_TERMINATE: usize = 13;
const CTX_INIT_DX11: usize = 18;
const CTX_SURFACE_FROM_DX11: usize = 49;
// No Windows, `GetPropertyInfo` ocupa duas posições (14 e 15) antes destas.
const COMP_INIT: usize = 17;
const COMP_TERMINATE: usize = 19;
const COMP_SUBMIT: usize = 22;
const COMP_QUERY: usize = 23;
const DATA_SET_PTS: usize = 19;
const DATA_GET_PTS: usize = 20;
const BUF_GET_SIZE: usize = 24;
const BUF_GET_NATIVE: usize = 25;
// `AMFFactoryVtbl` não tem contagem de referências: começa direto nestas.
const FACTORY_CREATE_CONTEXT: usize = 0;
const FACTORY_CREATE_COMPONENT: usize = 1;

/// `AMFVariantStruct`: o tipo e uma união de 16 bytes alinhada em 8.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Variant {
    ty: i32,
    v: [u64; 2],
}

impl Variant {
    const fn boolean(b: bool) -> Self {
        Variant { ty: 1, v: [b as u64, 0] }
    }
    const fn int(i: i64) -> Self {
        Variant { ty: 2, v: [i as u64, 0] }
    }
    /// `AMFSize` e `AMFRate`: dois inteiros de 32 bits, o primeiro embaixo.
    const fn pair(ty: i32, a: u32, b: u32) -> Self {
        Variant { ty, v: [a as u64 | (b as u64) << 32, 0] }
    }
}

/// A função na posição `i` da tabela do objeto.
unsafe fn slot<F: Copy>(obj: *mut c_void, i: usize) -> F {
    let table = *(obj as *const *const usize);
    std::mem::transmute_copy(&*table.add(i))
}

unsafe fn set(obj: *mut c_void, name: PCWSTR, v: Variant) -> Res {
    slot::<unsafe extern "system" fn(*mut c_void, PCWSTR, Variant) -> Res>(obj, SET_PROPERTY)(obj, name, v)
}

unsafe fn get(obj: *mut c_void, name: PCWSTR) -> Option<Variant> {
    let mut v = Variant::default();
    (slot::<unsafe extern "system" fn(*mut c_void, PCWSTR, *mut Variant) -> Res>(obj, GET_PROPERTY)(obj, name, &mut v) == OK).then_some(v)
}

unsafe fn release(obj: *mut c_void) {
    if !obj.is_null() {
        slot::<unsafe extern "system" fn(*mut c_void) -> i32>(obj, RELEASE)(obj);
    }
}

fn fail(what: &str, r: Res) -> Error {
    Error::new(E_FAIL, format!("AMF: {what} falhou ({r})"))
}

/// `AMFInit` com a versão do runtime instalado, carregado uma vez só.
fn factory() -> Result<*mut c_void> {
    static F: OnceLock<usize> = OnceLock::new();
    let f = *F.get_or_init(|| unsafe {
        let Ok(m) = LoadLibraryExW(w!("amfrt64.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32) else { return 0 };
        let (Some(query), Some(init)) = (GetProcAddress(m, s!("AMFQueryVersion")), GetProcAddress(m, s!("AMFInit"))) else { return 0 };
        let query: unsafe extern "C" fn(*mut u64) -> Res = std::mem::transmute(query);
        let init: unsafe extern "C" fn(u64, *mut *mut c_void) -> Res = std::mem::transmute(init);
        let mut ver = 0u64;
        let mut fac = std::ptr::null_mut();
        if query(&mut ver) != OK || init(ver, &mut fac) != OK {
            return 0;
        }
        fac as usize
    });
    if f == 0 {
        Err(Error::new(E_FAIL, "sem AMF (amfrt64.dll do driver da AMD)"))
    } else {
        Ok(f as *mut c_void)
    }
}

/// Nomes e valores das propriedades de cada codec (`components/VideoEncoderVCE.h`, `...HEVC.h`, `...AV1.h`).
struct Names {
    id: PCWSTR,
    usage: (PCWSTR, i64),
    size: PCWSTR,
    rate: PCWSTR,
    target: PCWSTR,
    peak: PCWSTR,
    vbv: PCWSTR,
    vbv_full: PCWSTR,
    rc: (PCWSTR, i64),
    preset: (PCWSTR, i64),
    hrd: PCWSTR,
    gop: PCWSTR,
    timeout: PCWSTR,
    force: (PCWSTR, i64),
    /// Cabeçalhos (SPS, PPS, sequência) que vão junto com o quadro-chave pedido.
    headers: &'static [PCWSTR],
    /// Tipo do quadro que saiu: 0 e 1 são IDR e I no H.264 e no HEVC, chave e só-intra no AV1.
    out_type: PCWSTR,
    /// Ajustes que nem toda placa aceita: se recusar, segue sem.
    extra: &'static [(PCWSTR, Variant)],
}

fn names(c: Codec) -> Names {
    match c {
        Codec::H264 => Names {
            id: w!("AMFVideoEncoderVCE_AVC"),
            usage: (w!("Usage"), 1),
            size: w!("FrameSize"),
            rate: w!("FrameRate"),
            target: w!("TargetBitrate"),
            peak: w!("PeakBitrate"),
            vbv: w!("VBVBufferSize"),
            vbv_full: w!("InitialVBVBufferFullness"),
            rc: (w!("RateControlMethod"), 1),
            preset: (w!("QualityPreset"), 1),
            hrd: w!("EnforceHRD"),
            gop: w!("IDRPeriod"),
            timeout: w!("QueryTimeout"),
            force: (w!("ForcePictureType"), 2),
            headers: const { &[w!("InsertSPS"), w!("InsertPPS")] },
            out_type: w!("OutputDataType"),
            extra: const { &[(w!("BPicturesPattern"), Variant::int(0)), (w!("LowLatencyInternal"), Variant::boolean(true)), (w!("FillerDataEnable"), Variant::boolean(false))] },
        },
        Codec::Hevc => Names {
            id: w!("AMFVideoEncoderHW_HEVC"),
            usage: (w!("HevcUsage"), 1),
            size: w!("HevcFrameSize"),
            rate: w!("HevcFrameRate"),
            target: w!("HevcTargetBitrate"),
            peak: w!("HevcPeakBitrate"),
            vbv: w!("HevcVBVBufferSize"),
            vbv_full: w!("HevcInitialVBVBufferFullness"),
            rc: (w!("HevcRateControlMethod"), 3),
            preset: (w!("HevcQualityPreset"), 10),
            hrd: w!("HevcEnforceHRD"),
            gop: w!("HevcGOPSize"),
            timeout: w!("HevcQueryTimeout"),
            force: (w!("HevcForcePictureType"), 2),
            headers: const { &[w!("HevcInsertHeader")] },
            out_type: w!("HevcOutputDataType"),
            extra: const { &[(w!("HevcGOPSPerIDR"), Variant::int(1)), (w!("LowLatencyInternal"), Variant::boolean(true))] },
        },
        Codec::Av1 => Names {
            id: w!("AMFVideoEncoderHW_AV1"),
            usage: (w!("Av1Usage"), 2),
            size: w!("Av1FrameSize"),
            rate: w!("Av1FrameRate"),
            target: w!("Av1TargetBitrate"),
            peak: w!("Av1PeakBitrate"),
            vbv: w!("Av1VBVBufferSize"),
            vbv_full: w!("Av1InitialVBVBufferFullness"),
            rc: (w!("Av1RateControlMethod"), 3),
            preset: (w!("Av1QualityPreset"), 100),
            hrd: w!("Av1EnforceHRD"),
            gop: w!("Av1GOPSize"),
            timeout: w!("Av1QueryTimeout"),
            force: (w!("Av1ForceFrameType"), 1),
            headers: const { &[w!("Av1ForceInsertSequenceHeader")] },
            out_type: w!("Av1OutputFrameType"),
            // Latência: o mais rápido possível (3). Alinhamento sem restrição (3): 1080 não vira 1082.
            extra: const { &[(w!("Av1EncodingLatencyMode"), Variant::int(3)), (w!("Av1AlignmentMode"), Variant::int(3))] },
        },
    }
}

pub struct Amf {
    ctx: *mut c_void,
    comp: *mut c_void,
    n: Names,
    fps: u32,
    force: Cell<bool>,
    ready: Vec<Pkt>,
    pub name: String,
}

impl Amf {
    /// Abre o codificador da AMD para NV12 de 8 bits (HDR10 e 4:4:4 ficam com o NVENC).
    pub unsafe fn new(dev: &ID3D11Device, codec: Codec, w: u32, h: u32, fps: u32, bitrate: u32) -> Result<Self> {
        let fac = factory()?;
        let mut me = Amf { ctx: std::ptr::null_mut(), comp: std::ptr::null_mut(), n: names(codec), fps: fps.max(1), force: Cell::new(false), ready: Vec::new(), name: String::new() };
        // Daqui em diante, um erro solta o que já foi aberto pelo Drop.
        let r = slot::<unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Res>(fac, FACTORY_CREATE_CONTEXT)(fac, &mut me.ctx);
        if r != OK {
            return Err(fail("criar o contexto", r));
        }
        let r = slot::<unsafe extern "system" fn(*mut c_void, *mut c_void, i32) -> Res>(me.ctx, CTX_INIT_DX11)(me.ctx, dev.as_raw(), DX11_0);
        if r != OK {
            return Err(fail("abrir o Direct3D 11", r));
        }
        let r = slot::<unsafe extern "system" fn(*mut c_void, *mut c_void, PCWSTR, *mut *mut c_void) -> Res>(fac, FACTORY_CREATE_COMPONENT)(fac, me.ctx, me.n.id, &mut me.comp);
        if r != OK {
            return Err(fail("criar o codificador (a placa não codifica este codec?)", r));
        }
        let (c, n) = (me.comp, &me.n);
        // O uso vem primeiro: ele redefine todo o resto.
        let must = [
            ("uso de latência ultrabaixa", n.usage.0, Variant::int(n.usage.1)),
            ("tamanho", n.size, Variant::pair(5, w, h)),
            ("quadros por segundo", n.rate, Variant::pair(7, fps, 1)),
            ("taxa constante", n.rc.0, Variant::int(n.rc.1)),
            ("taxa", n.target, Variant::int(bitrate as i64)),
            ("pico", n.peak, Variant::int(bitrate as i64)),
            // Um quadro de memória, como no NVENC: nenhum quadro fica muito maior que a média.
            ("memória de um quadro", n.vbv, Variant::int((bitrate / me.fps) as i64)),
        ];
        for (what, key, v) in must {
            let r = set(c, key, v);
            if r != OK {
                return Err(fail(what, r));
            }
        }
        let _ = set(c, n.vbv_full, Variant::int(64));
        let _ = set(c, n.preset.0, Variant::int(n.preset.1));
        let _ = set(c, n.hrd, Variant::boolean(true));
        // Um quadro-chave a cada minuto; quem recebe pede outro quando perde algo.
        let _ = set(c, n.gop, Variant::int((me.fps * 60) as i64));
        // `QueryOutput` espera até 100 ms pelo quadro em vez de voltar na hora.
        let _ = set(c, n.timeout, Variant::int(100));
        for &(key, v) in n.extra {
            let _ = set(c, key, v);
        }
        let r = slot::<unsafe extern "system" fn(*mut c_void, i32, i32, i32) -> Res>(c, COMP_INIT)(c, NV12, w as i32, h as i32);
        if r != OK {
            return Err(fail("iniciar o codificador", r));
        }
        me.name = "AMF direto".into();
        Ok(me)
    }

    /// Codifica um quadro NV12 do tamanho combinado e espera o pacote dele (sai em `pump`).
    pub unsafe fn feed(&mut self, tex: &ID3D11Texture2D, pts: i64) -> Result<()> {
        let mut surf = std::ptr::null_mut();
        let r = slot::<unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void, *mut c_void) -> Res>(self.ctx, CTX_SURFACE_FROM_DX11)(self.ctx, tex.as_raw(), &mut surf, std::ptr::null_mut());
        if r != OK {
            return Err(fail("usar a textura", r));
        }
        slot::<unsafe extern "system" fn(*mut c_void, i64)>(surf, DATA_SET_PTS)(surf, pts);
        if self.force.replace(false) {
            let _ = set(surf, self.n.force.0, Variant::int(self.n.force.1));
            for &hdr in self.n.headers {
                let _ = set(surf, hdr, Variant::boolean(true));
            }
        }
        let submit = slot::<unsafe extern "system" fn(*mut c_void, *mut c_void) -> Res>(self.comp, COMP_SUBMIT);
        let mut r = submit(self.comp, surf);
        // Fila cheia: tira o que está pronto e tenta de novo.
        for _ in 0..20 {
            if r != INPUT_FULL {
                break;
            }
            if let Err(e) = self.take() {
                release(surf);
                return Err(e);
            }
            r = submit(self.comp, surf);
        }
        release(surf);
        if r != OK {
            return Err(fail("codificar", r));
        }
        // Espera o quadro sair: a textura do conversor é reaproveitada no próximo.
        for _ in 0..5 {
            if self.take()? {
                break;
            }
        }
        Ok(())
    }

    /// Lê um pacote pronto. Falso se ainda não havia nenhum.
    unsafe fn take(&mut self) -> Result<bool> {
        let mut data = std::ptr::null_mut();
        let r = slot::<unsafe extern "system" fn(*mut c_void, *mut *mut c_void) -> Res>(self.comp, COMP_QUERY)(self.comp, &mut data);
        if r == REPEAT || r == EOF || (r == OK && data.is_null()) {
            return Ok(false);
        }
        if r != OK {
            return Err(fail("ler o pacote", r));
        }
        let mut buf = std::ptr::null_mut();
        let q = slot::<unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> Res>(data, QUERY_INTERFACE)(data, &IID_BUFFER, &mut buf);
        if q == OK && !buf.is_null() {
            let size = slot::<unsafe extern "system" fn(*mut c_void) -> usize>(buf, BUF_GET_SIZE)(buf);
            let ptr = slot::<unsafe extern "system" fn(*mut c_void) -> *mut c_void>(buf, BUF_GET_NATIVE)(buf);
            let pts = slot::<unsafe extern "system" fn(*mut c_void) -> i64>(data, DATA_GET_PTS)(data);
            // Só IDR (H.264/HEVC) ou KEY (AV1), os dois 0: um I sem IDR não abre o vídeo para quem chega.
            let key = get(buf, self.n.out_type).is_some_and(|v| v.v[0] as i64 == 0);
            if !ptr.is_null() && size > 0 {
                self.ready.push(Pkt { data: std::slice::from_raw_parts(ptr as *const u8, size).to_vec(), pts, key });
            }
            release(buf);
        }
        release(data);
        Ok(true)
    }

    pub fn pump(&mut self, out: &mut Vec<Pkt>) {
        out.append(&mut self.ready);
    }

    /// O próximo quadro sai como quadro-chave, com os cabeçalhos.
    pub fn force_key(&self) {
        self.force.set(true);
    }

    /// Troca a taxa durante a transmissão. Falso se o AMF recusou.
    pub unsafe fn set_bitrate(&mut self, bitrate: u32) -> bool {
        let c = self.comp;
        let _ = set(c, self.n.vbv, Variant::int((bitrate / self.fps) as i64));
        let _ = set(c, self.n.peak, Variant::int(bitrate as i64));
        set(c, self.n.target, Variant::int(bitrate as i64)) == OK
    }
}

impl Drop for Amf {
    fn drop(&mut self) {
        unsafe {
            if !self.comp.is_null() {
                slot::<unsafe extern "system" fn(*mut c_void) -> Res>(self.comp, COMP_TERMINATE)(self.comp);
                release(self.comp);
            }
            if !self.ctx.is_null() {
                slot::<unsafe extern "system" fn(*mut c_void) -> Res>(self.ctx, CTX_TERMINATE)(self.ctx);
                release(self.ctx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variante_tem_o_layout_do_c() {
        // AMFVariantStruct: tipo em 0, união de 16 bytes em 8, 24 bytes no total.
        assert_eq!(std::mem::size_of::<Variant>(), 24);
        assert_eq!(std::mem::offset_of!(Variant, v), 8);
        // AMFSize {width, height}: a largura vem primeiro na memória.
        let v = Variant::pair(5, 1920, 1080);
        assert_eq!(v.v[0].to_le_bytes(), [0x80, 0x07, 0, 0, 0x38, 0x04, 0, 0]);
    }
}
