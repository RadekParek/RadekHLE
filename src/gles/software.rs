use super::gles11_raw as gl;
use super::gles11_raw::types::*;
use super::gles_generic::GLES;
use super::GLESContext;
use crate::window::{GLContext, Window};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_void, CStr, CString};
use std::marker::PhantomData;

const IDENTITY: [f32; 16] = [
    1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
];

const GL_CURRENT_PROGRAM: GLenum = 0x8B8D;
const GL_COMPILE_STATUS: GLenum = 0x8B81;
const GL_INFO_LOG_LENGTH: GLenum = 0x8B84;
const GL_LINK_STATUS: GLenum = 0x8B82;
const GL_VALIDATE_STATUS: GLenum = 0x8B83;

#[derive(Clone, Copy)]
struct ArrayState {
    size: GLint,
    type_: GLenum,
    stride: GLsizei,
    pointer: *const c_void,
    buffer: GLuint,
    enabled: bool,
    normalized: bool,
}

impl Default for ArrayState {
    fn default() -> Self {
        Self {
            size: 4,
            type_: gl::FLOAT,
            stride: 0,
            pointer: std::ptr::null(),
            buffer: 0,
            enabled: false,
            normalized: false,
        }
    }
}

#[derive(Clone)]
struct Texture {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
    min_filter: GLenum,
    mag_filter: GLenum,
    wrap_s: GLenum,
    wrap_t: GLenum,
    env_mode: GLenum,
}

struct Vertex {
    position: [f32; 4],
    color: [f32; 4],
    texcoord: [f32; 2],
}

#[derive(Clone)]
pub struct SoftwareState {
    width: usize,
    height: usize,
    color: Vec<u8>,
    depth: Vec<f32>,
    textures: HashMap<GLuint, Texture>,
    buffers: HashMap<GLuint, Vec<u8>>,
    next_id: GLuint,
    bound_texture: [GLuint; 4],
    active_texture: usize,
    bound_array_buffer: GLuint,
    bound_element_buffer: GLuint,
    bound_framebuffer: GLuint,
    bound_renderbuffer: GLuint,
    renderbuffers: HashSet<GLuint>,
    framebuffers: HashSet<GLuint>,
    arrays: [ArrayState; 4],
    attributes: [ArrayState; 16],
    current_color: [f32; 4],
    current_normal: [f32; 3],
    current_texcoord: [[f32; 4]; 4],
    enabled: HashSet<GLenum>,
    blend_src: GLenum,
    blend_dst: GLenum,
    blend_equation: GLenum,
    depth_func: GLenum,
    depth_mask: bool,
    depth_range: [f32; 2],
    cull_face: GLenum,
    front_face: GLenum,
    alpha_func: GLenum,
    alpha_ref: f32,
    polygon_offset: [f32; 2],
    shade_model: GLenum,
    color_mask: [bool; 4],
    clear_color: [f32; 4],
    clear_depth: f32,
    clear_stencil: GLint,
    viewport: [GLint; 4],
    scissor: [GLint; 4],
    matrix_mode: GLenum,
    modelview: [f32; 16],
    projection: [f32; 16],
    texture_matrices: [[f32; 16]; 4],
    modelview_stack: Vec<[f32; 16]>,
    projection_stack: Vec<[f32; 16]>,
    texture_stacks: [Vec<[f32; 16]>; 4],
    error: GLenum,
    shader_ids: HashSet<GLuint>,
    program_ids: HashSet<GLuint>,
    current_program: GLuint,
    strings: HashMap<GLenum, CString>,
}

impl SoftwareState {
    fn new(width: u32, height: u32) -> Self {
        let width = width.max(1) as usize;
        let height = height.max(1) as usize;
        let mut strings = HashMap::new();
        strings.insert(gl::VENDOR, CString::new("RadekHLE").unwrap());
        strings.insert(
            gl::RENDERER,
            CString::new("RadekHLE CPU rasterizer").unwrap(),
        );
        strings.insert(
            gl::VERSION,
            CString::new("OpenGL ES-CM 1.1 RadekHLE software").unwrap(),
        );
        strings.insert(
            gl::EXTENSIONS,
            CString::new("GL_OES_framebuffer_object GL_OES_draw_texture").unwrap(),
        );
        Self {
            width,
            height,
            color: vec![0; width * height * 4],
            depth: vec![1.0; width * height],
            textures: HashMap::new(),
            buffers: HashMap::new(),
            next_id: 1,
            bound_texture: [0; 4],
            active_texture: 0,
            bound_array_buffer: 0,
            bound_element_buffer: 0,
            bound_framebuffer: 0,
            bound_renderbuffer: 0,
            renderbuffers: HashSet::new(),
            framebuffers: HashSet::new(),
            arrays: [ArrayState::default(); 4],
            attributes: [ArrayState::default(); 16],
            current_color: [1.0; 4],
            current_normal: [0.0, 0.0, 1.0],
            current_texcoord: [[0.0, 0.0, 0.0, 1.0]; 4],
            enabled: HashSet::new(),
            blend_src: gl::ONE,
            blend_dst: gl::ZERO,
            blend_equation: gl::FUNC_ADD_OES,
            depth_func: gl::LESS,
            depth_mask: true,
            depth_range: [0.0, 1.0],
            cull_face: gl::BACK,
            front_face: gl::CCW,
            alpha_func: gl::ALWAYS,
            alpha_ref: 0.0,
            polygon_offset: [0.0, 0.0],
            shade_model: gl::SMOOTH,
            color_mask: [true; 4],
            clear_color: [0.0, 0.0, 0.0, 0.0],
            clear_depth: 1.0,
            clear_stencil: 0,
            viewport: [0, 0, width as GLint, height as GLint],
            scissor: [0, 0, width as GLint, height as GLint],
            matrix_mode: gl::MODELVIEW,
            modelview: IDENTITY,
            projection: IDENTITY,
            texture_matrices: [IDENTITY; 4],
            modelview_stack: Vec::new(),
            projection_stack: Vec::new(),
            texture_stacks: std::array::from_fn(|_| Vec::new()),
            error: gl::NO_ERROR,
            shader_ids: HashSet::new(),
            program_ids: HashSet::new(),
            current_program: 0,
            strings,
        }
    }

    fn error(&mut self, error: GLenum) {
        if self.error == gl::NO_ERROR {
            self.error = error;
        }
    }

    fn alloc_id(&mut self) -> GLuint {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    fn matrix(&self) -> [f32; 16] {
        match self.matrix_mode {
            gl::PROJECTION => self.projection,
            gl::TEXTURE => self.texture_matrices[self.active_texture],
            _ => self.modelview,
        }
    }

    fn matrix_mut(&mut self) -> &mut [f32; 16] {
        match self.matrix_mode {
            gl::PROJECTION => &mut self.projection,
            gl::TEXTURE => &mut self.texture_matrices[self.active_texture],
            _ => &mut self.modelview,
        }
    }

    fn push_matrix(&mut self) {
        let current = self.matrix();
        match self.matrix_mode {
            gl::PROJECTION => self.projection_stack.push(current),
            gl::TEXTURE => self.texture_stacks[self.active_texture].push(current),
            _ => self.modelview_stack.push(current),
        }
    }

    fn pop_matrix(&mut self) {
        let value = match self.matrix_mode {
            gl::PROJECTION => self.projection_stack.pop(),
            gl::TEXTURE => self.texture_stacks[self.active_texture].pop(),
            _ => self.modelview_stack.pop(),
        };
        if let Some(value) = value {
            *self.matrix_mut() = value;
        } else {
            self.error(gl::STACK_UNDERFLOW);
        }
    }

    fn transform(&self, vertex: [f32; 4]) -> [f32; 4] {
        mul_vec(mul(self.projection, self.modelview), vertex)
    }

    fn texture(&self) -> Option<&Texture> {
        self.textures.get(&self.bound_texture[self.active_texture])
    }

    fn texture_mut(&mut self) -> Option<&mut Texture> {
        let id = self.bound_texture[self.active_texture];
        self.textures.get_mut(&id)
    }

    fn resize_framebuffer(&mut self, width: usize, height: usize) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.color.resize(self.width * self.height * 4, 0);
        self.depth
            .resize(self.width * self.height, self.clear_depth);
        self.viewport = [0, 0, self.width as GLint, self.height as GLint];
        self.scissor = [0, 0, self.width as GLint, self.height as GLint];
    }
}

pub struct SoftwareGLESContext {
    state: SoftwareState,
}

