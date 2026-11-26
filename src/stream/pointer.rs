//! Cursor local: quem assiste mostra o cursor do host com o cursor do Windows dele, que anda na hora,
//! em vez de esperar o cursor pintado no vídeo fazer a ida e volta (opção [`LOCAL_CURSOR`]).
//!
//! O host tira o cursor do vídeo e manda só o formato e se ele está à vista ([`Msg::Cursor`]); a
//! posição é a do mouse de quem assiste, que o host já segue. Com o cursor escondido no host (jogo
//! que trava o mouse), quem assiste esconde o dele também, como sem a opção.

use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::screen::{cursor_image, CursorImage};
use super::*;

/// O formato do cursor e o ponto que clica, dentro da imagem.
#[derive(Debug, PartialEq, Clone)]
pub(super) struct Shape {
    pub img: CursorImage,
    pub hot: (u16, u16),
}

const PART: usize = 1000;
const MAX_PARTS: usize = 255;
/// Cabeçalho (largura, altura, ponto, inversão) e, no máximo, 256x256 em BGRA mais a inversão.
const MAX_RAW: usize = 9 + 256 * 256 * 5;
/// Quem perdeu um pedaço, ou acabou de entrar, recebe o estado de novo a cada segundo.
const REPEAT: Duration = Duration::from_secs(1);

/// Números de formato únicos no processo: uma sessão nova do vídeo nunca repete um número antigo.
static NEXT_SHAPE: AtomicU16 = AtomicU16::new(1);

fn encode(s: &Shape) -> Vec<u8> {
    let mut raw = Vec::with_capacity(9 + s.img.color.len() * 5 / 4);
    for v in [s.img.w as u16, s.img.h as u16, s.hot.0, s.hot.1] {
        raw.extend_from_slice(&v.to_le_bytes());
    }
    raw.push(s.img.invert.is_some() as u8);
    raw.extend_from_slice(&s.img.color);
    if let Some(inv) = &s.img.invert {
        raw.extend(inv.chunks_exact(4).map(|p| p[0]));
    }
    miniz_oxide::deflate::compress_to_vec(&raw, 6)
}

fn decode(b: &[u8]) -> Option<Shape> {
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(b, MAX_RAW).ok()?;
    let u = |i: usize| raw.get(i..i + 2).map(|x| u16::from_le_bytes([x[0], x[1]]));
    let (w, h, hx, hy) = (u(0)?, u(2)?, u(4)?, u(6)?);
    if w == 0 || h == 0 || w > 256 || h > 256 {
        return None;
    }
    let n = w as usize * h as usize;
    let inverts = *raw.get(8)? != 0;
    if raw.len() != 9 + n * 4 + if inverts { n } else { 0 } {
        return None;
    }
    let color = raw[9..9 + n * 4].to_vec();
    let invert = inverts.then(|| raw[9 + n * 4..].iter().flat_map(|&v| [v; 4]).collect());
    Some(Shape { img: CursorImage { w: w as u32, h: h as u32, color, invert }, hot: (hx.min(w - 1), hy.min(h - 1)) })
}

/// Os pacotes de um estado: escondido (um pacote sem pedaços) ou à vista com o formato em pedaços.
fn packets(shape: u16, blob: Option<&[u8]>) -> Vec<Vec<u8>> {
    let pieces: Vec<&[u8]> = blob.map_or(Vec::new(), |b| b.chunks(PART).collect());
    let parts = pieces.len() as u8;
    let one = |part: u8, data: &[u8]| {
        let mut out = Vec::with_capacity(data.len() + 16);
        write(&mut out, &Msg::Cursor { shape, visible: blob.is_some(), part, parts, data });
        out
    };
    if pieces.is_empty() {
        return vec![one(0, &[])];
    }
    pieces.iter().enumerate().map(|(i, d)| one(i as u8, d)).collect()
}

/// O cursor do host, do lado de quem transmite.
#[derive(Default)]
pub(super) struct Sender {
    shape: u16,
    /// O formato atual, comprimido.
    blob: Option<Vec<u8>>,
    visible: bool,
    /// WGC: o último cursor do Windows lido.
    handle: isize,
    /// Desktop Duplication: quantos formatos ela já tinha dado na última leitura.
    pub seen: u32,
    sent: Option<(Instant, bool)>,
    dirty: bool,
}

impl Sender {
    /// Formato novo, da Desktop Duplication ou do Windows.
    pub fn set_shape(&mut self, s: &Shape) {
        let b = encode(s);
        // shortcut: formato que não cabe em 255 pedaços (256x256 sem nada transparente) é ignorado e
        // fica o anterior; trocar por um formato reduzido se aparecer cursor assim.
        if b.len() > PART * MAX_PARTS || self.blob.as_ref() == Some(&b) {
            return;
        }
        self.shape = NEXT_SHAPE.fetch_add(1, Ordering::Relaxed);
        self.blob = Some(b);
        self.dirty = true;
    }

