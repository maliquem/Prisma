//! Editor dos perfis do mapeamento (Fase 2b). Ocupa a coluna da direita da aba Controles.
//! Cada mudança grava o arquivo do perfil na hora e o motor passa a usar a versão nova.

use super::*;
use crate::config::{Hotkey, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};
use crate::sys::pads::{self, Family};
use crate::sys::macros;
use crate::sys::remap::{self, Batt, Bind, Light, Motion, Layer, Macro, Mode, Out, Profile, Step, Stick, Trig, When, Zone};

#[derive(Default)]
pub struct Editor {
    prof: usize,
    /// Perfil em uso no controle escolhido, da última vez que o editor foi atrás dele.
    follow: Option<String>,
    layer: usize,
    button: pads::Buttons,
    when: usize,
    with: usize,
    kind: usize,
    val: usize,
    hk: Hotkey,
    /// Dando nome a um perfil: 0 renomeia o aberto, 1 cria uma cópia dele, 2 cria um de fábrica;
    /// e o texto.
    naming: Option<(u8, String)>,
    /// "Voltar ao de fábrica" esperando a resposta da pergunta.
    reset: bool,
    /// Programa escolhido para "Abrir programa" e o link de "Abrir link".
    run: String,
    url: String,
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
    /// Origem das ligações: 0 botões do controle, 1 teclado e mouse; a tecla escolhida
    /// (`KEY_SOURCES`, 0 é a do campo `khk`) e o "junto com" dela (0 sozinho, 1 a tecla do campo
    /// `kwhk`, depois `KEY_SOURCES`).
    src: usize,
    ksrc: usize,
    khk: Hotkey,
    kwith: usize,
    kwhk: Hotkey,
    /// Texto rápido novo: abreviação e texto.
    tabbr: String,
    ttext: String,
    /// Altura que o editor ocupou no desenho anterior, para a rolagem.
    body_h: f32,
    /// Janela da barra de luz pela bateria, aberta.
    batt: bool,
    /// Janela do seletor de cor: qual cor está sendo mexida e a matiz, saturação e brilho dela.
    /// 0 é a cor do perfil; 1, 2 e 3 são a carga vazia, a metade e a cheia.
    pick: Option<(u8, [f32; 3])>,
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
    // `|` separa os perfis em `pad_fixed` no config.
    p.name.retain(|c| c != '|');
    while list.iter().any(|x| remap::same_file(&x.name, &p.name)) {
        p.name = tr("{} (importado)").replace("{}", &p.name);
    }
    let mut runs: Vec<u8> = vec![];
    for b in p.layers.iter().flat_map(|l| l.binds.iter().chain(&l.keys)) {
        if let Out::Run(i) = b.out {
            if !runs.contains(&i) {
                runs.push(i);
            }
        }
    }
    ed.import = Some((p, runs, vec![], false));
}

