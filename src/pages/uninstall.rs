//! Desinstalador: escolher, remover em lote e limpar o que sobrou.

use super::*;
use crate::sys::uninstall::{self, LeftKind};

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
    let x = a.x + 28.0;
    let top = 88.0;
    let foot = 76.0;
    let h = a.h - top - 20.0;
    let l = rect(x, top, a.w - 56.0, h - foot - 12.0);
    ui.card(l);
    let mut q = std::mem::take(&mut app.un_search);
    ui.search("un-q", rect(l.x + 12.0, l.y + 10.0, 320.0, 36.0), &mut q, "Buscar por nome ou empresa");
    app.un_search = q;
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
            app.un_confirm = false;
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
    let label = if app.un_confirm {
        format!("Confirmar: remover {n}")
    } else if n == 1 {
        "Desinstalar 1".to_string()
    } else {
        format!("Desinstalar {n}")
    };
    let ok = n > 0 && app.elevated;
    if ui.button_ex("un-go", rect(f.right() - 216.0, f.y + 16.0, 200.0, 44.0), &label, true, ok) {
        if app.un_confirm {
            app.un_confirm = false;
            app.un_review = true;
            let chosen: Vec<_> = app.programs.iter().filter(|p| p.on).cloned().collect();
            for p in &chosen {
                app.log(&format!("desinstalou {} {}", p.name, p.version));
            }
            uninstall::start(chosen, app.cfg.un_silent, app.cfg.un_restore, app.un_job.clone());
        } else {
            app.un_confirm = true;
        }
    }
    if !app.elevated {
        ui.c.text(rect(f.right() - 460.0, f.y + 16.0, 236.0, 44.0), "Precisa do Prisma como administrador", 12.0, WARN, Align::Right, Weight::Regular);
    }
}

fn draw_running(ui: &mut Ui, a: Rect, job: &uninstall::Job) {
    header(ui, a, "Desinstalando", "Se o programa abrir a própria janela, siga por ela.");
    let r = rect(a.x + 28.0, 88.0, a.w - 56.0, a.h - 108.0);
    ui.card(r);
    let mut y = r.y + 16.0;
    if let Some(ok) = job.restore {
        let t = if ok { "Ponto de restauração pedido" } else { "O Windows recusou o ponto de restauração" };
        ui.c.text(rect(r.x + 16.0, y, r.w - 32.0, 24.0), t, 14.0, if ok { TEXT } else { WARN }, Align::Left, Weight::Regular);
        y += 30.0;
    }
    for (n, ok) in &job.done {
        let t = format!("{} {n}", if *ok { "Concluído:" } else { "Não abriu:" });
        ui.c.text(rect(r.x + 16.0, y, r.w - 32.0, 24.0), &t, 14.0, if *ok { TEXT2 } else { WARN }, Align::Left, Weight::Regular);
        y += 28.0;
    }
    let cur = if job.current.is_empty() { "Procurando restos…".to_string() } else { format!("Removendo {}…", job.current) };
    ui.c.text(rect(r.x + 16.0, y, r.w - 32.0, 24.0), &cur, 14.0, ACCENT, Align::Left, Weight::Semibold);
}

fn draw_review(ui: &mut Ui, app: &mut App, a: Rect, job: &uninstall::Job) {
    if app.un_left.is_none() {
        app.un_left = Some(job.leftovers.clone());
    }
    let left = app.un_left.as_mut().unwrap();
    let n = left.iter().filter(|l| l.on).count();
    let sub = if left.is_empty() { "Nada sobrou.".to_string() } else { format!("{} restos encontrados. Confira antes de limpar.", left.len()) };
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
    let close = ui.button("left-no", rect(f.right() - 400.0, f.y + 16.0, 160.0, 44.0), "Manter tudo", false);
    let clean = ui.button_ex("left-go", rect(f.right() - 232.0, f.y + 16.0, 216.0, 44.0), &format!("Limpar {n} selecionados"), true, n > 0);
    if clean {
        let back = crate::config::data_dir().join("backups");
        let items = app.un_left.clone().unwrap_or_default();
        let mut failed = 0;
        for it in items.iter().filter(|l| l.on) {
            if uninstall::remove(it, &back) {
                app.log(&format!("limpou {}", it.label));
            } else {
                failed += 1;
            }
        }
        if failed > 0 {
            app.toast.info(&format!("{failed} itens não saíram"));
        } else {
            app.toast.info("Restos limpos");
        }
    }
    if clean || close {
        app.un_left = None;
        app.un_review = false;
        app.programs = uninstall::list();
    }
}
