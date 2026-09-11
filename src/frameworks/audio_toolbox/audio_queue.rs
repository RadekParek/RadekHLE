/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `AudioQueue.h` (Audio Queue Services)
//!
//! The audio playback here is mapped onto OpenAL Soft for convenience.
//! Apple's implementation probably uses Core Audio instead.

use crate::abi::{CallFromHost, GuestFunction};
use crate::audio::decode_ima4;
use crate::audio::openal as al;
use crate::audio::openal::al_types::*;
use crate::audio::openal::{OpenAL, OpenALManager};
use crate::dyld::{export_c_func, FunctionExports};
use crate::frameworks::carbon_core::OSStatus;
use crate::frameworks::core_audio_types::{
    debug_fourcc, fourcc, kAudioFormatAppleIMA4, kAudioFormatFlagIsBigEndian,
    kAudioFormatFlagIsFloat, kAudioFormatFlagIsPacked, kAudioFormatLinearPCM, kAudioFormatMPEG4AAC,
    kAudioFormatMPEGLayer3, AudioStreamBasicDescription, AudioTimeStamp,
};
use crate::frameworks::core_foundation::cf_run_loop::{
    kCFRunLoopCommonModes, CFRunLoopGetMain, CFRunLoopMode, CFRunLoopRef,
};
use crate::frameworks::foundation::ns_run_loop;
use crate::frameworks::foundation::ns_string::get_static_str;
use crate::mem::{
    guest_size_of, ConstPtr, ConstVoidPtr, GuestUSize, Mem, MutPtr, MutVoidPtr, Ptr, SafeRead,
};
use crate::objc::msg;
use crate::Environment;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};

#[derive(Default)]
pub struct State {
    audio_queues: HashMap<AudioQueueRef, AudioQueueHostObject>,
    /// The output queue that currently owns the music source. Many older games
    /// recreate this queue at scene boundaries without disposing the previous
    /// one first; reusing it avoids two queues competing for one host mixer.
    active_output: Option<AudioQueueRef>,
}
impl State {
    fn get(framework_state: &mut crate::frameworks::State) -> &mut Self {
        &mut framework_state.audio_toolbox.audio_queue
    }
    fn get_with_context<'s, 'm: 's>(
        framework_state: &'s mut crate::frameworks::State,
        manager: &'m mut OpenALManager,
    ) -> (&'s mut Self, OpenAL<'s>) {
        (
            &mut framework_state.audio_toolbox.audio_queue,
            framework_state
                .audio_toolbox
                .al_context
                .make_al_context_current(manager),
        )
    }
}

struct AudioQueueHostObject {
    format: AudioStreamBasicDescription,
    callback_proc: AudioQueueOutputCallback,
    callback_user_data: MutVoidPtr,
    /// Weak reference
    run_loop: CFRunLoopRef,
    volume: f32,
    /// Stereo pan, -1.0 (full left) .. 1.0 (full right), 0.0 (centered).
    pan: f32,
    buffers: Vec<AudioQueueBufferRef>,
    /// There is also a queue of OpenAL buffers, which must be kept in sync:
    /// the nth item in this queue must also be the nth item in the OpenAL
    /// queue, though the OpenAL queue may be shorter.
    buffer_queue: VecDeque<AudioQueueBufferRef>,
    is_running: AudioQueueIsRunning,
    al_source: Option<ALuint>,
    al_unused_buffers: Vec<ALuint>,
    decoded_buffer_cache: VecDeque<DecodedAudioBuffer>,
    compressed_pending: Vec<u8>,
    compressed_pending_format: Option<u32>,
    compressed_emitted_pcm_bytes: usize,
    aq_is_running_proc: Option<AudioQueuePropertyListenerProc>,
    aq_is_running_user_data: Option<MutVoidPtr>,
    property_listeners: Vec<(
        AudioQueuePropertyID,
        AudioQueuePropertyListenerProc,
        MutVoidPtr,
    )>,
    is_running_handler: bool,
    callbacks: u64,
    requested_frames: u64,
    supplied_frames: u64,
    underruns: u64,
    last_underrun_log: Option<std::time::Instant>,
    is_input: bool,
    input_delay: u32,
    /// Stored `kAudioQueueProperty_HardwareCodecPolicy` value. Defaults to
    /// `kAudioQueueHardwareCodecPolicy_Default` (0). touchHLE has no hardware
    /// codecs, so this is informational only.
    hardware_codec_policy: u32,
    /// PCM format set via `AudioQueueSetOfflineRenderFormat`. `None` means the
    /// queue is in real-time mode (default). `Some(format)` means the queue is
    /// in offline-rendering mode and the caller is expected to drive playback
    /// via `AudioQueueOfflineRender`.
    offline_render_format: Option<AudioStreamBasicDescription>,
}

#[derive(Clone)]
pub(crate) struct DecodedAudioBuffer {
    pub(crate) key: u64,
    pub(crate) format: ALenum,
    pub(crate) frequency: ALsizei,
    pub(crate) data: Vec<u8>,
}

/// Track whether the audio queue is meant to be running, in order to handle
/// OpenAL stop events caused by running out of data:
/// - If it's running, the OpenAL source can be restarted.
/// - If it's stopping asynchronously, the audio queue stop can be completed.
#[derive(PartialEq, Eq, Clone, Copy)]
enum AudioQueueIsRunning {
    Running,
    Stopping,
    Stopped,
}

#[repr(C, packed)]
pub struct OpaqueAudioQueue {
    _filler: u8,
}
unsafe impl SafeRead for OpaqueAudioQueue {}

pub type AudioQueueRef = MutPtr<OpaqueAudioQueue>;

#[repr(C, packed)]
pub struct AudioQueueBuffer {
    audio_data_bytes_capacity: u32,
    pub audio_data: MutVoidPtr,
    pub audio_data_byte_size: u32,
    user_data: MutVoidPtr,
    packet_description_capacity: u32,
    /// Should be a `MutPtr<AudioStreamPacketDescription>`, but that's not
    /// implemented yet.
    _packet_descriptions: MutVoidPtr,
    _packet_description_count: u32,
}
unsafe impl SafeRead for AudioQueueBuffer {}

pub type AudioQueueBufferRef = MutPtr<AudioQueueBuffer>;

/// (*void)(void *in_user_data, AudioQueueRef in_aq, AudioQueueBufferRef in_buf)
pub type AudioQueueOutputCallback = GuestFunction;

type AudioQueueParameterID = u32;
pub const kAudioQueueParam_Volume: AudioQueueParameterID = 1;
// `kAudioQueueParam_Pan` per Apple's `AudioQueue.h`. Range -1.0 (full left)
// to 1.0 (full right). Mirrors `AVAudioPlayer.pan`.
pub const kAudioQueueParam_Pan: AudioQueueParameterID = 13;

type AudioQueueParameterValue = f32;

pub type AudioQueuePropertyID = u32;
pub const kAudioQueueProperty_IsRunning: AudioQueuePropertyID = fourcc(b"aqrn");
const kAudioQueueProperty_MagicCookie: AudioQueuePropertyID = fourcc(b"aqmc");
const kAudioQueueProperty_StreamDescription: AudioQueuePropertyID = fourcc(b"aqft");
const kAudioQueueProperty_MaximumOutputPacketSize: AudioQueuePropertyID = fourcc(b"aqmv");
const kAudioQueueProperty_EnableLevelMetering: AudioQueuePropertyID = fourcc(b"aqme");
/// `kAudioQueueProperty_HardwareCodecPolicy` from Apple's `AudioQueue.h`.
/// Controls whether the queue uses hardware or software codecs. touchHLE only
/// ships software codecs (the host has no audio hardware codec), so this is
/// stored as a hint and the codec choice never changes.
const kAudioQueueProperty_HardwareCodecPolicy: AudioQueuePropertyID = fourcc(b"aqcp");
type AudioQueuePropertyListenerProc = GuestFunction;

/// `AudioQueueHardwareCodecPolicy` from Apple's `AudioQueue.h`.
#[allow(dead_code)]
mod codec_policy {
    pub const DEFAULT: u32 = 0;
    pub const USE_SOFTWARE_ONLY: u32 = 1;
    pub const USE_HARDWARE_ONLY: u32 = 2;
    pub const PREFER_SOFTWARE: u32 = 3;
    pub const PREFER_HARDWARE: u32 = 4;
}

const kAudioQueueErr_InvalidBuffer: OSStatus = -66687;
const kAudioQueueErr_InvalidPropertySize: OSStatus = -66683;
const kAudioQueueErr_BufferInQueue: OSStatus = -66679;
const kAudioQueueErr_InvalidProperty: OSStatus = -66684;
/// `kAudioQueueErr_QueueNotStopped` from Apple's `AudioQueue.h`. Returned by
/// `AudioQueueSetOfflineRenderFormat` when the queue is currently running.
const kAudioQueueErr_QueueNotStopped: OSStatus = -66677;
const AUDIO_QUEUE_TARGET_UNPROCESSED_BUFFERS: usize = 4;
const AUDIO_QUEUE_STARTUP_BUFFER_COUNT: usize = 4;

fn formats_can_share_output(
    left: &AudioStreamBasicDescription,
    right: &AudioStreamBasicDescription,
) -> bool {
    left.format_id == right.format_id
        && left.sample_rate.to_bits() == right.sample_rate.to_bits()
        && left.channels_per_frame == right.channels_per_frame
        && left.bits_per_channel == right.bits_per_channel
}

fn reuse_audio_queue(
    env: &mut Environment,
    queue_ref: AudioQueueRef,
    format: AudioStreamBasicDescription,
    callback_proc: AudioQueueOutputCallback,
    callback_user_data: MutVoidPtr,
    run_loop: CFRunLoopRef,
) -> bool {
    let compatible = State::get(&mut env.framework_state)
        .audio_queues
        .get(&queue_ref)
        .is_some_and(|queue| {
            !queue.is_input
                && formats_can_share_output(&queue.format, &format)
                && (queue.is_running == AudioQueueIsRunning::Stopped
                    || (queue.callback_proc == callback_proc
                        && queue.callback_user_data == callback_user_data))
        });
    if !compatible {
        return false;
    }

    let _ = AudioQueueReset(env, queue_ref);
    let (old_run_loop, old_buffers) = {
        let state = State::get(&mut env.framework_state);
        let Some(queue) = state.audio_queues.get_mut(&queue_ref) else {
            return false;
        };
        let old_run_loop = queue.run_loop;
        queue.is_running = AudioQueueIsRunning::Stopped;
        queue.format = format;
        queue.callback_proc = callback_proc;
        queue.callback_user_data = callback_user_data;
        queue.run_loop = run_loop;
        queue.callbacks = 0;
        queue.requested_frames = 0;
        queue.supplied_frames = 0;
        queue.underruns = 0;
        queue.last_underrun_log = None;
        queue.compressed_pending.clear();
        queue.compressed_pending_format = None;
        queue.compressed_emitted_pcm_bytes = 0;
        (old_run_loop, std::mem::take(&mut queue.buffers))
    };

    for buffer_ref in old_buffers {
        let buffer = env.mem.read(buffer_ref);
        env.mem.free(buffer.audio_data);
        env.mem.free(buffer_ref.cast());
    }
    ns_run_loop::remove_audio_queue(env, old_run_loop, queue_ref);
    ns_run_loop::add_audio_queue(env, run_loop, queue_ref);
    preallocate_output_buffers(env, queue_ref, AUDIO_QUEUE_STARTUP_BUFFER_COUNT);
    true
}