/// Fotos de teste: o formulário em Ação do Prisma (a mais longa), Abrir programa ou o assistente.
#[cfg(debug_assertions)]
pub fn test_target(app: &mut App, what: &str) {
    if let Some(ed) = app.pads.edit.as_mut() {
        match what {
            "run" => {
                ed.kind = 8;
                ed.run = "C:/Windows/System32/notepad.exe".into();
            }
            "keyboard" => ed.kind = 9,
            "link" => {
                ed.kind = 12;
                ed.url = "https://github.com/maliquem/Prisma".into();
            }
            "macro" => {
                ed.kind = 10;
                app.pads.tab = 1;
            }
            "settings" => app.pads.tab = 3,
            "radial" => app.pads.tab = 2,
            // As duas janelas da barra de luz, para a foto de teste.
            "color" => ed.pick = Some((0, [210.0, 1.0, 1.0])),
            "battery" => ed.batt = true,
            // Luz em Piscar: a lista fechada mostra o nome do efeito, com a amostra e o botão.
            "blink" => {
                let mut list = remap::profiles();
                if let Some(p) = list.get_mut(ed.prof) {
                    p.light = Some(Light::Blink(remap::FIRST_COLOR));
                    remap::save(p);
                }
            }
            "stick" => ed.button = pads::LS,
            "trigger" => {
                ed.button = remap::LT;
                // Com um efeito escolhido, a foto pega também as linhas de ajuste do gatilho.
                let mut list = remap::profiles();
                if let Some(p) = list.get_mut(ed.prof) {
                    p.trig[0] = Trig::Buzz { start: 3, power: 8, hz: 40 };
                    p.buzz = 2;
                    remap::save(p);
                }
            }
            "gyro" => ed.layer = 1,
            // A linha de perfis dando nome a um perfil novo, e com o primeiro perfil da pasta aberto
            // (a foto põe dois perfis no mesmo programa para mostrar o padrão).
            "naming" => ed.naming = Some((1, "Corrida".into())),
            "shared" => ed.prof = 2,
            "text" => {
                app.pads.tab = 1;
                ed.kind = 11;
                ed.tabbr = "vlw".into();
                ed.ttext = "Valeu, até a próxima!".into();
            }
            "keys" => {
                ed.src = 1;
                ed.ksrc = 8;
                ed.kind = 6;
            }
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

/// Botão escolhido no editor (0 com a origem Teclado e mouse), para o desenho contornar.
pub fn chosen(app: &App) -> pads::Buttons {
    app.pads.edit.as_ref().map_or(0, |e| if e.src == 0 { e.button } else { 0 })
}

/// Clique num botão do desenho: o editor passa para ele, com a origem nos botões do controle.
pub fn choose(app: &mut App, bit: pads::Buttons) {
    if let Some(e) = app.pads.edit.as_mut() {
        e.src = 0;
        e.set_button(bit);
    }
}

impl Editor {
    /// Outro botão: a ligação nova começa do padrão, sem o que foi escolhido para o anterior.
    fn set_button(&mut self, bit: pads::Buttons) {
        if self.button != bit {
            *self = Editor { button: bit, when: 0, with: 0, kind: 0, val: 0, hk: Hotkey::default(), run: String::new(), url: String::new(), ..std::mem::take(self) };
        }
    }
}

/// A camada escolhida no editor: o nome e uma linha por ligação (botão, quando e o que faz).
pub fn layer_changes(app: &App, f: Family) -> Option<(String, Vec<String>)> {
    let ed = app.pads.edit.as_ref()?;
    let list = remap::profiles();
    let p = list.get(ed.prof)?;
    let l = p.layers.get(ed.layer)?;
    let lines = l
        .binds
        .iter()
        .map(|b| {
            let mut who = button_label(f, b.button);
            if b.with != 0 {
                who = format!("{} + {who}", button_label(f, b.with));
            }
            let when = tr(WHENS.iter().find(|w| w.0 == b.when).map_or("", |w| w.1));
            format!("{who}: {when} → {}", describe(b.out, p))
        })
        .collect();
    Some((layer_name(p, ed.layer as u8), lines))
}

/// Lugar `i` da linha Perfis (`r` é a linha toda); o 3, depois do Mais, é o "Abre sozinho com".
pub fn slot(r: Rect, i: usize) -> Rect {
    let title_w = crate::ui::gfx::measure(&tr("Perfis"), 15.0, Weight::Semibold, None).0 + 16.0;
    // Larguras em partes: a lista dos perfis, o "Abre sozinho com" e o jeito de abrir o programa têm os textos mais longos.
    const PARTS: [f32; 6] = [1.3, 0.9, 0.8, 2.0, 1.2, 1.3];
    let unit = (r.w - title_w - 40.0) / PARTS.iter().sum::<f32>();
    let left: f32 = PARTS[..i].iter().sum();
    rect((r.x + title_w + left * unit + i as f32 * 8.0).floor(), r.y, (PARTS[i] * unit).floor(), 34.0)
}

/// Escrevendo o nome de um perfil: a linha Perfis está ocupada pelo campo.
pub fn naming(app: &App) -> bool {
    app.pads.edit.as_ref().is_some_and(|e| e.naming.is_some())
}

/// "Abre sozinho com", na linha Perfis: o jogo cujo foco troca para o perfil aberto no editor.
/// Desktop e Base não têm jogo: dizem quando valem.
pub fn opens_with(ui: &mut Ui, app: &mut App, r: Rect) {
    let list = remap::profiles();
    let Some(ed) = app.pads.edit.as_mut() else { return };
    let Some(p) = list.get(ed.prof) else { return };
    let label = tr("Abre sozinho com");
    let sw = (r.w * 0.55).min(260.0);
    let sr = rect(r.right() - sw, r.y, sw, r.h);
    ui.c.text_fit(rect(r.x, r.y, r.w - sw - 8.0, r.h), &label, 13.0, MUTED, Align::Right, Weight::Regular);
    let fixed = match p.name.as_str() {
        n if n == remap::DESKTOP => Some("Fora dos jogos"),
        n if n == remap::BASE => Some("Jogos sem perfil próprio"),
        _ if ed.rec => Some(""),
        _ => None,
    };
    if let Some(f) = fixed {
        let shown = if f.is_empty() { p.games.first().cloned().unwrap_or_else(|| tr("Nenhum programa")) } else { tr(f) };
        ui.c.text_fit(sr, &shown, 13.0, TEXT, Align::Left, Weight::Regular);
        return;
    }
    // shortcut: mostra e troca só o primeiro programa; um perfil com vários (vindo de arquivo) fica com o escolhido.
    let cur = p.games.first().cloned();
    let mut exes: Vec<String> = cur.iter().cloned().collect();
    // Todos os jogos do Medidor: um programa pode ter vários perfis.
    for g in &app.cfg.meter_games {
        let e = g.exe.to_lowercase();
        if !exes.contains(&e) {
            exes.push(e);
        }
    }
    let mut opts = vec![tr("Nenhum programa")];
    opts.extend(exes.iter().cloned());
    opts.push(tr("Escolher programa"));
    opts.push(tr("Outros programas…"));
    let mut sel = usize::from(cur.is_some());
    if !ui.select("pp-opens", sr, &opts, &mut sel) {
        return;
    }
    if sel == opts.len() - 2 {
        app.open_picker(super::picker::Target::PadGame);
        return;
    }
    // Qualquer .exe pelo Explorer; a resposta chega em `set_game`.
    if sel == opts.len() - 1 {
        app.pick_exe_for(super::picker::Target::PadGame);
        return;
    }
    let mut p = p.clone();
    p.games = if sel == 0 { vec![] } else { vec![exes[sel - 1].clone()] };
    if !remap::save(&p) {
        app.toast.info("Não deu para salvar o perfil.");
    }
}

/// Programa escolhido para "Abre sozinho com": passa a abrir o perfil aberto (sem sair dos outros).
pub fn set_game(app: &mut App, path: &str) {
    let Some(ed) = app.pads.edit.as_ref() else { return };
    let exe = crate::sys::procs::exe_name(path).to_lowercase();
    let Some(mut p) = remap::profiles().get(ed.prof).cloned() else { return };
    // Os outros perfis do programa continuam com ele: um programa pode ter vários perfis.
    p.games = vec![exe];
    if !remap::save(&p) {
        app.toast.info("Não deu para salvar o perfil.");
    }
}

/// Nomes das camadas do perfil aberto e a escolhida, para as fichas do desenho; só com mais de uma.
pub fn layers(app: &App) -> Option<(Vec<String>, usize)> {
    let ed = app.pads.edit.as_ref()?;
    let list = remap::profiles();
    let p = list.get(ed.prof)?;
    (p.layers.len() > 1).then(|| (p.layers.iter().map(|l| l.name.clone()).collect(), ed.layer.min(p.layers.len() - 1)))
}

pub fn set_layer(app: &mut App, i: usize) {
    if let Some(ed) = app.pads.edit.as_mut() {
        ed.layer = i;
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
const KINDS: [&str; 16] = ["Tecla", "Tecla especial", "Mouse", "Roda", "Camada enquanto segura", "Liga e desliga camada", "Ação do Prisma", "Trocar de perfil", "Abrir programa", "Teclado do Prisma", "Macro", "Texto rápido", "Abrir link", "Radial do Prisma", "Desconectar o Bluetooth", "Mostrar a bateria"];
/// Destinos 9 e 13 a 15: ações da seção Controles, com a frase que fica no lugar da lista.
const PAD_KINDS: [(usize, u8, &str); 4] = [
    (9, remap::KEYBOARD, "Abre e fecha o teclado na tela"),
    (13, remap::RADIAL, "Abre o radial enquanto o botão fica segurado"),
    (14, 24, "Desconecta este controle do Bluetooth"),
    (15, 25, "Mostra a bateria deste controle"),
];

/// Nome de uma ação do Prisma nas listas, com a página de onde vem antes: "(Medidor) Medidor compacto".
pub fn action_choice(n: u8) -> String {
    let name = if n >= remap::SOUND { tr("Combinação de som {}").replace("{}", &(n - remap::SOUND + 1).to_string()) } else { tr(remap::action_name(n)) };
    match remap::action_section(n) {
        Some(sec) => format!("({}) {name}", tr(sec)),
        None => name,
    }
}
/// Na ordem de `remap::MODES`.
const MODE_NAMES: [&str; 3] = ["Uma vez", "Repetir enquanto segura", "Repetir até apertar de novo"];
const STEP_KINDS: [&str; 3] = ["Tecla", "Clique", "Espera"];
/// Origens que o campo de tecla não pega (modificadores sozinhos, Esc, apagar) e os botões do mouse
/// que podem mudar. A primeira é "a tecla do campo".
const KEY_SOURCES: [(u16, &str); 13] = [
    (0, "Tecla"),
    (0xA2, "Ctrl esquerdo"),
    (0xA3, "Ctrl direito"),
    (0xA0, "Shift esquerdo"),
    (0xA1, "Shift direito"),
    (0xA4, "Alt esquerdo"),
    (0xA5, "Alt direito"),
    (0x5B, "Windows"),
    (0x1B, "Esc"),
    (0x08, "Backspace"),
    (4, "Botão do meio do mouse"),
    (5, "Botão lateral 1 do mouse"),
    (6, "Botão lateral 2 do mouse"),
];

/// Nome de uma tecla de origem (ou botão do mouse).
fn src_name(vk: pads::Buttons) -> String {
    match KEY_SOURCES.iter().skip(1).find(|s| s.0 as pads::Buttons == vk) {
        Some(s) => tr(s.1),
        None if vk == 0 => tr("Nenhuma tecla"),
        None => key_label(vk as u16, 0),
    }
}
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
fn buttons(f: Family) -> Vec<(pads::Buttons, String)> {
    let mut v: Vec<(pads::Buttons, String)> = [(pads::UP, "↑"), (pads::DOWN, "↓"), (pads::LEFT, "←"), (pads::RIGHT, "→")].iter().map(|(b, n)| (*b, n.to_string())).collect();
    if f == Family::Generic {
        v.extend((0..pads::generic_buttons()).map(|n| (pads::generic_bit(n), (n + 1).to_string())));
    } else {
        v.extend(super::controllers::button_names(f).iter().map(|(b, n)| (*b, tr(n))));
    }
    let sony = matches!(f, Family::DualSense | Family::Ds4);
    v.push((remap::LT, if sony { "L2" } else { "LT" }.into()));
    v.push((remap::RT, if sony { "R2" } else { "RT" }.into()));
    for (b, n) in [(remap::LTF, if sony { "L2" } else { "LT" }), (remap::RTF, if sony { "R2" } else { "RT" })] {
        v.push((b, format!("{n} {}", tr("até o fim"))));
    }
    for (b, n) in [(remap::LSU, "LS ↑"), (remap::LSD, "LS ↓"), (remap::LSL, "LS ←"), (remap::LSR, "LS →"), (remap::RSU, "RS ↑"), (remap::RSD, "RS ↓"), (remap::RSL, "RS ←"), (remap::RSR, "RS →")] {
        v.push((b, n.into()));
    }
    if sony {
        for (b, n) in [(remap::TPU, "Touchpad ↑"), (remap::TPD, "Touchpad ↓"), (remap::TPL, "Touchpad ←"), (remap::TPR, "Touchpad →")] {
            v.push((b, n.into()));
        }
        for (b, n) in [(remap::TPTAP, "Toque no touchpad"), (remap::TPCL, "Clique na metade esquerda"), (remap::TPCR, "Clique na metade direita")] {
            v.push((b, tr(n)));
        }
        for (b, a) in [(remap::TPUL, "↖"), (remap::TPUR, "↗"), (remap::TPDL, "↙"), (remap::TPDR, "↘")] {
            v.push((b, format!("{} {a}", tr("Canto do touchpad"))));
        }
    }
    v
}

/// Nome do botão no controle desta família.
pub fn button_label(f: Family, bit: pads::Buttons) -> String {
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
        Out::Action(n) => tr(remap::action_name(n)),
        Out::Profile(n) => tr("Trocar para o perfil {}").replace("{}", p.strings.get(n as usize).map_or("", |s| s)),
        Out::Run(n) => tr("Abrir {}").replace("{}", &p.strings.get(n as usize).map_or_else(String::new, |s| if remap::link_ok(s) { s.clone() } else { crate::sys::procs::exe_name(s) })),
        Out::Macro(n) => tr("Macro {}").replace("{}", p.macros.get(n as usize).map_or("", |m| &m.name)),
        Out::Text(n) => tr("Digitar {}").replace("{}", &p.texts.get(n as usize).map_or_else(String::new, short)),
        Out::Pad(_) => out_label(o, None),
    }
}

/// Tecla, clique, roda ou botão do controle `pad` (família e vid; sem ele, os nomes do Xbox): o
/// passo de macro e o aviso da gravação.
pub fn out_label(o: Out, pad: Option<(Family, u16)>) -> String {
    match o {
        Out::Pad(b) => button_label(pad.map_or(Family::Xbox, |x| x.0), 1 << b),
        Out::Key(vk, m) => key_label(vk, m),
        Out::Mouse(n) => tr(MOUSE[(n as usize).clamp(1, 5) - 1]),
        Out::Wheel(n) => tr(WHEEL[if n > 0 { 0 } else { 1 }]),
        _ => String::new(),
    }
}

/// Texto rápido numa linha curta: a abreviação, ou o começo do texto.
fn short(t: &remap::Snip) -> String {
    if !t.abbr.is_empty() {
        return t.abbr.clone();
    }
    let mut s: String = t.text.chars().take(24).collect();
    if t.text.chars().count() > 24 {
        s.push('…');
    }
    s
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

fn step_text(s: Step, p: &Profile, pad: Option<(Family, u16)>) -> String {
    let d = |o: Out| if let Out::Pad(_) = o { out_label(o, pad) } else { describe(o, p) };
    match s {
        Step::Down(o @ Out::Wheel(_)) => d(o),
        Step::Down(o) => tr("Aperta {}").replace("{}", &d(o)),
        Step::Up(o) => tr("Solta {}").replace("{}", &d(o)),
        Step::Wait(_) => tr("Espera"),
    }
}

/// Apaga a macro `i`: as ligações dela saem e as das macros seguintes descem um número.
fn remove_macro(p: &mut Profile, i: usize) {
    p.macros.remove(i);
    drop_out(p, Out::Macro(i as u8));
}

/// Tira as ligações de botão e de tecla que vão para `gone` (macro ou texto apagado) e faz as que
/// apontam para os seguintes descerem um número.
fn drop_out(p: &mut Profile, gone: Out) {
    for l in &mut p.layers {
        for v in [&mut l.binds, &mut l.keys] {
            v.retain(|b| b.out != gone);
            for b in v.iter_mut() {
                match (&mut b.out, gone) {
                    (Out::Macro(n), Out::Macro(i)) | (Out::Text(n), Out::Text(i)) if *n > i => *n -= 1,
                    _ => {}
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

/// Uma linha do stick na camada: modo (botões, mouse, rolar ou igual à mãe) e a velocidade.
fn stick_row(ui: &mut Ui, layer: &mut Layer, base: bool, right: bool, x: f32, y: f32, w: f32) -> bool {
    // Seletor e velocidade dividem o que sobra depois do nome, sem passar da largura.
    let sel_w = ((w - 138.0) * 0.56).min(220.0).floor();
    let speed_w = (w - 138.0 - sel_w - 8.0).min(170.0).floor();
    let label = if right { "Stick direito" } else { "Stick esquerdo" };
    let mut changed = false;
    let cur = if right { layer.rs } else { layer.ls };
    let mut opts = vec![];
    if base {
        opts.push(tr("Igual à camada mãe"));
    }
    opts.extend(["Direções como botões", "Mouse", "Rolar", "Flick stick"].iter().map(|s| tr(s)));
    let mut sel = match cur {
        None => 0,
        Some(Stick::Buttons) => 0,
        Some(Stick::Mouse(_)) => 1,
        Some(Stick::Scroll(_)) => 2,
        Some(Stick::Flick(_)) => 3,
    } + if base && cur.is_some() { 1 } else { 0 };
    ui.label(rect(x, y, 120.0, 34.0), label, TEXT2);
    if ui.select(if right { "pp-rs" } else { "pp-ls" }, rect(x + 130.0, y, sel_w, 34.0), &opts, &mut sel) {
        let k = if base { sel as i32 - 1 } else { sel as i32 };
        let st = match k {
            -1 => None,
            0 => Some(Stick::Buttons),
            1 => Some(Stick::Mouse(1600.0)),
            2 => Some(Stick::Scroll(12.0)),
            _ => Some(Stick::Flick(6000.0)),
        };
        if right {
            layer.rs = st;
        } else {
            layer.ls = st;
        }
        changed = true;
    }
    let key = if right { "pp-rs-v" } else { "pp-ls-v" };
    let speed = rect(x + 138.0 + sel_w, y, speed_w, 34.0);
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
        // Pixels por volta inteira: acha-se com o botão Testar volta.
        Some(Stick::Flick(v)) => {
            let mut n = *v as u32;
            if ui.stepper(key, speed, &mut n, (500, 60000), 50, "px") {
                *v = n as f32;
                changed = true;
            }
        }
        _ => {}
    }
    changed
}

/// Gyro ou touchpad na camada: igual à mãe, desligado ou mouse, com a sensibilidade.
fn motion_row(ui: &mut Ui, layer: &mut Layer, touch: bool, base: bool, x: f32, y: f32, w: f32) -> bool {
    let (key, label, slot, def, range, step, unit) = if touch {
        ("pp-touch", "Touchpad", &mut layer.touch, 1200.0, (200, 4000), 100, "px")
    } else {
        ("pp-gyro", "Gyro", &mut layer.gyro, 10.0, (1, 60), 1, "px/°")
    };
    let sel_w = ((w - 138.0) * 0.56).min(220.0).floor();
    let speed_w = (w - 138.0 - sel_w - 8.0).min(170.0).floor();
    let mut opts = vec![];
    if base {
        opts.push(tr("Igual à camada mãe"));
    }
    // "Passa ao jogo" em vez de "Desligado": o jogo lê o gyro e o touchpad direto do controle,
    // que o Prisma não esconde; é o que os jogos com suporte nativo ao DualSense usam.
    opts.extend(["Passa ao jogo", "Mouse"].iter().map(|s| tr(s)));
    if touch {
        opts.extend(["Stick esquerdo", "Stick direito"].iter().map(|s| tr(s)));
    }
    let mut sel = match slot {
        Some(Motion::Mouse(_)) => 1,
        Some(Motion::Stick(s)) => 2 + *s as usize,
        _ => 0,
    } + usize::from(base && slot.is_some());
    ui.label(rect(x, y, 120.0, 34.0), label, TEXT2);
    let mut changed = false;
    ui.tip(rect(x + 130.0, y, sel_w, 34.0), "Passa ao jogo: o Prisma não usa nem esconde, e o jogo lê direto do controle (suporte nativo ao DualSense).");
    if ui.select(key, rect(x + 130.0, y, sel_w, 34.0), &opts, &mut sel) {
        let k = if base { sel as i32 - 1 } else { sel as i32 };
        *slot = match k {
            -1 => None,
            0 => Some(Motion::Off),
            1 => Some(Motion::Mouse(def)),
            k => Some(Motion::Stick(k as u8 - 2)),
        };
        changed = true;
    }
    if let Some(Motion::Mouse(v)) = slot {
        let mut n = v.round() as u32;
        if ui.stepper(&format!("{key}-v"), rect(x + 138.0 + sel_w, y, speed_w, 34.0), &mut n, range, step, unit) {
            *v = n as f32;
            changed = true;
        }
    }
    changed
}

/// Uma linha de zona morta (0 e 1 sticks, 2 e 3 gatilhos): início e fim do curso.
fn zone_row(ui: &mut Ui, zones: &mut [Zone; 4], i: usize, label: &str, x: f32, y: f32) -> bool {
    let z = zones[i];
    let (mut a, mut b) = ((z.start * 100.0).round() as u32, (z.end * 100.0).round() as u32);
    ui.label(rect(x, y, 140.0, 34.0), label, TEXT2);
    let ca = ui.stepper(&format!("pp-z{i}a"), rect(x + 150.0, y, 130.0, 34.0), &mut a, (0, 50), 1, "%");
    ui.label(rect(x + 290.0, y, 30.0, 34.0), "a", MUTED);
    let cb = ui.stepper(&format!("pp-z{i}b"), rect(x + 320.0, y, 130.0, 34.0), &mut b, (55, 100), 1, "%");
    if ca || cb {
        zones[i] = Zone { start: a as f32 / 100.0, end: b as f32 / 100.0 };
        return true;
    }
    false
}

const ZONES: [&str; 4] = ["Stick esquerdo", "Stick direito", "Gatilho esquerdo", "Gatilho direito"];

/// Troca o conteúdo do perfil pelo do modelo, mantendo nome e jogos.
fn apply(p: &mut Profile, t: Profile) {
    *p = Profile { name: std::mem::take(&mut p.name), games: std::mem::take(&mut p.games), ..t };
}

/// Linha do perfil no topo da página: escolher, novo para um jogo, apagar, modelo, importar e exportar.
/// `family`: o controle escolhido na página; perfil novo fica marcado como feito para ele.
pub fn top(ui: &mut Ui, app: &mut App, r: Rect, family: Family, pad: Option<&str>) {
    let list = remap::profiles();
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
                let q = if remap::link_ok(&path) { "O perfil {} quer abrir um link. Permitir?" } else { "O perfil {} quer abrir um programa. Permitir?" };
                ui.ask("pp-imp", &tr(q).replace("{}", &imp.name), &path);
                *open = true;
            } else {
                let (mut p, _, refused, _) = ed.import.take().unwrap();
                for l in &mut p.layers {
                    l.binds.retain(|b| !matches!(b.out, Out::Run(i) if refused.contains(&i)));
                    l.keys.retain(|b| !matches!(b.out, Out::Run(i) if refused.contains(&i)));
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
    // O editor abre no perfil em uso no controle escolhido (o próprio dele ou o do programa) e vai
    // atrás quando ele muda; escolhido outro à mão, fica nele até lá.
    let used = remap::profile_for(pad);
    if used != ed.follow {
        if let Some(i) = used.as_ref().and_then(|n| list.iter().position(|x| x.name == *n)) {
            if i != ed.prof {
                (ed.prof, ed.layer) = (i, 0);
            }
        }
        ed.follow = used;
    }
    ed.prof = ed.prof.min(list.len() - 1);
    // Gravação começada em qualquer lugar (atalho, radial, combinação ou aqui).
    ed.rec = macros::recording();
    let title_w = crate::ui::gfx::measure(&tr("Perfis"), 15.0, Weight::Semibold, None).0 + 16.0;
    let at = |i: usize| slot(r, i);
    ui.title(rect(r.x, r.y + 6.0, title_w, 22.0), "Perfis");
    let names: Vec<String> = list.iter().map(|p| p.name.clone()).collect();
    let mut p = list[ed.prof].clone();
    let builtin = p.name == remap::DESKTOP || p.name == remap::BASE;
    // Dando nome a um perfil novo (cópia do aberto ou de fábrica) ou ao aberto: campo, de onde o
    // novo começa, confirmar e cancelar.
    if let Some((mut mode, mut text)) = ed.naming.take() {
        let rename = mode == 0;
        let (a, b) = (at(0), at(1));
        if rename {
            ui.search("pp-name", rect(a.x, a.y, b.right() - a.x, 34.0), &mut text, "Nome do perfil");
        } else {
            ui.search("pp-name", a, &mut text, "Nome do perfil");
            let mut from = (mode - 1) as usize;
            if ui.select("pp-from", b, &[tr("Cópia de {}").replace("{}", &p.name), tr("De fábrica")], &mut from) {
                mode = from as u8 + 1;
            }
        }
        // Sem `|`: ele separa os perfis em `pad_fixed` no config.
        let name: String = text.trim().chars().filter(|c| !c.is_control() && *c != '|').take(60).collect();
        // Nomes diferentes podem cair no mesmo arquivo; o próprio perfil, ao renomear, não conta.
        let taken = list.iter().any(|x| remap::same_file(&x.name, &name) && !(rename && x.name == p.name));
        // Cancelar com a largura do Criar, no começo do lugar largo; o aviso vem logo depois.
        let (c, w) = (at(3), at(2).w);
        let mut done = ui.button("pp-name-no", rect(c.x, c.y, w, 34.0), "Cancelar", false);
        if taken {
            ui.c.text_fit(rect(c.x + w + 8.0, c.y, at(4).right() - c.x - w - 8.0, 34.0), &tr("Esse nome já existe"), 12.0, WARN, Align::Left, Weight::Regular);
        }
        if ui.button_ex("pp-name-ok", at(2), if rename { "Renomear" } else { "Criar" }, true, !name.is_empty() && !taken) {
            // De fábrica: o Base, sem programa ligado.
            if mode == 2 {
                p = Profile::base();
            }
            let old = std::mem::replace(&mut p.name, name.clone());
            // A cópia não herda o padrão do programa: o padrão continua sendo um só.
            p.main &= rename;
            if !rename {
                p.made_for = if matches!(family, Family::Generic | Family::EightBitDo | Family::Switch) { Family::Xbox } else { family };
            }
            let mut ok = remap::save(&p);
            if ok && rename {
                // Ligações que trocam para o nome antigo passam para o novo.
                for mut o in remap::profiles() {
                    if let Some(x) = o.strings.iter_mut().find(|x| **x == old) {
                        *x = name.clone();
                        ok &= remap::save(&o);
                    }
                }
                // Mesmo arquivo (só maiúsculas ou símbolos mudaram): o save já gravou por cima.
                if !remap::same_file(&old, &name) {
                    remap::remove(&old);
                }
                // Controles com este perfil próprio continuam com ele.
                for e in app.cfg.pad_fixed.iter_mut().filter(|e| e.1 == old) {
                    e.1 = name.clone();
                }
                app.cfg.save();
                app.apply_pads();
            }
            if !ok {
                app.toast.info("Não deu para salvar o perfil.");
            }
            let Some(ed) = app.pads.edit.as_mut() else { return };
            ed.prof = remap::profiles().iter().position(|x| x.name == name).unwrap_or(0);
            ed.layer = 0;
            done = true;
        }
        if !done {
            if let Some(ed) = app.pads.edit.as_mut() {
                ed.naming = Some((mode, text));
            }
        }
        return;
    }
    // Gravando uma macro, o perfil não troca: os passos iriam para outro perfil.
    if ed.rec {
        ui.c.text_fit(at(0), &names[ed.prof], 14.0, TEXT, Align::Left, Weight::Regular);
        return;
    }
    if ui.select("pp-prof", at(0), &names, &mut ed.prof) {
        ed.layer = 0;
    }
    if ui.button("pp-new", at(1), "Novo perfil", false) {
        ed.naming = Some((1, String::new()));
    }
    ui.tip(at(1), "Cria um perfil: cópia do aberto, ligada aos mesmos programas, ou de fábrica.");
    // Mais: o que se faz com o perfil aberto, e os modelos (trocam as ligações pelas do modelo).
    let mut items: Vec<(String, bool)> = vec![
        (tr("Renomear"), !builtin),
        (tr("Importar"), true),
        (tr("Exportar"), true),
        (tr("Voltar ao de fábrica"), !builtin || remap::has_file(&p.name)),
        (tr("Apagar perfil"), !builtin),
    ];
    // Para qual controle o perfil foi feito: o atual fica apagado na lista.
    const KINDS_FOR: [Family; 3] = [Family::DualSense, Family::Ds4, Family::Xbox];
    let first_for = items.len();
    for f in KINDS_FOR {
        let name = remap::made_for_name(f);
        let t = if f == p.made_for { tr("Perfil de {}").replace("{}", name) } else { tr("Marcar como perfil de {}").replace("{}", name) };
        items.push((t, f != p.made_for && !builtin));
    }
    let first_tpl = items.len();
    let tpls: Vec<String> = remap::TEMPLATES.iter().chain(["Acessível"].iter()).map(|t| tr(t)).collect();
    items.extend(tpls.iter().map(|t| (tr("Modelo: {}").replace("{}", t), true)));
    let acc_i = tpls.len();
    if ui.button("pp-more", at(2), "Mais ▾", false) {
        let refs: Vec<(&str, bool)> = items.iter().map(|(t, on)| (t.as_str(), *on)).collect();
        ui.menu("pp-more-menu", &refs);
    }
    match ui.menu_pick("pp-more-menu") {
        Some(0) => ed.naming = Some((0, p.name.clone())),
        Some(1) => return app.pad_profile_file(false),
        Some(2) => return app.pad_profile_file(true),
        Some(3) => {
            ed.reset = true;
            ui.ask(
                "pp-reset-ok",
                &tr("Voltar o perfil {} ao de fábrica?").replace("{}", &p.name),
                &tr("Camadas, ligações, macros e ajustes voltam ao de fábrica. Os programas ligados a ele continuam."),
            );
        }
        Some(4) => {
            remap::remove(&p.name);
            if !builtin {
                ed.prof = 0;
            }
            ed.layer = 0;
            return;
        }
        Some(i) if (first_for..first_tpl).contains(&i) => {
            p.made_for = KINDS_FOR[i - first_for];
            if !remap::save(&p) {
                app.toast.info("Não deu para salvar o perfil.");
            }
        }
        Some(i) if i >= first_tpl => {
            ed.tpl = i - first_tpl + 1;
            if ed.tpl < acc_i {
                ask_template(ui, &tpls[ed.tpl - 1], &p.name);
            }
        }
        _ => {}
    }
    if ed.reset && !ui.asking() {
        if ui.answered("pp-reset-ok") {
            // Desktop e Base: o arquivo sai e volta o de fábrica deles; um perfil seu vira o Base,
            // com o nome e os programas dele.
            if builtin {
                remap::remove(&p.name);
            } else {
                let mut f = Profile::base();
                (f.name, f.games, f.main) = (p.name.clone(), p.games.clone(), p.main);
                if !remap::save(&f) {
                    app.toast.info("Não deu para salvar o perfil.");
                }
            }
        }
        let Some(ed) = app.pads.edit.as_mut() else { return };
        (ed.reset, ed.layer) = (false, 0);
        return;
    }
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
    // O assistente do Acessível fica na aba Mapeamento.
    if ed.tpl == acc_i {
        app.pads.tab = 0;
    }
    // O programa do perfil aberto tem outros perfis: qual é o padrão dele e com qual ele abre.
    if p.games.iter().any(|g| list.iter().any(|o| o.name != p.name && o.games.contains(g))) {
        let mut main = p.main;
        if ui.check("pp-main", at(4), "Padrão do programa", &mut main) {
            set_main(app, &p.name, main);
        }
        ui.tip(at(4), "O programa tem mais de um perfil: o padrão é o que abre com ele.");
        let mut v = app.cfg.pad_reopen_last as usize;
        if ui.select("pp-reopen", at(5), &[tr("Abre com o padrão"), tr("Abre com o último usado")], &mut v) {
            app.cfg.pad_reopen_last = v == 1;
            app.cfg.save();
            app.apply_pads();
        }
    }
}

/// Liga (`on`) ou desliga o perfil `name` como padrão dos seus programas; os outros perfis desses
/// programas deixam de ser o padrão.
fn set_main(app: &mut App, name: &str, on: bool) {
    let list = remap::profiles();
    let Some(games) = list.iter().find(|p| p.name == name).map(|p| p.games.clone()) else { return };
    let mut ok = true;
    for mut p in list {
        let was = p.main;
        p.main = if p.name == name { on } else { p.main && !(on && p.games.iter().any(|g| games.contains(g))) };
        if p.main != was {
            ok &= remap::save(&p);
        }
    }
    if !ok {
        app.toast.info("Não deu para salvar o perfil.");
    }
}

/// O editor do perfil escolhido em `top`: a aba 0 (Mapeamento) tem camadas, sticks, ligações e
/// turbo; a 1 (Macros), as macros e o texto rápido.
pub fn draw(ui: &mut Ui, app: &mut App, r: Rect, family: Family, tab: usize) {
    let list = remap::profiles();
    let Some(ed) = app.pads.edit.as_mut() else { return };
    ed.prof = ed.prof.min(list.len() - 1);
    // Gravação começada em qualquer lugar (atalho, radial, combinação ou aqui).
    ed.rec = macros::recording();
    // Com uma janela aberta, a página atrás dela para de receber cliques.
    if ed.batt || ed.pick.is_some() {
        ui.block();
    }
    let mut p = list[ed.prof].clone();
    let acc_i = remap::TEMPLATES.len() + 1;
    ui.card(r);
    let x = r.x + 16.0;
    let w = r.w - 32.0;
    let mut y = r.y + 12.0;

    ed.layer = ed.layer.min(p.layers.len() - 1);
    let chips = buttons(family);
    // Ícones das listas de botões: o estilo vem do controle escolhido (o Nintendo depende do vid).
    let gstyle = pads::list().iter().find(|d| d.family == family).map_or_else(|| crate::ui::glyph::Style::of(family, 0), |d| crate::ui::glyph::Style::of(d.family, d.vid));
    if !chips.iter().any(|c| c.0 == ed.button) {
        ed.button = chips[4].0;
    }
    let keys_mode = ed.src == 1;
    let kvk = if ed.ksrc == 0 { ed.khk.vk as pads::Buttons } else { KEY_SOURCES[ed.ksrc.min(KEY_SOURCES.len() - 1)].0 as pads::Buttons };
    let kwvk = match ed.kwith {
        0 => 0,
        1 => ed.kwhk.vk as pads::Buttons,
        n => KEY_SOURCES.get(n - 1).map_or(0, |s| s.0 as pads::Buttons),
    };
    let target = if keys_mode { kvk } else { ed.button };
    let list_of = |l: &Layer| if keys_mode { l.keys.clone() } else { l.binds.clone() };
    let own: Vec<Bind> = list_of(&p.layers[ed.layer]).into_iter().filter(|b| b.button == target && target != 0).collect();
    let body = rect(r.x, y, r.w - 8.0, r.bottom() - y - 8.0);
    // Partes iguais em três: camada, assistente e o formulário de ligação.
    let third = ((w - 16.0) / 3.0).floor();
    let wizard = ed.tpl == acc_i;
    ed.mac = ed.mac.min(p.macros.len().saturating_sub(1));
    ui.begin_scroll("pp-body", body, ed.body_h);
    // A luz e os gatilhos deste perfil valem no controle enquanto o editor está aberto: é o teste.
    remap::preview_light(p.light.map(|l| (l, p.light_bt)));
    remap::preview_trig(Some(p.trig));
    let mut changed = false;
    let mut pick = None;
    // Gravar ou Parar no editor (com a macro de destino) e o atalho de gravar mudado.
    let mut rec_note: Option<Option<(String, usize)>> = None;
    let mut hk_macro = false;
    if tab == 0 {

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
            ask_template(ui, &tr("Acessível"), &p.name);
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
        p.layers.push(Layer { name: format!("{} {n}", tr("Camada")), parent: Some(0), ls: None, rs: None, binds: vec![], keys: vec![], gyro: None, touch: None });
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

    // Stick ou gatilho escolhido (no desenho ou na lista): o modo e a zona morta dele vêm primeiro.
    let focus = if keys_mode {
        None
    } else if ed.button & (pads::LS | remap::LSU | remap::LSD | remap::LSL | remap::LSR) != 0 {
        Some(0)
    } else if ed.button & (pads::RS | remap::RSU | remap::RSD | remap::RSL | remap::RSR) != 0 {
        Some(1)
    } else if ed.button & (remap::LT | remap::LTF) != 0 {
        Some(2)
    } else if ed.button & (remap::RT | remap::RTF) != 0 {
        Some(3)
    } else {
        None
    };
    if let Some(i) = focus {
        ui.title(rect(x, y + 4.0, w, 22.0), ZONES[i]);
        y += 32.0;
        if i < 2 {
            changed |= stick_row(ui, &mut p.layers[ed.layer], ed.layer > 0, i == 1, x, y, w);
            y += 44.0;
        }
        changed |= zone_row(ui, &mut p.zones, i, "Zona morta", x, y);
        y += 52.0;
        if i < 2 {
            // Curva do stick como mouse: vale para o perfil todo.
            let opts = vec![tr("Reta"), tr("Suave"), tr("Forte")];
            let mut sel = (p.curve[i].round() as usize).clamp(1, 3) - 1;
            ui.label(rect(x, y, 140.0, 34.0), "Curva do mouse", TEXT2);
            if ui.select(&format!("pp-curve{i}"), rect(x + 150.0, y, ((w - 150.0) * 0.56).min(220.0).floor(), 34.0), &opts, &mut sel) {
                p.curve[i] = sel as f32 + 1.0;
                changed = true;
            }
            y += 44.0;
        } else {
            changed |= ui.check(&format!("pp-hair{i}"), rect(x, y, w.min(320.0), 34.0), "Hair trigger", &mut p.hair[i - 2]);
            y += 40.0;
            // Gatilho adaptativo: só o DualSense tem os motores do gatilho.
            if family == Family::DualSense {
                let t = i - 2;
                let opts = vec![tr("Passa ao jogo"), tr("Solto"), tr("Resistência"), tr("Arma"), tr("Vibração")];
                let mut sel = match p.trig[t] {
                    Trig::Pass => 0,
                    Trig::Off => 1,
                    Trig::Rigid { .. } => 2,
                    Trig::Click { .. } => 3,
                    Trig::Buzz { .. } => 4,
                };
                ui.label(rect(x, y, 140.0, 34.0), "Gatilho adaptativo", TEXT2);
                if ui.select(&format!("pp-trig{t}"), rect(x + 150.0, y, ((w - 150.0) * 0.56).min(220.0).floor(), 34.0), &opts, &mut sel) {
                    let (start, power) = match p.trig[t] {
                        Trig::Rigid { start, power } | Trig::Click { start, power } | Trig::Buzz { start, power, .. } => (start, power),
                        Trig::Pass | Trig::Off => (3, 5),
                    };
                    p.trig[t] = match sel {
                        1 => Trig::Off,
                        2 => Trig::Rigid { start, power },
                        3 => Trig::Click { start, power },
                        4 => Trig::Buzz { start, power, hz: 10 },
                        _ => Trig::Pass,
                    };
                    changed = true;
                }
                y += 44.0;
                if !matches!(p.trig[t], Trig::Pass | Trig::Off) {
                    let (mut s0, mut pw, mut hz) = match p.trig[t] {
                        Trig::Rigid { start, power } | Trig::Click { start, power } => (start as u32 * 10, power as u32, 10),
                        Trig::Buzz { start, power, hz } => (start as u32 * 10, power as u32, hz as u32),
                        Trig::Pass | Trig::Off => (0, 0, 0),
                    };
                    ui.label(rect(x, y, 140.0, 34.0), "Começa em", TEXT2);
                    let mut c = ui.stepper(&format!("pp-trs{t}"), rect(x + 150.0, y, 130.0, 34.0), &mut s0, (0, 90), 10, "%");
                    ui.label(rect(x + 292.0, y, 60.0, 34.0), "Força", TEXT2);
                    c |= ui.stepper(&format!("pp-trp{t}"), rect(x + 352.0, y, 120.0, 34.0), &mut pw, (1, 8), 1, "");
                    y += 44.0;
                    if matches!(p.trig[t], Trig::Buzz { .. }) {
                        ui.label(rect(x, y, 140.0, 34.0), "Vibra", TEXT2);
                        c |= ui.stepper(&format!("pp-trh{t}"), rect(x + 150.0, y, 130.0, 34.0), &mut hz, (1, 40), 1, " Hz");
                        y += 44.0;
                    }
                    if c {
                        let (start, power) = ((s0 / 10) as u8, pw as u8);
                        p.trig[t] = match p.trig[t] {
                            Trig::Rigid { .. } => Trig::Rigid { start, power },
                            Trig::Click { .. } => Trig::Click { start, power },
                            _ => Trig::Buzz { start, power, hz: hz as u8 },
                        };
                        changed = true;
                    }
                }
            }
            let note = tr("Hair trigger: um movimento curto aperta e solta, em qualquer ponto do curso. Até o fim é outro botão, para o segundo estágio do gatilho.");
            let nh = crate::ui::gfx::measure(&note, 12.0, Weight::Regular, Some(w)).1.ceil();
            ui.c.text_wrap(rect(x, y, w, nh), &note, 12.0, MUTED, Weight::Regular);
            y += nh + 16.0;
        }
    }

    // Botões: clique em um para ver e mudar o que ele faz nesta camada.
    // Com desenho próprio, cada botão é o ícone dele; no genérico, o número.
    let glyphs = family != Family::Generic;
    let style = crate::ui::glyph::Style::of(family, 0);
    // Origem: botões do controle, ou teclas e botões do mouse.
    ui.title(rect(x, y + 4.0, w, 22.0), "Ligações");
    y += 32.0;
    let srcs = [tr("Botões do controle"), tr("Teclado e mouse")];
    ui.select("pp-src", rect(x, y, third, 34.0), &srcs, &mut ed.src);
    y += 44.0;
    if keys_mode {
        let kinds: Vec<String> = KEY_SOURCES.iter().map(|s| tr(s.1)).collect();
        ui.select("pp-ksrc", rect(x, y, third, 34.0), &kinds, &mut ed.ksrc);
        if ed.ksrc == 0 {
            ui.hotkey("pp-khk", rect(x + third + 8.0, y, third, 34.0), &mut ed.khk);
        }
        if ed.kwith == 1 {
            ui.hotkey("pp-kwhk", rect(x + 2.0 * (third + 8.0), y, third, 34.0), &mut ed.kwhk);
        }
        y += 40.0;
        // Quebra em vez de cortar: em inglês e espanhol a frase passa de uma linha.
        let note = "Vale fora do Prisma e fora de programas abertos como administrador. Os cliques esquerdo e direito não mudam.";
        let nh = crate::ui::gfx::measure(note, 12.0, Weight::Regular, Some(w)).1.ceil();
        ui.c.text_wrap(rect(x, y, w, nh), note, 12.0, MUTED, Weight::Regular);
        y += nh + 16.0;
    } else {
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
            ed.set_button(*bit);
            ui.s.needs_redraw = true;
        }
        bx += cw + 6.0;
    }
    y = by + 40.0;
    }

    // O que o botão escolhido faz.
    let name = if keys_mode { src_name(kvk) } else { chips.iter().find(|c| c.0 == ed.button).map_or(String::new(), |c| c.1.clone()) };
    ui.c.text_fit(rect(x, y, w, 22.0), &tr("{}: o que faz nesta camada").replace("{}", &name), 14.0, TEXT, Align::Left, Weight::Semibold);
    y += 30.0;
    if own.is_empty() {
        // Sem ligação aqui: mostra o que vem da camada mãe.
        let mut from = p.layers[ed.layer].parent;
        let mut text = tr("Nada.");
        while let Some(l) = from {
            let all = list_of(&p.layers[l as usize]);
            let got: Vec<&Bind> = all.iter().filter(|b| b.button == target && target != 0 && b.with == 0).collect();
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
            let with: Vec<String> = if keys_mode { vec![src_name(b.with)] } else { chips.iter().filter(|c| b.with & c.0 != 0).map(|c| c.1.clone()).collect() };
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
        let v = if keys_mode { &mut l.keys } else { &mut l.binds };
        if let Some(i) = v.iter().position(|b| *b == d) {
            v.remove(i);
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
    let others: Vec<&(pads::Buttons, String)> = chips.iter().filter(|c| c.0 != ed.button && c.0 & pads::PHYSICAL != 0).collect();
    let mut withs = vec![tr("Sozinho")];
    withs.extend(others.iter().map(|c| tr("Junto com {}").replace("{}", &c.1)));
    ed.with = ed.with.min(withs.len() - 1);
    if keys_mode {
        let mut kw = vec![tr("Sozinho"), tr("Junto com uma tecla")];
        kw.extend(KEY_SOURCES.iter().skip(1).map(|s| tr("Junto com {}").replace("{}", &tr(s.1))));
        ed.kwith = ed.kwith.min(kw.len() - 1);
        ui.select("pp-kwith", rect(x + third + 8.0, y, third, 34.0), &kw, &mut ed.kwith);
    } else {
        let wg: Vec<pads::Buttons> = std::iter::once(0).chain(others.iter().map(|c| c.0)).collect();
        ui.select_pad("pp-with", rect(x + third + 8.0, y, third, 34.0), &withs, &wg, gstyle, &mut ed.with);
    }
    let kinds: Vec<String> = KINDS.iter().map(|k| tr(k)).collect();
    if ui.select("pp-kind", rect(x + 2.0 * (third + 8.0), y, third, 34.0), &kinds, &mut ed.kind) {
        ed.val = 0;
    }
    y += 44.0;
    let vr = rect(x, y, 2.0 * third + 8.0, 34.0);
    let sounds = app.cfg.snd_keys.len().min(crate::app::SND_KEYS_MAX);
    let others_p: Vec<String> = list.iter().map(|o| o.name.clone()).filter(|n| *n != p.name).collect();
    // Ações do Prisma fora da seção Controles, mais as combinações de som.
    let prisma_ids: Vec<u8> = (0..remap::ACTIONS.len() as u8).filter(|a| !remap::PAD_ACTIONS.contains(a)).chain([remap::MACRO_REC]).chain((0..sounds as u8).map(|n| remap::SOUND + n)).collect();
    let values: Vec<String> = match ed.kind {
        1 => SPECIAL.iter().map(|s| tr(s.1)).collect(),
        2 => MOUSE.iter().map(|s| tr(s)).collect(),
        3 => WHEEL.iter().map(|s| tr(s)).collect(),
        4 | 5 => p.layers.iter().map(|l| l.name.clone()).collect(),
        6 => prisma_ids.iter().map(|&a| action_choice(a)).collect(),
        7 => others_p.clone(),
        10 => p.macros.iter().map(|m| m.name.clone()).collect(),
        11 => p.texts.iter().map(short).collect(),
        _ => vec![],
    };
    if ed.kind == 0 {
        ui.hotkey("pp-key", vr, &mut ed.hk);
    } else if ed.kind == 8 {
        let half_v = ((vr.w - 8.0) / 2.0).floor();
        let shown = if ed.run.is_empty() { tr("Nenhum programa escolhido") } else { crate::sys::procs::exe_name(&ed.run) };
        ui.c.text_fit(rect(vr.x, vr.y, half_v, vr.h), &shown, 13.0, if ed.run.is_empty() { MUTED } else { TEXT }, Align::Left, Weight::Regular);
        pick = Some(rect(vr.x + half_v + 8.0, vr.y, half_v, vr.h));
    } else if ed.kind == 12 {
        ui.search("pp-url", vr, &mut ed.url, "https://");
        ed.url = ed.url.trim().chars().take(500).collect();
        if !ed.url.is_empty() && !remap::link_ok(&ed.url) {
            ui.c.text_fit(rect(x, y + 40.0, w, 20.0), &tr("O link começa com https:// ou http://."), 12.0, WARN, Align::Left, Weight::Regular);
        }
    } else if let Some(k) = PAD_KINDS.iter().find(|k| k.0 == ed.kind) {
        ui.c.text_fit(vr, &tr(k.2), 13.0, MUTED, Align::Left, Weight::Regular);
    } else if values.is_empty() {
        let none = if ed.kind == 11 { "Nenhum texto: crie um em Texto rápido, mais abaixo" } else { "Nenhuma macro: crie uma em Macros, mais abaixo" };
        ui.c.text_fit(vr, &tr(none), 13.0, MUTED, Align::Left, Weight::Regular);
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
        6 => prisma_ids.get(ed.val).map(|&a| Out::Action(a)),
        // Perfil e programa só entram na lista de textos do perfil ao adicionar.
        7 => others_p.get(ed.val).map(|_| Out::Profile(0)),
        9 | 13..=15 => PAD_KINDS.iter().find(|k| k.0 == ed.kind).map(|k| Out::Action(k.1)),
        10 => p.macros.get(ed.val).map(|_| Out::Macro(ed.val as u8)),
        11 => p.texts.get(ed.val).map(|_| Out::Text(ed.val as u8)),
        12 => remap::link_ok(&ed.url).then_some(Out::Run(0)),
        _ => (!ed.run.is_empty()).then_some(Out::Run(0)),
    };
    // Teclado: cada tecla diferente do perfil ocupa um dos `KEY_SLOTS` lugares do motor.
    let mut used: Vec<pads::Buttons> = p.layers.iter().flat_map(|l| &l.keys).flat_map(|b| [b.button, b.with]).filter(|&v| v != 0).collect();
    used.extend([kvk, kwvk].into_iter().filter(|&v| v != 0));
    used.sort_unstable();
    used.dedup();
    let key_ok = kvk != 0 && kwvk != kvk && (ed.kwith != 1 || kwvk != 0) && used.len() <= remap::KEY_SLOTS;
    if keys_mode && !key_ok && used.len() > remap::KEY_SLOTS {
        ui.c.text_fit(rect(x, y + 40.0, w, 20.0), &tr("Um perfil usa até 17 teclas diferentes."), 12.0, WARN, Align::Left, Weight::Regular);
    }
    if ui.button_ex("pp-add", rect(x + 2.0 * (third + 8.0), y, third, 34.0), "Adicionar", true, out.is_some() && (!keys_mode || key_ok)) {
        let out = match out {
            Some(Out::Profile(_)) => remap::intern(&mut p.strings, &others_p[ed.val]).map(Out::Profile),
            Some(Out::Run(_)) => remap::intern(&mut p.strings, if ed.kind == 12 { &ed.url } else { &ed.run }).map(Out::Run),
            o => o,
        };
        if let Some(out) = out {
            if keys_mode {
                p.layers[ed.layer].keys.push(Bind { button: kvk, with: kwvk, when: WHENS[ed.when].0, out });
            } else {
                let with = if ed.with == 0 { 0 } else { others[ed.with - 1].0 };
                p.layers[ed.layer].binds.push(Bind { button: ed.button, with, when: WHENS[ed.when].0, out });
            }
            changed = true;
        }
    }
    // Combinação que já faz outra coisa: avisa, sem impedir.
    if !keys_mode && ed.with != 0 {
        let combo = ed.button | others[ed.with - 1].0;
        if let Some(by) = remap::combo_conflict(&p, combo, false) {
            ui.c.text_fit(rect(x, y + 40.0, w, 20.0), &tr("Essa combinação já é usada por {}.").replace("{}", &by), 12.0, WARN, Align::Left, Weight::Regular);
        }
    }
    y += 70.0;

    // Turbo do botão escolhido: à parte das ligações, vale em todas as camadas do perfil.
    if !keys_mode {
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
    let keyable: Vec<&(pads::Buttons, String)> = chips.iter().filter(|c| c.0 & dirs == 0).collect();
    let mut firsts = vec![tr("Sem atalho")];
    firsts.extend(keyable.iter().map(|c| c.1.clone()));
    let mut seconds = vec![tr("Nenhum")];
    seconds.extend(keyable.iter().map(|c| c.1.clone()));
    let bits: Vec<usize> = keyable.iter().enumerate().filter(|(_, c)| p.turbo_key & c.0 != 0).map(|(i, _)| i + 1).collect();
    let (mut k1, mut k2) = (bits.first().copied().unwrap_or(0), bits.get(1).copied().unwrap_or(0));
    let kg: Vec<pads::Buttons> = std::iter::once(0).chain(keyable.iter().map(|c| c.0)).collect();
    let c1 = ui.select_pad("pp-tk1", rect(x + third + 8.0, y, third, 34.0), &firsts, &kg, gstyle, &mut k1);
    let c2 = ui.select_pad("pp-tk2", rect(x + 2.0 * (third + 8.0), y, third, 34.0), &seconds, &kg, gstyle, &mut k2);
    if c1 || c2 {
        let bit = |k: usize| if k == 0 { 0 } else { keyable[k - 1].0 };
        p.turbo_key = if k1 == 0 { 0 } else { bit(k1) | bit(k2) };
        changed = true;
    }
    y += 40.0;
    let warn = if p.turbo_key == 0 { None } else { remap::combo_conflict(&p, p.turbo_key, true).map(|by| tr("Esse atalho já é usado por {}.").replace("{}", &by)) };
    let note = warn.clone().unwrap_or_else(|| if p.turbo_key == 0 { tr("Sem atalho: o turbo só muda aqui.") } else { tr("Segure o atalho e aperte o botão para ligar ou desligar o turbo dele.") });
    ui.c.text_fit(rect(x, y, w, 20.0), &note, 12.0, if warn.is_some() { WARN } else { MUTED }, Align::Left, Weight::Regular);
    }
    y += 40.0;

    // Sticks da camada e zonas mortas; o que já está no painel de cima não se repete.
    for right in [false, true] {
        if focus == Some(right as usize) {
            continue;
        }
        changed |= stick_row(ui, &mut p.layers[ed.layer], ed.layer > 0, right, x, y, w);
        y += 44.0;
    }
    // Flick stick nesta camada: o teste da volta de 360° com o valor dela.
    if let Some(px) = [p.layers[ed.layer].ls, p.layers[ed.layer].rs].iter().find_map(|s| if let Some(Stick::Flick(v)) = s { Some(*v) } else { None }) {
        if ui.button("pp-flick-test", rect(x, y, 200.0_f32.min(w), 34.0), "Testar volta", false) {
            remap::test_turn(px);
        }
        y += 42.0;
        let note = tr("Em 3 s, o Prisma move o mouse o valor do flick stick para a direita. Volte ao jogo: a câmera deve dar uma volta inteira e parar onde estava. Se passar, diminua o valor; se faltar, aumente.");
        let nh = crate::ui::gfx::measure(&note, 12.0, Weight::Regular, Some(w)).1.ceil();
        ui.c.text_wrap(rect(x, y, w, nh), &note, 12.0, MUTED, Weight::Regular);
        y += nh + 16.0;
    }
    // Gyro desta camada e, se alguma camada usa o gyro, as opções do perfil.
    changed |= motion_row(ui, &mut p.layers[ed.layer], false, ed.layer > 0, x, y, w);
    y += 44.0;
    if p.layers.iter().any(|l| matches!(l.gyro, Some(Motion::Mouse(_)))) {
        let o = &mut p.gyro_opt;
        let spaces = vec![tr("Espaço local"), tr("Espaço do jogador")];
        let mut sp = usize::from(o.player);
        ui.label(rect(x, y, 120.0, 34.0), "Virar", TEXT2);
        if ui.select("pp-gyro-sp", rect(x + 130.0, y, ((w - 138.0) * 0.56).min(220.0).floor(), 34.0), &spaces, &mut sp) {
            o.player = sp == 1;
            changed = true;
        }
        y += 44.0;
        let cw = ((w - 16.0) / 3.0).floor();
        changed |= ui.check("pp-gyro-ix", rect(x, y, cw, 34.0), "Inverter horizontal", &mut o.inv_x);
        changed |= ui.check("pp-gyro-iy", rect(x + cw + 8.0, y, cw, 34.0), "Inverter vertical", &mut o.inv_y);
        changed |= ui.check("pp-gyro-sm", rect(x + 2.0 * (cw + 8.0), y, cw, 34.0), "Suavizar tremor", &mut o.smooth);
        y += 44.0;
    }
    // Touchpad desta camada: um dedo move o mouse, dois rolam.
    changed |= motion_row(ui, &mut p.layers[ed.layer], true, ed.layer > 0, x, y, w);
    y += 44.0;
    // Barra de luz do perfil: só DualSense e DualShock 4.
    if matches!(family, Family::DualSense | Family::Ds4) {
        let opts = vec![tr("Não mexer"), tr("Desligada"), tr("Pela bateria"), tr("Cor"), tr("Piscar"), tr("Arco-íris")];
        let mut sel = match p.light {
            None => 0,
            Some(Light::Off) => 1,
            Some(Light::Battery(_)) => 2,
            Some(Light::Color(_)) => 3,
            Some(Light::Blink(_)) => 4,
            Some(Light::Rainbow) => 5,
        };
        ui.label(rect(x, y, 120.0, 34.0), "Barra de luz", TEXT2);
        let sw = ((w - 138.0) * 0.56).min(220.0).floor();
        if ui.select("pp-light", rect(x + 130.0, y, sw, 34.0), &opts, &mut sel) {
            p.light = match sel {
                0 => None,
                1 => Some(Light::Off),
                2 => Some(Light::Battery(Batt::default())),
                3 => Some(Light::Color(remap::FIRST_COLOR)),
                4 => Some(Light::Blink(remap::FIRST_COLOR)),
                _ => Some(Light::Rainbow),
            };
            changed = true;
        }
        // Amostra da cor e o botão que abre a janela de escolher; o botão não passa da borda da coluna.
        let bx = x + 130.0 + sw + 12.0;
        match p.light {
            Some(Light::Color(c) | Light::Blink(c)) => {
                ui.c.round(rect(bx, y, 34.0, 34.0), 8.0, swatch(c));
                ui.c.round_line(rect(bx, y, 34.0, 34.0), 8.0, BORDER2, 1.0);
                if ui.button("pp-light-pick", rect(bx + 42.0, y, (x + w - bx - 42.0).min(150.0), 34.0), "Escolher cor…", false) {
                    ed.pick = Some((0, crate::ui::gfx::rgb_hsv(c)));
                }
            }
            Some(Light::Battery(b)) => {
                // Faixa com o degradê inteiro, de vazia a cheia.
                for i in 0..34 {
                    ui.c.fill(rect(bx + i as f32 * 3.0, y + 6.0, 3.0, 22.0), swatch(remap::battery_color(b, (i * 3) as u8)));
                }
                if ui.button("pp-light-batt", rect(bx + 114.0, y, (x + w - bx - 114.0).min(150.0), 34.0), "Ajustar cores…", false) {
                    ed.batt = true;
                }
            }
            _ => {}
        }
        y += 44.0;
        if p.light.is_some() {
            changed |= ui.check("pp-light-bt", rect(x, y, w.min(360.0), 34.0), "Também pelo Bluetooth", &mut p.light_bt);
            y += 44.0;
        }
        // Vibração de confirmação: ao passar para este perfil e na parada de emergência.
        let opts = vec![tr("Não"), tr("Leve"), tr("Forte")];
        let mut sel = p.buzz.min(2) as usize;
        // Rótulo mais largo que o da luz: "Vibrar ao confirmar" não cabe em 120 px.
        ui.label(rect(x, y, 170.0, 34.0), "Vibrar ao confirmar", TEXT2);
        let sw = ((w - 188.0) * 0.56).min(220.0).floor();
        if ui.select("pp-buzz", rect(x + 180.0, y, sw, 34.0), &opts, &mut sel) {
            p.buzz = sel as u8;
            changed = true;
        }
        if p.buzz != 0 && ui.button("pp-buzz-test", rect(x + 180.0 + sw + 12.0, y, 110.0, 34.0), "Testar", false) {
            pads::buzz(p.buzz);
        }
        y += 44.0;
        let note = tr("O controle vibra ao passar para este perfil e ao ligar ou desligar a parada de emergência.");
        let nh = crate::ui::gfx::measure(&note, 12.0, Weight::Regular, Some(w)).1.ceil();
        ui.c.text_wrap(rect(x, y, w, nh), &note, 12.0, MUTED, Weight::Regular);
        y += nh + 16.0;
        // Háptico: o motor fraco responde ao que se faz no controle, com a força de cada fonte.
        let h = &mut p.haptic;
        for (i, (label, v)) in [("Vibrar com os botões", &mut h.buttons), ("Vibrar com os gatilhos", &mut h.triggers), ("Vibrar com os sticks", &mut h.sticks), ("Vibrar com o som do PC", &mut h.audio)].into_iter().enumerate() {
            let mut n = *v as u32;
            ui.label(rect(x, y, 170.0, 34.0), label, TEXT2);
            if ui.stepper(&format!("pp-hap{i}"), rect(x + 180.0, y, 130.0, 34.0), &mut n, (0, 8), 1, "") {
                *v = n as u8;
                changed = true;
            }
            y += 44.0;
        }
        let note = tr("0 desliga. Os botões dão um toque curto a cada aperto; os gatilhos e os sticks vibram mais quanto mais longe vão. O som do PC vibra com o que o Windows toca: no DualSense, nos atuadores; no DualShock 4, no motor.");
        let nh = crate::ui::gfx::measure(&note, 12.0, Weight::Regular, Some(w)).1.ceil();
        ui.c.text_wrap(rect(x, y, w, nh), &note, 12.0, MUTED, Weight::Regular);
        y += nh + 16.0;
    }

    // Zonas mortas: valem para o perfil todo.
    ui.title(rect(x, y + 4.0, w, 22.0), "Zona morta");
    y += 32.0;
    for i in 0..4 {
        if focus == Some(i) {
            continue;
        }
        changed |= zone_row(ui, &mut p.zones, i, ZONES[i], x, y);
        y += 40.0;
    }
    ui.c.text_fit(rect(x, y, w, 20.0), &tr("Abaixo do primeiro valor conta como parado; a partir do segundo, como fim do curso."), 12.0, MUTED, Align::Left, Weight::Regular);
    y += 36.0;

    }

    if tab == 1 {
    // Macros do perfil: escolher, criar, apagar, o modo, gravar e os passos. A gravação pode ter
    // começado fora daqui (atalho, radial, combinação): os passos aparecem ao vivo do mesmo jeito.
    let live = macros::live();
    ed.rec = live.is_some();
    ui.title(rect(x, y + 4.0, w, 22.0), "Macros");
    y += 32.0;
    let names: Vec<String> = p.macros.iter().map(|m| m.name.clone()).collect();
    if let Some(l) = &live {
        let target = match &l.into {
            Some((pn, i)) if *pn == p.name => names.get(*i).cloned().unwrap_or_default(),
            _ => tr("Macro nova"),
        };
        ui.c.text_fit(rect(x, y, third, 34.0), &target, 13.0, TEXT, Align::Left, Weight::Regular);
    } else if names.is_empty() {
        ui.c.text_fit(rect(x, y, third, 34.0), &tr("Nenhuma macro."), 13.0, MUTED, Align::Left, Weight::Regular);
    } else {
        ui.select("pp-mac", rect(x, y, third, 34.0), &names, &mut ed.mac);
    }
    if ui.button_ex("pp-mac-add", rect(x + third + 8.0, y, third, 34.0), "Nova macro", false, !ed.rec && p.macros.len() < remap::MAX_MACROS) {
        p.macros.push(Macro { name: macros::next_name(&p, &tr("Macro")), mode: Mode::Once, steps: vec![], pad: None });
        ed.mac = p.macros.len() - 1;
        changed = true;
    }
    if ui.button_ex("pp-mac-del", rect(x + 2.0 * (third + 8.0), y, third, 34.0), "Apagar macro", false, !ed.rec && !p.macros.is_empty()) {
        remove_macro(&mut p, ed.mac);
        ed.mac = 0;
        changed = true;
    }
    y += 44.0;
    let has = ed.mac < p.macros.len();
    if let Some(l) = &live {
        let t = format!("{:02}:{:02}", l.secs / 60, l.secs % 60);
        ui.c.text_fit(rect(x, y, 2.0 * third + 8.0, 34.0), &t, 13.0, WARN, Align::Left, Weight::Semibold);
    } else if has {
        let modes: Vec<String> = MODE_NAMES.iter().map(|s| tr(s)).collect();
        let mut mi = remap::MODES.iter().position(|x| x.0 == p.macros[ed.mac].mode).unwrap_or(0);
        if ui.select("pp-mac-mode", rect(x, y, 2.0 * third + 8.0, 34.0), &modes, &mut mi) {
            p.macros[ed.mac].mode = remap::MODES[mi].0;
            changed = true;
        }
    } else {
        ui.c.text_fit(rect(x, y, 2.0 * third + 8.0, 34.0), &tr("Gravar cria uma macro nova."), 12.0, MUTED, Align::Left, Weight::Regular);
    }
    // Sem macro escolhida, grava numa nova; Parar guarda onde a gravação começou.
    if ui.button_ex("pp-mac-rec", rect(x + 2.0 * (third + 8.0), y, third, 34.0), if ed.rec { "Parar" } else { "Gravar" }, ed.rec, true) {
        rec_note = Some(if has && !ed.rec { Some((p.name.clone(), ed.mac)) } else { None });
    }
    y += 44.0;
    // Atalho de teclado que começa e para (o mesmo que a ação no radial e nos botões).
    ui.c.text_wrap(rect(x, y, 2.0 * third + 8.0, 40.0), &tr("Atalho de teclado para gravar e parar, em qualquer programa:"), 12.0, MUTED, Weight::Regular);
    hk_macro = ui.hotkey("hk-macro", rect(x + 2.0 * (third + 8.0), y, third, 40.0), &mut app.cfg.hk_macro);
    y += 48.0;
    let (steps, pad) = match &live {
        Some(l) => (l.steps.clone(), l.pad),
        None if has => (p.macros[ed.mac].steps.clone(), p.macros[ed.mac].pad),
        None => (vec![], None),
    };
    let n = steps.len();
    if has || ed.rec {
        let status = if ed.rec { tr("Gravando: aperte os botões do controle (aqui mesmo ou em outro programa) ou use o teclado e o mouse em outro programa; clique em Parar no fim. Sticks, gyro e o movimento do mouse não entram.") } else { tr("{} passos. Gravar troca os passos pelos novos.").replace("{}", &n.to_string()) };
        ui.c.text_fit(rect(x, y, w, 20.0), &status, 12.0, if ed.rec { WARN } else { MUTED }, Align::Left, Weight::Regular);
        y += 28.0;
    }
    // Botões no desenho do controle em que a macro foi gravada.
    let pstyle = pad.map_or(gstyle, |(f, vid)| crate::ui::glyph::Style::of(f, vid));
    let mut drop = None;
    for (i, &s) in steps.iter().enumerate() {
        let mut tx = x;
        if let Step::Down(Out::Pad(b)) | Step::Up(Out::Pad(b)) = s {
            tx += crate::ui::glyph::draw(ui.c, x, y + 17.0, 18.0, pstyle, 1 << b, TEXT2) + 8.0;
        }
        ui.c.text_fit(rect(tx, y, 2.0 * third + 8.0 - 150.0 - (tx - x), 34.0), &step_text(s, &p, pad), 13.0, TEXT, Align::Left, Weight::Regular);
        if ed.rec {
            y += 40.0;
            continue;
        }
        if let Step::Wait(ms) = s {
            let mut v = ms;
            if ui.stepper(&format!("pp-ms{i}"), rect(x + 2.0 * third + 8.0 - 140.0, y, 140.0, 34.0), &mut v, (10, remap::MAX_WAIT), 10, "ms") {
                p.macros[ed.mac].steps[i] = Step::Wait(v);
                changed = true;
            }
        }
        if ui.button_ex(&format!("pp-mrm{i}"), rect(x + 2.0 * (third + 8.0), y, third, 32.0), "Remover", false, true) {
            drop = Some(i);
        }
        y += 40.0;
    }
    if let Some(i) = drop {
        p.macros[ed.mac].steps.remove(i);
        changed = true;
    }
    if has && !ed.rec {
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
        if ui.button_ex("pp-sadd", rect(x + 2.0 * (third + 8.0), y, third, 34.0), "Adicionar passo", true, room && add.is_some()) {
            p.macros[ed.mac].steps.extend(add.unwrap_or_default());
            changed = true;
        }
        y += 44.0;
    }

    // Texto rápido: sai por uma ligação (destino Texto rápido) ou pela abreviação digitada.
    y += 12.0;
    ui.title(rect(x, y + 4.0, w, 22.0), "Texto rápido");
    y += 32.0;
    let mut gone = None;
    for (i, t) in p.texts.iter().enumerate() {
        let a = if t.abbr.is_empty() { tr("Sem abreviação") } else { t.abbr.clone() };
        ui.c.text_fit(rect(x, y, third, 34.0), &a, 13.0, TEXT2, Align::Left, Weight::Regular);
        ui.c.text_fit(rect(x + third + 8.0, y, third, 34.0), &t.text, 13.0, TEXT, Align::Left, Weight::Regular);
        if ui.button_ex(&format!("pp-trm{i}"), rect(x + 2.0 * (third + 8.0), y, third, 32.0), "Remover", false, true) {
            gone = Some(i);
        }
        y += 40.0;
    }
    if let Some(i) = gone {
        p.texts.remove(i);
        drop_out(&mut p, Out::Text(i as u8));
        changed = true;
    }
    ui.search("pp-tabbr", rect(x, y, third, 34.0), &mut ed.tabbr, "Abreviação (opcional)");
    ed.tabbr = ed.tabbr.chars().filter(char::is_ascii_alphanumeric).take(16).collect::<String>().to_ascii_lowercase();
    ui.search("pp-ttext", rect(x + third + 8.0, y, third, 34.0), &mut ed.ttext, "Texto");
    ed.ttext = ed.ttext.chars().filter(|c| !c.is_control()).take(remap::MAX_TEXT).collect();
    let text = ed.ttext.trim().to_string();
    let dup = !ed.tabbr.is_empty() && p.texts.iter().any(|t| t.abbr == ed.tabbr);
    let ok = !text.is_empty() && remap::abbr_ok(&ed.tabbr) && !dup && p.texts.len() < remap::MAX_TEXTS;
    if ui.button_ex("pp-tadd", rect(x + 2.0 * (third + 8.0), y, third, 34.0), "Adicionar", true, ok) {
        p.texts.push(remap::Snip { abbr: std::mem::take(&mut ed.tabbr), text });
        ed.ttext.clear();
        changed = true;
    }
    y += 42.0;
    let (note, warn) = if dup {
        ("Essa abreviação já existe neste perfil.", true)
    } else if !remap::abbr_ok(&ed.tabbr) {
        ("A abreviação tem de 2 a 16 letras e números.", true)
    } else {
        ("Em qualquer programa, a abreviação seguida de espaço, Enter ou Tab vira o texto. Sem abreviação, o texto sai por um botão ou tecla (destino Texto rápido).", false)
    };
    ui.c.text_wrap(rect(x, y, w, 36.0), &tr(note), 12.0, if warn { WARN } else { MUTED }, Weight::Regular);
    y += 40.0;
    }
    let h = y + 12.0 - body.y;
    if (h - ed.body_h).abs() > 0.5 {
        ed.body_h = h;
        ui.s.needs_redraw = true;
    }

    // Depois do último uso de `ed`: o atalho precisa da App inteira.
    if hk_macro {
        app.cfg.save();
        app.register_hotkeys();
    }
    if let Some(r) = pick {
        super::picker::add_button(ui, app, r, super::picker::Target::PadProgram);
    }
    ui.end_scroll();

    changed |= light_sheets(ui, app, &mut p);
    if changed && !remap::save(&p) {
        app.toast.info("Não deu para salvar o perfil.");
    }
    // Depois de salvar o perfil: Parar guarda os passos nele, e o `p` daqui não pode apagá-los.
    if let Some(into) = rec_note {
        app.macro_toggle(into);
    }
}

/// A cor da amostra, a partir dos três valores de 0 a 255.
fn swatch(c: [u8; 3]) -> crate::ui::gfx::Color {
    crate::ui::gfx::Color { r: c[0] as f32 / 255.0, g: c[1] as f32 / 255.0, b: c[2] as f32 / 255.0, a: 1.0 }
}

/// As três cores da bateria, na ordem em que a janela mostra.
const BATT_ROWS: [&str; 3] = ["Carga vazia", "Metade da carga", "Carga cheia"];

fn batt_color(b: &mut Batt, i: usize) -> &mut [u8; 3] {
    match i {
        0 => &mut b.low,
        1 => &mut b.mid,
        _ => &mut b.full,
    }
}

/// As janelas da barra de luz: o seletor de cor e os ajustes da bateria. Devolve true quando o
/// perfil mudou. O seletor fica na frente dos ajustes, e voltar dele traz os ajustes de volta.
fn light_sheets(ui: &mut Ui, app: &mut App, p: &mut Profile) -> bool {
    let Some(ed) = app.pads.edit.as_mut() else { return false };
    let mut changed = false;
    if let Some((what, mut hsv)) = ed.pick {
        let b = ui.sheet(600.0, 392.0);
        ui.title(rect(b.x + 24.0, b.y + 20.0, b.w - 48.0, 24.0), if what == 0 { "Cor da barra de luz" } else { BATT_ROWS[what as usize - 1] });
        if ui.color_picker("pp-cp", rect(b.x + 24.0, b.y + 60.0, 276.0, 240.0), &mut hsv) {
            ed.pick = Some((what, hsv));
        }
        let c = crate::ui::gfx::hsv_rgb(hsv[0], hsv[1], hsv[2]);
        // A barra de luz do controle já acompanha a escolha, antes de confirmar.
        remap::preview_light(Some((Light::Color(c), p.light_bt)));
        let px = b.x + 336.0;
        ui.c.round(rect(px, b.y + 60.0, 120.0, 72.0), 10.0, swatch(c));
        ui.c.round_line(rect(px, b.y + 60.0, 120.0, 72.0), 10.0, BORDER2, 1.0);
        ui.c.text_raw(rect(px + 136.0, b.y + 60.0, 120.0, 72.0), &format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]), 14.0, TEXT, Align::Left, Weight::Semibold);
        for (i, n) in ["R", "G", "B"].iter().enumerate() {
            let ry = b.y + 148.0 + i as f32 * 26.0;
            ui.c.text_raw(rect(px, ry, 24.0, 22.0), n, 13.0, TEXT2, Align::Left, Weight::Regular);
            ui.c.text_raw(rect(px + 28.0, ry, 60.0, 22.0), &c[i].to_string(), 13.0, TEXT, Align::Left, Weight::Semibold);
        }
        if ui.button("pp-cp-ok", rect(b.right() - 300.0, b.bottom() - 56.0, 136.0, 38.0), "Confirmar", true) {
            match (what, p.light) {
                (0, Some(Light::Blink(_))) => p.light = Some(Light::Blink(c)),
                (0, _) => p.light = Some(Light::Color(c)),
                (k, Some(Light::Battery(mut bt))) => {
                    *batt_color(&mut bt, k as usize - 1) = c;
                    p.light = Some(Light::Battery(bt));
                }
                _ => {}
            }
            changed = true;
            ed.pick = None;
        }
        if ui.button("pp-cp-no", rect(b.right() - 156.0, b.bottom() - 56.0, 136.0, 38.0), "Cancelar", false) {
            ed.pick = None;
        }
        return changed;
    }
    if !ed.batt {
        return false;
    }
    // O botão que abre esta janela só aparece com a luz pela bateria; o padrão é só a rede de
    // segurança (e a foto de teste).
    let mut bt = match p.light {
        Some(Light::Battery(b)) => b,
        _ => Batt::default(),
    };
    let b = ui.sheet(560.0, 396.0);
    ui.title(rect(b.x + 24.0, b.y + 20.0, b.w - 48.0, 24.0), "Barra de luz pela bateria");
    ui.c.text_wrap(rect(b.x + 24.0, b.y + 50.0, b.w - 48.0, 36.0), "A cor acompanha a carga. Escolha as três cores e de quantos em quantos por cento ela muda.", 12.0, TEXT2, Weight::Regular);
    // Prévia: o degradê inteiro, de vazia a cheia.
    let bar = rect(b.x + 24.0, b.y + 94.0, b.w - 48.0, 26.0);
    for i in 0..=100 {
        ui.c.fill(rect(bar.x + bar.w * i as f32 / 101.0, bar.y, bar.w / 101.0 + 1.0, bar.h), swatch(remap::battery_color(bt, i)));
    }
    ui.c.round_line(bar, 4.0, BORDER2, 1.0);
    for (i, name) in BATT_ROWS.iter().enumerate() {
        let ry = b.y + 136.0 + i as f32 * 44.0;
        let c = *batt_color(&mut bt, i);
        ui.c.round(rect(b.x + 24.0, ry, 34.0, 34.0), 8.0, swatch(c));
        ui.c.round_line(rect(b.x + 24.0, ry, 34.0, 34.0), 8.0, BORDER2, 1.0);
        ui.label(rect(b.x + 68.0, ry, 220.0, 34.0), name, TEXT);
        if ui.button(&format!("pp-batt-{i}"), rect(b.right() - 160.0, ry, 136.0, 34.0), "Mudar…", false) {
            ed.pick = Some((i as u8 + 1, crate::ui::gfx::rgb_hsv(c)));
        }
    }
    ui.label(rect(b.x + 24.0, b.y + 274.0, 240.0, 36.0), "Passo da cor", TEXT2);
    let mut step = bt.step.max(1) as u32;
    if ui.stepper("pp-batt-step", rect(b.x + 280.0, b.y + 274.0, 140.0, 36.0), &mut step, (1, 25), 1, "%") {
        bt.step = step as u8;
        p.light = Some(Light::Battery(bt));
        changed = true;
    }
    if ui.button("pp-batt-ok", rect(b.right() - 160.0, b.bottom() - 56.0, 136.0, 38.0), "Fechar", true) {
        ed.batt = false;
    }
    changed
}
