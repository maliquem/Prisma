//! Mantém a saída de som acordada com um fluxo sem som audível.
//! Sem isso, HDMI e Dolby dormem no silêncio e cortam o começo do próximo som.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

/// 0 desligado, 1 silêncio puro, 2 sinal mínimo.
static MODE: AtomicU8 = AtomicU8::new(0);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Fluxo tocando impede a suspensão automática: longe do PC ele para sozinho.
const IDLE: Duration = Duration::from_secs(600);
const PULSE_HZ: u32 = 50;
/// Um degrau de 16 bits: alguns aparelhos tratam zero puro como fim do sinal.
const PULSE: f32 = 1.0 / 65536.0;

pub fn set(mode: u8) {
    MODE.store(mode, Ordering::Relaxed);
    if mode > 0 && !RUNNING.swap(true, Ordering::Relaxed) {
        let _ = std::thread::Builder::new().name("som acordado".into()).spawn(|| unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            run();
            CoUninitialize();
            RUNNING.store(false, Ordering::Relaxed);
        });
    }
}

fn idle() -> Duration {
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    unsafe {
        if !GetLastInputInfo(&mut info).as_bool() {
            return Duration::ZERO;
        }
        Duration::from_millis(GetTickCount().wrapping_sub(info.dwTime) as u64)
    }
}

unsafe fn default_id(en: &IMMDeviceEnumerator) -> Option<(IMMDevice, String)> {
    let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
    let raw = dev.GetId().ok()?;
    let id = raw.to_string().unwrap_or_default();
    CoTaskMemFree(Some(raw.0 as *const _));
    Some((dev, id))
}

/// Teste: toca alguns segundos e diz o que o Windows respondeu.
#[cfg(debug_assertions)]
pub fn probe(mode: u8, secs: u64) -> String {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MODE.store(mode, Ordering::Relaxed);
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return "sem enumerador".into() };
        let Some((dev, id)) = default_id(&en) else { return "sem saida".into() };
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            MODE.store(0, Ordering::Relaxed);
        });
        format!("{id}: {:?} idle {:?}", stream(&en, &dev, &id), idle())
    }
}

unsafe fn run() {
    let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return };
    while MODE.load(Ordering::Relaxed) > 0 {
        if idle() < IDLE {
            if let Some((dev, id)) = default_id(&en) {
                let _ = stream(&en, &dev, &id);
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// Toca até desligarem, a saída padrão mudar, o aparelho sumir ou a pessoa se afastar.
unsafe fn stream(en: &IMMDeviceEnumerator, dev: &IMMDevice, id: &str) -> Result<()> {
    const ONE_SECOND: i64 = 10_000_000;
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    let format = client.GetMixFormat()?;
    let (rate, channels) = ((*format).nSamplesPerSec, (*format).nChannels as usize);
    let opened = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_NOPERSIST, ONE_SECOND, 0, format, None);
    CoTaskMemFree(Some(format as *const _));
    opened?;
    let render: IAudioRenderClient = client.GetService()?;
    let size = client.GetBufferSize()?;
    let gap = (rate / PULSE_HZ).max(1) as u64;
    let mut written = 0u64;
    let mut sign = 1.0f32;
    let mut checked = Instant::now();
    let mut started = false;
    loop {
        let mode = MODE.load(Ordering::Relaxed);
        if mode == 0 || idle() >= IDLE {
            break;
        }
        let free = size - client.GetCurrentPadding()?;
        if free > 0 {
            let data = render.GetBuffer(free)? as *mut f32;
            let out = std::slice::from_raw_parts_mut(data, free as usize * channels);
            out.fill(0.0);
            if mode == 2 {
                for (i, frame) in out.chunks_exact_mut(channels).enumerate() {
                    if (written + i as u64) % gap == 0 {
                        sign = -sign;
                        frame.fill(PULSE * sign);
                    }
                }
            }
            render.ReleaseBuffer(free, 0)?;
            written += free as u64;
        }
        if !started {
            client.Start()?;
            started = true;
        }
        if checked.elapsed() >= Duration::from_secs(2) {
            checked = Instant::now();
            if default_id(en).map_or(true, |d| d.1 != id) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = client.Stop();
    Ok(())
}
