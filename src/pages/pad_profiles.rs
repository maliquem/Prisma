//! Editor dos perfis do mapeamento (Fase 2b). Ocupa a coluna da direita da aba Controles.
//! Cada mudança grava o arquivo do perfil na hora e o motor passa a usar a versão nova.

use super::*;
use crate::config::{Hotkey, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};
use crate::sys::pads::{self, Family};
use crate::sys::macros;
use crate::sys::remap::{self, Bind, Layer, Macro, Mode, Out, Profile, Step, Stick, When, Zone};

#[derive(Default)]
pub struct Editor {
    prof: usize,
    layer: usize,
    button: u32,
    when: usize,
    with: usize,
    kind: usize,
    val: usize,
    hk: Hotkey,
    game: usize,
    /// Programa escolhido para "Abrir programa".
    run: String,
    /// Modelo escolhido: 1 a 4 esperam a pergunta; o último é o assistente do Acessível.
    tpl: usize,
    /// Respostas do assistente: lado, clique, velocidade.
    acc: [usize; 3],
    /// Perfil importado esperando: programas a confirmar, os recusados e se a pergunta está aberta.
    import: Option<(Profile, Vec<u8>, Vec<u8>, bool)>,
    /// Macro escolhida, se o gravador está ligado para ela, e o passo novo (tipo, tecla, clique, espera).
    mac: usize,
    rec: bool,
    skind: usize,
    shk: Hotkey,
    sval: usize,
    swait: u32,
}

/// O perfil aberto no editor, para exportar.
pub fn current(app: &App) -> Option<Profile> {
    let ed = app.pads.edit.as_ref()?;
    remap::profiles().get(ed.prof).cloned()
}

/// Perfil lido de um arquivo. Vem de fora: cada programa que ele abre é perguntado antes de gravar.
pub fn imported(app: &mut App, mut p: Profile) {
    let Some(ed) = app.pads.edit.as_mut() else { return };
    let list = remap::profiles();
    while list.iter().any(|x| x.name == p.name) {
        p.name = tr("{} (importado)").replace("{}", &p.name);
    }
    let mut runs: Vec<u8> = vec![];
    for b in p.layers.iter().flat_map(|l| &l.binds) {
        if let Out::Run(i) = b.out {
            if !runs.contains(&i) {
                runs.push(i);
            }
        }
    }
    ed.import = Some((p, runs, vec![], false));
}

/// Fotos de teste: o formulário em Ação do Prisma (a mais longa), Abrir programa ou o assistente.
pub fn test_target(app: &mut App, what: &str) {
    if let Some(ed) = app.pads.edit.as_mut() {
        match what {
            "run" => {
                ed.kind = 8;
                ed.run = "C:/Windows/System32/notepad.exe".into();
            }
            "keyboard" => ed.kind = 9,
            "macro" => ed.kind = 10,
            "wizard" => {
                ed.tpl = remap::TEMPLATES.len() + 1;
                ed.acc = [1, 1, 2];
            }
            _ => {
                ed.kind = 6;
                ed.val = 18;
            }
        }
    }
}

/// Caminho que o seletor de programa devolveu.
pub fn set_program(app: &mut App, path: &str) {
    if let Some(ed) = app.pads.edit.as_mut() {
        ed.run = path.to_string();
    }
}

const WHENS: [(When, &str); 6] = [
    (When::Press, "Pressionar"),
    (When::Tap, "Tocar"),
    (When::Hold, "Segurar"),
    (When::Double, "Duplo"),
    (When::Release, "Soltar"),
    (When::Latch, "Alternar"),
];
const SPEEDS_T: [&str; 4] = ["Turbo 2x", "Turbo 3x", "Turbo 4x", "Turbo 5x"];
const SIDES: [&str; 3] = ["Os dois lados", "Só o lado esquerdo", "Só o lado direito"];
const CLICKS: [&str; 2] = ["Segurar para arrastar", "Clique liga e desliga"];
const SPEEDS: [(&str, f32); 3] = [("Mouse devagar", 800.0), ("Mouse normal", 1600.0), ("Mouse rápido", 2600.0)];
const KINDS: [&str; 11] = ["Tecla", "Tecla especial", "Mouse", "Roda", "Camada enquanto segura", "Liga e desliga camada", "Ação do Prisma", "Trocar de perfil", "Abrir programa", "Teclado do Prisma", "Macro"];
/// Na ordem de `remap::MODES`.
const MODE_NAMES: [&str; 3] = ["Uma vez", "Repetir enquanto segura", "Repetir até apertar de novo"];
const STEP_KINDS: [&str; 3] = ["Tecla", "Clique", "Espera"];
/// Teclas que o campo de tecla não pega (Esc cancela) ou que não existem no teclado comum.
const SPECIAL: [(u16, &str); 12] = [
    (0x1B, "Esc"),
    (0x0D, "Enter"),
    (0x09, "Tab"),
    (0x5B, "Windows"),
    (0xAF, "Volume +"),
    (0xAE, "Volume −"),
    (0xAD, "Sem som"),
    (0xB3, "Tocar ou pausar"),
    (0xB0, "Próxima faixa"),
    (0xB1, "Faixa anterior"),
    (0xA6, "Voltar no navegador"),
    (0xA7, "Avançar no navegador"),
];
const MOUSE: [&str; 5] = ["Clique esquerdo", "Clique direito", "Clique do meio", "Botão lateral 1", "Botão lateral 2"];
const WHEEL: [&str; 2] = ["Roda para cima", "Roda para baixo"];

fn tr(s: &str) -> String {
    crate::i18n::tr(s).into_owned()
}

