//! HDR no Vulkan. A mesma DLL do gancho entra no jogo como camada do Vulkan: `setup` escreve o manifesto ao lado
//! dela e põe a camada nas variáveis de ambiente do próprio processo, antes de o jogo rodar a primeira linha. Nada
//! fica registrado no Windows. A cadeia de verdade nasce FP16 em sRGB estendido; o jogo desenha em imagens nossas
//! no formato que ele pediu, e no `vkQueuePresentKHR` o passe do `hdr.rs` (o mesmo HLSL, em SPIR-V) leva o quadro
//! para a cadeia, na fila do jogo.
//! shortcut: carregador do Vulkan antigo (sem `VK_ADD_LAYER_PATH`), `vulkan-1.dll` própria do jogo e caminho com
//! letras fora do ASCII ficam sem HDR; trocar por camada implícita só se aparecer jogo assim. Processos filhos
//! herdam as variáveis e carregam a camada, mas sem HDR pedido ela só repassa as chamadas.

use crate::hdr::{self, Params, INSIDE};
use crate::{scan, Ptr, Shared};
use ash::vk::{self, Handle};
use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering::*};
use std::sync::{Mutex, MutexGuard};
use windows::core::HRESULT;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;

const LAYER: &str = "VK_LAYER_PRISMA_hdr";
const VS: &[u8] = include_bytes!("hdr_vs.spv");
const PS: &[u8] = include_bytes!("hdr_ps.spv");
const CS: &[u8] = include_bytes!("hdr_cs.spv");
const MAX: usize = 8;
const FP16: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
const SCRGB: vk::ColorSpaceKHR = vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT;

/// Põe a camada no jogo. Roda com o jogo ainda parado, então o carregador do Vulkan já acha tudo.
pub(crate) unsafe fn setup(module: Ptr) {
    let mut buf = [0u16; 1024];
    let n = GetModuleFileNameW(Some(HMODULE(module)), &mut buf) as usize;
    let Ok(dll) = String::from_utf16(&buf[..n]) else { return };
    let Some(name) = dll.rsplit('\\').next().filter(|s| n > 0 && n < buf.len() && s.is_ascii() && dll.is_ascii()) else { return };
    let json = format!("{}json", dll.trim_end_matches("dll"));
    // O caminho da biblioteca é relativo ao manifesto: a pasta do Prisma pode mudar de lugar.
    let text = format!(
        r#"{{"file_format_version":"1.2.0","layer":{{"name":"{LAYER}","type":"GLOBAL","library_path":".\\{name}","api_version":"1.4.309","implementation_version":"1","description":"Prisma HDR"}}}}"#
    );
    if std::fs::read_to_string(&json).ok().as_deref() != Some(text.as_str()) && std::fs::write(&json, &text).is_err() {
        return;
    }
    let add = |var: &str, v: &str| {
        let old = std::env::var(var).unwrap_or_default();
        std::env::set_var(var, if old.is_empty() { v.to_string() } else { format!("{old};{v}") });
    };
    add("VK_ADD_LAYER_PATH", &json);
    add("VK_INSTANCE_LAYERS", LAYER);
}

// --- O que o carregador passa às camadas (vk_layer.h). ---

const LOADER_INSTANCE_CREATE_INFO: i32 = 47;
const LOADER_DEVICE_CREATE_INFO: i32 = 48;
const LAYER_LINK_INFO: i32 = 0;
const LOADER_DATA_CALLBACK: i32 = 1;

#[repr(C)]
struct LinkInfo {
    s_type: i32,
    p_next: *const c_void,
    function: i32,
    u: *mut c_void,
}

#[repr(C)]
struct InstanceLink {
    next: *mut InstanceLink,
    gipa: vk::PFN_vkGetInstanceProcAddr,
    gpdpa: *const c_void,
}

#[repr(C)]
struct DeviceLink {
    next: *mut DeviceLink,
    gipa: vk::PFN_vkGetInstanceProcAddr,
    gdpa: vk::PFN_vkGetDeviceProcAddr,
}

#[repr(C)]
pub struct Negotiate {
    s_type: i32,
    p_next: *mut c_void,
    version: u32,
    gipa: Option<vk::PFN_vkGetInstanceProcAddr>,
    gdpa: Option<vk::PFN_vkGetDeviceProcAddr>,
    gpdpa: *const c_void,
}

type SetLoaderData = unsafe extern "system" fn(vk::Device, *mut c_void) -> vk::Result;

#[no_mangle]
pub unsafe extern "system" fn vkNegotiateLoaderLayerInterfaceVersion(n: *mut Negotiate) -> vk::Result {
    let Some(n) = n.as_mut().filter(|n| n.version >= 2) else { return vk::Result::ERROR_INITIALIZATION_FAILED };
    n.version = 2;
    n.gipa = Some(gipa);
    n.gdpa = Some(gdpa);
    n.gpdpa = std::ptr::null();
    vk::Result::SUCCESS
}

unsafe fn link_info(mut p: *const c_void, s_type: i32, function: i32) -> Option<&'static mut LinkInfo> {
    while let Some(l) = (p as *mut LinkInfo).as_mut() {
        if l.s_type == s_type && l.function == function {
            return Some(l);
        }
        p = l.p_next;
    }
    None
}

// --- Instâncias e dispositivos, pela chave de despacho (a primeira palavra do objeto). ---

/// Tabela sem trava para os ganchos de gravação. O valor vaza: algumas centenas de bytes por dispositivo criado.
struct Table<T: 'static> {
    keys: [AtomicUsize; 32],
    vals: [AtomicPtr<T>; 32],
}

impl<T> Table<T> {
    const fn new() -> Self {
        Self { keys: [const { AtomicUsize::new(0) }; 32], vals: [const { AtomicPtr::new(std::ptr::null_mut()) }; 32] }
    }

    fn get(&self, key: usize) -> Option<&'static T> {
        let i = self.keys.iter().position(|k| k.load(Acquire) == key)?;
        unsafe { self.vals[i].load(Acquire).as_ref() }
    }

    fn put(&self, key: usize, v: T) {
        let Some(i) = self.keys.iter().position(|k| k.compare_exchange(0, usize::MAX, AcqRel, Relaxed).is_ok()) else { return };
        self.vals[i].store(Box::leak(Box::new(v)), Release);
        self.keys[i].store(key, Release);
    }

    fn remove(&self, key: usize) {
        if let Some(k) = self.keys.iter().find(|k| k.load(Acquire) == key) {
            k.store(0, Release);
        }
    }
}

fn key(handle: u64) -> usize {
    if handle == 0 {
        return 0;
    }
    unsafe { *(handle as usize as *const usize) }
}

struct Inst {
    gipa: vk::PFN_vkGetInstanceProcAddr,
    fns: ash::Instance,
    surface: ash::khr::surface::InstanceFn,
    win32: ash::khr::win32_surface::InstanceFn,
    /// `VK_EXT_swapchain_colorspace` ligado: sem ele não há sRGB estendido.
    space: bool,
}

struct Dev {
    gdpa: vk::PFN_vkGetDeviceProcAddr,
    fns: ash::Device,
    swap: ash::khr::swapchain::DeviceFn,
    phys: vk::PhysicalDevice,
    inst: &'static Inst,
    loader_data: Option<SetLoaderData>,
    queues: Vec<(vk::Queue, u32)>,
    families: Vec<vk::QueueFamilyProperties>,
    mem: vk::PhysicalDeviceMemoryProperties,
    period: f32,
}

static INSTANCES: Table<Inst> = Table::new();
static DEVICES: Table<Dev> = Table::new();
static SURFACES: Mutex<Option<HashMap<u64, usize>>> = Mutex::new(None);
static CHAINS: Mutex<Vec<Chain>> = Mutex::new(Vec::new());

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn dev_of(handle: u64) -> Option<&'static Dev> {
    DEVICES.get(key(handle))
}

