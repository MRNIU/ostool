use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::{
    artifact::state::DebugArtifactKind,
    build::config::{
        AnalysisConfig, ArtifactConfig, BuildConfig, BuildSystem, Cargo, CargoBuildProfile, Custom,
    },
    invocation::{Invocation, InvocationOptions},
};

use super::{RuntimeArtifactInput, build_with_config, prepare_runtime_artifact};

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project with spaces");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"analysis-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(
            root.join("src/main.rs"),
            "fn main() { println!(\"fixture\"); }\n",
        )
        .unwrap();
        Self { _temp: temp, root }
    }

    fn invocation(&self) -> Invocation {
        Invocation::new(InvocationOptions::new(
            Some(self.root.clone()),
            None,
            None,
            false,
        ))
        .unwrap()
    }

    fn copy_current_executable(&self, name: &str) -> PathBuf {
        let path = self.root.join(name);
        fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        path
    }
}

fn native_target() -> String {
    let output = Command::new("rustc").arg("-vV").output().unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .expect("rustc -vV reports a host target")
        .to_owned()
}

fn quoted(path: &Path) -> String {
    format!("'{}'", path.display())
}

fn analysis_config(disassembly: bool, elf_info: bool, symbols: bool) -> ArtifactConfig {
    ArtifactConfig {
        analysis: AnalysisConfig {
            disassembly,
            elf_info,
            symbols,
        },
    }
}

fn custom_build(elf_path: &Path, build_cmd: String, artifacts: ArtifactConfig) -> BuildConfig {
    BuildConfig {
        artifacts,
        system: BuildSystem::Custom(Custom {
            build_cmd,
            elf_path: elf_path.display().to_string(),
            to_bin: false,
        }),
    }
}

fn llvm_nm(path: &Path) -> Vec<u8> {
    let output = Command::new(crate::artifact::llvm_tools::llvm_tool("llvm-nm").unwrap())
        .args(["--demangle", "--print-size", "--numeric-sort"])
        .arg(path)
        .output()
        .unwrap();
    assert!(output.status.success());
    output.stdout
}

#[tokio::test]
async fn cargo_build_emits_each_requested_analysis_artifact_after_hooks() {
    let fixture = Fixture::new();
    let pre_marker = fixture.root.join("pre build marker");
    let post_marker = fixture.root.join("post build marker");
    let mut invocation = fixture.invocation();
    let config = BuildConfig {
        artifacts: analysis_config(true, true, true),
        system: BuildSystem::Cargo(Box::new(Cargo {
            env: HashMap::from([("RUSTC_BOOTSTRAP".into(), "1".into())]),
            target: native_target(),
            package: "analysis-fixture".into(),
            bin: Some("analysis-fixture".into()),
            profile: Some(CargoBuildProfile::Debug),
            disable_someboot_build_config: true,
            pre_build_cmds: vec![format!("touch {}", quoted(&pre_marker))],
            post_build_cmds: vec![format!(
                "printf '%s' \"$KERNEL_ELF\" > {}",
                quoted(&post_marker)
            )],
            ..Default::default()
        })),
    };

    build_with_config(&mut invocation, &config, None)
        .await
        .unwrap();

    let source = invocation
        .runtime_artifacts()
        .cargo_source_elf()
        .unwrap()
        .to_path_buf();
    assert!(pre_marker.exists());
    assert_eq!(
        fs::read_to_string(&post_marker).unwrap(),
        source.display().to_string()
    );
    for (kind, suffix) in [
        (DebugArtifactKind::Disassembly, "disassembly"),
        (DebugArtifactKind::ElfInfo, "elf-info"),
        (DebugArtifactKind::Symbols, "symbols"),
    ] {
        let path = invocation
            .runtime_artifacts()
            .debug_artifacts()
            .get(kind)
            .unwrap();
        assert_eq!(
            path,
            source
                .with_file_name(format!("analysis-fixture.{suffix}"))
                .as_path()
        );
        assert!(!fs::read(path).unwrap().is_empty());
    }
}

