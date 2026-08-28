use crate::a64_abi::A64Abi;
use crate::dyld::{search_host_dylibs, HostConstant};
use crate::mach_o64::ObjCClass64;
use crate::mem64::Mem64;
use crate::window::{DeviceFamily, DeviceOrientation, Window};
use std::collections::{HashMap, HashSet};
use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

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
const A64_KIND_APPLICATION: u64 = 24;
const A64_KIND_DISPLAY_LINK: u64 = 25;
const A64_KIND_RUN_LOOP: u64 = 26;
const A64_KIND_THREAD: u64 = 27;
const A64_KIND_VIEW: u64 = 28;
const A64_KIND_EAGL_VIEW: u64 = 29;
const A64_KIND_CONTEXT: u64 = 30;
const A64_KIND_ARRAY: u64 = 31;
const A64_KIND_DICTIONARY: u64 = 32;
const A64_KIND_DATA: u64 = 33;
const A64_KIND_SET: u64 = 34;
const A64_KIND_MUTABLE_ARRAY: u64 = 35;
const A64_KIND_MUTABLE_STRING: u64 = 36;
const A64_KIND_MUTABLE_DICTIONARY: u64 = 37;
const A64_UIVIEWCONTROLLER_VIEW_IVAR: u64 = 0x148;
const MAX_ARM64_PTHREAD_WORKERS: usize = 16;

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
    pub orientation: DeviceOrientation,
    pub screen_width: u32,
    pub screen_height: u32,
    pub host_dispatches: u64,
    pub objc_messages: u64,
    pub metal_commands: u64,
    pub frame_serial: u64,
    pub present_requested: bool,
    pub boot_screen_reached: bool,
    pub boot_screen_notification_pending: bool,
    pub application_main_active: bool,
    pub application_main_calls: u64,
    pub application_bootstrap_requested: bool,
    pub arm64_rng_state: u32,
    pub clear_color: [f32; 4],
    pub last_selector: Option<String>,
    pub last_symbol: Option<String>,
    pub last_successful_symbol: Option<String>,
    pub current_module: Option<String>,
    pub bundle_identifier: String,
    pub bundle_path: String,
    pub bundle_name: String,
    pub unimplemented_symbols: HashSet<String>,
    pub reached_unimplemented_symbols: HashSet<String>,
    pub loaded_images: Vec<LoadedImage>,
    pub guest_transfer_pc: Option<u64>,
    pub unity_framework_instance: Option<u64>,
    pub objc_classes: Vec<ObjCClass64>,
    pub class_objects: HashMap<String, u64>,
    pub application_object: Option<u64>,
    pub application_delegate: Option<u64>,
    pub application_window: Option<u64>,
    pub application_view_controller: Option<u64>,
    pub application_view: Option<u64>,
    pub application_eagl_view: Option<u64>,
    pub graphics_context: Option<u64>,
    pub eagl_context_api: u32,
    pub main_nib_name: Option<String>,
    pub launch_callback_return_pc: Option<u64>,
    pub application_return_stub: Option<u64>,
    pub application_launch_return_stub: Option<u64>,
    pub application_active_return_stub: Option<u64>,
    pub nib_awake_return_stub: Option<u64>,
    pub guest_method_return_stub: Option<u64>,
    pub guest_method_return_pcs: Vec<u64>,
    pub nib_awake_dispatched: bool,
    pub pending_application_did_become_active: bool,
    pub display_link_object: Option<u64>,
    pub display_link_target: Option<u64>,
    pub display_link_selector: Option<u64>,
    pub display_link_scheduled: bool,
    pub display_link_frame_interval: u64,
    pub display_link_return_stub: Option<u64>,
    pub display_link_callbacks: u64,
    pub display_link_callback_returned: bool,
    pub display_link_return_pc: Option<u64>,
    pub guest_yield_requested: bool,
    pub arm64_current_context: Option<u64>,
    pub arm64_gl_present_requested: bool,
    pub arm64_gl: Option<Arm64GuestGlState>,
    pub arm64_application_bootstrap_dispatched: bool,
    pub next_pthread_id: u64,
    pub next_pthread_key: u64,
    pub pthread_handles: HashMap<u64, u64>,
    pub pthread_finished: HashSet<u64>,
    pub pthread_once_controls: HashSet<u64>,
    pub pthread_key_values: HashMap<u64, u64>,
    pub signal_handlers: HashMap<i32, u64>,
    pub render_diagnostics: A64RenderDiagnostics,
}

#[derive(Debug, Default)]
pub struct A64RenderDiagnostics {
    pub display_link_callbacks: u64,
    pub set_framebuffer_calls: u64,
    pub present_framebuffer_calls: u64,
    pub gl_calls: u64,
    pub trace_events: u64,
    pub last_gl_symbol: Option<String>,
    pub last_gl_pc: u64,
    pub last_callback_pc: u64,
    pub last_guest_pc: u64,
    pub callback_entry_lr: u64,
    pub callback_return_pc: u64,
    pub callback_active: bool,
    pub last_unresolved_symbol: Option<String>,
    pub last_unresolved_pc: u64,
    pub last_dispatch_receiver: u64,
    pub last_dispatch_selector: Option<String>,
    pub last_dispatch_pc: u64,
    pub last_dispatch_lr: u64,
    pub last_dispatch_sp: u64,
    pub last_dispatch_callback_target: u64,
    pub missing_selectors: HashSet<String>,
}

#[derive(Debug, Default)]
pub struct Arm64GuestGlState {
    pub current_framebuffer: u32,
    pub current_renderbuffer: u32,
    pub viewport: [i32; 4],
    pub clear_color: [f32; 4],
    pub gl_error: u32,
    pub draw_calls: u64,
    pub bind_framebuffer_calls: u64,
    pub last_bind_framebuffer_pc: u64,
    pub last_bind_framebuffer_target: u32,
    pub last_bind_framebuffer: u32,
    pub array_buffer_binding: u32,
    pub element_array_buffer_binding: u32,
    pub strings: HashMap<u32, u64>,
}

impl RuntimeState {
    pub fn new(
        ios_version: (i32, i32, i32),
        graphics_backend: A64GraphicsBackend,
        device_family: DeviceFamily,
        orientation: DeviceOrientation,
    ) -> Self {
        let (portrait_width, portrait_height) = device_family.portrait_size();
        let (screen_width, screen_height) = match orientation {
            DeviceOrientation::Portrait | DeviceOrientation::PortraitUpsideDown => {
                (portrait_width, portrait_height)
            }
            DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight => {
                (portrait_height, portrait_width)
            }
        };
        Self {
            ios_version,
            graphics_backend,
            device_family,
            orientation,
            screen_width: screen_width as u32,
            screen_height: screen_height as u32,
            host_dispatches: 0,
            objc_messages: 0,
            metal_commands: 0,
            frame_serial: 0,
            present_requested: false,
            boot_screen_reached: false,
            boot_screen_notification_pending: false,
            application_main_active: false,
            application_main_calls: 0,
            application_bootstrap_requested: false,
            arm64_rng_state: 1,
            clear_color: [0.0, 0.0, 0.0, 1.0],
            last_selector: None,
            last_symbol: None,
            last_successful_symbol: None,
            current_module: None,
            bundle_identifier: "org.touchhle.app".to_owned(),
            bundle_path: "/".to_owned(),
            bundle_name: "Application".to_owned(),
            unimplemented_symbols: HashSet::new(),
            reached_unimplemented_symbols: HashSet::new(),
            loaded_images: Vec::new(),
            guest_transfer_pc: None,
            unity_framework_instance: None,
            objc_classes: Vec::new(),
            class_objects: HashMap::new(),
            application_object: None,
            application_delegate: None,
            application_window: None,
            application_view_controller: None,
            application_view: None,
            application_eagl_view: None,
            graphics_context: None,
            eagl_context_api: 1,
            main_nib_name: None,
            launch_callback_return_pc: None,
            application_return_stub: None,
            application_launch_return_stub: None,
            application_active_return_stub: None,
            nib_awake_return_stub: None,
            guest_method_return_stub: None,
            guest_method_return_pcs: Vec::new(),
            nib_awake_dispatched: false,
            pending_application_did_become_active: false,
            display_link_object: None,
            display_link_target: None,
            display_link_selector: None,
            display_link_scheduled: false,
            display_link_frame_interval: 1,
            display_link_return_stub: None,
            display_link_callbacks: 0,
            display_link_callback_returned: false,
            display_link_return_pc: None,
            guest_yield_requested: false,
            arm64_current_context: None,
            arm64_gl_present_requested: false,
            arm64_gl: None,
            arm64_application_bootstrap_dispatched: false,
            next_pthread_id: 0x2000_0000,
            next_pthread_key: 1,
            pthread_handles: HashMap::new(),
            pthread_finished: HashSet::new(),
            pthread_once_controls: HashSet::new(),
            pthread_key_values: HashMap::new(),
            signal_handlers: HashMap::new(),
            render_diagnostics: A64RenderDiagnostics::default(),
        }
    }

    pub fn take_present_request(&mut self) -> bool {
        std::mem::take(&mut self.present_requested)
    }

    pub fn take_boot_screen_request(&mut self) -> bool {
        std::mem::take(&mut self.boot_screen_notification_pending)
    }

    pub fn application_main_is_active(&self) -> bool {
        self.application_main_active
    }

    pub fn take_application_bootstrap_request(&mut self) -> bool {
        std::mem::take(&mut self.application_bootstrap_requested)
    }

    pub fn mark_boot_screen_reached(&mut self) {
        if !self.boot_screen_reached {
            self.boot_screen_reached = true;
            self.boot_screen_notification_pending = true;
        }
    }

    pub fn mark_unimplemented_reached(&mut self, symbol: &str) -> bool {
        self.reached_unimplemented_symbols.insert(symbol.to_owned())
    }

    pub fn take_guest_transfer(&mut self) -> Option<u64> {
        self.guest_transfer_pc.take()
    }

    pub fn take_display_link_callback(&mut self) -> Option<(u64, u64, u64)> {
        if !self.display_link_scheduled || self.display_link_callback_returned {
            return None;
        }
        Some((
            self.display_link_target?,
            self.display_link_selector?,
            self.display_link_object?,
        ))
    }

    pub fn mark_display_link_callback_started(&mut self) {
        self.display_link_return_pc = None;
        self.display_link_callbacks = self.display_link_callbacks.saturating_add(1);
        self.render_diagnostics.display_link_callbacks = self.display_link_callbacks;
        self.render_diagnostics.last_callback_pc = self.guest_transfer_pc.unwrap_or(0);
        self.render_diagnostics.callback_active = true;
        self.render_diagnostics.callback_entry_lr = 0;
        self.render_diagnostics.callback_return_pc = 0;
        self.render_diagnostics.last_guest_pc = 0;
        self.display_link_callback_returned = false;
    }

    pub fn mark_display_link_callback_returned(&mut self, return_pc: u64) {
        self.display_link_return_pc = Some(return_pc);
        self.display_link_callback_returned = true;
        self.render_diagnostics.callback_active = false;
    }

    pub fn trace_render_event(&mut self, message: impl std::fmt::Display) {
        if self.render_diagnostics.trace_events >= 64 {
            return;
        }
        self.render_diagnostics.trace_events += 1;
        log_dbg!(
            "ARM64 render trace #{}: {}",
            self.render_diagnostics.trace_events,
            message
        );
    }

    pub fn mark_unresolved_call(&mut self, symbol: &str, pc: u64) {
        self.render_diagnostics.last_unresolved_symbol = Some(symbol.to_owned());
        self.render_diagnostics.last_unresolved_pc = pc;
        if self.reached_unimplemented_symbols.insert(symbol.to_owned()) {
            log!(
                "ARM64 unresolved import first call: symbol={} pc={:#x} lr={:#x}",
                symbol,
                pc,
                self.render_diagnostics.callback_entry_lr
            );
        }
    }

    pub fn display_link_is_scheduled(&self) -> bool {
        self.display_link_scheduled
    }

    pub fn take_guest_yield(&mut self) -> bool {
        std::mem::take(&mut self.guest_yield_requested)
    }

    pub fn take_guest_gl_present_request(&mut self) -> bool {
        std::mem::take(&mut self.arm64_gl_present_requested)
    }

    pub fn resolve_image_symbol(&self, candidates: &[&str]) -> Option<u64> {
        self.find_image_symbol(candidates)
            .map(|(_, address)| address)
    }

    fn find_image_symbol(&self, candidates: &[&str]) -> Option<(String, u64)> {
        for image in &self.loaded_images {
            for candidate in candidates {
                if let Some(&address) = image.exports.get(*candidate) {
                    return Some((image.name.clone(), address));
                }
            }
            for (symbol, &address) in &image.exports {
                if candidates
                    .iter()
                    .any(|candidate| symbol.ends_with(candidate))
                {
                    return Some((image.name.clone(), address));
                }
            }
        }
        None
    }

    fn transfer_to_method(&mut self, selector: &str, candidates: &[&str]) -> Result<(), String> {
        let Some((image, address)) = self.find_image_symbol(candidates) else {
            return Err(format!(
                "ARM64 could not resolve UnityFramework method for selector {selector}"
            ));
        };
        log!(
            "ARM64 guest transfer: selector {} -> {} at {:#x}",
            selector,
            image,
            address
        );
        self.guest_transfer_pc = Some(address);
        Ok(())
    }
}

fn name(symbol: &str) -> &str {
    let symbol = symbol.trim_start_matches('_');
    symbol.strip_prefix('_').unwrap_or(symbol)
}
const LIGHT_HOST_CALLS: &[&str] = &[
    "fabs",
    "fabsf",
    "sin",
    "sinf",
    "cos",
    "cosf",
    "tan",
    "tanf",
    "asin",
    "asinf",
    "acos",
    "acosf",
    "atan",
    "atanf",
    "atan2",
    "atan2f",
    "sinh",
    "sinhf",
    "cosh",
    "coshf",
    "tanh",
    "tanhf",
    "asinh",
    "asinhf",
    "acosh",
    "acoshf",
    "atanh",
    "atanhf",
    "log",
    "logf",
    "log1p",
    "log1pf",
    "log2",
    "log2f",
    "log10",
    "log10f",
    "exp",
    "expf",
    "expm1",
    "expm1f",
    "exp2",
    "exp2f",
    "pow",
    "powf",
    "sqrt",
    "sqrtf",
    "ceil",
    "ceilf",
    "floor",
    "floorf",
    "round",
    "roundf",
    "trunc",
    "truncf",
    "rint",
    "rintf",
    "nearbyint",
    "nearbyintf",
    "fmod",
    "fmodf",
    "fmax",
    "fmaxf",
    "fmin",
    "fminf",
    "hypot",
    "hypotf",
];

pub fn is_light_host_call(symbol: &str) -> bool {
    LIGHT_HOST_CALLS.contains(&name(symbol))
}

fn materialize_host_constant(mem: &mut Mem64, symbol: &str) -> Result<Option<u64>, String> {
    let normalized = name(symbol);
    if normalized == "stack_chk_guard" {
        let guard = mem.alloc_zeroed(8).map_err(str::to_owned)?;
        mem.write_u64(guard, 0x9e37_79b9_7f4a_7c15)
            .map_err(str::to_owned)?;
        return Ok(Some(guard));
    }
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
    let symbol = name(symbol);
    if is_light_host_call(symbol) {
        return true;
    }
    match symbol {
        "ARM64_nib_awake_return" | "ARM64_guest_method_return" | "ARM64_application_return" | "ARM64_application_launch_return" | "ARM64_application_active_return" => true,
        "access" | "mkdir" | "signal" => true,
        "malloc" | "calloc" | "valloc" | "posix_memalign" | "free"
        | "malloc_zone_free" | "realloc" | "malloc_zone_realloc" | "memcpy"
        | "memmove" | "memcpy_chk" | "memmove_chk" | "memset" | "bzero"
        | "memset_chk" | "strlen" | "strnlen" | "strcmp" | "strncmp" | "memcmp"
        | "strcpy" | "strncpy" | "strcat" | "strncat" | "strdup" | "strndup"
        | "objc_alloc" | "objc_allocWithZone" | "objc_release" | "objc_storeStrong" | "objc_retain"
        | "objc_retainAutoreleasedReturnValue" | "objc_retainAutoreleaseReturnValue"
        | "objc_autorelease" | "objc_autoreleaseReturnValue"
        | "objc_unsafeClaimAutoreleasedReturnValue" | "objc_retainAutorelease"
        | "objc_retainBlock" | "objc_setProperty" | "objc_setProperty_nonatomic"
        | "objc_setProperty_atomic" | "objc_setProperty_nonatomic_copy"
        | "objc_setProperty_atomic_copy" | "objc_msgSend" | "objc_msgSendSuper2"
        | "objc_msgSend_stret" | "objc_msgSendSuper2_stret" | "objc_msgSend_fpret"
        | "objc_msgSend_fp2ret" | "objc_getClass" | "objc_getRequiredClass"
        | "objc_lookUpClass" | "object_getClass" | "object_getClassName"
        | "sel_registerName" | "sel_getUid" | "NSSelectorFromString"
        | "NSSearchPathForDirectoriesInDomains" | "time" | "srand" | "rand"
        | "objc_autoreleasePoolPush" | "objc_autoreleasePoolPop"
        | "objc_exception_throw" | "objc_begin_catch" | "objc_end_catch"
        | "cxa_guard_acquire" | "cxa_guard_release" | "cxa_guard_abort"
        | "cxa_pure_virtual" | "stack_chk_fail" | "stack_chk_fail_local"
        | "memchr" | "strchr" | "strrchr" | "strstr"
        | "strcasecmp" | "strncasecmp" | "bcopy" | "bcmp"
        | "memset_pattern4" | "memset_pattern8" | "memset_pattern16"
        | "cxa_atexit" | "atexit" | "pthread_mutex_lock"
        | "pthread_mutex_unlock" | "pthread_mutex_init" | "pthread_mutex_destroy" | "pthread_mutex_trylock"
        | "pthread_once" | "pthread_key_create" | "pthread_key_delete" | "pthread_getspecific" | "pthread_setspecific"
        | "pthread_condattr_init" | "pthread_condattr_destroy" | "pthread_condattr_setpshared" | "pthread_condattr_getpshared" | "pthread_condattr_setclock" | "pthread_condattr_getclock" | "pthread_mutexattr_init" | "pthread_mutexattr_destroy" | "pthread_mutexattr_setpshared" | "pthread_mutexattr_settype" | "pthread_mutexattr_gettype" | "pthread_mutexattr_setprotocol" | "pthread_mutexattr_getprotocol" | "pthread_mutexattr_setprioceiling" | "pthread_mutexattr_getprioceiling"
        | "pthread_cond_init" | "pthread_cond_destroy" | "pthread_cond_wait" | "pthread_cond_signal" | "pthread_cond_broadcast"
        | "pthread_create" | "pthread_join" | "pthread_detach" | "pthread_equal" | "pthread_setname_np" | "pthread_self" | "pthread_exit" | "gettimeofday" | "sched_yield" | "abort" | "exit" | "_exit" | "ZNSt3__16thread20hardware_concurrencyEv"
        | "_Znwm" | "_Znam" | "_ZdlPv" | "_ZdaPv"
        | "__Znwm" | "__Znam" | "__ZdlPv" | "__ZdaPv"
        | "Znwm" | "Znam" | "ZdlPv" | "ZdaPv" | "ZnwmRKSt9nothrow_t"
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
        |         "ZNSt3__112__next_primeEm" | "ZNSt3__16thread20hardware_concurrencyEv" | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE5eraseEmm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6__initEPKcmm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6__initEmc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6appendEPKc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6appendEPKcm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6assignEPKc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6assignEPKcm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6insertEmPKc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6insertEmPKcm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE6resizeEmc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE7replaceEmmPKcm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE7reserveEm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE9__grow_byEmmmmmm"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEE9push_backEc"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEEC1ERKS5_"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEEC1ERKS5_mmRKS4_"
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEEaSERKS5_"
        | "ZNSt3__113basic_istreamIcNS_11char_traitsIcEEE6sentryC1ERS3_b"
        | "ZNSt3__113basic_ostreamIcNS_11char_traitsIcEEE5writeEPKcl"
        | "ZNSt3__113basic_ostreamIcNS_11char_traitsIcEEE6sentryC1ERS3_"
        | "ZNSt3__113basic_ostreamIcNS_11char_traitsIcEEED2Ev"
        | "ZNSt3__113basic_ostreamIcNS_11char_traitsIcEEElsEPKv"
        | "ZNSt3__113basic_ostreamIcNS_11char_traitsIcEEElsEb"
        | "ZNSt3__113basic_ostreamIcNS_11char_traitsIcEEElsEd"
        | "ZNSt3__113basic_ostreamIcNS_11char_traitsIcEEElsEf"
        | "ZNSt3__113basic_ostreamIcNS_11char_traitsIcEEElsEi"
        | "ZNSt3__113basic_ostreamIcNS_11char_traitsIcEEElsEm"
        | "ZNSt3__113basic_ostreamIcNS_11char_traitsIcEEElsEs"
        | "ZNSt3__113basic_ostreamIcNS_11char_traitsIcEEElsEx"
        | "ZNSt3__114basic_iostreamIcNS_11char_traitsIcEEED2Ev"
        | "ZNSt3__115__thread_structC1Ev"
        | "ZNSt3__115__thread_structD1Ev"
        | "ZNSt3__115basic_streambufIcNS_11char_traitsIcEEEC2Ev"
        | "ZNSt3__115basic_streambufIcNS_11char_traitsIcEEED2Ev"
        | "ZNSt3__118condition_variable10notify_allEv"
        | "ZNSt3__118condition_variable10notify_oneEv"
        | "ZNSt3__118condition_variable15__do_timed_waitERNS_11unique_lockINS_5mutexEEENS_6chrono10time_pointINS5_12system_clockENS5_8durationIxNS_5ratioILl1ELl1000000000EEEEEEE"
        | "ZNSt3__111this_thread9sleep_forERKNS_6chrono8durationIxNS_5ratioILl1ELl1000000000EEEEE"
        | "ZSt9terminatev" | "Unwind_Resume" | "__Unwind_Resume"
        => true,
        value if value.starts_with("ZNSt3__") || value.starts_with("ZNKSt3__") => true,
        _ => false,
    }
}

