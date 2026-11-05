//! Reportar bug: a pessoa preenche aqui e o navegador abre a página do GitHub já escrita.

use super::*;
use crate::report::{Direct, Photo, View, CLIENT_ID, LEVELS};

const FOOT: f32 = 104.0;
const SIDE_COL: f32 = 280.0;
const MEANING: [&str; 3] = ["Incomoda, mas dá para usar.", "Atrapalha o uso, mas tem como contornar.", "Trava, fecha, perde dados ou impede o uso."];

/// Nomes das funções, na ordem da barra lateral. Termina em Outra coisa.
pub fn areas() -> Vec<&'static str> {
    NAV.iter().filter(|n| n.0 != Page::Report).map(|n| n.1).chain(["Outra coisa"]).collect()
}

/// Começa em quem chamou: quase sempre é a função com o problema.
pub fn area_of(page: Page) -> usize {
    NAV.iter().filter(|n| n.0 != Page::Report).position(|n| n.0 == page).unwrap_or(0)
}

fn photo_card(ui: &mut Ui, app: &mut App, r: Rect) {
    ui.card(r);
    let x = r.x + 14.0;
    let w = r.w - 28.0;
    ui.c.text(rect(x, r.y + 12.0, w, 20.0), "Foto (opcional)", 14.0, TEXT, Align::Left, Weight::Semibold);
    let bw = (w - 8.0) / 2.0;
    let by = r.bottom() - 52.0;
    if let Some(p) = &app.report.photo {
        let area = rect(x, r.y + 42.0, w, (by - r.y - 54.0).max(20.0));
        let (tw, th, px) = &p.thumb;
        let fit = (area.w / *tw as f32).min(area.h / *th as f32);
        let (dw, dh) = (*tw as f32 * fit, *th as f32 * fit);
        if let Some(b) = ui.c.bitmap(*tw, *th, px) {
            ui.c.draw_bitmap(&b, rect(area.x + (area.w - dw) / 2.0, area.y + (area.h - dh) / 2.0, dw, dh));
        }
        let size = format!("{} x {}", p.w, p.h);
        ui.c.text(rect(x, by - 20.0, w, 18.0), &size, 12.0, MUTED, Align::Center, Weight::Regular);
        if ui.button("bug-rm", rect(x, by, w, 38.0), "Tirar a foto", false) {
            app.report.photo = None;
            ui.s.needs_redraw = true;
        }
    } else {
        ui.c.text_wrap(rect(x, r.y + 42.0, w, (by - r.y - 54.0).max(20.0)), "Ajuda em bugs visuais. Use o atalho de foto do Prisma, copie e clique em Colar.", 13.0, MUTED, Weight::Regular);
        if ui.button("bug-pick", rect(x, by, bw, 38.0), "Escolher", false) {
            app.pick_report_photo();
        }
        if ui.button("bug-paste", rect(x + bw + 8.0, by, bw, 38.0), "Colar", false) {
            match crate::capture::clipboard_image() {
                Some((w, h, px)) => app.report.photo = Some(Photo::new(w, h, px)),
                None => app.toast.info("Não há imagem copiada"),
            }
            ui.s.needs_redraw = true;
        }
    }
}

