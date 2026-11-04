//! Discos e redes: tudo que está ligado no PC, com saúde dos discos.

use super::*;
use crate::sys::disks::{self, Health, Level};

fn level_color(l: Level) -> Color {
    match l {
        Level::Good => ACCENT,
        Level::Warn => WARN,
        Level::Risk => REC,
        Level::Unknown => MUTED,
    }
}

fn level_name(l: Level) -> &'static str {
    match l {
        Level::Good => "Bom",
        Level::Warn => "Atenção",
        Level::Risk => "Risco",
        Level::Unknown => "Sem dados",
    }
}

fn fmt_size(b: u64) -> String {
    if b >= 1_000_000_000_000 {
        format!("{:.1} TB", b as f64 / 1e12).replace('.', ",")
    } else {
        format!("{} GB", b / 1_000_000_000)
    }
}

fn fmt_hours(h: u64) -> String {
    if h >= 24 * 365 {
        format!("{h} h ({:.1} anos)", h as f64 / (24.0 * 365.0)).replace('.', ",")
    } else {
        format!("{h} h")
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Discos e redes", "Tudo que está ligado no PC, em tempo real.");
    let bw = 104.0;
    for (i, name) in ["Discos", "Redes"].iter().enumerate() {
        let b = rect(a.right() - 28.0 - (2 - i) as f32 * (bw + 8.0) + 8.0, 28.0, bw, 40.0);
        if ui.button(name, b, name, app.store_tab == i as u8) {
            app.store_tab = i as u8;
        }
    }
    if app.store_tab == 0 {
        draw_disks(ui, app, a);
    } else {
        draw_nets(ui, app, a);
    }
}

fn draw_disks(ui: &mut Ui, app: &mut App, a: Rect) {
    let x = a.x + 28.0;
    let top = 88.0;
    let h = a.h - top - 20.0;
    let rw = 320.0;
    let lw = a.w - 56.0 - rw - 12.0;
    let health = app.health.lock().unwrap().clone();

    let list = rect(x, top, lw, h);
    let row_h = 112.0;
    ui.begin_scroll("disks", list, app.disks.len() as f32 * (row_h + 10.0));
    let mut pick = None;
    for (i, d) in app.disks.iter().enumerate() {
        let r = rect(list.x, list.y + i as f32 * (row_h + 10.0), list.w, row_h);
        let sel = app.disk_sel == i;
        if sel {
            ui.card_on(r);
        } else {
            ui.card(r);
        }
        if ui.hit(&format!("d{i}"), r) {
            pick = Some(i);
        }
        let hl = health.get(&d.number);
        let lvl = hl.map_or(Level::Unknown, |h| h.level);
        let tx = r.x + 16.0;
        let name = if d.name.is_empty() { format!("Disco {}", d.number) } else { d.name.clone() };
        let side = 196.0;
        ui.c.text(rect(tx, r.y + 10.0, r.w - side - 32.0, 22.0), &name, 15.0, TEXT, Align::Left, Weight::Semibold);
        let letters: Vec<String> = d.volumes.iter().map(|v| format!("{}:", v.letter)).collect();
        let sub = format!("{} {} · {} · {}", d.kind(), d.bus_name(), fmt_size(d.size), letters.join(" "));
        ui.c.text(rect(tx, r.y + 33.0, r.w - side - 32.0, 18.0), &sub, 13.0, MUTED, Align::Left, Weight::Regular);

        // Espaço usado somando os volumes do disco.
        let (total, free) = d.volumes.iter().fold((0, 0), |s, v| (s.0 + v.total, s.1 + v.free));
        if total > 0 {
            let used = 1.0 - free as f32 / total as f32;
            let bar = rect(tx, r.y + 62.0, r.w - side - 32.0, 6.0);
            ui.c.round(bar, 3.0, SIDE);
            ui.c.round(rect(bar.x, bar.y, bar.w * used, bar.h), 3.0, if used > 0.95 { WARN } else { ACCENT });
            let t = format!("{} livres de {}", fmt_size(free), fmt_size(total));
            ui.c.text(rect(tx, r.y + 74.0, bar.w, 18.0), &t, 12.0, MUTED, Align::Left, Weight::Regular);
        }

        let sx = r.right() - side;
        let color = level_color(lvl);
        ui.c.circle(sx + 6.0, r.y + 21.0, 5.0, color);
        ui.c.text(rect(sx + 18.0, r.y + 10.0, 80.0, 22.0), level_name(lvl), 14.0, color, Align::Left, Weight::Semibold);
        if let Some(t) = hl.and_then(|h| h.temp) {
            ui.c.text(rect(sx + 90.0, r.y + 10.0, 90.0, 22.0), &format!("{t} °C"), 13.0, TEXT2, Align::Right, Weight::Regular);
        }
        let line = format!("Ler {} · Gravar {}", fmt_rate(d.read), fmt_rate(d.write));
        ui.c.text(rect(sx, r.y + 34.0, side - 16.0, 18.0), &line, 12.0, TEXT2, Align::Left, Weight::Regular);
        ui.c.text(rect(sx, r.y + 52.0, side - 16.0, 18.0), &format!("Ativo {:.0}%", d.active), 12.0, MUTED, Align::Left, Weight::Regular);
        ui.c.sparkline(rect(sx, r.y + 74.0, side - 16.0, 26.0), &d.hist, ACCENT, 1.5);
    }
    ui.end_scroll();
    if let Some(i) = pick {
        app.disk_sel = i;
    }

    let r = rect(list.right() + 12.0, top, rw, h);
    ui.card(r);
    let Some(d) = app.disks.get(app.disk_sel) else { return };
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Saúde");
    let empty = Health::default();
    let hl = health.get(&d.number);
    let lvl = hl.map_or(Level::Unknown, |h| h.level);
    let v = rect(r.x + 16.0, r.y + 40.0, r.w - 32.0, 40.0);
    ui.c.text(v, level_name(lvl), 26.0, level_color(lvl), Align::Left, Weight::Semibold);
    let summary = hl.map_or("Lendo o disco…", |h| h.summary.as_str());
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 84.0, r.w - 32.0, 44.0), summary, 13.0, TEXT2, Weight::Regular);
    let h0 = hl.unwrap_or(&empty);
    let mut rows: Vec<(String, String)> = Vec::new();
    if let Some(t) = h0.temp {
        rows.push(("Temperatura".into(), format!("{t} °C")));
    }
    if let Some(v) = h0.hours {
        rows.push(("Tempo ligado".into(), fmt_hours(v)));
    }
    rows.extend(h0.details.iter().cloned());
    rows.push(("Número de série".into(), d.serial.clone()));
    let mut y = r.y + 132.0;
    for (k, v) in &rows {
        ui.c.line(r.x + 16.0, y, r.right() - 16.0, y, LINE, 1.0);
        ui.label(rect(r.x + 16.0, y + 5.0, 150.0, 20.0), k, MUTED);
        ui.c.text(rect(r.x + 130.0, y + 5.0, r.w - 146.0, 20.0), v, 13.0, TEXT, Align::Right, Weight::Regular);
        y += 30.0;
        if y + 30.0 > r.bottom() - 52.0 {
            break;
        }
    }
    if ui.button("hl", rect(r.x + 16.0, r.bottom() - 52.0, r.w - 32.0, 40.0), "Ler saúde de novo", false) {
        app.health.lock().unwrap().clear();
        disks::spawn_health(&app.disks, app.health.clone());
    }
}

