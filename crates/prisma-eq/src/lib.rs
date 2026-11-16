//! Equalizador do Prisma: a mesma conta serve ao APO (som) e à página (curva desenhada).
//! Sem Windows e sem alocação em `Chain::process`.

mod apo;
mod biquad;
mod chain;
mod profile;

pub use biquad::{Biquad, Kind};
pub use chain::{Chain, MAX_DELAY_MS, channel_names};
pub use profile::{Filter, Group, PROTECT_HP_HZ, PROTECT_LP_HZ, Profile, graphic_freqs};
