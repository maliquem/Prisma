//! Shaders: filtros do RetroArch (.slangp) por cima dos jogos, um perfil por jogo. O motor é outro processo
//! (`--shader-profile=`, `--shader-preview=`); esta página edita o perfil, baixa a biblioteca e liga e desliga.
//! Contrato com o motor: research_notes/Shaders/implementacao.md.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::Mutex;
use std::time::Instant;

use super::picker::{self, Target};
use super::*;
use crate::shader_profile::{self as sp, params, Keys, Param, Pass, Profile};

const ZIP_URL: &str = "https://github.com/libretro/slang-shaders/archive/refs/heads/master.zip";
const KEY_NAMES: [&str; 4] = ["Liga / desliga", "Menu no jogo", "Próximo perfil", "Comparar (segurar)"];
const INPUTS: [(&str, &str); 8] = [("auto", "Automática"), ("full", "Original"), ("240", "240 linhas"), ("480", "480 linhas"), ("720", "720 linhas"), ("d2", "÷2"), ("d3", "÷3"), ("d4", "÷4")];
/// Pedidos ao `WM_PICK_SHADER` (janelas do Windows, fora do desenho).
pub const PICK_EXE: usize = 0;
pub const PICK_IMAGE: usize = 1;
pub const PICK_IMPORT: usize = 2;

struct Browse {
    /// Carregar preset: troca todos os passes pelo escolhido.
    replace: bool,
    dir: usize,
    file: Option<String>,
    q: String,
}

#[derive(Default)]
struct St {
    profiles: Vec<(PathBuf, Profile)>,
    sel: usize,
    /// Como estava ao abrir ou no último Salvar perfil: Descartar volta a ele.
    saved: Option<Profile>,
    /// Mudança ainda não gravada (deslizante): grava um pouco depois, não a cada passo do arraste.
    save_at: Option<Instant>,
    /// Data do arquivo no último gravar ou ler: outra data é o menu do jogo mexendo.
    mtime: Option<std::time::SystemTime>,
    params: Vec<Param>,
    params_of: Vec<Pass>,
    pass: usize,
    q: String,
    only_changed: bool,
    browse: Option<Browse>,
    /// Presets da biblioteca, caminhos relativos com `\`.
    lib: Option<Vec<String>>,
    /// Arquivos que o último preset importado pede e não existem.
    missing: Vec<String>,
    anti_cheat: bool,
    hdr: bool,
    loaded: bool,
    left_h: f32,
}

static ST: Mutex<Option<St>> = Mutex::new(None);

#[derive(Default)]
struct Dl {
    on: bool,
    got: usize,
    total: usize,
    extracting: bool,
    err: bool,
    /// Motivo da falha, no download do modo Injeção.
    why: &'static str,
}
static DL: Mutex<Dl> = Mutex::new(Dl { on: false, got: 0, total: 0, extracting: false, err: false, why: "" });
/// Download do modo Injeção (`shader::files`).
static INJ: Mutex<Dl> = Mutex::new(Dl { on: false, got: 0, total: 0, extracting: false, err: false, why: "" });

/// Motores rodando: perfil, processo e se já terminou com o jogo ainda aberto (não volta a iniciar até o
/// jogo fechar).
struct Run {
    profile: PathBuf,
    /// Sem processo: anti-cheat, não inicia (e não lê a pasta do jogo de novo) até ele fechar.
    child: Option<Child>,
    ended: bool,
}
static RUNS: Mutex<Vec<Run>> = Mutex::new(Vec::new());
static PREVIEW: Mutex<Option<Child>> = Mutex::new(None);
static JOB: Mutex<isize> = Mutex::new(0);

fn st() -> std::sync::MutexGuard<'static, Option<St>> {
    ST.lock().unwrap_or_else(|e| e.into_inner())
}

fn load_all() -> Vec<(PathBuf, Profile)> {
    let mut v: Vec<(PathBuf, Profile)> = std::fs::read_dir(sp::profiles_dir())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("txt")))
        .filter_map(|p| Profile::load(&p).ok().map(|f| (p, f)))
        .collect();
    v.sort_by_key(|(_, p)| p.name.to_lowercase());
    v
}

fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

impl St {
    fn cur(&mut self) -> Option<&mut Profile> {
        self.profiles.get_mut(self.sel).map(|p| &mut p.1)
    }

    fn select(&mut self, i: usize) {
        self.flush();
        self.sel = i.min(self.profiles.len().saturating_sub(1));
        let (path, p) = match self.profiles.get(self.sel) {
            Some((path, p)) => (path.clone(), p.clone()),
            None => return,
        };
        self.anti_cheat = crate::meter::anti_cheat(&p.exe.to_string_lossy());
        self.mtime = mtime(&path);
        self.pass = p.passes.len().saturating_sub(1);
        self.missing.clear();
        self.saved = Some(p);
    }

    fn changed(&mut self) {
        self.save_at.get_or_insert_with(Instant::now);
    }

    /// Grava já o que estiver pendente.
    fn flush(&mut self) {
        if self.save_at.take().is_some() {
            if let Some((path, p)) = self.profiles.get(self.sel) {
                if p.save(path).is_ok() {
                    self.mtime = mtime(path);
                }
            }
        }
    }
}

/// Lê de novo os perfis, mantendo o escolhido pelo arquivo.
fn reload(st: &mut St, pick: Option<&Path>) {
    st.flush();
    let keep = pick.map(Path::to_path_buf).or_else(|| st.profiles.get(st.sel).map(|p| p.0.clone()));
    st.profiles = load_all();
    let i = keep.and_then(|k| st.profiles.iter().position(|p| p.0 == k)).unwrap_or(0);
    st.select(i);
}

fn monitor_hdr() -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
    crate::capture::monitor_color(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) }).hdr
}

/// `teclas.txt`, que o motor lê: o mesmo `mods,vk` do config.
fn write_keys(app: &App) {
    let [toggle, menu, next, compare] = app.cfg.hk_shader;
    let _ = Keys { toggle, menu, next, compare }.save(&sp::keys_path());
}

/// Algum perfil existe: só então o atalho liga/desliga ocupa a combinação no Windows.
pub fn in_use() -> bool {
    std::fs::read_dir(sp::profiles_dir()).is_ok_and(|mut d| d.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "txt"))))
}

/// Jogo escolhido na lista de programas ou pelo .exe: cria o perfil (se não houver) e abre.
pub fn add_game(app: &mut App, path: &str) {
    let exe = Path::new(path);
    let file = sp::path_for(exe);
    if !file.exists() {
        let name = app_name(&crate::sys::procs::exe_name(path)).to_string();
        let p = Profile { name, exe: exe.to_path_buf(), ..Profile::default() };
        if p.save(&file).is_err() {
            app.toast.info("Não deu para gravar o perfil");
            return;
        }
        if !sp::keys_path().exists() {
            write_keys(app);
        }
        app.register_hotkeys();
    }
    let mut g = st();
    let s = g.get_or_insert_with(St::default);
    reload(s, Some(&file));
}

/// Atalho liga/desliga: alterna o perfil do jogo em foco.
pub fn toggle_focused(app: &mut App) {
    let Some((_, _, exe)) = app.fg_info() else { return };
    let path = sp::path_for(Path::new(&exe));
    let Ok(mut p) = Profile::load(&path) else {
        let key = app.cfg.hk_shader[1].label();
        app.toast.info(&crate::i18n::tr("Este jogo não tem perfil de shader: {} abre o menu e cria um").replace("{}", &key));
        return;
    };
    p.on = !p.on;
    if p.save(&path).is_err() {
        return;
    }
    app.toast.info(if p.on { "Shader ligado" } else { "Shader desligado" });
    if let Some(s) = st().as_mut() {
        if let Some(e) = s.profiles.iter_mut().find(|e| e.0 == path) {
            e.1.on = p.on;
        }
        if s.profiles.get(s.sel).is_some_and(|e| e.0 == path) {
            s.mtime = mtime(&path);
        }
    }
    app.invalidate();
}

