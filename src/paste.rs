//! Janela de cópia do Prisma. Abre de dois jeitos, sempre num prisma.exe à parte, ao lado do Prisma aberto:
//! `--paste=<pasta>` (Explorer: cola o que está na área de transferência) e `--copy-file=<lista>`
//! (página Cópias ou agenda: a lista vem num arquivo temporário). O motor fica em `sys::copy`;
//! ao terminar, a cópia vai para o histórico (`sys::copylist`).

use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_ESCAPE, VK_SPACE};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::i18n::tr;
use crate::sys::copy::{self, State};
use crate::sys::copylist::{self, Record};
use crate::sys::monitor::{self, CpuTimes, DiskBusy};
use crate::ui::gfx::{measure_raw, rect, Align, Canvas, Rect, Weight, ACCENT, BG, BORDER, BORDER2, CARD, CARD2, LINE, MUTED, ON_ACCENT, TEXT, TEXT2, WARN};
use crate::ui::layered::Layered;

const W: f32 = 560.0;
const PAD: f32 = 20.0;
const MAX_ERRORS: usize = 5;
/// Pontos do gráfico de velocidade, um a cada meio segundo; os mais velhos saem.
const SAMPLES: usize = 120;
/// Onde começa a lista de erros.
const BODY: f32 = 384.0;

#[derive(Clone, Copy, PartialEq)]
enum Pick {
    Policy(u8),
    Verify,
    CloseDone,
    Pause,
    Retry,
    Close,
}

/// O que a janela mostra, copiado do estado do trabalho a cada quadro.
struct View {
    dest: String,
    dest_path: String,
    from: String,
    mv: bool,
    phase: u8,
    current: String,
    done: u64,
    total: u64,
    files_done: u32,
    files: u32,
    speed: f64,
    samples: Vec<f32>,
    asking: bool,
    policy: u8,
    verify: bool,
    close_done: bool,
    paused: bool,
    need: u64,
    free: u64,
    errors: Vec<(String, String)>,
    can_retry: bool,
    /// Bytes lidos na conferência, de `check_total`.
    checked: u64,
    check_total: u64,
    /// Segundos desde o começo, parado ao terminar.
    elapsed: u64,
    cpu: Option<f32>,
    /// (rótulo, letra, % de tempo ativo): um disco quando origem e destino são o mesmo.
    disks: Vec<(&'static str, String, f32)>,
}

/// Como a janela abre: o que já vem escolhido e se pode tirar o foco de quem está usando o PC.
pub struct Opts {
    pub policy: u8,
    pub verify: bool,
    /// Nome da lista de cópia, vazio numa cópia avulsa.
    pub name: String,
    /// Cópia agendada: aparece sem roubar o foco.
    pub quiet: bool,
}

struct Win {
    layer: Layered,
    hits: Vec<(Rect, Pick)>,
    hover: Option<usize>,
    pos: (i32, i32),
    st: Arc<State>,
    items: Vec<PathBuf>,
    dest_path: PathBuf,
    dest: String,
    from: String,
    name: String,
    mv: bool,
    close_done: bool,
    speed: f64,
    samples: Vec<f32>,
    last: (Instant, u64),
    worker: Option<std::thread::JoinHandle<()>>,
    done_at: Option<Instant>,
    started: Instant,
    /// Fase vista no quadro anterior: ao passar para a conferência, a velocidade recomeça.
    seen: u8,
    cpu_prev: Vec<CpuTimes>,
    cpu_threads: Vec<f32>,
    cpu: Option<f32>,
    disk: Option<DiskBusy>,
    /// Letras do disco de origem e de destino.
    drives: (Option<String>, Option<String>),
    disks: Vec<(&'static str, String, f32)>,
    meter_at: Instant,
}

thread_local! {
    static WIN: std::cell::RefCell<Option<Win>> = const { std::cell::RefCell::new(None) };
}

/// `--paste=<pasta>` ou `--copy-file=<lista>`: true quando o argumento veio e esta cópia já fez o seu trabalho.
pub fn run_args() -> bool {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::args().find_map(|a| a.strip_prefix("--test-paste=").map(str::to_string)) {
        test(&dir);
        return true;
    }
    if let Some(spec) = std::env::args().find_map(|a| a.strip_prefix("--copy-file=").map(str::to_string)) {
        if let Some(job) = copylist::read_spec(std::path::Path::new(spec.trim_matches('"'))) {
            let quiet = std::env::args().any(|a| a == "--quiet");
            open(job.items, job.dest, job.mv, Opts { policy: job.policy, verify: job.verify, name: job.name, quiet });
        }
        return true;
    }
    let Some(dest) = std::env::args().find_map(|a| a.strip_prefix("--paste=").map(str::to_string)) else { return false };
    let dest = PathBuf::from(dest.trim_matches('"'));
    let Some((items, cut)) = copy::clipboard() else {
        let msg = HSTRING::from(tr("Não há arquivos copiados para colar.").as_ref());
        unsafe { MessageBoxW(None, &msg, w!("Prisma"), MB_OK | MB_ICONINFORMATION) };
        return true;
    };
    open(items, dest, cut, Opts { policy: copy::ASK, verify: false, name: String::new(), quiet: false });
    true
}

pub fn size_text(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64).replace('.', crate::i18n::comma())
    } else if b >= 1 << 20 {
        format!("{} MB", b >> 20)
    } else {
        format!("{} KB", b.div_ceil(1024))
    }
}

