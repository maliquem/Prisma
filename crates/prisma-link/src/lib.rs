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
    Capabilities, MeterView, NvidiaKind, NvidiaSetting, NvidiaValue, Page, PowerPlan, Reply, Request,
    VolumeChange,
};

#[cfg(windows)]
pub use client::{CONNECT_TIMEOUT, Client};
#[cfg(windows)]
pub use pipe::{current_user_sid, default_pipe_name, pipe_name};
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
    pub const PAGE_ENERGY: &str = "page.energy";
    pub const PAGE_FULLSCREEN: &str = "page.fullscreen";
    pub const PAGE_SOUND: &str = "page.sound";
    pub const PAGE_HDR: &str = "page.hdr";
    pub const PAGE_NVIDIA: &str = "page.nvidia";
    pub const PAGE_CAPTURE: &str = "page.capture";
    /// Reservada: a tela Controles ainda não existe. Nenhum Prisma anuncia esta capacidade até
    /// ela existir; as ações dos Controles virão com nomes próprios quando forem combinadas.
    pub const PAGE_CONTROLLERS: &str = "page.controllers";
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