fn return_value(context: &mut touchHLE_DynarmicA64Context, value: u64) {
    A64Abi::set_return(context, value);
}
fn arm64_double_arg(context: &touchHLE_DynarmicA64Context, index: usize) -> f64 {
    f64::from_bits(context.vectors[index][0])
}

fn arm64_float_arg(context: &touchHLE_DynarmicA64Context, index: usize) -> f32 {
    f32::from_bits(context.vectors[index][0] as u32)
}

fn dispatch_arm64_math(context: &mut touchHLE_DynarmicA64Context, symbol: &str) -> bool {
    debug_assert!(is_light_host_call(symbol));
    let result = match symbol {
        "fabs" => Some(arm64_double_arg(context, 0).abs()),
        "sin" => Some(arm64_double_arg(context, 0).sin()),
        "cos" => Some(arm64_double_arg(context, 0).cos()),
        "tan" => Some(arm64_double_arg(context, 0).tan()),
        "asin" => Some(arm64_double_arg(context, 0).asin()),
        "acos" => Some(arm64_double_arg(context, 0).acos()),
        "atan" => Some(arm64_double_arg(context, 0).atan()),
        "atan2" => Some(arm64_double_arg(context, 0).atan2(arm64_double_arg(context, 1))),
        "sinh" => Some(arm64_double_arg(context, 0).sinh()),
        "cosh" => Some(arm64_double_arg(context, 0).cosh()),
        "tanh" => Some(arm64_double_arg(context, 0).tanh()),
        "asinh" => Some(arm64_double_arg(context, 0).asinh()),
        "acosh" => Some(arm64_double_arg(context, 0).acosh()),
        "atanh" => Some(arm64_double_arg(context, 0).atanh()),
        "log" => Some(arm64_double_arg(context, 0).ln()),
        "log1p" => Some(arm64_double_arg(context, 0).ln_1p()),
        "log2" => Some(arm64_double_arg(context, 0).log2()),
        "log10" => Some(arm64_double_arg(context, 0).log10()),
        "exp" => Some(arm64_double_arg(context, 0).exp()),
        "expm1" => Some(arm64_double_arg(context, 0).exp_m1()),
        "exp2" => Some(arm64_double_arg(context, 0).exp2()),
        "pow" => Some(arm64_double_arg(context, 0).powf(arm64_double_arg(context, 1))),
        "sqrt" => Some(arm64_double_arg(context, 0).sqrt()),
        "ceil" => Some(arm64_double_arg(context, 0).ceil()),
        "floor" => Some(arm64_double_arg(context, 0).floor()),
        "round" => Some(arm64_double_arg(context, 0).round()),
        "trunc" => Some(arm64_double_arg(context, 0).trunc()),
        "rint" | "nearbyint" => Some(arm64_double_arg(context, 0).round_ties_even()),
        "fmod" => Some(arm64_double_arg(context, 0) % arm64_double_arg(context, 1)),
        "fmax" => Some(arm64_double_arg(context, 0).max(arm64_double_arg(context, 1))),
        "fmin" => Some(arm64_double_arg(context, 0).min(arm64_double_arg(context, 1))),
        "hypot" => Some(arm64_double_arg(context, 0).hypot(arm64_double_arg(context, 1))),
        "sinf" => Some(arm64_float_arg(context, 0).sin() as f64),
        "cosf" => Some(arm64_float_arg(context, 0).cos() as f64),
        "tanf" => Some(arm64_float_arg(context, 0).tan() as f64),
        "asinf" => Some(arm64_float_arg(context, 0).asin() as f64),
        "acosf" => Some(arm64_float_arg(context, 0).acos() as f64),
        "atanf" => Some(arm64_float_arg(context, 0).atan() as f64),
        "atan2f" => Some(arm64_float_arg(context, 0).atan2(arm64_float_arg(context, 1)) as f64),
        "sinhf" => Some(arm64_float_arg(context, 0).sinh() as f64),
        "coshf" => Some(arm64_float_arg(context, 0).cosh() as f64),
        "tanhf" => Some(arm64_float_arg(context, 0).tanh() as f64),
        "asinhf" => Some(arm64_float_arg(context, 0).asinh() as f64),
        "acoshf" => Some(arm64_float_arg(context, 0).acosh() as f64),
        "atanhf" => Some(arm64_float_arg(context, 0).atanh() as f64),
        "logf" => Some(arm64_float_arg(context, 0).ln() as f64),
        "log1pf" => Some(arm64_float_arg(context, 0).ln_1p() as f64),
        "log2f" => Some(arm64_float_arg(context, 0).log2() as f64),
        "log10f" => Some(arm64_float_arg(context, 0).log10() as f64),
        "expf" => Some(arm64_float_arg(context, 0).exp() as f64),
        "expm1f" => Some(arm64_float_arg(context, 0).exp_m1() as f64),
        "exp2f" => Some(arm64_float_arg(context, 0).exp2() as f64),
        "powf" => Some(arm64_float_arg(context, 0).powf(arm64_float_arg(context, 1)) as f64),
        "sqrtf" => Some(arm64_float_arg(context, 0).sqrt() as f64),
        "ceilf" => Some(arm64_float_arg(context, 0).ceil() as f64),
        "floorf" => Some(arm64_float_arg(context, 0).floor() as f64),
        "roundf" => Some(arm64_float_arg(context, 0).round() as f64),
        "truncf" => Some(arm64_float_arg(context, 0).trunc() as f64),
        "rintf" | "nearbyintf" => Some(arm64_float_arg(context, 0).round_ties_even() as f64),
        "fmodf" => Some((arm64_float_arg(context, 0) % arm64_float_arg(context, 1)) as f64),
        "fmaxf" => Some(arm64_float_arg(context, 0).max(arm64_float_arg(context, 1)) as f64),
        "fminf" => Some(arm64_float_arg(context, 0).min(arm64_float_arg(context, 1)) as f64),
        "hypotf" => Some(arm64_float_arg(context, 0).hypot(arm64_float_arg(context, 1)) as f64),
        _ => None,
    };
    let Some(result) = result else {
        return false;
    };
    if symbol.ends_with('f') {
        context.vectors[0][0] = (result as f32).to_bits() as u64;
        context.vectors[0][1] = 0;
    } else {
        context.vectors[0][0] = result.to_bits();
    }
    true
}

fn c_string(mem: &Mem64, address: u64) -> Option<Vec<u8>> {
    let length = mem.cstr_len(address, MAX_CSTRING).ok()?;
    mem.read_bytes(address, length).ok()
}

fn c_string_eq(mem: &Mem64, address: u64, value: &[u8]) -> bool {
    c_string(mem, address).as_deref() == Some(value)
}

fn arm64_home_directory(bundle_path: &str) -> String {
    bundle_path.rsplit_once('/').map_or_else(
        || "/var/mobile/Applications/00000000-0000-0000-0000-000000000000".to_owned(),
        |(parent, _)| parent.to_owned(),
    )
}

fn arm64_search_path(state: &RuntimeState, directory: u64, domain_mask: u64) -> Option<String> {
    let home = arm64_home_directory(&state.bundle_path);
    if domain_mask & 0x1 == 0 {
        return None;
    }
    match directory {
        1 | 100 => Some("/var/mobile/Applications".to_owned()),
        5 => Some(format!("{home}/Library")),
        7 => Some(home),
        9 => Some(format!("{home}/Documents")),
        13 => Some(format!("{home}/Library/Caches")),
        14 => Some(format!("{home}/Library/Application Support")),
        22 => Some(format!("{home}/Library/PreferencePanes")),
        101 => Some(format!("{home}/Library")),
        _ => None,
    }
}
fn cxx_string_bytes(mem: &Mem64, object: u64) -> Option<Vec<u8>> {
    let first = mem.read_u64(object).ok()?;
    if first & 1 == 0 {
        let length = ((first & 0xff) / 2) as u64;
        mem.read_bytes(object + 1, length).ok()
    } else {
        let length = mem.read_u64(object + 8).ok()?;
        let capacity = mem.read_u64(object + 16).ok()?;
        if length > capacity || length > MAX_CSTRING {
            return None;
        }
        mem.read_bytes(first & !1, length).ok()
    }
}

fn normalize_arm64_allocation_size(requested: u64) -> Option<u64> {
    const MAX_ALLOCATION: u64 = 512 * 1024 * 1024;
    if requested <= MAX_ALLOCATION {
        return Some(requested.max(1));
    }
    let signed = requested as i64;
    if signed < 0 {
        let corrected = signed.unsigned_abs();
        if corrected <= MAX_ALLOCATION {
            log_once_fmt!(
                "ARM64 allocator: corrected signed negative size {requested:#x} to {corrected:#x} [repeated corrections suppressed]"
            );
            return Some(corrected.max(1));
        }
    }
    log_once_fmt!(
        "ARM64 allocator: rejected invalid size {requested:#x} [repeated invalid sizes suppressed]"
    );
    None
}

fn cxx_find(text: &[u8], needle: &[u8], position: u64, reverse: bool) -> u64 {
    let start = usize::try_from(position)
        .unwrap_or(usize::MAX)
        .min(text.len());
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
    const MAX_REASONABLE: u64 = 1 << 30;

    fn is_prime(value: u64) -> bool {
        if value < 2 {
            return false;
        }
        if value == 2 {
            return true;
        }
        if value & 1 == 0 {
            return false;
        }
        let mut divisor = 3;
        while divisor <= value / divisor {
            if value % divisor == 0 {
                return false;
            }
            divisor += 2;
        }
        true
    }

    if value <= 2 {
        return 2;
    }
    if value > MAX_REASONABLE {
        log_once_fmt!(
            "ARM64 libc++ __next_prime received an invalid oversized value {value:#x}; returning 2 [repeated invalid values suppressed]"
        );
        return 2;
    }
    let mut candidate = if value & 1 == 0 { value + 1 } else { value };
    while !is_prime(candidate) {
        candidate = candidate.saturating_add(2);
    }
    candidate
}

fn arm64_prng(state: u32) -> u32 {
    let mut state = state.max(1);
    state ^= state << 13;
    state ^= state >> 17;
    state ^= state << 5;
    state
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

fn objc_is_collection_kind(kind: u64) -> bool {
    matches!(
        kind,
        A64_KIND_ARRAY | A64_KIND_MUTABLE_ARRAY | A64_KIND_DICTIONARY | A64_KIND_MUTABLE_DICTIONARY
    )
}

fn objc_is_string_kind(kind: u64) -> bool {
    matches!(kind, A64_KIND_STRING | A64_KIND_MUTABLE_STRING)
}

fn objc_is_array_kind(kind: u64) -> bool {
    matches!(kind, A64_KIND_ARRAY | A64_KIND_MUTABLE_ARRAY)
}

fn objc_data_from_guest(mem: &Mem64, pointer: u64, length: u64) -> Result<Vec<u8>, String> {
    if pointer == 0 || length == 0 {
        return Ok(Vec::new());
    }
    mem.read_bytes(pointer, length).map_err(str::to_owned)
}

fn objc_data(mem: &mut Mem64, bytes: &[u8]) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_DATA)?;
    let pointer = mem
        .alloc_zeroed(bytes.len().max(1) as u64)
        .map_err(str::to_owned)?;
    if !bytes.is_empty() {
        mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
    }
    set_objc_field(mem, object, 56, pointer);
    set_objc_field(mem, object, 64, bytes.len() as u64);
    Ok(object)
}

fn objc_dictionary(mem: &mut Mem64, key: u64, value: u64) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_DICTIONARY)?;
    let keys = mem.alloc_zeroed(8).map_err(str::to_owned)?;
    let values = mem.alloc_zeroed(8).map_err(str::to_owned)?;
    mem.write_u64(keys, key).map_err(str::to_owned)?;
    mem.write_u64(values, value).map_err(str::to_owned)?;
    set_objc_field(mem, object, 56, 1);
    set_objc_field(mem, object, 64, keys);
    set_objc_field(mem, object, 72, values);
    Ok(object)
}

fn objc_dictionary_value(mem: &Mem64, dictionary: u64, key: u64) -> u64 {
    let count = objc_field(mem, dictionary, 56);
    let keys = objc_field(mem, dictionary, 64);
    let values = objc_field(mem, dictionary, 72);
    for index in 0..count.min(4096) {
        if mem
            .read_u64(keys.saturating_add(index.saturating_mul(8)))
            .ok()
            == Some(key)
        {
            return mem
                .read_u64(values.saturating_add(index.saturating_mul(8)))
                .unwrap_or(0);
        }
    }
    0
}

fn objc_dictionary_set(
    mem: &mut Mem64,
    dictionary: u64,
    key: u64,
    value: u64,
) -> Result<(), String> {
    let count = objc_field(mem, dictionary, 56);
    let keys = objc_field(mem, dictionary, 64);
    let values = objc_field(mem, dictionary, 72);
    for index in 0..count.min(4096) {
        let key_address = keys
            .checked_add(index.saturating_mul(8))
            .ok_or("ARM64 dictionary key address overflows")?;
        if mem.read_u64(key_address).ok() == Some(key) {
            mem.write_u64(
                values
                    .checked_add(index.saturating_mul(8))
                    .ok_or("ARM64 dictionary value address overflows")?,
                value,
            )
            .map_err(str::to_owned)?;
            return Ok(());
        }
    }
    let new_keys = mem
        .alloc_zeroed((count + 1).saturating_mul(8))
        .map_err(str::to_owned)?;
    let new_values = mem
        .alloc_zeroed((count + 1).saturating_mul(8))
        .map_err(str::to_owned)?;
    if count > 0 {
        let old_keys = mem
            .read_bytes(keys, count.saturating_mul(8))
            .map_err(str::to_owned)?;
        let old_values = mem
            .read_bytes(values, count.saturating_mul(8))
            .map_err(str::to_owned)?;
        mem.write_bytes(new_keys, &old_keys)
            .map_err(str::to_owned)?;
        mem.write_bytes(new_values, &old_values)
            .map_err(str::to_owned)?;
    }
    mem.write_u64(new_keys + count * 8, key)
        .map_err(str::to_owned)?;
    mem.write_u64(new_values + count * 8, value)
        .map_err(str::to_owned)?;
    set_objc_field(mem, dictionary, 56, count + 1);
    set_objc_field(mem, dictionary, 64, new_keys);
    set_objc_field(mem, dictionary, 72, new_values);
    Ok(())
}

fn objc_object_with_size(mem: &mut Mem64, kind: u64, size: u64) -> Result<u64, String> {
    let address = mem
        .alloc_zeroed(size.max(A64_OBJECT_SIZE))
        .map_err(str::to_owned)?;
    mem.write_u64(address, kind).map_err(str::to_owned)?;
    Ok(address)
}

fn objc_array(mem: &mut Mem64, objects: &[u64]) -> Result<u64, String> {
    objc_array_with_kind(mem, A64_KIND_ARRAY, objects)
}

fn objc_array_with_kind(mem: &mut Mem64, kind: u64, objects: &[u64]) -> Result<u64, String> {
    let object = objc_object(mem, kind)?;
    let elements = mem
        .alloc_zeroed((objects.len() as u64).saturating_mul(8))
        .map_err(str::to_owned)?;
    for (index, value) in objects.iter().copied().enumerate() {
        mem.write_u64(elements + index as u64 * 8, value)
            .map_err(str::to_owned)?;
    }
    set_objc_field(mem, object, 56, objects.len() as u64);
    set_objc_field(mem, object, 64, elements);
    Ok(object)
}

fn handle_missing_objc_selector(
    receiver: u64,
    class_name: &str,
    selector: &str,
    class_method: bool,
    state: &mut RuntimeState,
) -> u64 {
    let key = format!("{class_name}::{selector}");
    if state.render_diagnostics.missing_selectors.insert(key) {
        log!(
            "ARM64 Objective-C stub: {} {} -> {}",
            class_name,
            selector,
            if class_method || receiver == 0 {
                "nil"
            } else {
                "self"
            }
        );
    }
    if class_method || receiver == 0 {
        0
    } else {
        receiver
    }
}

fn objc_string_with_format(mem: &mut Mem64, format: u64) -> Result<u64, String> {
    let value = objc_text(mem, format)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    objc_string(mem, &value)
}