fn form(ui: &mut Ui, app: &mut App, a: Rect) {
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let bottom = a.h - FOOT;
    let col = w - SIDE_COL - 12.0;
    let names: Vec<String> = areas().iter().map(|s| s.to_string()).collect();
    ui.label(rect(x, 84.0, col, 18.0), "Função com problema", TEXT2);
    let mut area = app.report.area;
    if ui.select("bug-area", rect(x, 106.0, 230.0, 38.0), &names, &mut area) {
        app.report.area = area;
    }
    let lx = x + 242.0;
    ui.label(rect(lx, 84.0, col - 242.0, 18.0), "Gravidade", TEXT2);
    let bw = ((col - 242.0 - 12.0) / 3.0).max(70.0);
    for (i, name) in LEVELS.iter().enumerate() {
        if ui.button(&format!("bug-l{i}"), rect(lx + i as f32 * (bw + 6.0), 106.0, bw, 38.0), name, app.report.level == i) {
            app.report.level = i;
            ui.s.needs_redraw = true;
        }
    }
    ui.c.text(rect(lx, 148.0, col - 242.0, 18.0), MEANING[app.report.level], 12.0, MUTED, Align::Left, Weight::Regular);

    ui.label(rect(x, 172.0, col, 18.0), "O que aconteceu? Conte o que fez, o que esperava e o que viu.", TEXT2);
    let field = rect(x, 194.0, col, bottom - 194.0);
    ui.c.round(field, 8.0, SIDE);
    ui.c.round_line(field, 8.0, BORDER, 1.0);
    let inner = field.inset_xy(10.0, 8.0);
    let (hwnd, scale) = app.paint_window;
    if app.report.text.is_none() {
        app.report.text = crate::ui::edit::TextBox::new(hwnd);
        if let Some(t) = &app.report.text {
            t.set_text(&app.report.draft);
        }
    }
    let covered = ui.s.covered();
    if let Some(t) = &mut app.report.text {
        app.report.draft = t.text();
        if covered {
            t.hide();
        } else {
            t.place(inner, scale);
        }
    }
    if covered {
        ui.c.text_wrap(inner, &app.report.draft, 14.0, TEXT, Weight::Regular);
    } else if ui.hit("bug-text", field) {
        if let Some(t) = &app.report.text {
            t.focus();
        }
    }

    photo_card(ui, app, rect(x + col + 12.0, 84.0, SIDE_COL, bottom - 84.0));

    let y = bottom + 12.0;
    let mut extras = app.report.extras;
    if ui.check("bug-extras", rect(x, y, 280.0, 36.0), "Incluir dados do PC e registro", &mut extras) {
        app.report.extras = extras;
    }
    if ui.button("bug-see", rect(x + 288.0, y, 170.0, 36.0), "Ver o que vai junto", false) {
        app.prepare_report();
        app.report.view = View::Preview;
        ui.reset_scroll("bug-prev");
        ui.s.needs_redraw = true;
    }
    ui.c.text_wrap(rect(x, y + 40.0, w - 400.0, 36.0), "A página do GitHub é pública. Nome de usuário, nome do PC e caminhos de arquivo não vão junto.", 12.0, MUTED, Weight::Regular);
    let ready = !app.report.draft.trim().is_empty();
    let sw = 200.0;
    if ui.button_ex("bug-send", rect(x + w - sw, y, sw, 40.0), "Enviar pelo navegador", true, ready) {
        app.prepare_report();
        app.send_report();
        ui.s.needs_redraw = true;
    }
    if !CLIENT_ID.is_empty() && ui.button_ex("bug-direct", rect(x + w - sw * 2.0 - 8.0, y, sw, 40.0), "Enviar direto", false, ready) {
        app.prepare_report();
        app.send_report_direct();
        ui.s.needs_redraw = true;
    }
}

fn preview(ui: &mut Ui, app: &mut App, a: Rect) {
    if ui.button("bug-back", rect(a.right() - 124.0, 30.0, 96.0, 36.0), "Voltar", false) {
        app.report.view = View::Form;
        ui.s.needs_redraw = true;
        return;
    }
    let r = rect(a.x + 28.0, 84.0, a.w - 56.0, a.h - 108.0);
    ui.card(r);
    let lines: Vec<String> = app.report.ready.as_ref().map(|d| std::iter::once(d.title.clone()).chain(std::iter::once(String::new())).chain(d.full.lines().map(str::to_string)).collect()).unwrap_or_default();
    log_view(ui, "bug-prev", r.inset(12.0), &lines);
}

/// Passos que dependem do que coube no endereço e da foto.
fn steps(short: bool, photo: bool) -> Vec<&'static str> {
    let mut s = vec!["Se o GitHub pedir, entre na sua conta. O texto continua lá depois."];
    if short {
        s.push("O texto era grande demais para o endereço: clique na descrição, selecione tudo e aperte Ctrl+V.");
    }
    if photo {
        s.push(if short { "Clique em Copiar a foto, depois clique no fim da descrição e aperte Ctrl+V." } else { "Clique no fim da descrição e aperte Ctrl+V: a foto já está copiada." });
    }
    s.push("Confira o texto e clique em Create no GitHub.");
    s
}