    pub fn set_visible(&mut self, v: bool) {
        self.visible = v;
    }

    /// WGC: lê o cursor do Windows (a Desktop Duplication já entrega o dela).
    pub fn read_system(&mut self) {
        let cur = system_cursor();
        if let Some(c) = cur.filter(|c| c.0 as isize != self.handle) {
            self.handle = c.0 as isize;
            // shortcut: cursor animado (o de espera) vai só no quadro que o Windows dá aqui; ler os
            // quadros pede uma função não documentada.
            if let Some(s) = unsafe { shape_of(c) } {
                self.set_shape(&s);
            }
        }
        self.visible = cur.is_some();
    }

    /// Os pacotes a mandar agora: na mudança (duas vezes, contra perda) e a cada segundo. `on`
    /// falso: o cursor está no vídeo, e quem pediu o local esconde o dele.
    pub fn due(&mut self, on: bool) -> Option<Vec<Vec<u8>>> {
        let visible = on && self.visible && self.blob.is_some();
        let changed = std::mem::take(&mut self.dirty) || self.sent.is_none_or(|(_, v)| v != visible);
        let now = Instant::now();
        if !changed && self.sent.is_some_and(|(t, _)| now - t < REPEAT) {
            return None;
        }
        self.sent = Some((now, visible));
        let p = packets(self.shape, if visible { self.blob.as_deref() } else { None });
        Some(if changed { p.iter().chain(p.iter()).cloned().collect() } else { p })
    }
}

/// O cursor do Windows, se está à vista (o jogo não escondeu).
fn system_cursor() -> Option<HCURSOR> {
    let mut ci = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
    let ok = unsafe { GetCursorInfo(&mut ci) }.is_ok() && ci.flags.0 & CURSOR_SHOWING.0 != 0 && !ci.hCursor.is_invalid();
    ok.then_some(ci.hCursor)
}

pub(super) fn showing() -> bool {
    system_cursor().is_some()
}

/// Escondido e preso por este tempo antes do mouse relativo: um esconder rápido não troca o modo.
const GRAB_WAIT: Duration = Duration::from_millis(150);

/// O modo do mouse para quem pediu o relativo: relativo quando o jogo escondeu e prendeu o
/// cursor. Manda na mudança (duas vezes, contra perda) e a cada segundo, para quem entrou depois.
#[derive(Default)]
pub(super) struct Grab {
    since: Option<Instant>,
    sent: Option<Instant>,
    on: bool,
}

impl Grab {
    /// `locked`: o cursor está escondido e preso agora. Devolve o modo e quantas vezes mandar.
    pub fn due(&mut self, locked: bool, now: Instant) -> Option<(bool, usize)> {
        self.since = if locked { self.since.or(Some(now)) } else { None };
        let on = self.since.is_some_and(|t| now - t >= GRAB_WAIT);
        if on != self.on || self.sent.is_none() {
            self.on = on;
            self.sent = Some(now);
            return Some((on, 2));
        }
        if self.sent.is_some_and(|t| now - t >= REPEAT) {
            self.sent = Some(now);
            return Some((on, 1));
        }
        None
    }
}

