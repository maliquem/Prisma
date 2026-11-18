//! Microfone de quem assiste: quem assiste capta a entrada padrão dele e manda em blocos de 10 ms
//! ([`Msg::Mic`]); o host toca no CABLE Input do VB-CABLE, e os programas de lá ouvem o CABLE
//! Output como microfone. Só com a permissão [`MIC`] do aparelho. Host antigo ignora o tipo 27.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::Win32::Media::Audio::eCapture;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use super::view::{play_until, Speaker};
use super::*;
use crate::capture::sound;

/// Abaixo disso (uns -54 dB) o bloco é silêncio e não vai: poupa a rede e solta a vez.
const GATE: i16 = 64;
/// Quem fala fica com a vez até ficar calado por este tempo.
const HOLD: Duration = Duration::from_secs(1);

/// Um bloco estéreo em ponto flutuante vira um canal em 16 bits.
fn mono(lr: &[f32]) -> i16 {
    (((lr[0] + lr[1]) * 0.5).clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Quem assiste: capta a entrada padrão e manda ao host enquanto ele der a permissão, até `quit`.
// shortcut: PCM de 16 bits (~770 kbit/s falando); passar para o Opus quando ele chegar ao compartilhamento.
pub(super) fn send_until(link: Arc<dyn Link>, perms: &AtomicU8, quit: &AtomicBool) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut src: Option<sound::Source> = None;
    let mut raw: Vec<f32> = Vec::new();
    let mut block: Vec<i16> = Vec::with_capacity(MIC_BLOCK);
    let mut bytes = Vec::with_capacity(MIC_BLOCK * 2);
    let mut out = Vec::with_capacity(MTU);
    while !quit.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
        if perms.load(Ordering::Relaxed) & MIC == 0 {
            if let Some(s) = src.take() {
                unsafe {
                    let _ = s.client.Stop();
                }
                eprintln!("microfone: parado (o host não deu a permissão)");
            }
            raw.clear();
            block.clear();
            continue;
        }
        if src.is_none() {
            match unsafe { sound::open_on("", eCapture, false) } {
                Ok(s) => {
                    eprintln!("microfone: mandando a entrada padrão ao host");
                    src = Some(s);
                }
                Err(e) => {
                    eprintln!("microfone: a entrada padrão não abriu: {e:?}");
                    return;
                }
            }
        }
        if let Some(s) = &src {
            unsafe { sound::read(s, &mut raw) };
        }
        for lr in raw.chunks_exact(2) {
            block.push(mono(lr));
            if block.len() == MIC_BLOCK {
                if block.iter().any(|v| v.unsigned_abs() >= GATE as u16) {
                    bytes.clear();
                    bytes.extend(block.iter().flat_map(|v| v.to_le_bytes()));
                    write(&mut out, &Msg::Mic(&bytes));
                    link.send(&out);
                }
                block.clear();
            }
        }
        raw.clear();
    }
    if let Some(s) = src {
        unsafe {
            let _ = s.client.Stop();
        }
    }
}

/// O host: toca no CABLE Input o microfone de uma pessoa por vez.
pub(super) struct Cable {
    spk: Arc<Speaker>,
    quit: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// Quem fala agora (a chave do aparelho) e quando chegou o último bloco.
    owner: Option<(String, Instant)>,
}

impl Cable {
    /// Começa a tocar no CABLE Input; sem o VB-CABLE, `None`.
    pub(super) fn open() -> Option<Cable> {
        let id = crate::sys::vbcable::input_id()?;
        let spk = Arc::new(Speaker::new(false));
        let quit = Arc::new(AtomicBool::new(false));
        let (s2, q2) = (spk.clone(), quit.clone());
        let thread = std::thread::Builder::new().name("compartilhar-microfone".into()).spawn(move || play_until(&s2, &q2, &id)).ok();
        Some(Cable { spk, quit, thread, owner: None })
    }

    /// Um bloco de `who`. Outro falando há menos de [`HOLD`]: o bloco cai.
    // shortcut: uma voz por vez; misturar as vozes se várias pessoas precisarem falar juntas.
    pub(super) fn push(&mut self, who: &str, data: &[u8]) {
        let now = Instant::now();
        if self.owner.as_ref().is_some_and(|(o, t)| o != who && now.duration_since(*t) < HOLD) {
            return;
        }
        if !self.owner.as_ref().is_some_and(|(o, _)| o == who) {
            self.owner = Some((who.to_string(), now));
        }
        if let Some(o) = &mut self.owner {
            o.1 = now;
        }
        let mut q = self.spk.audio.lock().unwrap();
        // Sem a linha que toca (o aparelho sumiu), a fila não cresce para sempre.
        if q.len() > AUDIO_RATE as usize * 2 {
            q.clear();
        }
        for s in data.chunks_exact(2) {
            let v = i16::from_le_bytes([s[0], s[1]]);
            q.extend([v, v]);
        }
    }
}

impl Drop for Cable {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estereo_vira_um_canal() {
        assert_eq!(mono(&[1.0, 1.0]), 32767);
        assert_eq!(mono(&[-2.0, -2.0]), -32767);
        assert_eq!(mono(&[0.5, -0.5]), 0);
    }

    #[test]
    fn uma_voz_por_vez() {
        let mut c = Cable { spk: Arc::new(Speaker::new(false)), quit: Arc::new(AtomicBool::new(false)), thread: None, owner: None };
        c.push("a", &[1, 0, 2, 0]);
        c.push("b", &[3, 0]);
        // Os dois blocos de "a" em estéreo; "b" esperou a vez.
        assert_eq!(c.spk.audio.lock().unwrap().len(), 4);
        // "a" calado por mais de HOLD: "b" fala.
        c.owner.as_mut().unwrap().1 -= HOLD;
        c.push("b", &[3, 0]);
        assert_eq!(c.spk.audio.lock().unwrap().back(), Some(&3));
        assert_eq!(c.owner.as_ref().unwrap().0, "b");
    }
}
