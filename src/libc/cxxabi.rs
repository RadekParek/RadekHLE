/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `cxxabi.h` and the SjLj exception unwinder.
//!
//! Resources:
//! - [Itanium C++ ABI specification](https://itanium-cxx-abi.github.io/cxx-abi/abi.html)
//! - [SjLj-style exception unwinding overview](https://gcc.gnu.org/wiki/SjLjEH)

use crate::abi::{GuestFunction, FRAME_POINTER};
use crate::cpu::Cpu;
use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstPtr, ConstVoidPtr, GuestUSize, MutPtr, MutVoidPtr, Ptr};
use crate::Environment;
use std::sync::Mutex;

// === atexit / finalize ===

static ATEXIT_HANDLERS: Mutex<Vec<(GuestFunction, MutVoidPtr, MutVoidPtr)>> =
    Mutex::new(Vec::new());

fn __cxa_atexit(_env: &mut Environment, func: GuestFunction, p: MutVoidPtr, d: MutVoidPtr) -> i32 {
    if let Ok(mut handlers) = ATEXIT_HANDLERS.lock() {
        handlers.push((func, p, d));
        0
    } else {
        -1
    }
}

fn __cxa_finalize(_env: &mut Environment, d: MutVoidPtr) {
    let mut to_run = Vec::new();
    if let Ok(mut handlers) = ATEXIT_HANDLERS.lock() {
        if d.is_null() {
            to_run = handlers.drain(..).collect();
        } else {
            let mut i = 0;
            while i < handlers.len() {
                if handlers[i].2 == d {
                    to_run.push(handlers.remove(i));
                } else {
                    i += 1;
                }
            }
        }
    }
    for (_func, _p, _d) in to_run.into_iter().rev() {
        // touchHLE relies on host-process exit for cleanup; we just drop
        // the registered destructors.
    }
}

// === C++ Itanium ABI: guard variables for static initialisation ===
//
// `__cxa_guard_acquire(guard*)` returns 1 if the guarded static still
// needs to be initialised, 0 if it's already initialised. After a
// successful initialisation the caller invokes `__cxa_guard_release`.
//
// On 32-bit ARM the guard is a 64-bit object whose first byte is the
// "initialised" flag. touchHLE is single-threaded for static init so
// we don't need locking. We pre-mark it 1 so a re-entrant call sees
// "already done".

fn __cxa_guard_acquire(env: &mut Environment, guard: MutPtr<u8>) -> i32 {
    let initialized = env.mem.read(guard);
    if initialized != 0 {
        0
    } else {
        env.mem.write(guard, 1);
        1
    }
}

fn __cxa_guard_release(env: &mut Environment, guard: MutPtr<u8>) {
    env.mem.write(guard, 1);
}

fn __cxa_guard_abort(env: &mut Environment, guard: MutPtr<u8>) {
    env.mem.write(guard, 0);
}

// === SjLj exception bypass ===
//
// touchHLE has no real C++ unwinder. Implementing one means parsing
// .gcc_except_table LSDAs, walking the SjLj jmpbuf chain and dispatching
// to the right `catch` clause. That's a multi-week project.
//
// Instead, when a guest exception is thrown we walk the ARM frame-pointer
// chain looking for a return address that lives inside the user code
// segment (below 0x10000000 — guest binaries always load there; the
// system dylibs are mapped at >= 0x38000000). When we find one, we treat
// that frame as if it caught the exception: restore SP and FP for that
// frame, set R0=0 (the "no exception in flight" return) and branch to
// LR. Effectively we make the throwing function return to the first
// app-level frame above it.
//
// This is wrong in the strict sense — destructors of automatic objects
// in skipped frames don't run and the caller's local state may be inconsistent
// — but it lets games that
// throw recoverable errors (parse failures, missing assets, etc.) keep
// running instead of crashing on a NULL-page indirect call.

const APP_CODE_LIMIT: u32 = 0x1000_0000;

