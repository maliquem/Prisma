//! Som do PC para o háptico. Uma linha lê o som que o Windows toca (loopback da saída padrão),
//! tira os agudos e entrega a cada controle do jeito que ele aceita:
//! - DualSense pelo cabo: os canais 3 e 4 da saída de som do controle são os atuadores. A linha
//!   escreve neles em modo compartilhado (o fone e o alto-falante do controle continuam livres).
//! - DualSense pelo Bluetooth: amostras de 3 kHz, 8 bits, que a linha do 0x32 (`pads::coil_thread`)
//!   mistura ao que já manda. Baixa fidelidade: o 0x32 só leva 3 kHz.
//! - O resto (DualShock 4, ou o cabo sem a saída de 4 canais): a força do som move o motor fraco.
//! A linha só existe enquanto algum controle pede, e para 2 s depois do último pedido.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;

/// Força pedida (0 a 8, a maior entre os controles) e quando foi o último pedido.
static DEMAND: Mutex<Option<(u8, Instant)>> = Mutex::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);
/// Força do som agora, de 0 a 255, já com a força pedida: vai ao motor.
static ENV: AtomicU8 = AtomicU8::new(0);
/// A linha está escrevendo nos atuadores de algum DualSense pelo cabo.
static USB: AtomicBool = AtomicBool::new(false);
/// Amostras de 3 kHz para o Bluetooth, com o índice absoluto da última.
static RING: Mutex<(u64, VecDeque<i8>)> = Mutex::new((0, VecDeque::new()));

const LOOP_RATE: u32 = 48_000;
const BT_RATE: u32 = 3_000;
/// O que o Bluetooth guarda: 100 ms.
const RING_LEN: usize = 300;

/// Um controle pede o som do PC no háptico com a força `strength` (0 a 8). Chamado a cada leitura
/// do controle; a linha começa no primeiro pedido.
pub fn demand(strength: u8) {
    if strength == 0 {
        return;
    }
    {
        let mut d = DEMAND.lock().unwrap();
        let now = Instant::now();
        *d = Some(match *d {
            // Pedido de outro controle há menos de 100 ms: fica a maior força.
            Some((s, t)) if now.duration_since(t) < Duration::from_millis(100) => (s.max(strength), now),
            _ => (strength, now),
        });
    }
    if !RUNNING.swap(true, Ordering::AcqRel) && std::thread::Builder::new().name("som para háptico".into()).spawn(run).is_err() {
        RUNNING.store(false, Ordering::Release);
    }
}

/// Força do som agora para o motor (0 a 255).
pub fn envelope() -> u8 {
    if RUNNING.load(Ordering::Relaxed) { ENV.load(Ordering::Relaxed) } else { 0 }
}

/// Os DualSense no cabo já recebem o som nos atuadores: o motor não precisa repetir.
pub fn usb_active() -> bool {
    RUNNING.load(Ordering::Relaxed) && USB.load(Ordering::Relaxed)
}

/// Copia para `out` as próximas amostras de 3 kHz a partir de `cursor` (de cada leitor), com zero
/// no que faltar. Leitor atrasado ou adiantado pula para perto do fim: atraso curto.
pub fn take(cursor: &mut u64, out: &mut [i8]) {
    let r = RING.lock().unwrap();
    let (end, buf) = (&r.0, &r.1);
    let start = end - buf.len() as u64;
    if *cursor < start || *cursor > *end || end - *cursor > 2 * out.len() as u64 {
        *cursor = end.saturating_sub(out.len() as u64).max(start);
    }
    for o in out.iter_mut() {
        *o = if *cursor < *end { buf[(*cursor - start) as usize] } else { 0 };
        if *cursor < *end {
            *cursor += 1;
        }
    }
}

/// Filtro do som: média das duas pistas, passa-baixa de um polo (só o grave e o médio, que é o que
/// o atuador sente) e a força pedida.
struct Shape {
    lp: f32,
    a: f32,
    gain: f32,
}