fn objc_substring(
    mem: &mut Mem64,
    receiver: u64,
    start: u64,
    end: Option<u64>,
) -> Result<u64, String> {
    let bytes = objc_text(mem, receiver).unwrap_or_default();
    let start = (start as usize).min(bytes.len());
    let end = end.map_or(bytes.len(), |end| (end as usize).min(bytes.len()));
    let end = end.max(start);
    objc_string(mem, &String::from_utf8_lossy(&bytes[start..end]))
}

fn objc_kind(mem: &Mem64, address: u64) -> Option<u64> {
    if address == 0 || mem.allocation_size(address).is_none() {
        return None;
    }
    let first_word = mem.read_u64(address).ok()?;
    if (1..=A64_KIND_MUTABLE_DICTIONARY).contains(&first_word) {
        return Some(first_word);
    }
    let class = if first_word != 0 && mem.allocation_size(first_word).is_some() {
        first_word
    } else {
        objc_field(mem, address, 48)
    };
    let class_name = objc_field(mem, class, 56);
    Some(objc_class_kind(mem, class_name))
}

fn objc_field(mem: &Mem64, object: u64, offset: u64) -> u64 {
    if object == 0 || mem.allocation_size(object).is_none() {
        return 0;
    }
    mem.read_u64(object.saturating_add(offset)).unwrap_or(0)
}

fn set_objc_field(mem: &mut Mem64, object: u64, offset: u64, value: u64) {
    if object != 0 && mem.allocation_size(object).is_some() {
        let _ = mem.write_u64(object.saturating_add(offset), value);
    }
}

fn guest_ivar_offset(
    mem: &Mem64,
    state: &RuntimeState,
    class_name: &str,
    ivar_name: &str,
) -> Option<u64> {
    let mut current = Some(class_name.to_owned());
    while let Some(name) = current {
        let class_info = state.objc_classes.iter().find(|class| class.name == name)?;
        if let Some(ivar) = class_info.ivars.iter().find(|ivar| ivar.name == ivar_name) {
            let offset = mem.read_u32(ivar.offset_address).ok()? as u64;
            if (8..0x100000).contains(&offset) {
                return Some(offset);
            }
        }
        current = class_info.superclass.clone();
    }
    None
}

fn set_guest_ivar_u64(
    mem: &mut Mem64,
    state: &RuntimeState,
    class_name: &str,
    object: u64,
    ivar_name: &str,
    value: u64,
) -> Result<bool, String> {
    let Some(offset) = guest_ivar_offset(mem, state, class_name, ivar_name) else {
        return Ok(false);
    };
    mem.write_u64(
        object
            .checked_add(offset)
            .ok_or("ARM64 ivar address overflows")?,
        value,
    )
    .map_err(str::to_owned)?;
    Ok(true)
}

fn set_guest_ivar_u32(
    mem: &mut Mem64,
    state: &RuntimeState,
    class_name: &str,
    object: u64,
    ivar_name: &str,
    value: u32,
) -> Result<bool, String> {
    let Some(offset) = guest_ivar_offset(mem, state, class_name, ivar_name) else {
        return Ok(false);
    };
    mem.write_u32(
        object
            .checked_add(offset)
            .ok_or("ARM64 ivar address overflows")?,
        value,
    )
    .map_err(str::to_owned)?;
    Ok(true)
}

fn set_guest_ivar_f64(
    mem: &mut Mem64,
    state: &RuntimeState,
    class_name: &str,
    object: u64,
    ivar_name: &str,
    value: f64,
) -> Result<bool, String> {
    set_guest_ivar_u64(mem, state, class_name, object, ivar_name, value.to_bits())
}
fn set_guest_ivar_u64_aliases(
    mem: &mut Mem64,
    state: &RuntimeState,
    class_name: &str,
    object: u64,
    ivar_names: &[&str],
    value: u64,
) -> Result<bool, String> {
    let mut mapped = false;
    for ivar_name in ivar_names {
        mapped |= set_guest_ivar_u64(mem, state, class_name, object, ivar_name, value)?;
    }
    Ok(mapped)
}

fn initialize_eagl_view(
    mem: &mut Mem64,
    state: &mut RuntimeState,
    view: u64,
) -> Result<(), String> {
    let context = if let Some(context) = state.graphics_context {
        context
    } else {
        let context = objc_object(mem, A64_KIND_CONTEXT)?;
        state.graphics_context = Some(context);
        context
    };
    let class_name =
        receiver_class_name(mem, view, A64_KIND_EAGL_VIEW).unwrap_or_else(|| "EAGLView".to_owned());
    let context_mapped = set_guest_ivar_u64(mem, state, &class_name, view, "context", context)?;
    let framebuffer_mapped =
        set_guest_ivar_u32(mem, state, &class_name, view, "defaultFramebuffer", 1)?;
    let renderbuffer_mapped =
        set_guest_ivar_u32(mem, state, &class_name, view, "colorRenderbuffer", 1)?;
    let _width_mapped = set_guest_ivar_u32(
        mem,
        state,
        &class_name,
        view,
        "framebufferWidth",
        state.screen_width,
    )?;
    let _height_mapped = set_guest_ivar_u32(
        mem,
        state,
        &class_name,
        view,
        "framebufferHeight",
        state.screen_height,
    )?;
    let depth_mapped = set_guest_ivar_u32(mem, state, &class_name, view, "_depthRenderBuffer", 0)?;
    let scale_mapped = set_guest_ivar_f64(
        mem,
        state,
        &class_name,
        view,
        "viewScale",
        state.device_family.scale_factor() as f64,
    )?;
    state.application_eagl_view = Some(view);
    let view_ivar_values = [
        "context",
        "defaultFramebuffer",
        "colorRenderbuffer",
        "framebufferWidth",
        "framebufferHeight",
        "_depthRenderBuffer",
        "viewScale",
    ]
    .iter()
    .filter_map(|name| {
        guest_ivar_offset(mem, state, &class_name, name).map(|offset| {
            format!(
                "{}@{:#x}={:#x}",
                name,
                offset,
                mem.read_u64(view + offset).unwrap_or(0)
            )
        })
    })
    .collect::<Vec<_>>()
    .join(",");
    log!(
        "ARM64 initialized EAGLView framebuffer: view={:#x} class={} context={:#x} mapped=context:{} framebuffer:{} renderbuffer:{} size={}x{} depth:{} scale:{} ivars=[{}]",
        view,
        class_name,
        context,
        context_mapped,
        framebuffer_mapped,
        renderbuffer_mapped,
        state.screen_width,
        state.screen_height,
        depth_mapped,
        scale_mapped,
        view_ivar_values,
    );
    Ok(())
}

fn objc_string_append(mem: &mut Mem64, left: u64, right: u64) -> Result<u64, String> {
    let mut bytes = objc_text(mem, left).unwrap_or_default();
    bytes.extend(objc_text(mem, right).unwrap_or_default());
    let value = String::from_utf8(bytes).map_err(|_| "ARM64 Objective-C string is not UTF-8")?;
    objc_string(mem, &value)
}

fn objc_string(mem: &mut Mem64, value: &str) -> Result<u64, String> {
    objc_string_with_kind(mem, value, A64_KIND_STRING)
}

fn objc_string_with_kind(mem: &mut Mem64, value: &str, kind: u64) -> Result<u64, String> {
    let object = objc_object(mem, kind)?;
    let bytes = value.as_bytes();
    let pointer = mem
        .alloc_zeroed(bytes.len() as u64 + 1)
        .map_err(str::to_owned)?;
    mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
    mem.write_u8(pointer + bytes.len() as u64, 0)
        .map_err(str::to_owned)?;
    set_objc_field(mem, object, 56, pointer);
    set_objc_field(mem, object, 64, bytes.len() as u64);
    Ok(object)
}

fn initialize_arm64_string(
    mem: &mut Mem64,
    receiver: u64,
    utf8_string: u64,
) -> Result<u64, String> {
    let bytes = c_string(mem, utf8_string).unwrap_or_default();
    let pointer = mem
        .alloc_zeroed(bytes.len() as u64 + 1)
        .map_err(str::to_owned)?;
    mem.write_bytes(pointer, &bytes).map_err(str::to_owned)?;
    mem.write_u8(pointer + bytes.len() as u64, 0)
        .map_err(str::to_owned)?;
    set_objc_field(mem, receiver, 56, pointer);
    set_objc_field(mem, receiver, 64, bytes.len() as u64);
    Ok(receiver)
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

fn objc_ui_screen(
    mem: &mut Mem64,
    family: DeviceFamily,
    orientation: DeviceOrientation,
) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_UI_SCREEN)?;
    let (portrait_width, portrait_height) = family.portrait_size();
    let (width, height) = match orientation {
        DeviceOrientation::Portrait | DeviceOrientation::PortraitUpsideDown => {
            (portrait_width, portrait_height)
        }
        DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight => {
            (portrait_height, portrait_width)
        }
    };
    set_objc_field(mem, object, 56, width as u64);
    set_objc_field(mem, object, 64, height as u64);
    set_objc_field(mem, object, 72, family.scale_factor().to_bits() as u64);
    Ok(object)
}

fn objc_number(mem: &mut Mem64, value: i64) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_NUMBER)?;
    set_objc_field(mem, object, 56, value as u64);
    set_objc_field(mem, object, 64, 0);
    Ok(object)
}

fn objc_number_float(mem: &mut Mem64, value: f64) -> Result<u64, String> {
    let object = objc_object(mem, A64_KIND_NUMBER)?;
    set_objc_field(mem, object, 56, value.to_bits());
    set_objc_field(mem, object, 64, 1);
    Ok(object)
}

fn objc_number_value(mem: &Mem64, object: u64) -> f64 {
    let value = objc_field(mem, object, 56);
    if objc_field(mem, object, 64) != 0 {
        f64::from_bits(value)
    } else {
        value as i64 as f64
    }
}

fn set_float_return(context: &mut touchHLE_DynarmicA64Context, value: f64) {
    context.vectors[0][0] = value.to_bits();
    context.regs[0] = value.to_bits();
}

