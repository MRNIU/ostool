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

    pub fn manifest(&self) -> Option<&Path> {
        self.manifest.as_deref()
    }

    pub fn build_dir(&self) -> Option<&Path> {
        self.build_dir.as_deref()
    }

    pub fn bin_dir(&self) -> Option<&Path> {
        self.bin_dir.as_deref()
    }

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
    pub(crate) fn runtime_context(&self) -> &RuntimeContext {
        &self.runtime_context
    }

    pub(crate) fn runtime_context_mut(&mut self) -> &mut RuntimeContext {
        &mut self.runtime_context
    }

    pub(crate) fn active_build(&self) -> Option<&ActiveBuildContext> {
        self.active_build.as_ref()
    }

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
    pub fn new(package: String, bin: Option<String>, target: String) -> Self {
        Self {
            package,
            bin,
            target,
        }
    }

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
    pub fn new(options: InvocationOptions) -> anyhow::Result<Self> {
        let project_layout =
            crate::project::resolve_project_layout(options.manifest().map(PathBuf::from))?;
        Ok(Self {
            options,
            project_layout,
            state: InvocationState::default(),
        })
    }

    pub fn options(&self) -> &InvocationOptions {
        &self.options
    }

    pub fn project_layout(&self) -> &ProjectLayout {
        &self.project_layout
    }

    pub(crate) fn state(&self) -> &InvocationState {
        &self.state
    }

    pub(crate) fn state_mut(&mut self) -> &mut InvocationState {
        &mut self.state
    }

    pub fn into_project_layout(self) -> ProjectLayout {
        self.project_layout
    }

    pub(crate) fn ctx(&self) -> &RuntimeContext {
        self.state.runtime_context()
    }

    pub(crate) fn ctx_mut(&mut self) -> &mut RuntimeContext {
        self.state.runtime_context_mut()
    }

    pub(crate) fn set_build_config_path(&mut self, path: Option<PathBuf>) {
        self.ctx_mut().build_config_path = path;
    }

    pub(crate) fn debug_enabled(&self) -> bool {
        self.options.debug()
    }

    pub(crate) fn set_debug_enabled(&mut self, debug: bool) {
        self.options.debug = debug;
    }

    pub(crate) fn sync_cargo_context(&mut self, cargo: &Cargo) {
        self.set_build_config(BuildConfig {
            system: BuildSystem::Cargo(cargo.clone()),
        });
    }

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

    pub fn manifest_dir(&self) -> &PathBuf {
        self.project_layout.manifest_dir()
    }

    pub fn workspace_dir(&self) -> &PathBuf {
        self.project_layout.workspace_dir()
    }

    pub fn build_dir(&self) -> PathBuf {
        self.options
            .build_dir()
            .map(|dir| self.resolve_dir(dir))
            .unwrap_or_else(|| self.manifest_dir().join("target"))
    }

    pub fn bin_dir(&self) -> Option<PathBuf> {
        self.options.bin_dir().map(|dir| self.resolve_dir(dir))
    }

    fn resolve_dir(&self, dir: &Path) -> PathBuf {
        if dir.is_relative() {
            self.manifest_dir().join(dir)
        } else {
            dir.to_path_buf()
        }
    }

    pub(crate) fn shell_run_cmd(&self, cmd: &str) -> anyhow::Result<()> {
        crate::process::shell_run_cmd(&self.process_context(), cmd)
    }

    pub(crate) fn command(&self, program: &str) -> crate::utils::Command {
        crate::process::command(program, &self.process_context())
    }

    pub(crate) fn metadata(&self) -> anyhow::Result<Metadata> {
        metadata::cargo_metadata(self.project_layout())
    }

    pub(crate) fn resolve_package_manifest_dir(&self, package: &str) -> anyhow::Result<PathBuf> {
        metadata::package_manifest_dir(self.project_layout(), package)
    }

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
        crate::build::config_loader::resolve_build_config_path(self.project_layout(), explicit_path)
    }

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

    pub(crate) fn replace_string(&self, input: &str) -> anyhow::Result<String> {
        crate::project::variables::expand_variables(input, &self.variable_scope())
    }

    pub(crate) fn replace_path_variables(&self, path: PathBuf) -> anyhow::Result<PathBuf> {
        crate::project::variables::expand_path_variables(path, &self.variable_scope())
    }

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

    pub(crate) fn variable_scope(&self) -> VariableScope {
        let package_dir = self
            .package_root_for_variables()
            .unwrap_or_else(|_| self.manifest_dir().to_path_buf());
        VariableScope::for_package(self.project_layout(), package_dir)
    }

    pub(crate) fn process_context(&self) -> ProcessContext {
        ProcessContext::new(
            self.manifest_dir().to_path_buf(),
            self.workspace_dir().to_path_buf(),
            self.variable_scope(),
            self.ctx().artifacts.elf.clone(),
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
            self.ctx().artifacts.clone(),
            self.ctx()
                .arch
                .ok_or_else(|| anyhow!("architecture not detected"))?,
        ))
    }

    fn apply_prepared_runtime_artifacts(&mut self, prepared: PreparedRuntimeArtifacts) {
        self.ctx_mut().arch = Some(prepared.arch());
        self.ctx_mut().artifacts = prepared.artifacts().clone();
    }

    pub(crate) fn ui_hooks(&self) -> Vec<ElementHook> {
        crate::build::config_hooks::ui_hooks(self.project_layout())
    }
}
