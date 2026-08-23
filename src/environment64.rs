use crate::a64_runtime::{dispatch, materialize_import, schedule_display_link_callback, A64GraphicsBackend, LoadedImage, RuntimeState};
use crate::bundle::Bundle;
use crate::cpu::A64Cpu;
use crate::fs::Fs;
use crate::mach_o64::MachO64;
use crate::mem64::Mem64;
use crate::options::Options;
use crate::window::DeviceFamily;
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};
use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

const STACK_BASE: u64 = 0x7fff_ffff_0000;
const STACK_SIZE: u64 = 0x0010_0000;
const SVC_THREAD_EXIT: u32 = 1;
const SVC_RETURN_TO_HOST: u32 = 2;
const SVC_HOST_BASE: u32 = 0x100;
const HOST_STUB_SIZE: u64 = 8;
const MAX_HOST_DISPATCHES_PER_CALLBACK: u64 = 100_000;
const A64_HALT_USER_DEFINED1: u32 = 0x0100_0000;
const A64_HALT_USER_DEFINED2: u32 = 0x0200_0000;
const A64_HALT_USER_DEFINED3: u32 = 0x0400_0000;
const STARTUP_TRACE_INSTRUCTIONS: u64 = 10_000;
const STALL_THRESHOLD: u64 = 512;
const EXECUTION_SLICE_TICKS: u64 = 1_000;
const ARM64_BOOTSTRAP_GRACE_SLICES: u32 = 8;

fn sign_extend(value: u64, bits: u32) -> i64 {
    let shift = 64 - bits;
    ((value << shift) as i64) >> shift
}

fn branch_target(instruction: u32, pc: u64) -> Option<u64> {
    let instruction = u64::from(instruction);
    let immediate = if instruction & 0xfc00_0000 == 0x1400_0000 || instruction & 0xfc00_0000 == 0x9400_0000 {
        sign_extend(instruction & 0x03ff_ffff, 26) << 2
    } else if instruction & 0x7e00_0000 == 0x3400_0000 {
        sign_extend((instruction >> 5) & 0x7f_ffff, 19) << 2
    } else {
        return None;
    };
    pc.checked_add_signed(immediate)
}

fn host_call_continuation(context: &touchHLE_DynarmicA64Context) -> u64 {
    context.regs[30]
}

fn host_call_identity(context: &touchHLE_DynarmicA64Context) -> (u64, u64) {
    (context.pc, context.regs[30])
}

fn host_call_site(context: &touchHLE_DynarmicA64Context) -> u64 {
    context.regs[30].saturating_sub(4)
}

fn callback_event_context(context: &touchHLE_DynarmicA64Context) -> String {
    format!(
        "pc={:#x} lr={:#x} sp={:#x} fp={:#x} x0={:#x} x1={:#x} x2={:#x} x3={:#x} x4={:#x} x5={:#x} x6={:#x} x7={:#x}",
        context.pc, context.regs[30], context.sp, context.regs[29], context.regs[0],
        context.regs[1], context.regs[2], context.regs[3], context.regs[4], context.regs[5],
        context.regs[6], context.regs[7],
    )
}

fn decode_instruction(instruction: u32, pc: u64) -> String {
    if instruction == 0xd65f_03c0 {
        "ret".to_string()
    } else if instruction & 0xfc00_0000 == 0x1400_0000 {
        format!("b {:#x}", branch_target(instruction, pc).unwrap_or(0))
    } else if instruction & 0xfc00_0000 == 0x9400_0000 {
        format!("bl {:#x}", branch_target(instruction, pc).unwrap_or(0))
    } else if instruction & 0x7e00_0000 == 0x3400_0000 {
        format!("cbz/cbnz {:#x}", branch_target(instruction, pc).unwrap_or(0))
    } else if (instruction & 0x1fe0_0000 == 0x1a80_0000
        || instruction & 0x1fe0_0000 == 0x5a80_0000
        || instruction & 0x1fe0_0000 == 0x1ac0_0000)
        && instruction & 0x0000_0810 == 0
    {
        let mnemonic = if instruction & 0x4000_0000 != 0 {
            if instruction & 0x0000_0400 != 0 { "csneg" } else { "csinv" }
        } else if instruction & 0x0000_0400 != 0 {
            "csinc"
        } else {
            "csel"
        };
        let condition = (instruction >> 12) & 0xf;
        format!("{} cond={:#x} rn={} rm={} rd={}", mnemonic, condition, (instruction >> 5) & 31, (instruction >> 16) & 31, instruction & 31)
    } else if instruction & 0xffff_fc1f == 0xd61f_0000 {
        "br/blr".to_string()
    } else {
        format!(".word {instruction:#010x}")
    }
}

fn register_dump(context: &touchHLE_DynarmicA64Context) -> String {
    context.regs.iter().enumerate().map(|(index, value)| format!("x{index}={value:#018x}")).collect::<Vec<_>>().join(" ")
}

fn vector_dump(context: &touchHLE_DynarmicA64Context) -> String {
    context.vectors.iter().enumerate().map(|(index, value)| format!("v{index}={:#018x}{:#018x}", value[1], value[0])).collect::<Vec<_>>().join(" ")
}

fn processor_state_dump(context: &touchHLE_DynarmicA64Context) -> String {
    format!("pstate={:#010x} fpcr={:#010x} fpsr={:#010x} {}", context.pstate, context.fpcr, context.fpsr, vector_dump(context))
}

fn stack_dump(memory: &Mem64, sp: u64) -> String {
    let start = sp.saturating_sub(64);
    match memory.read_bytes(start, 128) {
        Ok(bytes) => bytes.chunks(16).enumerate().map(|(index, chunk)| format!("{:#x}: {}", start + index as u64 * 16, chunk.iter().map(|byte| format!("{byte:02x}")).collect::<Vec<_>>().join(" "))).collect::<Vec<_>>().join(" | "),
        Err(error) => format!("unavailable around sp={sp:#x}: {error}"),
    }
}

