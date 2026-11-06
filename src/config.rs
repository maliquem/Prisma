//! Configuração em texto simples em %APPDATA%\Prisma\config.ini.

use std::fmt::Write as _;
use std::path::PathBuf;

pub const MOD_ALT: u32 = 1;
pub const MOD_CTRL: u32 = 2;
pub const MOD_SHIFT: u32 = 4;
pub const MOD_WIN: u32 = 8;

/// Jogo da lista do Medidor.
#[derive(Clone, Default, Debug)]
pub struct MeterGame {
    pub exe: String,
    pub path: String,
    /// 0 compacto, 1 completo, 2 nenhum.
    pub view: u8,
    /// 0 desligado, 1 automático, 2 manual.
    pub limit_mode: u8,
    pub limit_fps: u32,
    pub limit_real: bool,
    /// O automático já mediu este jogo.
    pub measured: bool,
    /// 0 segue a regra geral, 1 liga, 2 não liga.
    pub hdr: u8,
    /// 0 do jogo, 1 recomendado, 2 K, 3 L, 4 M.
    pub preset: u8,
    /// Limita de dentro do jogo em vez de pelo driver.
    pub inject: bool,
    /// A injeção caiu, não carregou ou encontrou anti-cheat: fica no driver até a pessoa religar.
    pub inject_off: bool,
}

/// Escolha de Auto HDR de um jogo: 0 Windows decide, 1 força, 2 desliga.
#[derive(Clone, Default, Debug)]
pub struct HdrGame {
    pub exe: String,
    pub path: String,
    pub mode: u8,
}

/// Perfil feito pela calibração do Prisma, um por modo de imagem da TV.
#[derive(Clone, Default, Debug)]
pub struct HdrProfile {
    pub screen: String,
    pub mode: String,
    pub file: String,
    pub peak: f32,
    pub full: f32,
    pub black: f32,
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Hotkey {
    pub mods: u32,
    pub vk: u32,
}

impl Hotkey {
    pub const fn new(mods: u32, vk: u32) -> Self {
        Self { mods, vk }
    }
    fn parse(s: &str) -> Self {
        let mut it = s.split(',');
        let mods = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let vk = it.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        Self { mods, vk }
    }
    pub fn label(&self) -> String {
        if self.vk == 0 {
            return "Nenhum".into();
        }
        let mut s = String::new();
        if self.mods & MOD_CTRL != 0 {
            s.push_str("Ctrl + ");
        }
        if self.mods & MOD_ALT != 0 {
            s.push_str("Alt + ");
        }
        if self.mods & MOD_SHIFT != 0 {
            s.push_str("Shift + ");
        }
        if self.mods & MOD_WIN != 0 {
            s.push_str("Win + ");
        }
        s.push_str(&vk_name(self.vk));
        s
    }
}

pub fn vk_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).unwrap().to_string(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x2C => "Print Screen".into(),
        0x13 => "Pause".into(),
        0x91 => "Scroll Lock".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        0x24 => "Home".into(),
        0x23 => "End".into(),
        0x21 => "Page Up".into(),
        0x22 => "Page Down".into(),
        0x20 => "Espaço".into(),
        0x25 => "←".into(),
        0x26 => "↑".into(),
        0x27 => "→".into(),
        0x28 => "↓".into(),
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        _ => format!("Tecla {vk}"),
    }
}

/// Programa que já usou o Fullscreenizer.
#[derive(Clone, Default, Debug)]
pub struct FsApp {
    pub exe: String,
    pub path: String,
    /// Aplica sozinho sempre que o programa abrir.
    pub auto: bool,
}

/// Programa com aparelho de som escolhido. Vazio usa o padrão do Windows.
#[derive(Clone, Default, Debug)]
pub struct SndApp {
    pub exe: String,
    pub path: String,
    pub output: String,
    pub input: String,
}

/// Tela cujas opções voltam ao padrão.
#[derive(Clone, Copy, PartialEq)]
pub enum Defaults {
    Meter,
    Capture,
    Energy,
    Fullscreen,
    Volume,
    Widget,
    Optimize,
    Settings,
    Tweaks,
    Uninstall,
}

