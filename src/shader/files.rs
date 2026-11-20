//! Arquivos do modo Injeção: a DLL de shaders em 64 e 32 bits e o ajudante de 32 bits. Não vêm dentro do exe:
//! saem no `Prisma-injecao.zip` da versão no GitHub, baixado pela página Shaders. Só servem da mesma versão do
//! Prisma, porque a DLL e o motor conversam pela memória compartilhada (`prisma_shader::link`), que muda entre
//! versões. A DLL entra no processo do jogo: o zip é conferido (hash e assinatura da versão) antes de extrair, e
//! cada arquivo tem a própria assinatura (`.sig`, de `tools/publish.ps1`), conferida de novo a cada vez que é usado.

use std::path::{Path, PathBuf};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const ZIP: &str = "Prisma-injecao.zip";
/// DLL de 64 bits, DLL de 32 bits e o ajudante de 32 bits, nessa ordem (como o `build.rs` grava).
const FILES: [&str; 3] = ["prisma_shader_hook.dll", "prisma_shader_hook32.dll", "prisma_hook32.exe"];
/// Versão do Prisma de que os arquivos vieram.
const STAMP: &str = "versao.txt";

pub const BAD: &str = "O arquivo baixado não confere. Nada foi alterado.";
pub const UNSIGNED: &str = "O arquivo baixado não tem a assinatura do Prisma. Nada foi alterado.";
pub const OFFLINE: &str = "Não consegui baixar do GitHub. Tente de novo.";
pub const MISSING: &str = "Esta versão do Prisma ainda não tem o modo Injeção publicado no GitHub.";

fn home() -> PathBuf {
    crate::config::data_dir().join("injecao")
}

/// Onde ficam e se pedem assinatura. Em debug, a pasta que o `cargo build` acabou de compilar ao lado do exe
/// (`build.rs`), sem assinatura: a chave só existe no PC de quem publica. A versão publicada só usa a baixada.
fn dir() -> (PathBuf, bool) {
    #[cfg(debug_assertions)]
    if let Some(d) = std::env::current_exe().ok().and_then(|e| Some(e.parent()?.join("injecao"))).filter(|d| has(d, VERSION, false)) {
        return (d, false);
    }
    (home(), true)
}

fn installed(d: &Path) -> Option<String> {
    std::fs::read_to_string(d.join(STAMP)).ok().map(|s| s.trim().to_string())
}

fn sig_of(f: &str) -> String {
    format!("{f}.sig")
}

/// Os três arquivos (e as assinaturas, se `signed`) estão lá e são da versão `version`, sem ler as DLLs: a página
/// olha a cada quadro.
fn has(d: &Path, version: &str, signed: bool) -> bool {
    installed(d).as_deref() == Some(version) && FILES.iter().all(|f| d.join(f).is_file() && (!signed || d.join(sig_of(f)).is_file()))
}

/// Os três arquivos, só se forem da versão `version` e, com `signed`, cada um com a assinatura do Prisma para essa
/// versão. De outra versão, faltando um ou mexido depois do download, o motor fica Por cima.
fn load_from(d: &Path, version: &str, signed: bool) -> Option<[Vec<u8>; 3]> {
    if !has(d, version, signed) {
        return None;
    }
    let read = |f: &str| {
        let data = std::fs::read(d.join(f)).ok()?;
        if signed {
            let sig = std::fs::read(d.join(sig_of(f))).ok()?;
            crate::sys::update::signed(version, &crate::sys::update::sha256(&data)?, &sig).then_some(())?;
        }
        Some(data)
    };
    let [a, b, c] = FILES.map(read);
    Some([a?, b?, c?])
}

pub fn load() -> Option<[Vec<u8>; 3]> {
    let (d, signed) = dir();
    load_from(&d, VERSION, signed)
}

pub fn ready() -> bool {
    let (d, signed) = dir();
    has(&d, VERSION, signed)
}

/// Baixados para outra versão do Prisma: depois de uma atualização, o app baixa os desta sozinho.
pub fn stale() -> bool {
    installed(&home()).is_some_and(|v| v != VERSION)
}

/// Hash do `.sha256`, assinatura da versão e cara de zip. O texto assinado junta versão e hash, como o do exe.
fn verify(version: &str, data: &[u8], sha: &[u8], sig: &[u8]) -> Result<(), &'static str> {
    let want = String::from_utf8_lossy(sha).split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    if want.len() != 64 || crate::sys::update::sha256(data).as_deref() != Some(want.as_str()) || !data.starts_with(b"PK") {
        return Err(BAD);
    }
    if !crate::sys::update::signed(version, &want, sig) {
        return Err(UNSIGNED);
    }
    Ok(())
}