fn unwind_to_app_frame(env: &mut Environment) -> bool {
    // The thread's stack typically lives at the top of the 4 GiB guest
    // address space (e.g. SP ≈ 0xffffee40). Use the recorded stack range
    // when available — otherwise fall back to "any non-zero, non-all-ones
    // address that's 4-byte aligned".
    let stack_range = env
        .threads
        .get(env.current_thread)
        .and_then(|t| t.stack.clone());

    let mut fp = env.cpu.regs()[FRAME_POINTER];
    let return_to_host = env.dyld.return_to_host_routine().addr_with_thumb_bit();
    let thread_exit = env.dyld.thread_exit_routine().addr_with_thumb_bit();

    for _ in 0..64 {
        if fp == 0 || fp == 0xffff_ffff || (fp & 3) != 0 {
            break;
        }
        if let Some(ref r) = stack_range {
            if !r.contains(&fp) {
                break;
            }
        }
        let prev_fp: u32 = env.mem.read(ConstPtr::<u32>::from_bits(fp));
        let lr: u32 = env.mem.read(ConstPtr::<u32>::from_bits(fp + 4));
        let lr_no_thumb = lr & !1;
        // Skip frames where LR is one of touchHLE's host trampoline
        // sentinels (return-to-host / thread-exit). Those mark the
        // boundary between host and guest code; unwinding past them
        // would dump us back into the wrong place.
        let is_host_trampoline = lr == return_to_host || lr == thread_exit;
        if !is_host_trampoline && lr_no_thumb > 0 && lr_no_thumb < APP_CODE_LIMIT {
            let regs = env.cpu.regs_mut();
            regs[FRAME_POINTER] = prev_fp;
            regs[Cpu::SP] = fp + 8;
            regs[0] = 0;
            env.cpu.branch(GuestFunction::from_addr_with_thumb_bit(lr));
            return true;
        }
        fp = prev_fp;
    }
    false
}

fn terminate_current_thread_after_exception(env: &mut Environment, reason: &str) {
    log!(
        "Warning: terminating guest thread {} after unrecoverable C++ exception loop ({})",
        env.current_thread,
        reason
    );
    let thread_exit = env.dyld.thread_exit_routine();
    let regs = env.cpu.regs_mut();
    regs[0] = 0;
    regs[Cpu::LR] = thread_exit.addr_with_thumb_bit();
    env.cpu.branch(thread_exit);
}

// === Exception-loop detection (shared) ===
//
// touchHLE's exception "bypass" can return control to a caller that
// immediately re-throws (classic example: `operator new` in a loop that
// keeps getting NULL from a refused huge `malloc`, throwing `bad_alloc`
// every iteration — see P. Harvest, which spins on malloc(0x4420000c)).
// Both the Itanium (`__cxa_throw`) and the SjLj (`_Unwind_SjLj_*`) entry
// points funnel through here so neither can hang the emulator forever.
//
// Returns the number of consecutive throws that share the same `key`.

const THROW_LOOP_LIMIT: u32 = 512;

fn note_exception_throw(key: &str) -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static LAST_THROW_KEY: Mutex<String> = Mutex::new(String::new());
    static SAME_KEY_COUNT: AtomicU32 = AtomicU32::new(0);

    let mut last = LAST_THROW_KEY.lock().unwrap();
    if *last == key {
        SAME_KEY_COUNT.fetch_add(1, Ordering::Relaxed) + 1
    } else {
        *last = key.to_owned();
        SAME_KEY_COUNT.store(1, Ordering::Relaxed);
        1
    }
}

// === C++ Itanium ABI: exception machinery ===
//
// We allocate the requested storage prefixed by a fake __cxa_exception
// header, so that pointer arithmetic in the app's exception-handling
// code (ABI offsets, exception_class field, etc.) lands inside live
// memory. The compatibility unwinder bypasses guest catch frames, so release
// the storage when the throw is handled to avoid a retry loop exhausting the
// guest heap.

