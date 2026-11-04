//! Configuração em texto simples em %APPDATA%\Prisma\config.ini.

use std::fmt::Write as _;
use std::path::PathBuf;

pub const MOD_ALT: u32 = 1;
pub const MOD_CTRL: u32 = 2;
pub const MOD_SHIFT: u32 = 4;
pub const MOD_WIN: u32 = 8;

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
        0x60..=0x69 => format!("Num {}", vk - 0x60),
        _ => format!("Tecla {vk}"),
    }
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
    pub hk_shot: Hotkey,
    pub hk_shot_full: Hotkey,
    pub hk_record: Hotkey,
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            first_run: true,
            hk_fullscreen: Hotkey::new(MOD_CTRL | MOD_ALT, b'F' as u32),
            hk_widget: Hotkey::new(MOD_CTRL | MOD_ALT, b'W' as u32),
            hk_optimize: Hotkey::new(MOD_CTRL | MOD_ALT, b'O' as u32),
            hk_shot: Hotkey::new(MOD_CTRL | MOD_ALT, b'S' as u32),
            hk_shot_full: Hotkey::new(MOD_CTRL | MOD_ALT, b'A' as u32),
            hk_record: Hotkey::new(MOD_CTRL | MOD_ALT, b'R' as u32),
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
            plan_game: "aead177f-ad3a-4424-aaea-03508d489617".into(),
            plan_rest: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
            on_game: 0,
            rule_fullscreen: true,
            rule_gpu: true,
            rule_cpu: true,
            rule_library: true,
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
            lang_en: false,
        }
    }
}

fn default_capture_dir() -> String {
    let base = std::env::var("USERPROFILE").unwrap_or_else(|_| "C:\\".into());
    format!("{base}\\Videos\\Prisma")
}

fn config_path() -> PathBuf {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    PathBuf::from(base).join("Prisma").join("config.ini")
}

pub fn data_dir() -> PathBuf {
    config_path().parent().unwrap().to_path_buf()
}

impl Config {
    pub fn load() -> Self {
        let mut c = Config::default();
        let Ok(text) = std::fs::read_to_string(config_path()) else {
            return c;
        };
        c.first_run = false;
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let b = v == "1";
            match k {
                "hk_fullscreen" => c.hk_fullscreen = Hotkey::parse(v),
                "hk_widget" => c.hk_widget = Hotkey::parse(v),
                "hk_optimize" => c.hk_optimize = Hotkey::parse(v),
                "hk_shot" => c.hk_shot = Hotkey::parse(v),
                "hk_shot_full" => c.hk_shot_full = Hotkey::parse(v),
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
                _ => {}
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
        hk(&mut s, "hk_shot", self.hk_shot);
        hk(&mut s, "hk_shot_full", self.hk_shot_full);
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
        let path = config_path();
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        let _ = std::fs::write(path, s);
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
