//! Telas da janela principal, na direção visual Grafite.

mod adjust;
mod analyze;
mod capture;
pub mod copies;
pub mod controllers;
pub mod pad_profiles;
mod energy;
mod fullscreen;
pub mod guide;
mod hdr;
mod meter;
mod monitor;
pub mod news;
pub mod nvidia;
mod optimize;
pub mod picker;
mod plan_edit;
pub mod report;
pub mod search;
mod services;
#[cfg(feature = "stream")]
pub mod share;
/// Cópia compilada sem o compartilhamento (`--no-default-features`).
#[cfg(not(feature = "stream"))]
pub mod share {
    use super::*;

    pub fn draw(ui: &mut Ui, _app: &mut App, a: Rect) {
        header(ui, a, "Compartilhar tela", "Esta cópia do Prisma foi compilada sem o compartilhamento.");
    }

    pub fn shutdown() {}

    pub fn tick(_app: &mut App) {}

    pub fn open_room() {}
}
mod startup;
mod storage;
mod tools;
mod tweaks;
mod uninstall;
mod volume;
pub mod welcome;
mod widget;

use crate::app::App;
use crate::ui::gfx::*;
use crate::ui::Ui;

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Monitor,
    Meter,
    Storage,
    Analyze,
    Search,
    Startup,
    Services,
    Uninstall,
    Tweaks,
    Tools,
    Copies,
    Optimize,
    Energy,
    Fullscreen,
    Volume,
    Hdr,
    Nvidia,
    Capture,
    Share,
    Controllers,
    Widget,
    Adjust,
    Report,
    Guide,
    News,
}

