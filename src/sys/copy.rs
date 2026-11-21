//! Cópia do "Colar com o Prisma": um trabalho com todos os itens colados, pausa no meio do
//! arquivo, conferência por SHA-256, o mesmo destino para todos os arquivos que já existem e a
//! lista de erros sem parar o resto. Nada roda dentro do Explorer: ele só abre o prisma.exe.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::Sha;

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
/// Conferindo o SHA-256 das cópias, depois de copiar tudo.
pub const VERIFYING: u8 = 5;

/// Arquivo a partir deste tamanho é copiado sem o cache do Windows, como fazem o Explorer e o
/// TeraCopy: com cache, um arquivo de vários GB enche a memória e a cópia perde velocidade.
const NO_CACHE_FROM: u64 = 256 << 20;

#[derive(Default)]
pub struct State {
    pub total: AtomicU64,
    pub done: AtomicU64,
    pub files: AtomicU32,
    pub files_done: AtomicU32,
    /// Arquivos e bytes que chegaram ao destino, sem os pulados e os com erro: vão para o histórico.
    pub ok_files: AtomicU32,
    pub ok_bytes: AtomicU64,
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
    /// Bytes já lidos na conferência, de `check_total` (origem e destino somados).
    pub checked: AtomicU64,
    pub check_total: AtomicU64,
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
    /// O destino do item não existia no plano: nada aqui dentro pode já existir.
    new: bool,
}

/// O que aconteceu com um arquivo.
enum Done {
    /// Copiado; com a conferência ligada, leva o SHA-256 da origem lido durante a cópia.
    Copied(PathBuf, Option<[u8; 32]>),
    /// Movido no mesmo disco só trocando o nome.
    Moved,
    /// Pulado ou com erro: segue o próximo.
    Skipped,
    /// Parar tudo: cancelado ou disco cheio.
    Stop,
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

/// O mesmo arquivo ou pasta, também por outro caminho (unidade mapeada, `subst`, junção): mover
/// por cima de si mesmo apagaria o arquivo. O texto igual resolve sem abrir nada.
fn same_file(a: &Path, b: &Path) -> bool {
    same_path(a, b) || file_id(a).is_some_and(|x| file_id(b) == Some(x))
}

/// Número de série do volume e número do arquivo nele.
fn file_id(p: &Path) -> Option<(u32, u32, u32)> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Storage::FileSystem::{GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION};
    // Só ler atributos, sem travar ninguém; FILE_FLAG_BACKUP_SEMANTICS abre pasta também.
    let f = std::fs::OpenOptions::new().access_mode(0x80).share_mode(7).custom_flags(0x0200_0000).open(p).ok()?;
    let mut bi = BY_HANDLE_FILE_INFORMATION::default();
    unsafe { GetFileInformationByHandle(windows::Win32::Foundation::HANDLE(f.as_raw_handle()), &mut bi) }.ok()?;
    Some((bi.dwVolumeSerialNumber, bi.nFileIndexHigh, bi.nFileIndexLow))
}

/// "nome (2).ext", "nome (3).ext"... o primeiro que não existe, como o Windows.
pub fn unique(p: &Path) -> PathBuf {
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    // Pasta sem extensão: "Fotos.2024" fica "Fotos.2024 (2)".
    let (stem, ext) = if p.is_dir() { (p.file_name().unwrap_or_default().to_string_lossy().into_owned(), String::new()) } else { (stem, ext) };
    (2..).map(|n| p.with_file_name(format!("{stem} ({n}){ext}"))).find(|c| c.symlink_metadata().is_err()).unwrap()
}

