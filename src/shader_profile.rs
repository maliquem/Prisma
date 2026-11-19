//! Perfil de shaders por jogo: o formato fica em `crates/prisma-shader` (o motor por cima e a DLL de injeção
//! leem o mesmo); aqui ficam as pastas do Prisma e os atalhos do motor (`teclas.txt`), que só o app e o motor usam.

use std::path::{Path, PathBuf};

pub use prisma_shader::profile::*;

use crate::config::Hotkey;

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
}
