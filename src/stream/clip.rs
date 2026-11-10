//! Área de transferência, só texto, nos dois sentidos (fase 7). Cada lado vigia a própria área
//! de transferência e manda o texto novo em pedaços; quem recebe junta e grava.
//!
//! Só vale para quem tem a permissão `COPY`. O texto vai até 64 KB.

use std::sync::mpsc;
use std::time::Duration;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{write, Msg};

/// Tamanho máximo do texto, em bytes UTF-8.
pub const MAX_TEXT: usize = 64 * 1024;
/// Bytes de texto por pacote: cabe no MTU com o cabeçalho.
const PART: usize = 1000;
const CF_UNICODETEXT: u32 = 13;

/// A vigia da área de transferência deste PC, numa thread com janela própria.
pub struct Clip {
    /// Texto que chegou de longe, para gravar aqui.
    pub set: mpsc::Sender<String>,
    /// Texto copiado aqui, para mandar.
    pub changed: mpsc::Receiver<String>,
}

pub fn start() -> Clip {
    let (set_tx, set_rx) = mpsc::channel::<String>();
    let (ch_tx, ch_rx) = mpsc::channel();
    std::thread::spawn(move || unsafe {
        // O Windows quer uma janela dona para gravar, e ela precisa atender mensagens: quem
        // esvazia a área de transferência avisa a dona e espera a resposta.
        let Ok(h) = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("PrismaClip"), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None) else {
            return;
        };
        let mut seq = GetClipboardSequenceNumber();
        // O último texto que passou por aqui, nos dois sentidos: com os dois lados no mesmo PC,
        // o texto gravado voltaria como novo para sempre.
        let mut last = String::new();
        let mut msg = MSG::default();
        loop {
            let _ = MsgWaitForMultipleObjects(None, false, 150, QS_ALLINPUT);
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            loop {
                match set_rx.try_recv() {
                    Ok(t) => {
                        if put(h, &t) {
                            seq = GetClipboardSequenceNumber();
                        }
                        last = t;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        let _ = DestroyWindow(h);
                        return;
                    }
                }
            }
            let now = GetClipboardSequenceNumber();
            if now != seq {
                seq = now;
                if let Some(t) = get(h).filter(|t| *t != last) {
                    last = t.clone();
                    if ch_tx.send(t).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = DestroyWindow(h);
    });
    Clip { set: set_tx, changed: ch_rx }
}

/// A área de transferência pode estar aberta por outro programa: tenta algumas vezes.
unsafe fn open(h: HWND) -> bool {
    for _ in 0..10 {
        if OpenClipboard(Some(h)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

unsafe fn put(h: HWND, text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else { return false };
    let p = GlobalLock(mem) as *mut u16;
    if p.is_null() {
        let _ = GlobalFree(Some(mem));
        return false;
    }
    std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
    let _ = GlobalUnlock(mem);
    if !open(h) {
        let _ = GlobalFree(Some(mem));
        return false;
    }
    let _ = EmptyClipboard();
    let ok = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(mem.0))).is_ok();
    if !ok {
        let _ = GlobalFree(Some(mem));
    }
    let _ = CloseClipboard();
    ok
}

unsafe fn get(h: HWND) -> Option<String> {
    if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() || !open(h) {
        return None;
    }
    let out = (|| {
        let d = GetClipboardData(CF_UNICODETEXT).ok()?;
        let p = GlobalLock(HGLOBAL(d.0)) as *const u16;
        if p.is_null() {
            return None;
        }
        let max = GlobalSize(HGLOBAL(d.0)) / 2;
        let mut n = 0;
        while n < max && *p.add(n) != 0 {
            n += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
        let _ = GlobalUnlock(HGLOBAL(d.0));
        Some(s)
    })();
    let _ = CloseClipboard();
    out.filter(|s| !s.is_empty())
}

/// Corta o texto em pacotes `Msg::Clip`. Texto grande demais fica cortado num caractere inteiro.
pub fn packets(text: &str, id: u16) -> Vec<Vec<u8>> {
    let mut end = text.len().min(MAX_TEXT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let b = &text.as_bytes()[..end];
    let chunks: Vec<&[u8]> = if b.is_empty() { vec![b] } else { b.chunks(PART).collect() };
    let parts = chunks.len() as u8;
    chunks
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut out = Vec::with_capacity(c.len() + 16);
            write(&mut out, &Msg::Clip { id, part: i as u8, parts, data: c });
            out
        })
        .collect()
}

/// Junta os pedaços de um texto. Os pacotes vão repetidos: o que já foi entregue é ignorado.
#[derive(Default)]
pub struct Pieces {
    id: Option<u16>,
    got: Vec<Option<Vec<u8>>>,
    done: Option<u16>,
}

impl Pieces {
    pub fn add(&mut self, id: u16, part: u8, parts: u8, data: &[u8]) -> Option<String> {
        if self.done == Some(id) || parts == 0 || part >= parts || parts as usize > MAX_TEXT.div_ceil(PART) {
            return None;
        }
        if self.id != Some(id) {
            self.id = Some(id);
            self.got = vec![None; parts as usize];
        }
        if self.got.len() != parts as usize {
            return None;
        }
        self.got[part as usize] = Some(data.to_vec());
        if self.got.iter().any(Option::is_none) {
            return None;
        }
        self.done = Some(id);
        let all: Vec<u8> = std::mem::take(&mut self.got).into_iter().flatten().flatten().collect();
        String::from_utf8(all).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_goes_in_pieces_and_back() {
        let text: String = "ação 😀 ".repeat(400);
        let pk = packets(&text, 7);
        assert!(pk.len() > 1);
        let mut p = Pieces::default();
        let mut out = None;
        // Fora de ordem e repetidos, como numa rede ruim.
        for b in pk.iter().rev().chain(pk.iter()) {
            if let Some(Msg::Clip { id, part, parts, data }) = super::super::read(b) {
                if let Some(t) = p.add(id, part, parts, data) {
                    assert!(out.is_none(), "entregou duas vezes");
                    out = Some(t);
                }
            }
        }
        assert_eq!(out.as_deref(), Some(text.as_str()));
        // Grande demais: corta num caractere inteiro.
        let big = "é".repeat(MAX_TEXT);
        let pk = packets(&big, 8);
        let mut p = Pieces::default();
        let t = pk.iter().filter_map(|b| match super::super::read(b) {
            Some(Msg::Clip { id, part, parts, data }) => p.add(id, part, parts, data),
            _ => None,
        });
        assert_eq!(t.last().map(|s| s.len()), Some(MAX_TEXT));
    }
}
