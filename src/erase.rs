//! Janela do Shift+Del do Prisma, num prisma.exe à parte. `--erase` recebe os itens pela entrada do
//! processo (o Prisma aberto escreve, uma linha por item), pergunta e apaga; `--erase-admin=` é a cópia
//! de administrador que continua o que o Windows negou. O motor fica em `sys::erase`.

use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Instant;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_ESCAPE, VK_RETURN};
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::i18n::tr;
use crate::paste::size_text;
use crate::sys::erase::{self, Erased, State};
use crate::ui::gfx::{rect, Align, Canvas, Rect, Weight, ACCENT, BORDER, CARD, CARD2, LINE, MUTED, ON_ACCENT, TEXT, TEXT2, WARN};
use crate::ui::layered::Layered;

const W: f32 = 520.0;
const PAD: f32 = 20.0;
const MAX_LINES: usize = 5;

#[derive(Clone, Copy, PartialEq)]
enum Pick {
    Erase,
    Close,
}

/// O que a janela mostra, copiado do trabalho a cada quadro.
#[derive(Clone, Default)]
struct View {
    /// Ainda perguntando.
    asking: bool,
    admin: bool,
    phase: u8,
    name: String,
    parent: String,
    counted: bool,
    files: u64,
    bytes: u64,
    done: u64,
    freed: u64,
    pending: u64,
    plan_b: bool,
    step: u8,
    current: String,
    /// (item, motivo) dos itens que o Prisma não apaga.
    refused: Vec<(String, String)>,
    errors: Vec<(String, String)>,
}

struct Win {
    layer: Layered,
    hits: Vec<(Rect, Pick)>,
    hover: Option<usize>,
    pos: (i32, i32),
    items: Vec<PathBuf>,
    refused: Vec<(String, String)>,
    admin: bool,
    /// A contagem roda à parte e para quando a pessoa confirma.
    count: Arc<State>,
    st: Arc<State>,
    asking: bool,
    /// Já apagado pela cópia sem administrador (só na cópia de administrador).
    base: (u64, u64),
    worker: Option<std::thread::JoinHandle<()>>,
    done_at: Option<Instant>,
}

thread_local! {
    static WIN: std::cell::RefCell<Option<Win>> = const { std::cell::RefCell::new(None) };
}

/// `--erase` ou `--erase-admin=`: true quando o argumento veio e esta cópia já fez o seu trabalho.
pub fn run_args() -> bool {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::args().find_map(|a| a.strip_prefix("--test-erase=").map(str::to_string)) {
        test(&dir);
        return true;
    }
    if let Some(v) = std::env::args().find_map(|a| a.strip_prefix("--erase-admin=").map(str::to_string)) {
        // Só a cópia que o Windows elevou; os caminhos passam de novo pela recusa no motor.
        if let (true, Some((done, freed, items))) = (crate::startup::is_elevated(), erase::parse_admin(&v)) {
            open(items, true, false, (done, freed));
        }
        return true;
    }
    if !std::env::args().any(|a| a == "--erase") {
        return false;
    }
    let mut text = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text);
    let items: Vec<PathBuf> = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(PathBuf::from).filter(|p| p.is_absolute()).collect();
    if !items.is_empty() {
        open(items, crate::startup::is_elevated(), true, (0, 0));
    }
    true
}

fn file_name(p: &str) -> String {
    std::path::Path::new(p).file_name().map_or(p.to_string(), |n| n.to_string_lossy().into_owned())
}

fn title(v: &View) -> String {
    let errs = v.errors.len();
    match v.phase {
        _ if v.asking => tr("Apagar para sempre?").into_owned(),
        erase::NEED_ADMIN | erase::HANDED => tr("Pedindo permissão de administrador…").into_owned(),
        erase::CANCELED => tr("Cancelado").into_owned(),
        erase::DONE if errs == 0 => tr("Apagado").into_owned(),
        erase::DONE => tr(&format!("Apagado, com {} erros", errs)).into_owned(),
        _ => tr("Apagando…").into_owned(),
    }
}

/// "1.208 arquivos, 3,4 GB", com o que se sabe até agora.
fn amount(files: u64, bytes: u64) -> String {
    let n = if files == 1 { tr("1 arquivo").into_owned() } else { tr(&format!("{} arquivos", files)).into_owned() };
    format!("{n}, {}", size_text(bytes))
}

