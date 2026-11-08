use super::*;
use crate::config::Defaults;

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Configurações", "Início com o Windows, permissões e atalhos gerais.");
    defaults_button(ui, app, a.right() - 124.0, Defaults::Settings, "Volta os atalhos de Otimizar, Fechar à força e Reiniciar o Explorer, o idioma e a busca de atualização ao padrão.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let r0 = rect(x, top, w, 150.0);
    ui.card(r0);
    let (mode, hint) = if app.installed {
        ("Versão instalada", "Em Arquivos de Programas, com atalho e entrada para desinstalar.")
    } else {
        ("Versão portátil", "Roda de qualquer pasta. As configurações ficam ao lado do programa, em PrismaData.")
    };
    ui.c.text(rect(r0.x + 16.0, r0.y + 8.0, r0.w - 32.0, 26.0), mode, 15.0, TEXT, Align::Left, Weight::Semibold);
    ui.c.text(rect(r0.x + 16.0, r0.y + 34.0, r0.w - 32.0, 20.0), hint, 13.0, MUTED, Align::Left, Weight::Regular);
    if app.installed {
        let mut on = app.autostart;
        if ui.check("auto", rect(r0.x + 16.0, r0.y + 58.0, 330.0, 44.0), "Iniciar com o Windows", &mut on) && crate::startup::set_enabled(on, app.cfg.always_admin) {
            app.autostart = on;
        }
        admin_check(ui, app, rect(r0.x + 356.0, r0.y + 58.0, r0.w - 372.0, 44.0));
        if !app.elevated && ui.button("elev", rect(r0.x + 16.0, r0.y + 102.0, 240.0, 40.0), "Reabrir como administrador", true) {
            crate::startup::relaunch_elevated("");
        }
    } else {
        let t = "Instalar libera o início com o Windows e a troca do Gerenciador de Tarefas.";
        ui.c.text(rect(r0.x + 16.0, r0.y + 62.0, r0.w - 32.0, 22.0), t, 13.0, TEXT2, Align::Left, Weight::Regular);
        if ui.button("install", rect(r0.x + 16.0, r0.y + 94.0, 200.0, 44.0), "Instalar no PC", true) {
            ui.ask("install", "Instalar no PC?", "O Prisma vai para Arquivos de Programas e reabre de lá. O Windows pede permissão de administrador.");
        }
        if ui.answered("install") {
            app.install_now();
        }
        if !app.elevated && ui.button("elev", rect(r0.x + 224.0, r0.y + 94.0, 240.0, 44.0), "Reabrir como administrador", false) {
            crate::startup::relaunch_elevated("");
        }
        admin_check(ui, app, rect(r0.x + 480.0, r0.y + 94.0, r0.w - 496.0, 44.0));
    }

    let rt = rect(x, top + 162.0, w, 76.0);
    ui.card(rt);
    ui.c.text(rect(rt.x + 16.0, rt.y + 10.0, rt.w - 260.0, 26.0), "Substituir o Gerenciador de Tarefas", 15.0, TEXT, Align::Left, Weight::Semibold);
    let state = if app.replaces_tm {
        "Ligado: Ctrl+Shift+Esc abre o Prisma."
    } else if app.installed {
        "Desligado: Ctrl+Shift+Esc abre o Gerenciador de Tarefas do Windows."
    } else {
        "Desligado. Só na versão instalada, por segurança."
    };
    ui.c.text(rect(rt.x + 16.0, rt.y + 38.0, rt.w - 260.0, 22.0), state, 13.0, if app.replaces_tm { ACCENT } else { MUTED }, Align::Left, Weight::Regular);
    let tm_label = if app.replaces_tm { "Desativar" } else { "Ativar" };
    // Na portátil só dá para desligar: ligar apontaria o atalho para um arquivo que qualquer programa troca.
    let tm_usable = app.replaces_tm || app.installed;
    if ui.button_ex("tm", rect(rt.right() - 236.0, rt.y + 18.0, 220.0, 40.0), tm_label, !app.replaces_tm, tm_usable) {
        if app.replaces_tm {
            ui.ask("tm-off", "Devolver o Gerenciador de Tarefas?", "Ctrl+Shift+Esc volta a abrir o Gerenciador de Tarefas do Windows.");
        } else {
            ui.ask("tm-on", "Substituir o Gerenciador de Tarefas?", "Ctrl+Shift+Esc passa a abrir o Prisma. Dá para desfazer aqui a qualquer momento.");
        }
    }
    if ui.answered("tm-on") {
        app.set_taskmgr(true);
    }
    if ui.answered("tm-off") {
        app.set_taskmgr(false);
    }

    let rf = rect(x, top + 250.0, w, 76.0);
    ui.card(rf);
    ui.c.text(rect(rf.x + 16.0, rf.y + 10.0, rf.w - 290.0, 26.0), "Fechar à força", 15.0, TEXT, Align::Left, Weight::Semibold);
    ui.c.text(rect(rf.x + 16.0, rf.y + 38.0, rf.w - 290.0, 22.0), "Encerra na hora o programa em foco, mesmo travado. O que não foi salvo é perdido.", 13.0, MUTED, Align::Left, Weight::Regular);
    if ui.hotkey("hk-force", rect(rf.right() - 256.0, rf.y + 16.0, 240.0, 44.0), &mut app.cfg.hk_force) {
        app.cfg.save();
        app.register_hotkeys();
    }

    let third = (w - 24.0) / 3.0;
    let field = (third - 32.0).min(240.0);
    let r1 = rect(x, top + 338.0, third, 92.0);
    ui.card(r1);
    ui.label(rect(r1.x + 16.0, r1.y + 10.0, r1.w - 32.0, 18.0), "Atalho de Otimizar", TEXT2);
    if ui.hotkey("hk-opt", rect(r1.x + 16.0, r1.y + 34.0, field, 44.0), &mut app.cfg.hk_optimize) {
        app.cfg.save();
        app.register_hotkeys();
    }

    // O atalho fecha a barra de tarefas: pede dois toques, e o botão pergunta antes.
    let re = rect(r1.right() + 12.0, r1.y, third, 92.0);
    ui.card(re);
    ui.label(rect(re.x + 16.0, re.y + 10.0, re.w - 72.0, 18.0), "Reiniciar o Explorer", TEXT2);
    let go = rect(re.right() - 52.0, re.y + 6.0, 40.0, 24.0);
    ui.c.round(go, 6.0, if ui.is_hover(go) { CARD2 } else { SIDE });
    ui.c.icon(go.x + 10.0, go.y + 2.0, 20.0, &[&[(18., 8.), (16., 5.5), (12., 4.), (8., 5.), (5., 8.), (4., 12.), (5., 16.), (8., 19.), (12., 20.), (16., 19.), (19., 16.)], &[(18., 3.), (18., 8.), (13., 8.)]], ACCENT);
    ui.tip(go, "Reiniciar agora");
    if ui.hit("explorer-now", go) {
        super::monitor::ask_explorer(ui);
    }
    if ui.answered("explorer") {
        app.restart_explorer();
    }
    if ui.hotkey("hk-explorer", rect(re.x + 16.0, re.y + 34.0, field, 44.0), &mut app.cfg.hk_explorer) {
        app.cfg.save();
        app.register_hotkeys();
    }

    let rl = rect(re.right() + 12.0, r1.y, third, 92.0);
    ui.card(rl);
    ui.label(rect(rl.x + 16.0, rl.y + 10.0, rl.w - 32.0, 18.0), "Idioma", TEXT2);

    let ru = rect(x, top + 442.0, w, 112.0);
    draw_update(ui, app, ru);

    if !app.hk_errors.is_empty() {
        let r2 = rect(x, top + 566.0, w, 40.0 + app.hk_errors.len() as f32 * 22.0);
        ui.card(r2);
        ui.c.text(rect(r2.x + 16.0, r2.y + 8.0, r2.w - 32.0, 22.0), "Atalhos que não funcionaram", 14.0, WARN, Align::Left, Weight::Semibold);
        for (i, e) in app.hk_errors.iter().enumerate() {
            ui.label(rect(r2.x + 16.0, r2.y + 32.0 + i as f32 * 22.0, r2.w - 32.0, 20.0), e, TEXT2);
        }
    }

    // Lista por último, para abrir por cima do resto.
    let opts = vec!["Português (Brasil)".to_string(), "English".to_string()];
    let mut v = app.cfg.lang_en as usize;
    if ui.select("lang", rect(rl.x + 16.0, rl.y + 34.0, field, 44.0), &opts, &mut v) {
        app.cfg.lang_en = v == 1;
        app.cfg.save();
        crate::i18n::set_english(app.cfg.lang_en);
    }
}

