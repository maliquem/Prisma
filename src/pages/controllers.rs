//! Aba Controles (Fase 1, só leitura): cada controle ao vivo, bateria, taxa de relatório,
//! assistente de medida dos sticks e gatilhos, avisos e conflitos com outros programas.

use super::*;
use crate::sys::pads::{self, Battery, Family, Link, Pad};
use std::time::Instant;

/// Passos do assistente, em segundos: parado, girando os sticks, apertando os gatilhos.
const STEPS: [f32; 3] = [3.0, 6.0, 4.0];

#[derive(Default)]
pub struct State {
    sel: usize,
    wiz: Option<Wizard>,
    result: Option<(String, Results)>,
    conflicts: Vec<&'static str>,
    /// Editor de perfis aberto no lugar da coluna da direita.
    pub(super) edit: Option<super::pad_profiles::Editor>,
    /// Altura da coluna da esquerda no quadro anterior, para a rolagem.
    left_h: f32,
}

impl State {
    pub fn open_editor(&mut self) {
        self.edit = Some(Default::default());
    }
}

struct Wizard {
    key: String,
    started: Instant,
    rest: [Vec<(f32, f32)>; 2],
    turn: [Vec<(f32, f32)>; 2],
    trig_rest: [Vec<f32>; 2],
    trig: [Vec<f32>; 2],
}

struct Results {
    rest: [Option<(f32, f32)>; 2],
    outer: [Option<(f32, f32, f32)>; 2],
    trig: [Option<(f32, f32, f32)>; 2],
}

/// Programas que mexem no controle antes do jogo, com o conflito em uma frase.
const CONFLICTS: &[(&str, &str)] = &[
    ("steam.exe", "Steam aberto: o Steam Input pode trocar os botões que o jogo recebe."),
    ("rewasdengine.exe", "reWASD aberto: ele pode esconder ou remapear o controle."),
    ("ds4windows.exe", "DS4Windows aberto: ele pode esconder o controle e criar um Xbox virtual."),
    ("dsx.exe", "DSX aberto: ele pode mudar o modo do controle e criar um controle virtual."),
];

/// Ao abrir a aba: procura os programas que disputam o controle.
pub fn refresh(app: &mut App) {
    let run = crate::sys::procs::running_exes();
    app.pads.conflicts = CONFLICTS.iter().filter(|(exe, _)| run.contains(*exe)).map(|(_, t)| *t).collect();
}

/// A cada quadro com a aba aberta: o assistente junta as amostras do controle escolhido.
pub fn tick(app: &mut App) {
    let Some(w) = &mut app.pads.wiz else { return };
    let Some(p) = pads::list().into_iter().find(|p| p.key == w.key) else {
        app.pads.wiz = None;
        return;
    };
    let t = w.started.elapsed().as_secs_f32();
    let i = p.input;
    let sticks = [(i.lx, i.ly), (i.rx, i.ry)];
    let trig = [i.lt, i.rt];
    if t < STEPS[0] {
        for k in 0..2 {
            w.rest[k].push(sticks[k]);
            w.trig_rest[k].push(trig[k]);
        }
    } else if t < STEPS[0] + STEPS[1] {
        for k in 0..2 {
            w.turn[k].push(sticks[k]);
        }
    } else if t < STEPS.iter().sum::<f32>() {
        for k in 0..2 {
            w.trig[k].push(trig[k]);
        }
    } else {
        let r = Results {
            rest: [pads::rest(&w.rest[0]), pads::rest(&w.rest[1])],
            outer: [pads::outer(&w.turn[0]), pads::outer(&w.turn[1])],
            trig: [pads::trigger(&w.trig_rest[0], &w.trig[0]), pads::trigger(&w.trig_rest[1], &w.trig[1])],
        };
        app.pads.result = Some((w.key.clone(), r));
        app.pads.wiz = None;
    }
}

fn pct(v: f32) -> String {
    format!("{:.1}%", v * 100.0).replace('.', crate::i18n::comma())
}

fn ms(v: f32) -> String {
    format!("{:.1} ms", v).replace('.', crate::i18n::comma())
}

fn link_text(l: Link) -> &'static str {
    match l {
        Link::Usb => "USB",
        Link::Bluetooth => "Bluetooth",
    }
}

pub fn battery_label(p: &Pad) -> String {
    format!("{} {}", crate::i18n::tr("bateria"), battery_text(p))
}

