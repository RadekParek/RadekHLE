/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! `mach/mach_error.h`: `mach_error_string`.
//!
//! Реализация повторяет `libsyscall/mach/mach_error_string.c` из XNU:
//! 1. `do_compat()` — перекодировка устаревших (до Mach 3) номеров ошибок;
//! 2. разложение по полям `| system(6) | subsystem(12) | code(14) |`
//!    из `<mach/error.h>`;
//! 3. таблицы строк, перенесённые точно из файлов Apple
//!    `libsyscall/mach/err_kern.sub`, `err_ipc.sub`, `err_mach_ipc.sub`
//!    (включая оригинальные формулировки и опечатки вроде «availaible»).
//!
//! Оговорка: таблицы подсистем user space (1), server (2), vm (7),
//! libkern (0x37) и iokit (0x38) не перенесены — для них возвращается
//! NO_SUCH_ERROR, как для кода вне диапазона. «Пустые» системы
//! (5, 6, 0x08–0x36, 0x39–0x3f) в настоящей таблице имеют
//! `bad_sub == NULL`, и функция возвращает нулевой указатель — так и
//! здесь.

use crate::dyld::{export_c_func, FunctionExports};
use crate::mem::{ConstPtr, Ptr};
use crate::Environment;
use std::collections::HashMap;

/// Кэш гостевых строк: код ошибки → стабильный указатель (аналогично
/// `errno::strerror`): строка один раз переносится в гостевую память и
/// переиспользуется.
#[derive(Default)]
pub struct State {
    strings_cache: HashMap<i32, ConstPtr<u8>>,
}

/// `NO_SUCH_ERROR` из `libsyscall/mach/errorlib.h` (Apple).
const NO_SUCH_ERROR: &str = "unknown error code";

/// `bad_sub` для system 0 из `error_codes.c` (Apple).
const BAD_SUB_OS: &str = "(operating system/?) unknown subsystem error";
/// `bad_sub` для систем 3 и 4 из `error_codes.c` (Apple).
const BAD_SUB_IPC: &str = "(ipc/?) unknown subsystem error";

// MOD-константы из `libsyscall/mach/errorlib.h` — используются в do_compat.
const IPC_SEND_MOD: i32 = (3 << 26) | (0 << 14);
const IPC_RCV_MOD: i32 = (3 << 26) | (1 << 14);
const MACH_IPC_MIG_MOD: i32 = (4 << 26) | (2 << 14);
const SERV_NETNAME_MOD: i32 = (2 << 26) | (0 << 14);
const SERV_ENV_MOD: i32 = (2 << 26) | (1 << 14);
const SERV_EXECD_MOD: i32 = (2 << 26) | (2 << 14);

/// `err_codes_kern` из `err_kern.sub` (system 0, subsystem 0).
static ERR_CODES_KERN: &[&str] = &[
    "(os/kern) successful",
    "(os/kern) invalid address",
    "(os/kern) protection failure",
    "(os/kern) no space available",
    "(os/kern) invalid argument",
    "(os/kern) failure",
    "(os/kern) resource shortage",
    "(os/kern) not receiver",
    "(os/kern) no access",
    "(os/kern) memory failure",
    "(os/kern) memory error",
    "(os/kern) already in set",
    "(os/kern) not in set",
    "(os/kern) name exists",
    "(os/kern) aborted",
    "(os/kern) invalid name",
    "(os/kern) invalid task",
    "(os/kern) invalid right",
    "(os/kern) invalid value",
    "(os/kern) urefs overflow",
    "(os/kern) invalid capability",
    "(os/kern) right exists",
    "(os/kern) invalid host",
    "(os/kern) memory present",
    "(os/kern) memory data moved",
    "(os/kern) memory restart copy",
    "(os/kern) invalid processor set",
    "(os/kern) policy limit",
    "(os/kern) invalid policy",
    "(os/kern) invalid object",
    "(os/kern) already waiting",
    "(os/kern) default set",
    "(os/kern) exception protected",
    "(os/kern) invalid ledger",
    "(os/kern) invalid memory control",
    "(os/kern) invalid security",
    "(os/kern) not depressed",
    "(os/kern) object terminated",
    "(os/kern) lock set destroyed",
    "(os/kern) lock unstable",
    "(os/kern) lock owned by another",
    "(os/kern) lock owned by self",
    "(os/kern) semaphore destroyed",
    "(os/kern) RPC terminated",
    "(os/kern) terminate orphan",
    "(os/kern) let orphan continue",
    "(os/kern) service not supported",
    "(os/kern) remote node down",
    "(os/kern) thread not waiting",
    "(os/kern) operation timed out",
    "(os/kern) code signing error",
    "(os/kern) policy is static",
    "(os/kern) insufficient input buffer size",
    "(os/kern) denied by security policy",
    "(os/kern) missing kernel collection",
    "(os/kern) invalid kernel collection",
    "(os/kern) result not found",
];

