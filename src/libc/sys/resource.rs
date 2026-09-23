/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::dyld::FunctionExports;
use crate::environment::Environment;
use crate::export_c_func;
use crate::libc::errno::{set_errno, EFAULT, EINVAL};
use crate::mem::{GuestUSize, MutPtr, SafeRead};

#[allow(non_camel_case_types)]
type rlim_t = u64;

#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct rlimit {
    pub rlim_cur: rlim_t,
    pub rlim_max: rlim_t,
}
unsafe impl SafeRead for rlimit {}

const RLIM_INFINITY: rlim_t = (1u64 << 63) - 1;
const RLIM_NLIMITS: i32 = 9;
const MB: rlim_t = 1024 * 1024;

const DEFAULT_LIMITS: [rlimit; RLIM_NLIMITS as usize] = [
    rlimit {
        rlim_cur: RLIM_INFINITY,
        rlim_max: RLIM_INFINITY,
    },
    rlimit {
        rlim_cur: RLIM_INFINITY,
        rlim_max: RLIM_INFINITY,
    },
    rlimit {
        rlim_cur: RLIM_INFINITY,
        rlim_max: RLIM_INFINITY,
    },
    rlimit {
        rlim_cur: 8 * MB,
        rlim_max: 64 * MB,
    },
    rlimit {
        rlim_cur: 0,
        rlim_max: RLIM_INFINITY,
    },
    rlimit {
        rlim_cur: RLIM_INFINITY,
        rlim_max: RLIM_INFINITY,
    },
    rlimit {
        rlim_cur: RLIM_INFINITY,
        rlim_max: RLIM_INFINITY,
    },
    rlimit {
        rlim_cur: 256,
        rlim_max: 256,
    },
    rlimit {
        rlim_cur: 256,
        rlim_max: 10_240,
    },
];

pub struct State {
    limits: [rlimit; RLIM_NLIMITS as usize],
}

impl Default for rlimit {
    fn default() -> Self {
        Self {
            rlim_cur: 0,
            rlim_max: 0,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            limits: DEFAULT_LIMITS,
        }
    }
}

fn getrlimit(env: &mut Environment, which: i32, limit: MutPtr<rlimit>) -> i32 {
    if !(0..RLIM_NLIMITS).contains(&which) {
        set_errno(env, EINVAL);
        return -1;
    }
    if limit.is_null() {
        set_errno(env, EFAULT);
        return -1;
    }
    if env
        .mem
        .get_bytes_fallible_mut(
            limit.cast_const().cast(),
            std::mem::size_of::<rlimit>() as GuestUSize,
        )
        .is_none()
    {
        set_errno(env, EFAULT);
        return -1;
    }
    let value = env.libc_state.resource.limits[which as usize];
    env.mem.write(limit, value);
    set_errno(env, 0);
    0
}

fn setrlimit(env: &mut Environment, which: i32, limit: MutPtr<rlimit>) -> i32 {
    if !(0..RLIM_NLIMITS).contains(&which) {
        set_errno(env, EINVAL);
        return -1;
    }
    if limit.is_null() {
        set_errno(env, EFAULT);
        return -1;
    }
    if env
        .mem
        .get_bytes_fallible(
            limit.cast_const().cast(),
            std::mem::size_of::<rlimit>() as GuestUSize,
        )
        .is_none()
    {
        set_errno(env, EFAULT);
        return -1;
    }
    let value = env.mem.read(limit);
    let (soft, hard) = (value.rlim_cur, value.rlim_max);
    if soft > hard {
        set_errno(env, EINVAL);
        return -1;
    }
    env.libc_state.resource.limits[which as usize] = value;
    set_errno(env, 0);
    0
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(getrlimit(_, _)),
    export_c_func!(setrlimit(_, _)),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn darwin_resource_limit_slots_use_darwin_order() {
        assert_eq!(DEFAULT_LIMITS.len(), 9);
        assert_eq!(DEFAULT_LIMITS[3].rlim_cur, 8 * MB);
        assert_eq!(DEFAULT_LIMITS[5].rlim_cur, RLIM_INFINITY);
        assert_eq!(DEFAULT_LIMITS[7].rlim_cur, 256);
        assert_eq!(DEFAULT_LIMITS[8].rlim_cur, 256);
    }
}
