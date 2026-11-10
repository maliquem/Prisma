//! Compartilhar pela internet, sem conta: identidade do aparelho, pareamento por convite e o canal
//! do iroh (n0, licença MIT ou Apache-2.0), que tenta o caminho direto, depois a VPN que ela já tem
//! (ZeroTier, Tailscale) e por último os relés públicos da n0. Tudo vai criptografado de ponta a
//! ponta; o relé só repassa pacotes que não consegue ler.
//!
//! - `--stream-net[=control]`: compartilha para os aparelhos pareados e mostra um convite de 10 min.
//! - `--stream-pair=CONVITE`: pareia com o PC que mostrou o convite.
//! - `--stream-connect=nome|saida.txt|segundos`: assiste a um PC pareado (nome ou começo do id).
//! - `--stream-devices`: mostra o id deste PC e os aparelhos pareados.
//! - `--stream-forget=nome`: esquece um aparelho.

use std::io;
use std::net::IpAddr;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iroh::endpoint::{presets, Connection, QuicTransportConfig};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, TransportAddr};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Security::Cryptography::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::host::{self, Hello};
use super::*;

const ALPN_STREAM: &[u8] = b"prisma/stream/1";
const ALPN_PAIR: &[u8] = b"prisma/pair/1";
const INVITE_TTL: Duration = Duration::from_secs(600);
const INVITE_TRIES: u32 = 5;
const INVITE_PREFIX: &str = "PRISMA-";
/// Datagrama que cabe em qualquer caminho do iroh (o QUIC garante 1200 bytes de UDP).
const SAFE_DATAGRAM: usize = 1150;

const NOTICE: &str = "Conexão pela internet: usa o iroh, um programa de terceiros (n0, licença MIT ou Apache-2.0). \
Quando não há caminho direto nem VPN, os pacotes passam pelos relés públicos da n0, sempre criptografados de ponta a ponta.";

fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("prisma-rede")
            .enable_all()
            .build()
            .expect("tokio")
    })
}

fn dir() -> PathBuf {
    crate::config::data_dir().join("compartilhar")
}

fn my_name() -> String {
    clean_name(&std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into()))
}

/// Nome de aparelho sem tabulação nem quebra de linha, até 40 letras.
fn clean_name(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).take(40).collect();
    let t = t.trim().to_string();
    if t.is_empty() {
        "sem nome".into()
    } else {
        t
    }
}

// ---------- Identidade ----------

/// Protege (ou abre) dados com a DPAPI: só este usuário do Windows, neste PC, abre de volta.
unsafe fn dpapi(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    let entropy = b"Prisma compartilhar tela";
    let inp = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let ent = CRYPT_INTEGER_BLOB { cbData: entropy.len() as u32, pbData: entropy.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    let r = if protect {
        CryptProtectData(&inp, w!("Prisma"), Some(&ent), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    } else {
        CryptUnprotectData(&inp, None, Some(&ent), None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
    };
    r.ok()?;
    if out.pbData.is_null() {
        return None;
    }
    let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
    let _ = LocalFree(Some(HLOCAL(out.pbData as *mut _)));
    Some(v)
}

/// A chave deste aparelho: criada na primeira vez e guardada protegida pela DPAPI.
fn identity() -> io::Result<SecretKey> {
    let p = dir().join("identidade.bin");
    if let Ok(blob) = std::fs::read(&p) {
        return match unsafe { dpapi(&blob, false) } {
            Some(b) if b.len() == 32 => Ok(SecretKey::from_bytes(b[..].try_into().unwrap())),
            _ => Err(io::Error::other("a identidade deste PC não abriu (foi criada por outro usuário do Windows?)")),
        };
    }
    let sk = SecretKey::generate();
    let blob = unsafe { dpapi(&sk.to_bytes(), true) }.ok_or_else(|| io::Error::other("a DPAPI do Windows recusou proteger a chave"))?;
    std::fs::create_dir_all(dir())?;
    std::fs::write(&p, blob)?;
    Ok(sk)
}

fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    unsafe {
        let st = BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
        assert!(st.is_ok(), "BCryptGenRandom");
    }
    b
}

// ---------- Aparelhos pareados ----------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Role {
    /// Pode assistir a este PC.
    Viewer,
    /// Este PC assiste a ele.
    Host,
}

#[derive(Clone, Debug)]
struct Device {
    role: Role,
    id: EndpointId,
    name: String,
    /// Pode usar mouse e teclado (só vale para quem assiste a este PC).
    control: bool,
    /// Pergunta antes de cada conexão.
    ask: bool,
}

fn hex(id: &EndpointId) -> String {
    data_encoding::HEXLOWER.encode(id.as_bytes())
}

fn id_from_hex(s: &str) -> Option<EndpointId> {
    let b = data_encoding::HEXLOWER_PERMISSIVE.decode(s.as_bytes()).ok()?;
    EndpointId::from_bytes(b.as_slice().try_into().ok()?).ok()
}

fn devices_path() -> PathBuf {
    dir().join("aparelhos.tsv")
}

/// Uma linha por aparelho: papel, id, permissão, se pergunta e nome.
fn load() -> Vec<Device> {
    let Ok(s) = std::fs::read_to_string(devices_path()) else { return Vec::new() };
    s.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let role = match f.next()? {
                "assiste" => Role::Viewer,
                "host" => Role::Host,
                _ => return None,
            };
            let id = id_from_hex(f.next()?)?;
            let control = f.next()? == "controlar";
            let ask = f.next()? == "perguntar";
            let name = clean_name(f.next()?);
            Some(Device { role, id, name, control, ask })
        })
        .collect()
}

