use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use console::style;
use futures_util::stream::{self, StreamExt};

use scip_io_core::config_discovery::{
    discover_additional_config_roots, discover_additional_configs,
    supported_additional_config_languages,
};
use scip_io_core::detect::{
    Language, LanguageScanOptions, discover_project_roots, scan_languages_with_options,
};
use scip_io_core::indexer::registry::REGISTRY;
use scip_io_core::indexer::{IndexerEntry, runner};
use scip_io_core::merge::merge_scip_files;
use scip_io_core::scip_language::prefix_scip_file_document_paths;

use super::IndexArgs;
use super::progress_handler::CliProgressHandler;

/// A single indexer task to be executed.
struct IndexerTask {
    lang: Language,
    entry: &'static IndexerEntry,
    binary_path: PathBuf,
    project_root: PathBuf,
    additional_configs: Vec<PathBuf>,
    /// Additional detected languages whose indexing is handled by the same
    /// tool invocation (e.g. `javascript` is covered by a single
    /// `scip-typescript` run when `tsconfig.json` has `allowJs: true`).
    covers: Vec<String>,
}

/// Languages detected for one project root.
struct ProjectLanguages {
    root: PathBuf,
    languages: Vec<Language>,
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

    let project_roots = resolve_project_roots(&args, &path)?;
    let projects = detect_languages_for_roots(&args, &project_roots)?;
    let total_languages = projects
        .iter()
        .map(|project| project.languages.len())
        .sum::<usize>();

    if total_languages == 0 {
        bail!("No supported languages found to index");
    }

    let is_json = args.format == "json";

    // Dry-run mode: show what would be done then exit
    if args.dry_run {
        return run_dry_run(&args, &projects, is_json);
    }

    if !is_json {
        print_index_plan(&projects, &path);
    }

    let progress = Arc::new(CliProgressHandler::new());

    // Phase 1: Ensure all indexers are installed (sequentially, to avoid
    // duplicate downloads of the same binary)
    let mut tasks = Vec::new();
    for project in &projects {
        for lang in &project.languages {
            let entry = REGISTRY
                .runnable_for(lang)
                .with_context(|| format!("No indexer registered for {}", lang.name()))?;

            let binary_path = entry.ensure_installed(progress.as_ref()).await?;

            tasks.push(IndexerTask {
                lang: lang.clone(),
                entry,
                binary_path,
                project_root: project.root.clone(),
                additional_configs: lang.additional_configs.clone(),
                covers: Vec::new(),
            });
        }
    }

