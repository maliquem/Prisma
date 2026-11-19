//! A cadeia de um perfil: junta os presets ligados, carrega no librashader, acompanha o arquivo do perfil e
//! desenha (redução por blocos, cadeia) numa saída qualquer. Usada pela janela por cima, pela prévia e pela
//! DLL de injeção.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use librashader::preprocess::ShaderSource;
use librashader::presets::{ShaderFeatures, ShaderPreset};
use librashader::runtime::d3d11::FilterChain;
use librashader::runtime::{FilterChainParameters, Size, Viewport};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;

use crate::blocks::{self, Detect};
use crate::profile::Profile;
use crate::{say, texture, FullPass, Res, PS_COPY, PS_REDUCE};

/// Junta os presets numa cadeia só, na ordem: as passadas uma atrás da outra (renumeradas) e as texturas sem
/// repetir nome (a primeira vence). Os valores de `parameters` saem: eles entraram no perfil ao carregar o
/// preset (`profile::add_pass`), e o que não está no perfil vale o padrão do shader. O librashader resolve o
/// `#reference` e deixa os caminhos absolutos ao ler cada preset, então não precisa de .slangp temporário.
/// Preset depois do primeiro que lê uma passada pelo número (`PassOutput1`) enxergaria a passada errada da
/// cadeia: a junção é recusada com o nome dele.
pub fn join(paths: &[PathBuf]) -> Res<ShaderPreset> {
    let mut out: Option<ShaderPreset> = None;
    for p in paths {
        let next = ShaderPreset::try_parse(p, ShaderFeatures::NONE).map_err(|e| format!("{}: {e}", p.display()))?;
        let Some(o) = &mut out else {
            out = Some(next);
            continue;
        };
        for pass in &next.passes {
            let src = ShaderSource::load(&pass.path, ShaderFeatures::NONE).map_err(|e| format!("{}: {e}", pass.path.display()))?;
            if uses_pass_number(&src.vertex) || uses_pass_number(&src.fragment) {
                return Err(format!("{}: lê uma passada pelo número (PassOutputN) e só funciona como primeiro preset da cadeia", p.display()).into());
            }
        }
        let base = o.passes.len() as i32;
        for mut pass in next.passes {
            pass.meta.id += base;
            o.passes.push(pass);
        }
        for t in next.textures {
            if !o.textures.iter().any(|x| x.meta.name == t.meta.name) {
                o.textures.push(t);
            }
        }
    }
    let mut o = out.ok_or("nenhum passe ligado")?;
    o.pass_count = o.passes.len() as i32;
    o.parameters.clear();
    Ok(o)
}

