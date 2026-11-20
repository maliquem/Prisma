//! Perfil de shaders por jogo: o formato que o app (página Shaders), o motor (`--shader-profile`) e a DLL de
//! injeção dividem.
//! Contrato completo em research_notes/Shaders/implementacao.md.

use std::path::{Path, PathBuf};

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
    /// `mode=inject`: dentro do jogo, no Present; `mode=overlay` (padrão): janela por cima.
    pub inject: bool,
    /// API de vídeo que o motor viu no jogo (dx11, d3d9, dx12, vulkan, opengl); vazio antes de ver.
    pub api: String,
    /// Por que o motor tirou a Injeção (api, dll, frames, crash), para a página dizer; vazio se não tirou.
    pub fallback: String,
    pub passes: Vec<Pass>,
    /// Só os valores diferentes do padrão, na ordem em que foram gravados.
    pub params: Vec<(String, f32)>,
    /// Linhas que esta versão não conhece: voltam intactas ao gravar.
    pub extra: Vec<String>,
}

impl Default for Profile {
    fn default() -> Self {
        Self { name: String::new(), exe: PathBuf::new(), on: true, input: "auto".into(), record: false, inject: false, api: String::new(), fallback: String::new(), passes: vec![], params: vec![], extra: vec![] }
    }
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
                "mode" => p.inject = v == "inject",
                "api" => p.api = v.to_string(),
                "fallback" => p.fallback = v.to_string(),
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
        let mode = if self.inject { "inject" } else { "overlay" };
        let mut s = format!("name={}\nexe={}\non={}\ninput={}\nrecord={}\nmode={mode}\n", self.name, self.exe.display(), b(self.on), self.input, b(self.record));
        if !self.api.is_empty() {
            s += &format!("api={}\n", self.api);
        }
        if !self.fallback.is_empty() {
            s += &format!("fallback={}\n", self.fallback);
        }
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
        crate::write_safe(path, self.text())
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
/// do próprio shader (`#pragma parameter`): os valores de um preset entram no perfil ao carregá-lo (`add_pass`),
/// como mudados. Nome repetido em dois passes fica só no primeiro (um valor só na cadeia). Passe que não abre fica
/// de fora.
pub fn params(profile: &Profile) -> Vec<Param> {
    use librashader::preprocess::ShaderSource;
    use librashader::presets::{ShaderFeatures, ShaderPreset};
    let mut out: Vec<Param> = Vec::new();
    for (i, pass) in profile.passes.iter().enumerate() {
        let Ok(preset) = ShaderPreset::try_parse(&pass.path, ShaderFeatures::NONE) else { continue };
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
                out.push(Param { name: p.id.to_string(), desc: p.description, default: p.initial, min: p.minimum, max: p.maximum, step: p.step, pass: i });
            }
        }
    }
    out
}

/// Põe o preset no fim dos passes, com os valores dele (`parameters`, `#reference`) no perfil, como mudados: o
/// "Restaurar padrão" volta ao valor do shader. Valor já no perfil com o mesmo nome dá lugar ao do preset novo.
pub fn add_pass(p: &mut Profile, path: PathBuf) {
    insert_pass(p, p.passes.len(), path);
}

/// `add_pass` na posição `at` (o menu no jogo põe depois do passe escolhido); além do fim vai para o fim.
pub fn insert_pass(p: &mut Profile, at: usize, path: PathBuf) {
    use librashader::presets::{ShaderFeatures, ShaderPreset};
    if let Ok(preset) = ShaderPreset::try_parse(&path, ShaderFeatures::NONE) {
        for o in &preset.parameters {
            p.set_param(&o.name, Some(o.value));
        }
    }
    p.passes.insert(at.min(p.passes.len()), Pass { on: true, path });
}

/// Tira o passe `i` e os valores de parâmetro que eram só dele (outro passe com o mesmo nome segura o valor).
pub fn remove_pass(p: &mut Profile, i: usize) {
    let before = params(p);
    p.passes.remove(i);
    let after = params(p);
    p.params.retain(|(n, _)| !before.iter().any(|x| x.pass == i && &x.name == n) || after.iter().any(|x| &x.name == n));
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let t = "name=Castlevania\nexe=C:\\Jogos\\game.exe\non=1\ninput=auto\nrecord=0\nmode=inject\napi=dx11\nfallback=frames\npass=1|C:\\lib\\ntsc\\a.slangp\npass=0|C:\\lib\\crt\\b.slangp\nparam=hardScan=-14\nfuturo=x\n";
        let p = Profile::parse(t);
        assert_eq!(p.passes.len(), 2);
        assert!(p.inject && p.api == "dx11" && p.fallback == "frames" && !Profile::default().inject);
        assert!(!p.passes[1].on);
        assert_eq!(p.param("hardScan"), Some(-14.0));
        assert_eq!(p.extra, vec!["futuro=x".to_string()]);
        assert_eq!(Profile::parse(&p.text()), p);
    }

    #[test]
    fn insere_e_tira_passe() {
        let mut p = Profile::default();
        add_pass(&mut p, "a.slangp".into());
        add_pass(&mut p, "c.slangp".into());
        insert_pass(&mut p, 1, "b.slangp".into());
        insert_pass(&mut p, 99, "d.slangp".into());
        p.set_param("x", Some(1.0));
        let names = |p: &Profile| p.passes.iter().map(|x| x.path.to_string_lossy().to_string()).collect::<Vec<_>>();
        assert_eq!(names(&p), ["a.slangp", "b.slangp", "c.slangp", "d.slangp"]);
        remove_pass(&mut p, 1);
        assert_eq!(names(&p), ["a.slangp", "c.slangp", "d.slangp"]);
        // Valor que nenhum passe lido declara fica (o preset pode só não abrir agora).
        assert_eq!(p.param("x"), Some(1.0));
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
