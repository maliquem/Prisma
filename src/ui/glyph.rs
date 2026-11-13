//! Ícones dos botões do controle, no traço dos ícones do Prisma e sem cor própria: a cor vem de
//! quem desenha. A forma segue o controle em uso (Xbox, PlayStation, Nintendo); nenhum logo de
//! marca, o botão Guia é um círculo com um ponto.

use super::gfx::*;
use crate::sys::pads::{self, Family};
use crate::sys::remap;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Style {
    Xbox,
    PlayStation,
    /// Mesmas letras do Xbox em outras posições: A à direita, B embaixo, X em cima, Y à esquerda.
    Nintendo,
    /// Controle genérico: os botões numerados, como o Windows mostra.
    Numbers,
}

impl Style {
    pub fn of(f: Family, vid: u16) -> Self {
        match f {
            Family::DualSense | Family::Ds4 => Style::PlayStation,
            Family::Xbox => Style::Xbox,
            Family::Generic if vid == 0x057e => Style::Nintendo,
            Family::Generic => Style::Numbers,
        }
    }

    /// O primeiro controle da lista; sem nenhum, o Xbox.
    pub fn current() -> Self {
        pads::list().first().map_or(Style::Xbox, |p| Style::of(p.family, p.vid))
    }
}

/// Letra ou número do botão de face `bit` (pela posição: A embaixo, B à direita, X à esquerda,
/// Y em cima), fora do PlayStation, que usa formas.
fn face_text(s: Style, bit: u32) -> &'static str {
    let i = [pads::A, pads::B, pads::X, pads::Y].iter().position(|&b| b == bit).unwrap_or(0);
    match s {
        Style::Nintendo => ["B", "A", "Y", "X"][i],
        Style::Numbers => ["1", "2", "3", "4"][i],
        _ => ["A", "B", "X", "Y"][i],
    }
}

/// Texto dentro dos ombros, gatilhos e sticks.
fn side_text(s: Style, bit: u32) -> &'static str {
    let left = matches!(bit, pads::LB | remap::LT | pads::LS);
    let k = match bit {
        pads::LB | pads::RB => 0,
        remap::LT | remap::RT => 1,
        _ => 2,
    };
    let t: [[&str; 2]; 3] = match s {
        Style::PlayStation => [["L1", "R1"], ["L2", "R2"], ["L", "R"]],
        Style::Nintendo => [["L", "R"], ["ZL", "ZR"], ["L", "R"]],
        Style::Numbers => [["5", "6"], ["LT", "RT"], ["L", "R"]],
        Style::Xbox => [["LB", "RB"], ["LT", "RT"], ["L", "R"]],
    };
    t[k][if left { 0 } else { 1 }]
}

/// O direcional inteiro, sem braço aceso.
pub const DPAD_ANY: u32 = pads::UP | pads::DOWN | pads::LEFT | pads::RIGHT;

/// Largura do ícone de `bit` com altura `h`.
pub fn width(bit: u32, h: f32) -> f32 {
    match bit {
        pads::LB | pads::RB | remap::LT | remap::RT | pads::PAD => h * 1.7,
        remap::LSU | remap::LSD | remap::LSL | remap::LSR | remap::RSU | remap::RSD | remap::RSL | remap::RSR => h * 1.6,
        _ => h,
    }
}

fn stroke(h: f32) -> f32 {
    (h / 14.0).clamp(1.2, 1.6)
}

