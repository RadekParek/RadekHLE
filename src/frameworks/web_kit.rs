/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, you can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! WebKit framework registration and compatibility exports.
//!
//! Public application-facing web content is represented by UIKit's UIWebView
//! implementation in this emulator. Older iOS applications can nevertheless
//! link the private WebKit framework because UIKit's web view implementation
//! depended on it. Registering both historical framework paths lets dyld
//! resolve that dependency without exporting private WebKit classes into the
//! Objective-C runtime.

use crate::dyld::FunctionExports;

pub const FUNCTIONS: FunctionExports = &[];

pub const DYLIB: crate::dyld::HostDylib = crate::dyld::HostDylib {
    path: "/System/Library/PrivateFrameworks/WebKit.framework/WebKit",
    aliases: &["/System/Library/Frameworks/WebKit.framework/WebKit"],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};
