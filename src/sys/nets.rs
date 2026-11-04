//! Adaptadores de rede: velocidade, total transferido e endereço.

use std::time::Instant;

use windows::Win32::Foundation::*;
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::NetworkManagement::Ndis::*;
use windows::Win32::Networking::WinSock::*;

#[derive(Clone)]
pub struct Net {
    pub luid: u64,
    pub name: String,
    pub desc: String,
    pub up: bool,
    pub speed: u64,
    pub ip: String,
    pub down_rate: u64,
    pub up_rate: u64,
    pub total_down: u64,
    pub total_up: u64,
    pub hist: Vec<f32>,
    prev: Option<(u64, u64, Instant)>,
}

fn pwstr(p: windows::core::PWSTR) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { p.to_string().unwrap_or_default() }
    }
}

/// Lista como a do Painel de Controle, sem as entradas internas do Windows.
pub fn list() -> Vec<Net> {
    let mut out = Vec::new();
    unsafe {
        let mut size = 32768u32;
        let mut buf = vec![0u8; size as usize];
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let p = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
        let mut r = GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(p), &mut size);
        if r == ERROR_BUFFER_OVERFLOW.0 {
            buf.resize(size as usize, 0);
            let p = buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH;
            r = GetAdaptersAddresses(AF_UNSPEC.0 as u32, flags, None, Some(p), &mut size);
        }
        if r != 0 {
            return out;
        }
        let mut a = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !a.is_null() {
            let x = &*a;
            a = x.Next;
            if x.IfType == 24 || x.IfType == 131 {
                continue;
            }
            let mut ip = String::new();
            let mut u = x.FirstUnicastAddress;
            while !u.is_null() {
                let s = (*u).Address.lpSockaddr;
                if !s.is_null() && (*s).sa_family == AF_INET {
                    let v4 = &*(s as *const SOCKADDR_IN);
                    let b = v4.sin_addr.S_un.S_un_b;
                    ip = format!("{}.{}.{}.{}", b.s_b1, b.s_b2, b.s_b3, b.s_b4);
                    break;
                }
                u = (*u).Next;
            }
            out.push(Net {
                luid: x.Luid.Value,
                name: pwstr(x.FriendlyName),
                desc: pwstr(x.Description),
                up: x.OperStatus == IfOperStatusUp,
                speed: if x.TransmitLinkSpeed == u64::MAX { 0 } else { x.TransmitLinkSpeed.min(x.ReceiveLinkSpeed) },
                ip,
                down_rate: 0,
                up_rate: 0,
                total_down: 0,
                total_up: 0,
                hist: Vec::new(),
                prev: None,
            });
        }
    }
    out.sort_by_key(|n| !n.up);
    out
}

/// Atualiza as taxas. `max` guarda o pico recente para escalar o gráfico.
pub fn sample(list: &mut [Net]) {
    for n in list {
        let mut row = MIB_IF_ROW2::default();
        row.InterfaceLuid = NET_LUID_LH { Value: n.luid };
        if unsafe { GetIfEntry2(&mut row) }.is_err() {
            continue;
        }
        let now = Instant::now();
        n.total_down = row.InOctets;
        n.total_up = row.OutOctets;
        n.up = row.OperStatus == IfOperStatusUp;
        if let Some((d, u, t)) = n.prev {
            let dt = (now - t).as_secs_f64().max(0.05);
            n.down_rate = (row.InOctets.saturating_sub(d) as f64 / dt) as u64;
            n.up_rate = (row.OutOctets.saturating_sub(u) as f64 / dt) as u64;
        }
        n.prev = Some((row.InOctets, row.OutOctets, now));
        if n.hist.len() == 60 {
            n.hist.remove(0);
        }
        let pct = if n.speed > 0 {
            ((n.down_rate + n.up_rate) * 8) as f64 / n.speed as f64 * 100.0
        } else {
            0.0
        };
        n.hist.push(pct.min(100.0) as f32);
    }
}