impl Shape {
    fn new(rate: u32, cutoff: f32) -> Self {
        Shape { lp: 0.0, a: 1.0 - (-std::f32::consts::TAU * cutoff / rate as f32).exp(), gain: 0.0 }
    }
    fn next(&mut self, l: f32, r: f32) -> f32 {
        self.lp += self.a * ((l + r) * 0.5 - self.lp);
        (self.lp * self.gain).clamp(-1.0, 1.0)
    }
}

/// A força pedida (1 a 8) em ganho: 8 dobra o som, que no loopback costuma vir baixo.
fn gain(strength: u8) -> f32 {
    strength.min(8) as f32 / 4.0
}

/// Saídas de som dos DualSense no cabo: as de controle com 4 canais.
unsafe fn usb_pads(en: &IMMDeviceEnumerator) -> Vec<(IAudioClient, IAudioRenderClient, usize, u32)> {
    let mut out = vec![];
    // Se o próprio controle é a saída padrão, o loopback leria o que a linha escreve nele.
    let default = en.GetDefaultAudioEndpoint(eRender, eConsole).ok().and_then(|d| d.GetId().ok()).map(|p| {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    });
    let Ok(list) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return out };
    for i in 0..list.GetCount().unwrap_or(0) {
        let Ok(d) = list.Item(i) else { continue };
        let name = d.OpenPropertyStore(STGM_READ).and_then(|s| s.GetValue(&PKEY_Device_FriendlyName)).map(|v| v.to_string()).unwrap_or_default();
        if !super::pad_sound::is_pad_endpoint(&name) {
            continue;
        }
        let id = d.GetId().ok().map(|p| {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        });
        if id.is_some() && id == default {
            continue;
        }
        let Ok(client) = d.Activate::<IAudioClient>(CLSCTX_ALL, None) else { continue };
        let Ok(format) = client.GetMixFormat() else { continue };
        let (ch, bits) = ((*format).nChannels as usize, (*format).wBitsPerSample);
        // Duas pistas é o DualShock 4 (sem atuadores); só o de 4 tem os canais do háptico.
        let opened = ch == 4 && bits == 32 && client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, 1_000_000, 0, format, None).is_ok();
        CoTaskMemFree(Some(format as *const _));
        if !opened {
            continue;
        }
        let Ok(render) = client.GetService::<IAudioRenderClient>() else { continue };
        let Ok(size) = client.GetBufferSize() else { continue };
        if client.Start().is_ok() {
            out.push((client, render, ch, size));
        }
    }
    out
}

fn run() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = work();
        CoUninitialize();
    }
    ENV.store(0, Ordering::Relaxed);
    USB.store(false, Ordering::Relaxed);
    *DEMAND.lock().unwrap() = None;
    RUNNING.store(false, Ordering::Release);
}

