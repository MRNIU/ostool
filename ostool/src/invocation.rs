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

/// Mutable state accumulated while an invocation runs.
#[derive(Clone, Debug, Default)]
pub struct InvocationState {
    app_context: AppContext,
    active_build: Option<ActiveBuildContext>,
}

impl InvocationState {
    pub fn app_context(&self) -> &AppContext {
        &self.app_context
    }

    pub fn app_context_mut(&mut self) -> &mut AppContext {
        &mut self.app_context
    }

    pub fn active_build(&self) -> Option<&ActiveBuildContext> {
        self.active_build.as_ref()
    }

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

    pub fn bin(&self) -> Option<&str> {
        self.bin.as_deref()
    }

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
    pub fn new(elf_path: PathBuf, to_bin: bool) -> Self {
        Self { elf_path, to_bin }
    }

    pub fn elf_path(&self) -> &Path {
        &self.elf_path
    }

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

    pub fn state(&self) -> &InvocationState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut InvocationState {
        &mut self.state
    }

    pub fn into_project_layout(self) -> ProjectLayout {
        self.project_layout
    }
}
