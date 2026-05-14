//! Invocation state and compatibility helpers for one ostool CLI or library run.

use std::path::{Path, PathBuf};

use anyhow::anyhow;
use cargo_metadata::Metadata;
use jkconfig::data::ElementHook;
use object::Architecture;

use crate::{
    artifact::{
        runtime::{
            PreparedRuntimeArtifacts, RuntimeArtifactOptions, objcopy_output_bin,
            prepare_cargo_build_outcome, prepare_custom_elf_artifact,
        },
        state::OutputArtifacts,
    },
    build::{
        artifact_selector::CargoBuildOutcome,
        config::{BuildConfig, BuildSystem, Cargo},
    },
    process::ProcessContext,
    project::{ProjectLayout, metadata, variables::VariableScope},
};

/// Static inputs for one CLI or library invocation.
#[derive(Clone, Debug, Default)]
pub struct InvocationOptions {
    manifest: Option<PathBuf>,
    build_dir: Option<PathBuf>,
    bin_dir: Option<PathBuf>,
    debug: bool,
}

impl InvocationOptions {
    /// Creates immutable invocation options from CLI or library inputs.
    pub fn new(
        manifest: Option<PathBuf>,
        build_dir: Option<PathBuf>,
        bin_dir: Option<PathBuf>,
        debug: bool,
    ) -> Self {
        Self {
            manifest,
            build_dir,
            bin_dir,
            debug,
        }
    }

    /// Returns the optional Cargo manifest path supplied by the caller.
    pub fn manifest(&self) -> Option<&Path> {
        self.manifest.as_deref()
    }

    /// Returns the optional build output directory supplied by the caller.
    pub fn build_dir(&self) -> Option<&Path> {
        self.build_dir.as_deref()
    }

    /// Returns the optional BIN output directory supplied by the caller.
    pub fn bin_dir(&self) -> Option<&Path> {
        self.bin_dir.as_deref()
    }

    /// Returns whether debug-mode runtime artifacts should be preserved.
    pub fn debug(&self) -> bool {
        self.debug
    }
}

/// Runtime state accumulated while one invocation runs.
#[derive(Default, Clone, Debug)]
pub(crate) struct RuntimeContext {
    /// Detected CPU architecture from the ELF file.
    pub(crate) arch: Option<Architecture>,
    /// Current build configuration.
    pub(crate) build_config: Option<BuildConfig>,
    /// Path to the build configuration file.
    pub(crate) build_config_path: Option<PathBuf>,
    /// Generated build artifacts.
    pub(crate) artifacts: OutputArtifacts,
}

/// Mutable state accumulated while an invocation runs.
#[derive(Clone, Debug, Default)]
pub(crate) struct InvocationState {
    runtime_context: RuntimeContext,
    active_build: Option<ActiveBuildContext>,
}

impl InvocationState {
    /// Returns the runtime context accumulated by the active invocation.
    pub(crate) fn runtime_context(&self) -> &RuntimeContext {
        &self.runtime_context
    }

    /// Returns mutable runtime context for build and runner stages.
    pub(crate) fn runtime_context_mut(&mut self) -> &mut RuntimeContext {
        &mut self.runtime_context
    }

    /// Returns the active build context captured from the current build config.
    pub(crate) fn active_build(&self) -> Option<&ActiveBuildContext> {
        self.active_build.as_ref()
    }