/// Botões que o editor mostra, com o nome do controle escolhido.
fn buttons(f: Family) -> Vec<(u32, String)> {
    let mut v: Vec<(u32, String)> = [(pads::UP, "↑"), (pads::DOWN, "↓"), (pads::LEFT, "←"), (pads::RIGHT, "→")].iter().map(|(b, n)| (*b, n.to_string())).collect();
    if f == Family::Generic {
        let order = [pads::A, pads::B, pads::X, pads::Y, pads::LB, pads::RB, pads::BACK, pads::START, pads::LS, pads::RS];
        v.extend(order.iter().enumerate().map(|(i, b)| (*b, (i + 1).to_string())));
    } else {
        v.extend(super::controllers::button_names(f).iter().map(|(b, n)| (*b, tr(n))));
    }
    let sony = matches!(f, Family::DualSense | Family::Ds4);
    v.push((remap::LT, if sony { "L2" } else { "LT" }.into()));
    v.push((remap::RT, if sony { "R2" } else { "RT" }.into()));
    for (b, n) in [(remap::LSU, "LS ↑"), (remap::LSD, "LS ↓"), (remap::LSL, "LS ←"), (remap::LSR, "LS →"), (remap::RSU, "RS ↑"), (remap::RSD, "RS ↓"), (remap::RSL, "RS ←"), (remap::RSR, "RS →")] {
        v.push((b, n.into()));
    }
    v
}

/// Nome do botão no controle desta família.
pub fn button_label(f: Family, bit: u32) -> String {
    buttons(f).into_iter().find(|b| b.0 == bit).map_or_else(|| remap::button_name(bit).to_string(), |b| b.1)
}

fn key_label(vk: u16, m: u8) -> String {
    if m == 0 {
        if let Some((_, n)) = SPECIAL.iter().find(|s| s.0 == vk) {
            return tr(n);
        }
    }
    let mut mods = 0;
    for (a, b) in [(remap::CTRL, MOD_CTRL), (remap::SHIFT, MOD_SHIFT), (remap::ALT, MOD_ALT), (remap::WIN, MOD_WIN)] {
        if m & a != 0 {
            mods |= b;
        }
    }
    Hotkey::new(mods, vk as u32).label()
}

fn layer_name(p: &Profile, n: u8) -> String {
    p.layers.get(n as usize).map_or_else(|| n.to_string(), |l| l.name.clone())
}

fn describe(o: Out, p: &Profile) -> String {
    match o {
        Out::Key(vk, m) => key_label(vk, m),
        Out::Mouse(n) => tr(MOUSE[(n as usize).clamp(1, 5) - 1]),
        Out::Wheel(n) => tr(WHEEL[if n > 0 { 0 } else { 1 }]),
        Out::Layer(n) => tr("Camada {} enquanto segura").replace("{}", &layer_name(p, n)),
        Out::Toggle(n) => tr("Liga e desliga a camada {}").replace("{}", &layer_name(p, n)),
        Out::Action(n) if n >= remap::SOUND => tr("Combinação de som {}").replace("{}", &(n - remap::SOUND + 1).to_string()),
        Out::Action(n) => remap::ACTIONS.get(n as usize).map_or_else(String::new, |a| tr(a)),
        Out::Profile(n) => tr("Trocar para o perfil {}").replace("{}", p.strings.get(n as usize).map_or("", |s| s)),
        Out::Run(n) => tr("Abrir {}").replace("{}", &p.strings.get(n as usize).map_or_else(String::new, |s| crate::sys::procs::exe_name(s))),
        Out::Macro(n) => tr("Macro {}").replace("{}", p.macros.get(n as usize).map_or("", |m| &m.name)),
    }
}

/// Tecla do campo de tecla, com os modificadores do mapeamento.
fn key_out(hk: &Hotkey) -> Option<Out> {
    (hk.vk != 0).then(|| {
        let mut m = 0;
        for (a, b) in [(MOD_CTRL, remap::CTRL), (MOD_SHIFT, remap::SHIFT), (MOD_ALT, remap::ALT), (MOD_WIN, remap::WIN)] {
            if hk.mods & a != 0 {
                m |= b;
            }
        }
        Out::Key(hk.vk as u16, m)
    })
}

fn step_text(s: Step, p: &Profile) -> String {
    match s {
        Step::Down(o @ Out::Wheel(_)) => describe(o, p),
        Step::Down(o) => tr("Aperta {}").replace("{}", &describe(o, p)),
        Step::Up(o) => tr("Solta {}").replace("{}", &describe(o, p)),
        Step::Wait(_) => tr("Espera"),
    }
}

/// Apaga a macro `i`: as ligações dela saem e as das macros seguintes descem um número.
fn remove_macro(p: &mut Profile, i: usize) {
    p.macros.remove(i);
    for l in &mut p.layers {
        l.binds.retain(|b| b.out != Out::Macro(i as u8));
        for b in &mut l.binds {
            if let Out::Macro(n) = &mut b.out {
                if *n as usize > i {
                    *n -= 1;
                }
            }
        }
    }
}

/// Pergunta antes de trocar o perfil pelo modelo: as ligações dele se perdem.
fn ask_template(ui: &mut Ui, tpl: &str, prof: &str) {
    ui.ask(
        "pp-tpl-ok",
        &tr("Aplicar o modelo {}?").replace("{}", tpl),
        &tr("As camadas, ligações e zonas mortas do perfil {} serão trocadas pelas do modelo. Os jogos ligados a ele continuam.").replace("{}", prof),
    );
}