fn write_screen_rect(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
    family: DeviceFamily,
    orientation: DeviceOrientation,
    scale: f64,
) -> Result<(), String> {
    let result = context.regs[8];
    if result == 0 || mem.allocation_size(result).is_none() {
        return Ok(());
    }
    let (portrait_width, portrait_height) = family.portrait_size();
    let (width, height) = match orientation {
        DeviceOrientation::Portrait | DeviceOrientation::PortraitUpsideDown => {
            (portrait_width, portrait_height)
        }
        DeviceOrientation::LandscapeLeft | DeviceOrientation::LandscapeRight => {
            (portrait_height, portrait_width)
        }
    };
    for (offset, value) in [
        (0, 0.0),
        (8, 0.0),
        (16, width as f64 / scale),
        (24, height as f64 / scale),
    ] {
        mem.write_u64(result + offset, value.to_bits())
            .map_err(str::to_owned)?;
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
        Some(b"NSString") => A64_KIND_STRING,
        Some(b"NSMutableString") => A64_KIND_MUTABLE_STRING,
        Some(b"NSData") | Some(b"NSMutableData") => A64_KIND_DATA,
        Some(b"UIApplication") => A64_KIND_APPLICATION,
        Some(b"CADisplayLink") => A64_KIND_DISPLAY_LINK,
        Some(b"NSRunLoop") | Some(b"CFRunLoop") => A64_KIND_RUN_LOOP,
        Some(b"NSThread") => A64_KIND_THREAD,
        Some(b"EAGLView") => A64_KIND_EAGL_VIEW,
        Some(b"EAGLContext") => A64_KIND_CONTEXT,
        Some(b"UIView") | Some(b"UIWindow") => A64_KIND_VIEW,
        Some(b"NSArray") => A64_KIND_ARRAY,
        Some(b"NSMutableArray") => A64_KIND_MUTABLE_ARRAY,
        Some(b"NSDictionary") => A64_KIND_DICTIONARY,
        Some(b"NSMutableDictionary") => A64_KIND_MUTABLE_DICTIONARY,
        Some(b"NSNumber") => A64_KIND_NUMBER,
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
    let selector_bytes = c_string(mem, context.regs[1]).unwrap_or_default();
    let selector = String::from_utf8_lossy(&selector_bytes).into_owned();
    state.objc_messages = state.objc_messages.saturating_add(1);
    state.last_selector = Some(selector.clone());
    state.render_diagnostics.last_dispatch_receiver = receiver;
    state.render_diagnostics.last_dispatch_selector = Some(selector.clone());
    state.render_diagnostics.last_dispatch_pc = context.pc;
    state.render_diagnostics.last_dispatch_lr = context.regs[30];
    state.render_diagnostics.last_dispatch_sp = context.sp;
    state.render_diagnostics.last_dispatch_callback_target = state.display_link_target.unwrap_or(0);
    let kind = objc_kind(mem, receiver).unwrap_or(A64_KIND_GENERIC);
    let receiver_class = if kind == A64_KIND_CLASS {
        receiver
    } else if receiver != 0 && mem.allocation_size(receiver).is_some() {
        let first_word = mem.read_u64(receiver).unwrap_or(0);
        if first_word != 0 && mem.allocation_size(first_word).is_some() {
            first_word
        } else {
            objc_field(mem, receiver, 48)
        }
    } else {
        0
    };
    let class_name = objc_field(mem, receiver_class, 56);

    if selector == "runUIApplicationMainWithArgc:argv:" && receiver == 0 {
        echo!("ARM64 Objective-C bootstrap call used a nil receiver; returning zero and continuing startup");
    }

    if state.objc_messages <= 10 || state.objc_messages % 100 == 0 {
        log_dbg!(
            "ARM64 Objective-C message #{}: receiver={:#x} kind={} selector={}",
            state.objc_messages,
            receiver,
            kind,
            selector,
        );
    }
    if matches!(
        selector.as_str(),
        "displayLinkWithTarget:selector:"
            | "setFrameInterval:"
            | "addToRunLoop:forMode:"
            | "setDisplayLink:"
    ) {
        log_once_fmt!(
            "ARM64 display-link message: receiver={:#x} kind={} class_name={:#x} selector={} x2={:#x} x3={:#x} [repeated messages suppressed]",
            receiver,
            kind,
            class_name,
            selector,
            context.regs[2],
            context.regs[3],
        );
    }
    if matches!(
        selector.as_str(),
        "commit" | "waitUntilCompleted" | "presentDrawable:" | "endEncoding"
    ) {
        state.metal_commands = state.metal_commands.saturating_add(1);
    }
    if matches!(selector.as_str(), "commit" | "presentDrawable:") {
        state.frame_serial = state.frame_serial.saturating_add(1);
        state.present_requested = true;
    }
    let class_method = kind == A64_KIND_CLASS;
    let view_controller_receiver = state.application_view_controller == Some(receiver)
        || receiver_class_name(mem, receiver, kind)
            .is_some_and(|name| name.to_ascii_lowercase().contains("viewcontroller"));
    let application_accessor = (state.application_delegate == Some(receiver)
        && matches!(selector.as_str(), "window" | "viewController" | "view"))
        || (view_controller_receiver && matches!(selector.as_str(), "view" | "context"));
    if selector == "view"
        && receiver != 0
        && (view_controller_receiver || state.application_view_controller == Some(receiver))
    {
        let view = state.application_view.unwrap_or(0);
        log_once_fmt!(
            "ARM64 controller view bridge: receiver={:#x} class={} result={:#x} application_view={:#x} [repeated bridge calls suppressed]",
            receiver,
            receiver_class_name(mem, receiver, kind).as_deref().unwrap_or("<unknown>"),
            view,
            state.application_view.unwrap_or(0),
        );
        return_value(context, view);
        return Ok(());
    }
    if selector == "platform" && receiver != 0 {
        let platform = objc_field(mem, receiver, 0x158);
        if platform == 0 {
            let platform = objc_object_with_size(mem, A64_KIND_GENERIC, 32)?;
            set_objc_field(mem, platform, 16, platform + 24);
            set_objc_field(mem, receiver, 0x158, platform);
            log!(
                "ARM64 initialized empty platform list for {} at {:#x}",
                receiver_class_name(mem, receiver, A64_KIND_GENERIC)
                    .as_deref()
                    .unwrap_or("<unknown>"),
                platform
            );
        }
    }
    if !application_accessor
        && !matches!(
            selector.as_str(),
            "alloc"
                | "new"
                | "init"
                | "self"
                | "retain"
                | "autorelease"
                | "copy"
                | "mutableCopy"
                | "release"
                | "class"
                | "respondsToSelector:"
                | "isKindOfClass:"
                | "hasUnifiedMemory"
        )
        && transfer_guest_method(mem, context, state, &selector, class_method)
    {
        return Ok(());
    }
    let result = match selector.as_str() {
        "runUIApplicationMainWithArgc:argv:" if receiver == 0 => {
            state.application_main_active = true;
            state.application_main_calls = state.application_main_calls.saturating_add(1);
            state.application_bootstrap_requested = true;
            state.present_requested = true;
            0
        }
        "init" | "self" | "retain" | "autorelease" | "copy" => receiver,
        "mutableCopy" if objc_is_string_kind(kind) => {
            let bytes = objc_text(mem, receiver).unwrap_or_default();
            let value = String::from_utf8_lossy(&bytes);
            objc_string_with_kind(mem, &value, A64_KIND_MUTABLE_STRING)?
        }
        "mutableCopy" if objc_is_array_kind(kind) => {
            let count = objc_field(mem, receiver, 56).min(4096);
            let elements = objc_field(mem, receiver, 64);
            let mut objects = Vec::with_capacity(count as usize);
            for index in 0..count {
                objects.push(mem.read_u64(elements + index * 8).unwrap_or(0));
            }
            objc_array_with_kind(mem, A64_KIND_MUTABLE_ARRAY, &objects)?
        }
        "createFramebuffer" if kind == A64_KIND_EAGL_VIEW => {
            initialize_eagl_view(mem, state, receiver)?;
            0
        }
        "deleteFramebuffer" if kind == A64_KIND_EAGL_VIEW => {
            let class_name = receiver_class_name(mem, receiver, A64_KIND_GENERIC)
                .unwrap_or_else(|| "EAGLView".to_owned());
            set_guest_ivar_u32(mem, state, &class_name, receiver, "defaultFramebuffer", 0)?;
            set_guest_ivar_u32(mem, state, &class_name, receiver, "colorRenderbuffer", 0)?;
            0
        }
        "window" if state.application_delegate == Some(receiver) => {
            state.application_window.unwrap_or(0)
        }
        "viewController" if state.application_delegate == Some(receiver) => {
            state.application_view_controller.unwrap_or(0)
        }
        "view" if state.application_delegate == Some(receiver) => {
            state.application_view.unwrap_or(0)
        }
        "view" if view_controller_receiver => {
            let mut view = objc_field(mem, receiver, A64_UIVIEWCONTROLLER_VIEW_IVAR);
            if view == 0 {
                view = state.application_view.unwrap_or(0);
                if view != 0 {
                    set_objc_field(mem, receiver, A64_UIVIEWCONTROLLER_VIEW_IVAR, view);
                }
            }
            if view != 0 && state.application_view_controller.is_none() {
                state.application_view_controller = Some(receiver);
            }
            log_once_fmt!(
                "ARM64 view getter: receiver={:#x} class={} view_ivar={:#x} result={:#x} [repeated getters suppressed]",
                receiver,
                receiver_class_name(mem, receiver, kind).as_deref().unwrap_or("<unknown>"),
                A64_UIVIEWCONTROLLER_VIEW_IVAR,
                view,
            );
            view
        }
        "view" if state.application_view == Some(receiver) => {
            state.application_eagl_view.unwrap_or(0)
        }
        "context"
            if state.application_view_controller == Some(receiver)
                || state.application_eagl_view == Some(receiver) =>
        {
            let class_name = receiver_class_name(mem, receiver, A64_KIND_GENERIC)
                .unwrap_or_else(|| "EAGLView".to_owned());
            let value = guest_ivar_offset(mem, state, &class_name, "context")
                .and_then(|offset| mem.read_u64(receiver.checked_add(offset)?).ok())
                .unwrap_or(0);
            log_dbg!(
                "ARM64 context getter: receiver={:#x} class={} value={:#x}",
                receiver,
                class_name,
                value,
            );
            value
        }
        "setContext:"
            if state.application_view_controller == Some(receiver)
                || state.application_eagl_view == Some(receiver) =>
        {
            let class_name = receiver_class_name(mem, receiver, A64_KIND_GENERIC)
                .unwrap_or_else(|| "EAGLView".to_owned());
            if let Some(offset) = guest_ivar_offset(mem, state, &class_name, "context") {
                mem.write_u64(
                    receiver
                        .checked_add(offset)
                        .ok_or("ARM64 context ivar address overflows")?,
                    context.regs[2],
                )
                .map_err(str::to_owned)?;
            }
            state.graphics_context = Some(context.regs[2]);
            0
        }
        "currentContext"
            if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"EAGLContext") =>
        {
            state.arm64_current_context.unwrap_or(0)
        }
        "initWithAPI:"
            if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"EAGLContext") =>
        {
            let object = objc_object(mem, A64_KIND_CONTEXT)?;
            state.eagl_context_api = context.regs[2] as u32;
            state.graphics_context = Some(object);
            state.arm64_current_context = Some(object);
            log_dbg!(
                "ARM64 EAGLContext initialized: api={} object={:#x}",
                state.eagl_context_api,
                object
            );
            object
        }
        "setCurrentContext:"
            if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"EAGLContext") =>
        {
            state.arm64_current_context = (context.regs[2] != 0).then_some(context.regs[2]);
            1
        }
        "presentRenderbuffer:" if kind == A64_KIND_CONTEXT => {
            state.arm64_gl_present_requested = true;
            state.present_requested = true;
            state.frame_serial = state.frame_serial.saturating_add(1);
            1
        }
        "platform" if receiver != 0 => objc_string(mem, state.device_family.machine_name())?,
        "currentThread" if kind == A64_KIND_CLASS => objc_object(mem, A64_KIND_THREAD)?,
        "currentRunLoop" if kind == A64_KIND_THREAD => objc_object(mem, A64_KIND_RUN_LOOP)?,
        "displayLinkWithTarget:selector:"
            if kind == A64_KIND_CLASS || kind == A64_KIND_UI_SCREEN =>
        {
            let object = objc_object(mem, A64_KIND_DISPLAY_LINK)?;
            state.display_link_object = Some(object);
            state.display_link_target = Some(context.regs[2]);
            state.display_link_selector = Some(context.regs[3]);
            state.display_link_scheduled = false;
            state.display_link_callback_returned = true;
            echo!(
                "ARM64 display-link created: object={:#x} target={:#x} selector={:#x}",
                object,
                context.regs[2],
                context.regs[3]
            );
            object
        }
        "setFrameInterval:" if kind == A64_KIND_DISPLAY_LINK => {
            state.display_link_frame_interval = context.regs[2].max(1);
            set_objc_field(mem, receiver, 56, context.regs[2].max(1));
            0
        }
        "addToRunLoop:forMode:" if kind == A64_KIND_DISPLAY_LINK => {
            state.display_link_scheduled = true;
            state.display_link_callback_returned = true;
            echo!(
                "ARM64 display-link scheduled: object={:#x} run_loop={:#x} mode={:#x}",
                receiver,
                context.regs[2],
                context.regs[3]
            );
            0
        }
        "startAnimation" | "invalidate" if kind == A64_KIND_DISPLAY_LINK => 0,
        "setFramebuffer"
            if state.application_eagl_view == Some(receiver) || kind == A64_KIND_EAGL_VIEW =>
        {
            state.render_diagnostics.set_framebuffer_calls = state
                .render_diagnostics
                .set_framebuffer_calls
                .saturating_add(1);
            log_once_fmt!(
                "ARM64 setFramebuffer entry: receiver={:#x} class={} context={:#x} sp={:#x} fp={:#x} lr={:#x} [repeated calls suppressed]",
                receiver,
                receiver_class_name(mem, receiver, A64_KIND_GENERIC).as_deref().unwrap_or("<unknown>"),
                state.graphics_context.unwrap_or(0),
                context.sp,
                context.regs[29],
                context.regs[30],
            );
            initialize_eagl_view(mem, state, receiver)?;
            let class_name = receiver_class_name(mem, receiver, A64_KIND_GENERIC)
                .unwrap_or_else(|| "EAGLView".to_owned());
            let framebuffer = guest_ivar_offset(mem, state, &class_name, "defaultFramebuffer")
                .and_then(|offset| mem.read_u32(receiver + offset).ok())
                .unwrap_or(0);
            let renderbuffer = guest_ivar_offset(mem, state, &class_name, "colorRenderbuffer")
                .and_then(|offset| mem.read_u32(receiver + offset).ok())
                .unwrap_or(0);
            log_once_fmt!(
                "ARM64 setFramebuffer success: receiver={:#x} context={:#x} framebuffer={} renderbuffer={} size={}x{} sp={:#x} [repeated calls suppressed]",
                receiver,
                state.graphics_context.unwrap_or(0),
                framebuffer,
                renderbuffer,
                state.screen_width,
                state.screen_height,
                context.sp,
            );
            0
        }
        "presentFramebuffer" if kind == A64_KIND_EAGL_VIEW => {
            state.present_requested = true;
            state.frame_serial = state.frame_serial.saturating_add(1);
            state.render_diagnostics.present_framebuffer_calls = state
                .render_diagnostics
                .present_framebuffer_calls
                .saturating_add(1);
            0
        }
        "setDisplayLink:" if state.application_delegate == Some(receiver) => {
            set_objc_field(mem, receiver, 80, context.regs[2]);
            0
        }
        "release" => 0,
        "class" => receiver_class,
        "respondsToSelector:" | "isKindOfClass:" | "hasUnifiedMemory" => 1,
        "count" if objc_is_collection_kind(kind) || kind == A64_KIND_SET => {
            objc_field(mem, receiver, 56)
        }
        "addObject:" if objc_is_array_kind(kind) && kind == A64_KIND_MUTABLE_ARRAY => {
            let count = objc_field(mem, receiver, 56);
            let elements = objc_field(mem, receiver, 64);
            let new_elements = mem
                .alloc_zeroed((count + 1).saturating_mul(8))
                .map_err(str::to_owned)?;
            if count > 0 {
                let old = mem
                    .read_bytes(elements, count.saturating_mul(8))
                    .map_err(str::to_owned)?;
                mem.write_bytes(new_elements, &old).map_err(str::to_owned)?;
            }
            mem.write_u64(new_elements + count * 8, context.regs[2])
                .map_err(str::to_owned)?;
            set_objc_field(mem, receiver, 56, count + 1);
            set_objc_field(mem, receiver, 64, new_elements);
            0
        }
        "removeAllObjects" if kind == A64_KIND_MUTABLE_ARRAY => {
            set_objc_field(mem, receiver, 56, 0);
            set_objc_field(mem, receiver, 64, 0);
            0
        }
        "objectForKey:" if matches!(kind, A64_KIND_DICTIONARY | A64_KIND_MUTABLE_DICTIONARY) => {
            objc_dictionary_value(mem, receiver, context.regs[2])
        }
        "setObject:forKey:"
            if matches!(kind, A64_KIND_DICTIONARY | A64_KIND_MUTABLE_DICTIONARY) =>
        {
            objc_dictionary_set(mem, receiver, context.regs[3], context.regs[2])?;
            0
        }
        "allKeys" if matches!(kind, A64_KIND_DICTIONARY | A64_KIND_MUTABLE_DICTIONARY) => {
            let count = objc_field(mem, receiver, 56).min(4096);
            let keys = objc_field(mem, receiver, 64);
            let mut values = Vec::with_capacity(count as usize);
            for index in 0..count {
                values.push(mem.read_u64(keys + index * 8).unwrap_or(0));
            }
            objc_array(mem, &values)?
        }
        "firstObject" if objc_is_array_kind(kind) => {
            let result = if objc_field(mem, receiver, 56) > 0 {
                let elements = objc_field(mem, receiver, 64);
                mem.read_u64(elements).map_err(str::to_owned)?
            } else {
                0
            };
            return_value(context, result);
            return Ok(());
        }
        "lastObject" if objc_is_array_kind(kind) => {
            let count = objc_field(mem, receiver, 56);
            let result = if count > 0 {
                let elements = objc_field(mem, receiver, 64);
                mem.read_u64(elements + (count - 1) * 8)
                    .map_err(str::to_owned)?
            } else {
                0
            };
            return_value(context, result);
            return Ok(());
        }
        "objectAtIndexedSubscript:" | "objectAtIndex:" if objc_is_array_kind(kind) => {
            let index = context.regs[2];
            let count = objc_field(mem, receiver, 56);
            let result = if index < count {
                let elements = objc_field(mem, receiver, 64);
                let address = elements
                    .checked_add(index.saturating_mul(8))
                    .ok_or("ARM64 NSArray index address overflows")?;
                let result = mem.read_u64(address).map_err(str::to_owned)?;
                log_once_fmt!(
                    "ARM64 NSArray objectAtIndex trace: receiver={receiver:#x} index={index} count={count} elements={elements:#x} result={result:#x} pc={:#x} lr={:#x} [first in-process access only]",
                    context.pc,
                    context.regs[30],
                );
                result
            } else {
                log_once_fmt!(
                    "ARM64 NSArray objectAtIndex out of bounds: receiver={:#x} index={} count={} caller_pc={:#x} [repeated invalid accesses suppressed]",
                    receiver,
                    index,
                    count,
                    context.pc,
                );
                0
            };
            return_value(context, result);
            return Ok(());
        }
        "status" if kind == A64_KIND_COMMAND_BUFFER => 4,
        "error" if kind == A64_KIND_COMMAND_BUFFER => 0,
        "newFence" | "newEvent" | "newHeapWithDescriptor:" | "newArgumentEncoderWithArguments:" => {
            objc_object(mem, A64_KIND_GENERIC)?
        }
        "mainBundle" if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"NSBundle") => {
            objc_bundle(mem, state)?
        }
        "currentDevice" if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"UIDevice") => {
            objc_ui_device(mem, state.device_family)?
        }
        "mainScreen" if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"UIScreen") => {
            objc_ui_screen(mem, state.device_family, state.orientation)?
        }
        "bundleIdentifier" if kind == A64_KIND_BUNDLE => objc_field(mem, receiver, 64),
        "bundlePath" | "resourcePath" if kind == A64_KIND_BUNDLE => objc_field(mem, receiver, 56),
        "stringByAppendingString:" if objc_is_string_kind(kind) => {
            objc_string_append(mem, receiver, context.regs[2])?
        }
        "dataWithContentsOfFile:"
            if kind == A64_KIND_CLASS
                && (objc_text_eq(mem, class_name, b"NSData")
                    || objc_text_eq(mem, class_name, b"NSMutableData")) =>
        {
            let path = objc_text(mem, context.regs[2]).unwrap_or_default();
            let path = String::from_utf8_lossy(&path);
            match std::fs::read(path.as_ref()) {
                Ok(bytes) => objc_data(mem, &bytes)?,
                Err(_) => objc_data(mem, &[])?,
            }
        }
        "stringWithUTF8String:"
            if kind == A64_KIND_CLASS
                && (objc_text_eq(mem, class_name, b"NSString")
                    || objc_text_eq(mem, class_name, b"NSMutableString")) =>
        {
            let pointer = context.regs[2];
            if pointer == 0 {
                0
            } else {
                let bytes = c_string(mem, pointer).unwrap_or_default();
                let value = String::from_utf8_lossy(&bytes).into_owned();
                objc_string(mem, &value)?
            }
        }
        "initWithUTF8String:" if objc_is_string_kind(kind) => {
            initialize_arm64_string(mem, receiver, context.regs[2])?
        }
        "compare:options:" if objc_is_string_kind(kind) => {
            let left = objc_text(mem, receiver).unwrap_or_default();
            let right = objc_text(mem, context.regs[2]).unwrap_or_default();
            match left.cmp(&right) {
                std::cmp::Ordering::Less => (-1_i64) as u64,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }
        }
        "bundleWithPath:"
            if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"NSBundle") =>
        {
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
                    &format!(
                        "{}.{}.{}",
                        state.ios_version.0, state.ios_version.1, state.ios_version.2
                    ),
                )?,
                _ => 0,
            }
        }
        "pathForResource:ofType:" if kind == A64_KIND_BUNDLE => 0,
        "systemVersion" | "operatingSystemVersionString" => objc_string(
            mem,
            &format!(
                "{}.{}.{}",
                state.ios_version.0, state.ios_version.1, state.ios_version.2
            ),
        )?,
        "model" | "localizedModel" | "name" if kind == A64_KIND_UI_DEVICE => objc_string(
            mem,
            if state.device_family.is_ipad() {
                "iPad"
            } else {
                "iPhone"
            },
        )?,
        "systemName" if kind == A64_KIND_UI_DEVICE => objc_string(mem, "iPhone OS")?,
        "userInterfaceIdiom" if kind == A64_KIND_UI_DEVICE => {
            u64::from(state.device_family.is_ipad())
        }
        "bounds" | "applicationFrame" if kind == A64_KIND_UI_SCREEN => {
            write_screen_rect(mem, context, state.device_family, state.orientation, 1.0)?;
            0
        }
        "nativeBounds" if kind == A64_KIND_UI_SCREEN => {
            write_screen_rect(
                mem,
                context,
                state.device_family,
                state.orientation,
                state.device_family.scale_factor() as f64,
            )?;
            0
        }
        "scale" | "nativeScale" if kind == A64_KIND_UI_SCREEN => {
            set_float_return(context, state.device_family.scale_factor() as f64);
            0
        }
        "operatingSystemVersion" => {
            context.regs[0] =
                (state.ios_version.0 as u32 as u64) | ((state.ios_version.1 as u32 as u64) << 32);
            context.regs[1] = state.ios_version.2 as u32 as u64;
            0
        }
        "isOperatingSystemAtLeastVersion:" => {
            let requested_major = context.regs[2] as i32;
            let requested_minor = (context.regs[2] >> 32) as u32 as i32;
            let requested_patch = context.regs[3] as u32 as i32;
            u64::from(
                (
                    state.ios_version.0,
                    state.ios_version.1,
                    state.ios_version.2,
                ) >= (requested_major, requested_minor, requested_patch),
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
        "setLabel:"
        | "setCullMode:"
        | "setFrontFacingWinding:"
        | "setTriangleFillMode:"
        | "setDepthStencilState:"
        | "setViewport:"
        | "setScissorRect:"
        | "setVertexBytes:length:atIndex:"
        | "setFragmentBytes:length:atIndex:"
        | "setVertexBufferOffset:atIndex:"
        | "setFragmentBufferOffset:atIndex:" => 0,
        "drawPrimitives:vertexStart:vertexCount:"
        | "drawIndexedPrimitives:indexCount:indexType:indexBuffer:indexBufferOffset:"
        | "dispatchThreadgroups:threadsPerThreadgroup:" => {
            state.metal_commands = state.metal_commands.saturating_add(1);
            0
        }
        "name" => objc_string(mem, "RadekHLE Metal device")?,
        "UTF8String" => objc_field(mem, receiver, 56),
        "length"
            if objc_is_string_kind(kind) || kind == A64_KIND_BUFFER || kind == A64_KIND_DATA =>
        {
            objc_field(mem, receiver, 64)
        }
        "bytes" if kind == A64_KIND_DATA => objc_field(mem, receiver, 56),
        "dataWithBytes:length:"
            if kind == A64_KIND_CLASS
                && (objc_text_eq(mem, class_name, b"NSData")
                    || objc_text_eq(mem, class_name, b"NSMutableData")) =>
        {
            let bytes = objc_data_from_guest(mem, context.regs[2], context.regs[3])?;
            objc_data(mem, &bytes)?
        }
        "data" if kind == A64_KIND_DATA => objc_data(mem, &[])?,
        "containsObject:" if kind == A64_KIND_SET => 1,
        "boolValue" if objc_is_string_kind(kind) => u64::from(!objc_text_eq(mem, receiver, b"0")),
        "boolValue" if kind == A64_KIND_NUMBER => u64::from(objc_field(mem, receiver, 56) != 0),
        "intValue" | "integerValue" | "longLongValue" if kind == A64_KIND_NUMBER => {
            objc_field(mem, receiver, 56)
        }
        "unsignedIntegerValue" if kind == A64_KIND_NUMBER => objc_field(mem, receiver, 56),
        "doubleValue" | "floatValue" if kind == A64_KIND_NUMBER => {
            set_float_return(context, objc_number_value(mem, receiver));
            0
        }
        "isEqualToString:" | "isEqual:" if objc_is_string_kind(kind) => {
            u64::from(objc_text(mem, receiver) == objc_text(mem, context.regs[2]))
        }
        "rangeOfString:options:" if objc_is_string_kind(kind) => {
            let haystack = objc_text(mem, receiver).unwrap_or_default();
            let needle = objc_text(mem, context.regs[2]).unwrap_or_default();
            if let Some(pos) = haystack.windows(needle.len()).position(|w| w == needle) {
                A64Abi::set_return_pair(context, pos as u64, needle.len() as u64);
            } else {
                A64Abi::set_return_pair(context, !0, 0);
            }
            return Ok(());
        }
        "cStringUsingEncoding:" if objc_is_string_kind(kind) => objc_field(mem, receiver, 56),
        "getInstance"
            if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"UnityFramework") =>
        {
            state.transfer_to_method(
                selector.as_str(),
                &["+[UnityFramework getInstance]", "getInstance"],
            )?;
            receiver
        }
        "appController" if kind == A64_KIND_UNITY_FRAMEWORK => 0,
        "setExecuteHeader:" if kind == A64_KIND_UNITY_FRAMEWORK => 0,
        "runUIApplicationMainWithArgc:argv:" if kind == A64_KIND_UNITY_FRAMEWORK => {
            state.transfer_to_method(
                selector.as_str(),
                &[
                    "-[UnityFramework runUIApplicationMainWithArgc:argv:]",
                    "runUIApplicationMainWithArgc:argv:",
                ],
            )?;
            receiver
        }
        "newCommandQueue" | "newCommandQueueWithMaxCommandBufferCount:" => {
            objc_object(mem, A64_KIND_QUEUE)?
        }
        "commandBuffer" | "commandBufferWithUnretainedReferences" => {
            objc_object(mem, A64_KIND_COMMAND_BUFFER)?
        }
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
                set_objc_field(
                    mem,
                    object,
                    offset,
                    objc_field(mem, context.regs[2], offset),
                );
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
        "description" => {
            let desc = objc_string(
                mem,
                &format!(
                    "{} <{}>",
                    receiver_class_name(mem, receiver, kind).unwrap_or_else(|| "Object".to_owned()),
                    receiver
                ),
            )?;
            desc
        }
        "hash" => {
            let hash = arm64_prng(state.arm64_rng_state);
            u64::from(hash)
        }
        "stringWithFormat:"
            if kind == A64_KIND_CLASS
                && (objc_text_eq(mem, class_name, b"NSString")
                    || objc_text_eq(mem, class_name, b"NSMutableString")) =>
        {
            objc_string_with_format(mem, context.regs[2])?
        }
        "substringToIndex:" if objc_is_string_kind(kind) => {
            objc_substring(mem, receiver, 0, Some(context.regs[2]))?
        }
        "substringFromIndex:" if objc_is_string_kind(kind) => {
            objc_substring(mem, receiver, context.regs[2], None)?
        }
        "capitalizedString" if objc_is_string_kind(kind) => {
            let mut bytes = objc_text(mem, receiver).unwrap_or_default();
            if let Some(first) = bytes.first_mut() {
                first.make_ascii_uppercase();
            }
            objc_string_with_kind(
                mem,
                &String::from_utf8_lossy(&bytes),
                if kind == A64_KIND_MUTABLE_STRING {
                    A64_KIND_MUTABLE_STRING
                } else {
                    A64_KIND_STRING
                },
            )?
        }
        "numberWithBool:"
            if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"NSNumber") =>
        {
            objc_number(mem, context.regs[2] as i64)?
        }
        "numberWithInt:" | "numberWithInteger:"
            if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"NSNumber") =>
        {
            objc_number(mem, context.regs[2] as i64)?
        }
        "numberWithFloat:"
            if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"NSNumber") =>
        {
            objc_number_float(mem, arm64_float_arg(context, 0) as f64)?
        }
        "numberWithDouble:"
            if kind == A64_KIND_CLASS && objc_text_eq(mem, class_name, b"NSNumber") =>
        {
            objc_number_float(mem, arm64_double_arg(context, 0))?
        }
        "arrayWithObject:"
            if kind == A64_KIND_CLASS
                && (objc_text_eq(mem, class_name, b"NSArray")
                    || objc_text_eq(mem, class_name, b"NSMutableArray")) =>
        {
            objc_array_with_kind(
                mem,
                if objc_text_eq(mem, class_name, b"NSMutableArray") {
                    A64_KIND_MUTABLE_ARRAY
                } else {
                    A64_KIND_ARRAY
                },
                &[context.regs[2]],
            )?
        }
        "arrayWithObjects:count:"
            if kind == A64_KIND_CLASS
                && (objc_text_eq(mem, class_name, b"NSArray")
                    || objc_text_eq(mem, class_name, b"NSMutableArray")) =>
        {
            let count = context.regs[3].min(4096);
            let mut objects = Vec::with_capacity(count as usize);
            for index in 0..count {
                objects.push(mem.read_u64(context.regs[2] + index * 8).unwrap_or(0));
            }
            objc_array_with_kind(
                mem,
                if objc_text_eq(mem, class_name, b"NSMutableArray") {
                    A64_KIND_MUTABLE_ARRAY
                } else {
                    A64_KIND_ARRAY
                },
                &objects,
            )?
        }
        "dictionaryWithObject:forKey:"
            if kind == A64_KIND_CLASS
                && (objc_text_eq(mem, class_name, b"NSDictionary")
                    || objc_text_eq(mem, class_name, b"NSMutableDictionary")) =>
        {
            let dictionary = objc_dictionary(mem, context.regs[3], context.regs[2])?;
            if objc_text_eq(mem, class_name, b"NSMutableDictionary") {
                mem.write_u64(dictionary, A64_KIND_MUTABLE_DICTIONARY)
                    .map_err(str::to_owned)?;
            }
            dictionary
        }
        "alloc" | "new" if kind == A64_KIND_CLASS => {
            let class_name_text = objc_text(mem, class_name)
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default();
            let object = objc_instance_for_class(mem, state, &class_name_text)?;
            if class_name_text == "UIApplication" {
                state.application_object = Some(object);
            }
            object
        }
        _ => handle_missing_objc_selector(
            receiver,
            receiver_class_name(mem, receiver, kind)
                .as_deref()
                .unwrap_or("<unknown>"),
            &selector,
            class_method,
            state,
        ),
    };
    if state.guest_transfer_pc.is_none() {
        return_value(context, result);
    }
    if state.render_diagnostics.callback_active {
        let transfer = state.guest_transfer_pc.unwrap_or(0);
        state.trace_render_event(format!("frame={} objc_return selector={} receiver={:#x} result={:#x} pc={:#x} lr={:#x} transfer={}", state.render_diagnostics.display_link_callbacks, selector, receiver, result, context.pc, context.regs[30], transfer));
    }
    if selector == "setFramebuffer" {
        log_once_fmt!(
            "ARM64 Objective-C return: selector=setFramebuffer result={:#x} receiver={:#x} sp={:#x} fp={:#x} lr={:#x} [repeated returns suppressed]",
            result,
            receiver,
            context.sp,
            context.regs[29],
            context.regs[30],
        );
    }
    Ok(())
}

