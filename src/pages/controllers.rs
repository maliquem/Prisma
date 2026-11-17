//! Aba Controles (Fase 1, só leitura): cada controle ao vivo, bateria, taxa de relatório,
//! assistente de medida dos sticks e gatilhos, avisos e conflitos com outros programas.

use super::*;
use crate::sys::pads::{self, Battery, Family, Link, Pad};
use crate::sys::remap;
use crate::sys::xbridge;
use std::time::Instant;

/// A medição dos sticks está rodando (a navegação do Prisma fica pausada).
pub fn measuring(s: &State) -> bool {
    s.wiz.is_some() || s.testing
}

/// Passos do assistente, em segundos: parado, girando os sticks, apertando os gatilhos.
const STEPS: [f32; 3] = [3.0, 6.0, 4.0];

#[derive(Default)]
pub struct State {
    sel: usize,
    wiz: Option<Wizard>,
    result: Option<(String, Results)>,
    conflicts: Vec<&'static str>,
    /// Editor de perfis: abre na primeira vez que a página é desenhada.
    pub(super) edit: Option<super::pad_profiles::Editor>,
    /// Aba aberta: 0 Mapeamento, 1 Macros, 2 Ajustes.
    pub(super) tab: usize,
    /// Altura da coluna da esquerda de Ajustes no quadro anterior, para a rolagem.
    left_h: f32,
    /// O mesmo para a aba do Menu radial.
    radial_h: f32,
    /// O mesmo para a coluna do desenho, na aba Mapeamento.
    art_h: f32,
    test: Test,
    /// Teste dos botões ligado: o controle para de navegar pelo Prisma.
    testing: bool,
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

/// Teste embaixo do desenho: os botões que já responderam e quanto durou cada aperto.
#[derive(Default)]
struct Test {
    key: String,
    prev: pads::Buttons,
    seen: pads::Buttons,
    down: Vec<(pads::Buttons, Instant)>,
    up: Vec<(pads::Buttons, Instant)>,
    /// Do mais novo ao mais velho: botão, duração em ms e se veio colado ao aperto anterior.
    hist: Vec<(pads::Buttons, u32, bool)>,
}

/// Soltar e apertar de novo em menos que isso é aperto duplo: o botão pode estar falhando.
const DOUBLE_MS: u128 = 60;
const HIST: usize = 6;

/// Um relatório do controle `key`: os gatilhos contam como botão passando da metade.
fn test_feed(t: &mut Test, key: &str, i: pads::Input, now: Instant) {
    if t.key != key {
        *t = Test { key: key.to_string(), ..Default::default() };
    }
    let mut cur = i.buttons;
    if i.lt > 0.5 {
        cur |= remap::LT;
    }
    if i.rt > 0.5 {
        cur |= remap::RT;
    }
    for n in 0..32 {
        let bit: pads::Buttons = 1 << n;
        let (was, is) = (t.prev & bit != 0, cur & bit != 0);
        if is && !was {
            t.seen |= bit;
            t.down.push((bit, now));
        } else if was && !is {
            let Some(k) = t.down.iter().position(|d| d.0 == bit) else { continue };
            let (_, at) = t.down.remove(k);
            let double = t.up.iter().any(|u| u.0 == bit && at.saturating_duration_since(u.1).as_millis() < DOUBLE_MS);
            t.up.retain(|u| u.0 != bit);
            t.up.push((bit, now));
            t.hist.insert(0, (bit, now.saturating_duration_since(at).as_millis() as u32, double));
            t.hist.truncate(HIST);
        }
    }
    t.prev = cur;
}

/// A cada quadro com a aba aberta: o teste e o assistente leem o controle escolhido.
pub fn tick(app: &mut App) {
    if let Some(p) = pads::list().get(app.pads.sel) {
        test_feed(&mut app.pads.test, &p.key, p.input, Instant::now());
    }
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
pub(super) fn button_names(f: Family) -> &'static [(pads::Buttons, &'static str)] {
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
        // Nomes impressos no 8BitDo (letras na posição da Nintendo).
        Family::EightBitDo => &[
            (A, "B"),
            (B, "A"),
            (X, "Y"),
            (Y, "X"),
            (LB, "L"),
            (RB, "R"),
            (LS, "L3"),
            (RS, "R3"),
            (BACK, "Select"),
            (START, "Start"),
            (GUIDE, "Home"),
            (L4, "L4"),
            (R4, "R4"),
            (PL, "PL"),
            (PR, "PR"),
        ],
        // Nomes do Pro Controller do Switch.
        Family::Switch => &[(A, "B"), (B, "A"), (X, "Y"), (Y, "X"), (LB, "L"), (RB, "R"), (LS, "L3"), (RS, "R3"), (BACK, "-"), (START, "+"), (GUIDE, "Home"), (PAD, "Capture")],
        _ => &[(A, "A"), (B, "B"), (X, "X"), (Y, "Y"), (LB, "LB"), (RB, "RB"), (LS, "LS"), (RS, "RS"), (BACK, "View"), (START, "Menu"), (GUIDE, "Xbox")],
    }
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    draw_page(ui, app, a);
    remap::preview_commit();
}

fn draw_page(ui: &mut Ui, app: &mut App, a: Rect) {
    #[cfg(debug_assertions)]
    if crate::ui::glyph::SHEET.load(std::sync::atomic::Ordering::Relaxed) {
        crate::ui::glyph::sheet(ui.c, rect(a.x + 28.0, 40.0, a.w - 56.0, a.h), TEXT);
        return;
    }
    header_room(ui, a, "Controles", "Cada controle ao vivo e o que cada botão faz. Nada é gravado no controle.", 0.0);
    if app.pads.edit.is_none() {
        app.pads.open_editor();
    }
    let list = pads::list();
    app.pads.sel = app.pads.sel.min(list.len().saturating_sub(1));
    let x = a.x + 28.0;
    let fw = a.w - 56.0;
    let family = list.get(app.pads.sel).map_or(Family::Xbox, |p| p.family);
    let key = list.get(app.pads.sel).map(|p| p.key.clone());
    let row = rect(x, 88.0, fw, 34.0);
    super::pad_profiles::top(ui, app, row, family, key.as_deref());
    if !super::pad_profiles::naming(app) {
        super::pad_profiles::opens_with(ui, app, super::pad_profiles::slot(row, 3));
    }
    let y = pad_card(ui, app, &list, rect(x, 134.0, fw, 0.0));
    // Gravando uma macro, a aba não troca: o botão Parar fica na aba Macros.
    tabs(ui, x, y, &mut app.pads.tab, !crate::sys::macros::recording());
    let top = y + 46.0;
    let h = a.h - top - 12.0;
    // Fora do editor, nenhuma luz de mostra: as abas que o desenham põem a dele de volta, e a
    // mostra só muda no fim do quadro (`preview_commit`).
    remap::preview_light(None);
    remap::preview_trig(None);
    match app.pads.tab {
        0 => {
            // Sem controle ligado, o editor ocupa a largura toda.
            let mut lw = 0.0;
            if let Some(p) = list.get(app.pads.sel) {
                // Coluna esquerda: desenho, teste e o que a camada muda. Rola: passa da janela.
                let col: f32 = 380.0;
                ui.begin_scroll("pads-art", rect(x, top, col, h), app.pads.art_h);
                let art_h = art_head(app) + ((col - 24.0) * 470.0 / 640.0).floor() + 12.0;
                let mut y = top + art_h + 12.0;
                if !draw_art(ui, app, p, rect(x, top, col, art_h)) {
                    let live = rect(x, top, col, live_h(p, col));
                    ui.card(live);
                    draw_live(ui, p, live);
                    y = live.bottom() + 12.0;
                } else {
                    y = draw_test(ui, app, p, rect(x, y, col, 0.0));
                    y = draw_motion(ui, p, rect(x, y, col, 0.0));
                }
                y = draw_changes(ui, app, p.family, rect(x, y, col, 0.0));
                ui.end_scroll();
                if (y - top - app.pads.art_h).abs() > 0.5 {
                    app.pads.art_h = y - top;
                    ui.s.needs_redraw = true;
                }
                lw = col + 12.0;
            }
            super::pad_profiles::draw(ui, app, rect(x + lw, top, fw - lw, h), family, 0);
        }
        1 => super::pad_profiles::draw(ui, app, rect(x, top, fw, h), family, 1),
        // O radial tem muitas opções: uma aba só dele, na largura toda.
        2 => {
            ui.begin_scroll("pads-radial", rect(x, top, fw, h), app.pads.radial_h);
            let used = radial(ui, app, rect(x, top, fw.min(900.0), 0.0)) - top;
            ui.end_scroll();
            if (used - app.pads.radial_h).abs() > 0.5 {
                app.pads.radial_h = used;
                ui.s.needs_redraw = true;
            }
        }
        _ => settings(ui, app, &list, rect(x, top, fw, h)),
    }
}

