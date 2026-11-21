//! Jogos: a lista dos jogos e, ao abrir um, os ajustes dele de cada aba num lugar só. Cada seção lê e
//! grava o mesmo dado que a aba dela usa (`games` junta tudo pelo nome do exe).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use super::*;
use crate::config::{FsApp, HdrGame, MeterGame};
use crate::games::{self as gm, Found, Row};
use crate::sys::autohdr;

/// `WM_PICK_GAMES`: adicionar um .exe ou .dll, escanear uma pasta, ou só redesenhar (medida pronta).
pub const ADD: usize = 0;
pub const SCAN: usize = 1;
pub const REDRAW: usize = 2;

/// Cada seção: o bit em `games`, a aba dona do dado, o título e o nome da aba no botão.
const SECTIONS: [(u16, Page, &str, &str); 8] = [
    (gm::METER, Page::Meter, "Medidor e limitador de FPS", "Medidor"),
    (gm::HDR, Page::Hdr, "HDR", "HDR"),
    (gm::SOUND, Page::Volume, "Som", "Som"),
    (gm::ENERGY, Page::Energy, "Energia", "Energia"),
    (gm::PAD, Page::Controllers, "Controle", "Controles"),
    (gm::SHADER, Page::Shaders, "Shader", "Shaders"),
    (gm::FULL, Page::Fullscreen, "Fullscreenizer", "Fullscreenizer"),
    (gm::NVIDIA, Page::Nvidia, "Driver NVIDIA", "Driver NVIDIA"),
];

#[derive(Default)]
struct St {
    query: String,
    sort: usize,
    tab: usize,
    /// O jogo aberto (exe, caminho, nome); None é a lista.
    open: Option<(String, String, String)>,
    scan: Option<Vec<Found>>,
    /// Aparelhos de som (saídas), lidos ao abrir o jogo.
    outputs: Vec<crate::sys::audio::Device>,
    /// Perfil do driver NVIDIA de cada exe: o nome, vazio sem perfil.
    nv: HashMap<String, String>,
    /// Perfis do equalizador da saída do jogo aberto.
    eq: Vec<String>,
}

thread_local! {
    static ST: RefCell<St> = RefCell::new(St::default());
}

/// As seções das funções ligadas, em bits de `games`.
fn mask(app: &App) -> u16 {
    SECTIONS.iter().filter(|s| shown(&app.cfg, s.1)).fold(0, |m, s| m | s.0)
}

/// A aba abriu: mede os exes de novo (podem ter sido instalados ou apagados).
pub fn refresh() {
    gm::forget_info();
    gm::forget_extra();
}

/// Foto de teste (`--test-page=games-open`): abre o primeiro jogo com ajuste.
#[cfg(debug_assertions)]
pub fn open_first(app: &mut App) {
    let mut rows = gm::rows(&app.cfg, &gm::extra(), mask(app));
    gm::sort(&mut rows, 0);
    if let Some(r) = rows.iter().find(|r| r.funcs != 0).or(rows.first()) {
        ST.with(|s| open_game(app, &mut s.borrow_mut(), r));
    }
}

/// O .exe ou .dll escolhido em Adicionar.
pub fn added(app: &mut App, path: &str) {
    if gm::add(&mut app.cfg.games, path, "") {
        app.cfg.save();
    } else {
        app.toast.info("Esse jogo já está na lista.");
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    ST.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(f) = gm::take_scan() {
            if f.is_empty() {
                app.toast.info("Nenhum executável nessa pasta.");
            } else {
                s.scan = Some(f);
            }
        }
        if s.scan.is_some() {
            ui.block();
        }
        match s.open.clone() {
            Some(g) => game(ui, app, a, &mut s, &g),
            None => list(ui, app, a, &mut s),
        }
        if s.scan.is_some() {
            scan_sheet(ui, app, &mut s);
        }
    });
}

fn hub(app: &App) -> isize {
    app.hub.0 as isize
}

