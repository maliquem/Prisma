//! Tela virtual opcional para o Compartilhar tela: o Virtual Display Driver (VirtualDrivers,
//! MikeTheTech, MIT, assinado pela SignPath), baixado da release oficial só depois do consentimento
//! na tela, como a ponte do controle Xbox (`xbridge.rs`). Nada dele vai dentro do Prisma.
//!
//! - Instala com o NefCon (Nefarius, MIT), como o `Community Scripts/silent-install.ps1` do VDD:
//!   os certificados do `.cat` em Editores Confiáveis e `nefconw install MttVDD.inf Root\MttVDD`.
//! - Quando alguém conecta com a opção ligada, a tela virtual entra com o tamanho e os Hz de quem
//!   assiste (o modo vai no `C:\VirtualDisplayDriver\vdd_settings.xml` e o driver recarrega pelo
//!   pipe `\\.\pipe\MTTVirtualDisplayPipe`). No modo privacidade, só ela fica ligada.
//! - Tudo pelo `SetDisplayConfig` sem gravar no banco do Windows: ao sair (ou no próximo início do
//!   Prisma, se ele cair), o Windows volta ao arranjo salvo, com a tela virtual desligada.
//! - Remove tudo pelo próprio Prisma (`start_remove`). Notas do que não deu para confirmar sem o
//!   driver: `docs/GUIA.md`, seção da tela virtual.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Gdi::*;

pub use super::xbridge::Stage;

const VERSION: &str = "25.7.23";
const ASSET: &str = "VirtualDisplayDriver-x86.Driver.Only.zip";
/// Somas do GitHub (`digest` da API) das duas releases, fixas no código.
const ASSET_SUM: &str = "e24210692b442b39af763536330ce78b423f19342b7a7792c26de3944e418b3a";
const NEFCON: &str = "https://github.com/nefarius/nefcon/releases/download/v1.14.0/nefcon_v1.14.0.zip";
const NEFCON_SUM: &str = "a15557da24a9efca203158de3b43b0eaf982db231f0194031f1ed428bc13e669";
const PIPE: &str = r"\\.\pipe\MTTVirtualDisplayPipe";
const SETTINGS: &str = r"C:\VirtualDisplayDriver\vdd_settings.xml";

// ---------- Configuração do driver ----------

/// O XML com o modo `w`x`h` a `hz` na lista e uma tela só. `None`: já estava tudo lá.
pub fn with_mode(xml: Option<&str>, w: u32, h: u32, hz: u32) -> Option<String> {
    let Some(xml) = xml.filter(|x| x.contains("<resolutions>") && x.contains("</resolutions>")) else {
        return Some(format!(
            "<?xml version='1.0' encoding='utf-8'?>\n<vdd_settings>\n    <monitors>\n        <count>1</count>\n    </monitors>\n    <resolutions>\n{}    </resolutions>\n</vdd_settings>\n",
            entry(w, h, hz)
        ));
    };
    let mut out = xml.to_string();
    // Uma tela basta: com zero o driver não sobe, com mais aparecem telas que ninguém vê.
    if let (Some(a), Some(b)) = (out.find("<count>"), out.find("</count>")) {
        if a < b && out[a + 7..b].trim() != "1" {
            out.replace_range(a + 7..b, "1");
        }
    }
    let has = |x: &str| {
        x.split("<resolution>").skip(1).any(|r| tag(r, "width") == Some(w) && tag(r, "height") == Some(h) && tag(r, "refresh_rate") == Some(hz))
    };
    if !has(&out) {
        let at = out.find("</resolutions>").unwrap_or(out.len());
        out.insert_str(at, &format!("{}    ", entry(w, h, hz).trim_start()));
    }
    (out != xml).then_some(out)
}

fn entry(w: u32, h: u32, hz: u32) -> String {
    format!("        <resolution>\n            <width>{w}</width>\n            <height>{h}</height>\n            <refresh_rate>{hz}</refresh_rate>\n        </resolution>\n")
}

