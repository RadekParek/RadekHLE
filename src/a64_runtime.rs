use crate::a64_abi::A64Abi;
use crate::dyld::{search_host_dylibs, HostConstant};
use crate::mem64::Mem64;
use crate::window::DeviceFamily;
use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;
use std::collections::{HashMap, HashSet};

const MAX_CSTRING: u64 = 1024 * 1024;
const A64_OBJECT_SIZE: u64 = 96;
const A64_KIND_CLASS: u64 = 1;
const A64_KIND_DEVICE: u64 = 2;
const A64_KIND_QUEUE: u64 = 3;
const A64_KIND_COMMAND_BUFFER: u64 = 4;
const A64_KIND_RENDER_ENCODER: u64 = 5;
const A64_KIND_COMPUTE_ENCODER: u64 = 6;
const A64_KIND_BLIT_ENCODER: u64 = 7;
const A64_KIND_BUFFER: u64 = 8;
const A64_KIND_TEXTURE: u64 = 9;
const A64_KIND_TEXTURE_DESCRIPTOR: u64 = 10;
const A64_KIND_STRING: u64 = 11;
const A64_KIND_PIPELINE: u64 = 12;
const A64_KIND_GENERIC: u64 = 13;
const A64_KIND_BUNDLE: u64 = 14;
const A64_KIND_RENDER_PASS_DESCRIPTOR: u64 = 15;
const A64_KIND_ATTACHMENT_ARRAY: u64 = 16;
const A64_KIND_ATTACHMENT: u64 = 17;
const A64_KIND_SAMPLER: u64 = 18;
const A64_KIND_DEPTH_STENCIL: u64 = 19;
const A64_KIND_UI_DEVICE: u64 = 20;
const A64_KIND_UI_SCREEN: u64 = 21;
const A64_KIND_UNITY_FRAMEWORK: u64 = 22;
const A64_KIND_NUMBER: u64 = 23;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A64GraphicsBackend {
    MetalCompatibility,
    OpenGLESCompatibility,
    SoftwareCompatibility,
}

impl A64GraphicsBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::MetalCompatibility => "Metal compatibility",
            Self::OpenGLESCompatibility => "OpenGL ES compatibility",
            Self::SoftwareCompatibility => "software compatibility",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedImage {
    pub name: String,
    pub base: u64,
    pub end: u64,
    pub entry: Option<u64>,
    pub exports: HashMap<String, u64>,
}

#[derive(Debug)]
pub struct RuntimeState {
    pub ios_version: (i32, i32, i32),
    pub graphics_backend: A64GraphicsBackend,
    pub device_family: DeviceFamily,
    pub host_dispatches: u64,
    pub objc_messages: u64,
    pub metal_commands: u64,
    pub frame_serial: u64,
    pub present_requested: bool,
    pub boot_screen_reached: bool,
    pub clear_color: [f32; 4],
    pub last_selector: Option<String>,
    pub last_symbol: Option<String>,
    pub last_successful_symbol: Option<String>,
    pub current_module: Option<String>,
    pub bundle_identifier: String,
    pub bundle_path: String,
    pub bundle_name: String,
    pub unimplemented_symbols: HashSet<String>,
    pub loaded_images: Vec<LoadedImage>,
    pub guest_transfer_pc: Option<u64>,
    pub unity_framework_instance: Option<u64>,
}

impl RuntimeState {
    pub fn new(
        ios_version: (i32, i32, i32),
        graphics_backend: A64GraphicsBackend,
        device_family: DeviceFamily,
    ) -> Self {
        Self {
            ios_version,
            graphics_backend,
            device_family,
            host_dispatches: 0,
            objc_messages: 0,
            metal_commands: 0,
            frame_serial: 0,
            present_requested: false,
            boot_screen_reached: false,
            clear_color: [0.0, 0.0, 0.0, 1.0],
            last_selector: None,
            last_symbol: None,
            last_successful_symbol: None,
            current_module: None,
            bundle_identifier: "org.touchhle.app".to_owned(),
            bundle_path: "/".to_owned(),
            bundle_name: "Application".to_owned(),
            unimplemented_symbols: HashSet::new(),
            loaded_images: Vec::new(),
            guest_transfer_pc: None,
            unity_framework_instance: None,
        }
    }

    pub fn take_present_request(&mut self) -> bool {
        std::mem::take(&mut self.present_requested)
    }

    pub fn take_boot_screen_request(&mut self) -> bool {
        std::mem::take(&mut self.boot_screen_reached)
    }

    pub fn take_guest_transfer(&mut self) -> Option<u64> {
        self.guest_transfer_pc.take()
    }

    pub fn resolve_image_symbol(&self, candidates: &[&str]) -> Option<u64> {
        self.find_image_symbol(candidates).map(|(_, address)| address)
    }

    fn find_image_symbol(&self, candidates: &[&str]) -> Option<(String, u64)> {
        for image in &self.loaded_images {
            for candidate in candidates {
                if let Some(&address) = image.exports.get(*candidate) {
                    return Some((image.name.clone(), address));
                }
            }
            for (symbol, &address) in &image.exports {
                if candidates.iter().any(|candidate| symbol.ends_with(candidate)) {
                    return Some((image.name.clone(), address));
                }
            }
        }
        None
    }

    fn transfer_to_method(&mut self, selector: &str, candidates: &[&str]) -> Result<(), String> {
        let Some((image, address)) = self.find_image_symbol(candidates) else {
            return Err(format!("ARM64 could not resolve UnityFramework method for selector {selector}"));
        };
        log!("ARM64 guest transfer: selector {} -> {} at {:#x}", selector, image, address);
        self.guest_transfer_pc = Some(address);
        Ok(())
    }
}

fn name(symbol: &str) -> &str {
    symbol.trim_start_matches('_')
}

fn materialize_host_constant(mem: &mut Mem64, symbol: &str) -> Result<Option<u64>, String> {
    let normalized = name(symbol);
    if let Some((_, constant)) = search_host_dylibs(|dylib| dylib.constant_exports, symbol)
        .or_else(|| search_host_dylibs(|dylib| dylib.constant_exports, normalized))
    {
        return match constant {
            HostConstant::NSString(value) => Ok(Some(objc_string(mem, value)?)),
            HostConstant::NullPtr => Ok(Some(0)),
            HostConstant::Custom(_) => Ok(materialize_custom_constant(mem, normalized)),
        };
    }

    match normalized {
        "ZSt7nothrow"
        | "ZNSt3__15ctypeIcE2id"
        | "ZNSt3__15ctypeIcE2idE"
        | "ZNSt3__17codecvtIcc11__mbstate_tE2id"
        | "ZNSt3__17codecvtIcc11__mbstate_tE2idE" => {
            Ok(Some(mem.alloc_zeroed(16).map_err(str::to_owned)?))
        }
        _ => Ok(None),
    }
}

fn materialize_custom_constant(mem: &mut Mem64, symbol: &str) -> Option<u64> {
    match name(symbol) {
        "DefaultRuneLocale" => mem.alloc_zeroed(128).ok(),
        "stderrp" => {
            let file = mem.alloc_zeroed(16).ok()?;
            mem.write_u32(file, 2).ok()?;
            let slot = mem.alloc_zeroed(8).ok()?;
            mem.write_u64(slot, file).ok()?;
            Some(slot)
        }
        "kCFBooleanTrue" => objc_number(mem, 1).ok(),
        "kCFBooleanFalse" => objc_number(mem, 0).ok(),
        "kCFNull" => objc_object(mem, A64_KIND_GENERIC).ok(),
        "kCFAllocatorSystemDefault" => Some(0),
        "gxx_personality_v0" => Some(0),
        _ => None,
    }
}