fn height(v: &View) -> f32 {
    let errs = v.errors.len().min(MAX_ERRORS) as f32 * 20.0 + if v.errors.len() > MAX_ERRORS { 20.0 } else { 0.0 };
    BODY + if v.errors.is_empty() { 0.0 } else { 26.0 + errs } + 66.0
}

fn title(v: &View) -> String {
    match v.phase {
        copy::PREPARING => tr("Preparando a lista de arquivos…").into_owned(),
        copy::NO_SPACE => tr("Falta espaço no destino").into_owned(),
        copy::CANCELED => tr("Cancelado").into_owned(),
        copy::DONE if v.errors.is_empty() && v.check_total > 0 => tr("Pronto, tudo conferido").into_owned(),
        copy::DONE if v.errors.is_empty() => tr("Pronto").into_owned(),
        copy::DONE => tr(&format!("Pronto, com {} erros", v.errors.len())).into_owned(),
        _ if v.paused => tr("Pausado").into_owned(),
        copy::VERIFYING => tr("Conferindo as cópias…").into_owned(),
        _ if v.mv => tr(&format!("Movendo para {}", v.dest)).into_owned(),
        _ => tr(&format!("Copiando para {}", v.dest)).into_owned(),
    }
}

/// O primeiro item com o caminho, os outros só pelo nome.
fn from_text(items: &[PathBuf]) -> String {
    let mut s = items.first().map(|p| p.display().to_string()).unwrap_or_default();
    for p in items.iter().skip(1) {
        s.push_str(", ");
        s.push_str(&p.file_name().map_or(p.display().to_string(), |n| n.to_string_lossy().into_owned()));
    }
    s
}