/// O cursor do host está preso numa área (`ClipCursor`) menor que a área de trabalho inteira.
/// shortcut: o Windows também prende durante o arraste de janela, e o cursor volta ao vídeo nesse
/// tempo; trocar por um teste do processo em primeiro plano se isso incomodar.
pub(super) fn clipped() -> bool {
    let mut r = windows::Win32::Foundation::RECT::default();
    if unsafe { GetClipCursor(&mut r) }.is_err() {
        return false;
    }
    let (x, y) = unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN)) };
    let (w, h) = unsafe { (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
    r.left > x || r.top > y || r.right < x + w || r.bottom < y + h
}

/// A janela da frente cobre o monitor inteiro (jogo em tela cheia ou sem borda). Com um monitor só,
/// o jogo que prende o cursor na própria janela prende na tela toda, e `clipped` não vê.
pub(super) fn front_fullscreen() -> bool {
    unsafe {
        let h = GetForegroundWindow();
        let mut r = windows::Win32::Foundation::RECT::default();
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if h.is_invalid() || GetWindowRect(h, &mut r).is_err() || h == GetShellWindow() {
            return false;
        }
        if !GetMonitorInfoW(MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST), &mut mi).as_bool() {
            return false;
        }
        let m = mi.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

/// Junta o cursor do lado de quem assiste.
#[derive(Default)]
pub(super) struct Receiver {
    pieces: super::clip::Pieces,
    shape: Option<u16>,
    visible: bool,
}

impl Receiver {
    /// Um pacote do cursor. Devolve o que mudou: se ele está à vista e, quando chegou inteiro, o
    /// formato novo.
    pub fn on(&mut self, shape: u16, visible: bool, part: u8, parts: u8, data: &[u8]) -> Option<(bool, Option<Shape>)> {
        if !visible {
            return std::mem::take(&mut self.visible).then_some((false, None));
        }
        if let Some(s) = self.pieces.add_bytes(shape, part, parts, data, MAX_PARTS).and_then(|b| decode(&b)) {
            self.shape = Some(shape);
            self.visible = true;
            return Some((true, Some(s)));
        }
        (self.shape == Some(shape) && !std::mem::replace(&mut self.visible, true)).then_some((true, None))
    }
}

/// A imagem de um cursor do Windows, no formato da Desktop Duplication.
unsafe fn shape_of(c: HCURSOR) -> Option<Shape> {
    let mut ii = ICONINFO::default();
    GetIconInfo(HICON(c.0), &mut ii).ok()?;
    let img = if ii.hbmColor.is_invalid() {
        // Monocromático: máscara E em cima, OU-exclusivo embaixo.
        bits(ii.hbmMask, 1).and_then(|(w, h, p, b)| cursor_image(1, w, h, p, &b))
    } else {
        bits(ii.hbmColor, 32).and_then(|(w, h, p, mut px)| {
            if px.chunks_exact(4).any(|q| q[3] != 0) {
                return cursor_image(2, w, h, p, &px);
            }
            // Sem alfa: a máscara diz onde a cor se combina com a tela (alfa 255, como o tipo 4).
            let (_, _, mp, m) = bits(ii.hbmMask, 1)?;
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let on = m.get(y * mp as usize + x / 8).is_some_and(|b| b >> (7 - x % 8) & 1 != 0);
                    px[y * p as usize + x * 4 + 3] = if on { 255 } else { 0 };
                }
            }
            cursor_image(4, w, h, p, &px)
        })
    };
    if !ii.hbmColor.is_invalid() {
        let _ = DeleteObject(ii.hbmColor.into());
    }
    let _ = DeleteObject(ii.hbmMask.into());
    let img = img?;
    Some(Shape { hot: ((ii.xHotspot as u16).min(img.w as u16 - 1), (ii.yHotspot as u16).min(img.h as u16 - 1)), img })
}

/// BITMAPINFO com lugar para a paleta de 2 cores que o GetDIBits escreve no 1 bit por pixel.
#[repr(C)]
#[derive(Default)]
struct Bmi {
    head: BITMAPINFOHEADER,
    colors: [RGBQUAD; 2],
}

/// Os pixels de um bitmap, de cima para baixo: largura, altura, bytes por linha e os bytes.
unsafe fn bits(bm: HBITMAP, bpp: u16) -> Option<(u32, u32, u32, Vec<u8>)> {
    let mut b = BITMAP::default();
    if GetObjectW(bm.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut b as *mut _ as *mut _)) == 0 {
        return None;
    }
    let (w, h) = (b.bmWidth.max(0) as u32, b.bmHeight.max(0) as u32);
    if w == 0 || h == 0 || w > 256 || h > 512 {
        return None;
    }
    let pitch = (w * bpp as u32).div_ceil(32) * 4;
    let mut buf = vec![0u8; (pitch * h) as usize];
    let mut bi = Bmi { head: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w as i32, biHeight: -(h as i32), biPlanes: 1, biBitCount: bpp, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
    let dc = GetDC(None);
    let n = GetDIBits(dc, bm, 0, h, Some(buf.as_mut_ptr() as *mut _), &mut bi as *mut Bmi as *mut BITMAPINFO, DIB_RGB_COLORS);
    ReleaseDC(None, dc);
    (n == h as i32).then_some((w, h, pitch, buf))
}