/// Lembrança por jogo: o que aplicar quando esse programa volta a abrir.
#[derive(Clone, Default, Debug)]
pub struct Profile {
    pub exe: String,
    pub borderless: bool,
    /// 0 a 100; -1 quando não lembra.
    pub volume: i32,
    /// GUID do plano; vazio quando usa o plano padrão de jogo.
    pub plan: String,
    /// 0 sem preferência, 1 mostrar widget, 2 esconder.
    pub widget: u8,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub first_run: bool,
    pub hk_fullscreen: Hotkey,
    pub hk_widget: Hotkey,
    pub hk_optimize: Hotkey,
    pub hk_force: Hotkey,
    pub hk_shot: Hotkey,
    pub hk_shot_full: Hotkey,
    pub hk_shot_win: Hotkey,
    pub hk_record: Hotkey,
    pub hk_record_full: Hotkey,
    pub hk_replay: Hotkey,
    pub fs_remember: bool,
    pub vol_mods: u32,
    pub vol_step: u32,
    pub vol_osd: bool,
    pub widget_on: bool,
    pub widget_style: u8,
    pub widget_x: i32,
    pub widget_y: i32,
    pub power_auto: bool,
    pub plan_game: String,
    pub plan_rest: String,
    /// 0 pergunta, 1 otimiza sem perguntar, 2 só troca o plano.
    pub on_game: u8,
    pub rule_fullscreen: bool,
    pub rule_gpu: bool,
    pub rule_cpu: bool,
    pub rule_library: bool,
    pub load_gpu_pct: u32,
    pub load_gpu_secs: u32,
    pub load_cpu_pct: u32,
    pub load_cpu_secs: u32,
    pub fs_apps: Vec<FsApp>,
    pub hdr_profiles: Vec<HdrProfile>,
    pub hdr_games: Vec<HdrGame>,
    pub meter_games: Vec<MeterGame>,
    pub meter_rule: u8,
    pub meter_hdr: u8,
    pub meter_items: u32,
    pub meter_op_compact: u32,
    pub meter_op_full: u32,
    pub hk_meter_compact: Hotkey,
    pub hk_meter_full: Hotkey,
    pub hk_meter_off: Hotkey,
    /// 0 superior esquerdo, 1 superior centro, 2 superior direito.
    pub meter_pos_compact: u8,
    /// 0 lado direito, 1 lado esquerdo.
    pub meter_pos_full: u8,
    pub hk_log: Hotkey,
    /// 0 um minuto, 1 cinco, 2 dez, 3 sem limite.
    pub meter_log_time: u8,
    pub meter_log_auto: bool,
    pub hk_meter_keys: Hotkey,
    pub hk_limit_toggle: Hotkey,
    pub hk_limit_up: Hotkey,
    pub hk_limit_down: Hotkey,
    pub snd_keep: u8,
    pub snd_apps: Vec<SndApp>,
    pub black: Vec<String>,
    pub white: Vec<String>,
    pub ignored: Vec<String>,
    pub profiles: Vec<Profile>,
    pub cap_dir: String,
    pub cap_target: u8,
    pub cap_mode: u8,
    pub cap_codec: u8,
    pub cap_hdr: bool,
    pub cap_sys_audio: bool,
    pub cap_mic: bool,
    pub cap_cursor: bool,
    pub shot_hdr: bool,
    pub replay_on: bool,
    pub replay_secs: u32,
    pub lang_en: bool,
    pub update_auto: bool,
    /// Dia da última busca, para o programa procurar uma vez por dia mesmo reabrindo.
    pub upd_checked: u64,
    /// Versão achada na última busca. Com ela, abrir de novo busca os detalhes para perguntar.
    pub upd_found: String,
    pub welcome: bool,
    pub always_admin: bool,
    pub un_restore: bool,
    pub tw_log: bool,
    pub un_silent: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            first_run: true,
            hk_fullscreen: Hotkey::new(MOD_CTRL | MOD_ALT, b'F' as u32),
            hk_widget: Hotkey::new(MOD_CTRL | MOD_ALT, b'W' as u32),
            hk_optimize: Hotkey::new(MOD_CTRL | MOD_ALT, b'O' as u32),
            hk_force: Hotkey::new(MOD_ALT, 0x73),
            hk_shot: Hotkey::new(MOD_CTRL | MOD_ALT, b'P' as u32),
            hk_shot_full: Hotkey::new(MOD_CTRL | MOD_ALT, b'A' as u32),
            hk_shot_win: Hotkey::new(MOD_CTRL | MOD_ALT, b'J' as u32),
            hk_record: Hotkey::new(MOD_CTRL | MOD_ALT, b'R' as u32),
            hk_record_full: Hotkey::new(MOD_CTRL | MOD_ALT, b'T' as u32),
            hk_replay: Hotkey::new(MOD_CTRL | MOD_ALT, b'B' as u32),
            fs_remember: true,
            vol_mods: MOD_CTRL,
            vol_step: 5,
            vol_osd: true,
            widget_on: false,
            widget_style: 0,
            widget_x: 24,
            widget_y: 24,
            power_auto: true,
            plan_game: "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c".into(),
            plan_rest: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
            on_game: 0,
            rule_fullscreen: true,
            rule_gpu: true,
            rule_cpu: true,
            rule_library: true,
            load_gpu_pct: 40,
            load_gpu_secs: 5,
            load_cpu_pct: 35,
            load_cpu_secs: 20,
            fs_apps: Vec::new(),
            hdr_profiles: Vec::new(),
            hdr_games: Vec::new(),
            meter_games: Vec::new(),
            meter_rule: 1,
            meter_hdr: 0,
            meter_items: crate::meter::ITEMS_DEFAULT,
            meter_op_compact: 84,
            meter_op_full: 88,
            hk_meter_compact: Hotkey::new(MOD_CTRL | MOD_ALT, b'1' as u32),
            hk_meter_full: Hotkey::new(MOD_CTRL | MOD_ALT, b'2' as u32),
            hk_meter_off: Hotkey::new(MOD_CTRL | MOD_ALT, b'0' as u32),
            meter_pos_compact: 1,
            meter_pos_full: 0,
            hk_log: Hotkey::new(MOD_CTRL | MOD_ALT, b'4' as u32),
            meter_log_time: 1,
            meter_log_auto: false,
            hk_meter_keys: Hotkey::new(MOD_CTRL | MOD_ALT, b'5' as u32),
            hk_limit_toggle: Hotkey::new(MOD_CTRL | MOD_ALT, b'3' as u32),
            hk_limit_up: Hotkey::new(MOD_CTRL | MOD_ALT, 0x26),
            hk_limit_down: Hotkey::new(MOD_CTRL | MOD_ALT, 0x28),
            snd_keep: 0,
            snd_apps: Vec::new(),
            black: Vec::new(),
            white: Vec::new(),
            ignored: Vec::new(),
            profiles: Vec::new(),
            cap_dir: default_capture_dir(),
            cap_target: 0,
            cap_mode: 0,
            cap_codec: 0,
            cap_hdr: false,
            cap_sys_audio: true,
            cap_mic: false,
            cap_cursor: true,
            shot_hdr: false,
            replay_on: false,
            replay_secs: 30,
            lang_en: !crate::i18n::system_is_portuguese(),
            update_auto: true,
            upd_checked: 0,
            upd_found: String::new(),
            welcome: true,
            always_admin: false,
            un_restore: true,
            tw_log: false,
            un_silent: true,
        }
    }
}

