//! O motor do equalizador (o APO, uma DLL que o serviço de áudio do Windows carrega): estado,
//! instalação e remoção. Ler o estado não pede nada; instalar e remover rodam numa cópia de
//! administrador (`--eq=...`) aberta pelo botão, com o pedido do Windows (UAC). O Prisma não fica
//! elevado.
//!
//! Instalar num aparelho, guardando o que muda em `ProgramData\Prisma\eq\backup\<guid>.txt`:
//! põe o APO no fim da lista de efeitos do aparelho (`{d04e05a6-...},13`) ou, sem lista, no lugar
//! antigo (`,5`); liga as melhorias de áudio se estavam desligadas. Os modos (`{d3993a3f-...}`)
//! nunca são tocados. Remover desfaz valor por valor (o administrador não pode apagar a chave).

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use super::reg::{self, Hive};

pub const CLSID: &str = "{6F1E3A52-8C47-4D0B-9A3E-2B5C7D9E1F40}";
const IID_SFX2: &str = "{BAFE99D2-7436-44CE-9E0E-4D89AFBFFF56}";
const FX: &str = "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d}";
const SYSFX: &str = "{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5";
const RENDER: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Render";
const AUDIO: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Audio";
const NO_WINDOW: u32 = 0x0800_0000;

/// A DLL vai junto dentro do Prisma: tudo continua sendo um arquivo só.
static DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/apo/release/prisma_apo.dll"));

fn com_key() -> String {
    format!(r"SOFTWARE\Classes\CLSID\{CLSID}")
}

fn apo_key() -> String {
    format!(r"SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\{CLSID}")
}

fn fx_key(guid: &str) -> String {
    format!(r"{RENDER}\{{{guid}}}\FxProperties")
}

fn dll_path() -> PathBuf {
    let base = std::env::var_os("ProgramFiles").map_or_else(|| PathBuf::from(r"C:\Program Files"), PathBuf::from);
    base.join(r"Prisma\eq\prisma_apo.dll")
}

fn backup_dir() -> PathBuf {
    super::eq::dir().join("backup")
}

