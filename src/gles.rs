/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! OpenGL ES abstraction and implementations.
//!
//! touchHLE uses OpenGL ES for several things. OpenGL ES is part of iPhone OS's
//! API surface and can be used by apps for rendering, so there must be an
//! implementation of it to expose to the app. Beyond that, there are various
//! internal uses for which any graphics API would work, but using the same one
//! makes things simpler:
//! - Presenting frames rendered by the app to the screen, with appropriate
//!   rotation and scaling.
//! - Drawing touchHLE's virtual cursor.
//! - Drawing the app's splash screen.
//! - Compositing the app's Core Animation layers (usually for UIKit views).
//!
//! touchHLE's OpenGL ES implementation consists of a series of layers. This
//! module contains the layers that aren't specific to a particular use:
//!
//! - [gles_generic] provides an abstraction over OpenGL ES implementations.
//! - Various modules provide implementations:
//!   - [gles1_native] passes through native OpenGL ES 1.1.
//!   - [gles1_on_gl2] provides an implementation of OpenGL ES 1.1 using OpenGL
//!     2.1 compatibility profile.
//!   - There might be more in future.
//! - [gles11_raw] provides raw bindings for OpenGL ES 1.1 generated from the
//!   Khronos API headers. **The function bindings are only for use within this
//!   module.** The constants and types can be used outside it, however.
//!   - [gl21compat_raw] is the same thing, but for OpenGL 2.1 compatibility
//!     profile, which can't be used outside this module at all.
//! - [present] provides utilities for presenting frames to the window using an
//!   abstract OpenGL ES implementation.
//!
//! In contrast, [crate::frameworks::opengles] is a layer specific to OpenGL
//! ES's role as a part of the iPhone OS API surface. It wraps [gles_generic] to
//! expose OpenGL ES to the guest app.
//!
//! Useful resources for OpenGL ES 1.1:
//! - [Reference pages](https://registry.khronos.org/OpenGL-Refpages/es1.1/xhtml/)
//! - [Specification](https://registry.khronos.org/OpenGL/specs/es/1.1/es_full_spec_1.1.pdf)
//! - Apple's [OpenGL ES Hardware Platform Guide for iOS](https://developer.apple.com/library/archive/documentation/OpenGLES/Conceptual/OpenGLESHardwarePlatformGuide_iOS/OpenGLESPlatforms/OpenGLESPlatforms.html)
//! - Extensions:
//!   - [OES_framebuffer_object](https://registry.khronos.org/OpenGL/extensions/OES/OES_framebuffer_object.txt)
//!   - [IMG_texture_compression_pvrtc](https://registry.khronos.org/OpenGL/extensions/IMG/IMG_texture_compression_pvrtc.txt)
//!   - [OES_compressed_paletted_texture](https://registry.khronos.org/OpenGL/extensions/OES/OES_compressed_paletted_texture.txt) (also incorporated into the main spec)
//!   - [OES_matrix_palette](https://registry.khronos.org/OpenGL/extensions/OES/OES_matrix_palette.txt)
//!   - [EXT_texture_format_BGRA8888](https://registry.khronos.org/OpenGL/extensions/EXT/EXT_texture_format_BGRA8888.txt)
//!   - [OES_blend_subtract](https://registry.khronos.org/OpenGL/extensions/OES/OES_blend_subtract.txt)
//!
//! Useful resources for OpenGL 2.1:
//! - [Reference pages](https://registry.khronos.org/OpenGL-Refpages/gl2.1/)
//! - [Specification](https://registry.khronos.org/OpenGL/specs/gl/glspec21.pdf)
//! - Extensions:
//!   - [EXT_framebuffer_object](https://registry.khronos.org/OpenGL/extensions/EXT/EXT_framebuffer_object.txt)
//!   - [ARB_matrix_palette](https://registry.khronos.org/OpenGL/extensions/ARB/ARB_matrix_palette.txt)
//!   - [ARB_vertex_blend](https://registry.khronos.org/OpenGL/extensions/ARB/ARB_vertex_blend.txt)
//!   - [EXT_blend_subtract](https://registry.khronos.org/OpenGL/extensions/EXT/EXT_blend_subtract.txt)
//!
//! Useful resources for both:
//! - Extensions:
//!   - [EXT_texture_filter_anisotropic](https://registry.khronos.org/OpenGL/extensions/EXT/EXT_texture_filter_anisotropic.txt)
//!   - [EXT_texture_lod_bias](https://registry.khronos.org/OpenGL/extensions/EXT/EXT_texture_lod_bias.txt)

