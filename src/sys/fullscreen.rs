//! Fullscreenizer: tira as bordas e põe a janela no monitor, mantendo a proporção (barras pretas atrás),
//! esticada ou em escala inteira. O mesmo atalho desfaz.

use std::collections::HashMap;

use windows::core::w;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{ITaskbarList2, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::*;

/// Como a imagem do jogo ocupa o monitor.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Fit {
    /// Mantém a proporção do jogo, com barras pretas nas sobras.
    #[default]
    Aspect,
    /// Cobre o monitor inteiro (o comportamento até a 1.10).
    Stretch,
    /// Maior múltiplo inteiro do tamanho do jogo que cabe: pixels iguais, mais barra.
    Integer,
}

impl Fit {
    pub const ALL: [Fit; 3] = [Fit::Aspect, Fit::Stretch, Fit::Integer];
    pub fn key(self) -> &'static str {
        match self {
            Fit::Aspect => "aspect",
            Fit::Stretch => "stretch",
            Fit::Integer => "integer",
        }
    }
    pub fn parse(s: &str) -> Self {
        Self::ALL.into_iter().find(|f| f.key() == s).unwrap_or_default()
    }
}

/// Retângulo da janela no monitor `mon` para um jogo de `w`×`h`, centralizado. Jogo maior que o monitor na
/// escala inteira cai para a proporção.
pub fn fit_rect(mon: RECT, (w, h): (i32, i32), fit: Fit) -> RECT {
    let (mw, mh) = (mon.right - mon.left, mon.bottom - mon.top);
    if fit == Fit::Stretch || w <= 0 || h <= 0 {
        return mon;
    }
    let k = (mw / w).min(mh / h);
    let (fw, fh) = if fit == Fit::Integer && k >= 1 {
        (w * k, h * k)
    } else if mw as i64 * h as i64 <= mh as i64 * w as i64 {
        (mw, (mw as i64 * h as i64 / w as i64) as i32)
    } else {
        ((mh as i64 * w as i64 / h as i64) as i32, mh)
    };
    let (x, y) = (mon.left + (mw - fw) / 2, mon.top + (mh - fh) / 2);
    RECT { left: x, top: y, right: x + fw, bottom: y + fh }
}

#[derive(Clone)]
struct Saved {
    /// Caminho do exe, para o app desligar o filtro quando o jogo fecha.
    path: String,
    style: i32,
    ex: i32,
    rect: RECT,
    maximized: bool,
    /// Janela preta atrás do jogo quando ele não cobre o monitor.
    backdrop: Option<isize>,
}

/// O que o `apply` viu do jogo antes de mexer: o tamanho da área cliente (a resolução da janela), o do monitor
/// e se a janela aceitava redimensionar (jogo que desenha no tamanho da janela).
#[derive(Clone, Copy, Debug)]
pub struct Applied {
    pub native: (i32, i32),
    pub monitor: (i32, i32),
    pub resizable: bool,
}

#[derive(Default)]
pub struct Fullscreenizer {
    saved: HashMap<isize, Saved>,
}

const STYLE_OFF: u32 = WS_CAPTION.0 | WS_THICKFRAME.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0 | WS_SYSMENU.0;
const EX_OFF: u32 = WS_EX_DLGMODALFRAME.0 | WS_EX_WINDOWEDGE.0 | WS_EX_CLIENTEDGE.0 | WS_EX_STATICEDGE.0;

fn monitor_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let m = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(m, &mut mi).as_bool().then_some(mi.rcMonitor)
    }
}

unsafe extern "system" fn backdrop_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    // Clique na barra não tira o foco do jogo.
    if m == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    DefWindowProcW(h, m, w, l)
}

/// Janela preta do tamanho do monitor, logo abaixo do jogo.
fn backdrop(game: HWND, mon: RECT) -> Option<HWND> {
    unsafe {
        let class = w!("PrismaFullscreenBars");
        let wc = WNDCLASSW { lpfnWndProc: Some(backdrop_proc), lpszClassName: class, hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0), ..Default::default() };
        RegisterClassW(&wc);
        let ex = WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        let h = CreateWindowExW(ex, class, w!("Prisma"), WS_POPUP, mon.left, mon.top, mon.right - mon.left, mon.bottom - mon.top, None, None, None, None).ok()?;
        let _ = SetWindowPos(h, Some(game), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        Some(h)
    }
}