impl GLESContext for SoftwareGLESContext {
    fn description() -> &'static str {
        "CPU software OpenGL ES 1.1 rasterizer"
    }

    fn new(window: &mut Window) -> Result<Self, String> {
        let (width, height) = window.framebuffer_size();
        Ok(Self {
            state: SoftwareState::new(width, height),
        })
    }

    fn make_current<'gl_ctx, 'win: 'gl_ctx>(
        &'gl_ctx mut self,
        _window: &'win mut Window,
    ) -> Box<dyn GLES + 'gl_ctx> {
        Box::new(SoftwareGLES {
            state: &mut self.state,
            _lifetime: PhantomData,
        })
    }

    unsafe fn make_current_unchecked_for_window<'gl_ctx>(
        &'gl_ctx mut self,
        _make_current_fn: &mut dyn FnMut(&GLContext),
        _loader_fn: &mut dyn FnMut(&'static str) -> *const c_void,
    ) -> Box<dyn GLES + 'gl_ctx> {
        Box::new(SoftwareGLES {
            state: &mut self.state,
            _lifetime: PhantomData,
        })
    }
}

pub struct SoftwareGLES<'a> {
    state: &'a mut SoftwareState,
    _lifetime: PhantomData<&'a mut SoftwareState>,
}

impl SoftwareGLES<'_> {
    fn read_scalar(&self, array: ArrayState, index: usize, components: usize) -> f32 {
        let component_size = match array.type_ {
            gl::BYTE | gl::UNSIGNED_BYTE => 1,
            gl::SHORT | gl::UNSIGNED_SHORT | gl::FIXED => 2,
            _ => 4,
        };
        let stride = if array.stride == 0 {
            component_size * array.size.max(1) as usize
        } else {
            array.stride as usize
        };
        let offset = index
            .saturating_mul(stride)
            .saturating_add(components.saturating_mul(component_size));
        let ptr = if array.buffer != 0 {
            self.state
                .buffers
                .get(&array.buffer)
                .and_then(|data| data.get(offset..))
                .map(|data| data.as_ptr())
        } else {
            if array.pointer.is_null() {
                None
            } else {
                Some((array.pointer as *const u8).wrapping_add(offset))
            }
        };
        let Some(ptr) = ptr else { return 0.0 };
        unsafe {
            match array.type_ {
                gl::BYTE => *(ptr as *const i8) as f32 / 127.0,
                gl::UNSIGNED_BYTE => {
                    let value = *ptr;
                    if array.normalized {
                        value as f32 / 255.0
                    } else {
                        value as f32
                    }
                }
                gl::SHORT => *(ptr as *const i16) as f32 / 32767.0,
                gl::UNSIGNED_SHORT => *(ptr as *const u16) as f32,
                gl::FIXED => *(ptr as *const i32) as f32 / 65536.0,
                _ => *(ptr as *const f32),
            }
        }
    }

    fn array_value(&self, array: ArrayState, index: usize, defaults: [f32; 4]) -> [f32; 4] {
        let mut value = defaults;
        if !array.enabled {
            return value;
        }
        for i in 0..(array.size.clamp(1, 4) as usize) {
            value[i] = self.read_scalar(array, index, i);
        }
        value
    }

    fn vertex(&self, index: usize) -> Vertex {
        let position_array = if self.state.attributes[0].enabled {
            self.state.attributes[0]
        } else {
            self.state.arrays[3]
        };
        let color_array = if self.state.attributes[1].enabled {
            self.state.attributes[1]
        } else {
            self.state.arrays[0]
        };
        let tex_array = if self.state.attributes[2].enabled {
            self.state.attributes[2]
        } else {
            self.state.arrays[2]
        };
        let p = self.array_value(position_array, index, [0.0, 0.0, 0.0, 1.0]);
        let c = self.array_value(color_array, index, self.state.current_color);
        let t = self.array_value(
            tex_array,
            index,
            [
                self.state.current_texcoord[self.state.active_texture][0],
                self.state.current_texcoord[self.state.active_texture][1],
                0.0,
                1.0,
            ],
        );
        Vertex {
            position: self.state.transform([
                p[0],
                p[1],
                p[2],
                if position_array.size >= 4 { p[3] } else { 1.0 },
            ]),
            color: c,
            texcoord: [t[0], t[1]],
        }
    }

    fn draw_triangle(&mut self, a: Vertex, b: Vertex, c: Vertex) {
        let [a, b, c] = [a, b, c].map(|v| project(v, self.state.viewport));
        let min_x =
            a.x.min(b.x)
                .min(c.x)
                .floor()
                .max(self.state.viewport[0] as f32) as i32;
        let max_x =
            a.x.max(b.x)
                .max(c.x)
                .ceil()
                .min((self.state.viewport[0] + self.state.viewport[2] - 1) as f32)
                as i32;
        let min_y =
            a.y.min(b.y)
                .min(c.y)
                .floor()
                .max(self.state.viewport[1] as f32) as i32;
        let max_y =
            a.y.max(b.y)
                .max(c.y)
                .ceil()
                .min((self.state.viewport[1] + self.state.viewport[3] - 1) as f32)
                as i32;
        let area = edge(a.x, a.y, b.x, b.y, c.x, c.y);
        if area.abs() < f32::EPSILON {
            return;
        }
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let w0 = edge(b.x, b.y, c.x, c.y, px, py) / area;
                let w1 = edge(c.x, c.y, a.x, a.y, px, py) / area;
                let w2 = edge(a.x, a.y, b.x, b.y, px, py) / area;
                if (area > 0.0 && (w0 < 0.0 || w1 < 0.0 || w2 < 0.0))
                    || (area < 0.0 && (w0 > 0.0 || w1 > 0.0 || w2 > 0.0))
                {
                    continue;
                }
                let depth = (self.state.depth_range[0]
                    + (self.state.depth_range[1] - self.state.depth_range[0])
                        * (a.z * w0 + b.z * w1 + c.z * w2))
                    .clamp(0.0, 1.0)
                    + if self.state.enabled.contains(&gl::POLYGON_OFFSET_FILL) {
                        (self.state.polygon_offset[0] * 0.000001
                            + self.state.polygon_offset[1] * 0.000001)
                            .clamp(-0.0001, 0.0001)
                    } else {
                        0.0
                    };
                let winding_is_front = if self.state.front_face == gl::CCW {
                    area > 0.0
                } else {
                    area < 0.0
                };
                if self.state.enabled.contains(&gl::CULL_FACE)
                    && ((self.state.cull_face == gl::BACK && !winding_is_front)
                        || (self.state.cull_face == gl::FRONT && winding_is_front)
                        || self.state.cull_face == gl::FRONT_AND_BACK)
                {
                    continue;
                }
                if x < 0 || y < 0 || x >= self.state.width as i32 || y >= self.state.height as i32 {
                    continue;
                }
                let offset = y as usize * self.state.width + x as usize;
                if self.state.enabled.contains(&gl::DEPTH_TEST)
                    && !depth_pass(self.state.depth_func, depth, self.state.depth[offset])
                {
                    continue;
                }
                if self.state.enabled.contains(&gl::DEPTH_TEST) && self.state.depth_mask {
                    self.state.depth[offset] = depth;
                }
                let color = lerp3(a.color, b.color, c.color, w0, w1, w2);
                let uv = [
                    a.u * w0 + b.u * w1 + c.u * w2,
                    a.v * w0 + b.v * w1 + c.v * w2,
                ];
                let src = self.sample(uv, color);
                if self.state.enabled.contains(&gl::ALPHA_TEST)
                    && !alpha_pass(self.state.alpha_func, src[3], self.state.alpha_ref)
                {
                    continue;
                }
                let dst_offset = offset * 4;
                let dst = [
                    self.state.color[dst_offset] as f32 / 255.0,
                    self.state.color[dst_offset + 1] as f32 / 255.0,
                    self.state.color[dst_offset + 2] as f32 / 255.0,
                    self.state.color[dst_offset + 3] as f32 / 255.0,
                ];
                let out = blend_equation(
                    src,
                    dst,
                    self.state.blend_src,
                    self.state.blend_dst,
                    self.state.blend_equation,
                );
                for i in 0..4 {
                    if self.state.color_mask[i] {
                        self.state.color[dst_offset + i] =
                            (out[i].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
                    }
                }
            }
        }
    }

    fn sample(&self, uv: [f32; 2], color: [f32; 4]) -> [f32; 4] {
        let Some(texture) = self.state.texture() else {
            return color;
        };
        if !self.state.enabled.contains(&gl::TEXTURE_2D)
            || texture.width == 0
            || texture.height == 0
        {
            return color;
        }
        let s = wrap(uv[0], texture.wrap_s);
        let t = wrap(uv[1], texture.wrap_t);
        let x = (s * (texture.width.saturating_sub(1)) as f32)
            .round()
            .clamp(0.0, texture.width.saturating_sub(1) as f32) as usize;
        let y = ((1.0 - t) * (texture.height.saturating_sub(1)) as f32)
            .round()
            .clamp(0.0, texture.height.saturating_sub(1) as f32) as usize;
        let p = (y * texture.width + x) * 4;
        let tex = [
            texture.pixels[p] as f32 / 255.0,
            texture.pixels[p + 1] as f32 / 255.0,
            texture.pixels[p + 2] as f32 / 255.0,
            texture.pixels[p + 3] as f32 / 255.0,
        ];
        match texture.env_mode {
            gl::REPLACE => tex,
            _ => [
                tex[0] * color[0],
                tex[1] * color[1],
                tex[2] * color[2],
                tex[3] * color[3],
            ],
        }
    }

    fn index_at(&self, data: *const c_void, index: usize, type_: GLenum) -> usize {
        let size = if type_ == gl::UNSIGNED_SHORT { 2 } else { 1 };
        let ptr = if self.state.bound_element_buffer != 0 {
            self.state
                .buffers
                .get(&self.state.bound_element_buffer)
                .and_then(|v| v.get(index * size..))
                .map(|v| v.as_ptr())
        } else if data.is_null() {
            None
        } else {
            Some((data as *const u8).wrapping_add(index * size))
        };
        let Some(ptr) = ptr else { return 0 };
        unsafe {
            if type_ == gl::UNSIGNED_SHORT {
                *(ptr as *const u16) as usize
            } else {
                *ptr as usize
            }
        }
    }
}