/// Atalho do menu num jogo sem perfil (ou com ele desligado): cria o perfil ligado, no modo padrão, com o nome do
/// catálogo ou o título da janela, e abre o motor já com o menu. Com o motor rodando, quem abre o menu é ele.
pub fn menu_focused(app: &mut App) {
    let Some((hwnd, pid, exe)) = app.fg_info() else { return };
    // Nem o Windows (Explorer incluído) nem o Prisma (este ou outra cópia, como o motor de outro perfil).
    let me = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    if pid == std::process::id() || exe.eq_ignore_ascii_case(&me) || crate::optimizer::is_system(&exe) {
        return;
    }
    let Some(full) = crate::sys::procs::image_path(pid) else { return };
    let path = sp::path_for(Path::new(&full));
    if running(&path) {
        return;
    }
    if crate::meter::anti_cheat(&full) {
        app.toast.info("Jogo com anti-cheat: o shader não entra nele");
        return;
    }
    let new = !path.exists();
    let mut p = if new {
        let low = full.to_lowercase();
        let title = crate::sys::procs::window_title(hwnd);
        let name = crate::sys::stores::games().into_iter().find(|e| e.path.to_lowercase() == low).map(|e| e.name).or_else(|| (!title.trim().is_empty()).then(|| title.trim().to_string()));
        Profile { name: name.unwrap_or_else(|| app_name(&exe).to_string()), exe: full.clone().into(), ..Profile::default() }
    } else {
        match Profile::load(&path) {
            Ok(p) => p,
            Err(_) => return,
        }
    };
    p.on = true;
    if p.save(&path).is_err() {
        app.toast.info("Não deu para gravar o perfil");
        return;
    }
    if !sp::keys_path().exists() {
        write_keys(app);
    }
    if new {
        // O liga/desliga passa a valer: agora há perfil.
        app.register_hotkeys();
        app.toast.info(&crate::i18n::tr("Perfil de shader criado: {}").replace("{}", &p.name));
    }
    let log = format!("{}.log", path.file_stem().unwrap_or_default().to_string_lossy());
    let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    for r in runs.iter_mut().filter(|r| r.profile == path) {
        if let Some(c) = r.child.take() {
            stop(c);
        }
    }
    runs.retain(|r| r.profile != path);
    if let Some(child) = spawn(&[format!("--shader-profile={}", path.display()), "--shader-menu".into()], &log) {
        runs.push(Run { profile: path.clone(), child: Some(child), ended: false });
    }
    drop(runs);
    if let Some(s) = st().as_mut() {
        reload(s, Some(&path));
    }
    app.invalidate();
}

/// Resposta das janelas do Windows (`WM_PICK_SHADER`).
pub fn picked(app: &mut App, kind: usize, path: String) {
    match kind {
        PICK_EXE => add_game(app, &path),
        PICK_IMAGE => preview(&path),
        _ => import(app, Path::new(&path)),
    }
}

/// Importar preset: copia para `shaders\` com os caminhos na biblioteca e troca os passes por ele, como o
/// Carregar preset.
fn import(app: &mut App, from: &Path) {
    let (to, missing) = match sp::import(from, &sp::root()) {
        Ok(x) => x,
        Err(e) => {
            crate::config::log(&format!("shader: importar {}: {e}", from.display()));
            app.toast.info("Não deu para importar o preset");
            return;
        }
    };
    let mut g = st();
    let Some(s) = g.as_mut() else { return };
    let Some(c) = s.cur() else { return };
    c.passes.clear();
    c.params.clear();
    sp::add_pass(c, to);
    s.pass = 0;
    s.missing = missing;
    s.lib = None;
    s.changed();
    s.flush();
    app.toast.info("Preset importado");
}

fn job() -> windows::Win32::Foundation::HANDLE {
    use windows::Win32::System::JobObjects::*;
    let mut j = JOB.lock().unwrap_or_else(|e| e.into_inner());
    if *j == 0 {
        unsafe {
            if let Ok(h) = CreateJobObjectW(None, None) {
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                // O motor fecha junto com o Prisma.
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let _ = SetInformationJobObject(h, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32);
                *j = h.0 as isize;
            }
        }
    }
    windows::Win32::Foundation::HANDLE(*j as *mut _)
}

/// Inicia o próprio Prisma com `args`, sem console; o que ele diz vai para `shaders\logs\<log>`.
fn spawn(args: &[String], log: &str) -> Option<Child> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    let dir = sp::root().join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    let out = std::fs::File::create(dir.join(log)).ok()?;
    let err = out.try_clone().ok()?;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let child = std::process::Command::new(std::env::current_exe().ok()?).args(args).stdin(std::process::Stdio::null()).stdout(out).stderr(err).creation_flags(CREATE_NO_WINDOW).spawn().ok()?;
    unsafe {
        let _ = windows::Win32::System::JobObjects::AssignProcessToJobObject(job(), windows::Win32::Foundation::HANDLE(child.as_raw_handle()));
    }
    Some(child)
}

fn stop(mut c: Child) {
    let _ = c.kill();
    let _ = c.wait();
}

/// Testar com imagem: janela do motor com `scene`, `bars` ou o caminho de uma imagem.
fn preview(img: &str) {
    let path = {
        let mut g = st();
        let Some(s) = g.as_mut() else { return };
        s.flush();
        let Some((p, _)) = s.profiles.get(s.sel) else { return };
        p.clone()
    };
    if let Some(old) = PREVIEW.lock().unwrap_or_else(|e| e.into_inner()).take() {
        stop(old);
    }
    let c = spawn(&[format!("--shader-preview={},{img}", path.display())], "preview.log");
    *PREVIEW.lock().unwrap_or_else(|e| e.into_inner()) = c;
}

/// Ciclo de vida, a cada leitura do monitor: um motor por perfil ligado cujo jogo está aberto e sem
/// anti-cheat. Com `on=0` depois de iniciado, o motor esconde e espera sozinho.
pub fn tick(app: &mut App) {
    let open: std::collections::HashSet<String> = app.snap.groups.iter().map(|g| g.exe.to_lowercase()).collect();
    let all = load_all();
    let mut runs = RUNS.lock().unwrap_or_else(|e| e.into_inner());
    let mut i = 0;
    while i < runs.len() {
        let r = &mut runs[i];
        let prof = all.iter().find(|p| p.0 == r.profile);
        let up = prof.is_some_and(|p| open.contains(&exe_of(&p.1)));
        if !up {
            if let Some(c) = runs.remove(i).child {
                stop(c);
            }
            continue;
        }
        if !r.ended {
            if let Some(code) = r.child.as_mut().and_then(|c| c.try_wait().ok().flatten()) {
                r.ended = true;
                if !code.success() {
                    crate::config::log(&format!("shader: o motor de {} saiu com erro ({}); veja shaders/logs", r.profile.display(), code.code().unwrap_or(-1)));
                }
            }
        }
        i += 1;
    }
    for (path, p) in &all {
        if starts(path, p, &open) && !runs.iter().any(|r| &r.profile == path) {
            if crate::meter::anti_cheat(&p.exe.to_string_lossy()) {
                runs.push(Run { profile: path.clone(), child: None, ended: true });
                continue;
            }
            let log = format!("{}.log", path.file_stem().unwrap_or_default().to_string_lossy());
            if let Some(child) = spawn(&[format!("--shader-profile={}", path.display())], &log) {
                runs.push(Run { profile: path.clone(), child: Some(child), ended: false });
            }
        }
    }
    drop(runs);
    // Depois de uma atualização do Prisma, quem já tinha baixado a Injeção recebe a desta versão sozinho.
    static STALE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !STALE.swap(true, std::sync::atomic::Ordering::Relaxed) && crate::shader::files::stale() {
        download_inject();
    }
    if app.page != Page::Shaders || app.main.is_none() {
        return;
    }
    // O menu do jogo grava no perfil: a página mostra na hora.
    if let Some(s) = st().as_mut() {
        if let Some((path, _)) = s.profiles.get(s.sel) {
            let path = path.clone();
            if s.save_at.is_none() && mtime(&path) != s.mtime {
                if let Ok(p) = Profile::load(&path) {
                    s.profiles[s.sel].1 = p;
                    s.mtime = mtime(&path);
                }
            }
        }
    }
    app.invalidate();
}

