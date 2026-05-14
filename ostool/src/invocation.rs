//! Invocation options and state shared by CLI and library entrypoints.

use std::path::{Path, PathBuf};

use crate::{ctx::AppContext, project::ProjectLayout};

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

/// Mutable state accumulated while an invocation runs.
#[derive(Clone, Debug, Default)]
pub struct InvocationState {
    app_context: AppContext,
    active_build: Option<ActiveBuildContext>,
}

impl InvocationState {
    /// Returns the compatibility app context used by existing call sites.
    pub fn app_context(&self) -> &AppContext {
        &self.app_context
    }

    /// Returns mutable compatibility app context used by existing call sites.
    pub fn app_context_mut(&mut self) -> &mut AppContext {
        &mut self.app_context
    }

    /// Returns the active build context captured from the current build config.
    pub fn active_build(&self) -> Option<&ActiveBuildContext> {
        self.active_build.as_ref()
    }

    /// Replaces the active build context after config loading or sync.
    pub fn set_active_build(&mut self, active_build: Option<ActiveBuildContext>) {
        self.active_build = active_build;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActiveBuildContext {
    Cargo(ActiveCargoBuild),
    Custom(ActiveCustomBuild),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveCargoBuild {
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

    /// Returns the package name from the active Cargo config.
    pub fn package(&self) -> &str {
        &self.package
    }

    /// Returns the selected Cargo binary target, if configured.
    pub fn bin(&self) -> Option<&str> {
        self.bin.as_deref()
    }

    /// Returns the selected Rust compilation target triple.
    pub fn target(&self) -> &str {
        &self.target
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveCustomBuild {
    elf_path: PathBuf,
    to_bin: bool,
}

impl ActiveCustomBuild {
    /// Creates active custom build metadata from an ELF path and BIN flag.
    pub fn new(elf_path: PathBuf, to_bin: bool) -> Self {
        Self { elf_path, to_bin }
    }

    /// Returns the configured custom ELF path.
    pub fn elf_path(&self) -> &Path {
        &self.elf_path
    }

    /// Returns whether the custom ELF should also be converted to BIN.
    pub fn to_bin(&self) -> bool {
        self.to_bin
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
    /// Resolves the project layout and initializes invocation state.
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
    pub fn state(&self) -> &InvocationState {
        &self.state
    }

    /// Returns mutable access to the runtime state wrapper.
    pub fn state_mut(&mut self) -> &mut InvocationState {
        &mut self.state
    }

    pub fn into_project_layout(self) -> ProjectLayout {
        self.project_layout
    }
}
