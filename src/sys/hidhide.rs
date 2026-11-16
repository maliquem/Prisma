//! Esconder o controle físico do jogo com o HidHide (Nefarius, MIT), driver de terceiro e opcional.
//!
//! O HidHide tem duas listas: dispositivos bloqueados e programas que ainda veem os bloqueados.
//! O Prisma nunca sobrescreve as listas: lê, acrescenta só o que falta e anota o que acrescentou em
//! `hidhide.txt` antes de gravar. Desfazer tira exatamente o anotado. Se o Prisma cair, a próxima
//! abertura desfaz (`limpar`). Sem o driver, nada aqui faz nada.
//!
//! A ponte (VIIPER) chama `esconder(id)` ao ligar e `mostrar(id)` ao desligar; `id` é o
//! `Pad::key`. Esconde só se a pessoa ligou o interruptor do controle na aba Controles.

use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum List {
    /// Programas que continuam vendo os dispositivos escondidos (whitelist).
    Apps,
    /// Dispositivos escondidos (blacklist), pelo caminho da instância (`HID\VID_...`).
    Devices,
}

/// O que o Prisma precisa do driver. A versão de verdade é `Device`; os testes usam uma falsa.
pub trait Driver {
    fn get(&mut self, l: List) -> Result<Vec<String>, ()>;
    /// Grava a lista inteira (o driver substitui a anterior).
    fn set(&mut self, l: List, v: &[String]) -> Result<(), ()>;
    fn active(&mut self) -> Result<bool, ()>;
    fn set_active(&mut self, on: bool) -> Result<(), ()>;
}

/// O que o Prisma acrescentou ao HidHide, gravado antes de cada mudança.
#[derive(Default, Clone, PartialEq, Debug)]
pub struct Ledger {
    pub apps: Vec<String>,
    pub devs: Vec<String>,
    /// O HidHide estava desligado e o Prisma ligou.
    pub turned_on: bool,
    /// O driver foi instalado pelo Prisma: só então a desinstalação do Prisma remove ele.
    pub installed: bool,
}

impl Ledger {
    pub fn parse(s: &str) -> Ledger {
        let mut l = Ledger::default();
        for line in s.lines() {
            match line.split_once(' ') {
                Some(("app", v)) => l.apps.push(v.to_string()),
                Some(("dev", v)) => l.devs.push(v.to_string()),
                _ if line == "on" => l.turned_on = true,
                _ if line == "installed" => l.installed = true,
                _ => {}
            }
        }
        l
    }
    pub fn text(&self) -> String {
        let mut s = String::new();
        for a in &self.apps {
            s += &format!("app {a}\n");
        }
        for d in &self.devs {
            s += &format!("dev {d}\n");
        }
        if self.turned_on {
            s += "on\n";
        }
        if self.installed {
            s += "installed\n";
        }
        s
    }
    /// Nada do Prisma nas listas.
    pub fn clean(&self) -> bool {
        self.apps.is_empty() && self.devs.is_empty() && !self.turned_on
    }
}

/// Caminhos e instâncias valem sem diferença de maiúsculas, como no Windows.
fn has(v: &[String], s: &str) -> bool {
    v.iter().any(|x| x.eq_ignore_ascii_case(s))
}

fn without(v: Vec<String>, out: &[String]) -> Vec<String> {
    v.into_iter().filter(|x| !has(out, x)).collect()
}

/// Esconde `dev` e põe `app` (o Prisma) na lista de permitidos, acrescentando só o que falta.
/// `save` grava a anotação antes de cada gravação no driver: uma queda no meio não deixa resto sem nome.
pub fn hide(d: &mut dyn Driver, lg: &mut Ledger, dev: &str, app: &str, save: &mut dyn FnMut(&Ledger)) -> Result<(), ()> {
    let mut apps = d.get(List::Apps)?;
    let mut devs = d.get(List::Devices)?;
    // Primeiro o Prisma na lista de permitidos: ele nunca deixa de ler o controle.
    if !has(&apps, app) {
        lg.apps.push(app.to_string());
        save(lg);
        apps.push(app.to_string());
        d.set(List::Apps, &apps)?;
    }
    if !has(&devs, dev) {
        lg.devs.push(dev.to_string());
        save(lg);
        devs.push(dev.to_string());
        d.set(List::Devices, &devs)?;
    }
    if !d.active()? {
        lg.turned_on = true;
        save(lg);
        d.set_active(true)?;
    }
    Ok(())
}

