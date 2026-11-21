//! Telas da janela principal, na direção visual Grafite.

mod adjust;
mod analyze;
mod capture;
pub mod copies;
pub mod updates;
pub mod controllers;
pub mod pad_profiles;
mod pad_hide;
mod energy;
pub mod equalizer;
mod fullscreen;
pub mod games;
pub mod guide;
pub(crate) mod hdr;
pub mod meter;
mod monitor;
mod perf;
pub mod news;
pub mod nvidia;
mod optimize;
pub mod picker;
mod plan_edit;
pub mod report;
pub mod search;
mod services;
pub mod shaders;
#[cfg(feature = "stream")]
pub mod share;
pub mod shortcuts;
/// Cópia compilada sem o compartilhamento (`--no-default-features`).
#[cfg(not(feature = "stream"))]
pub mod share {
    use super::*;

    pub fn draw(ui: &mut Ui, _app: &mut App, a: Rect) {
        header(ui, a, "Compartilhar tela", "Esta cópia do Prisma foi compilada sem o compartilhamento.");
    }

    pub fn shutdown() {}

    pub fn tick(_app: &mut App) {}
}
mod startup;
mod storage;
mod tools;
mod tweaks;
mod uninstall;
pub mod volume;
pub mod welcome;
mod widget;

use crate::app::App;
use crate::ui::gfx::*;
use crate::ui::Ui;

