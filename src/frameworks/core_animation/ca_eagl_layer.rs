/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `CAEAGLLayer`.

use super::ca_layer::CALayerHostObject;
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect};
use crate::frameworks::foundation::ns_string;
use crate::objc::{id, msg, msg_class, nil, objc_classes, release, Class, ClassExports};
use crate::Environment;

// MARK: - EAGLDrawable property key constants
//
// These are the keys apps put in the drawableProperties dictionary.
// We export them as static strings so other modules can reference them.
pub const kEAGLDrawablePropertyRetainedBacking: &str = "kEAGLDrawablePropertyRetainedBacking";
pub const kEAGLDrawablePropertyColorFormat: &str = "kEAGLDrawablePropertyColorFormat";

// kEAGLColorFormat values
pub const kEAGLColorFormatRGBA8: &str = "kEAGLColorFormatRGBA8";
pub const kEAGLColorFormatRGB565: &str = "kEAGLColorFormatRGB565";
pub const kEAGLColorFormatSRGBA8: &str = "kEAGLColorFormatSRGBA8";

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation CAEAGLLayer: CALayer

// =========================================================================
// MARK: - EAGLDrawable protocol
// =========================================================================

- (id)drawableProperties { // NSDictionary<NSString*, id>*
    env.objc.borrow::<CALayerHostObject>(this).drawable_properties
}

- (())setDrawableProperties:(id)props { // NSDictionary<NSString*, id>*
    // Store a copy — matches Apple's behaviour.
    let old = env.objc.borrow::<CALayerHostObject>(this).drawable_properties;
    let new_props: id = if props != nil { msg![env; props copy] } else { nil };
    release(env, old);
    env.objc.borrow_mut::<CALayerHostObject>(this).drawable_properties = new_props;

    // Log the properties the app is requesting so rendering issues are
    // easier to diagnose.
    if new_props != nil {
        let retained_key = ns_string::get_static_str(env, kEAGLDrawablePropertyRetainedBacking);
        let format_key    = ns_string::get_static_str(env, kEAGLDrawablePropertyColorFormat);

        let retained_val: id = msg![env; new_props objectForKey:retained_key];
        let format_val:   id = msg![env; new_props objectForKey:format_key];

        let retained_str = if retained_val != nil {
            let b: bool = msg![env; retained_val boolValue];
            if b { "YES" } else { "NO" }
        } else {
            "(not set)"
        };

        let format_str = if format_val != nil {
            ns_string::to_rust_string(env, format_val).into_owned()
        } else {
            "(not set)".to_string()
        };

        log_dbg!(
            "CAEAGLLayer setDrawableProperties: retainedBacking={} colorFormat={}",
            retained_str, format_str
        );
    }
}

// =========================================================================
// MARK: - Convenience helpers (read individual drawable properties)
// =========================================================================

// Returns YES if the backing store should be retained after presentation.
// Corresponds to kEAGLDrawablePropertyRetainedBacking.
- (bool)_touchHLE_retainedBacking {
    let props = env.objc.borrow::<CALayerHostObject>(this).drawable_properties;
    if props == nil { return false; }
    let key: id = ns_string::get_static_str(env, kEAGLDrawablePropertyRetainedBacking);
    let val: id = msg![env; props objectForKey:key];
    if val == nil { return false; }
    msg![env; val boolValue]
}

// Returns the requested color format string, or "kEAGLColorFormatRGBA8"
// as the default if not specified.
- (id)_touchHLE_colorFormat { // NSString*
    let props = env.objc.borrow::<CALayerHostObject>(this).drawable_properties;
    if props != nil {
        let key: id = ns_string::get_static_str(env, kEAGLDrawablePropertyColorFormat);
        let val: id = msg![env; props objectForKey:key];
        if val != nil { return val; }
    }
    ns_string::get_static_str(env, kEAGLColorFormatRGBA8)
}

// =========================================================================
// MARK: - CALayer overrides
// =========================================================================

