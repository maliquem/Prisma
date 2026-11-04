//! Aplicativos de inicialização.

use super::*;
use crate::sys::autostart;

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let on = app.boot_items.iter().filter(|i| i.enabled).count();
    let sub = format!("{on} de {} abrem com o Windows.", app.boot_items.len());
    header(ui, a, "Inicialização", &sub);
    if ui.button("boot-add", rect(a.right() - 168.0, 24.0, 140.0, 44.0), "Adicionar", true) {
        app.pick_exe();
    }

    let x = a.x + 28.0;
    let top = 88.0;
    let h = a.h - top - 20.0;
    let rw = 300.0;
    let l = rect(x, top, a.w - 56.0 - rw - 12.0, h);
    ui.card(l);
    let list = l.inset(8.0);
    ui.begin_scroll("boot", list, app.boot_items.len() as f32 * 52.0);
    let mut toggle = None;
    let mut pick = None;
    for (i, it) in app.boot_items.iter().enumerate() {
        let y = list.y + i as f32 * 52.0;
        let r = rect(list.x, y, list.w, 50.0);
        if app.boot_sel == Some(i) {
            ui.c.round(r, 8.0, CARD2);
        }
        if ui.hit(&format!("bi{i}"), rect(r.x, y, r.w - 120.0, r.h)) {
            pick = Some(i);
        }
        if y - ui.scroll_y() + 50.0 >= list.y && y - ui.scroll_y() <= list.bottom() {
            ui.file_icon(&it.path, &it.name, rect(r.x + 10.0, y + 13.0, 24.0, 24.0));
        }
        let tw = r.w - 170.0;
        ui.c.text(rect(r.x + 44.0, y + 6.0, tw, 20.0), &it.name, 14.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text(rect(r.x + 44.0, y + 26.0, tw, 18.0), it.source.label(), 12.0, MUTED, Align::Left, Weight::Regular);
        let b = rect(r.right() - 112.0, y + 9.0, 104.0, 32.0);
        let label = if it.enabled { "Ativado" } else { "Desativado" };
        if ui.button(&format!("bt{i}"), b, label, it.enabled) {
            toggle = Some(i);
        }
    }
    ui.end_scroll();
    if let Some(i) = pick {
        app.select_boot(i);
    }
    if let Some(i) = toggle {
        let it = app.boot_items[i].clone();
        let want = !it.enabled;
        if autostart::set_enabled(&it, want) {
            app.boot_items[i].enabled = want;
            app.log(&format!("inicialização {} {}", if want { "ativou" } else { "desativou" }, it.name));
        } else {
            app.toast.info("Precisa do Prisma como administrador");
        }
    }

    let r = rect(l.right() + 12.0, top, rw, h);
    ui.card(r);
    let Some(i) = app.boot_sel else {
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 16.0, r.w - 32.0, 60.0), "Escolha um item para ver de onde ele vem e quem assinou.", 14.0, MUTED, Weight::Regular);
        return;
    };
    let Some(it) = app.boot_items.get(i).cloned() else { return };
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), &it.name);
    let mut y = r.y + 44.0;
    ui.label(rect(r.x + 16.0, y, r.w - 32.0, 18.0), "Assinatura", MUTED);
    let sig = app.boot_sig.clone().unwrap_or_default();
    ui.c.text_wrap(rect(r.x + 16.0, y + 20.0, r.w - 32.0, 40.0), &sig, 13.0, TEXT, Weight::Regular);
    y += 66.0;
    ui.label(rect(r.x + 16.0, y, r.w - 32.0, 18.0), "Comando", MUTED);
    ui.c.text_wrap(rect(r.x + 16.0, y + 20.0, r.w - 32.0, 120.0), &it.command, 12.0, TEXT2, Weight::Regular);

    let b = rect(r.x + 16.0, r.bottom() - 56.0, r.w - 32.0, 44.0);
    let label = if app.boot_confirm { "Confirmar remoção" } else { "Remover da inicialização" };
    if ui.button("boot-rm", b, label, false) {
        if app.boot_confirm {
            if autostart::remove(&it) {
                app.log(&format!("inicialização removeu {} = {}", it.name, it.command));
                app.boot_items.remove(i);
                app.boot_sel = None;
            } else {
                app.toast.info("Precisa do Prisma como administrador");
            }
            app.boot_confirm = false;
        } else {
            app.boot_confirm = true;
        }
    }
}
