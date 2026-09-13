use super::{
    c_string, objc_field, objc_number, objc_number_float, objc_object, objc_text, set_objc_field,
    A64_KIND_ARRAY, A64_KIND_DATA, A64_KIND_DICTIONARY, A64_KIND_GENERIC, A64_KIND_MUTABLE_ARRAY,
    A64_KIND_MUTABLE_DICTIONARY, A64_KIND_MUTABLE_STRING, A64_KIND_STRING,
};
use crate::mem64::Mem64;
use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

#[derive(Clone, Copy)]
enum StubKind {
    ArrayCreate(bool),
    ArrayCount,
    ArrayValue,
    ArrayGetValues,
    ArrayFirstIndex,
    ArrayContains,
    ArrayAppend,
    ArrayInsert,
    ArraySet,
    ArrayReplace,
    ArrayExchange,
    ArrayRemove,
    ArrayRemoveAll,
    DictionaryCreate(bool),
    DictionaryCopy,
    DictionaryCount,
    DictionaryValue,
    DictionaryContainsKey,
    DictionaryContainsValue,
    DictionarySet,
    DictionaryRemove,
    DictionaryRemoveAll,
    DictionaryKeysAndValues,
    StringCreate(bool),
    StringCreateCharacters,
    StringCreateBytes,
    StringLength,
    StringCharacter,
    StringGetCString,
    StringCStringPointer,
    StringMaximumSize,
    StringAppend,
    StringAppendCString,
    DataCreate(bool),
    DataCreateCopy,
    DataBytePointer,
    DataGetBytes,
    DataLength,
    DataAppend,
    NumberCreate,
    NumberValue,
    Retain,
    Release,
    Allocator,
    Null,
    GenericPointer,
    GenericReceiver,
    GenericZero,
    ExceptionWhat,
    ExceptionConstructor,
    ExceptionPointer,
    CryptoNoop,
    CMTimeGetSeconds,
    CMTimeMakeWithSeconds,
    CVTextureCacheCreate,
    CVTextureName,
    CVTextureTarget,
    DispatchDataApply,
    Asprintf,
    GetProgname,
    DigitToInt,
    IsXDigit,
    StringCompare,
    VmMap,
    VmReadOverwrite,
}

fn normalized(symbol: &str) -> &str {
    let symbol = symbol.trim_start_matches('_');
    symbol.strip_prefix('_').unwrap_or(symbol)
}

fn compatibility_kind(symbol: &str) -> Option<StubKind> {
    match symbol {
        "CCHmacInit" | "CCHmacUpdate" | "CCHmacFinal" => Some(StubKind::CryptoNoop),
        "CMTimeGetSeconds" => Some(StubKind::CMTimeGetSeconds),
        "CMTimeMakeWithSeconds" => Some(StubKind::CMTimeMakeWithSeconds),
        "CVOpenGLESTextureCacheCreate"
        | "CVOpenGLESTextureCacheCreateTextureFromImage"
        | "CVOpenGLESTextureCacheFlush" => Some(StubKind::CVTextureCacheCreate),
        "CVOpenGLESTextureGetName" => Some(StubKind::CVTextureName),
        "CVOpenGLESTextureGetTarget" => Some(StubKind::CVTextureTarget),
        "dispatch_data_apply" => Some(StubKind::DispatchDataApply),
        "dispatch_data_create" | "UTTypeCopyPreferredTagWithClass"
        | "UTTypeCreatePreferredIdentifierForTag" => Some(StubKind::GenericPointer),
        "dispatch_data_get_size" | "dispatch_read" | "dispatch_write" => {
            Some(StubKind::GenericZero)
        }
        "asprintf" => Some(StubKind::Asprintf),
        "getprogname" => Some(StubKind::GetProgname),
        "digittoint" => Some(StubKind::DigitToInt),
        "isxdigit" => Some(StubKind::IsXDigit),
        "strcoll" => Some(StubKind::StringCompare),
        "vm_map" => Some(StubKind::VmMap),
        "vm_read_overwrite" => Some(StubKind::VmReadOverwrite),
        "regcomp" | "regexec" | "readdir_r" | "nftw" | "utime" | "pathconf"
        | "arc4random_buf" | "class_conformsToProtocol" | "protocol_getMethodDescription"
        | "objc_exception_rethrow" | "objc_terminate" | "exception_raise"
        | "exception_raise_state" | "exception_raise_state_identity" | "mach_make_memory_entry_64"
        | "mach_port_mod_refs" | "mach_port_move_member" | "mach_port_request_notification"
        | "thread_get_exception_ports" | "thread_swap_exception_ports" | "kill" | "raise"
        | "DNSServiceNATPortMappingCreate" | "DNSServiceProcessResult" | "DNSServiceRefDeallocate"
        | "___objc_personality_v0" | "objc_personality_v0" | "cxa_bad_cast"
        | "ZSt17rethrow_exceptionSt13exception_ptr"
        | "ZSt18uncaught_exceptionv" => Some(StubKind::GenericZero),
        "cxa_get_exception_ptr" => Some(StubKind::ExceptionPointer),
        "ZSt17current_exceptionv" => Some(StubKind::GenericPointer),
        "_hash_create" | "hash_create" | "_hash_search" | "hash_search" | "getpwnam" => {
            Some(StubKind::GenericPointer)
        }
        symbol if symbol.starts_with("ZNKSt9exception4what")
            || symbol.starts_with("ZNKSt13runtime_error4what") => Some(StubKind::ExceptionWhat),
        symbol if symbol.starts_with("ZNSt11logic_errorC")
            || symbol.starts_with("ZNSt13runtime_errorC") => Some(StubKind::ExceptionConstructor),
        symbol if symbol.starts_with("ZN7plcrash") => {
            if symbol.contains("C1") || symbol.contains("C2") {
                Some(StubKind::GenericReceiver)
            } else {
                Some(StubKind::GenericZero)
            }
        }
        symbol if symbol.starts_with("ZThn") || symbol.starts_with("ZTv") => {
            Some(StubKind::GenericReceiver)
        }
        symbol if symbol.starts_with("ZNSt") || symbol.starts_with("ZNKSt") || symbol.starts_with("ZSt") => {
            if symbol.contains("D1") || symbol.contains("D2") {
                Some(StubKind::GenericReceiver)
            } else if symbol.contains("what") {
                Some(StubKind::ExceptionWhat)
            } else if symbol.contains("C1") || symbol.contains("C2") {
                Some(StubKind::ExceptionConstructor)
            } else {
                Some(StubKind::GenericReceiver)
            }
        }
        _ => None,
    }
}