/// Nossa versão de cada função; as do HDR só quando o Prisma pediu HDR neste jogo.
fn ours(name: &[u8]) -> Option<vk::PFN_vkVoidFunction> {
    let f = match name {
        b"vkGetInstanceProcAddr" => gipa as *const (),
        b"vkGetDeviceProcAddr" => gdpa as *const (),
        b"vkCreateInstance" => create_instance as *const (),
        b"vkDestroyInstance" => destroy_instance as *const (),
        b"vkCreateDevice" => create_device as *const (),
        b"vkDestroyDevice" => destroy_device as *const (),
        _ if !hdr::wanted() => return None,
        b"vkCreateWin32SurfaceKHR" => create_surface as *const (),
        b"vkCreateSwapchainKHR" => create_swapchain as *const (),
        b"vkDestroySwapchainKHR" => destroy_swapchain as *const (),
        b"vkGetSwapchainImagesKHR" => get_images as *const (),
        b"vkQueuePresentKHR" => present as *const (),
        b"vkCreateImageView" => create_view as *const (),
        b"vkCreateFramebuffer" => create_framebuffer as *const (),
        b"vkCreateRenderPass" => create_pass as *const (),
        b"vkCreateRenderPass2" | b"vkCreateRenderPass2KHR" => create_pass2 as *const (),
        b"vkCmdBeginRenderPass" => begin_pass as *const (),
        b"vkCmdBeginRenderPass2" | b"vkCmdBeginRenderPass2KHR" => begin_pass2 as *const (),
        b"vkCmdBeginRendering" | b"vkCmdBeginRenderingKHR" => begin_rendering as *const (),
        b"vkCmdEndRenderPass" => end_pass as *const (),
        b"vkCmdEndRenderPass2" | b"vkCmdEndRenderPass2KHR" => end_pass2 as *const (),
        b"vkCmdEndRendering" | b"vkCmdEndRenderingKHR" => end_rendering as *const (),
        b"vkCmdDraw" => draw as *const (),
        b"vkCmdDrawIndexed" => draw_indexed as *const (),
        b"vkCmdSetViewport" => set_viewport as *const (),
        _ => return None,
    };
    Some(unsafe { std::mem::transmute::<*const (), vk::PFN_vkVoidFunction>(f) })
}

unsafe extern "system" fn gipa(instance: vk::Instance, name: *const c_char) -> vk::PFN_vkVoidFunction {
    let n = CStr::from_ptr(name).to_bytes();
    if matches!(n, b"vkGetInstanceProcAddr" | b"vkCreateInstance") {
        return ours(n).flatten();
    }
    let inst = INSTANCES.get(key(instance.as_raw()))?;
    // Só o que a camada de baixo tem: o jogo pergunta para saber se existe.
    let next = (inst.gipa)(instance, name)?;
    ours(n).flatten().or(Some(next))
}

unsafe extern "system" fn gdpa(device: vk::Device, name: *const c_char) -> vk::PFN_vkVoidFunction {
    let dev = dev_of(device.as_raw())?;
    let n = CStr::from_ptr(name).to_bytes();
    if n == b"vkGetDeviceProcAddr" {
        return ours(n).flatten();
    }
    let next = (dev.gdpa)(device, name)?;
    ours(n).flatten().or(Some(next))
}

unsafe extern "system" fn create_instance(info: *const vk::InstanceCreateInfo, alloc: *const vk::AllocationCallbacks, out: *mut vk::Instance) -> vk::Result {
    let Some(link) = link_info((*info).p_next, LOADER_INSTANCE_CREATE_INFO, LAYER_LINK_INFO) else { return vk::Result::ERROR_INITIALIZATION_FAILED };
    let l = &*(link.u as *const InstanceLink);
    let (gipa_next, below) = (l.gipa, l.next);
    link.u = below as *mut c_void;
    let Some(create) = gipa_next(vk::Instance::null(), c"vkCreateInstance".as_ptr()) else { return vk::Result::ERROR_INITIALIZATION_FAILED };
    let create: vk::PFN_vkCreateInstance = std::mem::transmute(create);

    let i = &*info;
    let exts: Vec<*const c_char> = slice(i.pp_enabled_extension_names, i.enabled_extension_count).to_vec();
    let has = |e: &CStr| exts.iter().any(|p| CStr::from_ptr(*p) == e);
    let mut space = has(vk::EXT_SWAPCHAIN_COLORSPACE_NAME);
    let mut r = vk::Result::ERROR_EXTENSION_NOT_PRESENT;
    if !space && hdr::wanted() && has(vk::KHR_SURFACE_NAME) {
        let mut more = exts.clone();
        more.push(vk::EXT_SWAPCHAIN_COLORSPACE_NAME.as_ptr());
        let mut with = *i;
        with.enabled_extension_count = more.len() as u32;
        with.pp_enabled_extension_names = more.as_ptr();
        r = create(&with, alloc, out);
        space = r == vk::Result::SUCCESS;
        // As camadas de baixo já andaram no elo: volta para a próxima antes de tentar de novo.
        link.u = below as *mut c_void;
    }
    if r != vk::Result::SUCCESS {
        r = create(info, alloc, out);
    }
    if r == vk::Result::SUCCESS {
        let instance = *out;
        let load = |n: &CStr| gipa_next(instance, n.as_ptr()).map_or(std::ptr::null(), |f| f as *const c_void);
        INSTANCES.put(
            key(instance.as_raw()),
            Inst {
                gipa: gipa_next,
                fns: ash::Instance::load_with(load, instance),
                surface: ash::khr::surface::InstanceFn::load(load),
                win32: ash::khr::win32_surface::InstanceFn::load(load),
                space,
            },
        );
    }
    r
}

unsafe extern "system" fn destroy_instance(instance: vk::Instance, alloc: *const vk::AllocationCallbacks) {
    let k = key(instance.as_raw());
    if let Some(inst) = INSTANCES.get(k) {
        INSTANCES.remove(k);
        (inst.fns.fp_v1_0().destroy_instance)(instance, alloc);
    }
}

unsafe extern "system" fn create_device(phys: vk::PhysicalDevice, info: *const vk::DeviceCreateInfo, alloc: *const vk::AllocationCallbacks, out: *mut vk::Device) -> vk::Result {
    let fail = vk::Result::ERROR_INITIALIZATION_FAILED;
    // O objeto da placa usa a chave da instância.
    let (Some(inst), Some(link)) = (INSTANCES.get(key(phys.as_raw())), link_info((*info).p_next, LOADER_DEVICE_CREATE_INFO, LAYER_LINK_INFO)) else { return fail };
    let l = &*(link.u as *const DeviceLink);
    let (gipa_next, gdpa_next) = (l.gipa, l.gdpa);
    link.u = l.next as *mut c_void;
    let Some(create) = gipa_next(vk::Instance::null(), c"vkCreateDevice".as_ptr()) else { return fail };
    let create: vk::PFN_vkCreateDevice = std::mem::transmute(create);
    let r = create(phys, info, alloc, out);
    if r != vk::Result::SUCCESS {
        return r;
    }
    let device = *out;
    let loader_data = link_info((*info).p_next, LOADER_DEVICE_CREATE_INFO, LOADER_DATA_CALLBACK).map(|l| std::mem::transmute::<*mut c_void, SetLoaderData>(l.u));
    // Dispositivo 1.0 ou 1.1 com a extensão: o nome com KHR no fim.
    let load = |n: &CStr| {
        let core = gdpa_next(device, n.as_ptr());
        let f = core.or_else(|| {
            let mut khr = n.to_bytes().to_vec();
            khr.extend_from_slice(b"KHR\0");
            gdpa_next(device, khr.as_ptr() as *const c_char)
        });
        f.map_or(std::ptr::null(), |f| f as *const c_void)
    };
    let fns = ash::Device::load_with(load, device);
    let mut queues = Vec::new();
    let i = &*info;
    for q in slice(i.p_queue_create_infos, i.queue_create_info_count).iter().filter(|q| q.flags.is_empty()) {
        for k in 0..q.queue_count {
            queues.push((fns.get_device_queue(q.queue_family_index, k), q.queue_family_index));
        }
    }
    DEVICES.put(
        key(device.as_raw()),
        Dev {
            gdpa: gdpa_next,
            swap: ash::khr::swapchain::DeviceFn::load(load),
            fns,
            phys,
            inst,
            loader_data,
            queues,
            families: inst.fns.get_physical_device_queue_family_properties(phys),
            mem: inst.fns.get_physical_device_memory_properties(phys),
            period: inst.fns.get_physical_device_properties(phys).limits.timestamp_period,
        },
    );
    r
}

unsafe extern "system" fn destroy_device(device: vk::Device, alloc: *const vk::AllocationCallbacks) {
    let k = key(device.as_raw());
    let Some(dev) = DEVICES.get(k) else { return };
    // Cadeia que o jogo esqueceu: o que é nosso sai antes do dispositivo.
    let gone: Vec<Chain> = {
        let mut list = lock(&CHAINS);
        let (gone, keep) = std::mem::take(&mut *list).into_iter().partition(|c| c.dev.fns.handle() == device);
        *list = keep;
        gone
    };
    drop(gone);
    DEVICES.remove(k);
    (dev.fns.fp_v1_0().destroy_device)(device, alloc);
}

unsafe fn slice<'a, T>(p: *const T, n: u32) -> &'a [T] {
    if p.is_null() || n == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(p, n as usize)
    }
}

