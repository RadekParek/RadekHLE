//! App picker GUI.
//!
//! This also includes a license text viewer. The license text viewer is needed
//! on Android, where the command-line way to view license text doesn't exist.

use crate::bundle::Bundle;
use crate::frameworks::core_graphics::cg_bitmap_context::{
    CGBitmapContextCreate, CGBitmapContextCreateImage,
};
use crate::frameworks::core_graphics::cg_color_space::CGColorSpaceCreateDeviceRGB;
use crate::frameworks::core_graphics::cg_context::{
    CGContextFillRect, CGContextRelease, CGContextScaleCTM, CGContextSetRGBFillColor,
    CGContextTranslateCTM,
};
use crate::frameworks::core_graphics::cg_image::{self, kCGImageAlphaPremultipliedLast};
use crate::frameworks::core_graphics::{CGFloat, CGPoint, CGRect, CGSize};
use crate::frameworks::foundation::ns_run_loop::run_run_loop_single_iteration;
use crate::frameworks::foundation::ns_string;
use crate::frameworks::foundation::NSInteger;
use crate::frameworks::uikit::ui_font::{
    UITextAlignmentCenter, UITextAlignmentLeft, UITextAlignmentRight,
};
use crate::frameworks::uikit::ui_graphics::{UIGraphicsPopContext, UIGraphicsPushContext};
use crate::frameworks::uikit::ui_view::ui_control::ui_button::{
    UIButtonTypeCustom, UIButtonTypeRoundedRect,
};
use crate::frameworks::uikit::ui_view::ui_control::{
    UIControlEventTouchUpInside, UIControlEventValueChanged, UIControlStateNormal,
};
use crate::fs::BundleData;
use crate::image::Image;
use crate::mem::Ptr;
use crate::objc::{id, msg, msg_class, nil, objc_classes, release, ClassExports, HostObject};
use crate::options::Options;
use crate::paths;
use crate::window::DeviceOrientation;
use crate::Environment;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};

struct AppInfo {
    path: PathBuf,
    display_name: String,
    icon: Option<Image>,
    /// `NSString*`
    display_name_ns_string: Option<id>,
    /// `UIImage*`
    icon_ui_image: Option<id>,
}

pub fn app_picker(options: Options) -> Result<(PathBuf, Vec<String>), String> {
    let apps_dir = paths::user_data_base_path().join(paths::APPS_DIR);

    let apps: Result<Vec<AppInfo>, String> = if !apps_dir.is_dir() {
        Err(format!("The {} directory couldn't be found. Check you're running touchHLE from the right directory.", apps_dir.display()))
    } else {
        enumerate_apps(&apps_dir)
            .map_err(|err| {
                format!(
                    "Couldn't get list of apps in the {} directory: {}.",
                    apps_dir.display(),
                    err
                )
            })
    };

    show_app_picker_gui(options, apps)
}

fn enumerate_apps(apps_dir: &Path) -> Result<Vec<AppInfo>, std::io::Error> {
    let mut apps = Vec::new();
    let mut directories = vec![apps_dir.to_path_buf()];
    while let Some(directory) = directories.pop() {
        for entry in std::fs::read_dir(directory)? {
            let app_path = entry?.path();
            let extension = app_path.extension();
            if extension == Some(OsStr::new("app")) || extension == Some(OsStr::new("ipa")) {
                let (bundle, fs) = match BundleData::open_any(&app_path).and_then(|bundle_data| {
                    Bundle::new_bundle_and_fs_from_host_path(bundle_data, /* read_only_mode: */ true)
                }) {
                    Ok(ok) => ok,
                    Err(e) => {
                        log!(
                            "Warning: couldn't open app bundle {}: {} (skipping)",
                            app_path.display(),
                            e
                        );
                        continue;
                    }
                };

                let display_name = bundle.display_name().to_owned();
                let icon = match bundle.load_icon(&fs) {
                    Ok(icon) => Some(icon),
                    Err(e) => {
                        log!("Warning: couldn't load icon for app bundle {}: {} (displaying placeholder instead)", app_path.display(), e);
                        None
                    }
                };

                apps.push(AppInfo {
                    path: app_path,
                    display_name,
                    icon,
                    display_name_ns_string: None,
                    icon_ui_image: None,
                });
            } else if app_path.is_dir() {
                directories.push(app_path);
            }
        }
    }

    apps.sort_by_key(|app| app.display_name.to_uppercase());
    Ok(apps)
}

const IOS_VERSION_ENTRIES: &[(&str, i32)] = &[
    ("Latest (iOS 26.6)", 0),
    ("iOS 2.0", 1),
    ("iOS 3.0", 2),
    ("iOS 4.3", 3),
    ("iOS 5.1", 4),
    ("iOS 6.1", 5),
    ("iOS 7.1", 6),
    ("iOS 8.4", 7),
    ("iOS 9.3", 8),
    ("iOS 10.3", 9),
    ("iOS 11.4", 10),
    ("iOS 12.4.1", 11),
    ("iOS 13.7", 12),
    ("iOS 14.8.1", 13),
    ("iOS 15.8.8", 14),
    ("iOS 16.7.16", 15),
    ("iOS 17.7.11", 16),
    ("iOS 18.7.9", 17),
    ("iOS 26.6", 18),
];

fn ios_version_for_tag(tag: i32) -> Option<(i32, i32, i32)> {
    match tag {
        0 => None,
        1 => Some((2, 0, 0)),
        2 => Some((3, 0, 0)),
        3 => Some((4, 3, 0)),
        4 => Some((5, 1, 0)),
        5 => Some((6, 1, 0)),
        6 => Some((7, 1, 0)),
        7 => Some((8, 4, 0)),
        8 => Some((9, 3, 0)),
        9 => Some((10, 3, 0)),
        10 => Some((11, 4, 0)),
        11 => Some((12, 4, 1)),
        12 => Some((13, 7, 0)),
        13 => Some((14, 8, 1)),
        14 => Some((15, 8, 8)),
        15 => Some((16, 7, 16)),
        16 => Some((17, 7, 11)),
        17 => Some((18, 7, 9)),
        18 => Some((26, 6, 0)),
        _ => None,
    }
}

fn ios_version_tag(value: Option<(i32, i32, i32)>) -> i32 {
    match value {
        None => 0,
        Some((2, 0, 0)) => 1,
        Some((3, 0, 0)) => 2,
        Some((4, 3, 0)) => 3,
        Some((5, 1, 0)) => 4,
        Some((6, 1, 0)) => 5,
        Some((7, 1, 0)) => 6,
        Some((8, 4, 0)) => 7,
        Some((9, 3, 0)) => 8,
        Some((10, 3, 0)) => 9,
        Some((11, 4, 0)) => 10,
        Some((12, 4, 1)) => 11,
        Some((13, 7, 0)) => 12,
        Some((14, 8, 1)) => 13,
        Some((15, 8, 8)) => 14,
        Some((16, 7, 16)) => 15,
        Some((17, 7, 11)) => 16,
        Some((18, 7, 9)) => 17,
        Some((26, 6, 0)) => 18,
        _ => 0,
    }
}

fn ios_version_label(value: Option<(i32, i32, i32)>) -> String {
    let tag = ios_version_tag(value);
    IOS_VERSION_ENTRIES
        .iter()
        .find(|(_, entry_tag)| *entry_tag == tag)
        .map(|(label, _)| (*label).to_string())
        .unwrap_or_else(|| "Latest (iOS 26.6)".to_string())
}

#[derive(Default)]
struct AppPickerDelegateHostObject {
    icon_tapped: id,
    copyright_show: bool,
    copyright_hide: bool,
    copyright_prev: bool,
    copyright_next: bool,
    quick_options_show: bool,
    quick_options_hide: bool,
    scale_hack_default: bool,
    scale_hack1: bool,
    scale_hack_half: bool,
    scale_hack_three_quarters: bool,
    scale_hack2: bool,
    scale_hack3: bool,
    scale_hack4: bool,
    orientation_default: bool,
    orientation_landscape_left: bool,
    orientation_landscape_right: bool,
    orientation_portrait_upside_down: bool,
    analog_stick_tilt_controls: Option<bool>,
    network: Option<bool>,
    /// Quick option: show FPS counter (maps to --print-fps)
    show_fps: Option<bool>,
    frame_pacing: Option<bool>,
    fullscreen: Option<bool>,
    angle_driver: Option<bool>,
    log_file: Option<bool>,
    fast_memory: Option<bool>,
    force_32_bit: Option<bool>,
    device_model_tag: Option<i32>,
    device_model_toggle: bool,
    device_model_scroll_up: bool,
    device_model_scroll_down: bool,
    apps_refresh_requested: bool,
    ios_version_toggle: bool,
    ios_version: Option<Option<(i32, i32, i32)>>,
    graphics_api_toggle: bool,
    graphics_api: Option<crate::options::GraphicsApi>,
}
impl HostObject for AppPickerDelegateHostObject {}

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    // Not a real iOS dylib obviously. This shouldn't really be in the list of
    // dylibs if we can avoid it somehow (TODO?).
    path: "/.touchHLE/AppPickerHelpers.dylib",
    aliases: &[],
    class_exports: &[CLASSES],
    constant_exports: &[],
    function_exports: &[],
};

/// Be careful! These classes go in the normal class list, just like everything
/// else, so an app could try to instantiate them. Don't give them special
/// powers that could be exploited!
const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation _touchHLE_AppPickerDelegate: NSObject

- (())iconTapped:(id)sender {
    // There is no allocWithZone: that creates AppPickerDelegateHostObject, so
    // this downcast effectively acts as an assertion that this class is being
    // used within the app picker, so it can't be abused. :)
    let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);
    host_obj.icon_tapped = sender;
}

- (())copyrightInfoShow {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).copyright_show = true;
}
- (())copyrightInfoHide {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).copyright_hide = true;
}
- (())copyrightInfoPrevPage {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).copyright_prev = true;
}
- (())copyrightInfoNextPage {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).copyright_next = true;
}

- (())quickOptionsShow {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).quick_options_show = true;
}
- (())quickOptionsHide {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).quick_options_hide = true;
}
- (())scaleHackDefault {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack_default = true;
}
- (())scaleHack1 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack1 = true;
}
- (())scaleHackHalf {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack_half = true;
}
- (())scaleHackThreeQuarters {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack_three_quarters = true;
}
- (())scaleHack2 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack2 = true;
}
- (())scaleHack3 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack3 = true;
}
- (())scaleHack4 {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).scale_hack4 = true;
}
- (())orientationDefault {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_default = true;
}
- (())orientationLandscapeLeft {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_landscape_left = true;
}
- (())orientationLandscapeRight {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_landscape_right = true;
}
- (())orientationPortraitUpsideDown {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).orientation_portrait_upside_down = true;
}
- (())analogStickTiltControls:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).analog_stick_tilt_controls = Some(switch_state);
}
- (())network:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).network = Some(switch_state);
}
- (())showFPS:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).show_fps = Some(switch_state);
    if switch_state {
        std::env::set_var("TOUCHHLE_ONSCREEN_FPS", "1");
        crate::gles::present::set_onscreen_fps_enabled(true);
    } else {
        std::env::remove_var("TOUCHHLE_ONSCREEN_FPS");
        crate::gles::present::set_onscreen_fps_enabled(false);
    }
}
- (())framePacing:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).frame_pacing = Some(switch_state);
}
- (())fullscreen:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fullscreen = Some(switch_state);
}
- (())angleDriver:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).angle_driver = Some(switch_state);
}
- (())logFile:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).log_file = Some(switch_state);
}
- (())fastMemory:(id)switch { // UISwitch*
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).fast_memory = Some(switch_state);
}
- (())force32Bit:(id)switch {
    let switch_state: bool = msg![env; switch isOn];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).force_32_bit = Some(switch_state);
}
- (())deviceModel:(id)sender { // UIButton*
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).device_model_tag = Some(tag as i32);
}
- (())deviceModelToggle {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).device_model_toggle = true;
}
- (())deviceModelScrollUp {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).device_model_scroll_up = true;
}
- (())deviceModelScrollDown {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).device_model_scroll_down = true;
}
- (())refreshApps {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).apps_refresh_requested = true;
}
- (())iosVersionToggle {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).ios_version_toggle = true;
}
- (())iosVersion:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).ios_version = Some(ios_version_for_tag(tag as i32));
}