fn list(ui: &mut Ui, app: &mut App, a: Rect, s: &mut St) {
    header_room(ui, a, "Jogos", "Os ajustes de cada jogo num lugar só.", 420.0);
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let post = |kind: usize| unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(app.hub), crate::app::WM_PICK_GAMES, windows::Win32::Foundation::WPARAM(kind), windows::Win32::Foundation::LPARAM(0));
    };
    let add_w = 210.0;
    if ui.button("gm-add", rect(a.right() - 28.0 - add_w, 30.0, add_w, 36.0), "Adicionar .exe ou .dll…", true) {
        post(ADD);
    }
    if ui.button_ex("gm-scan", rect(a.right() - 40.0 - add_w - 160.0, 30.0, 160.0, 36.0), "Escanear pasta…", false, !gm::scanning()) {
        post(SCAN);
    }
    let on = mask(app);
    let extra = gm::extra();
    let mut rows = gm::rows(&app.cfg, &extra, on);
    let (with, without) = rows.iter().fold((0, 0), |(a, b), r| if r.funcs != 0 { (a + 1, b) } else { (a, b + 1) });
    let names = [format!("{} ({with})", crate::i18n::tr("Com perfil")), format!("{} ({without})", crate::i18n::tr("Sem perfil"))];
    let mut y = 88.0;
    ui.tabs("gm-tab", x, y, &names, &mut s.tab, true, None);
    let sorts: Vec<String> = ["Nome", "Loja", "Tamanho"].map(String::from).to_vec();
    ui.select("gm-sort", rect(a.right() - 28.0 - 140.0, y, 140.0, 36.0), &sorts, &mut s.sort);
    let sw = (w - 140.0 - 12.0 - 320.0).clamp(160.0, 280.0);
    ui.search("gm-q", rect(a.right() - 28.0 - 140.0 - 12.0 - sw, y, sw, 36.0), &mut s.query, "Buscar jogo");
    y += 48.0;
    // Sem o Prisma Launcher, as lojas não chegam: dizer de onde elas vêm.
    if !app.cfg.games.iter().any(|g| !g.store.is_empty()) {
        let r = rect(x, y, w, 48.0);
        ui.c.round(r, 10.0, CARD);
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 6.0, r.w - 32.0, 36.0), "Os jogos da Steam, Epic, GOG e Xbox aparecem aqui sozinhos com o Prisma Launcher aberto. Sem ele, ficam os postos à mão e os que já têm ajuste.", 12.0, TEXT2, Weight::Regular);
        y += 58.0;
    }
    let fresh = gm::FRESH.load(std::sync::atomic::Ordering::Relaxed);
    if fresh > 0 {
        let r = rect(x, y, w, 44.0);
        ui.c.round(r, 10.0, CARD2);
        let t = crate::i18n::tr(if fresh == 1 { "1 jogo novo chegou das lojas." } else { "{} jogos novos chegaram das lojas." }).replace("{}", &fresh.to_string());
        ui.c.text(rect(r.x + 16.0, r.y, r.w - 140.0, r.h), &t, 13.0, TEXT, Align::Left, Weight::Regular);
        if ui.button("gm-fresh", rect(r.right() - 96.0, r.y + 6.0, 84.0, 32.0), "OK", false) {
            gm::FRESH.store(0, std::sync::atomic::Ordering::Relaxed);
        }
        y += 54.0;
    }
    if gm::scanning() {
        ui.c.text(rect(x, y, w, 22.0), "Escaneando a pasta…", 13.0, MUTED, Align::Left, Weight::Regular);
        y += 30.0;
    }
    let mut q = Vec::new();
    crate::sys::index::fold(&s.query, &mut q);
    rows.retain(|r| (r.funcs != 0) == (s.tab == 0) && gm::matches(r, &q));
    gm::sort(&mut rows, s.sort);
    let view = rect(x, y, w, a.h - y - 20.0);
    if rows.is_empty() {
        let t = match (s.query.is_empty(), s.tab) {
            (false, _) => "Nenhum jogo com esse nome.",
            (true, 0) => "Nenhum jogo com ajuste ainda. Abra um da aba Sem perfil para ajustar.",
            _ => "Nenhum jogo. Use Adicionar ou Escanear pasta.",
        };
        ui.c.text_wrap(rect(x, y + 4.0, w, 40.0), t, 13.0, MUTED, Weight::Regular);
        return;
    }
    const ROW: f32 = 56.0;
    ui.begin_scroll("gm-list", view, rows.len() as f32 * ROW);
    let mut open = None;
    for (i, r) in rows.iter().enumerate() {
        let top = view.y + i as f32 * ROW;
        // Fora da tela, nem mede nem desenha.
        if top + ROW < view.y + ui.scroll_y() || top > view.bottom() + ui.scroll_y() {
            continue;
        }
        let rr = rect(view.x, top, view.w, ROW - 4.0);
        if ui.is_hover(rr) {
            ui.c.round(rr, 8.0, CARD2);
        }
        if ui.hit_as(&format!("gm-r{}", r.exe), rr, &r.name, crate::ui::a11y::Role::Item, None) {
            open = Some(r.clone());
        }
        ui.file_icon(&r.path, &r.exe, rect(rr.x + 10.0, top + 13.0, 26.0, 26.0));
        let funcs = func_names(r.funcs);
        let fw = if funcs.is_empty() { 0.0 } else { crate::ui::gfx::measure(&funcs, 12.0, Weight::Regular, None).0.min(rr.w * 0.4) + 16.0 };
        ui.c.text_fit(rect(rr.x + 48.0, top + 6.0, rr.w - 60.0 - fw, 22.0), &r.name, 14.0, TEXT, Align::Left, Weight::Regular);
        let missing = gm::info(&r.path, hub(app)).is_some_and(|i| !i.exists) && !r.path.is_empty();
        let sub = if missing { format!("{} · {}", r.exe, crate::i18n::tr("exe não encontrado")) } else { r.exe.clone() };
        ui.c.text_fit(rect(rr.x + 48.0, top + 28.0, rr.w - 60.0 - fw, 18.0), &sub, 12.0, if missing { WARN } else { MUTED }, Align::Left, Weight::Regular);
        if !funcs.is_empty() {
            ui.c.text_fit(rect(rr.right() - fw, top, fw - 12.0, ROW - 4.0), &funcs, 12.0, TEXT2, Align::Right, Weight::Regular);
        }
    }
    ui.end_scroll();
    if let Some(r) = open {
        open_game(app, s, &r);
    }
}

