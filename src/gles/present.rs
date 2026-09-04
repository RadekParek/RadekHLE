/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Utilities for presenting frames to the window using an abstract OpenGL ES
//! implementation.

use super::gles11_raw as gles11; // constants and types only
use super::GLES;
use crate::matrix::Matrix;
use std::time::{Duration, Instant};

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::sync::OnceLock;

pub struct FpsCounter {
    time: std::time::Instant,
    frames: u32,
}

// Global FPS text cache updated by FpsCounter so present_frame can draw it.
static LAST_FPS_TEXT: OnceLock<Mutex<String>> = OnceLock::new();
// Per-process cached GL glyph textures. Created lazily on first overlay draw.
static GLYPH_TEXTURES: OnceLock<Mutex<Option<Vec<u32>>>> = OnceLock::new();
// Runtime-controlled flag to enable the on-screen FPS overlay without requiring
// an environment variable. Use set_onscreen_fps_enabled(true/false) to control it
// from other parts of the runtime (e.g., the app picker or window input).
use std::sync::atomic::AtomicBool;
static ONSCREEN_FPS_ENABLED: OnceLock<AtomicBool> = OnceLock::new();
static HUD_METRICS: OnceLock<Mutex<HudMetrics>> = OnceLock::new();
static HUD_FPS_TENTHS: AtomicU64 = AtomicU64::new(0);
#[derive(Copy, Clone)]
struct Es2HudProgram {
    program: u32,
    position: i32,
    tex_coord: i32,
    texture: i32,
    vbo: u32,
}
thread_local! {
    static ES2_HUD_PROGRAM: std::cell::Cell<Option<Es2HudProgram>> = const { std::cell::Cell::new(None) };
}

#[derive(Clone, Default)]
struct HudMetrics {
    cpu_percent: Option<f32>,
    gpu_percent: Option<f32>,
    ram_mb: Option<u64>,
    architecture: String,
}

impl FpsCounter {
    pub fn start() -> Self {
        LAST_FPS_TEXT.get_or_init(|| Mutex::new(String::new()));
        GLYPH_TEXTURES.get_or_init(|| Mutex::new(None));
        FpsCounter {
            time: Instant::now(),
            frames: 0,
        }
    }

    pub fn count_frame(&mut self, label: std::fmt::Arguments<'_>) {
        self.frames += 1;
        let now = Instant::now();
        let duration = now - self.time;
        if duration >= Duration::from_secs(1) {
            self.time = now;
            let fps = std::mem::take(&mut self.frames) as f32 / duration.as_secs_f32();
            echo!("touchHLE: {} FPS: {:.2}", label, fps);
            // Update global text cache for on-screen overlay if enabled via
            // environment variable or the runtime flag.
            let onscreen_env = std::env::var_os("TOUCHHLE_ONSCREEN_FPS").is_some();
            let onscreen_runtime = ONSCREEN_FPS_ENABLED
                .get()
                .map(|b| b.load(Ordering::SeqCst))
                .unwrap_or(false);
            if onscreen_env || onscreen_runtime {
                HUD_FPS_TENTHS.store((fps * 10.0).round() as u64, Ordering::Relaxed);
                refresh_hud_metrics();
            }
        }
    }
}

/// Runtime API: enable/disable the on-screen FPS overlay at runtime.
pub fn set_onscreen_fps_enabled(enabled: bool) {
    ONSCREEN_FPS_ENABLED
        .get_or_init(|| AtomicBool::new(false))
        .store(enabled, Ordering::SeqCst);
}

pub fn set_onscreen_hud_architecture(architecture: &str) {
    let mutex = HUD_METRICS.get_or_init(|| Mutex::new(HudMetrics::default()));
    if let Ok(mut metrics) = mutex.lock() {
        metrics.architecture = architecture.to_owned();
    }
    update_hud_text();
}