pub type Icon = &'static [&'static [(f32, f32)]];

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
const ICON_BUG: Icon = &[
    &[(9., 9.), (15., 9.), (17., 12.), (17., 16.), (15., 19.), (12., 20.), (9., 19.), (7., 16.), (7., 12.), (9., 9.)],
    &[(12., 9.), (12., 20.)],
    &[(9.5, 9.), (8., 5.)],
    &[(14.5, 9.), (16., 5.)],
    &[(7., 13.), (3., 12.)],
    &[(17., 13.), (21., 12.)],
    &[(7.5, 17.), (4., 19.)],
    &[(16.5, 17.), (20., 19.)],
];
const ICON_HELP: Icon = &[
    &[(12., 3.), (16.5, 4.2), (19.8, 7.5), (21., 12.), (19.8, 16.5), (16.5, 19.8), (12., 21.), (7.5, 19.8), (4.2, 16.5), (3., 12.), (4.2, 7.5), (7.5, 4.2), (12., 3.)],
    &[(9.5, 9.5), (10., 8.), (12., 7.2), (14., 8.), (14.5, 10.), (13., 11.5), (12., 12.5), (12., 14.)],
    &[(12., 16.6), (12., 17.)],
];
const ICON_HDR: Icon = &[
    &[(12., 3.), (12., 5.)],
    &[(12., 19.), (12., 21.)],
    &[(3., 12.), (5., 12.)],
    &[(19., 12.), (21., 12.)],
    &[(5.6, 5.6), (7., 7.)],
    &[(17., 17.), (18.4, 18.4)],
    &[(5.6, 18.4), (7., 17.)],
    &[(17., 7.), (18.4, 5.6)],
    &[(12., 8.), (15., 9.), (16., 12.), (15., 15.), (12., 16.), (9., 15.), (8., 12.), (9., 9.), (12., 8.)],
];
const ICON_METER: Icon = &[
    &[(4., 17.), (5., 12.), (8., 8.), (12., 6.), (16., 8.), (19., 12.), (20., 17.)],
    &[(12., 16.), (16., 10.)],
    &[(9., 18.), (15., 18.)],
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

const ICON_GPU: Icon = &[
    &[(3., 7.), (21., 7.), (21., 17.), (3., 17.), (3., 7.)],
    &[(9., 9.5), (11.5, 10.), (12.5, 12.), (11.5, 14.), (9., 14.5), (6.5, 14.), (5.5, 12.), (6.5, 10.), (9., 9.5)],
    &[(15., 10.), (19., 10.)],
    &[(15., 14.), (19., 14.)],
    &[(6., 17.), (6., 20.)],
    &[(10., 17.), (10., 20.)],
];

const ICON_PAD: Icon = &[
    &[(6., 8.), (18., 8.), (21., 15.), (20., 18.), (17., 18.), (15., 15.), (9., 15.), (7., 18.), (4., 18.), (3., 15.), (6., 8.)],
    &[(6.5, 11.5), (10.5, 11.5)],
    &[(8.5, 9.5), (8.5, 13.5)],
    &[(15., 10.5), (15., 11.5)],
    &[(17.5, 11.5), (17.5, 12.5)],
];
const ICON_SHARE: Icon = &[
    &[(3., 5.), (21., 5.), (21., 16.), (3., 16.), (3., 5.)],
    &[(9., 20.), (15., 20.)],
    &[(12., 16.), (12., 20.)],
    &[(9., 12.), (12., 8.5), (15., 12.)],
    &[(12., 8.5), (12., 13.5)],
];

const ICON_DISK: Icon = &[
    &[(4., 6.), (20., 6.), (20., 18.), (4., 18.), (4., 6.)],
    &[(4., 12.), (20., 12.)],
    &[(16., 15.), (17., 15.)],
    &[(16., 9.), (17., 9.)],
];

const ICON_FIND: Icon = &[
    &[(17., 10.5), (16.1, 13.8), (13.8, 16.1), (10.5, 17.), (7.2, 16.1), (4.9, 13.8), (4., 10.5), (4.9, 7.2), (7.2, 4.9), (10.5, 4.), (13.8, 4.9), (16.1, 7.2), (17., 10.5)],
    &[(15.2, 15.2), (20., 20.)],
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
const ICON_COPY: Icon = &[&[(8., 8.), (20., 8.), (20., 20.), (8., 20.), (8., 8.)], &[(16., 8.), (16., 4.), (4., 4.), (4., 16.), (8., 16.)]];
const ICON_BOOT: Icon = &[&[(12., 4.), (12., 16.)], &[(7., 9.), (12., 4.), (17., 9.)], &[(5., 20.), (19., 20.)]];
const ICON_SVC: Icon = &[
    &[(4., 5.), (20., 5.), (20., 10.), (4., 10.), (4., 5.)],
    &[(4., 14.), (20., 14.), (20., 19.), (4., 19.), (4., 14.)],
    &[(7., 7.5), (8., 7.5)],
    &[(7., 16.5), (8., 16.5)],
];

const ICON_REC: Icon = &[
    &[(12., 3.), (16.5, 4.2), (19.8, 7.5), (21., 12.), (19.8, 16.5), (16.5, 19.8), (12., 21.), (7.5, 19.8), (4.2, 16.5), (3., 12.), (4.2, 7.5), (7.5, 4.2), (12., 3.)],
    &[(12., 9.), (14., 10.), (15., 12.), (14., 14.), (12., 15.), (10., 14.), (9., 12.), (10., 10.), (12., 9.)],
];
const ICON_AGAIN: Icon = &[&[(18., 8.), (16., 5.5), (12., 4.), (8., 5.), (5., 8.), (4., 12.), (5., 16.), (8., 19.), (12., 20.), (16., 19.), (19., 16.)], &[(18., 3.), (18., 8.), (13., 8.)]];
const ICON_X: Icon = &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]];
const ICON_BT: Icon = &[&[(7., 7.5), (17., 16.5), (12., 21.), (12., 3.), (17., 7.5), (7., 16.5)]];
const ICON_BAT: Icon = &[&[(3., 8.), (18., 8.), (18., 16.), (3., 16.), (3., 8.)], &[(21., 10.5), (21., 13.5)], &[(6., 11.), (6., 13.)], &[(9., 11.), (9., 13.)]];
const ICON_KEYS: Icon = &[
    &[(3., 6.), (21., 6.), (21., 18.), (3., 18.), (3., 6.)],
    &[(6., 9.5), (7., 9.5)],
    &[(10., 9.5), (11., 9.5)],
    &[(14., 9.5), (15., 9.5)],
    &[(17.5, 9.5), (18.5, 9.5)],
    &[(8., 14.5), (16., 14.5)],
];
const ICON_APP: Icon = &[
    &[(3., 5.), (21., 5.), (21., 19.), (3., 19.), (3., 5.)],
    &[(3., 9.), (21., 9.)],
    &[(10., 16.), (15., 12.)],
    &[(11.5, 12.), (15., 12.), (15., 15.5)],
];

