use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use console::style;
use futures_util::stream::{self, StreamExt};

use scip_io_core::cmake_compile_databases::{
    CmakeCompileDatabaseGenerationPlan, cmake_compile_database_generation_enabled,
    generate_cmake_compile_databases_with_backend, plan_cmake_compile_database_generation,
};
use scip_io_core::compile_commands::{
    CompileCommandCoverageOptions, CompileCommandDatabaseSkip, discover_compile_command_databases,
    select_compile_command_databases, summarize_compile_command_databases,
};
use scip_io_core::config::{CmakeCompileDatabaseConfig, IndexScope, ProjectConfig};
use scip_io_core::config_discovery::{
    discover_additional_configs, supported_additional_config_languages,
};
use scip_io_core::detect::{
    DetectionEvidenceKind, Language, LanguageScanOptions, scan_languages_with_options,
};
use scip_io_core::indexer::backend::{BackendPreference, ExecutionBackendKind};
use scip_io_core::indexer::registry::REGISTRY;
use scip_io_core::indexer::{IndexerEntry, runner};
use scip_io_core::merge::merge_scip_files_atomically_with_project_root;
use scip_io_core::scip_language::{
    compact_scip_file, copy_scip_file_atomically, prefix_scip_file_document_paths,
    prune_scip_file_document_paths_with_prefixes,
};
use scip_io_core::scope::{IndexScopeResolution, resolve_indexing_roots};
use scip_io_core::toolchain::toolchain_preflight_for_indexer;

use super::IndexArgs;
use super::progress_handler::CliProgressHandler;

/// A single indexer task to be executed.
struct IndexerTask {
    lang: Language,
    entry: &'static IndexerEntry,
    binary_path: Option<PathBuf>,
    project_root: PathBuf,
    additional_configs: Vec<PathBuf>,
    owned_child_prefixes: Vec<String>,
    backend_preference: BackendPreference,
    args_override: Option<Vec<String>>,
    /// Additional detected languages whose indexing is handled by the same
    /// tool invocation (e.g. `javascript` is covered by a single
    /// `scip-typescript` run when `tsconfig.json` has `allowJs: true`).
    covers: Vec<String>,
}

/// Languages detected for one project root.
struct ProjectLanguages {
    root: PathBuf,
    languages: Vec<Language>,
    owned_child_prefixes: Vec<String>,
}

/// Result of running a single indexer.
struct IndexerResult {
    lang_name: String,
    covers: Vec<String>,
    outcome: Result<PathBuf>,
}