fn refresh_hud_metrics() {
    let mutex = HUD_METRICS.get_or_init(|| Mutex::new(HudMetrics::default()));
    if let Ok(mut metrics) = mutex.lock() {
        metrics.cpu_percent = process_cpu_percent();
        metrics.gpu_percent = gpu_percent();
        metrics.ram_mb = resident_memory_mb();
    }
    update_hud_text();
}

pub fn onscreen_hud_text() -> Option<String> {
    let enabled = ONSCREEN_FPS_ENABLED
        .get()
        .map(|flag| flag.load(Ordering::SeqCst))
        .unwrap_or(false)
        || std::env::var_os("TOUCHHLE_ONSCREEN_FPS").is_some();
    if !enabled {
        return None;
    }
    let text = LAST_FPS_TEXT
        .get_or_init(|| Mutex::new(String::new()))
        .lock()
        .ok()
        .map(|value| value.clone())?;
    (!text.is_empty()).then_some(text)
}

pub fn overlay_software_hud(pixels: &mut [u8], width: u32, height: u32) {
    let Some(text) = onscreen_hud_text() else {
        return;
    };
    let scale = (width / 320).max(1) * 6;
    let glyph_width = GLYPH_W * scale;
    let glyph_height = GLYPH_H * scale;
    let mut x = 8u32;
    let y = 8u32;
    for character in text.chars() {
        if let Some(index) = glyph_index(character) {
            let bitmap = GLYPH_BITMAPS[index];
            for glyph_y in 0..GLYPH_H {
                for glyph_x in 0..GLYPH_W {
                    if bitmap[glyph_y as usize] & (1 << (7 - glyph_x)) == 0 {
                        continue;
                    }
                    for py in 0..scale {
                        for px in 0..scale {
                            let out_x = x + glyph_x * scale + px;
                            let out_y = y + glyph_y * scale + py;
                            if out_x < width && out_y < height {
                                let offset = ((out_y * width + out_x) * 4) as usize;
                                pixels[offset..offset + 4].copy_from_slice(&[255, 255, 255, 255]);
                            }
                        }
                    }
                }
            }
            x = x.saturating_add(glyph_width + 2);
        } else {
            x = x.saturating_add(glyph_width / 2);
        }
        if x >= width {
            break;
        }
    }
}

unsafe fn ensure_es2_hud_program(gles: &mut dyn GLES) -> Option<Es2HudProgram> {
    use gles11::types::*;
    use crate::gles::gles2_raw as gles2;
    if let Some(program) = ES2_HUD_PROGRAM.with(|cell| cell.get()) {
        return Some(program);
    }
    let vertex_source = b"attribute vec2 aPos; attribute vec2 aUV; varying vec2 vUV; void main(){gl_Position=vec4(aPos,0.0,1.0);vUV=aUV;}\0";
    let fragment_source = b"precision mediump float; varying vec2 vUV; uniform sampler2D uTex; void main(){gl_FragColor=texture2D(uTex,vUV);}\0";
    let compile = |source: &[u8], kind| {
        let shader = gles.CreateShader(kind);
        let pointer = source.as_ptr().cast();
        let length = (source.len() - 1) as GLint;
        gles.ShaderSource(shader, 1, &pointer, &length);
        gles.CompileShader(shader);
        let mut status = 0;
        gles.GetShaderiv(shader, gles2::COMPILE_STATUS, &mut status);
        (shader, status != 0)
    };
    let (vertex, vertex_ok) = compile(vertex_source, gles2::VERTEX_SHADER);
    let (fragment, fragment_ok) = compile(fragment_source, gles2::FRAGMENT_SHADER);
    if !vertex_ok || !fragment_ok {
        log!("[HUD] ES2 HUD shader compilation failed: vertex={}, fragment={}", vertex_ok, fragment_ok);
        gles.DeleteShader(vertex);
        gles.DeleteShader(fragment);
        return None;
    }
    let program = gles.CreateProgram();
    gles.AttachShader(program, vertex);
    gles.AttachShader(program, fragment);
    gles.BindAttribLocation(program, 0, b"aPos\0".as_ptr().cast());
    gles.BindAttribLocation(program, 1, b"aUV\0".as_ptr().cast());
    gles.LinkProgram(program);
    let mut linked = 0;
    gles.GetProgramiv(program, gles2::LINK_STATUS, &mut linked);
    gles.DeleteShader(vertex);
    gles.DeleteShader(fragment);
    if linked == 0 {
        log!("[HUD] ES2 HUD shader link failed");
        gles.DeleteProgram(program);
        return None;
    }
    let mut vbo = 0;
    gles.GenBuffers(1, &mut vbo);
    let result = Es2HudProgram {
        program,
        position: gles.GetAttribLocation(program, b"aPos\0".as_ptr().cast()),
        tex_coord: gles.GetAttribLocation(program, b"aUV\0".as_ptr().cast()),
        texture: gles.GetUniformLocation(program, b"uTex\0".as_ptr().cast()),
        vbo,
    };
    ES2_HUD_PROGRAM.with(|cell| cell.set(Some(result)));
    Some(result)
}

