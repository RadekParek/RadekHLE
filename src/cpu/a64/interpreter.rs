use crate::mem64::Mem64;
use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

const NZCV_N: u32 = 1 << 31;
const NZCV_Z: u32 = 1 << 30;
const NZCV_C: u32 = 1 << 29;
const NZCV_V: u32 = 1 << 28;

#[derive(Debug)]
pub struct A64Interpreter {
    reservation: Option<(u64, u128)>,
}

impl A64Interpreter {
    pub fn new() -> Self {
        Self { reservation: None }
    }

    pub fn run_or_step(&mut self, memory: &mut Mem64, context: &mut touchHLE_DynarmicA64Context, ticks: Option<&mut u64>) -> i32 {
        let run = ticks.is_some();
        let mut executed = 0;
        let result = if !run {
            self.step(memory, context)
        } else {
            let mut result = -1;
            for _ in 0..4096 {
                let pc = context.pc;
                let instruction = match memory.read_code_u32(pc) {
                    Ok(instruction) => instruction,
                    Err(error) => {
                        log_once_fmt!("ARM64 EXCEPTION PC={pc:#x} SP={:#x} instruction=<unreadable> address={pc:#x} reason=execute fault: {error} [subsequent identical faults suppressed]", context.sp);
                        result = -2;
                        break;
                    }
                };
                result = self.step(memory, context);
                if result != -1 || is_control_flow(instruction, pc, context.pc) {
                    break;
                }
                executed += 1;
            }
            result
        };
        if let Some(ticks) = ticks {
            *ticks = ticks.saturating_sub(executed);
        }
        result
    }

    fn step(&mut self, memory: &mut Mem64, context: &mut touchHLE_DynarmicA64Context) -> i32 {
        let pc = context.pc;
        let instruction = match memory.read_code_u32(pc) {
            Ok(instruction) => instruction,
            Err(error) => {
                log_once_fmt!("ARM64 EXCEPTION PC={pc:#x} SP={:#x} instruction=<unreadable> address={pc:#x} reason=execute fault: {error} [subsequent identical faults suppressed]", context.sp);
                return -2;
            }
        };
        let sp_before = context.sp;
        let result = self.execute(memory, context, instruction);
        if context.sp != sp_before {
            log!(
                "ARM64 interpreter SP change: pc={pc:#x} instruction={instruction:#010x} before={sp_before:#x} after={:#x}",
                context.sp,
            );
        }
        match result {
            Ok(Some(svc)) => svc as i32,
            Ok(None) => -1,
            Err(InterpreterError::Memory(error, address)) => {
                log_once_fmt!("ARM64 EXCEPTION PC={pc:#x} SP={:#x} instruction={instruction:#010x} address={address:#x} reason=memory fault: {error} [subsequent identical faults suppressed]", context.sp);
                -2
            }
            Err(InterpreterError::Undefined) => {
                log_once_fmt!("ARM64 EXCEPTION PC={pc:#x} SP={:#x} instruction={instruction:#010x} address=<none> reason=unimplemented ARM64 instruction [subsequent identical faults suppressed]", context.sp);
                -3
            }
            Err(InterpreterError::Breakpoint) => {
                log_once_fmt!("ARM64 EXCEPTION PC={pc:#x} SP={:#x} instruction={instruction:#010x} address=<none> reason=breakpoint [subsequent identical faults suppressed]", context.sp);
                -4
            }
        }
    }

