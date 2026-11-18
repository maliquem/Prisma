//! VB-CABLE (VB-Audio, donationware de terceiros): o cabo de som virtual onde o host toca o
//! microfone de quem assiste (`stream/mic.rs`); os programas do host ouvem o CABLE Output como
//! microfone. Não vem no instalador do Prisma. Com o consentimento na tela, o Prisma baixa o pacote
//! da página oficial, confere a soma e abre o instalador da VB-Audio, onde a pessoa clica em
//! Install Driver. Remover abre o mesmo instalador (Remove Driver) e só aparece se foi o Prisma que
//! instalou. Mesmo padrão da ponte VIIPER (`xbridge.rs`) e da tela virtual (`vdd.rs`).

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

pub use super::xbridge::Stage;

const URL: &str = "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack46.zip";
/// SHA-256 do pacote 46 (31/10/2026). A VB-Audio não publica a soma: tirada do download oficial em 18/11/2026.
const SUM: &str = "bc44e8974191a5a2427d6c979e2dab08dbb09925f3f435dfa2f0ba3e45b6eb90";
/// SHA-256 do instalador de 64 bits que vem no pacote (assinado por BUREL VINCENT, o autor).
const SETUP_SUM: &str = "734c35dfa6d98f48782a451633ceb471166ec70d60482fd89a1123d0ee3c4f41";
/// Começo do nome da saída do cabo no Windows: "CABLE Input (VB-Audio Virtual Cable)".
const INPUT: &str = "CABLE Input";

/// O id do Windows do CABLE Input, se o VB-CABLE está instalado e ligado.
pub fn input_id() -> Option<String> {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    super::audio::devices(false).into_iter().find(|d| d.name.starts_with(INPUT)).map(|d| d.id)
}

fn present() -> bool {
    input_id().is_some()
}

/// O pacote conferido fica guardado: marca que o Prisma instalou e serve ao Remover. Sem o cabo na
/// primeira olhada (instalação cancelada ou removida por fora), a marca vai embora.
// shortcut: abrir o Prisma de novo antes do reinício que a instalação pediu também apaga a marca, e
// o cabo passa a contar como instalado por fora; guardar o estado Reboot em disco se isso acontecer.
fn kept() -> PathBuf {
    crate::config::data_dir().join("compartilhar").join("vbcable.zip")
}

/// O Prisma instalou o VB-CABLE neste PC; só então o cartão oferece Remover.
pub fn ours() -> bool {
    kept().exists()
}

/// O script que roda como administrador: confere a soma do pacote de novo, abre numa pasta do
/// sistema, confere o instalador e espera a pessoa terminar nele (instalar ou remover).
pub fn setup_script(zip: &Path) -> String {
    format!(
        "$ErrorActionPreference='Stop'\n\
function Sum($p){{(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLower()}}\n\
if((Sum {z}) -ne '{zs}'){{exit 5}}\n\
$d=Join-Path $env:SystemRoot ('Temp\\prisma-vbcable-'+[guid]::NewGuid().ToString('N'))\n\
Expand-Archive -LiteralPath {z} -DestinationPath $d\n\
$s=Join-Path $d 'VBCABLE_Setup_x64.exe'\n\
if((Sum $s) -ne '{ss}'){{Remove-Item -LiteralPath $d -Recurse -Force; exit 5}}\n\
Start-Process -FilePath $s -WorkingDirectory $d -Wait\n\
Remove-Item -LiteralPath $d -Recurse -Force -ErrorAction SilentlyContinue\n\
exit 0\n",
        z = super::quick::ps_quote(&zip.to_string_lossy()),
        zs = SUM,
        ss = SETUP_SUM,
    )
}

/// Abre o instalador da VB-Audio como administrador, com o pacote preso sem escrita até o fim.
fn run_setup(zip: &Path) -> Result<(), String> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;
    let lock = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ.0).open(zip).map_err(|_| "o pacote guardado sumiu".to_string())?;
    let script = setup_script(zip);
    let r = super::xbridge::shell_wait(&super::system_tool("powershell").to_string_lossy(), &format!("-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}", super::quick::utf16_b64(&script)), true);
    drop(lock);
    r
}

fn install() -> Stage {
    let zip = kept();
    let data = match super::update::download(URL) {
        Ok(d) => d,
        Err(_) => return Stage::Failed("o download falhou".into()),
    };
    if super::update::sha256(&data).as_deref() != Some(SUM) {
        return Stage::Failed("o arquivo baixado não confere com o pacote oficial".into());
    }
    if let Err(e) = std::fs::create_dir_all(zip.parent().unwrap()).and_then(|_| std::fs::write(&zip, data)) {
        return Stage::Failed(e.to_string());
    }
    set_stage(Stage::Installing);
    if let Err(e) = run_setup(&zip) {
        // Recusou o UAC ou o instalador falhou: nada foi instalado, então o Prisma não é dono.
        let _ = std::fs::remove_file(&zip);
        return Stage::Failed(e);
    }
    // O cabo costuma aparecer logo; senão, depois de reiniciar (ou ela fechou sem instalar).
    for _ in 0..20 {
        if present() {
            return Stage::Ready;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Stage::Reboot
}

static STAGE: Mutex<Option<Stage>> = Mutex::new(None);

fn set_stage(s: Stage) {
    *STAGE.lock().unwrap() = Some(s);
}

/// Estado agora (olha o sistema na primeira vez e depois de `recheck`).
pub fn stage() -> Stage {
    let mut g = STAGE.lock().unwrap();
    g.get_or_insert_with(|| {
        if present() {
            Stage::Ready
        } else {
            let _ = std::fs::remove_file(kept());
            Stage::Absent
        }
    })
    .clone()
}

/// Olha o sistema de novo (depois de reiniciar o PC, por exemplo).
pub fn recheck() {
    if !stage().busy() {
        set_stage(if present() { Stage::Ready } else { Stage::Absent });
    }
}

/// Começa a instalar, numa linha à parte. Só depois do consentimento na tela.
pub fn start_install() {
    if stage().busy() {
        return;
    }
    set_stage(Stage::Downloading);
    std::thread::spawn(|| set_stage(install()));
}

/// Abre o instalador da VB-Audio para remover. Instalado por outro caminho: não mexe (o cartão nem
/// mostra o botão).
pub fn start_remove() {
    if !ours() || stage().busy() {
        return;
    }
    set_stage(Stage::Removing);
    std::thread::spawn(|| {
        let zip = kept();
        set_stage(match run_setup(&zip) {
            Ok(()) if !present() => {
                let _ = std::fs::remove_file(zip);
                Stage::Absent
            }
            Ok(()) => Stage::Reboot,
            Err(e) => Stage::Failed(e),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_confere_pacote_e_instalador() {
        let s = setup_script(Path::new(r"C:\Users\o'neil\vbcable.zip"));
        assert!(s.contains(&format!("(Sum 'C:\\Users\\o''neil\\vbcable.zip') -ne '{SUM}'")));
        assert!(s.contains(SETUP_SUM));
        // A pessoa clica no instalador: ele não pode rodar escondido nem sem esperar.
        assert!(s.contains("Start-Process -FilePath $s -WorkingDirectory $d -Wait\n"));
    }
}
