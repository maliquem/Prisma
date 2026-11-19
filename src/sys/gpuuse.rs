//! Uso da GPU para o medidor detalhado do compartilhamento de tela: o total e o que este processo
//! gasta em cada tipo de motor. Lê numa thread própria, 1 vez por segundo, só enquanto alguém pede.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::w;
use windows::Win32::System::Performance::*;

use super::monitor::{instance_pid, GpuCounters};

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

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let &[total, d3, encode, decode, copy] = b.get(..5)? else { return None };
        Some(Self { total, d3, encode, decode, copy })
    }
}

/// Soma as instâncias de `\GPU Engine(*)` (nomes como
/// `pid_12_luid_0x0_0x1_phys_0_eng_3_engtype_VideoEncode`).
// shortcut: o "Video Codec" da AMD (codifica e decodifica no mesmo motor) só entra no total.
pub fn sum<'a>(items: impl IntoIterator<Item = (&'a str, f64)>, pid: u32) -> GpuUse {
    let mut engines: Vec<(&str, f64)> = Vec::new();
    let mut mine = [0f64; 4];
    for (name, v) in items {
        let Some(at) = name.find("_luid_") else { continue };
        let engine = &name[at..];
        match engines.iter_mut().find(|e| e.0 == engine) {
            Some(e) => e.1 += v,
            None => engines.push((engine, v)),
        }
        if instance_pid(name) == Some(pid) {
            let kind = engine.rsplit("engtype_").next().unwrap_or("");
            if let Some(i) = ["3D", "VideoEncode", "VideoDecode", "Copy"].iter().position(|k| *k == kind) {
                mine[i] += v;
            }
        }
    }
    let pct = |v: f64| v.round().clamp(0.0, 100.0) as u8;
    GpuUse {
        total: pct(engines.iter().map(|e| e.1).fold(0.0, f64::max)),
        d3: pct(mine[0]),
        encode: pct(mine[1]),
        decode: pct(mine[2]),
        copy: pct(mine[3]),
    }
}

struct Reader {
    query: PDH_HQUERY,
    engine: PDH_HCOUNTER,
    buf: Vec<u8>,
}

impl Reader {
    fn new() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return None;
            }
            let mut engine = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(query, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut engine) != 0 {
                PdhCloseQuery(query);
                return None;
            }
            PdhCollectQueryData(query);
            Some(Self { query, engine, buf: Vec::new() })
        }
    }

    fn read(&mut self) -> GpuUse {
        unsafe { PdhCollectQueryData(self.query) };
        let items = GpuCounters::array(self.engine, &mut self.buf);
        sum(items.iter().map(|(n, v)| (n.as_str(), *v)), std::process::id())
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
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
                let Some(mut r) = Reader::new() else { return };
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    let mut s = me.lock().unwrap();
                    if s.asked.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
                        s.running = false;
                        s.last = None;
                        return;
                    }
                    drop(s);
                    let v = r.read();
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
        let items = [
            ("pid_7_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 20.0),
            ("pid_9_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 50.4),
            ("pid_7_luid_0x0_0x1_phys_0_eng_4_engtype_VideoEncode", 12.0),
            ("pid_7_luid_0x0_0x1_phys_0_eng_5_engtype_VideoDecode", 3.0),
            ("pid_7_luid_0x0_0x1_phys_0_eng_6_engtype_Copy", 1.0),
            ("pid_7_luid_0x0_0x1_phys_0_eng_7_engtype_Copy", 1.0),
            ("pid_9_luid_0x0_0x1_phys_0_eng_8_engtype_VideoCodec", 40.0),
            ("_Total", 99.0),
        ];
        let u = sum(items, 7);
        assert_eq!(u, GpuUse { total: 70, d3: 20, encode: 12, decode: 3, copy: 2 });
        assert_eq!(GpuUse::from_bytes(&u.bytes()), Some(u));
        assert_eq!(GpuUse::from_bytes(&[1, 2]), None);
    }
}
