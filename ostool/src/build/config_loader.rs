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

    pub fn config(&self) -> &BuildConfig {
        &self.config
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

pub fn apply_cargo_selector(
    config: &mut BuildConfig,
    package: Option<&str>,
    bin: Option<&str>,
) -> anyhow::Result<()> {
    if package.is_none() && bin.is_none() {
        return Ok(());
    }

    let BuildSystem::Cargo(cargo) = &mut config.system else {
        bail!("--package/--bin can only be used with system.Cargo build configs");
    };

    if let Some(package) = package {
        cargo.package = package.to_string();
    }
    if let Some(bin) = bin {
        cargo.bin = Some(bin.to_string());
    }

    Ok(())
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

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::build::{
        config::{BuildConfig, BuildSystem, Cargo, Custom},
        config_hooks::build_config_hooks,
        config_loader::{apply_cargo_selector, load_build_config},
    };

    #[tokio::test]
    async fn load_build_config_records_path_and_skips_disabled_someboot_args() {
        let temp = tempfile::tempdir().unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"app\", \"someboot\"]\nresolver = \"3\"\n",
        )
        .unwrap();
        fs::write(
            temp.path().join(".build.toml"),
            r#"
[system.Cargo]
package = "app"
target = "x86_64-unknown-none"
disable_someboot_build_config = true
env = {}
features = []
args = []
pre_build_cmds = []
post_build_cmds = []
to_bin = false
"#,
        )
        .unwrap();
        let app_dir = temp.path().join("app");
        fs::create_dir_all(app_dir.join("src")).unwrap();
        fs::write(
            app_dir.join("Cargo.toml"),
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\nsomeboot = { path = \"../someboot\" }\n",
        )
        .unwrap();
        fs::write(app_dir.join("src/main.rs"), "fn main() {}\n").unwrap();
        let someboot_dir = temp.path().join("someboot");
        fs::create_dir_all(someboot_dir.join("src")).unwrap();
        fs::write(
            someboot_dir.join("Cargo.toml"),
            "[package]\nname = \"someboot\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(someboot_dir.join("src/lib.rs"), "pub fn marker() {}\n").unwrap();
        fs::write(
            someboot_dir.join("build-info.toml"),
            "[x86_64-unknown-none]\ncargoargs = [\"--someboot-cargoarg\"]\nrustflags = [\"-Cdebuginfo=2\"]\n",
        )
        .unwrap();

        let hooks = build_config_hooks(temp.path());
        let loaded = load_build_config(temp.path(), None, false, &hooks, true)
            .await
            .unwrap();

        assert_eq!(loaded.path(), temp.path().join(".build.toml"));
        let BuildSystem::Cargo(cargo) = &loaded.config().system else {
            panic!("expected Cargo build config");
        };
        assert!(!cargo.args.iter().any(|arg| arg == "--someboot-cargoarg"));
        assert!(
            !cargo
                .args
                .iter()
                .any(|arg| arg.contains("target.x86_64-unknown-none.rustflags"))
        );
    }

    #[test]
    fn apply_cargo_selector_overrides_cargo_build_config() {
        let mut build_config = BuildConfig {
            system: BuildSystem::Cargo(Cargo {
                package: "default-package".into(),
                bin: None,
                ..Default::default()
            }),
        };

        apply_cargo_selector(&mut build_config, Some("kernel"), Some("kernel-qemu")).unwrap();

        match &build_config.system {
            BuildSystem::Cargo(cargo) => {
                assert_eq!(cargo.package, "kernel");
                assert_eq!(cargo.bin.as_deref(), Some("kernel-qemu"));
            }
            other => panic!("unexpected build system: {other:?}"),
        }
    }

    #[test]
    fn apply_cargo_selector_rejects_custom_build_config() {
        let mut build_config = BuildConfig {
            system: BuildSystem::Custom(Custom {
                build_cmd: "make".into(),
                elf_path: "target/kernel.elf".into(),
                to_bin: true,
            }),
        };

        let err = apply_cargo_selector(&mut build_config, Some("kernel"), None)
            .unwrap_err()
            .to_string();

        assert!(err.contains("--package/--bin can only be used with system.Cargo"));
    }
}
