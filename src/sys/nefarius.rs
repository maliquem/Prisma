//! Os dois drivers da Nefarius que a aba Controles usa: o HidHide (esconde o controle físico do
//! jogo) e o ViGEmBus (o controle de Xbox virtual da ponte). Instalar, atualizar e remover, sempre
//! pelo instalador oficial, baixado do GitHub do projeto e conferido pela assinatura. O Prisma não
//! roda elevado: quem pede administrador é o instalador, com a tela dele.

use std::sync::Mutex;

pub struct Kit {
    pub name: &'static str,
    /// `dono/projeto` no GitHub, em minúsculas (a URL do instalador é conferida contra ele).
    repo: &'static str,
    /// Pedaços do nome que o instalador registra em "Aplicativos instalados".
    shown: &'static [&'static str],
}

pub const HIDHIDE: Kit = Kit { name: "HidHide", repo: "nefarius/hidhide", shown: &["HidHide"] };
pub const VIGEMBUS: Kit = Kit { name: "ViGEmBus", repo: "nefarius/vigembus", shown: &["ViGEm", "Virtual Gamepad Emulation"] };

/// Quem assina o instalador oficial.
const SIGNER: &str = "Nefarius Software Solutions";
const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";

/// O que o instalador registrou no Windows: a chave em "Uninstall" e a versão.
pub struct Product {
    key: String,
    pub version: String,
}

impl Kit {
    fn releases(&self) -> String {
        format!("https://api.github.com/repos/{}/releases/latest", self.repo)
    }

    /// O programa instalado, pelo registro de "Aplicativos instalados" (leitura sem administrador).
    fn product(&self) -> Option<Product> {
        use crate::sys::reg::{self, Hive};
        crate::sys::reg::subkeys(Hive::Machine, UNINSTALL).into_iter().find_map(|s| {
            let k = format!(r"{UNINSTALL}\{s}");
            let name = reg::text(Hive::Machine, &k, "DisplayName")?;
            let ours = self.shown.iter().any(|n| name.contains(n)) && reg::text(Hive::Machine, &k, "Publisher").is_some_and(|p| p.contains("Nefarius"));
            ours.then(|| Product { version: reg::text(Hive::Machine, &k, "DisplayVersion").unwrap_or_default(), key: k })
        })
    }

    /// A versão instalada (vazia se o instalador não disse), lida no máximo uma vez por segundo.
    pub fn version(&self) -> Option<String> {
        type Slot = Mutex<Option<(std::time::Instant, Option<String>)>>;
        static HH: Slot = Mutex::new(None);
        static VB: Slot = Mutex::new(None);
        let slot = if self.name == HIDHIDE.name { &HH } else { &VB };
        crate::sys::hidhide::cached(slot, || self.product().map(|p| p.version))
    }

    /// Instalado: o registro do instalador ou, sem ele, o próprio driver.
    pub fn installed(&self) -> bool {
        if self.name == HIDHIDE.name {
            return crate::sys::hidhide::installed();
        }
        self.version().is_some() || crate::sys::reg::exists(crate::sys::reg::Hive::Machine, r"SYSTEM\CurrentControlSet\Services\ViGEmBus")
    }

