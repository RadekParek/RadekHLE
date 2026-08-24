use crate::dyld::{export_c_func, FunctionExports, HostDylib};
use crate::mem::{ConstPtr, ConstVoidPtr, MutPtr, MutVoidPtr, SafeRead};
use crate::Environment;

pub const DYLIB: HostDylib = HostDylib {
    path: "/usr/lib/libvulkan.dylib",
    aliases: &[
        "/System/Library/Frameworks/Vulkan.framework/Vulkan",
        "/System/Library/Frameworks/MoltenVK.framework/MoltenVK",
        "/usr/local/lib/libMoltenVK.dylib",
    ],
    class_exports: &[],
    constant_exports: &[],
    function_exports: &[FUNCTIONS],
};

const VK_SUCCESS: i32 = 0;
const VK_NOT_READY: i32 = 1;
const VK_TIMEOUT: i32 = 2;
const VK_INCOMPLETE: i32 = 5;
const VK_ERROR_INITIALIZATION_FAILED: i32 = -3;
const VK_QUEUE_GRAPHICS_BIT: u32 = 0x00000001;
const VK_MAX_EXTENSION_NAME_SIZE: usize = 256;
const VK_MAX_LAYERS: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
struct VkExtensionProperties {
    extension_name: [u8; VK_MAX_EXTENSION_NAME_SIZE],
    spec_version: u32,
}
unsafe impl SafeRead for VkExtensionProperties {}

#[repr(C)]
#[derive(Clone, Copy)]
struct VkLayerProperties {
    layer_name: [u8; VK_MAX_EXTENSION_NAME_SIZE],
    spec_version: u32,
    implementation_version: u32,
    description: [u8; VK_MAX_EXTENSION_NAME_SIZE],
}
unsafe impl SafeRead for VkLayerProperties {}

#[repr(C)]
#[derive(Clone, Copy)]
struct VkQueueFamilyProperties {
    queue_flags: u32,
    queue_count: u32,
    timestamp_valid_bits: u32,
    min_image_transfer_granularity: [u32; 3],
}
unsafe impl SafeRead for VkQueueFamilyProperties {}

fn write_name(target: &mut [u8; VK_MAX_EXTENSION_NAME_SIZE], name: &str) {
    let bytes = name.as_bytes();
    let count = bytes.len().min(target.len().saturating_sub(1));
    target[..count].copy_from_slice(&bytes[..count]);
    target[count] = 0;
}

fn write_u32(env: &mut Environment, address: MutPtr<u32>, value: u32) {
    if !address.is_null() {
        env.mem.write(address, value);
    }
}

fn write_struct<T: Copy + SafeRead>(env: &mut Environment, address: MutVoidPtr, value: T) {
    if !address.is_null() {
        env.mem.write(address.cast(), value);
    }
}

fn alloc_handle(env: &mut Environment) -> MutVoidPtr {
    env.mem.alloc(8).cast()
}

fn vkGetInstanceProcAddr(
    _env: &mut Environment,
    _instance: MutVoidPtr,
    _name: ConstPtr<u8>,
) -> MutVoidPtr {
    MutVoidPtr::null()
}

fn vkGetDeviceProcAddr(
    _env: &mut Environment,
    _device: MutVoidPtr,
    _name: ConstPtr<u8>,
) -> MutVoidPtr {
    MutVoidPtr::null()
}

fn vkEnumerateInstanceVersion(env: &mut Environment, version: MutPtr<u32>) -> i32 {
    write_u32(env, version, (1 << 22) | (3 << 12));
    VK_SUCCESS
}

fn vkEnumerateInstanceExtensionProperties(
    env: &mut Environment,
    _layer_name: ConstPtr<u8>,
    count: MutPtr<u32>,
    properties: MutPtr<VkExtensionProperties>,
) -> i32 {
    let names = [
        "VK_KHR_surface",
        "VK_EXT_metal_surface",
        "VK_KHR_portability_enumeration",
    ];
    if count.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let requested = env.mem.read(count);
    write_u32(env, count, names.len() as u32);
    if !properties.is_null() {
        let write_count = requested.min(names.len() as u32);
        for (index, name) in names.iter().take(write_count as usize).enumerate() {
            let mut property = VkExtensionProperties {
                extension_name: [0; VK_MAX_EXTENSION_NAME_SIZE],
                spec_version: 1,
            };
            write_name(&mut property.extension_name, name);
            write_struct(env, (properties + index as u32).cast(), property);
        }
        if write_count < names.len() as u32 {
            return VK_INCOMPLETE;
        }
    }
    VK_SUCCESS
}

