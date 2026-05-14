use std::path::PathBuf;

use anyhow::{Context, bail};

use crate::{
    build::{
        config::{BuildConfig, BuildSystem, Cargo},
        config_hooks, someboot,
    },
    project::ProjectLayout,
};

#[derive(Clone, Debug)]
pub(crate) struct LoadedBuildConfig {
    pub(crate) path: PathBuf,
    pub(crate) config: BuildConfig,
}

pub(crate) fn resolve_build_config_path(
    layout: &ProjectLayout,
    explicit_path: Option<PathBuf>,
) -> PathBuf {
    explicit_path.unwrap_or_else(|| layout.workspace_dir().join(".build.toml"))
}

pub(crate) async fn load_build_config(
    layout: &ProjectLayout,
    config_path: Option<PathBuf>,
    menu: bool,
) -> anyhow::Result<LoadedBuildConfig> {
    let path = resolve_build_config_path(layout, config_path);
    let hooks = config_hooks::ui_hooks(layout);
    let Some(mut config): Option<BuildConfig> = jkconfig::run(path.clone(), menu, &hooks)
        .await
        .with_context(|| format!("failed to load build config: {}", path.display()))?
    else {
        bail!("No build configuration obtained");
    };

    if let BuildSystem::Cargo(cargo) = &mut config.system {
        let iter = someboot_cargo_args(layout, cargo)?.into_iter();
        cargo.args.extend(iter);
    }

    Ok(LoadedBuildConfig { path, config })
}

fn someboot_cargo_args(layout: &ProjectLayout, cargo: &Cargo) -> anyhow::Result<Vec<String>> {
    let manifest_path = layout.workspace_dir().join("Cargo.toml");
    someboot::detect_build_config_for_package(
        &manifest_path,
        &cargo.package,
        &cargo.features,
        &cargo.target,
    )
}
