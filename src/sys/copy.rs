//! Cópia do "Colar com o Prisma": um trabalho com todos os itens colados, pausa no meio do
//! arquivo, conferência por SHA-256, o mesmo destino para todos os arquivos que já existem e a
//! lista de erros sem parar o resto. Nada roda dentro do Explorer: ele só abre o prisma.exe.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::Duration;

/// O que fazer com um arquivo que já existe no destino. `ASK` espera a escolha na janela.
pub const ASK: u8 = 0;
pub const SKIP: u8 = 1;
pub const REPLACE: u8 = 2;
pub const KEEP_BOTH: u8 = 3;

/// Fases do trabalho.
pub const PREPARING: u8 = 0;
pub const COPYING: u8 = 1;
pub const DONE: u8 = 2;
pub const CANCELED: u8 = 3;
pub const NO_SPACE: u8 = 4;

#[derive(Default)]
pub struct State {
    pub total: AtomicU64,
    pub done: AtomicU64,
    pub files: AtomicU32,
    pub files_done: AtomicU32,
    pub paused: AtomicBool,
    pub cancel: AtomicBool,
    pub verify: AtomicBool,
    pub policy: AtomicU8,
    pub phase: AtomicU8,
    /// Há um arquivo esperando a escolha de `policy`.
    pub asking: AtomicBool,
    /// Bytes que faltam e bytes livres, quando a fase é `NO_SPACE`.
    pub need: AtomicU64,
    pub free: AtomicU64,
    pub current: Mutex<String>,
    /// (arquivo, motivo). O motivo é a chave em português, traduzida ao desenhar.
    pub errors: Mutex<Vec<(String, String)>>,
}

impl State {
    fn error(&self, path: &Path, why: impl Into<String>) {
        self.errors.lock().unwrap().push((path.display().to_string(), why.into()));
    }
}

struct Entry {
    src: PathBuf,
    dst: PathBuf,
    dir: bool,
    size: u64,
}

/// Motivo curto para os erros mais comuns; o resto vem do Windows, no idioma dele.
pub fn reason(e: &std::io::Error) -> String {
    match e.raw_os_error() {
        Some(32 | 33) => "Em uso por outro programa".into(),
        Some(5) => "Acesso negado".into(),
        Some(39 | 112) => "Sem espaço no disco".into(),
        Some(2 | 3) => "Não encontrado".into(),
        Some(206) => "Caminho longo demais".into(),
        _ => e.to_string(),
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.as_os_str().to_string_lossy().to_lowercase() == b.as_os_str().to_string_lossy().to_lowercase()
}

/// "nome (2).ext", "nome (3).ext"... o primeiro que não existe, como o Windows.
pub fn unique(p: &Path) -> PathBuf {
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    // Pasta sem extensão: "Fotos.2024" fica "Fotos.2024 (2)".
    let (stem, ext) = if p.is_dir() { (p.file_name().unwrap_or_default().to_string_lossy().into_owned(), String::new()) } else { (stem, ext) };
    (2..).map(|n| p.with_file_name(format!("{stem} ({n}){ext}"))).find(|c| c.symlink_metadata().is_err()).unwrap()
}

fn walk(src: &Path, dst: &Path, out: &mut Vec<Entry>, st: &State) {
    match std::fs::symlink_metadata(src) {
        Ok(m) if m.is_dir() => {
            out.push(Entry { src: src.into(), dst: dst.into(), dir: true, size: 0 });
            match std::fs::read_dir(src) {
                Ok(rd) => {
                    for e in rd.flatten() {
                        walk(&e.path(), &dst.join(e.file_name()), out, st);
                    }
                }
                Err(e) => st.error(src, reason(&e)),
            }
        }
        Ok(m) => out.push(Entry { src: src.into(), dst: dst.into(), dir: false, size: m.len() }),
        Err(e) => st.error(src, reason(&e)),
    }
}

/// Bytes livres no disco de `dir`.
pub fn free_space(dir: &Path) -> Option<u64> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let w = windows::core::HSTRING::from(dir.as_os_str());
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(&w, Some(&mut free as *mut u64), None, None) }.ok()?;
    Some(free)
}

