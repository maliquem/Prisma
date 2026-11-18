//! Compartilhar pela internet, sem conta: identidade do aparelho, pareamento por convite e o canal
//! do iroh (n0, licença MIT ou Apache-2.0), que tenta o caminho direto, depois a VPN que ela já tem
//! (ZeroTier, Tailscale) e por último os relés públicos da n0. Tudo vai criptografado de ponta a
//! ponta; o relé só repassa pacotes que não consegue ler.
//!
//! - `--stream-net[=permissões]`: compartilha para os aparelhos pareados e mostra um convite de 10 min.
//!   As permissões valem para quem parear com esse convite (padrão `ver`; veja `parse_perms`).
//!   Várias pessoas assistem juntas, cada uma com as permissões do aparelho dela.
//! - `--stream-allow=nome|permissões`: muda as permissões de um aparelho pareado.
//! - `--stream-pair=CONVITE`: pareia com o PC que mostrou o convite.
//! - `--stream-connect=nome|saida.txt|segundos`: assiste a um PC pareado (nome ou começo do id).
//! - `--stream-devices`: mostra o id deste PC e os aparelhos pareados.
//! - `--stream-forget=nome`: esquece um aparelho.

use std::io;
use std::net::{IpAddr, Ipv4Addr};
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

use super::host::{self, Hello, Joiner};
use super::*;
use super::code;
use super::wake::{self, Mac};

const ALPN_STREAM: &[u8] = b"prisma/stream/1";
const ALPN_PAIR: &[u8] = b"prisma/pair/1";
/// Só "estou ligado" (e os MACs), ou ligar outro PC da rede: sem sessão, sem pergunta, sem captura.
const ALPN_PRESENCE: &[u8] = b"prisma/presenca/1";
/// Quanto a página espera cada PC responder, e de quanto em quanto tempo pergunta.
const PRESENCE_WAIT: Duration = Duration::from_secs(5);
const PRESENCE_ROUND: Duration = Duration::from_secs(30);
const INVITE_TTL: Duration = Duration::from_secs(600);
/// Tentativas por aparelho que tenta, e no total: quem erra de propósito não gasta o convite dos outros.
const INVITE_TRIES: usize = 5;
const INVITE_TRIES_ALL: usize = 50;
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

use super::{dir, user_dir};

/// O nome das opções, ou o do PC no Windows.
fn my_name() -> String {
    let o = Opts::load().name;
    clean_name(&if o.is_empty() { std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into()) } else { o })
}

/// Este PC está assistindo a outro agora (a janela de quem assiste está aberta).
fn viewing_now() -> bool {
    unsafe { FindWindowW(w!("PrismaAssistir"), None).is_ok() }
}

/// Nome de aparelho sem tabulação nem quebra de linha, até 40 letras.
pub(crate) fn clean_name(s: &str) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).take(40).collect();
    let t = t.trim().to_string();
    if t.is_empty() {
        "sem nome".into()
    } else {
        t
    }
}

// ---------- Identidade ----------

/// Protege (ou abre) dados com a DPAPI: só este usuário do Windows, neste PC, abre de volta. Com
/// `machine`, qualquer conta deste PC abre (o serviço é SYSTEM); aí quem protege é a pasta.
unsafe fn dpapi(data: &[u8], protect: bool, machine: bool) -> Option<Vec<u8>> {
    let entropy = b"Prisma compartilhar tela";
    let inp = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let ent = CRYPT_INTEGER_BLOB { cbData: entropy.len() as u32, pbData: entropy.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    let r = if protect {
        let flags = if machine { CRYPTPROTECT_UI_FORBIDDEN | CRYPTPROTECT_LOCAL_MACHINE } else { CRYPTPROTECT_UI_FORBIDDEN };
        CryptProtectData(&inp, w!("Prisma"), Some(&ent), None, None, flags, &mut out)
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
        return match unsafe { dpapi(&blob, false, false) } {
            Some(b) if b.len() == 32 => Ok(SecretKey::from_bytes(b[..].try_into().unwrap())),
            _ => Err(io::Error::other("a identidade deste PC não abriu (foi criada por outro usuário do Windows?)")),
        };
    }
    let sk = SecretKey::generate();
    let machine = dir() != user_dir();
    let blob = unsafe { dpapi(&sk.to_bytes(), true, machine) }.ok_or_else(|| io::Error::other("a DPAPI do Windows recusou proteger a chave"))?;
    std::fs::create_dir_all(dir())?;
    crate::config::write_safe(&p, blob)?;
    Ok(sk)
}

pub(super) fn random<const N: usize>() -> [u8; N] {
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
    /// O que pode fazer além de ver (só vale para quem assiste a este PC): bits de `SOUND`, `PAD`,
    /// `KEYBOARD` e `MOUSE`.
    perms: u8,
    /// Pergunta antes de cada conexão.
    ask: bool,
    /// Os MACs e os broadcasts que o PC mandou na última conexão (só nos PCs que este assiste).
    macs: Vec<Mac>,
    nets: Vec<Ipv4Addr>,
}

fn hex(id: &EndpointId) -> String {
    data_encoding::HEXLOWER.encode(id.as_bytes())
}

fn id_from_hex(s: &str) -> Option<EndpointId> {
    let b = data_encoding::HEXLOWER_PERMISSIVE.decode(s.as_bytes()).ok()?;
    EndpointId::from_bytes(b.as_slice().try_into().ok()?).ok()
}

/// Quem assiste a este PC fica na pasta do compartilhamento (a do serviço, que só administrador
/// altera); os PCs que este assiste, na pasta do usuário, para parear com eles sem administrador.
/// Sem o serviço, as duas são a mesma.
fn devices_path(role: Role) -> PathBuf {
    match role {
        Role::Viewer => dir(),
        Role::Host => user_dir(),
    }
    .join("aparelhos.tsv")
}

/// Marca de que os PCs que este assiste já saíram do arquivo do serviço (versões antigas os
/// guardavam lá).
fn moved_mark() -> PathBuf {
    user_dir().join("aparelhos-movidos.txt")
}

/// Os dois papéis. Com o serviço, cada arquivo vale só para o seu papel.
fn load() -> Vec<Device> {
    let (shared, mine) = (devices_path(Role::Viewer), devices_path(Role::Host));
    if shared == mine {
        return read_list(&shared);
    }
    let migrate = !moved_mark().exists();
    let (list, extra) = join_lists(read_list(&shared), read_list(&mine), migrate);
    if migrate {
        let own: Vec<Device> = read_list(&mine).into_iter().chain(extra).collect();
        if write_list(&mine, &own).is_ok() {
            let _ = std::fs::write(moved_mark(), "");
        }
    }
    list
}

/// Quem assiste vem do arquivo do serviço; os PCs que este assiste, do arquivo do usuário. Com
/// `migrate`, os PCs que este assiste que só estão no arquivo do serviço entram também e voltam em
/// `extra`, para gravar no do usuário.
fn join_lists(shared: Vec<Device>, mine: Vec<Device>, migrate: bool) -> (Vec<Device>, Vec<Device>) {
    let hosts: Vec<Device> = mine.into_iter().filter(|d| d.role == Role::Host).collect();
    let extra: Vec<Device> = if migrate { shared.iter().filter(|d| d.role == Role::Host && !hosts.iter().any(|h| h.id == d.id)).cloned().collect() } else { Vec::new() };
    let list = shared.into_iter().filter(|d| d.role == Role::Viewer).chain(hosts).chain(extra.iter().cloned()).collect();
    (list, extra)
}

/// Uma linha por aparelho: papel, id, permissão, se pergunta e nome.
fn read_list(path: &std::path::Path) -> Vec<Device> {
    let Ok(s) = std::fs::read_to_string(path) else { return Vec::new() };
    s.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let role = match f.next()? {
                "assiste" => Role::Viewer,
                "host" => Role::Host,
                _ => return None,
            };
            let id = id_from_hex(f.next()?)?;
            let perms = parse_perms(f.next()?).unwrap_or(SOUND);
            let ask = f.next()? == "perguntar";
            let name = clean_name(f.next()?);
            // Colunas novas no fim: a 1.8.0 lê o arquivo e ignora o resto.
            let macs = f.next().map(|v| v.split(',').filter_map(wake::mac_parse).collect()).unwrap_or_default();
            let nets = f.next().map(|v| v.split(',').filter_map(|x| x.parse().ok()).collect()).unwrap_or_default();
            Some(Device { role, id, name, perms, ask, macs, nets })
        })
        .collect()
}

