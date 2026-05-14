use std::path::PathBuf;

use anyhow::anyhow;
use cargo_metadata::Metadata;
use jkconfig::data::ElementHook;

use crate::{
    artifact::runtime::{
        PreparedRuntimeArtifacts, RuntimeArtifactOptions, objcopy_output_bin,
        prepare_cargo_build_outcome, prepare_custom_elf_artifact,
    },
    build::{
        artifact_selector::CargoBuildOutcome,
        config::{BuildConfig, BuildSystem, Cargo},
    },
    ctx::AppContext,
    invocation::Invocation,
    process::ProcessContext,
    project::{ProjectLayout, metadata, resolve_project_layout, variables::VariableScope},
};

/// Static configuration used to initialize a [`Tool`].
#[derive(Default, Clone, Debug)]
pub struct ToolConfig {
    /// Optional manifest path or manifest directory.
    pub manifest: Option<PathBuf>,
    /// Optional custom build output directory.
    pub build_dir: Option<PathBuf>,
    /// Optional custom binary output directory.
    pub bin_dir: Option<PathBuf>,
    /// Whether debug mode is enabled.
    pub debug: bool,
}

/// Main library object orchestrating build and run operations.
#[derive(Clone, Debug)]
pub struct Tool {
    pub(crate) config: ToolConfig,
    pub(crate) manifest_path: PathBuf,
    pub(crate) manifest_dir: PathBuf,
    pub(crate) workspace_dir: PathBuf,
    pub(crate) ctx: AppContext,
}

/// Resolved Cargo manifest and workspace paths derived from `cargo metadata`.
#[derive(Clone, Debug)]
pub struct ManifestContext {
    pub manifest_path: PathBuf,
    pub manifest_dir: PathBuf,
    pub workspace_dir: PathBuf,
}

impl From<ProjectLayout> for ManifestContext {
    fn from(layout: ProjectLayout) -> Self {
        Self {
            manifest_path: layout.manifest_path().to_path_buf(),
            manifest_dir: layout.manifest_dir().to_path_buf(),
            workspace_dir: layout.workspace_dir().to_path_buf(),
        }
    }
}

impl Tool {
    /// Creates a new tool from the provided configuration.
    pub fn new(config: ToolConfig) -> anyhow::Result<Self> {
        let layout = resolve_project_layout(config.manifest.clone())?;
        Ok(Self::from_project_layout(config, layout))
    }

    #[doc(hidden)]
    pub fn from_invocation(config: ToolConfig, invocation: Invocation) -> Self {
        Self::from_project_layout(config, invocation.into_project_layout())
    }

    #[doc(hidden)]
    pub fn from_project_layout(config: ToolConfig, layout: ProjectLayout) -> Self {
        Self {
            config,
            manifest_path: layout.manifest_path().to_path_buf(),
            manifest_dir: layout.manifest_dir().to_path_buf(),
            workspace_dir: layout.workspace_dir().to_path_buf(),
            ctx: AppContext::default(),
        }
    }

    pub fn ctx(&self) -> &AppContext {
        &self.ctx
    }

    pub fn ctx_mut(&mut self) -> &mut AppContext {
        &mut self.ctx
    }

    pub fn set_build_config_path(&mut self, path: Option<PathBuf>) {
        self.ctx.build_config_path = path;
    }

    pub fn into_context(self) -> AppContext {
        self.ctx
    }

    pub(crate) fn debug_enabled(&self) -> bool {
        self.config.debug
    }

    pub(crate) fn sync_cargo_context(&mut self, cargo: &Cargo) {
        self.ctx.build_config = Some(BuildConfig {
            system: BuildSystem::Cargo(cargo.clone()),
        });
    }

    pub(crate) fn manifest_dir(&self) -> &PathBuf {
        &self.manifest_dir
    }

    pub(crate) fn workspace_dir(&self) -> &PathBuf {
        &self.workspace_dir
    }

    pub(crate) fn build_dir(&self) -> PathBuf {
        self.config
            .build_dir
            .as_ref()
            .map(|dir| self.resolve_dir(dir))
            .unwrap_or_else(|| self.manifest_dir.join("target"))
    }

