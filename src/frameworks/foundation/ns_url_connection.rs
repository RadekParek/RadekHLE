/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

//! `NSURLConnection`.
//!
//! Requests use the host's network when the emulator's network option is
//! enabled. Host failures are converted into the normal Foundation error
//! callback instead of hanging or crashing the guest.
//!
//! For block-based API (`sendAsynchronousRequest:queue:completionHandler:`),
//! we deliver an NSError to the completion handler so the app can handle
//! the offline state gracefully (e.g. Sonic Runners shows "Error" and retries).
//!
//! The compatibility profile can still request a deliberately synthetic 200
//! response for apps that need that legacy behaviour.

use crate::mem::{ConstVoidPtr, MutPtr, MutVoidPtr};
use crate::objc::{
    autorelease, id, msg, msg_class, nil, objc_classes, release, retain, ClassExports, HostObject,
    NSZonePtr,
};
use std::io::Read;
use std::time::Duration;

// NSError domain / code used when reporting "no network in emulator".
const NS_URL_ERROR_DOMAIN: &str = "NSURLErrorDomain";
const NS_URL_ERROR_NOT_CONNECTED_TO_INTERNET: i32 = -1009;

fn fake_network_success_enabled() -> bool {
    std::env::var_os("TOUCHHLE_FAKE_NETWORK_SUCCESS").is_some()
}

#[derive(Debug)]
pub(crate) struct NetworkResponse {
    pub(crate) status_code: u16,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: Vec<u8>,
}

pub(crate) fn perform_request(
    env: &mut crate::Environment,
    request: id,
) -> Result<NetworkResponse, String> {
    if request == nil {
        return Err("request is nil".to_string());
    }
    if !env.options.network_access {
        return Err("network access is disabled".to_string());
    }

    let url_object: id = msg![env; request URL];
    if url_object == nil {
        return Err("request URL is nil".to_string());
    }
    let absolute_string: id = msg![env; url_object absoluteString];
    let url = crate::frameworks::foundation::ns_string::to_rust_string(env, absolute_string)
        .into_owned();
    if url.is_empty() {
        return Err("request URL is empty".to_string());
    }

    let method_object: id = msg![env; request HTTPMethod];
    let method = crate::frameworks::foundation::ns_string::to_rust_string(env, method_object)
        .into_owned();
    let method = if method.is_empty() { "GET".to_string() } else { method };
    let timeout: f64 = msg![env; request timeoutInterval];
    let timeout = timeout.clamp(1.0, 120.0);

    let body_object: id = msg![env; request HTTPBody];
    let body_length: u32 = if body_object == nil {
        0
    } else {
        msg![env; body_object length]
    };
    let body = if body_length == 0 {
        Vec::new()
    } else {
        let bytes: ConstVoidPtr = msg![env; body_object bytes];
        env.mem.bytes_at(bytes.cast::<u8>(), body_length).to_vec()
    };

    log!(
        "NSURLConnection: fetching {} {} (timeout {:.0}s)",
        method,
        url,
        timeout
    );

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs_f64(timeout))
        .build();
    let mut builder = agent.request(&method, &url);
    builder = builder.set("User-Agent", "RadekHLE9.0");
    let result = if body.is_empty() && method.eq_ignore_ascii_case("GET") {
        builder.call()
    } else {
        builder.send_bytes(&body)
    };
    let response = match result {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(error) => return Err(error.to_string()),
    };
    let status_code = response.status() as u16;
    let headers = response
        .headers_names()
        .into_iter()
        .filter_map(|name| {
            response
                .header(&name)
                .map(|value| (name, value.to_string()))
        })
        .collect();
    let mut response_body = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut response_body)
        .map_err(|error| error.to_string())?;
    log!(
        "NSURLConnection: received HTTP {} ({} bytes)",
        status_code,
        response_body.len()
    );
    if !(200..400).contains(&status_code) {
        log_once_fmt!(
            "NSURLConnection: treating HTTP status {} as a request failure; repeated HTTP failures are suppressed",
            status_code
        );
        return Err(format!("HTTP status {}", status_code));
    }
    Ok(NetworkResponse {
        status_code,
        headers,
        body: response_body,
    })
}