pub unsafe fn draw_onscreen_hud_es2(gles: &mut dyn GLES, viewport: (u32, u32, u32, u32)) {
    let Some(text) = onscreen_hud_text() else {
        return;
    };
    let Some(program) = ensure_es2_hud_program(gles) else {
        return;
    };
    let Some(textures) = ensure_glyph_textures(gles) else {
        return;
    };
    use gles11::types::*;
    use crate::gles::gles2_raw as gles2;
    let (vx, vy, vw, vh) = viewport;
    let scale = (vw / 320).max(1) * 6;
    let glyph_width = GLYPH_W * scale;
    let glyph_height = GLYPH_H * scale;
    let mut old_program = 0;
    let mut old_texture = 0;
    let mut old_active = 0;
    let mut old_buffer = 0;
    gles.GetIntegerv(gles2::CURRENT_PROGRAM, &mut old_program);
    gles.GetIntegerv(gles2::TEXTURE_BINDING_2D, &mut old_texture);
    gles.GetIntegerv(gles2::ACTIVE_TEXTURE, &mut old_active);
    gles.GetIntegerv(gles2::ARRAY_BUFFER_BINDING, &mut old_buffer);
    let blend_enabled = gles.IsEnabled(gles2::BLEND) != 0;
    gles.UseProgram(program.program);
    gles.Uniform1i(program.texture, 0);
    gles.ActiveTexture(gles2::TEXTURE0);
    gles.BindBuffer(gles2::ARRAY_BUFFER, program.vbo);
    gles.Enable(gles2::BLEND);
    gles.BlendFunc(gles2::SRC_ALPHA, gles2::ONE_MINUS_SRC_ALPHA);
    let mut x = vx + 8;
    let y = vy + 8;
    for character in text.chars() {
        let Some(index) = glyph_index(character) else {
            x = x.saturating_add(glyph_width / 2);
            continue;
        };
        let x0 = (x as f32 / vw as f32) * 2.0 - 1.0;
        let x1 = ((x + glyph_width) as f32 / vw as f32) * 2.0 - 1.0;
        let y0 = 1.0 - (y as f32 / vh as f32) * 2.0;
        let y1 = 1.0 - ((y + glyph_height) as f32 / vh as f32) * 2.0;
        let vertices: [f32; 16] = [x0, y1, 0.0, 1.0, x1, y1, 1.0, 1.0, x0, y0, 0.0, 0.0, x1, y0, 1.0, 0.0];
        gles.BufferData(gles2::ARRAY_BUFFER, std::mem::size_of_val(&vertices) as isize, vertices.as_ptr().cast(), gles2::STREAM_DRAW);
        gles.BindTexture(gles2::TEXTURE_2D, textures[index]);
        gles.EnableVertexAttribArray(program.position as GLuint);
        gles.EnableVertexAttribArray(program.tex_coord as GLuint);
        gles.VertexAttribPointer(program.position as GLuint, 2, gles2::FLOAT, gles2::FALSE, 16, std::ptr::null());
        gles.VertexAttribPointer(program.tex_coord as GLuint, 2, gles2::FLOAT, gles2::FALSE, 16, 8usize as *const _);
        gles.DrawArrays(gles2::TRIANGLE_STRIP, 0, 4);
        x = x.saturating_add(glyph_width + 2);
        if x >= vx + vw {
            break;
        }
    }
    gles.DisableVertexAttribArray(program.position as GLuint);
    gles.DisableVertexAttribArray(program.tex_coord as GLuint);
    if !blend_enabled {
        gles.Disable(gles2::BLEND);
    }
    gles.BindBuffer(gles2::ARRAY_BUFFER, old_buffer as GLuint);
    gles.BindTexture(gles2::TEXTURE_2D, old_texture as GLuint);
    gles.ActiveTexture(old_active as GLenum);
    gles.UseProgram(old_program as GLuint);
}