- (())graphicsApiToggle {
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).graphics_api_toggle = true;
}
- (())graphicsApi:(id)sender {
    let tag: NSInteger = msg![env; sender tag];
    let api = match tag as i32 {
        0 => crate::options::GraphicsApi::Default,
        1 => crate::options::GraphicsApi::Translator,
        2 => crate::options::GraphicsApi::TranslatorGLES30,
        3 => crate::options::GraphicsApi::Metal,
        _ => crate::options::GraphicsApi::Default,
    };
    env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).graphics_api = Some(api);
}

- (())openFileManager {
    // Assert (see above).
    let _ = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);

    match paths::url_for_opening_apps_dir() {
        Ok(url) => {
            // Our `openURL:` implementation is bypassed because it doesn't
            // allow non-web URLs.
            let url_res = crate::window::open_url(env, &url);
            if let Err(e) = url_res {
                echo!("Couldn't open file manager at {:?}: {}", url, e);
            } else {
                echo!("Opened game folder at {:?}, returning to the picker.", url);
                env.objc.borrow_mut::<AppPickerDelegateHostObject>(this).apps_refresh_requested = true;
            }
        },
        Err(e) => echo!("Couldn't open file manager: {}", e),
    }
}

- (())visitWebsite {
    // Assert (see above).
    let _ = env.objc.borrow_mut::<AppPickerDelegateHostObject>(this);

    let url = ns_string::get_static_str(env, "https://touchhle.org/");
    let url: id = msg_class![env; NSURL URLWithString:url];
    let ui_application: id = msg_class![env; UIApplication sharedApplication];
    assert!(msg![env; ui_application openURL:url]);
}

@end

};

fn show_app_picker_gui(
    options: Options,
    apps: Result<Vec<AppInfo>, String>,
) -> Result<(PathBuf, Vec<String>), String> {
    let icon = {
        let bytes: &[u8] = match crate::branding() {
            "" => include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon.png")),
            "UNOFFICIAL" => include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/res/icon_unofficial.png"
            )),
            "PREVIEW" => {
                include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/icon_preview.png"))
            }
            _ => panic!(),
        };
        let mut image = Image::from_bytes(bytes).unwrap();
        // should match Bundle::load_icon()
        // Use a slightly smaller corner radius for larger icons for a cleaner look.
                let corner_radius_px = 12.0;
                image.round_corners(
                    corner_radius_px,
                    /* four_corners: */ true,
                    /* add_sheen: */ true,
                );
        image
    };
    let mut options = options;
    let picker_canvas_size = crate::window::host_screen_size()
        .map(|(width, height)| {
            let short_side = width.min(height).max(1);
            let long_side = width.max(height);
            let logical_width = 320u32;
            let logical_height = ((logical_width as f32 * long_side as f32 / short_side as f32)
                .round() as u32)
                .max(480);
            (logical_width, logical_height)
        })
        .unwrap_or((320, 568));
    options.host_screen_size = Some(picker_canvas_size);
    options.scale_hack = 2.0;
    log!(
        "App picker: using fixed {}x{} logical canvas at 2x internal resolution, preserving host aspect ratio.",
        picker_canvas_size.0,
        picker_canvas_size.1
    );
    if !options.fullscreen && !crate::window::Window::rotatable_fullscreen() {
        options.fullscreen = true;
        log!("App picker: enabling fullscreen so the picker uses the complete host display");
    }
    let environment = Environment::new_without_app(options, icon)?;
    Ok(environment.run_app_picker(|env| app_picker_inner(env, apps)))
}