/// Avisa a barra de tarefas que a janela é tela cheia: sem isso ela fica por cima do jogo que não cobre o
/// monitor inteiro.
fn mark_fullscreen(hwnd: HWND, on: bool) {
    unsafe {
        if let Ok(t) = CoCreateInstance::<_, ITaskbarList2>(&TaskbarList, None, CLSCTX_INPROC_SERVER) {
            if t.HrInit().is_ok() {
                let _ = t.MarkFullscreenWindow(hwnd, on);
            }
        }
    }
}

impl Fullscreenizer {
    pub fn is_on(&self, hwnd: HWND) -> bool {
        self.saved.contains_key(&(hwnd.0 as isize))
    }

    /// Alterna a janela. Devolve Some(Some(..)) ao ligar e Some(None) ao restaurar.
    pub fn toggle(&mut self, hwnd: HWND, fit: Fit) -> Option<Option<Applied>> {
        if hwnd.is_invalid() {
            return None;
        }
        unsafe {
            let shell = GetShellWindow();
            let desktop = GetDesktopWindow();
            if hwnd == shell || hwnd == desktop {
                return None;
            }
        }
        if self.is_on(hwnd) {
            self.restore(hwnd);
            Some(None)
        } else {
            self.apply(hwnd, fit).map(Some)
        }
    }

    pub fn apply(&mut self, hwnd: HWND, fit: Fit) -> Option<Applied> {
        unsafe {
            let mon = monitor_rect(hwnd)?;
            let style = GetWindowLongW(hwnd, GWL_STYLE);
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE);
            let mut rect = RECT::default();
            GetWindowRect(hwnd, &mut rect).ok()?;
            let maximized = IsZoomed(hwnd).as_bool();
            if maximized {
                let _ = ShowWindow(hwnd, SW_RESTORE);
                let _ = GetWindowRect(hwnd, &mut rect);
            }
            // A área cliente antes de mexer é a resolução em que o jogo está em janela.
            let mut cr = RECT::default();
            let _ = GetClientRect(hwnd, &mut cr);
            let native = (cr.right, cr.bottom);
            let to = fit_rect(mon, native, fit);
            let bars = to != mon;
            let back = if bars { backdrop(hwnd, mon) } else { None };
            let path = super::procs::image_path(super::procs::window_pid(hwnd)).unwrap_or_default();
            self.saved.insert(hwnd.0 as isize, Saved { path, style, ex, rect, maximized, backdrop: back.map(|b| b.0 as isize) });
            SetWindowLongW(hwnd, GWL_STYLE, style & !(STYLE_OFF as i32));
            SetWindowLongW(hwnd, GWL_EXSTYLE, ex & !(EX_OFF as i32));
            // Janela que não deixa mexer (de outro usuário, elevada): volta como estava, sem barras sobrando.
            if SetWindowPos(hwnd, Some(HWND_TOP), to.left, to.top, to.right - to.left, to.bottom - to.top, SWP_FRAMECHANGED | SWP_SHOWWINDOW).is_err() {
                self.restore(hwnd);
                return None;
            }
            if bars {
                mark_fullscreen(hwnd, true);
            }
            Some(Applied { native, monitor: (mon.right - mon.left, mon.bottom - mon.top), resizable: style as u32 & WS_THICKFRAME.0 != 0 })
        }
    }

    pub fn restore(&mut self, hwnd: HWND) {
        let Some(s) = self.saved.remove(&(hwnd.0 as isize)) else { return };
        unsafe {
            if let Some(b) = s.backdrop {
                let _ = DestroyWindow(HWND(b as *mut _));
                mark_fullscreen(hwnd, false);
            }
            SetWindowLongW(hwnd, GWL_STYLE, s.style);
            SetWindowLongW(hwnd, GWL_EXSTYLE, s.ex);
            let _ = SetWindowPos(
                hwnd,
                None,
                s.rect.left,
                s.rect.top,
                s.rect.right - s.rect.left,
                s.rect.bottom - s.rect.top,
                SWP_FRAMECHANGED | SWP_NOZORDER | SWP_SHOWWINDOW,
            );
            if s.maximized {
                let _ = ShowWindow(hwnd, SW_MAXIMIZE);
            }
        }
    }

    /// As barras somem com o jogo minimizado (senão cobrem a área de trabalho de preto) e, com ele na frente,
    /// voltam para logo abaixo dele.
    pub fn follow(&self, fg: HWND) {
        for (&h, s) in &self.saved {
            let Some(b) = s.backdrop else { continue };
            unsafe {
                let (game, b) = (HWND(h as *mut _), HWND(b as *mut _));
                if IsIconic(game).as_bool() {
                    let _ = ShowWindow(b, SW_HIDE);
                } else if game == fg {
                    let _ = SetWindowPos(b, Some(game), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                }
            }
        }
    }

    /// Esquece janelas que já fecharam (e fecha as barras delas). Devolve o exe de cada uma.
    pub fn sweep(&mut self) -> Vec<String> {
        let mut gone = Vec::new();
        self.saved.retain(|h, s| {
            let alive = unsafe { IsWindow(Some(HWND(*h as *mut _))).as_bool() };
            if !alive {
                if let Some(b) = s.backdrop {
                    unsafe {
                        let _ = DestroyWindow(HWND(b as *mut _));
                    }
                }
                gone.push(std::mem::take(&mut s.path));
            }
            alive
        });
        gone
    }
}