    fn execute(&mut self, memory: &mut Mem64, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<Option<u32>, InterpreterError> {
        let pc = context.pc;
        if instruction & 0xffe0_001f == 0xd400_0001 {
            context.pc = pc.wrapping_add(4);
            return Ok(Some((instruction >> 5) & 0xffff));
        }
        if instruction & 0xffff_ffe0 == 0xd420_0000 {
            return Err(InterpreterError::Breakpoint);
        }
        if instruction & 0xffff_f01f == 0xd503_201f {
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x7c00_0000 == 0x1400_0000 {
            let offset = sign_extend((instruction & 0x03ff_ffff) as u64, 26) << 2;
            if instruction & 0x8000_0000 != 0 {
                set_x(context, 30, pc.wrapping_add(4));
            }
            context.pc = pc.wrapping_add_signed(offset);
            return Ok(None);
        }
        if instruction & 0xff00_0010 == 0x5400_0000 {
            let offset = sign_extend(((instruction >> 5) & 0x7ffff) as u64, 19) << 2;
            if condition_holds(context, instruction & 0xf) {
                context.pc = pc.wrapping_add_signed(offset);
            } else {
                context.pc = pc.wrapping_add(4);
            }
            return Ok(None);
        }
        if instruction & 0x7e00_0000 == 0x3400_0000 {
            let sf = instruction >> 31 != 0;
            let nonzero = instruction & 0x0100_0000 != 0;
            let value = read_reg(context, (instruction & 31) as usize, sf);
            let offset = sign_extend(((instruction >> 5) & 0x7ffff) as u64, 19) << 2;
            let taken = (value == 0) != nonzero;
            context.pc = if taken { pc.wrapping_add_signed(offset) } else { pc.wrapping_add(4) };
            return Ok(None);
        }
        if instruction & 0x7e00_0000 == 0x3600_0000 {
            let bit = (((instruction >> 31) & 1) * 32 + ((instruction >> 19) & 31)) as u32;
            let value = read_reg(context, (instruction & 31) as usize, true);
            let nonzero = instruction & 0x0100_0000 != 0;
            let offset = sign_extend(((instruction >> 5) & 0x3fff) as u64, 14) << 2;
            let taken = (((value >> bit) & 1) != 0) == nonzero;
            context.pc = if taken { pc.wrapping_add_signed(offset) } else { pc.wrapping_add(4) };
            return Ok(None);
        }
        if instruction & 0xffff_fc1f == 0xd61f_0000 {
            let target = read_reg(context, ((instruction >> 5) & 31) as usize, true);
            context.pc = target;
            return Ok(None);
        }
        if instruction & 0xffff_fc1f == 0xd63f_0000 {
            let target = read_reg(context, ((instruction >> 5) & 31) as usize, true);
            set_x(context, 30, pc.wrapping_add(4));
            context.pc = target;
            return Ok(None);
        }
        if instruction & 0xffff_fc1f == 0xd65f_0000 {
            context.pc = read_reg(context, ((instruction >> 5) & 31) as usize, true);
            return Ok(None);
        }
        if instruction & 0x9f00_0000 == 0x1000_0000 {
            let imm = (((instruction >> 5) & 0x7ffff) << 2) | ((instruction >> 29) & 3);
            let value = pc.wrapping_add_signed(sign_extend(imm as u64, 21));
            set_x(context, (instruction & 31) as usize, value);
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x9f00_0000 == 0x9000_0000 {
            let imm = (((instruction >> 5) & 0x7ffff) << 2) | ((instruction >> 29) & 3);
            let base = pc & !0xfff;
            let value = base.wrapping_add_signed(sign_extend(imm as u64, 21) << 12);
            set_x(context, (instruction & 31) as usize, value);
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1f80_0000 == 0x1200_0000 {
            self.execute_logical_immediate(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1f80_0000 == 0x1280_0000 {
            self.execute_move_wide(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1f00_0000 == 0x1100_0000 {
            self.execute_add_sub_immediate(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1f20_0000 == 0x0b00_0000 {
            self.execute_add_sub_register(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1f00_0000 == 0x0a00_0000 {
            self.execute_logical(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1fe0_0c00 == 0x1a80_0000 {
            self.execute_conditional_select(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1f00_0000 == 0x1b00_0000 {
            self.execute_multiply(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1fe0_fc00 == 0x1ac0_0800 {
            self.execute_divide(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x7fff_fc00 == 0x5ac0_1000 {
            let sf = instruction >> 31 != 0;
            let value = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
            set_reg(context, (instruction & 31) as usize, value.leading_zeros() as u64, sf);
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x3b00_0000 == 0x1800_0000 {
            self.execute_literal_load(memory, context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x3e00_0000 == 0x0800_0000 {
            self.execute_exclusive(memory, context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x3b20_0000 == 0x3800_0000 {
            self.execute_load_store_unscaled(memory, context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x3b20_0000 == 0x3820_0000 {
            self.execute_load_store_register(memory, context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x3b00_0000 == 0x3900_0000 {
            self.execute_load_store_unsigned(memory, context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x3e00_0000 == 0x2800_0000 {
            self.execute_pair(memory, context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        Err(InterpreterError::Undefined)
    }

    fn execute_logical_immediate(&self, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let width = if sf { 64 } else { 32 };
        let n = ((instruction >> 22) & 1) as u32;
        let immr = ((instruction >> 16) & 0x3f) as u32;
        let imms = ((instruction >> 10) & 0x3f) as u32;
        let combined = (n << 6) | ((!imms) & 0x3f);
        let len = 31 - combined.leading_zeros();
        if len < 1 || (!sf && len > 5) {
            return Err(InterpreterError::Undefined);
        }
        let element_size = 1u32 << len;
        let levels = element_size - 1;
        let s = imms & levels;
        let r = immr & levels;
        let ones = if s + 1 == 64 { u64::MAX } else { (1u64 << (s + 1)) - 1 };
        let element_mask = if element_size == 64 { u64::MAX } else { (1u64 << element_size) - 1 };
        let rotated = ((ones >> r) | (ones << (element_size - r))) & element_mask;
        let mut mask = 0u64;
        let mut shift = 0;
        while shift < width {
            mask |= rotated << shift;
            shift += element_size;
        }
        let left = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let value = match (instruction >> 29) & 3 {
            0 => left & mask,
            1 => left | mask,
            2 => left ^ mask,
            3 => {
                let result = left & mask;
                set_flags(context, result, false, false, sf);
                result
            }
            _ => unreachable!(),
        };
        set_reg(context, (instruction & 31) as usize, value, sf);
        Ok(())
    }

    fn execute_move_wide(&self, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let opc = (instruction >> 29) & 3;
        let shift = ((instruction >> 21) & 3) * 16;
        if !sf && shift >= 32 {
            return Err(InterpreterError::Undefined);
        }
        let width = if sf { 64 } else { 32 };
        let mask = if sf { u64::MAX } else { u32::MAX as u64 };
        let immediate = (((instruction >> 5) & 0xffff) as u64) << shift;
        let old = read_reg(context, (instruction & 31) as usize, sf);
        let value = match opc {
            0 => (!immediate) & mask,
            2 => immediate & mask,
            3 => (old & !(0xffffu64 << shift)) | immediate,
            _ => return Err(InterpreterError::Undefined),
        };
        set_reg(context, (instruction & 31) as usize, value, sf);
        let _ = width;
        Ok(())
    }

    fn execute_add_sub_immediate(&self, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let subtract = instruction & 0x4000_0000 != 0;
        let update_flags = instruction & 0x2000_0000 != 0;
        let shift = if instruction & 0x0040_0000 != 0 { 12 } else { 0 };
        let immediate = (((instruction >> 10) & 0xfff) as u64) << shift;
        let left = read_sp_or_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let (value, carry, overflow) = add_sub(left, immediate, subtract, sf);
        write_add_sub_result(context, (instruction & 31) as usize, value, sf, update_flags);
        if update_flags {
            set_flags(context, value, carry, overflow, sf);
        }
        Ok(())
    }

    fn execute_add_sub_register(&self, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let subtract = instruction & 0x4000_0000 != 0;
        let update_flags = instruction & 0x2000_0000 != 0;
        let rm = read_reg(context, ((instruction >> 16) & 31) as usize, sf);
        let shift_type = (instruction >> 22) & 3;
        let amount = ((instruction >> 10) & 0x3f) as u32;
        let right = shift_value(rm, shift_type, amount, sf);
        let left = read_sp_or_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let (value, carry, overflow) = add_sub(left, right, subtract, sf);
        write_add_sub_result(context, (instruction & 31) as usize, value, sf, update_flags);
        if update_flags {
            set_flags(context, value, carry, overflow, sf);
        }
        Ok(())
    }

    fn execute_logical(&self, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let opc = (instruction >> 29) & 3;
        let invert = instruction & 0x0020_0000 != 0;
        let left = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let mut right = read_reg(context, ((instruction >> 16) & 31) as usize, sf);
        right = shift_value(right, (instruction >> 22) & 3, ((instruction >> 10) & 0x3f) as u32, sf);
        if invert { right = !right; }
        let value = match opc {
            0 => left & right,
            1 => left | right,
            2 => left ^ right,
            3 => left & right,
            _ => unreachable!(),
        };
        set_reg(context, (instruction & 31) as usize, value, sf);
        if opc == 3 {
            set_flags_logic(context, value, sf);
        }
        Ok(())
    }

    fn execute_conditional_select(&self, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let condition = instruction & 0xf;
        let first = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let second = read_reg(context, ((instruction >> 16) & 31) as usize, sf);
        let mut value = if condition_holds(context, condition) { first } else { second };
        if instruction & 0x0000_0400 != 0 { value = value.wrapping_add(1); }
        if instruction & 0x0000_0800 != 0 { value = !value; }
        if instruction & 0x0000_1000 != 0 { value = value.wrapping_neg(); }
        set_reg(context, (instruction & 31) as usize, value, sf);
        Ok(())
    }

    fn execute_multiply(&self, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let left = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let right = read_reg(context, ((instruction >> 16) & 31) as usize, sf);
        let addend = read_reg(context, ((instruction >> 10) & 31) as usize, sf);
        let product = left.wrapping_mul(right);
        let value = if instruction & 0x0000_8000 != 0 { product.wrapping_sub(addend) } else { product.wrapping_add(addend) };
        set_reg(context, (instruction & 31) as usize, value, sf);
        Ok(())
    }

    fn execute_divide(&self, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let left = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let right = read_reg(context, ((instruction >> 16) & 31) as usize, sf);
        let signed = instruction & 0x400 != 0;
        let value = if right == 0 {
            0
        } else if signed {
            if sf { (left as i64).wrapping_div(right as i64) as u64 } else { (left as i32).wrapping_div(right as i32) as u32 as u64 }
        } else {
            left / right
        };
        set_reg(context, (instruction & 31) as usize, value, sf);
        Ok(())
    }

    fn execute_literal_load(&self, memory: &Mem64, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let size = if instruction & 0x4000_0000 != 0 { 8 } else { 4 };
        let address = context.pc.wrapping_add_signed(sign_extend(((instruction >> 5) & 0x7ffff) as u64, 19) << 2);
        let value = if size == 8 { memory.read_u64(address) } else { memory.read_u32(address).map(u64::from) }.map_err(|error| InterpreterError::Memory(error, address))?;
        set_reg(context, (instruction & 31) as usize, value, size == 8);
        Ok(())
    }

    fn execute_exclusive(&mut self, memory: &mut Mem64, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let size = 1u64 << ((instruction >> 30) & 3);
        let load = instruction & 0x0040_0000 != 0;
        let address = read_sp_or_reg(context, ((instruction >> 5) & 31) as usize, true);
        let rt = (instruction & 31) as usize;
        let rs = ((instruction >> 16) & 31) as usize;
        if load {
            let value = read_memory_value(memory, address, size)?;
            set_reg(context, rt, value as u64, size == 8);
            self.reservation = Some((address, value));
        } else {
            let success = self.reservation.take().is_some_and(|(reserved, expected)| {
                reserved == address && read_memory_value(memory, address, size).ok() == Some(expected)
            });
            if success {
                let value = read_reg(context, rs, size == 8);
                let result = match size {
                    1 => memory.write_u8(address, value as u8),
                    2 => memory.write_u16(address, value as u16),
                    4 => memory.write_u32(address, value as u32),
                    8 => memory.write_u64(address, value),
                    _ => unreachable!(),
                };
                result.map_err(|error| InterpreterError::Memory(error, address))?;
            }
            set_reg(context, rs, u64::from(!success), false);
        }
        Ok(())
    }

    fn execute_load_store_unsigned(&self, memory: &mut Mem64, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let size = 1u64 << ((instruction >> 30) & 3);
        let opcode = instruction & 0xffc0_0000;
        let signed_load = matches!(opcode, 0x3980_0000 | 0x7980_0000 | 0xb980_0000);
        let load = instruction & 0x0040_0000 != 0 || signed_load;
        let address = read_sp_or_reg(context, ((instruction >> 5) & 31) as usize, true).wrapping_add(((instruction >> 10) & 0xfff) as u64 * size);
        if signed_load {
            let value = match size {
                1 => memory.read_u8(address).map(|value| (value as i8 as i64) as u64),
                2 => memory.read_u16(address).map(|value| (value as i16 as i64) as u64),
                4 => memory.read_u32(address).map(|value| (value as i32 as i64) as u64),
                8 => memory.read_u64(address),
                _ => unreachable!(),
            }.map_err(|error| InterpreterError::Memory(error, address))?;
            set_reg(context, (instruction & 31) as usize, value, instruction & 0x8000_0000 != 0);
            Ok(())
        } else {
            self.load_store(memory, context, instruction, address, size, load)
        }
    }

    fn execute_load_store_register(&self, memory: &mut Mem64, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let size = 1u64 << ((instruction >> 30) & 3);
        let load = instruction & 0x0040_0000 != 0;
        let signed = instruction & 0x0000_0800 != 0;
        let base = read_sp_or_reg(context, ((instruction >> 5) & 31) as usize, true);
        let index = read_reg(context, ((instruction >> 16) & 31) as usize, true);
        let extend = (instruction >> 13) & 7;
        let mut offset = match extend {
            2 | 6 => (index as i32 as i64) as u64,
            3 | 7 => index,
            0 => index as u8 as u64,
            1 => index as u16 as u64,
            4 => index as u32 as u64,
            5 => index,
            _ => index,
        };
        if instruction & 0x0000_1000 != 0 {
            offset = offset.wrapping_shl(size.trailing_zeros());
        }
        let address = base.wrapping_add(offset);
        if load && signed {
            let value = match size {
                1 => memory.read_u8(address).map(|value| (value as i8 as i64) as u64),
                2 => memory.read_u16(address).map(|value| (value as i16 as i64) as u64),
                4 => memory.read_u32(address).map(|value| (value as i32 as i64) as u64),
                8 => memory.read_u64(address),
                _ => unreachable!(),
            }.map_err(|error| InterpreterError::Memory(error, address))?;
            set_reg(context, (instruction & 31) as usize, value, true);
            Ok(())
        } else {
            self.load_store(memory, context, instruction, address, size, load)
        }
    }

    fn execute_load_store_unscaled(&self, memory: &mut Mem64, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let size = 1u64 << ((instruction >> 30) & 3);
        let load = instruction & 0x0040_0000 != 0;
        let base = read_sp_or_reg(context, ((instruction >> 5) & 31) as usize, true);
        let offset = sign_extend(((instruction >> 12) & 0x1ff) as u64, 9);
        let mode = (instruction >> 10) & 3;
        let address = base.wrapping_add_signed(offset);
        self.load_store(memory, context, instruction, address, size, load)?;
        if mode == 1 { write_sp_or_reg(context, ((instruction >> 5) & 31) as usize, address, true); }
        if mode == 3 { write_sp_or_reg(context, ((instruction >> 5) & 31) as usize, address, true); }
        Ok(())
    }

    fn load_store(&self, memory: &mut Mem64, context: &mut touchHLE_DynarmicA64Context, instruction: u32, address: u64, size: u64, load: bool) -> Result<(), InterpreterError> {
        let rt = (instruction & 31) as usize;
        if load {
            let value = match size { 1 => memory.read_u8(address).map(u64::from), 2 => memory.read_u16(address).map(u64::from), 4 => memory.read_u32(address).map(u64::from), 8 => memory.read_u64(address), _ => unreachable!() }.map_err(|error| InterpreterError::Memory(error, address))?;
            set_reg(context, rt, value, size == 8);
        } else {
            let value = read_reg(context, rt, size == 8);
            let result = match size { 1 => memory.write_u8(address, value as u8), 2 => memory.write_u16(address, value as u16), 4 => memory.write_u32(address, value as u32), 8 => memory.write_u64(address, value), _ => unreachable!() };
            result.map_err(|error| InterpreterError::Memory(error, address))?;
        }
        Ok(())
    }

    fn execute_pair(&self, memory: &mut Mem64, context: &mut touchHLE_DynarmicA64Context, instruction: u32) -> Result<(), InterpreterError> {
        let load = instruction & 0x0040_0000 != 0;
        let size = if instruction & 0x8000_0000 != 0 { 8u64 } else { 4 };
        let base_reg = ((instruction >> 5) & 31) as usize;
        let base = read_sp_or_reg(context, base_reg, true);
        let offset = sign_extend(((instruction >> 15) & 0x7f) as u64, 7) << size.trailing_zeros();
        let mode = (instruction >> 23) & 3;
        let address = match mode {
            0 => return Err(InterpreterError::Undefined),
            1 => base,
            2 => base.wrapping_add_signed(offset),
            3 => base.wrapping_add_signed(offset),
            _ => return Err(InterpreterError::Undefined),
        };
        if load {
            let first = if size == 8 { memory.read_u64(address) } else { memory.read_u32(address).map(u64::from) }.map_err(|error| InterpreterError::Memory(error, address))?;
            let second_address = address.checked_add(size).ok_or(InterpreterError::Memory("pair address overflows", address))?;
            let second = if size == 8 { memory.read_u64(second_address) } else { memory.read_u32(second_address).map(u64::from) }.map_err(|error| InterpreterError::Memory(error, second_address))?;
            set_reg(context, (instruction & 31) as usize, first, size == 8);
            set_reg(context, ((instruction >> 10) & 31) as usize, second, size == 8);
        } else {
            let second_address = address.checked_add(size).ok_or(InterpreterError::Memory("pair address overflows", address))?;
            if !memory.can_write(address, size).is_ok() || !memory.can_write(second_address, size).is_ok() {
                return Err(InterpreterError::Memory("pair store is not writable", address));
            }
            let first = read_reg(context, (instruction & 31) as usize, size == 8);
            let second = read_reg(context, ((instruction >> 10) & 31) as usize, size == 8);
            if size == 8 { memory.write_u64(address, first).map_err(|error| InterpreterError::Memory(error, address))?; memory.write_u64(second_address, second).map_err(|error| InterpreterError::Memory(error, second_address))?; }
            else { memory.write_u32(address, first as u32).map_err(|error| InterpreterError::Memory(error, address))?; memory.write_u32(second_address, second as u32).map_err(|error| InterpreterError::Memory(error, second_address))?; }
        }
        if mode == 1 { write_sp_or_reg(context, base_reg, base.wrapping_add_signed(offset), true); }
        if mode == 3 { write_sp_or_reg(context, base_reg, address, true); }
        Ok(())
    }
}

#[derive(Debug)]
enum InterpreterError {
    Memory(&'static str, u64),
    Undefined,
    Breakpoint,
}

fn read_memory_value(memory: &Mem64, address: u64, size: u64) -> Result<u128, InterpreterError> {
    match size {
        1 => memory.read_u8(address).map(u128::from),
        2 => memory.read_u16(address).map(u128::from),
        4 => memory.read_u32(address).map(u128::from),
        8 => memory.read_u64(address).map(u128::from),
        _ => unreachable!(),
    }
    .map_err(|error| InterpreterError::Memory(error, address))
}

fn sign_extend(value: u64, bits: u32) -> i64 {
    let shift = 64 - bits;
    ((value << shift) as i64) >> shift
}

fn read_reg(context: &touchHLE_DynarmicA64Context, index: usize, sf: bool) -> u64 {
    if index == 31 { return 0; }
    let value = context.regs[index];
    if sf { value } else { value as u32 as u64 }
}

fn set_x(context: &mut touchHLE_DynarmicA64Context, index: usize, value: u64) {
    if index != 31 { context.regs[index] = value; }
}

fn set_reg(context: &mut touchHLE_DynarmicA64Context, index: usize, value: u64, sf: bool) {
    set_x(context, index, if sf { value } else { value as u32 as u64 });
}

fn read_sp_or_reg(context: &touchHLE_DynarmicA64Context, index: usize, sf: bool) -> u64 {
    if index == 31 { if sf { context.sp } else { context.sp as u32 as u64 } } else { read_reg(context, index, sf) }
}

fn write_add_sub_result(
    context: &mut touchHLE_DynarmicA64Context,
    index: usize,
    value: u64,
    sf: bool,
    update_flags: bool,
) {
    if index == 31 {
        if !update_flags {
            context.sp = if sf { value } else { value as u32 as u64 };
        }
    } else {
        set_reg(context, index, value, sf);
    }
}

fn write_sp_or_reg(context: &mut touchHLE_DynarmicA64Context, index: usize, value: u64, sf: bool) {
    if index == 31 { context.sp = if sf { value } else { value as u32 as u64 }; } else { set_reg(context, index, value, sf); }
}

fn shift_value(value: u64, kind: u32, amount: u32, sf: bool) -> u64 {
    let width = if sf { 64 } else { 32 };
    let amount = amount % width;
    let value = if sf { value } else { value as u32 as u64 };
    match kind {
        0 => value << amount,
        1 => value >> amount,
        2 => if sf { (value as i64 >> amount) as u64 } else { ((value as u32 as i32) >> amount) as u32 as u64 },
        _ => value.rotate_right(amount),
    }
}

fn add_sub(left: u64, right: u64, subtract: bool, sf: bool) -> (u64, bool, bool) {
    let mask = if sf { u64::MAX } else { u32::MAX as u64 };
    let left = left & mask;
    let right = right & mask;
    let (value, carry) = if subtract { let (value, borrow) = left.overflowing_sub(right); (value & mask, !borrow) } else { let (value, carry) = left.overflowing_add(right); (value & mask, carry) };
    let sign = if sf { 63 } else { 31 };
    let overflow = if subtract { ((left ^ right) & (left ^ value) & (1u64 << sign)) != 0 } else { ((!(left ^ right)) & (left ^ value) & (1u64 << sign)) != 0 };
    (value, carry, overflow)
}

fn set_flags(context: &mut touchHLE_DynarmicA64Context, value: u64, carry: bool, overflow: bool, sf: bool) {
    let mask = if sf { u64::MAX } else { u32::MAX as u64 };
    let value = value & mask;
    context.pstate &= !(NZCV_N | NZCV_Z | NZCV_C | NZCV_V);
    if value & if sf { 1 << 63 } else { 1 << 31 } != 0 { context.pstate |= NZCV_N; }
    if value == 0 { context.pstate |= NZCV_Z; }
    if carry { context.pstate |= NZCV_C; }
    if overflow { context.pstate |= NZCV_V; }
}

fn set_flags_logic(context: &mut touchHLE_DynarmicA64Context, value: u64, sf: bool) {
    set_flags(context, value, false, false, sf);
    context.pstate |= NZCV_C;
}

fn condition_holds(context: &touchHLE_DynarmicA64Context, condition: u32) -> bool {
    let n = context.pstate & NZCV_N != 0;
    let z = context.pstate & NZCV_Z != 0;
    let c = context.pstate & NZCV_C != 0;
    let v = context.pstate & NZCV_V != 0;
    match condition & 0xf {
        0 => z,
        1 => !z,
        2 => c,
        3 => !c,
        4 => n,
        5 => !n,
        6 => v,
        7 => !v,
        8 => c && !z,
        9 => !c || z,
        10 => n == v,
        11 => n != v,
        12 => !z && n == v,
        13 => z || n != v,
        14 => true,
        _ => false,
    }
}

fn is_control_flow(instruction: u32, pc: u64, next_pc: u64) -> bool {
    next_pc != pc.wrapping_add(4)
        || instruction & 0x7c00_0000 == 0x1400_0000
        || instruction & 0xffff_fc1f == 0xd65f_0000
        || instruction & 0xffff_fc1f == 0xd61f_0000
        || instruction & 0xffff_fc1f == 0xd63f_0000
        || instruction & 0xff00_0010 == 0x5400_0000
        || instruction & 0x7e00_0000 == 0x3400_0000
        || instruction & 0x7e00_0000 == 0x3600_0000
}


#[cfg(test)]
mod tests {
    use super::A64Interpreter;
    use crate::mem64::{Mem64, Permissions};
    use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

    const CODE: u64 = 0x1_0000_0000;
    const STACK: u64 = 0x7fff_ffff_0000;

    #[test]
    fn exact_startup_stp_updates_sp_and_stores_pair() {
        let mut memory = Mem64::new();
        memory.map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute()).unwrap();
        const SP: u64 = 0x7fff_fffe_ff10;
        memory.map_zeroed_with_permissions(SP - 0x2000, 0x4000, Permissions::read_write()).unwrap();
        memory.load_bytes(CODE, &0xa9bd57f6u32.to_le_bytes()).unwrap();
        let mut context = touchHLE_DynarmicA64Context::default();
        context.pc = CODE;
        context.sp = SP;
        context.regs[21] = 0x2121_2121_2121_2121;
        context.regs[22] = 0x2222_2222_2222_2222;
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.pc, CODE + 4);
        assert_eq!(context.sp, SP - 0x30);
        assert_eq!(memory.read_u64(context.sp).unwrap(), context.regs[22]);
        assert_eq!(memory.read_u64(context.sp + 8).unwrap(), context.regs[21]);
    }

    #[test]
    fn signed_offset_ldp_uses_offset_without_updating_sp() {
        let mut memory = Mem64::new();
        memory.map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute()).unwrap();
        const SP: u64 = 0x7fff_fffe_ff00;
        memory.map_zeroed_with_permissions(SP, 0x1000, Permissions::read_write()).unwrap();
        memory.load_bytes(CODE, &0xa9417bfdu32.to_le_bytes()).unwrap();
        memory.write_u64(SP + 0x10, 0x2929_2929_2929_2929).unwrap();
        memory.write_u64(SP + 0x18, 0x3030_3030_3030_3030).unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context { pc: CODE, sp: SP, ..Default::default() };
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.regs[29], 0x2929_2929_2929_2929);
        assert_eq!(context.regs[30], 0x3030_3030_3030_3030);
        assert_eq!(context.sp, SP);
    }

    #[test]
    fn post_indexed_ldp_uses_signed_immediate_for_sp_update() {
        let mut memory = Mem64::new();
        memory.map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute()).unwrap();
        const SP: u64 = 0x7fff_fffe_ff00;
        memory.map_zeroed_with_permissions(SP, 0x1000, Permissions::read_write()).unwrap();
        memory.load_bytes(CODE, &0xa8c27bfdu32.to_le_bytes()).unwrap();
        memory.write_u64(SP, 0x2929_2929_2929_2929).unwrap();
        memory.write_u64(SP + 8, 0x3030_3030_3030_3030).unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context { pc: CODE, sp: SP, ..Default::default() };
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.regs[29], 0x2929_2929_2929_2929);
        assert_eq!(context.regs[30], 0x3030_3030_3030_3030);
        assert_eq!(context.sp, SP + 0x20);
    }

    #[test]
    fn cmp_using_xzr_does_not_modify_sp() {
        let mut memory = Mem64::new();
        memory.map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute()).unwrap();
        const SP: u64 = 0x7fff_fffe_fe30;
        memory.map_zeroed_with_permissions(SP - 0x1000, 0x2000, Permissions::read_write()).unwrap();
        memory.load_bytes(CODE, &0xeb08029fu32.to_le_bytes()).unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context { pc: CODE, sp: SP, ..Default::default() };
        context.regs[20] = 0x20;
        context.regs[8] = 0x10;
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.sp, SP);
        assert!(context.pstate & (1 << 29) != 0);
    }

    #[test]
    fn interpreter_executes_basic_stack_sequence() {
        let mut memory = Mem64::new();
        memory.map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute()).unwrap();
        memory.map_zeroed_with_permissions(STACK - 0x2000, 0x4000, Permissions::read_write()).unwrap();
        let instructions = [0xd2800540u32, 0x91000400, 0xa9bf07e0, 0xa8c107e0];
        for (index, instruction) in instructions.iter().enumerate() {
            memory.load_bytes(CODE + index as u64 * 4, &instruction.to_le_bytes()).unwrap();
        }
        let mut context = touchHLE_DynarmicA64Context::default();
        context.pc = CODE;
        context.sp = STACK;
        let mut interpreter = A64Interpreter::new();
        for _ in instructions {
            assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        }
        assert_eq!(context.regs[0], 43);
        assert_eq!(context.sp, STACK);
    }

    #[test]
    fn rejects_execute_and_write_access_to_read_only_code() {
        let mut memory = Mem64::new();
        memory.map_zeroed_with_permissions(0x1000, 0x1000, Permissions::READ).unwrap();
        assert_eq!(A64Interpreter::new().run_or_step(&mut memory, &mut touchHLE_DynarmicA64Context { pc: 0x1000, ..Default::default() }, None), -2);
        assert!(memory.write_u32(0x1000, 0).is_err());
    }
}
