//! Tela de edição de um plano de energia, aberta pela lista da aba Energia.

use super::*;
use crate::sys::power;
use crate::sys::powerplan::{Kind, Setting};

const ROW: f32 = 44.0;
const CONTROL: f32 = 230.0;

enum Row {
    Head(String),
    Item(usize),
}

fn visible(list: &[Setting], needle: &str, hidden: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut last = "";
    for (i, s) in list.iter().enumerate() {
        if (s.hidden && !hidden) || !(needle.is_empty() || s.name.to_lowercase().contains(needle) || s.group.to_lowercase().contains(needle)) {
            continue;
        }
        if s.group != last {
            last = &s.group;
            rows.push(Row::Head(s.group.clone()));
        }
        rows.push(Row::Item(i));
    }
    rows
}

/// Passo que não obriga a clicar milhares de vezes em faixas grandes, como segundos.
fn step_for(min: u32, max: u32, step: u32) -> u32 {
    match max - min {
        0..=100 => step,
        101..=1000 => step.max(10),
        _ => step.max(60),
    }
}

fn control(ui: &mut Ui, key: &str, r: Rect, s: &Setting, value: u32) -> Option<u32> {
    match &s.kind {
        Kind::Options(list) => {
            let mut names: Vec<String> = list.iter().map(|o| o.0.clone()).collect();
            let found = list.iter().position(|o| o.1 == value);
            if found.is_none() {
                names.push(value.to_string());
            }
            let mut sel = found.unwrap_or(names.len() - 1);
            (ui.select(key, r, &names, &mut sel) && sel < list.len()).then(|| list[sel].1)
        }
        Kind::Range { min, max, step, unit } => {
            let mut v = value;
            ui.stepper(key, r, &mut v, (*min, *max), step_for(*min, *max, *step), unit).then_some(v)
        }
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let Some(id) = app.pw_edit.clone() else { return };
    let plan = app.plans.iter().find(|p| p.id == id).cloned();
    let Some(plan) = plan else {
        app.pw_edit = None;
        return;
    };
    header(ui, a, "Editar plano", &plan.name);
    if ui.button("pw-back", rect(a.right() - 124.0, 30.0, 96.0, 36.0), "Voltar", false) {
        app.close_plan();
        return;
    }
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let t = rect(x, 88.0, w, a.h - 108.0);
    ui.card(t);
    let mut q = std::mem::take(&mut app.pw_search);
    ui.search("pw-q", rect(t.x + 16.0, t.y + 12.0, t.w - 230.0, 34.0), &mut q, "Buscar");
    app.pw_search = q;
    let mut hidden = app.pw_hidden;
    if ui.check("pw-hidden", rect(t.right() - 200.0, t.y + 8.0, 190.0, 40.0), "Mostrar ocultas", &mut hidden) {
        app.pw_hidden = hidden;
    }
    let battery = app.pw_battery;
    let ctrl_x = |dc: bool| t.right() - 26.0 - if battery && !dc { 2.0 * CONTROL + 12.0 } else { CONTROL };
    ui.c.text(rect(ctrl_x(false), t.y + 54.0, CONTROL, 20.0), "Na tomada", 12.0, MUTED, Align::Left, Weight::Regular);
    if battery {
        ui.c.text(rect(ctrl_x(true), t.y + 54.0, CONTROL, 20.0), "Na bateria", 12.0, MUTED, Align::Left, Weight::Regular);
    }
    let shared = app.pw_list.clone();
    let list = shared.lock().unwrap().clone();
    let area = rect(t.x + 8.0, t.y + 78.0, t.w - 16.0, t.h - 86.0);
    let Some(list) = list else {
        ui.c.text(rect(area.x + 8.0, area.y, 300.0, 24.0), "Lendo o plano…", 14.0, MUTED, Align::Left, Weight::Regular);
        return;
    };
    let needle = app.pw_search.to_lowercase();
    let rows = visible(&list, &needle, app.pw_hidden);
    if rows.is_empty() {
        ui.c.text(rect(area.x + 8.0, area.y, 400.0, 24.0), "Nenhuma opção encontrada.", 14.0, MUTED, Align::Left, Weight::Regular);
    }
    ui.begin_scroll("pw-list", area, rows.len() as f32 * ROW);
    let first = (ui.scroll_y() / ROW) as usize;
    let last = first + (area.h / ROW) as usize + 2;
    let mut change = None;
    for (i, row) in rows.iter().enumerate().take(last).skip(first) {
        let y = area.y + i as f32 * ROW;
        match row {
            Row::Head(name) => ui.title(rect(area.x + 8.0, y + 14.0, area.w - 24.0, 24.0), name),
            Row::Item(n) => {
                let s = &list[*n];
                let room = ctrl_x(false) - area.x - 28.0;
                ui.c.text(rect(area.x + 8.0, y, room, ROW), &s.name, 13.0, if s.hidden { TEXT2 } else { TEXT }, Align::Left, Weight::Regular);
                let key = power::guid_str(&s.id);
                if let Some(v) = control(ui, &format!("a{key}"), rect(ctrl_x(false), y + 5.0, CONTROL, 34.0), s, s.ac) {
                    change = Some((*n, false, v));
                }
                if battery {
                    if let Some(v) = control(ui, &format!("d{key}"), rect(ctrl_x(true), y + 5.0, CONTROL, 34.0), s, s.dc) {
                        change = Some((*n, true, v));
                    }
                }
            }
        }
    }
    ui.end_scroll();
    if let Some((n, dc, v)) = change {
        app.set_plan_value(&plan, n, dc, v);
    }
}