pub fn AudioQueueNewOutput(
    env: &mut Environment,
    in_format: ConstPtr<AudioStreamBasicDescription>,
    in_callback_proc: AudioQueueOutputCallback,
    in_user_data: MutVoidPtr,
    in_callback_run_loop: CFRunLoopRef,
    in_callback_run_loop_mode: CFRunLoopMode,
    in_flags: u32,
    out_aq: MutPtr<AudioQueueRef>,
) -> OSStatus {
    // reserved: real Audio Queue Services ignores non-zero flags as a
    // forward-compatibility measure. Don't panic if a game passes garbage.
    if in_flags != 0 {
        log!(
            "Warning: AudioQueueNewOutput: ignoring unexpected non-zero flags {:#x}",
            in_flags
        );
    }

    // NULL is a synonym of kCFRunLoopCommonModes here. Anything else is
    // technically unsupported, but real iOS quietly accepts arbitrary
    // strings and just runs the callback on the requested loop.
    if !in_callback_run_loop_mode.is_null() {
        let common_modes = get_static_str(env, kCFRunLoopCommonModes);
        let is_common: bool = msg![env; in_callback_run_loop_mode isEqual:common_modes];
        if !is_common {
            log!(
                "Warning: AudioQueueNewOutput called with non-kCFRunLoopCommonModes \
                 run loop mode {:?}; treating as kCFRunLoopCommonModes.",
                in_callback_run_loop_mode
            );
        }
    }

    let in_callback_run_loop = if in_callback_run_loop.is_null() {
        CFRunLoopGetMain(env)
    } else {
        in_callback_run_loop
    };

    let mut format = env.mem.read(in_format);
    if env.bundle.bundle_identifier().starts_with("com.ea.candcra")
        && format.format_id == fourcc(b".mp3")
    {
        log!("Applying game-specific hack for C&C Red Alert: Fixing hardcoded audio format from .mp3 to PCM.");
        format = AudioStreamBasicDescription {
            sample_rate: 44100.0,
            format_id: kAudioFormatLinearPCM,
            format_flags: 12,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: 2,
            bits_per_channel: 16,
            _reserved: 0,
        }
    }

    if let Some(existing) = State::get(&mut env.framework_state).active_output {
        if reuse_audio_queue(
            env,
            existing,
            format,
            in_callback_proc,
            in_user_data,
            in_callback_run_loop,
        ) {
            env.mem.write(out_aq, existing);
            log!(
                "AudioQueueNewOutput: reusing active output queue {:?} for the new scene",
                existing
            );
            log_audio_queue_state(env, "AudioQueueNewOutput reused");
            return 0;
        }
    }

    let host_object = AudioQueueHostObject {
        format,
        callback_proc: in_callback_proc,
        callback_user_data: in_user_data,
        run_loop: in_callback_run_loop,
        volume: 1.0,
        pan: 0.0,
        buffers: Vec::new(),
        buffer_queue: VecDeque::new(),
        is_running: AudioQueueIsRunning::Stopped,
        al_source: None,
        al_unused_buffers: Vec::new(),
        decoded_buffer_cache: VecDeque::new(),
        compressed_pending: Vec::new(),
        compressed_pending_format: None,
        compressed_emitted_pcm_bytes: 0,
        aq_is_running_proc: None,
        aq_is_running_user_data: None,
        property_listeners: Vec::new(),
        callbacks: 0,
        requested_frames: 0,
        is_running_handler: false,
        supplied_frames: 0,
        underruns: 0,
        last_underrun_log: None,
        is_input: false,
        input_delay: 0,
        hardware_codec_policy: codec_policy::DEFAULT,
        offline_render_format: None,
    };

    let format_id = format.format_id;
    let sample_rate = format.sample_rate;
    let channels = format.channels_per_frame;
    let bytes_per_frame = format.bytes_per_frame;
    let bits_per_channel = format.bits_per_channel;
    log!(
        "AudioQueueNewOutput: queue format id={:?}, rate={:.1} Hz, channels={}, bytes_per_frame={}, bits={}, callback={:?}",
        format_id,
        sample_rate,
        channels,
        bytes_per_frame,
        bits_per_channel,
        in_callback_proc
    );

    let aq_ref = env.mem.alloc_and_write(OpaqueAudioQueue { _filler: 0 });
    let state = State::get(&mut env.framework_state);
    state.audio_queues.insert(aq_ref, host_object);
    state.active_output = Some(aq_ref);

    preallocate_output_buffers(env, aq_ref, AUDIO_QUEUE_STARTUP_BUFFER_COUNT);
    env.mem.write(out_aq, aq_ref);

    ns_run_loop::add_audio_queue(env, in_callback_run_loop, aq_ref);

    log_if_broken_audio_format(&format);

    if !is_supported_audio_format(&format) {
        log_dbg!("Warning: Audio queue {:?} will be ignored because its format is not yet supported: {:#?}", aq_ref, format);
    }

    log_dbg!(
        "AudioQueueNewOutput() for format {:#?}, new audio queue handle: {:?}",
        format,
        aq_ref,
    );
    log_audio_queue_state(env, "AudioQueueNewOutput");

    0 // success
}

/// Apply the queue's pan to its OpenAL source. Pan is implemented per Apple's
/// AudioToolbox semantics: -1.0 = full left, 0.0 = centered, 1.0 = full
/// right. We place the source on the x-axis with a small forward offset so
/// that the listener (default at origin facing -z) hears it equally loud at
/// pan 0 and panned at ±1.
// Standard OpenAL enum values (al.h). They are missing from the local
// `openal_soft_wrapper::al_defines` re-export but are part of the public
// 1.1 ABI, see <https://www.openal.org/documentation/openal-1.1-specification.pdf>.
const AL_SOURCE_RELATIVE: ALenum = 0x202;
const AL_POSITION: ALenum = 0x1004;
const AL_TRUE_I32: ALint = 1;

fn apply_al_pan(context: &OpenAL<'_>, al_source: ALuint, pan: f32) {
    let pan = pan.clamp(-1.0, 1.0);
    // Keep source relative to the listener so head tracking can't drift.
    unsafe {
        context.Sourcei(al_source, AL_SOURCE_RELATIVE, AL_TRUE_I32);
        context.Source3f(
            al_source,
            AL_POSITION,
            pan,
            0.0,
            -(1.0 - pan * pan).sqrt().max(0.0),
        );
        // Panning is purely cosmetic; if the driver rejects any of these calls
        // just clear the error rather than crashing the whole emulator.
        let _ = context.GetError();
    }
}

pub fn AudioQueueGetParameter(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_param_id: AudioQueueParameterID,
    out_value: MutPtr<AudioQueueParameterValue>,
) -> OSStatus {
    return_if_null!(in_aq);

    let state = State::get(&mut env.framework_state);

    let host_object = match state.audio_queues.get_mut(&in_aq) {
        Some(obj) => obj,
        None => return 0,
    };

    match in_param_id {
        kAudioQueueParam_Volume => {
            env.mem.write(out_value, host_object.volume);
            0
        }
        kAudioQueueParam_Pan => {
            env.mem.write(out_value, host_object.pan);
            0
        }
        // Real Audio Queue Services returns kAudioQueueErr_InvalidParameter
        // (-66670) in this case, which we approximate with
        // kAudioQueueErr_InvalidProperty.
        _ => {
            log!(
                "Warning: AudioQueueGetParameter: unsupported param id {}; \
                 returning 0.",
                in_param_id
            );
            env.mem.write(out_value, 0.0);
            kAudioQueueErr_InvalidProperty
        }
    }
}

pub fn AudioQueueSetParameter(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_param_id: AudioQueueParameterID,
    in_value: AudioQueueParameterValue,
) -> OSStatus {
    return_if_null!(in_aq);

    let state = State::get(&mut env.framework_state);

    let host_object = match state.audio_queues.get_mut(&in_aq) {
        Some(obj) => obj,
        None => return 0,
    };

    match in_param_id {
        kAudioQueueParam_Volume => {
            host_object.volume = in_value;
            let al_source = host_object.al_source;
            log_dbg!(
                "AudioQueueSetParameter kAudioQueueParam_Volume is set to {}",
                in_value
            );
            if let Some(al_source) = al_source {
                let context = env
                    .framework_state
                    .audio_toolbox
                    .make_al_context_current(&mut env.openal_manager);
                let clamped = in_value.clamp(0.0, 1.0);
                unsafe {
                    context.Sourcef(al_source, al::AL_MAX_GAIN, clamped);
                }
            }
            0
        }
        kAudioQueueParam_Pan => {
            let clamped = in_value.clamp(-1.0, 1.0);
            host_object.pan = clamped;
            let al_source = host_object.al_source;
            log_dbg!(
                "AudioQueueSetParameter kAudioQueueParam_Pan is set to {}",
                clamped
            );
            if let Some(al_source) = al_source {
                let context = env
                    .framework_state
                    .audio_toolbox
                    .make_al_context_current(&mut env.openal_manager);
                apply_al_pan(&context, al_source, clamped);
            }
            0
        }
        _ => {
            log!(
                "Warning: AudioQueueSetParameter: unsupported param id {}; \
                 ignoring.",
                in_param_id
            );
            kAudioQueueErr_InvalidProperty
        }
    }
}

fn AudioQueueAllocateBufferWithPacketDescriptions(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_buffer_byte_size: GuestUSize,
    _in_number_packet_desc: GuestUSize,
    out_buffer: MutPtr<AudioQueueBufferRef>,
) -> OSStatus {
    AudioQueueAllocateBuffer(env, in_aq, in_buffer_byte_size, out_buffer)
}

pub fn AudioQueueAllocateBuffer(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_buffer_byte_size: GuestUSize,
    out_buffer: MutPtr<AudioQueueBufferRef>,
) -> OSStatus {
    return_if_null!(in_aq);

    if in_buffer_byte_size > 16 * 1024 * 1024 {
        log!(
            "Error: AudioQueueAllocateBuffer requested ridiculously large buffer: {:#x} bytes",
            in_buffer_byte_size
        );
        return -50;
    }

    let host_object = match State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    {
        Some(obj) => obj,
        None => return 0,
    };

    let packet_description_capacity =
        if env.bundle.bundle_identifier().starts_with("com.ea.candcra") {
            1024
        } else {
            0
        };

    let audio_data = env.mem.alloc(in_buffer_byte_size);
    let buffer_ptr = env.mem.alloc_and_write(AudioQueueBuffer {
        audio_data_bytes_capacity: in_buffer_byte_size,
        audio_data,
        audio_data_byte_size: 0,
        user_data: Ptr::null(),
        packet_description_capacity,
        _packet_descriptions: Ptr::null(),
        _packet_description_count: 0,
    });

    host_object.buffers.push(buffer_ptr);
    env.mem.write(out_buffer, buffer_ptr);

    log_dbg!(
        "AudioQueueAllocateBuffer: queue={:?} buffer={:?} capacity={}",
        in_aq,
        buffer_ptr,
        in_buffer_byte_size
    );

    0 // success
}

pub fn AudioQueueEnqueueBuffer(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_buffer: AudioQueueBufferRef,
    _in_num_packet_descs: u32,
    _in_packet_descs: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    let should_prime = {
        let host_object = match State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
        {
            Some(obj) => obj,
            None => return 0,
        };

        if !host_object.buffers.contains(&in_buffer) {
            return kAudioQueueErr_InvalidBuffer;
        }
        if host_object.buffer_queue.contains(&in_buffer) {
            return kAudioQueueErr_BufferInQueue;
        }

        host_object.buffer_queue.push_back(in_buffer);

        log_dbg!(
            "AudioQueueEnqueueBuffer: queue={:?} buffer={:?} guest_buffers={} queued_buffers={}",
            in_aq,
            in_buffer,
            host_object.buffers.len(),
            host_object.buffer_queue.len()
        );

        host_object.is_running == AudioQueueIsRunning::Running
    };

    if should_prime {
        prime_audio_queue(env, in_aq);
    }

    0 // success
}

fn AudioQueueEnqueueBufferWithParameters(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_buffer: AudioQueueBufferRef,
    in_num_packet_descs: u32,
    in_packet_descs: MutVoidPtr,
    in_trim_frames_at_start: u32,
    in_trim_frames_at_end: u32,
    in_num_param_values: u32,
    in_param_values: MutVoidPtr,
    in_start_time: ConstPtr<AudioTimeStamp>,
    out_actual_start_time: MutPtr<AudioTimeStamp>,
) -> OSStatus {
    if in_trim_frames_at_start != 0
        || in_trim_frames_at_end != 0
        || in_num_param_values != 0
        || !in_param_values.is_null()
        || !in_start_time.is_null()
        || !out_actual_start_time.is_null()
    {
        log!(
            "AudioQueueEnqueueBufferWithParameters: ignoring optional trim, parameter, and start-time arguments"
        );
    }
    AudioQueueEnqueueBuffer(env, in_aq, in_buffer, in_num_packet_descs, in_packet_descs)
}

