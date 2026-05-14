//! Interactive build-config hooks for selecting Cargo packages, features, and targets.

use std::{path::Path, process::Command, sync::Arc};

use anyhow::{Context, anyhow, bail};
use jkconfig::data::{
    ElementHook, HookContext, HookFlow, HookOption, MessageLevel, MultiSelectBinding,
    MultiSelectSpec, SingleSelectBinding, SingleSelectSpec,
};

use crate::project::ProjectLayout;

/// Builds the set of UI hooks used by the build config editor.
pub(crate) fn ui_hooks(layout: &ProjectLayout) -> Vec<ElementHook> {
    vec![
        ui_hook_feature_select(layout),
        ui_hook_package_select(layout),
        ui_hook_target_select(layout),
    ]
}

/// Creates the multi-select hook for Cargo feature values.
fn ui_hook_feature_select(layout: &ProjectLayout) -> ElementHook {
    let path = "system.features";
    let cargo_toml = layout.workspace_dir().join("Cargo.toml");
    ElementHook {
        path: path.into(),
        callback: Arc::new(move |ctx: &mut HookContext<'_>, path| {
            let package = ctx
                .get_string("system.package")?
                .unwrap_or_default()
                .trim()
                .to_string();
            if package.is_empty() {
                ctx.show_message(
                    jkconfig::data::MessageLevel::Warning,
                    "Select a package before editing features.",
                );
                return Ok(HookFlow::Consumed);
            }

            let feature_options = collect_feature_options(&cargo_toml, &package, None)?;
            let options = feature_options
                .into_iter()
                .map(|feature| HookOption::new(feature.clone(), feature))
                .collect();

            ctx.present_multi_select(MultiSelectSpec {
                title: format!("Features for {package}"),
                help: Some(
                    "Space toggle  Enter apply. Dependency features use dep_name/feature.".into(),
                ),
                options,
                selected: ctx.get_strings(path.clone())?,
                min_selected: None,
                max_selected: None,
                binding: MultiSelectBinding::SetStringArray { path: path.clone() },
            })?;

            Ok(HookFlow::Consumed)
        }),
    }
}

/// Creates the single-select hook for Cargo package values.
fn ui_hook_package_select(layout: &ProjectLayout) -> ElementHook {
    let path = "system.package";
    let cargo_toml = layout.workspace_dir().join("Cargo.toml");

    ElementHook {
        path: path.into(),
        callback: Arc::new(move |ctx: &mut HookContext<'_>, path| {
            let mut items = Vec::new();
            if let Ok(metadata) = cargo_metadata::MetadataCommand::new()
                .manifest_path(&cargo_toml)
                .no_deps()
                .exec()
            {
                for pkg in &metadata.packages {
                    items.push(pkg.name.to_string());
                }
            }

            let options = items
                .into_iter()
                .map(|item| HookOption::new(item.clone(), item))
                .collect();
            ctx.present_single_select(SingleSelectSpec {
                title: "Select Package".into(),
                help: Some("Choose the Cargo package used by the build config.".into()),
                options,
                initial: ctx.get_string(path.clone())?,
                allow_clear: false,
                binding: SingleSelectBinding::SetString { path: path.clone() },
            })?;
            Ok(HookFlow::Consumed)
        }),
    }
}