/// 0:42, 12:05 ou 1:02:03.
fn clock(s: u64) -> String {
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// O que a barra mede: a conferência na fase dela, a cópia no resto.
fn progress(v: &View) -> (u64, u64) {
    if v.phase == copy::VERIFYING {
        (v.checked, v.check_total)
    } else {
        (v.done, v.total)
    }
}

fn check(c: &Canvas, b: Rect, on: bool) {
    if on {
        c.round(b, 4.0, ACCENT);
        c.line(b.x + 3.5, b.y + 8.5, b.x + 6.5, b.y + 11.5, ON_ACCENT, 2.0);
        c.line(b.x + 6.5, b.y + 11.5, b.x + 12.5, b.y + 4.5, ON_ACCENT, 2.0);
    } else {
        c.round_line(b, 4.0, BORDER2, 1.5);
    }
}

/// Desenha a janela e devolve as áreas que se clicam.
fn draw(c: &Canvas, r: Rect, v: &View, hover: Option<usize>) -> Vec<(Rect, Pick)> {
    let mut hits = Vec::new();
    c.round(r, 12.0, CARD);
    c.round_line(r, 12.0, BORDER, 1.0);
    let x = r.x + PAD;
    let w = r.w - 2.0 * PAD;
    let (cur, all) = progress(v);
    let frac = if all == 0 { if v.phase == copy::DONE { 1.0 } else { 0.0 } } else { (cur as f64 / all as f64).min(1.0) as f32 };
    let pct = format!("{:.1}%", frac * 100.0).replace('.', crate::i18n::comma());
    c.text_raw(rect(x + w - 110.0, 10.0, 110.0, 32.0), &pct, 24.0, ACCENT, Align::Right, Weight::Semibold);
    c.text_raw(rect(x, 16.0, w - 120.0, 24.0), &title(v), 15.0, if v.phase == copy::NO_SPACE { WARN } else { TEXT }, Align::Left, Weight::Semibold);

    // De / Para: a coluna dos rótulos mede no idioma, para "Откуда" caber.
    let lw = measure_raw(&tr("De"), 12.0, Weight::Regular).max(measure_raw(&tr("Para"), 12.0, Weight::Regular)) + 10.0;
    for (i, (label, path)) in [("De", &v.from), ("Para", &v.dest_path)].into_iter().enumerate() {
        let y = 48.0 + i as f32 * 18.0;
        c.text(rect(x, y, lw, 18.0), label, 12.0, MUTED, Align::Left, Weight::Regular);
        c.text_raw(rect(x + lw, y, w - lw, 18.0), path, 12.0, TEXT2, Align::Left, Weight::Regular);
    }

    let bar = rect(x, 92.0, w, 8.0);
    c.round(bar, 4.0, LINE);
    if frac > 0.0 {
        c.round(rect(bar.x, bar.y, (bar.w * frac).max(8.0), bar.h), 4.0, if v.paused { MUTED } else { ACCENT });
    }

    let running = matches!(v.phase, copy::PREPARING | copy::COPYING | copy::VERIFYING);
    let live = matches!(v.phase, copy::COPYING | copy::VERIFYING) && !v.paused && v.speed > 1.0;
    let eta = if live { clock((all.saturating_sub(cur) as f64 / v.speed) as u64) } else { "—".into() };
    let speed = if live { format!("{}/s", size_text(v.speed as u64)) } else { "—".into() };
    let stats = [
        ("Copiado", format!("{} / {}", size_text(v.done), size_text(v.total))),
        ("Arquivos", format!("{} / {}", v.files_done, v.files)),
        ("Velocidade", speed),
        ("Tempo restante", eta),
    ];
    let cw = (w - 3.0 * 8.0) / 4.0;
    for (i, (label, value)) in stats.iter().enumerate() {
        let cx = x + i as f32 * (cw + 8.0);
        c.text_fit(rect(cx, 108.0, cw, 16.0), label, 11.0, MUTED, Align::Left, Weight::Regular);
        c.text_raw(rect(cx, 124.0, cw, 20.0), value, 14.0, TEXT, Align::Left, Weight::Semibold);
    }
    // CPU e discos como no Gerenciador de Tarefas; o tempo decorrido fica embaixo do restante.
    let mut meters: Vec<(usize, &str, String)> = vec![(0, "CPU", v.cpu.map_or("—".into(), |p| format!("{p:.0}%")))];
    for (i, (label, letter, p)) in v.disks.iter().enumerate() {
        meters.push((1 + i, *label, format!("{letter} {p:.0}%")));
    }
    meters.push((3, "Tempo decorrido", clock(v.elapsed)));
    for (i, label, value) in &meters {
        let cx = x + *i as f32 * (cw + 8.0);
        c.text_fit(rect(cx, 148.0, cw, 16.0), label, 11.0, MUTED, Align::Left, Weight::Regular);
        c.text_raw(rect(cx, 164.0, cw, 20.0), value, 14.0, TEXT, Align::Left, Weight::Semibold);
    }

    // Gráfico de velocidade: o pico manda na altura.
    let g = rect(x, 192.0, w, 56.0);
    c.round(g, 8.0, BG.alpha(0.55));
    let peak = v.samples.iter().cloned().fold(0.0f32, f32::max);
    if v.samples.len() >= 2 && peak > 0.0 {
        let inner = g.inset_xy(6.0, 6.0);
        let dx = inner.w / (SAMPLES - 1) as f32;
        let top = peak * 1.15;
        for (i, s) in v.samples.iter().enumerate() {
            let h = s / top * inner.h;
            c.fill(rect(inner.x + i as f32 * dx, inner.bottom() - h, dx + 0.5, h), ACCENT.alpha(0.16));
        }
        let vals: Vec<f32> = v.samples.iter().map(|s| s / top * 100.0).collect();
        c.sparkline(rect(inner.x, inner.y, dx * (vals.len() - 1) as f32, inner.h), &vals, ACCENT, 1.5);
        c.text_raw(rect(g.right() - 166.0, g.y + 4.0, 158.0, 16.0), &tr(&format!("Pico: {}/s", size_text(peak as u64))), 11.0, MUTED, Align::Right, Weight::Regular);
    }

    let sub = if v.phase == copy::NO_SPACE {
        tr(&format!("Precisa de {} e há {} livres. Nada foi copiado.", size_text(v.need), size_text(v.free))).into_owned()
    } else {
        v.current.clone()
    };
    c.text_raw(rect(x, 256.0, w, 18.0), &sub, 12.0, if v.phase == copy::NO_SPACE { WARN } else { TEXT2 }, Align::Left, Weight::Regular);

    let hovered = |hits: &Vec<(Rect, Pick)>| hover == Some(hits.len());
    // Arquivo que já existe: uma escolha para o lote todo.
    let (label, col) = if v.asking { ("Já existe um arquivo com esse nome. Escolha para todos:", WARN) } else { ("Arquivos que já existem no destino", TEXT2) };
    c.text_fit(rect(x, 282.0, w, 20.0), label, 13.0, col, Align::Left, Weight::Regular);
    let gap = 6.0;
    let sw = ((w - 2.0 * gap) / 3.0).floor();
    for (i, (t, p)) in [("Pular", copy::SKIP), ("Substituir", copy::REPLACE), ("Manter os dois", copy::KEEP_BOTH)].into_iter().enumerate() {
        let s = rect(x + i as f32 * (sw + gap), 304.0, sw, 30.0);
        let on = v.policy == p;
        let (bg, fg) = if on { (ACCENT, ON_ACCENT) } else if hovered(&hits) { (CARD2, TEXT) } else { (LINE.alpha(0.6), TEXT2) };
        c.round(s, 7.0, bg);
        if v.asking && !on {
            c.round_line(s, 7.0, WARN.alpha(0.7), 1.0);
        }
        c.text_fit(s.inset_xy(6.0, 0.0), t, 13.0, fg, Align::Center, if on { Weight::Semibold } else { Weight::Regular });
        hits.push((s, Pick::Policy(p)));
    }

    // Conferir e fechar sozinha ao terminar, lado a lado.
    let half = (r.w - 16.0) / 2.0;
    for (i, (t, on, p)) in [("Conferir com hash (SHA-256)", v.verify, Pick::Verify), ("Fechar ao terminar", v.close_done, Pick::CloseDone)].into_iter().enumerate() {
        let row = rect(r.x + 8.0 + i as f32 * half, 344.0, half, 30.0);
        if hovered(&hits) {
            c.round(row, 8.0, CARD2);
        }
        check(c, rect(row.x + 12.0, row.y + 7.0, 16.0, 16.0), on);
        c.text_fit(rect(row.x + 38.0, row.y, row.w - 44.0, row.h), t, 13.0, TEXT, Align::Left, Weight::Regular);
        hits.push((row, p));
    }

    let mut y = BODY;
    if !v.errors.is_empty() {
        c.line(x, y, x + w, y, LINE, 1.0);
        let head = if v.errors.len() == 1 { tr("1 erro").into_owned() } else { tr(&format!("{} erros", v.errors.len())).into_owned() };
        c.text_raw(rect(x, y + 6.0, w, 20.0), &head, 13.0, WARN, Align::Left, Weight::Semibold);
        y += 26.0;
        for (path, why) in v.errors.iter().take(MAX_ERRORS) {
            let name = std::path::Path::new(path).file_name().map_or(path.clone(), |n| n.to_string_lossy().into_owned());
            c.text_raw(rect(x, y, w * 0.55 - 8.0, 20.0), &name, 12.0, TEXT2, Align::Left, Weight::Regular);
            c.text_fit(rect(x + w * 0.55, y, w * 0.45, 20.0), why, 12.0, MUTED, Align::Right, Weight::Regular);
            y += 20.0;
        }
        if v.errors.len() > MAX_ERRORS {
            c.text_raw(rect(x, y, w, 20.0), &tr(&format!("e mais {}", v.errors.len() - MAX_ERRORS)), 12.0, MUTED, Align::Left, Weight::Regular);
            y += 20.0;
        }
    }

    let by = y + 16.0;
    let bw = 150.0;
    let button = |hits: &mut Vec<(Rect, Pick)>, s: Rect, t: &str, primary: bool, p: Pick| {
        let hov = hover == Some(hits.len());
        c.round(s, 8.0, if primary { if hov { ACCENT.alpha(0.85) } else { ACCENT } } else if hov { CARD2 } else { LINE });
        c.text_fit(s.inset_xy(8.0, 0.0), t, 13.0, if primary { ON_ACCENT } else { TEXT }, Align::Center, Weight::Semibold);
        hits.push((s, p));
    };
    let left = rect(r.right() - PAD - 2.0 * bw - 10.0, by, bw, 34.0);
    let right = rect(r.right() - PAD - bw, by, bw, 34.0);
    if running {
        button(&mut hits, left, if v.paused { "Continuar" } else { "Pausar" }, true, Pick::Pause);
        button(&mut hits, right, "Cancelar", false, Pick::Close);
    } else {
        if v.can_retry {
            button(&mut hits, left, "Tentar de novo", false, Pick::Retry);
        }
        button(&mut hits, right, "Fechar", true, Pick::Close);
    }
    hits
}

fn view(o: &Win) -> View {
    let st = &o.st;
    let phase = st.phase.load(Relaxed);
    let errors = st.errors.lock().unwrap().clone();
    View {
        dest: o.dest.clone(),
        dest_path: o.dest_path.display().to_string(),
        from: o.from.clone(),
        mv: o.mv,
        phase,
        current: st.current.lock().unwrap().clone(),
        done: st.done.load(Relaxed),
        total: st.total.load(Relaxed),
        files_done: st.files_done.load(Relaxed),
        files: st.files.load(Relaxed),
        speed: o.speed,
        samples: o.samples.clone(),
        asking: st.asking.load(Relaxed),
        policy: st.policy.load(Relaxed),
        verify: st.verify.load(Relaxed),
        close_done: o.close_done,
        paused: st.paused.load(Relaxed),
        need: st.need.load(Relaxed),
        free: st.free.load(Relaxed),
        can_retry: phase == copy::DONE && !running(o) && !copy::retry_pairs(&o.items, &o.dest_path, &errors).is_empty(),
        errors,
        checked: st.checked.load(Relaxed),
        check_total: st.check_total.load(Relaxed),
        elapsed: o.done_at.unwrap_or_else(Instant::now).duration_since(o.started).as_secs(),
        cpu: o.cpu,
        disks: o.disks.clone(),
    }
}

fn repaint(o: &mut Win) {
    let v = view(o);
    let hover = o.hover;
    let mut hits = Vec::new();
    o.layer.paint(o.pos.0, o.pos.1, W, height(&v), |c, r| hits = draw(c, r, &v, hover));
    o.hits = hits;
}

fn running(o: &Win) -> bool {
    o.worker.as_ref().is_some_and(|t| !t.is_finished())
}

/// Copia num thread e, ao terminar (ou ao ser cancelada), põe a cópia no histórico.
fn spawn(st: Arc<State>, pairs: Vec<(PathBuf, PathBuf)>, mv: bool, rec: Record) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        copy::run_to(&pairs, mv, &st, copy::free_space);
        let errors = st.errors.lock().unwrap().clone();
        let result = match st.phase.load(Relaxed) {
            copy::NO_SPACE => copylist::NO_SPACE,
            copy::CANCELED => copylist::CANCELED,
            _ if !errors.is_empty() => copylist::ERRORS,
            _ if st.verify.load(Relaxed) => copylist::VERIFIED,
            _ => copylist::OK,
        };
        let rec = Record { at: copylist::now(), mv, result, bytes: st.done.load(Relaxed), files: st.files_done.load(Relaxed), errors, ..rec };
        let _ = copylist::append(&copylist::history_path(), &rec);
    })
}

