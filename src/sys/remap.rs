//! Motor de mapeamento (Fase 2): controle para teclado e mouse, com ativadores, camadas e zonas
//! mortas. A parte de cima (perfil e motor) não toca no Windows e tem testes; a de baixo recebe os
//! relatórios de `pads`, troca o perfil pelo programa em foco e manda as teclas com `SendInput`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::pads::{self, Buttons, Input};

// Botões virtuais, depois dos físicos de `pads`: gatilhos e direções dos sticks viram botões.
pub const LT: Buttons = 1 << 17;
pub const RT: Buttons = 1 << 18;
pub const LSU: Buttons = 1 << 19;
pub const LSD: Buttons = 1 << 20;
pub const LSL: Buttons = 1 << 21;
pub const LSR: Buttons = 1 << 22;
pub const RSU: Buttons = 1 << 23;
pub const RSD: Buttons = 1 << 24;
pub const RSL: Buttons = 1 << 25;
pub const RSR: Buttons = 1 << 26;
/// Gestos do touchpad: deslizar um dedo nas 4 direções, tocar sem apertar, e o clique em cada metade.
pub const TPU: Buttons = 1 << 27;
pub const TPD: Buttons = 1 << 28;
pub const TPL: Buttons = 1 << 29;
pub const TPR: Buttons = 1 << 30;
pub const TPTAP: Buttons = 1 << 31;
pub const TPCL: Buttons = 1 << 32;
pub const TPCR: Buttons = 1 << 33;
/// Regiões do touchpad: um dedo parado em cada quarto segura o botão.
pub const TPUL: Buttons = 1 << 34;
pub const TPUR: Buttons = 1 << 35;
pub const TPDL: Buttons = 1 << 36;
pub const TPDR: Buttons = 1 << 37;
/// Gatilho apertado até o fim: o segundo estágio do gatilho.
pub const LTF: Buttons = 1 << 38;
pub const RTF: Buttons = 1 << 39;
const COUNT: usize = 40;
const ALL: Buttons = (1 << COUNT) - 1;

/// Nome de cada botão no arquivo do perfil.
pub const NAMES: [(&str, Buttons); COUNT] = [
    ("A", pads::A), ("B", pads::B), ("X", pads::X), ("Y", pads::Y), ("LB", pads::LB), ("RB", pads::RB),
    ("BACK", pads::BACK), ("START", pads::START), ("LS", pads::LS), ("RS", pads::RS), ("UP", pads::UP),
    ("DOWN", pads::DOWN), ("LEFT", pads::LEFT), ("RIGHT", pads::RIGHT), ("GUIDE", pads::GUIDE), ("PAD", pads::PAD),
    ("MUTE", pads::MUTE), ("LT", LT), ("RT", RT), ("LSU", LSU), ("LSD", LSD), ("LSL", LSL), ("LSR", LSR),
    ("RSU", RSU), ("RSD", RSD), ("RSL", RSL), ("RSR", RSR), ("TPU", TPU), ("TPD", TPD), ("TPL", TPL), ("TPR", TPR),
    ("TPTAP", TPTAP), ("TPCL", TPCL), ("TPCR", TPCR), ("TPUL", TPUL), ("TPUR", TPUR), ("TPDL", TPDL), ("TPDR", TPDR),
    ("LTF", LTF), ("RTF", RTF),
];

/// Segurar a partir de 400 ms; tocar é soltar antes disso; duplo é apertar de novo em até 300 ms.
pub const HOLD: Duration = Duration::from_millis(400);
pub const DOUBLE: Duration = Duration::from_millis(300);
/// Guia (ou PS) + Select + Start por 2 s liga e desliga tudo, sem teclado.
/// Marca das entradas que o Prisma manda (`dwExtraInfo`): a janela dele não as toma pelo mouse ou
/// teclado de verdade.
pub const MARK: usize = 0x5052_4953;

pub const STOP_COMBO: Buttons = pads::GUIDE | pads::BACK | pads::START;
const STOP_HOLD: Duration = Duration::from_secs(2);
/// Folga antes de trocar de perfil pelo foco.
const SWITCH_DELAY: Duration = Duration::from_millis(400);

pub const CTRL: u8 = 1;
pub const SHIFT: u8 = 2;
pub const ALT: u8 = 4;
pub const WIN: u8 = 8;
const MODS: [(u8, u16, &str); 4] = [(CTRL, 0x11, "ctrl"), (SHIFT, 0x10, "shift"), (ALT, 0x12, "alt"), (WIN, 0x5B, "win")];
const MAX_LAYERS: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Out {
    /// Tecla virtual do Windows com modificadores.
    Key(u16, u8),
    /// 1 esquerdo, 2 direito, 3 meio, 4 e 5 laterais.
    Mouse(u8),
    /// Cliques da roda, para cima positivo.
    Wheel(i8),
    /// Camada enquanto o botão estiver apertado.
    Layer(u8),
    /// Liga e desliga a camada.
    Toggle(u8),
    /// Ação do Prisma (`ACTIONS`), feita pela janela principal.
    Action(u8),
    /// Troca para o perfil com este nome (índice em `Profile::strings`).
    Profile(u8),
    /// Abre este programa (índice em `Profile::strings`).
    Run(u8),
    /// Macro do perfil (índice em `Profile::macros`), feita por `sys::macros`.
    Macro(u8),
    /// Texto rápido do perfil (índice em `Profile::texts`), digitado como texto.
    Text(u8),
}

/// Como a macro roda: uma vez, repetindo enquanto o botão fica apertado, ou repetindo até o
/// próximo aperto.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Once,
    Held,
    Toggle,
}
pub const MODES: [(Mode, &str); 3] = [(Mode::Once, "once"), (Mode::Held, "held"), (Mode::Toggle, "toggle")];

/// Passo de macro: aperta ou solta uma tecla, um botão do mouse ou gira a roda (só `Down`), ou espera.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Step {
    Down(Out),
    Up(Out),
    Wait(u32),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Macro {
    pub name: String,
    pub mode: Mode,
    pub steps: Vec<Step>,
}
pub const MAX_MACROS: usize = 32;
pub const MAX_STEPS: usize = 2000;
pub const MAX_WAIT: u32 = 60_000;

/// Texto rápido: sai por uma ligação, ou quando a abreviação é digitada e seguida de espaço,
/// Enter ou Tab em qualquer programa.
#[derive(Clone, PartialEq, Debug)]
pub struct Snip {
    pub abbr: String,
    pub text: String,
}
pub const MAX_TEXTS: usize = 64;
pub const MAX_TEXT: usize = 500;

/// Link que "Abrir link" aceita: só http ou https, sem espaço, até 500 letras. O resto o Windows
/// poderia tratar como programa ou arquivo.
pub fn link_ok(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    (l.starts_with("https://") || l.starts_with("http://")) && s.len() > l.find("//").unwrap() + 2 && s.len() <= 500 && !s.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"')
}

/// Abreviação vazia (o texto só sai por ligação) ou de 2 a 16 letras e números, em minúsculas.
pub fn abbr_ok(a: &str) -> bool {
    a.is_empty() || ((2..=16).contains(&a.len()) && a.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()))
}

/// Ações do Prisma que um botão pode chamar: todas as que têm atalho, mais HDR, Bluetooth e
/// bateria. A ordem é a do arquivo: só cresce no fim. `SOUND + n` troca para a combinação de som n.
pub const ACTIONS: [&str; 54] = [
    "Foto da área",
    "Foto da tela inteira",
    "Foto da janela em foco",
    "Gravar Janela e parar",
    "Gravar Tela e parar",
    "Salvar replay",
    "Medidor compacto",
    "Medidor completo",
    "Esconder o Medidor",
    "Controles no medidor",
    "Ligar e desligar o registro do Medidor",
    "Ligar e desligar o limite de FPS",
    "Aumentar o limite de FPS",
    "Diminuir o limite de FPS",
    "Ligar e desligar o HDR",
    "Aumentar o volume",
    "Diminuir o volume",
    "Ligar e desligar o som",
    "Tela cheia sem bordas na janela em foco",
    "Mostrar e esconder o widget",
    "Otimizar",
    "Fechar à força",
    "Reiniciar o Explorer",
    "Abrir e fechar a busca",
    "Desconectar o Bluetooth",
    "Mostrar a bateria",
    "Abrir e fechar o teclado do Prisma",
    "Abrir o radial do Prisma",
    "Abrir o Prisma",
    "Plano de energia automático",
    "Plano de energia de jogo",
    "Plano de energia de repouso",
    "Criar convite do Compartilhar tela",
    // Uma por página da barra lateral, na ordem dela (`pages::nav_page`).
    "Página Monitor",
    "Página Medidor",
    "Página Discos e redes",
    "Página Espaço em disco",
    "Página Pesquisar",
    "Página Inicialização",
    "Página Serviços",
    "Página Desinstalar",
    "Página Ajustes do Windows",
    "Página Ferramentas",
    "Página Otimizar",
    "Página Energia",
    "Página Fullscreenizer",
    "Página Som",
    "Página HDR",
    "Página Driver NVIDIA",
    "Página Captura",
    "Página Compartilhar tela",
    "Página Controles",
    "Página Widget",
    "Página Configurações",
];
/// Ação que abre e fecha o teclado do Prisma (`sys::osk`).
pub const KEYBOARD: u8 = 26;
/// Ação que abre o radial do Prisma enquanto o botão fica segurado (`sys::radial`).
pub const RADIAL: u8 = 27;
/// Primeira ação "Página ...": abre a página `n - PAGES` da barra lateral.
pub const PAGES: u8 = 33;
/// Modelos prontos, na ordem de `Profile::template`. O Acessível vem do assistente.
pub const TEMPLATES: [&str; 4] = ["Só Mouse", "Jogo sem controle", "Corrida", "Um lado só"];

/// Primeira ação de combinação de som (a combinação n é `SOUND + n`).
pub const SOUND: u8 = 100;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum When {
    Press,
    Tap,
    Hold,
    Double,
    Release,
    /// Assistência: um aperto liga, o próximo desliga (tecla e clique ficam segurados).
    Latch,
}
const WHENS: [(When, &str); 6] = [
    (When::Press, "press"),
    (When::Tap, "tap"),
    (When::Hold, "hold"),
    (When::Double, "double"),
    (When::Release, "release"),
    (When::Latch, "latch"),
];
/// Turbo 1x: 4 apertos por segundo; 5x: 20, ainda dois quadros por estado num jogo a 60 FPS.
pub const TURBO_HZ: u32 = 4;

/// Turbo de um botão, à parte das ligações: depois de `delay` ms segurando, o que o botão aperta
/// (Pressionar) passa a apertar e soltar sozinho, `TURBO_HZ * speed` vezes por segundo. Parado em
/// jogo com anti-cheat.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Turbo {
    pub button: Buttons,
    pub speed: u8,
    pub delay: u16,
    pub on: bool,
}

impl Turbo {
    /// De fábrica: desligado, 2x, 500 ms.
    pub fn new(button: Buttons) -> Self {
        Turbo { button, speed: 2, delay: 500, on: false }
    }
}

/// Atalho de fábrica do turbo: RS + RT, mais o botão que liga ou desliga.
pub const TURBO_KEY: Buttons = pads::RS | RT;

#[derive(Clone, PartialEq, Debug)]
pub struct Bind {
    pub button: Buttons,
    /// Combinação: só vale com estes botões já apertados, e no lugar da ação normal do botão.
    pub with: Buttons,
    pub when: When,
    pub out: Out,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stick {
    /// As direções viram botões (LSU, LSD...).
    Buttons,
    /// Pixels por segundo com o stick no fim.
    Mouse(f32),
    /// Cliques de roda por segundo com o stick no fim.
    Scroll(f32),
    /// Flick stick: empurrar o stick vira a câmera para aquela direção, girar o stick gira a
    /// câmera. O valor é quantos pixels de mouse dão uma volta inteira no jogo.
    Flick(f32),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Layer {
    pub name: String,
    /// O que a camada não define vem da mãe. Só aponta para uma camada anterior: sem ciclo.
    pub parent: Option<u8>,
    pub ls: Option<Stick>,
    pub rs: Option<Stick>,
    pub binds: Vec<Bind>,
    /// Teclado e mouse: `button` e `with` são teclas virtuais do Windows (botões do mouse: 4 meio,
    /// 5 e 6 laterais), não bits de botão.
    pub keys: Vec<Bind>,
    /// Gyro nesta camada; `None` é igual à mãe (na Base, desligado). Uma camada de segurar com o
    /// gyro ligado ou desligado é o botão de soltar.
    pub gyro: Option<Motion>,
    /// Touchpad nesta camada, igual ao gyro: um dedo move o mouse, dois dedos rolam.
    pub touch: Option<Motion>,
}

/// Gyro ou touchpad numa camada.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Motion {
    Off,
    /// Gyro: pixels por grau girado. Touchpad: pixels pela largura toda.
    Mouse(f32),
    /// Só touchpad: stick virtual, 0 esquerdo e 1 direito. O dedo desvia o stick desde onde encostou.
    Stick(u8),
}

/// Opções do gyro no perfil todo.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct GyroOpt {
    /// Espaço do jogador: virar é sempre em volta da vertical, com o controle deitado ou em pé.
    /// Desligado é o espaço local (o eixo do próprio controle).
    pub player: bool,
    pub inv_x: bool,
    pub inv_y: bool,
    /// Suaviza só os giros pequenos (tremor da mão); os grandes passam direto.
    pub smooth: bool,
}

