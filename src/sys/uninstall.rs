//! Desinstalador em lote com limpeza de restos.
//! Regra de segurança: só vem marcado o que tem o nome exato do programa, nada fora
//! da lista é tocado, arquivos vão para a Lixeira e o registro ganha backup antes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::JobObjects::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Shell::*;

const UNINSTALL: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
const UNINSTALL32: &str = "Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall";

#[derive(Clone, Debug)]
pub struct Program {
    pub hive: isize,
    pub key: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub size: u64,
    pub date: String,
    pub location: String,
    pub icon: String,
    pub uninstall: String,
    pub quiet: String,
    pub inno: bool,
    pub on: bool,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_key(root: HKEY, sub: &str) -> Option<HKEY> {
    unsafe {
        let mut k = HKEY::default();
        let s = wide(sub);
        RegOpenKeyExW(root, PCWSTR(s.as_ptr()), None, KEY_READ | KEY_WOW64_64KEY, &mut k).is_ok().then_some(k)
    }
}

fn subkeys(k: HKEY) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let mut i = 0;
        loop {
            let mut name = [0u16; 512];
            let mut len = name.len() as u32;
            if RegEnumKeyExW(k, i, Some(PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None).is_err() {
                break;
            }
            out.push(String::from_utf16_lossy(&name[..len as usize]));
            i += 1;
        }
    }
    out
}

fn text(k: HKEY, name: &str) -> String {
    unsafe {
        let n = wide(name);
        let mut buf = [0u16; 2048];
        let mut size = (buf.len() * 2) as u32;
        let r = RegGetValueW(
            k,
            None,
            PCWSTR(n.as_ptr()),
            RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        );
        if r.is_err() {
            return String::new();
        }
        let end = buf.iter().position(|c| *c == 0).unwrap_or(0);
        String::from_utf16_lossy(&buf[..end])
    }
}

fn dword(k: HKEY, name: &str) -> u32 {
    unsafe {
        let n = wide(name);
        let mut v = 0u32;
        let mut size = 4u32;
        let r = RegGetValueW(k, None, PCWSTR(n.as_ptr()), RRF_RT_REG_DWORD, None, Some(&mut v as *mut _ as *mut _), Some(&mut size));
        if r.is_err() {
            0
        } else {
            v
        }
    }
}

/// A data no registro não tem formato único. Deixa em AAAAMMDD; se for ambígua, fica sem data.
fn normal_date(s: &str) -> String {
    let s = s.trim();
    if s.len() == 8 && s.bytes().all(|b| b.is_ascii_digit()) {
        return s.to_string();
    }
    let t: Vec<&str> = s.split(|c: char| !c.is_ascii_alphanumeric()).filter(|p| !p.is_empty()).collect();
    let num = |p: &str| p.parse::<u32>().ok();
    if t.len() >= 3 && t[0].len() == 4 {
        if let (Some(y), Some(m), Some(d)) = (num(t[0]), num(t[1]), num(t[2])) {
            return format!("{y:04}{m:02}{d:02}");
        }
    }
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let month = t.iter().position(|p| MONTHS.iter().any(|m| p.eq_ignore_ascii_case(m)));
    if let Some(i) = month {
        let m = MONTHS.iter().position(|n| t[i].eq_ignore_ascii_case(n)).unwrap_or(0) + 1;
        let year = t.iter().find(|p| p.len() == 4).and_then(|p| num(p));
        if let (Some(y), Some(d)) = (year, t.get(i + 1).and_then(|p| num(p))) {
            return format!("{y:04}{m:02}{d:02}");
        }
    }
    String::new()
}