/// Copia (ou move, com `mv`) `items` para dentro da pasta `dest`. `free` diz o espaço livre:
/// sem espaço para tudo, nada começa.
pub fn run(items: &[PathBuf], dest: &Path, mv: bool, st: &State, free: impl Fn(&Path) -> Option<u64>) {
    let pairs: Vec<(PathBuf, PathBuf)> = items.iter().map(|i| (i.clone(), dest.to_path_buf())).collect();
    run_to(&pairs, mv, st, free);
}

/// Como `run`, com uma pasta de destino para cada item (Tentar de novo).
pub fn run_to(pairs: &[(PathBuf, PathBuf)], mv: bool, st: &State, free: impl Fn(&Path) -> Option<u64>) {
    st.phase.store(PREPARING, Relaxed);
    let mut plan = Vec::new();
    for (item, dest) in pairs {
        let dest = dest.as_path();
        let Some(name) = item.file_name() else { continue };
        if same_path(item, dest) || dest.to_string_lossy().to_lowercase().starts_with(&format!("{}\\", item.to_string_lossy().to_lowercase())) {
            st.error(item, "Uma pasta não pode ir para dentro dela mesma");
            continue;
        }
        let mut dst = dest.join(name);
        if same_path(item, &dst) {
            if mv {
                continue;
            }
            // Colar na mesma pasta faz uma cópia ao lado.
            dst = unique(&dst);
        }
        // Mover no mesmo disco é só trocar o nome: instantâneo, sem espaço novo.
        if mv && dst.symlink_metadata().is_err() && std::fs::rename(item, &dst).is_ok() {
            st.files.fetch_add(1, Relaxed);
            st.files_done.fetch_add(1, Relaxed);
            continue;
        }
        walk(item, &dst, &mut plan, st);
    }
    let files = plan.iter().filter(|e| !e.dir).count() as u32;
    st.files.fetch_add(files, Relaxed);
    st.total.store(plan.iter().map(|e| e.size).sum(), Relaxed);
    // Um arquivo substituído devolve o espaço do antigo.
    let need: u64 = plan.iter().filter(|e| !e.dir).map(|e| e.size.saturating_sub(e.dst.metadata().map_or(0, |m| m.len()))).sum();
    let Some(dest) = pairs.first().map(|p| p.1.as_path()) else {
        st.phase.store(DONE, Relaxed);
        return;
    };
    if let Some(f) = free(dest) {
        if need > f {
            st.need.store(need, Relaxed);
            st.free.store(f, Relaxed);
            st.phase.store(NO_SPACE, Relaxed);
            return;
        }
    }
    st.phase.store(COPYING, Relaxed);
    // Uma lista agendada pode apontar para uma pasta que ainda não existe.
    for (_, d) in pairs {
        let _ = std::fs::create_dir_all(d);
    }
    let mut base = 0u64;
    for e in &plan {
        if !wait_pause(st) {
            break;
        }
        if e.dir {
            if let Err(err) = std::fs::create_dir_all(&e.dst) {
                st.error(&e.src, reason(&err));
            }
            continue;
        }
        *st.current.lock().unwrap() = e.src.display().to_string();
        let ok = copy_file(e, st, base);
        base += e.size;
        st.done.store(base, Relaxed);
        st.files_done.fetch_add(1, Relaxed);
        if ok == Some(true) && mv {
            if let Err(err) = std::fs::remove_file(&e.src) {
                st.error(&e.src, reason(&err));
            }
        }
        if ok.is_none() {
            break;
        }
    }
    if mv && !st.cancel.load(Relaxed) {
        // Só sai a pasta que ficou vazia: o que falhou continua na origem.
        for e in plan.iter().rev().filter(|e| e.dir) {
            let _ = std::fs::remove_dir(&e.src);
        }
    }
    st.current.lock().unwrap().clear();
    st.phase.store(if st.cancel.load(Relaxed) { CANCELED } else { DONE }, Relaxed);
}

