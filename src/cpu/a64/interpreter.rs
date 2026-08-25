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

    pub fn run_or_step(
        &mut self,
        memory: &mut Mem64,
        context: &mut touchHLE_DynarmicA64Context,
        ticks: Option<&mut u64>,
    ) -> i32 {
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
        if context.sp != sp_before && (context.sp.abs_diff(sp_before) > 0x1000 || pc == 0x1000eb360)
        {
            log_dbg!(
                "ARM64 interpreter stack transition: pc={pc:#x} instruction={instruction:#010x} before={sp_before:#x} after={:#x}",
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

    fn execute(
        &mut self,
        memory: &mut Mem64,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<Option<u32>, InterpreterError> {
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
        if instruction & 0x1fe0_0000 == 0x1a40_0000 {
            self.execute_conditional_compare(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1fe0_0000 == 0x1a80_0000
            || instruction & 0x1fe0_0000 == 0x5a80_0000
            || instruction & 0x1fe0_0000 == 0x1ac0_0000
        {
            self.execute_conditional_select(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1f00_0000 == 0x1a00_0000 {
            self.execute_add_sub_carry(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0xffe0_fc00 == 0x9e60_0000 || instruction & 0xffe0_fc00 == 0x1e60_0000 {
            self.execute_scalar_integer_to_float(context, instruction)?;
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
            context.pc = if taken {
                pc.wrapping_add_signed(offset)
            } else {
                pc.wrapping_add(4)
            };
            return Ok(None);
        }
        if instruction & 0x7e00_0000 == 0x3600_0000 {
            let bit = (((instruction >> 31) & 1) * 32 + ((instruction >> 19) & 31)) as u32;
            let value = read_reg(context, (instruction & 31) as usize, true);
            let nonzero = instruction & 0x0100_0000 != 0;
            let offset = sign_extend(((instruction >> 5) & 0x3fff) as u64, 14) << 2;
            let taken = (((value >> bit) & 1) != 0) == nonzero;
            context.pc = if taken {
                pc.wrapping_add_signed(offset)
            } else {
                pc.wrapping_add(4)
            };
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
        if instruction & 0x1f80_0000 == 0x1300_0000 {
            self.execute_bitfield(context, instruction)?;
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
        if instruction & 0x1fa0_fc00 == 0x1e20_2000 {
            self.execute_scalar_compare(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if matches!(
            instruction & 0x1f3f_fc00,
            0x1e22_0000 | 0x1e23_0000 | 0x1e38_0000 | 0x1e39_0000
        ) {
            self.execute_fixed_point_scalar_convert(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1f20_fc00 == 0x0f20_0000 {
            self.execute_sshll(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x5f20_fc00 == 0x5e20_d800 {
            self.execute_fixed_point_scalar_convert(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1f00_0000 == 0x1f00_0000 {
            self.execute_scalar_fma(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1f00_0000 == 0x1e00_0000 {
            self.execute_scalar_floating(context, instruction)?;
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
        if instruction & 0x1fe0_fc00 == 0x1b40_7c00 {
            let left = read_reg(context, ((instruction >> 5) & 31) as usize, true) as i64 as i128;
            let right = read_reg(context, ((instruction >> 16) & 31) as usize, true) as i64 as i128;
            set_reg(
                context,
                (instruction & 31) as usize,
                ((left * right) >> 64) as u64,
                true,
            );
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x1fe0_fc00 == 0x1bc0_7c00 {
            let left = read_reg(context, ((instruction >> 5) & 31) as usize, true) as u128;
            let right = read_reg(context, ((instruction >> 16) & 31) as usize, true) as u128;
            set_reg(
                context,
                (instruction & 31) as usize,
                ((left * right) >> 64) as u64,
                true,
            );
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
            set_reg(
                context,
                (instruction & 31) as usize,
                value.leading_zeros() as u64,
                sf,
            );
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
        if instruction & 0x0f80_0000 == 0x0f00_0000 {
            self.execute_adv_simd_shift_long(context, instruction)?;
            context.pc = pc.wrapping_add(4);
            return Ok(None);
        }
        if instruction & 0x3f00_0000 == 0x3d00_0000 {
            self.execute_simd_load_store(memory, context, instruction)?;
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

    fn execute_add_sub_carry(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let sf = instruction & 0x8000_0000 != 0;
        let subtract = instruction & 0x4000_0000 != 0;
        let update_flags = instruction & 0x2000_0000 != 0;
        let left = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let right = read_reg(context, ((instruction >> 16) & 31) as usize, sf);
        let carry_in = context.pstate & NZCV_C != 0;
        let width_mask = if sf { u64::MAX } else { u32::MAX as u64 };
        let right_with_carry = if subtract {
            right.wrapping_add(u64::from(!carry_in))
        } else {
            right.wrapping_add(u64::from(carry_in))
        };
        let (value, carry, overflow) = add_sub(left, right_with_carry & width_mask, subtract, sf);
        set_reg(context, (instruction & 31) as usize, value, sf);
        if update_flags {
            set_flags(context, value, carry, overflow, sf);
        }
        Ok(())
    }

    fn execute_conditional_compare(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let sf = instruction & 0x8000_0000 != 0;
        let immediate = instruction & 0x0000_0800 != 0;
        let compare_negative = instruction & 0x4000_0000 != 0;
        let condition = instruction & 0xf;
        let nzcv = (instruction >> 12) & 0xf;
        if condition_holds(context, condition) {
            let left = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
            let right = if immediate {
                ((instruction >> 16) & 31) as u64
            } else {
                read_reg(context, ((instruction >> 16) & 31) as usize, sf)
            };
            if compare_negative {
                let (result, carry, overflow) = add_sub(left, right, false, sf);
                set_flags(context, result, carry, overflow, sf);
            } else {
                let (result, carry, overflow) = add_sub(left, right, true, sf);
                set_flags(context, result, carry, overflow, sf);
            }
        } else {
            context.pstate = (context.pstate & !(NZCV_N | NZCV_Z | NZCV_C | NZCV_V))
                | (nzcv << 28);
        }
        Ok(())
    }

    fn execute_scalar_integer_to_float(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let source_is_64 = instruction & 0x8000_0000 != 0;
        let destination_is_double = instruction & 0x0040_0000 != 0;
        let unsigned = instruction & 0x0001_0000 != 0;
        let source = read_reg(context, ((instruction >> 5) & 31) as usize, source_is_64);
        let value = if unsigned {
            source as f64
        } else if source_is_64 {
            source as i64 as f64
        } else {
            source as i32 as f64
        };
        let destination = (instruction & 31) as usize;
        if destination_is_double {
            context.vectors[destination][0] = value.to_bits();
        } else {
            context.vectors[destination][0] = (value as f32).to_bits() as u64;
            context.vectors[destination][1] = 0;
        }
        Ok(())
    }

    fn execute_fixed_point_scalar_convert(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let source = ((instruction >> 5) & 31) as usize;
        let destination = (instruction & 31) as usize;
        let source_is_64_bit = instruction & 0x8000_0000 != 0;
        let operation = instruction & 0x003f_fc00;
        if operation == 0x0021_d800 || operation == 0x0029_d800 {
            let scale = (instruction >> 10) & 0x3f;
            let integer = if operation == 0x0021_d800 {
                context.vectors[source][0] as i64 as f64
            } else {
                context.vectors[source][0] as f64
            };
            context.vectors[destination][0] = (integer / 2f64.powi(scale as i32)).to_bits();
            return Ok(());
        }
        let source_is_double = instruction & 0x0040_0000 != 0;
        let opcode = instruction & 0x003f_fc00;
        match opcode {
            0x0022_0000 | 0x0023_0000 => {
                let integer = read_reg(context, source, source_is_64_bit);
                let value = if opcode == 0x0022_0000 {
                    if source_is_64_bit {
                        integer as i64 as f64
                    } else {
                        integer as i32 as f64
                    }
                } else if source_is_64_bit {
                    integer as f64
                } else {
                    integer as u32 as f64
                };
                context.vectors[destination][0] = if source_is_double {
                    value.to_bits()
                } else {
                    (value as f32).to_bits() as u64
                };
                if !source_is_double {
                    context.vectors[destination][1] = 0;
                }
            }
            0x0038_0000 | 0x0039_0000 => {
                let value = if source_is_double {
                    f64::from_bits(context.vectors[source][0])
                } else {
                    f32::from_bits(context.vectors[source][0] as u32) as f64
                };
                let integer = if opcode == 0x0038_0000 {
                    value as i64 as u64
                } else {
                    value as u64
                };
                set_reg(context, destination, integer, source_is_64_bit);
            }
            _ => return Err(InterpreterError::Undefined),
        }
        Ok(())
    }
    fn execute_sshll(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let source = ((instruction >> 5) & 31) as usize;
        let destination = (instruction & 31) as usize;
        let shift = (((instruction >> 16) & 7) | (((instruction >> 19) & 1) << 3)) as u32;
        let source_values = [
            context.vectors[source][0] as u32,
            (context.vectors[source][0] >> 32) as u32,
            context.vectors[source][1] as u32,
            (context.vectors[source][1] >> 32) as u32,
        ];
        let mut result = [0_u64; 2];
        for lane in 0..2 {
            let value = (source_values[lane] as i32 as i64) << shift;
            result[lane] = value as u64;
        }
        context.vectors[destination] = result;
        Ok(())
    }

    fn execute_scalar_compare(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let double = instruction & 0x0040_0000 != 0;
        let signaling_compare = instruction & 0x10 != 0;
        let compare_zero = instruction & 0x8 != 0;
        let rn = ((instruction >> 5) & 31) as usize;
        let rm = ((instruction >> 16) & 31) as usize;
        let flush_to_zero = context.fpcr & (1 << 24) != 0;
        let (left, left_nan, left_signaling_nan) =
            scalar_float_value(context.vectors[rn][0], double, flush_to_zero);
        let (right, right_nan, right_signaling_nan) = if compare_zero {
            (0.0, false, false)
        } else {
            scalar_float_value(context.vectors[rm][0], double, flush_to_zero)
        };
        let unordered = left_nan || right_nan;
        if unordered && (signaling_compare || left_signaling_nan || right_signaling_nan) {
            context.fpsr |= 1;
        }
        let flags = if unordered {
            NZCV_C | NZCV_V
        } else if left < right {
            NZCV_N
        } else if left > right {
            NZCV_C
        } else {
            NZCV_Z | NZCV_C
        };
        context.pstate = (context.pstate & !(NZCV_N | NZCV_Z | NZCV_C | NZCV_V)) | flags;
        Ok(())
    }

    fn execute_scalar_floating(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let double = instruction & 0x0040_0000 != 0;
        let rd = (instruction & 31) as usize;
        let rn = ((instruction >> 5) & 31) as usize;
        let rm = ((instruction >> 16) & 31) as usize;
        let left = if double {
            f64::from_bits(context.vectors[rn][0])
        } else {
            f32::from_bits(context.vectors[rn][0] as u32) as f64
        };
        let right = if double {
            f64::from_bits(context.vectors[rm][0])
        } else {
            f32::from_bits(context.vectors[rm][0] as u32) as f64
        };
        let operation = instruction & 0x0000_fc00;
        if operation == 0x0000_d800 {
            let scale = (instruction >> 10) & 0x3f;
            let integer = context.vectors[rn][0] as i64;
            context.vectors[rd][0] = ((integer as f64) / 2f64.powi(scale as i32)).to_bits();
            return Ok(());
        }
        let result = match operation {
            0x0000_0800 => left * right,
            0x0000_2800 => left + right,
            0x0000_3800 => left - right,
            _ => return Err(InterpreterError::Undefined),
        };
        if double {
            context.vectors[rd][0] = result.to_bits();
        } else {
            context.vectors[rd][0] = (result as f32).to_bits() as u64;
            context.vectors[rd][1] = 0;
        }
        Ok(())
    }

    fn execute_scalar_fma(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let double = instruction & 0x0040_0000 != 0;
        let rd = (instruction & 31) as usize;
        let rn = ((instruction >> 5) & 31) as usize;
        let ra = ((instruction >> 10) & 31) as usize;
        let rm = ((instruction >> 16) & 31) as usize;
        let read = |register: usize| {
            if double {
                f64::from_bits(context.vectors[register][0])
            } else {
                f32::from_bits(context.vectors[register][0] as u32) as f64
            }
        };
        let product = read(rn) * read(rm);
        let result = if instruction & 0x8000 != 0 {
            read(ra) - product
        } else {
            read(ra) + product
        };
        if double {
            context.vectors[rd][0] = result.to_bits();
        } else {
            context.vectors[rd][0] = (result as f32).to_bits() as u64;
            context.vectors[rd][1] = 0;
        }
        Ok(())
    }

    fn execute_bitfield(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let n = ((instruction >> 22) & 1) as u32;
        let immr = ((instruction >> 16) & 0x3f) as u32;
        let imms = ((instruction >> 10) & 0x3f) as u32;
        if (sf && n == 0) || (!sf && (n != 0 || immr >= 32 || imms >= 32)) {
            return Err(InterpreterError::Undefined);
        }
        let combined = (n << 6) | ((!imms) & 0x3f);
        let len = 31 - combined.leading_zeros();
        if len < 1 {
            return Err(InterpreterError::Undefined);
        }
        let levels = (1u32 << len) - 1;
        let s = imms & levels;
        let r = immr & levels;
        let d = s.wrapping_sub(r) & levels;
        let element_size = 1u32 << len;
        let width = if sf { 64 } else { 32 };
        let width_mask = if sf { u64::MAX } else { u32::MAX as u64 };
        let w_element = if s + 1 == 64 {
            u64::MAX
        } else {
            (1u64 << (s + 1)) - 1
        };
        let t_element = if d + 1 == 64 {
            u64::MAX
        } else {
            (1u64 << (d + 1)) - 1
        };
        let w_mask =
            rotate_right_width(replicate_element(w_element, element_size, width), r, width);
        let t_mask = replicate_element(t_element, element_size, width);
        let rn = ((instruction >> 5) & 31) as usize;
        let rd = (instruction & 31) as usize;
        let source = read_reg(context, rn, sf);
        let rotated = rotate_right_width(source, r, width);
        let value = match (instruction >> 29) & 3 {
            0 => {
                let bottom = rotated & w_mask & t_mask;
                let top = if source & (1u64 << s) != 0 {
                    !t_mask
                } else {
                    0
                };
                (top | bottom) & width_mask
            }
            1 => {
                let destination = read_reg(context, rd, sf);
                let bottom = (destination & !w_mask) | (rotated & w_mask);
                ((destination & !t_mask) | (bottom & t_mask)) & width_mask
            }
            2 => rotated & w_mask & t_mask,
            _ => return Err(InterpreterError::Undefined),
        };
        set_reg(context, rd, value, sf);
        Ok(())
    }

    fn execute_logical_immediate(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
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
        let ones = if s + 1 == 64 {
            u64::MAX
        } else {
            (1u64 << (s + 1)) - 1
        };
        let element_mask = if element_size == 64 {
            u64::MAX
        } else {
            (1u64 << element_size) - 1
        };
        let rotated = if r == 0 {
            ones
        } else {
            ((ones >> r) | (ones << (element_size - r))) & element_mask
        };
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

    fn execute_move_wide(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
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

    fn execute_add_sub_immediate(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let subtract = instruction & 0x4000_0000 != 0;
        let update_flags = instruction & 0x2000_0000 != 0;
        let shift = if instruction & 0x0040_0000 != 0 {
            12
        } else {
            0
        };
        let immediate = (((instruction >> 10) & 0xfff) as u64) << shift;
        let left = read_sp_or_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let (value, carry, overflow) = add_sub(left, immediate, subtract, sf);
        write_add_sub_result(
            context,
            (instruction & 31) as usize,
            value,
            sf,
            update_flags,
        );
        if update_flags {
            set_flags(context, value, carry, overflow, sf);
        }
        Ok(())
    }

    fn execute_add_sub_register(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let subtract = instruction & 0x4000_0000 != 0;
        let update_flags = instruction & 0x2000_0000 != 0;
        let rm = read_reg(context, ((instruction >> 16) & 31) as usize, sf);
        let shift_type = (instruction >> 22) & 3;
        let amount = ((instruction >> 10) & 0x3f) as u32;
        let right = shift_value(rm, shift_type, amount, sf);
        let left = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let (value, carry, overflow) = add_sub(left, right, subtract, sf);
        write_add_sub_result(
            context,
            (instruction & 31) as usize,
            value,
            sf,
            update_flags,
        );
        if update_flags {
            set_flags(context, value, carry, overflow, sf);
        }
        Ok(())
    }

    fn execute_logical(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let opc = (instruction >> 29) & 3;
        let invert = instruction & 0x0020_0000 != 0;
        let left = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let mut right = read_reg(context, ((instruction >> 16) & 31) as usize, sf);
        right = shift_value(
            right,
            (instruction >> 22) & 3,
            ((instruction >> 10) & 0x3f) as u32,
            sf,
        );
        if invert {
            right = !right;
        }
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

    fn execute_conditional_select(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let condition = (instruction >> 12) & 0xf;
        let condition_true = condition_holds(context, condition);
        let first = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let second = read_reg(context, ((instruction >> 16) & 31) as usize, sf);
        let value = if condition_true {
            first
        } else if instruction & 0x4000_0000 != 0 {
            if instruction & 0x0000_0400 != 0 {
                (!second).wrapping_add(1)
            } else {
                !second
            }
        } else if instruction & 0x0000_0400 != 0 {
            second.wrapping_add(1)
        } else {
            second
        };
        set_reg(context, (instruction & 31) as usize, value, sf);
        Ok(())
    }

    fn execute_multiply(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let left = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let right = read_reg(context, ((instruction >> 16) & 31) as usize, sf);
        let addend = read_reg(context, ((instruction >> 10) & 31) as usize, sf);
        let product = left.wrapping_mul(right);
        let value = if instruction & 0x0000_8000 != 0 {
            product.wrapping_sub(addend)
        } else {
            product.wrapping_add(addend)
        };
        set_reg(context, (instruction & 31) as usize, value, sf);
        Ok(())
    }

    fn execute_divide(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let sf = instruction >> 31 != 0;
        let left = read_reg(context, ((instruction >> 5) & 31) as usize, sf);
        let right = read_reg(context, ((instruction >> 16) & 31) as usize, sf);
        let signed = instruction & 0x400 != 0;
        let value = if right == 0 {
            0
        } else if signed {
            if sf {
                (left as i64).wrapping_div(right as i64) as u64
            } else {
                (left as i32).wrapping_div(right as i32) as u32 as u64
            }
        } else {
            left / right
        };
        set_reg(context, (instruction & 31) as usize, value, sf);
        Ok(())
    }

    fn execute_literal_load(
        &self,
        memory: &Mem64,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let size = if instruction & 0x4000_0000 != 0 { 8 } else { 4 };
        let address = context
            .pc
            .wrapping_add_signed(sign_extend(((instruction >> 5) & 0x7ffff) as u64, 19) << 2);
        let value = if size == 8 {
            memory.read_u64(address)
        } else {
            memory.read_u32(address).map(u64::from)
        }
        .map_err(|error| InterpreterError::Memory(error, address))?;
        set_reg(context, (instruction & 31) as usize, value, size == 8);
        Ok(())
    }

    fn execute_exclusive(
        &mut self,
        memory: &mut Mem64,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
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
                reserved == address
                    && read_memory_value(memory, address, size).ok() == Some(expected)
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

    fn execute_adv_simd_shift_long(
        &self,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let destination = (instruction & 31) as usize;
        let source = ((instruction >> 5) & 31) as usize;
        let upper = instruction & 0x0080_0000 != 0;
        let unsigned = instruction & 0x0040_0000 != 0;
        let shift = ((instruction >> 16) & 0x7) as u32;
        if shift != 0 || upper || unsigned {
            return Err(InterpreterError::Undefined);
        }
        let source_low = context.vectors[source][0] as u32 as i32 as i64 as u64;
        let source_high = (context.vectors[source][0] >> 32) as u32 as i32 as i64 as u64;
        context.vectors[destination][0] = source_low | (source_high << 32);
        context.vectors[destination][1] = 0;
        Ok(())
    }

    fn execute_simd_load_store(
        &self,
        memory: &mut Mem64,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let load = instruction & 0x0040_0000 != 0;
        let base = read_sp_or_reg(context, ((instruction >> 5) & 31) as usize, true);
        let offset = sign_extend(((instruction >> 12) & 0x1ff) as u64, 9);
        let address = base.wrapping_add_signed(offset);
        let register = (instruction & 31) as usize;
        if load {
            context.vectors[register] = memory
                .read_u128(address)
                .map_err(|error| InterpreterError::Memory(error, address))?;
        } else {
            memory
                .write_u128(address, context.vectors[register])
                .map_err(|error| InterpreterError::Memory(error, address))?;
        }
        Ok(())
    }

    fn execute_load_store_unsigned(
        &self,
        memory: &mut Mem64,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let size = 1u64 << ((instruction >> 30) & 3);
        let opcode = instruction & 0xffc0_0000;
        let signed_load = matches!(opcode, 0x3980_0000 | 0x7980_0000 | 0xb980_0000);
        let load = instruction & 0x0040_0000 != 0 || signed_load;
        let address = read_sp_or_reg(context, ((instruction >> 5) & 31) as usize, true)
            .wrapping_add(((instruction >> 10) & 0xfff) as u64 * size);
        if signed_load {
            let value = match size {
                1 => memory
                    .read_u8(address)
                    .map(|value| (value as i8 as i64) as u64),
                2 => memory
                    .read_u16(address)
                    .map(|value| (value as i16 as i64) as u64),
                4 => memory
                    .read_u32(address)
                    .map(|value| (value as i32 as i64) as u64),
                8 => memory.read_u64(address),
                _ => unreachable!(),
            }
            .map_err(|error| InterpreterError::Memory(error, address))?;
            set_reg(
                context,
                (instruction & 31) as usize,
                value,
                instruction & 0x8000_0000 != 0,
            );
            Ok(())
        } else {
            self.load_store(memory, context, instruction, address, size, load)
        }
    }

    fn execute_load_store_register(
        &self,
        memory: &mut Mem64,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
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
                1 => memory
                    .read_u8(address)
                    .map(|value| (value as i8 as i64) as u64),
                2 => memory
                    .read_u16(address)
                    .map(|value| (value as i16 as i64) as u64),
                4 => memory
                    .read_u32(address)
                    .map(|value| (value as i32 as i64) as u64),
                8 => memory.read_u64(address),
                _ => unreachable!(),
            }
            .map_err(|error| InterpreterError::Memory(error, address))?;
            set_reg(context, (instruction & 31) as usize, value, true);
            Ok(())
        } else {
            self.load_store(memory, context, instruction, address, size, load)
        }
    }

    fn execute_load_store_unscaled(
        &self,
        memory: &mut Mem64,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let size = 1u64 << ((instruction >> 30) & 3);
        let load = instruction & 0x0040_0000 != 0;
        let base = read_sp_or_reg(context, ((instruction >> 5) & 31) as usize, true);
        let offset = sign_extend(((instruction >> 12) & 0x1ff) as u64, 9);
        let mode = (instruction >> 10) & 3;
        let address = base.wrapping_add_signed(offset);
        self.load_store(memory, context, instruction, address, size, load)?;
        if mode == 1 {
            write_sp_or_reg(context, ((instruction >> 5) & 31) as usize, address, true);
        }
        if mode == 3 {
            write_sp_or_reg(context, ((instruction >> 5) & 31) as usize, address, true);
        }
        Ok(())
    }

    fn load_store(
        &self,
        memory: &mut Mem64,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
        address: u64,
        size: u64,
        load: bool,
    ) -> Result<(), InterpreterError> {
        let rt = (instruction & 31) as usize;
        if load {
            let value = match size {
                1 => memory.read_u8(address).map(u64::from),
                2 => memory.read_u16(address).map(u64::from),
                4 => memory.read_u32(address).map(u64::from),
                8 => memory.read_u64(address),
                _ => unreachable!(),
            }
            .map_err(|error| InterpreterError::Memory(error, address))?;
            set_reg(context, rt, value, size == 8);
        } else {
            let value = read_reg(context, rt, size == 8);
            let result = match size {
                1 => memory.write_u8(address, value as u8),
                2 => memory.write_u16(address, value as u16),
                4 => memory.write_u32(address, value as u32),
                8 => memory.write_u64(address, value),
                _ => unreachable!(),
            };
            result.map_err(|error| InterpreterError::Memory(error, address))?;
        }
        Ok(())
    }

    fn execute_pair(
        &self,
        memory: &mut Mem64,
        context: &mut touchHLE_DynarmicA64Context,
        instruction: u32,
    ) -> Result<(), InterpreterError> {
        let load = instruction & 0x0040_0000 != 0;
        let size = if instruction & 0x8000_0000 != 0 {
            8u64
        } else {
            4
        };
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
            let first = if size == 8 {
                memory.read_u64(address)
            } else {
                memory.read_u32(address).map(u64::from)
            }
            .map_err(|error| InterpreterError::Memory(error, address))?;
            let second_address = address
                .checked_add(size)
                .ok_or(InterpreterError::Memory("pair address overflows", address))?;
            let second = if size == 8 {
                memory.read_u64(second_address)
            } else {
                memory.read_u32(second_address).map(u64::from)
            }
            .map_err(|error| InterpreterError::Memory(error, second_address))?;
            set_reg(context, (instruction & 31) as usize, first, size == 8);
            set_reg(
                context,
                ((instruction >> 10) & 31) as usize,
                second,
                size == 8,
            );
        } else {
            let second_address = address
                .checked_add(size)
                .ok_or(InterpreterError::Memory("pair address overflows", address))?;
            if !memory.can_write(address, size).is_ok()
                || !memory.can_write(second_address, size).is_ok()
            {
                return Err(InterpreterError::Memory(
                    "pair store is not writable",
                    address,
                ));
            }
            let first = read_reg(context, (instruction & 31) as usize, size == 8);
            let second = read_reg(context, ((instruction >> 10) & 31) as usize, size == 8);
            if size == 8 {
                memory
                    .write_u64(address, first)
                    .map_err(|error| InterpreterError::Memory(error, address))?;
                memory
                    .write_u64(second_address, second)
                    .map_err(|error| InterpreterError::Memory(error, second_address))?;
            } else {
                memory
                    .write_u32(address, first as u32)
                    .map_err(|error| InterpreterError::Memory(error, address))?;
                memory
                    .write_u32(second_address, second as u32)
                    .map_err(|error| InterpreterError::Memory(error, second_address))?;
            }
        }
        if mode == 1 {
            write_sp_or_reg(context, base_reg, base.wrapping_add_signed(offset), true);
        }
        if mode == 3 {
            write_sp_or_reg(context, base_reg, address, true);
        }
        Ok(())
    }
}

#[cfg(test)]
mod conditional_select_tests {
    use super::A64Interpreter;
    use crate::mem64::{Mem64, Permissions};

    const CODE: u64 = 0x1_0000_0000;

    fn run(instruction: u32, pstate: u32, first: u64, second: u64) -> u64 {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        memory.load_bytes(CODE, &instruction.to_le_bytes()).unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context {
            pc: CODE,
            pstate,
            ..Default::default()
        };
        context.regs[1] = first;
        context.regs[2] = second;
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.pc, CODE + 4);
        context.regs[instruction as usize & 31]
    }

    #[test]
    fn cset_ne_uses_inverted_condition_and_writes_zero_register() {
        assert_eq!(run(0x1a9f07e8, 0, 0, 0), 1);
        assert_eq!(run(0x1a9f07e8, super::NZCV_Z, 0, 0), 0);
    }

    #[test]
    fn conditional_select_variants_have_distinct_false_operands() {
        assert_eq!(run(0x9a820020, 0, 0x11, 0x22), 0x22);
        assert_eq!(run(0x9a820420, 0, 0x11, 0x22), 0x23);
        assert_eq!(run(0x5a820020, 0, 0x11, 0x22), !0x22u32 as u64);
        assert_eq!(
            run(0x5a820420, 0, 0x11, 0x22),
            (!0x22u32).wrapping_add(1) as u64
        );
    }
}

#[derive(Debug)]
enum InterpreterError {
    Memory(&'static str, u64),
    Undefined,
    Breakpoint,
}

fn scalar_float_value(bits: u64, double: bool, flush_to_zero: bool) -> (f64, bool, bool) {
    if double {
        let exponent = bits & 0x7ff0_0000_0000_0000;
        let fraction = bits & 0x000f_ffff_ffff_ffff;
        let nan = exponent == 0x7ff0_0000_0000_0000 && fraction != 0;
        let signaling_nan = nan && bits & 0x0008_0000_0000_0000 == 0;
        let bits = if flush_to_zero && exponent == 0 && fraction != 0 {
            bits & 0x8000_0000_0000_0000
        } else {
            bits
        };
        (f64::from_bits(bits), nan, signaling_nan)
    } else {
        let bits = bits as u32;
        let exponent = bits & 0x7f80_0000;
        let fraction = bits & 0x007f_ffff;
        let nan = exponent == 0x7f80_0000 && fraction != 0;
        let signaling_nan = nan && bits & 0x0040_0000 == 0;
        let bits = if flush_to_zero && exponent == 0 && fraction != 0 {
            bits & 0x8000_0000
        } else {
            bits
        };
        (f32::from_bits(bits) as f64, nan, signaling_nan)
    }
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
    if index == 31 {
        return 0;
    }
    let value = context.regs[index];
    if sf {
        value
    } else {
        value as u32 as u64
    }
}

fn set_x(context: &mut touchHLE_DynarmicA64Context, index: usize, value: u64) {
    if index != 31 {
        context.regs[index] = value;
    }
}

fn set_reg(context: &mut touchHLE_DynarmicA64Context, index: usize, value: u64, sf: bool) {
    set_x(context, index, if sf { value } else { value as u32 as u64 });
}

fn replicate_element(value: u64, element_size: u32, width: u32) -> u64 {
    let mut result = 0;
    let mut shift = 0;
    while shift < width {
        result |= value << shift;
        shift += element_size;
    }
    result
}

fn rotate_right_width(value: u64, amount: u32, width: u32) -> u64 {
    if width == 32 {
        u64::from((value as u32).rotate_right(amount))
    } else {
        value.rotate_right(amount)
    }
}

fn read_sp_or_reg(context: &touchHLE_DynarmicA64Context, index: usize, sf: bool) -> u64 {
    if index == 31 {
        if sf {
            context.sp
        } else {
            context.sp as u32 as u64
        }
    } else {
        read_reg(context, index, sf)
    }
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
    if index == 31 {
        context.sp = if sf { value } else { value as u32 as u64 };
    } else {
        set_reg(context, index, value, sf);
    }
}

fn shift_value(value: u64, kind: u32, amount: u32, sf: bool) -> u64 {
    let width = if sf { 64 } else { 32 };
    let amount = amount % width;
    let value = if sf { value } else { value as u32 as u64 };
    match kind {
        0 => value << amount,
        1 => value >> amount,
        2 => {
            if sf {
                (value as i64 >> amount) as u64
            } else {
                ((value as u32 as i32) >> amount) as u32 as u64
            }
        }
        _ => value.rotate_right(amount),
    }
}

fn add_sub(left: u64, right: u64, subtract: bool, sf: bool) -> (u64, bool, bool) {
    let mask = if sf { u64::MAX } else { u32::MAX as u64 };
    let left = left & mask;
    let right = right & mask;
    let (value, carry) = if subtract {
        let (value, borrow) = left.overflowing_sub(right);
        (value & mask, !borrow)
    } else {
        let (value, carry) = left.overflowing_add(right);
        (value & mask, carry)
    };
    let sign = if sf { 63 } else { 31 };
    let overflow = if subtract {
        ((left ^ right) & (left ^ value) & (1u64 << sign)) != 0
    } else {
        ((!(left ^ right)) & (left ^ value) & (1u64 << sign)) != 0
    };
    (value, carry, overflow)
}

fn set_flags(
    context: &mut touchHLE_DynarmicA64Context,
    value: u64,
    carry: bool,
    overflow: bool,
    sf: bool,
) {
    let mask = if sf { u64::MAX } else { u32::MAX as u64 };
    let value = value & mask;
    context.pstate &= !(NZCV_N | NZCV_Z | NZCV_C | NZCV_V);
    if value & if sf { 1 << 63 } else { 1 << 31 } != 0 {
        context.pstate |= NZCV_N;
    }
    if value == 0 {
        context.pstate |= NZCV_Z;
    }
    if carry {
        context.pstate |= NZCV_C;
    }
    if overflow {
        context.pstate |= NZCV_V;
    }
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
mod scalar_compare_tests {
    use super::{A64Interpreter, NZCV_C, NZCV_N, NZCV_V, NZCV_Z};
    use crate::mem64::{Mem64, Permissions};
    use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

    const CODE: u64 = 0x4_0000_0000;
    const INITIAL_FLAGS: u32 = 0x0800_0000 | NZCV_N | NZCV_C;

    fn instruction(double: bool, signaling: bool, zero: bool, rn: usize, rm: usize) -> u32 {
        0x1e20_2000
            | if double { 0x0040_0000 } else { 0 }
            | if signaling { 0x10 } else { 0 }
            | if zero { 0x8 } else { 0 }
            | ((rm as u32) << 16)
            | ((rn as u32) << 5)
    }

    fn run(
        instruction: u32,
        left: u64,
        right: u64,
        pstate: u32,
        fpcr: u32,
        fpsr: u32,
    ) -> touchHLE_DynarmicA64Context {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        memory.load_bytes(CODE, &instruction.to_le_bytes()).unwrap();
        let mut context = touchHLE_DynarmicA64Context {
            pc: CODE,
            pstate,
            fpcr,
            fpsr,
            ..Default::default()
        };
        context.vectors[17][0] = left;
        context.vectors[23][0] = right;
        context.vectors[17][1] = 0x1111_2222_3333_4444;
        context.regs[6] = 0x6666_7777_8888_9999;
        A64Interpreter::new().run_or_step(&mut memory, &mut context, None);
        context
    }

    #[test]
    fn fcmp_single_sets_flags_for_less_equal_and_greater() {
        let less = run(
            instruction(false, false, false, 17, 23),
            1.0f32.to_bits() as u64,
            2.0f32.to_bits() as u64,
            INITIAL_FLAGS,
            0,
            0,
        );
        let equal = run(
            instruction(false, false, false, 17, 23),
            1.0f32.to_bits() as u64,
            1.0f32.to_bits() as u64,
            INITIAL_FLAGS,
            0,
            0,
        );
        let greater = run(
            instruction(false, false, false, 17, 23),
            2.0f32.to_bits() as u64,
            1.0f32.to_bits() as u64,
            INITIAL_FLAGS,
            0,
            0,
        );
        assert_eq!(less.pstate & (NZCV_N | NZCV_Z | NZCV_C | NZCV_V), NZCV_N);
        assert_eq!(
            equal.pstate & (NZCV_N | NZCV_Z | NZCV_C | NZCV_V),
            NZCV_Z | NZCV_C
        );
        assert_eq!(greater.pstate & (NZCV_N | NZCV_Z | NZCV_C | NZCV_V), NZCV_C);
    }

    #[test]
    fn fcmp_treats_positive_and_negative_zero_as_equal() {
        let context = run(
            instruction(false, false, false, 17, 23),
            0.0f32.to_bits() as u64,
            (-0.0f32).to_bits() as u64,
            INITIAL_FLAGS,
            0,
            0,
        );
        assert_eq!(
            context.pstate & (NZCV_N | NZCV_Z | NZCV_C | NZCV_V),
            NZCV_Z | NZCV_C
        );
    }

    #[test]
    fn fcmp_handles_single_and_double_infinities() {
        let single = run(
            instruction(false, false, false, 17, 23),
            f32::INFINITY.to_bits() as u64,
            f32::NEG_INFINITY.to_bits() as u64,
            INITIAL_FLAGS,
            0,
            0,
        );
        let double = run(
            instruction(true, false, false, 17, 23),
            f64::NEG_INFINITY.to_bits(),
            f64::INFINITY.to_bits(),
            INITIAL_FLAGS,
            0,
            0,
        );
        assert_eq!(single.pstate & (NZCV_N | NZCV_Z | NZCV_C | NZCV_V), NZCV_C);
        assert_eq!(double.pstate & (NZCV_N | NZCV_Z | NZCV_C | NZCV_V), NZCV_N);
    }

    #[test]
    fn fcmp_nan_is_unordered_and_fcmpe_sets_invalid_operation() {
        let quiet_nan = run(
            instruction(false, false, false, 17, 23),
            0x7fc0_0001,
            1.0f32.to_bits() as u64,
            INITIAL_FLAGS,
            0,
            0,
        );
        let quiet_signaling = run(
            instruction(false, true, false, 17, 23),
            0x7fc0_0001,
            1.0f32.to_bits() as u64,
            INITIAL_FLAGS,
            0,
            0,
        );
        let signaling_nan = run(
            instruction(false, false, false, 17, 23),
            0x7f80_0001,
            1.0f32.to_bits() as u64,
            INITIAL_FLAGS,
            0,
            0,
        );
        assert_eq!(
            quiet_nan.pstate & (NZCV_N | NZCV_Z | NZCV_C | NZCV_V),
            NZCV_C | NZCV_V
        );
        assert_eq!(quiet_nan.fpsr, 0);
        assert_eq!(quiet_signaling.fpsr & 1, 1);
        assert_eq!(signaling_nan.fpsr & 1, 1);
    }

    #[test]
    fn scalar_compare_preserves_unrelated_registers_and_supports_zero_form() {
        let context = run(
            instruction(true, false, true, 17, 23),
            f64::NEG_INFINITY.to_bits(),
            0,
            INITIAL_FLAGS,
            0,
            0x20,
        );
        assert_eq!(context.pstate & (NZCV_N | NZCV_Z | NZCV_C | NZCV_V), NZCV_N);
        assert_eq!(context.regs[6], 0x6666_7777_8888_9999);
        assert_eq!(context.vectors[17][1], 0x1111_2222_3333_4444);
        assert_eq!(context.fpsr, 0x20);
        assert_eq!(context.pc, CODE + 4);
    }
}

#[cfg(test)]
mod scalar_integer_to_float_tests {
    use super::A64Interpreter;
    use crate::mem64::{Mem64, Permissions};
    use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

    const CODE: u64 = 0x3_0000_0000;

    fn run(instruction: u32, value: u64) -> [u64; 2] {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        memory.load_bytes(CODE, &instruction.to_le_bytes()).unwrap();
        let mut context = touchHLE_DynarmicA64Context {
            pc: CODE,
            ..Default::default()
        };
        context.regs[8] = value;
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.pc, CODE + 4);
        context.vectors[0]
    }

    #[test]
    fn scvtf_d_converts_signed_x_register() {
        assert_eq!(run(0x9e620100, u64::MAX)[0], (-1.0f64).to_bits());
    }

    #[test]
    fn ucvtf_s_converts_unsigned_w_register() {
        assert_eq!(
            run(0x1e230100, u32::MAX as u64)[0],
            (u32::MAX as f32).to_bits() as u64
        );
    }
}

#[cfg(test)]
mod bitfield_tests {
    use super::A64Interpreter;
    use crate::mem64::{Mem64, Permissions};

    const CODE: u64 = 0x2_0000_0000;

    fn run(instruction: u32, value: u64) -> (u64, u32) {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        memory.load_bytes(CODE, &instruction.to_le_bytes()).unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context {
            pc: CODE,
            pstate: super::NZCV_N | super::NZCV_C | super::NZCV_V,
            ..Default::default()
        };
        context.regs[8] = value;
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.pc, CODE + 4);
        (context.regs[8], context.pstate)
    }

    #[test]
    fn ubfm_exact_minecraft_encoding_is_lsr_w8_by_three() {
        let unchanged_flags = super::NZCV_N | super::NZCV_C | super::NZCV_V;
        assert_eq!(run(0x53037d08, 0xf000_0000), (0x1e00_0000, unchanged_flags));
        assert_eq!(run(0x53037d08, 0), (0, unchanged_flags));
        assert_eq!(
            run(0x53037d08, u32::MAX as u64),
            (0x1fff_ffff, unchanged_flags)
        );
        assert_eq!(
            run(0x53037d08, 0xffff_ffff_ffff_ffff),
            (0x1fff_ffff, unchanged_flags)
        );
    }

    #[test]
    fn bitfield_writes_zero_extend_without_modifying_flags() {
        let unchanged_flags = super::NZCV_N | super::NZCV_C | super::NZCV_V;
        assert_eq!(run(0x53007c08, 0xffff_ffff_0000_0001), (0, unchanged_flags));
    }
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
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        const SP: u64 = 0x7fff_fffe_ff10;
        memory
            .map_zeroed_with_permissions(SP - 0x2000, 0x4000, Permissions::read_write())
            .unwrap();
        memory
            .load_bytes(CODE, &0xa9bd57f6u32.to_le_bytes())
            .unwrap();
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
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        const SP: u64 = 0x7fff_fffe_ff00;
        memory
            .map_zeroed_with_permissions(SP, 0x1000, Permissions::read_write())
            .unwrap();
        memory
            .load_bytes(CODE, &0xa9417bfdu32.to_le_bytes())
            .unwrap();
        memory.write_u64(SP + 0x10, 0x2929_2929_2929_2929).unwrap();
        memory.write_u64(SP + 0x18, 0x3030_3030_3030_3030).unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context {
            pc: CODE,
            sp: SP,
            ..Default::default()
        };
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.regs[29], 0x2929_2929_2929_2929);
        assert_eq!(context.regs[30], 0x3030_3030_3030_3030);
        assert_eq!(context.sp, SP);
    }

    #[test]
    fn post_indexed_ldp_uses_signed_immediate_for_sp_update() {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        const SP: u64 = 0x7fff_fffe_ff00;
        memory
            .map_zeroed_with_permissions(SP, 0x1000, Permissions::read_write())
            .unwrap();
        memory
            .load_bytes(CODE, &0xa8c27bfdu32.to_le_bytes())
            .unwrap();
        memory.write_u64(SP, 0x2929_2929_2929_2929).unwrap();
        memory.write_u64(SP + 8, 0x3030_3030_3030_3030).unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context {
            pc: CODE,
            sp: SP,
            ..Default::default()
        };
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.regs[29], 0x2929_2929_2929_2929);
        assert_eq!(context.regs[30], 0x3030_3030_3030_3030);
        assert_eq!(context.sp, SP + 0x20);
    }

    #[test]
    fn cmp_using_xzr_does_not_modify_sp() {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        const SP: u64 = 0x7fff_fffe_fe30;
        memory
            .map_zeroed_with_permissions(SP - 0x1000, 0x2000, Permissions::read_write())
            .unwrap();
        memory
            .load_bytes(CODE, &0xeb08029fu32.to_le_bytes())
            .unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context {
            pc: CODE,
            sp: SP,
            ..Default::default()
        };
        context.regs[20] = 0x20;
        context.regs[8] = 0x10;
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.sp, SP);
        assert!(context.pstate & (1 << 29) != 0);
    }

    #[test]
    fn add_sub_register_uses_zero_register_for_register_31() {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        let instruction = 0x8b1f0108u32;
        memory.load_bytes(CODE, &instruction.to_le_bytes()).unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context {
            pc: CODE,
            sp: 0x7fff_fffe_f000,
            ..Default::default()
        };
        context.regs[8] = 7;
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.regs[8], 7);
        assert_eq!(context.sp, 0x7fff_fffe_f000);
    }

    #[test]
    fn interpreter_executes_basic_stack_sequence() {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        memory
            .map_zeroed_with_permissions(STACK - 0x2000, 0x4000, Permissions::read_write())
            .unwrap();
        let instructions = [0xd2800540u32, 0x91000400, 0xa9bf07e0, 0xa8c107e0];
        for (index, instruction) in instructions.iter().enumerate() {
            memory
                .load_bytes(CODE + index as u64 * 4, &instruction.to_le_bytes())
                .unwrap();
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
    fn umulh_matches_aarch64_high_product() {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        let instruction = 0x9bca7d29u32;
        memory.load_bytes(CODE, &instruction.to_le_bytes()).unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context {
            pc: CODE,
            ..Default::default()
        };
        context.regs[9] = u64::MAX;
        context.regs[10] = 16;
        assert_eq!(
            A64Interpreter::new().run_or_step(&mut memory, &mut context, None),
            -1
        );
        assert_eq!(context.regs[9], 15);
    }

    #[test]
    fn smulh_writes_signed_high_product() {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        let instruction = 0x9b407d09u32;
        memory.load_bytes(CODE, &instruction.to_le_bytes()).unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context {
            pc: CODE,
            ..Default::default()
        };
        context.regs[8] = u64::MAX;
        context.regs[0] = 2;
        assert_eq!(
            A64Interpreter::new().run_or_step(&mut memory, &mut context, None),
            -1
        );
        assert_eq!(context.regs[9], u64::MAX);
    }

    #[test]
    fn rejects_execute_and_write_access_to_read_only_code() {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(0x1000, 0x1000, Permissions::READ)
            .unwrap();
        assert_eq!(
            A64Interpreter::new().run_or_step(
                &mut memory,
                &mut touchHLE_DynarmicA64Context {
                    pc: 0x1000,
                    ..Default::default()
                },
                None
            ),
            -2
        );
        assert!(memory.write_u32(0x1000, 0).is_err());
    }

    #[test]
    fn register_offset_store_with_xzr_source_writes_zero() {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        const DATA: u64 = 0x3_0000_0000;
        memory
            .map_zeroed_with_permissions(DATA, 0x1000, Permissions::read_write())
            .unwrap();
        memory.write_u64(DATA + 0x20, u64::MAX).unwrap();
        memory
            .load_bytes(CODE, &0xf829695fu32.to_le_bytes())
            .unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context {
            pc: CODE,
            ..Default::default()
        };
        context.regs[10] = DATA;
        context.regs[9] = 0;
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(memory.read_u64(DATA).unwrap(), 0);
        assert_eq!(context.pc, CODE + 4);
    }

    #[test]
    fn csinc_w_form_decodes_the_minecraft_blocker() {
        let mut memory = Mem64::new();
        memory
            .map_zeroed_with_permissions(CODE, 0x1000, Permissions::read_execute())
            .unwrap();
        memory
            .load_bytes(CODE, &0x1ac92589u32.to_le_bytes())
            .unwrap();
        let mut context = touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context {
            pc: CODE,
            pstate: super::NZCV_Z,
            ..Default::default()
        };
        context.regs[12] = 41;
        context.regs[9] = 1;
        let mut interpreter = A64Interpreter::new();
        assert_eq!(interpreter.run_or_step(&mut memory, &mut context, None), -1);
        assert_eq!(context.regs[9], 2);
        assert_eq!(context.pc, CODE + 4);
        assert_eq!(context.pstate, super::NZCV_Z);
    }
}
