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

    #[derive(Clone, Copy)]
    enum Endian {
        Little,
        Big,
    }

    fn write_u16(data: &mut [u8], offset: usize, value: u16, endian: Endian) {
        let bytes = match endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };
        data[offset..offset + bytes.len()].copy_from_slice(&bytes);
    }

    fn write_u32(data: &mut [u8], offset: usize, value: u32, endian: Endian) {
        let bytes = match endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };
        data[offset..offset + bytes.len()].copy_from_slice(&bytes);
    }

    fn write_u64(data: &mut [u8], offset: usize, value: u64, endian: Endian) {
        let bytes = match endian {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };
        data[offset..offset + bytes.len()].copy_from_slice(&bytes);
    }

    fn elf64_with_symbol(symbol_defined: bool) -> Vec<u8> {
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

        write_u16(&mut data, 16, 2, Endian::Little);
        write_u16(&mut data, 18, 183, Endian::Little);
        write_u32(&mut data, 20, 1, Endian::Little);
        write_u64(&mut data, 24, 0x400080, Endian::Little);
        write_u64(&mut data, 32, PROGRAM_HEADER_OFFSET as u64, Endian::Little);
        write_u64(&mut data, 40, SECTION_HEADER_OFFSET as u64, Endian::Little);
        write_u16(&mut data, 52, 64, Endian::Little);
        write_u16(&mut data, 54, 56, Endian::Little);
        write_u16(&mut data, 56, 3, Endian::Little);
        write_u16(&mut data, 58, 64, Endian::Little);
        write_u16(&mut data, 60, 3, Endian::Little);
        write_u16(&mut data, 62, 1, Endian::Little);

        write_u32(&mut data, PROGRAM_HEADER_OFFSET, 1, Endian::Little);
        write_u32(&mut data, PROGRAM_HEADER_OFFSET + 4, 5, Endian::Little);
        write_u64(&mut data, PROGRAM_HEADER_OFFSET + 8, 0x100, Endian::Little);
        write_u64(
            &mut data,
            PROGRAM_HEADER_OFFSET + 16,
            0x400000,
            Endian::Little,
        );
        write_u64(
            &mut data,
            PROGRAM_HEADER_OFFSET + 24,
            0x500000,
            Endian::Little,
        );
        write_u64(&mut data, PROGRAM_HEADER_OFFSET + 32, 0x30, Endian::Little);
        write_u64(&mut data, PROGRAM_HEADER_OFFSET + 40, 0x40, Endian::Little);
        write_u64(
            &mut data,
            PROGRAM_HEADER_OFFSET + 48,
            0x1000,
            Endian::Little,
        );

        let second_program_header = PROGRAM_HEADER_OFFSET + 56;
        write_u32(&mut data, second_program_header, 1, Endian::Little);
        write_u32(&mut data, second_program_header + 4, 6, Endian::Little);
        write_u64(&mut data, second_program_header + 8, 0x200, Endian::Little);
        write_u64(
            &mut data,
            second_program_header + 16,
            0x401000,
            Endian::Little,
        );
        write_u64(
            &mut data,
            second_program_header + 24,
            0x501000,
            Endian::Little,
        );
        write_u64(&mut data, second_program_header + 32, 0x20, Endian::Little);
        write_u64(&mut data, second_program_header + 40, 0x30, Endian::Little);
        write_u64(
            &mut data,
            second_program_header + 48,
            0x1000,
            Endian::Little,
        );

        let note_program_header = second_program_header + 56;
        write_u32(&mut data, note_program_header, 4, Endian::Little);
        write_u32(&mut data, note_program_header + 4, 4, Endian::Little);
        write_u64(&mut data, note_program_header + 8, 0x280, Endian::Little);
        write_u64(
            &mut data,
            note_program_header + 16,
            0x402000,
            Endian::Little,
        );
        write_u64(
            &mut data,
            note_program_header + 24,
            0x502000,
            Endian::Little,
        );
        write_u64(&mut data, note_program_header + 32, 0x10, Endian::Little);
        write_u64(&mut data, note_program_header + 40, 0x10, Endian::Little);
        write_u64(&mut data, note_program_header + 48, 4, Endian::Little);

        data[STRING_TABLE_OFFSET..STRING_TABLE_OFFSET + STRING_TABLE.len()]
            .copy_from_slice(STRING_TABLE);

        let symbol = SYMBOL_TABLE_OFFSET + 24;
        write_u32(&mut data, symbol, 1, Endian::Little);
        data[symbol + 4] = 0x10;
        write_u16(
            &mut data,
            symbol + 6,
            u16::from(symbol_defined),
            Endian::Little,
        );
        write_u64(&mut data, symbol + 8, 0x400080, Endian::Little);

        let string_table_section = SECTION_HEADER_OFFSET + 64;
        write_u32(&mut data, string_table_section + 4, 3, Endian::Little);
        write_u64(
            &mut data,
            string_table_section + 24,
            STRING_TABLE_OFFSET as u64,
            Endian::Little,
        );
        write_u64(
            &mut data,
            string_table_section + 32,
            STRING_TABLE.len() as u64,
            Endian::Little,
        );
        write_u64(&mut data, string_table_section + 48, 1, Endian::Little);

        let symbol_table_section = SECTION_HEADER_OFFSET + 128;
        write_u32(&mut data, symbol_table_section + 4, 2, Endian::Little);
        write_u64(
            &mut data,
            symbol_table_section + 24,
            SYMBOL_TABLE_OFFSET as u64,
            Endian::Little,
        );
        write_u64(&mut data, symbol_table_section + 32, 48, Endian::Little);
        write_u32(&mut data, symbol_table_section + 40, 1, Endian::Little);
        write_u64(&mut data, symbol_table_section + 48, 8, Endian::Little);
        write_u64(&mut data, symbol_table_section + 56, 24, Endian::Little);

        data
    }

    fn elf32_big_endian_without_symbols() -> Vec<u8> {
        const PROGRAM_HEADER_OFFSET: usize = 52;

        let mut data = vec![0; 0xa0];
        data[..4].copy_from_slice(b"\x7fELF");
        data[4] = 1;
        data[5] = 2;
        data[6] = 1;

        write_u16(&mut data, 16, 2, Endian::Big);
        write_u16(&mut data, 18, 40, Endian::Big);
        write_u32(&mut data, 20, 1, Endian::Big);
        write_u32(&mut data, 24, 0x8004, Endian::Big);
        write_u32(&mut data, 28, PROGRAM_HEADER_OFFSET as u32, Endian::Big);
        write_u16(&mut data, 40, 52, Endian::Big);
        write_u16(&mut data, 42, 32, Endian::Big);
        write_u16(&mut data, 44, 1, Endian::Big);

        write_u32(&mut data, PROGRAM_HEADER_OFFSET, 1, Endian::Big);
        write_u32(&mut data, PROGRAM_HEADER_OFFSET + 4, 0x80, Endian::Big);
        write_u32(&mut data, PROGRAM_HEADER_OFFSET + 8, 0x8000, Endian::Big);
        write_u32(&mut data, PROGRAM_HEADER_OFFSET + 12, 0x9000, Endian::Big);
        write_u32(&mut data, PROGRAM_HEADER_OFFSET + 16, 0x10, Endian::Big);
        write_u32(&mut data, PROGRAM_HEADER_OFFSET + 20, 0x20, Endian::Big);
        write_u32(&mut data, PROGRAM_HEADER_OFFSET + 24, 6, Endian::Big);
        write_u32(&mut data, PROGRAM_HEADER_OFFSET + 28, 0x1000, Endian::Big);

        data
    }

    #[test]
    fn parses_elf64_load_segments_and_executable_start_symbol() {
        let metadata = ElfMetadata::parse(&elf64_with_symbol(true)).unwrap();

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
        assert_eq!(second.file_offset, 0x200);
        assert_eq!(second.file_size, 0x20);
        assert_eq!(second.memory_size, 0x30);
        assert_eq!(second.alignment, 0x1000);
        assert_eq!(second.flags, 6);
    }

    #[test]
    fn parses_big_endian_elf32_without_executable_start_symbol() {
        let metadata = ElfMetadata::parse(&elf32_big_endian_without_symbols()).unwrap();

        assert_eq!(metadata.arch, Architecture::Arm);
        assert_eq!(metadata.entry, 0x8004);
        assert_eq!(metadata.executable_start, None);
        assert_eq!(metadata.load_segments.len(), 1);

        let segment = &metadata.load_segments[0];
        assert_eq!(segment.virtual_address, 0x8000);
        assert_eq!(segment.physical_address, 0x9000);
        assert_eq!(segment.file_offset, 0x80);
        assert_eq!(segment.file_size, 0x10);
        assert_eq!(segment.memory_size, 0x20);
        assert_eq!(segment.alignment, 0x1000);
        assert_eq!(segment.flags, 6);
    }

    #[test]
    fn treats_an_undefined_executable_start_symbol_as_absent() {
        let metadata = ElfMetadata::parse(&elf64_with_symbol(false)).unwrap();

        assert_eq!(metadata.executable_start, None);
    }

    #[test]
    fn rejects_non_elf_input() {
        assert!(ElfMetadata::parse(b"not an ELF").is_err());
    }

    #[test]
    fn rejects_truncated_elf_header() {
        assert!(ElfMetadata::parse(b"\x7fELF\x02\x01\x01").is_err());
    }
}
