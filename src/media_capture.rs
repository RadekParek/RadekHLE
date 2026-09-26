use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct CameraFrame {
    pub width: u32,
    pub height: u32,
    pub timestamp: u64,
    pub rgba: Vec<u8>,
}

static LAST_CAMERA_TIMESTAMP: AtomicU64 = AtomicU64::new(0);
static MICROPHONE_OFFSET: AtomicU64 = AtomicU64::new(16);
static LAST_CAPTURE_LOG: OnceLock<Mutex<Option<SystemTime>>> = OnceLock::new();

fn capture_directory() -> PathBuf {
    #[cfg(target_os = "android")]
    unsafe {
        extern "C" {
            fn SDL_AndroidGetInternalStoragePath() -> *const std::ffi::c_char;
        }
        let path = SDL_AndroidGetInternalStoragePath();
        if !path.is_null() {
            if let Ok(path) = std::ffi::CStr::from_ptr(path).to_str() {
                return PathBuf::from(path).join("metalhle_capture");
            }
        }
    }
    PathBuf::from("metalhle_capture")
}

fn log_capture_once(message: &str) {
    let state = LAST_CAPTURE_LOG.get_or_init(|| Mutex::new(None));
    let Ok(mut last) = state.lock() else {
        return;
    };
    let now = SystemTime::now();
    if last
        .and_then(|time| now.duration_since(time).ok())
        .is_some_and(|elapsed| elapsed < Duration::from_secs(10))
    {
        return;
    }
    *last = Some(now);
    log!("{}", message);
}

pub fn camera_available() -> bool {
    std::fs::read_to_string(capture_directory().join("status"))
        .map(|status| status.lines().any(|line| line == "camera=1"))
        .unwrap_or(false)
}

pub fn microphone_available() -> bool {
    std::fs::read_to_string(capture_directory().join("status"))
        .map(|status| status.lines().any(|line| line == "microphone=1"))
        .unwrap_or(false)
}
pub fn log_native_capture_status() {
    let status = std::fs::read_to_string(capture_directory().join("status"))
        .unwrap_or_else(|_| "camera=0\nmicrophone=0\nstorage=unknown\n".to_owned());
    let camera = status
        .lines()
        .find_map(|line| line.strip_prefix("camera="))
        .unwrap_or("0");
    let microphone = status
        .lines()
        .find_map(|line| line.strip_prefix("microphone="))
        .unwrap_or("0");
    let storage = status
        .lines()
        .find_map(|line| line.strip_prefix("storage="))
        .unwrap_or("unknown");
    log!(
        "Native Android device capture status: camera={} microphone={} storage={}",
        camera,
        microphone,
        storage
    );
}

pub fn take_camera_frame() -> Option<CameraFrame> {
    let path = capture_directory().join("camera.nv21");
    let mut file = File::open(path).ok()?;
    let mut header = [0u8; 24];
    file.read_exact(&mut header).ok()?;
    if u32::from_le_bytes(header[0..4].try_into().ok()?) != 0x5248_4346 {
        log_capture_once("Native camera capture file has an invalid header; ignoring it");
        return None;
    }
    let width = u32::from_le_bytes(header[4..8].try_into().ok()?);
    let height = u32::from_le_bytes(header[8..12].try_into().ok()?);
    let timestamp = u64::from_le_bytes(header[12..20].try_into().ok()?);
    let payload_len = u32::from_le_bytes(header[20..24].try_into().ok()?) as usize;
    let expected_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(3))
        .map(|bytes| bytes / 2)?;
    if width == 0 || height == 0 || payload_len != expected_len || payload_len > 16 * 1024 * 1024 {
        log_capture_once("Native camera capture file has invalid dimensions; ignoring it");
        return None;
    }
    if LAST_CAMERA_TIMESTAMP.load(Ordering::Acquire) == timestamp {
        return None;
    }
    let mut nv21 = vec![0u8; payload_len];
    file.read_exact(&mut nv21).ok()?;
    let rgba = nv21_to_rgba(width, height, &nv21);
    LAST_CAMERA_TIMESTAMP.store(timestamp, Ordering::Release);
    Some(CameraFrame {
        width,
        height,
        timestamp,
        rgba,
    })
}

fn nv21_to_rgba(width: u32, height: u32, nv21: &[u8]) -> Vec<u8> {
    let width = width as usize;
    let height = height as usize;
    let frame_size = width * height;
    let mut rgba = vec![0u8; frame_size * 4];
    for y in 0..height {
        let uv_row = (y / 2) * width;
        for x in 0..width {
            let y_value = nv21[y * width + x] as i32;
            let uv_index = frame_size + uv_row + (x & !1);
            let v = nv21.get(uv_index).copied().unwrap_or(128) as i32 - 128;
            let u = nv21.get(uv_index + 1).copied().unwrap_or(128) as i32 - 128;
            let c = (y_value - 16).max(0);
            let r = (298 * c + 409 * v + 128) / 256;
            let g = (298 * c - 100 * u - 208 * v + 128) / 256;
            let b = (298 * c + 516 * u + 128) / 256;
            let offset = (y * width + x) * 4;
            rgba[offset] = b.clamp(0, 255) as u8;
            rgba[offset + 1] = g.clamp(0, 255) as u8;
            rgba[offset + 2] = r.clamp(0, 255) as u8;
            rgba[offset + 3] = 255;
        }
    }
    rgba
}

pub fn take_microphone_pcm(max_bytes: usize) -> Vec<u8> {
    let path = capture_directory().join("microphone.pcm");
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let mut offset = MICROPHONE_OFFSET.load(Ordering::Acquire);
    let file_length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if offset > file_length || file_length < 16 {
        offset = 16;
        MICROPHONE_OFFSET.store(offset, Ordering::Release);
    }
    let offset = MICROPHONE_OFFSET.load(Ordering::Acquire);
    if file.seek(SeekFrom::Start(offset)).is_err() {
        return Vec::new();
    }
    let mut bytes = vec![0u8; max_bytes.max(2)];
    let count = file.read(&mut bytes).unwrap_or(0);
    bytes.truncate(count);
    MICROPHONE_OFFSET.fetch_add(count as u64, Ordering::AcqRel);
    bytes
}

pub fn reset_microphone_cursor() {
    MICROPHONE_OFFSET.store(16, Ordering::Release);
}

pub fn native_capture_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