/// Cartão do controle: um chip por controle ligado e como o jogo vê o controle.
fn pad_card(ui: &mut Ui, app: &mut App, list: &[Pad], at: Rect) -> f32 {
    let tr = |s: &str| crate::i18n::tr(s).into_owned();
    let r = rect(at.x, at.y, at.w, if list.is_empty() { 140.0 } else { 186.0 });
    ui.card(r);
    if list.is_empty() {
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 48.0), "Nenhum controle ligado. Ligue um controle por USB ou Bluetooth e ele aparece aqui.", 13.0, MUTED, Weight::Regular);
    }
    let mut cx = r.x + 12.0;
    for (i, p) in list.iter().enumerate() {
        let sub = format!("{} · {} · {}", tr(link_text(p.link)), battery_text(p), rate_text(p));
        // O nome vem do aparelho; sem nome é "Controle", que se traduz.
        let pname = tr(&p.name);
        let cw = (measure(&pname, 15.0, Weight::Semibold, None).0.max(measure(&sub, 13.0, Weight::Regular, None).0) + 28.0).min(340.0);
        if cx + cw > r.right() - 12.0 {
            break;
        }
        let c = rect(cx, r.y + 10.0, cw, 52.0);
        let on = i == app.pads.sel;
        if on {
            ui.c.round(c, 8.0, CARD2);
        }
        ui.c.round_line(c, 8.0, if on { ACCENT } else { BORDER }, 1.0);
        ui.c.text_fit(rect(c.x + 14.0, c.y + 6.0, c.w - 28.0, 22.0), &pname, 15.0, TEXT, Align::Left, Weight::Semibold);
        ui.c.text_fit(rect(c.x + 14.0, c.y + 28.0, c.w - 28.0, 18.0), &sub, 13.0, MUTED, Align::Left, Weight::Regular);
        if ui.hit(&format!("pad-{}", p.key), c) && !on {
            app.pads.sel = i;
            ui.s.needs_redraw = true;
        }
        cx += cw + 8.0;
    }
    // Como o jogo vê o controle: como ele é, ou teclado e mouse. Virar Xbox vem com o controle virtual.
    let ry = r.y + 72.0;
    let label = tr("Como o jogo vê este controle");
    let lw = measure(&label, 13.0, Weight::Regular, None).0 + 16.0;
    ui.c.text_raw(rect(r.x + 16.0, ry, lw, 34.0), &label, 13.0, TEXT2, Align::Left, Weight::Regular);
    let bw = ((r.w - 32.0 - lw - 16.0) / 3.0).min(220.0).floor();
    let name = list.get(app.pads.sel).map_or_else(|| tr("Controle"), |p| tr(&p.name));
    let b = |i: f32| rect(r.x + 16.0 + lw + i * (bw + 8.0), ry, bw, 34.0);
    let map = app.cfg.pad_map;
    // Nome que o Windows dá ao controle pode ser longo: sem caber nem a 11 px, fica só "Nativo"
    // (o nome já aparece na lista de controles).
    let mut native = tr("Nativo: {}").replace("{}", &name);
    if measure_raw(&native, 11.0, Weight::Semibold) > bw - 12.0 {
        native = tr("Nativo: {}").replace("{}", "").trim_end().trim_end_matches([':', '：']).to_string();
    }
    // Os três modos se excluem: (teclado e mouse, Xbox).
    let xbox = app.cfg.pad_xbox;
    let mut mode = (map && !xbox, xbox);
    if ui.button_ex("pad-native", b(0.0), &native, !map && !xbox, true) {
        mode = (false, false);
    }
    if ui.button_ex("pad-kbm", b(1.0), "Teclado e mouse", map && !xbox, true) {
        mode = (true, false);
    }
    ui.tip(b(1.0), "Fora dos jogos vale o perfil Desktop: o stick esquerdo mexe o mouse, A e B clicam. Nos jogos (os do Medidor e qualquer programa em tela cheia) vale o perfil do jogo, ou o Base, que não muda nada: sem driver, o jogo continua vendo o controle.");
    let stage = xbridge::stage();
    if ui.button_ex("pad-xbox", b(2.0), "Virar controle Xbox", xbox, !stage.busy()) {
        if matches!(stage, xbridge::Stage::Ready | xbridge::Stage::Reboot) {
            mode = (false, true);
        } else {
            ui.ask("pad-xbox", "Instalar a ponte do controle Xbox?", CONSENT);
        }
    }
    ui.tip(b(2.0), "O jogo vê um controle Xbox no lugar deste, e a vibração do jogo volta a ele. Precisa da ponte VIIPER, um programa de terceiros instalado só com permissão.");
    if ui.answered("pad-xbox") {
        xbridge::start_install();
        mode = (false, true);
    }
    if let Some(v) = xbridge::take_usbip() {
        app.cfg.xbridge_usbip = v;
        app.cfg.save();
    }
    if mode != (map && !xbox, xbox) {
        (app.cfg.pad_map, app.cfg.pad_xbox) = mode;
        app.cfg.save();
        app.apply_pads();
    }
    let note = if xbox {
        bridge_note(&stage)
    } else if map {
        "Fora dos jogos, mouse e teclado. Nos jogos, o perfil do jogo ou o Base, que não muda nada."
    } else {
        "O jogo vê o controle como ele é, e o perfil vale junto. Fora dos jogos, o Desktop."
    };
    ui.c.text_fit(rect(r.x + 16.0, ry + 40.0, r.w - 32.0, 20.0), &tr(note), 12.0, MUTED, Align::Left, Weight::Regular);
    // O perfil deste controle e o botão de desconectar.
    if let Some(p) = list.get(app.pads.sel) {
        let py = ry + 72.0;
        ui.c.text_fit(rect(r.x + 16.0, py, lw - 8.0, 34.0), &tr("Perfil deste controle"), 13.0, TEXT2, Align::Left, Weight::Regular);
        let profiles = remap::profiles();
        let mut opts = vec![tr("Perfil do programa")];
        opts.extend(profiles.iter().map(|x| x.name.clone()));
        let own = app.cfg.pad_fixed.iter().find(|e| e.0 == p.key).map(|e| e.1.clone());
        let mut v = own.and_then(|n| profiles.iter().position(|x| x.name == n)).map_or(0, |i| i + 1);
        let pr = rect(b(0.0).x, py, bw * 2.0 + 8.0, 34.0);
        if ui.select("pad-own", pr, &opts, &mut v) {
            app.cfg.pad_fixed.retain(|e| e.0 != p.key);
            if v > 0 {
                app.cfg.pad_fixed.push((p.key.clone(), profiles[v - 1].name.clone()));
            }
            app.cfg.save();
            app.apply_pads();
        }
        ui.tip(pr, "Perfil do programa: este controle segue o programa em foco. Um perfil escolhido aqui vale só para este controle.");
        // Sempre ligado: o Xbox no Bluetooth pode aparecer como USB. Se não der, nada acontece.
        let dr = b(2.0);
        if ui.button("pad-disc", rect(dr.x, py, dr.w, 34.0), "Desconectar", false) && pads::disconnect(&p.key) {
            app.toast.info(&format!("{} desconectado", tr(&p.name)));
        }
        ui.tip(rect(dr.x, py, dr.w, 34.0), "Desliga o controle do Bluetooth. Pelo cabo, não faz nada.");
    }
    r.bottom() + 12.0
}