fn AudioQueueAddPropertyListener(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_id: AudioQueuePropertyID,
    in_proc: AudioQueuePropertyListenerProc,
    in_user_data: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    if in_id == kAudioQueueProperty_IsRunning {
        let host_object = match State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
        {
            Some(obj) => obj,
            None => return 0,
        };

        host_object.aq_is_running_proc = Some(in_proc);
        host_object.aq_is_running_user_data = Some(in_user_data);
    } else {
        let Some(host_object) = State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
        else {
            return kAudioQueueErr_InvalidProperty;
        };
        host_object
            .property_listeners
            .push((in_id, in_proc, in_user_data));
    }
    0 // success
}

fn AudioQueueRemovePropertyListener(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_id: AudioQueuePropertyID,
    in_proc: AudioQueuePropertyListenerProc,
    in_user_data: MutVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    if in_id == kAudioQueueProperty_IsRunning {
        // The guest can hold on to AudioQueueRef values past
        // AudioQueueDispose; mirror real Audio Queue Services and
        // return an error instead of panicking on a stale ref.
        let Some(host_object) = State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
        else {
            log!(
                "Warning: AudioQueueRemovePropertyListener({:?}): unknown / disposed queue.",
                in_aq
            );
            return kAudioQueueErr_InvalidProperty;
        };

        host_object.aq_is_running_proc = None;
        host_object.aq_is_running_user_data = None;
    } else {
        let Some(host_object) = State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
        else {
            return kAudioQueueErr_InvalidProperty;
        };
        if let Some(index) =
            host_object
                .property_listeners
                .iter()
                .position(|&(property_id, proc, user_data)| {
                    property_id == in_id && proc == in_proc && user_data == in_user_data
                })
        {
            host_object.property_listeners.remove(index);
        }
    }
    0 // success
}

fn property_size(property_id: AudioQueuePropertyID) -> Option<GuestUSize> {
    match property_id {
        kAudioQueueProperty_IsRunning => Some(guest_size_of::<u32>()),
        kAudioQueueProperty_MagicCookie => Some(0),
        kAudioQueueProperty_StreamDescription => {
            Some(guest_size_of::<AudioStreamBasicDescription>())
        }
        kAudioQueueProperty_MaximumOutputPacketSize => Some(guest_size_of::<u32>()),
        kAudioQueueProperty_EnableLevelMetering => Some(guest_size_of::<u32>()),
        kAudioQueueProperty_HardwareCodecPolicy => Some(guest_size_of::<u32>()),
        _ => None,
    }
}

fn AudioQueueGetPropertySize(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_property_id: AudioQueuePropertyID,
    out_data_size: MutPtr<u32>,
) -> OSStatus {
    return_if_null!(in_aq);

    match property_size(in_property_id) {
        Some(size) => {
            env.mem.write(out_data_size, size);
            0 // success
        }
        None => {
            log!(
                "TODO: AudioQueueGetPropertySize({:?}, {}): unknown property, returning error",
                in_aq,
                debug_fourcc(in_property_id)
            );
            kAudioQueueErr_InvalidProperty
        }
    }
}

fn AudioQueueGetProperty(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_property_id: AudioQueuePropertyID,
    out_property_data: MutVoidPtr,
    io_data_size: MutPtr<u32>,
) -> OSStatus {
    return_if_null!(in_aq);

    let required_size = match property_size(in_property_id) {
        Some(size) => size,
        None => {
            log!(
                "TODO: AudioQueueGetProperty({:?}, {}): unknown property, returning error",
                in_aq,
                debug_fourcc(in_property_id)
            );
            return kAudioQueueErr_InvalidProperty;
        }
    };
    let provided_size = env.mem.read(io_data_size);

    if required_size != 0 && provided_size < required_size {
        log!(
            "Warning: AudioQueueGetProperty() failed: provided size {} < required size {}",
            provided_size,
            required_size
        );
        return kAudioQueueErr_InvalidPropertySize;
    }

    // Don't panic on stale AudioQueueRef values: real Audio Queue
    // Services returns an error instead.
    let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    else {
        log!(
            "Warning: AudioQueueGetProperty({:?}): unknown / disposed queue.",
            in_aq
        );
        return kAudioQueueErr_InvalidProperty;
    };

    match in_property_id {
        kAudioQueueProperty_IsRunning => {
            let is_running: u32 = match host_object.is_running {
                AudioQueueIsRunning::Running => 1,
                AudioQueueIsRunning::Stopping => 1,
                AudioQueueIsRunning::Stopped => 0,
            };
            env.mem.write(out_property_data.cast(), is_running);
        }
        kAudioQueueProperty_MagicCookie => {
            log_dbg!("AudioQueueGetProperty: kAudioQueueProperty_MagicCookie requested, returning empty.");
        }
        kAudioQueueProperty_StreamDescription => {
            env.mem.write(out_property_data.cast(), host_object.format);
        }
        kAudioQueueProperty_MaximumOutputPacketSize => {
            // Return the bytes_per_packet from the queue's stream format.
            // If the format is VBR (bytes_per_packet == 0), report a
            // reasonable upper bound so the caller can size its read buffer.
            let max_packet = if host_object.format.bytes_per_packet > 0 {
                host_object.format.bytes_per_packet
            } else {
                // Conservative upper bound for common compressed formats
                // (AAC, MP3, etc.) — matches what Core Audio typically
                // reports on real hardware.
                2048
            };
            env.mem.write(out_property_data.cast(), max_packet);
            log_dbg!(
                "AudioQueueGetProperty: kAudioQueueProperty_MaximumOutputPacketSize => {}",
                max_packet
            );
        }
        kAudioQueueProperty_EnableLevelMetering => {
            // Level metering is not implemented; report it as disabled (0).
            env.mem.write(out_property_data.cast::<u32>(), 0u32);
        }
        kAudioQueueProperty_HardwareCodecPolicy => {
            env.mem.write(
                out_property_data.cast::<u32>(),
                host_object.hardware_codec_policy,
            );
        }
        _ => {
            // We only advertise known properties as readable via
            // property_size; if we somehow get here with a different ID it
            // means the size table and this match got out of sync. Don't
            // crash the host: return an InvalidProperty error code as Apple
            // does for unknown properties.
            log!(
                "Warning: AudioQueueGetProperty({:?}, {}): unsupported property id; returning kAudioQueueErr_InvalidProperty.",
                in_aq,
                debug_fourcc(in_property_id)
            );
            return kAudioQueueErr_InvalidProperty;
        }
    }

    0 // success
}

fn AudioQueueSetProperty(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_property_id: AudioQueuePropertyID,
    in_property_data: ConstVoidPtr,
    in_data_size: u32,
) -> OSStatus {
    return_if_null!(in_aq);

    // Per Apple `AudioQueue.h`: magic-cookie writes have format-specific
    // semantics we don't model, so refuse them rather than silently
    // succeeding (returning success here previously confused some apps).
    if in_property_id == kAudioQueueProperty_MagicCookie {
        return kAudioQueueErr_InvalidProperty;
    }

    match in_property_id {
        kAudioQueueProperty_HardwareCodecPolicy => {
            let required = guest_size_of::<u32>();
            if in_data_size < required || in_property_data.is_null() {
                return kAudioQueueErr_InvalidPropertySize;
            }
            let policy = env.mem.read(in_property_data.cast::<u32>());
            // Apple defines five valid policy values; anything else is
            // rejected to match Audio Queue Services behaviour.
            let valid = matches!(
                policy,
                codec_policy::DEFAULT
                    | codec_policy::USE_SOFTWARE_ONLY
                    | codec_policy::USE_HARDWARE_ONLY
                    | codec_policy::PREFER_SOFTWARE
                    | codec_policy::PREFER_HARDWARE
            );
            if !valid {
                return kAudioQueueErr_InvalidProperty;
            }
            let Some(host_object) = State::get(&mut env.framework_state)
                .audio_queues
                .get_mut(&in_aq)
            else {
                return kAudioQueueErr_InvalidProperty;
            };
            host_object.hardware_codec_policy = policy;
            log_dbg!(
                "AudioQueueSetProperty(kAudioQueueProperty_HardwareCodecPolicy = {})",
                policy
            );
            notify_aq_property_listeners(env, in_aq, kAudioQueueProperty_HardwareCodecPolicy);
            0 // success
        }
        kAudioQueueProperty_EnableLevelMetering => {
            // We don't implement level metering, but we accept the write so
            // the caller's setup code keeps going. The matching getter
            // always reports metering as disabled.
            let required = guest_size_of::<u32>();
            if in_data_size < required || in_property_data.is_null() {
                return kAudioQueueErr_InvalidPropertySize;
            }
            0 // success
        }
        _ => {
            // Per Apple `AudioQueue.h`, an unknown property ID yields
            // `kAudioQueueErr_InvalidProperty`. Log it so we can spot apps
            // that depend on an unsupported property.
            log!(
                "Warning: AudioQueueSetProperty({:?}, {}, {:?}, {}): unsupported property; \
                 returning kAudioQueueErr_InvalidProperty.",
                in_aq,
                debug_fourcc(in_property_id),
                in_property_data,
                in_data_size,
            );
            kAudioQueueErr_InvalidProperty
        }
    }
}

/// `AudioQueueSetOfflineRenderFormat` from Apple's `AudioQueue.h`.
///
/// > Sets the format for offline rendering. If `inFormat` is non-NULL, the
/// > queue switches to offline rendering mode using the given PCM format.
/// > Passing NULL leaves offline rendering mode.
///
/// Apple requires the queue to be stopped and the format (when non-NULL) to be
/// uncompressed PCM. The channel layout argument is accepted for API
/// compatibility but not modelled by touchHLE — every guest we have seen
/// passes NULL here, matching Apple's own examples.
fn AudioQueueSetOfflineRenderFormat(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_format: ConstPtr<AudioStreamBasicDescription>,
    _in_layout: ConstVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    else {
        return kAudioQueueErr_InvalidProperty;
    };

    // Per Apple docs, the queue must not be running when offline rendering is
    // (re)configured.
    if host_object.is_running != AudioQueueIsRunning::Stopped {
        log!(
            "AudioQueueSetOfflineRenderFormat({:?}): queue is running; \
             returning kAudioQueueErr_QueueNotStopped.",
            in_aq
        );
        return kAudioQueueErr_QueueNotStopped;
    }

    if in_format.is_null() {
        // NULL format leaves offline rendering mode.
        host_object.offline_render_format = None;
        log_dbg!(
            "AudioQueueSetOfflineRenderFormat({:?}): leaving offline mode",
            in_aq
        );
        return 0;
    }

    let format = env.mem.read(in_format);

    // Apple's offline rendering only supports uncompressed PCM destinations.
    if format.format_id != kAudioFormatLinearPCM {
        log!(
            "AudioQueueSetOfflineRenderFormat({:?}): non-PCM format {} \
             rejected (offline rendering only supports PCM).",
            in_aq,
            debug_fourcc(format.format_id)
        );
        return kAudioQueueErr_InvalidProperty;
    }
    if !is_supported_audio_format(&format) {
        log!(
            "AudioQueueSetOfflineRenderFormat({:?}): unsupported PCM format \
             {:?}.",
            in_aq,
            format
        );
        return kAudioQueueErr_InvalidProperty;
    }

    host_object.offline_render_format = Some(format);
    log_dbg!(
        "AudioQueueSetOfflineRenderFormat({:?}): entering offline mode with {:?}",
        in_aq,
        format
    );
    0 // success
}

pub fn log_if_broken_audio_format(format: &AudioStreamBasicDescription) {
    // Compressed formats (MPEG Layer III, AAC) intentionally use
    // zero for `bytes_per_packet`, `bytes_per_frame` and
    // `bits_per_channel` because their packets are variable-size and
    // their PCM frame width is not known until decode. Skip the
    // PCM-shape sanity check for them.
    if format.format_id == kAudioFormatMPEGLayer3 || format.format_id == kAudioFormatMPEG4AAC {
        return;
    }

    // Float PCM formats (e.g. 32-bit float stereo at 32kHz used by
    // Sonic Runners / Unity games) are perfectly valid per Apple's
    // Core Audio documentation. The bytes_per_frame for a 2-channel
    // 32-bit float format is 4 (not 8) when kAudioFormatFlagIsPacked
    // is set and frames_per_packet is 1 — this is valid because the
    // format uses interleaved float samples where each sample is 4 bytes
    // but channels_per_frame * bytes_per_channel may differ from
    // bytes_per_frame in packed non-interleaved layouts.
    // See: https://developer.apple.com/documentation/coreaudiotypes/audiostreambasicdescription
    if format.format_id == kAudioFormatLinearPCM
        && (format.format_flags & kAudioFormatFlagIsFloat) != 0
    {
        // Float PCM is valid; don't warn about it.
        return;
    }

    let bytes_per_channel = format.bits_per_channel / 8;
    let expected_bytes_per_packet = format.bytes_per_frame * format.frames_per_packet;
    let expected_bytes_per_frame = format.channels_per_frame * bytes_per_channel;

    if format.bytes_per_packet < expected_bytes_per_packet
        || format.bytes_per_frame < expected_bytes_per_frame
    {
        log!(
            "Warning: Stream format has non-sensical values: {:?}",
            format
        );
    }
}

