/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! Handling of Objective-C objects.

use super::{Class, ClassHostObject};
use crate::mem::{guest_size_of, GuestUSize, Mem, MutPtr, Ptr, SafeRead};
use std::any::{Any, TypeId};
use std::num::NonZeroU32;


#[repr(C, packed)]
pub struct objc_object {
    pub(super) isa: Class,
}
unsafe impl SafeRead for objc_object {}

#[allow(non_camel_case_types)]
pub type id = MutPtr<objc_object>;

#[allow(non_upper_case_globals)]
pub const nil: id = Ptr::null();

pub(super) struct HostObjectEntry {
    host_object: Box<dyn AnyHostObject>,
    refcount: Option<NonZeroU32>,
}

pub trait HostObject: Any + 'static {
    fn as_superclass<'a>(&'a self) -> Option<&'a (dyn AnyHostObject + 'static)> {
        None
    }
    fn as_superclass_mut<'a>(&'a mut self) -> Option<&'a mut (dyn AnyHostObject + 'static)> {
        None
    }
}

#[macro_export]
macro_rules! impl_HostObject_with_superclass {
    ( $ty:ty ) => {
        impl $crate::objc::HostObject for $ty {
            fn as_superclass<'a>(
                &'a self,
            ) -> Option<&'a (dyn $crate::objc::AnyHostObject + 'static)> {
                Some(&self.superclass)
            }
            fn as_superclass_mut<'a>(
                &'a mut self,
            ) -> Option<&'a mut (dyn $crate::objc::AnyHostObject + 'static)> {
                Some(&mut self.superclass)
            }
        }
    };
}
pub use crate::impl_HostObject_with_superclass;

pub trait AnyHostObject: HostObject {
    fn as_any<'a>(&'a self) -> &'a (dyn Any + 'static);
    fn as_any_mut<'a>(&'a mut self) -> &'a mut (dyn Any + 'static);
    fn type_name(&self) -> &'static str;
}
impl<T: HostObject> AnyHostObject for T {
    fn as_any<'a>(&'a self) -> &'a (dyn Any + 'static) {
        self
    }
    fn as_any_mut<'a>(&'a mut self) -> &'a mut (dyn Any + 'static) {
        self
    }
    fn type_name(&self) -> &'static str {
        std::any::type_name::<T>()
    }
}

pub struct TrivialHostObject;
impl HostObject for TrivialHostObject {}

impl super::ObjC {
    pub fn read_isa(object: id, mem: &Mem) -> Class {
        if object == nil {
            return Ptr::null();
        }
        mem.read(object).isa
    }

    fn alloc_object_inner(
        &mut self,
        isa: Class,
        instance_size: GuestUSize,
        host_object: Box<dyn AnyHostObject>,
        mem: &mut Mem,
        refcount: Option<NonZeroU32>,
    ) -> id {
        let guest_object = objc_object { isa };
        let ptr: MutPtr<objc_object> = mem.alloc(instance_size).cast();
        if ptr.is_null() {
            log!(
                "Warning: could not allocate {:#x} bytes for Objective-C object of class {:?}; returning nil",
                instance_size,
                isa
            );
            return nil;
        }
        mem.write(ptr, guest_object);
        self.objects.insert(
            ptr,
            HostObjectEntry {
                host_object,
                refcount,
            },
        );
        ptr
    }

    pub fn alloc_object(
        &mut self,
        isa: Class,
        host_object: Box<dyn AnyHostObject>,
        mem: &mut Mem,
    ) -> id {
        let instance_size = self
            .get_host_object(isa)
            .and_then(|h| h.as_any().downcast_ref::<ClassHostObject>())
            .map(|c| c.instance_size)
            .unwrap_or(guest_size_of::<objc_object>());

        self.alloc_object_inner(
            isa,
            instance_size,
            host_object,
            mem,
            Some(NonZeroU32::new(1).unwrap()),
        )
    }

    pub fn alloc_static_object(
        &mut self,
        isa: Class,
        host_object: Box<dyn AnyHostObject>,
        mem: &mut Mem,
    ) -> id {
        let size = guest_size_of::<objc_object>();
        self.alloc_object_inner(isa, size, host_object, mem, None)
    }

