/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Read-once, cached environment variable lookups for hot paths.
//!
//! `std::env::var_os`/`std::env::var` take a global lock, linearly scan the
//! process environ array and allocate, on *every* call. That's far too
//! expensive to do per frame or per touch event. The various
//! `TOUCHHLE_*` debug/compat toggles are set by the user before launch, so
//! reading them once and caching the result for the lifetime of the process
//! preserves their meaning while making repeated checks almost free.
//!
//! Note: unlike re-reading the variable each time, changes made *after* the
//! first lookup are (intentionally) no longer visible.

/// Check for the *presence* of an environment variable, once, and return the
/// cached result on subsequent calls.
#[macro_export]
macro_rules! env_flag_cached {
    ($name:literal) => {{
        static CACHED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *CACHED.get_or_init(|| std::env::var_os($name).is_some())
    }};
}

/// Read the string value of an environment variable, once, and return the
/// cached result (`Some(&str)` if set and valid UTF-8) on subsequent calls.
#[macro_export]
macro_rules! env_var_cached {
    ($name:literal) => {{
        static CACHED: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        CACHED.get_or_init(|| std::env::var($name).ok()).as_deref()
    }};
}