/// "Medidor · HDR · Som": o que o jogo tem de ajuste.
fn func_names(funcs: u16) -> String {
    SECTIONS.iter().filter(|s| funcs & s.0 != 0).map(|s| crate::i18n::tr(s.3).into_owned()).collect::<Vec<_>>().join(" · ")
}

fn open_game(app: &mut App, s: &mut St, r: &Row) {
    s.open = Some((r.exe.clone(), r.path.clone(), r.name.clone()));
    s.outputs = crate::sys::audio::devices(false);
    s.eq = crate::sys::eq::profiles(&crate::sys::eq::guid_of(&app.eq_device(&r.exe)));
    s.nv.remove(&r.exe.to_lowercase());
}

/// Linha de um cartão: o nome à esquerda e a caixa do controle à direita, que volta.
fn row(ui: &Ui, r: Rect, k: usize, label: &str) -> Rect {
    let y = r.y + 48.0 + k as f32 * 44.0;
    let cw = (r.w * 0.55).min(240.0);
    ui.c.text_fit(rect(r.x + 16.0, y, r.w - cw - 40.0, 36.0), label, 13.0, TEXT2, Align::Left, Weight::Regular);
    rect(r.right() - 16.0 - cw, y, cw, 36.0)
}

/// Texto que ocupa a linha inteira do cartão.
fn note(ui: &Ui, r: Rect, k: usize, s: &str, c: Color) {
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 48.0 + k as f32 * 44.0, r.w - 32.0, 40.0), s, 12.0, c, Weight::Regular);
}

/// Linhas de cada seção (sem o título e o rodapé).
fn rows_of(bit: u16, app: &App, exe: &str) -> usize {
    match bit {
        gm::METER => 7,
        gm::HDR => 2,
        gm::SOUND => 3 + app.cfg.snd_apps.iter().any(|a| a.exe.eq_ignore_ascii_case(exe) && a.pinned) as usize,
        gm::SHADER | gm::FULL => 2,
        gm::NVIDIA => 2,
        _ => 1,
    }
}

fn card_h(rows: usize) -> f32 {
    48.0 + rows as f32 * 44.0 + 52.0
}

