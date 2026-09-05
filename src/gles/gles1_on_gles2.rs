/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! OpenGL ES 1.1 fixed-function emulation on an OpenGL ES 2.0 context.
//!
//! OpenGL ES 2.0 removed the fixed-function pipeline. This backend keeps the
//! GLES 1.1 API state on the CPU and renders it through a small GLSL ES 1.00
//! program. It is intended for Android devices whose native GLES 1.1 path can
//! render a black frame while their GLES 2.0/3.0 path works correctly.

use super::gles2_raw as gl;
use super::gles2_raw::types::*;
use super::gles11_raw as es1;
use super::gles1_on_gles2_fixes::{rotation_fix_mode, MatrixFixer};
use super::gles1_on_gles2_logging::{self, GLES1to2Logger};
use super::gles_generic::{GLchar, GLES};
use super::util::{fixed_to_float, float_to_fixed, try_decode_pvrtc};
use super::GLESContext;
use crate::window::{GLContext, GLVersion, Window};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::marker::PhantomData;

const VIEWPORT_FIX_VERSION: u32 = 0;
const PROJECTION_FIX_VERSION: u32 = 0;
const ATTR_POSITION: GLuint = 0;
const ATTR_COLOR: GLuint = 1;
const ATTR_NORMAL: GLuint = 2;
const ATTR_TEX0: GLuint = 3;
const ATTR_MATRIX_INDEX: GLuint = 4;
const ATTR_WEIGHT: GLuint = 5;
const ATTR_POINT_SIZE: GLuint = 6;
const MAX_TEXTURE_UNITS: usize = 4;
const MAX_PALETTE_MATRICES: usize = 9;
const MATRIX_IDENTITY: [GLfloat; 16] = [
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 1.0, 0.0,
    0.0, 0.0, 0.0, 1.0,
];

#[derive(Clone, Copy)]
struct ArrayState {
    size: GLint,
    type_: GLenum,
    stride: GLsizei,
    pointer: *const GLvoid,
    buffer_binding: GLuint,
    enabled: bool,
    fixed: bool,
    normalized: bool,
}

impl Default for ArrayState {
    fn default() -> Self {
        Self {
            size: 4,
            type_: gl::FLOAT,
            stride: 0,
            pointer: std::ptr::null(),
            buffer_binding: 0,
            enabled: false,
            fixed: false,
            normalized: false,
        }
    }
}

struct MatrixState {
    current: [GLfloat; 16],
    stack: Vec<[GLfloat; 16]>,
}

impl MatrixState {
    fn new() -> Self {
        Self {
            current: MATRIX_IDENTITY,
            stack: Vec::new(),
        }
    }
}

struct TranslatorState {
    modelview: MatrixState,
    projection: MatrixState,
    texture: [MatrixState; MAX_TEXTURE_UNITS],
    matrix_mode: GLenum,
    active_texture: usize,
    client_active_texture: usize,
    color: [GLfloat; 4],
    normal: [GLfloat; 3],
    texcoords: [[GLfloat; 4]; MAX_TEXTURE_UNITS],
    arrays: [ArrayState; 3],
    texcoord_arrays: [ArrayState; MAX_TEXTURE_UNITS],
    palette_index_array: ArrayState,
    palette_weight_array: ArrayState,
    point_size_array: ArrayState,
    palette_matrices: [MatrixState; MAX_PALETTE_MATRICES],
    current_palette_matrix: usize,
    matrix_palette_enabled: bool,
    texture_enabled: [bool; MAX_TEXTURE_UNITS],
    texture_env_mode: [GLint; MAX_TEXTURE_UNITS],
    texture_env_color: [[GLfloat; 4]; MAX_TEXTURE_UNITS],
    fixed_buffers: [Vec<GLfloat>; 3],
    client_array_vbos: [GLuint; 7],
    client_element_vbo: GLuint,
    array_buffer_binding: GLuint,
    element_array_buffer_binding: GLuint,
    array_buffer_data: HashMap<GLuint, Vec<u8>>,
    element_array_buffer_data: HashMap<GLuint, Vec<u8>>,
    mapped_buffer: Option<(GLenum, GLuint)>,
    point_size: GLfloat,
    alpha_test_enabled: bool,
    alpha_func: GLenum,
    alpha_ref: GLclampf,
    fog_enabled: bool,
    fog_mode: GLenum,
    fog_density: GLfloat,
    fog_start: GLfloat,
    fog_end: GLfloat,
    fog_color: [GLfloat; 4],
    lighting_enabled: bool,
    light0_enabled: bool,
    color_material_enabled: bool,
    normalize_enabled: bool,
    light0_ambient: [GLfloat; 4],
    light0_diffuse: [GLfloat; 4],
    light0_specular: [GLfloat; 4],
    light0_position: [GLfloat; 4],
    material_ambient: [GLfloat; 4],
    material_diffuse: [GLfloat; 4],
    material_specular: [GLfloat; 4],
    material_shininess: GLfloat,
    light0_spot_direction: [GLfloat; 3],
    light0_spot_cutoff: GLfloat,
    light0_spot_exponent: GLfloat,
    light0_constant_attenuation: GLfloat,
    light0_linear_attenuation: GLfloat,
    light0_quadratic_attenuation: GLfloat,
    model_ambient: [GLfloat; 4],
    clip_planes: [[GLfloat; 4]; 6],
    clip_plane_enabled: [bool; 6],
    point_distance_attenuation: [GLfloat; 3],
    point_fade_threshold: GLfloat,
    texture_crop_rect: [GLint; 4],
    viewport: [GLint; 4],
    actual_window_size: (u32, u32),
    first_viewport_logged: bool,
    program: Option<GLuint>,
    program_creation_failed: bool,
    logic_op_enabled: bool,
    logic_op: GLenum,
}

impl TranslatorState {
    fn new() -> Self {
        Self {
            modelview: MatrixState::new(),
            projection: MatrixState::new(),
            texture: std::array::from_fn(|_| MatrixState::new()),
            matrix_mode: es1::MODELVIEW,
            active_texture: 0,
            client_active_texture: 0,
            color: [1.0; 4],
            normal: [0.0, 0.0, 1.0],
            texcoords: [[0.0, 0.0, 0.0, 1.0]; MAX_TEXTURE_UNITS],
            arrays: [ArrayState::default(); 3],
            texcoord_arrays: [ArrayState::default(); MAX_TEXTURE_UNITS],
            palette_index_array: ArrayState::default(),
            palette_weight_array: ArrayState::default(),
            point_size_array: ArrayState::default(),
            palette_matrices: std::array::from_fn(|_| MatrixState::new()),
            current_palette_matrix: 0,
            matrix_palette_enabled: false,
            texture_enabled: [false; MAX_TEXTURE_UNITS],
            texture_env_mode: [es1::MODULATE as GLint; MAX_TEXTURE_UNITS],
            texture_env_color: [[0.0, 0.0, 0.0, 0.0]; MAX_TEXTURE_UNITS],
            fixed_buffers: std::array::from_fn(|_| Vec::new()),
            client_array_vbos: [0; 7],
            client_element_vbo: 0,
            array_buffer_binding: 0,
            element_array_buffer_binding: 0,
            array_buffer_data: HashMap::new(),
            element_array_buffer_data: HashMap::new(),
            mapped_buffer: None,
            point_size: 1.0,
            alpha_test_enabled: false,
            alpha_func: es1::ALWAYS,
            alpha_ref: 0.0,
            fog_enabled: false,
            fog_mode: es1::EXP,
            fog_density: 1.0,
            fog_start: 0.0,
            fog_end: 1.0,
            fog_color: [0.0, 0.0, 0.0, 1.0],
            lighting_enabled: false,
            light0_enabled: false,
            color_material_enabled: false,
            normalize_enabled: false,
            light0_ambient: [0.0, 0.0, 0.0, 1.0],
            light0_diffuse: [1.0, 1.0, 1.0, 1.0],
            light0_specular: [1.0, 1.0, 1.0, 1.0],
            light0_position: [0.0, 0.0, 1.0, 0.0],
            material_ambient: [0.2, 0.2, 0.2, 1.0],
            material_diffuse: [0.8, 0.8, 0.8, 1.0],
            material_specular: [0.0, 0.0, 0.0, 1.0],
            material_shininess: 0.0,
            light0_spot_direction: [0.0, 0.0, -1.0],
            light0_spot_cutoff: 180.0,
            light0_spot_exponent: 0.0,
            light0_constant_attenuation: 1.0,
            light0_linear_attenuation: 0.0,
            light0_quadratic_attenuation: 0.0,
            model_ambient: [0.2, 0.2, 0.2, 1.0],
            clip_planes: [[0.0, 0.0, 0.0, 0.0]; 6],
            clip_plane_enabled: [false; 6],
            point_distance_attenuation: [1.0, 0.0, 0.0],
            point_fade_threshold: 1.0,
            texture_crop_rect: [0, 0, 0, 0],
            viewport: [0, 0, 0, 0],
            actual_window_size: (0, 0),
            first_viewport_logged: false,
            program: None,
            program_creation_failed: false,
            logic_op_enabled: false,
            logic_op: es1::COPY,
        }
    }

    fn matrix_mut(&mut self) -> &mut MatrixState {
        match self.matrix_mode {
            es1::PROJECTION => &mut self.projection,
            es1::TEXTURE => &mut self.texture[self.active_texture],
            es1::MATRIX_PALETTE_OES => &mut self.palette_matrices[self.current_palette_matrix],
            _ => &mut self.modelview,
        }
    }

    fn mvp(&self) -> [GLfloat; 16] {
        let mut matrix = apply_projection_fix(multiply(&self.projection.current, &self.modelview.current));
        let logger = GLES1to2Logger::new("mvp_upload", "GLES2 vertex shader");
        let result = MatrixFixer::apply_all_fixes(&mut matrix, &logger);
        logger.finish();
        result
    }
}

fn coordinate_trace_enabled() -> bool {
    crate::gles::translator_tracing_enabled()
}

fn matrix_mode_name(mode: GLenum) -> &'static str {
    match mode {
        es1::MODELVIEW => "GL_MODELVIEW",
        es1::PROJECTION => "GL_PROJECTION",
        es1::TEXTURE => "GL_TEXTURE",
        es1::MATRIX_PALETTE_OES => "GL_MATRIX_PALETTE_OES",
        _ => "UNKNOWN",
    }
}

fn log_matrix_operation(operation: &str, details: String) {
    if coordinate_trace_enabled() {
        log!("[GLES1→GLES2 MATRIX] {}: {}", operation, details);
    }
}

fn log_matrix(label: &str, matrix: &[GLfloat; 16]) {
    if !coordinate_trace_enabled() {
        return;
    }
    log!("[GLES1→GLES2 MATRIX] {}:", label);
    for row in 0..4 {
        log!(
            "[GLES1→GLES2 MATRIX]   [{:.6} {:.6} {:.6} {:.6}]",
            matrix[row * 4],
            matrix[row * 4 + 1],
            matrix[row * 4 + 2],
            matrix[row * 4 + 3]
        );
    }
}
fn log_matrix_result(operation: &str, matrix: &[GLfloat; 16]) {
    log_matrix(&format!("after {operation}"), matrix);
}

fn log_viewport(actual_width: u32, actual_height: u32, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
    if !coordinate_trace_enabled() {
        return;
    }
    let requested_aspect = if height > 0 { width as f32 / height as f32 } else { 0.0 };
    let actual_aspect = if actual_height > 0 {
        actual_width as f32 / actual_height as f32
    } else {
        0.0
    };
    let aspect_mismatch = actual_aspect > 0.0
        && (requested_aspect - actual_aspect).abs() > 0.01;
    log!(
        "[GLES1→GLES2] glViewport called: x={}, y={}, width={}, height={}, requested_aspect={:.3}, drawable={}x{}, drawable_aspect={:.3}, aspect_mismatch={}",
        x, y, width, height, requested_aspect, actual_width, actual_height, actual_aspect, aspect_mismatch
    );
    if aspect_mismatch {
        log!(
            "[GLES1→GLES2 VIEWPORT MISMATCH WARNING] game_requested={}x{} aspect={:.3}, drawable={}x{} aspect={:.3}",
            width, height, requested_aspect, actual_width, actual_height, actual_aspect
        );
    }
}

fn apply_viewport(x: GLint, y: GLint, width: GLsizei, height: GLsizei) -> (GLint, GLint, GLsizei, GLsizei) {
    match VIEWPORT_FIX_VERSION {
        1 => (x, y, height, width),
        2 => (x, -y, width, height),
        3 => (x, -y, height, width),
        _ => (x, y, width, height),
    }
}

fn diagnose_matrix_conversion(gles1_matrix: &[GLfloat; 16], gles2_matrix: &[GLfloat; 16]) {
    if !coordinate_trace_enabled() {
        return;
    }
    log!("[GLES1→GLES2 MATRIX CONVERSION DIAGNOSIS]");
    log_matrix("GLES1 tracked matrix", gles1_matrix);
    log_matrix("GLES2 upload matrix", gles2_matrix);
    if gles1_matrix[0] < 0.0 && gles2_matrix[0] > 0.0 {
        log!("[GLES1→GLES2 SIGN FLIP DETECTED] X-axis may be inverted");
    }
    if gles1_matrix[5] < 0.0 && gles2_matrix[5] > 0.0 {
        log!("[GLES1→GLES2 SIGN FLIP DETECTED] Y-axis may be inverted");
    }
    if (gles1_matrix[0] - gles2_matrix[0]).abs() > 0.001 {
        log!("[GLES1→GLES2 SCALE CHANGE] X-axis scaling changed");
    }
    if (gles1_matrix[5] - gles2_matrix[5]).abs() > 0.001 {
        log!("[GLES1→GLES2 SCALE CHANGE] Y-axis scaling changed");
    }
}

fn apply_projection_fix(mut matrix: [GLfloat; 16]) -> [GLfloat; 16] {
    if PROJECTION_FIX_VERSION == 1 {
        matrix[5] = -matrix[5];
    }
    matrix
}

fn transform_vec4(matrix: &[GLfloat; 16], value: [GLfloat; 4]) -> [GLfloat; 4] {
    [
        matrix[0] * value[0] + matrix[4] * value[1] + matrix[8] * value[2] + matrix[12] * value[3],
        matrix[1] * value[0] + matrix[5] * value[1] + matrix[9] * value[2] + matrix[13] * value[3],
        matrix[2] * value[0] + matrix[6] * value[1] + matrix[10] * value[2] + matrix[14] * value[3],
        matrix[3] * value[0] + matrix[7] * value[1] + matrix[11] * value[2] + matrix[15] * value[3],
    ]
}

fn log_vertex_transformation(original: [GLfloat; 3], transformed: [GLfloat; 4]) {
    if !gles1_on_gles2_logging::enabled() {
        return;
    }
    let logger = GLES1to2Logger::new("vertex_transform", "GLES2 vertex shader");
    logger.log_vertex_batch("sample", &[original], 1);
    logger.log_vertex_transformation(original, transformed);
    logger.finish();
}

pub struct GLES1OnGLES2Context {
    gl_ctx: GLContext,
    is_loaded: bool,
    state: TranslatorState,
}

impl GLESContext for GLES1OnGLES2Context {
    fn description() -> &'static str {
        "OpenGL ES 1.1 translated to native OpenGL ES 2.0 shaders"
    }

    fn new(window: &mut Window) -> Result<Self, String> {
        gles1_on_gles2_logging::log_initialization(rotation_fix_mode().as_str());
        Ok(Self {
            gl_ctx: window.create_gl_context(GLVersion::GLES20)?,
            is_loaded: false,
            state: TranslatorState::new(),
        })
    }

    fn make_current<'gl_ctx, 'win: 'gl_ctx>(
        &'gl_ctx mut self,
        window: &'win mut Window,
    ) -> Box<dyn GLES + 'gl_ctx> {
        if !self.gl_ctx.is_current() || !self.is_loaded {
            unsafe { window.make_gl_context_current(&self.gl_ctx) };
            gl::load_with(|s| window.gl_get_proc_address(s));
            es1::load_with(|s| window.gl_get_proc_address(s));
            self.is_loaded = true;
        }
        let logical_framebuffer_size = window.framebuffer_size();
        let drawable_size = window.drawable_size();
        if coordinate_trace_enabled() {
            log!(
                "[GLES1→GLES2 WINDOW] orientation={:?} logical_framebuffer={}x{} drawable={}x{} viewport={:?}",
                window.current_rotation(),
                logical_framebuffer_size.0,
                logical_framebuffer_size.1,
                drawable_size.0,
                drawable_size.1,
                window.viewport(),
            );
        }
        self.state.actual_window_size = drawable_size;
        Box::new(GLES1OnGLES2 {
            state: &mut self.state,
            _gl_lifetime: PhantomData,
        })
    }

    unsafe fn make_current_unchecked_for_window<'gl_ctx>(
        &'gl_ctx mut self,
        make_current_fn: &mut dyn FnMut(&GLContext),
        loader_fn: &mut dyn FnMut(&'static str) -> *const std::ffi::c_void,
    ) -> Box<dyn GLES + 'gl_ctx> {
        if !self.gl_ctx.is_current() || !self.is_loaded {
            make_current_fn(&self.gl_ctx);
            gl::load_with(&mut *loader_fn);
            es1::load_with(&mut *loader_fn);
            self.is_loaded = true;
        }
        Box::new(GLES1OnGLES2 {
            state: &mut self.state,
            _gl_lifetime: PhantomData,
        })
    }
}