/// Virar e inclinar, em graus por segundo, do gyro (`g`) e da gravidade medida (`grav`, em g).
pub fn gyro_axes(o: GyroOpt, g: [f32; 3], grav: [f32; 3]) -> (f32, f32) {
    let n = (grav[0] * grav[0] + grav[1] * grav[1] + grav[2] * grav[2]).sqrt();
    if !o.player || n < 0.5 {
        return (g[1], g[0]);
    }
    // Espaço do jogador (como o JoyShockMapper): o giro em volta da vertical real, sem passar do
    // giro que existe nos dois eixos que podem ser essa vertical.
    let world = (grav[1] * g[1] + grav[2] * g[2]) / n;
    let most = (g[1] * g[1] + g[2] * g[2]).sqrt();
    (world.signum() * (world.abs() * 1.41).min(most), g[0])
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Zone {
    pub start: f32,
    pub end: f32,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Profile {
    pub name: String,
    /// Programas (.exe) que usam este perfil.
    pub games: Vec<String>,
    /// Stick esquerdo, direito, gatilho esquerdo, direito.
    pub zones: [Zone; 4],
    pub layers: Vec<Layer>,
    /// Nomes de perfil e programas citados pelas ligações.
    pub strings: Vec<String>,
    /// Turbo de cada botão que já foi configurado.
    pub turbo: Vec<Turbo>,
    /// Botões que, segurados, fazem o próximo botão ligar ou desligar o turbo dele (0: sem atalho).
    pub turbo_key: Buttons,
    pub macros: Vec<Macro>,
    pub texts: Vec<Snip>,
    pub gyro_opt: GyroOpt,
    /// Curva dos sticks como mouse: o expoente (1 reta, 2 suave, 3 forte).
    pub curve: [f32; 2],
    /// Hair trigger em cada gatilho: aperta e solta com um movimento curto, em qualquer ponto.
    pub hair: [bool; 2],
    /// Barra de luz do DualSense e do DualShock 4; `None` não mexe na luz.
    pub light: Option<Light>,
    /// A luz vale também pelo Bluetooth (por padrão, só no cabo).
    pub light_bt: bool,
}

/// Barra de luz no perfil.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Light {
    Off,
    /// Verde, amarela ou vermelha, pela carga.
    Battery,
    Color([u8; 3]),
}

/// Cor com que a barra de luz começa quando o perfil passa a mandar nela.
pub const FIRST_COLOR: [u8; 3] = [0, 128, 255];

pub const DESKTOP: &str = "Desktop";
pub const BASE: &str = "Base";

impl Profile {
    fn empty(name: &str) -> Self {
        let stick = Zone { start: 0.15, end: 0.95 };
        let trig = Zone { start: 0.05, end: 0.95 };
        Profile { name: name.into(), games: vec![], zones: [stick, stick, trig, trig], layers: vec![Layer { name: "Base".into(), parent: None, ls: None, rs: None, binds: vec![], keys: vec![], gyro: None, touch: None }], strings: vec![], turbo: vec![], turbo_key: TURBO_KEY, macros: vec![], texts: vec![], gyro_opt: GyroOpt::default(), curve: [2.0; 2], hair: [false; 2], light: None, light_bt: false }
    }

    /// Base: nada mapeado, o jogo lê o controle como ele é.
    pub fn base() -> Self {
        Self::empty(BASE)
    }

    /// Desktop: stick esquerdo mexe o mouse, o direito rola, e os botões fazem o básico do Windows.
    pub fn desktop() -> Self {
        let mut p = Self::empty(DESKTOP);
        let l = &mut p.layers[0];
        l.ls = Some(Stick::Mouse(1600.0));
        l.rs = Some(Stick::Scroll(12.0));
        let key = |vk: u16, m: u8| Out::Key(vk, m);
        for (button, out) in [
            (pads::A, Out::Mouse(1)),
            (pads::B, Out::Mouse(2)),
            (pads::X, key(0x1B, 0)),
            (pads::Y, key(0x0D, 0)),
            (pads::LB, key(0xA6, 0)),
            (pads::RB, key(0xA7, 0)),
            (pads::START, key(0x5B, 0)),
            (pads::LS, Out::Mouse(3)),
            (pads::RS, Out::Action(KEYBOARD)),
            (pads::UP, key(0x26, 0)),
            (pads::DOWN, key(0x28, 0)),
            (pads::LEFT, key(0x25, 0)),
            (pads::RIGHT, key(0x27, 0)),
            (pads::MUTE, key(0xAD, 0)),
            (LT, key(0xAE, 0)),
            (RT, key(0xAF, 0)),
        ] {
            l.binds.push(Bind { button, with: 0, when: When::Press, out });
        }
        // Select: Win + Tab. Segurado, o botão do radial (cartão Radial do Prisma) fica com ele antes.
        l.binds.push(Bind { button: pads::BACK, with: 0, when: When::Press, out: key(0x09, WIN) });
        p
    }

    /// Modelo pronto (`TEMPLATES`). Só o conteúdo: quem aplica mantém nome e jogos.
    pub fn template(i: usize) -> Self {
        let mut p = Self::empty("");
        let l = &mut p.layers[0];
        let key = |vk: u16| Out::Key(vk, 0);
        let binds: Vec<(Buttons, Out)> = match i {
            // Só Mouse: mouse, rolagem e os três cliques.
            0 => {
                l.ls = Some(Stick::Mouse(1600.0));
                l.rs = Some(Stick::Scroll(12.0));
                vec![(pads::A, Out::Mouse(1)), (pads::B, Out::Mouse(2)), (pads::X, Out::Mouse(3))]
            }
            // Jogo sem controle: WASD no stick esquerdo, câmera no direito, tiro nos gatilhos.
            1 => {
                l.ls = Some(Stick::Buttons);
                l.rs = Some(Stick::Mouse(2400.0));
                vec![
                    (LSU, key(0x57)),
                    (LSD, key(0x53)),
                    (LSL, key(0x41)),
                    (LSR, key(0x44)),
                    (RT, Out::Mouse(1)),
                    (LT, Out::Mouse(2)),
                    (pads::A, key(0x20)),
                    (pads::B, key(0x43)),
                    (pads::X, key(0x52)),
                    (pads::Y, key(0x45)),
                    (pads::LB, key(0x51)),
                    (pads::RB, key(0x46)),
                    (pads::LS, key(0x10)),
                    (pads::RS, key(0x56)),
                    (pads::START, key(0x1B)),
                    (pads::BACK, key(0x09)),
                    (pads::UP, key(0x31)),
                    (pads::RIGHT, key(0x32)),
                    (pads::DOWN, key(0x33)),
                    (pads::LEFT, key(0x34)),
                ]
            }
            // Corrida: acelera e freia nos gatilhos, direção no stick esquerdo.
            2 => {
                l.ls = Some(Stick::Buttons);
                vec![
                    (RT, key(0x57)),
                    (LT, key(0x53)),
                    (LSL, key(0x41)),
                    (LSR, key(0x44)),
                    (pads::A, key(0x20)),
                    (pads::B, key(0x43)),
                    (pads::Y, key(0x52)),
                    (pads::LB, key(0x51)),
                    (pads::RB, key(0x45)),
                    (pads::START, key(0x1B)),
                    (pads::BACK, key(0x09)),
                ]
            }
            // Um lado só: o lado esquerdo do controle faz tudo do mouse.
            _ => return Self::accessible(1, false, 1600.0),
        };
        l.binds = binds.into_iter().map(|(button, out)| Bind { button, with: 0, when: When::Press, out }).collect();
        p
    }

    /// Modelo Acessível, das três respostas: lado (0 os dois, 1 esquerdo, 2 direito), se o clique
    /// esquerdo liga e desliga em vez de segurar, e a velocidade do mouse.
    pub fn accessible(side: u8, latch: bool, speed: f32) -> Self {
        let mut p = if side == 0 { Self::desktop() } else { Self::empty("") };
        let l = &mut p.layers[0];
        if side == 0 {
            l.ls = Some(Stick::Mouse(speed));
        } else {
            let left = side == 1;
            let st = Some(Stick::Mouse(speed));
            if left {
                l.ls = st;
            } else {
                l.rs = st;
            }
            let (up, down, back, fwd, click, right, middle, esc) = if left {
                (pads::UP, pads::DOWN, pads::LEFT, pads::RIGHT, pads::LB, LT, pads::LS, pads::BACK)
            } else {
                (pads::Y, pads::A, pads::X, pads::B, pads::RB, RT, pads::RS, pads::START)
            };
            for (button, out) in [
                (up, Out::Wheel(1)),
                (down, Out::Wheel(-1)),
                (back, Out::Key(0xA6, 0)),
                (fwd, Out::Key(0xA7, 0)),
                (click, Out::Mouse(1)),
                (right, Out::Mouse(2)),
                (middle, Out::Mouse(3)),
                (esc, Out::Key(0x1B, 0)),
            ] {
                l.binds.push(Bind { button, with: 0, when: When::Press, out });
            }
        }
        if latch {
            for b in &mut l.binds {
                if b.out == Out::Mouse(1) {
                    b.when = When::Latch;
                }
            }
        }
        p
    }

    /// Texto do perfil: o mesmo para guardar, importar e exportar.
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mut s = String::from("prisma-profile 1\n");
        let _ = writeln!(s, "name {}", self.name);
        for g in &self.games {
            let _ = writeln!(s, "game {g}");
        }
        for (z, n) in self.zones.iter().zip(["ls", "rs", "lt", "rt"]) {
            let _ = writeln!(s, "zone {n} {} {}", z.start, z.end);
        }
        let _ = writeln!(s, "turbokey {}", mask_text(self.turbo_key));
        for (c, n) in self.curve.iter().zip(["ls", "rs"]) {
            if *c != 2.0 {
                let _ = writeln!(s, "curve {n} {c}");
            }
        }
        for (h, n) in self.hair.iter().zip(["lt", "rt"]) {
            if *h {
                let _ = writeln!(s, "hair {n}");
            }
        }
        if let Some(l) = self.light {
            let bt = if self.light_bt { " bt" } else { "" };
            match l {
                Light::Off => _ = writeln!(s, "light off{bt}"),
                Light::Battery => _ = writeln!(s, "light battery{bt}"),
                Light::Color([r, g, b]) => _ = writeln!(s, "light color {r:02x}{g:02x}{b:02x}{bt}"),
            }
        }
        let o = self.gyro_opt;
        if o != GyroOpt::default() {
            let flags: Vec<&str> = [(o.inv_x, "invx"), (o.inv_y, "invy"), (o.smooth, "smooth")].iter().filter(|f| f.0).map(|f| f.1).collect();
            let _ = writeln!(s, "gyroopt {}{}{}", if o.player { "player" } else { "local" }, if flags.is_empty() { "" } else { " " }, flags.join(" "));
        }
        for t in &self.turbo {
            let _ = writeln!(s, "turbo {} {} {} {}", button_name(t.button), t.speed, t.delay, if t.on { "on" } else { "off" });
        }
        for (i, m) in self.macros.iter().enumerate() {
            let mode = MODES.iter().find(|x| x.0 == m.mode).map_or("once", |x| x.1);
            let _ = writeln!(s, "macro {mode} {}", m.name);
            for st in &m.steps {
                match st {
                    Step::Down(o) => _ = writeln!(s, "mstep {i} down {}", out_text(*o, &self.strings)),
                    Step::Up(o) => _ = writeln!(s, "mstep {i} up {}", out_text(*o, &self.strings)),
                    Step::Wait(ms) => _ = writeln!(s, "mstep {i} wait {ms}"),
                }
            }
        }
        for t in &self.texts {
            let _ = writeln!(s, "text {} {}", if t.abbr.is_empty() { "-" } else { &t.abbr }, t.text);
        }
        for (i, l) in self.layers.iter().enumerate() {
            match l.parent {
                Some(p) => _ = writeln!(s, "layer {p} {}", l.name),
                None => _ = writeln!(s, "layer - {}", l.name),
            }
            for (side, st) in [("ls", l.ls), ("rs", l.rs)] {
                match st {
                    Some(Stick::Buttons) => _ = writeln!(s, "stick {i} {side} buttons"),
                    Some(Stick::Mouse(v)) => _ = writeln!(s, "stick {i} {side} mouse {v}"),
                    Some(Stick::Scroll(v)) => _ = writeln!(s, "stick {i} {side} scroll {v}"),
                    Some(Stick::Flick(v)) => _ = writeln!(s, "stick {i} {side} flick {v}"),
                    None => {}
                }
            }
            for (key, m) in [("gyro", l.gyro), ("touch", l.touch)] {
                match m {
                    Some(Motion::Off) => _ = writeln!(s, "{key} {i} off"),
                    Some(Motion::Mouse(v)) => _ = writeln!(s, "{key} {i} mouse {v}"),
                    Some(Motion::Stick(side)) => _ = writeln!(s, "{key} {i} stick {side}"),
                    None => {}
                }
            }
            for b in &l.binds {
                let mut button = button_name(b.button).to_string();
                for (n, bit) in NAMES {
                    if b.with & bit != 0 {
                        button += "+";
                        button += n;
                    }
                }
                let when = WHENS.iter().find(|w| w.0 == b.when).map_or("press", |w| w.1);
                let _ = writeln!(s, "bind {i} {button} {when} {}", out_text(b.out, &self.strings));
            }
            for b in &l.keys {
                let when = WHENS.iter().find(|w| w.0 == b.when).map_or("press", |w| w.1);
                let src = if b.with == 0 { b.button.to_string() } else { format!("{}+{}", b.button, b.with) };
                let _ = writeln!(s, "kbind {i} {src} {when} {}", out_text(b.out, &self.strings));
            }
        }
        s
    }

    /// Lê um perfil. O texto pode vir de fora (importado), então tudo é conferido: botão, tecla,
    /// camada, número. Linha que não entende derruba o perfil inteiro, para não usar meio perfil.
    pub fn parse(text: &str) -> Option<Self> {
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        if lines.next()? != "prisma-profile 1" {
            return None;
        }
        let mut p = Profile { layers: vec![], ..Self::empty("") };
        for line in lines {
            let (cmd, rest) = line.split_once(' ')?;
            match cmd {
                "name" => p.name = rest.chars().filter(|c| !c.is_control()).take(60).collect(),
                "game" => p.games.push(rest.to_lowercase()),
                "turbokey" => p.turbo_key = if rest == "-" { 0 } else { mask_of(rest)? },
                "turbo" => {
                    let mut it = rest.split(' ');
                    let button = button_bit(it.next()?)?;
                    let speed = it.next()?.parse::<u8>().ok().filter(|v| (2..=5).contains(v))?;
                    let delay = it.next()?.parse::<u16>().ok().filter(|v| *v <= 5000)?;
                    let on = match it.next()? {
                        "on" => true,
                        "off" => false,
                        _ => return None,
                    };
                    if it.next().is_some() || p.turbo.iter().any(|t| t.button == button) {
                        return None;
                    }
                    p.turbo.push(Turbo { button, speed, delay, on });
                }
                "curve" => {
                    let (k, v) = rest.split_once(' ')?;
                    p.curve[["ls", "rs"].iter().position(|n| *n == k)?] = num(v)?.clamp(1.0, 3.0);
                }
                "hair" => p.hair[["lt", "rt"].iter().position(|n| *n == rest)?] = true,
                "light" => {
                    let mut it = rest.split(' ');
                    p.light = Some(match it.next()? {
                        "off" => Light::Off,
                        "battery" => Light::Battery,
                        "color" => {
                            let h = it.next().filter(|h| h.len() == 6)?;
                            let v = u32::from_str_radix(h, 16).ok()?;
                            Light::Color([(v >> 16) as u8, (v >> 8) as u8, v as u8])
                        }
                        _ => return None,
                    });
                    p.light_bt = it.next() == Some("bt");
                }
                "zone" => {
                    let mut it = rest.split(' ');
                    let k = it.next()?;
                    let i = ["ls", "rs", "lt", "rt"].iter().position(|n| *n == k)?;
                    let (a, b) = (num(it.next()?)?, num(it.next()?)?);
                    if !(a < b && b <= 1.0) {
                        return None;
                    }
                    p.zones[i] = Zone { start: a, end: b };
                }
                "layer" => {
                    let (parent, name) = rest.split_once(' ').unwrap_or((rest, ""));
                    let parent = if parent == "-" { None } else { Some(parent.parse::<u8>().ok().filter(|&x| (x as usize) < p.layers.len())?) };
                    if p.layers.len() >= MAX_LAYERS {
                        return None;
                    }
                    p.layers.push(Layer { name: name.chars().filter(|c| !c.is_control()).take(40).collect(), parent, ls: None, rs: None, binds: vec![], keys: vec![], gyro: None, touch: None });
                }
                "gyroopt" => {
                    let mut it = rest.split(' ');
                    let mut o = GyroOpt { player: it.next()? == "player", ..Default::default() };
                    for f in it {
                        match f {
                            "invx" => o.inv_x = true,
                            "invy" => o.inv_y = true,
                            "smooth" => o.smooth = true,
                            _ => return None,
                        }
                    }
                    p.gyro_opt = o;
                }
                "gyro" | "touch" => {
                    let mut it = rest.split(' ');
                    let l = layer_of(&p, it.next()?)?;
                    let max = if cmd == "gyro" { 200.0 } else { 8000.0 };
                    let m = Some(match it.next()? {
                        "off" => Motion::Off,
                        "mouse" => Motion::Mouse(num(it.next()?)?.clamp(0.5, max)),
                        "stick" if cmd == "touch" => Motion::Stick(it.next()?.parse::<u8>().ok().filter(|s| *s < 2)?),
                        _ => return None,
                    });
                    if cmd == "gyro" {
                        p.layers[l].gyro = m;
                    } else {
                        p.layers[l].touch = m;
                    }
                }
                "stick" => {
                    let mut it = rest.split(' ');
                    let l = layer_of(&p, it.next()?)?;
                    let side = it.next()?;
                    let st = match it.next()? {
                        "buttons" => Stick::Buttons,
                        "mouse" => Stick::Mouse(num(it.next()?)?.min(20000.0)),
                        "scroll" => Stick::Scroll(num(it.next()?)?.min(200.0)),
                        "flick" => Stick::Flick(num(it.next()?)?.clamp(100.0, 100000.0)),
                        _ => return None,
                    };
                    match side {
                        "ls" => p.layers[l].ls = Some(st),
                        "rs" => p.layers[l].rs = Some(st),
                        _ => return None,
                    }
                }
                "bind" => {
                    let mut it = rest.splitn(4, ' ');
                    let l = layer_of(&p, it.next()?)?;
                    let mut names = it.next()?.split('+');
                    let button = button_bit(names.next()?)?;
                    let mut with = 0;
                    for n in names {
                        with |= button_bit(n)?;
                    }
                    let w = it.next()?;
                    let when = WHENS.iter().find(|x| x.1 == w)?.0;
                    let out = parse_out(it.next()?, &mut p.strings)?;
                    if with & button != 0 || p.layers[l].binds.len() >= 256 {
                        return None;
                    }
                    p.layers[l].binds.push(Bind { button, with, when, out });
                }
                "kbind" => {
                    let mut it = rest.splitn(4, ' ');
                    let l = layer_of(&p, it.next()?)?;
                    let vk = |s: &str| s.parse::<Buttons>().ok().filter(|v| (1..=254).contains(v));
                    let src = it.next()?;
                    let (button, with) = match src.split_once('+') {
                        Some((a, b)) => (vk(a)?, vk(b)?),
                        None => (vk(src)?, 0),
                    };
                    let w = it.next()?;
                    let when = WHENS.iter().find(|x| x.1 == w)?.0;
                    let out = parse_out(it.next()?, &mut p.strings)?;
                    if with == button || p.layers[l].keys.len() >= 256 {
                        return None;
                    }
                    p.layers[l].keys.push(Bind { button, with, when, out });
                }
                "text" => {
                    let (abbr, text) = rest.split_once(' ')?;
                    let abbr = if abbr == "-" { String::new() } else { abbr.to_string() };
                    let dup = !abbr.is_empty() && p.texts.iter().any(|t| t.abbr == abbr);
                    if !abbr_ok(&abbr) || dup || text.chars().count() > MAX_TEXT || text.chars().any(char::is_control) || p.texts.len() >= MAX_TEXTS {
                        return None;
                    }
                    p.texts.push(Snip { abbr, text: text.into() });
                }
                "macro" => {
                    let (mode, name) = rest.split_once(' ').unwrap_or((rest, ""));
                    let mode = MODES.iter().find(|x| x.1 == mode)?.0;
                    let name: String = name.chars().filter(|c| !c.is_control()).take(40).collect();
                    if p.macros.len() >= MAX_MACROS || name.is_empty() {
                        return None;
                    }
                    p.macros.push(Macro { name, mode, steps: vec![] });
                }
                "mstep" => {
                    let mut it = rest.splitn(3, ' ');
                    let m = it.next()?.parse::<usize>().ok().filter(|&m| m < p.macros.len())?;
                    let kind = it.next()?;
                    let arg = it.next()?;
                    let step = match kind {
                        "wait" => Step::Wait(arg.parse::<u32>().ok().filter(|v| *v <= MAX_WAIT)?),
                        "down" | "up" => {
                            // Só tecla, mouse e roda (a roda só aperta).
                            let o = parse_out(arg, &mut p.strings)?;
                            match (o, kind) {
                                (Out::Key(..) | Out::Mouse(_) | Out::Wheel(_), "down") => Step::Down(o),
                                (Out::Key(..) | Out::Mouse(_), _) => Step::Up(o),
                                _ => return None,
                            }
                        }
                        _ => return None,
                    };
                    if p.macros[m].steps.len() >= MAX_STEPS {
                        return None;
                    }
                    p.macros[m].steps.push(step);
                }
                _ => return None,
            }
        }
        // Camada citada numa ligação tem que existir.
        let n = p.layers.len();
        let ok = p.layers.iter().flat_map(|l| l.binds.iter().chain(&l.keys)).all(|b| !matches!(b.out, Out::Layer(x) | Out::Toggle(x) if x as usize >= n) && !matches!(b.out, Out::Macro(x) if x as usize >= p.macros.len()) && !matches!(b.out, Out::Text(x) if x as usize >= p.texts.len()));
        (ok && n > 0 && !p.name.is_empty()).then_some(p)
    }
}