fn game(ui: &mut Ui, app: &mut App, a: Rect, s: &mut St, g: &(String, String, String)) {
    let (exe, path, name) = (g.0.as_str(), g.1.as_str(), g.2.as_str());
    header_room(ui, a, name, exe, 260.0);
    if ui.button("gm-back", rect(a.right() - 28.0 - 110.0, 30.0, 110.0, 36.0), "Voltar", false) {
        s.open = None;
        return;
    }
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let view = rect(x, top, w, a.h - top - 20.0);
    let on = mask(app);
    let extra = gm::extra();
    let row_now = gm::rows(&app.cfg, &extra, on).into_iter().find(|r| r.exe.eq_ignore_ascii_case(exe));
    let funcs = row_now.as_ref().map_or(0, |r| r.funcs);
    let manual = app.cfg.games.iter().any(|g| g.store.is_empty() && g.exe().eq_ignore_ascii_case(exe));
    // Duas colunas, cada cartão na mais curta, na ordem de `SECTIONS`.
    let two = w >= 760.0;
    let cw = if two { (w - 16.0) / 2.0 } else { w };
    let info_h = 104.0;
    let mut col = [top + info_h + 16.0; 2];
    let mut cards = Vec::new();
    for sec in SECTIONS.iter().filter(|sec| on & sec.0 != 0) {
        let h = card_h(rows_of(sec.0, app, exe));
        let c = if two && col[1] < col[0] { 1 } else { 0 };
        cards.push((sec, rect(x + c as f32 * (cw + 16.0), col[c], cw, h)));
        col[c] += h + 16.0;
    }
    let content = col[0].max(col[1]) - top;
    ui.begin_scroll("gm-game", view, content);
    // O que se sabe do exe.
    let ir = rect(x, top, w, info_h);
    ui.card(ir);
    let folder = Path::new(path).parent().map(|p| p.display().to_string()).unwrap_or_default();
    let info = gm::info(path, hub(app));
    let api = match info {
        None if !path.is_empty() => crate::i18n::tr("API: medindo…").into_owned(),
        Some(i) if i.api != 0 => {
            let names = gm::api_names(i.api);
            if i.guessed { crate::i18n::tr("API: {} (provável, pelos arquivos)").replace("{}", &names) } else { crate::i18n::tr("API: {}").replace("{}", &names) }
        }
        _ => crate::i18n::tr("API: não encontrada").into_owned(),
    };
    let ac = match info {
        Some(i) if i.anti_cheat => "Anti-cheat: detectado",
        Some(_) => "Anti-cheat: não encontrado",
        None => "Anti-cheat: …",
    };
    let missing = info.is_some_and(|i| !i.exists) && !path.is_empty();
    let lw = w - 32.0 - if manual { 170.0 } else { 0.0 };
    let place = if path.is_empty() { crate::i18n::tr("Pasta: desconhecida (só o perfil conhece o jogo)").into_owned() } else { crate::i18n::tr("Pasta: {}").replace("{}", &folder) };
    ui.c.text_fit(rect(ir.x + 16.0, ir.y + 12.0, lw, 22.0), &place, 13.0, TEXT2, Align::Left, Weight::Regular);
    ui.c.text_fit(rect(ir.x + 16.0, ir.y + 40.0, lw, 22.0), &api, 13.0, TEXT2, Align::Left, Weight::Regular);
    ui.c.text_fit(rect(ir.x + 16.0, ir.y + 68.0, lw, 22.0), if missing { "O exe não foi encontrado nesse caminho." } else { ac }, 13.0, if missing { WARN } else { TEXT2 }, Align::Left, Weight::Regular);
    if manual && ui.button("gm-drop", rect(ir.right() - 16.0 - 154.0, ir.y + 12.0, 154.0, 36.0), "Tirar da lista", false) {
        ui.ask("gm-drop", "Tirar da lista?", "Os ajustes do jogo nas outras abas continuam salvos.");
    }
    if ui.answered("gm-drop") {
        app.cfg.games.retain(|g| !(g.store.is_empty() && g.exe().eq_ignore_ascii_case(exe)));
        app.cfg.save();
    }
    let protected = info.is_some_and(|i| i.anti_cheat);
    for (sec, r) in cards {
        let adjusted = funcs & sec.0 != 0;
        ui.card(r);
        ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 140.0, 24.0), sec.2);
        let pill = if adjusted { "ajustado" } else { "padrão" };
        ui.c.text(rect(r.right() - 120.0, r.y + 12.0, 104.0, 24.0), pill, 12.0, if adjusted { ACCENT } else { MUTED }, Align::Right, Weight::Regular);
        match sec.0 {
            gm::METER => meter(ui, app, r, exe, path, protected),
            gm::HDR => hdr(ui, app, r, exe, path),
            gm::SOUND => sound(ui, app, r, exe, path, &s.outputs, &s.eq),
            gm::ENERGY => energy(ui, app, r, exe, path),
            gm::PAD => pad(ui, app, r, exe, &extra),
            gm::SHADER => shader(ui, app, r, path, exe),
            gm::FULL => full(ui, app, r, exe, path),
            _ => nvidia(ui, app, r, exe, s),
        }
        let fy = r.bottom() - 48.0;
        let label = crate::i18n::tr("Abrir na aba {}").replace("{}", &crate::i18n::tr(sec.3));
        let bw = (r.w - 48.0) / 2.0;
        if ui.button(&format!("gm-go{}", sec.0), rect(r.x + 16.0, fy, bw, 36.0), &label, false) {
            go_to(app, sec.1, exe, path);
        }
        if sec.0 != gm::NVIDIA && sec.0 != gm::SHADER && ui.button_ex(&format!("gm-reset{}", sec.0), rect(r.right() - 16.0 - bw, fy, bw, 36.0), "Voltar ao padrão", false, adjusted) {
            reset(app, sec.0, exe);
        }
    }
    ui.end_scroll();
}

/// Abre a aba dona do dado já com o jogo, quando ela tem como escolher.
fn go_to(app: &mut App, page: Page, exe: &str, path: &str) {
    match page {
        Page::Nvidia => {
            super::nvidia::add_game(app, exe);
            app.cfg.save();
            app.go(page);
        }
        Page::Shaders => {
            let prof = crate::shader_profile::path_for(Path::new(exe));
            if prof.exists() {
                super::shaders::open_profile(app, &prof);
                app.refresh_page();
            } else {
                if !path.is_empty() {
                    super::shaders::add_game(app, path);
                }
                app.go(page);
            }
        }
        _ => app.go(page),
    }
}

/// Voltar ao padrão: apaga a linha daquela função para o jogo, como a aba dela faz.
fn reset(app: &mut App, bit: u16, exe: &str) {
    let same = |e: &str| e.eq_ignore_ascii_case(exe);
    match bit {
        gm::METER => {
            if let Some(g) = app.cfg.meter_games.iter_mut().find(|g| same(&g.exe)) {
                (g.limit_mode, g.hdr, g.preset, g.show) = (0, 0, 0, 0);
                let g = g.clone();
                crate::meter::write_limit(&g);
                crate::meter::write_preset(&g);
            }
        }
        gm::ENERGY => {
            if let Some(g) = app.cfg.meter_games.iter_mut().find(|g| same(&g.exe)) {
                g.power.clear();
            }
        }
        gm::HDR => {
            app.cfg.hdr_games.retain(|g| !same(&g.exe));
            autohdr::apply(exe, autohdr::DEFAULT);
        }
        gm::SOUND => {
            if let Some(a) = app.cfg.snd_apps.iter_mut().find(|a| same(&a.exe)) {
                a.output.clear();
                a.input.clear();
            }
            app.route_app(exe);
            app.cfg.snd_apps.retain(|a| !same(&a.exe));
            app.cfg.eq_apps.retain(|(e, _)| !same(e));
        }
        gm::PAD => {
            for mut p in crate::sys::remap::profiles().into_iter().filter(|p| p.games.iter().any(|g| same(g))) {
                p.games.retain(|g| !same(g));
                if !crate::sys::remap::save(&p) {
                    app.toast.info("Não deu para salvar o perfil.");
                }
            }
            gm::forget_extra();
        }
        gm::FULL => app.cfg.fs_apps.retain(|a| !same(&a.exe)),
        _ => {}
    }
    app.cfg.save();
}