    /// Replaces the active build context after config loading or sync.
    pub(crate) fn set_active_build(&mut self, active_build: Option<ActiveBuildContext>) {
        self.active_build = active_build;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ActiveBuildContext {
    Cargo(ActiveCargoBuild),
    Custom(ActiveCustomBuild),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveCargoBuild {
    package: String,
    bin: Option<String>,
    target: String,
}

impl ActiveCargoBuild {
    /// Creates active Cargo build metadata for package-scoped path expansion.
    pub fn new(package: String, bin: Option<String>, target: String) -> Self {
        Self {
            package,
            bin,
            target,
        }
    }

    /// Returns the Cargo package name associated with the active build.
    pub fn package(&self) -> &str {
        &self.package
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveCustomBuild {
    elf_path: PathBuf,
    to_bin: bool,
}

impl ActiveCustomBuild {
    /// Creates active custom-build metadata for an explicit ELF path.
    pub fn new(elf_path: PathBuf, to_bin: bool) -> Self {
        Self { elf_path, to_bin }
    }
}

/// Top-level immutable layout plus mutable invocation state.
#[derive(Clone, Debug)]
pub struct Invocation {
    options: InvocationOptions,
    project_layout: ProjectLayout,
    state: InvocationState,
}

impl Invocation {
    /// Resolves project layout and creates a fresh invocation state.
    pub fn new(options: InvocationOptions) -> anyhow::Result<Self> {
        let project_layout =
            crate::project::resolve_project_layout(options.manifest().map(PathBuf::from))?;
        Ok(Self {
            options,
            project_layout,
            state: InvocationState::default(),
        })
    }

    /// Returns immutable options for this invocation.
    pub fn options(&self) -> &InvocationOptions {
        &self.options
    }

    /// Returns resolved Cargo manifest and workspace paths.
    pub fn project_layout(&self) -> &ProjectLayout {
        &self.project_layout
    }

    /// Returns the mutable runtime state wrapper.
    pub(crate) fn state(&self) -> &InvocationState {
        &self.state
    }

    /// Returns mutable access to the runtime state wrapper.
    pub(crate) fn state_mut(&mut self) -> &mut InvocationState {
        &mut self.state
    }

    /// Consumes the invocation and returns its resolved project layout.
    pub fn into_project_layout(self) -> ProjectLayout {
        self.project_layout
    }

    /// Returns the runtime context for compatibility with the old `ctx` call sites.
    pub(crate) fn ctx(&self) -> &RuntimeContext {
        self.state.runtime_context()
    }

    /// Returns mutable runtime context for compatibility with old `ctx` call sites.
    pub(crate) fn ctx_mut(&mut self) -> &mut RuntimeContext {
        self.state.runtime_context_mut()
    }

    /// Stores the build config path used by the active invocation.
    pub(crate) fn set_build_config_path(&mut self, path: Option<PathBuf>) {
        self.ctx_mut().build_config_path = path;
    }

    /// Returns whether this invocation should keep debug runtime artifacts.
    pub(crate) fn debug_enabled(&self) -> bool {
        self.options.debug()
    }

    /// Updates debug artifact behavior for paths that load it after initialization.
    pub(crate) fn set_debug_enabled(&mut self, debug: bool) {
        self.options.debug = debug;
    }

    /// Mirrors a Cargo build config into runtime state for variable expansion.
    pub(crate) fn sync_cargo_context(&mut self, cargo: &Cargo) {
        self.set_build_config(BuildConfig {
            system: BuildSystem::Cargo(cargo.clone()),
        });
    }

    /// Stores the build config and derives the active build context from it.
    pub fn set_build_config(&mut self, build_config: BuildConfig) {
        let active_build = match &build_config.system {
            BuildSystem::Cargo(cargo) => Some(ActiveBuildContext::Cargo(ActiveCargoBuild::new(
                cargo.package.clone(),
                cargo.bin.clone(),
                cargo.target.clone(),
            ))),
            BuildSystem::Custom(custom) => Some(ActiveBuildContext::Custom(
                ActiveCustomBuild::new(custom.elf_path.clone().into(), custom.to_bin),
            )),
        };
        self.ctx_mut().build_config = Some(build_config);
        self.state_mut().set_active_build(active_build);
    }

    /// Returns the package manifest directory used as the invocation workdir.
    pub fn manifest_dir(&self) -> &PathBuf {
        self.project_layout.manifest_dir()
    }

    /// Returns the Cargo workspace root resolved from metadata.
    pub fn workspace_dir(&self) -> &PathBuf {
        self.project_layout.workspace_dir()
    }

    /// Returns the resolved build directory, defaulting to `target` under the package.
    pub fn build_dir(&self) -> PathBuf {
        self.options
            .build_dir()
            .map(|dir| self.resolve_dir(dir))
            .unwrap_or_else(|| self.manifest_dir().join("target"))
    }

    /// Returns the resolved BIN directory when one was supplied.
    pub fn bin_dir(&self) -> Option<PathBuf> {
        self.options.bin_dir().map(|dir| self.resolve_dir(dir))
    }

    /// Resolves relative invocation directories against the manifest directory.
    fn resolve_dir(&self, dir: &Path) -> PathBuf {
        if dir.is_relative() {
            self.manifest_dir().join(dir)
        } else {
            dir.to_path_buf()
        }
    }

    /// Runs a shell hook command with invocation variables expanded.
    pub(crate) fn shell_run_cmd(&self, cmd: &str) -> anyhow::Result<()> {
        crate::process::shell_run_cmd(&self.process_context(), cmd)
    }

    /// Builds a process command rooted in this invocation.
    pub(crate) fn command(&self, program: &str) -> crate::utils::Command {
        crate::process::command(program, &self.process_context())
    }

    /// Loads Cargo metadata for the resolved project layout.
    pub(crate) fn metadata(&self) -> anyhow::Result<Metadata> {
        metadata::cargo_metadata(self.project_layout())
    }

    /// Resolves a Cargo package's manifest directory for package-scoped variables.
    pub(crate) fn resolve_package_manifest_dir(&self, package: &str) -> anyhow::Result<PathBuf> {
        metadata::package_manifest_dir(self.project_layout(), package)
    }

    /// Prepares an explicit ELF file and updates runtime artifact state.
    pub(crate) async fn prepare_elf_artifact(
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

    /// Converts the current ELF artifact to a BIN file and updates state.
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

    /// Applies a Cargo build outcome as runtime artifact state.
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

    /// Resolves the build config path from explicit input or workspace default.
    pub(crate) fn resolve_build_config_path(&self, explicit_path: Option<PathBuf>) -> PathBuf {
        crate::build::config_loader::resolve_build_config_path(self.project_layout(), explicit_path)
    }

    /// Loads build config, runs menu hooks when requested, and stores active state.
    pub(crate) async fn prepare_build_config(
        &mut self,
        config_path: Option<PathBuf>,
        menu: bool,
    ) -> anyhow::Result<BuildConfig> {
        let loaded = crate::build::config_loader::load_build_config(
            self.project_layout(),
            config_path,
            menu,
        )
        .await?;
        self.set_build_config_path(Some(loaded.path.clone()));
        self.set_build_config(loaded.config.clone());
        Ok(loaded.config)
    }

    /// Expands ostool placeholders in a string using invocation variable scope.
    pub(crate) fn replace_string(&self, input: &str) -> anyhow::Result<String> {
        crate::project::variables::expand_variables(input, &self.variable_scope())
    }

    /// Expands ostool placeholders in a filesystem path.
    pub(crate) fn replace_path_variables(&self, path: PathBuf) -> anyhow::Result<PathBuf> {
        crate::project::variables::expand_path_variables(path, &self.variable_scope())
    }

    /// Chooses the package root used when expanding package-scoped variables.
    fn package_root_for_variables(&self) -> anyhow::Result<PathBuf> {
        if let Some(ActiveBuildContext::Cargo(cargo)) = self.state().active_build() {
            return self.resolve_package_manifest_dir(cargo.package());
        }

        if let Some(BuildConfig {
            system: BuildSystem::Cargo(cargo),
        }) = &self.ctx().build_config
        {
            return self.resolve_package_manifest_dir(&cargo.package);
        }

        Ok(self.manifest_dir().to_path_buf())
    }

    /// Builds the variable scope used by config, hooks, and commands.
    pub(crate) fn variable_scope(&self) -> VariableScope {
        let package_dir = self
            .package_root_for_variables()
            .unwrap_or_else(|_| self.manifest_dir().to_path_buf());
        VariableScope::for_package(self.project_layout(), package_dir)
    }

    /// Creates process execution context from invocation layout and artifact state.
    pub(crate) fn process_context(&self) -> ProcessContext {
        ProcessContext::new(
            self.manifest_dir().to_path_buf(),
            self.workspace_dir().to_path_buf(),
            self.variable_scope(),
            self.ctx().artifacts.elf.clone(),
        )
    }

    /// Converts invocation options into artifact-preparation options.
    fn runtime_artifact_options(&self) -> RuntimeArtifactOptions {
        RuntimeArtifactOptions {
            bin_dir: self.bin_dir(),
            debug: self.debug_enabled(),
        }
    }

    /// Reconstructs prepared artifact state from the active runtime context.
    fn prepared_runtime_artifacts_from_context(&self) -> anyhow::Result<PreparedRuntimeArtifacts> {
        Ok(PreparedRuntimeArtifacts::new(
            self.ctx().artifacts.clone(),
            self.ctx()
                .arch
                .ok_or_else(|| anyhow!("architecture not detected"))?,
        ))
    }

    /// Writes prepared artifact paths and architecture back into runtime state.
    fn apply_prepared_runtime_artifacts(&mut self, prepared: PreparedRuntimeArtifacts) {
        self.ctx_mut().arch = Some(prepared.arch());
        self.ctx_mut().artifacts = prepared.artifacts().clone();
    }

    /// Returns UI hooks bound to the resolved project layout.
    pub(crate) fn ui_hooks(&self) -> Vec<ElementHook> {
        crate::build::config_hooks::ui_hooks(self.project_layout())
    }
}