/// Grava a lista de um papel (sem o serviço, os dois papéis moram no mesmo arquivo).
fn save(list: &[Device], role: Role) -> io::Result<()> {
    let same = devices_path(Role::Viewer) == devices_path(Role::Host);
    let rows: Vec<Device> = list.iter().filter(|d| same || d.role == role).cloned().collect();
    write_list(&devices_path(role), &rows)
}

fn write_list(path: &std::path::Path, list: &[Device]) -> io::Result<()> {
    let mut s = String::from("# Aparelhos pareados do Prisma: papel, id, permissão, conexão, nome, MACs, redes\n");
    for d in list {
        s += &format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            if d.role == Role::Viewer { "assiste" } else { "host" },
            hex(&d.id),
            perms_key(d.perms),
            if d.ask { "perguntar" } else { "confiar" },
            d.name,
            d.macs.iter().map(wake::mac_text).collect::<Vec<_>>().join(","),
            d.nets.iter().map(|n| n.to_string()).collect::<Vec<_>>().join(",")
        );
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    crate::config::write_safe(path, s)
}

/// O Prisma, o host e o ajudante do serviço mexem na lista ao mesmo tempo: sem trava, um apaga o
/// aparelho que o outro acabou de parear. Arquivo aberto sem compartilhar serve de trava.
/// shortcut: quem não pega a trava em 2 s segue sem ela; trocar por um mutex nomeado se isso aparecer no log.
fn devices_lock(role: Role) -> Option<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    let path = devices_path(role).with_file_name("aparelhos.lock");
    let _ = std::fs::create_dir_all(path.parent()?);
    let t = Instant::now();
    loop {
        match std::fs::OpenOptions::new().write(true).create(true).truncate(false).share_mode(0).open(&path) {
            Ok(f) => return Some(f),
            Err(_) if t.elapsed() < Duration::from_secs(2) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return None,
        }
    }
}

fn remember(dev: Device) -> io::Result<()> {
    let role = dev.role;
    let _lock = devices_lock(role);
    let mut list = load();
    list.retain(|d| !(d.role == dev.role && d.id == dev.id));
    list.push(dev);
    save(&list, role)
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
    /// Até quando vale, em segundos desde 1970. 0: revogado.
    until: u64,
    /// Quem tentou, uma vez por tentativa.
    tries: Vec<EndpointId>,
    used: bool,
    /// Permissões de quem parear com este convite.
    perms: u8,
}