/// Só o perfil principal (`<exe>.txt`) inicia o motor; os `<exe>*.txt` são do `next`, dentro dele.
fn starts(path: &Path, p: &Profile, open: &std::collections::HashSet<String>) -> bool {
    p.on && path == sp::path_for(&p.exe) && open.contains(&exe_of(p))
}

/// O motor lê `teclas.txt` só ao abrir: fecha os que rodam e o próximo `tick` abre de novo.
fn restart() {
    for c in RUNS.lock().unwrap_or_else(|e| e.into_inner()).drain(..).filter_map(|r| r.child) {
        stop(c);
    }
}

fn exe_of(p: &Profile) -> String {
    p.exe.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default()
}

fn running(profile: &Path) -> bool {
    RUNS.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|r| r.profile == profile && !r.ended)
}

/// Baixa o zip do GitHub, extrai ao lado da biblioteca e troca só as pastas que vieram nele: o que o jogador pôs
/// lá dentro fica.
fn download(lib: PathBuf) {
    {
        let mut d = DL.lock().unwrap_or_else(|e| e.into_inner());
        if d.on {
            return;
        }
        *d = Dl { on: true, ..Dl::default() };
    }
    std::thread::spawn(move || {
        let ok = fetch(&lib).is_some();
        // Solta o DL antes do ST: o desenho pega os dois na ordem contrária.
        {
            let mut d = DL.lock().unwrap_or_else(|e| e.into_inner());
            d.on = false;
            d.err = !ok;
        }
        if let Some(s) = st().as_mut() {
            s.lib = None;
        }
    });
}

/// Baixa a biblioteca de fora da página Shaders: os filtros de upscale do Fullscreenizer vêm dela.
pub fn download_lib() {
    download(sp::lib_dir());
}

/// Download da biblioteca: `None` parado, `Some(fração)` baixando (1 extraindo); `Err` quando o último falhou.
pub fn lib_progress() -> Result<Option<f32>, ()> {
    let d = DL.lock().unwrap_or_else(|e| e.into_inner());
    match (d.on, d.err) {
        (true, _) => Ok(Some(if d.extracting { 1.0 } else if d.total > 0 { d.got as f32 / d.total as f32 } else { 0.0 })),
        (false, true) => Err(()),
        _ => Ok(None),
    }
}

fn fetch(lib: &Path) -> Option<()> {
    use std::os::windows::process::CommandExt;
    let parent = lib.parent()?;
    let tmp = parent.join(format!("prisma-shaders-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).ok()?;
    std::fs::create_dir_all(lib).ok()?;
    let zip = tmp.join("master.zip");
    let got = crate::sys::update::download_to(ZIP_URL, &zip, |got, total| {
        let mut d = DL.lock().unwrap_or_else(|e| e.into_inner());
        d.got = got;
        d.total = total;
    });
    let out = tmp.join("x");
    let q = |p: &Path| crate::sys::quick::ps_quote(&p.to_string_lossy());
    let res = got.ok().and_then(|_| {
        DL.lock().unwrap_or_else(|e| e.into_inner()).extracting = true;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let script = format!("$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; Expand-Archive -LiteralPath {} -DestinationPath {} -Force", q(&zip), q(&out));
        let st = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).creation_flags(CREATE_NO_WINDOW).status().ok()?;
        st.success().then_some(())?;
        let src = std::fs::read_dir(&out).ok()?.flatten().map(|e| e.path()).find(|p| p.is_dir())?;
        for e in std::fs::read_dir(&src).ok()?.flatten() {
            let to = lib.join(e.file_name());
            if to.is_dir() {
                std::fs::remove_dir_all(&to).ok()?;
            } else if to.exists() {
                std::fs::remove_file(&to).ok()?;
            }
            std::fs::rename(e.path(), &to).ok()?;
        }
        Some(())
    });
    let _ = std::fs::remove_dir_all(&tmp);
    res
}

/// Baixa o modo Injeção desta versão e reabre os motores, que escolhem o modo ao abrir.
fn download_inject() {
    {
        let mut d = INJ.lock().unwrap_or_else(|e| e.into_inner());
        if d.on {
            return;
        }
        *d = Dl { on: true, ..Dl::default() };
    }
    std::thread::spawn(|| {
        let r = crate::shader::files::download(|got, total| {
            let mut d = INJ.lock().unwrap_or_else(|e| e.into_inner());
            d.got = got;
            d.total = total;
        });
        if let Err(e) = r {
            crate::config::log(&format!("shader: download do modo Injeção falhou: {e}"));
        }
        {
            let mut d = INJ.lock().unwrap_or_else(|e| e.into_inner());
            d.on = false;
            d.err = r.is_err();
            d.why = r.err().unwrap_or("");
        }
        if r.is_ok() {
            restart();
        }
    });
}

/// Presets da biblioteca: os .slangp e os .slang fora das pastas `shaders\` (que são as passadas dos presets).
fn scan(lib: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                if !e.file_name().eq_ignore_ascii_case("shaders") {
                    walk(&p, base, out);
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("slangp") || x.eq_ignore_ascii_case("slang")) {
                if let Ok(r) = p.strip_prefix(base) {
                    out.push(r.to_string_lossy().into_owned());
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(lib, lib, &mut out);
    out.sort_by_key(|s| s.to_lowercase());
    out
}

fn top_dir(rel: &str) -> &str {
    rel.split_once('\\').map_or("", |d| d.0)
}

/// Presets do jogador (importados ou salvos), em `shaders\`, como `..\nome.slangp` relativo à biblioteca: na lista
/// eles formam a pasta "..", mostrada como Meus presets.
fn mine() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(sp::root())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("slangp")))
        .map(|e| format!("..\\{}", e.file_name().to_string_lossy()))
        .collect();
    v.sort_by_key(|s| s.to_lowercase());
    v
}

