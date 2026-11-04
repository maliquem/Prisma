//! Telas da janela principal, na direção visual Grafite.

mod adjust;
mod analyze;
mod capture;
mod energy;
mod fullscreen;
mod monitor;
mod optimize;
mod services;
mod startup;
mod storage;
mod tools;
mod tweaks;
mod uninstall;
mod volume;
mod widget;

use crate::app::App;
use crate::ui::gfx::*;
use crate::ui::Ui;

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Monitor,
    Storage,
    Analyze,
    Startup,
    Services,
    Uninstall,
    Tweaks,
    Tools,
    Optimize,
    Energy,
    Fullscreen,
    Volume,
    Capture,
    Widget,
    Adjust,
}

type Icon = &'static [&'static [(f32, f32)]];

const ICON_MON: Icon = &[&[(3., 12.), (7., 12.), (10., 20.), (14., 4.), (17., 12.), (21., 12.)]];
const ICON_OPT: Icon = &[&[(13., 3.), (5., 14.), (11., 14.), (10., 21.), (18., 10.), (12., 10.), (13., 3.)]];
const ICON_POW: Icon = &[
    &[(12., 3.), (12., 11.)],
    &[(7., 6.5), (4.5, 9.5), (4., 13.), (5.5, 17.), (9., 20.), (12., 20.5), (15., 20.), (18.5, 17.), (20., 13.), (19.5, 9.5), (17., 6.5)],
];
const ICON_WIN: Icon = &[
    &[(4., 9.), (4., 4.), (9., 4.)],
    &[(20., 9.), (20., 4.), (15., 4.)],
    &[(4., 15.), (4., 20.), (9., 20.)],
    &[(20., 15.), (20., 20.), (15., 20.)],
];
const ICON_VOL: Icon = &[
    &[(4., 9.), (4., 15.), (8., 15.), (13., 19.), (13., 5.), (8., 9.), (4., 9.)],
    &[(16.5, 8.5), (18., 12.), (16.5, 15.5)],
];
const ICON_CAP: Icon = &[
    &[(4., 8.), (8., 8.), (10., 5.), (14., 5.), (16., 8.), (20., 8.), (20., 19.), (4., 19.), (4., 8.)],
    &[(12., 10.), (14.5, 11.), (15., 13.), (14.5, 15.), (12., 16.), (9.5, 15.), (9., 13.), (9.5, 11.), (12., 10.)],
];
const ICON_WID: Icon = &[
    &[(4., 4.), (11., 4.), (11., 11.), (4., 11.), (4., 4.)],
    &[(13., 4.), (20., 4.), (20., 11.), (13., 11.), (13., 4.)],
    &[(4., 13.), (11., 13.), (11., 20.), (4., 20.), (4., 13.)],
    &[(13., 13.), (20., 13.), (20., 20.), (13., 20.), (13., 13.)],
];
const ICON_ADJ: Icon = &[
    &[(4., 7.), (14., 7.)],
    &[(18., 7.), (20., 7.)],
    &[(16., 5.), (16., 9.)],
    &[(4., 17.), (6., 17.)],
    &[(10., 17.), (20., 17.)],
    &[(8., 15.), (8., 19.)],
];

const ICON_DISK: Icon = &[
    &[(4., 6.), (20., 6.), (20., 18.), (4., 18.), (4., 6.)],
    &[(4., 12.), (20., 12.)],
    &[(16., 15.), (17., 15.)],
    &[(16., 9.), (17., 9.)],
];

const ICON_MAP: Icon = &[
    &[(4., 4.), (20., 4.), (20., 20.), (4., 20.), (4., 4.)],
    &[(12., 4.), (12., 20.)],
    &[(12., 12.), (20., 12.)],
];
const ICON_DEL: Icon = &[
    &[(5., 7.), (19., 7.)],
    &[(9., 7.), (10., 4.), (14., 4.), (15., 7.)],
    &[(7., 7.), (8., 20.), (16., 20.), (17., 7.)],
];
const ICON_TW: Icon = &[
    &[(12., 3.), (19., 6.), (19., 12.), (16., 18.), (12., 21.), (8., 18.), (5., 12.), (5., 6.), (12., 3.)],
    &[(9., 12.), (11., 14.), (15., 10.)],
];
const ICON_TOOL: Icon = &[
    &[(5., 19.), (13., 11.)],
    &[(13., 11.), (12., 7.), (15., 4.), (19., 4.), (16., 7.), (17., 8.), (20., 5.), (20., 9.), (17., 12.), (13., 11.)],
];
const ICON_BOOT: Icon = &[&[(12., 4.), (12., 16.)], &[(7., 9.), (12., 4.), (17., 9.)], &[(5., 20.), (19., 20.)]];
const ICON_SVC: Icon = &[
    &[(4., 5.), (20., 5.), (20., 10.), (4., 10.), (4., 5.)],
    &[(4., 14.), (20., 14.), (20., 19.), (4., 19.), (4., 14.)],
    &[(7., 7.5), (8., 7.5)],
    &[(7., 16.5), (8., 16.5)],
];

