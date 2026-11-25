//! Por onde passa a conexão do iroh: que rede é cada caminho (direto, uma VPN ou o relé), quais
//! redes cada aparelho pode usar, o seletor que só escolhe entre as permitidas e a linha do caminho.
//!
//! O host guarda as redes fora em `redes_fora=` das opções (por aparelho que assiste, como as outras
//! de [`super::opts::PER_CLIENT`]); quem assiste guarda por PC em `cliente-redes.txt`. Cada lado
//! manda só pelo que ele permite, e fecha a conexão se ela ficar numa rede desmarcada. O relé ainda
//! pode levar o aperto de mão antes do caminho direto existir: vídeo, som e controles, não.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iroh::endpoint::transports::{FourTuple, PathSelection, PathSelectionContext, PathSelectionData, PathSelector};
use iroh::endpoint::Connection;
use iroh::{EndpointId, TransportAddr};

use super::link::{is_cgnat, is_local};

/// A chave da rede direta (rede local ou internet, sem VPN) e a do relé da n0.
pub const DIRECT: &str = "direto";
pub const RELAY: &str = "rele";

/// As VPNs que o Prisma reconhece pelo nome ou pela descrição do adaptador.
const VPNS: [(&str, &str); 9] = [
    ("zerotier", "ZeroTier"),
    ("tailscale", "Tailscale"),
    ("radmin", "Radmin VPN"),
    ("hamachi", "Hamachi"),
    ("wireguard", "WireGuard"),
    ("netbird", "NetBird"),
    ("openvpn", "OpenVPN"),
    ("tap-windows", "OpenVPN"),
    ("softether", "SoftEther"),
];

fn vendor(name_desc: &str) -> Option<&'static str> {
    let t = name_desc.to_lowercase();
    VPNS.iter().find(|(k, _)| t.contains(k)).map(|(_, v)| *v)
}

/// Os endereços das VPNs deste PC: (endereço, bits da máscara, VPN). Lidos de novo a cada 10 s,
/// porque o seletor roda a cada evento de caminho e a VPN pode ligar no meio.
fn vpn_addrs() -> Arc<Vec<(IpAddr, u8, &'static str)>> {
    static CACHE: Mutex<Option<(Instant, Arc<Vec<(IpAddr, u8, &'static str)>>)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap();
    if let Some((_, v)) = c.as_ref().filter(|(t, _)| t.elapsed() < Duration::from_secs(10)) {
        return v.clone();
    }
    let v: Arc<Vec<_>> = Arc::new(
        crate::sys::nets::list()
            .into_iter()
            .filter_map(|n| Some((vendor(&format!("{} {}", n.name, n.desc))?, n.addrs)))
            .flat_map(|(v, a)| a.into_iter().map(move |(ip, bits)| (ip, bits, v)))
            .collect(),
    );
    *c = Some((Instant::now(), v.clone()));
    v
}

/// `ip` está na rede `net/bits`?
fn in_net(ip: IpAddr, net: IpAddr, bits: u8) -> bool {
    match (ip, net) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let m = if bits == 0 { 0 } else { u32::MAX << (32 - bits.min(32) as u32) };
            u32::from(a) & m == u32::from(b) & m
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let m = if bits == 0 { 0 } else { u128::MAX << (128 - bits.min(128) as u32) };
            u128::from(a) & m == u128::from(b) & m
        }
        _ => false,
    }
}

