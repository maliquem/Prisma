//! Estado do programa e janelas principais. Tudo aqui roda na thread da interface.

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use windows::core::*;
use crate::config::Defaults;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::capture;
use crate::config::{Config, Hotkey, MOD_ALT, MOD_CTRL, MOD_SHIFT, MOD_WIN};
use crate::optimizer::{self, Candidate, Usage};
use crate::capture::engine::{self, Cmd, Handle, Settings, Target};
use crate::capture::mux::Codec;
use crate::overlays::{Hud, Prompt, PromptAnswer, Toast, Widget};
use crate::pages::{self, Page};
use crate::sys::audio::{self, Session};
use crate::sys::fullscreen::Fullscreenizer;
use crate::sys::{audit, autostart, disks, fileinfo, mft, nets, services, tools, tweaks, uninstall};
use crate::sys::monitor::{self, Snapshot};
use crate::sys::power::{self, Detector, Event, Mode, Plan, PlanValues};
use crate::sys::procs;
use crate::ui::gfx::*;
use crate::ui::UiState;

pub const WM_TICK: u32 = WM_APP + 1;
pub const WM_TRAY: u32 = WM_APP + 2;
pub const WM_SHOW: u32 = WM_APP + 3;
const WM_PICK: u32 = WM_APP + 4;
const TIMER_HUD: usize = 7;
const WM_UPDATED: u32 = WM_APP + 77;
const WM_PICK_FS: u32 = WM_APP + 78;
const WM_PICK_SND: u32 = WM_APP + 79;

static FORCE_VK: AtomicU32 = AtomicU32::new(0);
static FORCE_MODS: AtomicU32 = AtomicU32::new(0);
static FORCE_HUB: AtomicIsize = AtomicIsize::new(0);
static FORCE_HELD: AtomicBool = AtomicBool::new(false);

fn apply_route(pid: u32, a: &crate::config::SndApp) -> bool {
    audio::set_route(pid, false, &a.output) && audio::set_route(pid, true, &a.input)
}

fn held_mods() -> u32 {
    let down = |k: VIRTUAL_KEY| unsafe { GetAsyncKeyState(k.0 as i32) as u16 & 0x8000 != 0 };
    let mut m = 0;
    if down(VK_CONTROL) {
        m |= MOD_CTRL;
    }
    if down(VK_MENU) {
        m |= MOD_ALT;
    }
    if down(VK_SHIFT) {
        m |= MOD_SHIFT;
    }
    if down(VK_LWIN) || down(VK_RWIN) {
        m |= MOD_WIN;
    }
    m
}