    pub(crate) fn bin_dir(&self) -> Option<PathBuf> {
        self.config
            .bin_dir
            .as_ref()
            .map(|dir| self.resolve_dir(dir))
    }

    fn resolve_dir(&self, dir: &PathBuf) -> PathBuf {
        if dir.is_relative() {
            self.manifest_dir.join(dir)
        } else {
            dir.clone()
        }
    }

    /// Executes a shell command in the current context.
    pub(crate) fn shell_run_cmd(&self, cmd: &str) -> anyhow::Result<()> {
        crate::process::shell_run_cmd(&self.process_context(), cmd)
    }

    /// Creates a new command builder for the given program.
    pub(crate) fn command(&self, program: &str) -> crate::utils::Command {
        crate::process::command(program, &self.process_context())
    }

    /// Gets the Cargo metadata for the current manifest.
    pub fn metadata(&self) -> anyhow::Result<Metadata> {
        metadata::cargo_metadata(&self.project_layout())
    }

    pub(crate) fn resolve_package_manifest_dir(&self, package: &str) -> anyhow::Result<PathBuf> {
        metadata::package_manifest_dir(&self.project_layout(), package)
    }

    /// Sets the ELF artifact path and synchronizes derived runtime metadata.
    #[cfg(test)]
    pub(crate) async fn set_elf_artifact_path(&mut self, path: PathBuf) -> anyhow::Result<()> {
        let prepared = crate::artifact::runtime::record_elf_artifact(path).await?;
        self.apply_prepared_runtime_artifacts(prepared);
        Ok(())
    }

    /// Imports an ELF artifact, strips it to a runtime `.elf`, and optionally
    /// materializes a `.bin` image.
    pub async fn prepare_elf_artifact(
        &mut self,
        path: PathBuf,
        to_bin: bool,
    ) -> anyhow::Result<()> {
        let prepared = prepare_custom_elf_artifact(
            path,
            to_bin,
            &self.runtime_artifact_options(),
            &self.process_context(),
        )
        .await?;
        self.apply_prepared_runtime_artifacts(prepared);
        Ok(())
    }

    /// Converts the ELF file to raw binary format.
    pub(crate) fn objcopy_output_bin(&mut self) -> anyhow::Result<PathBuf> {
        let mut prepared = self.prepared_runtime_artifacts_from_context()?;
        let bin_path = objcopy_output_bin(
            &mut prepared,
            &self.runtime_artifact_options(),
            &self.process_context(),
        )?;
        self.apply_prepared_runtime_artifacts(prepared);
        Ok(bin_path)
    }

    pub(crate) async fn apply_cargo_build_outcome(
        &mut self,
        outcome: &CargoBuildOutcome,
        to_bin: bool,
    ) -> anyhow::Result<()> {
        let prepared = prepare_cargo_build_outcome(
            outcome,
            to_bin,
            &self.runtime_artifact_options(),
            &self.process_context(),
        )
        .await?;
        self.apply_prepared_runtime_artifacts(prepared);
        Ok(())
    }

    pub(crate) fn resolve_build_config_path(&self, explicit_path: Option<PathBuf>) -> PathBuf {
        crate::build::config_loader::resolve_build_config_path(
            &self.project_layout(),
            explicit_path,
        )
    }

    /// Loads and prepares the build configuration.
    pub(crate) async fn prepare_build_config(
        &mut self,
        config_path: Option<PathBuf>,
        menu: bool,
    ) -> anyhow::Result<BuildConfig> {
        let loaded = crate::build::config_loader::load_build_config(
            &self.project_layout(),
            config_path,
            menu,
        )
        .await?;
        self.ctx.build_config_path = Some(loaded.path.clone());
        self.ctx.build_config = Some(loaded.config.clone());
        Ok(loaded.config)
    }

    pub(crate) fn replace_string(&self, input: &str) -> anyhow::Result<String> {
        crate::project::variables::expand_variables(input, &self.variable_scope())
    }

    pub(crate) fn replace_path_variables(&self, path: PathBuf) -> anyhow::Result<PathBuf> {
        crate::project::variables::expand_path_variables(path, &self.variable_scope())
    }