// CAEAGLLayer is always opaque by default (unlike plain CALayer).
- (id)init {
    let _: () = msg![env; this setOpaque:true];
    this
}

// MARK: - Scale / Retina Support

- (CGFloat)contentScaleFactor {
    // Жестко задаем масштаб 1.0 (стандартный не-Retina экран)
    1.0
}

- (())setContentScaleFactor:(CGFloat)scale {
    // Заглушка, чтобы игра не упала, если попытается сама установить масштаб
    log_dbg!("CAEAGLLAYER setContentScaleFactor: {} (stubbed)", scale);
}

- (CGFloat)contentsScale {
    // Жестко задаем масштаб 1.0 (стандартный не-Retina экран)
    1.0
}

- (())setContentsScale:(CGFloat)scale {
    // Заглушка, чтобы игра не упала, если попытается сама установить масштаб
    log_dbg!("CAEAGLLAYER setContentsScale: {} (stubbed)", scale);
}

- (id)initWithLayer:(id)layer {
    let _: () = msg![env; this setOpaque:true];
    // Copy drawable properties from the source layer if it is also a
    // CAEAGLLayer.
    let ca_eagl_class: Class = msg_class![env; CAEAGLLayer class];
    let is_eagl: bool = msg![env; layer isKindOfClass:ca_eagl_class];
    if is_eagl {
        let src_props = env.objc.borrow::<CALayerHostObject>(layer).drawable_properties;
        if src_props != nil {
            let copy: id = msg![env; src_props copy];
            env.objc.borrow_mut::<CALayerHostObject>(this).drawable_properties = copy;
        }
    }
    this
}

// Prevent the layer from being drawn by Core Animation — its contents are
// managed exclusively by EAGL/OpenGL ES.
- (())display {
    // No-op: the layer is presented via EAGLContext presentRenderBuffer:.
}

- (())drawInContext:(id)_ctx { // CGContextRef
    // No-op: CAEAGLLayer content comes from OpenGL ES, not Core Graphics.
}

// =========================================================================
// MARK: - Description
// =========================================================================

- (id)description {
    let host = env.objc.borrow::<CALayerHostObject>(this);
    let opaque = host.opaque;
    let has_props = host.drawable_properties != nil;
    let s = format!(
        "<CAEAGLLayer: {:?}; opaque={}; drawableProperties={}>",
        this,
        opaque,
        if has_props { "(set)" } else { "(nil)" }
    );
    let cstr = env.mem.alloc_and_write_cstr(s.as_bytes());
    msg_class![env; NSString stringWithUTF8String:cstr]
}

@end

};

// =========================================================================
// MARK: - find_fullscreen_eagl_layer
// =========================================================================

