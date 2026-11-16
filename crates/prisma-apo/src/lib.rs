//! APO do equalizador do Prisma (seções 2.2 e 2.3 de `docs/estudo-equalizador.md`).
//!
//! Aplica o perfil de `perfil-<guid do aparelho>.txt` ou `perfil.txt` (pasta do log) com o
//! `prisma-eq`; sem perfil ou com erro nele, passa o som sem mexer. Anota em `C:\ProgramData\Prisma\eq\apo.log` tudo que o motor de áudio
//! entrega: a estrutura de `Initialize` (versão, modo, aparelho e todas as propriedades), os
//! formatos pedidos e o que passou em cada instância. Responde:
//! carrega? (há log), uma instância por fluxo? (instâncias SFX), dá para saber o programa?
//! (propriedades de cada instância), qual formato chega? (`LockForProcess`).
//!
//! Regras de tempo real: `APOProcess` não aloca, não trava e não escreve no log; só soma contadores.
//! Toda entrada passa por `guard`: um pânico vira "erro" ou "passar o som", nunca derruba o audiodg.

#![allow(non_snake_case)]
// Os métodos COM recebem ponteiros crus por contrato; a assinatura vem do trait e não pode ser `unsafe`.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

mod live;

use std::ffi::c_void;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

use windows::Win32::Foundation::{CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_FAIL, E_INVALIDARG, E_POINTER, HANDLE, PROPERTYKEY, S_FALSE};
use windows::Win32::Media::Audio::Apo::*;
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CoTaskMemAlloc, CoTaskMemFree, IClassFactory, IClassFactory_Impl};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, PSGetNameFromPropertyKey};
use windows_core::{BOOL, GUID, HRESULT, IUnknown, Interface, PWSTR, Ref, Result, implement};

/// Classe COM do APO de teste. A do produto vai ter outra.
pub const CLSID: GUID = GUID::from_u128(0x6f1e3a52_8c47_4d0b_9a3e_2b5c7d9e1f40);

static NEXT_ID: AtomicU32 = AtomicU32::new(1);

// ---------------------------------------------------------------------------------------------
// Log (só fora do tempo real)

fn log_path() -> PathBuf {
    // O teste dentro do processo troca o caminho para não sujar o ProgramData.
    if let Some(p) = std::env::var_os("PRISMA_APO_LOG") {
        return p.into();
    }
    let base = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    base.join(r"Prisma\eq\apo.log")
}

fn log(line: &str) {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let (pid, tid) = unsafe { (GetCurrentProcessId(), GetCurrentThreadId()) };
    let text = format!("{ms} pid={pid} tid={tid} {line}
");
    // O audiodg pode não ter escrita no ProgramData; nesse caso vai para a pasta temporária do serviço
    // (C:\Windows\ServiceProfiles\LocalService\AppData\Local\Temp\prisma-apo.log).
    for path in [log_path(), std::env::temp_dir().join("prisma-apo.log")] {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path)
            && f.write_all(text.as_bytes()).is_ok()
        {
            return;
        }
    }
}

fn guard<T>(fallback: T, what: &str, f: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            log(&format!("PÂNICO em {what}"));
            fallback
        }
    }
}

fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const c_void)) };
    s
}

fn key_name(k: &PROPERTYKEY) -> String {
    match unsafe { PSGetNameFromPropertyKey(k) } {
        Ok(p) => take_pwstr(p),
        Err(_) => format!("{:?},{}", k.fmtid, k.pid),
    }
}

/// Todas as propriedades de um `IPropertyStore`, uma por linha (valores longos cortados).
fn dump_store(title: &str, store: Option<&IPropertyStore>) {
    let Some(store) = store else {
        log(&format!("  {title}: nenhum"));
        return;
    };
    let n = unsafe { store.GetCount() }.unwrap_or(0);
    log(&format!("  {title}: {n} propriedades"));
    for i in 0..n.min(400) {
        let mut key = PROPERTYKEY::default();
        if unsafe { store.GetAt(i, &mut key) }.is_err() {
            continue;
        }
        let value = match unsafe { store.GetValue(&key) } {
            Ok(v) => match unsafe { PropVariantToStringAlloc(&v) } {
                Ok(s) => take_pwstr(s),
                Err(_) => format!("<{:?}>", v.vt()),
            },
            Err(e) => format!("<erro {e}>"),
        };
        let value: String = value.chars().take(300).collect();
        log(&format!("    {} = {}", key_name(&key), value));
    }
}