fn sent(ui: &mut Ui, app: &mut App, a: Rect) {
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let short = app.report.ready.as_ref().is_some_and(|r| r.short);
    let list = steps(short, app.report.photo.is_some());
    let card = rect(x, 84.0, w, 48.0 + list.len() as f32 * 40.0 + 72.0);
    ui.card(card);
    ui.c.text(rect(x + 20.0, 98.0, w - 40.0, 24.0), "Falta só confirmar no navegador", 16.0, TEXT, Align::Left, Weight::Semibold);
    for (i, s) in list.iter().enumerate() {
        let y = 134.0 + i as f32 * 40.0;
        ui.c.circle(x + 34.0, y + 15.0, 12.0, CARD2);
        ui.c.text(rect(x + 22.0, y + 3.0, 24.0, 24.0), &(i + 1).to_string(), 13.0, ACCENT, Align::Center, Weight::Semibold);
        ui.c.text(rect(x + 56.0, y, w - 76.0, 30.0), s, 14.0, TEXT2, Align::Left, Weight::Regular);
    }
    let by = card.bottom() - 56.0;
    let bw = 170.0;
    let mut bx = x + 20.0;
    if app.report.photo.is_some() {
        if ui.button("bug-cp", rect(bx, by, bw, 38.0), "Copiar a foto", false) {
            app.copy_report_photo();
        }
        bx += bw + 8.0;
    }
    if ui.button("bug-ct", rect(bx, by, bw, 38.0), "Copiar o relatório", false) {
        app.copy_report_text();
    }
    bx += bw + 8.0;
    if ui.button("bug-again", rect(bx, by, bw, 38.0), "Abrir de novo", false) {
        if let Some(r) = &app.report.ready {
            crate::report::open_url(&r.url);
        }
    }
    if ui.button("bug-edit", rect(x + w - 2.0 * bw - 8.0, card.bottom() + 12.0, bw, 38.0), "Voltar ao formulário", false) {
        app.report.view = View::Form;
        ui.s.needs_redraw = true;
    }
    if ui.button("bug-new", rect(x + w - bw, card.bottom() + 12.0, bw, 38.0), "Novo relatório", true) {
        app.report.reset();
        ui.s.needs_redraw = true;
    }
}

fn direct(ui: &mut Ui, app: &mut App, a: Rect) {
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let card = rect(x, 84.0, w, 230.0);
    ui.card(card);
    let state = app.report.direct.lock().unwrap().clone();
    let t = rect(x + 20.0, 104.0, w - 40.0, 24.0);
    let body = rect(x + 20.0, 136.0, w - 40.0, 60.0);
    match state {
        Direct::Idle | Direct::Asking => ui.c.text(t, "Falando com o GitHub...", 16.0, TEXT, Align::Left, Weight::Semibold),
        Direct::Code(code) => {
            ui.c.text(t, "Digite este código na página que abriu", 16.0, TEXT, Align::Left, Weight::Semibold);
            ui.c.text(rect(x + 20.0, 138.0, w - 40.0, 48.0), &code, 32.0, ACCENT, Align::Left, Weight::Semibold);
            ui.c.text_wrap(rect(x + 20.0, 196.0, w - 40.0, 40.0), "Depois de autorizar, o Prisma cria a página sozinho. A autorização vale só para este envio e não fica guardada.", 13.0, TEXT2, Weight::Regular);
        }
        Direct::Sending => ui.c.text(t, "Enviando...", 16.0, TEXT, Align::Left, Weight::Semibold),
        Direct::Done => {
            ui.c.text(t, "Relatório enviado. Obrigado!", 16.0, TEXT, Align::Left, Weight::Semibold);
            let text = if app.report.photo.is_some() { "A página abriu no navegador. A foto está copiada: clique no campo de comentário, aperte Ctrl+V e clique em Comment." } else { "A página abriu no navegador." };
            ui.c.text_wrap(body, text, 14.0, TEXT2, Weight::Regular);
        }
        Direct::Failed(e) => {
            ui.c.text(t, "Não deu para enviar direto", 16.0, WARN, Align::Left, Weight::Semibold);
            ui.c.text_wrap(body, &e, 14.0, TEXT2, Weight::Regular);
        }
    }
    let working = matches!(*app.report.direct.lock().unwrap(), Direct::Asking | Direct::Code(_) | Direct::Sending);
    if ui.button("bug-dback", rect(x + 20.0, card.bottom() - 54.0, 170.0, 38.0), if working { "Cancelar" } else { "Voltar ao formulário" }, false) {
        app.report.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        app.report.view = View::Form;
        ui.s.needs_redraw = true;
    }
    if matches!(*app.report.direct.lock().unwrap(), Direct::Done) && ui.button("bug-dnew", rect(x + 198.0, card.bottom() - 54.0, 170.0, 38.0), "Novo relatório", true) {
        app.report.reset();
        ui.s.needs_redraw = true;
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header(ui, a, "Reportar bug", "Preencha aqui. O navegador abre o GitHub com tudo escrito.");
    if app.report.view != View::Form {
        if let Some(t) = &mut app.report.text {
            t.hide();
        }
    }
    match app.report.view {
        View::Form => form(ui, app, a),
        View::Preview => preview(ui, app, a),
        View::Sent => sent(ui, app, a),
        View::Direct => direct(ui, app, a),
    }
}
