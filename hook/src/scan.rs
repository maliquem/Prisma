//! Analisar: o `cs` do passe do HDR soma num buffer da placa o histograma do quadro do jogo, antes da
//! nossa curva. O buffer nunca zera: cada leitura, alguns quadros depois e sem esperar a GPU, vale pela
//! diferença para a anterior. Daqui saem o pico, o preto, o meio e o brilho da interface que o jogo entrega.

use std::sync::atomic::{AtomicU32, Ordering::*};
use std::sync::{Mutex, MutexGuard};

use crate::Shared;

/// Faixas de 1/8 de oitava a partir de 1/256 nit; a 0 também leva o preto puro.
pub const BINS: usize = 176;
/// Y da cena, maior canal da cena e Y da interface, nessa ordem (como no `cs`).
pub const WORDS: usize = BINS * 3;
pub const BYTES: u64 = (WORDS * 4) as u64;
/// O `cs` roda em grupos de 16x16 lendo um pixel a cada dois: cada grupo cobre 32x32.
pub const TILE: u32 = 32;
/// Faixa de 0,05 nit (abaixo dela é preto) e de 90 nits (branco de papel no SDR).
const DARK: usize = 29;
const WHITE: usize = 116;

static ACC: Mutex<Acc> = Mutex::new(Acc::new(0));

fn acc() -> MutexGuard<'static, Acc> {
    ACC.lock().unwrap_or_else(|e| e.into_inner())
}

/// Nits do meio da faixa `b`.
fn nits(b: usize) -> f32 {
    if b == 0 { 0.0 } else { ((b as f32 + 0.5) / 8.0 - 8.0).exp2() }
}

/// Nits abaixo dos quais fica a fração `q` das amostras.
fn pct(bins: &[u64], q: f64) -> f32 {
    let total: u64 = bins.iter().sum();
    let want = (total as f64 * q).ceil().max(1.0) as u64;
    let mut seen = 0;
    for (b, &v) in bins.iter().enumerate() {
        seen += v;
        if seen >= want {
            return nits(b);
        }
    }
    0.0
}

/// O que o Analisar mediu até agora.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub frames: u32,
    /// Pico: o maior p99,99 do maior canal de um quadro. Preto: o menor p0,5 do Y de um quadro.
    pub peak: f32,
    pub black: f32,
    /// Do total dos quadros: p99,9 e p99 do maior canal, mediana do Y, p95 do Y da interface (0 sem
    /// interface separada) e a fração da cena em branco de papel ou acima.
    pub p999: f32,
    pub p99: f32,
    pub med: f32,
    pub ui: f32,
    pub white: f32,
}

struct Acc {
    gen: u32,
    frames: u32,
    prev: Vec<u32>,
    total: Vec<u64>,
    peak: f32,
    black: f32,
}

impl Acc {
    const fn new(gen: u32) -> Self {
        Self { gen, frames: 0, prev: Vec::new(), total: Vec::new(), peak: 0.0, black: f32::INFINITY }
    }

    /// Soma o quadro `d` se ele conta: fora quadro vazio, parado (igual ao anterior: menu, pausa) e quase
    /// todo preto (tela de carregamento).
    fn frame(&mut self, d: &[u32]) -> bool {
        let wide: Vec<u64> = d.iter().map(|&v| v as u64).collect();
        let y = &wide[..BINS];
        let n: u64 = y.iter().sum();
        let dark: u64 = y[..DARK].iter().sum();
        let still = self.prev == d;
        self.prev = d.to_vec();
        if n == 0 || still || dark * 50 >= n * 49 {
            return false;
        }
        self.peak = self.peak.max(pct(&wide[BINS..2 * BINS], 0.9999));
        self.black = self.black.min(pct(y, 0.005));
        if self.total.is_empty() {
            self.total = vec![0; WORDS];
        }
        for (t, v) in self.total.iter_mut().zip(&wide) {
            *t += v;
        }
        self.frames += 1;
        true
    }

    fn stats(&self) -> Stats {
        if self.frames == 0 {
            return Stats::default();
        }
        let (y, max, hud) = (&self.total[..BINS], &self.total[BINS..2 * BINS], &self.total[2 * BINS..]);
        let n: u64 = y.iter().sum();
        let ui_n: u64 = hud.iter().sum();
        Stats {
            frames: self.frames,
            peak: self.peak,
            black: self.black,
            p999: pct(max, 0.999),
            p99: pct(max, 0.99),
            med: pct(y, 0.5),
            // Interface em menos de 0,1% da tela é ruído da separação.
            ui: if ui_n * 1000 >= (n + ui_n) { pct(hud, 0.95) } else { 0.0 },
            white: max[WHITE..].iter().sum::<u64>() as f32 / n as f32,
        }
    }
}

/// Uma fila de leituras de um proxy: em que medida cada vaga foi escrita e a última soma lida.
pub struct Ring {
    gen: u32,
    marks: Vec<u32>,
    last: Option<Vec<u32>>,
}

impl Ring {
    pub fn new(slots: usize) -> Self {
        Self { gen: 0, marks: vec![0; slots], last: None }
    }

    /// Começo do quadro: o Analisar está ligado. Medida nova esquece as vagas da anterior.
    pub fn start(&mut self, s: &Shared) -> bool {
        let gen = s.hdr_scan.load(Relaxed);
        if gen != self.gen {
            (self.gen, self.last) = (gen, None);
        }
        gen != 0
    }