impl GLES for SoftwareGLES<'_> {
    unsafe fn driver_description(&self) -> String {
        "OpenGL ES 1.1 / RadekHLE / CPU rasterizer".to_owned()
    }
    unsafe fn GetError(&mut self) -> GLenum {
        let value = self.state.error;
        self.state.error = gl::NO_ERROR;
        value
    }
    unsafe fn IsBuffer(&mut self, buffer: GLuint) -> GLboolean {
        self.state.buffers.contains_key(&buffer) as GLboolean
    }
    unsafe fn Enable(&mut self, cap: GLenum) {
        self.state.enabled.insert(cap);
    }
    unsafe fn IsEnabled(&mut self, cap: GLenum) -> GLboolean {
        if self.state.enabled.contains(&cap) {
            gl::TRUE
        } else {
            gl::FALSE
        }
    }
    unsafe fn Disable(&mut self, cap: GLenum) {
        self.state.enabled.remove(&cap);
    }
    unsafe fn ClientActiveTexture(&mut self, texture: GLenum) {
        self.state.active_texture = texture.saturating_sub(gl::TEXTURE0) as usize;
    }
    unsafe fn EnableClientState(&mut self, array: GLenum) {
        if let Some(a) = array_index(array) {
            self.state.arrays[a].enabled = true;
        }
    }
    unsafe fn DisableClientState(&mut self, array: GLenum) {
        if let Some(a) = array_index(array) {
            self.state.arrays[a].enabled = false;
        }
    }
    unsafe fn GetIntegerv(&mut self, pname: GLenum, params: *mut GLint) {
        if params.is_null() {
            self.state.error(gl::INVALID_VALUE);
            return;
        }
        let value = match pname {
            gl::VIEWPORT => self.state.viewport.to_vec(),
            gl::SCISSOR_BOX => self.state.scissor.to_vec(),
            gl::ACTIVE_TEXTURE => {
                vec![(gl::TEXTURE0 + self.state.active_texture as GLenum) as GLint]
            }
            gl::CLIENT_ACTIVE_TEXTURE => {
                vec![(gl::TEXTURE0 + self.state.active_texture as GLenum) as GLint]
            }
            gl::ARRAY_BUFFER_BINDING => vec![self.state.bound_array_buffer as GLint],
            gl::ELEMENT_ARRAY_BUFFER_BINDING => vec![self.state.bound_element_buffer as GLint],
            gl::FRAMEBUFFER_BINDING_OES => vec![self.state.bound_framebuffer as GLint],
            gl::RENDERBUFFER_BINDING_OES => vec![self.state.bound_renderbuffer as GLint],
            gl::TEXTURE_BINDING_2D => {
                vec![self.state.bound_texture[self.state.active_texture] as GLint]
            }
            GL_CURRENT_PROGRAM => vec![self.state.current_program as GLint],
            gl::MAX_TEXTURE_SIZE => vec![4096],
            gl::MAX_TEXTURE_UNITS => vec![4],
            gl::MATRIX_MODE => vec![self.state.matrix_mode as GLint],
            _ => vec![0],
        };
        for (i, item) in value.into_iter().enumerate() {
            *params.add(i) = item;
        }
    }
    unsafe fn GetFloatv(&mut self, pname: GLenum, params: *mut GLfloat) {
        if params.is_null() {
            self.state.error(gl::INVALID_VALUE);
            return;
        }
        if pname == gl::MODELVIEW_MATRIX {
            params.copy_from_nonoverlapping(self.state.modelview.as_ptr(), 16);
        } else if pname == gl::PROJECTION_MATRIX {
            params.copy_from_nonoverlapping(self.state.projection.as_ptr(), 16);
        } else if pname == gl::CURRENT_COLOR {
            params.copy_from_nonoverlapping(self.state.current_color.as_ptr(), 4);
        }
    }
    unsafe fn GetString(&mut self, name: GLenum) -> *const GLubyte {
        self.state
            .strings
            .get(&name)
            .map_or(std::ptr::null(), |value| value.as_ptr() as *const GLubyte)
    }
    unsafe fn Finish(&mut self) {}
    unsafe fn Flush(&mut self) {}
    unsafe fn BlendFunc(&mut self, src: GLenum, dst: GLenum) {
        self.state.blend_src = src;
        self.state.blend_dst = dst;
    }
    unsafe fn BlendEquationOES(&mut self, mode: GLenum) {
        self.state.blend_equation = mode;
    }
    unsafe fn AlphaFunc(&mut self, func: GLenum, reference: GLclampf) {
        self.state.alpha_func = func;
        self.state.alpha_ref = reference.clamp(0.0, 1.0);
    }
    unsafe fn AlphaFuncx(&mut self, func: GLenum, reference: GLclampx) {
        self.AlphaFunc(func, reference as f32 / 65536.0);
    }
    unsafe fn CullFace(&mut self, mode: GLenum) {
        if matches!(mode, gl::FRONT | gl::BACK | gl::FRONT_AND_BACK) {
            self.state.cull_face = mode;
        } else {
            self.state.error(gl::INVALID_ENUM);
        }
    }
    unsafe fn FrontFace(&mut self, mode: GLenum) {
        if matches!(mode, gl::CW | gl::CCW) {
            self.state.front_face = mode;
        } else {
            self.state.error(gl::INVALID_ENUM);
        }
    }
    unsafe fn ShadeModel(&mut self, mode: GLenum) {
        if matches!(mode, gl::FLAT | gl::SMOOTH) {
            self.state.shade_model = mode;
        } else {
            self.state.error(gl::INVALID_ENUM);
        }
    }
    unsafe fn PolygonOffset(&mut self, factor: GLfloat, units: GLfloat) {
        self.state.polygon_offset = [factor, units];
    }
    unsafe fn PolygonOffsetx(&mut self, factor: GLfixed, units: GLfixed) {
        self.PolygonOffset(factor as f32 / 65536.0, units as f32 / 65536.0);
    }
    unsafe fn DepthRangef(&mut self, near: GLclampf, far: GLclampf) {
        self.state.depth_range = [near.clamp(0.0, 1.0), far.clamp(0.0, 1.0)];
    }
    unsafe fn DepthRangex(&mut self, near: GLclampx, far: GLclampx) {
        self.DepthRangef(near as f32 / 65536.0, far as f32 / 65536.0);
    }
    unsafe fn DepthFunc(&mut self, func: GLenum) {
        self.state.depth_func = func;
    }
    unsafe fn DepthMask(&mut self, flag: GLboolean) {
        self.state.depth_mask = flag != gl::FALSE;
    }
    unsafe fn ColorMask(&mut self, r: GLboolean, g: GLboolean, b: GLboolean, a: GLboolean) {
        self.state.color_mask = [r != 0, g != 0, b != 0, a != 0];
    }
    unsafe fn Viewport(&mut self, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
        self.state.viewport = [x, y, width.max(0), height.max(0)];
    }
    unsafe fn Scissor(&mut self, x: GLint, y: GLint, width: GLsizei, height: GLsizei) {
        self.state.scissor = [x, y, width.max(0), height.max(0)];
    }
    unsafe fn PixelStorei(&mut self, _pname: GLenum, _param: GLint) {}
    unsafe fn Hint(&mut self, _target: GLenum, _mode: GLenum) {}
    unsafe fn LineWidth(&mut self, _value: GLfloat) {}
    unsafe fn LineWidthx(&mut self, value: GLfixed) {
        self.LineWidth(value as f32 / 65536.0);
    }
    unsafe fn PointSize(&mut self, _value: GLfloat) {}
    unsafe fn PointSizex(&mut self, value: GLfixed) {
        self.PointSize(value as f32 / 65536.0);
    }
    unsafe fn Color4f(&mut self, r: GLfloat, g: GLfloat, b: GLfloat, a: GLfloat) {
        self.state.current_color = [r, g, b, a];
    }
    unsafe fn Color4ub(&mut self, r: GLubyte, g: GLubyte, b: GLubyte, a: GLubyte) {
        self.state.current_color = [
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            a as f32 / 255.0,
        ];
    }
    unsafe fn Color4x(&mut self, r: GLfixed, g: GLfixed, b: GLfixed, a: GLfixed) {
        self.Color4f(
            r as f32 / 65536.0,
            g as f32 / 65536.0,
            b as f32 / 65536.0,
            a as f32 / 65536.0,
        );
    }
    unsafe fn Normal3f(&mut self, x: GLfloat, y: GLfloat, z: GLfloat) {
        self.state.current_normal = [x, y, z];
    }
    unsafe fn Normal3x(&mut self, x: GLfixed, y: GLfixed, z: GLfixed) {
        self.Normal3f(x as f32 / 65536.0, y as f32 / 65536.0, z as f32 / 65536.0);
    }
    unsafe fn ColorPointer(
        &mut self,
        size: GLint,
        type_: GLenum,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        self.state.arrays[0] = ArrayState {
            size,
            type_,
            stride,
            pointer,
            buffer: self.state.bound_array_buffer,
            enabled: self.state.arrays[0].enabled,
            normalized: type_ == gl::UNSIGNED_BYTE,
        };
    }
    unsafe fn NormalPointer(&mut self, type_: GLenum, stride: GLsizei, pointer: *const GLvoid) {
        self.state.arrays[1] = ArrayState {
            size: 3,
            type_,
            stride,
            pointer,
            buffer: self.state.bound_array_buffer,
            enabled: self.state.arrays[1].enabled,
            normalized: false,
        };
    }
    unsafe fn TexCoordPointer(
        &mut self,
        size: GLint,
        type_: GLenum,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        self.state.arrays[2] = ArrayState {
            size,
            type_,
            stride,
            pointer,
            buffer: self.state.bound_array_buffer,
            enabled: self.state.arrays[2].enabled,
            normalized: false,
        };
    }
    unsafe fn VertexPointer(
        &mut self,
        size: GLint,
        type_: GLenum,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        self.state.arrays[3] = ArrayState {
            size,
            type_,
            stride,
            pointer,
            buffer: self.state.bound_array_buffer,
            enabled: self.state.arrays[3].enabled,
            normalized: false,
        };
    }
    unsafe fn EnableVertexAttribArray(&mut self, index: GLuint) {
        if let Some(array) = self.state.attributes.get_mut(index as usize) {
            array.enabled = true;
        }
    }
    unsafe fn DisableVertexAttribArray(&mut self, index: GLuint) {
        if let Some(array) = self.state.attributes.get_mut(index as usize) {
            array.enabled = false;
        }
    }
    unsafe fn VertexAttribPointer(
        &mut self,
        index: GLuint,
        size: GLint,
        type_: GLenum,
        normalized: GLboolean,
        stride: GLsizei,
        pointer: *const GLvoid,
    ) {
        if let Some(array) = self.state.attributes.get_mut(index as usize) {
            *array = ArrayState {
                size,
                type_,
                stride,
                pointer,
                buffer: self.state.bound_array_buffer,
                enabled: array.enabled,
                normalized: normalized != 0,
            };
        }
    }
    unsafe fn VertexAttrib4f(
        &mut self,
        index: GLuint,
        x: GLfloat,
        y: GLfloat,
        z: GLfloat,
        w: GLfloat,
    ) {
        if let Some(array) = self.state.attributes.get_mut(index as usize) {
            array.pointer = Box::into_raw(Box::new([x, y, z, w])) as *const c_void;
            array.size = 4;
            array.type_ = gl::FLOAT;
            array.stride = 0;
            array.buffer = 0;
            array.enabled = false;
        }
    }
    unsafe fn DrawArrays(&mut self, mode: GLenum, first: GLint, count: GLsizei) {
        self.draw_arrays(mode, first.max(0) as usize, count.max(0) as usize);
    }
    unsafe fn DrawElements(
        &mut self,
        mode: GLenum,
        count: GLsizei,
        type_: GLenum,
        indices: *const GLvoid,
    ) {
        let mut values = Vec::with_capacity(count.max(0) as usize);
        for i in 0..count.max(0) as usize {
            values.push(self.index_at(indices, i, type_));
        }
        self.draw_indexed(mode, &values);
    }
    unsafe fn Clear(&mut self, mask: GLbitfield) {
        if mask & gl::COLOR_BUFFER_BIT != 0 {
            let c = [
                (self.state.clear_color[0].clamp(0.0, 1.0) * 255.0) as u8,
                (self.state.clear_color[1].clamp(0.0, 1.0) * 255.0) as u8,
                (self.state.clear_color[2].clamp(0.0, 1.0) * 255.0) as u8,
                (self.state.clear_color[3].clamp(0.0, 1.0) * 255.0) as u8,
            ];
            for pixel in self.state.color.chunks_exact_mut(4) {
                pixel.copy_from_slice(&c);
            }
        }
        if mask & gl::DEPTH_BUFFER_BIT != 0 {
            self.state.depth.fill(self.state.clear_depth);
        }
    }
    unsafe fn ClearColor(&mut self, r: GLclampf, g: GLclampf, b: GLclampf, a: GLclampf) {
        self.state.clear_color = [r, g, b, a];
    }
    unsafe fn ClearColorx(&mut self, r: GLclampx, g: GLclampx, b: GLclampx, a: GLclampx) {
        self.ClearColor(
            r as f32 / 65536.0,
            g as f32 / 65536.0,
            b as f32 / 65536.0,
            a as f32 / 65536.0,
        );
    }
    unsafe fn ClearDepthf(&mut self, depth: GLclampf) {
        self.state.clear_depth = depth;
    }
    unsafe fn ClearDepthx(&mut self, depth: GLclampx) {
        self.state.clear_depth = depth as f32 / 65536.0;
    }
    unsafe fn ClearStencil(&mut self, value: GLint) {
        self.state.clear_stencil = value;
    }
    unsafe fn StencilFunc(&mut self, _func: GLenum, _reference: GLint, _mask: GLuint) {}
    unsafe fn StencilOp(&mut self, _sfail: GLenum, _dpfail: GLenum, _dppass: GLenum) {}
    unsafe fn StencilMask(&mut self, _mask: GLuint) {}
    unsafe fn LogicOp(&mut self, _opcode: GLenum) {}
    unsafe fn SampleCoverage(&mut self, _value: GLclampf, _invert: GLboolean) {}
    unsafe fn SampleCoveragex(&mut self, value: GLclampx, invert: GLboolean) {
        self.SampleCoverage(value as f32 / 65536.0, invert);
    }
    unsafe fn ReadPixels(
        &mut self,
        x: GLint,
        y: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
        type_: GLenum,
        pixels: *mut GLvoid,
    ) {
        if pixels.is_null() || type_ != gl::UNSIGNED_BYTE {
            self.state.error(gl::INVALID_VALUE);
            return;
        }
        let channels = if format == gl::RGB {
            3
        } else if format == gl::RGBA {
            4
        } else {
            self.state.error(gl::INVALID_ENUM);
            return;
        };
        for row in 0..height.max(0) as usize {
            for col in 0..width.max(0) as usize {
                let sx = x.max(0) as usize + col;
                let sy = y.max(0) as usize + row;
                let src = if sx < self.state.width && sy < self.state.height {
                    (sy * self.state.width + sx) * 4
                } else {
                    usize::MAX
                };
                let dst = (row * width.max(0) as usize + col) * channels;
                for channel in 0..channels {
                    *(pixels as *mut u8).add(dst + channel) = if src == usize::MAX {
                        0
                    } else {
                        self.state.color[src + channel]
                    };
                }
            }
        }
    }
    unsafe fn GenTextures(&mut self, n: GLsizei, textures: *mut GLuint) {
        if textures.is_null() {
            self.state.error(gl::INVALID_VALUE);
            return;
        }
        for i in 0..n.max(0) as usize {
            let id = self.state.alloc_id();
            self.state.textures.insert(
                id,
                Texture {
                    width: 0,
                    height: 0,
                    pixels: Vec::new(),
                    min_filter: gl::NEAREST_MIPMAP_LINEAR,
                    mag_filter: gl::LINEAR,
                    wrap_s: gl::REPEAT,
                    wrap_t: gl::REPEAT,
                    env_mode: gl::MODULATE,
                },
            );
            *textures.add(i) = id;
        }
    }
    unsafe fn DeleteTextures(&mut self, n: GLsizei, textures: *const GLuint) {
        if !textures.is_null() {
            for i in 0..n.max(0) as usize {
                self.state.textures.remove(&*textures.add(i));
            }
        }
    }
    unsafe fn ActiveTexture(&mut self, texture: GLenum) {
        self.state.active_texture = texture.saturating_sub(gl::TEXTURE0).min(3) as usize;
    }
    unsafe fn BindTexture(&mut self, _target: GLenum, texture: GLuint) {
        if texture != 0 && !self.state.textures.contains_key(&texture) {
            self.state.textures.insert(
                texture,
                Texture {
                    width: 0,
                    height: 0,
                    pixels: Vec::new(),
                    min_filter: gl::NEAREST_MIPMAP_LINEAR,
                    mag_filter: gl::LINEAR,
                    wrap_s: gl::REPEAT,
                    wrap_t: gl::REPEAT,
                    env_mode: gl::MODULATE,
                },
            );
        }
        self.state.bound_texture[self.state.active_texture] = texture;
    }
    unsafe fn TexParameteri(&mut self, _target: GLenum, pname: GLenum, param: GLint) {
        if let Some(texture) = self.state.texture_mut() {
            match pname {
                gl::TEXTURE_MIN_FILTER => texture.min_filter = param as GLenum,
                gl::TEXTURE_MAG_FILTER => texture.mag_filter = param as GLenum,
                gl::TEXTURE_WRAP_S => texture.wrap_s = param as GLenum,
                gl::TEXTURE_WRAP_T => texture.wrap_t = param as GLenum,
                _ => {}
            }
        }
    }
    unsafe fn IsTexture(&mut self, texture: GLuint) -> GLboolean {
        if self.state.textures.contains_key(&texture) {
            gl::TRUE
        } else {
            gl::FALSE
        }
    }
    unsafe fn TexParameterf(&mut self, target: GLenum, pname: GLenum, param: GLfloat) {
        self.TexParameteri(target, pname, param as GLint);
    }
    unsafe fn TexParameterx(&mut self, target: GLenum, pname: GLenum, param: GLfixed) {
        self.TexParameteri(target, pname, param >> 16);
    }
    unsafe fn TexParameteriv(&mut self, target: GLenum, pname: GLenum, params: *const GLint) {
        if !params.is_null() {
            self.TexParameteri(target, pname, *params);
        }
    }
    unsafe fn TexParameterfv(&mut self, target: GLenum, pname: GLenum, params: *const GLfloat) {
        if !params.is_null() {
            self.TexParameterf(target, pname, *params);
        }
    }
    unsafe fn TexParameterxv(&mut self, target: GLenum, pname: GLenum, params: *const GLfixed) {
        if !params.is_null() {
            self.TexParameterx(target, pname, *params);
        }
    }
    unsafe fn TexImage2D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        _internalformat: GLint,
        width: GLsizei,
        height: GLsizei,
        _border: GLint,
        format: GLenum,
        type_: GLenum,
        pixels: *const GLvoid,
    ) {
        if width <= 0 || height <= 0 {
            self.state.error(gl::INVALID_VALUE);
            return;
        }
        let mut output = vec![0; width as usize * height as usize * 4];
        if !pixels.is_null() && type_ == gl::UNSIGNED_BYTE {
            let channels = if format == gl::RGB {
                3
            } else if format == gl::RGBA || format == gl::BGRA_EXT {
                4
            } else {
                0
            };
            if channels == 0 {
                self.state.error(gl::INVALID_ENUM);
                return;
            }
            for i in 0..width as usize * height as usize {
                let source = pixels as *const u8;
                let src = i * channels;
                let dst = i * 4;
                if format == gl::BGRA_EXT {
                    output[dst..dst + 4].copy_from_slice(&[
                        *(source.add(src + 2)),
                        *(source.add(src + 1)),
                        *source.add(src),
                        *source.add(src + 3),
                    ]);
                } else {
                    output[dst..dst + channels]
                        .copy_from_slice(std::slice::from_raw_parts(source.add(src), channels));
                    if channels == 3 {
                        output[dst + 3] = 255;
                    }
                }
            }
        }
        if let Some(texture) = self.state.texture_mut() {
            texture.width = width as usize;
            texture.height = height as usize;
            texture.pixels = output;
        }
    }
    unsafe fn TexSubImage2D(
        &mut self,
        _target: GLenum,
        _level: GLint,
        xoffset: GLint,
        yoffset: GLint,
        width: GLsizei,
        height: GLsizei,
        format: GLenum,
        _type_: GLenum,
        pixels: *const GLvoid,
    ) {
        if pixels.is_null() {
            return;
        }
        let channels = if format == gl::RGB {
            3
        } else if format == gl::RGBA {
            4
        } else {
            self.state.error(gl::INVALID_ENUM);
            return;
        };
        let Some(texture) = self.state.texture_mut() else {
            return;
        };
        for y in 0..height.max(0) as usize {
            for x in 0..width.max(0) as usize {
                let sx = (y * width.max(0) as usize + x) * channels;
                let dx = xoffset.max(0) as usize + x;
                let dy = yoffset.max(0) as usize + y;
                if dx < texture.width && dy < texture.height {
                    let d = (dy * texture.width + dx) * 4;
                    let s = pixels as *const u8;
                    texture.pixels[d..d + channels]
                        .copy_from_slice(std::slice::from_raw_parts(s.add(sx), channels));
                    if channels == 3 {
                        texture.pixels[d + 3] = 255;
                    }
                }
            }
        }
    }
    unsafe fn TexEnvf(&mut self, _target: GLenum, pname: GLenum, param: GLfloat) {
        if pname == gl::TEXTURE_ENV_MODE {
            if let Some(texture) = self.state.texture_mut() {
                texture.env_mode = param as GLenum;
            }
        }
    }
    unsafe fn TexEnvi(&mut self, target: GLenum, pname: GLenum, param: GLint) {
        self.TexEnvf(target, pname, param as f32);
    }
    unsafe fn TexEnvx(&mut self, target: GLenum, pname: GLenum, param: GLfixed) {
        self.TexEnvf(target, pname, param as f32 / 65536.0);
    }
    unsafe fn TexEnvfv(&mut self, target: GLenum, pname: GLenum, params: *const GLfloat) {
        if !params.is_null() {
            self.TexEnvf(target, pname, *params);
        }
    }
    unsafe fn TexEnvxv(&mut self, target: GLenum, pname: GLenum, params: *const GLfixed) {
        if !params.is_null() {
            self.TexEnvx(target, pname, *params);
        }
    }
    unsafe fn TexEnviv(&mut self, target: GLenum, pname: GLenum, params: *const GLint) {
        if !params.is_null() {
            self.TexEnvi(target, pname, *params);
        }
    }
    unsafe fn MultiTexCoord4f(
        &mut self,
        target: GLenum,
        s: GLfloat,
        t: GLfloat,
        r: GLfloat,
        q: GLfloat,
    ) {
        let unit = target.saturating_sub(gl::TEXTURE0).min(3) as usize;
        self.state.current_texcoord[unit] = [s, t, r, q];
    }
    unsafe fn MultiTexCoord4x(
        &mut self,
        target: GLenum,
        s: GLfixed,
        t: GLfixed,
        r: GLfixed,
        q: GLfixed,
    ) {
        self.MultiTexCoord4f(
            target,
            s as f32 / 65536.0,
            t as f32 / 65536.0,
            r as f32 / 65536.0,
            q as f32 / 65536.0,
        );
    }
    unsafe fn MatrixMode(&mut self, mode: GLenum) {
        self.state.matrix_mode = mode;
    }
    unsafe fn LoadIdentity(&mut self) {
        *self.state.matrix_mut() = IDENTITY;
    }
    unsafe fn LoadMatrixf(&mut self, matrix: *const GLfloat) {
        if matrix.is_null() {
            self.state.error(gl::INVALID_VALUE);
        } else {
            self.state
                .matrix_mut()
                .copy_from_slice(std::slice::from_raw_parts(matrix, 16));
        }
    }
    unsafe fn LoadMatrixx(&mut self, matrix: *const GLfixed) {
        if matrix.is_null() {
            self.state.error(gl::INVALID_VALUE);
        } else {
            for (i, value) in std::slice::from_raw_parts(matrix, 16).iter().enumerate() {
                self.state.matrix_mut()[i] = *value as f32 / 65536.0;
            }
        }
    }
    unsafe fn MultMatrixf(&mut self, matrix: *const GLfloat) {
        if !matrix.is_null() {
            let value = std::slice::from_raw_parts(matrix, 16);
            *self.state.matrix_mut() = mul(self.state.matrix(), value.try_into().unwrap());
        }
    }
    unsafe fn MultMatrixx(&mut self, matrix: *const GLfixed) {
        if !matrix.is_null() {
            let mut value = IDENTITY;
            for (i, item) in std::slice::from_raw_parts(matrix, 16).iter().enumerate() {
                value[i] = *item as f32 / 65536.0;
            }
            *self.state.matrix_mut() = mul(self.state.matrix(), value);
        }
    }
    unsafe fn PushMatrix(&mut self) {
        self.state.push_matrix();
    }
    unsafe fn PopMatrix(&mut self) {
        self.state.pop_matrix();
    }
    unsafe fn Orthof(
        &mut self,
        left: GLfloat,
        right: GLfloat,
        bottom: GLfloat,
        top: GLfloat,
        near: GLfloat,
        far: GLfloat,
    ) {
        let mut m = [0.0; 16];
        m[0] = 2.0 / (right - left);
        m[5] = 2.0 / (top - bottom);
        m[10] = -2.0 / (far - near);
        m[12] = -(right + left) / (right - left);
        m[13] = -(top + bottom) / (top - bottom);
        m[14] = -(far + near) / (far - near);
        m[15] = 1.0;
        *self.state.matrix_mut() = mul(self.state.matrix(), m);
    }
    unsafe fn Orthox(
        &mut self,
        l: GLfixed,
        r: GLfixed,
        b: GLfixed,
        t: GLfixed,
        n: GLfixed,
        f: GLfixed,
    ) {
        self.Orthof(
            l as f32 / 65536.0,
            r as f32 / 65536.0,
            b as f32 / 65536.0,
            t as f32 / 65536.0,
            n as f32 / 65536.0,
            f as f32 / 65536.0,
        );
    }
    unsafe fn Frustumf(
        &mut self,
        l: GLfloat,
        r: GLfloat,
        b: GLfloat,
        t: GLfloat,
        n: GLfloat,
        f: GLfloat,
    ) {
        let mut m = [0.0; 16];
        m[0] = 2.0 * n / (r - l);
        m[5] = 2.0 * n / (t - b);
        m[8] = (r + l) / (r - l);
        m[9] = (t + b) / (t - b);
        m[10] = -(f + n) / (f - n);
        m[11] = -1.0;
        m[14] = -(2.0 * f * n) / (f - n);
        *self.state.matrix_mut() = mul(self.state.matrix(), m);
    }
    unsafe fn Frustumx(
        &mut self,
        l: GLfixed,
        r: GLfixed,
        b: GLfixed,
        t: GLfixed,
        n: GLfixed,
        f: GLfixed,
    ) {
        self.Frustumf(
            l as f32 / 65536.0,
            r as f32 / 65536.0,
            b as f32 / 65536.0,
            t as f32 / 65536.0,
            n as f32 / 65536.0,
            f as f32 / 65536.0,
        );
    }
    unsafe fn Translatef(&mut self, x: GLfloat, y: GLfloat, z: GLfloat) {
        let mut m = IDENTITY;
        m[12] = x;
        m[13] = y;
        m[14] = z;
        *self.state.matrix_mut() = mul(self.state.matrix(), m);
    }
    unsafe fn Translatex(&mut self, x: GLfixed, y: GLfixed, z: GLfixed) {
        self.Translatef(x as f32 / 65536.0, y as f32 / 65536.0, z as f32 / 65536.0);
    }
    unsafe fn Scalef(&mut self, x: GLfloat, y: GLfloat, z: GLfloat) {
        let mut m = IDENTITY;
        m[0] = x;
        m[5] = y;
        m[10] = z;
        *self.state.matrix_mut() = mul(self.state.matrix(), m);
    }
    unsafe fn Scalex(&mut self, x: GLfixed, y: GLfixed, z: GLfixed) {
        self.Scalef(x as f32 / 65536.0, y as f32 / 65536.0, z as f32 / 65536.0);
    }
    unsafe fn Rotatef(&mut self, angle: GLfloat, x: GLfloat, y: GLfloat, z: GLfloat) {
        let radians = angle.to_radians();
        let c = radians.cos();
        let s = radians.sin();
        let one = 1.0 - c;
        let mut m = IDENTITY;
        m[0] = x * x * one + c;
        m[1] = y * x * one + z * s;
        m[2] = x * z * one - y * s;
        m[4] = x * y * one - z * s;
        m[5] = y * y * one + c;
        m[6] = y * z * one + x * s;
        m[8] = x * z * one + y * s;
        m[9] = y * z * one - x * s;
        m[10] = z * z * one + c;
        *self.state.matrix_mut() = mul(self.state.matrix(), m);
    }
    unsafe fn Rotatex(&mut self, angle: GLfixed, x: GLfixed, y: GLfixed, z: GLfixed) {
        self.Rotatef(
            angle as f32 / 65536.0,
            x as f32 / 65536.0,
            y as f32 / 65536.0,
            z as f32 / 65536.0,
        );
    }
    unsafe fn GenBuffers(&mut self, n: GLsizei, buffers: *mut GLuint) {
        if !buffers.is_null() {
            for i in 0..n.max(0) as usize {
                let id = self.state.alloc_id();
                self.state.buffers.insert(id, Vec::new());
                *buffers.add(i) = id;
            }
        }
    }
    unsafe fn DeleteBuffers(&mut self, n: GLsizei, buffers: *const GLuint) {
        if !buffers.is_null() {
            for i in 0..n.max(0) as usize {
                self.state.buffers.remove(&*buffers.add(i));
            }
        }
    }
    unsafe fn BindBuffer(&mut self, target: GLenum, buffer: GLuint) {
        if target == gl::ARRAY_BUFFER {
            self.state.bound_array_buffer = buffer;
        } else if target == gl::ELEMENT_ARRAY_BUFFER {
            self.state.bound_element_buffer = buffer;
        }
    }
    unsafe fn BufferData(
        &mut self,
        target: GLenum,
        size: GLsizeiptr,
        data: *const GLvoid,
        _usage: GLenum,
    ) {
        if size < 0 {
            self.state.error(gl::INVALID_VALUE);
            return;
        }
        let output = if data.is_null() {
            vec![0; size as usize]
        } else {
            std::slice::from_raw_parts(data as *const u8, size as usize).to_vec()
        };
        let id = if target == gl::ARRAY_BUFFER {
            self.state.bound_array_buffer
        } else {
            self.state.bound_element_buffer
        };
        if id != 0 {
            self.state.buffers.insert(id, output);
        }
    }
    unsafe fn BufferSubData(
        &mut self,
        target: GLenum,
        offset: GLintptr,
        size: GLsizeiptr,
        data: *const GLvoid,
    ) {
        let id = if target == gl::ARRAY_BUFFER {
            self.state.bound_array_buffer
        } else {
            self.state.bound_element_buffer
        };
        if id == 0 || offset < 0 || size < 0 || data.is_null() {
            self.state.error(gl::INVALID_VALUE);
            return;
        }
        if let Some(buffer) = self.state.buffers.get_mut(&id) {
            let end = offset as usize + size as usize;
            if end <= buffer.len() {
                buffer[offset as usize..end]
                    .copy_from_slice(std::slice::from_raw_parts(data as *const u8, size as usize));
            } else {
                self.state.error(gl::INVALID_VALUE);
            }
        }
    }
    unsafe fn GenFramebuffersOES(&mut self, n: GLsizei, framebuffers: *mut GLuint) {
        if !framebuffers.is_null() {
            for i in 0..n.max(0) as usize {
                let id = self.state.alloc_id();
                self.state.framebuffers.insert(id);
                *framebuffers.add(i) = id;
            }
        }
    }
    unsafe fn DeleteFramebuffersOES(&mut self, n: GLsizei, framebuffers: *const GLuint) {
        if !framebuffers.is_null() {
            for i in 0..n.max(0) as usize {
                self.state.framebuffers.remove(&*framebuffers.add(i));
            }
        }
    }
    unsafe fn BindFramebufferOES(&mut self, _target: GLenum, framebuffer: GLuint) {
        self.state.bound_framebuffer = framebuffer;
    }
    unsafe fn CheckFramebufferStatusOES(&mut self, _target: GLenum) -> GLenum {
        gl::FRAMEBUFFER_COMPLETE_OES
    }
    unsafe fn GenRenderbuffersOES(&mut self, n: GLsizei, renderbuffers: *mut GLuint) {
        if !renderbuffers.is_null() {
            for i in 0..n.max(0) as usize {
                let id = self.state.alloc_id();
                self.state.renderbuffers.insert(id);
                *renderbuffers.add(i) = id;
            }
        }
    }
    unsafe fn DeleteRenderbuffersOES(&mut self, n: GLsizei, renderbuffers: *const GLuint) {
        if !renderbuffers.is_null() {
            for i in 0..n.max(0) as usize {
                self.state.renderbuffers.remove(&*renderbuffers.add(i));
            }
        }
    }
    unsafe fn BindRenderbufferOES(&mut self, _target: GLenum, renderbuffer: GLuint) {
        self.state.bound_renderbuffer = renderbuffer;
    }
    unsafe fn RenderbufferStorageOES(
        &mut self,
        _target: GLenum,
        _internalformat: GLenum,
        width: GLsizei,
        height: GLsizei,
    ) {
        if width <= 0 || height <= 0 {
            self.state.error(gl::INVALID_VALUE);
            return;
        }
        self.state.width = width as usize;
        self.state.height = height as usize;
        self.state
            .color
            .resize(self.state.width * self.state.height * 4, 0);
        self.state
            .depth
            .resize(self.state.width * self.state.height, 1.0);
        self.state.viewport = [0, 0, width, height];
    }
    unsafe fn FramebufferRenderbufferOES(
        &mut self,
        _target: GLenum,
        _attachment: GLenum,
        _renderbuffertarget: GLenum,
        renderbuffer: GLuint,
    ) {
        self.state.bound_renderbuffer = renderbuffer;
    }
    unsafe fn GetRenderbufferParameterivOES(
        &mut self,
        _target: GLenum,
        pname: GLenum,
        params: *mut GLint,
    ) {
        if !params.is_null() {
            *params = match pname {
                gl::RENDERBUFFER_WIDTH_OES => self.state.width as GLint,
                gl::RENDERBUFFER_HEIGHT_OES => self.state.height as GLint,
                _ => 0,
            };
        }
    }
    unsafe fn GenerateMipmapOES(&mut self, _target: GLenum) {}
    unsafe fn CreateShader(&mut self, _type_: GLenum) -> GLuint {
        let id = self.state.alloc_id();
        self.state.shader_ids.insert(id);
        id
    }
    unsafe fn DeleteShader(&mut self, shader: GLuint) {
        self.state.shader_ids.remove(&shader);
    }
    unsafe fn ShaderSource(
        &mut self,
        _shader: GLuint,
        _count: GLsizei,
        _string: *const *const GLchar,
        _length: *const GLint,
    ) {
    }
    unsafe fn CompileShader(&mut self, _shader: GLuint) {}
    unsafe fn GetShaderiv(&mut self, _shader: GLuint, pname: GLenum, params: *mut GLint) {
        if !params.is_null() {
            *params = if pname == GL_COMPILE_STATUS {
                gl::TRUE as GLint
            } else if pname == GL_INFO_LOG_LENGTH {
                1
            } else {
                0
            };
        }
    }
    unsafe fn GetShaderInfoLog(
        &mut self,
        _shader: GLuint,
        _max: GLsizei,
        length: *mut GLsizei,
        info: *mut GLchar,
    ) {
        if !length.is_null() {
            *length = 0;
        }
        if !info.is_null() {
            *info = 0;
        }
    }
    unsafe fn IsShader(&mut self, shader: GLuint) -> GLboolean {
        if self.state.shader_ids.contains(&shader) {
            gl::TRUE
        } else {
            gl::FALSE
        }
    }
    unsafe fn CreateProgram(&mut self) -> GLuint {
        let id = self.state.alloc_id();
        self.state.program_ids.insert(id);
        id
    }
    unsafe fn DeleteProgram(&mut self, program: GLuint) {
        self.state.program_ids.remove(&program);
    }
    unsafe fn AttachShader(&mut self, _program: GLuint, _shader: GLuint) {}
    unsafe fn LinkProgram(&mut self, _program: GLuint) {}
    unsafe fn UseProgram(&mut self, program: GLuint) {
        self.state.current_program = program;
    }
    unsafe fn GetProgramiv(&mut self, _program: GLuint, pname: GLenum, params: *mut GLint) {
        if !params.is_null() {
            *params = if pname == GL_LINK_STATUS || pname == GL_VALIDATE_STATUS {
                gl::TRUE as GLint
            } else {
                0
            };
        }
    }
    unsafe fn GetProgramInfoLog(
        &mut self,
        _program: GLuint,
        _max: GLsizei,
        length: *mut GLsizei,
        info: *mut GLchar,
    ) {
        if !length.is_null() {
            *length = 0;
        }
        if !info.is_null() {
            *info = 0;
        }
    }
    unsafe fn IsProgram(&mut self, program: GLuint) -> GLboolean {
        if self.state.program_ids.contains(&program) {
            gl::TRUE
        } else {
            gl::FALSE
        }
    }
    unsafe fn ValidateProgram(&mut self, _program: GLuint) {}
    unsafe fn BindAttribLocation(
        &mut self,
        _program: GLuint,
        _index: GLuint,
        _name: *const GLchar,
    ) {
    }
    unsafe fn GetAttribLocation(&mut self, _program: GLuint, name: *const GLchar) -> GLint {
        if name.is_null() {
            return -1;
        }
        match CStr::from_ptr(name).to_bytes() {
            b"position" | b"a_position" | b"inPosition" => 0,
            b"color" | b"a_color" | b"inColor" => 1,
            b"texcoord" | b"texCoord" | b"a_texCoord" | b"a_texcoord" => 2,
            _ => 0,
        }
    }
    unsafe fn GetUniformLocation(&mut self, _program: GLuint, name: *const GLchar) -> GLint {
        if name.is_null() {
            -1
        } else {
            CStr::from_ptr(name)
                .to_bytes()
                .iter()
                .fold(1i32, |hash, byte| {
                    hash.wrapping_mul(31).wrapping_add(*byte as i32)
                })
        }
    }
    unsafe fn Uniform1f(&mut self, _location: GLint, _v0: GLfloat) {}
    unsafe fn Uniform1i(&mut self, _location: GLint, _v0: GLint) {}
    unsafe fn UniformMatrix4fv(
        &mut self,
        _location: GLint,
        _count: GLsizei,
        _transpose: GLboolean,
        _value: *const GLfloat,
    ) {
    }
    fn is_software(&self) -> bool {
        true
    }
    fn software_frame(&self) -> Option<(Vec<u8>, u32, u32)> {
        Some((
            self.state.color.clone(),
            self.state.width as u32,
            self.state.height as u32,
        ))
    }
}

