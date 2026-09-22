//! Human-readable ELF analysis artifact generation.

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Output,
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

    let requested = requested_artifacts(config, source_elf)?;
    let temporary = requested
        .iter()
        .map(|artifact| artifact.temporary_path())
        .collect::<Vec<_>>();

    let result = generate_requested_artifacts(context, source_elf, &requested, &mut resolve_tool)
        .and_then(|contents| publish_artifacts(&requested, &temporary, contents));

    match result {
        Ok(registry) => Ok(registry),
        Err(error) => {
            remove_paths(
                temporary
                    .iter()
                    .chain(requested.iter().map(|artifact| &artifact.path)),
            );
            Err(error)
        }
    }
}

#[derive(Clone)]
struct RequestedArtifact {
    kind: DebugArtifactKind,
    path: PathBuf,
}

impl RequestedArtifact {
    fn temporary_path(&self) -> PathBuf {
        let mut name = OsString::from(".");
        name.push(
            self.path
                .file_name()
                .expect("analysis artifact has a file name"),
        );
        name.push(format!(".{}.tmp", std::process::id()));
        self.path.with_file_name(name)
    }
}

fn requested_artifacts(
    config: &AnalysisConfig,
    source_elf: &Path,
) -> anyhow::Result<Vec<RequestedArtifact>> {
    if source_elf.file_name().is_none() {
        bail!("invalid ELF file path: {}", source_elf.display());
    }

    let mut artifacts = Vec::new();
    if config.disassembly {
        artifacts.push(RequestedArtifact {
            kind: DebugArtifactKind::Disassembly,
            path: output_path(source_elf, "disassembly"),
        });
    }
    if config.elf_info {
        artifacts.push(RequestedArtifact {
            kind: DebugArtifactKind::ElfInfo,
            path: output_path(source_elf, "elf-info"),
        });
    }
    if config.symbols {
        artifacts.push(RequestedArtifact {
            kind: DebugArtifactKind::Symbols,
            path: output_path(source_elf, "symbols"),
        });
    }
    Ok(artifacts)
}

fn generate_requested_artifacts<F>(
    context: &ProcessContext,
    source_elf: &Path,
    requested: &[RequestedArtifact],
    resolve_tool: &mut F,
) -> anyhow::Result<Vec<Vec<u8>>>
where
    F: FnMut(&str) -> anyhow::Result<PathBuf>,
{
    let metadata = elf_metadata(source_elf)?;

    let mut contents = Vec::with_capacity(requested.len());
    for artifact in requested {
        let (tool_name, args) = match artifact.kind {
            DebugArtifactKind::Disassembly => ("llvm-objdump", vec!["--disassemble"]),
            DebugArtifactKind::ElfInfo => ("llvm-readobj", vec!["--all"]),
            DebugArtifactKind::Symbols => (
                "llvm-nm",
                vec!["--demangle", "--print-size", "--numeric-sort"],
            ),
        };
        let tool = resolve_tool(tool_name).with_context(|| {
            format!(
                "failed to resolve {tool_name} for ELF {}",
                source_elf.display()
            )
        })?;
        let stdout = run_tool(context, &tool, tool_name, &args, source_elf)?;

        if artifact.kind == DebugArtifactKind::ElfInfo {
            let mut elf_info = metadata_summary(&metadata);
            elf_info.extend_from_slice(b"\n\nllvm-readobj output:\n");
            elf_info.extend_from_slice(&stdout);
            contents.push(elf_info);
        } else {
            contents.push(stdout);
        }
    }
    Ok(contents)
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
    successful_stdout(tool_name, source_elf, output)
}

fn successful_stdout(
    tool_name: &str,
    source_elf: &Path,
    output: Output,
) -> anyhow::Result<Vec<u8>> {
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

fn publish_artifacts(
    requested: &[RequestedArtifact],
    temporary: &[PathBuf],
    contents: Vec<Vec<u8>>,
) -> anyhow::Result<DebugArtifactRegistry> {
    for (path, content) in temporary.iter().zip(contents) {
        fs::write(path, content)
            .with_context(|| format!("failed to write analysis artifact: {}", path.display()))?;
    }

    let mut registry = DebugArtifactRegistry::default();
    for (artifact, temporary) in requested.iter().zip(temporary) {
        if artifact.path.exists() {
            fs::remove_file(&artifact.path).with_context(|| {
                format!(
                    "failed to replace analysis artifact: {}",
                    artifact.path.display()
                )
            })?;
        }
        fs::rename(temporary, &artifact.path).with_context(|| {
            format!(
                "failed to publish analysis artifact: {}",
                artifact.path.display()
            )
        })?;
        registry.register(artifact.kind, artifact.path.clone());
    }
    Ok(registry)
}

fn remove_paths<'a>(paths: impl Iterator<Item = &'a PathBuf>) {
    for path in paths {
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
        artifact::{analysis::generate_with_tool_resolver, state::DebugArtifactKind},
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

    #[test]
    fn artifact_output_paths_append_full_source_filename() {
        let source = PathBuf::from("output with spaces/kernel.elf");

        assert_eq!(
            super::output_path(&source, "disassembly"),
            PathBuf::from("output with spaces/kernel.elf.disassembly")
        );
        assert_eq!(
            super::output_path(&source, "elf-info"),
            PathBuf::from("output with spaces/kernel.elf.elf-info")
        );
    }

    #[cfg(unix)]
    #[test]
    fn artifact_names_preserve_non_utf8_source_names() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let source = PathBuf::from(OsString::from_vec(b"kernel-\xff.elf".to_vec()));
        let expected = PathBuf::from(OsString::from_vec(b"kernel-\xff.elf.symbols".to_vec()));
        assert_eq!(super::output_path(&source, "symbols"), expected);
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
    fn successful_generation_overwrites_output_and_registers_it() {
        let temp = tempfile::tempdir().unwrap();
        let context = process_context(temp.path());
        let source = temp.path().join("kernel with spaces.elf");
        write_elf(&source);
        let output = super::output_path(&source, "disassembly");
        fs::write(&output, "stale").unwrap();
        let objdump = fake_tool(temp.path(), "objdump", "printf 'fresh disassembly'");
        let config = AnalysisConfig {
            disassembly: true,
            ..Default::default()
        };

        let registry =
            generate_with_tool_resolver(&context, &config, &source, |_| Ok(objdump.clone()))
                .unwrap();

        assert_eq!(fs::read_to_string(&output).unwrap(), "fresh disassembly");
        assert_eq!(
            registry.get(DebugArtifactKind::Disassembly),
            Some(output.as_path())
        );
    }

    #[cfg(unix)]
    #[test]
    fn invalid_elf_fails_before_any_tool_is_resolved() {
        let temp = tempfile::tempdir().unwrap();
        let context = process_context(temp.path());
        let source = temp.path().join("invalid.elf");
        fs::write(&source, "not an ELF").unwrap();
        for config in [
            AnalysisConfig {
                disassembly: true,
                ..Default::default()
            },
            AnalysisConfig {
                elf_info: true,
                ..Default::default()
            },
            AnalysisConfig {
                symbols: true,
                ..Default::default()
            },
        ] {
            let error = generate_with_tool_resolver(&context, &config, &source, |_| {
                panic!("invalid ELF must be rejected before tool lookup")
            })
            .unwrap_err();
            assert!(format!("{error:#}").contains("failed to parse ELF file"));
        }
        assert!(!super::output_path(&source, "elf-info").exists());
    }
}