pub fn is_supported_audio_format(format: &AudioStreamBasicDescription) -> bool {
    let &AudioStreamBasicDescription {
        format_id,
        format_flags,
        channels_per_frame,
        bits_per_channel,
        bytes_per_frame,
        ..
    } = format;

    match format_id {
        kAudioFormatAppleIMA4 => (channels_per_frame == 1) || (channels_per_frame == 2),
        kAudioFormatLinearPCM => {
            (channels_per_frame == 1 || channels_per_frame == 2)
                && (bits_per_channel == 8 || bits_per_channel == 16 || bits_per_channel == 32)
                && ((format_flags & kAudioFormatFlagIsPacked) != 0
                    || ((bits_per_channel / 8) * channels_per_frame) == bytes_per_frame)
                && (format_flags & kAudioFormatFlagIsBigEndian) == 0
        }
        // MPEG-1 / MPEG-2 Layer III. Apple's documentation for
        // `kAudioFormatMPEGLayer3` (see
        // <https://developer.apple.com/documentation/coreaudiotypes/kaudioformatmpeglayer3>)
        // says the format is compressed: `bytes_per_packet`,
        // `bytes_per_frame` and `bits_per_channel` are conventionally 0
        // because each MPEG audio frame is variable-size and decompresses
        // to 1152 PCM frames (`frames_per_packet`). We support mono /
        // stereo at any sample rate; symphonia handles the actual
        // decoding in `decode_buffer`.
        kAudioFormatMPEGLayer3 => channels_per_frame == 1 || channels_per_frame == 2,
        // MPEG-4 AAC. Like MP3, the format is compressed (variable-size
        // packets, zero `bytes_per_packet`/`bytes_per_frame`/
        // `bits_per_channel`); see Apple's documentation for
        // `kAudioFormatMPEG4AAC`. Buffers are expected to contain ADTS
        // frames, which is what `AudioFile` produces for `.aac` files and
        // AAC-in-CAF; symphonia handles the decoding in `decode_buffer`.
        kAudioFormatMPEG4AAC => channels_per_frame == 1 || channels_per_frame == 2,
        _ => false,
    }
}

pub fn decode_buffer(
    mem: &Mem,
    format: &AudioStreamBasicDescription,
    audio_data: MutPtr<u8>,
    audio_data_byte_size: GuestUSize,
) -> (ALenum, ALsizei, Vec<u8>) {
    let data_slice = mem.bytes_at(audio_data, audio_data_byte_size);

    if !is_supported_audio_format(format) {
        // Real CoreAudio would refuse the buffer back at
        // AudioQueueNewOutput, but if a previously valid queue is fed an
        // unsupported format mid-stream (e.g. after seek) we still don't
        // want to crash the host.
        log!(
            "Warning: decode_buffer: format is not supported by our HLE: {:?}; returning empty buffer.",
            format
        );
        return (
            al::AL_FORMAT_MONO16,
            format.sample_rate.max(8000.0) as ALsizei,
            Vec::new(),
        );
    }

    match format.format_id {
        kAudioFormatAppleIMA4 => {
            let valid_len = data_slice.len() / 34 * 34;
            if valid_len != data_slice.len() {
                log!(
                    "Warning: decode_buffer: ignoring {} trailing bytes in Apple IMA4 data.",
                    data_slice.len() - valid_len
                );
            }
            let data_slice = &data_slice[..valid_len];
            let mut out_pcm = Vec::<u8>::with_capacity((data_slice.len() / 34) * 64 * 2);
            let packets = data_slice.chunks(34);

            if format.channels_per_frame == 1 {
                for packet in packets {
                    let pcm_packet: [i16; 64] = decode_ima4(packet.try_into().unwrap());
                    let pcm_bytes: &[u8] = unsafe {
                        std::slice::from_raw_parts(pcm_packet.as_ptr() as *const u8, 128)
                    };
                    out_pcm.extend_from_slice(pcm_bytes);
                }

                (al::AL_FORMAT_MONO16, format.sample_rate as ALsizei, out_pcm)
            } else {
                let mut peekable_packets = packets.peekable();

                while peekable_packets.peek().is_some() {
                    let left = peekable_packets.next().unwrap();
                    let left_pcm_packet: [i16; 64] = decode_ima4(left.try_into().unwrap());
                    let right = peekable_packets.next().unwrap();
                    let right_pcm_packet: [i16; 64] = decode_ima4(right.try_into().unwrap());

                    for (l, r) in left_pcm_packet.iter().zip(right_pcm_packet.iter()) {
                        out_pcm.extend_from_slice(&l.to_le_bytes());
                        out_pcm.extend_from_slice(&r.to_le_bytes());
                    }
                }

                (
                    al::AL_FORMAT_STEREO16,
                    format.sample_rate as ALsizei,
                    out_pcm,
                )
            }
        }
        kAudioFormatLinearPCM => {
            let misaligned_by = data_slice.len() % (format.bytes_per_frame as usize);
            let data_slice = if misaligned_by != 0 {
                &data_slice[..data_slice.len() - misaligned_by]
            } else {
                data_slice
            };

            let bytes_per_channel = format.bits_per_channel / 8;
            let actual_bytes_per_frame = format.channels_per_frame * bytes_per_channel;
            let actual_channels_per_frame = format.bytes_per_frame / bytes_per_channel;

            let mut processed_data: Vec<u8> = if actual_bytes_per_frame == format.bytes_per_frame {
                data_slice.to_owned()
            } else {
                let actual_frame_count = data_slice.len() / actual_bytes_per_frame as usize;

                let processed_frame_count = format.bytes_per_frame as usize * actual_frame_count;
                let mut processed_data = Vec::<u8>::with_capacity(processed_frame_count);

                for frame in data_slice.chunks(actual_bytes_per_frame as usize) {
                    let frame_bytes = &frame[frame.len() - format.bytes_per_frame as usize..];

                    match format.bytes_per_frame {
                        1 => processed_data.extend(
                            &u8::from_be_bytes(frame_bytes.try_into().unwrap()).to_le_bytes(),
                        ),
                        2 => processed_data.extend_from_slice(
                            &u16::from_be_bytes(frame_bytes.try_into().unwrap()).to_le_bytes(),
                        ),
                        4 => processed_data.extend_from_slice(
                            &u32::from_be_bytes(frame_bytes.try_into().unwrap()).to_le_bytes(),
                        ),
                        8 => processed_data.extend_from_slice(
                            &u64::from_be_bytes(frame_bytes.try_into().unwrap()).to_le_bytes(),
                        ),
                        16 => processed_data.extend_from_slice(
                            &u128::from_be_bytes(frame_bytes.try_into().unwrap_or([0u8; 16]))
                                .to_le_bytes(),
                        ),
                        other => {
                            log!(
                                "Warning: decode_buffer: unsupported bytes_per_frame={}, dropping frame.",
                                other
                            );
                            // Pad with zeroes to keep frame alignment in
                            // the consumer; better than aborting.
                            processed_data.extend(std::iter::repeat_n(0u8, other as usize));
                        }
                    };
                }
                processed_data
            };

            let f = match (actual_channels_per_frame, format.bits_per_channel) {
                (1, 8) => al::AL_FORMAT_MONO8,
                (1, 16) => al::AL_FORMAT_MONO16,
                (2, 8) => al::AL_FORMAT_STEREO8,
                (2, 16) => al::AL_FORMAT_STEREO16,
                // --- ДОБАВЛЕНА РАБОЧАЯ ВЕТКА ДЛЯ (1, 32) ---
                (1, 32) => {
                    let valid_len = processed_data.len() / 4 * 4;
                    processed_data.truncate(valid_len);
                    let new_size = (processed_data.len() / 4) * 2;
                    let mut new_processed_data = Vec::<u8>::with_capacity(new_size);

                    if (format.format_flags & kAudioFormatFlagIsFloat) != 0 {
                        // 32-bit float PCM: convert float [-1.0, 1.0] to i16
                        // Apple Core Audio docs: kAudioFormatFlagIsFloat
                        // indicates IEEE 754 floating point samples.
                        // https://developer.apple.com/documentation/coreaudiotypes/kaudioformatflagisfloat
                        for chunk in processed_data.chunks(4) {
                            let val = f32::from_le_bytes(chunk.try_into().unwrap());
                            // Clamp to [-1.0, 1.0] then scale to i16 range
                            let clamped = val.clamp(-1.0, 1.0);
                            let new_val: i16 = (clamped * 32767.0) as i16;
                            new_processed_data.extend(new_val.to_le_bytes());
                        }
                    } else {
                        // 32-bit signed integer PCM: shift down to 16-bit
                        for chunk in processed_data.chunks(4) {
                            let val: i32 = i32::from_le_bytes(chunk.try_into().unwrap());
                            let new_val: i16 = (val >> 16) as i16;
                            new_processed_data.extend(new_val.to_le_bytes());
                        }
                    }
                    return (
                        al::AL_FORMAT_MONO16,
                        format.sample_rate as ALsizei,
                        new_processed_data,
                    );
                }
                // --- СУЩЕСТВУЮЩАЯ ВЕТКА (2, 32) ---
                (2, 32) => {
                    let valid_len = processed_data.len() / 4 * 4;
                    processed_data.truncate(valid_len);
                    let new_size = (processed_data.len() / 4) * 2;
                    let mut new_processed_data = Vec::<u8>::with_capacity(new_size);

                    if (format.format_flags & kAudioFormatFlagIsFloat) != 0 {
                        // 32-bit float PCM stereo: convert float [-1.0, 1.0] to i16
                        for chunk in processed_data.chunks(4) {
                            let val = f32::from_le_bytes(chunk.try_into().unwrap());
                            let clamped = val.clamp(-1.0, 1.0);
                            let new_val: i16 = (clamped * 32767.0) as i16;
                            new_processed_data.extend(new_val.to_le_bytes());
                        }
                    } else {
                        // 32-bit signed integer PCM: shift down to 16-bit
                        for chunk in processed_data.chunks(4) {
                            let val: i32 = i32::from_le_bytes(chunk.try_into().unwrap());
                            let new_val: i16 = (val >> 16) as i16;
                            new_processed_data.extend(new_val.to_le_bytes());
                        }
                    }
                    return (
                        al::AL_FORMAT_STEREO16,
                        format.sample_rate as ALsizei,
                        new_processed_data,
                    );
                }
                // ... предыдущие рабочие ветки (1, 32) и (2, 32) остаются как
                // есть ...
                _ => {
                    // Копируем значение в локальную переменную, чтобы избежать
                    // создания ссылки на packed-поле
                    let bits = format.bits_per_channel;
                    log!(
                        "Warning: decode_buffer: unhandled audio format: {} channels, {} bits; returning empty mono16 buffer.",
                        actual_channels_per_frame,
                        bits
                    );
                    return (
                        al::AL_FORMAT_MONO16,
                        format.sample_rate.max(8000.0) as ALsizei,
                        Vec::new(),
                    );
                }
            };

            (f, format.sample_rate as ALsizei, processed_data)
        }
        kAudioFormatMPEGLayer3 | kAudioFormatMPEG4AAC => {
            // Decode the buffer's worth of raw MPEG-1/2 Layer III frames
            // (or ADTS AAC frames) via symphonia (the same decoder we use
            // for `AudioFile` / `ExtAudioFile` MP3/AAC sources). Packets in
            // AudioQueueEnqueueBuffer are typically frame-aligned per the
            // contract of `AudioFileStreamParseBytes` /
            // `AudioFileReadPackets`, so feeding the slice as a single
            // raw stream works in practice. Frames that straddle
            // buffer boundaries are dropped by symphonia and logged at
            // debug level — better than letting AudioQueueStart fail.
            let cursor = std::io::Cursor::new(data_slice.to_vec());
            let Ok(decoded) = crate::audio::symphonia_formats::decode_symphonia_to_pcm(cursor)
            else {
                let sample_rate = format.sample_rate;
                log!(
                    "Warning: decode_buffer: MP3/AAC chunk could not be decoded; \
                     returning empty mono16 buffer."
                );
                return (
                    al::AL_FORMAT_MONO16,
                    sample_rate.max(8000.0) as ALsizei,
                    Vec::new(),
                );
            };
            normalise_decoded_audio(decoded.bytes, decoded.sample_rate, decoded.channels)
        }
        _ => {
            // Copy values out of the packed struct before formatting to
            // avoid taking unaligned references.
            let format_id = format.format_id;
            let sample_rate = format.sample_rate;
            log!(
                "Warning: decode_buffer: unsupported audio format id {}; returning empty mono16 buffer.",
                format_id
            );
            (
                al::AL_FORMAT_MONO16,
                sample_rate.max(8000.0) as ALsizei,
                Vec::new(),
            )
        }
    }
}

