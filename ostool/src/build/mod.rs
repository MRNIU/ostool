//! Build system configuration and Cargo integration.
//!
//! This module provides functionality for building operating system projects
//! using Cargo or custom build commands. It supports:
//!
//! - Configuring build options via TOML configuration files
//! - Running pre-build and post-build shell commands
//! - Automatic feature detection and configuration
//! - Multiple runner types (QEMU, U-Boot)
//!
//! # Example
//!
//! ```rust,no_run
//! use ostool::build::config::{BuildConfig, BuildSystem, Cargo};
//! use ostool::Invocation;
//!
//! // Build configurations are typically loaded from TOML files
//! // See .build.toml for example configuration format
//! ```

use std::path::Path;

use crate::{
    Invocation,
    build::{
        cargo_builder::CargoBuilder,
        config::{Cargo, Custom},
    },
    run::{
        qemu::{self, QemuConfig, RunQemuOptions},
        uboot::{self, RunUbootOptions, UbootConfig},
    },
};

/// Cargo builder implementation for building projects.
pub(crate) mod artifact_selector;
mod cargo_builder;
pub(crate) mod config_hooks;
pub(crate) mod config_loader;

/// Build configuration types and structures.
pub mod config;

pub mod someboot;

/// Parameters for running a built Cargo artifact in QEMU.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CargoQemuRunnerArgs {
    /// Optional fully prepared QEMU runtime configuration.
    pub qemu: Option<QemuConfig>,
    /// Whether to enable debug mode (GDB server).
    pub debug: bool,
    /// Whether to dump the device tree blob.
    pub dtb_dump: bool,
    /// Whether to show QEMU output.
    pub show_output: bool,
}

/// Parameters for running a built Cargo artifact on real hardware via U-Boot.
#[derive(Debug, Clone, Default)]
pub struct CargoUbootRunnerArgs {
    /// Optional fully prepared U-Boot runtime configuration.
    pub uboot: Option<UbootConfig>,
    /// Whether to show U-Boot output.
    pub show_output: bool,
}

/// Specifies the type of runner to use after building.
///
/// This enum determines how the built artifact will be executed,
/// either through QEMU emulation or via U-Boot on real hardware.
pub enum CargoRunnerKind {
    /// Run the built artifact in QEMU emulator.
    Qemu(Box<CargoQemuRunnerArgs>),
    /// Run the built artifact on real hardware via U-Boot.
    Uboot(Box<CargoUbootRunnerArgs>),
}

impl CargoRunnerKind {
    pub fn new_qemu(args: CargoQemuRunnerArgs) -> Self {
        Self::Qemu(Box::new(args))
    }

    pub fn new_uboot(args: CargoUbootRunnerArgs) -> Self {
        Self::Uboot(Box::new(args))
    }
}

/// Returns the default build configuration template.
pub fn default_build_config() -> config::BuildConfig {
    config::BuildConfig::default()
}

/// Loads a build configuration from a workspace-like directory.
pub async fn load_build_config_from_dir(
    invocation: &mut Invocation,
    dir: &Path,
    menu: bool,
) -> anyhow::Result<config::BuildConfig> {
    invocation
        .prepare_build_config(Some(dir.join(".build.toml")), menu)
        .await
}

/// Loads a build configuration from an explicit file path.
pub async fn load_build_config_from_path(
    invocation: &mut Invocation,
    path: &Path,
    menu: bool,
) -> anyhow::Result<config::BuildConfig> {
    invocation
        .prepare_build_config(Some(path.to_path_buf()), menu)
        .await
}

/// Builds the project using the specified build configuration.
pub async fn build_with_config(
    invocation: &mut Invocation,
    config: &config::BuildConfig,
) -> anyhow::Result<()> {
    match &config.system {
        config::BuildSystem::Custom(custom) => build_custom(invocation, custom)?,
        config::BuildSystem::Cargo(cargo) => {
            cargo_build(invocation, cargo).await?;
        }
    }
    Ok(())
}

/// Runs a custom build command with invocation variable expansion.
pub(crate) fn build_custom(invocation: &mut Invocation, config: &Custom) -> anyhow::Result<()> {
    invocation.shell_run_cmd(&config.build_cmd)?;
    Ok(())
}

/// Builds the project using Cargo.
pub async fn cargo_build(invocation: &mut Invocation, config: &Cargo) -> anyhow::Result<()> {
    invocation.sync_cargo_context(config);
    cargo_builder::CargoBuilder::build_auto(invocation, config)
        .execute()
        .await?;
    Ok(())
}

/// Builds and prepares runtime artifacts without starting a runner.
pub(crate) async fn prepare_runtime_artifacts(
    invocation: &mut Invocation,
    config: &config::BuildConfig,
    debug: bool,
) -> anyhow::Result<()> {
    match &config.system {
        config::BuildSystem::Custom(custom) => {
            prepare_custom_runtime_artifacts(invocation, custom).await
        }
        config::BuildSystem::Cargo(cargo) => {
            prepare_cargo_runtime_artifacts(invocation, cargo, debug).await
        }
    }
}

/// Prepares runtime artifact state from a custom build configuration.
async fn prepare_custom_runtime_artifacts(
    invocation: &mut Invocation,
    config: &Custom,
) -> anyhow::Result<()> {
    build_custom(invocation, config)?;
    invocation
        .prepare_elf_artifact(config.elf_path.clone().into(), config.to_bin)
        .await
}

/// Prepares runtime artifact state from a Cargo build configuration.
async fn prepare_cargo_runtime_artifacts(
    invocation: &mut Invocation,
    config: &Cargo,
    debug: bool,
) -> anyhow::Result<()> {
    let build_config_path = invocation.ctx().build_config_path.clone();
    CargoBuilder::build(invocation, config, build_config_path)
        .debug(debug)
        .skip_objcopy(true)
        .resolve_artifact_from_json(true)
        .execute()
        .await?;
    Ok(())
}

/// Builds and runs the project using Cargo with the specified runner.
pub async fn cargo_run(
    invocation: &mut Invocation,
    config: &Cargo,
    runner: &CargoRunnerKind,
) -> anyhow::Result<()> {
    invocation.sync_cargo_context(config);
    let build_config_path = invocation.ctx().build_config_path.clone();

    let debug = matches!(runner, CargoRunnerKind::Qemu(args) if args.debug);

    CargoBuilder::build(invocation, config, build_config_path)
        .debug(debug)
        .skip_objcopy(true)
        .resolve_artifact_from_json(true)
        .execute()
        .await?;

    match runner {
        CargoRunnerKind::Qemu(args) => {
            let qemu = match &args.qemu {
                Some(config) => config.clone(),
                None => qemu::ensure_qemu_config_for_cargo(invocation, config).await?,
            };
            qemu::run_qemu(
                invocation,
                &qemu,
                RunQemuOptions {
                    dtb_dump: args.dtb_dump,
                    show_output: args.show_output,
                },
            )
            .await?;
        }
        CargoRunnerKind::Uboot(args) => {
            let uboot = match &args.uboot {
                Some(config) => config.clone(),
                None => uboot::ensure_uboot_config_for_cargo(invocation, config).await?,
            };
            uboot::run_uboot(
                invocation,
                &uboot,
                RunUbootOptions {
                    show_output: args.show_output,
                },
            )
            .await?;
        }
    }

    Ok(())
}
