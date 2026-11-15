//! Compatibilidade com mods feitos para o DSX: o mod do jogo manda instruções em JSON por UDP para
//! a porta 6969, e o Prisma põe a luz e os gatilhos pedidos no DualSense. Desligada de fábrica; só
//! escuta no próprio PC (127.0.0.1). Formato em `research_notes/dsx-udp-6969.md`.
//! O que o mod pede fica até ele mandar voltar às opções do usuário, até a opção ser desligada ou
//! até o Prisma fechar; a parada de emergência passa na frente.

use std::net::UdpSocket;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use super::remap::Trig;

pub const PORT: u16 = 6969;

/// O que os mods pediram: a cor da luz e os gatilhos (L2, R2). `None` é o do perfil.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Over {
    pub light: Option<[u8; 3]>,
    pub trig: [Option<Trig>; 2],
}

static OVER: Mutex<Over> = Mutex::new(Over { light: None, trig: [None, None] });
static ON: AtomicBool = AtomicBool::new(false);
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Luz pedida por um mod, se houver.
pub fn light() -> Option<[u8; 3]> {
    if ON.load(Ordering::Relaxed) { OVER.lock().unwrap().light } else { None }
}

/// Gatilhos pedidos por um mod (cada um `None` quando o mod não mexeu nele).
pub fn triggers() -> [Option<Trig>; 2] {
    if ON.load(Ordering::Relaxed) { OVER.lock().unwrap().trig } else { [None; 2] }
}

/// Liga ou desliga a escuta. Desligar solta o que os mods tinham pedido.
pub fn set_enabled(on: bool) {
    ON.store(on, Ordering::Relaxed);
    if !on {
        *OVER.lock().unwrap() = Over::default();
        return;
    }
    if !RUNNING.swap(true, Ordering::AcqRel) && std::thread::Builder::new().name("mods do DSX".into()).spawn(serve).is_err() {
        RUNNING.store(false, Ordering::Release);
    }
}

fn serve() {
    // Só o próprio PC: um mod roda junto do jogo, e ninguém da rede manda no controle.
    let Ok(sock) = UdpSocket::bind(("127.0.0.1", PORT)) else {
        RUNNING.store(false, Ordering::Release);
        return;
    };
    let _ = sock.set_read_timeout(Some(Duration::from_millis(500)));
    let mut buf = [0u8; 8192];
    while ON.load(Ordering::Relaxed) {
        let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
        let Ok(text) = std::str::from_utf8(&buf[..n]) else { continue };
        let Some(v) = Json::parse(text) else { continue };
        {
            let mut o = OVER.lock().unwrap();
            apply(&mut o, &v);
        }
        let _ = sock.send_to(reply().as_bytes(), from);
    }
    RUNNING.store(false, Ordering::Release);
}

/// Resposta no formato do DSX: alguns mods esperam saber que a mensagem chegou.
fn reply() -> String {
    let pads = super::pads::list();
    let ds = pads.iter().find(|p| p.family == super::pads::Family::DualSense);
    let battery = match ds.map(|p| p.battery) {
        Some(super::pads::Battery::Percent { pct, .. }) => pct as u32,
        _ => 0,
    };
    format!("{{\"Status\":\"DSX Received UDP Instructions\",\"TimeReceived\":\"{}\",\"isControllerConnected\":{},\"BatteryLevel\":{battery}}}", now_text(), ds.is_some())
}

fn now_text() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    format!("{s}")
}

/// Aplica as instruções de uma mensagem.
fn apply(o: &mut Over, v: &Json) {
    let Some(list) = v.get("instructions").and_then(Json::array) else { return };
    for ins in list {
        let Some(kind) = ins.get("type").and_then(|t| t.code(INSTRUCTIONS)) else { continue };
        // O modo do gatilho (terceiro número) também pode vir pelo nome.
        let p: Vec<f64> = ins.get("parameters").and_then(Json::array).map(|a| a.iter().map(|x| x.code(MODES).unwrap_or(0.0)).collect()).unwrap_or_default();
        let at = |i: usize| p.get(i).copied().unwrap_or(0.0);
        match kind as i64 {
            // TriggerUpdate: controle, gatilho (1 esquerdo, 2 direito), modo e os números do modo.
            1 => {
                let side = match at(1) as i64 {
                    1 => 0,
                    2 => 1,
                    _ => continue,
                };
                o.trig[side] = trigger(at(2) as i64, &p[3.min(p.len())..]);
            }
            // RGBUpdate: controle, vermelho, verde, azul.
            2 => o.light = Some([at(1), at(2), at(3)].map(|c| c.clamp(0.0, 255.0) as u8)),
            // ResetToUserSettings: volta ao perfil.
            7 => *o = Over::default(),
            _ => {}
        }
    }
}