fn walk(src: &Path, dst: &Path, new: bool, out: &mut Vec<Entry>, st: &State) {
    use std::os::windows::fs::MetadataExt;
    if st.cancel.load(Relaxed) {
        return;
    }
    match std::fs::symlink_metadata(src) {
        // Junção ou link de pasta (pasta com reparse point): seguir copiaria outra pasta inteira, ou
        // entraria num laço sem fim.
        Ok(m) if m.file_attributes() & 0x410 == 0x410 => st.error(src, "Link de pasta, não copiado"),
        Ok(m) if m.is_dir() => {
            out.push(Entry { src: src.into(), dst: dst.into(), dir: true, size: 0, new });
            match std::fs::read_dir(src) {
                Ok(rd) => {
                    for e in rd.flatten() {
                        walk(&e.path(), &dst.join(e.file_name()), new, out, st);
                    }
                }
                Err(e) => st.error(src, reason(&e)),
            }
        }
        Ok(m) => out.push(Entry { src: src.into(), dst: dst.into(), dir: false, size: m.len(), new }),
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

/// Copia (ou move, com `mv`) cada item para dentro da sua pasta de destino. `free` diz o espaço
/// livre: sem espaço para tudo, nada começa.
pub fn run_to(pairs: &[(PathBuf, PathBuf)], mv: bool, st: &State, free: impl Fn(&Path) -> Option<u64>) {
    st.phase.store(PREPARING, Relaxed);
    let mut plan = Vec::new();
    // Onde começa no plano o que vai para cada destino, para somar o espaço de cada disco.
    let mut groups: Vec<(usize, &Path)> = Vec::new();
    for (item, dest) in pairs {
        if st.cancel.load(Relaxed) {
            st.phase.store(CANCELED, Relaxed);
            return;
        }
        let dest = dest.as_path();
        let Some(name) = item.file_name() else { continue };
        if same_file(item, dest) || dest.to_string_lossy().to_lowercase().starts_with(&format!("{}\\", item.to_string_lossy().to_lowercase())) {
            st.error(item, "Uma pasta não pode ir para dentro dela mesma");
            continue;
        }
        let mut dst = dest.join(name);
        if same_file(item, &dst) {
            if mv {
                continue;
            }
            // Colar na mesma pasta faz uma cópia ao lado.
            dst = unique(&dst);
        }
        let new = dst.symlink_metadata().is_err();
        // Mover no mesmo disco é só trocar o nome: instantâneo, sem espaço novo.
        if mv && new && std::fs::rename(item, &dst).is_ok() {
            st.files.fetch_add(1, Relaxed);
            st.files_done.fetch_add(1, Relaxed);
            st.ok_files.fetch_add(1, Relaxed);
            continue;
        }
        groups.push((plan.len(), dest));
        walk(item, &dst, new, &mut plan, st);
    }
    if st.cancel.load(Relaxed) {
        st.phase.store(CANCELED, Relaxed);
        return;
    }
    let files = plan.iter().filter(|e| !e.dir).count() as u32;
    st.files.fetch_add(files, Relaxed);
    st.total.store(plan.iter().map(|e| e.size).sum(), Relaxed);
    if pairs.is_empty() {
        st.phase.store(DONE, Relaxed);
        return;
    }
    for (dest, need) in needs(&plan, &groups, mv, st.policy.load(Relaxed) == REPLACE) {
        if let Some(f) = free(dest) {
            if need > f {
                st.need.store(need, Relaxed);
                st.free.store(f, Relaxed);
                st.phase.store(NO_SPACE, Relaxed);
                return;
            }
        }
    }
    st.phase.store(COPYING, Relaxed);
    // Uma lista agendada pode apontar para uma pasta que ainda não existe.
    for (_, d) in pairs {
        let _ = std::fs::create_dir_all(d);
    }
    // As pastas primeiro, numa passada: depois os arquivos podem ir vários ao mesmo tempo.
    for e in plan.iter().filter(|e| e.dir) {
        if !wait_pause(st) {
            break;
        }
        if let Err(err) = std::fs::create_dir_all(&e.dst) {
            st.error(&e.src, reason(&err));
        }
    }
    let files: Vec<&Entry> = plan.iter().filter(|e| !e.dir).collect();
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    // (origem, cópia, tamanho, hash da origem) de cada arquivo que chegou: a conferência e o mover
    // vêm no fim.
    let copied = Mutex::new(Vec::new());
    let workers = if files.len() > 1 { workers(pairs) } else { 1 };
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                let mut buf = Vec::new();
                let mut shown: Option<Instant> = None;
                while !stop.load(Relaxed) && wait_pause(st) {
                    let Some(e) = files.get(next.fetch_add(1, Relaxed)) else { break };
                    // Com milhares de arquivos pequenos, trocar o nome na tela a cada um só gasta.
                    if e.size >= 1 << 20 || shown.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) {
                        *st.current.lock().unwrap() = e.src.display().to_string();
                        shown = Some(Instant::now());
                    }
                    let verify = st.verify.load(Relaxed);
                    if verify && buf.is_empty() {
                        buf = chunk();
                    }
                    let mut moved = 0;
                    let r = copy_file(e, st, mv, verify.then(|| aligned(&mut buf)), &mut moved);
                    st.done.fetch_add(e.size.saturating_sub(moved), Relaxed);
                    st.files_done.fetch_add(1, Relaxed);
                    match r {
                        Done::Copied(dst, h) => {
                            st.ok_files.fetch_add(1, Relaxed);
                            st.ok_bytes.fetch_add(e.size, Relaxed);
                            copied.lock().unwrap().push((e.src.clone(), dst, e.size, h));
                        }
                        Done::Moved => {
                            st.ok_files.fetch_add(1, Relaxed);
                            st.ok_bytes.fetch_add(e.size, Relaxed);
                        }
                        Done::Skipped => {}
                        Done::Stop => stop.store(true, Relaxed),
                    }
                }
            });
        }
    });
    let copied = copied.into_inner().unwrap();
    let verify = st.verify.load(Relaxed);
    let good = if verify && !st.cancel.load(Relaxed) {
        check(&copied, st)
    } else {
        // Cancelado com a conferência ligada: nada foi conferido, a origem fica.
        vec![!verify; copied.len()]
    };
    if mv {
        // Mover só apaga a origem do que chegou (e bateu, com a conferência ligada).
        for ((src, ..), _) in copied.iter().zip(&good).filter(|g| *g.1) {
            if let Err(err) = std::fs::remove_file(src) {
                st.error(src, reason(&err));
            }
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

/// Bytes que cada disco de destino precisa ter livres. Mover no mesmo disco só troca o nome e não
/// ocupa nada; só substituir devolve o espaço do antigo (perguntar pode virar "manter os dois", que
/// ocupa tudo).
// shortcut: caminho de rede conta por pasta de destino, não por compartilhamento; basta enquanto
// os itens colados vão para uma pasta só.
fn needs<'a>(plan: &[Entry], groups: &[(usize, &'a Path)], mv: bool, replace: bool) -> Vec<(&'a Path, u64)> {
    let vol = |p: &Path| drive(p).unwrap_or_else(|| p.to_string_lossy().to_lowercase());
    let mut out: Vec<(&Path, u64)> = Vec::new();
    for (i, &(start, dest)) in groups.iter().enumerate() {
        let end = groups.get(i + 1).map_or(plan.len(), |g| g.0);
        let need: u64 = plan[start..end]
            .iter()
            .filter(|e| !e.dir)
            .map(|e| {
                if mv && drive(&e.src).is_some() && drive(&e.src) == drive(&e.dst) {
                    0
                } else if replace && !e.new {
                    e.size.saturating_sub(e.dst.metadata().map_or(0, |m| m.len()))
                } else {
                    e.size
                }
            })
            .sum();
        match out.iter_mut().find(|o| vol(o.0) == vol(dest)) {
            Some(o) => o.1 += need,
            None => out.push((dest, need)),
        }
    }
    out
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

/// A escolha para o que já existe ao tentar de novo: a mesma de antes, para "Manter os dois" não
/// virar substituir. Pular viraria não tentar nada: o que deu erro e tem destino é cópia nossa que
/// não bateu, e essa é substituída.
pub fn retry_policy(prev: u8) -> u8 {
    if prev == SKIP {
        REPLACE
    } else {
        prev
    }
}

/// Espera enquanto pausado. False: cancelado.
fn wait_pause(st: &State) -> bool {
    while st.paused.load(Relaxed) && !st.cancel.load(Relaxed) {
        std::thread::sleep(Duration::from_millis(100));
    }
    !st.cancel.load(Relaxed)
}

/// Copia (ou move) um arquivo. `buf` (de `aligned`) liga a conferência: a origem é lida uma vez só
/// e o hash sai junto. `moved` soma os bytes que já foram para `st.done`.
fn copy_file(e: &Entry, st: &State, mv: bool, buf: Option<&mut [u8]>, moved: &mut u64) -> Done {
    let mut dst = e.dst.clone();
    let mut replace = false;
    if !e.new && dst.symlink_metadata().is_ok() {
        st.asking.store(st.policy.load(Relaxed) == ASK, Relaxed);
        while st.policy.load(Relaxed) == ASK {
            if st.cancel.load(Relaxed) {
                st.asking.store(false, Relaxed);
                return Done::Stop;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        st.asking.store(false, Relaxed);
        match st.policy.load(Relaxed) {
            SKIP => return Done::Skipped,
            KEEP_BOTH => dst = unique(&dst),
            _ => replace = true,
        }
    }
    // Mover para uma pasta que já existe, no mesmo disco: troca o nome de cada arquivo, sem copiar.
    // Em outro disco o Windows recusa e segue a cópia.
    if mv && drive(&e.src) == drive(&dst) && rename_file(&e.src, &dst, replace) {
        return Done::Moved;
    }
    // Substituir escreve num arquivo ao lado e só troca no fim: cancelado ou com erro no meio, o
    // antigo continua inteiro.
    let tmp = replace.then(|| dst.with_file_name(format!("{}.prisma-tmp", dst.file_name().unwrap_or_default().to_string_lossy())));
    let target = tmp.as_ref().unwrap_or(&dst);
    let res = match buf {
        Some(b) => copy_verified(&e.src, target, e.size, replace, st, b, moved).map(Some),
        None => copy_with_progress(&e.src, target, e.size, replace, st, moved).map(|_| None),
    };
    let res = res.and_then(|h| tmp.as_ref().map_or(Ok(()), |t| replace_file(t, &dst)).map(|_| h));
    match res {
        Ok(h) => Done::Copied(dst, h),
        Err(err) => {
            if let Some(t) = &tmp {
                let _ = std::fs::remove_file(t);
            }
            if st.cancel.load(Relaxed) {
                return Done::Stop;
            }
            st.error(&e.src, reason(&err));
            if matches!(err.raw_os_error(), Some(39 | 112)) {
                Done::Stop
            } else {
                Done::Skipped
            }
        }
    }
}

/// Só leitura no destino faz a troca de nome dar "acesso negado"; a cópia leva o atributo junto, então
/// um arquivo copiado uma vez não seria substituído nunca mais.
/// Devolve os atributos de antes quando tirou, para pôr de volta se a troca falhar.
fn clear_readonly(p: &Path) -> Option<std::fs::Permissions> {
    let old = std::fs::metadata(p).ok()?.permissions();
    if !old.readonly() {
        return None;
    }
    let mut perm = old.clone();
    perm.set_readonly(false);
    std::fs::set_permissions(p, perm).ok()?;
    Some(old)
}

/// A troca falhou (destino aberto em outro programa): o antigo fica como estava, só leitura inclusive.
fn restore_readonly(p: &Path, old: Option<std::fs::Permissions>) {
    if let Some(perm) = old {
        let _ = std::fs::set_permissions(p, perm);
    }
}

/// Põe `tmp` no lugar de `dst`, que pode estar só leitura.
fn replace_file(tmp: &Path, dst: &Path) -> std::io::Result<()> {
    let old = clear_readonly(dst);
    let res = std::fs::rename(tmp, dst);
    if res.is_err() {
        restore_readonly(dst, old);
    }
    res
}

/// Troca o nome de um arquivo, por cima do destino só com `replace`. False em outro disco ou com erro.
fn rename_file(src: &Path, dst: &Path, replace: bool) -> bool {
    use windows::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVE_FILE_FLAGS};
    let old = if replace { clear_readonly(dst) } else { None };
    let (s, d) = (windows::core::HSTRING::from(src.as_os_str()), windows::core::HSTRING::from(dst.as_os_str()));
    let flags = if replace { MOVEFILE_REPLACE_EXISTING } else { MOVE_FILE_FLAGS(0) };
    let ok = unsafe { MoveFileExW(&s, &d, flags) }.is_ok();
    if !ok {
        restore_readonly(dst, old);
    }
    ok
}

/// Quantos arquivos ao mesmo tempo: 4 de SSD para SSD, onde muitos arquivos pequenos ganham com
/// isso; 1 com HD no meio, onde a cabeça pulando entre arquivos deixa tudo mais lento.
// shortcut: olha só o primeiro item; no Explorer os itens colados vêm de uma pasta só.
fn workers(pairs: &[(PathBuf, PathBuf)]) -> usize {
    let ssd = |p: &Path| drive(p).and_then(|d| super::disks::is_ssd(d.chars().next()?)) == Some(true);
    match pairs.first() {
        Some((item, dest)) if ssd(item) && ssd(dest) => 4,
        _ => 1,
    }
}

/// A letra do disco ("C:"), ou nada num caminho de rede.
pub fn drive(p: &Path) -> Option<String> {
    let s = p.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    s.get(..2).filter(|d| d.ends_with(':')).map(str::to_uppercase)
}

/// Confere cada cópia com a origem pelo SHA-256: true onde bate. O hash da origem já veio da cópia;
/// só falta quando a conferência foi ligada no meio, e aí a origem é lida aqui também.
fn check(copied: &[(PathBuf, PathBuf, u64, Option<[u8; 32]>)], st: &State) -> Vec<bool> {
    st.checked.store(0, Relaxed);
    st.check_total.store(copied.iter().map(|c| if c.3.is_some() { c.2 } else { c.2 * 2 }).sum(), Relaxed);
    st.phase.store(VERIFYING, Relaxed);
    let mut v = chunk();
    copied
        .iter()
        .map(|(src, dst, _, h)| {
            if st.cancel.load(Relaxed) {
                return false;
            }
            *st.current.lock().unwrap() = dst.display().to_string();
            let a = match h {
                Some(h) => Ok(*h),
                None => sha256(src, st, aligned(&mut v)),
            };
            let b = sha256(dst, st, aligned(&mut v));
            match (a, b) {
                (Ok(a), Ok(b)) if a == b => true,
                (Ok(_), Ok(_)) => {
                    st.error(src, "O hash da cópia não confere");
                    false
                }
                (Err(err), _) | (_, Err(err)) => {
                    if !st.cancel.load(Relaxed) {
                        st.error(src, reason(&err));
                    }
                    false
                }
            }
        })
        .collect()
}

struct Ctx<'a> {
    st: &'a State,
    /// Bytes deste arquivo já somados em `st.done`; vários arquivos andam ao mesmo tempo, então
    /// cada um soma só o que andou.
    moved: std::cell::Cell<u64>,
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
    let last = ctx.moved.replace(moved as u64);
    ctx.st.done.fetch_add((moved as u64).saturating_sub(last), Relaxed);
    PROGRESS_CONTINUE
}

/// Cancelado no meio, o Windows apaga o pedaço já escrito. `moved` recebe os bytes somados em `st.done`.
fn copy_with_progress(src: &Path, dst: &Path, size: u64, replace: bool, st: &State, moved: &mut u64) -> std::io::Result<()> {
    use windows::Win32::Storage::FileSystem::{CopyFileExW, COPYFILE_FLAGS, COPY_FILE_FAIL_IF_EXISTS, COPY_FILE_NO_BUFFERING};
    let ctx = Ctx { st, moved: std::cell::Cell::new(0) };
    let (s, d) = (windows::core::HSTRING::from(src.as_os_str()), windows::core::HSTRING::from(dst.as_os_str()));
    // Sem a escolha de substituir, nunca escreve por cima, nem se o arquivo aparecer agora.
    let flags = if replace { COPYFILE_FLAGS(0) } else { COPY_FILE_FAIL_IF_EXISTS };
    let flags = if size >= NO_CACHE_FROM { COPYFILE_FLAGS(flags.0 | COPY_FILE_NO_BUFFERING.0) } else { flags };
    let res = unsafe { CopyFileExW(&s, &d, Some(progress), Some(&ctx as *const Ctx as _), None, flags) };
    *moved = ctx.moved.get();
    res.map_err(|e| std::io::Error::from_raw_os_error((e.code().0 & 0xFFFF) as i32))
}

/// Copia lendo a origem uma vez só e devolve o SHA-256 dela, para a conferência ler só a cópia.
/// `buf` vem de `aligned`. Copia a data de modificação, a marca de arquivo baixado da internet e os
/// atributos (só leitura, oculto, sistema, arquivo).
// shortcut: outros fluxos alternativos e as permissões não vêm junto, como no CopyFileExW; se fizer
// falta, voltar a copiar pelo CopyFileExW e ler a origem duas vezes.
fn copy_verified(src: &Path, dst: &Path, size: u64, replace: bool, st: &State, buf: &mut [u8], moved: &mut u64) -> std::io::Result<[u8; 32]> {
    use std::io::{Read, Write};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    let nocache = size >= NO_CACHE_FROM;
    // FILE_FLAG_NO_BUFFERING e FILE_FLAG_SEQUENTIAL_SCAN.
    let mut f = std::fs::OpenOptions::new().read(true).custom_flags(if nocache { 0x2000_0000 | 0x0800_0000 } else { 0x0800_0000 }).open(src)?;
    let meta = f.metadata()?;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).custom_flags(if nocache { 0x2000_0000 } else { 0 });
    // Sem a escolha de substituir, nunca escreve por cima, nem se o arquivo aparecer agora.
    if replace {
        o.create(true).truncate(true);
    } else {
        o.create_new(true);
    }
    let mut out = o.open(dst)?;
    let res = (|| {
        let sha = Sha::new()?;
        loop {
            if !wait_pause(st) {
                return Err(std::io::Error::from_raw_os_error(1223));
            }
            let mut n = 0;
            while n < buf.len() {
                match f.read(&mut buf[n..])? {
                    0 => break,
                    r => n += r,
                }
            }
            sha.add(&buf[..n])?;
            // Sem cache, a escrita vai em blocos inteiros; o excesso do fim sai no set_len.
            out.write_all(&buf[..if nocache { n.next_multiple_of(4096) } else { n }])?;
            *moved += n as u64;
            st.done.fetch_add(n as u64, Relaxed);
            if n < buf.len() {
                break;
            }
        }
        if nocache {
            out.set_len(*moved)?;
        }
        out.set_modified(meta.modified()?)?;
        sha.finish()
    })();
    drop(out);
    let h = match res {
        Ok(h) => h,
        Err(e) => {
            let _ = std::fs::remove_file(dst);
            return Err(e);
        }
    };
    let zone = |p: &Path| format!("{}:Zone.Identifier", p.display());
    if let Ok(z) = std::fs::read(zone(src)) {
        let _ = std::fs::write(zone(dst), z);
    }
    // Por último: com só leitura, mais nada escreve nele.
    let attr = meta.file_attributes() & 0x27;
    if attr != 0 {
        use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_FLAGS_AND_ATTRIBUTES};
        let _ = unsafe { SetFileAttributesW(&windows::core::HSTRING::from(dst.as_os_str()), FILE_FLAGS_AND_ATTRIBUTES(attr)) };
    }
    Ok(h)
}