/// Roda a cada tecla do sistema: precisa ser curto.
unsafe extern "system" fn key_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code == 0 {
        let vk = (*(l.0 as *const KBDLLHOOKSTRUCT)).vkCode;
        if vk == FORCE_VK.load(Ordering::Relaxed) {
            let press = w.0 as u32 == WM_KEYDOWN || w.0 as u32 == WM_SYSKEYDOWN;
            if !press {
                FORCE_HELD.store(false, Ordering::Relaxed);
            } else if held_mods() == FORCE_MODS.load(Ordering::Relaxed) {
                if !FORCE_HELD.swap(true, Ordering::Relaxed) {
                    let hub = HWND(FORCE_HUB.load(Ordering::Relaxed) as *mut _);
                    let _ = PostMessageW(Some(hub), WM_HOTKEY, WPARAM(HK_FORCE as usize), LPARAM(0));
                }
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, w, l)
}
pub const ACT_SHOT: i32 = HK_SHOT;
pub const ACT_RECORD: i32 = HK_REC;

const HK_FULL: i32 = 1;
const HK_WIDGET: i32 = 2;
const HK_OPT: i32 = 3;
const HK_SHOT: i32 = 4;
const HK_SHOT_FULL: i32 = 5;
const HK_REC: i32 = 6;
const HK_REPLAY: i32 = 7;
const HK_FORCE: i32 = 8;
const HK_VOL_UP: i32 = 10;
const HK_VOL_DOWN: i32 = 11;
const HK_MUTE: i32 = 12;

const VK_VOL_UP: u32 = 0xAF;
const VK_VOL_DOWN: u32 = 0xAE;
const VK_MUTE: u32 = 0xAD;

const TRAY_OPEN: u32 = 1;
const TRAY_WIDGET: u32 = 2;
const TRAY_OPT: u32 = 3;
const TRAY_SHOT: u32 = 4;
const TRAY_EXIT: u32 = 9;

pub struct MainWin {
    pub hwnd: HWND,
    pub rt: ID2D1HwndRenderTarget,
    pub canvas: Canvas,
    pub ui: UiState,
    pub scale: f32,
    pub w: f32,
    pub h: f32,
}

pub struct WinInfo {
    pub hwnd: HWND,
    pub title: String,
    pub exe: String,
    pub path: String,
    pub size: (i32, i32),
}

pub struct App {
    pub cfg: Config,
    pub hub: HWND,
    shared: Arc<Mutex<Snapshot>>,
    pub snap: Snapshot,
    pub main: Option<MainWin>,
    pub page: Page,
    pub full: Fullscreenizer,
    pub detector: Detector,
    pub widget: Widget,
    pub toast: Toast,
    pub prompt: Prompt,
    pub usage: Usage,
    pub hk_errors: Vec<String>,
    pub sessions: Vec<Session>,
    pub plans: Vec<Plan>,
    pub plan_values: Vec<PlanValues>,
    pub active_plan: String,
    pub cands: Vec<Candidate>,
    pub unchecked: HashSet<String>,
    pub checked: HashSet<String>,
    pub stubborn: Vec<(String, Vec<u32>)>,
    pending: Vec<(String, Vec<u32>, Instant)>,
    pub windows: Vec<WinInfo>,
    pub sort: u8,
    pub store_tab: u8,
    pub disks: Vec<disks::Disk>,
    pub disk_sel: usize,
    pub health: disks::HealthMap,
    pub nets: Vec<nets::Net>,
    pub tw_tab: u8,
    pub tw_layers: Vec<char>,
    pub tw_on: HashSet<&'static str>,
    pub tw_journal: Vec<tweaks::Entry>,
    pub tw_apps: Vec<String>,
    tw_apps_slot: std::sync::Arc<Mutex<Option<Vec<String>>>>,
    pub tw_run: tweaks::Shared,
    pub upd: crate::sys::update::Shared,
    upd_told: bool,
    tm_check: Option<Instant>,
    force_hook: Option<HHOOK>,
    pub snd_picker: bool,
    pub snd_search: String,
    snd_seen: std::collections::HashSet<u32>,
    upd_last: Option<Instant>,
    started: Instant,
    pub audit: audit::Shared,
    pub tool_sel: usize,
    pub tool_drive: usize,
    pub tool_mode: usize,
    pub tool_net: usize,
    pub tool_dns: usize,
    pub tool_ack: bool,
    pub tool_src: String,
    pub tool_dst: String,
    pub programs: Vec<uninstall::Program>,
    pub un_sort: u8,
    pub boot_sort: u8,
    pub un_search: String,
    pub un_review: bool,
    pub un_job: uninstall::Shared,
    pub un_left: Option<Vec<uninstall::Leftover>>,
    pub scan: mft::Slot,
    pub scan_progress: std::sync::Arc<mft::Progress>,
    pub scan_drive: usize,
    pub scan_at: Option<u32>,
    pub scan_tab: u8,
    pub scan_sel: Option<u32>,
    pub scan_delete: Option<u32>,
    pub installed: bool,
    pub boot_items: Vec<autostart::Item>,
    pub boot_sel: Option<usize>,
    pub boot_sig: Option<String>,
    pub services: Vec<services::Service>,
    pub svc_filter: u8,
    pub svc_search: String,
    pub svc_sel: Option<String>,
    pub svc_info: (String, String),
    pub selected: Option<String>,
    pub mon_tab: u8,
    pub mon_filter: Option<String>,
    pub sel_pid: Option<u32>,
    /// Alvo do menu aberto no Monitor: programa, processos e o primeiro PID.
    pub mon_target: Option<(String, Vec<u32>)>,
    pub svc_target: Option<(String, String)>,
    tool_logs: std::collections::HashMap<usize, tools::Shared>,
    exe_names: std::collections::HashMap<String, String>,
    exe_paths: std::collections::HashMap<String, Option<String>>,
    pub elevated: bool,
    pub autostart: bool,
    pub replaces_tm: bool,
    fg_pid: u32,
    pub fg_exe: String,
    asked: HashSet<String>,
    last_tick: Instant,
    last_save: Instant,
    capturing_keys: bool,
    engine: Option<Handle>,
    rec_start: Option<Instant>,
    hud: Hud,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Acessa o estado. Devolve None numa mensagem reentrante, que segue para o padrão.
pub fn with<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

pub fn start(background: bool) {
    unsafe {
        let inst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(hub_proc),
            lpszClassName: w!("PrismaHub"),
            hInstance: inst.into(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let main = WNDCLASSW {
            lpfnWndProc: Some(main_proc),
            lpszClassName: w!("PrismaMain"),
            hInstance: inst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: LoadIconW(Some(inst.into()), PCWSTR(1 as *const u16)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&main);
        let hub = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("PrismaHub"),
            w!("Prisma"),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(inst.into()),
            None,
        )
        .expect("janela de controle");
        // Sem isso, uma cópia comum não consegue avisar a cópia aberta como administrador.
        let _ = ChangeWindowMessageFilterEx(hub, WM_SHOW, MSGFLT_ALLOW, None);
        let cfg = Config::load();
        let shared = monitor::spawn(hub.0 as isize, WM_TICK);
        let mut app = App {
            hub,
            shared,
            snap: Snapshot::default(),
            main: None,
            page: Page::Monitor,
            full: Fullscreenizer::default(),
            detector: Detector::new(),
            widget: Widget::new(),
            toast: Toast::new(),
            prompt: Prompt::new(),
            usage: Usage::load(),
            hk_errors: Vec::new(),
            sessions: Vec::new(),
            plans: Vec::new(),
            plan_values: Vec::new(),
            active_plan: String::new(),
            cands: Vec::new(),
            unchecked: HashSet::new(),
            checked: HashSet::new(),
            stubborn: Vec::new(),
            pending: Vec::new(),
            windows: Vec::new(),
            sort: 0,
            store_tab: 0,
            disks: Vec::new(),
            disk_sel: 0,
            health: Default::default(),
            nets: Vec::new(),
            tw_tab: 0,
            tw_layers: Vec::new(),
            tw_on: HashSet::new(),
            tw_journal: tweaks::load_journal(),
            tw_apps: Vec::new(),
            tw_apps_slot: Default::default(),
            tw_run: Default::default(),
            upd: Default::default(),
            upd_told: false,
            tm_check: None,
            force_hook: None,
            snd_picker: false,
            snd_search: String::new(),
            snd_seen: std::collections::HashSet::new(),
            upd_last: None,
            started: Instant::now(),
            audit: Default::default(),
            tool_sel: 0,
            tool_drive: 0,
            tool_mode: 0,
            tool_net: 0,
            tool_dns: 0,
            tool_ack: false,
            tool_src: String::new(),
            tool_dst: String::new(),
            programs: Vec::new(),
            un_sort: 0,
            boot_sort: 0,
            un_search: String::new(),
            un_review: false,
            un_job: Default::default(),
            un_left: None,
            scan: Default::default(),
            scan_progress: Default::default(),
            scan_drive: 0,
            scan_at: None,
            scan_tab: 0,
            scan_sel: None,
            scan_delete: None,
            installed: crate::startup::is_installed(),
            boot_items: Vec::new(),
            boot_sel: None,
            boot_sig: None,
            services: Vec::new(),
            svc_filter: 0,
            svc_search: String::new(),
            svc_sel: None,
            svc_info: Default::default(),
            selected: None,
            mon_tab: 0,
            mon_filter: None,
            sel_pid: None,
            mon_target: None,
            svc_target: None,
            tool_logs: std::collections::HashMap::new(),
            exe_names: std::collections::HashMap::new(),
            exe_paths: std::collections::HashMap::new(),
            elevated: crate::startup::is_elevated(),
            autostart: crate::startup::is_enabled(),
            replaces_tm: crate::startup::replaces_taskmgr(),
            fg_pid: 0,
            fg_exe: String::new(),
            asked: HashSet::new(),
            last_tick: Instant::now(),
            last_save: Instant::now(),
            capturing_keys: false,
            engine: None,
            rec_start: None,
            hud: Hud::new(),
            cfg,
        };
        app.first_run();
        #[cfg(debug_assertions)]
        for a in std::env::args() {
            if let Some(path) = a.strip_prefix("--test-grab=") {
                let mon = capture::cursor_monitor();
                eprintln!("{:?}", capture::monitor_color(mon));
                let t = Instant::now();
                match capture::grab(mon) {
                    Ok(f) => {
                        eprintln!("{}x{} hdr={} {:?}", f.w, f.h, f.half.is_some(), t.elapsed());
                        eprintln!("{:?}", capture::save_png(path, f.w, f.h, &f.bgra));
                        if let Some(h) = &f.half {
                            eprintln!("{:?}", capture::save_jxr(&format!("{path}.jxr"), f.w, f.h, h));
                        }
                    }
                    Err(e) => eprintln!("erro {e:?}"),
                }
                std::process::exit(0);
            }
            if let Some(path) = a.strip_prefix("--test-rec=") {
                use capture::engine::*;
                let mon = capture::cursor_monitor();
                let codec = match std::env::var("PRISMA_CODEC").as_deref() {
                    Ok("hevc") => capture::mux::Codec::Hevc,
                    Ok("h264") => capture::mux::Codec::H264,
                    _ => capture::mux::Codec::Av1,
                };
                let h = start(Settings {
                    target: Target::Monitor(mon.0 as isize),
                    out_h: 0,
                    fps: 60,
                    codec,
                    cursor: true,
                    sys_audio: true,
                    audio_pid: 0,
                    mic: false,
                    hdr10: std::env::var("PRISMA_HDR10").is_ok(),
                    replay_secs: 3,
                });
                std::thread::sleep(std::time::Duration::from_millis(1500));
                h.send(Cmd::Record(path.to_string()));
                std::thread::sleep(std::time::Duration::from_secs(6));
                h.send(Cmd::SaveReplay(format!("{path}.replay.mp4")));
                std::thread::sleep(std::time::Duration::from_millis(300));
                h.send(Cmd::StopRecord);
                std::thread::sleep(std::time::Duration::from_millis(300));
                let (f, d, b) = (h.status.frames.load(std::sync::atomic::Ordering::Relaxed), h.status.dropped.load(std::sync::atomic::Ordering::Relaxed), h.status.bytes.load(std::sync::atomic::Ordering::Relaxed));
                eprintln!("quadros {f} perdidos {d} bytes {b} falhou {} {}", h.status.failed.load(std::sync::atomic::Ordering::Relaxed), h.info.lock().unwrap());
                h.stop();
                std::process::exit(0);
            }
            if let Some(path) = a.strip_prefix("--selftest=") {
                crate::selftest::run(path, app.shared.clone());
                std::process::exit(0);
            }
            if let Some(path) = a.strip_prefix("--test-uninstall=") {
                crate::selftest::fake_uninstall(path);
                std::process::exit(0);
            }
            if let Some(path) = a.strip_prefix("--test-misc=") {
                crate::selftest::misc(path);
                std::process::exit(0);
            }
            if let Some(path) = a.strip_prefix("--test-tweaks=") {
                crate::selftest::tweaks(path);
                std::process::exit(0);
            }
            if let Some(path) = a.strip_prefix("--test-tools=") {
                crate::selftest::toolrun(path);
                std::process::exit(0);
            }
            if let Some(path) = a.strip_prefix("--test-audit=") {
                crate::selftest::audit(path);
                std::process::exit(0);
            }
            if a == "--test-crash" {
                windows::Win32::System::Diagnostics::Debug::RaiseException(0xE000_0001, 0, None);
            }
            if a == "--test-panic" {
                panic!("teste do relatório");
            }
            if let Some(v) = a.strip_prefix("--test-scan=") {
                let (name, path) = v.split_once('|').unwrap_or((v, "scan.txt"));
                let _ = std::fs::write(path, crate::sys::uninstall::scan_preview(name));
                std::process::exit(0);
            }
            if let Some(v) = a.strip_prefix("--test-force=") {
                let (h, path) = v.split_once('|').unwrap_or((v, "forca.txt"));
                let hwnd = HWND(h.parse::<isize>().unwrap_or(0) as *mut _);
                let text = match procs::force_close(hwnd) {
                    procs::Forced::Killed(n) => format!("encerrou {n}"),
                    procs::Forced::Closed => "fechamento normal".to_string(),
                    procs::Forced::Denied(n) => format!("recusado {n}"),
                    procs::Forced::Nothing => "nada".to_string(),
                };
                let _ = std::fs::write(path, text);
                std::process::exit(0);
            }
            if let Some(v) = a.strip_prefix("--test-route=") {
                let (pid, path) = v.split_once('|').unwrap_or((v, "rota.txt"));
                let pid: u32 = pid.parse().unwrap_or(0);
                let mut out = String::new();
                for (capture, name) in [(false, "saida"), (true, "entrada")] {
                    for d in audio::devices(capture) {
                        let on = audio::set_route(pid, capture, &d.id);
                        let read = audio::route_of(pid, capture).unwrap_or_default();
                        out += &format!("{name} {}: {on} leitura confere {}\n", d.name, read.contains(&d.id));
                    }
                    out += &format!("  padrao: {} vazio {}\n", audio::set_route(pid, capture, ""), audio::route_of(pid, capture).is_some_and(|r| r.is_empty()));
                }
                let _ = std::fs::write(path, out);
                std::process::exit(0);
            }
            if let Some(v) = a.strip_prefix("--test-proc-audio=") {
                let (pid, path) = v.split_once('|').unwrap_or((v, "som.txt"));
                let _ = std::fs::write(path, capture::sound::probe(pid.parse().unwrap_or(0), 4000));
                std::process::exit(0);
            }
            if a == "--twlog" {
                app.cfg.tw_log = true;
                app.tw_run.lock().unwrap().lines = crate::selftest::tweak_log();
            }
            if a == "--profiles" {
                app.page = Page::Tweaks;
                app.tw_tab = 1;
                app.tw_layers = vec!['J', 'D'];
                for t in tweaks::CATALOG {
                    if t.base.chars().any(|c| "JD".contains(c)) {
                        app.tw_on.insert(t.id);
                    }
                }
            }
            if let Some(path) = a.strip_prefix("--test-svc=") {
                let mut out = String::new();
                // Sem fechar nada entre um e outro: cada abertura tem de fechar a anterior.
                for name in ["Agendador de Tarefas", "Adaptador de Desempenho WMI", "Windows Update"] {
                    let t0 = Instant::now();
                    let r = services::open_properties(name);
                    let elapsed = t0.elapsed();
                    let open = services::console_windows();
                    out += &format!("{name}: {r:?} em {elapsed:?}, consoles {}\n", open.len());
                    for f in open {
                        out += &format!("  {:?}\n", services::console_dump(f));
                    }
                }
                services::close_consoles();
                let _ = std::fs::write(path, out);
                std::process::exit(0);
            }
            if let Some(d) = a.strip_prefix("--scan=") {
                app.page = Page::Analyze;
                let c = d.chars().next().unwrap_or('X');
                app.scan_drive = mft::drives().iter().position(|l| *l == c).unwrap_or(0);
                mft::start(c, app.scan_progress.clone(), app.scan.clone());
            }
            if a == "--test-gpu" {
                let mut k = crate::sys::kmt::Kmt::new().expect("kmt");
                k.read();
                for _ in 0..3 {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    let r = k.read();
                    eprintln!("kmt uso {:.1}% vram {} de {} MB", r.util, r.used >> 20, r.total >> 20);
                }
                if let Some(n) = crate::sys::nvml::Nvml::load() {
                    let r = n.read();
                    eprintln!("nvml uso {:.1}% vram {} de {} MB {} C", r.util, r.used >> 20, r.total >> 20, r.temp);
                }
                std::process::exit(0);
            }
            if a == "--test-audio" {
                for s in audio::sessions() {
                    eprintln!("{} pid {} vol {:.2} mudo {} ativo {}", s.exe, s.pid, s.volume, s.muted, s.active);
                }
                if let Some(s) = audio::sessions().into_iter().next() {
                    let v = s.volume;
                    let pid = s.pid;
                    eprintln!("mudança {:?}", audio::change_focused(pid, audio::Change::Step(-5)).map(|r| r.0));
                    eprintln!("lido {:?}", audio::for_process(pid).first().map(|x| x.volume));
                    for x in audio::for_process(pid) {
                        x.set_volume(v);
                    }
                    eprintln!("restaurado {:?}", audio::for_process(pid).first().map(|x| x.volume));
                }
                std::process::exit(0);
            }
            if let Some(path) = a.strip_prefix("--test-overlay=") {
                let mon = capture::cursor_monitor();
                if let Ok(f) = capture::grab(mon) {
                    capture::shot::open(f, mon, String::new(), false);
                    capture::shot::debug_setup(path);
                }
            }
            app.page = match a.as_str() {
                "--page=storage" => Page::Storage,
                "--page=boot" => Page::Startup,
                "--page=tweaks" => Page::Tweaks,
                "--page=tools" => Page::Tools,
                "--page=uninstall" => Page::Uninstall,
                "--page=analyze" => Page::Analyze,
                "--page=services" => Page::Services,
                "--page=optimize" => Page::Optimize,
                "--page=energy" => Page::Energy,
                "--page=full" => Page::Fullscreen,
                "--page=volume" => Page::Volume,
                "--page=capture" => Page::Capture,
                "--page=widget" => Page::Widget,
                "--page=adjust" => Page::Adjust,
                _ => app.page,
            };
        }
        app.refresh_plans();
        app.register_hotkeys();
        app.widget.set_visible(app.cfg.widget_on);
        tray_add(hub, true);
        let open = !background && !std::env::args().any(|a| a.starts_with("--test-"));
        APP.with(|a| *a.borrow_mut() = Some(app));
        if open {
            let _ = PostMessageW(Some(hub), WM_SHOW, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn shutdown() {
    with(|a| {
        if a.rec_start.is_some() {
            a.toggle_record();
        }
        a.stop_engine();
        a.cfg.save();
        a.usage.save();
        tray_remove(a.hub);
    });
}

unsafe fn tray_add(hub: HWND, add: bool) {
    let inst = GetModuleHandleW(None).unwrap_or_default();
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hub,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: LoadIconW(Some(inst.into()), PCWSTR(1 as *const u16)).unwrap_or_default(),
        ..Default::default()
    };
    let tip: Vec<u16> = "Prisma".encode_utf16().collect();
    nid.szTip[..tip.len()].copy_from_slice(&tip);
    let _ = Shell_NotifyIconW(if add { NIM_ADD } else { NIM_MODIFY }, &nid);
}

fn tray_remove(hub: HWND) {
    let nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hub,
        uID: 1,
        ..Default::default()
    };
    unsafe {
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

impl App {
    /// Na primeira vez, cria os planos do Prisma. O jogo segue no plano que já estava.
    fn first_run(&mut self) {
        if !self.cfg.first_run {
            return;
        }
        self.cfg.first_run = false;
        // O modo portátil não deixa rastros: planos só na versão instalada.
        if self.installed {
            if let (Some(rest), _) = power::ensure_prisma_plans() {
                self.cfg.plan_rest = rest;
            }
        }
        self.cfg.save();
    }

    pub fn refresh_plans(&mut self) {
        self.plans = power::list();
        self.plan_values = self.plans.iter().map(power::values).collect();
        self.active_plan = power::active().unwrap_or_default();
    }

    pub fn register_hotkeys(&mut self) {
        self.unhook_force_key();
        unsafe {
            for id in [HK_FULL, HK_WIDGET, HK_OPT, HK_SHOT, HK_SHOT_FULL, HK_REC, HK_REPLAY, HK_FORCE, HK_VOL_UP, HK_VOL_DOWN, HK_MUTE] {
                let _ = UnregisterHotKey(Some(self.hub), id);
            }
        }
        self.hk_errors.clear();
        let c = &self.cfg;
        let list = [
            (HK_FULL, c.hk_fullscreen, "Fullscreenizer", true),
            (HK_WIDGET, c.hk_widget, "Widget", true),
            (HK_OPT, c.hk_optimize, "Otimizar", true),
            (HK_SHOT, c.hk_shot, "Foto da área", true),
            (HK_SHOT_FULL, c.hk_shot_full, "Foto da tela inteira", true),
            (HK_REC, c.hk_record, "Gravar", true),
            (HK_REPLAY, c.hk_replay, "Replay", true),
            (HK_FORCE, c.hk_force, "Fechar à força", true),
            (HK_VOL_UP, Hotkey::new(c.vol_mods, VK_VOL_UP), "Aumentar volume", false),
            (HK_VOL_DOWN, Hotkey::new(c.vol_mods, VK_VOL_DOWN), "Diminuir volume", false),
            (HK_MUTE, Hotkey::new(c.vol_mods, VK_MUTE), "Mudo", true),
        ];
        let mut errors = Vec::new();
        let mut force_hook = None;
        for (id, hk, name, norepeat) in list {
            if hk.vk == 0 {
                continue;
            }
            // A versão de teste não pega o Alt+F4 do PC: ela roda junto com programas de verdade.
            #[cfg(debug_assertions)]
            if id == HK_FORCE && !std::env::args().any(|a| a.starts_with("--test-force-key=")) {
                continue;
            }
            let mut m = HOT_KEY_MODIFIERS(hk.mods);
            if norepeat {
                m |= MOD_NOREPEAT;
            }
            if unsafe { RegisterHotKey(Some(self.hub), id, m, hk.vk) }.is_err() {
                if id == HK_FORCE {
                    force_hook = Some(hk);
                } else {
                    errors.push(format!("{name}: {} já está em uso", hk.label()));
                }
            }
        }
        self.hk_errors = errors;
        if let Some(hk) = force_hook {
            self.hook_force_key(hk);
        }
    }

    fn pause_hotkeys(&mut self, on: bool) {
        if self.capturing_keys == on {
            return;
        }
        self.capturing_keys = on;
        if on {
            self.unhook_force_key();
            unsafe {
                for id in [HK_FULL, HK_WIDGET, HK_OPT, HK_SHOT, HK_SHOT_FULL, HK_REC, HK_REPLAY, HK_FORCE] {
                    let _ = UnregisterHotKey(Some(self.hub), id);
                }
            }
        } else {
            self.register_hotkeys();
        }
    }

    fn fg_info(&self) -> Option<(HWND, u32, String)> {
        unsafe {
            let h = GetForegroundWindow();
            if h.is_invalid() {
                return None;
            }
            let pid = procs::window_pid(h);
            let exe = procs::image_path(pid).map(|p| procs::exe_name(&p))?;
            Some((h, pid, exe))
        }
    }

    fn on_hotkey(&mut self, id: i32) {
        match id {
            HK_FULL => self.toggle_fullscreen(),
            HK_WIDGET => self.toggle_widget(),
            HK_OPT => self.optimize_prompt(true),
            HK_SHOT => self.take_shot(),
            HK_SHOT_FULL => self.take_full_shot(),
            HK_REC => self.toggle_record(),
            HK_REPLAY => self.save_replay(),
            HK_FORCE => self.force_close(),
            HK_VOL_UP => self.volume(audio::Change::Step(self.cfg.vol_step as i32)),
            HK_VOL_DOWN => self.volume(audio::Change::Step(-(self.cfg.vol_step as i32))),
            HK_MUTE => self.volume(audio::Change::ToggleMute),
            _ => {}
        }
    }

    /// Outro programa já registrou a combinação: o gancho recebe a tecla antes de todos.
    fn hook_force_key(&mut self, hk: Hotkey) {
        FORCE_VK.store(hk.vk, Ordering::Relaxed);
        FORCE_MODS.store(hk.mods, Ordering::Relaxed);
        FORCE_HUB.store(self.hub.0 as isize, Ordering::Relaxed);
        if self.force_hook.is_none() {
            let inst = unsafe { GetModuleHandleW(None) }.unwrap_or_default();
            self.force_hook = unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_hook), Some(inst.into()), 0) }.ok();
        }
    }

    fn unhook_force_key(&mut self) {
        if let Some(h) = self.force_hook.take() {
            unsafe {
                let _ = UnhookWindowsHookEx(h);
            }
        }
    }

    fn force_close(&mut self) {
        let h = unsafe { GetForegroundWindow() };
        // No teste, só a janela descartável pode ser fechada.
        #[cfg(debug_assertions)]
        {
            let allowed = std::env::args().find_map(|a| a.strip_prefix("--test-force-key=").and_then(|p| p.parse::<u32>().ok()));
            if allowed != Some(procs::window_pid(h)) {
                return;
            }
        }
        match procs::force_close(h) {
            procs::Forced::Killed(name) => {
                self.log(&format!("fechou à força {name}"));
                self.toast.info(&format!("{name} fechado à força"));
            }
            procs::Forced::Denied(name) => {
                let hint = if self.elevated { "O Windows protegeu {}" } else { "Precisa do Prisma como administrador" };
                self.toast.info(&hint.replace("{}", &name));
            }
            _ => {}
        }
    }

    /// Pede uma ação de um botão pela fila, fora do desenho da tela.
    pub fn post_action(&self, id: i32) {
        unsafe {
            let _ = PostMessageW(Some(self.hub), WM_HOTKEY, WPARAM(id as usize), LPARAM(0));
        }
    }

    pub fn pick_folder(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hub), WM_PICK, WPARAM(0), LPARAM(0));
        }
    }

    pub fn is_recording(&self) -> bool {
        self.rec_start.is_some()
    }

    fn capture_settings(&self, replay: bool) -> Settings {
        let fg = unsafe { GetForegroundWindow() };
        let mon = unsafe { MonitorFromWindow(fg, MONITOR_DEFAULTTOPRIMARY) };
        let target = match self.cfg.cap_target {
            1 if !fg.is_invalid() => Target::Window(fg.0 as isize),
            _ => Target::Monitor(mon.0 as isize),
        };
        let (out_h, fps) = match self.cfg.cap_mode {
            1 => (2160, 60),
            2 => (1440, 60),
            3 => (1080, 60),
            _ => (0, native_fps(mon)),
        };
        Settings {
            target,
            out_h,
            fps,
            codec: match self.cfg.cap_codec {
                1 => Codec::Hevc,
                2 => Codec::H264,
                _ => Codec::Av1,
            },
            cursor: self.cfg.cap_cursor,
            sys_audio: self.cfg.cap_sys_audio,
            // Sem o som do sistema, a janela capturada grava só o som dela.
            audio_pid: if !self.cfg.cap_sys_audio && self.cfg.cap_target == 1 && !fg.is_invalid() { procs::window_pid(fg) } else { 0 },
            mic: self.cfg.cap_mic,
            hdr10: self.cfg.cap_hdr,
            replay_secs: if replay { self.cfg.replay_secs } else { 0 },
        }
    }

    fn start_engine(&mut self, replay: bool) {
        if self.engine.is_none() {
            let s = self.capture_settings(replay);
            self.engine = Some(engine::start(s));
        }
    }

    fn stop_engine(&mut self) {
        if let Some(e) = self.engine.take() {
            e.stop();
        }
    }

    fn end_record_ui(&mut self) {
        self.rec_start = None;
        self.hud.hide();
        unsafe {
            let _ = KillTimer(Some(self.hub), TIMER_HUD);
        }
    }

    fn toggle_record(&mut self) {
        if self.rec_start.is_some() {
            if let Some(e) = &self.engine {
                e.send(Cmd::StopRecord);
            }
            self.end_record_ui();
            if !(self.cfg.replay_on && self.detector.mode == Mode::Game) {
                self.stop_engine();
            }
            self.toast.info(&format!("Gravação salva em {}", self.cfg.cap_dir));
            return;
        }
        self.start_engine(false);
        let path = capture::new_path(&self.cfg.cap_dir, "mp4");
        if let Some(e) = &self.engine {
            e.send(Cmd::Record(path));
        }
        self.rec_start = Some(Instant::now());
        self.hud_update();
        unsafe {
            SetTimer(Some(self.hub), TIMER_HUD, 1000, None);
        }
    }

    fn save_replay(&mut self) {
        if !self.cfg.replay_on {
            self.toast.info("O replay está desligado na tela Captura");
            return;
        }
        match &self.engine {
            Some(e) => {
                e.send(Cmd::SaveReplay(capture::new_path(&self.cfg.cap_dir, "mp4")));
                self.toast.info(&format!("Últimos {} s salvos", self.cfg.replay_secs));
            }
            None => self.toast.info("O replay só grava em modo jogo"),
        }
    }

    fn hud_update(&mut self) {
        let Some(t) = self.rec_start else { return };
        let Some(e) = &self.engine else { return };
        let dead = !e.status.running.load(Ordering::Relaxed) && t.elapsed().as_secs() > 2;
        if e.status.failed.load(Ordering::Relaxed) || dead {
            self.end_record_ui();
            self.stop_engine();
            self.toast.info("A gravação parou por um erro");
            return;
        }
        let info = e.info.lock().unwrap().clone();
        let bytes = e.status.bytes.load(Ordering::Relaxed);
        self.hud.update(t.elapsed().as_secs(), &info, bytes);
    }

    pub fn hud_stop(&mut self) {
        if self.rec_start.is_some() {
            self.toggle_record();
        }
    }

    /// Caminho do executável, lembrado por nome para não abrir o processo a cada quadro.
    pub fn path_of(&mut self, exe: &str, pid: u32) -> Option<String> {
        self.exe_paths.entry(exe.to_ascii_lowercase()).or_insert_with(|| procs::image_path(pid)).clone()
    }

    /// Caminho de um programa da lista, se ele estiver aberto ou já tiver sido visto.
    pub fn path_by_name(&mut self, exe: &str) -> String {
        let pid = self.snap.groups.iter().find(|g| g.exe.eq_ignore_ascii_case(exe)).and_then(|g| g.pids.first().copied());
        match pid {
            Some(pid) => self.path_of(exe, pid).unwrap_or_default(),
            None => self.exe_paths.get(&exe.to_ascii_lowercase()).cloned().flatten().unwrap_or_default(),
        }
    }

    fn load_disks(&mut self) {
        if self.disks.is_empty() {
            self.disks = disks::list();
            self.nets = nets::list();
            if self.health.lock().unwrap().is_empty() {
                disks::spawn_health(&self.disks, self.health.clone());
            }
        }
    }

    pub fn go(&mut self, page: Page) {
        self.page = page;
        self.refresh_page();
        self.invalidate();
    }

    /// Letras em discos com nota abaixo de Bom.
    pub fn suspect_letters(&self) -> Vec<char> {
        let h = self.health.lock().unwrap();
        let bad = |n: u32| h.get(&n).is_some_and(|v| matches!(v.level, disks::Level::Warn | disks::Level::Risk));
        self.disks.iter().filter(|d| bad(d.number)).flat_map(|d| d.volumes.iter().map(|v| v.letter)).collect()
    }

    /// Roda em segundo plano e espera a saúde dos discos para não dar falso "tudo certo".
    pub fn start_audit(&mut self) {
        *self.audit.lock().unwrap() = None;
        let list: Vec<(u32, String, Vec<char>)> =
            self.disks.iter().map(|d| (d.number, d.name.clone(), d.volumes.iter().map(|v| v.letter).collect())).collect();
        let (slot, health) = (self.audit.clone(), self.health.clone());
        let _ = std::thread::Builder::new().name("auditoria".into()).spawn(move || {
            for _ in 0..120 {
                if health.lock().unwrap().len() >= list.len() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(250));
            }
            let h: Vec<(u32, disks::Health)> = health.lock().unwrap().iter().map(|(k, v)| (*k, v.clone())).collect();
            *slot.lock().unwrap() = Some(audit::run(&list, &h));
        });
    }

    pub fn tw_undo(&mut self, id: &str) {
        let rep = tweaks::undo(id, &mut self.tw_journal);
        {
            let mut run = self.tw_run.lock().unwrap();
            let what = if id.is_empty() { "Desfazendo tudo" } else { "Desfazendo" };
            run.lines.push(format!("▸ {}  {what}", crate::sys::tools::clock()));
            run.lines.extend(rep.lines.iter().cloned());
        }
        self.log(&format!("desfez ajustes {id}: {} restaurados, {} intocados", rep.restored, rep.skipped.len()));
        let msg = if !rep.apps.is_empty() {
            format!("{} valores restaurados. Apps voltam pela Loja.", rep.restored)
        } else if rep.skipped.is_empty() {
            format!("{} valores restaurados", rep.restored)
        } else {
            format!("{} restaurados, {} mudaram depois e ficaram como estão", rep.restored, rep.skipped.len())
        };
        self.toast.info(&msg);
    }

    pub fn pick_tool_folder(&self, which: usize) {
        unsafe {
            let _ = PostMessageW(Some(self.hub), WM_PICK, WPARAM(which), LPARAM(0));
        }
    }

    /// Nome amigável gravado no executável; sem ele, o nome do arquivo.
    pub fn name_of(&mut self, exe: &str, pid: u32) -> String {
        let key = exe.to_ascii_lowercase();
        if let Some(n) = self.exe_names.get(&key) {
            return n.clone();
        }
        let n = self
            .path_of(exe, pid)
            .and_then(|p| fileinfo::description(&p))
            .unwrap_or_else(|| exe.trim_end_matches(".exe").trim_end_matches(".EXE").to_string());
        self.exe_names.insert(key, n.clone());
        n
    }

    /// Procura uma vez logo depois de abrir e depois a cada dia.
    fn update_tick(&mut self) {
        use crate::sys::update::{self, Phase};
        if !self.cfg.update_auto {
            return;
        }
        let due = match self.upd_last {
            Some(t) => t.elapsed().as_secs() > 86_400,
            None => self.started.elapsed().as_secs() > 8,
        };
        let idle = matches!(self.upd.lock().unwrap().phase, Phase::Idle | Phase::Latest | Phase::Failed(_));
        if due && idle {
            self.upd_last = Some(Instant::now());
            update::check(self.upd.clone());
        }
        let s = self.upd.lock().unwrap().clone();
        if s.phase == Phase::Available && !self.upd_told {
            self.upd_told = true;
            self.toast.info(&format!("Nova versão {} disponível em Configurações", s.version));
        }
    }

    /// Cada ferramenta guarda o próprio passo a passo.
    pub fn tool_log(&mut self, i: usize) -> tools::Shared {
        self.tool_logs.entry(i).or_default().clone()
    }

    pub fn tool_busy(&self) -> bool {
        self.tool_logs.values().any(|t| t.lock().unwrap().running)
    }

    pub fn update_now(&mut self) {
        let busy = self.tw_run.lock().unwrap().running || self.tool_busy() || self.un_job.lock().unwrap().running || self.rec_start.is_some();
        if busy {
            self.toast.info("Espere a tarefa em andamento terminar.");
        } else {
            crate::sys::update::install(self.upd.clone(), self.hub.0 as isize, WM_UPDATED);
        }
    }

    pub fn sort_boot(&mut self, by: u8) {
        self.boot_sort = by;
        self.boot_sel = None;
        match by {
            1 => self.boot_items.sort_by_key(|i| (!i.enabled, i.name.to_lowercase())),
            2 => self.boot_items.sort_by_key(|i| (i.enabled, i.name.to_lowercase())),
            _ => self.boot_items.sort_by_key(|i| i.name.to_lowercase()),
        }
    }

    pub fn sort_programs(&mut self, by: u8) {
        self.un_sort = by;
        match by {
            1 => self.programs.sort_by_key(|p| (std::cmp::Reverse(p.size), p.name.to_lowercase())),
            // A data vem como AAAAMMDD: comparar o texto já dá a ordem certa.
            2 => self.programs.sort_by_key(|p| (std::cmp::Reverse(p.date.clone()), p.name.to_lowercase())),
            _ => self.programs.sort_by_key(|p| p.name.to_lowercase()),
        }
    }

    pub fn select_boot(&mut self, i: usize) {
        self.boot_sel = Some(i);
        self.boot_sig = self.boot_items.get(i).map(|it| fileinfo::check(&it.path).label());
    }

    pub fn select_service(&mut self, name: String) {
        self.svc_info = services::details(&name);
        self.svc_sel = Some(name);
    }

    /// A cópia nova abre sozinha; esta precisa sair para liberar o nome.
    /// Se a pessoa recusar a permissão do Windows, nada muda e o programa continua aberto.
    pub fn install_now(&mut self) {
        if crate::startup::run_elevated(&format!("--install={}", std::process::id())) {
            unsafe { PostQuitMessage(0) };
        } else {
            self.toast.info("A instalação precisa da permissão de administrador");
        }
    }

    /// Liga ou desliga Ctrl+Shift+Esc. Sem administrador, uma cópia elevada faz a troca.
    pub fn set_taskmgr(&mut self, on: bool) {
        if self.elevated {
            if crate::startup::set_replace_taskmgr(on) {
                self.replaces_tm = on;
                self.log(if on { "passou a substituir o Gerenciador de Tarefas" } else { "devolveu o Gerenciador de Tarefas" });
            }
        } else if crate::startup::run_elevated(&format!("--set-taskmgr={}", on as u8)) {
            self.log(if on { "passou a substituir o Gerenciador de Tarefas" } else { "devolveu o Gerenciador de Tarefas" });
            self.tm_check = Some(Instant::now());
        }
    }

    pub fn pick_exe(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hub), WM_PICK, WPARAM(1), LPARAM(0));
        }
    }

    /// Registro simples de tudo que o Prisma mudou no sistema, para consultar e desfazer.
    pub fn log(&self, line: &str) {
        use std::io::Write;
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        let path = crate::config::data_dir().join("acoes.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {line}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
        }
    }

    fn take_shot(&mut self) {
        let mon = capture::cursor_monitor();
        match capture::grab(mon) {
            Ok(f) => capture::shot::open(f, mon, self.cfg.cap_dir.clone(), self.cfg.shot_hdr),
            Err(_) => self.toast.info("Não consegui capturar a tela"),
        }
    }

    fn take_full_shot(&mut self) {
        let Ok(f) = capture::grab(capture::cursor_monitor()) else {
            self.toast.info("Não consegui capturar a tela");
            return;
        };
        let path = capture::new_path(&self.cfg.cap_dir, "png");
        let mut ok = capture::save_png(&path, f.w, f.h, &f.bgra).is_ok();
        if let (true, Some(half)) = (self.cfg.shot_hdr, &f.half) {
            ok &= capture::save_jxr(&capture::new_path(&self.cfg.cap_dir, "jxr"), f.w, f.h, half).is_ok();
        }
        capture::copy_to_clipboard(self.hub, f.w, f.h, &f.bgra);
        self.toast.info(if ok { "Foto salva e copiada" } else { "Não consegui salvar a foto" });
    }

    fn volume(&mut self, ch: audio::Change) {
        let Some((_, pid, exe)) = self.fg_info() else { return };
        let name = exe.trim_end_matches(".exe").to_string();
        match audio::change_focused(pid, ch) {
            Some((vol, muted)) => {
                if self.cfg.vol_osd {
                    self.toast.volume(&name, vol, muted);
                }
                if self.detector.game_pid == pid && !exe.is_empty() {
                    self.cfg.profile_mut(&exe).volume = (vol * 100.0).round() as i32;
                    self.cfg.save();
                }
            }
            None => self.toast.info(&format!("{name} não tem som tocando")),
        }
    }

    fn toggle_fullscreen(&mut self) {
        let Some((h, _, exe)) = self.fg_info() else { return };
        if let Some(on) = self.full.toggle(h) {
            let msg = format!("{} · {} desfaz", if on { "Sem bordas" } else { "Janela restaurada" }, self.cfg.hk_fullscreen.label());
            self.toast.info(&msg);
            if on {
                self.note_fullscreen(h, &exe);
            }
        }
    }

    /// Quem usou o Fullscreenizer uma vez entra na lista de programas salvos.
    fn note_fullscreen(&mut self, h: HWND, exe: &str) {
        let path = procs::image_path(procs::window_pid(h)).unwrap_or_default();
        self.cfg.fs_note(exe, &path);
        self.cfg.save();
    }

    pub fn toggle_window(&mut self, h: HWND, exe: &str) {
        if self.full.toggle(h) == Some(true) {
            self.note_fullscreen(h, exe);
        }
    }

    /// Programas com rota escolhida recebem ela assim que abrem.
    fn sound_tick(&mut self) {
        if !self.cfg.snd_apps.iter().any(|a| !a.output.is_empty() || !a.input.is_empty()) {
            return;
        }
        if self.snd_seen.len() > 4096 {
            self.snd_seen.clear();
        }
        // O Windows só aceita depois que o processo abre o áudio: quem falhou tenta de novo.
        for g in &self.snap.groups {
            let Some(a) = self.cfg.snd_apps.iter().find(|a| a.exe.eq_ignore_ascii_case(&g.exe)) else { continue };
            for pid in &g.pids {
                if !self.snd_seen.contains(pid) && apply_route(*pid, a) {
                    self.snd_seen.insert(*pid);
                }
            }
        }
    }

    /// Aplica agora a rota em tudo que está aberto desse programa.
    pub fn route_app(&mut self, exe: &str) {
        let Some(a) = self.cfg.snd_apps.iter().find(|a| a.exe.eq_ignore_ascii_case(exe)).cloned() else { return };
        for g in self.snap.groups.iter().filter(|g| g.exe.eq_ignore_ascii_case(exe)) {
            for pid in &g.pids {
                if apply_route(*pid, &a) {
                    self.snd_seen.insert(*pid);
                } else {
                    self.snd_seen.remove(pid);
                }
            }
        }
    }

    pub fn pick_sound_app(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hub), WM_PICK_SND, WPARAM(0), LPARAM(0));
        }
    }

    pub fn pick_fullscreen_app(&self) {
        unsafe {
            let _ = PostMessageW(Some(self.hub), WM_PICK_FS, WPARAM(0), LPARAM(0));
        }
    }

    /// Volta as opções de uma tela ao padrão e aplica na hora.
    pub fn reset_defaults(&mut self, page: Defaults) {
        self.cfg.reset(page);
        self.cfg.save();
        match page {
            Defaults::Capture => {
                self.register_hotkeys();
                self.replay_changed();
            }
            Defaults::Widget => {
                let on = self.cfg.widget_on;
                self.set_widget(on);
                self.widget.update(&self.cfg, &self.snap);
                self.register_hotkeys();
            }
            Defaults::Settings => {
                crate::i18n::set_english(self.cfg.lang_en);
                self.register_hotkeys();
            }
            Defaults::Fullscreen => self.register_hotkeys(),
            _ => {}
        }
        self.toast.info("As opções desta tela voltaram ao padrão");
    }

    pub fn toggle_widget(&mut self) {
        let on = !self.widget.visible;
        self.set_widget(on);
    }

    pub fn set_widget(&mut self, on: bool) {
        self.cfg.widget_on = on;
        self.widget.set_visible(on);
        self.widget.update(&self.cfg, &self.snap);
        self.cfg.save();
    }

    pub fn refresh_candidates(&mut self) {
        let fg = self.fg_pid;
        self.cands = optimizer::candidates(&self.cfg, &self.usage, &self.snap.groups, fg, self.detector.game_pid);
        for c in &mut self.cands {
            if self.unchecked.contains(&c.exe) {
                c.on = false;
            } else if self.checked.contains(&c.exe) {
                c.on = true;
            }
        }
    }

    pub fn close_selected(&mut self) {
        self.refresh_candidates();
        let now = Instant::now();
        let picked: Vec<usize> = self.cands.iter().enumerate().filter(|(_, c)| c.on).map(|(i, _)| i).collect();
        for i in picked {
            let c = &self.cands[i];
            if optimizer::close(c) {
                self.pending.push((c.exe.clone(), c.pids.clone(), now));
            } else {
                self.stubborn.push((c.exe.clone(), c.pids.clone()));
            }
        }
    }

    pub fn force_stubborn(&mut self) {
        for (_, pids) in self.stubborn.drain(..) {
            optimizer::force(&pids);
        }
    }

    /// Mostra a pergunta de otimização. `manual` mostra mesmo sem nada a fechar.
    fn optimize_prompt(&mut self, manual: bool) {
        self.refresh_candidates();
        let selected: Vec<&Candidate> = self.cands.iter().filter(|c| c.on).collect();
        if selected.is_empty() {
            if manual {
                self.toast.info("Nada para fechar agora");
            }
            return;
        }
        let ram: u64 = selected.iter().map(|c| c.ram).sum();
        let gb = format!("{:.1}", ram as f64 / (1u64 << 30) as f64).replace('.', crate::i18n::comma());
        let n = selected.len();
        let title = if self.detector.game_exe.is_empty() {
            "Otimizar".to_string()
        } else {
            format!("{} detectado", self.detector.game_exe.trim_end_matches(".exe"))
        };
        let plural = if n == 1 { "programa" } else { "programas" };
        self.prompt.ask(title, format!("Fechar {n} {plural} para liberar {gb} GB de RAM?"));
    }

    pub fn prompt_mouse(&mut self, msg: u32, x: f32, y: f32) {
        match self.prompt.mouse(msg, x, y) {
            Some(PromptAnswer::Close) => {
                self.prompt.hide();
                self.close_selected();
            }
            Some(PromptAnswer::List) => {
                self.prompt.hide();
                self.page = Page::Optimize;
                self.show_main();
            }
            Some(PromptAnswer::Dismiss) => self.prompt.hide(),
            None => {}
        }
    }

    pub fn prompt_hide(&mut self) {
        self.prompt.hide();
    }

    fn apply_plan(&mut self, id: &str) {
        if !id.is_empty() && self.active_plan != id && power::set_active(id) {
            self.active_plan = id.to_string();
        }
    }

    fn on_tick(&mut self) {
        self.snap = self.shared.lock().unwrap().clone();
        let secs = self.last_tick.elapsed().as_secs().max(1);
        self.last_tick = Instant::now();

        if let Some((h, pid, exe)) = self.fg_info() {
            if pid != self.fg_pid {
                self.fg_pid = pid;
                self.fg_exe = exe.clone();
                self.apply_profile(h, &exe);
            }
        }
        let fg_exe = self.fg_exe.clone();
        self.usage.note(&self.snap.groups, &fg_exe, secs);

        let ev = self.detector.tick(&self.cfg, self.snap.cpu, self.snap.gpu.util);
        match ev {
            Event::Enter { game } => {
                if game.is_some() && self.cfg.replay_on {
                    self.start_engine(true);
                }
                if self.cfg.power_auto {
                    let mut plan = self.cfg.plan_game.clone();
                    if let Some(p) = game.as_ref().and_then(|g| self.cfg.profile(g)) {
                        if !p.plan.is_empty() {
                            plan = p.plan.clone();
                        }
                    }
                    self.apply_plan(&plan);
                }
                if let Some(g) = game {
                    if self.cfg.on_game != 2 && self.asked.insert(g.clone()) {
                        if self.cfg.on_game == 1 {
                            self.close_selected();
                        } else {
                            self.optimize_prompt(false);
                        }
                    }
                }
            }
            Event::Leave => {
                if self.rec_start.is_none() {
                    self.stop_engine();
                }
                if self.cfg.power_auto {
                    let plan = self.cfg.plan_rest.clone();
                    self.apply_plan(&plan);
                }
            }
            Event::None => {}
        }

        let now = Instant::now();
        let mut done = Vec::new();
        for (i, (exe, pids, t)) in self.pending.iter().enumerate() {
            if now.duration_since(*t).as_secs() >= 3 {
                let alive = self.snap.groups.iter().any(|g| g.pids.iter().any(|p| pids.contains(p)));
                if alive {
                    self.stubborn.push((exe.clone(), pids.clone()));
                }
                done.push(i);
            }
        }
        for i in done.into_iter().rev() {
            self.pending.remove(i);
        }

        self.widget.update(&self.cfg, &self.snap);
        self.full.sweep();
        self.update_tick();
        self.sound_tick();
        if let Some(t) = self.tm_check {
            self.replaces_tm = crate::startup::replaces_taskmgr();
            if t.elapsed().as_secs() > 6 {
                self.tm_check = None;
            }
        }
        if self.last_save.elapsed().as_secs() > 300 {
            self.last_save = Instant::now();
            self.usage.save();
        }
        if self.main.is_some() {
            self.refresh_page();
            self.invalidate();
        }
    }

    pub fn replay_changed(&mut self) {
        if !self.cfg.replay_on && self.rec_start.is_none() {
            self.stop_engine();
        }
        if self.cfg.replay_on && self.detector.mode == Mode::Game {
            self.start_engine(true);
        }
    }

    fn apply_profile(&mut self, h: HWND, exe: &str) {
        if self.cfg.fs_auto(exe) && !self.full.is_on(h) {
            self.full.apply(h);
        }
        let Some(p) = self.cfg.profile(exe).cloned() else { return };
        if p.volume >= 0 {
            for s in audio::for_process(self.fg_pid) {
                s.set_volume(p.volume as f32 / 100.0);
            }
        }
        match p.widget {
            1 if !self.widget.visible => self.widget.set_visible(true),
            2 if self.widget.visible => self.widget.set_visible(false),
            _ => {}
        }
    }

    pub fn refresh_page(&mut self) {
        match self.page {
            Page::Volume => self.sessions = audio::sessions(),
            Page::Optimize => self.refresh_candidates(),
            Page::Fullscreen => self.refresh_windows(),
            Page::Energy => self.active_plan = power::active().unwrap_or_default(),
            Page::Tweaks => {
                self.load_disks();
                if self.audit.lock().unwrap().is_none() {
                    self.start_audit();
                }
                if self.tw_apps.is_empty() {
                    if let Some(v) = self.tw_apps_slot.lock().unwrap().take() {
                        self.tw_apps = v;
                    } else {
                        let slot = self.tw_apps_slot.clone();
                        let _ = std::thread::Builder::new().name("apps".into()).spawn(move || {
                            *slot.lock().unwrap() = Some(tweaks::installed_apps());
                        });
                    }
                }
                self.tw_journal = tweaks::load_journal();
            }
            Page::Tools => {
                self.load_disks();
            }
            Page::Uninstall => {
                if self.programs.is_empty() {
                    self.programs = uninstall::list();
                    self.sort_programs(self.un_sort);
                }
            }
            Page::Startup => {
                if self.boot_items.is_empty() {
                    self.boot_items = autostart::list();
                    self.sort_boot(self.boot_sort);
                }
            }
            Page::Services => self.services = services::list(),
            Page::Storage => {
                self.load_disks();
                disks::sample(&mut self.disks);
                nets::sample(&mut self.nets);
            }
            _ => {}
        }
        let fast = matches!(self.page, Page::Storage | Page::Tools | Page::Tweaks)
            || self.scan_progress.running.load(Ordering::Relaxed)
            || self.un_job.lock().unwrap().running;
        monitor::FAST.store(fast && self.main.is_some(), Ordering::Relaxed);
        let detail = matches!(self.page, Page::Monitor | Page::Optimize);
        monitor::DETAIL.store(detail && self.main.is_some(), Ordering::Relaxed);
    }

    fn refresh_windows(&mut self) {
        let own = self.main.as_ref().map(|m| m.hwnd);
        self.windows = procs::top_windows()
            .into_iter()
            .filter(|h| Some(*h) != own)
            .map(|h| {
                let path = procs::image_path(procs::window_pid(h)).unwrap_or_default();
                let exe = Some(path.clone()).filter(|p| !p.is_empty()).map(|p| procs::exe_name(&p)).unwrap_or_default();
                let mut r = RECT::default();
                unsafe {
                    let _ = GetClientRect(h, &mut r);
                }
                WinInfo { hwnd: h, title: procs::window_title(h), exe, path, size: (r.right, r.bottom) }
            })
            .collect();
    }

    pub fn show_main(&mut self) {
        if let Some(m) = &self.main {
            unsafe {
                let _ = ShowWindow(m.hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(m.hwnd);
            }
            return;
        }
        unsafe {
            let inst = GetModuleHandleW(None).unwrap_or_default();
            let h = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("PrismaMain"),
                w!("Prisma"),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                100,
                100,
                None,
                None,
                Some(inst.into()),
                None,
            )
            .expect("janela principal");
            let dark: BOOL = true.into();
            let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, 4);
            let cap: u32 = 0x00221d1b;
            let _ = DwmSetWindowAttribute(h, DWMWA_CAPTION_COLOR, &cap as *const _ as _, 4);
            let dpi = GetDpiForWindow(h);
            let scale = dpi as f32 / 96.0;
            let mut r = RECT { left: 0, top: 0, right: (1280.0 * scale) as i32, bottom: (820.0 * scale) as i32 };
            let _ = AdjustWindowRectExForDpi(&mut r, WS_OVERLAPPEDWINDOW, false, WINDOW_EX_STYLE(0), dpi);
            let _ = SetWindowPos(
                h,
                None,
                0,
                0,
                r.right - r.left,
                r.bottom - r.top,
                SWP_NOMOVE | SWP_NOZORDER,
            );
            let mut c = RECT::default();
            let _ = GetClientRect(h, &mut c);
            let rt = hwnd_target(h, c.right as u32, c.bottom as u32, dpi as f32).expect("alvo de desenho");
            let base: ID2D1RenderTarget = rt.cast().unwrap();
            self.main = Some(MainWin {
                hwnd: h,
                rt,
                canvas: Canvas::new(base),
                ui: UiState::default(),
                scale,
                w: c.right as f32 / scale,
                h: c.bottom as f32 / scale,
            });
            self.refresh_plans();
            self.refresh_page();
            let _ = ShowWindow(h, SW_SHOW);
            let _ = SetForegroundWindow(h);
        }
    }

    pub fn invalidate(&self) {
        if let Some(m) = &self.main {
            unsafe {
                let _ = InvalidateRect(Some(m.hwnd), None, false);
            }
        }
    }

    fn paint(&mut self) {
        let Some(mut m) = self.main.take() else { return };
        let (w, h) = (m.w, m.h);
        let mut lost = false;
        unsafe {
            m.rt.BeginDraw();
            m.ui.view_h = h;
            m.ui.view_w = w;
            let ui = crate::ui::Ui::begin(&m.canvas, &mut m.ui);
            m.canvas.clear(BG);
            pages::draw(ui, self, rect(0.0, 0.0, w, h));
            if m.rt.EndDraw(None, None).is_err() {
                lost = true;
            }
        }
        if lost {
            let dpi = 96.0 * m.scale;
            let px = D2D_SIZE_U { width: (m.w * m.scale) as u32, height: (m.h * m.scale) as u32 };
            if let Ok(rt) = hwnd_target(m.hwnd, px.width, px.height, dpi) {
                let base: ID2D1RenderTarget = rt.cast().unwrap();
                m.canvas = Canvas::new(base);
                m.rt = rt;
            }
        }
        self.pause_hotkeys(m.ui.capturing != 0);
        let again = std::mem::take(&mut m.ui.needs_redraw);
        let hwnd = m.hwnd;
        self.main = Some(m);
        if again {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
        }
    }

    fn on_key(&mut self, vk: u32) {
        let Some(m) = &mut self.main else { return };
        if vk == 0x1B && m.ui.close_ask() {
            self.invalidate();
            return;
        }
        if m.ui.capturing == 0 {
            return;
        }
        let down = |k: VIRTUAL_KEY| unsafe { GetAsyncKeyState(k.0 as i32) as u16 & 0x8000 != 0 };
        match vk {
            0x1B => m.ui.key_cancel = true,
            0x08 => m.ui.key_event = Some(Hotkey::new(0, 0)),
            0x10 | 0x11 | 0x12 | 0x5B | 0x5C => return,
            _ => {
                let mut mods = 0;
                if down(VK_CONTROL) {
                    mods |= MOD_CTRL;
                }
                if down(VK_MENU) {
                    mods |= MOD_ALT;
                }
                if down(VK_SHIFT) {
                    mods |= MOD_SHIFT;
                }
                if down(VK_LWIN) || down(VK_RWIN) {
                    mods |= MOD_WIN;
                }
                m.ui.key_event = Some(Hotkey::new(mods, vk));
            }
        }
        self.invalidate();
    }

    fn tray_menu(&mut self) {
        unsafe {
            let menu = CreatePopupMenu().unwrap();
            let item = |s: &str| HSTRING::from(crate::i18n::tr(s).as_ref());
            let _ = AppendMenuW(menu, MF_STRING, TRAY_OPEN as usize, &item("Abrir o Prisma"));
            let flag = if self.widget.visible { MF_CHECKED } else { MF_UNCHECKED };
            let _ = AppendMenuW(menu, MF_STRING | flag, TRAY_WIDGET as usize, w!("Widget"));
            let _ = AppendMenuW(menu, MF_STRING, TRAY_OPT as usize, &item("Otimizar agora"));
            let _ = AppendMenuW(menu, MF_STRING, TRAY_SHOT as usize, &item("Tirar foto da área"));
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
            let _ = AppendMenuW(menu, MF_STRING, TRAY_EXIT as usize, &item("Sair"));
            let mut p = POINT::default();
            let _ = GetCursorPos(&mut p);
            let _ = SetForegroundWindow(self.hub);
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY, p.x, p.y, None, self.hub, None);
            let _ = DestroyMenu(menu);
            match cmd.0 as u32 {
                TRAY_OPEN => self.show_main(),
                TRAY_WIDGET => self.toggle_widget(),
                TRAY_OPT => self.optimize_prompt(true),
                TRAY_SHOT => self.on_hotkey(HK_SHOT),
                TRAY_EXIT => PostQuitMessage(0),
                _ => {}
            }
        }
    }
}