fn generic_kind(symbol: &str) -> Option<StubKind> {
    let symbol = normalized(symbol);
    if let Some(kind) = compatibility_kind(symbol) {
        return Some(kind);
    }
    if symbol.starts_with("CFArray") {
        return Some(match symbol {
            "CFArrayCreate" => StubKind::ArrayCreate(false),
            "CFArrayCreateMutable" => StubKind::ArrayCreate(true),
            "CFArrayGetCount" => StubKind::ArrayCount,
            "CFArrayGetValueAtIndex" => StubKind::ArrayValue,
            "CFArrayGetValues" => StubKind::ArrayGetValues,
            "CFArrayGetFirstIndexOfValue" => StubKind::ArrayFirstIndex,
            "CFArrayContainsValue" => StubKind::ArrayContains,
            "CFArrayAppendValue" => StubKind::ArrayAppend,
            "CFArrayInsertValueAtIndex" => StubKind::ArrayInsert,
            "CFArraySetValueAtIndex" => StubKind::ArraySet,
            "CFArrayReplaceValues" => StubKind::ArrayReplace,
            "CFArrayExchangeValuesAtIndices" => StubKind::ArrayExchange,
            "CFArrayRemoveValueAtIndex" => StubKind::ArrayRemove,
            "CFArrayRemoveAllValues" => StubKind::ArrayRemoveAll,
            _ => StubKind::GenericPointer,
        });
    }
    if symbol.starts_with("CFDictionary") {
        return Some(match symbol {
            "CFDictionaryCreate" => StubKind::DictionaryCreate(false),
            "CFDictionaryCreateMutable" => StubKind::DictionaryCreate(true),
            "CFDictionaryCreateCopy" => StubKind::DictionaryCopy,
            "CFDictionaryGetCount" => StubKind::DictionaryCount,
            "CFDictionaryGetValue" => StubKind::DictionaryValue,
            "CFDictionaryContainsKey" => StubKind::DictionaryContainsKey,
            "CFDictionaryContainsValue" => StubKind::DictionaryContainsValue,
            "CFDictionarySetValue" => StubKind::DictionarySet,
            "CFDictionaryRemoveValue" => StubKind::DictionaryRemove,
            "CFDictionaryRemoveAllValues" => StubKind::DictionaryRemoveAll,
            "CFDictionaryGetKeysAndValues" => StubKind::DictionaryKeysAndValues,
            _ => StubKind::GenericPointer,
        });
    }
    if symbol.starts_with("CFString") {
        return Some(match symbol {
            "CFStringCreate" => StubKind::StringCreate(true),
            "CFStringCreateWithCString" => StubKind::StringCreate(false),
            "CFStringCreateWithCharacters" => StubKind::StringCreateCharacters,
            "CFStringCreateWithBytes" => StubKind::StringCreateBytes,
            "CFStringCreateMutable" => StubKind::StringCreate(false),
            "CFStringGetLength" => StubKind::StringLength,
            "CFStringGetCharacterAtIndex" => StubKind::StringCharacter,
            "CFStringGetCString" => StubKind::StringGetCString,
            "CFStringGetCStringPtr" => StubKind::StringCStringPointer,
            "CFStringGetMaximumSizeForEncoding" => StubKind::StringMaximumSize,
            "CFStringAppend" => StubKind::StringAppend,
            "CFStringAppendCString" => StubKind::StringAppendCString,
            _ => StubKind::GenericPointer,
        });
    }
    if symbol.starts_with("CFData") {
        return Some(match symbol {
            "CFDataCreate" => StubKind::DataCreate(false),
            "CFDataCreateMutable" => StubKind::DataCreate(true),
            "CFDataCreateCopy" => StubKind::DataCreateCopy,
            "CFDataGetBytePtr" | "CFDataGetMutableBytePtr" => StubKind::DataBytePointer,
            "CFDataGetBytes" => StubKind::DataGetBytes,
            "CFDataGetLength" => StubKind::DataLength,
            "CFDataAppendBytes" => StubKind::DataAppend,
            _ => StubKind::GenericPointer,
        });
    }
    if symbol.starts_with("CFNumber") {
        return Some(match symbol {
            "CFNumberCreate" => StubKind::NumberCreate,
            "CFNumberGetValue" => StubKind::NumberValue,
            _ => StubKind::GenericPointer,
        });
    }
    match symbol {
        "CFNull" | "kCFNull" => Some(StubKind::Null),
        "CFGetRetainCount" => Some(StubKind::NumberValue),
        "CFRetain" | "CFAutorelease" => Some(StubKind::Retain),
        "CFRelease" => Some(StubKind::Release),
        "CFAllocatorCreate" | "CFAllocatorGetDefault" | "kCFAllocatorDefault" => {
            Some(StubKind::Allocator)
        }
        "NSArrayObjectAtIndex" => Some(StubKind::ArrayValue),
        "NSArrayCount" => Some(StubKind::ArrayCount),
        "NSDictionaryObjectForKey" => Some(StubKind::DictionaryValue),
        "NSStringFromClass" | "NSClassFromString" | "NSStringFromSelector" => {
            Some(StubKind::GenericPointer)
        }
        _ if symbol.starts_with("CF")
            || symbol.starts_with("CG")
            || symbol.starts_with("NS")
            || symbol.starts_with("UI") =>
        {
            if symbol.contains("Create")
                || symbol.contains("Copy")
                || symbol.contains("Alloc")
                || symbol.contains("New")
                || symbol.contains("Class")
                || symbol.contains("FromString")
                || symbol.contains("With")
            {
                Some(StubKind::GenericPointer)
            } else if symbol.contains("Init") {
                Some(StubKind::GenericReceiver)
            } else if symbol.contains("Set")
                || symbol.contains("Add")
                || symbol.contains("Append")
                || symbol.contains("Remove")
                || symbol.contains("Release")
                || symbol.contains("Destroy")
            {
                Some(StubKind::GenericReceiver)
            } else if symbol.contains("Count")
                || symbol.contains("Length")
                || symbol.contains("Value")
                || symbol.contains("TypeID")
                || symbol.contains("Index")
                || symbol.contains("Size")
                || symbol.contains("Width")
                || symbol.contains("Height")
                || symbol.contains("Is")
                || symbol.contains("Has")
                || symbol.contains("Equal")
                || symbol.contains("Compare")
                || symbol.contains("Status")
                || symbol.contains("Error")
            {
                Some(StubKind::GenericZero)
            } else {
                Some(StubKind::GenericPointer)
            }
        }
        _ => None,
    }
}

