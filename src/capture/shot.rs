//! Sobreposição de foto: escolher a área, anotar, copiar, salvar ou fixar na tela.

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::Frame;
use crate::overlays::register;
use crate::ui::gfx::*;

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Arrow,
    Rect,
    Ellipse,
    Line,
    Pen,
    Marker,
    Text,
    Number,
    Pixelate,
}

const TOOLS: &[(Tool, &str, &[&[(f32, f32)]])] = &[
    (Tool::Arrow, "Seta", &[&[(5., 19.), (19., 5.)], &[(9., 5.), (19., 5.), (19., 15.)]]),
    (Tool::Rect, "Retângulo", &[&[(4., 6.), (20., 6.), (20., 18.), (4., 18.), (4., 6.)]]),
    (Tool::Ellipse, "Elipse", &[&[(12., 5.), (17., 6.), (20., 9.), (20., 15.), (17., 18.), (12., 19.), (7., 18.), (4., 15.), (4., 9.), (7., 6.), (12., 5.)]]),
    (Tool::Line, "Linha", &[&[(5., 19.), (19., 5.)]]),
    (Tool::Pen, "Caneta", &[&[(4., 20.), (8., 19.), (19., 8.), (16., 5.), (5., 16.), (4., 20.)]]),
    (Tool::Marker, "Marcador", &[&[(5., 15.), (11., 9.), (16., 14.), (10., 20.), (6., 20.), (5., 15.)], &[(14., 6.), (18., 10.)]]),
    (Tool::Text, "Texto", &[&[(5., 6.), (19., 6.)], &[(12., 6.), (12., 19.)]]),
    (Tool::Number, "Número", &[&[(12., 3.), (18., 6.), (21., 12.), (18., 18.), (12., 21.), (6., 18.), (3., 12.), (6., 6.), (12., 3.)], &[(10., 9.), (12., 8.), (12., 16.)]]),
    (Tool::Pixelate, "Pixelar", &[&[(4., 4.), (10., 4.), (10., 10.), (4., 10.), (4., 4.)], &[(14., 14.), (20., 14.), (20., 20.), (14., 20.), (14., 14.)], &[(14., 4.), (20., 4.), (20., 10.), (14., 10.), (14., 4.)], &[(4., 14.), (10., 14.), (10., 20.), (4., 20.), (4., 14.)]]),
];

const COLORS: [Color; 4] = [rgb(0xff8a4c), ACCENT, WHITE, rgb(0x14161c)];

/// O que cada botão faz, com a tecla entre parênteses, na ordem da barra.
const TOOL_HELP: [&str; 9] = [
    "Seta: aponta para algo (A)",
    "Retângulo: destaca uma área (R)",
    "Elipse: circula algo (E)",
    "Linha: traça uma reta (L)",
    "Caneta: desenha à mão livre (P)",
    "Marcador: grifa por cima (M)",
    "Texto: escreve na imagem (T)",
    "Número: marca passos em ordem (N)",
    "Pixelar: esconde uma parte (B)",
];
const TOOL_KEYS: [u8; 9] = [b'A', b'R', b'E', b'L', b'P', b'M', b'T', b'N', b'B'];
const COLOR_HELP: [&str; 4] = ["Laranja (1)", "Azul (2)", "Branco (3)", "Preto (4)"];
const ACTION_HELP: [&str; 3] = ["Fixar a imagem flutuando na tela (F)", "Salvar na pasta (Ctrl+S)", "Copiar para a área de transferência (Ctrl+C)"];

#[derive(Clone)]
enum Mark {
    Arrow(f32, f32, f32, f32, Color),
    Rect(f32, f32, f32, f32, Color),
    Ellipse(f32, f32, f32, f32, Color),
    Line(f32, f32, f32, f32, Color),
    Pen(Vec<(f32, f32)>, Color),
    Marker(Vec<(f32, f32)>, Color),
    Text(f32, f32, String, Color),
    Number(f32, f32, u32, Color),
    Pixelate(f32, f32, f32, f32),
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    None,
    New,
    Handle(u8),
    Mark,
}

