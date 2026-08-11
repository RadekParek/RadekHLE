use std::collections::HashMap;
use std::io::{Cursor, Seek, SeekFrom};

use std::convert::TryInto;

use mach_object::{Bind, BindSymbolType, LazyBind, LinkEditData, LoadCommand, MachCommand, OFile, Rebase, Symbol, SymbolIter, ThreadState, WeakBind};

use crate::mem64::{Guest64Addr, Mem64, Permissions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Architecture {
    Arm64,
}

#[derive(Debug, Clone)]
pub struct Binding64 {
    pub address: Guest64Addr,
    pub symbol: String,
    pub addend: i64,
}

#[derive(Debug, Clone)]
pub struct Section64 {
    pub name: String,
    pub address: Guest64Addr,
    pub size: u64,
}

#[derive(Debug)]
pub struct MachO64 {
    pub architecture: Architecture,
    pub name: String,
    pub dynamic_libraries: Vec<String>,
    pub exported_symbols: HashMap<String, Guest64Addr>,
    pub bindings: Vec<Binding64>,
    pub entry_point_pc: Option<Guest64Addr>,
    pub text_base: Guest64Addr,
    pub last_segment_end: Guest64Addr,
    pub memory: Mem64,
    pub sections: Vec<Section64>,
}

fn command_bytes<'a>(bytes: &'a [u8], offset: u32, size: u32) -> Result<&'a [u8], String> {
    let start = usize::try_from(offset).map_err(|_| "ARM64 dyld info offset is too large")?;
    let length = usize::try_from(size).map_err(|_| "ARM64 dyld info size is too large")?;
    let end = start.checked_add(length).ok_or("ARM64 dyld info range overflows")?;
    bytes.get(start..end).ok_or_else(|| "ARM64 dyld info extends past the Mach-O file".to_string())
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, String> {
    let end = offset.checked_add(2).ok_or("ARM64 fixup offset overflows")?;
    let data = bytes.get(offset..end).ok_or("ARM64 fixups are truncated")?;
    Ok(u16::from_le_bytes([data[0], data[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, String> {
    let end = offset.checked_add(4).ok_or("ARM64 fixup offset overflows")?;
    let data = bytes.get(offset..end).ok_or("ARM64 fixups are truncated")?;
    Ok(u32::from_le_bytes([data[0], data[1], data[2], data[3]]))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, String> {
    let end = offset.checked_add(8).ok_or("ARM64 fixup offset overflows")?;
    let data = bytes.get(offset..end).ok_or("ARM64 fixups are truncated")?;
    Ok(u64::from_le_bytes(data.try_into().unwrap()))
}

fn read_cstr(bytes: &[u8], offset: usize) -> Result<String, String> {
    let data = bytes.get(offset..).ok_or("ARM64 fixup string offset is invalid")?;
    let end = data.iter().position(|&byte| byte == 0).ok_or("ARM64 fixup string is unterminated")?;
    String::from_utf8(data[..end].to_vec()).map_err(|_| "ARM64 fixup symbol is not UTF-8".to_string())
}

fn add_signed(base: u64, offset: i64) -> Result<u64, String> {
    if offset >= 0 {
        base.checked_add(offset as u64).ok_or("ARM64 address overflows".into())
    } else {
        base.checked_sub(offset.unsigned_abs()).ok_or("ARM64 address underflows".into())
    }
}

fn parse_chained_fixups(
    bytes: &[u8],
    data: LinkEditData,
    segment_bases: &[u64],
    memory: &mut Mem64,
    bindings: &mut Vec<Binding64>,
    slide: u64,
) -> Result<(), String> {
    let blob = command_bytes(bytes, data.off, data.size)?;
    if blob.len() < 28 {
        return Err("ARM64 chained fixups header is truncated".into());
    }
    let starts_offset = read_u32(blob, 4)? as usize;
    let imports_offset = read_u32(blob, 8)? as usize;
    let symbols_offset = read_u32(blob, 12)? as usize;
    let imports_count = read_u32(blob, 16)? as usize;
    let imports_format = read_u32(blob, 20)?;
    let starts = blob.get(starts_offset..).ok_or("ARM64 chained fixup starts offset is invalid")?;
    let segment_count = read_u32(starts, 0)? as usize;
    let segment_offsets_end = 4usize.checked_add(segment_count.checked_mul(4).ok_or("ARM64 chained fixup segment table overflows")?).ok_or("ARM64 chained fixup segment table overflows")?;
    if starts.len() < segment_offsets_end {
        return Err("ARM64 chained fixup segment table is truncated".into());
    }

    let mut imports = Vec::with_capacity(imports_count);
    let mut import_cursor = imports_offset;
    for _ in 0..imports_count {
        let (ordinal, name_offset, addend, entry_size) = match imports_format {
            1 => {
                let raw = read_u32(blob, import_cursor)?;
                ((raw & 0xff) as i32, (raw >> 9) as usize, 0i64, 4usize)
            }
            2 => {
                let raw = read_u32(blob, import_cursor)?;
                let addend = read_u32(blob, import_cursor + 4)? as i32 as i64;
                ((raw & 0xff) as i32, (raw >> 9) as usize, addend, 8usize)
            }
            3 => {
                let raw = read_u64(blob, import_cursor)?;
                let addend = read_u64(blob, import_cursor + 8)? as i64;
                (((raw & 0xffff) as i32), ((raw >> 17) & 0x7fff_ffff) as usize, addend, 16usize)
            }
            other => return Err(format!("unsupported ARM64 chained import format {other}")),
        };
        let name = read_cstr(blob, symbols_offset.checked_add(name_offset).ok_or("ARM64 fixup symbol offset overflows")?)?;
        imports.push((ordinal, name, addend));
        import_cursor = import_cursor.checked_add(entry_size).ok_or("ARM64 fixup import table overflows")?;
    }

    for segment_index in 0..segment_count {
        let offset = read_u32(starts, 4 + segment_index * 4)? as usize;
        if offset == 0 {
            continue;
        }
        let segment = starts.get(offset..).ok_or("ARM64 chained segment starts offset is invalid")?;
        let size = read_u32(segment, 0)? as usize;
        let page_size = read_u16(segment, 4)? as u64;
        let pointer_format = read_u16(segment, 6)?;
        let segment_offset = read_u64(segment, 8)?;
        let page_count = read_u16(segment, 20)? as usize;
        let page_starts_offset = 22usize;
        let page_starts_end = page_starts_offset.checked_add(page_count.checked_mul(2).ok_or("ARM64 chained page table overflows")?).ok_or("ARM64 chained page table overflows")?;
        if size < page_starts_end || segment.len() < page_starts_end {
            return Err("ARM64 chained segment starts is truncated".into());
        }
        if segment_index >= segment_bases.len() {
            return Err("ARM64 chained fixup references an invalid segment".into());
        }
        if pointer_format != 2 && pointer_format != 6 {
            return Err(format!("unsupported ARM64 chained pointer format {pointer_format}"));
        }
        let base = segment_bases[segment_index];
        for page in 0..page_count {
            let page_start = read_u16(segment, page_starts_offset + page * 2)?;
            if page_start == 0xffff {
                continue;
            }
            let mut chain_offset = segment_offset
                .checked_add(page as u64 * page_size)
                .and_then(|value| value.checked_add(page_start as u64))
                .ok_or("ARM64 chained pointer address overflows")?;
            let page_end = segment_offset
                .checked_add((page as u64 + 1) * page_size)
                .ok_or("ARM64 chained page end overflows")?;
            loop {
                let address = base.checked_add(chain_offset).ok_or("ARM64 chained pointer address overflows")?;
                let raw = memory.read_u64(address).map_err(str::to_owned)?;
                let bind = (raw >> 63) != 0;
                let next = (raw >> 51) & 0xfff;
                if bind {
                    let ordinal = (raw & 0x00ff_ffff) as usize;
                    let (_, symbol, import_addend) = imports.get(ordinal).ok_or("ARM64 chained bind ordinal is invalid")?;
                    let inline_addend = ((raw >> 24) & 0xff) as u8 as i8 as i64;
                    bindings.push(Binding64 { address, symbol: symbol.clone(), addend: *import_addend + inline_addend });
                } else {
                    let target = raw & 0x0000_000f_ffff_ffff;
                    let target = if pointer_format == 2 {
                        target | (((raw >> 36) & 0xff) << 56)
                    } else {
                        target
                    };
                    memory.load_u64(address, target.checked_add(slide).ok_or("ARM64 chained rebase overflows")?).map_err(str::to_owned)?;
                }
                if next == 0 {
                    break;
                }
                chain_offset = chain_offset.checked_add(next * 4).ok_or("ARM64 chained pointer chain overflows")?;
                if chain_offset >= page_end {
                    return Err("ARM64 chained pointer escapes its page".into());
                }
            }
        }
    }
    Ok(())
}

impl MachO64 {
    pub fn load_from_file<P: AsRef<crate::fs::GuestPath>>(
        path: P,
        fs: &crate::fs::Fs,
        slide: u64,
    ) -> Result<Self, String> {
        let name = path
            .as_ref()
            .file_name()
            .ok_or("64-bit executable has no file name")?
            .to_string();
        let bytes = fs.read(path.as_ref()).map_err(|_| "Could not read 64-bit executable file")?;
        Self::load_from_bytes(&bytes, name, slide)
    }

    pub fn load_from_bytes(bytes: &[u8], name: impl Into<String>, slide: u64) -> Result<Self, String> {
        let name = name.into();
        let mut cursor = Cursor::new(bytes);
        let file = OFile::parse(&mut cursor).map_err(|e| format!("could not parse ARM64 Mach-O: {e}"))?;
        let (image_bytes, file) = match file {
            OFile::FatFile { files, .. } => {
                let (arch, file) = files
                    .into_iter()
                    .find(|(arch, _)| arch.cputype == mach_object::CPU_TYPE_ARM64)
                    .ok_or("fat binary has no ARM64 slice")?;
                let start = usize::try_from(arch.offset).map_err(|_| "ARM64 fat slice offset is too large")?;
                let length = usize::try_from(arch.size).map_err(|_| "ARM64 fat slice size is too large")?;
                let end = start.checked_add(length).ok_or("ARM64 fat slice range overflows")?;
                (bytes.get(start..end).ok_or("ARM64 fat slice extends past the file")?, file)
            }
            file => (bytes, file),
        };
        let (header, commands) = match file {
            OFile::MachFile { header, commands } => (header, commands),
            _ => return Err("ARM64 input is not an executable Mach-O".into()),
        };
        if header.cputype != mach_object::CPU_TYPE_ARM64 || !header.is_64bit() {
            return Err("Mach-O is not an ARM64 64-bit image".into());
        }
        if header.is_bigend() {
            return Err("ARM64 Mach-O is big-endian".into());
        }

        let mut memory = Mem64::new();
        let mut dynamic_libraries = Vec::new();
        let mut exported_symbols = HashMap::new();
        let mut bindings = Vec::new();
        let mut text_base = None;
        let mut last_segment_end = 0;
        let mut entry_point_pc = None;
        let mut entry_point_offset = None;
        let mut symtab = None;
        let mut chained_fixups = None;
        let mut macho_sections = Vec::new();
        let mut sections = Vec::new();
        let mut segment_bases = Vec::new();

        for MachCommand(command, _) in commands {
            match command {
                LoadCommand::Segment64 {
                    segname,
                    vmaddr,
                    vmsize,
                    fileoff,
                    filesize,
                    maxprot,
                    initprot,
                    sections: segment_sections,
                    ..
                } => {
                    let base = (vmaddr as u64).checked_add(slide).ok_or("segment address overflows")?;
                    segment_bases.push(base);
                    last_segment_end = last_segment_end
                        .max(base.checked_add(vmsize as u64).ok_or("segment end overflows")?);
                    if segname == "__PAGEZERO" {
                        continue;
                    }
                    if segname == "__TEXT" {
                        text_base = Some(base);
                    }
                    if vmsize == 0 {
                        continue;
                    }
                    let permissions = Permissions::from_mach_protection(initprot, maxprot);
                    memory.map_zeroed_with_permissions(base, vmsize as u64, permissions)?;
                    if filesize != 0 {
                        let start = usize::try_from(fileoff).map_err(|_| "segment file offset is too large")?;
                        let length = usize::try_from(filesize).map_err(|_| "segment file size is too large")?;
                        let end = start.checked_add(length).ok_or("segment file range overflows")?;
                        let source = image_bytes.get(start..end).ok_or_else(|| format!("segment {segname} extends past the Mach-O file"))?;
                        memory.load_bytes(base, source)?;
                    }
                    for section in segment_sections {
                        sections.push(Section64 {
                            name: section.sectname.clone(),
                            address: (section.addr as u64).checked_add(slide).ok_or("section address overflows")?,
                            size: section.size as u64,
                        });
                        macho_sections.push(section);
                    }
                }
                LoadCommand::SymTab { symoff, nsyms, stroff, strsize } => {
                    symtab = Some((symoff, nsyms, stroff, strsize));
                }
                LoadCommand::DyldChainedFixups(data) => {
                    chained_fixups = Some(data);
                }
                LoadCommand::LoadDyLib(lib) => dynamic_libraries.push(lib.name.to_string()),
                LoadCommand::EncryptionInfo64 { id, .. } if id != 0 => {
                    return Err("ARM64 executable is encrypted".into());
                }
                LoadCommand::EntryPoint { entryoff, .. } => entry_point_offset = Some(entryoff),
                LoadCommand::UnixThread { state: ThreadState::Arm64 { __pc, .. }, .. } => {
                    entry_point_pc = Some(__pc.checked_add(slide).ok_or("entry point overflows")?);
                }
                LoadCommand::DyldInfo {
                    rebase_off,
                    rebase_size,
                    bind_off,
                    bind_size,
                    weak_bind_off,
                    weak_bind_size,
                    lazy_bind_off,
                    lazy_bind_size,
                    ..
                } => {
                    for rebased in Rebase::parse(command_bytes(image_bytes, rebase_off, rebase_size)?, 8) {
                        if rebased.symbol_type != BindSymbolType::Pointer {
                            continue;
                        }
                        let segment = *segment_bases
                            .get(rebased.segment_index)
                            .ok_or("ARM64 rebase references an invalid segment")?;
                        let address = segment
                            .checked_add(rebased.symbol_offset as u64)
                            .ok_or("ARM64 rebase address overflows")?;
                        let value = memory.read_u64(address)?;
                        memory.load_u64(address, value.checked_add(slide).ok_or("ARM64 rebase value overflows")?)?;
                    }
                    for bound in Bind::parse(command_bytes(image_bytes, bind_off, bind_size)?, 8) {
                        if bound.symbol_type != BindSymbolType::Pointer {
                            continue;
                        }
                        let segment = *segment_bases
                            .get(bound.segment_index)
                            .ok_or("ARM64 bind references an invalid segment")?;
                        let address = add_signed(segment, bound.symbol_offset as i64)?;
                        bindings.push(Binding64 { address, symbol: bound.name, addend: bound.addend as i64 });
                    }
                    for bound in WeakBind::parse(command_bytes(image_bytes, weak_bind_off, weak_bind_size)?, 8) {
                        if bound.symbol_type != BindSymbolType::Pointer {
                            continue;
                        }
                        let segment = *segment_bases
                            .get(bound.segment_index)
                            .ok_or("ARM64 weak bind references an invalid segment")?;
                        let address = add_signed(segment, bound.symbol_offset as i64)?;
                        bindings.push(Binding64 { address, symbol: bound.name, addend: bound.addend as i64 });
                    }
                    for bound in LazyBind::parse(command_bytes(image_bytes, lazy_bind_off, lazy_bind_size)?, 8) {
                        let segment = *segment_bases
                            .get(bound.segment_index)
                            .ok_or("ARM64 lazy bind references an invalid segment")?;
                        let address = add_signed(segment, bound.symbol_offset as i64)?;
                        bindings.push(Binding64 { address, symbol: bound.name, addend: 0 });
                    }
                }
                _ => {}
            }
        }

        if let Some(data) = chained_fixups {
            parse_chained_fixups(image_bytes, data, &segment_bases, &mut memory, &mut bindings, slide)?;
        }

        bindings.sort_by_key(|binding| binding.address);
        bindings.dedup_by(|left, right| {
            left.address == right.address
                && left.symbol == right.symbol
                && left.addend == right.addend
        });

        if let Some(entryoff) = entry_point_offset {
            entry_point_pc = Some(
                text_base
                    .ok_or("ARM64 LC_MAIN image has no __TEXT segment")?
                    .checked_add(entryoff)
                    .ok_or("entry point overflows")?,
            );
        }

        if let Some((symoff, nsyms, stroff, strsize)) = symtab {
            let mut symbols_cursor = Cursor::new(image_bytes);
            symbols_cursor
                .seek(SeekFrom::Start(symoff as u64))
                .map_err(|_| "invalid symbol table offset")?;
            for symbol in SymbolIter::new(&mut symbols_cursor, macho_sections, nsyms, stroff, strsize, false, true) {
                if let Symbol::Defined { name: Some(symbol_name), entry, .. } = symbol {
                    exported_symbols.insert(symbol_name.to_string(), entry as u64 + slide);
                }
            }
        }

        Ok(Self {
            architecture: Architecture::Arm64,
            name,
            dynamic_libraries,
            exported_symbols,
            bindings,
            entry_point_pc,
            text_base: text_base.unwrap_or(0),
            last_segment_end,
            memory,
            sections,
        })
    }
}
