//! Shift+Del do Prisma: apaga para sempre, sem a Lixeira, em várias threads. A janela fica em
//! `src/erase.rs`, num prisma.exe à parte; aqui ficam o motor, a recusa das pastas do sistema e o histórico.
//!
//! Nada segue junção nem link: cada item é aberto pelo nome dentro da pasta mãe já aberta
//! (`NtCreateFile` com `RootDirectory` e `FILE_OPEN_REPARSE_POINT`). Uma junção é apagada como link,
//! e trocar uma pasta por junção no meio do trabalho não leva a exclusão para outro lugar.
//! Contas e grupos vão sempre por SID, e nada depende do texto do Windows: funciona em qualquer idioma.

use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex};

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::*;

pub const COUNTING: u8 = 0;
pub const DELETING: u8 = 1;
pub const DONE: u8 = 2;
pub const CANCELED: u8 = 3;
/// Sem administrador, algo ficou por permissão: a janela pede a cópia de administrador.
pub const NEED_ADMIN: u8 = 4;
/// A cópia de administrador continuou o trabalho.
pub const HANDED: u8 = 5;

/// O que está sendo feito num item que o Windows negou (`State::step`).
pub const STEP_OWNER: u8 = 1;
pub const STEP_GRANT: u8 = 2;
pub const STEP_PLAN_B: u8 = 3;

/// O que a janela lê enquanto o trabalho anda.
#[derive(Default)]
pub struct State {
    pub phase: AtomicU8,
    /// Arquivos e bytes achados na contagem; `counted` quando ela termina.
    pub files: AtomicU64,
    pub bytes: AtomicU64,
    pub counted: AtomicBool,
    /// Arquivos e bytes apagados.
    pub done: AtomicU64,
    pub freed: AtomicU64,
    /// Itens marcados para apagar no próximo reinício.
    pub pending: AtomicU64,
    /// Precisou do `takeown` e do `icacls`.
    pub plan_b: AtomicBool,
    /// Passo atual num item sem permissão (`STEP_*`, 0 = nenhum) e o nome dele.
    pub step: AtomicU8,
    pub current: Mutex<String>,
    pub cancel: AtomicBool,
    /// (caminho, motivo).
    pub errors: Mutex<Vec<(String, String)>>,
    /// Itens escolhidos com algo que ficou por permissão ou em uso (só sem administrador).
    pub denied: Mutex<Vec<PathBuf>>,
}

impl State {
    fn error(&self, p: &Path, why: &str) {
        self.errors.lock().unwrap().push((p.display().to_string(), why.to_string()));
    }
}

// --- recusa ---

/// Pastas que o Shift+Del do Prisma nunca apaga, já em minúsculas e sem `\` no fim.
pub struct Protected {
    /// Não apaga a pasta nem nada dentro dela.
    pub inside: Vec<String>,
    /// Não apaga a pasta nem uma pasta acima dela.
    pub above: Vec<String>,
    /// A pasta dos perfis (C:\Users): nem ela nem um perfil inteiro.
    pub profiles: String,
}

fn norm(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = if let Some(r) = s.strip_prefix(r"\\?\UNC\") { format!(r"\\{r}") } else { s.strip_prefix(r"\\?\").map_or(s.clone(), str::to_string) };
    s.trim_end_matches('\\').to_lowercase()
}

fn within(a: &str, b: &str) -> bool {
    !b.is_empty() && (a == b || a.strip_prefix(b).is_some_and(|r| r.starts_with('\\')))
}

/// As pastas do sistema pelo próprio Windows (`SHGetKnownFolderPath`), nunca pelo nome em português ou inglês.
pub fn protected() -> Protected {
    use windows::Win32::UI::Shell::*;
    let kf = |id| norm(&super::known_folder(id));
    let exe = std::env::current_exe().unwrap_or_default();
    let mut above = vec![kf(&FOLDERID_Profile), kf(&FOLDERID_ProgramFiles), kf(&FOLDERID_ProgramFilesX86), kf(&FOLDERID_ProgramData)];
    above.extend(exe.parent().map(norm));
    above.push(norm(&exe));
    let mut inside = vec![kf(&FOLDERID_Windows), norm(&crate::config::data_dir())];
    inside.retain(|s| !s.is_empty());
    above.retain(|s| !s.is_empty());
    Protected { inside, above, profiles: kf(&FOLDERID_UserProfiles) }
}

/// Por que o Prisma não apaga este caminho, ou None.
pub fn refusal(p: &Path, pr: &Protected) -> Option<&'static str> {
    let s = norm(p);
    let path = Path::new(&s);
    if s.is_empty() || path.parent().is_none() || path.file_name().is_none() {
        return Some("Raiz de disco");
    }
    if pr.inside.iter().any(|w| within(&s, w)) {
        return Some("Pasta do Windows ou do Prisma");
    }
    let parent = path.parent().map(|q| q.to_string_lossy().into_owned()).unwrap_or_default();
    if within(&pr.profiles, &s) || (!pr.profiles.is_empty() && parent == pr.profiles) || pr.above.iter().any(|w| within(w, &s)) {
        return Some("Pasta do sistema");
    }
    None
}

