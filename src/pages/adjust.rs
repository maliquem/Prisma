use super::*;
use crate::config::Defaults;

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Configurações", "Início com o Windows, permissões e atalhos gerais.");
    defaults_button(ui, app, a.right() - 124.0, Defaults::Settings, "Volta o atalho de Otimizar, o idioma e a busca de atualização ao padrão.");
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
        if ui.check("auto", rect(r0.x + 16.0, r0.y + 58.0, r0.w - 32.0, 44.0), "Iniciar com o Windows, como administrador", &mut on) && crate::startup::set_enabled(on) {
            app.autostart = on;
        }
        if !app.elevated && ui.button("elev", rect(r0.x + 16.0, r0.y + 102.0, 240.0, 40.0), "Reabrir como administrador", true) {
            crate::startup::relaunch_elevated();
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
            crate::startup::relaunch_elevated();
        }
    }

    let rt = rect(x, top + 162.0, w, 76.0);
    ui.card(rt);
    ui.c.text(rect(rt.x + 16.0, rt.y + 10.0, rt.w - 260.0, 26.0), "Substituir o Gerenciador de Tarefas", 15.0, TEXT, Align::Left, Weight::Semibold);
    let state = if app.replaces_tm { "Ligado: Ctrl+Shift+Esc abre o Prisma." } else { "Desligado: Ctrl+Shift+Esc abre o Gerenciador de Tarefas do Windows." };
    ui.c.text(rect(rt.x + 16.0, rt.y + 38.0, rt.w - 260.0, 22.0), state, 13.0, if app.replaces_tm { ACCENT } else { MUTED }, Align::Left, Weight::Regular);
    let tm_label = if app.replaces_tm { "Desativar" } else { "Ativar" };
    if ui.button("tm", rect(rt.right() - 236.0, rt.y + 18.0, 220.0, 40.0), tm_label, !app.replaces_tm) {
        if app.replaces_tm {
            ui.ask("tm-off", "Devolver o Gerenciador de Tarefas?", "Ctrl+Shift+Esc volta a abrir o Gerenciador de Tarefas do Windows.");
        } else if app.installed {
            ui.ask("tm-on", "Substituir o Gerenciador de Tarefas?", "Ctrl+Shift+Esc passa a abrir o Prisma. Dá para desfazer aqui a qualquer momento.");
        } else {
            ui.ask("tm-on", "Substituir o Gerenciador de Tarefas?", "Ctrl+Shift+Esc passa a abrir este arquivo do Prisma. Se você mover ou apagar o arquivo, o atalho deixa de abrir qualquer coisa até desativar aqui. A versão instalada não tem esse risco.");
        }
    }
    if ui.answered("tm-on") {
        app.set_taskmgr(true);
    }
    if ui.answered("tm-off") {
        app.set_taskmgr(false);
    }

    let half = (w - 12.0) / 2.0;
    let r1 = rect(x, top + 250.0, half, 92.0);
    ui.card(r1);
    ui.label(rect(r1.x + 16.0, r1.y + 10.0, r1.w - 32.0, 18.0), "Atalho de Otimizar", TEXT2);
    if ui.hotkey("hk-opt", rect(r1.x + 16.0, r1.y + 34.0, 240.0, 44.0), &mut app.cfg.hk_optimize) {
        app.cfg.save();
        app.register_hotkeys();
    }

    let rl = rect(r1.right() + 12.0, r1.y, half, 92.0);
    ui.card(rl);
    ui.label(rect(rl.x + 16.0, rl.y + 10.0, rl.w - 32.0, 18.0), "Idioma", TEXT2);

    let ru = rect(x, top + 354.0, w, 112.0);
    draw_update(ui, app, ru);

    if !app.hk_errors.is_empty() {
        let r2 = rect(x, top + 478.0, w, 40.0 + app.hk_errors.len() as f32 * 22.0);
        ui.card(r2);
        ui.c.text(rect(r2.x + 16.0, r2.y + 8.0, r2.w - 32.0, 22.0), "Atalhos que não funcionaram", 14.0, WARN, Align::Left, Weight::Semibold);
        for (i, e) in app.hk_errors.iter().enumerate() {
            ui.label(rect(r2.x + 16.0, r2.y + 32.0 + i as f32 * 22.0, r2.w - 32.0, 20.0), e, TEXT2);
        }
    }

    // Lista por último, para abrir por cima do resto.
    let opts = vec!["Português (Brasil)".to_string(), "English".to_string()];
    let mut v = app.cfg.lang_en as usize;
    if ui.select("lang", rect(rl.x + 16.0, rl.y + 34.0, 240.0, 44.0), &opts, &mut v) {
        app.cfg.lang_en = v == 1;
        app.cfg.save();
        crate::i18n::set_english(app.cfg.lang_en);
    }
}

fn draw_update(ui: &mut Ui, app: &mut App, r: Rect) {
    use crate::sys::update::{self, Phase};
    ui.card(r);
    let s = app.upd.lock().unwrap().clone();
    ui.title(rect(r.x + 16.0, r.y + 10.0, 260.0, 22.0), "Atualizações");
    ui.label(rect(r.x + 16.0, r.y + 32.0, 260.0, 18.0), &format!("Versão {}", update::current()), MUTED);
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
    if ui.check("upd-auto", rect(r.x + 16.0, r.y + 60.0, 320.0, 44.0), "Procurar ao abrir o programa", &mut auto) {
        app.cfg.update_auto = auto;
        app.cfg.save();
    }
}