/// Mostra `dev` de novo se foi o Prisma que escondeu. Sem nenhum escondido, desfaz o resto.
pub fn show(d: &mut dyn Driver, lg: &mut Ledger, dev: &str, save: &mut dyn FnMut(&Ledger)) -> Result<(), ()> {
    if !has(&lg.devs, dev) {
        return Ok(());
    }
    let mine: Vec<String> = lg.devs.iter().filter(|x| x.eq_ignore_ascii_case(dev)).cloned().collect();
    let devs = without(d.get(List::Devices)?, &mine);
    d.set(List::Devices, &devs)?;
    lg.devs.retain(|x| !x.eq_ignore_ascii_case(dev));
    save(lg);
    if lg.devs.is_empty() {
        undo(d, lg, save)?;
    }
    Ok(())
}

/// Tira das listas exatamente o que o Prisma acrescentou e devolve o liga-desliga como estava.
/// O que a pessoa ou outro programa acrescentou fica.
pub fn undo(d: &mut dyn Driver, lg: &mut Ledger, save: &mut dyn FnMut(&Ledger)) -> Result<(), ()> {
    if !lg.devs.is_empty() {
        let devs = without(d.get(List::Devices)?, &lg.devs);
        d.set(List::Devices, &devs)?;
        lg.devs.clear();
        save(lg);
    }
    if !lg.apps.is_empty() {
        let apps = without(d.get(List::Apps)?, &lg.apps);
        d.set(List::Apps, &apps)?;
        lg.apps.clear();
        save(lg);
    }
    if lg.turned_on {
        d.set_active(false)?;
        lg.turned_on = false;
        save(lg);
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// O driver de verdade.

/// Nome do dispositivo de controle do HidHide.
const CONTROL: &str = r"\\.\HidHide";

/// `CTL_CODE(0x8001, função, METHOD_BUFFERED, FILE_READ_DATA)`, como em `HidHide/HidHide.h`.
const fn ioctl(func: u32) -> u32 {
    (0x8001 << 16) | (1 << 14) | (func << 2)
}
const GET_WHITELIST: u32 = ioctl(2048);
const SET_WHITELIST: u32 = ioctl(2049);
const GET_BLACKLIST: u32 = ioctl(2050);
const SET_BLACKLIST: u32 = ioctl(2051);
const GET_ACTIVE: u32 = ioctl(2052);
const SET_ACTIVE: u32 = ioctl(2053);

/// Lista no formato do driver: texto UTF-16, cada item terminado em nulo e mais um nulo no fim.
pub fn multi_sz(v: &[String]) -> Vec<u16> {
    let mut out: Vec<u16> = v.iter().flat_map(|s| s.encode_utf16().chain(std::iter::once(0))).collect();
    out.push(0);
    if v.is_empty() {
        out.push(0);
    }
    out
}

pub fn parse_multi_sz(b: &[u16]) -> Vec<String> {
    b.split(|&c| c == 0).filter(|s| !s.is_empty()).map(String::from_utf16_lossy).collect()
}

pub struct Device(windows::Win32::Foundation::HANDLE);

impl Device {
    /// Abre o HidHide só para leitura, como a CLI dele (`FilterDriverProxy.cpp`): os IOCTLs pedem
    /// `FILE_READ_DATA`. `Err(true)` quando ele existe mas pede administrador; `Err(false)` sem o driver.
    pub fn open() -> Result<Device, bool> {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::*;
        use windows::Win32::Storage::FileSystem::*;
        let h = unsafe { CreateFileW(&HSTRING::from(CONTROL), GENERIC_READ.0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, None) };
        match h {
            Ok(h) => Ok(Device(h)),
            Err(e) => Err(e.code() == ERROR_ACCESS_DENIED.to_hresult()),
        }
    }

    fn call(&self, code: u32, input: &[u8], out: &mut [u8]) -> Result<usize, ()> {
        use windows::Win32::System::IO::DeviceIoControl;
        let mut n = 0u32;
        let inp = (!input.is_empty()).then(|| input.as_ptr() as *const _);
        let outp = (!out.is_empty()).then(|| out.as_mut_ptr() as *mut _);
        unsafe { DeviceIoControl(self.0, code, inp, input.len() as u32, outp, out.len() as u32, Some(&mut n as *mut u32), None) }.map_err(|_| ())?;
        Ok(n as usize)
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(self.0) };
    }
}

impl Driver for Device {
    fn get(&mut self, l: List) -> Result<Vec<String>, ()> {
        let code = if l == List::Apps { GET_WHITELIST } else { GET_BLACKLIST };
        // Primeiro o tamanho, depois a lista (como a CLI do HidHide); sem resposta, 64 KB bastam.
        let need = self.call(code, &[], &mut []).unwrap_or(65536);
        let mut buf = vec![0u8; need.max(4)];
        let n = self.call(code, &[], &mut buf)?;
        let wide: Vec<u16> = buf[..n].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Ok(parse_multi_sz(&wide))
    }
    fn set(&mut self, l: List, v: &[String]) -> Result<(), ()> {
        let code = if l == List::Apps { SET_WHITELIST } else { SET_BLACKLIST };
        let bytes: Vec<u8> = multi_sz(v).iter().flat_map(|c| c.to_le_bytes()).collect();
        self.call(code, &bytes, &mut []).map(|_| ())
    }
    fn active(&mut self) -> Result<bool, ()> {
        let mut b = [0u8; 1];
        self.call(GET_ACTIVE, &[], &mut b)?;
        Ok(b[0] != 0)
    }
    fn set_active(&mut self, on: bool) -> Result<(), ()> {
        self.call(SET_ACTIVE, &[on as u8], &mut []).map(|_| ())
    }
}

// ---------------------------------------------------------------------------------------------
// Estado do Prisma e o que o resto do app chama.

fn ledger_path() -> std::path::PathBuf {
    crate::config::data_dir().join("hidhide.txt")
}

/// A aba desenha a cada quadro: o arquivo e o driver são lidos no máximo uma vez por segundo.
fn cached<T: Clone>(slot: &Mutex<Option<(std::time::Instant, T)>>, f: impl FnOnce() -> T) -> T {
    let mut c = slot.lock().unwrap_or_else(|e| e.into_inner());
    match &*c {
        Some((t, v)) if t.elapsed() < std::time::Duration::from_secs(1) => v.clone(),
        _ => {
            let v = f();
            *c = Some((std::time::Instant::now(), v.clone()));
            v
        }
    }
}

static LEDGER: Mutex<Option<(std::time::Instant, Ledger)>> = Mutex::new(None);

pub fn ledger() -> Ledger {
    cached(&LEDGER, || Ledger::parse(&std::fs::read_to_string(ledger_path()).unwrap_or_default()))
}

fn save(l: &Ledger) {
    *LEDGER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let p = ledger_path();
    if l.clean() && !l.installed {
        let _ = std::fs::remove_file(p);
    } else {
        let _ = std::fs::create_dir_all(p.parent().unwrap_or(&p));
        let _ = std::fs::write(p, l.text());
    }
}

/// O HidHide está instalado (o dispositivo de controle existe, mesmo pedindo administrador).
pub fn installed() -> bool {
    static C: Mutex<Option<(std::time::Instant, bool)>> = Mutex::new(None);
    cached(&C, || matches!(Device::open(), Ok(_) | Err(true)))
}

/// Caminho do Prisma no formato que o HidHide guarda (`\Device\HarddiskVolumeN\...`).
fn own_image() -> Option<String> {
    use windows::Win32::System::Threading::*;
    let mut buf = [0u16; 1024];
    let mut n = buf.len() as u32;
    unsafe { QueryFullProcessImageNameW(GetCurrentProcess(), PROCESS_NAME_NATIVE, windows::core::PWSTR(buf.as_mut_ptr()), &mut n) }.ok()?;
    Some(String::from_utf16_lossy(&buf[..n as usize]))
}

/// Caminho da instância (`HID\VID_054C&PID_0CE6\...`) do controle, pelo caminho que o Prisma abre.
/// `xinput:N` não tem: o controle Xbox lido pelo XInput fica de fora.
pub fn instance(key: &str) -> Option<String> {
    use windows::core::HSTRING;
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    if key.starts_with("xinput:") {
        return None;
    }
    // O caminho do controle não muda de instância: guarda o que já achou.
    static SEEN: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    if let Some((_, i)) = SEEN.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| k == key) {
        return Some(i.clone());
    }
    let found = unsafe {
        let set = SetupDiCreateDeviceInfoList(None, None).ok()?;
        let mut iface = SP_DEVICE_INTERFACE_DATA { cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32, ..Default::default() };
        let mut info = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
        let mut buf = [0u16; 512];
        let mut n = 0u32;
        let mut ok = SetupDiOpenDeviceInterfaceW(set, &HSTRING::from(key), 0, Some(&mut iface as *mut _)).is_ok();
        if ok {
            // Sem buffer a chamada "falha" por tamanho, mas já preenche `info`, que é o que importa.
            let _ = SetupDiGetDeviceInterfaceDetailW(set, &iface, None, 0, None, Some(&mut info as *mut _));
            ok = SetupDiGetDeviceInstanceIdW(set, &info, Some(&mut buf), Some(&mut n as *mut u32)).is_ok();
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
        ok.then(|| String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1)]))
    }?;
    SEEN.lock().unwrap_or_else(|e| e.into_inner()).push((key.to_string(), found.clone()));
    Some(found)
}