// --- chamadas do NT ---

#[repr(C)]
struct UnicodeString {
    len: u16,
    max: u16,
    buf: *const u16,
}

#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root: HANDLE,
    name: *const UnicodeString,
    attributes: u32,
    sd: *const c_void,
    qos: *const c_void,
}

#[repr(C)]
struct IoStatus {
    status: usize,
    info: usize,
}

#[link(name = "ntdll")]
extern "system" {
    fn NtCreateFile(h: *mut HANDLE, access: u32, oa: *const ObjectAttributes, io: *mut IoStatus, alloc: *const i64, attrs: u32, share: u32, disposition: u32, options: u32, ea: *const c_void, ea_len: u32) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}

const DELETE: u32 = 0x0001_0000;
const LIST: u32 = 0x0001;
const READ_ATTRS: u32 = 0x0080;
const SYNC: u32 = 0x0010_0000;
const WRITE_DAC: u32 = 0x0004_0000;
const WRITE_OWNER: u32 = 0x0008_0000;
const ERR_DENIED: u32 = 5;
const ERR_IN_USE: u32 = 32;
const ERR_NOT_EMPTY: u32 = 145;

/// Abre `name` dentro da pasta `dir`, sem seguir link. Com os privilégios de backup ligados, a
/// intenção de backup passa por cima das permissões.
fn open_in(dir: HANDLE, name: &[u16], access: u32) -> Result<OwnedHandle, u32> {
    let us = UnicodeString { len: (name.len() * 2) as u16, max: (name.len() * 2) as u16, buf: name.as_ptr() };
    // OBJ_CASE_INSENSITIVE
    let oa = ObjectAttributes { length: std::mem::size_of::<ObjectAttributes>() as u32, root: dir, name: &us, attributes: 0x40, sd: std::ptr::null(), qos: std::ptr::null() };
    let mut io = IoStatus { status: 0, info: 0 };
    let mut h = HANDLE::default();
    // FILE_OPEN; FILE_OPEN_REPARSE_POINT | FILE_OPEN_FOR_BACKUP_INTENT | FILE_SYNCHRONOUS_IO_NONALERT
    let st = unsafe { NtCreateFile(&mut h, access | SYNC, &oa, &mut io, std::ptr::null(), 0, 7, 1, 0x0020_0000 | 0x4000 | 0x20, std::ptr::null(), 0) };
    if st < 0 {
        return Err(unsafe { RtlNtStatusToDosError(st) });
    }
    Ok(unsafe { OwnedHandle::from_raw_handle(h.0) })
}

/// O caminho como o Windows aceita sem limite de tamanho.
fn long(p: &Path) -> Vec<u16> {
    let s = p.to_string_lossy();
    let s = if s.starts_with(r"\\?\") {
        s.into_owned()
    } else if let Some(unc) = s.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{unc}")
    } else {
        format!(r"\\?\{s}")
    };
    super::wide(&s)
}

/// Abre um item escolhido pelo caminho, também sem seguir link.
fn open_path(p: &Path, access: u32) -> Result<OwnedHandle, u32> {
    let w = long(p);
    let flags = FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT;
    match unsafe { CreateFileW(windows::core::PCWSTR(w.as_ptr()), access | SYNC, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, None, OPEN_EXISTING, flags, None) } {
        Ok(h) => Ok(unsafe { OwnedHandle::from_raw_handle(h.0) }),
        Err(e) => Err(code(&e)),
    }
}

fn code(e: &windows::core::Error) -> u32 {
    (e.code().0 as u32) & 0xFFFF
}

fn raw(h: &OwnedHandle) -> HANDLE {
    HANDLE(h.as_raw_handle())
}

/// (atributos, é link) pelo handle já aberto.
fn attrs(h: &OwnedHandle) -> Option<(u32, bool)> {
    let mut t = FILE_ATTRIBUTE_TAG_INFO::default();
    unsafe { GetFileInformationByHandleEx(raw(h), FileAttributeTagInfo, &mut t as *mut _ as *mut c_void, std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32) }.ok()?;
    Some((t.FileAttributes, t.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0))
}

/// Onde o handle está de verdade, com as junções do caminho resolvidas.
fn final_path(h: &OwnedHandle) -> Option<PathBuf> {
    let mut buf = vec![0u16; 32768];
    let n = unsafe { GetFinalPathNameByHandleW(raw(h), &mut buf, GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0)) } as usize;
    (n > 0 && n < buf.len()).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

fn is_dir(a: u32) -> bool {
    a & FILE_ATTRIBUTE_DIRECTORY.0 != 0 && a & FILE_ATTRIBUTE_REPARSE_POINT.0 == 0
}

