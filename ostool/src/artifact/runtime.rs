//! Runtime artifact preparation and conversion for built or supplied ELF files.

use std::path::PathBuf;

use anyhow::{Context, anyhow};
use colored::Colorize;
use object::{Architecture, Object};
use tokio::fs;

use crate::{
    artifact::state::OutputArtifacts, build::artifact_selector::CargoBuildOutcome,
    process::ProcessContext, utils::PathResultExt,
};

#[derive(Clone, Debug)]
pub(crate) struct RuntimeArtifactOptions {
    pub(crate) bin_dir: Option<PathBuf>,
    pub(crate) debug: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedRuntimeArtifacts {
    artifacts: OutputArtifacts,
    arch: Architecture,
}

impl PreparedRuntimeArtifacts {
    /// Creates a prepared runtime artifact bundle with its detected architecture.
    pub(crate) fn new(artifacts: OutputArtifacts, arch: Architecture) -> Self {
        Self { artifacts, arch }
    }

    /// Returns the output artifact paths associated with this prepared bundle.
    pub(crate) fn artifacts(&self) -> &OutputArtifacts {
        &self.artifacts
    }

    /// Returns the architecture detected from the prepared ELF file.
    pub(crate) fn arch(&self) -> Architecture {
        self.arch
    }
}

/// Records an existing ELF path without running conversion tools.
pub(crate) async fn record_elf_artifact(path: PathBuf) -> anyhow::Result<PreparedRuntimeArtifacts> {
    let path = path
        .canonicalize()
        .with_path("failed to canonicalize file", &path)?;
    let artifact_dir = path
        .parent()
        .ok_or_else(|| anyhow!("invalid ELF file path: {}", path.display()))?
        .to_path_buf();
    let arch = detect_architecture(&path).await?;

    Ok(PreparedRuntimeArtifacts::new(
        OutputArtifacts {
            elf: Some(path),
            bin: None,
            cargo_artifact_dir: Some(artifact_dir.clone()),
            runtime_artifact_dir: Some(artifact_dir),
        },
        arch,
    ))
}

/// Records Cargo JSON build output as the active runtime artifact state.
pub(crate) async fn record_cargo_build_outcome(
    outcome: &CargoBuildOutcome,
) -> anyhow::Result<PreparedRuntimeArtifacts> {
    let mut prepared = record_elf_artifact(outcome.artifact().elf_path.clone()).await?;
    prepared.artifacts.cargo_artifact_dir = Some(outcome.artifact().cargo_artifact_dir.clone());
    prepared.artifacts.runtime_artifact_dir = Some(outcome.artifact().cargo_artifact_dir.clone());
    Ok(prepared)
}

/// Prepares a Cargo build outcome and optionally converts its ELF to a BIN file.
pub(crate) async fn prepare_cargo_build_outcome(
    outcome: &CargoBuildOutcome,
    to_bin: bool,
    options: &RuntimeArtifactOptions,
    process: &ProcessContext,
) -> anyhow::Result<PreparedRuntimeArtifacts> {
    let mut prepared = record_cargo_build_outcome(outcome).await?;
    if to_bin {
        objcopy_output_bin(&mut prepared, options, process)?;
    }
    Ok(prepared)
}

/// Prepares a user-provided ELF file, including strip and optional BIN conversion.
pub(crate) async fn prepare_custom_elf_artifact(
    path: PathBuf,
    to_bin: bool,
    options: &RuntimeArtifactOptions,
    process: &ProcessContext,
) -> anyhow::Result<PreparedRuntimeArtifacts> {
    let mut prepared = record_elf_artifact(path).await?;
    objcopy_elf(&mut prepared, process)?;
    if to_bin {
        objcopy_output_bin(&mut prepared, options, process)?;
    }
    Ok(prepared)
}

/// Produces the stripped ELF artifact used by custom runtime flows.
pub(crate) fn objcopy_elf(
    prepared: &mut PreparedRuntimeArtifacts,
    process: &ProcessContext,
) -> anyhow::Result<PathBuf> {
    let elf_path = prepared
        .artifacts
        .elf
        .as_ref()
        .ok_or_else(|| anyhow!("elf not exist"))?;
    let elf_path = elf_path
        .canonicalize()
        .with_path("failed to canonicalize file", elf_path)?;

    let stripped_elf_path = elf_path.with_file_name(
        elf_path
            .file_stem()
            .ok_or_else(|| anyhow!("invalid ELF file path: {}", elf_path.display()))?
            .to_string_lossy()
            .to_string()
            + ".elf",
    );
    println!(
        "{}",
        format!(
            "Stripping ELF file...\r\n  original elf: {}\r\n  stripped elf: {}",
            elf_path.display(),
            stripped_elf_path.display()
        )
        .bold()
        .purple()
    );

    let mut objcopy = crate::process::command("rust-objcopy", process);
    objcopy.arg(format!(
        "--binary-architecture={}",
        format!("{:?}", prepared.arch()).to_lowercase()
    ));
    objcopy.arg(&elf_path);
    objcopy.arg(&stripped_elf_path);
    objcopy.run()?;

    prepared.artifacts.elf = Some(stripped_elf_path.clone());
    prepared.artifacts.bin = None;
    prepared.artifacts.cargo_artifact_dir = stripped_elf_path.parent().map(PathBuf::from);
    prepared.artifacts.runtime_artifact_dir = stripped_elf_path.parent().map(PathBuf::from);

    Ok(stripped_elf_path)
}

/// Converts the prepared ELF artifact to a binary image when a runner needs one.
pub(crate) fn objcopy_output_bin(
    prepared: &mut PreparedRuntimeArtifacts,
    options: &RuntimeArtifactOptions,
    process: &ProcessContext,
) -> anyhow::Result<PathBuf> {
    if let Some(bin) = &prepared.artifacts.bin {
        debug!("BIN file already exists: {:?}", bin);
        return Ok(bin.clone());
    }

    let elf_path = prepared
        .artifacts
        .elf
        .as_ref()
        .ok_or_else(|| anyhow!("elf not exist"))?;
    let elf_path = elf_path
        .canonicalize()
        .with_path("failed to canonicalize file", elf_path)?;

    let bin_name = elf_path
        .file_stem()
        .ok_or_else(|| anyhow!("invalid ELF file path: {}", elf_path.display()))?
        .to_string_lossy()
        .to_string()
        + ".bin";

    let bin_path = if let Some(bin_dir) = &options.bin_dir {
        bin_dir.join(bin_name)
    } else {
        elf_path.with_file_name(bin_name)
    };

    if let Some(parent) = bin_path.parent() {
        std::fs::create_dir_all(parent).with_path("failed to create directory", parent)?;
    }

    println!(
        "{}",
        format!(
            "Converting ELF to BIN format...\r\n  elf: {}\r\n  bin: {}",
            elf_path.display(),
            bin_path.display()
        )
        .bold()
        .purple()
    );

    let mut objcopy = crate::process::command("rust-objcopy", process);

    if !options.debug {
        objcopy.arg("--strip-all");
    }

    objcopy
        .arg("-O")
        .arg("binary")
        .arg(&elf_path)
        .arg(&bin_path);
    objcopy.run()?;

    prepared.artifacts.bin = Some(bin_path.clone());
    prepared.artifacts.runtime_artifact_dir = bin_path.parent().map(PathBuf::from);
    Ok(bin_path)
}

/// Reads an ELF file and returns the architecture reported by the object parser.
async fn detect_architecture(path: &PathBuf) -> anyhow::Result<Architecture> {
    let binary_data = fs::read(path)
        .await
        .with_path("failed to read ELF file", path)?;
    let file = object::File::parse(binary_data.as_slice())
        .with_context(|| format!("failed to parse ELF file: {}", path.display()))?;
    Ok(file.architecture())
}
