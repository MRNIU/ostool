//! End-to-end regression tests for configured FIT images.
//!
//! These tests deliberately inspect FIT images with U-Boot's `dumpimage` and
//! extract each embedded component.  Checking the FDT header alone would not
//! prove that the requested payload, addresses, or optional FDT made it into
//! the image.

use std::{ffi::OsString, fs, path::Path, process::Command};

use super::{FitInput, generate_configured_fit, generate_fit_image};
use crate::{
    artifact::{
        elf_metadata::ElfMetadata,
        llvm_tools::llvm_objcopy,
        runtime::{PreparedRuntimeArtifacts, RuntimeArtifactOptions, prepare_runtime_artifacts},
    },
    boot::fit_config::{FitAddress, FitConfig, FitFdt, FitFormat, FitOs},
    process::ProcessContext,
    project::{resolve_project_layout, variables::VariableScope},
};

const ELF_LOAD: u64 = 0x400_000;
const ELF_ENTRY: u64 = ELF_LOAD + 4;

fn process_context(root: &Path) -> ProcessContext {
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fit-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "").unwrap();

    let layout = resolve_project_layout(Some(root.to_path_buf())).unwrap();
    let scope = VariableScope::for_package(&layout, root.to_path_buf());
    ProcessContext::new(root.to_path_buf(), root.to_path_buf(), scope, None)
}

fn prepare_fixture(
    root: &Path,
    architecture: u16,
    strip_elf: bool,
) -> (ProcessContext, PreparedRuntimeArtifacts) {
    let context = process_context(root);
    let source = root.join("kernel.elf");
    fs::write(&source, minimal_elf(architecture)).unwrap();
    let prepared = prepare_runtime_artifacts(
        &context,
        RuntimeArtifactOptions {
            elf_path: source,
            to_bin: false,
            bin_dir: None,
            debug: false,
            cargo_artifact_dir: None,
            strip_elf,
        },
    )
    .unwrap();
    (context, prepared)
}

/// Builds the smallest useful little-endian ELF64 fixture in memory.
///
/// The file contains one executable PT_LOAD segment, a `.text` section, and
/// a defined `__executable_start` symbol.  This lets the configured FIT path
/// exercise the same metadata reader as production without compiling an
/// external target or relying on host architecture.
fn minimal_elf(machine: u16) -> Vec<u8> {
    const PROGRAM_OFFSET: usize = 0x100;
    const SHSTRTAB_OFFSET: usize = 0x120;
    const STRTAB_OFFSET: usize = 0x150;
    const SYMTAB_OFFSET: usize = 0x170;
    const SECTION_HEADERS_OFFSET: usize = 0x200;
    const SECTION_HEADER_SIZE: usize = 64;

    let mut elf = vec![0_u8; SECTION_HEADERS_OFFSET + SECTION_HEADER_SIZE * 5];
    elf[..4].copy_from_slice(b"\x7fELF");
    elf[4] = 2; // ELFCLASS64
    elf[5] = 1; // ELFDATA2LSB
    elf[6] = 1; // EV_CURRENT
    put_u16(&mut elf, 16, 2); // ET_EXEC
    put_u16(&mut elf, 18, machine);
    put_u32(&mut elf, 20, 1);
    put_u64(&mut elf, 24, ELF_ENTRY);
    put_u64(&mut elf, 32, 64); // e_phoff
    put_u64(&mut elf, 40, SECTION_HEADERS_OFFSET as u64);
    put_u16(&mut elf, 52, 64); // e_ehsize
    put_u16(&mut elf, 54, 56); // e_phentsize
    put_u16(&mut elf, 56, 1); // e_phnum
    put_u16(&mut elf, 58, SECTION_HEADER_SIZE as u16);
    put_u16(&mut elf, 60, 5); // e_shnum
    put_u16(&mut elf, 62, 1); // e_shstrndx

    // The single PT_LOAD makes `llvm-objcopy -O binary` deterministic.
    put_u32(&mut elf, 64, 1); // PT_LOAD
    put_u32(&mut elf, 68, 5); // PF_R | PF_X
    put_u64(&mut elf, 72, PROGRAM_OFFSET as u64);
    put_u64(&mut elf, 80, ELF_LOAD);
    put_u64(&mut elf, 88, ELF_LOAD);
    put_u64(&mut elf, 96, 8);
    put_u64(&mut elf, 104, 8);
    put_u64(&mut elf, 112, 0x100);
    elf[PROGRAM_OFFSET..PROGRAM_OFFSET + 8].copy_from_slice(b"KERNEL\0!");

    let section_names = b"\0.shstrtab\0.strtab\0.symtab\0.text\0";
    elf[SHSTRTAB_OFFSET..SHSTRTAB_OFFSET + section_names.len()].copy_from_slice(section_names);
    let symbol_names = b"\0__executable_start\0";
    elf[STRTAB_OFFSET..STRTAB_OFFSET + symbol_names.len()].copy_from_slice(symbol_names);

    // ELF64 symbol table: the first null entry plus __executable_start.
    let symbol = SYMTAB_OFFSET + 24;
    put_u32(&mut elf, symbol, 1);
    elf[symbol + 4] = 0x10; // STB_GLOBAL | STT_NOTYPE
    put_u16(&mut elf, symbol + 6, 4); // .text section index
    put_u64(&mut elf, symbol + 8, ELF_LOAD);

    // Section 1: .shstrtab; Section 2: .strtab; Section 3: .symtab;
    // Section 4: .text.  The production parser only needs the normal ELF
    // layout here; the contents are intentionally tiny.
    section_header(
        &mut elf,
        SECTION_HEADERS_OFFSET + SECTION_HEADER_SIZE,
        1,
        3,
        0,
        0,
        SHSTRTAB_OFFSET as u64,
        section_names.len() as u64,
        0,
        0,
        1,
        0,
    );
    section_header(
        &mut elf,
        SECTION_HEADERS_OFFSET + SECTION_HEADER_SIZE * 2,
        11,
        3,
        0,
        0,
        STRTAB_OFFSET as u64,
        symbol_names.len() as u64,
        0,
        0,
        1,
        0,
    );
    section_header(
        &mut elf,
        SECTION_HEADERS_OFFSET + SECTION_HEADER_SIZE * 3,
        19,
        2,
        0,
        0,
        SYMTAB_OFFSET as u64,
        48,
        2,
        1,
        8,
        24,
    );
    section_header(
        &mut elf,
        SECTION_HEADERS_OFFSET + SECTION_HEADER_SIZE * 4,
        27,
        1,
        0x6,
        ELF_LOAD,
        PROGRAM_OFFSET as u64,
        8,
        0,
        0,
        4,
        0,
    );

    elf
}