fn num(s: &str) -> Option<f32> {
    s.parse::<f32>().ok().filter(|v| v.is_finite() && *v >= 0.0)
}

fn layer_of(p: &Profile, s: &str) -> Option<usize> {
    s.parse::<usize>().ok().filter(|&l| l < p.layers.len())
}

pub fn button_name(bit: Buttons) -> &'static str {
    NAMES.iter().find(|n| n.1 == bit).map_or("?", |n| n.0)
}

fn button_bit(s: &str) -> Option<Buttons> {
    NAMES.iter().find(|n| n.0 == s).map(|n| n.1)
}

/// "RS+RT"; "-" sem botão.
fn mask_text(m: Buttons) -> String {
    let v: Vec<&str> = NAMES.iter().filter(|n| m & n.1 != 0).map(|n| n.0).collect();
    if v.is_empty() { "-".into() } else { v.join("+") }
}

fn mask_of(s: &str) -> Option<Buttons> {
    s.split('+').try_fold(0, |m, n| button_bit(n).map(|b| m | b))
}

/// O que já usa esta combinação de botões, para avisar antes de dar a mesma a outra coisa: a parada
/// de emergência, o atalho do turbo ou uma ligação com "junto com", em qualquer camada.
/// `for_turbo` ao conferir o próprio atalho do turbo (que vale com qualquer botão a mais).
pub fn combo_conflict(p: &Profile, combo: Buttons, for_turbo: bool) -> Option<String> {
    let tr = |s: &str| crate::i18n::tr(s).into_owned();
    if combo.count_ones() < 2 {
        return None;
    }
    if combo & STOP_COMBO == STOP_COMBO || STOP_COMBO & combo == combo {
        return Some(tr("a parada de emergência"));
    }
    if !for_turbo && p.turbo_key != 0 && combo & p.turbo_key == p.turbo_key {
        return Some(tr("o atalho do turbo"));
    }
    for l in &p.layers {
        for b in &l.binds {
            let used = b.button | b.with;
            // Para o turbo: a ligação usa só botões do atalho, ou o atalho inteiro mais um botão.
            let hit = b.with != 0 && if for_turbo { used & combo == used || used & combo == combo } else { used == combo };
            if hit {
                return Some(tr("uma ligação da camada {}").replace("{}", &l.name));
            }
        }
    }
    None
}

fn out_text(o: Out, strings: &[String]) -> String {
    let text = |i: u8| strings.get(i as usize).map_or("", |s| s.as_str());
    match o {
        Out::Key(vk, m) => {
            let mut s = format!("key {vk}");
            for (bit, _, n) in MODS {
                if m & bit != 0 {
                    s += " ";
                    s += n;
                }
            }
            s
        }
        Out::Mouse(n) => format!("mouse {n}"),
        Out::Wheel(n) => format!("wheel {n}"),
        Out::Layer(n) => format!("layer {n}"),
        Out::Toggle(n) => format!("toggle {n}"),
        Out::Action(n) => format!("action {n}"),
        Out::Profile(n) => format!("profile {}", text(n)),
        Out::Run(n) => format!("run {}", text(n)),
        Out::Macro(n) => format!("macro {n}"),
        Out::Text(n) => format!("text {n}"),
    }
}

/// Guarda o texto no perfil e devolve o índice (o mesmo texto não se repete).
pub fn intern(strings: &mut Vec<String>, s: &str) -> Option<u8> {
    match strings.iter().position(|x| x == s) {
        Some(i) => Some(i as u8),
        None if strings.len() < 255 => {
            strings.push(s.to_string());
            Some(strings.len() as u8 - 1)
        }
        None => None,
    }
}

fn parse_out(s: &str, strings: &mut Vec<String>) -> Option<Out> {
    // Perfil e programa levam o resto da linha, com espaços.
    if let Some(rest) = s.strip_prefix("profile ").or_else(|| s.strip_prefix("run ")) {
        let rest = rest.trim();
        if rest.is_empty() || rest.chars().any(|c| c.is_control()) || rest.len() > 400 {
            return None;
        }
        let i = intern(strings, rest)?;
        return Some(if s.starts_with("run ") { Out::Run(i) } else { Out::Profile(i) });
    }
    let mut it = s.split(' ');
    let kind = it.next()?;
    let n = it.next()?;
    let out = match kind {
        "key" => {
            let vk = n.parse::<u16>().ok().filter(|v| (1..=254).contains(v))?;
            let mut m = 0;
            for name in it.by_ref() {
                m |= MODS.iter().find(|x| x.2 == name)?.0;
            }
            Out::Key(vk, m)
        }
        "mouse" => Out::Mouse(n.parse::<u8>().ok().filter(|v| (1..=5).contains(v))?),
        "wheel" => Out::Wheel(n.parse::<i8>().ok().filter(|v| *v != 0)?),
        "layer" => Out::Layer(n.parse::<u8>().ok()?),
        "toggle" => Out::Toggle(n.parse::<u8>().ok()?),
        "macro" => Out::Macro(n.parse::<u8>().ok().filter(|&m| (m as usize) < MAX_MACROS)?),
        "text" => Out::Text(n.parse::<u8>().ok().filter(|&m| (m as usize) < MAX_TEXTS)?),
        "action" => Out::Action(n.parse::<u8>().ok().filter(|&a| (a as usize) < ACTIONS.len() || (SOUND..SOUND + crate::app::SND_KEYS_MAX as u8).contains(&a))?),
        _ => return None,
    };
    it.next().is_none().then_some(out)
}

// ---------- Zonas mortas ----------

/// Zona morta radial com zona externa: abaixo de `start` é zero, acima de `end` é o máximo, e o
/// meio é esticado para começar do zero sem salto.
pub fn radial(x: f32, y: f32, z: Zone) -> (f32, f32) {
    let m = (x * x + y * y).sqrt();
    if m <= z.start {
        return (0.0, 0.0);
    }
    let s = ((m - z.start) / (z.end - z.start).max(0.01)).min(1.0) / m;
    (x * s, y * s)
}

/// Gatilho: início e fim do curso.
pub fn linear(v: f32, z: Zone) -> f32 {
    ((v - z.start) / (z.end - z.start).max(0.01)).clamp(0.0, 1.0)
}

// ---------- Motor ----------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Act {
    Down(Out),
    Up(Out),
    Move(i32, i32),
    /// Em unidades do Windows (120 por clique).
    Wheel(i32),
    /// Ação, troca de perfil ou programa: fica para quem chama o motor.
    Cmd(Out),
    /// O atalho do turbo foi usado com este botão: ligar ou desligar o turbo dele.
    Turbo(Buttons),
}

#[derive(Default)]
struct Btn {
    down: Option<Instant>,
    /// Camada onde foi apertado: o que ele apertou é solto lá, mesmo se a camada mudou.
    layer: u8,
    hold_fired: bool,
    /// Já usado (combinação ou duplo): não toca, não segura, não solta.
    used: bool,
    outs: Vec<Out>,
    released: Option<Instant>,
    /// Toque esperando para ver se vem o duplo.
    tap: Option<(Instant, Vec<Out>)>,
    /// Turbo: o que repete, a próxima virada, se está apertado agora e meio período.
    turbo: Option<(Vec<Out>, Instant, bool, Duration)>,
}

pub struct Engine {
    btn: [Btn; COUNT],
    prev: Buttons,
    holds: Vec<u8>,
    toggles: Vec<u8>,
    /// Quantos botões seguram cada saída: a tecla só solta quando o último soltar.
    count: HashMap<Out, u32>,
    mx: f32,
    my: f32,
    wheel: f32,
    last: Option<Instant>,
    /// Saídas presas pelo Alternar.
    latched: Vec<Out>,
    /// Gravidade medida pelo acelerômetro (média lenta) e a média curta do gyro para suavizar.
    grav: [f32; 3],
    soft: [f32; 2],
    /// Dedos no touchpad no relatório anterior.
    fingers: [Option<(f32, f32)>; 2],
    /// Gesto em curso: quando o dedo encostou, onde, onde está, e se já não vale (apertou ou pôs
    /// outro dedo).
    swipe: Option<(Instant, (f32, f32), (f32, f32), bool)>,
    /// Metade do clique do touchpad em curso (TPCL ou TPCR), escolhida ao apertar.
    half: Buttons,
    /// Flick stick de cada lado: o ângulo do stick no relatório anterior (fora do centro), e o giro
    /// que ainda falta do flick, em pixels, com o tempo que ainda tem.
    flick: [Option<f32>; 2],
    flick_go: [(f32, f32); 2],
    /// Hair trigger: se o gatilho está apertado, e o ponto mais fundo (apertado) ou mais raso
    /// (solto) desde a última troca.
    hair: [(bool, f32); 2],
    /// Jogo com anti-cheat em foco: turbo vira Pressionar.
    pub safe: bool,
}

impl Default for Engine {
    fn default() -> Self {
        // Começa com tudo "apertado": o que já estava apertado ao ligar não dispara nada.
        Engine {
            btn: std::array::from_fn(|_| Btn::default()),
            prev: ALL,
            holds: vec![],
            toggles: vec![],
            count: HashMap::new(),
            mx: 0.0,
            my: 0.0,
            wheel: 0.0,
            last: None,
            latched: vec![],
            safe: false,
            grav: [0.0; 3],
            soft: [0.0; 2],
            fingers: [None; 2],
            swipe: None,
            half: 0,
            flick: [None; 2],
            flick_go: [(0.0, 0.0); 2],
            hair: [(false, 0.0); 2],
        }
    }
}

fn binds(p: &Profile, layer: u8, bit: Buttons, chord: bool) -> Vec<&Bind> {
    let mut l = Some(layer);
    while let Some(i) = l {
        let Some(ly) = p.layers.get(i as usize) else { break };
        let v: Vec<&Bind> = ly.binds.iter().filter(|b| b.button == bit && (b.with != 0) == chord).collect();
        if !v.is_empty() {
            return v;
        }
        l = ly.parent;
    }
    vec![]
}

fn stick(p: &Profile, layer: u8, right: bool) -> Stick {
    let mut l = Some(layer);
    while let Some(i) = l {
        let Some(ly) = p.layers.get(i as usize) else { break };
        if let Some(s) = if right { ly.rs } else { ly.ls } {
            return s;
        }
        l = ly.parent;
    }
    Stick::Buttons
}

/// Hair trigger: aperta quando o gatilho desce 4% do ponto mais raso, solta quando sobe 4% do
/// mais fundo; solto de todo sempre solta.
fn hair(s: &mut (bool, f32), v: f32) -> bool {
    const STEP: f32 = 0.04;
    if s.0 {
        s.1 = s.1.max(v);
        if v <= 0.0 || v < s.1 - STEP {
            *s = (false, v);
        }
    } else {
        s.1 = s.1.min(v);
        if v > s.1 + STEP {
            *s = (true, v);
        }
    }
    s.0
}

/// Stick virtual no touchpad: o desvio do dedo desde onde encostou, cheio a um quarto da largura;
/// para cima é positivo, como no stick.
fn touch_stick(from: (f32, f32), to: (f32, f32)) -> (f32, f32) {
    let (x, y) = ((to.0 - from.0) / 0.25, (from.1 - to.1) * 0.55 / 0.25);
    let m = x.hypot(y).max(1.0);
    (x / m, y / m)
}

/// Gyro (`touch` falso) ou touchpad da camada, herdado da mãe; na Base, desligado.
fn motion(p: &Profile, layer: u8, touch: bool) -> Motion {
    let mut l = Some(layer);
    while let Some(i) = l {
        let Some(ly) = p.layers.get(i as usize) else { break };
        if let Some(g) = if touch { ly.touch } else { ly.gyro } {
            return g;
        }
        l = ly.parent;
    }
    Motion::Off
}

fn outs(v: &[&Bind], when: When) -> Vec<Out> {
    v.iter().filter(|b| b.when == when).map(|b| b.out).collect()
}

impl Engine {
    pub fn layer(&self) -> u8 {
        self.holds.last().or(self.toggles.last()).copied().unwrap_or(0)
    }

    /// Nada apertado: nem tecla, nem camada, nem botão no meio de um toque. O que o Alternar prendeu
    /// não conta: a troca de perfil solta.
    pub fn idle(&self) -> bool {
        self.count.keys().all(|o| self.latched.contains(o)) && self.holds.is_empty() && self.btn.iter().all(|b| b.down.is_none() && b.tap.is_none())
    }

    fn down(&mut self, o: Out, acts: &mut Vec<Act>) {
        match o {
            Out::Layer(n) => self.holds.push(n),
            Out::Toggle(n) => match self.toggles.iter().position(|&x| x == n) {
                Some(i) => _ = self.toggles.remove(i),
                None => self.toggles.push(n),
            },
            Out::Wheel(n) => acts.push(Act::Wheel(n as i32 * 120)),
            Out::Action(_) | Out::Profile(_) | Out::Run(_) | Out::Text(_) => acts.push(Act::Cmd(o)),
            _ => {
                let c = self.count.entry(o).or_default();
                *c += 1;
                if *c == 1 {
                    acts.push(Act::Down(o));
                }
            }
        }
    }