/// `err_codes_unix` из `err_kern.sub` (system 0, subsystem 3;
/// индекс соответствует номеру errno).
static ERR_CODES_UNIX: &[&str] = &[
    NO_SUCH_ERROR,
    "(os/unix) no rights to object",
    "(os/unix) file or directory does not exist",
    "(os/unix) no such process",
    "(os/unix) interrupted system call",
    "(os/unix) i/o error",
    "(os/unix) device does not exist",
    "(os/unix) argument list is too long",
    "(os/unix) invalid executable object format",
    "(os/unix) bad file descriptor number",
    "(os/unix) no child processes are present",
    "(os/unix) no more processes are available",
    "(os/unix) insufficient memory",
    "(os/unix) access denied",
    "(os/unix) memory access fault",
    "(os/unix) block device required for operation",
    "(os/unix) mount device busy",
    "(os/unix) file already exists",
    "(os/unix) cross device link",
    "(os/unix) device does not exist",
    "(os/unix) object is not a directory",
    "(os/unix) object is a directory",
    "(os/unix) invalid argument",
    "(os/unix) internal file table overflow",
    "(os/unix) maximum number of open files reached",
    "(os/unix) object is not a tty-like device",
    "(os/unix) executable object is in use",
    "(os/unix) file is too large",
    "(os/unix) no space is left on device",
    "(os/unix) illegal seek attempt",
    "(os/unix) read-only file system",
    "(os/unix) too many links",
    "(os/unix) broken pipe",
    "(os/unix) argument is too large",
    "(os/unix) result is out of range",
    "(os/unix) operation on device would block",
    "(os/unix) operation is now in progress",
    "(os/unix) operation is already in progress",
    "(os/unix) socket operation attempted on non-socket object",
    "(os/unix) destination address is required",
    "(os/unix) message is too long",
    "(os/unix) protocol type is incorrect for socket",
    "(os/unix) protocol type is not availaible",
    "(os/unix) protocol type is not supported",
    "(os/unix) socket type is not supported",
    "(os/unix) operation is not supported on sockets",
    "(os/unix) protocol family is not supported",
    "(os/unix) address family is not supported by protocol family",
    "(os/unix) address is already in use",
    "(os/unix) can't assign requested address",
    "(os/unix) network is down",
    "(os/unix) network is unreachable",
    "(os/unix) network dropped connection on reset",
    "(os/unix) software aborted connection",
    "(os/unix) connection reset by peer",
    "(os/unix) no buffer space is available",
    "(os/unix) socket is already connected",
    "(os/unix) socket is not connected",
    "(os/unix) can't send after socket shutdown",
    "(os/unix) too many references; can't splice",
    "(os/unix) connection timed out",
    "(os/unix) connection was refused",
    "(os/unix) too many levels of symbolic links",
    "(os/unix) file name exceeds system maximum limit",
    "(os/unix) host is down",
    "(os/unix) there is no route to host",
    "(os/unix) directory is not empty",
    "(os/unix) quota on number of processes exceeded",
    "(os/unix) quota on number of users exceeded",
    "(os/unix) quota on available disk space exceeded",
];