/// Lista a pasta pelo handle: (nome, atributos, tamanho), sem "." e "..".
fn list(h: &OwnedHandle, mut f: impl FnMut(&[u16], u32, u64)) -> Result<(), u32> {
    let mut buf = vec![0u64; 8192];
    let mut class = FileFullDirectoryRestartInfo;
    loop {
        if let Err(e) = unsafe { GetFileInformationByHandleEx(raw(h), class, buf.as_mut_ptr() as *mut c_void, (buf.len() * 8) as u32) } {
            // ERROR_NO_MORE_FILES
            return if code(&e) == 18 { Ok(()) } else { Err(code(&e)) };
        }
        class = FileFullDirectoryInfo;
        let mut off = 0usize;
        loop {
            let e = unsafe { &*((buf.as_ptr() as *const u8).add(off) as *const FILE_FULL_DIR_INFO) };
            let name = unsafe { std::slice::from_raw_parts(e.FileName.as_ptr(), e.FileNameLength as usize / 2) };
            if name != [46] && name != [46, 46] {
                f(name, e.FileAttributes, e.EndOfFile.max(0) as u64);
            }
            if e.NextEntryOffset == 0 {
                break;
            }
            off += e.NextEntryOffset as usize;
        }
    }
}

/// Marca para apagar: some na hora do disco (POSIX), mesmo somente leitura. FAT e exFAT não têm o
/// jeito POSIX: lá o nome some quando o handle fecha.
fn unlink(h: &OwnedHandle) -> Result<(), u32> {
    let ex = FILE_DISPOSITION_INFO_EX { Flags: FILE_DISPOSITION_INFO_EX_FLAGS(FILE_DISPOSITION_FLAG_DELETE.0 | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS.0 | FILE_DISPOSITION_FLAG_IGNORE_READONLY_ATTRIBUTE.0) };
    let Err(e) = (unsafe { SetFileInformationByHandle(raw(h), FileDispositionInfoEx, &ex as *const _ as *const c_void, 4) }) else { return Ok(()) };
    if !matches!(code(&e), 1 | 50 | 87) {
        return Err(code(&e));
    }
    // shortcut: no FAT, arquivo somente leitura dá erro; tirar o atributo pediria outro acesso.
    let d = FILE_DISPOSITION_INFO { DeleteFile: true };
    unsafe { SetFileInformationByHandle(raw(h), FileDispositionInfo, &d as *const _ as *const c_void, std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32) }.map_err(|e| code(&e))
}

// --- permissões (só administrador) ---

/// Backup, restauração e posse: abrir e apagar passa por cima das permissões de cada pasta.
pub fn enable_privileges() {
    use windows::core::w;
    use windows::Win32::Foundation::{CloseHandle, LUID};
    use windows::Win32::Security::*;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut tok = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut tok).is_err() {
            return;
        }
        for name in [w!("SeBackupPrivilege"), w!("SeRestorePrivilege"), w!("SeTakeOwnershipPrivilege")] {
            let mut luid = LUID::default();
            if LookupPrivilegeValueW(None, name, &mut luid).is_ok() {
                let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
                let _ = AdjustTokenPrivileges(tok, false, Some(&tp), 0, None, None);
            }
        }
        let _ = CloseHandle(tok);
    }
}

/// O grupo Administradores pelo SID (S-1-5-32-544): o nome muda com o idioma do Windows.
fn admins_sid() -> [u8; 68] {
    use windows::Win32::Security::*;
    let mut sid = [0u8; 68];
    let mut n = sid.len() as u32;
    let _ = unsafe { CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(PSID(sid.as_mut_ptr() as *mut c_void)), &mut n) };
    sid
}

/// Toma posse para os Administradores e dá controle total a eles, só neste item, pelo handle.
fn take_over(dir: HANDLE, name: &[u16], st: &State) -> bool {
    use windows::Win32::Security::Authorization::{SetSecurityInfo, SE_FILE_OBJECT};
    use windows::Win32::Security::*;
    let mut sid = admins_sid();
    let psid = PSID(sid.as_mut_ptr() as *mut c_void);
    st.step.store(STEP_OWNER, Relaxed);
    let Ok(h) = open_in(dir, name, WRITE_OWNER) else { return false };
    if unsafe { SetSecurityInfo(raw(&h), SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION, Some(psid), None, None, None) }.is_err() {
        return false;
    }
    drop(h);
    st.step.store(STEP_GRANT, Relaxed);
    let Ok(h) = open_in(dir, name, WRITE_DAC) else { return false };
    let mut acl = [0u32; 32];
    let pacl = acl.as_mut_ptr() as *mut ACL;
    unsafe {
        if InitializeAcl(pacl, 128, ACL_REVISION).is_err() || AddAccessAllowedAceEx(pacl, ACL_REVISION, CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE, FILE_ALL_ACCESS.0, psid).is_err() {
            return false;
        }
        SetSecurityInfo(raw(&h), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, None, None, Some(pacl), None).is_ok()
    }
}

