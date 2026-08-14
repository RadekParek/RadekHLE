/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
//! CPU emulation.
//!
//! Implemented using the C++ library dynarmic, which is a dynamic recompiler.
//!
//! iPhone OS apps used either ARMv6 or ARMv7-A, which are both 32-bit ISAs.
//! For the moment, only ARMv6 has been tested.

use crate::abi::GuestFunction;
use crate::mem::{ConstPtr, GuestUSize, Mem, MutPtr, Ptr, SafeRead, SafeWrite};
use crate::mem64::Mem64;
mod a64;

use self::a64::A64Interpreter;

use std::ffi::CStr;

// Import functions from C++
use touchHLE_dynarmic_wrapper::*;

type VAddr = u32;
pub type CpuContext = touchHLE_DynarmicContext;

#[no_mangle]
extern "C" fn touchHLE_cpu_a64_log(message: *const std::ffi::c_char) {
    if message.is_null() {
        return;
    }
    let message = unsafe { CStr::from_ptr(message) };
    let Ok(message) = message.to_str() else {
        return;
    };
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        echo!("ARM64 dynarmic: {}", message);
    }));
}

fn touchHLE_cpu_read_impl<T: SafeRead + Default>(
    mem: *mut touchHLE_Mem,
    addr: VAddr,
    error: *mut bool,
) -> T {
    // If a panic occurs (probably due to a null-pointer access), we can't let
    // it keep unwinding as it will hit non-Rust stack frames (dynarmic).
    // Instead we catch the unwind and then tell the C++ code a problem occurred
    // so it can immediately halt CPU execution and then panic itself, now
    // with only Rust stack frames to worry about and with CPU state information
    // available that's useful for debugging.
    //
    // TODO: Disable this in debug mode? This relies on dynarmic's
    // check_halt_on_memory_access option which surely has a significant
    // performance impact.
    //
    // I'm not sure if this actually is unwind-safe, but considering
    // the emulator will crash anyway, maybe this is okay.
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mem = unsafe { &mut *mem.cast::<Mem>() };
        let ptr: ConstPtr<T> = Ptr::from_bits(addr);
        mem.read(ptr)
    }));
    unsafe {
        error.write(res.is_err());
    }
    res.unwrap_or_default()
}

fn touchHLE_cpu_write_impl<T: SafeWrite>(mem: *mut touchHLE_Mem, addr: VAddr, value: T) -> bool {
    // See comments above about catch_unwind
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mem = unsafe { &mut *mem.cast::<Mem>() };
        let ptr: MutPtr<T> = Ptr::from_bits(addr);
        mem.write(ptr, value)
    }));
    res.is_err()
}

// Export functions for use by C++
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u8(mem: *mut touchHLE_Mem, addr: VAddr, error: *mut bool) -> u8 {
    touchHLE_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u16(mem: *mut touchHLE_Mem, addr: VAddr, error: *mut bool) -> u16 {
    touchHLE_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u32(mem: *mut touchHLE_Mem, addr: VAddr, error: *mut bool) -> u32 {
    touchHLE_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u64(mem: *mut touchHLE_Mem, addr: VAddr, error: *mut bool) -> u64 {
    touchHLE_cpu_read_impl(mem, addr, error)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u8(mem: *mut touchHLE_Mem, addr: VAddr, value: u8) -> bool {
    touchHLE_cpu_write_impl(mem, addr, value)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u16(mem: *mut touchHLE_Mem, addr: VAddr, value: u16) -> bool {
    touchHLE_cpu_write_impl(mem, addr, value)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u32(mem: *mut touchHLE_Mem, addr: VAddr, value: u32) -> bool {
    touchHLE_cpu_write_impl(mem, addr, value)
}
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u64(mem: *mut touchHLE_Mem, addr: VAddr, value: u64) -> bool {
    touchHLE_cpu_write_impl(mem, addr, value)
}

fn touchHLE_cpu_read_64_impl<T: SafeRead + Default + Copy>(
    mem: *mut touchHLE_Mem,
    addr: u64,
    error: *mut bool,
) -> T {
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mem = unsafe { &mut *mem.cast::<Mem64>() };
        mem.read(addr)
    }));
    match res {
        Ok(Ok(value)) => {
            unsafe { error.write(false) };
            value
        }
        Ok(Err(_)) | Err(_) => {
            unsafe { error.write(true) };
            T::default()
        }
    }
}

fn touchHLE_cpu_write_64_impl<T: SafeWrite>(
    mem: *mut touchHLE_Mem,
    addr: u64,
    value: T,
) -> bool {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mem = unsafe { &mut *mem.cast::<Mem64>() };
        mem.write(addr, value)
    })) {
        Ok(Ok(())) => false,
        Ok(Err(_)) | Err(_) => true,
    }
}