/// Abas da página; `on` falso deixa trocar de aba desligado (gravando macro).
fn tabs(ui: &mut Ui, x: f32, y: f32, sel: &mut usize, on: bool) {
    const TABS: [&str; 4] = ["Mapeamento", "Macros", "Menu radial", "Ajustes"];
    if on {
        ui.tab_step(sel, TABS.len());
    }
    let mut cx = x;
    for (i, name) in TABS.iter().enumerate() {
        let name = crate::i18n::tr(name);
        let w = measure(&name, 14.0, Weight::Semibold, None).0 + 28.0;
        let r = rect(cx, y, w, 34.0);
        let cur = *sel == i;
        if cur {
            ui.c.round(r, 8.0, CARD2);
        }
        if on && ui.hit(&format!("pad-tab{i}"), r) {
            *sel = i;
        }
        let color = if cur { TEXT } else { MUTED };
        ui.c.text_raw(r, &name, 14.0, color, Align::Center, if cur { Weight::Semibold } else { Weight::Regular });
        cx += w + 4.0;
    }
}

/// Aba Ajustes: até três colunas (avisos; diagnóstico, medir sticks e teclado; mods, ponte e drivers).
fn settings(ui: &mut Ui, app: &mut App, list: &[Pad], a: Rect) {
    let (x, top) = (a.x, a.y);
    // Colunas que rolam juntas, com alturas parecidas: avisos | medir e teclado | mods, ponte e
    // drivers. Janela mais estreita junta em duas (a terceira vai para baixo da primeira) ou uma.
    let cols = if a.w >= 1000.0 { 3 } else if a.w >= 652.0 { 2 } else { 1 };
    let cw = ((a.w - 12.0 * (cols - 1) as f32) / cols as f32).floor();
    let col = |i: usize| x + i as f32 * (cw + 12.0);
    ui.begin_scroll("pads-set", a, app.pads.left_h);
    let a_end = alerts(ui, app, list, rect(x, top, cw, 0.0));
    let (bx, by) = if cols > 1 { (col(1), top) } else { (x, a_end) };
    let b_end = tools(ui, app, list, rect(bx, by, cw, 0.0));
    let (cx, cy) = match cols {
        3 => (col(2), top),
        2 => (x, a_end),
        _ => (x, b_end),
    };
    let c_end = extras(ui, app, list, rect(cx, cy, cw, 0.0));
    let y = a_end.max(b_end).max(c_end);
    if (y - top - app.pads.left_h).abs() > 0.5 {
        app.pads.left_h = y - top;
        ui.s.needs_redraw = true;
    }
    ui.end_scroll();
}

/// Controles pareados sem HID, Avisos, o cartão do Mapeamento e os conflitos.
fn alerts(ui: &mut Ui, app: &mut App, list: &[Pad], at: Rect) -> f32 {
    let (x, lw) = (at.x, at.w);
    let mut y = at.y;
    for name in pads::without_hid() {
        let r = rect(x, y, lw, 122.0);
        ui.card(r);
        ui.c.text_fit(rect(r.x + 16.0, r.y + 10.0, r.w - 32.0, 20.0), &name, 14.0, WARN, Align::Left, Weight::Semibold);
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 34.0, r.w - 32.0, 80.0), "Pareado sem a parte de controle: nenhum programa consegue ler. Remova o controle e pareie de novo pelo assistente clássico (Win + R, devicepairingwizard).", 12.0, TEXT2, Weight::Regular);
        y += r.h + 8.0;
    }
    let r = rect(x, y, lw, 262.0);
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
    changed |= ui.check("pad-speaker", rect(r.x + 10.0, r.y + 168.0, r.w - 20.0, 36.0), "Som no alto-falante do controle", &mut app.cfg.pad_speaker);
    ui.tip(rect(r.x + 10.0, r.y + 168.0, r.w - 20.0, 36.0), "Um som curto no alto-falante do DualSense e do DualShock 4 ligados pelo cabo, ao conectar, desconectar, trocar de perfil, com bateria baixa e com variação alta.");
    // Toca mesmo com a caixa desligada: serve para ver se o alto-falante funciona.
    if ui.button("pad-speaker-test", rect(r.x + 16.0, r.y + 212.0, r.w - 32.0, 34.0), "Testar alto-falante", false) {
        let sony = |p: &&pads::Pad| matches!(p.family, pads::Family::DualSense | pads::Family::Ds4);
        if list.iter().filter(sony).any(|p| p.link == pads::Link::Usb) {
            crate::sys::pad_sound::play(crate::sys::pad_sound::Chime::Test);
        } else if list.iter().any(|p| sony(&p)) {
            app.toast.info("O alto-falante pelo Bluetooth está desligado por enquanto: ligue o controle pelo cabo");
        } else {
            app.toast.info("Nenhum DualSense ou DualShock 4 ligado");
        }
    }
    if changed {
        app.cfg.save();
        app.apply_pads();
    }
    y += r.h + 12.0;
    y = mapping(ui, rect(x, y, lw, 0.0));
    for t in &app.pads.conflicts {
        let r = rect(x, y, lw, 64.0);
        ui.card(r);
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 10.0, r.w - 32.0, r.h - 16.0), t, 12.0, TEXT2, Weight::Regular);
        y += r.h + 8.0;
    }
    y
}

/// Mods do DSX, a ponte do Xbox, esconder o controle e os drivers.
fn extras(ui: &mut Ui, app: &mut App, list: &[Pad], at: Rect) -> f32 {
    let (x, lw) = (at.x, at.w);
    let y = dsx(ui, app, at);
    let y = bridge(ui, app, rect(x, y, lw, 0.0));
    let y = super::pad_hide::card(ui, app, list, rect(x, y, lw, 0.0));
    super::pad_hide::drivers(ui, app, rect(x, y, lw, 0.0))
}

/// Copiar diagnóstico, medir os sticks do controle escolhido e o teclado do Prisma.
fn tools(ui: &mut Ui, app: &mut App, list: &[Pad], at: Rect) -> f32 {
    let (rx, rw) = (at.x, at.w);
    if ui.button("pads-copy", rect(rx, at.y, 200.0_f32.min(rw), 36.0), "Copiar diagnóstico", false) {
        let ok = crate::capture::copy_text(app.hub, &diagnostic(list, &app.pads.conflicts));
        app.toast.info(if ok { "Diagnóstico copiado" } else { "Não consegui copiar" });
    }
    let mut y = at.y + 48.0;
    if let Some(p) = list.get(app.pads.sel) {
        // O cartão só fica alto enquanto está medindo ou mostrando o resultado.
        let mh = if app.pads.wiz.is_some() || app.pads.result.is_some() { 220.0 } else { 150.0 };
        let m = rect(rx, y, rw, mh);
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
        y = m.bottom() + 12.0;
    }
    keyboard(ui, app, rect(rx, y, rw, 0.0))
}

/// Área de um botão no desenho: bit, x, y, largura e altura em % da imagem, e o raio da borda em
/// pixels do desenho de 640 (-1 é redondo). Vem do `hits.json` dos desenhos do mock-up.
type Hit = (pads::Buttons, f32, f32, f32, f32, f32);