/// Creates the single-select hook for target triples.
fn ui_hook_target_select(layout: &ProjectLayout) -> ElementHook {
    let path = "system.target";
    let cargo_toml = layout.workspace_dir().join("Cargo.toml");

    ElementHook {
        path: path.into(),
        callback: Arc::new(move |ctx: &mut HookContext<'_>, path| {
            let package = ctx
                .get_string("system.package")?
                .unwrap_or_default()
                .trim()
                .to_string();
            let current_target = ctx.get_string(path.clone())?;

            let mut warnings = Vec::new();
            let (options, help) = if package.is_empty() {
                fallback_rustup_targets()?
            } else {
                match collect_package_doc_targets(&cargo_toml, &package) {
                    Ok(Some(doc_targets)) => (
                        build_target_options(TargetCandidateSet::DocsRs(&doc_targets)),
                        "Select a target declared by the selected package docs.rs metadata."
                            .to_string(),
                    ),
                    Ok(None) => fallback_rustup_targets()?,
                    Err(err) => {
                        warnings.push(format!(
                            "Failed to inspect docs.rs targets for package '{package}': {err}"
                        ));
                        fallback_rustup_targets()?
                    }
                }
            };

            if options.is_empty() {
                bail!("No target candidates available for selection");
            }

            for warning in warnings {
                ctx.show_message(MessageLevel::Warning, warning);
            }

            ctx.present_single_select(SingleSelectSpec {
                title: "Select Target".into(),
                help: Some(help),
                options,
                initial: current_target,
                allow_clear: false,
                binding: SingleSelectBinding::SetString { path: path.clone() },
            })?;

            Ok(HookFlow::Consumed)
        }),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RustupTargetOption {
    pub(crate) triple: String,
    pub(crate) installed: bool,
}

pub(crate) enum TargetCandidateSet<'a> {
    DocsRs(&'a [String]),
    Rustup(&'a [RustupTargetOption]),
}

/// Returns rustup target candidates when package metadata has no docs.rs targets.
fn fallback_rustup_targets() -> anyhow::Result<(Vec<HookOption>, String)> {
    let rustup_targets = collect_rustup_targets()?;
    if rustup_targets.is_empty() {
        bail!("No Rust targets available from `rustup target list`");
    }
    Ok((
        build_target_options(TargetCandidateSet::Rustup(&rustup_targets)),
        "Package has no docs.rs targets; showing rustup targets.".to_string(),
    ))
}

/// Collects package and dependency feature names for config UI selection.
pub(crate) fn collect_feature_options(
    manifest_path: &Path,
    package_name: &str,
    deps_filter: Option<&[String]>,
) -> anyhow::Result<Vec<String>> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(manifest_path)
        .no_deps()
        .exec()?;
    let Some(pkg) = metadata
        .packages
        .iter()
        .find(|pkg| pkg.name == package_name)
    else {
        bail!(
            "package '{package_name}' not found in {}",
            manifest_path.display()
        );
    };

    let mut features = pkg.features.keys().cloned().collect::<Vec<_>>();
    features.sort();

    for dependency in &pkg.dependencies {
        let include = match deps_filter {
            Some(filter) => filter.contains(&dependency.name),
            None => true,
        };
        if !include {
            continue;
        }

        let Some(dep_pkg) = metadata
            .packages
            .iter()
            .find(|candidate| candidate.name == dependency.name)
        else {
            continue;
        };
        let mut dep_features = dep_pkg.features.keys().cloned().collect::<Vec<_>>();
        dep_features.sort();
        features.extend(
            dep_features
                .into_iter()
                .map(|feature| format!("{}/{}", dependency.name, feature)),
        );
    }

    Ok(features)
}

/// Reads `package.metadata.docs.rs` target declarations for one package.
pub(crate) fn collect_package_doc_targets(
    manifest_path: &Path,
    package_name: &str,
) -> anyhow::Result<Option<Vec<String>>> {
    let metadata = cargo_metadata::MetadataCommand::new()
        .manifest_path(manifest_path)
        .no_deps()
        .exec()
        .with_context(|| {
            format!(
                "failed to load cargo metadata from {}",
                manifest_path.display()
            )
        })?;
    let Some(pkg) = metadata
        .packages
        .iter()
        .find(|pkg| pkg.name == package_name)
    else {
        bail!(
            "package '{package_name}' not found in {}",
            manifest_path.display()
        );
    };

    parse_docs_rs_targets(&pkg.metadata)
}

/// Parses docs.rs target metadata into an ordered target list.
fn parse_docs_rs_targets(metadata: &serde_json::Value) -> anyhow::Result<Option<Vec<String>>> {
    let Some(docs) = metadata.get("docs") else {
        return Ok(None);
    };
    let Some(docs_rs) = docs.get("rs") else {
        return Ok(None);
    };

    let targets = match docs_rs.get("targets") {
        Some(serde_json::Value::Array(values)) => {
            let mut targets = Vec::with_capacity(values.len());
            for value in values {
                let target = value.as_str().ok_or_else(|| {
                    anyhow!("package.metadata.docs.rs.targets must be an array of strings")
                })?;
                let target = target.trim();
                if target.is_empty() {
                    bail!("package.metadata.docs.rs.targets must not contain empty strings");
                }
                if !targets.iter().any(|existing| existing == target) {
                    targets.push(target.to_string());
                }
            }
            Some(targets)
        }
        Some(_) => bail!("package.metadata.docs.rs.targets must be an array of strings"),
        None => None,
    };

    let default_target = match docs_rs.get("default-target") {
        Some(serde_json::Value::String(value)) => {
            let value = value.trim();
            if value.is_empty() {
                bail!("package.metadata.docs.rs.default-target must not be empty");
            }
            Some(value.to_string())
        }
        Some(_) => bail!("package.metadata.docs.rs.default-target must be a string"),
        None => None,
    };

    let mut normalized = match targets {
        Some(targets) if !targets.is_empty() => targets,
        _ => Vec::new(),
    };

    if let Some(default_target) = default_target {
        if let Some(index) = normalized
            .iter()
            .position(|target| target == &default_target)
        {
            let value = normalized.remove(index);
            normalized.insert(0, value);
        } else {
            normalized.insert(0, default_target);
        }
    }

    if normalized.is_empty() {
        Ok(None)
    } else {
        Ok(Some(normalized))
    }
}

/// Runs `rustup target list` and parses installed targets before available ones.
fn collect_rustup_targets() -> anyhow::Result<Vec<RustupTargetOption>> {
    let output = Command::new("rustup")
        .args(["target", "list"])
        .output()
        .context("failed to run `rustup target list`")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "`rustup target list` failed with {}:\n{}",
            output.status,
            stderr.trim()
        );
    }

    let stdout = String::from_utf8(output.stdout)
        .context("`rustup target list` output is not valid UTF-8")?;
    Ok(parse_rustup_targets(&stdout))
}