fn update_hud_text() {
    let fps = HUD_FPS_TENTHS.load(Ordering::Relaxed) as f32 / 10.0;
    let metrics = HUD_METRICS
        .get_or_init(|| Mutex::new(HudMetrics::default()))
        .lock()
        .ok()
        .map(|metrics| metrics.clone())
        .unwrap_or_default();
    let cpu = metrics
        .cpu_percent
        .map_or_else(|| "--".to_owned(), |value| format!("{value:.0}"));
    let gpu = metrics
        .gpu_percent
        .map_or_else(|| "--".to_owned(), |value| format!("{value:.0}"));
    let ram = metrics
        .ram_mb
        .map_or_else(|| "--".to_owned(), |value| value.to_string());
    let architecture = if metrics.architecture.is_empty() {
        "ARM32/ARM64"
    } else {
        metrics.architecture.as_str()
    };
    if let Some(mutex) = LAST_FPS_TEXT.get() {
        if let Ok(mut value) = mutex.lock() {
            *value = format!("FPS: {fps:.1} CPU: {cpu}% GPU: {gpu}% RAM: {ram}MB {architecture}");
        }
    }
}

fn process_cpu_percent() -> Option<f32> {
    static SAMPLE: OnceLock<Mutex<Option<(u64, u64)>>> = OnceLock::new();
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let fields = stat
        .rsplit_once(") ")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let process_ticks = fields
        .get(11)?
        .parse::<u64>()
        .ok()?
        .checked_add(fields.get(12)?.parse::<u64>().ok()?)?;
    let total_ticks = std::fs::read_to_string("/proc/stat")
        .ok()?
        .lines()
        .find(|line| line.starts_with("cpu "))?
        .split_whitespace()
        .skip(1)
        .filter_map(|value| value.parse::<u64>().ok())
        .sum::<u64>();
    let mutex = SAMPLE.get_or_init(|| Mutex::new(None));
    let mut previous = mutex.lock().ok()?;
    let result = previous.take().and_then(|(old_process, old_total)| {
        let process_delta = process_ticks.saturating_sub(old_process);
        let total_delta = total_ticks.saturating_sub(old_total);
        (total_delta > 0).then(|| {
            let cores = std::thread::available_parallelism().map_or(1, |value| value.get()) as f32;
            (process_delta as f32 / total_delta as f32 * cores * 100.0)
                .clamp(0.0, 100.0 * cores as f32)
        })
    });
    *previous = Some((process_ticks, total_ticks));
    result
}

fn resident_memory_mb() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kilobytes = status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()?;
    Some((kilobytes + 1023) / 1024)
}