fn battery_text(p: &Pad) -> String {
    let tr = |s: &str| crate::i18n::tr(s).into_owned();
    match p.battery {
        Battery::Percent { pct, charging: true } => format!("{pct}% {}", tr("carregando")),
        Battery::Percent { pct, .. } => format!("{pct}%"),
        Battery::Wired => tr("com fio"),
        Battery::Level(0) => tr("vazia"),
        Battery::Level(1) => tr("baixa"),
        Battery::Level(2) => tr("pela metade"),
        Battery::Level(_) => tr("cheia"),
        Battery::Unknown => tr("desconhecida"),
    }
}

fn rate_text(p: &Pad) -> String {
    match p.rate {
        Some(r) if r.counted => format!("{:.0} Hz {}", r.hz, crate::i18n::tr("em movimento")),
        Some(r) => format!("{:.0} Hz · p99 {}", r.hz, ms(r.p99_ms)),
        None => crate::i18n::tr("taxa a medir").into_owned(),
    }
}

/// Nomes dos botões no desenho de cada família, com o bit comum.
pub(super) fn button_names(f: Family) -> &'static [(u32, &'static str)] {
    use pads::*;
    match f {
        Family::DualSense => &[
            (A, "Cruz"),
            (B, "Círculo"),
            (X, "Quadrado"),
            (Y, "Triângulo"),
            (LB, "L1"),
            (RB, "R1"),
            (LS, "L3"),
            (RS, "R3"),
            (BACK, "Create"),
            (START, "Options"),
            (GUIDE, "PS"),
            (PAD, "Touchpad"),
            (MUTE, "Mudo"),
        ],
        Family::Ds4 => &[
            (A, "Cruz"),
            (B, "Círculo"),
            (X, "Quadrado"),
            (Y, "Triângulo"),
            (LB, "L1"),
            (RB, "R1"),
            (LS, "L3"),
            (RS, "R3"),
            (BACK, "Share"),
            (START, "Options"),
            (GUIDE, "PS"),
            (PAD, "Touchpad"),
        ],
        _ => &[(A, "A"), (B, "B"), (X, "X"), (Y, "Y"), (LB, "LB"), (RB, "RB"), (LS, "LS"), (RS, "RS"), (BACK, "View"), (START, "Menu"), (GUIDE, "Xbox")],
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    header_room(ui, a, "Controles", "Cada controle ao vivo, e o controle como teclado e mouse. Nada é gravado no controle.", 220.0);
    let list = pads::list();
    if ui.button("pads-copy", rect(a.right() - 208.0, 28.0, 180.0, 36.0), "Copiar diagnóstico", false) {
        let ok = crate::capture::copy_text(app.hub, &diagnostic(&list, &app.pads.conflicts));
        app.toast.info(if ok { "Diagnóstico copiado" } else { "Não consegui copiar" });
    }
    let x = a.x + 28.0;
    let top = 88.0;
    let lw = 300.0;
    let rw = a.w - 56.0 - lw - 12.0;

    // Coluna esquerda: controles, avisos, conflitos. Rola: o cartão do radial passa da janela.
    ui.begin_scroll("pads-left", rect(x, top, lw, a.h - top - 12.0), app.pads.left_h);
    let mut y = top;
    if list.is_empty() {
        let r = rect(x, y, lw, 96.0);
        ui.card(r);
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 14.0, r.w - 32.0, r.h - 24.0), "Nenhum controle ligado. Ligue um controle por USB ou Bluetooth e ele aparece aqui.", 13.0, MUTED, Weight::Regular);
        y += r.h + 12.0;
    }
    app.pads.sel = app.pads.sel.min(list.len().saturating_sub(1));
    for (i, p) in list.iter().enumerate() {
        let r = rect(x, y, lw, 64.0);
        let on = i == app.pads.sel;
        if on {
            ui.card_on(r);
        } else {
            ui.card(r);
        }
        ui.c.text_fit(rect(r.x + 16.0, r.y + 10.0, r.w - 32.0, 22.0), &p.name, 15.0, TEXT, Align::Left, Weight::Semibold);
        let sub = format!("{} · {} · {}", crate::i18n::tr(link_text(p.link)), battery_text(p), rate_text(p));
        ui.c.text_fit(rect(r.x + 16.0, r.y + 34.0, r.w - 32.0, 20.0), &sub, 13.0, MUTED, Align::Left, Weight::Regular);
        if ui.hit(&format!("pad-{}", p.key), r) && !on {
            app.pads.sel = i;
            ui.s.needs_redraw = true;
        }
        y += r.h + 8.0;
    }
    y += 4.0;
    for name in pads::without_hid() {
        let r = rect(x, y, lw, 122.0);
        ui.card(r);
        ui.c.text_fit(rect(r.x + 16.0, r.y + 10.0, r.w - 32.0, 20.0), &name, 14.0, WARN, Align::Left, Weight::Semibold);
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 34.0, r.w - 32.0, 80.0), "Pareado sem a parte de controle: nenhum programa consegue ler. Remova o controle e pareie de novo pelo assistente clássico (Win + R, devicepairingwizard).", 12.0, TEXT2, Weight::Regular);
        y += r.h + 8.0;
    }
    let r = rect(x, y, lw, 176.0);
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Avisos");
    let mut changed = ui.check("pad-notify", rect(r.x + 10.0, r.y + 38.0, r.w - 20.0, 36.0), "Ao conectar e desconectar", &mut app.cfg.pad_notify);
    ui.label(rect(r.x + 16.0, r.y + 80.0, r.w - 156.0, 36.0), "Bateria baixa", TEXT2);
    let mut low = app.cfg.pad_low as u32;
    if ui.stepper("pad-low", rect(r.right() - 140.0, r.y + 80.0, 124.0, 36.0), &mut low, (0, 50), 5, "%") {
        app.cfg.pad_low = low as u8;
        changed = true;
    }
    ui.label(rect(r.x + 16.0, r.y + 124.0, r.w - 156.0, 36.0), "Variação acima de", TEXT2);
    changed |= ui.stepper("pad-jitter", rect(r.right() - 140.0, r.y + 124.0, 124.0, 36.0), &mut app.cfg.pad_jitter_ms, (0, 100), 1, "ms");
    ui.tip(rect(r.x + 16.0, r.y + 124.0, r.w - 156.0, 36.0), "Avisa quando o intervalo entre relatórios do controle passa desse valor (p99). 0 desliga.");
    if changed {
        app.cfg.save();
        app.apply_pads();
    }
    y += r.h + 12.0;
    y = mapping(ui, app, rect(x, y, lw, 0.0));
    y = radial(ui, app, rect(x, y, lw, 0.0));
    for t in &app.pads.conflicts {
        let r = rect(x, y, lw, 64.0);
        ui.card(r);
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 10.0, r.w - 32.0, r.h - 16.0), t, 12.0, TEXT2, Weight::Regular);
        y += r.h + 8.0;
    }
    ui.end_scroll();
    if (y - top - app.pads.left_h).abs() > 0.5 {
        app.pads.left_h = y - top;
        ui.s.needs_redraw = true;
    }

    // Coluna direita: o editor de perfis, ou o controle escolhido.
    let rx = x + lw + 12.0;
    if app.pads.edit.is_some() {
        let family = list.get(app.pads.sel).map_or(Family::Xbox, |p| p.family);
        super::pad_profiles::draw(ui, app, rect(rx, top, rw, a.h - top - 24.0), family);
        return;
    }
    let Some(p) = list.get(app.pads.sel) else { return };
    let live = rect(rx, top, rw, 300.0);
    ui.card(live);
    draw_live(ui, p, live);

    let m = rect(rx, live.bottom() + 12.0, rw, (a.h - live.bottom() - 36.0).max(160.0));
    ui.card(m);
    ui.title(rect(m.x + 16.0, m.y + 12.0, m.w - 220.0, 22.0), "Medir sticks e gatilhos");
    let running = app.pads.wiz.is_some();
    if ui.button_ex("pad-measure", rect(m.right() - 196.0, m.y + 8.0, 180.0, 34.0), if running { "Medindo…" } else { "Começar" }, true, !running) {
        app.pads.result = None;
        app.pads.wiz = Some(Wizard { key: p.key.clone(), started: Instant::now(), rest: Default::default(), turn: Default::default(), trig_rest: Default::default(), trig: Default::default() });
    }
    let body = rect(m.x + 16.0, m.y + 48.0, m.w - 32.0, m.h - 60.0);
    if let Some(w) = &app.pads.wiz {
        let t = w.started.elapsed().as_secs_f32();
        let (step, text) = if t < STEPS[0] {
            (0, "Solte os sticks e os gatilhos e não toque no controle.")
        } else if t < STEPS[0] + STEPS[1] {
            (1, "Gire os dois sticks devagar, encostando na borda, algumas voltas.")
        } else {
            (2, "Aperte os dois gatilhos até o fim algumas vezes.")
        };
        ui.c.text_wrap(rect(body.x, body.y, body.w, 40.0), text, 14.0, TEXT, Weight::Regular);
        let done = (t / STEPS.iter().sum::<f32>()).min(1.0);
        ui.c.round(rect(body.x, body.y + 50.0, body.w, 6.0), 3.0, BORDER);
        ui.c.round(rect(body.x, body.y + 50.0, body.w * done, 6.0), 3.0, ACCENT);
        ui.label(rect(body.x, body.y + 62.0, body.w, 20.0), &format!("{} {} / 3", crate::i18n::tr("Etapa"), step + 1), MUTED);
    } else if let Some((_, r)) = app.pads.result.as_ref().filter(|(k, _)| *k == p.key) {
        draw_results(ui, r, body);
    } else {
        ui.c.text_wrap(rect(body.x, body.y, body.w, 60.0), "Mede o desvio dos sticks parados, a tremida, o alcance, o quanto o contorno foge de um círculo e o curso dos gatilhos. Leva 13 segundos.", 13.0, MUTED, Weight::Regular);
    }
}

