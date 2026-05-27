use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::detect::Language;
use crate::indexer::IndexerEntry;
use crate::scip_language::{normalize_scip_file_languages, relativize_scip_file_document_paths};

/// Run an indexer binary against a project root and return the output .scip path.
pub async fn run_indexer(
    binary: &Path,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
) -> Result<PathBuf> {
    run_indexer_with_configs(binary, entry, project_root, lang, &[]).await
}

/// Run an indexer binary against a project root with optional config files.
pub async fn run_indexer_with_configs(
    binary: &Path,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    config_paths: &[PathBuf],
) -> Result<PathBuf> {
    tracing::info!(
        indexer = %entry.indexer_name,
        lang = lang.name(),
        root = %project_root.display(),
        "running indexer"
    );

    // Build the output path so multiple indexers don't clobber each other
    let output_file = project_root.join(format!("{}.scip", lang.name()));

    let mut cmd = tokio::process::Command::new(binary);
    cmd.current_dir(project_root);

    for arg in build_indexer_args(entry, &output_file, config_paths) {
        cmd.arg(arg);
    }

    let status = cmd
        .status()
        .await
        .with_context(|| format!("Failed to execute {}", binary.display()))?;

    if !status.success() {
        anyhow::bail!(
            "{} exited with status {} for {}",
            entry.indexer_name,
            status,
            lang.name()
        );
    }

    // The indexer might have written to its default name instead
    if !output_file.exists() {
        let default_output = project_root.join(&entry.output_file);
        if default_output.exists() {
            std::fs::rename(&default_output, &output_file)?;
        } else {
            anyhow::bail!(
                "Indexer {} did not produce output file (expected {})",
                entry.indexer_name,
                output_file.display()
            );
        }
    }

    let updated_languages = normalize_scip_file_languages(&output_file, Some(lang.name()))?;
    if updated_languages > 0 {
        tracing::info!(
            path = %output_file.display(),
            docs = updated_languages,
            "filled missing SCIP document languages"
        );
    }
    let updated_paths = relativize_scip_file_document_paths(&output_file, project_root)?;
    if updated_paths > 0 {
        tracing::info!(
            path = %output_file.display(),
            docs = updated_paths,
            "relativized SCIP document paths"
        );
    }

    Ok(output_file)
}

/// Build argv after the binary name, placing supported config files before
/// the output option so CLIs with positional project arguments can parse them.
pub fn build_indexer_args(
    entry: &IndexerEntry,
    output_file: &Path,
    config_paths: &[PathBuf],
) -> Vec<OsString> {
    let mut args = Vec::new();
    let mut has_output_arg = false;

    for arg in &entry.default_args {
        if arg == "index.scip" {
            args.push(output_file.as_os_str().to_os_string());
            has_output_arg = true;
        } else {
            if arg == "--output" || arg.contains("index.scip") {
                has_output_arg = true;
            }
            args.push(OsString::from(arg));
        }
    }

    let project_root = output_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    args.extend(
        config_paths
            .iter()
            .map(|path| config_path_arg(path, project_root)),
    );

    // Config-driven indexers need an explicit destination because the default
    // output file can be overwritten when one invocation spans several configs.
    // Keep no-config runs on their historical default argv and let the rename
    // fallback handle indexers that do not expose an output flag.
    if !config_paths.is_empty() && !has_output_arg {
        args.push(OsString::from("--output"));
        args.push(output_file.as_os_str().to_os_string());
    }

    args
}

fn config_path_arg(path: &Path, project_root: Option<&Path>) -> OsString {
    project_root
        .and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path)
        .as_os_str()
        .to_os_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::{IndexerEntry, InstallMethod};

    fn entry(default_args: &[&str]) -> IndexerEntry {
        IndexerEntry {
            indexer_name: "test-indexer".into(),
            language: "typescript".into(),
            github_repo: "owner/repo".into(),
            binary_name: "test-indexer".into(),
            version: "1.0.0".into(),
            default_args: default_args.iter().map(|arg| arg.to_string()).collect(),
            output_file: "index.scip".into(),
            install_method: InstallMethod::Unsupported {
                reason: "test".into(),
            },
        }
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn build_indexer_args_places_config_paths_before_output_flag() {
        let args = build_indexer_args(
            &entry(&["index"]),
            Path::new("typescript.scip"),
            &[
                PathBuf::from("tsconfig.json"),
                PathBuf::from("tsconfig.test.json"),
            ],
        );

        assert_eq!(
            strings(args),
            vec![
                "index",
                "tsconfig.json",
                "tsconfig.test.json",
                "--output",
                "typescript.scip",
            ]
        );
    }

    #[test]
    fn build_indexer_args_uses_relative_config_paths_under_output_root() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/project");
        let args = build_indexer_args(
            &entry(&["index"]),
            &root.join("typescript.scip"),
            &[
                root.join("tsconfig.json"),
                root.join("tsconfig.scripts.json"),
            ],
        );

        assert_eq!(
            strings(args),
            vec![
                "index",
                "tsconfig.json",
                "tsconfig.scripts.json",
                "--output",
                root.join("typescript.scip").to_string_lossy().as_ref(),
            ]
        );
    }

    #[test]
    fn build_indexer_args_replaces_default_output_placeholder() {
        let args = build_indexer_args(
            &entry(&["index", "--output", "index.scip"]),
            Path::new("go.scip"),
            &[],
        );

        assert_eq!(strings(args), vec!["index", "--output", "go.scip"]);
    }

    #[test]
    fn build_indexer_args_preserves_default_args_without_configs() {
        let args = build_indexer_args(&entry(&["index"]), Path::new("typescript.scip"), &[]);

        assert_eq!(strings(args), vec!["index"]);
    }
}