/// Convite do serviço, num arquivo: segredo, permissões e validade (segundos desde 1970).
fn invite_path() -> PathBuf {
    dir().join("convite.tsv")
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn load_invite_file() -> Option<Invite> {
    let s = std::fs::read_to_string(invite_path()).ok()?;
    let mut f = s.trim().split('\t');
    let secret: [u8; 16] = data_encoding::HEXLOWER_PERMISSIVE.decode(f.next()?.as_bytes()).ok()?.try_into().ok()?;
    let perms = parse_perms(f.next()?)?;
    let until = f.next()?.parse::<u64>().ok()?;
    Some(Invite { secret, until, tries: Vec::new(), used: false, perms })
}

/// Quanto um convite da página pode valer, em minutos.
/// No máximo 60: o código curto tem 40 bits, e o tempo vivo é o que quem tenta adivinhar tem.
pub const INVITE_MINUTES: [u32; 4] = [5, 10, 30, 60];

/// A linha do registro do convite com a hora em que ele vence (segundos desde 1970).
pub const EXPIRES_LINE: &str = "vence em: ";

/// Revoga o convite que ainda não foi usado: o host passa a recusar o código na hora.
pub fn revoke_invite() -> std::io::Result<()> {
    let text = std::fs::read_to_string(invite_path())?;
    let mut f: Vec<String> = text.trim().split('\t').map(str::to_string).collect();
    if f.len() < 3 {
        return Ok(());
    }
    f[2] = "0".into();
    crate::config::write_safe(&invite_path(), f.join("\t") + "\n")
}

/// `--stream-invite`: um convite novo vai para o arquivo que o host lê no pedido de pareamento (o
/// do serviço, ou o da página na pasta deste usuário), e o código curto vai para os relés.
pub fn make_invite(perms: u8, minutes: u32) {
    let sk = match identity() {
        Ok(k) => k,
        Err(e) => return eprintln!("{e}"),
    };
    let shown = code::new_code();
    let root = code::stretch(&code::normalize(&shown).unwrap_or_default());
    let secret = code::secret(&root);
    let minutes = minutes.clamp(1, 60);
    let until = unix_now() + minutes as u64 * 60;
    let line = format!("{}\t{}\t{}\n", data_encoding::HEXLOWER.encode(&secret), perms_key(perms), until);
    if let Err(e) = crate::config::write_safe(&invite_path(), line) {
        return eprintln!("não deu para gravar o convite: {e}");
    }
    eprintln!("Convite novo ({}; vale {minutes} minutos e uma vez).", perms_text(perms));
    eprintln!("{EXPIRES_LINE}{until}");
    eprintln!("{CODE_LINE}{shown}");
    // O relé e os IPs vêm do host que está rodando (o ajudante do serviço ou a página).
    let lines = std::fs::read_to_string(addr_path()).unwrap_or_default();
    let addr = EndpointAddr::from_parts(sk.public(), code::parse_addrs(lines.lines()));
    eprintln!("Convite longo, se o código não for achado:\n\n{}\n", invite_text(&addr, &secret));
    eprintln!("A pergunta de confirmação aparece na tela deste PC.");
    let n = rt().block_on(code::publish(&root, &addr, until));
    published(n);
}

/// Onde o host deixa o relé e os IPs dele, para o convite curto levar junto.
fn addr_path() -> PathBuf {
    dir().join("endereco.txt")
}

/// Hora local para as linhas do log do host.
fn now_text() -> String {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// Cada vez que o relé do host liga ou cai, uma linha no log (no serviço, o `servico.log`) e o
/// endereço novo no arquivo do convite: um pareamento que não chega fica visível.
async fn watch_relay(ep: Endpoint) {
    use iroh::Watcher;
    let mut w = ep.home_relay_status();
    let mut last = String::new();
    loop {
        let now = w.get();
        let key: String = now.iter().map(|r| format!("{}{}", r.url(), r.is_connected())).collect();
        if key != last {
            last = key;
            let text: Vec<String> = now
                .iter()
                .map(|r| match (r.is_connected(), r.last_error()) {
                    (true, _) => format!("ligado a {}", r.url()),
                    (false, Some(e)) => format!("sem ligação com {} ({e})", r.url()),
                    (false, None) => format!("tentando {}", r.url()),
                })
                .collect();
            eprintln!("{} relé: {}", now_text(), if text.is_empty() { "nenhum".into() } else { text.join("; ") });
            let _ = crate::config::write_safe(&addr_path(), code::addr_lines(&ep.addr()).join("\n"));
        }
        if w.updated().await.is_err() {
            return;
        }
    }
}

/// Leva a identidade e os aparelhos pareados para a pasta do serviço (na instalação).
pub(super) fn copy_to_service(dst: &std::path::Path) -> std::result::Result<(), String> {
    let src = user_dir();
    let sk = match std::fs::read(src.join("identidade.bin")) {
        Ok(blob) => match unsafe { dpapi(&blob, false, false) } {
            Some(b) if b.len() == 32 => SecretKey::from_bytes(b[..].try_into().unwrap()),
            _ => return Err("a identidade deste PC não abriu".into()),
        },
        Err(_) => SecretKey::generate(),
    };
    let blob = unsafe { dpapi(&sk.to_bytes(), true, true) }.ok_or("a DPAPI recusou proteger a chave")?;
    crate::config::write_safe(&dst.join("identidade.bin"), blob).map_err(|e| e.to_string())?;
    // Numa reinstalação, a pasta do serviço já tem os aparelhos mais novos.
    let list = src.join("aparelhos.tsv");
    if list.exists() && !dst.join("aparelhos.tsv").exists() {
        std::fs::copy(&list, dst.join("aparelhos.tsv")).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Devolve os aparelhos pareados (e a identidade, se faltar) à pasta de sempre (na remoção).
pub(super) fn copy_from_service(src: &std::path::Path) -> std::result::Result<(), String> {
    let dst = user_dir();
    std::fs::create_dir_all(&dst).map_err(|e| e.to_string())?;
    // Quem assiste vem do serviço; os PCs que este assiste já estão na pasta do usuário.
    if src.join("aparelhos.tsv").exists() {
        let mine = dst.join("aparelhos.tsv");
        let (list, _) = join_lists(read_list(&src.join("aparelhos.tsv")), read_list(&mine), !moved_mark().exists());
        write_list(&mine, &list).map_err(|e| e.to_string())?;
    }
    if !dst.join("identidade.bin").exists() {
        if let Ok(blob) = std::fs::read(src.join("identidade.bin")) {
            let key = unsafe { dpapi(&blob, false, true) }.ok_or("a identidade do serviço não abriu")?;
            let mine = unsafe { dpapi(&key, true, false) }.ok_or("a DPAPI recusou proteger a chave")?;
            crate::config::write_safe(&dst.join("identidade.bin"), mine).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

impl Invite {
    /// Confere o segredo, contando a tentativa de `who`. Depois de usado, vencido, de 5 tentativas
    /// deste aparelho ou de 50 no total, recusa.
    fn check(&mut self, secret: &[u8], who: EndpointId) -> bool {
        let mine = self.tries.iter().filter(|t| **t == who).count();
        if self.used || unix_now() >= self.until || mine >= INVITE_TRIES || self.tries.len() >= INVITE_TRIES_ALL {
            return false;
        }
        self.tries.push(who);
        // Comparação em tempo constante.
        blake3::hash(secret) == blake3::hash(&self.secret)
    }
}

/// A linha do log de onde a página tira o código curto.
pub const CODE_LINE: &str = "Código do convite: ";
/// A linha que diz se o código foi publicado.
pub const CODE_OK: &str = "código publicado";
pub const CODE_FAIL: &str = "o código não foi publicado";

fn published(n: usize) {
    if n > 0 {
        eprintln!("{CODE_OK} ({n} de 3 relés)");
    } else {
        eprintln!("{CODE_FAIL}: mande o convite longo");
    }
}

/// Id, segredo e, depois deles, o relé e os IPs do host em texto (`rele=` e `ip=`, até 200 letras):
/// quem pareia conecta sem depender de achar o host pelo DNS da n0, e pela rede local ou pela VPN
/// mesmo sem relé. As versões até a 1.6.2 só aceitam o convite sem nada depois do segredo.
fn invite_text(host: &EndpointAddr, secret: &[u8; 16]) -> String {
    let mut b = host.id.as_bytes().to_vec();
    b.extend_from_slice(secret);
    let mut tail = String::new();
    for l in code::addr_lines(host) {
        if tail.len() + l.len() + 1 > 200 {
            break;
        }
        if !tail.is_empty() {
            tail.push('\n');
        }
        tail.push_str(&l);
    }
    b.extend_from_slice(tail.as_bytes());
    format!("{INVITE_PREFIX}{}", data_encoding::BASE32_NOPAD.encode(&b))
}

fn parse_invite(s: &str) -> Option<(EndpointAddr, [u8; 16])> {
    let s: String = s.trim().chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_uppercase();
    let s = s.strip_prefix(INVITE_PREFIX).unwrap_or(&s);
    let b = data_encoding::BASE32_NOPAD.decode(s.as_bytes()).ok()?;
    if b.len() < 48 || b.len() > 48 + 200 {
        return None;
    }
    let id = EndpointId::from_bytes(b[..32].try_into().ok()?).ok()?;
    // O que não lê fica de fora: o id basta para tentar pelo DNS.
    let text = std::str::from_utf8(&b[48..]).unwrap_or("");
    let mut addrs = code::parse_addrs(text.lines());
    // O convite da 1.7.0 tem só o relé, sem `rele=`.
    addrs.extend(text.parse().ok().map(TransportAddr::Relay));
    Some((EndpointAddr::from_parts(id, addrs), b[32..48].try_into().ok()?))
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

/// `host`: usa a porta fixa das opções. O UPnP vale para os dois lados.
async fn endpoint(sk: SecretKey, alpns: Vec<Vec<u8>>, host: bool) -> std::result::Result<Endpoint, String> {
    let o = Opts::load();
    // Um quadro-chave em 4K passa de 600 KB: os buffers de datagramas precisam caber nele.
    let cfg = QuicTransportConfig::builder()
        .datagram_send_buffer_size(8 << 20)
        .datagram_receive_buffer_size(Some(8 << 20))
        .keep_alive_interval(Duration::from_secs(1))
        .build();
    let make = |v6: bool| -> std::result::Result<iroh::endpoint::Builder, String> {
        let mut b = Endpoint::builder(presets::N0).secret_key(sk.clone()).alpns(alpns.clone()).transport_config(cfg.clone()).ca_tls_config(super::code::ca());
        if !o.upnp {
            b = b.portmapper_config(iroh::endpoint::PortmapperConfig::Disabled);
        }
        if host && o.port != 0 {
            b = b.clear_ip_transports().bind_addr(std::net::SocketAddr::from(([0, 0, 0, 0], o.port))).map_err(|e| e.to_string())?;
            if v6 {
                b = b.bind_addr(std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, o.port))).map_err(|e| e.to_string())?;
            }
        }
        Ok(b)
    };
    match make(true)?.bind().await {
        Ok(ep) => Ok(ep),
        // Porta fixa num PC sem IPv6: fica só o IPv4.
        Err(_) if host && o.port != 0 => make(false)?.bind().await.map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
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

    /// Só o caminho escolhido agora conta: o iroh pode trocar do relé para o direto (e voltar).
    fn near(&self) -> bool {
        self.conn.paths().iter().filter(|p| p.is_selected()).any(|p| match p.remote_addr() {
            TransportAddr::Ip(a) => is_local(a.ip()) || vpn_of(a.ip()).is_some(),
            _ => false,
        })
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
    if is_cgnat(ip) && nets.iter().any(|(_, v)| *v == "Tailscale") {
        return Some("Tailscale");
    }
    nets.iter().find(|(p, _)| s.starts_with(p.as_str())).map(|(_, v)| *v)
}

// ---------- Janelas de pergunta ----------

fn message(text: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    let text: Vec<String> = text.split('\n').map(|l| crate::i18n::tr(l).into_owned()).collect();
    unsafe { MessageBoxW(None, &HSTRING::from(text.join("\n")), w!("Prisma: compartilhar tela"), flags | MB_TOPMOST | MB_SETFOREGROUND) }
}

/// Pergunta pelo Prisma da bandeja da sessão, no visual dele: (permitir, confiar). None quando
/// não há Prisma desta sessão e deste usuário para perguntar; aí fica a caixa antiga, e ninguém
/// entra sem pergunta.
fn ask_tray(name: &str, perms: u8, code: Option<&str>) -> Option<(bool, bool)> {
    use prisma_link::{cap, Client, Reply, Request};
    use windows::Win32::Security::{ImpersonateLoggedOnUser, RevertToSelf, TOKEN_QUERY};
    use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSQueryUserToken};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let mut session = 0;
        ProcessIdToSessionId(GetCurrentProcessId(), &mut session).ok()?;
        // O ajudante do serviço é SYSTEM: pergunta ao usuário da sessão, com o token dele. Fora do
        // serviço (página, terminal), o processo já é dela.
        let mut user = HANDLE::default();
        let system = WTSQueryUserToken(session, &mut user).is_ok();
        if !system {
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut user).ok()?;
        }
        let user = Owned::new(user);
        let sid = prisma_link::token_user_sid(*user).ok()?;
        let pipe = prisma_link::share_pipe_name(&sid);
        // O pipe só abre para o usuário dele.
        if system {
            ImpersonateLoggedOnUser(*user).ok()?;
        }
        let client = Client::connect(&pipe, env!("CARGO_PKG_VERSION"));
        if system {
            let _ = RevertToSelf();
        }
        let mut client = client.ok()?;
        // Quem criou o pipe tem de ser esse usuário, nesta sessão: outro poderia ter criado o nome antes.
        let pid = client.server_pid().ok()?;
        let mut there = 0;
        ProcessIdToSessionId(pid, &mut there).ok()?;
        let owner = Owned::new(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut tok = HANDLE::default();
        OpenProcessToken(*owner, TOKEN_QUERY, &mut tok).ok()?;
        let tok = Owned::new(tok);
        if there != session || prisma_link::token_user_sid(*tok).ok()? != sid || !client.capabilities().has(cap::SHARE_ASK) {
            eprintln!("pergunta pela caixa do Windows: o Prisma da bandeja não pôde ser conferido");
            return None;
        }
        client.send(&Request::ShareAsk { name: name.to_string(), perms, code: code.map(str::to_string) }).ok()?;
        // Sem prazo, como a caixa: a pergunta fica até ela escolher. Pipe fechado volta à caixa.
        match client.recv().ok()? {
            Reply::ShareAnswer { allow, trust } => Some((allow, trust)),
            _ => None,
        }
    }
}

/// Pergunta se pareia. Devolve se deve perguntar a cada conexão, ou None se recusou.
fn approve_pair(name: &str, code: &str, perms: u8) -> Option<bool> {
    if let Some((allow, trust)) = ask_tray(name, perms, Some(code)) {
        return allow.then_some(!trust);
    }
    let text = format!(
        "«{name}» quer parear com este PC.\nPermissões: {}.\n\nCódigo de confirmação: {code}\nConfira se o outro PC mostra o mesmo código. Se não mostrar, clique em Cancelar.\n\nSim: parear e perguntar a cada conexão (recomendado).\nNão: parear e confiar neste aparelho, sem perguntar.\nCancelar: recusar.",
        perms_text(perms)
    );
    match message(&text, MB_YESNOCANCEL | MB_ICONQUESTION | MB_DEFBUTTON3) {
        IDYES => Some(true),
        IDNO => Some(false),
        _ => None,
    }
}

/// `--test-share-ask=Nome` pergunta da conexão; `Nome|código`, do pareamento. Mostra a resposta.
#[cfg(debug_assertions)]
pub fn test_ask(v: &str) {
    let perms = TOTAL;
    match v.split_once('|') {
        Some((name, code)) => eprintln!("pareamento: {:?}", approve_pair(name, code, perms)),
        None => eprintln!("conexão: {}", approve_connect(v, perms)),
    }
}

fn approve_connect(name: &str, perms: u8) -> bool {
    if let Some((allow, _)) = ask_tray(name, perms, None) {
        return allow;
    }
    message(&format!("«{name}» quer se conectar a este PC.\nPermissões: {}.\n\nPermitir?", perms_text(perms)), MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2) == IDYES
}

// ---------- Host ----------

async fn pair_host(conn: Connection, me: EndpointId, invite: Arc<Mutex<Invite>>) {
    let who = conn.remote_id();
    let run = async {
        let (mut send, mut recv) = conn.accept_bi().await.ok()?;
        let msg = recv.read_to_end(256).await.ok()?;
        if msg.len() < 16 {
            return None;
        }
        let (secret, name) = msg.split_at(16);
        let name = clean_name(&String::from_utf8_lossy(name));
        let perms = invite.lock().unwrap().perms;
        if !invite.lock().unwrap().check(secret, who) {
            eprintln!("pedido de pareamento de «{name}» recusado: convite errado, usado ou vencido");
            let _ = send.write_all(&[0]).await;
            let _ = send.finish();
            return None;
        }
        let code = sas(&me, &who, secret);
        eprintln!("«{name}» quer parear. Código de confirmação: {code}");
        let n2 = name.clone();
        let answer = tokio::task::spawn_blocking(move || approve_pair(&n2, &code, perms)).await.ok()?;
        let mut reply = vec![answer.is_some() as u8];
        if let Some(ask) = answer {
            invite.lock().unwrap().used = true;
            // O convite do serviço vale uma vez.
            let _ = std::fs::remove_file(invite_path());
            match remember(Device { role: Role::Viewer, id: who, name: name.clone(), perms, ask, macs: Vec::new(), nets: Vec::new() }) {
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

/// O que quem assiste assina: a página pergunta com uma chave de passagem (com a do aparelho, o
/// relé tiraria do ar o host deste mesmo PC), e a chave do aparelho diz que ela é dele.
fn presence_msg(who: &EndpointId) -> Vec<u8> {
    [b"Prisma 2026-11 presenca".as_slice(), who.as_bytes()].concat()
}

/// O aparelho que assinou o pedido, se a assinatura vale para a chave de quem conectou.
fn signed_by(msg: &[u8], remote: EndpointId) -> Option<EndpointId> {
    let id = EndpointId::from_bytes(msg.get(..32)?.try_into().ok()?).ok()?;
    let sig = iroh::Signature::from_bytes(msg.get(32..96)?.try_into().ok()?);
    id.verify(&presence_msg(&remote), &sig).ok()?;
    Some(id)
}

/// Os MACs e os broadcasts deste PC, uma linha cada.
fn presence_info() -> Vec<u8> {
    let (macs, nets) = wake::local();
    let mut s = String::new();
    for m in &macs {
        s += &format!("mac\t{}\n", wake::mac_text(m));
    }
    for n in &nets {
        s += &format!("rede\t{n}\n");
    }
    s.into_bytes()
}

fn parse_info(b: &[u8]) -> (Vec<Mac>, Vec<Ipv4Addr>) {
    let (mut macs, mut nets) = (Vec::new(), Vec::new());
    for l in String::from_utf8_lossy(b).lines() {
        match l.split_once('\t') {
            Some(("mac", v)) => macs.extend(wake::mac_parse(v)),
            Some(("rede", v)) => nets.extend(v.parse::<Ipv4Addr>().ok()),
            _ => {}
        }
    }
    macs.truncate(8);
    nets.truncate(8);
    (macs, nets)
}

/// Outro aparelho pede para este PC ligar os MACs de `req` (6 bytes cada) na rede daqui. Só os
/// que este PC conhece de um PC pareado com ele.
fn relay_wake(list: &[Device], req: &[u8]) -> Vec<u8> {
    static LAST: Mutex<Option<Instant>> = Mutex::new(None);
    let known: Vec<Mac> = req.chunks_exact(6).take(8).filter_map(|m| m.try_into().ok()).filter(|m| list.iter().any(|d| d.role == Role::Host && d.macs.contains(m))).collect();
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if !wake::may_relay(!known.is_empty(), *last, now) {
        return vec![0];
    }
    *last = Some(now);
    let nets = wake::local().1;
    let mut ok = false;
    for m in &known {
        eprintln!("{} pacote para ligar {} mandado a pedido de outro aparelho", now_text(), wake::mac_text(m));
        ok |= wake::send(*m, &nets);
    }
    vec![ok as u8]
}

/// Ligado ou não, para a página de quem assiste: responde e fecha, sem sessão, sem pergunta e sem
/// log (ela pergunta a cada 30 s). Quem não é pareado sai como na conexão.
async fn presence_host(conn: Connection) {
    let run = async {
        let (mut send, mut recv) = conn.accept_bi().await.ok()?;
        let msg = recv.read_to_end(128).await.ok()?;
        let viewer = signed_by(&msg, conn.remote_id())?;
        let list = load();
        if !list.iter().any(|d| d.role == Role::Viewer && d.id == viewer) {
            return None;
        }
        let reply = match &msg[96..] {
            [] => presence_info(),
            [1, m @ ..] if !m.is_empty() && m.len() % 6 == 0 => relay_wake(&list, m),
            _ => return None,
        };
        send.write_all(&reply).await.ok()?;
        send.finish().ok()?;
        let _ = tokio::time::timeout(PRESENCE_WAIT, conn.closed()).await;
        Some(())
    };
    if tokio::time::timeout(PRESENCE_WAIT * 2, run).await.ok().flatten().is_none() {
        conn.close(1u32.into(), b"nao pareado");
    }
}

async fn accept_loop(ep: Endpoint, invite: Arc<Mutex<Invite>>, tx: mpsc::Sender<(Connection, Device)>) {
    let me = ep.id();
    while let Some(incoming) = ep.accept().await {
        let (invite, tx) = (invite.clone(), tx.clone());
        tokio::spawn(async move {
            // Quem não termina o aperto de mão também vai para o log: sem isso, um pareamento que
            // chegou e caiu parecia nunca ter chegado.
            let conn = match incoming.accept() {
                Ok(a) => a.await,
                Err(e) => return eprintln!("{} conexão recusada antes do aperto de mão: {e}", now_text()),
            };
            let conn = match conn {
                Ok(c) => c,
                Err(e) => return eprintln!("{} conexão caiu no aperto de mão: {e}", now_text()),
            };
            let who = conn.remote_id();
            if conn.alpn() == ALPN_PAIR {
                // O convite mais novo vem do arquivo que o --stream-invite grava.
                // Um convite revogado chega com a mesma chave e a validade zerada.
                if let Some(i) = load_invite_file() {
                    let mut cur = invite.lock().unwrap();
                    if cur.secret != i.secret || cur.until != i.until {
                        *cur = i;
                    }
                }
                pair_host(conn, me, invite).await;
            } else if conn.alpn() == ALPN_PRESENCE {
                presence_host(conn).await;
            } else if conn.alpn() == ALPN_STREAM {
                let Some(dev) = load().into_iter().find(|d| d.role == Role::Viewer && d.id == who) else {
                    eprintln!("conexão recusada: {} não está pareado", who.fmt_short());
                    conn.close(1u32.into(), b"nao pareado");
                    return;
                };
                if Opts::load().no_chain && viewing_now() {
                    eprintln!("conexão de «{}» recusada: este PC está assistindo a outro agora", dev.name);
                    conn.close(4u32.into(), b"em cadeia");
                    return;
                }
                if dev.ask {
                    let (name, p) = (dev.name.clone(), dev.perms);
                    if !tokio::task::spawn_blocking(move || approve_connect(&name, p)).await.unwrap_or(false) {
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

pub fn host(perms: u8) {
    if super::service::installed() {
        eprintln!("o serviço do Prisma está instalado e já compartilha este PC (--stream-service-status).");
        return eprintln!("para parear um aparelho novo: --stream-invite=permissões");
    }
    host_with(Some(perms), false);
}

/// Espera o relé responder. O prazo é criado dentro do runtime: fora dele, o tokio derruba o
/// processo ("no reactor running"), que foi o erro da 0.11.0.
fn wait_online(ep: &Endpoint, limit: Duration) -> bool {
    rt().block_on(async { tokio::time::timeout(limit, ep.online()).await.is_ok() })
}

/// O ajudante do serviço: o convite vem de arquivo, e ninguém lê o terminal.
pub fn host_service() {
    host_with(None, true);
}

/// A página: como o serviço, sem convite na partida.
pub fn host_quiet() {
    host_with(None, false);
}

/// `invite`: permissões do convite que aparece no terminal; None sem convite na partida.
fn host_with(invite_perms: Option<u8>, service: bool) {
    eprintln!("{NOTICE}");
    // Uma sessão do ajudante que caiu: só ele apaga as marcas dela (a pasta é só leitura para a conta).
    if service {
        host::restore_hdr();
        super::cursor::restore_orphan();
    }
    host::emergency_key();
    let _ = std::fs::write(host::room_path(), "");
    let sk = match identity() {
        Ok(k) => k,
        Err(e) => return eprintln!("{e}"),
    };
    let ep = match rt().block_on(endpoint(sk, vec![ALPN_STREAM.to_vec(), ALPN_PAIR.to_vec(), ALPN_PRESENCE.to_vec()], true)) {
        Ok(e) => e,
        Err(e) => return eprintln!("não deu para abrir a conexão: {e}"),
    };
    if !wait_online(&ep, Duration::from_secs(10)) {
        eprintln!("sem resposta do relé da n0 em 10 s: só a rede local e a VPN vão funcionar por enquanto");
    }
    rt().spawn(watch_relay(ep.clone()));
    let shown = code::new_code();
    let root = code::stretch(&code::normalize(&shown).unwrap_or_default());
    let secret = if invite_perms.is_some() { code::secret(&root) } else { random::<16>() };
    // Sem convite na partida, ele começa usado: só vale o do arquivo.
    let invite = Arc::new(Mutex::new(Invite { secret, until: unix_now() + INVITE_TTL.as_secs(), tries: Vec::new(), used: invite_perms.is_none(), perms: invite_perms.unwrap_or(SOUND) }));
    match invite_perms {
        Some(perms) => {
            eprintln!("Prisma compartilhando pela internet. Quem parear agora: {}. Este PC: «{}», id {}", perms_text(perms), my_name(), ep.id().fmt_short());
            eprintln!("Convite para parear outro PC (vale 10 minutos e uma vez).");
            eprintln!("{CODE_LINE}{shown}");
            eprintln!("Convite longo, se o código não for achado:\n\n{}\n", invite_text(&ep.addr(), &secret));
            let addr = ep.addr();
            let until = unix_now() + INVITE_TTL.as_secs();
            rt().spawn(async move { published(code::publish(&root, &addr, until).await) });
        }
        None if service => eprintln!("serviço compartilhando pela internet. Este PC: «{}», id {}", my_name(), ep.id().fmt_short()),
        None => eprintln!("compartilhando pela internet. Este PC: «{}», id {}", my_name(), ep.id().fmt_short()),
    }
    let (tx, rx) = mpsc::channel();
    rt().spawn(accept_loop(ep.clone(), invite, tx));

    // Cada conexão espera a tela de quem assiste numa linha própria e entra na sala pronta.
    let (jtx, jrx) = mpsc::channel::<Joiner>();
    std::thread::Builder::new()
        .name("compartilhar-chegadas".into())
        .spawn(move || {
            while let Ok((conn, dev)) = rx.recv() {
                let jtx = jtx.clone();
                std::thread::spawn(move || {
                    eprintln!("«{}» conectou", dev.name);
                    let link = Net::new(conn);
                    let mut buf = vec![0u8; 2048];
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
                    match hello {
                        // Só mouse e teclado: fica fora da sala do vídeo, numa linha própria.
                        Some(hello) if hello.flags & NO_VIDEO != 0 => super::kvm::serve(link, dev.perms, dev.name),
                        // Só o som: também fora da sala do vídeo.
                        Some(hello) if hello.flags & SOUND_ONLY != 0 => super::ear::serve(link, dev.perms, dev.name, hello.pid, hello.flags & OPUS_OK != 0),
                        Some(hello) => {
                            let _ = jtx.send(Joiner { link, hello, perms: dev.perms, name: dev.name, id: hex(&dev.id) });
                        }
                        None => eprintln!("«{}» não mandou a tela dele a tempo", dev.name),
                    }
                });
            }
        })
        .ok();

    let Some(d) = host::prepare() else { return };
    while let Ok(first) = jrx.recv() {
        host::serve(&d, first, Some(&jrx));
    }
}

// ---------- Quem assiste ----------

/// O que este PC sabia quando o outro não respondeu: separa "este PC sem relé" de "o outro PC
/// não foi achado" e de "achado, mas calado" no log de quem pareia ou assiste.
async fn why_silent(ep: &Endpoint, other: EndpointId) -> String {
    use iroh::Watcher;
    let st = ep.home_relay_status().get();
    let mine = match st.iter().find(|r| r.is_connected()) {
        Some(r) => format!("este PC no relé {}", r.url()),
        None => match st.iter().find_map(|r| r.last_error().map(|e| format!("{}: {e}", r.url()))) {
            Some(e) => format!("este PC sem relé ({e})"),
            // Sem relé escolhido: nenhum relé da n0 respondeu ao teste de rede (firewall, antivírus, proxy).
            None => "este PC sem relé: nenhum relé da n0 respondeu".into(),
        },
    };
    let theirs = match ep.remote_info(other).await {
        Some(i) => code::addr_lines(&EndpointAddr::from_parts(other, i.into_addrs().map(|a| a.into_addr()))),
        None => Vec::new(),
    };
    format!("{mine}; endereços do outro: {}", if theirs.is_empty() { "nenhum achado".into() } else { theirs.join(", ") })
}

/// Pede o pareamento e espera a resposta. Devolve o nome do outro PC se ele aceitou.
async fn ask_pair(ep: &Endpoint, addr: EndpointAddr, secret: &[u8; 16]) -> std::result::Result<String, String> {
    let host_id = addr.id;
    let conn = match tokio::time::timeout(Duration::from_secs(30), ep.connect(addr, ALPN_PAIR)).await {
        Ok(c) => c.map_err(|e| e.to_string())?,
        Err(_) => return Err(format!("o outro PC não respondeu em 30 s ({})", why_silent(ep, host_id).await)),
    };
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
    let (host, secret): (EndpointAddr, _) = if let Some(raw) = code::normalize(text) {
        eprintln!("procurando o código {}", code::show(&raw));
        let root = code::stretch(&raw);
        match rt().block_on(code::resolve(&root)) {
            Ok(addr) => {
                eprintln!("achei o PC {} ({})", addr.id.fmt_short(), code::addr_lines(&addr).join(", "));
                (addr, code::secret(&root))
            }
            Err(e) => return eprintln!("não achei o código ({e}): confira o código, ou peça o convite longo"),
        }
    } else if let Some(x) = parse_invite(text) {
        x
    } else {
        return eprintln!("convite inválido");
    };
    let sk = match identity() {
        Ok(k) => k,
        Err(e) => return eprintln!("{e}"),
    };
    let r: std::result::Result<String, String> = rt().block_on(async {
        let ep = endpoint(sk.clone(), Vec::new(), false).await?;
        let host_id = host.id;
        let r = ask_pair(&ep, host, &secret).await.and_then(|name| {
            remember(Device { role: Role::Host, id: host_id, name: name.clone(), perms: 0, ask: false, macs: Vec::new(), nets: Vec::new() }).map(|_| name).map_err(|e| e.to_string())
        });
        // Os MACs para o botão Ligar já vêm no pareamento.
        if r.is_ok() {
            if let Some(Some(info)) = ask_presence(&ep, &sk, host_id, &[]).await {
                set_wol(host_id, &info);
            }
        }
        ep.close().await;
        r
    });
    match r {
        Ok(name) => eprintln!("pareado com «{name}». Para assistir: --stream-connect={name}"),
        Err(e) => eprintln!("não pareou: {e}"),
    }
}

pub fn connect(arg: &str) {
    let mut parts = arg.split('|');
    let target = parts.next().unwrap_or("");
    let out_file = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
    let secs: Option<u64> = parts.next().and_then(|s| s.parse().ok());
    with_host(target, |link, _| view::open(link, out_file.as_deref(), secs));
}

/// Conecta a um PC pareado (que este assiste), faz o trabalho e fecha.
pub(super) fn with_host(target: &str, work: impl FnOnce(Arc<dyn Link>, &str)) {
    eprintln!("{NOTICE}");
    let Some(dev) = find(&load(), Role::Host, target) else {
        return eprintln!("nenhum PC pareado com o nome «{target}» (veja --stream-devices)");
    };
    let sk = match identity() {
        Ok(k) => k,
        Err(e) => return eprintln!("{e}"),
    };
    let ep = match rt().block_on(endpoint(sk.clone(), Vec::new(), false)) {
        Ok(e) => e,
        Err(e) => return eprintln!("não deu para abrir a conexão: {e}"),
    };
    eprintln!("conectando a «{}»", dev.name);
    let conn = rt().block_on(async { tokio::time::timeout(Duration::from_secs(30), ep.connect(dev.id, ALPN_STREAM)).await });
    match conn {
        Ok(Ok(conn)) => {
            // A cada conexão, os MACs e a rede de agora (um host antigo não responde, e tudo bem).
            let (ep2, id) = (ep.clone(), dev.id);
            rt().spawn(async move {
                if let Some(Some(info)) = ask_presence(&ep2, &sk, id, &[]).await {
                    set_wol(id, &info);
                }
            });
            let link = Net::new(conn);
            eprintln!("conectado: {}", link.route());
            work(link, &dev.name);
        }
        Ok(Err(e)) => eprintln!("não conectou: {e}"),
        Err(_) => eprintln!("«{}» não respondeu em 30 s (está com o Prisma compartilhando?; {})", dev.name, rt().block_on(why_silent(&ep, dev.id))),
    }
    rt().block_on(ep.close());
}

// ---------- Ligado ou desligado (a página de quem assiste) ----------

/// O host respondeu, mesmo recusando o protocolo (a 1.8.0 não conhece o de presença).
fn answered(e: &iroh::endpoint::ConnectError) -> bool {
    use iroh::endpoint::{ConnectError as E, ConnectingError, ConnectionError as C};
    let c = match e {
        E::Connection { source, .. } => source,
        E::Connecting { source: ConnectingError::ConnectionError { source, .. }, .. } => source,
        _ => return false,
    };
    matches!(c, C::ConnectionClosed(_) | C::ApplicationClosed(_) | C::TransportError(_))
}

/// Pergunta ao PC `id` se está ligado, ou pede a ele para ligar `wake` na rede dele. None: não
/// respondeu. Some(None): está ligado, mas não respondeu ao pedido (versão antiga).
async fn ask_presence(ep: &Endpoint, me: &SecretKey, id: EndpointId, wake: &[Mac]) -> Option<Option<Vec<u8>>> {
    let run = async {
        let conn = match ep.connect(id, ALPN_PRESENCE).await {
            Ok(c) => c,
            Err(e) => return answered(&e).then_some(None),
        };
        let talk = async {
            let (mut send, mut recv) = conn.open_bi().await.ok()?;
            let mut msg = me.public().as_bytes().to_vec();
            msg.extend_from_slice(&me.sign(&presence_msg(&ep.id())).to_bytes());
            if !wake.is_empty() {
                msg.push(1);
                msg.extend(wake.iter().take(8).flatten());
            }
            send.write_all(&msg).await.ok()?;
            send.finish().ok()?;
            recv.read_to_end(4096).await.ok()
        };
        let r = talk.await;
        conn.close(0u32.into(), b"ok");
        Some(r)
    };
    tokio::time::timeout(PRESENCE_WAIT, run).await.ok().flatten()
}

/// Guarda os MACs e as redes que o PC `id` mandou. Sem MAC (adaptador desligado agora), fica o
/// que já estava.
fn set_wol(id: EndpointId, info: &[u8]) {
    let (macs, nets) = parse_info(info);
    if macs.is_empty() {
        return;
    }
    let _lock = devices_lock(Role::Host);
    let mut list = load();
    let mut changed = false;
    for d in list.iter_mut().filter(|d| d.role == Role::Host && d.id == id && (d.macs != macs || d.nets != nets)) {
        (d.macs, d.nets, changed) = (macs.clone(), nets.clone(), true);
    }
    if changed {
        let _ = save(&list, Role::Host);
    }
}

/// O que a página mostra ao lado de cada PC que este assiste.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Presence {
    Checking,
    Online,
    Offline,
}

/// Texto da página em português, com `{}` trocado pelo nome.
pub type Note = (&'static str, String);

struct Watch {
    /// A última vez que a página estava aberta e a janela visível.
    lease: Option<Instant>,
    running: bool,
    /// A última vez que a conexão não abriu: espera uma rodada para tentar de novo.
    failed: Option<Instant>,
    seen: Vec<(EndpointId, Presence)>,
    wake: Vec<String>,
    note: Option<Note>,
}

static WATCH: Mutex<Watch> = Mutex::new(Watch { lease: None, running: false, failed: None, seen: Vec::new(), wake: Vec::new(), note: None });

fn watch() -> std::sync::MutexGuard<'static, Watch> {
    WATCH.lock().unwrap_or_else(|e| e.into_inner())
}

/// A página está aberta e a janela visível: a verificação continua (ou começa). Sem chamada por
/// 2 s, ela para e fecha a conexão e a linha dela.
pub fn presence_keep() {
    let mut w = watch();
    w.lease = Some(Instant::now());
    if w.running || w.failed.is_some_and(|t| t.elapsed() < PRESENCE_ROUND) {
        return;
    }
    w.running = true;
    drop(w);
    if std::thread::Builder::new().name("compartilhar-presenca".into()).spawn(presence_worker).is_err() {
        watch().running = false;
    }
}

pub fn presence_of(id: &str) -> Presence {
    watch().seen.iter().find(|(d, _)| hex(d) == id).map_or(Presence::Checking, |s| s.1)
}

/// Ligar o PC `id` (vai na próxima volta da verificação).
pub fn wake_device(id: &str) {
    watch().wake.push(id.to_string());
}

/// O resultado do último Ligar.
pub fn wake_note() -> Option<Note> {
    watch().note.take()
}

fn presence_worker() {
    let ok = (|| {
        let me = identity().ok()?;
        // Runtime próprio: some junto com a linha quando a página fecha.
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
        rt.block_on(async {
            let ep = endpoint(SecretKey::generate(), Vec::new(), false).await.ok()?;
            let _ = tokio::time::timeout(PRESENCE_WAIT, ep.online()).await;
            let mut next = Instant::now();
            loop {
                let wake = {
                    let mut w = watch();
                    if w.lease.is_none_or(|t| t.elapsed() > Duration::from_secs(2)) {
                        break;
                    }
                    std::mem::take(&mut w.wake)
                };
                for id in wake {
                    let note = wake_now(&ep, &me, &id).await;
                    watch().note = note;
                }
                if Instant::now() >= next {
                    presence_round(&ep, &me).await;
                    next = Instant::now() + PRESENCE_ROUND;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            ep.close().await;
            Some(())
        })
    })()
    .is_some();
    let mut w = watch();
    w.running = false;
    w.wake.clear();
    if ok {
        w.seen.clear();
    } else {
        // Sem conexão neste PC: nenhum outro é alcançável.
        w.failed = Some(Instant::now());
        w.seen = load().into_iter().filter(|d| d.role == Role::Host).map(|d| (d.id, Presence::Offline)).collect();
    }
}

/// Todos os PCs que este assiste ao mesmo tempo, cada um com até 5 s.
async fn presence_round(ep: &Endpoint, me: &SecretKey) {
    let mut set = tokio::task::JoinSet::new();
    for d in load().into_iter().filter(|d| d.role == Role::Host) {
        let (ep, me) = (ep.clone(), me.clone());
        set.spawn(async move { (d.id, ask_presence(&ep, &me, d.id, &[]).await) });
    }
    while let Some(r) = set.join_next().await {
        let Ok((id, r)) = r else { continue };
        if let Some(Some(info)) = &r {
            set_wol(id, info);
        }
        let p = if r.is_some() { Presence::Online } else { Presence::Offline };
        let mut w = watch();
        w.seen.retain(|s| s.0 != id);
        w.seen.push((id, p));
    }
}

/// Manda o pacote mágico daqui e, se este PC não está na rede do outro, pede a um PC ligado
/// daquela rede para mandar lá.
async fn wake_now(ep: &Endpoint, me: &SecretKey, id: &str) -> Option<Note> {
    let list = load();
    let t = list.iter().find(|d| d.role == Role::Host && hex(&d.id) == id && !d.macs.is_empty())?;
    for m in &t.macs {
        wake::send(*m, &t.nets);
    }
    if t.nets.iter().any(|n| wake::local().1.contains(n)) {
        return Some(("Pedido para ligar «{}» mandado na rede deste PC.", t.name.clone()));
    }
    let online: Vec<EndpointId> = watch().seen.iter().filter(|s| s.1 == Presence::Online).map(|s| s.0).collect();
    let mut refused = None;
    for r in list.iter().filter(|d| d.role == Role::Host && d.id != t.id && online.contains(&d.id) && d.nets.iter().any(|n| t.nets.contains(n))) {
        match ask_presence(ep, me, r.id, &t.macs).await {
            Some(Some(b)) if b == [1] => {
                return Some(("Pedido para ligar mandado por «{}», na rede de casa.", r.name.clone()))
            }
            _ => refused = Some(r.name.clone()),
        }
    }
    Some(match refused {
        Some(n) => ("«{}» não ligou o PC: ele também precisa assistir a esse PC.", n),
        None => ("Para ligar de outra rede, deixe outro PC com o Prisma ligado na mesma rede de casa.", String::new()),
    })
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
            Role::Viewer => format!("pode assistir a este PC ({}; {})", perms_text(d.perms), if d.ask { "pergunta a cada conexão" } else { "confiável" }),
            Role::Host => "este PC assiste a ele".into(),
        };
        eprintln!("«{}» {}: {role}", d.name, &hex(&d.id)[..10]);
    }
}

/// `nome|permissões`: muda o que um aparelho que assiste a este PC pode fazer.
pub fn allow(arg: &str) {
    let (key, value) = arg.split_once('|').unwrap_or((arg, ""));
    let Some(perms) = parse_perms(value) else {
        return eprintln!("permissão desconhecida: «{value}» (use total, jogar, teclado-mouse, ver, video, ou uma lista de som, controle, teclado, mouse)");
    };
    let _lock = devices_lock(Role::Viewer);
    let mut list = load();
    let Some(dev) = find(&list, Role::Viewer, key) else {
        return eprintln!("nenhum aparelho que assiste com o nome «{key}» (veja --stream-devices)");
    };
    for d in list.iter_mut().filter(|d| d.role == Role::Viewer && d.id == dev.id) {
        d.perms = perms;
    }
    match save(&list, Role::Viewer) {
        Ok(()) => eprintln!("«{}» agora: {} (vale na próxima conexão)", dev.name, perms_text(perms)),
        Err(e) => eprintln!("não deu para salvar: {e}"),
    }
}

/// Um aparelho pareado, como a página Compartilhar tela mostra.
#[derive(Clone, PartialEq)]
pub struct DeviceRow {
    /// Assiste a este PC (senão, é um PC que este assiste).
    pub viewer: bool,
    pub id: String,
    pub name: String,
    pub perms: u8,
    pub ask: bool,
    /// Tem MAC guardado: dá para tentar ligar.
    pub wake: bool,
}

pub fn device_rows() -> Vec<DeviceRow> {
    load().into_iter().map(|d| DeviceRow { viewer: d.role == Role::Viewer, id: hex(&d.id), wake: !d.macs.is_empty(), name: d.name, perms: d.perms, ask: d.ask }).collect()
}

/// Muda permissões e confiança de um aparelho que assiste a este PC.
pub fn set_device(id: &str, perms: u8, ask: bool) -> io::Result<()> {
    let _lock = devices_lock(Role::Viewer);
    let mut list = load();
    for d in list.iter_mut().filter(|d| d.role == Role::Viewer && hex(&d.id) == id) {
        d.perms = perms;
        d.ask = ask;
    }
    save(&list, Role::Viewer)
}

pub fn forget_device(viewer: bool, id: &str) -> io::Result<()> {
    let role = if viewer { Role::Viewer } else { Role::Host };
    let _lock = devices_lock(role);
    let mut list = load();
    list.retain(|d| !(d.role == role && hex(&d.id) == id));
    save(&list, role)
}

/// Nome e id curto deste PC, para a página.
pub fn me() -> Option<(String, String)> {
    let k = identity().ok()?;
    Some((my_name(), k.public().fmt_short().to_string()))
}

pub fn forget(key: &str) {
    let mut found = false;
    // Um papel de cada vez: com o serviço, cada um tem o seu arquivo (e a sua trava).
    for role in [Role::Viewer, Role::Host] {
        let _lock = devices_lock(role);
        let mut list = load();
        let Some(dev) = find(&list, role, key) else { continue };
        found = true;
        list.retain(|d| !(d.role == role && d.id == dev.id));
        match save(&list, role) {
            Ok(()) => eprintln!("esquecido: «{}»", dev.name),
            Err(e) => eprintln!("não deu para salvar: {e}"),
        }
    }
    if !found {
        eprintln!("nenhum aparelho com o nome «{key}»");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_round_trip() {
        let id = SecretKey::from_bytes(&[7; 32]).public();
        let secret = [9u8; 16];
        // Sem relé: o mesmo texto das versões antigas.
        let t = invite_text(&id.into(), &secret);
        assert!(t.starts_with(INVITE_PREFIX));
        let (a2, s2) = parse_invite(&format!("  {}  ", t.to_lowercase())).unwrap();
        assert_eq!(a2, EndpointAddr::from(id));
        assert_eq!(s2, secret);
        // Com o relé e os IPs, eles voltam inteiros.
        let relay: iroh::RelayUrl = "https://use1-1.relay.n0.iroh.link./".parse().unwrap();
        let ip = |s: &str| TransportAddr::Ip(s.parse().unwrap());
        let full = EndpointAddr::from_parts(id, [TransportAddr::Relay(relay.clone()), ip("192.168.1.176:51458"), ip("172.24.66.19:51458"), ip("[2804:880:1680:b300:718f:682d:67c4:ee7a]:51460")]);
        let (a3, s3) = parse_invite(&invite_text(&full, &secret)).unwrap();
        assert_eq!(a3, full);
        assert_eq!(s3, secret);
        // Endereços demais: fica o que cabe em 200 letras, o relé primeiro.
        let many = EndpointAddr::from_parts(id, std::iter::once(TransportAddr::Relay(relay.clone())).chain((1..30).map(|i| ip(&format!("10.0.0.{i}:51458")))));
        let (a5, _) = parse_invite(&invite_text(&many, &secret)).unwrap();
        assert!(a5.relay_urls().next().is_some() && a5.addrs.len() > 3 && a5.addrs.len() < 12);
        // O convite da 1.7.0, com o relé sem `rele=`.
        let mut b = id.as_bytes().to_vec();
        b.extend_from_slice(&secret);
        b.extend_from_slice(relay.to_string().as_bytes());
        let (a6, _) = parse_invite(&format!("{INVITE_PREFIX}{}", data_encoding::BASE32_NOPAD.encode(&b))).unwrap();
        assert_eq!(a6, EndpointAddr::from_parts(id, [TransportAddr::Relay(relay)]));
        // Um final que não é relé fica de fora, sem perder o convite.
        let mut b = id.as_bytes().to_vec();
        b.extend_from_slice(&secret);
        b.extend_from_slice(b"lixo");
        let (a4, _) = parse_invite(&format!("{INVITE_PREFIX}{}", data_encoding::BASE32_NOPAD.encode(&b))).unwrap();
        assert_eq!(a4, EndpointAddr::from(id));
        assert!(parse_invite("PRISMA-ABC").is_none());
    }

    #[test]
    fn lists_split_by_role() {
        let dev = |role, n: u8| Device { role, id: SecretKey::from_bytes(&[n; 32]).public(), name: n.to_string(), perms: SOUND, ask: true, macs: Vec::new(), nets: Vec::new() };
        // Arquivo do serviço de uma versão antiga: quem assiste (1) e um PC que este assiste (2).
        let shared = vec![dev(Role::Viewer, 1), dev(Role::Host, 2)];
        // Arquivo do usuário com uma cópia velha de quem assiste (3) e um PC que este assiste (4).
        let mine = vec![dev(Role::Viewer, 3), dev(Role::Host, 4)];
        let (list, extra) = join_lists(shared.clone(), mine.clone(), true);
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["1", "4", "2"]);
        assert_eq!(extra.len(), 1);
        // Depois de passar uma vez, o PC esquecido no arquivo do usuário não volta do serviço.
        let (list, extra) = join_lists(shared, mine, false);
        assert_eq!(list.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["1", "4"]);
        assert!(extra.is_empty());
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
        let (a, b) = (SecretKey::from_bytes(&[1; 32]).public(), SecretKey::from_bytes(&[2; 32]).public());
        let new = |until| Invite { secret: [5; 16], until, tries: Vec::new(), used: false, perms: SOUND };
        let mut inv = new(unix_now() + 600);
        for _ in 0..4 {
            assert!(!inv.check(&[6; 16], a));
        }
        assert!(inv.check(&[5; 16], a));
        // Quinta tentativa de a gasta: nem o certo passa mais para ele.
        assert!(!inv.check(&[5; 16], a));
        // Os erros de a não gastam as tentativas de b.
        assert!(inv.check(&[5; 16], b));
        // Teto geral: 50 tentativas de aparelhos diferentes.
        let mut many = new(unix_now() + 600);
        for i in 0..50u8 {
            assert!(!many.check(&[6; 16], SecretKey::from_bytes(&[i + 10; 32]).public()));
        }
        assert!(!many.check(&[5; 16], b));
        assert!(!new(unix_now() - 1).check(&[5; 16], a));
        // Revogado: validade zerada.
        assert!(!new(0).check(&[5; 16], a));
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
    /// O começo do host, fora de qualquer runtime, como no processo de verdade: sem relé, só espera
    /// o prazo e segue.
    #[test]
    fn host_start_outside_runtime() {
        let ep = rt().block_on(Endpoint::builder(presets::Minimal).relay_mode(iroh::RelayMode::Disabled).bind()).unwrap();
        let t = Instant::now();
        let _ = wait_online(&ep, Duration::from_millis(300));
        assert!(t.elapsed() < Duration::from_secs(5));
        rt().block_on(ep.close());
    }

    #[test]
    fn wrong_invite_is_refused() {
        let r = rt().block_on(async {
            let a = Endpoint::builder(presets::Minimal).relay_mode(iroh::RelayMode::Disabled).alpns(vec![ALPN_PAIR.to_vec()]).bind().await.unwrap();
            let b = Endpoint::builder(presets::Minimal).relay_mode(iroh::RelayMode::Disabled).bind().await.unwrap();
            let invite = Arc::new(Mutex::new(Invite { secret: [1; 16], until: unix_now() + 600, tries: Vec::new(), used: false, perms: SOUND }));
            let (a2, inv2) = (a.clone(), invite.clone());
            let host = tokio::spawn(async move {
                let conn = a2.accept().await.unwrap().accept().unwrap().await.unwrap();
                pair_host(conn, a2.id(), inv2).await;
            });
            let r = ask_pair(&b, a.addr(), &[2; 16]).await;
            let _ = host.await;
            let inv = invite.lock().unwrap();
            assert_eq!((inv.tries.len(), inv.used), (1, false));
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

    /// O pedido de presença ou de ligar só vale assinado pelo aparelho para a chave que conectou.
    #[test]
    fn presence_request_is_signed() {
        let me = SecretKey::from_bytes(&[7; 32]);
        let pass = SecretKey::from_bytes(&[8; 32]).public();
        let mut msg = me.public().as_bytes().to_vec();
        msg.extend_from_slice(&me.sign(&presence_msg(&pass)).to_bytes());
        assert_eq!(signed_by(&msg, pass), Some(me.public()));
        assert_eq!(signed_by(&msg, SecretKey::from_bytes(&[9; 32]).public()), None);
        assert_eq!(signed_by(&msg[..90], pass), None);
        let (macs, nets) = parse_info(b"mac\t001a2b3c4d5e\nrede\t192.168.1.255\nlixo\n");
        assert_eq!(macs, vec![[0x00, 0x1a, 0x2b, 0x3c, 0x4d, 0x5e]]);
        assert_eq!(nets, vec![Ipv4Addr::new(192, 168, 1, 255)]);
    }
}