pub struct GLES1OnGLES2<'a> {
    state: &'a mut TranslatorState,
    _gl_lifetime: PhantomData<&'a ()>,
}

fn multiply(a: &[GLfloat; 16], b: &[GLfloat; 16]) -> [GLfloat; 16] {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    out
}

fn translation(x: GLfloat, y: GLfloat, z: GLfloat) -> [GLfloat; 16] {
    let mut m = MATRIX_IDENTITY;
    m[12] = x;
    m[13] = y;
    m[14] = z;
    m
}

fn scaling(x: GLfloat, y: GLfloat, z: GLfloat) -> [GLfloat; 16] {
    let mut m = MATRIX_IDENTITY;
    m[0] = x;
    m[5] = y;
    m[10] = z;
    m
}

fn rotation(angle: GLfloat, x: GLfloat, y: GLfloat, z: GLfloat) -> [GLfloat; 16] {
    let length = (x * x + y * y + z * z).sqrt();
    if length == 0.0 {
        return MATRIX_IDENTITY;
    }
    let (x, y, z) = (x / length, y / length, z / length);
    let r = angle.to_radians();
    let (s, c) = (r.sin(), r.cos());
    let t = 1.0 - c;
    [
        t * x * x + c, t * x * y + s * z, t * x * z - s * y, 0.0,
        t * x * y - s * z, t * y * y + c, t * y * z + s * x, 0.0,
        t * x * z + s * y, t * y * z - s * x, t * z * z + c, 0.0,
        0.0, 0.0, 0.0, 1.0,
    ]
}

fn ortho(left: GLfloat, right: GLfloat, bottom: GLfloat, top: GLfloat, near: GLfloat, far: GLfloat) -> [GLfloat; 16] {
    [
        2.0 / (right - left), 0.0, 0.0, 0.0,
        0.0, 2.0 / (top - bottom), 0.0, 0.0,
        0.0, 0.0, -2.0 / (far - near), 0.0,
        -(right + left) / (right - left), -(top + bottom) / (top - bottom),
        -(far + near) / (far - near), 1.0,
    ]
}

fn frustum(left: GLfloat, right: GLfloat, bottom: GLfloat, top: GLfloat, near: GLfloat, far: GLfloat) -> [GLfloat; 16] {
    [
        2.0 * near / (right - left), 0.0, 0.0, 0.0,
        0.0, 2.0 * near / (top - bottom), 0.0, 0.0,
        (right + left) / (right - left), (top + bottom) / (top - bottom),
        -(far + near) / (far - near), -1.0,
        0.0, 0.0, -2.0 * far * near / (far - near), 0.0,
    ]
}

fn compile_shader(kind: GLenum, source: &str) -> Result<GLuint, String> {
    unsafe {
        let shader = gl::CreateShader(kind);
        let source = CString::new(source).unwrap();
        let pointer = source.as_ptr();
        gl::ShaderSource(shader, 1, &pointer, std::ptr::null());
        gl::CompileShader(shader);
        let mut ok = 0;
        gl::GetShaderiv(shader, gl::COMPILE_STATUS, &mut ok);
        if ok == 0 {
            let mut log = [0i8; 2048];
            let mut len = 0;
            gl::GetShaderInfoLog(shader, log.len() as GLsizei, &mut len, log.as_mut_ptr() as _);
            return Err(format!(
                "GLES1-on-GLES2 shader compilation failed: {}",
                String::from_utf8_lossy(std::slice::from_raw_parts(log.as_ptr() as *const u8, len.max(0) as usize))
            ));
        }
        Ok(shader)
    }
}