fn record(o: &Win) -> Record {
    Record { name: o.name.clone(), dest: o.dest_path.display().to_string(), items: o.items.iter().map(|p| p.display().to_string()).collect(), ..Default::default() }
}

/// Copia de novo só o que deu erro, substituindo o que já tiver chegado pela metade.
fn retry(o: &mut Win) {
    let errors = o.st.errors.lock().unwrap().clone();
    let pairs = copy::retry_pairs(&o.items, &o.dest_path, &errors);
    if pairs.is_empty() || running(o) {
        return;
    }
    let st = Arc::new(State::default());
    st.policy.store(copy::REPLACE, Relaxed);
    st.verify.store(o.st.verify.load(Relaxed), Relaxed);
    o.st = st.clone();
    o.samples.clear();
    o.speed = 0.0;
    o.last = (Instant::now(), 0);
    o.done_at = None;
    o.started = Instant::now();
    o.worker = Some(spawn(st, pairs, o.mv, record(o)));
}

/// Lê a CPU e o tempo ativo dos discos de origem e destino (uma vez por segundo).
fn meter(o: &mut Win) {
    o.cpu = Some(monitor::read_cpu(&mut o.cpu_prev, &mut o.cpu_threads));
    let Some(d) = o.disk.as_mut() else { return };
    let all = d.read();
    let find = |l: &Option<String>| l.as_ref().and_then(|l| monitor::disk_of(&all, l).map(|i| (l.clone(), all[i].1, i)));
    o.disks = match (find(&o.drives.0), find(&o.drives.1)) {
        (Some(a), Some(b)) if a.2 == b.2 => vec![("Disco de origem e destino", a.0, a.1)],
        (a, b) => [("Disco de origem", a), ("Disco de destino", b)].into_iter().filter_map(|(k, x)| x.map(|(l, p, _)| (k, l, p))).collect(),
    };
}

