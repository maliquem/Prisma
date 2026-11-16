//! Teste dentro do processo: chama o APO como o motor de áudio chamaria, sem registrar nada no Windows.

use std::ffi::c_void;
use std::mem::ManuallyDrop;

use windows::Win32::Foundation::{E_POINTER, S_OK};
use windows::Win32::Media::Audio::Apo::*;
use windows::Win32::Media::Audio::WAVEFORMATEX;
use windows::Win32::Media::Multimedia::KSDATAFORMAT_SUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::IClassFactory;
use windows_core::{BOOL, GUID, IUnknown, Interface, Ref, Result, implement};

const RATE: u32 = 48_000;
const CH: u32 = 2;

/// Tipo de mídia mínimo: o crate não traz `CreateAudioMediaType`.
#[implement(IAudioMediaType)]
struct MediaType {
    u: UNCOMPRESSEDAUDIOFORMAT,
    wfx: std::cell::UnsafeCell<WAVEFORMATEX>,
}

fn media(float: bool) -> IAudioMediaType {
    let bytes = if float { 4 } else { 2 };
    let guid = if float { KSDATAFORMAT_SUBTYPE_IEEE_FLOAT } else { GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71) };
    let u = UNCOMPRESSEDAUDIOFORMAT { guidFormatType: guid, dwSamplesPerFrame: CH, dwBytesPerSampleContainer: bytes, dwValidBitsPerSample: bytes * 8, fFramesPerSecond: RATE as f32, dwChannelMask: 3 };
    let wfx = WAVEFORMATEX { wFormatTag: if float { 3 } else { 1 }, nChannels: CH as u16, nSamplesPerSec: RATE, nAvgBytesPerSec: RATE * CH * bytes, nBlockAlign: (CH * bytes) as u16, wBitsPerSample: (bytes * 8) as u16, cbSize: 0 };
    MediaType { u, wfx: wfx.into() }.into()
}

impl IAudioMediaType_Impl for MediaType_Impl {
    fn IsCompressedFormat(&self) -> Result<BOOL> {
        Ok(false.into())
    }

    fn IsEqual(&self, other: Ref<IAudioMediaType>) -> Result<u32> {
        let other = other.ok()?;
        let mut u = UNCOMPRESSEDAUDIOFORMAT::default();
        unsafe { other.GetUncompressedAudioFormat(&mut u) }?;
        let same_type = u.guidFormatType == self.u.guidFormatType;
        let same_data = same_type && u.dwSamplesPerFrame == self.u.dwSamplesPerFrame && u.dwBytesPerSampleContainer == self.u.dwBytesPerSampleContainer && u.fFramesPerSecond == self.u.fFramesPerSecond;
        Ok(if same_type { AUDIOMEDIATYPE_EQUAL_FORMAT_TYPES } else { 0 } | if same_data { AUDIOMEDIATYPE_EQUAL_FORMAT_DATA } else { 0 })
    }

    fn GetAudioFormat(&self) -> *mut WAVEFORMATEX {
        self.wfx.get()
    }

    fn GetUncompressedAudioFormat(&self, out: *mut UNCOMPRESSEDAUDIOFORMAT) -> Result<()> {
        if out.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *out = self.u };
        Ok(())
    }
}

fn descriptor(buf: &mut [f32], t: &IAudioMediaType) -> APO_CONNECTION_DESCRIPTOR {
    APO_CONNECTION_DESCRIPTOR { Type: APO_CONNECTION_BUFFER_TYPE(1), pBuffer: buf.as_mut_ptr() as usize, u32MaxFrameCount: (buf.len() as u32) / CH, pFormat: ManuallyDrop::new(Some(t.clone())), u32Signature: 0 }
}

fn release(mut d: APO_CONNECTION_DESCRIPTOR) {
    unsafe { ManuallyDrop::drop(&mut d.pFormat) };
}