/// Nomes das instruções e dos modos do gatilho, na ordem dos números do DSX.
const INSTRUCTIONS: &[&str] = &["GetDSXStatus", "TriggerUpdate", "RGBUpdate", "PlayerLED", "TriggerThreshold", "MicLED", "PlayerLEDNewRevision", "ResetToUserSettings"];
const MODES: &[&str] = &["Normal", "GameCube", "VerySoft", "Soft", "Hard", "VeryHard", "Hardest", "Rigid", "VibrateTrigger", "Choppy", "Medium", "VibrateTriggerPulse", "CustomTriggerValue", "Resistance", "Bow", "Galloping", "SemiAutomaticGun", "AutomaticGun", "Machine", "VIBRATE_TRIGGER_10Hz", "OFF", "FEEDBACK", "WEAPON", "VIBRATION"];

/// Um número de 0 a 255 do mod numa das dez zonas do curso (0 a 9).
fn zone(v: f64) -> u8 {
    (v.clamp(0.0, 255.0) / 255.0 * 9.0).round() as u8
}

/// Um número de 0 a 255 do mod numa força de 1 a 8.
fn power(v: f64) -> u8 {
    ((v.clamp(0.0, 255.0) / 255.0 * 7.0).round() as u8 + 1).min(8)
}

/// O modo do DSX nos efeitos que o Prisma já sabe pôr no gatilho. Os modos de força fixa viram
/// Resistência a partir do começo; os de arma, Arma; os de vibrar, Vibração.
/// shortcut: modos sem par exato (Bow, Galloping, Machine, CustomTriggerValue) ficam no mais
/// próximo; trocar por blocos próprios se alguém pedir o efeito igual.
fn trigger(mode: i64, p: &[f64]) -> Option<Trig> {
    let at = |i: usize| p.get(i).copied().unwrap_or(0.0);
    Some(match mode {
        0 => Trig::Off,
        // GameCube: resiste no fim do curso, como o gatilho de dois estágios.
        1 => Trig::Click { start: 8, power: 8 },
        2 => Trig::Rigid { start: 0, power: 1 },
        3 => Trig::Rigid { start: 0, power: 2 },
        10 => Trig::Rigid { start: 0, power: 4 },
        4 => Trig::Rigid { start: 0, power: 5 },
        5 => Trig::Rigid { start: 0, power: 6 },
        6 => Trig::Rigid { start: 0, power: 7 },
        7 => Trig::Rigid { start: 0, power: 8 },
        // VibrateTrigger: a frequência vem no primeiro número.
        8 => Trig::Buzz { start: 0, power: 6, hz: (at(0) as u8).clamp(1, 40) },
        9 => Trig::Buzz { start: 0, power: 3, hz: 10 },
        // VibrateTriggerPulse.
        11 => Trig::Buzz { start: 0, power: 8, hz: 20 },
        // CustomTriggerValue: o modo do firmware vem no primeiro número; fica a Resistência
        // do ponto e da força dos seguintes.
        12 => Trig::Rigid { start: zone(at(1)), power: power(at(2)) },
        // Resistance: começo (0 a 9) e força (0 a 8).
        13 => Trig::Rigid { start: (at(0) as u8).min(9), power: (at(1) as u8).clamp(1, 8) },
        // Bow: começo, fim, força e força de volta.
        14 => Trig::Click { start: (at(0) as u8).min(9), power: (at(2) as u8).clamp(1, 8) },
        // Galloping: começo, fim, os dois pés e a frequência.
        15 => Trig::Buzz { start: (at(0) as u8).min(9), power: 4, hz: (at(4) as u8).clamp(1, 40) },
        // SemiAutomaticGun: começo, fim e força.
        16 => Trig::Click { start: (at(0) as u8).min(9), power: (at(2) as u8).clamp(1, 8) },
        // AutomaticGun: começo, força e frequência.
        17 => Trig::Buzz { start: (at(0) as u8).min(9), power: (at(1) as u8).clamp(1, 8), hz: (at(2) as u8).clamp(1, 40) },
        // Machine: começo, fim, as duas forças, a frequência e o período.
        18 => Trig::Buzz { start: (at(0) as u8).min(9), power: (at(2) as u8).max(at(3) as u8).clamp(1, 8), hz: (at(4) as u8).clamp(1, 40) },
        19 => Trig::Buzz { start: 0, power: 6, hz: 10 },
        20 => Trig::Off,
        // FEEDBACK: começo e força.
        21 => Trig::Rigid { start: (at(0) as u8).min(9), power: (at(1) as u8).clamp(1, 8) },
        // WEAPON: começo, fim e força.
        22 => Trig::Click { start: (at(0) as u8).min(9), power: (at(2) as u8).clamp(1, 8) },
        // VIBRATION: começo, força e frequência.
        23 => Trig::Buzz { start: (at(0) as u8).min(9), power: (at(1) as u8).clamp(1, 8), hz: (at(2) as u8).clamp(1, 40) },
        _ => return None,
    })
}