pub fn list() -> Vec<Program> {
    let mut out: Vec<Program> = Vec::new();
    for (root, base) in [
        (HKEY_LOCAL_MACHINE, UNINSTALL),
        (HKEY_LOCAL_MACHINE, UNINSTALL32),
        (HKEY_CURRENT_USER, UNINSTALL),
    ] {
        let Some(k) = open_key(root, base) else { continue };
        for sub in subkeys(k) {
            let path = format!("{base}\\{sub}");
            let Some(e) = open_key(root, &path) else { continue };
            let name = text(e, "DisplayName");
            let uninstall = text(e, "UninstallString");
            let hidden = dword(e, "SystemComponent") == 1 || !text(e, "ParentKeyName").is_empty();
            let inno = !text(e, "Inno Setup: Setup Version").is_empty();
            if !name.is_empty() && !uninstall.is_empty() && !hidden {
                let mut location = text(e, "InstallLocation").trim_end_matches('\\').to_string();
                let icon = text(e, "DisplayIcon").split(',').next().unwrap_or("").trim_matches('"').to_string();
                if location.is_empty() && icon.to_ascii_lowercase().ends_with(".exe") {
                    location = Path::new(&icon).parent().map(|p| p.display().to_string()).unwrap_or_default();
                }
                out.push(Program {
                    hive: root.0 as isize,
                    key: path,
                    publisher: text(e, "Publisher"),
                    version: text(e, "DisplayVersion"),
                    size: dword(e, "EstimatedSize") as u64 * 1024,
                    date: normal_date(&text(e, "InstallDate")),
                    quiet: text(e, "QuietUninstallString"),
                    location,
                    icon,
                    uninstall,
                    inno,
                    name,
                    on: false,
                });
            }
            unsafe {
                let _ = RegCloseKey(e);
            }
        }
        unsafe {
            let _ = RegCloseKey(k);
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    out.dedup_by(|a, b| a.name == b.name && a.version == b.version);
    out
}

fn msi_guid(cmd: &str) -> Option<String> {
    let a = cmd.find('{')?;
    let b = cmd[a..].find('}')?;
    Some(cmd[a..a + b + 1].to_string())
}

/// Comando final: sem janela sempre que o instalador permitir.
pub fn command(p: &Program, silent: bool) -> String {
    let low = p.uninstall.to_ascii_lowercase();
    if low.contains("msiexec") {
        if let Some(g) = msi_guid(&p.uninstall) {
            return if silent { format!("msiexec.exe /x {g} /qn /norestart") } else { format!("msiexec.exe /x {g}") };
        }
    }
    if silent {
        if !p.quiet.is_empty() {
            return p.quiet.clone();
        }
        if p.inno {
            return format!("{} /VERYSILENT /SUPPRESSMSGBOXES /NORESTART", p.uninstall);
        }
    }
    p.uninstall.clone()
}

/// Roda e espera também os processos filhos, porque muitos desinstaladores se copiam.
fn run_and_wait(cmd: &str) -> bool {
    unsafe {
        let Ok(job) = CreateJobObjectW(None, None) else { return false };
        let mut w = wide(cmd);
        let si = STARTUPINFOW { cb: std::mem::size_of::<STARTUPINFOW>() as u32, ..Default::default() };
        let mut pi = PROCESS_INFORMATION::default();
        let ok = CreateProcessW(None, Some(PWSTR(w.as_mut_ptr())), None, None, false, CREATE_SUSPENDED, None, None, &si, &mut pi);
        if ok.is_err() {
            let _ = CloseHandle(job);
            return false;
        }
        let _ = AssignProcessToJobObject(job, pi.hProcess);
        ResumeThread(pi.hThread);
        let t0 = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
            let r = QueryInformationJobObject(
                Some(job),
                JobObjectBasicAccountingInformation,
                &mut info as *mut _ as *mut _,
                std::mem::size_of_val(&info) as u32,
                None,
            );
            if r.is_err() || info.ActiveProcesses == 0 || t0.elapsed() > Duration::from_secs(1800) {
                break;
            }
        }
        let _ = CloseHandle(pi.hThread);
        let _ = CloseHandle(pi.hProcess);
        let _ = CloseHandle(job);
        true
    }
}

#[repr(C, packed)]
struct RestoreInfo {
    event: u32,
    kind: u32,
    seq: i64,
    desc: [u16; 256],
}

#[repr(C, packed)]
struct RestoreStatus {
    status: u32,
    seq: i64,
}

type SrSet = unsafe extern "system" fn(*const RestoreInfo, *mut RestoreStatus) -> BOOL;

/// Pede o ponto. Devolve a sequência para fechar depois, ou None quando o Windows recusa.
pub fn restore_begin(desc: &str) -> Option<i64> {
    unsafe {
        let lib = LoadLibraryW(w!("srclient.dll")).ok()?;
        let f: SrSet = std::mem::transmute(GetProcAddress(lib, s!("SRSetRestorePointW"))?);
        let mut text = [0u16; 256];
        for (i, c) in desc.encode_utf16().take(250).enumerate() {
            text[i] = c;
        }
        let i = RestoreInfo { event: 100, kind: 1, seq: 0, desc: text };
        let mut st = RestoreStatus { status: 0, seq: 0 };
        f(&i, &mut st).as_bool().then_some(st.seq)
    }
}

pub fn restore_end(seq: i64) {
    unsafe {
        let Ok(lib) = LoadLibraryW(w!("srclient.dll")) else { return };
        let Some(p) = GetProcAddress(lib, s!("SRSetRestorePointW")) else { return };
        let f: SrSet = std::mem::transmute(p);
        let i = RestoreInfo { event: 101, kind: 1, seq, desc: [0; 256] };
        let mut st = RestoreStatus { status: 0, seq: 0 };
        let _ = f(&i, &mut st);
    }
}

/// Cria um ponto de restauração de verdade: o Windows limita a um por dia e a API
/// pode dizer sucesso sem criar, então libera o limite, cria, confere e devolve.
pub fn restore_point(desc: &str) -> bool {
    use crate::sys::reg::{self, Hive};
    let key = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion\\SystemRestore";
    let count = || {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(crate::sys::system_tool("powershell"))
            .args(["-NoProfile", "-Command", "(Get-ComputerRestorePoint | Measure-Object).Count"])
            .creation_flags(0x0800_0000)
            .output()
            .ok()
            .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().ok())
            .unwrap_or(0)
    };
    let before = count();
    let old = reg::dword(Hive::Machine, key, "SystemRestorePointCreationFrequency");
    reg::write_dword(Hive::Machine, key, "SystemRestorePointCreationFrequency", 0);
    let seq = restore_begin(desc);
    if let Some(s) = seq {
        restore_end(s);
    }
    match old {
        Some(v) => {
            reg::write_dword(Hive::Machine, key, "SystemRestorePointCreationFrequency", v);
        }
        None => {
            reg::delete(Hive::Machine, key, "SystemRestorePointCreationFrequency");
        }
    }
    seq.is_some() && count() > before
}