fn objc_class(mem: &mut Mem64, name: u64) -> Result<u64, String> {
    let object = objc_object_with_size(mem, A64_KIND_CLASS, 0xb0)?;
    set_objc_field(mem, object, 56, name);
    Ok(object)
}
fn objc_class_for_name(
    mem: &mut Mem64,
    state: &mut RuntimeState,
    name: &str,
) -> Result<u64, String> {
    if let Some(&class) = state.class_objects.get(name) {
        return Ok(class);
    }
    let bytes = name.as_bytes();
    let pointer = mem
        .alloc_zeroed(bytes.len() as u64 + 1)
        .map_err(str::to_owned)?;
    mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
    mem.write_u8(pointer + bytes.len() as u64, 0)
        .map_err(str::to_owned)?;
    let class = objc_class(mem, pointer)?;
    if name == "EAGLView" {
        if let Some(method) = guest_method(state, name, "presentFramebuffer", false) {
            set_objc_field(mem, class, 0x98, method);
            log!(
                "ARM64 initialized EAGLView class dispatch slot: class={:#x} slot={:#x} presentFramebuffer={:#x}",
                class,
                0x98,
                method,
            );
        }
    }
    state.class_objects.insert(name.to_owned(), class);
    Ok(class)
}

fn objc_instance_for_class(
    mem: &mut Mem64,
    state: &mut RuntimeState,
    name: &str,
) -> Result<u64, String> {
    let class = objc_class_for_name(mem, state, name)?;
    let instance_size = state
        .objc_classes
        .iter()
        .find(|class_info| class_info.name == name)
        .map(|class_info| class_info.instance_size)
        .unwrap_or(A64_OBJECT_SIZE)
        .max(0x180);
    let object = objc_object_with_size(
        mem,
        objc_class_kind(mem, {
            let name_pointer = objc_field(mem, class, 56);
            name_pointer
        }),
        instance_size,
    )?;
    mem.write_u64(object, class).map_err(str::to_owned)?;
    initialize_guest_ivars(mem, state, name, object)?;
    Ok(object)
}

fn initialize_guest_ivars(
    mem: &mut Mem64,
    state: &RuntimeState,
    class_name: &str,
    object: u64,
) -> Result<(), String> {
    let mut current = Some(class_name.to_owned());
    while let Some(name) = current {
        let Some(class_info) = state
            .objc_classes
            .iter()
            .find(|class_info| class_info.name == name)
        else {
            break;
        };
        for ivar in &class_info.ivars {
            let offset = mem.read_u32(ivar.offset_address).map_err(str::to_owned)? as u64;
            if offset < 8 || offset >= 0x100000 {
                continue;
            }
            mem.write_u64(
                object
                    .checked_add(offset)
                    .ok_or("ARM64 ivar address overflows")?,
                0,
            )
            .map_err(str::to_owned)?;
        }
        current = class_info.superclass.clone();
    }
    Ok(())
}

fn receiver_class_name(mem: &Mem64, receiver: u64, kind: u64) -> Option<String> {
    let class = if kind == A64_KIND_CLASS {
        receiver
    } else {
        let first_word = mem.read_u64(receiver).ok().unwrap_or(0);
        if first_word != 0 && mem.allocation_size(first_word).is_some() {
            first_word
        } else {
            objc_field(mem, receiver, 48)
        }
    };
    objc_text(mem, objc_field(mem, class, 56)).and_then(|bytes| String::from_utf8(bytes).ok())
}

fn guest_method(
    state: &RuntimeState,
    class_name: &str,
    selector: &str,
    class_method: bool,
) -> Option<u64> {
    let mut current = Some(class_name.to_owned());
    for _ in 0..32 {
        let name = current?;
        let class = state.objc_classes.iter().find(|class| class.name == name)?;
        let methods = if class_method {
            &class.class_methods
        } else {
            &class.instance_methods
        };
        if let Some(method) = methods.iter().find(|method| method.name == selector) {
            return Some(method.address);
        }
        current = class.superclass.clone();
    }
    None
}

fn selector_pointer(mem: &mut Mem64, selector: &str) -> Result<u64, String> {
    let bytes = selector.as_bytes();
    let pointer = mem
        .alloc_zeroed(bytes.len() as u64 + 1)
        .map_err(str::to_owned)?;
    mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
    mem.write_u8(pointer + bytes.len() as u64, 0)
        .map_err(str::to_owned)?;
    Ok(pointer)
}

fn transfer_guest_method(
    mem: &Mem64,
    context: &mut touchHLE_DynarmicA64Context,
    state: &mut RuntimeState,
    selector: &str,
    class_method: bool,
) -> bool {
    let Some(class_name) = receiver_class_name(
        mem,
        context.regs[0],
        if class_method {
            A64_KIND_CLASS
        } else {
            A64_KIND_GENERIC
        },
    ) else {
        return false;
    };
    let Some(address) = guest_method(state, &class_name, selector, class_method) else {
        return false;
    };
    log_dbg!(
        "ARM64 guest Objective-C transfer: {}{} on {} -> {:#x}",
        if class_method { "+" } else { "-" },
        selector,
        class_name,
        address
    );
    if state.render_diagnostics.callback_active {
        state.trace_render_event(format!(
            "frame={} objc_transfer selector={} class={} imp={:#x} caller_pc={:#x} lr={:#x}",
            state.render_diagnostics.display_link_callbacks,
            selector,
            class_name,
            address,
            context.pc,
            context.regs[30]
        ));
    }
    if let Some(return_stub) = state.guest_method_return_stub {
        state.guest_method_return_pcs.push(context.regs[30]);
        context.regs[30] = return_stub;
    }
    state.guest_transfer_pc = Some(address);
    true
}

pub fn schedule_display_link_callback(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
    state: &mut RuntimeState,
) -> Result<bool, String> {
    if !state.display_link_scheduled || !state.display_link_callback_returned {
        log_dbg!(
            "ARM64 display-link callback not ready: scheduled={} returned={} callbacks={}",
            state.display_link_scheduled,
            state.display_link_callback_returned,
            state.display_link_callbacks
        );
        return Ok(false);
    }
    let (Some(target), Some(selector_pointer), Some(display_link), Some(return_stub)) = (
        state.display_link_target,
        state.display_link_selector,
        state.display_link_object,
        state.display_link_return_stub,
    ) else {
        return Ok(false);
    };
    let selector = c_string(mem, selector_pointer)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    let Some(class_name) = receiver_class_name(mem, target, A64_KIND_GENERIC) else {
        log!(
            "ARM64 display-link callback target has no class: target={:#x} selector={}",
            target,
            selector
        );
        return Ok(false);
    };
    let Some(address) = guest_method(state, &class_name, &selector, false) else {
        log!(
            "ARM64 display-link callback selector {} is not implemented on {}",
            selector,
            class_name
        );
        return Ok(false);
    };
    context.regs[0] = target;
    context.regs[1] = selector_pointer;
    context.regs[2] = display_link;
    context.regs[30] = return_stub;
    state.mark_display_link_callback_started();
    state.guest_transfer_pc = Some(address);
    log_dbg!(
        "ARM64 display-link callback #{}: {} on {} -> {:#x}",
        state.display_link_callbacks,
        selector,
        class_name,
        address
    );
    Ok(true)
}

pub fn materialize_import(mem: &mut Mem64, symbol: &str) -> Result<Option<u64>, String> {
    if name(symbol) == "stack_chk_guard" {
        let guard = mem.alloc_zeroed(8).map_err(str::to_owned)?;
        mem.write_u64(guard, 0x9e37_79b9_7f4a_7c15)
            .map_err(str::to_owned)?;
        return Ok(Some(guard));
    }
    if let Some(value) = materialize_host_constant(mem, symbol)? {
        return Ok(Some(value));
    }
    let symbol = name(symbol);
    if let Some(class_name) = symbol.strip_prefix("OBJC_CLASS_$_") {
        let pointer = mem
            .alloc_zeroed(class_name.len() as u64 + 1)
            .map_err(str::to_owned)?;
        mem.write_bytes(pointer, class_name.as_bytes())
            .map_err(str::to_owned)?;
        mem.write_u8(pointer + class_name.len() as u64, 0)
            .map_err(str::to_owned)?;
        return Ok(Some(objc_class(mem, pointer)?));
    }
    if let Some(class_name) = symbol.strip_prefix("OBJC_METACLASS_$_") {
        let pointer = mem
            .alloc_zeroed(class_name.len() as u64 + 1)
            .map_err(str::to_owned)?;
        mem.write_bytes(pointer, class_name.as_bytes())
            .map_err(str::to_owned)?;
        mem.write_u8(pointer + class_name.len() as u64, 0)
            .map_err(str::to_owned)?;
        return Ok(Some(objc_class(mem, pointer)?));
    }
    if symbol == "CFConstantStringClassReference" {
        let bytes = b"NSConstantString";
        let pointer = mem
            .alloc_zeroed(bytes.len() as u64 + 1)
            .map_err(str::to_owned)?;
        mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
        mem.write_u8(pointer + bytes.len() as u64, 0)
            .map_err(str::to_owned)?;
        return Ok(Some(objc_class(mem, pointer)?));
    }
    Ok(None)
}