impl SoftwareGLES<'_> {
    fn draw_arrays(&mut self, mode: GLenum, first: usize, count: usize) {
        match mode {
            gl::TRIANGLES => {
                for i in (0..count.saturating_sub(2)).step_by(3) {
                    self.draw_triangle(
                        self.vertex(first + i),
                        self.vertex(first + i + 1),
                        self.vertex(first + i + 2),
                    );
                }
            }
            gl::TRIANGLE_STRIP => {
                for i in 0..count.saturating_sub(2) {
                    let a = self.vertex(first + i);
                    let b = self.vertex(first + i + 1);
                    let c = self.vertex(first + i + 2);
                    if i % 2 == 0 {
                        self.draw_triangle(a, b, c);
                    } else {
                        self.draw_triangle(b, a, c);
                    }
                }
            }
            gl::TRIANGLE_FAN => {
                for i in 1..count.saturating_sub(1) {
                    self.draw_triangle(
                        self.vertex(first),
                        self.vertex(first + i),
                        self.vertex(first + i + 1),
                    );
                }
            }
            gl::POINTS => {
                for i in 0..count {
                    self.draw_point(self.vertex(first + i));
                }
            }
            _ => {}
        }
    }

    fn draw_indexed(&mut self, mode: GLenum, indices: &[usize]) {
        match mode {
            gl::TRIANGLES => {
                for i in (0..indices.len().saturating_sub(2)).step_by(3) {
                    self.draw_triangle(
                        self.vertex(indices[i]),
                        self.vertex(indices[i + 1]),
                        self.vertex(indices[i + 2]),
                    );
                }
            }
            gl::TRIANGLE_STRIP => {
                for i in 0..indices.len().saturating_sub(2) {
                    let a = self.vertex(indices[i]);
                    let b = self.vertex(indices[i + 1]);
                    let c = self.vertex(indices[i + 2]);
                    if i % 2 == 0 {
                        self.draw_triangle(a, b, c);
                    } else {
                        self.draw_triangle(b, a, c);
                    }
                }
            }
            gl::TRIANGLE_FAN => {
                for i in 1..indices.len().saturating_sub(1) {
                    self.draw_triangle(
                        self.vertex(indices[0]),
                        self.vertex(indices[i]),
                        self.vertex(indices[i + 1]),
                    );
                }
            }
            gl::POINTS => {
                for &index in indices {
                    self.draw_point(self.vertex(index));
                }
            }
            _ => {}
        }
    }

    fn draw_point(&mut self, vertex: Vertex) {
        let projected = project(vertex, self.state.viewport);
        let color = projected.color;
        let x = projected.x.round() as i32;
        let y = projected.y.round() as i32;
        if x < 0 || y < 0 || x >= self.state.width as i32 || y >= self.state.height as i32 {
            return;
        }
        let p = (y as usize * self.state.width + x as usize) * 4;
        for i in 0..4 {
            self.state.color[p + i] = (color[i].clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
}

#[derive(Clone, Copy)]
struct Projected {
    x: f32,
    y: f32,
    z: f32,
    u: f32,
    v: f32,
    color: [f32; 4],
}

fn project(vertex: Vertex, viewport: [GLint; 4]) -> Projected {
    let w = if vertex.position[3].abs() < f32::EPSILON {
        1.0
    } else {
        vertex.position[3]
    };
    let x = vertex.position[0] / w;
    let y = vertex.position[1] / w;
    let z = vertex.position[2] / w;
    Projected {
        x: (x * 0.5 + 0.5) * viewport[2] as f32 + viewport[0] as f32,
        y: (y * 0.5 + 0.5) * viewport[3] as f32 + viewport[1] as f32,
        z: z * 0.5 + 0.5,
        u: vertex.texcoord[0],
        v: vertex.texcoord[1],
        color: vertex.color,
    }
}

fn mul(a: [f32; 16], b: [f32; 16]) -> [f32; 16] {
    let mut out = [0.0; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = (0..4).map(|i| a[i * 4 + row] * b[col * 4 + i]).sum();
        }
    }
    out
}

fn mul_vec(m: [f32; 16], v: [f32; 4]) -> [f32; 4] {
    [
        m[0] * v[0] + m[4] * v[1] + m[8] * v[2] + m[12] * v[3],
        m[1] * v[0] + m[5] * v[1] + m[9] * v[2] + m[13] * v[3],
        m[2] * v[0] + m[6] * v[1] + m[10] * v[2] + m[14] * v[3],
        m[3] * v[0] + m[7] * v[1] + m[11] * v[2] + m[15] * v[3],
    ]
}

fn edge(ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32) -> f32 {
    (cx - ax) * (by - ay) - (cy - ay) * (bx - ax)
}

fn lerp3(a: [f32; 4], b: [f32; 4], c: [f32; 4], wa: f32, wb: f32, wc: f32) -> [f32; 4] {
    [
        a[0] * wa + b[0] * wb + c[0] * wc,
        a[1] * wa + b[1] * wb + c[1] * wc,
        a[2] * wa + b[2] * wb + c[2] * wc,
        a[3] * wa + b[3] * wb + c[3] * wc,
    ]
}

fn depth_pass(func: GLenum, source: f32, destination: f32) -> bool {
    match func {
        gl::NEVER => false,
        gl::ALWAYS => true,
        gl::EQUAL => (source - destination).abs() < 0.0001,
        gl::LEQUAL => source <= destination,
        gl::GREATER => source > destination,
        gl::GEQUAL => source >= destination,
        gl::NOTEQUAL => (source - destination).abs() >= 0.0001,
        _ => source < destination,
    }
}

fn factor(kind: GLenum, source: [f32; 4], destination: [f32; 4]) -> [f32; 4] {
    match kind {
        gl::ZERO => [0.0; 4],
        gl::ONE => [1.0; 4],
        gl::SRC_ALPHA => [source[3]; 4],
        gl::ONE_MINUS_SRC_ALPHA => [1.0 - source[3]; 4],
        gl::DST_ALPHA => [destination[3]; 4],
        gl::ONE_MINUS_DST_ALPHA => [1.0 - destination[3]; 4],
        gl::SRC_COLOR => source,
        gl::ONE_MINUS_SRC_COLOR => [
            1.0 - source[0],
            1.0 - source[1],
            1.0 - source[2],
            1.0 - source[3],
        ],
        gl::DST_COLOR => destination,
        gl::ONE_MINUS_DST_COLOR => [
            1.0 - destination[0],
            1.0 - destination[1],
            1.0 - destination[2],
            1.0 - destination[3],
        ],
        _ => [1.0; 4],
    }
}

fn blend_equation(
    source: [f32; 4],
    destination: [f32; 4],
    src_factor: GLenum,
    dst_factor: GLenum,
    equation: GLenum,
) -> [f32; 4] {
    let s = factor(src_factor, source, destination);
    let d = factor(dst_factor, source, destination);
    let add = [
        source[0] * s[0] + destination[0] * d[0],
        source[1] * s[1] + destination[1] * d[1],
        source[2] * s[2] + destination[2] * d[2],
        source[3] * s[3] + destination[3] * d[3],
    ];
    match equation {
        gl::FUNC_SUBTRACT_OES => [
            source[0] * s[0] - destination[0] * d[0],
            source[1] * s[1] - destination[1] * d[1],
            source[2] * s[2] - destination[2] * d[2],
            source[3] * s[3] - destination[3] * d[3],
        ],
        gl::FUNC_REVERSE_SUBTRACT_OES => [
            destination[0] * d[0] - source[0] * s[0],
            destination[1] * d[1] - source[1] * s[1],
            destination[2] * d[2] - source[2] * s[2],
            destination[3] * d[3] - source[3] * s[3],
        ],
        _ => add,
    }
}

fn alpha_pass(func: GLenum, source: f32, reference: f32) -> bool {
    match func {
        gl::NEVER => false,
        gl::LESS => source < reference,
        gl::EQUAL => (source - reference).abs() < 0.0001,
        gl::LEQUAL => source <= reference,
        gl::GREATER => source > reference,
        gl::NOTEQUAL => (source - reference).abs() >= 0.0001,
        gl::GEQUAL => source >= reference,
        _ => true,
    }
}

fn wrap(value: f32, mode: GLenum) -> f32 {
    match mode {
        gl::CLAMP_TO_EDGE => value.clamp(0.0, 1.0),
        _ => value.rem_euclid(1.0),
    }
}

fn array_index(array: GLenum) -> Option<usize> {
    match array {
        gl::COLOR_ARRAY => Some(0),
        gl::NORMAL_ARRAY => Some(1),
        gl::TEXTURE_COORD_ARRAY => Some(2),
        gl::VERTEX_ARRAY => Some(3),
        _ => None,
    }
}