    /// A versão mais nova no GitHub e o instalador dela. Leva tempo: fora da janela.
    fn latest(&self) -> Result<(String, String), &'static str> {
        let json = crate::sys::update::download(&self.releases()).map_err(|_| "Não consegui falar com o GitHub da Nefarius")?;
        let json = String::from_utf8_lossy(&json);
        let url = installer_url(&json, self.repo).ok_or("Não achei o instalador na versão mais nova")?;
        Ok((tag(&json).unwrap_or_default(), url))
    }

    /// Baixa o instalador da versão mais nova, confere a assinatura e abre o instalador oficial,
    /// com a tela dele (os projetos não documentam modo silencioso). Serve para instalar, atualizar
    /// e reparar: o instalador novo tira o antigo. `Ok(true)`: o Windows pede reinício.
    pub fn install(&self) -> Result<bool, &'static str> {
        let (_, url) = self.latest()?;
        let data = crate::sys::update::download(&url).map_err(|_| "Não consegui baixar o instalador")?;
        let file = std::env::temp_dir().join(format!("{}_setup.exe", self.name));
        let lock = crate::sys::fileinfo::write_locked(&file, &data).map_err(|_| "Não consegui gravar o instalador")?;
        let path = file.display().to_string();
        match crate::sys::fileinfo::check(&path) {
            crate::sys::fileinfo::Signature::Valid(n) if n.contains(SIGNER) => {}
            _ => {
                drop(lock);
                let _ = std::fs::remove_file(&file);
                return Err("O instalador baixado não tem a assinatura da Nefarius");
            }
        }
        let ok = crate::startup::run_file_elevated_wait(&file, "", true).and_then(crate::startup::install_ok);
        drop(lock);
        let _ = std::fs::remove_file(&file);
        ok.ok_or("O driver não foi instalado")
    }

    /// Remove pelo desinstalador que o próprio instalador registrou, com a tela e o pedido de
    /// administrador dele. `Ok(true)`: o Windows pede reinício.
    pub fn uninstall(&self) -> Result<bool, &'static str> {
        let (exe, args) = self.uninstall_command(false).ok_or("Não achei o desinstalador do driver")?;
        crate::startup::run_file_elevated_wait(std::path::Path::new(&exe), &args, true).and_then(crate::startup::install_ok).ok_or("O driver não foi removido")
    }

    /// Programa e argumentos do desinstalador registrado. `quiet`: sem tela, para quem já é administrador.
    pub fn uninstall_command(&self, quiet: bool) -> Option<(String, String)> {
        use crate::sys::reg::{self, Hive};
        let p = self.product()?;
        let code = p.key.rsplit('\\').next().unwrap_or_default();
        let msiexec = crate::sys::system_tool("msiexec").display().to_string();
        // MSI: o código do produto é o nome da chave.
        let msi = || code.starts_with('{').then(|| (msiexec.clone(), format!("/x {code}{}", if quiet { " /quiet /norestart" } else { "" })));
        let text = |v: &str| reg::text(Hive::Machine, &p.key, v).filter(|s| !s.trim().is_empty());
        let cmd = if quiet { text("QuietUninstallString") } else { None }.or_else(|| text("UninstallString"));
        match cmd {
            Some(c) if !c.to_ascii_lowercase().contains("msiexec") => Some(split_command(&c)),
            _ => msi(),
        }
    }
}

/// A versão (`tag_name`) da resposta da API do GitHub, sem o `v`.
fn tag(json: &str) -> Option<String> {
    let v = json.split("\"tag_name\"").nth(1)?.split('"').nth(1)?;
    Some(v.trim_start_matches(['v', 'V']).to_string())
}

/// O `.exe` x64 entre os arquivos da versão, só dos lançamentos oficiais do projeto.
pub fn installer_url(json: &str, repo: &str) -> Option<String> {
    let base = format!("https://github.com/{repo}/releases/download/");
    json.split("\"browser_download_url\"").skip(1).filter_map(|s| s.split('"').nth(1)).find(|u| {
        let l = u.to_ascii_lowercase();
        l.starts_with(&base) && l.ends_with(".exe") && l.contains("x64")
    }).map(str::to_string)
}

/// `a` é mais nova que `b` ("1.2.146" > "1.2.128.0"). Zeros no fim não contam.
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |s: &str| {
        let mut v: Vec<u32> = s.trim_start_matches(['v', 'V']).split('.').map(|p| p.trim().parse().unwrap_or(0)).collect();
        while v.last() == Some(&0) {
            v.pop();
        }
        v
    };
    parts(a) > parts(b)
}

/// Separa `"C:\x\setup.exe" /x {..}` (ou sem aspas) em programa e argumentos.
fn split_command(c: &str) -> (String, String) {
    let c = c.trim();
    if let Some(rest) = c.strip_prefix('"') {
        if let Some((exe, args)) = rest.split_once('"') {
            return (exe.to_string(), args.trim().to_string());
        }
    }
    match c.to_ascii_lowercase().find(".exe") {
        Some(i) => (c[..i + 4].to_string(), c[i + 4..].trim().to_string()),
        None => (c.to_string(), String::new()),
    }
}

// ---------------------------------------------------------------------------------------------
// O filtro do HidHide. Instalado, ele se põe em UpperFilters destas classes; sem isso o serviço
// roda e esconder não tem efeito. O reparo é reinstalar (caminho da Nefarius), nunca gravar aqui.

const HID_CLASS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{745a17a0-74d3-11d0-b6fe-00a0c90f57da}";

/// A lista UpperFilters (REG_MULTI_SZ) tem o HidHide.
pub fn has_filter(multi_sz: &[u16]) -> bool {
    crate::sys::hidhide::parse_multi_sz(multi_sz).iter().any(|s| s.eq_ignore_ascii_case("HidHide"))
}

