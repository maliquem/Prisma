//! Corrente de processamento de um fluxo: tudo alocado em `new` (fora do tempo real);
//! `process` só faz conta.

use crate::{Biquad, Profile};

/// Atraso máximo por canal (alinhar caixas de uma sala cabe com folga).
pub const MAX_DELAY_MS: f64 = 50.0;
/// Teto do limitador: o máximo do som. Abaixo dele não mexe, então sem ganho o volume cheio do
/// Windows passa igual.
pub const LIMIT_DB: f64 = 0.0;
/// Tempo para o limitador soltar depois de um pico.
const RELEASE_MS: f64 = 120.0;

/// Nomes dos canais na ordem dos bits da máscara do Windows (`SPEAKER_FRONT_LEFT` = bit 0...).
/// Máscara 0: o padrão do Windows para aquela quantidade de canais.
pub fn channel_names(mask: u32, count: usize) -> Vec<&'static str> {
    const NAMES: [&str; 18] = ["L", "R", "C", "LFE", "RL", "RR", "FLC", "FRC", "BC", "SL", "SR", "TC", "TFL", "TFC", "TFR", "TBL", "TBC", "TBR"];
    let mask = match (mask, count) {
        (0, 1) => 0x4,
        (0, 2) => 0x3,
        (0, 6) => 0x3F,
        (0, 8) => 0x63F,
        (m, _) => m,
    };
    let mut names: Vec<&str> = (0..18).filter(|b| mask & (1 << b) != 0).map(|b| NAMES[b]).collect();
    // Canais além da máscara ficam sem nome: só os filtros comuns valem neles.
    names.resize(count, "");
    names.truncate(count);
    names
}

struct Channel {
    filters: Vec<Biquad>,
    gain: f64,
    delay: Vec<f32>,
    pos: usize,
}

/// Limitador de pico sem antecipação (não soma atraso): baixa na hora o ganho do quadro que passaria
/// do teto e solta devagar. O mesmo ganho em todos os canais, para não mexer na posição do som.
struct Limiter {
    ceiling: f32,
    /// Maior pico recente, caindo pela soltura; nunca menor que o teto.
    env: f32,
    release: f32,
}

impl Limiter {
    fn new(rate: f64) -> Limiter {
        let ceiling = 10f64.powf(LIMIT_DB / 20.0) as f32;
        Limiter { ceiling, env: ceiling, release: (-1.0 / (RELEASE_MS / 1000.0 * rate)).exp() as f32 }
    }

    fn run(&mut self, frame: &mut [f32]) {
        let peak = frame.iter().fold(0f32, |m, v| m.max(v.abs()));
        self.env = (self.env * self.release).max(peak).max(self.ceiling);
        if self.env > self.ceiling {
            let g = self.ceiling / self.env;
            frame.iter_mut().for_each(|v| *v *= g);
        }
    }
}

pub struct Chain {
    channels: Vec<Channel>,
    limiter: Option<Limiter>,
}

impl Chain {
    pub fn new(profile: &Profile, mask: u32, count: usize, rate: f64) -> Chain {
        let channels = channel_names(mask, count)
            .into_iter()
            .map(|name| {
                let filters = profile.filters_for(name).iter().map(|f| Biquad::new(f.kind, f.freq, f.gain_db, f.q, rate)).collect();
                let (gain_db, delay_ms) = profile.gain_delay_for(name);
                let samples = (delay_ms.clamp(0.0, MAX_DELAY_MS) * rate / 1000.0).round() as usize;
                Channel { filters, gain: 10f64.powf(gain_db / 20.0), delay: vec![0.0; samples], pos: 0 }
            })
            .collect();
        Chain { channels, limiter: profile.limiter.then(|| Limiter::new(rate)) }
    }

    /// Nada a fazer (perfil vazio): o APO pode só copiar. O limitador não conta: sem ganho, o som do
    /// Windows já chega abaixo do máximo.
    pub fn is_flat(&self) -> bool {
        self.channels.iter().all(|c| c.filters.is_empty() && c.gain == 1.0 && c.delay.is_empty())
    }