#[no_mangle]
extern "C" fn touchHLE_cpu_read_u8_64(mem: *mut touchHLE_Mem, addr: u64, error: *mut bool) -> u8 { touchHLE_cpu_read_64_impl(mem, addr, error) }
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u16_64(mem: *mut touchHLE_Mem, addr: u64, error: *mut bool) -> u16 { touchHLE_cpu_read_64_impl(mem, addr, error) }
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u32_64(mem: *mut touchHLE_Mem, addr: u64, error: *mut bool) -> u32 { touchHLE_cpu_read_64_impl(mem, addr, error) }
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u64_64(mem: *mut touchHLE_Mem, addr: u64, error: *mut bool) -> u64 { touchHLE_cpu_read_64_impl(mem, addr, error) }
#[no_mangle]
extern "C" fn touchHLE_cpu_read_u128_64(mem: *mut touchHLE_Mem, addr: u64, error: *mut bool) -> [u64; 2] { touchHLE_cpu_read_64_impl(mem, addr, error) }
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u8_64(mem: *mut touchHLE_Mem, addr: u64, value: u8) -> bool { touchHLE_cpu_write_64_impl(mem, addr, value) }
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u16_64(mem: *mut touchHLE_Mem, addr: u64, value: u16) -> bool { touchHLE_cpu_write_64_impl(mem, addr, value) }
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u32_64(mem: *mut touchHLE_Mem, addr: u64, value: u32) -> bool { touchHLE_cpu_write_64_impl(mem, addr, value) }
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u64_64(mem: *mut touchHLE_Mem, addr: u64, value: u64) -> bool { touchHLE_cpu_write_64_impl(mem, addr, value) }
#[no_mangle]
extern "C" fn touchHLE_cpu_write_u128_64(mem: *mut touchHLE_Mem, addr: u64, value: [u64; 2]) -> bool { touchHLE_cpu_write_64_impl(mem, addr, value) }

pub struct Cpu {
    dynarmic_wrapper: *mut touchHLE_DynarmicWrapper,
    /// Copy of the direct memory access pointer used to check it has not
    /// changed. If this is null, direct memory access is not in use.
    direct_memory_access_ptr: *const std::ffi::c_void,
}

impl Drop for Cpu {
    fn drop(&mut self) {
        unsafe { touchHLE_DynarmicWrapper_delete(self.dynarmic_wrapper) }
    }
}

/// Why CPU execution ended.
#[derive(Debug)]
pub enum CpuState {
    /// Execution halted due to using up all remaining ticks (normal execution)
    /// or after the single instruction was executed (step execution).
    Normal,
    /// SVC instruction encountered.
    Svc(u32),
    /// An error was encountered.
    Error(CpuError),
}

/// A reason that can cause CPU execution to be interrupted.
#[derive(Debug, Clone, PartialEq)]
pub enum CpuError {
    /// Memory error during execution (probably a null page access).
    MemoryError,
    /// Undefined instruction (perhaps from a GDB software breakpoint).
    UndefinedInstruction,
    /// Breakpoint (`bkpt` instruction).
    Breakpoint,
}

impl Cpu {
    /// The register number of the stack pointer.
    pub const SP: usize = 13;
    /// The register number of the link register.
    #[allow(unused)]
    pub const LR: usize = 14;
    /// The register number of the program counter.
    pub const PC: usize = 15;

    /// When this bit is set in CPSR, the CPU is in Thumb mode.
    pub const CPSR_THUMB: u32 = 0x00000020;

    /// When this bit is set in CPSR, the CPU is in user mode.
    pub const CPSR_USER_MODE: u32 = 0x00000010;

