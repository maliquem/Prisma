//! Perfil de shaders por jogo: o formato fica em `crates/prisma-shader` (o motor por cima e a DLL de injeção
//! leem o mesmo); aqui ficam as pastas do Prisma e os atalhos do motor (`teclas.txt`), que só o app e o motor usam.

use std::path::{Path, PathBuf};

pub use prisma_shader::profile::*;

use crate::config::Hotkey;

pub fn root() -> PathBuf {
    crate::config::data_dir().join("shaders")
}
/// O slang-shaders inteiro, com o nome que o RetroArch usa: presets dele gravados em `shaders\` acham as passadas
/// aqui sem ajuste, e os do Prisma (em `root()`) funcionam na pasta `shaders\` do RetroArch.
pub fn lib_dir() -> PathBuf {
    root().join(LIB)
}
const LIB: &str = "shaders_slang";
pub fn profiles_dir() -> PathBuf {
    root().join("perfis")
}
pub fn keys_path() -> PathBuf {
    root().join("teclas.txt")
}

/// `perfis\<exe em minúsculas, sem .exe>.txt`
pub fn path_for(exe: &Path) -> PathBuf {
    let stem = exe.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    profiles_dir().join(format!("{stem}.txt"))
}

/// Atalhos do motor (`teclas.txt`): `nome=mods,vk`, o mesmo texto do `config::Hotkey`. O app escreve, o motor lê.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keys {
    pub toggle: Hotkey,
    pub menu: Hotkey,
    pub next: Hotkey,
    pub compare: Hotkey,
}

impl Default for Keys {
    fn default() -> Self {
        let [toggle, menu, next, compare] = crate::config::Config::default().hk_shader;
        Self { toggle, menu, next, compare }
    }
}

impl Keys {
    pub fn parse(text: &str) -> Self {
        let mut k = Self::default();
        for (name, v) in text.lines().filter_map(|l| l.trim().split_once('=')) {
            let slot = match name {
                "toggle" => &mut k.toggle,
                "menu" => &mut k.menu,
                "next" => &mut k.next,
                "compare" => &mut k.compare,
                _ => continue,
            };
            let mut it = v.split(',').map(|x| x.trim().parse::<u32>());
            if let (Some(Ok(mods)), Some(Ok(vk))) = (it.next(), it.next()) {
                *slot = Hotkey::new(mods, vk);
            }
        }
        k
    }

    pub fn text(&self) -> String {
        let h = |k: Hotkey| format!("{},{}", k.mods, k.vk);
        format!("toggle={}
menu={}
next={}
compare={}
", h(self.toggle), h(self.menu), h(self.next), h(self.compare))
    }

    /// Sem arquivo, os padrões do config (Ctrl+Alt+H/M/D/C; P e A são as fotos do Prisma, S costuma ser atalho de programa).
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::config::write_safe(path, self.text())
    }
}

/// Copia um .slangp de qualquer lugar para `root` com os caminhos (`shaderN`, texturas, `#reference`) apontando
/// para a biblioteca do Prisma: o que passa por `shaders_slang/` (relativo, absoluto ou com o `:` do RetroArch)
/// vira `shaders_slang/<resto>`; sem esse marcador, acha pelo fim do caminho na biblioteca; senão fica o arquivo
/// original, absoluto. Devolve o preset novo e os caminhos que não existem.
pub fn import(src: &Path, root: &Path) -> std::io::Result<(PathBuf, Vec<String>)> {
    let text = String::from_utf8_lossy(&std::fs::read(src)?).into_owned();
    let from = src.parent().unwrap_or(Path::new(""));
    let unq = |v: &str| v.trim().trim_matches('"').to_string();
    let textures: Vec<String> = text
        .lines()
        .find_map(|l| l.split_once('=').filter(|x| x.0.trim() == "textures").map(|x| unq(x.1).split(';').map(|t| t.trim().to_string()).collect()))
        .unwrap_or_default();
    let (mut files, mut missing, mut out) = (None, Vec::new(), String::new());
    for line in text.lines() {
        let t = line.trim();
        let (head, val) = match t.strip_prefix("#reference") {
            Some(v) => ("#reference ".to_string(), v),
            None => match t.split_once('=') {
                Some((k, v)) if k.trim().strip_prefix("shader").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) || textures.iter().any(|x| x == k.trim()) => (format!("{} = ", k.trim()), v),
                _ => {
                    out += line;
                    out.push('\n');
                    continue;
                }
            },
        };
        let old = unq(val);
        let new = relink(&old, from, &root.join(LIB), &mut files);
        if !root.join(&new).exists() && !missing.contains(&old) {
            missing.push(old);
        }
        out += &format!("{head}\"{new}\"\n");
    }
    std::fs::create_dir_all(root)?;
    // Um preset que já está em `root` é reescrito no lugar; um de fora não toma o lugar de outro de mesmo nome.
    let here = src.parent().and_then(|d| std::fs::canonicalize(d).ok()) == std::fs::canonicalize(root).ok();
    let stem = src.file_stem().map_or("importado".into(), |s| s.to_string_lossy());
    let to = if here { root.join(format!("{stem}.slangp")) } else { free_name(root, &stem) };
    crate::config::write_safe(&to, out)?;
    Ok((to, missing))
}