unsafe extern "system" fn create_surface(instance: vk::Instance, info: *const vk::Win32SurfaceCreateInfoKHR, alloc: *const vk::AllocationCallbacks, out: *mut vk::SurfaceKHR) -> vk::Result {
    let Some(inst) = INSTANCES.get(key(instance.as_raw())) else { return vk::Result::ERROR_INITIALIZATION_FAILED };
    let r = (inst.win32.create_win32_surface_khr)(instance, info, alloc, out);
    if r == vk::Result::SUCCESS {
        lock(&SURFACES).get_or_insert_with(HashMap::new).insert((*out).as_raw(), (*info).hwnd as usize);
    }
    r
}

// --- A cadeia. ---

/// O buffer do jogo que separa a interface: as imagens do proxy mais novo, a cena e o tamanho (largura << 32 | altura).
/// shortcut: um proxy só separa (o mais novo), como no D3D12; e a cena é uma só para os quadros em voo, então um
/// quadro pode sair com a cena do seguinte. Separar por quadro se aparecer interface tremendo.
static IMAGES: [AtomicU64; MAX] = [const { AtomicU64::new(0) }; MAX];
static SCENE: AtomicU64 = AtomicU64::new(0);
static EXTENT: AtomicU64 = AtomicU64::new(0);
static PHASES: [AtomicU32; MAX] = [const { AtomicU32::new(hdr::PHASE_NONE) }; MAX];

#[derive(Default)]
struct Res {
    images: Vec<vk::Image>,
    mems: Vec<vk::DeviceMemory>,
    views: Vec<vk::ImageView>,
    targets: Vec<vk::ImageView>,
    fbs: Vec<vk::Framebuffer>,
    scene: vk::Image,
    scene_mem: vk::DeviceMemory,
    scene_view: vk::ImageView,
    pass: vk::RenderPass,
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    pool: vk::DescriptorPool,
    sets: Vec<vk::DescriptorSet>,
    fences: Vec<vk::Fence>,
    sems: Vec<vk::Semaphore>,
    queries: vk::QueryPool,
    cmd_pool: vk::CommandPool,
    cmds: Vec<vk::CommandBuffer>,
    /// Analisar: o `cs`, o buffer que ele soma e as leituras (uma vaga por imagem, já mapeadas em `read_at`).
    cs_set_layout: vk::DescriptorSetLayout,
    cs_layout: vk::PipelineLayout,
    cs_pipeline: vk::Pipeline,
    cs_pool: vk::DescriptorPool,
    cs_sets: Vec<vk::DescriptorSet>,
    hist: vk::Buffer,
    hist_mem: vk::DeviceMemory,
    read: vk::Buffer,
    read_mem: vk::DeviceMemory,
    read_at: usize,
}

struct Chain {
    real: vk::SwapchainKHR,
    dev: &'static Dev,
    extent: vk::Extent2D,
    srgb: bool,
    mode: u32,
    r: Res,
    scene_ready: bool,
    /// Bits válidos do relógio da fila; zero: sem medida.
    mask: u64,
    ms: f64,
    /// Sem ele o Analisar não mede neste jogo.
    ring: Option<scan::Ring>,
}

unsafe impl Send for Chain {}

impl Drop for Chain {
    fn drop(&mut self) {
        let d = &self.dev.fns;
        let r = &self.r;
        unsafe {
            // Sem cercas quando a montagem falhou antes delas; esperar por zero cercas não vale no Vulkan.
            if !r.fences.is_empty() {
                let _ = d.wait_for_fences(&r.fences, true, 1_000_000_000);
            }
            if self.splits() {
                IMAGES.iter().for_each(|i| i.store(0, Relaxed));
                SCENE.store(0, Relaxed);
            }
            r.fences.iter().for_each(|f| d.destroy_fence(*f, None));
            r.sems.iter().for_each(|s| d.destroy_semaphore(*s, None));
            d.destroy_command_pool(r.cmd_pool, None);
            d.destroy_descriptor_pool(r.cs_pool, None);
            d.destroy_pipeline(r.cs_pipeline, None);
            d.destroy_pipeline_layout(r.cs_layout, None);
            d.destroy_descriptor_set_layout(r.cs_set_layout, None);
            // Memória mapeada sai do mapa ao ser solta.
            [r.hist, r.read].iter().for_each(|b| d.destroy_buffer(*b, None));
            [r.hist_mem, r.read_mem].iter().for_each(|m| d.free_memory(*m, None));
            d.destroy_query_pool(r.queries, None);
            d.destroy_descriptor_pool(r.pool, None);
            d.destroy_pipeline(r.pipeline, None);
            d.destroy_pipeline_layout(r.layout, None);
            d.destroy_descriptor_set_layout(r.set_layout, None);
            r.fbs.iter().for_each(|f| d.destroy_framebuffer(*f, None));
            d.destroy_render_pass(r.pass, None);
            r.views.iter().chain(&r.targets).chain([&r.scene_view]).for_each(|v| d.destroy_image_view(*v, None));
            r.images.iter().chain([&r.scene]).for_each(|i| d.destroy_image(*i, None));
            r.mems.iter().chain([&r.scene_mem]).for_each(|m| d.free_memory(*m, None));
        }
    }
}

fn color() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 }
}

fn barrier(image: vk::Image, old: vk::ImageLayout, new: vk::ImageLayout, src: vk::AccessFlags, dst: vk::AccessFlags) -> vk::ImageMemoryBarrier<'static> {
    vk::ImageMemoryBarrier::default()
        .image(image)
        .old_layout(old)
        .new_layout(new)
        .src_access_mask(src)
        .dst_access_mask(dst)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .subresource_range(color())
}