/// Baixa o zip da versão deste Prisma, confere e troca a pasta inteira. `progress`: o que veio e o total.
pub fn download(progress: impl FnMut(usize, usize)) -> Result<(), &'static str> {
    use std::os::windows::process::CommandExt;
    use crate::sys::update;
    let base = format!("https://github.com/maliquem/Prisma/releases/download/v{VERSION}/{ZIP}");
    let to = home();
    let tmp = to.with_file_name(format!("injecao-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|_| OFFLINE)?;
    let res = (|| {
        let zip = tmp.join(ZIP);
        update::download_to(&base, &zip, progress).map_err(|e| if e == 404 { MISSING } else { OFFLINE })?;
        let sha = update::download(&format!("{base}.sha256")).map_err(|_| OFFLINE)?;
        // Sem `.sig` (404) é zip sem assinatura: recusado como assinatura errada.
        let sig = update::download(&format!("{base}.sig")).unwrap_or_default();
        verify(VERSION, &std::fs::read(&zip).map_err(|_| BAD)?, &sha, &sig)?;
        let out = tmp.join("x");
        let q = |p: &Path| crate::sys::quick::ps_quote(&p.to_string_lossy());
        let script = format!("$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; Expand-Archive -LiteralPath {} -DestinationPath {} -Force", q(&zip), q(&out));
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let ok = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).creation_flags(CREATE_NO_WINDOW).status().is_ok_and(|s| s.success());
        std::fs::write(out.join(STAMP), VERSION).map_err(|_| BAD)?;
        // Cada arquivo também assinado: é a mesma conferência que o motor faz antes de pôr a DLL no jogo.
        if !ok || load_from(&out, VERSION, true).is_none() {
            return Err(UNSIGNED);
        }
        // As DLLs daqui só são lidas (o jogo carrega uma cópia em `shaders\`): nada as prende.
        let _ = std::fs::remove_dir_all(&to);
        std::fs::rename(&out, &to).map_err(|_| BAD)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn so_usa_arquivos_da_mesma_versao() {
        let d = std::env::temp_dir().join(format!("prisma-injecao-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // Sem nada baixado: o perfil com mode=inject fica Por cima.
        assert!(load_from(&d, "1.11.0", false).is_none());
        for f in FILES {
            std::fs::write(d.join(f), f).unwrap();
        }
        assert!(load_from(&d, "1.11.0", false).is_none(), "sem versão gravada passou");
        std::fs::write(d.join(STAMP), "1.10.0\n").unwrap();
        assert!(load_from(&d, "1.11.0", false).is_none(), "versão de outro Prisma passou");
        // A pasta do cargo build (debug) não tem assinatura; a baixada tem que ter.
        let got = load_from(&d, "1.10.0", false).expect("mesma versão");
        assert_eq!(got[2], b"prisma_hook32.exe");
        assert!(!has(&d, "1.10.0", true) && load_from(&d, "1.10.0", true).is_none(), "sem .sig passou");
        for f in FILES {
            std::fs::write(d.join(sig_of(f)), [7u8; 64]).unwrap();
        }
        assert!(has(&d, "1.10.0", true), "com .sig devia contar como baixado");
        assert!(load_from(&d, "1.10.0", true).is_none(), "assinatura falsa passou");
        std::fs::remove_file(d.join(FILES[1])).unwrap();
        assert!(load_from(&d, "1.10.0", false).is_none(), "faltando a DLL de 32 bits passou");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn zip_sem_hash_ou_assinatura_nao_passa() {
        let data = b"PK\x03\x04 zip de teste";
        let sha = crate::sys::update::sha256(data).unwrap();
        assert_eq!(verify("1.10.0", data, b"0000", &[]), Err(BAD));
        let exe = b"MZ outro";
        assert_eq!(verify("1.10.0", exe, crate::sys::update::sha256(exe).unwrap().as_bytes(), &[]), Err(BAD), "o que não é zip passou");
        assert_eq!(verify("1.10.0", data, format!("{sha}  {ZIP}\n").as_bytes(), &[0; 64]), Err(UNSIGNED));
        assert_eq!(verify("1.10.0", data, sha.as_bytes(), &[]), Err(UNSIGNED));
    }
}