#[derive(Clone, Debug, PartialEq)]
pub enum LeftKind {
    Folder,
    File,
    Registry(isize, String),
}

#[derive(Clone, Debug)]
pub struct Leftover {
    pub program: String,
    pub label: String,
    pub kind: LeftKind,
    pub size: u64,
    pub on: bool,
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Nome sem versão e sem arquitetura: "Foo Bar 1.2 (x64)" vira "Foo Bar".
fn clean_name(n: &str) -> String {
    let base = n.split('(').next().unwrap_or(n);
    let mut parts: Vec<&str> = base.split_whitespace().collect();
    while let Some(l) = parts.last() {
        let low = l.to_ascii_lowercase();
        let ver = l.trim_start_matches(['v', 'V']).chars().next().is_some_and(|c| c.is_ascii_digit());
        if ver || matches!(low.as_str(), "x64" | "x86" | "64-bit" | "32-bit" | "edition") {
            parts.pop();
        } else {
            break;
        }
    }
    parts.join(" ")
}

fn clean_publisher(p: &str) -> String {
    let mut v: Vec<&str> = p.split([' ', ',']).filter(|s| !s.is_empty()).collect();
    while let Some(l) = v.last() {
        let low = l.trim_end_matches('.').to_ascii_lowercase();
        if matches!(low.as_str(), "inc" | "llc" | "ltd" | "limited" | "corporation" | "corp" | "gmbh" | "co" | "software") {
            v.pop();
        } else {
            break;
        }
    }
    v.join(" ")
}

const NEVER: &[&str] = &[
    "windows", "microsoft", "commonfiles", "system32", "programfiles", "programdata", "users", "nvidia",
    "nvidiacorporation", "intel", "amd", "google", "packages", "temp", "cache", "steam", "installer", "common",
];

fn too_generic(n: &str) -> bool {
    n.len() < 4 || NEVER.contains(&n)
}

fn folder_size(p: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            if m.is_dir() {
                stack.push(e.path());
            } else {
                total += m.len();
            }
        }
    }
    total
}

