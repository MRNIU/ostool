use anyhow::{Context, bail};
use object::{
    Object, ObjectSegment, ObjectSymbol,
    read::elf::{ElfFile, FileHeader, ProgramHeader},
};

pub(crate) struct ElfMetadata {
    pub(crate) arch: object::Architecture,
    pub(crate) entry: u64,
    pub(crate) load_segments: Vec<LoadSegment>,
    pub(crate) executable_start: Option<u64>,
}

pub(crate) struct LoadSegment {
    pub(crate) virtual_address: u64,
    pub(crate) physical_address: u64,
    pub(crate) file_offset: u64,
    pub(crate) file_size: u64,
    pub(crate) memory_size: u64,
    pub(crate) alignment: u64,
    pub(crate) flags: u32,
}

impl ElfMetadata {
    pub(crate) fn parse(data: &[u8]) -> anyhow::Result<Self> {
        let file = object::File::parse(data).context("failed to parse ELF")?;

        match file {
            object::File::Elf32(file) => Self::from_elf(&file),
            object::File::Elf64(file) => Self::from_elf(&file),
            _ => bail!("expected an ELF file"),
        }
    }

    fn from_elf<Elf>(file: &ElfFile<'_, Elf>) -> anyhow::Result<Self>
    where
        Elf: FileHeader,
    {
        let endian = file.endian();
        let load_segments = file
            .segments()
            .map(|segment| {
                let header = segment.elf_program_header();
                LoadSegment {
                    virtual_address: segment.address(),
                    physical_address: header.p_paddr(endian).into(),
                    file_offset: header.p_offset(endian).into(),
                    file_size: header.p_filesz(endian).into(),
                    memory_size: header.p_memsz(endian).into(),
                    alignment: header.p_align(endian).into(),
                    flags: header.p_flags(endian),
                }
            })
            .collect();

        let mut executable_start = None;
        for symbol in file.symbols() {
            if symbol.name()? == "__executable_start" && symbol.is_definition() {
                executable_start = Some(symbol.address());
                break;
            }
        }

        Ok(Self {
            arch: file.architecture(),
            entry: file.entry(),
            load_segments,
            executable_start,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use object::Architecture;

    fn write_u16(data: &mut [u8], offset: usize, value: u16) {
        let bytes = value.to_le_bytes();
        data[offset..offset + bytes.len()].copy_from_slice(&bytes);
    }

    fn write_u32(data: &mut [u8], offset: usize, value: u32) {
        let bytes = value.to_le_bytes();
        data[offset..offset + bytes.len()].copy_from_slice(&bytes);
    }

    fn write_u64(data: &mut [u8], offset: usize, value: u64) {
        let bytes = value.to_le_bytes();
        data[offset..offset + bytes.len()].copy_from_slice(&bytes);
    }

    fn elf64(symbol_defined: Option<bool>) -> Vec<u8> {
        const PROGRAM_HEADER_OFFSET: usize = 64;
        const STRING_TABLE_OFFSET: usize = 0x180;
        const SYMBOL_TABLE_OFFSET: usize = 0x1c0;
        const SECTION_HEADER_OFFSET: usize = 0x200;
        const STRING_TABLE: &[u8] = b"\0__executable_start\0";

        let mut data = vec![0; 0x300];
        data[..4].copy_from_slice(b"\x7fELF");
        data[4] = 2;
        data[5] = 1;
        data[6] = 1;

        write_u16(&mut data, 16, 2);
        write_u16(&mut data, 18, 183);
        write_u32(&mut data, 20, 1);
        write_u64(&mut data, 24, 0x400080);
        write_u64(&mut data, 32, PROGRAM_HEADER_OFFSET as u64);
        write_u16(&mut data, 52, 64);
        write_u16(&mut data, 54, 56);
        write_u16(&mut data, 56, 3);

        write_u32(&mut data, PROGRAM_HEADER_OFFSET, 1);
        write_u32(&mut data, PROGRAM_HEADER_OFFSET + 4, 5);
        write_u64(&mut data, PROGRAM_HEADER_OFFSET + 8, 0x100);
        write_u64(&mut data, PROGRAM_HEADER_OFFSET + 16, 0x400000);
        write_u64(&mut data, PROGRAM_HEADER_OFFSET + 24, 0x500000);
        write_u64(&mut data, PROGRAM_HEADER_OFFSET + 32, 0x30);
        write_u64(&mut data, PROGRAM_HEADER_OFFSET + 40, 0x40);
        write_u64(&mut data, PROGRAM_HEADER_OFFSET + 48, 0x1000);

        let second_program_header = PROGRAM_HEADER_OFFSET + 56;
        write_u32(&mut data, second_program_header, 1);
        write_u32(&mut data, second_program_header + 4, 6);
        write_u64(&mut data, second_program_header + 8, 0x200);
        write_u64(&mut data, second_program_header + 16, 0x401000);
        write_u64(&mut data, second_program_header + 24, 0x501000);
        write_u64(&mut data, second_program_header + 32, 0x20);
        write_u64(&mut data, second_program_header + 40, 0x30);
        write_u64(&mut data, second_program_header + 48, 0x1000);

        // An empty PT_NOTE must not appear among the load segments.
        write_u32(&mut data, PROGRAM_HEADER_OFFSET + 112, 4);

        if let Some(symbol_defined) = symbol_defined {
            write_u64(&mut data, 40, SECTION_HEADER_OFFSET as u64);
            write_u16(&mut data, 58, 64);
            write_u16(&mut data, 60, 3);
            write_u16(&mut data, 62, 1);
            data[STRING_TABLE_OFFSET..STRING_TABLE_OFFSET + STRING_TABLE.len()]
                .copy_from_slice(STRING_TABLE);

            let symbol = SYMBOL_TABLE_OFFSET + 24;
            write_u32(&mut data, symbol, 1);
            data[symbol + 4] = 0x10;
            write_u16(&mut data, symbol + 6, u16::from(symbol_defined));
            write_u64(&mut data, symbol + 8, 0x400080);

            let string_table_section = SECTION_HEADER_OFFSET + 64;
            write_u32(&mut data, string_table_section + 4, 3);
            write_u64(
                &mut data,
                string_table_section + 24,
                STRING_TABLE_OFFSET as u64,
            );
            write_u64(
                &mut data,
                string_table_section + 32,
                STRING_TABLE.len() as u64,
            );
            write_u64(&mut data, string_table_section + 48, 1);

            let symbol_table_section = SECTION_HEADER_OFFSET + 128;
            write_u32(&mut data, symbol_table_section + 4, 2);
            write_u64(
                &mut data,
                symbol_table_section + 24,
                SYMBOL_TABLE_OFFSET as u64,
            );
            write_u64(&mut data, symbol_table_section + 32, 48);
            write_u32(&mut data, symbol_table_section + 40, 1);
            write_u64(&mut data, symbol_table_section + 48, 8);
            write_u64(&mut data, symbol_table_section + 56, 24);
        }

        data
    }

    #[test]
    fn parses_elf64_load_segments_and_executable_start_symbol() {
        let metadata = ElfMetadata::parse(&elf64(Some(true))).unwrap();

        assert_eq!(metadata.arch, Architecture::Aarch64);
        assert_eq!(metadata.entry, 0x400080);
        assert_eq!(metadata.executable_start, Some(0x400080));
        assert_eq!(metadata.load_segments.len(), 2);

        let first = &metadata.load_segments[0];
        assert_eq!(first.virtual_address, 0x400000);
        assert_eq!(first.physical_address, 0x500000);
        assert_eq!(first.file_offset, 0x100);
        assert_eq!(first.file_size, 0x30);
        assert_eq!(first.memory_size, 0x40);
        assert_eq!(first.alignment, 0x1000);
        assert_eq!(first.flags, 5);

        let second = &metadata.load_segments[1];
        assert_eq!(second.virtual_address, 0x401000);
        assert_eq!(second.physical_address, 0x501000);
    }

    #[test]
    fn treats_missing_or_undefined_executable_start_symbol_as_absent() {
        assert_eq!(
            ElfMetadata::parse(&elf64(None)).unwrap().executable_start,
            None
        );
        assert_eq!(
            ElfMetadata::parse(&elf64(Some(false)))
                .unwrap()
                .executable_start,
            None
        );
    }
}
