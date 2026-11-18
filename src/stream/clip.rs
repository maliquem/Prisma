//! Área de transferência nos dois sentidos (fase 7). Cada lado vigia a própria área de
//! transferência. O texto novo vai em pedaços, até 64 KB; quem recebe junta e grava.
//!
//! Imagem e arquivos (até 64 MB) vão pelo modelo de puxar: quem copiou oferece o tamanho
//! ([`Msg::ClipOffer`]), quem recebe pede faixas de pedaços ([`Msg::ClipWant`]) e pede de novo o
//! que se perdeu. Uma versão anterior ignora a oferta. Os arquivos chegam numa pasta temporária e
//! vão para a área de transferência como arquivos copiados, prontos para colar no Explorer.
//!
//! Só vale para quem tem a permissão `COPY`.

use std::hash::{Hash, Hasher};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::*;
use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;

use super::link::Link;
use super::{write, Msg};

/// Tamanho máximo do texto, em bytes UTF-8.
pub const MAX_TEXT: usize = 64 * 1024;
/// Bytes de texto por pacote: cabe no MTU com o cabeçalho.
const PART: usize = 1000;
const CF_UNICODETEXT: u32 = 13;
const CF_DIB: u32 = 8;
const CF_HDROP: u32 = 15;

/// Imagem ou arquivos: até 64 MB por vez, e até 2000 arquivos.
pub const MAX_RICH: usize = 64 << 20;
const MAX_FILES: usize = 2000;
/// Primeiro byte do conteúdo rico.
const IMAGE: u8 = 1;
const FILES: u8 = 2;
/// Bytes por pedaço do conteúdo rico (cabe no MTU com o cabeçalho de 10 bytes).
const PIECE: usize = 1000;
/// Pedaços por pedido, e quantos podem estar pedidos sem ter chegado.
const ASK: u32 = 256;
const AHEAD: u32 = 512;
/// De quanto em quanto tempo os pedaços perdidos são pedidos de novo, e quando desistir.
const RETRY: Duration = Duration::from_millis(250);
const GIVE_UP: Duration = Duration::from_secs(10);

/// A vigia da área de transferência deste PC, numa thread com janela própria.
pub struct Clip {
    /// Texto que chegou de longe, para gravar aqui.
    pub set: mpsc::Sender<String>,
    /// Texto copiado aqui, para mandar.
    pub changed: mpsc::Receiver<String>,
    /// Imagem ou arquivos que chegaram de longe (já juntos), para gravar aqui.
    pub set_rich: mpsc::Sender<Vec<u8>>,
    /// Imagem ou arquivos copiados aqui, para oferecer.
    pub rich: mpsc::Receiver<Arc<Vec<u8>>>,
}

fn digest(b: &[u8]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.hash(&mut h);
    h.finish()
}

pub fn start() -> Clip {
    let (set_tx, set_rx) = mpsc::channel::<String>();
    let (ch_tx, ch_rx) = mpsc::channel();
    let (rich_set_tx, rich_set_rx) = mpsc::channel::<Vec<u8>>();
    let (rich_tx, rich_rx) = mpsc::channel();
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
        // O mesmo para imagem e arquivos, pelo resumo do conteúdo.
        let mut last_rich = 0u64;
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
            while let Ok(b) = rich_set_rx.try_recv() {
                last_rich = digest(&b);
                if put_rich(h, &b) {
                    seq = GetClipboardSequenceNumber();
                }
            }
            let now = GetClipboardSequenceNumber();
            if now != seq {
                seq = now;
                // Arquivos primeiro; imagem só quando não há texto (uma planilha copia os dois).
                let text = IsClipboardFormatAvailable(CF_UNICODETEXT).is_ok();
                let rich = files(h).or_else(|| if text { None } else { get_bytes(h, CF_DIB).map(|d| [&[IMAGE][..], &d].concat()) });
                if let Some(b) = rich {
                    let k = digest(&b);
                    if k != last_rich {
                        last_rich = k;
                        if rich_tx.send(Arc::new(b)).is_err() {
                            break;
                        }
                    }
                } else if let Some(t) = get(h).filter(|t| *t != last) {
                    last = t.clone();
                    if ch_tx.send(t).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = DestroyWindow(h);
    });
    Clip { set: set_tx, changed: ch_rx, set_rich: rich_set_tx, rich: rich_rx }
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
    let bytes: Vec<u8> = text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    put_bytes(h, CF_UNICODETEXT, &bytes)
}

