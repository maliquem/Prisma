//! Ritmo de envio do vídeo: um quadro grande (o quadro-chave em 4K tem centenas de pacotes) sai
//! espalhado em 65% do intervalo do quadro, e não numa rajada só, que estoura a fila do roteador
//! Wi-Fi e perde vários pacotes de uma vez. Quadro pequeno sai na hora.

use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::link::Link;

/// Pacotes que saem juntos sem espera: uma rajada desse tamanho (uns 18 KB) cabe em qualquer fila.
const BURST: usize = 16;
/// Parte do intervalo do quadro usada para espalhar o resto.
const SPREAD_PCT: u32 = 65;

/// Um quadro já cortado em pacotes, para estas pessoas. Os pacotes a partir de `parity_from` são
/// a paridade do FEC, só para quem tem `true` ao lado.
struct Batch {
    links: Vec<(Arc<dyn Link>, bool)>,
    packets: Vec<Vec<u8>>,
    parity_from: usize,
    span: Duration,
}

/// A linha que manda os pacotes no ritmo. Fecha sozinha quando o `Pacer` sai de cena.
pub struct Pacer {
    tx: mpsc::Sender<Batch>,
}

/// Quantos pacotes de `n` já deviam ter saído depois de `elapsed`, espalhando em `span`.
pub fn due(n: usize, elapsed: Duration, span: Duration) -> usize {
    if n <= BURST || span.is_zero() || elapsed >= span {
        return n;
    }
    BURST + ((n - BURST) as u128 * elapsed.as_nanos() / span.as_nanos()) as usize
}

impl Pacer {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Batch>();
        std::thread::spawn(move || {
            let mut next = None;
            while let Some(b) = next.take().or_else(|| rx.recv().ok()) {
                let t0 = Instant::now();
                let mut sent = 0;
                while sent < b.packets.len() {
                    // Chegou o quadro seguinte: o resto deste sai já, para não ficar para trás.
                    if next.is_none() {
                        next = rx.try_recv().ok();
                    }
                    let upto = if next.is_some() { b.packets.len() } else { due(b.packets.len(), t0.elapsed(), b.span) };
                    for (i, p) in b.packets.iter().enumerate().take(upto).skip(sent) {
                        for (l, fec) in &b.links {
                            if i < b.parity_from || *fec {
                                l.send(p);
                            }
                        }
                    }
                    sent = upto.max(sent);
                    if sent < b.packets.len() {
                        std::thread::sleep(Duration::from_micros(500));
                    }
                }
            }
        });
        Pacer { tx }
    }

    /// Manda um quadro inteiro; `interval` é o intervalo entre quadros. Com os dois lados no mesmo
    /// PC (sem roteador no meio), passe `interval` zero para sair tudo de uma vez.
    pub fn frame(&self, links: Vec<(Arc<dyn Link>, bool)>, packets: Vec<Vec<u8>>, parity_from: usize, interval: Duration) {
        let span = interval * SPREAD_PCT / 100;
        let _ = self.tx.send(Batch { links, packets, parity_from, span });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn small_frames_go_at_once_big_ones_spread() {
        let span = Duration::from_millis(10);
        assert_eq!(due(5, Duration::ZERO, span), 5);
        assert_eq!(due(116, Duration::ZERO, span), BURST);
        assert_eq!(due(116, Duration::from_millis(5), span), BURST + 50);
        assert_eq!(due(116, span, span), 116);
        assert_eq!(due(116, Duration::from_millis(50), span), 116);
        assert_eq!(due(116, Duration::from_millis(1), Duration::ZERO), 116);
    }

    struct Count(Mutex<Vec<(u8, Instant)>>);
    impl Link for Count {
        fn send(&self, b: &[u8]) {
            self.0.lock().unwrap().push((b[0], Instant::now()));
        }
        fn recv(&self, _: &mut [u8], _: Duration) -> super::super::link::Poll {
            super::super::link::Poll::Empty
        }
    }

    #[test]
    fn every_packet_arrives_in_order_and_spread() {
        let c = Arc::new(Count(Mutex::new(Vec::new())));
        let old = Arc::new(Count(Mutex::new(Vec::new())));
        let p = Pacer::new();
        let pk: Vec<Vec<u8>> = (0..100u8).map(|i| vec![i]).collect();
        let t0 = Instant::now();
        p.frame(vec![(c.clone(), true), (old.clone(), false)], pk, 90, Duration::from_millis(20));
        // Espera até chegar tudo (com folga para a máquina ocupada rodando os outros testes), no lugar de um sono fixo.
        let end = Instant::now() + Duration::from_secs(2);
        while c.0.lock().unwrap().len() < 100 && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        let got = c.0.lock().unwrap();
        assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), (0..100u8).collect::<Vec<_>>());
        // Quem não pediu o FEC fica sem a paridade.
        assert_eq!(old.0.lock().unwrap().len(), 90);
        // O último sai perto de 65% de 20 ms, e não na hora.
        let last = got[99].1 - t0;
        assert!(last >= Duration::from_millis(10), "{last:?}");
    }
}