/// A VPN do caminho: pela placa daqui que ele usa ou pela faixa da VPN (a máscara de verdade do
/// adaptador; o ZeroTier usa /16, e comparar só o /24 dava "rede local").
fn vpn_in(remote: IpAddr, local: Option<IpAddr>, vpns: &[(IpAddr, u8, &'static str)]) -> Option<&'static str> {
    if let Some(v) = local.and_then(|l| vpns.iter().find(|(a, _, _)| *a == l)) {
        return Some(v.2);
    }
    if let Some(v) = vpns.iter().find(|(a, bits, _)| *bits > 0 && in_net(remote, *a, *bits)) {
        return Some(v.2);
    }
    // O Tailscale dá endereços soltos em 100.64.0.0/10, fora da máscara da placa.
    vpns.iter().find(|v| v.2 == "Tailscale" && is_cgnat(remote)).map(|v| v.2)
}

pub fn vpn_of(remote: IpAddr, local: Option<IpAddr>) -> Option<&'static str> {
    vpn_in(remote, local, &vpn_addrs())
}

/// Que rede um caminho usa.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Via {
    Lan,
    Internet,
    Vpn(&'static str),
    Relay,
}

impl Via {
    pub fn ip(remote: IpAddr, local: Option<IpAddr>) -> Via {
        match vpn_of(remote, local) {
            Some(v) => Via::Vpn(v),
            None if is_local(remote) => Via::Lan,
            None => Via::Internet,
        }
    }

    /// A chave da caixa da rede.
    pub fn key(self) -> &'static str {
        match self {
            Via::Lan | Via::Internet => DIRECT,
            Via::Vpn(v) => v,
            Via::Relay => RELAY,
        }
    }

    fn of(t: &FourTuple) -> Via {
        match t {
            FourTuple::Ip { remote, local } => Via::ip(remote.ip(), *local),
            FourTuple::Relay { .. } => Via::Relay,
            _ => Via::Internet,
        }
    }
}

/// As caixas: direto, cada VPN instalada neste PC e o relé. A página pede a cada quadro: a lista
/// das placas é lida de novo a cada 10 s.
pub fn networks() -> Vec<&'static str> {
    static CACHE: Mutex<Option<(Instant, Vec<&'static str>)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap();
    if let Some((_, v)) = c.as_ref().filter(|(t, _)| t.elapsed() < Duration::from_secs(10)) {
        return v.clone();
    }
    let mut v = vec![DIRECT];
    for n in crate::sys::nets::list() {
        if let Some(x) = vendor(&format!("{} {}", n.name, n.desc)).filter(|x| !v.contains(x)) {
            v.push(x);
        }
    }
    v.push(RELAY);
    *c = Some((Instant::now(), v.clone()));
    v
}

/// `redes_fora=`: as chaves separadas por vírgula.
pub fn parse_deny(s: &str) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for k in s.split(',').map(|k| k.chars().filter(|c| !c.is_control()).collect::<String>()).map(|k| k.trim().to_string()) {
        if !k.is_empty() && !v.iter().any(|x| x.eq_ignore_ascii_case(&k)) {
            v.push(k);
        }
    }
    v
}

pub fn allowed(deny: &[String], key: &str) -> bool {
    !deny.iter().any(|d| d.eq_ignore_ascii_case(key))
}

/// Marca ou desmarca uma rede no texto de `redes_fora=`.
pub fn set_allowed(deny: &str, key: &str, on: bool) -> String {
    let mut v = parse_deny(deny);
    v.retain(|d| !d.eq_ignore_ascii_case(key));
    if !on {
        v.push(key.to_string());
    }
    v.join(",")
}

// ---------- As redes de cada lado ----------

/// As redes fora para quem assiste a este PC (as dele, ou as de todos). Lidas de novo a cada 5 s:
/// mudar na página vale na conexão aberta.
pub fn host_deny(id: &EndpointId) -> Vec<String> {
    static CACHE: Mutex<Option<HashMap<EndpointId, (Instant, Vec<String>)>>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap();
    let map = c.get_or_insert_with(HashMap::new);
    if let Some((_, v)) = map.get(id).filter(|(t, _)| t.elapsed() < Duration::from_secs(5)) {
        return v.clone();
    }
    let v = parse_deny(&super::Opts::load_for(&data_encoding::HEXLOWER.encode(id.as_bytes())).nets_off);
    map.insert(*id, (Instant::now(), v.clone()));
    v
}

fn client_path() -> std::path::PathBuf {
    super::user_dir().join("cliente-redes.txt")
}

/// As redes fora para assistir ao PC `id` (aqui quem assiste): `id<TAB>redes` por linha.
pub fn client_deny(id: &str) -> String {
    let t = std::fs::read_to_string(client_path()).unwrap_or_default();
    t.lines().find_map(|l| l.split_once('\t').filter(|(k, _)| k.eq_ignore_ascii_case(id)).map(|(_, v)| parse_deny(v).join(","))).unwrap_or_default()
}