    fn up(&mut self, o: Out, acts: &mut Vec<Act>) {
        match o {
            Out::Layer(n) => {
                if let Some(i) = self.holds.iter().rposition(|&x| x == n) {
                    self.holds.remove(i);
                }
            }
            Out::Toggle(_) | Out::Wheel(_) | Out::Action(_) | Out::Profile(_) | Out::Run(_) | Out::Text(_) => {}
            _ => {
                if let Some(c) = self.count.get_mut(&o) {
                    *c -= 1;
                    if *c == 0 {
                        self.count.remove(&o);
                        acts.push(Act::Up(o));
                    }
                }
            }
        }
    }

    fn tap(&mut self, o: Out, acts: &mut Vec<Act>) {
        self.down(o, acts);
        self.up(o, acts);
    }

    /// Teclas que o botão `i` segura agora (a repetição do teclado manda de novo).
    fn held_keys(&self, i: usize) -> Vec<Out> {
        self.btn[i].outs.iter().copied().filter(|o| matches!(o, Out::Key(..))).collect()
    }

    /// Solta tudo e esquece o estado. O que continuar apertado não dispara de novo.
    pub fn release_all(&mut self) -> Vec<Act> {
        let acts: Vec<Act> = self.count.drain().map(|(o, _)| Act::Up(o)).collect();
        let safe = self.safe;
        *self = Engine { safe, ..Engine::default() };
        acts
    }

    pub fn step(&mut self, p: &Profile, inp: &Input, now: Instant) -> Vec<Act> {
        let mut acts = vec![];
        let dt = self.last.map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32().min(0.05));
        self.last = Some(now);
        let prev = self.prev;
        let tm = motion(p, self.layer(), true);
        let gest = self.gesture(inp, now, tm != Motion::Off);
        // Stick virtual: vale o mais desviado entre o stick de verdade e o dedo.
        let (mut l, mut r) = ((inp.lx, inp.ly), (inp.rx, inp.ry));
        if let (Motion::Stick(side), Some((_, from, to, _))) = (tm, self.swipe) {
            let t = touch_stick(from, to);
            let s = if side == 0 { &mut l } else { &mut r };
            if t.0.hypot(t.1) > s.0.hypot(s.1) {
                *s = t;
            }
        }
        let (lx, ly) = radial(l.0, l.1, p.zones[0]);
        let (rx, ry) = radial(r.0, r.1, p.zones[1]);
        let (lt, rt) = (linear(inp.lt, p.zones[2]), linear(inp.rt, p.zones[3]));
        // Com folga: liga em 0,5 e só desliga abaixo de 0,4, para não piscar na borda.
        let on = |v: f32, bit: Buttons| if v > 0.5 || (v > 0.4 && prev & bit != 0) { bit } else { 0 };
        let mut trig = [on(lt, LT), on(rt, RT)];
        for (i, v) in [lt, rt].into_iter().enumerate() {
            if p.hair[i] {
                trig[i] = if hair(&mut self.hair[i], v) { [LT, RT][i] } else { 0 };
            }
        }
        // Até o fim: liga em 0,95 e só desliga abaixo de 0,9.
        let full = |v: f32, bit: Buttons| if v > 0.95 || (v > 0.9 && prev & bit != 0) { bit } else { 0 };
        let cur = (inp.buttons & 0x1ffff) | gest | trig[0] | trig[1] | full(lt, LTF) | full(rt, RTF) | on(ly, LSU) | on(-ly, LSD) | on(-lx, LSL) | on(lx, LSR) | on(ry, RSU) | on(-ry, RSD) | on(-rx, RSL) | on(rx, RSR);
        let changed = cur ^ prev;
        // Soltar antes de apertar: uma troca de botões no mesmo relatório solta a tecla velha antes.
        for i in 0..COUNT {
            if changed & !cur & (1 << i) != 0 {
                self.release(p, i, now, &mut acts);
            }
        }
        for i in 0..COUNT {
            if changed & cur & (1 << i) != 0 {
                self.press(p, i, cur, now, &mut acts);
            }
        }
        for i in 0..COUNT {
            let b = &self.btn[i];
            if let Some(t) = b.down {
                if !b.hold_fired && !b.used && now.saturating_duration_since(t) >= HOLD {
                    // A camada de segurar já entrou no aperto (ver `press`).
                    let o: Vec<Out> = outs(&binds(p, b.layer, 1 << i, false), When::Hold).into_iter().filter(|o| !matches!(o, Out::Layer(_))).collect();
                    self.btn[i].hold_fired = true;
                    for o in o {
                        self.down(o, &mut acts);
                        self.btn[i].outs.push(o);
                    }
                }
            }
            if let Some((o, next, down, half)) = self.btn[i].turbo.take() {
                if now >= next {
                    for &o in &o {
                        if down {
                            self.up(o, &mut acts);
                        } else {
                            self.down(o, &mut acts);
                        }
                    }
                    // Atrasou demais (controle que só manda relatório ao mudar): recomeça daqui.
                    let next = if now.saturating_duration_since(next) > half { now + half } else { next + half };
                    self.btn[i].turbo = Some((o, next, !down, half));
                } else {
                    self.btn[i].turbo = Some((o, next, down, half));
                }
            }
            if self.btn[i].tap.as_ref().is_some_and(|t| now >= t.0) {
                let (_, o) = self.btn[i].tap.take().unwrap();
                for o in o {
                    self.tap(o, &mut acts);
                }
            }
        }
        self.prev = cur;
        let layer = self.layer();
        for (right, x, y) in [(false, lx, ly), (true, rx, ry)] {
            match stick(p, layer, right) {
                Stick::Mouse(v) => {
                    // Curva: devagar perto do centro, rápido no fim.
                    let c = p.curve[usize::from(right)];
                    self.mx += x * x.abs().powf(c - 1.0) * v * dt;
                    self.my -= y * y.abs().powf(c - 1.0) * v * dt;
                }
                Stick::Scroll(v) => self.wheel += y * v * 120.0 * dt,
                Stick::Flick(px) => {
                    let s = usize::from(right);
                    // Com folga: liga no fim do curso (0,9) e só solta abaixo de 0,8.
                    let r = x.hypot(y);
                    if r > 0.9 || (r > 0.8 && self.flick[s].is_some()) {
                        // Para cima é 0°, para a direita é positivo, como virar a câmera.
                        let a = x.atan2(y).to_degrees();
                        match self.flick[s] {
                            // Flick: a câmera vira para onde o stick aponta, em 0,1 s.
                            None => self.flick_go[s] = (self.flick_go[s].0 + a / 360.0 * px, 0.1),
                            // Girar o stick gira a câmera junto, pelo caminho mais curto.
                            Some(b) => self.mx += ((a - b + 540.0).rem_euclid(360.0) - 180.0) / 360.0 * px,
                        }
                        self.flick[s] = Some(a);
                    } else {
                        self.flick[s] = None;
                    }
                }
                Stick::Buttons => {}
            }
        }
        for g in &mut self.flick_go {
            if g.0 != 0.0 {
                let part = if g.1 <= dt { g.0 } else { g.0 * dt / g.1 };
                self.mx += part;
                *g = (g.0 - part, (g.1 - dt).max(0.0));
            }
        }
        if inp.motion {
            // Gravidade: média de uns 0,25 s, que o tremor da mão não mexe.
            let k = (dt * 4.0).min(1.0);
            for a in 0..3 {
                self.grav[a] += (inp.accel[a] - self.grav[a]) * k;
            }
            if let Motion::Mouse(px) = motion(p, layer, false) {
                let o = p.gyro_opt;
                let (mut yaw, mut pitch) = gyro_axes(o, inp.gyro, self.grav);
                if o.smooth {
                    // Abaixo de 5 °/s vale a média curta; acima de 10 °/s, o valor cru; no meio, a mistura.
                    for (v, s) in [(&mut yaw, 0), (&mut pitch, 1)] {
                        self.soft[s] += (*v - self.soft[s]) * (dt * 25.0).min(1.0);
                        let w = ((v.abs() - 5.0) / 5.0).clamp(0.0, 1.0);
                        *v = w * *v + (1.0 - w) * self.soft[s];
                    }
                }
                // Virar para a direita leva o cursor para a direita; inclinar para cima, para cima.
                self.mx += if o.inv_x { yaw } else { -yaw } * px * dt;
                self.my += if o.inv_y { pitch } else { -pitch } * px * dt;
            }
        }
        if let Motion::Mouse(px) = motion(p, layer, true) {
            // Só o dedo que já estava no relatório anterior conta: encostar não pula o cursor.
            let d = |k: usize| match (inp.touch[k], self.fingers[k]) {
                (Some(a), Some(b)) => Some((a.0 - b.0, a.1 - b.1)),
                _ => None,
            };
            // shortcut: altura do touchpad como 0,55 da largura (DualSense 0,56, DualShock 4 0,49), basta para o mouse.
            match (inp.touch.iter().flatten().count(), d(0), d(1)) {
                (1, Some(v), _) | (1, _, Some(v)) => {
                    self.mx += v.0 * px;
                    self.my += v.1 * px * 0.55;
                }
                // Dois dedos: a página segue os dedos, como no touchpad do notebook.
                (2, Some(a), Some(b)) => self.wheel += (a.1 + b.1) / 2.0 * 1800.0,
                _ => {}
            }
        }
        self.fingers = inp.touch;
        let (dx, dy) = (self.mx.trunc(), self.my.trunc());
        if dx != 0.0 || dy != 0.0 {
            acts.push(Act::Move(dx as i32, dy as i32));
            self.mx -= dx;
            self.my -= dy;
        }
        let w = self.wheel.trunc();
        if w != 0.0 {
            acts.push(Act::Wheel(w as i32));
            self.wheel -= w;
        }
        acts
    }

    /// Gestos do touchpad como botões: o deslizar e o toque valem um relatório (aperta e solta no
    /// seguinte); a metade do clique e as regiões ficam apertadas enquanto duram. Com o touchpad
    /// como mouse ou stick, o deslizar não vale (o dedo é o mouse ou o stick).
    fn gesture(&mut self, inp: &Input, now: Instant, mouse: bool) -> Buttons {
        let click = inp.buttons & pads::PAD != 0;
        let quads = inp.touch.iter().flatten().fold(0, |b, t| b | [TPUL, TPUR, TPDL, TPDR][usize::from(t.0 >= 0.5) + 2 * usize::from(t.1 >= 0.5)]);
        let first = inp.touch[0].or(inp.touch[1]);
        if !click {
            self.half = 0;
        } else if self.half == 0 {
            self.half = if first.map_or(true, |t| t.0 < 0.5) { TPCL } else { TPCR };
        }
        let n = inp.touch.iter().flatten().count();
        match (&mut self.swipe, first) {
            (None, Some(at)) => self.swipe = Some((now, at, at, click || n > 1)),
            (Some(s), Some(at)) => {
                s.2 = at;
                s.3 |= click || n > 1;
            }
            (Some(_), None) => {
                let (t0, from, to, spoiled) = self.swipe.take().unwrap();
                // Em larguras do touchpad: a altura é uns 0,55 da largura.
                let (dx, dy) = (to.0 - from.0, (to.1 - from.1) * 0.55);
                let t = now.saturating_duration_since(t0);
                if spoiled || t > Duration::from_millis(800) {
                } else if dx.abs().max(dy.abs()) < 0.03 && t < Duration::from_millis(250) {
                    return quads | self.half | TPTAP;
                } else if !mouse && dx.abs() > 0.2 && dx.abs() > dy.abs() * 1.5 {
                    return quads | self.half | if dx > 0.0 { TPR } else { TPL };
                } else if !mouse && dy.abs() > 0.12 && dy.abs() > dx.abs() * 1.5 {
                    return quads | self.half | if dy > 0.0 { TPD } else { TPU };
                }
            }
            (None, None) => {}
        }
        quads | self.half
    }

    fn press(&mut self, p: &Profile, i: usize, cur: Buttons, now: Instant, acts: &mut Vec<Act>) {
        let bit: Buttons = 1 << i;
        let layer = self.layer();
        let pending = self.btn[i].tap.take();
        let released = self.btn[i].released;
        self.btn[i] = Btn { down: Some(now), layer, ..Default::default() };
        let key = p.turbo_key;
        if key != 0 && bit & key == 0 && cur & key == key {
            for j in 0..COUNT {
                if key & (1 << j) != 0 {
                    self.btn[j].used = true;
                    self.btn[j].tap = None;
                }
            }
            self.btn[i].used = true;
            acts.push(Act::Turbo(bit));
            return;
        }
        let chord: Vec<Bind> = binds(p, layer, bit, true).into_iter().filter(|b| cur & b.with == b.with).cloned().collect();
        if !chord.is_empty() {
            for b in &chord {
                for j in 0..COUNT {
                    if b.with & (1 << j) != 0 {
                        self.btn[j].used = true;
                        // O toque guardado do outro botão também foi usado pela combinação.
                        self.btn[j].tap = None;
                    }
                }
                self.down(b.out, acts);
                self.btn[i].outs.push(b.out);
            }
            self.btn[i].used = true;
            return;
        }
        let all = binds(p, layer, bit, false);
        let double = outs(&all, When::Double);
        if !double.is_empty() && released.is_some_and(|r| now.saturating_duration_since(r) <= DOUBLE) {
            self.btn[i].used = true;
            for o in double {
                self.down(o, acts);
                self.btn[i].outs.push(o);
            }
            return;
        }
        if let Some((_, o)) = pending {
            for o in o {
                self.tap(o, acts);
            }
        }
        // Camada em "segurar" entra já no aperto, para o stick e os botões apertados junto usarem a
        // camada sem esperar 0,4 s; se o botão soltar antes disso, o toque dele sai como sempre.
        let layers = outs(&all, When::Hold).into_iter().filter(|o| matches!(o, Out::Layer(_)));
        for o in outs(&all, When::Press).into_iter().chain(layers) {
            self.down(o, acts);
            self.btn[i].outs.push(o);
        }
        for o in outs(&all, When::Latch) {
            if !matches!(o, Out::Key(..) | Out::Mouse(_)) {
                self.tap(o, acts);
            } else if let Some(k) = self.latched.iter().position(|&x| x == o) {
                self.latched.remove(k);
                self.up(o, acts);
            } else {
                self.latched.push(o);
                self.down(o, acts);
            }
        }
        // Turbo: só o que o botão aperta (teclas e cliques), depois da espera.
        if let Some(t) = p.turbo.iter().find(|t| t.button == bit && t.on).filter(|_| !self.safe) {
            let o: Vec<Out> = self.btn[i].outs.iter().copied().filter(|o| matches!(o, Out::Key(..) | Out::Mouse(_))).collect();
            if !o.is_empty() {
                let half = Duration::from_micros(500_000 / (TURBO_HZ * t.speed.clamp(2, 5) as u32) as u64);
                let first = if t.delay == 0 { half } else { Duration::from_millis(t.delay as u64) };
                self.btn[i].turbo = Some((o, now + first, true, half));
            }
        }
    }

    fn release(&mut self, p: &Profile, i: usize, now: Instant, acts: &mut Vec<Act>) {
        let b = std::mem::take(&mut self.btn[i]);
        // O turbo pode estar no meio do "solto": o que já está solto não solta de novo.
        let loose = match &b.turbo {
            Some((o, _, false, _)) => o.clone(),
            _ => vec![],
        };
        for &o in &b.outs {
            if !loose.contains(&o) {
                self.up(o, acts);
            }
        }
        let Some(t) = b.down else { return };
        if b.used {
            return;
        }
        let all = binds(p, b.layer, 1 << i, false);
        if !b.hold_fired && now.saturating_duration_since(t) < HOLD {
            let tap = outs(&all, When::Tap);
            if !tap.is_empty() && all.iter().any(|x| x.when == When::Double) {
                self.btn[i].tap = Some((now + DOUBLE, tap));
            } else {
                for o in tap {
                    self.tap(o, acts);
                }
            }
        }
        for o in outs(&all, When::Release) {
            self.tap(o, acts);
        }
        self.btn[i].released = Some(now);
    }
}

// ---------- No Windows ----------

