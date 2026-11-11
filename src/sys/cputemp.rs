//! Temperatura da CPU pelo PawnIO, um driver gratuito, aberto e assinado, instalado à parte.
//! O Windows não entrega a temperatura da CPU sem um driver: o Steam faz o mesmo com o dele.
//!
//! O Prisma conversa com o PawnIO só pela interface de controle do dispositivo, como a exceção da
//! licença dele permite, e carrega nele um módulo pronto do PawnIO.Modules (LGPL 2.1, texto em
//! `res/pawnio/COPYING`): AMDFamily17 para Ryzen, IntelMSR para Intel. Nada disso entra no Prisma
//! como código: os módulos rodam dentro do driver.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::IO::DeviceIoControl;

static AMD_MODULE: &[u8] = include_bytes!("../../res/pawnio/AMDFamily17.bin");
static INTEL_MODULE: &[u8] = include_bytes!("../../res/pawnio/IntelMSR.bin");

const DEVICE: &str = r"\\?\GLOBALROOT\Device\PawnIO";
const DEVICE_TYPE: u32 = 41394;
/// CTL_CODE com METHOD_BUFFERED e FILE_ANY_ACCESS.
const fn ctl(function: u32) -> u32 {
    (DEVICE_TYPE << 16) | (function << 2)
}
const LOAD_BINARY: u32 = ctl(0x821);
const EXECUTE_FN: u32 = ctl(0x841);
/// Nome da função do módulo: 32 bytes, completados com zero.
const NAME_LEN: usize = 32;

/// Sensor da Ryzen (THM_TCON_CUR_TMP), lido pela rede interna da AMD.
const AMD_TCTL: u64 = 0x0005_9800;
const MSR_TEMPERATURE_TARGET: u64 = 0x1A2;
const MSR_PACKAGE_THERM_STATUS: u64 = 0x1B1;

/// Quem quer a temperatura agora: só com o medidor mostrando ela o driver fica em uso.
pub static WANTED: AtomicBool = AtomicBool::new(false);
/// O driver abriu na última tentativa: a tela usa para dizer o que falta.
pub static OPEN: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq)]
enum Vendor {
    Amd,
    Intel,
    Other,
}

pub struct CpuTemp {
    dev: HANDLE,
    vendor: Vendor,
    /// Ryzen antigas somam um valor fixo ao sensor.
    offset: f32,
    /// Outros programas de monitoramento usam o mesmo par de portas da AMD: este nome combina com eles.
    pci: HANDLE,
}

unsafe impl Send for CpuTemp {}

fn cpu_info() -> (Vendor, String) {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::__cpuid;
        let v = __cpuid(0);
        let mut id = Vec::new();
        for r in [v.ebx, v.edx, v.ecx] {
            id.extend_from_slice(&r.to_le_bytes());
        }
        let vendor = match &id[..] {
            b"AuthenticAMD" => Vendor::Amd,
            b"GenuineIntel" => Vendor::Intel,
            _ => Vendor::Other,
        };
        let mut brand = Vec::new();
        if __cpuid(0x8000_0000).eax >= 0x8000_0004 {
            for leaf in 0x8000_0002u32..=0x8000_0004 {
                let r = __cpuid(leaf);
                for x in [r.eax, r.ebx, r.ecx, r.edx] {
                    brand.extend_from_slice(&x.to_le_bytes());
                }
            }
        }
        let brand = String::from_utf8_lossy(&brand).trim_matches(char::from(0)).trim().to_string();
        (vendor, brand)
    }
    #[cfg(not(target_arch = "x86_64"))]
    (Vendor::Other, String::new())
}

/// Ryzen e Threadripper das primeiras gerações mostram o Tctl acima da temperatura real.
fn amd_offset(brand: &str) -> f32 {
    const LIST: [(&str, f32); 9] = [
        ("1600X", 20.0),
        ("1700X", 20.0),
        ("1800X", 20.0),
        ("2700X", 10.0),
        ("1920X", 27.0),
        ("1950X", 27.0),
        ("2920X", 27.0),
        ("2950X", 27.0),
        ("2990WX", 27.0),
    ];
    LIST.iter().find(|(name, _)| brand.contains(name)).map_or(0.0, |v| v.1)
}

/// O PawnIO está instalado? Abrir o dispositivo pede administrador.
pub fn installed() -> bool {
    open_device().is_some_and(|h| {
        unsafe {
            let _ = CloseHandle(h);
        }
        true
    })
}