fn save(list: &[Device]) -> io::Result<()> {
    let mut s = String::from("# Aparelhos pareados do Prisma: papel, id, permissão, conexão, nome\n");
    for d in list {
        s += &format!(
            "{}\t{}\t{}\t{}\t{}\n",
            if d.role == Role::Viewer { "assiste" } else { "host" },
            hex(&d.id),
            if d.control { "controlar" } else { "ver" },
            if d.ask { "perguntar" } else { "confiar" },
            d.name
        );
    }
    std::fs::create_dir_all(dir())?;
    let tmp = devices_path().with_extension("tmp");
    std::fs::write(&tmp, s)?;
    std::fs::rename(tmp, devices_path())
}

fn remember(dev: Device) -> io::Result<()> {
    let mut list = load();
    list.retain(|d| !(d.role == dev.role && d.id == dev.id));
    list.push(dev);
    save(&list)
}

/// Acha um aparelho pelo nome (sem diferença de maiúsculas) ou pelo começo do id.
fn find(list: &[Device], role: Role, key: &str) -> Option<Device> {
    let key = key.trim();
    let mine = list.iter().filter(|d| d.role == role);
    mine.clone()
        .find(|d| d.name.eq_ignore_ascii_case(key))
        .or_else(|| (key.len() >= 6).then(|| mine.clone().find(|d| hex(&d.id).starts_with(&key.to_ascii_lowercase()))).flatten())
        .cloned()
}

// ---------- Convite e código de confirmação ----------

struct Invite {
    secret: [u8; 16],
    made: Instant,
    tries: u32,
    used: bool,
}

impl Invite {
    /// Confere o segredo, contando a tentativa. Depois de usado, vencido ou de 5 tentativas, recusa.
    fn check(&mut self, secret: &[u8]) -> bool {
        if self.used || self.made.elapsed() > INVITE_TTL || self.tries >= INVITE_TRIES {
            return false;
        }
        self.tries += 1;
        // Comparação em tempo constante.
        blake3::hash(secret) == blake3::hash(&self.secret)
    }
}

fn invite_text(id: &EndpointId, secret: &[u8; 16]) -> String {
    let mut b = id.as_bytes().to_vec();
    b.extend_from_slice(secret);
    format!("{INVITE_PREFIX}{}", data_encoding::BASE32_NOPAD.encode(&b))
}

fn parse_invite(s: &str) -> Option<(EndpointId, [u8; 16])> {
    let s: String = s.trim().chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
    let s = s.strip_prefix(INVITE_PREFIX).unwrap_or(&s);
    let b = data_encoding::BASE32_NOPAD.decode(s.as_bytes()).ok()?;
    if b.len() != 48 {
        return None;
    }
    let id = EndpointId::from_bytes(b[..32].try_into().ok()?).ok()?;
    Some((id, b[32..].try_into().ok()?))
}