/// Salvar como preset: a cadeia ligada, juntada pelo motor, num .slangp em `shaders\` com o nome do perfil e
/// caminhos relativos, que funciona também na pasta `shaders\` do RetroArch.
fn export(app: &mut App, s: &mut St) {
    let Some(p) = s.cur().cloned() else { return };
    let root = sp::root();
    let name: String = p.name.chars().map(|c| if c.is_alphanumeric() || " -_.()".contains(c) { c } else { '_' }).collect();
    let to = sp::free_name(&root, if name.trim().is_empty() { "preset" } else { name.trim() });
    match crate::shader::slangp(&p, &root) {
        Ok(text) if crate::config::write_safe(&to, &text).is_ok() => {
            s.lib = None;
            app.toast.info(&crate::i18n::tr("Preset salvo em shaders\\{}").replace("{}", &to.file_name().unwrap_or_default().to_string_lossy()));
        }
        Ok(_) => app.toast.info("Não deu para gravar o preset"),
        Err(e) => {
            crate::config::log(&format!("shader: exportar {}: {e}", p.name));
            app.toast.info("Não deu para juntar os passes num preset");
        }
    }
}

fn pick(app: &App, kind: usize) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(Some(app.hub), crate::app::WM_PICK_SHADER, WPARAM(kind), LPARAM(0));
    }
}

/// Valor no passo do parâmetro, dentro do mínimo e do máximo.
fn snap(p: &Param, v: f32) -> f32 {
    let v = if p.step > 0.0 { p.min + ((v - p.min) / p.step).round() * p.step } else { v };
    v.clamp(p.min.min(p.max), p.max.max(p.min))
}

fn decimals(step: f32) -> usize {
    format!("{step}").split_once('.').map_or(0, |d| d.1.len().min(4))
}

fn fmt(v: f32, step: f32) -> String {
    format!("{v:.*}", decimals(step)).replace('.', crate::i18n::comma())
}

pub fn draw(ui: &mut Ui, app: &mut App, a: Rect) {
    let mut g = st();
    let s = g.get_or_insert_with(St::default);
    if !s.loaded {
        s.loaded = true;
        s.hdr = monitor_hdr();
        reload(s, None);
    }
    if s.save_at.is_some_and(|t| t.elapsed().as_millis() > 200) {
        s.flush();
    }
    if s.browse.is_some() {
        ui.block();
    }
    header(ui, a, "Shaders", "Filtros do RetroArch (.slang) por cima de qualquer jogo: CRT, scanlines, NTSC, nitidez. Um perfil por jogo.");
    let x = a.x + 28.0;
    let w = a.w - 56.0;
    let top = 88.0;
    let cw = ((w - 24.0) / 3.0).floor();
    game_card(ui, app, s, rect(x, top, cw, 176.0));
    lib_card(ui, s, rect(x + cw + 12.0, top, cw, 176.0));
    keys_card(ui, app, rect(x + 2.0 * (cw + 12.0), top, w - 2.0 * (cw + 12.0), 176.0));

    let y0 = top + 188.0;
    let h = a.h - 20.0 - y0;
    let lw = (w * 0.55).floor();
    if s.profiles.is_empty() {
        let r = rect(x, y0, w, h);
        ui.card(r);
        ui.c.text_wrap(rect(r.x + 16.0, r.y + 16.0, r.w - 32.0, 60.0), "Nenhum jogo ainda. Use Adicionar no cartão Jogo e escolha o .exe; depois baixe a biblioteca e adicione um passe.", 14.0, MUTED, Weight::Regular);
    } else {
        // Os parâmetros dependem dos passes: lê de novo só quando eles mudam.
        let passes = s.cur().map(|p| p.passes.clone()).unwrap_or_default();
        if passes != s.params_of {
            s.params = s.cur().map(|p| params(p)).unwrap_or_default();
            s.params_of = passes;
        }
        left_card(ui, app, s, rect(x, y0, lw, h));
        params_card(ui, s, rect(x + lw + 12.0, y0, w - lw - 12.0, h));
    }
    if s.browse.is_some() {
        browse_sheet(ui, app, s);
    }
}

fn game_card(ui: &mut Ui, app: &mut App, s: &mut St, r: Rect) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 140.0, 22.0), "Perfil do jogo");
    let ix = r.x + 16.0;
    let iw = r.w - 32.0;
    if let Some(p) = s.cur() {
        let mut on = p.on;
        if ui.check("shd-on", rect(r.right() - 110.0, r.y + 10.0, 100.0, 26.0), "Ligado", &mut on) {
            p.on = on;
            s.changed();
        }
    }
    let names: Vec<String> = s.profiles.iter().map(|p| p.1.name.clone()).collect();
    let mut sel = s.sel;
    if names.is_empty() {
        ui.c.text(rect(ix, r.y + 44.0, iw - 116.0, 36.0), "Nenhum jogo", 14.0, MUTED, Align::Left, Weight::Regular);
    } else if ui.select("shd-game", rect(ix, r.y + 44.0, iw - 116.0, 36.0), &names, &mut sel) {
        s.select(sel);
    }
    picker::add_button(ui, app, rect(r.right() - 124.0, r.y + 46.0, 108.0, 32.0), Target::ShaderGame);
    let Some((path, p)) = s.profiles.get(s.sel) else { return };
    let exe = p.exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let n = p.passes.len();
    let meta = crate::i18n::tr(if n == 1 { "{} · 1 passe" } else { "{} · {} passes" }).replacen("{}", &exe, 1).replacen("{}", &n.to_string(), 1);
    ui.c.text_fit(rect(ix, r.y + 88.0, iw, 20.0), &meta, 12.0, MUTED, Align::Left, Weight::Regular);
    let (state, color) = if s.anti_cheat {
        ("Anti-cheat: o shader não entra neste jogo", WARN)
    } else if running(path) {
        ("Aplicando agora", ACCENT)
    } else if p.on {
        ("Ligado: entra quando o jogo abrir", TEXT2)
    } else {
        ("Desligado", MUTED)
    };
    ui.c.text_fit(rect(ix, r.y + 110.0, iw, 20.0), state, 13.0, color, Align::Left, Weight::Regular);
}

fn lib_card(ui: &mut Ui, s: &mut St, r: Rect) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Biblioteca de shaders");
    let ix = r.x + 16.0;
    let iw = r.w - 32.0;
    let lib = sp::lib_dir();
    let (on, got, total, extracting, err) = {
        let d = DL.lock().unwrap_or_else(|e| e.into_inner());
        (d.on, d.got, d.total, d.extracting, d.err)
    };
    let have = lib.join("crt").is_dir();
    let status = if on && extracting {
        crate::i18n::tr("Extraindo… pode levar alguns minutos.").into_owned()
    } else if on {
        crate::i18n::tr("Baixando de github.com/libretro/slang-shaders: {} MB").replace("{}", &(got >> 20).to_string())
    } else if err {
        crate::i18n::tr("O download falhou. Tente de novo.").into_owned()
    } else if have {
        let n = s.lib.get_or_insert_with(|| scan(&lib)).len();
        crate::i18n::tr("Instalada: {} presets.").replace("{}", &n.to_string())
    } else {
        crate::i18n::tr("Não baixada. Não vem com o Prisma por causa das licenças.").into_owned()
    };
    let bw = 108.0;
    ui.c.text_wrap(rect(ix, r.y + 40.0, iw - bw - 12.0, 40.0), &status, 12.0, if err { WARN } else { MUTED }, Weight::Regular);
    if ui.button_ex("shd-dl", rect(r.right() - 16.0 - bw, r.y + 44.0, bw, 32.0), if have { "Atualizar" } else { "Baixar" }, !have, !on) {
        download(lib.clone());
    }
    if on {
        let bar = rect(ix, r.y + 86.0, iw, 4.0);
        ui.c.round(bar, 2.0, BORDER);
        let f = if extracting { 1.0 } else if total > 0 { got as f32 / total as f32 } else { 0.5 };
        ui.c.round(rect(bar.x, bar.y, bar.w * f.min(1.0), 4.0), 2.0, ACCENT);
    }
    let y = r.y + 108.0;
    ui.c.text_fit(rect(ix, y, iw - 100.0, 32.0), &lib.to_string_lossy(), 12.0, TEXT2, Align::Left, Weight::Regular);
    if ui.button("shd-lib-open", rect(r.right() - 96.0, y, 80.0, 32.0), "Abrir", false) {
        let _ = std::fs::create_dir_all(&lib);
        let _ = std::process::Command::new("explorer").arg(&lib).spawn();
    }
}