fn meter(ui: &mut Ui, app: &mut App, r: Rect, exe: &str, path: &str, protected: bool) {
    let cap = crate::meter::first_limit(app.refresh_hz());
    let at = app.cfg.meter_games.iter().position(|g| g.exe.eq_ignore_ascii_case(exe));
    // Sem linha ainda, mostra o que a aba Medidor poria ao ver o jogo; só grava quando algo muda.
    let mut g = at.map(|i| app.cfg.meter_games[i].clone()).unwrap_or_else(|| MeterGame { exe: exe.to_string(), path: path.to_string(), limit_fps: cap, limit_real: true, inject: true, last: crate::config::today(), ..Default::default() });
    let (mut limit, mut preset, mut changed) = (false, false, false);
    let opts = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut m = g.limit_mode as usize;
    if ui.select("gm-ml", row(ui, r, 0, "Limitador"), &opts(&["Desligado", "Automático", "Manual"]), &mut m) {
        g.limit_mode = m as u8;
        if m == 1 {
            g.limit_fps = cap;
            g.measured = false;
        }
        limit = true;
    }
    let fr = row(ui, r, 1, "FPS");
    match g.limit_mode {
        2 => limit |= ui.stepper("gm-mf", fr, &mut g.limit_fps, (20, 1023), 1, ""),
        1 => ui.c.text(fr, &g.limit_fps.to_string(), 13.0, TEXT, Align::Left, Weight::Regular),
        _ => ui.c.text(fr, "—", 13.0, MUTED, Align::Left, Weight::Regular),
    }
    let mut m = crate::meter::uses_injection(&g) as usize;
    if ui.select("gm-mm", row(ui, r, 2, "Método"), &opts(&["Driver", "Injeção"]), &mut m) {
        if m == 1 && protected {
            app.toast.info("Injeção bloqueada: anti-cheat detectado.");
        } else {
            g.inject = m == 1;
            g.inject_off = false;
            limit = true;
        }
    }
    let mut m = g.limit_real as usize;
    if ui.select("gm-mu", row(ui, r, 3, "Limitando"), &opts(&["Com geração", "FPS real"]), &mut m) {
        g.limit_real = m == 1;
        limit = true;
    }
    let mut m = g.hdr as usize;
    if ui.select("gm-mh", row(ui, r, 4, "HDR do Windows ao abrir"), &opts(&["Padrão", "Ligar", "Não ligar"]), &mut m) {
        g.hdr = m as u8;
        changed = true;
    }
    let pr = row(ui, r, 5, "Perfil DLSS");
    match crate::meter::has_dlss(&g.path) {
        Some(true) => {
            let mut m = g.preset as usize;
            if ui.select("gm-mp", pr, &opts(&["Do jogo", "Recomendado", "K", "L", "M"]), &mut m) {
                g.preset = m as u8;
                preset = true;
            }
        }
        Some(false) => ui.c.text(pr, "Sem DLSS", 12.0, MUTED, Align::Left, Weight::Regular),
        None => ui.c.text(pr, "…", 12.0, MUTED, Align::Left, Weight::Regular),
    }
    let mut m = g.show as usize;
    if ui.select("gm-ms", row(ui, r, 6, "Medidor ao abrir"), &opts(&["Nenhum", "Compacto", "Completo"]), &mut m) {
        g.show = m as u8;
        changed = true;
    }
    if !(limit || preset || changed) {
        return;
    }
    match at {
        Some(i) => app.cfg.meter_games[i] = g.clone(),
        None => app.cfg.meter_games.push(g.clone()),
    }
    if limit {
        if !crate::meter::write_limit(&g) {
            app.toast.info("Não consegui gravar no driver NVIDIA.");
        } else if crate::meter::uses_injection(&g) {
            app.toast.info("Com injeção, o limite muda na hora.");
        } else {
            app.toast.info("Limite gravado. Vale na próxima abertura do jogo.");
        }
    }
    if preset && !crate::meter::write_preset(&g) {
        app.toast.info("Não consegui gravar no driver NVIDIA.");
    }
    app.cfg.save();
}