pub(super) fn is_known(symbol: &str) -> bool {
    generic_kind(symbol).is_some()
}

fn array_values(mem: &Mem64, array: u64) -> Vec<u64> {
    let count = objc_field(mem, array, 56).min(4096);
    let elements = objc_field(mem, array, 64);
    (0..count)
        .filter_map(|index| mem.read_u64(elements.saturating_add(index * 8)).ok())
        .collect()
}
fn range_values(mem: &Mem64, range: u64, available: usize) -> (usize, usize) {
    if range == 0 {
        return (0, 0);
    }
    let location = mem.read_u64(range).unwrap_or(0).min(available as u64) as usize;
    let length = mem
        .read_u64(range.saturating_add(8))
        .unwrap_or(0)
        .min((available - location) as u64) as usize;
    (location, length)
}

fn guest_values(mem: &Mem64, pointer: u64, count: u64) -> Vec<u64> {
    if pointer == 0 {
        return Vec::new();
    }
    (0..count.min(4096))
        .filter_map(|index| mem.read_u64(pointer.saturating_add(index * 8)).ok())
        .collect()
}

fn write_array_values(mem: &mut Mem64, array: u64, values: &[u64]) -> Result<(), String> {
    let elements = mem
        .alloc_zeroed((values.len().max(1) as u64).saturating_mul(8))
        .map_err(str::to_owned)?;
    for (index, value) in values.iter().copied().enumerate() {
        mem.write_u64(elements + index as u64 * 8, value)
            .map_err(str::to_owned)?;
    }
    set_objc_field(mem, array, 56, values.len() as u64);
    set_objc_field(mem, array, 64, elements);
    Ok(())
}

fn dictionary_pairs(mem: &Mem64, dictionary: u64) -> (Vec<u64>, Vec<u64>) {
    let count = objc_field(mem, dictionary, 56).min(4096);
    let keys = objc_field(mem, dictionary, 64);
    let values = objc_field(mem, dictionary, 72);
    let keys = (0..count)
        .filter_map(|index| mem.read_u64(keys.saturating_add(index * 8)).ok())
        .collect();
    let values = (0..count)
        .filter_map(|index| mem.read_u64(values.saturating_add(index * 8)).ok())
        .collect();
    (keys, values)
}

fn write_dictionary_pairs(
    mem: &mut Mem64,
    dictionary: u64,
    keys: &[u64],
    values: &[u64],
) -> Result<(), String> {
    let count = keys.len().min(values.len());
    let key_storage = mem
        .alloc_zeroed((count.max(1) as u64).saturating_mul(8))
        .map_err(str::to_owned)?;
    let value_storage = mem
        .alloc_zeroed((count.max(1) as u64).saturating_mul(8))
        .map_err(str::to_owned)?;
    for index in 0..count {
        mem.write_u64(key_storage + index as u64 * 8, keys[index])
            .map_err(str::to_owned)?;
        mem.write_u64(value_storage + index as u64 * 8, values[index])
            .map_err(str::to_owned)?;
    }
    set_objc_field(mem, dictionary, 56, count as u64);
    set_objc_field(mem, dictionary, 64, key_storage);
    set_objc_field(mem, dictionary, 72, value_storage);
    Ok(())
}