/// Plano B, o último recurso: os comandos do Windows, sem janela. Grupo pelo SID e sem o `/D`
/// do takeown, cuja letra muda com o idioma; o resultado se confere tentando de novo, nunca pelo texto.
pub fn plan_b_cmds(p: &Path) -> [(&'static str, Vec<String>); 2] {
    let p = p.display().to_string();
    [("takeown", vec!["/F".into(), p.clone(), "/A".into()]), ("icacls", vec![p, "/grant".into(), "*S-1-5-32-544:F".into(), "/C".into(), "/Q".into()])]
}

fn plan_b(p: &Path, st: &State) {
    use std::os::windows::process::CommandExt;
    st.step.store(STEP_PLAN_B, Relaxed);
    st.plan_b.store(true, Relaxed);
    for (tool, args) in plan_b_cmds(p) {
        let _ = std::process::Command::new(super::system_tool(tool))
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .status();
    }
}

// --- o trabalho ---

struct Ctx {
    st: Arc<State>,
    delete: bool,
    admin: bool,
    items: Vec<PathBuf>,
}

impl Ctx {
    /// Sem administrador, o que ficou por permissão ou em uso vai para a cópia de administrador.
    fn denied(&self, top: usize) {
        let mut d = self.st.denied.lock().unwrap();
        if !d.contains(&self.items[top]) {
            d.push(self.items[top].clone());
        }
    }

    /// O que não saiu: como administrador, fica para o próximo reinício (depois do que está dentro).
    fn leftover(&self, p: &Path, err: u32, top: usize, count: bool) {
        if self.admin {
            let w = long(p);
            if unsafe { MoveFileExW(windows::core::PCWSTR(w.as_ptr()), None, MOVEFILE_DELAY_UNTIL_REBOOT) }.is_ok() {
                if count {
                    self.st.pending.fetch_add(1, Relaxed);
                }
                return;
            }
        } else if matches!(err, ERR_DENIED | ERR_IN_USE) {
            self.denied(top);
        }
        if count {
            self.st.error(p, &super::copy::reason(&std::io::Error::from_raw_os_error(err as i32)));
        }
    }

    /// Abre um item dentro da pasta. Negado, como administrador: toma posse e dá permissão, e se ainda
    /// negar, o plano B; depois tenta de novo.
    fn open(&self, dir: &OwnedHandle, name: &[u16], path: &Path, access: u32) -> Result<OwnedHandle, u32> {
        let first = open_in(raw(dir), name, access);
        // Contando, nada muda no disco: as permissões só mudam depois da confirmação.
        if !self.admin || !self.delete || first.as_ref().err() != Some(&ERR_DENIED) {
            return first;
        }
        *self.st.current.lock().unwrap() = path.display().to_string();
        let mut r = if take_over(raw(dir), name, &self.st) { open_in(raw(dir), name, access) } else { first };
        // O plano B vai pelo caminho em texto: só se a pasta mãe ainda está nele (ninguém trocou uma
        // pasta do caminho por junção) e o item não é link, para o takeown não sair do lugar.
        // shortcut: sobra a janela entre esta conferência e o comando; o plano B é o último recurso.
        let same = final_path(dir).is_some_and(|d| path.parent().is_some_and(|q| norm(&d) == norm(q)));
        if r.as_ref().err() == Some(&ERR_DENIED) && same && !super::is_link(path) {
            plan_b(path, &self.st);
            r = open_in(raw(dir), name, access);
        }
        self.st.step.store(0, Relaxed);
        r
    }
}

/// Uma pasta aberta. Quando a última referência some (tudo dentro já foi), ela mesma é apagada.
struct Node {
    h: OwnedHandle,
    /// Aberta sem o acesso de apagar (em uso como pasta atual de um programa).
    keep: bool,
    path: PathBuf,
    top: usize,
    ctx: Arc<Ctx>,
    _parent: Option<Arc<Node>>,
}

impl Drop for Node {
    fn drop(&mut self) {
        if !self.ctx.delete || self.ctx.st.cancel.load(Relaxed) {
            return;
        }
        let r = if self.keep { Err(ERR_IN_USE) } else { unlink(&self.h) };
        if let Err(e) = r {
            // Pasta que não esvaziou: o motivo já está no que ficou dentro dela.
            self.ctx.leftover(&self.path, e, self.top, e != ERR_NOT_EMPTY);
        }
    }
}

enum Job {
    Root(usize),
    Dir(Arc<Node>, Vec<u16>),
    Files(Arc<Node>, Vec<(Vec<u16>, u64)>),
}

#[derive(Default)]
struct Queue {
    jobs: Mutex<(Vec<Job>, usize)>,
    cv: Condvar,
}

impl Queue {
    fn push(&self, j: Job) {
        self.jobs.lock().unwrap().0.push(j);
        self.cv.notify_one();
    }
}

const BATCH: usize = 64;

fn child(dir: &Node, name: &[u16]) -> PathBuf {
    dir.path.join(String::from_utf16_lossy(name))
}

/// Apaga um item que não é pasta (arquivo, link ou junção) dentro de `dir`.
fn delete_file(ctx: &Ctx, dir: &Node, name: &[u16], size: u64) {
    let path = child(dir, name);
    let r = ctx.open(&dir.h, name, &path, DELETE | READ_ATTRS).and_then(|h| unlink(&h));
    match r {
        Ok(()) => {
            ctx.st.done.fetch_add(1, Relaxed);
            ctx.st.freed.fetch_add(size, Relaxed);
        }
        Err(e) => ctx.leftover(&path, e, dir.top, true),
    }
}

/// Abre a pasta (já aberta em `h`) como nó e põe o que há dentro na fila.
fn enter(ctx: &Arc<Ctx>, q: &Queue, h: OwnedHandle, keep: bool, path: PathBuf, top: usize, parent: Option<Arc<Node>>) {
    let node = Arc::new(Node { h, keep, path, top, ctx: ctx.clone(), _parent: parent });
    let mut batch = Vec::new();
    let r = list(&node.h, |name, a, size| {
        if is_dir(a) {
            q.push(Job::Dir(node.clone(), name.to_vec()));
        } else if ctx.delete {
            batch.push((name.to_vec(), size));
            if batch.len() == BATCH {
                q.push(Job::Files(node.clone(), std::mem::take(&mut batch)));
            }
        } else {
            ctx.st.files.fetch_add(1, Relaxed);
            ctx.st.bytes.fetch_add(size, Relaxed);
        }
    });
    if !batch.is_empty() {
        q.push(Job::Files(node.clone(), batch));
    }
    if let Err(e) = r {
        if ctx.delete {
            ctx.leftover(&node.path, e, top, true);
        }
    }
}

/// Abre o que veio na fila: pasta vira nó, o resto é apagado como arquivo.
fn run(ctx: &Arc<Ctx>, q: &Queue, job: Job) {
    if ctx.st.cancel.load(Relaxed) {
        return;
    }
    let access = DELETE | LIST | READ_ATTRS;
    match job {
        Job::Files(dir, names) => {
            for (name, size) in names {
                if ctx.st.cancel.load(Relaxed) {
                    return;
                }
                delete_file(ctx, &dir, &name, size);
            }
        }
        Job::Dir(parent, name) => {
            let path = child(&parent, &name);
            let mut keep = false;
            let h = match ctx.open(&parent.h, &name, &path, access) {
                Err(ERR_IN_USE) => {
                    keep = true;
                    ctx.open(&parent.h, &name, &path, LIST | READ_ATTRS)
                }
                r => r,
            };
            match h {
                // Virou link ou arquivo depois de listada: apaga como arquivo, sem entrar.
                Ok(h) if !attrs(&h).is_some_and(|(a, _)| is_dir(a)) => {
                    drop(h);
                    if ctx.delete {
                        delete_file(ctx, &parent, &name, 0);
                    } else {
                        ctx.st.files.fetch_add(1, Relaxed);
                    }
                }
                Ok(h) => enter(ctx, q, h, keep, path, parent.top, Some(parent.clone())),
                Err(e) if ctx.delete => ctx.leftover(&path, e, parent.top, true),
                Err(_) => {}
            }
        }
        Job::Root(i) => root(ctx, q, i),
    }
}

/// Um item escolhido: abre pelo caminho, confere o caminho de verdade (com o handle já aberto) e segue.
fn root(ctx: &Arc<Ctx>, q: &Queue, i: usize) {
    let p = ctx.items[i].clone();
    let mut keep = false;
    let h = match open_path(&p, DELETE | LIST | READ_ATTRS) {
        Err(ERR_IN_USE) => {
            keep = true;
            open_path(&p, LIST | READ_ATTRS)
        }
        r => r,
    };
    let h = match h {
        Ok(h) => h,
        // Já não existe (a cópia de administrador recebe também o que a outra já apagou).
        Err(2 | 3) => return,
        Err(e) => {
            if ctx.delete {
                ctx.leftover(&p, e, i, true);
            }
            return;
        }
    };
    // O caminho final, pelo handle: unidade de `subst`, pasta mapeada e junção no caminho já resolvidas.
    let real = final_path(&h).unwrap_or_else(|| p.clone());
    let pr = protected();
    if let Some(why) = refusal(&real, &pr).or_else(|| refusal(&p, &pr)) {
        if ctx.delete {
            ctx.st.error(&p, why);
        }
        return;
    }
    match attrs(&h) {
        Some((a, _)) if is_dir(a) => enter(ctx, q, h, keep, p, i, None),
        _ if !ctx.delete => {
            ctx.st.files.fetch_add(1, Relaxed);
            ctx.st.bytes.fetch_add(std::fs::symlink_metadata(&p).map_or(0, |m| m.len()), Relaxed);
        }
        _ => {
            let size = std::fs::symlink_metadata(&p).map_or(0, |m| m.len());
            match if keep { Err(ERR_IN_USE) } else { unlink(&h) } {
                Ok(()) => {
                    ctx.st.done.fetch_add(1, Relaxed);
                    ctx.st.freed.fetch_add(size, Relaxed);
                }
                Err(e) => {
                    drop(h);
                    ctx.leftover(&p, e, i, true);
                }
            }
        }
    }
}

/// Vários threads pegando da mesma fila; acaba quando a fila esvazia e ninguém mais trabalha.
fn walk(ctx: Arc<Ctx>) {
    let q = Queue::default();
    for i in 0..ctx.items.len() {
        q.push(Job::Root(i));
    }
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let job = {
                    let mut g = q.jobs.lock().unwrap();
                    loop {
                        if let Some(j) = g.0.pop() {
                            g.1 += 1;
                            break j;
                        }
                        if g.1 == 0 {
                            q.cv.notify_all();
                            return;
                        }
                        g = q.cv.wait(g).unwrap();
                    }
                };
                // O nó solto aqui pode apagar a pasta dele (e as de cima): antes de dizer que terminou.
                run(&ctx, &q, job);
                let mut g = q.jobs.lock().unwrap();
                g.1 -= 1;
                if g.1 == 0 && g.0.is_empty() {
                    q.cv.notify_all();
                }
            });
        }
    });
}

