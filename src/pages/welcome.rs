//! Boas-vindas: três escolhas marcadas por padrão, aplicadas de uma vez só.

use super::*;
use crate::startup::Plan;

const W: f32 = 640.0;
const ROW: f32 = 84.0;

#[derive(Clone, Copy)]
pub struct Choices {
    pub install: bool,
    pub admin: bool,
    pub taskmgr: bool,
    /// Não mostrar novamente.
    pub hide: bool,
}

impl Choices {
    pub fn new() -> Self {
        Self { install: true, admin: true, taskmgr: true, hide: true }
    }
}

/// Uma escolha já feita não se desfaz aqui: fica marcada e o caminho é Configurações.
fn row(ui: &mut Ui, key: &str, r: Rect, title: &str, body: &str, val: &mut bool, done: bool) {
    ui.card(r);
    let line = rect(r.x + 16.0, r.y + 10.0, r.w - 32.0, 26.0);
    if done {
        let bx = rect(line.x, line.y + 4.0, 18.0, 18.0);
        ui.c.round(bx, 4.0, BORDER2);
        ui.c.icon(bx.x, bx.y, 18.0, &[&[(6.0, 12.5), (10.0, 16.5), (18.0, 7.5)]], TEXT);
        ui.c.text(rect(line.x + 28.0, line.y, line.w - 128.0, line.h), title, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(line.right() - 100.0, line.y, 100.0, line.h), "Já feito", 13.0, ACCENT, Align::Right, Weight::Regular);
    } else {
        ui.check(key, line, title, val);
    }
    ui.c.text_wrap(rect(r.x + 44.0, r.y + 38.0, r.w - 60.0, 40.0), body, 13.0, MUTED, Weight::Regular);
}

fn finish(ui: &mut Ui, app: &mut App, c: Choices) {
    app.welcome = None;
    app.cfg.welcome = !c.hide;
    app.cfg.save();
    let install = c.install && !app.installed;
    let taskmgr = c.taskmgr && !app.replaces_tm;
    let admin = c.admin || app.cfg.always_admin;
    if install || taskmgr || admin != app.cfg.always_admin {
        let restart = admin && (install || !app.elevated);
        app.run_setup(Plan { install, admin, taskmgr, restart });
    }
    ui.s.needs_redraw = true;
}

pub fn draw(ui: &mut Ui, app: &mut App, full: Rect) {
    let Some(mut c) = app.welcome else { return };
    let w = W.min(full.w - 48.0);
    let x = (full.w - w) / 2.0;
    let total = 480.0;
    let mut y = ((full.h - total) / 2.0).max(16.0);

    ui.c.icon(x, y + 4.0, 30.0, &[&[(12., 3.), (3., 20.), (21., 20.), (12., 3.)], &[(12., 3.), (12., 20.)]], ACCENT);
    ui.c.text(rect(x + 42.0, y, w - 242.0, 38.0), "Boas-vindas ao Prisma", 24.0, TEXT, Align::Left, Weight::Semibold);
    let langs = vec!["Português (Brasil)".to_string(), "English".to_string()];
    let mut lang = app.cfg.lang_en as usize;
    if ui.select("w-lang", rect(x + w - 190.0, y, 190.0, 38.0), &langs, &mut lang) {
        app.cfg.lang_en = lang == 1;
        app.cfg.save();
        crate::i18n::set_english(app.cfg.lang_en);
        ui.s.needs_redraw = true;
    }
    y += 44.0;
    ui.c.text(rect(x, y, w, 22.0), "Escolha como ele vai ficar neste PC. Tudo isso muda depois em Configurações.", 14.0, MUTED, Align::Left, Weight::Regular);
    y += 34.0;

    let install = c.install && !app.installed;
    let admin_note = if install || app.installed {
        "Libera o Medidor, a rede por programa, a saúde dos discos e a leitura rápida do NTFS. Abre sem perguntar."
    } else {
        "Libera o Medidor, a rede por programa, a saúde dos discos e a leitura rápida do NTFS. Sem instalar, o Windows pede permissão a cada abertura."
    };
    let tm_note = if install || app.installed {
        "Ctrl+Shift+Esc passa a abrir o Prisma. O Gerenciador do Windows volta quando você quiser."
    } else {
        "Ctrl+Shift+Esc passa a abrir este arquivo. Sem instalar, mover ou apagar o arquivo deixa o atalho sem abrir nada."
    };
    let step = ROW + 10.0;
    row(ui, "w-install", rect(x, y, w, ROW), "Instalar no PC", "Vai para Arquivos de Programas, ganha atalho no menu Iniciar e abre com o Windows. Sem isso, roda daqui mesmo, como portátil.", &mut c.install, app.installed);
    row(ui, "w-admin", rect(x, y + step, w, ROW), "Dar permissão de administrador", admin_note, &mut c.admin, app.cfg.always_admin);
    row(ui, "w-tm", rect(x, y + step * 2.0, w, ROW), "Substituir o Gerenciador de Tarefas", tm_note, &mut c.taskmgr, app.replaces_tm);
    y += step * 3.0 + 4.0;

    ui.check("w-hide", rect(x, y, 220.0, 44.0), "Não mostrar novamente", &mut c.hide);
    let go = rect(x + w - 150.0, y, 150.0, 44.0);
    let guide = rect(go.x - 158.0, y, 150.0, 44.0);
    app.welcome = Some(c);
    if ui.button("w-guide", guide, "Abrir o guia", false) {
        app.page = Page::Guide;
        ui.s.needs_redraw = true;
    }
    if ui.button("w-go", go, "Continuar", true) {
        finish(ui, app, c);
    }
}
