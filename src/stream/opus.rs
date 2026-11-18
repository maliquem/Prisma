//! Som do Compartilhar tela fora da rede local: Opus CELT de 5 ms (RESTRICTED_LOWDELAY, sem atraso
//! extra), uns 128 kbit/s em vez dos 1,5 Mbit/s do PCM. Na rede local o som continua PCM.

use opus_head_sys as opus;
use std::ptr::null_mut;

use super::AUDIO_BLOCK;

const BITRATE: i32 = 128_000;
/// Maior pacote aceito: a 128 kbit/s um bloco de 5 ms dá uns 80 bytes.
const MAX: usize = 400;
/// Até quantos blocos perdidos seguidos o decodificador inventa (PLC); acima disso, só silêncio.
const PLC_MAX: u32 = 4;

/// Codifica blocos de 5 ms (240 amostras estéreo intercaladas). Abre a libopus no primeiro uso.
pub struct Encoder(*mut opus::OpusEncoder);
// O ponteiro é só desta linha de som.
unsafe impl Send for Encoder {}

impl Default for Encoder {
    fn default() -> Self {
        Encoder(null_mut())
    }
}

impl Encoder {
    pub fn encode(&mut self, pcm: &[i16]) -> Option<Vec<u8>> {
        if pcm.len() != AUDIO_BLOCK * 2 {
            return None;
        }
        unsafe {
            if self.0.is_null() {
                let mut err = 0;
                let e = opus::opus_encoder_create(48_000, 2, opus::OPUS_APPLICATION_RESTRICTED_LOWDELAY as i32, &mut err);
                if e.is_null() || err != 0 {
                    return None;
                }
                opus::opus_encoder_ctl(e, opus::OPUS_SET_BITRATE_REQUEST as i32, BITRATE);
                self.0 = e;
            }
            let mut out = vec![0u8; MAX];
            let n = opus::opus_encode(self.0, pcm.as_ptr(), AUDIO_BLOCK as i32, out.as_mut_ptr(), MAX as i32);
            (n > 0).then(|| {
                out.truncate(n as usize);
                out
            })
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { opus::opus_encoder_destroy(self.0) };
        }
    }
}

/// Decodifica os blocos na ordem do `seq`; nas lacunas curtas o PLC da libopus preenche.
pub struct Decoder {
    d: *mut opus::OpusDecoder,
    next: Option<u32>,
}
unsafe impl Send for Decoder {}

impl Default for Decoder {
    fn default() -> Self {
        Decoder { d: null_mut(), next: None }
    }
}

impl Decoder {
    /// As amostras do bloco `seq` (i16 estéreo intercaladas), com as dos blocos perdidos antes dele.
    /// Um bloco atrasado (que o PLC já cobriu) volta vazio.
    pub fn decode(&mut self, seq: u32, data: &[u8]) -> Vec<i16> {
        let gap = self.next.map_or(0, |n| seq.wrapping_sub(n));
        if gap >= 1 << 31 {
            return Vec::new();
        }
        self.next = Some(seq.wrapping_add(1));
        unsafe {
            if self.d.is_null() {
                let mut err = 0;
                let d = opus::opus_decoder_create(48_000, 2, &mut err);
                if d.is_null() || err != 0 {
                    return Vec::new();
                }
                self.d = d;
            }
            let lost = if gap <= PLC_MAX { gap as usize } else { 0 };
            let mut out = vec![0i16; (lost + 1) * AUDIO_BLOCK * 2];
            for (i, b) in out.chunks_exact_mut(AUDIO_BLOCK * 2).enumerate() {
                let (p, len) = if i < lost { (std::ptr::null(), 0) } else { (data.as_ptr(), data.len() as i32) };
                if opus::opus_decode(self.d, p, len, b.as_mut_ptr(), AUDIO_BLOCK as i32, 0) != AUDIO_BLOCK as i32 {
                    b.fill(0);
                }
            }
            out
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        if !self.d.is_null() {
            unsafe { opus::opus_decoder_destroy(self.d) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(block: u32) -> Vec<i16> {
        (0..AUDIO_BLOCK as u32)
            .flat_map(|i| {
                let t = (block * AUDIO_BLOCK as u32 + i) as f32 / 48_000.0;
                let v = ((t * 440.0 * std::f32::consts::TAU).sin() * 12_000.0) as i16;
                [v, v]
            })
            .collect()
    }

    #[test]
    fn tone_survives_and_gaps_are_filled() {
        let (mut e, mut d) = (Encoder::default(), Decoder::default());
        let packets: Vec<Vec<u8>> = (0..40).map(|b| e.encode(&tone(b)).unwrap()).collect();
        assert!(packets.iter().all(|p| p.len() < 200), "um bloco de 5 ms cabe com folga num datagrama");
        let mut total = 0;
        let mut energy = 0f64;
        for (seq, p) in packets.iter().enumerate() {
            if seq == 20 || seq == 21 {
                continue; // dois perdidos: o bloco 22 traz também os dois do PLC
            }
            let pcm = d.decode(seq as u32, p);
            total += pcm.len();
            if seq > 25 {
                energy += pcm.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
            }
        }
        assert_eq!(total, 40 * AUDIO_BLOCK * 2);
        assert!(energy / (14.0 * 480.0) > 1e6, "o tom chega com volume");
        // Bloco atrasado depois do PLC: nada.
        assert!(d.decode(20, &packets[20]).is_empty());
        assert!(e.encode(&[0; 10]).is_none());
    }
}