fn normalise_decoded_audio(
    data: Vec<u8>,
    sample_rate: u32,
    channels: u32,
) -> (ALenum, ALsizei, Vec<u8>) {
    let frequency = sample_rate.max(8_000) as ALsizei;
    match channels {
        1 => (al::AL_FORMAT_MONO16, frequency, data),
        2 => (al::AL_FORMAT_STEREO16, frequency, data),
        0 => (al::AL_FORMAT_MONO16, frequency, Vec::new()),
        channel_count => {
            let channel_count = channel_count as usize;
            let frame_size = channel_count.saturating_mul(2);
            if frame_size == 0 {
                return (al::AL_FORMAT_MONO16, frequency, Vec::new());
            }
            let frame_count = data.len() / frame_size;
            let mut downmixed = Vec::with_capacity(frame_count.saturating_mul(2));
            for frame in data[..frame_count * frame_size].chunks_exact(frame_size) {
                let mut sum = 0i64;
                for sample in frame.chunks_exact(2) {
                    sum += i16::from_le_bytes([sample[0], sample[1]]) as i64;
                }
                let sample =
                    (sum / channel_count as i64).clamp(i16::MIN as i64, i16::MAX as i64) as i16;
                downmixed.extend_from_slice(&sample.to_le_bytes());
            }
            log!(
                "Audio decoder produced {} channels; downmixed to mono for OpenAL",
                channel_count
            );
            (al::AL_FORMAT_MONO16, frequency, downmixed)
        }
    }
}

pub(crate) fn decode_compressed_buffer(
    pending: &mut Vec<u8>,
    pending_format: &mut Option<u32>,
    emitted_pcm_bytes: &mut usize,
    mem: &Mem,
    format: &AudioStreamBasicDescription,
    audio_data: MutPtr<u8>,
    audio_data_byte_size: GuestUSize,
) -> (ALenum, ALsizei, Vec<u8>) {
    let format_id = format.format_id;
    if *pending_format != Some(format_id) {
        pending.clear();
        *pending_format = Some(format_id);
        *emitted_pcm_bytes = 0;
    }
    let bytes = mem.bytes_at(audio_data, audio_data_byte_size);
    if bytes.is_empty() {
        return (
            fallback_al_format(format),
            format.sample_rate.max(8_000.0) as ALsizei,
            Vec::new(),
        );
    }
    pending.extend_from_slice(&bytes);

    // Re-decoding the complete prefix keeps frame boundaries intact when a
    // game splits an MP3 or AAC packet across guest buffers. The previous
    // implementation discarded the prefix after every successful decode, so
    // the next buffer began in the middle of a compressed frame and became
    // silence. Keep the decoded prefix and emit only the newly decoded PCM.
    const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;
    if pending.len() > MAX_STREAM_BYTES {
        log!(
            "Warning: compressed AudioQueue stream exceeded {} MiB; restarting its decode window",
            MAX_STREAM_BYTES / (1024 * 1024)
        );
        pending.clear();
        pending.extend_from_slice(&bytes);
        *emitted_pcm_bytes = 0;
    }
    if pending.len() < 64 {
        return (
            fallback_al_format(format),
            format.sample_rate.max(8_000.0) as ALsizei,
            Vec::new(),
        );
    }

    let cursor = std::io::Cursor::new(pending.clone());
    let decoded = crate::audio::symphonia_formats::decode_symphonia_to_pcm(cursor);
    match decoded {
        Ok(decoded) if !decoded.bytes.is_empty() => {
            let (_, frequency, normalised) =
                normalise_decoded_audio(decoded.bytes, decoded.sample_rate, decoded.channels);
            if *emitted_pcm_bytes > normalised.len() {
                *emitted_pcm_bytes = 0;
            }
            let start = *emitted_pcm_bytes;
            *emitted_pcm_bytes = normalised.len();
            let new_pcm = normalised.get(start..).unwrap_or_default().to_vec();
            let al_format = if decoded.channels == 1 || decoded.channels > 2 {
                al::AL_FORMAT_MONO16
            } else {
                al::AL_FORMAT_STEREO16
            };
            (al_format, frequency, new_pcm)
        }
        Ok(_) | Err(_) => (
            fallback_al_format(format),
            format.sample_rate.max(8_000.0) as ALsizei,
            Vec::new(),
        ),
    }
}

pub(crate) fn decode_buffer_cached(
    cache: &mut VecDeque<DecodedAudioBuffer>,
    mem: &Mem,
    format: &AudioStreamBasicDescription,
    audio_data: MutPtr<u8>,
    audio_data_byte_size: GuestUSize,
) -> (ALenum, ALsizei, Vec<u8>) {
    const CACHE_CAPACITY: usize = 8;
    let bytes = mem.bytes_at(audio_data, audio_data_byte_size);
    let format_id = format.format_id;
    let format_flags = format.format_flags;
    let sample_rate = format.sample_rate;
    let channels = format.channels_per_frame;
    let bits = format.bits_per_channel;
    let bytes_per_frame = format.bytes_per_frame;
    let bytes_per_packet = format.bytes_per_packet;
    let frames_per_packet = format.frames_per_packet;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format_id.hash(&mut hasher);
    format_flags.hash(&mut hasher);
    sample_rate.to_bits().hash(&mut hasher);
    channels.hash(&mut hasher);
    bits.hash(&mut hasher);
    bytes_per_frame.hash(&mut hasher);
    bytes_per_packet.hash(&mut hasher);
    frames_per_packet.hash(&mut hasher);
    bytes.hash(&mut hasher);
    let key = hasher.finish();

    if let Some(cached) = cache.iter().find(|cached| cached.key == key) {
        return (cached.format, cached.frequency, cached.data.clone());
    }

    let decoded = decode_buffer(mem, format, audio_data, audio_data_byte_size);
    if !decoded.2.is_empty() {
        if cache.len() == CACHE_CAPACITY {
            cache.pop_front();
        }
        cache.push_back(DecodedAudioBuffer {
            key,
            format: decoded.0,
            frequency: decoded.1,
            data: decoded.2.clone(),
        });
    }
    decoded
}

fn frames_for_audio_bytes(format: &AudioStreamBasicDescription, bytes: usize) -> u64 {
    if format.bytes_per_frame != 0 {
        (bytes as u64) / u64::from(format.bytes_per_frame)
    } else if format.bytes_per_packet != 0 && format.frames_per_packet != 0 {
        (bytes as u64 / u64::from(format.bytes_per_packet))
            .saturating_mul(u64::from(format.frames_per_packet))
    } else {
        0
    }
}

pub(crate) fn apply_lower_audio_quality(
    al_format: ALenum,
    frequency: ALsizei,
    data: Vec<u8>,
) -> (ALsizei, Vec<u8>) {
    let (channels, bytes_per_sample) = match al_format {
        al::AL_FORMAT_MONO8 => (1usize, 1usize),
        al::AL_FORMAT_STEREO8 => (2usize, 1usize),
        al::AL_FORMAT_MONO16 => (1usize, 2usize),
        al::AL_FORMAT_STEREO16 => (2usize, 2usize),
        _ => return (frequency, data),
    };
    if frequency <= 22_050 {
        return (frequency, data);
    }
    let frame_size = channels * bytes_per_sample;
    let frame_count = data.len() / frame_size;
    if frame_count < 2 {
        return (frequency, data);
    }
    let mut reduced = Vec::with_capacity((frame_count.div_ceil(2)) * frame_size);
    for frame in data[..frame_count * frame_size]
        .chunks_exact(frame_size)
        .step_by(2)
    {
        reduced.extend_from_slice(frame);
    }
    ((frequency / 2).max(8_000), reduced)
}

fn fallback_al_format(format: &AudioStreamBasicDescription) -> ALenum {
    if format.channels_per_frame == 2 {
        match format.bits_per_channel {
            8 => al::AL_FORMAT_STEREO8,
            _ => al::AL_FORMAT_STEREO16,
        }
    } else if format.bits_per_channel == 8 {
        al::AL_FORMAT_MONO8
    } else {
        al::AL_FORMAT_MONO16
    }
}

fn silence_buffer(al_format: ALenum, frames: usize) -> Vec<u8> {
    let bytes_per_frame = match al_format {
        al::AL_FORMAT_MONO8 => 1,
        al::AL_FORMAT_STEREO8 => 2,
        al::AL_FORMAT_MONO16 => 2,
        al::AL_FORMAT_STEREO16 => 4,
        _ => 2,
    };
    vec![0; frames.saturating_mul(bytes_per_frame)]
}

fn frames_for_al_buffer(al_format: ALenum, byte_count: usize) -> u64 {
    let bytes_per_frame = match al_format {
        al::AL_FORMAT_MONO8 => 1,
        al::AL_FORMAT_STEREO8 => 2,
        al::AL_FORMAT_MONO16 => 2,
        al::AL_FORMAT_STEREO16 => 4,
        _ => 0,
    };
    if bytes_per_frame == 0 {
        0
    } else {
        (byte_count / bytes_per_frame) as u64
    }
}

fn record_audio_queue_underrun(
    host_object: &mut AudioQueueHostObject,
    in_aq: AudioQueueRef,
    now: std::time::Instant,
) {
    host_object.underruns = host_object.underruns.saturating_add(1);
    let should_log = host_object.last_underrun_log.map_or(true, |last| {
        now.duration_since(last) >= std::time::Duration::from_secs(1)
    });
    if should_log {
        log!(
            "AudioQueue underrun: queue={:?} callbacks={} requested_frames={} supplied_frames={} underruns={}",
            in_aq,
            host_object.callbacks,
            host_object.requested_frames,
            host_object.supplied_frames,
            host_object.underruns,
        );
        host_object.last_underrun_log = Some(now);
    }
}