/// Página `i` da barra lateral, para as ações "Página ..." (`sys::remap::PAGES`).
pub fn nav_page(i: usize) -> Option<(Page, Icon)> {
    action_nav().nth(i).map(|n| (n.0, n.2))
}

/// A barra lateral na ordem das ações "Página ...": Cópias entrou depois, então fica no fim e os
/// números das ações já gravadas não mudam.
fn action_nav() -> impl Iterator<Item = &'static (Page, &'static str, Icon)> {
    NAV.iter().filter(|n| n.0 != Page::Copies).chain(NAV.iter().filter(|n| n.0 == Page::Copies))
}

/// Ícone de uma ação do Prisma (`sys::remap::ACTIONS`), ou de abrir um programa (`sys::radial::PROGRAM`).
pub fn action_icon(id: u8) -> Icon {
    match id {
        0..=2 => ICON_CAP,
        3 | 4 => ICON_REC,
        5 | 22 => ICON_AGAIN,
        6..=10 => ICON_METER,
        11..=13 => ICON_MON,
        14 => ICON_HDR,
        15..=17 => ICON_VOL,
        18 => ICON_WIN,
        19 => ICON_WID,
        20 => ICON_OPT,
        21 => ICON_X,
        23 => ICON_FIND,
        24 => ICON_BT,
        25 => ICON_BAT,
        26 => ICON_KEYS,
        28 => ICON_ADJ,
        29..=31 => ICON_POW,
        32 => ICON_SHARE,
        n if n >= crate::sys::remap::PAGES && n < crate::sys::radial::PROFILES => nav_page((n - crate::sys::remap::PAGES) as usize).map_or(ICON_PAD, |p| p.1),
        crate::sys::radial::PROGRAM => ICON_APP,
        crate::sys::radial::PROFILES => ICON_PAD,
        _ => ICON_PAD,
    }
}

/// Página seguinte (`d` = 1) ou anterior (-1) da barra lateral, dando a volta; de fora dela (Guia,
/// Reportar bug), a primeira ou a última.
pub fn nav_step(page: Page, d: i8) -> Page {
    let nav: Vec<Page> = NAV.iter().map(|n| n.0).filter(|&p| p != Page::Nvidia || nvidia::available()).collect();
    let i = nav.iter().position(|&p| p == page).map_or(if d > 0 { -1 } else { 0 }, |i| i as isize);
    nav[(i + d as isize).rem_euclid(nav.len() as isize) as usize]
}