fn gpu_percent() -> Option<f32> {
    const PATHS: &[&str] = &[
        "/sys/class/kgsl/kgsl-3d0/gpu_busy_percentage",
        "/sys/class/kgsl/kgsl-3d0/gpu_busy",
        "/sys/class/devfreq/3d00000.gpu/load",
        "/sys/class/devfreq/gpu/load",
    ];
    PATHS.iter().find_map(|path| {
        let value = std::fs::read_to_string(path).ok()?;
        let value = value
            .trim()
            .trim_end_matches('%')
            .split('@')
            .next()?
            .trim()
            .parse::<f32>()
            .ok()?;
        Some(value.clamp(0.0, 100.0))
    })
}

pub fn centered_texture_rotation(rotation_matrix: Matrix<2>) -> Matrix<4> {
    let r = Matrix::<4>::from(&rotation_matrix);
    let to_center = Matrix::<4>::translate_3d(-0.5, -0.5, 0.0);
    let from_center = Matrix::<4>::translate_3d(0.5, 0.5, 0.0);
    from_center.multiply(&r).multiply(&to_center)
}

/// Present the the latest frame (e.g. the app's splash screen or rendering
/// output), provided as a texture bound to `GL_TEXTURE_2D`, by drawing it on
/// the window. It may be rotated, scaled and/or letterboxed as necessary. The
/// virtual cursor is also drawn if it should be currently visible.
///
/// The provided context must be current.
pub unsafe fn present_frame(
    gles: &mut dyn GLES,
    viewport: (u32, u32, u32, u32),
    rotation_matrix: Matrix<2>,
    virtual_cursor_visible_at: Option<(f32, f32, bool)>,
) {
    // While this is a generic utility, it is closely tied to
    // crate::frameworks::opengles::eagl::present_renderbuffer, which handles
    // backing up and restoring OpenGL ES state that this function might touch,
    // so these need to be updated in tandem.

    use gles11::types::*;

    // Draw the quad
    gles.Viewport(
        viewport.0 as _,
        viewport.1 as _,
        viewport.2 as _,
        viewport.3 as _,
    );
    gles.ClearColor(0.0, 0.0, 0.0, 1.0);
    gles.Clear(gles11::COLOR_BUFFER_BIT | gles11::DEPTH_BUFFER_BIT | gles11::STENCIL_BUFFER_BIT);
    gles.BindBuffer(gles11::ARRAY_BUFFER, 0);
    // Stretch the full rendered frame to fill the active host viewport.
    //
    // This does NOT crop or shift the texture. The whole renderbuffer is
    // sampled from normal 0..1 texture coordinates and mapped to a full-screen
    // quad. This is the correct "fill the current window" behavior for
    // PotatoGold-style landscape tests.
    if std::env::var_os("TOUCHHLE_PRESENT_STRETCH_TO_VIEWPORT").is_some() {
        log_once!(
            "TOUCHHLE_PRESENT_STRETCH_TO_VIEWPORT=1: stretching full rendered frame to the active viewport [this log will only be shown once]"
        );
    }

    let vertices: [f32; 12] = [
        -1.0, -1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0, 1.0, 1.0, 1.0,
    ];
    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);

    let tex_coords: [f32; 12] = [0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.TexCoordPointer(2, gles11::FLOAT, 0, tex_coords.as_ptr() as *const GLvoid);
    // Apply the device-rotation matrix to the TEXTURE matrix, but rotate
    // around the centre of the tex coord square (0.5, 0.5) instead of the
    // origin. The naive `LoadMatrixf(rotation_matrix)` rotates around (0, 0),
    // which sends standard [0, 1]² UVs out of range — e.g. for a 90° rotation
    // (v, -u) reaches v' = -u ∈ [-1, 0]. On lenient drivers (Mesa, Apple
    // PowerVR) GL_REPEAT wrap quietly maps that back into [0, 1], but
    // strict drivers (Qualcomm Adreno's native ES 1.1 path) treat the
    // resulting sample of an NPOT texture (renderbuffer is typically
    // 320x480) as undefined and produce a mangled / black presented
    // frame. Pre- and post-translating by (0.5, 0.5) keeps tex coords in
    // [0, 1]² for any 90°/180°/270°/identity device rotation while
    // producing the same visual output as before on lenient drivers.
    let centered_rotation = centered_texture_rotation(rotation_matrix);
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadMatrixf(centered_rotation.columns().as_ptr() as *const _);
    gles.Enable(gles11::TEXTURE_2D);
    gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    // clean this up so we don't need to worry about it in e.g. Core Animation
    gles.LoadIdentity();

    // Display virtual cursor
    if let Some((x, y, pressed)) = virtual_cursor_visible_at {
        let (vx, vy, vw, vh) = viewport;
        let x = x - vx as f32;
        let y = y - vy as f32;

        gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
        gles.Disable(gles11::TEXTURE_2D);

        gles.Enable(gles11::BLEND);
        gles.BlendFunc(gles11::ONE, gles11::ONE_MINUS_SRC_ALPHA);
        gles.Color4f(0.0, 0.0, 0.0, if pressed { 2.0 / 3.0 } else { 1.0 / 3.0 });

        let radius = 10.0;

        let mut vertices = vertices;
        for i in (0..vertices.len()).step_by(2) {
            vertices[i] = (vertices[i] * radius + x) / (vw as f32 / 2.0) - 1.0;
            vertices[i + 1] = 1.0 - (vertices[i + 1] * radius + y) / (vh as f32 / 2.0);
        }
        gles.VertexPointer(2, gles11::FLOAT, 0, vertices.as_ptr() as *const GLvoid);
        gles.DrawArrays(gles11::TRIANGLES, 0, 6);
    }

    // On-screen FPS overlay (simple bitmap font). Enabled by env var
    // TOUCHHLE_ONSCREEN_FPS=1 or by the runtime flag set via
    // crate::gles::present::set_onscreen_fps_enabled(true).
    let onscreen_env = std::env::var_os("TOUCHHLE_ONSCREEN_FPS").is_some();
    let onscreen_runtime = ONSCREEN_FPS_ENABLED
        .get()
        .map(|b| b.load(Ordering::SeqCst))
        .unwrap_or(false);
    if onscreen_env || onscreen_runtime {
        if let Some(mutex) = LAST_FPS_TEXT.get() {
            if let Ok(s) = mutex.lock() {
                if !s.is_empty() {
                    draw_onscreen_text(gles, viewport, &s);
                }
            }
        }
    }
}

