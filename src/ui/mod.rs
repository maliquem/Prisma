//! Interface imediata desenhada só quando algo muda: cada redesenho descreve a tela
//! e os controles respondem ao mouse nesse mesmo passe.

pub mod a11y;
pub mod edit;
pub mod gfx;
pub mod glyph;
pub mod icons;
pub mod layered;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::config::Hotkey;
use gfx::*;

/// Faixa da barra de rolagem, na margem à direita da lista.
pub const BAR_GAP: f32 = 2.0;
pub const BAR_W: f32 = 8.0;

/// Deixa um campo pronto para digitar.
pub fn focus_on(s: &mut UiState, key: &str) {
    s.focus = hash_id(key);
}

fn hash_id(key: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    h.finish() | 1
}

struct Popup {
    id: u64,
    rect: Rect,
    options: Vec<String>,
    selected: usize,
    /// Opções de menu que aparecem apagadas e não respondem.
    off: Vec<bool>,
    /// Lista mais alta que a caixa: quanto já desceu.
    top: f32,
    /// Lista de botões do controle: o ícone de cada opção (0 é sem ícone).
    glyphs: Vec<crate::sys::pads::Buttons>,
    style: glyph::Style,
}

/// Altura do ícone de botão do controle dentro de uma lista.
const GLYPH_H: f32 = 16.0;

/// Altura de uma opção da lista, e quantas cabem antes de rolar.
const ROW: f32 = 36.0;
const ROWS_MAX: f32 = 10.0;

/// Altura da caixa da lista: no máximo `ROWS_MAX` opções e o que cabe na janela.
fn popup_h(n: usize, view_h: f32) -> f32 {
    let h = (n as f32).min(ROWS_MAX) * ROW + 8.0;
    if view_h > 0.0 { h.min(view_h - 16.0) } else { h }
}

/// Pergunta no meio da tela: nada atrás dela responde até escolher.
struct Ask {
    id: u64,
    title: String,
    body: String,
    /// Só avisa: mostra OK em vez de Sim e Não.
    notice: bool,
    /// Caixa de marcar abaixo do texto: rótulo e estado.
    check: Option<(String, bool)>,
}

/// Controle clicável deste desenho, para o controle (gamepad) andar entre eles. `r` já está em
/// coordenadas da janela; `clip` é a parte visível da lista dele; `area` é a lista (0: fora de lista).
#[derive(Clone, Copy, Debug)]
struct Target {
    r: Rect,
    clip: Rect,
    area: u64,
    field: bool,
    /// Linha de uma lista de `begin_rows`: para o Tab a lista toda é uma parada só.
    rows: bool,
}

/// Altura da faixa de dicas do controle, embaixo da página.
pub const PAD_BAR: f32 = 40.0;

/// Área das opções de uma lista aberta (as ids de `hash_id` são ímpares).
const POPUP: u64 = 2;

impl Target {
    fn center(&self) -> (f32, f32) {
        (self.r.x + self.r.w / 2.0, self.r.y + self.r.h / 2.0)
    }

    fn shown(&self) -> bool {
        let (x, y) = self.center();
        self.clip.contains(x, y)
    }
}

