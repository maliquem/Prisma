//! Serviços do Windows.

use super::*;
use crate::sys::services::{self, Action};

fn run_action(app: &mut App, name: &str, a: Action) {
    if !app.elevated {
        app.toast.info("Precisa do Prisma como administrador");
        return;
    }
    services::control(name.to_string(), a);
    app.log(&format!("serviço {name}: {}", match a {
        Action::Start => "iniciar",
        Action::Stop => "parar",
        Action::Restart => "reiniciar",
    }));
}

fn ask_action(ui: &mut Ui, display: &str, stop: bool) {
    if stop {
        ui.ask("svc-stop", "Parar este serviço?", &format!("{display} vai parar agora. Programas que dependem dele podem deixar de funcionar."));
    } else {
        ui.ask("svc-restart", "Reiniciar este serviço?", &format!("{display} vai parar e iniciar de novo."));
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let running = app.services.iter().filter(|s| s.running).count();
    let sub = format!("{running} rodando de {}.", app.services.len());
    header(ui, a, "Serviços", &sub);
    if ui.button("svc-native", rect(a.right() - 216.0, 24.0, 188.0, 44.0), "Editar no Windows", false) {
        services::open_native();
    }

    let x = a.x + 28.0;
    let top = 88.0;
    let h = a.h - top - 20.0;
    let rw = 300.0;
    let l = rect(x, top, a.w - 56.0 - rw - 12.0, h);
    ui.card(l);
    let bw = 92.0;
    for (i, name) in ["Todos", "Rodando", "Parados"].iter().enumerate() {
        if ui.button(name, rect(l.x + 12.0 + i as f32 * (bw + 6.0), l.y + 10.0, bw, 36.0), name, app.svc_filter == i as u8) {
            app.svc_filter = i as u8;
        }
    }
    let sx = l.x + 12.0 + 3.0 * (bw + 6.0) + 6.0;
    let mut q = std::mem::take(&mut app.svc_search);
    ui.search("svc-q", rect(sx, l.y + 10.0, l.right() - sx - 12.0, 36.0), &mut q, "Buscar");
    app.svc_search = q;

    let needle = app.svc_search.to_lowercase();
    let filter = app.svc_filter;
    let rows: Vec<usize> = app
        .services
        .iter()
        .enumerate()
        .filter(|(_, s)| match filter {
            1 => s.running,
            2 => !s.running,
            _ => true,
        })
        .filter(|(_, s)| needle.is_empty() || s.display.to_lowercase().contains(&needle) || s.name.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect();

    let list = rect(l.x + 8.0, l.y + 54.0, l.w - 16.0, l.h - 62.0);
    ui.begin_scroll("svc", list, rows.len() as f32 * 34.0);
    let mut pick = None;
    let mut right = None;
    let first = (ui.scroll_y() / 34.0) as usize;
    let last = first + (list.h / 34.0) as usize + 2;
    for (row, &i) in rows.iter().enumerate().take(last).skip(first) {
        let y = list.y + row as f32 * 34.0;
        let s = &app.services[i];
        let r = rect(list.x, y, list.w, 34.0);
        if app.svc_sel.as_deref() == Some(s.name.as_str()) {
            ui.c.round(r, 6.0, CARD2);
        }
        if ui.hit(&format!("s{i}"), r) {
            pick = Some(s.name.clone());
        }
        if ui.right_hit(r) {
            pick = Some(s.name.clone());
            right = Some((s.name.clone(), s.display.clone(), s.running));
        }
        ui.c.circle(r.x + 16.0, y + 17.0, 4.0, if s.running { ACCENT } else { BORDER2 });
        ui.c.text(rect(r.x + 30.0, y, r.w - 130.0, 34.0), &s.display, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(r.right() - 100.0, y, 92.0, 34.0), s.start_name(), 12.0, MUTED, Align::Right, Weight::Regular);
    }
    ui.end_scroll();
    if let Some(n) = pick {
        app.select_service(n);
    }
    if let Some((n, d, on)) = right {
        app.svc_target = Some((n, d));
        ui.menu("svc-menu", &[("Iniciar", !on), ("Parar", on), ("Reiniciar", on), ("Propriedades", true)]);
    }
    if let Some(i) = ui.menu_pick("svc-menu") {
        if let Some((n, d)) = app.svc_target.clone() {
            match i {
                0 => run_action(app, &n, Action::Start),
                1 => ask_action(ui, &d, true),
                2 => ask_action(ui, &d, false),
                _ if app.elevated => {
                    app.toast.info("Abrindo as propriedades no Windows");
                    services::show_properties(d);
                }
                _ => app.toast.info("Precisa do Prisma como administrador"),
            }
        }
    }
    if ui.answered("svc-stop") {
        if let Some((n, _)) = app.svc_target.clone() {
            run_action(app, &n, Action::Stop);
        }
    }
    if ui.answered("svc-restart") {
        if let Some((n, _)) = app.svc_target.clone() {
            run_action(app, &n, Action::Restart);
        }
    }

    let r = rect(l.right() + 12.0, top, rw, h);
    ui.card(r);
    let Some(name) = app.svc_sel.clone() else {
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 16.0, r.w - 32.0, 60.0), "Escolha um serviço para ver o que ele faz.", 14.0, MUTED, Weight::Regular);
        return;
    };
    let Some(s) = app.services.iter().find(|s| s.name == name).cloned() else { return };
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 44.0), &s.display, 15.0, TEXT, Weight::Semibold);
    let state = if s.running { format!("Rodando · PID {}", s.pid) } else { "Parado".to_string() };
    ui.c.text(rect(r.x + 16.0, r.y + 58.0, r.w - 32.0, 20.0), &format!("{state} · {}", s.start_name()), 13.0, ACCENT, Align::Left, Weight::Regular);
    let (desc, bin) = app.svc_info.clone();
    let text = if desc.is_empty() { "O serviço não traz descrição.".to_string() } else { desc };
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 86.0, r.w - 32.0, 190.0), &text, 13.0, TEXT2, Weight::Regular);
    ui.label(rect(r.x + 16.0, r.y + 284.0, r.w - 32.0, 18.0), "Executável", MUTED);
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 304.0, r.w - 32.0, 70.0), &bin, 12.0, TEXT2, Weight::Regular);

    let bw = (r.w - 40.0) / 2.0;
    let by = r.bottom() - 108.0;
    if s.running {
        if ui.button("svc-stop-b", rect(r.x + 16.0, by, bw, 44.0), "Parar", false) {
            app.svc_target = Some((name.clone(), s.display.clone()));
            ask_action(ui, &s.display, true);
        }
    } else if ui.button("svc-start", rect(r.x + 16.0, by, bw, 44.0), "Iniciar", true) {
        run_action(app, &name, Action::Start);
    }
    if ui.button_ex("svc-re", rect(r.x + 24.0 + bw, by, bw, 44.0), "Reiniciar", false, s.running) {
        app.svc_target = Some((name.clone(), s.display.clone()));
        ask_action(ui, &s.display, false);
    }
    ui.c.text_wrap(rect(r.x + 16.0, r.bottom() - 56.0, r.w - 32.0, 44.0), "Botão direito no serviço mostra iniciar, parar, reiniciar e propriedades.", 12.0, MUTED, Weight::Regular);
}
