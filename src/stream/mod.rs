//! Compartilhar tela: a página Compartilhar tela chama estes modos em processos próprios.
//!
//! Rede local, sem pareamento nem criptografia: `--stream-host[=porta]` espera quem assiste; `--stream-control[=porta]` faz o mesmo e aceita mouse
//! e teclado de quem assiste. `--stream-view=endereço[:porta]|saida.txt|segundos` abre a janela que
//! assiste (saída e segundos são opcionais, para medir sem ninguém olhando).
//!
//! Internet, com pareamento e criptografia (compilado com `--features stream`): veja `net.rs`.

mod clip;
mod hud;
#[cfg(feature = "stream")]
pub mod code;
pub mod host;
mod link;
#[cfg(feature = "stream")]
mod net;
#[cfg(feature = "stream")]
pub use net::{device_rows, forget_device, me, set_device, CODE_FAIL, CODE_LINE, CODE_OK};
mod pads;
mod screen;
#[cfg(feature = "stream")]
pub mod service;
pub mod view;
#[cfg(feature = "stream")]
pub mod weblink;

pub use link::*;
mod opts;
pub use opts::*;

use crate::capture::mux::Codec;

pub const PORT: u16 = 47_600;

/// Pasta dos dados do compartilhamento: a do serviço, quando ele está instalado (ela é a mesma
/// para o ajudante SYSTEM e para quem usa o PC).
pub fn dir() -> std::path::PathBuf {
    let service = service_dir();
    if SERVICE.load(std::sync::atomic::Ordering::Relaxed) || service.join("servico.tsv").exists() {
        service
    } else {
        user_dir()
    }
}

pub fn user_dir() -> std::path::PathBuf {
    crate::config::data_dir().join("compartilhar")
}

pub fn service_dir() -> std::path::PathBuf {
    crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramData).join("Prisma").join("compartilhar")
}

/// Este processo é o ajudante do serviço (SYSTEM, na sessão de quem usa o PC).
pub static SERVICE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Tamanho máximo de um pacote: cabe em qualquer rede sem se partir.
pub const MTU: usize = 1200;
const MAGIC: [u8; 2] = *b"PR";
const VERSION: u8 = 1;
pub const VIDEO_HEAD: usize = 24;

#[derive(Clone, Copy, PartialEq, Debug)]
#[repr(u8)]
pub enum Kind {
    Video = 1,
    Info = 2,
    Hello = 3,
    KeyReq = 4,
    Ping = 5,
    Pong = 6,
    Bye = 7,
    Audio = 8,
    Input = 9,
    Nack = 10,
    Pad = 11,
    Rumble = 12,
    PadInfo = 13,
    Sas = 14,
    Clip = 15,
}

// ---------- Permissões de quem assiste ----------
// Ver o vídeo vale sempre; o resto é por aparelho. Os bits livres ficam para o que vier
// (área de transferência, controle do host), e o "Acesso total" já os inclui.

pub const SOUND: u8 = 1;
pub const PAD: u8 = 2;
pub const KEYBOARD: u8 = 4;
pub const MOUSE: u8 = 8;
/// A tela segura do Windows (UAC, bloqueio, PIN) e o Ctrl+Alt+Del, só com o serviço.
pub const SYSTEM: u8 = 16;
/// Área de transferência, só texto, nos dois sentidos.
pub const COPY: u8 = 32;
pub const TOTAL: u8 = 0xFF;

/// Predefinições: nome no arquivo e na linha de comando, nome na tela, permissões.
pub const PRESETS: [(&str, &str, u8); 4] = [
    ("total", "Acesso total", TOTAL),
    ("jogar", "Ver e usar controle", SOUND | PAD),
    ("teclado-mouse", "Ver, controle, teclado e mouse", SOUND | PAD | KEYBOARD | MOUSE),
    ("ver", "Só ver", SOUND),
];
pub const PERM_NAMES: [(&str, &str, u8); 6] = [
    ("som", "som", SOUND),
    ("controle", "controle de jogo", PAD),
    ("teclado", "teclado", KEYBOARD),
    ("mouse", "mouse", MOUSE),
    ("sistema", "tela segura do Windows", SYSTEM),
    ("copiar", "área de transferência", COPY),
];

