use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::detect::Language;
use crate::indexer::IndexerEntry;

/// Run an indexer binary against a project root and return the output .scip path.
pub async fn run_indexer(
    binary: &Path,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
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

    for arg in &entry.default_args {
        // Replace the generic output file with our language-specific one
        if arg == "index.scip" {
            cmd.arg(output_file.to_string_lossy().as_ref());
        } else {
            cmd.arg(arg);
        }
    }

    // Some indexers accept --output to control the destination
    if !entry
        .default_args
        .iter()
        .any(|a| a == "--output" || a.contains("index.scip"))
    {
        cmd.arg("--output").arg(&output_file);
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

    Ok(output_file)
}
