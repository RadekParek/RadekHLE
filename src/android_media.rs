/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Bridge from the emulated camera/mic stack (`AVCaptureDevice`,
//! `UIImagePickerController`, `AVAudioRecorder`, RemoteIO input) to *real*
//! host hardware on Android.
//!
//! All calls go through hand-rolled JNI (no `jni` crate dependency) into
//! static methods on `org.touchhle.android.HostMedia` — see that file for
//! the Android side (Camera2 still capture, AudioRecord streaming).
//!
//! On other platforms (or when the permission is denied / hardware absent)
//! every function here returns the "not available" answer, so emulated apps
//! see an honest "no camera / no mic" device instead of a fake one.

#[cfg(target_os = "android")]
mod imp {
    use std::ffi::CString;
    use std::os::raw::{c_char, c_int, c_void};

    extern "C" {
        // Exported by libSDL2.so on Android (SDL_system.h).
        fn SDL_AndroidGetJNIEnv() -> *mut c_void;
    }

    /// JNI function-table slot indices. The table starts with 4 reserved
    /// pointers, hence every index is `4 + <ordinal in jni.h>`.
    mod slots {
        pub const FIND_CLASS: usize = 6;
        pub const EXCEPTION_OCCURRED: usize = 15;
        pub const EXCEPTION_CLEAR: usize = 17;
        pub const DELETE_LOCAL_REF: usize = 23;
        pub const GET_STATIC_METHOD_ID: usize = 113;
        pub const CALL_STATIC_OBJECT_METHOD_A: usize = 116;
        pub const CALL_STATIC_BOOLEAN_METHOD_A: usize = 119;
        pub const CALL_STATIC_VOID_METHOD_A: usize = 143;
        pub const NEW_STRING_UTF: usize = 167;
        pub const GET_STRING_UTF_CHARS: usize = 169;
        pub const RELEASE_STRING_UTF_CHARS: usize = 170;
        pub const GET_ARRAY_LENGTH: usize = 171;
        pub const NEW_BYTE_ARRAY: usize = 176;
        pub const GET_BYTE_ARRAY_ELEMENTS: usize = 184;
        pub const RELEASE_BYTE_ARRAY_ELEMENTS: usize = 192;
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    union JValue {
        l: *mut c_void,
        i: c_int,
        z: u8,
        _pad: u64,
    }

    struct Jni {
        env: *mut c_void,
    }

    impl Jni {
        fn attach() -> Option<Jni> {
            unsafe {
                let env = SDL_AndroidGetJNIEnv();
                if env.is_null() {
                    return None;
                }
                Some(Jni { env })
            }
        }

        fn slot<F>(&self, index: usize) -> F {
            unsafe {
                let table = self.env as *mut *mut c_void;
                std::mem::transmute_copy::<*mut c_void, F>(&*table.add(index))
            }
        }

        fn exception_pending(&self) -> bool {
            let f: unsafe extern "C" fn(*mut c_void) -> *mut c_void =
                self.slot(slots::EXCEPTION_OCCURRED);
            !unsafe { f(self.env) }.is_null()
        }

        fn clear_exception(&self) {
            let f: unsafe extern "C" fn(*mut c_void) = self.slot(slots::EXCEPTION_CLEAR);
            unsafe { f(self.env) }
        }

        fn find_host_media_class(&self) -> Option<*mut c_void> {
            let name = CString::new("org/touchhle/android/HostMedia").ok()?;
            let f: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void =
                self.slot(slots::FIND_CLASS);
            let class = unsafe { f(self.env, name.as_ptr()) };
            if class.is_null() {
                self.clear_exception();
                return None;
            }
            Some(class)
        }

        fn get_static_method(
            &self,
            class: *mut c_void,
            name: &str,
            sig: &str,
        ) -> Option<*mut c_void> {
            let name = CString::new(name).ok()?;
            let sig = CString::new(sig).ok()?;
            let f: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *const c_char,
                *const c_char,
            ) -> *mut c_void = self.slot(slots::GET_STATIC_METHOD_ID);
            let method = unsafe { f(self.env, class, name.as_ptr(), sig.as_ptr()) };
            if method.is_null() {
                self.clear_exception();
                return None;
            }
            Some(method)
        }

        fn call_static_bool(
            &self,
            class: *mut c_void,
            method: *mut c_void,
            args: &[JValue],
        ) -> bool {
            let f: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *const JValue,
            ) -> u8 = self.slot(slots::CALL_STATIC_BOOLEAN_METHOD_A);
            let r = unsafe { f(self.env, class, method, args.as_ptr()) };
            if self.exception_pending() {
                self.clear_exception();
            }
            r != 0
        }

        fn call_static_void(&self, class: *mut c_void, method: *mut c_void, args: &[JValue]) {
            let f: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *const JValue,
            ) = self.slot(slots::CALL_STATIC_VOID_METHOD_A);
            unsafe { f(self.env, class, method, args.as_ptr()) };
            if self.exception_pending() {
                self.clear_exception();
            }
        }

        fn call_static_object(
            &self,
            class: *mut c_void,
            method: *mut c_void,
            args: &[JValue],
        ) -> *mut c_void {
            let f: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut c_void,
                *const JValue,
            ) -> *mut c_void = self.slot(slots::CALL_STATIC_OBJECT_METHOD_A);
            let r = unsafe { f(self.env, class, method, args.as_ptr()) };
            if self.exception_pending() {
                self.clear_exception();
            }
            r
        }

        fn delete_local_ref(&self, obj: *mut c_void) {
            if obj.is_null() {
                return;
            }
            let f: unsafe extern "C" fn(*mut c_void, *mut c_void) =
                self.slot(slots::DELETE_LOCAL_REF);
            unsafe { f(self.env, obj) }
        }

        fn with_class<T>(&self, f: impl FnOnce(&Jni, *mut c_void) -> Option<T>) -> Option<T> {
            let class = self.find_host_media_class()?;
            let result = f(self, class);
            self.delete_local_ref(class);
            result
        }
    }

    fn bool_arg(v: bool) -> JValue {
        JValue {
            z: if v { 1 } else { 0 },
        }
    }

    pub fn has_camera(front: bool) -> bool {
        let Some(jni) = Jni::attach() else {
            return false;
        };
        jni.with_class(|jni, class| {
            let method = jni.get_static_method(class, "hasCamera", "(Z)Z")?;
            Some(jni.call_static_bool(class, method, &[bool_arg(front)]))
        })
        .unwrap_or(false)
    }

    /// Take a still photo; returns JPEG bytes or None.
    pub fn take_photo(front: bool) -> Option<Vec<u8>> {
        let jni = Jni::attach()?;
        jni.with_class(|jni, class| {
            let method = jni.get_static_method(class, "takePhoto", "(Z)[B")?;
            let arr = jni.call_static_object(class, method, &[bool_arg(front)]);
            if arr.is_null() {
                return None;
            }
            let len_f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int =
                jni.slot(slots::GET_ARRAY_LENGTH);
            let get_f: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut u8,
            ) -> *mut u8 = jni.slot(slots::GET_BYTE_ARRAY_ELEMENTS);
            let rel_f: unsafe extern "C" fn(
                *mut c_void,
                *mut c_void,
                *mut u8,
                c_int,
            ) = jni.slot(slots::RELEASE_BYTE_ARRAY_ELEMENTS);
            let len = unsafe { len_f(jni.env, arr) };
            if len <= 0 {
                jni.delete_local_ref(arr);
                return None;
            }
            let mut bytes = vec![0u8; len as usize];
            let ptr = unsafe { get_f(jni.env, arr, bytes.as_mut_ptr()) };
            if !ptr.is_null() && ptr != bytes.as_mut_ptr() {
                unsafe {
                    std::ptr::copy_nonoverlapping(ptr, bytes.as_mut_ptr(), len as usize);
                }
            }
            unsafe { rel_f(jni.env, arr, ptr, 0) };
            jni.delete_local_ref(arr);
            Some(bytes)
        })
    }

    pub fn has_microphone() -> bool {
        let Some(jni) = Jni::attach() else {
            return false;
        };
        jni.with_class(|jni, class| {
            let method = jni.get_static_method(class, "hasMicrophone", "()Z")?;
            Some(jni.call_static_bool(class, method, &[]))
        })
        .unwrap_or(false)
    }

    pub fn start_mic() -> bool {
        let Some(jni) = Jni::attach() else {
            return false;
        };
        jni.with_class(|jni, class| {
            let method = jni.get_static_method(class, "startMic", "()Z")?;
            Some(jni.call_static_bool(class, method, &[]))
        })
        .unwrap_or(false)
    }

    pub fn stop_mic() {
        if let Some(jni) = Jni::attach() {
            if let Some(class) = jni.find_host_media_class() {
                if let Some(method) = jni.get_static_method(class, "stopMic", "()V") {
                    jni.call_static_void(class, method, &[]);
                }
                jni.delete_local_ref(class);
            }
        }
    }

    /// The most recent mic chunk: mono 16-bit LE PCM samples.
    pub fn read_mic_chunk() -> Vec<i16> {
        let Some(jni) = Jni::attach() else {
            return Vec::new();
        };
        let Some(bytes) = jni.with_class(|jni, class| {
            let method = jni.get_static_method(class, "readMicChunk", "()[S")?;
            Some(jni.call_static_object(class, method, &[]))
        }) else {
            return Vec::new();
        };
        if bytes.is_null() {
            return Vec::new();
        }
        let len_f: unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int =
            jni.slot(slots::GET_ARRAY_LENGTH);
        // short[] elements via GetShortArrayElements (slot 186) — but we can
        // reuse the critical-free path: Get<Primitive>ArrayRegion for shorts
        // is slot 189. Read into a Vec<i16> directly.
        const GET_SHORT_ARRAY_REGION: usize = 202; // 4 + ordinal 198 (GetShortArrayRegion)
        let region_f: unsafe extern "C" fn(
            *mut c_void,
            *mut c_void,
            c_int,
            c_int,
            *mut i16,
        ) = jni.slot(GET_SHORT_ARRAY_REGION);
        let len = unsafe { len_f(jni.env, bytes) };
        let mut out = Vec::new();
        if len > 0 {
            let mut samples = vec![0i16; len as usize];
            unsafe { region_f(jni.env, bytes, 0, len, samples.as_mut_ptr()) };
            out = samples;
        }
        jni.delete_local_ref(bytes);
        out
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    pub fn has_camera(_front: bool) -> bool {
        false
    }
    pub fn take_photo(_front: bool) -> Option<Vec<u8>> {
        None
    }
    pub fn has_microphone() -> bool {
        false
    }
    pub fn start_mic() -> bool {
        false
    }
    pub fn stop_mic() {}
    pub fn read_mic_chunk() -> Vec<i16> {
        Vec::new()
    }
}

pub use imp::*;

/// Sample rate of chunks returned by [read_mic_chunk] (matches
/// `HostMedia.MIC_SAMPLE_RATE` on Android; unused elsewhere).
pub const MIC_SAMPLE_RATE: u32 = 44100;