const CXA_EXCEPTION_HEADER_SIZE: GuestUSize = 0x60;

fn __cxa_allocate_exception(env: &mut Environment, thrown_size: GuestUSize) -> MutVoidPtr {
    let Some(total_size) = CXA_EXCEPTION_HEADER_SIZE.checked_add(thrown_size) else {
        log!(
            "Warning: __cxa_allocate_exception size overflow for {:#x} bytes; returning NULL",
            thrown_size
        );
        return MutVoidPtr::null();
    };
    let block: MutVoidPtr = env.mem.alloc(total_size);
    if block.is_null() {
        log_once_fmt!(
            "Warning: __cxa_allocate_exception could not allocate {:#x} bytes; returning NULL; repeated failures are suppressed",
            total_size
        );
        return MutVoidPtr::null();
    }
    Ptr::from_bits(block.to_bits() + CXA_EXCEPTION_HEADER_SIZE)
}

fn free_exception_storage(env: &mut Environment, thrown: MutVoidPtr) {
    let Some(base) = thrown.to_bits().checked_sub(CXA_EXCEPTION_HEADER_SIZE) else {
        return;
    };
    if env.mem.is_known_allocation(base) {
        env.mem.free(Ptr::from_bits(base));
    }
}

fn __cxa_free_exception(env: &mut Environment, thrown: MutVoidPtr) {
    free_exception_storage(env, thrown);
}

fn __cxa_decrement_exception_refcount(_env: &mut Environment, _exception: MutVoidPtr) {}

fn __cxa_increment_exception_refcount(_env: &mut Environment, _exception: MutVoidPtr) {}

fn __cxa_throw(env: &mut Environment, exc: MutVoidPtr, tinfo: ConstVoidPtr, _dtor: GuestFunction) {
    // Itanium type_info layout (32-bit):
    //   +0  vptr
    //   +4  const char *name
    let type_name = if !tinfo.is_null() {
        let name_field: ConstPtr<ConstPtr<u8>> = tinfo.cast();
        let name_ptr: ConstPtr<u8> = env.mem.read(name_field + 1);
        if !name_ptr.is_null() {
            env.mem
                .cstr_at_utf8(name_ptr)
                .unwrap_or("(non-utf8)")
                .to_owned()
        } else {
            "(unknown)".to_owned()
        }
    } else {
        "(null type_info)".to_owned()
    };

    free_exception_storage(env, exc);

    // Throw-rate limiter: if the app enters an exception loop (e.g. because
    // our SjLj bypass returns it to a `while (true) new X;` path that throws
    // again the next iteration), we'll silently burn CPU forever. Track the
    // rate of throws of the same type and abort after a reasonable ceiling so
    // the emulator stays responsive.
    let count = note_exception_throw(&type_name);

    if count <= 3 || count.is_multiple_of(64) {
        log!(
            "Guest threw a C++ exception of type {:?} (consecutive #{}): \
             touchHLE has no real unwinder, so we unwind to the nearest \
             app-level frame via the frame-pointer chain.",
            type_name,
            count
        );
    }
    if count >= THROW_LOOP_LIMIT {
        log!(
            "Warning: Exception loop detected: {} threw {} times in a row; \
             terminating only the current guest thread instead of aborting the \
             emulator.",
            type_name,
            count
        );
        terminate_current_thread_after_exception(env, &type_name);
        return;
    }

    if !unwind_to_app_frame(env) {
        log!(
            "Warning: Could not unwind past C++ exception ({}); no app-level \
             frame on the stack; terminating only the current guest thread.",
            type_name
        );
        terminate_current_thread_after_exception(env, &type_name);
    }
}

fn __cxa_rethrow(env: &mut Environment) {
    log_once!("__cxa_rethrow — bypassing; repeated calls are suppressed");
    if !unwind_to_app_frame(env) {
        log!(
            "Warning: Could not unwind past __cxa_rethrow; no app-level frame; \
             terminating only the current guest thread."
        );
        terminate_current_thread_after_exception(env, "__cxa_rethrow");
    }
}