fn keys_card(ui: &mut Ui, app: &mut App, r: Rect) {
    ui.card(r);
    ui.title(rect(r.x + 16.0, r.y + 12.0, r.w - 32.0, 22.0), "Atalhos");
    let fw = (r.w * 0.5).min(190.0);
    let mut changed = false;
    for (i, name) in KEY_NAMES.iter().enumerate() {
        let y = r.y + 40.0 + i as f32 * 28.0;
        ui.c.text_fit(rect(r.x + 16.0, y, r.w - fw - 40.0, 26.0), name, 13.0, TEXT2, Align::Left, Weight::Regular);
        let mut hk = app.cfg.hk_shader[i];
        if ui.hotkey(&format!("shd-k{i}"), rect(r.right() - 16.0 - fw, y, fw, 26.0), &mut hk) {
            app.cfg.hk_shader[i] = hk;
            changed = true;
        }
    }
    if changed {
        app.cfg.save();
        write_keys(app);
        app.register_hotkeys();
        restart();
    }
    // O liga/desliga é do app; os outros três o motor registra quando abre.
    if let Some(e) = app.hk_errors.iter().find(|e| e.starts_with("Shader liga / desliga")) {
        let msg = crate::i18n::tr("{} já está em uso por outro programa").replace("{}", e.rsplit_once(": ").map_or("", |x| x.1).trim_end_matches(" já está em uso"));
        ui.c.text_fit(rect(r.x + 16.0, r.bottom() - 26.0, r.w - 32.0, 20.0), &msg, 12.0, WARN, Align::Left, Weight::Regular);
    }
}

