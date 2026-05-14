use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

use crate::{project::variables::VariableScope, utils::Command};

/// Concrete process inputs for command construction and shell hooks.
#[derive(Clone, Debug)]
pub struct ProcessContext {
    workdir: PathBuf,
    workspace_dir: PathBuf,
    variables: VariableScope,
    kernel_elf: Option<PathBuf>,
}

impl ProcessContext {
    pub fn new(
        workdir: PathBuf,
        workspace_dir: PathBuf,
        variables: VariableScope,
        kernel_elf: Option<PathBuf>,
    ) -> Self {
        Self {
            workdir,
            workspace_dir,
            variables,
            kernel_elf,
        }
    }

    pub fn workdir(&self) -> &Path {
        &self.workdir
    }

    pub fn workspace_dir(&self) -> &Path {
        &self.workspace_dir
    }

    pub fn variables(&self) -> &VariableScope {
        &self.variables
    }

    pub fn kernel_elf(&self) -> Option<&Path> {
        self.kernel_elf.as_deref()
    }
}

pub fn command<S>(program: S, context: &ProcessContext) -> Command
where
    S: AsRef<OsStr>,
{
    let variables = context.variables().clone();
    let mut command = Command::new(program, context.workdir(), move |s| {
        crate::project::variables::expand_os_value(s, &variables)
    });
    command.env(
        "WORKSPACE_FOLDER",
        context.workspace_dir().display().to_string(),
    );
    command
}

pub fn shell_run_cmd(context: &ProcessContext, cmd: &str) -> anyhow::Result<()> {
    let mut command = match std::env::consts::OS {
        "windows" => {
            let mut command = command("powershell", context);
            command.arg("-Command");
            command
        }
        _ => {
            let mut command = command("sh", context);
            command.arg("-c");
            command
        }
    };

    command.arg(cmd);

    if let Some(elf) = context.kernel_elf() {
        command.env("KERNEL_ELF", elf.display().to_string());
    }

    command.run()?;
    Ok(())
}