/// Para tentar de novo só o que falhou: cada arquivo da lista de erros com a pasta onde ele ia.
/// Erros fora dos itens (uma pasta dentro dela mesma) ficam de fora.
pub fn retry_pairs(items: &[PathBuf], dest: &Path, errors: &[(String, String)]) -> Vec<(PathBuf, PathBuf)> {
    let mut out: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (p, _) in errors {
        let p = PathBuf::from(p);
        let Some(item) = items.iter().find(|i| p.starts_with(i)) else { continue };
        let (Some(name), Ok(rel)) = (item.file_name(), p.strip_prefix(item)) else { continue };
        let target = dest.join(name).join(rel);
        let Some(dir) = target.parent() else { continue };
        if !same_path(&target, &p) && !out.iter().any(|o| o.0 == p) {
            out.push((p.clone(), dir.to_path_buf()));
        }
    }
    out
}

/// Espera enquanto pausado. False: cancelado.
fn wait_pause(st: &State) -> bool {
    while st.paused.load(Relaxed) && !st.cancel.load(Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
    !st.cancel.load(Relaxed)
}

/// Some(true): copiado (e conferido). Some(false): pulado ou com erro, segue o próximo.
/// None: parar tudo (cancelado ou disco cheio).
fn copy_file(e: &Entry, st: &State, base: u64) -> Option<bool> {
    let mut dst = e.dst.clone();
    let mut replace = false;
    if dst.symlink_metadata().is_ok() {
        st.asking.store(st.policy.load(Relaxed) == ASK, Relaxed);
        while st.policy.load(Relaxed) == ASK {
            if st.cancel.load(Relaxed) {
                st.asking.store(false, Relaxed);
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        st.asking.store(false, Relaxed);
        match st.policy.load(Relaxed) {
            SKIP => return Some(false),
            KEEP_BOTH => dst = unique(&dst),
            _ => replace = true,
        }
    }
    if let Err(err) = copy_with_progress(&e.src, &dst, replace, st, base) {
        if st.cancel.load(Relaxed) {
            return None;
        }
        let full = matches!(err.raw_os_error(), Some(39 | 112));
        st.error(&e.src, reason(&err));
        return if full { None } else { Some(false) };
    }
    if st.verify.load(Relaxed) {
        match (sha256(&e.src, st), sha256(&dst, st)) {
            (Ok(a), Ok(b)) if a == b => {}
            (Ok(_), Ok(_)) => {
                st.error(&e.src, "O hash da cópia não confere");
                return Some(false);
            }
            (Err(err), _) | (_, Err(err)) => {
                if st.cancel.load(Relaxed) {
                    return None;
                }
                st.error(&e.src, reason(&err));
                return Some(false);
            }
        }
    }
    Some(true)
}

struct Ctx<'a> {
    st: &'a State,
    base: u64,
}

unsafe extern "system" fn progress(
    _total: i64,
    moved: i64,
    _stream: i64,
    _stream_moved: i64,
    _n: u32,
    _why: windows::Win32::Storage::FileSystem::LPPROGRESS_ROUTINE_CALLBACK_REASON,
    _src: windows::Win32::Foundation::HANDLE,
    _dst: windows::Win32::Foundation::HANDLE,
    data: *const core::ffi::c_void,
) -> windows::Win32::Storage::FileSystem::COPYPROGRESSROUTINE_PROGRESS {
    use windows::Win32::Storage::FileSystem::{PROGRESS_CANCEL, PROGRESS_CONTINUE};
    let ctx = &*(data as *const Ctx);
    // A pausa segura o Windows aqui, no meio do arquivo.
    if !wait_pause(ctx.st) {
        return PROGRESS_CANCEL;
    }
    ctx.st.done.store(ctx.base + moved as u64, Relaxed);
    PROGRESS_CONTINUE
}

/// Cancelado no meio, o Windows apaga o pedaço já escrito.
fn copy_with_progress(src: &Path, dst: &Path, replace: bool, st: &State, base: u64) -> std::io::Result<()> {
    use windows::Win32::Storage::FileSystem::{CopyFileExW, COPYFILE_FLAGS, COPY_FILE_FAIL_IF_EXISTS};
    let ctx = Ctx { st, base };
    let (s, d) = (windows::core::HSTRING::from(src.as_os_str()), windows::core::HSTRING::from(dst.as_os_str()));
    // Sem a escolha de substituir, nunca escreve por cima, nem se o arquivo aparecer agora.
    let flags = if replace { COPYFILE_FLAGS(0) } else { COPY_FILE_FAIL_IF_EXISTS };
    unsafe { CopyFileExW(&s, &d, Some(progress), Some(&ctx as *const Ctx as _), None, flags) }.map_err(|e| std::io::Error::from_raw_os_error((e.code().0 & 0xFFFF) as i32))
}

/// SHA-256 pelo Windows (BCrypt), lido em pedaços de 1 MB.
pub fn sha256(path: &Path, st: &State) -> std::io::Result<[u8; 32]> {
    use std::io::Read;
    use windows::Win32::Security::Cryptography::*;
    let mut f = std::fs::File::open(path)?;
    let mut h = BCRYPT_HASH_HANDLE::default();
    let fail = |_: windows::core::Error| std::io::Error::other("BCrypt");
    unsafe { BCryptCreateHash(BCRYPT_SHA256_ALG_HANDLE, &mut h, None, None, 0) }.ok().map_err(fail)?;
    let mut buf = vec![0u8; 1 << 20];
    let res = (|| {
        loop {
            if !wait_pause(st) {
                return Err(std::io::Error::from_raw_os_error(1223));
            }
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            unsafe { BCryptHashData(h, &buf[..n], 0) }.ok().map_err(fail)?;
        }
        let mut out = [0u8; 32];
        unsafe { BCryptFinishHash(h, &mut out, 0) }.ok().map_err(fail)?;
        Ok(out)
    })();
    unsafe {
        let _ = BCryptDestroyHash(h);
    }
    res
}

/// Os arquivos copiados ou recortados no Explorer: (itens, recortado).
pub fn clipboard() -> Option<(Vec<PathBuf>, bool)> {
    use windows::Win32::System::DataExchange::*;
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_HDROP;
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    unsafe {
        OpenClipboard(None).ok()?;
        let read = || -> Option<(Vec<PathBuf>, bool)> {
            let h = GetClipboardData(CF_HDROP.0 as u32).ok()?;
            let drop = HDROP(h.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            let items: Vec<PathBuf> = (0..n)
                .map(|i| {
                    let mut buf = vec![0u16; DragQueryFileW(drop, i, None) as usize + 1];
                    let len = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                    PathBuf::from(String::from_utf16_lossy(&buf[..len]))
                })
                .collect();
            // "Preferred DropEffect" 2 = recortar.
            let fmt = RegisterClipboardFormatW(windows::core::w!("Preferred DropEffect"));
            let cut = GetClipboardData(fmt).ok().is_some_and(|g| {
                let p = GlobalLock(windows::Win32::Foundation::HGLOBAL(g.0)) as *const u32;
                let v = !p.is_null() && *p & 2 != 0;
                let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(g.0));
                v
            });
            Some((items, cut))
        };
        let got = read();
        // Recortar só cola uma vez, como no Explorer.
        if got.as_ref().is_some_and(|g| g.1) {
            let _ = EmptyClipboard();
        }
        let _ = CloseClipboard();
        got.filter(|g| !g.0.is_empty())
    }
}

/// Ctrl+V do Explorer: o controle em foco quando a lista de arquivos de uma pasta do Explorer
/// está em foco (não a barra de endereço nem um nome sendo editado) e há arquivos copiados.
/// Chamado de dentro do gancho de teclado: só perguntas rápidas ao Windows.
pub fn explorer_paste_target() -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::DataExchange::IsClipboardFormatAvailable;
    use windows::Win32::System::Ole::CF_HDROP;
    use windows::Win32::UI::WindowsAndMessaging::*;
    let class = |h: HWND| {
        let mut b = [0u16; 64];
        let n = unsafe { GetClassNameW(h, &mut b) } as usize;
        String::from_utf16_lossy(&b[..n])
    };
    unsafe {
        let fg = GetForegroundWindow();
        if class(fg) != "CabinetWClass" {
            return None;
        }
        let mut gi = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        GetGUIThreadInfo(GetWindowThreadProcessId(fg, None), &mut gi).ok()?;
        let focus = gi.hwndFocus;
        if focus.is_invalid() || class(focus) != "DirectUIHWND" || class(GetParent(focus).ok()?) != "SHELLDLL_DefView" {
            return None;
        }
        IsClipboardFormatAvailable(CF_HDROP.0 as u32).ok()?;
        Some(focus)
    }
}

/// Manda o Ctrl+V de volta para o Explorer, como tecla enviada por programa (o gancho deixa passar).
pub fn send_paste() {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, ..Default::default() } },
    };
    // Com o Ctrl ainda segurado, só o V; senão o Ctrl vai junto e é solto no fim.
    let held = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) } as u16 & 0x8000 != 0;
    let list: Vec<INPUT> = if held {
        vec![key(VK_V, false), key(VK_V, true)]
    } else {
        vec![key(VK_CONTROL, false), key(VK_V, false), key(VK_V, true), key(VK_CONTROL, true)]
    };
    unsafe {
        SendInput(&list, std::mem::size_of::<INPUT>() as i32);
    }
}