fn __cxa_begin_catch(_env: &mut Environment, exception_obj: MutVoidPtr) -> MutVoidPtr {
    // Return the exception object unchanged. Combined with the unwind
    // bypass above, no frame ever actually reaches __cxa_begin_catch
    // unless someone called it manually; in that case keep the value
    // sane.
    exception_obj
}

fn __cxa_end_catch(_env: &mut Environment) {}

fn __cxa_pure_virtual(env: &mut Environment) {
    log!("Pure virtual function called — vtable slot was NULL. Bypassing.");
    if !unwind_to_app_frame(env) {
        log!(
            "Warning: Pure virtual function called and no recoverable frame; \
             returning to caller. Guest will likely abort."
        );
    }
}

/// `__cxa_uncaught_exception` (Itanium C++ ABI; also re-exported by
/// libc++abi). Returns whether an exception is currently in flight
/// ("thrown but not yet caught"). touchHLE's exception bypass never
/// leaves an exception in flight after `__cxa_throw` returns control to
/// an app frame, so the truthful answer is `false`. libstdc++/libc++
/// use this in `std::uncaught_exception()` and in stream/destructor
/// guards.
fn __cxa_uncaught_exception(_env: &mut Environment) -> bool {
    false
}

fn __cxa_call_unexpected(env: &mut Environment, _exc: MutVoidPtr) {
    log!("__cxa_call_unexpected — bypassing");
    if !unwind_to_app_frame(env) {
        log!(
            "Warning: __cxa_call_unexpected with no recoverable frame; \
             returning to caller. Guest will likely abort."
        );
    }
}

/// Itanium ABI `__dynamic_cast`:
///
/// ```c
/// void *__dynamic_cast(const void *src,
///                      const __class_type_info *src_type,
///                      const __class_type_info *dst_type,
///                      ptrdiff_t src2dst_offset);
/// ```
///
/// Returns the casted pointer on success, or NULL on failure (the cast
/// does not apply / a `dynamic_cast<T*>` should evaluate to nullptr).
///
/// The emulator parses the guest's bounded Itanium RTTI hierarchy to support
/// public upcasts, downcasts, and cross-casts. Malformed or excessive RTTI is
/// treated as a failed cast rather than a host crash.
const MAX_RTTI_SUBOBJECTS: usize = 4096;
const MAX_RTTI_BASES: u32 = 512;
const MAX_RTTI_DEPTH: u8 = 64;

#[derive(Clone, Copy)]
struct RttiSubobject {
    type_info: u32,
    object: u32,
    parent: Option<usize>,
    public_from_parent: bool,
    public_from_root: bool,
    depth: u8,
}

fn read_guest_u32(env: &Environment, address: u32) -> Option<u32> {
    if address < env.mem.null_segment_size() || address & 3 != 0 {
        return None;
    }
    let bytes = env
        .mem
        .get_bytes_fallible(ConstVoidPtr::from_bits(address), 4)?;
    Some(u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?))
}

fn guest_add_signed(base: u32, offset: i32) -> Option<u32> {
    let address = i64::from(base).checked_add(i64::from(offset))?;
    (0..=i64::from(u32::MAX))
        .contains(&address)
        .then_some(address as u32)
}

fn rtti_type_name(env: &Environment, type_info: u32) -> Option<&str> {
    let name_ptr = read_guest_u32(env, type_info.checked_add(4)?)?;
    if name_ptr < env.mem.null_segment_size() {
        return None;
    }
    let bytes = env
        .mem
        .get_bytes_fallible(ConstVoidPtr::from_bits(name_ptr), 256)?;
    let length = bytes.iter().position(|byte| *byte == 0)?;
    std::str::from_utf8(&bytes[..length]).ok()
}