/// Na frente das outras janelas. Aberta pelo Ctrl+V, quem abre é o Prisma em segundo plano e o
/// Windows recusa o SetForegroundWindow: ligar a entrada à da janela em foco (o Explorer) libera.
unsafe fn to_front(h: HWND) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    let fg = GetForegroundWindow();
    let fg_thread = GetWindowThreadProcessId(fg, None);
    let me = GetCurrentThreadId();
    let attached = fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
    // Mesmo se a ativação for recusada, a janela sobe para cima das outras.
    let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
    let _ = SetWindowPos(h, Some(HWND_NOTOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
    let _ = BringWindowToTop(h);
    let _ = SetForegroundWindow(h);
    if attached {
        let _ = AttachThreadInput(me, fg_thread, false);
    }
}

/// Fecha: cancela e espera o Windows apagar o arquivo pela metade antes de sair.
fn close(h: HWND, o: &mut Win) {
    o.st.cancel.store(true, Relaxed);
    if let Some(t) = o.worker.take() {
        let _ = t.join();
    }
    unsafe {
        let _ = DestroyWindow(h);
    }
}

fn act(h: HWND, o: &mut Win, p: Pick) {
    match p {
        Pick::Policy(v) => o.st.policy.store(v, Relaxed),
        Pick::Verify => {
            let on = !o.st.verify.load(Relaxed);
            o.st.verify.store(on, Relaxed);
        }
        Pick::CloseDone => o.close_done = !o.close_done,
        Pick::Pause => {
            let on = !o.st.paused.load(Relaxed);
            o.st.paused.store(on, Relaxed);
        }
        Pick::Retry => retry(o),
        Pick::Close => return close(h, o),
    }
    repaint(o);
}

unsafe extern "system" fn proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_DESTROY {
        PostQuitMessage(0);
        return LRESULT(0);
    }
    let out = WIN.with(|cell| {
        let Ok(mut b) = cell.try_borrow_mut() else { return None };
        let o = b.as_mut()?;
        let scale = o.layer.scale.max(0.5);
        let at = |l: LPARAM| ((l.0 & 0xFFFF) as i16 as f32 / scale, ((l.0 >> 16) & 0xFFFF) as i16 as f32 / scale);
        match m {
            // Fora dos botões, arrastar move a janela.
            WM_NCHITTEST => {
                let (sx, sy) = at(l);
                let (x, y) = (sx - o.pos.0 as f32 / scale, sy - o.pos.1 as f32 / scale);
                let ht = if o.hits.iter().any(|t| t.0.contains(x, y)) { HTCLIENT } else { HTCAPTION };
                Some(LRESULT(ht as isize))
            }
            WM_MOVE => {
                let mut rc = RECT::default();
                let _ = GetWindowRect(h, &mut rc);
                o.pos = (rc.left, rc.top);
                Some(LRESULT(0))
            }
            WM_MOUSEMOVE => {
                let (x, y) = at(l);
                let hover = o.hits.iter().position(|t| t.0.contains(x, y));
                if hover != o.hover {
                    o.hover = hover;
                    repaint(o);
                }
                Some(LRESULT(0))
            }
            WM_LBUTTONUP => {
                let (x, y) = at(l);
                if let Some(p) = o.hits.iter().find(|t| t.0.contains(x, y)).map(|t| t.1) {
                    act(h, o, p);
                }
                Some(LRESULT(0))
            }
            WM_KEYDOWN => {
                match VIRTUAL_KEY(w.0 as u16) {
                    VK_ESCAPE => close(h, o),
                    VK_SPACE if running(o) => act(h, o, Pick::Pause),
                    _ => {}
                }
                Some(LRESULT(0))
            }
            WM_CLOSE => {
                close(h, o);
                Some(LRESULT(0))
            }
            WM_TIMER => {
                let now = Instant::now();
                let phase = o.st.phase.load(Relaxed);
                let done = if phase == copy::VERIFYING { o.st.checked.load(Relaxed) } else { o.st.done.load(Relaxed) };
                if phase != o.seen {
                    o.seen = phase;
                    o.speed = 0.0;
                    o.last = (now, done);
                }
                let dt = now.duration_since(o.last.0).as_secs_f64();
                if dt >= 0.5 {
                    let rate = done.saturating_sub(o.last.1) as f64 / dt;
                    o.speed = if o.speed == 0.0 { rate } else { o.speed * 0.7 + rate * 0.3 };
                    o.last = (now, done);
                    if running(o) && phase == copy::COPYING && !o.st.paused.load(Relaxed) {
                        if o.samples.len() == SAMPLES {
                            o.samples.remove(0);
                        }
                        o.samples.push(rate as f32);
                    }
                }
                if now.duration_since(o.meter_at).as_millis() >= 1000 {
                    o.meter_at = now;
                    meter(o);
                }
                let finished = !running(o);
                if finished {
                    o.done_at.get_or_insert(now);
                }
                repaint(o);
                // Terminou sem erro (e conferida, com o hash ligado): fecha sozinha depois de mostrar o
                // "Pronto". Com erro, conflito ou hash que não bate, fica aberta com o aviso.
                let ok = phase == copy::DONE && o.st.errors.lock().unwrap().is_empty();
                if o.close_done && finished && ok && o.done_at.is_some_and(|at| now.duration_since(at).as_millis() >= 1200) {
                    close(h, o);
                }
                Some(LRESULT(0))
            }
            _ => None,
        }
    });
    out.unwrap_or_else(|| DefWindowProcW(h, m, w, l))
}

