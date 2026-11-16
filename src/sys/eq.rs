//! Perfil de equalizador por jogo. Os perfis de cada aparelho ficam em
//! `C:\ProgramData\Prisma\eq\perfis\<guid>\<nome>.txt`. Com o jogo aberto, o Prisma copia o perfil
//! escolhido para `jogo-<guid>.txt`, que o APO prefere ao `perfil-<guid>.txt` (troca ao vivo, com
//! mistura); o jogo fechou, o arquivo sai e o aparelho volta ao perfil dele.
// shortcut: o perfil vale para o aparelho inteiro enquanto o jogo está aberto (não só para o jogo);
// trocar quando o APO souber de qual programa vem cada fluxo.

use std::path::{Path, PathBuf};

pub fn dir() -> PathBuf {
    let base = std::env::var_os("ProgramData").map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
    base.join(r"Prisma\eq")
}

/// GUID do aparelho como o APO usa: `{0.0.0.00000000}.{GUID}` → `guid`.
pub fn guid_of(endpoint: &str) -> String {
    endpoint.rsplit_once('.').map_or("", |(_, g)| g).trim_matches(['{', '}']).to_ascii_lowercase()
}

/// Nomes dos perfis guardados para o aparelho, em ordem.
pub fn profiles_in(base: &Path, guid: &str) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(base.join("perfis").join(guid)) else { return Vec::new() };
    let mut v: Vec<String> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("txt")))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    v.sort_by_key(|s| s.to_lowercase());
    v
}

pub fn profiles(guid: &str) -> Vec<String> {
    if guid.is_empty() { Vec::new() } else { profiles_in(&dir(), guid) }
}

/// Deixa no lugar só o perfil de jogo pedido (`Some((guid, nome))`) ou nenhum. Devolve o erro para o log.
pub fn apply_in(base: &Path, want: Option<(&str, &str)>) -> Result<(), String> {
    let keep = want.map(|(g, _)| format!("jogo-{g}.txt"));
    if let Ok(rd) = std::fs::read_dir(base) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("jogo-") && name.ends_with(".txt") && Some(&name) != keep.as_ref() {
                std::fs::remove_file(e.path()).map_err(|err| format!("não apaguei {name}: {err}"))?;
            }
        }
    }
    let (Some((guid, profile)), Some(keep)) = (want, keep) else { return Ok(()) };
    let text = std::fs::read(base.join("perfis").join(guid).join(format!("{profile}.txt"))).map_err(|e| format!("não li o perfil {profile}: {e}"))?;
    let dest = base.join(keep);
    // Só grava se mudou: o APO troca o som a cada gravação.
    if std::fs::read(&dest).ok().as_deref() != Some(&text[..]) {
        std::fs::write(&dest, &text).map_err(|e| format!("não gravei {}: {e}", dest.display()))?;
    }
    Ok(())
}

pub fn apply(want: Option<(&str, &str)>) -> Result<(), String> {
    apply_in(&dir(), want)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_do_aparelho() {
        assert_eq!(guid_of("{0.0.0.00000000}.{AB12-CD}"), "ab12-cd");
        assert_eq!(guid_of(""), "");
    }

    #[test]
    fn perfil_do_jogo_entra_e_sai() {
        let base = std::env::temp_dir().join(format!("prisma-eq-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join(r"perfis\g1")).unwrap();
        std::fs::write(base.join(r"perfis\g1\FPS.txt"), "preamp=3").unwrap();
        std::fs::write(base.join(r"perfis\g1\jogos.txt"), "preamp=1").unwrap();
        std::fs::write(base.join(r"perfis\g1\nota.md"), "").unwrap();
        assert_eq!(profiles_in(&base, "g1"), ["FPS", "jogos"]);
        assert!(profiles_in(&base, "outro").is_empty());

        std::fs::write(base.join("jogo-velho.txt"), "x").unwrap();
        apply_in(&base, Some(("g1", "FPS"))).unwrap();
        assert_eq!(std::fs::read_to_string(base.join("jogo-g1.txt")).unwrap(), "preamp=3");
        assert!(!base.join("jogo-velho.txt").exists());
        assert!(apply_in(&base, Some(("g1", "sumiu"))).is_err());
        apply_in(&base, None).unwrap();
        assert!(!base.join("jogo-g1.txt").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
