//! Trybuild fixture covering the supported invocation-based public API.

use ostool::{
    Invocation, InvocationOptions, board,
    build::{self, config::{BuildConfig, Cargo}},
    run::{
        qemu::{self, QemuConfig, RunQemuOptions},
        uboot::{self, RunUbootOptions, UbootConfig},
    },
};

/// Exercises supported public configuration and runner construction calls.
fn main() {
    let invocation = Invocation::new(InvocationOptions::default()).unwrap();
    let _: BuildConfig = build::default_build_config();
    let cargo = Cargo::default();
    let qemu: QemuConfig = qemu::default_qemu_config_for_cargo(&invocation, &cargo);
    let _ = qemu::default_qemu_config(&invocation);
    let uboot: UbootConfig = uboot::default_uboot_config();
    let _ = board::default_board_run_config();
    let _ = RunQemuOptions::default();
    let _ = RunUbootOptions::default();
    let _ = board::RunBoardOptions::default();
    let _ = build::CargoRunnerKind::new_qemu(build::CargoQemuRunnerArgs {
        qemu: Some(qemu),
        debug: false,
        dtb_dump: false,
        show_output: true,
    });
    let _ = build::CargoRunnerKind::new_uboot(build::CargoUbootRunnerArgs {
        uboot: Some(uboot),
        show_output: true,
    });
}