const NAV: &[(Page, &str, Icon)] = &[
    (Page::Monitor, "Monitor", ICON_MON),
    (Page::Storage, "Discos e redes", ICON_DISK),
    (Page::Analyze, "Espaço em disco", ICON_MAP),
    (Page::Startup, "Inicialização", ICON_BOOT),
    (Page::Services, "Serviços", ICON_SVC),
    (Page::Uninstall, "Desinstalar", ICON_DEL),
    (Page::Tweaks, "Ajustes do Windows", ICON_TW),
    (Page::Tools, "Ferramentas", ICON_TOOL),
    (Page::Optimize, "Otimizar", ICON_OPT),
    (Page::Energy, "Energia", ICON_POW),
    (Page::Fullscreen, "Fullscreenizer", ICON_WIN),
    (Page::Volume, "Volume", ICON_VOL),
    (Page::Capture, "Captura", ICON_CAP),
    (Page::Widget, "Widget", ICON_WID),
    (Page::Adjust, "Configurações", ICON_ADJ),
];

const SIDE_W: f32 = 216.0;

pub fn draw(mut ui: Ui, app: &mut App, full: Rect) {
    let side = rect(0.0, 0.0, SIDE_W, full.h);
    ui.c.fill(side, SIDE);
    let c = ui.c;
    c.icon(24.0, 20.0, 22.0, &[&[(12., 3.), (3., 20.), (21., 20.), (12., 3.)], &[(12., 3.), (12., 20.)]], ACCENT);
    c.text(rect(56.0, 18.0, 150.0, 26.0), "Prisma", 16.0, TEXT, Align::Left, Weight::Semibold);

    let mut y = 58.0;
    for (page, label, icon) in NAV {
        let r = rect(12.0, y, SIDE_W - 24.0, 32.0);
        let current = app.page == *page;
        let hover = ui.is_hover(r);
        if current || hover {
            ui.c.round(r, 8.0, if current { CARD2 } else { CARD });
        }
        let color = if current { WHITE } else { TEXT2 };
        ui.c.icon(r.x + 12.0, r.y + 7.0, 18.0, icon, color);
        ui.c.text(rect(r.x + 42.0, r.y, r.w - 48.0, r.h), label, 14.0, color, Align::Left, Weight::Regular);
        if ui.hit(label, r) && !current {
            app.page = *page;
            app.refresh_page();
            ui.s.needs_redraw = true;
        }
        y += 33.0;
    }

    let chip = rect(12.0, full.h - 88.0, SIDE_W - 24.0, 76.0);
    ui.c.round(chip, 10.0, CARD);
    let plan = app.plans.iter().find(|p| p.id == app.active_plan).map(|p| p.name.as_str()).unwrap_or("Plano desconhecido");
    let state = if !app.cfg.power_auto {
        "Troca automática desligada"
    } else {
        match app.detector.mode {
            crate::sys::power::Mode::Rest => "Repouso",
            crate::sys::power::Mode::Load => "Carga alta",
            crate::sys::power::Mode::Game => "Modo jogo ativo",
        }
    };
    let cx = chip.x + 12.0;
    ui.c.text(rect(cx, chip.y + 8.0, chip.w - 24.0, 18.0), "Energia automática", 12.0, MUTED, Align::Left, Weight::Regular);
    ui.c.text(rect(cx, chip.y + 27.0, chip.w - 24.0, 20.0), plan, 14.0, TEXT, Align::Left, Weight::Semibold);
    ui.c.text(rect(cx, chip.y + 48.0, chip.w - 24.0, 18.0), state, 12.0, ACCENT, Align::Left, Weight::Regular);

    let area = rect(SIDE_W, 0.0, full.w - SIDE_W, full.h);
    match app.page {
        Page::Monitor => monitor::draw(&mut ui, app, area),
        Page::Storage => storage::draw(&mut ui, app, area),
        Page::Analyze => analyze::draw(&mut ui, app, area),
        Page::Startup => startup::draw(&mut ui, app, area),
        Page::Services => services::draw(&mut ui, app, area),
        Page::Uninstall => uninstall::draw(&mut ui, app, area),
        Page::Tweaks => tweaks::draw(&mut ui, app, area),
        Page::Tools => tools::draw(&mut ui, app, area),
        Page::Optimize => optimize::draw(&mut ui, app, area),
        Page::Energy => energy::draw(&mut ui, app, area),
        Page::Fullscreen => fullscreen::draw(&mut ui, app, area),
        Page::Volume => volume::draw(&mut ui, app, area),
        Page::Capture => capture::draw(&mut ui, app, area),
        Page::Widget => widget::draw(&mut ui, app, area),
        Page::Adjust => adjust::draw(&mut ui, app, area),
    }
    ui.end();
}

