//! Filtros de segunda ordem (fórmulas do "Audio EQ Cookbook", R. Bristow-Johnson), em f64.

use std::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// Pico (sino): `PK`.
    Peak,
    /// Prateleira de graves: `LSC`/`LS`.
    LowShelf,
    /// Prateleira de agudos: `HSC`/`HS`.
    HighShelf,
    /// Passa-baixa: `LP`/`LPQ`.
    LowPass,
    /// Passa-alta: `HP`/`HPQ`.
    HighPass,
}

/// Coeficientes já divididos por a0, e o estado (forma direta transposta II).
#[derive(Clone, Copy, Debug, Default)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    s1: f64,
    s2: f64,
}

impl Biquad {
    pub fn new(kind: Kind, freq: f64, gain_db: f64, q: f64, rate: f64) -> Self {
        // Frequência acima de Nyquist vira instável: segura um pouco abaixo.
        let freq = freq.clamp(1.0, rate * 0.49);
        let q = if q > 0.0 { q } else { std::f64::consts::FRAC_1_SQRT_2 };
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * PI * freq / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let sq = 2.0 * a.sqrt() * alpha;
        let (b0, b1, b2, a0, a1, a2) = match kind {
            Kind::Peak => (1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a),
            Kind::LowShelf => (
                a * ((a + 1.0) - (a - 1.0) * cos + sq),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - sq),
                (a + 1.0) + (a - 1.0) * cos + sq,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - sq,
            ),
            Kind::HighShelf => (
                a * ((a + 1.0) + (a - 1.0) * cos + sq),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - sq),
                (a + 1.0) - (a - 1.0) * cos + sq,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - sq,
            ),
            Kind::LowPass => ((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            Kind::HighPass => ((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
        };
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, s1: 0.0, s2: 0.0 }
    }

    #[inline]
    pub fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.s1;
        self.s1 = self.b1 * x - self.a1 * y + self.s2;
        self.s2 = self.b2 * x - self.a2 * y;
        y
    }

    /// Zera restos minúsculos do estado (denormais deixam a CPU lenta no silêncio).
    #[inline]
    pub fn flush(&mut self) {
        if self.s1.abs() < 1e-25 {
            self.s1 = 0.0;
        }
        if self.s2.abs() < 1e-25 {
            self.s2 = 0.0;
        }
    }

    /// Ganho em dB na frequência `freq`: o que a página desenha.
    pub fn response_db(&self, freq: f64, rate: f64) -> f64 {
        let w = 2.0 * PI * freq / rate;
        let (c1, s1) = (w.cos(), -w.sin());
        let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num = ((self.b0 + self.b1 * c1 + self.b2 * c2).powi(2) + (self.b1 * s1 + self.b2 * s2).powi(2)).sqrt();
        let den = ((1.0 + self.a1 * c1 + self.a2 * c2).powi(2) + (self.a1 * s1 + self.a2 * s2).powi(2)).sqrt();
        20.0 * (num / den).log10()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    #[test]
    fn pico_tem_o_ganho_no_centro_e_nada_longe() {
        let f = Biquad::new(Kind::Peak, 1000.0, 6.0, 1.0, RATE);
        assert!((f.response_db(1000.0, RATE) - 6.0).abs() < 0.01);
        assert!(f.response_db(20.0, RATE).abs() < 0.1);
        assert!(f.response_db(18_000.0, RATE).abs() < 0.2);
    }

    #[test]
    fn prateleiras_e_cortes() {
        let ls = Biquad::new(Kind::LowShelf, 100.0, 4.0, 0.7, RATE);
        assert!((ls.response_db(20.0, RATE) - 4.0).abs() < 0.2);
        assert!(ls.response_db(5000.0, RATE).abs() < 0.1);
        let hs = Biquad::new(Kind::HighShelf, 8000.0, -3.0, 0.7, RATE);
        assert!((hs.response_db(20_000.0, RATE) + 3.0).abs() < 0.3);
        assert!(hs.response_db(200.0, RATE).abs() < 0.1);
        let hp = Biquad::new(Kind::HighPass, 80.0, 0.0, 0.707, RATE);
        assert!((hp.response_db(80.0, RATE) + 3.0).abs() < 0.1);
        assert!(hp.response_db(20.0, RATE) < -20.0);
        let lp = Biquad::new(Kind::LowPass, 120.0, 0.0, 0.707, RATE);
        assert!(lp.response_db(1000.0, RATE) < -30.0);
    }

    #[test]
    fn seno_sai_com_o_ganho_da_curva() {
        // A conta que toca é a mesma que desenha: medir um seno no centro do pico.
        let mut f = Biquad::new(Kind::Peak, 1000.0, 6.0, 1.0, RATE);
        let mut peak = 0f64;
        for i in 0..48_000 {
            let y = f.run((2.0 * PI * 1000.0 * i as f64 / RATE).sin());
            if i > 24_000 {
                peak = peak.max(y.abs());
            }
        }
        assert!((20.0 * peak.log10() - 6.0).abs() < 0.05, "pico {peak}");
    }
}