pub fn dispatch(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
    symbol: &str,
    state: &mut RuntimeState,
    window: Option<&mut Window>,
) -> Result<bool, String> {
    state.host_dispatches = state.host_dispatches.saturating_add(1);
    let symbol = name(symbol);
    state.last_symbol = Some(symbol.to_owned());
    if is_light_host_call(symbol) {
        return Ok(dispatch_arm64_math(context, symbol));
    }
    if symbol == "ARM64_guest_method_return" {
        let return_pc = state
            .guest_method_return_pcs
            .pop()
            .ok_or("ARM64 guest method return had no saved continuation")?;
        context.regs[30] = return_pc;
        log_dbg!(
            "ARM64 guest Objective-C method returned: continuation={:#x} result={:#x} remaining={}",
            return_pc,
            context.regs[0],
            state.guest_method_return_pcs.len()
        );
        return Ok(true);
    }
    if symbol == "ARM64_nib_awake_return" {
        state.nib_awake_dispatched = true;
        let (Some(delegate), Some(application)) =
            (state.application_delegate, state.application_object)
        else {
            return Err(
                "ARM64 awakeFromNib returned before application objects were initialized"
                    .to_owned(),
            );
        };
        let class_name = receiver_class_name(mem, delegate, A64_KIND_GENERIC).unwrap_or_default();
        let Some(address) = guest_method(
            state,
            &class_name,
            "application:didFinishLaunchingWithOptions:",
            false,
        ) else {
            return Err("ARM64 could not resolve application:didFinishLaunchingWithOptions: after awakeFromNib".to_owned());
        };
        let selector = selector_pointer(mem, "application:didFinishLaunchingWithOptions:")?;
        context.regs[0] = delegate;
        context.regs[1] = selector;
        context.regs[2] = application;
        context.regs[3] = 0;
        if let Some(return_stub) = state.application_launch_return_stub {
            context.regs[30] = return_stub;
        }
        state.pending_application_did_become_active = true;
        state.guest_transfer_pc = Some(address);
        log!("ARM64 lifecycle transfer: awakeFromNib completed; application:didFinishLaunchingWithOptions: -> {:#x}", address);
        return Ok(true);
    }
    if symbol == "ARM64_application_launch_return" {
        state.pending_application_did_become_active = false;
        if let (Some(delegate), Some(application)) =
            (state.application_delegate, state.application_object)
        {
            let class_name =
                receiver_class_name(mem, delegate, A64_KIND_GENERIC).unwrap_or_default();
            if let Some(address) =
                guest_method(state, &class_name, "applicationDidBecomeActive:", false)
            {
                let selector = selector_pointer(mem, "applicationDidBecomeActive:")?;
                context.regs[0] = delegate;
                context.regs[1] = selector;
                context.regs[2] = application;
                if let Some(return_stub) = state.application_active_return_stub {
                    context.regs[30] = return_stub;
                }
                state.guest_transfer_pc = Some(address);
                log!(
                    "ARM64 lifecycle transfer: applicationDidBecomeActive: -> {:#x}",
                    address
                );
                return Ok(true);
            }
        }
        if let Some(return_stub) = state.application_return_stub {
            context.regs[30] = return_stub;
        }
        log!("ARM64 lifecycle callback completed: no guest applicationDidBecomeActive: implementation");
        return Ok(true);
    }
    if symbol == "ARM64_application_return" {
        state.pending_application_did_become_active = false;
        if let Some(return_pc) = state.launch_callback_return_pc {
            context.regs[30] = return_pc;
        }
        log!(
            "ARM64 lifecycle callback completed: returning from UIApplicationMain to guest caller"
        );
        return Ok(true);
    }
    if symbol == "ARM64_application_active_return" {
        state.pending_application_did_become_active = false;
        if let Some(return_stub) = state.application_return_stub {
            context.regs[30] = return_stub;
        }
        log!("ARM64 lifecycle callback completed: applicationDidBecomeActive:");
        return Ok(true);
    }
    if symbol == "ARM64_display_link_return" {
        state.mark_display_link_callback_returned(context.regs[30]);
        state.guest_yield_requested = true;
        return_value(context, 0);
        log_dbg!(
            "ARM64 display-link callback returned: callbacks={} selector={}",
            state.display_link_callbacks,
            state.last_selector.as_deref().unwrap_or("<none>")
        );
        return Ok(true);
    }
    match symbol {
        "malloc" | "calloc" | "valloc" | "posix_memalign" => {
            let requested_size = if symbol == "calloc" {
                A64Abi::arg(context, 0)
                    .checked_mul(A64Abi::arg(context, 1))
                    .unwrap_or(u64::MAX)
            } else if symbol == "posix_memalign" {
                A64Abi::arg(context, 2)
            } else {
                A64Abi::arg(context, 0)
            };
            let Some(size) = normalize_arm64_allocation_size(requested_size) else {
                return_value(context, 0);
                return Ok(true);
            };
            let address = match mem.alloc_zeroed(size) {
                Ok(address) => address,
                Err(_) => {
                    return_value(context, 0);
                    return Ok(true);
                }
            };
            if symbol == "posix_memalign" {
                mem.write_u64(context.regs[0], address).map_err(str::to_owned)?;
                return_value(context, 0);
            } else {
                return_value(context, address);
            }
            Ok(true)
        }
        "free" | "malloc_zone_free" | "_ZdlPv" | "_ZdaPv" | "__ZdlPv" | "__ZdaPv" | "ZdlPv" | "ZdaPv" => {
            let pointer = A64Abi::arg(context, 0);
            let allocation_size = mem.allocation_size(pointer);
            let released = pointer == 0 || mem.free(pointer);
            if state.host_dispatches <= 16 || state.host_dispatches.is_power_of_two() {
                log_dbg!(
                    "ARM64 deallocation: symbol={} pointer={pointer:#x} allocation_size={allocation_size:?} released={} pc={:#x} lr={:#x} sp={:#x}",
                    symbol, released, context.pc, context.regs[30], context.sp,
                );
            }
            return_value(context, 0);
            Ok(true)
        }
        "_Znwm" | "_Znam" | "__Znwm" | "__Znam"
        | "Znwm" | "Znam" | "ZnwmRKSt9nothrow_t" | "__ZnwmRKSt9nothrow_t" => {
            let requested_size = A64Abi::arg(context, 0);
            let Some(size) = normalize_arm64_allocation_size(requested_size) else {
                return_value(context, 0);
                return Ok(true);
            };
            match mem.alloc_zeroed(size) {
                Ok(address) => {
                    if state.host_dispatches <= 16 || state.host_dispatches.is_power_of_two() {
                        log_dbg!(
                            "ARM64 allocation: symbol={} size={} size_hex={size:#x} result={address:#x} pc={:#x} lr={:#x} sp={:#x}",
                            symbol, size, context.pc, context.regs[30], context.sp,
                        );
                    }
                    return_value(context, address);
                    Ok(true)
                }
                Err(error) => {
                    log_dbg!(
                        "ARM64 allocation failed: symbol={} size={} requested={requested_size:#x} reason={} pc={:#x} lr={:#x} sp={:#x}",
                        symbol, size, error, context.pc, context.regs[30], context.sp,
                    );
                    return_value(context, 0);
                    Ok(true)
                }
            }
        }
        "access" => {
            return_value(context, 0);
            Ok(true)
        }
        "mkdir" => {
            return_value(context, 0);
            Ok(true)
        }
        "signal" => {
            let signum = context.regs[0] as i32;
            let handler = context.regs[1];
            let previous = state.signal_handlers.insert(signum, handler).unwrap_or(0);
            return_value(context, previous);
            Ok(true)
        }
        "NSSearchPathForDirectoriesInDomains" => {
            let path = arm64_search_path(state, context.regs[0], context.regs[1]);
            let result = match path.as_deref() {
                Some(path) => {
                    let string = objc_string(mem, path)?;
                    objc_array(mem, &[string])?
                }
                None => objc_array(mem, &[])?
            };
            log_once_fmt!(
                "ARM64 Foundation path search: directory={} domain_mask={:#x} result={:#x} path={} [repeated calls suppressed]",
                context.regs[0],
                context.regs[1],
                result,
                path.as_deref().unwrap_or("<empty>"),
            );
            return_value(context, result);
            Ok(true)
        }
        "NSSelectorFromString" => {
            let selector = objc_text(mem, context.regs[0]).unwrap_or_default();
            let selector = String::from_utf8(selector).map_err(|_| "ARM64 selector string is not UTF-8")?;
            let pointer = selector_pointer(mem, &selector)?;
            log_once_fmt!(
                "ARM64 selector registration: selector={} pointer={:#x} [repeated registrations suppressed]",
                selector,
                pointer,
            );
            return_value(context, pointer);
            Ok(true)
        }
        "objc_setProperty" | "objc_setProperty_nonatomic" | "objc_setProperty_atomic"
        | "objc_setProperty_nonatomic_copy" | "objc_setProperty_atomic_copy" => {
            let offset = context.regs[2] as i32 as i64;
            let address = context.regs[0]
                .checked_add_signed(offset)
                .ok_or("ARM64 objc_setProperty address overflows")?;
            mem.write_u64(address, context.regs[3]).map_err(str::to_owned)?;
            log_once_fmt!(
                "ARM64 Objective-C property store: receiver={:#x} offset={} value={:#x} address={:#x} [repeated stores suppressed]",
                context.regs[0],
                offset,
                context.regs[3],
                address,
            );
            return_value(context, 0);
            Ok(true)
        }
        "time" => {
            let seconds = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "ARM64 system clock is before the Unix epoch")?
                .as_secs() as i32;
            if context.regs[0] != 0 {
                mem.write_u32(context.regs[0], seconds as u32).map_err(str::to_owned)?;
            }
            return_value(context, seconds as i64 as u64);
            Ok(true)
        }
        "srand" => {
            state.arm64_rng_state = (context.regs[0] as u32).max(1);
            return_value(context, 0);
            Ok(true)
        }
        "rand" => {
            state.arm64_rng_state = arm64_prng(state.arm64_rng_state);
            return_value(context, u64::from(state.arm64_rng_state & 0x7fff_ffff));
            Ok(true)
        }
        "objc_alloc" | "objc_allocWithZone" => {
            let class = context.regs[0];
            let class_name = c_string(mem, objc_field(mem, class, 56))
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .or_else(|| receiver_class_name(mem, class, A64_KIND_CLASS));
            let Some(class_name) = class_name else {
                return_value(context, 0);
                return Ok(true);
            };
            log_dbg!(
                "ARM64 objc_alloc: class={:#x} name={} zone={:#x}",
                class,
                class_name,
                context.regs[1]
            );
            let object = objc_instance_for_class(mem, state, &class_name)?;
            return_value(context, object);
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
            let requested_size = context.regs[1];
            let Some(size) = normalize_arm64_allocation_size(requested_size) else {
                return_value(context, 0);
                return Ok(true);
            };
            let address = if old == 0 {
                mem.alloc_zeroed(size).map_err(str::to_owned)?
            } else {
                mem.realloc(old, size).map_err(str::to_owned)?
            };
            return_value(context, address);
            Ok(true)
        }
        "memcpy" | "memcpy_chk" => {
            let size = context.regs[2];
            mem.copy_bytes(context.regs[0], context.regs[1], size)
                .map_err(str::to_owned)?;
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "memmove" | "memmove_chk" => {
            let size = context.regs[2];
            mem.copy_bytes_overlap_safe(context.regs[0], context.regs[1], size)
                .map_err(str::to_owned)?;
            return_value(context, context.regs[0]);
            Ok(true)
        }
        "bcopy" => {
            mem.copy_bytes_overlap_safe(context.regs[1], context.regs[0], context.regs[2])
                .map_err(str::to_owned)?;
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
        | "ZNSt3__112basic_stringIcNS_11char_traitsIcEENS_9allocatorIcEEED1Ev"
        | "ZSt9terminatev" | "Unwind_Resume" | "__Unwind_Resume" => {
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
        "ZNSt3__16thread20hardware_concurrencyEv" => {
            return_value(context, 4);
            Ok(true)
        }
        "ZNSt3__112__next_primeEm" => {
            let input = context.regs[0];
            let result = next_prime(input);
            return_value(context, result);
            Ok(true)
        }
        "ZNKSt3__120__vector_base_commonILb1EE20__throw_length_errorEv"
        | "ZNKSt3__120__vector_base_commonILb1EE20__throw_out_of_rangeEv"
        | "ZNKSt3__121__basic_string_commonILb1EE20__throw_length_errorEv"
        | "ZNKSt3__16locale9has_facetERNS0_2idE" | "ZNKSt3__16locale9use_facetERNS0_2idE"
        | "ZNKSt3__18ios_base6getlocEv" | "ZNSt3__111this_thread9sleep_forERKNS_6chrono8durationIxNS_5ratioILl1ELl1000000000EEEEE" => {
            return_value(context, 0);
            Ok(true)
        }
        value if value.starts_with("ZNSt3__") || value.starts_with("ZNKSt3__") => {
            return_value(context, if context.regs[0] != 0 { context.regs[0] } else { 0 });
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
        "pthread_setname_np" => {
            let name = if context.regs[0] == 0 {
                String::new()
            } else {
                arm64_cstring(mem, context.regs[0])?
                    .to_string_lossy()
                    .chars()
                    .take(63)
                    .collect()
            };
            log_once_fmt!(
                "ARM64 pthread_setname_np: name={name:?} [subsequent calls suppressed]"
            );
            return_value(context, 0);
            Ok(true)
        }
        "pthread_create" => {
            let output = context.regs[0];
            let start_routine = context.regs[2];
            if output == 0 {
                return_value(context, 22);
                return Ok(true);
            }
            if state.pthread_handles.len() >= MAX_ARM64_PTHREAD_WORKERS {
                log_once_fmt!(
                    "ARM64 pthread_create: worker limit ({MAX_ARM64_PTHREAD_WORKERS}) reached; returning EAGAIN for later requests"
                );
                return_value(context, 35);
                return Ok(true);
            }
            let thread_id = state.next_pthread_id;
            state.next_pthread_id = state.next_pthread_id.saturating_add(1);
            mem.write_u64(output, thread_id).map_err(str::to_owned)?;
            state.pthread_handles.insert(thread_id, start_routine);
            log_once_fmt!(
                "ARM64 pthread_create: bounded emulated worker pool enabled; repeated calls suppressed"
            );
            return_value(context, 0);
            Ok(true)
        }
        "pthread_join" => {
            let thread_id = context.regs[0];
            if context.regs[1] != 0 {
                mem.write_u64(context.regs[1], 0).map_err(str::to_owned)?;
            }
            let result = if state.pthread_handles.remove(&thread_id).is_some()
                || state.pthread_finished.remove(&thread_id)
            {
                0
            } else {
                3
            };
            return_value(context, result);
            Ok(true)
        }
        "cxa_atexit" | "atexit" | "pthread_mutex_lock" | "pthread_mutex_unlock" | "pthread_mutex_init" | "pthread_mutex_destroy" | "pthread_once" | "pthread_key_create" | "pthread_setspecific" | "sched_yield" => {
            if symbol == "pthread_once" && context.regs[0] != 0 {
                let control = context.regs[0];
                if !state.pthread_once_controls.contains(&control) {
                    state.pthread_once_controls.insert(control);
                    mem.write_u32(control, 1).map_err(str::to_owned)?;
                }
            }
            if symbol == "pthread_key_create" {
                let output = context.regs[0];
                let key = state.next_pthread_key;
                state.next_pthread_key = state.next_pthread_key.saturating_add(1);
                if output != 0 {
                    mem.write_u64(output, key).map_err(str::to_owned)?;
                }
            }
            if symbol == "pthread_setspecific" {
                state.pthread_key_values.insert(context.regs[0], context.regs[1]);
            }
            return_value(context, 0);
            Ok(true)
        }
        "pthread_getspecific" => {
            return_value(context, state.pthread_key_values.get(&context.regs[0]).copied().unwrap_or(0));
            Ok(true)
        }
        "pthread_key_delete" => {
            state.pthread_key_values.remove(&context.regs[0]);
            return_value(context, 0);
            Ok(true)
        }
        "pthread_detach" => {
            state.pthread_handles.remove(&context.regs[0]);
            state.pthread_finished.remove(&context.regs[0]);
            return_value(context, 0);
            Ok(true)
        }
        "pthread_equal" => {
            return_value(context, u64::from(context.regs[0] == context.regs[1]));
            Ok(true)
        }
        "pthread_exit" => {
            return_value(context, 0);
            Ok(true)
        }
        "gettimeofday" => {
            if context.regs[0] != 0 {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| "ARM64 system clock is before the Unix epoch")?;
                mem.write_u64(context.regs[0], now.as_secs()).map_err(str::to_owned)?;
                mem.write_u32(context.regs[0] + 8, now.subsec_micros()).map_err(str::to_owned)?;
            }
            if context.regs[1] != 0 {
                mem.write_u32(context.regs[1], 0).map_err(str::to_owned)?;
                mem.write_u32(context.regs[1] + 4, 0).map_err(str::to_owned)?;
            }
            return_value(context, 0);
            Ok(true)
        }
        "pthread_condattr_init" | "pthread_mutexattr_init" => {
            let output = context.regs[0];
            if output != 0 {
                mem.fill_bytes(output, 0, if symbol == "pthread_condattr_init" { 4 } else { 12 }).map_err(str::to_owned)?;
            }
            return_value(context, 0);
            Ok(true)
        }
        "pthread_condattr_destroy" | "pthread_mutexattr_destroy" | "pthread_cond_destroy"
        | "pthread_cond_wait" | "pthread_cond_signal" | "pthread_cond_broadcast"
        | "pthread_mutex_trylock" | "pthread_mutexattr_setpshared"
        | "pthread_mutexattr_settype" | "pthread_mutexattr_gettype"
        | "pthread_mutexattr_setprotocol" | "pthread_mutexattr_getprotocol"
        | "pthread_mutexattr_setprioceiling" | "pthread_mutexattr_getprioceiling"
        | "pthread_condattr_setpshared" | "pthread_condattr_getpshared"
        | "pthread_condattr_setclock" | "pthread_condattr_getclock" => {
            return_value(context, 0);
            Ok(true)
        }

        "pthread_cond_init" => {
            let output = context.regs[0];
            if output != 0 {
                mem.fill_bytes(output, 0, 28).map_err(str::to_owned)?;
            }
            return_value(context, 0);
            Ok(true)
        }
        "pthread_mutex_init" => {
            let output = context.regs[0];
            if output != 0 {
                mem.fill_bytes(output, 0, 12).map_err(str::to_owned)?;
            }
            return_value(context, 0);
            Ok(true)
        }
        "pthread_setname_np" => {
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
            state.application_main_active = true;
            state.application_main_calls = state.application_main_calls.saturating_add(1);
            if !state.arm64_application_bootstrap_dispatched {
                state.application_bootstrap_requested = true;
            }
            state.present_requested = true;
            let application = state.application_object.unwrap_or(objc_instance_for_class(mem, state, "UIApplication")?);
            state.application_object = Some(application);
            let delegate_name = if context.regs[3] != 0 {
                objc_text(mem, context.regs[3])
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .filter(|name| {
                        guest_method(state, name, "application:didFinishLaunchingWithOptions:", false)
                            .is_some()
                            || guest_method(state, name, "applicationDidFinishLaunching:", false)
                                .is_some()
                    })
            } else {
                None
            }
            .or_else(|| {
                state
                    .objc_classes
                    .iter()
                    .find(|class| {
                        guest_method(
                            state,
                            &class.name,
                            "application:didFinishLaunchingWithOptions:",
                            false,
                        )
                        .is_some()
                            || guest_method(
                                state,
                                &class.name,
                                "applicationDidFinishLaunching:",
                                false,
                            )
                            .is_some()
                    })
                    .map(|class| class.name.clone())
            });
            log_dbg!(
                "ARM64 UIApplicationMain delegate candidate: arg={:#x} name={} selected={}",
                context.regs[3],
                objc_text(mem, context.regs[3])
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .as_deref()
                    .unwrap_or("<invalid>"),
                delegate_name.as_deref().unwrap_or("<none>")
            );
            if let Some(delegate_name) = delegate_name {
                if state.arm64_application_bootstrap_dispatched {
                    return_value(context, 0);
                    return Ok(true);
                }
                state.arm64_application_bootstrap_dispatched = true;
                let delegate = objc_instance_for_class(mem, state, &delegate_name)?;
                state.application_delegate = Some(delegate);
                if state.main_nib_name.is_some() {
                    let view_controller_name = state.objc_classes.iter()
                        .map(|class| class.name.as_str())
                        .find(|name| name.to_ascii_lowercase().contains("viewcontroller"))
                        .map(str::to_owned);
                    let window = objc_object(mem, A64_KIND_VIEW)?;
                    let view = objc_instance_for_class(mem, state, "EAGLView")?;
                    state.application_window = Some(window);
                    state.application_view = Some(view);
                    initialize_eagl_view(mem, state, view)?;
                    if let Some(view_controller_name) = view_controller_name {
                        let view_controller = objc_instance_for_class(mem, state, &view_controller_name)?;
                        log!(
                            "ARM64 initialized UIViewController view bridge: class={} object={:#x} view={:#x}",
                            view_controller_name,
                            view_controller,
                            view,
                        );
                        if let Some(context) = state.graphics_context {
                            let context_offsets = ["context", "_context"]
                                .iter()
                                .filter_map(|name| {
                                    guest_ivar_offset(mem, state, &view_controller_name, name)
                                        .map(|offset| (*name, offset))
                                })
                                .collect::<Vec<_>>();
                            let context_mapped = set_guest_ivar_u64_aliases(
                                mem,
                                state,
                                &view_controller_name,
                                view_controller,
                                &["context", "_context"],
                                context,
                            )?;
                            let context_values = context_offsets
                                .iter()
                                .map(|(name, offset)| {
                                    format!(
                                        "{}@{:#x}={:#x}",
                                        name,
                                        offset,
                                        mem.read_u64(view_controller + *offset).unwrap_or(0)
                                    )
                                })
                                .collect::<Vec<_>>()
                                .join(",");
                            log!(
                                "ARM64 initialized view-controller context: class={} object={:#x} context={:#x} mapped={} ivars=[{}]",
                                view_controller_name,
                                view_controller,
                                context,
                                context_mapped,
                                context_values,
                            );
                        }
                        state.application_view_controller = Some(view_controller);
                    }
                }
                state.launch_callback_return_pc = Some(context.regs[30]);
                if let Some(view_controller) = state.application_view_controller {
                    if let Some(address) = receiver_class_name(mem, view_controller, A64_KIND_GENERIC).as_deref().and_then(|class_name| guest_method(state, class_name, "awakeFromNib", false)) {
                        let selector = selector_pointer(mem, "awakeFromNib")?;
                        context.regs[0] = view_controller;
                        context.regs[1] = selector;
                        context.regs[2] = 0;
                        state.nib_awake_dispatched = false;
                        if let Some(return_stub) = state.nib_awake_return_stub {
                            context.regs[30] = return_stub;
                        }
                        state.guest_transfer_pc = Some(address);
                        log!("ARM64 UIApplicationMain transferring to view controller awakeFromNib at {:#x}; callback return is intercepted for lifecycle continuation", address);
                    }
                }
                if state.guest_transfer_pc.is_none() {
                    let selector = selector_pointer(mem, "application:didFinishLaunchingWithOptions:")?;
                    context.regs[0] = delegate;
                    context.regs[1] = selector;
                    context.regs[2] = application;
                    context.regs[3] = 0;
                    if let Some(address) = guest_method(state, &delegate_name, "application:didFinishLaunchingWithOptions:", false) {
                        state.guest_transfer_pc = Some(address);
                        state.launch_callback_return_pc = Some(context.regs[30]);
                        state.pending_application_did_become_active = true;
                        if let Some(return_stub) = state.application_launch_return_stub {
                            context.regs[30] = return_stub;
                        }
                        log!("ARM64 UIApplicationMain transferring to {} application:didFinishLaunchingWithOptions: at {:#x}; callback return is intercepted for lifecycle continuation", delegate_name, address);
                    }
                }
            }
            echo!("ARM64 UIApplicationMain entered the compatibility application lifecycle; guest application delegate dispatch is active");
            if state.guest_transfer_pc.is_none() {
                return_value(context, 0);
            }
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
            return arm64_gl_call(mem, context, symbol, state, window);
        }
        _ => Ok(false),
    }
}

fn arm64_const_ptr(
    mem: &Mem64,
    address: u64,
    size: u64,
) -> Result<*const std::ffi::c_void, String> {
    if address == 0 {
        Ok(std::ptr::null())
    } else {
        mem.host_ptr(address, size)
            .map(|pointer| pointer.cast())
            .map_err(str::to_owned)
    }
}

fn arm64_mut_ptr(
    mem: &mut Mem64,
    address: u64,
    size: u64,
) -> Result<*mut std::ffi::c_void, String> {
    if address == 0 {
        Ok(std::ptr::null_mut())
    } else {
        mem.host_ptr_mut(address, size)
            .map(|pointer| pointer.cast())
            .map_err(str::to_owned)
    }
}

fn arm64_cstring(mem: &Mem64, address: u64) -> Result<std::ffi::CString, String> {
    if address == 0 {
        return Ok(std::ffi::CString::default());
    }
    let length = mem.cstr_len(address, MAX_CSTRING).map_err(str::to_owned)?;
    let bytes = mem.read_bytes(address, length).map_err(str::to_owned)?;
    std::ffi::CString::new(bytes)
        .map_err(|_| "ARM64 guest string contains an embedded NUL".to_owned())
}

fn arm64_host_gl(window: Option<&mut Window>, call: impl FnOnce(&mut dyn crate::gles::GLES)) {
    if let Some(window) = window {
        let mut gl = window.make_internal_gl_ctx_current();
        call(gl.as_mut());
    }
}

fn arm64_host_gl_with_error(
    window: Option<&mut Window>,
    state: &mut RuntimeState,
    operation: &str,
    pc: u64,
    call: impl FnOnce(&mut dyn crate::gles::GLES),
) -> u32 {
    let mut error = 0;
    if let Some(window) = window {
        let mut gl = window.make_internal_gl_ctx_current();
        call(gl.as_mut());
        error = unsafe { gl.GetError() as u32 };
    }
    state.trace_render_event(format!(
        "operation={} pc={:#x} host_error={:#x}",
        operation, pc, error
    ));
    error
}

fn arm64_framebuffer_host_id(id: u32) -> u32 {
    id.saturating_sub(1)
}

fn arm64_renderbuffer_host_id(id: u32) -> u32 {
    id.saturating_sub(1)
}

fn arm64_texture_data_size(width: u32, height: u32, format: u32, type_: u32) -> u64 {
    let components = match format {
        0x1907 => 3,
        0x1908 => 4,
        0x1909 | 0x190A => 1,
        _ => 4,
    };
    let bytes_per_component = match type_ {
        0x1401 => 1,
        0x1403 | 0x1405 => 2,
        _ => 4,
    };
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(components)
        .saturating_mul(bytes_per_component)
}

fn arm64_copy_host_string(mem: &mut Mem64, pointer: *const u8) -> Result<u64, String> {
    if pointer.is_null() {
        return Ok(0);
    }
    let bytes = unsafe { std::ffi::CStr::from_ptr(pointer.cast()) }.to_bytes();
    let guest = mem
        .alloc_zeroed(bytes.len() as u64 + 1)
        .map_err(str::to_owned)?;
    mem.write_bytes(guest, bytes).map_err(str::to_owned)?;
    mem.write_u8(guest + bytes.len() as u64, 0)
        .map_err(str::to_owned)?;
    Ok(guest)
}

fn arm64_gl_call(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
    symbol: &str,
    state: &mut RuntimeState,
    window: Option<&mut Window>,
) -> Result<bool, String> {
    state.render_diagnostics.gl_calls = state.render_diagnostics.gl_calls.saturating_add(1);
    state.render_diagnostics.last_gl_symbol = Some(symbol.to_owned());
    state.render_diagnostics.last_gl_pc = context.pc;
    let guest_pc = context.pc;
    if state.arm64_gl.is_none() {
        state.arm64_gl = Some(Arm64GuestGlState {
            viewport: [0, 0, state.screen_width as i32, state.screen_height as i32],
            clear_color: [0.0, 0.0, 0.0, 1.0],
            ..Arm64GuestGlState::default()
        });
    }
    match symbol {
        "glGetError" => {
            let mut error = state.arm64_gl.as_ref().map_or(0, |gl| gl.gl_error);
            arm64_host_gl(window, |host| error = unsafe { host.GetError() as u32 });
            if let Some(gl) = state.arm64_gl.as_mut() {
                gl.gl_error = 0;
            }
            return_value(context, u64::from(error));
        }
        "glGetString" => {
            let name = context.regs[0] as u32;
            if let Some(&value) = state.arm64_gl.as_ref().and_then(|gl| gl.strings.get(&name)) {
                return_value(context, value);
                return Ok(true);
            }
            let mut host_string = std::ptr::null();
            arm64_host_gl(window, |host| {
                host_string = unsafe { host.GetString(name as _) }
            });
            let value = arm64_copy_host_string(mem, host_string.cast())?;
            if let Some(gl) = state.arm64_gl.as_mut() {
                gl.strings.insert(name, value);
            }
            return_value(context, value);
        }
        "glBindFramebuffer" | "glBindFramebufferOES" => {
            if let Some(gl) = state.arm64_gl.as_mut() {
                gl.bind_framebuffer_calls = gl.bind_framebuffer_calls.saturating_add(1);
                gl.last_bind_framebuffer_pc = context.pc;
                gl.last_bind_framebuffer_target = context.regs[0] as u32;
                gl.last_bind_framebuffer = context.regs[1] as u32;
                gl.current_framebuffer = context.regs[1] as u32;
            }
            let target = context.regs[0] as _;
            let framebuffer = arm64_framebuffer_host_id(context.regs[1] as u32);
            let host_error =
                arm64_host_gl_with_error(window, state, symbol, guest_pc, |host| unsafe {
                    if symbol.ends_with("OES") {
                        host.BindFramebufferOES(target, framebuffer)
                    } else {
                        host.BindFramebuffer(target, framebuffer)
                    }
                });
            state.trace_render_event(format!("frame={} callback_pc={:#x} gl_pc={:#x} op={} target={:#x} guest={} host={} gl_error={:#x} continuation={:#x}", state.render_diagnostics.display_link_callbacks, state.render_diagnostics.last_callback_pc, guest_pc, symbol, target, context.regs[1], framebuffer, host_error, context.regs[30]));
            log_once_fmt!(
                "ARM64 GLES framebuffer binding active: target={:#x} framebuffer={} pc={:#x} gl_error={:#x} [repeated binds suppressed]",
                state.arm64_gl.as_ref().map_or(0, |gl| gl.last_bind_framebuffer_target),
                state.arm64_gl.as_ref().map_or(0, |gl| gl.last_bind_framebuffer),
                state.arm64_gl.as_ref().map_or(0, |gl| gl.last_bind_framebuffer_pc),
                state.arm64_gl.as_ref().map_or(0, |gl| gl.gl_error),
            );
        }
        "glBindRenderbuffer" | "glBindRenderbufferOES" => {
            if let Some(gl) = state.arm64_gl.as_mut() {
                gl.current_renderbuffer = context.regs[1] as u32;
            }
            let target = context.regs[0] as _;
            let renderbuffer = arm64_renderbuffer_host_id(context.regs[1] as u32);
            let host_error =
                arm64_host_gl_with_error(window, state, symbol, guest_pc, |host| unsafe {
                    if symbol.ends_with("OES") {
                        host.BindRenderbufferOES(target, renderbuffer)
                    } else {
                        host.BindRenderbuffer(target, renderbuffer)
                    }
                });
            state.trace_render_event(format!("frame={} callback_pc={:#x} gl_pc={:#x} op={} target={:#x} guest={} host={} gl_error={:#x} continuation={:#x}", state.render_diagnostics.display_link_callbacks, state.render_diagnostics.last_callback_pc, guest_pc, symbol, target, context.regs[1], renderbuffer, host_error, context.regs[30]));
        }
        "glGenFramebuffers"
        | "glGenFramebuffersOES"
        | "glGenRenderbuffers"
        | "glGenRenderbuffersOES" => {
            let count = context.regs[0] as i32;
            let output = context.regs[1];
            let size = u64::try_from(count.max(0)).unwrap_or(0).saturating_mul(4);
            let pointer = arm64_mut_ptr(mem, output, size)?;
            let mut values = vec![0u32; count.max(0) as usize];
            arm64_host_gl(window, |host| unsafe {
                if symbol.contains("Framebuffers") {
                    host.GenFramebuffers(count, values.as_mut_ptr());
                } else {
                    host.GenRenderbuffers(count, values.as_mut_ptr());
                }
            });
            for value in &mut values {
                *value = value.saturating_add(1);
            }
            if !pointer.is_null() {
                let bytes = unsafe {
                    std::slice::from_raw_parts(values.as_ptr().cast::<u8>(), size as usize)
                };
                mem.write_bytes(output, bytes).map_err(str::to_owned)?;
            }
        }
        "glDeleteFramebuffers"
        | "glDeleteFramebuffersOES"
        | "glDeleteRenderbuffers"
        | "glDeleteRenderbuffersOES" => {
            let count = context.regs[0] as i32;
            let input = context.regs[1];
            let mut values = Vec::with_capacity(count.max(0) as usize);
            for index in 0..count.max(0) {
                values.push(
                    mem.read_u32(input + index as u64 * 4)
                        .map_err(str::to_owned)?
                        .saturating_sub(1),
                );
            }
            arm64_host_gl(window, |host| unsafe {
                if symbol.contains("Framebuffers") {
                    host.DeleteFramebuffers(count, values.as_ptr());
                } else {
                    host.DeleteRenderbuffers(count, values.as_ptr());
                }
            });
        }
        "glCheckFramebufferStatus" | "glCheckFramebufferStatusOES" => {
            let target = context.regs[0] as _;
            let mut status = 0x8cd5u32;
            let host_error =
                arm64_host_gl_with_error(window, state, symbol, guest_pc, |host| unsafe {
                    status = if symbol.ends_with("OES") {
                        host.CheckFramebufferStatusOES(target) as u32
                    } else {
                        host.CheckFramebufferStatus(target) as u32
                    };
                });
            state.trace_render_event(format!("frame={} callback_pc={:#x} gl_pc={:#x} op={} target={:#x} status={:#x} host_error={:#x} continuation={:#x}", state.render_diagnostics.display_link_callbacks, state.render_diagnostics.last_callback_pc, guest_pc, symbol, target, status, host_error, context.regs[30]));
            return_value(context, u64::from(status));
            return Ok(true);
        }
        "glRenderbufferStorageOES" | "glRenderbufferStorage" => {
            let target = context.regs[0] as _;
            let internalformat = context.regs[1] as _;
            let width = context.regs[2] as i32;
            let height = context.regs[3] as i32;
            arm64_host_gl(window, |host| unsafe {
                if symbol.ends_with("OES") {
                    host.RenderbufferStorageOES(target, internalformat, width, height)
                } else {
                    host.RenderbufferStorage(target, internalformat, width, height)
                }
            });
        }
        "glFramebufferRenderbuffer" | "glFramebufferRenderbufferOES" => {
            let target = context.regs[0] as _;
            let attachment = context.regs[1] as _;
            let renderbuffer_target = context.regs[2] as _;
            let renderbuffer = arm64_renderbuffer_host_id(context.regs[3] as u32);
            let host_error =
                arm64_host_gl_with_error(window, state, symbol, guest_pc, |host| unsafe {
                    if symbol.ends_with("OES") {
                        host.FramebufferRenderbufferOES(
                            target,
                            attachment,
                            renderbuffer_target,
                            renderbuffer,
                        )
                    } else {
                        host.FramebufferRenderbuffer(
                            target,
                            attachment,
                            renderbuffer_target,
                            renderbuffer,
                        )
                    }
                });
            state.trace_render_event(format!("frame={} callback_pc={:#x} gl_pc={:#x} op={} target={:#x} attachment={:#x} guest={} host={} gl_error={:#x} continuation={:#x}", state.render_diagnostics.display_link_callbacks, state.render_diagnostics.last_callback_pc, guest_pc, symbol, target, attachment, context.regs[3], renderbuffer, host_error, context.regs[30]));
        }
        "glViewport" => {
            let viewport = [
                context.regs[0] as i32,
                context.regs[1] as i32,
                context.regs[2] as i32,
                context.regs[3] as i32,
            ];
            if let Some(gl) = state.arm64_gl.as_mut() {
                gl.viewport = viewport;
            }
            let host_error =
                arm64_host_gl_with_error(window, state, symbol, guest_pc, |host| unsafe {
                    host.Viewport(
                        context.regs[0] as _,
                        context.regs[1] as _,
                        context.regs[2] as _,
                        context.regs[3] as _,
                    )
                });
            state.trace_render_event(format!("frame={} callback_pc={:#x} gl_pc={:#x} op={} viewport={:?} host_error={:#x} continuation={:#x}", state.render_diagnostics.display_link_callbacks, state.render_diagnostics.last_callback_pc, guest_pc, symbol, viewport, host_error, context.regs[30]));
        }
        "glScissor" => arm64_host_gl(window, |host| unsafe {
            host.Scissor(
                context.regs[0] as _,
                context.regs[1] as _,
                context.regs[2] as _,
                context.regs[3] as _,
            )
        }),
        "glClearColor" => {
            let clear_color = [
                arm64_float_arg(context, 0),
                arm64_float_arg(context, 1),
                arm64_float_arg(context, 2),
                arm64_float_arg(context, 3),
            ];
            if let Some(gl) = state.arm64_gl.as_mut() {
                gl.clear_color = clear_color;
            }
            state.clear_color = clear_color;
            arm64_host_gl(window, |host| unsafe {
                host.ClearColor(
                    arm64_float_arg(context, 0),
                    arm64_float_arg(context, 1),
                    arm64_float_arg(context, 2),
                    arm64_float_arg(context, 3),
                )
            });
        }
        "glClear" => {
            if let Some(gl) = state.arm64_gl.as_mut() {
                gl.draw_calls = gl.draw_calls.saturating_add(1);
            }
            let host_error =
                arm64_host_gl_with_error(window, state, symbol, guest_pc, |host| unsafe {
                    host.Clear(context.regs[0] as _)
                });
            state.trace_render_event(format!("frame={} callback_pc={:#x} gl_pc={:#x} op={} mask={:#x} host_error={:#x} continuation={:#x}", state.render_diagnostics.display_link_callbacks, state.render_diagnostics.last_callback_pc, guest_pc, symbol, context.regs[0], host_error, context.regs[30]));
        }
        "glClearStencil" => arm64_host_gl(window, |host| unsafe {
            host.ClearStencil(context.regs[0] as _)
        }),
        "glEnable" => arm64_host_gl(window, |host| unsafe { host.Enable(context.regs[0] as _) }),
        "glDisable" => arm64_host_gl(window, |host| unsafe { host.Disable(context.regs[0] as _) }),
        "glBlendFunc" => arm64_host_gl(window, |host| unsafe {
            host.BlendFunc(context.regs[0] as _, context.regs[1] as _)
        }),
        "glColorMask" => arm64_host_gl(window, |host| unsafe {
            host.ColorMask(
                context.regs[0] as _,
                context.regs[1] as _,
                context.regs[2] as _,
                context.regs[3] as _,
            )
        }),
        "glCullFace" => arm64_host_gl(window, |host| unsafe {
            host.CullFace(context.regs[0] as _)
        }),
        "glDepthFunc" => arm64_host_gl(window, |host| unsafe {
            host.DepthFunc(context.regs[0] as _)
        }),
        "glDepthMask" => arm64_host_gl(window, |host| unsafe {
            host.DepthMask(context.regs[0] as _)
        }),
        "glDepthRangef" => arm64_host_gl(window, |host| unsafe {
            host.DepthRangef(arm64_float_arg(context, 0), arm64_float_arg(context, 1))
        }),
        "glPolygonOffset" => arm64_host_gl(window, |host| unsafe {
            host.PolygonOffset(arm64_float_arg(context, 0), arm64_float_arg(context, 1))
        }),
        "glStencilFunc" => arm64_host_gl(window, |host| unsafe {
            host.StencilFunc(
                context.regs[0] as _,
                context.regs[1] as _,
                context.regs[2] as _,
            )
        }),
        "glStencilMask" => arm64_host_gl(window, |host| unsafe {
            host.StencilMask(context.regs[0] as _)
        }),
        "glStencilOp" => arm64_host_gl(window, |host| unsafe {
            host.StencilOp(
                context.regs[0] as _,
                context.regs[1] as _,
                context.regs[2] as _,
            )
        }),
        "glActiveTexture" => arm64_host_gl(window, |host| unsafe {
            host.ActiveTexture(context.regs[0] as _)
        }),
        "glBindTexture" => arm64_host_gl(window, |host| unsafe {
            host.BindTexture(context.regs[0] as _, context.regs[1] as _)
        }),
        "glTexParameteri" => arm64_host_gl(window, |host| unsafe {
            host.TexParameteri(
                context.regs[0] as _,
                context.regs[1] as _,
                context.regs[2] as _,
            )
        }),
        "glGenBuffers" | "glGenTextures" => {
            let count = context.regs[0] as i32;
            let output = context.regs[1];
            let pointer = arm64_mut_ptr(
                mem,
                output,
                u64::try_from(count.max(0)).unwrap_or(0).saturating_mul(4),
            )?;
            arm64_host_gl(window, |host| unsafe {
                if symbol == "glGenBuffers" {
                    host.GenBuffers(count, pointer.cast())
                } else {
                    host.GenTextures(count, pointer.cast())
                }
            });
        }
        "glDeleteBuffers" | "glDeleteTextures" => {
            let count = context.regs[0] as i32;
            let input = arm64_const_ptr(
                mem,
                context.regs[1],
                u64::try_from(count.max(0)).unwrap_or(0).saturating_mul(4),
            )?;
            arm64_host_gl(window, |host| unsafe {
                if symbol == "glDeleteBuffers" {
                    host.DeleteBuffers(count, input.cast())
                } else {
                    host.DeleteTextures(count, input.cast())
                }
            });
        }
        "glBindBuffer" => {
            let target = context.regs[0] as u32;
            let buffer = context.regs[1] as u32;
            if let Some(gl) = state.arm64_gl.as_mut() {
                if target == 0x8892 {
                    gl.array_buffer_binding = buffer;
                }
                if target == 0x8893 {
                    gl.element_array_buffer_binding = buffer;
                }
            }
            arm64_host_gl(window, |host| unsafe {
                host.BindBuffer(target as _, buffer as _)
            });
        }
        "glBufferData" => {
            let size = context.regs[1];
            let data = arm64_const_ptr(mem, context.regs[2], size)?;
            arm64_host_gl(window, |host| unsafe {
                host.BufferData(context.regs[0] as _, size as _, data, context.regs[3] as _)
            });
        }
        "glCreateShader" => {
            let mut value = 0;
            arm64_host_gl(window, |host| {
                value = unsafe { host.CreateShader(context.regs[0] as _) }
            });
            return_value(context, u64::from(value));
        }
        "glCreateProgram" => {
            let mut value = 0;
            arm64_host_gl(window, |host| value = unsafe { host.CreateProgram() });
            return_value(context, u64::from(value));
        }
        "glDeleteProgram" => arm64_host_gl(window, |host| unsafe {
            host.DeleteProgram(context.regs[0] as _)
        }),
        "glCompileShader" => arm64_host_gl(window, |host| unsafe {
            host.CompileShader(context.regs[0] as _)
        }),
        "glAttachShader" => arm64_host_gl(window, |host| unsafe {
            host.AttachShader(context.regs[0] as _, context.regs[1] as _)
        }),
        "glLinkProgram" => arm64_host_gl(window, |host| unsafe {
            host.LinkProgram(context.regs[0] as _)
        }),
        "glUseProgram" => arm64_host_gl(window, |host| unsafe {
            host.UseProgram(context.regs[0] as _)
        }),
        "glGetAttribLocation" | "glGetUniformLocation" => {
            let name = arm64_cstring(mem, context.regs[1])?;
            let mut value = -1;
            arm64_host_gl(window, |host| unsafe {
                value = if symbol == "glGetAttribLocation" {
                    host.GetAttribLocation(context.regs[0] as _, name.as_ptr())
                } else {
                    host.GetUniformLocation(context.regs[0] as _, name.as_ptr())
                }
            });
            return_value(context, value as i64 as u64);
            return Ok(true);
        }
        "glShaderSource" => {
            let count = context.regs[1] as i32;
            let mut sources = Vec::new();
            let mut pointers = Vec::new();
            for index in 0..count.max(0) {
                let pointer = mem
                    .read_u64(context.regs[2] + index as u64 * 8)
                    .map_err(str::to_owned)?;
                let length = if context.regs[3] == 0 {
                    -1
                } else {
                    mem.read_u32(context.regs[3] + index as u64 * 4)
                        .map_err(str::to_owned)? as i32
                };
                let bytes = if length < 0 {
                    arm64_cstring(mem, pointer)?.into_bytes()
                } else {
                    mem.read_bytes(pointer, length as u64)
                        .map_err(str::to_owned)?
                };
                sources.push(
                    std::ffi::CString::new(bytes)
                        .map_err(|_| "ARM64 shader source contains an embedded NUL".to_owned())?,
                );
                pointers.push(sources.last().unwrap().as_ptr());
            }
            arm64_host_gl(window, |host| unsafe {
                host.ShaderSource(
                    context.regs[0] as _,
                    count,
                    pointers.as_ptr(),
                    std::ptr::null(),
                )
            });
        }
        "glGetShaderiv" | "glGetProgramiv" => {
            let pointer = arm64_mut_ptr(mem, context.regs[2], 4)?;
            arm64_host_gl(window, |host| unsafe {
                if symbol == "glGetShaderiv" {
                    host.GetShaderiv(context.regs[0] as _, context.regs[1] as _, pointer.cast())
                } else {
                    host.GetProgramiv(context.regs[0] as _, context.regs[1] as _, pointer.cast())
                }
            });
        }
        "glEnableVertexAttribArray" => arm64_host_gl(window, |host| unsafe {
            host.EnableVertexAttribArray(context.regs[0] as _)
        }),
        "glUniform2iv" | "glUniform3iv" | "glUniform4iv" => {
            let components = match symbol {
                "glUniform2iv" => 2,
                "glUniform3iv" => 3,
                _ => 4,
            };
            let pointer = arm64_const_ptr(
                mem,
                context.regs[2],
                context.regs[1].saturating_mul(components * 4),
            )?;
            arm64_host_gl(window, |host| unsafe {
                match symbol {
                    "glUniform2iv" => {
                        host.Uniform2iv(context.regs[0] as _, context.regs[1] as _, pointer.cast())
                    }
                    "glUniform3iv" => {
                        host.Uniform3iv(context.regs[0] as _, context.regs[1] as _, pointer.cast())
                    }
                    _ => {
                        host.Uniform4iv(context.regs[0] as _, context.regs[1] as _, pointer.cast())
                    }
                }
            });
        }
        "glVertexAttribPointer" => {
            let array_buffer_binding = state
                .arm64_gl
                .as_ref()
                .map_or(0, |gl| gl.array_buffer_binding);
            let pointer = if array_buffer_binding == 0 {
                arm64_const_ptr(mem, context.regs[5], 1)?
            } else {
                context.regs[5] as usize as *const std::ffi::c_void
            };
            arm64_host_gl(window, |host| unsafe {
                host.VertexAttribPointer(
                    context.regs[0] as _,
                    context.regs[1] as _,
                    context.regs[2] as _,
                    context.regs[3] as _,
                    context.regs[4] as _,
                    pointer,
                )
            });
        }
        "glUniform1fv" | "glUniform2fv" | "glUniform3fv" | "glUniform4fv" => {
            let components = match symbol {
                "glUniform1fv" => 1,
                "glUniform2fv" => 2,
                "glUniform3fv" => 3,
                _ => 4,
            };
            let pointer = arm64_const_ptr(
                mem,
                context.regs[2],
                context.regs[1].saturating_mul(components * 4),
            )?;
            arm64_host_gl(window, |host| unsafe {
                match symbol {
                    "glUniform1fv" => {
                        host.Uniform1fv(context.regs[0] as _, context.regs[1] as _, pointer.cast())
                    }
                    "glUniform2fv" => {
                        host.Uniform2fv(context.regs[0] as _, context.regs[1] as _, pointer.cast())
                    }
                    "glUniform3fv" => {
                        host.Uniform3fv(context.regs[0] as _, context.regs[1] as _, pointer.cast())
                    }
                    _ => {
                        host.Uniform4fv(context.regs[0] as _, context.regs[1] as _, pointer.cast())
                    }
                }
            });
        }
        "glUniform1iv" => {
            let pointer = arm64_const_ptr(mem, context.regs[2], context.regs[1].saturating_mul(4))?;
            arm64_host_gl(window, |host| unsafe {
                host.Uniform1iv(context.regs[0] as _, context.regs[1] as _, pointer.cast())
            });
        }
        "glUniformMatrix2fv" | "glUniformMatrix3fv" => {
            let components = if symbol == "glUniformMatrix2fv" { 4 } else { 9 };
            let pointer = arm64_const_ptr(
                mem,
                context.regs[3],
                context.regs[1].saturating_mul(components * 4),
            )?;
            arm64_host_gl(window, |host| unsafe {
                if symbol == "glUniformMatrix2fv" {
                    host.UniformMatrix2fv(
                        context.regs[0] as _,
                        context.regs[1] as _,
                        context.regs[2] as _,
                        pointer.cast(),
                    )
                } else {
                    host.UniformMatrix3fv(
                        context.regs[0] as _,
                        context.regs[1] as _,
                        context.regs[2] as _,
                        pointer.cast(),
                    )
                }
            });
        }
        "glUniformMatrix4fv" => {
            let pointer =
                arm64_const_ptr(mem, context.regs[3], context.regs[1].saturating_mul(64))?;
            arm64_host_gl(window, |host| unsafe {
                host.UniformMatrix4fv(
                    context.regs[0] as _,
                    context.regs[1] as _,
                    context.regs[2] as _,
                    pointer.cast(),
                )
            });
        }
        "glDrawArrays" => {
            if let Some(gl) = state.arm64_gl.as_mut() {
                gl.draw_calls = gl.draw_calls.saturating_add(1);
            }
            arm64_host_gl(window, |host| unsafe {
                host.DrawArrays(
                    context.regs[0] as _,
                    context.regs[1] as _,
                    context.regs[2] as _,
                )
            });
        }
        "glDrawElements" => {
            if let Some(gl) = state.arm64_gl.as_mut() {
                gl.draw_calls = gl.draw_calls.saturating_add(1);
            }
            let element_array_buffer_binding = state
                .arm64_gl
                .as_ref()
                .map_or(0, |gl| gl.element_array_buffer_binding);
            let pointer = if element_array_buffer_binding == 0 {
                let bytes = context.regs[1].saturating_mul(if context.regs[2] as u32 == 0x1403 {
                    2
                } else {
                    1
                });
                arm64_const_ptr(mem, context.regs[3], bytes)?
            } else {
                context.regs[3] as usize as *const std::ffi::c_void
            };
            arm64_host_gl(window, |host| unsafe {
                host.DrawElements(
                    context.regs[0] as _,
                    context.regs[1] as _,
                    context.regs[2] as _,
                    pointer,
                )
            });
        }
        "glTexImage2D" => {
            let bytes = arm64_texture_data_size(
                context.regs[3] as u32,
                context.regs[4] as u32,
                context.regs[6] as u32,
                context.regs[7] as u32,
            );
            let pointer = arm64_const_ptr(mem, context.regs[8], bytes)?;
            arm64_host_gl(window, |host| unsafe {
                host.TexImage2D(
                    context.regs[0] as _,
                    context.regs[1] as _,
                    context.regs[2] as _,
                    context.regs[3] as _,
                    context.regs[4] as _,
                    context.regs[5] as _,
                    context.regs[6] as _,
                    context.regs[7] as _,
                    pointer,
                )
            });
        }
        "glTexSubImage2D" => {
            let bytes = arm64_texture_data_size(
                context.regs[4] as u32,
                context.regs[5] as u32,
                context.regs[6] as u32,
                context.regs[7] as u32,
            );
            let pointer = arm64_const_ptr(mem, context.regs[8], bytes)?;
            arm64_host_gl(window, |host| unsafe {
                host.TexSubImage2D(
                    context.regs[0] as _,
                    context.regs[1] as _,
                    context.regs[2] as _,
                    context.regs[3] as _,
                    context.regs[4] as _,
                    context.regs[5] as _,
                    context.regs[6] as _,
                    context.regs[7] as _,
                    pointer,
                )
            });
        }
        "glCompressedTexImage2D" => {
            let pointer = arm64_const_ptr(mem, context.regs[7], context.regs[6])?;
            arm64_host_gl(window, |host| unsafe {
                host.CompressedTexImage2D(
                    context.regs[0] as _,
                    context.regs[1] as _,
                    context.regs[2] as _,
                    context.regs[3] as _,
                    context.regs[4] as _,
                    context.regs[5] as _,
                    context.regs[6] as _,
                    pointer,
                )
            });
        }
        "glReadPixels" => {
            let bytes = arm64_texture_data_size(
                context.regs[2] as u32,
                context.regs[3] as u32,
                context.regs[4] as u32,
                context.regs[5] as u32,
            );
            let pointer = arm64_mut_ptr(mem, context.regs[6], bytes)?;
            arm64_host_gl(window, |host| unsafe {
                host.ReadPixels(
                    context.regs[0] as _,
                    context.regs[1] as _,
                    context.regs[2] as _,
                    context.regs[3] as _,
                    context.regs[4] as _,
                    context.regs[5] as _,
                    pointer,
                )
            });
        }
        "glReleaseShaderCompiler"
        | "glGetActiveAttrib"
        | "glGetActiveUniform"
        | "glGetShaderPrecisionFormat" => {}
        "glGetProgramInfoLog" | "glGetShaderInfoLog" => {
            let program = context.regs[0] as u32;
            let buffer = arm64_mut_ptr(mem, context.regs[2], 1024)?;
            let length = context.regs[3] as i32;
            arm64_host_gl(window, |host| unsafe {
                if symbol == "glGetProgramInfoLog" {
                    host.GetProgramInfoLog(program, length, std::ptr::null_mut(), buffer.cast());
                } else {
                    host.GetShaderInfoLog(program, length, std::ptr::null_mut(), buffer.cast());
                }
            });
            return_value(context, 0);
            return Ok(true);
        }
        "glDiscardFramebufferEXT" => {}
        "glDeleteShader" => arm64_host_gl(window, |host| unsafe {
            host.DeleteShader(context.regs[0] as _)
        }),
        "glFramebufferTexture2D" | "glFramebufferTexture2DOES" => {
            let target = context.regs[0] as _;
            let attachment = context.regs[1] as _;
            let texture_target = context.regs[2] as _;
            let texture = context.regs[3] as _;
            let level = context.regs[4] as i32;
            arm64_host_gl(window, |host| unsafe {
                if symbol.ends_with("OES") {
                    host.FramebufferTexture2DOES(target, attachment, texture_target, texture, level)
                } else {
                    host.FramebufferTexture2D(target, attachment, texture_target, texture, level)
                }
            });
        }
        _ => {}
    }
    return_value(context, 0);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> RuntimeState {
        RuntimeState::new(
            (8, 0, 0),
            A64GraphicsBackend::OpenGLESCompatibility,
            DeviceFamily::iPhone5s,
            DeviceOrientation::Portrait,
        )
    }

    #[test]
    fn light_math_dispatches_double_and_single_precision_values() {
        let mut memory = Mem64::new();
        let mut runtime_state = state();
        let mut context = touchHLE_DynarmicA64Context::default();
        context.vectors[0][0] = (0.5_f64).to_bits();
        assert!(dispatch(&mut memory, &mut context, "_sin", &mut runtime_state, None).unwrap());
        assert!((f64::from_bits(context.vectors[0][0]) - 0.5_f64.sin()).abs() < f64::EPSILON);
        context.vectors[0][0] = (0.5_f32).to_bits() as u64;
        assert!(dispatch(
            &mut memory,
            &mut context,
            "_sqrtf",
            &mut runtime_state,
            None
        )
        .unwrap());
        assert_eq!(f32::from_bits(context.vectors[0][0] as u32), 0.5_f32.sqrt());
    }

    #[test]
    fn light_math_symbols_are_classified_without_callback_counting() {
        assert!(is_light_host_call("_sin"));
        assert!(is_light_host_call("__sqrt"));
        assert!(!is_light_host_call("_malloc"));
    }

    #[test]
    fn filesystem_and_signal_stubs_return_success() {
        let mut memory = Mem64::new();
        let mut runtime_state = state();
        let mut context = touchHLE_DynarmicA64Context::default();
        assert!(dispatch(
            &mut memory,
            &mut context,
            "_access",
            &mut runtime_state,
            None
        )
        .unwrap());
        assert_eq!(context.regs[0], 0);
        assert!(dispatch(
            &mut memory,
            &mut context,
            "_mkdir",
            &mut runtime_state,
            None
        )
        .unwrap());
        assert_eq!(context.regs[0], 0);
        context.regs[0] = 2;
        context.regs[1] = 0x1234;
        assert!(dispatch(
            &mut memory,
            &mut context,
            "_signal",
            &mut runtime_state,
            None
        )
        .unwrap());
        assert_eq!(context.regs[0], 0);
        context.regs[0] = 2;
        assert!(dispatch(
            &mut memory,
            &mut context,
            "_signal",
            &mut runtime_state,
            None
        )
        .unwrap());
        assert_eq!(context.regs[0], 0x1234);
    }

    #[test]
    fn arm64_memory_backed_foundation_objects_preserve_values() {
        let mut memory = Mem64::new();
        let mut runtime_state = state();
        let mut context = touchHLE_DynarmicA64Context::default();
        let data_class = objc_class_for_name(&mut memory, &mut runtime_state, "NSData").unwrap();
        let source = memory.alloc_zeroed(4).unwrap();
        memory.write_bytes(source, &[1, 2, 3, 4]).unwrap();
        context.regs[0] = data_class;
        context.regs[1] = selector_pointer(&mut memory, "dataWithBytes:length:").unwrap();
        context.regs[2] = source;
        context.regs[3] = 4;
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        let data = context.regs[0];
        assert_eq!(objc_field(&memory, data, 64), 4);
        assert_eq!(
            memory.read_bytes(objc_field(&memory, data, 56), 4).unwrap(),
            [1, 2, 3, 4]
        );

        let array_class = objc_class_for_name(&mut memory, &mut runtime_state, "NSArray").unwrap();
        let first = objc_string(&mut memory, "first").unwrap();
        let second = objc_string(&mut memory, "second").unwrap();
        let elements = memory.alloc_zeroed(16).unwrap();
        memory.write_u64(elements, first).unwrap();
        memory.write_u64(elements + 8, second).unwrap();
        context.regs[0] = array_class;
        context.regs[1] = selector_pointer(&mut memory, "arrayWithObjects:count:").unwrap();
        context.regs[2] = elements;
        context.regs[3] = 2;
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        let array = context.regs[0];
        context.regs[0] = array;
        context.regs[1] = selector_pointer(&mut memory, "count").unwrap();
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        assert_eq!(context.regs[0], 2);
        context.regs[0] = array;
        context.regs[1] = selector_pointer(&mut memory, "objectAtIndex:").unwrap();
        context.regs[2] = 1;
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        assert_eq!(context.regs[0], second);

        let dictionary_class =
            objc_class_for_name(&mut memory, &mut runtime_state, "NSDictionary").unwrap();
        let key = objc_string(&mut memory, "key").unwrap();
        context.regs[0] = dictionary_class;
        context.regs[1] = selector_pointer(&mut memory, "dictionaryWithObject:forKey:").unwrap();
        context.regs[2] = first;
        context.regs[3] = key;
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        let dictionary = context.regs[0];
        context.regs[0] = dictionary;
        context.regs[1] = selector_pointer(&mut memory, "objectForKey:").unwrap();
        context.regs[2] = key;
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        assert_eq!(context.regs[0], first);

        let number_class =
            objc_class_for_name(&mut memory, &mut runtime_state, "NSNumber").unwrap();
        context.regs[0] = number_class;
        context.regs[1] = selector_pointer(&mut memory, "numberWithInt:").unwrap();
        context.regs[2] = 42;
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        let number = context.regs[0];
        context.regs[0] = number;
        context.regs[1] = selector_pointer(&mut memory, "intValue").unwrap();
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        assert_eq!(context.regs[0], 42);
    }

    #[test]
    fn arm64_foundation_file_loading_selectors_are_handled() {
        let mut memory = Mem64::new();
        let mut runtime_state = state();
        let mut context = touchHLE_DynarmicA64Context::default();

        let ns_string_class =
            objc_class_for_name(&mut memory, &mut runtime_state, "NSString").unwrap();
        let ns_data_class = objc_class_for_name(&mut memory, &mut runtime_state, "NSData").unwrap();
        let input = memory.alloc_zeroed(6).unwrap();
        memory.write_bytes(input, b"asset").unwrap();

        context.regs[0] = ns_string_class;
        context.regs[1] = selector_pointer(&mut memory, "stringWithUTF8String:").unwrap();
        context.regs[2] = input;
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        let string = context.regs[0];
        assert_eq!(objc_text(&memory, string).as_deref(), Some(&b"asset"[..]));

        let receiver =
            objc_instance_for_class(&mut memory, &mut runtime_state, "NSString").unwrap();
        context.regs[0] = receiver;
        context.regs[1] = selector_pointer(&mut memory, "initWithUTF8String:").unwrap();
        context.regs[2] = input;
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        assert_eq!(context.regs[0], receiver);
        assert_eq!(objc_text(&memory, receiver).as_deref(), Some(&b"asset"[..]));

        let other = objc_string(&mut memory, "other").unwrap();
        context.regs[0] = string;
        context.regs[1] = selector_pointer(&mut memory, "compare:options:").unwrap();
        context.regs[2] = other;
        context.regs[3] = 0;
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        assert_eq!(context.regs[0], (-1_i64) as u64);

        context.regs[0] = ns_data_class;
        context.regs[1] = selector_pointer(&mut memory, "dataWithContentsOfFile:").unwrap();
        context.regs[2] = string;
        objc_send(&mut memory, &mut context, &mut runtime_state).unwrap();
        let data = context.regs[0];
        assert_eq!(objc_kind(&memory, data), Some(A64_KIND_DATA));
        assert_eq!(objc_field(&memory, data, 64), 0);
    }
}