/// A pasta aberta na aba do Explorer que contém `focus` (as abas do Windows 11 dividem a janela).
pub fn explorer_folder(focus: windows::Win32::Foundation::HWND) -> Option<PathBuf> {
    use windows::core::Interface;
    use windows::Win32::System::Com::*;
    use windows::Win32::System::Variant::VARIANT;
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::IsChild;
    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok()?;
        for i in 0..windows.Count().ok()? {
            let Ok(disp) = windows.Item(&VARIANT::from(i)) else { continue };
            let Ok(sp) = disp.cast::<IServiceProvider>() else { continue };
            let Ok(browser) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else { continue };
            let Ok(tab) = browser.GetWindow() else { continue };
            if tab != focus && !IsChild(tab, focus).as_bool() {
                continue;
            }
            let view: IFolderView = browser.QueryActiveShellView().ok()?.cast().ok()?;
            let folder: IPersistFolder2 = view.GetFolder().ok()?;
            let pidl = folder.GetCurFolder().ok()?;
            let name = SHGetNameFromIDList(pidl, SIGDN_FILESYSPATH);
            CoTaskMemFree(Some(pidl as *const _));
            let name = name.ok()?;
            let path = name.to_string().ok();
            CoTaskMemFree(Some(name.0 as *const _));
            return path.map(PathBuf::from);
        }
        None
    }
}