    /// Construct a new CPU instance. If a mutable reference to a [Mem] instance
    /// is provided, direct memory access is enabled, and the CPU instance
    /// becomes bound to that [Mem] instance (subsequent calls must use the same
    /// one).
    pub fn new(direct_memory_access: Option<&mut Mem>) -> Cpu {
        // Null page count is in pages rather than bytes. Mem ensures it is
        // page aligned.
        let null_page_count: usize = direct_memory_access
            .as_ref()
            .map_or(0, |mem| mem.null_segment_size() / 0x1000)
            .try_into()
            .unwrap();
        // Safety: the direct memory access pointer will be retained directly by
        // the dynarmic wrapper and indirectly by cached JIT code, so we must
        // ensure we only execute the CPU while holding a &mut on the Mem object
        // to which that pointer belongs.
        let direct_memory_access_ptr = direct_memory_access
            .map_or(std::ptr::null_mut(), |mem| unsafe {
                mem.direct_memory_access_ptr()
            });
        let dynarmic_wrapper =
            unsafe { touchHLE_DynarmicWrapper_new(direct_memory_access_ptr, null_page_count) };
        Cpu {
            dynarmic_wrapper,
            direct_memory_access_ptr,
        }
    }

    pub fn regs(&self) -> &[u32; 16] {
        unsafe {
            let ptr = touchHLE_DynarmicWrapper_regs_const(self.dynarmic_wrapper);
            &*(ptr as *const [u32; 16])
        }
    }
    pub fn regs_mut(&mut self) -> &mut [u32; 16] {
        unsafe {
            let ptr = touchHLE_DynarmicWrapper_regs_mut(self.dynarmic_wrapper);
            &mut *(ptr as *mut [u32; 16])
        }
    }

    /// Dump the registers of the current cpu to the log output.
    /// Silently ignores panics.
    pub fn dump_regs(&self) {
        let regs = self.regs();
        Self::echo_regs(regs);
    }

    pub fn echo_regs(regs: &[u32; 16]) {
        // Silently ignore panics so it's safe to use in contexts where we
        // can't panic.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for row in 0..4 {
                use std::fmt::Write;
                let mut line = String::new();
                for col in 0..4 {
                    let reg_idx = row * 4 + col;
                    match reg_idx {
                        Self::SP => write!(&mut line, "\t SP: "),
                        Self::LR => write!(&mut line, "\t LR: "),
                        Self::PC => write!(&mut line, "\t PC: "),
                        _ if reg_idx <= 9 => write!(&mut line, "\t R{reg_idx}: "),
                        _ => write!(&mut line, "\tR{reg_idx}: "),
                    }
                    .unwrap();
                    write!(&mut line, "{:#010x}", regs[reg_idx]).unwrap();
                }
                echo!("{}", line);
            }
        }));
    }

    pub fn cpsr(&self) -> u32 {
        unsafe { touchHLE_DynarmicWrapper_cpsr(self.dynarmic_wrapper) }
    }
    pub fn set_cpsr(&mut self, cpsr: u32) {
        unsafe { touchHLE_DynarmicWrapper_set_cpsr(self.dynarmic_wrapper, cpsr) }
    }

    /// Swap the current state of the CPU (registers etc) with the state stored
    /// in the context object.
    pub fn swap_context(&mut self, context: &mut CpuContext) {
        unsafe { touchHLE_DynarmicWrapper_swap_context(self.dynarmic_wrapper, context) }
    }

    /// Get PC with the Thumb bit appropriately set.
    pub fn pc_with_thumb_bit(&self) -> GuestFunction {
        let pc = self.regs()[Self::PC];
        let thumb = (self.cpsr() & Self::CPSR_THUMB) == Self::CPSR_THUMB;
        GuestFunction::from_addr_and_thumb_flag(pc, thumb)
    }

    /// Set PC and the Thumb flag for executing a guest function. Note that this
    /// does not touch LR.
    pub fn branch(&mut self, new_pc: GuestFunction) {
        self.regs_mut()[Self::PC] = new_pc.addr_without_thumb_bit();
        let cpsr_without_thumb = self.cpsr() & (!Self::CPSR_THUMB);
        self.set_cpsr(cpsr_without_thumb | ((new_pc.is_thumb() as u32) * Self::CPSR_THUMB))
    }

    /// Set the PC and Thumb flag (like [Self::branch]), but also set the LR,
    /// and return the original PC and LR.
    pub fn branch_with_link(
        &mut self,
        new_pc: GuestFunction,
        new_lr: GuestFunction,
    ) -> (GuestFunction, GuestFunction) {
        let old_pc = self.pc_with_thumb_bit();
        let old_lr = GuestFunction::from_addr_with_thumb_bit(self.regs()[Self::LR]);
        self.branch(new_pc);
        self.regs_mut()[Self::LR] = new_lr.addr_with_thumb_bit();
        (old_pc, old_lr)
    }

    /// Clear dynarmic's instruction cache for some range of addresses.
    /// This is of interest to the dynamic linker, which will sometimes rewrite
    /// code.
    pub fn invalidate_cache_range(&mut self, base: VAddr, size: GuestUSize) {
        unsafe {
            touchHLE_DynarmicWrapper_invalidate_cache_range(self.dynarmic_wrapper, base, size)
        }
    }

    /// Start CPU execution.
    ///
    /// If `ticks` is [Some], it is used as an abstract time limit. The value
    /// will be reduced proportionately with the amount of ticks expended.
    ///
    /// If `ticks` is [None], the CPU executes only a single instruction. This
    /// is also known as "stepping".
    ///
    /// This will return either because the CPU ran out of time, or because
    /// something else happened which requires attention from the host.
    #[must_use]
    pub fn run_or_step(&mut self, mem: &mut Mem, ticks: Option<&mut u64>) -> CpuState {
        // See ::new() for why this is done.
        if !self.direct_memory_access_ptr.is_null() {
            assert!(self.direct_memory_access_ptr == unsafe { mem.direct_memory_access_ptr() });
        }

        let res = unsafe {
            touchHLE_DynarmicWrapper_run_or_step(
                self.dynarmic_wrapper,
                mem as *mut Mem as *mut touchHLE_Mem,
                ticks,
            )
        };
        match res {
            -1 => CpuState::Normal,
            -2 => CpuState::Error(CpuError::MemoryError),
            -3 => CpuState::Error(CpuError::UndefinedInstruction),
            -4 => CpuState::Error(CpuError::Breakpoint),
            _ if res < -4 => panic!("Unexpected CPU execution result"),
            svc => CpuState::Svc(svc as u32),
        }
    }
}