fn create_program() -> Result<GLuint, String> {
    let vertex = compile_shader(gl::VERTEX_SHADER, r#"#version 100
precision mediump float;
attribute vec4 a_position;
attribute vec4 a_color;
attribute vec3 a_normal;
attribute vec4 a_tex0;
attribute vec4 a_matrix_index;
attribute vec4 a_weight;
attribute float a_point_size;
uniform mat4 u_mvp;
uniform mat4 u_modelview;
uniform mat4 u_texture_matrix0;
uniform vec4 u_color;
uniform float u_point_size;
uniform int u_point_size_array_enabled;
uniform mat4 u_palette_matrices[9];
uniform int u_matrix_palette_enabled;
uniform int u_lighting_enabled;
uniform int u_light0_enabled;
uniform int u_color_material_enabled;
uniform int u_normalize_enabled;
uniform vec4 u_light0_ambient;
uniform vec4 u_light0_diffuse;
uniform vec4 u_light0_position;
uniform vec3 u_light0_spot_direction;
uniform float u_light0_spot_cutoff;
uniform float u_light0_spot_exponent;
uniform float u_light0_constant_attenuation;
uniform float u_light0_linear_attenuation;
uniform float u_light0_quadratic_attenuation;
uniform vec4 u_material_ambient;
uniform vec4 u_material_diffuse;
uniform vec4 u_model_ambient;
uniform vec4 u_clip_planes[6];
uniform int u_clip_enabled[6];
uniform vec3 u_point_distance_attenuation;
uniform float u_point_fade_threshold;
varying vec4 v_color;
varying vec2 v_tex0;
varying float v_fog_coord;
varying vec4 v_clip_distances0;
varying vec2 v_clip_distances1;
void main() {
    vec4 transformed_position = a_position;
    if (u_matrix_palette_enabled != 0) {
        transformed_position = vec4(0.0);
        for (int i = 0; i < 4; i++) {
            int matrix_index = int(a_matrix_index[i]);
            matrix_index = matrix_index < 0 ? 0 : matrix_index;
            matrix_index = matrix_index > 8 ? 8 : matrix_index;
            transformed_position += a_weight[i] * (u_palette_matrices[matrix_index] * a_position);
        }
    }
    vec4 eye_position = u_modelview * transformed_position;
    gl_Position = u_mvp * transformed_position;
    float point_distance = length(eye_position.xyz);
    float point_attenuation = sqrt(max(u_point_distance_attenuation.x + u_point_distance_attenuation.y * point_distance + u_point_distance_attenuation.z * point_distance * point_distance, 0.0001));
    gl_PointSize = (u_point_size_array_enabled != 0 ? a_point_size : u_point_size) / point_attenuation;
    vec3 transformed_normal = (u_modelview * vec4(a_normal, 0.0)).xyz;
    if (u_normalize_enabled != 0) transformed_normal = normalize(transformed_normal);
    vec4 base_color = a_color * u_color;
    if (u_lighting_enabled != 0 && u_light0_enabled != 0) {
        vec3 light_direction = u_light0_position.w == 0.0 ? normalize(u_light0_position.xyz) : normalize(u_light0_position.xyz - eye_position.xyz);
        float distance_to_light = u_light0_position.w == 0.0 ? 1.0 : length(u_light0_position.xyz - eye_position.xyz);
        float attenuation = 1.0 / (u_light0_constant_attenuation + u_light0_linear_attenuation * distance_to_light + u_light0_quadratic_attenuation * distance_to_light * distance_to_light);
        float spot_factor = 1.0;
        if (u_light0_position.w != 0.0 && u_light0_spot_cutoff < 180.0) {
            float spot_cos = dot(normalize(u_light0_spot_direction), normalize(eye_position.xyz - u_light0_position.xyz));
            spot_factor = spot_cos < cos(radians(u_light0_spot_cutoff)) ? 0.0 : pow(max(spot_cos, 0.0), u_light0_spot_exponent);
        }
        float diffuse_factor = max(dot(normalize(transformed_normal), light_direction), 0.0) * attenuation * spot_factor;
        vec4 material_diffuse = u_color_material_enabled != 0 ? base_color : u_material_diffuse;
        vec4 material_ambient = u_color_material_enabled != 0 ? base_color : u_material_ambient;
        vec3 lit_rgb = u_model_ambient.rgb * material_ambient.rgb + u_light0_ambient.rgb * material_ambient.rgb + u_light0_diffuse.rgb * material_diffuse.rgb * diffuse_factor;
        v_color = vec4(lit_rgb, material_diffuse.a);
    } else {
        v_color = base_color;
    }
    v_tex0 = (u_texture_matrix0 * a_tex0).xy;
    v_fog_coord = abs(eye_position.z);
    v_clip_distances0 = vec4(dot(u_clip_planes[0], eye_position), dot(u_clip_planes[1], eye_position), dot(u_clip_planes[2], eye_position), dot(u_clip_planes[3], eye_position));
    v_clip_distances1 = vec2(dot(u_clip_planes[4], eye_position), dot(u_clip_planes[5], eye_position));
}
"#)?;
    let fragment = compile_shader(gl::FRAGMENT_SHADER, r#"#version 100
precision mediump float;
varying vec4 v_color;
varying vec2 v_tex0;
varying float v_fog_coord;
varying vec4 v_clip_distances0;
varying vec2 v_clip_distances1;
uniform sampler2D u_tex0;
uniform vec4 u_env_color0;
uniform int u_tex_enabled0;
uniform int u_tex_mode0;
uniform int u_alpha_test_enabled;
uniform int u_alpha_func;
uniform float u_alpha_ref;
uniform int u_fog_enabled;
uniform vec4 u_fog_color;
uniform float u_fog_density;
uniform float u_fog_start;
uniform float u_fog_end;
uniform int u_fog_mode;
uniform int u_clip_enabled[6];
uniform int u_logic_op_enabled;
uniform int u_logic_op;
float fog_factor() {
    if (u_fog_mode == 2048) return exp(-u_fog_density * v_fog_coord);
    if (u_fog_mode == 2049) {
        float d = u_fog_density * v_fog_coord;
        return exp(-d * d);
    }
    return (u_fog_end - v_fog_coord) / (u_fog_end - u_fog_start);
}
bool alpha_pass(float alpha) {
    if (u_alpha_func == 512) return false;
    if (u_alpha_func == 513) return alpha < u_alpha_ref;
    if (u_alpha_func == 514) return alpha == u_alpha_ref;
    if (u_alpha_func == 515) return alpha <= u_alpha_ref;
    if (u_alpha_func == 516) return alpha > u_alpha_ref;
    if (u_alpha_func == 517) return alpha != u_alpha_ref;
    if (u_alpha_func == 518) return alpha >= u_alpha_ref;
    return true;
}
void main() {
    vec4 color = v_color;
    if (u_tex_enabled0 != 0) {
        vec4 texel = texture2D(u_tex0, v_tex0);
        if (u_tex_mode0 == 1) color = texel;
        else if (u_tex_mode0 == 2) color = color * texel;
        else if (u_tex_mode0 == 3) color = vec4(color.rgb + texel.rgb, color.a * texel.a);
        else if (u_tex_mode0 == 4) color = vec4(mix(color.rgb, texel.rgb, texel.a), color.a);
    }
    if (u_alpha_test_enabled != 0 && !alpha_pass(color.a)) discard;
    if (u_clip_enabled[0] != 0 && v_clip_distances0.x < 0.0) discard;
    if (u_clip_enabled[1] != 0 && v_clip_distances0.y < 0.0) discard;
    if (u_clip_enabled[2] != 0 && v_clip_distances0.z < 0.0) discard;
    if (u_clip_enabled[3] != 0 && v_clip_distances0.w < 0.0) discard;
    if (u_clip_enabled[4] != 0 && v_clip_distances1.x < 0.0) discard;
    if (u_clip_enabled[5] != 0 && v_clip_distances1.y < 0.0) discard;
    if (u_fog_enabled != 0) {
        float factor = clamp(fog_factor(), 0.0, 1.0);
        color = mix(u_fog_color, color, factor);
    }
    if (u_logic_op_enabled != 0) {
        vec4 rounded = floor(color * 255.0 + 0.5) / 255.0;
        if (u_logic_op == 538) color = vec4(1.0) - rounded;
        else if (u_logic_op == 539) color = rounded;
    }
    gl_FragColor = color;
}
"#)?;
    unsafe {
        let program = gl::CreateProgram();
        gl::AttachShader(program, vertex);
        gl::AttachShader(program, fragment);
        gl::BindAttribLocation(program, ATTR_POSITION, b"a_position\0".as_ptr() as *const GLchar);
        gl::BindAttribLocation(program, ATTR_COLOR, b"a_color\0".as_ptr() as *const GLchar);
        gl::BindAttribLocation(program, ATTR_NORMAL, b"a_normal\0".as_ptr() as *const GLchar);
        gl::BindAttribLocation(program, ATTR_TEX0, b"a_tex0\0".as_ptr() as *const GLchar);
        gl::BindAttribLocation(program, ATTR_MATRIX_INDEX, b"a_matrix_index\0".as_ptr() as *const GLchar);
        gl::BindAttribLocation(program, ATTR_WEIGHT, b"a_weight\0".as_ptr() as *const GLchar);
        gl::BindAttribLocation(program, ATTR_POINT_SIZE, b"a_point_size\0".as_ptr() as *const GLchar);
        gl::LinkProgram(program);
        let mut ok = 0;
        gl::GetProgramiv(program, gl::LINK_STATUS, &mut ok);
        if ok == 0 {
            let mut log = [0i8; 2048];
            let mut len = 0;
            gl::GetProgramInfoLog(program, log.len() as GLsizei, &mut len, log.as_mut_ptr() as _);
            gl::DeleteShader(vertex);
            gl::DeleteShader(fragment);
            return Err(format!(
                "GLES1-on-GLES2 program link failed: {}",
                String::from_utf8_lossy(std::slice::from_raw_parts(log.as_ptr() as *const u8, len.max(0) as usize))
            ));
        }
        gl::DeleteShader(vertex);
        gl::DeleteShader(fragment);
        Ok(program)
    }
}

impl GLES for GLES1OnGLES2<'_> {
    fn is_es2(&self) -> bool {
        true
    }

    unsafe fn driver_description(&self) -> String {
        let version = CStr::from_ptr(gl::GetString(gl::VERSION) as *const _);
        let vendor = CStr::from_ptr(gl::GetString(gl::VENDOR) as *const _);
        let renderer = CStr::from_ptr(gl::GetString(gl::RENDERER) as *const _);
        format!("GLES1 translated by GLES2 / {} / {} / {}", version.to_string_lossy(), vendor.to_string_lossy(), renderer.to_string_lossy())
    }

    unsafe fn CreateShader(&mut self, type_: GLenum) -> GLuint { gl::CreateShader(type_) }
    unsafe fn DeleteShader(&mut self, shader: GLuint) { gl::DeleteShader(shader); }
    unsafe fn ShaderSource(&mut self, shader: GLuint, count: GLsizei, string: *const *const GLchar, length: *const GLint) { gl::ShaderSource(shader, count, string, length); }
    unsafe fn CompileShader(&mut self, shader: GLuint) { gl::CompileShader(shader); }
    unsafe fn GetShaderiv(&mut self, shader: GLuint, pname: GLenum, params: *mut GLint) { gl::GetShaderiv(shader, pname, params); }
    unsafe fn GetShaderInfoLog(&mut self, shader: GLuint, max_length: GLsizei, length: *mut GLsizei, info_log: *mut GLchar) { gl::GetShaderInfoLog(shader, max_length, length, info_log); }
    unsafe fn IsShader(&mut self, shader: GLuint) -> GLboolean { gl::IsShader(shader) }
    unsafe fn CreateProgram(&mut self) -> GLuint { gl::CreateProgram() }
    unsafe fn DeleteProgram(&mut self, program: GLuint) { gl::DeleteProgram(program); }
    unsafe fn AttachShader(&mut self, program: GLuint, shader: GLuint) { gl::AttachShader(program, shader); }
    unsafe fn DetachShader(&mut self, program: GLuint, shader: GLuint) { gl::DetachShader(program, shader); }
    unsafe fn LinkProgram(&mut self, program: GLuint) { gl::LinkProgram(program); }
    unsafe fn UseProgram(&mut self, program: GLuint) { gl::UseProgram(program); }
    unsafe fn GetProgramiv(&mut self, program: GLuint, pname: GLenum, params: *mut GLint) { gl::GetProgramiv(program, pname, params); }
    unsafe fn GetProgramInfoLog(&mut self, program: GLuint, max_length: GLsizei, length: *mut GLsizei, info_log: *mut GLchar) { gl::GetProgramInfoLog(program, max_length, length, info_log); }
    unsafe fn IsProgram(&mut self, program: GLuint) -> GLboolean { gl::IsProgram(program) }
    unsafe fn ValidateProgram(&mut self, program: GLuint) { gl::ValidateProgram(program); }
    unsafe fn BindAttribLocation(&mut self, program: GLuint, index: GLuint, name: *const GLchar) { gl::BindAttribLocation(program, index, name); }
    unsafe fn GetAttribLocation(&mut self, program: GLuint, name: *const GLchar) -> GLint { gl::GetAttribLocation(program, name) }
    unsafe fn GetUniformLocation(&mut self, program: GLuint, name: *const GLchar) -> GLint { gl::GetUniformLocation(program, name) }
    unsafe fn GetActiveAttrib(&mut self, program: GLuint, index: GLuint, buf_size: GLsizei, length: *mut GLsizei, size: *mut GLint, type_: *mut GLenum, name: *mut GLchar) { gl::GetActiveAttrib(program, index, buf_size, length, size, type_, name); }
    unsafe fn GetActiveUniform(&mut self, program: GLuint, index: GLuint, buf_size: GLsizei, length: *mut GLsizei, size: *mut GLint, type_: *mut GLenum, name: *mut GLchar) { gl::GetActiveUniform(program, index, buf_size, length, size, type_, name); }
    unsafe fn EnableVertexAttribArray(&mut self, index: GLuint) { gl::EnableVertexAttribArray(index); }
    unsafe fn DisableVertexAttribArray(&mut self, index: GLuint) { gl::DisableVertexAttribArray(index); }
    unsafe fn VertexAttribPointer(&mut self, index: GLuint, size: GLint, type_: GLenum, normalized: GLboolean, stride: GLsizei, pointer: *const GLvoid) { gl::VertexAttribPointer(index, size, type_, normalized, stride, pointer); }
    unsafe fn VertexAttrib1f(&mut self, index: GLuint, x: GLfloat) { gl::VertexAttrib1f(index, x); }
    unsafe fn VertexAttrib2f(&mut self, index: GLuint, x: GLfloat, y: GLfloat) { gl::VertexAttrib2f(index, x, y); }
    unsafe fn VertexAttrib3f(&mut self, index: GLuint, x: GLfloat, y: GLfloat, z: GLfloat) { gl::VertexAttrib3f(index, x, y, z); }
    unsafe fn VertexAttrib4f(&mut self, index: GLuint, x: GLfloat, y: GLfloat, z: GLfloat, w: GLfloat) { gl::VertexAttrib4f(index, x, y, z, w); }
    unsafe fn VertexAttrib1fv(&mut self, index: GLuint, v: *const GLfloat) { gl::VertexAttrib1fv(index, v); }
    unsafe fn VertexAttrib2fv(&mut self, index: GLuint, v: *const GLfloat) { gl::VertexAttrib2fv(index, v); }
    unsafe fn VertexAttrib3fv(&mut self, index: GLuint, v: *const GLfloat) { gl::VertexAttrib3fv(index, v); }
    unsafe fn VertexAttrib4fv(&mut self, index: GLuint, v: *const GLfloat) { gl::VertexAttrib4fv(index, v); }
    unsafe fn Uniform1f(&mut self, location: GLint, v0: GLfloat) { gl::Uniform1f(location, v0); }
    unsafe fn Uniform2f(&mut self, location: GLint, v0: GLfloat, v1: GLfloat) { gl::Uniform2f(location, v0, v1); }
    unsafe fn Uniform3f(&mut self, location: GLint, v0: GLfloat, v1: GLfloat, v2: GLfloat) { gl::Uniform3f(location, v0, v1, v2); }
    unsafe fn Uniform4f(&mut self, location: GLint, v0: GLfloat, v1: GLfloat, v2: GLfloat, v3: GLfloat) { gl::Uniform4f(location, v0, v1, v2, v3); }
    unsafe fn Uniform1i(&mut self, location: GLint, v0: GLint) { gl::Uniform1i(location, v0); }
    unsafe fn Uniform2i(&mut self, location: GLint, v0: GLint, v1: GLint) { gl::Uniform2i(location, v0, v1); }
    unsafe fn Uniform3i(&mut self, location: GLint, v0: GLint, v1: GLint, v2: GLint) { gl::Uniform3i(location, v0, v1, v2); }
    unsafe fn Uniform4i(&mut self, location: GLint, v0: GLint, v1: GLint, v2: GLint, v3: GLint) { gl::Uniform4i(location, v0, v1, v2, v3); }
    unsafe fn Uniform1fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) { gl::Uniform1fv(location, count, value); }
    unsafe fn Uniform2fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) { gl::Uniform2fv(location, count, value); }
    unsafe fn Uniform3fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) { gl::Uniform3fv(location, count, value); }
    unsafe fn Uniform4fv(&mut self, location: GLint, count: GLsizei, value: *const GLfloat) { gl::Uniform4fv(location, count, value); }
    unsafe fn Uniform1iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) { gl::Uniform1iv(location, count, value); }
    unsafe fn Uniform2iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) { gl::Uniform2iv(location, count, value); }
    unsafe fn Uniform3iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) { gl::Uniform3iv(location, count, value); }
    unsafe fn Uniform4iv(&mut self, location: GLint, count: GLsizei, value: *const GLint) { gl::Uniform4iv(location, count, value); }
    unsafe fn UniformMatrix2fv(&mut self, location: GLint, count: GLsizei, transpose: GLboolean, value: *const GLfloat) { gl::UniformMatrix2fv(location, count, transpose, value); }
    unsafe fn UniformMatrix3fv(&mut self, location: GLint, count: GLsizei, transpose: GLboolean, value: *const GLfloat) { gl::UniformMatrix3fv(location, count, transpose, value); }
    unsafe fn UniformMatrix4fv(&mut self, location: GLint, count: GLsizei, transpose: GLboolean, value: *const GLfloat) { gl::UniformMatrix4fv(location, count, transpose, value); }
    unsafe fn GetShaderSource(&mut self, shader: GLuint, buf_size: GLsizei, length: *mut GLsizei, source: *mut GLchar) { gl::GetShaderSource(shader, buf_size, length, source); }
    unsafe fn GetAttachedShaders(&mut self, program: GLuint, max_count: GLsizei, count: *mut GLsizei, shaders: *mut GLuint) { gl::GetAttachedShaders(program, max_count, count, shaders); }
    unsafe fn GetUniformiv(&mut self, program: GLuint, location: GLint, params: *mut GLint) { gl::GetUniformiv(program, location, params); }
    unsafe fn GetUniformfv(&mut self, program: GLuint, location: GLint, params: *mut GLfloat) { gl::GetUniformfv(program, location, params); }
    unsafe fn GetShaderPrecisionFormat(&mut self, shader_type: GLenum, precision_type: GLenum, range: *mut GLint, precision: *mut GLint) { gl::GetShaderPrecisionFormat(shader_type, precision_type, range, precision); }
    unsafe fn ReleaseShaderCompiler(&mut self) { gl::ReleaseShaderCompiler(); }
    unsafe fn ShaderBinary(&mut self, count: GLsizei, shaders: *const GLuint, binary_format: GLenum, binary: *const GLvoid, length: GLsizei) { gl::ShaderBinary(count, shaders, binary_format, binary, length); }

    unsafe fn GetError(&mut self) -> GLenum { gl::GetError() }
    unsafe fn GetString(&mut self, name: GLenum) -> *const GLubyte { gl::GetString(name) }
    unsafe fn GetBooleanv(&mut self, pname: GLenum, params: *mut GLboolean) {
        match pname {
            es1::TEXTURE_2D => *params = if self.state.texture_enabled[self.state.active_texture] { gl::TRUE } else { gl::FALSE },
            es1::ALPHA_TEST => *params = if self.state.alpha_test_enabled { gl::TRUE } else { gl::FALSE },
            es1::FOG => *params = if self.state.fog_enabled { gl::TRUE } else { gl::FALSE },
            es1::LIGHTING => *params = if self.state.lighting_enabled { gl::TRUE } else { gl::FALSE },
            es1::LIGHT0 => *params = if self.state.light0_enabled { gl::TRUE } else { gl::FALSE },
            es1::COLOR_MATERIAL => *params = if self.state.color_material_enabled { gl::TRUE } else { gl::FALSE },
            es1::NORMALIZE => *params = if self.state.normalize_enabled { gl::TRUE } else { gl::FALSE },
            es1::CLIP_PLANE0..=es1::CLIP_PLANE5 => {
                let index = (pname - es1::CLIP_PLANE0) as usize;
                *params = if self.state.clip_plane_enabled[index] { gl::TRUE } else { gl::FALSE };
            }
            _ => gl::GetBooleanv(pname, params),
        }
    }
    unsafe fn GetFloatv(&mut self, pname: GLenum, params: *mut GLfloat) {
        match pname {
            es1::MODELVIEW_MATRIX => params.copy_from(self.state.modelview.current.as_ptr(), 16),
            es1::PROJECTION_MATRIX => params.copy_from(self.state.projection.current.as_ptr(), 16),
            es1::TEXTURE_MATRIX => params.copy_from(self.state.texture[self.state.active_texture].current.as_ptr(), 16),
            es1::CURRENT_COLOR => params.copy_from(self.state.color.as_ptr(), 4),
            es1::CURRENT_NORMAL => params.copy_from(self.state.normal.as_ptr(), 3),
            es1::FOG_COLOR => params.copy_from(self.state.fog_color.as_ptr(), 4),
            es1::FOG_DENSITY => *params = self.state.fog_density,
            es1::FOG_START => *params = self.state.fog_start,
            es1::FOG_END => *params = self.state.fog_end,
            es1::POINT_SIZE => *params = self.state.point_size,
            es1::CURRENT_TEXTURE_COORDS => params.copy_from(self.state.texcoords[self.state.active_texture].as_ptr(), 4),
            es1::POINT_DISTANCE_ATTENUATION => params.copy_from(self.state.point_distance_attenuation.as_ptr(), 3),
            es1::POINT_FADE_THRESHOLD_SIZE => *params = self.state.point_fade_threshold,
            es1::CLIP_PLANE0..=es1::CLIP_PLANE5 => {
                let index = (pname - es1::CLIP_PLANE0) as usize;
                params.copy_from(self.state.clip_planes[index].as_ptr(), 4);
            }
            _ => gl::GetFloatv(pname, params),
        }
    }
    unsafe fn GetTexEnviv(&mut self, target: GLenum, pname: GLenum, params: *mut GLint) {
        assert_eq!(target, es1::TEXTURE_ENV);
        if pname == es1::TEXTURE_ENV_MODE {
            *params = self.state.texture_env_mode[self.state.active_texture];
        } else if pname == es1::TEXTURE_ENV_COLOR {
            for (index, value) in self.state.texture_env_color[self.state.active_texture].iter().enumerate() {
                *params.add(index) = *value as GLint;
            }
        }
    }
    unsafe fn GetTexEnvfv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfloat) {
        assert_eq!(target, es1::TEXTURE_ENV);
        if pname == es1::TEXTURE_ENV_MODE {
            *params = self.state.texture_env_mode[self.state.active_texture] as GLfloat;
        } else if pname == es1::TEXTURE_ENV_COLOR {
            params.copy_from(self.state.texture_env_color[self.state.active_texture].as_ptr(), 4);
        }
    }
    unsafe fn GetTexEnvxv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfixed) {
        let mut values = [0.0; 4];
        self.GetTexEnvfv(target, pname, values.as_mut_ptr());
        for (index, value) in values.iter().enumerate() {
            *params.add(index) = float_to_fixed(*value);
        }
    }
    unsafe fn GetLightfv(&mut self, light: GLenum, pname: GLenum, params: *mut GLfloat) {
        if light != es1::LIGHT0 || params.is_null() { return; }
        match pname {
            es1::AMBIENT => params.copy_from(self.state.light0_ambient.as_ptr(), 4),
            es1::DIFFUSE => params.copy_from(self.state.light0_diffuse.as_ptr(), 4),
            es1::SPECULAR => params.copy_from(self.state.light0_specular.as_ptr(), 4),
            es1::POSITION => params.copy_from(self.state.light0_position.as_ptr(), 4),
            es1::SPOT_DIRECTION => params.copy_from(self.state.light0_spot_direction.as_ptr(), 3),
            es1::SPOT_CUTOFF => *params = self.state.light0_spot_cutoff,
            es1::SPOT_EXPONENT => *params = self.state.light0_spot_exponent,
            es1::CONSTANT_ATTENUATION => *params = self.state.light0_constant_attenuation,
            es1::LINEAR_ATTENUATION => *params = self.state.light0_linear_attenuation,
            es1::QUADRATIC_ATTENUATION => *params = self.state.light0_quadratic_attenuation,
            _ => {}
        }
    }
    unsafe fn GetLightxv(&mut self, light: GLenum, pname: GLenum, params: *mut GLfixed) {
        let count = if pname == es1::SPOT_DIRECTION { 3 } else if matches!(pname, es1::AMBIENT | es1::DIFFUSE | es1::SPECULAR | es1::POSITION) { 4 } else { 1 };
        let mut values = [0.0; 4];
        self.GetLightfv(light, pname, values.as_mut_ptr());
        for i in 0..count { *params.add(i) = float_to_fixed(values[i]); }
    }
    unsafe fn GetMaterialfv(&mut self, face: GLenum, pname: GLenum, params: *mut GLfloat) {
        if face != es1::FRONT_AND_BACK || params.is_null() { return; }
        match pname {
            es1::AMBIENT => params.copy_from(self.state.material_ambient.as_ptr(), 4),
            es1::DIFFUSE => params.copy_from(self.state.material_diffuse.as_ptr(), 4),
            es1::SPECULAR => params.copy_from(self.state.material_specular.as_ptr(), 4),
            es1::SHININESS => *params = self.state.material_shininess,
            _ => {}
        }
    }
    unsafe fn GetMaterialxv(&mut self, face: GLenum, pname: GLenum, params: *mut GLfixed) {
        let mut values = [0.0; 4];
        self.GetMaterialfv(face, pname, values.as_mut_ptr());
        let count = if pname == es1::SHININESS { 1 } else { 4 };
        for i in 0..count { *params.add(i) = float_to_fixed(values[i]); }
    }
    unsafe fn GetTexParameteriv(&mut self, target: GLenum, pname: GLenum, params: *mut GLint) { gl::GetTexParameteriv(target, pname, params); }
    unsafe fn GetTexParameterfv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfloat) { gl::GetTexParameterfv(target, pname, params); }
    unsafe fn GetTexParameterxv(&mut self, target: GLenum, pname: GLenum, params: *mut GLfixed) {
        let mut value = 0.0;
        gl::GetTexParameterfv(target, pname, &mut value);
        *params = float_to_fixed(value);
    }
    unsafe fn Enable(&mut self, cap: GLenum) {
        if cap == es1::COLOR_LOGIC_OP {
            self.state.logic_op_enabled = true;
        } else if cap == es1::TEXTURE_2D {
            self.state.texture_enabled[self.state.active_texture] = true;
        } else if cap == es1::ALPHA_TEST {
            self.state.alpha_test_enabled = true;
        } else if cap == es1::FOG {
            self.state.fog_enabled = true;
        } else if cap == es1::LIGHTING {
            self.state.lighting_enabled = true;
        } else if cap == es1::LIGHT0 {
            self.state.light0_enabled = true;
        } else if cap == es1::COLOR_MATERIAL {
            self.state.color_material_enabled = true;
        } else if cap == es1::NORMALIZE {
            self.state.normalize_enabled = true;
        } else if cap == es1::MATRIX_PALETTE_OES {
            self.state.matrix_palette_enabled = true;
        } else if (es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&cap) {
            self.state.clip_plane_enabled[(cap - es1::CLIP_PLANE0) as usize] = true;
        } else {
            gl::Enable(cap);
        }
    }
    unsafe fn Disable(&mut self, cap: GLenum) {
        if cap == es1::COLOR_LOGIC_OP {
            self.state.logic_op_enabled = false;
        } else if cap == es1::TEXTURE_2D {
            self.state.texture_enabled[self.state.active_texture] = false;
        } else if cap == es1::ALPHA_TEST {
            self.state.alpha_test_enabled = false;
        } else if cap == es1::FOG {
            self.state.fog_enabled = false;
        } else if cap == es1::LIGHTING {
            self.state.lighting_enabled = false;
        } else if cap == es1::LIGHT0 {
            self.state.light0_enabled = false;
        } else if cap == es1::COLOR_MATERIAL {
            self.state.color_material_enabled = false;
        } else if cap == es1::NORMALIZE {
            self.state.normalize_enabled = false;
        } else if cap == es1::MATRIX_PALETTE_OES {
            self.state.matrix_palette_enabled = false;
        } else if (es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&cap) {
            self.state.clip_plane_enabled[(cap - es1::CLIP_PLANE0) as usize] = false;
        } else {
            gl::Disable(cap);
        }
    }
    unsafe fn IsEnabled(&mut self, cap: GLenum) -> GLboolean {
        if cap == es1::COLOR_LOGIC_OP {
            return if self.state.logic_op_enabled { gl::TRUE } else { gl::FALSE };
        }
        if cap == es1::TEXTURE_2D {
            return if self.state.texture_enabled[self.state.active_texture] { gl::TRUE } else { gl::FALSE };
        }
        if cap == es1::ALPHA_TEST {
            return if self.state.alpha_test_enabled { gl::TRUE } else { gl::FALSE };
        }
        if cap == es1::FOG {
            return if self.state.fog_enabled { gl::TRUE } else { gl::FALSE };
        }
        if cap == es1::LIGHTING {
            return if self.state.lighting_enabled { gl::TRUE } else { gl::FALSE };
        }
        if cap == es1::LIGHT0 {
            return if self.state.light0_enabled { gl::TRUE } else { gl::FALSE };
        }
        if cap == es1::COLOR_MATERIAL {
            return if self.state.color_material_enabled { gl::TRUE } else { gl::FALSE };
        }
        if cap == es1::NORMALIZE {
            return if self.state.normalize_enabled { gl::TRUE } else { gl::FALSE };
        }
        if cap == es1::MATRIX_PALETTE_OES {
            return if self.state.matrix_palette_enabled { gl::TRUE } else { gl::FALSE };
        }
        if (es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&cap) {
            return if self.state.clip_plane_enabled[(cap - es1::CLIP_PLANE0) as usize] { gl::TRUE } else { gl::FALSE };
        }
        gl::IsEnabled(cap)
    }
    unsafe fn ClientActiveTexture(&mut self, texture: GLenum) {
        self.state.client_active_texture = (texture - es1::TEXTURE0).min((MAX_TEXTURE_UNITS - 1) as GLenum) as usize;
    }
    unsafe fn ActiveTexture(&mut self, texture: GLenum) {
        self.state.active_texture = texture.saturating_sub(es1::TEXTURE0).min((MAX_TEXTURE_UNITS - 1) as GLenum) as usize;
        gl::ActiveTexture(es1::TEXTURE0 + self.state.active_texture as GLenum);
    }
    unsafe fn EnableClientState(&mut self, array: GLenum) {
        match array {
            es1::VERTEX_ARRAY => self.state.arrays[0].enabled = true,
            es1::COLOR_ARRAY => self.state.arrays[1].enabled = true,
            es1::NORMAL_ARRAY => self.state.arrays[2].enabled = true,
            es1::TEXTURE_COORD_ARRAY => self.state.texcoord_arrays[self.state.client_active_texture].enabled = true,
            es1::MATRIX_INDEX_ARRAY_OES => self.state.palette_index_array.enabled = true,
            es1::WEIGHT_ARRAY_OES => self.state.palette_weight_array.enabled = true,
            es1::POINT_SIZE_ARRAY_OES => self.state.point_size_array.enabled = true,
            _ => {}
        }
    }
    unsafe fn DisableClientState(&mut self, array: GLenum) {
        match array {
            es1::VERTEX_ARRAY => self.state.arrays[0].enabled = false,
            es1::COLOR_ARRAY => self.state.arrays[1].enabled = false,
            es1::NORMAL_ARRAY => self.state.arrays[2].enabled = false,
            es1::TEXTURE_COORD_ARRAY => self.state.texcoord_arrays[self.state.client_active_texture].enabled = false,
            es1::MATRIX_INDEX_ARRAY_OES => self.state.palette_index_array.enabled = false,
            es1::WEIGHT_ARRAY_OES => self.state.palette_weight_array.enabled = false,
            es1::POINT_SIZE_ARRAY_OES => self.state.point_size_array.enabled = false,
            _ => {}
        }
    }
    unsafe fn GetFixedv(&mut self, pname: GLenum, params: *mut GLfixed) {
        if params.is_null() { return; }
        let count = if matches!(pname, es1::MODELVIEW_MATRIX | es1::PROJECTION_MATRIX | es1::TEXTURE_MATRIX) { 16 } else if pname == es1::CURRENT_NORMAL { 3 } else { 4 };
        let mut values = [0.0; 16];
        self.GetFloatv(pname, values.as_mut_ptr());
        for i in 0..count { *params.add(i) = float_to_fixed(values[i]); }
    }
    unsafe fn GetPointerv(&mut self, pname: GLenum, params: *mut *const GLvoid) {
        if params.is_null() { return; }
        *params = match pname {
            es1::VERTEX_ARRAY_POINTER => self.state.arrays[0].pointer,
            es1::COLOR_ARRAY_POINTER => self.state.arrays[1].pointer,
            es1::NORMAL_ARRAY_POINTER => self.state.arrays[2].pointer,
            es1::TEXTURE_COORD_ARRAY_POINTER => self.state.texcoord_arrays[self.state.client_active_texture].pointer,
            es1::POINT_SIZE_ARRAY_POINTER_OES => self.state.point_size_array.pointer,
            es1::MATRIX_INDEX_ARRAY_POINTER_OES => self.state.palette_index_array.pointer,
            es1::WEIGHT_ARRAY_POINTER_OES => self.state.palette_weight_array.pointer,
            _ => std::ptr::null(),
        };
    }
    unsafe fn GetVertexAttribiv(&mut self, index: GLuint, pname: GLenum, params: *mut GLint) {
        gl::GetVertexAttribiv(index, pname, params);
    }
    unsafe fn GetVertexAttribfv(&mut self, index: GLuint, pname: GLenum, params: *mut GLfloat) {
        gl::GetVertexAttribfv(index, pname, params);
    }
    unsafe fn GetVertexAttribPointerv(&mut self, index: GLuint, pname: GLenum, pointer: *mut *mut GLvoid) {
        gl::GetVertexAttribPointerv(index, pname, pointer);
    }
    unsafe fn Hint(&mut self, _target: GLenum, _mode: GLenum) {}
    unsafe fn ClipPlanef(&mut self, plane: GLenum, equation: *const GLfloat) {
        if equation.is_null() || !(es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&plane) { return; }
        self.state.clip_planes[(plane - es1::CLIP_PLANE0) as usize] = std::slice::from_raw_parts(equation, 4).try_into().unwrap();
    }
    unsafe fn ClipPlanex(&mut self, plane: GLenum, equation: *const GLfixed) {
        if equation.is_null() || !(es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&plane) { return; }
        self.state.clip_planes[(plane - es1::CLIP_PLANE0) as usize] = std::slice::from_raw_parts(equation, 4).iter().map(|v| fixed_to_float(*v)).collect::<Vec<_>>().try_into().unwrap();
    }
    unsafe fn ClearDepthx(&mut self, depth: GLclampx) { self.ClearDepthf(fixed_to_float(depth)); }
    unsafe fn LineWidthx(&mut self, width: GLfixed) { self.LineWidth(fixed_to_float(width)); }
    unsafe fn StencilFunc(&mut self, func: GLenum, ref_: GLint, mask: GLuint) { gl::StencilFunc(func, ref_, mask); }
    unsafe fn StencilOp(&mut self, sfail: GLenum, dpfail: GLenum, dppass: GLenum) { gl::StencilOp(sfail, dpfail, dppass); }
    unsafe fn StencilMask(&mut self, mask: GLuint) { gl::StencilMask(mask); }
    unsafe fn PointParameterf(&mut self, pname: GLenum, param: GLfloat) {
        match pname { es1::POINT_SIZE_MIN | es1::POINT_SIZE_MAX => {}, es1::POINT_FADE_THRESHOLD_SIZE => self.state.point_fade_threshold = param, _ => {} }
    }
    unsafe fn PointParameterx(&mut self, pname: GLenum, param: GLfixed) { self.PointParameterf(pname, fixed_to_float(param)); }
    unsafe fn PointParameterfv(&mut self, pname: GLenum, params: *const GLfloat) {
        if params.is_null() { return; }
        if pname == es1::POINT_DISTANCE_ATTENUATION { self.state.point_distance_attenuation = std::slice::from_raw_parts(params, 3).try_into().unwrap(); } else { self.PointParameterf(pname, *params); }
    }
    unsafe fn PointParameterxv(&mut self, pname: GLenum, params: *const GLfixed) {
        if params.is_null() { return; }
        if pname == es1::POINT_DISTANCE_ATTENUATION { self.state.point_distance_attenuation = std::slice::from_raw_parts(params, 3).iter().map(|v| fixed_to_float(*v)).collect::<Vec<_>>().try_into().unwrap(); } else { self.PointParameterx(pname, *params); }
    }
    unsafe fn AlphaFunc(&mut self, func: GLenum, ref_: GLclampf) {
        self.state.alpha_func = func;
        self.state.alpha_ref = ref_;
    }
    unsafe fn AlphaFuncx(&mut self, func: GLenum, ref_: GLclampx) {
        self.AlphaFunc(func, fixed_to_float(ref_));
    }
    unsafe fn DepthRangef(&mut self, near: GLclampf, far: GLclampf) {
        gl::DepthRangef(near, far);
    }
    unsafe fn DepthRangex(&mut self, near: GLclampx, far: GLclampx) {
        self.DepthRangef(fixed_to_float(near), fixed_to_float(far));
    }
    unsafe fn PolygonOffset(&mut self, factor: GLfloat, units: GLfloat) {
        gl::PolygonOffset(factor, units);
    }
    unsafe fn PolygonOffsetx(&mut self, factor: GLfixed, units: GLfixed) {
        self.PolygonOffset(fixed_to_float(factor), fixed_to_float(units));
    }
    unsafe fn SampleCoverage(&mut self, value: GLclampf, invert: GLboolean) {
        gl::SampleCoverage(value, invert);
    }
    unsafe fn SampleCoveragex(&mut self, value: GLclampx, invert: GLboolean) {
        self.SampleCoverage(fixed_to_float(value), invert);
    }
    unsafe fn ShadeModel(&mut self, mode: GLenum) {
        if mode != es1::FLAT && mode != es1::SMOOTH { return; }
    }
    unsafe fn Lightf(&mut self, light: GLenum, pname: GLenum, param: GLfloat) {
        if light != es1::LIGHT0 { return; }
        match pname {
            es1::SPOT_CUTOFF => self.state.light0_spot_cutoff = param,
            es1::SPOT_EXPONENT => self.state.light0_spot_exponent = param,
            es1::CONSTANT_ATTENUATION => self.state.light0_constant_attenuation = param,
            es1::LINEAR_ATTENUATION => self.state.light0_linear_attenuation = param,
            es1::QUADRATIC_ATTENUATION => self.state.light0_quadratic_attenuation = param,
            _ => {}
        }
    }
    unsafe fn Lightx(&mut self, light: GLenum, pname: GLenum, param: GLfixed) {
        self.Lightf(light, pname, fixed_to_float(param));
    }
    unsafe fn Lightfv(&mut self, light: GLenum, pname: GLenum, params: *const GLfloat) {
        if light != es1::LIGHT0 || params.is_null() { return; }
        match pname {
            es1::AMBIENT => self.state.light0_ambient = std::slice::from_raw_parts(params, 4).try_into().unwrap(),
            es1::DIFFUSE => self.state.light0_diffuse = std::slice::from_raw_parts(params, 4).try_into().unwrap(),
            es1::SPECULAR => self.state.light0_specular = std::slice::from_raw_parts(params, 4).try_into().unwrap(),
            es1::POSITION => self.state.light0_position = std::slice::from_raw_parts(params, 4).try_into().unwrap(),
            es1::SPOT_DIRECTION => self.state.light0_spot_direction = std::slice::from_raw_parts(params, 3).try_into().unwrap(),
            _ => {}
        }
    }
    unsafe fn Lightxv(&mut self, light: GLenum, pname: GLenum, params: *const GLfixed) {
        if params.is_null() { return; }
        let count = if pname == es1::SPOT_DIRECTION { 3 } else { 4 };
        let values: Vec<GLfloat> = std::slice::from_raw_parts(params, count).iter().map(|v| fixed_to_float(*v)).collect();
        self.Lightfv(light, pname, values.as_ptr());
    }
    unsafe fn LightModelf(&mut self, _pname: GLenum, _param: GLfloat) {}
    unsafe fn LightModelx(&mut self, pname: GLenum, param: GLfixed) {
        self.LightModelf(pname, fixed_to_float(param));
    }
    unsafe fn LightModelfv(&mut self, pname: GLenum, params: *const GLfloat) {
        if pname == es1::LIGHT_MODEL_AMBIENT && !params.is_null() {
            self.state.model_ambient = std::slice::from_raw_parts(params, 4).try_into().unwrap();
        }
    }
    unsafe fn LightModelxv(&mut self, pname: GLenum, params: *const GLfixed) {
        if pname == es1::LIGHT_MODEL_AMBIENT && !params.is_null() {
            self.state.model_ambient = std::slice::from_raw_parts(params, 4).iter().map(|v| fixed_to_float(*v)).collect::<Vec<_>>().try_into().unwrap();
        }
    }
    unsafe fn Materialf(&mut self, face: GLenum, pname: GLenum, param: GLfloat) {
        if face != es1::FRONT_AND_BACK { return; }
        if pname == es1::SHININESS { self.state.material_shininess = param; }
    }
    unsafe fn Materialx(&mut self, face: GLenum, pname: GLenum, param: GLfixed) {
        self.Materialf(face, pname, fixed_to_float(param));
    }
    unsafe fn Materialfv(&mut self, face: GLenum, pname: GLenum, params: *const GLfloat) {
        if face != es1::FRONT_AND_BACK || params.is_null() { return; }
        let values: [GLfloat; 4] = std::slice::from_raw_parts(params, 4).try_into().unwrap();
        match pname {
            es1::AMBIENT => self.state.material_ambient = values,
            es1::DIFFUSE => self.state.material_diffuse = values,
            es1::AMBIENT_AND_DIFFUSE => {
                self.state.material_ambient = values;
                self.state.material_diffuse = values;
            }
            es1::SPECULAR => self.state.material_specular = values,
            _ => {}
        }
    }
    unsafe fn Materialxv(&mut self, face: GLenum, pname: GLenum, params: *const GLfixed) {
        if params.is_null() { return; }
        let values: [GLfloat; 4] = std::slice::from_raw_parts(params, 4).iter().map(|v| fixed_to_float(*v)).collect::<Vec<_>>().try_into().unwrap();
        self.Materialfv(face, pname, values.as_ptr());
    }
    unsafe fn Color4f(&mut self, r: GLfloat, g: GLfloat, b: GLfloat, a: GLfloat) { self.state.color = [r, g, b, a]; }
    unsafe fn Color4x(&mut self, r: GLfixed, g: GLfixed, b: GLfixed, a: GLfixed) { self.Color4f(fixed_to_float(r), fixed_to_float(g), fixed_to_float(b), fixed_to_float(a)); }
    unsafe fn Color4ub(&mut self, r: GLubyte, g: GLubyte, b: GLubyte, a: GLubyte) { self.state.color = [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, a as f32 / 255.0]; }
    unsafe fn Normal3f(&mut self, x: GLfloat, y: GLfloat, z: GLfloat) { self.state.normal = [x, y, z]; }
    unsafe fn Normal3x(&mut self, x: GLfixed, y: GLfixed, z: GLfixed) { self.Normal3f(fixed_to_float(x), fixed_to_float(y), fixed_to_float(z)); }
    unsafe fn MultiTexCoord4f(&mut self, texture: GLenum, s: GLfloat, t: GLfloat, r: GLfloat, q: GLfloat) {
        let logger = GLES1to2Logger::new("glMultiTexCoord4f", "texture coordinates");
        let i = (texture - es1::TEXTURE0).min((MAX_TEXTURE_UNITS - 1) as GLenum) as usize;
        self.state.texcoords[i] = [s, t, r, q];
        logger.log_texture_coordinates(i, [s, t, r, q]);
        logger.finish();
    }
    unsafe fn MultiTexCoord4x(&mut self, texture: GLenum, s: GLfixed, t: GLfixed, r: GLfixed, q: GLfixed) { self.MultiTexCoord4f(texture, fixed_to_float(s), fixed_to_float(t), fixed_to_float(r), fixed_to_float(q)); }
    unsafe fn TexCoordPointer(&mut self, size: GLint, type_: GLenum, stride: GLsizei, pointer: *const GLvoid) {
        let logger = GLES1to2Logger::new("glTexCoordPointer", "texture coordinate array");
        let enabled = self.state.texcoord_arrays[self.state.client_active_texture].enabled;
        let buffer_binding = self.state.array_buffer_binding;
        self.state.texcoord_arrays[self.state.client_active_texture] = ArrayState { size, type_, stride, pointer, buffer_binding, enabled, fixed: type_ == es1::FIXED, normalized: false };
        if gles1_on_gles2_logging::enabled() {
            log!(
                "[GLES1→GLES2 TEXCOORD_POINTER] op={} unit={} size={} type=0x{:x} stride={} buffer_binding={}",
                logger.operation_id(),
                self.state.client_active_texture,
                size,
                type_,
                stride,
                buffer_binding
            );
        }
        logger.finish();
    }
    unsafe fn ColorPointer(&mut self, size: GLint, type_: GLenum, stride: GLsizei, pointer: *const GLvoid) {
        let enabled = self.state.arrays[1].enabled;
        let buffer_binding = self.state.array_buffer_binding;
        self.state.arrays[1] = ArrayState { size, type_, stride, pointer, buffer_binding, enabled, fixed: type_ == es1::FIXED, normalized: true };
    }
    unsafe fn NormalPointer(&mut self, type_: GLenum, stride: GLsizei, pointer: *const GLvoid) {
        let enabled = self.state.arrays[2].enabled;
        let buffer_binding = self.state.array_buffer_binding;
        self.state.arrays[2] = ArrayState { size: 3, type_, stride, pointer, buffer_binding, enabled, fixed: type_ == es1::FIXED, normalized: false };
    }
    unsafe fn VertexPointer(&mut self, size: GLint, type_: GLenum, stride: GLsizei, pointer: *const GLvoid) {
        let enabled = self.state.arrays[0].enabled;
        let buffer_binding = self.state.array_buffer_binding;
        self.state.arrays[0] = ArrayState { size, type_, stride, pointer, buffer_binding, enabled, fixed: type_ == es1::FIXED, normalized: false };
    }
    unsafe fn BindBuffer(&mut self, target: GLenum, buffer: GLuint) {
        match target {
            gl::ARRAY_BUFFER => self.state.array_buffer_binding = buffer,
            gl::ELEMENT_ARRAY_BUFFER => self.state.element_array_buffer_binding = buffer,
            _ => {}
        }
        gl::BindBuffer(target, buffer);
    }
    unsafe fn GenBuffers(&mut self, n: GLsizei, buffers: *mut GLuint) { gl::GenBuffers(n, buffers); }
    unsafe fn IsBuffer(&mut self, buffer: GLuint) -> GLboolean { gl::IsBuffer(buffer) }
    unsafe fn DeleteBuffers(&mut self, n: GLsizei, buffers: *const GLuint) {
        if !buffers.is_null() {
            for i in 0..n.max(0) as usize {
                let buffer = buffers.add(i).read();
                self.state.array_buffer_data.remove(&buffer);
                self.state.element_array_buffer_data.remove(&buffer);
            }
        }
        gl::DeleteBuffers(n, buffers);
    }
    unsafe fn BufferData(&mut self, target: GLenum, size: GLsizeiptr, data: *const GLvoid, usage: GLenum) {
        let binding = match target {
            gl::ARRAY_BUFFER => self.state.array_buffer_binding,
            gl::ELEMENT_ARRAY_BUFFER => self.state.element_array_buffer_binding,
            _ => 0,
        };
        if binding != 0 && size >= 0 {
            let store = if target == gl::ARRAY_BUFFER { &mut self.state.array_buffer_data } else { &mut self.state.element_array_buffer_data };
            let bytes = store.entry(binding).or_default();
            bytes.resize(size as usize, 0);
            if !data.is_null() { std::ptr::copy_nonoverlapping(data.cast::<u8>(), bytes.as_mut_ptr(), size as usize); }
        }
        gl::BufferData(target, size, data, usage);
    }
    unsafe fn BufferSubData(&mut self, target: GLenum, offset: GLintptr, size: GLsizeiptr, data: *const GLvoid) {
        let binding = match target {
            gl::ARRAY_BUFFER => self.state.array_buffer_binding,
            gl::ELEMENT_ARRAY_BUFFER => self.state.element_array_buffer_binding,
            _ => 0,
        };
        if binding != 0 && offset >= 0 && size >= 0 && !data.is_null() {
            let store = if target == gl::ARRAY_BUFFER { &mut self.state.array_buffer_data } else { &mut self.state.element_array_buffer_data };
            let bytes = store.entry(binding).or_default();
            let end = offset as usize + size as usize;
            if end > bytes.len() { bytes.resize(end, 0); }
            std::ptr::copy_nonoverlapping(data.cast::<u8>(), bytes.as_mut_ptr().add(offset as usize), size as usize);
        }
        gl::BufferSubData(target, offset, size, data);
    }
    unsafe fn BindTexture(&mut self, target: GLenum, texture: GLuint) { gl::BindTexture(target, texture); }
    unsafe fn GenTextures(&mut self, n: GLsizei, textures: *mut GLuint) { gl::GenTextures(n, textures); }
    unsafe fn DeleteTextures(&mut self, n: GLsizei, textures: *const GLuint) { gl::DeleteTextures(n, textures); }
    unsafe fn TexParameteri(&mut self, target: GLenum, pname: GLenum, param: GLint) { gl::TexParameteri(target, pname, param); }
    unsafe fn TexParameterf(&mut self, target: GLenum, pname: GLenum, param: GLfloat) { gl::TexParameterf(target, pname, param); }
    unsafe fn TexParameterx(&mut self, target: GLenum, pname: GLenum, param: GLfixed) { gl::TexParameterf(target, pname, fixed_to_float(param)); }
    unsafe fn TexParameteriv(&mut self, target: GLenum, pname: GLenum, params: *const GLint) {
        if pname == es1::TEXTURE_CROP_RECT_OES && !params.is_null() { self.state.texture_crop_rect = std::slice::from_raw_parts(params, 4).try_into().unwrap(); return; }
        gl::TexParameteriv(target, pname, params);
    }
    unsafe fn TexParameterfv(&mut self, target: GLenum, pname: GLenum, params: *const GLfloat) {
        if pname == es1::TEXTURE_CROP_RECT_OES && !params.is_null() { self.state.texture_crop_rect = std::slice::from_raw_parts(params, 4).iter().map(|v| *v as GLint).collect::<Vec<_>>().try_into().unwrap(); return; }
        gl::TexParameterfv(target, pname, params);
    }
    unsafe fn TexParameterxv(&mut self, target: GLenum, pname: GLenum, params: *const GLfixed) {
        if pname == es1::TEXTURE_CROP_RECT_OES && !params.is_null() { self.state.texture_crop_rect = std::slice::from_raw_parts(params, 4).try_into().unwrap(); return; }
        let v = fixed_to_float(*params); gl::TexParameterf(target, pname, v);
    }
    unsafe fn DrawTexsOES(&mut self, x: i16, y: i16, z: i16, width: i16, height: i16) { self.DrawTexfOES(x as GLfloat, y as GLfloat, z as GLfloat, width as GLfloat, height as GLfloat); }
    unsafe fn DrawTexiOES(&mut self, x: GLint, y: GLint, z: GLint, width: GLint, height: GLint) { self.DrawTexfOES(x as GLfloat, y as GLfloat, z as GLfloat, width as GLfloat, height as GLfloat); }
    unsafe fn DrawTexxOES(&mut self, x: GLfixed, y: GLfixed, z: GLfixed, width: GLfixed, height: GLfixed) { self.DrawTexfOES(fixed_to_float(x), fixed_to_float(y), fixed_to_float(z), fixed_to_float(width), fixed_to_float(height)); }
    unsafe fn DrawTexsvOES(&mut self, coords: *const i16) { if !coords.is_null() { self.DrawTexsOES(coords.read_unaligned(), coords.add(1).read_unaligned(), coords.add(2).read_unaligned(), coords.add(3).read_unaligned(), coords.add(4).read_unaligned()); } }
    unsafe fn DrawTexivOES(&mut self, coords: *const GLint) { if !coords.is_null() { self.DrawTexiOES(coords.read_unaligned(), coords.add(1).read_unaligned(), coords.add(2).read_unaligned(), coords.add(3).read_unaligned(), coords.add(4).read_unaligned()); } }
    unsafe fn DrawTexxvOES(&mut self, coords: *const GLfixed) { if !coords.is_null() { self.DrawTexxOES(coords.read_unaligned(), coords.add(1).read_unaligned(), coords.add(2).read_unaligned(), coords.add(3).read_unaligned(), coords.add(4).read_unaligned()); } }
    unsafe fn DrawTexfvOES(&mut self, coords: *const GLfloat) { if !coords.is_null() { self.DrawTexfOES(coords.read_unaligned(), coords.add(1).read_unaligned(), coords.add(2).read_unaligned(), coords.add(3).read_unaligned(), coords.add(4).read_unaligned()); } }
    unsafe fn DrawTexfOES(&mut self, x: GLfloat, y: GLfloat, z: GLfloat, width: GLfloat, height: GLfloat) {
        let program = match self.state.program {
            Some(program) => program,
            None => { let Ok(program) = create_program() else { return; }; self.state.program = Some(program); program }
        };
        let crop = self.state.texture_crop_rect;
        let viewport = self.state.viewport;
        if width <= 0.0 || height <= 0.0 || viewport[2] <= 0 || viewport[3] <= 0 { return; }
        let sx = 2.0 / viewport[2] as GLfloat;
        let sy = 2.0 / viewport[3] as GLfloat;
        let x0 = x * sx - 1.0;
        let y0 = y * sy - 1.0;
        let x1 = (x + width) * sx - 1.0;
        let y1 = (y + height) * sy - 1.0;
        let vertices = [x0, y0, z, 1.0, x1, y0, z, 1.0, x0, y1, z, 1.0, x1, y1, z, 1.0];
        let tex_w = crop[2].max(1) as GLfloat;
        let tex_h = crop[3].max(1) as GLfloat;
        let u0 = crop[0] as GLfloat / tex_w;
        let v0 = crop[1] as GLfloat / tex_h;
        let u1 = (crop[0] + crop[2]) as GLfloat / tex_w;
        let v1 = (crop[1] + crop[3]) as GLfloat / tex_h;
        let texcoords = [u0, v1, u1, v1, u0, v0, u1, v0];
        gl::UseProgram(program);
        gl::UniformMatrix4fv(gl::GetUniformLocation(program, b"u_mvp\0".as_ptr() as *const _), 1, gl::FALSE, MATRIX_IDENTITY.as_ptr());
        gl::Uniform4f(gl::GetUniformLocation(program, b"u_color\0".as_ptr() as *const _), 1.0, 1.0, 1.0, 1.0);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_tex_enabled0\0".as_ptr() as *const _), 1);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_tex_mode0\0".as_ptr() as *const _), 1);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_tex0\0".as_ptr() as *const _), 0);
        gl::DisableVertexAttribArray(ATTR_COLOR);
        gl::VertexAttrib4f(ATTR_COLOR, 1.0, 1.0, 1.0, 1.0);
        gl::DisableVertexAttribArray(ATTR_NORMAL);
        gl::DisableVertexAttribArray(ATTR_TEX0);
        gl::EnableVertexAttribArray(ATTR_POSITION);
        gl::EnableVertexAttribArray(ATTR_TEX0);
        gl::VertexAttribPointer(ATTR_POSITION, 4, gl::FLOAT, gl::FALSE, 0, vertices.as_ptr().cast());
        gl::VertexAttribPointer(ATTR_TEX0, 2, gl::FLOAT, gl::FALSE, 0, texcoords.as_ptr().cast());
        gl::DrawArrays(gl::TRIANGLE_STRIP, 0, 4);
        gl::DisableVertexAttribArray(ATTR_POSITION);
        gl::DisableVertexAttribArray(ATTR_TEX0);
    }
    unsafe fn TexImage2D(&mut self, target: GLenum, level: GLint, internalformat: GLint, width: GLsizei, height: GLsizei, border: GLint, format: GLenum, type_: GLenum, pixels: *const GLvoid) { gl::TexImage2D(target, level, internalformat, width, height, border, format, type_, pixels); }
    unsafe fn TexSubImage2D(&mut self, target: GLenum, level: GLint, x: GLint, y: GLint, width: GLsizei, height: GLsizei, format: GLenum, type_: GLenum, pixels: *const GLvoid) { gl::TexSubImage2D(target, level, x, y, width, height, format, type_, pixels); }
    unsafe fn CompressedTexSubImage2D(&mut self, target: GLenum, level: GLint, x: GLint, y: GLint, width: GLsizei, height: GLsizei, format: GLenum, image_size: GLsizei, data: *const GLvoid) { gl::CompressedTexSubImage2D(target, level, x, y, width, height, format, image_size, data); }
    unsafe fn GetBufferParameteriv(&mut self, target: GLenum, pname: GLenum, params: *mut GLint) { if !params.is_null() { gl::GetBufferParameteriv(target, pname, params); } }
    unsafe fn MapBufferOES(&mut self, target: GLenum, _access: GLenum) -> *mut GLvoid {
        let binding = match target {
            gl::ARRAY_BUFFER => self.state.array_buffer_binding,
            gl::ELEMENT_ARRAY_BUFFER => self.state.element_array_buffer_binding,
            _ => 0,
        };
        if binding == 0 {
            return std::ptr::null_mut();
        }
        let store = if target == gl::ARRAY_BUFFER {
            &mut self.state.array_buffer_data
        } else {
            &mut self.state.element_array_buffer_data
        };
        let Some(bytes) = store.get_mut(&binding) else {
            return std::ptr::null_mut();
        };
        self.state.mapped_buffer = Some((target, binding));
        bytes.as_mut_ptr().cast()
    }
    unsafe fn UnmapBufferOES(&mut self, target: GLenum) -> GLboolean {
        if self.state.mapped_buffer.map(|(mapped_target, _)| mapped_target) == Some(target) {
            self.state.mapped_buffer = None;
            gl::TRUE
        } else {
            gl::FALSE
        }
    }
    unsafe fn CopyTexImage2D(&mut self, target: GLenum, level: GLint, internalformat: GLenum, x: GLint, y: GLint, width: GLsizei, height: GLsizei, border: GLint) { gl::CopyTexImage2D(target, level, internalformat, x, y, width, height, border); }
    unsafe fn CopyTexSubImage2D(&mut self, target: GLenum, level: GLint, xoffset: GLint, yoffset: GLint, x: GLint, y: GLint, width: GLsizei, height: GLsizei) { gl::CopyTexSubImage2D(target, level, xoffset, yoffset, x, y, width, height); }
    unsafe fn CompressedTexImage2D(&mut self, target: GLenum, level: GLint, internalformat: GLenum, width: GLsizei, height: GLsizei, border: GLint, image_size: GLsizei, data: *const GLvoid) {
        if !data.is_null() && image_size > 0 && try_decode_pvrtc(self, target, level, internalformat, width, height, border, std::slice::from_raw_parts(data.cast::<u8>(), image_size as usize)) {
            return;
        }
        gl::CompressedTexImage2D(target, level, internalformat, width, height, border, image_size, data);
    }
    unsafe fn TexEnvi(&mut self, _target: GLenum, pname: GLenum, param: GLint) {
        let unit = self.state.active_texture;
        match pname {
            es1::TEXTURE_ENV_MODE => self.state.texture_env_mode[unit] = param,
            es1::TEXTURE_ENV_COLOR => self.state.texture_env_color[unit] = [param as GLfloat; 4],
            _ => {}
        }
    }
    unsafe fn TexEnvf(&mut self, target: GLenum, pname: GLenum, param: GLfloat) { self.TexEnvi(target, pname, param as GLint); }
    unsafe fn TexEnvx(&mut self, target: GLenum, pname: GLenum, param: GLfixed) { self.TexEnvi(target, pname, param); }
    unsafe fn TexEnviv(&mut self, target: GLenum, pname: GLenum, params: *const GLint) {
        if pname == es1::TEXTURE_ENV_COLOR { self.state.texture_env_color[self.state.active_texture] = std::slice::from_raw_parts(params, 4).iter().map(|v| *v as GLfloat).collect::<Vec<_>>().try_into().unwrap(); } else { self.TexEnvi(target, pname, *params); }
    }
    unsafe fn TexEnvfv(&mut self, target: GLenum, pname: GLenum, params: *const GLfloat) {
        if pname == es1::TEXTURE_ENV_COLOR { self.state.texture_env_color[self.state.active_texture] = std::slice::from_raw_parts(params, 4).try_into().unwrap(); } else { self.TexEnvi(target, pname, *params as GLint); }
    }
    unsafe fn TexEnvxv(&mut self, target: GLenum, pname: GLenum, params: *const GLfixed) {
        if pname == es1::TEXTURE_ENV_COLOR { self.state.texture_env_color[self.state.active_texture] = std::slice::from_raw_parts(params, 4).iter().map(|v| fixed_to_float(*v)).collect::<Vec<_>>().try_into().unwrap(); } else { self.TexEnvi(target, pname, *params); }
    }
    unsafe fn MatrixMode(&mut self, mode: GLenum) {
        let logger = GLES1to2Logger::new("glMatrixMode", "matrix state");
        self.state.matrix_mode = mode;
        log_matrix_operation("glMatrixMode", format!("mode=0x{mode:x} ({})", matrix_mode_name(mode)));
        let current = self.state.matrix_mut().current;
        log_matrix_result("glMatrixMode", &current);
        logger.log_matrix("result", &current, false);
        logger.finish();
    }
    unsafe fn LoadIdentity(&mut self) {
        let logger = GLES1to2Logger::new("glLoadIdentity", "matrix state");
        self.state.matrix_mut().current = MATRIX_IDENTITY;
        log_matrix_operation("glLoadIdentity", format!("mode={}", matrix_mode_name(self.state.matrix_mode)));
        log_matrix_result("glLoadIdentity", &MATRIX_IDENTITY);
        logger.log_matrix("result", &MATRIX_IDENTITY, false);
        logger.finish();
    }
    unsafe fn LoadMatrixf(&mut self, m: *const GLfloat) {
        let logger = GLES1to2Logger::new("glLoadMatrixf", "matrix state");
        let values: [GLfloat; 16] = std::slice::from_raw_parts(m, 16).try_into().unwrap();
        self.state.matrix_mut().current = values;
        log_matrix_operation("glLoadMatrixf", format!("mode={}", matrix_mode_name(self.state.matrix_mode)));
        log_matrix_result("glLoadMatrixf", &values);
        logger.log_matrix("result", &values, false);
        logger.finish();
    }
    unsafe fn LoadMatrixx(&mut self, m: *const GLfixed) {
        let mut out = [0.0; 16];
        for (d, s) in out.iter_mut().zip(std::slice::from_raw_parts(m, 16)) {
            *d = fixed_to_float(*s);
        }
        self.state.matrix_mut().current = out;
        log_matrix_operation("glLoadMatrixx", format!("mode={}", matrix_mode_name(self.state.matrix_mode)));
        log_matrix_result("glLoadMatrixx", &out);
    }
    unsafe fn MultMatrixf(&mut self, m: *const GLfloat) {
        let logger = GLES1to2Logger::new("glMultMatrixf", "matrix state");
        let b: [GLfloat; 16] = std::slice::from_raw_parts(m, 16).try_into().unwrap();
        let a = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&a, &b);
        log_matrix_operation("glMultMatrixf", format!("mode={}", matrix_mode_name(self.state.matrix_mode)));
        log_matrix("glMultMatrixf input", &b);
        let current = self.state.matrix_mut().current;
        log_matrix_result("glMultMatrixf", &current);
        logger.log_matrix("input", &b, true);
        logger.log_matrix("result", &current, false);
        logger.finish();
    }
    unsafe fn MultMatrixx(&mut self, m: *const GLfixed) {
        let logger = GLES1to2Logger::new("glMultMatrixx", "matrix state");
        let mut b = [0.0; 16];
        for (d, s) in b.iter_mut().zip(std::slice::from_raw_parts(m, 16)) {
            *d = fixed_to_float(*s);
        }
        let a = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&a, &b);
        log_matrix_operation("glMultMatrixx", format!("mode={}", matrix_mode_name(self.state.matrix_mode)));
        log_matrix("glMultMatrixx input", &b);
        let current = self.state.matrix_mut().current;
        log_matrix_result("glMultMatrixx", &current);
        logger.log_matrix("input", &b, true);
        logger.log_matrix("result", &current, false);
        logger.finish();
    }
    unsafe fn PushMatrix(&mut self) {
        let logger = GLES1to2Logger::new("glPushMatrix", "matrix state");
        let current = self.state.matrix_mut().current;
        self.state.matrix_mut().stack.push(current);
        log_matrix_operation("glPushMatrix", format!("mode={}", matrix_mode_name(self.state.matrix_mode)));
        log_matrix_result("glPushMatrix", &current);
        logger.log_matrix("result", &current, false);
        logger.finish();
    }
    unsafe fn PopMatrix(&mut self) {
        let logger = GLES1to2Logger::new("glPopMatrix", "matrix state");
        if let Some(m) = self.state.matrix_mut().stack.pop() {
            self.state.matrix_mut().current = m;
        }
        log_matrix_operation("glPopMatrix", format!("mode={}", matrix_mode_name(self.state.matrix_mode)));
        let current = self.state.matrix_mut().current;
        log_matrix_result("glPopMatrix", &current);
        logger.log_matrix("result", &current, false);
        logger.finish();
    }
    unsafe fn Orthof(&mut self, l: GLfloat, r: GLfloat, b: GLfloat, t: GLfloat, n: GLfloat, f: GLfloat) {
        let logger = GLES1to2Logger::new("glOrthof", "projection");
        let a = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&a, &ortho(l, r, b, t, n, f));
        log_matrix_operation("glOrthof", format!("left={l}, right={r}, bottom={b}, top={t}, near={n}, far={f}"));
        let current = self.state.matrix_mut().current;
        log_matrix_result("glOrthof", &current);
        logger.log_projection("glOrthof", (l as f64, r as f64, b as f64, t as f64, n as f64, f as f64), None);
        logger.log_matrix("result", &current, false);
        logger.finish();
    }
    unsafe fn Orthox(&mut self, l: GLfixed, r: GLfixed, b: GLfixed, t: GLfixed, n: GLfixed, f: GLfixed) {
        log_matrix_operation("glOrthox", format!("left={l}, right={r}, bottom={b}, top={t}, near={n}, far={f}"));
        self.Orthof(fixed_to_float(l), fixed_to_float(r), fixed_to_float(b), fixed_to_float(t), fixed_to_float(n), fixed_to_float(f));
    }
    unsafe fn Frustumf(&mut self, l: GLfloat, r: GLfloat, b: GLfloat, t: GLfloat, n: GLfloat, f: GLfloat) {
        let logger = GLES1to2Logger::new("glFrustumf", "projection");
        let a = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&a, &frustum(l, r, b, t, n, f));
        log_matrix_operation("glFrustumf", format!("left={l}, right={r}, bottom={b}, top={t}, near={n}, far={f}"));
        let current = self.state.matrix_mut().current;
        log_matrix_result("glFrustumf", &current);
        logger.log_projection("glFrustumf", (l as f64, r as f64, b as f64, t as f64, n as f64, f as f64), None);
        logger.log_matrix("result", &current, false);
        logger.finish();
    }
    unsafe fn Frustumx(&mut self, l: GLfixed, r: GLfixed, b: GLfixed, t: GLfixed, n: GLfixed, f: GLfixed) {
        log_matrix_operation("glFrustumx", format!("left={l}, right={r}, bottom={b}, top={t}, near={n}, far={f}"));
        self.Frustumf(fixed_to_float(l), fixed_to_float(r), fixed_to_float(b), fixed_to_float(t), fixed_to_float(n), fixed_to_float(f));
    }
    unsafe fn Translatef(&mut self, x: GLfloat, y: GLfloat, z: GLfloat) {
        let logger = GLES1to2Logger::new("glTranslatef", "matrix state");
        let a = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&a, &translation(x, y, z));
        log_matrix_operation("glTranslatef", format!("x={x}, y={y}, z={z}"));
        let current = self.state.matrix_mut().current;
        log_matrix_result("glTranslatef", &current);
        logger.log_matrix("result", &current, false);
        logger.finish();
    }
    unsafe fn Translatex(&mut self, x: GLfixed, y: GLfixed, z: GLfixed) {
        log_matrix_operation("glTranslatex", format!("x={x}, y={y}, z={z}"));
        self.Translatef(fixed_to_float(x), fixed_to_float(y), fixed_to_float(z));
    }
    unsafe fn Scalef(&mut self, x: GLfloat, y: GLfloat, z: GLfloat) {
        let logger = GLES1to2Logger::new("glScalef", "matrix state");
        let a = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&a, &scaling(x, y, z));
        log_matrix_operation("glScalef", format!("x={x}, y={y}, z={z}"));
        let current = self.state.matrix_mut().current;
        log_matrix_result("glScalef", &current);
        logger.log_matrix("result", &current, false);
        logger.finish();
    }
    unsafe fn Scalex(&mut self, x: GLfixed, y: GLfixed, z: GLfixed) {
        log_matrix_operation("glScalex", format!("x={x}, y={y}, z={z}"));
        self.Scalef(fixed_to_float(x), fixed_to_float(y), fixed_to_float(z));
    }
    unsafe fn Rotatef(&mut self, a: GLfloat, x: GLfloat, y: GLfloat, z: GLfloat) {
        let logger = GLES1to2Logger::new("glRotatef", "matrix state");
        let m = self.state.matrix_mut().current;
        self.state.matrix_mut().current = multiply(&m, &rotation(a, x, y, z));
        log_matrix_operation("glRotatef", format!("angle={a}, axis=({x}, {y}, {z})"));
        let current = self.state.matrix_mut().current;
        log_matrix_result("glRotatef", &current);
        logger.log_rotation_operation(a, (x, y, z), (x, y, z));
        logger.log_matrix("result", &current, false);
        logger.finish();
    }
    unsafe fn Rotatex(&mut self, a: GLfixed, x: GLfixed, y: GLfixed, z: GLfixed) {
        log_matrix_operation("glRotatex", format!("angle={a}, axis=({x}, {y}, {z})"));
        self.Rotatef(fixed_to_float(a), fixed_to_float(x), fixed_to_float(y), fixed_to_float(z));
    }
    unsafe fn Viewport(&mut self, x: GLint, y: GLint, w: GLsizei, h: GLsizei) {
        let logger = GLES1to2Logger::new("glViewport", "viewport");
        let (requested_x, requested_y, requested_w, requested_h) = (x, y, w, h);
        let (x, y, w, h) = apply_viewport(x, y, w, h);
        logger.log_viewport(
            requested_x,
            requested_y,
            requested_w.max(0) as u32,
            requested_h.max(0) as u32,
            Some((x, y, w.max(0) as u32, h.max(0) as u32)),
        );
        if !self.state.first_viewport_logged {
            log!(
                "[GLES1→GLES2 VIEWPORT FIX] version={} requested=({}, {}, {}, {}) applied=({}, {}, {}, {}) actual_window={}x{}",
                VIEWPORT_FIX_VERSION,
                requested_x,
                requested_y,
                requested_w,
                requested_h,
                x,
                y,
                w,
                h,
                self.state.actual_window_size.0,
                self.state.actual_window_size.1,
            );
            self.state.first_viewport_logged = true;
        }
        log_viewport(self.state.actual_window_size.0, self.state.actual_window_size.1, x, y, w, h);
        self.state.viewport = [x, y, w, h];
        gl::Viewport(x, y, w, h);
        logger.finish();
    }
    unsafe fn Scissor(&mut self, x: GLint, y: GLint, w: GLsizei, h: GLsizei) { gl::Scissor(x, y, w, h); }
    unsafe fn Clear(&mut self, mask: GLbitfield) { gl::Clear(mask); }
    unsafe fn ClearColor(&mut self, r: GLclampf, g: GLclampf, b: GLclampf, a: GLclampf) { gl::ClearColor(r, g, b, a); }
    unsafe fn ClearColorx(&mut self, r: GLclampx, g: GLclampx, b: GLclampx, a: GLclampx) { self.ClearColor(fixed_to_float(r), fixed_to_float(g), fixed_to_float(b), fixed_to_float(a)); }
    unsafe fn ClearDepthf(&mut self, d: GLclampf) { gl::ClearDepthf(d); }
    unsafe fn ClearStencil(&mut self, s: GLint) { gl::ClearStencil(s); }
    unsafe fn Fogf(&mut self, pname: GLenum, param: GLfloat) {
        match pname { es1::FOG_MODE => self.state.fog_mode = param as GLenum, es1::FOG_DENSITY => self.state.fog_density = param, es1::FOG_START => self.state.fog_start = param, es1::FOG_END => self.state.fog_end = param, _ => {} }
    }
    unsafe fn Fogx(&mut self, pname: GLenum, param: GLfixed) { self.Fogf(pname, fixed_to_float(param)); }
    unsafe fn Fogfv(&mut self, pname: GLenum, params: *const GLfloat) {
        if params.is_null() { return; }
        if pname == es1::FOG_COLOR { self.state.fog_color = std::slice::from_raw_parts(params, 4).try_into().unwrap(); } else { self.Fogf(pname, *params); }
    }
    unsafe fn Fogxv(&mut self, pname: GLenum, params: *const GLfixed) {
        if params.is_null() { return; }
        if pname == es1::FOG_COLOR { self.state.fog_color = std::slice::from_raw_parts(params, 4).iter().map(|v| fixed_to_float(*v)).collect::<Vec<_>>().try_into().unwrap(); } else { self.Fogx(pname, *params); }
    }
    unsafe fn GetClipPlanef(&mut self, plane: GLenum, equation: *mut GLfloat) {
        if equation.is_null() || !(es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&plane) { return; }
        equation.copy_from(self.state.clip_planes[(plane - es1::CLIP_PLANE0) as usize].as_ptr(), 4);
    }
    unsafe fn GetClipPlanex(&mut self, plane: GLenum, equation: *mut GLfixed) {
        if equation.is_null() || !(es1::CLIP_PLANE0..=es1::CLIP_PLANE5).contains(&plane) { return; }
        for (i, value) in self.state.clip_planes[(plane - es1::CLIP_PLANE0) as usize].iter().enumerate() { *equation.add(i) = float_to_fixed(*value); }
    }
    unsafe fn GetIntegerv(&mut self, pname: GLenum, params: *mut GLint) {
        if params.is_null() { return; }
        match pname {
            es1::VIEWPORT => params.copy_from(self.state.viewport.as_ptr(), 4),
            es1::TEXTURE_CROP_RECT_OES => params.copy_from(self.state.texture_crop_rect.as_ptr(), 4),
            es1::ACTIVE_TEXTURE => *params = es1::TEXTURE0 as GLint + self.state.active_texture as GLint,
            es1::CLIENT_ACTIVE_TEXTURE => *params = es1::TEXTURE0 as GLint + self.state.client_active_texture as GLint,
            es1::MATRIX_MODE => *params = self.state.matrix_mode as GLint,
            es1::ARRAY_BUFFER_BINDING => *params = self.state.array_buffer_binding as GLint,
            es1::ELEMENT_ARRAY_BUFFER_BINDING => *params = self.state.element_array_buffer_binding as GLint,
            es1::POINT_SIZE_ARRAY_OES => *params = if self.state.point_size_array.enabled { gl::TRUE as GLint } else { gl::FALSE as GLint },
            es1::MAX_PALETTE_MATRICES_OES => *params = MAX_PALETTE_MATRICES as GLint,
            _ => gl::GetIntegerv(pname, params),
        }
    }
    unsafe fn DepthFunc(&mut self, f: GLenum) { gl::DepthFunc(f); }
    unsafe fn DepthMask(&mut self, f: GLboolean) { gl::DepthMask(f); }
    unsafe fn CullFace(&mut self, f: GLenum) { gl::CullFace(f); }
    unsafe fn FrontFace(&mut self, f: GLenum) { gl::FrontFace(f); }
    unsafe fn BlendFunc(&mut self, s: GLenum, d: GLenum) { gl::BlendFunc(s, d); }
    unsafe fn BlendEquation(&mut self, mode: GLenum) { gl::BlendEquation(mode); }
    unsafe fn BlendEquationSeparate(&mut self, mode_rgb: GLenum, mode_alpha: GLenum) { gl::BlendEquationSeparate(mode_rgb, mode_alpha); }
    unsafe fn BlendFuncSeparate(&mut self, src_rgb: GLenum, dst_rgb: GLenum, src_alpha: GLenum, dst_alpha: GLenum) { gl::BlendFuncSeparate(src_rgb, dst_rgb, src_alpha, dst_alpha); }
    unsafe fn StencilFuncSeparate(&mut self, face: GLenum, func: GLenum, ref_: GLint, mask: GLuint) { gl::StencilFuncSeparate(face, func, ref_, mask); }
    unsafe fn StencilOpSeparate(&mut self, face: GLenum, sfail: GLenum, dpfail: GLenum, dppass: GLenum) { gl::StencilOpSeparate(face, sfail, dpfail, dppass); }
    unsafe fn StencilMaskSeparate(&mut self, face: GLenum, mask: GLuint) { gl::StencilMaskSeparate(face, mask); }
    unsafe fn BlendColor(&mut self, r: GLclampf, g: GLclampf, b: GLclampf, a: GLclampf) { gl::BlendColor(r, g, b, a); }
    unsafe fn BlendEquationOES(&mut self, m: GLenum) { gl::BlendEquation(m); }
    unsafe fn LogicOp(&mut self, opcode: GLenum) { self.state.logic_op = opcode; }
    unsafe fn ColorMask(&mut self, r: GLboolean, g: GLboolean, b: GLboolean, a: GLboolean) { gl::ColorMask(r, g, b, a); }
    unsafe fn LineWidth(&mut self, w: GLfloat) { gl::LineWidth(w); }
    unsafe fn Finish(&mut self) { gl::Finish(); }
    unsafe fn Flush(&mut self) { gl::Flush(); }
    unsafe fn ReadPixels(&mut self, x: GLint, y: GLint, w: GLsizei, h: GLsizei, format: GLenum, type_: GLenum, pixels: *mut GLvoid) { gl::ReadPixels(x, y, w, h, format, type_, pixels); }
    unsafe fn PixelStorei(&mut self, p: GLenum, v: GLint) { gl::PixelStorei(p, v); }
    unsafe fn GenFramebuffersOES(&mut self, n: GLsizei, p: *mut GLuint) { gl::GenFramebuffers(n, p); }
    unsafe fn DeleteFramebuffersOES(&mut self, n: GLsizei, p: *const GLuint) { gl::DeleteFramebuffers(n, p); }
    unsafe fn BindFramebufferOES(&mut self, t: GLenum, f: GLuint) { gl::BindFramebuffer(t, f); }
    unsafe fn GenRenderbuffersOES(&mut self, n: GLsizei, p: *mut GLuint) { gl::GenRenderbuffers(n, p); }
    unsafe fn DeleteRenderbuffersOES(&mut self, n: GLsizei, p: *const GLuint) { gl::DeleteRenderbuffers(n, p); }
    unsafe fn BindRenderbufferOES(&mut self, t: GLenum, r: GLuint) { gl::BindRenderbuffer(t, r); }
    unsafe fn RenderbufferStorageOES(&mut self, t: GLenum, f: GLenum, w: GLsizei, h: GLsizei) { gl::RenderbufferStorage(t, f, w, h); }
    unsafe fn RenderbufferStorageMultisampleAPPLE(&mut self, t: GLenum, samples: GLsizei, f: GLenum, w: GLsizei, h: GLsizei) { if gl::RenderbufferStorageMultisampleAPPLE::is_loaded() { gl::RenderbufferStorageMultisampleAPPLE(t, samples, f, w, h); } else { gl::RenderbufferStorage(t, f, w, h); } }
    unsafe fn ResolveMultisampleFramebufferAPPLE(&mut self) { if gl::ResolveMultisampleFramebufferAPPLE::is_loaded() { gl::ResolveMultisampleFramebufferAPPLE(); } }
    unsafe fn GetRenderbufferParameterivOES(&mut self, t: GLenum, p: GLenum, params: *mut GLint) { gl::GetRenderbufferParameteriv(t, p, params); }
    unsafe fn FramebufferRenderbufferOES(&mut self, t: GLenum, a: GLenum, rt: GLenum, r: GLuint) { gl::FramebufferRenderbuffer(t, a, rt, r); }
    unsafe fn FramebufferTexture2DOES(&mut self, t: GLenum, a: GLenum, tt: GLenum, tex: GLuint, level: GLint) { gl::FramebufferTexture2D(t, a, tt, tex, level); }
    unsafe fn GetFramebufferAttachmentParameterivOES(&mut self, t: GLenum, a: GLenum, p: GLenum, params: *mut GLint) { gl::GetFramebufferAttachmentParameteriv(t, a, p, params); }
    unsafe fn GenerateMipmapOES(&mut self, t: GLenum) { gl::GenerateMipmap(t); }
    unsafe fn CheckFramebufferStatus(&mut self, t: GLenum) -> GLenum { gl::CheckFramebufferStatus(t) }
    unsafe fn CheckFramebufferStatusOES(&mut self, t: GLenum) -> GLenum { gl::CheckFramebufferStatus(t) }
    unsafe fn IsFramebufferOES(&mut self, f: GLuint) -> GLboolean { gl::IsFramebuffer(f) }
    unsafe fn IsRenderbufferOES(&mut self, r: GLuint) -> GLboolean { gl::IsRenderbuffer(r) }
    unsafe fn GenerateMipmap(&mut self, t: GLenum) { gl::GenerateMipmap(t); }
    unsafe fn GetFramebufferAttachmentParameteriv(&mut self, t: GLenum, a: GLenum, p: GLenum, params: *mut GLint) { gl::GetFramebufferAttachmentParameteriv(t, a, p, params); }
    unsafe fn GetRenderbufferParameteriv(&mut self, t: GLenum, p: GLenum, params: *mut GLint) { gl::GetRenderbufferParameteriv(t, p, params); }
    unsafe fn BindFramebuffer(&mut self, t: GLenum, f: GLuint) { gl::BindFramebuffer(t, f); }
    unsafe fn DeleteFramebuffers(&mut self, n: GLsizei, p: *const GLuint) { gl::DeleteFramebuffers(n, p); }
    unsafe fn GenFramebuffers(&mut self, n: GLsizei, p: *mut GLuint) { gl::GenFramebuffers(n, p); }
    unsafe fn BindRenderbuffer(&mut self, t: GLenum, r: GLuint) { gl::BindRenderbuffer(t, r); }
    unsafe fn RenderbufferStorage(&mut self, t: GLenum, f: GLenum, w: GLsizei, h: GLsizei) { gl::RenderbufferStorage(t, f, w, h); }
    unsafe fn FramebufferRenderbuffer(&mut self, t: GLenum, a: GLenum, rt: GLenum, r: GLuint) { gl::FramebufferRenderbuffer(t, a, rt, r); }
    unsafe fn FramebufferTexture2D(&mut self, t: GLenum, a: GLenum, tt: GLenum, tex: GLuint, level: GLint) { gl::FramebufferTexture2D(t, a, tt, tex, level); }
    unsafe fn DeleteRenderbuffers(&mut self, n: GLsizei, p: *const GLuint) { gl::DeleteRenderbuffers(n, p); }
    unsafe fn GenRenderbuffers(&mut self, n: GLsizei, p: *mut GLuint) { gl::GenRenderbuffers(n, p); }
    unsafe fn IsFramebuffer(&mut self, f: GLuint) -> GLboolean { gl::IsFramebuffer(f) }
    unsafe fn IsRenderbuffer(&mut self, r: GLuint) -> GLboolean { gl::IsRenderbuffer(r) }
    unsafe fn IsTexture(&mut self, t: GLuint) -> GLboolean { gl::IsTexture(t) }
    unsafe fn PointSize(&mut self, size: GLfloat) {
        self.state.point_size = size;
    }
    unsafe fn PointSizex(&mut self, size: GLfixed) {
        self.state.point_size = fixed_to_float(size);
    }
    unsafe fn PointSizePointerOES(&mut self, type_: GLenum, stride: GLsizei, pointer: *const GLvoid) {
        self.state.point_size_array.size = 1;
        self.state.point_size_array.type_ = type_;
        self.state.point_size_array.stride = stride;
        self.state.point_size_array.pointer = pointer;
        self.state.point_size_array.buffer_binding = self.state.array_buffer_binding;
        self.state.point_size_array.fixed = type_ == es1::FIXED;
    }
    unsafe fn CurrentPaletteMatrixOES(&mut self, matrixpaletteindex: GLuint) {
        self.state.current_palette_matrix = (matrixpaletteindex as usize).min(MAX_PALETTE_MATRICES - 1);
    }
    unsafe fn LoadPaletteFromModelViewMatrixOES(&mut self) {
        let index = self.state.current_palette_matrix;
        self.state.palette_matrices[index].current = self.state.modelview.current;
    }
    unsafe fn MatrixIndexPointerOES(&mut self, size: GLint, type_: GLenum, stride: GLsizei, pointer: *const GLvoid) {
        self.state.palette_index_array.size = size;
        self.state.palette_index_array.type_ = type_;
        self.state.palette_index_array.stride = stride;
        self.state.palette_index_array.pointer = pointer;
        self.state.palette_index_array.buffer_binding = self.state.array_buffer_binding;
    }
    unsafe fn WeightPointerOES(&mut self, size: GLint, type_: GLenum, stride: GLsizei, pointer: *const GLvoid) {
        self.state.palette_weight_array.size = size;
        self.state.palette_weight_array.type_ = type_;
        self.state.palette_weight_array.stride = stride;
        self.state.palette_weight_array.pointer = pointer;
        self.state.palette_weight_array.buffer_binding = self.state.array_buffer_binding;
        self.state.palette_weight_array.fixed = type_ == es1::FIXED;
    }
    unsafe fn DrawArrays(&mut self, mode: GLenum, first: GLint, count: GLsizei) {
        let program = match self.ensure_program() {
            Some(program) => program,
            None => return,
        };
        gl::UseProgram(program);
        let mvp = self.state.mvp();
        log_matrix("Final GLES2 projection/MVP upload", &mvp);
        diagnose_matrix_conversion(&self.state.projection.current, &mvp);
        let mvp_loc = gl::GetUniformLocation(program, b"u_mvp\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(mvp_loc, 1, gl::FALSE, mvp.as_ptr());
        let modelview_loc = gl::GetUniformLocation(program, b"u_modelview\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(modelview_loc, 1, gl::FALSE, self.state.modelview.current.as_ptr());
        let texture_matrix_loc = gl::GetUniformLocation(program, b"u_texture_matrix0\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(texture_matrix_loc, 1, gl::FALSE, self.state.texture[0].current.as_ptr());
        let color_loc = gl::GetUniformLocation(program, b"u_color\0".as_ptr() as *const _);
        gl::Uniform4fv(color_loc, 1, self.state.color.as_ptr());
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_lighting_enabled\0".as_ptr() as *const _), self.state.lighting_enabled as GLint);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_light0_enabled\0".as_ptr() as *const _), self.state.light0_enabled as GLint);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_color_material_enabled\0".as_ptr() as *const _), self.state.color_material_enabled as GLint);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_normalize_enabled\0".as_ptr() as *const _), self.state.normalize_enabled as GLint);
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_light0_ambient\0".as_ptr() as *const _), 1, self.state.light0_ambient.as_ptr());
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_light0_diffuse\0".as_ptr() as *const _), 1, self.state.light0_diffuse.as_ptr());
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_light0_position\0".as_ptr() as *const _), 1, self.state.light0_position.as_ptr());
        gl::Uniform3fv(gl::GetUniformLocation(program, b"u_light0_spot_direction\0".as_ptr() as *const _), 1, self.state.light0_spot_direction.as_ptr());
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_light0_spot_cutoff\0".as_ptr() as *const _), self.state.light0_spot_cutoff);
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_light0_spot_exponent\0".as_ptr() as *const _), self.state.light0_spot_exponent);
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_light0_constant_attenuation\0".as_ptr() as *const _), self.state.light0_constant_attenuation);
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_light0_linear_attenuation\0".as_ptr() as *const _), self.state.light0_linear_attenuation);
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_light0_quadratic_attenuation\0".as_ptr() as *const _), self.state.light0_quadratic_attenuation);
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_material_ambient\0".as_ptr() as *const _), 1, self.state.material_ambient.as_ptr());
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_material_diffuse\0".as_ptr() as *const _), 1, self.state.material_diffuse.as_ptr());
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_model_ambient\0".as_ptr() as *const _), 1, self.state.model_ambient.as_ptr());
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_clip_planes\0".as_ptr() as *const _), 6, self.state.clip_planes.as_ptr().cast());
        gl::Uniform1iv(gl::GetUniformLocation(program, b"u_clip_enabled\0".as_ptr() as *const _), 6, self.state.clip_plane_enabled.as_ptr().cast());
        gl::Uniform3fv(gl::GetUniformLocation(program, b"u_point_distance_attenuation\0".as_ptr() as *const _), 1, self.state.point_distance_attenuation.as_ptr());
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_point_fade_threshold\0".as_ptr() as *const _), self.state.point_fade_threshold);
        let alpha_test_loc = gl::GetUniformLocation(program, b"u_alpha_test_enabled\0".as_ptr() as *const _);
        gl::Uniform1i(alpha_test_loc, if self.state.alpha_test_enabled { 1 } else { 0 });
        let alpha_func_loc = gl::GetUniformLocation(program, b"u_alpha_func\0".as_ptr() as *const _);
        gl::Uniform1i(alpha_func_loc, self.state.alpha_func as GLint);
        let alpha_ref_loc = gl::GetUniformLocation(program, b"u_alpha_ref\0".as_ptr() as *const _);
        gl::Uniform1f(alpha_ref_loc, self.state.alpha_ref);
        let fog_enabled_loc = gl::GetUniformLocation(program, b"u_fog_enabled\0".as_ptr() as *const _);
        gl::Uniform1i(fog_enabled_loc, if self.state.fog_enabled { 1 } else { 0 });
        let fog_color_loc = gl::GetUniformLocation(program, b"u_fog_color\0".as_ptr() as *const _);
        gl::Uniform4fv(fog_color_loc, 1, self.state.fog_color.as_ptr());
        let fog_density_loc = gl::GetUniformLocation(program, b"u_fog_density\0".as_ptr() as *const _);
        gl::Uniform1f(fog_density_loc, self.state.fog_density);
        let fog_start_loc = gl::GetUniformLocation(program, b"u_fog_start\0".as_ptr() as *const _);
        gl::Uniform1f(fog_start_loc, self.state.fog_start);
        let fog_end_loc = gl::GetUniformLocation(program, b"u_fog_end\0".as_ptr() as *const _);
        gl::Uniform1f(fog_end_loc, self.state.fog_end);
        let fog_mode_loc = gl::GetUniformLocation(program, b"u_fog_mode\0".as_ptr() as *const _);
        gl::Uniform1i(fog_mode_loc, self.state.fog_mode as GLint);
        let point_size_loc = gl::GetUniformLocation(program, b"u_point_size\0".as_ptr() as *const _);
        gl::Uniform1f(point_size_loc, self.state.point_size);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_point_size_array_enabled\0".as_ptr() as *const _), if self.state.point_size_array.enabled { 1 } else { 0 });
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_matrix_palette_enabled\0".as_ptr() as *const _), if self.state.matrix_palette_enabled { 1 } else { 0 });
        for (i, matrix) in self.state.palette_matrices.iter().enumerate() {
            let name = format!("u_palette_matrices[{}]\\0", i);
            gl::UniformMatrix4fv(gl::GetUniformLocation(program, name.as_ptr() as *const _), 1, gl::FALSE, matrix.current.as_ptr());
        }
        let tex_enabled = self.state.texture_enabled[0];
        let enabled_loc = gl::GetUniformLocation(program, b"u_tex_enabled0\0".as_ptr() as *const _);
        gl::Uniform1i(enabled_loc, if tex_enabled { 1 } else { 0 });
        let mode_loc = gl::GetUniformLocation(program, b"u_tex_mode0\0".as_ptr() as *const _);
        gl::Uniform1i(mode_loc, match self.state.texture_env_mode[0] as GLenum { es1::REPLACE => 1, es1::ADD => 3, es1::DECAL => 4, _ => 2 });
        let env_color_loc = gl::GetUniformLocation(program, b"u_env_color0\0".as_ptr() as *const _);
        gl::Uniform4fv(env_color_loc, 1, self.state.texture_env_color[0].as_ptr());
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_tex0\0".as_ptr() as *const _), 0);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_logic_op_enabled\0".as_ptr() as *const _), self.state.logic_op_enabled as GLint);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_logic_op\0".as_ptr() as *const _), self.state.logic_op as GLint);
        let position = self.state.arrays[0];
        let color = self.state.arrays[1];
        let normal = self.state.arrays[2];
        let tex0 = self.state.texcoord_arrays[0];
        self.bind_array_range(ATTR_POSITION, &position, first, count);
        self.bind_array_range(ATTR_COLOR, &color, first, count);
        self.bind_array_range(ATTR_NORMAL, &normal, first, count);
        self.bind_array_range(ATTR_TEX0, &tex0, first, count);
        let palette_index = self.state.palette_index_array;
        let palette_weight = self.state.palette_weight_array;
        let point_size_array = self.state.point_size_array;
        self.bind_array_range(ATTR_MATRIX_INDEX, &palette_index, first, count);
        self.bind_array_range(ATTR_WEIGHT, &palette_weight, first, count);
        self.bind_array_range(ATTR_POINT_SIZE, &point_size_array, first, count);
        if coordinate_trace_enabled() && position.enabled && position.buffer_binding == 0 && !position.pointer.is_null() {
            let components = position.size.max(1) as usize;
            let first_offset = (first.max(0) as usize).saturating_mul(if position.stride > 0 { position.stride as usize } else { components * std::mem::size_of::<GLfloat>() });
            let raw = (position.pointer as *const u8).add(first_offset) as *const GLfloat;
            let vertex = [raw.read_unaligned(), if components > 1 { raw.add(1).read_unaligned() } else { 0.0 }, if components > 2 { raw.add(2).read_unaligned() } else { 0.0 }];
            log_vertex_transformation(vertex, transform_vec4(&mvp, [vertex[0], vertex[1], vertex[2], 1.0]));
        }
        gl::DrawArrays(mode, first, count);
        gl::BindBuffer(gl::ARRAY_BUFFER, self.state.array_buffer_binding);
    }
    unsafe fn DrawElements(&mut self, mode: GLenum, count: GLsizei, type_: GLenum, indices: *const GLvoid) {
        let program = match self.ensure_program() {
            Some(program) => program,
            None => return,
        };
        gl::UseProgram(program);
        let mvp = self.state.mvp();
        log_matrix("Final GLES2 projection/MVP upload", &mvp);
        diagnose_matrix_conversion(&self.state.projection.current, &mvp);
        let mvp_loc = gl::GetUniformLocation(program, b"u_mvp\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(mvp_loc, 1, gl::FALSE, mvp.as_ptr());
        let modelview_loc = gl::GetUniformLocation(program, b"u_modelview\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(modelview_loc, 1, gl::FALSE, self.state.modelview.current.as_ptr());
        let texture_matrix_loc = gl::GetUniformLocation(program, b"u_texture_matrix0\0".as_ptr() as *const _);
        gl::UniformMatrix4fv(texture_matrix_loc, 1, gl::FALSE, self.state.texture[0].current.as_ptr());
        let color_loc = gl::GetUniformLocation(program, b"u_color\0".as_ptr() as *const _);
        gl::Uniform4fv(color_loc, 1, self.state.color.as_ptr());
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_lighting_enabled\0".as_ptr() as *const _), self.state.lighting_enabled as GLint);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_light0_enabled\0".as_ptr() as *const _), self.state.light0_enabled as GLint);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_color_material_enabled\0".as_ptr() as *const _), self.state.color_material_enabled as GLint);
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_normalize_enabled\0".as_ptr() as *const _), self.state.normalize_enabled as GLint);
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_light0_ambient\0".as_ptr() as *const _), 1, self.state.light0_ambient.as_ptr());
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_light0_diffuse\0".as_ptr() as *const _), 1, self.state.light0_diffuse.as_ptr());
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_light0_position\0".as_ptr() as *const _), 1, self.state.light0_position.as_ptr());
        gl::Uniform3fv(gl::GetUniformLocation(program, b"u_light0_spot_direction\0".as_ptr() as *const _), 1, self.state.light0_spot_direction.as_ptr());
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_light0_spot_cutoff\0".as_ptr() as *const _), self.state.light0_spot_cutoff);
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_light0_spot_exponent\0".as_ptr() as *const _), self.state.light0_spot_exponent);
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_light0_constant_attenuation\0".as_ptr() as *const _), self.state.light0_constant_attenuation);
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_light0_linear_attenuation\0".as_ptr() as *const _), self.state.light0_linear_attenuation);
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_light0_quadratic_attenuation\0".as_ptr() as *const _), self.state.light0_quadratic_attenuation);
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_material_ambient\0".as_ptr() as *const _), 1, self.state.material_ambient.as_ptr());
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_material_diffuse\0".as_ptr() as *const _), 1, self.state.material_diffuse.as_ptr());
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_model_ambient\0".as_ptr() as *const _), 1, self.state.model_ambient.as_ptr());
        gl::Uniform4fv(gl::GetUniformLocation(program, b"u_clip_planes\0".as_ptr() as *const _), 6, self.state.clip_planes.as_ptr().cast());
        gl::Uniform1iv(gl::GetUniformLocation(program, b"u_clip_enabled\0".as_ptr() as *const _), 6, self.state.clip_plane_enabled.as_ptr().cast());
        gl::Uniform3fv(gl::GetUniformLocation(program, b"u_point_distance_attenuation\0".as_ptr() as *const _), 1, self.state.point_distance_attenuation.as_ptr());
        gl::Uniform1f(gl::GetUniformLocation(program, b"u_point_fade_threshold\0".as_ptr() as *const _), self.state.point_fade_threshold);
        let point_size_loc = gl::GetUniformLocation(program, b"u_point_size\0".as_ptr() as *const _);
        gl::Uniform1f(point_size_loc, self.state.point_size);
        let tex_enabled = self.state.texture_enabled[0];
        let enabled_loc = gl::GetUniformLocation(program, b"u_tex_enabled0\0".as_ptr() as *const _);
        gl::Uniform1i(enabled_loc, if tex_enabled { 1 } else { 0 });
        let mode_loc = gl::GetUniformLocation(program, b"u_tex_mode0\0".as_ptr() as *const _);
        gl::Uniform1i(mode_loc, match self.state.texture_env_mode[0] as GLenum { es1::REPLACE => 1, es1::ADD => 3, es1::DECAL => 1, _ => 2 });
        gl::Uniform1i(gl::GetUniformLocation(program, b"u_tex0\0".as_ptr() as *const _), 0);
        let position = self.state.arrays[0];
        let color = self.state.arrays[1];
        let normal = self.state.arrays[2];
        let tex0 = self.state.texcoord_arrays[0];
        self.bind_array_range(ATTR_POSITION, &position, 0, count);
        self.bind_array_range(ATTR_COLOR, &color, 0, count);
        self.bind_array_range(ATTR_NORMAL, &normal, 0, count);
        self.bind_array_range(ATTR_TEX0, &tex0, 0, count);
        let palette_index = self.state.palette_index_array;
        let palette_weight = self.state.palette_weight_array;
        let point_size_array = self.state.point_size_array;
        self.bind_array_range(ATTR_MATRIX_INDEX, &palette_index, 0, count);
        self.bind_array_range(ATTR_WEIGHT, &palette_weight, 0, count);
        self.bind_array_range(ATTR_POINT_SIZE, &point_size_array, 0, count);
        if coordinate_trace_enabled() {
            log_matrix("GLES2 indexed draw matrix", &mvp);
        }
        let (draw_indices, restore_element_buffer) = self.stage_client_indices(type_, indices, count);
        gl::DrawElements(mode, count, type_, draw_indices);
        if restore_element_buffer {
            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, self.state.element_array_buffer_binding);
        }
        gl::BindBuffer(gl::ARRAY_BUFFER, self.state.array_buffer_binding);
    }
}