fn draw_live(ui: &mut Ui, p: &Pad, r: Rect) {
    let i = p.input;
    let line = format!("{} · {} · {}: {}", crate::i18n::tr(link_text(p.link)), rate_text(p), crate::i18n::tr("bateria"), battery_text(p));
    ui.c.text_fit(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 20.0), &line, 13.0, TEXT2, Align::Left, Weight::Regular);
    if let Some(rt) = p.rate.filter(|r| !r.counted) {
        let detail = format!("{} {} · p99 {} · {} {}", crate::i18n::tr("média"), ms(rt.avg_ms), ms(rt.p99_ms), crate::i18n::tr("máx"), ms(rt.max_ms));
        ui.c.text_fit(rect(r.x + 16.0, r.y + 32.0, r.w - 32.0, 18.0), &detail, 12.0, MUTED, Align::Left, Weight::Regular);
    }
    // Sticks e gatilhos.
    let rad = 46.0;
    let cy = r.y + 62.0 + rad;
    for (k, (sx, sy)) in [(i.lx, i.ly), (i.rx, i.ry)].into_iter().enumerate() {
        let cx = r.x + 30.0 + rad + k as f32 * (2.0 * rad + 70.0);
        ui.c.circle(cx, cy, rad, CARD2);
        ui.c.circle_line(cx, cy, rad, BORDER, 1.5);
        ui.c.line(cx - rad, cy, cx + rad, cy, BORDER, 1.0);
        ui.c.line(cx, cy - rad, cx, cy + rad, BORDER, 1.0);
        ui.c.circle(cx + sx.clamp(-1.0, 1.0) * rad, cy - sy.clamp(-1.0, 1.0) * rad, 6.0, ACCENT);
        let tv = if k == 0 { i.lt } else { i.rt };
        let bar = rect(cx + rad + 12.0, cy - rad, 12.0, 2.0 * rad);
        ui.c.round(bar, 4.0, CARD2);
        ui.c.round(rect(bar.x, bar.bottom() - bar.h * tv, bar.w, bar.h * tv), 4.0, ACCENT);
        let name = match (p.family, k) {
            (Family::DualSense | Family::Ds4, 0) => "L2",
            (Family::DualSense | Family::Ds4, _) => "R2",
            (_, 0) => "LT",
            _ => "RT",
        };
        ui.c.text_raw(rect(bar.x - 10.0, bar.bottom() + 4.0, 32.0, 16.0), name, 11.0, MUTED, Align::Center, Weight::Regular);
    }
    // Botões: os do desenho da família, ou numerados no controle genérico, e o direcional.
    let mut chips: Vec<(bool, String)> = [(pads::UP, "↑"), (pads::DOWN, "↓"), (pads::LEFT, "←"), (pads::RIGHT, "→")].iter().map(|(b, n)| (i.buttons & b != 0, n.to_string())).collect();
    if p.family == Family::Generic {
        chips.extend((0..p.generic_buttons.min(10)).map(|n| (i.buttons & (1 << n) != 0, (n + 1).to_string())));
    } else {
        chips.extend(button_names(p.family).iter().map(|(b, n)| (i.buttons & b != 0, crate::i18n::tr(n).into_owned())));
    }
    let (mut bx, mut by) = (r.x + 16.0, cy + rad + 32.0);
    for (on, label) in chips {
        let w = measure(&label, 12.0, Weight::Semibold, None).0 + 20.0;
        if bx + w > r.right() - 16.0 {
            bx = r.x + 16.0;
            by += 30.0;
        }
        if by + 24.0 > r.bottom() - 8.0 {
            break;
        }
        let c = rect(bx, by, w, 24.0);
        ui.c.round(c, 6.0, if on { ACCENT } else { CARD2 });
        ui.c.text_raw(c, &label, 12.0, if on { ON_ACCENT } else { TEXT2 }, Align::Center, Weight::Semibold);
        bx += w + 6.0;
    }
}