/// Desenha o botão `bit` com o canto esquerdo em `x` e o meio em `cy`. Devolve a largura.
pub fn draw(c: &Canvas, x: f32, cy: f32, h: f32, s: Style, bit: u32, col: Color) -> f32 {
    let (w, r, sw) = (width(bit, h), h / 2.0, stroke(h));
    let cx = x + r;
    let label = |t: &str, size: f32, at: Rect| c.text_raw(at, t, size, col, Align::Center, Weight::Semibold);
    match bit {
        DPAD_ANY => dpad(c, cx, cy, h, None, col),
        pads::A | pads::B | pads::X | pads::Y => {
            c.circle_line(cx, cy, r - sw / 2.0, col, sw);
            if s == Style::PlayStation {
                ps_shape(c, cx, cy, h * 0.2, bit, col, sw);
            } else {
                label(face_text(s, bit), h * 0.52, rect(x, cy - r, h, h));
            }
        }
        pads::LB | pads::RB | remap::LT | remap::RT => {
            // Ombro reto; gatilho mais redondo, como a peça.
            let rad = if matches!(bit, pads::LB | pads::RB) { h * 0.22 } else { h * 0.48 };
            c.round_line(rect(x, cy - r, w, h), rad, col, sw);
            label(side_text(s, bit), h * 0.46, rect(x, cy - r, w, h));
        }
        pads::LS | pads::RS => stick(c, cx, cy, r, s, bit, col, sw),
        pads::UP | pads::RIGHT | pads::DOWN | pads::LEFT => {
            let d = [pads::UP, pads::RIGHT, pads::DOWN, pads::LEFT].iter().position(|&b| b == bit);
            dpad(c, cx, cy, h, d, col);
        }
        remap::LSU | remap::LSD | remap::LSL | remap::LSR | remap::RSU | remap::RSD | remap::RSL | remap::RSR => {
            let left = matches!(bit, remap::LSU | remap::LSD | remap::LSL | remap::LSR);
            stick(c, cx, cy, r, s, if left { pads::LS } else { pads::RS }, col, sw);
            let d = [remap::LSU, remap::LSR, remap::LSD, remap::LSL, remap::RSU, remap::RSR, remap::RSD, remap::RSL].iter().position(|&b| b == bit).unwrap_or(0) % 4;
            arrow(c, x + h + (w - h) / 2.0, cy, h * 0.24, d, col, sw);
        }
        pads::GUIDE => {
            c.circle_line(cx, cy, r - sw / 2.0, col, sw);
            c.circle(cx, cy, h * 0.14, col);
        }
        pads::BACK | pads::START => {
            c.circle_line(cx, cy, r - sw / 2.0, col, sw);
            let k = h * 0.16;
            match (s, bit) {
                (Style::Nintendo, pads::BACK) => c.line(cx - k, cy, cx + k, cy, col, sw),
                (Style::Nintendo, _) => {
                    c.line(cx - k, cy, cx + k, cy, col, sw);
                    c.line(cx, cy - k, cx, cy + k, col, sw);
                }
                // Dois quadrados um sobre o outro (voltar, ver) e três traços (menu).
                (_, pads::BACK) => {
                    c.round_line(rect(cx - k * 1.2, cy - k * 1.2, k * 1.5, k * 1.5), 1.0, col, sw * 0.8);
                    c.round_line(rect(cx - k * 0.3, cy - k * 0.3, k * 1.5, k * 1.5), 1.0, col, sw * 0.8);
                }
                _ => {
                    for dy in [-k * 0.8, 0.0, k * 0.8] {
                        c.line(cx - k, cy + dy, cx + k, cy + dy, col, sw * 0.8);
                    }
                }
            }
        }
        pads::PAD => {
            c.round_line(rect(x, cy - r * 0.8, w, h * 0.8), h * 0.18, col, sw);
            c.line(x + w / 2.0, cy - r * 0.8, x + w / 2.0, cy + r * 0.8, col, sw * 0.6);
        }
        _ => {
            // Mudo e o que mais vier: uma pílula com um ponto.
            c.round_line(rect(x, cy - r, w, h), h * 0.3, col, sw);
            c.circle(cx, cy, h * 0.12, col);
        }
    }
    w
}

/// Cruz, círculo, quadrado e triângulo do PlayStation, pela posição do botão.
fn ps_shape(c: &Canvas, cx: f32, cy: f32, k: f32, bit: u32, col: Color, sw: f32) {
    match bit {
        pads::A => {
            c.line(cx - k, cy - k, cx + k, cy + k, col, sw);
            c.line(cx - k, cy + k, cx + k, cy - k, col, sw);
        }
        pads::B => c.circle_line(cx, cy, k, col, sw),
        pads::X => c.round_line(rect(cx - k, cy - k, k * 2.0, k * 2.0), 0.5, col, sw),
        _ => {
            let (top, l, r) = ((cx, cy - k * 1.1), (cx - k * 1.15, cy + k * 0.85), (cx + k * 1.15, cy + k * 0.85));
            c.line(top.0, top.1, l.0, l.1, col, sw);
            c.line(l.0, l.1, r.0, r.1, col, sw);
            c.line(r.0, r.1, top.0, top.1, col, sw);
        }
    }
}

fn stick(c: &Canvas, cx: f32, cy: f32, r: f32, s: Style, bit: u32, col: Color, sw: f32) {
    c.circle_line(cx, cy, r - sw / 2.0, col, sw);
    c.circle_line(cx, cy, r * 0.62, col, sw * 0.7);
    c.text_raw(rect(cx - r, cy - r, r * 2.0, r * 2.0), side_text(s, bit), r * 0.85, col, Align::Center, Weight::Semibold);
}

/// Setinha na direção `d` (0 cima, 1 direita, 2 baixo, 3 esquerda).
fn arrow(c: &Canvas, cx: f32, cy: f32, k: f32, d: usize, col: Color, sw: f32) {
    let (ux, uy) = [(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)][d % 4];
    let (tx, ty) = (cx + ux * k, cy + uy * k);
    c.line(cx - ux * k, cy - uy * k, tx, ty, col, sw);
    for side in [1.0, -1.0] {
        c.line(tx, ty, tx - ux * k * 0.7 - uy * k * 0.6 * side, ty - uy * k * 0.7 + ux * k * 0.6 * side, col, sw);
    }
}