#[test]
fn prepared_runtime_copy_keeps_source_and_debug_registry_when_adding_bin() {
    let fixture = Fixture::new();
    let source = fixture.copy_current_executable("original image.elf");
    let original_bytes = fs::read(&source).unwrap();
    let cargo_artifact_dir = fixture.root.join("cargo artifacts");
    fs::create_dir_all(&cargo_artifact_dir).unwrap();
    let mut invocation = fixture.invocation();

    prepare_runtime_artifact(
        &mut invocation,
        RuntimeArtifactInput::new(&source, false)
            .with_cargo_artifact_dir(&cargo_artifact_dir)
            .strip_elf(true),
    )
    .unwrap();
    let source = source.canonicalize().unwrap();
    let runtime_elf = source.with_file_name("original image.elf.runtime.elf");
    let stripped_runtime = source.with_file_name("runtime symbols removed.elf");
    let status = Command::new(crate::artifact::llvm_tools::llvm_objcopy().unwrap())
        .arg("--strip-all")
        .arg(&runtime_elf)
        .arg(&stripped_runtime)
        .status()
        .unwrap();
    assert!(status.success());
    fs::rename(&stripped_runtime, &runtime_elf).unwrap();
    let config = BuildConfig {
        artifacts: analysis_config(false, false, true),
        system: BuildSystem::Cargo(Box::default()),
    };
    super::generate_build_analysis(&mut invocation, &config).unwrap();
    let symbols = source.with_file_name("original image.elf.symbols");

    let bin = invocation.ensure_runtime_bin().unwrap();

    assert!(
        fs::read(&source).unwrap() == original_bytes,
        "runtime preparation must preserve the source bytes"
    );
    assert!(runtime_elf.exists());
    assert_eq!(
        invocation.runtime_artifacts().cargo_source_elf(),
        Some(source.as_path())
    );
    assert_eq!(
        invocation.runtime_artifacts().elf(),
        Some(runtime_elf.as_path())
    );
    assert!(bin.exists());
    let source_symbols = llvm_nm(&source);
    let runtime_symbols = llvm_nm(&runtime_elf);
    assert_ne!(source_symbols, runtime_symbols);
    assert!(
        fs::read(&symbols).unwrap() == source_symbols,
        "analysis must use the original ELF symbols"
    );
    assert_eq!(
        invocation
            .runtime_artifacts()
            .debug_artifacts()
            .get(DebugArtifactKind::Symbols),
        Some(symbols.as_path())
    );
}

#[tokio::test]
async fn rebuild_clears_debug_artifacts_when_analysis_is_disabled_or_fails() {
    let fixture = Fixture::new();
    let source = fixture.copy_current_executable("analysis source");
    let mut invocation = fixture.invocation();
    let requested = custom_build(&source, "true".into(), analysis_config(false, false, true));
    let disabled = custom_build(&source, "true".into(), ArtifactConfig::default());

    build_with_config(&mut invocation, &requested, None)
        .await
        .unwrap();
    assert!(invocation.runtime_artifacts().elf().is_none());
    assert!(invocation.runtime_artifacts().bin().is_none());
    let symbols = invocation
        .runtime_artifacts()
        .debug_artifacts()
        .get(DebugArtifactKind::Symbols)
        .unwrap()
        .to_path_buf();
    fs::write(&symbols, "stale symbols").unwrap();

    build_with_config(&mut invocation, &requested, None)
        .await
        .unwrap();
    assert_ne!(fs::read(&symbols).unwrap(), b"stale symbols");
    assert_eq!(
        invocation
            .runtime_artifacts()
            .debug_artifacts()
            .get(DebugArtifactKind::Symbols),
        Some(symbols.as_path())
    );

    build_with_config(&mut invocation, &disabled, None)
        .await
        .unwrap();

    assert!(invocation.runtime_artifacts().debug_artifacts().is_empty());
    let missing = fixture.root.join("missing source");
    let failing = custom_build(&missing, "true".into(), analysis_config(false, false, true));

    build_with_config(&mut invocation, &requested, None)
        .await
        .unwrap();
    assert!(
        invocation
            .runtime_artifacts()
            .debug_artifacts()
            .get(DebugArtifactKind::Symbols)
            .is_some()
    );

    assert!(
        build_with_config(&mut invocation, &failing, None)
            .await
            .is_err()
    );

    assert!(invocation.runtime_artifacts().debug_artifacts().is_empty());
}