pub struct A64Cpu {
    backend: A64Backend,
}

enum A64Backend {
    Jit {
        wrapper: *mut touchHLE_DynarmicWrapper,
        interpreter: A64Interpreter,
        disabled: bool,
    },
    Interpreter(A64Interpreter),
}

impl std::fmt::Debug for A64Cpu {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("A64Cpu").finish_non_exhaustive()
    }
}

impl Drop for A64Cpu {
    fn drop(&mut self) {
        if let A64Backend::Jit { wrapper, .. } = self.backend {
            unsafe { touchHLE_DynarmicA64Wrapper_delete(wrapper) }
        }
    }
}

impl A64Cpu {
    pub fn new() -> Self {
        Self::with_backend(crate::options::Arm64Backend::Auto)
    }

    pub fn with_backend(backend: crate::options::Arm64Backend) -> Self {
        let backend = match backend {
            crate::options::Arm64Backend::Interpreter => {
                echo!("ARM64 backend selected: interpreter (explicit diagnostic mode)");
                A64Backend::Interpreter(A64Interpreter::new())
            }
            crate::options::Arm64Backend::Auto => {
                let use_interpreter = cfg!(target_os = "android");
                if use_interpreter {
                    echo!("ARM64 backend selected: interpreter (Android compatibility default; use --arm64-backend=jit to opt in)");
                    A64Backend::Interpreter(A64Interpreter::new())
                } else {
                    echo!("ARM64 backend selected: Dynarmic JIT (desktop default; use --arm64-backend=interpreter for compatibility diagnostics)");
                    A64Backend::Jit {
                        wrapper: unsafe { touchHLE_DynarmicA64Wrapper_new() },
                        interpreter: A64Interpreter::new(),
                        disabled: false,
                    }
                }
            }
            crate::options::Arm64Backend::Jit => {
                echo!("ARM64 backend selected: Dynarmic JIT (single-instruction compatibility stepping with interpreter fallback)");
                A64Backend::Jit {
                    wrapper: unsafe { touchHLE_DynarmicA64Wrapper_new() },
                    interpreter: A64Interpreter::new(),
                    disabled: false,
                }
            }
        };
        Self { backend }
    }

    pub fn swap_context(&mut self, context: &mut touchHLE_DynarmicA64Context) {
        if let A64Backend::Jit { wrapper, .. } = self.backend {
            unsafe { touchHLE_DynarmicA64Wrapper_swap_context(wrapper, context) }
        }
    }

