use anyhow::{Context, Result, bail};
use console::style;

use scip_io_core::indexer::install::resolve_latest_compatible_version;

use super::InstallArgs;
use super::indexer_target::action_entry_for_target;
use super::progress_handler::CliProgressHandler;

pub async fn run(args: InstallArgs) -> Result<()> {
    let target = args.target_identifier()?;
    let entry = action_entry_for_target(target)?;

    if !entry.is_installable() {
        bail!("{} cannot be automatically installed", entry.indexer_name);
    }

    if let Some(path) = entry.installed_path() {
        println!(
            "{} {} is already installed at {}",
            style("*").yellow(),
            entry.indexer_name,
            path.display()
        );
        return Ok(());
    }

    let version = resolve_latest_compatible_version(entry)
        .await
        .with_context(|| {
            format!(
                "failed to resolve latest version for {}",
                entry.indexer_name
            )
        })?;
    println!(
        "{} Installing {} {}",
        style(">").cyan().bold(),
        entry.indexer_name,
        style(&version).dim()
    );

    let progress = CliProgressHandler::new();
    let path = entry.install_version(&version, &progress).await?;

    println!(
        "{} Installed {} {} at {}",
        style("v").green().bold(),
        entry.indexer_name,
        style(&version).dim(),
        path.display()
    );

    Ok(())
}
