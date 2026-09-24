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