pub mod wgpu;
pub mod gles1_native;
pub mod gles1_on_gl2;
pub mod gles1_on_gles2;
pub mod gles1_on_gles3;
pub mod gles2_glsl;
pub mod gles2_native;
pub mod gles2_on_gl3;
pub mod gles3_native;
pub mod gles3_on_gl3;
mod gles_generic;
pub mod present;
pub mod software;
pub mod util;
use touchHLE_gl_bindings::gl21compat as gl21compat_raw;
use touchHLE_gl_bindings::gl33core as gl33core_raw;
pub use touchHLE_gl_bindings::gles11 as gles11_raw;
pub use touchHLE_gl_bindings::gles2 as gles2_raw;
pub use touchHLE_gl_bindings::gles30 as gles30_raw;

use crate::environment::Environment;
use crate::window::GLVersion;
use gles1_native::GLES1NativeContext;
use gles1_on_gl2::GLES1OnGL2Context;
use gles1_on_gles2::GLES1OnGLES2Context;
use gles1_on_gles3::GLES1OnGLES3Context;
use gles2_native::GLES2NativeContext;
use gles2_on_gl3::GLES2OnGL3Context;
use gles3_native::GLES3NativeContext;
use gles3_on_gl3::GLES3OnGL3Context;

pub use gles_generic::GLESContext;
pub use gles_generic::GLES;
pub use software::SoftwareGLESContext;

use std::sync::atomic::{AtomicU32, Ordering};

static TRANSLATOR_TRACE_EVENTS: AtomicU32 = AtomicU32::new(0);

pub(crate) fn configure_translator_tracing(_enabled: bool) {}

pub(crate) fn translator_tracing_enabled() -> bool {
    std::env::var_os("TOUCHHLE_TRACE_TRANSLATOR").is_some()
}

pub(crate) fn trace_translator_event(event: String) {
    if !translator_tracing_enabled() {
        return;
    }
    let number = TRANSLATOR_TRACE_EVENTS.fetch_add(1, Ordering::Relaxed);
    if number < 512 {
        log!("[translator] #{:03} {}", number + 1, event);
    } else if number == 512 {
        log!("[translator] further events suppressed after 512 entries");
    }
}

pub fn llvmpipe_fallback_available() -> bool {
    software::available()
}

pub fn configure_llvmpipe_fallback(enabled: bool) -> bool {
    software::configure(enabled)
}
/// Configure a user-supplied EGL/GLES driver before SDL creates its window.
/// ZIP files are extracted into the emulator data directory and reused there.
pub fn configure_custom_driver(path: Option<&std::path::Path>) -> bool {
    let Some(path) = path else {
        return false;
    };
    let requested = if path.is_absolute() {
        path.to_path_buf()
    } else {
        crate::paths::user_data_base_path().join(path)
    };
    let requested = if requested.is_dir() {
        if let Some(archive) = find_driver_archive(&requested) {
            log!("Custom driver archive selected: {}", archive.display());
            archive
        } else {
            log!("Custom driver directory contains no ZIP archive: {}", requested.display());
            return false;
        }
    } else {
        requested
    };
    log!("Custom driver requested: {}", requested.display());
    let driver_dir = if requested.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("zip")) {
        match extract_custom_driver_archive(&requested) {

            Ok(directory) => directory,
            Err(error) => {
                log!("Custom driver archive could not be prepared: {}", error);
                return false;
            }
        }
    } else if requested.is_dir() {
        requested.clone()
    } else if requested.is_file() {
        requested.parent().unwrap_or_else(|| std::path::Path::new(".")).to_path_buf()
    } else {
        log!("Custom driver path does not exist: {}", requested.display());
        return false;
    };
    let (egl, gles) = if requested.is_file() && requested.extension().is_none() {
        (requested.clone(), requested.clone())
    } else if requested.is_file() && requested.extension().is_some_and(|extension| {
        ["so", "dylib", "dll"].iter().any(|name| extension.eq_ignore_ascii_case(name))
    }) {
        log!("Custom driver file {} selected; SDL will use it as the GLES library and retain the directory for EGL lookup", requested.display());
        (requested.clone(), requested.clone())
    } else {
        let egl = find_driver_library(&driver_dir, &["libEGL.so", "libEGL.so.1", "libEGL.dylib", "libEGL.dll"]);
        let gles = find_driver_library(&driver_dir, &["libGLESv2.so", "libGLESv2.so.2", "libGLESv2.dylib", "libGLESv2.dll"]);
        let (Some(egl), Some(gles)) = (egl, gles) else {
            log!("Custom driver requested at {} but both EGL and GLESv2 libraries were not found under {}", requested.display(), driver_dir.display());
            return false;
        };
        (egl, gles)
    };
    unsafe {
        std::env::set_var("SDL_VIDEO_EGL_DRIVER", &egl);
        std::env::set_var("SDL_VIDEO_GL_DRIVER", &gles);
    }
    sdl2::hint::set("SDL_OPENGL_ES_DRIVER", "1");
    log!("Custom driver active: EGL={}, GLES={}, source={}", egl.display(), gles.display(), requested.display());
    true
}