fn left_card(ui: &mut Ui, app: &mut App, s: &mut St, r: Rect) {
    ui.card(r);
    let ix = r.x + 16.0;
    let iw = r.w - 32.0;
    let inner = rect(r.x, r.y + 4.0, r.w, r.h - 8.0);
    ui.begin_scroll("shd-left", inner, s.left_h);
    let mut y = inner.y + 8.0;
    ui.title(rect(ix, y, iw, 22.0), "Passes");
    y += 30.0;
    let bw = ((iw - 16.0) / 3.0).floor();
    if ui.button("shd-test", rect(ix, y, bw, 32.0), "Testar com imagem", false) {
        ui.menu("shd-test-m", &[("Cena 2D (320×240)", true), ("Barras de cor e grade", true), ("Abrir imagem ou captura…", true)]);
    }
    match ui.menu_pick("shd-test-m") {
        Some(0) => preview("scene"),
        Some(1) => preview("bars"),
        Some(_) => pick(app, PICK_IMAGE),
        None => {}
    }
    for (i, (key, label, replace)) in [("shd-add", "Adicionar passe", false), ("shd-load", "Carregar preset", true)].into_iter().enumerate() {
        if ui.button(key, rect(ix + (bw + 8.0) * (i + 1) as f32, y, bw, 32.0), label, false) {
            s.browse = Some(Browse { replace, dir: 0, file: None, q: String::new() });
            s.lib = None;
        }
    }
    y += 42.0;
    let Some(p) = s.profiles.get(s.sel).map(|p| p.1.clone()) else {
        ui.end_scroll();
        return;
    };
    let names: Vec<String> = p.passes.iter().filter(|x| x.on).map(|x| x.path.file_stem().unwrap_or_default().to_string_lossy().into_owned()).collect();
    let flow = format!("{} → {} → {}", crate::i18n::tr("Imagem do jogo"), if names.is_empty() { crate::i18n::tr("nada").into_owned() } else { names.join(" → ") }, crate::i18n::tr("Tela"));
    ui.c.text_raw(rect(ix, y, iw, 20.0), &flow, 13.0, TEXT2, Align::Left, Weight::Regular);
    y += 26.0;
    if p.passes.is_empty() {
        ui.c.text(rect(ix, y, iw, 24.0), "Nenhum passe. Adicione um preset.", 13.0, MUTED, Align::Left, Weight::Regular);
        y += 32.0;
    }
    let (root, lib) = (sp::root(), sp::lib_dir());
    let mut act: Option<(usize, i8)> = None;
    let n = p.passes.len();
    for (i, pass) in p.passes.iter().enumerate() {
        let row = rect(ix - 8.0, y, iw + 16.0, 42.0);
        if i == s.pass {
            ui.c.round(row, 8.0, CARD2);
        } else if ui.is_hover(row) {
            ui.c.round(row, 8.0, LINE);
        }
        let color = if pass.on { TEXT } else { MUTED };
        ui.c.text_raw(rect(ix, y, 22.0, 42.0), &format!("{}", i + 1), 14.0, ACCENT, Align::Left, Weight::Semibold);
        let name = pass.path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let rel = pass.path.strip_prefix(&lib).or_else(|_| pass.path.strip_prefix(&root)).unwrap_or(&pass.path).to_string_lossy().into_owned();
        let np = s.params.iter().filter(|x| x.pass == i).count();
        let tw = iw - 22.0 - 150.0;
        ui.c.text_fit(rect(ix + 22.0, y + 3.0, tw, 20.0), &name, 14.0, color, Align::Left, Weight::Semibold);
        let sub = crate::i18n::tr("{} · {} parâmetros").replacen("{}", &rel, 1).replacen("{}", &np.to_string(), 1);
        ui.c.text_fit(rect(ix + 22.0, y + 22.0, tw, 18.0), &sub, 11.0, MUTED, Align::Left, Weight::Regular);
        let bx = ix + iw - 146.0;
        if ui.button_ex(&format!("shd-up{i}"), rect(bx, y + 6.0, 30.0, 30.0), "▲", false, i > 0) {
            act = Some((i, -1));
        }
        if ui.button_ex(&format!("shd-dn{i}"), rect(bx + 34.0, y + 6.0, 30.0, 30.0), "▼", false, i + 1 < n) {
            act = Some((i, 1));
        }
        let mut on = pass.on;
        let cb = rect(bx + 76.0, y + 6.0, 30.0, 30.0);
        if ui.check(&format!("shd-po{i}"), cb, "", &mut on) {
            act = Some((i, 2));
        }
        ui.tip(cb, "Ligar ou desligar só este passe");
        if ui.button(&format!("shd-x{i}"), rect(bx + 112.0, y + 6.0, 30.0, 30.0), "✕", false) {
            act = Some((i, 3));
        }
        if act.is_none() && ui.hit(&format!("shd-row{i}"), row) {
            s.pass = i;
        }
        y += 46.0;
    }
    let mut pass = s.pass;
    if let (Some((i, op)), Some(cur)) = (act, s.cur()) {
        match op {
            -1 | 1 => {
                let j = (i as isize + op as isize) as usize;
                cur.passes.swap(i, j);
                pass = j;
            }
            2 => cur.passes[i].on = !cur.passes[i].on,
            _ => {
                sp::remove_pass(cur, i);
                pass = pass.min(cur.passes.len().saturating_sub(1));
            }
        }
        s.pass = pass;
        s.changed();
    }
    ui.c.text_wrap(rect(ix, y, iw, 36.0), "O nº 1 roda primeiro, sobre a imagem do jogo; o último fica por cima. Cada linha pode ser um preset inteiro (.slangp) ou um .slang avulso.", 12.0, MUTED, Weight::Regular);
    y += 46.0;

    ui.title(rect(ix, y, iw, 22.0), "Como aplicar");
    y += 32.0;
    let lw = 170.0;
    ui.c.text(rect(ix, y, lw, 32.0), "Modo", 14.0, TEXT, Align::Left, Weight::Regular);
    // `api=` vem do motor (módulos do jogo e primeiro Present): vazio é jogo ainda não aberto, ainda pode.
    let can = !s.anti_cheat && (p.api.is_empty() || prisma_shader::link::api_supported(&p.api));
    let names: &[&str] = if can { &["Por cima", "Injeção"] } else { &["Por cima"] };
    let mut m = usize::from(can && p.inject);
    if ui.segment("shd-mode", ix + lw, y, 32.0, names, &mut m) {
        if let Some(c) = s.cur() {
            c.inject = m == 1;
            c.fallback.clear();
        }
        s.changed();
        s.flush();
        // O motor escolhe o modo ao abrir; a DLL já no jogo para sozinha ao ler mode=overlay.
        restart();
    }
    if !can {
        let inj = rect(ix + lw + Ui::segment_width(names) + 8.0, y, 120.0, 32.0);
        ui.button_ex("shd-inj", inj, "Injeção", false, false);
        ui.tip(inj, if s.anti_cheat { "Bloqueado: este jogo tem anti-cheat" } else { "Não suportado neste jogo: a injeção cobre DX9 a 12, OpenGL e Vulkan" });
    }
    y += 36.0;
    let how = if can && p.inject {
        "Dentro do jogo, ao mostrar o quadro: atraso menor. Não pega em todo jogo (só DX9 a 12, OpenGL e Vulkan) nem com anti-cheat; HDR fica sem efeito."
    } else {
        "Janela por cima do jogo, capturando a imagem: pega em qualquer janela. A Injeção tem atraso menor, mas não pega em todo jogo nem com anti-cheat."
    };
    // Três linhas: o texto da Injeção quebra em três, e mais ainda traduzido.
    ui.c.text_wrap(rect(ix + lw, y, iw - lw, 50.0), how, 12.0, MUTED, Weight::Regular);
    y += 56.0;
    // A Injeção não vem no exe: sem os arquivos desta versão o motor fica Por cima, e aqui se baixa.
    if can && p.inject && !crate::shader::files::ready() {
        let (on, got, total, err, why) = {
            let d = INJ.lock().unwrap_or_else(|e| e.into_inner());
            (d.on, d.got, d.total, d.err, d.why)
        };
        let bw = 200.0;
        let text = if on {
            crate::i18n::tr("Baixando o modo Injeção: {} MB").replace("{}", &format!("{:.1}", got as f32 / 1048576.0))
        } else if err {
            crate::i18n::tr(why).into_owned()
        } else {
            crate::i18n::tr("O modo Injeção não está baixado: este jogo usa Por cima até baixar.").into_owned()
        };
        ui.c.text_wrap(rect(ix, y, iw - bw - 12.0, 36.0), &text, 12.0, if on { MUTED } else { WARN }, Weight::Regular);
        if ui.button_ex("shd-inj-dl", rect(ix + iw - bw, y, bw, 32.0), "Baixar o modo Injeção", true, !on) {
            download_inject();
        }
        if on {
            let bar = rect(ix, y + 38.0, iw, 4.0);
            ui.c.round(bar, 2.0, BORDER);
            let f = if total > 0 { got as f32 / total as f32 } else { 0.5 };
            ui.c.round(rect(bar.x, bar.y, bar.w * f.min(1.0), 4.0), 2.0, ACCENT);
        }
        y += 48.0;
    }

    ui.c.text(rect(ix, y, lw, 36.0), "Resolução de entrada", 14.0, TEXT, Align::Left, Weight::Regular);
    let mut opts: Vec<String> = INPUTS.iter().map(|o| crate::i18n::tr(o.1).into_owned()).collect();
    // Tamanho exato (`LxA`, do Fullscreenizer) aparece como mais uma opção, a última.
    if !INPUTS.iter().any(|o| o.0 == p.input) {
        opts.push(p.input.replace('x', " × "));
    }
    let mut i = INPUTS.iter().position(|o| o.0 == p.input).unwrap_or(INPUTS.len());
    if ui.select("shd-input", rect(ix + lw, y, 200.0, 36.0), &opts, &mut i) && i < INPUTS.len() {
        if let Some(c) = s.cur() {
            c.input = INPUTS[i].0.to_string();
        }
        s.changed();
    }
    y += 40.0;
    ui.c.text_wrap(rect(ix + lw, y, iw - lw, 34.0), "Automática acha o tamanho do pixel do jogo e entrega ao filtro a resolução original.", 12.0, MUTED, Weight::Regular);
    y += 40.0;

    let mut rec = p.record;
    if ui.check("shd-rec", rect(ix, y, iw, 28.0), "Gravar e transmitir com o shader", &mut rec) {
        if let Some(c) = s.cur() {
            c.record = rec;
        }
        s.changed();
    }
    y += 28.0;
    ui.c.text(rect(ix + 28.0, y, iw - 28.0, 18.0), "Captura e Compartilhar tela usam a imagem já filtrada", 12.0, MUTED, Align::Left, Weight::Regular);
    y += 28.0;

    // Avisos.
    let exe = p.exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let warn = |ui: &mut Ui, y: &mut f32, text: &str, button: Option<&str>| -> bool {
        let tw = if button.is_some() { iw - 182.0 } else { iw };
        // Até três linhas: em inglês o aviso de tela cheia ao lado do botão quebra em três.
        ui.c.text_wrap(rect(ix, *y, tw, 50.0), text, 12.0, WARN, Weight::Regular);
        let hit = button.is_some_and(|b| ui.button("shd-fs", rect(ix + iw - 170.0, *y + 9.0, 170.0, 32.0), b, false));
        *y += 56.0;
        hit
    };
    if s.anti_cheat {
        warn(ui, &mut y, "Anti-cheat detectado: o Prisma não aplica shader neste jogo, nem por cima.", None);
    }
    if !exe.is_empty() && !app.cfg.fs_auto(&exe) && warn(ui, &mut y, "Em tela cheia exclusiva o filtro some. Use janela sem borda ou ligue o Fullscreenizer para este jogo.", Some("Ligar Fullscreenizer")) {
        app.cfg.fs_note(&exe, &p.exe.to_string_lossy());
        if let Some(f) = app.cfg.fs_apps.iter_mut().find(|f| f.exe.eq_ignore_ascii_case(&exe)) {
            f.auto = true;
        }
        app.cfg.save();
    }
    if !s.missing.is_empty() {
        warn(ui, &mut y, &crate::i18n::tr("Faltam arquivos do preset importado (baixe a biblioteca ou copie para ela): {}").replace("{}", &s.missing.join(", ")), None);
    }
    // O motor tirou a Injeção deste jogo: o motivo, até o jogador escolher o modo de novo.
    let why = match p.fallback.as_str() {
        "api" => Some(crate::i18n::tr("A Injeção não cobre a API deste jogo ({}); voltou para Por cima.").replace("{}", &p.api)),
        "dll" => Some(crate::i18n::tr("A DLL da Injeção não entrou no jogo; voltou para Por cima.").into_owned()),
        "frames" => Some(crate::i18n::tr("Nenhum quadro do jogo passou pela Injeção; voltou para Por cima.").into_owned()),
        "crash" => Some(crate::i18n::tr("O jogo fechou com erro durante a Injeção; voltou para Por cima.").into_owned()),
        _ => None,
    };
    if let Some(w) = why.filter(|_| !p.inject) {
        warn(ui, &mut y, &w, None);
    }
    if s.hdr {
        warn(ui, &mut y, "Monitor em HDR: a imagem é convertida para SDR antes do filtro (brilho acima do branco é cortado).", None);
    }

    y += 4.0;
    let bw = 150.0;
    if ui.button("shd-save", rect(ix + iw - bw, y, bw, 36.0), "Salvar perfil", true) {
        s.flush();
        s.saved = s.cur().cloned();
        app.toast.info("Perfil salvo para este jogo");
    }
    if ui.button("shd-export", rect(ix + iw - 2.0 * bw - 8.0, y, bw, 36.0), "Salvar como preset", false) {
        s.flush();
        export(app, s);
    }
    let dirty = s.saved.as_ref() != s.profiles.get(s.sel).map(|p| &p.1);
    if ui.button_ex("shd-undo", rect(ix + iw - 3.0 * bw - 16.0, y, bw, 36.0), "Descartar", false, dirty) {
        if let (Some(old), Some(cur)) = (s.saved.clone(), s.cur()) {
            *cur = old;
            s.changed();
            s.flush();
            app.toast.info("Voltou ao último salvo");
        }
    }
    y += 48.0;
    s.left_h = y - inner.y;
    ui.end_scroll();
}