static TASKBAR_MSG: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

unsafe extern "system" fn hub_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let tb = *TASKBAR_MSG.get_or_init(|| RegisterWindowMessageW(w!("TaskbarCreated")));
    if m == tb {
        tray_add(h, true);
        return LRESULT(0);
    }
    match m {
        WM_HOTKEY => {
            with(|a| a.on_hotkey(w.0 as i32));
            LRESULT(0)
        }
        WM_TICK => {
            with(|a| a.on_tick());
            LRESULT(0)
        }
        WM_SHOW => {
            with(|a| {
                if w.0 == 1 {
                    a.page = Page::Monitor;
                }
                a.show_main();
            });
            LRESULT(0)
        }
        WM_UPDATED => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        WM_TIMER => {
            with(|a| a.hud_update());
            LRESULT(0)
        }
        WM_PICK_SND => {
            if let Some(path) = capture::choose_exe(h) {
                with(|a| {
                    a.cfg.snd_entry(&procs::exe_name(&path), &path);
                    a.cfg.save();
                    a.invalidate();
                });
            }
            LRESULT(0)
        }
        WM_PICK_FS => {
            if let Some(path) = capture::choose_exe(h) {
                with(|a| {
                    a.cfg.fs_note(&procs::exe_name(&path), &path);
                    a.cfg.save();
                    a.invalidate();
                });
            }
            LRESULT(0)
        }
        WM_PICK if w.0 == 1 => {
            if let Some(path) = capture::choose_exe(h) {
                with(|a| {
                    if autostart::add(&path) {
                        a.log(&format!("inicialização adicionou {path}"));
                        a.boot_items = autostart::list();
                        a.sort_boot(a.boot_sort);
                        a.invalidate();
                    }
                });
            }
            LRESULT(0)
        }
        WM_PICK if w.0 >= 2 => {
            if let Some(dir) = capture::choose_folder(h) {
                with(|a| {
                    if w.0 == 2 {
                        a.tool_src = dir;
                    } else {
                        a.tool_dst = dir;
                    }
                    a.invalidate();
                });
            }
            LRESULT(0)
        }
        WM_PICK => {
            if let Some(dir) = capture::choose_folder(h) {
                with(|a| {
                    a.cfg.cap_dir = dir;
                    a.cfg.save();
                    a.invalidate();
                });
            }
            LRESULT(0)
        }
        WM_TRAY => {
            match l.0 as u32 {
                WM_LBUTTONUP => {
                    with(|a| a.show_main());
                }
                WM_RBUTTONUP => {
                    with(|a| a.tray_menu());
                }
                _ => {}
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, w, l),
    }
}