pub fn can_dispatch(symbol: &str) -> bool {
    match name(symbol) {
        "malloc" | "calloc" | "valloc" | "posix_memalign" | "free"
        | "malloc_zone_free" | "realloc" | "malloc_zone_realloc" | "memcpy"
        | "memmove" | "memcpy_chk" | "memmove_chk" | "memset" | "bzero"
        | "memset_chk" | "strlen" | "strnlen" | "strcmp" | "strncmp" | "memcmp"
        | "strcpy" | "strncpy" | "strcat" | "strncat" | "strdup" | "strndup"
        | "objc_release" | "objc_storeStrong" | "objc_retain"
        | "objc_retainAutoreleasedReturnValue" | "objc_retainAutoreleaseReturnValue"
        | "objc_autorelease" | "objc_autoreleaseReturnValue"
        | "objc_unsafeClaimAutoreleasedReturnValue" | "objc_retainAutorelease"
        | "objc_retainBlock" | "objc_msgSend" | "objc_msgSendSuper2"
        | "objc_msgSend_stret" | "objc_msgSendSuper2_stret" | "objc_msgSend_fpret"
        | "objc_msgSend_fp2ret" | "objc_getClass" | "objc_getRequiredClass"
        | "objc_lookUpClass" | "object_getClass" | "object_getClassName"
        | "sel_registerName" | "sel_getUid" | "strcpy" | "strncpy" | "strcat"
        | "objc_autoreleasePoolPush" | "objc_autoreleasePoolPop"
        | "objc_exception_throw" | "objc_begin_catch" | "objc_end_catch"
        | "cxa_guard_acquire" | "cxa_guard_release" | "cxa_guard_abort"
        | "cxa_pure_virtual" | "stack_chk_fail" | "stack_chk_fail_local"
        | "memchr" | "strnlen" | "strchr" | "strrchr" | "strstr"
        | "strcasecmp" | "strncasecmp" | "bcopy" | "bcmp"
        | "memset_pattern4" | "memset_pattern8" | "memset_pattern16"
        | "cxa_atexit" | "atexit" | "pthread_mutex_lock"
        | "pthread_mutex_unlock" | "pthread_mutex_init" | "pthread_mutex_destroy"
        | "pthread_once" | "pthread_key_create" | "pthread_getspecific" | "pthread_setspecific"
        | "pthread_self" | "sched_yield" | "abort" | "exit" | "_exit"
        | "NSLog" | "NSLogv" | "os_log" | "os_logv" | "UIApplicationMain" | "dyld_stub_binder"
        | "CFConstantStringClassReference" | "__CFConstantStringClassReference"
        | "MTLCreateSystemDefaultDevice" | "vkEnumerateInstanceVersion"
        | "vkCreateInstance" | "vkDestroyInstance" | "vkDestroyDevice"
        | "vkEnumeratePhysicalDevices" | "vkGetPhysicalDeviceQueueFamilyProperties"
        | "vkCreateDevice" | "vkGetDeviceQueue" | "vkDeviceWaitIdle" => true,
        value if value.starts_with("gl") || value.starts_with("egl") || value.starts_with("EAGL") => true,
        "inflate" | "inflateEnd" | "inflateInit_" | "compressBound" | "deflate" | "deflateEnd" | "deflateInit_" | "deflateReset"
        | "class_addProperty" | "class_addProtocol" | "class_getInstanceVariable" | "class_isMetaClass"
        | "objc_constructInstance" | "objc_enumerationMutation" | "objc_initializeClassPair" | "object_getIvar"
        | "property_copyAttributeList" | "gxx_personality_v0"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEED1Ev"
        | "ZNKSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE4findEPKcmm"
        | "ZNKSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE4findEcm"
        | "ZNKSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE5rfindEPKcmm"
        | "ZNKSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE5rfindEcm"
        | "ZNKSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE7compareEPKc"
        | "ZNKSt3__120__vector_base_commonILb1EE20__throw_length_errorEv"
        | "ZNKSt3__120__vector_base_commonILb1EE20__throw_out_of_rangeEv"
        | "ZNKSt3__121__basic_string_commonILb1EE20__throw_length_errorEv"
        | "ZNKSt3__16locale9has_facetERNS0_2idE" | "ZNKSt3__16locale9use_facetERNS0_2idE"
        | "ZNKSt3__18ios_base6getlocEv" | "ZNKSt3__111this_thread9sleep_forERKNS_6chrono8durationIxNS_5ratioILl1ELl1000000000EEEEE"
        | "ZNSt3__112__next_primeEm" | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE5eraseEmm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6__initEPKcmm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6__initEmc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6appendEPKc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6appendEPKcm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6assignEPKc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6assignEPKcm" => true,
        value if value.starts_with("ZNSt3__") || value.starts_with("ZNKSt3__") => true,
        _ => false,
    }
}

fn return_value(context: &mut touchHLE_DynarmicA64Context, value: u64) {
    A64Abi::set_return(context, value);
}

fn c_string(mem: &Mem64, address: u64) -> Option<Vec<u8>> {
    let length = mem.cstr_len(address, MAX_CSTRING).ok()?;
    mem.read_bytes(address, length).ok()
}

fn c_string_eq(mem: &Mem64, address: u64, value: &[u8]) -> bool {
    c_string(mem, address).as_deref() == Some(value)
}
fn cxx_string_bytes(mem: &Mem64, object: u64) -> Option<Vec<u8>> {
    let first = mem.read_u64(object).ok()?;
    if first & 1 == 0 {
        let length = ((first & 0xff) / 2) as u64;
        mem.read_bytes(object + 1, length).ok()
    } else {
        let length = mem.read_u64(object + 8).ok()?;
        mem.read_bytes(first & !1, length).ok()
    }
}

fn cxx_find(text: &[u8], needle: &[u8], position: u64, reverse: bool) -> u64 {
    let start = usize::try_from(position).unwrap_or(usize::MAX).min(text.len());
    if needle.is_empty() {
        return start as u64;
    }
    if reverse {
        if needle.len() > text.len() {
            return u64::MAX;
        }
        let end = start.min(text.len() - needle.len());
        text[..=end]
            .windows(needle.len())
            .rposition(|window| window == needle)
            .map_or(u64::MAX, |index| index as u64)
    } else {
        text[start..]
            .windows(needle.len())
            .position(|window| window == needle)
            .map_or(u64::MAX, |index| (start + index) as u64)
    }
}

fn next_prime(value: u64) -> u64 {
    fn is_prime(value: u64) -> bool {
        value >= 2 && (2..=((value as f64).sqrt() as u64)).all(|divisor| value % divisor != 0)
    }
    let mut candidate = value.max(2);
    while !is_prime(candidate) {
        candidate = candidate.saturating_add(1);
    }
    candidate
}
fn objc_text(mem: &Mem64, address: u64) -> Option<Vec<u8>> {
    if objc_kind(mem, address) == Some(A64_KIND_STRING) {
        c_string(mem, objc_field(mem, address, 56))
    } else {
        c_string(mem, address)
    }
}

fn objc_text_eq(mem: &Mem64, address: u64, value: &[u8]) -> bool {
    objc_text(mem, address).as_deref() == Some(value)
}