unsafe fn put_bytes(h: HWND, format: u32, bytes: &[u8]) -> bool {
    let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes.len()) else { return false };
    let p = GlobalLock(mem) as *mut u8;
    if p.is_null() {
        let _ = GlobalFree(Some(mem));
        return false;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    let _ = GlobalUnlock(mem);
    if !open(h) {
        let _ = GlobalFree(Some(mem));
        return false;
    }
    let _ = EmptyClipboard();
    let ok = SetClipboardData(format, Some(HANDLE(mem.0))).is_ok();
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

/// Os bytes de um formato da área de transferência, até [`MAX_RICH`].
unsafe fn get_bytes(h: HWND, format: u32) -> Option<Vec<u8>> {
    if IsClipboardFormatAvailable(format).is_err() || !open(h) {
        return None;
    }
    let out = (|| {
        let d = GetClipboardData(format).ok()?;
        let n = GlobalSize(HGLOBAL(d.0));
        if n == 0 || n > MAX_RICH {
            return None;
        }
        let p = GlobalLock(HGLOBAL(d.0)) as *const u8;
        if p.is_null() {
            return None;
        }
        let v = std::slice::from_raw_parts(p, n).to_vec();
        let _ = GlobalUnlock(HGLOBAL(d.0));
        Some(v)
    })();
    let _ = CloseClipboard();
    out
}

/// Arquivos e pastas copiados no Explorer, já lidos e juntos (ou nada, se passam do limite).
unsafe fn files(h: HWND) -> Option<Vec<u8>> {
    if IsClipboardFormatAvailable(CF_HDROP).is_err() || !open(h) {
        return None;
    }
    let paths: Vec<PathBuf> = (|| {
        let d = HDROP(GetClipboardData(CF_HDROP).ok()?.0);
        let n = DragQueryFileW(d, u32::MAX, None);
        Some(
            (0..n)
                .map(|i| {
                    let mut w = vec![0u16; DragQueryFileW(d, i, None) as usize + 1];
                    let len = DragQueryFileW(d, i, Some(&mut w)) as usize;
                    PathBuf::from(std::ffi::OsString::from_wide(&w[..len]))
                })
                .collect(),
        )
    })()
    .unwrap_or_default();
    let _ = CloseClipboard();
    let out = pack_files(&paths);
    if out.is_none() && !paths.is_empty() {
        eprintln!("área de transferência: arquivos copiados passam de 64 MB ou de {MAX_FILES} arquivos, ficam aqui");
    }
    out
}

/// Junta os arquivos (e o que há dentro das pastas) com o caminho relativo à pasta de cada um.
/// Nada se passar de [`MAX_RICH`] ou [`MAX_FILES`]; o tamanho é olhado antes de ler.
pub fn pack_files(paths: &[PathBuf]) -> Option<Vec<u8>> {
    fn walk(p: &Path, base: &Path, list: &mut Vec<(String, PathBuf)>, total: &mut u64) -> Option<()> {
        let m = std::fs::metadata(p).ok()?;
        if m.is_dir() {
            for e in std::fs::read_dir(p).ok()? {
                walk(&e.ok()?.path(), base, list, total)?;
            }
        } else {
            *total += m.len();
            list.push((p.strip_prefix(base).ok()?.to_str()?.to_string(), p.to_path_buf()));
        }
        (list.len() <= MAX_FILES && *total <= MAX_RICH as u64).then_some(())
    }
    let (mut list, mut total) = (Vec::new(), 0);
    for p in paths {
        walk(p, p.parent()?, &mut list, &mut total)?;
    }
    if list.is_empty() {
        return None;
    }
    let mut out = vec![FILES];
    for (name, path) in list {
        let data = std::fs::read(path).ok()?;
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&data);
    }
    (out.len() <= MAX_RICH).then_some(out)
}