fn tag(x: &str, name: &str) -> Option<u32> {
    let a = x.find(&format!("<{name}>"))? + name.len() + 2;
    let b = a + x[a..].find(&format!("</{name}>"))?;
    x[a..b].trim().parse().ok()
}

/// Manda um comando ao driver (uma conexão por mensagem, em UTF-16) e lê a resposta, se houver.
fn pipe(cmd: &str, answer: bool) -> Option<String> {
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(PIPE).ok()?;
    let msg: Vec<u8> = cmd.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    f.write_all(&msg).ok()?;
    if !answer {
        return Some(String::new());
    }
    // O PING volta em bytes simples ("PONG"), não em UTF-16 (`SendToPipe` no Driver.cpp).
    let mut buf = [0u8; 64];
    let n = f.read(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf[..n]).into_owned())
}

// ---------- Telas ----------

fn wide(s: &[u16]) -> String {
    String::from_utf16_lossy(&s[..s.iter().position(|c| *c == 0).unwrap_or(s.len())])
}

/// As saídas do VDD pelo nome GDI (`\\.\DISPLAYn`), ligadas ou não.
fn vdd_outputs() -> Vec<String> {
    let mut out = Vec::new();
    for i in 0.. {
        let mut d = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut d, 0) }.as_bool() {
            break;
        }
        let id = wide(&d.DeviceID).to_ascii_lowercase();
        let name = wide(&d.DeviceString).to_ascii_lowercase();
        // shortcut: o id de hardware é `Root\MttVDD` no INF; o nome cobre o caso de o Windows mostrar outro id.
        if id.contains("mttvdd") || name.contains("virtual display driver") {
            out.push(wide(&d.DeviceName));
        }
    }
    out
}

/// O driver está carregado (a saída dele aparece no Windows).
pub fn present() -> bool {
    !vdd_outputs().is_empty() || pipe("PING", true).is_some_and(|s| s.starts_with("PONG"))
}

fn query(flags: QUERY_DISPLAY_CONFIG_FLAGS) -> Option<(Vec<DISPLAYCONFIG_PATH_INFO>, Vec<DISPLAYCONFIG_MODE_INFO>)> {
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        GetDisplayConfigBufferSizes(flags, &mut np, &mut nm).ok().ok()?;
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        QueryDisplayConfig(flags, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None).ok().ok()?;
        paths.truncate(np as usize);
        modes.truncate(nm as usize);
        Some((paths, modes))
    }
}

/// Nome GDI da origem de um caminho.
fn source_gdi(adapter: LUID, id: u32) -> String {
    let mut s = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
    s.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
    s.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
    s.header.adapterId = adapter;
    s.header.id = id;
    if unsafe { DisplayConfigGetDeviceInfo(&mut s.header) } != 0 {
        return String::new();
    }
    wide(&s.viewGdiDeviceName)
}

const PATH_ACTIVE: u32 = 1;
const MODE_INVALID: u32 = 0xffff_ffff;

/// Liga a saída do VDD (sozinha, com `alone`) sem gravar no banco do Windows. Devolve o nome GDI
/// e o alvo (placa e id, para o HDR).
fn attach(alone: bool) -> Option<(String, LUID, u32)> {
    let outs = vdd_outputs();
    let (active, modes) = query(QDC_ONLY_ACTIVE_PATHS)?;
    let is_vdd = |p: &DISPLAYCONFIG_PATH_INFO| outs.iter().any(|o| o.eq_ignore_ascii_case(&source_gdi(p.sourceInfo.adapterId, p.sourceInfo.id)));
    if let Some(p) = active.iter().find(|p| is_vdd(p)) {
        if !alone {
            return Some((source_gdi(p.sourceInfo.adapterId, p.sourceInfo.id), p.targetInfo.adapterId, p.targetInfo.id));
        }
    }
    let (all, _) = query(QDC_ALL_PATHS)?;
    let mut p = *all.iter().find(|p| p.targetInfo.targetAvailable.as_bool() && is_vdd(p))?;
    p.flags |= PATH_ACTIVE;
    p.sourceInfo.Anonymous.modeInfoIdx = MODE_INVALID;
    p.targetInfo.Anonymous.modeInfoIdx = MODE_INVALID;
    let gdi = source_gdi(p.sourceInfo.adapterId, p.sourceInfo.id);
    let mut paths: Vec<_> = if alone { Vec::new() } else { active.into_iter().filter(|q| !is_vdd(q)).collect() };
    paths.push(p);
    let modes = if alone { None } else { Some(&modes[..]) };
    let r = unsafe { SetDisplayConfig(Some(&paths), modes, SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES) };
    (r == 0).then_some((gdi, p.targetInfo.adapterId, p.targetInfo.id))
}