fn mouse_xy(a: &mut App, l: LPARAM) {
    if let Some(mw) = &mut a.main {
        let (x, y) = ((l.0 & 0xffff) as i16 as f32, ((l.0 >> 16) & 0xffff) as i16 as f32);
        mw.ui.mx = x / mw.scale;
        mw.ui.my = y / mw.scale;
    }
}

unsafe extern "system" fn main_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    let r = with(|a| {
        match m {
            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                BeginPaint(h, &mut ps);
                a.paint();
                let _ = EndPaint(h, &ps);
                Some(0)
            }
            WM_ERASEBKGND => Some(1),
            WM_SIZE => {
                if let Some(mw) = &mut a.main {
                    let (pw, ph) = ((l.0 & 0xffff) as u32, ((l.0 >> 16) & 0xffff) as u32);
                    if pw > 0 && ph > 0 {
                        let _ = mw.rt.Resize(&D2D_SIZE_U { width: pw, height: ph });
                        mw.w = pw as f32 / mw.scale;
                        mw.h = ph as f32 / mw.scale;
                    }
                }
                Some(0)
            }
            WM_DPICHANGED => {
                let dpi = (w.0 & 0xffff) as u32;
                let rc = &*(l.0 as *const RECT);
                if let Some(mw) = &mut a.main {
                    mw.scale = dpi as f32 / 96.0;
                    mw.rt.SetDpi(dpi as f32, dpi as f32);
                }
                let _ = SetWindowPos(
                    h,
                    None,
                    rc.left,
                    rc.top,
                    rc.right - rc.left,
                    rc.bottom - rc.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
                Some(0)
            }
            WM_GETMINMAXINFO => {
                let mi = &mut *(l.0 as *mut MINMAXINFO);
                let s = GetDpiForWindow(h) as f32 / 96.0;
                mi.ptMinTrackSize.x = (1100.0 * s) as i32;
                mi.ptMinTrackSize.y = (700.0 * s) as i32;
                Some(0)
            }
            WM_MOUSEMOVE => {
                mouse_xy(a, l);
                a.invalidate();
                Some(0)
            }
            WM_LBUTTONDOWN => {
                mouse_xy(a, l);
                if let Some(mw) = &mut a.main {
                    mw.ui.down = true;
                    mw.ui.pressed = true;
                }
                SetCapture(h);
                a.invalidate();
                Some(0)
            }
            WM_LBUTTONUP => {
                mouse_xy(a, l);
                if let Some(mw) = &mut a.main {
                    mw.ui.down = false;
                    mw.ui.released = true;
                }
                let _ = ReleaseCapture();
                a.invalidate();
                Some(0)
            }
            WM_RBUTTONUP => {
                mouse_xy(a, l);
                if let Some(mw) = &mut a.main {
                    mw.ui.right_released = true;
                }
                a.invalidate();
                Some(0)
            }
            WM_MOUSEWHEEL => {
                let d = ((w.0 >> 16) & 0xffff) as i16 as f32 / 120.0;
                if let Some(mw) = &mut a.main {
                    mw.ui.wheel += d;
                }
                a.invalidate();
                Some(0)
            }
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                a.on_key(w.0 as u32);
                None
            }
            // O Windows só entrega a tecla Print Screen ao soltar.
            WM_KEYUP | WM_SYSKEYUP if w.0 == 0x2C => {
                a.on_key(0x2C);
                None
            }
            WM_CHAR => {
                if let Some(mw) = &mut a.main {
                    if mw.ui.focus != 0 {
                        match w.0 as u32 {
                            8 => mw.ui.backspace += 1,
                            c if c >= 32 => {
                                if let Some(c) = char::from_u32(c) {
                                    mw.ui.typed.push(c);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                a.invalidate();
                Some(0)
            }
            WM_DESTROY => {
                a.main = None;
                a.sessions.clear();
                a.exe_paths.clear();
                a.disks = Vec::new();
                a.nets = Vec::new();
                a.scan_progress.cancel.store(true, Ordering::Relaxed);
                *a.scan.lock().unwrap() = None;
                a.scan_at = None;
                a.boot_items = Vec::new();
                a.services = Vec::new();
                if !a.un_review {
                    a.programs = Vec::new();
                }
                a.exe_names.clear();
                monitor::FAST.store(false, Ordering::Relaxed);
                monitor::DETAIL.store(false, Ordering::Relaxed);
                a.cfg.save();
                let me = windows::Win32::System::Threading::GetCurrentProcess();
                let _ = windows::Win32::System::Memory::SetProcessWorkingSetSizeEx(me, usize::MAX, usize::MAX, Default::default());
                Some(0)
            }
            _ => None,
        }
    });
    match r.flatten() {
        Some(v) => LRESULT(v),
        None => DefWindowProcW(h, m, w, l),
    }
}


/// Frequência do monitor arredondada: 119 vira 120 e 59 vira 60.
pub fn native_fps(mon: HMONITOR) -> u32 {
    unsafe {
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let _ = GetMonitorInfoW(mon, &mut mi as *mut _ as *mut _);
        let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        let name = PCWSTR(mi.szDevice.as_ptr());
        if EnumDisplaySettingsW(name, ENUM_CURRENT_SETTINGS, &mut dm).as_bool() {
            match dm.dmDisplayFrequency {
                0 | 1 => 60,
                f if (f + 1) % 10 == 0 => f + 1,
                f => f,
            }
        } else {
            60
        }
    }
}
