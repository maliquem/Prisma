//! Jogos instalados pela Steam e pela Epic. Os atalhos deles no menu Iniciar abrem o iniciador
//! (`steam://rungameid/…`), não o jogo, então a lista de programas não achava quase nenhum.
//!
//! Steam: as bibliotecas estão em `steamapps\libraryfolders.vdf`, e cada jogo tem um
//! `appmanifest_*.acf` com o nome e a pasta. A Steam não diz qual é o executável, então vale o
//! maior `.exe` da pasta que não seja instalador, relatório de erro ou anti-cheat.
//! Epic: cada jogo tem um `.item` (JSON) com o nome, a pasta e o executável.

use std::path::{Path, PathBuf};

use super::catalog::Entry;

/// Partes de nome que nunca são o jogo.
const NOT_GAME: [&str; 27] = [
    "crash", "report", "redist", "vcredist", "dxsetup", "setup", "unins", "install", "helper", "easyanticheat", "eac_", "battleye", "beservice", "be_service", "prereq", "dotnet", "directx", "cefprocess", "webhelper", "update", "patcher", "python", "7z", "notification", "editor", "packager", "toolkit",
];

/// Pastas que não têm o jogo.
const SKIP_DIRS: [&str; 10] = ["_commonredist", "redist", "redistributables", "directx", "vcredist", "easyanticheat", "battleye", "__installer", "support", "engine"];

pub fn games() -> Vec<Entry> {
    let mut out = steam(&|_| true);
    out.extend(epic());
    out
}

/// Texto entre aspas do formato da Valve, com as barras desfeitas.
fn vdf_strings(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    if let Some(n) = chars.next() {
                        s.push(n);
                    }
                }
                '"' => break,
                c => s.push(c),
            }
        }
        out.push(s);
    }
    out
}

/// O valor depois da chave (as chaves e valores vêm em pares de texto).
fn vdf_value(strings: &[String], key: &str) -> Option<String> {
    strings.windows(2).find(|w| w[0].eq_ignore_ascii_case(key)).map(|w| w[1].clone())
}

fn steam_root() -> Option<PathBuf> {
    let p = super::reg::text(super::reg::Hive::User, "Software\\Valve\\Steam", "SteamPath")?;
    let p = PathBuf::from(p.replace('/', "\\"));
    p.is_dir().then_some(p)
}

fn steam(keep: &dyn Fn(&Path) -> bool) -> Vec<Entry> {
    let Some(root) = steam_root() else { return Vec::new() };
    let mut libs = vec![root.clone()];
    if let Ok(t) = std::fs::read_to_string(root.join("steamapps").join("libraryfolders.vdf")) {
        let s = vdf_strings(&t);
        for w in s.windows(2) {
            if w[0].eq_ignore_ascii_case("path") {
                let p = PathBuf::from(&w[1]);
                if !libs.iter().any(|l| same_dir(l, &p)) {
                    libs.push(p);
                }
            }
        }
    }
    let mut out = Vec::new();
    for lib in libs.into_iter().filter(|l| keep(l)) {
        let apps = lib.join("steamapps");
        let Ok(rd) = std::fs::read_dir(&apps) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let Ok(t) = std::fs::read_to_string(e.path()) else { continue };
            let s = vdf_strings(&t);
            let (Some(title), Some(dir)) = (vdf_value(&s, "name"), vdf_value(&s, "installdir")) else { continue };
            if tool(&title) {
                continue;
            }
            let folder = apps.join("common").join(&dir);
            if let Some(exe) = main_exe(&folder, &dir) {
                out.push(Entry { name: title, path: exe.display().to_string() });
            }
        }
    }
    out
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| p.display().to_string().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    n(a) == n(b)
}

/// Itens da Steam que não são jogos.
fn tool(title: &str) -> bool {
    let t = title.to_lowercase();
    t.contains("redistributable") || t.starts_with("proton") || t.starts_with("steam linux runtime") || t == "steamvr" || t.contains("steamworks") || t.contains("dedicated server") || t.ends_with(" sdk")
}