/// Seis dígitos que os dois lados mostram: se forem iguais, ninguém se meteu no meio.
fn sas(host: &EndpointId, viewer: &EndpointId, secret: &[u8]) -> String {
    let mut h = blake3::Hasher::new_derive_key("Prisma 2026-11 pareamento, codigo de confirmacao");
    h.update(host.as_bytes());
    h.update(viewer.as_bytes());
    h.update(secret);
    let n = u32::from_le_bytes(h.finalize().as_bytes()[..4].try_into().unwrap()) % 1_000_000;
    format!("{:03} {:03}", n / 1000, n % 1000)
}

// ---------- Canal ----------

async fn endpoint(sk: SecretKey, alpns: Vec<Vec<u8>>) -> std::result::Result<Endpoint, String> {
    // Um quadro-chave em 4K passa de 600 KB: os buffers de datagramas precisam caber nele.
    let cfg = QuicTransportConfig::builder()
        .datagram_send_buffer_size(8 << 20)
        .datagram_receive_buffer_size(Some(8 << 20))
        .keep_alive_interval(Duration::from_secs(1))
        .build();
    Endpoint::builder(presets::N0).secret_key(sk).alpns(alpns).transport_config(cfg).bind().await.map_err(|e| e.to_string())
}

/// Datagramas do iroh, com a leitura num canal comum para o resto do código não precisar de async.
struct Net {
    conn: Connection,
    rx: Mutex<mpsc::Receiver<Vec<u8>>>,
}

impl Net {
    fn new(conn: Connection) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let c = conn.clone();
        rt().spawn(async move {
            while let Ok(d) = c.read_datagram().await {
                if tx.send(d.to_vec()).is_err() {
                    break;
                }
            }
        });
        Arc::new(Net { conn, rx: Mutex::new(rx) })
    }
}

impl Drop for Net {
    fn drop(&mut self) {
        self.conn.close(0u32.into(), b"tchau");
    }
}

impl Link for Net {
    fn send(&self, b: &[u8]) {
        let _ = self.conn.send_datagram(b.to_vec().into());
    }

    fn recv(&self, buf: &mut [u8], wait: Duration) -> Poll {
        let rx = self.rx.lock().unwrap();
        let r = if wait.is_zero() {
            rx.try_recv().map_err(|e| matches!(e, mpsc::TryRecvError::Disconnected))
        } else {
            rx.recv_timeout(wait).map_err(|e| matches!(e, mpsc::RecvTimeoutError::Disconnected))
        };
        match r {
            Ok(d) => {
                let n = d.len().min(buf.len());
                buf[..n].copy_from_slice(&d[..n]);
                Poll::Got(n)
            }
            Err(true) => Poll::Closed,
            Err(false) => Poll::Empty,
        }
    }

    /// O máximo muda quando o iroh troca de caminho (no teste, de mais de 1200 para 1162 bytes), e
    /// um pacote maior que o novo máximo é recusado. Então fica abaixo do que cabe em qualquer caminho.
    fn mtu(&self) -> usize {
        self.conn.max_datagram_size().unwrap_or(SAFE_DATAGRAM).min(SAFE_DATAGRAM)
    }

    fn route(&self) -> String {
        for p in self.conn.paths().iter() {
            if !p.is_selected() {
                continue;
            }
            let rtt = p.rtt().as_secs_f64() * 1000.0;
            return match p.remote_addr() {
                TransportAddr::Ip(a) => match vpn_of(a.ip()) {
                    Some(v) => format!("pela VPN ({v}), {a}, {rtt:.0} ms"),
                    None if is_local(a.ip()) => format!("direto na rede local, {a}, {rtt:.0} ms"),
                    None => format!("direto pela internet, {a}, {rtt:.0} ms"),
                },
                TransportAddr::Relay(u) => format!("pelo relé da n0 ({u}), {rtt:.0} ms"),
                other => format!("{other:?}, {rtt:.0} ms"),
            };
        }
        "procurando caminho".into()
    }
}

fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_link_local() || v.is_loopback(),
        IpAddr::V6(v) => v.is_loopback() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// O endereço é de uma VPN deste PC (ZeroTier ou Tailscale)? Compara com os adaptadores daqui.