fn call_stack_dump(memory: &Mem64, context: &touchHLE_DynarmicA64Context) -> String {
    let mut frame = context.regs[29];
    let mut frames = Vec::new();
    for _ in 0..8 {
        if frame == 0 { break; }
        let Ok(previous) = memory.read_u64(frame) else { break; };
        let Ok(lr) = memory.read_u64(frame + 8) else { break; };
        frames.push(format!("fp={:#x} lr={:#x}", frame, lr));
        if previous <= frame || previous - frame > 0x100000 { break; }
        frame = previous;
    }
    if frames.is_empty() { "unavailable".to_string() } else { frames.join(" -> ") }
}
fn verify_abi(context: &touchHLE_DynarmicA64Context, module: &str) {
    if context.sp == 0 {
        echo!(
            "ARM64 ABI violation in {module}: SP became NULL (pc={:#x} lr={:#x} fp={:#x} x0={:#x} x1={:#x} x2={:#x} x3={:#x})",
            context.pc,
            context.regs[30],
            context.regs[29],
            context.regs[0],
            context.regs[1],
            context.regs[2],
            context.regs[3],
        );
    } else if context.sp & 15 != 0 {
        echo!("ARM64 ABI violation in {module}: SP is not 16-byte aligned: {:#x}", context.sp);
    }
    if context.regs[8] != 0 {
        log_dbg!("ARM64 ABI indirect-result register x8={:#x} in {module}", context.regs[8]);
    }
}

fn verify_guest_mappings(memory: &Mem64, pc: u64, sp: u64) {
    let pc_mapped = memory
        .mapped_regions()
        .any(|region| pc >= region.base && pc.saturating_sub(region.base) < region.size);
    let stack_store = sp.saturating_sub(0x30);
    let stack_store_end = stack_store.saturating_add(16);
    let stack_store_mapped = memory
        .mapped_regions()
        .any(|region| stack_store >= region.base && stack_store_end <= region.base.saturating_add(region.size));
    echo!(
        "ARM64 guest mappings: entry_pc={:#x} executable_page_mapped={} stack_sp={:#x} writable_stack_mapping={} stp_range={:#x}..{:#x} stp_range_mapped={}",
        pc,
        pc_mapped,
        sp,
        sp >= STACK_BASE - STACK_SIZE && sp <= STACK_BASE,
        stack_store,
        stack_store_end,
        stack_store_mapped,
    );
}

fn mapping_dump(memory: &Mem64) -> String {
    let regions = memory.mapped_regions().collect::<Vec<_>>();
    let total_bytes = regions.iter().map(|region| region.size).sum::<u64>();
    format!("{} mapped regions, {} total bytes", regions.len(), total_bytes)
}

fn display_boot_screen(
    bundle: &Bundle,
    fs: &Fs,
    device_family: DeviceFamily,
    window: &mut crate::window::Window,
) -> bool {
    let launch_image_path = bundle.launch_image_path(fs, device_family);
    if let Ok(bytes) = fs.read(&launch_image_path) {
        if let Ok(image) = crate::image::Image::from_bytes(&bytes) {
            window.display_compatibility_image(
                image,
                crate::window::DeviceOrientation::Portrait,
            );
            echo!("ARM64 boot screen reached: displaying {}", launch_image_path.as_str());
            return true;
        }
    }
    match bundle.load_icon(fs) {
        Ok(image) => {
            window.display_compatibility_image(
                image,
                crate::window::DeviceOrientation::Portrait,
            );
            echo!("ARM64 boot/logo fallback: displaying the app icon");
            true
        }
        Err(error) => {
            log!("ARM64 boot screen: no usable launch image or icon: {}", error);
            false
        }
    }
}

