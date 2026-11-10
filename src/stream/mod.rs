//! Compartilhamento de tela, fase 1: ver a tela de outro PC na rede local.
//!
//! Ainda sem pareamento nem criptografia, então só existe na versão de teste.
//! `--stream-host[=porta]` espera quem assiste; `--stream-view=endereço[:porta]|saida.txt|segundos`
//! abre a janela que assiste (saída e segundos são opcionais, para medir sem ninguém olhando).

pub mod host;
pub mod view;

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
}

/// O que um pacote diz.
#[derive(Debug, PartialEq)]
pub enum Msg<'a> {
    Video { key: bool, idx: u16, count: u16, frame: u32, pts: i64, data: &'a [u8] },
    /// Como o vídeo está saindo: codec, tamanho e quadros por segundo.
    Info { codec: Codec, w: u32, h: u32, fps: u32 },
    /// Quem assiste chegou: tamanho e Hz da tela dele e codecs que ele decodifica (bits 0 H.264, 1 HEVC, 2 AV1).
    Hello { w: u32, h: u32, hz: u32, codecs: u8 },
    KeyReq,
    Ping { t: i64 },
    /// Resposta ao Ping, com a hora do host para acertar os relógios.
    Pong { t: i64, host: i64 },
    Bye,
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
        Msg::Info { codec, w, h, fps } => {
            head(out, Kind::Info);
            out.push(codec_id(codec));
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&h.to_le_bytes());
            out.extend_from_slice(&fps.to_le_bytes());
        }
        Msg::Hello { w, h, hz, codecs } => {
            head(out, Kind::Hello);
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&h.to_le_bytes());
            out.extend_from_slice(&hz.to_le_bytes());
            out.push(codecs);
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
    }
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
        2 => Msg::Info { codec: codec_of(*b.get(4)?)?, w: u32_at(b, 5)?, h: u32_at(b, 9)?, fps: u32_at(b, 13)? },
        3 => Msg::Hello { w: u32_at(b, 4)?, h: u32_at(b, 8)?, hz: u32_at(b, 12)?, codecs: *b.get(16)? },
        4 => Msg::KeyReq,
        5 => Msg::Ping { t: i64_at(b, 4)? },
        6 => Msg::Pong { t: i64_at(b, 4)?, host: i64_at(b, 12)? },
        7 => Msg::Bye,
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
        if a == "--stream-host" || a.starts_with("--stream-host=") {
            let port = a.strip_prefix("--stream-host=").and_then(|p| p.parse().ok()).unwrap_or(PORT);
            host::run(port);
            return true;
        }
        if let Some(v) = a.strip_prefix("--stream-view=") {
            view::run(v);
            return true;
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
        round(Msg::Info { codec: Codec::Av1, w: 1920, h: 1080, fps: 60 });
        round(Msg::Hello { w: 3840, h: 2160, hz: 119, codecs: 7 });
        round(Msg::KeyReq);
        round(Msg::Ping { t: 123 });
        round(Msg::Pong { t: 1, host: 2 });
        round(Msg::Bye);
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
