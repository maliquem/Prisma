use std::collections::BTreeMap;

use super::picker::{self, Target};
use super::*;
use crate::config::{MOD_ALT, MOD_CTRL, MOD_SHIFT};
use crate::sys::audio;

struct Row {
    exe: String,
    name: String,
    path: String,
    idx: Vec<usize>,
    saved: bool,
}

fn device_select(ui: &mut Ui, key: &str, r: Rect, list: &[audio::Device], cur: &str) -> Option<String> {
    let mut opts = vec!["Padrão do Windows".to_string()];
    opts.extend(list.iter().map(|d| d.name.clone()));
    let mut sel = list.iter().position(|d| d.id == cur).map_or(0, |i| i + 1);
    if ui.select(key, r, &opts, &mut sel) {
        return Some(if sel == 0 { String::new() } else { list[sel - 1].id.clone() });
    }
    None
}

fn draw_list(ui: &mut Ui, app: &mut App, t: Rect) {
    ui.title(rect(t.x + 16.0, t.y + 12.0, 300.0, 22.0), "Programas");
    picker::add_button(ui, app, rect(t.right() - 124.0, t.y + 8.0, 108.0, 32.0), Target::Sound);

    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, s) in app.sessions.iter().enumerate() {
        groups.entry(s.exe.to_ascii_lowercase()).or_default().push(i);
    }
    let mut rows: Vec<Row> = Vec::new();
    for idx in groups.values() {
        let (exe, pid, name) = {
            let s = &app.sessions[idx[0]];
            (s.exe.clone(), s.pid, s.name.clone())
        };
        let path = app.path_of(&exe, pid).unwrap_or_default();
        rows.push(Row { saved: app.cfg.snd_apps.iter().any(|a| a.exe.eq_ignore_ascii_case(&exe)), exe, name, path, idx: idx.clone() });
    }
    for a in &app.cfg.snd_apps {
        if !rows.iter().any(|r| r.exe.eq_ignore_ascii_case(&a.exe)) {
            rows.push(Row { exe: a.exe.clone(), name: app_name(&a.exe).to_string(), path: a.path.clone(), idx: Vec::new(), saved: true });
        }
    }
    rows.sort_by_key(|r| r.name.to_lowercase());
    if rows.is_empty() {
        ui.c.text(rect(t.x + 24.0, t.y + 48.0, t.w - 48.0, 24.0), "Nenhum programa com som agora.", 14.0, MUTED, Align::Left, Weight::Regular);
    }
    let outputs = audio::devices(false);
    let inputs = audio::devices(true);
    let list = rect(t.x + 8.0, t.y + 46.0, t.w - 16.0, t.h - 54.0);
    let row_h = 98.0;
    ui.begin_scroll("snd", list, rows.len() as f32 * row_h);
    let mut remove = None;
    let mut changed = None;
    for (i, r) in rows.iter().enumerate() {
        let y = list.y + i as f32 * row_h;
        let focused = r.exe.eq_ignore_ascii_case(&app.fg_exe);
        if focused {
            ui.c.round(rect(list.x, y, list.w, row_h - 4.0), 8.0, CARD2);
        }
        ui.file_icon(&r.path, &r.name, rect(list.x + 12.0, y + 12.0, 26.0, 26.0));
        let running = !r.idx.is_empty();
        let muted = running && r.idx.iter().all(|i| app.sessions[*i].muted);
        let active = r.idx.iter().any(|i| app.sessions[*i].active);
        let state = if !running {
            "Sem som agora"
        } else if focused {
            "Em foco"
        } else if muted {
            "Silenciado"
        } else if active {
            "Tocando"
        } else {
            "Parado"
        };
        ui.c.text(rect(list.x + 48.0, y + 6.0, 214.0, 20.0), &r.name, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(list.x + 48.0, y + 27.0, 214.0, 18.0), state, 12.0, MUTED, Align::Left, Weight::Regular);
        if running {
            let sx = list.x + 270.0;
            let sw = list.w - 270.0 - 130.0;
            let mut v = app.sessions[r.idx[0]].volume;
            if ui.slider(&format!("s{}", r.exe), rect(sx, y + 14.0, sw, 24.0), &mut v) {
                for j in &r.idx {
                    app.sessions[*j].set_volume(v);
                    app.sessions[*j].volume = v;
                }
            }
            let t = format!("{:.0}%", v * 100.0);
            ui.c.text(rect(sx + sw + 8.0, y, 48.0, 52.0), &t, 14.0, TEXT, Align::Right, Weight::Regular);
            let b = rect(list.right() - 58.0, y + 4.0, 44.0, 44.0);
            ui.c.round(b, 8.0, if muted { WARN } else { CARD });
            ui.c.round_line(b, 8.0, BORDER, 1.0);
            let ic = if muted { rgb(0x1f1508) } else { TEXT };
            ui.c.icon(b.x + 12.0, b.y + 12.0, 20.0, &[&[(4., 9.), (4., 15.), (8., 15.), (13., 19.), (13., 5.), (8., 9.), (4., 9.)], &[(17., 9.5), (21., 14.5)], &[(21., 9.5), (17., 14.5)]], ic);
            if ui.hit(&format!("m{}", r.exe), b) {
                for j in &r.idx {
                    app.sessions[*j].set_mute(!muted);
                    app.sessions[*j].muted = !muted;
                }
            }
        }
        let saved = app.cfg.snd_apps.iter().find(|a| a.exe.eq_ignore_ascii_case(&r.exe)).cloned().unwrap_or_default();
        let fw = (list.w - 200.0) / 2.0;
        let fy = y + 54.0;
        ui.c.text(rect(list.x + 12.0, fy, 44.0, 34.0), "Saída", 12.0, MUTED, Align::Left, Weight::Regular);
        if let Some(id) = device_select(ui, &format!("o{}", r.exe), rect(list.x + 56.0, fy, fw, 34.0), &outputs, &saved.output) {
            changed = Some((r.exe.clone(), r.path.clone(), false, id));
        }
        let ix = list.x + 56.0 + fw + 16.0;
        ui.c.text(rect(ix, fy, 56.0, 34.0), "Entrada", 12.0, MUTED, Align::Left, Weight::Regular);
        if let Some(id) = device_select(ui, &format!("i{}", r.exe), rect(ix + 58.0, fy, fw, 34.0), &inputs, &saved.input) {
            changed = Some((r.exe.clone(), r.path.clone(), true, id));
        }
        if r.saved {
            let b = rect(list.right() - 46.0, fy + 5.0, 24.0, 24.0);
            if ui.is_hover(b) {
                ui.c.round(b, 6.0, BORDER);
            }
            ui.c.icon(b.x + 4.0, b.y + 4.0, 16.0, &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]], MUTED);
            if ui.hit(&format!("sx{}", r.exe), b) {
                remove = Some(r.exe.clone());
            }
        }
    }
    ui.end_scroll();
    if let Some((exe, path, input, id)) = changed {
        let e = app.cfg.snd_entry(&exe, &path);
        if input {
            e.input = id;
        } else {
            e.output = id;
        }
        app.cfg.save();
        app.route_app(&exe);
    }
    if let Some(exe) = remove {
        // Tira da lista e devolve o programa ao padrão do Windows.
        let e = app.cfg.snd_entry(&exe, "");
        e.output.clear();
        e.input.clear();
        app.route_app(&exe);
        app.cfg.snd_apps.retain(|a| !a.exe.eq_ignore_ascii_case(&exe));
        app.cfg.save();
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Som", "Volume e aparelhos de saída e de entrada de cada programa.");
    defaults_button(ui, app, a.right() - 124.0, crate::config::Defaults::Volume, "Volta o modificador, o passo e o aviso na tela ao padrão.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let cw = (w - 24.0) / 3.0;
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

    let r2 = rect(r1.right() + 12.0, top, cw, 132.0);
    ui.card(r2);
    ui.label(rect(r2.x + 16.0, r2.y + 10.0, r2.w - 32.0, 18.0), "Manter o som acordado", TEXT2);
    let hint = "Evita o corte no começo do som em HDMI e Dolby. Para sozinho depois de 10 minutos sem uso.";
    ui.c.text_wrap(rect(r2.x + 16.0, r2.y + 78.0, r2.w - 32.0, 48.0), hint, 12.0, MUTED, Weight::Regular);

    let t = rect(x, top + 144.0, w, a.h - top - 164.0);
    ui.card(t);
    if app.pick_for == Some(Target::Sound) {
        picker::draw(ui, app, t);
    } else {
        draw_list(ui, app, t);
    }

    let keep = ["Desligado".to_string(), "Silêncio puro".to_string(), "Sinal inaudível".to_string()];
    let mut s = app.cfg.snd_keep as usize;
    if ui.select("keep", rect(r2.x + 16.0, r2.y + 34.0, r2.w - 32.0, 36.0), &keep, &mut s) {
        app.cfg.snd_keep = s as u8;
        app.cfg.save();
        crate::sys::keepalive::set(s as u8);
    }
    let opts = ["2% por toque".to_string(), "5% por toque".to_string(), "10% por toque".to_string()];
    let vals = [2u32, 5, 10];
    let mut sel = vals.iter().position(|v| *v == app.cfg.vol_step).unwrap_or(1);
    if ui.select("step", rect(r1.x + 16.0, r1.y + 34.0, r1.w - 32.0, 44.0), &opts, &mut sel) {
        app.cfg.vol_step = vals[sel];
        app.cfg.save();
    }
}
