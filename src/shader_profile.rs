//! Perfil de shaders por jogo: o formato que o app (página Shaders) e o motor (`--shader-profile`) dividem.
//! Contrato completo em research_notes/Shaders/implementacao.md.

use std::path::{Path, PathBuf};

use crate::config::Hotkey;

#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub on: bool,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub exe: PathBuf,
    pub on: bool,
    /// auto | full | 240 | 480 | 720 | d2 | d3 | d4
    pub input: String,
    pub record: bool,
    pub passes: Vec<Pass>,
    /// Só os valores diferentes do padrão, na ordem em que foram gravados.
    pub params: Vec<(String, f32)>,
    /// Linhas que esta versão não conhece: voltam intactas ao gravar.
    pub extra: Vec<String>,
}

impl Default for Profile {
    fn default() -> Self {
        Self { name: String::new(), exe: PathBuf::new(), on: true, input: "auto".into(), record: false, passes: vec![], params: vec![], extra: vec![] }
    }
}

pub fn root() -> PathBuf {
    crate::config::data_dir().join("shaders")
}
pub fn lib_dir() -> PathBuf {
    root().join("lib")
}
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

impl Profile {
    pub fn parse(text: &str) -> Self {
        let mut p = Profile::default();
        for line in text.lines() {
            let t = line.trim();
            let Some((k, v)) = t.split_once('=') else {
                if !t.is_empty() {
                    p.extra.push(line.to_string());
                }
                continue;
            };
            match k {
                "name" => p.name = v.to_string(),
                "exe" => p.exe = v.into(),
                "on" => p.on = v == "1",
                "input" => p.input = v.to_string(),
                "record" => p.record = v == "1",
                "pass" => match v.split_once('|') {
                    Some((on, path)) => p.passes.push(Pass { on: on == "1", path: path.into() }),
                    None => p.extra.push(line.to_string()),
                },
                "param" => match v.rsplit_once('=').and_then(|(n, x)| Some((n.to_string(), x.parse::<f32>().ok()?))) {
                    Some(nv) => p.params.push(nv),
                    None => p.extra.push(line.to_string()),
                },
                _ => p.extra.push(line.to_string()),
            }
        }
        p
    }

    pub fn text(&self) -> String {
        let b = |x: bool| if x { "1" } else { "0" };
        let mut s = format!("name={}\nexe={}\non={}\ninput={}\nrecord={}\n", self.name, self.exe.display(), b(self.on), self.input, b(self.record));
        for p in &self.passes {
            s += &format!("pass={}|{}\n", b(p.on), p.path.display());
        }
        for (n, v) in &self.params {
            s += &format!("param={n}={v}\n");
        }
        for e in &self.extra {
            s += e;
            s.push('\n');
        }
        s
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        Ok(Self::parse(&std::fs::read_to_string(path)?))
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::config::write_safe(path, self.text())
    }

    pub fn param(&self, name: &str) -> Option<f32> {
        self.params.iter().find(|(n, _)| n == name).map(|p| p.1)
    }

    /// `None` volta ao padrão (some do arquivo).
    pub fn set_param(&mut self, name: &str, v: Option<f32>) {
        self.params.retain(|(n, _)| n != name);
        if let Some(v) = v {
            self.params.push((name.to_string(), v));
        }
    }
}

/// Um parâmetro de shader para a tela: `pass` é o índice do passe no perfil onde ele aparece primeiro.
#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub name: String,
    pub desc: String,
    pub default: f32,
    pub min: f32,
    pub max: f32,
    pub step: f32,
    pub pass: usize,
}