static ON: AtomicBool = AtomicBool::new(false);
static STOPPED: AtomicBool = AtomicBool::new(false);
static RUN: Mutex<Option<Run>> = Mutex::new(None);
/// Última olhada no foco: quando, a janela e se ela ocupava a tela inteira.
static FOCUS: Mutex<(Option<Instant>, isize, bool)> = Mutex::new((None, 0, false));
/// O programa em foco é de administrador: o Windows não deixa mandar teclas a ele, então o teclado
/// e o mouse passam direto.
static FRONT_ADMIN: AtomicBool = AtomicBool::new(false);
/// Motor do teclado e do mouse, entre os dos controles.
const KBD: &str = "teclado";
/// Teclas do perfil que o motor vê como botões (um bit cada).
/// shortcut: até 17 teclas diferentes por perfil, os bits de botão que o motor lê; mais pede máscara maior.
pub const KEY_SLOTS: usize = 17;

struct Run {
    profiles: Vec<Profile>,
    cur: usize,
    /// Jogos do Medidor: usam o perfil Base quando não têm perfil próprio.
    games: Vec<String>,
    /// Motor de cada controle, e desde quando a combinação de parada está apertada.
    engines: HashMap<String, (Engine, Option<(Instant, bool)>)>,
    want: Option<(usize, Instant)>,
    /// Do relatório chegar até o `SendInput` voltar, em µs, nos últimos envios.
    lat: Vec<u32>,
    warned: HashSet<String>,
    /// O programa em foco tem anti-cheat: turbo parado. A pasta de cada .exe é lida uma vez.
    ac: bool,
    ac_seen: HashMap<String, bool>,
    /// Ligações de teclado e mouse do perfil em uso, como perfil de botões, e a tecla de cada bit.
    kbd: Option<(Profile, Vec<u16>)>,
    /// Teclas apertadas (bits) e as que o Prisma engoliu ao apertar (a soltura também fica com ele).
    kheld: Buttons,
    kswallow: Buttons,
    /// Palavra sendo digitada, para a abreviação do texto rápido ("#": longa demais, não vale).
    typed: String,
}

/// As ligações de teclado e mouse de `p` como um perfil de botões: cada tecla usada ganha um bit.
fn key_profile(p: &Profile) -> Option<(Profile, Vec<u16>)> {
    let mut slots: Vec<u16> = vec![];
    for b in p.layers.iter().flat_map(|l| &l.keys) {
        for vk in [b.button, b.with] {
            if vk != 0 && !slots.contains(&(vk as u16)) && slots.len() < KEY_SLOTS {
                slots.push(vk as u16);
            }
        }
    }
    if slots.is_empty() {
        return None;
    }
    let bit = |vk: Buttons| slots.iter().position(|&s| s as Buttons == vk).map(|i| 1 << i);
    let layers = p
        .layers
        .iter()
        .map(|l| Layer {
            name: l.name.clone(),
            parent: l.parent,
            ls: None,
            rs: None,
            binds: l.keys.iter().filter_map(|b| Some(Bind { button: bit(b.button)?, with: if b.with == 0 { 0 } else { bit(b.with)? }, when: b.when, out: b.out })).collect(),
            keys: vec![],
            gyro: None,
            touch: None,
        })
        .collect();
    Some((Profile { layers, turbo: vec![], turbo_key: 0, ..p.clone() }, slots))
}

/// Perfil em uso mudou (ou o mapeamento ligou, desligou ou parou): refaz as teclas e diz ao gancho
/// se ele é preciso (1 teclado, 2 mouse).
fn set_keys(r: &mut Run) {
    r.kbd = key_profile(&r.profiles[r.cur]);
    r.kheld = 0;
    r.kswallow = 0;
    r.typed.clear();
    let mut need = r.kbd.as_ref().map_or(0, |(_, slots)| slots.iter().fold(0, |n, &vk| n | if (4..=6).contains(&vk) { 2 } else { 1 }));
    if r.profiles[r.cur].texts.iter().any(|t| !t.abbr.is_empty()) {
        need |= 1;
    }
    // Jogo com anti-cheat: nenhum gancho; a thread fica só com o relógio, para ver o foco mudar.
    let on = ON.load(Ordering::Relaxed) && !STOPPED.load(Ordering::Relaxed);
    super::keymap::sync(if !on || need == 0 { 0 } else if r.ac { 4 } else { need });
}

fn profiles_dir() -> std::path::PathBuf {
    crate::config::data_dir().join("perfis")
}

/// Desktop e Base sempre existem; os arquivos da pasta `perfis` substituem ou somam.
fn load_profiles() -> Vec<Profile> {
    let mut list = vec![Profile::desktop(), Profile::base()];
    if let Ok(dir) = std::fs::read_dir(profiles_dir()) {
        for e in dir.flatten() {
            let Some(p) = std::fs::read_to_string(e.path()).ok().and_then(|t| Profile::parse(&t)) else { continue };
            match list.iter_mut().find(|x| x.name == p.name) {
                Some(x) => *x = p,
                None => list.push(p),
            }
        }
    }
    list
}

/// Perfis como estão agora: Desktop, Base e os da pasta.
pub fn profiles() -> Vec<Profile> {
    RUN.lock().unwrap().as_ref().map_or_else(load_profiles, |r| r.profiles.clone())
}

/// O perfil tem arquivo próprio (para Desktop e Base: foi mudado e pode voltar ao de fábrica).
pub fn has_file(name: &str) -> bool {
    profiles_dir().join(file_name(name)).exists()
}

fn file_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || " -_.".contains(c) { c } else { '_' }).collect();
    format!("{}.txt", s.trim())
}

/// Grava o perfil e passa a usar a versão nova.
pub fn save(p: &Profile) -> bool {
    let dir = profiles_dir();
    let ok = std::fs::create_dir_all(&dir).is_ok() && crate::config::write_safe(&dir.join(file_name(&p.name)), p.to_text()).is_ok();
    reload();
    ok
}

/// Apaga o arquivo do perfil. Desktop e Base voltam ao de fábrica.
pub fn remove(name: &str) {
    let _ = std::fs::remove_file(profiles_dir().join(file_name(name)));
    reload();
}

/// Relê os perfis. Solta tudo antes: as camadas e as teclas podem ter mudado.
fn reload() {
    super::macros::stop_all();
    let acts = {
        let mut g = RUN.lock().unwrap();
        let Some(r) = g.as_mut() else { return };
        let name = r.profiles[r.cur].name.clone();
        r.profiles = load_profiles();
        r.cur = r.profiles.iter().position(|p| p.name == name).unwrap_or(0);
        r.want = None;
        set_keys(r);
        r.engines.values_mut().flat_map(|e| e.0.release_all()).collect::<Vec<_>>()
    };
    send(&acts);
    // O programa em foco é conferido de novo: um perfil novo pode ser o dele.
    *FOCUS.lock().unwrap() = (None, 0, false);
}

/// Liga ou desliga o mapeamento. `games` são os .exe do Medidor.
pub fn configure(on: bool, games: Vec<String>) {
    let acts = {
        let mut g = RUN.lock().unwrap();
        let r = g.get_or_insert_with(|| Run { profiles: load_profiles(), cur: 0, games: vec![], engines: HashMap::new(), want: None, lat: vec![], warned: HashSet::new(), ac: false, ac_seen: HashMap::new(), kbd: None, kheld: 0, kswallow: 0, typed: String::new() });
        r.games = games.into_iter().map(|s| s.to_lowercase()).collect();
        ON.store(on, Ordering::Relaxed);
        set_keys(r);
        if on {
            vec![]
        } else {
            super::macros::stop_all();
            let acts: Vec<Act> = r.engines.values_mut().flat_map(|e| e.0.release_all()).collect();
            r.engines.clear();
            acts
        }
    };
    send(&acts);
    super::pads::poke();
}

pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Botões ligados sozinhos, no perfil em uso, a abrir e fechar o teclado do Prisma.
pub fn keyboard_buttons() -> Buttons {
    let g = RUN.lock().unwrap();
    let Some(r) = g.as_ref().filter(|_| enabled()) else { return 0 };
    r.profiles[r.cur].layers.iter().flat_map(|l| &l.binds).filter(|b| b.out == Out::Action(KEYBOARD) && b.with == 0).fold(0, |m, b| m | b.button)
}

pub fn stopped() -> bool {
    STOPPED.load(Ordering::Relaxed)
}

/// Luz que o editor de perfis mostra enquanto está aberto, na frente da do perfil em uso.
static PREVIEW: Mutex<Option<(Light, bool)>> = Mutex::new(None);

/// O editor de perfis mostra esta luz no controle enquanto está aberto; `None` solta a mostra.
pub fn preview_light(l: Option<(Light, bool)>) {
    *PREVIEW.lock().unwrap() = l;
}

/// Luz do perfil em uso e se vale pelo Bluetooth. Vale com o mapeamento em Nativo (a luz não mexe
/// no que o jogo recebe); só a parada de emergência apaga tudo.
pub fn light() -> Option<(Light, bool)> {
    if STOPPED.load(Ordering::Relaxed) {
        return None;
    }
    if let Some(l) = *PREVIEW.lock().unwrap() {
        return Some(l);
    }
    let g = RUN.lock().unwrap();
    g.as_ref().and_then(|r| {
        let p = &r.profiles[r.cur];
        p.light.map(|l| (l, p.light_bt))
    })
}

pub fn active() -> Option<String> {
    let g = RUN.lock().unwrap();
    g.as_ref().filter(|_| enabled()).map(|r| r.profiles[r.cur].name.clone())
}

/// Mediana e p99 do tempo interno, em µs.
pub fn latency() -> Option<(u32, u32)> {
    let mut v = RUN.lock().unwrap().as_ref()?.lat.clone();
    if v.len() < 20 {
        return None;
    }
    v.sort_unstable();
    Some((v[v.len() / 2], v[v.len() * 99 / 100]))
}

/// Um relatório novo do controle `key`, que chegou em `arrived`.
pub fn feed(key: &str, inp: &Input, arrived: Instant) {
    // Teclado aberto: o controle digita nele, mesmo com o mapeamento desligado.
    if super::osk::feed(key, inp, arrived) || super::radial::feed(key, inp, arrived) {
        return;
    }
    // Prisma em foco: o controle navega por ele.
    let rest = super::nav::feed(key, inp, arrived);
    let inp = rest.as_ref().unwrap_or(inp);
    if !ON.load(Ordering::Relaxed) {
        return;
    }
    let mut events = vec![];
    let mut radial = None;
    let acts = {
        let mut g = RUN.lock().unwrap();
        let Some(r) = g.as_mut() else { return };
        let mut acts = vec![];
        // Parada de emergência: segurar a combinação liga e desliga; dispara uma vez por aperto.
        let fire = {
            let e = r.engines.entry(key.to_string()).or_default();
            if inp.buttons & STOP_COMBO == STOP_COMBO {
                let (since, fired) = e.1.get_or_insert((arrived, false));
                let fire = !*fired && arrived.saturating_duration_since(*since) >= STOP_HOLD;
                *fired |= fire;
                fire
            } else {
                e.1 = None;
                false
            }
        };
        if fire {
            let on = !STOPPED.load(Ordering::Relaxed);
            acts.extend(stop(r, on));
            events.push(pads::Event::Stopped { on });
        }
        if !STOPPED.load(Ordering::Relaxed) {
            // Botão que abre o radial: o perfil não o vê; o toque (ou o segurar) volta como aperto.
            let trig = super::radial::trigger(key, inp.buttons, arrived, r.profiles[r.cur].name == BASE);
            radial = trig.open;
            if trig.win_g {
                acts.extend([Act::Down(Out::Key(0x47, WIN)), Act::Up(Out::Key(0x47, WIN))]);
            }
            for buttons in trig.steps {
                let p = &r.profiles[r.cur];
                let e = r.engines.get_mut(key).unwrap();
                e.0.safe = r.ac;
                let step = e.0.step(p, &Input { buttons, ..*inp }, arrived);
                handle(r, key, step, &mut acts, &mut events);
            }
            apply_want(r, arrived, &mut acts, &mut events);
        }
        acts
    };
    if !acts.is_empty() {
        send(&acts);
        let us = arrived.elapsed().as_micros().min(u32::MAX as u128) as u32;
        if let Some(r) = RUN.lock().unwrap().as_mut() {
            if r.lat.len() >= 4096 {
                r.lat.drain(..2048);
            }
            r.lat.push(us);
        }
    }
    events.into_iter().for_each(pads::emit);
    if let Some(o) = radial {
        super::radial::open_from(o);
    }
    focus_tick(arrived);
}

/// O que o motor do controle (ou do teclado) `key` pediu: ações, programas, perfis e macros saem
/// como eventos; teclas e cliques vão para `acts`.
fn handle(r: &mut Run, key: &str, step: Vec<Act>, acts: &mut Vec<Act>, events: &mut Vec<pads::Event>) {
    let strings = r.profiles[r.cur].strings.clone();
    for a in step {
                match a {
                    Act::Cmd(Out::Action(n)) => events.push(pads::Event::Action { id: n, pad: key.to_string() }),
                    Act::Cmd(Out::Text(n)) => {
                        if let Some(t) = r.profiles[r.cur].texts.get(n as usize) {
                            super::osk::type_text(&t.text);
                        }
                    }
                    Act::Cmd(Out::Run(n)) => events.extend(strings.get(n as usize).map(|s| pads::Event::Run { path: s.clone() })),
                    Act::Cmd(Out::Profile(n)) => {
                        // Troca na mão: vale até o foco mudar de programa.
                        if let Some(to) = strings.get(n as usize).and_then(|s| r.profiles.iter().position(|p| p.name == *s)) {
                            acts.extend(switch(r, to));
                            events.push(pads::Event::Profile { name: r.profiles[to].name.clone() });
                        }
                    }
                    Act::Down(Out::Macro(_)) if r.ac => {
                        if r.warned.insert("ac:macro".into()) {
                            events.push(pads::Event::Notice { text: "Jogo com anti-cheat: as macros ficam paradas nele.".into() });
                        }
                    }
                    Act::Down(Out::Macro(n)) => {
                        if let Some(m) = r.profiles[r.cur].macros.get(n as usize) {
                            super::macros::press(key, n, m);
                        }
                    }
                    Act::Up(Out::Macro(n)) => super::macros::release(key, n),
                    Act::Turbo(bit) => {
                        // Vale para o perfil em uso até o editor gravar de novo ou o Prisma fechar.
                        let p = &mut r.profiles[r.cur];
                        let i = p.turbo.iter().position(|t| t.button == bit).unwrap_or_else(|| {
                            p.turbo.push(Turbo::new(bit));
                            p.turbo.len() - 1
                        });
                        p.turbo[i].on = !p.turbo[i].on;
                        let on = p.turbo[i].on;
                        events.push(pads::Event::Turbo { button: bit, on, pad: key.to_string() });
                    }
                    a => acts.push(a),
                }
    }
}

/// Troca de perfil pelo foco, esperando a folga e nada apertado.
fn apply_want(r: &mut Run, now: Instant, acts: &mut Vec<Act>, events: &mut Vec<pads::Event>) {
    if let Some((to, at)) = r.want {
        if now >= at && r.engines.values().all(|e| e.0.idle()) {
            acts.extend(switch(r, to));
            events.push(pads::Event::Profile { name: r.profiles[to].name.clone() });
        }
    }
}

