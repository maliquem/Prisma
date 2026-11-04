use super::*;

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Fullscreenizer", "Deixa a janela em foco em tela cheia sem bordas. O mesmo atalho desfaz.");
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
    let mut mem = app.cfg.fs_remember;
    let box_ = r1.inset_xy(16.0, 24.0);
    if ui.check("fs-mem", box_, "Aplicar sozinho quando o programa abrir de novo", &mut mem) {
        app.cfg.fs_remember = mem;
        app.cfg.save();
    }

    let t = rect(x, top + 104.0, w, a.h - top - 124.0);
    ui.card(t);
    ui.title(rect(t.x + 16.0, t.y + 12.0, 300.0, 22.0), "Janelas abertas");
    let list = rect(t.x + 8.0, t.y + 42.0, t.w - 16.0, t.h - 50.0);
    ui.begin_scroll("wins", list, app.windows.len() as f32 * 54.0);
    let mut toggle = None;
    for (i, wi) in app.windows.iter().enumerate() {
        let y = list.y + i as f32 * 54.0;
        let on = app.full.is_on(wi.hwnd);
        if on {
            ui.c.round(rect(list.x, y, list.w, 52.0), 8.0, CARD2);
        }
        let ic = rect(list.x + 8.0, y + 15.0, 22.0, 22.0);
        ui.c.round(ic, 6.0, LINE);
        let letter: String = wi.exe.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        ui.c.text(ic, &letter, 11.0, TEXT2, Align::Center, Weight::Semibold);
        ui.c.text(rect(list.x + 42.0, y + 6.0, list.w - 170.0, 20.0), &wi.title, 14.0, TEXT, Align::Left, Weight::Regular);
        let st = format!("{} · {} × {}", if on { "Sem bordas" } else { "Janela" }, wi.size.0, wi.size.1);
        ui.c.text(rect(list.x + 42.0, y + 27.0, list.w - 170.0, 18.0), &st, 12.0, MUTED, Align::Left, Weight::Regular);
        let label = if on { "Restaurar" } else { "Tela cheia" };
        if ui.button(&format!("fs{}", wi.hwnd.0 as isize), rect(list.right() - 112.0, y + 8.0, 104.0, 36.0), label, false) {
            toggle = Some(wi.hwnd);
        }
    }
    ui.end_scroll();
    if let Some(h) = toggle {
        app.full.toggle(h);
    }
}