fn make_data_from_bytes(env: &mut crate::Environment, body: &[u8]) -> id {
    if body.is_empty() {
        return msg_class![env; NSData data];
    }
    let length: u32 = body.len().try_into().unwrap_or(u32::MAX);
    let buffer = env.mem.alloc(length);
    env.mem.bytes_at_mut(buffer.cast(), length).copy_from_slice(&body[..length as usize]);
    let bytes: ConstVoidPtr = buffer.cast_const().cast_void();
    let data: id = msg_class![env; NSData dataWithBytes:bytes length:length];
    env.mem.free(buffer.cast());
    data
}

fn make_http_response(
    env: &mut crate::Environment,
    request: id,
    status_code: u16,
    response_headers: &[(String, String)],
) -> id {
    use crate::frameworks::foundation::ns_string::from_rust_string;

    let url: id = msg![env; request URL];
    let headers: id = msg_class![env; NSMutableDictionary new];
    autorelease(env, headers);
    for (name, value) in response_headers {
        let name_object = from_rust_string(env, name.clone());
        let value_object = from_rust_string(env, value.clone());
        autorelease(env, name_object);
        autorelease(env, value_object);
        () = msg![env; headers setObject:value_object forKey:name_object];
    }
    let http_version = from_rust_string(env, "HTTP/1.1".to_string());
    autorelease(env, http_version);
    let response: id = msg_class![env; NSHTTPURLResponse alloc];
    let response: id = msg![env;
        response initWithURL:url
                 statusCode:(status_code as i32)
                HTTPVersion:http_version
               headerFields:headers];
    autorelease(env, response);
    response
}

// ---------------------------------------------------------------------------
// Host object — stores the delegate so we can call it back.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct NSURLConnectionHostObject {
    /// `id<NSURLConnectionDelegate>` — retained while the connection is
    /// alive, released on dealloc / cancel.
    delegate: id,
    /// `NSURLRequest*` retained while the connection is alive.
    request: id,
    /// Whether the connection has already been cancelled / finished.
    cancelled: bool,
}
impl HostObject for NSURLConnectionHostObject {}

// ---------------------------------------------------------------------------
// Helper — build an NSError for "not connected to internet".
// ---------------------------------------------------------------------------
fn make_network_error(env: &mut crate::Environment) -> id {
    use crate::frameworks::foundation::ns_string::{from_rust_string, get_static_str};

    let domain = from_rust_string(env, NS_URL_ERROR_DOMAIN.to_string());
    autorelease(env, domain);

    let desc_key = get_static_str(env, "NSLocalizedDescription");
    let desc_val = from_rust_string(
        env,
        "The network connection was unavailable."
            .to_string(),
    );
    autorelease(env, desc_val);

    let user_info: id = msg_class![env; NSMutableDictionary new];
    autorelease(env, user_info);
    () = msg![env; user_info setObject:desc_val forKey:desc_key];

    let error: id = msg_class![env; NSError alloc];
    let error: id = msg![env;
        error initWithDomain:domain
                        code:NS_URL_ERROR_NOT_CONNECTED_TO_INTERNET
                    userInfo:user_info];
    autorelease(env, error);
    error
}

fn make_fake_success_data(env: &mut crate::Environment) -> id {
    let body = b"{}";
    let len: u32 = body.len().try_into().unwrap();
    let ptr = env.mem.alloc(len);
    env.mem.bytes_at_mut(ptr.cast(), len).copy_from_slice(body);

    // msg_class! cannot parse chained calls like ptr.cast_const().cast_void()
    // directly inside the macro, so prepare the argument first.
    let bytes_ptr = ptr.cast_const().cast_void();
    let data: id = msg_class![env; NSData dataWithBytes:bytes_ptr length:len];

    // dataWithBytes:length: copies the buffer, so free our temporary guest memory.
    env.mem.free(ptr.cast());
    data
}

fn make_fake_http_response(env: &mut crate::Environment, request: id) -> id {
    let headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        ("Content-Length".to_string(), "2".to_string()),
    ];
    make_http_response(env, request, 200, &headers)
}