fn app_picker_inner(
    env: &mut Environment,
    mut apps: Result<Vec<AppInfo>, String>,
) -> (PathBuf, Vec<String>) {
    let mut option_args = Vec::new();
    // Note that objects are generally not released in this code, because they
    // don't need to be: the entire Environment is thrown away at the end.

    // Bypassing UIApplicationMain!
    let ui_application: id = msg_class![env; UIApplication new];
    let delegate = env
        .objc
        .get_known_class("_touchHLE_AppPickerDelegate", &mut env.mem);
    let delegate = env.objc.alloc_object(
        delegate,
        Box::<AppPickerDelegateHostObject>::default(),
        &mut env.mem,
    );
    () = msg![env; ui_application setDelegate:delegate];

    let screen: id = msg_class![env; UIScreen mainScreen];
    let bounds: CGRect = msg![env; screen bounds];

    let window: id = msg_class![env; UIWindow alloc];
    let window: id = msg![env; window initWithFrame:bounds];

    let app_frame: CGRect = bounds;
    let CGSize { width: app_frame_width, height: app_frame_height } = app_frame.size;
    let ui_scale = picker_ui_scale(app_frame.size);
    log!(
        "App picker layout: logical frame {:.0}x{:.0}, UI scale {:.2}",
        app_frame_width,
        app_frame_height,
        ui_scale
    );
    let main_view: id = msg_class![env; UIView alloc];
    let main_view: id = msg![env; main_view initWithFrame:app_frame];
    let picker_background: id = msg_class![env; UIColor colorWithWhite:0.72 alpha:1.0];
    () = msg![env; main_view setBackgroundColor:picker_background];
    () = msg![env; main_view setOpaque:true];
    () = msg![env; window setBackgroundColor:picker_background];
    () = msg![env; window addSubview:main_view];

    // Wallpaper
    let mut found_wallpaper = false;
    let mut have_wallpaper = false;
    for candidate in paths::WALLPAPER_FILES {
        let candidate = paths::user_data_base_path().join(candidate);
        if !candidate.exists() {
            continue;
        }
        found_wallpaper = true;

        let image = match std::fs::read(&candidate) {
            Ok(image) => image,
            Err(e) => {
                log!("Warning: couldn't read {}: {}", candidate.display(), e);
                break;
            }
        };
        let image = match Image::from_bytes(&image) {
            Ok(image) => image,
            Err(e) => {
                log!("Warning: couldn't decode {}: {}", candidate.display(), e);
                break;
            }
        };

        let image = cg_image::from_image(env, image);
        let image: id = msg_class![env; UIImage imageWithCGImage:image];
        let wallpaper: id = msg_class![env; UIImageView alloc];
        let wallpaper: id = msg![env; wallpaper initWithImage:image];
        () = msg![env; wallpaper setFrame:(CGRect {
            origin: CGPoint {
                x: 0.0,
                y: 0.0,
            },
            size: app_frame.size,
        })];
        () = msg![env; wallpaper setContentMode:2];
        () = msg![env; wallpaper setAlpha:(1.0 as CGFloat)];
        () = msg![env; main_view insertSubview:wallpaper atIndex:0];
        have_wallpaper = true;
        break;
    }
    if !found_wallpaper {
        if let Ok(mut resource) = paths::ResourceFile::open("touchHLE_wallpaper.png") {
            let mut bytes = Vec::new();
            if resource.get().read_to_end(&mut bytes).is_ok() {
                if let Ok(image) = Image::from_bytes(&bytes) {
                    let image = cg_image::from_image(env, image);
                    let image: id = msg_class![env; UIImage imageWithCGImage:image];
                    let wallpaper: id = msg_class![env; UIImageView alloc];
                    let wallpaper: id = msg![env; wallpaper initWithImage:image];
                    () = msg![env; wallpaper setFrame:(CGRect {
                        origin: CGPoint { x: 0.0, y: 0.0 },
                        size: app_frame.size,
                    })];
                    () = msg![env; wallpaper setContentMode:2];
                    () = msg![env; wallpaper setAlpha:(1.0 as CGFloat)];
                    () = msg![env; main_view insertSubview:wallpaper atIndex:0];
                    have_wallpaper = true;
                }
            }
        }
    }
    if !have_wallpaper {
        let CGSize { width, height } = app_frame.size;
        log!(
            "No wallpaper found; filename can be one of: {}; ideal size is {}×{} pixels",
            paths::WALLPAPER_FILES.join(", "),
            width,
            height,
        );
    }

    // Version label
    {
        let label_frame = CGRect {
            origin: CGPoint {
                x: 0.0,
                y: app_frame.size.height - 20.0 * ui_scale,
            },
            size: CGSize {
                width: app_frame.size.width - 5.0,
                height: 18.0 * ui_scale,
            },
        };
        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:label_frame];
        let text = ns_string::from_rust_string(
            env,
            format!(
                "RadekHLE 3.0 {}{}{}",
                crate::branding(),
                if crate::branding().is_empty() {
                    ""
                } else {
                    " "
                },
                crate::VERSION
            ),
        );
        () = msg![env; label setText:text];
        () = msg![env; label setTextAlignment:UITextAlignmentRight];
        let font_size: CGFloat = 12.0 * ui_scale;
        let font: id = picker_font(env, font_size);
        () = msg![env; label setFont:font];
        () = msg![env; label setAdjustsFontSizeToFitWidth:true];
        () = msg![env; label setMinimumFontSize:8.0];
        let text_color: id = if have_wallpaper {
            msg_class![env; UIColor whiteColor]
        } else {
            msg_class![env; UIColor lightGrayColor]
        };
        () = msg![env; label setTextColor:text_color];
        let bg_color: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:bg_color];
        () = msg![env; main_view addSubview:label];
    }

    let divider = app_frame.size.height - 220.0 * ui_scale;

    let mut icon_grid_stuff = match &mut apps {
        Ok(ref mut apps) => {
            let mut icon_grid_stuff = make_icon_grid(
                env,
                delegate,
                main_view,
                app_frame,
                apps.len(),
                have_wallpaper,
            );
            update_icon_grid(env, &mut icon_grid_stuff, apps, 0);
            Some(icon_grid_stuff)
        }
        Err(e) => {
            let label_frame = CGRect {
                origin: CGPoint { x: 10.0, y: 10.0 },
                size: CGSize {
                    width: app_frame.size.width - 20.0,
                    height: divider - 20.0,
                },
            };
            let label: id = msg_class![env; UILabel alloc];
            let label: id = msg![env; label initWithFrame:label_frame];
            let text = ns_string::from_rust_string(env, e.clone());
            () = msg![env; label setText:text];
            () = msg![env; label setTextAlignment:UITextAlignmentCenter];
            () = msg![env; label setNumberOfLines:0]; // unlimited
            let text_color: id = msg_class![env; UIColor lightGrayColor];
            () = msg![env; label setTextColor:text_color];
            let bg_color: id = msg_class![env; UIColor clearColor];
            () = msg![env; label setBackgroundColor:bg_color];
            () = msg![env; main_view addSubview:label];
            None
        }
    };

    let buttons_row_center = divider + 48.0 * ui_scale;
    let buttons_row2_center = divider + 122.0 * ui_scale;
    make_app_launcher_grid(
        env,
        delegate,
        main_view,
        app_frame.size,
        buttons_row_center,
        buttons_row2_center,
    );

    let copyright_info_text = crate::licenses::get_text();
    let mut copyright_info_stuff = setup_copyright_info(env, delegate, main_view, app_frame);
    let mut copyright_info_page_idx = 0;

    let quick_options_stuff = setup_quick_options(env, delegate, main_view, app_frame);
    let mut quick_options_scale_hack: Option<f32> = None;
    let mut quick_options_fullscreen: Option<()> = None;
    let mut quick_options_orientation: Option<DeviceOrientation> = None;
    let mut quick_options_analog_stick_tilt_controls = true;
    let mut quick_options_network = false;
    let mut quick_options_show_fps = true;
    let mut quick_options_frame_pacing = true;
    let mut quick_options_angle_driver = false;
    let mut quick_options_log_file = true;
    let mut quick_options_fast_memory = true;
    let mut quick_options_force_32_bit = false;
    let mut quick_options_device_tag: Option<i32> = None;
    let mut quick_options_device_model_open = false;
    let mut quick_options_device_model_scroll: isize = 0;
    let mut quick_options_ios_version: Option<(i32, i32, i32)> = None;
    let mut quick_options_graphics_api = crate::options::GraphicsApi::Default;

    fn update_quick_option_buttons(env: &mut Environment, buttons: &[id], selected_idx: usize) {
        for (idx, &button) in buttons.iter().enumerate() {
            let color: id = if idx == selected_idx {
                msg_class![env; UIColor magentaColor]
            } else {
                msg_class![env; UIColor grayColor]
            };
            () = msg![env; button setBackgroundColor:color];
        }
    }
    fn update_scale_hack_buttons(env: &mut Environment, buttons: &[id], value: Option<f32>) {
        let selected = match value {
            None => 0,
            Some(v) if (v - 1.0).abs() < f32::EPSILON => 1,
            Some(v) if (v - 0.5).abs() < f32::EPSILON => 2,
            Some(v) if (v - 0.75).abs() < f32::EPSILON => 3,
            Some(v) if (v - 2.0).abs() < f32::EPSILON => 4,
            Some(v) if (v - 3.0).abs() < f32::EPSILON => 5,
            Some(v) if (v - 4.0).abs() < f32::EPSILON => 6,
            Some(_) => 0,
        };
        update_quick_option_buttons(env, buttons, selected);
    }
    fn update_ios_version_dropdown(
        env: &mut Environment,
        button: id,
        menu: id,
        items: &[id],
        value: Option<(i32, i32, i32)>,
    ) {
        let tag = ios_version_tag(value);
        for &item in items {
            let item_tag: NSInteger = msg![env; item tag];
            let selected = item_tag == tag as NSInteger;
            let color: id = if selected {
                msg_class![env; UIColor magentaColor]
            } else {
                msg_class![env; UIColor darkGrayColor]
            };
            () = msg![env; item setBackgroundColor:color];
        }
        let label = ios_version_label(value);
        let title = ns_string::from_rust_string(env, label);
        () = msg![env; button setTitle:title forState:UIControlStateNormal];
        let black: id = msg_class![env; UIColor blackColor];
        () = msg![env; button setTitleColor:black forState:UIControlStateNormal];
        () = msg![env; button layoutSubviews];
        release(env, title);
        () = msg![env; menu setHidden:true];
    }
    fn update_orientation_buttons(
        env: &mut Environment,
        buttons: &[id],
        value: Option<DeviceOrientation>,
    ) {
        update_quick_option_buttons(
            env,
            buttons,
            value.map_or(0, |v| match v {
                DeviceOrientation::LandscapeLeft => 1,
                DeviceOrientation::LandscapeRight => 2,
                DeviceOrientation::PortraitUpsideDown => 3,
                _ => panic!(),
            }),
        );
    }
    update_ios_version_dropdown(
        env,
        quick_options_stuff.ios_version_btn,
        quick_options_stuff.ios_version_menu,
        &quick_options_stuff.ios_version_items,
        quick_options_ios_version,
    );
    update_graphics_api_dropdown(
        env,
        quick_options_stuff.graphics_api_btn,
        &quick_options_stuff.graphics_api_items,
        quick_options_graphics_api,
    );
    update_scale_hack_buttons(
        env,
        &quick_options_stuff.scale_hack_buttons,
        quick_options_scale_hack,
    );
    update_orientation_buttons(
        env,
        &quick_options_stuff.orientation_buttons,
        quick_options_orientation,
    );
    update_device_model_menu(
        env,
        &quick_options_stuff.device_model_items,
        quick_options_stuff.device_model_thumb,
        quick_options_device_tag,
        quick_options_device_model_scroll,
    );

    () = msg![env; window makeKeyAndVisible];

    let main_run_loop: id = msg_class![env; NSRunLoop mainRunLoop];
    // If an app is picked, this loop returns. If the user quits touchHLE, the
    // process exits.
    let app_path = loop {
        run_run_loop_single_iteration(env, main_run_loop);
        let host_obj = env.objc.borrow_mut::<AppPickerDelegateHostObject>(delegate);
        let icon_tapped = std::mem::take(&mut host_obj.icon_tapped);
        if icon_tapped != nil {
            match icon_grid_stuff.as_ref().unwrap().icon_map.get(&icon_tapped) {
                Some(&TappedIcon::App(app_idx)) => {
                    // Provide visual feedback that the app has been picked
                    // (it may take a while for the splash screen to appear etc)
                    () = msg![env; icon_tapped setAlpha:(0.5 as CGFloat)];
                    // Redraw screen, even if this makes the next frame early
                    // (the app picker will never be redrawn after this).
                    crate::frameworks::core_animation::recomposite_if_necessary(
                        env, /* force: */ true,
                    );
                    // Ensure touchHLE is responsive from the OS perspective,
                    // otherwise screen redraw might not show up? (Unclear if
                    // this explanation is correct.)
                    run_run_loop_single_iteration(env, main_run_loop);

                    let app_path = &apps.as_ref().unwrap()[app_idx].path;
                    echo!("Picked: {}", app_path.display());
                    break app_path.clone();
                }
                Some(&TappedIcon::ChangePage(page_idx)) => {
                    update_icon_grid(
                        env,
                        icon_grid_stuff.as_mut().unwrap(),
                        apps.as_mut().unwrap(),
                        page_idx,
                    );
                }
                None => (), // Tapped on a black space
            }
            continue;
        }
        if std::mem::take(&mut host_obj.copyright_show) {
            copyright_info_page_idx = 0;
            change_copyright_page(
                env,
                &mut copyright_info_stuff,
                &copyright_info_text,
                copyright_info_page_idx,
            );
            () = msg![env; (copyright_info_stuff.main_view) setHidden:false];
        } else if std::mem::take(&mut host_obj.copyright_hide) {
            () = msg![env; (copyright_info_stuff.main_view) setHidden:true];
        } else if std::mem::take(&mut host_obj.copyright_prev) && copyright_info_page_idx != 0 {
            copyright_info_page_idx -= 1;
            change_copyright_page(
                env,
                &mut copyright_info_stuff,
                &copyright_info_text,
                copyright_info_page_idx,
            );
        } else if std::mem::take(&mut host_obj.copyright_next)
            && Some(copyright_info_page_idx) != copyright_info_stuff.last_page_idx
        {
            copyright_info_page_idx += 1;
            change_copyright_page(
                env,
                &mut copyright_info_stuff,
                &copyright_info_text,
                copyright_info_page_idx,
            );
        } else if std::mem::take(&mut host_obj.quick_options_show) {
            () = msg![env; (quick_options_stuff.main_view) setHidden:false];
        } else if std::mem::take(&mut host_obj.quick_options_hide) {
            () = msg![env; (quick_options_stuff.main_view) setHidden:true];
        } else if std::mem::take(&mut host_obj.apps_refresh_requested) {
            let apps_dir = paths::user_data_base_path().join(paths::APPS_DIR);
            match enumerate_apps(&apps_dir) {
                Ok(new_apps) if !new_apps.is_empty() => {
                    apps = Ok(new_apps);
                    if let Some(icon_grid) = icon_grid_stuff.as_mut() {
                        *icon_grid = make_icon_grid(
                            env,
                            delegate,
                            main_view,
                            app_frame,
                            apps.as_ref().unwrap().len(),
                            have_wallpaper,
                        );
                        update_icon_grid(env, icon_grid, apps.as_mut().unwrap(), 0);
                    }
                }
                Ok(_) => echo!("No games found in the game folder yet."),
                Err(e) => echo!("Couldn't refresh the game list: {}", e),
            }
        } else if std::mem::take(&mut host_obj.ios_version_toggle) {
            let hidden: bool = msg![env; (quick_options_stuff.ios_version_menu) isHidden];
            () = msg![env; (quick_options_stuff.ios_version_menu) setHidden:(!hidden)];
            if hidden {
                () = msg![env; (quick_options_stuff.main_view) bringSubviewToFront:(quick_options_stuff.ios_version_menu)];
                () = msg![env; (quick_options_stuff.main_view) bringSubviewToFront:(quick_options_stuff.ios_version_btn)];
            }
        } else if let Some(version) = std::mem::take(&mut host_obj.ios_version) {
            quick_options_ios_version = version;
            update_ios_version_dropdown(env, quick_options_stuff.ios_version_btn, quick_options_stuff.ios_version_menu, &quick_options_stuff.ios_version_items, quick_options_ios_version);
        } else if std::mem::take(&mut host_obj.graphics_api_toggle) {
            let hidden: bool = msg![env; (quick_options_stuff.graphics_api_menu) isHidden];
            () = msg![env; (quick_options_stuff.graphics_api_menu) setHidden:(!hidden)];
            if hidden {
                () = msg![env; (quick_options_stuff.main_view) bringSubviewToFront:(quick_options_stuff.graphics_api_menu)];
                () = msg![env; (quick_options_stuff.main_view) bringSubviewToFront:(quick_options_stuff.graphics_api_btn)];
            }
        } else if let Some(api) = std::mem::take(&mut host_obj.graphics_api) {
            quick_options_graphics_api = api;
            update_graphics_api_dropdown(env, quick_options_stuff.graphics_api_btn, &quick_options_stuff.graphics_api_items, api);
            () = msg![env; (quick_options_stuff.graphics_api_menu) setHidden:true];
        } else if std::mem::take(&mut host_obj.scale_hack_default) {
            quick_options_scale_hack = None;
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack1) {
            quick_options_scale_hack = Some(1.0);
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack_half) {
            quick_options_scale_hack = Some(0.5);
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack_three_quarters) {
            quick_options_scale_hack = Some(0.75);
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack2) {
            quick_options_scale_hack = Some(2.0);
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack3) {
            quick_options_scale_hack = Some(3.0);
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.scale_hack4) {
            quick_options_scale_hack = Some(4.0);
            update_scale_hack_buttons(
                env,
                &quick_options_stuff.scale_hack_buttons,
                quick_options_scale_hack,
            );
        } else if std::mem::take(&mut host_obj.orientation_default) {
            quick_options_orientation = None;
            update_orientation_buttons(
                env,
                &quick_options_stuff.orientation_buttons,
                quick_options_orientation,
            );
        } else if std::mem::take(&mut host_obj.orientation_landscape_left) {
            quick_options_orientation = Some(DeviceOrientation::LandscapeLeft);
            update_orientation_buttons(
                env,
                &quick_options_stuff.orientation_buttons,
                quick_options_orientation,
            );
        } else if std::mem::take(&mut host_obj.orientation_landscape_right) {
            quick_options_orientation = Some(DeviceOrientation::LandscapeRight);
            update_orientation_buttons(
                env,
                &quick_options_stuff.orientation_buttons,
                quick_options_orientation,
            );
        } else if std::mem::take(&mut host_obj.orientation_portrait_upside_down) {
            quick_options_orientation = Some(DeviceOrientation::PortraitUpsideDown);
            update_orientation_buttons(
                env,
                &quick_options_stuff.orientation_buttons,
                quick_options_orientation,
            );
        } else if let Some(tag) = std::mem::take(&mut host_obj.device_model_tag) {
            quick_options_device_tag = Some(tag);
            quick_options_device_model_open = false;
            () = msg![env; (quick_options_stuff.device_model_menu) setHidden:true];
            update_device_model_menu(
                env,
                &quick_options_stuff.device_model_items,
                quick_options_stuff.device_model_thumb,
                quick_options_device_tag,
                quick_options_device_model_scroll,
            );
            let title = format!("{} ▼", device_model_label_for_tag(quick_options_device_tag));
            let title_ns = ns_string::from_rust_string(env, title);
            () = msg![env; (quick_options_stuff.device_model_btn)
                setTitle:title_ns forState:UIControlStateNormal];
            release(env, title_ns);
        } else if std::mem::take(&mut host_obj.device_model_toggle) {
            quick_options_device_model_open = !quick_options_device_model_open;
            () = msg![env; (quick_options_stuff.device_model_menu)
                setHidden:(!quick_options_device_model_open)];
            if quick_options_device_model_open {
                () = msg![env; (quick_options_stuff.main_view)
                    bringSubviewToFront:(quick_options_stuff.device_model_menu)];
                () = msg![env; (quick_options_stuff.main_view)
                    bringSubviewToFront:(quick_options_stuff.device_model_btn)];
            }
            let arrow = if quick_options_device_model_open { "▲" } else { "▼" };
            let title = format!(
                "{} {}",
                device_model_label_for_tag(quick_options_device_tag),
                arrow
            );
            let title_ns = ns_string::from_rust_string(env, title);
            () = msg![env; (quick_options_stuff.device_model_btn)
                setTitle:title_ns forState:UIControlStateNormal];
            release(env, title_ns);
        } else if std::mem::take(&mut host_obj.device_model_scroll_up) {
            if quick_options_device_model_scroll > 0 {
                quick_options_device_model_scroll -= 1;
            }
            update_device_model_menu(
                env,
                &quick_options_stuff.device_model_items,
                quick_options_stuff.device_model_thumb,
                quick_options_device_tag,
                quick_options_device_model_scroll,
            );
        } else if std::mem::take(&mut host_obj.device_model_scroll_down) {
            let max_scroll = (quick_options_stuff.device_model_items.len() as isize)
                .saturating_sub(DEVICE_MENU_VISIBLE_ITEMS as isize);
            if quick_options_device_model_scroll < max_scroll {
                quick_options_device_model_scroll += 1;
            }
            update_device_model_menu(
                env,
                &quick_options_stuff.device_model_items,
                quick_options_stuff.device_model_thumb,
                quick_options_device_tag,
                quick_options_device_model_scroll,
            );
        } else if let Some(enabled) = std::mem::take(&mut host_obj.analog_stick_tilt_controls) {
            quick_options_analog_stick_tilt_controls = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.network) {
            quick_options_network = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.show_fps) {
            quick_options_show_fps = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.angle_driver) {
            quick_options_angle_driver = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.log_file) {
            quick_options_log_file = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.fast_memory) {
            quick_options_fast_memory = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.force_32_bit) {
            quick_options_force_32_bit = enabled;
        } else if let Some(enabled) = std::mem::take(&mut host_obj.frame_pacing) {
            quick_options_frame_pacing = enabled;
        } else if let Some(fullscreen) = std::mem::take(&mut host_obj.fullscreen) {
            quick_options_fullscreen = match fullscreen {
                false => None,
                true => Some(()),
            };
        }
    };

    // Apply user-specified overrides
    if let Some((major, minor, patch)) = quick_options_ios_version {
        option_args.push(format!("--ios-version={major}.{minor}.{patch}"));
    }
    if let Some(scale_hack) = quick_options_scale_hack {
        option_args.push(format!("--scale-hack={scale_hack}"));
    }
    if let Some(orientation) = quick_options_orientation {
        option_args.push(
            match orientation {
                DeviceOrientation::LandscapeLeft => "--landscape-left",
                DeviceOrientation::LandscapeRight => "--landscape-right",
                DeviceOrientation::PortraitUpsideDown => "--upside-down",
                _ => todo!(),
            }
            .to_string(),
        );
    }
    if let Some(()) = quick_options_fullscreen {
        option_args.push("--fullscreen".to_string());
    }
    if !quick_options_analog_stick_tilt_controls {
        option_args.push("--disable-analog-stick-tilt-controls".to_string());
    }
    if quick_options_network {
        option_args.push("--allow-network-access".to_string());
    }

    if quick_options_show_fps {
        option_args.push("--print-fps".to_string());
        std::env::set_var("TOUCHHLE_ONSCREEN_FPS", "1");
        crate::gles::present::set_onscreen_fps_enabled(true);
    }
    option_args.push(if quick_options_frame_pacing {
        "--enable-frame-pacing"
    } else {
        "--disable-frame-pacing"
    }.to_string());
    if quick_options_graphics_api != crate::options::GraphicsApi::Default {
        let value = match quick_options_graphics_api {
            crate::options::GraphicsApi::Translator => "translator",
            crate::options::GraphicsApi::TranslatorGLES30 => "translator-gles3",
            crate::options::GraphicsApi::GLES10 => "gles1.0",
            crate::options::GraphicsApi::GLES11 => "gles1.1",
            crate::options::GraphicsApi::GLES20 => "gles2.0",
            crate::options::GraphicsApi::GLES30 => "gles3.0",
            crate::options::GraphicsApi::Metal => "metal",
            crate::options::GraphicsApi::Default => unreachable!(),
        };
        option_args.push(format!("--graphics-api={value}"));
    }
    option_args.push(if quick_options_angle_driver {
        "--angle-driver"
    } else {
        "--disable-angle-driver"
    }.to_string());
    option_args.push(if quick_options_log_file {
        "--enable-log-file"
    } else {
        "--disable-log-file"
    }.to_string());
    option_args.push(if quick_options_fast_memory {
        "--enable-direct-memory-access"
    } else {
        "--disable-direct-memory-access"
    }.to_string());
    if quick_options_force_32_bit {
        option_args.push("--force-32-bit".to_string());
    }

    if let Some(tag) = quick_options_device_tag {
        let tag = tag as NSInteger;
        if tag == DEVICE_TAG_DEFAULT {
            // No override — fall back to the app bundle / built-in default.
        } else if tag == DEVICE_TAG_AUTO {
            option_args.push("--device-family=auto".to_string());
        } else if let Some(family) =
            crate::window::DeviceFamily::ALL_SELECTABLE.get(tag as usize)
        {
            option_args.push(format!("--device-family={}", family.option_name()));
        }
    }

    // Return the environment so some parts of it can be salvaged.
    (app_path, option_args)
}

