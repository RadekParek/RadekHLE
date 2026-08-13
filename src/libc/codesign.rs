/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! The `csops(2)` code-signing inspection API.
//!
//! touchHLE does not execute Apple's code-signing machinery, but it does load
//! the guest executable as a real Mach-O image. This implementation exposes the
//! stable, read-only `CS_OPS_STATUS` operation instead of using dyld's generic
//! return-zero fallback. The reported flags describe an already-loaded app
//! image: valid and ad-hoc signed, with no platform or enforcement privileges.

use crate::dyld::{export_c_func, FunctionExports};
use crate::libc::errno::{set_errno, EFAULT, EINVAL, ESRCH};
use crate::libc::unistd::pid_t;
use crate::mem::{GuestUSize, MutVoidPtr};
use crate::Environment;

const CS_OPS_STATUS: u32 = 0;
const CS_VALID: u32 = 0x0000_0001;
const CS_ADHOC: u32 = 0x0000_0002;

fn current_pid(env: &mut Environment) -> pid_t {
    crate::libc::unistd::getpid(env)
}

fn checked_target_pid(env: &mut Environment, pid: pid_t) -> Result<(), i32> {
    let target = if pid == 0 { current_pid(env) } else { pid };
    if target == current_pid(env) {
        Ok(())
    } else {
        Err(ESRCH)
    }
}

fn write_u32(env: &mut Environment, useraddr: MutVoidPtr, value: u32) -> Result<(), i32> {
    if useraddr.is_null() {
        return Err(EFAULT);
    }
    env.mem.write(useraddr.cast(), value);
    Ok(())
}

fn write_bytes(
    env: &mut Environment,
    useraddr: MutVoidPtr,
    usersize: GuestUSize,
    value: &[u8],
) -> Result<(), i32> {
    if useraddr.is_null() {
        return Err(EFAULT);
    }
    if usersize < value.len() as GuestUSize {
        return Err(EINVAL);
    }
    env.mem
        .bytes_at_mut(useraddr.cast(), value.len() as GuestUSize)
        .copy_from_slice(value);
    Ok(())
}

fn csops(
    env: &mut Environment,
    pid: pid_t,
    ops: u32,
    useraddr: MutVoidPtr,
    usersize: GuestUSize,
) -> i32 {
    if let Err(errno) = checked_target_pid(env, pid) {
        set_errno(env, errno);
        return -1;
    }

    let result = match ops {
        CS_OPS_STATUS => {
            if usersize < 4 {
                Err(EINVAL)
            } else {
                write_u32(env, useraddr, CS_VALID | CS_ADHOC)
            }
        }
        _ => Err(EINVAL),
    };

    match result {
        Ok(()) => {
            set_errno(env, 0);
            0
        }
        Err(errno) => {
            set_errno(env, errno);
            -1
        }
    }
}

pub const FUNCTIONS: FunctionExports = &[export_c_func!(csops(_, _, _, _))];
