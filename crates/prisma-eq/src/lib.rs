//! Equalizador do Prisma: a mesma conta serve ao APO (som) e à página (curva desenhada).
//! Sem Windows e sem alocação em `Chain::process`.

mod biquad;
mod chain;
mod profile;

pub use biquad::{Biquad, Kind};
pub use chain::{Chain, MAX_DELAY_MS, channel_names};
pub use profile::{Filter, Group, Profile, graphic_freqs};