/// O HidHide está instalado, mas o filtro não está na classe dos controles (lido sem administrador).
pub fn filter_missing() -> bool {
    static C: Mutex<Option<(std::time::Instant, bool)>> = Mutex::new(None);
    crate::sys::hidhide::cached(&C, read_filter_missing)
}

fn read_filter_missing() -> bool {
    use crate::sys::reg::{self, Hive};
    if !crate::sys::hidhide::installed() || !reg::exists(Hive::Machine, HID_CLASS) {
        return false;
    }
    let w: Vec<u16> = reg::read(Hive::Machine, HID_CLASS, "UpperFilters").map(|(_, b)| b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()).unwrap_or_default();
    !has_filter(&w)
}

// ---------------------------------------------------------------------------------------------
// A versão mais nova de cada um, vista uma vez por sessão, fora da janela.

/// `None`: ainda olhando. `Some(None)`: não deu para ver.
static LATEST: Mutex<Vec<(&'static str, Option<String>)>> = Mutex::new(Vec::new());

/// A versão mais nova no GitHub; a primeira chamada começa a olhar e devolve `None`.
pub fn latest(k: &'static Kit) -> Option<Option<String>> {
    let mut l = LATEST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, v)) = l.iter().find(|(n, _)| *n == k.name) {
        return v.clone().map(|s| (!s.is_empty()).then_some(s));
    }
    l.push((k.name, None));
    std::thread::spawn(move || {
        let v = k.latest().ok().map(|(v, _)| v).filter(|v| !v.is_empty());
        let mut l = LATEST.lock().unwrap_or_else(|e| e.into_inner());
        l.retain(|(n, _)| *n != k.name);
        l.push((k.name, v.or(Some(String::new()))));
    });
    None
}

/// Esquece a versão vista (depois de instalar ou remover, para olhar de novo).
pub fn forget() {
    LATEST.lock().unwrap_or_else(|e| e.into_inner()).clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare() {
        assert!(newer("1.2.146", "1.2.128"));
        assert!(newer("v1.22.0", "1.21.442.0"));
        assert!(!newer("1.2.128.0", "1.2.128"));
        assert!(!newer("1.2.128", "1.2.128.0"));
        assert!(!newer("1.17.333", "1.22.0"));
        assert!(newer("1.10", "1.9"));
    }

    #[test]
    fn installer_only_from_official_releases() {
        let json = r#"{"tag_name":"v1.22.0","assets":[{"browser_download_url":"https://evil.example/ViGEmBus_x64.exe"},{"browser_download_url":"https://github.com/nefarius/ViGEmBus/releases/download/v1.22.0/ViGEmBus_1.22.0_x64_x86_arm64.exe"}]}"#;
        assert_eq!(installer_url(json, VIGEMBUS.repo).as_deref(), Some("https://github.com/nefarius/ViGEmBus/releases/download/v1.22.0/ViGEmBus_1.22.0_x64_x86_arm64.exe"));
        // O instalador de um projeto não serve para o outro.
        assert_eq!(installer_url(json, HIDHIDE.repo), None);
        assert_eq!(installer_url(r#"{"browser_download_url":"https://evil.example/x64.exe"}"#, HIDHIDE.repo), None);
        let hh = r#"{"assets":[{"browser_download_url":"https://github.com/nefarius/HidHide/releases/download/v1.5.230.0/HidHide_1.5.230_x64.exe"}]}"#;
        assert_eq!(installer_url(hh, HIDHIDE.repo).as_deref(), Some("https://github.com/nefarius/HidHide/releases/download/v1.5.230.0/HidHide_1.5.230_x64.exe"));
        assert_eq!(tag(json).as_deref(), Some("1.22.0"));
    }

    #[test]
    fn filter_missing_from_multi_sz() {
        use crate::sys::hidhide::multi_sz;
        let s = |v: &[&str]| multi_sz(&v.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert!(has_filter(&s(&["xusb22", "HidHide"])));
        assert!(has_filter(&s(&["hidhide"])));
        assert!(!has_filter(&s(&["xusb22"])));
        assert!(!has_filter(&s(&["HidHideX"])));
        assert!(!has_filter(&[]));
    }

    #[test]
    fn uninstall_command_split() {
        assert_eq!(split_command(r#""C:\ProgramData\x\setup.exe" /x {AB}"#), (r"C:\ProgramData\x\setup.exe".into(), "/x {AB}".into()));
        assert_eq!(split_command(r"C:\x\un.exe /S"), (r"C:\x\un.exe".into(), "/S".into()));
    }
}
