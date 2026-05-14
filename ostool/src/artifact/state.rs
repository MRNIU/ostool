//! Shared artifact path state produced while an ostool invocation runs.

use std::path::PathBuf;

/// Build and runtime artifacts produced during one invocation.
#[derive(Default, Clone, Debug)]
pub struct OutputArtifacts {
    /// Path to the built ELF file.
    pub elf: Option<PathBuf>,
    /// Path to the converted binary file.
    pub bin: Option<PathBuf>,
    /// Cargo-reported directory containing the original ELF artifact.
    pub cargo_artifact_dir: Option<PathBuf>,
    /// Directory containing the runtime artifact consumed by runners.
    pub runtime_artifact_dir: Option<PathBuf>,
}
