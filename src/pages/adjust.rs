use super::*;

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Ajustes", "Início com o Windows, permissões e atalhos gerais.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let r0 = rect(x, top, w, 150.0);
    ui.card(r0);
    let mut on = app.autostart;
    let enabled = app.elevated;
    if enabled {
        if ui.check("auto", rect(r0.x + 16.0, r0.y + 8.0, r0.w - 32.0, 44.0), "Iniciar com o Windows, como administrador", &mut on) {
            if crate::startup::set_enabled(on) {
                app.autostart = on;
            }
        }
    } else {
        let t = if app.autostart { "Inicia com o Windows" } else { "Não inicia com o Windows" };
        ui.c.text(rect(r0.x + 16.0, r0.y + 8.0, r0.w - 32.0, 44.0), t, 14.0, TEXT, Align::Left, Weight::Regular);
    }
    let st = if app.elevated {
        "Rodando como administrador: consegue mexer em todas as janelas e processos."
    } else {
        "Rodando sem administrador: não alcança programas abertos como administrador."
    };
    ui.c.text(rect(r0.x + 16.0, r0.y + 54.0, r0.w - 32.0, 22.0), st, 13.0, MUTED, Align::Left, Weight::Regular);
    if !app.elevated && ui.button("elev", rect(r0.x + 16.0, r0.y + 90.0, 240.0, 44.0), "Reabrir como administrador", true) {
        crate::startup::relaunch_elevated();
    }

    let r1 = rect(x, top + 162.0, w, 92.0);
    ui.card(r1);
    ui.label(rect(r1.x + 16.0, r1.y + 10.0, r1.w - 32.0, 18.0), "Atalho de Otimizar", TEXT2);
    if ui.hotkey("hk-opt", rect(r1.x + 16.0, r1.y + 34.0, 240.0, 44.0), &mut app.cfg.hk_optimize) {
        app.cfg.save();
        app.register_hotkeys();
    }

    if !app.hk_errors.is_empty() {
        let r2 = rect(x, top + 266.0, w, 40.0 + app.hk_errors.len() as f32 * 22.0);
        ui.card(r2);
        ui.c.text(rect(r2.x + 16.0, r2.y + 8.0, r2.w - 32.0, 22.0), "Atalhos que não funcionaram", 14.0, WARN, Align::Left, Weight::Semibold);
        for (i, e) in app.hk_errors.iter().enumerate() {
            ui.label(rect(r2.x + 16.0, r2.y + 32.0 + i as f32 * 22.0, r2.w - 32.0, 20.0), e, TEXT2);
        }
    }
}