fn process(rt: &IAudioProcessingObjectRT, input: *mut f32, output: *mut f32, frames: u32, flags: APO_BUFFER_FLAGS) -> APO_CONNECTION_PROPERTY {
    let i = APO_CONNECTION_PROPERTY { pBuffer: input as usize, u32ValidFrameCount: frames, u32BufferFlags: flags, u32Signature: 0 };
    let mut o = APO_CONNECTION_PROPERTY { pBuffer: output as usize, u32ValidFrameCount: 0, u32BufferFlags: BUFFER_INVALID, u32Signature: 0 };
    let ip: *const APO_CONNECTION_PROPERTY = &i;
    let mut op: *mut APO_CONNECTION_PROPERTY = &mut o;
    unsafe { rt.APOProcess(1, &ip, 1, &mut op) };
    o
}

#[test]
fn passa_o_som_e_anota_no_log() -> Result<()> {
    let dir = std::env::temp_dir().join(format!("prisma-apo-teste-{}", std::process::id()));
    let log = dir.join("apo.log");
    let _ = std::fs::remove_dir_all(&dir);
    // Único teste do arquivo: mexer no ambiente aqui não corre com outra thread.
    unsafe { std::env::set_var("PRISMA_APO_LOG", &log) };

    // Fábrica: CLSID errado é recusado, o certo entrega o objeto.
    let mut p: *mut c_void = std::ptr::null_mut();
    let wrong = GUID::from_u128(1);
    assert_ne!(unsafe { prisma_apo::DllGetClassObject(&wrong, &IClassFactory::IID, &mut p) }, S_OK);
    assert_eq!(unsafe { prisma_apo::DllGetClassObject(&prisma_apo::CLSID, &IClassFactory::IID, &mut p) }, S_OK);
    let factory = unsafe { IClassFactory::from_raw(p) };
    let apo: IAudioProcessingObject = unsafe { factory.CreateInstance(None::<&IUnknown>) }?;

    // Registro: o motor lê isto antes de tudo.
    let reg = unsafe { apo.GetRegistrationProperties() }?;
    unsafe {
        assert_eq!((*reg).clsid, prisma_apo::CLSID);
        assert_eq!((*reg).iidAPOInterfaceList[0], IAudioSystemEffects2::IID);
        windows::Win32::System::Com::CoTaskMemFree(Some(reg as *const c_void));
    }

    // Initialize com a estrutura da versão 2 vazia (sem aparelho, sem propriedades).
    let mut init = APOInitSystemEffects2::default();
    init.APOInit.cbSize = std::mem::size_of::<APOInitSystemEffects2>() as u32;
    init.APOInit.clsid = prisma_apo::CLSID;
    let bytes = unsafe { std::slice::from_raw_parts(&init as *const _ as *const u8, std::mem::size_of_val(&init)) };
    unsafe { apo.Initialize(bytes) }?;

    // Formato: float igual dos dois lados passa; inteiro é recusado.
    let f = media(true);
    let back = unsafe { apo.IsInputFormatSupported(&f, &f) }?;
    assert_eq!(back.as_raw(), f.as_raw());
    assert!(unsafe { apo.IsOutputFormatSupported(&f, &media(false)) }.is_err());

    let frames = 480u32;
    let n = (frames * CH) as usize;
    let mut input: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin()).collect();
    let mut output = vec![0f32; n];

    let cfg: IAudioProcessingObjectConfiguration = apo.cast()?;
    let rt: IAudioProcessingObjectRT = apo.cast()?;
    let lock = |input: &mut Vec<f32>, output: &mut Vec<f32>| -> Result<()> {
        let (di, do_) = (descriptor(input, &f), descriptor(output, &f));
        unsafe { cfg.LockForProcess(&[&di as *const _], &[&do_ as *const _]) }?;
        release(di);
        release(do_);
        Ok(())
    };
    // Sem perfil: passa o som sem mexer.
    lock(&mut input, &mut output)?;
    assert_eq!(unsafe { apo.GetInputChannelCount() }?, CH);

    // Buffers separados: copia igual.
    let o = process(&rt, input.as_mut_ptr(), output.as_mut_ptr(), frames, BUFFER_VALID);
    assert_eq!(o.u32ValidFrameCount, frames);
    assert_eq!(o.u32BufferFlags, BUFFER_VALID);
    assert_eq!(input, output);

    // Mesmo buffer: não mexe.
    let before = input.clone();
    let ptr = input.as_mut_ptr();
    let o = process(&rt, ptr, ptr, frames, BUFFER_VALID);
    assert_eq!(o.u32ValidFrameCount, frames);
    assert_eq!(input, before);
    unsafe { cfg.UnlockForProcess() }?;

    // Com perfil (vale no próximo LockForProcess): -6,02 dB = metade, nos dois jeitos de buffer.
    std::fs::write(dir.join("perfil.txt"), "preamp=-6.0206
").unwrap();
    lock(&mut input, &mut output)?;
    process(&rt, input.as_mut_ptr(), output.as_mut_ptr(), frames, BUFFER_VALID);
    assert!(input.iter().zip(&output).all(|(i, o)| (i * 0.5 - o).abs() < 1e-6));
    let ptr = input.as_mut_ptr();
    process(&rt, ptr, ptr, frames, BUFFER_VALID);
    assert!(before.iter().zip(&input).all(|(b, i)| (b * 0.5 - i).abs() < 1e-6));

    // Troca ao vivo: o perfil salvo de novo vale sem outro LockForProcess, depois de 20 ms de mistura
    // (960 quadros a 48 kHz: o terceiro bloco de 480 já é só o novo).
    std::fs::write(dir.join("perfil.txt"), "preamp=-12.0412
").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(800));
    let mut src = before.clone();
    for _ in 0..3 {
        process(&rt, src.as_mut_ptr(), output.as_mut_ptr(), frames, BUFFER_VALID);
    }
    assert!(src.iter().zip(&output).all(|(i, o)| (i * 0.25 - o).abs() < 1e-6), "perfil novo não entrou");
    unsafe { cfg.UnlockForProcess() }?;

    // Perfil com erro: passa o som sem mexer.
    std::fs::write(dir.join("perfil.txt"), "bobagem
").unwrap();
    lock(&mut input, &mut output)?;
    let before = input.clone();
    process(&rt, input.as_mut_ptr(), output.as_mut_ptr(), frames, BUFFER_VALID);
    assert_eq!(before, output);

    // Silêncio: passa a marca adiante, sem copiar.
    output.fill(9.0);
    let o = process(&rt, input.as_mut_ptr(), output.as_mut_ptr(), frames, BUFFER_SILENT);
    assert_eq!(o.u32BufferFlags, BUFFER_SILENT);
    assert!(output.iter().all(|&v| v == 9.0));

    unsafe { cfg.UnlockForProcess() }?;
    drop((rt, cfg, back, apo));

    // Agregado, como o motor de áudio cria: só IUnknown; as interfaces saem pelo IUnknown de dentro.
    let outer: IUnknown = media(true).cast()?;
    assert!(unsafe { factory.CreateInstance::<_, IAudioProcessingObject>(&outer) }.is_err());
    let inner: IUnknown = unsafe { factory.CreateInstance(&outer) }?;
    let agg: IAudioProcessingObject = inner.cast()?;
    let agg_rt: IAudioProcessingObjectRT = inner.cast()?;
    unsafe { agg.Initialize(bytes) }?;
    assert_eq!(unsafe { agg_rt.CalcOutputFrames(7) }, 7);
    // O IUnknown das interfaces é o de fora (identidade do COM agregado).
    let via_agg: IUnknown = agg.cast()?;
    assert_eq!(via_agg.as_raw(), outer.as_raw());
    drop((via_agg, agg_rt, agg, inner, factory));

    let text = std::fs::read_to_string(&log).expect("log não foi escrito");
    for wanted in ["criada", "Initialize", "versão 2", "aceito", "recusado", "LockForProcess: entrada [float 2 canais 32 bits 48000 Hz", "chamadas=9 quadros=4320 silenciosos=1 copias=6", "sem perfil", "com aviso", "perfil.txt (0 filtros", "sozinha", "agregada", "instância 2 destruída"] {
        assert!(text.contains(wanted), "falta \"{wanted}\" no log:\n{text}");
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}
