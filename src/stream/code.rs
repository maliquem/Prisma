//! Convite curto: oito letras e números, como `K7M2-QX9P`, no lugar dos 77 caracteres do convite
//! longo. O código nunca sai deste PC pela rede; dele saem duas coisas:
//!
//! - uma chave ed25519, que assina um registro curto com o id do host. O registro vai para os relés
//!   pkarr (o da n0, que o iroh já usa, e os dois públicos do pkarr), e quem tem o código acha o
//!   registro pela mesma chave;
//! - o segredo do pareamento, que o host confere como o do convite longo.
//!
//! São 40 bits: adivinhar pelos relés levaria séculos, e o host recusa o convite depois de 5
//! tentativas erradas, de 10 minutos ou do primeiro pareamento. O código de confirmação de seis
//! dígitos continua valendo contra quem se meter no meio.

use std::time::Duration;

use iroh::address_lookup::pkarr::PkarrRelayClient;
use iroh::{EndpointId, SecretKey};
use iroh_dns::pkarr::SignedPacket;

/// Base32 de Crockford: sem I, L, O e U, que se confundem ao ditar ou copiar à mão.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const LEN: usize = 8;
const RELAYS: [&str; 3] = ["https://dns.iroh.link/pkarr", "https://relay.pkarr.org", "https://pkarr.pubky.org"];
const NAME: &str = "_prisma";
const WAIT: Duration = Duration::from_secs(8);

/// Um código novo, já no formato de mostrar.
pub fn new_code() -> String {
    let r = super::net::random::<LEN>();
    let raw: String = r.iter().map(|b| ALPHABET[(b & 31) as usize] as char).collect();
    show(&raw)
}

/// `K7M2QX9P` → `K7M2-QX9P`.
pub fn show(raw: &str) -> String {
    format!("{}-{}", &raw[..4], &raw[4..])
}

/// Aceita o código como a pessoa digitar: com ou sem traço, espaços, minúsculas, O no lugar de 0
/// e I ou L no lugar de 1. Também tira o endereço do link (`prisma://parear/...` e a página).
pub fn normalize(s: &str) -> Option<String> {
    let s = s.trim();
    let s = s.rsplit(['/', '#', '=']).next().unwrap_or(s);
    let mut out = String::new();
    for c in s.chars() {
        let c = match c.to_ascii_uppercase() {
            '-' | ' ' | '\t' => continue,
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        };
        if !c.is_ascii() || !ALPHABET.contains(&(c as u8)) {
            return None;
        }
        out.push(c);
    }
    (out.len() == LEN).then_some(out)
}

fn derive(context: &str, raw: &str) -> [u8; 32] {
    blake3::derive_key(context, raw.as_bytes())
}

fn key(raw: &str) -> SecretKey {
    SecretKey::from_bytes(&derive("Prisma 2026-11 convite curto, chave do registro", raw))
}

/// O segredo que o host confere no pareamento.
pub fn secret(raw: &str) -> [u8; 16] {
    derive("Prisma 2026-11 convite curto, segredo", raw)[..16].try_into().unwrap()
}

fn client(url: &str) -> std::result::Result<PkarrRelayClient, String> {
    let tls = iroh::tls::CaTlsConfig::embedded().client_config(iroh::tls::default_provider()).map_err(|e| e.to_string())?;
    let url = url.parse().map_err(|e: url::ParseError| e.to_string())?;
    Ok(PkarrRelayClient::new(url, tls, iroh::dns::DnsResolver::new()))
}

/// Publica o id do host nos relés, ao mesmo tempo. Devolve em quantos deu certo.
pub async fn publish(raw: &str, host: &EndpointId, until: u64) -> usize {
    let values = [format!("id={}", data_encoding::HEXLOWER.encode(host.as_bytes())), format!("ate={until}")];
    let packet = match SignedPacket::from_txt_strings(&key(raw), NAME, values, 600) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("não deu para montar o registro do código: {e}");
            return 0;
        }
    };
    let jobs: Vec<_> = RELAYS
        .iter()
        .map(|url| {
            let packet = packet.clone();
            tokio::spawn(async move {
                let c = client(url)?;
                match tokio::time::timeout(WAIT, c.publish(&packet)).await {
                    Ok(Ok(())) => Ok(()),
                    Ok(Err(e)) => Err(format!("{url}: {e}")),
                    Err(_) => Err(format!("{url}: sem resposta")),
                }
            })
        })
        .collect();
    let mut ok = 0;
    for j in jobs {
        match j.await {
            Ok(Ok(())) => ok += 1,
            Ok(Err(e)) => eprintln!("relé recusou o código: {e}"),
            Err(_) => {}
        }
    }
    ok
}