/// Põe o modo na saída, sem gravar (vale até o Windows voltar ao arranjo salvo).
fn set_mode(gdi: &str, w: u32, h: u32, hz: u32) -> bool {
    let name = windows::core::HSTRING::from(gdi);
    let dm = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        dmFields: DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY,
        dmPelsWidth: w,
        dmPelsHeight: h,
        dmDisplayFrequency: hz,
        ..Default::default()
    };
    unsafe { ChangeDisplaySettingsExW(&name, Some(&dm), None, CDS_TYPE(0), None) == DISP_CHANGE_SUCCESSFUL }
}

/// A saída já tem este modo na lista do driver.
fn has_mode(gdi: &str, w: u32, h: u32, hz: u32) -> bool {
    let name = windows::core::HSTRING::from(gdi);
    (0..).map_while(|i| {
        let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        unsafe { EnumDisplaySettingsW(&name, ENUM_DISPLAY_SETTINGS_MODE(i), &mut dm) }.as_bool().then_some(dm)
    })
    .any(|dm| dm.dmPelsWidth == w && dm.dmPelsHeight == h && dm.dmDisplayFrequency == hz)
}

/// Números do monitor nas opções do host (por aparelho): a tela virtual, e ela sozinha com as
/// telas daqui apagadas. Longe da contagem dos monitores de verdade.
pub const VIRTUAL: u32 = 1000;
pub const PRIVATE: u32 = 1001;

/// O monitor da tela virtual ligada agora.
static ON: Mutex<Option<String>> = Mutex::new(None);

/// Para o host: o monitor da tela virtual, se ela estiver ligada nesta sessão.
pub fn monitor() -> Option<HMONITOR> {
    let gdi = ON.lock().unwrap().clone()?;
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut windows::Win32::Foundation::RECT, l: windows::Win32::Foundation::LPARAM) -> windows::core::BOOL {
        let (gdi, found) = &mut *(l.0 as *mut (&str, Option<HMONITOR>));
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(m, &mut mi.monitorInfo).as_bool() && wide(&mi.szDevice).eq_ignore_ascii_case(*gdi) {
            *found = Some(m);
            return false.into();
        }
        true.into()
    }
    let mut ctx = (gdi.as_str(), None);
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), windows::Win32::Foundation::LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx.1
}

/// Uma sessão com a tela virtual: liga ao criar, volta ao arranjo salvo ao soltar.
pub struct Virtual(());

fn mark() -> PathBuf {
    crate::config::data_dir().join("compartilhar").join("tela_virtual.txt")
}

impl Virtual {
    /// Para o host: liga quando o monitor escolhido para este aparelho é a tela virtual.
    pub fn for_monitor(monitor: u32, w: u32, h: u32, hz: u32, hdr: bool) -> Option<Self> {
        matches!(monitor, VIRTUAL | PRIVATE).then(|| Self::start(w, h, hz, monitor == PRIVATE, hdr)).flatten()
    }