fn dictionary_create(
    mem: &mut Mem64,
    keys_pointer: u64,
    values_pointer: u64,
    count: u64,
    mutable: bool,
) -> Result<u64, String> {
    let count = count.min(4096);
    let keys = if keys_pointer == 0 {
        Vec::new()
    } else {
        (0..count)
            .filter_map(|index| mem.read_u64(keys_pointer + index * 8).ok())
            .collect::<Vec<_>>()
    };
    let values = if values_pointer == 0 {
        Vec::new()
    } else {
        (0..count)
            .filter_map(|index| mem.read_u64(values_pointer + index * 8).ok())
            .collect::<Vec<_>>()
    };
    let object = objc_object(
        mem,
        if mutable {
            A64_KIND_MUTABLE_DICTIONARY
        } else {
            A64_KIND_DICTIONARY
        },
    )?;
    write_dictionary_pairs(mem, object, &keys, &values)?;
    Ok(object)
}

fn string_from_utf16(mem: &Mem64, pointer: u64, count: u64) -> String {
    let mut value = String::new();
    for index in 0..count.min(1_048_576) {
        let Ok(character) = mem.read_u16(pointer.saturating_add(index * 2)) else {
            break;
        };
        value.push(char::from_u32(u32::from(character)).unwrap_or('\u{fffd}'));
    }
    value
}

fn replace_string(mem: &mut Mem64, object: u64, bytes: &[u8]) -> Result<(), String> {
    let pointer = mem
        .alloc_zeroed(bytes.len() as u64 + 1)
        .map_err(str::to_owned)?;
    if !bytes.is_empty() {
        mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
    }
    mem.write_u8(pointer + bytes.len() as u64, 0)
        .map_err(str::to_owned)?;
    set_objc_field(mem, object, 56, pointer);
    set_objc_field(mem, object, 64, bytes.len() as u64);
    Ok(())
}

fn replace_data(mem: &mut Mem64, object: u64, bytes: &[u8]) -> Result<(), String> {
    let pointer = mem
        .alloc_zeroed(bytes.len().max(1) as u64)
        .map_err(str::to_owned)?;
    if !bytes.is_empty() {
        mem.write_bytes(pointer, bytes).map_err(str::to_owned)?;
    }
    set_objc_field(mem, object, 56, pointer);
    set_objc_field(mem, object, 64, bytes.len() as u64);
    Ok(())
}

fn read_number_as_i64(mem: &Mem64, pointer: u64, number_type: u64) -> i64 {
    match number_type {
        1 | 7 => mem
            .read_u8(pointer)
            .map(|value| value as i8)
            .unwrap_or_default() as i64,
        2 | 8 => mem
            .read_u16(pointer)
            .map(|value| i16::from_le_bytes(value.to_le_bytes()) as i64)
            .unwrap_or_default(),
        3 | 9 => mem
            .read_u32(pointer)
            .map(|value| i32::from_le_bytes(value.to_le_bytes()) as i64)
            .unwrap_or_default(),
        4 | 10 | 11 | 14 | 15 => mem
            .read_u64(pointer)
            .map(|value| i64::from_le_bytes(value.to_le_bytes()))
            .unwrap_or_default(),
        _ => mem
            .read_u64(pointer)
            .map(|value| i64::from_le_bytes(value.to_le_bytes()))
            .unwrap_or_default(),
    }
}

fn read_number_as_f64(mem: &Mem64, pointer: u64, number_type: u64) -> f64 {
    match number_type {
        5 | 12 => mem
            .read_u32(pointer)
            .map(f32::from_bits)
            .unwrap_or_default() as f64,
        6 | 13 | 16 => mem
            .read_u64(pointer)
            .map(f64::from_bits)
            .unwrap_or_default(),
        _ => read_number_as_i64(mem, pointer, number_type) as f64,
    }
}

fn write_number_value(
    mem: &mut Mem64,
    pointer: u64,
    number_type: u64,
    value: f64,
) -> Result<(), String> {
    let result = match number_type {
        1 | 7 => mem.write_u8(pointer, value as i8 as u8),
        2 | 8 => mem.write_u16(pointer, (value as i16) as u16),
        3 | 9 => mem.write_u32(pointer, (value as i32) as u32),
        4 | 10 | 11 | 14 | 15 => mem.write_u64(pointer, (value as i64) as u64),
        5 | 12 => mem.write_u32(pointer, (value as f32).to_bits()),
        6 | 13 | 16 => mem.write_u64(pointer, value.to_bits()),
        _ => mem.write_u64(pointer, (value as i64) as u64),
    };
    result.map_err(str::to_owned)
}

fn generic_pointer(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
) -> Result<(), String> {
    let receiver = context.regs[0];
    let value = if receiver != 0 && mem.allocation_size(receiver).is_some() {
        receiver
    } else {
        objc_object(mem, A64_KIND_GENERIC)?
    };
    super::return_value(context, value);
    Ok(())
}

