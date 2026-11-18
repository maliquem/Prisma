//! Controle de taxa por atraso: quem assiste mede, o host ajusta o codificador.
//!
//! Quem assiste ([`Meter`]) compara a hora de chegada de cada pedaço de vídeo com a hora da captura
//! no host. O relógio dos dois não precisa bater: só conta quanto o atraso passou do menor atraso
//! visto nos últimos segundos (a fila que cresceu no caminho). A cada [`REPORT_MS`] ele manda esse
//! excesso, a perda de pedaços e o que chegou por segundo ([`super::Msg::Rate`]).
//!
//! O host ([`Rate`]) começa no teto de sempre e só desce quando a fila cresce ou a perda passa do
//! limite; sobe devagar depois de um tempo bom. Quem assiste de uma versão sem o retorno nunca manda
//! nada, e a taxa fica fixa como antes.

use std::collections::BTreeMap;

/// De quanto em quanto tempo quem assiste manda o retorno.
pub const REPORT_MS: i64 = 40;
/// 100 ns por milissegundo, a régua de `super::now()`.
const MS: i64 = 10_000;
/// Janela do menor atraso: curta o bastante para seguir a deriva dos relógios, longa para ver a fila.
const BASE_MS: i64 = 10_000;

/// Retorno de quem assiste.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Report {
    /// Quanto o atraso passou do menor visto, em ms.
    pub delay_ms: u16,
    /// Pedaços de vídeo perdidos, por mil.
    pub loss_pm: u16,
    /// O que chegou de vídeo, em kbit/s.
    pub kbps: u32,
}

/// O lado de quem assiste: mede a fila e a perda.
#[derive(Default)]
pub struct Meter {
    /// Menor atraso (chegada menos captura) da janela atual e da anterior.
    base: (i64, i64),
    base_since: i64,
    /// Soma e número dos excessos da janela de retorno.
    excess: (i64, u32),
    bytes: u64,
    /// Quadro: pedaços que chegaram e quantos ele tem.
    frames: BTreeMap<u32, (u16, u16)>,
    last_report: i64,
}

impl Meter {
    /// Um pedaço de vídeo chegou: quadro, número do pedaço, total, hora da captura no host e agora
    /// (as duas em 100 ns).
    pub fn video(&mut self, frame: u32, count: u16, pts: i64, len: usize, now: i64) {
        let d = now - pts;
        if self.base_since == 0 {
            self.base = (d, d);
            self.base_since = now;
            self.last_report = now;
        }
        if now - self.base_since > BASE_MS * MS {
            self.base = (self.base.1, d);
            self.base_since = now;
        }
        self.base.1 = self.base.1.min(d);
        let base = self.base.0.min(self.base.1);
        self.excess.0 += d - base;
        self.excess.1 += 1;
        self.bytes += len as u64;
        let e = self.frames.entry(frame).or_insert((0, count));
        e.0 = e.0.saturating_add(1);
    }

    /// O retorno, quando já passou [`REPORT_MS`] desde o último.
    pub fn report(&mut self, now: i64) -> Option<Report> {
        let span = now - self.last_report;
        if self.base_since == 0 || span < REPORT_MS * MS || self.excess.1 == 0 {
            return None;
        }
        let delay_ms = (self.excess.0 / self.excess.1 as i64 / MS).clamp(0, u16::MAX as i64) as u16;
        // Só conta perda nos quadros que já deviam ter terminado: todos menos os dois mais novos.
        let newest = *self.frames.keys().next_back()?;
        let (mut got, mut want) = (0u64, 0u64);
        let mut prev: Option<u32> = None;
        let done: Vec<u32> = self.frames.keys().copied().filter(|f| newest.wrapping_sub(*f) >= 2).collect();
        for f in done {
            let (g, c) = self.frames.remove(&f).unwrap_or_default();
            got += g as u64;
            want += c.max(g) as u64;
            // Um quadro inteiro que não apareceu conta como um pedaço perdido.
            if let Some(p) = prev {
                want += f.wrapping_sub(p).saturating_sub(1).min(64) as u64;
            }
            prev = Some(f);
        }
        let loss_pm = if want == 0 { 0 } else { ((want - got) * 1000 / want) as u16 };
        let kbps = (self.bytes * 8 * 10_000 / span.max(1) as u64) as u32;
        self.excess = (0, 0);
        self.bytes = 0;
        self.last_report = now;
        Some(Report { delay_ms, loss_pm, kbps })
    }
}

/// Fila acima disto (ms) é sinal de excesso.
const OVER_MS: f32 = 25.0;
/// Fila abaixo disto (ms) deixa subir.
const CALM_MS: f32 = 8.0;
/// Perda acima disto (por mil) desce; abaixo de [`CALM_LOSS`], deixa subir.
const OVER_LOSS: f32 = 50.0;
const CALM_LOSS: f32 = 10.0;
/// Depois de descer, espera isto antes de descer de novo (a fila leva um tempo para esvaziar).
const HOLD_DOWN_MS: i64 = 400;
/// Depois de descer, espera isto antes de voltar a subir.
const HOLD_UP_MS: i64 = 2_000;
/// Sobe um vigésimo do teto a cada tanto tempo bom.
const STEP_UP_MS: i64 = 500;