fn open_device() -> Option<HANDLE> {
    unsafe {
        CreateFileW(
            &HSTRING::from(DEVICE),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .ok()
    }
}

fn ioctl(h: HANDLE, code: u32, input: &[u8], output: &mut [u8]) -> Option<usize> {
    unsafe {
        let mut got = 0;
        DeviceIoControl(
            h,
            code,
            Some(input.as_ptr() as *const c_void),
            input.len() as u32,
            (!output.is_empty()).then_some(output.as_mut_ptr() as *mut c_void),
            output.len() as u32,
            Some(&mut got),
            None,
        )
        .ok()?;
        Some(got as usize)
    }
}

impl CpuTemp {
    /// Abre o driver e carrega o módulo do processador. Sem PawnIO, ou com CPU sem módulo, nada.
    pub fn open() -> Option<Self> {
        let (vendor, brand) = cpu_info();
        let module = match vendor {
            Vendor::Amd => AMD_MODULE,
            Vendor::Intel => INTEL_MODULE,
            Vendor::Other => return None,
        };
        let dev = open_device()?;
        if ioctl(dev, LOAD_BINARY, module, &mut []).is_none() {
            unsafe {
                let _ = CloseHandle(dev);
            }
            return None;
        }
        let pci = unsafe { CreateMutexW(None, false, w!("Global\\Access_PCI")) }.unwrap_or_default();
        let offset = if vendor == Vendor::Amd { amd_offset(&brand) } else { 0.0 };
        Some(Self { dev, vendor, offset, pci })
    }

    fn call(&self, name: &str, args: &[u64]) -> Option<u64> {
        let mut input = vec![0u8; NAME_LEN + args.len() * 8];
        input[..name.len().min(NAME_LEN - 1)].copy_from_slice(&name.as_bytes()[..name.len().min(NAME_LEN - 1)]);
        for (i, a) in args.iter().enumerate() {
            input[NAME_LEN + i * 8..NAME_LEN + i * 8 + 8].copy_from_slice(&a.to_le_bytes());
        }
        let mut out = [0u8; 8];
        (ioctl(self.dev, EXECUTE_FN, &input, &mut out)? == 8).then(|| u64::from_le_bytes(out))
    }

    /// Temperatura do processador em °C.
    pub fn read(&self) -> Option<f32> {
        match self.vendor {
            Vendor::Amd => {
                // Mesma trava dos outros programas que leem este sensor, para não misturar as leituras.
                let locked = !self.pci.is_invalid() && unsafe { WaitForSingleObject(self.pci, 20) } == WAIT_OBJECT_0;
                let raw = self.call("ioctl_read_smn", &[AMD_TCTL]);
                if locked {
                    unsafe {
                        let _ = ReleaseMutex(self.pci);
                    }
                }
                let v = raw? as u32;
                let mut t = (v >> 21) as f32 * 0.125;
                // Faixa estendida: o sensor conta a partir de -49 °C.
                if v & (1 << 19) != 0 {
                    t -= 49.0;
                }
                let t = t - self.offset;
                (0.0..150.0).contains(&t).then_some(t)
            }
            Vendor::Intel => {
                let target = (self.call("ioctl_read_msr", &[MSR_TEMPERATURE_TARGET])? >> 16) & 0xFF;
                let status = self.call("ioctl_read_msr", &[MSR_PACKAGE_THERM_STATUS])?;
                if status & (1 << 31) == 0 {
                    return None;
                }
                let t = target as f32 - ((status >> 16) & 0x7F) as f32;
                (0.0..150.0).contains(&t).then_some(t)
            }
            Vendor::Other => None,
        }
    }
}

impl Drop for CpuTemp {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.dev);
            if !self.pci.is_invalid() {
                let _ = CloseHandle(self.pci);
            }
        }
    }
}

/// Instalação do PawnIO pelo Prisma: baixa o instalador oficial, confere a assinatura e abre ele.
pub mod setup {
    use super::*;

    pub const URL: &str = "https://github.com/namazso/PawnIO.Setup/releases/latest/download/PawnIO_setup.exe";
    /// Quem assina o instalador oficial.
    pub const SIGNER: &str = "namazso.eu";

    pub const IDLE: u8 = 0;
    pub const DOWNLOADING: u8 = 1;
    pub const RUNNING: u8 = 2;
    pub const DONE: u8 = 3;
    pub const FAILED: u8 = 4;

    pub static STATE: AtomicU8 = AtomicU8::new(IDLE);
    pub static ERROR: Mutex<String> = Mutex::new(String::new());

    fn fail(text: &str) {
        crate::config::log(&format!("PawnIO: {text}"));
        *ERROR.lock().unwrap() = text.to_string();
        STATE.store(FAILED, Ordering::Relaxed);
    }

    /// Roda numa linha própria; o estado diz à tela em que passo está.
    pub fn start() {
        if matches!(STATE.load(Ordering::Relaxed), DOWNLOADING | RUNNING) {
            return;
        }
        STATE.store(DOWNLOADING, Ordering::Relaxed);
        let _ = std::thread::Builder::new().name("pawnio".into()).spawn(|| {
            let Ok(data) = crate::sys::update::download(URL) else { return fail("o download falhou") };
            let path = std::env::temp_dir().join("PawnIO_setup.exe");
            if std::fs::write(&path, &data).is_err() {
                return fail("não deu para salvar o instalador");
            }
            let text = path.to_string_lossy().to_string();
            match crate::sys::fileinfo::check(&text) {
                crate::sys::fileinfo::Signature::Valid(who) if who == SIGNER => {}
                _ => {
                    let _ = std::fs::remove_file(&path);
                    return fail("a assinatura do instalador não confere");
                }
            }
            STATE.store(RUNNING, Ordering::Relaxed);
            crate::config::log("PawnIO: instalador aberto");
            let code = run_and_wait(&text);
            let _ = std::fs::remove_file(&path);
            if installed() {
                crate::config::log("PawnIO: instalado");
                STATE.store(DONE, Ordering::Relaxed);
            } else {
                fail(&format!("o instalador terminou sem instalar (código {})", code.unwrap_or(u32::MAX)));
            }
        });
    }

    /// Abre o instalador com a janela dele, para a pessoa ver os termos, e espera ele fechar.
    fn run_and_wait(path: &str) -> Option<u32> {
        use windows::Win32::UI::Shell::*;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let file = HSTRING::from(path);
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOCLOSEPROCESS,
            lpVerb: w!("open"),
            lpFile: PCWSTR(file.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe {
            ShellExecuteExW(&mut info).ok()?;
            let mut code = 1;
            let _ = WaitForSingleObject(info.hProcess, INFINITE);
            let _ = GetExitCodeProcess(info.hProcess, &mut code);
            let _ = CloseHandle(info.hProcess);
            Some(code)
        }
    }
}
