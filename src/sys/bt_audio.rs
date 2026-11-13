//! Som no alto-falante do controle pelo Bluetooth, sem driver: o áudio vai em relatórios HID.
//! Formatos lidos no dsz (github.com/MaximumFreak/dsz, MIT), a única fonte que os descreve;
//! nada copiado. Detalhes e conflitos em `research_notes/bluetooth-audio-haptico.md`.
//! - DualSense: relatório 0x39 com dois quadros Opus de 10 ms (48 kHz, estéreo, 200 bytes cada,
//!   libopus) e os dois blocos do háptico, um a cada 21,3 ms; o volume vai no 0x31.
//! - DualShock 4: relatório 0x17 com quatro quadros SBC (32 kHz, estéreo conjunto, 16 blocos,
//!   8 sub-bandas, SNR, bitpool 48, 109 bytes cada), um a cada 16 ms; o volume vai no 0x11.

use std::time::{Duration, Instant};

use nanoaudio_core::codec::Encoder;
use nanoaudio_sbc::{SbcAllocation, SbcChannelMode, SbcEncoder, SbcParameters};
use opus_head_sys as opus;

use super::pads::{crc32, write_report, Family};

/// Volume do alto-falante que o aviso pede: perto do que o driver do Linux usa (0x64 de 0xFF) no
/// DualSense, e a mesma proporção na faixa de 0 a 127 do DualShock 4.
const DS_VOLUME: u8 = 0x64;
const DS4_VOLUME: u8 = 0x32;

/// Toca `mono` (amostras de -1 a 1 na taxa que `rate_of` diz) no alto-falante do controle em
/// `path`. Bloqueia até o fim. Devolve false se o controle recusou o relatório.
pub fn play(path: &str, fam: Family, mono: &[f32]) -> bool {
    match fam {
        Family::DualSense => dualsense(path, mono),
        Family::Ds4 => ds4(path, mono),
        _ => false,
    }
}

/// A taxa em que `play` espera as amostras de cada modelo.
pub fn rate_of(fam: Family) -> u32 {
    if fam == Family::Ds4 { 32_000 } else { 48_000 }
}