fn rtti_types_equal(env: &Environment, left: u32, right: u32) -> bool {
    left == right
        || matches!(
            (rtti_type_name(env, left), rtti_type_name(env, right)),
            (Some(left), Some(right)) if left == right
        )
}

fn rtti_typeinfo_kind(
    env: &Environment,
    type_info: u32,
) -> Option<crate::dyld::CxxAbiTypeInfoKind> {
    let vtable = read_guest_u32(env, type_info)?;
    env.dyld.cxxabi_typeinfo_kind(vtable)
}

fn rtti_base_object_address(env: &Environment, derived: u32, offset_flags: u32) -> Option<u32> {
    let flags = offset_flags & 0xff;
    let offset = (offset_flags as i32) >> 8;
    if flags & 1 == 0 {
        return guest_add_signed(derived, offset);
    }
    let vtable = read_guest_u32(env, derived)?;
    let virtual_offset_address = guest_add_signed(vtable, offset)?;
    let virtual_offset = read_guest_u32(env, virtual_offset_address)? as i32;
    guest_add_signed(derived, virtual_offset)
}

fn push_rtti_subobject(
    nodes: &mut Vec<RttiSubobject>,
    type_info: u32,
    object: u32,
    parent: usize,
    is_public: bool,
) -> Option<()> {
    if nodes.len() >= MAX_RTTI_SUBOBJECTS {
        return None;
    }
    let parent_node = nodes.get(parent)?;
    if parent_node.depth >= MAX_RTTI_DEPTH {
        return None;
    }
    let public_from_root = parent_node.public_from_root && is_public;
    let depth = parent_node.depth + 1;
    nodes.push(RttiSubobject {
        type_info,
        object,
        parent: Some(parent),
        public_from_parent: is_public,
        public_from_root,
        depth,
    });
    Some(())
}

fn rtti_subobjects(
    env: &Environment,
    dynamic_type: u32,
    dynamic_object: u32,
) -> Option<Vec<RttiSubobject>> {
    use crate::dyld::CxxAbiTypeInfoKind;

    let mut nodes = vec![RttiSubobject {
        type_info: dynamic_type,
        object: dynamic_object,
        parent: None,
        public_from_parent: true,
        public_from_root: true,
        depth: 0,
    }];
    let mut index = 0;
    while index < nodes.len() {
        let node = nodes[index];
        match rtti_typeinfo_kind(env, node.type_info)? {
            CxxAbiTypeInfoKind::Class => {}
            CxxAbiTypeInfoKind::SingleInheritance => {
                let base_type = read_guest_u32(env, node.type_info.checked_add(8)?)?;
                push_rtti_subobject(&mut nodes, base_type, node.object, index, true)?;
            }
            CxxAbiTypeInfoKind::MultipleInheritance => {
                let base_count = read_guest_u32(env, node.type_info.checked_add(12)?)?;
                if base_count > MAX_RTTI_BASES {
                    return None;
                }
                for base_index in 0..base_count {
                    let entry = node
                        .type_info
                        .checked_add(16)?
                        .checked_add(base_index.checked_mul(8)?)?;
                    let base_type = read_guest_u32(env, entry)?;
                    let offset_flags = read_guest_u32(env, entry.checked_add(4)?)?;
                    let base_object = rtti_base_object_address(env, node.object, offset_flags)?;
                    push_rtti_subobject(
                        &mut nodes,
                        base_type,
                        base_object,
                        index,
                        offset_flags & 2 != 0,
                    )?;
                }
            }
        }
        index += 1;
    }
    Some(nodes)
}

fn rtti_path_is_public(nodes: &[RttiSubobject], source: usize, target: usize) -> bool {
    let mut current = target;
    while current != source {
        let Some(node) = nodes.get(current) else {
            return false;
        };
        if !node.public_from_parent {
            return false;
        }
        let Some(parent) = node.parent else {
            return false;
        };
        current = parent;
    }
    true
}

