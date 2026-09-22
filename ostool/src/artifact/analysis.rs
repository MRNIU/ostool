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
    generate_with_tool_resolver(context, config, source_elf, llvm_tools::llvm_tool)
}

fn generate_with_tool_resolver<F>(
    context: &ProcessContext,
    config: &AnalysisConfig,
    source_elf: &Path,
    mut resolve_tool: F,
) -> anyhow::Result<DebugArtifactRegistry>
where
    F: FnMut(&str) -> anyhow::Result<PathBuf>,
{
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
            let tool = resolve_tool(tool_name).with_context(|| {
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

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    use crate::{
        artifact::analysis::generate_with_tool_resolver,
        build::config::AnalysisConfig,
        process::ProcessContext,
        project::{resolve_project_layout, variables::VariableScope},
    };
    use anyhow::anyhow;

    fn process_context(root: &std::path::Path) -> ProcessContext {
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();

        let layout = resolve_project_layout(Some(root.to_path_buf())).unwrap();
        let scope = VariableScope::for_package(&layout, root.to_path_buf());
        ProcessContext::new(root.to_path_buf(), root.to_path_buf(), scope, None)
    }

    fn write_elf(path: &Path) {
        // Minimal ELF64 with no sections or program headers.
        let mut elf = [0u8; 64];
        elf[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        elf[16] = 2; // ET_EXEC
        elf[18] = 62; // EM_X86_64
        elf[20] = 1;
        elf[52] = 64;
        fs::write(path, elf).unwrap();
    }

    #[test]
    fn disabled_analysis_does_not_read_the_elf_or_lookup_tools() {
        let temp = tempfile::tempdir().unwrap();
        let context = process_context(temp.path());
        let missing_source = temp.path().join("missing kernel.elf");

        let registry = generate_with_tool_resolver(
            &context,
            &AnalysisConfig::default(),
            &missing_source,
            |_| panic!("disabled analysis must not resolve any tools"),
        )
        .unwrap();

        assert!(registry.is_empty());
    }

    #[cfg(unix)]
    fn make_executable(path: &Path) {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(unix)]
    fn fake_tool(root: &Path, name: &str, body: &str) -> PathBuf {
        let path = root.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        make_executable(&path);
        path
    }

    #[cfg(unix)]
    #[test]
    fn missing_tool_clears_stale_requested_output() {
        let temp = tempfile::tempdir().unwrap();
        let context = process_context(temp.path());
        let source = temp.path().join("kernel with spaces.elf");
        write_elf(&source);
        let stale = super::output_path(&source, "symbols");
        fs::write(&stale, "stale").unwrap();
        let config = AnalysisConfig {
            symbols: true,
            ..Default::default()
        };

        let error = generate_with_tool_resolver(&context, &config, &source, |_| {
            Err(anyhow!("llvm-tools unavailable"))
        })
        .unwrap_err();

        assert!(error.to_string().contains("failed to resolve llvm-nm"));
        assert!(!stale.exists());
    }

    #[cfg(unix)]
    #[test]
    fn tool_failure_reports_source_status_and_stderr_without_partial_outputs() {
        let temp = tempfile::tempdir().unwrap();
        let context = process_context(temp.path());
        let source = temp.path().join("kernel with spaces.elf");
        write_elf(&source);
        let disassembly = super::output_path(&source, "disassembly");
        let symbols = super::output_path(&source, "symbols");
        fs::write(&disassembly, "stale disassembly").unwrap();
        fs::write(&symbols, "stale symbols").unwrap();
        let objdump = fake_tool(temp.path(), "objdump", "printf disassembly");
        let nm = fake_tool(temp.path(), "nm", "printf 'broken symbols' >&2\nexit 7");
        let config = AnalysisConfig {
            disassembly: true,
            symbols: true,
            ..Default::default()
        };

        let error = generate_with_tool_resolver(&context, &config, &source, |tool| match tool {
            "llvm-objdump" => Ok(objdump.clone()),
            "llvm-nm" => Ok(nm.clone()),
            unexpected => Err(anyhow!("unexpected tool: {unexpected}")),
        })
        .unwrap_err();
        let message = format!("{error:#}");

        assert!(message.contains("llvm-nm"));
        assert!(message.contains(&source.display().to_string()));
        assert!(message.contains("exit status: 7"));
        assert!(message.contains("broken symbols"));
        assert!(!disassembly.exists());
        assert!(!symbols.exists());
    }

    #[cfg(unix)]
    #[test]
    fn invalid_elf_fails_before_any_tool_is_resolved() {
        let temp = tempfile::tempdir().unwrap();
        let context = process_context(temp.path());
        let source = temp.path().join("invalid.elf");
        fs::write(&source, "not an ELF").unwrap();
        let config = AnalysisConfig {
            symbols: true,
            ..Default::default()
        };
        let error = generate_with_tool_resolver(&context, &config, &source, |_| {
            panic!("invalid ELF must be rejected before tool lookup")
        })
        .unwrap_err();
        assert!(format!("{error:#}").contains("failed to parse ELF file"));
        assert!(!super::output_path(&source, "symbols").exists());
    }
}