const ICON_SIZE: CGSize = CGSize {
    width: 70.0,
    height: 70.0,
};
const ICON_IMAGE_INSET: CGFloat = 9.0;

fn picker_ui_scale(size: CGSize) -> CGFloat {
    let short_side = size.width.min(size.height);
    (short_side / 320.0).clamp(1.0, 4.5)
}

fn picker_font(env: &mut Environment, size: CGFloat) -> id {
    let name = ns_string::get_static_str(env, "HelveticaNeue");
    let font: id = msg_class![env; UIFont fontWithName:name size:size];
    release(env, name);
    if font == nil {
        msg_class![env; UIFont systemFontOfSize:size]
    } else {
        font
    }
}

enum TappedIcon {
    App(usize),
    ChangePage(usize),
}

struct IconGridStuff {
    icon_buttons_and_labels: Vec<(id, id)>,
    placeholder_icon: Option<id>,
    prev_icon: Option<id>,
    next_icon: Option<id>,
    pages: Vec<std::ops::Range<usize>>,
    icon_map: HashMap<id, TappedIcon>,
}

fn make_icon_grid(
    env: &mut Environment,
    delegate: id,
    main_view: id,
    app_frame: CGRect,
    total_app_count: usize,
    have_wallpaper: bool,
) -> IconGridStuff {
    let ui_scale = picker_ui_scale(app_frame.size);
    let short_side = app_frame.size.width.min(app_frame.size.height);
    let icon_size_value = (56.0 * ui_scale).min(short_side * 0.22).max(48.0);
    let icon_size = CGSize {
        width: icon_size_value,
        height: icon_size_value,
    };
    let num_cols = 4;
    let num_cols_f = num_cols as CGFloat;
    let num_rows = if app_frame.size.height >= 640.0 * ui_scale { 5 } else { 4 };
    let label_size = CGSize {
        width: icon_size.width + 14.0 * ui_scale,
        height: 22.0 * ui_scale,
    };
    let icon_gap_x: CGFloat = (short_side * 0.028).clamp(8.0, 22.0);
    let icon_gap_y: CGFloat = (short_side * 0.008).clamp(3.0, 8.0) + label_size.height;
    let icon_grid_width = (icon_size.width * num_cols_f) + icon_gap_x * (num_cols_f - 1.0);
    let icon_grid_origin = CGPoint {
        x: (app_frame.size.width - icon_grid_width) / 2.0,
        y: 16.0 * ui_scale,
    };

    let icon_tapped_sel = env.objc.lookup_selector("iconTapped:").unwrap();

    let mut icon_buttons_and_labels = Vec::new();

    for i in 0..(num_cols * num_rows) {
        let col = i % num_cols;
        let row = i / num_cols;

        // Rounding is needed here to avoid a blurry or offset image.
        let icon_frame = CGRect {
            origin: CGPoint {
                x: (icon_grid_origin.x + (col as CGFloat) * (icon_size.width + icon_gap_x)).round(),
                y: (icon_grid_origin.y + (row as CGFloat) * (icon_size.height + icon_gap_y))
                    .round(),
            },
            size: icon_size,
        };
        let icon_button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        () = msg![env; icon_button setFrame:icon_frame];
        let image_view: id = msg![env; icon_button imageView];
        let bounds: CGRect = msg![env; icon_button bounds];
        let inset = ICON_IMAGE_INSET * ui_scale;
        () = msg![env; image_view setFrame:(CGRect {
            origin: CGPoint { x: inset, y: inset },
            size: CGSize {
                width: (bounds.size.width - inset * 2.0).max(1.0),
                height: (bounds.size.height - inset * 2.0).max(1.0),
            },
        })];
        let layer: id = msg![env; image_view layer];
        let gravity = ns_string::get_static_str(env, "resizeAspect");
        () = msg![env; layer setContentsGravity:gravity];
        () = msg![env; icon_button addTarget:delegate
                                      action:icon_tapped_sel
                            forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; main_view addSubview:icon_button];

        // Rounding is needed here to avoid blurry text.
        let label_frame = CGRect {
            origin: CGPoint {
                x: (icon_frame.origin.x - (label_size.width - icon_size.width) / 2.0).round(),
                y: (icon_frame.origin.y + icon_size.height + 4.0 * ui_scale).round(),
            },
            size: label_size,
        };
        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:label_frame];
        () = msg![env; label setTextAlignment:UITextAlignmentCenter];
        let font_size: CGFloat = (11.0 * ui_scale).max(8.0);
        let font: id = picker_font(env, font_size);
        () = msg![env; label setFont:font];
        () = msg![env; label setNumberOfLines:2];
        () = msg![env; label setAdjustsFontSizeToFitWidth:true];
        () = msg![env; label setMinimumFontSize:8.0];
        let text_color: id = if have_wallpaper {
            msg_class![env; UIColor whiteColor]
        } else {
            msg_class![env; UIColor lightGrayColor]
        };
        () = msg![env; label setTextColor:text_color];
        let bg_color: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:bg_color];
        () = msg![env; main_view addSubview:label];

        icon_buttons_and_labels.push((icon_button, label));
    }

    // TODO: Use UIScrollView pagination and UIPageControl once available.
    let mut pages = Vec::new();
    if total_app_count == 0 {
        pages.push(0..0);
    }
    let mut start = 0;
    while start < total_app_count {
        let mut end = start + icon_buttons_and_labels.len();
        if start > 0 {
            end -= 1; // one icon space taken by "previous" button
        }
        if end < total_app_count {
            end -= 1; // one icon space taken by "next" button
        } else {
            end = total_app_count;
        }
        pages.push(start..end);
        start = end;
    }

    IconGridStuff {
        icon_buttons_and_labels,
        placeholder_icon: None,
        prev_icon: None,
        next_icon: None,
        pages,
        icon_map: HashMap::new(),
    }
}

fn make_icon_from_glyph(
    env: &mut Environment,
    glyph: char,
    font_size: CGFloat,
    baseline_offset: CGFloat,
    bg_color: (CGFloat, CGFloat, CGFloat, CGFloat),
) -> id {
    let color_space = CGColorSpaceCreateDeviceRGB(env);
    let context = CGBitmapContextCreate(
        env,
        Ptr::null(),
        ICON_SIZE.width as u32,
        ICON_SIZE.height as u32,
        8,
        4 * (ICON_SIZE.width as u32),
        color_space,
        kCGImageAlphaPremultipliedLast,
    );
    UIGraphicsPushContext(env, context);

    // Compensate for row order inversion
    CGContextTranslateCTM(env, context, 0.0, ICON_SIZE.height);
    CGContextScaleCTM(env, context, 1.0, -1.0);

    let (r, g, b, a) = bg_color;
    CGContextSetRGBFillColor(env, context, r, g, b, a);
    CGContextFillRect(
        env,
        context,
        CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: ICON_SIZE,
        },
    );

    let font: id = picker_font(env, font_size);
    let glyph_string: id = ns_string::from_rust_string(env, [glyph].into_iter().collect());
    let glyph_size: CGSize = msg![env; glyph_string sizeWithFont:font];
    CGContextSetRGBFillColor(env, context, 1.0, 1.0, 1.0, 1.0); // white
    let glyph_origin = CGPoint {
        x: ICON_SIZE.width / 2.0 - glyph_size.width / 2.0,
        y: ICON_SIZE.height / 2.0 - glyph_size.height / 2.0 + baseline_offset,
    };
    let _: CGSize = msg![env; glyph_string drawAtPoint:glyph_origin withFont:font];
    release(env, glyph_string);

    UIGraphicsPopContext(env);

    let cg_image = CGBitmapContextCreateImage(env, context);
    // This radius should match the one in src/bundle.rs.
    cg_image::borrow_image_mut(&mut env.objc, cg_image).round_corners(
            12.0,
        /* four_corners: */ true,
        /* add_sheen: */ true,
    );
    CGContextRelease(env, context);

    let ui_image: id = msg_class![env; UIImage imageWithCGImage:cg_image];
    release(env, cg_image);

    ui_image
}

