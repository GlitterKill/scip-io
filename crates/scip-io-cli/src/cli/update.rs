use std::io::{self, Write};

use anyhow::{Context, Result, bail};
use console::style;

use scip_io_core::indexer::IndexerEntry;
use scip_io_core::indexer::install::resolve_latest_compatible_version;
use scip_io_core::indexer::version::version_is_newer;

use super::UpdateArgs;
use super::indexer_target::{action_entry_for_target, unique_action_entries};
use super::progress_handler::CliProgressHandler;

#[derive(Debug, Clone)]
pub struct UpdateCheck {
    pub entry: &'static IndexerEntry,
    pub installed: bool,
    pub managed: bool,
    pub current_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub error: Option<String>,
}

enum UpdateSelection {
    All,
    One(usize),
    Cancel,
}

pub async fn run(args: UpdateArgs) -> Result<()> {
    if args.all {
        let checks = collect_installed_update_checks().await;
        print_update_report(&checks);
        return update_all_available(&checks).await;
    }

    if let Some(target) = args.target_identifier() {
        let entry = action_entry_for_target(target)?;
        let check = check_entry_for_update(entry).await;
        print_update_report(std::slice::from_ref(&check));
        return update_one_available(&check).await;
    }

    let checks = collect_installed_update_checks().await;
    print_update_report(&checks);
    let updateable = updateable_checks(&checks);

    if updateable.is_empty() {
        println!(
            "\n{} No managed indexer updates available",
            style("v").green()
        );
        return Ok(());
    }

    match prompt_update_selection(&updateable)? {
        UpdateSelection::All => update_many(&updateable).await,
        UpdateSelection::One(index) => update_one(&updateable[index]).await,
        UpdateSelection::Cancel => {
            println!("{} Update cancelled", style("*").yellow());
            Ok(())
        }
    }
}

pub async fn collect_installed_update_checks() -> Vec<UpdateCheck> {
    let mut checks = Vec::new();
    for entry in unique_action_entries() {
        if entry.is_installed() {
            checks.push(check_entry_for_update(entry).await);
        }
    }
    checks
}

pub async fn check_entry_for_update(entry: &'static IndexerEntry) -> UpdateCheck {
    let installed = entry.is_installed();
    let managed = entry.is_managed_installed();
    let current_version = entry.installed_version();

    if !installed {
        return UpdateCheck {
            entry,
            installed,
            managed,
            current_version,
            latest_version: None,
            update_available: false,
            error: None,
        };
    }

    match resolve_latest_compatible_version(entry).await {
        Ok(latest_version) => {
            let update_available = managed
                && current_version
                    .as_deref()
                    .is_some_and(|current| version_is_newer(&latest_version, current));
            UpdateCheck {
                entry,
                installed,
                managed,
                current_version,
                latest_version: Some(latest_version),
                update_available,
                error: None,
            }
        }
        Err(error) => UpdateCheck {
            entry,
            installed,
            managed,
            current_version,
            latest_version: None,
            update_available: false,
            error: Some(format!("{error:#}")),
        },
    }
}

fn updateable_checks(checks: &[UpdateCheck]) -> Vec<UpdateCheck> {
    checks
        .iter()
        .filter(|check| check.update_available)
        .cloned()
        .collect()
}

fn print_update_report(checks: &[UpdateCheck]) {
    println!("{} Indexer update check:\n", style("SCIP-IO").cyan().bold());

    if checks.is_empty() {
        println!("  {} No installed SCIP indexers found", style("-").dim());
        return;
    }

    for check in checks {
        print_single_update_status(check);
    }
}