fn draw_results(ui: &mut Ui, r: &Results, b: Rect) {
    let col = (b.w - 12.0) / 3.0;
    for k in 0..2 {
        let x = b.x + k as f32 * (col + 6.0);
        ui.label(rect(x, b.y, col, 20.0), if k == 0 { "Stick esquerdo" } else { "Stick direito" }, TEXT);
        let mut rows: Vec<(&str, String, bool)> = vec![];
        match r.rest[k] {
            Some((drift, noise)) => {
                rows.push(("Desvio parado", pct(drift), drift > 0.08));
                rows.push(("Tremida", pct(noise), noise > 0.02));
            }
            None => rows.push(("Em repouso", "—".into(), false)),
        }
        match r.outer[k] {
            Some((cover, reach, round)) => {
                rows.push(("Alcance", pct(reach), reach < 0.9));
                rows.push(("Fora do círculo", pct(round), false));
                rows.push(("Voltas cobertas", pct(cover), cover < 0.9));
            }
            None => rows.push(("Gire mais para medir", String::new(), true)),
        }
        rows_draw(ui, x, b.y + 24.0, col, &rows);
    }
    let x = b.x + 2.0 * (col + 6.0);
    ui.label(rect(x, b.y, col, 20.0), "Gatilhos", TEXT);
    let mut rows = vec![];
    for (k, name) in ["Esquerdo", "Direito"].into_iter().enumerate() {
        match r.trig[k] {
            Some((start, end, noise)) => {
                rows.push((name, format!("{} → {}", pct(start), pct(end)), start > 0.05 || end < 0.95));
                rows.push(("Tremida", pct(noise), noise > 0.02));
            }
            None => rows.push((name, "—".into(), false)),
        }
    }
    rows_draw(ui, x, b.y + 24.0, col, &rows);
}