fn vpn_of(ip: IpAddr) -> Option<&'static str> {
    static NETS: OnceLock<Vec<(String, &'static str)>> = OnceLock::new();
    let nets = NETS.get_or_init(|| {
        let mut v = Vec::new();
        for n in crate::sys::nets::list() {
            let t = format!("{} {}", n.name, n.desc).to_lowercase();
            let vendor = if t.contains("zerotier") {
                "ZeroTier"
            } else if t.contains("tailscale") {
                "Tailscale"
            } else {
                continue;
            };
            if let Some(i) = n.ip.rfind('.') {
                v.push((n.ip[..=i].to_string(), vendor));
            }
        }
        v
    });
    let IpAddr::V4(v4) = ip else { return None };
    let s = v4.to_string();
    // Tailscale dá endereços soltos em 100.64.0.0/10.
    let o = v4.octets();
    if o[0] == 100 && (o[1] & 0xc0) == 64 && nets.iter().any(|(_, v)| *v == "Tailscale") {
        return Some("Tailscale");
    }
    nets.iter().find(|(p, _)| s.starts_with(p.as_str())).map(|(_, v)| *v)
}

// ---------- Janelas de pergunta ----------

fn message(text: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe { MessageBoxW(None, &HSTRING::from(text), w!("Prisma: compartilhar tela"), flags | MB_TOPMOST | MB_SETFOREGROUND) }
}

/// Pergunta se pareia. Devolve se deve perguntar a cada conexão, ou None se recusou.
fn approve_pair(name: &str, code: &str, control: bool) -> Option<bool> {
    let what = if control { "assistir a este PC e usar o mouse e o teclado dele" } else { "assistir a este PC" };
    let text = format!(
        "«{name}» quer parear para {what}.\n\nCódigo de confirmação: {code}\nConfira se o outro PC mostra o mesmo código. Se não mostrar, clique em Cancelar.\n\nSim: confiar neste aparelho.\nNão: parear, mas perguntar a cada conexão.\nCancelar: recusar."
    );
    match message(&text, MB_YESNOCANCEL | MB_ICONQUESTION | MB_DEFBUTTON3) {
        IDYES => Some(false),
        IDNO => Some(true),
        _ => None,
    }
}

fn approve_connect(name: &str) -> bool {
    message(&format!("«{name}» quer se conectar a este PC. Permitir?"), MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2) == IDYES
}

// ---------- Host ----------

async fn pair_host(conn: Connection, me: EndpointId, invite: Arc<Mutex<Invite>>, control: bool) {
    let who = conn.remote_id();
    let run = async {
        let (mut send, mut recv) = conn.accept_bi().await.ok()?;
        let msg = recv.read_to_end(256).await.ok()?;
        if msg.len() < 16 {
            return None;
        }
        let (secret, name) = msg.split_at(16);
        let name = clean_name(&String::from_utf8_lossy(name));
        if !invite.lock().unwrap().check(secret) {
            eprintln!("pedido de pareamento de «{name}» recusado: convite errado, usado ou vencido");
            let _ = send.write_all(&[0]).await;
            let _ = send.finish();
            return None;
        }
        let code = sas(&me, &who, secret);
        eprintln!("«{name}» quer parear. Código de confirmação: {code}");
        let n2 = name.clone();
        let answer = tokio::task::spawn_blocking(move || approve_pair(&n2, &code, control)).await.ok()?;
        let mut reply = vec![answer.is_some() as u8];
        if let Some(ask) = answer {
            invite.lock().unwrap().used = true;
            match remember(Device { role: Role::Viewer, id: who, name: name.clone(), control, ask }) {
                Ok(()) => eprintln!("pareado com «{name}»"),
                Err(e) => eprintln!("pareou, mas não deu para guardar: {e}"),
            }
            reply.extend_from_slice(my_name().as_bytes());
        } else {
            eprintln!("pareamento com «{name}» recusado");
        }
        send.write_all(&reply).await.ok()?;
        send.finish().ok()?;
        // Espera o outro lado ler a resposta e fechar.
        let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
        Some(())
    };
    let _ = run.await;
}

async fn accept_loop(ep: Endpoint, invite: Arc<Mutex<Invite>>, tx: mpsc::Sender<(Connection, Device)>, control: bool) {
    let me = ep.id();
    while let Some(incoming) = ep.accept().await {
        let (invite, tx) = (invite.clone(), tx.clone());
        tokio::spawn(async move {
            let Ok(accepting) = incoming.accept() else { return };
            let Ok(conn) = accepting.await else { return };
            let who = conn.remote_id();
            if conn.alpn() == ALPN_PAIR {
                pair_host(conn, me, invite, control).await;
            } else if conn.alpn() == ALPN_STREAM {
                let Some(dev) = load().into_iter().find(|d| d.role == Role::Viewer && d.id == who) else {
                    eprintln!("conexão recusada: {} não está pareado", who.fmt_short());
                    conn.close(1u32.into(), b"nao pareado");
                    return;
                };
                if dev.ask {
                    let name = dev.name.clone();
                    if !tokio::task::spawn_blocking(move || approve_connect(&name)).await.unwrap_or(false) {
                        eprintln!("conexão de «{}» recusada", dev.name);
                        conn.close(2u32.into(), b"recusado");
                        return;
                    }
                }
                let _ = tx.send((conn, dev));
            } else {
                conn.close(3u32.into(), b"protocolo desconhecido");
            }
        });
    }
}

