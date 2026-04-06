use anyhow::Result;
use console::style;
use std::fs;

use scip_io_core::indexer::install_dir;
use scip_io_core::indexer::registry::REGISTRY;

use super::CleanArgs;

pub async fn run(args: CleanArgs) -> Result<()> {
    let dir = install_dir();

    if args.all {
        if args.dry_run {
            println!(
                "{} Would remove: {}",
                style("*").yellow(),
                dir.display()
            );
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

    for entry in entries {
        // Filter by --lang if specified
        if let Some(ref lang) = args.lang {
            if !entry.language_name().eq_ignore_ascii_case(lang) {
                continue;
            }
        }

        if let Some(path) = entry.installed_path() {
            if args.dry_run {
                println!(
                    "{} Would remove: {} ({})",
                    style("*").yellow(),
                    path.display(),
                    entry.indexer_name,
                );
            } else {
                fs::remove_file(&path)?;
                println!(
                    "{} Removed: {} ({})",
                    style("v").green(),
                    path.display(),
                    entry.indexer_name,
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
            println!(
                "{} No installed indexers found",
                style("*").yellow(),
            );
        }
    }

    Ok(())
}