// ---------------------------------------------------------------------------
// Helper — call `connection:didFailWithError:` on the delegate.
// Uses msg! which already handles unimplemented selectors gracefully.
// ---------------------------------------------------------------------------
fn notify_delegate_failure(env: &mut crate::Environment, connection: id, delegate: id) {
    if delegate == nil {
        return;
    }
    log_dbg!("NSURLConnection: notifying delegate of failure");
    let error = make_network_error(env);
    () = msg![env; delegate connection:connection didFailWithError:error];
}

fn notify_delegate_success(
    env: &mut crate::Environment,
    connection: id,
    delegate: id,
    request: id,
) {
    if delegate == nil {
        return;
    }

    let (response, data) = if fake_network_success_enabled() {
        log!("NSURLConnection: delivering explicit compatibility-profile fake HTTP 200 response");
        (make_fake_http_response(env, request), make_fake_success_data(env))
    } else {
        match perform_request(env, request) {
            Ok(result) => (
                make_http_response(env, request, result.status_code, &result.headers),
                make_data_from_bytes(env, &result.body),
            ),
            Err(error) => {
                log!("NSURLConnection: request failed: {}", error);
                notify_delegate_failure(env, connection, delegate);
                return;
            }
        }
    };

    () = msg![env; delegate connection:connection didReceiveResponse:response];
    () = msg![env; delegate connection:connection didReceiveData:data];
    () = msg![env; delegate connectionDidFinishLoading:connection];
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation NSURLConnection: NSObject

+ (id)allocWithZone:(NSZonePtr)_zone {
    let host = Box::new(NSURLConnectionHostObject {
        delegate: nil,
        request: nil,
        cancelled: false,
    });
    env.objc.alloc_object(this, host, &mut env.mem)
}

// MARK: - canHandleRequest: (class method)

+ (bool)canHandleRequest:(id)_request {
    // Advertise support so the app doesn't take a different code path;
    // failure is reported via the delegate / error out-param instead.
    true
}

// MARK: - Synchronous API

+ (id)sendSynchronousRequest:(id)request
           returningResponse:(MutPtr<id>)response_ptr
                       error:(MutPtr<id>)error_ptr {

    if fake_network_success_enabled() {
        if !response_ptr.is_null() {
            let response = make_fake_http_response(env, request);
            retain(env, response);
            env.mem.write(response_ptr, response);
        }
        if !error_ptr.is_null() {
            env.mem.write(error_ptr, nil);
        }
        return make_fake_success_data(env);
    }

    match perform_request(env, request) {
        Ok(result) => {
            if !response_ptr.is_null() {
                let response = make_http_response(env, request, result.status_code, &result.headers);
                retain(env, response);
                env.mem.write(response_ptr, response);
            }
            if !error_ptr.is_null() {
                env.mem.write(error_ptr, nil);
            }
            make_data_from_bytes(env, &result.body)
        }
        Err(error) => {
            log!("NSURLConnection sendSynchronousRequest: request failed: {}", error);
            if !response_ptr.is_null() {
                env.mem.write(response_ptr, nil);
            }
            if !error_ptr.is_null() {
                let error_object = make_network_error(env);
                retain(env, error_object);
                env.mem.write(error_ptr, error_object);
            }
            msg_class![env; NSData data]
        }
    }
}

// MARK: - Asynchronous block API
//
// `+[NSURLConnection sendAsynchronousRequest:queue:completionHandler:]`
// — iOS 5+ block-based convenience. Requests use the same host-network
// bridge as the synchronous API and report transport failures through the
// completion handler.

+ (())sendAsynchronousRequest:(id)request
                        queue:(id)queue
            completionHandler:(MutVoidPtr)handler {
    if handler.is_null() {
        return;
    }
    // The completion handler is a `void (^)(NSURLResponse *, NSData *,
    // NSError *)` block. ARM32 ABI: the block struct's third word
    // (index 3 == byte offset 12) is the invoke function pointer.
    let invoke_ptr = env.mem.read(handler.cast::<u32>() + 3u32);
    if invoke_ptr == 0 {
        return;
    }
    use crate::abi::CallFromHost;
    let invoke = crate::abi::GuestFunction::from_addr_with_thumb_bit(invoke_ptr);

    let _ = queue;

    if fake_network_success_enabled() {
        let empty_data = make_fake_success_data(env);
        let response = make_fake_http_response(env, request);
        let _: () = invoke.call_from_host(env, (handler, response, empty_data, nil));
        return;
    }

    match perform_request(env, request) {
        Ok(result) => {
            let response = make_http_response(env, request, result.status_code, &result.headers);
            let data = make_data_from_bytes(env, &result.body);
            let _: () = invoke.call_from_host(env, (handler, response, data, nil));
        }
        Err(error_message) => {
            log!("NSURLConnection asynchronous request failed: {}", error_message);
            let error = make_network_error(env);
            let _: () = invoke.call_from_host(env, (handler, nil, nil, error));
        }
    }
}

// MARK: - Asynchronous API

+ (id)connectionWithRequest:(id)request
                   delegate:(id)delegate {
    let new: id = msg![env; this alloc];
    let new: id = msg![env; new initWithRequest:request delegate:delegate];
    autorelease(env, new);
    new
}

- (id)initWithRequest:(id)request
             delegate:(id)delegate {
    msg![env;
        this initWithRequest:request
                    delegate:delegate
            startImmediately:true]
}

- (id)initWithRequest:(id)request
             delegate:(id)delegate
     startImmediately:(bool)start_immediately {

    if request == nil {
        log!("NSURLConnection initWithRequest: nil request — returning nil");
        release(env, this);
        return nil;
    }

    log_dbg!(
        "NSURLConnection initWithRequest:... delegate:... \
         startImmediately:{} (stub — failure via delegate)",
        start_immediately,
    );

    retain(env, delegate);
    retain(env, request);
    {
        let host = env.objc.borrow_mut::<NSURLConnectionHostObject>(this);
        host.delegate  = delegate;
        host.request   = request;
        host.cancelled = false;
    }

    if start_immediately {
        // Per Apple's documentation, when startImmediately is YES the
        // connection begins loading data immediately. Since touchHLE has
        // no network stack, we schedule the delegate failure callback via
        // performSelector:withObject:afterDelay: so that it fires on the
        // next run-loop iteration rather than synchronously during init.
        // This matches real iOS timing behavior — delegates are never
        // called during the initializer itself.
        if fake_network_success_enabled() {
            log_dbg!(
                "NSURLConnection: scheduling deferred network-success notification"
            );
            let sel = env.objc.register_host_selector("_touchHLE_deliverSuccess".to_string(), &mut env.mem);
            () = msg![env; this performSelector:sel withObject:nil afterDelay:0.0_f64];
        } else {
            log_dbg!(
                "NSURLConnection: scheduling deferred network request"
            );
            let sel = env.objc.register_host_selector("_touchHLE_deliverSuccess".to_string(), &mut env.mem);
            () = msg![env; this performSelector:sel withObject:nil afterDelay:0.0_f64];
        }
    }

    this
}

// Internal helper method: delivers the failure callback to the delegate.
- (())_touchHLE_deliverFailure {
    let host = env.objc.borrow::<NSURLConnectionHostObject>(this);
    if host.cancelled {
        return;
    }
    let delegate = host.delegate;
    if delegate == nil {
        return;
    }
    notify_delegate_failure(env, this, delegate);
}

- (())_touchHLE_deliverSuccess {
    let host = env.objc.borrow::<NSURLConnectionHostObject>(this);
    if host.cancelled {
        return;
    }
    let delegate = host.delegate;
    let request = host.request;
    if delegate == nil {
        return;
    }
    notify_delegate_success(env, this, delegate, request);
}

// MARK: - Instance methods

- (())start {
    log_dbg!("NSURLConnection start: scheduling deferred network request");
    let sel = env.objc.register_host_selector("_touchHLE_deliverSuccess".to_string(), &mut env.mem);
    () = msg![env; this performSelector:sel withObject:nil afterDelay:0.0_f64];
}

- (())cancel {
    log_dbg!("NSURLConnection cancel");
    env.objc
        .borrow_mut::<NSURLConnectionHostObject>(this)
        .cancelled = true;
}

// MARK: - Dealloc

- (())dealloc {
    log_dbg!("NSURLConnection dealloc");
    let (delegate, request) = {
        let host = env.objc.borrow::<NSURLConnectionHostObject>(this);
        (host.delegate, host.request)
    };
    release(env, delegate);
    release(env, request);
    env.objc.dealloc_object(this, &mut env.mem);
}

@end

};