fn devices(c: Option<&windows::Win32::Media::Audio::IMMDeviceCollection>) -> String {
    let Some(c) = c else { return "nenhum".into() };
    let n = unsafe { c.GetCount() }.unwrap_or(0);
    (0..n).map(|i| unsafe { c.Item(i) }.and_then(|d| unsafe { d.GetId() }).map(take_pwstr).unwrap_or_else(|e| format!("<erro {e}>"))).collect::<Vec<_>>().join(" | ")
}

/// Formato sem compressão do tipo de mídia: (tipo é float?, canais, bytes por amostra, taxa, máscara).
fn format_of(t: Option<&IAudioMediaType>) -> Option<(bool, u32, u32, f32, u32)> {
    let t = t?;
    let mut u = UNCOMPRESSEDAUDIOFORMAT::default();
    unsafe { t.GetUncompressedAudioFormat(&mut u) }.ok()?;
    Some((u.guidFormatType == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, u.dwSamplesPerFrame, u.dwBytesPerSampleContainer, u.fFramesPerSecond, u.dwChannelMask))
}

fn format_text(t: Option<&IAudioMediaType>) -> String {
    match format_of(t) {
        Some((float, ch, bytes, rate, mask)) => format!("{} {ch} canais {} bits {rate} Hz máscara 0x{mask:X}", if float { "float" } else { "inteiro" }, bytes * 8),
        None => "nenhum".into(),
    }
}

// ---------------------------------------------------------------------------------------------
// O APO

/// Estado de uma instância. O objeto COM é o `Agg` (mais abaixo), que o motor de áudio cria agregado.
pub struct Apo {
    id: u32,
    channels: AtomicU32,
    frame_bytes: AtomicU32,
    calls: AtomicU64,
    frames: AtomicU64,
    silent: AtomicU64,
    copied: AtomicU64,
    /// GUID do aparelho (o fim do id do endpoint), lido em `Initialize`.
    device: Mutex<String>,
    /// Equalizador do fluxo, com troca ao vivo do perfil.
    eq: live::Live,
}

impl Apo {
    pub fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Relaxed);
        log(&format!("instância {id} criada"));
        Apo { id, channels: AtomicU32::new(0), frame_bytes: AtomicU32::new(0), calls: AtomicU64::new(0), frames: AtomicU64::new(0), silent: AtomicU64::new(0), copied: AtomicU64::new(0), device: Mutex::default(), eq: live::Live::default() }
    }

    fn counters(&self) -> String {
        format!("chamadas={} quadros={} silenciosos={} copias={}", self.calls.load(Relaxed), self.frames.load(Relaxed), self.silent.load(Relaxed), self.copied.load(Relaxed))
    }

    /// Anota os aparelhos e guarda o GUID do endpoint (`{0.0.0.00000000}.{guid}` → `guid`).
    fn remember_device(&self, list: &str) {
        log(&format!("  aparelhos: {list}"));
        let guid = list.split(" | ").find(|d| d.starts_with("{0.0.")).and_then(|d| d.rsplit_once('.')).map(|(_, g)| g.trim_matches(['{', '}']).to_ascii_lowercase());
        if let (Some(g), Ok(mut d)) = (guid, self.device.lock()) {
            *d = g;
        }
    }

    /// Aceita só float, e o mesmo formato do outro lado (o APO trabalha no lugar, sem converter).
    fn supported(&self, side: &str, opposite: Option<&IAudioMediaType>, requested: Option<&IAudioMediaType>) -> Result<IAudioMediaType> {
        let req = requested.ok_or(E_POINTER)?;
        let float = format_of(Some(req)).is_some_and(|f| f.0);
        // Mesma taxa, canais e tamanho de amostra do outro lado (o APO só copia); máscara e bits válidos
        // podem diferir sem atrapalhar a cópia.
        let same = match (opposite, format_of(Some(req))) {
            (None, _) => true,
            (Some(o), Some((_, ch, bytes, rate, _))) => format_of(Some(o)).is_some_and(|(of, och, obytes, orate, _)| of && och == ch && obytes == bytes && orate == rate),
            _ => false,
        };
        let ok = float && same;
        log(&format!("instância {} {side}: pedido [{}] outro lado [{}] → {}", self.id, format_text(Some(req)), format_text(opposite), if ok { "aceito" } else { "recusado" }));
        if ok { Ok(req.clone()) } else { Err(APOERR_FORMAT_NOT_SUPPORTED.into()) }
    }
}

