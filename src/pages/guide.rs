//! Guia: o mesmo texto de docs/guide, que o GitHub mostra, desenhado aqui dentro.
//! Cada linha do arquivo é um bloco: título, parágrafo, item ou imagem.

use super::*;
use crate::ui::gfx::{layout_height, rich_layout};
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;

const PT: &str = include_str!("../../docs/guide/GUIA.md");
const EN: &str = include_str!("../../docs/guide/GUIDE.md");
const IMAGES: &[(&str, &[u8])] = &[
    ("welcome-pt.png", include_bytes!("../../docs/guide/welcome-pt.png")),
    ("welcome-en.png", include_bytes!("../../docs/guide/welcome-en.png")),
    ("help-pt.png", include_bytes!("../../docs/guide/help-pt.png")),
    ("help-en.png", include_bytes!("../../docs/guide/help-en.png")),
    ("meter-compact-pt.png", include_bytes!("../../docs/guide/meter-compact-pt.png")),
    ("meter-compact-en.png", include_bytes!("../../docs/guide/meter-compact-en.png")),
    ("meter-full-pt.png", include_bytes!("../../docs/guide/meter-full-pt.png")),
    ("meter-full-en.png", include_bytes!("../../docs/guide/meter-full-en.png")),
    ("limiter-pt.png", include_bytes!("../../docs/guide/limiter-pt.png")),
    ("limiter-en.png", include_bytes!("../../docs/guide/limiter-en.png")),
];
const INDEX_W: f32 = 220.0;
const INDEX_ROW: f32 = 30.0;
const MAX_W: f32 = 760.0;

enum Kind {
    Section,
    Sub,
    Text,
    Bullet,
    Image(&'static str, &'static [u8]),
}

struct Block {
    kind: Kind,
    layout: Option<IDWriteTextLayout>,
    y: f32,
    h: f32,
    w: f32,
}

pub struct Layout {
    en: bool,
    w: f32,
    height: f32,
    blocks: Vec<Block>,
    sections: Vec<(&'static str, f32)>,
}

/// Largura e altura gravadas no começo de todo PNG. As imagens vêm em escala 2.
fn png_size(d: &[u8]) -> (f32, f32) {
    let read = |i: usize| u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as f32 / 2.0;
    (read(16), read(20))
}

/// Texto com a distância até o bloco seguinte.
fn add(out: &mut Layout, y: &mut f32, kind: Kind, s: &str, size: f32, width: f32, after: f32) {
    let layout = rich_layout(s, size, width);
    let h = layout.as_ref().map_or(0.0, layout_height);
    out.blocks.push(Block { kind, layout, y: *y, h, w: width });
    *y += h + after;
}

fn build(en: bool, w: f32) -> Layout {
    let mut out = Layout { en, w, height: 0.0, blocks: Vec::new(), sections: Vec::new() };
    let mut y = 0.0;
    for line in (if en { EN } else { PT }).lines() {
        let line = line.trim_end();
        if let Some(s) = line.strip_prefix("## ") {
            y += if out.sections.is_empty() { 0.0 } else { 22.0 };
            out.sections.push((s, y));
            add(&mut out, &mut y, Kind::Section, s, 19.0, w, 10.0);
        } else if let Some(s) = line.strip_prefix("### ") {
            y += 8.0;
            add(&mut out, &mut y, Kind::Sub, s, 15.0, w, 6.0);
        } else if let Some(s) = line.strip_prefix("- ") {
            add(&mut out, &mut y, Kind::Bullet, s, 14.0, w - 18.0, 6.0);
        } else if let Some(s) = line.strip_prefix("![") {
            let name = s.split_once("](").map_or("", |p| p.1).trim_end_matches(')');
            let Some((key, data)) = IMAGES.iter().find(|i| i.0 == name) else { continue };
            let (iw, ih) = png_size(data);
            let dw = iw.min(w);
            let h = dw * ih / iw;
            y += 6.0;
            out.blocks.push(Block { kind: Kind::Image(key, data), layout: None, y, h, w: dw });
            y += h + 14.0;
        } else if !line.is_empty() && !line.starts_with('#') && !line.starts_with('[') {
            add(&mut out, &mut y, Kind::Text, line, 14.0, w, 9.0);
        }
    }
    out.height = y + 20.0;
    out
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Guia", "O que cada função faz, o que dá e o que não dá para fazer.");
    if app.welcome.is_some() && ui.button("guide-back", rect(a.right() - 148.0, 30.0, 120.0, 36.0), "Voltar", true) {
        app.page = Page::Monitor;
        ui.s.needs_redraw = true;
        return;
    }
    let top = 88.0;
    let h = a.h - top - 20.0;
    let index = rect(a.x + 28.0, top, INDEX_W, h);
    let body_x = index.right() + 24.0;
    let body = rect(body_x, top, (a.right() - body_x - 24.0).min(MAX_W), h);

    let en = crate::i18n::is_english();
    let g = match app.guide.take() {
        Some(g) if g.en == en && g.w == body.w => g,
        _ => build(en, body.w),
    };

    ui.begin_scroll("guide", body, g.height);
    let seen = ui.scroll_y();
    for b in g.blocks.iter().filter(|b| b.y + b.h >= seen && b.y <= seen + body.h) {
        let y = body.y + b.y;
        match (&b.kind, &b.layout) {
            (Kind::Image(key, data), _) => ui.png(key, data, rect(body.x, y, b.w, b.h)),
            (Kind::Section | Kind::Sub, Some(l)) => {
                let bold = matches!(b.kind, Kind::Section);
                ui.c.draw_layout(l, body.x, y, if bold { ACCENT } else { TEXT });
            }
            (Kind::Bullet, Some(l)) => {
                ui.c.circle(body.x + 6.0, y + 10.0, 2.5, MUTED);
                ui.c.draw_layout(l, body.x + 18.0, y, TEXT2);
            }
            (_, Some(l)) => ui.c.draw_layout(l, body.x, y, TEXT2),
            _ => {}
        }
    }
    ui.end_scroll();

    let current = g.sections.iter().rposition(|s| s.1 <= seen + 8.0).unwrap_or(0);
    ui.begin_scroll("guide-index", index, g.sections.len() as f32 * INDEX_ROW);
    for (i, (title, y)) in g.sections.iter().enumerate() {
        let r = rect(index.x, index.y + i as f32 * INDEX_ROW, index.w, INDEX_ROW - 2.0);
        let on = i == current;
        if on || ui.is_hover(r) {
            ui.c.round(r, 6.0, if on { CARD2 } else { CARD });
        }
        ui.c.text(r.inset_xy(10.0, 0.0), title, 13.0, if on { WHITE } else { TEXT2 }, Align::Left, Weight::Regular);
        if ui.hit(title, r) {
            ui.set_scroll("guide", *y);
        }
    }
    ui.end_scroll();
    app.guide = Some(g);
}