fn rows_draw(ui: &mut Ui, x: f32, y: f32, w: f32, rows: &[(&str, String, bool)]) {
    for (n, (label, value, warn)) in rows.iter().enumerate() {
        let ry = y + n as f32 * 22.0;
        let vw = 110.0f32.min(w * 0.5);
        ui.c.text_fit(rect(x, ry, w - vw - 4.0, 20.0), label, 12.0, if *warn && value.is_empty() { WARN } else { MUTED }, Align::Left, Weight::Regular);
        ui.c.text_raw(rect(x + w - vw, ry, vw, 20.0), value, 12.0, if *warn { WARN } else { TEXT }, Align::Right, Weight::Semibold);
    }
}

/// Texto para colar num relatório: controles, conexão, taxa, bateria, conflitos.
/// Mapeamento: liga e desliga, perfil ativo, parada de emergência e o tempo interno.
fn mapping(ui: &mut Ui, app: &mut App, at: Rect) -> f32 {
    use crate::sys::remap;
    let r = rect(at.x, at.y, at.w, 262.0);
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Mapeamento");
    let check = rect(r.x + 10.0, r.y + 38.0, r.w - 20.0, 36.0);
    if ui.check("pad-map", check, "Controle como teclado e mouse", &mut app.cfg.pad_map) {
        app.cfg.save();
        app.apply_pads();
    }
    ui.tip(check, "Fora dos jogos vale o perfil Desktop: o stick esquerdo mexe o mouse, A e B clicam. Nos jogos (os do Medidor e qualquer programa em tela cheia) vale o perfil Base, que não muda nada: sem driver, o jogo continua vendo o controle.");
    let state = match (app.cfg.pad_map, remap::stopped(), remap::active()) {
        (false, ..) => (crate::i18n::tr("Desligado").into_owned(), MUTED),
        (true, true, _) => (crate::i18n::tr("Parado pela combinação").into_owned(), WARN),
        (true, false, p) => (crate::i18n::tr("Perfil {}").replace("{}", &crate::i18n::tr(&p.unwrap_or_default())), TEXT),
    };
    ui.c.text_fit(rect(r.x + 16.0, r.y + 80.0, r.w - 32.0, 20.0), &state.0, 13.0, state.1, Align::Left, Weight::Semibold);
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 106.0, r.w - 32.0, 72.0), "Para parar tudo sem teclado, segure Guia (ou PS) + Select + Start por 2 segundos. A mesma combinação liga de novo.", 12.0, TEXT2, Weight::Regular);
    if let Some((med, p99)) = remap::latency() {
        let t = crate::i18n::tr("Tempo interno: {} µs na mediana, {} µs no p99").replacen("{}", &med.to_string(), 1).replacen("{}", &p99.to_string(), 1);
        ui.c.text_fit(rect(r.x + 16.0, r.y + 184.0, r.w - 32.0, 20.0), &t, 12.0, MUTED, Align::Left, Weight::Regular);
    }
    let editing = app.pads.edit.is_some();
    if ui.button_ex("pad-edit", rect(r.x + 16.0, r.y + 212.0, r.w - 32.0, 36.0), if editing { "Fechar perfis" } else { "Editar perfis" }, false, true) {
        if editing {
            app.pads.edit = None;
        } else {
            app.pads.open_editor();
        }
    }
    r.bottom() + 12.0
}

