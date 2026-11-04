use super::*;
use crate::optimizer::Suggestion;

fn list_card(ui: &mut Ui, r: Rect, title: &str, sub: &str, key: &str, items: &mut Vec<String>) -> bool {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 10.0, r.w - 32.0, 22.0), title);
    ui.label(rect(r.x + 16.0, r.y + 32.0, r.w - 32.0, 18.0), sub, MUTED);
    let list = rect(r.x + 8.0, r.y + 56.0, r.w - 16.0, r.h - 64.0);
    ui.begin_scroll(key, list, items.len() as f32 * 32.0);
    let mut remove = None;
    for (i, v) in items.iter().enumerate() {
        let y = list.y + i as f32 * 32.0;
        ui.c.line(list.x + 8.0, y, list.right() - 8.0, y, LINE, 1.0);
        ui.c.text(rect(list.x + 8.0, y, list.w - 44.0, 32.0), app_name(v), 14.0, TEXT, Align::Left, Weight::Regular);
        let b = rect(list.right() - 34.0, y + 4.0, 24.0, 24.0);
        if ui.is_hover(b) {
            ui.c.round(b, 6.0, BORDER);
        }
        ui.c.icon(b.x + 4.0, b.y + 4.0, 16.0, &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]], MUTED);
        if ui.hit(&format!("{key}-x-{v}"), b) {
            remove = Some(i);
        }
    }
    ui.end_scroll();
    if let Some(i) = remove {
        items.remove(i);
        return true;
    }
    false
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Otimizar", "Nada é fechado sem você confirmar.");
    defaults_button(ui, app, a.right() - 124.0, crate::config::Defaults::Optimize, "Esvazia as listas de nunca fechar e sempre fechar, e as sugestões recusadas. Essa ação não fecha nenhum programa.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let rw = 300.0;
    let lw = w - rw - 12.0;
    let h = a.h - top - 20.0;
    let l = rect(x, top, lw, h);
    ui.card(l);
    ui.title(rect(l.x + 16.0, l.y + 12.0, 200.0, 22.0), "Pode fechar");
    let sel: Vec<_> = app.cands.iter().filter(|c| c.on).collect();
    let n = sel.len();
    let ram: u64 = sel.iter().map(|c| c.ram).sum();
    let freed = format!("Libera {} de RAM", fmt_mem(ram));
    ui.c.text(rect(l.x + 200.0, l.y + 12.0, l.w - 216.0, 22.0), &freed, 14.0, MUTED, Align::Right, Weight::Regular);

    let footer = 120.0 + if app.stubborn.is_empty() { 0.0 } else { 56.0 };
    let list = rect(l.x + 8.0, l.y + 42.0, l.w - 16.0, l.h - 42.0 - footer);
    ui.begin_scroll("cands", list, app.cands.len() as f32 * 56.0);
    let mut to_black = None;
    let mut to_white = None;
    for (i, c) in app.cands.iter_mut().enumerate() {
        let y = list.y + i as f32 * 56.0;
        let row = rect(list.x, y, list.w, 54.0);
        if c.on {
            ui.c.round(row, 8.0, CARD2);
        }
        let mut on = c.on;
        let chk = rect(row.x + 8.0, y, 30.0, 54.0);
        if ui.check(&format!("c-{}", c.exe), chk, "", &mut on) {
            c.on = on;
            if on {
                app.unchecked.remove(&c.exe);
                app.checked.insert(c.exe.clone());
            } else {
                app.checked.remove(&c.exe);
                app.unchecked.insert(c.exe.clone());
            }
        }
        let tw = row.w - 300.0;
        ui.c.text(rect(row.x + 44.0, y + 7.0, tw, 20.0), app_name(&c.exe), 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(row.x + 44.0, y + 28.0, tw, 18.0), &c.why, 12.0, MUTED, Align::Left, Weight::Regular);
        let ramx = row.right() - 250.0;
        let mut more = fmt_pct(c.cpu) + " CPU";
        if c.vram > 0 {
            more = format!("{more} · {} VRAM", fmt_mem(c.vram));
        }
        ui.c.text(rect(ramx - 90.0, y + 7.0, 170.0, 20.0), &fmt_mem(c.ram), 13.0, TEXT, Align::Right, Weight::Regular);
        ui.c.text(rect(ramx - 90.0, y + 28.0, 170.0, 18.0), &more, 12.0, MUTED, Align::Right, Weight::Regular);
        if ui.button(&format!("b-{}", c.exe), rect(row.right() - 160.0, y + 11.0, 76.0, 32.0), "Proteger", false) {
            to_black = Some(c.exe.clone());
        }
        if !c.white && ui.button(&format!("w-{}", c.exe), rect(row.right() - 80.0, y + 11.0, 72.0, 32.0), "Sempre", false) {
            to_white = Some(c.exe.clone());
        }
    }
    ui.end_scroll();
    if let Some(e) = to_black {
        app.cfg.white.retain(|v| !v.eq_ignore_ascii_case(&e));
        app.cfg.black.push(e);
        app.cfg.save();
        app.refresh_candidates();
    }
    if let Some(e) = to_white {
        app.cfg.white.push(e);
        app.cfg.save();
        app.refresh_candidates();
    }

    let mut fy = l.bottom() - footer + 8.0;
    if !app.stubborn.is_empty() {
        let names: Vec<&str> = app.stubborn.iter().map(|s| app_name(&s.0)).collect();
        let msg = format!("Não fecharam: {}", names.join(", "));
        ui.c.text(rect(l.x + 16.0, fy, l.w - 150.0, 44.0), &msg, 13.0, WARN, Align::Left, Weight::Regular);
        if ui.button("force", rect(l.right() - 126.0, fy, 110.0, 44.0), "Forçar", false) {
            app.force_stubborn();
        }
        fy += 56.0;
    }
    let label = if n == 1 { "Fechar 1 selecionado".to_string() } else { format!("Fechar {n} selecionados") };
    if ui.button_ex("close", rect(l.x + 16.0, fy, 200.0, 44.0), &label, true, n > 0) {
        app.close_selected();
    }
    let note = "Pede para o programa fechar normalmente, então trabalho não salvo gera o aviso dele. Forçar é uma opção à parte.";
    ui.c.text_wrap(rect(l.x + 16.0, fy + 52.0, l.w - 32.0, 40.0), note, 13.0, MUTED, Weight::Regular);

    let r = rect(l.right() + 12.0, top, rw, h);
    let detect_h = 132.0;
    let lh = (h - detect_h - 24.0) / 2.0;
    let mut changed = false;
    changed |= list_card(ui, rect(r.x, top, rw, lh), "Blacklist", "Nunca fechar", "black", &mut app.cfg.black);
    changed |= list_card(ui, rect(r.x, top + lh + 12.0, rw, lh), "Whitelist", "Sempre fechar ao otimizar", "white", &mut app.cfg.white);
    if changed {
        app.cfg.save();
    }

    let d = rect(r.x, top + 2.0 * lh + 24.0, rw, detect_h);
    ui.card_on(d);
    ui.title(rect(d.x + 16.0, d.y + 10.0, d.w - 32.0, 22.0), "Detecção automática");
    let hint = app.usage.suggest(&app.cfg, &app.snap.groups);
    match hint {
        None => ui.c.text_wrap(rect(d.x + 16.0, d.y + 36.0, d.w - 32.0, 44.0), "Sem sugestões agora. O Prisma aprende com o que você usa.", 13.0, TEXT2, Weight::Regular),
        Some(s) => {
            let (msg, exe, white) = match &s {
                Suggestion::White(e, days) => (format!("{} ficou {days} dias aberto sem você usar. Colocar na whitelist?", app_name(e)), e.clone(), true),
                Suggestion::Black(e) => (format!("Você usa {} sempre. Colocar na blacklist?", app_name(e)), e.clone(), false),
            };
            ui.c.text_wrap(rect(d.x + 16.0, d.y + 34.0, d.w - 32.0, 44.0), &msg, 13.0, TEXT2, Weight::Regular);
            let bw = (d.w - 40.0) / 2.0;
            if ui.button("sug-y", rect(d.x + 16.0, d.y + 82.0, bw, 40.0), "Sim", false) {
                if white {
                    app.cfg.white.push(exe);
                } else {
                    app.cfg.black.push(exe);
                }
                app.cfg.save();
            } else if ui.button("sug-n", rect(d.x + 24.0 + bw, d.y + 82.0, bw, 40.0), "Não", false) {
                app.cfg.ignored.push(exe);
                app.cfg.save();
            }
        }
    }
}