const NAV: &[(Page, &str, Icon)] = &[
    (Page::Monitor, "Monitor", ICON_MON),
    (Page::Meter, "Medidor", ICON_METER),
    (Page::Storage, "Discos e redes", ICON_DISK),
    (Page::Analyze, "Espaço em disco", ICON_MAP),
    (Page::Search, "Pesquisar", ICON_FIND),
    (Page::Startup, "Inicialização", ICON_BOOT),
    (Page::Services, "Serviços", ICON_SVC),
    (Page::Uninstall, "Desinstalar", ICON_DEL),
    (Page::Tweaks, "Ajustes do Windows", ICON_TW),
    (Page::Tools, "Ferramentas", ICON_TOOL),
    (Page::Copies, "Cópias", ICON_COPY),
    (Page::Optimize, "Otimizar", ICON_OPT),
    (Page::Energy, "Energia", ICON_POW),
    (Page::Fullscreen, "Fullscreenizer", ICON_WIN),
    (Page::Volume, "Som", ICON_VOL),
    (Page::Hdr, "HDR", ICON_HDR),
    (Page::Nvidia, "Driver NVIDIA", ICON_GPU),
    (Page::Capture, "Captura", ICON_CAP),
    (Page::Share, "Compartilhar tela", ICON_SHARE),
    (Page::Controllers, "Controles", ICON_PAD),
    (Page::Widget, "Widget", ICON_WID),
    (Page::Adjust, "Configurações", ICON_ADJ),
];

/// Ajuda fora da lista de funções: ao lado do nome, sempre à vista.
const HELP: &[(Page, &str, Icon)] = &[(Page::Guide, "Guia e tutorial", ICON_HELP), (Page::Report, "Reportar bug", ICON_BUG)];

const SIDE_W: f32 = 216.0;
const KOFI_URL: &str = "https://ko-fi.com/maliquem";
const KOFI_ICON: &[u8] = include_bytes!("../../res/kofi.bgra");