/// A cadeia ligada do perfil como um .slangp comum, com os valores do perfil: o "Salvar como preset…" da página.
/// Caminhos dentro de `base` (a pasta onde o preset fica) saem relativos e com `/`, como o RetroArch grava; os de
/// fora saem absolutos. Abre no RetroArch e de volta no Prisma como um passe só.
pub fn slangp(p: &Profile, base: &Path) -> Res<String> {
    use librashader::presets::{ScaleFactor, ScaleType};
    use librashader::{FilterMode, WrapMode};
    use std::fmt::Write;
    let c = join(&on_paths(p))?;
    let wrap = |w: WrapMode| match w {
        WrapMode::ClampToBorder => "clamp_to_border",
        WrapMode::ClampToEdge => "clamp_to_edge",
        WrapMode::Repeat => "repeat",
        WrapMode::MirroredRepeat => "mirrored_repeat",
    };
    // O librashader devolve os caminhos canônicos (`\\?\C:\...`), que o RetroArch não lê e o strip_prefix não casa.
    let plain = |f: &Path| {
        let s = std::path::absolute(f).unwrap_or_else(|_| f.to_path_buf()).to_string_lossy().into_owned();
        PathBuf::from(s.strip_prefix(r"\\?\UNC\").map(|x| format!(r"\\{x}")).or_else(|| s.strip_prefix(r"\\?\").map(str::to_string)).unwrap_or(s))
    };
    let base = &plain(base);
    let at = |f: &Path| {
        let f = plain(f);
        f.strip_prefix(base).map_or_else(|_| f.display().to_string(), |r| r.to_string_lossy().replace('\\', "/"))
    };
    let mut s = format!("# Prisma: {}\nshaders = {}\n", p.name, c.passes.len());
    for (i, pass) in c.passes.iter().enumerate() {
        let m = &pass.meta;
        let _ = writeln!(s, "shader{i} = \"{}\"", at(&pass.path));
        // `alias3 = ""` no original chega aqui com as aspas: escrito de novo, ganharia mais um par a cada volta.
        if let Some(a) = m.alias.as_ref().map(|a| a.trim_matches('"')).filter(|a| !a.is_empty()) {
            let _ = writeln!(s, "alias{i} = \"{a}\"");
        }
        let _ = writeln!(s, "filter_linear{i} = {}", m.filter == FilterMode::Linear);
        let _ = writeln!(s, "wrap_mode{i} = {}", wrap(m.wrap_mode));
        if m.frame_count_mod != 0 {
            let _ = writeln!(s, "frame_count_mod{i} = {}", m.frame_count_mod);
        }
        let _ = writeln!(s, "srgb_framebuffer{i} = {}\nfloat_framebuffer{i} = {}\nmipmap_input{i} = {}", m.srgb_framebuffer, m.float_framebuffer, m.mipmap_input);
        // Sem escala no original fica sem escala: o padrão do librashader (e do RetroArch) para cada passada.
        if m.scaling.valid {
            for (axis, sc) in [("x", &m.scaling.x), ("y", &m.scaling.y)] {
                let kind = match sc.scale_type {
                    ScaleType::Input => "source",
                    ScaleType::Absolute => "absolute",
                    ScaleType::Viewport => "viewport",
                    ScaleType::Original => "original",
                };
                let f = match sc.factor {
                    ScaleFactor::Float(f) => f.to_string(),
                    ScaleFactor::Absolute(n) => n.to_string(),
                };
                let _ = writeln!(s, "scale_type_{axis}{i} = {kind}\nscale_{axis}{i} = {f}");
            }
        }
    }
    if !c.textures.is_empty() {
        let names: Vec<&str> = c.textures.iter().map(|t| t.meta.name.as_ref()).collect();
        let _ = writeln!(s, "textures = \"{}\"", names.join(";"));
        for t in &c.textures {
            let n = &t.meta.name;
            let _ = writeln!(s, "{n} = \"{}\"\n{n}_linear = {}\n{n}_wrap_mode = {}\n{n}_mipmap = {}", at(&t.path), t.meta.filter_mode == FilterMode::Linear, wrap(t.meta.wrap_mode), t.meta.mipmap);
        }
    }
    let vals = &p.params;
    if !vals.is_empty() {
        let _ = writeln!(s, "parameters = \"{}\"", vals.iter().map(|x| x.0.as_str()).collect::<Vec<_>>().join(";"));
        for (n, v) in vals {
            let _ = writeln!(s, "{n} = \"{v}\"");
        }
    }
    Ok(s)
}

/// O shader lê a saída de uma passada pelo número: `PassOutput1`, `PassFeedbackSize0`...
fn uses_pass_number(text: &str) -> bool {
    ["PassOutput", "PassFeedback"].iter().any(|k| text.match_indices(k).any(|(i, _)| text[i + k.len()..].trim_start_matches("Size").starts_with(|c: char| c.is_ascii_digit())))
}

fn on_paths(p: &Profile) -> Vec<PathBuf> {
    p.passes.iter().filter(|x| x.on).map(|x| x.path.clone()).collect()
}

/// O arquivo do perfil acompanhado: relê quando a data muda. Fica fora do `Engine` porque a DLL de injeção
/// também acompanha o perfil no D3D9, com a cadeia D3D9 do librashader.
pub struct Tracker {
    pub path: PathBuf,
    stamp: Option<SystemTime>,
    pub profile: Profile,
}

/// O que mudou no perfil relido.
#[derive(Default, Debug, PartialEq)]
pub struct Changed {
    /// Passes ligados: refazer a cadeia.
    pub chain: bool,
    /// Só valores: aplicar sem recarregar.
    pub params: bool,
    pub input: bool,
}

impl Tracker {
    pub fn open(path: &Path) -> Res<Self> {
        let profile = Profile::load(path).map_err(|err| format!("{}: {err}", path.display()))?;
        Ok(Self { path: path.to_path_buf(), stamp: stamp(path), profile })
    }

    /// Relê o perfil se o arquivo mudou. Arquivo sumido ou ilegível fica com o perfil de antes.
    pub fn poll(&mut self) -> Changed {
        let now = stamp(&self.path);
        if now == self.stamp {
            return Changed::default();
        }
        self.stamp = now;
        let Ok(new) = Profile::load(&self.path) else { return Changed::default() };
        let old = std::mem::replace(&mut self.profile, new);
        Changed { chain: on_paths(&old) != on_paths(&self.profile), params: old.params != self.profile.params, input: old.input != self.profile.input }
    }

    /// Passa a acompanhar outro perfil: o próximo `poll` lê o arquivo novo.
    pub fn switch(&mut self, path: &Path) {
        self.path = path.to_path_buf();
        self.stamp = None;
    }

    /// Outro perfil salvo do mesmo jogo (`dir` +1 ou -1), se houver.
    pub fn sibling(&self, dir: isize) -> Option<PathBuf> {
        let folder = self.path.parent()?;
        let list = crate::profile::siblings(folder, &self.profile.exe);
        let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
        let i = list.iter().position(|p| name(p) == name(&self.path)).map_or(0, |i| i as isize);
        let p = list.get((i + dir).rem_euclid(list.len().max(1) as isize) as usize)?;
        (name(p) != name(&self.path)).then(|| p.clone())
    }

    pub fn on_paths(&self) -> Vec<PathBuf> {
        on_paths(&self.profile)
    }
}

/// Valores padrão de todos os parâmetros de uma cadeia carregada (D3D11 ou D3D9).
pub fn defaults(c: &impl FilterChainParameters) -> HashMap<String, f32> {
    c.parameters().parameters().iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

/// Os valores do perfil na cadeia; o que sumiu do perfil volta ao padrão.
pub fn apply(c: &impl FilterChainParameters, defaults: &HashMap<String, f32>, p: &Profile) {
    for (k, d) in defaults {
        c.parameters().set_parameter_value(k, p.param(k).unwrap_or(*d));
    }
}

pub struct Engine {
    t: Tracker,
    /// Sem cadeia: só acompanha o perfil (o menu do motor quando o efeito roda injetado no jogo).
    idle: bool,
    chain: Option<FilterChain>,
    /// Valores padrão de todos os parâmetros da cadeia, para voltar ao padrão o que sumiu do perfil.
    defaults: HashMap<String, f32>,
    detect: Detect,
    in_size: (u32, u32),
    small: Option<((u32, u32), ID3D11RenderTargetView, ID3D11ShaderResourceView)>,
    reduce: FullPass,
    copy: FullPass,
    frame: usize,
}

impl std::ops::Deref for Engine {
    type Target = Tracker;
    fn deref(&self) -> &Tracker {
        &self.t
    }
}

impl Engine {
    /// `idle`: não carrega a cadeia, só acompanha o perfil.
    pub fn open(path: &Path, dev: &ID3D11Device, idle: bool) -> Res<Self> {
        let mut e = Self {
            t: Tracker::open(path)?,
            idle,
            chain: None,
            defaults: HashMap::new(),
            detect: Detect::default(),
            in_size: (0, 0),
            small: None,
            reduce: FullPass::new(dev, PS_REDUCE)?,
            copy: FullPass::new(dev, PS_COPY)?,
            frame: 0,
        };
        e.build(dev);
        Ok(e)
    }

    /// Tem o que desenhar (algum passe ligado e carregado).
    pub fn active(&self) -> bool {
        self.chain.is_some()
    }

    /// Relê o perfil se o arquivo mudou: só `param` mudou → valores na hora; `pass` mudou → refaz a cadeia;
    /// `input` mudou → refaz a redução.
    pub fn poll(&mut self, dev: &ID3D11Device) {
        let c = self.t.poll();
        if c.chain {
            self.build(dev);
        } else if c.params {
            self.apply();
        }
        if c.input {
            self.detect = Detect::default();
        }
    }

    /// Muda o perfil e grava: o `poll` logo em seguida aplica, como se o app tivesse gravado.
    pub fn edit(&mut self, dev: &ID3D11Device, f: impl FnOnce(&mut Profile)) {
        let mut p = self.t.profile.clone();
        f(&mut p);
        if let Err(e) = p.save(&self.t.path) {
            say!("shader: não gravou {}: {e}", self.t.path.display());
        }
        self.poll(dev);
    }

    /// Troca para outro perfil salvo do mesmo jogo (`dir` +1 ou -1).
    pub fn next(&mut self, dev: &ID3D11Device, dir: isize) {
        let Some(p) = self.t.sibling(dir) else { return };
        say!("shader: perfil {}", p.display());
        self.switch(dev, &p);
    }

    /// Passa a acompanhar o perfil `path`.
    pub fn switch(&mut self, dev: &ID3D11Device, path: &Path) {
        self.t.switch(path);
        self.poll(dev);
    }

    fn build(&mut self, dev: &ID3D11Device) {
        self.chain = None;
        self.defaults.clear();
        let paths = self.t.on_paths();
        if paths.is_empty() || self.idle {
            return;
        }
        let t = std::time::Instant::now();
        match join(&paths).and_then(|p| unsafe { FilterChain::load_from_preset(p, dev, None) }.map_err(Into::into)) {
            Ok(c) => {
                self.defaults = defaults(&c);
                self.chain = Some(c);
                self.apply();
                say!("shader: cadeia de {} preset(s) carregada em {} ms", paths.len(), t.elapsed().as_millis());
            }
            Err(e) => say!("shader: {e}"),
        }
    }

    fn apply(&self) {
        if let Some(c) = &self.chain {
            apply(c, &self.defaults, &self.t.profile);
        }
    }

    /// Desenha `src` (BGRA, `w`×`h`) pela cadeia em `out`, no retângulo `vp` (x, y, largura, altura).
    /// Sem cadeia, copia a imagem como está.
    pub fn render(&mut self, dev: &ID3D11Device, ctx: &ID3D11DeviceContext, src: (&ID3D11Texture2D, &ID3D11ShaderResourceView), (w, h): (u32, u32), out: &ID3D11RenderTargetView, vp: [f32; 4]) -> Res<()> {
        if (w, h) != self.in_size {
            self.in_size = (w, h);
            self.detect = Detect::default();
        }
        let Some(chain) = &mut self.chain else {
            return self.copy.draw(ctx, src.1, out, vp, [0.0; 4]);
        };
        if self.t.profile.input == "auto" {
            self.detect.step(dev, ctx, src.0, w, h)?;
        }
        let mut input = src.1;
        if let Some(k) = blocks::reduce_for(&self.t.profile.input, h, self.detect.found) {
            let size = blocks::reduced_size(w, h, k);
            if self.small.as_ref().is_none_or(|s| s.0 != size) {
                let (t, v) = texture(dev, size.0, size.1, DXGI_FORMAT_B8G8R8A8_UNORM, true)?;
                self.small = Some((size, crate::rtv(dev, &t)?, v));
                say!("shader: entrada {}x{} → {}x{}", w, h, size.0, size.1);
            }
            let (_, rt, v) = self.small.as_ref().unwrap();
            self.reduce.draw(ctx, src.1, rt, [0.0, 0.0, size.0 as f32, size.1 as f32], k)?;
            input = v;
        }
        let viewport = Viewport { x: vp[0], y: vp[1], mvp: None, output: out, size: Size::new(vp[2] as u32, vp[3] as u32) };
        unsafe { chain.frame(None, input, &viewport, self.frame, None)? };
        self.frame += 1;
        Ok(())
    }

    /// A imagem sem efeito, para comparar.
    pub fn bypass(&self, ctx: &ID3D11DeviceContext, src: &ID3D11ShaderResourceView, out: &ID3D11RenderTargetView, vp: [f32; 4]) -> Res<()> {
        self.copy.draw(ctx, src, out, vp, [0.0; 4])
    }
}

fn stamp(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLANG: &str = "#version 450\n#pragma parameter A \"a\" 0.5 0.0 1.0 0.1\nlayout(push_constant) uniform Push { float A; } params;\nlayout(std140, set = 0, binding = 0) uniform UBO { mat4 MVP; } global;\n#pragma stage vertex\nlayout(location = 0) in vec4 Position;\nlayout(location = 1) in vec2 TexCoord;\nlayout(location = 0) out vec2 vTexCoord;\nvoid main() { gl_Position = global.MVP * Position; vTexCoord = TexCoord; }\n#pragma stage fragment\nlayout(location = 0) in vec2 vTexCoord;\nlayout(location = 0) out vec4 FragColor;\nlayout(set = 0, binding = 2) uniform sampler2D Source;\nvoid main() { FragColor = texture(Source, vTexCoord) * params.A; }\n";

    /// Dois presets em pastas diferentes, um deles por `#reference`: passadas renumeradas, caminhos absolutos
    /// relativos ao preset de cada um, textura repetida uma vez só e sem os `parameters` (eles vão para o perfil).
    #[test]
    fn junta_presets() {
        let dir = std::env::temp_dir().join(format!("prisma_junta_{}", std::process::id()));
        let (a, b) = (dir.join("a"), dir.join("b").join("sub"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("x.slang"), SLANG).unwrap();
        std::fs::write(b.join("y.slang"), SLANG).unwrap();
        std::fs::write(a.join("lut.png"), []).unwrap();
        std::fs::write(a.join("a.slangp"), "shaders = 2\nshader0 = x.slang\nshader1 = x.slang\nscale_type1 = source\nscale1 = 2.0\ntextures = LUT\nLUT = lut.png\nparameters = A\nA = 0.2\n").unwrap();
        std::fs::write(b.join("base.slangp"), "shaders = 1\nshader0 = y.slang\nalias0 = Fim\ntextures = LUT\nLUT = ../../a/lut.png\n").unwrap();
        // O preset que o jogador escolhe só aponta para o outro e muda um valor.
        std::fs::write(dir.join("b.slangp"), "#reference \"b/sub/base.slangp\"\nparameters = A\nA = 0.9\n").unwrap();

        let p = join(&[a.join("a.slangp"), dir.join("b.slangp")]).unwrap();
        assert_eq!(p.pass_count, 3);
        assert_eq!(p.passes.iter().map(|x| x.meta.id).collect::<Vec<_>>(), [0, 1, 2]);
        assert!(p.passes[2].path.ends_with("y.slang") && p.passes[2].path.is_absolute(), "{:?}", p.passes[2].path);
        assert!(p.passes[2].path.exists());
        assert_eq!(p.passes[2].meta.alias.as_deref(), Some("Fim"));
        assert_eq!(p.textures.len(), 1);
        assert!(p.parameters.is_empty());

        // Preset que lê PassOutput0 só entra como primeiro.
        std::fs::write(a.join("z.slang"), SLANG.replace("texture(Source", "texture(PassOutput0")).unwrap();
        std::fs::write(a.join("z.slangp"), "shaders = 1\nshader0 = z.slang\n").unwrap();
        let err = join(&[a.join("a.slangp"), a.join("z.slangp")]).unwrap_err().to_string();
        assert!(err.contains("z.slangp") && err.contains("PassOutputN"), "{err}");
        assert!(join(&[a.join("z.slangp"), a.join("a.slangp")]).is_ok());
        assert!(uses_pass_number("PassFeedbackSize2.x") && !uses_pass_number("PassOutput;") && !uses_pass_number("OriginalHistory1"));

        // O valor de cada preset entra no perfil ao carregar (o do último vence); a tela mostra um A só, no
        // primeiro passe, com o padrão do shader.
        let mut prof = Profile::default();
        crate::profile::add_pass(&mut prof, a.join("a.slangp"));
        assert_eq!(prof.params, [("A".to_string(), 0.2)]);
        crate::profile::add_pass(&mut prof, dir.join("b.slangp"));
        assert_eq!(prof.params, [("A".to_string(), 0.9)]);
        let ps = crate::profile::params(&prof);
        assert_eq!(ps.len(), 1);
        assert_eq!((ps[0].name.as_str(), ps[0].default, ps[0].pass, ps[0].step), ("A", 0.5, 0, 0.1));

        // Salvar como preset: a cadeia vira um .slangp com caminhos relativos que o librashader lê de volta igual,
        // com o valor do perfil.
        prof.set_param("A", Some(0.4));
        let out = dir.join("saida.slangp");
        let text = slangp(&prof, &dir).unwrap();
        assert!(text.contains("shader0 = \"a/x.slang\"") && text.contains("shader2 = \"b/sub/y.slang\"") && text.contains("LUT = \"a/lut.png\""), "{text}");
        std::fs::write(&out, text).unwrap();
        let back = ShaderPreset::try_parse(&out, ShaderFeatures::NONE).unwrap();
        assert_eq!(back.passes.iter().map(|x| &x.path).collect::<Vec<_>>(), p.passes.iter().map(|x| &x.path).collect::<Vec<_>>());
        assert_eq!(back.passes[2].meta.alias.as_deref(), Some("Fim"));
        assert!(back.passes[1].meta.scaling.valid && f32::from(back.passes[1].meta.scaling.x.factor) == 2.0);
        assert!(!back.passes[0].meta.scaling.valid);
        assert_eq!(back.textures.len(), 1);
        assert_eq!(back.parameters.iter().map(|x| (x.name.to_string(), x.value)).collect::<Vec<_>>(), [("A".to_string(), 0.4)]);

        // Tirar um passe leva junto o valor que era só dele; o que outro passe ainda usa fica.
        std::fs::write(a.join("c.slang"), SLANG.replace('A', "B")).unwrap();
        std::fs::write(a.join("c.slangp"), "shaders = 1\nshader0 = c.slang\n").unwrap();
        prof.passes.push(crate::profile::Pass { on: true, path: a.join("c.slangp") });
        prof.set_param("B", Some(0.3));
        crate::profile::remove_pass(&mut prof, 2);
        assert_eq!(prof.params, [("A".to_string(), 0.4)]);
        prof.set_param("B", Some(0.3));
        crate::profile::remove_pass(&mut prof, 1);
        assert_eq!(prof.params, [("A".to_string(), 0.4), ("B".to_string(), 0.3)]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