fn words(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

impl Chain {
    fn splits(&self) -> bool {
        self.r.images.first().is_some_and(|i| IMAGES[0].load(Relaxed) == i.as_raw())
    }

    unsafe fn memory(&self, image: vk::Image) -> ash::prelude::VkResult<vk::DeviceMemory> {
        let d = &self.dev.fns;
        let req = d.get_image_memory_requirements(image);
        let m = &self.dev.mem;
        let ok = |i: &u32| req.memory_type_bits & (1 << i) != 0;
        let local = |i: &u32| m.memory_types[*i as usize].property_flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL);
        let ty = (0..m.memory_type_count).find(|i| ok(i) && local(i)).or_else(|| (0..m.memory_type_count).find(ok));
        let ty = ty.ok_or(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)?;
        let mem = d.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty), None)?;
        if let Err(e) = d.bind_image_memory(image, mem, 0) {
            d.free_memory(mem, None);
            return Err(e);
        }
        Ok(mem)
    }

    /// Memória do tipo `want` presa ao buffer.
    unsafe fn buffer_memory(&self, buf: vk::Buffer, want: vk::MemoryPropertyFlags) -> ash::prelude::VkResult<vk::DeviceMemory> {
        let d = &self.dev.fns;
        let req = d.get_buffer_memory_requirements(buf);
        let m = &self.dev.mem;
        let ty = (0..m.memory_type_count).find(|&i| req.memory_type_bits & (1 << i) != 0 && m.memory_types[i as usize].property_flags.contains(want));
        let ty = ty.ok_or(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)?;
        let mem = d.allocate_memory(&vk::MemoryAllocateInfo::default().allocation_size(req.size).memory_type_index(ty), None)?;
        if let Err(e) = d.bind_buffer_memory(buf, mem, 0) {
            d.free_memory(mem, None);
            return Err(e);
        }
        Ok(mem)
    }

    /// O `cs` do Analisar e os buffers dele, para `n` imagens. Falhar só deixa o Analisar de fora.
    unsafe fn scan_build(&mut self, n: u32) -> ash::prelude::VkResult<()> {
        let d = &self.dev.fns;
        let bind = |b: u32, ty| vk::DescriptorSetLayoutBinding::default().binding(b).descriptor_type(ty).descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE);
        let binds = [bind(0, vk::DescriptorType::SAMPLED_IMAGE), bind(1, vk::DescriptorType::SAMPLED_IMAGE), bind(2, vk::DescriptorType::STORAGE_BUFFER)];
        self.r.cs_set_layout = d.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&binds), None)?;
        let push = [vk::PushConstantRange { stage_flags: vk::ShaderStageFlags::COMPUTE, offset: 0, size: std::mem::size_of::<Params>() as u32 }];
        let layouts = [self.r.cs_set_layout];
        self.r.cs_layout = d.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts).push_constant_ranges(&push), None)?;
        let code = words(CS);
        let module = d.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&code), None)?;
        let stage = vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::COMPUTE).module(module).name(c"cs");
        let made = d.create_compute_pipelines(vk::PipelineCache::null(), &[vk::ComputePipelineCreateInfo::default().stage(stage).layout(self.r.cs_layout)], None);
        d.destroy_shader_module(module, None);
        self.r.cs_pipeline = made.map(|p| p[0]).map_err(|(_, e)| e)?;
        let info = |size, usage| vk::BufferCreateInfo::default().size(size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE);
        self.r.hist = d.create_buffer(&info(scan::BYTES, vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC), None)?;
        self.r.hist_mem = self.buffer_memory(self.r.hist, vk::MemoryPropertyFlags::DEVICE_LOCAL).or_else(|_| self.buffer_memory(self.r.hist, vk::MemoryPropertyFlags::empty()))?;
        self.r.read = d.create_buffer(&info(scan::BYTES * n as u64, vk::BufferUsageFlags::TRANSFER_DST), None)?;
        self.r.read_mem = self.buffer_memory(self.r.read, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
        self.r.read_at = d.map_memory(self.r.read_mem, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as usize;
        let sizes = [vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLED_IMAGE, descriptor_count: 2 * n }, vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: n }];
        self.r.cs_pool = d.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(n).pool_sizes(&sizes), None)?;
        let layouts = vec![self.r.cs_set_layout; n as usize];
        self.r.cs_sets = d.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(self.r.cs_pool).set_layouts(&layouts))?;
        let hist = [vk::DescriptorBufferInfo { buffer: self.r.hist, offset: 0, range: vk::WHOLE_SIZE }];
        for (set, view) in self.r.cs_sets.iter().zip(&self.r.views) {
            let game = [vk::DescriptorImageInfo { sampler: vk::Sampler::null(), image_view: *view, image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL }];
            let scene = [vk::DescriptorImageInfo { image_view: self.r.scene_view, ..game[0] }];
            let write = vk::WriteDescriptorSet::default().dst_set(*set);
            let image = write.descriptor_type(vk::DescriptorType::SAMPLED_IMAGE);
            d.update_descriptor_sets(&[image.dst_binding(0).image_info(&game), image.dst_binding(1).image_info(&scene), write.dst_binding(2).descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&hist)], &[]);
        }
        Ok(())
    }

    unsafe fn view(&self, image: vk::Image, format: vk::Format) -> ash::prelude::VkResult<vk::ImageView> {
        let info = vk::ImageViewCreateInfo::default().image(image).view_type(vk::ImageViewType::TYPE_2D).format(format).subresource_range(color());
        self.dev.fns.create_image_view(&info, None)
    }

    /// As imagens do jogo, a cena e o passe que leva tudo à cadeia de verdade.
    unsafe fn build(&mut self, g: &vk::SwapchainCreateInfoKHR, list: Option<&vk::ImageFormatListCreateInfo>) -> ash::prelude::VkResult<()> {
        let d = &self.dev.fns;
        let device = d.handle();
        let mut n = 0;
        (self.dev.swap.get_swapchain_images_khr)(device, self.real, &mut n, std::ptr::null_mut()).result()?;
        let mut reals = vec![vk::Image::null(); n as usize];
        (self.dev.swap.get_swapchain_images_khr)(device, self.real, &mut n, reals.as_mut_ptr()).result()?;
        reals.truncate(n as usize);

        let extent = vk::Extent3D { width: g.image_extent.width, height: g.image_extent.height, depth: 1 };
        let mut formats = list.map(|l| vk::ImageFormatListCreateInfo { p_next: std::ptr::null(), ..*l });
        let mutable = g.flags.contains(vk::SwapchainCreateFlagsKHR::MUTABLE_FORMAT);
        let mut info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(g.image_format)
            .extent(extent)
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .usage(g.image_usage | vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(g.image_sharing_mode)
            .queue_family_indices(slice(g.p_queue_family_indices, g.queue_family_index_count));
        if mutable {
            info = info.flags(vk::ImageCreateFlags::MUTABLE_FORMAT);
            if let Some(f) = formats.as_mut() {
                info = info.push_next(f);
            }
        }
        for real in &reals {
            let image = d.create_image(&info, None)?;
            self.r.images.push(image);
            self.r.mems.push(self.memory(image)?);
            self.r.views.push(self.view(image, g.image_format)?);
            self.r.targets.push(self.view(*real, FP16)?);
        }
        let scene = vk::ImageCreateInfo { p_next: std::ptr::null(), flags: vk::ImageCreateFlags::empty(), usage: vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED, sharing_mode: vk::SharingMode::EXCLUSIVE, queue_family_index_count: 0, p_queue_family_indices: std::ptr::null(), ..info };
        self.r.scene = d.create_image(&scene, None)?;
        self.r.scene_mem = self.memory(self.r.scene)?;
        self.r.scene_view = self.view(self.r.scene, g.image_format)?;

        let att = [vk::AttachmentDescription::default()
            .format(FP16)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::DONT_CARE)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];
        let refs = [vk::AttachmentReference { attachment: 0, layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL }];
        let sub = [vk::SubpassDescription::default().pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS).color_attachments(&refs)];
        let dep = [vk::SubpassDependency::default()
            .src_subpass(vk::SUBPASS_EXTERNAL)
            .dst_subpass(0)
            .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)];
        self.r.pass = d.create_render_pass(&vk::RenderPassCreateInfo::default().attachments(&att).subpasses(&sub).dependencies(&dep), None)?;
        for t in &self.r.targets {
            let views = [*t];
            let fb = vk::FramebufferCreateInfo::default().render_pass(self.r.pass).attachments(&views).width(extent.width).height(extent.height).layers(1);
            self.r.fbs.push(d.create_framebuffer(&fb, None)?);
        }

        let bind = |b: u32| vk::DescriptorSetLayoutBinding::default().binding(b).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(1).stage_flags(vk::ShaderStageFlags::FRAGMENT);
        let binds = [bind(0), bind(1)];
        self.r.set_layout = d.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&binds), None)?;
        let push = [vk::PushConstantRange { stage_flags: vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, offset: 0, size: std::mem::size_of::<Params>() as u32 }];
        let layouts = [self.r.set_layout];
        self.r.layout = d.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts).push_constant_ranges(&push), None)?;
        self.r.pipeline = self.pipeline(g.image_extent)?;

        let sizes = [vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLED_IMAGE, descriptor_count: 2 * n }];
        self.r.pool = d.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(n).pool_sizes(&sizes), None)?;
        let layouts = vec![self.r.set_layout; n as usize];
        self.r.sets = d.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(self.r.pool).set_layouts(&layouts))?;
        for (set, view) in self.r.sets.iter().zip(&self.r.views) {
            let game = [vk::DescriptorImageInfo { sampler: vk::Sampler::null(), image_view: *view, image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL }];
            let scene = [vk::DescriptorImageInfo { image_view: self.r.scene_view, ..game[0] }];
            let write = vk::WriteDescriptorSet::default().dst_set(*set).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE);
            d.update_descriptor_sets(&[write.dst_binding(0).image_info(&game), write.dst_binding(1).image_info(&scene)], &[]);
        }
        for _ in 0..n {
            self.r.fences.push(d.create_fence(&vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED), None)?);
            self.r.sems.push(d.create_semaphore(&vk::SemaphoreCreateInfo::default(), None)?);
        }
        self.r.queries = d.create_query_pool(&vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(2 * n), None)?;
        self.ring = self.scan_build(n).ok().map(|_| scan::Ring::new(n as usize));

        if reals.len() <= MAX {
            IMAGES.iter().enumerate().for_each(|(i, a)| a.store(self.r.images.get(i).map_or(0, |x| x.as_raw()), Relaxed));
            PHASES.iter().for_each(|p| p.store(hdr::PHASE_NONE, Relaxed));
            SCENE.store(self.r.scene.as_raw(), Relaxed);
            EXTENT.store((extent.width as u64) << 32 | extent.height as u64, Relaxed);
        }
        Ok(())
    }

    unsafe fn pipeline(&self, extent: vk::Extent2D) -> ash::prelude::VkResult<vk::Pipeline> {
        let d = &self.dev.fns;
        let (vs, ps) = (words(VS), words(PS));
        let vs = d.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&vs), None)?;
        let ps = match d.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&ps), None) {
            Ok(m) => m,
            Err(e) => {
                d.destroy_shader_module(vs, None);
                return Err(e);
            }
        };
        let stages = [
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(vs).name(c"vs"),
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(ps).name(c"ps"),
        ];
        let vi = vk::PipelineVertexInputStateCreateInfo::default();
        let ia = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let vp = [vk::Viewport { x: 0.0, y: 0.0, width: extent.width as f32, height: extent.height as f32, min_depth: 0.0, max_depth: 1.0 }];
        let sc = [vk::Rect2D { offset: vk::Offset2D::default(), extent }];
        let vps = vk::PipelineViewportStateCreateInfo::default().viewports(&vp).scissors(&sc);
        let rs = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).cull_mode(vk::CullModeFlags::NONE).line_width(1.0);
        let ms = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend = [vk::PipelineColorBlendAttachmentState::default().color_write_mask(vk::ColorComponentFlags::RGBA)];
        let cb = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vi)
            .input_assembly_state(&ia)
            .viewport_state(&vps)
            .rasterization_state(&rs)
            .multisample_state(&ms)
            .color_blend_state(&cb)
            .layout(self.r.layout)
            .render_pass(self.r.pass);
        let r = d.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None);
        d.destroy_shader_module(vs, None);
        d.destroy_shader_module(ps, None);
        r.map(|p| p[0]).map_err(|(_, e)| e)
    }

    /// Os comandos nascem na família da fila que apresenta, na primeira vez.
    /// shortcut: jogo que apresenta por fila sem gráficos (computação assíncrona) fica com a cadeia sem o passe e mostra
    /// imagem errada; desenhar noutra fila pede sincronizar com as filas do jogo. Tratar se aparecer jogo assim.
    unsafe fn commands(&mut self, queue: vk::Queue) -> ash::prelude::VkResult<()> {
        let dev = self.dev;
        let family = dev.queues.iter().find(|q| q.0 == queue).map(|q| q.1).ok_or(vk::Result::ERROR_FEATURE_NOT_PRESENT)?;
        let props = dev.families.get(family as usize).ok_or(vk::Result::ERROR_FEATURE_NOT_PRESENT)?;
        if !props.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
            return Err(vk::Result::ERROR_FEATURE_NOT_PRESENT);
        }
        let bits = props.timestamp_valid_bits;
        self.mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
        let d = &dev.fns;
        let info = vk::CommandPoolCreateInfo::default().queue_family_index(family).flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        self.r.cmd_pool = d.create_command_pool(&info, None)?;
        let alloc = vk::CommandBufferAllocateInfo::default().command_pool(self.r.cmd_pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(self.r.images.len() as u32);
        let cmds = d.allocate_command_buffers(&alloc)?;
        // Objeto despachável criado por camada: o carregador precisa marcar.
        if let Some(set) = dev.loader_data {
            for c in &cmds {
                set(d.handle(), c.as_raw() as usize as *mut c_void).result()?;
            }
        }
        self.r.cmds = cmds;
        Ok(())
    }

    /// O passe do quadro `i`, esperando o jogo e avisando a apresentação por `sems[i]`.
    unsafe fn draw(&mut self, queue: vk::Queue, i: usize, waits: &[vk::Semaphore], pr: Params, s: &Shared) -> ash::prelude::VkResult<f64> {
        if self.r.cmds.is_empty() {
            self.commands(queue)?;
        }
        let d = &self.dev.fns;
        let r = &self.r;
        let (fence, cmd, image) = (r.fences[i], r.cmds[i], r.images[i]);
        d.wait_for_fences(&[fence], true, 1_000_000_000)?;
        let first = 2 * i as u32;
        let mut t = [0u64; 2];
        if self.mask != 0 && d.get_query_pool_results(r.queries, first, &mut t, vk::QueryResultFlags::TYPE_64).is_ok() {
            let ms = (t[1].wrapping_sub(t[0]) & self.mask) as f64 * self.dev.period as f64 / 1e6;
            self.ms = if self.ms <= 0.0 { ms } else { self.ms * 0.9 + ms * 0.1 };
        }
        let mut ring = self.ring.as_mut().and_then(|g| g.start(s).then_some(g));
        if let Some(g) = ring.as_mut().filter(|g| g.ready(i)) {
            let cur = std::slice::from_raw_parts((r.read_at as *const u8).add(i * scan::BYTES as usize) as *const u32, scan::WORDS).to_vec();
            g.feed(s, self.mode, &cur);
        }
        d.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
        d.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))?;
        if self.mask != 0 {
            d.cmd_reset_query_pool(cmd, r.queries, first, 2);
            d.cmd_write_timestamp(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, r.queries, first);
        }
        let read = vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL;
        let mut pre = vec![barrier(image, vk::ImageLayout::PRESENT_SRC_KHR, read, vk::AccessFlags::MEMORY_WRITE, vk::AccessFlags::SHADER_READ)];
        if !self.scene_ready {
            pre.push(barrier(r.scene, vk::ImageLayout::UNDEFINED, read, vk::AccessFlags::empty(), vk::AccessFlags::SHADER_READ));
        }
        let shaders = vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::COMPUTE_SHADER;
        d.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::ALL_COMMANDS, shaders, vk::DependencyFlags::empty(), &[], &[], &pre);
        let bytes = std::slice::from_raw_parts(&pr as *const Params as *const u8, std::mem::size_of::<Params>());
        if let Some(g) = ring {
            let mem = |src, dst| [vk::MemoryBarrier::default().src_access_mask(src).dst_access_mask(dst)];
            let (compute, transfer) = (vk::PipelineStageFlags::COMPUTE_SHADER, vk::PipelineStageFlags::TRANSFER);
            // A soma e a cópia do quadro anterior acabam antes desta soma.
            d.cmd_pipeline_barrier(cmd, compute | transfer, compute, vk::DependencyFlags::empty(), &mem(vk::AccessFlags::SHADER_WRITE | vk::AccessFlags::TRANSFER_READ, vk::AccessFlags::SHADER_READ | vk::AccessFlags::SHADER_WRITE), &[], &[]);
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, r.cs_pipeline);
            d.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::COMPUTE, r.cs_layout, 0, &[r.cs_sets[i]], &[]);
            d.cmd_push_constants(cmd, r.cs_layout, vk::ShaderStageFlags::COMPUTE, 0, bytes);
            d.cmd_dispatch(cmd, self.extent.width.div_ceil(scan::TILE), self.extent.height.div_ceil(scan::TILE), 1);
            d.cmd_pipeline_barrier(cmd, compute, transfer, vk::DependencyFlags::empty(), &mem(vk::AccessFlags::SHADER_WRITE, vk::AccessFlags::TRANSFER_READ), &[], &[]);
            let region = [vk::BufferCopy { src_offset: 0, dst_offset: i as u64 * scan::BYTES, size: scan::BYTES }];
            d.cmd_copy_buffer(cmd, r.hist, r.read, &region);
            d.cmd_pipeline_barrier(cmd, transfer, vk::PipelineStageFlags::HOST, vk::DependencyFlags::empty(), &mem(vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::HOST_READ), &[], &[]);
            g.wrote(i);
        }
        let area = vk::Rect2D { offset: vk::Offset2D::default(), extent: self.extent };
        d.cmd_begin_render_pass(cmd, &vk::RenderPassBeginInfo::default().render_pass(r.pass).framebuffer(r.fbs[i]).render_area(area), vk::SubpassContents::INLINE);
        d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, r.pipeline);
        d.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::GRAPHICS, r.layout, 0, &[r.sets[i]], &[]);
        d.cmd_push_constants(cmd, r.layout, vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT, 0, bytes);
        d.cmd_draw(cmd, 3, 1, 0, 0);
        d.cmd_end_render_pass(cmd);
        let post = [barrier(image, read, vk::ImageLayout::PRESENT_SRC_KHR, vk::AccessFlags::SHADER_READ, vk::AccessFlags::empty())];
        d.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::FRAGMENT_SHADER, vk::PipelineStageFlags::BOTTOM_OF_PIPE, vk::DependencyFlags::empty(), &[], &[], &post);
        if self.mask != 0 {
            d.cmd_write_timestamp(cmd, vk::PipelineStageFlags::BOTTOM_OF_PIPE, r.queries, first + 1);
        }
        d.end_command_buffer(cmd)?;
        let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; waits.len()];
        let (cmds, signal) = ([cmd], [r.sems[i]]);
        let submit = vk::SubmitInfo::default().wait_semaphores(waits).wait_dst_stage_mask(&stages).command_buffers(&cmds).signal_semaphores(&signal);
        d.reset_fences(&[fence])?;
        d.queue_submit(queue, &[submit], fence)?;
        self.scene_ready = true;
        Ok(self.ms)
    }
}