fn find_driver_archive(directory: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut archives: Vec<_> = std::fs::read_dir(directory)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
        })
        .collect();
    archives.sort();
    archives.into_iter().next()
}

fn find_driver_library(directory: &std::path::Path, names: &[&str]) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(directory).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.file_name().and_then(|name| name.to_str()).is_some_and(|name| names.iter().any(|candidate| name.eq_ignore_ascii_case(candidate))) {
            return Some(path);
        }
    }
    None
}

fn extract_custom_driver_archive(path: &std::path::Path) -> Result<std::path::PathBuf, String> {
    use std::io::Read;
    if !path.is_file() {
        return Err(format!("archive does not exist: {}", path.display()));
    }
    let archive_file = std::fs::File::open(path).map_err(|error| format!("could not open {}: {}", path.display(), error))?;
    let mut archive = zip::ZipArchive::new(archive_file).map_err(|error| format!("invalid ZIP archive: {}", error))?;
    let stem = path.file_stem().and_then(|name| name.to_str()).unwrap_or("custom-driver");
    let safe_stem: String = stem.chars().map(|character| if character.is_ascii_alphanumeric() || character == '-' || character == '_' { character } else { '_' }).collect();
    let output = crate::paths::user_data_base_path().join("touchHLE_custom_drivers").join(safe_stem);
    std::fs::create_dir_all(&output).map_err(|error| format!("could not create {}: {}", output.display(), error))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| format!("could not read ZIP entry {}: {}", index, error))?;
        let Some(enclosed) = entry.enclosed_name().map(|name| output.join(name)) else {
            return Err(format!("unsafe ZIP entry at index {}", index));
        };
        if entry.is_dir() {
            std::fs::create_dir_all(&enclosed).map_err(|error| format!("could not create {}: {}", enclosed.display(), error))?;
            continue;
        }
        if let Some(parent) = enclosed.parent() {
            std::fs::create_dir_all(parent).map_err(|error| format!("could not create {}: {}", parent.display(), error))?;
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|error| format!("could not extract {}: {}", entry.name(), error))?;
        std::fs::write(&enclosed, bytes).map_err(|error| format!("could not write {}: {}", enclosed.display(), error))?;
    }
    log!("Custom driver archive extracted to {}", output.display());
    Ok(output)
}