#[derive(Clone, Copy, PartialEq)]
pub enum Page {
    Monitor,
    Performance,
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
    Updates,
    Optimize,
    Energy,
    Fullscreen,
    Games,
    Volume,
    Hdr,
    Nvidia,
    Capture,
    Share,
    Shaders,
    Controllers,
    Shortcuts,
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

/// Velocímetro: arco e ponteiro.
const ICON_PERF: Icon = &[
    &[(4., 17.), (3.5, 13.), (5., 9.), (8., 6.), (12., 5.), (16., 6.), (19., 9.), (20.5, 13.), (20., 17.)],
    &[(12., 15.), (16., 9.)],
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

/// Tela com linhas de varredura.
const ICON_SHADER: Icon = &[
    &[(3., 5.), (21., 5.), (21., 17.), (3., 17.), (3., 5.)],
    &[(6., 8.5), (18., 8.5)],
    &[(6., 11.), (18., 11.)],
    &[(6., 13.5), (18., 13.5)],
    &[(9., 20.), (15., 20.)],
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
const ICON_UPD: Icon = &[&[(12., 4.), (12., 15.)], &[(7., 10.), (12., 15.), (17., 10.)], &[(4., 15.), (4., 20.), (20., 20.), (20., 15.)]];
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
/// Controle de jogo: o corpo e a cruz de direção.
const ICON_LIB: Icon = &[
    &[(7., 7.), (17., 7.), (21., 15.), (19., 18.), (15., 15.), (9., 15.), (5., 18.), (3., 15.), (7., 7.)],
    &[(8., 9.5), (8., 12.5)],
    &[(6.5, 11.), (9.5, 11.)],
    &[(15.5, 10.), (16.5, 10.)],
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

/// Nome da página na barra lateral; fora dela, vazio.
pub fn nav_label(p: Page) -> &'static str {
    NAV.iter().find(|n| n.0 == p).map_or("", |n| n.1)
}

/// Funções que não desligam: o início e Configurações, onde se religa o resto.
const ALWAYS: [Page; 2] = [Page::Monitor, Page::Adjust];

/// A função `p` está ligada (`Config::funcs_off`, pela posição em `ACTION_PAGES`). Fora da lista
/// (Guia, Reportar bug, Novidades) e `ALWAYS`: sempre.
pub fn is_on(cfg: &crate::config::Config, p: Page) -> bool {
    ALWAYS.contains(&p) || ACTION_PAGES.iter().position(|&a| a == p).is_none_or(|i| !cfg.funcs_off.contains(&(i as u8)))
}

pub fn set_on(cfg: &mut crate::config::Config, p: Page, on: bool) {
    let Some(i) = ACTION_PAGES.iter().position(|&a| a == p) else { return };
    cfg.funcs_off.retain(|&n| n != i as u8);
    if !on && !ALWAYS.contains(&p) {
        cfg.funcs_off.push(i as u8);
    }
}

/// Aparece na barra lateral e na lista de funções: o Driver NVIDIA só com placa NVIDIA.
fn exists(p: Page) -> bool {
    p != Page::Nvidia || nvidia::available()
}

/// Item da barra lateral à vista: ligado e existe neste PC.
pub fn shown(cfg: &crate::config::Config, p: Page) -> bool {
    is_on(cfg, p) && exists(p)
}

/// Ordem das ações "Página ..." (33 a 59): a da barra lateral antes dos grupos, com Cópias, Atalhos, Desempenho,
/// Shaders, Atualizações e Jogos no fim. Nunca muda, mesmo que a barra mude: os números já gravados dependem dela.
const ACTION_PAGES: [Page; 27] = [
    Page::Monitor,
    Page::Meter,
    Page::Storage,
    Page::Analyze,
    Page::Search,
    Page::Startup,
    Page::Services,
    Page::Uninstall,
    Page::Tweaks,
    Page::Tools,
    Page::Optimize,
    Page::Energy,
    Page::Fullscreen,
    Page::Volume,
    Page::Hdr,
    Page::Nvidia,
    Page::Capture,
    Page::Share,
    Page::Controllers,
    Page::Widget,
    Page::Adjust,
    Page::Copies,
    Page::Shortcuts,
    Page::Performance,
    Page::Shaders,
    Page::Updates,
    Page::Games,
];

/// Os itens da barra lateral na ordem das ações "Página ...".
fn action_nav() -> impl Iterator<Item = &'static (Page, &'static str, Icon, &'static str)> {
    ACTION_PAGES.iter().filter_map(|p| NAV.iter().find(|n| n.0 == *p))
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
        crate::sys::remap::MACRO_REC => ICON_REC,
        n if n >= crate::sys::remap::PAGES && n < crate::sys::radial::PROFILES => nav_page((n - crate::sys::remap::PAGES) as usize).map_or(ICON_PAD, |p| p.1),
        crate::sys::radial::PROGRAM => ICON_APP,
        crate::sys::radial::PROFILES => ICON_PAD,
        _ => ICON_PAD,
    }
}

/// Página seguinte (`d` = 1) ou anterior (-1) da barra lateral, dando a volta; de fora dela (Guia,
/// Reportar bug), a primeira ou a última.
pub fn nav_step(cfg: &crate::config::Config, page: Page, d: i8) -> Page {
    let nav: Vec<Page> = NAV.iter().map(|n| n.0).filter(|&p| shown(cfg, p)).collect();
    let i = nav.iter().position(|&p| p == page).map_or(if d > 0 { -1 } else { 0 }, |i| i as isize);
    nav[(i + d as isize).rem_euclid(nav.len() as isize) as usize]
}

/// Barra lateral, em grupos (`GROUPS`): página, nome, ícone e o que ela faz (na lista de funções de
/// Configurações e das boas-vindas). Mudar a ordem não mexe nas ações "Página ..." (`ACTION_PAGES`).
const NAV: &[(Page, &str, Icon, &str)] = &[
    (Page::Monitor, "Monitor", ICON_MON, "Programas abertos, uso do PC e fechar o que travou."),
    (Page::Performance, "Desempenho", ICON_PERF, "Gráficos de processador, placa de vídeo, memória, discos e rede."),
    (Page::Storage, "Discos e redes", ICON_DISK, "Tudo que está ligado no PC, em tempo real."),
    (Page::Analyze, "Espaço em disco", ICON_MAP, "O que ocupa espaço em cada disco, pasta por pasta."),
    (Page::Startup, "Inicialização", ICON_BOOT, "Programas que abrem com o Windows, para ligar ou desligar."),
    (Page::Services, "Serviços", ICON_SVC, "Serviços do Windows, com o que cada um faz."),
    (Page::Uninstall, "Desinstalar", ICON_DEL, "Remove programas e as sobras que eles deixam."),
    (Page::Search, "Pesquisar", ICON_FIND, "Acha arquivos e pastas pelo nome em todos os discos, enquanto você digita."),
    (Page::Tweaks, "Ajustes do Windows", ICON_TW, "Só o que tem efeito comprovado. Tudo pode ser desfeito."),
    (Page::Tools, "Ferramentas", ICON_TOOL, "O que o Windows esconde em linha de comando, com resultado em português."),
    (Page::Copies, "Cópias", ICON_COPY, "Cópias com pausa e conferência, e listas para rodar na hora marcada."),
    (Page::Updates, "Atualizações", ICON_UPD, "Avisa quando sai driver da NVIDIA ou atualização do Windows e diz se vale instalar."),
    (Page::Optimize, "Otimizar", ICON_OPT, "Fecha programas parados antes de jogar. Nada é fechado sem você confirmar."),
    (Page::Energy, "Energia", ICON_POW, "Troca o plano de energia sozinho entre repouso, carga alta e jogo."),
    (Page::Games, "Jogos", ICON_LIB, "Os ajustes de cada jogo num lugar só: medidor, HDR, som, controle, shader e driver."),
    (Page::Meter, "Medidor", ICON_METER, "Leitor de performance, limitador de FPS e HDR automático por jogo."),
    (Page::Fullscreen, "Fullscreenizer", ICON_WIN, "Deixa a janela em foco em tela cheia sem bordas. O mesmo atalho desfaz."),
    (Page::Hdr, "HDR", ICON_HDR, "O que a tela, o perfil e o Windows estão informando, e a calibração própria."),
    (Page::Nvidia, "Driver NVIDIA", ICON_GPU, "Ajustes do driver da NVIDIA para cada jogo."),
    (Page::Capture, "Captura", ICON_CAP, "Fotos, gravação da tela e replay dos últimos segundos. Tudo fica no seu PC."),
    (Page::Share, "Compartilhar tela", ICON_SHARE, "Acesso remoto e co-op pela internet, sem conta. Em teste."),
    (Page::Shaders, "Shaders", ICON_SHADER, "Filtros do RetroArch por cima de qualquer jogo: CRT, scanlines, nitidez."),
    (Page::Volume, "Som", ICON_VOL, "Volume, aparelhos e equalizador de cada saída e de cada programa."),
    (Page::Controllers, "Controles", ICON_PAD, "Botões, macros, menu radial e teclado na tela para controles."),
    (Page::Shortcuts, "Atalhos", ICON_KEYS, "Combinações de teclas que abrem programas de qualquer lugar do Windows."),
    (Page::Widget, "Widget", ICON_WID, "Uso, temperatura, plano de energia, HDR e VRR por cima de tudo."),
    (Page::Adjust, "Configurações", ICON_ADJ, ""),
];

/// Grupos da barra lateral: nome e quantos itens seguidos de `NAV` entram nele.
const GROUPS: [(&str, usize); 5] = [("Sistema", 7), ("Manutenção", 7), ("Jogos e tela", 8), ("Entrada e som", 3), ("Prisma", 2)];

/// Grupo do item `i` de `NAV`.
fn group_of(i: usize) -> usize {
    let mut end = 0;
    GROUPS
        .iter()
        .position(|g| {
            end += g.1;
            i < end
        })
        .unwrap_or(GROUPS.len() - 1)
}

/// Onde fica cada item `nav` (índices de `NAV`) no conteúdo da barra lateral e a altura dele todo:
/// cada grupo abre com `cap` (o nome dele, ou um traço na barra compacta) e cada item ocupa `step`.
fn side_rows(nav: &[usize], cap: f32, step: f32) -> (Vec<f32>, f32) {
    let (mut ys, mut y, mut group) = (Vec::new(), 0.0, usize::MAX);
    for &i in nav {
        if group_of(i) != group {
            group = group_of(i);
            y += cap;
        }
        ys.push(y);
        y += step;
    }
    (ys, y)
}

/// Ajuda fora da lista de funções: na barra de título, sempre à vista.
const HELP: &[(Page, &str, Icon)] = &[(Page::Guide, "Guia e tutorial", ICON_HELP), (Page::Report, "Reportar bug", ICON_BUG)];

const SIDE_W: f32 = 216.0;
/// Altura do nome de um grupo na barra lateral.
const GROUP_CAP: f32 = 18.0;
/// Altura de cada item da barra lateral.
const SIDE_STEP: f32 = 33.0;
/// Barra lateral só com ícones: largura, lado do botão e o espaço do traço entre os grupos.
const MINI_W: f32 = 68.0;
const MINI_BTN: f32 = 44.0;
const MINI_CAP: f32 = 15.0;
const KOFI_URL: &str = "https://ko-fi.com/maliquem";
const KOFI_ICON: &[u8] = include_bytes!("../../res/kofi.bgra");
/// Altura da barra de título do Prisma (no lugar da do Windows): a página começa abaixo dela.
pub const TITLE_H: f32 = 40.0;
/// Largura de cada botão da janela (minimizar, maximizar, fechar) na barra de título.
const CAP_W: f32 = 46.0;
const LOGO: Icon = &[&[(12., 3.), (3., 20.), (21., 20.), (12., 3.)], &[(12., 3.), (12., 20.)]];
const CLOSE_HOT: Color = rgb(0xc42b1c);

/// Botão da janela sob `x`, `y` (em coordenadas da página: a barra de título fica entre `-TITLE_H`
/// e 0) numa janela de largura `w`: 0 minimizar, 1 maximizar, 2 fechar. Quem clica neles é o
/// Windows, que assim mostra o Encaixar no de maximizar.
pub fn caption_button(w: f32, x: f32, y: f32) -> Option<usize> {
    let left = w - 3.0 * CAP_W;
    ((-TITLE_H..0.0).contains(&y) && x >= left && x < w).then(|| ((x - left) / CAP_W) as usize)
}

pub fn draw(mut ui: Ui, app: &mut App, full: Rect) {
    // Função desligada (por um aviso, pelo controle ou agora mesmo): fica em Configurações, onde se religa.
    if !shown(&app.cfg, app.page) && NAV.iter().any(|n| n.0 == app.page) {
        app.page = Page::Adjust;
    }
    if ui.s.top > 0.0 {
        title_bar(&mut ui, app, full.w);
    }
    // O guia abre por cima das boas-vindas, que continuam esperando a resposta.
    if app.welcome.is_some() && app.page != Page::Guide {
        if let Some(t) = &mut app.report.text {
            t.hide();
        }
        welcome::draw(&mut ui, app, full);
        ui.end();
        return;
    }
    let mini = app.cfg.side_compact;
    let side_w = if mini { MINI_W } else { SIDE_W };
    let (cap, step) = if mini { (MINI_CAP, MINI_BTN + 2.0) } else { (GROUP_CAP, SIDE_STEP) };
    let side = rect(0.0, 0.0, side_w, full.h);
    ui.c.fill(side, SIDE);

    let nav: Vec<_> = NAV.iter().enumerate().filter(|(_, n)| shown(&app.cfg, n.0)).collect();
    let (ys, content) = side_rows(&nav.iter().map(|(i, _)| *i).collect::<Vec<_>>(), cap, step);
    // Em janela baixa a lista rola, com a barra à direita (sem passar da barra lateral).
    let top = 6.0;
    let view = rect(0.0, top, side_w - crate::ui::BAR_GAP - crate::ui::BAR_W, (full.h - 8.0 - top).max(64.0));
    // Página nova (pelo controle, um atalho ou ao abrir), janela de outra altura ou o outro jeito da
    // barra: a lista rola até o item da página aberta.
    if app.side_seen != Some((app.page, view.h + side_w)) {
        app.side_seen = Some((app.page, view.h + side_w));
        if let Some(k) = nav.iter().position(|(_, n)| n.0 == app.page) {
            let v = ui.scroll_y_of("side");
            // O nome do grupo acima do item entra junto.
            let to = v.clamp(ys[k] + step - view.h, (ys[k] - cap).max(0.0));
            if to != v {
                ui.set_scroll("side", to);
            }
        }
    }
    ui.begin_scroll("side", view, content);
    let mut group = usize::MAX;
    for ((i, (page, label, icon, _)), dy) in nav.into_iter().zip(ys) {
        let y = top + dy;
        if group_of(i) != group {
            // O primeiro grupo não leva traço em cima.
            if mini && group != usize::MAX {
                ui.c.fill(rect(16.0, y - (MINI_CAP + 1.0) / 2.0, side_w - 32.0, 1.0), BORDER);
            }
            group = group_of(i);
            if !mini {
                ui.c.text(rect(24.0, y - GROUP_CAP, SIDE_W - 48.0, GROUP_CAP), GROUPS[group].0, 12.0, MUTED, Align::Left, Weight::Semibold);
            }
        }
        let r = if mini { rect((side_w - MINI_BTN) / 2.0, y, MINI_BTN, MINI_BTN) } else { rect(12.0, y, SIDE_W - 28.0, SIDE_STEP - 1.0) };
        let current = app.page == *page;
        let hover = ui.is_hover(r);
        if current || hover {
            ui.c.round(r, 8.0, if current { CARD2 } else { CARD });
        }
        // A página aberta: traço azul à esquerda e ícone azul, além do fundo.
        if current {
            ui.c.round(rect(r.x, r.y + 8.0, 3.0, r.h - 16.0), 1.5, ACCENT);
        }
        let color = if current { WHITE } else { TEXT2 };
        if mini {
            ui.c.icon(r.x + 11.0, r.y + 11.0, 22.0, icon, if current { ACCENT } else { color });
        } else {
            ui.c.icon(r.x + 12.0, r.y + (r.h - 18.0) / 2.0, 18.0, icon, if current { ACCENT } else { color });
            ui.c.text(rect(r.x + 42.0, r.y, r.w - 48.0, r.h), label, 14.0, color, Align::Left, if current { Weight::Semibold } else { Weight::Regular });
        }
        if ui.hit_as(&format!("nav-{label}"), r, label, crate::ui::a11y::Role::Tab, Some(current)) && !current {
            crate::report::note(&format!("page: {label}"));
            app.page = *page;
            app.refresh_page();
            ui.s.needs_redraw = true;
        }
        if mini {
            ui.tip_right(r, label);
        }
    }
    ui.end_scroll();

    let area = rect(side_w, 0.0, full.w - side_w, full.h);
    if app.page != Page::Report {
        if let Some(t) = &mut app.report.text {
            t.hide();
        }
    }
    match app.page {
        Page::Monitor => monitor::draw(&mut ui, app, area),
        Page::Performance => perf::draw(&mut ui, app, area),
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
        Page::Updates => updates::draw(&mut ui, app, area),
        Page::Optimize => optimize::draw(&mut ui, app, area),
        Page::Energy => energy::draw(&mut ui, app, area),
        Page::Fullscreen => fullscreen::draw(&mut ui, app, area),
        Page::Games => games::draw(&mut ui, app, area),
        Page::Volume => volume::draw(&mut ui, app, area),
        Page::Hdr => hdr::draw(&mut ui, app, area),
        Page::Nvidia => nvidia::draw(&mut ui, app, area),
        Page::Capture => capture::draw(&mut ui, app, area),
        Page::Share => share::draw(&mut ui, app, area),
        Page::Shaders => shaders::draw(&mut ui, app, area),
        Page::Controllers => controllers::draw(&mut ui, app, area),
        Page::Shortcuts => shortcuts::draw(&mut ui, app, area),
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

/// Barra de título do Prisma, no lugar da do Windows (entre `-TITLE_H` e 0): nome, Ko-fi, Guia,
/// Reportar bug, o botão da energia automática, o do jeito da barra lateral e os botões da janela.
/// Guarda em `app.title_hot` o que é clicável; o resto arrasta a janela (`WM_NCHITTEST`, app.rs).
fn title_bar(ui: &mut Ui, app: &mut App, w: f32) {
    let y = -TITLE_H;
    ui.c.fill(rect(0.0, y, w, TITLE_H), SIDE);
    ui.c.icon(14.0, y + 10.0, 20.0, LOGO, ACCENT);
    ui.c.text(rect(42.0, y, 80.0, TITLE_H), "Prisma", 14.0, TEXT, Align::Left, Weight::Semibold);
    app.title_hot.clear();
    let by = y + 6.0;
    let bg = |ui: &Ui, r: Rect, on: bool| {
        if on || ui.is_hover(r) {
            ui.c.round(r, 6.0, if on { ACCENT } else { CARD2 });
        }
    };
    let mut x = 42.0 + crate::ui::gfx::measure("Prisma", 14.0, Weight::Semibold, None).0 + 12.0;
    let coffee = rect(x, by, 32.0, 28.0);
    bg(ui, coffee, false);
    ui.image("kofi", 64, KOFI_ICON, rect(coffee.x + 7.0, coffee.y + 5.0, 18.0, 18.0));
    ui.tip(coffee, "Apoiar o Prisma no Ko-fi");
    if ui.hit("kofi", coffee) {
        crate::report::open_url(KOFI_URL);
    }
    app.title_hot.push(coffee);
    for (page, tip, icon) in HELP {
        x += 36.0;
        let r = rect(x, by, 32.0, 28.0);
        let current = app.page == *page;
        bg(ui, r, current);
        ui.c.icon(r.x + 7.0, r.y + 5.0, 18.0, icon, if current { ON_ACCENT } else { ACCENT });
        ui.tip(r, tip);
        if ui.hit(&format!("nav-help-{tip}"), r) && !current {
            if *page == Page::Report {
                app.report.area = report::area_of(app.page);
            }
            app.page = *page;
            app.refresh_page();
            ui.s.needs_redraw = true;
        }
        app.title_hot.push(r);
    }
    x += 36.0 + 14.0;

    // Energia desligada: sem o botão dela. O clique liga e desliga a troca automática.
    let modes_x = w - 4.0 * CAP_W;
    if is_on(&app.cfg, Page::Energy) {
        let auto = app.cfg.power_auto;
        let plan = app.plans.iter().find(|p| p.id == app.active_plan).map(|p| p.name.as_str()).unwrap_or("Plano desconhecido");
        let state = if !auto && !app.cfg.plan_fixed.is_empty() {
            "Plano fixo"
        } else if !auto {
            "Troca desligada"
        } else {
            match app.detector.mode {
                crate::sys::power::Mode::Rest => "Repouso",
                crate::sys::power::Mode::Load => "Carga alta",
                crate::sys::power::Mode::Game => "Modo jogo ativo",
            }
        };
        // O nome do plano vem do Windows (e pode ser traduzido): fica com no máximo 200, e diminui até caber.
        let pw = crate::ui::gfx::measure(plan, 14.0, Weight::Semibold, None).0.min(200.0);
        let sw = crate::ui::gfx::measure(state, 12.0, Weight::Regular, None).0;
        let r = rect(x, by, (10.0 + 18.0 + 8.0 + pw + 10.0 + sw + 10.0).min(modes_x - 8.0 - x), 28.0);
        bg(ui, r, false);
        ui.c.icon(r.x + 10.0, r.y + 5.0, 18.0, ICON_POW, if auto { ACCENT } else { TEXT2 });
        ui.c.text_fit(rect(r.x + 36.0, r.y, pw, r.h), plan, 14.0, TEXT, Align::Left, Weight::Semibold);
        ui.c.text(rect(r.x + 46.0 + pw, r.y, sw + 4.0, r.h), state, 12.0, if auto { ACCENT } else { MUTED }, Align::Left, Weight::Regular);
        let tip = if auto { "Troca automática ligada. Clique para deixar o plano fixo." } else { "Troca automática desligada. Clique para ligar." };
        ui.tip(r, tip);
        if ui.hit_as("energy-auto", r, "Troca automática", crate::ui::a11y::Role::Check, Some(auto)) {
            app.cfg.power_auto = !auto;
            app.cfg.save();
            app.apply_power_choice();
            ui.s.needs_redraw = true;
        }
        app.title_hot.push(r);
    }

    // O jeito da barra lateral, ao lado do minimizar: com nomes ou só com ícones.
    let mini = app.cfg.side_compact;
    let r = rect(modes_x, y, CAP_W, TITLE_H);
    bg(ui, r, false);
    let rail: f32 = if mini { 7.0 } else { 4.0 };
    ui.c.round_line(rect(r.x + 15.0, r.y + 13.0, 16.0, 14.0), 2.0, TEXT2, 1.2);
    ui.c.fill(rect(r.x + 15.0 + rail, r.y + 13.0, 1.2, 14.0), TEXT2);
    let tip = if mini { "Mostrar os nomes no menu" } else { "Menu só com ícones" };
    ui.tip(r, tip);
    if ui.hit_as("side-mode", r, tip, crate::ui::a11y::Role::Button, None) {
        app.cfg.side_compact = !mini;
        app.cfg.save();
        ui.s.needs_redraw = true;
    }
    app.title_hot.push(r);

    // Minimizar, maximizar e fechar: o clique vem pelo Windows (`caption_button`); aqui só o desenho.
    let zoomed = unsafe { windows::Win32::UI::WindowsAndMessaging::IsZoomed(app.paint_window.0).as_bool() };
    for i in 0..3 {
        let r = rect(w - (3 - i) as f32 * CAP_W, y, CAP_W, TITLE_H);
        let hot = ui.is_hover(r);
        if hot {
            ui.c.fill(r, if i == 2 { CLOSE_HOT } else { CARD2 });
        }
        let (cx, cy) = (r.x + CAP_W / 2.0, y + TITLE_H / 2.0);
        let c = if hot && i == 2 { WHITE } else { TEXT2 };
        match i {
            0 => ui.c.line(cx - 5.0, cy, cx + 5.0, cy, c, 1.0),
            1 if zoomed => {
                ui.c.round_line(rect(cx - 5.0, cy - 3.0, 8.0, 8.0), 1.0, c, 1.0);
                ui.c.line(cx - 3.0, cy - 5.0, cx + 5.0, cy - 5.0, c, 1.0);
                ui.c.line(cx + 5.0, cy - 5.0, cx + 5.0, cy + 3.0, c, 1.0);
            }
            1 => ui.c.round_line(rect(cx - 5.0, cy - 5.0, 10.0, 10.0), 1.0, c, 1.0),
            _ => {
                ui.c.line(cx - 5.0, cy - 5.0, cx + 5.0, cy + 5.0, c, 1.0);
                ui.c.line(cx + 5.0, cy - 5.0, cx - 5.0, cy + 5.0, c, 1.0);
            }
        }
    }
}

/// Liga e desliga as funções, em Configurações e nas boas-vindas: nome, o que faz e a caixa, em
/// duas colunas por grupo. Sai de `NAV`: página nova entra sozinha.
pub fn funcs_list(ui: &mut Ui, app: &mut App, r: Rect) {
    const ROW: f32 = 84.0;
    const CAP: f32 = 30.0;
    let col = ((r.w - 28.0) / 2.0).floor();
    // Primeiro onde fica cada coisa (a rolagem pede a altura toda), depois o desenho.
    let mut cells: Vec<(Option<usize>, usize, Rect)> = Vec::new();
    let (mut y, mut group, mut k) = (0.0, usize::MAX, 0);
    for (i, _) in NAV.iter().enumerate().filter(|(_, n)| exists(n.0) && !ALWAYS.contains(&n.0)) {
        if group_of(i) != group {
            group = group_of(i);
            if k % 2 == 1 {
                y += ROW;
            }
            k = 0;
            cells.push((Some(group), i, rect(r.x, r.y + y, r.w, CAP)));
            y += CAP;
        }
        let x = if k % 2 == 0 { r.x } else { r.x + col + 12.0 };
        cells.push((None, i, rect(x, r.y + y, col, ROW - 8.0)));
        k += 1;
        if k % 2 == 0 {
            y += ROW;
        }
    }
    if k % 2 == 1 {
        y += ROW;
    }
    ui.begin_scroll("funcs", r, y);
    for (cap, i, c) in cells {
        if let Some(g) = cap {
            ui.c.text(rect(c.x, c.y + 6.0, c.w, c.h - 6.0), GROUPS[g].0, 13.0, MUTED, Align::Left, Weight::Semibold);
            continue;
        }
        let (page, label, _, what) = NAV[i];
        ui.card(c);
        let mut on = is_on(&app.cfg, page);
        if ui.check(&format!("func-{label}"), rect(c.x + 14.0, c.y + 6.0, c.w - 28.0, 28.0), label, &mut on) {
            app.set_func(page, on);
        }
        ui.c.text_wrap(rect(c.x + 42.0, c.y + 34.0, c.w - 56.0, c.h - 38.0), what, 12.0, MUTED, Weight::Regular);
    }
    ui.end_scroll();
}

/// Atalho recém-gravado que já tinha dono: outra função do Prisma ou outro programa.
fn hotkey_prompt(ui: &mut Ui, app: &mut App) {
    use crate::app::HkAsk;
    if let Some(a) = app.hk_ask.take() {
        match a {
            HkAsk::Swap { hk, other, .. } => {
                let (item, page) = app.hk_place(other);
                ui.ask("hk-swap", "Trocar o atalho?", &format!("{} já é o atalho de {item}, na página {page}. Sim tira de lá e coloca aqui.", hk.label()));
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
    /// A altura da barra lateral cobre cada item e o nome de cada grupo que aparece, sem sobrar.
    #[test]
    fn side_rows_fit_items_and_captions() {
        use super::*;
        let all: Vec<usize> = (0..NAV.len()).collect();
        let (ys, h) = side_rows(&all, GROUP_CAP, SIDE_STEP);
        assert_eq!(h, NAV.len() as f32 * SIDE_STEP + GROUPS.len() as f32 * GROUP_CAP);
        assert_eq!(ys[0], GROUP_CAP);
        assert_eq!(ys[ys.len() - 1] + SIDE_STEP, h);
        // Um grupo inteiro desligado não deixa o nome dele.
        let some: Vec<usize> = all.into_iter().filter(|&i| group_of(i) != 3).collect();
        assert_eq!(side_rows(&some, GROUP_CAP, SIDE_STEP).1, some.len() as f32 * SIDE_STEP + (GROUPS.len() - 1) as f32 * GROUP_CAP);
    }

    #[test]
    fn caption_buttons_sit_at_the_right_of_the_title_bar() {
        use super::*;
        let w = 1200.0;
        assert_eq!(caption_button(w, w - 1.0, -20.0), Some(2));
        assert_eq!(caption_button(w, w - CAP_W - 1.0, -20.0), Some(1));
        assert_eq!(caption_button(w, w - 3.0 * CAP_W, -1.0), Some(0));
        // Fora da barra (a página) ou à esquerda dos três: não é botão da janela.
        assert_eq!(caption_button(w, w - 1.0, 0.0), None);
        assert_eq!(caption_button(w, w - 3.0 * CAP_W - 1.0, -20.0), None);
        assert_eq!(caption_button(w, w, -20.0), None);
    }

    #[test]
    fn page_actions_follow_the_sidebar() {
        use crate::sys::remap::{ACTIONS, PAGES};
        let pages = &ACTIONS[PAGES as usize..];
        assert_eq!(pages.len(), super::NAV.len());
        for (a, n) in pages.iter().zip(super::action_nav()) {
            assert_eq!(*a, format!("Página {}", n.1));
        }
    }

    /// Cada item da barra lateral tem uma ação "Página ..." só, os números das primeiras e das últimas
    /// continuam os gravados, e os grupos cobrem a barra inteira.
    #[test]
    fn every_page_has_one_action_and_one_group() {
        use super::*;
        assert_eq!(ACTION_PAGES.len(), NAV.len());
        for n in NAV {
            assert_eq!(ACTION_PAGES.iter().filter(|&&p| p == n.0).count(), 1, "{}", n.1);
        }
        assert_eq!(GROUPS.iter().map(|g| g.1).sum::<usize>(), NAV.len());
        assert!(nav_page(0).is_some_and(|n| n.0 == Page::Monitor));
        assert!(nav_page(1).is_some_and(|n| n.0 == Page::Meter));
        assert!(nav_page(21).is_some_and(|n| n.0 == Page::Copies));
        assert!(nav_page(22).is_some_and(|n| n.0 == Page::Shortcuts));
        assert!(nav_page(23).is_some_and(|n| n.0 == Page::Performance));
        assert!(nav_page(24).is_some_and(|n| n.0 == Page::Shaders));
        assert!(nav_page(25).is_some_and(|n| n.0 == Page::Updates));
        assert!(nav_page(26).is_some_and(|n| n.0 == Page::Games));
        assert!(nav_page(27).is_none());
    }

    /// Tudo começa ligado; desligar e religar volta ao começo; o início e Configurações não desligam,
    /// e a escolha gravada no config volta igual.
    #[test]
    fn functions_turn_off_and_back_on() {
        use super::*;
        let mut cfg = crate::config::Config::default();
        assert!(NAV.iter().all(|n| is_on(&cfg, n.0)));
        set_on(&mut cfg, Page::Meter, false);
        set_on(&mut cfg, Page::Meter, false);
        set_on(&mut cfg, Page::Adjust, false);
        set_on(&mut cfg, Page::Monitor, false);
        assert_eq!(cfg.funcs_off, vec![1]);
        assert!(!is_on(&cfg, Page::Meter) && is_on(&cfg, Page::Adjust) && is_on(&cfg, Page::Monitor));
        assert!(is_on(&cfg, Page::Guide));
        // Config editado à mão com Configurações desligada: continua ligada.
        cfg.funcs_off.push(ACTION_PAGES.iter().position(|&p| p == Page::Adjust).unwrap() as u8);
        assert!(is_on(&cfg, Page::Adjust));
        set_on(&mut cfg, Page::Meter, true);
        assert!(is_on(&cfg, Page::Meter));
        // Toda função que desliga tem o que ela faz, para a lista.
        assert!(NAV.iter().filter(|n| !ALWAYS.contains(&n.0)).all(|n| !n.3.is_empty()), "função sem descrição");
    }
}
