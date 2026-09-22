//! Human-readable ELF analysis artifact generation.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, anyhow, bail};

use crate::{
    artifact::{
        elf_metadata::ElfMetadata,
        llvm_tools,
        state::{DebugArtifactKind, DebugArtifactRegistry},
    },
    build::config::AnalysisConfig,
    process::ProcessContext,
};

/// Generates every requested analysis artifact beside `source_elf`.
///
/// Outputs are staged in sibling paths and registered only after all succeed.
/// A failure removes both temporary paths and any stale requested analysis outputs.
pub(crate) fn generate(
    context: &ProcessContext,
    config: &AnalysisConfig,
    source_elf: &Path,
) -> anyhow::Result<DebugArtifactRegistry> {
    if !config.is_enabled() {
        return Ok(DebugArtifactRegistry::default());
    }

    if source_elf.file_name().is_none() {
        bail!("invalid ELF file path: {}", source_elf.display());
    }
    let requested: Vec<_> = [
        (
            config.disassembly,
            DebugArtifactKind::Disassembly,
            "disassembly",
            "llvm-objdump",
            &["--disassemble"][..],
        ),
        (
            config.elf_info,
            DebugArtifactKind::ElfInfo,
            "elf-info",
            "llvm-readobj",
            &["--all"][..],
        ),
        (
            config.symbols,
            DebugArtifactKind::Symbols,
            "symbols",
            "llvm-nm",
            &["--demangle", "--print-size", "--numeric-sort"][..],
        ),
    ]
    .into_iter()
    .filter(|(enabled, ..)| *enabled)
    .map(|(_, kind, suffix, tool, args)| (kind, output_path(source_elf, suffix), tool, args))
    .collect();

    let result = (|| {
        let metadata = elf_metadata(source_elf)?;
        let mut staged = Vec::new();
        for (kind, path, tool_name, args) in &requested {
            let tool = llvm_tools::llvm_tool(tool_name).with_context(|| {
                format!(
                    "failed to resolve {tool_name} for ELF {}",
                    source_elf.display()
                )
            })?;
            let mut contents = run_tool(context, &tool, tool_name, args, source_elf)?;
            let mut temporary = tempfile::NamedTempFile::new_in(path.parent().unwrap())
                .with_context(|| {
                    format!("failed to stage analysis artifact: {}", path.display())
                })?;
            if *kind == DebugArtifactKind::ElfInfo {
                let mut summary = metadata_summary(&metadata);
                summary.extend_from_slice(b"\n\nllvm-readobj output:\n");
                summary.extend_from_slice(&contents);
                contents = summary;
            }
            temporary.write_all(&contents).with_context(|| {
                format!("failed to write analysis artifact: {}", path.display())
            })?;
            staged.push((kind, path, temporary));
        }

        let mut registry = DebugArtifactRegistry::default();
        for (kind, path, temporary) in staged {
            temporary
                .persist(path)
                .map_err(|error| error.error)
                .with_context(|| {
                    format!("failed to publish analysis artifact: {}", path.display())
                })?;
            registry.register(*kind, path.clone());
        }
        Ok(registry)
    })();
    if result.is_err() {
        // Tempfiles clean themselves up; discard stale or partially published outputs too.
        for (_, path, _, _) in &requested {
            if let Err(error) = fs::remove_file(path)
                && error.kind() != std::io::ErrorKind::NotFound
            {
                log::warn!(
                    "failed to remove analysis artifact {}: {error}",
                    path.display()
                );
            }
        }
    }
    result
}

fn elf_metadata(source_elf: &Path) -> anyhow::Result<ElfMetadata> {
    let bytes = fs::read(source_elf)
        .with_context(|| format!("failed to read ELF file: {}", source_elf.display()))?;
    ElfMetadata::parse(&bytes)
        .with_context(|| format!("failed to parse ELF file: {}", source_elf.display()))
}

fn run_tool(
    context: &ProcessContext,
    program: &Path,
    tool_name: &str,
    args: &[&str],
    source_elf: &Path,
) -> anyhow::Result<Vec<u8>> {
    let mut command = crate::process::command(program, context).into_std();
    command.args(args).arg(source_elf);
    let output = command.output().with_context(|| {
        format!(
            "failed to execute {tool_name} for ELF {}",
            source_elf.display()
        )
    })?;
    if output.status.success() {
        return Ok(output.stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(anyhow!(
        "{tool_name} failed for ELF {} with status {}: {}",
        source_elf.display(),
        output.status,
        stderr.trim()
    ))
}

fn metadata_summary(metadata: &ElfMetadata) -> Vec<u8> {
    let mut summary = format!(
        "ELF metadata\narchitecture: {:?}\nentry: 0x{:x}\nload segments:\n",
        metadata.arch, metadata.entry
    );
    if metadata.load_segments.is_empty() {
        summary.push_str("  absent\n");
    } else {
        for segment in &metadata.load_segments {
            summary.push_str(&format!(
                "  virtual_address: 0x{:x}\n  physical_address: 0x{:x}\n  file_offset: 0x{:x}\n  file_size: 0x{:x}\n  memory_size: 0x{:x}\n  alignment: 0x{:x}\n  flags: 0x{:x}\n",
                segment.virtual_address,
                segment.physical_address,
                segment.file_offset,
                segment.file_size,
                segment.memory_size,
                segment.alignment,
                segment.flags,
            ));
        }
    }
    match metadata.executable_start {
        Some(address) => summary.push_str(&format!("executable_start: 0x{address:x}\n")),
        None => summary.push_str("executable_start: absent\n"),
    }
    summary.into_bytes()
}

fn output_path(source_elf: &Path, suffix: &str) -> PathBuf {
    let mut name = source_elf.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    source_elf.with_file_name(name)
}