    pub fn register_static_object(
        &mut self,
        guest_object: id,
        host_object: Box<dyn AnyHostObject>,
    ) {
        if guest_object == nil {
            return;
        }
        self.objects.insert(
            guest_object,
            HostObjectEntry {
                host_object,
                refcount: None,
            },
        );
    }

    pub fn get_host_object(&self, object: id) -> Option<&dyn AnyHostObject> {
        if object == nil {
            return None;
        }
        self.objects.get(&object).map(|entry| &*entry.host_object)
    }

    pub fn borrow<T: AnyHostObject + Default + 'static>(&self, object: id) -> &T {
        if let Some(entry) = self.objects.get(&object) {
            let mut host_object: &(dyn AnyHostObject + 'static) = &*entry.host_object;
            loop {
                if let Some(res) = host_object.as_any().downcast_ref() {
                    return res;
                } else if let Some(next) = host_object.as_superclass() {
                    host_object = next;
                } else {
                    break;
                }
            }
        }

        let key = (object, TypeId::of::<T>());
        if let Some(host_object) = self.missing_objects.borrow().get(&key) {
            let host_object = host_object
                .as_any()
                .downcast_ref::<T>()
                .expect("missing-object compatibility cache type mismatch")
                as *const T;
            return unsafe { &*host_object };
        }

        if object == nil {
            log_dbg!(
                "borrow on nil receiver of type {} — returning isolated phantom",
                std::any::type_name::<T>()
            );
        } else if let Some(entry) = self.objects.get(&object) {
            log_once_fmt!(
                "Warning: host object {:?} has type {}, not requested {}; using isolated compatibility state",
                object,
                entry.host_object.type_name(),
                std::any::type_name::<T>(),
            );
        } else {
            log_once_fmt!(
                "Warning: missing host object {:?} of type {}; using isolated compatibility state",
                object,
                std::any::type_name::<T>(),
            );
        }
        let mut missing_objects = self.missing_objects.borrow_mut();
        let host_object = missing_objects
            .entry(key)
            .or_insert_with(|| Box::new(T::default()));
        let host_object = host_object
            .as_any()
            .downcast_ref::<T>()
            .expect("missing-object compatibility cache type mismatch")
            as *const T;
        drop(missing_objects);
        unsafe { &*host_object }
    }

    pub fn borrow_mut<T: AnyHostObject + Default + 'static>(&mut self, object: id) -> &mut T {
        if let Some(entry) = self.objects.get_mut(&object) {
            type Aho = dyn AnyHostObject + 'static;
            let mut host_object: &mut Aho = &mut *entry.host_object;
            loop {
                let current_ptr = host_object as *mut Aho;
                if let Some(res) = unsafe { &mut *current_ptr }.as_any_mut().downcast_mut() {
                    return res;
                }

                let has_super = unsafe { &*current_ptr }.as_superclass().is_some();
                if has_super {
                    host_object = unsafe { &mut *current_ptr }.as_superclass_mut().unwrap();
                } else {
                    break;
                }
            }
        }

        if object == nil {
            log_dbg!(
                "borrow_mut on nil receiver of type {} — returning isolated compatibility state",
                std::any::type_name::<T>()
            );
        }

        if let Some(entry) = self.objects.get(&object) {
            log_once_fmt!(
                "Warning: host object {:?} has type {}, not requested {}; using isolated compatibility state",
                object,
                entry.host_object.type_name(),
                std::any::type_name::<T>(),
            );
        } else {
            log_once_fmt!(
                "Warning: missing host object {:?} of type {}; using isolated compatibility state",
                object,
                std::any::type_name::<T>(),
            );
        }