/// Só cadeias que dá para levar ao HDR: a tela está em HDR e aceita FP16 em sRGB estendido.
unsafe fn takes(dev: &Dev, g: &vk::SwapchainCreateInfoKHR) -> bool {
    // Só os espaços que a curva conhece: sRGB, HDR10 e scRGB. P3, BT.2020 linear e afins passam como estão.
    let known = matches!(g.image_color_space, vk::ColorSpaceKHR::SRGB_NONLINEAR | vk::ColorSpaceKHR::HDR10_ST2084_EXT | SCRGB);
    if INSIDE.get() || !known || !dev.inst.space || g.image_array_layers != 1 || !(g.flags & !vk::SwapchainCreateFlagsKHR::MUTABLE_FORMAT).is_empty() {
        return false;
    }
    let Some(hwnd) = lock(&SURFACES).as_ref().and_then(|m| m.get(&g.surface.as_raw()).copied()) else { return false };
    if !hdr::screen_hdr(HWND(hwnd as *mut c_void)) {
        return false;
    }
    let f = &dev.inst.surface;
    let mut n = 0;
    if (f.get_physical_device_surface_formats_khr)(dev.phys, g.surface, &mut n, std::ptr::null_mut()) != vk::Result::SUCCESS {
        return false;
    }
    let mut formats = vec![vk::SurfaceFormatKHR::default(); n as usize];
    let r = (f.get_physical_device_surface_formats_khr)(dev.phys, g.surface, &mut n, formats.as_mut_ptr());
    matches!(r, vk::Result::SUCCESS | vk::Result::INCOMPLETE) && formats[..n as usize].iter().any(|s| s.format == FP16 && s.color_space == SCRGB)
}