/// Lê uma predefinição ou uma lista como `som,controle`. `video` é só o vídeo, sem nada mais.
/// Aceita os nomes antigos do arquivo: `controlar` (tudo) e `ver`.
#[cfg_attr(not(feature = "stream"), allow(dead_code))]
pub fn parse_perms(s: &str) -> Option<u8> {
    let s = s.trim().to_lowercase();
    match s.as_str() {
        "controlar" | "control" => return Some(TOTAL),
        "video" | "vídeo" => return Some(0),
        _ => {}
    }
    if let Some(p) = PRESETS.iter().find(|p| p.0 == s) {
        return Some(p.2);
    }
    let mut bits = 0;
    for part in s.split([',', '+', ' ']).filter(|p| !p.is_empty()) {
        bits |= PERM_NAMES.iter().find(|n| n.0 == part)?.2;
    }
    (bits != 0).then_some(bits)
}

/// Como as permissões vão para o arquivo: a predefinição, ou a lista.
#[cfg_attr(not(feature = "stream"), allow(dead_code))]
pub fn perms_key(p: u8) -> String {
    if let Some(x) = PRESETS.iter().find(|x| x.2 == p) {
        return x.0.to_string();
    }
    let list: Vec<&str> = PERM_NAMES.iter().filter(|n| p & n.2 != 0).map(|n| n.0).collect();
    if list.is_empty() {
        "video".to_string()
    } else {
        list.join(",")
    }
}

/// Como as permissões aparecem para ela.
pub fn perms_text(p: u8) -> String {
    if let Some(x) = PRESETS.iter().find(|x| x.2 == p) {
        return x.1.to_string();
    }
    let mut list = vec!["ver"];
    list.extend(PERM_NAMES.iter().filter(|n| p & n.2 != 0).map(|n| n.1));
    list.join(", ")
}

// ---------- Controles ----------

/// Estado de um controle tipo Xbox, igual ao do XInput.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PadState {
    pub buttons: u16,
    pub lt: u8,
    pub rt: u8,
    pub lx: i16,
    pub ly: i16,
    pub rx: i16,
    pub ry: i16,
}

/// Até quatro controles de cada lado, como no XInput. O número 4 é o teclado virando controle.
pub const PADS: usize = 5;
/// `PadInfo.why`: por que o controle não entrou.
pub const PAD_OK: u8 = 0;
pub const PAD_NO_PERMISSION: u8 = 1;
pub const PAD_NO_DRIVER: u8 = 2;
pub const PAD_FULL: u8 = 3;
pub const PAD_ANTI_CHEAT: u8 = 4;
pub const PAD_LOOP: u8 = 5;

pub fn pad_why(why: u8) -> &'static str {
    match why {
        PAD_NO_PERMISSION => "sem permissão de controle",
        PAD_NO_DRIVER => "o host não tem a ponte de controles",
        PAD_FULL => "as quatro vagas estão ocupadas",
        PAD_ANTI_CHEAT => "o jogo aberto no host tem anti-cheat",
        PAD_LOOP => "é um controle virtual do próprio host",
        _ => "",
    }
}

/// Som: 48 kHz, dois canais, 16 bits, em blocos de 5 ms.
pub const AUDIO_RATE: u32 = 48_000;
pub const AUDIO_BLOCK: usize = 240;

/// Um evento de mouse ou teclado de quem assiste.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Input {
    /// Posição na imagem, de 0 a 65535 nos dois eixos.
    Move { x: u16, y: u16 },
    /// 0 esquerdo, 1 direito, 2 meio, 3 e 4 laterais.
    Button { button: u8, down: bool },
    Wheel { delta: i16, horizontal: bool },
    /// Código de varredura do teclado, que não depende do idioma do teclado.
    Key { scan: u16, extended: bool, down: bool },
}