impl Default for Apo {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Apo {
    fn drop(&mut self) {
        log(&format!("instância {} destruída: {}", self.id, self.counters()));
    }
}

impl IAudioProcessingObject_Impl for Agg {
    fn Reset(&self) -> Result<()> {
        Ok(())
    }

    fn GetLatency(&self) -> Result<i64> {
        Ok(0)
    }

    fn GetRegistrationProperties(&self) -> Result<*mut APO_REG_PROPERTIES> {
        guard(Err(E_FAIL.into()), "GetRegistrationProperties", || {
            let p = unsafe { CoTaskMemAlloc(std::mem::size_of::<APO_REG_PROPERTIES>()) } as *mut APO_REG_PROPERTIES;
            if p.is_null() {
                return Err(windows::Win32::Foundation::E_OUTOFMEMORY.into());
            }
            let mut r = APO_REG_PROPERTIES { clsid: CLSID, Flags: APO_FLAG_DEFAULT, u32MajorVersion: 0, u32MinorVersion: 1, u32MinInputConnections: 1, u32MaxInputConnections: 1, u32MinOutputConnections: 1, u32MaxOutputConnections: 1, u32MaxInstances: u32::MAX, u32NumAPOInterfaces: 1, ..Default::default() };
            r.iidAPOInterfaceList[0] = IAudioSystemEffects2::IID;
            for (dst, s) in [(&mut r.szFriendlyName, "Prisma EQ (teste)"), (&mut r.szCopyrightInfo, "Prisma")] {
                for (d, c) in dst.iter_mut().zip(s.encode_utf16()) {
                    *d = c;
                }
            }
            unsafe { p.write(r) };
            Ok(p)
        })
    }

    fn Initialize(&self, cbdatasize: u32, pbydata: *const u8) -> Result<()> {
        guard(Err(E_FAIL.into()), "Initialize", || {
            let size = cbdatasize as usize;
            if pbydata.is_null() || size < std::mem::size_of::<APOInitBaseStruct>() {
                log(&format!("instância {} Initialize sem dados ({size} bytes)", self.id));
                return if pbydata.is_null() && size == 0 { Ok(()) } else { Err(E_INVALIDARG.into()) };
            }
            let base = unsafe { &*(pbydata as *const APOInitBaseStruct) };
            log(&format!("instância {} Initialize: {size} bytes, cbSize={} clsid={:?}", self.id, base.cbSize, base.clsid));
            // Os campos de interface são lidos por referência: quem é dono é o motor de áudio.
            if size == std::mem::size_of::<APOInitSystemEffects3>() {
                let d = unsafe { &*(pbydata as *const APOInitSystemEffects3) };
                log(&format!("  versão 3: modo={:?} só descoberta={} aparelho no conjunto={} conector={} serviços={}", d.AudioProcessingMode, d.InitializeForDiscoveryOnly.as_bool(), d.nSoftwareIoDeviceInCollection, d.nSoftwareIoConnectorIndex, d.pServiceProvider.is_some()));
                self.remember_device(&devices(d.pDeviceCollection.as_ref()));
                dump_store("propriedades do aparelho", d.pAPOEndpointProperties.as_ref());
            } else if size == std::mem::size_of::<APOInitSystemEffects2>() {
                let d = unsafe { &*(pbydata as *const APOInitSystemEffects2) };
                log(&format!("  versão 2: modo={:?} só descoberta={} aparelho no conjunto={} conector={}", d.AudioProcessingMode, d.InitializeForDiscoveryOnly.as_bool(), d.nSoftwareIoDeviceInCollection, d.nSoftwareIoConnectorIndex));
                self.remember_device(&devices(d.pDeviceCollection.as_ref()));
                dump_store("propriedades do aparelho", d.pAPOEndpointProperties.as_ref());
                dump_store("propriedades de efeitos", d.pAPOSystemEffectsProperties.as_ref());
            } else if size == std::mem::size_of::<APOInitSystemEffects>() {
                let d = unsafe { &*(pbydata as *const APOInitSystemEffects) };
                log("  versão 1");
                log(&format!("  aparelhos: {}", devices(d.pDeviceCollection.as_ref())));
                dump_store("propriedades do aparelho", d.pAPOEndpointProperties.as_ref());
                dump_store("propriedades de efeitos", d.pAPOSystemEffectsProperties.as_ref());
            } else {
                log("  tamanho desconhecido: nada lido além do cabeçalho");
            }
            Ok(())
        })
    }