/// `err_ipc.sub` (system 3, subsystem 0 — send).
static ERR_CODES_IPC_SEND: &[&str] = &[
    "(ipc/send) unknown error",
    "(ipc/send) invalid memory",
    "(ipc/send) invalid port",
    "(ipc/send) timed out",
    "(ipc/send) unused error",
    "(ipc/send) will notify",
    "(ipc/send) notify in progress",
    "(ipc/send) kernel refused message",
    "(ipc/send) send interrupted",
    "(ipc/send) send message too large",
    "(ipc/send) send message too small",
    "(ipc/send) message size changed while being copied",
];

/// `err_ipc.sub` (system 3, subsystem 1 — receive).
static ERR_CODES_IPC_RCV: &[&str] = &[
    "(ipc/rcv) unknown error",
    "(ipc/rcv) invalid memory",
    "(ipc/rcv) invalid port",
    "(ipc/rcv) receive timed out",
    "(ipc/rcv) message too large",
    "(ipc/rcv) no space for message data",
    "(ipc/rcv) only sender remaining",
    "(ipc/rcv) receive interrupted",
    "(ipc/rcv) port receiver changed or port became enabled",
];

/// `err_ipc.sub` (system 3, subsystem 2 — MIG).
static ERR_CODES_IPC_MIG: &[&str] = &[
    "(ipc/mig) type check failure in message interface",
    "(ipc/mig) wrong return message ID",
    "(ipc/mig) server detected error",
    "(ipc/mig) bad message ID",
    "(ipc/mig) server found wrong arguments",
    "(ipc/mig) no reply should be sent",
    "(ipc/mig) server raised exception",
    "(ipc/mig) user specified array not large enough for return info",
];

/// `err_mach_ipc.sub` (system 4, subsystem 0 — send).
static ERR_CODES_MACH_SEND: &[&str] = &[
    "(ipc/send) no error",
    "(ipc/send) send in progress",
    "(ipc/send) invalid data",
    "(ipc/send) invalid destination port",
    "(ipc/send) timed out",
    "(ipc/send) invalid voucher",
    "(ipc/send) unused error",
    "(ipc/send) interrupted",
    "(ipc/send) msg too small",
    "(ipc/send) invalid reply port",
    "(ipc/send) invalid port right",
    "(ipc/send) invalid notify port",
    "(ipc/send) invalid memory",
    "(ipc/send) no msg buffer",
    "(ipc/send) msg too large",
    "(ipc/send) invalid msg-type",
    "(ipc/send) invalid msg-header",
    "(ipc/send) invalid msg-trailer",
    "(ipc/send) invalid context for reply",
    "(ipc/send) unused error",
    "(ipc/send) unused error",
    "(ipc/send) out-of-line buffer too large",
    "(ipc/send) destination does not accept OOL ports",
];

/// `err_mach_ipc.sub` (system 4, subsystem 1 — receive).
static ERR_CODES_MACH_RCV: &[&str] = &[
    "(ipc/rcv) no error",
    "(ipc/rcv) receive in progress",
    "(ipc/rcv) invalid name",
    "(ipc/rcv) timed out",
    "(ipc/rcv) msg too large",
    "(ipc/rcv) interrupted",
    "(ipc/rcv) port changed",
    "(ipc/rcv) invalid notify port",
    "(ipc/rcv) invalid data",
    "(ipc/rcv) port died",
    "(ipc/rcv) port in set",
    "(ipc/rcv) header error",
    "(ipc/rcv) body error",
    "(ipc/rcv) invalid scatter list entry",
    "(ipc/rcv) overwrite region too small",
    "(ipc/rcv) invalid msg-trailer",
    "(ipc/rcv) DIPC transport error",
];

/// `err_mach_ipc.sub` (system 4, subsystem 2 — MIG).
static ERR_CODES_MACH_MIG: &[&str] = &[
    "(ipc/mig) client type check failure",
    "(ipc/mig) wrong reply message ID",
    "(ipc/mig) server detected error",
    "(ipc/mig) bad request message ID",
    "(ipc/mig) server type check failure",
    "(ipc/mig) no reply should be sent",
    "(ipc/mig) server raised exception",
    "(ipc/mig) array not large enough",
    "(ipc/mig) server died",
    "(ipc/mig) unknown trailer format",
];