pub fn host(control: bool) {
    eprintln!("{NOTICE}");
    let sk = match identity() {
        Ok(k) => k,
        Err(e) => return eprintln!("{e}"),
    };
    let ep = match rt().block_on(endpoint(sk, vec![ALPN_STREAM.to_vec(), ALPN_PAIR.to_vec()])) {
        Ok(e) => e,
        Err(e) => return eprintln!("não deu para abrir a conexão: {e}"),
    };
    if rt().block_on(tokio::time::timeout(Duration::from_secs(10), ep.online())).is_err() {
        eprintln!("sem resposta do relé da n0 em 10 s: só a rede local e a VPN vão funcionar por enquanto");
    }
    let secret = random::<16>();
    let invite = Arc::new(Mutex::new(Invite { secret, made: Instant::now(), tries: 0, used: false }));
    let mode = if control { "com mouse e teclado de quem assiste" } else { "só para ver" };
    eprintln!("Prisma compartilhando pela internet ({mode}). Este PC: «{}», id {}", my_name(), ep.id().fmt_short());
    eprintln!("Convite para parear outro PC (vale 10 minutos e uma vez):\n\n{}\n", invite_text(&ep.id(), &secret));
    let (tx, rx) = mpsc::channel();
    rt().spawn(accept_loop(ep.clone(), invite, tx, control));

    let Some(d) = host::prepare() else { return };
    let mut buf = vec![0u8; 2048];
    while let Ok((conn, dev)) = rx.recv() {
        eprintln!("«{}» conectou", dev.name);
        let link = Net::new(conn);
        let t = Instant::now();
        let hello = loop {
            if t.elapsed() > Duration::from_secs(10) {
                break None;
            }
            match link.recv(&mut buf, Duration::from_millis(200)) {
                Poll::Got(n) => {
                    if let Some(h) = read(&buf[..n]).as_ref().and_then(Hello::from_msg) {
                        break Some(h);
                    }
                }
                Poll::Closed => break None,
                _ => {}
            }
        };
        let Some(hello) = hello else {
            eprintln!("«{}» não mandou a tela dele a tempo", dev.name);
            continue;
        };
        host::serve(&d, link, hello, control && dev.control);
    }
}

// ---------- Quem assiste ----------

/// Pede o pareamento e espera a resposta. Devolve o nome do outro PC se ele aceitou.
async fn ask_pair(ep: &Endpoint, addr: EndpointAddr, secret: &[u8; 16]) -> std::result::Result<String, String> {
    let host_id = addr.id;
    let conn = tokio::time::timeout(Duration::from_secs(30), ep.connect(addr, ALPN_PAIR))
        .await
        .map_err(|_| "o outro PC não respondeu em 30 s".to_string())?
        .map_err(|e| e.to_string())?;
    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| e.to_string())?;
    let mut msg = secret.to_vec();
    msg.extend_from_slice(my_name().as_bytes());
    send.write_all(&msg).await.map_err(|e| e.to_string())?;
    send.finish().map_err(|e| e.to_string())?;
    eprintln!("Código de confirmação: {}. Confira se o outro PC mostra o mesmo e aceite lá.", sas(&host_id, &ep.id(), secret));
    let reply = recv.read_to_end(256).await.map_err(|e| e.to_string())?;
    conn.close(0u32.into(), b"ok");
    match reply.split_first() {
        Some((1, name)) => Ok(clean_name(&String::from_utf8_lossy(name))),
        _ => Err("o outro PC recusou, ou o convite venceu".to_string()),
    }
}

