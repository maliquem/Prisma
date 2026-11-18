//! Wake on LAN: o pacote mágico (6 × 0xFF e 16 × o MAC) por UDP nas portas 9 e 7. Só manda o
//! pacote; a placa-mãe e o adaptador do outro PC precisam estar com o Wake on LAN ligado.

use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

pub type Mac = [u8; 6];

/// Um pedido de ligar de outro aparelho por vez a cada 10 s, para ninguém usar este PC de canhão.
pub const RELAY_GAP: Duration = Duration::from_secs(10);

pub fn magic(mac: Mac) -> [u8; 102] {
    let mut p = [0xFF; 102];
    for k in 0..16 {
        p[6 + k * 6..12 + k * 6].copy_from_slice(&mac);
    }
    p
}

/// O broadcast da rede de `ip` com `prefix` bits de máscara.
pub fn broadcast(ip: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let mask = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix.min(32) as u32) };
    Ipv4Addr::from(u32::from(ip) | !mask)
}

pub fn mac_text(m: &Mac) -> String {
    data_encoding::HEXLOWER.encode(m)
}

pub fn mac_parse(s: &str) -> Option<Mac> {
    data_encoding::HEXLOWER_PERMISSIVE.decode(s.trim().as_bytes()).ok()?.try_into().ok()
}

/// O pedido de outro aparelho (já conferido como pareado) para ligar um PC na rede deste: só para
/// um MAC que este PC já viu num PC pareado com ele, e com intervalo desde o último.
pub fn may_relay(known: bool, last: Option<Instant>, now: Instant) -> bool {
    known && last.is_none_or(|t| now.saturating_duration_since(t) >= RELAY_GAP)
}

/// MAC e broadcast de cada adaptador físico ligado deste PC (sem VPN nem placa virtual).
pub fn local() -> (Vec<Mac>, Vec<Ipv4Addr>) {
    let (mut macs, mut nets) = (Vec::new(), Vec::new());
    for n in crate::sys::nets::list() {
        let (Some(mac), Ok(ip)) = (n.mac, n.ip.parse::<Ipv4Addr>()) else { continue };
        if !n.up || !n.hardware || ip.is_link_local() {
            continue;
        }
        if !macs.contains(&mac) {
            macs.push(mac);
        }
        let b = broadcast(ip, n.prefix);
        if !nets.contains(&b) {
            nets.push(b);
        }
    }
    (macs, nets)
}

/// Manda o pacote para 255.255.255.255 e para cada broadcast de `nets`, nas portas 9 e 7.
pub fn send(mac: Mac, nets: &[Ipv4Addr]) -> bool {
    let Ok(s) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else { return false };
    if s.set_broadcast(true).is_err() {
        return false;
    }
    let p = magic(mac);
    let mut sent = false;
    for ip in std::iter::once(Ipv4Addr::BROADCAST).chain(nets.iter().copied()) {
        for port in [9, 7] {
            sent |= s.send_to(&p, SocketAddr::from((ip, port))).is_ok();
        }
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_packet() {
        let mac = [0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e];
        let p = magic(mac);
        assert_eq!(&p[..6], &[0xFF; 6]);
        for k in 0..16 {
            assert_eq!(&p[6 + k * 6..12 + k * 6], &mac);
        }
        assert_eq!(mac_parse(&mac_text(&mac)), Some(mac));
        assert_eq!(mac_parse("12"), None);
    }

    #[test]
    fn broadcast_of_network() {
        let ip = Ipv4Addr::new(192, 168, 1, 37);
        assert_eq!(broadcast(ip, 24), Ipv4Addr::new(192, 168, 1, 255));
        assert_eq!(broadcast(ip, 16), Ipv4Addr::new(192, 168, 255, 255));
        assert_eq!(broadcast(Ipv4Addr::new(10, 0, 0, 5), 22), Ipv4Addr::new(10, 0, 3, 255));
        assert_eq!(broadcast(ip, 32), ip);
        assert_eq!(broadcast(ip, 0), Ipv4Addr::BROADCAST);
    }

    #[test]
    fn relay_rule() {
        let now = Instant::now();
        assert!(may_relay(true, None, now));
        assert!(!may_relay(false, None, now));
        assert!(!may_relay(true, Some(now), now + Duration::from_secs(3)));
        assert!(may_relay(true, Some(now), now + RELAY_GAP));
    }
}
