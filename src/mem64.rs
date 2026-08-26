use std::collections::BTreeMap;

use crate::mem::{SafeRead, SafeWrite};

pub type Guest64USize = u64;
pub type Guest64Addr = u64;

const MAX_GUEST_ALLOCATION: Guest64USize = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions(u8);

impl Permissions {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1);
    pub const WRITE: Self = Self(2);
    pub const EXECUTE: Self = Self(4);

    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    pub const fn from_mach_protection(initprot: i32, _maxprot: i32) -> Self {
        let mut permissions = Self::NONE;
        if initprot & 1 != 0 {
            permissions = Self(permissions.0 | Self::READ.0);
        }
        if initprot & 2 != 0 {
            permissions = Self(permissions.0 | Self::WRITE.0);
        }
        if initprot & 4 != 0 {
            permissions = Self(permissions.0 | Self::EXECUTE.0);
        }
        permissions
    }

    pub const fn read_write() -> Self {
        Self(Self::READ.0 | Self::WRITE.0)
    }

    pub const fn read_execute() -> Self {
        Self(Self::READ.0 | Self::EXECUTE.0)
    }

    pub const fn read_write_execute() -> Self {
        Self(Self::READ.0 | Self::WRITE.0 | Self::EXECUTE.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessType {
    Read,
    Write,
    Execute,
}

impl AccessType {
    fn permission(self) -> Permissions {
        match self {
            Self::Read => Permissions::READ,
            Self::Write => Permissions::WRITE,
            Self::Execute => Permissions::EXECUTE,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub base: Guest64Addr,
    pub size: Guest64USize,
    pub permissions: Permissions,
}

#[derive(Debug)]
struct Mapping {
    bytes: Vec<u8>,
    permissions: Permissions,
}

#[derive(Debug, Default)]
pub struct Mem64 {
    regions: BTreeMap<Guest64Addr, Mapping>,
    allocations: BTreeMap<Guest64Addr, Guest64USize>,
    next_allocation: Guest64Addr,
}

impl Mem64 {
    pub fn new() -> Self {
        Self {
            next_allocation: 0x1_0000_0000,
            ..Self::default()
        }
    }

    pub fn map_zeroed(
        &mut self,
        base: Guest64Addr,
        size: Guest64USize,
    ) -> Result<(), &'static str> {
        self.map_zeroed_with_permissions(base, size, Permissions::read_write_execute())
    }

    pub fn map_zeroed_with_permissions(
        &mut self,
        base: Guest64Addr,
        size: Guest64USize,
        permissions: Permissions,
    ) -> Result<(), &'static str> {
        let size_usize =
            usize::try_from(size).map_err(|_| "64-bit mapping is too large for this host")?;
        let end = base.checked_add(size).ok_or("64-bit mapping overflows")?;
        if size == 0 {
            return Ok(());
        }
        if let Some((&previous_base, previous)) = self.regions.range(..=base).next_back() {
            let previous_end = previous_base
                .checked_add(previous.bytes.len() as u64)
                .ok_or("mapping overflows")?;
            if previous_end > base {
                return Err("64-bit mapping overlaps an existing mapping");
            }
        }
        if self
            .regions
            .range(base..)
            .next()
            .is_some_and(|(&next_base, _)| next_base < end)
        {
            return Err("64-bit mapping overlaps an existing mapping");
        }
        self.regions.insert(
            base,
            Mapping {
                bytes: vec![0; size_usize],
                permissions,
            },
        );
        Ok(())
    }

    pub fn set_permissions(
        &mut self,
        base: Guest64Addr,
        size: Guest64USize,
        permissions: Permissions,
    ) -> Result<(), &'static str> {
        let end = base.checked_add(size).ok_or("permission range overflows")?;
        let mapping = self
            .regions
            .get_mut(&base)
            .ok_or("permission range is unmapped")?;
        if mapping.bytes.len() as u64 != size {
            return Err("permission range must cover one complete mapping");
        }
        let mapping_end = base
            .checked_add(mapping.bytes.len() as u64)
            .ok_or("mapping overflows")?;
        if mapping_end != end {
            return Err("permission range is not a complete mapping");
        }
        mapping.permissions = permissions;
        Ok(())
    }

    pub fn write_bytes(&mut self, base: Guest64Addr, bytes: &[u8]) -> Result<(), &'static str> {
        self.write_bytes_with_access(base, bytes, AccessType::Write)
    }

    pub fn load_u64(&mut self, address: Guest64Addr, value: u64) -> Result<(), &'static str> {
        self.load_bytes(address, &value.to_le_bytes())
    }

    pub fn load_bytes(&mut self, base: Guest64Addr, bytes: &[u8]) -> Result<(), &'static str> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut address = base;
        let mut source_offset = 0;
        while source_offset < bytes.len() {
            let (&region_base, mapping) = self
                .regions
                .range(..=address)
                .next_back()
                .ok_or("64-bit memory is unmapped")?;
            let offset = usize::try_from(address - region_base)
                .map_err(|_| "64-bit memory offset overflows host usize")?;
            let available = mapping.bytes.len().saturating_sub(offset);
            let count = available.min(bytes.len() - source_offset);
            if count == 0 {
                return Err("64-bit memory load is out of bounds");
            }
            let mapping = self
                .regions
                .get_mut(&region_base)
                .ok_or("64-bit memory is unmapped")?;
            mapping.bytes[offset..offset + count]
                .copy_from_slice(&bytes[source_offset..source_offset + count]);
            source_offset += count;
            address = address
                .checked_add(count as u64)
                .ok_or("64-bit memory address overflows")?;
        }
        Ok(())
    }

    pub fn write_bytes_with_access(
        &mut self,
        base: Guest64Addr,
        bytes: &[u8],
        access: AccessType,
    ) -> Result<(), &'static str> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut address = base;
        let mut source_offset = 0;
        while source_offset < bytes.len() {
            let (&region_base, mapping) = self
                .regions
                .range(..=address)
                .next_back()
                .ok_or("64-bit memory is unmapped")?;
            if !mapping.permissions.contains(access.permission()) {
                return Err(match access {
                    AccessType::Read => "64-bit memory read protection fault",
                    AccessType::Write => "64-bit memory write protection fault",
                    AccessType::Execute => "64-bit memory execute protection fault",
                });
            }
            let offset = usize::try_from(address - region_base)
                .map_err(|_| "64-bit memory offset overflows host usize")?;
            let available = mapping.bytes.len().saturating_sub(offset);
            let count = available.min(bytes.len() - source_offset);
            if count == 0 {
                return Err("64-bit memory write is out of bounds");
            }
            let mapping = self
                .regions
                .get_mut(&region_base)
                .ok_or("64-bit memory is unmapped")?;
            mapping.bytes[offset..offset + count]
                .copy_from_slice(&bytes[source_offset..source_offset + count]);
            source_offset += count;
            address = address
                .checked_add(count as u64)
                .ok_or("64-bit memory address overflows")?;
        }
        Ok(())
    }

    pub fn fill_bytes(
        &mut self,
        base: Guest64Addr,
        value: u8,
        size: Guest64USize,
    ) -> Result<(), &'static str> {
        let size = usize::try_from(size).map_err(|_| "64-bit fill is too large for this host")?;
        if size == 0 {
            return Ok(());
        }
        let mut address = base;
        let end = base
            .checked_add(size as u64)
            .ok_or("64-bit fill address overflows")?;
        while address < end {
            let (&region_base, mapping) = self
                .regions
                .range(..=address)
                .next_back()
                .ok_or("64-bit memory is unmapped")?;
            if !mapping.permissions.contains(AccessType::Write.permission()) {
                return Err("64-bit memory write protection fault");
            }
            let offset = usize::try_from(address - region_base)
                .map_err(|_| "64-bit memory offset overflows host usize")?;
            let count = (mapping.bytes.len() - offset).min((end - address) as usize);
            if count == 0 {
                return Err("64-bit memory fill is out of bounds");
            }
            let mapping = self
                .regions
                .get_mut(&region_base)
                .ok_or("64-bit memory is unmapped")?;
            mapping.bytes[offset..offset + count].fill(value);
            address += count as u64;
        }
        Ok(())
    }

    pub fn copy_bytes(
        &mut self,
        destination: Guest64Addr,
        source: Guest64Addr,
        size: Guest64USize,
    ) -> Result<(), &'static str> {
        let size = usize::try_from(size).map_err(|_| "64-bit copy is too large for this host")?;
        if size == 0 {
            return Ok(());
        }
        let bytes = self.read_bytes_with_access(source, size, AccessType::Read)?;
        self.write_bytes_with_access(destination, &bytes, AccessType::Write)
    }

    pub fn cstr_len(
        &self,
        base: Guest64Addr,
        limit: Guest64USize,
    ) -> Result<Guest64USize, &'static str> {
        let limit =
            usize::try_from(limit).map_err(|_| "64-bit string limit is too large for this host")?;
        for length in 0..limit {
            let address = base
                .checked_add(length as u64)
                .ok_or("64-bit string address overflows")?;
            if self.read_u8(address)? == 0 {
                return Ok(length as u64);
            }
        }
        Err("64-bit string has no terminator within the safety limit")
    }

    pub fn free(&mut self, address: Guest64Addr) -> bool {
        let Some(_) = self.allocations.remove(&address) else {
            return false;
        };
        self.regions.remove(&address).is_some()
    }

    pub fn read_bytes(
        &self,
        base: Guest64Addr,
        size: Guest64USize,
    ) -> Result<Vec<u8>, &'static str> {
        let size = usize::try_from(size).map_err(|_| "64-bit read is too large for this host")?;
        self.read_bytes_with_access(base, size, AccessType::Read)
    }

    fn read_bytes_with_access(
        &self,
        base: Guest64Addr,
        size: usize,
        access: AccessType,
    ) -> Result<Vec<u8>, &'static str> {
        if size == 0 {
            return Ok(Vec::new());
        }
        let mut bytes = Vec::with_capacity(size);
        let mut address = base;
        while bytes.len() < size {
            let (&region_base, mapping) = self
                .regions
                .range(..=address)
                .next_back()
                .ok_or("64-bit memory is unmapped")?;
            if !mapping.permissions.contains(access.permission()) {
                return Err(match access {
                    AccessType::Read => "64-bit memory read protection fault",
                    AccessType::Write => "64-bit memory write protection fault",
                    AccessType::Execute => "64-bit memory execute protection fault",
                });
            }
            let offset = usize::try_from(address - region_base)
                .map_err(|_| "64-bit memory offset overflows host usize")?;
            let available = mapping.bytes.len().saturating_sub(offset);
            let count = available.min(size - bytes.len());
            if count == 0 {
                return Err("64-bit memory read is out of bounds");
            }
            bytes.extend_from_slice(&mapping.bytes[offset..offset + count]);
            address = address
                .checked_add(count as u64)
                .ok_or("64-bit memory address overflows")?;
        }
        Ok(bytes)
    }
    pub fn host_ptr(
        &self,
        base: Guest64Addr,
        size: Guest64USize,
    ) -> Result<*const u8, &'static str> {
        let size = usize::try_from(size).map_err(|_| "64-bit host pointer size is too large")?;
        Ok(self
            .slice_with_access(base, size, AccessType::Read)?
            .as_ptr())
    }

    pub fn host_ptr_mut(
        &mut self,
        base: Guest64Addr,
        size: Guest64USize,
    ) -> Result<*mut u8, &'static str> {
        let size =
            usize::try_from(size).map_err(|_| "64-bit mutable host pointer size is too large")?;
        Ok(self
            .slice_mut_with_access(base, size, AccessType::Write)?
            .as_mut_ptr())
    }

    pub fn allocation_size(&self, address: Guest64Addr) -> Option<Guest64USize> {
        self.allocations.get(&address).copied()
    }

    pub fn alloc_zeroed(&mut self, size: Guest64USize) -> Result<Guest64Addr, &'static str> {
        self.alloc_zeroed_with_permissions(size, Permissions::read_write())
    }

    pub fn alloc_zeroed_with_permissions(
        &mut self,
        size: Guest64USize,
        permissions: Permissions,
    ) -> Result<Guest64Addr, &'static str> {
        if size > MAX_GUEST_ALLOCATION {
            return Err("64-bit allocation request exceeds the safety limit");
        }
        let size = size
            .max(16)
            .checked_add(15)
            .ok_or("allocation size overflows")?
            & !15;
        let mut base = self.next_allocation.max(0x1_0000_0000);
        loop {
            let end = base
                .checked_add(size)
                .ok_or("allocation address overflows")?;
            let overlapping =
                self.regions
                    .range(..end)
                    .next_back()
                    .and_then(|(&region_base, mapping)| {
                        let region_end = region_base.checked_add(mapping.bytes.len() as u64)?;
                        (region_end > base && region_base < end).then_some(region_end)
                    });
            match overlapping {
                Some(region_end) => {
                    base = region_end
                        .checked_add(15)
                        .ok_or("allocation address overflows")?
                        & !15
                }
                None => break,
            }
        }
        self.map_zeroed_with_permissions(base, size, permissions)?;
        self.allocations.insert(base, size);
        self.next_allocation = base
            .checked_add(size)
            .ok_or("allocation cursor overflows")?;
        Ok(base)
    }

    fn mapping(&self, addr: Guest64Addr, size: usize) -> Result<(&Mapping, usize), &'static str> {
        let (&base, mapping) = self
            .regions
            .range(..=addr)
            .next_back()
            .ok_or("64-bit memory access is unmapped")?;
        let offset = addr.checked_sub(base).ok_or("64-bit address underflow")?;
        let end = offset
            .checked_add(size as u64)
            .ok_or("64-bit access overflows")?;
        if end > mapping.bytes.len() as u64 {
            return Err("64-bit memory access is out of bounds");
        }
        Ok((
            mapping,
            usize::try_from(offset).map_err(|_| "64-bit offset overflows host usize")?,
        ))
    }

    fn check_access(
        &self,
        addr: Guest64Addr,
        size: usize,
        access: AccessType,
    ) -> Result<(&Mapping, usize), &'static str> {
        let (mapping, offset) = self.mapping(addr, size)?;
        if !mapping.permissions.contains(access.permission()) {
            return Err(match access {
                AccessType::Read => "64-bit memory read protection fault",
                AccessType::Write => "64-bit memory write protection fault",
                AccessType::Execute => "64-bit memory execute protection fault",
            });
        }
        Ok((mapping, offset))
    }

    fn slice_with_access(
        &self,
        addr: Guest64Addr,
        size: usize,
        access: AccessType,
    ) -> Result<&[u8], &'static str> {
        let (mapping, offset) = self.check_access(addr, size, access)?;
        Ok(&mapping.bytes[offset..offset + size])
    }

    fn slice_mut_with_access(
        &mut self,
        addr: Guest64Addr,
        size: usize,
        access: AccessType,
    ) -> Result<&mut [u8], &'static str> {
        let (&base, _) = self
            .regions
            .range(..=addr)
            .next_back()
            .ok_or("64-bit memory access is unmapped")?;
        let (mapping, offset) = self.mapping(addr, size)?;
        if !mapping.permissions.contains(access.permission()) {
            return Err(match access {
                AccessType::Read => "64-bit memory read protection fault",
                AccessType::Write => "64-bit memory write protection fault",
                AccessType::Execute => "64-bit memory execute protection fault",
            });
        }
        let mapping = self
            .regions
            .get_mut(&base)
            .ok_or("64-bit memory access is unmapped")?;
        Ok(&mut mapping.bytes[offset..offset + size])
    }

    pub fn can_write(&self, addr: Guest64Addr, size: Guest64USize) -> Result<(), &'static str> {
        self.check_access(
            addr,
            usize::try_from(size).map_err(|_| "64-bit write size is too large")?,
            AccessType::Write,
        )
        .map(|_| ())
    }

    pub fn read_code_u32(&self, addr: Guest64Addr) -> Result<u32, &'static str> {
        self.read_with_access(addr, AccessType::Execute)
    }

    pub fn read_with_access<T: SafeRead + Copy>(
        &self,
        addr: Guest64Addr,
        access: AccessType,
    ) -> Result<T, &'static str> {
        let size = std::mem::size_of::<T>();
        let bytes = self.read_bytes_with_access(addr, size, access)?;
        let mut value = std::mem::MaybeUninit::<T>::uninit();
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), value.as_mut_ptr().cast(), size);
            Ok(value.assume_init())
        }
    }

    pub fn write_with_access<T: SafeWrite>(
        &mut self,
        addr: Guest64Addr,
        value: T,
        access: AccessType,
    ) -> Result<(), &'static str> {
        let size = std::mem::size_of::<T>();
        let bytes = unsafe { std::slice::from_raw_parts((&value as *const T).cast::<u8>(), size) };
        self.write_bytes_with_access(addr, bytes, access)
    }
    pub fn read<T: SafeRead + Copy>(&self, addr: Guest64Addr) -> Result<T, &'static str> {
        self.read_with_access(addr, AccessType::Read)
    }

    pub fn write<T: SafeWrite>(&mut self, addr: Guest64Addr, value: T) -> Result<(), &'static str> {
        self.write_with_access(addr, value, AccessType::Write)
    }

    pub fn read_u8(&self, addr: Guest64Addr) -> Result<u8, &'static str> {
        self.read(addr)
    }
    pub fn read_u16(&self, addr: Guest64Addr) -> Result<u16, &'static str> {
        self.read(addr)
    }
    pub fn read_u32(&self, addr: Guest64Addr) -> Result<u32, &'static str> {
        self.read(addr)
    }
    pub fn read_u64(&self, addr: Guest64Addr) -> Result<u64, &'static str> {
        self.read(addr)
    }
    pub fn read_u128(&self, addr: Guest64Addr) -> Result<[u64; 2], &'static str> {
        self.read(addr)
    }
    pub fn write_u8(&mut self, addr: Guest64Addr, value: u8) -> Result<(), &'static str> {
        self.write(addr, value)
    }
    pub fn write_u16(&mut self, addr: Guest64Addr, value: u16) -> Result<(), &'static str> {
        self.write(addr, value)
    }
    pub fn write_u32(&mut self, addr: Guest64Addr, value: u32) -> Result<(), &'static str> {
        self.write(addr, value)
    }
    pub fn write_u64(&mut self, addr: Guest64Addr, value: u64) -> Result<(), &'static str> {
        self.write(addr, value)
    }
    pub fn write_u128(&mut self, addr: Guest64Addr, value: [u64; 2]) -> Result<(), &'static str> {
        self.write(addr, value)
    }

    pub fn merge_mappings(&mut self, other: Mem64) -> Result<(), &'static str> {
        for (&base, mapping) in &other.regions {
            self.map_zeroed_with_permissions(
                base,
                mapping.bytes.len() as u64,
                mapping.permissions,
            )?;
        }
        for (base, mapping) in other.regions {
            self.load_bytes(base, &mapping.bytes)?;
        }
        self.allocations.extend(other.allocations);
        self.next_allocation = self.next_allocation.max(other.next_allocation);
        Ok(())
    }

    pub fn mapped_regions(&self) -> impl Iterator<Item = Region> + '_ {
        self.regions.iter().map(|(&base, mapping)| Region {
            base,
            size: mapping.bytes.len() as u64,
            permissions: mapping.permissions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Mem64, Permissions};

    #[test]
    fn allocations_skip_loaded_regions() {
        let mut mem = Mem64::new();
        mem.map_zeroed(0x1_0000_0000, 0x2000).unwrap();
        let allocation = mem.alloc_zeroed(0x100).unwrap();
        assert_eq!(allocation, 0x1_0000_2000);
    }

    #[test]
    fn accesses_are_checked_at_region_boundaries() {
        let mut mem = Mem64::new();
        mem.map_zeroed(0x1_0000_0000, 0x10).unwrap();
        assert!(mem.write_u64(0x1_0000_0008, 1).is_ok());
        assert!(mem.write_u64(0x1_0000_0009, 1).is_err());
        assert!(mem.read_u32(0x1_0000_000e).is_err());
    }

    #[test]
    fn permissions_separate_code_and_data_access() {
        let mut mem = Mem64::new();
        mem.map_zeroed_with_permissions(0x1_0000_0000, 0x1000, Permissions::read_execute())
            .unwrap();
        assert!(mem.read_code_u32(0x1_0000_0000).is_ok());
        assert!(mem.write_u32(0x1_0000_0000, 1).is_err());
        assert!(mem.read_u32(0x1_0000_0000).is_ok());
    }

    #[test]
    fn free_releases_only_emulator_allocations() {
        let mut mem = Mem64::new();
        let address = mem.alloc_zeroed(32).unwrap();
        assert!(mem.free(address));
        assert!(mem.read_u8(address).is_err());
        assert!(!mem.free(address));
    }

    #[test]
    fn bulk_operations_cross_adjacent_mappings() {
        let mut mem = Mem64::new();
        mem.map_zeroed(0x1_0000_0000, 0x10).unwrap();
        mem.map_zeroed(0x1_0000_0010, 0x10).unwrap();
        mem.write_bytes(0x1_0000_000c, &[1, 2, 3, 4, 5, 6, 7, 8])
            .unwrap();
        assert_eq!(
            mem.read_bytes(0x1_0000_000c, 8).unwrap(),
            [1, 2, 3, 4, 5, 6, 7, 8]
        );
        mem.fill_bytes(0x1_0000_000e, 0xaa, 4).unwrap();
        assert_eq!(mem.read_bytes(0x1_0000_000e, 4).unwrap(), [0xaa; 4]);
    }
}