    /// Liga a tela virtual no modo de quem assiste. `alone`: as telas daqui apagam (privacidade).
    /// `hdr`: tenta ligar o HDR nela. `None` quando o driver não está ou o Windows recusou.
    pub fn start(w: u32, h: u32, hz: u32, alone: bool, hdr: bool) -> Option<Self> {
        if !present() {
            crate::config::log("tela virtual: o driver não está carregado; segue o monitor de sempre");
            return None;
        }
        let (w, h) = if w >= 640 && h >= 480 { (w & !1, h & !1) } else { (1920, 1080) };
        let hz = if hz > 1 { hz.min(240) } else { 60 };
        // O modo tem de estar na lista do driver: entra no XML e o driver recarrega.
        let old = std::fs::read_to_string(SETTINGS).ok();
        if let Some(new) = with_mode(old.as_deref(), w, h, hz) {
            let _ = std::fs::create_dir_all(Path::new(SETTINGS).parent().unwrap());
            if std::fs::write(SETTINGS, new).is_ok() {
                let _ = pipe("RELOAD_DRIVER", false);
                std::thread::sleep(Duration::from_millis(1500));
            }
        }
        if std::fs::create_dir_all(mark().parent().unwrap()).and_then(|_| std::fs::write(mark(), "1")).is_err() {
            crate::config::log("tela virtual: sem a marca de volta no disco; não liga");
            return None;
        }
        let me = Virtual(());
        let mut gdi = None;
        for _ in 0..10 {
            gdi = attach(alone);
            if gdi.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let Some((gdi, adapter, target)) = gdi else {
            crate::config::log("tela virtual: o Windows não ligou a saída do VDD");
            return None;
        };
        for _ in 0..10 {
            if has_mode(&gdi, w, h, hz) {
                break;
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        if !set_mode(&gdi, w, h, hz) {
            crate::config::log(&format!("tela virtual: o modo {w}x{h} a {hz} Hz não entrou; segue o que o Windows escolheu"));
        }
        // O VDD anuncia HDR10 só com HDRPlus/SDR10bit no XML; se não der, segue em SDR.
        if hdr && !super::hdr::set_hdr(adapter, target, true) {
            crate::config::log("tela virtual: o HDR não ligou nela; segue em SDR");
        }
        crate::config::log(&format!("tela virtual: ligada em {gdi}, {w}x{h} a {hz} Hz{}", if alone { ", só ela (privacidade)" } else { "" }));
        *ON.lock().unwrap() = Some(gdi);
        Some(me)
    }
}

impl Drop for Virtual {
    fn drop(&mut self) {
        *ON.lock().unwrap() = None;
        restore();
    }
}

/// O Windows volta ao arranjo salvo (as telas daqui, sem a virtual). No início do Prisma, só se
/// uma sessão caiu com a tela virtual ligada.
pub fn restore() {
    if !mark().exists() {
        return;
    }
    let r = unsafe { SetDisplayConfig(None, None, SDC_APPLY | SDC_USE_DATABASE_CURRENT) };
    if r == 0 {
        let _ = std::fs::remove_file(mark());
    }
    detach_saved();
}

/// Tira a tela virtual do arranjo salvo do Windows. Logo depois de instalar o Windows liga a tela
/// nova sozinho e guarda assim; aqui ela sai e fica desligada até alguém conectar.
fn detach_saved() {
    let outs = vdd_outputs();
    let Some((active, modes)) = query(QDC_ONLY_ACTIVE_PATHS) else { return };
    let is_vdd = |p: &DISPLAYCONFIG_PATH_INFO| outs.iter().any(|o| o.eq_ignore_ascii_case(&source_gdi(p.sourceInfo.adapterId, p.sourceInfo.id)));
    let rest: Vec<_> = active.iter().filter(|p| !is_vdd(p)).copied().collect();
    if rest.len() == active.len() || rest.is_empty() {
        return;
    }
    let r = unsafe { SetDisplayConfig(Some(&rest), Some(&modes), SDC_APPLY | SDC_USE_SUPPLIED_DISPLAY_CONFIG | SDC_ALLOW_CHANGES | SDC_SAVE_TO_DATABASE) };
    crate::config::log(&format!("tela virtual: tirada do arranjo salvo ({})", if r == 0 { "ok" } else { "o Windows recusou" }));
}

// ---------- Instalação ----------

/// O script que roda como administrador: confere as somas de novo, abre os zips numa pasta do
/// sistema, põe os certificados do driver em Editores Confiáveis e instala.
pub fn install_script(vdd_zip: &Path, nefcon_zip: &Path) -> String {
    let q = |p: &Path| super::quick::ps_quote(&p.to_string_lossy());
    format!(
        "$ErrorActionPreference='Stop'\n\
function Sum($p){{(Get-FileHash -LiteralPath $p -Algorithm SHA256).Hash.ToLower()}}\n\
if((Sum {v}) -ne '{vs}' -or (Sum {n}) -ne '{ns}'){{exit 5}}\n\
$d=Join-Path $env:SystemRoot ('Temp\\prisma-vdd-'+[guid]::NewGuid().ToString('N'))\n\
Expand-Archive -LiteralPath {n} -DestinationPath $d\n\
Expand-Archive -LiteralPath {v} -DestinationPath $d\n\
$cs=New-Object System.Security.Cryptography.X509Certificates.X509Certificate2Collection\n\
$cs.Import([IO.File]::ReadAllBytes((Join-Path $d 'VirtualDisplayDriver\\mttvdd.cat')))\n\
$st=New-Object System.Security.Cryptography.X509Certificates.X509Store('TrustedPublisher','LocalMachine')\n\
$st.Open('ReadWrite'); foreach($c in $cs){{$st.Add($c)}}; $st.Close()\n\
& (Join-Path $d 'x64\\nefconw.exe') install (Join-Path $d 'VirtualDisplayDriver\\MttVDD.inf') 'Root\\MttVDD'\n\
$r=$LASTEXITCODE\n\
Remove-Item -LiteralPath $d -Recurse -Force -ErrorAction SilentlyContinue\n\
exit $r\n",
        v = q(vdd_zip),
        n = q(nefcon_zip),
        vs = ASSET_SUM,
        ns = NEFCON_SUM,
    )
}

/// O script da remoção: o aparelho, os certificados que vieram no `.cat`, o driver e a pasta.
pub const REMOVE_SCRIPT: &str = "$ErrorActionPreference='Continue'\n\
foreach($x in Get-PnpDevice | Where-Object { $_.HardwareID -contains 'Root\\MttVDD' }){ pnputil /remove-device $x.InstanceId | Out-Null }\n\
foreach($x in Get-WindowsDriver -Online | Where-Object { $_.OriginalFileName -like '*\\mttvdd.inf' }){\n\
  $cat=Join-Path (Split-Path $x.OriginalFileName) 'mttvdd.cat'\n\
  if(Test-Path -LiteralPath $cat){\n\
    $cs=New-Object System.Security.Cryptography.X509Certificates.X509Certificate2Collection\n\
    $cs.Import([IO.File]::ReadAllBytes($cat))\n\
    $st=New-Object System.Security.Cryptography.X509Certificates.X509Store('TrustedPublisher','LocalMachine')\n\
    $st.Open('ReadWrite'); foreach($c in $cs){$st.Remove($c)}; $st.Close()\n\
  }\n\
  pnputil /delete-driver $x.Driver /uninstall /force | Out-Null\n\
}\n\
Remove-Item -LiteralPath 'C:\\VirtualDisplayDriver' -Recurse -Force -ErrorAction SilentlyContinue\n\
if(Get-PnpDevice | Where-Object { $_.HardwareID -contains 'Root\\MttVDD' }){exit 1}\n\
exit 0\n";

fn elevated(script: &str) -> Result<(), String> {
    super::xbridge::shell_wait(&super::system_tool("powershell").to_string_lossy(), &format!("-NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}", super::quick::utf16_b64(script)), true)
}

fn fetch(url: &str, sum: &str, dir: &Path, name: &str) -> Result<PathBuf, String> {
    let data = super::update::download(url).map_err(|_| "o download falhou".to_string())?;
    if super::update::sha256(&data).as_deref() != Some(sum) {
        return Err("o arquivo baixado não confere com a release oficial".into());
    }
    let p = dir.join(name);
    std::fs::write(&p, data).map_err(|e| e.to_string())?;
    Ok(p)
}

fn install() -> Stage {
    use std::os::windows::fs::OpenOptionsExt;
    use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;
    let dir = std::env::temp_dir().join("prisma-vdd");
    let _ = std::fs::create_dir_all(&dir);
    set_stage(Stage::Downloading);
    let got = fetch(&format!("https://github.com/VirtualDrivers/Virtual-Display-Driver/releases/download/{VERSION}/{ASSET}"), ASSET_SUM, &dir, ASSET)
        .and_then(|v| fetch(NEFCON, NEFCON_SUM, &dir, "nefcon.zip").map(|n| (v, n)));
    let (v, n) = match got {
        Ok(x) => x,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&dir);
            return Stage::Failed(e);
        }
    };
    set_stage(Stage::Installing);
    // Presos sem escrita nem troca da conferência até o fim; o script confere as somas de novo.
    let lock = |p: &Path| std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ.0).open(p);
    let r = match (lock(&v), lock(&n)) {
        (Ok(a), Ok(b)) => {
            let r = elevated(&install_script(&v, &n));
            drop((a, b));
            r
        }
        _ => Err("não deu para abrir o que foi baixado".into()),
    };
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = r {
        return Stage::Failed(e);
    }
    for _ in 0..30 {
        if present() {
            std::thread::sleep(Duration::from_secs(2));
            detach_saved();
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
    g.get_or_insert_with(|| if present() { Stage::Ready } else { Stage::Absent }).clone()
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

/// Remove o driver, os certificados dele e a pasta de configuração.
pub fn start_remove() {
    if stage().busy() {
        return;
    }
    set_stage(Stage::Removing);
    std::thread::spawn(|| {
        restore();
        let r = elevated(REMOVE_SCRIPT);
        set_stage(match r {
            Ok(()) if !present() => Stage::Absent,
            Ok(()) => Stage::Reboot,
            Err(e) => Stage::Failed(e),
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modo_entra_uma_vez_e_conta_vira_um() {
        let xml = "<vdd_settings>\n    <monitors>\n        <count>2</count>\n    </monitors>\n    <resolutions>\n        <resolution>\n            <width>1920</width>\n            <height>1080</height>\n            <refresh_rate>60</refresh_rate>\n        </resolution>\n    </resolutions>\n    <colour><HDRPlus>false</HDRPlus></colour>\n</vdd_settings>\n";
        let new = with_mode(Some(xml), 2560, 1440, 144).unwrap();
        assert!(new.contains("<count>1</count>"));
        assert!(new.contains("<width>2560</width>") && new.contains("<refresh_rate>144</refresh_rate>"));
        // O resto do arquivo da pessoa fica.
        assert!(new.contains("<HDRPlus>false</HDRPlus>") && new.contains("<width>1920</width>"));
        assert_eq!(new.matches("<resolution>").count(), 2);
        // Já está lá: nada muda.
        assert_eq!(with_mode(Some(&new), 2560, 1440, 144), None);
        assert_eq!(with_mode(Some(&new), 1920, 1080, 60), None);
        // Sem arquivo (ou quebrado): um novo, só com o modo.
        let fresh = with_mode(None, 1280, 720, 60).unwrap();
        assert_eq!(tag(&fresh, "count"), Some(1));
        assert!(with_mode(Some(&fresh), 1280, 720, 60).is_none());
    }

    #[test]
    fn script_confere_as_somas_e_aspas() {
        let s = install_script(Path::new(r"C:\Users\O'Neil\AppData\Local\Temp\prisma-vdd\a.zip"), Path::new(r"C:\t\n.zip"));
        assert!(s.contains("'C:\\Users\\O''Neil\\AppData\\Local\\Temp\\prisma-vdd\\a.zip'"));
        assert!(s.contains(ASSET_SUM) && s.contains(NEFCON_SUM));
        assert!(s.contains("install (Join-Path $d 'VirtualDisplayDriver\\MttVDD.inf') 'Root\\MttVDD'"));
        assert!(REMOVE_SCRIPT.contains("pnputil /delete-driver"));
    }
}
