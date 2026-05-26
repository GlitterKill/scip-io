use anyhow::Result;
use console::style;

use super::UninstallArgs;
use super::indexer_target::action_entry_for_target;

pub async fn run(args: UninstallArgs) -> Result<()> {
    let target = args.target_identifier()?;
    let entry = action_entry_for_target(target)?;

    let Some(path) = entry.installed_path() else {
        println!(
            "{} No installed indexer found for '{}'",
            style("*").yellow(),
            target
        );
        return Ok(());
    };

    if args.dry_run {
        if entry.is_managed_installed() {
            println!(
                "{} Would remove {} ({})",
                style("*").yellow(),
                path.display(),
                entry.indexer_name
            );
        } else {
            println!(
                "{} Would skip {} at {} because it is outside the SCIP-IO cache",
                style("*").yellow(),
                entry.indexer_name,
                path.display()
            );
        }
        return Ok(());
    }

    entry.uninstall_managed()?;
    println!(
        "{} Removed {} ({})",
        style("v").green().bold(),
        path.display(),
        entry.indexer_name
    );

    Ok(())
}
