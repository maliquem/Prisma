//! Compartilhamento de tela, só na versão de teste por enquanto.
//!
//! Rede local, sem pareamento nem criptografia: `--stream-host[=porta]` espera quem assiste; `--stream-control[=porta]` faz o mesmo e aceita mouse
//! e teclado de quem assiste. `--stream-view=endereço[:porta]|saida.txt|segundos` abre a janela que
//! assiste (saída e segundos são opcionais, para medir sem ninguém olhando).
//!
//! Internet, com pareamento e criptografia (compilado com `--features stream`): veja `net.rs`.

pub mod host;
mod link;
#[cfg(feature = "stream")]
mod net;
pub mod view;

pub use link::*;

use crate::capture::mux::Codec;

pub const PORT: u16 = 47_600;
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
    /// `control` diz se o host aceita mouse e teclado; `recover`, se ele se recupera de um quadro
    /// perdido sem quadro-chave (quem assiste avisa com `Nack` e segue mostrando). `hdr10`: 10 bits,
    /// BT.2020 com PQ. `yuv444`: HEVC com a cor inteira em cada pixel.
    Info { codec: Codec, w: u32, h: u32, fps: u32, control: bool, recover: bool, hdr10: bool, yuv444: bool },
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
        Msg::Info { codec, w, h, fps, control, recover, hdr10, yuv444 } => {
            head(out, Kind::Info);
            out.push(codec_id(codec));
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&h.to_le_bytes());
            out.extend_from_slice(&fps.to_le_bytes());
            out.push(control as u8);
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
        2 => Msg::Info { codec: codec_of(*b.get(4)?)?, w: u32_at(b, 5)?, h: u32_at(b, 9)?, fps: u32_at(b, 13)?, control: *b.get(17)? != 0, recover: *b.get(18)? != 0, hdr10: *b.get(19)? != 0, yuv444: *b.get(20)? != 0 },
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
        #[cfg(feature = "stream")]
        {
            if a == "--stream-net" || a == "--stream-net=control" {
                net::host(a.ends_with("=control"));
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
        round(Msg::Info { codec: Codec::Av1, w: 1920, h: 1080, fps: 60, control: true, recover: true, hdr10: true, yuv444: false });
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
