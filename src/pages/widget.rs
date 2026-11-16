use super::*;

const STYLES: [&str; 4] = ["Pílula", "Barras finas", "Anéis", "Instrumento"];

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Widget", "Mostra uso, temperatura, plano de energia, HDR e VRR por cima de tudo. Arraste para mudar de lugar.");
    defaults_button(ui, app, a.right() - 124.0, crate::config::Defaults::Widget, "Volta o atalho, o estilo, os itens e a posição do widget ao padrão, e esconde o widget.");
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

    // Barra de tarefas: só o estilo Pílula cabe nela; o painel abre ao clicar no widget preso.
    let r2 = rect(x, top + 104.0, cw, 60.0);
    ui.card(r2);
    if ui.check("wg-dock", rect(r2.x + 16.0, r2.y + 8.0, r2.w - 248.0, 44.0), "Prender na barra de tarefas (estilo Pílula)", &mut app.cfg.widget_dock) {
        app.cfg.save();
        app.widget.update(&app.cfg, &app.snap);
    }
    // Onde fica na barra: com os ícones centralizados, o canto esquerdo costuma estar livre.
    let mut side = app.cfg.widget_dock_left as usize;
    let sides = ["Ao lado do relógio".to_string(), "No canto esquerdo".to_string()];
    if ui.select("wg-side", rect(r2.right() - 216.0, r2.y + 10.0, 200.0, 40.0), &sides, &mut side) {
        app.cfg.widget_dock_left = side == 1;
        app.cfg.save();
        app.widget.update(&app.cfg, &app.snap);
    }
    let r3 = rect(x + cw + 12.0, top + 104.0, cw, 60.0);
    ui.card(r3);
    if ui.check("wg-panel", r3.inset_xy(16.0, 8.0), "Clique no widget da barra abre o painel detalhado", &mut app.cfg.widget_panel) {
        app.cfg.save();
    }

    let by = top + 176.0;
    let ch = 176.0;
    // Estilos em 2 x 2 à esquerda; a lista de itens à direita.
    let lw = 300.0;
    let sw = (w - lw - 24.0) / 2.0;
    app.widget.refresh(app.cfg.widget_items);
    for (i, name) in STYLES.iter().enumerate() {
        let col = (i % 2) as f32;
        let row = (i / 2) as f32;
        let r = rect(x + col * (sw + 12.0), by + row * (ch + 12.0), sw, ch);
        let sel = app.cfg.widget_style == i as u8;
        if sel {
            ui.card_on(r);
            ui.c.round_line(r, 12.0, ACCENT, 1.5);
        } else {
            ui.card(r);
        }
        ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), name);
        crate::overlays::preview(ui.c, i as u8, rect(r.x + 16.0, r.y + 44.0, r.w - 32.0, r.h - 56.0), &app.snap, &app.widget.status, app.cfg.widget_items);
        if ui.hit(&format!("st{i}"), r) {
            app.cfg.widget_style = i as u8;
            app.cfg.save();
            app.widget.update(&app.cfg, &app.snap);
        }
    }
    let r = rect(x + w - lw, by, lw, ch * 2.0 + 12.0);
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "O que aparece");
    for (i, name) in crate::overlays::ITEMS.iter().enumerate() {
        let mut on = app.cfg.widget_items & (1 << i) != 0;
        if ui.check(&format!("wi{i}"), rect(r.x + 12.0, r.y + 44.0 + i as f32 * 34.0, r.w - 24.0, 32.0), name, &mut on) {
            app.cfg.widget_items = crate::overlays::toggle_item(app.cfg.widget_items, i);
            app.cfg.save();
            app.widget.update(&app.cfg, &app.snap);
        }
    }
}