/// Controles que a ponte está usando agora (pelo `Pad::key`).
static BRIDGED: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Uma operação no driver, com a anotação. Sem administrador, pede pela cópia elevada.
fn run(op: &str) -> bool {
    // A cópia de administrador grava a anotação por fora: relê depois.
    *LEDGER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let ok = match Device::open() {
        Ok(mut d) => apply(&mut d, op),
        Err(true) => crate::startup::run_elevated_wait(&elevated_arg(op)) == Some(0),
        Err(false) => false,
    };
    *LEDGER.lock().unwrap_or_else(|e| e.into_inner()) = None;
    ok
}

/// O argumento da cópia de administrador, entre aspas: o caminho da instância tem `&` e pode ter espaço.
fn elevated_arg(op: &str) -> String {
    format!("\"--hidhide={op}\"")
}

/// Reinicia o aparelho. O HidHide só barra quem abre o controle depois: quem já estava com ele
/// aberto (a Steam na bandeja) continuava vendo. Reiniciado, todos fecham e, ao abrir de novo, o
/// HidHide decide; ao mostrar, o jogo nota o controle voltando. Pede administrador, como o driver.
fn restart(instance: &str) {
    use windows::core::HSTRING;
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    unsafe {
        let Ok(set) = SetupDiCreateDeviceInfoList(None, None) else { return };
        let mut info = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
        if SetupDiOpenDeviceInfoW(set, &HSTRING::from(instance), None, 0, Some(&mut info as *mut _)).is_ok() {
            let p = SP_PROPCHANGE_PARAMS {
                ClassInstallHeader: SP_CLASSINSTALL_HEADER { cbSize: std::mem::size_of::<SP_CLASSINSTALL_HEADER>() as u32, InstallFunction: DIF_PROPERTYCHANGE },
                StateChange: DICS_PROPCHANGE,
                Scope: DICS_FLAG_CONFIGSPECIFIC,
                HwProfile: 0,
            };
            if SetupDiSetClassInstallParamsW(set, Some(&info as *const _), Some(&p.ClassInstallHeader as *const _), std::mem::size_of::<SP_PROPCHANGE_PARAMS>() as u32).is_ok() {
                let _ = SetupDiCallClassInstaller(DIF_PROPERTYCHANGE, set, Some(&info as *const _));
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(set);
    }
}

/// Aparelhos que a operação mudou: o da operação, ou todos os anotados no `undo`.
fn touched(op: &str, before: &[String]) -> Vec<String> {
    match op.split_once(':') {
        Some((_, dev)) => vec![dev.to_string()],
        None => before.to_vec(),
    }
}

/// `hide:<instância>`, `show:<instância>` ou `undo`.
fn apply(d: &mut dyn Driver, op: &str) -> bool {
    let mut lg = ledger();
    let before = lg.devs.clone();
    let r = match op.split_once(':') {
        Some(("hide", dev)) => match own_image() {
            Some(app) => hide(d, &mut lg, dev, &app, &mut save),
            None => Err(()),
        },
        Some(("show", dev)) => show(d, &mut lg, dev, &mut save),
        _ if op == "undo" => undo(d, &mut lg, &mut save),
        _ => Err(()),
    };
    save(&lg);
    if r.is_ok() {
        touched(op, &before).iter().for_each(|dev| restart(dev));
    }
    r.is_ok()
}

/// `--hidhide=<op>`: a cópia de administrador faz a operação e sai (0 deu certo).
pub fn run_arg() -> Option<i32> {
    let op = std::env::args().find_map(|a| a.strip_prefix("--hidhide=").map(str::to_string))?;
    let ok = match Device::open() {
        Ok(mut d) => apply(&mut d, &op),
        Err(_) => false,
    };
    Some(if ok { 0 } else { 3 })
}

/// A ponte ligou para este controle: esconde do jogo se a pessoa quis (interruptor ligado).
pub fn esconder(id: &str) -> bool {
    let mut b = BRIDGED.lock().unwrap_or_else(|e| e.into_inner());
    if !b.iter().any(|k| k == id) {
        b.push(id.to_string());
    }
    drop(b);
    !crate::config::Config::load().pad_hide.iter().any(|k| k == id) || set_hidden(id, true)
}

/// A ponte desligou para este controle: ele volta a aparecer para o jogo.
pub fn mostrar(id: &str) -> bool {
    BRIDGED.lock().unwrap_or_else(|e| e.into_inner()).retain(|k| k != id);
    set_hidden(id, false)
}

/// A ponte está usando este controle.
pub fn bridged(id: &str) -> bool {
    BRIDGED.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|k| k == id)
}

/// O Prisma escondeu este controle (está na anotação).
pub fn hidden(id: &str) -> bool {
    instance(id).is_some_and(|i| has(&ledger().devs, &i))
}

/// Esconde ou mostra pelo `Pad::key`. Esconder só vale com a ponte ligada para ele.
pub fn set_hidden(id: &str, on: bool) -> bool {
    let Some(dev) = instance(id) else { return false };
    if on && !bridged(id) {
        return false;
    }
    if !on && !has(&ledger().devs, &dev) {
        return true;
    }
    run(&format!("{}:{dev}", if on { "hide" } else { "show" }))
}

/// Desfaz tudo o que o Prisma acrescentou. Também na abertura (o Prisma pode ter caído) e ao fechar.
pub fn limpar() -> bool {
    ledger().clean() || run("undo")
}

/// Trabalho fora da janela (a confirmação de administrador e a instalação esperam a pessoa).
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static MESSAGE: Mutex<Option<&'static str>> = Mutex::new(None);

/// Roda `f` numa thread, uma de cada vez; a mensagem que ela devolver vira aviso na aba Controles.
pub fn background(f: impl FnOnce() -> Option<&'static str> + Send + 'static) {
    use std::sync::atomic::Ordering;
    if BUSY.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        let m = f();
        *MESSAGE.lock().unwrap_or_else(|e| e.into_inner()) = m;
        BUSY.store(false, Ordering::Release);
    });
}

