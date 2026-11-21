//! FEC do vídeo: cada quadro leva pacotes de paridade Reed-Solomon (10 a 20%, conforme a perda que
//! quem assiste mede), e quem assiste refaz os pedaços que se perderam antes de pedir socorro ao
//! host (o `Nack`, que custa uma invalidação ou um quadro-chave).
//!
//! Cada pedaço de paridade cobre os pedaços do quadro como "fragmentos" do mesmo tamanho: dois
//! bytes com o tamanho real do pedaço, o pedaço e zeros até o maior (par, como a biblioteca pede).
//! Só quem avisou no `Hello` ([`super::FEC_OK`]) recebe a paridade; versões anteriores nem a veem.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use super::{write, Msg};

/// Bytes a menos em cada pedaço de vídeo quando a paridade vai junto: o fragmento de paridade leva
/// o tamanho (2) e um byte para ficar par, e o pacote dele tem o mesmo cabeçalho do vídeo.
pub const EXTRA: usize = 4;
/// Quadros guardados para refazer: um pedaço atrasado de um quadro mais velho já não serve.
const KEEP: usize = 8;
/// Janela da medida de perda.
const WINDOW: Duration = Duration::from_secs(1);

/// Porcentagem de paridade para uma perda medida (em milésimos): 10% sem perda, 20% a partir de 5%.
pub fn percent(loss_permille: u16) -> usize {
    (10 + loss_permille as usize / 5).min(20)
}

/// Pacotes de paridade para um quadro de `n` pedaços: ao menos um.
pub fn recovery_count(n: usize, pct: usize) -> usize {
    (n * pct).div_ceil(100).max(1)
}

fn shard_size(max_chunk: usize) -> usize {
    (max_chunk + 2).next_multiple_of(2)
}

fn shard(chunk: &[u8], size: usize) -> Vec<u8> {
    let mut s = Vec::with_capacity(size);
    s.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
    s.extend_from_slice(chunk);
    s.resize(size, 0);
    s
}

/// Os pacotes de paridade de um quadro já cortado em `chunks`, prontos para a rede.
pub fn packets(chunks: &[Vec<u8>], frame: u32, pts: i64, key: bool, pct: usize) -> Vec<Vec<u8>> {
    let n = chunks.len();
    let Some(max) = chunks.iter().map(Vec::len).max() else { return Vec::new() };
    if n > u16::MAX as usize || max + 2 > u16::MAX as usize {
        return Vec::new();
    }
    let size = shard_size(max);
    let rec = recovery_count(n, pct);
    let Ok(parity) = reed_solomon_simd::encode(n, rec, chunks.iter().map(|c| shard(c, size))) else { return Vec::new() };
    parity
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let mut out = Vec::with_capacity(p.len() + super::VIDEO_HEAD);
            write(&mut out, &Msg::Fec { frame, pts, key, count: n as u16, idx: i as u16, rec: rec as u16, data: p });
            out
        })
        .collect()
}

struct Frame {
    frame: u32,
    count: u16,
    key: bool,
    pts: i64,
    data: Vec<Option<Vec<u8>>>,
    /// Pedaços que chegaram pela rede (sem contar os refeitos), para a medida de perda.
    arrived: u16,
    rec: u16,
    parity: Vec<(usize, Vec<u8>)>,
    done: bool,
}

/// O lado de quem assiste: guarda os pedaços e a paridade dos últimos quadros e mede a perda.
pub struct Rx {
    frames: VecDeque<Frame>,
    seen: u64,
    missed: u64,
    since: Instant,
    loss: u16,
}

impl Default for Rx {
    fn default() -> Self {
        Rx { frames: VecDeque::new(), seen: 0, missed: 0, since: Instant::now(), loss: 0 }
    }
}

fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

impl Rx {
    /// Perda de pacotes de vídeo da última janela (1 s), em milésimos, antes do FEC refazer. Quadro
    /// que não mandou nenhum pedaço não entra na conta.
    #[cfg(test)]
    pub fn loss_permille(&self) -> u16 {
        self.loss
    }

    /// Fecha a janela da medida quando ela passou: devolve a perda nova, para avisar o host.
    pub fn tick(&mut self, now: Instant) -> Option<u16> {
        if now.duration_since(self.since) < WINDOW {
            return None;
        }
        self.since = now;
        if self.seen > 0 {
            self.loss = (self.missed * 1000 / self.seen).min(1000) as u16;
        }
        self.seen = 0;
        self.missed = 0;
        Some(self.loss)
    }

