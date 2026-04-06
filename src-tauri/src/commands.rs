use scip_io_core::config::ProjectConfig;
use scip_io_core::detect::scan_languages;
use scip_io_core::indexer::registry::REGISTRY;
use scip_io_core::indexer::install_dir;
use scip_io_core::validate::validate_scip_file;
use scip_io_core::progress::{ProgressEvent, ProgressHandler};
use serde::Serialize;
use std::path::PathBuf;
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
    pub installed_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub name: String,
    pub language: String,
    pub current_version: String,
    pub latest_version: String,
    pub update_available: bool,
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
            ProgressEvent::DownloadProgress { indexer, bytes, total } => {
                let pct = total.map(|t| if t > 0 { (*bytes as f64 / t as f64 * 100.0) as u32 } else { 0 }).unwrap_or(0);
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
            ProgressEvent::IndexerComplete { language, duration_secs, output } => {
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
    let to_index: Vec<_> = if languages.is_empty() {
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

    let mut outputs = Vec::new();
    let mut lang_results: Vec<serde_json::Value> = Vec::new();
    let indexing_start = std::time::Instant::now();

    for lang in &to_index {
        if CANCEL_FLAG.load(Ordering::SeqCst) {
            return Err("Indexing cancelled".to_string());
        }

        if let Some(entry) = REGISTRY.get(lang) {
            // Download indexer
            let binary = entry
                .ensure_installed(&handler)
                .await
                .map_err(|e| format!("Download failed: {}", e))?;

            // Run indexer
            let start = std::time::Instant::now();
            handler.on_event(ProgressEvent::IndexerStart {
                language: lang.kind.name().to_string(),
                command: format!("{} {}", binary.display(), entry.default_args.join(" ")),
            });

            match scip_io_core::indexer::runner::run_indexer(&binary, entry, &root, lang).await {
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
                    outputs.push(output_path);
                }
                Err(e) => {
                    handler.on_event(ProgressEvent::IndexerFailed {
                        language: lang.kind.name().to_string(),
                        error: e.to_string(),
                    });
                }
            }
        }
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
        let _ = std::fs::copy(&outputs[0], &output_path);
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
    let entries = REGISTRY.all();
    Ok(entries
        .iter()
        .map(|e| {
            let installed_path = e.installed_path();
            IndexerStatusInfo {
                name: e.indexer_name.clone(),
                language: e.language.clone(),
                version: e.version.clone(),
                binary_name: e.binary_name.clone(),
                github_repo: e.github_repo.clone(),
                installed: installed_path.is_some(),
                installed_path: installed_path.map(|p| p.to_string_lossy().to_string()),
            }
        })
        .collect())
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
            if entry.language.eq_ignore_ascii_case(&lang) {
                if let Some(path) = entry.installed_path() {
                    std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                    return Ok(format!("Removed: {}", path.display()));
                }
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
pub async fn validate_index(path: String) -> Result<scip_io_core::validate::ValidationResult, String> {
    let file_path = PathBuf::from(&path);
    validate_scip_file(&file_path).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn check_updates() -> Result<Vec<UpdateInfo>, String> {
    let mut updates = Vec::new();
    let client = reqwest::Client::new();

    for entry in REGISTRY.all() {
        let url = format!(
            "https://api.github.com/repos/{}/releases/latest",
            entry.github_repo
        );

        let latest = match client
            .get(&url)
            .header("User-Agent", "scip-io")
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<serde_json::Value>().await {
                    Ok(json) => json["tag_name"]
                        .as_str()
                        .unwrap_or("unknown")
                        .trim_start_matches('v')
                        .to_string(),
                    Err(_) => "unknown".to_string(),
                }
            }
            _ => "check failed".to_string(),
        };

        updates.push(UpdateInfo {
            name: entry.indexer_name.clone(),
            language: entry.language.clone(),
            current_version: entry.version.clone(),
            latest_version: latest.clone(),
            update_available: latest != entry.version
                && latest != "unknown"
                && latest != "check failed",
        });

        // Rate limit
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    Ok(updates)
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