fn metal_clear_color(context: &touchHLE_DynarmicA64Context) -> [f32; 4] {
    [
        f64::from_bits(context.vectors[0][0]) as f32,
        f64::from_bits(context.vectors[0][1]) as f32,
        f64::from_bits(context.vectors[1][0]) as f32,
        f64::from_bits(context.vectors[1][1]) as f32,
    ]
}

fn objc_object(mem: &mut Mem64, kind: u64) -> Result<u64, String> {
    let address = mem.alloc_zeroed(A64_OBJECT_SIZE).map_err(str::to_owned)?;
    mem.write_u64(address, kind).map_err(str::to_owned)?;
    Ok(address)
}

fn objc_kind(mem: &Mem64, address: u64) -> Option<u64> {
    (address != 0 && mem.allocation_size(address).is_some())
        .then(|| mem.read_u64(address).ok())
        .flatten()
}

fn objc_field(mem: &Mem64, object: u64, offset: u64) -> u64 {
    mem.read_u64(object.saturating_add(offset)).unwrap_or(0)
}

fn set_objc_field(mem: &mut Mem64, object: u64, offset: u64, value: u64) {
    let _ = mem.write_u64(object.saturating_add(offset), value);
}

fn objc_string_append(mem: &mut Mem64, left: u64, right: u64) -> Result<u64, String> {
    let mut bytes = objc_text(mem, left).unwrap_or_default();
    bytes.extend(objc_text(mem, right).unwrap_or_default());
    let value = String::from_utf8(bytes).map_err(|_| "ARM64 Objective-C string is not UTF-8")?;
    objc_string(mem, &value)
}

fn objc_string(mem: &mut Mem64, value: &str) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_STRING)?;
    let bytes = value.as_bytes();
    let pointer = mem.alloc_zeroed(bytes.len() as u64 + 1).map_err(str::to_owned)?;
    mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
    mem.write_u8(pointer + bytes.len() as u64, 0).map_err(str::to_owned)?;
    set_objc_field(mem, object, 56, pointer);
    set_objc_field(mem, object, 64, bytes.len() as u64);
    Ok(object)
}

fn objc_bundle(mem: &mut Mem64, state: &RuntimeState) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_BUNDLE)?;
    let path = objc_string(mem, &state.bundle_path)?;
    let identifier = objc_string(mem, &state.bundle_identifier)?;
    let name = objc_string(mem, &state.bundle_name)?;
    set_objc_field(mem, object, 56, path);
    set_objc_field(mem, object, 64, identifier);
    set_objc_field(mem, object, 72, name);
    Ok(object)
}

fn objc_ui_device(mem: &mut Mem64, family: DeviceFamily) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_UI_DEVICE)?;
    let machine = objc_string(mem, family.machine_name())?;
    set_objc_field(mem, object, 56, machine);
    Ok(object)
}

fn objc_ui_screen(mem: &mut Mem64, family: DeviceFamily) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_UI_SCREEN)?;
    let (width, height) = family.portrait_size();
    set_objc_field(mem, object, 56, width as u64);
    set_objc_field(mem, object, 64, height as u64);
    set_objc_field(mem, object, 72, family.scale_factor().to_bits() as u64);
    Ok(object)
}

fn objc_number(mem: &mut Mem64, value: i64) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_NUMBER)?;
    set_objc_field(mem, object, 56, value as u64);
    Ok(object)
}

fn set_float_return(context: &mut touchHLE_DynarmicA64Context, value: f64) {
    context.vectors[0][0] = value.to_bits();
    context.regs[0] = value.to_bits();
}

fn write_screen_rect(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
    family: DeviceFamily,
    scale: f64,
) -> Result<(), String> {
    let result = context.regs[8];
    if result == 0 || mem.allocation_size(result).is_none() {
        return Ok(());
    }
    let (width, height) = family.portrait_size();
    for (offset, value) in [
        (0, 0.0),
        (8, 0.0),
        (16, width as f64 / scale),
        (24, height as f64 / scale),
    ] {
        mem.write_u64(result + offset, value.to_bits()).map_err(str::to_owned)?;
    }
    Ok(())
}

fn objc_class_kind(mem: &Mem64, class_name: u64) -> u64 {
    match c_string(mem, class_name).as_deref() {
        Some(b"MTLDevice") => A64_KIND_DEVICE,
        Some(b"MTLCommandQueue") => A64_KIND_QUEUE,
        Some(b"MTLCommandBuffer") => A64_KIND_COMMAND_BUFFER,
        Some(b"MTLRenderCommandEncoder") => A64_KIND_RENDER_ENCODER,
        Some(b"MTLComputeCommandEncoder") => A64_KIND_COMPUTE_ENCODER,
        Some(b"MTLBlitCommandEncoder") => A64_KIND_BLIT_ENCODER,
        Some(b"MTLBuffer") => A64_KIND_BUFFER,
        Some(b"MTLTexture") => A64_KIND_TEXTURE,
        Some(b"MTLTextureDescriptor") => A64_KIND_TEXTURE_DESCRIPTOR,
        Some(b"MTLRenderPipelineState") => A64_KIND_PIPELINE,
        Some(b"MTLRenderPassDescriptor") => A64_KIND_RENDER_PASS_DESCRIPTOR,
        Some(b"MTLRenderPassColorAttachmentDescriptorArray") => A64_KIND_ATTACHMENT_ARRAY,
        Some(b"MTLRenderPassColorAttachmentDescriptor")
        | Some(b"MTLRenderPassDepthAttachmentDescriptor")
        | Some(b"MTLRenderPassStencilAttachmentDescriptor") => A64_KIND_ATTACHMENT,
        Some(b"MTLSamplerState") => A64_KIND_SAMPLER,
        Some(b"MTLDepthStencilState") => A64_KIND_DEPTH_STENCIL,
        Some(b"UIDevice") => A64_KIND_UI_DEVICE,
        Some(b"UIScreen") => A64_KIND_UI_SCREEN,
        Some(b"NSBundle") => A64_KIND_BUNDLE,
        Some(b"UnityFramework") => A64_KIND_UNITY_FRAMEWORK,
        _ => A64_KIND_GENERIC,
    }
}