/// O lado do host: decide a taxa entre o mínimo e o teto.
pub struct Rate {
    pub max: u32,
    pub min: u32,
    cur: u32,
    /// Atraso e perda suavizados (média móvel), para um pico só não mexer na taxa.
    delay: f32,
    loss: f32,
    last_down: i64,
    last_up: i64,
}

impl Rate {
    /// Começa no teto. O mínimo é um sexto do teto, nunca abaixo de 1 Mbit/s (nem acima do teto).
    pub fn new(max: u32) -> Self {
        Rate { max, min: (max / 6).max(1_000_000).min(max), cur: max, delay: 0.0, loss: 0.0, last_down: i64::MIN / 2, last_up: 0 }
    }

    pub fn current(&self) -> u32 {
        self.cur
    }

    /// Recebe um retorno (`now` em ms). Devolve a taxa nova quando ela muda.
    pub fn feedback(&mut self, r: Report, now: i64) -> Option<u32> {
        self.delay += (r.delay_ms as f32 - self.delay) * 0.3;
        self.loss += (r.loss_pm as f32 - self.loss) * 0.3;
        let old = self.cur;
        if self.delay > OVER_MS || self.loss > OVER_LOSS {
            if now - self.last_down >= HOLD_DOWN_MS {
                // Desce para 85% do atual, ou para perto do que chegou se isso for menor.
                let mut to = self.cur as u64 * 85 / 100;
                if r.kbps > 0 {
                    to = to.min(r.kbps as u64 * 1000 * 9 / 10).max(to / 2);
                }
                self.cur = (to as u32).clamp(self.min, self.max);
                self.last_down = now;
                self.last_up = now;
            }
        } else if self.delay < CALM_MS && self.loss < CALM_LOSS {
            if now - self.last_down >= HOLD_UP_MS && now - self.last_up >= STEP_UP_MS {
                self.cur = self.cur.saturating_add(self.max / 20).min(self.max);
                self.last_up = now;
            }
        } else {
            // Entre os dois: segura, sem subir nem descer.
            self.last_up = now;
        }
        (self.cur != old).then_some(self.cur)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> Report {
        Report { delay_ms: 2, loss_pm: 0, kbps: 20_000 }
    }
    fn bad() -> Report {
        Report { delay_ms: 60, loss_pm: 0, kbps: 8_000 }
    }

    #[test]
    fn rede_boa_fica_no_teto() {
        let mut r = Rate::new(30_000_000);
        for t in 0..200 {
            assert_eq!(r.feedback(good(), t * 40), None);
        }
        assert_eq!(r.current(), 30_000_000);
    }

    #[test]
    fn fila_crescendo_desce_e_respeita_o_minimo() {
        let mut r = Rate::new(30_000_000);
        let mut t = 0;
        for _ in 0..400 {
            r.feedback(bad(), t);
            t += 40;
        }
        assert_eq!(r.current(), r.min);
        assert_eq!(r.min, 5_000_000);
    }

    #[test]
    fn um_pico_so_nao_mexe() {
        let mut r = Rate::new(30_000_000);
        for t in 0..50 {
            r.feedback(good(), t * 40);
        }
        assert_eq!(r.feedback(Report { delay_ms: 60, ..good() }, 2_000), None);
    }

    #[test]
    fn sobe_devagar_depois_de_descer_sem_oscilar() {
        let mut r = Rate::new(30_000_000);
        let (mut t, mut cut) = (0, 0);
        for _ in 0..30 {
            if r.feedback(bad(), t).is_some() {
                cut = t;
            }
            t += 40;
        }
        let low = r.current();
        assert!(low < 30_000_000);
        // Logo depois de descer não sobe; depois sobe um passo por vez e nunca passa do teto.
        let mut changes = Vec::new();
        for _ in 0..1000 {
            if let Some(b) = r.feedback(good(), t) {
                changes.push((t, b));
            }
            t += 40;
        }
        assert!(changes.first().unwrap().0 - cut >= HOLD_UP_MS);
        assert!(changes.windows(2).all(|w| w[1].1 > w[0].1 && w[1].0 - w[0].0 >= STEP_UP_MS));
        assert_eq!(r.current(), 30_000_000);
    }

    #[test]
    fn medidor_ve_fila_e_perda() {
        let mut m = Meter::default();
        let mut now = 1_000_000_000i64;
        // 10 quadros de 4 pedaços, chegando 5 ms depois da captura (o relógio do host está 3 s atrás).
        for f in 0..10u32 {
            let pts = now - 30_000_000;
            for i in 0..4 {
                if !(f == 3 && i == 2) {
                    m.video(f, 4, pts, 1000, pts + 30_000_000 + 5 * MS);
                }
            }
            now += 16 * MS;
        }
        let r = m.report(now).unwrap();
        assert_eq!(r.delay_ms, 0);
        // 8 quadros terminados, 32 pedaços, 1 perdido.
        assert_eq!(r.loss_pm, 31);
        // Agora a fila cresce 40 ms.
        for f in 10..14u32 {
            let pts = now;
            for _ in 0..4 {
                m.video(f, 4, pts, 1000, pts + 30_000_000 + 45 * MS);
            }
            now += 16 * MS;
        }
        let r = m.report(now).unwrap();
        assert_eq!(r.delay_ms, 40);
    }
}
