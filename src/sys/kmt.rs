//! Uso de GPU e VRAM direto do Windows, sem carregar a biblioteca da NVIDIA.
//! Custa poucas chamadas e quase nenhuma memória, então pode rodar o tempo todo.

use std::time::Instant;

use windows::Wdk::Graphics::Direct3D::*;
use windows::Win32::Foundation::LUID;

use super::nvml::GpuReading;

pub struct Kmt {
    luid: LUID,
    nodes: u32,
    segments: u32,
    last: Vec<i64>,
    at: Instant,
}

fn query(q: &mut D3DKMT_QUERYSTATISTICS) -> bool {
    unsafe { D3DKMTQueryStatistics(q).0 == 0 }
}

fn dedicated(luid: LUID, seg: u32) -> Option<(u64, u64)> {
    let mut q = D3DKMT_QUERYSTATISTICS {
        Type: D3DKMT_QUERYSTATISTICS_SEGMENT,
        AdapterLuid: luid,
        ..Default::default()
    };
    q.Anonymous.QuerySegment.SegmentId = seg;
    if !query(&mut q) {
        return None;
    }
    let s = unsafe { q.QueryResult.SegmentInformation };
    (s.Aperture == 0).then_some((s.CommitLimit, s.BytesResident))
}

impl Kmt {
    /// Escolhe o adaptador com mais memória dedicada: a placa de vídeo real.
    pub fn new() -> Option<Self> {
        let mut list = vec![D3DKMT_ADAPTERINFO::default(); 16];
        let mut en = D3DKMT_ENUMADAPTERS2 { NumAdapters: 16, pAdapters: list.as_mut_ptr() };
        unsafe {
            if D3DKMTEnumAdapters2(&mut en).0 != 0 {
                return None;
            }
        }
        let mut best: Option<(u64, Self)> = None;
        for a in &list[..en.NumAdapters as usize] {
            let mut q = D3DKMT_QUERYSTATISTICS {
                Type: D3DKMT_QUERYSTATISTICS_ADAPTER,
                AdapterLuid: a.AdapterLuid,
                ..Default::default()
            };
            if !query(&mut q) {
                continue;
            }
            let i = unsafe { q.QueryResult.AdapterInformation };
            let total: u64 = (0..i.NbSegments).filter_map(|s| dedicated(a.AdapterLuid, s)).map(|v| v.0).sum();
            if best.as_ref().is_none_or(|b| total > b.0) {
                let k = Self {
                    luid: a.AdapterLuid,
                    nodes: i.NodeCount,
                    segments: i.NbSegments,
                    last: Vec::new(),
                    at: Instant::now(),
                };
                best = Some((total, k));
            }
        }
        best.map(|b| b.1)
    }

    pub fn read(&mut self) -> GpuReading {
        let mut r = GpuReading::default();
        for s in 0..self.segments {
            if let Some((limit, used)) = dedicated(self.luid, s) {
                r.total += limit;
                r.used += used;
            }
        }
        let now: Vec<i64> = (0..self.nodes)
            .map(|n| {
                let mut q = D3DKMT_QUERYSTATISTICS {
                    Type: D3DKMT_QUERYSTATISTICS_NODE,
                    AdapterLuid: self.luid,
                    ..Default::default()
                };
                q.Anonymous.QueryNode.NodeId = n;
                if query(&mut q) {
                    unsafe { q.QueryResult.NodeInformation.GlobalInformation.RunningTime }
                } else {
                    0
                }
            })
            .collect();
        let dt = self.at.elapsed().as_secs_f32() * 1e7;
        self.at = Instant::now();
        if self.last.len() == now.len() && dt > 0.0 {
            // Como o Gerenciador de Tarefas: o motor mais ocupado define o uso.
            r.util = now
                .iter()
                .zip(&self.last)
                .map(|(a, b)| (a - b) as f32 / dt * 100.0)
                .fold(0.0, f32::max)
                .clamp(0.0, 100.0);
        }
        self.last = now;
        r
    }
}