fn vkEnumerateInstanceLayerProperties(
    env: &mut Environment,
    count: MutPtr<u32>,
    properties: MutPtr<VkLayerProperties>,
) -> i32 {
    if count.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let requested = env.mem.read(count);
    let layers = ["MoltenVK"];
    write_u32(env, count, layers.len() as u32);
    if !properties.is_null() {
        let write_count = requested.min(layers.len() as u32);
        for (index, name) in layers.iter().take(write_count as usize).enumerate() {
            let mut property = VkLayerProperties {
                layer_name: [0; VK_MAX_EXTENSION_NAME_SIZE],
                spec_version: 1,
                implementation_version: 1,
                description: [0; VK_MAX_EXTENSION_NAME_SIZE],
            };
            write_name(&mut property.layer_name, name);
            write_name(
                &mut property.description,
                "RadekHLE Vulkan compatibility layer",
            );
            write_struct(env, (properties + index as u32).cast(), property);
        }
        if write_count < layers.len() as u32 {
            return VK_INCOMPLETE;
        }
    }
    VK_SUCCESS
}

fn vkCreateInstance(
    env: &mut Environment,
    _create_info: ConstVoidPtr,
    _allocator: ConstVoidPtr,
    instance: MutPtr<MutVoidPtr>,
) -> i32 {
    if instance.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let handle = alloc_handle(env);
    env.mem.write(instance, handle);
    VK_SUCCESS
}

fn vkDestroyInstance(_env: &mut Environment, _instance: MutVoidPtr, _allocator: ConstVoidPtr) {}

fn vkEnumeratePhysicalDevices(
    env: &mut Environment,
    _instance: MutVoidPtr,
    count: MutPtr<u32>,
    devices: MutPtr<MutVoidPtr>,
) -> i32 {
    if count.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let requested = env.mem.read(count);
    write_u32(env, count, 1);
    if !devices.is_null() && requested != 0 {
        let handle = alloc_handle(env);
        env.mem.write(devices, handle);
    }
    VK_SUCCESS
}

fn vkGetPhysicalDeviceQueueFamilyProperties(
    env: &mut Environment,
    _physical_device: MutVoidPtr,
    count: MutPtr<u32>,
    properties: MutPtr<VkQueueFamilyProperties>,
) {
    if count.is_null() {
        return;
    }
    let requested = env.mem.read(count);
    write_u32(env, count, 1);
    if !properties.is_null() && requested != 0 {
        let property = VkQueueFamilyProperties {
            queue_flags: VK_QUEUE_GRAPHICS_BIT,
            queue_count: 1,
            timestamp_valid_bits: 64,
            min_image_transfer_granularity: [1, 1, 1],
        };
        write_struct(env, properties.cast(), property);
    }
}

fn vkCreateDevice(
    env: &mut Environment,
    _physical_device: MutVoidPtr,
    _create_info: ConstVoidPtr,
    _allocator: ConstVoidPtr,
    device: MutPtr<MutVoidPtr>,
) -> i32 {
    if device.is_null() {
        return VK_ERROR_INITIALIZATION_FAILED;
    }
    let handle = alloc_handle(env);
    env.mem.write(device, handle);
    VK_SUCCESS
}

fn vkGetDeviceQueue(
    env: &mut Environment,
    _device: MutVoidPtr,
    _queue_family_index: u32,
    _queue_index: u32,
    queue: MutPtr<MutVoidPtr>,
) {
    if !queue.is_null() {
        let handle = alloc_handle(env);
        env.mem.write(queue, handle);
    }
}

fn vkDeviceWaitIdle(_env: &mut Environment, _device: MutVoidPtr) -> i32 {
    VK_SUCCESS
}

fn vkDestroyDevice(_env: &mut Environment, _device: MutVoidPtr, _allocator: ConstVoidPtr) {}

pub const FUNCTIONS: FunctionExports = &[
    export_c_func!(vkGetInstanceProcAddr(_, _)),
    export_c_func!(vkGetDeviceProcAddr(_, _)),
    export_c_func!(vkEnumerateInstanceVersion(_)),
    export_c_func!(vkEnumerateInstanceExtensionProperties(_, _, _)),
    export_c_func!(vkEnumerateInstanceLayerProperties(_, _)),
    export_c_func!(vkCreateInstance(_, _, _)),
    export_c_func!(vkDestroyInstance(_, _)),
    export_c_func!(vkEnumeratePhysicalDevices(_, _, _)),
    export_c_func!(vkGetPhysicalDeviceQueueFamilyProperties(_, _, _)),
    export_c_func!(vkCreateDevice(_, _, _, _)),
    export_c_func!(vkGetDeviceQueue(_, _, _, _)),
    export_c_func!(vkDeviceWaitIdle(_)),
    export_c_func!(vkDestroyDevice(_, _)),
];

const _: i32 = VK_NOT_READY;
const _: i32 = VK_TIMEOUT;
const _: usize = VK_MAX_LAYERS;