pub fn draw(mut ui: Ui, app: &mut App, full: Rect) {
    // O guia abre por cima das boas-vindas, que continuam esperando a resposta.
    if app.welcome.is_some() && app.page != Page::Guide {
        if let Some(t) = &mut app.report.text {
            t.hide();
        }
        welcome::draw(&mut ui, app, full);
        ui.end();
        return;
    }
    let side = rect(0.0, 0.0, SIDE_W, full.h);
    ui.c.fill(side, SIDE);
    let c = ui.c;
    c.icon(24.0, 20.0, 22.0, &[&[(12., 3.), (3., 20.), (21., 20.), (12., 3.)], &[(12., 3.), (12., 20.)]], ACCENT);
    c.text(rect(56.0, 18.0, 70.0, 26.0), "Prisma", 16.0, TEXT, Align::Left, Weight::Semibold);
    for (i, (page, tip, icon)) in HELP.iter().enumerate() {
        let r = rect(SIDE_W - 84.0 + i as f32 * 36.0, 16.0, 30.0, 30.0);
        let current = app.page == *page;
        ui.c.round(r, 8.0, if current { ACCENT } else if ui.is_hover(r) { CARD2 } else { CARD });
        ui.c.icon(r.x + 6.0, r.y + 6.0, 18.0, icon, if current { ON_ACCENT } else { ACCENT });
        ui.tip(r, tip);
        if ui.hit(&format!("nav-help-{tip}"), r) && !current {
            if *page == Page::Report {
                app.report.area = report::area_of(app.page);
            }
            app.page = *page;
            app.refresh_page();
            ui.s.needs_redraw = true;
        }
    }

    let mut y = 58.0;
    // Em janela baixa as linhas apertam para não entrar no cartão de energia.
    // Driver NVIDIA só aparece com placa NVIDIA.
    let nav: Vec<_> = NAV.iter().filter(|n| n.0 != Page::Nvidia || nvidia::available()).collect();
    let step = ((full.h - 140.0 - y) / nav.len() as f32).min(33.0).floor();
    for (page, label, icon) in nav {
        let r = rect(12.0, y, SIDE_W - 24.0, step - 1.0);
        let current = app.page == *page;
        let hover = ui.is_hover(r);
        if current || hover {
            ui.c.round(r, 8.0, if current { CARD2 } else { CARD });
        }
        let color = if current { WHITE } else { TEXT2 };
        ui.c.icon(r.x + 12.0, r.y + (r.h - 18.0) / 2.0, 18.0, icon, color);
        ui.c.text(rect(r.x + 42.0, r.y, r.w - 48.0, r.h), label, 14.0, color, Align::Left, Weight::Regular);
        if ui.hit(&format!("nav-{label}"), r) && !current {
            crate::report::note(&format!("page: {label}"));
            app.page = *page;
            app.refresh_page();
            ui.s.needs_redraw = true;
        }
        y += step;
    }

    let coffee = rect(12.0, full.h - 128.0, SIDE_W - 24.0, 34.0);
    ui.c.round(coffee, 8.0, if ui.is_hover(coffee) { CARD2 } else { CARD });
    ui.image("kofi", 64, KOFI_ICON, rect(coffee.x + 10.0, coffee.y + 6.0, 22.0, 22.0));
    ui.c.text(rect(coffee.x + 42.0, coffee.y, coffee.w - 48.0, coffee.h), "Buy me a coffee", 14.0, TEXT, Align::Left, Weight::Regular);
    if ui.hit("kofi", coffee) {
        crate::report::open_url(KOFI_URL);
    }

    let chip = rect(12.0, full.h - 88.0, SIDE_W - 24.0, 76.0);
    ui.c.round(chip, 10.0, CARD);
    let plan = app.plans.iter().find(|p| p.id == app.active_plan).map(|p| p.name.as_str()).unwrap_or("Plano desconhecido");
    let state = if !app.cfg.power_auto && !app.cfg.plan_fixed.is_empty() {
        "Plano fixo"
    } else if !app.cfg.power_auto {
        "Troca desligada"
    } else {
        match app.detector.mode {
            crate::sys::power::Mode::Rest => "Repouso",
            crate::sys::power::Mode::Load => "Carga alta",
            crate::sys::power::Mode::Game => "Modo jogo ativo",
        }
    };
    let cx = chip.x + 12.0;
    ui.c.text(rect(cx, chip.y + 8.0, chip.w - 24.0, 18.0), "Energia automática", 12.0, MUTED, Align::Left, Weight::Regular);
    // O nome do plano vem do Windows (e pode ser traduzido): diminui até caber.
    ui.c.text_fit(rect(cx, chip.y + 27.0, chip.w - 24.0, 20.0), plan, 14.0, TEXT, Align::Left, Weight::Semibold);
    ui.c.text(rect(cx, chip.y + 48.0, chip.w - 24.0, 18.0), state, 12.0, ACCENT, Align::Left, Weight::Regular);

    let area = rect(SIDE_W, 0.0, full.w - SIDE_W, full.h);
    if app.page != Page::Report {
        if let Some(t) = &mut app.report.text {
            t.hide();
        }
    }
    match app.page {
        Page::Monitor => monitor::draw(&mut ui, app, area),
        Page::Meter => meter::draw(&mut ui, app, area),
        Page::Storage => storage::draw(&mut ui, app, area),
        Page::Analyze => analyze::draw(&mut ui, app, area),
        Page::Search => search::draw(&mut ui, app, area),
        Page::Startup => startup::draw(&mut ui, app, area),
        Page::Services => services::draw(&mut ui, app, area),
        Page::Uninstall => uninstall::draw(&mut ui, app, area),
        Page::Tweaks => tweaks::draw(&mut ui, app, area),
        Page::Tools => tools::draw(&mut ui, app, area),
        Page::Copies => copies::draw(&mut ui, app, area),
        Page::Optimize => optimize::draw(&mut ui, app, area),
        Page::Energy => energy::draw(&mut ui, app, area),
        Page::Fullscreen => fullscreen::draw(&mut ui, app, area),
        Page::Volume => volume::draw(&mut ui, app, area),
        Page::Hdr => hdr::draw(&mut ui, app, area),
        Page::Nvidia => nvidia::draw(&mut ui, app, area),
        Page::Capture => capture::draw(&mut ui, app, area),
        Page::Share => share::draw(&mut ui, app, area),
        Page::Controllers => controllers::draw(&mut ui, app, area),
        Page::Widget => widget::draw(&mut ui, app, area),
        Page::Adjust => adjust::draw(&mut ui, app, area),
        Page::Report => report::draw(&mut ui, app, area),
        Page::Guide => guide::draw(&mut ui, app, area),
        Page::News => news::draw(&mut ui, app, area),
    }
    update_prompt(&mut ui, app);
    hotkey_prompt(&mut ui, app);
    ui.end();
}