const HITS_DS: &[Hit] = &[
    (pads::PAD, 31.60, 13.10, 36.79, 26.90, 10.0),
    (pads::BACK, 25.95, 16.22, 4.06, 9.36, 13.0),
    (pads::START, 69.98, 16.22, 4.06, 9.36, 13.0),
    (pads::UP, 17.32, 23.16, 6.56, 8.94, 10.0),
    (pads::DOWN, 17.32, 38.05, 6.56, 8.94, 10.0),
    (pads::LEFT, 11.85, 30.60, 6.56, 8.94, 10.0),
    (pads::RIGHT, 22.79, 30.60, 6.56, 8.94, 10.0),
    (pads::Y, 75.35, 19.90, 7.81, 10.64, -1.0),
    (pads::B, 82.43, 29.75, 7.81, 10.64, -1.0),
    (pads::A, 75.35, 39.40, 7.81, 10.64, -1.0),
    (pads::X, 68.26, 29.75, 7.81, 10.64, -1.0),
    (pads::LS, 27.28, 42.33, 14.38, 19.57, -1.0),
    (pads::RS, 58.34, 42.33, 14.38, 19.57, -1.0),
    (pads::GUIDE, 46.56, 48.50, 6.88, 6.81, 12.0),
    (pads::MUTE, 46.56, 57.38, 6.88, 3.83, 8.0),
    (remap::LT, 14.38, 0.85, 16.56, 4.89, 10.0),
    (pads::LB, 14.06, 5.96, 16.56, 4.89, 8.0),
    (remap::RT, 69.38, 0.85, 16.56, 4.89, 10.0),
    (pads::RB, 69.38, 5.96, 16.56, 4.89, 8.0),
];
const HITS_DS4: &[Hit] = &[
    (pads::PAD, 33.09, 13.28, 33.81, 21.57, 10.0),
    (pads::UP, 18.22, 18.60, 5.62, 7.87, 9.0),
    (pads::DOWN, 18.22, 31.57, 5.62, 7.87, 9.0),
    (pads::LEFT, 13.38, 25.19, 5.78, 7.66, 9.0),
    (pads::RIGHT, 22.91, 25.19, 5.78, 7.66, 9.0),
    (pads::Y, 76.56, 16.89, 5.94, 8.09, -1.0),
    (pads::B, 82.50, 24.98, 5.94, 8.09, -1.0),
    (pads::A, 76.56, 33.06, 5.94, 8.09, -1.0),
    (pads::X, 70.62, 24.98, 5.94, 8.09, -1.0),
    (pads::BACK, 28.72, 14.51, 3.75, 6.81, 10.0),
    (pads::START, 68.09, 14.51, 3.75, 6.81, 10.0),
    (pads::LS, 27.34, 36.85, 14.38, 19.57, -1.0),
    (pads::RS, 58.84, 36.85, 14.38, 19.57, -1.0),
    (pads::GUIDE, 47.47, 45.11, 5.62, 7.66, -1.0),
    (remap::LT, 17.50, 0.85, 13.44, 4.89, 10.0),
    (pads::LB, 17.19, 5.96, 13.44, 4.89, 8.0),
    (remap::RT, 69.38, 0.85, 13.44, 4.89, 10.0),
    (pads::RB, 69.38, 5.96, 13.44, 4.89, 8.0),
];
const HITS_XBOX: &[Hit] = &[
    (pads::GUIDE, 45.78, 13.23, 8.44, 11.49, -1.0),
    (pads::LS, 17.85, 21.70, 14.69, 20.00, -1.0),
    (pads::RS, 55.21, 40.46, 14.69, 20.00, -1.0),
    (pads::UP, 34.79, 43.80, 5.31, 6.81, 8.0),
    (pads::DOWN, 34.79, 57.41, 5.31, 6.81, 8.0),
    (pads::LEFT, 29.94, 50.39, 5.00, 7.23, 8.0),
    (pads::RIGHT, 39.94, 50.39, 5.00, 7.23, 8.0),
    (pads::BACK, 40.30, 28.70, 5.62, 7.66, -1.0),
    (pads::START, 54.23, 28.70, 5.62, 7.66, -1.0),
    (pads::Y, 70.90, 18.66, 7.81, 10.64, -1.0),
    (pads::B, 77.33, 27.84, 7.81, 10.64, -1.0),
    (pads::A, 70.90, 36.39, 7.81, 10.64, -1.0),
    (pads::X, 64.16, 27.42, 7.81, 10.64, -1.0),
    (remap::LT, 15.62, 0.85, 17.19, 4.89, 10.0),
    (pads::LB, 15.31, 5.96, 17.19, 4.89, 8.0),
    (remap::RT, 67.50, 0.85, 17.19, 4.89, 10.0),
    (pads::RB, 67.50, 5.96, 17.19, 4.89, 8.0),
];

/// Desenho da família (PNG feito dos vetores do Prisma) e as áreas dos botões. O genérico não tem.
fn art(f: Family) -> Option<(&'static str, &'static [u8], &'static [Hit])> {
    match f {
        Family::DualSense => Some(("pad-ds", include_bytes!("../../res/pads/ds.png"), HITS_DS)),
        Family::Ds4 => Some(("pad-ds4", include_bytes!("../../res/pads/ds4.png"), HITS_DS4)),
        Family::Xbox => Some(("pad-xbox", include_bytes!("../../res/pads/xbox.png"), HITS_XBOX)),
        Family::EightBitDo | Family::Switch | Family::Generic => None,
    }
}

/// Cartão do desenho: o botão apertado acende, o escolhido no editor fica contornado, e clicar
/// num botão escolhe ele. Devolve false sem desenho para a família (o genérico usa `draw_live`).
/// Altura acima do desenho: a linha do estado e, com mais de uma camada no perfil, as fichas delas.
fn art_head(app: &App) -> f32 {
    if super::pad_profiles::layers(app).is_some() { 74.0 } else { 40.0 }
}

fn draw_art(ui: &mut Ui, app: &mut App, p: &Pad, r: Rect) -> bool {
    let Some((key, png, hits)) = art(p.family) else { return false };
    ui.card(r);
    let line = format!("{} · {} · {}: {}", crate::i18n::tr(link_text(p.link)), rate_text(p), crate::i18n::tr("bateria"), battery_text(p));
    ui.c.text_fit(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 20.0), &line, 13.0, TEXT2, Align::Left, Weight::Regular);
    // Fichas das camadas: a escolhida é a que o editor e o desenho mostram.
    if let Some((names, cur)) = super::pad_profiles::layers(app) {
        let mut cx = r.x + 16.0;
        for (i, n) in names.iter().enumerate() {
            let cw = (measure(n, 13.0, Weight::Regular, None).0 + 24.0).min(140.0);
            if cx + cw > r.right() - 16.0 {
                break; // shortcut: as que não cabem ficam só na lista do editor.
            }
            let c = rect(cx, r.y + 38.0, cw, 26.0);
            ui.c.round(c, 13.0, if i == cur { ACCENT.alpha(0.25) } else { CARD2 });
            ui.c.text_fit(rect(c.x + 10.0, c.y, c.w - 20.0, c.h), n, 13.0, if i == cur { TEXT } else { TEXT2 }, Align::Center, Weight::Regular);
            if ui.hit(&format!("pad-layer{i}"), c) {
                super::pad_profiles::set_layer(app, i);
            }
            cx += cw + 6.0;
        }
    }
    let w = r.w - 24.0;
    let img = rect(r.x + 12.0, r.y + art_head(app), w, (w * 470.0 / 640.0).floor());
    ui.png(key, png, img);
    let i = p.input;
    let chosen = super::pad_profiles::chosen(app);
    let k = img.w / 640.0;
    for &(bit, x, y, hw, hh, rad) in hits {
        let b = rect(img.x + img.w * x / 100.0, img.y + img.h * y / 100.0, img.w * hw / 100.0, img.h * hh / 100.0);
        let radius = if rad < 0.0 { b.w.min(b.h) / 2.0 } else { rad * k };
        let on = match bit {
            remap::LT => i.lt > 0.1,
            remap::RT => i.rt > 0.1,
            _ => i.buttons & bit != 0,
        };
        if on {
            ui.c.round(b, radius, ACCENT.alpha(0.55));
        }
        // O stick mostra onde está: um ponto que anda dentro da área dele.
        if bit == pads::LS || bit == pads::RS {
            let (sx, sy) = if bit == pads::LS { (i.lx, i.ly) } else { (i.rx, i.ry) };
            ui.c.circle(b.x + b.w / 2.0 + sx.clamp(-1.0, 1.0) * b.w / 4.0, b.y + b.h / 2.0 - sy.clamp(-1.0, 1.0) * b.h / 4.0, 4.0, ACCENT);
        }
        if bit == chosen {
            ui.c.round_line(b, radius, ACCENT, 2.0);
        } else if ui.is_hover(b) {
            ui.c.round_line(b, radius, BORDER2, 1.5);
        }
        let name = super::pad_profiles::button_label(p.family, bit);
        ui.tip(b, &name);
        if ui.hit(&format!("pad-art{bit}"), b) {
            super::pad_profiles::choose(app, bit);
        }
    }
    true
}