fn step_text(v: &View) -> String {
    let name = file_name(&v.current);
    match v.step {
        erase::STEP_OWNER => tr(&format!("Tomando posse de {}", name)).into_owned(),
        erase::STEP_GRANT => tr(&format!("Dando permissão em {}", name)).into_owned(),
        erase::STEP_PLAN_B => tr(&format!("Plano B (takeown e icacls) em {}", name)).into_owned(),
        _ => String::new(),
    }
}

fn lines_height(n: usize) -> f32 {
    if n == 0 {
        0.0
    } else {
        26.0 + n.min(MAX_LINES) as f32 * 20.0 + if n > MAX_LINES { 20.0 } else { 0.0 } + 6.0
    }
}

fn height(v: &View) -> f32 {
    let body = if v.asking { 196.0 } else { 150.0 };
    let notes = if v.asking { 0.0 } else { (v.pending > 0) as u8 as f32 * 20.0 + v.plan_b as u8 as f32 * 20.0 };
    body + notes + lines_height(v.refused.len()) + lines_height(v.errors.len()) + 66.0
}

fn list(c: &Canvas, x: f32, w: f32, y: &mut f32, head: &str, rows: &[(String, String)]) {
    if rows.is_empty() {
        return;
    }
    c.line(x, *y, x + w, *y, LINE, 1.0);
    c.text_raw(rect(x, *y + 6.0, w, 20.0), head, 13.0, WARN, Align::Left, Weight::Semibold);
    *y += 26.0;
    for (path, why) in rows.iter().take(MAX_LINES) {
        c.text_raw(rect(x, *y, w * 0.55 - 8.0, 20.0), &file_name(path), 12.0, TEXT2, Align::Left, Weight::Regular);
        c.text_fit(rect(x + w * 0.55, *y, w * 0.45, 20.0), why, 12.0, MUTED, Align::Right, Weight::Regular);
        *y += 20.0;
    }
    if rows.len() > MAX_LINES {
        c.text_raw(rect(x, *y, w, 20.0), &tr(&format!("e mais {}", rows.len() - MAX_LINES)), 12.0, MUTED, Align::Left, Weight::Regular);
        *y += 20.0;
    }
    *y += 6.0;
}