const CHUNK: usize = 4 << 20;

fn chunk() -> Vec<u8> {
    vec![0u8; CHUNK + 4096]
}

/// Os 4 MB de `v` que começam num múltiplo de 4 KB, como pede a leitura sem cache.
fn aligned(v: &mut [u8]) -> &mut [u8] {
    let off = v.as_ptr().align_offset(4096);
    &mut v[off..off + CHUNK]
}

/// SHA-256 pelo Windows (BCrypt), lido em pedaços de `buf` (de `aligned`) sem o cache do Windows:
/// a cópia recém-escrita vem do disco, não da memória. Soma o lido em `st.checked`.
fn sha256(path: &Path, st: &State, buf: &mut [u8]) -> std::io::Result<[u8; 32]> {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_FLAG_NO_BUFFERING | FILE_FLAG_SEQUENTIAL_SCAN.
    let mut f = std::fs::OpenOptions::new().read(true).custom_flags(0x2000_0000 | 0x0800_0000).open(path)?;
    let sha = Sha::new()?;
    loop {
        if !wait_pause(st) {
            return Err(std::io::Error::from_raw_os_error(1223));
        }
        let n = f.read(buf)?;
        if n == 0 {
            break;
        }
        sha.add(&buf[..n])?;
        st.checked.fetch_add(n as u64, Relaxed);
    }
    sha.finish()
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
        // Recortar é esvaziado só depois de mover (`clear_clipboard`): falhou, dá para colar de novo.
        let got = read();
        let _ = CloseClipboard();
        got.filter(|g| !g.0.is_empty())
    }
}