fn prime_audio_queue(env: &mut Environment, in_aq: AudioQueueRef) {
    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    // The guest can hold on to AudioQueueRef pointers across
    // AudioQueueDispose, or pass references that were never created (e.g.
    // junk memory left in the AVAudioPlayer host object when its underlying
    // AudioFile became a Dummy and `prepareToPlay` returned early). Real
    // Audio Queue Services would return an error in those cases; mirror that
    // here instead of panicking on `unwrap()`.
    let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
        log!(
            "Warning: prime_audio_queue({:?}) called on an unknown / disposed \
             audio queue; skipping.",
            in_aq
        );
        return;
    };

    if !is_supported_audio_format(&host_object.format) {
        return;
    }

    if host_object.al_source.is_none() {
        let volume = host_object.volume.clamp(0.0, 1.0);
        let pan = host_object.pan.clamp(-1.0, 1.0);
        let mut al_source = 0;

        // A real Audio Queue is backed by the system audio hardware, which can
        // always play at least a handful of concurrent sounds. OpenAL Soft,
        // however, has a finite source pool (mono_sources + stereo_sources).
        // Games like Ghost Blade and UDKGame allocate a large number of
        // AVAudioPlayer / AudioQueue objects at once and can exhaust it. When
        // that happens `alGenSources` sets AL_OUT_OF_MEMORY / AL_INVALID_VALUE
        // and returns no source. Previously we `assert!`ed the error was zero
        // here, which crashed the entire emulator. Apple's AudioToolbox never
        // tears down the process for this; the affected sound just doesn't
        // play. Mirror that: on failure, drop this source and leave the queue
        // silent instead of panicking.
        let err = unsafe {
            // Clear any pre-existing error so we only observe GenSources'.
            let _ = context.GetError();
            context.GenSources(1, &mut al_source);
            context.GetError()
        };
        if err != 0 || al_source == 0 {
            log!(
                "Warning: prime_audio_queue({:?}): could not allocate an OpenAL \
                 source (alGetError() = {:#x}); this audio queue will be silent.",
                in_aq,
                err
            );
            // Best-effort cleanup if a source id was (partially) produced.
            if al_source != 0 {
                unsafe {
                    context.DeleteSources(1, &al_source);
                    let _ = context.GetError();
                }
            }
            return;
        }
        unsafe {
            context.Sourcef(al_source, al::AL_MAX_GAIN, volume);
            let _ = context.GetError();
        };
        apply_al_pan(&context, al_source, pan);
        host_object.al_source = Some(al_source);
    }
    let Some(al_source) = host_object.al_source else {
        return;
    };

    loop {
        let mut al_buffers_queued = 0;
        let mut al_buffers_processed = 0;

        unsafe {
            context.GetSourcei(al_source, al::AL_BUFFERS_QUEUED, &mut al_buffers_queued);
            context.GetSourcei(
                al_source,
                al::AL_BUFFERS_PROCESSED,
                &mut al_buffers_processed,
            );
            let err = context.GetError();
            if err != 0 {
                log!(
                    "Warning: audio queue {:?} OpenAL query failed: {:#x}; skipping refill.",
                    in_aq,
                    err
                );
                break;
            }
        }
        let Ok(al_buffers_queued) = usize::try_from(al_buffers_queued) else {
            log!(
                "Warning: audio queue {:?} reported negative queued-buffer count.",
                in_aq
            );
            break;
        };
        let Ok(al_buffers_processed) = usize::try_from(al_buffers_processed) else {
            log!(
                "Warning: audio queue {:?} reported negative processed-buffer count.",
                in_aq
            );
            break;
        };

        if al_buffers_processed > al_buffers_queued
            || al_buffers_queued > host_object.buffer_queue.len()
        {
            log!("Warning: audio queue {:?} reported inconsistent buffer counts (queued={}, processed={}, tracked={}); skipping refill.", in_aq, al_buffers_queued, al_buffers_processed, host_object.buffer_queue.len());
            break;
        }
        let unprocessed_buffers = al_buffers_queued - al_buffers_processed;

        if unprocessed_buffers >= AUDIO_QUEUE_TARGET_UNPROCESSED_BUFFERS
            || al_buffers_queued == host_object.buffer_queue.len()
        {
            break;
        }

        let next_buffer_idx = al_buffers_queued;
        let next_buffer_ref = host_object.buffer_queue[next_buffer_idx];
        let next_buffer = env.mem.read(next_buffer_ref);

        log_dbg!(
            "Decoding buffer {:?} for queue {:?}",
            next_buffer_ref,
            in_aq
        );

        let next_al_buffer = host_object.al_unused_buffers.pop().unwrap_or_else(|| {
            let mut al_buffer = 0;
            unsafe { context.GenBuffers(1, &mut al_buffer) };
            let err = unsafe { context.GetError() };
            if err != 0 || al_buffer == 0 {
                log!("Warning: audio queue {:?} could not allocate an OpenAL buffer: {:#x}; stopping refill.", in_aq, err);
                return 0;
            }
            al_buffer
        });

        if next_al_buffer == 0 {
            break;
        }

        let (mut al_format, mut al_frequency, mut data) = if matches!(
            host_object.format.format_id,
            kAudioFormatMPEGLayer3 | kAudioFormatMPEG4AAC
        ) {
            decode_compressed_buffer(
                &mut host_object.compressed_pending,
                &mut host_object.compressed_pending_format,
                &mut host_object.compressed_emitted_pcm_bytes,
                &env.mem,
                &host_object.format,
                next_buffer.audio_data.cast(),
                next_buffer.audio_data_byte_size,
            )
        } else {
            decode_buffer_cached(
                &mut host_object.decoded_buffer_cache,
                &env.mem,
                &host_object.format,
                next_buffer.audio_data.cast(),
                next_buffer.audio_data_byte_size,
            )
        };
        if env.options.low_audio_quality {
            (al_frequency, data) = apply_lower_audio_quality(al_format, al_frequency, data);
        }

        if data.is_empty() {
            record_audio_queue_underrun(host_object, in_aq, std::time::Instant::now());
            let silence_format = fallback_al_format(&host_object.format);
            let silence_frames = if host_object.format.format_id == kAudioFormatLinearPCM {
                256usize
            } else {
                512usize
            };
            al_format = silence_format;
            data = silence_buffer(silence_format, silence_frames);
            log_dbg!(
                "AudioQueue {:?}: decoder produced no PCM; queueing {} frames of format {:#x} silence to keep playback alive",
                in_aq,
                silence_frames,
                silence_format
            );
        }

        let supplied_frames = frames_for_al_buffer(al_format, data.len());
        host_object.supplied_frames = host_object.supplied_frames.saturating_add(supplied_frames);

        unsafe {
            context.BufferData(
                next_al_buffer,
                al_format,
                data.as_ptr() as *const ALvoid,
                data.len().try_into().unwrap(),
                al_frequency,
            )
        };

        let error = unsafe {
            context.SourceQueueBuffers(al_source, 1, &next_al_buffer);
            context.GetError()
        };
        if error != 0 {
            log!(
                "Warning: audio queue {:?} could not queue OpenAL buffer: {:#x}; retrying later.",
                in_aq,
                error
            );
            host_object.al_unused_buffers.push(next_al_buffer);
            break;
        }
    }

    if host_object.is_running == AudioQueueIsRunning::Running {
        let mut source_state = 0;
        let mut queued = 0;
        unsafe {
            context.GetSourcei(al_source, al::AL_SOURCE_STATE, &mut source_state);
            context.GetSourcei(al_source, al::AL_BUFFERS_QUEUED, &mut queued);
            let error = context.GetError();
            if error == 0 && queued > 0 && source_state != al::AL_PLAYING {
                context.SourcePlay(al_source);
                let restart_error = context.GetError();
                if restart_error != 0 {
                    log!(
                        "Warning: audio queue {:?} could not restart after an underrun: {:#x}",
                        in_aq,
                        restart_error
                    );
                } else {
                    log!(
                        "AudioQueue {:?}: restarted OpenAL source after underrun (queued={}, previous_state={:#x})",
                        in_aq,
                        queued,
                        source_state
                    );
                }
            } else if error != 0 {
                log!(
                    "Warning: audio queue {:?} source-state query failed: {:#x}",
                    in_aq,
                    error
                );
            }
        }
    }
}

fn unqueue_all_buffers<F: FnMut(ALuint)>(al_source: ALuint, context: &OpenAL<'_>, mut callback: F) {
    loop {
        let mut queued = 0;
        unsafe {
            context.GetSourcei(al_source, al::AL_BUFFERS_QUEUED, &mut queued);
            if context.GetError() != 0 {
                log!("Warning: audio queue OpenAL queued-buffer query failed during reset.");
                break;
            }
        }
        if queued <= 0 {
            break;
        }
        let mut al_buffer = 0;
        unsafe {
            context.SourceUnqueueBuffers(al_source, 1, &mut al_buffer);
            if context.GetError() != 0 || al_buffer == 0 {
                log!("Warning: audio queue could not drain a stopped OpenAL source; leaving the remaining buffers attached.");
                break;
            }
        }
        callback(al_buffer);
    }
}

fn unqueue_buffers<F: FnMut(ALuint)>(al_source: ALuint, context: &OpenAL<'_>, mut callback: F) {
    loop {
        let mut al_buffers_processed = 0;

        unsafe {
            context.GetSourcei(
                al_source,
                al::AL_BUFFERS_PROCESSED,
                &mut al_buffers_processed,
            );
            if context.GetError() != 0 {
                log!("Warning: audio queue OpenAL processed-buffer query failed; stopping this tick.");
                break;
            }
        }
        if al_buffers_processed == 0 {
            break;
        }

        let mut al_buffer = 0;

        unsafe {
            context.SourceUnqueueBuffers(al_source, 1, &mut al_buffer);
            if context.GetError() != 0 || al_buffer == 0 {
                log!("Warning: audio queue OpenAL buffer dequeue failed; stopping this tick.");
                break;
            }
        }

        callback(al_buffer);
    }
}

pub fn handle_audio_queue(env: &mut Environment, in_aq: AudioQueueRef) {
    let needs_source = State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
        .is_some_and(|queue| {
            queue.is_running == AudioQueueIsRunning::Running && queue.al_source.is_none()
        });
    if needs_source {
        prime_audio_queue(env, in_aq);
    }

    let retry_empty_startup_buffers = State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
        .is_some_and(|queue| {
            queue.is_running == AudioQueueIsRunning::Running && queue.buffer_queue.is_empty()
        });
    if retry_empty_startup_buffers {
        // Some older games allocate their buffers before the decoder has
        // produced the first packet. Asking the callback again on the next
        // run-loop tick matches Audio Queue Services' refill behaviour and
        // prevents a one-time empty callback from permanently silencing music.
        ensure_output_buffers(env, in_aq);
    }

    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    // ns_run_loop can still hold a stale `in_aq` for one tick after
    // AudioQueueDispose. Skip silently instead of panicking.
    let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
        return;
    };

    let Some(al_source) = host_object.al_source else {
        return;
    };

    if !is_supported_audio_format(&host_object.format) {
        return;
    }
    if host_object.is_running_handler {
        return;
    }

    host_object.is_running_handler = true;

    let mut buffers_to_reuse = Vec::new();

    unqueue_buffers(al_source, &context, |al_buffer| {
        host_object.al_unused_buffers.push(al_buffer);
        // OpenAL is reporting one buffer as processed, so the queue should
        // be non-empty. If the host and OpenAL views ever desync (e.g.
        // through an unexpected reset), just stop pulling from the empty
        // queue instead of panicking on `.unwrap()`.
        if let Some(buffer_ref) = host_object.buffer_queue.pop_front() {
            buffers_to_reuse.push(buffer_ref);
        } else {
            log!(
                "Warning: handle_audio_queue({:?}): OpenAL reported a processed \
                 buffer but the guest buffer_queue is empty; skipping.",
                in_aq
            );
        }
    });

    let callback_proc = host_object.callback_proc;
    let callback_user_data = host_object.callback_user_data;
    let is_running = host_object.is_running;

    for buffer_ref in buffers_to_reuse.drain(..) {
        let buffer_size = env.mem.read(buffer_ref).audio_data_byte_size;
        if let Some(queue) = State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
        {
            queue.callbacks = queue.callbacks.saturating_add(1);
            queue.requested_frames = queue
                .requested_frames
                .saturating_add(frames_for_audio_bytes(&queue.format, buffer_size as usize));
        }
        log_dbg!(
            "Recyling buffer {:?} for queue {:?}. Calling callback {:?} with user data {:?}.",
            buffer_ref,
            in_aq,
            callback_proc,
            callback_user_data
        );

        let () = callback_proc.call_from_host(env, (callback_user_data, in_aq, buffer_ref));
    }

    prime_audio_queue(env, in_aq);

    // The guest callback we just invoked above is allowed to call
    // AudioQueueDispose on `in_aq`. If it did, the queue is gone and there
    // is nothing left to do here.
    if State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
        .is_none()
    {
        return;
    }

    let context = env
        .framework_state
        .audio_toolbox
        .make_al_context_current(&mut env.openal_manager);

    if is_running != AudioQueueIsRunning::Stopped {
        unsafe {
            let mut al_source_state = 0;

            context.GetSourcei(al_source, al::AL_SOURCE_STATE, &mut al_source_state);
            if context.GetError() == 0 && al_source_state == al::AL_STOPPED {
                context.SourcePlay(al_source);
                if context.GetError() != 0 {
                    log!(
                        "Warning: audio queue {:?} could not restart OpenAL source.",
                        in_aq
                    );
                } else {
                    log_dbg!("Restarted OpenAL source for queue {:?}", in_aq);
                }
            }
        }
    }

    if is_running == AudioQueueIsRunning::Stopping {
        let mut al_source_state = 0;

        unsafe {
            context.GetSourcei(al_source, al::AL_SOURCE_STATE, &mut al_source_state);
            if context.GetError() != 0 {
                log!(
                    "Warning: audio queue {:?} could not query stop state.",
                    in_aq
                );
                return;
            }
        }

        if al_source_state == al::AL_STOPPED {
            log_dbg!(
                "OpenAL source stopped for queue {:?}, completing asynchronous stop.",
                in_aq
            );

            finish_stopping_audio_queue(env, in_aq);
        }
    }

    let state = State::get(&mut env.framework_state);

    if let Some(host_object) = state.audio_queues.get_mut(&in_aq) {
        host_object.is_running_handler = false;
    }
}