pub fn configure_angle_driver(enabled: bool) {
    if !enabled {
        return;
    }

    let default_egl = if cfg!(target_os = "windows") {
        "libEGL.dll"
    } else if cfg!(target_os = "macos") {
        "libEGL.dylib"
    } else {
        "libEGL.so"
    };
    let default_gles = if cfg!(target_os = "windows") {
        "libGLESv2.dll"
    } else if cfg!(target_os = "macos") {
        "libGLESv2.dylib"
    } else {
        "libGLESv2.so"
    };
    let egl_path = std::env::var("TOUCHHLE_ANGLE_EGL").unwrap_or_else(|_| default_egl.to_owned());
    let gles_path =
        std::env::var("TOUCHHLE_ANGLE_GLES").unwrap_or_else(|_| default_gles.to_owned());
    let egl_exists = std::path::Path::new(&egl_path).exists();
    let gles_exists = std::path::Path::new(&gles_path).exists();

    unsafe {
        std::env::set_var("SDL_VIDEO_EGL_DRIVER", &egl_path);
        std::env::set_var("SDL_VIDEO_GL_DRIVER", &gles_path);
    }
    sdl2::hint::set("SDL_OPENGL_ES_DRIVER", "1");
    log!(
        "ANGLE override requested: EGL={} (exists={}), GLES={} (exists={}); SDL will try these before the first window",
        egl_path,
        egl_exists,
        gles_path,
        gles_exists
    );
    if !egl_exists || !gles_exists {
        log!("ANGLE libraries are not present at the configured paths; SDL may fall back or context creation may fail");
    }
}

/// Labels for [GLES] implementations and an abstraction for constructing them.
#[derive(Copy, Clone)]
pub enum GLESImplementation {
    /// [gles1_native::GLES1Native].
    GLES1Native,
    /// [gles1_on_gl2::GLES1OnGL2].
    GLES1OnGL2,
    /// [gles1_on_gles2::GLES1OnGLES2].
    GLES1OnGLES2,
    /// [gles1_on_gles3::GLES1OnGLES3].
    GLES1OnGLES3,
    Software,
}
impl GLESImplementation {
    /// List of OpenGL ES 1.1 implementations in order of preference.
    pub const GLES1_IMPLEMENTATIONS: &'static [Self] = &[Self::GLES1Native, Self::GLES1OnGL2];
    /// Convert from short name used for command-line arguments. Returns [Err]
    /// if name is not recognized..
    pub fn from_short_name(name: &str) -> Result<Self, ()> {
        match name {
            "gles1_on_gl2" => Ok(Self::GLES1OnGL2),
            "gles1_on_gles2" => Ok(Self::GLES1OnGLES2),
            "gles1_on_gles3" => Ok(Self::GLES1OnGLES3),
            "gles1_native" => Ok(Self::GLES1Native),
            "software" | "software-rendering" | "cpu" => Ok(Self::Software),
            _ => Err(()),
        }
    }
    /// See [GLESContext::description].
    pub fn description(self) -> &'static str {
        match self {
            Self::GLES1Native => GLES1NativeContext::description(),
            Self::GLES1OnGL2 => GLES1OnGL2Context::description(),
            Self::GLES1OnGLES2 => GLES1OnGLES2Context::description(),
            Self::GLES1OnGLES3 => GLES1OnGLES3Context::description(),
            Self::Software => SoftwareGLESContext::description(),
        }
    }
    /// See [GLESContext::new].
    pub fn construct(
        self,
        window: &mut crate::window::Window,
    ) -> Result<Box<dyn GLESContext>, String> {
        fn boxer<T: GLESContext + 'static>(ctx: T) -> Box<dyn GLESContext> {
            Box::new(ctx)
        }
        match self {
            Self::GLES1Native => GLES1NativeContext::new(window).map(boxer),
            Self::GLES1OnGL2 => GLES1OnGL2Context::new(window).map(boxer),
            Self::GLES1OnGLES2 => GLES1OnGLES2Context::new(window).map(boxer),
            Self::GLES1OnGLES3 => GLES1OnGLES3Context::new(window).map(boxer),
            Self::Software => SoftwareGLESContext::new(window).map(boxer),
        }
    }
}

pub fn create_gles1_translator_ctx_no_parent_stack(
    window: &mut crate::window::Window,
) -> Box<dyn GLESContext> {
    assert!(window.on_main_stack());
    log!("Creating the OpenGL ES 1.1 to native OpenGL ES 2.0 translator");
    Box::new(
        GLES1OnGLES2Context::new(window)
            .expect("Couldn't create OpenGL ES 1.1-on-GLES2 translator context!"),
    )
}
pub fn create_gles1_gles3_translator_ctx_no_parent_stack(
    window: &mut crate::window::Window,
) -> Box<dyn GLESContext> {
    assert!(window.on_main_stack());
    log!("Creating the OpenGL ES 1.1 to native OpenGL ES 3.0 translator");
    Box::new(
        GLES1OnGLES3Context::new(window)
            .expect("Couldn't create OpenGL ES 1.1-on-GLES3 translator context!"),
    )
}