    fn package_root_for_variables(&self) -> anyhow::Result<PathBuf> {
        if let Some(BuildConfig {
            system: BuildSystem::Cargo(cargo),
        }) = &self.ctx.build_config
        {
            return self.resolve_package_manifest_dir(&cargo.package);
        }

        Ok(self.manifest_dir.clone())
    }

    fn project_layout(&self) -> ProjectLayout {
        ProjectLayout::from_manifest_parts(
            self.manifest_path.clone(),
            self.manifest_dir.clone(),
            self.workspace_dir.clone(),
        )
    }

    fn variable_scope(&self) -> VariableScope {
        let package_dir = self
            .package_root_for_variables()
            .unwrap_or_else(|_| self.manifest_dir.clone());
        VariableScope::for_package(&self.project_layout(), package_dir)
    }

    fn process_context(&self) -> ProcessContext {
        ProcessContext::new(
            self.manifest_dir.clone(),
            self.workspace_dir.clone(),
            self.variable_scope(),
            self.ctx.artifacts.elf.clone(),
        )
    }

    fn runtime_artifact_options(&self) -> RuntimeArtifactOptions {
        RuntimeArtifactOptions {
            bin_dir: self.bin_dir(),
            debug: self.debug_enabled(),
        }
    }

    fn prepared_runtime_artifacts_from_context(&self) -> anyhow::Result<PreparedRuntimeArtifacts> {
        Ok(PreparedRuntimeArtifacts::new(
            self.ctx.artifacts.clone(),
            self.ctx
                .arch
                .ok_or_else(|| anyhow!("architecture not detected"))?,
        ))
    }

    fn apply_prepared_runtime_artifacts(&mut self, prepared: PreparedRuntimeArtifacts) {
        self.ctx.arch = Some(prepared.arch());
        self.ctx.artifacts = prepared.artifacts().clone();
    }

    pub(crate) fn ui_hooks(&self) -> Vec<ElementHook> {
        crate::build::config_hooks::ui_hooks(&self.project_layout())
    }
}

pub fn resolve_manifest_context(input: Option<PathBuf>) -> anyhow::Result<ManifestContext> {
    resolve_project_layout(input).map(ManifestContext::from)
}

#[cfg(test)]
mod tests {
    use super::{Tool, ToolConfig, resolve_manifest_context};
    use crate::build::{
        config::{BuildConfig, BuildSystem, Cargo},
        config_hooks::{
            RustupTargetOption, TargetCandidateSet, build_target_options,
            collect_package_doc_targets, parse_rustup_targets,
        },
    };
    use crate::run::qemu::resolve_qemu_config_path_in_dir;
    use jkconfig::data::ElementHook;
    use object::Architecture;
    use std::{
        collections::HashMap,
        fs,
        path::{Path, PathBuf},
    };