    // Dedupe tasks whose indexer binary handles multiple languages in a
    // single invocation. The canonical case is scip-typescript, which
    // indexes both .ts and .js files from one run when tsconfig.json
    // has `allowJs: true`. Running it twice would just produce a
    // duplicate index. Detection still reports all languages; this is
    // only about how many times the tool is actually invoked.
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
    let results: Vec<IndexerResult> = stream::iter(tasks)
        .map(|task| {
            let dur = timeout_duration;
            let base_path = base_path_for_results.clone();
            async move {
                let lang_name = task.lang.name().to_string();
                let covers = task.covers.clone();
                let outcome = tokio::time::timeout(
                    dur,
                    runner::run_indexer_with_configs(
                        &task.binary_path,
                        task.entry,
                        &task.project_root,
                        &task.lang,
                        &task.additional_configs,
                    ),
                )
                .await;
                match outcome {
                    Ok(Ok(output)) => {
                        let outcome = prefix_output_paths_for_project_root(
                            &output,
                            &task.project_root,
                            &base_path,
                        )
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
    let mut failures = Vec::new();

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
        merge_scip_files(&scip_outputs, &args.output)?;
        if !is_json {
            println!(
                "{} Merged index written to {}",
                style("v").green().bold(),
                args.output.display()
            );
        }
    } else if scip_outputs.len() == 1 && !args.no_merge {
        std::fs::copy(&scip_outputs[0], &args.output)?;
        if !is_json {
            println!(
                "\n{} Index written to {}",
                style("v").green().bold(),
                args.output.display()
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
                "\n{} {} of {} indexer(s) failed",
                style("!").yellow().bold(),
                failures.len(),
                failures.len() + scip_outputs.len(),
            );
            // Return a partial-failure error so main.rs can set exit code 1
            bail!(
                "partial-failure: {} indexer(s) succeeded, {} failed",
                scip_outputs.len(),
                failures.len()
            );
        }
    }

    Ok(())
}

/// Resolve which project roots the index command should operate on.
fn resolve_project_roots(args: &IndexArgs, base_path: &Path) -> Result<Vec<PathBuf>> {
    if args.all_roots {
        let mut roots = discover_project_roots(base_path)?;
        if args.include_additional_configs && has_allowed_additional_config_language(args) {
            roots.extend(discover_additional_config_roots(base_path)?);
            roots.sort();
            roots.dedup();
        }
        if roots.is_empty() {
            bail!(
                "No language config roots found under {}",
                base_path.display()
            );
        }
        return Ok(roots);
    }

    if !args.roots.is_empty() {
        let canonical_base = base_path
            .canonicalize()
            .with_context(|| format!("Invalid base path: {}", base_path.display()))?;
        let mut seen = HashSet::new();
        let mut roots = Vec::new();
        for root in &args.roots {
            let candidate = if root.is_absolute() {
                root.clone()
            } else {
                base_path.join(root)
            };
            let candidate = candidate
                .canonicalize()
                .with_context(|| format!("Invalid project root: {}", candidate.display()))?;
            if !candidate.starts_with(&canonical_base) {
                bail!(
                    "Project root {} is outside base path {}",
                    candidate.display(),
                    base_path.display()
                );
            }
            if seen.insert(candidate.clone()) {
                roots.push(candidate);
            }
        }
        return Ok(roots);
    }

    Ok(vec![base_path.to_path_buf()])
}

fn detect_languages_for_roots(
    args: &IndexArgs,
    project_roots: &[PathBuf],
) -> Result<Vec<ProjectLanguages>> {
    project_roots
        .iter()
        .map(|root| {
            let detected = scan_languages_with_options(
                root,
                LanguageScanOptions {
                    max_depth: if args.all_roots { Some(1) } else { Some(3) },
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
            let languages = add_additional_configs_if_requested(args, root, languages)?;
            Ok(ProjectLanguages {
                root: root.clone(),
                languages,
            })
        })
        .collect()
}

fn add_additional_configs_if_requested(
    args: &IndexArgs,
    root: &Path,
    mut languages: Vec<Language>,
) -> Result<Vec<Language>> {
    if !args.include_additional_configs {
        return Ok(languages);
    }

    for language in &mut languages {
        language.additional_configs = discover_additional_configs(root, language.kind)?;
    }
    add_languages_from_additional_configs(args, root, &mut languages)?;

    Ok(languages)
}

fn add_languages_from_additional_configs(
    args: &IndexArgs,
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

        let configs = discover_additional_configs(root, kind)?;
        if let Some(first_config) = configs.first() {
            let evidence = display_project_root(first_config, root);
            languages.push(Language {
                kind,
                evidence,
                additional_configs: configs,
            });
        }
    }

    Ok(())
}

fn has_allowed_additional_config_language(args: &IndexArgs) -> bool {
    supported_additional_config_languages()
        .iter()
        .any(|&kind| language_filter_allows(args, kind))
}

fn language_filter_allows(args: &IndexArgs, kind: scip_io_core::LanguageKind) -> bool {
    args.lang.is_empty()
        || args
            .lang
            .iter()
            .any(|name| name.eq_ignore_ascii_case(kind.name()))
}

fn print_index_plan(projects: &[ProjectLanguages], base_path: &Path) {
    println!("{} Indexing:", style(">").cyan().bold());
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
fn run_dry_run(args: &IndexArgs, projects: &[ProjectLanguages], is_json: bool) -> Result<()> {
    if is_json {
        let plan: Vec<serde_json::Value> = projects
            .iter()
            .flat_map(|project| {
                project.languages.iter().map(|lang| {
                    let indexer = REGISTRY.runnable_for(lang);
                    serde_json::json!({
                        "root": project.root.display().to_string(),
                        "language": lang.name(),
                        "evidence": lang.evidence(),
                        "indexer": indexer.map(|e| &e.indexer_name),
                        "installed": indexer.map(|e| e.is_installed()).unwrap_or(false),
                        "command": indexer.map(|e| {
                            format!(
                                "{} {}",
                                e.binary_name,
                                dry_run_command_args(e, &project.root, lang).join(" ")
                            )
                        }),
                        "configs": lang.additional_configs.iter().map(|path| {
                            display_project_root(path, &project.root)
                        }).collect::<Vec<_>>(),
                    })
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        println!(
            "{} Dry run -- the following would be indexed:",
            style("*").cyan().bold()
        );
        for project in projects {
            println!("  {} {}", style("root").dim(), project.root.display());
            for lang in &project.languages {
                let indexer = REGISTRY.runnable_for(lang);
                let status = match indexer {
                    Some(e) if e.is_installed() => format!("{} (installed)", e.indexer_name),
                    Some(e) => format!("{} (will download)", e.indexer_name),
                    None => "no indexer registered".to_string(),
                };
                println!("    {} {} -- {}", style(">").cyan(), lang.name(), status);
                if let Some(e) = indexer {
                    println!(
                        "      command: {} {}",
                        e.binary_name,
                        dry_run_command_args(e, &project.root, lang).join(" ")
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

fn dry_run_command_args(entry: &IndexerEntry, project_root: &Path, lang: &Language) -> Vec<String> {
    let output_file = PathBuf::from(format!("{}.scip", lang.name()));
    let display_configs = lang
        .additional_configs
        .iter()
        .map(|path| PathBuf::from(display_project_root(path, project_root)))
        .collect::<Vec<_>>();

    runner::build_indexer_args(entry, &output_file, &display_configs)
        .into_iter()
        .map(|arg| arg.to_string_lossy().to_string())
        .collect()
}

/// Group tasks by their `indexer_name` and project root, then collapse each
/// group to a single invocation. When multiple detected languages in the same
/// project root resolve to the same indexer tool (e.g. scip-typescript handles
/// both TypeScript and JavaScript), running it once is enough. The same indexer
/// in a different project root remains a separate task.
fn dedupe_tasks_by_indexer(tasks: Vec<IndexerTask>, is_json: bool) -> Vec<IndexerTask> {
    let mut grouped: HashMap<String, Vec<IndexerTask>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for task in tasks {
        let key = format!(
            "{}\0{}",
            task.project_root.display(),
            task.entry.indexer_name
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

        // Pick the primary: prefer args without `--infer-tsconfig`,
        // then fall back to lexicographic language name for determinism.
        group.sort_by(|a, b| {
            let a_infer = a.entry.default_args.iter().any(|x| x == "--infer-tsconfig");
            let b_infer = b.entry.default_args.iter().any(|x| x == "--infer-tsconfig");
            a_infer
                .cmp(&b_infer)
                .then_with(|| a.lang.name().cmp(b.lang.name()))
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
#[cfg(test)]
mod tests {
    use super::{IndexArgs, detect_languages_for_roots, resolve_project_roots};
    use scip_io_core::LanguageKind;
    use std::fs;
    use std::path::PathBuf;
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
            include_additional_configs: false,
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
        let (_dir, root) = fixture(&["services/api/Cargo.toml", "packages/web/package.json"]);
        let mut args = base_args();
        args.all_roots = true;

        let roots = resolve_project_roots(&args, &root).unwrap();
        assert_eq!(
            roots,
            vec![root.join("packages/web"), root.join("services/api")]
        );
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
