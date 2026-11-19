//! Resolução de entrada: jogo de pixels grandes (240p esticado para 1080p) vira a imagem pequena de antes,
//! com um ponto do centro de cada bloco, que é o que os shaders de CRT esperam.

use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::{say, Res};

/// Tamanho dos blocos e onde o primeiro começa, em pixels, em cada eixo. Eixo sem blocos fica 1×0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blocks {
    pub bx: u32,
    pub by: u32,
    pub ox: u32,
    pub oy: u32,
}

/// Tamanho e começo dos blocos numa fileira de `n` posições; `edge(i)`: a posição i difere da i-1.
/// Vale o maior tamanho (2 a 16) em que quase todas as bordas caem no mesmo resto; bordas espalhadas dão
/// (1, 0), imagem na resolução da tela. `None`: bordas de menos para dizer (tela lisa, fade).
pub fn block(n: usize, edge: impl Fn(usize) -> bool) -> Option<(u32, u32)> {
    let e: Vec<usize> = (1..n).filter(|&i| edge(i)).collect();
    if e.len() < 8 {
        return None;
    }
    (2..=16usize).rev().find_map(|g| {
        let mut count = [0usize; 16];
        for &x in &e {
            count[x % g] += 1;
        }
        let (r, best) = count.iter().enumerate().max_by_key(|c| *c.1).unwrap();
        // Um texto do jogo na resolução da tela, ou um cursor, deixa algumas bordas fora do lugar.
        (best * 10 >= e.len() * 9).then_some((g as u32, r as u32))
    })
    .or(Some((1, 0)))
}

/// Blocos de um quadro a partir de duas faixas: `hs` (largura `w`, `s` linhas) para as colunas e `vs`
/// (`s` colunas, altura `h`) para as linhas, em BGRA juntinho.
pub fn detect(hs: &[u32], w: usize, vs: &[u32], h: usize, s: usize) -> Option<Blocks> {
    let x = block(w, |i| (0..s).any(|y| hs[y * w + i] != hs[y * w + i - 1]));
    let y = block(h, |i| (0..s).any(|x| vs[i * s + x] != vs[(i - 1) * s + x]));
    if x.is_none() && y.is_none() {
        return None;
    }
    let ((bx, ox), (by, oy)) = (x.unwrap_or((1, 0)), y.unwrap_or((1, 0)));
    Some(Blocks { bx, by, ox, oy })
}

/// Constantes do `PS_REDUCE` (bloco x, bloco y, começo x, começo y), ou `None` para a imagem inteira.
/// `auto` usa os blocos achados (sem blocos, a original); 240/480/720 reduzem à altura, mantendo a proporção;
/// d2/d3/d4 dividem.
pub fn reduce_for(input: &str, h: u32, found: Option<Blocks>) -> Option<[f32; 4]> {
    match input {
        "d2" | "d3" | "d4" => {
            let b = input[1..].parse::<f32>().ok()?;
            Some([b, b, 0.0, 0.0])
        }
        "240" | "480" | "720" => {
            let b = h as f32 / input.parse::<f32>().ok()?;
            (b > 1.0).then_some([b, b, 0.0, 0.0])
        }
        "auto" => found.map(|f| [f.bx as f32, f.by as f32, f.ox as f32, f.oy as f32]),
        _ => None,
    }
}

pub fn reduced_size(w: u32, h: u32, k: [f32; 4]) -> (u32, u32) {
    (((w as f32 - k[2]) / k[0]).round().max(1.0) as u32, ((h as f32 - k[3]) / k[1]).round().max(1.0) as u32)
}

/// Linhas lidas em cada faixa.
const STRIP: u32 = 32;

/// Detecção contínua: copia duas faixas do meio para a CPU, lê no quadro seguinte (sem esperar a GPU) e troca
/// o que vale só com o mesmo resultado 3 vezes seguidas, com uns quadros entre as leituras. Depois de
/// confirmar, confere de novo a cada ~300 quadros: o jogo muda de escala entre menu e jogo.
#[derive(Default)]
pub struct Detect {
    staging: Option<((u32, u32), ID3D11Texture2D, ID3D11Texture2D)>,
    pending: bool,
    votes: Vec<Blocks>,
    wait: u32,
    /// Blocos maiores que 1 pixel em uso; `None` é a imagem na resolução original.
    pub found: Option<Blocks>,
}

impl Detect {
    pub fn step(&mut self, dev: &ID3D11Device, ctx: &ID3D11DeviceContext, src: &ID3D11Texture2D, w: u32, h: u32) -> Res<()> {
        if self.wait > 0 {
            self.wait -= 1;
            return Ok(());
        }
        let s = STRIP.min(w).min(h);
        if self.staging.as_ref().is_none_or(|x| x.0 != (w, h)) {
            self.staging = Some(((w, h), staging(dev, w, s)?, staging(dev, s, h)?));
            self.pending = false;
        }
        let (_, hs, vs) = self.staging.as_ref().unwrap();
        if self.pending {
            self.pending = false;
            let (a, b) = (read(ctx, hs, w, s)?, read(ctx, vs, s, h)?);
            self.vote(detect(&a, w as usize, &b, h as usize, s as usize));
            return Ok(());
        }
        let (y0, x0) = ((h - s) / 2, (w - s) / 2);
        unsafe {
            ctx.CopySubresourceRegion(hs, 0, 0, 0, 0, src, 0, Some(&D3D11_BOX { left: 0, top: y0, front: 0, right: w, bottom: y0 + s, back: 1 }));
            ctx.CopySubresourceRegion(vs, 0, 0, 0, 0, src, 0, Some(&D3D11_BOX { left: x0, top: 0, front: 0, right: x0 + s, bottom: h, back: 1 }));
        }
        self.pending = true;
        Ok(())
    }

