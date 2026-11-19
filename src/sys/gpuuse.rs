//! Uso da GPU para o medidor detalhado do compartilhamento de tela: o total e o que este processo
//! gasta em cada tipo de motor. Lê numa thread própria, 1 vez por segundo, só enquanto alguém pede.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::perf::Reading;

/// Porcentagens de 0 a 100. `total`: o motor mais ocupado da placa, como no Gerenciador de
/// Tarefas. Os outros são deste processo, somados por tipo de motor.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuUse {
    pub total: u8,
    pub d3: u8,
    pub encode: u8,
    pub decode: u8,
    pub copy: u8,
}

impl GpuUse {
    pub fn bytes(&self) -> [u8; 5] {
        [self.total, self.d3, self.encode, self.decode, self.copy]
    }

    /// O total da placa e o que o processo `pid` gasta em cada tipo de motor, de uma leitura do `perf`.
    // shortcut: o "Video Codec" da AMD (codifica e decodifica no mesmo motor) só entra no total.
    pub fn of(r: &Reading, pid: u32) -> Self {
        let pct = |v: f32| v.round().clamp(0.0, 100.0) as u8;
        let e = r.pid_eng.get(&pid).copied().unwrap_or_default();
        Self { total: pct(r.busiest), d3: pct(e[0]), encode: pct(e[2]), decode: pct(e[3]), copy: pct(e[1]) }
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let &[total, d3, encode, decode, copy] = b.get(..5)? else { return None };
        Some(Self { total, d3, encode, decode, copy })
    }
}

#[derive(Default)]
struct State {
    last: Option<GpuUse>,
    asked: Option<Instant>,
    running: bool,
}

/// A leitura mais recente. Quem chama [`Watch::get`] mantém a thread viva; sem pedido por 3 s,
/// ela fecha a consulta e para.
#[derive(Clone, Default)]
pub struct Watch(Arc<Mutex<State>>);

impl Watch {
    pub fn get(&self) -> Option<GpuUse> {
        let mut s = self.0.lock().unwrap();
        s.asked = Some(Instant::now());
        if !s.running {
            s.running = true;
            let me = self.0.clone();
            std::thread::spawn(move || {
                // Sem o contador de GPU, `running` fica ligado e ninguém tenta de novo.
                let Some(mut r) = super::perf::Counters::new().filter(|c| c.has_gpu()) else { return };
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let mut s = me.lock().unwrap();
                    if s.asked.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
                        s.running = false;
                        s.last = None;
                        return;
                    }
                    drop(s);
                    let v = GpuUse::of(&r.read(), std::process::id());
                    me.lock().unwrap().last = Some(v);
                }
            });
        }
        s.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_own_engines_and_busiest_total() {
        let items: Vec<(String, f64)> = [
            ("pid_7_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 20.0),
            ("pid_9_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 50.4),
            ("pid_7_luid_0x0_0x1_phys_0_eng_4_engtype_VideoEncode", 12.0),
            ("pid_7_luid_0x0_0x1_phys_0_eng_5_engtype_VideoDecode", 3.0),
            ("pid_7_luid_0x0_0x1_phys_0_eng_6_engtype_Copy", 1.0),
            ("pid_7_luid_0x0_0x1_phys_0_eng_7_engtype_Copy", 1.0),
            ("pid_9_luid_0x0_0x1_phys_0_eng_8_engtype_VideoCodec", 40.0),
            ("_Total", 99.0),
        ]
        .into_iter()
        .map(|(n, v)| (n.to_string(), v))
        .collect();
        let u = GpuUse::of(&crate::sys::perf::engines(&items), 7);
        assert_eq!(u, GpuUse { total: 70, d3: 20, encode: 12, decode: 3, copy: 2 });
        assert_eq!(GpuUse::from_bytes(&u.bytes()), Some(u));
        assert_eq!(GpuUse::from_bytes(&[1, 2]), None);
    }
}