/// `dir/nome.slangp`, ou `nome (2).slangp`, `(3)`… se já existir: nunca grava por cima de um preset do jogador.
pub fn free_name(dir: &Path, stem: &str) -> PathBuf {
    (1..).map(|i| dir.join(if i == 1 { format!("{stem}.slangp") } else { format!("{stem} ({i}).slangp") })).find(|p| !p.exists()).unwrap()
}

/// Um caminho do preset importado, visto de `root` (veja `import`). `files` é a biblioteca toda, lida só se
/// algum caminho vier sem o marcador.
fn relink(old: &str, from: &Path, lib: &Path, files: &mut Option<Vec<PathBuf>>) -> String {
    let p = old.replace('\\', "/");
    if let Some(i) = p.to_ascii_lowercase().find("shaders_slang/") {
        return format!("{LIB}/{}", &p[i + "shaders_slang/".len()..]);
    }
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let parts: Vec<&str> = p.split('/').filter(|c| !c.is_empty() && *c != "." && *c != "..").collect();
    // shortcut: dois arquivos com o mesmo fim de caminho ficam com o último achado; o slang-shaders não repete.
    let best = files
        .get_or_insert_with(|| {
            let mut v = Vec::new();
            walk(lib, &mut v);
            v
        })
        .iter()
        .filter_map(|f| {
            let r = f.strip_prefix(lib).ok()?;
            let n = r.components().rev().zip(parts.iter().rev()).take_while(|(a, b)| a.as_os_str().to_string_lossy().eq_ignore_ascii_case(b)).count();
            (n > 0).then_some((n, r))
        })
        .max_by_key(|x| x.0);
    let lib_path = |r: &Path| format!("{LIB}/{}", r.to_string_lossy().replace('\\', "/"));
    // Pasta e arquivo batendo é a biblioteca; só o nome do arquivo perde para o arquivo ao lado do preset.
    if let Some((n, r)) = best {
        if n >= 2 {
            return lib_path(r);
        }
    }
    let f = from.join(old);
    if f.exists() {
        return std::path::absolute(&f).unwrap_or(f).display().to_string();
    }
    best.map_or_else(|| old.to_string(), |(_, r)| lib_path(r))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Os três jeitos do RetroArch (relativo ao preset, absoluto e com o `:`), no `shaderN`, na textura e no
    /// `#reference`; sem o marcador, acha pelo fim do caminho; o que não existe volta na lista.
    #[test]
    fn importa_preset_do_retroarch() {
        let d = std::env::temp_dir().join(format!("prisma-shd-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let root = d.join("shaders");
        for f in ["crt/shaders/a.slang", "crt/crt-x.slangp", "crt/frame.png"] {
            let p = root.join(LIB).join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let ra = d.join("RetroArch").join("config").join("Snes9x");
        std::fs::create_dir_all(&ra).unwrap();
        std::fs::write(ra.join("mine.slang"), "").unwrap();
        // Mesmo nome de um da biblioteca, ao lado do preset: fica o de perto.
        std::fs::write(ra.join("a.slang"), "").unwrap();
        let src = ra.join("Snes9x.slangp");
        std::fs::write(
            &src,
            "#reference \":/shaders/shaders_slang/crt/crt-x.slangp\"\nshaders = 6\n\
             shader0 = \"../../shaders/shaders_slang/crt/shaders/a.slang\"\n\
             shader1 = \"C:\\RetroArch\\shaders\\shaders_slang\\crt\\shaders\\a.slang\"\n\
             shader2 = \"outro/lugar/crt/shaders/a.slang\"\nshader3 = mine.slang\nshader4 = \"sumiu.slang\"\nshader5 = a.slang\n\
             alias0 = \"Reference\"\ntextures = \"frame;gone\"\nframe = \":/shaders/shaders_slang/crt/frame.png\"\n\
             frame_linear = true\ngone = \"nada/sumiu.png\"\nwarpX = \"0.000000\"\n",
        )
        .unwrap();
        let (to, missing) = import(&src, &root).unwrap();
        assert_eq!(to, root.join("Snes9x.slangp"));
        let got = std::fs::read_to_string(&to).unwrap();
        let mine = std::path::absolute(ra.join("mine.slang")).unwrap();
        for l in [
            "#reference \"shaders_slang/crt/crt-x.slangp\"",
            "shaders = 6",
            "shader0 = \"shaders_slang/crt/shaders/a.slang\"",
            "shader1 = \"shaders_slang/crt/shaders/a.slang\"",
            "shader2 = \"shaders_slang/crt/shaders/a.slang\"",
            &format!("shader3 = \"{}\"", mine.display()),
            &format!("shader5 = \"{}\"", std::path::absolute(ra.join("a.slang")).unwrap().display()),
            "alias0 = \"Reference\"",
            "frame = \"shaders_slang/crt/frame.png\"",
            "frame_linear = true",
            "warpX = \"0.000000\"",
        ] {
            assert!(got.lines().any(|x| x == l), "{l}\n{got}");
        }
        assert_eq!(missing, ["sumiu.slang", "nada/sumiu.png"]);
        // De novo: o primeiro fica, o novo ganha outro nome; o que já está em `root` é reescrito no lugar.
        assert_eq!(import(&src, &root).unwrap().0, root.join("Snes9x (2).slangp"));
        assert_eq!(import(&to, &root).unwrap().0, to);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Presets de verdade do RetroArch (só lidos), ida e volta: importa, salva como preset, importa o salvo e
    /// confere a mesma cadeia e os mesmos valores.
    /// `PRISMA_RA=X:\RetroArch-Win64 PRISMA_SHD_ROOT=<dados>\shaders cargo test retroarch -- --ignored`
    #[test]
    #[ignore]
    fn importa_do_retroarch_e_volta() {
        use prisma_shader::chain::{join, slangp};
        let ra = PathBuf::from(std::env::var("PRISMA_RA").expect("PRISMA_RA"));
        let root = PathBuf::from(std::env::var("PRISMA_SHD_ROOT").expect("PRISMA_SHD_ROOT"));
        for f in [r"config\ScummVM\ScummVM.slangp", r"config\Snes9x\Snes9x.slangp"] {
            let (to, missing) = import(&ra.join(f), &root).unwrap();
            assert!(missing.is_empty(), "{f}: {missing:?}");
            let mut p = Profile { name: format!("{}-volta", to.file_stem().unwrap().to_string_lossy()), ..Profile::default() };
            add_pass(&mut p, to.clone());
            assert!(!p.params.is_empty(), "{f}");
            let text = slangp(&p, &root).unwrap();
            assert!(!text.contains(":\\") && !text.contains(":/"), "{f}: caminho absoluto\n{text}");
            let out = root.join(format!("{}.slangp", p.name));
            std::fs::write(&out, text).unwrap();
            let (back, missing) = import(&out, &root).unwrap();
            assert!(missing.is_empty(), "{f}: {missing:?}");
            let mut q = Profile::default();
            add_pass(&mut q, back.clone());
            assert_eq!(q.params, p.params, "{f}");
            let (a, b) = (join(&[to]).unwrap(), join(&[back]).unwrap());
            // `alias = ""` do original sai sem alias: o mesmo para o librashader.
            let meta = |x: &librashader::presets::ShaderPreset| {
                let alias = |p: &librashader::presets::PassConfig| p.meta.alias.as_ref().map(|a| a.trim_matches('"').to_string()).filter(|a| !a.is_empty());
                let m = |p: &librashader::presets::PassConfig| (p.meta.filter, p.meta.wrap_mode, p.meta.frame_count_mod, p.meta.srgb_framebuffer, p.meta.float_framebuffer, p.meta.mipmap_input);
                format!("{:?}", x.passes.iter().map(|p| (&p.path, alias(p), m(p), &p.meta.scaling)).collect::<Vec<_>>())
            };
            assert_eq!(meta(&a), meta(&b), "{f}");
            assert_eq!(a.textures.iter().map(|t| &t.path).collect::<Vec<_>>(), b.textures.iter().map(|t| &t.path).collect::<Vec<_>>(), "{f}");
            eprintln!("{f}: {} passadas, {} texturas, {} valores", a.passes.len(), a.textures.len(), p.params.len());
        }
    }

    #[test]
    fn teclas() {
        let k = Keys::parse("menu=2,80
compare=lixo
");
        assert_eq!(k.menu, Hotkey::new(2, 80));
        assert_eq!(k.compare, Keys::default().compare);
        assert_eq!(Keys::parse(&k.text()), k);
    }
}