pub fn create_gles1_gles3_translator_ctx(env: &mut Environment) -> Box<dyn GLESContext> {
    env.on_parent_stack_in_coroutine(|window, _options| {
        create_gles1_gles3_translator_ctx_no_parent_stack(window)
    })
}

pub fn create_gles1_translator_ctx(env: &mut Environment) -> Box<dyn GLESContext> {
    env.on_parent_stack_in_coroutine(|window, _options| {
        create_gles1_translator_ctx_no_parent_stack(window)
    })
}

/// Try to create an OpenGL ES 1.1 context using the configured strategies,
/// panicking on failure.
pub fn create_gles1_ctx(env: &mut Environment) -> Box<dyn GLESContext> {
    env.on_parent_stack_in_coroutine(|window, options| {
        create_gles1_ctx_no_parent_stack(window, options)
    })
}

/// Try to create an OpenGL ES 2.0 context, panicking on failure.
///
/// The preference order, from "most-correct" to "only as a last resort":
///
/// 1. [`GLES2NativeContext`] — a real OpenGL ES 2.0 driver. This is the
///    only thing that works on platforms without desktop OpenGL such as
///    Android, and on real iOS hardware emulation. Every ES 2.0 entry point
///    is a direct passthrough to the host driver.
/// 2. [`GLES2OnGL3Context`] — a full ES 2.0 backend built on top of
///    desktop OpenGL 3.3 Core. This shares its implementation with the ES
///    3.0 fallback ([`GLES3OnGL3Context`]), giving us a single source of
///    truth for ES 2.0 / ES 3.0 emulation, full shader support, and proper
///    GLSL ES → desktop GLSL translation via
///    [`gles2_glsl::translate_glsl_es_to_120`]. This is the preferred
///    fallback on x86 Linux/macOS desktops where Mesa lacks a native ES 2.0
///    surface.
/// 3. [`GLES1OnGL2Context`] — legacy fallback that piggy-backs on a desktop
///    OpenGL 2.1 compatibility profile context. Only used on the rare host
///    that has GL 2.1 compat but no GL 3.3 Core (e.g. very old macOS
///    installations); kept around for backwards compatibility.
pub fn create_software_gles_ctx(env: &mut Environment) -> Box<dyn GLESContext> {
    env.on_parent_stack_in_coroutine(|window, _options| {
        log!("Using CPU software OpenGL ES 2.0 / 3.0 compatibility rasterizer");
        Box::new(
            SoftwareGLESContext::new(window)
                .expect("Could not create software GLES context"),
        )
    })
}

pub fn create_gles2_ctx(env: &mut Environment) -> Box<dyn GLESContext> {
    env.on_parent_stack_in_coroutine(|window, _options| {
        assert!(window.on_main_stack());
        log!("Creating an OpenGL ES 2.0 context:");

        log!("Trying: {}", GLES2NativeContext::description());
        match GLES2NativeContext::new(window) {
            Ok(ctx) => {
                log!("=> Success!");
                let boxed: Box<dyn GLESContext> = Box::new(ctx);
                return boxed;
            }
            Err(err) => {
                log!("=> Failed: {}.", err);
            }
        }

        log!(
            "Trying: {} (used for OpenGL ES 2.0)",
            GLES2OnGL3Context::description()
        );
        match GLES2OnGL3Context::new(window) {
            Ok(ctx) => {
                log!("=> Success!");
                let boxed: Box<dyn GLESContext> = Box::new(ctx);
                return boxed;
            }
            Err(err) => {
                log!("=> Failed: {}.", err);
            }
        }

        log!(
            "Trying: {} (legacy GL 2.1 fallback for OpenGL ES 2.0)",
            GLES1OnGL2Context::description()
        );
        match GLES1OnGL2Context::new(window) {
            Ok(ctx) => {
                log!("=> Success!");
                let boxed: Box<dyn GLESContext> = Box::new(ctx);
                boxed
            }
            Err(err) => panic!("Couldn't create OpenGL ES 2.0 context: {}", err),
        }
    })
}