/// Cartão do teste: último botão, quantos responderam, os que faltam e o histórico.
fn draw_test(ui: &mut Ui, app: &mut App, p: &Pad, at: Rect) -> f32 {
    let tr = |s: &str| crate::i18n::tr(s).into_owned();
    let Some((_, _, hits)) = art(p.family) else { return at.y };
    let name = |bit: pads::Buttons| super::pad_profiles::button_label(p.family, bit);
    let t = &app.pads.test;
    let x = at.x + 16.0;
    let w = at.w - 32.0;
    let rows = t.hist.len();
    let r = rect(at.x, at.y, at.w, 140.0 + if rows > 0 { 26.0 + rows as f32 * 22.0 } else { 0.0 });
    ui.card(r);
    let on = app.pads.testing;
    ui.title(rect(x, r.y + 12.0, w - 216.0, 22.0), "Teste dos botões");
    let start = ui.button_ex("pad-test-on", rect(r.right() - 224.0, r.y + 8.0, 100.0, 32.0), if on { "Parar" } else { "Testar" }, on, true);
    let clear = ui.button_ex("pad-test-clear", rect(r.right() - 116.0, r.y + 8.0, 100.0, 32.0), "Limpar", false, t.seen != 0);
    let last = match (on, t.down.last(), t.hist.first()) {
        // Sem o teste ligado, os botões navegam pelo Prisma e não dá para apertá-los à vontade.
        (false, ..) => tr("Clique em Testar: o controle para de navegar pelo Prisma."),
        (_, Some(&(bit, _)), _) => tr("Último botão: {}, apertado").replace("{}", &name(bit)),
        (_, None, Some(&(bit, ms, _))) => tr("Último botão: {} ({} ms)").replacen("{}", &name(bit), 1).replacen("{}", &ms.to_string(), 1),
        _ => tr("Aperte um botão no controle."),
    };
    ui.c.text_fit(rect(x, r.y + 44.0, w, 20.0), &last, 13.0, TEXT, Align::Left, Weight::Regular);
    let all: Vec<pads::Buttons> = hits.iter().map(|h| h.0).collect();
    let ok = all.iter().filter(|b| t.seen & **b != 0).count();
    let count = tr("Responderam {} de {}").replacen("{}", &ok.to_string(), 1).replacen("{}", &all.len().to_string(), 1);
    ui.c.text_fit(rect(x, r.y + 68.0, w, 20.0), &count, 13.0, TEXT2, Align::Left, Weight::Semibold);
    let missing: Vec<String> = all.iter().filter(|b| t.seen & **b == 0).map(|b| name(*b)).collect();
    let rest = if missing.is_empty() { tr("Todos responderam.") } else { tr("Faltam: {}").replace("{}", &missing.join(", ")) };
    ui.c.text_wrap(rect(x, r.y + 92.0, w, 40.0), &rest, 12.0, MUTED, Weight::Regular);
    let mut y = r.y + 140.0;
    if rows > 0 {
        ui.c.text_fit(rect(x, y, w, 20.0), &tr("Histórico"), 13.0, TEXT2, Align::Left, Weight::Semibold);
        y += 26.0;
        for &(bit, ms, double) in &t.hist {
            ui.c.text_fit(rect(x, y, w * 0.4, 20.0), &name(bit), 12.0, TEXT, Align::Left, Weight::Regular);
            if double {
                ui.c.text_fit(rect(x + w * 0.4, y, w * 0.4, 20.0), &tr("aperto duplo"), 12.0, WARN, Align::Left, Weight::Regular);
            }
            ui.c.text_raw(rect(x + w * 0.8, y, w * 0.2, 20.0), &format!("{ms} ms"), 12.0, MUTED, Align::Right, Weight::Regular);
            y += 22.0;
        }
    }
    if clear {
        app.pads.test = Test { key: p.key.clone(), prev: app.pads.test.prev, ..Default::default() };
    }
    if start {
        app.pads.testing = !on;
    }
    r.bottom() + 12.0
}

/// Cartão do gyro e do touchpad (DualSense e DualShock 4): os três giros ao vivo, a calibração e
/// os dedos no touchpad.
fn draw_motion(ui: &mut Ui, p: &Pad, at: Rect) -> f32 {
    let tr = |s: &str| crate::i18n::tr(s).into_owned();
    if !matches!(p.family, Family::DualSense | Family::Ds4) {
        return at.y;
    }
    let x = at.x + 16.0;
    let w = at.w - 32.0;
    let i = p.input;
    if !i.motion {
        // Modo simples do Bluetooth: o relatório não traz gyro nem touchpad.
        let r = rect(at.x, at.y, at.w, 92.0);
        ui.card(r);
        ui.title(rect(x, r.y + 12.0, w, 22.0), "Gyro e touchpad");
        ui.c.text_wrap(rect(x, r.y + 42.0, w, 40.0), &tr("Pelo Bluetooth, o gyro e o touchpad chegam quando o controle passa ao modo completo, que o Prisma pede ao conectar. Se não aparecerem, desligue e ligue o controle."), 12.0, MUTED, Weight::Regular);
        return r.bottom() + 12.0;
    }
    let pad_h = (w * 0.45).floor();
    let r = rect(at.x, at.y, at.w, 72.0 + 3.0 * 26.0 + 12.0 + pad_h + 16.0);
    ui.card(r);
    ui.title(rect(x, r.y + 12.0, w - 124.0, 22.0), "Gyro e touchpad");
    let c = p.gyro_cal;
    if ui.button_ex("pad-gyro-cal", rect(r.right() - 132.0, r.y + 8.0, 116.0, 32.0), "Calibrar", false, !c.ask) {
        pads::calibrate_gyro();
    }
    let state = if c.ask {
        tr("Deixe o controle parado na mesa.")
    } else if c.done {
        tr("Calibrado: o desvio parado sai de toda leitura.")
    } else {
        tr("Ainda sem calibrar: deixe o controle parado por 1 s.")
    };
    ui.c.text_fit(rect(x, r.y + 44.0, w, 20.0), &state, 12.0, if c.ask { WARN } else { MUTED }, Align::Left, Weight::Regular);
    // Barras de -360 a 360 °/s, com o zero no meio.
    let mut y = r.y + 72.0;
    let lw = (w * 0.3).floor();
    for (k, name) in ["Inclinar", "Virar", "Girar"].iter().enumerate() {
        let v = i.gyro[k];
        ui.c.text_fit(rect(x, y, lw, 20.0), &tr(name), 12.0, TEXT2, Align::Left, Weight::Regular);
        let bar = rect(x + lw + 8.0, y + 6.0, w - lw - 8.0 - 76.0, 8.0);
        ui.c.round(bar, 4.0, CARD2);
        let mid = bar.x + bar.w / 2.0;
        let len = (v / 360.0).clamp(-1.0, 1.0) * bar.w / 2.0;
        ui.c.round(rect(mid.min(mid + len), bar.y, len.abs().max(2.0), bar.h), 4.0, ACCENT);
        ui.c.text_raw(rect(bar.right() + 8.0, y, 68.0, 20.0), &format!("{v:.0} °/s"), 12.0, MUTED, Align::Right, Weight::Regular);
        y += 26.0;
    }
    // Touchpad: o retângulo com um ponto por dedo.
    let tp = rect(x, y + 12.0, w, pad_h);
    ui.c.round_line(tp, 8.0, BORDER2, 1.5);
    for (f, t) in i.touch.iter().enumerate() {
        if let Some((tx, ty)) = t {
            ui.c.circle(tp.x + tx * tp.w, tp.y + ty * tp.h, if f == 0 { 7.0 } else { 6.0 }, ACCENT);
        }
    }
    if i.touch.iter().all(Option::is_none) {
        ui.c.text_fit(tp, &tr("Toque no touchpad"), 12.0, MUTED, Align::Center, Weight::Regular);
    }
    r.bottom() + 12.0
}

