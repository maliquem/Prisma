//! Leituras extras da página Desempenho: cada engine da GPU, VRAM compartilhada, clock de cada
//! thread e listas da RAM. Só ficam abertas enquanto a página pede (`monitor::PERF`).

use std::collections::HashMap;

use windows::core::*;
use windows::Win32::System::Performance::*;

/// Engines na ordem da página: 3D, Copy, Video Encode, Video Decode, Compute.
pub const ENGINES: usize = 5;

#[derive(Clone, Default)]
pub struct Reading {
    /// Uso de cada tipo de engine (a engine mais cheia daquele tipo), em %.
    pub eng: [f32; ENGINES],
    /// Por processo: uso de cada tipo de engine, em %.
    pub pid_eng: HashMap<u32, [f32; ENGINES]>,
    pub pid_shared: HashMap<u32, u64>,
    /// Memória compartilhada da GPU em uso (sai da RAM).
    pub shared: u64,
    /// Clock efetivo de cada thread, em % do clock base.
    pub thread_perf: Vec<f32>,
    pub modified: u64,
    pub standby: u64,
    pub free: u64,
}

pub struct Counters {
    query: PDH_HQUERY,
    engine: PDH_HCOUNTER,
    pid_shared: PDH_HCOUNTER,
    shared: PDH_HCOUNTER,
    thread: PDH_HCOUNTER,
    modified: PDH_HCOUNTER,
    standby: [PDH_HCOUNTER; 3],
    free: PDH_HCOUNTER,
    buf: Vec<u8>,
}

fn add(query: PDH_HQUERY, path: PCWSTR) -> PDH_HCOUNTER {
    let mut c = PDH_HCOUNTER::default();
    unsafe { PdhAddEnglishCounterW(query, path, 0, &mut c) };
    c
}

fn value(c: PDH_HCOUNTER) -> f64 {
    let mut v = PDH_FMT_COUNTERVALUE::default();
    if unsafe { PdhGetFormattedCounterValue(c, PDH_FMT_DOUBLE, None, &mut v) } == 0 {
        unsafe { v.Anonymous.doubleValue }
    } else {
        0.0
    }
}

/// Tipo e engine física de uma instância `pid_1_luid_0x0_0x1_phys_0_eng_3_engtype_VideoDecode`.
fn engine_of(name: &str) -> Option<(usize, &str)> {
    let (head, ty) = name.rsplit_once("_engtype_")?;
    let kind = match ty {
        "3D" => 0,
        "Copy" => 1,
        "VideoEncode" => 2,
        "VideoDecode" => 3,
        t if t.starts_with("Compute") || t == "Cuda" => 4,
        _ => return None,
    };
    Some((kind, head.split_once("_luid_")?.1))
}

/// Posição de uma thread da instância "grupo,número" do `Processor Information`; os totais ficam de fora.
fn thread_of(name: &str) -> Option<(u32, u32)> {
    let (g, n) = name.split_once(',')?;
    Some((g.parse().ok()?, n.parse().ok()?))
}

impl Counters {
    pub fn new() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return None;
            }
            let c = Self {
                query,
                engine: add(query, w!("\\GPU Engine(*)\\Utilization Percentage")),
                pid_shared: add(query, w!("\\GPU Process Memory(*)\\Shared Usage")),
                shared: add(query, w!("\\GPU Adapter Memory(*)\\Shared Usage")),
                thread: add(query, w!("\\Processor Information(*)\\% Processor Performance")),
                modified: add(query, w!("\\Memory\\Modified Page List Bytes")),
                standby: [
                    add(query, w!("\\Memory\\Standby Cache Reserve Bytes")),
                    add(query, w!("\\Memory\\Standby Cache Normal Priority Bytes")),
                    add(query, w!("\\Memory\\Standby Cache Core Bytes")),
                ],
                free: add(query, w!("\\Memory\\Free & Zero Page List Bytes")),
                buf: Vec::new(),
            };
            PdhCollectQueryData(query);
            Some(c)
        }
    }

    pub fn read(&mut self) -> Reading {
        unsafe { PdhCollectQueryData(self.query) };
        let mut r = Reading::default();
        // Soma os processos de cada engine física; o tipo mostra a mais cheia, como o Gerenciador de Tarefas.
        // shortcut: junta as engines de todas as placas (iGPU incluída); separar por LUID se alguém tiver duas.
        let mut by_engine: HashMap<String, (usize, f32)> = HashMap::new();
        for (name, v) in super::monitor::counter_array(self.engine, &mut self.buf) {
            let Some((kind, engine)) = engine_of(&name) else { continue };
            by_engine.entry(engine.to_string()).or_insert((kind, 0.0)).1 += v as f32;
            if let Some(pid) = super::monitor::instance_pid(&name) {
                let e = r.pid_eng.entry(pid).or_default();
                e[kind] = e[kind].max(v as f32);
            }
        }
        for (kind, v) in by_engine.into_values() {
            r.eng[kind] = r.eng[kind].max(v.min(100.0));
        }
        for (name, v) in super::monitor::counter_array(self.pid_shared, &mut self.buf) {
            if let Some(pid) = super::monitor::instance_pid(&name) {
                *r.pid_shared.entry(pid).or_default() += v as u64;
            }
        }
        r.shared = super::monitor::counter_array(self.shared, &mut self.buf).into_iter().map(|(_, v)| v as u64).max().unwrap_or(0);
        let mut threads: Vec<((u32, u32), f32)> =
            super::monitor::counter_array(self.thread, &mut self.buf).into_iter().filter_map(|(n, v)| Some((thread_of(&n)?, v as f32))).collect();
        threads.sort_by_key(|t| t.0);
        r.thread_perf = threads.into_iter().map(|t| t.1).collect();
        r.modified = value(self.modified) as u64;
        r.standby = self.standby.iter().map(|&c| value(c) as u64).sum();
        r.free = value(self.free) as u64;
        r
    }
}