fn update_icon_grid(
    env: &mut Environment,
    icon_grid_stuff: &mut IconGridStuff,
    apps: &mut [AppInfo],
    page_idx: usize,
) {
    icon_grid_stuff.icon_map.clear();

    let app_idx_range = icon_grid_stuff.pages[page_idx].clone();
    let have_prev_icon = page_idx != 0;
    let have_next_icon = app_idx_range.end != apps.len();

    let mut icon_iter = icon_grid_stuff.icon_buttons_and_labels.iter();

    if have_prev_icon {
        let &(icon_button, label) = icon_iter.next().unwrap();
        let image = *icon_grid_stuff.prev_icon.get_or_insert_with(|| {
            make_icon_from_glyph(env, '←', 50.0, -9.0, (0.25, 0.25, 0.25, 1.0))
        });
        () = msg![env; icon_button setImage:image forState:UIControlStateNormal];
        () = msg![env; label setText:(ns_string::get_static_str(env, ""))];
        icon_grid_stuff
            .icon_map
            .insert(icon_button, TappedIcon::ChangePage(page_idx - 1));
    }

    for app_idx in app_idx_range.clone() {
        let app = &mut apps[app_idx];

        let &(icon_button, label) = icon_iter.next().unwrap();

        if let Some(icon) = app.icon.take() {
            let image = cg_image::from_image(env, icon);
            let image: id = msg_class![env; UIImage imageWithCGImage:image];
            app.icon_ui_image = Some(image);
        }

        let image = app.icon_ui_image.unwrap_or_else(|| {
            *icon_grid_stuff.placeholder_icon.get_or_insert_with(|| {
                make_icon_from_glyph(env, '?', 40.0, 0.0, (0.5, 0.5, 0.5, 1.0))
            })
        });
        () = msg![env; icon_button setImage:image forState:UIControlStateNormal];

        let text = *app
            .display_name_ns_string
            .get_or_insert_with(|| ns_string::from_rust_string(env, app.display_name.clone()));
        () = msg![env; label setText:text];

        icon_grid_stuff
            .icon_map
            .insert(icon_button, TappedIcon::App(app_idx));
    }

    if have_next_icon {
        let &(icon_button, label) = icon_iter.next().unwrap();
        let image = *icon_grid_stuff.next_icon.get_or_insert_with(|| {
            make_icon_from_glyph(env, '→', 50.0, -9.0, (0.25, 0.25, 0.25, 1.0))
        });
        () = msg![env; icon_button setImage:image forState:UIControlStateNormal];
        () = msg![env; label setText:(ns_string::get_static_str(env, ""))];
        icon_grid_stuff
            .icon_map
            .insert(icon_button, TappedIcon::ChangePage(page_idx + 1));
    }

    // There may be remaining spaces might need to be blanked.
    for &(icon_button, label) in icon_iter {
        () = msg![env; icon_button setImage:nil forState:UIControlStateNormal];
        () = msg![env; label setText:(ns_string::get_static_str(env, ""))];
    }
}

fn make_app_launcher_grid(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    super_view_size: CGSize,
    first_row_center: CGFloat,
    second_row_center: CGFloat,
) {
    let ui_scale = picker_ui_scale(super_view_size);
    let short_side = super_view_size.width.min(super_view_size.height);
    let icon_size = (52.0 * ui_scale).min(short_side * 0.21).max(38.0);
    let card_width = (super_view_size.width * 0.40).max(icon_size + 12.0 * ui_scale);
    let items = [
        ("Files", "openFileManager", "/res/picker_files_icon.jpg"),
        ("Settings", "quickOptionsShow", "/res/picker_settings_icon.jpg"),
        ("Info", "copyrightInfoShow", "/res/picker_touchhle_icon.png"),
        ("TouchHLE.org", "visitWebsite", "/res/picker_touchhle_icon.png"),
    ];
    for (index, (title, selector_name, icon_path)) in items.iter().enumerate() {
        let row = index / 2;
        let column = index % 2;
        let center = if row == 0 { first_row_center } else { second_row_center };
        let card_center_x = if column == 0 {
            super_view_size.width * 0.28
        } else {
            super_view_size.width * 0.72
        };
        let icon_frame = CGRect {
            origin: CGPoint {
                x: (card_center_x - icon_size / 2.0).round(),
                y: (center - icon_size / 2.0 - 7.0 * ui_scale).round(),
            },
            size: CGSize {
                width: icon_size,
                height: icon_size,
            },
        };
        let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        () = msg![env; button setFrame:icon_frame];
        let resource: &[u8] = match *icon_path {
            "/res/picker_files_icon.jpg" => &include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/picker_files_icon.jpg"))[..],
            "/res/picker_settings_icon.jpg" => &include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/picker_settings_icon.jpg"))[..],
            "/res/picker_touchhle_icon.png" => &include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/res/picker_touchhle_icon.png"))[..],
            _ => unreachable!(),
        };
        let image = Image::from_bytes(resource).expect("picker icon resource must be valid");
        let image = cg_image::from_image(env, image);
        let image: id = msg_class![env; UIImage imageWithCGImage:image];
        () = msg![env; button setImage:image forState:UIControlStateNormal];
        let clear: id = msg_class![env; UIColor clearColor];
        () = msg![env; button setBackgroundColor:clear];
        let image_view: id = msg![env; button imageView];
        () = msg![env; image_view setContentMode:2];
        () = msg![env; image_view setFrame:(CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: icon_frame.size,
        })];
        let selector = env.objc.lookup_selector(selector_name).unwrap();
        () = msg![env; button addTarget:delegate
                                 action:selector
                       forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; super_view addSubview:button];

        let label_frame = CGRect {
            origin: CGPoint {
                x: (card_center_x - card_width / 2.0).round(),
                y: (icon_frame.origin.y + icon_size + 4.0 * ui_scale).round(),
            },
            size: CGSize {
                width: card_width,
                height: (17.0 * ui_scale).max(13.0),
            },
        };
        let label: id = msg_class![env; UILabel alloc];
        let label: id = msg![env; label initWithFrame:label_frame];
        let text = ns_string::get_static_str(env, title);
        () = msg![env; label setText:text];
        () = msg![env; label setTextAlignment:UITextAlignmentCenter];
        let font = picker_font(env, (11.0 * ui_scale).max(9.0));
        () = msg![env; label setFont:font];
        () = msg![env; label setAdjustsFontSizeToFitWidth:true];
        () = msg![env; label setMinimumFontSize:8.0];
        () = msg![env; label setNumberOfLines:1];
        let text_color: id = msg_class![env; UIColor whiteColor];
        () = msg![env; label setTextColor:text_color];
        let clear: id = msg_class![env; UIColor clearColor];
        () = msg![env; label setBackgroundColor:clear];
        () = msg![env; super_view addSubview:label];
    }
}

fn make_button_row(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    super_view_size: CGSize,
    buttons_row_center: CGFloat,
    buttons: &[(&'static str, &'static str)],
    font_size: Option<CGFloat>,
) -> Vec<id> {
    let ui_scale = picker_ui_scale(super_view_size);
    let margin = 6.0 * ui_scale;
    let button_size = CGSize {
        width: (super_view_size.width - margin * (buttons.len() as CGFloat + 1.0))
            / buttons.len() as CGFloat,
        height: 30.0 * ui_scale,
    };
    let mut button_frame = CGRect {
        origin: CGPoint {
            x: margin,
            y: buttons_row_center - button_size.height / 2.0,
        },
        size: button_size,
    };

    let mut ui_buttons = Vec::new();
    for (title_text, selector) in buttons {
        let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        let text = ns_string::get_static_str(env, title_text);
        () = msg![env; button setTitle:text forState:UIControlStateNormal];
        () = msg![env; button setFrame:button_frame];

        let label: id = msg![env; button titleLabel];
        let scaled_font_size = font_size.unwrap_or(12.0) * ui_scale;
        let font: id = picker_font(env, scaled_font_size);
        () = msg![env; label setFont:font];
        () = msg![env; label setAdjustsFontSizeToFitWidth:true];
        () = msg![env; label setMinimumFontSize:8.0];
        () = msg![env; label setTextAlignment:UITextAlignmentCenter];
        let black: id = msg_class![env; UIColor blackColor];
        () = msg![env; button setTitleColor:black forState:UIControlStateNormal];
        let white: id = msg_class![env; UIColor whiteColor];
        let _: () = msg![env; button setBackgroundColor:white];
        let layer: id = msg![env; button layer];
        () = msg![env; layer setCornerRadius:(7.0 * ui_scale)];
        () = msg![env; button layoutSubviews];

        let selector = env.objc.lookup_selector(selector).unwrap();
        () = msg![env; button addTarget:delegate
                                 action:selector
                       forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; super_view addSubview:button];

        button_frame.origin.x += button_size.width + margin;
        ui_buttons.push(button);
    }
    ui_buttons
}

struct CopyrightInfoStuff {
    main_view: id,
    text_frame: CGRect,
    text_label: id,
    font: id,
    pages: Vec<(std::ops::Range<usize>, CGFloat)>,
    last_page_idx: Option<usize>,
    prev_page_button: id,
    next_page_button: id,
}

fn setup_copyright_info(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    app_frame: CGRect,
) -> CopyrightInfoStuff {
    let main_frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: app_frame.size,
    };

    let divider = main_frame.size.height - 40.0;

    // Container for all the other stuff

    let main_view: id = msg_class![env; UIView alloc];
    let main_view: id = msg![env; main_view initWithFrame:main_frame];
    // TODO: Isn't white the default?
    let bg_color: id = msg_class![env; UIColor whiteColor];
    () = msg![env; main_view setBackgroundColor:bg_color];
    // This main_view is hidden until the copyright info button is tapped.
    () = msg![env; main_view setHidden:true];
    () = msg![env; super_view addSubview:main_view];

    // UILabel that will display part of the copyright text

    let padding = 10.0;
    let text_frame = CGRect {
        origin: CGPoint {
            x: padding,
            y: padding,
        },
        size: CGSize {
            width: app_frame.size.width - padding * 2.0,
            height: divider - padding * 2.0,
        },
    };

    let text_label: id = msg_class![env; UILabel alloc];
    let text_label: id = msg![env; text_label initWithFrame:text_frame];
    () = msg![env; text_label setNumberOfLines:0]; // unlimited
    let text_color: id = msg_class![env; UIColor blackColor];
    () = msg![env; text_label setTextColor:text_color];
    let bg_color: id = msg_class![env; UIColor clearColor];
    () = msg![env; text_label setBackgroundColor:bg_color];
    let font_size: CGFloat = 16.0;
    let font: id = picker_font(env, font_size);
    () = msg![env; text_label setFont:font];
    () = msg![env; main_view addSubview:text_label];

    // Navigation

    let buttons_row_center = (main_frame.size.height + divider) / 2.0;
    let buttons = make_button_row(
        env,
        delegate,
        main_view,
        main_frame.size,
        buttons_row_center,
        &[
            ("↑", "copyrightInfoPrevPage"),
            ("↓", "copyrightInfoNextPage"),
            ("×", "copyrightInfoHide"),
        ],
        Some(30.0),
    );

    CopyrightInfoStuff {
        main_view,
        text_frame,
        text_label,
        font,
        pages: Vec::new(),
        last_page_idx: None,
        prev_page_button: buttons[0],
        next_page_button: buttons[1],
    }
}