/// Conta arquivos e bytes, sem apagar nada (a janela mostra enquanto pergunta).
pub fn count(items: &[PathBuf], st: &Arc<State>, admin: bool) {
    walk(Arc::new(Ctx { st: st.clone(), delete: false, admin, items: items.to_vec() }));
    st.counted.store(true, Relaxed);
}

/// Apaga tudo. Como administrador (`admin`, com `enable_privileges` antes), toma posse do que negar
/// e marca o que está em uso para o próximo reinício.
pub fn erase(items: &[PathBuf], st: &Arc<State>, admin: bool) {
    st.phase.store(DELETING, Relaxed);
    walk(Arc::new(Ctx { st: st.clone(), delete: true, admin, items: items.to_vec() }));
    let phase = if st.cancel.load(Relaxed) {
        CANCELED
    } else if !admin && !st.denied.lock().unwrap().is_empty() {
        NEED_ADMIN
    } else {
        DONE
    };
    st.phase.store(phase, Relaxed);
}

// --- pedido para a cópia de administrador ---

/// `--erase-admin=<apagados>,<bytes>|<caminho>|<caminho>...`: a linha de comando da cópia elevada
/// não muda depois do pedido (um arquivo no %TEMP% poderia ser trocado por outro programa).
pub fn admin_arg(done: u64, freed: u64, items: &[PathBuf]) -> String {
    let mut s = format!("--erase-admin=\"{done},{freed}");
    for p in items {
        s.push('|');
        s.push_str(&p.display().to_string());
    }
    s.push('"');
    s
}

