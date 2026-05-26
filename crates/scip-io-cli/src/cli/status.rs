use anyhow::Result;
use console::style;

use scip_io_core::indexer::IndexerEntry;
use scip_io_core::indexer::registry::REGISTRY;

use super::StatusArgs;
use super::update::{UpdateCheck, check_entry_for_update};

pub async fn run(args: StatusArgs) -> Result<()> {
    let entries = REGISTRY.all();

    match args.format.as_str() {
        "json" => {
            let mut json_entries: Vec<serde_json::Value> = entries
                .iter()
                .map(|entry| {
                    let action_entry = REGISTRY.action_entry_for(entry).unwrap_or(entry);
                    let covered_by = (action_entry.indexer_name != entry.indexer_name)
                        .then_some(action_entry.indexer_name.as_str());
                    serde_json::json!({
                        "indexer": entry.indexer_name,
                        "language": entry.language_name(),
                        "binary": action_entry.binary_name(),
                        "installed": action_entry.is_installed(),
                        "version": action_entry.installed_version(),
                        "path": action_entry.installed_path().map(|p| p.display().to_string()),
                        "covered_by": covered_by,
                    })
                })
                .collect();

            if args.check_updates {
                for (i, entry) in entries.iter().enumerate() {
                    let action_entry = REGISTRY.action_entry_for(entry).unwrap_or(entry);
                    let check = check_entry_for_update(action_entry).await;
                    if let Some(latest) = check.latest_version {
                        json_entries[i]["latest_version"] = serde_json::Value::String(latest);
                    }
                    json_entries[i]["update_available"] =
                        serde_json::Value::Bool(check.update_available);
                    json_entries[i]["managed"] = serde_json::Value::Bool(check.managed);
                    if let Some(error) = check.error {
                        json_entries[i]["update_check_error"] = serde_json::Value::String(error);
                    }
                }
            }

            println!("{}", serde_json::to_string_pretty(&json_entries)?);
        }
        _ => {
            println!("{} Registered indexers:\n", style("SCIP-IO").cyan().bold());

            for entry in entries {
                let action_entry = REGISTRY.action_entry_for(entry).unwrap_or(entry);
                let installed = action_entry.is_installed();
                let status_icon = if installed {
                    style("v").green().bold()
                } else {
                    style("x").red()
                };

                println!(
                    "  {} {:<14} {}",
                    status_icon,
                    entry.language_name(),
                    status_label(entry, action_entry),
                );

                if args.verbose {
                    println!("    binary:  {}", action_entry.binary_name());
                    if action_entry.indexer_name != entry.indexer_name {
                        println!("    via:     {}", action_entry.indexer_name);
                    }
                    if installed {
                        if let Some(version) = action_entry.installed_version() {
                            println!("    version: {}", version);
                        }
                        if let Some(path) = action_entry.installed_path() {
                            println!("    path:    {}", path.display());
                        }
                    } else {
                        println!("    status:  not installed");
                    }
                    println!();
                }
            }

            if args.check_updates {
                println!("\n{} Checking for updates...\n", style(">").cyan().bold());

                for entry in entries {
                    let action_entry = REGISTRY.action_entry_for(entry).unwrap_or(entry);
                    let check = check_entry_for_update(action_entry).await;
                    print_update_status(entry, action_entry, &check);
                }
            }
        }
    }

    Ok(())
}

fn status_label(entry: &IndexerEntry, action_entry: &IndexerEntry) -> String {
    if action_entry.indexer_name == entry.indexer_name {
        style(&entry.indexer_name).dim().to_string()
    } else {
        format!(
            "{} {} {}",
            style(&entry.indexer_name).dim(),
            style("via").dim(),
            style(&action_entry.indexer_name).dim()
        )
    }
}

/// Display update status for a single indexer entry.
fn print_update_status(entry: &IndexerEntry, action_entry: &IndexerEntry, check: &UpdateCheck) {
    let status_marker = if check.installed {
        style("*").green().to_string()
    } else {
        style("-").dim().to_string()
    };

    let install_label = if check.installed {
        "Installed"
    } else {
        "Not installed"
    };

    if !check.installed {
        println!(
            "  {} {:<20} {:<11} {}",
            status_marker,
            status_label(entry, action_entry),
            "-",
            install_label,
        );
        return;
    }

    if let Some(error) = &check.error {
        println!(
            "  {} {:<20} v{:<10} {} (check failed: {})",
            status_marker,
            status_label(entry, action_entry),
            check
                .current_version
                .as_deref()
                .unwrap_or(&action_entry.version),
            install_label,
            style(error).red(),
        );
        return;
    }

    let current_version = check
        .current_version
        .as_deref()
        .unwrap_or(&action_entry.version);
    let latest_version = check.latest_version.as_deref().unwrap_or("unknown");
    let update_info = if check.update_available {
        format!(
            "(latest: {} {})",
            latest_version,
            style("^ update available").yellow()
        )
    } else if check.installed && !check.managed {
        format!("(latest: {} update manually)", latest_version)
    } else {
        format!("(up to date {})", style("ok").green())
    };

    println!(
        "  {} {:<20} v{:<10} {} {}",
        status_marker,
        status_label(entry, action_entry),
        current_version,
        install_label,
        update_info,
    );
}