/// Tira da corrente `p_next` a lista de formatos (vale só com MUTABLE, e não tem FP16). Devolve onde pôr de volta.
unsafe fn cut(head: *mut *const c_void, ty: vk::StructureType) -> Option<(*mut *const c_void, *const c_void)> {
    let mut link = head;
    while let Some(s) = (*link as *mut vk::BaseOutStructure).as_mut() {
        if s.s_type == ty {
            let was = *link;
            *link = s.p_next as *const c_void;
            return Some((link, was));
        }
        link = &mut s.p_next as *mut *mut vk::BaseOutStructure as *mut *const c_void;
    }
    None
}

unsafe fn find<T>(mut p: *const c_void, ty: vk::StructureType) -> Option<&'static T> {
    while let Some(s) = (p as *const vk::BaseInStructure).as_ref() {
        if s.s_type == ty {
            return Some(&*(p as *const T));
        }
        p = s.p_next as *const c_void;
    }
    None
}

fn note(e: vk::Result) {
    hdr::note(HRESULT(e.as_raw()));
}

unsafe extern "system" fn create_swapchain(device: vk::Device, info: *const vk::SwapchainCreateInfoKHR, alloc: *const vk::AllocationCallbacks, out: *mut vk::SwapchainKHR) -> vk::Result {
    let Some(dev) = dev_of(device.as_raw()) else { return vk::Result::ERROR_INITIALIZATION_FAILED };
    let next = dev.swap.create_swapchain_khr;
    // O próprio driver pode apresentar pelo DXGI: os ganchos de lá deixam passar.
    let call = |i: &vk::SwapchainCreateInfoKHR| hdr::passthrough(|| next(device, i, alloc, out));
    let g = &*info;
    if !takes(dev, g) {
        return call(g);
    }
    let mut real = *g;
    real.image_format = FP16;
    real.image_color_space = SCRGB;
    real.image_usage = vk::ImageUsageFlags::COLOR_ATTACHMENT;
    real.flags = vk::SwapchainCreateFlagsKHR::empty();
    let back = cut(&mut real.p_next, vk::StructureType::IMAGE_FORMAT_LIST_CREATE_INFO);
    let r = call(&real);
    if let Some((link, was)) = back {
        *link = was;
    }
    // A cadeia antiga já se aposentou na primeira tentativa.
    let plain = vk::SwapchainCreateInfoKHR { old_swapchain: vk::SwapchainKHR::null(), ..*g };
    if r != vk::Result::SUCCESS {
        note(r);
        return call(&plain);
    }
    let mode = match (g.image_format, g.image_color_space) {
        (_, vk::ColorSpaceKHR::HDR10_ST2084_EXT) => hdr::MODE_HDR10,
        (FP16, SCRGB) => hdr::MODE_SCRGB,
        _ => hdr::MODE_SDR,
    };
    let srgb = matches!(g.image_format, vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB | vk::Format::A8B8G8R8_SRGB_PACK32);
    let mut c = Chain { real: *out, dev, extent: g.image_extent, srgb, mode, r: Res::default(), scene_ready: false, mask: 0, ms: 0.0, ring: None };
    match c.build(g, find(g.p_next, vk::StructureType::IMAGE_FORMAT_LIST_CREATE_INFO)) {
        Ok(()) => {
            lock(&CHAINS).push(c);
            if let Some(s) = hdr::shared() {
                s.hdr_chains.fetch_add(1, Relaxed);
            }
            vk::Result::SUCCESS
        }
        Err(e) => {
            note(e);
            drop(c);
            (dev.swap.destroy_swapchain_khr)(device, *out, alloc);
            call(&plain)
        }
    }
}

unsafe extern "system" fn destroy_swapchain(device: vk::Device, chain: vk::SwapchainKHR, alloc: *const vk::AllocationCallbacks) {
    let Some(dev) = dev_of(device.as_raw()) else { return };
    let gone = {
        let mut list = lock(&CHAINS);
        list.iter().position(|c| c.real == chain).map(|i| list.swap_remove(i))
    };
    drop(gone);
    hdr::passthrough(|| (dev.swap.destroy_swapchain_khr)(device, chain, alloc));
}

/// O jogo recebe as imagens nossas, no formato que pediu.
unsafe extern "system" fn get_images(device: vk::Device, chain: vk::SwapchainKHR, count: *mut u32, images: *mut vk::Image) -> vk::Result {
    let Some(dev) = dev_of(device.as_raw()) else { return vk::Result::ERROR_INITIALIZATION_FAILED };
    if let Some(c) = lock(&CHAINS).iter().find(|c| c.real == chain) {
        let all = &c.r.images;
        if images.is_null() {
            *count = all.len() as u32;
            return vk::Result::SUCCESS;
        }
        let k = (*count as usize).min(all.len());
        std::ptr::copy_nonoverlapping(all.as_ptr(), images, k);
        *count = k as u32;
        return if k < all.len() { vk::Result::INCOMPLETE } else { vk::Result::SUCCESS };
    }
    (dev.swap.get_swapchain_images_khr)(device, chain, count, images)
}

unsafe extern "system" fn present(queue: vk::Queue, info: *const vk::PresentInfoKHR) -> vk::Result {
    let Some(dev) = dev_of(queue.as_raw()) else { return vk::Result::ERROR_INITIALIZATION_FAILED };
    let next = dev.swap.queue_present_khr;
    let i = &*info;
    let mut waits = slice(i.p_wait_semaphores, i.wait_semaphore_count).to_vec();
    if let Some(s) = hdr::shared().filter(|_| !INSIDE.get()) {
        let chains = slice(i.p_swapchains, i.swapchain_count);
        let indices = slice(i.p_image_indices, i.swapchain_count);
        let mut list = lock(&CHAINS);
        for (sc, idx) in chains.iter().zip(indices) {
            let Some(c) = list.iter_mut().find(|c| c.real == *sc) else { continue };
            let idx = *idx as usize;
            if idx >= c.r.images.len() {
                continue;
            }
            let phase = match PHASES.get(idx) {
                Some(p) if c.splits() => p.swap(hdr::PHASE_NONE, Relaxed),
                _ => hdr::PHASE_NONE,
            };
            let split = hdr::split_of(s, phase);
            let pr = hdr::params(s, c.srgb, c.mode, split == hdr::SPLIT_ON);
            match c.draw(queue, idx, &waits, pr, s) {
                Ok(ms) => {
                    waits = vec![c.r.sems[idx]];
                    hdr::report(s, ms, split, c.mode);
                }
                Err(e) => note(e),
            }
        }
    }
    let mut copy = *i;
    copy.wait_semaphore_count = waits.len() as u32;
    copy.p_wait_semaphores = waits.as_ptr();
    hdr::passthrough(|| next(queue, &copy))
}

// --- Separar a interface: o passe de tela cheia no buffer do jogo guarda a cena. ---

#[derive(Default)]
struct Track {
    /// Vista → índice da imagem do jogo.
    views: HashMap<u64, usize>,
    /// Framebuffer → (índice, anexo).
    fbs: HashMap<u64, (usize, usize)>,
    /// Passe → layout final de cada anexo.
    passes: HashMap<u64, Vec<vk::ImageLayout>>,
}

static TRACK: Mutex<Option<Track>> = Mutex::new(None);

