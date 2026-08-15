/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Compatibility implementation for UIKit's private launch delegate.

use crate::objc::{id, objc_classes, ClassExports, TrivialHostObject};

#[derive(Default)]
pub struct State {
    shared_instance: Option<id>,
}

pub const CLASSES: ClassExports = objc_classes! {

(env, this, _cmd);

@implementation TCUILaunchDelegate: NSObject

+ (id)sharedInstance {
    if let Some(instance) = env.framework_state.uikit.ui_launch_delegate.shared_instance {
        return instance;
    }

    let instance = env.objc.alloc_static_object(this, Box::new(TrivialHostObject), &mut env.mem);
    env.framework_state.uikit.ui_launch_delegate.shared_instance = Some(instance);
    instance
}

- (id)retain { this }
- (())release {}
- (id)autorelease { this }

@end

};
