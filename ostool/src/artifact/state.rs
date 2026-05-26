//! Runtime artifact state shared by build orchestration and runners.

use std::path::{Path, PathBuf};

use anyhow::anyhow;

use crate::artifact::runtime::PreparedRuntimeArtifacts;

/// Build artifacts generated during the build process.
#[derive(Default, Clone, Debug)]
pub struct OutputArtifacts {
    /// Path to the built ELF file.
    elf: Option<PathBuf>,
    /// Path to the converted binary file.
    bin: Option<PathBuf>,
    /// Cargo-reported directory containing the original ELF artifact.
    cargo_artifact_dir: Option<PathBuf>,
    /// Directory containing the runtime artifact consumed by runners.
    runtime_artifact_dir: Option<PathBuf>,
}

impl OutputArtifacts {
    pub fn elf(&self) -> Option<&Path> {
        self.elf.as_deref()
    }

    pub fn bin(&self) -> Option<&Path> {
        self.bin.as_deref()
    }

    pub fn cargo_artifact_dir(&self) -> Option<&Path> {
        self.cargo_artifact_dir.as_deref()
    }

    pub fn runtime_artifact_dir(&self) -> Option<&Path> {
        self.runtime_artifact_dir.as_deref()
    }

    pub(crate) fn runtime_image(&self) -> Option<&Path> {
        self.bin().or_else(|| self.elf())
    }

    pub(crate) fn require_bin(&self, message: &'static str) -> anyhow::Result<&Path> {
        self.bin().ok_or_else(|| anyhow!(message))
    }

    #[cfg(test)]
    pub(crate) fn set_runtime_artifact_dir(&mut self, path: PathBuf) {
        self.runtime_artifact_dir = Some(path);
    }

    pub(crate) fn apply_prepared_runtime_artifacts(&mut self, prepared: &PreparedRuntimeArtifacts) {
        self.elf = Some(prepared.elf().to_path_buf());
        self.bin = prepared.bin().map(PathBuf::from);
        self.cargo_artifact_dir = prepared.cargo_artifact_dir().map(PathBuf::from);
        self.runtime_artifact_dir = prepared.runtime_artifact_dir().map(PathBuf::from);
    }
}