fn params_card(ui: &mut Ui, s: &mut St, r: Rect) {
    ui.card(r);
    let ix = r.x + 16.0;
    let iw = r.w - 32.0;
    let pass = s.pass;
    let title = s.profiles.get(s.sel).and_then(|p| p.1.passes.get(pass)).map(|x| x.path.file_stem().unwrap_or_default().to_string_lossy().into_owned());
    match &title {
        Some(t) => ui.c.text_raw(rect(ix, r.y + 12.0, iw - 170.0, 22.0), t, 15.0, TEXT, Align::Left, Weight::Semibold),
        None => ui.title(rect(ix, r.y + 12.0, iw - 170.0, 22.0), "Parâmetros"),
    }
    let mine: Vec<Param> = s.params.iter().filter(|x| x.pass == pass).cloned().collect();
    if ui.button_ex("shd-reset", rect(r.right() - 16.0 - 160.0, r.y + 8.0, 160.0, 32.0), "Restaurar padrão", false, !mine.is_empty()) {
        if let Some(c) = s.cur() {
            for m in &mine {
                c.set_param(&m.name, None);
            }
        }
        s.changed();
    }
    ui.search("shd-q", rect(ix, r.y + 48.0, iw, 34.0), &mut s.q, "Buscar parâmetro (ex.: mask, scan, gamma)");
    let mut only = s.only_changed;
    if ui.check("shd-only", rect(ix, r.y + 88.0, iw, 24.0), "Só os alterados", &mut only) {
        s.only_changed = only;
    }
    let Some(cur) = s.profiles.get(s.sel).map(|p| p.1.clone()) else { return };
    let q = s.q.to_lowercase();
    let rows: Vec<&Param> = mine
        .iter()
        .filter(|m| q.is_empty() || m.name.to_lowercase().contains(&q) || m.desc.to_lowercase().contains(&q))
        .filter(|m| !s.only_changed || cur.param(&m.name).is_some())
        .collect();
    let list = rect(ix - 4.0, r.y + 120.0, iw + 8.0, r.h - 120.0 - 44.0);
    const ROW: f32 = 64.0;
    if rows.is_empty() {
        let msg = if title.is_none() {
            "Nenhum passe."
        } else if mine.is_empty() {
            "Este passe não tem parâmetros."
        } else {
            "Nada encontrado."
        };
        ui.c.text(rect(ix, list.y, iw, 24.0), msg, 13.0, MUTED, Align::Left, Weight::Regular);
    }
    ui.begin_scroll("shd-params", list, rows.len() as f32 * ROW);
    let first = (ui.scroll_y() / ROW) as usize;
    let last = first + (list.h / ROW) as usize + 2;
    let mut set: Option<(String, Option<f32>)> = None;
    for (i, m) in rows.iter().enumerate().take(last).skip(first) {
        let y = list.y + i as f32 * ROW;
        let row = rect(list.x, y, list.w, ROW - 4.0);
        let v = cur.param(&m.name).unwrap_or(m.default);
        let changed = cur.param(&m.name).is_some();
        let x = list.x + 4.0;
        let w = list.w - 8.0;
        ui.c.text_fit(rect(x, y + 2.0, w - 80.0, 20.0), &m.desc, 13.0, TEXT, Align::Left, Weight::Regular);
        ui.c.text_raw(rect(x + w - 80.0, y + 2.0, 80.0, 20.0), &fmt(v, m.step), 13.0, if changed { WARN } else { TEXT2 }, Align::Right, Weight::Semibold);
        let span = m.max - m.min;
        let mut t = if span > 0.0 { (v - m.min) / span } else { 0.0 };
        if ui.slider(&format!("shd-p-{}", m.name), rect(x + 6.0, y + 22.0, w - 12.0, 20.0), &mut t) {
            let nv = snap(m, m.min + t * span);
            set = Some((m.name.clone(), ((nv - m.default).abs() > m.step.abs() * 0.01 + f32::EPSILON).then_some(nv)));
        }
        let range = format!("{}   {} · {} {}   {}", fmt(m.min, m.step), m.name, crate::i18n::tr("padrão"), fmt(m.default, m.step), fmt(m.max, m.step));
        ui.c.text_raw(rect(x, y + 42.0, w, 16.0), &range, 11.0, MUTED, Align::Center, Weight::Regular);
        // Botão direito volta o valor ao padrão.
        if ui.right_hit(row) && changed {
            set = Some((m.name.clone(), None));
        }
    }
    ui.end_scroll();
    if let Some((name, v)) = set {
        if let Some(c) = s.cur() {
            c.set_param(&name, v);
        }
        s.changed();
    }
    ui.c.text_wrap(rect(ix, r.bottom() - 40.0, iw, 34.0), "Mínimo, máximo e passo vêm do próprio shader. A mudança aparece no jogo na hora; laranja é diferente do padrão. Botão direito volta o valor.", 11.0, MUTED, Weight::Regular);
}

