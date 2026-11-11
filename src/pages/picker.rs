//! Adicionar programa: o mesmo botão em todas as telas que usam, com a lista em janela própria.

use super::*;
use crate::sys::{catalog, procs};

pub const SEARCH: &str = "pick-q";

#[derive(Clone, Copy, PartialEq)]
pub enum Target {
    Sound,
    Fullscreen,
    HdrGame,
    MeterGame,
    NvGame,
}

pub fn add_button(ui: &mut Ui, app: &mut App, r: Rect, target: Target) {
    let (key, menu) = match target {
        Target::Sound => ("add-sound", "add-sound-menu"),
        Target::Fullscreen => ("add-full", "add-full-menu"),
        Target::HdrGame => ("add-game", "add-game-menu"),
        Target::MeterGame => ("add-meter", "add-meter-menu"),
        Target::NvGame => ("add-nv", "add-nv-menu"),
    };
    if ui.button(key, r, "Adicionar", false) {
        ui.menu(menu, &[("Da lista de programas", true), ("Escolher arquivo .exe", true)]);
    }
    match ui.menu_pick(menu) {
        Some(0) => app.open_picker(target),
        Some(_) => app.pick_exe_for(target),
        None => {}
    }
}

fn add(app: &mut App, target: Target, path: &str) {
    let exe = procs::exe_name(path);
    match target {
        Target::Sound => {
            app.cfg.snd_entry(&exe, path);
        }
        Target::Fullscreen => app.cfg.fs_note(&exe, path),
        Target::MeterGame => app.add_meter_game(&exe, path),
        Target::NvGame => super::nvidia::add_game(app, &exe),
        Target::HdrGame => {
            if !app.cfg.hdr_games.iter().any(|g| g.exe.eq_ignore_ascii_case(&exe)) {
                app.cfg.hdr_games.push(crate::config::HdrGame { exe, path: path.to_string(), mode: 0 });
            }
        }
    }
    app.cfg.save();
    app.close_picker();
}

pub fn draw(ui: &mut Ui, app: &mut App, t: Rect) {
    let Some(target) = app.pick_for else { return };
    let mut q = std::mem::take(&mut app.pick_search);
    ui.search(SEARCH, rect(t.x + 16.0, t.y + 14.0, t.w - 32.0, 34.0), &mut q, "Buscar");
    app.pick_search = q;
    let list = rect(t.x + 8.0, t.y + 58.0, t.w - 16.0, t.h - 66.0);
    let needle = app.pick_search.to_lowercase();
    let rows: Option<Vec<catalog::Entry>> = app.pick_list.lock().unwrap().as_ref().map(|all| all.iter().filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle)).cloned().collect());
    let Some(rows) = rows else {
        ui.c.text(rect(list.x + 16.0, list.y + 4.0, list.w - 32.0, 24.0), "Procurando programas…", 14.0, MUTED, Align::Left, Weight::Regular);
        return;
    };
    if rows.is_empty() {
        ui.c.text(rect(list.x + 16.0, list.y + 4.0, list.w - 32.0, 24.0), "Nenhum programa encontrado.", 14.0, MUTED, Align::Left, Weight::Regular);
    }
    ui.begin_scroll("pick", list, rows.len() as f32 * 44.0);
    let first = (ui.scroll_y() / 44.0) as usize;
    let last = first + (list.h / 44.0) as usize + 2;
    let mut pick = None;
    for (i, e) in rows.iter().enumerate().take(last).skip(first) {
        let y = list.y + i as f32 * 44.0;
        let r = rect(list.x, y, list.w, 42.0);
        if ui.is_hover(r) {
            ui.c.round(r, 6.0, CARD2);
        }
        ui.file_icon(&e.path, &e.name, rect(r.x + 10.0, y + 9.0, 24.0, 24.0));
        ui.c.text(rect(r.x + 44.0, y, r.w - 60.0, 42.0), &e.name, 14.0, TEXT, Align::Left, Weight::Regular);
        if ui.hit(&format!("pk{i}"), r) {
            pick = Some(e.path.clone());
        }
    }
    ui.end_scroll();
    if let Some(path) = pick {
        add(app, target, &path);
    }
}
