//! Narrador e outros leitores de tela (UI Automation). Cada controle clicável à vista no último
//! desenho vira um elemento filho da janela, com nome, papel e, nas caixas de marcar, o estado. O
//! foco é o controle sob o cursor enquanto o teclado ou o controle navegam (o anel azul).
//!
//! shortcut: os filhos são índices do último desenho, então um leitor que guarde um elemento de um
//! desenho para o outro pode cair em outro controle; ids estáveis por controle resolveriam.

#![allow(non_upper_case_globals)] // as constantes da UIA no match.

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Mutex;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::Com::SAFEARRAY;
use windows::Win32::System::Ole::{SafeArrayCreateVector, SafeArrayPutElement};
use windows::Win32::System::Variant::{VARENUM, VARIANT, VARIANT_0_0_0, VT_BOOL, VT_BSTR, VT_I4};
use windows::Win32::UI::Accessibility::*;

use super::gfx::Rect;

/// Papel do controle, como o leitor de tela anuncia.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Role {
    #[default]
    Button,
    Check,
    Tab,
    Edit,
    Item,
    Combo,
}

/// Nome, papel e estado (caixa de marcar) de um controle.
#[derive(Clone, Debug, Default)]
pub struct Node {
    pub name: String,
    pub role: Role,
    /// Caixa de marcar: marcada. Aba ou página: a escolhida.
    pub on: Option<bool>,
}

struct Tree {
    hwnd: isize,
    rects: Vec<UiaRect>,
    nodes: Vec<Node>,
    focus: Option<usize>,
}

static TREE: Mutex<Tree> = Mutex::new(Tree { hwnd: 0, rects: Vec::new(), nodes: Vec::new(), focus: None });
/// Algum leitor já pediu a árvore: só então os nomes são guardados a cada desenho.
static ON: AtomicBool = AtomicBool::new(false);

pub fn on() -> bool {
    ON.load(Relaxed)
}

fn tree() -> std::sync::MutexGuard<'static, Tree> {
    TREE.lock().unwrap_or_else(|e| e.into_inner())
}

/// WM_GETOBJECT da janela principal: responde quando quem pede é a UI Automation. Não usa o estado
/// do app, porque chega também no meio do desenho.
pub fn answer(hwnd: HWND, w: WPARAM, l: LPARAM) -> Option<LRESULT> {
    if l.0 as i32 != UiaRootObjectId {
        return None;
    }
    tree().hwnd = hwnd.0 as isize;
    if !ON.swap(true, Relaxed) {
        // Primeiro pedido: redesenha para os nomes entrarem.
        unsafe {
            let _ = windows::Win32::Graphics::Gdi::InvalidateRect(Some(hwnd), None, false);
        }
    }
    let root: IRawElementProviderSimple = Root.into();
    Some(unsafe { UiaReturnRawElementProvider(hwnd, w, l, &root) })
}

/// Troca a árvore pelos controles deste desenho (`r` em px da janela, sem escala) e avisa o leitor
/// quando o foco muda de controle.
pub fn publish(hwnd: HWND, scale: f32, items: Vec<(Rect, Node)>, focus: Option<usize>) {
    let mut o = POINT::default();
    unsafe {
        let _ = ClientToScreen(hwnd, &mut o);
    }
    let s = scale as f64;
    let moved = {
        let mut t = tree();
        let was = t.focus.and_then(|i| t.nodes.get(i)).map(|n| n.name.clone());
        let old = t.focus;
        t.hwnd = hwnd.0 as isize;
        t.rects = items.iter().map(|(r, _)| UiaRect { left: o.x as f64 + r.x as f64 * s, top: o.y as f64 + r.y as f64 * s, width: r.w as f64 * s, height: r.h as f64 * s }).collect();
        t.nodes = items.into_iter().map(|(_, n)| n).collect();
        t.focus = focus;
        let now = focus.and_then(|i| t.nodes.get(i)).map(|n| n.name.clone());
        focus.is_some() && (old != focus || was != now)
    };
    // Fora do cadeado: o leitor pode chamar de volta na hora.
    if let Some(i) = focus.filter(|_| moved && unsafe { UiaClientsAreListening() }.as_bool()) {
        let p: IRawElementProviderSimple = Item(i).into();
        let _ = unsafe { UiaRaiseAutomationEvent(&p, UIA_AutomationFocusChangedEventId) };
    }
}

