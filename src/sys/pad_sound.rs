//! Avisos pelo alto-falante do controle (DualSense e DualShock 4 no cabo). No cabo o controle é
//! também uma saída de som do Windows: o aviso toca nela em modo compartilhado, misturado ao que
//! já estiver tocando. No DualSense os canais 1 e 2 vão ao alto-falante e ao fone; os 3 e 4
//! (háptico) ficam calados. Pelo Bluetooth o controle não tem saída de som, e o aviso não toca.

use std::time::Duration;

use windows::core::*;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

use super::pads::Event;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Chime {
    Connected,
    Disconnected,
    Profile,
    BatteryLow,
    Latency,
}

/// O aviso de cada acontecimento do controle; os outros não tocam nada.
pub fn for_event(e: &Event) -> Option<Chime> {
    Some(match e {
        Event::Connected { .. } => Chime::Connected,
        Event::Disconnected { .. } => Chime::Disconnected,
        Event::Profile { .. } | Event::PadProfile { .. } => Chime::Profile,
        Event::BatteryLow { .. } => Chime::BatteryLow,
        Event::Jitter { .. } => Chime::Latency,
        _ => return None,
    })
}

/// As notas do aviso: (frequência em Hz, duração em ms); frequência 0 é pausa.
fn notes(c: Chime) -> &'static [(f32, u32)] {
    match c {
        Chime::Connected => &[(660.0, 90), (0.0, 30), (990.0, 140)],
        Chime::Disconnected => &[(990.0, 90), (0.0, 30), (660.0, 140)],
        Chime::Profile => &[(880.0, 80)],
        Chime::BatteryLow => &[(440.0, 120), (0.0, 80), (440.0, 120), (0.0, 80), (440.0, 120)],
        Chime::Latency => &[(1320.0, 60), (0.0, 50), (1320.0, 60)],
    }
}

/// O aviso em amostras mono de -1 a 1, na taxa `rate`. Cada nota sobe e desce em 5 ms, sem estalo.
fn samples(c: Chime, rate: u32) -> Vec<f32> {
    let mut out = Vec::new();
    for &(hz, ms) in notes(c) {
        let n = (rate as u64 * ms as u64 / 1000) as usize;
        let edge = (rate as usize / 200).max(1).min(n / 2).max(1);
        for i in 0..n {
            let env = (i.min(n - 1 - i) as f32 / edge as f32).min(1.0);
            let v = if hz > 0.0 { (std::f32::consts::TAU * hz * i as f32 / rate as f32).sin() } else { 0.0 };
            out.push(0.35 * env * v);
        }
    }
    out
}

/// Saída de som de um controle: o nome que o Windows dá ao DualSense e ao DualShock 4 no cabo.
fn is_pad_endpoint(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("wireless controller") || n.contains("dualsense") || n.contains("dualshock")
}

/// Toca o aviso em todos os controles ligados pelo cabo, numa linha própria. Ao conectar, a saída
/// de som do controle aparece um pouco depois do controle: tenta por até 3 s.
pub fn play(c: Chime) {
    let _ = std::thread::Builder::new().name("aviso do controle".into()).spawn(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        for _ in 0..if c == Chime::Connected { 6 } else { 1 } {
            if c == Chime::Connected {
                std::thread::sleep(Duration::from_millis(500));
            }
            if play_all(c) > 0 {
                break;
            }
        }
        CoUninitialize();
    });
}

/// Toca em cada saída de controle; devolve em quantas tocou.
unsafe fn play_all(c: Chime) -> usize {
    let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return 0 };
    let Ok(list) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return 0 };
    let mut played = 0;
    for i in 0..list.GetCount().unwrap_or(0) {
        let Ok(d) = list.Item(i) else { continue };
        let name = d.OpenPropertyStore(STGM_READ).and_then(|s| s.GetValue(&PKEY_Device_FriendlyName)).map(|v| v.to_string()).unwrap_or_default();
        if is_pad_endpoint(&name) && play_on(&d, c).is_ok() {
            played += 1;
        }
    }
    played
}

unsafe fn play_on(d: &IMMDevice, c: Chime) -> Result<()> {
    let client: IAudioClient = d.Activate(CLSCTX_ALL, None)?;
    let format = client.GetMixFormat()?;
    let (rate, channels, bits) = ((*format).nSamplesPerSec, (*format).nChannels as usize, (*format).wBitsPerSample);
    let opened = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, format, None);
    CoTaskMemFree(Some(format as *const _));
    opened?;
    // O modo compartilhado do Windows mistura em ponto flutuante de 32 bits.
    if bits != 32 || channels == 0 {
        return Err(Error::from(windows::Win32::Foundation::E_NOTIMPL));
    }
    let render: IAudioRenderClient = client.GetService()?;
    let size = client.GetBufferSize()?;
    let wave = samples(c, rate);
    let mut at = 0;
    client.Start()?;
    while at < wave.len() {
        let free = (size - client.GetCurrentPadding()?) as usize;
        let n = free.min(wave.len() - at);
        if n > 0 {
            let out = std::slice::from_raw_parts_mut(render.GetBuffer(n as u32)? as *mut f32, n * channels);
            for (frame, v) in out.chunks_exact_mut(channels).zip(&wave[at..at + n]) {
                frame.fill(0.0);
                frame[0] = *v;
                if channels > 1 {
                    frame[1] = *v;
                }
            }
            render.ReleaseBuffer(n as u32, 0)?;
            at += n;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Espera o fim do que já foi para o controle antes de fechar.
    while client.GetCurrentPadding()? > 0 {
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = client.Stop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chimes_are_short_and_soft() {
        for c in [Chime::Connected, Chime::Disconnected, Chime::Profile, Chime::BatteryLow, Chime::Latency] {
            let s = samples(c, 48_000);
            let ms: u32 = notes(c).iter().map(|n| n.1).sum();
            assert_eq!(s.len(), (48 * ms) as usize);
            assert!(ms <= 600 && s.iter().all(|v| v.abs() <= 0.35));
            // Começa e termina calado: sem estalo no alto-falante.
            assert!(s[0].abs() < 0.01 && s[s.len() - 1].abs() < 0.01);
        }
        assert!(is_pad_endpoint("Alto-falantes (Wireless Controller)"));
        assert!(is_pad_endpoint("Speakers (DualSense Wireless Controller)"));
        assert!(!is_pad_endpoint("Alto-falantes (Realtek(R) Audio)"));
    }
}
