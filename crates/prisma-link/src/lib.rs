//! Protocolo e enquadramento da ponte Prisma Launcher <-> Prisma.

mod frame;
mod proto;

#[cfg(windows)]
mod client;
#[cfg(windows)]
mod pipe;
#[cfg(windows)]
mod server;

pub use frame::{read_frame, write_frame};
pub use proto::{
    AudioDevice, Capabilities, Controller, FpsLimit, GameInvite, InviteState, MeterView, NvidiaKind, NvidiaSetting, NvidiaValue, OptionValue, Page,
    PowerPlan, ReplayOptions, ReplayResolution, Reply, Request, ShareMode, VolumeChange,
};

#[cfg(windows)]
pub use client::{CONNECT_TIMEOUT, Client};
#[cfg(windows)]
pub use pipe::{current_user_sid, default_pipe_name, pipe_name, share_pipe_name, token_user_sid};
#[cfg(windows)]
pub use server::{ServerInfo, serve};

pub const PROTOCOL: u32 = 1;
pub const MAX_FRAME: u32 = 65_536;

pub mod cap {
    pub const GAME_EVENTS: &str = "game_events";
    pub const PAGE_METER: &str = "page.meter";
    pub const PAGE_SHARE: &str = "page.share";
    pub const PAGE_GAME_PROFILE: &str = "page.game_profile";
    pub const SHARE_INVITE: &str = "share_invite";
    pub const SAVE_REPLAY: &str = "save_replay";
    pub const CAPTURE_DIRS: &str = "capture_dirs";
    /// `share_invite` aceita `minutes`.
    pub const SHARE_INVITE_MINUTES: &str = "share_invite.minutes";
    pub const METER_VIEW: &str = "meter.view";
    pub const POWER_PLAN: &str = "power.plan";
    pub const FULLSCREEN: &str = "fullscreen";
    pub const SOUND_VOLUME: &str = "sound.volume";
    pub const HDR: &str = "hdr";
    pub const NVIDIA: &str = "nvidia";
    pub const WINDOW_SHOT: &str = "capture.window_shot";
    pub const RECORD_WINDOW: &str = "capture.record_window";
    /// `Request::RecordScreen`.
    pub const RECORD_SCREEN: &str = "capture.record_screen";
    pub const PAGE_ENERGY: &str = "page.energy";
    pub const PAGE_FULLSCREEN: &str = "page.fullscreen";
    pub const PAGE_SOUND: &str = "page.sound";
    pub const PAGE_HDR: &str = "page.hdr";
    pub const PAGE_NVIDIA: &str = "page.nvidia";
    pub const PAGE_CAPTURE: &str = "page.capture";
    pub const PAGE_CONTROLLERS: &str = "page.controllers";
    /// Lista dos controles ligados, com conexão, bateria e taxa (`Request::Controllers`).
    pub const CONTROLLERS: &str = "controllers";
    /// Perfis do mapeamento: listar, trocar, parar, ligar e desligar (`PadProfiles`, `SetPadProfile`,
    /// `PadStop`, `PadMap`).
    pub const CONTROLLERS_MAPPING: &str = "controllers.mapping";
    /// Ligar e desligar a ponte do controle Xbox, já instalada (`Request::PadBridge`).
    pub const CONTROLLERS_BRIDGE: &str = "controllers.bridge";
    /// Abrir e fechar o teclado do Prisma para controle (`Request::Keyboard`).
    pub const CONTROLLERS_KEYBOARD: &str = "controllers.keyboard";
    /// Perguntar se alguém entra no compartilhamento de tela (`Request::ShareAsk`), no pipe
    /// `share_pipe_name`.
    pub const SHARE_ASK: &str = "share.ask";
    /// Ler o estado das opções da barra do launcher (`Request::Options`).
    pub const OPTIONS: &str = "options";
    /// `OptionValue::FpsLimit`.
    pub const METER_FPS_LIMIT: &str = "meter.fps_limit";
    /// `OptionValue::AutoHdr`.
    pub const HDR_AUTO: &str = "hdr.auto";
    /// `OptionValue::AudioOut` e `AudioIn`.
    pub const SOUND_DEVICES: &str = "sound.devices";
    /// `OptionValue::ShareMode`.
    pub const SHARE_MODE: &str = "share.mode";
    /// `OptionValue::Replay`.
    pub const REPLAY_OPTIONS: &str = "replay.options";
    /// `OptionValue::Fullscreenizer`.
    pub const FULLSCREENIZER: &str = "fullscreenizer";
    /// `OptionValue::Optimize`.
    pub const OPTIMIZE: &str = "optimize";
    /// `ShareInviteForGame` e `ShareInviteCancel`, com o estado em `Reply::Options::invite`.
    pub const SHARE_INVITE_GAME: &str = "share.invite_game";
    /// `OptionValue::Meter`: o medidor abre com o jogo.
    pub const METER_ON_LAUNCH: &str = "meter.on_launch";
    /// `OptionValue::HdrOnLaunch`.
    pub const HDR_ON_LAUNCH: &str = "hdr.on_launch";
    /// `OptionValue::Volume`: volume guardado do jogo.
    pub const SOUND_GAME_VOLUME: &str = "sound.game_volume";
    /// `OptionValue::EqProfile` e a lista `eq_profiles` no `Options`: perfil de equalizador do jogo.
    pub const SOUND_EQ: &str = "sound.eq";
}

/// Códigos estáveis de `Reply::Failed`.
pub mod fail {
    /// A janela do jogo não foi achada.
    pub const NO_WINDOW: &str = "no_window";
    /// O processo não tem sessão de som aberta.
    pub const NO_AUDIO: &str = "no_audio";
    /// Este PC não tem o que o pedido usa (placa NVIDIA, tela com HDR).
    pub const NOT_SUPPORTED: &str = "not_supported";
    /// O Prisma não atendeu a tempo (janela ocupada, menu aberto).
    pub const BUSY: &str = "busy";
    /// Pedido com valor que o Prisma não aceita (exe, id, valor, plano).
    pub const INVALID: &str = "invalid";
    /// O Windows ou o driver recusou, ou a captura falhou.
    pub const FAILED: &str = "failed";
}