/// Atalho recém-gravado que já tinha dono: outra função do Prisma ou outro programa.
fn hotkey_prompt(ui: &mut Ui, app: &mut App) {
    use crate::app::HkAsk;
    if let Some(a) = app.hk_ask.take() {
        match a {
            HkAsk::Swap { hk, other, .. } => {
                ui.ask("hk-swap", "Trocar o atalho?", &format!("{} já é o atalho de {}. Sim tira de lá e coloca aqui.", hk.label(), app.hk_name(other)));
            }
            HkAsk::Taken(hk) => {
                ui.ask("hk-taken", "Escolher outro atalho?", &format!("{} já é usado por outro programa, e o Windows não diz qual. Com essa combinação, o atalho não funciona no Prisma.", hk.label()));
            }
        }
        app.hk_asked = Some(a);
    }
    if ui.answered("hk-swap") {
        if let Some(HkAsk::Swap { id, hk, other }) = app.hk_asked {
            app.hk_swap(id, hk, other);
        }
    }
    if ui.answered("hk-taken") {
        // Abre de novo o campo que acabou de gravar.
        ui.s.capturing = ui.s.last_hotkey;
        ui.s.needs_redraw = true;
    }
}

/// Só chega aqui com a janela aberta: é a única hora em que o programa pergunta.
fn update_prompt(ui: &mut Ui, app: &mut App) {
    if app.upd_ask && !ui.s.covered() {
        app.upd_ask = false;
        let v = app.upd.lock().unwrap().version.clone();
        ui.ask_check("upd-new", "Nova atualização disponível", &format!("A versão {v} está pronta. Quer atualizar agora?"), "Não mostrar de novo");
    }
    if ui.ask_mark("upd-new") == Some(true) {
        app.cfg.update_auto = false;
        app.cfg.save();
    }
    if ui.answered("upd-new") {
        // O andamento aparece em Configurações.
        app.page = Page::Adjust;
        app.update_now();
        ui.s.needs_redraw = true;
    }
}

/// Botão Padrão: pergunta antes e volta só as opções daquela tela.
pub fn defaults_button(ui: &mut Ui, app: &mut App, x: f32, page: crate::config::Defaults, body: &str) {
    if ui.button("defaults-btn", rect(x, 30.0, 96.0, 36.0), "Padrão", false) {
        ui.ask("defaults", "Voltar ao padrão?", body);
    }
    if ui.answered("defaults") {
        app.reset_defaults(page);
    }
}

pub fn header(ui: &Ui, a: Rect, title: &str, sub: &str) {
    header_room(ui, a, title, sub, 240.0);
}

/// Cabeçalho que deixa `room` livre à direita, para os botões da página.
pub fn header_room(ui: &Ui, a: Rect, title: &str, sub: &str, room: f32) {
    let x = a.x + 28.0;
    ui.c.text(rect(x, 22.0, a.w - room - 28.0, 32.0), title, 24.0, TEXT, Align::Left, Weight::Semibold);
    ui.c.text(rect(x, 54.0, a.w - room - 28.0, 20.0), sub, 14.0, MUTED, Align::Left, Weight::Regular);
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

#[cfg(test)]
mod tests {
    #[test]
    fn page_actions_follow_the_sidebar() {
        use crate::sys::remap::{ACTIONS, PAGES};
        let pages = &ACTIONS[PAGES as usize..];
        assert_eq!(pages.len(), super::NAV.len());
        for (a, n) in pages.iter().zip(super::action_nav()) {
            assert_eq!(*a, format!("Página {}", n.1));
        }
    }
}