fn draw_nets(ui: &mut Ui, app: &mut App, a: Rect) {
    let x = a.x + 28.0;
    let top = 88.0;
    let list = rect(x, top, a.w - 56.0, a.h - top - 20.0);
    let row_h = 104.0;
    ui.begin_scroll("nets", list, app.nets.len() as f32 * (row_h + 10.0));
    for (i, n) in app.nets.iter().enumerate() {
        let r = rect(list.x, list.y + i as f32 * (row_h + 10.0), list.w, row_h);
        if n.up {
            ui.card_on(r);
        } else {
            ui.card(r);
        }
        let tx = r.x + 16.0;
        let tw = r.w * 0.42;
        ui.c.text(rect(tx, r.y + 10.0, tw, 22.0), &n.name, 15.0, TEXT, Align::Left, Weight::Semibold);
        ui.c.text(rect(tx, r.y + 33.0, tw, 18.0), &n.desc, 13.0, MUTED, Align::Left, Weight::Regular);
        let state = if n.up {
            let speed = if n.speed >= 1_000_000_000 {
                format!("{} Gbps", n.speed / 1_000_000_000)
            } else {
                format!("{} Mbps", n.speed / 1_000_000)
            };
            if n.ip.is_empty() { format!("Conectado · {speed}") } else { format!("Conectado · {speed} · {}", n.ip) }
        } else {
            "Desconectado".to_string()
        };
        let color = if n.up { ACCENT } else { MUTED };
        ui.c.text(rect(tx, r.y + 56.0, tw, 18.0), &state, 13.0, color, Align::Left, Weight::Regular);
        let sx = r.x + tw + 32.0;
        let sw = 190.0;
        ui.c.text(rect(sx, r.y + 12.0, sw, 20.0), &format!("Baixando {}", fmt_rate(n.down_rate)), 13.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(sx, r.y + 34.0, sw, 20.0), &format!("Enviando {}", fmt_rate(n.up_rate)), 13.0, TEXT, Align::Left, Weight::Regular);
        let t = format!("Total: {} recebidos, {} enviados", fmt_mem(n.total_down), fmt_mem(n.total_up));
        ui.c.text(rect(sx, r.y + 58.0, r.right() - sx - 16.0, 18.0), &t, 12.0, MUTED, Align::Left, Weight::Regular);
        let g = rect(sx + sw + 8.0, r.y + 12.0, r.right() - sx - sw - 24.0, 40.0);
        ui.c.sparkline(g, &n.hist, ACCENT, 1.5);
    }
    ui.end_scroll();
}
