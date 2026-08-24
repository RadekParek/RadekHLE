use crate::mem64::Mem64;
use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

const MAX_CSTRING: u64 = 1024 * 1024;

pub struct A64Abi;

impl A64Abi {
    pub fn arg(context: &touchHLE_DynarmicA64Context, index: usize) -> u64 {
        context.regs[index]
    }

    pub fn stack_arg(
        memory: &Mem64,
        context: &touchHLE_DynarmicA64Context,
        index: usize,
    ) -> Result<u64, String> {
        if index < 8 {
            return Ok(Self::arg(context, index));
        }
        let offset = (index - 8)
            .checked_mul(8)
            .ok_or("ARM64 ABI argument offset overflows")? as u64;
        memory
            .read_u64(
                context
                    .sp
                    .checked_add(offset)
                    .ok_or("ARM64 ABI stack argument overflows")?,
            )
            .map_err(str::to_owned)
    }

    pub fn set_return(context: &mut touchHLE_DynarmicA64Context, value: u64) {
        context.regs[0] = value;
    }

    pub fn set_return_pair(context: &mut touchHLE_DynarmicA64Context, low: u64, high: u64) {
        context.regs[0] = low;
        context.regs[1] = high;
    }

    pub fn c_string(memory: &Mem64, address: u64) -> Option<Vec<u8>> {
        let length = memory.cstr_len(address, MAX_CSTRING).ok()?;
        memory.read_bytes(address, length).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::A64Abi;
    use crate::mem64::Mem64;
    use touchHLE_dynarmic_wrapper::touchHLE_DynarmicA64Context;

    #[test]
    fn reads_register_arguments_and_writes_return_register() {
        let memory = Mem64::new();
        let mut context = touchHLE_DynarmicA64Context::default();
        context.regs[0] = 41;
        assert_eq!(A64Abi::arg(&context, 0), 41);
        A64Abi::set_return(&mut context, 42);
        assert_eq!(context.regs[0], 42);
        assert!(memory.mapped_regions().next().is_none());
    }
}