fn energy(ui: &mut Ui, app: &mut App, r: Rect, exe: &str, path: &str) {
    let plans: Vec<(String, String)> = std::iter::once((String::new(), crate::i18n::tr("Não mudar").into_owned())).chain(app.plans.iter().map(|p| (p.id.clone(), p.name.clone()))).collect();
    let names: Vec<String> = plans.iter().map(|p| p.1.clone()).collect();
    let at = app.cfg.meter_games.iter().position(|g| g.exe.eq_ignore_ascii_case(exe));
    let now = at.map_or("", |i| app.cfg.meter_games[i].power.as_str());
    let mut k = plans.iter().position(|p| p.0.eq_ignore_ascii_case(now)).unwrap_or(0);
    if ui.select("gm-e", row(ui, r, 0, "Plano ao abrir o jogo"), &names, &mut k) {
        let id = plans[k].0.clone();
        match at {
            Some(i) => app.cfg.meter_games[i].power = id,
            None => app.cfg.meter_games.push(MeterGame { exe: exe.to_string(), path: path.to_string(), limit_fps: crate::meter::first_limit(app.refresh_hz()), limit_real: true, inject: true, last: crate::config::today(), power: id, ..Default::default() }),
        }
        app.cfg.save();
    }
}

fn hdr(ui: &mut Ui, app: &mut App, r: Rect, exe: &str, path: &str) {
    let modes: Vec<_> = autohdr::MODES.iter().filter(|m| m.0 != autohdr::RTX || autohdr::nvidia()).collect();
    let at = app.cfg.hdr_games.iter().position(|h| h.exe.eq_ignore_ascii_case(exe));
    let mode = at.map_or(autohdr::DEFAULT, |i| app.cfg.hdr_games[i].mode);
    let names: Vec<String> = modes.iter().map(|m| m.1.to_string()).collect();
    let mut k = modes.iter().position(|m| m.0 == mode).unwrap_or(0);
    if ui.select("gm-hm", row(ui, r, 0, "Modo"), &names, &mut k) {
        if super::meter::set_auto_hdr(app, exe, path, modes[k].0) {
            app.cfg.save();
        } else {
            app.toast.info("Não consegui gravar a escolha.");
        }
    }
    let marks = ["Detectar sozinho", "Tem HDR próprio", "Não tem HDR próprio"].map(String::from);
    let native = at.map_or(autohdr::NATIVE_UNKNOWN, |i| app.cfg.hdr_games[i].native);
    let mut k = match native {
        autohdr::NATIVE_YES => 1,
        autohdr::NATIVE_NO => 2,
        _ => 0,
    };
    if ui.select("gm-hn", row(ui, r, 1, "HDR próprio do jogo"), &marks, &mut k) {
        let n = [autohdr::NATIVE_UNKNOWN, autohdr::NATIVE_YES, autohdr::NATIVE_NO][k];
        match app.cfg.hdr_games.iter_mut().find(|h| h.exe.eq_ignore_ascii_case(exe)) {
            Some(h) => h.native = n,
            None => app.cfg.hdr_games.push(HdrGame { exe: exe.to_string(), path: path.to_string(), native: n, ..Default::default() }),
        }
        app.cfg.save();
    }
}

fn sound(ui: &mut Ui, app: &mut App, r: Rect, exe: &str, path: &str, outputs: &[crate::sys::audio::Device], profiles: &[String]) {
    let entry = app.cfg.snd_apps.iter().find(|a| a.exe.eq_ignore_ascii_case(exe)).cloned().unwrap_or_default();
    let mut k = 0;
    let mut pinned = entry.pinned;
    if ui.check("gm-sp", rect(r.x + 10.0, r.y + 44.0, r.w - 20.0, 44.0), "Guardar o volume deste jogo", &mut pinned) {
        super::volume::set_pinned(app, exe, path, pinned, Some(if entry.volume >= 0 { entry.volume as f32 / 100.0 } else { 1.0 }));
    }
    k += 1;
    if entry.pinned {
        let mut v = entry.volume.max(0) as f32 / 100.0;
        if ui.slider("gm-sv", row(ui, r, k, "Volume"), &mut v) {
            super::volume::set_pinned(app, exe, path, true, Some(v));
        }
        k += 1;
    }
    let names: Vec<String> = std::iter::once(crate::i18n::tr("Padrão do Windows").into_owned()).chain(outputs.iter().map(|d| d.name.clone())).collect();
    let mut s = outputs.iter().position(|d| d.id == entry.output).map_or(0, |i| i + 1);
    if ui.select("gm-so", row(ui, r, k, "Saída"), &names, &mut s) {
        let id = if s == 0 { String::new() } else { outputs[s - 1].id.clone() };
        super::volume::set_device(app, exe, path, false, id);
    }
    k += 1;
    let names: Vec<String> = std::iter::once(crate::i18n::tr("Nenhum").into_owned()).chain(profiles.iter().cloned()).collect();
    let now = app.cfg.eq_apps.iter().find(|(e, _)| e.eq_ignore_ascii_case(exe)).map(|(_, p)| p.clone()).unwrap_or_default();
    let mut s = profiles.iter().position(|p| *p == now).map_or(0, |i| i + 1);
    if ui.select("gm-se", row(ui, r, k, "Equalizador"), &names, &mut s) {
        app.cfg.eq_apps.retain(|(e, _)| !e.eq_ignore_ascii_case(exe));
        if s > 0 {
            app.cfg.eq_apps.push((exe.to_string(), profiles[s - 1].clone()));
        }
        app.cfg.save();
    }
}