fn rtti_cast_from_subobjects<F>(
    nodes: &[RttiSubobject],
    src_type: u32,
    src_object: u32,
    dst_type: u32,
    same_type: F,
) -> Option<u32>
where
    F: Fn(u32, u32) -> bool,
{
    let sources: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| {
            (node.object == src_object && same_type(node.type_info, src_type)).then_some(index)
        })
        .collect();
    if sources.is_empty() {
        return None;
    }
    let targets: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| same_type(node.type_info, dst_type).then_some(index))
        .collect();

    let mut downcast_targets = Vec::new();
    for &target in &targets {
        if sources
            .iter()
            .any(|&source| rtti_path_is_public(nodes, target, source))
        {
            let address = nodes[target].object;
            if !downcast_targets.contains(&address) {
                downcast_targets.push(address);
            }
        }
    }
    if !downcast_targets.is_empty() {
        return (downcast_targets.len() == 1).then_some(downcast_targets[0]);
    }

    let source_is_public = sources.iter().any(|&source| nodes[source].public_from_root);
    if !source_is_public {
        return None;
    }
    let mut public_targets = Vec::new();
    for target in targets {
        if nodes[target].public_from_root {
            let address = nodes[target].object;
            if !public_targets.contains(&address) {
                public_targets.push(address);
            }
        }
    }
    if public_targets.len() == 1 {
        Some(public_targets[0])
    } else {
        None
    }
}

fn exact_dynamic_type_cast(dynamic_object: u32, offset_to_top: i32, hint: i32) -> Option<u32> {
    if hint < 0 || offset_to_top != hint.checked_neg()? {
        return None;
    }
    Some(dynamic_object)
}

fn __dynamic_cast(
    env: &mut Environment,
    src: ConstVoidPtr,
    src_type: ConstVoidPtr,
    dst_type: ConstVoidPtr,
    src2dst_offset: i32,
) -> ConstVoidPtr {
    static TRACE_COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let trace = std::env::var_os("TOUCHHLE_TRACE_DYNAMIC_CAST").is_some()
        && TRACE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 64;
    if trace {
        log!(
            "__dynamic_cast input: src={:#010x} src_type={:#010x} ({:?}) dst_type={:#010x} ({:?}) hint={}",
            src.to_bits(),
            src_type.to_bits(),
            rtti_type_name(env, src_type.to_bits()),
            dst_type.to_bits(),
            rtti_type_name(env, dst_type.to_bits()),
            src2dst_offset
        );
    }
    if src.is_null() {
        return Ptr::null();
    }
    let src_type = src_type.to_bits();
    let dst_type = dst_type.to_bits();
    if src_type == 0 || dst_type == 0 {
        return Ptr::null();
    }
    if rtti_types_equal(env, src_type, dst_type) {
        return src;
    }

    let Some(vtable) = read_guest_u32(env, src.to_bits()) else {
        return Ptr::null();
    };
    let Some(offset_to_top_address) = vtable.checked_sub(8) else {
        return Ptr::null();
    };
    let Some(type_info_address) = vtable.checked_sub(4) else {
        return Ptr::null();
    };
    let Some(offset_to_top) = read_guest_u32(env, offset_to_top_address).map(|x| x as i32) else {
        return Ptr::null();
    };
    let Some(dynamic_type) = read_guest_u32(env, type_info_address) else {
        return Ptr::null();
    };
    let Some(dynamic_object) = guest_add_signed(src.to_bits(), offset_to_top) else {
        return Ptr::null();
    };
    if trace {
        log!(
            "__dynamic_cast object: vtable={:#010x} dynamic_type={:#010x} ({:?}) dynamic_object={:#010x} offset_to_top={}",
            vtable,
            dynamic_type,
            rtti_type_name(env, dynamic_type),
            dynamic_object,
            offset_to_top
        );
    }

    if rtti_types_equal(env, dynamic_type, dst_type) && src2dst_offset >= 0 {
        return exact_dynamic_type_cast(dynamic_object, offset_to_top, src2dst_offset)
            .map_or(Ptr::null(), ConstVoidPtr::from_bits);
    }

    let Some(nodes) = rtti_subobjects(env, dynamic_type, dynamic_object) else {
        if trace {
            log!(
                "__dynamic_cast hierarchy decode failed for {:#010x}",
                dynamic_type
            );
        }
        return Ptr::null();
    };
    let result =
        rtti_cast_from_subobjects(&nodes, src_type, src.to_bits(), dst_type, |left, right| {
            rtti_types_equal(env, left, right)
        });
    if trace {
        let hierarchy: Vec<_> = nodes
            .iter()
            .map(|node| {
                (
                    format!("{:#010x}", node.type_info),
                    rtti_type_name(env, node.type_info),
                    format!("{:#010x}", node.object),
                    node.parent,
                    node.public_from_parent,
                    node.public_from_root,
                )
            })
            .collect();
        log!("__dynamic_cast hierarchy={hierarchy:?} result={result:#?}");
    }
    result.map_or(Ptr::null(), ConstVoidPtr::from_bits)
}