fn roots() -> Vec<PathBuf> {
    let mut v = Vec::new();
    // Pelo Windows, e não pelas variáveis: o que aparece aqui pode ir para a Lixeira.
    for id in [FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86, FOLDERID_ProgramData, FOLDERID_RoamingAppData, FOLDERID_LocalAppData, FOLDERID_UserProgramFiles] {
        let p = super::known_folder(&id);
        if !p.as_os_str().is_empty() {
            v.push(p);
        }
    }
    v
}

fn starts_with_dir(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a == b || a.starts_with(&format!("{b}\\"))
}

/// Pasta tirada do ícone ou do desinstalador, para quem não informa onde foi instalado.
fn dir_of(cmd: &str) -> Option<PathBuf> {
    let c = cmd.trim();
    let path = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next()?.to_string()
    } else {
        c.split(',').next()?.split(" /").next()?.to_string()
    };
    let p = Path::new(&path);
    if !p.is_absolute() {
        return None;
    }
    let dir = p.parent()?;
    let low = dir.to_string_lossy().to_ascii_lowercase();
    if low.contains("\\windows\\") || low.contains("\\installshield") || low.contains("package cache") || dir.parent().is_none() {
        return None;
    }
    Some(dir.to_path_buf())
}

fn stem_of(cmd: &str) -> Option<String> {
    let c = cmd.trim().trim_start_matches('"');
    let end = c.to_ascii_lowercase().find(".exe")?;
    let name = Path::new(&c[..end]).file_name()?.to_string_lossy().to_string();
    let n = norm(&name);
    (!n.starts_with("unins") && !n.starts_with("setup") && !n.starts_with("update")).then_some(n)
}

/// Lugares onde programas costumam deixar pasta com o próprio nome.
fn deep_roots() -> Vec<PathBuf> {
    let mut v = roots();
    let h = super::known_folder(&FOLDERID_Profile);
    if !h.as_os_str().is_empty() {
        for s in ["AppData\\LocalLow", "Documents", "Documents\\My Games", "Saved Games", ""] {
            v.push(h.join(s));
        }
    }
    v
}