    fn slot(&mut self, frame: u32, count: u16, key: bool, pts: i64) -> Option<&mut Frame> {
        if count == 0 {
            return None;
        }
        let pos = match self.frames.iter().position(|f| f.frame == frame) {
            Some(p) => p,
            None => {
                if self.frames.front().is_some_and(|f| !newer(frame, f.frame)) && self.frames.len() >= KEEP {
                    return None;
                }
                while self.frames.len() >= KEEP {
                    let f = self.frames.pop_front().unwrap();
                    self.seen += f.count as u64;
                    self.missed += (f.count - f.arrived) as u64;
                }
                self.frames.push_back(Frame { frame, count, key, pts, data: vec![None; count as usize], arrived: 0, rec: 0, parity: Vec::new(), done: false });
                self.frames.len() - 1
            }
        };
        let f = &mut self.frames[pos];
        (f.count == count).then_some(f)
    }

    /// Um pedaço de vídeo chegou. Devolve os pacotes de vídeo que a paridade já guardada refez.
    pub fn data(&mut self, frame: u32, count: u16, idx: u16, key: bool, pts: i64, chunk: &[u8]) -> Vec<Vec<u8>> {
        let Some(f) = self.slot(frame, count, key, pts) else { return Vec::new() };
        let Some(s) = f.data.get_mut(idx as usize) else { return Vec::new() };
        if s.is_none() {
            *s = Some(chunk.to_vec());
            f.arrived += 1;
        }
        rebuild(f)
    }

    /// Um pedaço de paridade chegou. Devolve os pacotes de vídeo refeitos, se já dá.
    #[allow(clippy::too_many_arguments)]
    pub fn parity(&mut self, frame: u32, count: u16, idx: u16, rec: u16, key: bool, pts: i64, shard: &[u8]) -> Vec<Vec<u8>> {
        let Some(f) = self.slot(frame, count, key, pts) else { return Vec::new() };
        // Mais paridade que pedaços nenhum host manda (até 20%): sem o teto, um host hostil faria
        // guardar 65535 pedaços de paridade por quadro.
        if rec == 0 || rec > count || idx >= rec || (f.rec != 0 && f.rec != rec) || shard.is_empty() || shard.len() % 2 != 0 {
            return Vec::new();
        }
        f.rec = rec;
        if f.parity.iter().all(|p| p.0 != idx as usize) {
            f.parity.push((idx as usize, shard.to_vec()));
        }
        rebuild(f)
    }
}