pub fn header(ui: &Ui, a: Rect, title: &str, sub: &str) {
    let x = a.x + 28.0;
    ui.c.text(rect(x, 22.0, a.w - 56.0, 32.0), title, 24.0, TEXT, Align::Left, Weight::Semibold);
    ui.c.text(rect(x, 54.0, a.w - 240.0, 20.0), sub, 14.0, MUTED, Align::Left, Weight::Regular);
}

pub fn fmt_gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / (1u64 << 30) as f64).replace('.', crate::i18n::comma())
}

pub fn fmt_mem(b: u64) -> String {
    if b >= 1 << 30 {
        fmt_gb(b)
    } else {
        format!("{} MB", b >> 20)
    }
}

pub fn fmt_rate(b: u64) -> String {
    if b == 0 {
        "0".into()
    } else if b >= 1 << 30 {
        format!("{:.1} GB/s", b as f64 / (1u64 << 30) as f64).replace('.', crate::i18n::comma())
    } else if b >= 1 << 20 {
        format!("{:.1} MB/s", b as f64 / (1u64 << 20) as f64).replace('.', crate::i18n::comma())
    } else {
        format!("{} KB/s", (b >> 10).max(1))
    }
}

pub fn fmt_pct(v: f32) -> String {
    if v >= 10.0 || v == 0.0 {
        format!("{v:.0}%")
    } else {
        format!("{v:.1}%").replace('.', crate::i18n::comma())
    }
}

pub fn app_name(exe: &str) -> &str {
    exe.strip_suffix(".exe").or(exe.strip_suffix(".EXE")).unwrap_or(exe)
}

/// Quebra uma linha longa, de preferência em espaço ou barra.
fn wrap_line(line: &str, cols: usize) -> Vec<String> {
    let mut c: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    while c.len() > cols {
        let cut = c[cols / 2..cols].iter().rposition(|x| *x == ' ' || *x == '\\').map_or(cols, |i| cols / 2 + i + 1);
        out.push(c[..cut].iter().collect());
        let mut rest = vec![' '; 4];
        rest.extend_from_slice(&c[cut..]);
        c = rest;
    }
    out.push(c.into_iter().collect());
    out
}

/// Passo a passo de uma tarefa: rola, acompanha o fim e nunca passa da borda.
pub fn log_view(ui: &mut Ui, key: &str, r: Rect, lines: &[String]) {
    const ROW: f32 = 19.0;
    let cols = ((r.w / 7.0) as usize).max(24);
    let rows: Vec<(String, Color, Weight)> = lines
        .iter()
        .flat_map(|l| {
            let l = crate::i18n::tr(l);
            let t = l.trim_start();
            let (color, weight) = if t.starts_with('▸') {
                (TEXT, Weight::Semibold)
            } else if t.starts_with('✗') {
                (WARN, Weight::Regular)
            } else if t.starts_with('✓') {
                (ACCENT, Weight::Regular)
            } else {
                (TEXT2, Weight::Regular)
            };
            wrap_line(&l, cols).into_iter().map(move |s| (s, color, weight))
        })
        .collect();
    ui.stick_bottom(key);
    ui.begin_scroll(key, r, rows.len() as f32 * ROW);
    let first = (ui.scroll_y() / ROW) as usize;
    let last = first + (r.h / ROW) as usize + 2;
    for (i, (s, color, weight)) in rows.iter().enumerate().take(last).skip(first) {
        ui.c.text(rect(r.x, r.y + i as f32 * ROW, r.w, ROW), s, 12.0, *color, Align::Left, *weight);
    }
    ui.end_scroll();
}