impl Drop for Counters {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.query);
        }
    }
}

/// Pentes de memória pela tabela SMBIOS: tipo (DDR4, DDR5), velocidade em MT/s, slots ocupados e total.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Ram {
    pub kind: &'static str,
    pub speed: u32,
    pub used: u32,
    pub slots: u32,
}

pub fn ram() -> Ram {
    static CACHE: std::sync::OnceLock<Ram> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| {
        use windows::Win32::System::SystemInformation::*;
        let rsmb = FIRMWARE_TABLE_PROVIDER(u32::from_be_bytes(*b"RSMB"));
        let n = unsafe { GetSystemFirmwareTable(rsmb, 0, None) } as usize;
        let mut buf = vec![0u8; n];
        if n == 0 || unsafe { GetSystemFirmwareTable(rsmb, 0, Some(&mut buf)) } as usize != n || n < 8 {
            return Ram::default();
        }
        parse_smbios(&buf[8..])
    })
}

/// Lê as estruturas tipo 17 (Memory Device) de uma tabela SMBIOS crua.
fn parse_smbios(t: &[u8]) -> Ram {
    let mut r = Ram::default();
    let mut off = 0;
    while off + 4 <= t.len() {
        let (ty, len) = (t[off], t[off + 1] as usize);
        if len < 4 || off + len > t.len() || ty == 127 {
            break;
        }
        let s = &t[off..off + len];
        let u16_at = |i: usize| s.get(i..i + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as u32);
        if ty == 17 && len >= 0x17 {
            r.slots += 1;
            let size = u16_at(0x0C);
            if size != 0 && size != 0xFFFF {
                r.used += 1;
                r.kind = match s[0x12] {
                    0x18 => "DDR3",
                    0x1A => "DDR4",
                    0x1D => "LPDDR3",
                    0x1E => "LPDDR4",
                    0x22 => "DDR5",
                    0x23 => "LPDDR5",
                    _ => r.kind,
                };
                // Velocidade configurada (SMBIOS 2.7+) é a que roda; a outra é a máxima do pente.
                let speed = if len >= 0x22 && u16_at(0x20) != 0 { u16_at(0x20) } else { u16_at(0x15) };
                r.speed = r.speed.max(speed);
            }
        }
        // As strings vêm depois da parte fixa e terminam em dois zeros.
        let mut end = off + len;
        while end + 1 < t.len() && (t[end] != 0 || t[end + 1] != 0) {
            end += 1;
        }
        off = end + 2;
    }
    r
}

/// Nome do processador, como o Windows mostra.
pub fn cpu_name() -> String {
    static CACHE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            super::reg::text(super::reg::Hive::Machine, "HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0", "ProcessorNameString")
                .map(|s| s.trim().to_string())
                .unwrap_or_default()
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_names() {
        assert_eq!(engine_of("pid_12_luid_0x0_0xD1B3_phys_0_eng_3_engtype_VideoDecode"), Some((3, "0x0_0xD1B3_phys_0_eng_3")));
        assert_eq!(engine_of("pid_12_luid_0x0_0xD1B3_phys_0_eng_0_engtype_3D").map(|e| e.0), Some(0));
        assert_eq!(engine_of("pid_12_luid_0x0_0x1_phys_0_eng_9_engtype_Compute_1").map(|e| e.0), Some(4));
        assert_eq!(engine_of("pid_12_luid_0x0_0x1_phys_0_eng_9_engtype_Security"), None);
        assert_eq!(thread_of("0,7"), Some((0, 7)));
        assert_eq!(thread_of("0,_Total"), None);
        assert_eq!(thread_of("_Total"), None);
    }

    #[test]
    fn smbios_memory() {
        // Dois slots de DDR5: um com pente de 16 GB a 6000 MT/s, um vazio; depois o fim da tabela.
        let mut dev = vec![0u8; 0x28];
        dev[0] = 17;
        dev[1] = 0x28;
        dev[0x0C..0x0E].copy_from_slice(&16384u16.to_le_bytes());
        dev[0x12] = 0x22;
        dev[0x15..0x17].copy_from_slice(&6400u16.to_le_bytes());
        dev[0x20..0x22].copy_from_slice(&6000u16.to_le_bytes());
        let mut empty = dev.clone();
        empty[0x0C..0x0E].copy_from_slice(&0u16.to_le_bytes());
        let mut t = Vec::new();
        for s in [&dev, &empty] {
            t.extend_from_slice(s);
            t.extend_from_slice(b"DIMM\0\0");
        }
        t.extend_from_slice(&[127, 4, 0, 0, 0, 0]);
        assert_eq!(parse_smbios(&t), Ram { kind: "DDR5", speed: 6000, used: 1, slots: 2 });
    }
}