pub fn pair(text: &str) {
    eprintln!("{NOTICE}");
    let Some((host_id, secret)) = parse_invite(text) else {
        return eprintln!("convite inválido");
    };
    let sk = match identity() {
        Ok(k) => k,
        Err(e) => return eprintln!("{e}"),
    };
    let r: std::result::Result<String, String> = rt().block_on(async {
        let ep = endpoint(sk, Vec::new()).await?;
        let r = ask_pair(&ep, host_id.into(), &secret).await;
        ep.close().await;
        let name = r?;
        remember(Device { role: Role::Host, id: host_id, name: name.clone(), control: false, ask: false }).map_err(|e| e.to_string())?;
        Ok(name)
    });
    match r {
        Ok(name) => eprintln!("pareado com «{name}». Para assistir: --stream-connect={name}"),
        Err(e) => eprintln!("não pareou: {e}"),
    }
}

pub fn connect(arg: &str) {
    eprintln!("{NOTICE}");
    let mut parts = arg.split('|');
    let target = parts.next().unwrap_or("");
    let out_file = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    let secs: Option<u64> = parts.next().and_then(|s| s.parse().ok());
    let Some(dev) = find(&load(), Role::Host, target) else {
        return eprintln!("nenhum PC pareado com o nome «{target}» (veja --stream-devices)");
    };
    let sk = match identity() {
        Ok(k) => k,
        Err(e) => return eprintln!("{e}"),
    };
    let ep = match rt().block_on(endpoint(sk, Vec::new())) {
        Ok(e) => e,
        Err(e) => return eprintln!("não deu para abrir a conexão: {e}"),
    };
    eprintln!("conectando a «{}»", dev.name);
    let conn = rt().block_on(async { tokio::time::timeout(Duration::from_secs(30), ep.connect(dev.id, ALPN_STREAM)).await });
    match conn {
        Ok(Ok(conn)) => {
            let link = Net::new(conn);
            eprintln!("conectado: {}", link.route());
            view::open(link, out_file.as_deref(), secs);
        }
        Ok(Err(e)) => eprintln!("não conectou: {e}"),
        Err(_) => eprintln!("«{}» não respondeu em 30 s (está com o Prisma compartilhando?)", dev.name),
    }
    rt().block_on(ep.close());
}

pub fn devices() {
    match identity() {
        Ok(k) => eprintln!("Este PC: «{}», id {}", my_name(), hex(&k.public())),
        Err(e) => eprintln!("{e}"),
    }
    let list = load();
    if list.is_empty() {
        eprintln!("nenhum aparelho pareado");
    }
    for d in list {
        let role = match d.role {
            Role::Viewer => format!("pode assistir a este PC ({}, {})", if d.control { "com mouse e teclado" } else { "só ver" }, if d.ask { "pergunta a cada conexão" } else { "confiável" }),
            Role::Host => "este PC assiste a ele".into(),
        };
        eprintln!("«{}» {}: {role}", d.name, &hex(&d.id)[..10]);
    }
}

