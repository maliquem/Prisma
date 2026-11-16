//! Sonda só de leitura: para cada saída de som ativa, o que o Windows diz sobre o atraso.
//! Não muda nada. Abre um fluxo compartilhado sem tocar (só `Initialize`) para ler o atraso
//! do fluxo; com `--tocar`, toca 2 s de silêncio para comparar o que foi escrito com o relógio
//! do aparelho.
//!
//! Uso: `cargo run --release -- [--tocar]`

use std::time::{Duration, Instant};

use windows::Media::Audio::SpatialAudioDeviceConfiguration;
use windows::Win32::Devices::FunctionDiscovery::{PKEY_Device_EnumeratorName, PKEY_Device_FriendlyName};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::core::*;

const MMDEV: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render";
const FX: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d}";
/// Nome de cada PKEY_FX_* (o número depois da vírgula).
const FX_KEYS: [(u32, &str); 9] = [
    (1, "pré-mistura (antigo)"),
    (2, "pós-mistura (antigo)"),
    (5, "por fluxo"),
    (6, "por modo"),
    (7, "no aparelho"),
    (13, "por fluxo"),
    (14, "por modo"),
    (15, "no aparelho"),
    (11, "por fluxo (offload)"),
];

fn main() -> Result<()> {
    let play = std::env::args().any(|a| a == "--tocar");
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let default = en.GetDefaultAudioEndpoint(eRender, eConsole).ok().and_then(|d| id_of(&d));
        let all = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for i in 0..all.GetCount()? {
            let dev = all.Item(i)?;
            let id = id_of(&dev).unwrap_or_default();
            let mark = if Some(&id) == default.as_ref() { "  [padrão]" } else { "" };
            println!("\n== {}{mark}", name(&dev));
            if let Err(e) = report(&dev, &id, play) {
                println!("   erro: {e}");
            }
        }
    }
    Ok(())
}

unsafe fn id_of(dev: &IMMDevice) -> Option<String> {
    let raw = unsafe { dev.GetId() }.ok()?;
    let s = unsafe { raw.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(raw.0 as *const _)) };
    s
}

unsafe fn prop(dev: &IMMDevice, key: &windows::Win32::Foundation::PROPERTYKEY) -> String {
    unsafe { dev.OpenPropertyStore(STGM_READ) }
        .and_then(|s| unsafe { s.GetValue(key) })
        .map(|v| v.to_string())
        .unwrap_or_default()
}

unsafe fn name(dev: &IMMDevice) -> String {
    unsafe { prop(dev, &PKEY_Device_FriendlyName) }
}

/// Barramento: HDAUDIO (HDMI ou placa), USB, BTHENUM (Bluetooth clássico), BTHLEDEVICE (LE Audio)…
unsafe fn bus(dev: &IMMDevice) -> String {
    unsafe { prop(dev, &PKEY_Device_EnumeratorName) }
}

fn ms(hns: i64) -> f64 {
    hns as f64 / 10_000.0
}

fn frames_ms(frames: u32, rate: u32) -> f64 {
    frames as f64 * 1000.0 / rate as f64
}

unsafe fn report(dev: &IMMDevice, id: &str, play: bool) -> Result<()> {
    println!("   barramento: {}", unsafe { bus(dev) });
    let client: IAudioClient = unsafe { dev.Activate(CLSCTX_ALL, None) }?;
    let mix = unsafe { client.GetMixFormat() }?;
    let (rate, channels) = unsafe { ((*mix).nSamplesPerSec, (*mix).nChannels) };
    println!("   mistura do Windows: {channels} canais, {rate} Hz");
    match device_format(id) {
        Some((tag, ch, r, bits)) => {
            let resample = if r != rate && !tag.starts_with("bitstream") { "  (reamostra!)" } else { "" };
            println!("   formato do aparelho: {tag}, {ch} canais, {r} Hz, {bits} bits{resample}");
        }
        None => println!("   formato do aparelho: não informado"),
    }
    let (mut def, mut min) = (0i64, 0i64);
    unsafe { client.GetDevicePeriod(Some(&mut def), Some(&mut min)) }?;
    println!("   período do aparelho: padrão {:.2} ms, mínimo {:.2} ms (exclusivo)", ms(def), ms(min));
    if let Ok(c3) = client.cast::<IAudioClient3>() {
        let (mut d, mut f, mut lo, mut hi) = (0u32, 0u32, 0u32, 0u32);
        if unsafe { c3.GetSharedModeEnginePeriod(mix, &mut d, &mut f, &mut lo, &mut hi) }.is_ok() {
            let r = rate;
            println!(
                "   motor compartilhado: padrão {:.2} ms, mínimo {:.2} ms, máximo {:.2} ms, passo {:.2} ms{}",
                frames_ms(d, r),
                frames_ms(lo, r),
                frames_ms(hi, r),
                frames_ms(f, r),
                if lo < d { "" } else { "  (driver não aceita período menor)" }
            );
        }
        let mut cur_fmt = std::ptr::null_mut();
        let mut cur = 0u32;
        if unsafe { c3.GetCurrentSharedModeEnginePeriod(&mut cur_fmt, &mut cur) }.is_ok() {
            println!("   motor agora: {:.2} ms", frames_ms(cur, rate));
            unsafe { CoTaskMemFree(Some(cur_fmt as *const _)) };
        }
    }
    // Fluxo compartilhado comum, como a maioria dos programas abre (sem Start: não toca).
    let s: IAudioClient = unsafe { dev.Activate(CLSCTX_ALL, None) }?;
    match unsafe { s.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 0, 0, mix, None) } {
        Ok(()) => {
            let lat = unsafe { s.GetStreamLatency() }.unwrap_or(0);
            let buf = unsafe { s.GetBufferSize() }.unwrap_or(0);
            println!(
                "   fluxo comum: GetStreamLatency {:.2} ms, buffer do programa {:.2} ms",
                ms(lat),
                frames_ms(buf, rate)
            );
        }
        Err(e) => println!("   fluxo comum: não abriu ({e})"),
    }
    let (spatial_ok, spatial) = spatial(id);
    println!("   som espacial: {}", if spatial.is_empty() { if spatial_ok { "desligado" } else { "sem suporte" } } else { &spatial });
    effects(id);
    if play {
        match unsafe { measure(dev, mix, rate) } {
            Ok((queued, pad)) => println!(
                "   tocando silêncio: escrito − relógio do aparelho = {queued:.1} ms (fila do programa {pad:.1} ms)"
            ),
            Err(e) => println!("   tocando silêncio: falhou ({e})"),
        }
    }
    unsafe { CoTaskMemFree(Some(mix as *const _)) };
    Ok(())
}