fn change_copyright_page(
    env: &mut Environment,
    copyright_info_stuff: &mut CopyrightInfoStuff,
    copyright_info_text: &str,
    page_idx: usize,
) {
    // TODO: Eventually this should be ripped out and replaced with a scrolling
    // UITextView, once that's implemented.

    let &mut CopyrightInfoStuff {
        text_frame,
        text_label,
        font,
        ref mut pages,
        ref mut last_page_idx,
        prev_page_button,
        next_page_button,
        ..
    } = copyright_info_stuff;

    // Lazily lay out pages of text as needed.

    if page_idx == pages.len() {
        let mut page_start = pages.last().map_or(0, |page| page.0.end);
        while copyright_info_text[page_start..].starts_with([' ', '\n', '\r']) {
            page_start += 1;
        }
        let mut page_height = 0.0;
        let page_end = loop {
            let mut line_start = page_start;
            while line_start < copyright_info_text.len() {
                let is_first_line = line_start == page_start;

                let line_end = if let Some(i) = copyright_info_text[line_start..].find('\n') {
                    line_start + i + 1
                } else {
                    copyright_info_text.len()
                };

                let line = &copyright_info_text[line_start..line_end];

                // Force pagination before headings (in Dynarmic's license text)
                if !is_first_line && line.starts_with("###") {
                    break;
                }

                let line_temp = ns_string::from_rust_string(env, line.to_string());
                let line_size: CGSize = msg![env; line_temp sizeWithFont:font
                                                       constrainedToSize:(text_frame.size)];
                // Avoid accumulation of old line strings.
                release(env, line_temp);

                if page_height + line_size.height > text_frame.size.height {
                    break;
                }

                page_height += line_size.height;
                line_start = line_end;

                // Force pagination after dividers
                if !is_first_line && line.starts_with("---") {
                    break;
                }
            }
            let page_end = line_start;
            assert!(page_start != page_end);

            // Avoid entirely blank pages
            if copyright_info_text[page_start..page_end].trim() == "" {
                page_start = page_end;
            } else {
                break page_end;
            }
        };
        assert!(page_start != page_end);
        pages.push((page_start..page_end, page_height));
        if page_end == copyright_info_text.len() {
            *last_page_idx = Some(page_idx);
        }
    }

    // Actually display the page

    let (page, page_height) = pages[page_idx].clone();
    let page = &copyright_info_text[page];

    let page: id = ns_string::from_rust_string(env, page.to_string());
    () = msg![env; text_label setText:page];
    // Avoid accumulation of old page strings.
    release(env, page);

    // UILabel always vertically centers text. Work around that by resizing it.
    let label_frame = CGRect {
        origin: text_frame.origin,
        size: CGSize {
            width: text_frame.size.width,
            // The page height is slightly off, a little padding is needed.
            height: page_height + 10.0,
        },
    };
    () = msg![env; text_label setFrame:label_frame];

    () = msg![env; prev_page_button setHidden:(page_idx == 0)];
    () = msg![env; next_page_button setHidden:(Some(page_idx) == *last_page_idx)];
}

struct QuickOptionsStuff {
    main_view: id,
    ios_version_btn: id,
    ios_version_menu: id,
    ios_version_items: Vec<id>,
    graphics_api_btn: id,
    graphics_api_menu: id,
    graphics_api_items: Vec<id>,
    scale_hack_buttons: [id; 7],
    orientation_buttons: [id; 4],
    /// The button that toggles the "Device model" dropdown open/closed. Its
    /// title shows the currently-selected model plus an up/down arrow.
    device_model_btn: id,
    /// The dropdown container view (hidden until toggled). Holds the scrollable
    /// list of choices, the scrollbar track/thumb, and the scroll arrows.
    device_model_menu: id,
    /// One button per choice in `device_model_entries()` order ("Default",
    /// "Auto", then every [crate::window::DeviceFamily] in `ALL_SELECTABLE`).
    /// Each carries a UIView `tag` identifying its choice (see
    /// `DEVICE_TAG_DEFAULT` / `DEVICE_TAG_AUTO` / model index).
    device_model_items: Vec<id>,
    /// The scrollbar thumb shown alongside the list.
    device_model_thumb: id,
}

/// Sentinel button tags for the device-model dropdown. Model buttons use their
/// index into `DeviceFamily::ALL_SELECTABLE` (0..=19) as their tag, so the
/// sentinels are placed well above that range.
const DEVICE_TAG_DEFAULT: NSInteger = 1000;
const DEVICE_TAG_AUTO: NSInteger = 1001;

/// How many rows of the device-model dropdown are visible at once before the
/// list has to be scrolled.
const DEVICE_MENU_VISIBLE_ITEMS: usize = 6;
/// Height of a single row in the device-model dropdown.
const DEVICE_MENU_ITEM_HEIGHT: CGFloat = 30.0;

/// The choices shown in the device-model dropdown, in display order, as
/// `(title, tag)` pairs: "Default" (no override), "Auto" (match host screen),
/// then one entry per [crate::window::DeviceFamily] in `ALL_SELECTABLE` order
/// tagged with its index.
fn device_model_entries() -> Vec<(String, NSInteger)> {
    use crate::window::DeviceFamily;
    let mut entries: Vec<(String, NSInteger)> = Vec::new();
    entries.push(("Default".to_string(), DEVICE_TAG_DEFAULT));
    entries.push(("Auto".to_string(), DEVICE_TAG_AUTO));
    for (idx, family) in DeviceFamily::ALL_SELECTABLE.iter().enumerate() {
        entries.push((family.display_name().to_string(), idx as NSInteger));
    }
    entries
}

/// Human-readable label for a device-model choice tag, used as the dropdown
/// button title.
fn device_model_label_for_tag(tag: Option<i32>) -> String {
    use crate::window::DeviceFamily;
    match tag.map(|t| t as NSInteger) {
        None | Some(DEVICE_TAG_DEFAULT) => "Default".to_string(),
        Some(DEVICE_TAG_AUTO) => "Native".to_string(),
        Some(idx) => DeviceFamily::ALL_SELECTABLE
            .get(idx as usize)
            .map(|f| f.display_name().to_string())
            .unwrap_or_else(|| "Default".to_string()),
    }
}