/// Desenha a janela e devolve as áreas que se clicam.
fn draw(c: &Canvas, r: Rect, v: &View, hover: Option<usize>) -> Vec<(Rect, Pick)> {
    let mut hits = Vec::new();
    c.round(r, 12.0, CARD);
    c.round_line(r, 12.0, BORDER, 1.0);
    let x = r.x + PAD;
    let w = r.w - 2.0 * PAD;
    c.text_raw(rect(x, 16.0, w, 24.0), &title(v), 16.0, if v.asking { WARN } else { TEXT }, Align::Left, Weight::Semibold);
    c.text_raw(rect(x, 46.0, w, 20.0), &v.name, 14.0, TEXT, Align::Left, Weight::Semibold);
    c.text_raw(rect(x, 66.0, w, 18.0), &v.parent, 12.0, MUTED, Align::Left, Weight::Regular);
    let mut y = 92.0;
    if v.asking {
        let n = amount(v.files, v.bytes);
        let n = if v.counted { n } else { tr(&format!("Contando… {}", n)).into_owned() };
        c.text_raw(rect(x, y, w, 20.0), &n, 13.0, TEXT2, Align::Left, Weight::Regular);
        let warn = rect(x, y + 30.0, w, 58.0);
        c.round(warn, 8.0, WARN.alpha(0.12));
        c.round_line(warn, 8.0, WARN.alpha(0.5), 1.0);
        c.text(rect(warn.x + 12.0, warn.y + 8.0, warn.w - 24.0, 20.0), "Não passa pela Lixeira e não tem volta.", 13.0, WARN, Align::Left, Weight::Semibold);
        let note = if v.admin { "Como administrador, passa por cima das permissões." } else { "O que o Windows negar pede a permissão de administrador." };
        c.text_fit(rect(warn.x + 12.0, warn.y + 30.0, warn.w - 24.0, 18.0), note, 12.0, TEXT2, Align::Left, Weight::Regular);
        y = 196.0;
    } else {
        let (all, cur) = (v.files.max(v.done), v.done);
        let frac = if v.phase == erase::DONE || all == 0 { if v.phase == erase::DELETING { 0.0 } else { 1.0 } } else { cur as f32 / all as f32 };
        let bar = rect(x, y, w, 8.0);
        c.round(bar, 4.0, LINE);
        if frac > 0.0 {
            c.round(rect(bar.x, bar.y, (bar.w * frac.min(1.0)).max(8.0), bar.h), 4.0, ACCENT);
        }
        let count = if v.counted && v.phase == erase::DELETING { tr(&format!("{} de {} arquivos", cur, all)).into_owned() } else { amount(cur, v.freed) };
        c.text_raw(rect(x, y + 16.0, w, 20.0), &count, 13.0, TEXT2, Align::Left, Weight::Regular);
        c.text_raw(rect(x, y + 36.0, w, 18.0), &step_text(v), 12.0, ACCENT, Align::Left, Weight::Regular);
        y = 150.0;
        if v.pending > 0 {
            let t = if v.pending == 1 { tr("1 item fica para o próximo reinício").into_owned() } else { tr(&format!("{} itens ficam para o próximo reinício", v.pending)).into_owned() };
            c.text_raw(rect(x, y, w, 20.0), &t, 13.0, WARN, Align::Left, Weight::Regular);
            y += 20.0;
        }
        if v.plan_b {
            c.text(rect(x, y, w, 20.0), "Usou o plano B (takeown e icacls)", 12.0, MUTED, Align::Left, Weight::Regular);
            y += 20.0;
        }
    }
    list(c, x, w, &mut y, &tr("Ficam de fora"), &v.refused);
    let head = if v.errors.len() == 1 { tr("1 erro").into_owned() } else { tr(&format!("{} erros", v.errors.len())).into_owned() };
    list(c, x, w, &mut y, &head, &v.errors);

    let by = y + 16.0;
    let bw = 170.0;
    let button = |hits: &mut Vec<(Rect, Pick)>, s: Rect, t: &str, primary: bool, p: Pick| {
        let hov = hover == Some(hits.len());
        c.round(s, 8.0, if primary { if hov { WARN.alpha(0.85) } else { WARN } } else if hov { CARD2 } else { LINE });
        c.text_fit(s.inset_xy(8.0, 0.0), t, 13.0, if primary { ON_ACCENT } else { TEXT }, Align::Center, Weight::Semibold);
        hits.push((s, p));
    };
    let left = rect(r.right() - PAD - 2.0 * bw - 10.0, by, bw, 34.0);
    let right = rect(r.right() - PAD - bw, by, bw, 34.0);
    let running = !v.asking && v.phase == erase::DELETING;
    if v.asking && !v.name.is_empty() {
        button(&mut hits, left, "Apagar para sempre", true, Pick::Erase);
        button(&mut hits, right, "Cancelar", false, Pick::Close);
    } else {
        button(&mut hits, right, if running { "Cancelar" } else { "Fechar" }, false, Pick::Close);
    }
    hits
}

/// Nome do que vai ser apagado: o item, ou "N itens" com a pasta de onde vêm.
fn names(items: &[PathBuf]) -> (String, String) {
    let parent = items.first().and_then(|p| p.parent()).map(|p| p.display().to_string()).unwrap_or_default();
    match items {
        [] => (String::new(), String::new()),
        [one] => (one.file_name().map_or(one.display().to_string(), |n| n.to_string_lossy().into_owned()), parent),
        many => (tr(&format!("{} itens", many.len())).into_owned(), parent),
    }
}

