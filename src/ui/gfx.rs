//! Desenho com Direct2D e DirectWrite. Serve à janela principal, ao widget e aos avisos.

use std::cell::RefCell;
use std::collections::HashMap;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

pub const fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

impl Rect {
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn inset(&self, d: f32) -> Rect {
        rect(self.x + d, self.y + d, (self.w - 2.0 * d).max(0.0), (self.h - 2.0 * d).max(0.0))
    }
    pub fn inset_xy(&self, dx: f32, dy: f32) -> Rect {
        rect(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
    fn d2d(&self) -> D2D_RECT_F {
        D2D_RECT_F { left: self.x, top: self.y, right: self.right(), bottom: self.bottom() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

pub const fn rgb(v: u32) -> Color {
    Color {
        r: ((v >> 16) & 255) as f32 / 255.0,
        g: ((v >> 8) & 255) as f32 / 255.0,
        b: (v & 255) as f32 / 255.0,
        a: 1.0,
    }
}

impl Color {
    pub const fn alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }
}

pub const BG: Color = rgb(0x17191d);
pub const SIDE: Color = rgb(0x1b1d22);
pub const CARD: Color = rgb(0x24272e);
pub const CARD2: Color = rgb(0x2c3038);
pub const LINE: Color = rgb(0x343843);
pub const BORDER: Color = rgb(0x3b404b);
pub const BORDER2: Color = rgb(0x5a6170);
pub const TEXT: Color = rgb(0xe8eaee);
pub const TEXT2: Color = rgb(0xc3c7cf);
pub const MUTED: Color = rgb(0xa4a9b4);
pub const ACCENT: Color = rgb(0x7cc4ff);
pub const ON_ACCENT: Color = rgb(0x0e1a24);
pub const WARN: Color = rgb(0xffb26b);
pub const REC: Color = rgb(0xff5a52);
pub const WHITE: Color = rgb(0xffffff);

#[derive(Clone, Copy, PartialEq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Weight {
    Regular,
    Semibold,
}

struct Shared {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    fmts: HashMap<(u32, Weight, bool), IDWriteTextFormat>,
    ellipsis: Option<IDWriteInlineObject>,
}

thread_local! {
    static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

fn shared<T>(f: impl FnOnce(&mut Shared) -> T) -> T {
    SHARED.with(|s| {
        let mut s = s.borrow_mut();
        if s.is_none() {
            unsafe {
                let d2d: ID2D1Factory =
                    D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).expect("Direct2D");
                let dw: IDWriteFactory =
                    DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).expect("DirectWrite");
                *s = Some(Shared { d2d, dw, fmts: HashMap::new(), ellipsis: None });
            }
        }
        f(s.as_mut().unwrap())
    })
}

/// Matiz (0 a 360), saturação e brilho (0 a 1) em R, G e B de 0 a 255.
pub fn hsv_rgb(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [r, g, b].map(|v| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// O contrário: R, G e B de 0 a 255 em matiz, saturação e brilho.
pub fn rgb_hsv(c: [u8; 3]) -> [f32; 3] {
    let [r, g, b] = c.map(|v| v as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    [h.rem_euclid(360.0), if max == 0.0 { 0.0 } else { d / max }, max]
}

pub fn factory() -> ID2D1Factory {
    shared(|s| s.d2d.clone())
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn format(size: f32, weight: Weight, wrap: bool) -> IDWriteTextFormat {
    shared(|s| {
        let key = ((size * 10.0) as u32, weight, wrap);
        if let Some(f) = s.fmts.get(&key) {
            return f.clone();
        }
        unsafe {
            let w = match weight {
                Weight::Regular => DWRITE_FONT_WEIGHT_NORMAL,
                Weight::Semibold => DWRITE_FONT_WEIGHT_SEMI_BOLD,
            };
            let f = s
                .dw
                .CreateTextFormat(
                    w!("Segoe UI Variable Text"),
                    None,
                    w,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("pt-BR"),
                )
                .expect("formato de texto");
            if wrap {
                let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            } else {
                let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                if s.ellipsis.is_none() {
                    s.ellipsis = s.dw.CreateEllipsisTrimmingSign(&f).ok();
                }
                let trim = DWRITE_TRIMMING {
                    granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                    delimiter: 0,
                    delimiterCount: 0,
                };
                let _ = f.SetTrimming(&trim, s.ellipsis.as_ref());
            }
            s.fmts.insert(key, f.clone());
            f
        }
    })
}

/// Teste (`PRISMA_OVERFLOW`): o arquivo onde vão os textos que não cabem.
#[cfg(debug_assertions)]
fn overflow_path() -> Option<&'static str> {
    static P: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    P.get_or_init(|| std::env::var("PRISMA_OVERFLOW").ok()).as_deref()
}

#[cfg(debug_assertions)]
thread_local! {
    /// Teste: as caixas desenhadas neste quadro (a janela, cartões, botões, campos), já na posição
    /// da tela, o deslocamento em uso (rolagem ou miniatura), os recortes abertos (o recorte e
    /// quantas caixas já havia quando ele abriu: só as de dentro contam para o texto de dentro) e a
    /// escala em uso.
    static BOXES: RefCell<(Vec<Rect>, f32, f32, Vec<(Rect, usize)>, f32)> = const { RefCell::new((Vec::new(), 0.0, 0.0, Vec::new(), 1.0)) };
}

/// Teste: uma janela própria cobre a página, então as caixas de trás deixam de valer para o texto
/// desenhado em cima dela (senão o texto da janela é medido contra uma caixinha do fundo).
#[cfg(debug_assertions)]
pub fn forget_boxes() {
    if overflow_path().is_some() {
        BOXES.with(|b| b.borrow_mut().0.clear());
    }
}

#[cfg(debug_assertions)]
fn note_box(r: Rect) {
    if overflow_path().is_some() {
        BOXES.with(|b| {
            let mut b = b.borrow_mut();
            let (dx, dy, k) = (b.1, b.2, b.4);
            b.0.push(Rect { x: r.x * k + dx, y: r.y * k + dy, w: r.w * k, h: r.h * k });
        });
    }
}

/// Teste: grava o texto que não cabe no espaço onde foi desenhado, e o que passa da borda da caixa
/// em volta (a menor caixa desenhada antes que tem o meio do texto e é pelo menos da altura dele).
/// Mede o que o DirectWrite vai desenhar, sem foto.
#[cfg(debug_assertions)]
fn note_overflow(f: &IDWriteTextFormat, t: &[u16], s: &str, r: Rect, a: Align, wrap: bool) {
    use std::io::Write;
    let Some(path) = overflow_path() else { return };
    if r.w < 4.0 || s.trim().is_empty() {
        return;
    }
    let (left, w, h) = shared(|sh| unsafe {
        let Ok(l) = sh.dw.CreateTextLayout(t, f, if wrap { r.w } else { 100000.0 }, 100000.0) else { return (0.0, 0.0, 0.0) };
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = l.GetMetrics(&mut m);
        (m.left, m.width, m.height)
    });
    let mut out = vec![];
    let bad = if wrap { h > r.h + 1.0 && r.h > 0.0 } else { w > r.w + 0.5 };
    if bad {
        // Numa linha o que não cabe termina em reticências; quebrado, as linhas passam de baixo.
        let how = if wrap { "needs" } else { "reticências, needs" };
        out.push(format!("{:.0}x{:.0} at {:.0},{:.0} {how} {:.0}x{:.0}: {s}", r.w, r.h, r.x, r.y, w, h));
    }
    let w = if wrap { w } else { w.min(r.w) };
    // Onde o texto fica de fato: no quebrado o próprio layout alinha; numa linha, o alinhamento
    // pedido e o meio na vertical.
    let x = if wrap {
        r.x + left
    } else {
        match a {
            Align::Left => r.x,
            Align::Center => r.x + (r.w - w) / 2.0,
            Align::Right => r.right() - w,
        }
    };
    let y = if wrap { r.y } else { r.y + (r.h - h) / 2.0 };
    BOXES.with(|b| {
        let b = b.borrow();
        let k = b.4;
        let tb = Rect { x: x * k + b.1, y: y * k + b.2, w: w * k, h: h * k };
        let (cx, cy) = (tb.x + tb.w / 2.0, tb.y + tb.h / 2.0);
        // Rolado para fora do recorte: não aparece.
        let from = match b.3.last() {
            Some((clip, _)) if !clip.contains(cx, cy) => return,
            Some((_, n)) => *n,
            None => 0,
        };
        let inner = b.0[from..].iter().filter(|c| c.h + 1.0 >= tb.h && c.contains(cx, cy)).min_by(|p, q| (p.w * p.h).total_cmp(&(q.w * q.h)));
        if let Some(c) = inner {
            let over = [c.x - tb.x, tb.right() - c.right(), c.y - tb.y, tb.bottom() - c.bottom()];
            let worst = over.iter().copied().fold(0.0f32, f32::max);
            if worst > 1.0 {
                let side = ["esquerda", "direita", "cima", "baixo"][over.iter().position(|&v| v == worst).unwrap_or(0)];
                out.push(format!("borda {side} {worst:.0}px: caixa {:.0}x{:.0} at {:.0},{:.0}, texto {:.0}x{:.0} at {:.0},{:.0}: {s}", c.w, c.h, c.x, c.y, w, h, tb.x, tb.y));
            }
        }
    });
    if !out.is_empty() {
        if let Ok(mut o) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            for l in out {
                let _ = writeln!(o, "{l}");
            }
        }
    }
}

/// Largura de uma linha já traduzida.
pub fn measure_raw(text: &str, size: f32, weight: Weight) -> f32 {
    let f = format(size, weight, false);
    let t = wide(text);
    shared(|s| unsafe {
        let Ok(layout) = s.dw.CreateTextLayout(&t, &f, 100000.0, 100000.0) else { return 0.0 };
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = layout.GetMetrics(&mut m);
        m.width
    })
}

/// Mede o texto em DIPs. `max_w` limita a largura quando há quebra de linha.
pub fn measure(text: &str, size: f32, weight: Weight, max_w: Option<f32>) -> (f32, f32) {
    let f = format(size, weight, max_w.is_some());
    let t = wide(&crate::i18n::tr(text));
    shared(|s| unsafe {
        let layout = s
            .dw
            .CreateTextLayout(&t, &f, max_w.unwrap_or(100000.0), 100000.0)
            .expect("layout de texto");
        let mut m = DWRITE_TEXT_METRICS::default();
        let _ = layout.GetMetrics(&mut m);
        (m.widthIncludingTrailingWhitespace, m.height)
    })
}

/// Texto longo do guia: já vem no idioma certo e `**` marca negrito.
/// Fica guardado pronto, para a rolagem não refazer tudo a cada quadro.
pub fn rich_layout(text: &str, size: f32, max_w: f32) -> Option<IDWriteTextLayout> {
    let (mut plain, mut bold) = (Vec::new(), Vec::new());
    for (i, part) in text.split("**").enumerate() {
        let start = plain.len() as u32;
        plain.extend(part.encode_utf16());
        if i % 2 == 1 {
            bold.push(DWRITE_TEXT_RANGE { startPosition: start, length: plain.len() as u32 - start });
        }
    }
    let f = format(size, Weight::Regular, true);
    shared(|s| unsafe {
        let l = s.dw.CreateTextLayout(&plain, &f, max_w, 100000.0).ok()?;
        let _ = l.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
        let _ = l.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
        for r in bold {
            let _ = l.SetFontWeight(DWRITE_FONT_WEIGHT_SEMI_BOLD, r);
        }
        Some(l)
    })
}

pub fn layout_height(l: &IDWriteTextLayout) -> f32 {
    let mut m = DWRITE_TEXT_METRICS::default();
    let _ = unsafe { l.GetMetrics(&mut m) };
    m.height
}

pub struct Canvas {
    pub rt: ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
    factory: ID2D1Factory,
}

fn cc(c: Color) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a }
}

impl Canvas {
    pub fn new(rt: ID2D1RenderTarget) -> Self {
        unsafe {
            let brush = rt.CreateSolidColorBrush(&cc(WHITE), None).expect("pincel");
            rt.SetAntialiasMode(D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let factory = rt.GetFactory().expect("fábrica");
            Self { factory, rt, brush }
        }
    }

    fn b(&self, c: Color) -> &ID2D1SolidColorBrush {
        unsafe { self.brush.SetColor(&cc(c)) };
        &self.brush
    }

    pub fn clear(&self, c: Color) {
        // Quadro novo: a janela é a primeira caixa.
        #[cfg(debug_assertions)]
        BOXES.with(|b| {
            let s = unsafe { self.rt.GetSize() };
            *b.borrow_mut() = (vec![Rect { x: 0.0, y: 0.0, w: s.width, h: s.height }], 0.0, 0.0, vec![], 1.0);
        });
        unsafe { self.rt.Clear(Some(&cc(c))) }
    }

    pub fn fill(&self, r: Rect, c: Color) {
        unsafe { self.rt.FillRectangle(&r.d2d(), self.b(c)) }
    }

    pub fn round(&self, r: Rect, radius: f32, c: Color) {
        #[cfg(debug_assertions)]
        note_box(r);
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.rt.FillRoundedRectangle(&rr, self.b(c)) }
    }

    pub fn round_line(&self, r: Rect, radius: f32, c: Color, w: f32) {
        #[cfg(debug_assertions)]
        note_box(r);
        let r = r.inset(w / 2.0);
        let rr = D2D1_ROUNDED_RECT { rect: r.d2d(), radiusX: radius, radiusY: radius };
        unsafe { self.rt.DrawRoundedRectangle(&rr, self.b(c), w, None) }
    }

    pub fn line(&self, x0: f32, y0: f32, x1: f32, y1: f32, c: Color, w: f32) {
        unsafe {
            self.rt.DrawLine(
                windows_numerics::Vector2 { X: x0, Y: y0 },
                windows_numerics::Vector2 { X: x1, Y: y1 },
                self.b(c),
                w,
                None,
            )
        }
    }

    pub fn circle(&self, cx: f32, cy: f32, r: f32, c: Color) {
        let e = D2D1_ELLIPSE {
            point: windows_numerics::Vector2 { X: cx, Y: cy },
            radiusX: r,
            radiusY: r,
        };
        unsafe { self.rt.FillEllipse(&e, self.b(c)) }
    }

    pub fn circle_line(&self, cx: f32, cy: f32, r: f32, c: Color, w: f32) {
        let e = D2D1_ELLIPSE {
            point: windows_numerics::Vector2 { X: cx, Y: cy },
            radiusX: r,
            radiusY: r,
        };
        unsafe { self.rt.DrawEllipse(&e, self.b(c), w, None) }
    }

    pub fn ellipse_line(&self, e: &D2D1_ELLIPSE, c: Color, w: f32) {
        unsafe { self.rt.DrawEllipse(e, self.b(c), w, None) }
    }

    /// Arco de anel, começando no topo, em sentido horário.
    pub fn arc(&self, cx: f32, cy: f32, r: f32, frac: f32, c: Color, w: f32) {
        let frac = frac.clamp(0.0, 0.9999);
        if frac <= 0.0 {
            return;
        }
        let a = frac * std::f32::consts::TAU;
        let (sx, sy) = (cx, cy - r);
        let (ex, ey) = (cx + r * a.sin(), cy - r * a.cos());
        unsafe {
            let Ok(path) = self.factory.CreatePathGeometry() else { return };
            let Ok(sink) = path.Open() else { return };
            sink.BeginFigure(windows_numerics::Vector2 { X: sx, Y: sy }, D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: windows_numerics::Vector2 { X: ex, Y: ey },
                size: D2D_SIZE_F { width: r, height: r },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if a > std::f32::consts::PI {
                    D2D1_ARC_SIZE_LARGE
                } else {
                    D2D1_ARC_SIZE_SMALL
                },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            let _ = sink.Close();
            self.rt.DrawGeometry(&path, self.b(c), w, None);
        }
    }

    /// Arco de `a0` a `a1` graus (0 em cima, sentido horário), menor que meia volta.
    pub fn arc_span(&self, cx: f32, cy: f32, r: f32, a0: f32, a1: f32, c: Color, w: f32) {
        let p = |a: f32| windows_numerics::Vector2 { X: cx + r * a.to_radians().sin(), Y: cy - r * a.to_radians().cos() };
        unsafe {
            let Ok(path) = self.factory.CreatePathGeometry() else { return };
            let Ok(sink) = path.Open() else { return };
            sink.BeginFigure(p(a0), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT { point: p(a1), size: D2D_SIZE_F { width: r, height: r }, rotationAngle: 0.0, sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE, arcSize: D2D1_ARC_SIZE_SMALL });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            let _ = sink.Close();
            self.rt.DrawGeometry(&path, self.b(c), w, None);
        }
    }

    /// Gomo de anel entre os raios `r0` e `r1`, de `a0` a `a1` graus (0 em cima, sentido
    /// horário): pinta cada cor de `fills` por cima da anterior e depois o contorno.
    pub fn sector(&self, cx: f32, cy: f32, r0: f32, r1: f32, a0: f32, a1: f32, fills: &[Color], line: Option<(Color, f32)>) {
        let p = |r: f32, a: f32| windows_numerics::Vector2 { X: cx + r * a.to_radians().sin(), Y: cy - r * a.to_radians().cos() };
        let arc = |r: f32, a: f32, dir| D2D1_ARC_SEGMENT { point: p(r, a), size: D2D_SIZE_F { width: r, height: r }, rotationAngle: 0.0, sweepDirection: dir, arcSize: D2D1_ARC_SIZE_SMALL };
        unsafe {
            let Ok(path) = self.factory.CreatePathGeometry() else { return };
            let Ok(sink) = path.Open() else { return };
            sink.BeginFigure(p(r1, a0), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddArc(&arc(r1, a1, D2D1_SWEEP_DIRECTION_CLOCKWISE));
            sink.AddLine(p(r0, a1));
            sink.AddArc(&arc(r0, a0, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            let _ = sink.Close();
            for &f in fills {
                self.rt.FillGeometry(&path, self.b(f), None);
            }
            if let Some((c, w)) = line {
                self.rt.DrawGeometry(&path, self.b(c), w, None);
            }
        }
    }

    /// Linha de um gráfico. Valores de 0 a 100, do mais antigo ao mais novo.
    pub fn sparkline(&self, r: Rect, vals: &[f32], c: Color, w: f32) {
        if vals.len() < 2 {
            return;
        }
        let dx = r.w / (vals.len() - 1) as f32;
        let y = |v: f32| r.bottom() - v.clamp(0.0, 100.0) / 100.0 * r.h;
        unsafe {
            let Ok(path) = self.factory.CreatePathGeometry() else { return };
            let Ok(sink) = path.Open() else { return };
            sink.BeginFigure(
                windows_numerics::Vector2 { X: r.x, Y: y(vals[0]) },
                D2D1_FIGURE_BEGIN_HOLLOW,
            );
            for (i, v) in vals.iter().enumerate().skip(1) {
                sink.AddLine(windows_numerics::Vector2 { X: r.x + i as f32 * dx, Y: y(*v) });
            }
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            let _ = sink.Close();
            self.rt.DrawGeometry(&path, self.b(c), w, None);
        }
    }

    /// Traço de ícone em grade de 24 unidades. `d` lista polilinhas de pontos.
    pub fn icon(&self, x: f32, y: f32, size: f32, lines: &[&[(f32, f32)]], c: Color) {
        let k = size / 24.0;
        for pts in lines {
            for p in pts.windows(2) {
                self.line(
                    x + p[0].0 * k,
                    y + p[0].1 * k,
                    x + p[1].0 * k,
                    y + p[1].1 * k,
                    c,
                    1.5,
                );
            }
        }
    }

    pub fn text(&self, r: Rect, s: &str, size: f32, c: Color, align: Align, weight: Weight) {
        self.text_ex(r, &crate::i18n::tr(s), size, c, align, weight, false)
    }

    /// Texto que não passa pela tradução: nomes de arquivos e pastas.
    pub fn text_raw(&self, r: Rect, s: &str, size: f32, c: Color, align: Align, weight: Weight) {
        self.text_ex(r, s, size, c, align, weight, false)
    }

    /// Uma linha que diminui a letra, até 11 px, quando o texto não cabe; serve aos botões e
    /// seletores, cujo texto em outros idiomas pode passar da largura.
    pub fn text_fit(&self, r: Rect, s: &str, size: f32, c: Color, align: Align, weight: Weight) {
        let t = crate::i18n::tr(s);
        let mut sz = size;
        while sz > 11.0 && measure_raw(&t, sz, weight) > r.w {
            sz -= 1.0;
        }
        self.text_ex(r, &t, sz, c, align, weight, false)
    }

    /// Como `text_wrap`, com as linhas centradas.
    pub fn text_wrap_center(&self, r: Rect, s: &str, size: f32, c: Color, weight: Weight) {
        self.text_ex(r, &crate::i18n::tr(s), size, c, Align::Center, weight, true)
    }

    pub fn text_wrap(&self, r: Rect, s: &str, size: f32, c: Color, weight: Weight) {
        self.text_ex(r, &crate::i18n::tr(s), size, c, Align::Left, weight, true)
    }

    fn text_ex(&self, r: Rect, s: &str, size: f32, c: Color, a: Align, wt: Weight, wrap: bool) {
        let f = format(size, wt, wrap);
        let t = wide(s);
        #[cfg(debug_assertions)]
        note_overflow(&f, &t, s, r, a, wrap);
        unsafe {
            let _ = f.SetTextAlignment(match a {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            let _ = f.SetParagraphAlignment(if wrap {
                DWRITE_PARAGRAPH_ALIGNMENT_NEAR
            } else {
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER
            });
            self.rt.DrawText(
                &t,
                &f,
                &r.d2d(),
                self.b(c),
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    pub fn draw_layout(&self, l: &IDWriteTextLayout, x: f32, y: f32, c: Color) {
        unsafe {
            self.rt.DrawTextLayout(windows_numerics::Vector2 { X: x, Y: y }, l, self.b(c), D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }

    /// Bitmap BGRA pré-multiplicado, usado nos ícones de programas.
    pub fn bitmap(&self, w: u32, h: u32, px: &[u8]) -> Option<ID2D1Bitmap> {
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let size = D2D_SIZE_U { width: w, height: h };
        unsafe { self.rt.CreateBitmap(size, Some(px.as_ptr() as *const _), w * 4, &props).ok() }
    }

    pub fn draw_bitmap(&self, b: &ID2D1Bitmap, r: Rect) {
        unsafe {
            self.rt.DrawBitmap(b, Some(&r.d2d()), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
        }
    }

    pub fn push_clip(&self, r: Rect) {
        #[cfg(debug_assertions)]
        BOXES.with(|b| {
            let mut b = b.borrow_mut();
            let (n, dx, dy, k) = (b.0.len(), b.1, b.2, b.4);
            b.3.push((Rect { x: r.x * k + dx, y: r.y * k + dy, w: r.w * k, h: r.h * k }, n));
        });
        unsafe { self.rt.PushAxisAlignedClip(&r.d2d(), D2D1_ANTIALIAS_MODE_ALIASED) }
    }

    pub fn pop_clip(&self) {
        #[cfg(debug_assertions)]
        BOXES.with(|b| _ = b.borrow_mut().3.pop());
        unsafe { self.rt.PopAxisAlignedClip() }
    }

    pub fn translate(&self, dx: f32, dy: f32) {
        #[cfg(debug_assertions)]
        BOXES.with(|b| {
            let mut b = b.borrow_mut();
            (b.1, b.2, b.4) = (dx, dy, 1.0);
        });
        let m = windows_numerics::Matrix3x2::translation(dx, dy);
        unsafe { self.rt.SetTransform(&m) }
    }

    /// Desenha na escala `k` a partir de `x`,`y` (miniaturas); `translate(0.0, 0.0)` volta ao normal.
    pub fn scale_at(&self, k: f32, x: f32, y: f32) {
        #[cfg(debug_assertions)]
        BOXES.with(|b| {
            let mut b = b.borrow_mut();
            (b.1, b.2, b.4) = (x, y, k);
        });
        let m = windows_numerics::Matrix3x2::scale(k, k) * windows_numerics::Matrix3x2::translation(x, y);
        unsafe { self.rt.SetTransform(&m) }
    }
}

/// Render target de janela normal, com escala pelo DPI.
pub fn hwnd_target(hwnd: HWND, w: u32, h: u32, dpi: f32) -> Result<ID2D1HwndRenderTarget> {
    unsafe {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            dpiX: dpi,
            dpiY: dpi,
            ..Default::default()
        };
        let hp = D2D1_HWND_RENDER_TARGET_PROPERTIES {
            hwnd,
            pixelSize: D2D_SIZE_U { width: w, height: h },
            presentOptions: D2D1_PRESENT_OPTIONS_NONE,
        };
        // Fábrica própria: ao fechar a janela, o driver de vídeo sai da memória junto.
        let f: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        f.CreateHwndRenderTarget(&props, &hp)
    }
}
