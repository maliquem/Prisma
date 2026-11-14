//! Menu do botão direito do Explorer (o clássico, em "Mostrar mais opções" no Windows 11):
//! "Pesquisar aqui" e "Espaço em disco" numa pasta, no fundo de uma pasta e numa unidade.
//! Só chaves em HKCU\Software\Classes, sem administrador; desligar apaga as chaves.

use windows::core::*;
use windows::Win32::System::Registry::*;

const BASES: [&str; 3] = ["Directory", "Directory\\Background", "Drive"];
/// Verbo, flag da linha de comando e texto do menu.
const VERBS: [(&str, &str, &str); 2] = [
    ("PrismaSearch", "--search=", "Pesquisar aqui com o Prisma"),
    ("PrismaSpace", "--space=", "Ver o espaço em disco com o Prisma"),
];

/// O que o menu pede: abrir a página Pesquisar ou Espaço em disco nesta pasta.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Open {
    Search = 1,
    Space = 2,
}

impl Open {
    pub fn flag(self) -> &'static str {
        VERBS[self as usize - 1].1
    }
    pub fn from(n: usize) -> Option<Open> {
        match n {
            1 => Some(Open::Search),
            2 => Some(Open::Space),
            _ => None,
        }
    }
}

/// Chave (sob HKCU), nome do valor ("" é o padrão) e texto de cada valor gravado.
fn entries(exe: &str, label: impl Fn(&str) -> String) -> Vec<(String, &'static str, String)> {
    let mut out = Vec::new();
    for base in BASES {
        for (verb, flag, text) in VERBS {
            let key = format!("Software\\Classes\\{base}\\shell\\{verb}");
            out.push((key.clone(), "", label(text)));
            out.push((key.clone(), "Icon", format!("\"{exe}\",0")));
            out.push((format!("{key}\\command"), "", format!("\"{exe}\" {flag}\"%V\"")));
        }
    }
    out
}

fn verb_keys() -> impl Iterator<Item = String> {
    BASES.into_iter().flat_map(|b| VERBS.into_iter().map(move |(v, _, _)| format!("Software\\Classes\\{b}\\shell\\{v}")))
}

/// Grava (no idioma atual e com o caminho deste .exe) ou apaga as entradas.
pub fn set(on: bool) -> bool {
    for k in verb_keys() {
        let w = HSTRING::from(k);
        unsafe {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &w);
        }
    }
    if !on {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let label = |t: &str| crate::i18n::tr(t).into_owned();
    entries(&exe.display().to_string(), label).iter().all(|(k, n, v)| {
        let data: Vec<u8> = v.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        super::reg::write(super::reg::Hive::User, k, n, REG_SZ.0, &data)
    })
}

/// A pasta pedida pelo menu, se a linha de comando trouxe uma.
pub fn from_args(args: &[String]) -> Option<(Open, String)> {
    args.iter().find_map(|a| {
        let (kind, rest) = if let Some(r) = a.strip_prefix("--search=") {
            (Open::Search, r)
        } else {
            (Open::Space, a.strip_prefix("--space=")?)
        };
        // `"C:\"` chega como `C:"`: a barra antes da aspa vira aspa na linha de comando.
        let mut dir = rest.trim().trim_matches('"').trim_end_matches('\\').to_string();
        let b = dir.as_bytes();
        let ok = (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':') || dir.starts_with("\\\\");
        if !ok || dir.contains(['"', '\n', '\r']) {
            return None;
        }
        dir.push('\\');
        Some((kind, dir))
    })
}

/// O texto da pesquisa que limita à pasta: o caminho com a barra no fim, entre aspas.
pub fn search_query(dir: &str) -> String {
    format!("\"{dir}\" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &str) -> Vec<String> {
        vec!["prisma.exe".into(), a.into()]
    }

    #[test]
    fn reads_the_folder() {
        assert_eq!(from_args(&args(r"--search=C:\Users\Ana\Meus docs")), Some((Open::Search, r"C:\Users\Ana\Meus docs\".into())));
        assert_eq!(from_args(&args("--space=D:\"")), Some((Open::Space, r"D:\".into())));
        assert_eq!(from_args(&args(r"--space=\\nas\fotos\")), Some((Open::Space, r"\\nas\fotos\".into())));
        assert_eq!(from_args(&args("--search=relativo")), None);
        assert_eq!(from_args(&args("--background")), None);
        assert_eq!(search_query(r"C:\a b\"), "\"C:\\a b\\\" ");
    }

    #[test]
    fn builds_the_keys() {
        let e = entries(r"C:\Program Files\Prisma\prisma.exe", |t| t.to_string());
        assert_eq!(e.len(), 3 * 2 * 3);
        assert!(e.iter().any(|(k, n, v)| k == r"Software\Classes\Drive\shell\PrismaSpace\command"
            && n.is_empty()
            && v == r#""C:\Program Files\Prisma\prisma.exe" --space="%V""#));
        assert!(e.iter().any(|(k, n, v)| k == r"Software\Classes\Directory\Background\shell\PrismaSearch" && n.is_empty() && v == "Pesquisar aqui com o Prisma"));
        assert_eq!(verb_keys().count(), 6);
    }
}