struct Overlay {
    hwnd: HWND,
    frame: Frame,
    rt: ID2D1HwndRenderTarget,
    canvas: Canvas,
    bmp: ID2D1Bitmap,
    scale: f32,
    w: f32,
    h: f32,
    sel: Option<Rect>,
    drag: Drag,
    anchor: (f32, f32),
    orig: Rect,
    mouse: (f32, f32),
    tool: Tool,
    color: usize,
    marks: Vec<Mark>,
    live: Option<Mark>,
    typing: Option<(f32, f32, String)>,
    number: u32,
    dir: String,
    hdr_file: bool,
    done: Option<&'static str>,
}

thread_local! {
    static OVERLAY: std::cell::RefCell<Option<Overlay>> = const { std::cell::RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Overlay) -> R) -> Option<R> {
    OVERLAY.with(|o| o.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

pub fn is_open() -> bool {
    OVERLAY.with(|o| o.borrow().is_some())
}

fn norm(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    rect(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs())
}

fn handles(r: Rect) -> [(f32, f32); 8] {
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    let (xm, ym) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    [(x0, y0), (xm, y0), (x1, y0), (x0, ym), (x1, ym), (x0, y1), (xm, y1), (x1, y1)]
}

fn bar_rect(sel: Rect, w: f32, h: f32) -> Rect {
    const BW: f32 = 690.0;
    const BH: f32 = 56.0;
    let x = (sel.x + sel.w / 2.0 - BW / 2.0).clamp(8.0, (w - BW - 8.0).max(8.0));
    let mut y = sel.bottom() + 12.0;
    if y + BH > h - 8.0 {
        y = (sel.y - BH - 12.0).max(8.0);
    }
    if sel.bottom() + 12.0 + BH > h - 8.0 && sel.y - BH - 12.0 < 8.0 {
        y = sel.bottom() - BH - 12.0;
    }
    rect(x, y, BW, BH)
}

fn pixel_blocks(c: &Canvas, f: &Frame, s: f32, r: Rect) {
    const B: f32 = 14.0;
    let (fw, fh) = (f.w as i32, f.h as i32);
    let mut y = r.y;
    while y < r.bottom() {
        let mut x = r.x;
        let bh = B.min(r.bottom() - y);
        while x < r.right() {
            let bw = B.min(r.right() - x);
            let (px, py) = (((x + bw / 2.0) * s) as i32, ((y + bh / 2.0) * s) as i32);
            if px >= 0 && py >= 0 && px < fw && py < fh {
                let i = (py * fw + px) as usize * 4;
                let col = Color {
                    r: f.bgra[i + 2] as f32 / 255.0,
                    g: f.bgra[i + 1] as f32 / 255.0,
                    b: f.bgra[i] as f32 / 255.0,
                    a: 1.0,
                };
                c.fill(rect(x, y, bw + 0.5, bh + 0.5), col);
            }
            x += B;
        }
        y += B;
    }
}

fn draw_arrow(c: &Canvas, x0: f32, y0: f32, x1: f32, y1: f32, col: Color) {
    c.line(x0, y0, x1, y1, col, 3.5);
    let a = (y1 - y0).atan2(x1 - x0);
    for d in [2.6f32, -2.6] {
        let (hx, hy) = (x1 + 16.0 * (a + d).cos(), y1 + 16.0 * (a + d).sin());
        c.line(x1, y1, hx, hy, col, 3.5);
    }
}

fn draw_mark(c: &Canvas, m: &Mark, f: &Frame, s: f32) {
    match m {
        Mark::Arrow(a, b, x, y, col) => draw_arrow(c, *a, *b, *x, *y, *col),
        Mark::Rect(a, b, x, y, col) => c.round_line(norm(*a, *b, *x, *y), 3.0, *col, 3.0),
        Mark::Ellipse(a, b, x, y, col) => {
            let r = norm(*a, *b, *x, *y);
            let e = D2D1_ELLIPSE {
                point: windows_numerics::Vector2 { X: r.x + r.w / 2.0, Y: r.y + r.h / 2.0 },
                radiusX: r.w / 2.0,
                radiusY: r.h / 2.0,
            };
            c.ellipse_line(&e, *col, 3.0);
        }
        Mark::Line(a, b, x, y, col) => c.line(*a, *b, *x, *y, *col, 3.0),
        Mark::Pen(p, col) => {
            for w in p.windows(2) {
                c.line(w[0].0, w[0].1, w[1].0, w[1].1, *col, 3.0);
            }
        }
        Mark::Marker(p, col) => {
            for w in p.windows(2) {
                c.line(w[0].0, w[0].1, w[1].0, w[1].1, col.alpha(0.4), 18.0);
            }
        }
        Mark::Text(x, y, t, col) => {
            let (tw, th) = measure(t, 18.0, Weight::Semibold, None);
            c.text(rect(*x, *y, tw + 8.0, th + 4.0), t, 18.0, *col, Align::Left, Weight::Semibold);
        }
        Mark::Number(x, y, n, col) => {
            c.circle(*x, *y, 13.0, *col);
            let fg = if col.r + col.g + col.b > 1.5 { rgb(0x1f1208) } else { WHITE };
            c.text(rect(x - 13.0, y - 13.0, 26.0, 26.0), &n.to_string(), 14.0, fg, Align::Center, Weight::Semibold);
        }
        Mark::Pixelate(a, b, x, y) => pixel_blocks(c, f, s, norm(*a, *b, *x, *y)),
    }
}

impl Overlay {
    fn paint(&mut self) {
        let c = &self.canvas;
        unsafe {
            self.rt.BeginDraw();
            let full = D2D_RECT_F { left: 0.0, top: 0.0, right: self.w, bottom: self.h };
            c.rt.DrawBitmap(&self.bmp, Some(&full), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
        }
        let dim = rgb(0x06080c).alpha(0.58);
        match self.sel {
            None => {
                c.fill(rect(0.0, 0.0, self.w, self.h), dim);
                let hint = "Arraste para escolher a área · Enter copia a tela inteira · Esc cancela";
                let t = rect(0.0, self.h - 70.0, self.w, 24.0);
                c.text(t, hint, 14.0, TEXT, Align::Center, Weight::Regular);
            }
            Some(s) => {
                c.fill(rect(0.0, 0.0, self.w, s.y), dim);
                c.fill(rect(0.0, s.bottom(), self.w, self.h - s.bottom()), dim);
                c.fill(rect(0.0, s.y, s.x, s.h), dim);
                c.fill(rect(s.right(), s.y, self.w - s.right(), s.h), dim);
                c.push_clip(s);
                for m in &self.marks {
                    draw_mark(c, m, &self.frame, self.scale);
                }
                if let Some(m) = &self.live {
                    draw_mark(c, m, &self.frame, self.scale);
                }
                if let Some((x, y, t)) = &self.typing {
                    let shown = format!("{t}|");
                    let (tw, th) = measure(&shown, 18.0, Weight::Semibold, None);
                    c.text(rect(*x, *y, tw + 8.0, th + 4.0), &shown, 18.0, COLORS[self.color], Align::Left, Weight::Semibold);
                }
                c.pop_clip();
                c.round_line(s.inset(-1.0), 0.0, ACCENT, 1.5);
                for (hx, hy) in handles(s) {
                    let h = rect(hx - 4.5, hy - 4.5, 9.0, 9.0);
                    c.round(h, 2.0, WHITE);
                    c.round_line(h, 2.0, ACCENT, 1.5);
                }
                let size = format!("{} × {}", (s.w * self.scale) as u32, (s.h * self.scale) as u32);
                let tw = measure(&size, 13.0, Weight::Regular, None).0 + 16.0;
                let ly = if s.y > 32.0 { s.y - 30.0 } else { s.y + 6.0 };
                let l = rect(s.x, ly, tw, 24.0);
                c.round(l, 6.0, rgb(0x0d0f14).alpha(0.88));
                c.text(l, &size, 13.0, TEXT, Align::Center, Weight::Regular);
                if self.drag == Drag::None || self.drag == Drag::Mark {
                    self.draw_bar(s);
                }
            }
        }
        unsafe {
            let _ = self.rt.EndDraw(None, None);
        }
    }

    fn draw_bar(&self, sel: Rect) {
        let c = &self.canvas;
        let b = bar_rect(sel, self.w, self.h);
        c.round(b, 12.0, rgb(0x14161c).alpha(0.96));
        let mut x = b.x + 6.0;
        let y = b.y + 6.0;
        for (t, _, icon) in TOOLS {
            let r = rect(x, y, 44.0, 44.0);
            let on = *t == self.tool;
            let hover = r.contains(self.mouse.0, self.mouse.1);
            if on {
                c.round(r, 8.0, ACCENT);
            } else if hover {
                c.round(r, 8.0, LINE);
            }
            c.icon(r.x + 12.0, r.y + 12.0, 20.0, icon, if on { ON_ACCENT } else { TEXT });
            x += 46.0;
        }
        x += 6.0;
        for (i, col) in COLORS.iter().enumerate() {
            let cy = y + 22.0;
            let cx = x + 12.0;
            if i == self.color {
                c.circle_line(cx, cy, 11.0, WHITE, 1.5);
            }
            c.circle(cx, cy, 8.0, *col);
            if i == 3 {
                c.circle_line(cx, cy, 8.0, BORDER2, 1.0);
            }
            x += 24.0;
        }
        x += 6.0;
        for (i, (_, icon)) in self.actions().iter().enumerate() {
            let r = rect(x + i as f32 * 42.0, y + 2.0, 40.0, 40.0);
            if r.contains(self.mouse.0, self.mouse.1) {
                c.round(r, 8.0, LINE);
            }
            c.icon(r.x + 10.0, r.y + 10.0, 20.0, icon, TEXT);
        }
        if let Some(text) = self.hover_help(b) {
            let (tw, _) = measure(text, 13.0, Weight::Regular, None);
            let w = tw + 24.0;
            let x = (self.mouse.0 - w / 2.0).clamp(8.0, (self.w - w - 8.0).max(8.0));
            let y = if b.y >= 44.0 { b.y - 40.0 } else { b.bottom() + 8.0 };
            let r = rect(x, y, w, 32.0);
            c.round(r, 8.0, rgb(0x14161c).alpha(0.96));
            c.round_line(r, 8.0, BORDER, 1.0);
            c.text(r, text, 13.0, TEXT, Align::Center, Weight::Regular);
        }
    }

    /// Texto do botão sob o mouse, nas mesmas posições que `bar_click` usa.
    fn hover_help(&self, b: Rect) -> Option<&'static str> {
        let (mx, my) = self.mouse;
        if !b.contains(mx, my) {
            return None;
        }
        let mut x = b.x + 6.0;
        for help in TOOL_HELP {
            if rect(x, b.y + 6.0, 44.0, 44.0).contains(mx, my) {
                return Some(help);
            }
            x += 46.0;
        }
        x += 6.0;
        for help in COLOR_HELP {
            if rect(x, b.y + 6.0, 24.0, 44.0).contains(mx, my) {
                return Some(help);
            }
            x += 24.0;
        }
        x += 6.0;
        (0..3).find(|i| rect(x + *i as f32 * 42.0, b.y + 8.0, 40.0, 40.0).contains(mx, my)).map(|i| ACTION_HELP[i])
    }

    fn over_bar(&self) -> bool {
        self.sel.is_some_and(|s| bar_rect(s, self.w, self.h).contains(self.mouse.0, self.mouse.1))
    }

    /// Atalhos de letra e número. Não valem enquanto a pessoa digita um texto.
    fn key(&mut self, vk: u32) {
        if self.typing.is_some() {
            return;
        }
        if let Some(i) = TOOL_KEYS.iter().position(|k| *k as u32 == vk) {
            self.tool = TOOLS[i].0;
        } else if (0x31..=0x34).contains(&vk) {
            self.color = (vk - 0x31) as usize;
        } else if vk == 0x46 {
            self.finish(Output::Pin);
        }
    }

    fn actions(&self) -> [(&'static str, &'static [&'static [(f32, f32)]]); 3] {
        [
            ("pin", &[&[(9., 4.), (15., 4.), (14., 10.), (17., 13.), (7., 13.), (10., 10.), (9., 4.)], &[(12., 13.), (12., 20.)]]),
            ("save", &[&[(12., 4.), (12., 15.)], &[(7., 11.), (12., 16.), (17., 11.)], &[(5., 20.), (19., 20.)]]),
            ("copy", &[&[(9., 9.), (20., 9.), (20., 20.), (9., 20.), (9., 9.)], &[(5., 15.), (4., 15.), (4., 4.), (15., 4.), (15., 5.)]]),
        ]
    }

    /// Clique na barra. Devolve true quando consumiu o clique.
    fn bar_click(&mut self, x: f32, y: f32) -> bool {
        let Some(sel) = self.sel else { return false };
        let b = bar_rect(sel, self.w, self.h);
        if !b.contains(x, y) {
            return false;
        }
        self.commit_text();
        let mut bx = b.x + 6.0;
        for (t, _, _) in TOOLS {
            if rect(bx, b.y + 6.0, 44.0, 44.0).contains(x, y) {
                self.tool = *t;
                return true;
            }
            bx += 46.0;
        }
        bx += 6.0;
        for i in 0..COLORS.len() {
            if rect(bx, b.y + 6.0, 24.0, 44.0).contains(x, y) {
                self.color = i;
                return true;
            }
            bx += 24.0;
        }
        bx += 6.0;
        for i in 0..3 {
            if rect(bx + i as f32 * 42.0, b.y + 8.0, 40.0, 40.0).contains(x, y) {
                match i {
                    0 => self.finish(Output::Pin),
                    1 => self.finish(Output::Save),
                    _ => self.finish(Output::Copy),
                }
                return true;
            }
        }
        true
    }

    fn commit_text(&mut self) {
        if let Some((x, y, t)) = self.typing.take() {
            if !t.is_empty() {
                self.marks.push(Mark::Text(x, y, t, COLORS[self.color]));
            }
        }
    }

    fn on_down(&mut self, x: f32, y: f32) {
        if self.bar_click(x, y) {
            return;
        }
        self.commit_text();
        self.anchor = (x, y);
        match self.sel {
            None => {
                self.drag = Drag::New;
                self.sel = Some(rect(x, y, 0.0, 0.0));
            }
            Some(s) => {
                for (i, (hx, hy)) in handles(s).iter().enumerate() {
                    if (x - hx).abs() < 9.0 && (y - hy).abs() < 9.0 {
                        self.drag = Drag::Handle(i as u8);
                        self.orig = s;
                        return;
                    }
                }
                if !s.contains(x, y) {
                    self.drag = Drag::New;
                    self.sel = Some(rect(x, y, 0.0, 0.0));
                    self.marks.clear();
                    return;
                }
                self.drag = Drag::Mark;
                let col = COLORS[self.color];
                self.live = match self.tool {
                    Tool::Arrow => Some(Mark::Arrow(x, y, x, y, col)),
                    Tool::Rect => Some(Mark::Rect(x, y, x, y, col)),
                    Tool::Ellipse => Some(Mark::Ellipse(x, y, x, y, col)),
                    Tool::Line => Some(Mark::Line(x, y, x, y, col)),
                    Tool::Pen => Some(Mark::Pen(vec![(x, y)], col)),
                    Tool::Marker => Some(Mark::Marker(vec![(x, y)], col)),
                    Tool::Pixelate => Some(Mark::Pixelate(x, y, x, y)),
                    Tool::Text => {
                        self.typing = Some((x, y, String::new()));
                        self.drag = Drag::None;
                        None
                    }
                    Tool::Number => {
                        self.number += 1;
                        self.marks.push(Mark::Number(x, y, self.number, col));
                        self.drag = Drag::None;
                        None
                    }
                };
            }
        }
    }

    fn on_move(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
        match self.drag {
            Drag::New => self.sel = Some(norm(self.anchor.0, self.anchor.1, x, y)),
            Drag::Handle(i) => {
                let o = self.orig;
                let (mut x0, mut y0, mut x1, mut y1) = (o.x, o.y, o.right(), o.bottom());
                if matches!(i, 0 | 3 | 5) {
                    x0 = x;
                }
                if matches!(i, 2 | 4 | 7) {
                    x1 = x;
                }
                if matches!(i, 0 | 1 | 2) {
                    y0 = y;
                }
                if matches!(i, 5 | 6 | 7) {
                    y1 = y;
                }
                self.sel = Some(norm(x0, y0, x1, y1));
            }
            Drag::Mark => match &mut self.live {
                Some(Mark::Arrow(_, _, a, b, _))
                | Some(Mark::Rect(_, _, a, b, _))
                | Some(Mark::Ellipse(_, _, a, b, _))
                | Some(Mark::Line(_, _, a, b, _))
                | Some(Mark::Pixelate(_, _, a, b)) => {
                    *a = x;
                    *b = y;
                }
                Some(Mark::Pen(p, _)) | Some(Mark::Marker(p, _)) => p.push((x, y)),
                _ => {}
            },
            Drag::None => {}
        }
    }

    fn on_up(&mut self) {
        if self.drag == Drag::New {
            if let Some(s) = self.sel {
                if s.w < 4.0 || s.h < 4.0 {
                    self.sel = None;
                }
            }
        }
        if let Some(m) = self.live.take() {
            self.marks.push(m);
        }
        self.drag = Drag::None;
    }

    /// Junta imagem e anotações na área escolhida, em resolução real.
    fn render(&self) -> Result<(u32, u32, Vec<u8>)> {
        let s = self.sel.unwrap_or(rect(0.0, 0.0, self.w, self.h));
        let k = self.scale;
        let (px, py) = ((s.x * k).round() as u32, (s.y * k).round() as u32);
        let w = ((s.w * k).round() as u32).clamp(1, self.frame.w - px.min(self.frame.w - 1));
        let h = ((s.h * k).round() as u32).clamp(1, self.frame.h - py.min(self.frame.h - 1));
        unsafe {
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let bmp = wic.CreateBitmap(w, h, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let rt = crate::ui::gfx::factory().CreateWicBitmapRenderTarget(&bmp, &props)?;
            let bprops = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            let src = rt.CreateBitmap(
                D2D_SIZE_U { width: self.frame.w, height: self.frame.h },
                Some(self.frame.bgra.as_ptr() as *const _),
                self.frame.w * 4,
                &bprops,
            )?;
            let c = Canvas::new(rt.clone());
            rt.BeginDraw();
            let dst = D2D_RECT_F { left: 0.0, top: 0.0, right: w as f32, bottom: h as f32 };
            let from = D2D_RECT_F { left: px as f32, top: py as f32, right: (px + w) as f32, bottom: (py + h) as f32 };
            rt.DrawBitmap(&src, Some(&dst), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_NEAREST_NEIGHBOR, Some(&from));
            let m = windows_numerics::Matrix3x2::translation(-s.x, -s.y) * windows_numerics::Matrix3x2::scale(k, k);
            rt.SetTransform(&m);
            for mark in &self.marks {
                draw_mark(&c, mark, &self.frame, k);
            }
            rt.EndDraw(None, None)?;
            let mut out = vec![0u8; (w * h * 4) as usize];
            bmp.CopyPixels(std::ptr::null(), w * 4, &mut out)?;
            for a in out.chunks_exact_mut(4) {
                a[3] = 255;
            }
            Ok((w, h, out))
        }
    }

    fn finish(&mut self, out: Output) {
        self.commit_text();
        let Ok((w, h, px)) = self.render() else {
            self.done = Some("Não consegui gerar a imagem");
            return;
        };
        let msg = match out {
            Output::Copy => {
                if super::copy_to_clipboard(self.hwnd, w, h, &px) {
                    "Foto copiada"
                } else {
                    "Não consegui copiar"
                }
            }
            Output::Save => {
                let path = super::new_path(&self.dir, "png");
                let mut ok = super::save_png(&path, w, h, &px).is_ok();
                if self.hdr_file && self.sel.is_none() {
                    if let Some(half) = &self.frame.half {
                        let p = super::new_path(&self.dir, "jxr");
                        ok &= super::save_jxr(&p, self.frame.w, self.frame.h, half).is_ok();
                    }
                }
                if ok {
                    "Foto salva"
                } else {
                    "Não consegui salvar a foto"
                }
            }
            Output::Pin => {
                let s = self.sel.unwrap_or(rect(0.0, 0.0, self.w, self.h));
                pin(px, w, h, (s.x * self.scale) as i32, (s.y * self.scale) as i32);
                "Foto fixada: arraste para mover, botão direito fecha"
            }
        };
        self.done = Some(msg);
    }
}

enum Output {
    Copy,
    Save,
    Pin,
}

fn close(h: HWND, msg: Option<&'static str>) {
    OVERLAY.with(|o| *o.borrow_mut() = None);
    unsafe {
        let _ = DestroyWindow(h);
    }
    if let Some(m) = msg {
        crate::app::with(|a| a.toast.info(m));
    }
}

unsafe extern "system" fn proc_(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let scale = GetDpiForWindow(h) as f32 / 96.0;
    let xy = || ((l.0 & 0xffff) as i16 as f32 / scale, ((l.0 >> 16) & 0xffff) as i16 as f32 / scale);
    match m {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            BeginPaint(h, &mut ps);
            with(|o| o.paint());
            let _ = EndPaint(h, &ps);
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_LBUTTONDOWN => {
            let (x, y) = xy();
            with(|o| o.on_down(x, y));
            SetCapture(h);
        }
        WM_MOUSEMOVE => {
            let (x, y) = xy();
            with(|o| o.on_move(x, y));
        }
        WM_LBUTTONUP => {
            with(|o| o.on_up());
            let _ = ReleaseCapture();
        }
        WM_RBUTTONUP => {
            close(h, None);
            return LRESULT(0);
        }
        WM_CHAR => {
            let ch = w.0 as u32;
            with(|o| {
                if let Some((_, _, t)) = &mut o.typing {
                    match ch {
                        8 => {
                            t.pop();
                        }
                        13 | 27 => {}
                        c if c >= 32 => {
                            if let Some(c) = char::from_u32(c) {
                                t.push(c);
                            }
                        }
                        _ => {}
                    }
                }
            });
        }
        WM_KEYDOWN => {
            let ctrl = GetAsyncKeyState(VK_CONTROL.0 as i32) as u16 & 0x8000 != 0;
            match w.0 as u32 {
                0x1B => {
                    if with(|o| o.typing.take().is_some()) != Some(true) {
                        close(h, None);
                        return LRESULT(0);
                    }
                }
                0x0D => {
                    if with(|o| o.typing.is_some()) == Some(true) {
                        with(|o| o.commit_text());
                    } else {
                        with(|o| o.finish(Output::Copy));
                    }
                }
                0x53 if ctrl => {
                    with(|o| o.finish(Output::Save));
                }
                0x43 if ctrl => {
                    with(|o| {
                        o.commit_text();
                        o.finish(Output::Copy);
                    });
                }
                0x5A if ctrl => {
                    with(|o| {
                        if matches!(o.marks.pop(), Some(Mark::Number(..))) {
                            o.number -= 1;
                        }
                    });
                }
                vk if !ctrl => {
                    with(|o| o.key(vk));
                }
                _ => {}
            }
        }
        WM_SETCURSOR => {
            // Cruz para mirar, e seta normal só em cima da barra de botões.
            let cursor = if with(|o| o.over_bar()) == Some(true) { IDC_ARROW } else { IDC_CROSS };
            SetCursor(LoadCursorW(None, cursor).ok());
            return LRESULT(1);
        }
        _ => return DefWindowProcW(h, m, w, l),
    }
    if let Some(Some(msg)) = with(|o| o.done) {
        close(h, Some(msg));
        return LRESULT(0);
    }
    let _ = InvalidateRect(Some(h), None, false);
    LRESULT(0)
}

/// Abre a sobreposição no monitor do cursor com o quadro já capturado.
/// `area`: retângulo já escolhido, em pixels da área de trabalho, como o de uma janela.
pub fn open(frame: Frame, mon: HMONITOR, dir: String, hdr_file: bool, area: Option<RECT>) {
    if is_open() {
        return;
    }
    register(w!("PrismaShot"), proc_);
    unsafe {
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(mon, &mut mi);
        let r = mi.rcMonitor;
        let (pw, ph) = (r.right - r.left, r.bottom - r.top);
        let Ok(h) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("PrismaShot"),
            w!("Prisma"),
            WS_POPUP,
            r.left,
            r.top,
            pw,
            ph,
            None,
            None,
            None,
            None,
        ) else {
            return;
        };
        let dpi = GetDpiForWindow(h) as f32;
        let Ok(rt) = hwnd_target(h, pw as u32, ph as u32, dpi) else { return };
        let base: ID2D1RenderTarget = rt.cast().unwrap();
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
            dpiX: dpi,
            dpiY: dpi,
        };
        let Ok(bmp) = base.CreateBitmap(
            D2D_SIZE_U { width: frame.w, height: frame.h },
            Some(frame.bgra.as_ptr() as *const _),
            frame.w * 4,
            &props,
        ) else {
            return;
        };
        let scale = dpi / 96.0;
        let sel = area.map(|a| {
            let (x0, y0) = (a.left.max(r.left) - r.left, a.top.max(r.top) - r.top);
            let (x1, y1) = (a.right.min(r.right) - r.left, a.bottom.min(r.bottom) - r.top);
            rect(x0 as f32 / scale, y0 as f32 / scale, (x1 - x0).max(1) as f32 / scale, (y1 - y0).max(1) as f32 / scale)
        });
        #[cfg(debug_assertions)]
        let sel = if std::env::var("PRISMA_SHOT_SEL").is_ok() { Some(rect(300.0, 120.0, 680.0, 400.0)) } else { sel };
        let ov = Overlay {
            hwnd: h,
            rt,
            canvas: Canvas::new(base),
            bmp,
            scale,
            w: pw as f32 / scale,
            h: ph as f32 / scale,
            frame,
            sel,
            drag: Drag::None,
            anchor: (0.0, 0.0),
            orig: Rect::default(),
            mouse: (0.0, 0.0),
            tool: Tool::Arrow,
            color: 0,
            marks: Vec::new(),
            live: None,
            typing: None,
            number: 0,
            dir,
            hdr_file,
            done: None,
        };
        OVERLAY.with(|o| *o.borrow_mut() = Some(ov));
        SetCursor(LoadCursorW(None, IDC_CROSS).ok());
        let _ = ShowWindow(h, SW_SHOW);
        let _ = SetForegroundWindow(h);
    }
}

unsafe extern "system" fn pin_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match m {
        WM_NCHITTEST => LRESULT(HTCAPTION as isize),
        WM_NCRBUTTONUP => {
            let _ = DestroyWindow(h);
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(h, &mut ps);
            let mut r = RECT::default();
            let _ = GetClientRect(h, &mut r);
            PIN.with(|p| {
                if let Some((w0, h0, px)) = p.borrow().iter().find(|v| v.0 == h.0 as isize).map(|v| (v.1, v.2, &v.3)) {
                    let bi = BITMAPINFO {
                        bmiHeader: BITMAPINFOHEADER {
                            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                            biWidth: w0 as i32,
                            biHeight: -(h0 as i32),
                            biPlanes: 1,
                            biBitCount: 32,
                            biCompression: BI_RGB.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    StretchDIBits(dc, 0, 0, r.right, r.bottom, 0, 0, w0 as i32, h0 as i32, Some(px.as_ptr() as *const _), &bi, DIB_RGB_COLORS, SRCCOPY);
                }
            });
            let _ = EndPaint(h, &ps);
            LRESULT(0)
        }
        WM_DESTROY => {
            PIN.with(|p| p.borrow_mut().retain(|v| v.0 != h.0 as isize));
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

thread_local! {
    static PIN: std::cell::RefCell<Vec<(isize, u32, u32, Vec<u8>)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn pin(px: Vec<u8>, w: u32, h: u32, x: i32, y: i32) {
    register(w!("PrismaPin"), pin_proc);
    unsafe {
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("PrismaPin"),
            w!("Prisma"),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            w as i32,
            h as i32,
            None,
            None,
            None,
            None,
        ) else {
            return;
        };
        PIN.with(|p| p.borrow_mut().push((hwnd.0 as isize, w, h, px)));
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
}

/// Só para teste: monta uma seleção com anotações e grava o resultado.
#[cfg(debug_assertions)]
pub fn debug_setup(path: &str) {
    with(|o| {
        let c = COLORS[0];
        o.sel = Some(rect(300.0, 120.0, 680.0, 400.0));
        o.mouse = (0.0, 0.0);
        o.marks = vec![
            Mark::Rect(660.0, 180.0, 900.0, 310.0, c),
            Mark::Arrow(440.0, 420.0, 648.0, 270.0, c),
            Mark::Number(685.0, 177.0, 1, c),
            Mark::Pixelate(340.0, 440.0, 500.0, 488.0),
            Mark::Text(400.0, 390.0, "Olá, mundo".into(), WHITE),
            Mark::Marker(vec![(700.0, 400.0), (800.0, 402.0), (900.0, 398.0)], rgb(0xffe14c)),
        ];
        o.number = 1;
        if let Ok((w, h, px)) = o.render() {
            eprintln!("{w}x{h} {:?}", super::save_png(path, w, h, &px));
        }
    });
}