/// Um caminho relativo que fica dentro da pasta: nada de `..`, unidade, raiz ou nome inválido.
fn safe(name: &str) -> Option<PathBuf> {
    let mut p = PathBuf::new();
    for c in name.split(['\\', '/']) {
        let bad = c.is_empty() || c == "." || c == ".." || c.ends_with([' ', '.']) || c.chars().any(|ch| ch < ' ' || ":*?\"<>|".contains(ch));
        if bad {
            return None;
        }
        p.push(c);
    }
    (!p.as_os_str().is_empty()).then_some(p)
}

/// Separa os arquivos do conteúdo; um nome perigoso ou um tamanho que não fecha recusa tudo.
pub fn unpack_files(b: &[u8]) -> Option<Vec<(PathBuf, &[u8])>> {
    let mut rest = b.strip_prefix(&[FILES])?;
    let mut out = Vec::new();
    while !rest.is_empty() {
        let n = u16::from_le_bytes(rest.get(..2)?.try_into().ok()?) as usize;
        let name = safe(std::str::from_utf8(rest.get(2..2 + n)?).ok()?)?;
        rest = &rest[2 + n..];
        let len = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
        out.push((name, rest.get(4..4 + len)?));
        rest = &rest[4 + len..];
        if out.len() > MAX_FILES {
            return None;
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Grava a imagem ou os arquivos que chegaram. Os arquivos vão para `%TEMP%\Prisma\Colado`, que
/// é esvaziada a cada vez (o que ficou lá já foi colado ou saiu da área de transferência).
unsafe fn put_rich(h: HWND, b: &[u8]) -> bool {
    match b.first() {
        Some(&IMAGE) => put_bytes(h, CF_DIB, &b[1..]),
        Some(&FILES) => {
            let Some(list) = unpack_files(b) else { return false };
            let root = std::env::temp_dir().join("Prisma").join("Colado");
            let _ = std::fs::remove_dir_all(&root);
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
            let dir = root.join(stamp.to_string());
            let mut top: Vec<PathBuf> = Vec::new();
            for (name, data) in list {
                let path = dir.join(&name);
                let ok = path.parent().is_some_and(|p| std::fs::create_dir_all(p).is_ok()) && std::fs::write(&path, data).is_ok();
                if !ok {
                    eprintln!("área de transferência: não deu para gravar {}", path.display());
                    return false;
                }
                let first = dir.join(name.components().next().map(|c| c.as_os_str()).unwrap_or_default());
                if !top.contains(&first) {
                    top.push(first);
                }
            }
            // DROPFILES: pFiles = 20, ponto (x, y), fNC e fWide = 1; depois os caminhos e um zero a mais.
            let mut drop: Vec<u8> = [20u32, 0, 0, 0, 1].iter().flat_map(|v| v.to_le_bytes()).collect();
            for p in &top {
                drop.extend(p.as_os_str().encode_wide().chain([0]).flat_map(u16::to_le_bytes));
            }
            drop.extend([0, 0]);
            put_bytes(h, CF_HDROP, &drop)
        }
        _ => false,
    }
}

/// Imagem ou arquivos com um PC: o que oferecemos a ele e o que estamos buscando dele.
#[derive(Default)]
pub struct Rich {
    out: Option<(u16, Arc<Vec<u8>>)>,
    inc: Option<Incoming>,
    done: Option<u16>,
}

struct Incoming {
    id: u16,
    data: Vec<u8>,
    got: Vec<bool>,
    have: u32,
    /// Pedaços já pedidos uma vez: `0..asked`.
    asked: u32,
    /// Até onde estava pedido na volta de reenvio anterior: o que falta abaixo disso se perdeu.
    mark: u32,
    heard: Instant,
    retry: Instant,
}

impl Incoming {
    fn ask(&mut self, link: &dyn Link) {
        let n = self.got.len() as u32;
        while self.asked < n && self.asked.saturating_sub(self.have) < AHEAD {
            let count = ASK.min(n - self.asked);
            send(link, &Msg::ClipWant { id: self.id, start: self.asked, count: count as u16 });
            self.asked += count;
        }
    }
}

fn send(link: &dyn Link, m: &Msg) {
    let mut b = Vec::with_capacity(PIECE + 16);
    write(&mut b, m);
    link.send(&b);
}

impl Rich {
    /// Oferece um conteúdo novo (duas vezes, contra perda). O anterior deixa de ser servido.
    pub fn offer(&mut self, id: u16, blob: Arc<Vec<u8>>, link: &dyn Link) {
        let m = Msg::ClipOffer { id, size: blob.len() as u32 };
        send(link, &m);
        send(link, &m);
        self.out = Some((id, blob));
    }

    /// Um pacote de imagem ou arquivos chegou. Devolve o conteúdo quando ele fica completo.
    pub fn on(&mut self, m: &Msg, link: &dyn Link) -> Option<Vec<u8>> {
        match *m {
            Msg::ClipOffer { id, size } => {
                if size == 0 || size as usize > MAX_RICH || self.done == Some(id) || self.inc.as_ref().is_some_and(|i| i.id == id) {
                    return None;
                }
                let now = Instant::now();
                let n = (size as usize).div_ceil(PIECE);
                let mut inc = Incoming { id, data: vec![0; size as usize], got: vec![false; n], have: 0, asked: 0, mark: 0, heard: now, retry: now };
                inc.ask(link);
                self.inc = Some(inc);
                None
            }
            Msg::ClipWant { id, start, count } => {
                let (oid, blob) = self.out.as_ref()?;
                if *oid != id {
                    return None;
                }
                for idx in start..start.saturating_add(count.min(ASK as u16) as u32) {
                    let s = idx as usize * PIECE;
                    let Some(data) = blob.get(s..(s + PIECE).min(blob.len())).filter(|d| !d.is_empty()) else { break };
                    send(link, &Msg::ClipData { id, idx, data });
                }
                None
            }
            Msg::ClipData { id, idx, data } => {
                let inc = self.inc.as_mut().filter(|i| i.id == id)?;
                let s = idx as usize * PIECE;
                if inc.got.get(idx as usize) != Some(&false) || data.len() != PIECE.min(inc.data.len() - s) {
                    return None;
                }
                inc.data[s..s + data.len()].copy_from_slice(data);
                inc.got[idx as usize] = true;
                inc.have += 1;
                inc.heard = Instant::now();
                if inc.have as usize == inc.got.len() {
                    self.done = Some(id);
                    return self.inc.take().map(|i| i.data);
                }
                inc.ask(link);
                None
            }
            _ => None,
        }
    }

    /// Pede de novo o que se perdeu e desiste de quem parou de mandar. Chame a cada volta do laço.
    pub fn tick(&mut self, link: &dyn Link, now: Instant) {
        let Some(inc) = &mut self.inc else { return };
        if now.saturating_duration_since(inc.heard) > GIVE_UP {
            eprintln!("área de transferência: a imagem ou os arquivos pararam de chegar");
            self.inc = None;
            return;
        }
        if now.saturating_duration_since(inc.retry) < RETRY {
            return;
        }
        inc.retry = now;
        let (mut i, mut budget) = (0, AHEAD);
        while i < inc.mark && budget > 0 {
            if inc.got[i as usize] {
                i += 1;
                continue;
            }
            let mut end = i;
            while end < inc.mark && !inc.got[end as usize] && end - i < ASK.min(budget) {
                end += 1;
            }
            send(link, &Msg::ClipWant { id: inc.id, start: i, count: (end - i) as u16 });
            budget -= end - i;
            i = end;
        }
        inc.mark = inc.asked;
        inc.ask(link);
    }
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

    /// Um lado da conexão de mentira: guarda o que foi mandado.
    #[derive(Default)]
    struct Wire(std::sync::Mutex<Vec<Vec<u8>>>);
    impl Link for Wire {
        fn send(&self, b: &[u8]) {
            self.0.lock().unwrap().push(b.to_vec());
        }
        fn recv(&self, _: &mut [u8], _: Duration) -> super::super::link::Poll {
            super::super::link::Poll::Empty
        }
    }

    #[test]
    fn rich_content_arrives_through_loss() {
        let blob: Vec<u8> = (0..300_500u32).map(|i| (i * 7 % 251) as u8).collect();
        let (mut a, mut b) = (Rich::default(), Rich::default());
        let (to_b, to_a) = (Wire::default(), Wire::default());
        a.offer(9, Arc::new(blob.clone()), &to_b);
        let (mut got, mut n, mut now) = (None, 0u32, Instant::now());
        for _ in 0..30 {
            let pk = std::mem::take(&mut *to_b.0.lock().unwrap());
            for p in pk {
                n += 1;
                // Um em cada sete se perde no caminho.
                if n % 7 == 0 {
                    continue;
                }
                if let Some(d) = b.on(&super::super::read(&p).unwrap(), &to_a) {
                    assert!(got.is_none(), "entregou duas vezes");
                    got = Some(d);
                }
            }
            for p in std::mem::take(&mut *to_a.0.lock().unwrap()) {
                a.on(&super::super::read(&p).unwrap(), &to_b);
            }
            if got.is_some() {
                break;
            }
            now += RETRY;
            b.tick(&to_a, now);
        }
        assert_eq!(got.as_deref(), Some(&blob[..]));
        // A oferta repetida não recomeça.
        b.on(&Msg::ClipOffer { id: 9, size: blob.len() as u32 }, &to_a);
        assert!(to_a.0.lock().unwrap().is_empty());
        // Pedaço de quem não foi oferecido, ou fora do tamanho: ignorado.
        assert!(b.on(&Msg::ClipData { id: 5, idx: 0, data: &[1] }, &to_a).is_none());
    }

    #[test]
    fn files_pack_and_unpack_safely() {
        let dir = std::env::temp_dir().join(format!("prisma-clip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("pasta").join("sub")).unwrap();
        std::fs::write(dir.join("um.txt"), b"ola").unwrap();
        std::fs::write(dir.join("pasta").join("sub").join("dois.bin"), [0u8, 1, 2, 255]).unwrap();
        let b = pack_files(&[dir.join("um.txt"), dir.join("pasta")]).unwrap();
        let mut list: Vec<(String, Vec<u8>)> = unpack_files(&b).unwrap().into_iter().map(|(p, d)| (p.to_str().unwrap().to_string(), d.to_vec())).collect();
        list.sort();
        assert_eq!(list, vec![("pasta\\sub\\dois.bin".to_string(), vec![0, 1, 2, 255]), ("um.txt".to_string(), b"ola".to_vec())]);
        let _ = std::fs::remove_dir_all(&dir);
        for bad in ["..\\x", "C:\\x", "\\x", "a\\..\\..\\x", "con:", "a\\b.", ""] {
            assert!(safe(bad).is_none(), "{bad}");
        }
        // Tamanho que passa do fim: nada.
        assert!(unpack_files(&[FILES, 1, 0, b'a', 9, 0, 0, 0, 1]).is_none());
    }
}