fn objc_send(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
    state: &mut RuntimeState,
) -> Result<(), String> {
    let receiver = context.regs[0];
    let selector = c_string(mem, context.regs[1])
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    state.objc_messages = state.objc_messages.saturating_add(1);
    state.last_selector = Some(selector.clone());
    let kind = objc_kind(mem, receiver).unwrap_or(A64_KIND_GENERIC);
    let class_name = objc_field(mem, receiver, 56);

    if selector == "runUIApplicationMainWithArgc:argv:" && receiver == 0 {
        echo!("ARM64 Objective-C bootstrap call used a nil receiver; returning zero and continuing startup");
    }

    log_dbg!(
        "ARM64 Objective-C message #{}: receiver={:#x} kind={} selector={} x2={:#x} x3={:#x} x4={:#x} x5={:#x}",
        state.objc_messages,
        receiver,
        kind,
        selector,
        context.regs[2],
        context.regs[3],
        context.regs[4],
        context.regs[5]
    );
    if matches!(selector.as_str(), "commit" | "waitUntilCompleted" | "presentDrawable:" | "endEncoding") {
        state.metal_commands = state.metal_commands.saturating_add(1);
    }
    if matches!(selector.as_str(), "commit" | "presentDrawable:") {
        state.frame_serial = state.frame_serial.saturating_add(1);
        state.present_requested = true;
    }
    let result = match selector.as_str() {
        "runUIApplicationMainWithArgc:argv:" if receiver == 0 => {
            state.boot_screen_reached = true;
            state.present_requested = true;
            0
        }
        "init" | "self" | "retain" | "autorelease" | "copy" | "mutableCopy" => receiver,
        "release" => 0,
        "class" => receiver,
        "respondsToSelector:" | "isKindOfClass:" | "hasUnifiedMemory" => 1,
        "status" if kind == A64_KIND_COMMAND_BUFFER => 4,
        "error" if kind == A64_KIND_COMMAND_BUFFER => 0,
        "newFence" | "newEvent" | "newHeapWithDescriptor:" | "newArgumentEncoderWithArguments:" => objc_object(mem, A64_KIND_GENERIC)?,
        "mainBundle" if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"NSBundle") => objc_bundle(mem, state)?,
        "currentDevice" if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"UIDevice") => {
            objc_ui_device(mem, state.device_family)?
        }
        "mainScreen" if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"UIScreen") => {
            objc_ui_screen(mem, state.device_family)?
        }
        "bundleIdentifier" if kind == A64_KIND_BUNDLE => objc_field(mem, receiver, 64),
        "bundlePath" | "resourcePath" if kind == A64_KIND_BUNDLE => objc_field(mem, receiver, 56),
        "stringByAppendingString:" if kind == A64_KIND_STRING => {
            objc_string_append(mem, receiver, context.regs[2])?
        }
        "bundleWithPath:" if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"NSBundle") => {
            let object = objc_object(mem, A64_KIND_BUNDLE)?;
            let identifier = objc_string(mem, "com.unity.framework")?;
            let name = objc_string(mem, "UnityFramework")?;
            set_objc_field(mem, object, 56, context.regs[2]);
            set_objc_field(mem, object, 64, identifier);
            set_objc_field(mem, object, 72, name);
            object
        }
        "isLoaded" | "load" if kind == A64_KIND_BUNDLE => 1,
        "principalClass" if kind == A64_KIND_BUNDLE => {
            let class_name = objc_string(mem, "UnityFramework")?;
            objc_class(mem, class_name)?
        }
        "executablePath" if kind == A64_KIND_BUNDLE => objc_string(mem, &state.bundle_path)?,
        "objectForInfoDictionaryKey:" if kind == A64_KIND_BUNDLE => {
            match objc_text(mem, context.regs[2]).as_deref() {
                Some(b"CFBundleIdentifier") => objc_field(mem, receiver, 64),
                Some(b"CFBundleDisplayName") => objc_field(mem, receiver, 72),
                Some(b"CFBundleShortVersionString") => objc_string(mem, "1.0")?,
                Some(b"MinimumOSVersion") | Some(b"DTPlatformVersion") => objc_string(
                    mem,
                    &format!("{}.{}.{}", state.ios_version.0, state.ios_version.1, state.ios_version.2),
                )?,
                _ => 0,
            }
        }
        "pathForResource:ofType:" if kind == A64_KIND_BUNDLE => 0,
        "systemVersion" | "operatingSystemVersionString" => objc_string(
            mem,
            &format!("{}.{}.{}", state.ios_version.0, state.ios_version.1, state.ios_version.2),
        )?,
        "model" | "localizedModel" | "name" if kind == A64_KIND_UI_DEVICE => {
            objc_string(mem, if state.device_family.is_ipad() { "iPad" } else { "iPhone" })?
        }
        "systemName" if kind == A64_KIND_UI_DEVICE => objc_string(mem, "iPhone OS")?,
        "userInterfaceIdiom" if kind == A64_KIND_UI_DEVICE => {
            u64::from(state.device_family.is_ipad())
        }
        "bounds" | "applicationFrame" if kind == A64_KIND_UI_SCREEN => {
            write_screen_rect(mem, context, state.device_family, 1.0)?;
            0
        }
        "nativeBounds" if kind == A64_KIND_UI_SCREEN => {
            write_screen_rect(mem, context, state.device_family, state.device_family.scale_factor() as f64)?;
            0
        }
        "scale" | "nativeScale" if kind == A64_KIND_UI_SCREEN => {
            set_float_return(context, state.device_family.scale_factor() as f64);
            0
        },
        "operatingSystemVersion" => {
            context.regs[0] = (state.ios_version.0 as u32 as u64)
                | ((state.ios_version.1 as u32 as u64) << 32);
            context.regs[1] = state.ios_version.2 as u32 as u64;
            0
        }
        "isOperatingSystemAtLeastVersion:" => {
            let requested_major = context.regs[2] as i32;
            let requested_minor = (context.regs[2] >> 32) as u32 as i32;
            let requested_patch = context.regs[3] as u32 as i32;
            u64::from(
                (state.ios_version.0, state.ios_version.1, state.ios_version.2)
                    >= (requested_major, requested_minor, requested_patch),
            )
        }
        "supportsFamily:" | "supportsFeatureSet:" | "supportsTextureSampleCount:" => 1,
        "renderPassDescriptor" if kind == A64_KIND_CLASS => {
            objc_object(mem, A64_KIND_RENDER_PASS_DESCRIPTOR)?
        }
        "colorAttachments" if kind == A64_KIND_RENDER_PASS_DESCRIPTOR => {
            objc_object(mem, A64_KIND_ATTACHMENT_ARRAY)?
        }
        "depthAttachment" | "stencilAttachment" if kind == A64_KIND_RENDER_PASS_DESCRIPTOR => {
            objc_object(mem, A64_KIND_ATTACHMENT)?
        }
        "objectAtIndexedSubscript:" | "objectAtIndex:" if kind == A64_KIND_ATTACHMENT_ARRAY => {
            objc_object(mem, A64_KIND_ATTACHMENT)?
        }
        "texture" | "loadAction" | "storeAction" if kind == A64_KIND_ATTACHMENT => {
            match selector.as_str() {
                "texture" => objc_field(mem, receiver, 80),
                "loadAction" => objc_field(mem, receiver, 72),
                _ => objc_field(mem, receiver, 88),
            }
        }
        "clearColor" if kind == A64_KIND_ATTACHMENT => objc_field(mem, receiver, 56),
        "setClearColor:" if kind == A64_KIND_ATTACHMENT => {
            set_objc_field(mem, receiver, 56, context.regs[2]);
            0
        }
        "setTexture:" if kind == A64_KIND_ATTACHMENT => {
            set_objc_field(mem, receiver, 80, context.regs[2]);
            0
        }
        "setLoadAction:" if kind == A64_KIND_ATTACHMENT => {
            set_objc_field(mem, receiver, 72, context.regs[2]);
            0
        }
        "setStoreAction:" if kind == A64_KIND_ATTACHMENT => {
            set_objc_field(mem, receiver, 88, context.regs[2]);
            0
        }
        "setClearColor:" => {
            state.clear_color = metal_clear_color(context);
            0
        }
        "setLabel:" | "setCullMode:" | "setFrontFacingWinding:" | "setTriangleFillMode:" | "setDepthStencilState:" | "setViewport:" | "setScissorRect:" | "setVertexBytes:length:atIndex:" | "setFragmentBytes:length:atIndex:" | "setVertexBufferOffset:atIndex:" | "setFragmentBufferOffset:atIndex:" => 0,
        "drawPrimitives:vertexStart:vertexCount:" | "drawIndexedPrimitives:indexCount:indexType:indexBuffer:indexBufferOffset:" | "dispatchThreadgroups:threadsPerThreadgroup:" => {
            state.metal_commands = state.metal_commands.saturating_add(1);
            0
        }
        "name" => objc_string(mem, "RadekHLE Metal device")?,
        "UTF8String" => objc_field(mem, receiver, 56),
        "length" if kind == A64_KIND_STRING => objc_field(mem, receiver, 64),
        "length" if kind == A64_KIND_BUFFER => objc_field(mem, receiver, 64),
        "boolValue" if kind == A64_KIND_STRING => u64::from(!objc_text_eq(mem, receiver, b"0")),
        "boolValue" if kind == A64_KIND_NUMBER => u64::from(objc_field(mem, receiver, 56) != 0),
        "intValue" | "integerValue" | "longLongValue" if kind == A64_KIND_NUMBER => objc_field(mem, receiver, 56),
        "unsignedIntegerValue" if kind == A64_KIND_NUMBER => objc_field(mem, receiver, 56),
        "doubleValue" | "floatValue" if kind == A64_KIND_NUMBER => {
            set_float_return(context, i64::from_ne_bytes(objc_field(mem, receiver, 56).to_ne_bytes()) as f64);
            0
        }
        "isEqualToString:" | "isEqual:" if kind == A64_KIND_STRING => {
            u64::from(objc_text(mem, receiver) == objc_text(mem, context.regs[2]))
        }
        "cStringUsingEncoding:" if kind == A64_KIND_STRING => objc_field(mem, receiver, 56),
        "getInstance" if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"UnityFramework") => {
            state.transfer_to_method(selector.as_str(), &["+[UnityFramework getInstance]", "getInstance"])?;
            receiver
        }
        "appController" if kind == A64_KIND_UNITY_FRAMEWORK => 0,
        "setExecuteHeader:" if kind == A64_KIND_UNITY_FRAMEWORK => 0,
        "runUIApplicationMainWithArgc:argv:" if kind == A64_KIND_UNITY_FRAMEWORK => {
            state.transfer_to_method(
                selector.as_str(),
                &["-[UnityFramework runUIApplicationMainWithArgc:argv:]", "runUIApplicationMainWithArgc:argv:"],
            )?;
            receiver
        }
        "newCommandQueue" | "newCommandQueueWithMaxCommandBufferCount:" => objc_object(mem, A64_KIND_QUEUE)?,
        "commandBuffer" | "commandBufferWithUnretainedReferences" => objc_object(mem, A64_KIND_COMMAND_BUFFER)?,
        "renderCommandEncoderWithDescriptor:" => objc_object(mem, A64_KIND_RENDER_ENCODER)?,
        "computeCommandEncoder" => objc_object(mem, A64_KIND_COMPUTE_ENCODER)?,
        "blitCommandEncoder" => objc_object(mem, A64_KIND_BLIT_ENCODER)?,
        "newRenderPipelineStateWithDescriptor:error:" => objc_object(mem, A64_KIND_PIPELINE)?,
        "newDepthStencilStateWithDescriptor:" => objc_object(mem, A64_KIND_DEPTH_STENCIL)?,
        "newSamplerStateWithDescriptor:" => objc_object(mem, A64_KIND_SAMPLER)?,
        "newTextureViewWithPixelFormat:" => objc_object(mem, A64_KIND_TEXTURE)?,
        "newBufferWithLength:options:" => {
            let object = objc_object(mem, A64_KIND_BUFFER)?;
            let length = context.regs[2];
            let contents = mem.alloc_zeroed(length).map_err(str::to_owned)?;
            set_objc_field(mem, object, 56, contents);
            set_objc_field(mem, object, 64, length);
            set_objc_field(mem, object, 72, context.regs[3]);
            set_objc_field(mem, object, 80, receiver);
            object
        }
        "newBufferWithBytes:length:options:" => {
            let object = objc_object(mem, A64_KIND_BUFFER)?;
            let source = context.regs[2];
            let length = context.regs[3];
            let contents = mem.alloc_zeroed(length).map_err(str::to_owned)?;
            if source != 0 && length != 0 {
                let bytes = mem.read_bytes(source, length).map_err(str::to_owned)?;
                mem.write_bytes(contents, &bytes).map_err(str::to_owned)?;
            }
            set_objc_field(mem, object, 56, contents);
            set_objc_field(mem, object, 64, length);
            set_objc_field(mem, object, 72, context.regs[4]);
            set_objc_field(mem, object, 80, receiver);
            object
        }
        "newTextureWithDescriptor:" => {
            let object = objc_object(mem, A64_KIND_TEXTURE)?;
            for offset in [8, 16, 24, 32, 40, 48] {
                set_objc_field(mem, object, offset, objc_field(mem, context.regs[2], offset));
            }
            set_objc_field(mem, object, 80, receiver);
            object
        }
        "texture2DDescriptorWithPixelFormat:width:height:mipmapped:" => {
            let object = objc_object(mem, A64_KIND_TEXTURE_DESCRIPTOR)?;
            set_objc_field(mem, object, 8, context.regs[2]);
            set_objc_field(mem, object, 16, context.regs[3]);
            set_objc_field(mem, object, 24, context.regs[4]);
            set_objc_field(mem, object, 32, 1);
            set_objc_field(mem, object, 40, if context.regs[5] != 0 { 0 } else { 1 });
            set_objc_field(mem, object, 48, 1);
            object
        }
        "pixelFormat" => objc_field(mem, receiver, 8),
        "width" => objc_field(mem, receiver, 16),
        "height" => objc_field(mem, receiver, 24),
        "depth" => objc_field(mem, receiver, 32),
        "mipmapLevelCount" => objc_field(mem, receiver, 40),
        "sampleCount" => objc_field(mem, receiver, 48),
        "contents" => objc_field(mem, receiver, 56),
        "storageMode" => objc_field(mem, receiver, 72),
        "device" => objc_field(mem, receiver, 80),
        selector if selector.starts_with("set") && selector.ends_with(':') => {
            let value = context.regs[2];
            let offset = match selector {
                "setPixelFormat:" => 8,
                "setWidth:" => 16,
                "setHeight:" => 24,
                "setDepth:" => 32,
                "setMipmapLevelCount:" => 40,
                "setSampleCount:" => 48,
                _ => 72,
            };
            set_objc_field(mem, receiver, offset, value);
            0
        }
        "alloc" | "new" if kind == A64_KIND_CLASS => objc_object(mem, objc_class_kind(mem, class_name))?,
        _ => 0,
    };
    if state.guest_transfer_pc.is_none() {
        return_value(context, result);
    }
    Ok(())
}