const BG_KEY: &str = "Software\\Classes\\Directory\\Background\\shell\\PrismaColar";
const DIR_KEY: &str = "Software\\Classes\\Directory\\shell\\PrismaColar";

/// "Colar com o Prisma" está no botão direito das pastas.
pub fn menu_on() -> bool {
    super::reg::exists(super::reg::Hive::User, BG_KEY)
}

/// Liga ou desliga o item, só para este usuário. Ligar de novo atualiza o caminho do .exe.
pub fn set_menu(on: bool) -> bool {
    use super::reg::{delete_key, write, Hive};
    if !on {
        // RegDeleteKeyW não apaga chave com subchave: primeiro o "command".
        for key in [BG_KEY, DIR_KEY] {
            delete_key(Hive::User, &format!("{key}\\command"));
            delete_key(Hive::User, key);
        }
        return !menu_on();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let sz = |s: &str| -> Vec<u8> { s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect() };
    let label = crate::i18n::tr("Colar com o Prisma").into_owned();
    let icon = format!("\"{}\",0", exe.display());
    let reg_sz = windows::Win32::System::Registry::REG_SZ.0;
    // No fundo da pasta, %V é a pasta aberta; numa pasta clicada, %1 é ela.
    [(BG_KEY, "%V"), (DIR_KEY, "%1")].iter().all(|(key, arg)| {
        let cmd = format!("\"{}\" \"--paste={arg}\"", exe.display());
        write(Hive::User, key, "", reg_sz, &sz(&label))
            && write(Hive::User, key, "Icon", reg_sz, &sz(&icon))
            && write(Hive::User, &format!("{key}\\command"), "", reg_sz, &sz(&cmd))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    /// Pasta de teste no %TEMP%, nunca no disco F:.
    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("prisma_copia_{name}_{}", std::process::id()));
        assert!(!d.to_string_lossy().to_uppercase().starts_with("F:"), "teste no disco F:");
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("src\\sub")).unwrap();
        std::fs::create_dir_all(d.join("dst")).unwrap();
        std::fs::write(d.join("src\\a.txt"), b"alfa").unwrap();
        std::fs::write(d.join("src\\sub\\b.bin"), vec![7u8; 3 << 20]).unwrap();
        d
    }

    fn state(policy: u8, verify: bool) -> State {
        let st = State::default();
        st.policy.store(policy, Relaxed);
        st.verify.store(verify, Relaxed);
        st
    }

    #[test]
    fn copies_tree_and_verifies() {
        let d = scratch("arvore");
        let st = state(SKIP, true);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        assert_eq!(st.phase.load(Relaxed), DONE);
        assert!(st.errors.lock().unwrap().is_empty(), "{:?}", st.errors.lock().unwrap());
        assert_eq!(std::fs::read(d.join("dst\\src\\a.txt")).unwrap(), b"alfa");
        assert_eq!(std::fs::read(d.join("dst\\src\\sub\\b.bin")).unwrap().len(), 3 << 20);
        assert_eq!(st.done.load(Relaxed), st.total.load(Relaxed));
        assert_eq!(st.files_done.load(Relaxed), 2);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn locked_file_is_listed_and_rest_continues() {
        let d = scratch("travado");
        // Aberto sem compartilhar nada: ninguém mais lê.
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(d.join("src\\a.txt")).unwrap();
        let st = state(SKIP, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        drop(lock);
        let errs = st.errors.lock().unwrap();
        assert_eq!(errs.len(), 1, "{errs:?}");
        assert!(errs[0].0.ends_with("a.txt") && errs[0].1 == "Em uso por outro programa", "{errs:?}");
        assert!(d.join("dst\\src\\sub\\b.bin").exists() && !d.join("dst\\src\\a.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn move_takes_the_source_away() {
        let d = scratch("mover");
        let st = state(SKIP, false);
        run(&[d.join("src\\a.txt"), d.join("src\\sub")], &d.join("dst"), true, &st, |_| None);
        assert_eq!(st.phase.load(Relaxed), DONE);
        assert!(d.join("dst\\a.txt").exists() && d.join("dst\\sub\\b.bin").exists());
        assert!(!d.join("src\\a.txt").exists() && !d.join("src\\sub").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn no_space_stops_before_writing() {
        let d = scratch("espaco");
        let st = state(SKIP, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| Some(1000));
        assert_eq!(st.phase.load(Relaxed), NO_SPACE);
        assert_eq!(st.need.load(Relaxed), 4 + (3 << 20));
        assert!(!d.join("dst\\src").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hash_tells_files_apart() {
        let d = scratch("hash");
        let st = State::default();
        std::fs::write(d.join("dst\\a.txt"), b"alfa").unwrap();
        std::fs::write(d.join("dst\\c.txt"), b"alfA").unwrap();
        let h = |p: &str| sha256(&d.join(p), &st).unwrap();
        assert_eq!(h("src\\a.txt"), h("dst\\a.txt"));
        assert_ne!(h("src\\a.txt"), h("dst\\c.txt"));
        // SHA-256 de "alfa", conferido com outra ferramenta.
        let hex: String = h("src\\a.txt").iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(hex, "a405eba78bf2e6db44ebe0b28bbc9cdc449f9ac990d2029c50a15e6853cfdf20");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn conflicts_follow_the_choice() {
        let d = scratch("conflito");
        std::fs::create_dir_all(d.join("dst\\src")).unwrap();
        std::fs::write(d.join("dst\\src\\a.txt"), b"velho").unwrap();
        let st = state(SKIP, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        assert_eq!(std::fs::read(d.join("dst\\src\\a.txt")).unwrap(), b"velho");
        let st = state(KEEP_BOTH, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        assert_eq!(std::fs::read(d.join("dst\\src\\a (2).txt")).unwrap(), b"alfa");
        let st = state(REPLACE, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        assert_eq!(std::fs::read(d.join("dst\\src\\a.txt")).unwrap(), b"alfa");
        // Colar na mesma pasta faz "src (2)" ao lado.
        let st = state(ASK, false);
        run(&[d.join("src")], &d, false, &st, |_| None);
        assert!(d.join("src (2)\\a.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn folder_into_itself_is_refused() {
        let d = scratch("dentro");
        let st = state(SKIP, false);
        run(&[d.join("src")], &d.join("src\\sub"), false, &st, |_| None);
        assert_eq!(st.errors.lock().unwrap().len(), 1);
        assert!(!d.join("src\\sub\\src").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn retry_copies_only_what_failed() {
        let d = scratch("tentar");
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(d.join(r"src\sub\b.bin")).unwrap();
        let st = state(SKIP, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        drop(lock);
        let errs = st.errors.lock().unwrap().clone();
        assert_eq!(errs.len(), 1, "{errs:?}");
        let pairs = retry_pairs(&[d.join("src")], &d.join("dst"), &errs);
        assert_eq!(pairs, vec![(d.join(r"src\sub\b.bin"), d.join(r"dst\src\sub"))]);
        // A cópia boa não é tocada de novo.
        std::fs::write(d.join(r"dst\src\a.txt"), b"mexido").unwrap();
        let st = state(REPLACE, false);
        run_to(&pairs, false, &st, |_| None);
        assert!(st.errors.lock().unwrap().is_empty());
        assert_eq!(std::fs::read(d.join(r"dst\src\sub\b.bin")).unwrap().len(), 3 << 20);
        assert_eq!(std::fs::read(d.join(r"dst\src\a.txt")).unwrap(), b"mexido");
        // Erro fora dos itens não volta.
        assert!(retry_pairs(&[d.join("src")], &d.join("dst"), &[(r"Z:\x".into(), "?".into())]).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn missing_destination_is_created() {
        let d = scratch("criar");
        let st = state(SKIP, false);
        run(&[d.join(r"src\a.txt")], &d.join(r"dst\nova\pasta"), false, &st, |_| None);
        assert_eq!(std::fs::read(d.join(r"dst\nova\pasta\a.txt")).unwrap(), b"alfa");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn cancel_stops() {
        let d = scratch("cancelar");
        let st = state(SKIP, false);
        st.cancel.store(true, Relaxed);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        assert_eq!(st.phase.load(Relaxed), CANCELED);
        assert!(!d.join("dst\\src\\a.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
