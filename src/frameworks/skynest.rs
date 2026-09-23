/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use crate::abi::{CallFromHost, GuestFunction};
use crate::dyld::{export_c_func, export_c_func_aliased, FunctionExports, HostDylib};
use crate::mem::MutVoidPtr;
use crate::Environment;

pub const DYLIB: HostDylib = HostDylib {
    path: "/usr/lib/libSkynest.dylib",
    aliases: &[],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};

#[derive(Default)]
pub struct State {
    identity_logged_in: Option<GuestFunction>,
    identity_time_success: Option<GuestFunction>,
    assets_load_error: Option<GuestFunction>,
    assets_metadata_error: Option<GuestFunction>,
}

fn call_no_args(env: &mut Environment, callback: Option<GuestFunction>) {
    if let Some(callback) = callback.filter(|callback| *callback != GuestFunction::null_ptr()) {
        <GuestFunction as CallFromHost<(), ()>>::call_from_host(&callback, env, ());
    }
}

fn call_i32(env: &mut Environment, callback: Option<GuestFunction>, value: i32) {
    if let Some(callback) = callback.filter(|callback| *callback != GuestFunction::null_ptr()) {
        <GuestFunction as CallFromHost<(), (i32,)>>::call_from_host(&callback, env, (value,));
    }
}

fn call_asset_error(
    env: &mut Environment,
    callback: Option<GuestFunction>,
    name: MutVoidPtr,
    context_a: MutVoidPtr,
    context_b: MutVoidPtr,
    context_c: MutVoidPtr,
) {
    if let Some(callback) = callback.filter(|callback| *callback != GuestFunction::null_ptr()) {
        <GuestFunction as CallFromHost<
            (),
            (MutVoidPtr, i32, MutVoidPtr, MutVoidPtr, MutVoidPtr),
        >>::call_from_host(&callback, env, (name, 503, context_a, context_b, context_c));
    }
}

fn call_metadata_error(
    env: &mut Environment,
    callback: Option<GuestFunction>,
    name: MutVoidPtr,
    context_a: MutVoidPtr,
    context_b: MutVoidPtr,
) {
    if let Some(callback) = callback.filter(|callback| *callback != GuestFunction::null_ptr()) {
        <GuestFunction as CallFromHost<(), (MutVoidPtr, i32, MutVoidPtr, MutVoidPtr)>>::call_from_host(
            &callback,
            env,
            (name, 503, context_a, context_b),
        );
    }
}

fn skynest_initialize_sdk(_env: &mut Environment, _parameters: MutVoidPtr) {}

fn skynest_destroy_sdk(_env: &mut Environment) {}

fn skynest_update(_env: &mut Environment, _delta_time: f32) {}

fn skynest_activate(_env: &mut Environment, _should_be_activated: bool) {}

fn skynest_identity_set_callbacks(
    env: &mut Environment,
    logged_in: GuestFunction,
    _login_error: GuestFunction,
    time_success: GuestFunction,
    _time_error: GuestFunction,
    _access_token_success: GuestFunction,
    _access_token_error: GuestFunction,
) {
    let state = &mut env.framework_state.skynest;
    state.identity_logged_in = Some(logged_in);
    state.identity_time_success = Some(time_success);
}

fn skynest_identity_login_by_method(env: &mut Environment, _login_method: i32) {
    log!("Skynest compatibility: completing guest login locally");
    let callback = env.framework_state.skynest.identity_logged_in;
    call_no_args(env, callback);
}

fn skynest_identity_login(_env: &mut Environment, _parameters: MutVoidPtr) {}

fn skynest_identity_login_with_ui(_env: &mut Environment, _view: MutVoidPtr) {}

fn skynest_identity_logout(_env: &mut Environment) {}

fn skynest_identity_get_user_profile(
    _env: &mut Environment,
    _profile: MutVoidPtr,
    _buffer_length: i32,
) {
}

fn skynest_identity_fetch_access_token(_env: &mut Environment) {}

fn skynest_identity_is_service_available(
    _env: &mut Environment,
    _service_name: MutVoidPtr,
) -> bool {
    true
}