/// Результат поиска: строка либо NULL (у «пустых» систем `bad_sub == NULL`,
/// и настоящая функция возвращает нулевой указатель).
enum ErrorString {
    Str(&'static str),
    Null,
}

fn table(
    table: &'static [&'static str],
    code: usize,
) -> ErrorString {
    match table.get(code) {
        Some(&s) => ErrorString::Str(s),
        None => ErrorString::Str(NO_SUCH_ERROR),
    }
}

/// `do_compat()` из `mach_error_string.c`: устаревшие (до Mach 3) номера
/// ошибок перекодируются в современную разметку. Совпадает с Apple-версией
/// ( диапазоны -100…-400 и «серверные» 1000/1600/27600).
fn do_compat(err: i32) -> i32 {
    if err > -200 && err <= -100 {
        -(err + 100) | IPC_SEND_MOD
    } else if err > -300 && err <= -200 {
        -(err + 200) | IPC_RCV_MOD
    } else if err > -400 && err <= -300 {
        -(err + 300) | MACH_IPC_MIG_MOD
    } else if (1000..1100).contains(&err) {
        (err - 1000) | SERV_NETNAME_MOD
    } else if (1600..1700).contains(&err) {
        (err - 1600) | SERV_ENV_MOD
    } else if (27600..27700).contains(&err) {
        (err - 27600) | SERV_EXECD_MOD
    } else {
        err
    }
}

/// Разложить код по полям `<mach/error.h>`:
/// `| system(6) | subsystem(12) | code(14) |` и найти строку.
fn lookup(err: i32) -> ErrorString {
    let err = do_compat(err);
    let system = (err >> 26) & 0x3f;
    let sub = (err >> 14) & 0xfff;
    let code = (err & 0x3fff) as usize;

    match system {
        // err_kern: подсистемы (os/kern), (os/?), (os/?), (os/unix).
        0 => match sub {
            0 => table(ERR_CODES_KERN, code),
            // err_os_sub[1..2]: max_code = 0 → всегда NO_SUCH_ERROR.
            1 | 2 => ErrorString::Str(NO_SUCH_ERROR),
            3 => table(ERR_CODES_UNIX, code),
            _ => ErrorString::Str(BAD_SUB_OS),
        },
        // err_ipc (старый IPC: send/rcv/mig).
        3 => match sub {
            0 => table(ERR_CODES_IPC_SEND, code),
            1 => table(ERR_CODES_IPC_RCV, code),
            2 => table(ERR_CODES_IPC_MIG, code),
            _ => ErrorString::Str(BAD_SUB_IPC),
        },
        // err_mach_ipc (современный Mach IPC: send/rcv/mig).
        4 => match sub {
            0 => table(ERR_CODES_MACH_SEND, code),
            1 => table(ERR_CODES_MACH_RCV, code),
            2 => table(ERR_CODES_MACH_MIG, code),
            _ => ErrorString::Str(BAD_SUB_IPC),
        },
        // Таблицы этих систем не перенесены (см. док. модуля) — фолбэк,
        // идентичный поведению для кода вне диапазона.
        1 | 2 | 7 | 0x37 | 0x38 => ErrorString::Str(NO_SUCH_ERROR),
        // errorlib_system_null: bad_sub == NULL → возвращаем NULL.
        5 | 6 | 8..=0x36 | 0x39..=0x3f => ErrorString::Null,
        // system ∈ 0..=0x3f покрыт ветками выше; компилятору нравится _.
        _ => ErrorString::Str(NO_SUCH_ERROR),
    }
}

fn mach_error_string(env: &mut Environment, err: i32) -> ConstPtr<u8> {
    match lookup(err) {
        ErrorString::Null => Ptr::null(),
        ErrorString::Str(s) => {
            if let Some(&cached) = env.libc_state.mach_error.strings_cache.get(&err) {
                return cached;
            }
            let ptr = env.mem.alloc_and_write_cstr(s.as_bytes()).cast_const();
            env.libc_state.mach_error.strings_cache.insert(err, ptr);
            ptr
        }
    }
}

pub const FUNCTIONS: FunctionExports = &[export_c_func!(mach_error_string(_))];
