//! Desinstalador: escolher, remover em lote e limpar o que sobrou.

use super::*;
use crate::config::Defaults;
use crate::sys::uninstall::{self, LeftKind, Stage};

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let job = app.un_job.lock().unwrap().clone();
    if job.running {
        draw_running(ui, a, &job);
    } else if app.un_review {
        draw_review(ui, app, a, &job);
    } else {
        draw_list(ui, app, a);
    }
}

fn fmt_date(d: &str) -> String {
    if d.len() == 8 {
        format!("{}/{}/{}", &d[6..8], &d[4..6], &d[..4])
    } else {
        String::new()
    }
}

fn draw_list(ui: &mut Ui, app: &mut App, a: Rect) {
    let sub = format!("{} programas instalados.", app.programs.len());
    header(ui, a, "Desinstalar", &sub);
    defaults_button(ui, app, a.right() - 124.0, Defaults::Uninstall, "Volta as opções de ponto de restauração e de desinstalar sem janela ao padrão.");
    let x = a.x + 28.0;
    let top = 88.0;
    let foot = 76.0;
    let h = a.h - top - 20.0;
    let l = rect(x, top, a.w - 56.0, h - foot - 12.0);
    ui.card(l);
    let mut q = std::mem::take(&mut app.un_search);
    ui.search("un-q", rect(l.x + 12.0, l.y + 10.0, 320.0, 36.0), &mut q, "Buscar por nome ou empresa");
    app.un_search = q;
    ui.c.text(rect(l.right() - 360.0, l.y + 10.0, 92.0, 36.0), "Ordenar por", 13.0, MUTED, Align::Right, Weight::Regular);
    // Na mesma ordem das colunas da lista.
    for (i, (name, by)) in [("Nome", 0u8), ("Data", 2), ("Tamanho", 1)].iter().enumerate() {
        let b = rect(l.right() - 262.0 + i as f32 * 84.0, l.y + 10.0, 78.0, 36.0);
        if ui.button(&format!("us{i}"), b, name, app.un_sort == *by) {
            app.sort_programs(*by);
        }
    }
    let needle = app.un_search.to_lowercase();
    let rows: Vec<usize> = app
        .programs
        .iter()
        .enumerate()
        .filter(|(_, p)| needle.is_empty() || p.name.to_lowercase().contains(&needle) || p.publisher.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect();
    let list = rect(l.x + 8.0, l.y + 54.0, l.w - 16.0, l.h - 62.0);
    ui.begin_scroll("un", list, rows.len() as f32 * 46.0);
    let first = (ui.scroll_y() / 46.0) as usize;
    let last = first + (list.h / 46.0) as usize + 2;
    for (row, &i) in rows.iter().enumerate().take(last).skip(first) {
        let y = list.y + row as f32 * 46.0;
        let p = app.programs[i].clone();
        let r = rect(list.x, y, list.w, 44.0);
        if p.on {
            ui.c.round(r, 8.0, CARD2);
        }
        let mut on = p.on;
        if ui.check(&format!("u{i}"), rect(r.x + 8.0, y, r.w - 16.0, 44.0), "", &mut on) {
            app.programs[i].on = on;
        }
        let path = if p.icon.is_empty() { p.name.clone() } else { p.icon.clone() };
        ui.file_icon(&path, &p.name, rect(r.x + 38.0, y + 10.0, 24.0, 24.0));
        let tw = r.w - 340.0;
        ui.c.text(rect(r.x + 72.0, y + 3.0, tw, 20.0), &p.name, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(r.x + 72.0, y + 23.0, tw, 18.0), &p.publisher, 12.0, MUTED, Align::Left, Weight::Regular);
        let size = if p.size > 0 { fmt_mem(p.size) } else { String::new() };
        ui.c.text(rect(r.right() - 260.0, y, 80.0, 44.0), &p.version.chars().take(12).collect::<String>(), 12.0, MUTED, Align::Right, Weight::Regular);
        ui.c.text(rect(r.right() - 170.0, y, 74.0, 44.0), &fmt_date(&p.date), 12.0, MUTED, Align::Right, Weight::Regular);
        ui.c.text(rect(r.right() - 90.0, y, 82.0, 44.0), &size, 13.0, TEXT, Align::Right, Weight::Regular);
    }
    ui.end_scroll();

    let f = rect(x, l.bottom() + 12.0, a.w - 56.0, foot);
    ui.card_on(f);
    let n = app.programs.iter().filter(|p| p.on).count();
    let mut c = app.cfg.un_restore;
    if ui.check("un-rp", rect(f.x + 16.0, f.y + 16.0, 250.0, 44.0), "Criar ponto de restauração", &mut c) {
        app.cfg.un_restore = c;
        app.cfg.save();
    }
    let mut c = app.cfg.un_silent;
    if ui.check("un-si", rect(f.x + 280.0, f.y + 16.0, 220.0, 44.0), "Sem janela quando der", &mut c) {
        app.cfg.un_silent = c;
        app.cfg.save();
    }
    let label = if n == 1 { "Desinstalar 1".to_string() } else { format!("Desinstalar {n}") };
    let ok = n > 0 && app.elevated;
    if ui.button_ex("un-go", rect(f.right() - 216.0, f.y + 16.0, 200.0, 44.0), &label, true, ok) {
        let names: Vec<&str> = app.programs.iter().filter(|p| p.on).map(|p| p.name.as_str()).collect();
        let mut body = names.iter().take(6).copied().collect::<Vec<_>>().join(", ");
        if names.len() > 6 {
            body = format!("{body} e mais {}", names.len() - 6);
        }
        ui.ask("un-go", "Desinstalar estes programas?", &body);
    }
    if ui.answered("un-go") {
        app.un_review = true;
        let chosen: Vec<_> = app.programs.iter().filter(|p| p.on).cloned().collect();
        for p in &chosen {
            app.log(&format!("desinstalou {} {}", p.name, p.version));
        }
        uninstall::start(chosen, app.cfg.un_silent, app.cfg.un_restore, app.un_job.clone());
    }
    if !app.elevated {
        ui.c.text(rect(f.right() - 460.0, f.y + 16.0, 236.0, 44.0), "Precisa do Prisma como administrador", 12.0, WARN, Align::Right, Weight::Regular);
    }
}

fn draw_running(ui: &mut Ui, a: Rect, job: &uninstall::Job) {
    let title = if job.stage == Stage::Scanning { "Procurando restos" } else { "Desinstalando" };
    header(ui, a, title, "Se o programa abrir a própria janela, siga por ela.");
    let r = rect(a.x + 28.0, 88.0, a.w - 56.0, a.h - 108.0);
    ui.card(r);
    let lines = job.lines();
    let list = r.inset(16.0);
    ui.stick_bottom("un-run");
    ui.begin_scroll("un-run", list, lines.len() as f32 * 28.0);
    for (i, (text, color, strong)) in lines.iter().enumerate() {
        let c = match color {
            1 => ACCENT,
            2 => WARN,
            3 => MUTED,
            _ => TEXT2,
        };
        let w = if *strong { Weight::Semibold } else { Weight::Regular };
        ui.c.text(rect(list.x, list.y + i as f32 * 28.0, list.w - 16.0, 24.0), text, 14.0, c, Align::Left, w);
    }
    ui.end_scroll();
}

fn draw_review(ui: &mut Ui, app: &mut App, a: Rect, job: &uninstall::Job) {
    if app.un_left.is_none() {
        app.un_left = Some(job.leftovers.clone());
    }
    let left = app.un_left.as_mut().unwrap();
    let n = left.iter().filter(|l| l.on).count();
    let stuck: Vec<&str> = job.done.iter().filter(|d| !d.1).map(|d| d.0.as_str()).collect();
    let sub = if !stuck.is_empty() {
        format!("Não foi removido: {}", stuck.join(", "))
    } else if left.is_empty() {
        "Nada sobrou.".to_string()
    } else {
        format!("{} restos encontrados. Só vêm marcados os que têm o nome exato; confira os outros.", left.len())
    };
    header(ui, a, "O que sobrou", &sub);
    let x = a.x + 28.0;
    let top = 88.0;
    let foot = 76.0;
    let h = a.h - top - 20.0;
    let l = rect(x, top, a.w - 56.0, h - foot - 12.0);
    ui.card(l);
    let list = l.inset(8.0);
    ui.begin_scroll("left", list, left.len() as f32 * 46.0);
    for (i, it) in left.iter_mut().enumerate() {
        let y = list.y + i as f32 * 46.0;
        let r = rect(list.x, y, list.w, 44.0);
        if it.on {
            ui.c.round(r, 8.0, CARD2);
        }
        ui.check(&format!("l{i}"), rect(r.x + 8.0, y, 30.0, 44.0), "", &mut it.on);
        let kind = match it.kind {
            LeftKind::Folder => "Pasta",
            LeftKind::File => "Arquivo",
            LeftKind::Registry(..) => "Registro",
        };
        let sub = if it.size > 0 { format!("{kind} · {} · {}", fmt_mem(it.size), it.program) } else { format!("{kind} · {}", it.program) };
        ui.c.text(rect(r.x + 44.0, y + 3.0, r.w - 56.0, 20.0), &it.label, 13.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(r.x + 44.0, y + 23.0, r.w - 56.0, 18.0), &sub, 12.0, MUTED, Align::Left, Weight::Regular);
    }
    ui.end_scroll();

    let f = rect(x, l.bottom() + 12.0, a.w - 56.0, foot);
    ui.card_on(f);
    let note = "Pastas vão para a Lixeira. O registro ganha um backup antes.";
    ui.c.text(rect(f.x + 16.0, f.y + 16.0, f.w - 460.0, 44.0), note, 13.0, TEXT2, Align::Left, Weight::Regular);
    let keep = if left.is_empty() { "Fechar" } else { "Manter tudo" };
    let close = ui.button("left-no", rect(f.right() - 400.0, f.y + 16.0, 160.0, 44.0), keep, false);
    if ui.button_ex("left-go", rect(f.right() - 232.0, f.y + 16.0, 216.0, 44.0), &format!("Limpar {n} selecionados"), true, n > 0) {
        ui.ask("left-go", "Limpar os restos selecionados?", "Pastas e arquivos vão para a Lixeira. As chaves do registro são apagadas depois de um backup.");
    }
    let clean = ui.answered("left-go");
    if clean {
        let back = crate::config::data_dir().join("backups");
        let items = app.un_left.clone().unwrap_or_default();
        let (mut removed, mut failed) = (0, Vec::new());
        for it in items.iter().filter(|l| l.on) {
            if uninstall::remove(it, &back) {
                app.log(&format!("limpou {}", it.label));
                removed += 1;
            } else {
                failed.push(it.label.clone());
            }
        }
        if failed.is_empty() {
            ui.notice("Restos excluídos", &format!("{removed} itens foram excluídos com sucesso."));
        } else {
            let list = failed.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
            ui.notice("Alguns restos não saíram", &format!("{removed} itens excluídos, {} não saíram: {list}", failed.len()));
        }
    }
    if clean || close {
        app.un_left = None;
        app.un_review = false;
        app.programs = uninstall::list();
        app.sort_programs(app.un_sort);
    }
}