/// Lê o pedido; caminho que não é absoluto ou que tem caractere proibido recusa tudo.
pub fn parse_admin(v: &str) -> Option<(u64, u64, Vec<PathBuf>)> {
    let mut parts = v.trim_matches('"').split('|');
    let (done, freed) = parts.next()?.split_once(',')?;
    let items: Vec<PathBuf> = parts.map(PathBuf::from).collect();
    let ok = |p: &PathBuf| {
        let s = p.to_string_lossy();
        p.is_absolute() && !s.chars().any(|c| matches!(c, '"' | '<' | '>' | '*' | '?') || (c as u32) < 32)
    };
    if items.is_empty() || !items.iter().all(ok) {
        return None;
    }
    Some((done.parse().ok()?, freed.parse().ok()?, items))
}

// --- histórico ---

const KEEP: usize = 500;

/// Uma exclusão, só para a página Cópias mostrar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Erased {
    pub at: i64,
    pub files: u64,
    pub bytes: u64,
    pub pending: u64,
    pub errors: u32,
    pub plan_b: bool,
    pub items: Vec<String>,
}

impl Erased {
    pub fn line(&self) -> String {
        let items: Vec<String> = self.items.iter().map(|s| s.replace(['\t', '\n', '\r', '|'], " ")).collect();
        format!("{}\t{}\t{}\t{}\t{}\t{}\t{}", self.at, self.files, self.bytes, self.pending, self.errors, self.plan_b as u8, items.join("|"))
    }

    pub fn parse(line: &str) -> Option<Erased> {
        let [at, files, bytes, pending, errors, plan_b, items] = line.split('\t').collect::<Vec<_>>()[..] else { return None };
        Some(Erased {
            at: at.parse().ok()?,
            files: files.parse().ok()?,
            bytes: bytes.parse().ok()?,
            pending: pending.parse().ok()?,
            errors: errors.parse().ok()?,
            plan_b: plan_b == "1",
            items: items.split('|').filter(|s| !s.is_empty()).map(String::from).collect(),
        })
    }
}

pub fn history_path() -> PathBuf {
    crate::config::data_dir().join("exclusoes_historico.txt")
}

/// Abre o histórico só para esta cópia: duas exclusões que terminam juntas esperam a vez, e
/// nenhuma linha se perde (nem no corte das mais velhas).
fn locked(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    for _ in 0..250 {
        match std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).share_mode(0).open(path) {
            Err(e) if e.raw_os_error() == Some(ERR_IN_USE as i32) => std::thread::sleep(std::time::Duration::from_millis(20)),
            r => return r,
        }
    }
    Err(std::io::Error::from_raw_os_error(ERR_IN_USE as i32))
}

pub fn append(path: &Path, r: &Erased) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = locked(path)?;
    let mut text = String::new();
    f.read_to_string(&mut text)?;
    text.push_str(&r.line());
    text.push('\n');
    let n = text.lines().count();
    if n > KEEP + 100 {
        text = text.lines().skip(n - KEEP).map(|l| format!("{l}\n")).collect();
        f.set_len(0)?;
        f.seek(SeekFrom::Start(0))?;
        f.write_all(text.as_bytes())
    } else {
        f.write_all(format!("{}\n", r.line()).as_bytes())
    }
}

