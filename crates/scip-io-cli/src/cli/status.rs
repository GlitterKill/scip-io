use anyhow::{Context, Result};
use console::style;

use scip_io_core::config::ProjectConfig;
use scip_io_core::indexer::IndexerEntry;
use scip_io_core::indexer::backend::{
    ExecutionBackendKind, backend_availability_for_entry_with_preference,
};
use scip_io_core::indexer::registry::REGISTRY;
use scip_io_core::toolchain::{ToolchainPreflight, toolchain_preflight_for_indexer};

use super::StatusArgs;
use super::update::{UpdateCheck, check_entry_for_update};

pub async fn run(args: StatusArgs) -> Result<()> {
    let entries = REGISTRY.all();
    let config = load_status_config()?;

    match args.format.as_str() {
        "json" => {
            let mut json_entries: Vec<serde_json::Value> = Vec::new();
            for entry in entries {
                let action_entry = REGISTRY.action_entry_for(entry).unwrap_or(entry);
                let covered_by = (action_entry.indexer_name != entry.indexer_name)
                    .then_some(action_entry.indexer_name.as_str());
                let backend_preference = config
                    .backend_preference_for(entry.language_name(), &action_entry.indexer_name);
                let probes = backend_availability_for_entry_with_preference(
                    action_entry,
                    &backend_preference,
                )
                .await;
                let toolchain = toolchain_preflight_for_indexer(action_entry, &config.toolchains);
                json_entries.push(serde_json::json!({
                        "indexer": entry.indexer_name,
                        "language": entry.language_name(),
                        "binary": action_entry.binary_name(),
                        "installed": action_entry.is_installed(),
                        "native_supported": action_entry.native_supported_on_current_platform(),
                        "native_installed": action_entry.is_installed() && action_entry.native_supported_on_current_platform(),
                        "native_unsupported_reason": action_entry.windows_native_unsupported_reason(),
                        "backend_support": action_entry.backend_capabilities.backend_names(),
                        "selected_backend": backend_kind_label(backend_preference.kind),
                        "selected_docker_image": backend_preference.docker_image.as_deref(),
                        "selected_wsl_distro": backend_preference.wsl_distro.as_deref(),
                        "backend_available": probes.iter().any(|probe| probe.available),
                        "backend_probes": probes.iter().map(|probe| {
                            serde_json::json!({
                                "kind": format!("{:?}", probe.kind).to_ascii_lowercase(),
                                "available": probe.available,
                                "detail": probe.detail.as_deref(),
                            })
                        }).collect::<Vec<_>>(),
                        "version": action_entry.installed_version(),
                        "path": action_entry.installed_path().map(|p| p.display().to_string()),
                        "covered_by": covered_by,
                        "toolchain_required": toolchain.as_ref().map(|status| status.kind.as_str()),
                        "toolchain_available": toolchain.as_ref().map(|status| status.available),
                        "toolchain_source": toolchain.as_ref().map(|status| status.source.as_str()),
                        "toolchain_home": toolchain.as_ref().and_then(|status| status.home.as_ref()).map(|path| path.display().to_string()),
                        "toolchain_executable": toolchain.as_ref().and_then(|status| status.executable.as_ref()).map(|path| path.display().to_string()),
                        "toolchain_message": toolchain.as_ref().map(|status| status.message.as_str()),
                    }));
            }

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
                    println!(
                        "    native:  {}",
                        if action_entry.native_supported_on_current_platform() {
                            "supported"
                        } else {
                            "unsupported"
                        }
                    );
                    if let Some(reason) = action_entry.windows_native_unsupported_reason() {
                        println!("    reason:  {}", reason);
                    }
                    let backend_names = action_entry.backend_capabilities.backend_names();
                    let backend_preference = config
                        .backend_preference_for(entry.language_name(), &action_entry.indexer_name);
                    if !backend_names.is_empty() {
                        let probes = backend_availability_for_entry_with_preference(
                            action_entry,
                            &backend_preference,
                        )
                        .await;
                        let available = probes
                            .iter()
                            .filter(|probe| probe.available)
                            .map(|probe| format!("{:?}", probe.kind).to_ascii_lowercase())
                            .collect::<Vec<_>>();
                        println!("    backend: {}", backend_names.join(", "));
                        println!(
                            "    selected_backend: {}",
                            backend_kind_label(backend_preference.kind)
                        );
                        if let Some(distro) = &backend_preference.wsl_distro {
                            println!("    selected_wsl_distro: {}", distro);
                        }
                        if let Some(image) = &backend_preference.docker_image {
                            println!("    selected_docker_image: {}", image);
                        }
                        println!(
                            "    available: {}",
                            if available.is_empty() {
                                "none".to_string()
                            } else {
                                available.join(", ")
                            }
                        );
                    }
                    if action_entry.indexer_name != entry.indexer_name {
                        println!("    via:     {}", action_entry.indexer_name);
                    }
                    if let Some(toolchain) =
                        toolchain_preflight_for_indexer(action_entry, &config.toolchains)
                    {
                        print_toolchain_status(&toolchain);
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

fn load_status_config() -> Result<ProjectConfig> {
    let cwd = std::env::current_dir().context("Failed to resolve current directory")?;
    ProjectConfig::load(&cwd).with_context(|| {
        format!(
            "Failed to load project config for status from {}",
            cwd.join(".scip-io.toml").display()
        )
    })
}

fn backend_kind_label(kind: ExecutionBackendKind) -> &'static str {
    match kind {
        ExecutionBackendKind::Auto => "auto",
        ExecutionBackendKind::Native => "native",
        ExecutionBackendKind::Wsl => "wsl",
        ExecutionBackendKind::Docker => "docker",
        ExecutionBackendKind::Disabled => "disabled",
    }
}

fn print_toolchain_status(toolchain: &ToolchainPreflight) {
    println!(
        "    toolchain: {} ({})",
        toolchain.kind.display_name(),
        if toolchain.available {
            "ready"
        } else {
            "missing"
        }
    );
    println!("    toolchain_source: {}", toolchain.source.as_str());
    if let Some(home) = &toolchain.home {
        println!("    toolchain_home: {}", home.display());
    }
    if let Some(executable) = &toolchain.executable {
        println!("    toolchain_executable: {}", executable.display());
    }
    if !toolchain.available {
        println!("    toolchain_reason: {}", toolchain.message);
    }
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