    /// A vaga já tem uma soma desta medida.
    pub fn ready(&self, slot: usize) -> bool {
        self.marks.get(slot) == Some(&self.gen)
    }

    pub fn wrote(&mut self, slot: usize) {
        if let Some(m) = self.marks.get_mut(slot) {
            *m = self.gen;
        }
    }

    /// Uma soma lida da vaga, na ordem em que a placa as escreveu; `mode` é a saída do jogo.
    pub fn feed(&mut self, s: &Shared, mode: u32, cur: &[u32]) {
        let Some(last) = self.last.replace(cur.to_vec()) else { return };
        let d: Vec<u32> = cur.iter().zip(&last).map(|(a, b)| a.wrapping_sub(*b)).collect();
        let mut acc = acc();
        if acc.gen != self.gen {
            *acc = Acc::new(self.gen);
        }
        if acc.frame(&d) {
            publish(s, self.gen, mode, &acc.stats());
        }
    }
}

fn publish(s: &Shared, gen: u32, mode: u32, st: &Stats) {
    let put = |a: &AtomicU32, v: f32| a.store(v.to_bits(), Relaxed);
    put(&s.scan_peak, st.peak);
    put(&s.scan_black, st.black);
    put(&s.scan_p999, st.p999);
    put(&s.scan_p99, st.p99);
    put(&s.scan_med, st.med);
    put(&s.scan_ui, st.ui);
    put(&s.scan_white, st.white);
    s.scan_mode.store(mode, Relaxed);
    s.scan_frames.store(st.frames, Relaxed);
    s.scan_gen.store(gen, Release);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin(n: f32) -> usize {
        if n <= 0.0 { 0 } else { ((n.log2() + 8.0) * 8.0).clamp(0.0, 175.0) as usize }
    }

    fn hist(y: &[(f32, u32)], max: &[(f32, u32)], hud: &[(f32, u32)]) -> Vec<u32> {
        let mut h = vec![0u32; WORDS];
        for (part, list) in [(0, y), (BINS, max), (2 * BINS, hud)] {
            for &(n, c) in list {
                h[part + bin(n)] += c;
            }
        }
        h
    }

    #[test]
    fn frames_and_percentiles() {
        let mut a = Acc::new(1);
        // Tela de carregamento: quase tudo preto.
        assert!(!a.frame(&hist(&[(0.0, 990), (100.0, 10)], &[(0.0, 990), (100.0, 10)], &[])));
        let f = hist(&[(0.01, 10), (50.0, 980), (300.0, 10)], &[(0.02, 10), (100.0, 980), (800.0, 10)], &[(250.0, 5)]);
        assert!(a.frame(&f));
        // O mesmo quadro de novo: parado.
        assert!(!a.frame(&f));
        let g = hist(&[(0.01, 10), (60.0, 989), (300.0, 1)], &[(0.02, 10), (100.0, 989), (1000.0, 1)], &[(250.0, 5)]);
        assert!(a.frame(&g));
        let st = a.stats();
        assert_eq!(st.frames, 2);
        // O pico é o do quadro mais claro; o preto, o do mais escuro; nits no meio da faixa.
        assert!((st.peak / 1000.0 - 1.0).abs() < 0.1, "{st:?}");
        assert!((st.black / 0.01 - 1.0).abs() < 0.1, "{st:?}");
        assert!((st.med / 55.0 - 1.0).abs() < 0.15, "{st:?}");
        assert!((st.ui / 250.0 - 1.0).abs() < 0.1, "{st:?}");
        assert!(st.white > 0.95 && st.white <= 1.0, "{st:?}");
    }

    #[test]
    fn ring_reads_differences_per_measure() {
        // Só atômicos: zerado é o estado de antes de o Prisma ligar o Analisar.
        let s: Shared = unsafe { std::mem::zeroed() };
        let mut r = Ring::new(2);
        assert!(!r.start(&s));
        s.hdr_scan.store(5, Relaxed);
        assert!(r.start(&s) && !r.ready(0));
        r.wrote(0);
        assert!(r.ready(0));
        // A soma de antes da medida não conta; a diferença para ela é um quadro, também quando o contador dá a volta.
        let base = vec![u32::MAX - 3; WORDS];
        r.feed(&s, 1, &base);
        assert_eq!(s.scan_gen.load(Relaxed), 0);
        let f = hist(&[(50.0, 900), (300.0, 100)], &[(100.0, 1000)], &[]);
        let cur: Vec<u32> = base.iter().zip(&f).map(|(a, b)| a.wrapping_add(*b)).collect();
        r.feed(&s, 1, &cur);
        assert_eq!((s.scan_gen.load(Relaxed), s.scan_frames.load(Relaxed), s.scan_mode.load(Relaxed)), (5, 1, 1));
        assert!((f32::from_bits(s.scan_med.load(Relaxed)) / 50.0 - 1.0).abs() < 0.1);
        // Medida nova esquece as vagas da anterior.
        s.hdr_scan.store(9, Relaxed);
        assert!(r.start(&s) && !r.ready(0));
    }

    #[test]
    fn bins_match_the_shader() {
        // O meio da faixa volta à mesma faixa.
        for b in 1..BINS {
            assert_eq!(bin(nits(b)), b);
        }
        assert_eq!(bin(0.05), DARK);
        assert!(nits(WHITE) >= 90.0 && nits(WHITE - 1) < 90.0);
    }
}
