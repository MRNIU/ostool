//! Custom build hooks may generate the explicitly requested runner configuration.
#![cfg(unix)]

use std::{fs, process::Command};

fn custom_build_generates_runner_config(runner: &str, field: &str) {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("src")).unwrap();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"hook-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::copy(std::env::current_exe().unwrap(), temp.path().join("kernel")).unwrap();
    // Deliberately invalid field type: config parsing must fail after the hook,
    // without starting QEMU or accessing any serial device.
    let config = ostool::build::config::BuildConfig {
        system: ostool::build::config::BuildSystem::Custom(ostool::build::config::Custom {
            build_cmd: format!("printf '{field} = 42\\n' > generated.toml"),
            elf_path: "kernel".into(),
            to_bin: false,
        }),
        artifacts: ostool::build::config::ArtifactConfig {
            analysis: ostool::build::config::AnalysisConfig {
                elf_info: true,
                ..Default::default()
            },
        },
    };
    fs::write(
        temp.path().join(".build.toml"),
        toml::to_string(&config).unwrap(),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ostool"))
        .current_dir(temp.path())
        .args([
            "run",
            runner,
            &format!("--{runner}-config"),
            "generated.toml",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(temp.path().join("generated.toml")).unwrap(),
        format!("{field} = 42\n")
    );
    assert!(temp.path().join("kernel.elf-info").exists());
    let message = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(message.contains("invalid type"), "{message}");
}

#[test]
fn custom_qemu_config_is_read_after_build_hook() {
    custom_build_generates_runner_config("qemu", "args");
}

#[test]
fn custom_uboot_config_is_read_after_build_hook() {
    custom_build_generates_runner_config("uboot", "dtb_file");
}