fn objc_class(mem: &mut Mem64, name: u64) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_CLASS)?;
    set_objc_field(mem, object, 56, name);
    Ok(object)
}

pub fn materialize_import(mem: &mut Mem64, symbol: &str) -> Result<Option<u64>, String> {
    if let Some(value) = materialize_host_constant(mem, symbol)? {
        return Ok(Some(value));
    }
    let symbol = name(symbol);
    if let Some(class_name) = symbol.strip_prefix("OBJC_CLASS_$_") {
        let pointer = mem.alloc_zeroed(class_name.len() as u64 + 1).map_err(str::to_owned)?;
        mem.write_bytes(pointer, class_name.as_bytes()).map_err(str::to_owned)?;
        mem.write_u8(pointer + class_name.len() as u64, 0).map_err(str::to_owned)?;
        return Ok(Some(objc_class(mem, pointer)?));
    }
    if let Some(class_name) = symbol.strip_prefix("OBJC_METACLASS_$_") {
        let pointer = mem.alloc_zeroed(class_name.len() as u64 + 1).map_err(str::to_owned)?;
        mem.write_bytes(pointer, class_name.as_bytes()).map_err(str::to_owned)?;
        mem.write_u8(pointer + class_name.len() as u64, 0).map_err(str::to_owned)?;
        return Ok(Some(objc_class(mem, pointer)?));
    }
    if symbol == "CFConstantStringClassReference" {
        let bytes = b"NSConstantString";
        let pointer = mem.alloc_zeroed(bytes.len() as u64 + 1).map_err(str::to_owned)?;
        mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
        mem.write_u8(pointer + bytes.len() as u64, 0).map_err(str::to_owned)?;
        return Ok(Some(objc_class(mem, pointer)?));
    }
    if symbol == "stack_chk_guard" {
        let guard = mem.alloc_zeroed(8).map_err(str::to_owned)?;
        mem.write_u64(guard, 0x9e37_79b9_7f4a_7c15).map_err(str::to_owned)?;
        return Ok(Some(guard));
    }
    Ok(None)
}