fn view(o: &Win) -> View {
    let st = if o.asking { &o.count } else { &o.st };
    let (name, parent) = names(&o.items);
    View {
        asking: o.asking,
        admin: o.admin,
        phase: o.st.phase.load(Relaxed),
        name,
        parent,
        counted: o.count.counted.load(Relaxed),
        files: o.count.files.load(Relaxed),
        bytes: o.count.bytes.load(Relaxed),
        done: o.base.0 + o.st.done.load(Relaxed),
        freed: o.base.1 + o.st.freed.load(Relaxed),
        pending: o.st.pending.load(Relaxed),
        plan_b: o.st.plan_b.load(Relaxed),
        step: st.step.load(Relaxed),
        current: st.current.lock().unwrap().clone(),
        refused: o.refused.clone(),
        errors: o.st.errors.lock().unwrap().clone(),
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

/// O histórico da página Cópias.
fn record(st: &State, items: &[PathBuf], base: (u64, u64)) {
    let rec = Erased {
        at: crate::sys::copylist::now(),
        files: base.0 + st.done.load(Relaxed),
        bytes: base.1 + st.freed.load(Relaxed),
        pending: st.pending.load(Relaxed),
        errors: st.errors.lock().unwrap().len() as u32,
        plan_b: st.plan_b.load(Relaxed),
        items: items.iter().map(|p| p.display().to_string()).collect(),
    };
    let _ = erase::append(&erase::history_path(), &rec);
}

/// Confirmado: para a contagem e apaga num thread. Sem administrador e com algo negado, o
/// histórico fica para quem terminar (a cópia de administrador ou a janela, se o Windows recusar).
fn start(o: &mut Win) {
    if !o.asking || o.items.is_empty() {
        return;
    }
    o.asking = false;
    let counter = o.worker.take();
    // A contagem parada no meio não serve como total.
    let whole = counter.as_ref().is_none_or(|t| t.is_finished());
    if !whole {
        o.count.cancel.store(true, Relaxed);
    }
    let (count, st, items, admin, base) = (o.count.clone(), o.st.clone(), o.items.clone(), o.admin, o.base);
    o.worker = Some(std::thread::spawn(move || {
        if let Some(t) = counter {
            let _ = t.join();
        }
        if !whole {
            count.counted.store(false, Relaxed);
        }
        erase::erase(&items, &st, admin);
        if st.phase.load(Relaxed) != erase::NEED_ADMIN {
            record(&st, &items, base);
        }
    }));
}

/// Sem administrador, o Windows negou algo: abre a cópia de administrador com tudo que foi escolhido
/// (o que já saiu ela pula). Recusada a confirmação do Windows, a janela mostra e grava o histórico.
fn hand_off(h: HWND, o: &mut Win) {
    o.st.phase.store(erase::HANDED, Relaxed);
    repaint(o);
    let arg = erase::admin_arg(o.st.done.load(Relaxed), o.st.freed.load(Relaxed), &o.items);
    // shortcut: a linha de comando aceita uns 32 mil caracteres; uma escolha maior que isso
    // falha como recusa e mostra o erro, sem perder nada.
    if arg.len() < 32_000 && crate::startup::run_elevated(&arg) {
        unsafe {
            let _ = DestroyWindow(h);
        }
        return;
    }
    let first = o.st.denied.lock().unwrap().first().map(|p| p.display().to_string()).unwrap_or_default();
    o.st.errors.lock().unwrap().push((first, tr("O Windows não deu a permissão de administrador").into_owned()));
    o.st.phase.store(erase::DONE, Relaxed);
    record(&o.st, &o.items, o.base);
    o.done_at = Some(Instant::now());
    repaint(o);
}

/// Fecha: cancela e espera o que está sendo apagado parar antes de sair.
fn close(h: HWND, o: &mut Win) {
    o.count.cancel.store(true, Relaxed);
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
        Pick::Erase => start(o),
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
                    VK_RETURN if o.asking => act(h, o, Pick::Erase),
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
                if o.st.phase.load(Relaxed) == erase::NEED_ADMIN && !running(o) {
                    hand_off(h, o);
                    return Some(LRESULT(0));
                }
                let finished = !o.asking && !running(o);
                if !finished || o.done_at.is_none() {
                    repaint(o);
                }
                if finished {
                    o.done_at.get_or_insert(now);
                }
                // Tudo saiu, sem erro nem nada para o reinício: fecha sozinha depois de mostrar.
                let clean = o.st.phase.load(Relaxed) == erase::DONE && o.st.errors.lock().unwrap().is_empty() && o.st.pending.load(Relaxed) == 0 && o.refused.is_empty();
                if finished && clean && o.done_at.is_some_and(|at| now.duration_since(at).as_millis() >= 1200) {
                    close(h, o);
                }
                Some(LRESULT(0))
            }
            _ => None,
        }
    });
    out.unwrap_or_else(|| DefWindowProcW(h, m, w, l))
}

