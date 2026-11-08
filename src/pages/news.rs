//! Novidades: o CHANGELOG.md, o mesmo do GitHub, desenhado aqui dentro.
//! Depois de atualizar, mostra todas as versões entre a última aberta e a atual.

use super::*;
use crate::sys::update::{current, is_newer};
use crate::ui::gfx::{layout_height, rich_layout};
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;

const LOG: &str = include_str!("../../CHANGELOG.md");
const MAX_W: f32 = 760.0;

/// Versões publicadas, da mais nova para a mais antiga. Cada item é "inglês / português".
fn versions() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    let mut out: Vec<(&str, Vec<(&str, &str)>)> = Vec::new();
    for line in LOG.lines() {
        let line = line.trim_end();
        if let Some(v) = line.strip_prefix("## ") {
            out.push((v.trim(), Vec::new()));
        } else if let (Some(item), Some(last)) = (line.strip_prefix("- "), out.last_mut()) {
            last.1.push(item.split_once(" / ").unwrap_or((item, item)));
        }
    }
    // O que ainda não saiu fica de fora: o número dele não existe.
    out.retain(|v| v.0.starts_with(|c: char| c.is_ascii_digit()));
    out
}

/// As versões depois de `from` até a atual. Vazio quando nada mudou para quem usa.
fn since(from: Option<&str>) -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    let cur = current();
    versions().into_iter().filter(|v| !is_newer(v.0, &cur) && from.is_none_or(|f| is_newer(v.0, f))).collect()
}

pub fn has_news(from: &str) -> bool {
    !since(Some(from)).is_empty()
}

enum Kind {
    Version,
    Item,
}

struct Block {
    kind: Kind,
    layout: Option<IDWriteTextLayout>,
    y: f32,
    h: f32,
}

pub struct Layout {
    en: bool,
    w: f32,
    from: Option<String>,
    height: f32,
    blocks: Vec<Block>,
}

fn add(out: &mut Layout, y: &mut f32, kind: Kind, s: &str, size: f32, width: f32, after: f32) {
    let layout = rich_layout(s, size, width);
    let h = layout.as_ref().map_or(0.0, layout_height);
    out.blocks.push(Block { kind, layout, y: *y, h });
    *y += h + after;
}

fn build(en: bool, w: f32, from: Option<String>) -> Layout {
    let list = since(from.as_deref());
    // Folga para a barra de rolagem, que fica na borda direita.
    let w = w - 16.0;
    let mut out = Layout { en, w: w + 16.0, from, height: 0.0, blocks: Vec::new() };
    let mut y = 0.0;
    for (i, (v, items)) in list.iter().enumerate() {
        y += if i == 0 { 0.0 } else { 22.0 };
        add(&mut out, &mut y, Kind::Version, &crate::i18n::tr(&format!("Versão {v}")), 19.0, w, 10.0);
        for (item_en, item_pt) in items {
            add(&mut out, &mut y, Kind::Item, if en { item_en } else { item_pt }, 14.0, w - 18.0, 8.0);
        }
    }
    out.height = y + 20.0;
    out
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let sub = match &app.news_from {
        Some(f) => format!("O que mudou da versão {f} para a {}", current()),
        None => "O que mudou em cada versão".to_string(),
    };
    header_room(ui, a, "Novidades", &sub, 400.0);
    if ui.button("news-ok", rect(a.right() - 148.0, 30.0, 120.0, 36.0), "Continuar", true) {
        app.page = Page::Monitor;
        app.news_from = None;
        app.news = None;
        ui.s.needs_redraw = true;
        return;
    }
    if app.news_from.is_some() && ui.button("news-all", rect(a.right() - 360.0, 30.0, 200.0, 36.0), "Ver todas as versões", false) {
        app.news_from = None;
        ui.reset_scroll("news");
    }
    let top = 88.0;
    let body = rect(a.x + 28.0, top, (a.w - 56.0).min(MAX_W), a.h - top - 20.0);
    let en = crate::i18n::is_english();
    let n = match app.news.take() {
        Some(n) if n.en == en && n.w == body.w && n.from == app.news_from => n,
        _ => build(en, body.w, app.news_from.clone()),
    };
    ui.begin_scroll("news", body, n.height);
    let seen = ui.scroll_y();
    for b in n.blocks.iter().filter(|b| b.y + b.h >= seen && b.y <= seen + body.h) {
        let Some(l) = &b.layout else { continue };
        let y = body.y + b.y;
        match b.kind {
            Kind::Version => ui.c.draw_layout(l, body.x, y, ACCENT),
            Kind::Item => {
                ui.c.circle(body.x + 6.0, y + 10.0, 2.5, MUTED);
                ui.c.draw_layout(l, body.x + 18.0, y, TEXT2);
            }
        }
    }
    ui.end_scroll();
    app.news = Some(n);
}
