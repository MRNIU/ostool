use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use jkconfig::data::ElementHook;

use crate::build::{
    config::{BuildConfig, BuildSystem},
    someboot,
};

#[derive(Debug, Clone)]
pub struct LoadedBuildConfig {
    path: PathBuf,
    config: BuildConfig,
}

impl LoadedBuildConfig {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn into_config(self) -> BuildConfig {
        self.config
    }
}

pub fn resolve_build_config_path(workspace_dir: &Path, explicit_path: Option<PathBuf>) -> PathBuf {
    explicit_path.unwrap_or_else(|| workspace_dir.join(".build.toml"))
}

pub async fn load_build_config(
    workspace_dir: &Path,
    explicit_path: Option<PathBuf>,
    menu: bool,
    hooks: &[ElementHook],
    enable_someboot_build_config: bool,
) -> anyhow::Result<LoadedBuildConfig> {
    let path = resolve_build_config_path(workspace_dir, explicit_path);

    let Some(mut config): Option<BuildConfig> = jkconfig::run(path.clone(), menu, hooks)
        .await
        .with_context(|| format!("failed to load build config: {}", path.display()))?
    else {
        bail!("No build configuration obtained");
    };

    apply_someboot_build_config(workspace_dir, &mut config, enable_someboot_build_config)?;

    Ok(LoadedBuildConfig { path, config })
}

fn apply_someboot_build_config(
    workspace_dir: &Path,
    config: &mut BuildConfig,
    enable_someboot_build_config: bool,
) -> anyhow::Result<()> {
    if let BuildSystem::Cargo(cargo) = &mut config.system
        && enable_someboot_build_config
        && !cargo.disable_someboot_build_config
    {
        let manifest_path = workspace_dir.join("Cargo.toml");
        let iter = someboot::detect_build_config_for_package(
            &manifest_path,
            &cargo.package,
            &cargo.features,
            &cargo.target,
        )?
        .into_iter();
        cargo.args.extend(iter);
    }

    Ok(())
}