/// Radial do Prisma: as 8 ações, de cima no sentido do relógio.
fn radial(ui: &mut Ui, app: &mut App, at: Rect) -> f32 {
    use super::picker::Target;
    use crate::sys::radial::{EMPTY, PROGRAM};
    use crate::sys::remap;
    const DIRS: [&str; 8] = ["↑", "↗", "→", "↘", "↓", "↙", "←", "↖"];
    let two = app.cfg.pad_radial_two;
    let r = rect(at.x, at.y, at.w, if two { 1210.0 } else { 870.0 });
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Radial do Prisma");
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 40.0, r.w - 32.0, 58.0), "O stick direito aponta. O radial fica aberto até Confirmar disparar ou Voltar fechar.", 12.0, TEXT2, Weight::Regular);
    let tr = |s: &str| crate::i18n::tr(s).into_owned();
    let mut changed = false;
    let row = |i: usize| (rect(r.x + 16.0, r.y + 104.0 + i as f32 * 42.0, 108.0, 34.0), rect(r.x + 128.0, r.y + 104.0 + i as f32 * 42.0, r.w - 144.0, 34.0));
    let (l, c) = row(0);
    ui.c.text_fit(l, &tr("Botão"), 12.0, TEXT2, Align::Left, Weight::Regular);
    let mut b = app.cfg.pad_radial_btn.min(2) as usize;
    if ui.select("pad-radial-btn", c, &[tr("Nenhum"), "Select".into(), tr("Guia")], &mut b) {
        app.cfg.pad_radial_btn = b as u8;
        changed = true;
    }
    ui.tip(rect(l.x, l.y, r.w - 32.0, 34.0), "Vale com o controle como teclado e mouse ligado. Nos jogos (perfil Base) o Select fica só com o jogo; o Guia abre em qualquer perfil. Também dá para ligar qualquer botão de um perfil a Radial do Prisma.");
    let (l, c) = row(1);
    ui.c.text_fit(l, &tr("Abrir com"), 12.0, TEXT2, Align::Left, Weight::Regular);
    let mut t = app.cfg.pad_radial_tap as usize;
    if ui.select("pad-radial-tap", c, &[tr("Segurar"), tr("Toque")], &mut t) {
        app.cfg.pad_radial_tap = t == 1;
        changed = true;
    }
    ui.tip(rect(l.x, l.y, r.w - 32.0, 34.0), "O outro jeito faz a função normal do botão.");
    let (l, _) = row(2);
    ui.c.text_fit(rect(l.x, l.y, r.w - 172.0, 34.0), &tr("Segurar por"), 12.0, TEXT2, Align::Left, Weight::Regular);
    if ui.stepper("pad-radial-ms", rect(r.right() - 140.0, l.y, 124.0, 34.0), &mut app.cfg.pad_radial_ms, (200, 2000), 50, "ms") {
        changed = true;
    }
    // Confirmar e Voltar: escolher no outro o botão de um troca os dois.
    let f = pads::list().get(app.pads.sel).map_or(Family::Xbox, |p| p.family);
    let btns: Vec<String> = crate::sys::radial::PICK_BUTTONS.iter().map(|&b| super::pad_profiles::button_label(f, b)).collect();
    for (i, key, label) in [(3, "pad-radial-ok", "Confirmar"), (4, "pad-radial-back", "Voltar")] {
        let (l, c) = row(i);
        ui.c.text_fit(l, &tr(label), 12.0, TEXT2, Align::Left, Weight::Regular);
        let (mine, other) = if i == 3 { (app.cfg.pad_radial_ok, app.cfg.pad_radial_back) } else { (app.cfg.pad_radial_back, app.cfg.pad_radial_ok) };
        let mut v = mine.min(5) as usize;
        if ui.select(key, c, &btns, &mut v) && v as u8 != mine {
            let (m, o) = (v as u8, if v as u8 == other { mine } else { other });
            (app.cfg.pad_radial_ok, app.cfg.pad_radial_back) = if i == 3 { (m, o) } else { (o, m) };
            changed = true;
        }
    }
    ui.tip(rect(r.x + 16.0, r.y + 230.0, r.w - 32.0, 76.0), "O radial fica aberto até Confirmar disparar o lugar escolhido ou Voltar fechar sem escolher.");
    let check = rect(r.x + 10.0, r.y + 314.0, r.w - 20.0, 36.0);
    if ui.check("pad-radial-keep", check, "A escolha fica com o stick no centro", &mut app.cfg.pad_radial_keep) {
        changed = true;
    }
    ui.tip(check, "Soltar o stick não desfaz a escolha: o último lugar apontado continua marcado até apontar outro.");
    let check = rect(r.x + 10.0, r.y + 354.0, r.w - 20.0, 36.0);
    let mut guide = app.cfg.pad_guide;
    if ui.check("pad-guide", check, "O Prisma cuida do botão Guia", &mut guide) {
        if guide {
            app.cfg.pad_gamebar_prev = crate::sys::radial::gamebar_take();
        } else {
            crate::sys::radial::gamebar_restore(app.cfg.pad_gamebar_prev);
            app.cfg.pad_gamebar_prev = -1;
        }
        app.cfg.pad_guide = guide;
        changed = true;
    }
    ui.tip(check, "A Xbox Game Bar deixa de abrir pelo botão Guia do controle, e o Prisma faz a função normal dele (Win + G). Desligar devolve a opção da Game Bar como estava.");
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 392.0, r.w - 32.0, 46.0), "Com o Steam aberto, desligue nele a opção do botão Guia.", 12.0, MUTED, Weight::Regular);
    let check = rect(r.x + 10.0, r.y + 438.0, r.w - 20.0, 36.0);
    if ui.check("pad-radial-two", check, "Dois círculos (16 lugares)", &mut app.cfg.pad_radial_two) {
        changed = true;
    }
    ui.tip(check, "Um círculo em cada stick, com 8 lugares cada. O stick que apontar por último escolhe.");
    // Nada, e todas as ações menos o próprio radial.
    let ids: Vec<u8> = std::iter::once(EMPTY).chain((0..remap::ACTIONS.len() as u8).filter(|&a| a != remap::RADIAL)).collect();
    let names: Vec<String> = ids.iter().map(|&a| crate::i18n::tr(remap::ACTIONS.get(a as usize).copied().unwrap_or("Nada")).into_owned()).collect();
    // Lugares 0 a 7 no stick direito; com dois círculos, 8 a 15 no esquerdo.
    // Um círculo: escolher o stick que aponta; dois: um título para cada stick.
    if !two {
        let (l, c) = row(9);
        ui.c.text_fit(l, &tr("Stick"), 12.0, TEXT2, Align::Left, Weight::Regular);
        let mut s = app.cfg.pad_radial_left as usize;
        if ui.select("pad-radial-left", c, &[tr("Direito"), tr("Esquerdo")], &mut s) {
            app.cfg.pad_radial_left = s == 1;
            changed = true;
        }
    }
    for i in 0..if two { 16 } else { 8 } {
        let top = r.y + if two { 480.0 + (i / 8) as f32 * 360.0 } else { 502.0 };
        if two && i % 8 == 0 {
            ui.c.text_fit(rect(r.x + 16.0, top, r.w - 32.0, 20.0), &tr(if i == 0 { "Stick direito" } else { "Stick esquerdo" }), 12.0, MUTED, Align::Left, Weight::Semibold);
        }
        // Uma escolha por linha: em duas colunas os nomes das ações não cabiam.
        let (cx, cy) = (r.x + 16.0, top + 22.0 + (i % 8) as f32 * 42.0);
        ui.c.text_raw(rect(cx, cy, 22.0, 34.0), DIRS[i % 8], 16.0, MUTED, Align::Left, Weight::Semibold);
        // No fim: o programa deste lugar (se houver) e escolher um programa. O lugar só muda quando a escolha volta.
        let prog = app.cfg.pad_radial[i] == PROGRAM;
        let mut opts = names.clone();
        if prog {
            opts.push(app.cfg.pad_radial_prog[i].0.clone());
        }
        let pick_at = opts.len();
        opts.push(tr(if prog { "Escolher outro programa…" } else { "Abrir um programa…" }));
        let mut sel = if prog { pick_at - 1 } else { ids.iter().position(|&a| a == app.cfg.pad_radial[i]).unwrap_or(0) };
        let menu = format!("pad-radial-run-{i}");
        if ui.select(&format!("pad-radial-{i}"), rect(cx + 24.0, cy, r.w - 56.0, 34.0), &opts, &mut sel) {
            if sel == pick_at {
                ui.menu(&menu, &[("Da lista de programas", true), ("Escolher arquivo .exe", true)]);
            } else if sel < ids.len() {
                app.cfg.pad_radial[i] = ids[sel];
                changed = true;
            }
        }
        match ui.menu_pick(&menu) {
            Some(0) => app.open_picker(Target::RadialProgram(i as u8)),
            Some(_) => app.pick_exe_for(Target::RadialProgram(i as u8)),
            None => {}
        }
    }
    if changed {
        app.cfg.save();
        app.apply_pads();
    }
    r.bottom() + 12.0
}