pub fn forget(key: &str) {
    let mut list = load();
    let before = list.len();
    let names: Vec<String> = [Role::Viewer, Role::Host].iter().filter_map(|&r| find(&list, r, key)).map(|d| hex(&d.id)).collect();
    list.retain(|d| !names.contains(&hex(&d.id)));
    if list.len() == before {
        return eprintln!("nenhum aparelho com o nome «{key}»");
    }
    match save(&list) {
        Ok(()) => eprintln!("esquecido: «{key}»"),
        Err(e) => eprintln!("não deu para salvar: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_round_trip() {
        let id = SecretKey::from_bytes(&[7; 32]).public();
        let secret = [9u8; 16];
        let t = invite_text(&id, &secret);
        assert!(t.starts_with(INVITE_PREFIX));
        let (id2, s2) = parse_invite(&format!("  {}  ", t.to_lowercase())).unwrap();
        assert_eq!(id2, id);
        assert_eq!(s2, secret);
        assert!(parse_invite("PRISMA-ABC").is_none());
    }

    #[test]
    fn sas_depends_on_everything() {
        let a = SecretKey::from_bytes(&[1; 32]).public();
        let b = SecretKey::from_bytes(&[2; 32]).public();
        let s = sas(&a, &b, &[3; 16]);
        assert_eq!(s.len(), 7);
        assert_eq!(s, sas(&a, &b, &[3; 16]));
        assert_ne!(s, sas(&b, &a, &[3; 16]));
        assert_ne!(s, sas(&a, &b, &[4; 16]));
    }

    #[test]
    fn invite_limits() {
        let mut inv = Invite { secret: [5; 16], made: Instant::now(), tries: 0, used: false };
        for _ in 0..4 {
            assert!(!inv.check(&[6; 16]));
        }
        assert!(inv.check(&[5; 16]));
        // Quinta tentativa gasta: nem o certo passa mais.
        assert!(!inv.check(&[5; 16]));
        let mut old = Invite { secret: [5; 16], made: Instant::now() - INVITE_TTL - Duration::from_secs(1), tries: 0, used: false };
        assert!(!old.check(&[5; 16]));
    }

    /// Dois pontos do iroh neste PC, sem relé: o canal entrega os pacotes, inclusive uma rajada do
    /// tamanho de um quadro-chave em 4K (600 KB), e diz por onde foi.
    #[test]
    fn iroh_datagrams_loopback() {
        let cfg = || QuicTransportConfig::builder().datagram_send_buffer_size(8 << 20).datagram_receive_buffer_size(Some(8 << 20)).build();
        let (ea, eb, ca, cb) = rt().block_on(async {
            let a = Endpoint::builder(presets::Minimal).relay_mode(iroh::RelayMode::Disabled).alpns(vec![ALPN_STREAM.to_vec()]).transport_config(cfg()).bind().await.unwrap();
            let b = Endpoint::builder(presets::Minimal).relay_mode(iroh::RelayMode::Disabled).transport_config(cfg()).bind().await.unwrap();
            let addr = a.addr();
            let a2 = a.clone();
            let acc = tokio::spawn(async move { a2.accept().await.unwrap().accept().unwrap().await.unwrap() });
            let cb = b.connect(addr, ALPN_STREAM).await.unwrap();
            let ca = acc.await.unwrap();
            assert_eq!(ca.remote_id(), b.id());
            (a, b, ca, cb)
        });
        let (la, lb) = (Net::new(ca), Net::new(cb));
        let mtu = lb.mtu();
        assert!(mtu >= 1000, "datagrama máximo {mtu}");
        let mut buf = vec![0u8; 2048];
        lb.send(b"oi");
        assert!(matches!(la.recv(&mut buf, Duration::from_secs(2)), Poll::Got(2)));
        let chunks = 600_000 / mtu;
        let t = Instant::now();
        for i in 0..chunks {
            let mut p = vec![0u8; mtu];
            p[..4].copy_from_slice(&(i as u32).to_le_bytes());
            lb.send(&p);
        }
        let mut got = 0;
        while got < chunks && t.elapsed() < Duration::from_secs(5) {
            if let Poll::Got(_) = la.recv(&mut buf, Duration::from_millis(100)) {
                got += 1;
            }
        }
        eprintln!("rajada de {chunks} pacotes de {mtu} bytes: {got} chegaram em {:.1} ms; caminho: {}", t.elapsed().as_secs_f64() * 1000.0, lb.route());
        assert_eq!(got, chunks);
        drop((la, lb));
        rt().block_on(async {
            ea.close().await;
            eb.close().await;
        });
    }

    /// Convite errado: o host recusa sem abrir janela, e o convite certo continua valendo.
    #[test]
    fn wrong_invite_is_refused() {
        let r = rt().block_on(async {
            let a = Endpoint::builder(presets::Minimal).relay_mode(iroh::RelayMode::Disabled).alpns(vec![ALPN_PAIR.to_vec()]).bind().await.unwrap();
            let b = Endpoint::builder(presets::Minimal).relay_mode(iroh::RelayMode::Disabled).bind().await.unwrap();
            let invite = Arc::new(Mutex::new(Invite { secret: [1; 16], made: Instant::now(), tries: 0, used: false }));
            let (a2, inv2) = (a.clone(), invite.clone());
            let host = tokio::spawn(async move {
                let conn = a2.accept().await.unwrap().accept().unwrap().await.unwrap();
                pair_host(conn, a2.id(), inv2, false).await;
            });
            let r = ask_pair(&b, a.addr(), &[2; 16]).await;
            let _ = host.await;
            let inv = invite.lock().unwrap();
            assert_eq!((inv.tries, inv.used), (1, false));
            drop(inv);
            a.close().await;
            b.close().await;
            r
        });
        assert!(r.is_err());
    }

    #[test]
    fn names_are_clean() {
        assert_eq!(clean_name("a\tb\nc"), "abc");
        assert_eq!(clean_name("   "), "sem nome");
        assert_eq!(clean_name(&"x".repeat(80)).len(), 40);
    }
}
