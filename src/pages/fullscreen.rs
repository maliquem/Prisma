use super::picker::{self, Target};
use super::*;
use crate::config::Defaults;

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Fullscreenizer", "Deixa a janela em foco em tela cheia sem bordas. O mesmo atalho desfaz.");
    defaults_button(ui, app, a.right() - 124.0, Defaults::Fullscreen, "Volta o atalho ao padrão e desliga o modo automático de todos os programas. A lista continua salva.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let cw = (w - 12.0) / 2.0;
    let r0 = rect(x, top, cw, 92.0);
    ui.card_on(r0);
    ui.label(rect(r0.x + 16.0, r0.y + 10.0, r0.w - 32.0, 18.0), "Atalho", TEXT2);
    if ui.hotkey("hk-full", rect(r0.x + 16.0, r0.y + 34.0, 240.0, 44.0), &mut app.cfg.hk_fullscreen) {
        app.cfg.save();
        app.register_hotkeys();
    }
    let r1 = rect(x + cw + 12.0, top, cw, 92.0);
    ui.card(r1);
    let hint = "Quem usa o Fullscreenizer uma vez entra na lista. Marque Automático para aplicar sempre que o programa abrir.";
    ui.c.text_wrap(rect(r1.x + 16.0, r1.y + 16.0, r1.w - 32.0, 60.0), hint, 13.0, TEXT2, Weight::Regular);

    let h = a.h - top - 124.0;
    let t = rect(x, top + 104.0, cw, h);
    ui.card(t);
    ui.title(rect(t.x + 16.0, t.y + 12.0, t.w - 32.0, 22.0), "Janelas abertas");
    let list = rect(t.x + 8.0, t.y + 42.0, t.w - 16.0, t.h - 50.0);
    ui.begin_scroll("wins", list, app.windows.len() as f32 * 54.0);
    let mut toggle = None;
    for (i, wi) in app.windows.iter().enumerate() {
        let y = list.y + i as f32 * 54.0;
        let on = app.full.is_on(wi.hwnd);
        if on {
            ui.c.round(rect(list.x, y, list.w, 52.0), 8.0, CARD2);
        }
        ui.file_icon(&wi.path, &wi.exe, rect(list.x + 8.0, y + 15.0, 22.0, 22.0));
        ui.c.text(rect(list.x + 42.0, y + 6.0, list.w - 170.0, 20.0), &wi.title, 14.0, TEXT, Align::Left, Weight::Regular);
        let st = format!("{} · {} × {}", if on { "Sem bordas" } else { "Janela" }, wi.size.0, wi.size.1);
        ui.c.text(rect(list.x + 42.0, y + 27.0, list.w - 170.0, 18.0), &st, 12.0, MUTED, Align::Left, Weight::Regular);
        let label = if on { "Restaurar" } else { "Tela cheia" };
        if ui.button(&format!("fs{}", wi.hwnd.0 as isize), rect(list.right() - 118.0, y + 8.0, 104.0, 36.0), label, false) {
            toggle = Some((wi.hwnd, wi.exe.clone()));
        }
    }
    ui.end_scroll();
    if let Some((h, exe)) = toggle {
        app.toggle_window(h, &exe);
    }

    let s = rect(t.right() + 12.0, t.y, cw, h);
    ui.card(s);
    if app.pick_for == Some(Target::Fullscreen) {
        picker::draw(ui, app, s);
        return;
    }
    ui.title(rect(s.x + 16.0, s.y + 12.0, s.w - 150.0, 22.0), "Programas salvos");
    picker::add_button(ui, app, rect(s.right() - 124.0, s.y + 8.0, 108.0, 32.0), Target::Fullscreen);
    let list = rect(s.x + 8.0, s.y + 46.0, s.w - 16.0, s.h - 54.0);
    if app.cfg.fs_apps.is_empty() {
        ui.c.text_wrap(rect(list.x + 8.0, list.y + 4.0, list.w - 16.0, 44.0), "Nenhum programa ainda. Use o Fullscreenizer em uma janela ou clique em Adicionar.", 13.0, MUTED, Weight::Regular);
    }
    ui.begin_scroll("fs-apps", list, app.cfg.fs_apps.len() as f32 * 48.0);
    let mut remove = None;
    let mut changed = false;
    let apps = &mut app.cfg.fs_apps;
    for (i, p) in apps.iter_mut().enumerate() {
        let y = list.y + i as f32 * 48.0;
        ui.c.line(list.x + 8.0, y, list.right() - 14.0, y, LINE, 1.0);
        ui.file_icon(&p.path, &p.exe, rect(list.x + 8.0, y + 12.0, 24.0, 24.0));
        ui.c.text(rect(list.x + 42.0, y, list.w - 216.0, 48.0), app_name(&p.exe), 14.0, TEXT, Align::Left, Weight::Regular);
        changed |= ui.check(&format!("fsa{i}"), rect(list.right() - 170.0, y + 2.0, 120.0, 44.0), "Automático", &mut p.auto);
        let b = rect(list.right() - 44.0, y + 12.0, 24.0, 24.0);
        if ui.is_hover(b) {
            ui.c.round(b, 6.0, BORDER);
        }
        ui.c.icon(b.x + 4.0, b.y + 4.0, 16.0, &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]], MUTED);
        if ui.hit(&format!("fsx{i}"), b) {
            remove = Some(i);
        }
    }
    ui.end_scroll();
    if let Some(i) = remove {
        app.cfg.fs_apps.remove(i);
        changed = true;
    }
    if changed {
        app.cfg.save();
    }
}