/// JSON mínimo para as mensagens dos mods: números, textos, verdadeiro e falso, nulo, listas e
/// objetos. Profundidade limitada: a mensagem vem de outro programa.
#[derive(Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn parse(s: &str) -> Option<Json> {
        let s = s.trim_start_matches('\u{feff}');
        let mut it = s.char_indices().peekable();
        let v = Self::value(s, &mut it, 0)?;
        Self::ws(&mut it);
        it.peek().is_none().then_some(v)
    }
    fn get(&self, k: &str) -> Option<&Json> {
        // O DSX aceita os nomes sem diferença de maiúsculas.
        match self {
            Json::Obj(m) => m.iter().find(|e| e.0.eq_ignore_ascii_case(k)).map(|e| &e.1),
            _ => None,
        }
    }
    fn array(&self) -> Option<&Vec<Json>> {
        if let Json::Arr(a) = self { Some(a) } else { None }
    }
    fn num(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            Json::Bool(b) => Some(*b as u8 as f64),
            Json::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }
    /// Número, ou o nome dele em `names` (sem diferença de maiúsculas).
    fn code(&self, names: &[&str]) -> Option<f64> {
        self.num().or_else(|| match self {
            Json::Str(s) => names.iter().position(|n| n.eq_ignore_ascii_case(s.trim())).map(|i| i as f64),
            _ => None,
        })
    }
    fn ws(it: &mut std::iter::Peekable<std::str::CharIndices>) {
        while it.peek().is_some_and(|c| c.1.is_whitespace()) {
            it.next();
        }
    }
    fn value(s: &str, it: &mut std::iter::Peekable<std::str::CharIndices>, depth: u8) -> Option<Json> {
        if depth > 16 {
            return None;
        }
        Self::ws(it);
        let &(start, c) = it.peek()?;
        Some(match c {
            '{' => {
                it.next();
                let mut m = vec![];
                loop {
                    Self::ws(it);
                    if it.peek()?.1 == '}' {
                        it.next();
                        break;
                    }
                    let Json::Str(k) = Self::value(s, it, depth + 1)? else { return None };
                    Self::ws(it);
                    (it.next()?.1 == ':').then_some(())?;
                    m.push((k, Self::value(s, it, depth + 1)?));
                    Self::ws(it);
                    match it.next()?.1 {
                        ',' => {}
                        '}' => break,
                        _ => return None,
                    }
                }
                Json::Obj(m)
            }
            '[' => {
                it.next();
                let mut a = vec![];
                loop {
                    Self::ws(it);
                    if it.peek()?.1 == ']' {
                        it.next();
                        break;
                    }
                    a.push(Self::value(s, it, depth + 1)?);
                    Self::ws(it);
                    match it.next()?.1 {
                        ',' => {}
                        ']' => break,
                        _ => return None,
                    }
                }
                Json::Arr(a)
            }
            '"' => {
                it.next();
                let mut out = String::new();
                loop {
                    match it.next()?.1 {
                        '"' => break,
                        '\\' => match it.next()?.1 {
                            'n' => out.push('\n'),
                            't' => out.push('\t'),
                            'u' => {
                                let h: String = (0..4).filter_map(|_| it.next().map(|c| c.1)).collect();
                                out.push(char::from_u32(u32::from_str_radix(&h, 16).ok()?).unwrap_or('?'));
                            }
                            c => out.push(c),
                        },
                        c => out.push(c),
                    }
                }
                Json::Str(out)
            }
            _ => {
                let mut end = start;
                while let Some(&(i, c)) = it.peek() {
                    if c == ',' || c == ']' || c == '}' || c.is_whitespace() {
                        break;
                    }
                    end = i + c.len_utf8();
                    it.next();
                }
                match &s[start..end] {
                    "true" => Json::Bool(true),
                    "false" => Json::Bool(false),
                    "null" => Json::Null,
                    t => Json::Num(t.parse().ok().filter(|n: &f64| n.is_finite())?),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pela porta de verdade: `cargo test --offline dsx_port_answers -- --ignored`.
    #[test]
    #[ignore]
    fn dsx_port_answers() {
        set_enabled(true);
        std::thread::sleep(Duration::from_millis(200));
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        s.send_to(br#"{"instructions":[{"type":2,"parameters":[0,1,2,3]}]}"#, ("127.0.0.1", PORT)).unwrap();
        let mut b = [0u8; 512];
        let n = s.recv(&mut b).unwrap();
        assert!(std::str::from_utf8(&b[..n]).unwrap().contains("DSX Received UDP Instructions"));
        assert_eq!(light(), Some([1, 2, 3]));
        // Desligar solta o pedido e fecha a porta.
        set_enabled(false);
        assert_eq!(light(), None);
        std::thread::sleep(Duration::from_millis(800));
        assert!(!RUNNING.load(Ordering::Relaxed));
    }

    #[test]
    fn dsx_messages_set_light_and_triggers() {
        let mut o = Over::default();
        let msg = r#"{"instructions":[{"type":2,"parameters":[0,255,"10",300]},{"type":1,"parameters":[0,2,13,3,6]},{"type":1,"parameters":[0,1,8,30]}]}"#;
        apply(&mut o, &Json::parse(msg).unwrap());
        assert_eq!(o.light, Some([255, 10, 255]));
        assert_eq!(o.trig[1], Some(Trig::Rigid { start: 3, power: 6 }));
        assert_eq!(o.trig[0], Some(Trig::Buzz { start: 0, power: 6, hz: 30 }));
        // Pelo nome, como o dsz aceita.
        apply(&mut o, &Json::parse(r#"{"instructions":[{"type":"TriggerUpdate","parameters":[0,1,"semiautomaticgun",2,7,8]}]}"#).unwrap());
        assert_eq!(o.trig[0], Some(Trig::Click { start: 2, power: 8 }));
        // Volta às opções do usuário.
        apply(&mut o, &Json::parse(r#"{"Instructions":[{"Type":7,"Parameters":[0]}]}"#).unwrap());
        assert_eq!(o, Over::default());
        // Lixo, gatilho inválido e mensagem funda demais não mexem em nada nem travam.
        for bad in ["", "{", "[1,2", "{\"instructions\":[{\"type\":1,\"parameters\":[0,9,7]}]}", &"[".repeat(500)] {
            if let Some(v) = Json::parse(bad) {
                apply(&mut o, &v);
            }
        }
        assert_eq!(o, Over::default());
        assert_eq!(Json::parse(" {\"a\": [true, null, -1.5e2, \"x\\\"\\u0041\"]} ").unwrap().get("A").unwrap().array().unwrap().len(), 4);
        // BOM no começo: os índices têm de ser do texto sem ele.
        assert!(matches!(Json::parse("\u{feff}{\"ab\": \"cd\"}").unwrap().get("ab"), Some(Json::Str(v)) if v == "cd"));
    }
}