/// Da mais nova para a mais velha; None enquanto outra cópia grava (a página tenta de novo).
pub fn load(path: &Path) -> Option<Vec<Erased>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(_) => return None,
    };
    let mut v: Vec<Erased> = text.lines().filter_map(Erased::parse).collect();
    v.reverse();
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pasta de teste no %TEMP%, nunca no disco F:.
    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("prisma_apagar_{name}_{}", std::process::id()));
        assert!(!d.to_string_lossy().to_uppercase().starts_with("F:"), "teste no disco F:");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn run(items: &[PathBuf]) -> Arc<State> {
        let st = Arc::new(State::default());
        count(items, &st, false);
        erase(items, &st, false);
        st
    }

    #[test]
    fn apaga_arvore_grande() {
        let d = scratch("arvore");
        let top = d.join("muitos");
        for i in 0..40 {
            let sub = top.join(format!("p{i}")).join("dentro");
            std::fs::create_dir_all(&sub).unwrap();
            for j in 0..30 {
                std::fs::write(sub.join(format!("{j}.txt")), b"x").unwrap();
            }
        }
        let ro = top.join("somente_leitura.txt");
        std::fs::write(&ro, b"abc").unwrap();
        let mut perm = std::fs::metadata(&ro).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&ro, perm).unwrap();
        let st = Arc::new(State::default());
        count(std::slice::from_ref(&top), &st, false);
        assert_eq!(st.files.load(Relaxed), 40 * 30 + 1);
        assert_eq!(st.bytes.load(Relaxed), 40 * 30 + 3);
        erase(std::slice::from_ref(&top), &st, false);
        assert!(st.errors.lock().unwrap().is_empty(), "{:?}", st.errors.lock().unwrap());
        assert_eq!(st.done.load(Relaxed), 40 * 30 + 1);
        assert_eq!(st.phase.load(Relaxed), DONE);
        assert!(!top.exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn juncao_apaga_o_link_e_nao_o_alvo() {
        let d = scratch("juncao");
        let alvo = d.join("alvo");
        std::fs::create_dir_all(alvo.join("sub")).unwrap();
        std::fs::write(alvo.join("precisa_sobrar.txt"), b"fica").unwrap();
        std::fs::write(alvo.join("sub\\tambem.txt"), b"fica").unwrap();
        let top = d.join("apagar");
        std::fs::create_dir_all(&top).unwrap();
        std::fs::write(top.join("a.txt"), b"vai").unwrap();
        let link = top.join("link");
        let ok = std::process::Command::new(crate::sys::system_tool("cmd")).args(["/c", "mklink", "/J"]).arg(&link).arg(&alvo).stdout(std::process::Stdio::null()).status().unwrap().success();
        assert!(ok, "mklink /J falhou");
        // A junção solta, escolhida ela mesma, e a pasta com uma junção dentro.
        let solta = d.join("solta");
        assert!(std::process::Command::new(crate::sys::system_tool("cmd")).args(["/c", "mklink", "/J"]).arg(&solta).arg(&alvo).stdout(std::process::Stdio::null()).status().unwrap().success());
        let st = run(&[top.clone(), solta.clone()]);
        assert!(st.errors.lock().unwrap().is_empty(), "{:?}", st.errors.lock().unwrap());
        assert!(!top.exists() && std::fs::symlink_metadata(&solta).is_err());
        assert_eq!(std::fs::read(alvo.join("precisa_sobrar.txt")).unwrap(), b"fica");
        assert!(alvo.join("sub\\tambem.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn arquivo_em_uso_fica_e_o_resto_sai() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = scratch("em_uso");
        let top = d.join("pasta");
        std::fs::create_dir_all(top.join("sub")).unwrap();
        std::fs::write(top.join("sub\\livre.txt"), b"x").unwrap();
        let busy = top.join("travado.txt");
        std::fs::write(&busy, b"x").unwrap();
        // Aberto sem deixar apagar, como um programa com o arquivo aberto.
        let lock = std::fs::OpenOptions::new().read(true).share_mode(1).open(&busy).unwrap();
        let st = run(std::slice::from_ref(&top));
        assert!(!top.join("sub").exists());
        assert!(busy.exists());
        let errs = st.errors.lock().unwrap().clone();
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert_eq!(errs[0].1, "Em uso por outro programa");
        // Sem administrador, o que ficou vai para a cópia de administrador.
        assert_eq!(st.phase.load(Relaxed), NEED_ADMIN);
        assert_eq!(*st.denied.lock().unwrap(), vec![top.clone()]);
        drop(lock);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn arquivo_solto_e_cancelar() {
        let d = scratch("solto");
        let f = d.join("um.bin");
        std::fs::write(&f, vec![1u8; 5000]).unwrap();
        let st = run(std::slice::from_ref(&f));
        assert!(!f.exists());
        assert_eq!((st.done.load(Relaxed), st.freed.load(Relaxed)), (1, 5000));
        // Cancelado antes de começar: nada sai.
        let g = d.join("fica.txt");
        std::fs::write(&g, b"x").unwrap();
        let st = Arc::new(State::default());
        st.cancel.store(true, Relaxed);
        erase(std::slice::from_ref(&g), &st, false);
        assert!(g.exists());
        assert_eq!(st.phase.load(Relaxed), CANCELED);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn recusa_pastas_do_sistema() {
        let pr = Protected {
            inside: vec![r"c:\windows".into(), r"d:\prisma\prismadata".into()],
            above: vec![r"c:\users\ana".into(), r"c:\program files".into(), r"c:\program files (x86)".into(), r"c:\programdata".into(), r"d:\prisma".into(), r"d:\prisma\prisma.exe".into()],
            profiles: r"c:\users".into(),
        };
        let no = |s: &str| refusal(Path::new(s), &pr);
        for bad in [
            r"C:\",
            r"C:",
            r"D:\",
            r"\\servidor\pasta",
            r"\\?\C:\",
            r"C:\Windows",
            r"c:\WINDOWS\System32",
            r"\\?\C:\Windows\WinSxS\x",
            r"C:\Users",
            r"C:\Users\Ana",
            r"C:\Users\Public",
            r"C:\Users\Outra\",
            r"C:\Program Files",
            r"C:\Program Files (x86)\",
            r"C:\ProgramData",
            r"D:\Prisma",
            r"D:\Prisma\prisma.exe",
            r"D:\Prisma\PrismaData\config.txt",
        ] {
            assert!(no(bad).is_some(), "devia recusar {bad}");
        }
        for ok in [r"C:\Windows.old", r"C:\Program Files\WindowsApps\App_1.0", r"C:\ProgramData\Lixo", r"C:\Users\Ana\Downloads\x", r"D:\Prisma\outro", r"D:\Prismas", r"\\servidor\pasta\x", r"E:\Jogos"] {
            assert_eq!(no(ok), None, "não devia recusar {ok}");
        }
        // As pastas de verdade deste PC também.
        let real = protected();
        assert!(refusal(&crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_Windows), &real).is_some());
        assert!(refusal(&crate::sys::known_folder(&windows::Win32::UI::Shell::FOLDERID_Profile), &real).is_some());
        assert_eq!(refusal(&std::env::temp_dir().join("x"), &real), None);
    }

    #[test]
    fn plano_b_nao_depende_do_idioma() {
        for (tool, args) in plan_b_cmds(Path::new(r"C:\Windows.old\x")) {
            assert!(matches!(tool, "takeown" | "icacls"));
            let line = args.join(" ").to_lowercase();
            assert!(!line.contains("/d"), "{line}");
            assert!(!line.contains("administrators") && !line.contains("administradores"), "{line}");
            assert!(!line.contains("/r") && !line.contains("/t"), "não passa por junções: {line}");
        }
        assert!(plan_b_cmds(Path::new("x"))[1].1.contains(&"*S-1-5-32-544:F".to_string()));
    }

    #[test]
    fn pedido_de_administrador() {
        let items = vec![PathBuf::from(r"C:\Windows.old"), PathBuf::from(r"D:\Jogos\Velho jogo")];
        let arg = admin_arg(12, 3400, &items);
        let v = arg.strip_prefix("--erase-admin=").unwrap();
        assert_eq!(parse_admin(v), Some((12, 3400, items)));
        for bad in ["", "1,2", "1,2|", "x,2|C:\\a", "1,2|relativo", "1,2|C:\\a\"b", "1,2|C:\\a|C:\\b\n", "1|C:\\a"] {
            assert!(parse_admin(bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn historico_nao_perde_linha() {
        let d = scratch("historico");
        let path = d.join("exclusoes.txt");
        let rec = |i: i64| Erased { at: i, files: 3, bytes: 99, pending: (i % 2) as u64, errors: 0, plan_b: i == 5, items: vec![format!("C:\\pasta {i}"), "D:\\b|c".into()] };
        std::thread::scope(|s| {
            for t in 0..4 {
                let path = &path;
                s.spawn(move || {
                    for i in 0..40 {
                        append(path, &rec(t * 1000 + i)).unwrap();
                    }
                });
            }
        });
        assert_eq!(load(&path).unwrap().len(), 160);
        // Passou de 600 linhas: ficam as 500 mais novas, nenhuma truncada.
        let old: String = (0..700).map(|i| format!("{}\n", rec(i).line())).collect();
        std::fs::write(&path, old).unwrap();
        append(&path, &rec(9999)).unwrap();
        let all = load(&path).unwrap();
        assert_eq!(all.len(), KEEP);
        assert_eq!(all[0].at, 9999);
        assert_eq!(all[KEEP - 1].at, 201);
        assert_eq!(Erased::parse(&rec(5).line()), Some(Erased { items: vec!["C:\\pasta 5".into(), "D:\\b c".into()], ..rec(5) }));
        let _ = std::fs::remove_dir_all(&d);
    }
}
