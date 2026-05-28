use scip_io_core::config::ProjectConfig;
use scip_io_core::config_discovery::{
    discover_additional_configs, supported_additional_config_languages,
};
use scip_io_core::detect::{Language, scan_languages};
use scip_io_core::indexer::registry::REGISTRY;
use scip_io_core::indexer::version::version_is_newer;
use scip_io_core::indexer::{IndexerEntry, install_dir, is_managed_install_path};
use scip_io_core::progress::{ProgressEvent, ProgressHandler};
use scip_io_core::validate::validate_scip_file;
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Emitter};

#[derive(Debug, Clone, Serialize)]
pub struct LanguageInfo {
    pub name: String,
    pub kind: String,
    pub evidence: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndexerStatusInfo {
    pub name: String,
    pub language: String,
    pub version: String,
    pub binary_name: String,
    pub github_repo: String,
    pub installed: bool,
    pub installable: bool,
    pub managed: bool,
    pub installed_path: Option<String>,
    pub action_indexer: String,
    pub covered_by: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub name: String,
    pub language: String,
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
    pub installed: bool,
    pub managed: bool,
    pub action_indexer: String,
    pub error: Option<String>,
}

struct TauriProgressHandler {
    app: AppHandle,
}

impl TauriProgressHandler {
    /// Emit a frontend-compatible progress event.
    /// The frontend expects objects with a "kind" field for routing.
    fn emit_frontend(&self, payload: serde_json::Value) {
        let _ = self.app.emit("progress", &payload);
    }
}

impl ProgressHandler for TauriProgressHandler {
    fn on_event(&self, event: ProgressEvent) {
        match &event {
            ProgressEvent::DetectStart { .. } => {
                self.emit_frontend(serde_json::json!({
                    "kind": "pipeline_step",
                    "step": "detect",
                    "progress": 5,
                }));
                self.emit_frontend(serde_json::json!({
                    "kind": "log",
                    "level": "info",
                    "message": "Detecting languages...",
                }));
            }
            ProgressEvent::DetectResult { languages } => {
                self.emit_frontend(serde_json::json!({
                    "kind": "pipeline_step",
                    "step": "download",
                    "progress": 15,
                }));
                self.emit_frontend(serde_json::json!({
                    "kind": "log",
                    "level": "info",
                    "message": format!("Detected: {}", languages.join(", ")),
                }));
            }
            ProgressEvent::DownloadStart { indexer, version } => {
                self.emit_frontend(serde_json::json!({
                    "kind": "language_progress",
                    "language": indexer,
                    "status": "downloading",
                    "progress": 0,
                    "message": format!("Downloading {} v{}...", indexer, version),
                }));
                self.emit_frontend(serde_json::json!({
                    "kind": "log",
                    "level": "info",
                    "message": format!("Downloading {} v{}", indexer, version),
                }));
            }
            ProgressEvent::DownloadProgress {
                indexer,
                bytes,
                total,
            } => {
                let pct = total
                    .map(|t| {
                        if t > 0 {
                            (*bytes as f64 / t as f64 * 100.0) as u32
                        } else {
                            0
                        }
                    })
                    .unwrap_or(0);
                self.emit_frontend(serde_json::json!({
                    "kind": "language_progress",
                    "language": indexer,
                    "status": "downloading",
                    "progress": pct,
                    "message": format!("Downloading... {}KB", bytes / 1024),
                }));
            }
            ProgressEvent::DownloadComplete { indexer, path } => {
                self.emit_frontend(serde_json::json!({
                    "kind": "language_progress",
                    "language": indexer,
                    "status": "downloading",
                    "progress": 100,
                    "message": format!("Installed to {}", path.display()),
                }));
                self.emit_frontend(serde_json::json!({
                    "kind": "log",
                    "level": "success",
                    "message": format!("{} installed", indexer),
                }));
            }
            ProgressEvent::IndexerStart { language, command } => {
                self.emit_frontend(serde_json::json!({
                    "kind": "pipeline_step",
                    "step": "index",
                    "progress": 40,
                }));
                self.emit_frontend(serde_json::json!({
                    "kind": "language_progress",
                    "language": language,
                    "status": "running",
                    "progress": 0,
                    "message": format!("Running: {}", command),
                }));
                self.emit_frontend(serde_json::json!({
                    "kind": "log",
                    "level": "info",
                    "message": format!("Indexing {}...", language),
                }));
            }
            ProgressEvent::IndexerOutput { language, line } => {
                self.emit_frontend(serde_json::json!({
                    "kind": "log",
                    "level": "info",
                    "message": format!("[{}] {}", language, line),
                }));
            }
            ProgressEvent::IndexerComplete {
                language,
                duration_secs,
                output,
            } => {
                self.emit_frontend(serde_json::json!({
                    "kind": "language_progress",
                    "language": language,
                    "status": "done",
                    "progress": 100,
                    "message": "Complete",
                    "duration": (*duration_secs * 1000.0) as u64,
                }));
                self.emit_frontend(serde_json::json!({
                    "kind": "log",
                    "level": "success",
                    "message": format!("{} indexed in {:.1}s -> {}", language, duration_secs, output.display()),
                }));
            }
            ProgressEvent::IndexerFailed { language, error } => {
                self.emit_frontend(serde_json::json!({
                    "kind": "language_progress",
                    "language": language,
                    "status": "failed",
                    "progress": 0,
                    "message": error.clone(),
                }));
                self.emit_frontend(serde_json::json!({
                    "kind": "error",
                    "message": format!("{} indexing failed: {}", language, error),
                }));
            }
            ProgressEvent::MergeStart { inputs } => {
                self.emit_frontend(serde_json::json!({
                    "kind": "pipeline_step",
                    "step": "merge",
                    "progress": 85,
                }));
                self.emit_frontend(serde_json::json!({
                    "kind": "log",
                    "level": "info",
                    "message": format!("Merging {} index files...", inputs.len()),
                }));
            }
            ProgressEvent::MergeComplete { output, stats } => {
                self.emit_frontend(serde_json::json!({
                    "kind": "pipeline_step",
                    "step": "done",
                    "progress": 100,
                }));
                self.emit_frontend(serde_json::json!({
                    "kind": "log",
                    "level": "success",
                    "message": format!("Merged -> {} ({} bytes)", output.display(), stats.size_bytes),
                }));
            }
        }
    }
}

static CANCEL_FLAG: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub async fn detect_languages(path: String) -> Result<Vec<LanguageInfo>, String> {
    let root = PathBuf::from(&path);
    if !root.exists() {
        return Err(format!("Path does not exist: {}", path));
    }

    let languages = scan_languages(&root).map_err(|e| e.to_string())?;
    Ok(languages
        .iter()
        .map(|l| LanguageInfo {
            name: l.kind.name().to_string(),
            kind: format!("{:?}", l.kind),
            evidence: l.evidence.clone(),
        })
        .collect())
}

#[tauri::command]
pub async fn start_indexing(
    app: AppHandle,
    path: String,
    languages: Vec<String>,
    output: String,
    include_additional_configs: bool,
) -> Result<(), String> {
    CANCEL_FLAG.store(false, Ordering::SeqCst);

    let root = PathBuf::from(&path);
    let handler = TauriProgressHandler { app: app.clone() };

    // Detect languages
    handler.on_event(ProgressEvent::DetectStart { path: root.clone() });
    let detected = scan_languages(&root).map_err(|e| e.to_string())?;
    let lang_names: Vec<String> = detected.iter().map(|l| l.kind.name().to_string()).collect();
    handler.on_event(ProgressEvent::DetectResult {
        languages: lang_names,
    });

    // Filter languages if specified
    let mut to_index: Vec<_> = if languages.is_empty() {
        detected
    } else {
        detected
            .into_iter()
            .filter(|l| {
                languages
                    .iter()
                    .any(|f| f.eq_ignore_ascii_case(l.kind.name()))
            })
            .collect()
    };
    if include_additional_configs {
        apply_additional_configs(&root, &languages, &mut to_index)?;
    }

    // Dedupe by indexer_name so a tool that handles multiple languages
    // (e.g. scip-typescript for both .ts and .js via `allowJs: true`)
    // is only invoked once. Languages rolled into another task are
    // tracked as `covers` and still reported in the results.
    let plans = build_indexing_plans(&to_index);
    if plans.is_empty() {
        return Err("No registered SCIP indexers found for the selected languages".to_string());
    }

    let mut prepared_plans = Vec::with_capacity(plans.len());
    for plan in plans {
        if CANCEL_FLAG.load(Ordering::SeqCst) {
            return Err("Indexing cancelled".to_string());
        }

        // Log covered languages so the UI can show they're folded
        // into this run instead of silently disappearing.
        for covered in &plan.covers {
            handler.emit_frontend(serde_json::json!({
                "kind": "log",
                "level": "info",
                "message": format!(
                    "{} will be indexed by the {} run (shared tool: {})",
                    covered.kind.name(),
                    plan.primary.kind.name(),
                    plan.entry.indexer_name,
                ),
            }));
        }

        // Preflight installation before any indexer process is invoked. This
        // lets first-run indexing install missing tools and still complete in
        // the same operation.
        let binary = plan
            .entry
            .ensure_installed(&handler)
            .await
            .map_err(|e| format!("Indexer install failed: {}", e))?;

        prepared_plans.push(PreparedIndexingPlan {
            primary: plan.primary,
            entry: plan.entry,
            covers: plan.covers,
            binary,
        });
    }

    let mut outputs = Vec::new();
    let mut lang_results: Vec<serde_json::Value> = Vec::new();
    let indexing_start = std::time::Instant::now();

    for plan in &prepared_plans {
        if CANCEL_FLAG.load(Ordering::SeqCst) {
            return Err("Indexing cancelled".to_string());
        }

        let lang = &plan.primary;
        let entry = plan.entry;

        // Run indexer
        let start = std::time::Instant::now();
        handler.on_event(ProgressEvent::IndexerStart {
            language: lang.kind.name().to_string(),
            command: format!(
                "{} {}",
                plan.binary.display(),
                display_indexer_args(entry, lang, &lang.additional_configs)
            ),
        });

        match scip_io_core::indexer::runner::run_indexer_with_configs(
            &plan.binary,
            entry,
            &root,
            lang,
            &lang.additional_configs,
        )
        .await
        {
            Ok(output_path) => {
                let duration = start.elapsed();
                handler.on_event(ProgressEvent::IndexerComplete {
                    language: lang.kind.name().to_string(),
                    duration_secs: duration.as_secs_f64(),
                    output: output_path.clone(),
                });

                // Read per-language stats from the SCIP output
                let (files, symbols) = match validate_scip_file(&output_path) {
                    Ok(v) => {
                        let s = v.stats.unwrap_or_default();
                        (s.documents, s.symbols)
                    }
                    Err(_) => (0, 0),
                };

                lang_results.push(serde_json::json!({
                    "name": lang.kind.name(),
                    "files": files,
                    "symbols": symbols,
                    "duration": (duration.as_secs_f64() * 1000.0) as u64,
                }));

                // Emit a derived result for each covered language so
                // the UI still shows them. They share the primary's
                // stats because the output file is the same.
                for covered in &plan.covers {
                    lang_results.push(serde_json::json!({
                        "name": covered.kind.name(),
                        "files": files,
                        "symbols": symbols,
                        "duration": (duration.as_secs_f64() * 1000.0) as u64,
                        "coveredBy": lang.kind.name(),
                    }));
                }

                outputs.push(output_path);
            }
            Err(e) => {
                handler.on_event(ProgressEvent::IndexerFailed {
                    language: lang.kind.name().to_string(),
                    error: e.to_string(),
                });
                for covered in &plan.covers {
                    handler.on_event(ProgressEvent::IndexerFailed {
                        language: covered.kind.name().to_string(),
                        error: format!("covered by {} run which failed: {}", lang.kind.name(), e),
                    });
                }
            }
        }
    }

    if outputs.is_empty() {
        return Err("No SCIP output was generated; all selected indexer runs failed".to_string());
    }

    // Determine the final output path (resolve to absolute)
    let output_path = if PathBuf::from(&output).is_absolute() {
        PathBuf::from(&output)
    } else {
        root.join(&output)
    };

    // Merge or copy
    if outputs.len() > 1 {
        handler.on_event(ProgressEvent::MergeStart {
            inputs: outputs.clone(),
        });
        scip_io_core::merge::merge_scip_files(&outputs, &output_path)
            .map_err(|e| format!("Merge failed: {}", e))?;
        scip_io_core::scip_language::compact_scip_file(&output_path)
            .map_err(|e| format!("SCIP compaction failed: {}", e))?;

        let size = std::fs::metadata(&output_path)
            .map(|m| m.len())
            .unwrap_or(0);
        handler.on_event(ProgressEvent::MergeComplete {
            output: output_path.clone(),
            stats: scip_io_core::progress::MergeStats {
                documents: 0,
                symbols: 0,
                size_bytes: size,
            },
        });
    } else if outputs.len() == 1 {
        std::fs::copy(&outputs[0], &output_path)
            .map_err(|e| format!("Failed to write final index: {}", e))?;
        scip_io_core::scip_language::compact_scip_file(&output_path)
            .map_err(|e| format!("SCIP compaction failed: {}", e))?;
    }

    // Read final stats from the output SCIP file
    let (total_files, total_symbols) = match validate_scip_file(&output_path) {
        Ok(v) => {
            let s = v.stats.unwrap_or_default();
            (s.documents, s.symbols)
        }
        Err(_) => (0, 0),
    };

    let total_duration_ms = indexing_start.elapsed().as_millis() as u64;
    let output_size = std::fs::metadata(&output_path)
        .map(|m| m.len())
        .unwrap_or(0);

    handler.emit_frontend(serde_json::json!({
        "kind": "indexing_complete",
        "output": output_path.display().to_string(),
        "total_files": total_files,
        "total_symbols": total_symbols,
        "total_duration": total_duration_ms,
        "languages": lang_results,
        "output_size": output_size,
    }));

    Ok(())
}

#[tauri::command]
pub async fn cancel_indexing() -> Result<(), String> {
    CANCEL_FLAG.store(true, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
pub async fn get_indexer_status() -> Result<Vec<IndexerStatusInfo>, String> {
    let mut seen = BTreeSet::new();
    let mut statuses = Vec::new();

    for entry in REGISTRY.all() {
        if seen.insert(entry.indexer_name.clone()) {
            statuses.push(indexer_status_for_entry(entry));
        }
    }

    Ok(statuses)
}

#[tauri::command]
pub async fn install_indexer(app: AppHandle, indexer: String) -> Result<IndexerStatusInfo, String> {
    let entry = find_indexer_entry(&indexer)
        .ok_or_else(|| format!("No SCIP indexer matches '{indexer}'"))?;
    let action_entry = action_entry_for(entry)
        .ok_or_else(|| format!("No install target registered for '{}'", entry.indexer_name))?;
    if !action_entry.is_installable() {
        return Err(format!(
            "{} cannot be automatically installed",
            entry.indexer_name
        ));
    }

    let handler = TauriProgressHandler { app };
    action_entry
        .ensure_installed(&handler)
        .await
        .map_err(|e| e.to_string())?;

    Ok(indexer_status_for_entry(entry))
}

#[tauri::command]
pub async fn uninstall_indexer(indexer: String) -> Result<IndexerStatusInfo, String> {
    let entry = find_indexer_entry(&indexer)
        .ok_or_else(|| format!("No SCIP indexer matches '{indexer}'"))?;
    let action_entry = action_entry_for(entry).ok_or_else(|| {
        format!(
            "No uninstall target registered for '{}'",
            entry.indexer_name
        )
    })?;
    action_entry
        .uninstall_managed()
        .map_err(|e| e.to_string())?;
    Ok(indexer_status_for_entry(entry))
}

#[tauri::command]
pub async fn update_indexer(
    app: AppHandle,
    indexer: String,
    version: String,
) -> Result<IndexerStatusInfo, String> {
    let entry = find_indexer_entry(&indexer)
        .ok_or_else(|| format!("No SCIP indexer matches '{indexer}'"))?;
    let action_entry = action_entry_for(entry)
        .ok_or_else(|| format!("No update target registered for '{}'", entry.indexer_name))?;

    if !action_entry.is_installed() {
        return Err(format!("{} is not installed", action_entry.indexer_name));
    }
    if !action_entry.is_managed_installed() {
        return Err(format!(
            "{} is installed outside SCIP-IO's managed cache",
            action_entry.indexer_name
        ));
    }

    let handler = TauriProgressHandler { app };
    action_entry
        .update_managed_to_version(&version, &handler)
        .await
        .map_err(|e| e.to_string())?;

    Ok(indexer_status_for_entry(entry))
}

#[tauri::command]
pub async fn get_config(path: String) -> Result<ProjectConfig, String> {
    let root = PathBuf::from(&path);
    ProjectConfig::load(&root).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn save_config(path: String, config: ProjectConfig) -> Result<(), String> {
    let config_path = PathBuf::from(&path).join(".scip-io.toml");
    let toml_str = toml::to_string_pretty(&config).map_err(|e| e.to_string())?;
    std::fs::write(&config_path, toml_str).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn clean_cache(language: Option<String>) -> Result<String, String> {
    let dir = install_dir();
    if let Some(lang) = language {
        let entries = REGISTRY.all();
        for entry in entries {
            if indexer_matches(entry, &lang) {
                let Some(action_entry) = action_entry_for(entry) else {
                    return Ok("No matching managed indexer found".to_string());
                };
                return match action_entry.uninstall_managed() {
                    Ok(Some(path)) => Ok(format!("Removed: {}", path.display())),
                    Ok(None) => Ok("No managed indexer install found".to_string()),
                    Err(e) => Err(e.to_string()),
                };
            }
        }
        Ok("No matching indexer found".to_string())
    } else if dir.exists() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(format!("Removed cache: {}", dir.display()))
    } else {
        Ok("Cache directory not found".to_string())
    }
}

#[tauri::command]
pub async fn validate_index(
    path: String,
) -> Result<scip_io_core::validate::ValidationResult, String> {
    let file_path = PathBuf::from(&path);
    validate_scip_file(&file_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_updates() -> Result<Vec<UpdateInfo>, String> {
    let mut updates = Vec::new();
    let mut seen = BTreeSet::new();

    for entry in REGISTRY.all() {
        let Some(action_entry) = action_entry_for(entry) else {
            continue;
        };
        if !seen.insert(action_entry.indexer_name.clone()) || !action_entry.is_installed() {
            continue;
        }

        let current_version = action_entry
            .installed_version()
            .unwrap_or_else(|| action_entry.version.clone());
        let managed = action_entry.is_managed_installed();
        let latest_result =
            scip_io_core::indexer::install::resolve_latest_compatible_version(action_entry).await;
        let (latest_version, update_available, error) = match latest_result {
            Ok(latest) => (
                latest.clone(),
                managed && version_is_newer(&latest, &current_version),
                None,
            ),
            Err(err) => ("unknown".to_string(), false, Some(err.to_string())),
        };

        updates.push(UpdateInfo {
            name: action_entry.indexer_name.clone(),
            language: languages_for_action_entry(action_entry),
            current_version,
            latest_version,
            update_available,
            installed: true,
            managed,
            action_indexer: action_entry.indexer_name.clone(),
            error,
        });

        // Rate limit
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    Ok(updates)
}

/// Execution plan for a single indexer invocation. `covers` holds extra
/// detected languages whose indexing is folded into the `primary` run —
/// e.g. `scip-typescript` handles both TypeScript and JavaScript in one
/// invocation when `tsconfig.json` has `allowJs: true`.
struct IndexingPlan {
    primary: Language,
    entry: &'static IndexerEntry,
    covers: Vec<Language>,
}

struct PreparedIndexingPlan {
    primary: Language,
    entry: &'static IndexerEntry,
    covers: Vec<Language>,
    binary: PathBuf,
}

/// Group selected languages by their indexer tool and collapse shared
/// tools into one invocation. Within a group the primary is chosen by
/// preferring args without `--infer-tsconfig` (that flag only helps
/// projects lacking a tsconfig.json; when TypeScript and JavaScript are
/// both detected, a tsconfig.json exists and the plain invocation is
/// the right choice).
fn build_indexing_plans(languages: &[Language]) -> Vec<IndexingPlan> {
    let mut plans: Vec<IndexingPlan> = Vec::new();

    for lang in languages {
        let entry = match REGISTRY.runnable_for(lang) {
            Some(e) => e,
            None => continue,
        };

        if let Some(existing) = plans
            .iter_mut()
            .find(|p| p.entry.indexer_name == entry.indexer_name)
        {
            let existing_infer = existing
                .entry
                .default_args
                .iter()
                .any(|x| x == "--infer-tsconfig");
            let new_infer = entry.default_args.iter().any(|x| x == "--infer-tsconfig");

            if existing_infer && !new_infer {
                // The new entry has a cleaner invocation — promote it
                // to primary and demote the old one to covered.
                let demoted = std::mem::replace(&mut existing.primary, lang.clone());
                existing.entry = entry;
                existing
                    .primary
                    .additional_configs
                    .extend(lang.additional_configs.clone());
                existing.primary.additional_configs.sort();
                existing.primary.additional_configs.dedup();
                existing.covers.push(demoted);
            } else {
                existing
                    .primary
                    .additional_configs
                    .extend(lang.additional_configs.clone());
                existing.primary.additional_configs.sort();
                existing.primary.additional_configs.dedup();
                existing.covers.push(lang.clone());
            }
        } else {
            plans.push(IndexingPlan {
                primary: lang.clone(),
                entry,
                covers: Vec::new(),
            });
        }
    }

    plans
}

fn display_indexer_args(
    entry: &IndexerEntry,
    language: &Language,
    additional_configs: &[PathBuf],
) -> String {
    let output_file = PathBuf::from(format!("{}.scip", language.kind.name()));
    scip_io_core::indexer::runner::build_indexer_args(entry, &output_file, additional_configs)
        .into_iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn apply_additional_configs(
    root: &Path,
    filters: &[String],
    languages: &mut Vec<Language>,
) -> Result<(), String> {
    for language in languages.iter_mut() {
        language.additional_configs =
            discover_additional_configs(root, language.kind).map_err(|e| e.to_string())?;
    }

    for &kind in supported_additional_config_languages() {
        if !language_filter_allows(filters, kind) {
            continue;
        }
        if languages.iter().any(|language| language.kind == kind) {
            continue;
        }

        let configs = discover_additional_configs(root, kind).map_err(|e| e.to_string())?;
        if let Some(first_config) = configs.first() {
            languages.push(Language {
                kind,
                evidence: display_relative_path(first_config, root),
                additional_configs: configs,
            });
        }
    }

    Ok(())
}

fn language_filter_allows(filters: &[String], kind: scip_io_core::LanguageKind) -> bool {
    filters.is_empty()
        || filters
            .iter()
            .any(|name| name.eq_ignore_ascii_case(kind.name()))
}

fn display_relative_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .display()
        .to_string()
}

fn find_indexer_entry(identifier: &str) -> Option<&'static IndexerEntry> {
    REGISTRY
        .all()
        .iter()
        .find(|entry| indexer_matches(entry, identifier))
}

fn action_entry_for(entry: &'static IndexerEntry) -> Option<&'static IndexerEntry> {
    REGISTRY.action_entry_for(entry)
}

fn indexer_matches(entry: &IndexerEntry, identifier: &str) -> bool {
    entry.indexer_name.eq_ignore_ascii_case(identifier)
        || entry.binary_name.eq_ignore_ascii_case(identifier)
        || entry.language.eq_ignore_ascii_case(identifier)
}

fn indexer_status_for_entry(entry: &IndexerEntry) -> IndexerStatusInfo {
    let action_entry = REGISTRY.action_entry_for(entry).unwrap_or(entry);
    let languages = languages_for_status_entry(entry);
    let installed_path = action_entry.installed_path();
    let managed = installed_path
        .as_deref()
        .is_some_and(is_managed_install_path);
    let covered_by = (action_entry.indexer_name != entry.indexer_name)
        .then_some(action_entry.indexer_name.clone());

    IndexerStatusInfo {
        name: entry.indexer_name.clone(),
        language: languages,
        version: entry.version.clone(),
        binary_name: action_entry.binary_name.clone(),
        github_repo: entry.github_repo.clone(),
        installed: installed_path.is_some(),
        installable: action_entry.is_installable(),
        managed,
        installed_path: installed_path.map(|p| p.to_string_lossy().to_string()),
        action_indexer: action_entry.indexer_name.clone(),
        covered_by,
    }
}

fn languages_for_status_entry(entry: &IndexerEntry) -> String {
    REGISTRY
        .all()
        .iter()
        .filter(|candidate| candidate.indexer_name == entry.indexer_name)
        .map(|candidate| candidate.language.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn languages_for_action_entry(action_entry: &IndexerEntry) -> String {
    REGISTRY
        .all()
        .iter()
        .filter(|candidate| {
            REGISTRY
                .action_entry_for(candidate)
                .is_some_and(|candidate_action| {
                    candidate_action.indexer_name == action_entry.indexer_name
                })
        })
        .map(|candidate| candidate.language.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use tempfile::TempDir;

    fn fixture(files: &[&str]) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("project");
        fs::create_dir_all(&root).unwrap();
        for file in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "").unwrap();
        }
        (dir, root)
    }

    #[test]
    fn find_indexer_entry_accepts_indexer_binary_and_language_names() {
        assert_eq!(
            find_indexer_entry("scip-typescript").map(|entry| entry.indexer_name.as_str()),
            Some("scip-typescript")
        );
        assert_eq!(
            find_indexer_entry("rust-analyzer").map(|entry| entry.binary_name.as_str()),
            Some("rust-analyzer")
        );
        assert_eq!(
            find_indexer_entry("javascript").map(|entry| entry.indexer_name.as_str()),
            Some("scip-typescript")
        );
    }

    #[tokio::test]
    async fn get_indexer_status_returns_one_row_per_indexer_binary() {
        let statuses = get_indexer_status().await.unwrap();
        let unique_indexers = REGISTRY
            .all()
            .iter()
            .map(|entry| entry.indexer_name.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(statuses.len(), unique_indexers.len());
        assert_eq!(
            statuses
                .iter()
                .filter(|status| status.name == "scip-typescript")
                .count(),
            1
        );
        assert!(
            statuses
                .iter()
                .find(|status| status.name == "scip-typescript")
                .unwrap()
                .language
                .contains("javascript")
        );
        assert!(
            statuses
                .iter()
                .find(|status| status.name == "scip-java")
                .unwrap()
                .language
                .contains("scala")
        );
    }

    #[test]
    fn indexer_status_exposes_installability_for_dashboard_actions() {
        let kotlin = find_indexer_entry("kotlin").unwrap();
        let python = find_indexer_entry("scip-python").unwrap();

        assert!(indexer_status_for_entry(kotlin).installable);
        assert!(indexer_status_for_entry(python).installable);
    }

    #[test]
    fn indexer_status_exposes_kotlin_as_scip_java_proxy_action() {
        let kotlin = find_indexer_entry("kotlin").unwrap();
        let java = find_indexer_entry("scip-java").unwrap();

        let kotlin_status = indexer_status_for_entry(kotlin);
        let java_status = indexer_status_for_entry(java);

        assert_eq!(kotlin_status.name, "scip-kotlin");
        assert_eq!(kotlin_status.binary_name, "scip-java");
        assert_eq!(kotlin_status.action_indexer, "scip-java");
        assert_eq!(kotlin_status.covered_by.as_deref(), Some("scip-java"));
        assert_eq!(kotlin_status.installed, java_status.installed);
        assert_eq!(kotlin_status.managed, java_status.managed);
        assert_eq!(kotlin_status.installed_path, java_status.installed_path);
    }

    #[test]
    fn kotlin_indexing_plan_runs_scip_java() {
        let kotlin = scip_io_core::detect::languages::LanguageKind::Kotlin
            .with_evidence("build.gradle.kts".into());
        let plans = build_indexing_plans(&[kotlin]);

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].primary.kind.name(), "kotlin");
        assert_eq!(plans[0].entry.indexer_name, "scip-java");
    }

    #[test]
    fn java_and_kotlin_share_one_scip_java_plan() {
        let java =
            scip_io_core::detect::languages::LanguageKind::Java.with_evidence("pom.xml".into());
        let kotlin = scip_io_core::detect::languages::LanguageKind::Kotlin
            .with_evidence("build.gradle.kts".into());
        let plans = build_indexing_plans(&[java, kotlin]);

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].entry.indexer_name, "scip-java");
        assert!(
            plans[0]
                .covers
                .iter()
                .any(|lang| lang.kind.name() == "kotlin")
        );
    }

    #[test]
    fn shared_indexing_plan_keeps_additional_configs_on_primary_run() {
        let javascript = scip_io_core::detect::languages::LanguageKind::JavaScript
            .with_evidence("package.json".into());
        let mut typescript = scip_io_core::detect::languages::LanguageKind::TypeScript
            .with_evidence("tsconfig.json".into());
        typescript.additional_configs = vec![
            PathBuf::from("tsconfig.json"),
            PathBuf::from("tsconfig.test.json"),
        ];

        let plans = build_indexing_plans(&[javascript, typescript]);

        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].primary.kind.name(), "typescript");
        assert_eq!(
            plans[0].primary.additional_configs,
            vec![
                PathBuf::from("tsconfig.json"),
                PathBuf::from("tsconfig.test.json")
            ]
        );
    }

    #[test]
    fn apply_additional_configs_adds_config_only_typescript_root() {
        let (_dir, root) = fixture(&["tsconfig.scripts.json"]);
        let mut languages = Vec::new();

        apply_additional_configs(&root, &[], &mut languages).unwrap();

        assert_eq!(languages.len(), 1);
        assert_eq!(languages[0].kind.name(), "typescript");
        assert_eq!(languages[0].evidence, "tsconfig.scripts.json");
        assert_eq!(
            languages[0].additional_configs,
            vec![root.join("tsconfig.scripts.json")]
        );
    }

    #[test]
    fn apply_additional_configs_respects_selected_language_filter() {
        let (_dir, root) = fixture(&["tsconfig.scripts.json"]);
        let mut languages = Vec::new();

        apply_additional_configs(&root, &["rust".to_string()], &mut languages).unwrap();

        assert!(languages.is_empty());
    }

    #[test]
    fn update_language_summary_uses_logical_languages_for_action_indexer() {
        let java = find_indexer_entry("scip-java").unwrap();

        let languages = languages_for_action_entry(java);

        assert!(languages.contains("java"));
        assert!(languages.contains("scala"));
        assert!(languages.contains("kotlin"));
    }
}

#[tauri::command]
pub async fn reveal_in_explorer(path: String) -> Result<(), String> {
    let p = std::path::Path::new(&path);
    let dir = if p.is_file() {
        p.parent().unwrap_or(p)
    } else {
        p
    };

    #[cfg(target_os = "windows")]
    {
        // On Windows, use explorer /select to highlight the file
        if p.is_file() {
            std::process::Command::new("explorer")
                .args(["/select,", &path])
                .spawn()
                .map_err(|e| e.to_string())?;
        } else {
            std::process::Command::new("explorer")
                .arg(dir)
                .spawn()
                .map_err(|e| e.to_string())?;
        }
    }

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg("-R")
            .arg(&path)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(dir)
            .spawn()
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}