/// Parses `rustup target list` output into ordered target options.
pub(crate) fn parse_rustup_targets(output: &str) -> Vec<RustupTargetOption> {
    let mut installed = Vec::new();
    let mut available = Vec::new();

    for line in output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let installed_flag = line.ends_with(" (installed)");
        let triple = line
            .strip_suffix(" (installed)")
            .unwrap_or(line)
            .trim()
            .to_string();
        if triple.is_empty() {
            continue;
        }

        let option = RustupTargetOption {
            triple,
            installed: installed_flag,
        };
        if installed_flag {
            installed.push(option);
        } else {
            available.push(option);
        }
    }

    installed.extend(available);
    installed
}

/// Converts target candidates into jkconfig hook options.
pub(crate) fn build_target_options(candidates: TargetCandidateSet<'_>) -> Vec<HookOption> {
    match candidates {
        TargetCandidateSet::DocsRs(targets) => targets
            .iter()
            .cloned()
            .map(|target| HookOption {
                value: target.clone(),
                label: target,
                detail: Some("docs.rs target".into()),
                disabled: false,
            })
            .collect(),
        TargetCandidateSet::Rustup(targets) => targets
            .iter()
            .map(|target| HookOption {
                value: target.triple.clone(),
                label: target.triple.clone(),
                detail: Some(if target.installed {
                    "installed".into()
                } else {
                    "available".into()
                }),
                disabled: false,
            })
            .collect(),
    }
}
