//! Áudio da gravação: som do sistema e microfone, misturados e codificados em AAC.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::Media::Multimedia::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Performance::*;
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};

use super::mux::Pkt;

const CLSID_AAC_ENCODER: GUID = GUID::from_u128(0x93af0c51_2275_45d2_a35b_f2ba21caed00);
const RATE: u32 = 48000;
const FRAME: usize = 1024;

pub(crate) fn format() -> WAVEFORMATEXTENSIBLE {
    WAVEFORMATEXTENSIBLE {
        Format: WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
            nChannels: 2,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 8,
            nBlockAlign: 8,
            wBitsPerSample: 32,
            cbSize: 22,
        },
        Samples: WAVEFORMATEXTENSIBLE_0 { wValidBitsPerSample: 32 },
        dwChannelMask: 3,
        SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
    }
}

pub(crate) struct Source {
    pub(crate) client: IAudioClient,
    capture: IAudioCaptureClient,
}

pub(crate) unsafe fn open(flow: EDataFlow, loopback: bool) -> Result<Source> {
    open_on("", flow, loopback)
}

/// Como `open`, num aparelho escolhido pelo id do Windows; vazio é o padrão.
pub(crate) unsafe fn open_on(id: &str, flow: EDataFlow, loopback: bool) -> Result<Source> {
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let dev = if id.is_empty() { en.GetDefaultAudioEndpoint(flow, eConsole)? } else { en.GetDevice(&HSTRING::from(id))? };
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    let mut flags = AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
    if loopback {
        flags |= AUDCLNT_STREAMFLAGS_LOOPBACK;
    }
    let f = format();
    client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &f.Format, None)?;
    let capture: IAudioCaptureClient = client.GetService()?;
    client.Start()?;
    Ok(Source { client, capture })
}

#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct Activated(isize);

impl IAgileObject_Impl for Activated_Impl {}

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(&self, _op: Ref<'_, IActivateAudioInterfaceAsyncOperation>) -> Result<()> {
        unsafe { SetEvent(HANDLE(self.0 as *mut _)) }
    }
}

/// Mesma memória de um PROPVARIANT com VT_BLOB: a API só aceita os parâmetros assim.
#[repr(C)]
struct Blob {
    vt: u16,
    pad: [u16; 3],
    size: u32,
    data: *const AUDIOCLIENT_ACTIVATION_PARAMS,
}

/// Grava só um programa e os processos filhos dele. Existe desde o Windows 10 2004.
unsafe fn open_process(pid: u32) -> Result<Source> {
    open_process_mode(pid, false)
}

/// Som de um programa e dos filhos dele, ou, com `exclude`, de todos menos eles.
pub(crate) unsafe fn open_process_mode(pid: u32, exclude: bool) -> Result<Source> {
    const VT_BLOB: u16 = 65;
    let params = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: if exclude {
                    PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE
                } else {
                    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE
                },
            },
        },
    };
    let blob = Blob { vt: VT_BLOB, pad: [0; 3], size: std::mem::size_of_val(&params) as u32, data: &params };
    let event = CreateEventW(None, false, false, None)?;
    let handler: IActivateAudioInterfaceCompletionHandler = Activated(event.0 as isize).into();
    let op = ActivateAudioInterfaceAsync(w!("VAD\\Process_Loopback"), &IAudioClient::IID, Some(&blob as *const Blob as *const _), &handler)?;
    let waited = WaitForSingleObject(event, 5000);
    let _ = CloseHandle(event);
    if waited != WAIT_OBJECT_0 {
        return Err(Error::from(E_FAIL));
    }
    let mut hr = HRESULT(0);
    let mut unknown = None;
    op.GetActivateResult(&mut hr, &mut unknown)?;
    hr.ok()?;
    let client: IAudioClient = unknown.ok_or_else(|| Error::from(E_FAIL))?.cast()?;
    let f = format();
    client.Initialize(
        AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
        2_000_000,
        0,
        &f.Format,
        None,
    )?;
    let capture: IAudioCaptureClient = client.GetService()?;
    client.Start()?;
    Ok(Source { client, capture })
}

pub(crate) unsafe fn read(s: &Source, out: &mut Vec<f32>) {
    while s.capture.GetNextPacketSize().unwrap_or(0) > 0 {
        let mut data = std::ptr::null_mut();
        let mut frames = 0;
        let mut flags = 0;
        if s.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None).is_err() {
            return;
        }
        let n = frames as usize * 2;
        if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
            out.resize(out.len() + n, 0.0);
        } else {
            out.extend_from_slice(std::slice::from_raw_parts(data as *const f32, n));
        }
        let _ = s.capture.ReleaseBuffer(frames);
    }
}

struct Aac {
    mft: IMFTransform,
}

