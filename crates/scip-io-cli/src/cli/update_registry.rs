use std::path::PathBuf;

use anyhow::{Context, Result};
use console::style;

use super::UpdateRegistryArgs;

/// Default URL for the remote indexer registry.
const DEFAULT_REGISTRY_URL: &str =
    "https://raw.githubusercontent.com/user/scip-io/main/registry.json";

pub async fn run(args: UpdateRegistryArgs) -> Result<()> {
    let url = args.url.unwrap_or_else(|| DEFAULT_REGISTRY_URL.to_string());

    println!(
        "{} Fetching registry from {}...",
        style(">").cyan().bold(),
        url,
    );

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "scip-io")
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .with_context(|| format!("Failed to fetch registry from {}", url))?;

    if !resp.status().is_success() {
        anyhow::bail!(
            "Failed to fetch registry: HTTP {} from {}",
            resp.status(),
            url,
        );
    }

    let body = resp.text().await.context("Failed to read response body")?;

    // Validate that the response is valid JSON
    let parsed: serde_json::Value =
        serde_json::from_str(&body).context("Remote registry is not valid JSON")?;

    // Basic structural validation: expect an array of indexer entries
    if !parsed.is_array() {
        anyhow::bail!(
            "Remote registry has unexpected format (expected a JSON array of indexer entries)"
        );
    }

    let entry_count = parsed.as_array().map(|a| a.len()).unwrap_or(0);

    // Determine cache directory
    let cache_dir = registry_dir();
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("Cannot create directory {}", cache_dir.display()))?;

    let registry_path = cache_dir.join("registry.json");

    // Check if content is the same as existing (unless --force)
    if !args.force && registry_path.exists() {
        let existing = std::fs::read_to_string(&registry_path).unwrap_or_default();
        if existing == body {
            println!(
                "{} Registry is already up to date ({} entries)",
                style("v").green().bold(),
                entry_count,
            );
            return Ok(());
        }
    }

    std::fs::write(&registry_path, &body)
        .with_context(|| format!("Failed to write registry to {}", registry_path.display()))?;

    println!(
        "{} Registry updated: {} ({} entries)",
        style("v").green().bold(),
        registry_path.display(),
        entry_count,
    );

    Ok(())
}

/// Return the directory where registry data is cached.
fn registry_dir() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("scip-io")
}