fn var(vt: VARENUM, set: impl FnOnce(&mut VARIANT_0_0_0)) -> VARIANT {
    let mut v = VARIANT::default();
    unsafe {
        let x = &mut *v.Anonymous.Anonymous;
        x.vt = vt;
        set(&mut x.Anonymous);
    }
    v
}

fn item(i: usize) -> Result<IRawElementProviderFragment> {
    if i < tree().nodes.len() {
        Ok(Item(i).into())
    } else {
        Err(Error::empty())
    }
}

#[implement(IRawElementProviderSimple, IRawElementProviderFragment, IRawElementProviderFragmentRoot)]
struct Root;

impl IRawElementProviderSimple_Impl for Root_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }
    fn GetPatternProvider(&self, _: UIA_PATTERN_ID) -> Result<IUnknown> {
        Err(Error::empty())
    }
    fn GetPropertyValue(&self, _: UIA_PROPERTY_ID) -> Result<VARIANT> {
        Ok(VARIANT::default())
    }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        let h = tree().hwnd;
        unsafe { UiaHostProviderFromHwnd(HWND(h as *mut _)) }
    }
}

impl IRawElementProviderFragment_Impl for Root_Impl {
    fn Navigate(&self, d: NavigateDirection) -> Result<IRawElementProviderFragment> {
        let n = tree().nodes.len();
        match d {
            NavigateDirection_FirstChild => item(0),
            NavigateDirection_LastChild if n > 0 => item(n - 1),
            _ => Err(Error::empty()),
        }
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        Ok(UiaRect::default())
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        Ok(())
    }
    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(Root.into())
    }
}

impl IRawElementProviderFragmentRoot_Impl for Root_Impl {
    fn ElementProviderFromPoint(&self, x: f64, y: f64) -> Result<IRawElementProviderFragment> {
        let hit = tree().rects.iter().rposition(|r| x >= r.left && x < r.left + r.width && y >= r.top && y < r.top + r.height);
        match hit {
            Some(i) => item(i),
            None => Ok(Root.into()),
        }
    }
    fn GetFocus(&self) -> Result<IRawElementProviderFragment> {
        // O cadeado solta antes de `item`, que o pega de novo.
        let focus = tree().focus;
        match focus {
            Some(i) => item(i),
            None => Err(Error::empty()),
        }
    }
}

/// Um controle do último desenho, pelo índice.
#[implement(IRawElementProviderSimple, IRawElementProviderFragment, IToggleProvider, ISelectionItemProvider)]
struct Item(usize);

impl Item_Impl {
    fn node(&self) -> Option<Node> {
        tree().nodes.get(self.0).cloned()
    }
}

impl IRawElementProviderSimple_Impl for Item_Impl {
    fn ProviderOptions(&self) -> Result<ProviderOptions> {
        Ok(ProviderOptions_ServerSideProvider)
    }
    fn GetPatternProvider(&self, id: UIA_PATTERN_ID) -> Result<IUnknown> {
        match self.node() {
            // Caixa de marcar: ligada ou não. Aba e página: a escolhida.
            Some(n) if n.on.is_some() && n.role == Role::Check && id == UIA_TogglePatternId => IToggleProvider::from(Item(self.0)).cast(),
            Some(n) if n.on.is_some() && n.role != Role::Check && id == UIA_SelectionItemPatternId => ISelectionItemProvider::from(Item(self.0)).cast(),
            _ => Err(Error::empty()),
        }
    }
    fn GetPropertyValue(&self, id: UIA_PROPERTY_ID) -> Result<VARIANT> {
        let Some(n) = self.node() else { return Ok(VARIANT::default()) };
        let focused = tree().focus == Some(self.0);
        let int = |v: i32| var(VT_I4, |x| x.lVal = v);
        let flag = |b: bool| var(VT_BOOL, |x| x.boolVal = if b { VARIANT_TRUE } else { VARIANT_FALSE });
        Ok(match id {
            UIA_NamePropertyId => var(VT_BSTR, |x| x.bstrVal = std::mem::ManuallyDrop::new(BSTR::from(n.name.as_str()))),
            UIA_ControlTypePropertyId => int(match n.role {
                Role::Button => UIA_ButtonControlTypeId,
                Role::Check => UIA_CheckBoxControlTypeId,
                Role::Tab => UIA_TabItemControlTypeId,
                Role::Edit => UIA_EditControlTypeId,
                Role::Item => UIA_ListItemControlTypeId,
                Role::Combo => UIA_ComboBoxControlTypeId,
            }
            .0),
            UIA_IsKeyboardFocusablePropertyId | UIA_IsEnabledPropertyId => flag(true),
            UIA_HasKeyboardFocusPropertyId => flag(focused),
            _ => VARIANT::default(),
        })
    }
    fn HostRawElementProvider(&self) -> Result<IRawElementProviderSimple> {
        Err(Error::empty())
    }
}