/// Toca 2 s de silêncio e compara quadros escritos com a posição do relógio do aparelho,
/// trazida para o mesmo instante pelo QPC. Mede a fila que o Windows e o driver enxergam
/// (programa + motor + APOs + buffer do driver), não o que vem depois (cabo, TV, soundbar).
unsafe fn measure(dev: &IMMDevice, mix: *const WAVEFORMATEX, rate: u32) -> Result<(f64, f64)> {
    let c: IAudioClient = unsafe { dev.Activate(CLSCTX_ALL, None) }?;
    unsafe { c.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 200_000, 0, mix, None) }?;
    let size = unsafe { c.GetBufferSize() }?;
    let render: IAudioRenderClient = unsafe { c.GetService() }?;
    let clock: IAudioClock = unsafe { c.GetService() }?;
    let freq = unsafe { clock.GetFrequency() }? as f64;
    let mut written = 0u64;
    let fill = |written: &mut u64| -> Result<u32> {
        let pad = unsafe { c.GetCurrentPadding() }?;
        let free = size - pad;
        if free > 0 {
            unsafe { render.GetBuffer(free) }?;
            unsafe { render.ReleaseBuffer(free, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) }?;
            *written += free as u64;
        }
        Ok(pad)
    };
    fill(&mut written)?;
    unsafe { c.Start() }?;
    let start = Instant::now();
    let mut samples = Vec::new();
    while start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(5));
        fill(&mut written)?;
        if start.elapsed() < Duration::from_millis(500) {
            continue;
        }
        let (mut pos, mut qpc) = (0u64, 0u64);
        unsafe { clock.GetPosition(&mut pos, Some(&mut qpc)) }?;
        let mut now = 0i64;
        unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut now) }?;
        let mut qf = 0i64;
        unsafe { windows::Win32::System::Performance::QueryPerformanceFrequency(&mut qf) }?;
        let now_hns = now as f64 * 10_000_000.0 / qf as f64;
        // posição avançada até agora (o relógio anda em tempo real entre qpc e now)
        let played = pos as f64 / freq * rate as f64 + (now_hns - qpc as f64) / 10_000_000.0 * rate as f64;
        let pad = unsafe { c.GetCurrentPadding() }? as f64;
        samples.push(((written as f64 - played) * 1000.0 / rate as f64, pad * 1000.0 / rate as f64));
    }
    unsafe { c.Stop() }?;
    samples.sort_by(|a, b| a.0.total_cmp(&b.0));
    Ok(samples.get(samples.len() / 2).copied().unwrap_or((0.0, 0.0)))
}

fn spatial(id: &str) -> (bool, String) {
    let path = format!("\\\\?\\SWD#MMDEVAPI#{id}#{{e6327cad-dcec-4949-ae8a-991e976a79d2}}");
    let Ok(c) = SpatialAudioDeviceConfiguration::GetForDeviceId(&HSTRING::from(path)) else { return (false, String::new()) };
    let fmt = c.ActiveSpatialAudioFormat().map(|h| h.to_string()).unwrap_or_default();
    let name = match fmt.trim_matches(|c| c == '{' || c == '}').to_ascii_uppercase().as_str() {
        "" | "00000000-0000-0000-0000-000000000000" => "",
        "A289735D-FA3E-4E35-9D7D-B6F896ACB2E7" => "Dolby Atmos for Home Theater",
        "1459AC38-3875-49BF-BB59-0FE80F4D395D" => "Dolby Atmos for Headphones",
        "4C81E564-C8EF-4AD9-9F2C-9EF995533790" => "Dolby Atmos for Speakers",
        "B53D940C-B846-4831-9F76-D102B9B725A0" => "Windows Sonic",
        "10201B4A-3322-4967-BF40-2CAA9BAFCA44" => "DTS:X",
        _ => "outro",
    };
    (c.IsSpatialAudioSupported().unwrap_or(false), name.to_string())
}

