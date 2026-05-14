//! Runtime artifact state and preparation helpers.

use std::path::PathBuf;

use crate::Invocation;

pub mod runtime;
pub mod state;

pub async fn prepare_elf_artifact(
    invocation: &mut Invocation,
    path: PathBuf,
    to_bin: bool,
) -> anyhow::Result<()> {
    invocation.prepare_elf_artifact(path, to_bin).await
}