impl GLES1OnGLES2<'_> {
    fn ensure_program(&mut self) -> Option<GLuint> {
        if let Some(program) = self.state.program {
            return Some(program);
        }
        match create_program() {
            Ok(program) => {
                self.state.program = Some(program);
                Some(program)
            }
            Err(error) => {
                if !self.state.program_creation_failed {
                    self.state.program_creation_failed = true;
                    unsafe {
                        let version = CStr::from_ptr(gl::GetString(gl::VERSION) as *const _);
                        let vendor = CStr::from_ptr(gl::GetString(gl::VENDOR) as *const _);
                        let renderer = CStr::from_ptr(gl::GetString(gl::RENDERER) as *const _);
                        log!(
                            "GLES1-on-GLES2 host: version={}, vendor={}, renderer={}",
                            version.to_string_lossy(),
                            vendor.to_string_lossy(),
                            renderer.to_string_lossy(),
                        );
                    }
                    log!(
                        "Error: GLES1-on-GLES2 shader program unavailable; texture_enabled={}, lighting_enabled={}, fog_enabled={}, alpha_test_enabled={}, matrix_palette_enabled={}; {}",
                        self.state.texture_enabled.iter().any(|enabled| *enabled),
                        self.state.lighting_enabled,
                        self.state.fog_enabled,
                        self.state.alpha_test_enabled,
                        self.state.matrix_palette_enabled,
                        error
                    );
                }
                None
            }
        }
    }

    unsafe fn stage_client_indices(&mut self, type_: GLenum, indices: *const GLvoid, count: GLsizei) -> (*const GLvoid, bool) {
        if self.state.element_array_buffer_binding != 0 || indices.is_null() || count <= 0 {
            return (indices, false);
        }
        let index_size = match type_ {
            gl::UNSIGNED_BYTE => 1usize,
            gl::UNSIGNED_SHORT => 2usize,
            gl::UNSIGNED_INT => 4usize,
            _ => return (indices, false),
        };
        let byte_count = (count as usize).saturating_mul(index_size);
        if byte_count == 0 {
            return (indices, false);
        }
        if self.state.client_element_vbo == 0 {
            gl::GenBuffers(1, &mut self.state.client_element_vbo);
        }
        gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, self.state.client_element_vbo);
        gl::BufferData(gl::ELEMENT_ARRAY_BUFFER, byte_count as GLsizeiptr, indices, gl::STREAM_DRAW);
        (std::ptr::null(), true)
    }

    unsafe fn bind_array(&mut self, index: GLuint, array: &ArrayState) {
        self.bind_array_range(index, array, 0, 0);
    }

    unsafe fn bind_array_range(&mut self, index: GLuint, array: &ArrayState, first: GLint, count: GLsizei) {
        if !array.enabled {
            gl::DisableVertexAttribArray(index);
            let value = if index == ATTR_COLOR { [1.0, 1.0, 1.0, 1.0] } else if index == ATTR_TEX0 { self.state.texcoords[0] } else if index == ATTR_NORMAL { [self.state.normal[0], self.state.normal[1], self.state.normal[2], 1.0] } else { [0.0, 0.0, 0.0, 1.0] };
            gl::VertexAttrib4fv(index, value.as_ptr());
            return;
        }
        if array.buffer_binding != 0 {
            let Some(bytes) = self.state.array_buffer_data.get(&array.buffer_binding) else {
                gl::DisableVertexAttribArray(index);
                return;
            };
            let component_size = match array.type_ {
                gl::BYTE | gl::UNSIGNED_BYTE => 1usize,
                gl::SHORT | gl::UNSIGNED_SHORT => 2usize,
                gl::FIXED | gl::FLOAT => 4usize,
                _ => {
                    gl::DisableVertexAttribArray(index);
                    return;
                }
            };
            let components = array.size.max(1) as usize;
            let stride = if array.stride > 0 { array.stride as usize } else { components * component_size };
            let offset = array.pointer as usize;
            let first = first.max(0) as usize;
            let count = count.max(0) as usize;
            let upload_count = first.saturating_add(count);
            let required = upload_count.saturating_sub(1).saturating_mul(stride).saturating_add(components * component_size);
            if offset > bytes.len() || required > bytes.len().saturating_sub(offset) {
                gl::DisableVertexAttribArray(index);
                return;
            }
            let vbo_slot = (index as usize).min(self.state.client_array_vbos.len() - 1);
            if self.state.client_array_vbos[vbo_slot] == 0 {
                gl::GenBuffers(1, &mut self.state.client_array_vbos[vbo_slot]);
            }
            let vbo = self.state.client_array_vbos[vbo_slot];
            gl::BindBuffer(gl::ARRAY_BUFFER, vbo);
            if array.type_ == gl::FIXED {
                let source = &bytes[offset..offset + required];
                let mut converted = Vec::with_capacity(upload_count * components * std::mem::size_of::<GLfloat>());
                for vertex in 0..upload_count {
                    let base = vertex.saturating_mul(stride);
                    for component in 0..components {
                        let start = base + component * 4;
                        let value = GLfixed::from_ne_bytes(source[start..start + 4].try_into().unwrap());
                        converted.extend_from_slice(&fixed_to_float(value).to_ne_bytes());
                    }
                }
                gl::BufferData(gl::ARRAY_BUFFER, converted.len() as GLsizeiptr, converted.as_ptr().cast(), gl::STREAM_DRAW);
                gl::EnableVertexAttribArray(index);
                gl::VertexAttribPointer(index, array.size, gl::FLOAT, if array.normalized { gl::TRUE } else { gl::FALSE }, (components * std::mem::size_of::<GLfloat>()) as GLsizei, std::ptr::null());
            } else {
                gl::BufferData(gl::ARRAY_BUFFER, required as GLsizeiptr, bytes[offset..offset + required].as_ptr().cast(), gl::STREAM_DRAW);
                gl::EnableVertexAttribArray(index);
                gl::VertexAttribPointer(index, array.size, array.type_, if array.normalized { gl::TRUE } else { gl::FALSE }, array.stride, std::ptr::null());
            }
            return;
        }
        if array.pointer.is_null() || count <= 0 || array.size <= 0 {
            gl::DisableVertexAttribArray(index);
            return;
        }
        let component_size = match array.type_ {
            gl::BYTE | gl::UNSIGNED_BYTE => 1usize,
            gl::SHORT | gl::UNSIGNED_SHORT => 2usize,
            gl::FIXED | gl::FLOAT => 4usize,
            _ => {
                gl::DisableVertexAttribArray(index);
                return;
            }
        };
        let components = array.size as usize;
        let stride = if array.stride > 0 { array.stride as usize } else { components * component_size };
        let first = first.max(0) as usize;
        let count = count as usize;
        let upload_count = first.saturating_add(count);
        let byte_count = upload_count.saturating_sub(1).saturating_mul(stride).saturating_add(components * component_size);
        let vbo_slot = (index as usize).min(self.state.client_array_vbos.len() - 1);
        if self.state.client_array_vbos[vbo_slot] == 0 {
            gl::GenBuffers(1, &mut self.state.client_array_vbos[vbo_slot]);
        }
        let vbo = self.state.client_array_vbos[vbo_slot];
        gl::BindBuffer(gl::ARRAY_BUFFER, vbo);
        if array.type_ == gl::FIXED {
            let mut converted = Vec::with_capacity(upload_count * components * std::mem::size_of::<GLfloat>());
            for vertex in 0..upload_count {
                let source = (array.pointer as *const u8).add(vertex.saturating_mul(stride));
                for component in 0..components {
                    let value = (source.add(component * 4) as *const GLfixed).read_unaligned();
                    converted.extend_from_slice(&fixed_to_float(value).to_ne_bytes());
                }
            }
            gl::BufferData(gl::ARRAY_BUFFER, converted.len() as GLsizeiptr, converted.as_ptr().cast(), gl::STREAM_DRAW);
            gl::EnableVertexAttribArray(index);
            gl::VertexAttribPointer(index, array.size, gl::FLOAT, if array.normalized { gl::TRUE } else { gl::FALSE }, (components * std::mem::size_of::<GLfloat>()) as GLsizei, std::ptr::null());
        } else {
            let source = array.pointer as *const u8;
            gl::BufferData(gl::ARRAY_BUFFER, byte_count as GLsizeiptr, source.cast(), gl::STREAM_DRAW);
            gl::EnableVertexAttribArray(index);
            gl::VertexAttribPointer(index, array.size, array.type_, if array.normalized { gl::TRUE } else { gl::FALSE }, array.stride, std::ptr::null());
        }
        if index == ATTR_POSITION {
            log_once!("GLES1-on-GLES2: uploaded client-side vertex arrays to a host VBO for GLES2 compatibility");
        }
    }
}