/// O alvo mais perto de `from` na direção `dir` (0 esquerda, 1 cima, 2 direita, 3 baixo): andar de
/// lado pesa o dobro.
fn pick(ts: &[Target], from: (f32, f32), dir: u8, ok: impl Fn(&Target) -> bool) -> Option<usize> {
    ts.iter()
        .enumerate()
        .filter(|(_, t)| ok(t))
        .filter_map(|(i, t)| {
            let (dx, dy) = (t.center().0 - from.0, t.center().1 - from.1);
            let (along, side) = match dir {
                0 => (-dx, dy),
                1 => (-dy, dx),
                2 => (dx, dy),
                _ => (dy, dx),
            };
            (along > 4.0).then_some((i, along + side.abs() * 2.0))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

struct Bar {
    r: Rect,
    y: f32,
    h: f32,
    id: u64,
}

#[derive(Default)]
pub struct UiState {
    /// Altura da barra de título desenhada pelo Prisma: a página começa abaixo dela, e o mouse
    /// (`mx`, `my`) vem nas coordenadas da página.
    pub top: f32,
    pub mx: f32,
    pub my: f32,
    pub down: bool,
    pub pressed: bool,
    pub released: bool,
    pub right_released: bool,
    /// Botão do meio (roda) solto neste quadro.
    pub middle_released: bool,
    pub wheel: f32,
    active: u64,
    popup: Option<Popup>,
    /// A lista acabou de abrir: o clique que a abriu não pode fechá-la.
    popup_new: bool,
    pub view_h: f32,
    pub view_w: f32,
    ask: Option<Ask>,
    ask_new: bool,
    answer: Option<u64>,
    /// Estado da caixa da pergunta que acabou de fechar.
    ask_mark: Option<(u64, bool)>,
    grab: f32,
    popup_pick: Option<(u64, usize)>,
    pub capturing: u64,
    /// Último campo de tecla que gravou, para reabrir se o atalho não serviu.
    pub last_hotkey: u64,
    pub key_event: Option<Hotkey>,
    pub key_cancel: bool,
    scroll: HashMap<u64, f32>,
    scroll_max: HashMap<u64, f32>,
    stick: u64,
    pub needs_redraw: bool,
    icons: HashMap<String, Option<windows::Win32::Graphics::Direct2D::ID2D1Bitmap>>,
    /// Campo de texto com o foco, e o que foi digitado desde o último desenho.
    pub focus: u64,
    pub typed: String,
    pub backspace: u32,
    /// Ctrl+Backspace: apaga a última palavra.
    pub kill_word: u32,
    /// Setas, Enter, Esc e Ctrl+C com um campo de texto em foco: (tecla, Ctrl apertado).
    pub nav: Vec<(u32, u8)>,
    /// Navegação pelo controle: anel no controle sob o cursor e faixa de dicas embaixo.
    pub pad: bool,
    /// LT/RT: aba anterior (-1) ou seguinte (1), para a página que tiver abas.
    pub pad_tab: Option<i8>,
    /// Navegação pelo teclado (Tab, setas): o mesmo anel do controle, sem a faixa de dicas.
    pub keys: bool,
    /// Controles clicáveis do desenho em curso e do último.
    targets: Vec<Target>,
    last: Vec<Target>,
    /// Nome e papel de cada alvo, na mesma ordem, para o Narrador (vazios até um leitor pedir).
    names: Vec<a11y::Node>,
    last_names: Vec<a11y::Node>,
}

impl UiState {
    /// Fotos de teste: começa a área rolável `key` já descida.
    #[cfg(debug_assertions)]
    pub fn set_scroll(&mut self, key: &str, v: f32) {
        self.scroll.insert(hash_id(key), v);
    }

    /// Há lista ou pergunta na frente: janelas filhas do Windows ficariam por cima delas.
    pub fn covered(&self) -> bool {
        self.popup.is_some() || self.ask.is_some()
    }

    /// Fecha a pergunta aberta como se a resposta fosse não.
    pub fn close_ask(&mut self) -> bool {
        self.ask.take().is_some()
    }

    /// Controle: leva o cursor ao próximo controle na direção `dir` (0 esquerda, 1 cima, 2 direita,
    /// 3 baixo), rolando a lista se ele estiver fora da vista; sem direção, ao controle mais perto,
    /// se o cursor não estiver sobre um. Devolve onde pôr o cursor.
    pub fn pad_move(&mut self, dir: Option<u8>) -> Option<(f32, f32)> {
        let (mx, my) = (self.mx, self.my);
        let ts = &self.last;
        let cur = ts.iter().rposition(|t| t.shown() && t.r.contains(mx, my));
        let Some(dir) = dir else {
            let d = |t: &Target| (t.center().0 - mx).hypot(t.center().1 - my);
            return if cur.is_some() { None } else { ts.iter().filter(|t| t.shown()).min_by(|a, b| d(a).total_cmp(&d(b))).map(Target::center) };
        };
        let from = cur.map_or((mx, my), |i| ts[i].center());
        let area = cur.map_or(0, |i| ts[i].area);
        if let Some(c) = cur.filter(|_| area != 0) {
            if let Some(i) = pick(ts, from, dir, |t| t.area == area) {
                let t = ts[i];
                return Some(self.reveal(t));
            }
            // Lista que só desenha as linhas à vista: rola um pedaço e as próximas aparecem.
            if dir & 1 == 1 {
                let h = ts[c].clip.h * 0.6;
                if self.scroll_by(area, if dir == 1 { -h } else { h }) != 0.0 {
                    return None;
                }
            }
        }
        let ts = &self.last;
        pick(ts, from, dir, |t| t.shown()).map(|i| ts[i].center())
    }

    /// Rola a lista do alvo `t` até ele caber na vista e devolve onde pôr o cursor.
    fn reveal(&mut self, t: Target) -> (f32, f32) {
        let want = if t.r.y < t.clip.y {
            t.r.y - t.clip.y - 8.0
        } else if t.r.bottom() > t.clip.bottom() {
            t.r.bottom() - t.clip.bottom() + 8.0
        } else {
            0.0
        };
        let moved = if want != 0.0 && t.area != 0 { self.scroll_by(t.area, want) } else { 0.0 };
        let (x, y) = t.center();
        (x, (y - moved).clamp(t.clip.y + 2.0, t.clip.bottom() - 2.0))
    }

    /// Teclado: Tab leva ao próximo controle na ordem do desenho (Shift+Tab, ao anterior), dando a
    /// volta; os de uma área rolável fora da vista entram também e ela rola até eles. Uma lista de
    /// `begin_rows` é uma parada só: Tab entra na primeira linha à vista e sai da lista inteira, e
    /// as setas andam nela. Devolve onde pôr o cursor.
    pub fn tab_move(&mut self, back: bool) -> Option<(f32, f32)> {
        let (mx, my) = (self.mx, self.my);
        let ts = &self.last;
        let same_list = |a: &Target, b: &Target| a.rows && b.rows && a.area == b.area;
        let mut stops: Vec<usize> = Vec::new();
        for (i, t) in ts.iter().enumerate() {
            let stop = if t.rows { t.shown() && !stops.iter().any(|&j| same_list(&ts[j], t)) } else { t.shown() || t.area != 0 };
            if stop {
                stops.push(i);
            }
        }
        let n = stops.len();
        if n == 0 {
            return None;
        }
        let cur = ts.iter().rposition(|t| t.shown() && t.r.contains(mx, my)).and_then(|c| stops.iter().position(|&j| j == c || same_list(&ts[j], &ts[c])));
        let k = match cur {
            Some(c) if back => (c + n - 1) % n,
            Some(c) => (c + 1) % n,
            None if back => n - 1,
            None => 0,
        };
        let t = ts[stops[k]];
        Some(self.reveal(t))
    }

    /// Rola a lista `area` em `d` e devolve quanto andou de fato.
    fn scroll_by(&mut self, area: u64, d: f32) -> f32 {
        self.needs_redraw = true;
        if area == POPUP {
            let Some(p) = &mut self.popup else { return 0.0 };
            let max = (p.options.len() as f32 * ROW + 8.0 - p.rect.h).max(0.0);
            let v = p.top;
            p.top = (v + d).clamp(0.0, max);
            return p.top - v;
        }
        let max = self.scroll_max.get(&area).copied().unwrap_or(0.0);
        let v = self.scroll.get(&area).copied().unwrap_or(0.0);
        let to = (v + d).clamp(0.0, max);
        self.scroll.insert(area, to);
        to - v
    }

    /// O cursor está sobre um campo de texto.
    pub fn pad_field(&self) -> bool {
        self.last.iter().rev().find(|t| t.shown() && t.r.contains(self.mx, self.my)).is_some_and(|t| t.field)
    }

    /// Botão de voltar do controle: fecha a lista, a pergunta ou o campo de texto aberto.
    pub fn cancel(&mut self) -> bool {
        if self.popup.take().is_some() {
            self.popup_new = false;
        } else if !self.close_ask() {
            if self.focus == 0 {
                return false;
            }
            self.focus = 0;
        }
        self.needs_redraw = true;
        true
    }

    fn end_frame(&mut self) {
        self.pressed = false;
        self.released = false;
        self.right_released = false;
        self.middle_released = false;
        self.wheel = 0.0;
        self.key_event = None;
        self.key_cancel = false;
        self.nav.clear();
        self.pad_tab = None;
        self.last = std::mem::take(&mut self.targets);
        self.last_names = std::mem::take(&mut self.names);
    }

    /// Narrador: passa os controles à vista do último desenho e qual tem o foco (o do anel).
    pub fn publish(&self, hwnd: windows::Win32::Foundation::HWND, scale: f32) {
        if !a11y::on() {
            return;
        }
        let mut focus = None;
        let mut items = Vec::new();
        for (t, n) in self.last.iter().zip(&self.last_names).filter(|(t, _)| t.shown()) {
            if (self.pad || self.keys) && t.r.contains(self.mx, self.my) {
                focus = Some(items.len());
            }
            items.push((rect(t.r.x, t.r.y + self.top, t.r.w, t.r.h), n.clone()));
        }
        a11y::publish(hwnd, scale, items, focus);
    }
}

pub struct Ui<'a> {
    pub c: &'a Canvas,
    pub s: &'a mut UiState,
    dy: f32,
    blocked: bool,
    clip: Option<Rect>,
    /// Lista rolável em curso (0: nenhuma), para os alvos do controle.
    area: u64,
    /// A lista em curso é de `begin_rows`.
    rows: bool,
    bar: Option<Bar>,
    tip: Option<(Rect, String)>,
    /// Narrador: dica que chegou antes do alvo dela, com o lugar dele.
    tip_name: Option<(Rect, String)>,
}

impl<'a> Ui<'a> {
    pub fn begin(c: &'a Canvas, s: &'a mut UiState) -> Self {
        let blocked = s.popup.is_some() || s.ask.is_some();
        // Teste: `PRISMA_TEST_MOUSE=x,y` deixa o mouse parado ali, para a foto mostrar a dica.
        #[cfg(debug_assertions)]
        if let Some((x, y)) = std::env::var("PRISMA_TEST_MOUSE").ok().and_then(|v| v.split_once(',').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))) {
            (s.mx, s.my) = (x, y);
        }
        // Teste: `PRISMA_TEST_PAD=1` mostra o anel e as dicas do controle.
        #[cfg(debug_assertions)]
        if std::env::var_os("PRISMA_TEST_PAD").is_some() {
            s.pad = true;
        }
        c.set_top(s.top);
        c.translate(0.0, 0.0);
        Self { c, s, dy: 0.0, blocked, clip: None, area: 0, rows: false, bar: None, tip: None, tip_name: None }
    }

    fn hover(&self, r: Rect) -> bool {
        if self.blocked {
            return false;
        }
        if let Some(c) = self.clip {
            if !c.contains(self.s.mx, self.s.my) {
                return false;
            }
        }
        r.contains(self.s.mx, self.s.my + self.dy)
    }

    /// Guarda um alvo para o controle andar até ele.
    fn target(&mut self, r: Rect, clip: Option<Rect>, area: u64) {
        let view = rect(0.0, -self.s.top, self.s.view_w, self.s.view_h + self.s.top);
        self.s.targets.push(Target { r, clip: clip.unwrap_or(view), area, field: false, rows: self.rows });
        let mut node = a11y::Node::default();
        // Ícone com dica desenhada antes do clique: a dica é o nome.
        if let Some((tr, name)) = self.tip_name.take() {
            if tr == r {
                node.name = name;
            }
        }
        self.s.names.push(node);
    }

    /// Narrador: nome e papel do alvo `n` (o índice de antes de registrá-lo). Nada se o alvo não
    /// entrou (controle bloqueado) ou se nenhum leitor de tela pediu.
    fn named(&mut self, n: usize, name: &str, role: a11y::Role, on: Option<bool>) {
        if !a11y::on() {
            return;
        }
        if let Some(x) = self.s.names.get_mut(n) {
            *x = a11y::Node { name: crate::i18n::tr(name).into_owned(), role, on };
        }
    }

    /// `hit` com nome, papel e estado (`on`: escolhido ou marcado) para o Narrador.
    pub fn hit_as(&mut self, key: &str, r: Rect, name: &str, role: a11y::Role, on: Option<bool>) -> bool {
        let n = self.s.targets.len();
        let clicked = self.hit(key, r);
        self.named(n, name, role, on);
        clicked
    }

    /// Devolve true no clique que começou e terminou sobre este controle.
    fn clickable(&mut self, id: u64, r: Rect) -> bool {
        if !self.blocked {
            self.target(rect(r.x, r.y - self.dy, r.w, r.h), self.clip, self.area);
        }
        let hover = self.hover(r);
        if hover && self.s.pressed {
            self.s.active = id;
        }
        let clicked = hover && self.s.released && self.s.active == id;
        if self.s.released && self.s.active == id {
            self.s.active = 0;
        }
        clicked
    }

    pub fn is_hover(&self, r: Rect) -> bool {
        self.hover(r)
    }

    /// Área rolável. O conteúdo desenhado depois disso usa coordenadas sem rolagem.
    pub fn begin_scroll(&mut self, key: &str, r: Rect, content_h: f32) {
        let id = hash_id(key);
        let max = (content_h - r.h).max(0.0);
        let mut v = self.s.scroll.get(&id).copied().unwrap_or(0.0);
        if self.s.stick == id {
            v = max;
            self.s.stick = 0;
        }
        self.s.scroll_max.insert(id, max);
        if !self.blocked && r.contains(self.s.mx, self.s.my) && self.s.wheel != 0.0 {
            v -= self.s.wheel * 72.0;
            self.s.needs_redraw = true;
        }
        self.bar = None;
        if max > 0.0 {
            let bar_id = id ^ 2;
            let h = (r.h * r.h / content_h).max(32.0);
            let span = r.h - h;
            let top = r.y + span * (v.clamp(0.0, max) / max);
            let over = self.s.mx >= r.right() && self.s.mx <= r.right() + BAR_GAP + BAR_W && self.s.my >= r.y && self.s.my <= r.bottom();
            if !self.blocked && over && self.s.pressed {
                self.s.active = bar_id;
                // Clicar fora do trilho claro leva a barra até ali.
                let on_thumb = self.s.my >= top && self.s.my <= top + h;
                self.s.grab = if on_thumb { self.s.my - top } else { h / 2.0 };
            }
            if self.s.active == bar_id {
                v = (self.s.my - self.s.grab - r.y) / span * max;
                self.s.needs_redraw = true;
                if self.s.released || !self.s.down {
                    self.s.active = 0;
                }
            }
            v = v.clamp(0.0, max);
            self.bar = Some(Bar { r, y: r.y + span * (v / max), h, id: bar_id });
        }
        v = v.clamp(0.0, max);
        self.s.scroll.insert(id, v);
        self.dy = v;
        self.clip = Some(r);
        self.area = id;
        self.c.push_clip(r);
        self.c.translate(0.0, -v);
    }

    /// `begin_scroll` de uma lista de linhas, que pode ter milhares: para o Tab ela é uma parada só.
    pub fn begin_rows(&mut self, key: &str, r: Rect, content_h: f32) {
        self.begin_scroll(key, r, content_h);
        self.rows = true;
    }

    pub fn end_scroll(&mut self) {
        self.c.translate(0.0, 0.0);
        self.c.pop_clip();
        self.dy = 0.0;
        self.clip = None;
        self.area = 0;
        self.rows = false;
        if let Some(b) = self.bar.take() {
            let hot = self.s.active == b.id;
            let near = !self.blocked
                && self.s.mx >= b.r.right()
                && self.s.mx <= b.r.right() + BAR_GAP + BAR_W
                && self.s.my >= b.r.y
                && self.s.my <= b.r.bottom();
            let color = if hot { ACCENT } else if near { TEXT2 } else { BORDER2 };
            let w = if hot || near { 5.0 } else { 3.0 };
            self.c.round(rect(b.r.right() + BAR_GAP, b.y, w, b.h), w / 2.0, color);
        }
    }

    /// Se a lista está no fim, continua no fim quando crescer. Rolar para cima solta.
    pub fn stick_bottom(&mut self, key: &str) {
        let id = hash_id(key);
        let v = self.s.scroll.get(&id).copied().unwrap_or(0.0);
        if v >= self.s.scroll_max.get(&id).copied().unwrap_or(0.0) - 2.0 {
            self.s.stick = id;
        }
    }

    /// Volta uma lista ao topo, ao trocar de pasta ou de aba.
    pub fn reset_scroll(&mut self, key: &str) {
        self.s.scroll.remove(&hash_id(key));
    }

    /// Leva uma lista direto a um ponto do conteúdo.
    pub fn set_scroll(&mut self, key: &str, y: f32) {
        self.s.scroll.insert(hash_id(key), y);
        self.s.needs_redraw = true;
    }

    /// Dica que aparece com o mouse em cima, por cima de tudo.
    pub fn tip(&mut self, r: Rect, text: &str) {
        if a11y::on() {
            // A dica dá nome ao ícone clicável do mesmo lugar, venha antes ou depois do clique.
            let at = rect(r.x, r.y - self.dy, r.w, r.h);
            let name = crate::i18n::tr(text).into_owned();
            match (self.s.targets.last(), self.s.names.last_mut()) {
                (Some(t), Some(n)) if t.r == at && n.name.is_empty() => n.name = name,
                _ => self.tip_name = Some((at, name)),
            }
        }
        if self.hover(r) {
            // A dica é desenhada fora da lista: guarda onde a linha está na tela, já com a rolagem.
            self.tip = Some((rect(r.x, r.y - self.dy, r.w, r.h), text.to_string()));
        }
    }

    /// `tip` que abre à direita do controle, no meio da altura dele (ícones da barra lateral compacta).
    pub fn tip_right(&mut self, r: Rect, text: &str) {
        self.tip(r, text);
        if self.hover(r) {
            // A dica abre 6 abaixo do retângulo guardado: um sem altura, acima do meio, a centraliza.
            self.tip = Some((rect(r.right() + 8.0, r.y - self.dy + r.h / 2.0 - 21.0, 0.0, 0.0), text.to_string()));
        }
    }

    /// Rolagem guardada de uma lista, antes de desenhar.
    pub fn scroll_y_of(&self, key: &str) -> f32 {
        self.s.scroll.get(&hash_id(key)).copied().unwrap_or(0.0)
    }

    pub fn scroll_y(&self) -> f32 {
        self.dy
    }

    pub fn card(&self, r: Rect) {
        self.c.round(r, 12.0, CARD);
    }

    pub fn card_on(&self, r: Rect) {
        self.c.round(r, 12.0, CARD2);
    }

    pub fn title(&self, r: Rect, s: &str) {
        self.c.text(r, s, 15.0, TEXT, Align::Left, Weight::Semibold);
    }

    pub fn label(&self, r: Rect, s: &str, color: Color) {
        self.c.text(r, s, 13.0, color, Align::Left, Weight::Regular);
    }

    pub fn button(&mut self, key: &str, r: Rect, label: &str, primary: bool) -> bool {
        self.button_ex(key, r, label, primary, true)
    }

    pub fn button_ex(&mut self, key: &str, r: Rect, label: &str, primary: bool, on: bool) -> bool {
        let id = hash_id(key);
        let hover = on && self.hover(r);
        let n = self.s.targets.len();
        let clicked = on && self.clickable(id, r);
        self.named(n, label, a11y::Role::Button, None);
        if primary {
            let bg = if !on {
                LINE
            } else if hover {
                rgb(0xa9d9ff)
            } else {
                ACCENT
            };
            self.c.round(r, 8.0, bg);
            let fg = if on { ON_ACCENT } else { MUTED };
            self.c.text_fit(rect(r.x + 6.0, r.y, r.w - 12.0, r.h), label, 14.0, fg, Align::Center, Weight::Semibold);
        } else {
            if hover {
                self.c.round(r, 8.0, CARD2);
            }
            self.c.round_line(r, 8.0, if on { BORDER2 } else { BORDER }, 1.0);
            self.c.text_fit(rect(r.x + 6.0, r.y, r.w - 12.0, r.h), label, 14.0, if on { TEXT } else { MUTED }, Align::Center, Weight::Regular);
        }
        clicked
    }

    /// Largura de `segment` com estes nomes, para alinhar à direita.
    pub fn segment_width(names: &[&str]) -> f32 {
        names.iter().map(|n| measure(n, 14.0, Weight::Semibold, None).0 + 28.0 + 4.0).sum::<f32>() - 4.0
    }

    /// Abas (ou escolha de uma entre poucas): a escolhida em CARD2 com traço ACCENT embaixo, as
    /// outras só texto. Fica fora do azul cheio, que é da ação da página. Devolve true ao trocar.
    pub fn segment(&mut self, key: &str, x: f32, y: f32, h: f32, names: &[&str], sel: &mut usize) -> bool {
        let mut cx = x;
        let mut changed = false;
        for (i, name) in names.iter().enumerate() {
            let w = measure(name, 14.0, Weight::Semibold, None).0 + 28.0;
            let r = rect(cx, y, w, h);
            let id = hash_id(&format!("{key}#{i}"));
            let hover = self.hover(r);
            let n = self.s.targets.len();
            let hit = self.clickable(id, r);
            self.named(n, name, a11y::Role::Tab, Some(*sel == i));
            if hit && *sel != i {
                *sel = i;
                changed = true;
                self.s.needs_redraw = true;
            }
            let on = *sel == i;
            if on {
                self.c.round(r, 8.0, CARD2);
                self.c.round(rect(r.x + 12.0, r.bottom() - 3.0, r.w - 24.0, 2.0), 1.0, ACCENT);
            } else if hover {
                self.c.round(r, 8.0, LINE);
            }
            let color = if on || hover { TEXT } else { TEXT2 };
            self.c.text(r, name, 14.0, color, Align::Center, if on { Weight::Semibold } else { Weight::Regular });
            cx += w + 4.0;
        }
        changed
    }

    #[allow(clippy::too_many_arguments)]
    /// Abas sob o título de uma página: a escolhida em CARD2, as outras em MUTED, e o botão de aba
    /// do controle troca entre elas. `names` já vêm traduzidos; `key` + o número é o id de cada aba.
    /// `on` falso deixa as abas paradas; a aba `warn` vem em WARN.
    pub fn tabs<S: AsRef<str>>(&mut self, key: &str, x: f32, y: f32, names: &[S], sel: &mut usize, on: bool, warn: Option<usize>) {
        if on {
            self.tab_step(sel, names.len());
        }
        let mut cx = x;
        for (i, name) in names.iter().enumerate() {
            let name = name.as_ref();
            let w = measure(name, 14.0, Weight::Semibold, None).0 + 28.0;
            let r = rect(cx, y, w, 34.0);
            let cur = *sel == i;
            if cur {
                self.c.round(r, 8.0, CARD2);
            }
            if on && self.hit(&format!("{key}{i}"), r) {
                *sel = i;
            }
            let color = if warn == Some(i) { WARN } else if cur { TEXT } else { MUTED };
            self.c.text_raw(r, name, 14.0, color, Align::Center, if cur { Weight::Semibold } else { Weight::Regular });
            cx += w + 4.0;
        }
    }

    pub fn check(&mut self, key: &str, r: Rect, label: &str, val: &mut bool) -> bool {
        let id = hash_id(key);
        let n = self.s.targets.len();
        let clicked = self.clickable(id, r);
        if clicked {
            *val = !*val;
        }
        self.named(n, label, a11y::Role::Check, Some(*val));
        let bx = rect(r.x, r.y + (r.h - 18.0) / 2.0, 18.0, 18.0);
        if *val {
            self.c.round(bx, 4.0, ACCENT);
            self.c.icon(bx.x, bx.y, 18.0, &[&[(6.0, 12.5), (10.0, 16.5), (18.0, 7.5)]], ON_ACCENT);
        } else {
            self.c.round_line(bx, 4.0, BORDER2, 1.0);
        }
        let t = rect(r.x + 28.0, r.y, r.w - 28.0, r.h);
        self.c.text(t, label, 14.0, TEXT, Align::Left, Weight::Regular);
        clicked
    }

    pub fn select(&mut self, key: &str, r: Rect, options: &[String], sel: &mut usize) -> bool {
        self.select_in(key, r, options, &[], glyph::Style::Xbox, sel)
    }

    /// Lista de botões do controle: cada opção mostra o ícone do botão antes do nome. `glyphs` tem
    /// um bit por opção, na mesma ordem; 0 é opção sem ícone ("Nenhum", "Sozinho").
    pub fn select_pad(&mut self, key: &str, r: Rect, options: &[String], glyphs: &[crate::sys::pads::Buttons], style: glyph::Style, sel: &mut usize) -> bool {
        self.select_in(key, r, options, glyphs, style, sel)
    }

    fn select_in(&mut self, key: &str, r: Rect, options: &[String], glyphs: &[crate::sys::pads::Buttons], style: glyph::Style, sel: &mut usize) -> bool {
        let id = hash_id(key);
        let mut changed = false;
        if let Some((pid, v)) = self.s.popup_pick {
            if pid == id {
                *sel = v;
                changed = true;
                self.s.popup_pick = None;
            }
        }
        let hover = self.hover(r);
        let n = self.s.targets.len();
        let open = self.clickable(id, r);
        self.named(n, options.get(*sel).map_or("", |o| o.as_str()), a11y::Role::Combo, None);
        if open {
            let h = popup_h(options.len(), self.s.view_h);
            // Larga só o bastante para a opção mais longa: uma caixa larga não abre uma lista enorme.
            let widest = options.iter().map(|o| measure(o, 14.0, Weight::Regular, None).0).fold(0.0, f32::max);
            let icon = if glyphs.iter().any(|&g| g != 0) { GLYPH_H + 8.0 } else { 0.0 };
            let w = r.w.min((widest + icon + 40.0).max(200.0));
            let y = r.y - self.dy;
            let mut top = y + r.h + 4.0;
            // Sem espaço embaixo, a lista abre para cima.
            if self.s.view_h > 0.0 && top + h > self.s.view_h - 8.0 {
                top = (y - h - 4.0).max(8.0);
            }
            // Abre já mostrando a opção escolhida.
            let max = (options.len() as f32 * ROW + 8.0 - h).max(0.0);
            self.s.popup = Some(Popup {
                id,
                rect: rect(r.x, top, w, h),
                options: options.to_vec(),
                selected: *sel,
                off: Vec::new(),
                top: ((*sel as f32 - (ROWS_MAX / 2.0).floor()).max(0.0) * ROW).min(max),
                glyphs: glyphs.to_vec(),
                style,
            });
            self.s.popup_new = true;
            self.s.needs_redraw = true;
        }
        self.c.round(r, 8.0, SIDE);
        self.c.round_line(r, 8.0, if hover { BORDER2 } else { BORDER }, 1.0);
        let label = options.get(*sel).map(String::as_str).unwrap_or("");
        let gap = match glyphs.get(*sel).copied().unwrap_or(0) {
            0 => 0.0,
            bit => glyph::draw(self.c, r.x + 10.0, r.y + r.h / 2.0, GLYPH_H, style, bit, TEXT) + 6.0,
        };
        // O texto acaba 8 px antes do chevron (que começa em right - 24), para não encostar nele.
        let t = rect(r.x + 10.0 + gap, r.y, r.w - 42.0 - gap, r.h);
        self.c.text_fit(t, label, 14.0, TEXT, Align::Left, Weight::Regular);
        let (ax, ay) = (r.right() - 24.0, r.y + r.h / 2.0 - 2.0);
        self.c.line(ax, ay, ax + 5.0, ay + 5.0, MUTED, 1.5);
        self.c.line(ax + 5.0, ay + 5.0, ax + 10.0, ay, MUTED, 1.5);
        changed
    }

    /// Abre um menu onde o mouse está. A escolha chega depois por `menu_pick`.
    pub fn menu(&mut self, key: &str, items: &[(&str, bool)]) {
        let w = 230.0;
        let h = popup_h(items.len(), self.s.view_h);
        let mut x = self.s.mx;
        let mut y = self.s.my;
        if self.s.view_w > 0.0 && x + w > self.s.view_w - 8.0 {
            x = self.s.view_w - w - 8.0;
        }
        if self.s.view_h > 0.0 && y + h > self.s.view_h - 8.0 {
            y = (y - h).max(8.0);
        }
        self.s.popup = Some(Popup {
            id: hash_id(key),
            rect: rect(x, y, w, h),
            options: items.iter().map(|i| i.0.to_string()).collect(),
            selected: usize::MAX,
            off: items.iter().map(|i| !i.1).collect(),
            top: 0.0,
            glyphs: Vec::new(),
            style: glyph::Style::Xbox,
        });
        self.s.popup_new = true;
        self.s.needs_redraw = true;
    }

    pub fn menu_pick(&mut self, key: &str) -> Option<usize> {
        let id = hash_id(key);
        match self.s.popup_pick {
            Some((pid, v)) if pid == id => {
                self.s.popup_pick = None;
                Some(v)
            }
            _ => None,
        }
    }

    /// Janela própria de uma página. Enquanto ela estiver aberta, a página chama `block` antes de
    /// desenhar o resto (que deixa de receber cliques) e `sheet` no fim, para desenhar o fundo dela;
    /// os controles desenhados depois de `sheet` voltam a responder.
    #[cfg_attr(not(feature = "stream"), allow(dead_code))]
    pub fn block(&mut self) {
        self.blocked = true;
    }

    #[cfg_attr(not(feature = "stream"), allow(dead_code))]
    pub fn sheet(&mut self, w: f32, h: f32) -> Rect {
        let (vw, vh) = (self.s.view_w, self.s.view_h);
        self.c.translate(0.0, 0.0);
        self.dy = 0.0;
        self.clip = None;
        self.area = 0;
        self.c.fill(rect(0.0, 0.0, vw, vh), rgb(0x000000).alpha(0.6));
        #[cfg(debug_assertions)]
        gfx::forget_boxes();
        let w = w.min(vw - 48.0);
        let b = rect(((vw - w) / 2.0).floor(), ((vh - h) / 2.0).floor(), w, h);
        self.c.round(b, 14.0, CARD2);
        self.c.round_line(b, 14.0, BORDER2, 1.0);
        self.blocked = self.s.popup.is_some() || self.s.ask.is_some();
        b
    }

    /// Pergunta Sim ou Não. A resposta chega depois por `answered`, com a mesma chave.
    /// Há uma pergunta aberta (fechada sem `answered`, a resposta foi não).
    pub fn asking(&self) -> bool {
        self.s.ask.is_some()
    }

    pub fn ask(&mut self, key: &str, title: &str, body: &str) {
        self.s.ask = Some(Ask { id: hash_id(key), title: title.to_string(), body: body.to_string(), notice: false, check: None });
        self.s.ask_new = true;
        self.s.needs_redraw = true;
    }

    /// Pergunta com uma caixa de marcar. O estado dela chega por `ask_mark`, com qualquer resposta.
    pub fn ask_check(&mut self, key: &str, title: &str, body: &str, label: &str) {
        self.ask(key, title, body);
        if let Some(a) = &mut self.s.ask {
            a.check = Some((label.to_string(), false));
        }
    }

    pub fn ask_mark(&mut self, key: &str) -> Option<bool> {
        let id = hash_id(key);
        match self.s.ask_mark {
            Some((i, v)) if i == id => {
                self.s.ask_mark = None;
                Some(v)
            }
            _ => None,
        }
    }

    /// Aviso com um botão só.
    pub fn notice(&mut self, title: &str, body: &str) {
        self.s.ask = Some(Ask { id: 0, title: title.to_string(), body: body.to_string(), notice: true, check: None });
        self.s.ask_new = true;
        self.s.needs_redraw = true;
    }

    pub fn answered(&mut self, key: &str) -> bool {
        let id = hash_id(key);
        if self.s.answer == Some(id) {
            self.s.answer = None;
            return true;
        }
        false
    }

    /// Número com botões de menos e mais.
    pub fn stepper(&mut self, key: &str, r: Rect, val: &mut u32, range: (u32, u32), step: u32, unit: &str) -> bool {
        let mut changed = false;
        self.c.round(r, 8.0, SIDE);
        self.c.round_line(r, 8.0, BORDER, 1.0);
        let less = rect(r.x, r.y, 30.0, r.h);
        let more = rect(r.right() - 30.0, r.y, 30.0, r.h);
        if self.clickable(hash_id(&format!("{key}-")), less) {
            *val = val.saturating_sub(step).max(range.0);
            changed = true;
        }
        if self.clickable(hash_id(&format!("{key}+")), more) {
            *val = (*val + step).min(range.1);
            changed = true;
        }
        for (b, s) in [(less, "−"), (more, "+")] {
            let color = if self.hover(b) { ACCENT } else { TEXT2 };
            self.c.text(b, s, 16.0, color, Align::Center, Weight::Semibold);
        }
        self.c.text(r, &format!("{val}{unit}"), 13.0, TEXT, Align::Center, Weight::Regular);
        changed
    }

    /// Seletor de cor: o quadrado de saturação e brilho da matiz atual, e a faixa de matiz à
    /// direita. `r.h` é o lado do quadrado; a faixa ocupa mais 36 px de largura. Devolve true
    /// enquanto a pessoa arrasta.
    pub fn color_picker(&mut self, key: &str, r: Rect, hsv: &mut [f32; 3]) -> bool {
        const N: u32 = 64;
        let sq = rect(r.x, r.y, r.h, r.h);
        let hue = rect(r.x + r.h + 12.0, r.y, 24.0, r.h);
        // O quadrado é redesenhado a cada quadro porque muda junto com a matiz.
        let mut px = vec![0u8; (N * N * 4) as usize];
        for yy in 0..N {
            for xx in 0..N {
                let c = hsv_rgb(hsv[0], xx as f32 / (N - 1) as f32, 1.0 - yy as f32 / (N - 1) as f32);
                let i = ((yy * N + xx) * 4) as usize;
                px[i..i + 4].copy_from_slice(&[c[2], c[1], c[0], 255]);
            }
        }
        if let Some(b) = self.c.bitmap(N, N, &px) {
            self.c.draw_bitmap(&b, sq);
        }
        let mut hpx = vec![0u8; (N * 4) as usize];
        for yy in 0..N {
            let c = hsv_rgb(yy as f32 / (N - 1) as f32 * 360.0, 1.0, 1.0);
            let i = (yy * 4) as usize;
            hpx[i..i + 4].copy_from_slice(&[c[2], c[1], c[0], 255]);
        }
        if let Some(b) = self.c.bitmap(1, N, &hpx) {
            self.c.draw_bitmap(&b, hue);
        }
        let mut changed = false;
        let drag = |ui: &mut Self, name: &str, area: Rect| -> Option<(f32, f32)> {
            let id = hash_id(&format!("{key}{name}"));
            if ui.hover(area) && ui.s.pressed {
                ui.s.active = id;
            }
            if ui.s.active != id {
                return None;
            }
            if ui.s.released || !ui.s.down {
                ui.s.active = 0;
            }
            Some((((ui.s.mx - area.x) / area.w).clamp(0.0, 1.0), ((ui.s.my + ui.dy - area.y) / area.h).clamp(0.0, 1.0)))
        };
        if let Some((fx, fy)) = drag(self, "-sv", sq) {
            (hsv[1], hsv[2]) = (fx, 1.0 - fy);
            changed = true;
        }
        if let Some((_, fy)) = drag(self, "-h", hue) {
            hsv[0] = fy * 360.0;
            changed = true;
        }
        self.c.round_line(sq, 2.0, BORDER2, 1.0);
        self.c.round_line(hue, 2.0, BORDER2, 1.0);
        // Marcas: um anel no quadrado e uma barra na faixa.
        let (mx, my) = (sq.x + hsv[1] * sq.w, sq.y + (1.0 - hsv[2]) * sq.h);
        self.c.circle_line(mx, my, 6.0, if hsv[2] > 0.6 { rgb(0x000000) } else { WHITE }, 2.0);
        let hy = hue.y + hsv[0] / 360.0 * hue.h;
        self.c.round_line(rect(hue.x - 2.0, hy - 3.0, hue.w + 4.0, 6.0), 3.0, WHITE, 2.0);
        changed
    }

    pub fn slider(&mut self, key: &str, r: Rect, val: &mut f32) -> bool {
        let id = hash_id(key);
        let hover = self.hover(r);
        if hover && self.s.pressed {
            self.s.active = id;
        }
        let mut changed = false;
        if self.s.active == id {
            *val = ((self.s.mx - r.x) / r.w).clamp(0.0, 1.0);
            changed = true;
            if self.s.released || !self.s.down {
                self.s.active = 0;
            }
        }
        let cy = r.y + r.h / 2.0;
        self.c.round(rect(r.x, cy - 2.0, r.w, 4.0), 2.0, BORDER);
        self.c.round(rect(r.x, cy - 2.0, r.w * *val, 4.0), 2.0, ACCENT);
        self.c.circle(r.x + r.w * *val, cy, 7.0, if hover { WHITE } else { ACCENT });
        changed
    }

    /// Arrastar que começou dentro de `r`: true enquanto o botão segue apertado (o mouse pode sair).
    pub fn drag(&mut self, key: &str, r: Rect) -> bool {
        let id = hash_id(key);
        if self.hover(r) && self.s.pressed {
            self.s.active = id;
        }
        let on = self.s.active == id;
        if on && (self.s.released || !self.s.down) {
            self.s.active = 0;
        }
        on
    }

    /// O campo de texto `key` está com o foco.
    pub fn focused(&self, key: &str) -> bool {
        self.s.focus == hash_id(key)
    }

    /// Campo que captura a próxima combinação de teclas. Esc cancela e Backspace limpa.
    pub fn hotkey(&mut self, key: &str, r: Rect, hk: &mut Hotkey) -> bool {
        let id = hash_id(key);
        let mut changed = false;
        if self.clickable(id, r) {
            self.s.capturing = id;
            self.s.needs_redraw = true;
        }
        // Botão do meio (roda) no campo apaga a combinação.
        if self.s.middle_released && self.hover(r) {
            *hk = Hotkey::default();
            changed = true;
            self.s.last_hotkey = id;
            if self.s.capturing == id {
                self.s.capturing = 0;
            }
        }
        let active = self.s.capturing == id;
        if active {
            if let Some(k) = self.s.key_event {
                *hk = k;
                changed = true;
                self.s.last_hotkey = id;
                self.s.capturing = 0;
            } else if self.s.key_cancel {
                self.s.capturing = 0;
            }
        }
        self.c.round(r, 8.0, SIDE);
        self.c.round_line(r, 8.0, if active { ACCENT } else { BORDER }, 1.0);
        let txt = if active { "Pressione as teclas".to_string() } else { hk.label() };
        let color = if active { ACCENT } else { TEXT };
        self.c.text(r.inset_xy(10.0, 0.0), &txt, 14.0, color, Align::Left, Weight::Regular);
        changed
    }

    /// Ícone do programa em `path`. Sem ícone, desenha a inicial de `name`.
    /// Imagem embutida no programa, em pixels prontos para a tela.
    pub fn image(&mut self, key: &str, size: u32, px: &[u8], r: Rect) {
        if !self.s.icons.contains_key(key) {
            let b = self.c.bitmap(size, size, px);
            self.s.icons.insert(key.to_string(), b);
        }
        if let Some(b) = self.s.icons.get(key).and_then(|b| b.as_ref()) {
            self.c.draw_bitmap(b, r);
        }
    }

    /// PNG embutido. Só abre quando aparece, e sai da memória ao fechar a janela.
    pub fn png(&mut self, key: &str, data: &[u8], r: Rect) {
        if !self.s.icons.contains_key(key) {
            let b = icons::decode_png(data).and_then(|(w, h, px)| self.c.bitmap(w, h, &px));
            self.s.icons.insert(key.to_string(), b);
        }
        if let Some(b) = self.s.icons.get(key).and_then(|b| b.as_ref()) {
            self.c.draw_bitmap(b, r);
        }
    }

    pub fn file_icon(&mut self, path: &str, name: &str, r: Rect) {
        let key = path.to_ascii_lowercase();
        if !self.s.icons.contains_key(&key) {
            let b = icons::load(path).and_then(|(w, h, px)| self.c.bitmap(w, h, &px));
            self.s.icons.insert(key.clone(), b);
        }
        match self.s.icons.get(&key).and_then(|b| b.as_ref()) {
            Some(b) => self.c.draw_bitmap(b, r),
            None => {
                self.c.round(r, 6.0, LINE);
                let l: String = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                self.c.text(r, &l, 11.0, TEXT2, Align::Center, Weight::Semibold);
            }
        }
    }

    /// Ícone pelo tipo do arquivo. Programas e atalhos usam o ícone deles mesmos.
    pub fn kind_icon(&mut self, path: &str, name: &str, dir: bool, r: Rect) {
        let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
        if !dir && matches!(ext.as_str(), "exe" | "lnk" | "ico" | "url" | "msc" | "cpl" | "scr") {
            return self.file_icon(path, name, r);
        }
        let key = if dir { "<pasta>".to_string() } else { format!("<tipo>.{ext}") };
        if !self.s.icons.contains_key(&key) {
            let probe = if dir { "pasta".to_string() } else { format!("arquivo.{ext}") };
            let b = icons::load_kind(&probe, dir).and_then(|(w, h, px)| self.c.bitmap(w, h, &px));
            self.s.icons.insert(key.clone(), b);
        }
        match self.s.icons.get(&key).and_then(|b| b.as_ref()) {
            Some(b) => self.c.draw_bitmap(b, r),
            None => self.c.round(r, 4.0, LINE),
        }
    }

    /// Campo de busca. Clique dá o foco; digitação chega pela janela.
    pub fn search(&mut self, key: &str, r: Rect, text: &mut String, hint: &str) -> bool {
        let id = hash_id(key);
        let n = self.s.targets.len();
        let clicked = self.clickable(id, r);
        if let Some(t) = self.s.targets.get_mut(n) {
            t.field = true;
        }
        self.named(n, if text.is_empty() { hint } else { text.as_str() }, a11y::Role::Edit, None);
        if clicked {
            self.s.focus = id;
        } else if self.s.released && !self.hover(r) && self.s.focus == id {
            self.s.focus = 0;
        }
        let on = self.s.focus == id;
        let mut changed = false;
        if on {
            if !self.s.typed.is_empty() {
                text.push_str(&self.s.typed);
                self.s.typed.clear();
                changed = true;
            }
            for _ in 0..self.s.backspace {
                changed |= text.pop().is_some();
            }
            self.s.backspace = 0;
            for _ in 0..self.s.kill_word {
                while text.ends_with(' ') {
                    text.pop();
                }
                while text.chars().last().is_some_and(|c| c != ' ') {
                    text.pop();
                }
                changed = true;
            }
            self.s.kill_word = 0;
        }
        self.c.round(r, 8.0, SIDE);
        self.c.round_line(r, 8.0, if on { ACCENT } else { BORDER }, 1.0);
        let t = r.inset_xy(10.0, 0.0);
        if text.is_empty() && !on {
            self.c.text(t, hint, 14.0, MUTED, Align::Left, Weight::Regular);
        } else {
            let shown = if on { format!("{text}|") } else { text.clone() };
            self.c.text(t, &shown, 14.0, TEXT, Align::Left, Weight::Regular);
        }
        changed
    }

    /// Clique com o botão direito numa área.
    pub fn right_hit(&mut self, r: Rect) -> bool {
        self.s.right_released && self.hover(r)
    }

    /// LT/RT do controle: aba anterior ou seguinte entre `n`, dando a volta.
    pub fn tab_step(&mut self, sel: &mut usize, n: usize) {
        if let Some(d) = self.s.pad_tab.take().filter(|_| n > 0) {
            *sel = (*sel as isize + d as isize).rem_euclid(n as isize) as usize;
            self.s.needs_redraw = true;
        }
    }

    /// Botão com área invisível, para linhas clicáveis.
    pub fn hit(&mut self, key: &str, r: Rect) -> bool {
        self.clickable(hash_id(key), r)
    }

    pub fn end(mut self) {
        self.c.translate(0.0, 0.0);
        let mut close = false;
        let mut pick = None;
        if let Some(p) = &mut self.s.popup {
            let max = (p.options.len() as f32 * ROW + 8.0 - p.rect.h).max(0.0);
            if self.s.wheel != 0.0 && p.rect.contains(self.s.mx, self.s.my) {
                p.top = (p.top - self.s.wheel * ROW * 3.0).clamp(0.0, max);
                self.s.needs_redraw = true;
            }
            let p = &*p;
            let shadow = rect(p.rect.x, p.rect.y, p.rect.w, p.rect.h);
            self.c.round(shadow, 10.0, CARD2);
            self.c.round_line(shadow, 10.0, BORDER2, 1.0);
            let inner = p.rect.inset_xy(0.0, 4.0);
            let mut rows = vec![];
            self.c.push_clip(inner);
            for (i, o) in p.options.iter().enumerate() {
                let r = rect(p.rect.x + 4.0, p.rect.y + 4.0 + i as f32 * ROW - p.top, p.rect.w - (if max > 0.0 { 16.0 } else { 8.0 }), ROW);
                if r.bottom() < inner.y || r.y > inner.bottom() {
                    continue;
                }
                let off = p.off.get(i).copied().unwrap_or(false);
                if !off {
                    rows.push((r, o.to_string()));
                }
                let hover = !off && r.contains(self.s.mx, self.s.my) && inner.contains(self.s.mx, self.s.my);
                if hover {
                    self.c.round(r, 6.0, BORDER);
                }
                let color = if off {
                    MUTED
                } else if i == p.selected {
                    ACCENT
                } else {
                    TEXT
                };
                let gap = match p.glyphs.get(i).copied().unwrap_or(0) {
                    0 => 0.0,
                    bit => glyph::draw(self.c, r.x + 8.0, r.y + r.h / 2.0, GLYPH_H, p.style, bit, color) + 6.0,
                };
                let tr = r.inset_xy(8.0, 0.0);
                self.c.text_fit(rect(tr.x + gap, tr.y, tr.w - gap, tr.h), o, 14.0, color, Align::Left, Weight::Regular);
                if hover && self.s.released && !self.s.popup_new {
                    pick = Some((p.id, i));
                }
            }
            self.c.pop_clip();
            if max > 0.0 {
                let span = inner.h;
                let th = (span * span / (span + max)).max(24.0);
                let ty = inner.y + (span - th) * (p.top / max);
                self.c.round(rect(p.rect.right() - 8.0, ty, 4.0, th), 2.0, BORDER2);
            }
            if (self.s.released || self.s.right_released) && !self.s.popup_new {
                close = true;
            }
            self.s.popup_new = false;
            for (r, o) in rows {
                let n = self.s.targets.len();
                self.target(r, Some(inner), POPUP);
                self.named(n, &o, a11y::Role::Item, None);
            }
        }
        if close {
            self.s.popup = None;
            self.s.popup_pick = pick;
            self.s.needs_redraw = true;
        }
        if let Some((r, text)) = self.tip.take() {
            // Texto longo quebra em linhas; sem espaço embaixo da linha, a dica abre em cima.
            let max = (self.s.view_w - 16.0).min(640.0).max(120.0);
            let one = measure(&text, 13.0, Weight::Regular, None).0 + 20.0;
            let w = one.min(max);
            let h = if one > max { measure(&text, 13.0, Weight::Regular, Some(w - 20.0)).1 + 14.0 } else { 30.0 };
            let below = r.bottom() + 6.0;
            let y = if self.s.view_h > 0.0 && below + h > self.s.view_h - 8.0 { (r.y - h - 6.0).max(8.0) } else { below };
            let b = rect(r.x.min(self.s.view_w - w - 8.0).max(8.0), y, w, h);
            self.c.round(b, 8.0, CARD2);
            self.c.round_line(b, 8.0, BORDER2, 1.0);
            if one > max {
                self.c.text_wrap(rect(b.x + 10.0, b.y + 7.0, b.w - 20.0, b.h - 14.0), &text, 13.0, TEXT, Weight::Regular);
            } else {
                self.c.text(b, &text, 13.0, TEXT, Align::Center, Weight::Regular);
            }
        }
        self.draw_ask();
        if self.s.pad {
            self.pad_bar();
        } else if self.s.keys {
            self.ring();
        }
        self.blocked = false;
        self.s.end_frame();
    }

    /// Anel azul no controle sob o cursor, que é onde está o foco do controle ou do teclado.
    fn ring(&mut self) {
        let (mx, my) = (self.s.mx, self.s.my);
        if let Some(t) = self.s.targets.iter().rev().find(|t| t.shown() && t.r.contains(mx, my)) {
            self.c.round_line(t.r.inset(-3.0), 10.0, ACCENT, 2.0);
        }
    }

    /// Controle em uso: anel no controle sob o cursor e, embaixo, o que cada botão faz.
    fn pad_bar(&mut self) {
        use crate::sys::{pads, remap};
        self.ring();
        let t = |s: &str| crate::i18n::tr(s).into_owned();
        let mut items = vec![(vec![glyph::DPAD_ANY], t("Mover")), (vec![pads::A], t("Escolher"))];
        if self.s.popup.is_some() || self.s.ask.is_some() {
            items.push((vec![pads::B], t("Fechar")));
        } else {
            items.extend([
                (vec![pads::B], t("Voltar")),
                (vec![pads::LB, pads::RB], t("Página")),
                (vec![remap::LT, remap::RT], t("Aba")),
                (vec![pads::LS], t("Mouse")),
                (vec![pads::RS], t("Rolar")),
                (vec![pads::X], t("Clique")),
                (vec![pads::Y], t("Esconder")),
            ]);
        }
        let r = rect(0.0, self.s.view_h, self.s.view_w, PAD_BAR);
        self.c.fill(r, SIDE);
        self.c.line(r.x, r.y, r.right(), r.y, BORDER, 1.0);
        glyph::hints(self.c, r.inset_xy(16.0, 9.0), glyph::Style::current(), &items, 13.0, TEXT2);
    }

    fn draw_ask(&mut self) {
        let Some(a) = &self.s.ask else { return };
        let (vw, vh) = (self.s.view_w, self.s.view_h);
        self.c.fill(rect(0.0, 0.0, vw, vh), rgb(0x000000).alpha(0.6));
        let w = 460.0f32.min(vw - 48.0);
        let body_h = measure(&a.body, 14.0, Weight::Regular, Some(w - 48.0)).1;
        let h = 76.0 + body_h + 76.0 + if a.check.is_some() { 40.0 } else { 0.0 };
        let b = rect((vw - w) / 2.0, (vh - h) / 2.0, w, h);
        self.c.round(b, 14.0, CARD2);
        self.c.round_line(b, 14.0, BORDER2, 1.0);
        self.c.text(rect(b.x + 24.0, b.y + 22.0, b.w - 48.0, 28.0), &a.title, 17.0, TEXT, Align::Left, Weight::Semibold);
        self.c.text_wrap(rect(b.x + 24.0, b.y + 62.0, b.w - 48.0, body_h + 4.0), &a.body, 14.0, TEXT2, Weight::Regular);
        let by = b.bottom() - 60.0;
        let yes = rect(b.right() - 24.0 - 112.0, by, 112.0, 40.0);
        let no = rect(yes.x - 120.0, by, 112.0, 40.0);
        let (mx, my) = (self.s.mx, self.s.my);
        let check_row = a.check.as_ref().map(|_| rect(b.x + 24.0, by - 38.0, b.w - 48.0, 28.0));
        let mut toggle = false;
        if let Some((label, on)) = &a.check {
            let row = rect(b.x + 24.0, by - 38.0, b.w - 48.0, 28.0);
            let bx = rect(row.x, row.y + 5.0, 18.0, 18.0);
            if *on {
                self.c.round(bx, 4.0, ACCENT);
                self.c.icon(bx.x, bx.y, 18.0, &[&[(6.0, 12.5), (10.0, 16.5), (18.0, 7.5)]], ON_ACCENT);
            } else {
                self.c.round_line(bx, 4.0, BORDER2, 1.0);
            }
            self.c.text(rect(row.x + 28.0, row.y, row.w - 28.0, row.h), label, 14.0, TEXT, Align::Left, Weight::Regular);
            toggle = self.s.released && !self.s.ask_new && row.contains(mx, my);
        }
        let bg = if yes.contains(mx, my) { rgb(0xa9d9ff) } else { ACCENT };
        self.c.round(yes, 8.0, bg);
        self.c.text(yes, if a.notice { "OK" } else { "Sim" }, 14.0, ON_ACCENT, Align::Center, Weight::Semibold);
        if !a.notice {
            if no.contains(mx, my) {
                self.c.round(no, 8.0, BORDER);
            }
            self.c.round_line(no, 8.0, BORDER2, 1.0);
            self.c.text(no, "Não", 14.0, TEXT, Align::Center, Weight::Regular);
        }
        if self.s.released && !self.s.ask_new {
            let id = a.id;
            let notice = a.notice;
            let mark = a.check.as_ref().map(|c| (id, c.1));
            if yes.contains(mx, my) {
                self.s.ask = None;
                self.s.ask_mark = mark;
                if !notice {
                    self.s.answer = Some(id);
                }
                self.s.needs_redraw = true;
            } else if !notice && no.contains(mx, my) {
                self.s.ask = None;
                self.s.ask_mark = mark;
                self.s.needs_redraw = true;
            }
        }
        if toggle {
            if let Some((_, on)) = self.s.ask.as_mut().and_then(|a| a.check.as_mut()) {
                *on = !*on;
            }
        }
        self.s.ask_new = false;
        let notice = self.s.ask.as_ref().is_some_and(|a| a.notice);
        let mark = self.s.ask.as_ref().and_then(|a| a.check.clone());
        let named = [
            check_row.map(|r| (r, mark.as_ref().map_or(String::new(), |c| c.0.clone()), a11y::Role::Check, mark.as_ref().map(|c| c.1))),
            (!notice).then_some((no, "Não".to_string(), a11y::Role::Button, None)),
            Some((yes, if notice { "OK" } else { "Sim" }.to_string(), a11y::Role::Button, None)),
        ];
        for (r, name, role, on) in named.into_iter().flatten() {
            let n = self.s.targets.len();
            self.target(r, None, 0);
            self.named(n, &name, role, on);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(x: f32, y: f32, area: u64, clip: Rect) -> Target {
        Target { r: rect(x, y, 100.0, 30.0), clip, area, field: false, rows: false }
    }

    #[test]
    fn pad_moves_and_scrolls() {
        let (view, list, id) = (rect(0.0, 0.0, 800.0, 600.0), rect(0.0, 100.0, 400.0, 200.0), 7);
        let mut s = UiState::default();
        s.scroll_max.insert(id, 500.0);
        // Botão à direita, três linhas da lista (a última fora da vista) e um botão embaixo.
        s.last = vec![t(500.0, 120.0, 0, view), t(10.0, 110.0, id, list), t(10.0, 150.0, id, list), t(10.0, 320.0, id, list), t(10.0, 400.0, 0, view)];
        (s.mx, s.my) = (20.0, 160.0);
        // Para baixo: a linha seguinte está fora da vista, a lista rola até ela caber.
        assert_eq!(s.pad_move(Some(3)), Some((60.0, 277.0)));
        assert_eq!(s.scroll[&id], 58.0);
        assert_eq!(s.pad_move(Some(2)), Some((550.0, 135.0)));
        assert_eq!(s.pad_move(Some(1)), Some((60.0, 125.0)));
        assert_eq!(s.pad_move(Some(0)), None);
        // Fora de qualquer controle: vai ao mais perto.
        (s.mx, s.my) = (700.0, 500.0);
        assert_eq!(s.pad_move(None), Some((550.0, 135.0)));
    }

    #[test]
    fn tab_goes_round_in_draw_order() {
        let view = rect(0.0, 0.0, 800.0, 600.0);
        let mut s = UiState::default();
        // O do meio está fora da vista: Tab pula ele.
        s.last = vec![t(10.0, 10.0, 0, view), t(10.0, 700.0, 0, view), t(10.0, 100.0, 0, view)];
        (s.mx, s.my) = (400.0, 400.0);
        assert_eq!(s.tab_move(false), Some((60.0, 25.0)));
        assert_eq!(s.tab_move(true), Some((60.0, 115.0)));
        (s.mx, s.my) = (60.0, 25.0);
        assert_eq!(s.tab_move(false), Some((60.0, 115.0)));
        assert_eq!(s.tab_move(true), Some((60.0, 115.0)));
        (s.mx, s.my) = (60.0, 115.0);
        assert_eq!(s.tab_move(false), Some((60.0, 25.0)));
    }

    #[test]
    fn tab_scrolls_a_list_to_the_next_row() {
        let (list, id) = (rect(0.0, 100.0, 400.0, 100.0), 7);
        let mut s = UiState::default();
        s.scroll_max.insert(id, 500.0);
        // Duas linhas à vista e a terceira abaixo da vista da lista.
        s.last = vec![t(10.0, 110.0, id, list), t(10.0, 150.0, id, list), t(10.0, 230.0, id, list)];
        (s.mx, s.my) = (20.0, 165.0);
        // Tab: o cursor desce até a linha de fora, e a lista rola até ela caber.
        let to = s.tab_move(false).unwrap();
        assert_eq!(s.scroll[&id], 68.0);
        assert!(list.contains(to.0, to.1));
    }

    #[test]
    fn tab_takes_a_row_list_as_one_stop() {
        let (view, list, id) = (rect(0.0, 0.0, 800.0, 600.0), rect(0.0, 100.0, 400.0, 100.0), 7);
        let row = |y| Target { rows: true, ..t(10.0, y, id, list) };
        let mut s = UiState::default();
        s.scroll_max.insert(id, 5000.0);
        // Botão, duas linhas à vista e uma abaixo da vista da lista, e outro botão.
        s.last = vec![t(10.0, 10.0, 0, view), row(110.0), row(150.0), row(230.0), t(10.0, 400.0, 0, view)];
        (s.mx, s.my) = (60.0, 25.0);
        // Tab entra na primeira linha à vista.
        assert_eq!(s.tab_move(false), Some((60.0, 125.0)));
        // De qualquer linha, Tab sai da lista sem rolar, e Shift+Tab volta a ela.
        (s.mx, s.my) = (60.0, 165.0);
        assert_eq!(s.tab_move(false), Some((60.0, 415.0)));
        assert!(s.scroll.is_empty());
        assert_eq!(s.tab_move(true), Some((60.0, 25.0)));
        (s.mx, s.my) = (60.0, 415.0);
        assert_eq!(s.tab_move(true), Some((60.0, 125.0)));
    }
}
