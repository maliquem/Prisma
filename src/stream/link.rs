//! Canal entre os dois lados: UDP direto (rede local) ou uma conexão do iroh (internet).
//! O host e quem assiste só conhecem este trait; os pacotes são os mesmos nos dois casos.

use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use super::MTU;

pub enum Poll {
    /// Pacote do outro lado, com este tamanho.
    Got(usize),
    /// Pacote de outro endereço (só no UDP): pode ser alguém novo chegando.
    Stranger(usize, SocketAddr),
    Empty,
    /// O canal fechou e não volta (só na internet).
    #[cfg_attr(not(feature = "stream"), allow(dead_code))]
    Closed,
}

pub trait Link: Send + Sync {
    fn send(&self, b: &[u8]);
    /// Espera até `wait` por um pacote; zero não espera.
    fn recv(&self, buf: &mut [u8], wait: Duration) -> Poll;
    /// Maior pacote que passa agora.
    fn mtu(&self) -> usize {
        MTU
    }
    /// Os dois lados no mesmo PC.
    fn same_pc(&self) -> bool {
        false
    }
    /// Por onde os pacotes estão indo, para mostrar.
    fn route(&self) -> String {
        String::new()
    }
    /// O caminho é a rede local ou uma VPN, sem relé e sem passar pela internet aberta. Só assim
    /// sai o PyroWave, que gasta centenas de Mbit/s.
    fn near(&self) -> bool {
        false
    }
}

/// Endereço de rede local (privado, de enlace ou deste PC).
pub fn is_local(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_link_local() || v.is_loopback(),
        IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// Endereço solto da faixa 100.64.0.0/10, a que o Tailscale usa.
pub fn is_cgnat(ip: std::net::IpAddr) -> bool {
    matches!(ip, std::net::IpAddr::V4(v) if v.octets()[0] == 100 && (v.octets()[1] & 0xc0) == 64)
}

/// UDP com um endereço do outro lado.
pub struct Udp {
    pub sock: UdpSocket,
    pub peer: SocketAddr,
    /// Espera configurada no socket, em microssegundos (u64::MAX: ainda nenhuma).
    mode: AtomicU64,
}

impl Udp {
    pub fn new(sock: UdpSocket, peer: SocketAddr) -> Self {
        Self { sock, peer, mode: AtomicU64::new(u64::MAX) }
    }
}

impl Link for Udp {
    /// Com a fila do Windows cheia, tenta de novo por um instante.
    fn send(&self, b: &[u8]) {
        let t = Instant::now();
        loop {
            match self.sock.send_to(b, self.peer) {
                Err(e) if e.kind() == ErrorKind::WouldBlock && t.elapsed() < Duration::from_millis(20) => std::hint::spin_loop(),
                _ => return,
            }
        }
    }

    fn recv(&self, buf: &mut [u8], wait: Duration) -> Poll {
        let us = wait.as_micros() as u64;
        if self.mode.swap(us, Ordering::Relaxed) != us {
            if us == 0 {
                let _ = self.sock.set_nonblocking(true);
            } else {
                let _ = self.sock.set_nonblocking(false);
                let _ = self.sock.set_read_timeout(Some(wait));
            }
        }
        loop {
            return match self.sock.recv_from(buf) {
                Ok((n, from)) if from == self.peer => Poll::Got(n),
                Ok((n, from)) => Poll::Stranger(n, from),
                // Resposta de porta fechada do outro lado: o Windows avisa no próximo recv.
                Err(e) if e.kind() == ErrorKind::ConnectionReset => continue,
                Err(_) => Poll::Empty,
            };
        }
    }

    fn same_pc(&self) -> bool {
        self.peer.ip().is_loopback()
    }

    fn route(&self) -> String {
        format!("UDP direto, {}", self.peer)
    }

    fn near(&self) -> bool {
        is_local(self.peer.ip()) || is_cgnat(self.peer.ip())
    }
}
