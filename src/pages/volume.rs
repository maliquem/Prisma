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
    pinned: bool,
}

/// Volume de programa fixo mudado: grava um pouco depois, não a cada passo do arraste.
static SAVE_AT: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// Alfinete de fixar.
const PIN: [&[(f32, f32)]; 2] = [&[(9., 4.), (15., 4.), (14., 10.), (17., 13.), (7., 13.), (10., 10.), (9., 4.)], &[(12., 13.), (12., 20.)]];

fn device_select(ui: &mut Ui, key: &str, r: Rect, list: &[audio::Device], cur: &str) -> Option<String> {
    let mut opts = vec!["Padrão do Windows".to_string()];
    opts.extend(list.iter().map(|d| d.name.clone()));
    let mut sel = list.iter().position(|d| d.id == cur).map_or(0, |i| i + 1);
    if ui.select(key, r, &opts, &mut sel) {
        return Some(if sel == 0 { String::new() } else { list[sel - 1].id.clone() });
    }
    None
}

fn draw_list(ui: &mut Ui, app: &mut App, t: Rect, outputs: &[audio::Device], inputs: &[audio::Device]) {
    {
        let mut at = SAVE_AT.lock().unwrap();
        if at.is_some_and(|t| t.elapsed().as_millis() > 600) {
            *at = None;
            app.cfg.save();
        }
    }
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
        let saved = app.cfg.snd_apps.iter().find(|a| a.exe.eq_ignore_ascii_case(&exe));
        rows.push(Row { saved: saved.is_some(), pinned: saved.is_some_and(|a| a.pinned), exe, name, path, idx: idx.clone() });
    }
    for a in &app.cfg.snd_apps {
        if !rows.iter().any(|r| r.exe.eq_ignore_ascii_case(&a.exe)) {
            rows.push(Row { exe: a.exe.clone(), name: app_name(&a.exe).to_string(), path: a.path.clone(), idx: Vec::new(), saved: true, pinned: a.pinned });
        }
    }
    // Fixos primeiro.
    rows.sort_by_key(|r| (!r.pinned, r.name.to_lowercase()));
    if rows.is_empty() {
        ui.c.text(rect(t.x + 24.0, t.y + 48.0, t.w - 48.0, 24.0), "Nenhum programa com som agora.", 14.0, MUTED, Align::Left, Weight::Regular);
    }
    let list = rect(t.x + 8.0, t.y + 46.0, t.w - 16.0, t.h - 54.0);
    let row_h = 98.0;
    ui.begin_scroll("snd", list, rows.len() as f32 * row_h);
    let mut remove = None;
    let mut changed = None;
    let mut pin = None;
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
        ui.c.text(rect(list.x + 48.0, y + 6.0, 184.0, 20.0), &r.name, 14.0, TEXT, Align::Left, Weight::Regular);
        let state = if r.pinned && !running { "Fixo, fechado" } else { state };
        ui.c.text(rect(list.x + 48.0, y + 27.0, 184.0, 18.0), state, 12.0, MUTED, Align::Left, Weight::Regular);
        // Fixar: fica na lista fechado e guarda volume, saída e entrada.
        let pb = rect(list.x + 236.0, y + 12.0, 28.0, 28.0);
        let pin_hover = ui.is_hover(pb);
        if pin_hover || r.pinned {
            ui.c.round(pb, 6.0, if r.pinned { CARD } else { BORDER });
        }
        ui.c.icon(pb.x + 4.0, pb.y + 4.0, 20.0, &PIN, if r.pinned { ACCENT } else { MUTED });
        if ui.hit(&format!("sp{}", r.exe), pb) {
            pin = Some((r.exe.clone(), r.path.clone(), !r.pinned, running.then(|| app.sessions[r.idx[0]].volume)));
        }
        let sx = list.x + 276.0;
        let sw = list.w - 276.0 - 130.0;
        let kept = app.cfg.snd_apps.iter().find(|a| a.exe.eq_ignore_ascii_case(&r.exe)).map_or(-1, |a| a.volume);
        if running || (r.pinned && kept >= 0) {
            let mut v = if running { app.sessions[r.idx[0]].volume } else { kept as f32 / 100.0 };
            if ui.slider(&format!("s{}", r.exe), rect(sx, y + 14.0, sw, 24.0), &mut v) {
                for j in &r.idx {
                    app.sessions[*j].set_volume(v);
                    app.sessions[*j].volume = v;
                }
                if r.pinned {
                    app.cfg.snd_entry(&r.exe, &r.path).volume = (v * 100.0).round() as i32;
                    *SAVE_AT.lock().unwrap() = Some(std::time::Instant::now());
                }
            }
            let t = format!("{:.0}%", v * 100.0);
            ui.c.text(rect(sx + sw + 8.0, y, 48.0, 52.0), &t, 14.0, TEXT, Align::Right, Weight::Regular);
        }
        if running {
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
        if let Some(id) = device_select(ui, &format!("o{}", r.exe), rect(list.x + 56.0, fy, fw, 34.0), outputs, &saved.output) {
            changed = Some((r.exe.clone(), r.path.clone(), false, id));
        }
        let ix = list.x + 56.0 + fw + 16.0;
        ui.c.text(rect(ix, fy, 56.0, 34.0), "Entrada", 12.0, MUTED, Align::Left, Weight::Regular);
        if let Some(id) = device_select(ui, &format!("i{}", r.exe), rect(ix + 58.0, fy, fw, 34.0), inputs, &saved.input) {
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
    if let Some((exe, path, on, vol)) = pin {
        let e = app.cfg.snd_entry(&exe, &path);
        e.pinned = on;
        if on {
            if let Some(v) = vol {
                e.volume = (v * 100.0).round() as i32;
            }
        } else {
            e.volume = -1;
            if e.output.is_empty() && e.input.is_empty() {
                app.cfg.snd_apps.retain(|a| !a.exe.eq_ignore_ascii_case(&exe));
            }
        }
        app.cfg.save();
    }
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

    let outputs = audio::devices(false);
    let inputs = audio::devices(true);
    let n = app.cfg.snd_keys.len();
    let kh = 56.0 + n.max(1) as f32 * 44.0;
    let k = rect(x, top + 144.0, w, kh);
    ui.card(k);
    ui.title(rect(k.x + 16.0, k.y + 12.0, k.w - 160.0, 22.0), "Atalhos de aparelho padrão");
    if ui.button_ex("snd-k-add", rect(k.right() - 124.0, k.y + 8.0, 108.0, 32.0), "Adicionar", false, n < crate::app::SND_KEYS_MAX) {
        app.cfg.snd_keys.push(crate::config::SndKey::default());
        app.cfg.save();
    }
    if n == 0 {
        ui.c.text_wrap(rect(k.x + 16.0, k.y + 46.0, k.w - 32.0, 40.0), "Cada atalho troca a saída e a entrada padrão do Windows de uma vez. Use Adicionar.", 12.0, MUTED, Weight::Regular);
    }
    let fw = 200.0f32.min(k.w * 0.24);
    let dw = ((k.w - 32.0 - fw - 40.0 - 2.0 * 64.0 - 24.0) / 2.0).max(80.0);
    let mut keys_changed = false;
    let mut drop = None;
    let mut selects = Vec::new();
    for i in 0..n {
        let ry = k.y + 46.0 + i as f32 * 44.0;
        let mut hk = app.cfg.snd_keys[i].key;
        if ui.hotkey(&format!("snd-k{i}"), rect(k.x + 16.0, ry, fw, 36.0), &mut hk) {
            app.cfg.snd_keys[i].key = hk;
            keys_changed = true;
        }
        let ox = k.x + 16.0 + fw + 12.0;
        ui.c.text(rect(ox, ry, 60.0, 36.0), "Saída", 12.0, MUTED, Align::Left, Weight::Regular);
        let ix = ox + 64.0 + dw + 12.0;
        ui.c.text(rect(ix, ry, 60.0, 36.0), "Entrada", 12.0, MUTED, Align::Left, Weight::Regular);
        selects.push((i, rect(ox + 64.0, ry, dw, 36.0), rect(ix + 64.0, ry, dw, 36.0)));
        let b = rect(k.right() - 44.0, ry + 6.0, 24.0, 24.0);
        if ui.is_hover(b) {
            ui.c.round(b, 6.0, BORDER);
        }
        ui.c.icon(b.x + 4.0, b.y + 4.0, 16.0, &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]], MUTED);
        if ui.hit(&format!("snd-kx{i}"), b) {
            drop = Some(i);
        }
    }

    let t = rect(x, k.bottom() + 12.0, w, (a.h - 20.0 - k.bottom() - 12.0).max(120.0));
    ui.card(t);
    draw_list(ui, app, t, &outputs, &inputs);

    // Listas dos atalhos por último, para abrirem por cima da lista de programas.
    let keep_side = |list: &[audio::Device]| -> Vec<String> { std::iter::once(crate::i18n::tr("Não mudar").into_owned()).chain(list.iter().map(|d| d.name.clone())).collect() };
    let (out_names, in_names) = (keep_side(&outputs), keep_side(&inputs));
    for (i, ro, ri) in selects {
        for (capture, r, names, list) in [(false, ro, &out_names, &outputs), (true, ri, &in_names, &inputs)] {
            let cur = if capture { &app.cfg.snd_keys[i].input } else { &app.cfg.snd_keys[i].output };
            let mut sel = list.iter().position(|d| &d.id == cur).map_or(0, |p| p + 1);
            if ui.select(&format!("snd-k{}{i}", if capture { "i" } else { "o" }), r, names, &mut sel) {
                let id = if sel == 0 { String::new() } else { list[sel - 1].id.clone() };
                let e = &mut app.cfg.snd_keys[i];
                if capture {
                    e.input = id;
                } else {
                    e.output = id;
                }
                app.cfg.save();
            }
        }
    }
    if let Some(i) = drop {
        app.cfg.snd_keys.remove(i);
        keys_changed = true;
    }
    if keys_changed {
        app.cfg.save();
        app.register_hotkeys();
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