/// Procura o que sobrou. `others` são os programas que seguem instalados.
/// Nome exato vem marcado. Nome parecido aparece desmarcado, para a pessoa decidir.
pub fn scan(p: &Program, others: &[Program]) -> Vec<Leftover> {
    let mut out: Vec<Leftover> = Vec::new();
    let name = clean_name(&p.name);
    let nn = norm(&name);
    let pub_clean = clean_publisher(&p.publisher);
    let np = norm(&pub_clean);
    if too_generic(&nn) {
        return out;
    }
    let shared = |path: &str| {
        others.iter().any(|o| {
            !o.location.is_empty() && (starts_with_dir(&o.location, path) || starts_with_dir(path, &o.location))
        })
    };
    let other_names: Vec<String> = others.iter().map(|o| norm(&clean_name(&o.name))).filter(|n| n.len() >= 4).collect();
    let taken = |f: &str| other_names.iter().any(|o| f.starts_with(o.as_str()) || o.starts_with(f));
    let publisher_alone = !others.iter().any(|o| norm(&clean_publisher(&o.publisher)) == np);

    // Outros nomes que o mesmo programa usa: sem a empresa na frente, a pasta e o executável.
    let dir = if !p.location.is_empty() { Some(PathBuf::from(&p.location)) } else { dir_of(&p.icon).or_else(|| dir_of(&p.uninstall)) };
    let leaf = dir.as_ref().and_then(|d| d.file_name()).map(|f| norm(&f.to_string_lossy())).unwrap_or_default();
    let mut names = vec![nn.clone()];
    if !np.is_empty() && nn.len() > np.len() + 3 {
        if let Some(rest) = nn.strip_prefix(&np) {
            names.push(rest.to_string());
        }
    }
    names.push(leaf.clone());
    let stem = stem_of(&p.icon).or_else(|| stem_of(&p.uninstall)).unwrap_or_default();
    names.push(stem.clone());
    names.retain(|n| !too_generic(n) && !taken(n));
    names.dedup();
    // 0 não tem relação, 1 parece e 2 é o nome exato.
    let grade = |f: &str| -> u8 {
        if f == nn || (!leaf.is_empty() && f == leaf && !too_generic(&leaf)) {
            2
        } else if too_generic(f) || f.len() < 5 || taken(f) {
            0
        } else if names.iter().any(|n| f == n || (n.len() >= 5 && (f.starts_with(n.as_str()) || n.starts_with(f)))) {
            1
        } else {
            0
        }
    };
    let mut add = |path: PathBuf, on: bool| {
        let s = path.display().to_string();
        if out.iter().any(|l| l.label.eq_ignore_ascii_case(&s)) || shared(&s) {
            return;
        }
        let dir = path.is_dir();
        if !dir && !path.is_file() {
            return;
        }
        let size = if dir { folder_size(&path) } else { path.metadata().map(|m| m.len()).unwrap_or(0) };
        out.push(Leftover {
            program: p.name.clone(),
            label: s,
            kind: if dir { LeftKind::Folder } else { LeftKind::File },
            size,
            on,
        });
    };
    if let Some(d) = &dir {
        if d.is_dir() && !too_generic(&leaf) {
            add(d.clone(), grade(&leaf) == 2);
        }
    }
    for root in deep_roots() {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for e in rd.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            let f = norm(&e.file_name().to_string_lossy());
            match grade(&f) {
                2 => add(e.path(), true),
                1 => add(e.path(), false),
                _ if !np.is_empty() && f == np && !too_generic(&np) => {
                    let Ok(inner) = std::fs::read_dir(e.path()) else { continue };
                    let mut kids = 0;
                    for i in inner.flatten() {
                        kids += 1;
                        match grade(&norm(&i.file_name().to_string_lossy())) {
                            2 => add(i.path(), true),
                            1 => add(i.path(), false),
                            _ => {}
                        }
                    }
                    if kids <= 1 && publisher_alone {
                        add(e.path(), false);
                    }
                }
                _ => {}
            }
        }
    }
    let mut menus = Vec::new();
    for (base, sub) in [("APPDATA", "Microsoft\\Windows\\Start Menu\\Programs"), ("PROGRAMDATA", "Microsoft\\Windows\\Start Menu\\Programs"), ("USERPROFILE", "Desktop"), ("PUBLIC", "Desktop")] {
        if let Ok(b) = std::env::var(base) {
            menus.push(PathBuf::from(b).join(sub));
        }
    }
    for menu in menus {
        let Ok(rd) = std::fs::read_dir(&menu) else { continue };
        for e in rd.flatten() {
            let stem = Path::new(&e.file_name()).file_stem().map(|s| norm(&s.to_string_lossy())).unwrap_or_default();
            match grade(&stem) {
                2 => add(e.path(), true),
                1 => add(e.path(), false),
                _ => {}
            }
        }
    }
    let mut regs: Vec<(HKEY, String, bool)> = vec![(HKEY(p.hive as *mut _), p.key.clone(), true)];
    if !stem.is_empty() {
        for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
            let base = "Software\\Microsoft\\Windows\\CurrentVersion\\App Paths";
            let Some(k) = open_key(root, base) else { continue };
            for sub in subkeys(k) {
                if norm(sub.to_ascii_lowercase().trim_end_matches(".exe")) == stem {
                    regs.push((root, format!("{base}\\{sub}"), true));
                }
            }
            unsafe {
                let _ = RegCloseKey(k);
            }
        }
    }
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for base in ["Software", "Software\\WOW6432Node"] {
            let Some(k) = open_key(root, base) else { continue };
            for sub in subkeys(k) {
                let n = norm(&sub);
                match grade(&n) {
                    2 => regs.push((root, format!("{base}\\{sub}"), true)),
                    1 => regs.push((root, format!("{base}\\{sub}"), false)),
                    _ if !np.is_empty() && n == np && !too_generic(&np) => {
                        let path = format!("{base}\\{sub}");
                        if let Some(pk) = open_key(root, &path) {
                            let inner = subkeys(pk);
                            for s2 in &inner {
                                match grade(&norm(s2)) {
                                    2 => regs.push((root, format!("{path}\\{s2}"), true)),
                                    1 => regs.push((root, format!("{path}\\{s2}"), false)),
                                    _ => {}
                                }
                            }
                            if inner.len() <= 1 && publisher_alone {
                                regs.push((root, path.clone(), false));
                            }
                            unsafe {
                                let _ = RegCloseKey(pk);
                            }
                        }
                    }
                    _ => {}
                }
            }
            unsafe {
                let _ = RegCloseKey(k);
            }
        }
    }
    for (root, path, on) in regs {
        let Some(k) = open_key(root, &path) else { continue };
        unsafe {
            let _ = RegCloseKey(k);
        }
        let hive = if root == HKEY_CURRENT_USER { "HKCU" } else { "HKLM" };
        let label = format!("{hive}\\{path}");
        if !out.iter().any(|l| l.label == label) {
            out.push(Leftover { program: p.name.clone(), label, kind: LeftKind::Registry(root.0 as isize, path), size: 0, on });
        }
    }
    out
}