pub fn set_client_deny(id: &str, deny: &str) -> std::io::Result<()> {
    let t = std::fs::read_to_string(client_path()).unwrap_or_default();
    let mut out: String = t.lines().filter(|l| l.split_once('\t').is_some_and(|(k, _)| !k.eq_ignore_ascii_case(id))).map(|l| format!("{l}\n")).collect();
    if !deny.is_empty() {
        out += &format!("{id}\t{deny}\n");
    }
    std::fs::create_dir_all(super::user_dir())?;
    std::fs::write(client_path(), out)
}

// ---------- O seletor ----------

/// Seletor de caminho do iroh que só escolhe redes permitidas. Entre elas, o mesmo do iroh: IP
/// antes do relé, o menor tempo de ida e volta (IPv6 com 3 ms de vantagem) e só troca por 5 ms.
#[derive(Debug)]
pub struct Picker {
    /// As redes fora de quem assiste (uma conexão por endpoint). None: o host, que vê pelo aparelho.
    deny: Option<Vec<String>>,
}

impl Picker {
    pub fn viewer(deny: Vec<String>) -> Arc<Picker> {
        Arc::new(Picker { deny: Some(deny) })
    }

    pub fn host() -> Arc<Picker> {
        Arc::new(Picker { deny: None })
    }
}

/// Um candidato: a rede, se é IPv6, o tempo de ida e volta e se é o caminho de agora.
#[derive(Clone, Copy)]
struct Cand {
    via: Via,
    v6: bool,
    rtt: Duration,
    current: bool,
}

/// O índice do caminho a escolher, ou None para ficar no de agora. O de agora numa rede fora
/// troca na hora por qualquer permitido.
fn best(c: &[Cand], deny: &[String]) -> Option<usize> {
    let key = |x: &Cand| {
        let bias = if x.v6 { 3_000_000i128 } else { 0 };
        (x.via == Via::Relay, x.rtt.as_nanos() as i128 - bias)
    };
    let ok: Vec<usize> = (0..c.len()).filter(|&i| allowed(deny, c[i].via.key())).collect();
    let b = *ok.iter().min_by_key(|&&i| key(&c[i]))?;
    let Some(cur) = ok.iter().filter(|&&i| c[i].current).map(|&i| key(&c[i])).min() else {
        return Some(b);
    };
    let (tier, rtt) = key(&c[b]);
    (tier != cur.0 || rtt + 5_000_000 <= cur.1).then_some(b)
}

impl PathSelector for Picker {
    fn select(&self, ctx: &PathSelectionContext<'_>) -> PathSelection {
        let current = ctx.current();
        let mut data: Vec<PathSelectionData<'_>> = Vec::new();
        let mut cands = Vec::new();
        let mut who = None;
        for p in ctx.paths() {
            let Some(stats) = p.stats() else { continue };
            let t = p.network_path();
            if let FourTuple::Relay { endpoint_id, .. } = t {
                who = Some(*endpoint_id);
            }
            let v6 = matches!(t, FourTuple::Ip { remote: SocketAddr::V6(_), .. });
            cands.push(Cand { via: Via::of(t), v6, rtt: stats.rtt, current: Some(t) == current });
            data.push(p);
        }
        // shortcut: no host, o aparelho sai do caminho pelo relé (quem assiste chega por ele); sem
        // relé, valem as de todos, e a vigia de `guard` fecha se a do aparelho for diferente.
        let deny = match &self.deny {
            Some(d) => d.clone(),
            None => match who {
                Some(id) => host_deny(&id),
                None => parse_deny(&super::Opts::load().nets_off),
            },
        };
        let mut sel = PathSelection::none();
        if let Some(i) = best(&cands, &deny) {
            sel.set(&data[i]);
        }
        sel
    }
}

// ---------- Portão e vigia ----------

/// A rede do caminho escolhido agora, e a linha dele.
fn selected(conn: &Connection) -> Option<(Via, String)> {
    let paths = conn.paths();
    let p = paths.iter().find(|p| p.is_selected())?;
    let rtt = p.rtt();
    match p.remote_addr() {
        TransportAddr::Ip(a) => {
            let local = match p.local_addr() {
                iroh::endpoint::LocalTransportAddr::Ip(l) => *l,
                _ => None,
            };
            let via = Via::ip(a.ip(), local);
            Some((via, ip_text(via, *a, rtt)))
        }
        TransportAddr::Relay(u) => Some((Via::Relay, relay_text(u.as_str(), rtt))),
        _ => None,
    }
}