pub fn busy() -> bool {
    BUSY.load(std::sync::atomic::Ordering::Acquire)
}

pub fn take_message() -> Option<&'static str> {
    MESSAGE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

// ---------------------------------------------------------------------------------------------
// Instalar e remover o driver, da fonte oficial e só com consentimento.

const RELEASES: &str = "https://api.github.com/repos/nefarius/HidHide/releases/latest";
/// Quem assina o instalador oficial.
const SIGNER: &str = "Nefarius Software Solutions";

/// Baixa o instalador da última versão no GitHub do HidHide, confere a assinatura e abre o
/// instalador oficial, com a tela dele (o projeto não documenta modo silencioso). Anota que foi o
/// Prisma que instalou. Roda fora da janela (leva tempo). `Ok(true)`: o Windows pede reinício.
pub fn install() -> Result<bool, &'static str> {
    let json = crate::sys::update::download(RELEASES).map_err(|_| "Não consegui falar com o GitHub do HidHide")?;
    let json = String::from_utf8_lossy(&json);
    let url = installer_url(&json).ok_or("Não achei o instalador na versão mais nova do HidHide")?;
    let data = crate::sys::update::download(&url).map_err(|_| "Não consegui baixar o HidHide")?;
    let file = std::env::temp_dir().join("HidHide_setup.exe");
    std::fs::write(&file, data).map_err(|_| "Não consegui gravar o instalador")?;
    let path = file.display().to_string();
    match crate::sys::fileinfo::check(&path) {
        crate::sys::fileinfo::Signature::Valid(n) if n.contains(SIGNER) => {}
        _ => {
            let _ = std::fs::remove_file(&file);
            return Err("O instalador baixado não tem a assinatura da Nefarius");
        }
    }
    let ok = crate::startup::run_file_elevated_wait(&file, "", true).and_then(crate::startup::install_ok);
    let _ = std::fs::remove_file(&file);
    let Some(reboot) = ok else {
        return Err("O HidHide não foi instalado");
    };
    let mut lg = ledger();
    lg.installed = true;
    save(&lg);
    Ok(reboot)
}