    fn IsInputFormatSupported(&self, poppositeformat: Ref<IAudioMediaType>, prequestedinputformat: Ref<IAudioMediaType>) -> Result<IAudioMediaType> {
        guard(Err(E_FAIL.into()), "IsInputFormatSupported", || self.supported("entrada", poppositeformat.as_ref(), prequestedinputformat.as_ref()))
    }

    fn IsOutputFormatSupported(&self, poppositeformat: Ref<IAudioMediaType>, prequestedoutputformat: Ref<IAudioMediaType>) -> Result<IAudioMediaType> {
        guard(Err(E_FAIL.into()), "IsOutputFormatSupported", || self.supported("saída", poppositeformat.as_ref(), prequestedoutputformat.as_ref()))
    }

    fn GetInputChannelCount(&self) -> Result<u32> {
        Ok(self.channels.load(Relaxed))
    }
}

impl IAudioProcessingObjectConfiguration_Impl for Agg {
    fn LockForProcess(&self, nin: u32, ins: *const *const APO_CONNECTION_DESCRIPTOR, nout: u32, outs: *const *const APO_CONNECTION_DESCRIPTOR) -> Result<()> {
        guard(Err(E_FAIL.into()), "LockForProcess", || {
            if nin != 1 || nout != 1 || ins.is_null() || outs.is_null() {
                log(&format!("instância {} LockForProcess recusado: {nin} entradas, {nout} saídas", self.id));
                return Err(E_INVALIDARG.into());
            }
            let (i, o) = unsafe { (&**ins, &**outs) };
            let Some((float, ch, bytes, rate, mask)) = format_of(i.pFormat.as_ref()) else {
                log(&format!("instância {} LockForProcess sem formato", self.id));
                return Err(E_INVALIDARG.into());
            };
            if !float {
                return Err(APOERR_FORMAT_NOT_SUPPORTED.into());
            }
            self.channels.store(ch, Relaxed);
            self.frame_bytes.store(ch * bytes, Relaxed);
            log(&format!(
                "instância {} LockForProcess: entrada [{}] tipo {} até {} quadros; saída [{}] tipo {} até {} quadros; mesmo buffer={}",
                self.id,
                format_text(i.pFormat.as_ref()),
                i.Type.0,
                i.u32MaxFrameCount,
                format_text(o.pFormat.as_ref()),
                o.Type.0,
                o.u32MaxFrameCount,
                i.pBuffer == o.pBuffer
            ));
            let device = self.device.lock().map(|d| d.clone()).unwrap_or_default();
            let dir = log_path().parent().map(PathBuf::from).unwrap_or_default();
            self.eq.start(live::Params { id: self.id, dir, device, mask, ch, rate }, i.u32MaxFrameCount.max(o.u32MaxFrameCount));
            Ok(())
        })
    }

    fn UnlockForProcess(&self) -> Result<()> {
        log(&format!("instância {} UnlockForProcess: {}", self.id, self.counters()));
        self.eq.stop();
        Ok(())
    }
}

impl IAudioProcessingObjectRT_Impl for Agg {
    fn APOProcess(&self, nin: u32, ins: *const *const APO_CONNECTION_PROPERTY, nout: u32, outs: *mut *mut APO_CONNECTION_PROPERTY) {
        // Tempo real: nada de log, alocação ou trava aqui.
        guard((), "APOProcess", || {
            if nin < 1 || nout < 1 || ins.is_null() || outs.is_null() {
                return;
            }
            let (i, o) = unsafe { (&**ins, &mut **outs) };
            let frames = i.u32ValidFrameCount;
            let silent = i.u32BufferFlags == BUFFER_SILENT;
            if !silent && i.pBuffer != o.pBuffer && i.pBuffer != 0 && o.pBuffer != 0 {
                let n = frames as usize * self.frame_bytes.load(Relaxed) as usize;
                unsafe { std::ptr::copy_nonoverlapping(i.pBuffer as *const u8, o.pBuffer as *mut u8, n) };
                self.copied.fetch_add(1, Relaxed);
            }
            // Equalizador no buffer de saída (já com a cópia).
            if !silent && o.pBuffer != 0 {
                let n = frames as usize * self.channels.load(Relaxed) as usize;
                self.eq.process(unsafe { std::slice::from_raw_parts_mut(o.pBuffer as *mut f32, n) });
            }
            o.u32ValidFrameCount = frames;
            o.u32BufferFlags = i.u32BufferFlags;
            self.calls.fetch_add(1, Relaxed);
            self.frames.fetch_add(frames as u64, Relaxed);
            if silent {
                self.silent.fetch_add(1, Relaxed);
            }
        })
    }