fn admin_check(ui: &mut Ui, app: &mut App, r: Rect) {
    let mut on = app.cfg.always_admin;
    if ui.check("admin", r, "Sempre abrir como administrador", &mut on) {
        app.set_admin(on);
    }
}

fn draw_update(ui: &mut Ui, app: &mut App, r: Rect) {
    use crate::sys::update::{self, Phase};
    ui.card(r);
    let s = app.upd.lock().unwrap().clone();
    ui.title(rect(r.x + 16.0, r.y + 10.0, 260.0, 22.0), "Atualizações");
    let version = format!("Versão {}", update::current());
    ui.label(rect(r.x + 16.0, r.y + 32.0, 260.0, 18.0), &version, MUTED);
    let vw = measure(&version, 13.0, Weight::Regular, None).0;
    let news = rect(r.x + 28.0 + vw, r.y + 30.0, (248.0 - vw - 12.0).max(0.0), 22.0);
    let hover = ui.is_hover(news);
    ui.c.text(news, "Novidades", 13.0, if hover { TEXT } else { ACCENT }, Align::Left, Weight::Semibold);
    if ui.hit("news", news) {
        app.news_from = None;
        app.page = Page::News;
        ui.s.needs_redraw = true;
    }
    let (text, color) = match &s.phase {
        Phase::Idle => ("Busca só o GitHub do Prisma, sem enviar dados seus.".to_string(), TEXT2),
        Phase::Checking => ("Procurando…".to_string(), TEXT2),
        Phase::Latest => ("Você está na versão mais recente.".to_string(), TEXT2),
        Phase::Available => (format!("Versão {} disponível", s.version), ACCENT),
        Phase::Downloading(p) => (format!("Baixando a atualização… {p}%"), ACCENT),
        Phase::Installing => ("Instalando…".to_string(), ACCENT),
        Phase::Failed(e) => (e.to_string(), WARN),
    };
    ui.c.text_wrap(rect(r.x + 276.0, r.y + 10.0, r.w - 276.0 - 256.0, 44.0), &text, 13.0, color, Weight::Regular);
    let b = rect(r.right() - 236.0, r.y + 12.0, 220.0, 40.0);
    let working = matches!(s.phase, Phase::Checking | Phase::Downloading(_) | Phase::Installing);
    if s.phase == Phase::Available {
        if ui.button("upd-go", b, &format!("Instalar a versão {}", s.version), true) {
            ui.ask("upd-go", "Instalar a atualização?", &format!("O Prisma vai baixar a versão {}, reiniciar e continuar com as suas configurações.", s.version));
        }
        if ui.answered("upd-go") {
            app.update_now();
        }
    } else if ui.button_ex("upd-check", b, "Procurar agora", false, !working) {
        update::check(app.upd.clone());
    }
    let mut auto = app.cfg.update_auto;
    if ui.check("upd-auto", rect(r.x + 16.0, r.y + 60.0, 400.0, 44.0), "Procurar atualizações sozinho, uma vez por dia", &mut auto) {
        app.cfg.update_auto = auto;
        app.cfg.save();
    }
    let mut hello = app.cfg.welcome;
    if ui.check("welcome", rect(r.x + 430.0, r.y + 60.0, r.w - 446.0, 44.0), "Mostrar as boas-vindas ao abrir", &mut hello) {
        app.cfg.welcome = hello;
        app.cfg.save();
    }
}