/// O cursor do Windows deste PC com o formato do host. Sem inversão, a cor com transparência; com
/// inversão, as máscaras do Windows (o cursor de texto continua invertendo o que está atrás).
pub(super) unsafe fn to_cursor(s: &Shape) -> Option<HCURSOR> {
    let (w, h) = (s.img.w as usize, s.img.h as usize);
    let bi = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w as i32, biHeight: -(h as i32), biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
    let mut ptr = std::ptr::null_mut();
    let color = CreateDIBSection(None, &bi, DIB_RGB_COLORS, &mut ptr, None, 0).ok()?;
    let px = std::slice::from_raw_parts_mut(ptr as *mut u8, w * h * 4);
    // Máscara E, linhas de 16 bits: 1 deixa a tela (e, com cor, a inverte).
    let pitch = w.div_ceil(16) * 2;
    let mut and = vec![0u8; pitch * h];
    match &s.img.invert {
        None => px.copy_from_slice(&s.img.color),
        Some(inv) => {
            // Alfa todo zero: o Windows usa as máscaras.
            for i in 0..w * h {
                let c = &s.img.color[i * 4..i * 4 + 4];
                let (rgb, keep) = if inv[i * 4] != 0 { ([255; 3], true) } else if c[3] >= 128 { ([c[0], c[1], c[2]], false) } else { ([0; 3], true) };
                px[i * 4..i * 4 + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 0]);
                if keep {
                    and[(i / w) * pitch + (i % w) / 8] |= 0x80 >> (i % w % 8);
                }
            }
        }
    }
    let mask = CreateBitmap(w as i32, h as i32, 1, 1, Some(and.as_ptr() as *const _));
    let ii = ICONINFO { fIcon: false.into(), xHotspot: s.hot.0 as u32, yHotspot: s.hot.1 as u32, hbmMask: mask, hbmColor: color };
    let c = CreateIconIndirect(&ii);
    let _ = DeleteObject(color.into());
    let _ = DeleteObject(mask.into());
    c.ok().map(|i| HCURSOR(i.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(invert: bool) -> Shape {
        let (w, h) = (3u32, 2u32);
        let color: Vec<u8> = (0..w * h * 4).map(|i| i as u8).collect();
        let inv = invert.then(|| (0..w * h).flat_map(|i| [if i == 4 { 255 } else { 0 }; 4]).collect());
        Shape { img: CursorImage { w, h, color, invert: inv }, hot: (2, 1) }
    }

    #[test]
    fn relativo_so_depois_de_preso_um_tempo() {
        let t = Instant::now();
        let mut g = Grab::default();
        assert_eq!(g.due(false, t), Some((false, 2)));
        assert_eq!(g.due(true, t), None);
        assert_eq!(g.due(true, t + GRAB_WAIT), Some((true, 2)));
        assert_eq!(g.due(true, t + GRAB_WAIT * 2), None);
        assert_eq!(g.due(true, t + GRAB_WAIT + REPEAT), Some((true, 1)));
        // Soltou: volta na hora; um esconder curto não liga de novo.
        assert_eq!(g.due(false, t + REPEAT * 2), Some((false, 2)));
        assert_eq!(g.due(true, t + REPEAT * 2), None);
        assert_eq!(g.due(false, t + REPEAT * 2 + GRAB_WAIT / 2), None);
    }

    #[test]
    fn formato_vai_e_volta() {
        for invert in [false, true] {
            let s = shape(invert);
            assert_eq!(decode(&encode(&s)), Some(s));
        }
        assert_eq!(decode(b"lixo"), None);
        // Tamanho que não bate com o cabeçalho.
        let mut raw = encode(&shape(false));
        raw.truncate(raw.len() / 2);
        assert_eq!(decode(&raw), None);
    }

    #[test]
    fn quem_assiste_junta_esconde_e_ignora_repetido() {
        let mut tx = Sender::default();
        let s = shape(true);
        tx.set_shape(&s);
        tx.set_visible(true);
        // Fora do vídeo: o estado novo vai duas vezes; logo depois, nada.
        let pk = tx.due(true).unwrap();
        assert!(tx.due(true).is_none());
        let mut rx = Receiver::default();
        let mut got = Vec::new();
        for p in pk.iter().rev() {
            if let Some(Msg::Cursor { shape, visible, part, parts, data }) = read(p) {
                got.extend(rx.on(shape, visible, part, parts, data));
            }
        }
        assert_eq!(got, vec![(true, Some(s.clone()))], "um formato só, mesmo com o pacote repetido");
        // O cursor volta ao vídeo: quem assiste esconde o dele.
        let pk = tx.due(false).unwrap();
        let Some(Msg::Cursor { shape, visible, part, parts, data }) = read(&pk[0]) else { panic!() };
        assert_eq!(rx.on(shape, visible, part, parts, data), Some((false, None)));
        assert_eq!(rx.on(shape, visible, part, parts, data), None);
        // E de volta à vista com o mesmo formato: só a visibilidade muda.
        let pk = tx.due(true).unwrap();
        let Some(Msg::Cursor { shape, visible, part, parts, data }) = read(&pk[0]) else { panic!() };
        assert_eq!(rx.on(shape, visible, part, parts, data), Some((true, None)));
    }

    #[test]
    fn cursor_de_verdade_vira_formato_e_volta() {
        unsafe {
            let arrow = LoadCursorW(None, IDC_ARROW).unwrap();
            let s = shape_of(arrow).expect("a seta do Windows");
            assert!(s.img.w >= 16 && s.img.h >= 16);
            let ibeam = shape_of(LoadCursorW(None, IDC_IBEAM).unwrap()).expect("o cursor de texto");
            for x in [&s, &ibeam] {
                let c = to_cursor(x).expect("cursor daqui");
                let _ = DestroyCursor(c);
            }
        }
    }
}