    fn CalcInputFrames(&self, u32outputframecount: u32) -> u32 {
        u32outputframecount
    }

    fn CalcOutputFrames(&self, u32inputframecount: u32) -> u32 {
        u32inputframecount
    }
}

impl IAudioSystemEffects_Impl for Agg {}

impl IAudioSystemEffects2_Impl for Agg {
    fn GetEffectsList(&self, ppeffectsids: *mut *mut GUID, pceffects: *mut u32, _event: HANDLE) -> Result<()> {
        if ppeffectsids.is_null() || pceffects.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *ppeffectsids = std::ptr::null_mut();
            *pceffects = 0;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Objeto COM agregável. O motor de áudio cria o APO agregado (CreateInstance com um IUnknown de fora),
// e o `#[implement]` do windows-rs não suporta isso. Aqui: um IUnknown "de dentro" (não delegante) feito à
// mão, e as quatro interfaces do APO com as tabelas do windows-rs, cujo IUnknown delega para o de fora.

#[repr(C)]
pub struct Agg {
    unk: *const windows_core::IUnknown_Vtbl,
    apo: *const IAudioProcessingObject_Vtbl,
    rt: *const IAudioProcessingObjectRT_Vtbl,
    cfg: *const IAudioProcessingObjectConfiguration_Vtbl,
    sfx: *const IAudioSystemEffects2_Vtbl,
    /// IUnknown que manda: o de fora quando agregado, senão o nosso `unk`. Sem AddRef (regra do COM).
    outer: *mut c_void,
    refs: AtomicU32,
    state: Apo,
}

// Os OFFSETs contam ponteiros do campo da interface até o começo do `Agg`.
static UNK_VTBL: windows_core::IUnknown_Vtbl = windows_core::IUnknown_Vtbl { QueryInterface: inner_qi, AddRef: inner_addref, Release: inner_release };
static APO_VTBL: IAudioProcessingObject_Vtbl = IAudioProcessingObject_Vtbl::new::<Agg, -1>();
static RT_VTBL: IAudioProcessingObjectRT_Vtbl = IAudioProcessingObjectRT_Vtbl::new::<Agg, -2>();
static CFG_VTBL: IAudioProcessingObjectConfiguration_Vtbl = IAudioProcessingObjectConfiguration_Vtbl::new::<Agg, -3>();
static SFX_VTBL: IAudioSystemEffects2_Vtbl = IAudioSystemEffects2_Vtbl::new::<Agg, -4>();

unsafe impl Send for Agg {}
unsafe impl Sync for Agg {}

impl std::ops::Deref for Agg {
    type Target = Apo;
    fn deref(&self) -> &Apo {
        &self.state
    }
}

impl Agg {
    /// Cria a instância e devolve `riid` pelo IUnknown de dentro (o de fora pede `IUnknown` ao agregar).
    unsafe fn create(outer: *mut c_void, riid: *const GUID, out: *mut *mut c_void) -> HRESULT {
        let p = Box::into_raw(Box::new(Agg { unk: &UNK_VTBL, apo: &APO_VTBL, rt: &RT_VTBL, cfg: &CFG_VTBL, sfx: &SFX_VTBL, outer, refs: AtomicU32::new(1), state: Apo::new() }));
        unsafe {
            if outer.is_null() {
                (*p).outer = p as *mut c_void;
            }
            log(&format!("instância {} {}", (*p).state.id, if outer.is_null() { "sozinha" } else { "agregada" }));
            let inner = p as *mut c_void;
            let hr = inner_qi(inner, riid, out);
            inner_release(inner);
            hr
        }
    }

    unsafe fn outer_vtbl(&self) -> &windows_core::IUnknown_Vtbl {
        unsafe { &**(self.outer as *const *const windows_core::IUnknown_Vtbl) }
    }
}

unsafe extern "system" fn inner_qi(this: *mut c_void, iid: *const GUID, out: *mut *mut c_void) -> HRESULT {
    if iid.is_null() || out.is_null() {
        return E_POINTER;
    }
    let a = this as *mut Agg;
    unsafe {
        let iid = *iid;
        let field: *mut c_void = if iid == IUnknown::IID {
            &raw mut (*a).unk as _
        } else if iid == IAudioProcessingObject::IID {
            &raw mut (*a).apo as _
        } else if iid == IAudioProcessingObjectRT::IID {
            &raw mut (*a).rt as _
        } else if iid == IAudioProcessingObjectConfiguration::IID {
            &raw mut (*a).cfg as _
        } else if iid == IAudioSystemEffects::IID || iid == IAudioSystemEffects2::IID {
            &raw mut (*a).sfx as _
        } else {
            *out = std::ptr::null_mut();
            return windows::Win32::Foundation::E_NOINTERFACE;
        };
        // Pedido pelo IUnknown de dentro: a interface devolvida conta no dono (o de fora, se houver).
        if field == this {
            inner_addref(this);
        } else {
            ((*a).outer_vtbl().AddRef)((*a).outer);
        }
        *out = field;
    }
    windows::Win32::Foundation::S_OK
}

unsafe extern "system" fn inner_addref(this: *mut c_void) -> u32 {
    unsafe { (*(this as *const Agg)).refs.fetch_add(1, Relaxed) + 1 }
}

unsafe extern "system" fn inner_release(this: *mut c_void) -> u32 {
    let a = this as *mut Agg;
    let left = unsafe { (*a).refs.fetch_sub(1, std::sync::atomic::Ordering::AcqRel) } - 1;
    if left == 0 {
        drop(unsafe { Box::from_raw(a) });
    }
    left
}

impl windows_core::IUnknownImpl for Agg {
    type Impl = Agg;

    fn get_impl(&self) -> &Agg {
        self
    }

    fn get_impl_mut(&mut self) -> &mut Agg {
        self
    }

    fn into_inner(self) -> Agg {
        self
    }

    unsafe fn QueryInterface(&self, iid: *const GUID, interface: *mut *mut c_void) -> HRESULT {
        unsafe { (self.outer_vtbl().QueryInterface)(self.outer, iid, interface) }
    }

    fn AddRef(&self) -> u32 {
        unsafe { (self.outer_vtbl().AddRef)(self.outer) }
    }

    unsafe fn Release(self_: *mut Self) -> u32 {
        unsafe { ((*self_).outer_vtbl().Release)((*self_).outer) }
    }

    fn is_reference_count_one(&self) -> bool {
        false
    }

    unsafe fn GetTrustLevel(&self, _value: *mut i32) -> HRESULT {
        windows::Win32::Foundation::E_NOTIMPL
    }

    fn to_object(&self) -> windows_core::ComObject<Agg> {
        unreachable!("Agg não é um ComObject do windows-rs")
    }
}

impl windows_core::ComObjectInner for Agg {
    type Outer = Agg;

    fn into_object(self) -> windows_core::ComObject<Agg> {
        unreachable!("Agg não é um ComObject do windows-rs")
    }
}

// ---------------------------------------------------------------------------------------------
// COM

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, punkouter: Ref<IUnknown>, riid: *const GUID, ppvobject: *mut *mut c_void) -> Result<()> {
        guard(Err(E_FAIL.into()), "CreateInstance", || {
            if ppvobject.is_null() || riid.is_null() {
                return Err(E_POINTER.into());
            }
            unsafe { *ppvobject = std::ptr::null_mut() };
            let outer = punkouter.as_ref().map_or(std::ptr::null_mut(), |o| o.as_raw());
            // Regra do COM: agregado, só se pede o IUnknown de dentro.
            if !outer.is_null() && unsafe { *riid } != IUnknown::IID {
                return Err(CLASS_E_NOAGGREGATION.into());
            }
            unsafe { Agg::create(outer, riid, ppvobject) }.ok()
        })
    }

    fn LockServer(&self, _flock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// # Safety
/// Chamada pelo COM com ponteiros válidos.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(rclsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    guard(E_FAIL, "DllGetClassObject", || {
        if rclsid.is_null() || riid.is_null() || ppv.is_null() {
            return E_POINTER;
        }
        unsafe { *ppv = std::ptr::null_mut() };
        if unsafe { *rclsid } != CLSID {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let f: IClassFactory = Factory.into();
        unsafe { f.query(riid, ppv) }
    })
}

/// A DLL fica carregada até o processo acabar: mais simples e seguro que contar referências.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