fn print_single_update_status(check: &UpdateCheck) {
    let current = check.current_version.as_deref().unwrap_or("unknown");
    let latest = check.latest_version.as_deref().unwrap_or("unknown");

    if !check.installed {
        println!(
            "  {} {:<18} not installed",
            style("-").dim(),
            check.entry.indexer_name
        );
        return;
    }

    if let Some(error) = &check.error {
        println!(
            "  {} {:<18} {:<13} check failed: {}",
            style("!").red().bold(),
            check.entry.indexer_name,
            current,
            style(error).red()
        );
        return;
    }

    if !check.managed {
        println!(
            "  {} {:<18} {:<13} latest {:<13} installed outside SCIP-IO cache",
            style("*").yellow(),
            check.entry.indexer_name,
            current,
            latest
        );
        return;
    }

    if check.update_available {
        println!(
            "  {} {:<18} {:<13} -> {:<13} update available",
            style("^").yellow().bold(),
            check.entry.indexer_name,
            current,
            latest
        );
    } else {
        println!(
            "  {} {:<18} {:<13} latest {:<13} up to date",
            style("v").green(),
            check.entry.indexer_name,
            current,
            latest
        );
    }
}

fn prompt_update_selection(updateable: &[UpdateCheck]) -> Result<UpdateSelection> {
    println!("\nChoose an update to install:");
    if updateable.len() > 1 {
        println!("  0) Update all");
    }
    for (index, check) in updateable.iter().enumerate() {
        println!(
            "  {}) {} {} -> {}",
            index + 1,
            check.entry.indexer_name,
            check.current_version.as_deref().unwrap_or("unknown"),
            check.latest_version.as_deref().unwrap_or("unknown")
        );
    }
    println!("  q) Cancel");

    loop {
        print!("Selection: ");
        io::stdout().flush()?;

        let mut selection = String::new();
        io::stdin().read_line(&mut selection)?;
        let selection = selection.trim();

        if selection.eq_ignore_ascii_case("q") || selection.eq_ignore_ascii_case("cancel") {
            return Ok(UpdateSelection::Cancel);
        }

        if updateable.len() > 1 && selection == "0" {
            return Ok(UpdateSelection::All);
        }

        if let Ok(index) = selection.parse::<usize>()
            && (1..=updateable.len()).contains(&index)
        {
            return Ok(UpdateSelection::One(index - 1));
        }

        println!(
            "{} Enter a number from the list, or q to cancel",
            style("!").red()
        );
    }
}

async fn update_all_available(checks: &[UpdateCheck]) -> Result<()> {
    let updateable = updateable_checks(checks);
    if updateable.is_empty() {
        println!(
            "\n{} No managed indexer updates available",
            style("v").green()
        );
        return Ok(());
    }

    update_many(&updateable).await
}

async fn update_one_available(check: &UpdateCheck) -> Result<()> {
    if !check.installed {
        bail!(
            "{} is not installed. Run `scip-io install {}` first.",
            check.entry.indexer_name,
            check.entry.language_name()
        );
    }
    if !check.managed {
        bail!(
            "{} is installed outside SCIP-IO's managed cache and cannot be updated by SCIP-IO",
            check.entry.indexer_name
        );
    }
    if let Some(error) = &check.error {
        bail!(
            "failed to check latest version for {}: {}",
            check.entry.indexer_name,
            error
        );
    }
    if !check.update_available {
        println!(
            "\n{} {} is already up to date",
            style("v").green(),
            check.entry.indexer_name
        );
        return Ok(());
    }

    update_one(check).await
}

async fn update_many(checks: &[UpdateCheck]) -> Result<()> {
    let mut failures = Vec::new();
    for check in checks {
        if let Err(error) = update_one(check).await {
            eprintln!(
                "{} Failed to update {}: {:#}",
                style("!").red().bold(),
                check.entry.indexer_name,
                error
            );
            failures.push(check.entry.indexer_name.clone());
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        bail!("{} update(s) failed", failures.len())
    }
}

async fn update_one(check: &UpdateCheck) -> Result<()> {
    let latest_version = check
        .latest_version
        .as_deref()
        .context("update check did not include a latest version")?;

    println!(
        "\n{} Updating {} to {}",
        style(">").cyan().bold(),
        check.entry.indexer_name,
        latest_version
    );

    let progress = CliProgressHandler::new();
    let path = check
        .entry
        .update_managed_to_version(latest_version, &progress)
        .await?;

    println!(
        "{} Updated {} to {} at {}",
        style("v").green().bold(),
        check.entry.indexer_name,
        latest_version,
        path.display()
    );

    Ok(())
}