/// Acha o id do host pelo código. Pergunta aos relés ao mesmo tempo e fica com a primeira resposta.
pub async fn resolve(raw: &str) -> std::result::Result<EndpointId, String> {
    let public = key(raw).public();
    let (tx, mut rx) = tokio::sync::mpsc::channel(RELAYS.len());
    for url in RELAYS {
        let tx = tx.clone();
        tokio::spawn(async move {
            let r = async {
                let c = client(url)?;
                let p = tokio::time::timeout(WAIT, c.resolve(public)).await.map_err(|_| "sem resposta".to_string())?.map_err(|e| e.to_string())?;
                read(&p.txt_records(NAME))
            }
            .await;
            let _ = tx.send(r).await;
        });
    }
    drop(tx);
    let mut last = "nenhum relé respondeu".to_string();
    while let Some(r) = rx.recv().await {
        match r {
            Ok(id) => return Ok(id),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn read(values: &[String]) -> std::result::Result<EndpointId, String> {
    let get = |k: &str| values.iter().find_map(|v| v.strip_prefix(k));
    if let Some(until) = get("ate=").and_then(|s| s.parse::<u64>().ok()) {
        if until < super::net::unix_now() {
            return Err("o convite venceu".into());
        }
    }
    let hex = get("id=").ok_or("registro sem id")?;
    let b: [u8; 32] = data_encoding::HEXLOWER_PERMISSIVE.decode(hex.as_bytes()).ok().and_then(|b| b.try_into().ok()).ok_or("id inválido")?;
    EndpointId::from_bytes(&b).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for _ in 0..50 {
            let c = new_code();
            assert_eq!(c.len(), 9);
            let raw = normalize(&c).unwrap();
            assert_eq!(show(&raw), c);
        }
        assert_eq!(normalize(" k7m2-qx9p ").as_deref(), Some("K7M2QX9P"));
        assert_eq!(normalize("K7M2 QX9P").as_deref(), Some("K7M2QX9P"));
        assert_eq!(normalize("Ol23-4567").as_deref(), Some("01234567"));
        assert_eq!(normalize("prisma://parear/K7M2-QX9P").as_deref(), Some("K7M2QX9P"));
        assert_eq!(normalize("https://maliquem.github.io/Prisma/c/#K7M2-QX9P").as_deref(), Some("K7M2QX9P"));
        assert_eq!(normalize(&super::super::weblink::web_link("K7M2-QX9P")).as_deref(), Some("K7M2QX9P"));
        assert!(normalize("K7M2-QX9").is_none());
        assert!(normalize("K7M2-QX9U").is_none());
        assert!(normalize("PRISMA-ABCDEFGH").is_none());
    }

    #[test]
    fn code_derives_everything() {
        let a = normalize("K7M2-QX9P").unwrap();
        let b = normalize("K7M2-QX9Q").unwrap();
        assert_ne!(secret(&a), secret(&b));
        assert_ne!(key(&a).public(), key(&b).public());
        assert_eq!(key(&a).public(), key("K7M2QX9P").public());
        // O segredo não é a chave.
        assert_ne!(&secret(&a)[..], &key(&a).to_bytes()[..16]);
    }

    /// Pela internet, nos relés de verdade: `cargo test live_relays -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_relays() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let raw = normalize(&new_code()).unwrap();
        let host = SecretKey::generate().public();
        let n = rt.block_on(publish(&raw, &host, super::super::net::unix_now() + 120));
        eprintln!("publicado em {n} de 3");
        for url in RELAYS {
            let r = rt.block_on(async { client(url).unwrap().resolve(key(&raw).public()).await.map(|p| read(&p.txt_records(NAME))) });
            eprintln!("{url}: {:?}", r.map_err(|e| e.to_string()));
        }
        assert_eq!(rt.block_on(resolve(&raw)).unwrap(), host);
    }

    #[test]
    fn record_round_trip() {
        let host = SecretKey::from_bytes(&[7; 32]).public();
        let values = [format!("id={}", data_encoding::HEXLOWER.encode(host.as_bytes())), format!("ate={}", super::super::net::unix_now() + 600)];
        let p = SignedPacket::from_txt_strings(&key("K7M2QX9P"), NAME, values, 600).unwrap();
        let back = SignedPacket::from_relay_payload(&key("K7M2QX9P").public(), &p.to_relay_payload()).unwrap();
        assert_eq!(read(&back.txt_records(NAME)).unwrap(), host);
        assert_eq!(read(&["id=00".into()]).is_err(), true);
        assert_eq!(read(&[format!("id={}", data_encoding::HEXLOWER.encode(host.as_bytes())), "ate=5".into()]).unwrap_err(), "o convite venceu");
    }
}