/// Parâmetros de todos os passes do perfil (ligados ou não), na ordem em que aparecem nos shaders. O padrão é o
/// da cadeia: o `parameters` do último preset ligado que mexe nele vence, como na junção. Nome repetido em dois
/// passes fica só no primeiro (um valor só na cadeia). Passe que não abre fica de fora.
pub fn params(profile: &Profile) -> Vec<Param> {
    use librashader::preprocess::ShaderSource;
    use librashader::presets::{ShaderFeatures, ShaderPreset};
    let mut out: Vec<Param> = Vec::new();
    let mut chain_values = Vec::new();
    for (i, pass) in profile.passes.iter().enumerate() {
        let Ok(preset) = ShaderPreset::try_parse(&pass.path, ShaderFeatures::NONE) else { continue };
        if pass.on {
            chain_values.extend(preset.parameters.iter().map(|o| (o.name.to_string(), o.value)));
        }
        for sp in &preset.passes {
            let Ok(src) = ShaderSource::load(&sp.path, ShaderFeatures::NONE) else { continue };
            // O librashader guarda os parâmetros num mapa sem ordem: a posição do #pragma no texto devolve a ordem.
            let text = format!("{}
{}", src.vertex, src.fragment);
            let mut ps: Vec<_> = src.parameters.into_iter().map(|(_, v)| v).collect();
            ps.sort_by_key(|p| text.find(&format!(" {} ", p.id)).unwrap_or(usize::MAX));
            for p in ps {
                if out.iter().any(|o| o.name == p.id.as_str()) {
                    continue;
                }
                let default = preset.parameters.iter().find(|o| o.name == p.id).map_or(p.initial, |o| o.value);
                out.push(Param { name: p.id.to_string(), desc: p.description, default, min: p.minimum, max: p.maximum, step: p.step, pass: i });
            }
        }
    }
    for (name, v) in chain_values {
        if let Some(x) = out.iter_mut().find(|x| x.name == name) {
            x.default = v;
        }
    }
    out
}

/// Perfis salvos do mesmo jogo na pasta `dir` (`<exe>*.txt` com o mesmo `exe`), em ordem de nome. O `exe`
/// separa `game.txt` de `game2.txt`, que é de outro jogo.
pub fn siblings(dir: &Path, exe: &Path) -> Vec<PathBuf> {
    let stem_of = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
    let stem = stem_of(exe);
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()).is_some_and(|n| n.starts_with(&stem) && n.ends_with(".txt")))
        .filter(|p| Profile::load(p).is_ok_and(|x| stem_of(&x.exe) == stem))
        .collect();
    v.sort();
    v
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
        let ca = |c: char| Hotkey::new(crate::config::MOD_CTRL | crate::config::MOD_ALT, c as u32);
        Self { toggle: ca('S'), menu: ca('P'), next: ca('D'), compare: ca('A') }
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

    /// Sem arquivo, os padrões (Ctrl+Alt+S/P/D/A).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teclas() {
        let k = Keys::parse("menu=2,80
compare=lixo
");
        assert_eq!(k.menu, Hotkey::new(2, 80));
        assert_eq!(k.compare, Keys::default().compare);
        assert_eq!(Keys::parse(&k.text()), k);
    }

    #[test]
    fn perfis_do_mesmo_jogo() {
        let dir = std::env::temp_dir().join(format!("prisma_perfis_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (f, exe) in [("game.txt", "Game.exe"), ("game-crt.txt", "game.exe"), ("game2.txt", "Game2.exe"), ("outro.txt", "outro.exe"), ("game.tmp", "Game.exe")] {
            std::fs::write(dir.join(f), format!("exe=C:\\y\\{exe}\n")).unwrap();
        }
        let v = siblings(&dir, Path::new(r"C:\x\Game.exe"));
        let names: Vec<_> = v.iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["game-crt.txt", "game.txt"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ida_e_volta() {
        let t = "name=Castlevania\nexe=C:\\Jogos\\game.exe\non=1\ninput=auto\nrecord=0\npass=1|C:\\lib\\ntsc\\a.slangp\npass=0|C:\\lib\\crt\\b.slangp\nparam=hardScan=-14\nfuturo=x\n";
        let p = Profile::parse(t);
        assert_eq!(p.passes.len(), 2);
        assert!(!p.passes[1].on);
        assert_eq!(p.param("hardScan"), Some(-14.0));
        assert_eq!(p.extra, vec!["futuro=x".to_string()]);
        assert_eq!(Profile::parse(&p.text()), p);
    }

    #[test]
    fn param_some_quando_volta_ao_padrao() {
        let mut p = Profile::default();
        p.set_param("a", Some(1.5));
        p.set_param("a", Some(2.0));
        assert_eq!(p.params, vec![("a".to_string(), 2.0)]);
        p.set_param("a", None);
        assert!(p.params.is_empty());
    }
}