/// Manda para a Lixeira. Se o disco não tiver Lixeira, o Windows avisa antes.
pub fn to_recycle(path: &str) -> bool {
    let mut from: Vec<u16> = path.encode_utf16().collect();
    from.push(0);
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO.0 | FOF_NOCONFIRMATION.0 | FOF_WANTNUKEWARNING.0 | FOF_SILENT.0 | FOF_NOERRORUI.0) as u16,
        ..Default::default()
    };
    unsafe { SHFileOperationW(&mut op) == 0 && !op.fAnyOperationsAborted.as_bool() }
}

/// Remove um resto. Arquivos vão para a Lixeira; chaves ganham um .reg antes.
pub fn remove(l: &Leftover, backups: &Path) -> bool {
    match &l.kind {
        LeftKind::Folder | LeftKind::File => to_recycle(&l.label),
        LeftKind::Registry(hive, path) => {
            let _ = std::fs::create_dir_all(backups);
            let safe: String = l.label.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).take(80).collect();
            let file = backups.join(format!("{safe}.reg"));
            let done = std::process::Command::new(crate::sys::system_tool("reg"))
                .args(["export", &l.label, &file.display().to_string(), "/y"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !done {
                return false;
            }
            unsafe {
                let s = wide(path);
                RegDeleteTreeW(HKEY(*hive as *mut _), PCWSTR(s.as_ptr())) == ERROR_SUCCESS
            }
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub enum Stage {
    #[default]
    Restore,
    Removing,
    Scanning,
}

#[derive(Clone, Default)]
pub struct Job {
    pub running: bool,
    pub stage: Stage,
    pub queue: Vec<String>,
    pub current: String,
    pub done: Vec<(String, bool)>,
    pub restore: Option<bool>,
    pub want_restore: bool,
    pub leftovers: Vec<Leftover>,
}

impl Job {
    /// Passo a passo da tela: texto, cor (0 normal, 1 destaque, 2 aviso, 3 apagada) e peso.
    pub fn lines(&self) -> Vec<(String, u8, bool)> {
        let mut v = Vec::new();
        match self.restore {
            Some(true) => v.push(("Ponto de restauração criado".to_string(), 0, false)),
            Some(false) => v.push(("O Windows recusou o ponto de restauração".to_string(), 2, false)),
            None if self.want_restore => v.push(("Criando ponto de restauração…".to_string(), 1, true)),
            None => {}
        }
        for n in &self.queue {
            if let Some(d) = self.done.iter().find(|d| &d.0 == n) {
                if d.1 {
                    v.push((format!("Desinstalado: {n}"), 0, false));
                } else {
                    v.push((format!("Não foi removido: {n}"), 2, false));
                }
            } else if *n == self.current {
                v.push((format!("Desinstalando {n}…"), 1, true));
            } else {
                v.push((format!("Na fila: {n}"), 3, false));
            }
        }
        if self.stage == Stage::Scanning {
            v.push(("Procurando restos…".to_string(), 1, true));
        }
        v
    }
}

pub type Shared = Arc<Mutex<Job>>;

/// A entrada sumiu da lista do Windows? Alguns desinstaladores demoram um pouco a tirar.
fn removed(p: &Program) -> bool {
    for _ in 0..10 {
        match open_key(HKEY(p.hive as *mut _), &p.key) {
            Some(k) => unsafe {
                let _ = RegCloseKey(k);
            },
            None => return true,
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    false
}

pub fn start(list: Vec<Program>, silent: bool, restore: bool, shared: Shared) {
    let queue: Vec<String> = list.iter().map(|p| p.name.clone()).collect();
    *shared.lock().unwrap() = Job { running: true, queue, want_restore: restore, ..Default::default() };
    let _ = std::thread::Builder::new().name("desinstalar".into()).spawn(move || {
        if restore {
            let ok = restore_point("Prisma: antes de desinstalar");
            shared.lock().unwrap().restore = Some(ok);
        }
        shared.lock().unwrap().stage = Stage::Removing;
        let mut gone = Vec::new();
        for p in &list {
            shared.lock().unwrap().current = p.name.clone();
            let ok = run_and_wait(&command(p, silent)) && removed(p);
            shared.lock().unwrap().done.push((p.name.clone(), ok));
            // Quem cancelou o desinstalador continua com o programa: nada dele é resto.
            if ok {
                gone.push(p);
            }
        }
        {
            let mut j = shared.lock().unwrap();
            j.current.clear();
            j.stage = Stage::Scanning;
        }
        let others = list_all_except(&gone);
        let mut found = Vec::new();
        for p in &gone {
            found.extend(scan(p, &others));
        }
        let mut j = shared.lock().unwrap();
        j.leftovers = found;
        j.running = false;
    });
}

fn list_all_except(gone: &[&Program]) -> Vec<Program> {
    crate::sys::uninstall::list().into_iter().filter(|o| !gone.iter().any(|g| g.name == o.name)).collect()
}

/// Teste: o que seria mostrado se este programa tivesse saído. Só lê.
#[cfg(debug_assertions)]
pub fn scan_preview(name: &str) -> String {
    let all = list();
    let Some(p) = all.iter().find(|p| p.name.to_lowercase().contains(&name.to_lowercase())) else { return "nao achei".into() };
    let others: Vec<Program> = all.iter().filter(|o| o.name != p.name).cloned().collect();
    let mut s = format!("{} | {} | {}\n", p.name, p.location, p.icon);
    for l in scan(p, &others) {
        s += &format!("{} {} {}\n", if l.on { "[x]" } else { "[ ]" }, l.size >> 20, l.label);
    }
    s
}