    /// Processa quadros intercalados (L R L R ...) no lugar.
    pub fn process(&mut self, data: &mut [f32]) {
        let n = self.channels.len();
        if n == 0 {
            return;
        }
        for frame in data.chunks_exact_mut(n) {
            for (s, ch) in frame.iter_mut().zip(&mut self.channels) {
                let mut x = *s as f64;
                for f in &mut ch.filters {
                    x = f.run(x);
                }
                let mut y = (x * ch.gain) as f32;
                if !ch.delay.is_empty() {
                    std::mem::swap(&mut y, &mut ch.delay[ch.pos]);
                    ch.pos = (ch.pos + 1) % ch.delay.len();
                }
                *s = y;
            }
            if let Some(l) = &mut self.limiter {
                l.run(frame);
            }
        }
        for f in self.channels.iter_mut().flat_map(|c| &mut c.filters) {
            f.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nomes_pela_mascara() {
        assert_eq!(channel_names(0, 2), ["L", "R"]);
        assert_eq!(channel_names(0x3F, 6), ["L", "R", "C", "LFE", "RL", "RR"]);
        assert_eq!(channel_names(0x60F, 6), ["L", "R", "C", "LFE", "SL", "SR"]);
        assert_eq!(channel_names(0x3, 4), ["L", "R", "", ""]);
    }

    #[test]
    fn perfil_vazio_nao_mexe() {
        let mut c = Chain::new(&Profile::default(), 0x3, 2, 48_000.0);
        assert!(c.is_flat());
        let mut d: Vec<f32> = (0..960).map(|i| (i as f32 * 0.37).sin()).collect();
        let before = d.clone();
        c.process(&mut d);
        assert_eq!(d, before);
    }

    #[test]
    fn ganho_e_atraso_por_canal() {
        let p = Profile::parse("[R] ganho=-6.0206 atraso=1").unwrap(); // 1 ms a 48 kHz = 48 quadros
        let mut c = Chain::new(&p, 0x3, 2, 48_000.0);
        let mut d = vec![0f32; 200];
        d[0] = 1.0; // impulso no L
        d[1] = 1.0; // impulso no R
        c.process(&mut d);
        assert_eq!(d[0], 1.0);
        assert!((d[1 + 2 * 48] - 0.5).abs() < 1e-4, "R sai metade, 48 quadros depois");
        assert_eq!(d[1], 0.0);
    }

    #[test]
    fn limitador_segura_os_picos_e_deixa_o_baixo_subir() {
        let sine = |amp: f32| -> Vec<f32> { (0..48_000).flat_map(|i| [(i as f32 * 0.0577).sin() * amp; 2]).collect() };
        let peak = |d: &[f32]| d.iter().fold(0f32, |m, v| m.max(v.abs()));
        let ceiling = 10f32.powf(LIMIT_DB as f32 / 20.0);
        let p = Profile::parse("preamp=12").unwrap();
        // Volume cheio + 12 dB: nada passa do teto.
        let mut loud = sine(1.0);
        Chain::new(&p, 0x3, 2, 48_000.0).process(&mut loud);
        assert!(peak(&loud) <= ceiling + 1e-6, "pico {}", peak(&loud));
        assert!(peak(&loud) > ceiling * 0.99);
        // Som baixo (-30 dB) sobe os 12 dB inteiros, sem o limitador mexer.
        let mut quiet = sine(0.0316);
        Chain::new(&p, 0x3, 2, 48_000.0).process(&mut quiet);
        assert!((peak(&quiet) / 0.0316 - 3.981).abs() < 0.01, "ganho {}", peak(&quiet) / 0.0316);
        // Desligado: estoura.
        let mut off = sine(1.0);
        Chain::new(&Profile::parse("preamp=12\nlimitador=nao").unwrap(), 0x3, 2, 48_000.0).process(&mut off);
        assert!(peak(&off) > 3.9);
    }

    #[test]
    fn processar_em_pedacos_da_o_mesmo_que_de_uma_vez() {
        // O motor manda blocos de tamanhos variados: o estado tem que atravessar a fronteira.
        let p = Profile::parse("Filter 1: ON PK Fc 300 Hz Gain 5 dB Q 2\n[L] atraso=3").unwrap();
        let src: Vec<f32> = (0..9600).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect();
        let mut whole = src.clone();
        Chain::new(&p, 0x3, 2, 48_000.0).process(&mut whole);
        let mut parts = src.clone();
        let mut c = Chain::new(&p, 0x3, 2, 48_000.0);
        for chunk in parts.chunks_mut(2 * 441) {
            c.process(chunk);
        }
        assert_eq!(whole, parts);
    }
}