        let key = (object, TypeId::of::<T>());
        let host_object = self
            .missing_objects
            .get_mut()
            .entry(key)
            .or_insert_with(|| Box::new(T::default()));
        host_object
            .as_any_mut()
            .downcast_mut::<T>()
            .expect("missing-object compatibility cache type mismatch")
    }

    pub fn get_refcount(&mut self, object: id) -> NonZeroU32 {
        let default_rc = NonZeroU32::new(1).unwrap();
        if object == nil {
            return default_rc;
        }

        self.objects
            .get(&object)
            .and_then(|e| e.refcount)
            .unwrap_or(default_rc)
    }

    pub fn increment_refcount(&mut self, object: id) {
        if object == nil {
            return;
        }
        if let Some(entry) = self.objects.get_mut(&object) {
            if let Some(refcount) = entry.refcount.as_mut() {
                if let Some(new_rc) = refcount.get().checked_add(1) {
                    *refcount = NonZeroU32::new(new_rc).unwrap();
                }
            }
        }
    }

    #[must_use]
    pub fn decrement_refcount(&mut self, object: id) -> bool {
        if object == nil {
            return false;
        }
        if let Some(entry) = self.objects.get_mut(&object) {
            if let Some(refcount) = entry.refcount.as_mut() {
                if refcount.get() == 1 {
                    entry.refcount = None;
                    return true;
                } else {
                    *refcount = NonZeroU32::new(refcount.get() - 1).unwrap();
                }
            }
        }
        false
    }

    /// Look up the instance size (in bytes, including the `isa` pointer) for
    /// a class. Falls back to the size of a bare `objc_object` (just the isa)
    /// when the class has no registered [ClassHostObject] — matching the
    /// minimum size the runtime would allocate.
    pub fn class_instance_size(&self, class: Class) -> GuestUSize {
        self.get_host_object(class)
            .and_then(|h| h.as_any().downcast_ref::<ClassHostObject>())
            .map(|c| c.instance_size)
            .unwrap_or(guest_size_of::<objc_object>())
    }

    /// The runtime primitive behind Apple's (deprecated) `NSCopyObject`:
    /// create an exact, shallow, byte-for-byte copy of `object`.
    ///
    /// Per Apple's documentation, `NSCopyObject` "Creates an exact copy of an
    /// object." It allocates a new instance of the same class as `object`
    /// (plus `extra_bytes` of trailing storage) and copies the bytes of the
    /// original instance into it. This is a *shallow* copy: object-pointer
    /// ivars are duplicated as raw pointers without any extra retain, exactly
    /// like the real implementation (classes that adopt `NSCopying` via
    /// `NSCopyObject` are responsible for fixing up retained ivars
    /// themselves). The new object starts with a reference count of 1.
    ///
    /// Returns `nil` if `object` is `nil`.
    pub fn object_copy(&mut self, object: id, extra_bytes: GuestUSize, mem: &mut Mem) -> id {
        if object == nil {
            return nil;
        }
        let isa = Self::read_isa(object, mem);
        let instance_size = self.class_instance_size(isa);
        let total_size = instance_size.saturating_add(extra_bytes);

        let new_object: id = mem.alloc(total_size).cast();
        // Copy the original instance's bytes verbatim (this includes the isa
        // pointer, all ivars, and — if requested — leaves the trailing
        // `extra_bytes` zero-initialised, which `mem.alloc` guarantees).
        let src_bytes: Vec<u8> = mem.bytes_at(object.cast(), instance_size).to_vec();
        mem.bytes_at_mut(new_object.cast(), instance_size)
            .copy_from_slice(&src_bytes);

        // Register a host-side record so the copy is a first-class object
        // with its own reference count. A bitwise NSCopyObject copy does not
        // duplicate host-side state, so we attach a TrivialHostObject — this
        // matches the real runtime, where NSCopyObject is only used by classes
        // that keep all of their state in guest-memory ivars.
        self.objects.insert(
            new_object,
            HostObjectEntry {
                host_object: Box::new(TrivialHostObject),
                refcount: Some(NonZeroU32::new(1).unwrap()),
            },
        );
        new_object
    }

    pub fn dealloc_object(&mut self, object: id, mem: &mut Mem) {
        if object == nil {
            return;
        }

        // ARC weak-reference contract: zero out every `__weak` slot
        // that referred to this object before its memory is freed, so
        // subsequent `objc_loadWeakRetained` calls correctly observe
        // `nil`. This must happen before we drop the host object,
        // because the writeback uses guest memory only.
        self.zero_weak_references_for(object, mem);
        self.missing_objects
            .borrow_mut()
            .retain(|(candidate, _), _| *candidate != object);

        if let Some(entry) = self.objects.remove(&object) {
            std::mem::drop(entry.host_object);
            mem.free(object.cast());
        }
    }
}