fn skynest_identity_time(env: &mut Environment) {
    let timestamp = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_secs().min(i32::MAX as u64) as i32,
        Err(_) => 0,
    };
    let callback = env.framework_state.skynest.identity_time_success;
    call_i32(env, callback, timestamp);
}

fn skynest_assets_initialize(_env: &mut Environment) {}

fn skynest_assets_destroy(_env: &mut Environment) {}

fn skynest_assets_set_callbacks(
    env: &mut Environment,
    _load_success: GuestFunction,
    load_error: GuestFunction,
    _progress: GuestFunction,
    _metadata_success: GuestFunction,
    metadata_error: GuestFunction,
) {
    let state = &mut env.framework_state.skynest;
    state.assets_load_error = Some(load_error);
    state.assets_metadata_error = Some(metadata_error);
}

fn skynest_assets_load(
    env: &mut Environment,
    context_a: MutVoidPtr,
    context_b: MutVoidPtr,
    context_c: MutVoidPtr,
    asset_name: MutVoidPtr,
) {
    log_once!("Skynest assets are unavailable in the emulator; reporting HTTP 503 to the game");
    let callback = env.framework_state.skynest.assets_load_error;
    call_asset_error(env, callback, asset_name, context_a, context_b, context_c);
}

fn skynest_assets_load_metadata(
    env: &mut Environment,
    context_a: MutVoidPtr,
    context_b: MutVoidPtr,
    asset_name: MutVoidPtr,
) {
    log_once!("Skynest metadata is unavailable in the emulator; reporting HTTP 503 to the game");
    let callback = env.framework_state.skynest.assets_metadata_error;
    call_metadata_error(env, callback, asset_name, context_a, context_b);
}

fn skynest_assets_load_all_metadata(
    env: &mut Environment,
    context_a: MutVoidPtr,
    context_b: MutVoidPtr,
) {
    log_once!("Skynest metadata is unavailable in the emulator; reporting HTTP 503 to the game");
    let callback = env.framework_state.skynest.assets_metadata_error;
    call_metadata_error(env, callback, MutVoidPtr::null(), context_a, context_b);
}

fn skynest_analytics_log_event(_env: &mut Environment, _event: MutVoidPtr) {}

fn skynest_analytics_log_event_with_parameters(
    _env: &mut Environment,
    _event: MutVoidPtr,
    _parameters: MutVoidPtr,
) {
}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func_aliased!("skynest_initializeSdk", skynest_initialize_sdk(_)),
    export_c_func_aliased!("skynest_destroySdk", skynest_destroy_sdk()),
    export_c_func!(skynest_update(_)),
    export_c_func!(skynest_activate(_)),
    export_c_func_aliased!(
        "skynest_identity_setCallbacks",
        skynest_identity_set_callbacks(_, _, _, _, _, _)
    ),
    export_c_func!(skynest_identity_login_by_method(_)),
    export_c_func!(skynest_identity_login(_)),
    export_c_func!(skynest_identity_login_with_ui(_)),
    export_c_func!(skynest_identity_logout()),
    export_c_func!(skynest_identity_get_user_profile(_, _)),
    export_c_func_aliased!(
        "skynest_identity_fetch_accesstoken",
        skynest_identity_fetch_access_token()
    ),
    export_c_func_aliased!(
        "skynest_identity_isServiceAvailable",
        skynest_identity_is_service_available(_)
    ),
    export_c_func!(skynest_identity_time()),
    export_c_func_aliased!(
        "skynest_assets_setCallbacks",
        skynest_assets_set_callbacks(_, _, _, _, _)
    ),
    export_c_func!(skynest_assets_load(_, _, _, _)),
    export_c_func_aliased!(
        "skynest_assets_loadMetadata",
        skynest_assets_load_metadata(_, _, _)
    ),
    export_c_func_aliased!(
        "skynest_assets_load_all_metadata",
        skynest_assets_load_all_metadata(_, _)
    ),
    export_c_func_aliased!("skynest_analytics_logEvent", skynest_analytics_log_event(_)),
    export_c_func_aliased!(
        "skynest_analytics_logEventWithParameters",
        skynest_analytics_log_event_with_parameters(_, _)
    ),
    export_c_func!(skynest_assets_initialize()),
    export_c_func!(skynest_assets_destroy()),
];