fn AudioQueuePrime(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_number_of_frames_to_prepare: u32,
    out_number_of_frames_prepared: MutPtr<u32>,
) -> OSStatus {
    return_if_null!(in_aq);

    prime_audio_queue(env, in_aq);

    if !out_number_of_frames_prepared.is_null() {
        let Some(host_object) = State::get(&mut env.framework_state)
            .audio_queues
            .get(&in_aq)
        else {
            env.mem.write(out_number_of_frames_prepared, 0);
            return 0;
        };

        let mut prepared_frames = 0;
        let format = &host_object.format;

        for &buffer_ref in &host_object.buffer_queue {
            let buffer = env.mem.read(buffer_ref);
            let size = buffer.audio_data_byte_size;

            if format.bytes_per_packet > 0 && format.frames_per_packet > 0 {
                prepared_frames += (size / format.bytes_per_packet) * format.frames_per_packet;
            } else if format.bytes_per_frame > 0 {
                prepared_frames += size / format.bytes_per_frame;
            }
        }

        if in_number_of_frames_to_prepare > 0 && prepared_frames > in_number_of_frames_to_prepare {
            prepared_frames = in_number_of_frames_to_prepare;
        }

        env.mem
            .write(out_number_of_frames_prepared, prepared_frames);
    }

    0 // success
}

fn notify_aq_is_running(env: &mut Environment, in_aq: AudioQueueRef) {
    let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    else {
        return;
    };

    if let (Some(in_proc), Some(in_user_data)) = (
        host_object.aq_is_running_proc,
        host_object.aq_is_running_user_data,
    ) {
        <GuestFunction as CallFromHost<(), (MutVoidPtr, Ptr<OpaqueAudioQueue, true>, u32)>>::
        call_from_host(
            &in_proc, env, (in_user_data, in_aq, kAudioQueueProperty_IsRunning)
        );
    }
}

fn notify_aq_property_listeners(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    property_id: AudioQueuePropertyID,
) {
    let listeners = State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
        .map(|queue| {
            queue
                .property_listeners
                .iter()
                .filter(|(id, _, _)| *id == property_id)
                .copied()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    for (_, proc, user_data) in listeners {
        if proc.addr_with_thumb_bit() != 0 {
            let _: () = proc.call_from_host(env, (user_data, in_aq, property_id));
        }
    }
}

fn preallocate_output_buffers(env: &mut Environment, in_aq: AudioQueueRef, count: usize) {
    let Some(format) = State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
        .filter(|queue| !queue.is_input)
        .map(|queue| queue.format)
    else {
        return;
    };
    let buffer_size = startup_buffer_size(&format);
    let mut allocated = 0;
    for _ in 0..count {
        let audio_data = env.mem.alloc(buffer_size);
        let buffer_ptr = env.mem.alloc_and_write(AudioQueueBuffer {
            audio_data_bytes_capacity: buffer_size,
            audio_data,
            audio_data_byte_size: 0,
            user_data: Ptr::null(),
            packet_description_capacity: 0,
            _packet_descriptions: Ptr::null(),
            _packet_description_count: 0,
        });
        let Some(queue) = State::get(&mut env.framework_state)
            .audio_queues
            .get_mut(&in_aq)
        else {
            env.mem.free(audio_data);
            env.mem.free(buffer_ptr.cast());
            break;
        };
        queue.buffers.push(buffer_ptr);
        allocated += 1;
    }
    if allocated != 0 {
        log!(
            "AudioQueueNewOutput: preallocated {} output buffers of {} bytes for queue {:?}",
            allocated,
            buffer_size,
            in_aq
        );
    }
}

fn startup_buffer_size(format: &AudioStreamBasicDescription) -> u32 {
    if matches!(
        format.format_id,
        kAudioFormatMPEGLayer3 | kAudioFormatMPEG4AAC
    ) {
        return 4096;
    }
    let bytes_per_frame = u64::from(format.bytes_per_frame.max(1));
    let frames = if format.sample_rate.is_finite() && format.sample_rate > 0.0 {
        (format.sample_rate / 50.0).round().clamp(256.0, 4096.0) as u64
    } else {
        1024
    };
    frames
        .saturating_mul(bytes_per_frame)
        .clamp(4096, 32 * 1024) as u32
}

fn ensure_output_buffers(env: &mut Environment, in_aq: AudioQueueRef) {
    let Some((format, callback_proc, callback_user_data, should_allocate, mut buffers)) =
        State::get(&mut env.framework_state)
            .audio_queues
            .get(&in_aq)
            .map(|queue| {
                (
                    queue.format,
                    queue.callback_proc,
                    queue.callback_user_data,
                    !queue.is_input && queue.buffers.is_empty(),
                    queue.buffers.clone(),
                )
            })
    else {
        return;
    };

    if should_allocate {
        const STARTUP_BUFFER_COUNT: usize = 4;
        let buffer_size = startup_buffer_size(&format);
        for _ in 0..STARTUP_BUFFER_COUNT {
            let audio_data = env.mem.alloc(buffer_size);
            let buffer_ptr = env.mem.alloc_and_write(AudioQueueBuffer {
                audio_data_bytes_capacity: buffer_size,
                audio_data,
                audio_data_byte_size: 0,
                user_data: Ptr::null(),
                packet_description_capacity: 0,
                _packet_descriptions: Ptr::null(),
                _packet_description_count: 0,
            });
            let Some(queue) = State::get(&mut env.framework_state)
                .audio_queues
                .get_mut(&in_aq)
            else {
                env.mem.free(audio_data);
                env.mem.free(buffer_ptr.cast());
                return;
            };
            queue.buffers.push(buffer_ptr);
            buffers.push(buffer_ptr);
        }

        log!(
            "AudioQueueStart: allocated {} startup guest buffers of {} bytes for queue {:?}",
            STARTUP_BUFFER_COUNT,
            buffer_size,
            in_aq
        );
    }

    let is_input = State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
        .is_some_and(|queue| queue.is_input);
    if is_input {
        return;
    }

    // A number of older games allocate their output buffers before starting
    // the queue, then expect AudioQueueStart to make the first output callback.
    // The previous implementation only did that for buffers allocated by our
    // fallback path, so a queue could start with two guest buffers but zero
    // OpenAL buffers. OpenAL then stopped immediately and background music was
    // silent until the app happened to refill the queue later.
    for buffer_ptr in buffers {
        let should_callback = State::get(&mut env.framework_state)
            .audio_queues
            .get(&in_aq)
            .is_some_and(|queue| {
                queue.buffers.contains(&buffer_ptr) && !queue.buffer_queue.contains(&buffer_ptr)
            });
        if should_callback && callback_proc.addr_with_thumb_bit() != 0 {
            let () = callback_proc.call_from_host(env, (callback_user_data, in_aq, buffer_ptr));
            if State::get(&mut env.framework_state)
                .audio_queues
                .get(&in_aq)
                .is_none()
            {
                return;
            }
            let produced_bytes = env.mem.read(buffer_ptr).audio_data_byte_size;
            if produced_bytes > 0 {
                if let Some(queue) = State::get(&mut env.framework_state)
                    .audio_queues
                    .get_mut(&in_aq)
                {
                    queue.callbacks = queue.callbacks.saturating_add(1);
                    queue.requested_frames =
                        queue
                            .requested_frames
                            .saturating_add(frames_for_audio_bytes(
                                &queue.format,
                                produced_bytes as usize,
                            ));
                }
            } else {
                log!(
                    "Warning: AudioQueueStart({:?}) callback left startup buffer {:?} empty",
                    in_aq,
                    buffer_ptr
                );
            }
        }

        let should_enqueue = State::get(&mut env.framework_state)
            .audio_queues
            .get(&in_aq)
            .is_some_and(|queue| {
                queue.buffers.contains(&buffer_ptr)
                    && !queue.buffer_queue.contains(&buffer_ptr)
                    && env.mem.read(buffer_ptr).audio_data_byte_size > 0
            });
        if should_enqueue {
            if let Some(queue) = State::get(&mut env.framework_state)
                .audio_queues
                .get_mut(&in_aq)
            {
                queue.buffer_queue.push_back(buffer_ptr);
            }
        }
    }

    log_audio_queue_state(env, "startup buffer priming");
}

pub fn log_audio_queue_state(env: &mut Environment, label: &str) {
    let state = State::get(&mut env.framework_state);
    let active_sources = state
        .audio_queues
        .values()
        .filter(|queue| queue.al_source.is_some())
        .count();
    log!(
        "AudioQueue lifecycle [{}]: {} queue(s), {} active OpenAL source(s), active_output={:?}",
        label,
        state.audio_queues.len(),
        active_sources,
        state.active_output
    );
    for (index, (queue_ref, queue)) in state.audio_queues.iter().enumerate() {
        let status = match queue.is_running {
            AudioQueueIsRunning::Running => "RUNNING",
            AudioQueueIsRunning::Stopping => "STOPPING",
            AudioQueueIsRunning::Stopped => "STOPPED",
        };
        let sample_rate = queue.format.sample_rate;
        let channels = queue.format.channels_per_frame;
        log!(
            "AudioQueue state [{}] #{}: queue={:?} status={} guest_buffers={} guest_queued={} reusable_openal_buffers={} source={:?} rate={:.1}Hz channels={} callbacks={} underruns={}",
            label,
            index,
            queue_ref,
            status,
            queue.buffers.len(),
            queue.buffer_queue.len(),
            queue.al_unused_buffers.len(),
            queue.al_source,
            sample_rate,
            channels,
            queue.callbacks,
            queue.underruns,
        );
        if queue.buffers.is_empty() {
            log!(
                "AudioQueue state [{}]: queue={:?} has no guest buffers; playback cannot receive data until a buffer is allocated",
                label,
                queue_ref
            );
        }
    }
}

pub fn AudioQueueStart(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    _in_device_start_time: ConstVoidPtr,
) -> OSStatus {
    return_if_null!(in_aq);

    let already_running = State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
        .is_some_and(|queue| queue.is_running == AudioQueueIsRunning::Running);
    if already_running {
        log!(
            "AudioQueueStart({:?}) ignored because the queue is already running",
            in_aq
        );
        log_audio_queue_state(env, "duplicate start ignored");
        return 0;
    }

    let supported = State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
        .is_some_and(|queue| is_supported_audio_format(&queue.format));
    if !supported {
        log!(
            "Warning: AudioQueueStart({:?}) on an unknown queue or unsupported format",
            in_aq
        );
        return kAudioQueueErr_InvalidProperty;
    }

    if let Some(queue) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    {
        queue.is_running = AudioQueueIsRunning::Running;
    }

    ensure_output_buffers(env, in_aq);
    if State::get(&mut env.framework_state)
        .audio_queues
        .get(&in_aq)
        .is_none()
    {
        return kAudioQueueErr_InvalidProperty;
    }
    prime_audio_queue(env, in_aq);

    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);
    let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
        return kAudioQueueErr_InvalidProperty;
    };

    let Some(al_source) = host_object.al_source else {
        host_object.is_running = AudioQueueIsRunning::Stopped;
        log!(
            "Warning: AudioQueueStart({:?}) could not create an OpenAL source",
            in_aq
        );
        return 0;
    };

    let mut queued = 0;
    unsafe {
        context.GetSourcei(al_source, al::AL_BUFFERS_QUEUED, &mut queued);
        if context.GetError() != 0 {
            queued = 0;
        }
        if queued > 0 {
            context.SourcePlay(al_source);
            if context.GetError() != 0 {
                log!(
                    "Warning: AudioQueueStart({:?}) could not start OpenAL source",
                    in_aq
                );
            }
        } else {
            log!(
                "Warning: AudioQueueStart({:?}) has no decoded buffers after the initial callback",
                in_aq
            );
        }
    }

    log!(
        "AudioQueueStart: queue={:?}, guest_buffers={}, queued_buffers={}, openal_source={:?}",
        in_aq,
        host_object.buffers.len(),
        host_object.buffer_queue.len(),
        host_object.al_source
    );
    notify_aq_is_running(env, in_aq);
    log_audio_queue_state(env, "AudioQueueStart");

    0 // success
}

