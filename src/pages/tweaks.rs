//! Ajustes do Windows: auditoria primeiro, perfis em camadas e o que ficou de fora.

use super::*;
use crate::sys::audit::{self, Fix};
use crate::sys::disks::Level;
use crate::sys::tweaks::{self, Class, Status, CATALOG, LEFT_OUT};

fn color(l: Level) -> Color {
    match l {
        Level::Good => ACCENT,
        Level::Warn => WARN,
        Level::Risk => REC,
        Level::Unknown => MUTED,
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Ajustes do Windows", "Só o que tem efeito comprovado. Tudo pode ser desfeito.");
    defaults_button(ui, app, a.right() - 484.0, crate::config::Defaults::Tweaks, "Volta as opções Ver log e Restauração ao padrão. Os ajustes aplicados continuam como estão: para reverter, use Desfazer.");
    let bw = 112.0;
    for (i, name) in ["Auditoria", "Perfis", "Fora da lista"].iter().enumerate() {
        let b = rect(a.right() - 28.0 - (3 - i) as f32 * (bw + 8.0) + 8.0, 28.0, bw, 40.0);
        if ui.button(name, b, name, app.tw_tab == i as u8) {
            app.tw_tab = i as u8;
        }
    }
    let area = rect(a.x + 28.0, 88.0, a.w - 56.0, a.h - 108.0);
    match app.tw_tab {
        0 => draw_audit(ui, app, area),
        1 => draw_profiles(ui, app, area),
        _ => draw_left_out(ui, area),
    }
}

fn draw_audit(ui: &mut Ui, app: &mut App, r: Rect) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, 500.0, 22.0), "O que o Prisma achou neste PC");
    ui.label(rect(r.x + 16.0, r.y + 34.0, 500.0, 18.0), "Só leitura: nada foi alterado.", MUTED);
    if ui.button("aud", rect(r.right() - 156.0, r.y + 12.0, 140.0, 38.0), "Verificar de novo", false) {
        app.start_audit();
    }
    let found = app.audit.lock().unwrap().clone();
    let list = rect(r.x + 8.0, r.y + 60.0, r.w - 16.0, r.h - 68.0);
    let Some(items) = found else {
        ui.c.text(list, "Verificando…", 14.0, TEXT2, Align::Center, Weight::Regular);
        return;
    };
    ui.begin_scroll("aud", list, items.len() as f32 * 66.0);
    let mut go: Option<Fix> = None;
    for (i, f) in items.iter().enumerate() {
        let y = list.y + i as f32 * 66.0;
        let row = rect(list.x, y, list.w, 62.0);
        if matches!(f.level, Level::Risk | Level::Warn) {
            ui.c.round(row, 8.0, CARD2);
        }
        ui.c.circle(row.x + 18.0, y + 21.0, 5.0, color(f.level));
        let tw = row.w - 210.0;
        ui.c.text(rect(row.x + 34.0, y + 8.0, tw, 22.0), &f.title, 14.0, TEXT, Align::Left, Weight::Semibold);
        ui.c.text(rect(row.x + 34.0, y + 31.0, tw, 20.0), &f.detail, 12.0, TEXT2, Align::Left, Weight::Regular);
        let label = match f.fix {
            Fix::CoreIsolation => "Abrir tela do Windows",
            Fix::Disks => "Ver discos",
            Fix::Tools => "Ferramentas",
            Fix::None => "",
        };
        if !label.is_empty() && ui.button(&format!("fx{i}"), rect(row.right() - 176.0, y + 13.0, 168.0, 36.0), label, false) {
            go = Some(f.fix);
        }
    }
    ui.end_scroll();
    match go {
        Some(Fix::CoreIsolation) => {
            let _ = std::process::Command::new(crate::sys::system_tool("explorer")).arg("windowsdefender://coreisolation").spawn();
        }
        Some(Fix::Disks) => app.go(Page::Storage),
        Some(Fix::Tools) => app.go(Page::Tools),
        _ => {}
    }
}