// === SjLj unwinder entry points ===
//
// `_Unwind_SjLj_Register/Unregister` push and pop a jmpbuf onto the
// thread-local SjLj exception chain. We never actually consult the
// chain (because __cxa_throw doesn't walk it), so register/unregister
// are no-ops. `_Unwind_SjLj_RaiseException` and `_Unwind_SjLj_Resume`
// fall through to the same frame-pointer bypass as __cxa_throw.

#[allow(non_snake_case)]
fn _Unwind_SjLj_Register(_env: &mut Environment, _jmpbuf: MutVoidPtr) {}

#[allow(non_snake_case)]
fn _Unwind_SjLj_Unregister(_env: &mut Environment, _jmpbuf: MutVoidPtr) {}

#[allow(non_snake_case)]
fn _Unwind_SjLj_RaiseException(env: &mut Environment, _exc: MutVoidPtr) -> i32 {
    // Break runaway exception loops (e.g. `operator new` retrying a refused
    // huge allocation and re-throwing bad_alloc every iteration). Key on the
    // throwing call site (LR) so unrelated throws don't share a counter.
    let site = env.cpu.regs()[Cpu::LR];
    let count = note_exception_throw(&format!("sjlj:{:#x}", site));
    if count <= 3 || count.is_multiple_of(64) {
        log!(
            "_Unwind_SjLj_RaiseException — bypassing (site={:#x}, consecutive #{})",
            site,
            count
        );
    }
    if count >= THROW_LOOP_LIMIT {
        log!(
            "Warning: SjLj exception loop detected (site={:#x} raised {} times \
             in a row); terminating only the current guest thread.",
            site,
            count
        );
        terminate_current_thread_after_exception(env, &format!("site={site:#x}"));
        return 0;
    }
    if !unwind_to_app_frame(env) {
        log!(
            "Warning: _Unwind_SjLj_RaiseException with no recoverable frame; \
             terminating only the current guest thread."
        );
        terminate_current_thread_after_exception(env, &format!("site={site:#x}"));
    }
    0
}

#[allow(non_snake_case)]
fn _Unwind_SjLj_Resume(env: &mut Environment, _exc: MutVoidPtr) {
    log_once!("_Unwind_SjLj_Resume — bypassing; repeated resume calls are suppressed");
    if !unwind_to_app_frame(env) {
        log!(
            "Warning: _Unwind_SjLj_Resume with no recoverable frame; \
             terminating only the current guest thread."
        );
        terminate_current_thread_after_exception(env, "_Unwind_SjLj_Resume");
    }
}