/// Direcional em cruz, com o braço `lit` (0 cima, 1 direita, 2 baixo, 3 esquerda) preenchido.
pub fn dpad(c: &Canvas, cx: f32, cy: f32, h: f32, lit: Option<usize>, col: Color) {
    let (r, a, sw) = (h / 2.0, h * 0.17, stroke(h));
    // Contorno da cruz, ponto a ponto no sentido do relógio.
    let p = [(-a, -r), (a, -r), (a, -a), (r, -a), (r, a), (a, a), (a, r), (-a, r), (-a, a), (-r, a), (-r, -a), (-a, -a)];
    for i in 0..p.len() {
        let (q0, q1) = (p[i], p[(i + 1) % p.len()]);
        c.line(cx + q0.0, cy + q0.1, cx + q1.0, cy + q1.1, col, sw);
    }
    if let Some(d) = lit {
        let arm = [rect(cx - a, cy - r, a * 2.0, r - a), rect(cx + a, cy - a, r - a, a * 2.0), rect(cx - a, cy + a, a * 2.0, r - a), rect(cx - r, cy - a, r - a, a * 2.0)][d % 4];
        c.fill(arm, col);
    }
}

/// Os quatro botões de face em losango (Y em cima, X à esquerda, B à direita, A embaixo), para
/// mostrar que eles escolhem no círculo.
pub fn face(c: &Canvas, cx: f32, cy: f32, h: f32, s: Style, col: Color) {
    let (g, off) = (h * 0.42, h * 0.3);
    for (bit, dx, dy) in [(pads::Y, 0.0, -off), (pads::X, -off, 0.0), (pads::B, off, 0.0), (pads::A, 0.0, off)] {
        draw(c, cx + dx - g / 2.0, cy + dy, g, s, bit, col);
    }
}

/// Só no teste (`--test-page=glyphs`): todos os ícones de cada estilo, em dois tamanhos.
#[cfg(debug_assertions)]
pub static SHEET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(debug_assertions)]
pub fn sheet(c: &Canvas, r: Rect, col: Color) {
    use pads::*;
    let bits = [A, B, X, Y, LB, RB, remap::LT, remap::RT, LS, RS, UP, RIGHT, DOWN, LEFT, BACK, START, GUIDE, PAD, MUTE, remap::LSU, remap::RSR];
    let mut y = r.y + 20.0;
    for (s, name) in [(Style::Xbox, "Xbox"), (Style::PlayStation, "PlayStation"), (Style::Nintendo, "Nintendo"), (Style::Numbers, "Genérico")] {
        c.text_raw(rect(r.x, y - 10.0, 120.0, 20.0), name, 13.0, col, Align::Left, Weight::Semibold);
        for (h, dy) in [(24.0, 0.0), (16.0, 36.0)] {
            let mut x = r.x + 110.0;
            for &b in &bits {
                x += draw(c, x, y + dy, h, s, b, col) + h * 0.3;
            }
            face(c, x + h, y + dy, h * 1.6, s, col);
        }
        y += 92.0;
    }
    for d in 0..4 {
        dpad(c, r.x + 130.0 + d as f32 * 50.0, y, 30.0, Some(d), col);
    }
}

/// Uma linha de dicas centrada em `r`: para cada item, os ícones dos botões e o texto. Diminui
/// tudo junto quando não cabe.
pub fn hints(c: &Canvas, r: Rect, s: Style, items: &[(Vec<u32>, String)], size: f32, col: Color) {
    let line_w = |k: f32| -> f32 {
        let gap = 16.0 * k;
        let h = size * 1.45 * k;
        items.iter().map(|(bits, t)| bits.iter().map(|&b| width(b, h) + 3.0 * k).sum::<f32>() + 3.0 * k + measure_raw(t, size * k, Weight::Regular)).sum::<f32>() + gap * items.len().saturating_sub(1) as f32
    };
    let mut k = 1.0;
    while k > 0.7 && line_w(k) > r.w {
        k -= 0.05;
    }
    let (h, gap, ts) = (size * 1.45 * k, 16.0 * k, size * k);
    let mut x = r.x + (r.w - line_w(k)).max(0.0) / 2.0;
    let cy = r.y + r.h / 2.0;
    for (bits, t) in items {
        for &b in bits.iter() {
            x += draw(c, x, cy, h, s, b, col) + 3.0 * k;
        }
        x += 3.0 * k;
        let tw = measure_raw(t, ts, Weight::Regular);
        c.text_raw(rect(x, r.y, tw + 1.0, r.h), t, ts, col, Align::Left, Weight::Regular);
        x += tw + gap;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_follows_the_controller_and_nintendo_swaps_letters_by_position() {
        assert_eq!(Style::of(Family::DualSense, 0x054c), Style::PlayStation);
        assert_eq!(Style::of(Family::Xbox, 0x045e), Style::Xbox);
        assert_eq!(Style::of(Family::Generic, 0x057e), Style::Nintendo);
        assert_eq!(Style::of(Family::Generic, 0x1234), Style::Numbers);
        // O bit A é o botão de baixo: no Nintendo ele se chama B; o da direita (bit B) é o A.
        assert_eq!(face_text(Style::Nintendo, pads::A), "B");
        assert_eq!(face_text(Style::Nintendo, pads::B), "A");
        assert_eq!(face_text(Style::Nintendo, pads::Y), "X");
        assert_eq!(face_text(Style::Xbox, pads::Y), "Y");
        assert_eq!(side_text(Style::Nintendo, remap::RT), "ZR");
        assert_eq!(side_text(Style::PlayStation, pads::LB), "L1");
    }
}
