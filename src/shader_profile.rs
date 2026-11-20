//! Perfil de shaders por jogo: o formato que o app (página Shaders) e o motor (`--shader-profile`) dividem.
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

#[cfg(test)]
mod tests {
    use super::*;

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