fn draw_profiles(ui: &mut Ui, app: &mut App, r: Rect) {
    let run = app.tw_run.lock().unwrap().clone();
    let cw = (r.w - 24.0) / 3.0;
    let hints = [("Jogo", 'J', "Verifica o que importa para jogar e tira ruído. Não promete FPS."), ("Desenvolvimento", 'D', "Preserva WSL e virtualização, e liga caminhos longos."), ("Privacidade", 'P', "Reduz o envio de dados até o limite do Pro.")];
    let mut changed = false;
    for (i, (name, code, hint)) in hints.iter().enumerate() {
        let c = rect(r.x + i as f32 * (cw + 12.0), r.y, cw, 96.0);
        let mut on = app.tw_layers.contains(code);
        if on {
            ui.card_on(c);
        } else {
            ui.card(c);
        }
        if ui.check(&format!("pf{code}"), rect(c.x + 14.0, c.y + 6.0, c.w - 28.0, 36.0), name, &mut on) {
            if on {
                app.tw_layers.push(*code);
            } else {
                app.tw_layers.retain(|x| x != code);
            }
            changed = true;
        }
        ui.c.text_wrap(rect(c.x + 14.0, c.y + 44.0, c.w - 28.0, 44.0), hint, 12.0, TEXT2, Weight::Regular);
    }
    if changed {
        app.tw_on.clear();
        for t in CATALOG {
            if t.base.chars().any(|c| app.tw_layers.contains(&c)) {
                app.tw_on.insert(t.id);
            }
        }
    }

    let foot = 72.0;
    let room = r.h - 108.0 - foot - 12.0;
    let list_h = if app.cfg.tw_log { (room - 12.0) / 2.0 } else { room };
    let l = rect(r.x, r.y + 108.0, r.w, list_h);
    ui.card(l);
    let rows: Vec<usize> = (0..CATALOG.len())
        .filter(|i| {
            let t = &CATALOG[*i];
            t.base.chars().chain(t.opt.chars()).any(|c| app.tw_layers.contains(&c)) || app.tw_journal.iter().any(|e| e.tweak == t.id)
        })
        .collect();
    let list = l.inset(8.0);
    ui.begin_scroll("twl", list, rows.len() as f32 * 54.0);
    let mut undo_id = None;
    for (row, &i) in rows.iter().enumerate() {
        let t = &CATALOG[i];
        let y = list.y + row as f32 * 54.0;
        let st = tweaks::status(t, &app.tw_journal, &app.tw_apps);
        let mut on = app.tw_on.contains(t.id);
        let can = matches!(st, Status::Off | Status::Drifted);
        let r = rect(list.x, y, list.w, 52.0);
        if on && can {
            ui.c.round(r, 8.0, CARD2);
        }
        if can {
            if ui.check(&format!("tw{}", t.id), rect(r.x + 8.0, y, 30.0, 52.0), "", &mut on) {
                if on {
                    app.tw_on.insert(t.id);
                } else {
                    app.tw_on.remove(t.id);
                }
            }
        } else {
            ui.c.icon(r.x + 10.0, y + 17.0, 18.0, &[&[(6., 12.5), (10., 16.5), (18., 7.5)]], ACCENT);
        }
        let tw = r.w - 300.0;
        ui.c.text(rect(r.x + 44.0, y + 6.0, tw, 20.0), t.title, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(r.x + 44.0, y + 27.0, tw, 18.0), t.note, 12.0, MUTED, Align::Left, Weight::Regular);
        let (tag, c) = match st {
            Status::On => ("Aplicado", ACCENT),
            Status::Already => ("Já estava assim", TEXT2),
            Status::Drifted => ("O Windows desfez", WARN),
            Status::Off => match t.class {
                Class::R3 => ("Volta só pela Loja", WARN),
                Class::R2 => ("Pede reinício", TEXT2),
                Class::R1 if t.subjective => ("Questão de gosto", MUTED),
                Class::R1 => ("", TEXT2),
            },
        };
        ui.c.text(rect(r.right() - 250.0, y, 148.0, 52.0), tag, 12.0, c, Align::Right, Weight::Regular);
        if matches!(st, Status::On | Status::Drifted) && ui.button(&format!("un{}", t.id), rect(r.right() - 92.0, y + 10.0, 84.0, 32.0), "Desfazer", false) {
            undo_id = Some(t.id);
        }
    }
    ui.end_scroll();
    if let Some(id) = undo_id {
        app.tw_undo(id);
    }

    if app.cfg.tw_log {
        let b = rect(r.x, l.bottom() + 12.0, r.w, list_h);
        ui.card(b);
        if run.lines.is_empty() {
            ui.c.text(rect(b.x + 12.0, b.y + 12.0, b.w - 24.0, 22.0), "O passo a passo aparece aqui ao aplicar ou desfazer.", 13.0, MUTED, Align::Left, Weight::Regular);
        } else {
            log_view(ui, "tw-log", b.inset(12.0), &run.lines);
        }
    }

    let f = rect(r.x, r.bottom() - foot, r.w, foot);
    ui.card_on(f);
    let sel: Vec<&'static str> = CATALOG
        .iter()
        .filter(|t| app.tw_on.contains(t.id) && matches!(tweaks::status(t, &app.tw_journal, &app.tw_apps), Status::Off | Status::Drifted))
        .map(|t| t.id)
        .collect();
    let (b, name) = audit::build();
    let mut msg = format!("Não testado nesta build ({b}, {name}).");
    if run.running {
        msg = format!("{} de {}: {}", run.done, run.total, run.current);
    } else if run.total > 0 {
        let ok = run.checks.iter().all(|c| c.0);
        let rp = match run.restore {
            Some(true) => "Ponto de restauração criado. ",
            Some(false) => "O Windows recusou o ponto de restauração. ",
            None => "",
        };
        msg = format!("{rp}{}", if !run.failed.is_empty() {
            format!("Não aplicados: {}", run.failed.join(", "))
        } else if ok {
            "Aplicado. Windows Update, Loja e Central de Segurança conferidos.".to_string()
        } else {
            "Aplicado, mas uma conferência falhou. Veja o que mudou.".to_string()
        });
    }
    ui.c.text_wrap(rect(f.x + 16.0, f.y + 12.0, f.w - 590.0, 52.0), &msg, 13.0, TEXT2, Weight::Regular);
    let mut show = app.cfg.tw_log;
    if ui.check("tw-log", rect(f.right() - 566.0, f.y + 14.0, 96.0, 44.0), "Ver log", &mut show) {
        app.cfg.tw_log = show;
        app.cfg.save();
    }
    let mut rp = app.cfg.un_restore;
    if ui.check("tw-rp", rect(f.right() - 470.0, f.y + 14.0, 130.0, 44.0), "Restauração", &mut rp) {
        app.cfg.un_restore = rp;
        app.cfg.save();
    }
    if ui.button_ex("tw-all", rect(f.right() - 330.0, f.y + 14.0, 120.0, 44.0), "Desfazer tudo", false, !app.tw_journal.is_empty()) {
        ui.ask("tw-all", "Desfazer tudo?", "Todos os ajustes aplicados pelo Prisma voltam ao valor anterior, menos o que foi mudado depois.");
    }
    if ui.answered("tw-all") {
        app.tw_undo("");
    }
    let label = format!("Aplicar {}", sel.len());
    if ui.button_ex("tw-go", rect(f.right() - 202.0, f.y + 14.0, 186.0, 44.0), &label, true, !sel.is_empty() && app.elevated && !run.running) {
        ui.ask("tw-go", "Aplicar estes ajustes?", &format!("{} ajustes vão mudar o Windows. Tudo pode ser desfeito depois.", sel.len()));
    }
    if ui.answered("tw-go") {
        app.log(&format!("aplicou ajustes: {}", sel.join(" ")));
        tweaks::start(sel, app.cfg.un_restore, app.tw_run.clone());
    }
}

fn draw_left_out(ui: &mut Ui, r: Rect) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "O que outros otimizadores fazem e o Prisma não");
    ui.label(rect(r.x + 16.0, r.y + 34.0, r.w - 32.0, 18.0), "Cada item foi pesquisado. Menos ajustes aqui é uma decisão, não uma falta.", MUTED);
    let list = rect(r.x + 8.0, r.y + 60.0, r.w - 16.0, r.h - 68.0);
    ui.begin_scroll("out", list, LEFT_OUT.len() as f32 * 54.0);
    for (i, (name, why, grade)) in LEFT_OUT.iter().enumerate() {
        let y = list.y + i as f32 * 54.0;
        ui.c.text(rect(list.x + 10.0, y + 6.0, list.w - 130.0, 20.0), name, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(list.x + 10.0, y + 27.0, list.w - 20.0, 18.0), why, 12.0, MUTED, Align::Left, Weight::Regular);
        ui.c.text(rect(list.right() - 110.0, y + 6.0, 100.0, 20.0), grade, 12.0, TEXT2, Align::Right, Weight::Regular);
    }
    ui.end_scroll();
}
