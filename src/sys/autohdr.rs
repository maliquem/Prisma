//! Auto HDR por jogo: uma chave por programa no registro do usuário, sem tocar no jogo.

use super::reg::{self, Hive};

const ROOT: &str = "Software\\Microsoft\\Direct3D";
const PREFIX: &str = "Prisma ";

fn text(s: &str) -> Vec<u8> {
    s.encode_utf16().chain(std::iter::once(0)).flat_map(u16::to_le_bytes).collect()
}

/// 0 deixa o Windows decidir, 1 força, 2 desliga. Vale na próxima vez que o jogo abrir.
pub fn apply(exe: &str, mode: u8) -> bool {
    const REG_SZ: u32 = 1;
    let key = format!("{ROOT}\\{PREFIX}{exe}");
    let behavior = match mode {
        1 => "BufferUpgradeOverride=1",
        2 => "DisableBufferUpgrade=1",
        _ => return reg::delete_key(Hive::User, &key),
    };
    reg::write(Hive::User, &key, "Name", REG_SZ, &text(exe)) && reg::write(Hive::User, &key, "D3DBehaviors", REG_SZ, &text(behavior))
}

/// Tira todas as escolhas feitas pelo Prisma.
pub fn clear_all() {
    for k in reg::subkeys(Hive::User, ROOT).iter().filter(|k| k.starts_with(PREFIX)) {
        reg::delete_key(Hive::User, &format!("{ROOT}\\{k}"));
    }
}

/// A chave geral do Auto HDR em Configurações do Windows.
pub fn global_on() -> bool {
    let key = "Software\\Microsoft\\DirectX\\UserGpuPreferences";
    reg::text(Hive::User, key, "DirectXUserGlobalSettings").is_some_and(|v| v.contains("AutoHDREnable=1"))
}
