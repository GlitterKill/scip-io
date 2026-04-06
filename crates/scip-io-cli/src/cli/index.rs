use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use console::style;
use futures_util::stream::{self, StreamExt};

use scip_io_core::detect::{Language, scan_languages};
use scip_io_core::indexer::registry::REGISTRY;
use scip_io_core::indexer::{IndexerEntry, runner};
use scip_io_core::merge::merge_scip_files;

use super::IndexArgs;
use super::progress_handler::CliProgressHandler;

/// A single indexer task to be executed.
struct IndexerTask {
    lang: Language,
    entry: &'static IndexerEntry,
    binary_path: PathBuf,
    project_root: PathBuf,
}

/// Result of running a single indexer.
struct IndexerResult {
    lang_name: String,
    outcome: Result<PathBuf>,
}

pub async fn run(args: IndexArgs) -> Result<()> {
    let path = args
        .path
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap());
    let path = path.canonicalize()?;

    // Detect or filter languages
    let languages = if args.lang.is_empty() {
        scan_languages(&path)?
    } else {
        let detected = scan_languages(&path)?;
        detected
            .into_iter()
            .filter(|l| {
                args.lang
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(l.name()))
            })
            .collect()
    };

    if languages.is_empty() {
        bail!("No supported languages found to index");
    }

    let is_json = args.format == "json";

    // Dry-run mode: show what would be done then exit
    if args.dry_run {
        return run_dry_run(&args, &languages, is_json);
    }

    if !is_json {
        println!(
            "{} Indexing: {}",
            style(">").cyan().bold(),
            languages
                .iter()
                .map(|l| l.name())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    let progress = Arc::new(CliProgressHandler::new());

    // Phase 1: Ensure all indexers are installed (sequentially, to avoid
    // duplicate downloads of the same binary)
    let mut tasks = Vec::new();
    for lang in &languages {
        let entry = REGISTRY
            .get(lang)
            .with_context(|| format!("No indexer registered for {}", lang.name()))?;

        let binary_path = entry.ensure_installed(progress.as_ref()).await?;

        tasks.push(IndexerTask {
            lang: lang.clone(),
            entry,
            binary_path,
            project_root: path.clone(),
        });
    }

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

    let results: Vec<IndexerResult> = stream::iter(tasks)
        .map(|task| {
            let dur = timeout_duration;
            async move {
                let lang_name = task.lang.name().to_string();
                let outcome = tokio::time::timeout(
                    dur,
                    runner::run_indexer(
                        &task.binary_path,
                        task.entry,
                        &task.project_root,
                        &task.lang,
                    ),
                )
                .await;
                match outcome {
                    Ok(result) => IndexerResult {
                        lang_name,
                        outcome: result,
                    },
                    Err(_) => IndexerResult {
                        lang_name: lang_name.clone(),
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
                }
                failures.push((result.lang_name, format!("{:#}", err)));
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
            "languages": languages.iter().map(|l| l.name()).collect::<Vec<_>>(),
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

/// Dry-run mode: show what would happen without executing anything.
fn run_dry_run(args: &IndexArgs, languages: &[Language], is_json: bool) -> Result<()> {
    if is_json {
        let plan: Vec<serde_json::Value> = languages
            .iter()
            .map(|lang| {
                let indexer = REGISTRY.get(lang);
                serde_json::json!({
                    "language": lang.name(),
                    "evidence": lang.evidence(),
                    "indexer": indexer.map(|e| &e.indexer_name),
                    "installed": indexer.map(|e| e.is_installed()).unwrap_or(false),
                    "command": indexer.map(|e| {
                        format!("{} {}", e.binary_name, e.default_args.join(" "))
                    }),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&plan)?);
    } else {
        println!(
            "{} Dry run -- the following would be indexed:",
            style("*").cyan().bold()
        );
        for lang in languages {
            let indexer = REGISTRY.get(lang);
            let status = match indexer {
                Some(e) if e.is_installed() => format!("{} (installed)", e.indexer_name),
                Some(e) => format!("{} (will download)", e.indexer_name),
                None => "no indexer registered".to_string(),
            };
            println!("  {} {} -- {}", style(">").cyan(), lang.name(), status);
            if let Some(e) = indexer {
                println!(
                    "    command: {} {}",
                    e.binary_name,
                    e.default_args.join(" ")
                );
            }
        }
        if !args.no_merge && languages.len() > 1 {
            println!(
                "  {} Merge output: {}",
                style(">").cyan(),
                args.output.display()
            );
        }
    }
    Ok(())
}