pub fn dispatch(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
    symbol: &str,
    state: &mut RuntimeState,
) -> Result<bool, String> {
    state.host_dispatches = state.host_dispatches.saturating_add(1);
    let symbol = name(symbol);
    state.last_symbol = Some(symbol.to_owned());
    if state.host_dispatches <= 128 || state.host_dispatches.is_power_of_two() {
        log_dbg!(
            "ARM64 host dispatch #{}: symbol={} backend={} iOS={}.{}.{}",
            state.host_dispatches,
            symbol,
            state.graphics_backend.label(),
            state.ios_version.0,
            state.ios_version.1,
            state.ios_version.2
        );
    }
    match symbol {
        "malloc" | "calloc" | "valloc" | "posix_memalign" => {
            let size = if symbol == "calloc" {
                A64Abi::arg(context, 0)
                    .checked_mul(A64Abi::arg(context, 1))
                    .ok_or("ARM64 calloc size overflows")?
            } else if symbol == "posix_memalign" {
                A64Abi::arg(context, 2)
            } else {
                A64Abi::arg(context, 0)
            };
            let address = mem.alloc_zeroed(size).map_err(str::to_owned)?;
            if symbol == "posix_memalign" {
                mem.write_u64(context.regs[0], address).map_err(str::to_owned)?;
                return_value(context, 0);
            } else {
                return_value(context, address);
            }
            Ok(true)
        }
        "free" | "malloc_zone_free" => {
            if context.regs[0] != 0 {
                mem.free(context.regs[0]);
            }
            return_value(context, 0);
            Ok(true)
        }
        "objc_release" => {
            return_value(context, 0);
            Ok(true)
        }
        "objc_storeStrong" => {
            if context.regs[0] != 0 {
                mem.write_u64(context.regs[0], context.regs[1]).map_err(str::to_owned)?;
            }
            return_value(context, context.regs[1]);
            Ok(true)
        }
        "realloc" | "malloc_zone_realloc" => {
            let old = context.regs[0];
            let size = context.regs[1];
            let address = mem.alloc_zeroed(size).map_err(str::to_owned)?;
            if old != 0 {
                if let Some(old_size) = mem.allocation_size(old) {
                    mem.copy_bytes(address, old, old_size.min(size)).map_err(str::to_owned)?;
                }
            }
            return_value(context, address);
            Ok(true)
        }
        "memcpy" | "memmove" | "memcpy_chk" | "memmove_chk" => {
            let size = context.regs[2];
            mem.copy_bytes(context.regs[0], context.regs[1], size).map_err(str::to_owned)?;
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "bcopy" => {
            mem.copy_bytes(context.regs[1], context.regs[0], context.regs[2]).map_err(str::to_owned)?;
            return_value(context, context.regs[1]);
            Ok(true)
        }
        "memchr" => {
            let base = context.regs[0];
            let value = context.regs[1] as u8;
            let length = context.regs[2];
            let mut result = 0;
            for offset in 0..length {
                if mem.read_u8(base + offset).map_err(str::to_owned)? == value {
                    result = base + offset;
                    break;
                }
            }
            return_value(context, result);
            Ok(true)
        }
        "memset_pattern4" | "memset_pattern8" | "memset_pattern16" => {
            let pattern_size = match symbol {
                "memset_pattern4" => 4,
                "memset_pattern8" => 8,
                _ => 16,
            };
            let pattern = mem.read_bytes(context.regs[1], pattern_size).map_err(str::to_owned)?;
            let length = usize::try_from(context.regs[2]).map_err(|_| "ARM64 pattern fill is too large")?;
            let mut bytes = Vec::with_capacity(length);
            while bytes.len() < length {
                bytes.extend(pattern.iter().copied().take(length - bytes.len()));
            }
            mem.write_bytes(context.regs[0], &bytes).map_err(str::to_owned)?;
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "memset" | "bzero" | "memset_chk" => {
            let size = if symbol == "bzero" { context.regs[1] } else { context.regs[2] };
            let value = if symbol == "bzero" { 0 } else { context.regs[1] as u8 };
            mem.fill_bytes(context.regs[0], value, size).map_err(str::to_owned)?;
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "strlen" => {
            return_value(context, mem.cstr_len(context.regs[0], 1024 * 1024).map_err(str::to_owned)?);
            Ok(true)
        }
        "strnlen" => {
            let limit = context.regs[1];
            let mut length = 0;
            while length < limit && mem.read_u8(context.regs[0] + length).map_err(str::to_owned)? != 0 {
                length += 1;
            }
            return_value(context, length);
            Ok(true)
        }
        "strcpy" | "strncpy" => {
            let source = c_string(mem, context.regs[1]).ok_or("ARM64 source string is not readable")?;
            let limit = if symbol == "strncpy" { usize::try_from(context.regs[2]).map_err(|_| "ARM64 strncpy length is too large")? } else { source.len() + 1 };
            let mut bytes = vec![0; limit];
            let copy_len = source.len().min(limit);
            bytes[..copy_len].copy_from_slice(&source[..copy_len]);
            mem.write_bytes(context.regs[0], &bytes).map_err(str::to_owned)?;
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "strcat" | "strncat" => {
            let destination_len = mem.cstr_len(context.regs[0], MAX_CSTRING).map_err(str::to_owned)?;
            let source = c_string(mem, context.regs[1]).ok_or("ARM64 source string is not readable")?;
            let source_len = if symbol == "strncat" { source.len().min(usize::try_from(context.regs[2]).map_err(|_| "ARM64 strncat length is too large")?) } else { source.len() };
            let destination = context.regs[0].checked_add(destination_len).ok_or("ARM64 strcat address overflows")?;
            mem.write_bytes(destination, &source[..source_len]).map_err(str::to_owned)?;
            mem.write_u8(destination + source_len as u64, 0).map_err(str::to_owned)?;
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "strdup" | "strndup" => {
            let source = c_string(mem, context.regs[0]).ok_or("ARM64 source string is not readable")?;
            let length = if symbol == "strndup" { source.len().min(usize::try_from(context.regs[1]).map_err(|_| "ARM64 strndup length is too large")?) } else { source.len() };
            let address = mem.alloc_zeroed(length as u64 + 1).map_err(str::to_owned)?;
            mem.write_bytes(address, &source[..length]).map_err(str::to_owned)?;
            mem.write_u8(address + length as u64, 0).map_err(str::to_owned)?;
            return_value(context, address);
            Ok(true)
        }
        "strchr" | "strrchr" => {
            let text = c_string(mem, context.regs[0]).unwrap_or_default();
            let needle = context.regs[1] as u8;
            let index = if symbol == "strchr" {
                text.iter().position(|&byte| byte == needle)
            } else {
                text.iter().rposition(|&byte| byte == needle)
            };
            let index = index.or_else(|| (needle == 0).then_some(text.len()));
            return_value(context, index.map_or(0, |index| context.regs[0] + index as u64));
            Ok(true)
        }
        "strstr" => {
            let haystack = c_string(mem, context.regs[0]).unwrap_or_default();
            let needle = c_string(mem, context.regs[1]).unwrap_or_default();
            let index = if needle.is_empty() {
                Some(0)
            } else {
                haystack.windows(needle.len()).position(|window| window == needle)
            };
            return_value(context, index.map_or(0, |index| context.regs[0] + index as u64));
            Ok(true)
        }
        "strcasecmp" | "strncasecmp" => {
            let left = c_string(mem, context.regs[0]).unwrap_or_default();
            let right = c_string(mem, context.regs[1]).unwrap_or_default();
            let limit = if symbol == "strncasecmp" { context.regs[2] as usize } else { usize::MAX };
            let result = left.iter().map(u8::to_ascii_lowercase).take(limit).zip(right.iter().map(u8::to_ascii_lowercase).take(limit)).find_map(|(a, b)| (a != b).then_some((a as i32) - (b as i32))).unwrap_or_else(|| (left.len().min(limit) as i32) - (right.len().min(limit) as i32));
            return_value(context, result as i64 as u64);
            Ok(true)
        }
        "strcmp" | "strncmp" => {
            let left = c_string(mem, context.regs[0]).unwrap_or_default();
            let right = c_string(mem, context.regs[1]).unwrap_or_default();
            let limit = if symbol == "strncmp" { context.regs[2] as usize } else { usize::MAX };
            let result = left.iter().take(limit).zip(right.iter().take(limit)).find_map(|(a, b)| (a != b).then_some((*a as i32) - (*b as i32))).unwrap_or_else(|| {
                let left_len = left.len().min(limit);
                let right_len = right.len().min(limit);
                (left_len as i32) - (right_len as i32)
            });
            return_value(context, result as i64 as u64);
            Ok(true)
        }
        "memcmp" => {
            let size = context.regs[2];
            let left = mem.read_bytes(context.regs[0], size).map_err(str::to_owned)?;
            let right = mem.read_bytes(context.regs[1], size).map_err(str::to_owned)?;
            let result = left.iter().zip(right.iter()).find_map(|(a, b)| (a != b).then_some((*a as i32) - (*b as i32))).unwrap_or(0);
            return_value(context, result as i64 as u64);
            Ok(true)
        }
        "bcmp" => {
            let size = context.regs[2];
            let left = mem.read_bytes(context.regs[0], size).map_err(str::to_owned)?;
            let right = mem.read_bytes(context.regs[1], size).map_err(str::to_owned)?;
            return_value(context, u64::from(left != right));
            Ok(true)
        }
        "objc_retain" | "objc_retainAutoreleasedReturnValue" | "objc_retainAutoreleaseReturnValue" | "objc_autorelease" | "objc_autoreleaseReturnValue" | "objc_unsafeClaimAutoreleasedReturnValue" | "objc_retainAutorelease" | "objc_retainBlock" => {
            let value = context.regs[0];
            return_value(context, value);
            Ok(true)
        }
        "objc_msgSend" | "objc_msgSendSuper2" | "objc_msgSend_stret" | "objc_msgSendSuper2_stret" => {
            objc_send(mem, context, state)?;
            Ok(true)
        }
        "objc_msgSend_fpret" | "objc_msgSend_fp2ret" => {
            objc_send(mem, context, state)?;
            Ok(true)
        }
        "objc_getClass" | "objc_getRequiredClass" | "objc_lookUpClass" => {
            let class = objc_class(mem, context.regs[0])?;
            return_value(context, class);
            Ok(true)
        }
        "object_getClass" => {
            return_value(context, if context.regs[0] == 0 { 0 } else { objc_class(mem, context.regs[0])? });
            Ok(true)
        }
        "object_getClassName" => {
            let class_name = objc_field(mem, context.regs[0], 56);
            return_value(context, class_name);
            Ok(true)
        }
        "sel_registerName" | "sel_getUid" => {
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "objc_autoreleasePoolPush" => {
            let address = mem.alloc_zeroed(8).map_err(str::to_owned)?;
            return_value(context, address);
            Ok(true)
        }
        "objc_autoreleasePoolPop" | "objc_exception_throw" | "objc_begin_catch" | "objc_end_catch" => {
            return_value(context, 0);
            Ok(true)
        }
        "cxa_guard_acquire" => {
            let guard = context.regs[0];
            let initialized = mem.read_u64(guard).map_err(str::to_owned)? != 0;
            if !initialized {
                mem.write_u64(guard, 1).map_err(str::to_owned)?;
            }
            return_value(context, u64::from(!initialized));
            Ok(true)
        }
        "cxa_guard_release" => {
            mem.write_u64(context.regs[0], 2).map_err(str::to_owned)?;
            return_value(context, 0);
            Ok(true)
        }
        "cxa_guard_abort" => {
            mem.write_u64(context.regs[0], 0).map_err(str::to_owned)?;
            return_value(context, 0);
            Ok(true)
        }
        "cxa_pure_virtual" | "stack_chk_fail" | "stack_chk_fail_local" => {
            log!("Warning: ARM64 runtime bypassed {}", symbol);
            return_value(context, 0);
            Ok(true)
        }
        "gxx_personality_v0"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEED1Ev" => {
            return_value(context, 0);
            Ok(true)
        }
        "inflateInit_" => {
            return_value(context, 0);
            Ok(true)
        }
        "inflate" => {
            return_value(context, 0);
            Ok(true)
        }
        "inflateEnd" => {
            return_value(context, 0);
            Ok(true)
        }
        "class_addProperty" | "class_addProtocol" | "class_getInstanceVariable" | "class_isMetaClass"
        | "objc_constructInstance" | "objc_initializeClassPair" | "object_getIvar" | "property_copyAttributeList" => {
            return_value(context, 0);
            Ok(true)
        }
        "objc_enumerationMutation" => {
            return_value(context, 0);
            Ok(true)
        }
        "ZNKSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE4findEPKcmm"
        | "ZNKSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE4findEcm" => {
            let Some(text) = cxx_string_bytes(mem, context.regs[0]) else { return Ok(true) };
            let needle = if symbol.ends_with("findEcm") { vec![context.regs[2] as u8] } else { c_string(mem, context.regs[2]).unwrap_or_default() };
            return_value(context, cxx_find(&text, &needle, context.regs[3], false));
            Ok(true)
        }
        "ZNKSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE5rfindEPKcmm"
        | "ZNKSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE5rfindEcm" => {
            let Some(text) = cxx_string_bytes(mem, context.regs[0]) else { return Ok(true) };
            let needle = if symbol.ends_with("rfindEcm") { vec![context.regs[2] as u8] } else { c_string(mem, context.regs[2]).unwrap_or_default() };
            return_value(context, cxx_find(&text, &needle, context.regs[3], true));
            Ok(true)
        }
        "ZNKSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE7compareEPKc" => {
            let left = cxx_string_bytes(mem, context.regs[0]).unwrap_or_default();
            let right = c_string(mem, context.regs[2]).unwrap_or_default();
            return_value(context, u64::from(left.cmp(&right) as i8 as i64 as u64));
            Ok(true)
        }
        "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6eraseEmm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6__initEPKcmm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6__initEmc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6appendEPKc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6appendEPKcm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6assignEPKc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6assignEPKcm" => {
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "ZNKSt3__120__vector_base_commonILb1EE20__throw_length_errorEv"
        | "ZNKSt3__120__vector_base_commonILb1EE20__throw_out_of_rangeEv"
        | "ZNKSt3__121__basic_string_commonILb1EE20__throw_length_errorEv"
        | "ZNKSt3__16locale9has_facetERNS0_2idE" | "ZNKSt3__16locale9use_facetERNS0_2idE"
        | "ZNKSt3__18ios_base6getlocEv" | "ZNSt3__111this_thread9sleep_forERKNS_6chrono8durationIxNS_5ratioILl1ELl1000000000EEEEE"
        | "ZNSt3__112__next_primeEm" => {
            return_value(context, if symbol.ends_with("next_primeEm") { next_prime(context.regs[0]) } else { 0 });
            Ok(true)
        }
        "compressBound" => {
            let source_len = context.regs[0];
            let bound = source_len
                .saturating_add(source_len / 1000)
                .saturating_add(12);
            return_value(context, bound);
            Ok(true)
        }
        "deflateInit_" => {
            return_value(context, 0);
            Ok(true)
        }
        "deflate" => {
            return_value(context, 0);
            Ok(true)
        }
        "deflateEnd" | "deflateReset" => {
            return_value(context, 0);
            Ok(true)
        }
        "cxa_atexit" | "atexit" | "pthread_mutex_lock" | "pthread_mutex_unlock" | "pthread_mutex_init" | "pthread_mutex_destroy" | "pthread_once" | "pthread_key_create" | "pthread_setspecific" | "sched_yield" => {
            if symbol == "pthread_once" && context.regs[0] != 0 && mem.read_u32(context.regs[0]).map_err(str::to_owned)? == 0 {
                mem.write_u32(context.regs[0], 1).map_err(str::to_owned)?;
            }
            return_value(context, 0);
            Ok(true)
        }
        "pthread_getspecific" => {
            return_value(context, 0);
            Ok(true)
        }
        "pthread_self" => {
            return_value(context, 1);
            Ok(true)
        }
        "abort" | "exit" | "_exit" => {
            log!("Warning: ARM64 app requested {}; returning to emulator", symbol);
            return_value(context, 0);
            Ok(true)
        }
        "NSLog" | "NSLogv" | "os_log" | "os_logv" => {
            return_value(context, 0);
            Ok(true)
        }
        "UIApplicationMain" => {
            state.boot_screen_reached = true;
            state.present_requested = true;
            echo!(
                "ARM64 UIApplicationMain handled by the compatibility runtime; continuing app startup"
            );
            return_value(context, 0);
            Ok(true)
        }
        "dyld_stub_binder" => {
            return_value(context, 0);
            Ok(true)
        }
        "CFConstantStringClassReference" => {
            let class = materialize_import(mem, "OBJC_CLASS_$_NSConstantString")?.unwrap_or(0);
            return_value(context, class);
            Ok(true)
        }
        symbol if symbol.starts_with("OBJC_CLASS_$_") || symbol.starts_with("OBJC_METACLASS_$_") => {
            return_value(context, materialize_import(mem, symbol)?.unwrap_or(0));
            Ok(true)
        }
        _ if c_string_eq(mem, context.regs[0], b"NSConcreteGlobalBlock") || c_string_eq(mem, context.regs[0], b"NSConcreteStackBlock") => {
            return_value(context, 0);
            Ok(true)
        }
        "MTLCreateSystemDefaultDevice" => {
            let object = objc_object(mem, A64_KIND_DEVICE)?;
            state.metal_commands = state.metal_commands.saturating_add(1);
            log_dbg!("ARM64 Metal device creation #{} using {}", state.metal_commands, state.graphics_backend.label());
            return_value(context, object);
            Ok(true)
        }
        "vkEnumerateInstanceVersion" => {
            if context.regs[0] != 0 {
                mem.write_u32(context.regs[0], (1 << 22) | (3 << 12)).map_err(str::to_owned)?;
            }
            return_value(context, 0);
            Ok(true)
        }
        "vkCreateInstance" => {
            if context.regs[2] == 0 {
                return_value(context, u64::MAX);
            } else {
                let handle = mem.alloc_zeroed(8).map_err(str::to_owned)?;
                mem.write_u64(context.regs[2], handle).map_err(str::to_owned)?;
                return_value(context, 0);
            }
            Ok(true)
        }
        "vkDestroyInstance" | "vkDestroyDevice" => {
            return_value(context, 0);
            Ok(true)
        }
        "vkEnumeratePhysicalDevices" => {
            if context.regs[1] == 0 {
                return_value(context, u64::MAX);
            } else {
                mem.write_u32(context.regs[1], 1).map_err(str::to_owned)?;
                if context.regs[2] != 0 {
                    let handle = mem.alloc_zeroed(8).map_err(str::to_owned)?;
                    mem.write_u64(context.regs[2], handle).map_err(str::to_owned)?;
                }
                return_value(context, 0);
            }
            Ok(true)
        }
        "vkGetPhysicalDeviceQueueFamilyProperties" => {
            if context.regs[1] != 0 {
                mem.write_u32(context.regs[1], 1).map_err(str::to_owned)?;
                if context.regs[2] != 0 {
                    let values = [1u32, 1, 64, 1, 1, 1];
                    for (index, value) in values.iter().enumerate() {
                        mem.write_u32(context.regs[2] + (index as u64 * 4), *value).map_err(str::to_owned)?;
                    }
                }
            }
            return_value(context, 0);
            Ok(true)
        }
        "vkCreateDevice" => {
            if context.regs[3] == 0 {
                return_value(context, u64::MAX);
            } else {
                let handle = mem.alloc_zeroed(8).map_err(str::to_owned)?;
                mem.write_u64(context.regs[3], handle).map_err(str::to_owned)?;
                return_value(context, 0);
            }
            Ok(true)
        }
        "vkGetDeviceQueue" => {
            if context.regs[3] != 0 {
                let handle = mem.alloc_zeroed(8).map_err(str::to_owned)?;
                mem.write_u64(context.regs[3], handle).map_err(str::to_owned)?;
            }
            return_value(context, 0);
            Ok(true)
        }
        "vkDeviceWaitIdle" => {
            state.metal_commands = state.metal_commands.saturating_add(1);
            log_dbg!("ARM64 Vulkan device idle call #{}", state.metal_commands);
            return_value(context, 0);
            Ok(true)
        }
        _ if symbol.starts_with("gl") || symbol.starts_with("egl") || symbol.starts_with("EAGL") => {
            return_value(context, 0);
            Ok(true)
        }
        value if value.starts_with("ZNSt3__") || value.starts_with("ZNKSt3__") => {
            return_value(context, 0);
            Ok(true)
        }
        _ => Ok(false),
    }
}
