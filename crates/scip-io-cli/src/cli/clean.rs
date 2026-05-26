use anyhow::Result;
use console::style;
use std::collections::BTreeSet;
use std::fs;

use scip_io_core::indexer::registry::REGISTRY;
use scip_io_core::indexer::{IndexerEntry, install_dir};

use super::CleanArgs;

pub async fn run(args: CleanArgs) -> Result<()> {
    let dir = install_dir();

    if args.all {
        if args.dry_run {
            println!("{} Would remove: {}", style("*").yellow(), dir.display());
        } else if dir.exists() {
            fs::remove_dir_all(&dir)?;
            println!(
                "{} Removed cache directory: {}",
                style("v").green().bold(),
                dir.display()
            );
        } else {
            println!(
                "{} Cache directory not found: {}",
                style("*").yellow(),
                dir.display()
            );
        }
        return Ok(());
    }

    let entries = REGISTRY.all();
    let mut removed = 0;
    let mut seen = BTreeSet::new();

    for entry in entries {
        if !seen.insert(entry.indexer_name.clone()) {
            continue;
        }

        // Filter by --lang if specified
        if let Some(ref lang) = args.lang
            && !entry_matches_clean_filter(entry, lang)
        {
            continue;
        }

        let action_entry = REGISTRY.action_entry_for(entry).unwrap_or(entry);
        if let Some(path) = action_entry.installed_path() {
            if !action_entry.is_managed_installed() {
                if args.lang.is_some() {
                    println!(
                        "{} Skipping {} at {} because it is outside the SCIP-IO cache",
                        style("*").yellow(),
                        action_entry.indexer_name,
                        path.display(),
                    );
                }
                continue;
            }

            if args.dry_run {
                println!(
                    "{} Would remove: {} ({})",
                    style("*").yellow(),
                    path.display(),
                    action_entry.indexer_name,
                );
            } else {
                action_entry.uninstall_managed()?;
                println!(
                    "{} Removed: {} ({})",
                    style("v").green(),
                    path.display(),
                    action_entry.indexer_name,
                );
            }
            removed += 1;
        }
    }

    if removed == 0 {
        if let Some(ref lang) = args.lang {
            println!(
                "{} No installed indexer found for '{}'",
                style("*").yellow(),
                lang,
            );
        } else {
            println!("{} No installed indexers found", style("*").yellow(),);
        }
    }

    Ok(())
}

fn entry_matches_clean_filter(entry: &IndexerEntry, filter: &str) -> bool {
    entry.indexer_name.eq_ignore_ascii_case(filter)
        || entry.binary_name().eq_ignore_ascii_case(filter)
        || REGISTRY
            .all()
            .iter()
            .filter(|candidate| candidate.indexer_name == entry.indexer_name)
            .any(|candidate| candidate.language_name().eq_ignore_ascii_case(filter))
}