pub(super) fn dispatch(
    mem: &mut Mem64,
    context: &mut touchHLE_DynarmicA64Context,
    symbol: &str,
) -> Result<bool, String> {
    let Some(kind) = generic_kind(symbol) else {
        return Ok(false);
    };
    let symbol = normalized(symbol);
    log_once_fmt!(
        "ARM64 unresolved import implementation selected: {} [repeated calls suppressed]",
        symbol
    );
    match kind {
        StubKind::ExceptionWhat => {
            let receiver = context.regs[0];
            let message = if receiver != 0 {
                let pointer = objc_field(mem, receiver, 56);
                c_string(mem, pointer).filter(|bytes| !bytes.is_empty())
            } else {
                None
            };
            let message = message.unwrap_or_else(|| b"std::exception".to_vec());
            let pointer = mem
                .alloc_zeroed(message.len() as u64 + 1)
                .map_err(str::to_owned)?;
            mem.write_bytes(pointer, &message).map_err(str::to_owned)?;
            mem.write_u8(pointer + message.len() as u64, 0)
                .map_err(str::to_owned)?;
            super::return_value(context, pointer);
        }
        StubKind::ExceptionConstructor => {
            if context.regs[0] != 0 && mem.allocation_size(context.regs[0]).is_some() {
                if context.regs[1] != 0 && mem.allocation_size(context.regs[1]).is_some() {
                    let pointer = context.regs[1];
                    set_objc_field(mem, context.regs[0], 56, pointer);
                }
                super::return_value(context, context.regs[0]);
            } else {
                super::return_value(context, 0);
            }
        }
        StubKind::ExceptionPointer => super::return_value(context, context.regs[0]),
        StubKind::CryptoNoop => {
            if context.regs[0] != 0 && context.regs[1] != 0 {
                let _ = mem.write_bytes(context.regs[1], &vec![0u8; 64]);
            }
            super::return_value(context, 0);
        }
        StubKind::CMTimeGetSeconds => {
            let value = if context.regs[0] != 0 && mem.allocation_size(context.regs[0]).is_some() {
                let numerator = mem.read_u64(context.regs[0]).unwrap_or(0) as i64;
                let scale = mem.read_u32(context.regs[0] + 8).unwrap_or(0) as i32;
                if scale == 0 { 0.0 } else { numerator as f64 / scale as f64 }
            } else {
                0.0
            };
            context.vectors[0][0] = value.to_bits();
            super::return_value(context, value.to_bits());
        }
        StubKind::CMTimeMakeWithSeconds => {
            let seconds = f64::from_bits(context.vectors[0][0]);
            let timescale = context.regs[0] as i32;
            let output = context.regs[8];
            if output != 0 && mem.allocation_size(output).is_some() {
                mem.write_u64(output, (seconds * timescale as f64).round() as i64 as u64)
                    .map_err(str::to_owned)?;
                mem.write_u32(output + 8, timescale as u32)
                    .map_err(str::to_owned)?;
                mem.write_u32(output + 12, 1).map_err(str::to_owned)?;
                mem.write_u64(output + 16, 0).map_err(str::to_owned)?;
                super::return_value(context, output);
            } else {
                super::return_value(context, 0);
            }
        }
        StubKind::CVTextureCacheCreate => {
            if symbol == "CVOpenGLESTextureCacheCreate" && context.regs[4] != 0 {
                let object = objc_object(mem, A64_KIND_GENERIC)?;
                if mem.allocation_size(context.regs[4]).is_some() {
                    mem.write_u64(context.regs[4], object).map_err(str::to_owned)?;
                }
            }
            super::return_value(context, 0);
        }
        StubKind::CVTextureName => {
            super::return_value(context, objc_field(mem, context.regs[0], 56) as u32 as u64);
        }
        StubKind::CVTextureTarget => super::return_value(context, 0x0de1),
        StubKind::DispatchDataApply => super::return_value(context, 1),
        StubKind::Asprintf => {
            let output = context.regs[0];
            let format = c_string(mem, context.regs[1]).unwrap_or_default();
            let pointer = mem
                .alloc_zeroed(format.len() as u64 + 1)
                .map_err(str::to_owned)?;
            mem.write_bytes(pointer, &format).map_err(str::to_owned)?;
            mem.write_u8(pointer + format.len() as u64, 0)
                .map_err(str::to_owned)?;
            if output != 0 && mem.allocation_size(output).is_some() {
                mem.write_u64(output, pointer).map_err(str::to_owned)?;
            }
            super::return_value(context, format.len() as u64);
        }
        StubKind::GetProgname => {
            let value = b"RadekHLE";
            let pointer = mem
                .alloc_zeroed(value.len() as u64 + 1)
                .map_err(str::to_owned)?;
            mem.write_bytes(pointer, value).map_err(str::to_owned)?;
            super::return_value(context, pointer);
        }
        StubKind::DigitToInt => {
            let value = (context.regs[0] as u8 as char).to_digit(16).unwrap_or(0);
            super::return_value(context, value as u64);
        }
        StubKind::IsXDigit => {
            super::return_value(context, u64::from((context.regs[0] as u8 as char).is_ascii_hexdigit()));
        }
        StubKind::StringCompare => {
            let left = c_string(mem, context.regs[0]).unwrap_or_default();
            let right = c_string(mem, context.regs[1]).unwrap_or_default();
            let value = match left.cmp(&right) {
                std::cmp::Ordering::Less => -1_i64,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            super::return_value(context, value as u64);
        }
        StubKind::VmMap => {
            let size = context.regs[2].min(64 * 1024 * 1024).max(1);
            let address = mem.alloc_zeroed(size).map_err(str::to_owned)?;
            if context.regs[1] != 0 && mem.allocation_size(context.regs[1]).is_some() {
                mem.write_u64(context.regs[1], address).map_err(str::to_owned)?;
            }
            super::return_value(context, 0);
        }
        StubKind::VmReadOverwrite => super::return_value(context, 0),
        StubKind::ArrayCreate(mutable) => {
            let (values, count) = if mutable {
                (0, 0)
            } else {
                (context.regs[1], context.regs[2].min(4096))
            };
            let objects = if values == 0 {
                Vec::new()
            } else {
                (0..count)
                    .filter_map(|index| mem.read_u64(values + index * 8).ok())
                    .collect::<Vec<_>>()
            };
            let object = super::objc_array_with_kind(
                mem,
                if mutable {
                    A64_KIND_MUTABLE_ARRAY
                } else {
                    A64_KIND_ARRAY
                },
                &objects,
            )?;
            super::return_value(context, object);
        }
        StubKind::ArrayCount => super::return_value(context, objc_field(mem, context.regs[0], 56)),
        StubKind::ArrayValue => {
            let values = array_values(mem, context.regs[0]);
            super::return_value(
                context,
                values.get(context.regs[1] as usize).copied().unwrap_or(0),
            );
        }
        StubKind::ArrayGetValues => {
            let values = array_values(mem, context.regs[0]);
            let (location, length) = range_values(mem, context.regs[1], values.len());
            if context.regs[2] != 0 {
                for (index, value) in values[location..location + length]
                    .iter()
                    .copied()
                    .enumerate()
                {
                    mem.write_u64(context.regs[2] + index as u64 * 8, value)
                        .map_err(str::to_owned)?;
                }
            }
            super::return_value(context, 0);
        }
        StubKind::ArrayFirstIndex => {
            let values = array_values(mem, context.regs[0]);
            let (location, length) = range_values(mem, context.regs[1], values.len());
            let index = values[location..location + length]
                .iter()
                .position(|value| *value == context.regs[2])
                .map(|index| location + index)
                .map(|index| index as u64)
                .unwrap_or(u64::MAX);
            super::return_value(context, index);
        }
        StubKind::ArrayContains => {
            let values = array_values(mem, context.regs[0]);
            let (location, length) = range_values(mem, context.regs[1], values.len());
            super::return_value(
                context,
                u64::from(
                    values[location..location + length]
                        .iter()
                        .any(|value| *value == context.regs[2]),
                ),
            );
        }
        StubKind::ArrayAppend => {
            let mut values = array_values(mem, context.regs[0]);
            values.push(context.regs[1]);
            write_array_values(mem, context.regs[0], &values)?;
            super::return_value(context, 0);
        }
        StubKind::ArrayInsert => {
            let mut values = array_values(mem, context.regs[0]);
            let index = (context.regs[1] as usize).min(values.len());
            values.insert(index, context.regs[2]);
            write_array_values(mem, context.regs[0], &values)?;
            super::return_value(context, 0);
        }
        StubKind::ArraySet => {
            let mut values = array_values(mem, context.regs[0]);
            let index = context.regs[1] as usize;
            if index < values.len() {
                values[index] = context.regs[2];
                write_array_values(mem, context.regs[0], &values)?;
            }
            super::return_value(context, 0);
        }
        StubKind::ArrayReplace => {
            let mut values = array_values(mem, context.regs[0]);
            let (location, length) = range_values(mem, context.regs[1], values.len());
            let replacement = guest_values(mem, context.regs[2], context.regs[3]);
            values.splice(location..location + length, replacement);
            write_array_values(mem, context.regs[0], &values)?;
            super::return_value(context, 0);
        }
        StubKind::ArrayExchange => {
            let mut values = array_values(mem, context.regs[0]);
            let first = context.regs[1] as usize;
            let second = context.regs[2] as usize;
            if first < values.len() && second < values.len() {
                values.swap(first, second);
                write_array_values(mem, context.regs[0], &values)?;
            }
            super::return_value(context, 0);
        }
        StubKind::ArrayRemove => {
            let mut values = array_values(mem, context.regs[0]);
            let index = context.regs[1] as usize;
            if index < values.len() {
                values.remove(index);
                write_array_values(mem, context.regs[0], &values)?;
            }
            super::return_value(context, 0);
        }
        StubKind::ArrayRemoveAll => {
            write_array_values(mem, context.regs[0], &[])?;
            super::return_value(context, 0);
        }
        StubKind::DictionaryCreate(mutable) => {
            let (keys, values, count) = if mutable {
                (0, 0, 0)
            } else {
                (context.regs[1], context.regs[2], context.regs[3].min(4096))
            };
            let object = dictionary_create(mem, keys, values, count, mutable)?;
            super::return_value(context, object);
        }
        StubKind::DictionaryCopy => {
            let (keys, values) = dictionary_pairs(mem, context.regs[1]);
            let object = objc_object(mem, A64_KIND_DICTIONARY)?;
            write_dictionary_pairs(mem, object, &keys, &values)?;
            super::return_value(context, object);
        }
        StubKind::DictionaryCount => {
            super::return_value(context, objc_field(mem, context.regs[0], 56));
        }
        StubKind::DictionaryValue => {
            let (keys, values) = dictionary_pairs(mem, context.regs[0]);
            let value = keys
                .iter()
                .position(|key| *key == context.regs[1])
                .and_then(|index| values.get(index).copied())
                .unwrap_or(0);
            super::return_value(context, value);
        }
        StubKind::DictionaryContainsKey => {
            let (keys, _) = dictionary_pairs(mem, context.regs[0]);
            super::return_value(context, u64::from(keys.contains(&context.regs[1])));
        }
        StubKind::DictionaryContainsValue => {
            let (_, values) = dictionary_pairs(mem, context.regs[0]);
            super::return_value(context, u64::from(values.contains(&context.regs[1])));
        }
        StubKind::DictionarySet => {
            let (mut keys, mut values) = dictionary_pairs(mem, context.regs[0]);
            if let Some(index) = keys.iter().position(|key| *key == context.regs[1]) {
                values[index] = context.regs[2];
            } else {
                keys.push(context.regs[1]);
                values.push(context.regs[2]);
            }
            write_dictionary_pairs(mem, context.regs[0], &keys, &values)?;
            super::return_value(context, 0);
        }
        StubKind::DictionaryRemove => {
            let (mut keys, mut values) = dictionary_pairs(mem, context.regs[0]);
            if let Some(index) = keys.iter().position(|key| *key == context.regs[1]) {
                keys.remove(index);
                values.remove(index);
            }
            write_dictionary_pairs(mem, context.regs[0], &keys, &values)?;
            super::return_value(context, 0);
        }
        StubKind::DictionaryRemoveAll => {
            write_dictionary_pairs(mem, context.regs[0], &[], &[])?;
            super::return_value(context, 0);
        }
        StubKind::DictionaryKeysAndValues => {
            let (keys, values) = dictionary_pairs(mem, context.regs[0]);
            for (index, value) in keys.iter().copied().enumerate() {
                if context.regs[1] != 0 {
                    mem.write_u64(context.regs[1] + index as u64 * 8, value)
                        .map_err(str::to_owned)?;
                }
            }
            for (index, value) in values.iter().copied().enumerate() {
                if context.regs[2] != 0 {
                    mem.write_u64(context.regs[2] + index as u64 * 8, value)
                        .map_err(str::to_owned)?;
                }
            }
            super::return_value(context, 0);
        }
        StubKind::StringCreate(utf16) => {
            let value = if symbol == "CFStringCreateMutable" {
                String::new()
            } else if symbol == "CFStringCreate" && utf16 {
                if context.regs[3] == 0x0800_0100 {
                    mem.read_bytes(context.regs[1], context.regs[2].min(1_048_576))
                        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                        .unwrap_or_default()
                } else {
                    string_from_utf16(mem, context.regs[1], context.regs[2])
                }
            } else {
                c_string(mem, context.regs[1])
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default()
            };
            let kind = if symbol == "CFStringCreateMutable" {
                A64_KIND_MUTABLE_STRING
            } else {
                A64_KIND_STRING
            };
            let object = super::objc_string_with_kind(mem, &value, kind)?;
            super::return_value(context, object);
        }
        StubKind::StringCreateCharacters => {
            let value = string_from_utf16(mem, context.regs[1], context.regs[2]);
            let object = super::objc_string_with_kind(mem, &value, A64_KIND_STRING)?;
            super::return_value(context, object);
        }
        StubKind::StringCreateBytes => {
            let value = if context.regs[3] == 0x0800_0100 {
                mem.read_bytes(context.regs[1], context.regs[2].min(1_048_576))
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default()
            } else {
                string_from_utf16(mem, context.regs[1], context.regs[2] / 2)
            };
            let object = super::objc_string_with_kind(mem, &value, A64_KIND_STRING)?;
            super::return_value(context, object);
        }
        StubKind::StringLength => {
            let length = objc_text(mem, context.regs[0])
                .map(|bytes| String::from_utf8_lossy(&bytes).encode_utf16().count() as u64)
                .unwrap_or(0);
            super::return_value(context, length);
        }
        StubKind::StringCharacter => {
            let value = objc_text(mem, context.regs[0])
                .and_then(|bytes| {
                    String::from_utf8_lossy(&bytes)
                        .encode_utf16()
                        .nth(context.regs[1] as usize)
                })
                .unwrap_or(0);
            super::return_value(context, u64::from(value));
        }
        StubKind::StringGetCString => {
            let bytes = objc_text(mem, context.regs[0]).unwrap_or_default();
            let buffer_size = context.regs[2] as usize;
            if context.regs[1] == 0 || buffer_size == 0 {
                super::return_value(context, 0);
            } else {
                let capacity = buffer_size.saturating_sub(1);
                let copied = bytes.len().min(capacity);
                mem.write_bytes(context.regs[1], &bytes[..copied])
                    .map_err(str::to_owned)?;
                mem.write_u8(context.regs[1] + copied as u64, 0)
                    .map_err(str::to_owned)?;
                super::return_value(context, u64::from(copied == bytes.len()));
            }
        }
        StubKind::StringCStringPointer => {
            super::return_value(context, objc_field(mem, context.regs[0], 56));
        }
        StubKind::StringMaximumSize => {
            let length = objc_field(mem, context.regs[0], 56);
            super::return_value(
                context,
                context.regs[0]
                    .saturating_mul(4)
                    .saturating_add(1)
                    .max(length),
            );
        }
        StubKind::StringAppend | StubKind::StringAppendCString => {
            let right = if matches!(kind, StubKind::StringAppendCString) {
                c_string(mem, context.regs[1]).unwrap_or_default()
            } else {
                objc_text(mem, context.regs[1]).unwrap_or_default()
            };
            let mut bytes = objc_text(mem, context.regs[0]).unwrap_or_default();
            bytes.extend(right);
            replace_string(mem, context.regs[0], &bytes)?;
            super::return_value(context, 0);
        }
        StubKind::DataCreate(mutable) => {
            let bytes = if mutable {
                Vec::new()
            } else if context.regs[1] == 0 {
                Vec::new()
            } else {
                mem.read_bytes(context.regs[1], context.regs[2].min(64 * 1024 * 1024))
                    .map_err(str::to_owned)?
            };
            let object = super::objc_object(mem, A64_KIND_DATA)?;
            if !bytes.is_empty() || !mutable {
                replace_data(mem, object, &bytes)?;
            } else {
                set_objc_field(mem, object, 56, 0);
                set_objc_field(mem, object, 64, 0);
            }
            super::return_value(context, object);
        }
        StubKind::DataCreateCopy => {
            let source_pointer = objc_field(mem, context.regs[1], 56);
            let source_length = objc_field(mem, context.regs[1], 64).min(64 * 1024 * 1024);
            let bytes = if source_pointer == 0 || source_length == 0 {
                Vec::new()
            } else {
                mem.read_bytes(source_pointer, source_length)
                    .map_err(str::to_owned)?
            };
            let object = super::objc_object(mem, A64_KIND_DATA)?;
            replace_data(mem, object, &bytes)?;
            super::return_value(context, object);
        }
        StubKind::DataGetBytes => {
            let pointer = objc_field(mem, context.regs[0], 56);
            let length = objc_field(mem, context.regs[0], 64).min(64 * 1024 * 1024) as usize;
            let (location, count) = range_values(mem, context.regs[1], length);
            if context.regs[2] != 0 && count > 0 && pointer != 0 {
                let bytes = mem
                    .read_bytes(pointer + location as u64, count as u64)
                    .map_err(str::to_owned)?;
                mem.write_bytes(context.regs[2], &bytes)
                    .map_err(str::to_owned)?;
            }
            super::return_value(context, 0);
        }
        StubKind::DataBytePointer => {
            super::return_value(context, objc_field(mem, context.regs[0], 56));
        }
        StubKind::DataLength => super::return_value(context, objc_field(mem, context.regs[0], 64)),
        StubKind::DataAppend => {
            let old_pointer = objc_field(mem, context.regs[0], 56);
            let old_length = objc_field(mem, context.regs[0], 64);
            let append_length = context.regs[2].min(64 * 1024 * 1024);
            let mut bytes = if old_pointer == 0 {
                Vec::new()
            } else {
                mem.read_bytes(old_pointer, old_length)
                    .map_err(str::to_owned)?
            };
            if context.regs[1] != 0 && append_length > 0 {
                bytes.extend(
                    mem.read_bytes(context.regs[1], append_length)
                        .map_err(str::to_owned)?,
                );
            }
            replace_data(mem, context.regs[0], &bytes)?;
            super::return_value(context, 0);
        }
        StubKind::NumberCreate => {
            let number_type = context.regs[1];
            let object = if matches!(number_type, 5 | 6 | 12 | 13 | 16) {
                objc_number_float(mem, read_number_as_f64(mem, context.regs[2], number_type))?
            } else {
                objc_number(mem, read_number_as_i64(mem, context.regs[2], number_type))?
            };
            super::return_value(context, object);
        }
        StubKind::NumberValue => {
            if symbol == "CFGetRetainCount" {
                super::return_value(context, 1);
            } else if context.regs[2] != 0 {
                let value = objc_field(mem, context.regs[0], 56);
                let is_float = objc_field(mem, context.regs[0], 64) != 0;
                let number = if is_float {
                    f64::from_bits(value)
                } else {
                    value as i64 as f64
                };
                write_number_value(mem, context.regs[2], context.regs[1], number)?;
                super::return_value(context, 1);
            } else {
                super::return_value(context, 0);
            }
        }
        StubKind::Retain => super::return_value(context, context.regs[0]),
        StubKind::Release => super::return_value(context, 0),
        StubKind::Allocator => {
            let object = objc_object(mem, A64_KIND_GENERIC)?;
            super::return_value(context, object);
        }
        StubKind::Null => {
            let object = objc_object(mem, A64_KIND_GENERIC)?;
            super::return_value(context, object);
        }
        StubKind::GenericPointer => generic_pointer(mem, context)?,
        StubKind::GenericReceiver => super::return_value(context, context.regs[0]),
        StubKind::GenericZero => super::return_value(context, 0),
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corefoundation_collection_stubs_preserve_guest_values() {
        let mut memory = Mem64::new();
        let first = objc_object(&mut memory, A64_KIND_GENERIC).unwrap();
        let second = objc_object(&mut memory, A64_KIND_GENERIC).unwrap();
        let values = memory.alloc_zeroed(16).unwrap();
        memory.write_u64(values, first).unwrap();
        memory.write_u64(values + 8, second).unwrap();
        let mut context = touchHLE_DynarmicA64Context::default();
        context.regs[1] = values;
        context.regs[2] = 2;
        dispatch(&mut memory, &mut context, "CFArrayCreate").unwrap();
        let array = context.regs[0];
        context.regs[0] = array;
        dispatch(&mut memory, &mut context, "CFArrayGetCount").unwrap();
        assert_eq!(context.regs[0], 2);
        context.regs[0] = array;
        context.regs[1] = 1;
        dispatch(&mut memory, &mut context, "CFArrayGetValueAtIndex").unwrap();
        assert_eq!(context.regs[0], second);
    }

    #[test]
    fn corefoundation_string_and_data_stubs_preserve_guest_values() {
        let mut memory = Mem64::new();
        let text = memory.alloc_zeroed(6).unwrap();
        memory.write_bytes(text, b"hello").unwrap();
        let mut context = touchHLE_DynarmicA64Context::default();
        context.regs[1] = text;
        dispatch(&mut memory, &mut context, "CFStringCreateWithCString").unwrap();
        let string = context.regs[0];
        assert_eq!(objc_text(&memory, string).as_deref(), Some(&b"hello"[..]));
        context.regs[0] = string;
        dispatch(&mut memory, &mut context, "CFStringGetLength").unwrap();
        assert_eq!(context.regs[0], 5);

        let bytes = memory.alloc_zeroed(3).unwrap();
        memory.write_bytes(bytes, &[1, 2, 3]).unwrap();
        context.regs[1] = bytes;
        context.regs[2] = 3;
        dispatch(&mut memory, &mut context, "CFDataCreate").unwrap();
        let data = context.regs[0];
        context.regs[0] = data;
        dispatch(&mut memory, &mut context, "CFDataGetLength").unwrap();
        assert_eq!(context.regs[0], 3);
    }
}