/// Try to create an OpenGL ES 3.0 context, panicking on failure.
///
/// This is the entry point used by [crate::frameworks::opengles::eagl] when
/// `EAGLContext initWithAPI:` is called with `kEAGLRenderingAPIOpenGLES3` (=
/// 3). It tries the native ES 3.0 backend first — the only thing that works
/// on Android and on desktop drivers configured for an ES context — and
/// falls back to the desktop GL 3.3 Core translation backend on hosts
/// without a native ES 3.0 driver (most x86 Linux/macOS desktops).
pub fn create_gles3_ctx(env: &mut Environment) -> Box<dyn GLESContext> {
    env.on_parent_stack_in_coroutine(|window, _options| {
        assert!(window.on_main_stack());
        log!("Creating an OpenGL ES 3.0 context:");

        log!("Trying: {}", GLES3NativeContext::description());
        match GLES3NativeContext::new(window) {
            Ok(ctx) => {
                log!("=> Success!");
                let boxed: Box<dyn GLESContext> = Box::new(ctx);
                return boxed;
            }
            Err(err) => {
                log!("=> Failed: {}.", err);
            }
        }

        log!(
            "Trying: {} (used for OpenGL ES 3.0)",
            GLES3OnGL3Context::description()
        );
        match GLES3OnGL3Context::new(window) {
            Ok(ctx) => {
                log!("=> Success!");
                let boxed: Box<dyn GLESContext> = Box::new(ctx);
                boxed
            }
            Err(err) => panic!("Couldn't create OpenGL ES 3.0 context: {}", err),
        }
    })
}

/// Create an OpenGL ES 2.0 context from the window's main stack.
///
/// The window owns the internal context used for compositing and the splash
/// screen, so it must be created before the guest environment exists.
pub fn create_gles2_ctx_no_parent_stack(
    window: &mut crate::window::Window,
) -> Box<dyn GLESContext> {
    assert!(window.on_main_stack());
    log!("Creating an OpenGL ES 2.0 context:");

    log!("Trying: {}", GLES2NativeContext::description());
    if let Ok(ctx) = GLES2NativeContext::new(window) {
        log!("=> Success!");
        return Box::new(ctx);
    }

    log!(
        "Trying: {} (used for OpenGL ES 2.0)",
        GLES2OnGL3Context::description()
    );
    if let Ok(ctx) = GLES2OnGL3Context::new(window) {
        log!("=> Success!");
        return Box::new(ctx);
    }

    log!(
        "Trying: {} (legacy GL 2.1 fallback for OpenGL ES 2.0)",
        GLES1OnGL2Context::description()
    );
    match GLES1OnGL2Context::new(window) {
        Ok(ctx) => {
            log!("=> Success!");
            Box::new(ctx)
        }
        Err(err) => panic!("Couldn't create OpenGL ES 2.0 context: {}", err),
    }
}

/// Same as [create_gles1_ctx], but without calling
/// [Environment::on_parent_stack_in_coroutine]. Only should be called by
/// functions not inside a coroutine that can't use [Environment].
pub fn create_gles1_ctx_no_parent_stack(
    window: &mut crate::window::Window,
    options: &crate::options::Options,
) -> Box<dyn GLESContext> {
    assert!(window.on_main_stack());
    log!("Creating an OpenGL ES 1.1 context:");
    configure_angle_driver(options.angle_driver);
    if options.software_rendering {
        log!("Using CPU-only software OpenGL ES 1.1 rasterizer");
        return Box::new(
            SoftwareGLESContext::new(window).expect("Could not create software GLES context"),
        );
    }
    let list = if let Some(ref preference) = options.gles1_implementation {
        std::slice::from_ref(preference)
    } else {
        GLESImplementation::GLES1_IMPLEMENTATIONS
    };
    let mut gles1_ctx = None;
    for implementation in list {
        log!("Trying: {}", implementation.description());
        match implementation.construct(window) {
            Ok(ctx) => {
                log!("=> Success!");
                gles1_ctx = Some(ctx);
                break;
            }
            Err(err) => {
                log!("=> Failed: {}.", err);
            }
        }
    }
    gles1_ctx.expect("Couldn't create OpenGL ES 1.1 context!")
}
