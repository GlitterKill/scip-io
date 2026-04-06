use std::time::Duration;

use anyhow::Result;
use console::style;

use scip_io_core::indexer::IndexerEntry;
use scip_io_core::indexer::registry::REGISTRY;

use super::StatusArgs;

pub async fn run(args: StatusArgs) -> Result<()> {
    let entries = REGISTRY.all();

    match args.format.as_str() {
        "json" => {
            let mut json_entries: Vec<serde_json::Value> = entries
                .iter()
                .map(|entry| {
                    serde_json::json!({
                        "indexer": entry.indexer_name,
                        "language": entry.language_name(),
                        "binary": entry.binary_name(),
                        "installed": entry.is_installed(),
                        "version": entry.installed_version(),
                        "path": entry.installed_path().map(|p| p.display().to_string()),
                    })
                })
                .collect();

            if args.check_updates {
                for (i, entry) in entries.iter().enumerate() {
                    let latest = check_latest_version(&entry.github_repo).await;
                    match latest {
                        Ok(ver) => {
                            json_entries[i]["latest_version"] =
                                serde_json::Value::String(ver.clone());
                            json_entries[i]["update_available"] = serde_json::Value::Bool(
                                ver != entry.version,
                            );
                        }
                        Err(e) => {
                            json_entries[i]["update_check_error"] =
                                serde_json::Value::String(format!("{:#}", e));
                        }
                    }
                    // Small delay to avoid GitHub API rate limits
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }

            println!("{}", serde_json::to_string_pretty(&json_entries)?);
        }
        _ => {
            println!(
                "{} Registered indexers:\n",
                style("SCIP-IO").cyan().bold()
            );

            for entry in entries {
                let installed = entry.is_installed();
                let status_icon = if installed {
                    style("v").green().bold()
                } else {
                    style("x").red()
                };

                println!(
                    "  {} {:<14} {}",
                    status_icon,
                    entry.language_name(),
                    style(&entry.indexer_name).dim(),
                );

                if args.verbose {
                    println!("    binary:  {}", entry.binary_name());
                    if installed {
                        if let Some(version) = entry.installed_version() {
                            println!("    version: {}", version);
                        }
                        if let Some(path) = entry.installed_path() {
                            println!("    path:    {}", path.display());
                        }
                    } else {
                        println!("    status:  not installed");
                    }
                    println!();
                }
            }

            if args.check_updates {
                println!(
                    "\n{} Checking for updates...\n",
                    style(">").cyan().bold()
                );

                for entry in entries {
                    let latest = check_latest_version(&entry.github_repo).await;
                    print_update_status(entry, latest);
                    // Small delay to avoid GitHub API rate limits
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            }
        }
    }

    Ok(())
}

/// Query the GitHub releases API for the latest version of an indexer.
async fn check_latest_version(github_repo: &str) -> Result<String> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        github_repo
    );

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("User-Agent", "scip-io")
        .header("Accept", "application/vnd.github.v3+json")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("network error: {}", e))?;

    if !resp.status().is_success() {
        anyhow::bail!("GitHub API returned {}", resp.status());
    }

    let json: serde_json::Value = resp
        .json::<serde_json::Value>()
        .await
        .map_err(|e| anyhow::anyhow!("failed to parse response: {}", e))?;

    let tag = json["tag_name"]
        .as_str()
        .unwrap_or("unknown")
        .trim_start_matches('v')
        .to_string();

    Ok(tag)
}

/// Display update status for a single indexer entry.
fn print_update_status(entry: &IndexerEntry, latest_result: Result<String>) {
    let installed = entry.is_installed();
    let status_marker = if installed {
        style("*").green().to_string()
    } else {
        style("-").dim().to_string()
    };

    let install_label = if installed { "Installed" } else { "Not installed" };

    match latest_result {
        Ok(latest_ver) => {
            let update_info = if latest_ver != entry.version {
                format!(
                    "(latest: {} {})",
                    latest_ver,
                    style("^ update available").yellow()
                )
            } else {
                format!("(up to date {})", style("ok").green())
            };
            println!(
                "  {} {:<20} v{:<10} {} {}",
                status_marker,
                entry.indexer_name,
                entry.version,
                install_label,
                update_info,
            );
        }
        Err(e) => {
            println!(
                "  {} {:<20} v{:<10} {} (check failed: {})",
                status_marker,
                entry.indexer_name,
                entry.version,
                install_label,
                style(format!("{:#}", e)).red(),
            );
        }
    }
}