fn setup_quick_options(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    app_frame: CGRect,
) -> QuickOptionsStuff {
    // UIView*
    let visible_frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: app_frame.size,
    };
    let content_height = app_frame.size.height.max(1800.0);
    let main_frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize {
            width: app_frame.size.width,
            height: content_height,
        },
    };

    // Container for all the other stuff. The settings list is taller than the
    // screen and is hosted in a real scroll view so every option keeps a
    // readable row instead of being compressed into overlapping controls.

    let main_view: id = msg_class![env; UIScrollView alloc];
    let main_view: id = msg![env; main_view initWithFrame:visible_frame];
    let content_size = main_frame.size;
    () = msg![env; main_view setContentSize:content_size];
    () = msg![env; main_view setScrollEnabled:true];
    () = msg![env; main_view setShowsVerticalScrollIndicator:true];
    () = msg![env; main_view setAlwaysBounceVertical:true];
    let bg_color: id = msg_class![env; UIColor colorWithWhite:0.72 alpha:1.0];
    () = msg![env; main_view setBackgroundColor:bg_color];
    () = msg![env; main_view setOpaque:true];
    // This main_view is hidden until the copyright info button is tapped.
    () = msg![env; main_view setHidden:true];
    () = msg![env; super_view addSubview:main_view];

    let ui_scale = picker_ui_scale(app_frame.size);
    let divider = 42.0 * ui_scale;

    let header_frame = CGRect {
        origin: CGPoint {
            x: 22.0 * ui_scale,
            y: 12.0 * ui_scale,
        },
        size: CGSize {
            width: main_frame.size.width - 44.0 * ui_scale,
            height: 30.0 * ui_scale,
        },
    };
    let header: id = msg_class![env; UILabel alloc];
    let header: id = msg![env; header initWithFrame:header_frame];
    let header_text = ns_string::get_static_str(env, "Settings");
    () = msg![env; header setText:header_text];
    () = msg![env; header setTextAlignment:UITextAlignmentLeft];
    let header_font = picker_font(env, 21.0 * ui_scale);
    () = msg![env; header setFont:header_font];
    let black: id = msg_class![env; UIColor blackColor];
    () = msg![env; header setTextColor:black];
    () = msg![env; main_view addSubview:header];

    let subtitle_frame = CGRect {
        origin: CGPoint {
            x: 22.0 * ui_scale,
            y: 42.0 * ui_scale,
        },
        size: CGSize {
            width: main_frame.size.width - 44.0 * ui_scale,
            height: 20.0 * ui_scale,
        },
    };
    let subtitle: id = msg_class![env; UILabel alloc];
    let subtitle: id = msg![env; subtitle initWithFrame:subtitle_frame];
    let subtitle_text = ns_string::get_static_str(env, "Scroll for more options");
    () = msg![env; subtitle setText:subtitle_text];
    () = msg![env; subtitle setTextAlignment:UITextAlignmentLeft];
    let subtitle_font = picker_font(env, 11.0 * ui_scale);
    () = msg![env; subtitle setFont:subtitle_font];
    let muted: id = msg_class![env; UIColor darkGrayColor];
    () = msg![env; subtitle setTextColor:muted];
    () = msg![env; main_view addSubview:subtitle];

    // Close button (×) in the upper right corner. It uses an explicit border
    // and a slightly larger frame than the title so the glyph is clearly
    // visible against the white menu background.
    {
        let ui_scale = picker_ui_scale(main_frame.size);
        let button_size: CGFloat = 30.0 * ui_scale;
        let button_margin: CGFloat = 6.0 * ui_scale;
        let button_frame = CGRect {
            origin: CGPoint {
                x: main_frame.size.width - button_size - button_margin,
                y: button_margin,
            },
            size: CGSize {
                width: button_size,
                height: button_size,
            },
        };

        let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeRoundedRect];
        let text = ns_string::get_static_str(env, "×");
        () = msg![env; button setTitle:text forState:UIControlStateNormal];
        () = msg![env; button setFrame:button_frame];
        // FIXME: manually calling layoutSubviews shouldn't be needed?
        () = msg![env; button layoutSubviews];

        let label: id = msg![env; button titleLabel];
        let scaled_font_size = 23.0 * ui_scale;
        let font: id = picker_font(env, scaled_font_size);
        () = msg![env; label setFont:font];

        // `buttonWithType:UIButtonTypeRoundedRect` does not actually apply the
        // rounded-rect appearance, so explicitly give the close button a
        // visible background, title color and rounded border. Without this
        // the white default title on a clear background would be invisible
        // against the white menu.
        let bg_color: id = msg_class![env; UIColor grayColor];
        () = msg![env; button setBackgroundColor:bg_color];
        let text_color: id = msg_class![env; UIColor whiteColor];
        () = msg![env; button setTitleColor:text_color forState:UIControlStateNormal];
        let layer: id = msg![env; button layer];
        () = msg![env; layer setCornerRadius:(8.0 as CGFloat)];

        let selector = env.objc.lookup_selector("quickOptionsHide").unwrap();
        () = msg![env; button addTarget:delegate
                                 action:selector
                       forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; main_view addSubview:button];
    }

    enum RowKind {
        Label(&'static str),
        Buttons(&'static [(&'static str, &'static str)]),
        /// Dropdown listing every selectable device model.
        DeviceDropdown,
        /// Compact dropdown for the emulated iOS version.
        IosVersionDropdown,
        GraphicsApiDropdown,
        Switch(&'static str, bool),
    }
    let rows = [
        RowKind::Label("iOS version"),
        RowKind::IosVersionDropdown,
        RowKind::Label("Graphics API"),
        RowKind::GraphicsApiDropdown,
        RowKind::Label("Game folder"),
        RowKind::Buttons(&[
            ("Open folder", "openFileManager"),
            ("Refresh", "refreshApps"),
        ]),
        RowKind::Label("Scale hack"),
        RowKind::Buttons(&[
            ("Default", "scaleHackDefault"),
            ("Off", "scaleHack1"),
            ("0.50×", "scaleHackHalf"),
            ("0.75×", "scaleHackThreeQuarters"),
            ("2×", "scaleHack2"),
            ("3×", "scaleHack3"),
            ("4×", "scaleHack4"),
        ]),
        RowKind::Label("Orientation"),
        RowKind::Buttons(&[
            ("Default", "orientationDefault"),
            ("←", "orientationLandscapeLeft"),
            ("→", "orientationLandscapeRight"),
            ("↓", "orientationPortraitUpsideDown"),
        ]),
        RowKind::Label("Device model"),
        RowKind::DeviceDropdown,
        RowKind::Label("Network access"),
        RowKind::Switch("network:", false),
        RowKind::Label("ANGLE driver"),
        RowKind::Switch("angleDriver:", false),
        RowKind::Label("Enable log file"),
        RowKind::Switch("logFile:", true),
        RowKind::Label("Fast memory"),
        RowKind::Switch("fastMemory:", true),
        RowKind::Label("Force 32-bit"),
        RowKind::Switch("force32Bit:", false),
        RowKind::Label("Frame pacing"),
        RowKind::Switch("framePacing:", true),
        RowKind::Label("Show FPS"),
        RowKind::Switch("showFPS:", true),
        RowKind::Label("Use analog sticks for tilt controls"),
        RowKind::Switch("analogStickTiltControls:", true),
        // ---- (divider for stuff skipped below)
        RowKind::Label("Fullscreen (override)"),
        RowKind::Switch("fullscreen:", false),
    ];
    let rows = if crate::window::Window::rotatable_fullscreen() {
        // Fullscreen option doesn't make sense on always-fullscreen platforms
        &rows[..rows.len() - 2]
    } else {
        &rows[..]
    };

    let mut button_rows = Vec::new();
    let mut ios_version_btn: id = nil;
    let mut ios_version_menu: id = nil;
    let mut ios_version_items: Vec<id> = Vec::new();
    let mut graphics_api_btn: id = nil;
    let mut graphics_api_menu: id = nil;
    let mut graphics_api_items: Vec<id> = Vec::new();
    let mut device_model_btn: id = nil;
    let mut device_model_menu: id = nil;
    let mut device_model_items: Vec<id> = Vec::new();
    let mut device_model_thumb: id = nil;
    for (i, row) in rows.iter().enumerate() {
        let row_center = divider + ((1 + i / 2) as CGFloat) * 78.0 * ui_scale;

        match *row {
            RowKind::Label(text) => {
                let frame = CGRect {
                    origin: CGPoint {
                        x: 22.0 * ui_scale,
                        y: row_center - (28.0 * ui_scale) / 2.0,
                    },
                    size: CGSize {
                        width: main_frame.size.width * 0.36,
                        height: 28.0 * ui_scale,
                    },
                };

                let label: id = msg_class![env; UILabel alloc];
                let label: id = msg![env; label initWithFrame:frame];
                let text = ns_string::get_static_str(env, text);
                () = msg![env; label setText:text];
                () = msg![env; label setTextAlignment:UITextAlignmentLeft];
                let label_font = picker_font(env, 14.0 * ui_scale);
                () = msg![env; label setFont:label_font];
                let black: id = msg_class![env; UIColor blackColor];
                () = msg![env; label setTextColor:black];
                () = msg![env; label setAdjustsFontSizeToFitWidth:true];
                () = msg![env; label setMinimumFontSize:9.0];
                () = msg![env; main_view addSubview:label];
            }
            RowKind::Buttons(buttons) => {
                let controls = make_button_row(
                    env,
                    delegate,
                    main_view,
                    main_frame.size,
                    row_center,
                    buttons,
                    /* font_size: */ Some(10.0),
                );
                let margin = 6.0 * ui_scale;
                let controls_width = main_frame.size.width * 0.56;
                let controls_x = main_frame.size.width * 0.42;
                let button_width = (controls_width - margin * (controls.len() as CGFloat + 1.0))
                    / controls.len() as CGFloat;
                for (index, &button) in controls.iter().enumerate() {
                    let button_frame = CGRect {
                        origin: CGPoint {
                            x: controls_x + margin + index as CGFloat * (button_width + margin),
                            y: row_center - 15.0 * ui_scale,
                        },
                        size: CGSize {
                            width: button_width,
                            height: 30.0 * ui_scale,
                        },
                    };
                    () = msg![env; button setFrame:button_frame];
                    () = msg![env; button layoutSubviews];
                }
                button_rows.push(controls);
            }
            RowKind::IosVersionDropdown => {
                let dropdown = make_ios_version_dropdown(
                    env,
                    delegate,
                    main_view,
                    main_frame.size,
                    row_center,
                );
                ios_version_btn = dropdown.0;
                ios_version_menu = dropdown.1;
                ios_version_items = dropdown.2;
            }
            RowKind::DeviceDropdown => {
                let dropdown = make_device_model_dropdown(
                    env,
                    delegate,
                    main_view,
                    main_frame.size,
                    row_center,
                );
                device_model_btn = dropdown.0;
                device_model_menu = dropdown.1;
                device_model_items = dropdown.2;
                device_model_thumb = dropdown.3;
            }
            RowKind::GraphicsApiDropdown => {
                let dropdown = make_graphics_api_dropdown(env, delegate, main_view, main_frame.size, row_center);
                graphics_api_btn = dropdown.0;
                graphics_api_menu = dropdown.1;
                graphics_api_items = dropdown.2;
            }
            RowKind::Switch(selector, default_state) => {
                let switch_frame = CGRect {
                    origin: CGPoint {
                        x: main_frame.size.width * 0.70,
                        y: row_center - (30.0 * ui_scale) / 2.0,
                    },
                    size: CGSize {
                        width: 94.0 * ui_scale,
                        height: 30.0 * ui_scale,
                    },
                };

                let switch: id = msg_class![env; UISwitch alloc];
                let switch: id = msg![env; switch initWithFrame:switch_frame];
                () = msg![env; switch setOn:default_state];
                let selector = env.objc.lookup_selector(selector).unwrap();
                () = msg![env; switch addTarget:delegate
                                         action:selector
                               forControlEvents:UIControlEventValueChanged];
                () = msg![env; main_view addSubview:switch];
            }
        }
    }

    QuickOptionsStuff {
        main_view,
        ios_version_btn,
        ios_version_menu,
        ios_version_items,
        graphics_api_btn,
        graphics_api_menu,
        graphics_api_items,
        scale_hack_buttons: button_rows[1][..].try_into().unwrap(),
        orientation_buttons: button_rows[2][..].try_into().unwrap(),
        device_model_btn,
        device_model_menu,
        device_model_items,
        device_model_thumb,
    }
}

/// Re-lay-out and re-style the device-model dropdown list for the given scroll
/// offset and current selection. Items are positioned relative to `scroll`
/// (each row is `DEVICE_MENU_ITEM_HEIGHT` tall); rows outside the visible
/// window are hidden. The currently-selected item is highlighted in magenta,
/// the rest in dark gray. The scrollbar thumb is moved to reflect `scroll`.
fn update_device_model_menu(
    env: &mut Environment,
    items: &[id],
    thumb: id,
    selected: Option<i32>,
    scroll: isize,
) {
    let thumb_frame: CGRect = msg![env; thumb frame];
    let list_width = thumb_frame.origin.x;
    let scrollbar_width = thumb_frame.size.width;
    let thumb_height = thumb_frame.size.height;
    let row_height = DEVICE_MENU_ITEM_HEIGHT * (thumb_frame.size.width / 22.0).max(1.0);
    let visible_menu_height = (DEVICE_MENU_VISIBLE_ITEMS as CGFloat) * row_height;
    let max_scroll = (items.len() as isize).saturating_sub(DEVICE_MENU_VISIBLE_ITEMS as isize);

    for (j, &item) in items.iter().enumerate() {
        let y_pos = ((j as isize - scroll) as CGFloat) * row_height;
        let is_visible = y_pos >= 0.0 && y_pos < visible_menu_height;
        () = msg![env; item setHidden:(!is_visible)];
        if is_visible {
            let item_frame = CGRect {
                origin: CGPoint { x: 0.0, y: y_pos },
                size: CGSize {
                    width: list_width,
                    height: row_height,
                },
            };
            () = msg![env; item setFrame:item_frame];
        }
        let tag: NSInteger = msg![env; item tag];
        let is_selected = selected.is_some_and(|v| v as NSInteger == tag);
        let color: id = if is_selected {
            msg_class![env; UIColor magentaColor]
        } else {
            msg_class![env; UIColor darkGrayColor]
        };
        () = msg![env; item setBackgroundColor:color];
    }

    // Position the scrollbar thumb proportionally to the scroll offset.
    let travel = (visible_menu_height - thumb_height).max(0.0);
    let thumb_y = if max_scroll > 0 {
        (scroll as CGFloat / max_scroll as CGFloat) * travel
    } else {
        0.0
    };
    let thumb_frame = CGRect {
        origin: CGPoint {
            x: list_width,
            y: thumb_y,
        },
        size: CGSize {
            width: scrollbar_width,
            height: thumb_height,
        },
    };
    () = msg![env; thumb setFrame:thumb_frame];
}

/// Graphics API choices shown in the settings dropdown.
const GRAPHICS_API_ENTRIES: &[(&str, crate::options::GraphicsApi)] = &[
    ("Default (game)", crate::options::GraphicsApi::Default),
    (
        "OpenGL ES 1.1 → OpenGL ES 2.0 translator",
        crate::options::GraphicsApi::Translator,
    ),
    (
        "OpenGL ES 1.1 → OpenGL ES 3.0 translator",
        crate::options::GraphicsApi::TranslatorGLES30,
    ),
    ("Metal compatibility", crate::options::GraphicsApi::Metal),
];

fn update_graphics_api_dropdown(env: &mut Environment, button: id, items: &[id], value: crate::options::GraphicsApi) {
    for (index, &item) in items.iter().enumerate() {
        let color: id = if GRAPHICS_API_ENTRIES[index].1 == value {
            msg_class![env; UIColor magentaColor]
        } else {
            msg_class![env; UIColor darkGrayColor]
        };
        () = msg![env; item setBackgroundColor:color];
    }
    let title = ns_string::get_static_str(env, value.label());
    () = msg![env; button setTitle:title forState:UIControlStateNormal];
    () = msg![env; button layoutSubviews];
}

fn make_graphics_api_dropdown(env: &mut Environment, delegate: id, super_view: id, super_view_size: CGSize, row_center: CGFloat) -> (id, id, Vec<id>) {
    let ui_scale = picker_ui_scale(super_view_size);
    let width = (super_view_size.width * 0.56).clamp(170.0, 720.0);
    let height = 30.0 * ui_scale;
    let frame = CGRect { origin: CGPoint { x: super_view_size.width * 0.42, y: row_center - height / 2.0 }, size: CGSize { width, height } };
    let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    let title = ns_string::get_static_str(env, "Default (game)");
    () = msg![env; button setTitle:title forState:UIControlStateNormal];
    release(env, title);
    let button_label: id = msg![env; button titleLabel];
    let button_font = picker_font(env, 13.0 * ui_scale);
    () = msg![env; button_label setFont:button_font];
    () = msg![env; button_label setAdjustsFontSizeToFitWidth:true];
    () = msg![env; button_label setMinimumFontSize:8.0];
    let white: id = msg_class![env; UIColor whiteColor];
    let gray: id = msg_class![env; UIColor darkGrayColor];
    () = msg![env; button setTitleColor:white forState:UIControlStateNormal];
    () = msg![env; button setBackgroundColor:gray];
    () = msg![env; button setFrame:frame];
    () = msg![env; button layoutSubviews];
    let toggle = env.objc.lookup_selector("graphicsApiToggle").unwrap();
    () = msg![env; button addTarget:delegate action:toggle forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; super_view addSubview:button];
    let menu: id = msg_class![env; UIView alloc];
    let menu: id = msg![env; menu initWithFrame:(CGRect { origin: CGPoint { x: frame.origin.x, y: (frame.origin.y - height * GRAPHICS_API_ENTRIES.len() as CGFloat).max(0.0) }, size: CGSize { width, height: height * GRAPHICS_API_ENTRIES.len() as CGFloat } })];
    () = msg![env; menu setBackgroundColor:gray];
    () = msg![env; menu setClipsToBounds:true];
    () = msg![env; menu setHidden:true];
    () = msg![env; super_view addSubview:menu];
    let selector = env.objc.lookup_selector("graphicsApi:").unwrap();
    let mut items = Vec::new();
    for (index, (label, _)) in GRAPHICS_API_ENTRIES.iter().enumerate() {
        let item: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        let text = ns_string::get_static_str(env, label);
        () = msg![env; item setTitle:text forState:UIControlStateNormal];
        release(env, text);
        let item_label: id = msg![env; item titleLabel];
        let item_font = picker_font(env, 12.0 * ui_scale);
        () = msg![env; item_label setFont:item_font];
        () = msg![env; item setTitleColor:white forState:UIControlStateNormal];
        () = msg![env; item setBackgroundColor:gray];
        () = msg![env; item setFrame:(CGRect { origin: CGPoint { x: 0.0, y: index as CGFloat * height }, size: CGSize { width, height } })];
        () = msg![env; item layoutSubviews];
        () = msg![env; item setTag:(index as NSInteger)];
        () = msg![env; item addTarget:delegate action:selector forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; menu addSubview:item];
        items.push(item);
    }
    (button, menu, items)
}

fn make_ios_version_dropdown(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    super_view_size: CGSize,
    row_center: CGFloat,
) -> (id, id, Vec<id>) {
    let ui_scale = picker_ui_scale(super_view_size);
    let button_width: CGFloat = (super_view_size.width * 0.56).clamp(170.0, 720.0);
    let button_height: CGFloat = 30.0 * ui_scale;
    let item_height: CGFloat = 30.0 * ui_scale;
    let button_frame = CGRect {
        origin: CGPoint {
            x: super_view_size.width * 0.42,
            y: row_center - button_height / 2.0,
        },
        size: CGSize { width: button_width, height: button_height },
    };
    let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    let title = ns_string::get_static_str(env, "iOS version");
    () = msg![env; button setTitle:title forState:UIControlStateNormal];
    release(env, title);
    let button_label: id = msg![env; button titleLabel];
    let button_font = picker_font(env, 13.0 * ui_scale);
    () = msg![env; button_label setFont:button_font];
    let white: id = msg_class![env; UIColor whiteColor];
    let dark_gray: id = msg_class![env; UIColor darkGrayColor];
    let magenta: id = msg_class![env; UIColor magentaColor];
    () = msg![env; button setTitleColor:white forState:UIControlStateNormal];
    () = msg![env; button setBackgroundColor:dark_gray];
    () = msg![env; button setFrame:button_frame];
    () = msg![env; button layoutSubviews];
    let button_layer: id = msg![env; button layer];
    () = msg![env; button_layer setCornerRadius:(6.0 as CGFloat)];
    let toggle_selector = env.objc.lookup_selector("iosVersionToggle").unwrap();
    () = msg![env; button addTarget:delegate action:toggle_selector forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; super_view addSubview:button];

    let menu: id = msg_class![env; UIView alloc];
    let menu: id = msg![env; menu initWithFrame:(CGRect {
        origin: CGPoint {
            x: button_frame.origin.x,
            y: (button_frame.origin.y - item_height * IOS_VERSION_ENTRIES.len() as CGFloat).max(0.0),
        },
        size: CGSize {
            width: button_width,
            height: item_height * IOS_VERSION_ENTRIES.len() as CGFloat,
        },
    })];
    () = msg![env; menu setBackgroundColor:dark_gray];
    () = msg![env; menu setClipsToBounds:true];
    let menu_layer: id = msg![env; menu layer];
    () = msg![env; menu_layer setCornerRadius:(6.0 as CGFloat)];
    () = msg![env; menu setHidden:true];
    () = msg![env; super_view addSubview:menu];

    let entries = IOS_VERSION_ENTRIES;
    let mut items = Vec::new();
    for (index, (label, tag)) in entries.iter().enumerate() {
        let item: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        let text = ns_string::from_rust_string(env, (*label).to_owned());
        () = msg![env; item setTitle:text forState:UIControlStateNormal];
        release(env, text);
        let item_label: id = msg![env; item titleLabel];
        let item_font = picker_font(env, 12.0 * ui_scale);
        () = msg![env; item_label setFont:item_font];
        let item_text_color: id = msg_class![env; UIColor whiteColor];
        () = msg![env; item setTitleColor:item_text_color forState:UIControlStateNormal];
        let item_color: id = if *tag == 0 { magenta } else { dark_gray };
        () = msg![env; item setBackgroundColor:item_color];
        () = msg![env; item setFrame:(CGRect {
            origin: CGPoint { x: 0.0, y: index as CGFloat * item_height },
            size: CGSize { width: button_width, height: item_height },
        })];
        () = msg![env; item layoutSubviews];
        let tag: NSInteger = *tag as NSInteger;
        () = msg![env; item setTag:tag];
        let selector = env.objc.lookup_selector("iosVersion:").unwrap();
        () = msg![env; item addTarget:delegate action:selector forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; menu addSubview:item];
        items.push(item);
    }
    (button, menu, items)
}

fn make_device_model_dropdown(
    env: &mut Environment,
    delegate: id,
    super_view: id,
    super_view_size: CGSize,
    row_center: CGFloat,
) -> (id, id, Vec<id>, id) {
    let ui_scale = picker_ui_scale(super_view_size);
    let btn_width: CGFloat = (super_view_size.width * 0.56).clamp(170.0, 720.0);
    let btn_height: CGFloat = 30.0 * ui_scale;
    let scrollbar_width: CGFloat = 22.0 * ui_scale;
    let list_width: CGFloat = btn_width - scrollbar_width;

    let btn_frame = CGRect {
        origin: CGPoint {
            x: super_view_size.width * 0.42,
            y: row_center - btn_height / 2.0,
        },
        size: CGSize {
            width: btn_width,
            height: btn_height,
        },
    };

    let dark_gray: id = msg_class![env; UIColor darkGrayColor];

    // Bordered container for the toggle button (a darker frame behind a lighter
    // inner button), so it reads as a control on the white menu background.
    let border_view: id = msg_class![env; UIView alloc];
    let border_view: id = msg![env; border_view initWithFrame:btn_frame];
    () = msg![env; border_view setBackgroundColor:dark_gray];
    () = msg![env; super_view addSubview:border_view];

    let inner_frame = CGRect {
        origin: CGPoint { x: 2.0, y: 2.0 },
        size: CGSize {
            width: btn_frame.size.width - 4.0,
            height: btn_frame.size.height - 4.0,
        },
    };
    let button: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    let initial_title = format!("{} ^", device_model_label_for_tag(None));
    let text = ns_string::from_rust_string(env, initial_title);
    () = msg![env; button setTitle:text forState:UIControlStateNormal];
    release(env, text);
    let button_label: id = msg![env; button titleLabel];
    let button_font = picker_font(env, 13.0 * ui_scale);
    () = msg![env; button_label setFont:button_font];
    let black: id = msg_class![env; UIColor blackColor];
    () = msg![env; button setTitleColor:black forState:UIControlStateNormal];
    let light_gray: id = msg_class![env; UIColor lightGrayColor];
    () = msg![env; button setBackgroundColor:light_gray];
    () = msg![env; button setFrame:inner_frame];
    () = msg![env; button layoutSubviews];
    let toggle_selector = env.objc.lookup_selector("deviceModelToggle").unwrap();
    () = msg![env; button addTarget:delegate
                             action:toggle_selector
                   forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; border_view addSubview:button];

    // The dropdown menu, placed directly above the toggle button. It is clipped
    // to its own bounds and hidden until the button is tapped.
    let row_height = DEVICE_MENU_ITEM_HEIGHT * ui_scale;
    let visible_menu_height = (DEVICE_MENU_VISIBLE_ITEMS as CGFloat) * row_height;
    let menu_frame = CGRect {
        origin: CGPoint {
            x: btn_frame.origin.x,
            y: (btn_frame.origin.y - visible_menu_height).max(0.0),
        },
        size: CGSize {
            width: btn_width,
            height: visible_menu_height,
        },
    };
    let menu_view: id = msg_class![env; UIView alloc];
    let menu_view: id = msg![env; menu_view initWithFrame:menu_frame];
    () = msg![env; menu_view setBackgroundColor:dark_gray];
    () = msg![env; menu_view setClipsToBounds:true];
    () = msg![env; menu_view setHidden:true];
    () = msg![env; super_view addSubview:menu_view];

    // List items: one button per choice. Items that fall outside the initially
    // visible window are hidden; scrolling reveals them (see
    // `update_device_model_menu`).
    let entries = device_model_entries();
    let item_selector = env.objc.lookup_selector("deviceModel:").unwrap();
    let white: id = msg_class![env; UIColor whiteColor];
    let mut items: Vec<id> = Vec::new();
    for (j, (title, tag)) in entries.into_iter().enumerate() {
        let y_pos = (j as CGFloat) * row_height;
        let item_frame = CGRect {
            origin: CGPoint { x: 0.0, y: y_pos },
            size: CGSize {
                width: list_width,
                height: row_height,
            },
        };
        let item_btn: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
        let text = ns_string::from_rust_string(env, title);
        () = msg![env; item_btn setTitle:text forState:UIControlStateNormal];
        release(env, text);
        let item_label: id = msg![env; item_btn titleLabel];
        let item_font = picker_font(env, 12.0 * ui_scale);
        () = msg![env; item_label setFont:item_font];
        () = msg![env; item_label setAdjustsFontSizeToFitWidth:true];
        () = msg![env; item_label setMinimumFontSize:8.0];
        () = msg![env; item_btn setTitleColor:white forState:UIControlStateNormal];
        () = msg![env; item_btn setFrame:item_frame];
        () = msg![env; item_btn layoutSubviews];
        let tag: NSInteger = tag as NSInteger;
        () = msg![env; item_btn setTag:tag];
        if y_pos >= visible_menu_height {
            () = msg![env; item_btn setHidden:true];
        }
        () = msg![env; item_btn addTarget:delegate
                                   action:item_selector
                         forControlEvents:UIControlEventTouchUpInside];
        () = msg![env; menu_view addSubview:item_btn];
        items.push(item_btn);
    }

    // Scrollbar track (full height) and thumb.
    let track_view: id = msg_class![env; UIView alloc];
    let track_frame = CGRect {
        origin: CGPoint { x: list_width, y: 0.0 },
        size: CGSize {
            width: scrollbar_width,
            height: visible_menu_height,
        },
    };
    let track_view: id = msg![env; track_view initWithFrame:track_frame];
    let black: id = msg_class![env; UIColor blackColor];
    () = msg![env; track_view setBackgroundColor:black];
    () = msg![env; menu_view addSubview:track_view];

    let thumb_view: id = msg_class![env; UIView alloc];
    let thumb_frame = CGRect {
        origin: CGPoint { x: list_width, y: 0.0 },
        size: CGSize {
            width: scrollbar_width,
            height: (54.0 * ui_scale).min(visible_menu_height),
        },
    };
    let thumb_view: id = msg![env; thumb_view initWithFrame:thumb_frame];
    let light_gray: id = msg_class![env; UIColor lightGrayColor];
    () = msg![env; thumb_view setBackgroundColor:light_gray];
    () = msg![env; menu_view addSubview:thumb_view];

    // Transparent up/down halves over the scrollbar that scroll the list.
    let clear: id = msg_class![env; UIColor clearColor];
    let up_btn: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    let up_frame = CGRect {
        origin: CGPoint { x: list_width, y: 0.0 },
        size: CGSize {
            width: scrollbar_width,
            height: visible_menu_height / 2.0,
        },
    };
    () = msg![env; up_btn setFrame:up_frame];
    () = msg![env; up_btn setBackgroundColor:clear];
    () = msg![env; up_btn addTarget:delegate
                             action:(env.objc.lookup_selector("deviceModelScrollUp").unwrap())
                   forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; menu_view addSubview:up_btn];

    let down_btn: id = msg_class![env; UIButton buttonWithType:UIButtonTypeCustom];
    let down_frame = CGRect {
        origin: CGPoint {
            x: list_width,
            y: visible_menu_height / 2.0,
        },
        size: CGSize {
            width: scrollbar_width,
            height: visible_menu_height / 2.0,
        },
    };
    () = msg![env; down_btn setFrame:down_frame];
    () = msg![env; down_btn setBackgroundColor:clear];
    () = msg![env; down_btn addTarget:delegate
                               action:(env.objc.lookup_selector("deviceModelScrollDown").unwrap())
                     forControlEvents:UIControlEventTouchUpInside];
    () = msg![env; menu_view addSubview:down_btn];

    (button, menu_view, items, thumb_view)
}