fn open(items: Vec<PathBuf>, dest: PathBuf, mv: bool, opts: Opts) {
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let Ok(inst) = GetModuleHandleW(None) else { return };
        let inst: HINSTANCE = inst.into();
        let class = w!("PrismaColar");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: inst,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(inst), PCWSTR(1 as *const u16)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let caption = if opts.name.is_empty() { tr("Cópia do Prisma").into_owned() } else { opts.name.clone() };
        let caption = HSTRING::from(caption.as_str());
        let Ok(h) = CreateWindowExW(WS_EX_LAYERED | WS_EX_APPWINDOW, class, &caption, WS_POPUP, 0, 0, 1, 1, None, None, Some(inst), None) else { return };
        // No meio da tela onde está o mouse.
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut mi);
        let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(h) as f32 / 96.0;
        let wa = mi.rcWork;
        let pos = ((wa.left + wa.right - (W * scale) as i32) / 2, (wa.top + wa.bottom - (460.0 * scale) as i32) / 2);

        let st = Arc::new(State::default());
        st.policy.store(opts.policy, Relaxed);
        st.verify.store(opts.verify, Relaxed);
        let dest_name = dest.file_name().map_or(dest.display().to_string(), |n| n.to_string_lossy().into_owned());
        let mut o = Win {
            layer: Layered::new(h),
            hits: Vec::new(),
            hover: None,
            pos,
            st: st.clone(),
            from: from_text(&items),
            items,
            dest_path: dest,
            dest: dest_name,
            name: opts.name,
            mv,
            close_done: true,
            speed: 0.0,
            samples: Vec::new(),
            last: (Instant::now(), 0),
            worker: None,
            done_at: None,
            started: Instant::now(),
            seen: copy::PREPARING,
            cpu_prev: Vec::new(),
            cpu_threads: Vec::new(),
            cpu: None,
            disk: DiskBusy::new(),
            drives: (None, None),
            disks: Vec::new(),
            meter_at: Instant::now(),
        };
        o.drives = (o.items.first().and_then(|p| copy::drive(p)), copy::drive(&o.dest_path));
        // A primeira leitura da CPU só guarda a base.
        monitor::read_cpu(&mut o.cpu_prev, &mut o.cpu_threads);
        let pairs = o.items.iter().map(|i| (i.clone(), o.dest_path.clone())).collect();
        o.worker = Some(spawn(st, pairs, mv, record(&o)));
        repaint(&mut o);
        WIN.with(|cell| *cell.borrow_mut() = Some(o));
        if opts.quiet {
            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
        } else {
            let _ = ShowWindow(h, SW_SHOW);
            to_front(h);
        }
        SetTimer(Some(h), 1, 100, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // Saída por fora da janela (fim de sessão): não deixa arquivo pela metade.
        WIN.with(|cell| {
            if let Some(mut o) = cell.borrow_mut().take() {
                o.st.cancel.store(true, Relaxed);
                if let Some(t) = o.worker.take() {
                    let _ = t.join();
                }
            }
        });
    }
}