/// GUID de aparelho vindo da linha de comando: só hexadecimal e traço.
fn valid_guid(g: &str) -> bool {
    g.len() == 36 && g.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn multi_decode(b: &[u8]) -> Vec<String> {
    let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    w.split(|c| *c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

fn multi_encode(list: &[String]) -> Vec<u8> {
    let mut w: Vec<u16> = Vec::new();
    for s in list {
        w.extend(s.encode_utf16());
        w.push(0);
    }
    w.push(0);
    w.iter().flat_map(|c| c.to_le_bytes()).collect()
}

fn list13(guid: &str) -> Option<Vec<String>> {
    let (ty, b) = reg::read(Hive::Machine, &fx_key(guid), &format!("{FX},13"))?;
    (ty == 7).then(|| multi_decode(&b))
}

/// A DLL está registrada no Windows.
pub fn registered() -> bool {
    reg::exists(Hive::Machine, &apo_key())
}

/// O APO está na lista de efeitos deste aparelho (GUID sem chaves).
pub fn active_on(guid: &str) -> bool {
    let has = |s: &String| s.eq_ignore_ascii_case(CLSID);
    list13(guid).is_some_and(|l| l.iter().any(has)) || reg::text(Hive::Machine, &fx_key(guid), &format!("{FX},5")).is_some_and(|s| has(&s))
}

/// Abre a cópia de administrador. `op`: `install:<guid>,...`, `remove:<guid>,...` ou `remove`.
/// Devolve se deu certo (false também quando o pedido do Windows foi recusado).
pub fn run_elevated(op: &str) -> bool {
    crate::startup::run_elevated_wait(&format!("--eq={op}")) == Some(0)
}

/// Cópia de administrador: lê `--eq=` e sai.
pub fn run_arg() -> Option<i32> {
    let op = std::env::args().find_map(|a| a.strip_prefix("--eq=").map(str::to_string))?;
    let ok = match op.split_once(':') {
        Some(("install", ids)) => install(&ids.split(',').map(str::to_string).collect::<Vec<_>>()),
        Some(("remove", ids)) => remove(Some(&ids.split(',').map(str::to_string).collect::<Vec<_>>())),
        None if op == "remove" => remove(None),
        _ => Err("pedido desconhecido".into()),
    };
    if let Err(e) = &ok {
        log(&format!("--eq={op}: {e}"));
    }
    Some(if ok.is_ok() { 0 } else { 3 })
}

fn log(msg: &str) {
    use std::io::Write;
    let _ = std::fs::create_dir_all(super::eq::dir());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(super::eq::dir().join("instalar.log")) {
        let _ = writeln!(f, "{msg}");
    }
}

fn net(args: &[&str]) {
    let _ = Command::new(super::system_tool("net")).args(args).creation_flags(NO_WINDOW).output();
}

/// Para o áudio (o AudioEndpointBuilder leva o Audiosrv junto): o motor solta a DLL e relê os
/// efeitos de cada aparelho ao voltar.
fn audio_stop() {
    net(&["stop", "AudioEndpointBuilder", "/y"]);
}

fn audio_start() {
    net(&["start", "Audiosrv"]);
}

fn write_text(key: &str, name: &str, v: &str) -> bool {
    let w: Vec<u8> = v.encode_utf16().chain(std::iter::once(0)).flat_map(|c| c.to_le_bytes()).collect();
    reg::write(Hive::Machine, key, name, 1, &w)
}

fn install(ids: &[String]) -> Result<(), String> {
    if ids.is_empty() || !ids.iter().all(|g| valid_guid(g)) {
        return Err("aparelho inválido".into());
    }
    let data = super::eq::dir();
    std::fs::create_dir_all(backup_dir()).map_err(|e| format!("pasta {}: {e}", data.display()))?;
    // O motor de áudio roda como LOCAL SERVICE: lê os perfis e escreve o log. Os usuários gravam os perfis.
    let _ = Command::new(super::system_tool("icacls"))
        .arg(&data)
        .args(["/grant", "*S-1-5-19:(OI)(CI)M", "/grant", "*S-1-5-32-545:(OI)(CI)M"])
        .creation_flags(NO_WINDOW)
        .output();

    audio_stop();
    let res = install_stopped(ids);
    audio_start();
    res
}

fn install_stopped(ids: &[String]) -> Result<(), String> {
    let dll = dll_path();
    if let Some(dir) = dll.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("pasta da DLL: {e}"))?;
    }
    if std::fs::read(&dll).ok().as_deref() != Some(DLL) {
        std::fs::write(&dll, DLL).map_err(|e| format!("não gravei {}: {e}", dll.display()))?;
    }
    let com = com_key();
    let apo = apo_key();
    let mut ok = write_text(&com, "", "Prisma EQ")
        && write_text(&format!(r"{com}\InprocServer32"), "", &dll.display().to_string())
        && write_text(&format!(r"{com}\InprocServer32"), "ThreadingModel", "Both")
        && write_text(&apo, "FriendlyName", "Prisma EQ")
        && write_text(&apo, "Copyright", "Prisma")
        && write_text(&apo, "APOInterface0", IID_SFX2);
    for (n, v) in [
        ("MajorVersion", 0u32),
        ("MinorVersion", 1),
        ("Flags", 14),
        ("MinInputConnections", 1),
        ("MaxInputConnections", 1),
        ("MinOutputConnections", 1),
        ("MaxOutputConnections", 1),
        ("MaxInstances", u32::MAX),
        ("NumAPOInterfaces", 1),
    ] {
        ok &= reg::write_dword(Hive::Machine, &apo, n, v);
    }
    if !ok {
        return Err("não registrei a DLL".into());
    }
    // A DLL não é assinada: o motor protegido não a carrega. É o que o Equalizer APO também faz.
    let prot = backup_dir().join("protecao.txt");
    if !prot.exists() {
        let before = reg::dword(Hive::Machine, AUDIO, "DisableProtectedAudioDG").map_or("ausente".to_string(), |v| v.to_string());
        let _ = std::fs::write(&prot, before);
    }
    reg::write_dword(Hive::Machine, AUDIO, "DisableProtectedAudioDG", 1);

    let mut failed = Vec::new();
    for g in ids {
        if let Err(e) = add_to_device(g) {
            failed.push(format!("{g}: {e}"));
        }
    }
    if failed.is_empty() { Ok(()) } else { Err(failed.join("; ")) }
}

fn add_to_device(guid: &str) -> Result<(), String> {
    let key = fx_key(guid);
    if !reg::exists(Hive::Machine, &key) {
        return Err("aparelho sem FxProperties".into());
    }
    let backup = backup_dir().join(format!("{guid}.txt"));
    // Só a primeira instalação guarda: rodar de novo não pode trocar o original pelo nosso.
    let mut note = std::fs::read_to_string(&backup).unwrap_or_default();
    let first = note.is_empty();
    let name13 = format!("{FX},13");
    match list13(guid) {
        Some(mut l) if !l.is_empty() => {
            if !l.iter().any(|s| s.eq_ignore_ascii_case(CLSID)) {
                l.push(CLSID.to_string());
                if !reg::write(Hive::Machine, &key, &name13, 7, &multi_encode(&l)) {
                    return Err("lista de efeitos negada".into());
                }
            }
            if first {
                note.push_str("lista13\n");
            }
        }
        _ => {
            let name5 = format!("{FX},5");
            let old = reg::text(Hive::Machine, &key, &name5).unwrap_or_default();
            if first {
                note.push_str(&format!("antigo|{old}\n"));
            }
            if !old.eq_ignore_ascii_case(CLSID) && !write_text(&key, &name5, CLSID) {
                return Err("efeito do aparelho negado".into());
            }
        }
    }
    // "Melhorias de áudio" desligadas: o Windows não carrega efeito nenhum.
    if reg::dword(Hive::Machine, &key, SYSFX) == Some(1) {
        reg::write_dword(Hive::Machine, &key, SYSFX, 0);
        note.push_str("melhorias|1\n");
    }
    if first || note.contains("melhorias") {
        std::fs::write(&backup, note).map_err(|e| format!("cópia: {e}"))?;
    }
    Ok(())
}

fn remove_from_device(guid: &str) {
    let key = fx_key(guid);
    let name13 = format!("{FX},13");
    if let Some(l) = list13(guid).filter(|l| l.iter().any(|s| s.eq_ignore_ascii_case(CLSID))) {
        let rest: Vec<String> = l.into_iter().filter(|s| !s.eq_ignore_ascii_case(CLSID)).collect();
        reg::write(Hive::Machine, &key, &name13, 7, &multi_encode(&rest));
    }
    let backup = backup_dir().join(format!("{guid}.txt"));
    let note = std::fs::read_to_string(&backup).unwrap_or_default();
    let name5 = format!("{FX},5");
    let old = note.lines().find_map(|l| l.strip_prefix("antigo|"));
    match old {
        Some("") => {
            reg::delete(Hive::Machine, &key, &name5);
        }
        Some(v) => {
            write_text(&key, &name5, v);
        }
        // Sem cópia: só tira o nosso se ele está lá.
        None if reg::text(Hive::Machine, &key, &name5).is_some_and(|s| s.eq_ignore_ascii_case(CLSID)) => {
            reg::delete(Hive::Machine, &key, &name5);
        }
        None => {}
    }
    if let Some(v) = note.lines().find_map(|l| l.strip_prefix("melhorias|")).and_then(|v| v.parse().ok()) {
        reg::write_dword(Hive::Machine, &key, SYSFX, v);
    }
    let _ = std::fs::remove_file(backup);
}

/// Aparelhos de saída (GUID) com o APO, achados no registro.
fn devices_with() -> Vec<String> {
    reg::subkeys(Hive::Machine, RENDER).into_iter().map(|k| k.trim_matches(['{', '}']).to_ascii_lowercase()).filter(|g| active_on(g)).collect()
}

/// Tira dos aparelhos pedidos (`None`: de todos). Sem nenhum aparelho com ele, tira o registro,
/// volta a proteção do motor e apaga a DLL.
fn remove(ids: Option<&[String]>) -> Result<(), String> {
    if ids.is_some_and(|l| !l.iter().all(|g| valid_guid(g))) {
        return Err("aparelho inválido".into());
    }
    audio_stop();
    let list = ids.map_or_else(devices_with, <[String]>::to_vec);
    for g in &list {
        remove_from_device(g);
    }
    let all = devices_with().is_empty();
    if all {
        reg::delete_tree(Hive::Machine, &apo_key());
        reg::delete_tree(Hive::Machine, &com_key());
        let prot = backup_dir().join("protecao.txt");
        if let Ok(v) = std::fs::read_to_string(&prot) {
            match v.trim().parse::<u32>() {
                Ok(n) => reg::write_dword(Hive::Machine, AUDIO, "DisableProtectedAudioDG", n),
                Err(_) => reg::delete(Hive::Machine, AUDIO, "DisableProtectedAudioDG"),
            };
            let _ = std::fs::remove_file(prot);
        }
    }
    audio_start();
    if all {
        // Com o motor parado e de volta sem o APO, a DLL está solta.
        let _ = std::fs::remove_file(dll_path());
        if let Some(d) = dll_path().parent() {
            let _ = std::fs::remove_dir(d);
        }
    }
    Ok(())
}

/// Desinstalar o Prisma (já como administrador): tira o APO de tudo, se estiver.
pub fn uninstall() {
    if registered() || !devices_with().is_empty() {
        let _ = remove(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lista_de_texto_vai_e_volta() {
        let l = vec!["{A}".to_string(), CLSID.to_string()];
        assert_eq!(multi_decode(&multi_encode(&l)), l);
        assert!(multi_decode(&multi_encode(&[])).is_empty());
    }

    #[test]
    fn guid_da_linha_de_comando() {
        assert!(valid_guid("6f1e3a52-8c47-4d0b-9a3e-2b5c7d9e1f40"));
        assert!(!valid_guid(r"..\..\x"));
        assert!(!valid_guid("6f1e3a52-8c47-4d0b-9a3e-2b5c7d9e1f4}"));
    }
}