/// Depois de mover sem erro, a área de transferência não guarda mais os itens, como no Explorer.
/// Se nesse meio-tempo outra coisa foi copiada, ela fica.
pub fn clear_clipboard(items: &[PathBuf]) {
    use windows::Win32::System::DataExchange::*;
    if !clipboard().is_some_and(|(c, cut)| cut && c == items) {
        return;
    }
    unsafe {
        if OpenClipboard(None).is_ok() {
            let _ = EmptyClipboard();
            let _ = CloseClipboard();
        }
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

    fn run(items: &[PathBuf], dest: &Path, mv: bool, st: &State, free: impl Fn(&Path) -> Option<u64>) {
        let pairs: Vec<(PathBuf, PathBuf)> = items.iter().map(|i| (i.clone(), dest.to_path_buf())).collect();
        run_to(&pairs, mv, st, free);
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
        // A origem foi lida na cópia; a conferência leu só a cópia.
        assert_eq!(st.checked.load(Relaxed), st.check_total.load(Relaxed));
        assert_eq!(st.check_total.load(Relaxed), 4 + (3 << 20));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn verified_copy_hashes_the_source_and_keeps_readonly() {
        let d = scratch("hashcopia");
        let st = State::default();
        let src = d.join("src\\sub\\b.bin");
        let mut p = std::fs::metadata(&src).unwrap().permissions();
        p.set_readonly(true);
        std::fs::set_permissions(&src, p).unwrap();
        let mut v = chunk();
        let mut moved = 0;
        let h = copy_verified(&src, &d.join("dst\\b.bin"), 3 << 20, false, &st, aligned(&mut v), &mut moved).unwrap();
        assert_eq!(moved, 3 << 20);
        assert_eq!(h, sha256(&src, &st, aligned(&mut v)).unwrap());
        assert_eq!(std::fs::read(d.join("dst\\b.bin")).unwrap(), std::fs::read(&src).unwrap());
        assert!(std::fs::metadata(d.join("dst\\b.bin")).unwrap().permissions().readonly());
        // Sem a escolha de substituir, não escreve por cima.
        assert!(copy_verified(&src, &d.join("dst\\b.bin"), 3 << 20, false, &st, aligned(&mut v), &mut moved).is_err());
        assert!(std::fs::metadata(d.join("dst\\b.bin")).is_ok());
        for f in ["src\\sub\\b.bin", "dst\\b.bin"] {
            let mut p = std::fs::metadata(d.join(f)).unwrap().permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            p.set_readonly(false);
            std::fs::set_permissions(d.join(f), p).unwrap();
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn many_files_keep_the_counts_right() {
        let d = scratch("muitos");
        for i in 0..40 {
            std::fs::write(d.join(format!("src\\f{i}.txt")), vec![i as u8; 1000 + i]).unwrap();
        }
        let st = state(SKIP, true);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        assert_eq!(st.phase.load(Relaxed), DONE);
        assert!(st.errors.lock().unwrap().is_empty());
        assert_eq!(st.files_done.load(Relaxed), 42);
        assert_eq!(st.done.load(Relaxed), st.total.load(Relaxed));
        assert_eq!(st.checked.load(Relaxed), st.check_total.load(Relaxed));
        assert_eq!(std::fs::read(d.join("dst\\src\\f39.txt")).unwrap(), vec![39u8; 1039]);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn move_into_existing_folder_renames_each_file() {
        let d = scratch("moverpasta");
        std::fs::create_dir_all(d.join("dst\\sub")).unwrap();
        std::fs::write(d.join("dst\\sub\\velho.txt"), b"fica").unwrap();
        let st = state(SKIP, false);
        run(&[d.join("src\\sub")], &d.join("dst"), true, &st, |_| None);
        assert_eq!(st.phase.load(Relaxed), DONE);
        assert_eq!(std::fs::read(d.join("dst\\sub\\b.bin")).unwrap().len(), 3 << 20);
        assert!(d.join("dst\\sub\\velho.txt").exists());
        assert!(!d.join("src\\sub").exists());
        assert_eq!(st.done.load(Relaxed), st.total.load(Relaxed));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn drive_letter() {
        assert_eq!(drive(Path::new(r"c:\x\y")).as_deref(), Some("C:"));
        assert_eq!(drive(Path::new(r"\\?\D:\x")).as_deref(), Some("D:"));
        assert_eq!(drive(Path::new(r"\\server\share\x")), None);
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
        let mut v = chunk();
        let mut h = |p: &str| sha256(&d.join(p), &st, aligned(&mut v)).unwrap();
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
    fn free_space_counts_old_files_only_when_replacing() {
        let d = scratch("espaco2");
        std::fs::create_dir_all(d.join(r"dst\src\sub")).unwrap();
        std::fs::write(d.join(r"dst\src\sub\b.bin"), vec![1u8; 3 << 20]).unwrap();
        let st = state(KEEP_BOTH, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| Some(1000));
        assert_eq!(st.need.load(Relaxed), 4 + (3 << 20));
        let st = state(REPLACE, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| Some(3)); // need is stored only when short
        assert_eq!(st.need.load(Relaxed), 4);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn failed_replace_keeps_the_old_file() {
        let d = scratch("substituir");
        std::fs::create_dir_all(d.join(r"dst\src")).unwrap();
        std::fs::write(d.join(r"dst\src\a.txt"), b"velho").unwrap();
        let lock = std::fs::OpenOptions::new().read(true).share_mode(0).open(d.join(r"src\a.txt")).unwrap();
        let st = state(REPLACE, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        drop(lock);
        assert_eq!(std::fs::read(d.join(r"dst\src\a.txt")).unwrap(), b"velho");
        assert!(!d.join(r"dst\src\a.txt.prisma-tmp").exists());
        // Só o que chegou vai para o histórico.
        assert_eq!((st.files_done.load(Relaxed), st.ok_files.load(Relaxed), st.ok_bytes.load(Relaxed)), (2, 1, 3 << 20));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn folder_link_is_not_followed() {
        let d = scratch("link");
        let ok = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(d.join(r"src\sub\volta")).arg(d.join("src")).output().unwrap().status.success();
        assert!(ok);
        let st = state(SKIP, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        let errs = st.errors.lock().unwrap().clone();
        assert!(errs.len() == 1 && errs[0].1 == "Link de pasta, não copiado", "{errs:?}");
        assert!(d.join(r"dst\src\sub\b.bin").exists() && !d.join(r"dst\src\sub\volta").exists());
        let _ = std::fs::remove_dir(d.join(r"src\sub\volta"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn retry_keeps_the_choice() {
        assert_eq!(retry_policy(KEEP_BOTH), KEEP_BOTH);
        assert_eq!(retry_policy(REPLACE), REPLACE);
        assert_eq!(retry_policy(ASK), ASK);
        assert_eq!(retry_policy(SKIP), REPLACE);
    }

    #[test]
    fn moving_on_the_same_disk_needs_no_space() {
        let e = |src: &str, dst: &str, size| Entry { src: src.into(), dst: dst.into(), dir: false, size, new: true };
        let plan = [e(r"C:\a\x", r"C:\b\x", 100), e(r"D:\y", r"C:\b\y", 50), e(r"C:\z", r"E:\c\z", 7)];
        let groups = [(0, Path::new(r"C:\b")), (2, Path::new(r"E:\c"))];
        assert_eq!(needs(&plan, &groups, true, false), vec![(Path::new(r"C:\b"), 50), (Path::new(r"E:\c"), 7)]);
        assert_eq!(needs(&plan, &groups, false, false), vec![(Path::new(r"C:\b"), 150), (Path::new(r"E:\c"), 7)]);
        // Duas pastas no mesmo disco somam juntas.
        let groups = [(0, Path::new(r"C:\b")), (2, Path::new(r"c:\outra"))];
        assert_eq!(needs(&plan, &groups, false, false), vec![(Path::new(r"C:\b"), 157)]);
    }

    #[test]
    fn moving_onto_itself_by_another_path_keeps_the_file() {
        let d = scratch("mesmo");
        let ok = std::process::Command::new("cmd").args(["/c", "mklink", "/J"]).arg(d.join("atalho")).arg(d.join("src")).output().unwrap().status.success();
        assert!(ok);
        assert!(same_file(&d.join(r"atalho\a.txt"), &d.join(r"src\a.txt")));
        assert!(!same_file(&d.join(r"src\a.txt"), &d.join(r"src\sub\b.bin")));
        let st = state(REPLACE, false);
        run(&[d.join(r"atalho\a.txt")], &d.join("src"), true, &st, |_| None);
        assert_eq!(std::fs::read(d.join(r"src\a.txt")).unwrap(), b"alfa");
        let _ = std::fs::remove_dir(d.join("atalho"));
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

    #[test]
    fn replacing_a_read_only_copy_works() {
        let d = scratch("somenteleitura");
        let b = d.join(r"dst\src\a.txt");
        let novo = d.join("novo.txt");
        let ro = |p: &Path| std::fs::metadata(p).unwrap().permissions().readonly();
        let set_ro = |p: &Path, on: bool| {
            let mut perm = std::fs::metadata(p).unwrap().permissions();
            perm.set_readonly(on);
            std::fs::set_permissions(p, perm).unwrap();
        };
        let st = state(REPLACE, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        // Copiar por cima de um destino só leitura.
        std::fs::write(&b, b"velho").unwrap();
        set_ro(&b, true);
        let st = state(REPLACE, false);
        run(&[d.join("src")], &d.join("dst"), false, &st, |_| None);
        assert!(st.errors.lock().unwrap().is_empty(), "{:?}", st.errors.lock().unwrap());
        assert_eq!(std::fs::read(&b).unwrap(), b"alfa");
        // A troca do temporário, sozinha.
        set_ro(&b, true);
        std::fs::write(&novo, b"novo").unwrap();
        replace_file(&novo, &b).unwrap();
        assert_eq!(std::fs::read(&b).unwrap(), b"novo");
        // Mover (troca de nome no mesmo disco) por cima de um destino só leitura.
        set_ro(&b, true);
        std::fs::write(&novo, b"movido").unwrap();
        assert!(rename_file(&novo, &b, true));
        assert_eq!(std::fs::read(&b).unwrap(), b"movido");
        // Destino aberto em outro programa: a troca falha e o só leitura volta.
        set_ro(&b, true);
        std::fs::write(&novo, b"x").unwrap();
        {
            let _open = std::fs::OpenOptions::new().read(true).share_mode(0).open(&b).unwrap();
            assert!(replace_file(&novo, &b).is_err());
            assert!(ro(&b));
            assert!(!rename_file(&novo, &b, true));
            assert!(ro(&b));
        }
        assert_eq!(std::fs::read(&b).unwrap(), b"movido");
        set_ro(&b, false);
        let _ = std::fs::remove_dir_all(&d);
    }
}
