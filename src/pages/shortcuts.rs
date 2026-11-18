//! Atalhos: combinações de teclas que abrem programas, da lista ou de um .exe.

use super::picker::Target;
use super::*;
use crate::app::RUN_KEYS_MAX;
use crate::config::RunKey;

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Atalhos", "Combinações de teclas que abrem programas de qualquer lugar do Windows.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let n = app.cfg.run_keys.len();
    let k = rect(x, top, w, a.h - 20.0 - top);
    ui.card(k);
    ui.title(rect(k.x + 16.0, k.y + 12.0, k.w - 160.0, 22.0), "Atalhos de programa");
    if ui.button_ex("run-add", rect(k.right() - 124.0, k.y + 8.0, 108.0, 32.0), "Adicionar", false, n < RUN_KEYS_MAX) {
        app.cfg.run_keys.push(RunKey::default());
        app.cfg.save();
    }
    let mut uac = app.cfg.run_key_uac;
    if ui.check("run-uac", rect(k.x + 16.0, k.y + 44.0, k.w - 32.0, 24.0), "Pedir confirmação do Windows ao abrir atalhos como administrador", &mut uac) {
        app.cfg.run_key_uac = uac;
        app.cfg.save();
    }
    if n == 0 {
        ui.c.text_wrap(rect(k.x + 16.0, k.y + 78.0, k.w - 32.0, 40.0), "Por exemplo, Ctrl + Alt + T para abrir o terminal. Use Adicionar.", 12.0, MUTED, Weight::Regular);
    }
    let fw = 200.0f32.min(k.w * 0.24);
    let bw = 150.0;
    let cw = 190.0;
    let list = rect(k.x, k.y + 78.0, k.w, k.h - 86.0);
    ui.begin_scroll("run-list", list, n as f32 * 44.0);
    let mut keys_changed = false;
    let mut drop = None;
    for i in 0..n {
        let ry = list.y + i as f32 * 44.0;
        let mut hk = app.cfg.run_keys[i].key;
        if ui.hotkey(&format!("run-k{i}"), rect(k.x + 16.0, ry, fw, 36.0), &mut hk) {
            app.cfg.run_keys[i].key = hk;
            keys_changed = true;
        }
        let px = k.x + 16.0 + fw + 12.0;
        let pw = (k.right() - 44.0 - cw - bw - 24.0 - px).max(60.0);
        let path = app.cfg.run_keys[i].path.clone();
        if path.is_empty() {
            ui.c.text(rect(px, ry, pw, 36.0), "Nenhum programa escolhido", 13.0, MUTED, Align::Left, Weight::Regular);
        } else {
            let name = crate::sys::procs::exe_name(&path);
            ui.file_icon(&path, &name, rect(px, ry + 6.0, 24.0, 24.0));
            ui.c.text(rect(px + 32.0, ry, pw - 32.0, 36.0), &name, 13.0, TEXT, Align::Left, Weight::Regular);
        }
        // Outro programa já tem a combinação: o registro falhou e a linha avisa.
        let label = hk.label();
        if hk.vk != 0 && app.hk_errors.contains(&format!("Abrir programa: {label} já está em uso")) {
            ui.c.text(rect(k.x + 16.0, ry + 30.0, fw, 14.0), "Já usada por outro programa", 10.0, WARN, Align::Left, Weight::Regular);
        }
        let menu = format!("run-menu{i}");
        if ui.button(&format!("run-p{i}"), rect(px + pw + 12.0, ry, bw, 36.0), "Escolher programa", false) {
            ui.menu(&menu, &[("Da lista de programas", true), ("Escolher arquivo .exe", true)]);
        }
        match ui.menu_pick(&menu) {
            Some(0) => app.open_picker(Target::RunKey(i as u8)),
            Some(_) => app.pick_exe_for(Target::RunKey(i as u8)),
            None => {}
        }
        let mut admin = app.cfg.run_keys[i].admin;
        if ui.check(&format!("run-a{i}"), rect(px + pw + bw + 24.0, ry + 6.0, cw, 24.0), "Como administrador", &mut admin) {
            app.cfg.run_keys[i].admin = admin;
            app.cfg.save();
        }
        let b = rect(k.right() - 44.0, ry + 6.0, 24.0, 24.0);
        if ui.is_hover(b) {
            ui.c.round(b, 6.0, BORDER);
        }
        ui.c.icon(b.x + 4.0, b.y + 4.0, 16.0, &[&[(6., 6.), (18., 18.)], &[(18., 6.), (6., 18.)]], MUTED);
        if ui.hit(&format!("run-x{i}"), b) {
            drop = Some(i);
        }
    }
    ui.end_scroll();
    if let Some(i) = drop {
        app.cfg.run_keys.remove(i);
        keys_changed = true;
    }
    if keys_changed {
        app.cfg.save();
        app.register_hotkeys();
    }
}

/// Programa escolhido para o atalho `i` (lista de programas ou arquivo .exe).
pub fn set_program(app: &mut App, i: usize, path: &str) {
    let Some(k) = app.cfg.run_keys.get_mut(i) else { return };
    k.path = path.to_string();
    app.cfg.save();
}