    #[tokio::test]
    async fn set_elf_artifact_path_updates_dirs_and_arch() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "").unwrap();

        let source = std::env::current_exe().unwrap();
        let copied = temp.path().join("sample-elf");
        std::fs::copy(&source, &copied).unwrap();

        let mut tool = Tool::new(ToolConfig {
            manifest: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();
        tool.set_elf_artifact_path(copied.clone()).await.unwrap();

        let expected_elf = copied.canonicalize().unwrap();
        let expected_dir = expected_elf.parent().unwrap().to_path_buf();

        assert_eq!(tool.ctx.artifacts.elf.as_ref(), Some(&expected_elf));
        assert_eq!(
            tool.ctx.artifacts.cargo_artifact_dir.as_ref(),
            Some(&expected_dir)
        );
        assert_eq!(
            tool.ctx.artifacts.runtime_artifact_dir.as_ref(),
            Some(&expected_dir)
        );
        assert!(tool.ctx.arch.is_some());
        assert!(tool.ctx.artifacts.bin.is_none());
    }

    #[test]
    fn resolve_manifest_context_uses_workspace_root() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"app\"]\nresolver = \"3\"\n",
        )
        .unwrap();

        let app_dir = temp.path().join("app");
        std::fs::create_dir_all(app_dir.join("src")).unwrap();
        std::fs::write(
            app_dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(app_dir.join("src/main.rs"), "fn main() {}\n").unwrap();

        let manifest = resolve_manifest_context(Some(app_dir.clone())).unwrap();

        assert_eq!(manifest.manifest_path, app_dir.join("Cargo.toml"));
        assert_eq!(manifest.manifest_dir, app_dir);
        assert_eq!(manifest.workspace_dir, temp.path());
    }

    #[test]
    fn resolve_package_manifest_dir_uses_selected_package() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"app\", \"kernel\"]\nresolver = \"3\"\n",
        )
        .unwrap();

        let app_dir = temp.path().join("app");
        std::fs::create_dir_all(app_dir.join("src")).unwrap();
        std::fs::write(
            app_dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(app_dir.join("src/main.rs"), "fn main() {}\n").unwrap();

        let kernel_dir = temp.path().join("kernel");
        std::fs::create_dir_all(kernel_dir.join("src")).unwrap();
        std::fs::write(
            kernel_dir.join("Cargo.toml"),
            "[package]\nname = \"kernel\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(kernel_dir.join("src/main.rs"), "fn main() {}\n").unwrap();

        let tool = Tool::new(ToolConfig {
            manifest: Some(app_dir.clone()),
            ..Default::default()
        })
        .unwrap();

        let resolved = tool.resolve_package_manifest_dir("kernel").unwrap();
        assert_eq!(resolved, kernel_dir);
    }

    #[test]
    fn cargo_qemu_config_resolution_prefers_package_dir_over_workspace_root() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"app\", \"kernel\"]\nresolver = \"3\"\n",
        )
        .unwrap();
        std::fs::write(temp.path().join("qemu-aarch64.toml"), "").unwrap();

        let app_dir = temp.path().join("app");
        std::fs::create_dir_all(app_dir.join("src")).unwrap();
        std::fs::write(
            app_dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(app_dir.join("src/main.rs"), "fn main() {}\n").unwrap();

        let kernel_dir = temp.path().join("kernel");
        std::fs::create_dir_all(kernel_dir.join("src")).unwrap();
        std::fs::write(
            kernel_dir.join("Cargo.toml"),
            "[package]\nname = \"kernel\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(kernel_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(kernel_dir.join(".qemu-aarch64.toml"), "").unwrap();

        let tool = Tool::new(ToolConfig {
            manifest: Some(app_dir),
            ..Default::default()
        })
        .unwrap();

        let package_dir = tool.resolve_package_manifest_dir("kernel").unwrap();
        let resolved =
            resolve_qemu_config_path_in_dir(&package_dir, Some(Architecture::Aarch64), None)
                .unwrap();

        assert_eq!(resolved, kernel_dir.join(".qemu-aarch64.toml"));
    }

    #[test]
    fn replace_string_uses_workspace_and_legacy_workspacefolder() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "").unwrap();

        let tool = Tool::new(ToolConfig {
            manifest: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();

        let replaced = tool
            .replace_string("${workspace}:${workspaceFolder}")
            .unwrap();
        let expected = temp.path().display().to_string();
        assert_eq!(replaced, format!("{expected}:{expected}"));
    }

    #[test]
    fn replace_string_uses_cross_platform_tmpdir() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "").unwrap();

        let tool = Tool::new(ToolConfig {
            manifest: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();

        let replaced = tool.replace_string("${tmpDir}").unwrap();
        assert_eq!(replaced, std::env::temp_dir().display().to_string());
    }

    #[test]
    fn replace_string_uses_empty_string_for_missing_env() {
        let temp = tempfile::tempdir().unwrap();
        write_single_package(temp.path(), "sample");

        let tool = Tool::new(ToolConfig {
            manifest: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();

        let missing = format!(
            "__OSTOOL_TEST_ENV_SHOULD_NOT_EXIST_{}__",
            std::process::id()
        );

        let replaced = tool
            .replace_string(&format!("before-${{env:{missing}}}-after"))
            .unwrap();
        assert_eq!(replaced, "before--after");
    }

    #[test]
    fn replace_string_uses_package_dir_from_build_config() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"app\", \"kernel\"]\nresolver = \"3\"\n",
        )
        .unwrap();

        let app_dir = temp.path().join("app");
        std::fs::create_dir_all(app_dir.join("src")).unwrap();
        std::fs::write(
            app_dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(app_dir.join("src/main.rs"), "fn main() {}\n").unwrap();

        let kernel_dir = temp.path().join("kernel");
        std::fs::create_dir_all(kernel_dir.join("src")).unwrap();
        std::fs::write(
            kernel_dir.join("Cargo.toml"),
            "[package]\nname = \"kernel\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(kernel_dir.join("src/main.rs"), "fn main() {}\n").unwrap();

        let mut tool = Tool::new(ToolConfig {
            manifest: Some(app_dir),
            ..Default::default()
        })
        .unwrap();
        tool.ctx.build_config = Some(BuildConfig {
            system: BuildSystem::Cargo(Cargo {
                env: HashMap::new(),
                target: "aarch64-unknown-none".into(),
                package: "kernel".into(),
                bin: None,
                features: vec![],
                log: None,
                extra_config: None,
                profile: None,
                args: vec![],
                pre_build_cmds: vec![],
                post_build_cmds: vec![],
                to_bin: false,
            }),
        });

        let replaced = tool.replace_string("${package}").unwrap();
        assert_eq!(replaced, kernel_dir.display().to_string());
    }

    #[test]
    fn replace_string_falls_back_to_manifest_dir_for_package() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "").unwrap();

        let tool = Tool::new(ToolConfig {
            manifest: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();

        let replaced = tool.replace_string("${package}").unwrap();
        assert_eq!(replaced, temp.path().display().to_string());
    }

    #[test]
    fn command_replaces_args_and_env() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(temp.path().join("src")).unwrap();
        std::fs::write(temp.path().join("src/lib.rs"), "").unwrap();

        let tool = Tool::new(ToolConfig {
            manifest: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();

        let mut cmd = tool.command("echo");
        cmd.arg("${workspace}");
        cmd.env("PKG_DIR", "${package}");

        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, vec![temp.path().display().to_string()]);

        let envs: Vec<(String, String)> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        assert!(
            envs.iter()
                .any(|(k, v)| k == "PKG_DIR" && v == &temp.path().display().to_string())
        );
        assert!(
            envs.iter()
                .any(|(k, v)| k == "WORKSPACE_FOLDER" && v == &temp.path().display().to_string())
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shell_run_cmd_injects_kernel_elf_when_runtime_elf_exists() {
        let temp = tempfile::tempdir().unwrap();
        write_single_package(temp.path(), "sample");

        let source = std::env::current_exe().unwrap();
        let copied = temp.path().join("sample-elf");
        std::fs::copy(&source, &copied).unwrap();

        let mut tool = Tool::new(ToolConfig {
            manifest: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();
        tool.set_elf_artifact_path(copied.clone()).await.unwrap();

        let output = temp.path().join("kernel-env.txt");
        tool.shell_run_cmd(&format!(
            "printf '%s' \"$KERNEL_ELF\" > {}",
            output.display()
        ))
        .unwrap();

        assert_eq!(
            fs::read_to_string(output).unwrap(),
            copied.canonicalize().unwrap().display().to_string()
        );
    }

    #[test]
    fn collect_package_doc_targets_uses_targets_list() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = write_workspace_with_package(
            temp.path(),
            "kernel",
            Some(
                r#"[package.metadata.docs.rs]
targets = ["riscv64gc-unknown-none-elf", "aarch64-unknown-none"]
"#,
            ),
        );

        let targets = collect_package_doc_targets(&manifest, "kernel")
            .unwrap()
            .unwrap();
        assert_eq!(
            targets,
            vec![
                "riscv64gc-unknown-none-elf".to_string(),
                "aarch64-unknown-none".to_string()
            ]
        );
    }

    #[test]
    fn collect_package_doc_targets_uses_default_target_when_targets_missing() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = write_workspace_with_package(
            temp.path(),
            "kernel",
            Some(
                r#"[package.metadata.docs.rs]
default-target = "aarch64-unknown-none"
"#,
            ),
        );

        let targets = collect_package_doc_targets(&manifest, "kernel")
            .unwrap()
            .unwrap();
        assert_eq!(targets, vec!["aarch64-unknown-none".to_string()]);
    }

    #[test]
    fn collect_package_doc_targets_moves_default_target_to_front() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = write_workspace_with_package(
            temp.path(),
            "kernel",
            Some(
                r#"[package.metadata.docs.rs]
targets = ["x86_64-unknown-none", "aarch64-unknown-none", "x86_64-unknown-none"]
default-target = "aarch64-unknown-none"
"#,
            ),
        );

        let targets = collect_package_doc_targets(&manifest, "kernel")
            .unwrap()
            .unwrap();
        assert_eq!(
            targets,
            vec![
                "aarch64-unknown-none".to_string(),
                "x86_64-unknown-none".to_string()
            ]
        );
    }

    #[test]
    fn collect_package_doc_targets_rejects_invalid_docs_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = write_workspace_with_package(
            temp.path(),
            "kernel",
            Some(
                r#"[package.metadata.docs.rs]
targets = "aarch64-unknown-none"
"#,
            ),
        );

        let err = collect_package_doc_targets(&manifest, "kernel")
            .unwrap_err()
            .to_string();
        assert!(err.contains("targets"));
        assert!(err.contains("array of strings"));
    }

    #[test]
    fn collect_package_doc_targets_errors_for_missing_package() {
        let temp = tempfile::tempdir().unwrap();
        let manifest = write_workspace_with_package(temp.path(), "kernel", None);

        let err = collect_package_doc_targets(&manifest, "missing")
            .unwrap_err()
            .to_string();
        assert!(err.contains("package 'missing' not found"));
    }

    #[test]
    fn parse_rustup_targets_prioritizes_installed_entries() {
        let parsed = parse_rustup_targets(
            "aarch64-unknown-none\nx86_64-unknown-none (installed)\nriscv64gc-unknown-none-elf\nthumbv7em-none-eabihf (installed)\n",
        );

        let triples: Vec<_> = parsed.iter().map(|target| target.triple.as_str()).collect();
        let installed: Vec<_> = parsed.iter().map(|target| target.installed).collect();
        assert_eq!(
            triples,
            vec![
                "x86_64-unknown-none",
                "thumbv7em-none-eabihf",
                "aarch64-unknown-none",
                "riscv64gc-unknown-none-elf"
            ]
        );
        assert_eq!(installed, vec![true, true, false, false]);
    }

    #[test]
    fn parse_rustup_targets_handles_empty_output() {
        let parsed = parse_rustup_targets("");
        assert!(parsed.is_empty());
    }

    #[test]
    fn build_target_options_marks_rustup_install_state() {
        let options = build_target_options(TargetCandidateSet::Rustup(&[
            RustupTargetOption {
                triple: "x86_64-unknown-none".into(),
                installed: true,
            },
            RustupTargetOption {
                triple: "aarch64-unknown-none".into(),
                installed: false,
            },
        ]));
        assert_eq!(options[0].detail.as_deref(), Some("installed"));
        assert_eq!(options[1].detail.as_deref(), Some("available"));
    }

    #[test]
    fn ui_hooks_include_system_target_hook() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::write(temp.path().join("src/lib.rs"), "").unwrap();

        let tool = Tool::new(ToolConfig {
            manifest: Some(temp.path().to_path_buf()),
            ..Default::default()
        })
        .unwrap();

        let hooks: Vec<ElementHook> = tool.ui_hooks();
        assert!(
            hooks
                .iter()
                .any(|hook| hook.path.as_key() == "system.target")
        );
    }

    fn write_workspace_with_package(root: &Path, package: &str, metadata: Option<&str>) -> PathBuf {
        fs::write(
            root.join("Cargo.toml"),
            format!("[workspace]\nmembers = [\"{package}\"]\nresolver = \"3\"\n"),
        )
        .unwrap();

        let package_dir = root.join(package);
        fs::create_dir_all(package_dir.join("src")).unwrap();
        let mut cargo_toml =
            format!("[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n");
        if let Some(metadata) = metadata {
            cargo_toml.push('\n');
            cargo_toml.push_str(metadata);
        }
        fs::write(package_dir.join("Cargo.toml"), cargo_toml).unwrap();
        fs::write(package_dir.join("src/lib.rs"), "").unwrap();
        root.join("Cargo.toml")
    }

    fn write_single_package(root: &Path, package: &str) {
        fs::write(
            root.join("Cargo.toml"),
            format!("[package]\nname = \"{package}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n"),
        )
        .unwrap();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();
    }
}