/// Finds the largest visible, opaque `CAEAGLLayer` that exactly covers the screen.
/// Otherwise, returns [nil] so the compositor can use its general path.
pub fn find_fullscreen_eagl_layer(env: &mut Environment) -> id {
    fn find_in_subtree(
        env: &mut Environment,
        layer: id,
        ca_eagl_layer_class: Class,
        screen_bounds: CGRect,
        depth: usize,
    ) -> Option<(id, CGFloat)> {
        const MAX_LAYER_DEPTH: usize = 128;
        if layer == nil || depth >= MAX_LAYER_DEPTH {
            return None;
        }

        let (
            hidden,
            opacity,
            transform_is_identity,
            opaque,
            bounds,
            anchor_point,
            position,
            sublayer_count,
        ) = {
            let host = env.objc.borrow::<CALayerHostObject>(layer);
            (
                host.hidden,
                host.opacity,
                host.affine_transform.is_identity(),
                host.opaque,
                host.bounds,
                host.anchor_point,
                host.position,
                host.sublayers.len(),
            )
        };
        if hidden || opacity <= 0.0 {
            return None;
        }

        let geometry_matches_screen = bounds.size == screen_bounds.size
            && bounds.origin == (CGPoint { x: 0.0, y: 0.0 })
            && anchor_point == (CGPoint { x: 0.5, y: 0.5 })
            && position
                == (CGPoint {
                    x: screen_bounds.size.width / 2.0,
                    y: screen_bounds.size.height / 2.0,
                })
            && bounds.size.width.is_finite()
            && bounds.size.height.is_finite();
        if opacity != 1.0 || !transform_is_identity || !geometry_matches_screen {
            return None;
        }

        let mut best = None;
        for index in (0..sublayer_count).rev() {
            let sublayer = env.objc.borrow::<CALayerHostObject>(layer).sublayers[index];
            if let Some(candidate) =
                find_in_subtree(env, sublayer, ca_eagl_layer_class, screen_bounds, depth + 1)
            {
                if best.map_or(true, |(_, best_area)| candidate.1 > best_area) {
                    best = Some(candidate);
                }
            }
        }

        if opaque && msg![env; layer isKindOfClass:ca_eagl_layer_class] {
            let area = bounds.size.width * bounds.size.height;
            if best.map_or(true, |(_, best_area)| area > best_area) {
                best = Some((layer, area));
            }
        }
        best
    }

    if env.options.force_composition {
        return nil;
    }

    let window_count = env.framework_state.uikit.ui_view.ui_window.windows.len();
    let mut top_window = nil;
    for index in (0..window_count).rev() {
        let window = env.framework_state.uikit.ui_view.ui_window.windows[index];
        if !msg![env; window isHidden] {
            top_window = window;
            break;
        }
    }
    if top_window == nil {
        return nil;
    }

    let root_layer: id = msg![env; top_window layer];
    if root_layer == nil {
        return nil;
    }

    let screen_bounds: CGRect = {
        let screen: id = msg_class![env; UIScreen mainScreen];
        msg![env; screen bounds]
    };
    let ca_eagl_layer_class: Class = msg_class![env; CAEAGLLayer class];
    find_in_subtree(env, root_layer, ca_eagl_layer_class, screen_bounds, 0)
        .map_or(nil, |(layer, _)| layer)
}

// =========================================================================
// MARK: - Pixel buffer helpers (used by EAGLContext)
// =========================================================================

/// Takes the pixel buffer out of the layer so it can be refilled by
/// `EAGLContext presentRenderBuffer:`. Pass the buffer back via
/// [present_pixels] once it is filled.
pub fn get_pixels_vec_for_presenting(env: &mut Environment, layer: id) -> Vec<u8> {
    env.objc
        .borrow_mut::<CALayerHostObject>(layer)
        .presented_pixels
        .take()
        .map(|(vec, _w, _h)| vec)
        .unwrap_or_default()
}

/// Stores the new rendered frame in the layer and marks the GLES texture as
/// stale. Data must be in RGBA8 format.
pub fn present_pixels(env: &mut Environment, layer: id, pixels: Vec<u8>, width: u32, height: u32) {
    let host_obj = env.objc.borrow_mut::<CALayerHostObject>(layer);
    host_obj.presented_pixels = Some((pixels, width, height));
    host_obj.gles_texture_is_up_to_date = false;
}

/// Returns whether the layer's backing store should be retained after
/// presentation (i.e. `kEAGLDrawablePropertyRetainedBacking` is YES).
/// Convenience wrapper for use by `EAGLContext`.
pub fn is_retained_backing(env: &mut Environment, layer: id) -> bool {
    msg![env; layer _touchHLE_retainedBacking]
}

/// Returns the drawable's pixel width (from the presented pixel buffer if
/// available, otherwise from the layer's bounds).
pub fn drawable_width(env: &mut Environment, layer: id) -> u32 {
    let host = env.objc.borrow::<CALayerHostObject>(layer);
    if let Some((_, w, _)) = host.presented_pixels {
        return w;
    }
    host.bounds.size.width as u32
}

/// Returns the drawable's pixel height.
pub fn drawable_height(env: &mut Environment, layer: id) -> u32 {
    let host = env.objc.borrow::<CALayerHostObject>(layer);
    if let Some((_, _, h)) = host.presented_pixels {
        return h;
    }
    host.bounds.size.height as u32
}