/// True quando a janela cobre o monitor inteiro, com ou sem bordas.
pub fn covers_monitor(hwnd: HWND) -> bool {
    unsafe {
        let Some(mon) = monitor_rect(hwnd) else { return false };
        // Janela maximizada com barra de título (navegador, pastas) não é tela cheia, mesmo cobrindo
        // o monitor sem barra de tarefas (escondida ou em outro monitor).
        let caption = WS_CAPTION.0;
        if IsZoomed(hwnd).as_bool() && GetWindowLongW(hwnd, GWL_STYLE) as u32 & caption == caption {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(hwnd, &mut r).is_err() {
            return false;
        }
        r.left <= mon.left && r.top <= mon.top && r.right >= mon.right && r.bottom >= mon.bottom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: RECT = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };

    #[test]
    fn proporcao() {
        // 800×600 (4:3) num 1080p: 1440×1080 no meio, barras de 240 dos lados.
        assert_eq!(fit_rect(MON, (800, 600), Fit::Aspect), RECT { left: 240, top: 0, right: 1680, bottom: 1080 });
        // Mais largo que o monitor (21:9): barras em cima e embaixo.
        assert_eq!(fit_rect(MON, (2560, 1080), Fit::Aspect), RECT { left: 0, top: 135, right: 1920, bottom: 945 });
        assert_eq!(fit_rect(MON, (800, 600), Fit::Stretch), MON);
        // Escala inteira: 640×480 ×2 = 1280×960 (×3 passaria da altura).
        assert_eq!(fit_rect(MON, (640, 480), Fit::Integer), RECT { left: 320, top: 60, right: 1600, bottom: 1020 });
        // Jogo maior que o monitor não tem múltiplo inteiro: cai para a proporção.
        assert_eq!(fit_rect(MON, (2560, 1440), Fit::Integer), MON);
        // Segundo monitor à direita: o meio é o dele.
        let m2 = RECT { left: 1920, top: 0, right: 3840, bottom: 1080 };
        assert_eq!(fit_rect(m2, (800, 600), Fit::Aspect).left, 2160);
        assert_eq!(fit_rect(MON, (0, 0), Fit::Aspect), MON, "sem tamanho, cobre tudo");
    }

    #[test]
    fn modo_pelo_nome() {
        for f in Fit::ALL {
            assert_eq!(Fit::parse(f.key()), f);
        }
        assert_eq!(Fit::parse(""), Fit::Aspect, "config antiga mantém a proporção");
    }
}