fn run_arm64_application_lifecycle(
    window: &mut Option<Box<crate::window::Window>>,
    options: &Options,
) {
    if window.is_none() {
        echo!("ARM64 application lifecycle: headless mode has no host event loop; returning after bootstrap");
        return;
    }

    echo!("ARM64 application lifecycle: guest entry returned after UIApplicationMain; entering the host run loop");
    loop {
        let Some(window) = window.as_mut() else {
            return;
        };
        window.poll_for_events(options);
        while let Some(event) = window.pop_event() {
            match event {
                crate::window::Event::Quit | crate::window::Event::AppWillTerminate => {
                    echo!("ARM64 application lifecycle: termination event received");
                    return;
                }
                crate::window::Event::AppWillResignActive
                | crate::window::Event::AppDidEnterBackground
                | crate::window::Event::AppWillEnterForeground
                | crate::window::Event::AppDidBecomeActive
                | crate::window::Event::TouchesDown(_)
                | crate::window::Event::TouchesMove(_)
                | crate::window::Event::TouchesUp(_)
                | crate::window::Event::EnterDebugger
                | crate::window::Event::TextInput(_) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn failure_diagnostics(
    memory: &Mem64,
    context: &touchHLE_DynarmicA64Context,
    previous_pcs: &VecDeque<u64>,
    previous_branches: &VecDeque<(u64, u64)>,
    runtime_state: &RuntimeState,
    reason: &str,
) {
    let instruction = memory.read_u32(context.pc).unwrap_or(0);
    echo!("ARM64 failure diagnostics: reason={} pc={:#x} instruction={:#010x} decoded={}", reason, context.pc, instruction, decode_instruction(instruction, context.pc));
    echo!("ARM64 failure registers: {}", register_dump(context));
    echo!("ARM64 failure processor state: {}", processor_state_dump(context));
    echo!("ARM64 failure previous_pcs={:?} branch_history={:?}", previous_pcs, previous_branches);
    echo!("ARM64 failure stack: {}", stack_dump(memory, context.sp));
    echo!("ARM64 failure call stack: {}", call_stack_dump(memory, context));
    let previous_instruction = context.pc.checked_sub(4).and_then(|pc| memory.read_u32(pc).ok());
    let next_instruction = context.pc.checked_add(4).and_then(|pc| memory.read_u32(pc).ok());
    echo!(
        "ARM64 failure instruction window: prev_pc={:#x} prev={:#010x} current_pc={:#x} current={:#010x} next_pc={:#x} next={:#010x} image_offset={:#x}",
        context.pc.saturating_sub(4),
        previous_instruction.unwrap_or(0),
        context.pc,
        instruction,
        context.pc.saturating_add(4),
        next_instruction.unwrap_or(0),
        context.pc.saturating_sub(0x1_0000_0000),
    );
    echo!("ARM64 failure mappings: {}", mapping_dump(memory));
    echo!(
        "ARM64 failure dispatch: receiver={:#x} selector={} callback_target={:#x} dispatch_pc={:#x} dispatch_lr={:#x} dispatch_sp={:#x} current_pc={:#x} current_lr={:#x} current_sp={:#x}",
        runtime_state.render_diagnostics.last_dispatch_receiver,
        runtime_state.render_diagnostics.last_dispatch_selector.as_deref().unwrap_or("<none>"),
        runtime_state.render_diagnostics.last_dispatch_callback_target,
        runtime_state.render_diagnostics.last_dispatch_pc,
        runtime_state.render_diagnostics.last_dispatch_lr,
        runtime_state.render_diagnostics.last_dispatch_sp,
        context.pc,
        context.regs[30],
        context.sp,
    );
    echo!("ARM64 failure state: module={} last_symbol={} last_callback={} selector={} dispatches={} objc={} metal={} unresolved_reached={}", runtime_state.current_module.as_deref().unwrap_or("<unknown>"), runtime_state.last_symbol.as_deref().unwrap_or("<none>"), runtime_state.last_successful_symbol.as_deref().unwrap_or("<none>"), runtime_state.last_selector.as_deref().unwrap_or("<none>"), runtime_state.host_dispatches, runtime_state.objc_messages, runtime_state.metal_commands, runtime_state.reached_unimplemented_symbols.len());
}

fn put_string(mem: &mut Mem64, cursor: &mut u64, value: &str) -> Result<u64, String> {
    let bytes = value.as_bytes();
    *cursor = cursor.checked_sub(bytes.len() as u64 + 1).ok_or("ARM64 stack overflow")?;
    mem.write_bytes(*cursor, bytes).map_err(str::to_owned)?;
    mem.write_u8(*cursor + bytes.len() as u64, 0).map_err(str::to_owned)?;
    Ok(*cursor)
}

fn prepare_stack(
    mem: &mut Mem64,
    argv: &[String],
    envp: &[String],
    apple: &[String],
) -> Result<(u64, u64, u64, u64), String> {
    mem.map_zeroed_with_permissions(STACK_BASE - STACK_SIZE, STACK_SIZE, crate::mem64::Permissions::read_write()).map_err(str::to_owned)?;
    let mut string_cursor = STACK_BASE & !15;
    let mut argv_strings = Vec::with_capacity(argv.len());
    let mut envp_strings = Vec::with_capacity(envp.len());
    let mut apple_strings = Vec::with_capacity(apple.len());
    for value in argv.iter().rev() {
        argv_strings.push(put_string(mem, &mut string_cursor, value)?);
    }
    for value in envp.iter().rev() {
        envp_strings.push(put_string(mem, &mut string_cursor, value)?);
    }
    for value in apple.iter().rev() {
        apple_strings.push(put_string(mem, &mut string_cursor, value)?);
    }
    argv_strings.reverse();
    envp_strings.reverse();
    apple_strings.reverse();
    let pointer_count = argv.len() + envp.len() + apple.len() + 4;
    let pointer_bytes = (pointer_count as u64)
        .checked_mul(8)
        .ok_or("ARM64 startup stack is too large")?;
    let sp = (string_cursor & !15)
        .checked_sub(pointer_bytes)
        .ok_or("ARM64 stack overflow")?
        & !15;
    let argc = argv.len() as u64;
    let argv_ptr = sp + 8;
    let envp_ptr = argv_ptr + ((argv.len() + 1) as u64 * 8);
    let apple_ptr = envp_ptr + ((envp.len() + 1) as u64 * 8);
    let mut cursor = sp;
    mem.write_u64(cursor, argc).map_err(str::to_owned)?;
    cursor += 8;
    for value in &argv_strings {
        mem.write_u64(cursor, *value).map_err(str::to_owned)?;
        cursor += 8;
    }
    mem.write_u64(cursor, 0).map_err(str::to_owned)?;
    cursor += 8;
    for value in &envp_strings {
        mem.write_u64(cursor, *value).map_err(str::to_owned)?;
        cursor += 8;
    }
    mem.write_u64(cursor, 0).map_err(str::to_owned)?;
    cursor += 8;
    for value in &apple_strings {
        mem.write_u64(cursor, *value).map_err(str::to_owned)?;
        cursor += 8;
    }
    mem.write_u64(cursor, 0).map_err(str::to_owned)?;
    Ok((sp, argv_ptr, envp_ptr, apple_ptr))
}

fn write_svc_stub(mem: &mut Mem64, svc: u32) -> Result<u64, String> {
    let stub = mem.alloc_zeroed_with_permissions(HOST_STUB_SIZE, crate::mem64::Permissions::read_write_execute()).map_err(str::to_owned)?;
    let instruction = 0xd4000001u32 | ((u64::from(svc) << 5) as u32);
    mem.write_u32(stub, instruction).map_err(str::to_owned)?;
    mem.write_u32(stub + 4, 0xd65f03c0).map_err(str::to_owned)?;
    Ok(stub)
}

fn lookup_host_symbol(symbol: &str) -> Option<&'static str> {
    crate::dyld::search_host_dylibs(|dylib| dylib.function_exports, symbol)
        .map(|(name, _)| *name)
}

fn load_embedded_unity_framework(
    bundle: &Bundle,
    fs: &Fs,
    memory: &mut Mem64,
    state: &mut RuntimeState,
) -> Result<(), String> {
    let framework_path = bundle
        .bundle_path()
        .join("Frameworks/UnityFramework.framework/UnityFramework");
    if !fs.is_file(&framework_path) {
        log!(
            "ARM64 app has no embedded UnityFramework at {}; continuing with the main executable",
            framework_path.as_str()
        );
        return Ok(());
    }
    let bytes = fs
        .read(&framework_path)
        .map_err(|_| format!("Could not read {}", framework_path.as_str()))?;
    let framework = MachO64::load_from_bytes(&bytes, "UnityFramework", 0x3000_0000)?;
    let base = framework.text_base;
    let end = framework.last_segment_end;
    memory.merge_mappings(framework.memory)?;
    state.loaded_images.push(LoadedImage {
        name: "UnityFramework".to_owned(),
        base,
        end,
        entry: framework.entry_point_pc,
        exports: framework.exported_symbols,
    });
    echo!(
        "ARM64 loaded embedded UnityFramework: base={:#x} end={:#x} entry={:?}",
        base,
        end,
        framework.entry_point_pc
    );
    Ok(())
}

fn detect_graphics_backend(executable: &MachO64, requested: crate::options::GraphicsApi) -> (A64GraphicsBackend, &'static str) {
    match requested {
        crate::options::GraphicsApi::GLES10
        | crate::options::GraphicsApi::GLES11
        | crate::options::GraphicsApi::GLES20
        | crate::options::GraphicsApi::GLES30
        | crate::options::GraphicsApi::Translator
        | crate::options::GraphicsApi::TranslatorGLES30 => {
            (A64GraphicsBackend::OpenGLESCompatibility, "graphics option explicitly selects OpenGL ES compatibility")
        }
        crate::options::GraphicsApi::Metal => {
            (A64GraphicsBackend::MetalCompatibility, "graphics option explicitly selects Metal compatibility")
        }
        crate::options::GraphicsApi::Default => {
            let uses_gles = executable.dynamic_libraries.iter().any(|library| library.contains("OpenGLES"))
                || executable.bindings.iter().any(|binding| {
                    let symbol = binding.symbol.trim_start_matches('_');
                    symbol.starts_with("gl") || symbol.starts_with("EAGL")
                });
            let uses_metal = executable.dynamic_libraries.iter().any(|library| library.contains("Metal"))
                || executable.bindings.iter().any(|binding| {
                    let symbol = binding.symbol.trim_start_matches('_');
                    symbol.starts_with("MTL") || symbol == "MTLCreateSystemDefaultDevice"
                });
            if uses_gles {
                if uses_metal {
                    (A64GraphicsBackend::OpenGLESCompatibility, "application uses OpenGL ES and Metal; native Metal is incomplete, so OpenGL ES compatibility is selected")
                } else {
                    (A64GraphicsBackend::OpenGLESCompatibility, "application imports OpenGL ES; OpenGL ES compatibility is selected automatically")
                }
            } else if uses_metal {
                (A64GraphicsBackend::MetalCompatibility, "application imports Metal; Metal compatibility is selected")
            } else {
                (A64GraphicsBackend::OpenGLESCompatibility, "application graphics API is not declared; OpenGL ES compatibility is the safe ARM64 fallback")
            }
        }
    }
}

pub fn run(bundle: Bundle, fs: Fs, options: Options, app_args: Vec<String>) -> Result<(), String> {
    echo!(
        "ARM64 launch configuration: device={:?}, orientation={:?}, fullscreen={}, screen={:?}, scale={:.2}, iOS={:?}",
        options.device_family,
        options.initial_orientation,
        options.fullscreen,
        options.host_screen_size,
        options.scale_hack,
        options.ios_version.unwrap_or(crate::options::LATEST_IOS_VERSION),
    );
    let executable_path = bundle.executable_path();
    let executable = MachO64::load_from_file(&executable_path, &fs, 0)?;
    let (graphics_backend, graphics_reason) = detect_graphics_backend(&executable, options.graphics_api);
    let entry = executable.entry_point_pc.ok_or("ARM64 Mach-O has no entry point")?;
    let image_end = executable.last_segment_end;
    echo!("ARM64 image loaded: entry {:#x}, image range ends at {:#x}", entry, image_end);
    let mut memory = executable.memory;
    let argv = std::iter::once(executable_path.as_str().to_owned())
        .chain(app_args)
        .collect::<Vec<_>>();
    let apple = vec![format!("executable_path={}", executable_path.as_str())];
    let ios_version = options.ios_version.unwrap_or(crate::options::LATEST_IOS_VERSION);
    let declared = bundle.device_family_array();
    let supports_ipad = declared.iter().any(DeviceFamily::is_ipad);
    let supports_phone = declared.iter().any(|family| !family.is_ipad());
    let oldest_compatible = if supports_phone {
        DeviceFamily::oldest_arm64_for_class(false)
    } else if supports_ipad {
        DeviceFamily::oldest_arm64_for_class(true)
    } else {
        DeviceFamily::iPhone5s
    };
    let device_family = match options.device_family {
        Some(requested)
            if requested.supports_arm64()
                && ((requested.is_ipad() && supports_ipad)
                    || (!requested.is_ipad() && supports_phone)) => requested,
        Some(requested) => {
            log!(
                "ARM64 device override {} is unavailable for this app; using oldest compatible model {} ({})",
                requested,
                oldest_compatible,
                oldest_compatible.machine_name()
            );
            oldest_compatible
        }
        None => {
            log!(
                "ARM64 device family defaulted to oldest compatible model: {} ({})",
                oldest_compatible,
                oldest_compatible.machine_name()
            );
            oldest_compatible
        }
    };
    if !device_family.supports_arm64() {
        return Err(format!(
            "ARM64 app requires an ARM64-capable device model; {} is 32-bit-only",
            device_family
        ));
    }
    let orientation = if options.initial_orientation == crate::window::DeviceOrientation::Portrait
        && !bundle.supported_interface_orientations().iter().any(|orientation| *orientation == "UIInterfaceOrientationPortrait")
    {
        bundle.supported_interface_orientations().iter().find_map(|orientation| match *orientation {
            "UIInterfaceOrientationLandscapeLeft" => Some(crate::window::DeviceOrientation::LandscapeRight),
            "UIInterfaceOrientationLandscapeRight" => Some(crate::window::DeviceOrientation::LandscapeLeft),
            "UIInterfaceOrientationPortraitUpsideDown" => Some(crate::window::DeviceOrientation::PortraitUpsideDown),
            _ => None,
        }).unwrap_or(crate::window::DeviceOrientation::Portrait)
    } else {
        options.initial_orientation
    };
    let mut runtime_state = RuntimeState::new(ios_version, graphics_backend, device_family, orientation);
    runtime_state.current_module = Some(executable.name.clone());
    runtime_state.bundle_identifier = bundle.bundle_identifier().to_owned();
    runtime_state.bundle_path = bundle.bundle_path().as_str().to_owned();
    runtime_state.bundle_name = bundle.bundle_name().to_owned();
    runtime_state.main_nib_name = bundle.main_nib_filename(Some(device_family)).map(str::to_owned);
    runtime_state.objc_classes = executable.objc_classes.clone();
    echo!("ARM64 Objective-C metadata: {} guest classes loaded", runtime_state.objc_classes.len());
    load_embedded_unity_framework(&bundle, &fs, &mut memory, &mut runtime_state)?;
    let mut window = if options.headless {
        None
    } else {
        let mut window_options = options.clone();
        window_options.device_family = Some(device_family);
        window_options.host_screen_size = Some(device_family.portrait_size());
        window_options.initial_orientation = orientation;
        match graphics_backend {
            A64GraphicsBackend::OpenGLESCompatibility => {
                window_options.graphics_api = crate::options::GraphicsApi::Translator;
                window_options.prefer_gles2_context = true;
                log!("ARM64 automatic graphics fallback: using the existing GLES1→GLES2 translator; reason={graphics_reason}");
            }
            A64GraphicsBackend::MetalCompatibility => {
                window_options.graphics_api = crate::options::GraphicsApi::GLES20;
                window_options.prefer_gles2_context = true;
                log!("ARM64 Metal compatibility: using a host GLES2 display surface for the Metal presenter; reason={graphics_reason}");
            }
            A64GraphicsBackend::SoftwareCompatibility => {
                log!("ARM64 software compatibility backend selected; reason={graphics_reason}");
            }
        }
        Some(Box::new(crate::window::Window::new(
            "RadekHLE ARM64",
            None,
            None,
            &window_options,
        )))
    };
    echo!(
        "ARM64 device selected: {} ({})",
        device_family,
        device_family.machine_name()
    );
    echo!(
        "ARM64 graphics backend selected: {} (automatic selection; {})",
        graphics_backend.label(),
        graphics_reason
    );
    log_dbg!(
        "ARM64 compatibility profile: iOS {}.{}.{}; pointer size=8; stack alignment=16; bindings={}",
        ios_version.0,
        ios_version.1,
        ios_version.2,
        executable.bindings.len()
    );
    let (sp, argv_ptr, envp_ptr, apple_ptr) = prepare_stack(&mut memory, &argv, &[], &apple)?;

    let return_stub = write_svc_stub(&mut memory, SVC_RETURN_TO_HOST)?;
    let application_return_stub = write_svc_stub(&mut memory, SVC_HOST_BASE + 0x7ffd)?;
    let application_launch_return_stub = write_svc_stub(&mut memory, SVC_HOST_BASE + 0x7ffe)?;
    let application_active_return_stub = write_svc_stub(&mut memory, SVC_HOST_BASE + 0x7fff)?;
    let display_link_return_stub = write_svc_stub(&mut memory, SVC_HOST_BASE + 0x7ffc)?;
    let nib_awake_return_stub = write_svc_stub(&mut memory, SVC_HOST_BASE + 0x7ffb)?;
    let guest_method_return_stub = write_svc_stub(&mut memory, SVC_HOST_BASE + 0x7ffa)?;
    runtime_state.application_return_stub = Some(application_return_stub);
    runtime_state.application_launch_return_stub = Some(application_launch_return_stub);
    runtime_state.application_active_return_stub = Some(application_active_return_stub);
    runtime_state.nib_awake_return_stub = Some(nib_awake_return_stub);
    runtime_state.guest_method_return_stub = Some(guest_method_return_stub);
    runtime_state.display_link_return_stub = Some(display_link_return_stub);
    let mut host_stubs = HashMap::new();
    host_stubs.insert((SVC_HOST_BASE + 0x7ffc) as i32, ("ARM64_display_link_return".to_owned(), "ARM64_display_link_return"));
    host_stubs.insert((SVC_HOST_BASE + 0x7ffb) as i32, ("ARM64_nib_awake_return".to_owned(), "ARM64_nib_awake_return"));
    host_stubs.insert((SVC_HOST_BASE + 0x7ffa) as i32, ("ARM64_guest_method_return".to_owned(), "ARM64_guest_method_return"));
    host_stubs.insert((SVC_HOST_BASE + 0x7ffd) as i32, ("ARM64_application_return".to_owned(), "ARM64_application_return"));
    host_stubs.insert((SVC_HOST_BASE + 0x7ffe) as i32, ("ARM64_application_launch_return".to_owned(), "ARM64_application_launch_return"));
    host_stubs.insert((SVC_HOST_BASE + 0x7fff) as i32, ("ARM64_application_active_return".to_owned(), "ARM64_application_active_return"));
    let mut stub_by_symbol: HashMap<String, (u32, u64)> = HashMap::new();
    let mut unresolved = Vec::new();
    let mut materialized_imports = 0usize;
    for (binding_index, binding) in executable.bindings.iter().enumerate() {
        if let Some(value) = materialize_import(&mut memory, &binding.symbol)? {
            if binding_index < 32 {
                log_dbg!("ARM64 materialized import #{}: {} -> {:#x}", binding_index, binding.symbol, value);
            }
            memory.load_u64(binding.address, value.checked_add_signed(binding.addend).ok_or("ARM64 import address overflows")?).map_err(str::to_owned)?;
            materialized_imports += 1;
            continue;
        }
        let symbol = lookup_host_symbol(&binding.symbol)
            .or_else(|| lookup_host_symbol(binding.symbol.strip_prefix('_').unwrap_or(&binding.symbol)))
            .unwrap_or("<unimplemented>");
        if symbol == "<unimplemented>" && !crate::a64_runtime::can_dispatch(&binding.symbol) {
            unresolved.push(binding.symbol.clone());
        }
        let binding_key = binding.symbol.clone();
        let (_svc, stub) = if let Some(&(_svc, stub)) = stub_by_symbol.get(&binding_key) {
            (_svc, stub)
        } else {
            let svc = SVC_HOST_BASE + host_stubs.len() as u32;
            let stub = write_svc_stub(&mut memory, svc)?;
            stub_by_symbol.insert(binding_key, (svc, stub));
            host_stubs.insert(svc as i32, (binding.symbol.clone(), symbol));
            (svc, stub)
        };
        let target = stub
            .checked_add_signed(binding.addend)
            .ok_or("ARM64 import target overflows")?;
        memory.load_u64(binding.address, target).map_err(str::to_owned)?;
    }

    if !unresolved.is_empty() {
        runtime_state.unimplemented_symbols.extend(unresolved.iter().cloned());
    }
    echo!(
        "ARM64 runtime: entry point {:#x}, image_end {:#x}, {} unique host stubs for {} bindings, {} materialized imports, {} unresolved, stack {:#x}, argv {:#x}, envp {:#x}, apple {:#x}",
        entry,
        image_end,
        host_stubs.len(),
        executable.bindings.len(),
        materialized_imports,
        unresolved.len(),
        sp,
        argv_ptr,
        envp_ptr,
        apple_ptr,
    );
    if !unresolved.is_empty() {
        echo!("ARM64 unresolved imports: {} (details available with --log-debug)", unresolved.len());
        for symbol in unresolved.iter().take(8) {
            log_dbg!("ARM64 unresolved import: {}", symbol);
        }
    }
    log_dbg!("ARM64 first bindings: {}", executable.bindings.iter().take(16).enumerate().map(|(i, binding)| format!("{}:{}@{:x}+{}", i, binding.symbol, binding.address, binding.addend)).collect::<Vec<_>>().join(", "));
    let mut context = touchHLE_DynarmicA64Context::default();
    context.sp = sp;
    context.pc = entry;
    context.regs[0] = argv.len() as u64;
    context.regs[1] = argv_ptr;
    context.regs[2] = envp_ptr;
    context.regs[3] = apple_ptr;
    context.regs[30] = return_stub;
    let mut cpu = A64Cpu::with_backend(options.arm64_backend);
    cpu.set_trace(false);
    echo!("ARM64 execution transition: context loaded; entering Dynarmic with pc={:#x} sp={:#x} lr={:#x}", context.pc, context.sp, context.regs[30]);
    cpu.load_context(&context);
    let mut ticks = Some(EXECUTION_SLICE_TICKS);
    let mut host_dispatches = 0_u64;
    let mut host_dispatches_since_callback = 0_u64;
    let mut last_host_call: Option<(u64, u64)> = None;
    let mut repeated_host_call = 0_u64;
    let mut guest_progress_since_host_call = true;
    let watchdog_ms = std::env::var("TOUCHHLE_ARM64_WATCHDOG_MS").ok().and_then(|value| value.parse::<u64>().ok()).unwrap_or(2000);
    let mut no_progress_since = Instant::now();
    let mut no_progress_slices = 0_u64;
    let trace_limit = std::env::var("TOUCHHLE_ARM64_TRACE_INSTRUCTIONS").ok().and_then(|value| value.parse::<u64>().ok()).unwrap_or(0);
    let mut trace_count = 0_u64;
    let mut previous_pcs = VecDeque::with_capacity(20);
    let mut previous_branches = VecDeque::with_capacity(20);
    let mut bootstrap_grace_slices = 0u32;
    let mut bootstrap_displayed = false;
    echo!("ARM64 execution transition: normal mode uses Dynarmic Run with {}-tick slices; instruction tracing limit={}", EXECUTION_SLICE_TICKS, trace_limit);
    verify_abi(&context, "entry");
    verify_guest_mappings(&memory, context.pc, context.sp);
    loop {
        let trace_this_instruction = trace_count < trace_limit;
        let instruction_pc = context.pc;
        let instruction = if trace_this_instruction { memory.read_u32(instruction_pc).unwrap_or(0) } else { 0 };
        let result = cpu.run_or_step(
            &mut memory,
            &mut context,
            ticks.as_mut(),
        );
        cpu.save_context(&mut context);
        if result >= SVC_HOST_BASE as i32 && instruction_pc != context.pc {
            guest_progress_since_host_call = true;
        }
        runtime_state.render_diagnostics.last_guest_pc = context.pc;
        if runtime_state.render_diagnostics.callback_active && runtime_state.render_diagnostics.callback_entry_lr == 0 {
            runtime_state.render_diagnostics.callback_entry_lr = context.regs[30];
        }
        if trace_this_instruction {
            trace_count += 1;
            echo!("ARM64 run slice #{}: result={} entry_pc={:#x} final_pc={:#x} sp={:#x} lr={:#x} instruction={:#010x} decoded={}", trace_count, result, instruction_pc, context.pc, context.sp, context.regs[30], instruction, decode_instruction(instruction, instruction_pc));
        }
        if result == -1 {
            if context.pc == instruction_pc {
                no_progress_slices = no_progress_slices.saturating_add(1);
            } else {
                no_progress_slices = 0;
                no_progress_since = Instant::now();
            }
            if no_progress_slices >= STALL_THRESHOLD || no_progress_since.elapsed() >= Duration::from_millis(watchdog_ms) {
                failure_diagnostics(&memory, &context, &previous_pcs, &previous_branches, &runtime_state, "no PC progress watchdog");
                return Err(format!("ARM64 execution stalled without PC progress at {:#x}", context.pc));
            }
        }
        if trace_this_instruction {
            if previous_pcs.len() == 20 { previous_pcs.pop_front(); }
            previous_pcs.push_back(instruction_pc);
            if let Some(target) = branch_target(instruction, instruction_pc) {
                if previous_branches.len() == 20 { previous_branches.pop_front(); }
                previous_branches.push_back((instruction_pc, target));
            }
        } else {
            if previous_pcs.len() == 20 { previous_pcs.pop_front(); }
            previous_pcs.push_back(instruction_pc);
            if let Some(target) = branch_target(instruction, instruction_pc) {
                if previous_branches.len() == 20 { previous_branches.pop_front(); }
                previous_branches.push_back((instruction_pc, target));
            }
        }
        match result {
            -1 => {
                ticks = Some(EXECUTION_SLICE_TICKS);
                continue;
            }
            -2 => {
                failure_diagnostics(&memory, &context, &previous_pcs, &previous_branches, &runtime_state, "memory abort");
                return Err(format!(
                    "ARM64 guest memory fault at pc {:#x}, sp {:#x}, lr {:#x}, fp {:#x}, x0 {:#x}, x1 {:#x}, x2 {:#x}, x3 {:#x}",
                    context.pc,
                    context.sp,
                    context.regs[30],
                    context.regs[29],
                    context.regs[0],
                    context.regs[1],
                    context.regs[2],
                    context.regs[3],
                ));
            }
            -3 => {
                failure_diagnostics(&memory, &context, &previous_pcs, &previous_branches, &runtime_state, "undefined instruction");
                return Err(format!(
                    "ARM64 undefined instruction at pc {:#x}, sp {:#x}, lr {:#x}, fp {:#x}, x0 {:#x}, x1 {:#x}, x2 {:#x}, x3 {:#x}",
                    context.pc,
                    context.sp,
                    context.regs[30],
                    context.regs[29],
                    context.regs[0],
                    context.regs[1],
                    context.regs[2],
                    context.regs[3],
                ));
            }
            -4 => {
                failure_diagnostics(&memory, &context, &previous_pcs, &previous_branches, &runtime_state, "breakpoint");
                return Err(format!(
                    "ARM64 breakpoint at pc {:#x}, sp {:#x}, lr {:#x}, fp {:#x}, x0 {:#x}, x1 {:#x}, x2 {:#x}, x3 {:#x}",
                    context.pc,
                    context.sp,
                    context.regs[30],
                    context.regs[29],
                    context.regs[0],
                    context.regs[1],
                    context.regs[2],
                    context.regs[3],
                ));
            }
            value if value == SVC_THREAD_EXIT as i32 || value == SVC_RETURN_TO_HOST as i32 => {
                if runtime_state.application_main_is_active() {
                    if !bootstrap_displayed {
                        echo!(
                            "ARM64 guest entry returned while UIApplicationMain is active; presenting compatibility boot state before continuing the application lifecycle"
                        );
                        if let Some(window) = window.as_mut() {
                            bootstrap_displayed = display_boot_screen(&bundle, &fs, device_family, window);
                        }
                        if bootstrap_displayed {
                            runtime_state.mark_boot_screen_reached();
                        }
                    }
                    echo!(
                        "ARM64 runtime returned from entry point: application_main_active=true application_main_calls={} boot_screen_reached={} last_selector={}; guest lifecycle continues on the host run loop",
                        runtime_state.application_main_calls,
                        runtime_state.boot_screen_reached,
                        runtime_state.last_selector.as_deref().unwrap_or("<none>"),
                    );
                    if schedule_display_link_callback(&mut memory, &mut context, &mut runtime_state)? {
                        if let Some(transfer_pc) = runtime_state.take_guest_transfer() {
                            echo!("ARM64 starting scheduled display-link guest callback at {:#x}", transfer_pc);
                            context.pc = transfer_pc;
                            cpu.load_context(&context);
                            cpu.clear_halt(A64_HALT_USER_DEFINED1);
                            cpu.clear_halt(A64_HALT_USER_DEFINED2);
                            cpu.clear_halt(A64_HALT_USER_DEFINED3);
                            continue;
                        }
                    }
                    run_arm64_application_lifecycle(&mut window, &options);
                    return Ok(());
                }
                echo!(
                    "ARM64 runtime returned from entry point: application_main_active=false application_main_calls={} boot_screen_reached={} last_selector={}",
                    runtime_state.application_main_calls,
                    runtime_state.boot_screen_reached,
                    runtime_state.last_selector.as_deref().unwrap_or("<none>"),
                );
                return Ok(());
            }
            value if value >= SVC_HOST_BASE as i32 => {
                host_dispatches += 1;
                host_dispatches_since_callback += 1;
                let symbol = host_stubs.get(&value).map(|(name, _)| name.as_str()).unwrap_or("<unknown>");
                let continuation_pc = host_call_continuation(&context);
                let call_site = host_call_site(&context);
                let host_call = host_call_identity(&context);
                let callback_arguments = callback_event_context(&context);
                if guest_progress_since_host_call {
                    last_host_call = None;
                    repeated_host_call = 0;
                    guest_progress_since_host_call = false;
                }
                if last_host_call == Some(host_call) {
                    repeated_host_call += 1;
                } else {
                    last_host_call = Some(host_call);
                    repeated_host_call = 0;
                }
                if repeated_host_call > STALL_THRESHOLD {
                    log_once_fmt!("ARM64 execution stall detected: repeated_guest_call={} host_stub_pc={:#x} guest_call_site={:#x} continuation_pc={:#x} binding={} previous_pcs={:?} branch_history={:?} [subsequent identical stalls suppressed]", repeated_host_call, context.pc, call_site, continuation_pc, symbol, previous_pcs, previous_branches);
                    log_once_fmt!("ARM64 stall registers: {}", register_dump(&context));
                    log_once_fmt!("ARM64 stall stack: {}", stack_dump(&memory, context.sp));
                    return Err(format!(
                        "ARM64 runtime stalled at host binding {}; host_stub_pc={:#x} guest_call_site={:#x} continuation_pc={:#x}; sp={:#x} lr={:#x} fp={:#x}; objc_messages={} metal_commands={} backend={}",
                        symbol,
                        context.pc,
                        call_site,
                        continuation_pc,
                        context.sp,
                        context.regs[30],
                        context.regs[29],
                        runtime_state.objc_messages,
                        runtime_state.metal_commands,
                        runtime_state.graphics_backend.label(),
                    ));
                }
                if symbol == "<unimplemented>" {
                    runtime_state.mark_unresolved_call(symbol, context.pc);
                }
                if host_dispatches <= 16 || host_dispatches.is_power_of_two() {
                    log_dbg!(
                        "ARM64 host binding #{}: {} host_stub_pc={:#x} guest_run_entry_pc={:#x} guest_call_site={:#x} continuation_pc={:#x} {}",
                        host_dispatches,
                        symbol,
                        context.pc,
                        instruction_pc,
                        call_site,
                        continuation_pc,
                        callback_event_context(&context),
                    );
                }
                verify_abi(&context, symbol);
                let sp_before_dispatch = context.sp;
                runtime_state.last_symbol = Some(symbol.to_owned());
                let handled = match dispatch(&mut memory, &mut context, symbol, &mut runtime_state, window.as_deref_mut()) {
                    Ok(handled) => {
                        runtime_state.last_successful_symbol = Some(symbol.to_owned());
                        handled
                    }
                    Err(error) => {
                        failure_diagnostics(&memory, &context, &previous_pcs, &previous_branches, &runtime_state, &format!("host callback {} failed: {}", symbol, error));
                        return Err(format!("ARM64 host callback {} failed: {}", symbol, error));
                    }
                };
                if context.sp != sp_before_dispatch {
                    log_dbg!(
                        "ARM64 host callback changed SP: symbol={} before={:#x} after={:#x}",
                        symbol,
                        sp_before_dispatch,
                        context.sp,
                    );
                }
                verify_abi(&context, symbol);
                if matches!(symbol, "malloc" | "calloc" | "valloc" | "posix_memalign" | "free" | "malloc_zone_free" | "_ZdlPv" | "_ZdaPv" | "__ZdlPv" | "__ZdaPv" | "_Znwm" | "_Znam" | "__Znwm" | "__Znam" | "Znwm" | "Znam" | "ZnwmRKSt9nothrow_t" | "__ZnwmRKSt9nothrow_t" | "memcpy" | "memmove" | "memcpy_chk" | "memmove_chk" | "memset" | "memset_chk")
                    && (host_dispatches <= 16 || host_dispatches.is_power_of_two())
                {
                    log_dbg!(
                        "ARM64 host callback return: symbol={} host_stub_pc={:#x} guest_run_entry_pc={:#x} guest_call_site={:#x} continuation_pc={:#x} result_x0={:#x} {}",
                        symbol,
                        context.pc,
                        instruction_pc,
                        call_site,
                        continuation_pc,
                        context.regs[0],
                        callback_arguments,
                    );
                }
                if runtime_state.take_present_request() {
                    log_dbg!(
                        "ARM64 compatibility frame {} submitted: commands={}, clear={:?}",
                        runtime_state.frame_serial,
                        runtime_state.metal_commands,
                        runtime_state.clear_color
                    );
                    if let Some(window) = window.as_mut() {
                        window.present_compatibility_frame(runtime_state.clear_color);
                        if runtime_state.application_main_is_active() {
                            runtime_state.mark_boot_screen_reached();
                        }
                    }
                }
                let guest_transfer = runtime_state.take_guest_transfer();
                if let Some(transfer_pc) = guest_transfer {
                    log_dbg!("ARM64 continuing guest execution at transferred Objective-C method {:#x}", transfer_pc);
                    context.pc = transfer_pc;
                    cpu.load_context(&context);
                    cpu.clear_halt(A64_HALT_USER_DEFINED1);
                    cpu.clear_halt(A64_HALT_USER_DEFINED2);
                    cpu.clear_halt(A64_HALT_USER_DEFINED3);
                }
                if runtime_state.take_boot_screen_request() {
                    log_once_fmt!(
                        "ARM64 boot screen notification consumed: application_main_calls={}; guest execution continuing [repeated notifications suppressed]",
                        runtime_state.application_main_calls,
                    );
                }
                if runtime_state.take_application_bootstrap_request() {
                    bootstrap_grace_slices = ARM64_BOOTSTRAP_GRACE_SLICES;
                    if !bootstrap_displayed {
                        if let Some(window) = window.as_mut() {
                            bootstrap_displayed = display_boot_screen(&bundle, &fs, device_family, window);
                            if bootstrap_displayed {
                                runtime_state.mark_boot_screen_reached();
                            }
                        }
                    }
                    echo!(
                        "ARM64 application lifecycle bootstrap observed: displayed_boot_state={} grace_slices={}",
                        bootstrap_displayed,
                        bootstrap_grace_slices,
                    );
                }
                if let Some(window) = window.as_mut() {
                    window.poll_for_events(&options);
                }
                if bootstrap_grace_slices > 0 {
                    bootstrap_grace_slices -= 1;
                    if let Some(window) = window.as_mut() {
                        window.poll_for_events(&options);
                    }
                }
                if !handled {
                    let first = runtime_state.mark_unimplemented_reached(symbol);
                    if first {
                        echo!("Warning: ARM64 reached unresolved host function {} at pc={:#x} lr={:#x} sp={:#x}; returning zero", symbol, context.pc, context.regs[30], context.sp);
                    }
                }
                if host_dispatches_since_callback > MAX_HOST_DISPATCHES_PER_CALLBACK {
                    return Err(format!(
                        "ARM64 runtime made too many host calls within one guest callback; last binding was {}",
                        symbol
                    ));
                }
                if host_dispatches.is_power_of_two() {
                    log_dbg!(
                        "ARM64 runtime counters: dispatches={}, objc_messages={}, metal_commands={}, backend={}",
                        runtime_state.host_dispatches,
                        runtime_state.objc_messages,
                        runtime_state.metal_commands,
                        runtime_state.graphics_backend.label()
                    );
                }
                if guest_transfer.is_none() {
                    context.pc = context.regs[30];
                }
                if runtime_state.take_guest_yield() {
                    let callback_return_pc = runtime_state.display_link_return_pc.unwrap_or(context.regs[30]);
                    runtime_state.render_diagnostics.callback_return_pc = callback_return_pc;
                    runtime_state.trace_render_event(format!("frame={} callback_return=drawFrame display_link_return_pc={:#x} lr={:#x} present={} next_scheduled={} last_gl={} last_guest_pc={:#x}", runtime_state.render_diagnostics.display_link_callbacks, context.pc, callback_return_pc, runtime_state.render_diagnostics.present_framebuffer_calls, runtime_state.display_link_is_scheduled(), runtime_state.render_diagnostics.last_gl_symbol.as_deref().unwrap_or("<none>"), runtime_state.render_diagnostics.last_guest_pc));
                    host_dispatches_since_callback = 0;
                    let callback_scheduled = schedule_display_link_callback(&mut memory, &mut context, &mut runtime_state)?;
                    if let Some(transfer_pc) = runtime_state.take_guest_transfer() {
                        log_dbg!(
                            "ARM64 scheduling next display-link guest callback at {:#x} (scheduled={})",
                            transfer_pc,
                            callback_scheduled,
                        );
                        context.pc = transfer_pc;
                        cpu.load_context(&context);
                        cpu.clear_halt(A64_HALT_USER_DEFINED1);
                        cpu.clear_halt(A64_HALT_USER_DEFINED2);
                        cpu.clear_halt(A64_HALT_USER_DEFINED3);
                    }
                }
                no_progress_slices = 0;
                no_progress_since = Instant::now();
                cpu.load_context(&context);
                cpu.clear_halt(A64_HALT_USER_DEFINED1);
                cpu.clear_halt(A64_HALT_USER_DEFINED2);
                cpu.clear_halt(A64_HALT_USER_DEFINED3);
                continue;
            }
            -6 => {
                failure_diagnostics(&memory, &context, &previous_pcs, &previous_branches, &runtime_state, "watchdog timeout");
                return Err(format!("ARM64 execution watchdog stopped the CPU at pc {:#x}", context.pc));
            }
            value if value >= 0 => return Err(format!("ARM64 runtime reached unimplemented SVC {} at {:#x}", value, context.pc)),
            value => {
                failure_diagnostics(&memory, &context, &previous_pcs, &previous_branches, &runtime_state, &format!("Dynarmic exit code {}", value));
                return Err(format!("ARM64 runtime failed with code {} at {:#x}", value, context.pc));
            }
        }
    }
}
