use std::collections::BTreeMap;

use super::*;
use crate::config::{MOD_ALT, MOD_CTRL, MOD_SHIFT};

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Volume por programa", "As teclas de volume do teclado, com o modificador, mudam só o programa em foco.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let cw = (w - 12.0) / 2.0;
    let r0 = rect(x, top, cw, 132.0);
    ui.card_on(r0);
    ui.label(rect(r0.x + 16.0, r0.y + 10.0, r0.w - 32.0, 18.0), "Modificador (pode combinar dois)", TEXT2);
    let keys = [("Ctrl", MOD_CTRL), ("Shift", MOD_SHIFT), ("Alt", MOD_ALT)];
    let bw = (r0.w - 48.0) / 3.0;
    let mut mods = app.cfg.vol_mods;
    for (i, (k, m)) in keys.iter().enumerate() {
        let on = mods & m != 0;
        let b = rect(r0.x + 16.0 + i as f32 * (bw + 8.0), r0.y + 34.0, bw, 44.0);
        if ui.button(k, b, k, on) {
            let count = mods.count_ones();
            if on && count > 1 {
                mods &= !m;
            } else if !on && count < 2 {
                mods |= m;
            }
        }
    }
    if mods != app.cfg.vol_mods {
        app.cfg.vol_mods = mods;
        app.cfg.save();
        app.register_hotkeys();
    }
    let names: Vec<&str> = keys.iter().filter(|k| mods & k.1 != 0).map(|k| k.0).collect();
    let now = format!("Agora: {} + tecla de volume ou mudo", names.join(" + "));
    ui.label(rect(r0.x + 16.0, r0.y + 90.0, r0.w - 32.0, 20.0), &now, TEXT2);

    let r1 = rect(x + cw + 12.0, top, cw, 132.0);
    ui.card(r1);
    ui.label(rect(r1.x + 16.0, r1.y + 10.0, r1.w - 32.0, 18.0), "Passo", TEXT2);
    let mut osd = app.cfg.vol_osd;
    if ui.check("osd", rect(r1.x + 16.0, r1.y + 84.0, r1.w - 32.0, 40.0), "Mostrar aviso na tela", &mut osd) {
        app.cfg.vol_osd = osd;
        app.cfg.save();
    }

    let t = rect(x, top + 144.0, w, a.h - top - 164.0);
    ui.card(t);
    ui.title(rect(t.x + 16.0, t.y + 12.0, 300.0, 22.0), "Programas com som");

    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, s) in app.sessions.iter().enumerate() {
        groups.entry(s.exe.to_ascii_lowercase()).or_default().push(i);
    }
    let list = rect(t.x + 8.0, t.y + 42.0, t.w - 16.0, t.h - 50.0);
    ui.begin_scroll("snd", list, groups.len() as f32 * 54.0);
    for (i, (_, idx)) in groups.iter().enumerate() {
        let y = list.y + i as f32 * 54.0;
        let first = &app.sessions[idx[0]];
        let focused = first.exe.eq_ignore_ascii_case(&app.fg_exe);
        if focused {
            ui.c.round(rect(list.x, y, list.w, 52.0), 8.0, CARD2);
        }
        let name = first.name.clone();
        let muted = idx.iter().all(|i| app.sessions[*i].muted);
        let active = idx.iter().any(|i| app.sessions[*i].active);
        let state = if focused {
            "Em foco"
        } else if muted {
            "Silenciado"
        } else if active {
            "Tocando"
        } else {
            "Parado"
        };
        ui.c.text(rect(list.x + 16.0, y + 6.0, 240.0, 20.0), &name, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(list.x + 16.0, y + 27.0, 240.0, 18.0), state, 12.0, MUTED, Align::Left, Weight::Regular);
        let sx = list.x + 270.0;
        let sw = list.w - 270.0 - 124.0;
        let mut v = first.volume;
        if ui.slider(&format!("s{name}"), rect(sx, y + 14.0, sw, 24.0), &mut v) {
            for j in idx {
                app.sessions[*j].set_volume(v);
                app.sessions[*j].volume = v;
            }
        }
        let t = format!("{:.0}%", v * 100.0);
        ui.c.text(rect(sx + sw + 8.0, y, 48.0, 52.0), &t, 14.0, TEXT, Align::Right, Weight::Regular);
        let b = rect(list.right() - 52.0, y + 4.0, 44.0, 44.0);
        ui.c.round(b, 8.0, if muted { WARN } else { CARD });
        ui.c.round_line(b, 8.0, BORDER, 1.0);
        let ic = if muted { rgb(0x1f1508) } else { TEXT };
        ui.c.icon(b.x + 12.0, b.y + 12.0, 20.0, &[&[(4., 9.), (4., 15.), (8., 15.), (13., 19.), (13., 5.), (8., 9.), (4., 9.)], &[(17., 9.5), (21., 14.5)], &[(21., 9.5), (17., 14.5)]], ic);
        if ui.hit(&format!("m{name}"), b) {
            for j in idx {
                app.sessions[*j].set_mute(!muted);
                app.sessions[*j].muted = !muted;
            }
        }
    }
    ui.end_scroll();
    if groups.is_empty() {
        ui.c.text(rect(t.x + 24.0, t.y + 48.0, t.w - 48.0, 24.0), "Nenhum programa com som agora.", 14.0, MUTED, Align::Left, Weight::Regular);
    }

    let opts = ["2% por toque".to_string(), "5% por toque".to_string(), "10% por toque".to_string()];
    let vals = [2u32, 5, 10];
    let mut sel = vals.iter().position(|v| *v == app.cfg.vol_step).unwrap_or(1);
    if ui.select("step", rect(r1.x + 16.0, r1.y + 34.0, r1.w - 32.0, 44.0), &opts, &mut sel) {
        app.cfg.vol_step = vals[sel];
        app.cfg.save();
    }
}
