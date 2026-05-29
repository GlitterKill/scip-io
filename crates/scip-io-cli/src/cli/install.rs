use anyhow::{Context, Result, bail};
use console::style;

use scip_io_core::config::ProjectConfig;
use scip_io_core::indexer::install::resolve_latest_compatible_version;
use scip_io_core::toolchain::toolchain_preflight_for_indexer;

use super::InstallArgs;
use super::indexer_target::action_entry_for_target;
use super::progress_handler::CliProgressHandler;

pub async fn run(args: InstallArgs) -> Result<()> {
    let target = args.target_identifier()?;
    let entry = action_entry_for_target(target)?;

    if !entry.is_installable() {
        bail!("{} cannot be automatically installed", entry.indexer_name);
    }

    if !entry.native_supported_on_current_platform() {
        let reason = entry
            .windows_native_unsupported_reason()
            .unwrap_or("native binary is unavailable on this platform");
        bail!(
            "{} has no native Windows install: {}. `scip-io index` can use WSL or Docker for this indexer when a backend is configured or available.",
            entry.indexer_name,
            reason
        );
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
    let config = std::env::current_dir()
        .ok()
        .and_then(|path| ProjectConfig::load(&path).ok())
        .unwrap_or_default();
    if let Some(toolchain) = toolchain_preflight_for_indexer(entry, &config.toolchains)
        && !toolchain.available
    {
        println!(
            "{} {} installed, but indexing also needs {}: {}",
            style("!").yellow().bold(),
            entry.indexer_name,
            toolchain.kind.display_name(),
            toolchain.message
        );
    }

    Ok(())
}