fn reg(root: HKEY, key: &str, value: &str) -> Option<(REG_VALUE_TYPE, Vec<u8>)> {
    let key = HSTRING::from(key);
    let value = HSTRING::from(value);
    let name = if value.is_empty() { PCWSTR::null() } else { PCWSTR(value.as_ptr()) };
    let (mut ty, mut len) = (REG_VALUE_TYPE(0), 0u32);
    unsafe {
        let e = RegGetValueW(root, &key, name, RRF_RT_ANY, Some(&mut ty), None, Some(&mut len));
        if e.is_err() {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        RegGetValueW(root, &key, name, RRF_RT_ANY, Some(&mut ty), Some(buf.as_mut_ptr() as _), Some(&mut len)).ok().ok()?;
        buf.truncate(len as usize);
        Some((ty, buf))
    }
}

fn wide(b: &[u8]) -> Vec<String> {
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    w.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

/// `PKEY_AudioEngine_DeviceFormat` guardado no registro: 8 bytes de cabeçalho e o WAVEFORMATEX(TENSIBLE).
fn device_format(id: &str) -> Option<(String, u16, u32, u16)> {
    let (_, b) = reg(HKEY_LOCAL_MACHINE, &format!(r"{MMDEV}\{}\Properties", key_of(id)), "{f19f064d-082c-4e27-bc73-6882a1bb8e4c},0")?;
    let w = b.get(8..)?;
    let u16at = |o: usize| u16::from_le_bytes([w[o], w[o + 1]]);
    let (tag, ch, rate, bits) = (u16at(0), u16at(2), u32::from_le_bytes(w[4..8].try_into().ok()?), u16at(14));
    let sub = if tag == 0xFFFE && w.len() >= 40 { u32::from_le_bytes(w[24..28].try_into().ok()?) } else { tag as u32 };
    let kind = match (sub, w.get(28..30)) {
        (1, _) => "PCM".to_string(),
        (3, _) => "float".to_string(),
        (0x92, _) => "bitstream AC-3 (Dolby Digital)".to_string(),
        (8, _) => "bitstream DTS".to_string(),
        (0x0A, Some([0xEA, 0x0C])) => "bitstream E-AC-3 (Dolby Digital Plus)".to_string(),
        (0x0C, Some([0xEA, 0x0C])) => "bitstream MLP/TrueHD (Dolby MAT)".to_string(),
        (0x0B, Some([0xEA, 0x0C])) => "bitstream DTS-HD".to_string(),
        (s, _) => format!("subtipo {s:#x}"),
    };
    Some((kind, ch, rate, bits))
}

fn clsid_name(clsid: &str) -> String {
    match reg(HKEY_CLASSES_ROOT, &format!(r"CLSID\{clsid}"), "") {
        Some((_, b)) => wide(&b).into_iter().next().unwrap_or_else(|| clsid.to_string()),
        None => clsid.to_string(),
    }
}

/// APOs registrados no aparelho (FxProperties) e se as melhorias estão desligadas.
fn effects(id: &str) {
    let key = format!(r"{MMDEV}\{}\FxProperties", key_of(id));
    let off = reg(HKEY_LOCAL_MACHINE, &key, "{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5")
        .and_then(|(_, b)| b.first().copied())
        .unwrap_or(0);
    let mut list = Vec::new();
    for (n, label) in FX_KEYS {
        if let Some((_, b)) = reg(HKEY_LOCAL_MACHINE, &key, &format!("{FX},{n}")) {
            for c in wide(&b) {
                list.push(format!("{label}: {}", clsid_name(&c)));
            }
        }
    }
    let state = if off == 1 { "melhorias DESLIGADAS (Windows não carrega os APOs abaixo)" } else { "melhorias ligadas" };
    println!("   efeitos: {state}");
    for l in list {
        println!("     - {l}");
    }
}

/// O id do aparelho é `{0.0.0.00000000}.{guid}`; no registro a chave é só o `{guid}`.
fn key_of(id: &str) -> &str {
    id.rsplit('.').next().unwrap_or(id)
}

#[cfg(test)]
mod tests {
    #[test]
    fn registry_helpers() {
        assert_eq!(super::key_of("{0.0.0.00000000}.{4defefca-800a}"), "{4defefca-800a}");
        let b: Vec<u8> = "ab\0cd\0\0".encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
        assert_eq!(super::wide(&b), ["ab", "cd"]);
    }
}