// --- Tiny bitmap font & overlay drawing implementation ---
const GLYPH_W: u32 = 8;
const GLYPH_H: u32 = 8;

// Glyphs available in this tiny font: "0123456789:.FPS"
const GLYPH_CHARS: &str = "0123456789:.FPSCUGRAMB%";
// Each glyph is 8 bytes, each bit is a pixel (MSB left).
const GLYPH_BITMAPS: &[[u8; 8]] = &[
    // 0
    [0x3C, 0x66, 0x6E, 0x7E, 0x76, 0x66, 0x3C, 0x00],
    // 1
    [0x18, 0x38, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00],
    // 2
    [0x3C, 0x66, 0x06, 0x0C, 0x18, 0x30, 0x7E, 0x00],
    // 3
    [0x3C, 0x66, 0x06, 0x1C, 0x06, 0x66, 0x3C, 0x00],
    // 4
    [0x0C, 0x1C, 0x3C, 0x6C, 0x7E, 0x0C, 0x1E, 0x00],
    // 5
    [0x7E, 0x60, 0x7C, 0x06, 0x06, 0x66, 0x3C, 0x00],
    // 6
    [0x3C, 0x66, 0x60, 0x7C, 0x66, 0x66, 0x3C, 0x00],
    // 7
    [0x7E, 0x66, 0x0C, 0x18, 0x18, 0x18, 0x18, 0x00],
    // 8
    [0x3C, 0x66, 0x66, 0x3C, 0x66, 0x66, 0x3C, 0x00],
    // 9
    [0x3C, 0x66, 0x66, 0x3E, 0x06, 0x66, 0x3C, 0x00],
    // : (colon)
    [0x00, 0x18, 0x18, 0x00, 0x00, 0x18, 0x18, 0x00],
    // . (dot)
    [0x00, 0x00, 0x00, 0x00, 0x00, 0x18, 0x18, 0x00],
    // F
    [0x7E, 0x60, 0x60, 0x7C, 0x60, 0x60, 0x60, 0x00],
    // P
    [0x7C, 0x66, 0x66, 0x7C, 0x60, 0x60, 0x60, 0x00],
    // S
    [0x3C, 0x66, 0x30, 0x1C, 0x06, 0x66, 0x3C, 0x00],
    // C
    [0x3C, 0x66, 0x60, 0x60, 0x60, 0x66, 0x3C, 0x00],
    // U
    [0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x3C, 0x00],
    // G
    [0x3C, 0x66, 0x60, 0x6E, 0x66, 0x66, 0x3E, 0x00],
    // R
    [0x7C, 0x66, 0x66, 0x7C, 0x6C, 0x66, 0x66, 0x00],
    // A
    [0x18, 0x3C, 0x66, 0x66, 0x7E, 0x66, 0x66, 0x00],
    // M
    [0x63, 0x77, 0x7F, 0x6B, 0x63, 0x63, 0x63, 0x00],
    // B
    [0x7C, 0x66, 0x66, 0x7C, 0x66, 0x66, 0x7C, 0x00],
    // %
    [0x62, 0x64, 0x08, 0x10, 0x26, 0x46, 0x00, 0x00],
];