thread_local! {
    /// O passe aberto num buffer do jogo nesta linha: (lista, índice + 1, layout no fim, área toda, cena a guardar no fim).
    static PASS: Cell<(u64, usize, vk::ImageLayout, bool, bool)> = const { Cell::new((0, 0, vk::ImageLayout::UNDEFINED, false, false)) };
    /// A última área de desenho desta linha, e se cobre a tela.
    static VP: Cell<(u64, bool)> = const { Cell::new((0, false)) };
}

fn track<R>(f: impl FnOnce(&mut Track) -> R) -> R {
    f(lock(&TRACK).get_or_insert_with(Track::default))
}

fn index_of(image: vk::Image) -> Option<usize> {
    IMAGES.iter().position(|i| i.load(Relaxed) == image.as_raw() && image.as_raw() != 0)
}

fn size() -> (u32, u32) {
    let e = EXTENT.load(Relaxed);
    ((e >> 32) as u32, e as u32)
}

unsafe extern "system" fn create_view(device: vk::Device, info: *const vk::ImageViewCreateInfo, alloc: *const vk::AllocationCallbacks, out: *mut vk::ImageView) -> vk::Result {
    let Some(dev) = dev_of(device.as_raw()) else { return vk::Result::ERROR_INITIALIZATION_FAILED };
    let r = (dev.fns.fp_v1_0().create_image_view)(device, info, alloc, out);
    if r == vk::Result::SUCCESS {
        // Um número de vista reaproveitado passa por aqui de novo: o que era do jogo sai.
        let v = (*out).as_raw();
        track(|t| match index_of((*info).image) {
            Some(i) => t.views.insert(v, i),
            None => t.views.remove(&v),
        });
    }
    r
}

unsafe extern "system" fn create_framebuffer(device: vk::Device, info: *const vk::FramebufferCreateInfo, alloc: *const vk::AllocationCallbacks, out: *mut vk::Framebuffer) -> vk::Result {
    let Some(dev) = dev_of(device.as_raw()) else { return vk::Result::ERROR_INITIALIZATION_FAILED };
    let r = (dev.fns.fp_v1_0().create_framebuffer)(device, info, alloc, out);
    if r == vk::Result::SUCCESS {
        let i = &*info;
        // shortcut: framebuffer sem imagens (IMAGELESS) não separa; ver se algum jogo usa.
        let atts = if i.flags.contains(vk::FramebufferCreateFlags::IMAGELESS) { &[][..] } else { slice(i.p_attachments, i.attachment_count) };
        let f = (*out).as_raw();
        track(|t| match atts.iter().enumerate().find_map(|(k, v)| t.views.get(&v.as_raw()).map(|i| (*i, k))) {
            Some(at) => t.fbs.insert(f, at),
            None => t.fbs.remove(&f),
        });
    }
    r
}

unsafe extern "system" fn create_pass(device: vk::Device, info: *const vk::RenderPassCreateInfo, alloc: *const vk::AllocationCallbacks, out: *mut vk::RenderPass) -> vk::Result {
    let Some(dev) = dev_of(device.as_raw()) else { return vk::Result::ERROR_INITIALIZATION_FAILED };
    let r = (dev.fns.fp_v1_0().create_render_pass)(device, info, alloc, out);
    if r == vk::Result::SUCCESS {
        let layouts = slice((*info).p_attachments, (*info).attachment_count).iter().map(|a| a.final_layout).collect();
        track(|t| t.passes.insert((*out).as_raw(), layouts));
    }
    r
}

unsafe extern "system" fn create_pass2(device: vk::Device, info: *const vk::RenderPassCreateInfo2, alloc: *const vk::AllocationCallbacks, out: *mut vk::RenderPass) -> vk::Result {
    let Some(dev) = dev_of(device.as_raw()) else { return vk::Result::ERROR_INITIALIZATION_FAILED };
    let r = (dev.fns.fp_v1_2().create_render_pass2)(device, info, alloc, out);
    if r == vk::Result::SUCCESS {
        let layouts = slice((*info).p_attachments, (*info).attachment_count).iter().map(|a| a.final_layout).collect();
        track(|t| t.passes.insert((*out).as_raw(), layouts));
    }
    r
}

fn covers(area: vk::Rect2D) -> bool {
    let (w, h) = size();
    area.offset.x <= 0 && area.offset.y <= 0 && area.extent.width >= w && area.extent.height >= h
}

unsafe fn begun(cmd: vk::CommandBuffer, info: *const vk::RenderPassBeginInfo) {
    PASS.set((0, 0, vk::ImageLayout::UNDEFINED, false, false));
    if INSIDE.get() || IMAGES[0].load(Relaxed) == 0 {
        return;
    }
    let i = &*info;
    let at = track(|t| {
        let (idx, att) = *t.fbs.get(&i.framebuffer.as_raw())?;
        Some((idx, *t.passes.get(&i.render_pass.as_raw())?.get(att)?))
    });
    if let Some((idx, layout)) = at {
        PASS.set((cmd.as_raw(), idx + 1, layout, covers(i.render_area), false));
    }
}

unsafe extern "system" fn begin_pass(cmd: vk::CommandBuffer, info: *const vk::RenderPassBeginInfo, contents: vk::SubpassContents) {
    let Some(dev) = dev_of(cmd.as_raw()) else { return };
    (dev.fns.fp_v1_0().cmd_begin_render_pass)(cmd, info, contents);
    begun(cmd, info);
}

unsafe extern "system" fn begin_pass2(cmd: vk::CommandBuffer, info: *const vk::RenderPassBeginInfo, sub: *const vk::SubpassBeginInfo) {
    let Some(dev) = dev_of(cmd.as_raw()) else { return };
    (dev.fns.fp_v1_2().cmd_begin_render_pass2)(cmd, info, sub);
    begun(cmd, info);
}

unsafe extern "system" fn begin_rendering(cmd: vk::CommandBuffer, info: *const vk::RenderingInfo) {
    let Some(dev) = dev_of(cmd.as_raw()) else { return };
    (dev.fns.fp_v1_3().cmd_begin_rendering)(cmd, info);
    PASS.set((0, 0, vk::ImageLayout::UNDEFINED, false, false));
    if INSIDE.get() || IMAGES[0].load(Relaxed) == 0 {
        return;
    }
    let i = &*info;
    let Some(a) = slice(i.p_color_attachments, i.color_attachment_count).first() else { return };
    if let Some(idx) = track(|t| t.views.get(&a.image_view.as_raw()).copied()) {
        PASS.set((cmd.as_raw(), idx + 1, a.image_layout, covers(i.render_area), false));
    }
}

/// Fim do passe: a cena guardada agora, fora dele, na própria lista do jogo.
/// shortcut: interface no mesmo passe do de tela cheia não separa (vira "tarde"); listas secundárias, desenho
/// indireto e área de desenho fixa no pipeline também não. Cobrir se um jogo testado precisar.
unsafe fn ended(dev: &Dev, cmd: vk::CommandBuffer) {
    let (c, idx, layout, _, pending) = PASS.replace((0, 0, vk::ImageLayout::UNDEFINED, false, false));
    let (image, scene) = (IMAGES.get(idx.wrapping_sub(1)).map_or(0, |i| i.load(Relaxed)), SCENE.load(Relaxed));
    if c != cmd.as_raw() || !pending || image == 0 || scene == 0 {
        return;
    }
    let (image, scene) = (vk::Image::from_raw(image), vk::Image::from_raw(scene));
    let (w, h) = size();
    let d = &dev.fns;
    let src = vk::ImageLayout::TRANSFER_SRC_OPTIMAL;
    let dst = vk::ImageLayout::TRANSFER_DST_OPTIMAL;
    let pre = [
        barrier(image, layout, src, vk::AccessFlags::COLOR_ATTACHMENT_WRITE, vk::AccessFlags::TRANSFER_READ),
        barrier(scene, vk::ImageLayout::UNDEFINED, dst, vk::AccessFlags::empty(), vk::AccessFlags::TRANSFER_WRITE),
    ];
    let stages = vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags::FRAGMENT_SHADER;
    d.cmd_pipeline_barrier(cmd, stages, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &pre);
    let layer = vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 };
    let region = vk::ImageCopy { src_subresource: layer, dst_subresource: layer, extent: vk::Extent3D { width: w, height: h, depth: 1 }, ..Default::default() };
    d.cmd_copy_image(cmd, image, src, scene, dst, &[region]);
    let all = vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE;
    let post = [
        barrier(image, src, layout, vk::AccessFlags::TRANSFER_READ, all),
        barrier(scene, dst, vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, vk::AccessFlags::TRANSFER_WRITE, vk::AccessFlags::SHADER_READ),
    ];
    d.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::ALL_COMMANDS, vk::DependencyFlags::empty(), &[], &[], &post);
    PHASES[idx - 1].store(hdr::PHASE_SCENE, Relaxed);
}