fn pad(ui: &mut Ui, app: &mut App, r: Rect, exe: &str, extra: &gm::Extra) {
    let names: Vec<String> = std::iter::once(crate::i18n::tr("Nenhum").into_owned()).chain(extra.pad_names.iter().cloned()).collect();
    let now = extra.pads.iter().find(|(_, e)| e.eq_ignore_ascii_case(exe)).map(|(p, _)| p.as_str());
    let mut k = now.and_then(|n| extra.pad_names.iter().position(|p| p == n)).map_or(0, |i| i + 1);
    if ui.select("gm-pad", row(ui, r, 0, "Perfil de controle"), &names, &mut k) {
        // Os perfis inteiros só na mudança: ler todos a cada quadro pesa. Como na aba Controles, o
        // escolhido passa a abrir com este jogo e os outros dele continuam; "Nenhum" tira o jogo de todos.
        // shortcut: com o jogo em vários perfis, a caixa mostra só o primeiro; listar todos se alguém usar assim.
        let pick = k.checked_sub(1).and_then(|i| extra.pad_names.get(i));
        let mut ok = true;
        for mut p in crate::sys::remap::profiles() {
            if Some(&p.name) == pick {
                p.games = vec![exe.to_lowercase()];
            } else if pick.is_none() && p.games.iter().any(|g| g.eq_ignore_ascii_case(exe)) {
                p.games.retain(|g| !g.eq_ignore_ascii_case(exe));
            } else {
                continue;
            }
            ok &= crate::sys::remap::save(&p);
        }
        if !ok {
            app.toast.info("Não deu para salvar o perfil.");
        }
        gm::forget_extra();
    }
}

fn shader(ui: &mut Ui, app: &mut App, r: Rect, path: &str, exe: &str) {
    let has = crate::shader_profile::path_for(Path::new(exe)).exists();
    let t = if has { "Este jogo tem perfil de shader, com os filtros e ajustes só dele." } else if path.is_empty() { "Sem perfil de shader." } else { "Sem perfil de shader. Abrir na aba cria um para este jogo." };
    note(ui, r, 0, t, TEXT2);
    let _ = app;
}

fn full(ui: &mut Ui, app: &mut App, r: Rect, exe: &str, path: &str) {
    let at = app.cfg.fs_apps.iter().position(|a| a.exe.eq_ignore_ascii_case(exe));
    let mut f = at.map(|i| app.cfg.fs_apps[i].clone()).unwrap_or_else(|| FsApp { exe: exe.to_string(), path: path.to_string(), ..Default::default() });
    let mut changed = ui.check("gm-fa", rect(r.x + 10.0, r.y + 44.0, r.w - 20.0, 44.0), "Tela cheia sem bordas sozinho ao abrir", &mut f.auto);
    let kinds: Vec<String> = crate::shader_profile::KINDS.iter().map(|k| k.to_string()).collect();
    changed |= ui.select("gm-fk", row(ui, r, 1, "Tipo de jogo (filtro de upscale)"), &kinds, &mut f.kind);
    if !changed {
        return;
    }
    // Tudo no padrão é o mesmo que não ter a linha.
    match (at, f.auto || f.kind != 0) {
        (Some(i), true) => app.cfg.fs_apps[i] = f,
        (Some(i), false) => {
            app.cfg.fs_apps.remove(i);
        }
        (None, true) => app.cfg.fs_apps.push(f),
        (None, false) => {}
    }
    app.cfg.save();
}

fn nvidia(ui: &mut Ui, app: &mut App, r: Rect, exe: &str, s: &mut St) {
    let key = exe.to_lowercase();
    // Uma leitura do driver por jogo aberto: abrir a sessão do driver leva um tempo.
    let name = s.nv.entry(key.clone()).or_insert_with(|| crate::sys::drs::snapshot(&crate::sys::drs::Profile::Game(exe.to_string()), &[]).map(|p| p.name).unwrap_or_default()).clone();
    if !name.is_empty() {
        note(ui, r, 0, &crate::i18n::tr("Usa o perfil \"{}\" do driver. Os ajustes ficam na aba Driver NVIDIA.").replace("{}", &name), TEXT2);
        return;
    }
    note(ui, r, 0, "Sem perfil no driver: o jogo segue o global.", TEXT2);
    let y = r.y + 48.0 + 44.0;
    let bw = (r.w - 48.0) / 2.0;
    let mut made = None;
    if ui.button("gm-nv-new", rect(r.x + 16.0, y, bw, 36.0), "Criar perfil (segue o global)", false) {
        made = Some(crate::sys::drs::create(exe, &[]));
    }
    if ui.button("gm-nv-copy", rect(r.right() - 16.0 - bw, y, bw, 36.0), "Criar com cópia do global", false) {
        ui.ask("gm-nv-copy", "Criar com cópia do global?", "O perfil novo grava os valores de agora do global. Depois disso, mudar o global não muda mais este jogo.");
    }
    if ui.answered("gm-nv-copy") {
        // shortcut: copia só os ajustes que o Prisma conhece (`nvset`), não o banco inteiro do driver.
        let ver = crate::sys::drs::driver_version().unwrap_or(0);
        let ids: Vec<u32> = crate::sys::nvset::GROUPS.iter().flat_map(|g| g.1.iter()).filter(|st| !st.global_only && st.min_driver <= ver).map(|st| st.id).collect();
        made = Some(crate::sys::drs::create(exe, &ids));
    }
    match made {
        Some(true) => {
            s.nv.remove(&key);
            super::nvidia::add_game(app, exe);
            super::nvidia::stale();
            app.cfg.save();
        }
        Some(false) => app.toast.info("Não consegui gravar no driver NVIDIA."),
        None => {}
    }
}