/// O `.exe` x64 entre os arquivos da versão (resposta da API do GitHub).
pub fn installer_url(json: &str) -> Option<String> {
    json.split("\"browser_download_url\"").skip(1).filter_map(|s| s.split('"').nth(1)).find(|u| {
        let l = u.to_ascii_lowercase();
        l.starts_with("https://github.com/nefarius/hidhide/releases/download/") && l.ends_with(".exe") && l.contains("x64")
    }).map(str::to_string)
}

/// Desinstalação do Prisma: tira o que ele acrescentou e remove o driver só se foi ele que instalou.
pub fn uninstall() {
    if let Ok(mut d) = Device::open() {
        let mut lg = ledger();
        let _ = undo(&mut d, &mut lg, &mut |_: &Ledger| {});
    }
    if ledger().installed {
        if let Some(cmd) = uninstall_command() {
            use std::os::windows::process::CommandExt;
            // `start /wait`: o msiexec é programa de janela e o cmd não esperaria por ele.
            let _ = std::process::Command::new(crate::sys::system_tool("cmd")).raw_arg(format!("/c start \"\" /wait {cmd}")).output();
        }
    }
    let _ = std::fs::remove_file(ledger_path());
}

/// Comando silencioso de remoção que o instalador do HidHide registrou no Windows.
fn uninstall_command() -> Option<String> {
    use crate::sys::reg::{self, Hive};
    const KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    let text = |k: &str, v: &str| reg::read(Hive::Machine, k, v).map(|(_, b)| {
        let w: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&w).trim_end_matches('\0').to_string()
    });
    reg::subkeys(Hive::Machine, KEY).into_iter().find_map(|s| {
        let k = format!(r"{KEY}\{s}");
        let ours = text(&k, "DisplayName")?.starts_with("HidHide") && text(&k, "Publisher").is_some_and(|p| p.contains("Nefarius"));
        ours.then_some(())?;
        // MSI: o código do produto é o nome da chave.
        Some(text(&k, "QuietUninstallString").unwrap_or_else(|| format!("msiexec /x {s} /quiet /norestart")))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O HidHide de mentira: as duas listas e o liga-desliga, e um defeito opcional.
    #[derive(Default)]
    struct Fake {
        apps: Vec<String>,
        devs: Vec<String>,
        on: bool,
        /// Falha a gravação de número `n` (simula o Prisma caindo no meio).
        fail_at: Option<usize>,
        writes: usize,
    }

    impl Driver for Fake {
        fn get(&mut self, l: List) -> Result<Vec<String>, ()> {
            Ok(if l == List::Apps { self.apps.clone() } else { self.devs.clone() })
        }
        fn set(&mut self, l: List, v: &[String]) -> Result<(), ()> {
            self.writes += 1;
            if self.fail_at == Some(self.writes) {
                return Err(());
            }
            *(if l == List::Apps { &mut self.apps } else { &mut self.devs }) = v.to_vec();
            Ok(())
        }
        fn active(&mut self) -> Result<bool, ()> {
            Ok(self.on)
        }
        fn set_active(&mut self, on: bool) -> Result<(), ()> {
            self.on = on;
            Ok(())
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    const APP: &str = r"\Device\HarddiskVolume3\Prisma\prisma.exe";
    const DS: &str = r"HID\VID_054C&PID_0CE6&MI_03\8&1";

    #[test]
    fn hide_adds_without_overwriting() {
        let mut d = Fake { apps: s(&[r"\Device\HarddiskVolume3\Steam\steam.exe"]), devs: s(&[r"HID\VID_045E&PID_0B13\1"]), ..Default::default() };
        let mut lg = Ledger::default();
        hide(&mut d, &mut lg, DS, APP, &mut |_: &Ledger| {}).unwrap();
        assert_eq!(d.apps, s(&[r"\Device\HarddiskVolume3\Steam\steam.exe", APP]));
        assert_eq!(d.devs, s(&[r"HID\VID_045E&PID_0B13\1", DS]));
        assert!(d.on);
        assert_eq!(lg, Ledger { apps: s(&[APP]), devs: s(&[DS]), turned_on: true, installed: false });
    }

    #[test]
    fn entries_already_there_are_not_claimed() {
        // A pessoa já tinha escondido o controle e posto o Prisma na lista: desfazer não tira nada.
        let mut d = Fake { apps: s(&[&APP.to_lowercase()]), devs: s(&[DS]), on: true, ..Default::default() };
        let mut lg = Ledger::default();
        hide(&mut d, &mut lg, DS, APP, &mut |_: &Ledger| {}).unwrap();
        assert!(lg.clean());
        undo(&mut d, &mut lg, &mut |_: &Ledger| {}).unwrap();
        assert_eq!((d.apps.len(), d.devs.len(), d.on), (1, 1, true));
    }

    #[test]
    fn undo_restores_previous_state() {
        let before = Fake { apps: s(&["a"]), devs: s(&["d"]), ..Default::default() };
        let mut d = Fake { apps: before.apps.clone(), devs: before.devs.clone(), ..Default::default() };
        let mut lg = Ledger::default();
        hide(&mut d, &mut lg, DS, APP, &mut |_: &Ledger| {}).unwrap();
        hide(&mut d, &mut lg, "HID\\OTHER\\2", APP, &mut |_: &Ledger| {}).unwrap();
        // Outro programa mexe nas listas no meio: o que ele pôs fica.
        d.devs.push("x".into());
        undo(&mut d, &mut lg, &mut |_: &Ledger| {}).unwrap();
        assert_eq!(d.apps, before.apps);
        assert_eq!(d.devs, s(&["d", "x"]));
        assert!(!d.on);
        assert!(lg.clean());
    }

    #[test]
    fn show_last_device_releases_everything() {
        let mut d = Fake::default();
        let mut lg = Ledger::default();
        hide(&mut d, &mut lg, DS, APP, &mut |_: &Ledger| {}).unwrap();
        hide(&mut d, &mut lg, "B", APP, &mut |_: &Ledger| {}).unwrap();
        show(&mut d, &mut lg, DS, &mut |_: &Ledger| {}).unwrap();
        assert_eq!(d.devs, s(&["B"]));
        assert_eq!(d.apps, s(&[APP]));
        show(&mut d, &mut lg, "b", &mut |_: &Ledger| {}).unwrap();
        assert!(d.devs.is_empty() && d.apps.is_empty() && !d.on);
        // Mostrar o que o Prisma não escondeu não mexe em nada.
        d.devs.push("Z".into());
        show(&mut d, &mut lg, "Z", &mut |_: &Ledger| {}).unwrap();
        assert_eq!(d.devs, s(&["Z"]));
    }

    #[test]
    fn crash_mid_hide_is_cleaned_next_start() {
        // Cai na segunda gravação (a lista de dispositivos): a anotação já tem o dispositivo.
        let mut d = Fake { apps: s(&["a"]), fail_at: Some(2), ..Default::default() };
        let mut saved = Ledger::default();
        let mut lg = Ledger::default();
        assert!(hide(&mut d, &mut lg, DS, APP, &mut |l: &Ledger| saved = l.clone()).is_err());
        // Próxima abertura: lê a anotação do arquivo e desfaz.
        let mut lg = Ledger::parse(&saved.text());
        assert_eq!(lg.devs, s(&[DS]));
        d.fail_at = None;
        undo(&mut d, &mut lg, &mut |_: &Ledger| {}).unwrap();
        assert_eq!(d.apps, s(&["a"]));
        assert!(d.devs.is_empty() && !d.on);
    }

    #[test]
    fn ledger_round_trip() {
        let l = Ledger { apps: s(&[APP]), devs: s(&[DS, "B"]), turned_on: true, installed: true };
        assert_eq!(Ledger::parse(&l.text()), l);
        assert!(Ledger::parse("").clean());
    }

    #[test]
    fn multi_sz_round_trip() {
        let v = s(&[APP, DS]);
        let w = multi_sz(&v);
        assert_eq!(&w[w.len() - 2..], &[0, 0]);
        assert_eq!(parse_multi_sz(&w), v);
        assert_eq!(multi_sz(&[]), vec![0, 0]);
        assert!(parse_multi_sz(&[0, 0]).is_empty());
    }

    #[test]
    fn installer_only_from_official_releases() {
        let json = r#"{"assets":[{"browser_download_url":"https://evil.example/HidHide_x64.exe"},{"browser_download_url":"https://github.com/nefarius/HidHide/releases/download/v1.5.230.0/HidHide_1.5.230_x64.exe"}]}"#;
        assert_eq!(installer_url(json).as_deref(), Some("https://github.com/nefarius/HidHide/releases/download/v1.5.230.0/HidHide_1.5.230_x64.exe"));
        assert_eq!(installer_url(r#"{"browser_download_url":"https://evil.example/x64.exe"}"#), None);
    }

    #[test]
    fn elevated_arg_keeps_instance_whole() {
        let op = r"hide:HID\{00001124-0000-1000-8000-00805F9B34FB}_VID&0002054C_PID&0CE6\9&2 A&0&0000";
        assert_eq!(elevated_arg(op), format!("\"--hidhide={op}\""));
    }

    #[test]
    fn restart_what_changed() {
        let before = s(&[r"HID\A\1", r"HID\B\2"]);
        assert_eq!(touched(r"hide:HID\VID_054C&PID_0CE6\1", &before), s(&[r"HID\VID_054C&PID_0CE6\1"]));
        assert_eq!(touched("undo", &before), before);
    }
}