pub const INPUT_SIZE: usize = 8;
/// Quantos eventos recentes vão em cada pacote, para um pacote perdido não perder uma tecla.
pub const INPUT_KEEP: usize = 16;

impl Input {
    fn put(&self, out: &mut Vec<u8>) {
        let (k, f, a, b): (u8, u8, u16, u16) = match *self {
            Input::Move { x, y } => (1, 0, x, y),
            Input::Button { button, down } => (2, down as u8, button as u16, 0),
            Input::Wheel { delta, horizontal } => (3, horizontal as u8, delta as u16, 0),
            Input::Key { scan, extended, down } => (4, down as u8 | (extended as u8) << 1, scan, 0),
        };
        out.push(k);
        out.push(f);
        out.extend_from_slice(&a.to_le_bytes());
        out.extend_from_slice(&b.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
    }

    pub fn get(b: &[u8]) -> Option<Self> {
        let (k, f, a, c) = (*b.first()?, *b.get(1)?, u16_at(b, 2)?, u16_at(b, 4)?);
        Some(match k {
            1 => Input::Move { x: a, y: c },
            2 => Input::Button { button: a as u8, down: f & 1 != 0 },
            3 => Input::Wheel { delta: a as i16, horizontal: f & 1 != 0 },
            4 => Input::Key { scan: a, extended: f & 2 != 0, down: f & 1 != 0 },
            _ => return None,
        })
    }
}

/// `Hello.flags`: a tela de quem assiste está em HDR.
pub const SEES_HDR: u8 = 1;
/// `Hello.flags`: quem assiste pede 4:4:4 (`PRISMA_444=1`) e decodifica HEVC 4:4:4 (NVDEC numa placa
/// NVIDIA).
pub const DECODES_444: u8 = 2;

/// O que um pacote diz.
#[derive(Debug, PartialEq)]
pub enum Msg<'a> {
    Video { key: bool, idx: u16, count: u16, frame: u32, pts: i64, data: &'a [u8] },
    /// Como o vídeo está saindo: codec, tamanho e quadros por segundo.
    /// `perms` são as permissões de quem assiste ([`SOUND`], [`PAD`], [`KEYBOARD`], [`MOUSE`]); `recover`, se ele se recupera de um quadro
    /// perdido sem quadro-chave (quem assiste avisa com `Nack` e segue mostrando). `hdr10`: 10 bits,
    /// BT.2020 com PQ. `yuv444`: HEVC com a cor inteira em cada pixel.
    Info { codec: Codec, w: u32, h: u32, fps: u32, perms: u8, recover: bool, hdr10: bool, yuv444: bool },
    /// Quem assiste chegou: tamanho e Hz da tela dele, codecs que ele decodifica (bits 0 H.264,
    /// 1 HEVC, 2 AV1) e o número do processo, para o host tirar o som dele quando os dois estão no mesmo PC.
    /// `flags`: [`SEES_HDR`] e [`DECODES_444`].
    Hello { w: u32, h: u32, hz: u32, codecs: u8, pid: u32, flags: u8 },
    KeyReq,
    Ping { t: i64 },
    /// Resposta ao Ping, com a hora do host para acertar os relógios.
    Pong { t: i64, host: i64 },
    Bye,
    /// Bloco de som, com número de sequência.
    Audio { seq: u32, pts: i64, data: &'a [u8] },
    /// Os eventos mais recentes; o primeiro tem o número `first` e os seguintes vêm em ordem.
    Input { first: u32, events: &'a [u8] },
    /// Quem assiste perdeu este quadro (e talvez os seguintes até agora).
    Nack { frame: u32 },
    /// Um controle de quem assiste: o número dele lá (0 a 3, 4 é o teclado), a sequência (só vale a
    /// mais nova) e o estado. `on` falso: o controle saiu.
    Pad { pad: u8, seq: u16, on: bool, state: PadState },
    /// Vibração do controle virtual, de 0 a 255 nos dois motores.
    Rumble { pad: u8, low: u8, high: u8 },
    /// O que o host fez com o controle: jogador 1 a 4, a vaga do XInput lá (255 sem saber), ou
    /// jogador 0 e o motivo em `why`.
    PadInfo { pad: u8, player: u8, slot: u8, why: u8 },
    /// Ctrl+Alt+Del no host (só com o serviço e a permissão `SYSTEM`).
    Sas,
    /// Um pedaço de texto copiado: `part` de `parts`, do texto `id`.
    Clip { id: u16, part: u8, parts: u8, data: &'a [u8] },
}

pub fn codec_bit(c: Codec) -> u8 {
    match c {
        Codec::H264 => 1,
        Codec::Hevc => 2,
        Codec::Av1 => 4,
    }
}

fn codec_id(c: Codec) -> u8 {
    match c {
        Codec::H264 => 0,
        Codec::Hevc => 1,
        Codec::Av1 => 2,
    }
}

fn codec_of(v: u8) -> Option<Codec> {
    Some(match v {
        0 => Codec::H264,
        1 => Codec::Hevc,
        2 => Codec::Av1,
        _ => return None,
    })
}

pub fn codec_name(c: Codec) -> &'static str {
    match c {
        Codec::Av1 => "AV1",
        Codec::Hevc => "HEVC",
        Codec::H264 => "H.264",
    }
}

fn head(out: &mut Vec<u8>, k: Kind) {
    out.clear();
    out.extend_from_slice(&MAGIC);
    out.push(k as u8);
    out.push(VERSION);
}

pub fn write(out: &mut Vec<u8>, m: &Msg) {
    match *m {
        Msg::Video { key, idx, count, frame, pts, data } => {
            head(out, Kind::Video);
            out.push(key as u8);
            out.push(0);
            out.extend_from_slice(&idx.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(&frame.to_le_bytes());
            out.extend_from_slice(&pts.to_le_bytes());
            out.extend_from_slice(data);
        }
        Msg::Info { codec, w, h, fps, perms, recover, hdr10, yuv444 } => {
            head(out, Kind::Info);
            out.push(codec_id(codec));
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&h.to_le_bytes());
            out.extend_from_slice(&fps.to_le_bytes());
            out.push(perms);
            out.push(recover as u8);
            out.push(hdr10 as u8);
            out.push(yuv444 as u8);
        }
        Msg::Hello { w, h, hz, codecs, pid, flags } => {
            head(out, Kind::Hello);
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&h.to_le_bytes());
            out.extend_from_slice(&hz.to_le_bytes());
            out.push(codecs);
            out.extend_from_slice(&pid.to_le_bytes());
            out.push(flags);
        }
        Msg::KeyReq => head(out, Kind::KeyReq),
        Msg::Ping { t } => {
            head(out, Kind::Ping);
            out.extend_from_slice(&t.to_le_bytes());
        }
        Msg::Pong { t, host } => {
            head(out, Kind::Pong);
            out.extend_from_slice(&t.to_le_bytes());
            out.extend_from_slice(&host.to_le_bytes());
        }
        Msg::Bye => head(out, Kind::Bye),
        Msg::Audio { seq, pts, data } => {
            head(out, Kind::Audio);
            out.extend_from_slice(&seq.to_le_bytes());
            out.extend_from_slice(&pts.to_le_bytes());
            out.extend_from_slice(data);
        }
        Msg::Input { first, events } => {
            head(out, Kind::Input);
            out.extend_from_slice(&first.to_le_bytes());
            out.extend_from_slice(events);
        }
        Msg::Nack { frame } => {
            head(out, Kind::Nack);
            out.extend_from_slice(&frame.to_le_bytes());
        }
        Msg::Pad { pad, seq, on, state: s } => {
            head(out, Kind::Pad);
            out.push(pad);
            out.push(on as u8);
            out.extend_from_slice(&seq.to_le_bytes());
            out.extend_from_slice(&s.buttons.to_le_bytes());
            out.push(s.lt);
            out.push(s.rt);
            for v in [s.lx, s.ly, s.rx, s.ry] {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        Msg::Rumble { pad, low, high } => {
            head(out, Kind::Rumble);
            out.extend_from_slice(&[pad, low, high]);
        }
        Msg::PadInfo { pad, player, slot, why } => {
            head(out, Kind::PadInfo);
            out.extend_from_slice(&[pad, player, slot, why]);
        }
        Msg::Sas => head(out, Kind::Sas),
        Msg::Clip { id, part, parts, data } => {
            head(out, Kind::Clip);
            out.extend_from_slice(&id.to_le_bytes());
            out.extend_from_slice(&[part, parts]);
            out.extend_from_slice(data);
        }
    }
}

/// Monta o pacote de entrada com os eventos guardados (o mais antigo tem o número `first`).
pub fn write_input(out: &mut Vec<u8>, first: u32, events: &[Input]) {
    let mut ev = Vec::with_capacity(events.len() * INPUT_SIZE);
    for e in events {
        e.put(&mut ev);
    }
    write(out, &Msg::Input { first, events: &ev });
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(i..i + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}
fn i64_at(b: &[u8], i: usize) -> Option<i64> {
    Some(i64::from_le_bytes(b.get(i..i + 8)?.try_into().ok()?))
}

/// Lê um pacote. Pacote de outro programa, de outra versão ou cortado vira `None`.
pub fn read(b: &[u8]) -> Option<Msg<'_>> {
    if b.len() < 4 || b[..2] != MAGIC || b[3] != VERSION {
        return None;
    }
    Some(match b[2] {
        1 => Msg::Video {
            key: *b.get(4)? != 0,
            idx: u16_at(b, 6)?,
            count: u16_at(b, 8)?,
            frame: u32_at(b, 12)?,
            pts: i64_at(b, 16)?,
            data: b.get(VIDEO_HEAD..)?,
        },
        2 => Msg::Info { codec: codec_of(*b.get(4)?)?, w: u32_at(b, 5)?, h: u32_at(b, 9)?, fps: u32_at(b, 13)?, perms: *b.get(17)?, recover: *b.get(18)? != 0, hdr10: *b.get(19)? != 0, yuv444: *b.get(20)? != 0 },
        3 => Msg::Hello { w: u32_at(b, 4)?, h: u32_at(b, 8)?, hz: u32_at(b, 12)?, codecs: *b.get(16)?, pid: u32_at(b, 17)?, flags: *b.get(21)? },
        4 => Msg::KeyReq,
        5 => Msg::Ping { t: i64_at(b, 4)? },
        6 => Msg::Pong { t: i64_at(b, 4)?, host: i64_at(b, 12)? },
        7 => Msg::Bye,
        8 => Msg::Audio { seq: u32_at(b, 4)?, pts: i64_at(b, 8)?, data: b.get(16..)? },
        9 => {
            let events = b.get(8..)?;
            if events.len() % INPUT_SIZE != 0 {
                return None;
            }
            Msg::Input { first: u32_at(b, 4)?, events }
        }
        10 => Msg::Nack { frame: u32_at(b, 4)? },
        11 => Msg::Pad {
            pad: *b.get(4)?,
            on: *b.get(5)? != 0,
            seq: u16_at(b, 6)?,
            state: PadState {
                buttons: u16_at(b, 8)?,
                lt: *b.get(10)?,
                rt: *b.get(11)?,
                lx: u16_at(b, 12)? as i16,
                ly: u16_at(b, 14)? as i16,
                rx: u16_at(b, 16)? as i16,
                ry: u16_at(b, 18)? as i16,
            },
        },
        12 => Msg::Rumble { pad: *b.get(4)?, low: *b.get(5)?, high: *b.get(6)? },
        13 => Msg::PadInfo { pad: *b.get(4)?, player: *b.get(5)?, slot: *b.get(6)?, why: *b.get(7)? },
        14 => Msg::Sas,
        15 => Msg::Clip { id: u16_at(b, 4)?, part: *b.get(6)?, parts: *b.get(7)?, data: b.get(8..)? },
        _ => return None,
    })
}

/// Agora em 100 ns, na mesma régua do relógio da captura.
pub fn now() -> i64 {
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    unsafe {
        let (mut c, mut f) = (0i64, 0i64);
        let _ = QueryPerformanceCounter(&mut c);
        let _ = QueryPerformanceFrequency(&mut f);
        (c as i128 * 10_000_000 / f.max(1) as i128) as i64
    }
}

/// Hz da tela de um monitor.
pub fn monitor_hz(mon: windows::Win32::Graphics::Gdi::HMONITOR) -> u32 {
    use windows::Win32::Graphics::Gdi::*;
    unsafe {
        let mut ex = MONITORINFOEXW::default();
        ex.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let _ = GetMonitorInfoW(mon, &mut ex as *mut _ as *mut MONITORINFO);
        let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        if EnumDisplaySettingsW(windows::core::PCWSTR(ex.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool() && dm.dmDisplayFrequency > 1 {
            dm.dmDisplayFrequency
        } else {
            60
        }
    }
}

/// Trata os argumentos da transmissão. Devolve verdadeiro quando um deles rodou.
pub fn run_args() -> bool {
    for a in std::env::args() {
        for (name, control) in [("--stream-host", false), ("--stream-control", true)] {
            if a == name || a.strip_prefix(name).is_some_and(|r| r.starts_with('=')) {
                let port = a.strip_prefix(name).and_then(|r| r.strip_prefix('=')).and_then(|p| p.parse().ok()).unwrap_or(PORT);
                host::run(port, control);
                return true;
            }
        }
        if let Some(v) = a.strip_prefix("--stream-view=") {
            view::run(v);
            return true;
        }
        if let Some(v) = a.strip_prefix("--test-dda=") {
            screen::bench(v);
            return true;
        }
        #[cfg(feature = "stream")]
        {
            // Sem valor: quem parear agora só vê. `--stream-net=jogar`, `=total` e outros dão mais.
            if a == "--stream-net" || a.starts_with("--stream-net=") {
                let v = a.strip_prefix("--stream-net=").unwrap_or("ver");
                match parse_perms(v) {
                    Some(p) => net::host(p),
                    None => eprintln!("permissão desconhecida: «{v}» (use total, jogar, teclado-mouse, ver, video, ou uma lista de som, controle, teclado, mouse)"),
                }
                return true;
            }
            // A página: compartilha sem convite; os convites vêm de --stream-invite.
            if a == "--stream-share" {
                net::host_quiet();
                return true;
            }
            if let Some(v) = a.strip_prefix("--stream-allow=") {
                net::allow(v);
                return true;
            }
            if let Some(v) = a.strip_prefix("--test-pads=") {
                pads::bench(v);
                return true;
            }
            if service::run_args(&a) {
                return true;
            }
            if let Some(v) = a.strip_prefix("--stream-pair=") {
                net::pair(v);
                return true;
            }
            if let Some(v) = a.strip_prefix("--stream-connect=") {
                net::connect(v);
                return true;
            }
            if a == "--stream-devices" {
                net::devices();
                return true;
            }
            if let Some(v) = a.strip_prefix("--stream-forget=") {
                net::forget(v);
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(m: Msg) {
        let mut b = Vec::new();
        write(&mut b, &m);
        assert_eq!(read(&b), Some(m));
    }

    #[test]
    fn packets_round_trip() {
        round(Msg::Video { key: true, idx: 3, count: 9, frame: 77, pts: -5, data: &[1, 2, 3] });
        round(Msg::Info { codec: Codec::Av1, w: 1920, h: 1080, fps: 60, perms: SOUND | PAD, recover: true, hdr10: true, yuv444: false });
        round(Msg::Pad { pad: 4, seq: 65535, on: true, state: PadState { buttons: 0xF00F, lt: 255, rt: 1, lx: -32768, ly: 32767, rx: -1, ry: 0 } });
        round(Msg::Pad { pad: 0, seq: 0, on: false, state: PadState::default() });
        round(Msg::Rumble { pad: 2, low: 200, high: 7 });
        round(Msg::PadInfo { pad: 1, player: 3, slot: 2, why: PAD_OK });
        round(Msg::Sas);
        round(Msg::Clip { id: 300, part: 1, parts: 3, data: b"texto" });
        round(Msg::Nack { frame: 77 });
        round(Msg::Hello { w: 3840, h: 2160, hz: 119, codecs: 7, pid: 4321, flags: SEES_HDR | DECODES_444 });
        round(Msg::Audio { seq: 9, pts: 10, data: &[0, 1, 2, 3] });
        round(Msg::KeyReq);
        round(Msg::Ping { t: 123 });
        round(Msg::Pong { t: 1, host: 2 });
        round(Msg::Bye);
    }

    #[test]
    fn input_round_trip() {
        let evs = [
            Input::Move { x: 65535, y: 12 },
            Input::Button { button: 4, down: true },
            Input::Wheel { delta: -120, horizontal: true },
            Input::Key { scan: 0x1D, extended: true, down: false },
        ];
        let mut b = Vec::new();
        write_input(&mut b, 41, &evs);
        let Some(Msg::Input { first, events }) = read(&b) else { panic!() };
        assert_eq!(first, 41);
        let back: Vec<Input> = events.chunks(INPUT_SIZE).filter_map(Input::get).collect();
        assert_eq!(back, evs);
    }

    #[test]
    fn perms_presets_and_lists() {
        assert_eq!(parse_perms("total"), Some(TOTAL));
        assert_eq!(parse_perms("controlar"), Some(TOTAL));
        assert_eq!(parse_perms("Ver"), Some(SOUND));
        assert_eq!(parse_perms("jogar"), Some(SOUND | PAD));
        assert_eq!(parse_perms("controle"), Some(PAD));
        assert_eq!(parse_perms("som,controle,teclado,mouse"), Some(SOUND | PAD | KEYBOARD | MOUSE));
        assert_eq!(parse_perms("teclado mouse"), Some(KEYBOARD | MOUSE));
        assert_eq!(parse_perms("video"), Some(0));
        assert_eq!(parse_perms("voar"), None);
        assert_eq!(parse_perms(""), None);
        // Ida e volta pelo arquivo.
        assert_eq!(parse_perms("teclado,mouse,sistema"), Some(KEYBOARD | MOUSE | SYSTEM));
        assert_eq!(parse_perms("copiar"), Some(COPY));
        for p in [0, SOUND, PAD, SOUND | PAD, PAD | MOUSE, KEYBOARD | MOUSE, SOUND | PAD | KEYBOARD | MOUSE, SYSTEM | MOUSE, TOTAL] {
            assert_eq!(parse_perms(&perms_key(p)), Some(p), "{p}");
        }
        assert_eq!(perms_text(SOUND | PAD), "Ver e usar controle");
        assert_eq!(perms_text(PAD | MOUSE), "ver, controle de jogo, mouse");
    }

    #[test]
    fn rejects_foreign_packets() {
        assert_eq!(read(b"XX\x01\x01"), None);
        assert_eq!(read(b"PR\x01\x02"), None);
        assert_eq!(read(b"PR\x02\x01\x09"), None);
        assert_eq!(read(b"PR"), None);
    }

    #[test]
    fn video_header_size() {
        let mut b = Vec::new();
        write(&mut b, &Msg::Video { key: false, idx: 0, count: 1, frame: 0, pts: 0, data: &[] });
        assert_eq!(b.len(), VIDEO_HEAD);
    }
}