/// Com pedaços e paridade que somam o quadro inteiro, refaz os que faltam.
fn rebuild(f: &mut Frame) -> Vec<Vec<u8>> {
    let have = f.data.iter().filter(|d| d.is_some()).count();
    if f.done || have == f.count as usize {
        f.done |= have == f.count as usize;
        return Vec::new();
    }
    if f.parity.is_empty() || have + f.parity.len() < f.count as usize {
        return Vec::new();
    }
    f.done = true;
    let size = f.parity[0].1.len();
    if f.parity.iter().any(|p| p.1.len() != size) || f.data.iter().flatten().any(|d| d.len() + 2 > size) {
        return Vec::new();
    }
    let original = f.data.iter().enumerate().filter_map(|(i, d)| d.as_ref().map(|d| (i, shard(d, size))));
    let Ok(back) = reed_solomon_simd::decode(f.count as usize, f.rec as usize, original, f.parity.iter().map(|(i, p)| (*i, p))) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, s) in back {
        let len = u16::from_le_bytes([s[0], s[1]]) as usize;
        let (Some(chunk), Some(slot)) = (s.get(2..2 + len), f.data.get_mut(i)) else { continue };
        if slot.is_none() {
            let mut b = Vec::with_capacity(len + super::VIDEO_HEAD);
            write(&mut b, &Msg::Video { key: f.key, more: false, idx: i as u16, count: f.count, frame: f.frame, pts: f.pts, data: chunk });
            *slot = Some(chunk.to_vec());
            out.push(b);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::read;

    fn chunks(n: usize) -> Vec<Vec<u8>> {
        // Tamanhos diferentes, como o último pedaço de um quadro.
        (0..n).map(|i| (0..(1100 - i * 7)).map(|b| (b * 31 + i) as u8).collect()).collect()
    }

    #[test]
    fn percent_follows_loss() {
        assert_eq!(percent(0), 10);
        assert_eq!(percent(25), 15);
        assert_eq!(percent(50), 20);
        assert_eq!(percent(1000), 20);
        assert_eq!(recovery_count(1, 10), 1);
        assert_eq!(recovery_count(100, 10), 10);
        assert_eq!(recovery_count(101, 20), 21);
    }

    #[test]
    fn lost_pieces_come_back() {
        let c = chunks(30);
        let par = packets(&c, 77, 123, true, 20);
        assert_eq!(par.len(), 6);
        // Cabe no MTU: o pedaço de vídeo já sai com EXTRA a menos.
        assert!(par.iter().all(|p| p.len() <= 1100 + super::super::VIDEO_HEAD + EXTRA));
        let mut rx = Rx::default();
        let lost = [0usize, 5, 13, 29, 2, 7];
        let mut back = Vec::new();
        for (i, d) in c.iter().enumerate() {
            if !lost.contains(&i) {
                back.extend(rx.data(77, 30, i as u16, true, 123, d));
            }
        }
        assert!(back.is_empty());
        // Perde também um de paridade: os 5 que sobram não bastam para 6 pedaços.
        for p in &par[1..] {
            let Some(Msg::Fec { frame, pts, key, count, idx, rec, data }) = read(p) else { panic!() };
            back.extend(rx.parity(frame, count, idx, rec, key, pts, data));
        }
        assert!(back.is_empty(), "refez sem paridade suficiente");
        // Com os 6, volta tudo, uma vez só.
        let Some(Msg::Fec { frame, pts, key, count, idx, rec, data }) = read(&par[0]) else { panic!() };
        back.extend(rx.parity(frame, count, idx, rec, key, pts, data));
        back.extend(rx.parity(frame, count, idx, rec, key, pts, data));
        assert_eq!(back.len(), lost.len());
        for b in &back {
            let Some(Msg::Video { key, more, idx, count, frame, pts, data }) = read(b) else { panic!() };
            assert!(key && !more && count == 30 && frame == 77 && pts == 123);
            assert_eq!(data, &c[idx as usize][..]);
        }
    }

    #[test]
    fn parity_first_then_data_and_loss_measured() {
        let c = chunks(10);
        let par = packets(&c, 5, 0, false, 10);
        assert_eq!(par.len(), 1);
        let mut rx = Rx::default();
        let Some(Msg::Fec { frame, pts, key, count, idx, rec, data }) = read(&par[0]) else { panic!() };
        assert!(rx.parity(frame, count, idx, rec, key, pts, data).is_empty());
        let mut back = Vec::new();
        for (i, d) in c.iter().enumerate().skip(1) {
            back.extend(rx.data(5, 10, i as u16, false, 0, d));
        }
        assert_eq!(back.len(), 1);
        // 1 de 10 perdido, contado quando o quadro sai da janela.
        for f in 6..6 + KEEP as u32 {
            rx.data(f, 10, 0, false, 0, &c[0]);
        }
        let t = Instant::now() + WINDOW;
        assert_eq!(rx.tick(t), Some(100));
        assert_eq!(rx.loss_permille(), 100);
        assert_eq!(rx.tick(t), None);
    }

    #[test]
    fn garbage_is_ignored() {
        let mut rx = Rx::default();
        assert!(rx.parity(1, 3, 5, 2, false, 0, &[1, 2]).is_empty());
        assert!(rx.parity(1, 3, 0, 1, false, 0, &[1, 2, 3]).is_empty());
        assert!(rx.data(1, 0, 0, false, 0, &[1]).is_empty());
        assert!(rx.data(1, 3, 9, false, 0, &[1]).is_empty());
        // Mais paridade que pedaços: nem guarda.
        assert!(rx.parity(3, 2, 2, 3, false, 0, &[1, 2]).is_empty());
        assert!(rx.frames.iter().find(|f| f.frame == 3).is_some_and(|f| f.parity.is_empty()));
        // Paridade que não confere com os pedaços: nada de pânico, nada refeito.
        rx.data(2, 2, 0, false, 0, &[9; 40]);
        assert!(rx.parity(2, 2, 0, 1, false, 0, &[0; 8]).is_empty());
    }
}