fn pcm(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Espera até `next` e marca o próximo, sem acumular atraso.
fn pace(next: &mut Instant, period: Duration) {
    *next += period;
    let now = Instant::now();
    if *next > now {
        std::thread::sleep(*next - now);
    } else {
        *next = now;
    }
}

/// Relatório 0x31 só com o volume do alto-falante (o resto com os bits de validade desligados não
/// muda nada no controle). Volume no byte 5 da parte comum, ligado pelo bit 5 do primeiro byte de
/// validade, como no driver do Linux.
fn ds_volume_report(seq: u8, volume: u8) -> [u8; 78] {
    let mut r = [0u8; 78];
    r[0] = 0x31;
    r[1] = (seq & 15) << 4;
    r[2] = 0x10;
    r[3] = 0x20;
    r[3 + 5] = volume;
    let c = crc32(0xA2, &r[..74]);
    r[74..].copy_from_slice(&c.to_le_bytes());
    r
}

/// Relatório 0x39: dois quadros Opus de 200 bytes para o alto-falante e o háptico calado.
fn ds_audio_report(seq: u8, count: u8, frames: [&[u8; 200]; 2]) -> [u8; 547] {
    let mut r = [0u8; 547];
    r[0] = 0x39;
    r[1] = (seq & 15) << 4;
    r[2..9].copy_from_slice(&[0x91, 0x06, 0x7F, 0x51, 0x1F, 0x62, 0x28]);
    r[9] = count;
    // Pacote 0x15: alto-falante (0x16 seria o fone).
    r[10..12].copy_from_slice(&[0xD5, 0xC8]);
    r[12..212].copy_from_slice(frames[0]);
    r[212..412].copy_from_slice(frames[1]);
    r[412..414].copy_from_slice(&[0xD2, 0x40]);
    let c = crc32(0xA2, &r[..543]);
    r[543..].copy_from_slice(&c.to_le_bytes());
    r
}

/// Codifica `mono` em quadros Opus de 10 ms e 200 bytes (CELT, 160 kb/s fixos), como o controle
/// espera: o primeiro byte de cada quadro é 0xF4.
fn opus_frames(mono: &[f32]) -> Option<Vec<[u8; 200]>> {
    unsafe {
        let mut err = 0;
        let enc = opus::opus_encoder_create(48_000, 2, opus::OPUS_APPLICATION_RESTRICTED_LOWDELAY as i32, &mut err);
        if enc.is_null() || err != 0 {
            return None;
        }
        opus::opus_encoder_ctl(enc, opus::OPUS_SET_BITRATE_REQUEST as i32, 160_000i32);
        opus::opus_encoder_ctl(enc, opus::OPUS_SET_VBR_REQUEST as i32, 0i32);
        let mut out = vec![];
        let mut pcm_in = [0i16; 960];
        for chunk in mono.chunks(480) {
            pcm_in.fill(0);
            for (i, v) in chunk.iter().enumerate() {
                pcm_in[2 * i] = pcm(*v);
                pcm_in[2 * i + 1] = pcm(*v);
            }
            let mut f = [0u8; 200];
            let n = opus::opus_encode(enc, pcm_in.as_ptr(), 480, f.as_mut_ptr(), 200);
            if n != 200 {
                opus::opus_encoder_destroy(enc);
                return None;
            }
            out.push(f);
        }
        opus::opus_encoder_destroy(enc);
        Some(out)
    }
}

fn dualsense(path: &str, mono: &[f32]) -> bool {
    let Some(mut frames) = opus_frames(mono) else { return false };
    // Três relatórios calados antes, como o dsz: o controle liga o alto-falante sem cortar o começo.
    let silence = opus_frames(&[0.0; 480 * 6]).unwrap_or_default();
    frames.splice(0..0, silence);
    if frames.len() % 2 == 1 {
        frames.push(frames[frames.len() - 1]);
    }
    unsafe {
        if !write_report(path, &ds_volume_report(0, DS_VOLUME), 0) {
            return false;
        }
    }
    let period = Duration::from_nanos(21_333_333);
    let mut next = Instant::now();
    for (i, pair) in frames.chunks_exact(2).enumerate() {
        let r = ds_audio_report(i as u8, (2 * i) as u8, [&pair[0], &pair[1]]);
        if !unsafe { write_report(path, &r, 0) } {
            return false;
        }
        pace(&mut next, period);
    }
    true
}

/// Relatório 0x11 do DualShock 4 só com o áudio ligado e o volume do alto-falante (fone calado).
fn ds4_volume_report(volume: u8) -> [u8; 78] {
    let mut r = [0u8; 78];
    r[0] = 0x11;
    r[1] = 0xC0;
    // Áudio ligado e os quatro volumes valendo; luz e vibração ficam de fora (bits 0 e 1).
    r[2] = 0xA0;
    r[3] = 0xF0;
    r[24] = volume.min(127);
    let c = crc32(0xA2, &r[..74]);
    r[74..].copy_from_slice(&c.to_le_bytes());
    r
}

/// Relatório 0x17: quatro quadros SBC para o alto-falante.
fn ds4_audio_report(count: u16, frames: &[u8; 436]) -> [u8; 462] {
    let mut r = [0u8; 462];
    r[0] = 0x17;
    r[1] = 0x40 | 4;
    r[2] = 0xA0;
    r[3..5].copy_from_slice(&count.to_le_bytes());
    // 0x02 alto-falante; 0x24 seria o fone.
    r[5] = 0x02;
    r[6..442].copy_from_slice(frames);
    let c = crc32(0xA2, &r[..458]);
    r[458..].copy_from_slice(&c.to_le_bytes());
    r
}

/// Codifica `mono` (32 kHz) em quadros SBC de 109 bytes, quatro por relatório.
fn sbc_frames(mono: &[f32]) -> Option<Vec<[u8; 436]>> {
    let p = SbcParameters { sample_rate: 32_000, channel_mode: SbcChannelMode::JointStereo, allocation: SbcAllocation::Snr, blocks: 16, subbands: 8, bitpool: 48 };
    let mut enc = SbcEncoder::new(p).ok()?;
    let mut out = vec![];
    // Quatro quadros de 128 amostras por canal.
    for chunk in mono.chunks(512) {
        let mut st = [0i16; 1024];
        for (i, v) in chunk.iter().enumerate() {
            st[2 * i] = pcm(*v);
            st[2 * i + 1] = pcm(*v);
        }
        let mut block = [0u8; 436];
        let (mut used, mut made) = (0, 0);
        while used < st.len() {
            let pr = enc.encode(&st[used..], &mut block[made..]).ok()?;
            if pr.consumed == 0 {
                return None;
            }
            used += pr.consumed;
            made += pr.produced;
        }
        (made == 436).then_some(())?;
        out.push(block);
    }
    Some(out)
}

fn ds4(path: &str, mono: &[f32]) -> bool {
    let Some(frames) = sbc_frames(mono) else { return false };
    unsafe {
        if !write_report(path, &ds4_volume_report(DS4_VOLUME), 0) {
            return false;
        }
    }
    let period = Duration::from_millis(16);
    let mut next = Instant::now();
    for (i, f) in frames.iter().enumerate() {
        if !unsafe { write_report(path, &ds4_audio_report((i * 4) as u16, f), 0) } {
            return false;
        }
        pace(&mut next, period);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bluetooth_audio_reports() {
        // Opus: 10 ms viram um quadro de 200 bytes com o TOC que o controle espera (CELT, 10 ms, estéreo).
        let tone: Vec<f32> = (0..960).map(|i| 0.3 * (i as f32 * 0.1).sin()).collect();
        let f = opus_frames(&tone).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0][0], 0xF4);
        let r = ds_audio_report(3, 6, [&f[0], &f[1]]);
        assert_eq!((r[0], r[1], r[9], r[10], r[11], r[12], r[412], r[413]), (0x39, 0x30, 6, 0xD5, 0xC8, 0xF4, 0xD2, 0x40));
        assert_eq!(u32::from_le_bytes(r[543..].try_into().unwrap()), crc32(0xA2, &r[..543]));
        let v = ds_volume_report(1, DS_VOLUME);
        assert_eq!((v[3], v[4], v[8]), (0x20, 0, DS_VOLUME));
        // SBC: 4 quadros de 109 bytes, cada um com o byte de sincronia e os parâmetros pedidos
        // (32 kHz, 16 blocos, estéreo conjunto, SNR, 8 sub-bandas) e o bitpool 48.
        let tone: Vec<f32> = (0..1024).map(|i| 0.3 * (i as f32 * 0.1).sin()).collect();
        let s = sbc_frames(&tone).unwrap();
        assert_eq!(s.len(), 2);
        for k in 0..4 {
            assert_eq!(&s[0][k * 109..k * 109 + 3], &[0x9C, 0x7F, 48]);
        }
        let r = ds4_audio_report(8, &s[0]);
        assert_eq!((r[0], r[1], r[2], r[3], r[5], r[6]), (0x17, 0x44, 0xA0, 8, 0x02, 0x9C));
        let v = ds4_volume_report(200);
        assert_eq!((v[2], v[3], v[21], v[24]), (0xA0, 0xF0, 0, 127));
    }
}