pub async fn run(args: IndexArgs) -> Result<()> {
    let path = args
        .path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let path = path.canonicalize()?;
    let config = ProjectConfig::load(&path)?;

    let project_roots = resolve_project_roots_with_config(&args, &path, &config)?;
    let is_json = args.format == "json";
    let cmake_generation_plans =
        plan_cmake_compile_database_generation_for_roots(&args, &config, &project_roots)?;
    if !args.dry_run {
        generate_cmake_compile_databases_for_roots(&args, &config, &project_roots, is_json).await?;
    }
    let projects = detect_languages_for_roots_with_config(&args, &project_roots, &config)?;
    let total_languages = projects
        .iter()
        .map(|project| project.languages.len())
        .sum::<usize>();

    if total_languages == 0 {
        bail!("No supported languages found to index");
    }

    // Dry-run mode: show what would be done then exit
    if args.dry_run {
        return run_dry_run(&args, &projects, is_json, &config, &cmake_generation_plans);
    }

    if !is_json {
        print_index_plan(&projects, &path, effective_scope(&args, &config));
    }

    let progress = Arc::new(CliProgressHandler::new());

    let mut failures = collect_unready_language_failures(&projects);
    if !is_json {
        for (language, error) in &failures {
            eprintln!("  {} {} skipped: {}", style("!").yellow(), language, error);
        }
    }

    // Phase 1: Ensure all indexers are installed (sequentially, to avoid
    // duplicate downloads of the same binary)
    let mut tasks = Vec::new();
    for project in &projects {
        for lang in &project.languages {
            if !lang.indexer_ready {
                continue;
            }
            let entry = REGISTRY
                .runnable_for(lang)
                .with_context(|| format!("No indexer registered for {}", lang.name()))?;

            let backend_preference =
                config.backend_preference_for(lang.name(), &entry.indexer_name);
            let args_override = config.args_override_for(lang.name(), &entry.indexer_name);
            let binary_path = if should_prepare_native_binary(entry, &backend_preference) {
                Some(entry.ensure_installed(progress.as_ref()).await?)
            } else {
                None
            };

            tasks.push(IndexerTask {
                lang: lang.clone(),
                entry,
                binary_path,
                project_root: project.root.clone(),
                additional_configs: lang.additional_configs.clone(),
                owned_child_prefixes: project.owned_child_prefixes.clone(),
                backend_preference,
                args_override,
                covers: Vec::new(),
            });
        }
    }

    // Dedupe duplicate tasks for the same language/root/indexer. Languages
    // that share a binary still need separate invocations when their project
    // arguments differ, such as scip-typescript's TypeScript and JavaScript
    // modes.
    let tasks = dedupe_tasks_by_indexer(tasks, is_json);

    // Phase 2: Run indexers in parallel with timeout
    let parallel = args.parallel.unwrap_or(4) as usize;
    let timeout_secs = args.timeout.unwrap_or(600);
    let timeout_duration = Duration::from_secs(timeout_secs);

    if !is_json {
        println!(
            "  {} Running {} indexer(s) (parallel={}, timeout={}s)...",
            style(">").cyan(),
            tasks.len(),
            parallel,
            timeout_secs,
        );
    }

    let base_path_for_results = path.clone();
    let toolchains_for_results = Arc::new(config.toolchains.clone());
    let results: Vec<IndexerResult> = stream::iter(tasks)
        .map(|task| {
            let dur = timeout_duration;
            let base_path = base_path_for_results.clone();
            let toolchains = Arc::clone(&toolchains_for_results);
            async move {
                let lang_name = task.lang.name().to_string();
                let covers = task.covers.clone();
                let outcome = tokio::time::timeout(
                    dur,
                    runner::run_indexer_with_request(runner::IndexerRunRequest {
                        binary: task.binary_path.as_deref(),
                        entry: task.entry,
                        project_root: &task.project_root,
                        lang: &task.lang,
                        config_paths: &task.additional_configs,
                        backend_preference: task.backend_preference.clone(),
                        toolchains: &toolchains,
                        args_override: task.args_override.as_deref(),
                    }),
                )
                .await;
                match outcome {
                    Ok(Ok(output)) => {
                        let outcome =
                            prune_nested_project_documents(&output, &task.owned_child_prefixes)
                                .and_then(|_| {
                                    prefix_output_paths_for_project_root(
                                        &output,
                                        &task.project_root,
                                        &base_path,
                                    )
                                })
                                .and_then(|_| compact_scip_file(&output).map(|_| ()))
                                .map(|_| output);
                        IndexerResult {
                            lang_name,
                            covers,
                            outcome,
                        }
                    }
                    Ok(Err(err)) => IndexerResult {
                        lang_name,
                        covers,
                        outcome: Err(err),
                    },
                    Err(_) => IndexerResult {
                        lang_name: lang_name.clone(),
                        covers,
                        outcome: Err(anyhow::anyhow!(
                            "Indexer for {} timed out after {}s",
                            lang_name,
                            dur.as_secs()
                        )),
                    },
                }
            }
        })
        .buffer_unordered(parallel)
        .collect()
        .await;

    // Collect successes and failures
    let mut scip_outputs = Vec::new();

    for result in results {
        match result.outcome {
            Ok(output) => {
                if !is_json {
                    println!(
                        "  {} {} -> {}",
                        style("v").green(),
                        result.lang_name,
                        output.display()
                    );
                    for covered in &result.covers {
                        println!(
                            "    {} {} covered by {} run",
                            style("->").dim(),
                            covered,
                            result.lang_name
                        );
                    }
                }
                scip_outputs.push(output);
            }
            Err(err) => {
                if !is_json {
                    eprintln!(
                        "  {} {} failed: {}",
                        style("x").red(),
                        result.lang_name,
                        err
                    );
                    for covered in &result.covers {
                        eprintln!(
                            "    {} {} also failed (covered by {} run)",
                            style("->").dim(),
                            covered,
                            result.lang_name
                        );
                    }
                }
                failures.push((result.lang_name.clone(), format!("{:#}", err)));
                for covered in &result.covers {
                    failures.push((covered.clone(), format!("{:#}", err)));
                }
            }
        }
    }

    // Merge if needed (only successful outputs)
    if !args.no_merge && scip_outputs.len() > 1 {
        if !is_json {
            println!(
                "\n{} Merging {} index files...",
                style(">").cyan().bold(),
                scip_outputs.len()
            );
        }
        merge_scip_files_atomically_with_project_root(&scip_outputs, &args.output, &path)?;
        if !is_json {
            println!(
                "{} {}",
                style("v").green().bold(),
                index_publication_message(&args.output, scip_outputs.len(), failures.len(), true)
            );
        }
    } else if scip_outputs.len() == 1 && !args.no_merge {
        copy_scip_file_atomically(&scip_outputs[0], &args.output)?;
        if !is_json {
            println!(
                "\n{} {}",
                style("v").green().bold(),
                index_publication_message(&args.output, scip_outputs.len(), failures.len(), false)
            );
        }
    }

    if is_json {
        let result = serde_json::json!({
            "languages": unique_language_names(&projects),
            "projects": projects.iter().map(|project| {
                serde_json::json!({
                    "root": project.root.display().to_string(),
                    "languages": project.languages.iter().map(|l| l.name()).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
            "outputs": scip_outputs.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
            "merged": if !args.no_merge && !scip_outputs.is_empty() {
                Some(args.output.display().to_string())
            } else {
                None
            },
            "partial": !failures.is_empty(),
            "successful_outputs": scip_outputs.len(),
            "failed_languages": failures.len(),
            "failures": failures.iter().map(|(lang, err)| {
                serde_json::json!({ "language": lang, "error": err })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
    }

    // Exit with appropriate status
    if !failures.is_empty() {
        if scip_outputs.is_empty() {
            // Total failure
            bail!("All {} indexer(s) failed", failures.len());
        } else {
            // Partial failure — report but don't bail so merge output is kept
            eprintln!(
                "\n{} Partial index: {} successful output(s), {} failed language(s)",
                style("!").yellow().bold(),
                scip_outputs.len(),
                failures.len(),
            );
            // Return a partial-failure error so main.rs can set exit code 1
            bail!(
                "partial-failure: {} successful output(s), {} failed language(s)",
                scip_outputs.len(),
                failures.len()
            );
        }
    }

    Ok(())
}

/// Resolve which project roots the index command should operate on.
#[cfg(test)]
fn resolve_project_roots(args: &IndexArgs, base_path: &Path) -> Result<Vec<PathBuf>> {
    resolve_project_roots_with_config(args, base_path, &ProjectConfig::default())
}

fn resolve_project_roots_with_config(
    args: &IndexArgs,
    base_path: &Path,
    config: &ProjectConfig,
) -> Result<Vec<PathBuf>> {
    Ok(resolve_indexing_roots(IndexScopeResolution {
        base_path,
        scope: effective_scope(args, config),
        explicit_roots: &args.roots,
        all_roots: args.all_roots,
        include_additional_configs: effective_include_additional_configs(args, config),
        language_filters: &args.lang,
    })?
    .into_iter()
    .map(|resolved| resolved.root)
    .collect())
}

fn effective_scope(args: &IndexArgs, config: &ProjectConfig) -> IndexScope {
    if args.all_roots {
        return IndexScope::Configs;
    }
    args.scope.or(config.scope).unwrap_or_default()
}

fn effective_include_additional_configs(args: &IndexArgs, config: &ProjectConfig) -> bool {
    args.include_additional_configs
        || config.include_additional_configs.unwrap_or(false)
        || effective_cmake_compile_database_config(args, config)
            .as_ref()
            .is_some_and(cmake_compile_database_generation_enabled)
}

fn effective_cmake_compile_database_config(
    args: &IndexArgs,
    config: &ProjectConfig,
) -> Option<CmakeCompileDatabaseConfig> {
    let mut cmake = config
        .cpp
        .as_ref()
        .and_then(|cpp| cpp.cmake.clone())
        .unwrap_or_default();
    if args.generate_cmake_compile_dbs {
        cmake.generate_compile_databases = Some(true);
    }
    if let Some(preset) = args.cmake_preset {
        cmake.preset = Some(preset);
    }
    if cmake_compile_database_generation_enabled(&cmake) {
        Some(cmake)
    } else {
        None
    }
}

fn plan_cmake_compile_database_generation_for_roots(
    args: &IndexArgs,
    config: &ProjectConfig,
    roots: &[PathBuf],
) -> Result<Vec<(PathBuf, CmakeCompileDatabaseGenerationPlan)>> {
    let Some(cmake_config) = effective_cmake_compile_database_config(args, config) else {
        return Ok(Vec::new());
    };
    roots
        .iter()
        .map(|root| {
            plan_cmake_compile_database_generation(root, &cmake_config)
                .map(|plan| (root.clone(), plan))
        })
        .collect()
}

async fn generate_cmake_compile_databases_for_roots(
    args: &IndexArgs,
    config: &ProjectConfig,
    roots: &[PathBuf],
    is_json: bool,
) -> Result<()> {
    let Some(cmake_config) = effective_cmake_compile_database_config(args, config) else {
        return Ok(());
    };

    let backend_preference = config.backend_preference_for("cpp", "scip-clang");
    for root in roots {
        let report =
            generate_cmake_compile_databases_with_backend(root, &cmake_config, &backend_preference)
                .await?;
        if is_json {
            continue;
        }
        if report.planned_jobs == 0 {
            continue;
        }
        println!(
            "  {} CMake compile DBs: {} generated, {} existing ({})",
            style("+").cyan(),
            report.generated_jobs,
            report.existing_jobs,
            display_project_root(root, root)
        );
        for job in &report.jobs {
            let status = match job.status {
                scip_io_core::cmake_compile_databases::CmakeCompileDatabaseJobStatus::Pending => {
                    "generated"
                }
                scip_io_core::cmake_compile_databases::CmakeCompileDatabaseJobStatus::Existing => {
                    "existing"
                }
            };
            println!(
                "      {} {} -> {}",
                status,
                job.name,
                display_project_root(&job.compile_commands, root)
            );
        }
    }
    Ok(())
}

#[cfg(test)]
fn detect_languages_for_roots(
    args: &IndexArgs,
    project_roots: &[PathBuf],
) -> Result<Vec<ProjectLanguages>> {
    detect_languages_for_roots_with_config(args, project_roots, &ProjectConfig::default())
}

fn detect_languages_for_roots_with_config(
    args: &IndexArgs,
    project_roots: &[PathBuf],
    config: &ProjectConfig,
) -> Result<Vec<ProjectLanguages>> {
    let mut projects = Vec::new();

    for root in project_roots {
        let excluded_roots: Vec<PathBuf> = project_roots
            .iter()
            .filter(|candidate| *candidate != root && candidate.starts_with(root))
            .cloned()
            .collect();
        let owned_child_prefixes = child_prefixes_for_project_root(root, &excluded_roots);
        let detected = scan_languages_with_options(
            root,
            LanguageScanOptions {
                max_depth: None,
                excluded_roots,
            },
        )?;
        let languages = if args.lang.is_empty() {
            detected
        } else {
            detected
                .into_iter()
                .filter(|l| {
                    args.lang
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(l.name()))
                })
                .collect()
        };
        let languages = add_additional_configs_if_requested(args, config, root, languages)?;
        if languages.is_empty() {
            continue;
        }
        projects.push(ProjectLanguages {
            root: root.clone(),
            languages,
            owned_child_prefixes,
        });
    }

    Ok(projects)
}

fn add_additional_configs_if_requested(
    args: &IndexArgs,
    config: &ProjectConfig,
    root: &Path,
    mut languages: Vec<Language>,
) -> Result<Vec<Language>> {
    if !effective_include_additional_configs(args, config) {
        return Ok(languages);
    }

    for language in &mut languages {
        language.additional_configs =
            discover_additional_configs_for_language(root, language.kind, config)?;
    }
    add_languages_from_additional_configs(args, config, root, &mut languages)?;

    Ok(languages)
}

fn add_languages_from_additional_configs(
    args: &IndexArgs,
    config: &ProjectConfig,
    root: &Path,
    languages: &mut Vec<Language>,
) -> Result<()> {
    for &kind in supported_additional_config_languages() {
        if !language_filter_allows(args, kind) {
            continue;
        }
        if languages.iter().any(|language| language.kind == kind) {
            continue;
        }

        let configs = discover_additional_configs_for_language(root, kind, config)?;
        if let Some(first_config) = configs.first() {
            let evidence = display_project_root(first_config, root);
            let mut language =
                kind.with_detected_evidence(evidence, DetectionEvidenceKind::ProjectConfig);
            language.additional_configs = configs;
            languages.push(language);
        }
    }

    Ok(())
}

fn discover_additional_configs_for_language(
    root: &Path,
    kind: scip_io_core::LanguageKind,
    config: &ProjectConfig,
) -> Result<Vec<PathBuf>> {
    let configs = discover_additional_configs(root, kind)?;
    if kind != scip_io_core::LanguageKind::Cpp {
        return Ok(configs);
    }
    if configs.is_empty() {
        return Ok(configs);
    }

    let selection =
        select_compile_command_databases(root, &configs, &cpp_coverage_options(config))?;
    if selection.configs.is_empty() {
        bail!("{}", cpp_coverage_empty_selection_error(root, &selection));
    }
    Ok(selection.configs)
}

fn cpp_coverage_options(config: &ProjectConfig) -> CompileCommandCoverageOptions {
    config
        .cpp
        .as_ref()
        .and_then(|cpp| cpp.coverage.as_ref())
        .map(|coverage| CompileCommandCoverageOptions {
            include: coverage.include.clone(),
            exclude: coverage.exclude.clone(),
            min_new_files: coverage.min_new_files,
        })
        .unwrap_or_default()
}

fn cpp_coverage_empty_selection_error(
    root: &Path,
    selection: &scip_io_core::compile_commands::CompileCommandSelection,
) -> String {
    let mut details = selection
        .databases
        .iter()
        .filter_map(|database| {
            let reason = database.skip_reason.as_ref()?;
            let path = display_project_root(&database.path, root).replace('\\', "/");
            Some(format!("{path}: {reason}"))
        })
        .collect::<Vec<_>>();
    let omitted = details.len().saturating_sub(8);
    details.truncate(8);

    let mut message = "C/C++ coverage profile selected no compile databases; adjust [cpp.coverage] include/exclude/min_new_files or remove C++ from the configured languages".to_string();
    if !details.is_empty() {
        message.push_str(". Skipped: ");
        message.push_str(&details.join("; "));
        if omitted > 0 {
            message.push_str(&format!("; ... {omitted} more"));
        }
    }
    message
}

fn child_prefixes_for_project_root(root: &Path, child_roots: &[PathBuf]) -> Vec<String> {
    let mut prefixes = child_roots
        .iter()
        .filter_map(|child_root| child_root.strip_prefix(root).ok())
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .collect::<Vec<_>>();
    prefixes.sort();
    prefixes.dedup();
    prefixes
}

fn prune_nested_project_documents(output: &Path, child_prefixes: &[String]) -> Result<()> {
    let stats = prune_scip_file_document_paths_with_prefixes(output, child_prefixes)?;
    if stats.documents_before > 0 && stats.documents_after == 0 {
        bail!(
            "SCIP output contains no documents owned by this project root after excluding nested roots"
        );
    }
    Ok(())
}

fn language_filter_allows(args: &IndexArgs, kind: scip_io_core::LanguageKind) -> bool {
    args.lang.is_empty()
        || args
            .lang
            .iter()
            .any(|name| name.eq_ignore_ascii_case(kind.name()))
}

fn collect_unready_language_failures(projects: &[ProjectLanguages]) -> Vec<(String, String)> {
    projects
        .iter()
        .flat_map(|project| {
            project
                .languages
                .iter()
                .filter(|language| !language.indexer_ready)
                .map(|language| {
                    (
                        language.name().to_string(),
                        language
                            .readiness_message
                            .clone()
                            .unwrap_or_else(|| "language is not index-ready".to_string()),
                    )
                })
        })
        .collect()
}

fn print_index_plan(projects: &[ProjectLanguages], base_path: &Path, scope: IndexScope) {
    println!("{} Indexing ({} scope):", style(">").cyan().bold(), scope);
    for project in projects {
        println!(
            "  {} {}",
            style("root").dim(),
            display_project_root(&project.root, base_path)
        );
        println!(
            "    {}",
            project
                .languages
                .iter()
                .map(|l| l.name())
                .collect::<Vec<_>>()
                .join(", ")
        );
        for lang in &project.languages {
            if !lang.indexer_ready
                && let Some(message) = &lang.readiness_message
            {
                println!("      {} {}: {}", style("!").yellow(), lang.name(), message);
            }
            if !lang.additional_configs.is_empty() {
                println!(
                    "      {} {} config(s): {}",
                    style("+").dim(),
                    lang.name(),
                    lang.additional_configs
                        .iter()
                        .map(|path| display_project_root(path, &project.root))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
        }
    }
}

fn display_project_root(root: &Path, base_path: &Path) -> String {
    root.strip_prefix(base_path)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map(|relative| relative.display().to_string())
        .unwrap_or_else(|| root.display().to_string())
}

fn unique_language_names(projects: &[ProjectLanguages]) -> Vec<&'static str> {
    let mut seen = BTreeSet::new();
    for project in projects {
        for language in &project.languages {
            seen.insert(language.name());
        }
    }
    seen.into_iter().collect()
}

fn index_publication_message(
    output: &Path,
    successful_outputs: usize,
    failed_languages: usize,
    merged: bool,
) -> String {
    let label = match (failed_languages > 0, merged) {
        (true, true) => "Partial merged index",
        (true, false) => "Partial index",
        (false, true) => "Merged index",
        (false, false) => "Index",
    };
    let mut message = format!("{label} written to {}", output.display());
    if failed_languages > 0 {
        message.push_str(&format!(
            " ({} successful output(s), {} failed language(s))",
            successful_outputs, failed_languages
        ));
    }
    message
}

fn prefix_output_paths_for_project_root(
    output: &Path,
    project_root: &Path,
    base_path: &Path,
) -> Result<usize> {
    let Ok(relative_root) = project_root.strip_prefix(base_path) else {
        return Ok(0);
    };
    if relative_root.as_os_str().is_empty() {
        return Ok(0);
    }

    let prefix = relative_root.to_string_lossy().replace('\\', "/");
    prefix_scip_file_document_paths(output, &prefix)
}

/// Dry-run mode: show what would happen without executing anything.
fn run_dry_run(
    args: &IndexArgs,
    projects: &[ProjectLanguages],
    is_json: bool,
    config: &ProjectConfig,
    cmake_generation_plans: &[(PathBuf, CmakeCompileDatabaseGenerationPlan)],
) -> Result<()> {
    if is_json {
        let mut plan = Vec::new();
        for project in projects {
            for lang in &project.languages {
                let indexer = REGISTRY.runnable_for(lang);
                let backend = indexer
                    .map(|entry| config.backend_preference_for(lang.name(), &entry.indexer_name));
                let toolchain = indexer
                    .and_then(|entry| toolchain_preflight_for_indexer(entry, &config.toolchains));
                let compile_database_summary =
                    dry_run_compile_database_summary(&project.root, lang, config)?;
                plan.push(serde_json::json!({
                    "root": project.root.display().to_string(),
                    "language": lang.name(),
                    "evidence": lang.evidence(),
                    "indexer": indexer.map(|e| &e.indexer_name),
                    "installed": indexer.map(|e| e.is_installed()).unwrap_or(false),
                    "backend": backend.as_ref().map(|preference| format!("{:?}", preference.kind).to_ascii_lowercase()),
                    "native_supported": indexer.map(|e| e.native_supported_on_current_platform()).unwrap_or(false),
                    "toolchain_required": toolchain.as_ref().map(|status| status.kind.as_str()),
                    "toolchain_available": toolchain.as_ref().map(|status| status.available),
                    "toolchain_message": toolchain.as_ref().map(|status| status.message.as_str()),
                    "indexer_ready": lang.indexer_ready,
                    "readiness_message": lang.readiness_message.as_deref(),
                    "scope": effective_scope(args, config).to_string(),
                    "command": if lang.indexer_ready { indexer.map(|e| {
                        format!(
                            "{} {}",
                            e.binary_name,
                            dry_run_command_args(e, &project.root, lang, config).join(" ")
                        )
                    }) } else { None },
                    "configs": lang.additional_configs.iter().map(|path| {
                        display_project_root(path, &project.root)
                    }).collect::<Vec<_>>(),
                    "compile_database_summary": compile_database_summary,
                    "cmake_compile_database_generation": dry_run_cmake_compile_database_generation(&project.root, cmake_generation_plans)?,
                }));
            }
        }
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        println!(
            "{} Dry run -- the following would be indexed ({} scope):",
            style("*").cyan().bold(),
            effective_scope(args, config)
        );
        for project in projects {
            println!("  {} {}", style("root").dim(), project.root.display());
            for lang in &project.languages {
                let indexer = REGISTRY.runnable_for(lang);
                let backend = indexer
                    .map(|entry| config.backend_preference_for(lang.name(), &entry.indexer_name));
                let status = if !lang.indexer_ready {
                    "not runnable".to_string()
                } else {
                    match indexer {
                        Some(e)
                            if !e.native_supported_on_current_platform()
                                && backend.as_ref().is_some_and(|preference| {
                                    preference.kind != ExecutionBackendKind::Native
                                }) =>
                        {
                            format!(
                                "{} (will use {})",
                                e.indexer_name,
                                backend_label(backend.as_ref())
                            )
                        }
                        Some(e) if e.is_installed() => format!("{} (installed)", e.indexer_name),
                        Some(e) => format!("{} (will download)", e.indexer_name),
                        None => "no indexer registered".to_string(),
                    }
                };
                println!("    {} {} -- {}", style(">").cyan(), lang.name(), status);
                if !lang.indexer_ready
                    && let Some(message) = &lang.readiness_message
                {
                    println!("      readiness: {}", message);
                }
                if !lang.indexer_ready {
                    continue;
                }
                if let Some(e) = indexer {
                    if lang.kind == scip_io_core::LanguageKind::Cpp
                        && let Some(summary) = dry_run_cmake_compile_database_generation(
                            &project.root,
                            cmake_generation_plans,
                        )?
                    {
                        println!(
                            "      cmake compile db generation: {} job(s), {} existing",
                            json_u64(&summary, "planned_jobs"),
                            json_u64(&summary, "existing_jobs")
                        );
                    }
                    if let Some(toolchain) = toolchain_preflight_for_indexer(e, &config.toolchains)
                    {
                        println!(
                            "      toolchain: {} ({})",
                            toolchain.kind.display_name(),
                            if toolchain.available {
                                "ready"
                            } else {
                                "missing"
                            }
                        );
                        if !toolchain.available {
                            println!("      reason: {}", toolchain.message);
                        }
                    }
                    println!(
                        "      command: {} {}",
                        e.binary_name,
                        dry_run_command_args(e, &project.root, lang, config).join(" ")
                    );
                    if !lang.additional_configs.is_empty() {
                        println!(
                            "      configs: {}",
                            lang.additional_configs
                                .iter()
                                .map(|path| display_project_root(path, &project.root))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                    if let Some(summary) =
                        dry_run_compile_database_summary(&project.root, lang, config)?
                    {
                        println!(
                            "      compile dbs: {} selected, {} input commands, {} merged commands, {} duplicate commands, {} unique files, {} new files vs primary, {} skipped",
                            json_u64(&summary, "selected_databases"),
                            json_u64(&summary, "input_commands"),
                            json_u64(&summary, "output_commands"),
                            json_u64(&summary, "duplicate_commands"),
                            json_u64(&summary, "unique_files"),
                            json_u64(&summary, "new_files_vs_primary"),
                            json_u64(&summary, "skipped_databases")
                        );
                        let skipped_details =
                            skipped_compile_database_details(&project.root, &summary);
                        if !skipped_details.is_empty() {
                            println!("      skipped compile dbs: {}", skipped_details.join("; "));
                        }
                    }
                }
            }
        }
        let language_count = projects
            .iter()
            .map(|project| project.languages.len())
            .sum::<usize>();
        if !args.no_merge && language_count > 1 {
            println!(
                "  {} Merge output: {}",
                style(">").cyan(),
                args.output.display()
            );
        }
    }
    Ok(())
}

fn dry_run_cmake_compile_database_generation(
    root: &Path,
    plans: &[(PathBuf, CmakeCompileDatabaseGenerationPlan)],
) -> Result<Option<serde_json::Value>> {
    let Some((_, plan)) = plans.iter().find(|(plan_root, _)| plan_root == root) else {
        return Ok(None);
    };
    let existing_jobs = plan
        .jobs
        .iter()
        .filter(|job| {
            job.status
                == scip_io_core::cmake_compile_databases::CmakeCompileDatabaseJobStatus::Existing
        })
        .count();
    Ok(Some(serde_json::json!({
        "planned_jobs": plan.jobs.len(),
        "existing_jobs": existing_jobs,
        "jobs": plan.jobs.iter().map(|job| serde_json::json!({
            "name": job.name,
            "source_dir": display_project_root(&job.source_dir, root),
            "build_dir": display_project_root(&job.build_dir, root),
            "compile_commands": display_project_root(&job.compile_commands, root),
            "status": job.status,
            "command": format!("{} {}", job.cmake.display(), job.args.join(" ")),
        })).collect::<Vec<_>>(),
    })))
}

fn dry_run_compile_database_summary(
    root: &Path,
    lang: &Language,
    config: &ProjectConfig,
) -> Result<Option<serde_json::Value>> {
    if lang.kind != scip_io_core::LanguageKind::Cpp || lang.additional_configs.is_empty() {
        return Ok(None);
    }

    let discovery = discover_compile_command_databases(root)?;
    let selection =
        select_compile_command_databases(root, &discovery.configs, &cpp_coverage_options(config))?;
    let mut report = summarize_compile_command_databases(&selection.configs)?;
    let mut skipped = discovery.skipped;
    skipped.extend(
        selection
            .databases
            .iter()
            .filter(|database| !database.selected)
            .filter_map(|database| {
                let reason = database.skip_reason.clone()?;
                Some(CompileCommandDatabaseSkip {
                    path: database.path.clone(),
                    reason,
                })
            }),
    );
    report.skipped = skipped;
    report.skipped_databases = report.skipped.len();
    report.databases = selection.databases;
    Ok(Some(serde_json::to_value(report)?))
}

fn json_u64(value: &serde_json::Value, field: &str) -> u64 {
    value
        .get(field)
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
}

fn skipped_compile_database_details(root: &Path, summary: &serde_json::Value) -> Vec<String> {
    summary
        .get("skipped")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|skip| {
            let path = skip.get("path")?.as_str()?;
            let reason = skip.get("reason")?.as_str()?;
            let display = display_project_root(Path::new(path), root).replace('\\', "/");
            Some(format!("{display}: {reason}"))
        })
        .collect()
}

fn should_prepare_native_binary(entry: &IndexerEntry, preference: &BackendPreference) -> bool {
    match preference.kind {
        ExecutionBackendKind::Native => true,
        ExecutionBackendKind::Auto => entry.native_supported_on_current_platform(),
        ExecutionBackendKind::Wsl
        | ExecutionBackendKind::Docker
        | ExecutionBackendKind::Disabled => false,
    }
}

fn backend_label(preference: Option<&BackendPreference>) -> String {
    let Some(preference) = preference else {
        return "auto backend".to_string();
    };
    match preference.kind {
        ExecutionBackendKind::Auto => "auto backend".to_string(),
        ExecutionBackendKind::Native => "native backend".to_string(),
        ExecutionBackendKind::Wsl => "WSL backend".to_string(),
        ExecutionBackendKind::Docker => "Docker backend".to_string(),
        ExecutionBackendKind::Disabled => "disabled backend".to_string(),
    }
}

fn dry_run_command_args(
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    config: &ProjectConfig,
) -> Vec<String> {
    let output_file = PathBuf::from(format!("{}.scip", lang.name()));
    let override_entry = config.args_override_for(lang.name(), &entry.indexer_name);
    if entry.indexer_name == "scip-clang" && !lang.additional_configs.is_empty() {
        let merged_compile_database = PathBuf::from("<merged compile_commands.json>");
        let args = match override_entry {
            Some(default_args) => runner::build_compile_command_database_args_with_defaults(
                entry,
                &merged_compile_database,
                &output_file,
                &default_args,
            ),
            None => runner::build_compile_command_database_args(
                entry,
                &merged_compile_database,
                &output_file,
            ),
        };
        return args
            .into_iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect();
    }

    let display_configs = lang
        .additional_configs
        .iter()
        .map(|path| PathBuf::from(display_project_root(path, project_root)))
        .collect::<Vec<_>>();

    match override_entry {
        Some(default_args) => runner::build_indexer_args_with_defaults_for_display(
            entry,
            &output_file,
            &display_configs,
            &default_args,
        ),
        None => runner::build_indexer_args(entry, &output_file, &display_configs),
    }
    .into_iter()
    .map(|arg| arg.to_string_lossy().to_string())
    .collect()
}

/// Group tasks by language, `indexer_name`, and project root, then collapse
/// true duplicates to one invocation. Different languages can share a binary
/// while still requiring distinct project arguments and outputs.
fn dedupe_tasks_by_indexer(tasks: Vec<IndexerTask>, is_json: bool) -> Vec<IndexerTask> {
    let mut grouped: HashMap<String, Vec<IndexerTask>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for task in tasks {
        let key = format!(
            "{}\0{}\0{}",
            task.project_root.display(),
            task.entry.indexer_name,
            task.lang.name()
        );
        if !grouped.contains_key(&key) {
            order.push(key.clone());
        }
        grouped.entry(key).or_default().push(task);
    }

    let mut out = Vec::with_capacity(order.len());
    for key in order {
        let mut group = grouped.remove(&key).unwrap();
        if group.len() == 1 {
            out.push(group.pop().unwrap());
            continue;
        }

        let needs_infer_tsconfig = group.iter().any(|task| {
            is_nested_typescript_without_explicit_configs(&task.lang, &task.additional_configs)
        });

        // Pick the primary: nested TypeScript evidence needs the
        // `--infer-tsconfig` invocation because the plain command only looks
        // for a root tsconfig.json. Otherwise prefer the cleaner non-infer
        // invocation, then fall back to language name for determinism.
        group.sort_by(|a, b| {
            shared_indexer_task_sort_key(a, needs_infer_tsconfig)
                .cmp(&shared_indexer_task_sort_key(b, needs_infer_tsconfig))
        });

        let mut iter = group.into_iter();
        let mut primary = iter.next().unwrap();
        for extra in iter {
            if !is_json {
                println!(
                    "  {} {} will be indexed by the {} run for {} (shared tool: {})",
                    style("i").yellow(),
                    extra.lang.name(),
                    primary.lang.name(),
                    primary.lang.name(),
                    primary.entry.indexer_name,
                );
            }
            primary.covers.push(extra.lang.name().to_string());
            primary.additional_configs.extend(extra.additional_configs);
        }
        primary.additional_configs.sort();
        primary.additional_configs.dedup();
        out.push(primary);
    }

    out
}

fn shared_indexer_task_sort_key(
    task: &IndexerTask,
    needs_infer_tsconfig: bool,
) -> (bool, bool, &str) {
    let uses_infer_tsconfig = uses_infer_tsconfig(task.entry);
    (
        needs_infer_tsconfig && !uses_infer_tsconfig,
        !needs_infer_tsconfig && uses_infer_tsconfig,
        task.lang.name(),
    )
}

fn uses_infer_tsconfig(entry: &IndexerEntry) -> bool {
    entry
        .default_args
        .iter()
        .any(|arg| arg == "--infer-tsconfig")
}

fn is_nested_typescript_without_explicit_configs(
    lang: &Language,
    additional_configs: &[PathBuf],
) -> bool {
    lang.kind == scip_io_core::detect::languages::LanguageKind::TypeScript
        && additional_configs.is_empty()
        && lang.evidence.contains(['/', '\\'])
}
#[cfg(test)]
mod tests {
    use super::{
        IndexArgs, IndexerTask, collect_unready_language_failures, dedupe_tasks_by_indexer,
        detect_languages_for_roots, detect_languages_for_roots_with_config,
        dry_run_compile_database_summary, effective_include_additional_configs,
        index_publication_message, resolve_project_roots, resolve_project_roots_with_config,
        skipped_compile_database_details,
    };
    use scip_io_core::LanguageKind;
    use scip_io_core::config::{
        CmakeCompileDatabaseConfig, CmakeCompileDatabasePreset, CppConfig, CppCoverageConfig,
        ProjectConfig,
    };
    use scip_io_core::indexer::backend::BackendPreference;
    use scip_io_core::indexer::registry::REGISTRY;
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::TempDir;

    fn base_args() -> IndexArgs {
        IndexArgs {
            path: None,
            lang: Vec::new(),
            output: PathBuf::from("index.scip"),
            no_merge: false,
            parallel: None,
            timeout: None,
            format: "text".to_string(),
            dry_run: true,
            roots: Vec::new(),
            all_roots: false,
            scope: None,
            include_additional_configs: false,
            generate_cmake_compile_dbs: false,
            cmake_preset: None,
        }
    }

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

    fn write_compile_database(root: &Path, relative_path: &str, contents: &str) {
        let path = root.join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn resolve_project_roots_uses_explicit_roots_relative_to_base_path() {
        let (_dir, root) = fixture(&["services/api/Cargo.toml", "packages/web/package.json"]);
        let mut args = base_args();
        args.roots = vec![PathBuf::from("services/api"), PathBuf::from("packages/web")];

        let roots = resolve_project_roots(&args, &root).unwrap();
        assert_eq!(
            roots,
            vec![
                root.join("services/api").canonicalize().unwrap(),
                root.join("packages/web").canonicalize().unwrap(),
            ]
        );
    }

    #[test]
    fn resolve_project_roots_discovers_all_manifest_roots() {
        let (_dir, root) = fixture(&[
            "services/api/Cargo.toml",
            "packages/web/package.json",
            "native/compile_commands.json",
            "cmake-only/CMakeLists.txt",
        ]);
        let mut args = base_args();
        args.all_roots = true;

        let roots = resolve_project_roots(&args, &root).unwrap();
        assert_eq!(
            roots,
            vec![
                root.join("native"),
                root.join("packages/web"),
                root.join("services/api")
            ]
        );
        assert!(!roots.contains(&root.join("cmake-only")));
    }

    #[test]
    fn default_project_roots_use_repo_tree_scope() {
        let (_dir, root) = fixture(&[
            "src/root.py",
            "services/api/Cargo.toml",
            "cmd/tool/go.mod",
            "apps/web/tsconfig.json",
            "packages/js/package.json",
            "java/pom.xml",
            "gradle/build.gradle",
            "dotnet/App.csproj",
            "gems/Gemfile",
            "kotlin/build.gradle.kts",
            "scala/build.sbt",
            "native/compile_commands.json",
            "cmake-only/CMakeLists.txt",
        ]);
        let args = base_args();

        let roots = resolve_project_roots(&args, &root).unwrap();

        assert_eq!(roots, vec![root]);
    }

    #[test]
    fn default_detection_keeps_nested_languages_in_repo_tree_scope() {
        let (_dir, root) = fixture(&[
            "src/root.py",
            "services/api/Cargo.toml",
            "services/api/src/main.rs",
            "apps/web/tsconfig.json",
            "apps/web/src/main.ts",
        ]);
        let args = base_args();

        let roots = resolve_project_roots(&args, &root).unwrap();
        let projects = detect_languages_for_roots(&args, &roots).unwrap();

        let root_project = projects
            .iter()
            .find(|project| project.root == root)
            .expect("root project");
        let kinds = root_project
            .languages
            .iter()
            .map(|language| language.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![
                LanguageKind::Python,
                LanguageKind::Rust,
                LanguageKind::TypeScript
            ]
        );
        assert_eq!(projects.len(), 1);
        assert!(root_project.owned_child_prefixes.is_empty());
    }

    #[test]
    fn repo_tree_detection_reports_unready_languages_as_partial_failures() {
        let (_dir, root) = fixture(&[
            "native/compile_commands.json",
            "native/src/main.cpp",
            "src/root.py",
        ]);
        let args = base_args();

        let roots = resolve_project_roots(&args, &root).unwrap();
        let projects = detect_languages_for_roots(&args, &roots).unwrap();
        let failures = collect_unready_language_failures(&projects);

        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, "cpp");
        assert!(failures[0].1.contains("nested compile database"));
    }

    #[test]
    fn config_scope_detection_assigns_child_prefixes_to_parent_project() {
        let (_dir, root) = fixture(&[
            "package.json",
            "src/root.js",
            "packages/web/package.json",
            "packages/web/src/index.js",
        ]);
        let mut args = base_args();
        args.scope = Some(scip_io_core::config::IndexScope::Configs);

        let roots = resolve_project_roots(&args, &root).unwrap();
        let projects = detect_languages_for_roots(&args, &roots).unwrap();

        let root_project = projects
            .iter()
            .find(|project| project.root == root)
            .expect("root project");
        assert_eq!(
            root_project.owned_child_prefixes,
            vec!["packages/web".to_string()]
        );

        let web_project = projects
            .iter()
            .find(|project| project.root == root.join("packages/web"))
            .expect("web project");
        assert!(web_project.owned_child_prefixes.is_empty());
    }

    #[test]
    fn all_roots_detects_direct_languages_for_each_discovered_root() {
        let (_dir, root) = fixture(&["package.json", "services/api/Cargo.toml"]);
        let mut args = base_args();
        args.all_roots = true;

        let roots = resolve_project_roots(&args, &root).unwrap();
        let projects = detect_languages_for_roots(&args, &roots).unwrap();

        let root_project = projects
            .iter()
            .find(|project| project.root == root)
            .expect("root project");
        assert_eq!(root_project.languages.len(), 1);
        assert_eq!(root_project.languages[0].kind, LanguageKind::JavaScript);

        let api_project = projects
            .iter()
            .find(|project| project.root == root.join("services/api"))
            .expect("api project");
        assert_eq!(api_project.languages.len(), 1);
        assert_eq!(api_project.languages[0].kind, LanguageKind::Rust);
    }

    #[test]
    fn include_additional_configs_adds_supported_config_files_to_project() {
        let (_dir, root) = fixture(&[
            "tsconfig.json",
            "tsconfig.scripts.json",
            "tsconfig.test.json",
            "src/index.ts",
        ]);
        let mut args = base_args();
        args.include_additional_configs = true;

        let projects = detect_languages_for_roots(&args, std::slice::from_ref(&root)).unwrap();

        let project = &projects[0];
        assert_eq!(project.languages.len(), 1);
        assert_eq!(
            project.languages[0].additional_configs,
            vec![
                root.join("tsconfig.json"),
                root.join("tsconfig.scripts.json"),
                root.join("tsconfig.test.json")
            ]
        );
    }

    #[test]
    fn dry_run_cpp_compile_database_summary_reports_coverage_delta() {
        let (_dir, root) = fixture(&["src/main.cpp"]);
        write_compile_database(
            &root,
            "compile_commands.json",
            r#"[{"directory":"src","file":"a.cc","command":"clang++ -c a.cc"}]"#,
        );
        write_compile_database(
            &root,
            "build-scip-wsl/compile_commands.json",
            r#"[
              {"directory":"src","file":"a.cc","command":"clang++ -c a.cc"},
              {"directory":"build-scip-wsl","file":"../src/b.cc","command":"clang++ -c ../src/b.cc"}
            ]"#,
        );
        write_compile_database(&root, "cmake-build-bad/compile_commands.json", "{bad json");
        let mut args = base_args();
        args.include_additional_configs = true;

        let projects = detect_languages_for_roots(&args, std::slice::from_ref(&root)).unwrap();
        let lang = projects[0]
            .languages
            .iter()
            .find(|language| language.kind == LanguageKind::Cpp)
            .unwrap();
        let summary = dry_run_compile_database_summary(&root, lang, &ProjectConfig::default())
            .unwrap()
            .unwrap();

        assert_eq!(summary["selected_databases"], 2);
        assert_eq!(summary["input_commands"], 3);
        assert_eq!(summary["output_commands"], 2);
        assert_eq!(summary["duplicate_commands"], 1);
        assert_eq!(summary["unique_files"], 2);
        assert_eq!(summary["new_files_vs_primary"], 1);
        assert_eq!(summary["skipped_databases"], 1);

        let details = skipped_compile_database_details(&root, &summary);
        assert_eq!(details.len(), 1);
        assert!(details[0].contains("cmake-build-bad/compile_commands.json"));
        assert!(details[0].contains("Failed to parse"));
    }

    #[test]
    fn config_include_additional_configs_discovers_cpp_compile_databases() {
        let (_dir, root) = fixture(&["src/main.cpp"]);
        write_compile_database(
            &root,
            "compile_commands.json",
            r#"[{"directory":"src","file":"main.cpp","command":"clang++ -c main.cpp"}]"#,
        );
        write_compile_database(
            &root,
            "build-scip-wsl/compile_commands.json",
            r#"[{"directory":"build-scip-wsl","file":"../src/tool.cpp","command":"clang++ -c ../src/tool.cpp"}]"#,
        );
        let args = base_args();
        let config = ProjectConfig {
            include_additional_configs: Some(true),
            ..ProjectConfig::default()
        };

        let projects =
            detect_languages_for_roots_with_config(&args, std::slice::from_ref(&root), &config)
                .unwrap();

        let cpp = projects[0]
            .languages
            .iter()
            .find(|language| language.kind == LanguageKind::Cpp)
            .unwrap();
        assert_eq!(
            cpp.additional_configs,
            vec![
                root.join("compile_commands.json"),
                root.join("build-scip-wsl/compile_commands.json")
            ]
        );
    }

    #[test]
    fn cpp_coverage_config_filters_additional_compile_databases() {
        let (_dir, root) = fixture(&["src/main.cpp"]);
        write_compile_database(
            &root,
            "compile_commands.json",
            r#"[{"directory":"src","file":"a.cc","command":"clang++ -c a.cc"}]"#,
        );
        write_compile_database(
            &root,
            "build-excluded/compile_commands.json",
            r#"[{"directory":"src","file":"b.cc","command":"clang++ -c b.cc"}]"#,
        );
        write_compile_database(
            &root,
            "build-large/compile_commands.json",
            r#"[
              {"directory":"src","file":"c.cc","command":"clang++ -c c.cc"},
              {"directory":"src","file":"d.cc","command":"clang++ -c d.cc"}
            ]"#,
        );
        write_compile_database(
            &root,
            "build-small/compile_commands.json",
            r#"[{"directory":"src","file":"e.cc","command":"clang++ -c e.cc"}]"#,
        );
        let args = base_args();
        let config = ProjectConfig {
            include_additional_configs: Some(true),
            cpp: Some(CppConfig {
                coverage: Some(CppCoverageConfig {
                    exclude: vec!["build-excluded/**".to_string()],
                    min_new_files: Some(2),
                    ..CppCoverageConfig::default()
                }),
                ..CppConfig::default()
            }),
            ..ProjectConfig::default()
        };

        let projects =
            detect_languages_for_roots_with_config(&args, std::slice::from_ref(&root), &config)
                .unwrap();

        let cpp = projects[0]
            .languages
            .iter()
            .find(|language| language.kind == LanguageKind::Cpp)
            .unwrap();
        assert_eq!(
            cpp.additional_configs,
            vec![
                root.join("compile_commands.json"),
                root.join("build-large/compile_commands.json")
            ]
        );

        let summary = dry_run_compile_database_summary(&root, cpp, &config)
            .unwrap()
            .unwrap();
        assert_eq!(summary["selected_databases"], 2);
        assert_eq!(summary["input_commands"], 3);
        assert_eq!(summary["unique_files"], 3);
        assert_eq!(summary["new_files_vs_primary"], 2);
        assert_eq!(summary["skipped_databases"], 2);
        assert_eq!(summary["databases"].as_array().unwrap().len(), 4);

        let details = skipped_compile_database_details(&root, &summary);
        assert_eq!(details.len(), 2);
        assert!(details.iter().any(
            |detail| detail.contains("build-excluded/compile_commands.json")
                && detail.contains("excluded by cpp.coverage.exclude")
        ));
        assert!(details.iter().any(
            |detail| detail.contains("build-small/compile_commands.json")
                && detail.contains("below cpp.coverage.min_new_files=2")
        ));
    }

    #[test]
    fn cpp_coverage_config_errors_when_all_compile_databases_filtered() {
        let (_dir, root) = fixture(&["src/main.cpp"]);
        write_compile_database(
            &root,
            "compile_commands.json",
            r#"[{"directory":"src","file":"a.cc","command":"clang++ -c a.cc"}]"#,
        );
        write_compile_database(
            &root,
            "build-extra/compile_commands.json",
            r#"[{"directory":"src","file":"b.cc","command":"clang++ -c b.cc"}]"#,
        );
        let args = base_args();
        let config = ProjectConfig {
            include_additional_configs: Some(true),
            cpp: Some(CppConfig {
                coverage: Some(CppCoverageConfig {
                    include: vec!["missing-*/compile_commands.json".to_string()],
                    ..CppCoverageConfig::default()
                }),
                ..CppConfig::default()
            }),
            ..ProjectConfig::default()
        };

        let error = match detect_languages_for_roots_with_config(
            &args,
            std::slice::from_ref(&root),
            &config,
        ) {
            Ok(_) => panic!("expected all-filtered C/C++ coverage profile to be a config error"),
            Err(error) => error.to_string(),
        };

        assert!(error.contains("C/C++ coverage profile selected no compile databases"));
        assert!(error.contains("compile_commands.json: not matched by cpp.coverage.include"));
        assert!(
            error
                .contains("build-extra/compile_commands.json: not matched by cpp.coverage.include")
        );
    }

    #[test]
    fn cmake_generation_implies_additional_config_discovery() {
        let (_dir, root) = fixture(&["llvm/CMakeLists.txt", "src/main.cpp"]);
        write_compile_database(
            &root,
            "build-scip-io-llvm-all-targets/compile_commands.json",
            r#"[{"directory":"build-scip-io-llvm-all-targets","file":"../src/main.cpp","command":"clang++ -c ../src/main.cpp"}]"#,
        );
        let args = base_args();
        let config = ProjectConfig {
            cpp: Some(CppConfig {
                cmake: Some(CmakeCompileDatabaseConfig {
                    generate_compile_databases: Some(true),
                    preset: Some(CmakeCompileDatabasePreset::LlvmBroad),
                    ..CmakeCompileDatabaseConfig::default()
                }),
                ..CppConfig::default()
            }),
            ..ProjectConfig::default()
        };

        assert!(effective_include_additional_configs(&args, &config));
        let projects =
            detect_languages_for_roots_with_config(&args, std::slice::from_ref(&root), &config)
                .unwrap();

        let cpp = projects[0]
            .languages
            .iter()
            .find(|language| language.kind == LanguageKind::Cpp)
            .unwrap();
        assert_eq!(
            cpp.additional_configs,
            vec![root.join("build-scip-io-llvm-all-targets/compile_commands.json")]
        );
    }

    #[test]
    fn config_include_additional_configs_discovers_config_only_roots() {
        let (_dir, root) = fixture(&["src/main.cpp"]);
        write_compile_database(
            &root,
            "build-scip-wsl/compile_commands.json",
            r#"[{"directory":"build-scip-wsl","file":"../src/main.cpp","command":"clang++ -c ../src/main.cpp"}]"#,
        );
        let mut args = base_args();
        args.all_roots = true;
        let config = ProjectConfig {
            include_additional_configs: Some(true),
            ..ProjectConfig::default()
        };

        let roots = resolve_project_roots_with_config(&args, &root, &config).unwrap();

        assert_eq!(roots, vec![root.join("build-scip-wsl")]);
    }

    #[test]
    fn index_detection_scans_nested_source_evidence_without_default_depth_cap() {
        let (_dir, root) = fixture(&["deep/a/b/c/d/lib.rs"]);
        let args = base_args();

        let projects = detect_languages_for_roots(&args, std::slice::from_ref(&root)).unwrap();

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].languages.len(), 1);
        assert_eq!(projects[0].languages[0].kind, LanguageKind::Rust);
        // Evidence is rendered with native path separators; compare components
        // so this regression guard is meaningful on Windows and Unix runners.
        let evidence = Path::new(projects[0].languages[0].evidence.as_str());
        let expected = Path::new("deep")
            .join("a")
            .join("b")
            .join("c")
            .join("d")
            .join("lib.rs");
        assert_eq!(
            evidence.components().collect::<Vec<_>>(),
            expected.components().collect::<Vec<_>>()
        );
    }

    #[test]
    fn javascript_and_typescript_tasks_remain_separate_by_default() {
        let (_dir, root) = fixture(&[
            "tools/vscode/tsconfig.json",
            "tools/vscode/src/extension.ts",
            "tools/tree-sitter/package.json",
            "tools/tree-sitter/grammar.js",
        ]);
        let typescript = LanguageKind::TypeScript.with_evidence(
            Path::new("tools")
                .join("vscode")
                .join("tsconfig.json")
                .to_string_lossy()
                .into_owned(),
        );
        let javascript = LanguageKind::JavaScript.with_evidence(
            Path::new("tools")
                .join("tree-sitter")
                .join("package.json")
                .to_string_lossy()
                .into_owned(),
        );

        let tasks = vec![
            IndexerTask {
                entry: REGISTRY.runnable_for(&typescript).unwrap(),
                lang: typescript,
                binary_path: None,
                project_root: root.clone(),
                additional_configs: Vec::new(),
                owned_child_prefixes: Vec::new(),
                backend_preference: BackendPreference::auto(),
                args_override: None,
                covers: Vec::new(),
            },
            IndexerTask {
                entry: REGISTRY.runnable_for(&javascript).unwrap(),
                lang: javascript,
                binary_path: None,
                project_root: root,
                additional_configs: Vec::new(),
                owned_child_prefixes: Vec::new(),
                backend_preference: BackendPreference::auto(),
                args_override: None,
                covers: Vec::new(),
            },
        ];

        let deduped = dedupe_tasks_by_indexer(tasks, true);

        let kinds = deduped
            .iter()
            .map(|task| task.lang.kind)
            .collect::<Vec<_>>();
        assert_eq!(
            kinds,
            vec![LanguageKind::TypeScript, LanguageKind::JavaScript]
        );
        assert!(deduped.iter().all(|task| task.covers.is_empty()));
    }

    #[test]
    fn publication_message_marks_single_output_with_failures_as_partial() {
        let message = index_publication_message(Path::new("index.scip"), 1, 3, false);

        assert!(message.contains("Partial index written to index.scip"));
        assert!(message.contains("1 successful output"));
        assert!(message.contains("3 failed language(s)"));
    }

    #[test]
    fn all_roots_include_additional_configs_discovers_config_only_roots() {
        let (_dir, root) = fixture(&["tools/tsconfig.scripts.json"]);
        let mut args = base_args();
        args.all_roots = true;
        args.include_additional_configs = true;

        let roots = resolve_project_roots(&args, &root).unwrap();
        let projects = detect_languages_for_roots(&args, &roots).unwrap();

        assert_eq!(roots, vec![root.join("tools")]);
        assert_eq!(projects[0].languages.len(), 1);
        assert_eq!(projects[0].languages[0].kind, LanguageKind::TypeScript);
        assert_eq!(
            projects[0].languages[0].additional_configs,
            vec![root.join("tools").join("tsconfig.scripts.json")]
        );
    }

    #[test]
    fn all_roots_include_additional_configs_respects_language_filter() {
        let (_dir, root) = fixture(&["tools/tsconfig.scripts.json", "crates/app/Cargo.toml"]);
        let mut args = base_args();
        args.all_roots = true;
        args.include_additional_configs = true;
        args.lang = vec!["rust".to_string()];

        let roots = resolve_project_roots(&args, &root).unwrap();
        let projects = detect_languages_for_roots(&args, &roots).unwrap();

        assert_eq!(roots, vec![root.join("crates/app")]);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].languages.len(), 1);
        assert_eq!(projects[0].languages[0].kind, LanguageKind::Rust);
    }
}