unsafe extern "system" fn end_pass(cmd: vk::CommandBuffer) {
    let Some(dev) = dev_of(cmd.as_raw()) else { return };
    (dev.fns.fp_v1_0().cmd_end_render_pass)(cmd);
    ended(dev, cmd);
}

unsafe extern "system" fn end_pass2(cmd: vk::CommandBuffer, info: *const vk::SubpassEndInfo) {
    let Some(dev) = dev_of(cmd.as_raw()) else { return };
    (dev.fns.fp_v1_2().cmd_end_render_pass2)(cmd, info);
    ended(dev, cmd);
}

unsafe extern "system" fn end_rendering(cmd: vk::CommandBuffer) {
    let Some(dev) = dev_of(cmd.as_raw()) else { return };
    (dev.fns.fp_v1_3().cmd_end_rendering)(cmd);
    ended(dev, cmd);
}

unsafe extern "system" fn set_viewport(cmd: vk::CommandBuffer, first: u32, count: u32, vps: *const vk::Viewport) {
    let Some(dev) = dev_of(cmd.as_raw()) else { return };
    (dev.fns.fp_v1_0().cmd_set_viewport)(cmd, first, count, vps);
    if let Some(v) = slice(vps, count).first().filter(|_| first == 0) {
        let (w, h) = (size().0 as f32, size().1 as f32);
        // Altura negativa vira a imagem: o topo é `y + height`.
        let top = v.y.min(v.y + v.height);
        VP.set((cmd.as_raw(), h > 0.0 && v.x <= 0.5 && top <= 0.5 && v.width >= w - 1.0 && v.height.abs() >= h - 1.0));
    }
}

/// Depois de cada desenho num buffer do jogo. A cópia só cabe fora do passe: fica marcada para o fim dele, e um
/// desenho que não é de tela cheia antes disso desfaz a separação no quadro.
fn drawn(cmd: vk::CommandBuffer, count: u32, instances: u32) {
    let (c, idx, layout, area, pending) = PASS.get();
    if c != cmd.as_raw() || idx == 0 || INSIDE.get() {
        return;
    }
    let i = idx - 1;
    let full = count <= 6 && instances <= 1 && area && VP.get() == (c, true);
    if pending {
        if !full {
            PHASES[i].store(hdr::PHASE_LATE, Relaxed);
            PASS.set((c, idx, layout, area, false));
        }
        return;
    }
    let (next, snap) = hdr::step(PHASES[i].load(Relaxed), full);
    if snap {
        PASS.set((c, idx, layout, area, true));
    } else {
        PHASES[i].store(next, Relaxed);
    }
}

unsafe extern "system" fn draw(cmd: vk::CommandBuffer, count: u32, instances: u32, first: u32, first_instance: u32) {
    let Some(dev) = dev_of(cmd.as_raw()) else { return };
    (dev.fns.fp_v1_0().cmd_draw)(cmd, count, instances, first, first_instance);
    drawn(cmd, count, instances);
}

unsafe extern "system" fn draw_indexed(cmd: vk::CommandBuffer, count: u32, instances: u32, first: u32, offset: i32, first_instance: u32) {
    let Some(dev) = dev_of(cmd.as_raw()) else { return };
    (dev.fns.fp_v1_0().cmd_draw_indexed)(cmd, count, instances, first, offset, first_instance);
    drawn(cmd, count, instances);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPIR-V do `SHADER` (HLSL) pelo glslang. O crate glslang não deixa escolher o ponto de entrada; o C direto deixa.
    fn spirv(stage: glslang_sys::glslang_stage_t, entry: &str) -> Vec<u8> {
        use glslang_sys::*;
        use std::ffi::CString;
        let _process = glslang::Compiler::acquire().unwrap();
        // O histograma do Analisar só existe no `cs`, como no fxc.
        let code = CString::new(if entry == "cs" { format!("#define SCAN 1\n{}", hdr::SHADER) } else { hdr::SHADER.to_string() }).unwrap();
        let messages = glslang_messages_t::READ_HLSL | glslang_messages_t::SPV_RULES | glslang_messages_t::VULKAN_RULES;
        // ResourceLimits é repr(transparent) sobre glslang_resource_t.
        let resource = &glslang::limits::DEFAULT_LIMITS as *const _ as *const glslang_resource_t;
        let input = glslang_input_t {
            language: glslang_source_t::HLSL,
            stage,
            client: glslang_client_t::Vulkan,
            client_version: glslang_target_client_version_t::Vulkan1_0,
            target_language: glslang_target_language_t::SPIRV,
            target_language_version: glslang_target_language_version_t::SPIRV1_0,
            code: code.as_ptr(),
            default_version: 100,
            default_profile: glslang_profile_t::None,
            force_default_version_and_profile: 0,
            forward_compatible: 0,
            messages,
            resource,
            callbacks: glsl_include_callbacks_t { include_system: None, include_local: None, free_include_result: None },
            callbacks_ctx: std::ptr::null_mut(),
        };
        let name = CString::new(entry).unwrap();
        unsafe {
            let log = |s: *mut glslang_shader_t| CStr::from_ptr(glslang_shader_get_info_log(s)).to_string_lossy().into_owned();
            let shader = glslang_shader_create(&input);
            glslang_shader_set_entry_point(shader, name.as_ptr());
            assert!(glslang_shader_preprocess(shader, &input) != 0, "{entry}: {}", log(shader));
            assert!(glslang_shader_parse(shader, &input) != 0, "{entry}: {}", log(shader));
            let program = glslang_program_create();
            glslang_program_add_shader(program, shader);
            assert!(glslang_program_link(program, messages.0) != 0, "{entry}: link");
            glslang_program_SPIRV_generate(program, stage);
            let mut words = vec![0u32; glslang_program_SPIRV_get_size(program)];
            glslang_program_SPIRV_get(program, words.as_mut_ptr());
            glslang_program_delete(program);
            glslang_shader_delete(shader);
            words.iter().flat_map(|w| w.to_le_bytes()).collect()
        }
    }

    /// Mudou o HLSL: o teste regrava os `.spv` e falha uma vez, para o commit levar os dois juntos.
    #[test]
    fn spirv_matches_hlsl() {
        use glslang_sys::glslang_stage_t::{Compute, Fragment, Vertex};
        let mut stale = Vec::new();
        for (stage, entry, have) in [(Vertex, "vs", VS), (Fragment, "ps", PS), (Compute, "cs", CS)] {
            let fresh = spirv(stage, entry);
            if fresh != have {
                std::fs::write(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(format!("hdr_{entry}.spv")), &fresh).unwrap();
                stale.push(entry);
            }
            // O ponto de entrada com o nome que o pipeline usa, e as constantes por push (classe 9).
            let w = words(&fresh);
            let mut at = 5;
            let (mut named, mut push) = (false, false);
            while at < w.len() {
                let (n, op) = ((w[at] >> 16) as usize, w[at] & 0xffff);
                named |= op == 15 && CStr::from_bytes_until_nul(bytemuck(&w[at + 3..at + n])).is_ok_and(|s| s.to_bytes() == entry.as_bytes());
                push |= op == 59 && w[at + 3] == 9;
                at += n.max(1);
            }
            assert!(named && push, "{entry}");
        }
        assert!(stale.is_empty(), "regravados: {stale:?}");
    }

    fn bytemuck(w: &[u32]) -> &[u8] {
        unsafe { std::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 4) }
    }

    #[test]
    fn table_reuses_slots() {
        let t: Table<u32> = Table::new();
        t.put(10, 1);
        t.put(20, 2);
        assert_eq!(t.get(20), Some(&2));
        t.remove(10);
        assert_eq!(t.get(10), None);
        t.put(30, 3);
        assert_eq!((t.get(20), t.get(30)), (Some(&2), Some(&3)));
    }

    #[test]
    fn format_list_cut_and_restored() {
        let list = vk::ImageFormatListCreateInfo::default();
        let mut tail = vk::SwapchainPresentModesCreateInfoEXT::default();
        tail.p_next = &list as *const _ as *const c_void;
        let mut real = vk::SwapchainCreateInfoKHR::default();
        real.p_next = &tail as *const _ as *const c_void;
        let back = unsafe { cut(&mut real.p_next, vk::StructureType::IMAGE_FORMAT_LIST_CREATE_INFO) };
        assert!(tail.p_next.is_null());
        let (link, was) = back.unwrap();
        unsafe { *link = was };
        assert_eq!(tail.p_next, &list as *const _ as *const c_void);
    }
}