fn default_capture_dir() -> String {
    let base = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
    format!("{base}\\Videos\\Prisma")
}

/// Instalado guarda em %APPDATA%; portátil guarda ao lado do programa.
pub fn data_dir() -> PathBuf {
    if crate::startup::is_installed() {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
        PathBuf::from(base).join("Prisma")
    } else {
        let exe = std::env::current_exe().unwrap_or_default();
        exe.parent().map(|p| p.join("PrismaData")).unwrap_or_else(|| PathBuf::from("PrismaData"))
    }
}

/// Acrescenta uma linha ao registro de ações.
pub fn log(line: &str) {
    use std::io::Write;
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(data_dir().join("acoes.log")) {
        let _ = writeln!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {line}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    }
}

fn config_path() -> PathBuf {
    data_dir().join("config.ini")
}

impl Config {
    pub fn load() -> Self {
        let mut c = Config::default();
        let Ok(text) = std::fs::read_to_string(config_path()) else {
            return c;
        };
        c.first_run = false;
        // Quem já usa não vê as boas-vindas, e a versão instalada sempre abriu como administrador.
        c.welcome = false;
        c.always_admin = crate::startup::is_installed();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let b = v == "1";
            match k {
                "hk_fullscreen" => c.hk_fullscreen = Hotkey::parse(v),
                "hk_widget" => c.hk_widget = Hotkey::parse(v),
                "hk_optimize" => c.hk_optimize = Hotkey::parse(v),
                "hk_force" => c.hk_force = Hotkey::parse(v),
                "hk_shot" => c.hk_shot = Hotkey::parse(v),
                "hk_shot_full" => c.hk_shot_full = Hotkey::parse(v),
                "hk_shot_win" => c.hk_shot_win = Hotkey::parse(v),
                "hk_record_full" => c.hk_record_full = Hotkey::parse(v),
                "hk_record" => c.hk_record = Hotkey::parse(v),
                "hk_replay" => c.hk_replay = Hotkey::parse(v),
                "fs_remember" => c.fs_remember = b,
                "vol_mods" => c.vol_mods = v.parse().unwrap_or(MOD_CTRL),
                "vol_step" => c.vol_step = v.parse().unwrap_or(5),
                "vol_osd" => c.vol_osd = b,
                "widget_on" => c.widget_on = b,
                "widget_style" => c.widget_style = v.parse().unwrap_or(0),
                "widget_x" => c.widget_x = v.parse().unwrap_or(24),
                "widget_y" => c.widget_y = v.parse().unwrap_or(24),
                "power_auto" => c.power_auto = b,
                "plan_game" => c.plan_game = v.into(),
                "plan_rest" => c.plan_rest = v.into(),
                "on_game" => c.on_game = v.parse().unwrap_or(0),
                "rule_fullscreen" => c.rule_fullscreen = b,
                "rule_gpu" => c.rule_gpu = b,
                "rule_cpu" => c.rule_cpu = b,
                "rule_library" => c.rule_library = b,
                "black" => c.black.push(v.into()),
                "white" => c.white.push(v.into()),
                "ignored" => c.ignored.push(v.into()),
                "load_gpu_pct" => c.load_gpu_pct = v.parse().unwrap_or(40),
                "load_gpu_secs" => c.load_gpu_secs = v.parse().unwrap_or(5),
                "load_cpu_pct" => c.load_cpu_pct = v.parse().unwrap_or(35),
                "load_cpu_secs" => c.load_cpu_secs = v.parse().unwrap_or(20),
                "snd_app" => {
                    let p: Vec<&str> = v.splitn(4, '|').collect();
                    if p.len() == 4 {
                        c.snd_apps.push(SndApp { exe: p[0].into(), output: p[1].into(), input: p[2].into(), path: p[3].into() });
                    }
                }
                "snd_keep" => c.snd_keep = v.parse().unwrap_or(0),
                "meter_rule" => c.meter_rule = v.parse().unwrap_or(1),
                "meter_hdr" => c.meter_hdr = v.parse().unwrap_or(0),
                "meter_items" => c.meter_items = v.parse().unwrap_or(crate::meter::ITEMS_DEFAULT),
                "meter_op_compact" => c.meter_op_compact = v.parse().unwrap_or(84),
                "meter_op_full" => c.meter_op_full = v.parse().unwrap_or(88),
                "hk_meter_compact" => c.hk_meter_compact = Hotkey::parse(v),
                "hk_meter_full" => c.hk_meter_full = Hotkey::parse(v),
                "hk_meter_off" => c.hk_meter_off = Hotkey::parse(v),
                "meter_pos_compact" => c.meter_pos_compact = v.parse().unwrap_or(1),
                "meter_pos_full" => c.meter_pos_full = v.parse().unwrap_or(0),
                "hk_log" => c.hk_log = Hotkey::parse(v),
                "meter_log_time" => c.meter_log_time = v.parse().unwrap_or(1),
                "meter_log_auto" => c.meter_log_auto = v == "1",
                "hk_meter_keys" => c.hk_meter_keys = Hotkey::parse(v),
                "hk_limit_toggle" => c.hk_limit_toggle = Hotkey::parse(v),
                "hk_limit_up" => c.hk_limit_up = Hotkey::parse(v),
                "hk_limit_down" => c.hk_limit_down = Hotkey::parse(v),
                "meter_game" => {
                    let p: Vec<&str> = v.split('|').collect();
                    if p.len() >= 9 {
                        let n = |s: &str| s.parse().unwrap_or(0);
                        let flag = |i: usize| p.get(i).is_some_and(|v| *v == "1");
                        c.meter_games.push(MeterGame { exe: p[0].into(), view: n(p[1]), limit_mode: n(p[2]), limit_fps: p[3].parse().unwrap_or(0), limit_real: p[4] == "1", measured: p[5] == "1", hdr: n(p[6]), preset: n(p[7]), path: p[8].into(), inject: p.get(9).map_or(true, |v| *v == "1"), inject_off: flag(10) });
                    }
                }
                "hdr_game" => {
                    let p: Vec<&str> = v.split('|').collect();
                    if p.len() == 3 {
                        c.hdr_games.push(HdrGame { exe: p[0].into(), mode: p[1].parse().unwrap_or(0), path: p[2].into() });
                    }
                }
                "hdr_profile" => {
                    let p: Vec<&str> = v.split('|').collect();
                    if p.len() == 6 {
                        let num = |s: &str| s.parse().unwrap_or(0.0);
                        c.hdr_profiles.push(HdrProfile { screen: p[0].into(), mode: p[1].into(), file: p[2].into(), peak: num(p[3]), full: num(p[4]), black: num(p[5]) });
                    }
                }
                "fs_app" => {
                    let p: Vec<&str> = v.splitn(3, '|').collect();
                    if p.len() == 3 {
                        c.fs_apps.push(FsApp { exe: p[0].into(), auto: p[1] == "1", path: p[2].into() });
                    }
                }
                "profile" => {
                    let p: Vec<&str> = v.split('|').collect();
                    if p.len() == 5 {
                        c.profiles.push(Profile {
                            exe: p[0].into(),
                            borderless: p[1] == "1",
                            volume: p[2].parse().unwrap_or(-1),
                            plan: p[3].into(),
                            widget: p[4].parse().unwrap_or(0),
                        });
                    }
                }
                "cap_dir" => c.cap_dir = v.into(),
                "cap_target" => c.cap_target = v.parse().unwrap_or(0),
                "cap_mode" => c.cap_mode = v.parse().unwrap_or(0),
                "cap_codec" => c.cap_codec = v.parse().unwrap_or(0),
                "cap_hdr" => c.cap_hdr = b,
                "cap_sys_audio" => c.cap_sys_audio = b,
                "cap_mic" => c.cap_mic = b,
                "cap_cursor" => c.cap_cursor = b,
                "shot_hdr" => c.shot_hdr = b,
                "replay_on" => c.replay_on = b,
                "replay_secs" => c.replay_secs = v.parse().unwrap_or(30),
                "lang_en" => c.lang_en = b,
                "update_auto" => c.update_auto = b,
                "upd_checked" => c.upd_checked = v.parse().unwrap_or(0),
                "upd_found" => c.upd_found = v.to_string(),
                "welcome" => c.welcome = b,
                "always_admin" => c.always_admin = b,
                "un_restore" => c.un_restore = b,
                "tw_log" => c.tw_log = b,
                "un_silent" => c.un_silent = b,
                _ => {}
            }
        }
        // A lista substitui a lembrança antiga por programa.
        for p in c.profiles.iter_mut().filter(|p| p.borderless) {
            p.borderless = false;
            if !c.fs_apps.iter().any(|a| a.exe.eq_ignore_ascii_case(&p.exe)) {
                c.fs_apps.push(FsApp { exe: p.exe.clone(), path: String::new(), auto: true });
            }
        }
        c
    }

    pub fn save(&self) {
        let mut s = String::new();
        let hk = |s: &mut String, k: &str, h: Hotkey| {
            let _ = writeln!(s, "{k}={},{}", h.mods, h.vk);
        };
        hk(&mut s, "hk_fullscreen", self.hk_fullscreen);
        hk(&mut s, "hk_widget", self.hk_widget);
        hk(&mut s, "hk_optimize", self.hk_optimize);
        hk(&mut s, "hk_force", self.hk_force);
        hk(&mut s, "hk_shot", self.hk_shot);
        hk(&mut s, "hk_shot_full", self.hk_shot_full);
        hk(&mut s, "hk_shot_win", self.hk_shot_win);
        hk(&mut s, "hk_record_full", self.hk_record_full);
        hk(&mut s, "hk_record", self.hk_record);
        hk(&mut s, "hk_replay", self.hk_replay);
        let b = |v: bool| if v { 1 } else { 0 };
        let _ = writeln!(s, "fs_remember={}", b(self.fs_remember));
        let _ = writeln!(s, "vol_mods={}", self.vol_mods);
        let _ = writeln!(s, "vol_step={}", self.vol_step);
        let _ = writeln!(s, "vol_osd={}", b(self.vol_osd));
        let _ = writeln!(s, "widget_on={}", b(self.widget_on));
        let _ = writeln!(s, "widget_style={}", self.widget_style);
        let _ = writeln!(s, "widget_x={}", self.widget_x);
        let _ = writeln!(s, "widget_y={}", self.widget_y);
        let _ = writeln!(s, "power_auto={}", b(self.power_auto));
        let _ = writeln!(s, "plan_game={}", self.plan_game);
        let _ = writeln!(s, "plan_rest={}", self.plan_rest);
        let _ = writeln!(s, "on_game={}", self.on_game);
        let _ = writeln!(s, "rule_fullscreen={}", b(self.rule_fullscreen));
        let _ = writeln!(s, "rule_gpu={}", b(self.rule_gpu));
        let _ = writeln!(s, "rule_cpu={}", b(self.rule_cpu));
        let _ = writeln!(s, "rule_library={}", b(self.rule_library));
        for v in &self.black {
            let _ = writeln!(s, "black={v}");
        }
        for v in &self.white {
            let _ = writeln!(s, "white={v}");
        }
        for v in &self.ignored {
            let _ = writeln!(s, "ignored={v}");
        }
        for p in &self.profiles {
            let _ = writeln!(
                s,
                "profile={}|{}|{}|{}|{}",
                p.exe,
                b(p.borderless),
                p.volume,
                p.plan,
                p.widget
            );
        }
        let _ = writeln!(s, "cap_dir={}", self.cap_dir);
        let _ = writeln!(s, "cap_target={}", self.cap_target);
        let _ = writeln!(s, "cap_mode={}", self.cap_mode);
        let _ = writeln!(s, "cap_codec={}", self.cap_codec);
        let _ = writeln!(s, "cap_hdr={}", b(self.cap_hdr));
        let _ = writeln!(s, "cap_sys_audio={}", b(self.cap_sys_audio));
        let _ = writeln!(s, "cap_mic={}", b(self.cap_mic));
        let _ = writeln!(s, "cap_cursor={}", b(self.cap_cursor));
        let _ = writeln!(s, "shot_hdr={}", b(self.shot_hdr));
        let _ = writeln!(s, "replay_on={}", b(self.replay_on));
        let _ = writeln!(s, "replay_secs={}", self.replay_secs);
        let _ = writeln!(s, "lang_en={}", b(self.lang_en));
        let _ = writeln!(s, "update_auto={}", b(self.update_auto));
        let _ = writeln!(s, "upd_checked={}", self.upd_checked);
        let _ = writeln!(s, "upd_found={}", self.upd_found);
        let _ = writeln!(s, "welcome={}", b(self.welcome));
        let _ = writeln!(s, "always_admin={}", b(self.always_admin));
        let _ = writeln!(s, "un_restore={}", b(self.un_restore));
        let _ = writeln!(s, "load_gpu_pct={}", self.load_gpu_pct);
        let _ = writeln!(s, "load_gpu_secs={}", self.load_gpu_secs);
        let _ = writeln!(s, "load_cpu_pct={}", self.load_cpu_pct);
        let _ = writeln!(s, "load_cpu_secs={}", self.load_cpu_secs);
        for a in &self.snd_apps {
            let _ = writeln!(s, "snd_app={}|{}|{}|{}", a.exe, a.output, a.input, a.path);
        }
        let _ = writeln!(s, "snd_keep={}", self.snd_keep);
        let _ = writeln!(s, "meter_rule={}", self.meter_rule);
        let _ = writeln!(s, "meter_hdr={}", self.meter_hdr);
        let _ = writeln!(s, "meter_items={}", self.meter_items);
        let _ = writeln!(s, "meter_op_compact={}", self.meter_op_compact);
        let _ = writeln!(s, "meter_op_full={}", self.meter_op_full);
        hk(&mut s, "hk_meter_compact", self.hk_meter_compact);
        hk(&mut s, "hk_meter_full", self.hk_meter_full);
        hk(&mut s, "hk_meter_off", self.hk_meter_off);
        s.push_str(&format!("meter_pos_compact={}\nmeter_pos_full={}\n", self.meter_pos_compact, self.meter_pos_full));
        hk(&mut s, "hk_log", self.hk_log);
        s.push_str(&format!("meter_log_time={}\nmeter_log_auto={}\n", self.meter_log_time, self.meter_log_auto as u8));
        hk(&mut s, "hk_meter_keys", self.hk_meter_keys);
        hk(&mut s, "hk_limit_toggle", self.hk_limit_toggle);
        hk(&mut s, "hk_limit_up", self.hk_limit_up);
        hk(&mut s, "hk_limit_down", self.hk_limit_down);
        for g in &self.meter_games {
            let _ = writeln!(s, "meter_game={}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}", g.exe, g.view, g.limit_mode, g.limit_fps, b(g.limit_real), b(g.measured), g.hdr, g.preset, g.path, b(g.inject), b(g.inject_off));
        }
        for g in &self.hdr_games {
            let _ = writeln!(s, "hdr_game={}|{}|{}", g.exe, g.mode, g.path);
        }
        for p in &self.hdr_profiles {
            let _ = writeln!(s, "hdr_profile={}|{}|{}|{}|{}|{}", p.screen, p.mode, p.file, p.peak, p.full, p.black);
        }
        for a in &self.fs_apps {
            let _ = writeln!(s, "fs_app={}|{}|{}", a.exe, b(a.auto), a.path);
        }
        let _ = writeln!(s, "tw_log={}", b(self.tw_log));
        let _ = writeln!(s, "un_silent={}", b(self.un_silent));
        let path = config_path();
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, s);
    }

    /// Entrada do programa na lista de som, criada se ainda não existir.
    pub fn snd_entry(&mut self, exe: &str, path: &str) -> &mut SndApp {
        if let Some(i) = self.snd_apps.iter().position(|a| a.exe.eq_ignore_ascii_case(exe)) {
            if self.snd_apps[i].path.is_empty() {
                self.snd_apps[i].path = path.to_string();
            }
            return &mut self.snd_apps[i];
        }
        self.snd_apps.push(SndApp { exe: exe.to_string(), path: path.to_string(), ..Default::default() });
        self.snd_apps.last_mut().unwrap()
    }

    pub fn fs_auto(&self, exe: &str) -> bool {
        self.fs_apps.iter().any(|a| a.auto && a.exe.eq_ignore_ascii_case(exe))
    }

    /// Acrescenta o programa à lista, sem mudar a escolha de quem já estava nela.
    pub fn fs_note(&mut self, exe: &str, path: &str) {
        match self.fs_apps.iter_mut().find(|a| a.exe.eq_ignore_ascii_case(exe)) {
            Some(a) if a.path.is_empty() => a.path = path.to_string(),
            Some(_) => {}
            None => self.fs_apps.push(FsApp { exe: exe.to_string(), path: path.to_string(), auto: false }),
        }
    }

    pub fn reset(&mut self, page: Defaults) {
        let d = Config::default();
        match page {
            Defaults::Capture => {
                self.hk_shot = d.hk_shot;
                self.hk_shot_full = d.hk_shot_full;
                self.hk_shot_win = d.hk_shot_win;
                self.hk_record = d.hk_record;
                self.hk_record_full = d.hk_record_full;
                self.hk_replay = d.hk_replay;
                self.cap_dir = d.cap_dir;
                self.cap_target = d.cap_target;
                self.cap_mode = d.cap_mode;
                self.cap_codec = d.cap_codec;
                self.cap_hdr = d.cap_hdr;
                self.cap_sys_audio = d.cap_sys_audio;
                self.cap_mic = d.cap_mic;
                self.cap_cursor = d.cap_cursor;
                self.shot_hdr = d.shot_hdr;
                self.replay_on = d.replay_on;
                self.replay_secs = d.replay_secs;
            }
            Defaults::Energy => {
                self.power_auto = d.power_auto;
                self.plan_game = d.plan_game;
                self.plan_rest = d.plan_rest;
                self.on_game = d.on_game;
                self.rule_fullscreen = d.rule_fullscreen;
                self.rule_gpu = d.rule_gpu;
                self.rule_cpu = d.rule_cpu;
                self.rule_library = d.rule_library;
                self.load_gpu_pct = d.load_gpu_pct;
                self.load_gpu_secs = d.load_gpu_secs;
                self.load_cpu_pct = d.load_cpu_pct;
                self.load_cpu_secs = d.load_cpu_secs;
            }
            Defaults::Fullscreen => {
                self.hk_fullscreen = d.hk_fullscreen;
                for a in &mut self.fs_apps {
                    a.auto = false;
                }
            }
            Defaults::Meter => {
                self.meter_rule = d.meter_rule;
                self.meter_hdr = d.meter_hdr;
                self.meter_items = d.meter_items;
                self.meter_op_compact = d.meter_op_compact;
                self.meter_op_full = d.meter_op_full;
                self.hk_meter_compact = d.hk_meter_compact;
                self.hk_meter_full = d.hk_meter_full;
                self.hk_meter_off = d.hk_meter_off;
                self.meter_pos_compact = d.meter_pos_compact;
                self.meter_pos_full = d.meter_pos_full;
                self.hk_log = d.hk_log;
                self.meter_log_time = d.meter_log_time;
                self.meter_log_auto = d.meter_log_auto;
                self.hk_meter_keys = d.hk_meter_keys;
                self.hk_limit_toggle = d.hk_limit_toggle;
                self.hk_limit_up = d.hk_limit_up;
                self.hk_limit_down = d.hk_limit_down;
            }
            Defaults::Volume => {
                self.vol_mods = d.vol_mods;
                self.vol_step = d.vol_step;
                self.vol_osd = d.vol_osd;
            }
            Defaults::Widget => {
                self.hk_widget = d.hk_widget;
                self.widget_on = d.widget_on;
                self.widget_style = d.widget_style;
                self.widget_x = d.widget_x;
                self.widget_y = d.widget_y;
            }
            Defaults::Optimize => {
                self.black = d.black;
                self.white = d.white;
                self.ignored = d.ignored;
            }
            Defaults::Settings => {
                self.hk_optimize = d.hk_optimize;
                self.hk_force = d.hk_force;
                self.update_auto = d.update_auto;
                self.lang_en = d.lang_en;
            }
            Defaults::Tweaks => {
                self.tw_log = d.tw_log;
                self.un_restore = d.un_restore;
            }
            Defaults::Uninstall => {
                self.un_restore = d.un_restore;
                self.un_silent = d.un_silent;
            }
        }
    }

    pub fn profile_mut(&mut self, exe: &str) -> &mut Profile {
        if let Some(i) = self.profiles.iter().position(|p| p.exe.eq_ignore_ascii_case(exe)) {
            return &mut self.profiles[i];
        }
        self.profiles.push(Profile { exe: exe.into(), volume: -1, ..Default::default() });
        self.profiles.last_mut().unwrap()
    }

    pub fn profile(&self, exe: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.exe.eq_ignore_ascii_case(exe))
    }
}