impl IRawElementProviderFragment_Impl for Item_Impl {
    fn Navigate(&self, d: NavigateDirection) -> Result<IRawElementProviderFragment> {
        match d {
            NavigateDirection_Parent => Ok(Root.into()),
            NavigateDirection_NextSibling => item(self.0 + 1),
            NavigateDirection_PreviousSibling if self.0 > 0 => item(self.0 - 1),
            _ => Err(Error::empty()),
        }
    }
    fn GetRuntimeId(&self) -> Result<*mut SAFEARRAY> {
        unsafe {
            let a = SafeArrayCreateVector(VT_I4, 0, 2);
            if a.is_null() {
                return Err(E_OUTOFMEMORY.into());
            }
            for (k, v) in [UiaAppendRuntimeId as i32, self.0 as i32 + 1].iter().enumerate() {
                SafeArrayPutElement(a, &(k as i32), v as *const i32 as *const _)?;
            }
            Ok(a)
        }
    }
    fn BoundingRectangle(&self) -> Result<UiaRect> {
        Ok(tree().rects.get(self.0).copied().unwrap_or_default())
    }
    fn GetEmbeddedFragmentRoots(&self) -> Result<*mut SAFEARRAY> {
        Ok(std::ptr::null_mut())
    }
    fn SetFocus(&self) -> Result<()> {
        Ok(())
    }
    fn FragmentRoot(&self) -> Result<IRawElementProviderFragmentRoot> {
        Ok(Root.into())
    }
}

impl IToggleProvider_Impl for Item_Impl {
    fn Toggle(&self) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn ToggleState(&self) -> Result<ToggleState> {
        Ok(if self.node().and_then(|n| n.on) == Some(true) { ToggleState_On } else { ToggleState_Off })
    }
}

impl ISelectionItemProvider_Impl for Item_Impl {
    fn Select(&self) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn AddToSelection(&self) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn RemoveFromSelection(&self) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn IsSelected(&self) -> Result<BOOL> {
        Ok(BOOL::from(self.node().and_then(|n| n.on) == Some(true)))
    }
    fn SelectionContainer(&self) -> Result<IRawElementProviderSimple> {
        Err(Error::empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::gfx::rect;

    #[test]
    fn tree_gives_name_role_state_and_focus() {
        let node = |name: &str, role, on| Node { name: name.into(), role, on };
        publish(HWND::default(), 2.0, vec![(rect(1.0, 2.0, 3.0, 4.0), node("Guia", Role::Button, None)), (rect(10.0, 0.0, 5.0, 5.0), node("Som", Role::Check, Some(true)))], Some(1));
        let root: IRawElementProviderFragmentRoot = Root.into();
        unsafe {
            let focus: IRawElementProviderSimple = root.GetFocus().unwrap().cast().unwrap();
            let name = focus.GetPropertyValue(UIA_NamePropertyId).unwrap();
            assert_eq!(name.Anonymous.Anonymous.Anonymous.bstrVal.to_string(), "Som");
            let kind = focus.GetPropertyValue(UIA_ControlTypePropertyId).unwrap();
            assert_eq!(kind.Anonymous.Anonymous.Anonymous.lVal, UIA_CheckBoxControlTypeId.0);
            let toggle: IToggleProvider = focus.GetPatternProvider(UIA_TogglePatternId).unwrap().cast().unwrap();
            assert_eq!(toggle.ToggleState().unwrap(), ToggleState_On);
            // Sem janela, a origem fica em 0: só a escala conta.
            let first = root.cast::<IRawElementProviderFragment>().unwrap().Navigate(NavigateDirection_FirstChild).unwrap();
            let r = first.BoundingRectangle().unwrap();
            assert_eq!((r.left, r.top, r.width, r.height), (2.0, 4.0, 6.0, 8.0));
        }
    }
}