/// `ask`: pergunta antes (o Shift+Del); a cópia de administrador já foi confirmada e começa a apagar.
fn open(items: Vec<PathBuf>, admin: bool, ask: bool, base: (u64, u64)) {
    if admin {
        erase::enable_privileges();
    }
    // A recusa aparece já na pergunta; o motor confere de novo pelo caminho de verdade.
    let pr = erase::protected();
    let (items, refused): (Vec<PathBuf>, Vec<PathBuf>) = items.into_iter().partition(|p| erase::refusal(p, &pr).is_none());
    let refused = refused.iter().map(|p| (p.display().to_string(), tr(erase::refusal(p, &pr).unwrap_or_default()).into_owned())).collect();
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let Ok(inst) = GetModuleHandleW(None) else { return };
        let inst: HINSTANCE = inst.into();
        let class = w!("PrismaApagar");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: inst,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(inst), PCWSTR(1 as *const u16)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let caption = HSTRING::from(tr("Shift+Del do Prisma").as_ref());
        let Ok(h) = CreateWindowExW(WS_EX_LAYERED | WS_EX_APPWINDOW, class, &caption, WS_POPUP, 0, 0, 1, 1, None, None, Some(inst), None) else { return };
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST), &mut mi);
        let scale = windows::Win32::UI::HiDpi::GetDpiForWindow(h) as f32 / 96.0;
        let wa = mi.rcWork;
        let pos = ((wa.left + wa.right - (W * scale) as i32) / 2, (wa.top + wa.bottom - (320.0 * scale) as i32) / 2);
        let count = Arc::new(State::default());
        let mut o = Win {
            layer: Layered::new(h),
            hits: Vec::new(),
            hover: None,
            pos,
            items,
            refused,
            admin,
            count: count.clone(),
            st: Arc::new(State::default()),
            asking: true,
            base,
            worker: None,
            done_at: None,
        };
        if !o.items.is_empty() {
            let items = o.items.clone();
            o.worker = Some(std::thread::spawn(move || erase::count(&items, &count, admin)));
        }
        if !ask {
            start(&mut o);
        }
        repaint(&mut o);
        WIN.with(|cell| *cell.borrow_mut() = Some(o));
        let _ = ShowWindow(h, SW_SHOW);
        crate::paste::to_front(h);
        SetTimer(Some(h), 1, 100, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        WIN.with(|cell| {
            if let Some(mut o) = cell.borrow_mut().take() {
                o.count.cancel.store(true, Relaxed);
                o.st.cancel.store(true, Relaxed);
                if let Some(t) = o.worker.take() {
                    let _ = t.join();
                }
            }
        });
    }
}

/// Teste: a janela em quatro momentos, salva como `apagar_<n>.png` na pasta.
#[cfg(debug_assertions)]
fn test(dir: &str) {
    if let Some(n) = std::env::args().find_map(|a| a.strip_prefix("--test-lang=").and_then(|v| v.parse().ok())) {
        crate::i18n::set_lang(n);
    }
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let Ok(inst) = GetModuleHandleW(None) else { return };
        let inst: HINSTANCE = inst.into();
        let class = w!("PrismaApagarTeste");
        unsafe extern "system" fn p(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
            DefWindowProcW(h, m, w, l)
        }
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(p), hInstance: inst, lpszClassName: class, ..Default::default() });
        let Ok(h) = CreateWindowExW(WS_EX_LAYERED | WS_EX_TOOLWINDOW, class, w!(""), WS_POPUP, -4000, -4000, 1, 1, None, None, Some(inst), None) else { return };
        let base = || View {
            asking: true,
            admin: false,
            phase: erase::COUNTING,
            name: "WindowsApps antigo".into(),
            parent: "D:\\Programas".into(),
            counted: false,
            files: 12_345,
            bytes: 3_650_722_201,
            ..Default::default()
        };
        let errors = vec![("D:\\Programas\\WindowsApps antigo\\app.dll".to_string(), tr("Em uso por outro programa").into_owned())];
        let views = [
            View { refused: vec![("C:\\Windows".into(), tr("Pasta do Windows ou do Prisma").into_owned())], ..base() },
            View { asking: false, admin: true, phase: erase::DELETING, counted: true, done: 8_000, freed: 2_000_000_000, step: erase::STEP_PLAN_B, current: "D:\\Programas\\WindowsApps antigo\\x".into(), ..base() },
            View { asking: false, admin: true, phase: erase::DONE, counted: true, done: 12_344, freed: 3_650_000_000, pending: 1, plan_b: true, errors, ..base() },
            View { asking: false, phase: erase::DONE, counted: true, done: 12_345, freed: 3_650_722_201, name: "3 itens".into(), ..base() },
        ];
        std::env::set_var("PRISMA_LAYER_DUMP", dir);
        for (i, v) in views.iter().enumerate() {
            let mut layer = Layered::new(h);
            layer.paint(-4000, -4000, W, height(v), |c, r| {
                draw(c, r, v, None);
            });
            let from = std::path::Path::new(dir).join(format!("camada_{}.png", (W * layer.scale).ceil() as i32));
            let _ = std::fs::rename(&from, std::path::Path::new(dir).join(format!("apagar_{i}.png")));
        }
        let _ = DestroyWindow(h);
    }
}