    pub fn load_context(&mut self, context: &touchHLE_DynarmicA64Context) {
        if let A64Backend::Jit { wrapper, .. } = self.backend {
            unsafe { touchHLE_DynarmicA64Wrapper_load_context(wrapper, context) }
        }
    }

    pub fn save_context(&mut self, context: &mut touchHLE_DynarmicA64Context) {
        match &mut self.backend {
            A64Backend::Jit { wrapper, disabled, .. } if !*disabled => {
                unsafe { touchHLE_DynarmicA64Wrapper_save_context(*wrapper, context) }
            }
            A64Backend::Jit { .. } | A64Backend::Interpreter(_) => {}
        }
    }

    pub fn run_or_step(&mut self, mem: &mut Mem64, context: &mut touchHLE_DynarmicA64Context, mut ticks: Option<&mut u64>) -> i32 {
        match &mut self.backend {
            A64Backend::Jit { wrapper, interpreter, disabled } => {
                if *disabled {
                    return interpreter.run_or_step(mem, context, ticks);
                }
                let result = unsafe { touchHLE_DynarmicA64Wrapper_run_or_step(*wrapper, mem as *mut _ as *mut _, ticks.as_deref_mut()) };
                if result == -3 || result == -6 {
                    unsafe { touchHLE_DynarmicA64Wrapper_save_context(*wrapper, context) };
                    *disabled = true;
                    echo!("ARM64 Dynarmic compatibility fallback: disabling JIT after result {result} at pc={:#x}; continuing with interpreter", context.pc);
                    return interpreter.run_or_step(mem, context, ticks);
                }
                result
            }
            A64Backend::Interpreter(interpreter) => interpreter.run_or_step(mem, context, ticks),
        }
    }

    pub fn clear_halt(&mut self, reason: u32) {
        if let A64Backend::Jit { wrapper, disabled, .. } = self.backend {
            if !disabled {
                unsafe { touchHLE_DynarmicA64Wrapper_clear_halt(wrapper, reason) }
            }
        }
    }

    pub fn set_trace(&mut self, enabled: bool) {
        if let A64Backend::Jit { wrapper, .. } = self.backend {
            unsafe { touchHLE_DynarmicA64Wrapper_set_trace(wrapper, enabled) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::A64Cpu;
    use crate::mem64::Mem64;
    use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

    #[test]
    fn a64_synthetic_cpu_executes_instructions_and_stack_writes() {
        const CODE: u64 = 0x1_0000_1000;
        const STACK: u64 = 0x2_0000_0000;
        let instructions = [
            0xd503201f,
            0xd2800540,
            0x91001401,
            0xd1000822,
            0xa9bf07e0,
            0xa8c113e3,
            0xd65f03c0,
        ];
        let mut memory = Mem64::new();
        memory.map_zeroed(CODE, 0x1000).unwrap();
        memory.map_zeroed(STACK, 0x1000).unwrap();
        for (index, instruction) in instructions.iter().enumerate() {
            memory.write_u32(CODE + index as u64 * 4, *instruction).unwrap();
        }
        let mut context = touchHLE_DynarmicA64Context::default();
        context.pc = CODE;
        context.sp = STACK + 0x800;
        context.regs[30] = CODE + 0x100;
        let original_sp = context.sp;
        let mut cpu = A64Cpu::new();
        cpu.load_context(&context);

        for (index, instruction) in instructions.iter().take(6).enumerate() {
            assert_eq!(cpu.run_or_step(&mut memory, &mut context, None), -1, "instruction {} {instruction:#010x}", index + 1);
            cpu.save_context(&mut context);
            assert_eq!(context.pc, CODE + (index as u64 + 1) * 4);
        }

        cpu.save_context(&mut context);
        assert_eq!(context.regs[0], 42);
        assert_eq!(context.regs[1], 47);
        assert_eq!(context.regs[2], 45);
        assert_eq!(context.regs[3], 42);
        assert_eq!(context.regs[4], 47);
        assert_eq!(context.sp, original_sp);
        assert_eq!(memory.read_u64(original_sp - 16).unwrap(), 42);
        assert_eq!(memory.read_u64(original_sp - 8).unwrap(), 47);

        assert_eq!(cpu.run_or_step(&mut memory, &mut context, None), -1);
        cpu.save_context(&mut context);
        assert_eq!(context.pc, CODE + 0x100);
    }
}