fn glyph_index(ch: char) -> Option<usize> {
    GLYPH_CHARS.chars().position(|c| c == ch)
}

unsafe fn ensure_glyph_textures(gles: &mut dyn GLES) -> Option<Vec<u32>> {
    use gles11::types::*;
    let lock = GLYPH_TEXTURES
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap();
    if lock.is_some() {
        return lock.clone();
    }
    drop(lock);

    let mut guard = GLYPH_TEXTURES.get().unwrap().lock().unwrap();
    if guard.is_some() {
        return guard.clone();
    }

    let count = GLYPH_BITMAPS.len();
    let mut texs = Vec::with_capacity(count);
    for i in 0..count {
        let mut tex: GLuint = 0;
        gles.GenTextures(1, &mut tex);
        gles.BindTexture(gles11::TEXTURE_2D, tex);
        // Build RGBA data from bitmap
        let mut data = vec![0u8; (GLYPH_W * GLYPH_H * 4) as usize];
        let bmp = GLYPH_BITMAPS[i];
        for y in 0..GLYPH_H {
            let row = bmp[y as usize];
            for x in 0..GLYPH_W {
                let bit = (row >> (7 - x)) & 1;
                let idx = ((y * GLYPH_W + x) * 4) as usize;
                if bit != 0 {
                    data[idx] = 255; // R
                    data[idx + 1] = 255;
                    data[idx + 2] = 255;
                    data[idx + 3] = 255; // A
                } else {
                    data[idx] = 0;
                    data[idx + 1] = 0;
                    data[idx + 2] = 0;
                    data[idx + 3] = 0;
                }
            }
        }
        gles.TexImage2D(
            gles11::TEXTURE_2D,
            0,
            gles11::RGBA as _,
            GLYPH_W as _,
            GLYPH_H as _,
            0,
            gles11::RGBA,
            gles11::UNSIGNED_BYTE,
            data.as_ptr() as *const _,
        );
        gles.TexParameteri(
            gles11::TEXTURE_2D,
            gles11::TEXTURE_MIN_FILTER,
            gles11::NEAREST as _,
        );
        gles.TexParameteri(
            gles11::TEXTURE_2D,
            gles11::TEXTURE_MAG_FILTER,
            gles11::NEAREST as _,
        );
        gles.TexParameteri(
            gles11::TEXTURE_2D,
            gles11::TEXTURE_WRAP_S,
            gles11::CLAMP_TO_EDGE as _,
        );
        gles.TexParameteri(
            gles11::TEXTURE_2D,
            gles11::TEXTURE_WRAP_T,
            gles11::CLAMP_TO_EDGE as _,
        );
        texs.push(tex);
    }
    *guard = Some(texs.clone());
    Some(texs)
}

