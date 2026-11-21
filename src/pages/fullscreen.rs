use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use super::picker::{self, Target};
use super::*;
use crate::config::Defaults;
use crate::shader_profile::{self as sp, upscale_for, Param, Profile, KINDS, UPSCALE};
use crate::sys::fullscreen::Fit;

/// Janela de ajustes de um filtro de upscale aberta.
struct Tune {
    title: String,
    path: PathBuf,
    prof: Profile,
    params: Vec<Param>,
    save_at: Option<Instant>,
}

static TUNE: Mutex<Option<Tune>> = Mutex::new(None);

/// Grava o pendente; o motor dos jogos abertos lê o perfil de novo na hora.
fn flush(app: &mut App, t: &mut Tune) {
    if t.save_at.take().is_some() && sp::save_filter(&t.path, &t.prof).is_err() {
        app.toast.info("Filtro de upscale: não deu para gravar o perfil");
    }
}

/// Filtro com preset na biblioteca baixada, que tem o que ajustar.
fn tunable(key: &str) -> bool {
    UPSCALE.iter().any(|u| u.0 == key && !u.2.is_empty() && sp::lib_dir().join(u.2).exists())
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let mut tune = TUNE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = tune.as_mut() {
        if t.save_at.is_some_and(|i| i.elapsed().as_millis() > 200) {
            flush(app, t);
        }
        ui.block();
    }
    header(ui, a, "Fullscreenizer", "Deixa a janela em foco em tela cheia sem bordas. O mesmo atalho desfaz.");
    defaults_button(ui, app, a.right() - 124.0, Defaults::Fullscreen, "Volta o atalho ao padrão e desliga o modo automático de todos os programas. A lista continua salva.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let cw = (w - 12.0) / 2.0;
    let r0 = rect(x, top, cw, 92.0);
    ui.card_on(r0);
    ui.label(rect(r0.x + 16.0, r0.y + 10.0, r0.w - 32.0, 18.0), "Atalho", TEXT2);
    if ui.hotkey("hk-full", rect(r0.x + 16.0, r0.y + 34.0, 240.0, 44.0), &mut app.cfg.hk_fullscreen) {
        app.cfg.save();
        app.register_hotkeys();
    }
    let r1 = rect(x + cw + 12.0, top, cw, 92.0);
    ui.card(r1);
    let fw = (r1.w - 44.0) / 2.0;
    ui.label(rect(r1.x + 16.0, r1.y + 10.0, fw, 18.0), "Proporção", TEXT2);
    let fits = ["Manter (barras pretas)", "Esticar", "Escala inteira"].map(|s| crate::i18n::tr(s).into_owned());
    let mut f = Fit::ALL.iter().position(|x| *x == app.cfg.fs_fit).unwrap_or(0);
    ui.tip(rect(r1.x + 16.0, r1.y + 34.0, fw, 44.0), "Manter: o jogo que aceita mudar de tamanho ocupa o monitor inteiro na proporção dele; o de tamanho fixo fica com barras pretas. Esticar cobre o monitor sempre.");
    if ui.select("fs-fit", rect(r1.x + 16.0, r1.y + 34.0, fw, 44.0), &fits, &mut f) {
        app.cfg.fs_fit = Fit::ALL[f];
        app.cfg.save();
    }
    // Os filtros abertos vêm da biblioteca de shaders, baixada à parte: não vão junto com o Prisma.
    let fx = r1.x + 28.0 + fw;
    ui.label(rect(fx, r1.y + 10.0, fw, 18.0), "Filtros abertos", TEXT2);
    let have = crate::shader_profile::lib_dir().join(UPSCALE[1].2).exists();
    match super::shaders::lib_progress() {
        Ok(Some(p)) => {
            let bar = rect(fx, r1.y + 52.0, fw, 6.0);
            ui.c.round(bar, 3.0, BORDER);
            ui.c.round(rect(bar.x, bar.y, bar.w * p.min(1.0), 6.0), 3.0, ACCENT);
        }
        _ if have => ui.c.text_fit(rect(fx, r1.y + 34.0, fw, 44.0), "Baixados", 13.0, MUTED, Align::Left, Weight::Regular),
        r => {
            if ui.button("fs-dl", rect(fx, r1.y + 34.0, fw, 44.0), if r.is_err() { "Tentar de novo" } else { "Baixar filtros" }, true) {
                super::shaders::download_lib();
            }
        }
    }

    // Um filtro para cada tipo de jogo; o tipo de cada programa fica na lista de salvos.
    let top = top + 104.0;
    let fc = rect(x, top, w, 92.0);
    ui.card(fc);
    let sw = (fc.w - 32.0 - 24.0) / 3.0;
    for (n, kind) in KINDS.iter().enumerate() {
        let sx = fc.x + 16.0 + n as f32 * (sw + 12.0);
        let label = crate::i18n::tr("Filtro de upscale · {}").replace("{}", &crate::i18n::tr(kind));
        ui.c.text_fit(rect(sx, fc.y + 10.0, sw, 18.0), &label, 13.0, TEXT2, Align::Left, Weight::Regular);
        let opts: Vec<_> = upscale_for(n).collect();
        let names: Vec<String> = opts.iter().map(|u| crate::i18n::tr(u.1).into_owned()).collect();
        let mut k = opts.iter().position(|u| u.0 == app.cfg.fs_filters[n]).unwrap_or(0);
        if ui.select(&format!("fs-filter{n}"), rect(sx, fc.y + 34.0, sw - 100.0, 44.0), &names, &mut k) {
            app.cfg.fs_filters[n] = opts[k].0.to_string();
            app.cfg.save();
        }
        let key = opts[k].0;
        if ui.button_ex(&format!("fs-tune{n}"), rect(sx + sw - 92.0, fc.y + 34.0, 92.0, 44.0), "Ajustes", false, tunable(key)) {
            if let Some((path, prof)) = sp::filter_profile(key) {
                let params = sp::params(&prof);
                *tune = Some(Tune { title: crate::i18n::tr(opts[k].1).into_owned(), path, prof, params, save_at: None });
            }
        }
    }
    let chosen = |key: &str| app.cfg.fs_filters.iter().any(|f| f == key);
    let hint = if chosen("driver") {
        "Filtro do driver: o painel da placa só amplia o jogo em tela cheia exclusiva numa resolução menor que a do monitor, não na janela sem bordas. NVIDIA: Painel de Controle › Ajustar o tamanho e a posição da área de trabalho › Dimensionamento de imagem (NIS, com nitidez). AMD: Adrenalin › Gráficos › Radeon Super Resolution. Intel: Centro de Comando Gráfico › Tela › Escala."
    } else if app.cfg.fs_filters.iter().all(String::is_empty) {
        "Quem usa o Fullscreenizer uma vez entra na lista. Marque Automático para aplicar sempre que o programa abrir."
    } else {
        "O filtro entra quando o jogo é menor que o monitor: o Prisma cria o perfil de shader do jogo (página Shaders) e desenha por cima. Jogo com perfil de shader próprio ou anti-cheat fica sem ele."
    };
    ui.c.text_wrap(rect(x + 4.0, top + 100.0, w - 8.0, 56.0), hint, 13.0, TEXT2, Weight::Regular);

    let top = top + 164.0;
    let h = a.h - top - 20.0;
    let t = rect(x, top, cw, h);
    ui.card(t);
    ui.title(rect(t.x + 16.0, t.y + 12.0, t.w - 32.0, 22.0), "Janelas abertas");
    let list = rect(t.x + 8.0, t.y + 42.0, t.w - 16.0, t.h - 50.0);
    ui.begin_scroll("wins", list, app.windows.len() as f32 * 54.0);
    let mut toggle = None;
    for (i, wi) in app.windows.iter().enumerate() {
        let y = list.y + i as f32 * 54.0;
        let on = app.full.is_on(wi.hwnd);
        if on {
            ui.c.round(rect(list.x, y, list.w, 52.0), 8.0, CARD2);
        }
        ui.file_icon(&wi.path, &wi.exe, rect(list.x + 8.0, y + 15.0, 22.0, 22.0));
        ui.c.text(rect(list.x + 42.0, y + 6.0, list.w - 170.0, 20.0), &wi.title, 14.0, TEXT, Align::Left, Weight::Regular);
        let st = format!("{} · {} × {}", if on { "Sem bordas" } else { "Janela" }, wi.size.0, wi.size.1);
        ui.c.text(rect(list.x + 42.0, y + 27.0, list.w - 170.0, 18.0), &st, 12.0, MUTED, Align::Left, Weight::Regular);
        let label = if on { "Restaurar" } else { "Tela cheia" };
        if ui.button(&format!("fs{}", wi.hwnd.0 as isize), rect(list.right() - 118.0, y + 8.0, 104.0, 36.0), label, false) {
            toggle = Some((wi.hwnd, wi.exe.clone()));
        }
    }
    ui.end_scroll();
    if let Some((h, exe)) = toggle {
        app.toggle_window(h, &exe);
    }

    let s = rect(t.right() + 12.0, t.y, cw, h);
    ui.card(s);
    ui.title(rect(s.x + 16.0, s.y + 12.0, s.w - 150.0, 22.0), "Programas salvos");
    picker::add_button(ui, app, rect(s.right() - 124.0, s.y + 8.0, 108.0, 32.0), Target::Fullscreen);
    let list = rect(s.x + 8.0, s.y + 46.0, s.w - 16.0, s.h - 54.0);
    if app.cfg.fs_apps.is_empty() {
        ui.c.text_wrap(rect(list.x + 8.0, list.y + 4.0, list.w - 16.0, 44.0), "Nenhum programa ainda. Use o Fullscreenizer em uma janela ou clique em Adicionar.", 13.0, MUTED, Weight::Regular);
    }
    ui.begin_scroll("fs-apps", list, app.cfg.fs_apps.len() as f32 * 48.0);
    let mut remove = None;
    let mut open = None;
    let mut changed = false;
    let kinds = KINDS.map(|k| crate::i18n::tr(k).into_owned());
    let apps = &mut app.cfg.fs_apps;
    for (i, p) in apps.iter_mut().enumerate() {
        let y = list.y + i as f32 * 48.0;
        ui.c.line(list.x + 8.0, y, list.right() - 14.0, y, LINE, 1.0);
        ui.file_icon(&p.path, &p.exe, rect(list.x + 8.0, y + 12.0, 24.0, 24.0));
        ui.c.text(rect(list.x + 42.0, y, list.w - 386.0, 48.0), app_name(&p.exe), 14.0, TEXT, Align::Left, Weight::Regular);
        // O perfil de shader do jogo, com os ajustes só dele, abre na página Shaders. Ícone, que a linha é estreita.
        let prof = sp::path_for(Path::new(&p.exe));
        let has = prof.exists();
        let tb = rect(list.right() - 334.0, y + 12.0, 24.0, 24.0);
        if has && ui.is_hover(tb) {
            ui.c.round(tb, 6.0, BORDER);
        }
        ui.c.icon(tb.x + 4.0, tb.y + 4.0, 16.0, &[&[(4., 7.), (20., 7.)], &[(4., 17.), (20., 17.)], &[(9., 4.), (9., 10.)], &[(15., 14.), (15., 20.)]], if has { TEXT2 } else { BORDER });
        ui.tip(tb, "Ajustar: abre o perfil de shader deste jogo na página Shaders, com valores só dele. Aparece depois que o filtro de upscale entrou no jogo.");
        if has && ui.hit(&format!("fsj{i}"), tb) {
            open = Some(prof);
        }
        changed |= ui.select(&format!("fsk{i}"), rect(list.right() - 300.0, y + 6.0, 122.0, 36.0), &kinds, &mut p.kind);
        changed |= ui.check(&format!("fsa{i}"), rect(list.right() - 170.0, y + 2.0, 120.0, 44.0), "Automático", &mut p.auto);
        let b = rect(list.right() - 44.0, y + 12.0, 24.0, 24.0);
        if ui.is_hover(b) {
            ui.c.round(b, 6.0, BORDER);
        }
        ui.c.icon(b.x + 4.0, b.y + 4.0, 16.0, &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]], MUTED);
        if ui.hit(&format!("fsx{i}"), b) {
            remove = Some(i);
        }
    }
    ui.end_scroll();
    if let Some(i) = remove {
        app.cfg.fs_apps.remove(i);
        changed = true;
    }
    if changed {
        app.cfg.save();
    }
    if let Some(prof) = open {
        super::shaders::open_profile(app, &prof);
    }
    if let Some(t) = tune.as_mut() {
        if tune_sheet(ui, t) {
            flush(app, t);
            *tune = None;
        }
    }
}