/// Cartão "O que mudou nesta camada": as ligações da camada escolhida no editor.
fn draw_changes(ui: &mut Ui, app: &App, f: Family, at: Rect) -> f32 {
    let Some((layer, lines)) = super::pad_profiles::layer_changes(app, f) else { return at.y };
    let x = at.x + 16.0;
    let w = at.w - 32.0;
    let r = rect(at.x, at.y, at.w, 48.0 + lines.len().max(1) as f32 * 22.0 + 8.0);
    ui.card(r);
    let title = crate::i18n::tr("O que mudou na camada {}").replace("{}", &layer);
    ui.c.text_fit(rect(x, r.y + 12.0, w, 22.0), &title, 15.0, TEXT, Align::Left, Weight::Semibold);
    let mut y = r.y + 44.0;
    if lines.is_empty() {
        ui.c.text_fit(rect(x, y, w, 20.0), &crate::i18n::tr("Nada: os botões fazem o que o jogo manda."), 12.0, MUTED, Align::Left, Weight::Regular);
    }
    for l in &lines {
        ui.c.text_fit(rect(x, y, w, 20.0), l, 12.0, TEXT2, Align::Left, Weight::Regular);
        y += 22.0;
    }
    r.bottom() + 12.0
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
            (Family::Switch, 0) => "ZL",
            (Family::Switch, _) => "ZR",
            (_, 0) => "LT",
            _ => "RT",
        };
        ui.c.text_raw(rect(bar.x - 10.0, bar.bottom() + 4.0, 32.0, 16.0), name, 11.0, MUTED, Align::Center, Weight::Regular);
    }
    let style = crate::ui::glyph::Style::of(p.family, p.vid);
    let mut bottom = cy + rad + 32.0;
    for (c, bit, on, label) in live_chips(p, r) {
        ui.c.round(c, 6.0, if on { ACCENT } else { CARD2 });
        if p.family != Family::Generic {
            crate::ui::glyph::draw(ui.c, c.x + 8.0, c.y + 12.0, 16.0, style, bit, if on { ON_ACCENT } else { TEXT2 });
        } else {
            ui.c.text_raw(c, &label, 12.0, if on { ON_ACCENT } else { TEXT2 }, Align::Center, Weight::Semibold);
        }
        bottom = c.bottom();
    }
    // Eixos do controle genérico, crus: manete, leme, pedais e o que mais vier.
    for (k, &(page, usage)) in p.generic_axes.iter().enumerate() {
        let y = bottom + 12.0 + k as f32 * AXIS_ROW;
        let lw = 110.0;
        ui.c.text_fit(rect(r.x + 16.0, y, lw, 18.0), &axis_name(page, usage), 12.0, MUTED, Align::Left, Weight::Regular);
        let bar = rect(r.x + 24.0 + lw, y + 5.0, r.w - lw - 40.0, 8.0);
        ui.c.round(bar, 4.0, CARD2);
        ui.c.round(rect(bar.x, bar.y, bar.w * i.axes[k].clamp(0.0, 1.0), bar.h), 4.0, ACCENT);
    }
}

const AXIS_ROW: f32 = 22.0;

/// Botões do teste ao vivo, já no lugar: os do desenho da família, ou numerados no genérico, e o
/// direcional. Começam embaixo dos sticks.
fn live_chips(p: &Pad, r: Rect) -> Vec<(Rect, pads::Buttons, bool, String)> {
    let i = p.input;
    let mut chips: Vec<(pads::Buttons, bool, String)> = [(pads::UP, "↑"), (pads::DOWN, "↓"), (pads::LEFT, "←"), (pads::RIGHT, "→")].iter().map(|(b, n)| (*b, i.buttons & b != 0, n.to_string())).collect();
    if p.family == Family::Generic {
        chips.extend((0..p.generic_buttons).map(|n| pads::generic_bit(n)).enumerate().map(|(n, b)| (b, i.buttons & b != 0, (n + 1).to_string())));
    } else {
        chips.extend(button_names(p.family).iter().map(|(b, n)| (*b, i.buttons & b != 0, crate::i18n::tr(n).into_owned())));
    }
    let (mut bx, mut by) = (r.x + 16.0, r.y + LIVE_CHIPS);
    let mut out = vec![];
    for (bit, on, label) in chips {
        // O genérico tem largura fixa: 32 botões com número de dois dígitos cabem em linhas iguais.
        let w = if p.family == Family::Generic { 36.0 } else { crate::ui::glyph::width(bit, 16.0) + 16.0 };
        if bx + w > r.right() - 16.0 {
            bx = r.x + 16.0;
            by += 30.0;
        }
        out.push((rect(bx, by, w, 24.0), bit, on, label));
        bx += w + 6.0;
    }
    out
}

/// Onde os botões começam: abaixo da linha de estado e dos sticks.
const LIVE_CHIPS: f32 = 62.0 + 2.0 * 46.0 + 32.0;

/// Altura do cartão do teste ao vivo com largura `w`: cresce com os botões e eixos do controle.
fn live_h(p: &Pad, w: f32) -> f32 {
    let r = rect(0.0, 0.0, w, 0.0);
    let chips = live_chips(p, r).last().map_or(LIVE_CHIPS, |c| c.0.bottom());
    let axes = p.generic_axes.len() as f32 * AXIS_ROW;
    (chips + if axes > 0.0 { 12.0 + axes } else { 0.0 } + 16.0).max(300.0)
}

/// Nome de um eixo HID: X, Y, Z e as rotações ficam como estão; os outros, pelo uso.
fn axis_name(page: u16, usage: u16) -> String {
    let n = match (page, usage) {
        (1, 0x30) => "X",
        (1, 0x31) => "Y",
        (1, 0x32) => "Z",
        (1, 0x33) => "Rx",
        (1, 0x34) => "Ry",
        (1, 0x35) => "Rz",
        (1, 0x36) => "Deslizante",
        (1, 0x37) => "Roda",
        (2, 0xBA) => "Leme",
        (2, 0xBB) => "Manete",
        (2, 0xC4) => "Acelerador",
        (2, 0xC5) => "Freio",
        (2, 0xC6) => "Embreagem",
        (2, 0xC8) => "Volante",
        _ => return format!("{page:X}:{usage:02X}"),
    };
    crate::i18n::tr(n).into_owned()
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
fn mapping(ui: &mut Ui, at: Rect) -> f32 {
    let r = rect(at.x, at.y, at.w, 180.0);
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Mapeamento");
    let state = match (remap::stopped(), remap::active()) {
        (true, _) => (crate::i18n::tr("Parado pela combinação").into_owned(), WARN),
        (false, p) => (crate::i18n::tr("Perfil {}").replace("{}", &crate::i18n::tr(&p.unwrap_or_default())), TEXT),
    };
    ui.c.text_fit(rect(r.x + 16.0, r.y + 42.0, r.w - 32.0, 20.0), &state.0, 13.0, state.1, Align::Left, Weight::Semibold);
    let ny = 68.0;
    ui.c.text_wrap(rect(r.x + 16.0, r.y + ny, r.w - 32.0, 72.0), "Para parar tudo sem teclado, segure Guia (ou PS) + Select + Start por 2 segundos. A mesma combinação liga de novo.", 12.0, TEXT2, Weight::Regular);
    if let Some((med, p99)) = remap::latency() {
        let t = crate::i18n::tr("Tempo interno: {} µs na mediana, {} µs no p99").replacen("{}", &med.to_string(), 1).replacen("{}", &p99.to_string(), 1);
        ui.c.text_fit(rect(r.x + 16.0, r.y + ny + 80.0, r.w - 32.0, 20.0), &t, 12.0, MUTED, Align::Left, Weight::Regular);
    }
    r.bottom() + 12.0
}

/// Mods do DSX: aceitar as mensagens dos mods de jogos pela porta UDP 6969.
fn dsx(ui: &mut Ui, app: &mut App, at: Rect) -> f32 {
    let note = crate::i18n::tr("Mods de jogos feitos para o DSX mudam a luz e os gatilhos do DualSense. Só aceita mensagens deste PC. O que o mod pedir fica até ele soltar, até desligar aqui ou até fechar o Prisma.");
    let nh = crate::ui::gfx::measure(&note, 12.0, Weight::Regular, Some(at.w - 32.0)).1.ceil();
    let r = rect(at.x, at.y, at.w, 90.0 + nh);
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Mods do DSX");
    if ui.check("pad-dsx", rect(r.x + 10.0, r.y + 38.0, r.w - 20.0, 36.0), "Aceitar mods pela porta UDP 6969", &mut app.cfg.dsx_udp) {
        app.cfg.save();
        app.apply_pads();
    }
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 78.0, r.w - 32.0, nh), &note, 12.0, TEXT2, Weight::Regular);
    r.bottom() + 12.0
}

/// Aviso antes de baixar e instalar a ponte: o que entra no PC e como desfazer.
const CONSENT: &str = "O Prisma vai baixar o VIIPER, programa livre de terceiros, da página oficial dele no GitHub, e rodar o instalador. Ele instala o driver usbip-win2, pede permissão de administrador, começa com o Windows e pode pedir para reiniciar o PC. Para desfazer: Controles, aba Ajustes, cartão Ponte do controle Xbox, Remover a ponte. Sai tudo o que veio com ela.";