/// Programa escolhido para o lugar `i` do radial (lista de programas ou arquivo .exe).
pub fn set_radial_program(app: &mut App, i: usize, path: &str) {
    let exe = crate::sys::procs::exe_name(path);
    let stem = exe.rsplit_once('.').map_or(exe.as_str(), |(s, _)| s).to_string();
    let name = crate::sys::fileinfo::description(path).filter(|d| !d.trim().is_empty()).unwrap_or(stem);
    // `|` e TAB separam os lugares no arquivo de configuração.
    let name = name.replace(['|', '\t'], " ").trim().to_string();
    app.cfg.pad_radial[i] = crate::sys::radial::PROGRAM;
    app.cfg.pad_radial_prog[i] = (name, path.to_string());
    app.cfg.save();
    app.apply_pads();
}

fn diagnostic(list: &[Pad], conflicts: &[&str]) -> String {
    use std::fmt::Write;
    let mut s = format!("Prisma {} · Controles\n", crate::sys::update::current());
    if list.is_empty() {
        s.push_str("Nenhum controle ligado.\n");
    }
    for p in list {
        let write = pads::WRITE_OK.iter().any(|(v, pid, _)| *v == p.vid && *pid == p.pid);
        let _ = writeln!(s, "- {} ({:?}) {:04x}:{:04x} · {} · bateria {} · {}", p.name, p.family, p.vid, p.pid, link_text(p.link), battery_text(p), rate_text(p));
        if let Some(r) = p.rate.filter(|r| !r.counted) {
            let _ = writeln!(s, "  média {:.2} ms · p99 {:.2} ms · máx {:.2} ms", r.avg_ms, r.p99_ms, r.max_ms);
        }
        let _ = writeln!(s, "  escrita futura permitida: {}", if write { "sim" } else { "não" });
    }
    for n in pads::without_hid() {
        let _ = writeln!(s, "- {n}: pareado por Bluetooth sem o serviço de controle (HID)");
    }
    for c in conflicts {
        let _ = writeln!(s, "! {c}");
    }
    let map = match (crate::sys::remap::enabled(), crate::sys::remap::stopped()) {
        (false, _) => "desligado".to_string(),
        (true, true) => "parado pela combinação".to_string(),
        (true, false) => format!("perfil {}", crate::sys::remap::active().unwrap_or_default()),
    };
    let _ = writeln!(s, "Mapeamento: {map}");
    if let Some((med, p99)) = crate::sys::remap::latency() {
        let _ = writeln!(s, "  tempo interno: mediana {med} µs, p99 {p99} µs");
    }
    s
}