fn minimal_elf_without_executable_start(machine: u16) -> Vec<u8> {
    let mut elf = minimal_elf(machine);
    // The ELF and first PT_LOAD both start at file offset zero.  Its physical
    // base differs from .text by 0x100, so ELF containers and raw BINs have
    // intentionally different correct auto-load values.
    put_u64(&mut elf, 72, 0);
    put_u64(&mut elf, 80, ELF_LOAD - 0x100);
    put_u64(&mut elf, 88, ELF_LOAD - 0x100);
    put_u64(&mut elf, 96, 0x108);
    put_u64(&mut elf, 104, 0x108);
    // Keep a named but undefined symbol to model a stripped source ELF.
    put_u16(&mut elf, 0x170 + 24 + 6, 0);
    elf
}

#[allow(clippy::too_many_arguments)]
fn section_header(
    bytes: &mut [u8],
    offset: usize,
    name: u32,
    kind: u32,
    flags: u64,
    address: u64,
    file_offset: u64,
    size: u64,
    link: u32,
    info: u32,
    alignment: u64,
    entry_size: u64,
) {
    put_u32(bytes, offset, name);
    put_u32(bytes, offset + 4, kind);
    put_u64(bytes, offset + 8, flags);
    put_u64(bytes, offset + 16, address);
    put_u64(bytes, offset + 24, file_offset);
    put_u64(bytes, offset + 32, size);
    put_u32(bytes, offset + 40, link);
    put_u32(bytes, offset + 44, info);
    put_u64(bytes, offset + 48, alignment);
    put_u64(bytes, offset + 56, entry_size);
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn configured(format: FitFormat) -> FitConfig {
    FitConfig {
        format,
        os: FitOs::Linux,
        load: FitAddress::Auto,
        entry: FitAddress::Auto,
        fdt: None,
        output: None,
    }
}

fn run_dumpimage(args: &[OsString]) -> std::process::Output {
    Command::new("dumpimage")
        .args(args)
        .output()
        .unwrap_or_else(|err| {
            panic!(
                "failed to execute dumpimage; install u-boot-tools (the CI FIT inspector): {err}"
            )
        })
}

fn inspect_fit(path: &Path) -> String {
    let output = run_dumpimage(&[OsString::from("-l"), path.as_os_str().to_os_string()]);
    assert!(
        output.status.success(),
        "dumpimage -l failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn fit_property(path: &Path, node: &str, property: &str) -> String {
    let output = Command::new("fdtget")
        .args(["-t", "s"])
        .arg(path)
        .arg(node)
        .arg(property)
        .output()
        .unwrap_or_else(|err| {
            panic!("failed to execute fdtget; install u-boot-tools (the CI FIT inspector): {err}")
        });
    assert!(
        output.status.success(),
        "fdtget {node} {property} failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn extract_fit_component(path: &Path, position: u32, destination: &Path) -> Vec<u8> {
    let output = run_dumpimage(&[
        OsString::from("-T"),
        OsString::from("flat_dt"),
        OsString::from("-p"),
        OsString::from(position.to_string()),
        OsString::from("-o"),
        destination.as_os_str().to_os_string(),
        path.as_os_str().to_os_string(),
    ]);
    assert!(
        output.status.success(),
        "dumpimage extraction for component {position} failed with status {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    fs::read(destination).unwrap()
}

fn assert_linux_arm64_header(listing: &str, load: u64, entry: u64) {
    let listing = listing.to_ascii_lowercase();
    assert_dumpimage_field(&listing, "os", &["linux"]);
    assert_dumpimage_field(&listing, "architecture", &["aarch64", "arm64"]);
    assert_dumpimage_address(&listing, "load address", load);
    assert_dumpimage_address(&listing, "entry point", entry);
}

fn assert_dumpimage_field(listing: &str, label: &str, expected: &[&str]) {
    let value = listing
        .lines()
        .find_map(|line| line.trim().strip_prefix(label))
        .and_then(|line| line.strip_prefix(':'))
        .map(str::trim)
        .unwrap_or_else(|| panic!("dumpimage listing has no {label}:\n{listing}"));
    assert!(
        expected.contains(&value),
        "unexpected dumpimage {label} `{value}`; expected one of {expected:?}\n{listing}"
    );
}

fn assert_dumpimage_address(listing: &str, label: &str, expected: u64) {
    let address = listing
        .lines()
        .find_map(|line| line.trim().strip_prefix(label))
        .and_then(|line| line.strip_prefix(':'))
        .map(str::trim)
        .unwrap_or_else(|| panic!("dumpimage listing has no {label}:\n{listing}"));
    assert_eq!(address, format!("0x{expected:08x}"), "{listing}");
}

fn image_listing(listing: &str, position: u32) -> &str {
    let marker = format!("Image {position} (");
    let start = listing
        .find(&marker)
        .unwrap_or_else(|| panic!("dumpimage listing has no {marker}:\n{listing}"));
    let remainder = &listing[start..];
    let next_marker = format!("\n Image {} (", position + 1);
    &remainder[..remainder.find(&next_marker).unwrap_or(remainder.len())]
}

fn expected_bin(context: &ProcessContext, source: &Path, output: &Path) -> Vec<u8> {
    let objcopy = llvm_objcopy().unwrap();
    let result = crate::process::command(objcopy, context)
        .arg("--strip-all")
        .arg("-O")
        .arg("binary")
        .arg(source)
        .arg(output)
        .run();
    result.unwrap();
    fs::read(output).unwrap()
}

fn strip_elf(context: &ProcessContext, source: &Path, output: &Path) {
    let objcopy = llvm_objcopy().unwrap();
    crate::process::command(objcopy, context)
        .arg("--strip-all")
        .arg(source)
        .arg(output)
        .run()
        .unwrap();
}

#[tokio::test]
async fn configured_elf_uses_runtime_payload_source_metadata_and_fdt() {
    let temp = tempfile::Builder::new()
        .prefix("FIT input with spaces ")
        .tempdir()
        .unwrap();
    let (context, prepared) = prepare_fixture(temp.path(), 183, true); // EM_AARCH64
    let source = prepared.source_elf().to_path_buf();
    let runtime = prepared.elf().to_path_buf();
    assert_ne!(
        source, runtime,
        "the fixture must exercise distinct sources"
    );

    // A stripped runtime ELF has no __executable_start.  It remains a valid
    // ELF payload, while auto-address resolution must use source_elf.
    let stripped = temp.path().join("actually-stripped.elf");
    strip_elf(&context, &source, &stripped);
    fs::copy(&stripped, &runtime).unwrap();
    let source_metadata = ElfMetadata::parse(&fs::read(&source).unwrap()).unwrap();
    let runtime_metadata = ElfMetadata::parse(&fs::read(&runtime).unwrap()).unwrap();
    assert_eq!(source_metadata.executable_start, Some(ELF_LOAD));
    assert_eq!(runtime_metadata.executable_start, None);
    let runtime_payload = fs::read(&runtime).unwrap();
    let fdt_path = temp.path().join("board.dtb");
    let fdt_payload = b"test dtb payload";
    fs::write(&fdt_path, fdt_payload).unwrap();
    let output = temp.path().join("configured.fit");

    let image = generate_configured_fit(
        &context,
        &prepared,
        &FitConfig {
            os: FitOs::Elf,
            fdt: Some(FitFdt {
                path: fdt_path,
                load: Some(0x4_8000_0000),
            }),
            output: Some(output.clone()),
            ..configured(FitFormat::Elf)
        },
    )
    .await
    .unwrap();

    assert_eq!(image.path(), output);
    let listing = inspect_fit(image.path());
    assert_dumpimage_field(
        &listing.to_ascii_lowercase(),
        "architecture",
        &["aarch64", "arm64"],
    );
    assert_dumpimage_address(&listing.to_ascii_lowercase(), "load address", ELF_LOAD);
    assert_dumpimage_address(&listing.to_ascii_lowercase(), "entry point", ELF_ENTRY);
    assert_eq!(fit_property(image.path(), "/images/kernel", "os"), "elf");
    assert_eq!(
        extract_fit_component(image.path(), 0, &temp.path().join("kernel.out")),
        runtime_payload
    );
    assert_eq!(
        extract_fit_component(image.path(), 1, &temp.path().join("fdt.out")),
        fdt_payload
    );
    assert_dumpimage_address(
        &image_listing(&listing, 1).to_ascii_lowercase(),
        "load address",
        0x4_8000_0000,
    );
}

#[tokio::test]
async fn configured_bin_derives_payload_and_uses_runtime_directory_default_output() {
    let temp = tempfile::tempdir().unwrap();
    let (context, prepared) = prepare_fixture(temp.path(), 183, false); // EM_AARCH64
    let expected = expected_bin(&context, prepared.elf(), &temp.path().join("expected.bin"));

    let image = generate_configured_fit(&context, &prepared, &configured(FitFormat::Bin))
        .await
        .unwrap();

    assert_eq!(image.path(), temp.path().join("image.fit"));
    let listing = inspect_fit(image.path());
    assert_linux_arm64_header(&listing, ELF_LOAD, ELF_ENTRY);
    assert_eq!(
        extract_fit_component(image.path(), 0, &temp.path().join("kernel.bin.out")),
        expected
    );
}

#[tokio::test]
async fn configured_explicit_zero_addresses_override_auto_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let (context, prepared) = prepare_fixture(temp.path(), 183, false); // EM_AARCH64
    let output = temp.path().join("zero.fit");
    let image = generate_configured_fit(
        &context,
        &prepared,
        &FitConfig {
            load: FitAddress::Explicit(0),
            entry: FitAddress::Explicit(0),
            output: Some(output),
            ..configured(FitFormat::Elf)
        },
    )
    .await
    .unwrap();

    let listing = inspect_fit(image.path()).to_ascii_lowercase();
    assert_dumpimage_address(&listing, "load address", 0);
    assert_dumpimage_address(&listing, "entry point", 0);
}

#[tokio::test]
async fn configured_auto_load_distinguishes_elf_container_from_bin_section_base() {
    let temp = tempfile::tempdir().unwrap();
    let (context, prepared) = prepare_fixture(temp.path(), 183, false); // EM_AARCH64
    fs::write(
        prepared.source_elf(),
        minimal_elf_without_executable_start(183),
    )
    .unwrap();
    let metadata = ElfMetadata::parse(&fs::read(prepared.source_elf()).unwrap()).unwrap();
    assert_eq!(metadata.executable_start, None);

    let elf_image = generate_configured_fit(
        &context,
        &prepared,
        &FitConfig {
            output: Some(temp.path().join("fallback-elf.fit")),
            ..configured(FitFormat::Elf)
        },
    )
    .await
    .unwrap();
    let elf_listing = inspect_fit(elf_image.path()).to_ascii_lowercase();
    assert_dumpimage_address(&elf_listing, "load address", ELF_LOAD - 0x100);
    assert_dumpimage_address(&elf_listing, "entry point", ELF_ENTRY);

    let bin_image = generate_configured_fit(
        &context,
        &prepared,
        &FitConfig {
            output: Some(temp.path().join("fallback-bin.fit")),
            ..configured(FitFormat::Bin)
        },
    )
    .await
    .unwrap();
    let bin_listing = inspect_fit(bin_image.path()).to_ascii_lowercase();
    assert_dumpimage_address(&bin_listing, "load address", ELF_LOAD);
    assert_dumpimage_address(&bin_listing, "entry point", ELF_ENTRY);
    assert_eq!(
        extract_fit_component(bin_image.path(), 0, &temp.path().join("fallback-bin.out")),
        b"KERNEL\0!"
    );
}

#[tokio::test]
async fn configured_fit_rejects_missing_fdt_and_unsupported_architecture() {
    let temp = tempfile::tempdir().unwrap();
    let (context, prepared) = prepare_fixture(temp.path(), 183, false); // EM_AARCH64
    let missing_fdt = temp.path().join("missing.dtb");
    let err = generate_configured_fit(
        &context,
        &prepared,
        &FitConfig {
            fdt: Some(FitFdt {
                path: missing_fdt,
                load: None,
            }),
            ..configured(FitFormat::Elf)
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("DTB"), "{err:#}");

    let unsupported_root = tempfile::tempdir().unwrap();
    let (unsupported_context, unsupported) = prepare_fixture(unsupported_root.path(), 62, false); // EM_X86_64
    let err = generate_configured_fit(
        &unsupported_context,
        &unsupported,
        &configured(FitFormat::Elf),
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("unsupported architecture for FIT image generation"),
        "{err:#}"
    );
}

#[tokio::test]
async fn configured_fit_reports_missing_raw_elf_and_unwritable_output() {
    let temp = tempfile::tempdir().unwrap();
    let (context, prepared) = prepare_fixture(temp.path(), 183, true); // EM_AARCH64
    let mut runtime = fs::read(prepared.elf()).unwrap();
    put_u64(&mut runtime, 24, 0); // A different ELF must not borrow this source's symbols.
    fs::write(prepared.elf(), runtime).unwrap();
    let err = generate_configured_fit(&context, &prepared, &configured(FitFormat::Elf))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("load layouts differ"), "{err:#}");
    fs::write(prepared.source_elf(), b"not an ELF anymore").unwrap();
    let err = generate_configured_fit(&context, &prepared, &configured(FitFormat::Elf))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("ELF"), "{err:#}");

    fs::remove_file(prepared.source_elf()).unwrap();
    let err = generate_configured_fit(&context, &prepared, &configured(FitFormat::Elf))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("source ELF"), "{err:#}");

    let alias_root = tempfile::tempdir().unwrap();
    let (alias_context, alias_prepared) = prepare_fixture(alias_root.path(), 183, false);
    let err = generate_configured_fit(
        &alias_context,
        &alias_prepared,
        &FitConfig {
            output: Some(alias_prepared.source_elf().to_path_buf()),
            ..configured(FitFormat::Elf)
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("overwrite"), "{err:#}");

    let missing_runtime_root = tempfile::tempdir().unwrap();
    let (missing_runtime_context, missing_runtime) =
        prepare_fixture(missing_runtime_root.path(), 183, true); // EM_AARCH64
    fs::remove_file(missing_runtime.elf()).unwrap();
    let err = generate_configured_fit(
        &missing_runtime_context,
        &missing_runtime,
        &configured(FitFormat::Elf),
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("ELF"), "{err:#}");

    let output_root = tempfile::tempdir().unwrap();
    let (output_context, output_prepared) = prepare_fixture(output_root.path(), 183, false);
    let output = output_root.path().join("absent-parent/image.fit");
    let err = generate_configured_fit(
        &output_context,
        &output_prepared,
        &FitConfig {
            output: Some(output),
            ..configured(FitFormat::Elf)
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("FIT"), "{err:#}");
}

#[tokio::test]
async fn configured_elf_rejects_invalid_load_ranges_before_explicit_addresses() {
    let temp = tempfile::tempdir().unwrap();
    let (context, prepared) = prepare_fixture(temp.path(), 183, false);
    for (offset, value) in [(72, 0x10_0000), (104, 1), (0x200 + 64 * 4 + 24, 0x10_0000)] {
        let mut elf = minimal_elf(183);
        put_u64(&mut elf, offset, value);
        fs::write(prepared.elf(), elf).unwrap();
        let result = generate_configured_fit(
            &context,
            &prepared,
            &FitConfig {
                load: FitAddress::Explicit(0),
                entry: FitAddress::Explicit(0),
                ..configured(FitFormat::Elf)
            },
        )
        .await;
        assert!(
            result.is_err(),
            "invalid load range at field {offset:#x} accepted"
        );
    }
}

#[tokio::test]
async fn configured_bin_rejects_empty_payload_even_with_explicit_addresses() {
    let temp = tempfile::tempdir().unwrap();
    let (context, prepared) = prepare_fixture(temp.path(), 183, false);
    let mut elf = minimal_elf(183);
    put_u64(&mut elf, 40, 0); // No section table or file-backed load bytes.
    put_u16(&mut elf, 60, 0);
    put_u16(&mut elf, 62, 0);
    put_u64(&mut elf, 96, 0);
    fs::write(prepared.elf(), elf).unwrap();
    let output = temp.path().join("empty.fit");
    let result = generate_configured_fit(
        &context,
        &prepared,
        &FitConfig {
            load: FitAddress::Explicit(ELF_LOAD),
            entry: FitAddress::Explicit(ELF_ENTRY),
            output: Some(output.clone()),
            ..configured(FitFormat::Bin)
        },
    )
    .await;
    assert!(result.unwrap_err().to_string().contains("empty"));
    assert!(!output.exists());
}

#[tokio::test]
async fn configured_output_replaces_hard_link_without_changing_source() {
    let temp = tempfile::tempdir().unwrap();
    let (context, prepared) = prepare_fixture(temp.path(), 183, false);
    let source = fs::read(prepared.source_elf()).unwrap();
    let output = temp.path().join("aliased.fit");
    fs::hard_link(prepared.source_elf(), &output).unwrap();
    let image = generate_configured_fit(
        &context,
        &prepared,
        &FitConfig {
            output: Some(output),
            ..configured(FitFormat::Elf)
        },
    )
    .await
    .unwrap();
    assert_eq!(fs::read(prepared.source_elf()).unwrap(), source);
    assert_linux_arm64_header(&inspect_fit(image.path()), ELF_LOAD, ELF_ENTRY);
}

#[tokio::test]
async fn configured_bin_surfaces_tool_launch_failure() {
    let temp = tempfile::tempdir().unwrap();
    let (context, prepared) = prepare_fixture(temp.path(), 183, false);
    // Use the real toolchain tool, but a vanished command working directory.
    // The ELF is valid, so failure must come from executing the conversion.
    let context = ProcessContext::new(
        temp.path().join("missing-workdir"),
        temp.path().to_path_buf(),
        context.variables().clone(),
        None,
    );
    let err = generate_configured_fit(&context, &prepared, &configured(FitFormat::Bin))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("llvm-objcopy"), "{err:#}");
    assert!(!temp.path().join("image.fit").exists());
}

#[tokio::test]
async fn legacy_linux_loongarch_fit_keeps_header_address_rule() {
    let temp = tempfile::tempdir().unwrap();
    let kernel_path = temp.path().join("Image");
    let output_path = temp.path().join("legacy.fit");
    let mut kernel = vec![0_u8; 0x40_0000];
    kernel[..2].copy_from_slice(b"MZ");
    kernel[8..16].copy_from_slice(&0x5c_8b40_u64.to_le_bytes());
    kernel[24..32].copy_from_slice(&0x20_0000_u64.to_le_bytes());
    fs::write(&kernel_path, &kernel).unwrap();

    let image = generate_fit_image(FitInput {
        kernel_path,
        dtb_path: None,
        arch: object::Architecture::LoongArch64,
        kernel_load_addr: 0x9000_0000_9800_0000,
        kernel_entry_addr: 0x9000_0000_9800_0000,
        fdt_load_addr: None,
        output_path: Some(output_path),
    })
    .await
    .unwrap();

    let listing = inspect_fit(image.path()).to_ascii_lowercase();
    assert_dumpimage_field(&listing, "os", &["linux"]);
    // Older U-Boot inspectors label LoongArch as unknown; inspect its stored
    // property directly while still checking the full FIT with dumpimage.
    assert_eq!(
        fit_property(image.path(), "/images/kernel", "arch"),
        "loongarch"
    );
    assert_dumpimage_address(&listing, "load address", 0x9000_0000_9800_0000);
    assert_dumpimage_address(&listing, "entry point", 0x9000_0000_983c_8b40);
    assert_eq!(
        extract_fit_component(image.path(), 0, &temp.path().join("legacy-kernel.out")),
        kernel
    );
}