/// Troca o conteúdo do perfil pelo do modelo, mantendo nome e jogos.
fn apply(p: &mut Profile, t: Profile) {
    *p = Profile { name: std::mem::take(&mut p.name), games: std::mem::take(&mut p.games), ..t };
}

/// Quanto o editor ocupa, para a rolagem.
fn content_h(binds: usize, chip_rows: usize, wizard: bool, steps: Option<usize>) -> f32 {
    let macros = 32.0 + 44.0 + steps.map_or(0.0, |n| 44.0 + 28.0 + n as f32 * 40.0 + 44.0);
    (if wizard { 3.0 * 44.0 } else { 0.0 }) + 30.0 + 5.0 * 44.0 + 2.0 * 44.0 + 32.0 + 4.0 * 40.0 + 36.0 + chip_rows as f32 * 32.0 + 36.0 + binds.max(1) as f32 * 40.0 + 40.0 + 2.0 * 44.0 + 24.0 + 40.0 + macros
}

pub fn draw(ui: &mut Ui, app: &mut App, r: Rect, family: Family) {
    let mut list = remap::profiles();
    let Some(ed) = app.pads.edit.as_mut() else { return };
    // Importado: um programa por vez; fechar a pergunta sem Sim recusa aquele programa.
    if let Some((imp, queue, refused, open)) = ed.import.as_mut() {
        if *open && !ui.asking() {
            let i = queue.remove(0);
            if !ui.answered("pp-imp") {
                refused.push(i);
            }
            *open = false;
        }
        if !*open {
            if let Some(&i) = queue.first() {
                let path = imp.strings.get(i as usize).cloned().unwrap_or_default();
                ui.ask("pp-imp", &tr("O perfil {} quer abrir um programa. Permitir?").replace("{}", &imp.name), &path);
                *open = true;
            } else {
                let (mut p, _, refused, _) = ed.import.take().unwrap();
                for l in &mut p.layers {
                    l.binds.retain(|b| !matches!(b.out, Out::Run(i) if refused.contains(&i)));
                }
                if remap::save(&p) {
                    ed.prof = remap::profiles().iter().position(|x| x.name == p.name).unwrap_or(0);
                    ed.layer = 0;
                    app.toast.info(&format!("Perfil {} importado", p.name));
                } else {
                    app.toast.info("Não deu para salvar o perfil.");
                }
                return;
            }
        }
    }
    let Some(ed) = app.pads.edit.as_mut() else { return };
    ed.prof = ed.prof.min(list.len() - 1);
    ui.card(r);
    let x = r.x + 16.0;
    let w = r.w - 32.0;

    // Cabeçalho: perfil e fechar; embaixo, novo perfil para um jogo e apagar.
    let mut y = r.y + 12.0;
    let close_w = 96.0;
    let title_w = crate::ui::gfx::measure(&tr("Perfis"), 15.0, Weight::Semibold, None).0 + 16.0;
    ui.title(rect(x, y + 6.0, title_w, 22.0), "Perfis");
    let names: Vec<String> = list.iter().map(|p| p.name.clone()).collect();
    if ui.select("pp-prof", rect(x + title_w, y, (w - title_w - close_w - 8.0).min(260.0), 34.0), &names, &mut ed.prof) {
        ed.layer = 0;
    }
    if ui.button("pp-close", rect(r.right() - 16.0 - close_w, y, close_w, 34.0), "Fechar", false) {
        if ed.rec {
            macros::record_stop();
            app.toast.hide();
        }
        app.pads.edit = None;
        return;
    }
    y += 44.0;
    let half = ((w - 8.0) / 2.0).floor();
    let free: Vec<&crate::config::MeterGame> = app.cfg.meter_games.iter().filter(|g| !list.iter().any(|p| p.games.iter().any(|e| e.eq_ignore_ascii_case(&g.exe)))).collect();
    let mut games = vec![tr("Novo perfil para jogo")];
    games.extend(free.iter().map(|g| g.exe.clone()));
    if ui.select("pp-game", rect(x, y, half, 34.0), &games, &mut ed.game) && ed.game > 0 {
        let g = free[ed.game - 1];
        let mut p = Profile::base();
        p.name = g.exe.trim_end_matches(".exe").trim_end_matches(".EXE").to_string();
        p.games = vec![g.exe.to_lowercase()];
        if remap::save(&p) {
            list = remap::profiles();
            ed.prof = list.iter().position(|x| x.name == p.name).unwrap_or(0);
            ed.layer = 0;
        } else {
            app.toast.info("Não deu para salvar o perfil.");
        }
        ed.game = 0;
    }
    let mut p = list[ed.prof].clone();
    let builtin = p.name == remap::DESKTOP || p.name == remap::BASE;
    let del = if builtin { "Voltar ao de fábrica" } else { "Apagar perfil" };
    if ui.button_ex("pp-del", rect(x + half + 8.0, y, half, 34.0), del, false, !builtin || remap::has_file(&p.name)) {
        remap::remove(&p.name);
        if !builtin {
            ed.prof = 0;
        }
        ed.layer = 0;
        return;
    }
    y += 44.0;

    // Modelos, importar e exportar.
    let quarter = ((w - 24.0) / 4.0).floor();
    let mut tpls = vec![tr("Aplicar modelo")];
    tpls.extend(remap::TEMPLATES.iter().chain(["Acessível"].iter()).map(|t| tr(t)));
    let acc_i = tpls.len() - 1;
    if ui.select("pp-tpl", rect(x, y, 2.0 * quarter + 8.0, 34.0), &tpls, &mut ed.tpl) && ed.tpl > 0 && ed.tpl < acc_i {
        ask_template(ui, &tpls[ed.tpl], &p.name);
    }
    if ui.button("pp-in", rect(x + 2.0 * (quarter + 8.0), y, quarter, 34.0), "Importar", false) {
        app.pad_profile_file(false);
    }
    if ui.button("pp-out", rect(x + 3.0 * (quarter + 8.0), y, quarter, 34.0), "Exportar", false) {
        app.pad_profile_file(true);
    }
    let Some(ed) = app.pads.edit.as_mut() else { return };
    if ed.tpl > 0 && ed.tpl < acc_i && !ui.asking() {
        if ui.answered("pp-tpl-ok") {
            apply(&mut p, Profile::template(ed.tpl - 1));
            if !remap::save(&p) {
                app.toast.info("Não deu para salvar o perfil.");
            }
            ed.layer = 0;
        }
        ed.tpl = 0;
    }
    y += 52.0;

    ed.layer = ed.layer.min(p.layers.len() - 1);
    let chips = buttons(family);
    if !chips.iter().any(|c| c.0 == ed.button) {
        ed.button = chips[4].0;
    }
    let own: Vec<Bind> = p.layers[ed.layer].binds.iter().filter(|b| b.button == ed.button).cloned().collect();
    // Linhas de botões: estimativa pela largura média, só para a rolagem.
    let chip_rows = (chips.len() as f32 * 64.0 / w).ceil() as usize + 1;
    let body = rect(r.x, y, r.w - 8.0, r.bottom() - y - 8.0);
    // Partes iguais em três: camada, assistente e o formulário de ligação.
    let third = ((w - 16.0) / 3.0).floor();
    let wizard = ed.tpl == acc_i;
    ed.mac = ed.mac.min(p.macros.len().saturating_sub(1));
    ui.begin_scroll("pp-body", body, content_h(own.len(), chip_rows, wizard, p.macros.get(ed.mac).map(|m| m.steps.len())));
    let mut changed = false;

    // Assistente do Acessível: três perguntas e Aplicar.
    if wizard {
        ui.c.text_fit(rect(x, y, w, 26.0), &tr("Acessível: escolha o lado, o clique e a velocidade"), 14.0, TEXT, Align::Left, Weight::Semibold);
        y += 34.0;
        let sides: Vec<String> = SIDES.iter().map(|s| tr(s)).collect();
        let clicks: Vec<String> = CLICKS.iter().map(|s| tr(s)).collect();
        let speeds: Vec<String> = SPEEDS.iter().map(|s| tr(s.0)).collect();
        ui.select("pp-acc0", rect(x, y, third, 34.0), &sides, &mut ed.acc[0]);
        ui.select("pp-acc1", rect(x + third + 8.0, y, third, 34.0), &clicks, &mut ed.acc[1]);
        ui.select("pp-acc2", rect(x + 2.0 * (third + 8.0), y, third, 34.0), &speeds, &mut ed.acc[2]);
        y += 44.0;
        if ui.button("pp-acc-no", rect(x + third + 8.0, y, third, 34.0), "Cancelar", false) {
            ed.tpl = 0;
        }
        if ui.button_ex("pp-acc-go", rect(x + 2.0 * (third + 8.0), y, third, 34.0), "Aplicar", true, true) {
            ask_template(ui, &tpls[acc_i], &p.name);
        }
        if ui.answered("pp-tpl-ok") {
            let a = ed.acc;
            apply(&mut p, Profile::accessible(a[0] as u8, a[1] == 1, SPEEDS[a[2].min(2)].1));
            ed.tpl = 0;
            ed.layer = 0;
            changed = true;
        }
        y += 54.0;
    }

    let layers: Vec<String> = p.layers.iter().map(|l| l.name.clone()).collect();
    ui.select("pp-layer", rect(x, y, third, 34.0), &layers, &mut ed.layer);
    if ui.button_ex("pp-layer-add", rect(x + third + 8.0, y, third, 34.0), "Nova camada", false, p.layers.len() < 16) {
        let n = p.layers.len();
        p.layers.push(Layer { name: format!("{} {n}", tr("Camada")), parent: Some(0), ls: None, rs: None, binds: vec![] });
        ed.layer = n;
        changed = true;
    }
    let last = p.layers.len() - 1;
    if ui.button_ex("pp-layer-del", rect(x + 2.0 * (third + 8.0), y, third, 34.0), "Apagar camada", false, ed.layer > 0 && ed.layer == last && !changed) {
        // Só a última: as outras camadas e ligações não mudam de número.
        p.layers.pop();
        for l in &mut p.layers {
            l.binds.retain(|b| !matches!(b.out, Out::Layer(n) | Out::Toggle(n) if n as usize == last));
        }
        ed.layer = 0;
        changed = true;
    }
    y += 44.0;

    // Sticks da camada.
    for (right, label) in [(false, "Stick esquerdo"), (true, "Stick direito")] {
        let layer = &mut p.layers[ed.layer];
        let cur = if right { layer.rs } else { layer.ls };
        let base = ed.layer > 0;
        let mut opts = vec![];
        if base {
            opts.push(tr("Igual à camada mãe"));
        }
        opts.extend(["Direções como botões", "Mouse", "Rolar"].iter().map(|s| tr(s)));
        let mut sel = match cur {
            None => 0,
            Some(Stick::Buttons) => 0,
            Some(Stick::Mouse(_)) => 1,
            Some(Stick::Scroll(_)) => 2,
        } + if base && cur.is_some() { 1 } else { 0 };
        ui.label(rect(x, y, 120.0, 34.0), label, TEXT2);
        if ui.select(if right { "pp-rs" } else { "pp-ls" }, rect(x + 130.0, y, 220.0, 34.0), &opts, &mut sel) {
            let k = if base { sel as i32 - 1 } else { sel as i32 };
            let st = match k {
                -1 => None,
                0 => Some(Stick::Buttons),
                1 => Some(Stick::Mouse(1600.0)),
                _ => Some(Stick::Scroll(12.0)),
            };
            if right {
                layer.rs = st;
            } else {
                layer.ls = st;
            }
            changed = true;
        }
        let key = if right { "pp-rs-v" } else { "pp-ls-v" };
        let speed = rect(x + 360.0, y, 170.0, 34.0);
        let slot = if right { &mut layer.rs } else { &mut layer.ls };
        match slot {
            Some(Stick::Mouse(v)) => {
                let mut n = *v as u32;
                if ui.stepper(key, speed, &mut n, (200, 6000), 100, "px/s") {
                    *v = n as f32;
                    changed = true;
                }
            }
            Some(Stick::Scroll(v)) => {
                let mut n = *v as u32;
                if ui.stepper(key, speed, &mut n, (1, 60), 1, "/s") {
                    *v = n as f32;
                    changed = true;
                }
            }
            _ => {}
        }
        y += 44.0;
    }

    // Zonas mortas: valem para o perfil todo.
    ui.title(rect(x, y + 4.0, w, 22.0), "Zona morta");
    y += 32.0;
    for (i, label) in ["Stick esquerdo", "Stick direito", "Gatilho esquerdo", "Gatilho direito"].iter().enumerate() {
        let z = p.zones[i];
        let (mut a, mut b) = ((z.start * 100.0).round() as u32, (z.end * 100.0).round() as u32);
        ui.label(rect(x, y, 140.0, 34.0), label, TEXT2);
        let ca = ui.stepper(&format!("pp-z{i}a"), rect(x + 150.0, y, 130.0, 34.0), &mut a, (0, 50), 1, "%");
        ui.label(rect(x + 290.0, y, 30.0, 34.0), "a", MUTED);
        let cb = ui.stepper(&format!("pp-z{i}b"), rect(x + 320.0, y, 130.0, 34.0), &mut b, (55, 100), 1, "%");
        if ca || cb {
            p.zones[i] = Zone { start: a as f32 / 100.0, end: b as f32 / 100.0 };
            changed = true;
        }
        y += 40.0;
    }
    ui.c.text_fit(rect(x, y, w, 20.0), &tr("Abaixo do primeiro valor conta como parado; a partir do segundo, como fim do curso."), 12.0, MUTED, Align::Left, Weight::Regular);
    y += 36.0;

    // Botões: clique em um para ver e mudar o que ele faz nesta camada.
    // Com desenho próprio, cada botão é o ícone dele; no genérico, o número.
    let glyphs = family != Family::Generic;
    let style = crate::ui::glyph::Style::of(family, 0);
    let (mut bx, mut by) = (x, y);
    for (bit, label) in &chips {
        let cw = if glyphs { crate::ui::glyph::width(*bit, 18.0) + 18.0 } else { crate::ui::gfx::measure(label, 12.0, Weight::Semibold, None).0 + 22.0 };
        if bx + cw > x + w {
            bx = x;
            by += 32.0;
        }
        let c = rect(bx, by, cw, 26.0);
        let on = *bit == ed.button;
        let used = p.layers[ed.layer].binds.iter().any(|b| b.button == *bit);
        ui.c.round(c, 6.0, if on { ACCENT } else { CARD2 });
        if used && !on {
            ui.c.round_line(c, 6.0, ACCENT, 1.0);
        }
        if glyphs {
            crate::ui::glyph::draw(ui.c, c.x + 9.0, c.y + 13.0, 18.0, style, *bit, if on { ON_ACCENT } else { TEXT2 });
        } else {
            ui.c.text_raw(c, label, 12.0, if on { ON_ACCENT } else { TEXT2 }, Align::Center, Weight::Semibold);
        }
        ui.tip(c, label);
        if ui.hit(&format!("pp-b{bit}"), c) {
            ed.button = *bit;
            ui.s.needs_redraw = true;
        }
        bx += cw + 6.0;
    }
    y = by + 40.0;

    // O que o botão escolhido faz.
    let name = chips.iter().find(|c| c.0 == ed.button).map_or(String::new(), |c| c.1.clone());
    ui.c.text_fit(rect(x, y, w, 22.0), &tr("{}: o que faz nesta camada").replace("{}", &name), 14.0, TEXT, Align::Left, Weight::Semibold);
    y += 30.0;
    if own.is_empty() {
        // Sem ligação aqui: mostra o que vem da camada mãe.
        let mut from = p.layers[ed.layer].parent;
        let mut text = tr("Nada.");
        while let Some(l) = from {
            let got: Vec<&Bind> = p.layers[l as usize].binds.iter().filter(|b| b.button == ed.button && b.with == 0).collect();
            if !got.is_empty() {
                let what: Vec<String> = got.iter().map(|b| format!("{} → {}", tr(WHENS.iter().find(|w| w.0 == b.when).map_or("", |w| w.1)), describe(b.out, &p))).collect();
                text = tr("Vem da camada {}: ").replace("{}", &p.layers[l as usize].name) + &what.join(", ");
                break;
            }
            from = p.layers[l as usize].parent;
        }
        ui.c.text_fit(rect(x, y, w, 34.0), &text, 13.0, MUTED, Align::Left, Weight::Regular);
        y += 40.0;
    }
    let mut drop = None;
    for (i, b) in own.iter().enumerate() {
        let mut t = format!("{} → {}", tr(WHENS.iter().find(|w| w.0 == b.when).map_or("", |w| w.1)), describe(b.out, &p));
        if b.with != 0 {
            let with: Vec<String> = chips.iter().filter(|c| b.with & c.0 != 0).map(|c| c.1.clone()).collect();
            t += &tr(", junto com {}").replace("{}", &with.join(" + "));
        }
        ui.c.text_fit(rect(x, y, w - 130.0, 34.0), &t, 13.0, TEXT, Align::Left, Weight::Regular);
        if ui.button(&format!("pp-rm{i}"), rect(x + w - 120.0, y, 120.0, 32.0), "Remover", false) {
            drop = Some(b.clone());
        }
        y += 40.0;
    }
    if let Some(d) = drop {
        let l = &mut p.layers[ed.layer];
        if let Some(i) = l.binds.iter().position(|b| *b == d) {
            l.binds.remove(i);
            changed = true;
        }
    }
    if own.iter().any(|b| matches!(b.when, When::Tap | When::Double)) {
        ui.c.text_fit(rect(x, y, w, 20.0), &tr("Tocar e duplo só respondem depois que o botão solta: até 0,3 s a mais."), 12.0, WARN, Align::Left, Weight::Regular);
    }
    y += 32.0;

    // Nova ligação: quando, junto com qual botão, e o destino.
    let whens: Vec<String> = WHENS.iter().map(|w| tr(w.1)).collect();
    ui.select("pp-when", rect(x, y, third, 34.0), &whens, &mut ed.when);
    let others: Vec<&(u32, String)> = chips.iter().filter(|c| c.0 != ed.button && c.0 & 0x1ffff != 0).collect();
    let mut withs = vec![tr("Sozinho")];
    withs.extend(others.iter().map(|c| tr("Junto com {}").replace("{}", &c.1)));
    ed.with = ed.with.min(withs.len() - 1);
    ui.select("pp-with", rect(x + third + 8.0, y, third, 34.0), &withs, &mut ed.with);
    let kinds: Vec<String> = KINDS.iter().map(|k| tr(k)).collect();
    if ui.select("pp-kind", rect(x + 2.0 * (third + 8.0), y, third, 34.0), &kinds, &mut ed.kind) {
        ed.val = 0;
    }
    y += 44.0;
    let vr = rect(x, y, 2.0 * third + 8.0, 34.0);
    let mut pick = None;
    let sounds = app.cfg.snd_keys.len().min(crate::app::SND_KEYS_MAX);
    let others_p: Vec<String> = list.iter().map(|o| o.name.clone()).filter(|n| *n != p.name).collect();
    let values: Vec<String> = match ed.kind {
        1 => SPECIAL.iter().map(|s| tr(s.1)).collect(),
        2 => MOUSE.iter().map(|s| tr(s)).collect(),
        3 => WHEEL.iter().map(|s| tr(s)).collect(),
        4 | 5 => layers.clone(),
        6 => remap::ACTIONS.iter().map(|a| tr(a)).chain((1..=sounds).map(|n| tr("Combinação de som {}").replace("{}", &n.to_string()))).collect(),
        7 => others_p.clone(),
        10 => p.macros.iter().map(|m| m.name.clone()).collect(),
        _ => vec![],
    };
    if ed.kind == 0 {
        ui.hotkey("pp-key", vr, &mut ed.hk);
    } else if ed.kind == 8 {
        let half_v = ((vr.w - 8.0) / 2.0).floor();
        let shown = if ed.run.is_empty() { tr("Nenhum programa escolhido") } else { crate::sys::procs::exe_name(&ed.run) };
        ui.c.text_fit(rect(vr.x, vr.y, half_v, vr.h), &shown, 13.0, if ed.run.is_empty() { MUTED } else { TEXT }, Align::Left, Weight::Regular);
        pick = Some(rect(vr.x + half_v + 8.0, vr.y, half_v, vr.h));
    } else if ed.kind == 9 {
        ui.c.text_fit(vr, &tr("Abre e fecha o teclado na tela"), 13.0, MUTED, Align::Left, Weight::Regular);
    } else if values.is_empty() {
        ui.c.text_fit(vr, &tr("Nenhuma macro: crie uma em Macros, mais abaixo"), 13.0, MUTED, Align::Left, Weight::Regular);
    } else {
        ed.val = ed.val.min(values.len().saturating_sub(1));
        ui.select("pp-val", vr, &values, &mut ed.val);
    }
    let out = match ed.kind {
        0 => key_out(&ed.hk),
        1 => Some(Out::Key(SPECIAL[ed.val.min(SPECIAL.len() - 1)].0, 0)),
        2 => Some(Out::Mouse(ed.val as u8 + 1)),
        3 => Some(Out::Wheel(if ed.val == 0 { 1 } else { -1 })),
        4 => Some(Out::Layer(ed.val as u8)),
        5 => Some(Out::Toggle(ed.val as u8)),
        6 => Some(Out::Action(if ed.val < remap::ACTIONS.len() { ed.val as u8 } else { remap::SOUND + (ed.val - remap::ACTIONS.len()) as u8 })),
        // Perfil e programa só entram na lista de textos do perfil ao adicionar.
        7 => others_p.get(ed.val).map(|_| Out::Profile(0)),
        9 => Some(Out::Action(remap::KEYBOARD)),
        10 => p.macros.get(ed.val).map(|_| Out::Macro(ed.val as u8)),
        _ => (!ed.run.is_empty()).then_some(Out::Run(0)),
    };
    if ui.button_ex("pp-add", rect(x + 2.0 * (third + 8.0), y, third, 34.0), "Adicionar", true, out.is_some()) {
        let out = match out {
            Some(Out::Profile(_)) => remap::intern(&mut p.strings, &others_p[ed.val]).map(Out::Profile),
            Some(Out::Run(_)) => remap::intern(&mut p.strings, &ed.run).map(Out::Run),
            o => o,
        };
        if let Some(out) = out {
            let with = if ed.with == 0 { 0 } else { others[ed.with - 1].0 };
            p.layers[ed.layer].binds.push(Bind { button: ed.button, with, when: WHENS[ed.when].0, out });
            changed = true;
        }
    }
    // Combinação que já faz outra coisa: avisa, sem impedir.
    if ed.with != 0 {
        let combo = ed.button | others[ed.with - 1].0;
        if let Some(by) = remap::combo_conflict(&p, combo, false) {
            ui.c.text_fit(rect(x, y + 40.0, w, 20.0), &tr("Essa combinação já é usada por {}.").replace("{}", &by), 12.0, WARN, Align::Left, Weight::Regular);
        }
    }
    y += 70.0;

    // Turbo do botão escolhido: à parte das ligações, vale em todas as camadas do perfil.
    ui.c.text_fit(rect(x, y, w, 22.0), &tr("Turbo do botão {}").replace("{}", &name), 14.0, TEXT, Align::Left, Weight::Semibold);
    y += 30.0;
    let ti = p.turbo.iter().position(|t| t.button == ed.button);
    let mut t = ti.map_or_else(|| remap::Turbo::new(ed.button), |i| p.turbo[i]);
    let before = t;
    ui.check("pp-t-on", rect(x, y, third, 34.0), "Ligado", &mut t.on);
    let speeds: Vec<String> = SPEEDS_T.iter().map(|s| tr(s)).collect();
    let mut sp = (t.speed.clamp(2, 5) - 2) as usize;
    if ui.select("pp-t-speed", rect(x + third + 8.0, y, third, 34.0), &speeds, &mut sp) {
        t.speed = sp as u8 + 2;
    }
    let mut d = t.delay as u32;
    if ui.stepper("pp-t-delay", rect(x + 2.0 * (third + 8.0), y, third, 34.0), &mut d, (0, 2000), 50, "ms") {
        t.delay = d as u16;
    }
    if t != before {
        match ti {
            Some(i) => p.turbo[i] = t,
            None => p.turbo.push(t),
        }
        changed = true;
    }
    y += 44.0;
    ui.c.text_fit(rect(x, y, w, 20.0), &tr("Espera, em milissegundos, antes de o turbo começar a repetir."), 12.0, MUTED, Align::Left, Weight::Regular);
    y += 28.0;

    // Atalho: segurar estes botões e apertar outro liga ou desliga o turbo dele.
    ui.c.text_fit(rect(x, y, third, 34.0), &tr("Atalho do turbo"), 13.0, TEXT2, Align::Left, Weight::Regular);
    let dirs = remap::LSU | remap::LSD | remap::LSL | remap::LSR | remap::RSU | remap::RSD | remap::RSL | remap::RSR;
    let keyable: Vec<&(u32, String)> = chips.iter().filter(|c| c.0 & dirs == 0).collect();
    let mut firsts = vec![tr("Sem atalho")];
    firsts.extend(keyable.iter().map(|c| c.1.clone()));
    let mut seconds = vec![tr("Nenhum")];
    seconds.extend(keyable.iter().map(|c| c.1.clone()));
    let bits: Vec<usize> = keyable.iter().enumerate().filter(|(_, c)| p.turbo_key & c.0 != 0).map(|(i, _)| i + 1).collect();
    let (mut k1, mut k2) = (bits.first().copied().unwrap_or(0), bits.get(1).copied().unwrap_or(0));
    let c1 = ui.select("pp-tk1", rect(x + third + 8.0, y, third, 34.0), &firsts, &mut k1);
    let c2 = ui.select("pp-tk2", rect(x + 2.0 * (third + 8.0), y, third, 34.0), &seconds, &mut k2);
    if c1 || c2 {
        let bit = |k: usize| if k == 0 { 0 } else { keyable[k - 1].0 };
        p.turbo_key = if k1 == 0 { 0 } else { bit(k1) | bit(k2) };
        changed = true;
    }
    y += 40.0;
    let warn = if p.turbo_key == 0 { None } else { remap::combo_conflict(&p, p.turbo_key, true).map(|by| tr("Esse atalho já é usado por {}.").replace("{}", &by)) };
    let note = warn.clone().unwrap_or_else(|| if p.turbo_key == 0 { tr("Sem atalho: o turbo só muda aqui.") } else { tr("Segure o atalho e aperte o botão para ligar ou desligar o turbo dele.") });
    ui.c.text_fit(rect(x, y, w, 20.0), &note, 12.0, if warn.is_some() { WARN } else { MUTED }, Align::Left, Weight::Regular);
    y += 40.0;

    // Macros do perfil: escolher, criar, apagar, o modo, gravar e os passos.
    let mut rec_note = None;
    if ed.rec && !macros::recording() {
        ed.rec = false;
    }
    ui.title(rect(x, y + 4.0, w, 22.0), "Macros");
    y += 32.0;
    let names: Vec<String> = p.macros.iter().map(|m| m.name.clone()).collect();
    if names.is_empty() {
        ui.c.text_fit(rect(x, y, third, 34.0), &tr("Nenhuma macro."), 13.0, MUTED, Align::Left, Weight::Regular);
    } else if !ed.rec {
        ui.select("pp-mac", rect(x, y, third, 34.0), &names, &mut ed.mac);
    } else {
        ui.c.text_fit(rect(x, y, third, 34.0), &names[ed.mac], 13.0, TEXT, Align::Left, Weight::Regular);
    }
    if ui.button_ex("pp-mac-add", rect(x + third + 8.0, y, third, 34.0), "Nova macro", false, !ed.rec && p.macros.len() < remap::MAX_MACROS) {
        let mut n = p.macros.len() + 1;
        while p.macros.iter().any(|m| m.name == format!("{} {n}", tr("Macro"))) {
            n += 1;
        }
        p.macros.push(Macro { name: format!("{} {n}", tr("Macro")), mode: Mode::Once, steps: vec![] });
        ed.mac = p.macros.len() - 1;
        changed = true;
    }
    if ui.button_ex("pp-mac-del", rect(x + 2.0 * (third + 8.0), y, third, 34.0), "Apagar macro", false, !ed.rec && !p.macros.is_empty()) {
        remove_macro(&mut p, ed.mac);
        ed.mac = 0;
        changed = true;
    }
    y += 44.0;
    if ed.mac < p.macros.len() {
        let modes: Vec<String> = MODE_NAMES.iter().map(|s| tr(s)).collect();
        let mut mi = remap::MODES.iter().position(|x| x.0 == p.macros[ed.mac].mode).unwrap_or(0);
        if ui.select("pp-mac-mode", rect(x, y, 2.0 * third + 8.0, 34.0), &modes, &mut mi) {
            p.macros[ed.mac].mode = remap::MODES[mi].0;
            changed = true;
        }
        if ui.button_ex("pp-mac-rec", rect(x + 2.0 * (third + 8.0), y, third, 34.0), if ed.rec { "Parar" } else { "Gravar" }, ed.rec, true) {
            if ed.rec {
                p.macros[ed.mac].steps = macros::record_stop();
                ed.rec = false;
                rec_note = Some(false);
                changed = true;
            } else {
                macros::record_start();
                ed.rec = true;
                rec_note = Some(true);
            }
        }
        y += 44.0;
        let n = p.macros[ed.mac].steps.len();
        let status = if ed.rec { tr("Gravando: use o teclado e o mouse em outro programa e volte aqui para parar. O movimento do mouse não entra.") } else { tr("{} passos. Gravar troca os passos pelos novos.").replace("{}", &n.to_string()) };
        ui.c.text_fit(rect(x, y, w, 20.0), &status, 12.0, if ed.rec { WARN } else { MUTED }, Align::Left, Weight::Regular);
        y += 28.0;
        let mut drop = None;
        for i in 0..n {
            let s = p.macros[ed.mac].steps[i];
            ui.c.text_fit(rect(x, y, 2.0 * third + 8.0 - 150.0, 34.0), &step_text(s, &p), 13.0, TEXT, Align::Left, Weight::Regular);
            if let Step::Wait(ms) = s {
                let mut v = ms;
                if ui.stepper(&format!("pp-ms{i}"), rect(x + 2.0 * third + 8.0 - 140.0, y, 140.0, 34.0), &mut v, (10, remap::MAX_WAIT), 10, "ms") {
                    p.macros[ed.mac].steps[i] = Step::Wait(v);
                    changed = true;
                }
            }
            if ui.button_ex(&format!("pp-mrm{i}"), rect(x + 2.0 * (third + 8.0), y, third, 32.0), "Remover", false, !ed.rec) {
                drop = Some(i);
            }
            y += 40.0;
        }
        if let Some(i) = drop {
            p.macros[ed.mac].steps.remove(i);
            changed = true;
        }
        // Passo novo no fim: tecla ou clique (aperta e solta) ou espera.
        let kinds: Vec<String> = STEP_KINDS.iter().map(|s| tr(s)).collect();
        ui.select("pp-skind", rect(x, y, third, 34.0), &kinds, &mut ed.skind);
        let vr = rect(x + third + 8.0, y, third, 34.0);
        let add = match ed.skind {
            0 => {
                ui.hotkey("pp-skey", vr, &mut ed.shk);
                key_out(&ed.shk).map(|o| vec![Step::Down(o), Step::Up(o)])
            }
            1 => {
                let clicks: Vec<String> = MOUSE.iter().map(|s| tr(s)).collect();
                ed.sval = ed.sval.min(clicks.len() - 1);
                ui.select("pp-sclick", vr, &clicks, &mut ed.sval);
                let o = Out::Mouse(ed.sval as u8 + 1);
                Some(vec![Step::Down(o), Step::Up(o)])
            }
            _ => {
                ed.swait = ed.swait.clamp(10, remap::MAX_WAIT);
                ui.stepper("pp-swait", vr, &mut ed.swait, (10, remap::MAX_WAIT), 10, "ms");
                Some(vec![Step::Wait(ed.swait)])
            }
        };
        let room = n + 2 <= remap::MAX_STEPS;
        if ui.button_ex("pp-sadd", rect(x + 2.0 * (third + 8.0), y, third, 34.0), "Adicionar passo", true, !ed.rec && room && add.is_some()) {
            p.macros[ed.mac].steps.extend(add.unwrap_or_default());
            changed = true;
        }
    }

    // Depois do último uso de `ed`: o aviso e o botão precisam da App inteira.
    match rec_note {
        Some(true) => app.toast.keep("Gravando macro: volte ao Prisma e clique em Parar"),
        Some(false) => {
            app.toast.hide();
            app.toast.info("Macro gravada");
        }
        None => {}
    }
    if let Some(r) = pick {
        super::picker::add_button(ui, app, r, super::picker::Target::PadProgram);
    }
    ui.end_scroll();

    if changed && !remap::save(&p) {
        app.toast.info("Não deu para salvar o perfil.");
    }
}