pub fn AudioQueuePause(env: &mut Environment, in_aq: AudioQueueRef) -> OSStatus {
    return_if_null!(in_aq);

    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
        return 0;
    };

    host_object.is_running = AudioQueueIsRunning::Stopped;

    if let Some(al_source) = host_object.al_source {
        unsafe {
            context.SourcePause(al_source);
            if context.GetError() != 0 {
                log!(
                    "Warning: AudioQueuePause({:?}) could not pause OpenAL source.",
                    in_aq
                );
            }
        }
    }

    0 // success
}

fn finish_stopping_audio_queue(env: &mut Environment, in_aq: AudioQueueRef) {
    AudioQueueReset(env, in_aq);
    if let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    {
        host_object.is_running = AudioQueueIsRunning::Stopped;
    }

    notify_aq_is_running(env, in_aq);
}

pub fn AudioQueueStop(env: &mut Environment, in_aq: AudioQueueRef, in_immediate: bool) -> OSStatus {
    return_if_null!(in_aq);

    if in_immediate {
        log_dbg!("Performing immediate AudioQueueStop for {:?}.", in_aq);

        let (state, context) =
            State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

        let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
            return 0;
        };
        if let Some(al_source) = host_object.al_source {
            unsafe {
                context.SourceStop(al_source);
                if context.GetError() != 0 {
                    log!(
                        "Warning: AudioQueueStop({:?}) could not stop OpenAL source.",
                        in_aq
                    );
                }
            }
        };

        finish_stopping_audio_queue(env, in_aq);
    } else {
        let state = State::get(&mut env.framework_state);

        let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
            return 0;
        };
        if host_object.is_running != AudioQueueIsRunning::Stopped {
            log_dbg!("Starting asynchronous AudioQueueStop for {:?}.", in_aq);

            host_object.is_running = AudioQueueIsRunning::Stopping;
        } else {
            log_dbg!(
                "Ignoring asynchronous AudioQueueStop for {:?} (already stopped).",
                in_aq
            );
        }
    }

    log_audio_queue_state(env, "AudioQueueStop");
    0 // success
}

fn AudioQueueReset(env: &mut Environment, in_aq: AudioQueueRef) -> OSStatus {
    return_if_null!(in_aq);

    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    log_dbg!("Resetting queue {:?}.", in_aq);

    let Some(host_object) = state.audio_queues.get_mut(&in_aq) else {
        return 0;
    };

    if let Some(al_source) = host_object.al_source {
        unsafe {
            let mut al_source_state = 0;

            context.GetSourcei(al_source, al::AL_SOURCE_STATE, &mut al_source_state);
            let error = context.GetError();
            if error != 0 {
                log!("Warning: audio queue {:?} reset state query failed: {:#x}; continuing cleanup.", in_aq, error);
            } else if al_source_state != al::AL_STOPPED {
                context.SourceStop(al_source);
                let error = context.GetError();
                if error != 0 {
                    log!(
                        "Warning: audio queue {:?} reset stop failed: {:#x}; continuing cleanup.",
                        in_aq,
                        error
                    );
                }
            }
        }

        unqueue_all_buffers(al_source, &context, |al_buffer| {
            host_object.al_unused_buffers.push(al_buffer);
            if host_object.buffer_queue.pop_front().is_none() {
                log!(
                    "Warning: audio queue {:?} reset found an untracked OpenAL buffer.",
                    in_aq
                );
            }
        });
    }

    host_object.buffer_queue.clear();
    host_object.decoded_buffer_cache.clear();
    host_object.compressed_pending.clear();
    host_object.compressed_pending_format = None;
    host_object.compressed_emitted_pcm_bytes = 0;

    0 // success
}

fn AudioQueueFlush(env: &mut Environment, in_aq: AudioQueueRef) -> OSStatus {
    return_if_null!(in_aq);
    AudioQueueReset(env, in_aq)
}

fn AudioQueueFreeBuffer(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    in_buffer: AudioQueueBufferRef,
) -> OSStatus {
    return_if_null!(in_aq);

    let Some(host_object) = State::get(&mut env.framework_state)
        .audio_queues
        .get_mut(&in_aq)
    else {
        return kAudioQueueErr_InvalidBuffer;
    };

    if host_object.buffer_queue.contains(&in_buffer) {
        return kAudioQueueErr_BufferInQueue;
    }

    if let Some(index) = host_object.buffers.iter().position(|x| x == &in_buffer) {
        host_object.buffers.remove(index);

        log_dbg!("Freeing buffer: {:?}", in_buffer);

        let buffer = env.mem.read(in_buffer);
        env.mem.free(buffer.audio_data);
        env.mem.free(in_buffer.cast());

        0 // success
    } else {
        kAudioQueueErr_InvalidBuffer
    }
}

pub fn AudioQueueDispose(
    env: &mut Environment,
    in_aq: AudioQueueRef,
    _in_immediate: bool,
) -> OSStatus {
    return_if_null!(in_aq);

    let (state, context) =
        State::get_with_context(&mut env.framework_state, &mut env.openal_manager);

    let Some(mut host_object) = state.audio_queues.remove(&in_aq) else {
        // Disposing a queue that was never created (or was already disposed)
        // is a no-op; don't panic and don't double-free the OpaqueAudioQueue
        // pointer.
        log_dbg!(
            "AudioQueueDispose({:?}) ignored: queue is unknown / already \
             disposed.",
            in_aq
        );
        return 0;
    };
    if state.active_output == Some(in_aq) {
        state.active_output = None;
    }
    log_dbg!("Disposing of audio queue {:?}", in_aq);

    env.mem.free(in_aq.cast());

    for buffer_ptr in host_object.buffers {
        let buffer = env.mem.read(buffer_ptr);
        env.mem.free(buffer.audio_data);
        env.mem.free(buffer_ptr.cast());
    }

    if let Some(al_source) = host_object.al_source {
        unsafe {
            context.SourceStop(al_source);
            let error = context.GetError();
            if error != 0 {
                log!(
                    "Warning: audio queue {:?} dispose stop failed: {:#x}; continuing cleanup.",
                    in_aq,
                    error
                );
            }
        }

        unqueue_all_buffers(al_source, &context, |al_buffer| {
            host_object.al_unused_buffers.push(al_buffer)
        });

        unsafe {
            context.DeleteBuffers(
                host_object.al_unused_buffers.len().try_into().unwrap(),
                host_object.al_unused_buffers.as_ptr(),
            );
            let error = context.GetError();
            if error != 0 {
                log!(
                    "Warning: audio queue {:?} dispose buffer cleanup failed: {:#x}.",
                    in_aq,
                    error
                );
            }
        }

        // Free the OpenAL source back to the (finite) source pool. OpenAL Soft
        // only provides a limited number of sources (mono_sources +
        // stereo_sources), unlike a real Audio Queue which is backed by the
        // system audio hardware. Previously the source was never deleted here,
        // so every disposed AudioQueue / AVAudioPlayer leaked one source.
        // Games that allocate many short-lived AVAudioPlayers for sound
        // effects (e.g. BAROQUE) eventually exhausted the pool: alGenSources
        // then failed with AL_OUT_OF_MEMORY (0xA005), all further audio went
        // silent, and the game aborted ("これ以上オーディを再生できません").
        //
        // Apple's AudioQueueDispose documents that it "disposes of an audio
        // queue object and all of its resources"
        // <https://developer.apple.com/documentation/audiotoolbox/audioqueuedispose(_:_:)>,
        // so releasing the source here matches the documented behaviour.
        unsafe {
            context.DeleteSources(1, &al_source);
            let error = context.GetError();
            if error != 0 {
                log!(
                    "Warning: audio queue {:?} dispose source cleanup failed: {:#x}.",
                    in_aq,
                    error
                );
            }
        }
        host_object.al_source = None;
    }

    ns_run_loop::remove_audio_queue(env, host_object.run_loop, in_aq);

    0 // success
}

pub fn AudioQueueNewInput(
    env: &mut Environment,
    in_format: ConstPtr<AudioStreamBasicDescription>,
    in_callback_proc: AudioQueueOutputCallback,
    in_user_data: MutVoidPtr,
    in_callback_run_loop: CFRunLoopRef,
    _in_callback_run_loop_mode: CFRunLoopMode,
    in_flags: u32,
    out_aq: MutPtr<AudioQueueRef>,
) -> OSStatus {
    log!("AudioQueueNewInput: using deterministic silent microphone input");

    if in_flags != 0 {
        log!(
            "Warning: AudioQueueNewInput ignoring unexpected flags {:#x}.",
            in_flags
        );
    }

    let in_callback_run_loop = if in_callback_run_loop.is_null() {
        CFRunLoopGetMain(env)
    } else {
        in_callback_run_loop
    };

    let format = env.mem.read(in_format);

    let host_object = AudioQueueHostObject {
        format,
        callback_proc: in_callback_proc,
        callback_user_data: in_user_data,
        run_loop: in_callback_run_loop,
        volume: 1.0,
        pan: 0.0,
        buffers: Vec::new(),
        buffer_queue: VecDeque::new(),
        is_running: AudioQueueIsRunning::Stopped,
        al_source: None,
        al_unused_buffers: Vec::new(),
        decoded_buffer_cache: VecDeque::new(),
        compressed_pending: Vec::new(),
        compressed_pending_format: None,
        compressed_emitted_pcm_bytes: 0,
        aq_is_running_proc: None,
        aq_is_running_user_data: None,
        property_listeners: Vec::new(),
        callbacks: 0,
        requested_frames: 0,
        supplied_frames: 0,
        underruns: 0,
        last_underrun_log: None,
        is_running_handler: false,
        is_input: true,
        input_delay: 0,
        hardware_codec_policy: codec_policy::DEFAULT,
        offline_render_format: None,
    };

    let aq_ref = env.mem.alloc_and_write(OpaqueAudioQueue { _filler: 0 });
    State::get(&mut env.framework_state)
        .audio_queues
        .insert(aq_ref, host_object);

    if !out_aq.is_null() {
        env.mem.write(out_aq, aq_ref);
    }

    ns_run_loop::add_audio_queue(env, in_callback_run_loop, aq_ref);

    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(AudioQueueNewOutput(_, _, _, _, _, _, _)),
    export_c_func!(AudioQueueGetParameter(_, _, _)),
    export_c_func!(AudioQueueSetParameter(_, _, _)),
    export_c_func!(AudioQueueAllocateBufferWithPacketDescriptions(_, _, _, _)),
    export_c_func!(AudioQueueAllocateBuffer(_, _, _)),
    export_c_func!(AudioQueueEnqueueBuffer(_, _, _, _)),
    export_c_func!(AudioQueueEnqueueBufferWithParameters(
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _
    )),
    export_c_func!(AudioQueueAddPropertyListener(_, _, _, _)),
    export_c_func!(AudioQueueRemovePropertyListener(_, _, _, _)),
    export_c_func!(AudioQueueGetPropertySize(_, _, _)),
    export_c_func!(AudioQueueGetProperty(_, _, _, _)),
    export_c_func!(AudioQueueSetProperty(_, _, _, _)),
    export_c_func!(AudioQueueSetOfflineRenderFormat(_, _, _)),
    export_c_func!(AudioQueuePrime(_, _, _)),
    export_c_func!(AudioQueueStart(_, _)),
    export_c_func!(AudioQueuePause(_)),
    export_c_func!(AudioQueueStop(_, _)),
    export_c_func!(AudioQueueReset(_)),
    export_c_func!(AudioQueueFlush(_)),
    export_c_func!(AudioQueueFreeBuffer(_, _)),
    export_c_func!(AudioQueueDispose(_, _)),
    export_c_func!(AudioQueueNewInput(_, _, _, _, _, _, _)),
];