impl Aac {
    unsafe fn new() -> Result<Self> {
        let mft: IMFTransform = CoCreateInstance(&CLSID_AAC_ENCODER, None, CLSCTX_INPROC_SERVER)?;
        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
        input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        mft.SetInputType(0, &input, 0)?;
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE)?;
        out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
        out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24000)?;
        mft.SetOutputType(0, &out, 0)?;
        mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        Ok(Self { mft })
    }

    unsafe fn encode(&self, pcm: &[i16], pts: i64, tx: &Sender<Pkt>) -> Result<()> {
        let bytes = std::slice::from_raw_parts(pcm.as_ptr() as *const u8, pcm.len() * 2);
        let buf = MFCreateMemoryBuffer(bytes.len() as u32)?;
        let mut p = std::ptr::null_mut();
        buf.Lock(&mut p, None, None)?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
        buf.Unlock()?;
        buf.SetCurrentLength(bytes.len() as u32)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(pts)?;
        s.SetSampleDuration(pcm.len() as i64 / 2 * 10_000_000 / RATE as i64)?;
        self.mft.ProcessInput(0, &s, 0)?;
        let mut next = pts;
        loop {
            let ob = MFCreateMemoryBuffer(4096)?;
            let os = MFCreateSample()?;
            os.AddBuffer(&ob)?;
            let mut out = [MFT_OUTPUT_DATA_BUFFER { pSample: std::mem::ManuallyDrop::new(Some(os.clone())), ..Default::default() }];
            let mut st = 0;
            let r = self.mft.ProcessOutput(0, &mut out, &mut st);
            drop(std::mem::ManuallyDrop::into_inner(std::mem::replace(&mut out[0].pSample, std::mem::ManuallyDrop::new(None))));
            if r.is_err() {
                return Ok(());
            }
            let mut p = std::ptr::null_mut();
            let mut len = 0;
            ob.Lock(&mut p, None, Some(&mut len))?;
            let data = std::slice::from_raw_parts(p, len as usize).to_vec();
            ob.Unlock()?;
            if !data.is_empty() {
                let _ = tx.send(Pkt { data, pts: next, key: true });
                next += FRAME as i64 * 10_000_000 / RATE as i64;
            }
        }
    }
}

fn now_100ns() -> i64 {
    let (mut c, mut f) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
    }
    (c as i128 * 10_000_000 / f as i128) as i64
}

pub fn spawn(system: bool, pid: u32, mic: bool, stop: Arc<AtomicBool>, tx: Sender<Pkt>) {
    let _ = std::thread::Builder::new().name("audio".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        if let Err(e) = run(system, pid, mic, &stop, &tx) {
            eprintln!("áudio: {e:?}");
        }
        let _ = MFShutdown();
    });
}

/// Teste: mede o pico do som de um programa durante `ms`.
#[cfg(debug_assertions)]
pub fn probe(pid: u32, ms: u64) -> String {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let s = match open_process(pid) {
            Ok(s) => s,
            Err(e) => return format!("erro {e:?}"),
        };
        let mut v = Vec::new();
        let t0 = std::time::Instant::now();
        while t0.elapsed().as_millis() < ms as u128 {
            std::thread::sleep(Duration::from_millis(20));
            read(&s, &mut v);
        }
        let _ = s.client.Stop();
        let peak = v.iter().fold(0.0f32, |a, x| a.max(x.abs()));
        format!("{} amostras, pico {peak:.3}", v.len())
    }
}

unsafe fn run(system: bool, pid: u32, mic: bool, stop: &AtomicBool, tx: &Sender<Pkt>) -> Result<()> {
    let sys = if system {
        open(eRender, true).ok()
    } else if pid != 0 {
        open_process(pid).ok()
    } else {
        None
    };
    let micro = if mic { open(eCapture, false).ok() } else { None };
    let aac = Aac::new()?;
    let start = now_100ns();
    let mut produced: i64 = 0;
    let (mut a, mut b) = (Vec::<f32>::new(), Vec::<f32>::new());
    let mut pcm = vec![0i16; FRAME * 2];
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(10));
        if let Some(s) = &sys {
            read(s, &mut a);
        }
        if let Some(m) = &micro {
            read(m, &mut b);
        }
        // Sem som tocando o Windows não entrega nada: completa com silêncio para manter o tempo.
        let expected = (now_100ns() - start) * RATE as i64 / 10_000_000;
        let have = produced + (a.len() / 2) as i64;
        if have + 2400 < expected {
            a.resize(a.len() + ((expected - have) * 2) as usize, 0.0);
        }
        while a.len() >= FRAME * 2 {
            for i in 0..FRAME * 2 {
                let m = b.get(i).copied().unwrap_or(0.0);
                pcm[i] = ((a[i] + m).clamp(-1.0, 1.0) * 32767.0) as i16;
            }
            a.drain(..FRAME * 2);
            b.drain(..(FRAME * 2).min(b.len()));
            let pts = start + produced * 10_000_000 / RATE as i64;
            aac.encode(&pcm, pts, tx)?;
            produced += FRAME as i64;
        }
    }
    if let Some(s) = &sys {
        let _ = s.client.Stop();
    }
    if let Some(m) = &micro {
        let _ = m.client.Stop();
    }
    Ok(())
}
