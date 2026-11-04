use super::*;

const STYLES: [&str; 4] = ["Pílula", "Barras finas", "Anéis", "Instrumento"];

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Widget", "Mostra CPU, GPU, RAM e VRAM por cima de tudo. Arraste para mudar de lugar.");
    defaults_button(ui, app, a.right() - 124.0, crate::config::Defaults::Widget, "Volta o atalho, o estilo e a posição do widget ao padrão, e esconde o widget.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let cw = (w - 12.0) / 2.0;
    let r0 = rect(x, top, cw, 92.0);
    ui.card_on(r0);
    let mut on = app.widget.visible;
    if ui.check("wg-on", r0.inset_xy(16.0, 24.0), "Mostrar widget", &mut on) {
        app.set_widget(on);
    }
    let r1 = rect(x + cw + 12.0, top, cw, 92.0);
    ui.card(r1);
    ui.label(rect(r1.x + 16.0, r1.y + 10.0, r1.w - 32.0, 18.0), "Atalho para ligar e desligar", TEXT2);
    if ui.hotkey("hk-wg", rect(r1.x + 16.0, r1.y + 34.0, 240.0, 44.0), &mut app.cfg.hk_widget) {
        app.cfg.save();
        app.register_hotkeys();
    }

    let by = top + 104.0;
    let ch = 176.0;
    for (i, name) in STYLES.iter().enumerate() {
        let col = (i % 2) as f32;
        let row = (i / 2) as f32;
        let r = rect(x + col * (cw + 12.0), by + row * (ch + 12.0), cw, ch);
        let sel = app.cfg.widget_style == i as u8;
        if sel {
            ui.card_on(r);
            ui.c.round_line(r, 12.0, ACCENT, 1.5);
        } else {
            ui.card(r);
        }
        ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), name);
        crate::overlays::preview(ui.c, i as u8, rect(r.x + 16.0, r.y + 44.0, r.w - 32.0, r.h - 56.0), &app.snap);
        if ui.hit(&format!("st{i}"), r) {
            app.cfg.widget_style = i as u8;
            app.cfg.save();
            app.widget.update(&app.cfg, &app.snap);
        }
    }
}