fn scan_sheet(ui: &mut Ui, app: &mut App, s: &mut St) {
    let Some(found) = s.scan.as_mut() else { return };
    let r = ui.sheet(760.0, 560.0);
    let ix = r.x + 20.0;
    let iw = r.w - 40.0;
    ui.c.text(rect(ix, r.y + 16.0, iw, 26.0), "Jogos encontrados", 17.0, TEXT, Align::Left, Weight::Semibold);
    ui.c.text_wrap(rect(ix, r.y + 44.0, iw, 36.0), "Marque o que é jogo. O nome pode ser trocado, e o exe escolhido é o que os perfis usam.", 12.0, MUTED, Weight::Regular);
    const H: f32 = 96.0;
    let list = rect(ix - 4.0, r.y + 84.0, iw + 8.0, r.h - 84.0 - 64.0);
    ui.begin_scroll("gm-scan", list, found.len() as f32 * H);
    for (i, f) in found.iter_mut().enumerate() {
        let y = list.y + i as f32 * H;
        if y + H < list.y + ui.scroll_y() || y > list.bottom() + ui.scroll_y() {
            continue;
        }
        ui.c.line(list.x + 4.0, y, list.right() - 4.0, y, LINE, 1.0);
        ui.check(&format!("gm-sc{i}"), rect(list.x + 4.0, y + 6.0, 150.0, 40.0), "É jogo", &mut f.on);
        ui.search(&format!("gm-sn{i}"), rect(list.x + 160.0, y + 8.0, list.w - 160.0 - 8.0, 36.0), &mut f.name, "Nome do jogo");
        let exes: Vec<String> = f.cands.iter().map(|c| c.path.strip_prefix(&f.folder).unwrap_or(&c.path).display().to_string()).collect();
        let ew = (list.w - 160.0) * 0.45;
        ui.select(&format!("gm-se{i}"), rect(list.x + 160.0, y + 52.0, ew, 36.0), &exes, &mut f.pick);
        let c = &f.cands[f.pick.min(f.cands.len() - 1)];
        let mut facts = Vec::new();
        if c.api != 0 {
            facts.push(if c.guessed { crate::i18n::tr("{} (provável)").replace("{}", &gm::api_names(c.api)) } else { gm::api_names(c.api) });
        } else {
            facts.push(crate::i18n::tr("sem API de imagem").into_owned());
        }
        if f.emulator {
            facts.push(crate::i18n::tr("emulador").into_owned());
        }
        if f.anti_cheat {
            facts.push(crate::i18n::tr("anti-cheat").into_owned());
        }
        let why = c.why.iter().map(|w| crate::i18n::tr(w).into_owned()).collect::<Vec<_>>().join(", ");
        let fx = list.x + 160.0 + ew + 12.0;
        ui.c.text_fit(rect(fx, y + 50.0, list.right() - fx - 8.0, 20.0), &facts.join(" · "), 12.0, TEXT2, Align::Left, Weight::Regular);
        ui.c.text_fit(rect(fx, y + 70.0, list.right() - fx - 8.0, 18.0), &why, 11.0, MUTED, Align::Left, Weight::Regular);
    }
    ui.end_scroll();
    let n = found.iter().filter(|f| f.on).count();
    let by = r.bottom() - 52.0;
    if ui.button("gm-scan-no", rect(r.right() - 20.0 - 240.0 - 12.0, by, 120.0, 36.0), "Cancelar", false) {
        s.scan = None;
        return;
    }
    let label = crate::i18n::tr("Pôr {} na lista").replace("{}", &n.to_string());
    if ui.button_ex("gm-scan-ok", rect(r.right() - 20.0 - 120.0 - 0.0, by, 120.0 + 0.0, 36.0), &label, true, n > 0) {
        let mut put = 0;
        for f in found.iter().filter(|f| f.on) {
            let c = &f.cands[f.pick.min(f.cands.len() - 1)];
            put += gm::add(&mut app.cfg.games, &c.path.display().to_string(), &f.name) as usize;
        }
        app.cfg.save();
        if put < n {
            app.toast.info("Alguns já estavam na lista.");
        }
        s.scan = None;
        s.tab = 1;
    }
}