/// Parâmetros do filtro, que valem para todo jogo que segue ele. Devolve true ao fechar.
fn tune_sheet(ui: &mut Ui, t: &mut Tune) -> bool {
    let r = ui.sheet(640.0, 540.0);
    let ix = r.x + 20.0;
    let iw = r.w - 40.0;
    let title = crate::i18n::tr("Ajustes · {}").replace("{}", &t.title);
    ui.c.text_fit(rect(ix, r.y + 18.0, iw - 180.0, 26.0), &title, 17.0, TEXT, Align::Left, Weight::Semibold);
    if ui.button_ex("fst-reset", rect(r.right() - 20.0 - 160.0, r.y + 14.0, 160.0, 32.0), "Restaurar padrão", false, !t.prof.params.is_empty()) {
        t.prof.params.clear();
        t.save_at.get_or_insert_with(Instant::now);
    }
    let list = rect(ix - 4.0, r.y + 60.0, iw + 8.0, r.h - 60.0 - 110.0);
    if t.params.is_empty() {
        ui.c.text(rect(ix, list.y, iw, 24.0), "Este filtro não tem parâmetros.", 13.0, MUTED, Align::Left, Weight::Regular);
    }
    let rows: Vec<&Param> = t.params.iter().collect();
    if let Some((name, v)) = super::shaders::param_list(ui, "fst", list, &rows, &t.prof) {
        t.prof.set_param(&name, v);
        t.save_at.get_or_insert_with(Instant::now);
    }
    ui.c.text_wrap(rect(ix, r.bottom() - 102.0, iw, 50.0), "Vale para todos os jogos com este filtro; o jogo ajustado à parte (ícone Ajustar dos salvos) fica com os dele. Com o jogo aberto, a mudança aparece na hora. Botão direito volta o valor.", 11.0, MUTED, Weight::Regular);
    ui.button("fst-close", rect(r.right() - 20.0 - 120.0, r.bottom() - 48.0, 120.0, 36.0), "Fechar", true)
}
