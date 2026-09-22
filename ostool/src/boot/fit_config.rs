//! Typed internal configuration and address resolution for FIT images.

use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::artifact::elf_metadata::{ElfMetadata, LoadSection, LoadSegment};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FitFormat {
    Elf,
    #[default]
    Bin,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FitOs {
    #[default]
    Linux,
    Elf,
}

impl FitOs {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Linux => "linux",
            Self::Elf => "elf",
        }
    }
}

/// An address supplied directly or derived from ELF metadata.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FitAddress {
    #[default]
    Auto,
    Explicit(u64),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct FitFdt {
    pub(crate) path: PathBuf,
    pub(crate) load: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FitConfig {
    pub(crate) format: FitFormat,
    pub(crate) os: FitOs,
    pub(crate) load: FitAddress,
    pub(crate) entry: FitAddress,
    pub(crate) fdt: Option<FitFdt>,
    pub(crate) output: Option<PathBuf>,
}

impl Default for FitConfig {
    fn default() -> Self {
        Self {
            format: FitFormat::Bin,
            os: FitOs::Linux,
            load: FitAddress::Auto,
            entry: FitAddress::Auto,
            fdt: None,
            output: None,
        }
    }
}

impl FitConfig {
    /// Resolves FIT addresses from the original, unstripped ELF metadata.
    pub(crate) fn resolve_addresses(&self, metadata: &ElfMetadata) -> Result<(u64, u64)> {
        let load = match self.load {
            FitAddress::Explicit(address) => address,
            FitAddress::Auto => match metadata.executable_start {
                // The linker-provided origin describes the selected project's image base.
                Some(address) => address,
                None => match self.format {
                    FitFormat::Elf => elf_load_base(metadata)?,
                    FitFormat::Bin => bin_load_base(metadata)?,
                },
            },
        };
        let entry = match self.entry {
            FitAddress::Explicit(address) => address,
            FitAddress::Auto => metadata.entry,
        };
        Ok((load, entry))
    }
}

fn elf_load_base(metadata: &ElfMetadata) -> Result<u64> {
    let mut bases = metadata
        .load_segments
        .iter()
        .filter(|segment| segment.file_size != 0)
        .map(elf_container_base);
    let first = bases.next().ok_or_else(|| {
        anyhow!("cannot derive ELF FIT load address: no file-backed PT_LOAD segment")
    })??;
    for base in bases {
        let base = base?;
        if base != first {
            bail!(
                "cannot derive ELF FIT load address: PT_LOAD segments have different file bases; set load explicitly"
            );
        }
    }
    Ok(first)
}

fn elf_container_base(segment: &LoadSegment) -> Result<u64> {
    valid_segment_range(segment)?;
    segment.physical_address.checked_sub(segment.file_offset).ok_or_else(|| {
        anyhow!(
            "cannot derive ELF FIT load address: PT_LOAD physical address {:#x} precedes file offset {:#x}; set load explicitly",
            segment.physical_address,
            segment.file_offset
        )
    })
}

fn bin_load_base(metadata: &ElfMetadata) -> Result<u64> {
    metadata
        .load_sections
        .iter()
        .map(|section| section_lma(section, &metadata.load_segments))
        .try_fold(None::<u64>, |minimum, address| {
            let address = address?;
            Ok::<_, anyhow::Error>(Some(minimum.map_or(address, |current| current.min(address))))
        })?
        .ok_or_else(|| {
            anyhow!(
                "cannot derive BIN FIT load address: no file-backed allocated section; set load explicitly"
            )
        })
}

fn section_lma(section: &LoadSection, segments: &[LoadSegment]) -> Result<u64> {
    let section_file_end = checked_end(section.file_offset, section.size, "section file range")?;
    let section_virtual_end = checked_end(
        section.virtual_address,
        section.size,
        "section virtual range",
    )?;
    let mut candidates = Vec::new();

    for segment in segments.iter().filter(|segment| segment.file_size != 0) {
        valid_segment_range(segment)?;
        let segment_file_end =
            checked_end(segment.file_offset, segment.file_size, "PT_LOAD file range")?;
        let segment_virtual_end = checked_end(
            segment.virtual_address,
            segment.file_size,
            "PT_LOAD virtual range",
        )?;
        if section.file_offset < segment.file_offset
            || section_file_end > segment_file_end
            || section.virtual_address < segment.virtual_address
            || section_virtual_end > segment_virtual_end
        {
            continue;
        }

        let file_delta = section.file_offset - segment.file_offset;
        let virtual_delta = section.virtual_address - segment.virtual_address;
        if file_delta != virtual_delta {
            continue;
        }
        candidates.push(
            segment
                .physical_address
                .checked_add(file_delta)
                .ok_or_else(|| {
                    anyhow!("cannot derive BIN FIT load address: PT_LOAD LMA overflow")
                })?,
        );
    }

    let first = candidates.first().copied().ok_or_else(|| {
        anyhow!(
            "cannot derive BIN FIT load address: an allocated file-backed section has no matching PT_LOAD segment; set load explicitly"
        )
    })?;
    if candidates.iter().any(|candidate| *candidate != first) {
        bail!(
            "cannot derive BIN FIT load address: an allocated file-backed section has ambiguous PT_LOAD LMA mappings; set load explicitly"
        );
    }
    Ok(first)
}

fn valid_segment_range(segment: &LoadSegment) -> Result<()> {
    checked_end(segment.file_offset, segment.file_size, "PT_LOAD file range")?;
    checked_end(
        segment.virtual_address,
        segment.file_size,
        "PT_LOAD virtual range",
    )?;
    checked_end(
        segment.physical_address,
        segment.file_size,
        "PT_LOAD physical range",
    )?;
    Ok(())
}

fn checked_end(start: u64, size: u64, kind: &str) -> Result<u64> {
    start
        .checked_add(size)
        .ok_or_else(|| anyhow!("cannot derive FIT load address: {kind} overflows"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{FitAddress, FitConfig, FitFdt, FitFormat, FitOs};
    use crate::artifact::elf_metadata::{ElfMetadata, LoadSection, LoadSegment};
    use object::Architecture;

    fn segment(vaddr: u64, paddr: u64, offset: u64, file_size: u64) -> LoadSegment {
        LoadSegment {
            virtual_address: vaddr,
            physical_address: paddr,
            file_offset: offset,
            file_size,
            memory_size: file_size,
            alignment: 1,
            flags: 0,
        }
    }

    fn section(vaddr: u64, offset: u64, size: u64) -> LoadSection {
        LoadSection {
            virtual_address: vaddr,
            file_offset: offset,
            size,
        }
    }

    fn metadata(
        entry: u64,
        executable_start: Option<u64>,
        load_segments: Vec<LoadSegment>,
        load_sections: Vec<LoadSection>,
    ) -> ElfMetadata {
        ElfMetadata {
            arch: Architecture::Riscv64,
            entry,
            load_segments,
            load_sections,
            executable_start,
        }
    }

    #[test]
    fn resolves_symbol_and_entry_zero_and_honors_explicit_addresses() {
        let metadata = metadata(0, Some(0), Vec::new(), Vec::new());
        assert_eq!(
            FitConfig::default().resolve_addresses(&metadata).unwrap(),
            (0, 0)
        );

        let config = FitConfig {
            load: FitAddress::Explicit(0x1000),
            entry: FitAddress::Explicit(0x2000),
            ..FitConfig::default()
        };
        assert_eq!(
            config.resolve_addresses(&metadata).unwrap(),
            (0x1000, 0x2000)
        );
    }

    #[test]
    fn elf_auto_load_requires_one_common_container_base() {
        let metadata = metadata(
            0x8010,
            None,
            vec![
                segment(0x8000, 0x8000, 0, 0x1000),
                segment(0x9000, 0x9000, 0x1000, 0x200),
            ],
            Vec::new(),
        );
        let config = FitConfig {
            format: FitFormat::Elf,
            ..FitConfig::default()
        };

        assert_eq!(
            config.resolve_addresses(&metadata).unwrap(),
            (0x8000, 0x8010)
        );
    }

    #[test]
    fn elf_auto_load_rejects_different_or_overflowing_segment_ranges() {
        let config = FitConfig {
            format: FitFormat::Elf,
            ..FitConfig::default()
        };
        let different = metadata(
            0,
            None,
            vec![segment(0x8000, 0x8000, 0, 4), segment(0x9000, 0x9000, 4, 4)],
            Vec::new(),
        );
        assert!(config.resolve_addresses(&different).is_err());

        let overflowing = metadata(
            0,
            None,
            vec![segment(u64::MAX - 1, u64::MAX - 1, 0, 2)],
            Vec::new(),
        );
        assert!(config.resolve_addresses(&overflowing).is_err());
    }

    #[test]
    fn bin_auto_load_starts_at_first_output_section_not_pt_load_header() {
        let metadata = metadata(
            0,
            None,
            vec![segment(0, 0x8000_0000, 0, 0x200)],
            vec![section(0x100, 0x100, 0x20)],
        );

        assert_eq!(
            FitConfig::default().resolve_addresses(&metadata).unwrap(),
            (0x8000_0100, 0)
        );
    }

    #[test]
    fn bin_auto_load_keeps_zero_and_rejects_bss_only_or_ambiguous_mappings() {
        let zero = metadata(0, None, vec![segment(0, 0, 0, 4)], vec![section(0, 0, 4)]);
        assert_eq!(
            FitConfig::default().resolve_addresses(&zero).unwrap(),
            (0, 0)
        );

        let bss_only = metadata(0, None, vec![segment(0, 0, 0, 4)], Vec::new());
        assert!(FitConfig::default().resolve_addresses(&bss_only).is_err());

        let ambiguous = metadata(
            0,
            None,
            vec![segment(0, 0x1000, 0, 4), segment(0, 0x2000, 0, 4)],
            vec![section(0, 0, 4)],
        );
        assert!(FitConfig::default().resolve_addresses(&ambiguous).is_err());
    }

    #[test]
    fn config_round_trips_json_and_toml_with_explicit_zero_and_high_addresses() {
        let config = FitConfig {
            format: FitFormat::Elf,
            os: FitOs::Elf,
            load: FitAddress::Explicit(0),
            entry: FitAddress::Explicit(0x9000_0000_8000_0000),
            fdt: Some(FitFdt {
                path: PathBuf::from("board.dtb"),
                load: Some(0),
            }),
            output: Some(PathBuf::from("boot.fit")),
        };

        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(serde_json::from_str::<FitConfig>(&json).unwrap(), config);

        let toml = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<FitConfig>(&toml).unwrap(), config);
    }

    #[test]
    fn omitted_addresses_default_to_auto_and_fdt_load_remains_optional() {
        let config: FitConfig = toml::from_str("[fdt]\npath = 'board.dtb'").unwrap();
        assert_eq!(config.load, FitAddress::Auto);
        assert_eq!(config.entry, FitAddress::Auto);
        assert_eq!(config.fdt.as_ref().unwrap().load, None);
        let encoded = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<FitConfig>(&encoded).unwrap(), config);
    }
}