/// Uma linha sobre a ponte.
fn bridge_note(s: &xbridge::Stage) -> &'static str {
    use xbridge::Stage::*;
    match s {
        Absent => "Ponte não instalada. Sem ela, nada muda no PC.",
        Downloading => "Baixando o VIIPER da página oficial.",
        Installing => "Instalando. Aceite a permissão de administrador do Windows.",
        Reboot => "Ponte instalada. Reinicie o PC para o driver começar.",
        Removing => "Removendo a ponte.",
        Failed(_) => "A ponte não deu certo. Veja o motivo em Ajustes.",
        Ready if remap::anti_cheat_front() => "Pausada: o jogo em foco tem anti-cheat.",
        Ready if remap::stopped() => "Pausada pela parada de emergência.",
        Ready => "O jogo vê um controle Xbox, e a vibração do jogo volta ao controle.",
    }
}

/// Ponte do controle Xbox (Fase 8): estado, juntar os controles, instalar e remover.
fn bridge(ui: &mut Ui, app: &mut App, at: Rect) -> f32 {
    let stage = xbridge::stage();
    let line = if stage == xbridge::Stage::Ready && !app.cfg.pad_xbox {
        "Ponte pronta. Escolha Virar controle Xbox no cartão do controle."
    } else if stage == xbridge::Stage::Ready && xbridge::reboot_asked() && !remap::anti_cheat_front() && !remap::stopped() {
        "Ponte instalada e funcionando. O Windows pede para reiniciar o PC para terminar a instalação do driver."
    } else {
        bridge_note(&stage)
    };
    let mut note = crate::i18n::tr(line).into_owned();
    if let xbridge::Stage::Failed(e) = &stage {
        note = format!("{note} ({})", crate::i18n::tr(e));
    }
    let nh = crate::ui::gfx::measure(&note, 12.0, Weight::Regular, Some(at.w - 32.0)).1.ceil();
    let r = rect(at.x, at.y, at.w, 140.0 + nh);
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Ponte do controle Xbox");
    ui.c.text_wrap(rect(r.x + 16.0, r.y + 40.0, r.w - 32.0, nh), &note, 12.0, TEXT2, Weight::Regular);
    let y = r.y + 46.0 + nh;
    if ui.check("pad-xbox-merge", rect(r.x + 10.0, y, r.w - 20.0, 36.0), "Juntar todos os controles num Xbox só", &mut app.cfg.pad_xbox_merge) {
        app.cfg.save();
        app.apply_pads();
    }
    ui.tip(rect(r.x + 10.0, y, r.w - 20.0, 36.0), "Os botões de todos valem juntos; em cada stick e gatilho vale o mais apertado. A vibração do jogo vai a todos.");
    let bw = ((r.w - 40.0) / 2.0).floor();
    let b1 = rect(r.x + 16.0, y + 44.0, bw, 34.0);
    let b2 = rect(r.x + 24.0 + bw, y + 44.0, bw, 34.0);
    let busy = stage.busy();
    let installed = !matches!(stage, xbridge::Stage::Absent);
    if !installed {
        if ui.button("xb-install", b1, "Instalar a ponte", true) && !busy {
            ui.ask("xb-install", "Instalar a ponte do controle Xbox?", CONSENT);
        }
    } else if ui.button("xb-check", b1, "Verificar de novo", false) && !busy {
        xbridge::recheck();
    }
    if installed && ui.button("xb-remove", b2, "Remover a ponte", false) && !busy {
        ui.ask("xb-remove", "Remover a ponte?", "Sai o VIIPER, o início automático dele e, se veio com ele, o driver usbip-win2. O Windows pede permissão de administrador.");
    }
    if ui.answered("xb-install") {
        xbridge::start_install();
    }
    if let Some(v) = xbridge::take_usbip() {
        app.cfg.xbridge_usbip = v;
        app.cfg.save();
    }
    if ui.answered("xb-remove") {
        app.cfg.pad_xbox = false;
        app.cfg.save();
        app.apply_pads();
        xbridge::start_remove(app.cfg.xbridge_usbip);
    }
    r.bottom() + 12.0
}