unsafe fn work() -> Result<()> {
    let src = crate::capture::sound::open(eRender, true)?;
    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let mut pads = usb_pads(&en);
    let mut found = Instant::now();
    let (mut usb, mut bt) = (Shape::new(LOOP_RATE, 500.0), Shape::new(LOOP_RATE, 500.0));
    let mut acc = (0.0f32, 0u32);
    let mut frames = Vec::new();
    loop {
        std::thread::sleep(Duration::from_millis(10));
        let strength = match *DEMAND.lock().unwrap() {
            Some((s, t)) if t.elapsed() < Duration::from_secs(2) => s,
            _ => break,
        };
        // Controle novo no cabo: procura de novo a cada 3 s.
        if found.elapsed() > Duration::from_secs(3) {
            pads = usb_pads(&en);
            found = Instant::now();
        }
        USB.store(!pads.is_empty(), Ordering::Relaxed);
        usb.gain = gain(strength);
        bt.gain = gain(strength);
        frames.clear();
        crate::capture::sound::read(&src, &mut frames);
        let mono: Vec<f32> = frames.chunks_exact(2).map(|f| usb.next(f[0], f[1])).collect();
        // Força para o motor: o pico dos últimos 10 ms.
        let peak = mono.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        ENV.store((peak * 255.0).round().min(255.0) as u8, Ordering::Relaxed);
        // 48 kHz para 3 kHz: média de 16 amostras, depois de um passa-baixa próprio.
        {
            let mut r = RING.lock().unwrap();
            for f in frames.chunks_exact(2) {
                acc.0 += bt.next(f[0], f[1]);
                acc.1 += 1;
                if acc.1 == LOOP_RATE / BT_RATE {
                    r.1.push_back((acc.0 / acc.1 as f32 * 127.0).round() as i8);
                    r.0 += 1;
                    acc = (0.0, 0);
                }
            }
            while r.1.len() > RING_LEN {
                r.1.pop_front();
            }
        }
        // Atuadores dos DualSense no cabo: canais 3 e 4. O loopback sai a 48 kHz e a saída do
        // controle também (a taxa da mistura do Windows); se não for, a escrita ainda sai, só que
        // um pouco mais grave ou aguda.
        pads.retain(|(client, render, ch, size)| {
            let Ok(pad) = client.GetCurrentPadding() else { return false };
            let n = mono.len().min((*size - pad) as usize);
            if n == 0 {
                return true;
            }
            let Ok(p) = render.GetBuffer(n as u32) else { return false };
            let out = std::slice::from_raw_parts_mut(p as *mut f32, n * ch);
            for (frame, v) in out.chunks_exact_mut(*ch).zip(&mono) {
                frame.fill(0.0);
                frame[2] = *v;
                frame[3] = *v;
            }
            render.ReleaseBuffer(n as u32, 0).is_ok()
        });
    }
    // Termina calado: os atuadores não ficam com o resto do último som.
    for (client, _, _, _) in &pads {
        let _ = client.Stop();
    }
    let _ = src.client.Stop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Abre o loopback de verdade: `cargo test --offline loopback_starts_and_stops -- --ignored`.
    #[test]
    #[ignore]
    fn loopback_starts_and_stops() {
        demand(8);
        std::thread::sleep(Duration::from_millis(500));
        assert!(RUNNING.load(Ordering::Relaxed), "a linha do som não ficou de pé");
        // Sem pedido novo, a linha para sozinha em 2 s e solta tudo.
        std::thread::sleep(Duration::from_millis(2600));
        assert!(!RUNNING.load(Ordering::Relaxed) && envelope() == 0 && !usb_active());
    }

    #[test]
    fn ring_readers_keep_their_place() {
        {
            let mut r = RING.lock().unwrap();
            *r = (0, VecDeque::new());
            for i in 0..100 {
                r.1.push_back(i as i8);
                r.0 += 1;
            }
        }
        // Leitor novo começa perto do fim; dois leitores não roubam amostras um do outro.
        let (mut a, mut b) = (0u64, 0u64);
        let mut out = [0i8; 32];
        take(&mut a, &mut out);
        assert_eq!((out[0], out[31], a), (68, 99, 100));
        take(&mut b, &mut out);
        assert_eq!(out[0], 68);
        // Sem amostra nova: calado.
        take(&mut a, &mut out);
        assert!(out.iter().all(|v| *v == 0));
        assert_eq!(a, 100);
        // Graves passam, agudos caem (um polo: 8 kHz sai com cerca de 1/16 da força).
        let mut s = Shape::new(48_000, 500.0);
        s.gain = 1.0;
        let tone = |s: &mut Shape, hz: f32| (0..4800).map(|i| s.next((std::f32::consts::TAU * hz * i as f32 / 48_000.0).sin(), 0.0).abs()).fold(0.0f32, f32::max);
        let low = tone(&mut s, 60.0);
        let high = tone(&mut Shape { lp: 0.0, ..s }, 8000.0);
        assert!(low > 0.4 && high < 0.08, "{low} {high}");
    }
}
