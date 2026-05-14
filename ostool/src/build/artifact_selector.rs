use std::path::PathBuf;

use anyhow::{anyhow, bail};

#[derive(Debug, Clone)]
pub(crate) struct ResolvedCargoArtifact {
    pub(crate) elf_path: PathBuf,
    pub(crate) cargo_artifact_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct CargoBuildOutcome {
    artifact: ResolvedCargoArtifact,
}

impl CargoBuildOutcome {
    pub(crate) fn new(artifact: ResolvedCargoArtifact) -> Self {
        Self { artifact }
    }

    pub(crate) fn artifact(&self) -> &ResolvedCargoArtifact {
        &self.artifact
    }
}

pub(crate) fn select_executable_artifact(
    executable_artifacts: &[(String, ResolvedCargoArtifact)],
    explicit_bin: Option<&str>,
    default_run: Option<&str>,
    package: &str,
) -> anyhow::Result<ResolvedCargoArtifact> {
    if let Some(bin) = explicit_bin {
        return executable_artifacts
            .iter()
            .rev()
            .find(|(name, _)| name == bin)
            .map(|(_, artifact)| artifact.clone())
            .ok_or_else(|| {
                anyhow!(
                    "binary target `{bin}` was not built for package `{package}`; check system.Cargo.bin or --bin"
                )
            });
    }

    if executable_artifacts.is_empty() {
        bail!(
            "no executable bin artifact found in cargo JSON output for package `{package}`; ostool currently resolves only Cargo bin targets"
        );
    }

    if let Some((_, artifact)) = executable_artifacts
        .iter()
        .rev()
        .find(|(name, _)| name == package)
    {
        return Ok(artifact.clone());
    }

    if let Some(default_bin) = default_run
        && let Some((_, artifact)) = executable_artifacts
            .iter()
            .rev()
            .find(|(name, _)| name == default_bin)
    {
        return Ok(artifact.clone());
    }

    if executable_artifacts.len() == 1 {
        return Ok(executable_artifacts[0].1.clone());
    }

    let bins = executable_artifacts
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    bail!(
        "package `{package}` has multiple binary targets ({bins}); pass system.Cargo.bin or --bin"
    )
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{ResolvedCargoArtifact, select_executable_artifact};

    fn artifact(name: &str) -> ResolvedCargoArtifact {
        let cargo_artifact_dir = PathBuf::from("/tmp/ostool-target/debug");
        ResolvedCargoArtifact {
            elf_path: cargo_artifact_dir.join(name),
            cargo_artifact_dir,
        }
    }

    fn select(
        artifacts: &[(String, ResolvedCargoArtifact)],
        explicit_bin: Option<&str>,
        default_run: Option<&str>,
        package: &str,
    ) -> anyhow::Result<ResolvedCargoArtifact> {
        select_executable_artifact(artifacts, explicit_bin, default_run, package)
    }

    #[test]
    fn select_executable_artifact_uses_explicit_bin_first() {
        let artifacts = vec![
            ("kernel".to_string(), artifact("kernel")),
            ("kernel-qemu".to_string(), artifact("kernel-qemu")),
        ];

        let selected = select(&artifacts, Some("kernel-qemu"), None, "kernel").unwrap();

        assert_eq!(
            selected.elf_path,
            Path::new("/tmp/ostool-target/debug/kernel-qemu")
        );
    }

    #[test]
    fn select_executable_artifact_errors_when_explicit_bin_was_not_built() {
        let artifacts = vec![("kernel".to_string(), artifact("kernel"))];

        let err = select(&artifacts, Some("missing-bin"), None, "kernel").unwrap_err();

        assert!(
            err.to_string()
                .contains("binary target `missing-bin` was not built")
        );
    }

    #[test]
    fn select_executable_artifact_prefers_package_name_before_default_run() {
        let artifacts = vec![
            ("helper".to_string(), artifact("helper")),
            ("kernel".to_string(), artifact("kernel")),
        ];

        let selected = select(&artifacts, None, Some("helper"), "kernel").unwrap();

        assert_eq!(
            selected.elf_path,
            Path::new("/tmp/ostool-target/debug/kernel")
        );
    }

    #[test]
    fn select_executable_artifact_uses_default_run_without_package_name_binary() {
        let artifacts = vec![
            ("helper".to_string(), artifact("helper")),
            ("boot-test".to_string(), artifact("boot-test")),
        ];

        let selected = select(&artifacts, None, Some("boot-test"), "kernel").unwrap();

        assert_eq!(
            selected.elf_path,
            Path::new("/tmp/ostool-target/debug/boot-test")
        );
    }

    #[test]
    fn select_executable_artifact_uses_single_binary_as_fallback() {
        let artifacts = vec![("helper".to_string(), artifact("helper"))];

        let selected = select(&artifacts, None, None, "kernel").unwrap();

        assert_eq!(
            selected.elf_path,
            Path::new("/tmp/ostool-target/debug/helper")
        );
    }

    #[test]
    fn select_executable_artifact_errors_on_empty_cargo_output() {
        let err = select(&[], None, None, "kernel").unwrap_err();

        assert!(err.to_string().contains("no executable bin artifact found"));
    }

    #[test]
    fn select_executable_artifact_errors_on_ambiguous_multiple_binaries() {
        let artifacts = vec![
            ("kernel-qemu".to_string(), artifact("kernel-qemu")),
            ("kernel-uboot".to_string(), artifact("kernel-uboot")),
        ];

        let err = select(&artifacts, None, None, "kernel").unwrap_err();

        let rendered = err.to_string();
        assert!(rendered.contains("multiple binary targets"));
        assert!(rendered.contains("kernel-qemu"));
        assert!(rendered.contains("kernel-uboot"));
    }
}