/// O executável principal de um jogo da Steam: o maior que sobra, até três pastas abaixo.
fn main_exe(folder: &Path, installdir: &str) -> Option<PathBuf> {
    let mut found: Vec<(PathBuf, u64, usize)> = Vec::new();
    let mut seen = 0usize;
    walk(folder, 0, &mut found, &mut seen);
    let key: String = installdir.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
    found
        .into_iter()
        .max_by_key(|(p, size, depth)| {
            let stem: String = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default().chars().filter(|c| c.is_alphanumeric()).collect();
            // O nome parecido com a pasta do jogo vale mais que o tamanho.
            let named = !key.is_empty() && (stem.starts_with(&key) || key.starts_with(&stem) && stem.len() >= 4);
            (named, *size / (1 << 20), std::cmp::Reverse(*depth))
        })
        .map(|(p, ..)| p)
}

fn walk(dir: &Path, depth: usize, found: &mut Vec<(PathBuf, u64, usize)>, seen: &mut usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        // Uma pasta enorme não trava a lista.
        *seen += 1;
        if *seen > 3000 {
            return;
        }
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().to_lowercase();
        if ft.is_dir() {
            if depth < 3 && !SKIP_DIRS.contains(&name.as_str()) {
                walk(&e.path(), depth + 1, found, seen);
            }
        } else if ft.is_file() && name.ends_with(".exe") && !NOT_GAME.iter().any(|n| name.contains(n)) {
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            found.push((e.path(), size, depth));
        }
    }
}

/// Um campo de texto de um JSON simples, sem montar o JSON inteiro.
fn json_text(text: &str, key: &str) -> Option<String> {
    let at = text.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = text[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    let mut s = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next()? {
                'n' => s.push('\n'),
                't' => s.push('\t'),
                'u' => {
                    let hex: String = chars.by_ref().take(4).collect();
                    s.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                c => s.push(c),
            },
            '"' => return Some(s),
            c => s.push(c),
        }
    }
    None
}

fn epic() -> Vec<Entry> {
    let dir = crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramData).join("Epic").join("EpicGamesLauncher").join("Data").join("Manifests");
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out = Vec::new();
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().to_lowercase().ends_with(".item") {
            continue;
        }
        let Ok(t) = std::fs::read_to_string(e.path()) else { continue };
        let (Some(name), Some(folder), Some(exe)) = (json_text(&t, "DisplayName"), json_text(&t, "InstallLocation"), json_text(&t, "LaunchExecutable")) else { continue };
        if exe.is_empty() {
            continue;
        }
        let path = PathBuf::from(folder.replace('/', "\\")).join(exe.replace('/', "\\"));
        if path.is_file() {
            out.push(Entry { name, path: path.display().to_string() });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_vdf_and_json() {
        let s = vdf_strings("\"AppState\"\n{\n\t\"name\"\t\t\"Game \\\"X\\\"\"\n\t\"installdir\"\t\t\"Dir\"\n}");
        assert_eq!(vdf_value(&s, "name").as_deref(), Some("Game \"X\""));
        assert_eq!(vdf_value(&s, "installdir").as_deref(), Some("Dir"));
        let lib = vdf_strings("\"0\" { \"path\" \"D:\\\\SteamLibrary\" }");
        assert_eq!(vdf_value(&lib, "path").as_deref(), Some("D:\\SteamLibrary"));
        let j = "{\n\t\"LaunchExecutable\": \"Bin/Game.exe\",\n\t\"DisplayName\": \"Caf\\u00e9 \\\"Z\\\"\"\n}";
        assert_eq!(json_text(j, "LaunchExecutable").as_deref(), Some("Bin/Game.exe"));
        assert_eq!(json_text(j, "DisplayName").as_deref(), Some("Café \"Z\""));
        assert!(tool("Steamworks Common Redistributables"));
        assert!(tool("Proton 8.0"));
        assert!(!tool("Hollow Knight"));
    }

    /// Os jogos deste PC (sem o disco F:): `cargo test stores_here -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn stores_here() {
        let mut all = steam(&|p| !p.display().to_string().to_uppercase().starts_with("F:"));
        all.extend(epic());
        for e in &all {
            println!("{}	{}", e.name, e.path);
        }
        println!("{} jogos", all.len());
    }
}