/// Tecla `vk` (ou botão do mouse 4 a 6) apertada ou solta, vinda do gancho. Devolve se o Prisma
/// fica com ela (o programa em foco não a vê). Fora do mapeamento, com o Prisma em foco e com um
/// programa de administrador em foco, tudo passa direto.
pub fn key_event(vk: u16, down: bool) -> bool {
    if down {
        if let Some((back, text)) = expansion(vk) {
            // A abreviação sai, o texto entra e a tecla que fechou a palavra vem depois.
            send(&[Act::Down(Out::Key(0x08, 0)), Act::Up(Out::Key(0x08, 0))].repeat(back));
            super::osk::type_text(&text);
            send(&[Act::Down(Out::Key(vk, 0)), Act::Up(Out::Key(vk, 0))]);
            return true;
        }
    }
    let now = Instant::now();
    let mut acts = vec![];
    let mut events = vec![];
    let swallow = {
        let mut g = RUN.lock().unwrap();
        let Some(r) = g.as_mut() else { return false };
        let Some(i) = r.kbd.as_ref().and_then(|k| k.1.iter().position(|&s| s == vk)) else { return false };
        let bit: Buttons = 1 << i;
        let kp = r.kbd.as_ref().unwrap().0.clone();
        let live = ON.load(Ordering::Relaxed) && !STOPPED.load(Ordering::Relaxed) && !super::nav::front() && !FRONT_ADMIN.load(Ordering::Relaxed) && !r.ac;
        let e = r.engines.entry(KBD.into()).or_insert_with(|| (Engine { prev: 0, ..Engine::default() }, None));
        if down && r.kheld & bit != 0 {
            // Repetição do Windows: a tecla ligada repete também.
            let mine = r.kswallow & bit != 0;
            let again: Vec<Act> = if mine { e.0.held_keys(i).into_iter().map(Act::Down).collect() } else { vec![] };
            drop(g);
            send(&again);
            return mine;
        }
        if down {
            if !live {
                return false;
            }
            r.kheld |= bit;
            let layer = e.0.layer();
            let held = r.kheld;
            if !binds(&kp, layer, bit, false).is_empty() || binds(&kp, layer, bit, true).iter().any(|b| held & b.with == b.with) {
                r.kswallow |= bit;
            }
        } else if r.kheld & bit == 0 {
            return false;
        } else {
            r.kheld &= !bit;
        }
        let held = r.kheld;
        let step = r.engines.get_mut(KBD).unwrap().0.step(&kp, &Input { buttons: held, ..Default::default() }, now);
        handle(r, KBD, step, &mut acts, &mut events);
        let mine = r.kswallow & bit != 0;
        if !down {
            r.kswallow &= !bit;
        }
        mine
    };
    send(&acts);
    events.into_iter().for_each(pads::emit);
    swallow
}

/// Abreviação do texto rápido terminada por esta tecla: quantas letras apagar e o texto.
fn expansion(vk: u16) -> Option<(usize, String)> {
    let mut g = RUN.lock().unwrap();
    let r = g.as_mut()?;
    let live = ON.load(Ordering::Relaxed) && !STOPPED.load(Ordering::Relaxed) && !super::nav::front() && !FRONT_ADMIN.load(Ordering::Relaxed) && !r.ac;
    let texts = &r.profiles[r.cur].texts;
    if !live || texts.iter().all(|t| t.abbr.is_empty()) {
        r.typed.clear();
        return None;
    }
    let modded = [0x11, 0x12, 0x5B, 0x5C].iter().any(|&k| unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(k) } < 0);
    let i = typed_key(&mut r.typed, vk, modded, texts)?;
    Some((texts[i].abbr.len(), texts[i].text.clone()))
}

/// Uma tecla apertada: letras e números fazem a palavra; espaço, Enter ou Tab depois de uma
/// abreviação devolvem o texto dela; qualquer outra tecla (ou com Ctrl, Alt ou Win) começa outra.
/// shortcut: clique do mouse não zera a palavra (sem gancho de mouse); a troca de janela zera.
fn typed_key(word: &mut String, vk: u16, modded: bool, texts: &[Snip]) -> Option<usize> {
    match vk {
        // Shift e Caps Lock não mudam a palavra.
        0x10 | 0x14 | 0xA0 | 0xA1 => None,
        _ if modded => {
            word.clear();
            None
        }
        0x08 => {
            word.pop();
            None
        }
        0x30..=0x39 | 0x41..=0x5A | 0x60..=0x69 => {
            if word.len() >= 16 {
                *word = "#".into();
            } else {
                word.push(match vk {
                    0x60..=0x69 => (b'0' + (vk - 0x60) as u8) as char,
                    _ => (vk as u8 as char).to_ascii_lowercase(),
                });
            }
            None
        }
        0x20 | 0x0D | 0x09 => {
            let hit = texts.iter().position(|t| !t.abbr.is_empty() && t.abbr == *word);
            word.clear();
            hit
        }
        _ => {
            word.clear();
            None
        }
    }
}

/// A cada 30 ms com o gancho ligado: segurar, toque duplo, troca de perfil pelo foco.
pub fn key_tick() {
    let now = Instant::now();
    let mut acts = vec![];
    let mut events = vec![];
    {
        let mut g = RUN.lock().unwrap();
        let Some(r) = g.as_mut() else { return };
        if let Some(kp) = r.kbd.as_ref().map(|k| k.0.clone()) {
            let held = r.kheld;
            if let Some(e) = r.engines.get_mut(KBD) {
                let step = e.0.step(&kp, &Input { buttons: held, ..Default::default() }, now);
                handle(r, KBD, step, &mut acts, &mut events);
            }
        }
        apply_want(r, now, &mut acts, &mut events);
    }
    send(&acts);
    events.into_iter().for_each(pads::emit);
    // Sem controle mandando relatórios, o foco é olhado daqui, fora da thread do gancho (ler a
    // pasta de um jogo pode demorar).
    static LOOKING: AtomicBool = AtomicBool::new(false);
    if !LOOKING.swap(true, Ordering::Relaxed) {
        std::thread::spawn(move || {
            focus_tick(now);
            LOOKING.store(false, Ordering::Relaxed);
        });
    }
}

/// Passa ao perfil `to` e solta tudo dos controles: as teclas da camada velha não ficam presas.
fn switch(r: &mut Run, to: usize) -> Vec<Act> {
    super::macros::stop_all();
    r.want = None;
    r.cur = to;
    set_keys(r);
    r.engines.values_mut().flat_map(|e| e.0.release_all()).collect()
}

/// Para (`on`) ou volta o mapeamento, soltando tudo.
fn stop(r: &mut Run, on: bool) -> Vec<Act> {
    super::macros::stop_all();
    STOPPED.store(on, Ordering::Relaxed);
    set_keys(r);
    r.engines.values_mut().flat_map(|e| e.0.release_all()).collect()
}

/// Troca de perfil pedida de fora (Prisma Launcher): vale até o programa em foco mudar.
pub fn switch_to(name: &str) -> bool {
    let acts = {
        let mut g = RUN.lock().unwrap();
        let Some(r) = g.as_mut() else { return false };
        let Some(to) = r.profiles.iter().position(|p| p.name == name) else { return false };
        switch(r, to)
    };
    send(&acts);
    pads::emit(pads::Event::Profile { name: name.to_string() });
    true
}

/// Parada pedida de fora; devolve se ficou parado.
pub fn set_stopped(on: bool) -> bool {
    let acts = RUN.lock().unwrap().as_mut().map(|r| stop(r, on)).unwrap_or_default();
    send(&acts);
    pads::emit(pads::Event::Stopped { on });
    on
}

/// Olha o programa em foco a cada 250 ms enquanto algum controle manda relatórios: pega também o
/// jogo que abre em janela e só depois vai para a tela cheia, o que o aviso de foco do Windows não dá.
fn focus_tick(now: Instant) {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let mut last = FOCUS.lock().unwrap();
    if last.0.is_some_and(|t| now.saturating_duration_since(t) < Duration::from_millis(250)) {
        return;
    }
    last.0 = Some(now);
    let h = unsafe { GetForegroundWindow() };
    let full = super::fullscreen::covers_monitor(h);
    if (h.0 as isize, full) != (last.1, last.2) {
        (last.1, last.2) = (h.0 as isize, full);
        drop(last);
        on_focus(super::procs::window_pid(h), full);
    }
}

/// Solta tudo o que os controles seguram (o teclado do Prisma abriu).
pub fn release_held() {
    super::macros::stop_all();
    let acts: Vec<Act> = RUN.lock().unwrap().as_mut().map(|r| r.engines.values_mut().flat_map(|e| e.0.release_all()).collect()).unwrap_or_default();
    send(&acts);
}

/// O controle saiu: solta o que ele segurava.
pub fn forget(key: &str) {
    super::macros::stop_pad(key);
    let acts = RUN.lock().unwrap().as_mut().and_then(|r| r.engines.remove(key)).map(|mut e| e.0.release_all()).unwrap_or_default();
    send(&acts);
}

/// Programa novo em foco. Troca de perfil depois da folga, e só com nada apertado. Jogo é o que está
/// no Medidor ou o que ocupa a tela inteira (menos a área de trabalho do Windows).
fn on_focus(pid: u32, full: bool) {
    if !ON.load(Ordering::Relaxed) || pid == 0 || pid == std::process::id() {
        return;
    }
    let Some(path) = super::procs::image_path(pid) else { return };
    let exe = super::procs::exe_name(&path).to_lowercase();
    let elevated = !crate::startup::is_elevated() && is_elevated(pid);
    FRONT_ADMIN.store(elevated, Ordering::Relaxed);
    let seen = RUN.lock().unwrap().as_ref().and_then(|r| r.ac_seen.get(&exe).copied());
    // Fora da trava: ler a pasta do jogo pode demorar.
    let ac = seen.unwrap_or_else(|| crate::meter::anti_cheat(&path));
    let mut events = vec![];
    if let Some(r) = RUN.lock().unwrap().as_mut() {
        r.ac_seen.insert(exe.clone(), ac);
        let was = std::mem::replace(&mut r.ac, ac);
        r.typed.clear();
        if was != ac {
            set_keys(r);
        }
        let named = |n: &str| r.profiles.iter().position(|p| p.name == n).unwrap_or(0);
        let game = r.games.contains(&exe) || (full && exe != "explorer.exe");
        let to = r.profiles.iter().position(|p| p.games.contains(&exe)).unwrap_or_else(|| if game { named(BASE) } else { named(DESKTOP) });
        r.want = (to != r.cur).then(|| (to, Instant::now() + SWITCH_DELAY));
        let turbo = r.profiles[to].turbo.iter().any(|t| t.on);
        if ac && turbo && r.warned.insert(format!("ac:{exe}")) {
            events.push(pads::Event::Notice { text: format!("{exe} tem anti-cheat: o turbo fica parado nele.") });
        }
        if elevated && r.warned.insert(exe.clone()) {
            events.push(pads::Event::Elevated { exe });
        }
    }
    events.into_iter().for_each(pads::emit);
}

/// Programa como administrador: o Windows não deixa um programa comum mandar teclas para ele.
fn is_elevated(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut token = HANDLE::default();
        // Sem permissão para ler o token de outro programa, ele é de administrador.
        let elevated = if OpenProcessToken(h, TOKEN_QUERY, &mut token).is_err() {
            true
        } else {
            let mut e = TOKEN_ELEVATION::default();
            let mut len = 0;
            let ok = GetTokenInformation(token, TokenElevation, Some(&mut e as *mut _ as *mut _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len);
            let _ = CloseHandle(token);
            ok.is_ok() && e.TokenIsElevated != 0
        };
        let _ = CloseHandle(h);
        elevated
    }
}

/// Teste do flick stick: depois de 3 s (tempo de voltar ao jogo), move o mouse `px` para a direita
/// em 1 s. Se a câmera der uma volta inteira e parar onde estava, o valor está certo.
pub fn test_turn(px: f32) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        let mut done = 0;
        for i in 1..=100 {
            let to = (px * i as f32 / 100.0) as i32;
            send(&[Act::Move(to - done, 0)]);
            done = to;
            std::thread::sleep(Duration::from_millis(10));
        }
    });
}