/// Radial do Prisma: as 8 ações, de cima no sentido do relógio.
fn radial(ui: &mut Ui, app: &mut App, at: Rect) -> f32 {
    use crate::sys::radial::{sub, EMPTY, GROUP, PROFILES};
    const DIRS: [&str; 8] = ["↑", "↗", "→", "↘", "↓", "↙", "←", "↖"];
    let two = app.cfg.pad_radial_two;
    // Cada lugar de 4 opções ganha 4 linhas, uma por direção do direcional.
    let n = if two { 16 } else { 8 };
    let groups = app.cfg.pad_radial[..n].iter().filter(|&&a| a == GROUP).count();
    let r = rect(at.x, at.y, at.w, if two { 536.0 } else { 528.0 } + (n + 4 * groups) as f32 * 42.0);
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
    let gstyle = pads::list().get(app.pads.sel).map_or(crate::ui::glyph::Style::Xbox, |p| crate::ui::glyph::Style::of(p.family, p.vid));
    let btns: Vec<String> = crate::sys::radial::PICK_BUTTONS.iter().map(|&b| super::pad_profiles::button_label(f, b)).collect();
    for (i, key, label) in [(3, "pad-radial-ok", "Confirmar"), (4, "pad-radial-back", "Voltar")] {
        let (l, c) = row(i);
        ui.c.text_fit(l, &tr(label), 12.0, TEXT2, Align::Left, Weight::Regular);
        let (mine, other) = if i == 3 { (app.cfg.pad_radial_ok, app.cfg.pad_radial_back) } else { (app.cfg.pad_radial_back, app.cfg.pad_radial_ok) };
        let mut v = mine.min(5) as usize;
        if ui.select_pad(key, c, &btns, &crate::sys::radial::PICK_BUTTONS, gstyle, &mut v) && v as u8 != mine {
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
        app.cfg.pad_radial_keep |= app.cfg.pad_radial_two && app.cfg.pad_radial[8..16].iter().any(|&a| a == GROUP || a == PROFILES);
        changed = true;
    }
    ui.tip(check, "Um círculo em cada stick, com 8 lugares cada. O stick que apontar por último escolhe.");
    // Nada, e todas as ações menos o próprio radial.
    let ids: Vec<u8> = std::iter::once(EMPTY).chain((0..remap::ACTIONS.len() as u8).filter(|&a| a != remap::RADIAL)).chain([remap::MACRO_REC]).collect();
    let names: Vec<String> = ids.iter().map(|&a| if a == EMPTY { crate::i18n::tr("Nada").into_owned() } else { super::pad_profiles::action_choice(a) }).collect();
    // Nos lugares (não nas 4 opções), também a troca de perfil.
    // Logo depois de "Nada", para não sumir no fim da lista das ações.
    let top_ids: Vec<u8> = [EMPTY, PROFILES].into_iter().chain(ids[1..].iter().copied()).collect();
    let top_names: Vec<String> = [names[0].clone(), tr("Trocar perfil")].into_iter().chain(names[1..].iter().cloned()).collect();
    // Lugares 0 a 7 no stick direito; com dois círculos, 8 a 15 no esquerdo.
    // Um círculo: escolher o stick que aponta; dois: um título para cada stick.
    if !two {
        let (l, c) = row(9);
        ui.c.text_fit(l, &tr("Stick"), 12.0, TEXT2, Align::Left, Weight::Regular);
        let mut s = app.cfg.pad_radial_left as usize;
        if ui.select("pad-radial-left", c, &[tr("Direito"), tr("Esquerdo")], &mut s) {
            app.cfg.pad_radial_left = s == 1;
            // Lugares de 4 opções passam para o stick esquerdo: a escolha fica com ele solto.
            app.cfg.pad_radial_keep |= s == 1 && app.cfg.pad_radial[..8].iter().any(|&a| a == GROUP || a == PROFILES);
            changed = true;
        }
    }
    let mut y = r.y + if two { 480.0 } else { 502.0 };
    for i in 0..n {
        if two && i % 8 == 0 {
            ui.c.text_fit(rect(r.x + 16.0, y, r.w - 32.0, 20.0), &tr(if i == 0 { "Stick direito" } else { "Stick esquerdo" }), 12.0, MUTED, Align::Left, Weight::Semibold);
            y += 22.0;
        } else if i == 0 {
            y += 22.0;
        }
        // Uma escolha por linha: em duas colunas os nomes das ações não cabiam.
        let cx = r.x + 16.0;
        ui.c.text_raw(rect(cx, y, 22.0, 34.0), DIRS[i % 8], 16.0, MUTED, Align::Left, Weight::Semibold);
        let was = app.cfg.pad_radial[i];
        // Caixa que liga as 4 opções do direcional neste lugar.
        let gw = 136.0_f32.min((r.w - 80.0) * 0.4);
        let sw = r.w - 56.0 - gw - 8.0;
        changed |= slot_select(ui, app, &top_ids, &top_names, i, rect(cx + 24.0, y, sw, 34.0));
        let gr = rect(cx + 24.0 + sw + 8.0, y, gw, 34.0);
        let mut g = was == GROUP;
        if ui.check(&format!("pad-radial-g{i}"), gr, "4 opções", &mut g) {
            app.cfg.pad_radial[i] = if g { GROUP } else { EMPTY };
            changed = true;
        }
        ui.tip(gr, "O direcional escolhe uma de quatro ações dentro deste lugar. Ligado, o lugar deixa de ter ação própria.");
        // Ficou de 4 opções num lugar do stick esquerdo: a escolha precisa ficar com o stick solto,
        // senão o polegar não chega ao direcional.
        if was != app.cfg.pad_radial[i] && matches!(app.cfg.pad_radial[i], GROUP | PROFILES) && (i >= 8 || app.cfg.pad_radial_left) {
            app.cfg.pad_radial_keep = true;
        }
        y += 42.0;
        if app.cfg.pad_radial[i] == GROUP {
            for (d, arrow) in ["↑", "→", "↓", "←"].into_iter().enumerate() {
                ui.c.text_raw(rect(cx + 24.0, y, 22.0, 34.0), arrow, 14.0, MUTED, Align::Left, Weight::Semibold);
                changed |= slot_select(ui, app, &ids, &names, sub(i, d), rect(cx + 48.0, y, r.w - 80.0, 34.0));
                y += 42.0;
            }
        }
    }
    if changed {
        app.cfg.save();
        app.apply_pads();
    }
    r.bottom() + 12.0
}

/// Cartão do teclado do Prisma: um stick só ou os dois, e em qual deles ficam as letras.
fn keyboard(ui: &mut Ui, app: &mut App, at: Rect) -> f32 {
    let two = app.cfg.pad_osk_two;
    let repeat = app.cfg.pad_osk_repeat;
    let r = rect(at.x, at.y, at.w, 166.0 + if repeat { 42.0 } else { 0.0 } + if two { 42.0 } else { 0.0 });
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Teclado do Prisma");
    let check = rect(r.x + 10.0, r.y + 44.0, r.w - 20.0, 36.0);
    let mut changed = ui.check("pad-osk-two", check, "Usar os dois sticks", &mut app.cfg.pad_osk_two);
    ui.tip(check, "Um círculo para as letras e outro para os números e símbolos. No stick esquerdo, A, B, X e Y digitam; no direito, o direcional digita, e com ele solto o direcional são as setas.");
    let dodge = rect(r.x + 10.0, r.y + 80.0, r.w - 20.0, 36.0);
    changed |= ui.check("pad-osk-dodge", dodge, "Fugir do cursor", &mut app.cfg.pad_osk_dodge);
    ui.tip(dodge, "Quando o cursor de texto fica atrás do teclado, ele sobe para o alto da tela, e desce de novo quando o cursor sai dali.");
    let rep = rect(r.x + 10.0, r.y + 116.0, r.w - 20.0, 36.0);
    changed |= ui.check("pad-osk-repeat", rep, "Repetir ao segurar", &mut app.cfg.pad_osk_repeat);
    ui.tip(rep, "Segurar qualquer botão repete a tecla, como num teclado, depois da espera. Desligado, só apagar e as setas repetem.");
    let mut y = r.y + 160.0;
    if repeat {
        let l = rect(r.x + 16.0, y, r.w - 172.0, 34.0);
        ui.c.text_fit(l, &crate::i18n::tr("Espera para repetir"), 12.0, TEXT2, Align::Left, Weight::Regular);
        changed |= ui.stepper("pad-osk-repeat-ms", rect(r.right() - 140.0, y, 124.0, 34.0), &mut app.cfg.pad_osk_repeat_ms, (200, 2000), 50, "ms");
        y += 42.0;
    }
    if two {
        let (l, c) = (rect(r.x + 16.0, y, 108.0, 34.0), rect(r.x + 128.0, y, r.w - 144.0, 34.0));
        ui.c.text_fit(l, &crate::i18n::tr("Letras no stick"), 12.0, TEXT2, Align::Left, Weight::Regular);
        let mut s = app.cfg.pad_osk_letters_right as usize;
        if ui.select("pad-osk-letters", c, &[crate::i18n::tr("Esquerdo").into_owned(), crate::i18n::tr("Direito").into_owned()], &mut s) {
            app.cfg.pad_osk_letters_right = s == 1;
            changed = true;
        }
    }
    if changed {
        app.cfg.save();
        app.apply_pads();
    }
    r.bottom() + 12.0
}

/// Escolha do lugar ou opção `i` do radial: uma ação, "4 opções…" (só nos 16 lugares) ou um
/// programa. Devolve se mudou; o programa só muda quando a escolha volta da lista ou do arquivo.
fn slot_select(ui: &mut Ui, app: &mut App, ids: &[u8], names: &[String], i: usize, at: Rect) -> bool {
    use super::picker::Target;
    use crate::sys::radial::{GROUP, PROGRAM};
    let tr = |s: &str| crate::i18n::tr(s).into_owned();
    let cur = app.cfg.pad_radial[i];
    // Com as 4 opções ligadas, o lugar não tem ação própria: a caixa fica parada.
    if cur == GROUP {
        ui.c.round(at, 8.0, CARD2);
        ui.c.round_line(at, 8.0, BORDER, 1.0);
        ui.c.text_fit(rect(at.x + 10.0, at.y, at.w - 20.0, at.h), &tr("4 opções no direcional"), 14.0, MUTED, Align::Left, Weight::Regular);
        return false;
    }
    let mut opts = names.to_vec();
    if cur == PROGRAM {
        opts.push(app.cfg.pad_radial_prog[i].0.clone());
    }
    let pick_at = opts.len();
    opts.push(tr(if cur == PROGRAM { "Escolher outro programa…" } else { "Abrir um programa…" }));
    let mut sel = match cur {
        PROGRAM => pick_at - 1,
        a => ids.iter().position(|&x| x == a).unwrap_or(0),
    };
    let menu = format!("pad-radial-run-{i}");
    let mut changed = false;
    if ui.select(&format!("pad-radial-{i}"), at, &opts, &mut sel) {
        if sel == pick_at {
            ui.menu(&menu, &[("Da lista de programas", true), ("Escolher arquivo .exe", true)]);
        } else if sel < ids.len() {
            changed = cur != ids[sel];
            app.cfg.pad_radial[i] = ids[sel];
        }
    }
    match ui.menu_pick(&menu) {
        Some(0) => app.open_picker(Target::RadialProgram(i as u8)),
        Some(_) => app.pick_exe_for(Target::RadialProgram(i as u8)),
        None => {}
    }
    changed
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_counts_presses_and_flags_double() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        let mut t = Test::default();
        let press = |b: pads::Buttons, lt: f32| pads::Input { buttons: b, lt, ..Default::default() };
        test_feed(&mut t, "a", press(pads::A, 0.0), ms(0));
        test_feed(&mut t, "a", press(0, 0.0), ms(120));
        // Apertou de novo 30 ms depois de soltar: aperto duplo.
        test_feed(&mut t, "a", press(pads::A, 0.0), ms(150));
        test_feed(&mut t, "a", press(0, 0.9), ms(200));
        test_feed(&mut t, "a", press(0, 0.0), ms(300));
        assert_eq!(t.seen, pads::A | remap::LT);
        assert_eq!(t.hist, vec![(remap::LT, 100, false), (pads::A, 50, true), (pads::A, 120, false)]);
        // Outro controle começa do zero.
        test_feed(&mut t, "b", press(0, 0.0), ms(400));
        assert_eq!((t.seen, t.hist.len()), (0, 0));
    }
}