    /// Um resultado lido. Sem resposta (tela lisa) não conta nem zera a contagem.
    fn vote(&mut self, v: Option<Blocks>) {
        self.wait = 6;
        let Some(v) = v else { return };
        if self.votes.last() != Some(&v) {
            self.votes.clear();
        }
        self.votes.push(v);
        if self.votes.len() < 3 {
            return;
        }
        self.votes.clear();
        self.wait = 300;
        let new = (v.bx > 1 || v.by > 1).then_some(v);
        if new != self.found {
            self.found = new;
            match new {
                Some(b) => say!("shader: blocos {}x{} (começo {},{})", b.bx, b.by, b.ox, b.oy),
                None => say!("shader: sem blocos de pixels, entrada na resolução original"),
            }
        }
    }
}

fn staging(dev: &ID3D11Device, w: u32, h: u32) -> Res<ID3D11Texture2D> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: D3D11_USAGE_STAGING,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        ..Default::default()
    };
    let mut t = None;
    unsafe { dev.CreateTexture2D(&desc, None, Some(&mut t))? };
    Ok(t.unwrap())
}

fn read(ctx: &ID3D11DeviceContext, t: &ID3D11Texture2D, w: u32, h: u32) -> Res<Vec<u32>> {
    unsafe {
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(t, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
        let mut out = Vec::with_capacity((w * h) as usize);
        for y in 0..h as usize {
            out.extend_from_slice(std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize) as *const u32, w as usize));
        }
        ctx.Unmap(t, 0);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Imagem de `bw`×`bh` blocos de cores quase aleatórias, ampliada `k` vezes com `off` pixels de borda preta.
    fn image(bw: usize, bh: usize, k: usize, off: usize) -> (Vec<u32>, usize, usize) {
        let (w, h) = (bw * k + off * 2, bh * k + off * 2);
        let mut px = vec![0u32; w * h];
        for y in off..off + bh * k {
            for x in off..off + bw * k {
                let (i, j) = ((x - off) / k, (y - off) / k);
                px[y * w + x] = ((i * 7919 + j * 104729) % 251) as u32 * 0x010101 + 1;
            }
        }
        (px, w, h)
    }

    fn strips(px: &[u32], w: usize, h: usize, s: usize) -> (Vec<u32>, Vec<u32>) {
        let (y0, x0) = ((h - s) / 2, (w - s) / 2);
        let hs = px[y0 * w..(y0 + s) * w].to_vec();
        let vs = (0..h).flat_map(|y| px[y * w + x0..y * w + x0 + s].to_vec()).collect();
        (hs, vs)
    }

    #[test]
    fn acha_blocos() {
        for (k, off) in [(4, 0), (3, 5), (5, 2), (2, 0)] {
            let (px, w, h) = image(64, 48, k, off);
            let (hs, vs) = strips(&px, w, h, 32);
            let b = detect(&hs, w, &vs, h, 32).unwrap();
            assert_eq!((b.bx, b.by), (k as u32, k as u32), "bloco {k}");
            assert_eq!((b.ox, b.oy), ((off % k) as u32, (off % k) as u32), "começo com borda {off}");
        }
    }

    #[test]
    fn sem_blocos() {
        // Cada pixel diferente do vizinho: imagem na resolução da tela.
        let (px, w, h) = image(300, 200, 1, 0);
        let (hs, vs) = strips(&px, w, h, 32);
        assert_eq!(detect(&hs, w, &vs, h, 32), Some(Blocks { bx: 1, by: 1, ox: 0, oy: 0 }));
        // Tela lisa: sem bordas, sem resposta.
        let flat = vec![5u32; 320 * 240];
        let (hs, vs) = strips(&flat, 320, 240, 32);
        assert_eq!(detect(&hs, 320, &vs, 240, 32), None);
    }

    #[test]
    fn borda_fora_do_lugar_nao_estraga() {
        let mut e: Vec<bool> = (0..400).map(|i| i % 4 == 1).collect();
        e[2] = true;
        e[203] = true;
        assert_eq!(block(400, |i| e[i]), Some((4, 1)));
    }

    #[test]
    fn reducao() {
        let f = Some(Blocks { bx: 4, by: 4, ox: 0, oy: 0 });
        assert_eq!(reduce_for("auto", 960, f), Some([4.0, 4.0, 0.0, 0.0]));
        assert_eq!(reduce_for("auto", 960, None), None);
        assert_eq!(reduce_for("full", 960, f), None);
        assert_eq!(reduce_for("d3", 960, None), Some([3.0, 3.0, 0.0, 0.0]));
        assert_eq!(reduced_size(1280, 960, reduce_for("240", 960, None).unwrap()), (320, 240));
        assert_eq!(reduce_for("720", 720, None), None, "já está na altura");
    }

    /// Três iguais seguidos trocam; tela lisa no meio não atrapalha; outro resultado no meio zera.
    #[test]
    fn votos() {
        let (b4, one) = (Blocks { bx: 4, by: 4, ox: 0, oy: 0 }, Blocks { bx: 1, by: 1, ox: 0, oy: 0 });
        let mut d = Detect::default();
        for v in [Some(b4), None, Some(b4)] {
            d.vote(v);
        }
        assert_eq!(d.found, None);
        d.vote(Some(b4));
        assert_eq!(d.found, Some(b4));
        for v in [one, one, b4] {
            d.vote(Some(v));
        }
        assert_eq!(d.found, Some(b4));
        for _ in 0..3 {
            d.vote(Some(one));
        }
        assert_eq!(d.found, None, "voltou à resolução original");
    }
}