/// Navegador da biblioteca: pastas à esquerda, presets à direita, busca em tudo.
fn browse_sheet(ui: &mut Ui, app: &mut App, s: &mut St) {
    let lib = sp::lib_dir();
    let mut all = s.lib.get_or_insert_with(|| scan(&lib)).clone();
    all.extend(mine());
    let Some(b) = s.browse.as_mut() else { return };
    let r = ui.sheet(760.0, 540.0);
    let ix = r.x + 20.0;
    let iw = r.w - 40.0;
    let title = if b.replace { "Carregar preset (substitui os passes)" } else { "Adicionar passe" };
    ui.c.text(rect(ix, r.y + 16.0, iw - 40.0, 26.0), title, 17.0, TEXT, Align::Left, Weight::Semibold);
    let mut close = false;
    let hint = crate::i18n::tr("Buscar em {} presets (ex.: lottes, ntsc, vhs)").replace("{}", &all.len().to_string());
    ui.search("shd-bq", rect(ix, r.y + 52.0, iw, 34.0), &mut b.q, &hint);
    let mut dirs: Vec<(String, usize)> = Vec::new();
    for f in &all {
        let d = top_dir(f);
        match dirs.last_mut() {
            Some(l) if l.0 == d => l.1 += 1,
            _ => dirs.push((d.to_string(), 1)),
        }
    }
    dirs.sort_by_key(|d| d.0.to_lowercase());
    dirs.dedup_by(|a, b| {
        let same = a.0 == b.0;
        if same {
            b.1 += a.1;
        }
        same
    });
    let body = rect(ix, r.y + 96.0, iw, r.h - 96.0 - 64.0);
    if all.is_empty() {
        ui.c.text_wrap(rect(body.x, body.y, body.w, 40.0), "A biblioteca está vazia. Baixe no cartão Biblioteca ou importe um preset.", 13.0, MUTED, Weight::Regular);
    }
    let dw = 200.0;
    let dl = rect(body.x, body.y, dw, body.h);
    let searching = !b.q.trim().is_empty();
    ui.begin_scroll("shd-bdirs", dl, dirs.len() as f32 * 30.0);
    for (i, (d, n)) in dirs.iter().enumerate() {
        let row = rect(dl.x, dl.y + i as f32 * 30.0, dl.w, 28.0);
        let on = i == b.dir && !searching;
        if on {
            ui.c.round(row, 6.0, CARD);
        }
        let label = match d.as_str() {
            "" => ".",
            ".." => "Meus presets",
            d => d,
        };
        ui.c.text_fit(rect(row.x + 10.0, row.y, row.w - 50.0, 28.0), label, 13.0, if on { TEXT } else { TEXT2 }, Align::Left, Weight::Regular);
        ui.c.text_raw(rect(row.right() - 44.0, row.y, 36.0, 28.0), &n.to_string(), 12.0, MUTED, Align::Right, Weight::Regular);
        if ui.hit(&format!("shd-bd{i}"), row) {
            b.dir = i;
            b.q.clear();
        }
    }
    ui.end_scroll();
    let q = b.q.to_lowercase();
    let cur = dirs.get(b.dir).map(|d| d.0.clone()).unwrap_or_default();
    let files: Vec<&String> = all.iter().filter(|f| if searching { f.to_lowercase().contains(q.trim()) } else { top_dir(f) == cur }).collect();
    let fl = rect(body.x + dw + 12.0, body.y, body.w - dw - 12.0, body.h);
    ui.begin_scroll("shd-bfiles", fl, files.len() as f32 * 30.0);
    let first = (ui.scroll_y() / 30.0) as usize;
    let last = first + (fl.h / 30.0) as usize + 2;
    let mut add = None;
    for (i, f) in files.iter().enumerate().take(last).skip(first) {
        let row = rect(fl.x, fl.y + i as f32 * 30.0, fl.w, 28.0);
        let on = b.file.as_deref() == Some(f.as_str());
        if on {
            ui.c.round(row, 6.0, CARD);
        } else if ui.is_hover(row) {
            ui.c.round(row, 6.0, LINE);
        }
        // Na pasta, só o resto do caminho; na busca, o caminho todo.
        let shown = if searching { f.as_str() } else { f.split_once('\\').map_or(f.as_str(), |x| x.1) };
        ui.c.text_fit(rect(row.x + 10.0, row.y, row.w - 20.0, 28.0), shown, 13.0, if on { TEXT } else { TEXT2 }, Align::Left, Weight::Regular);
        // Segundo clique no escolhido adiciona, como um clique duplo.
        if ui.hit(&format!("shd-bf{i}"), row) {
            if on {
                add = Some((*f).clone());
            }
            b.file = Some((*f).clone());
        }
    }
    ui.end_scroll();
    let fy = r.bottom() - 52.0;
    if b.replace {
        // Um .slangp de qualquer lugar, do RetroArch inclusive: vai para Meus presets com os caminhos na biblioteca.
        if ui.button("shd-bimport", rect(ix, fy, 170.0, 36.0), "Importar preset…", false) {
            pick(app, PICK_IMPORT);
            close = true;
        }
    } else {
        ui.c.text_wrap(rect(ix, fy, iw - 330.0, 36.0), "Preset (.slangp) entra como um passe com todas as passadas dele; .slang entra como uma passada só.", 11.0, MUTED, Weight::Regular);
    }
    if ui.button("shd-bcancel", rect(r.right() - 320.0, fy, 130.0, 36.0), "Cancelar", false) {
        close = true;
    }
    let label = if b.replace { "Carregar" } else { "Adicionar ao fim" };
    if ui.button_ex("shd-bok", rect(r.right() - 180.0, fy, 160.0, 36.0), label, true, b.file.is_some()) {
        add = b.file.clone();
    }
    let replace = b.replace;
    if let Some(f) = add {
        if let Some(c) = s.cur() {
            if replace {
                c.passes.clear();
                c.params.clear();
            }
            sp::add_pass(c, f.strip_prefix("..\\").map_or_else(|| lib.join(&f), |m| sp::root().join(m)));
            let n = c.passes.len();
            s.pass = n - 1;
            s.changed();
            s.flush();
        }
        close = true;
    }
    if close {
        s.browse = None;
        ui.s.needs_redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(min: f32, max: f32, step: f32) -> Param {
        Param { name: "x".into(), desc: String::new(), default: 0.0, min, max, step, pass: 0 }
    }

    #[test]
    fn valor_no_passo_e_nos_limites() {
        assert_eq!(snap(&p(-20.0, 0.0, 1.0), -13.6), -14.0);
        assert_eq!(snap(&p(0.0, 2.0, 0.1), 5.0), 2.0);
        assert!((snap(&p(0.0, 0.125, 0.01), 0.0312) - 0.03).abs() < 1e-6);
        assert_eq!(decimals(0.05), 2);
        assert_eq!(decimals(1.0), 0);
    }

    #[test]
    fn biblioteca_lista_presets_e_pula_as_passadas() {
        let d = std::env::temp_dir().join(format!("prisma-shd-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for f in ["crt\\crt-lottes.slangp", "crt\\shaders\\lottes.slang", "meus\\a.slang", "leia.txt"] {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        assert_eq!(scan(&d), vec!["crt\\crt-lottes.slangp".to_string(), "meus\\a.slang".to_string()]);
        assert_eq!(top_dir("crt\\crt-lottes.slangp"), "crt");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn so_o_perfil_principal_inicia_o_motor() {
        let open: std::collections::HashSet<String> = ["game.exe".to_string()].into();
        let p = Profile { exe: PathBuf::from(r"C:\Jogos\Game.exe"), ..Profile::default() };
        assert!(starts(&sp::path_for(&p.exe), &p, &open));
        assert!(!starts(&sp::profiles_dir().join("game-crt.txt"), &p, &open));
        assert!(!starts(&sp::path_for(&p.exe), &Profile { on: false, ..p.clone() }, &open));
        assert!(!starts(&sp::path_for(&p.exe), &p, &Default::default()));
    }

    /// Download de verdade (uns 50 MB): `PRISMA_SHD_LIB=<pasta> cargo test baixa -- --ignored`.
    #[test]
    #[ignore]
    fn baixa_biblioteca() {
        let lib = PathBuf::from(std::env::var("PRISMA_SHD_LIB").expect("PRISMA_SHD_LIB"));
        assert!(fetch(&lib).is_some());
        assert!(lib.join("crt").is_dir());
        assert!(scan(&lib).iter().any(|f| f.ends_with("crt-lottes.slangp")));
    }
}