/// A linha do caminho escolhido agora (em português; a tela traduz com [`label`]).
pub fn route_of(conn: &Connection) -> String {
    selected(conn).map_or_else(|| "procurando caminho".into(), |s| s.1)
}

/// O caminho é a rede local ou uma VPN (sem relé e sem a internet aberta)?
pub fn near_of(conn: &Connection) -> bool {
    selected(conn).is_some_and(|s| matches!(s.0, Via::Lan | Via::Vpn(_)))
}

/// Espera até `wait` o caminho cair numa rede permitida. Err: a linha do caminho de agora.
pub async fn gate(conn: &Connection, deny: &[String], wait: Duration) -> Result<(), String> {
    if deny.is_empty() {
        return Ok(());
    }
    let until = Instant::now() + wait;
    loop {
        let now = selected(conn);
        if now.as_ref().is_some_and(|s| allowed(deny, s.0.key())) {
            return Ok(());
        }
        if Instant::now() >= until || conn.close_reason().is_some() {
            return Err(now.map_or_else(|| "procurando caminho".into(), |s| s.1));
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// Código de fechamento quando a conexão fica numa rede desmarcada.
pub const CLOSE_NET: u32 = 5;

/// Vigia a conexão: mais de 4 s seguidos numa rede desmarcada, fecha (o iroh voltou para o relé
/// ou para uma VPN fora porque o caminho permitido caiu). `deny` é lido de novo a cada volta.
pub async fn guard(conn: Connection, deny: impl Fn() -> Vec<String>, who: String) {
    let mut bad = 0;
    while conn.close_reason().is_none() {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let d = deny();
        match selected(&conn) {
            Some((via, line)) if !allowed(&d, via.key()) => {
                bad += 1;
                if bad >= 4 {
                    eprintln!("{} conexão com «{who}» fechada: ficou numa rede desmarcada ({line})", super::net::now_text());
                    super::notice("Conexão com «{}» fechada: o único caminho dela era por uma rede desmarcada", &who);
                    conn.close(CLOSE_NET.into(), b"rede fora");
                    return;
                }
            }
            _ => bad = 0,
        }
    }
}

// ---------- A linha do caminho ----------

fn family(a: SocketAddr) -> &'static str {
    if a.is_ipv6() { "IPv6" } else { "IPv4" }
}

/// `pela VPN (ZeroTier · IPv4), 172.24.175.246:5000, 12 ms` e afins.
fn ip_text(via: Via, a: SocketAddr, rtt: Duration) -> String {
    let ms = rtt.as_secs_f64() * 1000.0;
    match via {
        Via::Vpn(v) => format!("pela VPN ({v} · {}), {a}, {ms:.0} ms", family(a)),
        Via::Lan => format!("direto na rede local ({}), {a}, {ms:.0} ms", family(a)),
        _ => format!("direto pela internet ({}), {a}, {ms:.0} ms", family(a)),
    }
}

/// A região de um relé da n0 pelo começo do nome (`use1-1.relay.n0.iroh.link`).
fn region(host: &str) -> Option<&'static str> {
    let code = host.split(['-', '.']).next()?;
    Some(match code {
        "use1" => "EUA leste",
        "usw1" => "EUA oeste",
        "euc1" => "Europa",
        "aps1" => "Ásia",
        _ => return None,
    })
}

/// `pelo relé da n0 (use1-1.relay.n0.iroh.link · EUA leste), 80 ms`.
fn relay_text(url: &str, rtt: Duration) -> String {
    let host = url.split("://").last().unwrap_or(url).trim_end_matches('/').trim_end_matches('.');
    let ms = rtt.as_secs_f64() * 1000.0;
    match region(host) {
        Some(r) => format!("pelo relé da n0 ({host} · {r}), {ms:.0} ms"),
        None => format!("pelo relé da n0 ({host}), {ms:.0} ms"),
    }
}

/// O nome mostrado de uma caixa de rede.
pub fn network_name(key: &str) -> String {
    match key {
        DIRECT => crate::i18n::tr("Direto (rede local e internet)").into_owned(),
        RELAY => crate::i18n::tr("Relé público da n0").into_owned(),
        v => format!("{} {v}", crate::i18n::tr("VPN")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(via: Via, v6: bool, ms: u64, current: bool) -> Cand {
        Cand { via, v6, rtt: Duration::from_millis(ms), current }
    }

    #[test]
    fn picks_only_allowed_networks() {
        let zt = Via::Vpn("ZeroTier");
        let paths = [c(Via::Relay, false, 80, true), c(zt, false, 10, false), c(Via::Internet, false, 20, false)];
        // Tudo marcado: o mais rápido.
        assert_eq!(best(&paths, &[]), Some(1));
        // Sem ZeroTier: o direto, mesmo mais lento.
        assert_eq!(best(&paths, &parse_deny("ZeroTier")), Some(2));
        // Só o relé: fica nele (já é o de agora).
        assert_eq!(best(&paths, &parse_deny("direto,zerotier")), None);
        // Nada permitido: fica como está, e a vigia fecha.
        assert_eq!(best(&paths, &parse_deny("direto,ZeroTier,rele")), None);
        // O de agora numa rede fora troca na hora, mesmo para um mais lento.
        let now = [c(zt, false, 10, true), c(Via::Relay, false, 90, false)];
        assert_eq!(best(&now, &parse_deny("ZeroTier")), Some(1));
        // Troca só por 5 ms de diferença, com 3 ms a favor do IPv6.
        let near = [c(Via::Internet, false, 20, true), c(Via::Internet, true, 19, false)];
        assert_eq!(best(&near, &[]), None);
        let far = [c(Via::Internet, false, 20, true), c(Via::Internet, true, 17, false)];
        assert_eq!(best(&far, &[]), Some(1));
    }

    #[test]
    fn deny_text_round_trip() {
        assert_eq!(parse_deny(" rele, ZeroTier ,,rele\n"), vec!["rele", "ZeroTier"]);
        assert_eq!(set_allowed("rele,ZeroTier", "zerotier", true), "rele");
        assert_eq!(set_allowed("rele", "Tailscale", false), "rele,Tailscale");
        assert!(allowed(&parse_deny("rele"), DIRECT) && !allowed(&parse_deny("RELE"), RELAY));
    }

    #[test]
    fn vpn_by_mask_and_by_local_card() {
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        let vpns = [(ip("172.24.66.19"), 16, "ZeroTier"), (ip("fd80:56c2:e21c::1"), 88, "ZeroTier"), (ip("100.101.1.2"), 32, "Tailscale")];
        // O caso dela: ZeroTier /16, o outro lado em 172.24.175.x (antes saía "rede local").
        assert_eq!(vpn_in(ip("172.24.175.246"), None, &vpns), Some("ZeroTier"));
        assert_eq!(vpn_in(ip("192.168.0.9"), Some(ip("172.24.66.19")), &vpns), Some("ZeroTier"));
        assert_eq!(vpn_in(ip("fd80:56c2:e21c:0:0:12:3456:7"), None, &vpns), Some("ZeroTier"));
        assert_eq!(vpn_in(ip("100.90.3.4"), None, &vpns), Some("Tailscale"));
        assert_eq!(vpn_in(ip("192.168.0.9"), None, &vpns), None);
        assert_eq!(vendor("Radmin VPN Radmin VPN Ethernet Adapter"), Some("Radmin VPN"));
        assert_eq!(vendor("Ethernet Realtek PCIe"), None);
    }

    #[test]
    fn route_lines() {
        let a: SocketAddr = "172.24.175.246:5000".parse().unwrap();
        assert_eq!(ip_text(Via::Vpn("ZeroTier"), a, Duration::from_millis(12)), "pela VPN (ZeroTier · IPv4), 172.24.175.246:5000, 12 ms");
        let b: SocketAddr = "[2804::1]:5000".parse().unwrap();
        assert_eq!(ip_text(Via::Internet, b, Duration::from_millis(30)), "direto pela internet (IPv6), [2804::1]:5000, 30 ms");
        assert_eq!(relay_text("https://use1-1.relay.n0.iroh.link./", Duration::from_millis(80)), "pelo relé da n0 (use1-1.relay.n0.iroh.link · EUA leste), 80 ms");
        assert_eq!(relay_text("https://x.example/", Duration::from_millis(1)), "pelo relé da n0 (x.example), 1 ms");
    }
}