unsafe fn draw_onscreen_text(gles: &mut dyn GLES, viewport: (u32, u32, u32, u32), text: &str) {
    use gles11::types::*;
    let (vx, vy, vw, vh) = viewport;
    // Pixel size per glyph
    let scale = 6;
    let gw = (GLYPH_W * scale) as f32;
    let gh = (GLYPH_H * scale) as f32;

    // Ensure textures
    let texs_opt = ensure_glyph_textures(gles);
    if texs_opt.is_none() {
        return;
    }
    let texs = texs_opt.unwrap();

    // Save state
    let mut old_active_texture: GLint = 0;
    gles.GetIntegerv(gles11::ACTIVE_TEXTURE, &mut old_active_texture);
    let mut old_texture: GLint = 0;
    gles.GetIntegerv(gles11::TEXTURE_BINDING_2D, &mut old_texture);

    // Setup orthographic projection in pixels
    gles.MatrixMode(gles11::PROJECTION);
    gles.PushMatrix();
    gles.LoadIdentity();
    gles.Orthof(0.0, vw as _, vh as _, 0.0, -1.0, 1.0);
    gles.MatrixMode(gles11::MODELVIEW);
    gles.PushMatrix();
    gles.LoadIdentity();

    // Prepare arrays
    gles.EnableClientState(gles11::VERTEX_ARRAY);
    gles.EnableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.Enable(gles11::TEXTURE_2D);
    gles.Enable(gles11::BLEND);
    gles.BlendFunc(gles11::SRC_ALPHA, gles11::ONE_MINUS_SRC_ALPHA);

    // Draw text at top-left with small margin
    let mut x_px = vx as f32 + 8.0;
    let y_px = vy as f32 + 8.0;

    for ch in text.chars() {
        if let Some(idx) = glyph_index(ch) {
            let tex = texs[idx] as GLint;
            gles.BindTexture(gles11::TEXTURE_2D, tex as _);

            // Quad: two triangles
            let x0 = x_px;
            let y0 = y_px;
            let x1 = x_px + gw;
            let y1 = y_px + gh;
            let verts: [f32; 8] = [x0, y0, x0, y1, x1, y0, x1, y1];
            let texcoords: [f32; 8] = [0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 1.0, 1.0];
            gles.VertexPointer(2, gles11::FLOAT, 0, verts.as_ptr() as *const GLvoid);
            gles.TexCoordPointer(2, gles11::FLOAT, 0, texcoords.as_ptr() as *const GLvoid);
            gles.DrawArrays(gles11::TRIANGLE_STRIP, 0, 4);

            x_px += gw + 2.0;
        } else {
            // Unknown char -> space
            x_px += gw / 2.0;
        }
    }

    // Restore state
    gles.BindTexture(gles11::TEXTURE_2D, old_texture as _);
    gles.ActiveTexture(old_active_texture as _);
    gles.Disable(gles11::BLEND);
    gles.Disable(gles11::TEXTURE_2D);
    gles.DisableClientState(gles11::TEXTURE_COORD_ARRAY);
    gles.DisableClientState(gles11::VERTEX_ARRAY);

    gles.MatrixMode(gles11::MODELVIEW);
    gles.PopMatrix();
    gles.MatrixMode(gles11::PROJECTION);
    gles.PopMatrix();
    gles.MatrixMode(gles11::TEXTURE);
    gles.LoadIdentity();
}