#[allow(non_snake_case)]
fn _Unwind_SjLj_Resume_or_Rethrow(env: &mut Environment, _exc: MutVoidPtr) -> i32 {
    log_once!("_Unwind_SjLj_Resume_or_Rethrow — bypassing; repeated calls are suppressed");
    if !unwind_to_app_frame(env) {
        log!(
            "Warning: _Unwind_SjLj_Resume_or_Rethrow with no recoverable frame; \
             terminating only the current guest thread."
        );
        terminate_current_thread_after_exception(env, "_Unwind_SjLj_Resume_or_Rethrow");
    }
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(__cxa_atexit(_, _, _)),
    export_c_func!(__cxa_finalize(_)),
    export_c_func!(__cxa_guard_acquire(_)),
    export_c_func!(__cxa_guard_release(_)),
    export_c_func!(__cxa_guard_abort(_)),
    export_c_func!(__cxa_allocate_exception(_)),
    export_c_func!(__cxa_free_exception(_)),
    export_c_func!(__cxa_decrement_exception_refcount(_)),
    export_c_func!(__cxa_increment_exception_refcount(_)),
    export_c_func!(__cxa_throw(_, _, _)),
    export_c_func!(__cxa_rethrow()),
    export_c_func!(__cxa_begin_catch(_)),
    export_c_func!(__cxa_end_catch()),
    export_c_func!(__cxa_pure_virtual()),
    export_c_func!(__cxa_call_unexpected(_)),
    export_c_func!(__cxa_uncaught_exception()),
    export_c_func!(__dynamic_cast(_, _, _, _)),
    export_c_func!(_Unwind_SjLj_Register(_)),
    export_c_func!(_Unwind_SjLj_Unregister(_)),
    export_c_func!(_Unwind_SjLj_RaiseException(_)),
    export_c_func!(_Unwind_SjLj_Resume(_)),
    export_c_func!(_Unwind_SjLj_Resume_or_Rethrow(_)),
];

#[cfg(test)]
mod dynamic_cast_tests {
    use super::{rtti_cast_from_subobjects, RttiSubobject};

    fn node(
        type_info: u32,
        object: u32,
        parent: Option<usize>,
        public_from_parent: bool,
        public_from_root: bool,
        depth: u8,
    ) -> RttiSubobject {
        RttiSubobject {
            type_info,
            object,
            parent,
            public_from_parent,
            public_from_root,
            depth,
        }
    }

    #[test]
    fn public_base_downcasts_to_derived() {
        let nodes = vec![
            node(10, 0x1000, None, true, true, 0),
            node(20, 0x1010, Some(0), true, true, 1),
        ];
        assert_eq!(
            rtti_cast_from_subobjects(&nodes, 20, 0x1010, 10, |left, right| left == right),
            Some(0x1000)
        );
    }

    #[test]
    fn private_base_does_not_downcast() {
        let nodes = vec![
            node(10, 0x1000, None, true, true, 0),
            node(20, 0x1010, Some(0), false, false, 1),
        ];
        assert_eq!(
            rtti_cast_from_subobjects(&nodes, 20, 0x1010, 10, |left, right| left == right),
            None
        );
    }

    #[test]
    fn public_sibling_base_crosscasts() {
        let nodes = vec![
            node(30, 0x1000, None, true, true, 0),
            node(10, 0x1010, Some(0), true, true, 1),
            node(20, 0x1020, Some(0), true, true, 1),
        ];
        assert_eq!(
            rtti_cast_from_subobjects(&nodes, 10, 0x1010, 20, |left, right| left == right),
            Some(0x1020)
        );
    }

    #[test]
    fn ambiguous_public_sibling_targets_fail() {
        let nodes = vec![
            node(30, 0x1000, None, true, true, 0),
            node(20, 0x1010, Some(0), true, true, 1),
            node(10, 0x1020, Some(0), true, true, 1),
            node(10, 0x1030, Some(0), true, true, 1),
        ];
        assert_eq!(
            rtti_cast_from_subobjects(&nodes, 20, 0x1010, 10, |left, right| left == right),
            None
        );
    }
}
