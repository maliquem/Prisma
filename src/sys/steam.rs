//! Steam Input dos controles PlayStation. O som pelo Bluetooth (relatório 0x39) faz o DualSense
//! mandar pacotes 0x31 que não são entrada; o Prisma os ignora, mas o Steam os lê como botões.
//! A opção fica no `localconfig.vdf` da conta logada (`"SteamController_PSSupport"`, 0 desligado)
//! e o Steam a regrava ao sair: só dá para mudar com ele fechado.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::reg::{self, Hive};

const KEY: &str = "\"SteamController_PSSupport\"";

struct Steam {
    /// O steam.exe da instalação (o que está no registro), com barras invertidas.
    exe: String,
    config: PathBuf,
}

fn find() -> Option<Steam> {
    let exe = reg::text(Hive::User, r"Software\Valve\Steam", "SteamExe")?.replace('/', "\\");
    let user = reg::dword(Hive::User, r"Software\Valve\Steam\ActiveProcess", "ActiveUser").filter(|&u| u != 0)?;
    let dir = PathBuf::from(&exe).parent()?.to_path_buf();
    let config = dir.join("userdata").join(user.to_string()).join("config").join("localconfig.vdf");
    config.exists().then_some(Steam { exe, config })
}

/// Steam Input ligado para os controles PlayStation (sem a linha, o Steam usa o padrão: ligado).
fn ps_on(text: &str) -> bool {
    text.lines().find_map(|l| l.trim_start().strip_prefix(KEY)).map_or(true, |v| v.trim().trim_matches('"') != "0")
}

/// O texto com a opção desligada; sem a linha, ela entra no começo do bloco de fora.
fn ps_off(text: &str) -> String {
    let mut found = false;
    let mut out: Vec<String> = text
        .split_inclusive('\n')
        .map(|l| match l.trim_start().strip_prefix(KEY) {
            Some(_) => {
                found = true;
                let indent = &l[..l.len() - l.trim_start().len()];
                let end = if l.ends_with("\r\n") { "\r\n" } else if l.ends_with('\n') { "\n" } else { "" };
                format!("{indent}{KEY}\t\t\"0\"{end}")
            }
            None => l.to_string(),
        })
        .collect();
    if !found {
        if let Some(i) = out.iter().position(|l| l.trim() == "{") {
            out.insert(i + 1, format!("\t{KEY}\t\t\"0\"\n"));
        }
    }
    out.concat()
}

/// Processos do steam.exe desta instalação. O SteamService.exe (serviço do Windows) e os
/// steamwebhelper.exe ficam de fora: estes fecham junto com o steam.exe.
fn pids(exe: &str) -> Vec<u32> {
    let want = exe.to_lowercase();
    super::procs::parent_map().into_keys().filter(|&p| super::procs::image_path(p).is_some_and(|i| i.to_lowercase().replace('/', "\\") == want)).collect()
}

/// `Some(true)` com o Steam instalado, alguém logado nele e o Steam Input dos controles
/// PlayStation ligado; `None` sem Steam.
pub fn ps_input_on() -> Option<bool> {
    let s = find()?;
    Some(ps_on(&std::fs::read_to_string(&s.config).ok()?))
}

/// Desliga o Steam Input dos controles PlayStation: fecha o Steam pelo pedido dele mesmo
/// (`steam.exe -shutdown`), espera o steam.exe sair, muda a opção (com cópia do arquivo antes) e
/// abre o Steam de novo se ele estava aberto. Bloqueia: chame fora da janela.
pub fn disable_ps_input() -> Result<(), &'static str> {
    let s = find().ok_or("Não achei o Steam")?;
    let open = !pids(&s.exe).is_empty();
    if open {
        std::process::Command::new(&s.exe).arg("-shutdown").spawn().map_err(|_| "Não consegui pedir para o Steam fechar")?;
        let until = Instant::now() + Duration::from_secs(45);
        while !pids(&s.exe).is_empty() {
            if Instant::now() > until {
                return Err("O Steam não fechou em 45 s; nada foi mudado");
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        // O Steam grava o arquivo ao sair: um respiro antes de ler.
        std::thread::sleep(Duration::from_secs(1));
    }
    let text = std::fs::read_to_string(&s.config).map_err(|_| "Não consegui ler a configuração do Steam")?;
    if ps_on(&text) {
        let mut bak = s.config.clone().into_os_string();
        bak.push(".prisma-bak");
        std::fs::copy(&s.config, &bak).map_err(|_| "Não consegui guardar uma cópia da configuração do Steam")?;
        std::fs::write(&s.config, ps_off(&text)).map_err(|_| "Não consegui gravar a configuração do Steam")?;
    }
    if open {
        super::quick::open_as_user(&s.exe);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ps_support_turns_off_in_place() {
        let on = "\"UserLocalConfigStore\"\r\n{\r\n\t\"A\"\t\t\"1\"\r\n\t\"SteamController_PSSupport\"\t\t\"1\"\r\n\t\"B\"\t\t\"2\"\r\n}\r\n";
        assert!(ps_on(on));
        let off = ps_off(on);
        assert!(!ps_on(&off));
        assert_eq!(off, on.replace("PSSupport\"\t\t\"1\"", "PSSupport\"\t\t\"0\""));
        // Sem a linha vale o padrão (ligado); desligar acrescenta a linha no bloco de fora.
        let none = "\"UserLocalConfigStore\"\n{\n\t\"A\"\t\t\"1\"\n}\n";
        assert!(ps_on(none));
        let added = ps_off(none);
        assert!(!ps_on(&added));
        assert!(added.starts_with("\"UserLocalConfigStore\"\n{\n\t\"SteamController_PSSupport\"\t\t\"0\"\n"));
        assert!(!ps_on("\t\"SteamController_PSSupport\"\t\t\"0\"\n"));
    }
}