/// Teste: a janela em cinco momentos, salva como `copia_<n>.png` na pasta.
#[cfg(debug_assertions)]
fn test(dir: &str) {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--test-en") {
        crate::i18n::set_lang(1);
    }
    if let Some(n) = args.iter().find_map(|a| a.strip_prefix("--test-lang=").and_then(|v| v.parse().ok())) {
        crate::i18n::set_lang(n);
    }
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let Ok(inst) = GetModuleHandleW(None) else { return };
        let inst: HINSTANCE = inst.into();
        let class = w!("PrismaColarTeste");
        unsafe extern "system" fn p(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(p), hInstance: inst, lpszClassName: class, ..Default::default() });
        let Ok(h) = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOOLWINDOW, class, w!(""), WS_POPUP, -4000, -4000, 1, 1, None, None, Some(inst), None) else { return };
        let errors: Vec<(String, String)> = [
            ("D:\\Projetos\\relatorio_final_versao_revisada_com_comentarios_da_equipe.docx", "Em uso por outro programa"),
            ("D:\\Projetos\\banco.db", "Acesso negado"),
            ("D:\\Projetos\\video.mp4", "O hash da cópia não confere"),
            ("D:\\Projetos\\sistema.sys", "Caminho longo demais"),
            ("D:\\Projetos\\notas.txt", "Não encontrado"),
            ("D:\\Projetos\\x.bin", "Sem espaço no disco"),
            ("D:\\Projetos\\y.bin", "Uma pasta não pode ir para dentro dela mesma"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect();
        // Velocidade que oscila, como num disco de verdade.
        let samples: Vec<f32> = (0..SAMPLES).map(|i| 80e6 + 25e6 * ((i as f32) * 0.21).sin() + 10e6 * ((i as f32) * 0.77).cos()).collect();
        let base = || View {
            dest: "Documentos do trabalho".into(),
            dest_path: "E:\\Backup\\Documentos do trabalho".into(),
            from: "D:\\Fotos\\Viagem 2024, Projetos, notas.txt".into(),
            mv: false,
            phase: copy::COPYING,
            current: "D:\\Fotos\\Viagem 2024\\IMG_2041.jpg".into(),
            done: 1_342_177_280,
            total: 3_650_722_201,
            files_done: 34,
            files: 1208,
            speed: 89_128_960.0,
            samples: samples.clone(),
            asking: false,
            policy: copy::ASK,
            verify: true,
            close_done: true,
            paused: false,
            need: 0,
            free: 0,
            errors: Vec::new(),
            can_retry: false,
            checked: 0,
            check_total: 0,
            elapsed: 15,
            cpu: Some(12.0),
            disks: vec![("Disco de origem", "D:".into(), 64.0), ("Disco de destino", "E:".into(), 97.0)],
        };
        let views = [
            View { asking: true, ..base() },
            View { mv: true, policy: copy::KEEP_BOTH, paused: true, errors: errors[..2].to_vec(), ..base() },
            View { phase: copy::DONE, policy: copy::REPLACE, current: String::new(), done: 3_650_722_201, files_done: 1208, errors, can_retry: true, ..base() },
            View { phase: copy::NO_SPACE, need: 3_650_722_201, free: 1_288_490_188, done: 0, files_done: 0, samples: Vec::new(), ..base() },
            View { phase: copy::VERIFYING, done: 3_650_722_201, files_done: 1208, checked: 2_400_000_000, check_total: 7_301_444_402, elapsed: 3725, disks: vec![("Disco de origem e destino", "D:".into(), 88.0)], ..base() },
        ];
        std::env::set_var("PRISMA_LAYER_DUMP", dir);
        for (i, v) in views.iter().enumerate() {
            let mut layer = Layered::new(h);
            layer.paint(-4000, -4000, W, height(v), |c, r| {
                draw(c, r, v, None);
            });
            let from = std::path::Path::new(dir).join(format!("camada_{}.png", (W * layer.scale).ceil() as i32));
            let _ = std::fs::rename(&from, std::path::Path::new(dir).join(format!("copia_{i}.png")));
        }
        let _ = DestroyWindow(h);
    }
}