pub(super) fn send(acts: &[Act]) {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    if acts.is_empty() {
        return;
    }
    let key = |vk: u16, up: bool| -> INPUT {
        let scan = unsafe { MapVirtualKeyW(vk as u32, MAPVK_VK_TO_VSC) } as u16;
        // Setas, Insert, Delete, Home, End, PgUp, PgDn, Windows, menu, divisão, NumLock, Ctrl e Alt
        // da direita e as teclas de mídia são "estendidas".
        let ext = matches!(vk, 0x21..=0x28 | 0x2D | 0x2E | 0x5B..=0x5D | 0x6F | 0x90 | 0xA3 | 0xA5 | 0xA6..=0xB7);
        let mut f = if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) };
        if ext {
            f |= KEYEVENTF_EXTENDEDKEY;
        }
        INPUT { r#type: INPUT_KEYBOARD, Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: f, dwExtraInfo: MARK, ..Default::default() } } }
    };
    let mouse = |flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32| INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, dwExtraInfo: MARK, ..Default::default() } },
    };
    let button = |n: u8, up: bool| match (n, up) {
        (1, false) => mouse(MOUSEEVENTF_LEFTDOWN, 0, 0, 0),
        (1, true) => mouse(MOUSEEVENTF_LEFTUP, 0, 0, 0),
        (2, false) => mouse(MOUSEEVENTF_RIGHTDOWN, 0, 0, 0),
        (2, true) => mouse(MOUSEEVENTF_RIGHTUP, 0, 0, 0),
        (3, false) => mouse(MOUSEEVENTF_MIDDLEDOWN, 0, 0, 0),
        (3, true) => mouse(MOUSEEVENTF_MIDDLEUP, 0, 0, 0),
        (n, up) => mouse(if up { MOUSEEVENTF_XUP } else { MOUSEEVENTF_XDOWN }, 0, 0, if n == 4 { 1 } else { 2 }),
    };
    let mut list = vec![];
    for a in acts {
        match *a {
            Act::Down(Out::Key(vk, m)) => {
                list.extend(MODS.iter().filter(|x| m & x.0 != 0).map(|x| key(x.1, false)));
                list.push(key(vk, false));
            }
            Act::Up(Out::Key(vk, m)) => {
                list.push(key(vk, true));
                list.extend(MODS.iter().rev().filter(|x| m & x.0 != 0).map(|x| key(x.1, true)));
            }
            Act::Down(Out::Mouse(n)) => list.push(button(n, false)),
            Act::Up(Out::Mouse(n)) => list.push(button(n, true)),
            Act::Move(dx, dy) => list.push(mouse(MOUSEEVENTF_MOVE, dx, dy, 0)),
            Act::Wheel(n) => list.push(mouse(MOUSEEVENTF_WHEEL, 0, 0, n)),
            _ => {}
        }
    }
    if !list.is_empty() {
        unsafe {
            SendInput(&list, std::mem::size_of::<INPUT>() as i32);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gyro_moves_mouse_only_where_the_layer_turns_it_on() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        p.layers.push(Layer { name: "Mira".into(), parent: Some(0), ls: None, rs: None, binds: vec![], keys: vec![], gyro: Some(Motion::Mouse(10.0)), touch: None });
        one(&mut p.layers[0], pads::LB, When::Press, Out::Layer(1));
        let mut e = Engine::default();
        let turn = |b: Buttons| Input { buttons: b, gyro: [0.0, -90.0, 0.0], accel: [0.0, 1.0, 0.0], motion: true, ..Default::default() };
        e.step(&p, &Input::default(), at(0, t0));
        // Na Base o gyro está desligado.
        assert!(e.step(&p, &turn(0), at(10, t0)).is_empty());
        e.step(&p, &turn(pads::LB), at(20, t0));
        // Virando para a direita a 90 °/s, 10 px por grau, 10 ms: 9 px para a direita.
        assert!(matches!(e.step(&p, &turn(pads::LB), at(30, t0))[..], [Act::Move(8 | 9, 0)]), "{:?}", e.step(&p, &turn(pads::LB), at(40, t0)));
        let text = p.to_text();
        assert!(text.contains("gyro 1 mouse 10"));
        assert_eq!(Profile::parse(&text).unwrap().layers[1].gyro, Some(Motion::Mouse(10.0)));
    }

    #[test]
    fn triggers_hair_and_full_pull() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        p.zones[2] = Zone { start: 0.0, end: 1.0 };
        p.hair[0] = true;
        one(&mut p.layers[0], LT, When::Press, Out::Key(0x41, 0));
        one(&mut p.layers[0], LTF, When::Press, Out::Key(0x42, 0));
        let mut e = Engine::default();
        let f = |lt: f32| Input { lt, ..Default::default() };
        let mut run = |v: f32, ms: u64| e.step(&p, &f(v), at(ms, t0));
        run(0.0, 0);
        // Hair: 10% já aperta; voltar de 50% para 44% solta; descer de novo aperta.
        assert!(run(0.1, 10).contains(&Act::Down(Out::Key(0x41, 0))));
        run(0.5, 20);
        assert!(run(0.44, 30).contains(&Act::Up(Out::Key(0x41, 0))));
        assert!(run(0.5, 40).contains(&Act::Down(Out::Key(0x41, 0))));
        // Até o fim: o segundo estágio.
        assert!(run(1.0, 50).contains(&Act::Down(Out::Key(0x42, 0))));
        assert!(run(0.85, 60).contains(&Act::Up(Out::Key(0x42, 0))));
        p.curve[1] = 3.0;
        p.light = Some(Light::Color([255, 96, 0]));
        p.light_bt = true;
        let q = Profile::parse(&p.to_text()).unwrap();
        assert_eq!((q.hair, q.curve, q.light, q.light_bt), ([true, false], [2.0, 3.0], p.light, true));
    }

    #[test]
    fn flick_stick_turns_and_rotates() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        p.layers[0].rs = Some(Stick::Flick(3600.0));
        let mut e = Engine::default();
        let moved = |acts: Vec<Act>| acts.iter().map(|a| if let Act::Move(x, _) = a { *x } else { 0 }).sum::<i32>();
        let f = |rx: f32, ry: f32| Input { rx, ry, ..Default::default() };
        e.step(&p, &f(0.0, 0.0), t0);
        // Stick todo para a direita: vira 90° (900 px) em 0,1 s.
        let mut sum = 0;
        for i in 1..=10 {
            sum += moved(e.step(&p, &f(1.0, 0.0), at(i * 20, t0)));
        }
        assert_eq!(sum, 900);
        // Girar o stick para baixo (mais 90°) gira a câmera junto.
        assert_eq!(moved(e.step(&p, &f(0.0, -1.0), at(220, t0))), 900);
        // Soltar não mexe; o próximo flick para a esquerda vira -90°.
        assert_eq!(moved(e.step(&p, &f(0.0, 0.0), at(240, t0))), 0);
        let mut sum = 0;
        for i in 13..=20 {
            sum += moved(e.step(&p, &f(-1.0, 0.0), at(i * 20, t0)));
        }
        assert_eq!(sum, -900);
        assert_eq!(Profile::parse(&p.to_text()).unwrap().layers[0].rs, Some(Stick::Flick(3600.0)));
    }

    #[test]
    fn touchpad_regions_and_stick() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        one(&mut p.layers[0], TPUR, When::Press, Out::Key(0x51, 0));
        one(&mut p.layers[0], LSU, When::Press, Out::Key(0x57, 0));
        let mut e = Engine::default();
        let f = |a: Option<(f32, f32)>| Input { touch: [a, None], ..Default::default() };
        // Dedo no quarto de cima à direita: segura a tecla até soltar.
        e.step(&p, &f(None), t0);
        assert!(e.step(&p, &f(Some((0.8, 0.2))), at(10, t0)).contains(&Act::Down(Out::Key(0x51, 0))));
        assert!(e.step(&p, &f(None), at(20, t0)).contains(&Act::Up(Out::Key(0x51, 0))));
        // Touchpad como stick esquerdo: subir o dedo é o stick para cima.
        p.layers[0].touch = Some(Motion::Stick(0));
        e.step(&p, &f(Some((0.3, 0.9))), at(30, t0));
        assert!(e.step(&p, &f(Some((0.3, 0.5))), at(40, t0)).contains(&Act::Down(Out::Key(0x57, 0))));
        assert!(e.step(&p, &f(None), at(50, t0)).contains(&Act::Up(Out::Key(0x57, 0))));
        assert_eq!(Profile::parse(&p.to_text()).unwrap().layers[0].touch, Some(Motion::Stick(0)));
        assert_eq!(touch_stick((0.5, 0.5), (1.0, 0.5)), (1.0, 0.0));
    }

    #[test]
    fn touchpad_gestures_are_buttons() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        one(&mut p.layers[0], TPR, When::Press, Out::Key(0x27, 0));
        one(&mut p.layers[0], TPTAP, When::Press, Out::Mouse(1));
        one(&mut p.layers[0], TPCR, When::Press, Out::Key(0x20, 0));
        let mut e = Engine::default();
        let f = |a: Option<(f32, f32)>, b: Buttons| Input { touch: [a, None], buttons: b, ..Default::default() };
        let mut run = |inp: Input, ms: u64| e.step(&p, &inp, at(ms, t0));
        run(f(None, 0), 0);
        // Deslizar para a direita: aperta e solta a seta.
        run(f(Some((0.2, 0.5)), 0), 10);
        run(f(Some((0.6, 0.5)), 0), 100);
        assert!(run(f(None, 0), 150).iter().any(|a| *a == Act::Down(Out::Key(0x27, 0))));
        assert!(run(f(None, 0), 160).iter().any(|a| *a == Act::Up(Out::Key(0x27, 0))));
        // Tocar sem mexer: clique esquerdo.
        run(f(Some((0.5, 0.5)), 0), 200);
        assert!(run(f(None, 0), 260).iter().any(|a| *a == Act::Down(Out::Mouse(1))));
        run(f(None, 0), 270);
        // Clique na metade direita: espaço, sem contar como toque.
        run(f(Some((0.8, 0.5)), 0), 300);
        assert!(run(f(Some((0.8, 0.5)), pads::PAD), 320).iter().any(|a| *a == Act::Down(Out::Key(0x20, 0))));
        assert!(run(f(None, 0), 340).iter().all(|a| *a != Act::Down(Out::Mouse(1))));
        assert_eq!(Profile::parse(&p.to_text()).unwrap().layers[0].binds.len(), 3);
    }

    #[test]
    fn touchpad_moves_and_scrolls() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        p.layers[0].touch = Some(Motion::Mouse(1000.0));
        let mut e = Engine::default();
        let f = |a: Option<(f32, f32)>, b: Option<(f32, f32)>| Input { touch: [a, b], ..Default::default() };
        // Encostar o dedo não move; arrastar 1% da largura move 10 px.
        assert!(e.step(&p, &f(Some((0.5, 0.5)), None), at(0, t0)).is_empty());
        assert!(matches!(e.step(&p, &f(Some((0.51, 0.5)), None), at(10, t0))[..], [Act::Move(9 | 10, 0)]));
        // Dois dedos subindo 10% da altura: a página desce (roda negativa).
        e.step(&p, &f(Some((0.4, 0.5)), Some((0.6, 0.5))), at(20, t0));
        assert!(matches!(e.step(&p, &f(Some((0.4, 0.4)), Some((0.6, 0.4))), at(30, t0))[..], [Act::Wheel(-181..=-179)]));
        assert_eq!(Profile::parse(&p.to_text()).unwrap().layers[0].touch, Some(Motion::Mouse(1000.0)));
    }

    #[test]
    fn player_space_follows_gravity() {
        let o = GyroOpt { player: true, ..Default::default() };
        // Deitado (gravidade no y): igual ao local.
        assert_eq!(gyro_axes(o, [5.0, 30.0, 0.0], [0.0, 1.0, 0.0]), (30.0, 5.0));
        // Em pé, com a face para quem joga: virar em volta da vertical é o giro do eixo z.
        let (yaw, _) = gyro_axes(o, [0.0, 0.0, 40.0], [0.0, 0.0, 1.0]);
        assert!((yaw - 40.0).abs() < 1e-3);
        let opt = GyroOpt { player: true, inv_y: true, smooth: true, inv_x: false };
        let mut p = Profile::base();
        p.gyro_opt = opt;
        assert_eq!(Profile::parse(&p.to_text()).unwrap().gyro_opt, opt);
    }

    #[test]
    fn link_only_http() {
        assert!(link_ok("https://example.com/a?b=1"));
        assert!(link_ok("HTTP://x.org"));
        for bad in ["", "https://", "file:///C:/x.exe", "C:/Windows/notepad.exe", "https://a b", "javascript:alert(1)", "https://a\"b"] {
            assert!(!link_ok(bad), "{bad}");
        }
    }

    fn at(ms: u64, t0: Instant) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    fn btn(b: Buttons) -> Input {
        Input { buttons: b, ..Default::default() }
    }

    fn one(layer: &mut Layer, button: Buttons, when: When, out: Out) {
        layer.binds.push(Bind { button, with: 0, when, out });
    }

    const K1: Out = Out::Key(0x41, 0);
    const K2: Out = Out::Key(0x42, 0);

    /// Motor ligado com nada apertado.
    fn engine(p: &Profile, t0: Instant) -> Engine {
        let mut e = Engine::default();
        assert!(e.step(p, &btn(0), t0).is_empty());
        e
    }

    #[test]
    fn press_and_release() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        one(&mut p.layers[0], pads::A, When::Press, K1);
        let mut e = engine(&p, t0);
        assert_eq!(e.step(&p, &btn(pads::A), at(10, t0)), vec![Act::Down(K1)]);
        assert_eq!(e.step(&p, &btn(0), at(20, t0)), vec![Act::Up(K1)]);
        assert!(e.idle());
    }

    #[test]
    fn latch_holds_until_the_next_press() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        one(&mut p.layers[0], pads::A, When::Latch, K1);
        let mut e = engine(&p, t0);
        assert_eq!(e.step(&p, &btn(pads::A), at(10, t0)), vec![Act::Down(K1)]);
        assert!(e.step(&p, &btn(0), at(20, t0)).is_empty());
        assert!(e.idle(), "o que o Alternar prende não segura a troca de perfil");
        assert_eq!(e.release_all(), vec![Act::Up(K1)]);
        // Do zero: segundo aperto solta.
        let mut e = engine(&p, t0);
        e.step(&p, &btn(pads::A), at(10, t0));
        e.step(&p, &btn(0), at(20, t0));
        assert_eq!(e.step(&p, &btn(pads::A), at(30, t0)), vec![Act::Up(K1)]);
        assert!(e.step(&p, &btn(0), at(40, t0)).is_empty());
    }

    #[test]
    fn turbo_repeats_and_stops_under_anti_cheat() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        one(&mut p.layers[0], pads::A, When::Press, K1);
        // 5x: 20 por segundo, meio período de 25 ms, depois de 100 ms segurando.
        p.turbo.push(Turbo { button: pads::A, speed: 5, delay: 100, on: true });
        let mut e = engine(&p, t0);
        assert_eq!(e.step(&p, &btn(pads::A), at(0, t0)), vec![Act::Down(K1)]);
        assert!(e.step(&p, &btn(pads::A), at(50, t0)).is_empty(), "ainda na espera");
        assert_eq!(e.step(&p, &btn(pads::A), at(100, t0)), vec![Act::Up(K1)]);
        assert_eq!(e.step(&p, &btn(pads::A), at(125, t0)), vec![Act::Down(K1)]);
        assert_eq!(e.step(&p, &btn(0), at(130, t0)), vec![Act::Up(K1)]);
        assert!(e.idle());
        // Solto no meio do "solto": nada mais sai.
        let mut e = engine(&p, t0);
        e.step(&p, &btn(pads::A), at(0, t0));
        e.step(&p, &btn(pads::A), at(100, t0));
        assert!(e.step(&p, &btn(0), at(110, t0)).is_empty());
        // Desligado: botão comum.
        p.turbo[0].on = false;
        let mut e = engine(&p, t0);
        e.step(&p, &btn(pads::A), at(0, t0));
        assert!(e.step(&p, &btn(pads::A), at(300, t0)).is_empty());
        p.turbo[0].on = true;
        // Com anti-cheat vira Pressionar.
        let mut e = engine(&p, t0);
        e.safe = true;
        assert_eq!(e.step(&p, &btn(pads::A), at(0, t0)), vec![Act::Down(K1)]);
        assert!(e.step(&p, &btn(pads::A), at(100, t0)).is_empty());
        assert_eq!(e.step(&p, &btn(0), at(110, t0)), vec![Act::Up(K1)]);
    }

    #[test]
    fn turbo_shortcut_toggles_the_next_button_and_conflicts_are_found() {
        let t0 = Instant::now();
        let mut p = Profile::desktop();
        let mut e = engine(&p, t0);
        let key = pads::RS | RT;
        let rt = Input { buttons: pads::RS, rt: 1.0, ..Default::default() };
        e.step(&p, &rt, at(0, t0));
        let acts = e.step(&p, &Input { buttons: pads::RS | pads::A, rt: 1.0, ..Default::default() }, at(10, t0));
        assert!(acts.contains(&Act::Turbo(pads::A)));
        assert!(!acts.contains(&Act::Down(Out::Mouse(1))), "o A não clica quando liga o turbo");
        assert_eq!(p.turbo_key, key);
        // Conflitos: ligação "junto com" que usa o atalho, e a parada de emergência.
        assert_eq!(combo_conflict(&p, key, true), None);
        p.layers[0].binds.push(Bind { button: RT, with: pads::RS, when: When::Press, out: K1 });
        assert!(combo_conflict(&p, key, true).is_some());
        assert!(combo_conflict(&p, pads::A | key, false).is_some(), "atalho do turbo mais um botão");
        assert!(combo_conflict(&p, pads::GUIDE | pads::START, true).is_some());
        assert_eq!(combo_conflict(&p, pads::A | pads::B, false), None);
    }

    #[test]
    fn templates_survive_the_file() {
        for i in 0..TEMPLATES.len() {
            let mut p = Profile::template(i);
            p.name = "x".into();
            assert!(!p.layers[0].binds.is_empty());
            assert_eq!(Profile::parse(&p.to_text()), Some(p));
        }
        for side in 0..3 {
            let mut p = Profile::accessible(side, true, 800.0);
            p.name = "x".into();
            assert!(p.layers[0].binds.iter().any(|b| b.when == When::Latch));
            assert_eq!(Profile::parse(&p.to_text()), Some(p));
        }
    }

    #[test]
    fn held_at_start_does_nothing() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        one(&mut p.layers[0], pads::A, When::Press, K1);
        let mut e = Engine::default();
        assert!(e.step(&p, &btn(pads::A), t0).is_empty());
        assert!(e.step(&p, &btn(0), at(10, t0)).is_empty());
        assert_eq!(e.step(&p, &btn(pads::A), at(20, t0)), vec![Act::Down(K1)]);
    }

    #[test]
    fn tap_hold_and_double() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        one(&mut p.layers[0], pads::A, When::Tap, K1);
        one(&mut p.layers[0], pads::A, When::Hold, K2);
        let mut e = engine(&p, t0);
        // Toque.
        assert!(e.step(&p, &btn(pads::A), at(10, t0)).is_empty());
        assert_eq!(e.step(&p, &btn(0), at(100, t0)), vec![Act::Down(K1), Act::Up(K1)]);
        // Segurar.
        assert!(e.step(&p, &btn(pads::A), at(1000, t0)).is_empty());
        assert_eq!(e.step(&p, &btn(pads::A), at(1500, t0)), vec![Act::Down(K2)]);
        assert_eq!(e.step(&p, &btn(0), at(1600, t0)), vec![Act::Up(K2)]);
        // Duplo: o toque espera a janela, e o segundo aperto dispara o duplo no lugar.
        one(&mut p.layers[0], pads::A, When::Double, Out::Mouse(1));
        e.step(&p, &btn(pads::A), at(3000, t0));
        assert!(e.step(&p, &btn(0), at(3050, t0)).is_empty());
        assert_eq!(e.step(&p, &btn(pads::A), at(3200, t0)), vec![Act::Down(Out::Mouse(1))]);
        assert_eq!(e.step(&p, &btn(0), at(3300, t0)), vec![Act::Up(Out::Mouse(1))]);
        // Toque só, sem segundo aperto: sai quando a janela acaba.
        e.step(&p, &btn(pads::A), at(5000, t0));
        assert!(e.step(&p, &btn(0), at(5050, t0)).is_empty());
        assert_eq!(e.step(&p, &btn(0), at(5400, t0)), vec![Act::Down(K1), Act::Up(K1)]);
        assert!(e.idle());
    }

    #[test]
    fn key_releases_in_the_layer_it_was_pressed() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        p.layers.push(Layer { name: "Fn".into(), parent: Some(0), ls: None, rs: None, binds: vec![], keys: vec![], gyro: None, touch: None });
        one(&mut p.layers[0], pads::LB, When::Press, Out::Layer(1));
        one(&mut p.layers[0], pads::A, When::Press, K1);
        one(&mut p.layers[1], pads::A, When::Press, K2);
        one(&mut p.layers[0], pads::B, When::Press, Out::Mouse(2));
        let mut e = engine(&p, t0);
        assert_eq!(e.step(&p, &btn(pads::A), at(10, t0)), vec![Act::Down(K1)]);
        // Camada muda com A apertado: soltar A solta a tecla da camada de antes.
        assert!(e.step(&p, &btn(pads::A | pads::LB), at(20, t0)).is_empty());
        assert_eq!(e.layer(), 1);
        assert_eq!(e.step(&p, &btn(pads::LB), at(30, t0)), vec![Act::Up(K1)]);
        assert_eq!(e.step(&p, &btn(pads::LB | pads::A), at(40, t0)), vec![Act::Down(K2)]);
        // Herança: B não está na camada 1 e vem da Base.
        assert_eq!(e.step(&p, &btn(pads::LB | pads::A | pads::B), at(50, t0)), vec![Act::Down(Out::Mouse(2))]);
        // Solta a camada antes dos botões: K2 e o clique soltam assim mesmo.
        assert!(e.step(&p, &btn(pads::A | pads::B), at(60, t0)).is_empty());
        assert_eq!(e.step(&p, &btn(0), at(70, t0)), vec![Act::Up(K2), Act::Up(Out::Mouse(2))]);
        assert!(e.idle());
    }

    #[test]
    fn held_layer_changes_stick_mode_at_once() {
        let t0 = Instant::now();
        let mut p = Profile::desktop();
        p.layers.push(Layer { name: "Fn".into(), parent: Some(0), ls: None, rs: Some(Stick::Mouse(1600.0)), binds: vec![], keys: vec![], gyro: None, touch: None });
        p.layers[0].binds.retain(|b| b.button != pads::LB);
        one(&mut p.layers[0], pads::LB, When::Press, Out::Layer(1));
        let mut e = engine(&p, t0);
        let i = |b: Buttons| Input { buttons: b, ry: 1.0, ..Default::default() };
        assert!(matches!(e.step(&p, &i(0), at(10, t0))[..], [Act::Wheel(_)]));
        // No mesmo relatório em que o LB desce, o stick direito já é mouse.
        assert!(matches!(e.step(&p, &i(pads::LB), at(20, t0))[..], [Act::Move(0, -16 | -15)]));
        // Também com a camada em "segurar" e um toque no mesmo botão: a camada entra no aperto, e o
        // toque sai se soltar rápido.
        p.layers[0].binds.retain(|b| b.button != pads::LB);
        one(&mut p.layers[0], pads::LB, When::Hold, Out::Layer(1));
        one(&mut p.layers[0], pads::LB, When::Tap, K1);
        let mut e = engine(&p, t0);
        assert!(matches!(e.step(&p, &i(pads::LB), at(10, t0))[..], [Act::Move(..)]));
        assert_eq!(e.layer(), 1);
        let acts = e.step(&p, &btn(0), at(100, t0));
        assert_eq!(acts, vec![Act::Down(K1), Act::Up(K1)]);
        assert_eq!(e.layer(), 0);
    }

    #[test]
    fn toggle_layer() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        p.layers.push(Layer { name: "Fn".into(), parent: Some(0), ls: None, rs: None, binds: vec![], keys: vec![], gyro: None, touch: None });
        one(&mut p.layers[0], pads::RB, When::Press, Out::Toggle(1));
        let mut e = engine(&p, t0);
        e.step(&p, &btn(pads::RB), at(10, t0));
        e.step(&p, &btn(0), at(20, t0));
        assert_eq!(e.layer(), 1);
        e.step(&p, &btn(pads::RB), at(30, t0));
        e.step(&p, &btn(0), at(40, t0));
        assert_eq!(e.layer(), 0);
    }

    #[test]
    fn chord_replaces_and_cancels_tap() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        one(&mut p.layers[0], pads::LB, When::Tap, K1);
        one(&mut p.layers[0], pads::A, When::Press, K2);
        p.layers[0].binds.push(Bind { button: pads::A, with: pads::LB, when: When::Press, out: Out::Mouse(1) });
        let mut e = engine(&p, t0);
        e.step(&p, &btn(pads::LB), at(10, t0));
        assert_eq!(e.step(&p, &btn(pads::LB | pads::A), at(20, t0)), vec![Act::Down(Out::Mouse(1))]);
        assert_eq!(e.step(&p, &btn(pads::LB), at(30, t0)), vec![Act::Up(Out::Mouse(1))]);
        // O toque do LB foi usado pela combinação.
        assert!(e.step(&p, &btn(0), at(40, t0)).is_empty());
        assert_eq!(e.step(&p, &btn(pads::A), at(50, t0)), vec![Act::Down(K2)]);
    }

    #[test]
    fn same_key_from_two_buttons_releases_with_the_last() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        one(&mut p.layers[0], pads::A, When::Press, K1);
        one(&mut p.layers[0], pads::B, When::Press, K1);
        let mut e = engine(&p, t0);
        assert_eq!(e.step(&p, &btn(pads::A), at(10, t0)), vec![Act::Down(K1)]);
        assert!(e.step(&p, &btn(pads::A | pads::B), at(20, t0)).is_empty());
        assert!(e.step(&p, &btn(pads::B), at(30, t0)).is_empty());
        assert_eq!(e.step(&p, &btn(0), at(40, t0)), vec![Act::Up(K1)]);
    }

    #[test]
    fn release_all_leaves_nothing_down() {
        let t0 = Instant::now();
        let p = Profile::desktop();
        let mut e = engine(&p, t0);
        let acts = e.step(&p, &btn(pads::A | pads::X | pads::UP), at(10, t0));
        assert_eq!(acts.len(), 3);
        let up = e.release_all();
        assert_eq!(up.len(), 3);
        assert!(up.iter().all(|a| matches!(a, Act::Up(_))));
        // Ainda apertados depois de soltar tudo: não disparam de novo.
        assert!(e.step(&p, &btn(pads::A | pads::X | pads::UP), at(20, t0)).is_empty());
        assert!(e.step(&p, &btn(0), at(30, t0)).is_empty());
    }

    #[test]
    fn trigger_and_stick_buttons_with_hysteresis() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        one(&mut p.layers[0], RT, When::Press, K1);
        one(&mut p.layers[0], LSR, When::Press, K2);
        let mut e = engine(&p, t0);
        let i = |rt: f32, lx: f32| Input { rt, lx, ..Default::default() };
        assert!(e.step(&p, &i(0.4, 0.0), at(10, t0)).is_empty());
        assert_eq!(e.step(&p, &i(0.6, 0.0), at(20, t0)), vec![Act::Down(K1)]);
        // Entre 0,4 e 0,5 continua apertado.
        assert!(e.step(&p, &i(0.47, 0.0), at(30, t0)).is_empty());
        assert_eq!(e.step(&p, &i(0.0, 0.9), at(40, t0)), vec![Act::Up(K1), Act::Down(K2)]);
    }

    #[test]
    fn stick_moves_mouse_and_scrolls() {
        let t0 = Instant::now();
        let p = Profile::desktop();
        let mut e = engine(&p, t0);
        let acts = e.step(&p, &Input { lx: 1.0, ry: 1.0, ..Default::default() }, at(10, t0));
        // 10 ms a 1600 px/s no fim = 16 px; roda a 12/s = 0,12 clique = 14 unidades.
        assert!(matches!(acts[..], [Act::Move(15 | 16, 0), Act::Wheel(14)]), "{acts:?}");
        // Dentro da zona morta, nada (só a sobra da conta anterior pode sair).
        let acts = e.step(&p, &Input { lx: 0.1, ..Default::default() }, at(20, t0));
        assert!(acts.iter().all(|a| matches!(a, Act::Move(0..=1, 0))), "{acts:?}");
    }

    #[test]
    fn deadzones() {
        let z = Zone { start: 0.1, end: 0.9 };
        assert_eq!(radial(0.05, 0.05, z), (0.0, 0.0));
        let (x, y) = radial(0.95, 0.0, z);
        assert!((x - 1.0).abs() < 1e-5 && y == 0.0);
        let (x, _) = radial(0.5, 0.0, z);
        assert!((x - 0.5).abs() < 1e-5);
        assert_eq!(linear(0.05, z), 0.0);
        assert_eq!(linear(1.0, z), 1.0);
        assert!((linear(0.5, z) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn quick_text_round_trip_and_abbreviation() {
        let mut p = Profile::base();
        p.texts.push(Snip { abbr: "obg".into(), text: "Obrigado pela partida!".into() });
        p.texts.push(Snip { abbr: String::new(), text: "gg wp".into() });
        p.layers[0].binds.push(Bind { button: pads::A, with: 0, when: When::Press, out: Out::Text(1) });
        let text = p.to_text();
        assert_eq!(Profile::parse(&text), Some(p.clone()), "{text}");
        let mut w = String::new();
        let mut keys = |list: &[u16], modded: bool| list.iter().map(|&k| typed_key(&mut w, k, modded, &p.texts)).last().flatten();
        // O, Shift, B, G e espaço: a abreviação vira o texto 0.
        assert_eq!(keys(&[0x4F, 0xA0, 0x42, 0x47, 0x20], false), Some(0));
        // Parte de outra palavra, com apagar no meio, ou com Ctrl.
        assert_eq!(keys(&[0x41, 0x4F, 0x42, 0x47, 0x20], false), None);
        assert_eq!(keys(&[0x4F, 0x42, 0x58, 0x08, 0x47, 0x0D], false), Some(0));
        assert_eq!(keys(&[0x4F, 0x42, 0x47], true), None);
        assert_eq!(keys(&[0x20], false), None);
        // Palavra longa demais não termina numa abreviação curta.
        let long: Vec<u16> = std::iter::repeat_n(0x41, 16).chain([0x4F, 0x42, 0x47, 0x09]).collect();
        assert_eq!(keys(&long, false), None);
    }

    #[test]
    fn keys_become_buttons_for_the_engine() {
        let t0 = Instant::now();
        let mut p = Profile::base();
        // Caps Lock: tocar é Esc, segurar é Ctrl; Caps + H é a seta esquerda.
        p.layers[0].keys.push(Bind { button: 0x14, with: 0, when: When::Tap, out: Out::Key(0x1B, 0) });
        p.layers[0].keys.push(Bind { button: 0x14, with: 0, when: When::Hold, out: Out::Key(0xA2, 0) });
        p.layers[0].keys.push(Bind { button: 0x48, with: 0x14, when: When::Press, out: Out::Key(0x25, 0) });
        let text = p.to_text();
        assert_eq!(Profile::parse(&text), Some(p.clone()), "{text}");
        let (kp, slots) = key_profile(&p).unwrap();
        assert_eq!(slots, vec![0x14, 0x48]);
        let mut e = Engine { prev: 0, ..Engine::default() };
        let caps = Input { buttons: 1, ..Default::default() };
        assert!(e.step(&kp, &caps, at(0, t0)).is_empty());
        // Caps + H: a seta, e o Caps não toca Esc ao soltar.
        assert_eq!(e.step(&kp, &Input { buttons: 3, ..Default::default() }, at(50, t0)), vec![Act::Down(Out::Key(0x25, 0))]);
        assert_eq!(e.step(&kp, &caps, at(80, t0)), vec![Act::Up(Out::Key(0x25, 0))]);
        assert!(e.step(&kp, &Input::default(), at(100, t0)).is_empty());
        // Toque: Esc. Segurar: Ctrl depois de 0,4 s.
        e.step(&kp, &caps, at(1000, t0));
        assert_eq!(e.step(&kp, &Input::default(), at(1100, t0)), vec![Act::Down(Out::Key(0x1B, 0)), Act::Up(Out::Key(0x1B, 0))]);
        e.step(&kp, &caps, at(2000, t0));
        assert_eq!(e.step(&kp, &caps, at(2450, t0)), vec![Act::Down(Out::Key(0xA2, 0))]);
        assert!(key_profile(&Profile::base()).is_none());
    }

    #[test]
    fn profile_text_round_trip_and_rejects_bad_input() {
        let mut p = Profile::desktop();
        p.games.push("jogo.exe".into());
        p.layers.push(Layer { name: "Fn".into(), parent: Some(0), ls: Some(Stick::Buttons), rs: None, binds: vec![], keys: vec![], gyro: None, touch: None });
        one(&mut p.layers[0], pads::LB, When::Hold, Out::Layer(1));
        p.layers[1].binds.push(Bind { button: pads::A, with: pads::LB | pads::RB, when: When::Double, out: Out::Key(0x43, CTRL | SHIFT) });
        let base = intern(&mut p.strings, "Base").unwrap();
        let prog = intern(&mut p.strings, "C:/Program Files/App Dir/app.exe").unwrap();
        for out in [Out::Action(0), Out::Action(ACTIONS.len() as u8 - 1), Out::Action(SOUND + 2), Out::Profile(base), Out::Run(prog)] {
            one(&mut p.layers[1], pads::B, When::Tap, out);
        }
        p.turbo.push(Turbo { button: pads::X, speed: 3, delay: 250, on: true });
        p.turbo.push(Turbo::new(pads::LB));
        p.turbo_key = pads::LB | LT;
        let k = Out::Key(0x41, 0);
        p.macros.push(Macro { name: "Combo 1".into(), mode: Mode::Held, steps: vec![Step::Down(k), Step::Wait(120), Step::Up(k), Step::Down(Out::Wheel(-2)), Step::Down(Out::Mouse(4)), Step::Up(Out::Mouse(4))] });
        p.macros.push(Macro { name: "Vazia".into(), mode: Mode::Toggle, steps: vec![] });
        one(&mut p.layers[0], pads::Y, When::Press, Out::Macro(1));
        let text = p.to_text();
        assert_eq!(Profile::parse(&text), Some(p), "{text}");
        for bad in [
            "",
            "prisma-profile 2\nname x\nlayer - B",
            "prisma-profile 1\nname x",
            "prisma-profile 1\nname x\nlayer 0 B",
            "prisma-profile 1\nname x\nlayer - B\nbind 0 A press key 0",
            "prisma-profile 1\nname x\nlayer - B\nbind 0 A press key 300",
            "prisma-profile 1\nname x\nlayer - B\nbind 1 A press key 65",
            "prisma-profile 1\nname x\nlayer - B\nbind 0 Z press key 65",
            "prisma-profile 1\nname x\nlayer - B\nbind 0 A+A press key 65",
            "prisma-profile 1\nname x\nlayer - B\nbind 0 A press key 65 hyper",
            "prisma-profile 1\nname x\nlayer - B\nbind 0 A press layer 3",
            "prisma-profile 1\nname x\nlayer - B\nzone ls 0.9 0.1",
            "prisma-profile 1\nname x\nlayer - B\nstick 0 ls mouse NaN",
            "prisma-profile 1\nname x\nlayer - B\nrun calc.exe",
            // Macro: ligação para macro que não existe, passo sem macro, roda soltando, espera longa demais, ação.
            "prisma-profile 1\nname x\nlayer - B\nbind 0 A press macro 0",
            "prisma-profile 1\nname x\nlayer - B\nmstep 0 wait 10",
            "prisma-profile 1\nname x\nmacro once M\nmstep 0 up wheel 1\nlayer - B",
            "prisma-profile 1\nname x\nmacro once M\nmstep 0 wait 60001\nlayer - B",
            "prisma-profile 1\nname x\nmacro once M\nmstep 0 down action 1\nlayer - B",
            "prisma-profile 1\nname x\nlayer - B\nkbind 0 20+20 press key 65",
            "prisma-profile 1\nname x\nlayer - B\nkbind 0 300 press key 65",
            "prisma-profile 1\nname x\ntext a Oi\nlayer - B",
            "prisma-profile 1\nname x\ntext Obg Oi\nlayer - B",
            "prisma-profile 1\nname x\ntext ob-g Oi\nlayer - B",
            "prisma-profile 1\nname x\ntext obg Oi\ntext obg Tchau\nlayer - B",
            "prisma-profile 1\nname x\ntext obg Oi\nlayer - B\nbind 0 A press text 1",
            "prisma-profile 1\nname x\nlayer - B\nbind 0 A press action 54",
            "prisma-profile 1\nname x\nlayer - B\nbind 0 A press action 112",
            "prisma-profile 1\nname x\nlayer - B\nturbo A 1 500 on",
            "prisma-profile 1\nname x\nlayer - B\nturbo A 2 500 maybe",
            "prisma-profile 1\nname x\nlayer - B\nturbo A 2 9000 on",
            "prisma-profile 1\nname x\nlayer - B\nturbokey RS+Z",
        ] {
            assert_eq!(Profile::parse(bad), None, "{bad}");
        }
    }
}
