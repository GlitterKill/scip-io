use std::collections::{HashSet, VecDeque};
use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::future::{BoxFuture, FutureExt};
use futures_util::stream::{FuturesUnordered, StreamExt};
use sha2::{Digest, Sha256};

use crate::detect::Language;
use crate::indexer::IndexerEntry;
use crate::indexer::backend::{self, BackendExecutionRequest, BackendPreference};
use crate::indexer::planner::{self, PlannedShard};
use crate::merge::merge_scip_files;
use crate::scip_language::{
    ScipCompactionStats, compact_scip_file, normalize_path_component,
    normalize_scip_file_languages, prefix_scip_file_document_paths, publish_scip_file_atomically,
    relativize_scip_file_document_paths, replace_empty_scip_document_paths,
};
use crate::toolchain::{ToolchainsConfig, require_toolchain_environment_for_indexer};
use crate::validate::{IndexStats, validate_scip_file};

const PYTHON_SHARD_TRIGGER_FILE_LIMIT: usize = 750;
const PYTHON_SHARD_TARGET_FILE_LIMIT: usize = 5_000;
const PYTHON_PARALLEL_SHARD_FILE_LIMIT: usize = PYTHON_SHARD_TRIGGER_FILE_LIMIT;
const PYTHON_MAX_PARALLEL_SHARDS: usize = 2;
const COMPILE_COMMANDS_SHARD_COMMAND_LIMIT: usize = 5_000;
const PROJECT_ARGUMENT_SHARD_CONFIG_LIMIT: usize = 64;
const DEFAULT_PYTHON_SHARD_POLICY: PythonShardPolicy = PythonShardPolicy {
    trigger_file_limit: PYTHON_SHARD_TRIGGER_FILE_LIMIT,
    target_file_limit: PYTHON_SHARD_TARGET_FILE_LIMIT,
};
const IGNORED_PYTHON_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "target",
    "dist",
    "build",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct PythonShard {
    target: PathBuf,
    file_count: usize,
    files: Option<Vec<PathBuf>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PythonShardPolicy {
    trigger_file_limit: usize,
    target_file_limit: usize,
}

impl PythonShardPolicy {
    #[cfg(test)]
    fn uniform(file_limit: usize) -> Self {
        Self {
            trigger_file_limit: file_limit,
            target_file_limit: file_limit,
        }
    }

    fn normalized(self) -> Self {
        let trigger_file_limit = self.trigger_file_limit.max(1);
        Self {
            trigger_file_limit,
            target_file_limit: self.target_file_limit.max(trigger_file_limit),
        }
    }
}

/// Run an indexer binary against a project root and return the output .scip path.
pub async fn run_indexer(
    binary: &Path,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
) -> Result<PathBuf> {
    run_indexer_with_configs(binary, entry, project_root, lang, &[]).await
}

/// Run an indexer binary against a project root with optional config files.
pub async fn run_indexer_with_configs(
    binary: &Path,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    config_paths: &[PathBuf],
) -> Result<PathBuf> {
    run_indexer_with_configs_and_backend(
        Some(binary),
        entry,
        project_root,
        lang,
        config_paths,
        BackendPreference::native(),
    )
    .await
}

pub async fn run_indexer_with_configs_and_backend(
    binary: Option<&Path>,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    config_paths: &[PathBuf],
    backend_preference: BackendPreference,
) -> Result<PathBuf> {
    let toolchains = ToolchainsConfig::default();
    run_indexer_with_configs_backend_and_toolchains(
        binary,
        entry,
        project_root,
        lang,
        config_paths,
        backend_preference,
        &toolchains,
    )
    .await
}

pub async fn run_indexer_with_configs_backend_and_toolchains(
    binary: Option<&Path>,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    config_paths: &[PathBuf],
    backend_preference: BackendPreference,
    toolchains: &ToolchainsConfig,
) -> Result<PathBuf> {
    run_indexer_with_request(IndexerRunRequest {
        binary,
        entry,
        project_root,
        lang,
        config_paths,
        backend_preference,
        toolchains,
        args_override: None,
    })
    .await
}

pub struct IndexerRunRequest<'a> {
    pub binary: Option<&'a Path>,
    pub entry: &'a IndexerEntry,
    pub project_root: &'a Path,
    pub lang: &'a Language,
    pub config_paths: &'a [PathBuf],
    pub backend_preference: BackendPreference,
    pub toolchains: &'a ToolchainsConfig,
    pub args_override: Option<&'a [String]>,
}

pub async fn run_indexer_with_request(request: IndexerRunRequest<'_>) -> Result<PathBuf> {
    if request.entry.indexer_name == "scip-python" && request.config_paths.is_empty() {
        return run_python_indexer_with_policy(
            request.binary,
            request.entry,
            request.project_root,
            request.lang,
            DEFAULT_PYTHON_SHARD_POLICY,
            &request.backend_preference,
            request.toolchains,
        )
        .await;
    }

    if should_shard_project_arguments_upfront(request.entry, request.config_paths) {
        tracing::info!(
            indexer = %request.entry.indexer_name,
            lang = request.lang.name(),
            configs = request.config_paths.len(),
            config_limit = PROJECT_ARGUMENT_SHARD_CONFIG_LIMIT,
            "running indexer with project/config argument shards"
        );
        return run_project_argument_sharded_indexer(
            request.binary,
            request.entry,
            request.project_root,
            request.lang,
            request.config_paths,
            &request.backend_preference,
            request.toolchains,
        )
        .await;
    }

    if request.entry.indexer_name == "scip-clang" && request.config_paths.is_empty() {
        let compile_commands = request.project_root.join("compile_commands.json");
        if compile_commands.exists() {
            let shards = planner::plan_compile_command_shards(
                &compile_commands,
                COMPILE_COMMANDS_SHARD_COMMAND_LIMIT,
            )?;
            if !shards.is_empty() {
                return run_compile_command_sharded_indexer(
                    request.binary,
                    request.entry,
                    request.project_root,
                    request.lang,
                    shards,
                    &request.backend_preference,
                    request.toolchains,
                )
                .await;
            }
        }
    }

    match run_indexer_once_with_configs(&request).await {
        Ok(output) => Ok(output),
        Err(error)
            if should_retry_with_project_argument_shards(
                request.entry,
                request.config_paths,
                &error,
            ) =>
        {
            tracing::warn!(
                indexer = %request.entry.indexer_name,
                lang = request.lang.name(),
                error = %error,
                shards = request.config_paths.len(),
                "retrying indexer with project/config argument shards"
            );
            run_project_argument_sharded_indexer(
                request.binary,
                request.entry,
                request.project_root,
                request.lang,
                request.config_paths,
                &request.backend_preference,
                request.toolchains,
            )
            .await
            .with_context(|| {
                format!(
                    "{} failed as a single {} run before project/config sharding: {error:#}",
                    request.entry.indexer_name,
                    request.lang.name()
                )
            })
        }
        Err(error) => Err(error),
    }
}

async fn run_indexer_once_with_configs(request: &IndexerRunRequest<'_>) -> Result<PathBuf> {
    tracing::info!(
        indexer = %request.entry.indexer_name,
        lang = request.lang.name(),
        root = %request.project_root.display(),
        "running indexer"
    );

    // Build the output path so multiple indexers don't clobber each other
    let output_file = request
        .project_root
        .join(format!("{}.scip", request.lang.name()));
    let temp_dir = tempfile::Builder::new()
        .prefix("scip-io-indexer-")
        .tempdir()
        .context("Failed to create temporary directory for indexer output")?;
    let run = run_indexer_to_temp_output(TempOutputRequest {
        binary: request.binary,
        entry: request.entry,
        project_root: request.project_root,
        lang: request.lang,
        config_paths: request.config_paths,
        temp_dir: temp_dir.path(),
        output_name: &format!("{}.scip", request.lang.name()),
        backend_preference: &request.backend_preference,
        toolchains: request.toolchains,
        args_override: request.args_override,
    })
    .await?;
    publish_scip_file_atomically(&run.path, &output_file)?;
    tracing::info!(
        indexer = %request.entry.indexer_name,
        lang = request.lang.name(),
        output = %output_file.display(),
        elapsed_ms = run.elapsed.as_millis(),
        output_bytes = run.output_bytes,
        documents = run.stats.documents,
        symbols = run.stats.symbols,
        occurrences = run.stats.occurrences,
        duplicate_documents = run.compaction.duplicate_documents,
        duplicate_occurrences = run.compaction.duplicate_occurrences,
        duplicate_symbols = run.compaction.duplicate_symbols,
        "finished indexer"
    );

    Ok(output_file)
}

async fn run_project_argument_sharded_indexer(
    binary: Option<&Path>,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    config_paths: &[PathBuf],
    backend_preference: &BackendPreference,
    toolchains: &ToolchainsConfig,
) -> Result<PathBuf> {
    let planned_shards = planner::plan_project_argument_shards(entry, config_paths);
    if planned_shards.is_empty() {
        return run_indexer_once_with_configs(&IndexerRunRequest {
            binary,
            entry,
            project_root,
            lang,
            config_paths,
            backend_preference: backend_preference.clone(),
            toolchains,
            args_override: None,
        })
        .await;
    }

    let output_file = project_root.join(format!("{}.scip", lang.name()));
    let temp_dir = tempfile::Builder::new()
        .prefix("scip-io-project-shards-")
        .tempdir()
        .context("Failed to create temporary directory for project/config shards")?;
    let started = Instant::now();
    let mut shard_outputs = Vec::with_capacity(planned_shards.len());

    for (index, shard) in planned_shards.iter().enumerate() {
        let PlannedShard::ProjectArgument(config_path) = shard else {
            continue;
        };
        let output_name = format!("{}-project-shard-{index:04}.scip", lang.name());
        let run = run_indexer_to_temp_output(TempOutputRequest {
            binary,
            entry,
            project_root,
            lang,
            config_paths: std::slice::from_ref(config_path),
            temp_dir: temp_dir.path(),
            output_name: &output_name,
            backend_preference,
            toolchains,
            args_override: None,
        })
        .await
        .with_context(|| {
            format!(
                "{} failed for {} shard {} ({})",
                entry.indexer_name,
                lang.name(),
                index + 1,
                config_path.display()
            )
        })?;
        shard_outputs.push(run.path);
    }

    merge_postprocess_and_publish_shards(
        &shard_outputs,
        ShardPublishContext {
            temp_dir: temp_dir.path(),
            output_file: &output_file,
            project_root,
            entry,
            lang,
            shard_kind: "project/config argument shards",
            elapsed: started.elapsed(),
        },
    )
}

async fn run_compile_command_sharded_indexer(
    binary: Option<&Path>,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    planned_shards: Vec<PlannedShard>,
    backend_preference: &BackendPreference,
    toolchains: &ToolchainsConfig,
) -> Result<PathBuf> {
    run_compile_command_sharded_indexer_with_plan(
        binary,
        entry,
        project_root,
        lang,
        planned_shards,
        backend_preference,
        toolchains,
    )
    .await
}

async fn run_compile_command_sharded_indexer_with_plan(
    binary: Option<&Path>,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    planned_shards: Vec<PlannedShard>,
    backend_preference: &BackendPreference,
    toolchains: &ToolchainsConfig,
) -> Result<PathBuf> {
    let output_file = project_root.join(format!("{}.scip", lang.name()));
    let temp_dir = tempfile::Builder::new()
        .prefix("scip-io-compile-command-shards-")
        .tempdir()
        .context("Failed to create temporary directory for compile command shards")?;
    let started = Instant::now();
    let mut shard_outputs = Vec::with_capacity(planned_shards.len());

    for (index, shard) in planned_shards.iter().enumerate() {
        let PlannedShard::CompileCommands {
            compile_commands,
            start,
            end,
        } = shard
        else {
            continue;
        };
        let chunk = planner::read_compile_command_chunk(compile_commands, *start, *end)?;
        let chunk_path = temp_dir
            .path()
            .join(format!("compile_commands-{index:04}.json"));
        std::fs::write(&chunk_path, serde_json::to_vec(&chunk)?)
            .with_context(|| format!("Failed to write {}", chunk_path.display()))?;

        let output_name = format!("{}-compile-shard-{index:04}.scip", lang.name());
        let args = build_compile_command_shard_args(entry, &chunk_path);
        let run = run_indexer_to_temp_output_with_args(ProtectedIndexerRun {
            binary,
            entry,
            project_root,
            lang,
            temp_dir: temp_dir.path(),
            output_name: &output_name,
            args,
            explicit_output: false,
            backend_preference: backend_preference.clone(),
            toolchains,
        })
        .await
        .with_context(|| {
            format!(
                "{} failed for compile_commands shard {} ({}..{})",
                entry.indexer_name,
                index + 1,
                start,
                end
            )
        })?;
        shard_outputs.push(run.path);
    }

    merge_postprocess_and_publish_shards(
        &shard_outputs,
        ShardPublishContext {
            temp_dir: temp_dir.path(),
            output_file: &output_file,
            project_root,
            entry,
            lang,
            shard_kind: "compile_commands chunks",
            elapsed: started.elapsed(),
        },
    )
}

struct ShardPublishContext<'a> {
    temp_dir: &'a Path,
    output_file: &'a Path,
    project_root: &'a Path,
    entry: &'a IndexerEntry,
    lang: &'a Language,
    shard_kind: &'a str,
    elapsed: Duration,
}

fn merge_postprocess_and_publish_shards(
    shard_outputs: &[PathBuf],
    context: ShardPublishContext<'_>,
) -> Result<PathBuf> {
    let merged_output = context
        .temp_dir
        .join(format!("{}-merged.scip", context.lang.name()));
    merge_scip_files(shard_outputs, &merged_output)?;
    let postprocess = postprocess_scip_output(
        &merged_output,
        context.project_root,
        context.entry,
        context.lang,
    )?;
    let output_bytes = std::fs::metadata(&merged_output)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    publish_scip_file_atomically(&merged_output, context.output_file)?;

    tracing::info!(
        indexer = %context.entry.indexer_name,
        lang = context.lang.name(),
        shards = shard_outputs.len(),
        shard_kind = context.shard_kind,
        output = %context.output_file.display(),
        elapsed_ms = context.elapsed.as_millis(),
        output_bytes,
        documents = postprocess.stats.documents,
        symbols = postprocess.stats.symbols,
        occurrences = postprocess.stats.occurrences,
        duplicate_documents = postprocess.compaction.duplicate_documents,
        duplicate_occurrences = postprocess.compaction.duplicate_occurrences,
        duplicate_symbols = postprocess.compaction.duplicate_symbols,
        "finished sharded indexer"
    );

    Ok(context.output_file.to_path_buf())
}

struct ProtectedIndexerOutput {
    path: PathBuf,
    stats: IndexStats,
    compaction: ScipCompactionStats,
    output_bytes: u64,
    elapsed: Duration,
}

struct TempOutputRequest<'a> {
    binary: Option<&'a Path>,
    entry: &'a IndexerEntry,
    project_root: &'a Path,
    lang: &'a Language,
    config_paths: &'a [PathBuf],
    temp_dir: &'a Path,
    output_name: &'a str,
    backend_preference: &'a BackendPreference,
    toolchains: &'a ToolchainsConfig,
    args_override: Option<&'a [String]>,
}

async fn run_indexer_to_temp_output(
    request: TempOutputRequest<'_>,
) -> Result<ProtectedIndexerOutput> {
    let temp_output = request.temp_dir.join(request.output_name);
    let explicit_output =
        indexer_supports_explicit_output_arg(request.entry) || !request.config_paths.is_empty();
    let default_args = request.args_override.unwrap_or(&request.entry.default_args);
    let args = build_indexer_args_for_project_with_defaults(
        request.entry,
        &temp_output,
        request.config_paths,
        request.project_root,
        default_args,
    );

    run_indexer_to_temp_output_with_args(ProtectedIndexerRun {
        binary: request.binary,
        entry: request.entry,
        project_root: request.project_root,
        lang: request.lang,
        temp_dir: request.temp_dir,
        output_name: request.output_name,
        args,
        explicit_output,
        backend_preference: request.backend_preference.clone(),
        toolchains: request.toolchains,
    })
    .await
}

struct ProtectedIndexerRun<'a> {
    binary: Option<&'a Path>,
    entry: &'a IndexerEntry,
    project_root: &'a Path,
    lang: &'a Language,
    temp_dir: &'a Path,
    output_name: &'a str,
    args: Vec<OsString>,
    explicit_output: bool,
    backend_preference: BackendPreference,
    toolchains: &'a ToolchainsConfig,
}

struct PythonIndexerOptions<'a> {
    policy: PythonShardPolicy,
    use_persistent_hints: bool,
    backend_preference: &'a BackendPreference,
    toolchains: &'a ToolchainsConfig,
}

async fn run_indexer_to_temp_output_with_args(
    request: ProtectedIndexerRun<'_>,
) -> Result<ProtectedIndexerOutput> {
    let started = Instant::now();
    let temp_output = request.temp_dir.join(request.output_name);
    let prepared = backend::prepare_execution(BackendExecutionRequest {
        native_binary: request.binary,
        entry: request.entry,
        project_root: request.project_root,
        temp_dir: request.temp_dir,
        output_name: request.output_name,
        args: request.args,
        preference: request.backend_preference,
    })
    .await?;
    let mut cmd = tokio::process::Command::new(&prepared.program);
    if let Some(current_dir) = &prepared.current_dir {
        cmd.current_dir(current_dir);
    }
    cmd.kill_on_drop(true);

    for arg in &prepared.args {
        cmd.arg(arg);
    }

    if prepared.backend == backend::ExecutionBackendKind::Native
        && let Some(environment) =
            require_toolchain_environment_for_indexer(request.entry, request.toolchains)?
    {
        environment.apply_to_command(&mut cmd)?;
        tracing::info!(
            indexer = %request.entry.indexer_name,
            toolchain = environment.kind.as_str(),
            home = environment.home.as_ref().map(|path| path.display().to_string()),
            executable = %environment.executable.display(),
            "injected toolchain environment for native indexer"
        );
    }

    let default_guard = DefaultOutputGuard::prepare(
        request.project_root,
        &request.entry.output_file,
        request.temp_dir,
    )?;

    let output = cmd
        .output()
        .await
        .with_context(|| format!("Failed to execute {}", prepared.display_command))?;
    let elapsed = started.elapsed();

    if !output.status.success() {
        anyhow::bail!(
            "{} exited with status {} for {}\nstdout:\n{}\nstderr:\n{}",
            request.entry.indexer_name,
            output.status,
            request.lang.name(),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    if !temp_output.exists() {
        if request.explicit_output {
            anyhow::bail!(
                "Indexer {} did not produce output file (expected {})",
                request.entry.indexer_name,
                prepared.output_path_on_host.display()
            );
        }

        let default_output = request.project_root.join(&request.entry.output_file);
        if default_output.exists() {
            move_file_or_copy_across_devices(&default_output, &temp_output).with_context(|| {
                format!(
                    "Failed to move default indexer output {} to {}",
                    default_output.display(),
                    temp_output.display()
                )
            })?;
        } else {
            anyhow::bail!(
                "Indexer {} did not produce output file (expected {})",
                request.entry.indexer_name,
                prepared.output_path_on_host.display()
            );
        }
    }
    drop(default_guard);

    let postprocess = postprocess_scip_output(
        &temp_output,
        request.project_root,
        request.entry,
        request.lang,
    )?;
    let output_bytes = std::fs::metadata(&temp_output)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    tracing::info!(
        indexer = %request.entry.indexer_name,
        lang = request.lang.name(),
        output = %temp_output.display(),
        elapsed_ms = elapsed.as_millis(),
        output_bytes,
        documents = postprocess.stats.documents,
        symbols = postprocess.stats.symbols,
        occurrences = postprocess.stats.occurrences,
        duplicate_documents = postprocess.compaction.duplicate_documents,
        duplicate_occurrences = postprocess.compaction.duplicate_occurrences,
        duplicate_symbols = postprocess.compaction.duplicate_symbols,
        "finished indexer"
    );

    Ok(ProtectedIndexerOutput {
        path: temp_output,
        stats: postprocess.stats,
        compaction: postprocess.compaction,
        output_bytes,
        elapsed,
    })
}

struct PostprocessedScipOutput {
    stats: IndexStats,
    compaction: ScipCompactionStats,
}

fn postprocess_scip_output(
    output_file: &Path,
    project_root: &Path,
    entry: &IndexerEntry,
    lang: &Language,
) -> Result<PostprocessedScipOutput> {
    let updated_languages = normalize_scip_file_languages(output_file, Some(lang.name()))?;
    if updated_languages > 0 {
        tracing::info!(
            path = %output_file.display(),
            docs = updated_languages,
            "filled missing SCIP document languages"
        );
    }
    let updated_paths = relativize_scip_file_document_paths(output_file, project_root)?;
    if updated_paths > 0 {
        tracing::info!(
            path = %output_file.display(),
            docs = updated_paths,
            "relativized SCIP document paths"
        );
    }
    let compaction = compact_scip_file(output_file)?;
    if compaction.changed() {
        tracing::info!(
            path = %output_file.display(),
            duplicate_documents = compaction.duplicate_documents,
            duplicate_occurrences = compaction.duplicate_occurrences,
            duplicate_symbols = compaction.duplicate_symbols,
            "compacted duplicate SCIP facts"
        );
    }

    let validation = validate_scip_file(output_file)?;
    if !validation.valid {
        let errors = validation
            .errors
            .iter()
            .map(|error| format!("{}: {}", error.kind, error.message))
            .collect::<Vec<_>>()
            .join("; ");
        anyhow::bail!(
            "{} produced invalid {} SCIP output after normalization: {}",
            entry.indexer_name,
            lang.name(),
            errors
        );
    }

    Ok(PostprocessedScipOutput {
        stats: validation.stats.unwrap_or_default(),
        compaction,
    })
}

struct DefaultOutputGuard {
    default_output: PathBuf,
    backup_output: PathBuf,
    restore_backup: bool,
}

impl DefaultOutputGuard {
    fn prepare(project_root: &Path, output_file: &str, temp_dir: &Path) -> Result<Self> {
        let default_output = project_root.join(output_file);
        let backup_output = temp_dir.join("original-default-output.scip");
        let restore_backup = default_output.exists();
        if restore_backup {
            move_file_or_copy_across_devices(&default_output, &backup_output).with_context(
                || {
                    format!(
                        "Failed to move existing default indexer output {} to {}",
                        default_output.display(),
                        backup_output.display()
                    )
                },
            )?;
        }

        Ok(Self {
            default_output,
            backup_output,
            restore_backup,
        })
    }
}

impl Drop for DefaultOutputGuard {
    fn drop(&mut self) {
        if self.restore_backup && self.backup_output.exists() {
            if self.default_output.exists()
                && let Err(error) = std::fs::remove_file(&self.default_output)
            {
                tracing::warn!(
                    path = %self.default_output.display(),
                    error = %error,
                    "failed to remove uncommitted default SCIP output before restore"
                );
                return;
            }
            if let Err(error) =
                move_file_or_copy_across_devices(&self.backup_output, &self.default_output)
            {
                tracing::warn!(
                    backup = %self.backup_output.display(),
                    destination = %self.default_output.display(),
                    error = %error,
                    "failed to restore previous default SCIP output"
                );
            }
        }
    }
}

fn move_file_or_copy_across_devices(source: &Path, destination: &Path) -> std::io::Result<()> {
    match std::fs::rename(source, destination) {
        Ok(()) => Ok(()),
        Err(error) if is_cross_device_rename_error(&error) => {
            std::fs::copy(source, destination)?;
            std::fs::remove_file(source)?;
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn is_cross_device_rename_error(error: &std::io::Error) -> bool {
    match error.raw_os_error() {
        #[cfg(windows)]
        Some(17) => true,
        #[cfg(unix)]
        Some(18) => true,
        _ => false,
    }
}

#[cfg(test)]
async fn run_python_indexer_with_file_limit(
    binary: &Path,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    max_files_per_shard: usize,
) -> Result<PathBuf> {
    let backend_preference = BackendPreference::native();
    let toolchains = ToolchainsConfig::default();
    run_python_indexer_with_policy_and_hints(
        Some(binary),
        entry,
        project_root,
        lang,
        PythonIndexerOptions {
            policy: PythonShardPolicy::uniform(max_files_per_shard),
            use_persistent_hints: false,
            backend_preference: &backend_preference,
            toolchains: &toolchains,
        },
    )
    .await
}

async fn run_python_indexer_with_policy(
    binary: Option<&Path>,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    policy: PythonShardPolicy,
    backend_preference: &BackendPreference,
    toolchains: &ToolchainsConfig,
) -> Result<PathBuf> {
    run_python_indexer_with_policy_and_hints(
        binary,
        entry,
        project_root,
        lang,
        PythonIndexerOptions {
            policy,
            use_persistent_hints: true,
            backend_preference,
            toolchains,
        },
    )
    .await
}

async fn run_python_indexer_with_policy_and_hints(
    binary: Option<&Path>,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    options: PythonIndexerOptions<'_>,
) -> Result<PathBuf> {
    let binary = binary.context("scip-python requires a native Node-based indexer binary")?;
    let policy = options.policy.normalized();
    let mut shards = plan_python_shards(project_root, policy)?;
    let python_file_count = shards.iter().map(|shard| shard.file_count).sum::<usize>();
    if shards.is_empty() || (shards.len() == 1 && shards[0].target.as_os_str().is_empty()) {
        return run_indexer_once_with_configs(&IndexerRunRequest {
            binary: Some(binary),
            entry,
            project_root,
            lang,
            config_paths: &[],
            backend_preference: options.backend_preference.clone(),
            toolchains: options.toolchains,
            args_override: None,
        })
        .await;
    }
    let mut shard_hints = if options.use_persistent_hints {
        load_python_shard_hints(project_root)
    } else {
        HashSet::new()
    };
    if !shard_hints.is_empty() {
        let hinted_targets = shards
            .iter()
            .filter(|shard| shard_hints.contains(&python_shard_target_key(&shard.target)))
            .count();
        if hinted_targets > 0 {
            shards = apply_python_shard_hints(project_root, shards, &shard_hints, policy)?;
            tracing::info!(
                targets = hinted_targets,
                shards = shards.len(),
                "pre-split scip-python shards from saved heap-limit hints"
            );
        }
    }

    let output_file = project_root.join(format!("{}.scip", lang.name()));

    tracing::info!(
        indexer = %entry.indexer_name,
        root = %project_root.display(),
        files = python_file_count,
        shards = shards.len(),
        trigger_file_limit = policy.trigger_file_limit,
        target_file_limit = policy.target_file_limit,
        parallel_shards = python_shard_parallelism(),
        parallel_file_limit = PYTHON_PARALLEL_SHARD_FILE_LIMIT,
        "running scip-python in memory-bounded shards"
    );

    let temp_dir = tempfile::Builder::new()
        .prefix("scip-io-python-shards-")
        .tempdir()
        .context("Failed to create temporary directory for scip-python shards")?;

    let mut shard_outputs = Vec::<PythonShardOutput>::new();
    let mut queue = VecDeque::from(shards);
    let mut shard_counter = 0usize;
    let mut stats = PythonShardRunStats::default();
    let started = Instant::now();
    let mut active = FuturesUnordered::<BoxFuture<'_, Result<PythonShardRun>>>::new();

    loop {
        while active.len() < python_shard_parallelism() {
            let Some(next_shard) = queue.front() else {
                break;
            };
            let next_is_parallel = can_run_python_shard_in_parallel(next_shard);
            if !next_is_parallel && !active.is_empty() {
                break;
            }

            let shard = queue
                .pop_front()
                .expect("queue front exists after earlier check");
            let shard_is_parallel = can_run_python_shard_in_parallel(&shard);
            shard_counter += 1;
            active.push(
                run_python_shard(
                    binary,
                    entry,
                    project_root,
                    lang,
                    temp_dir.path(),
                    shard_counter,
                    shard,
                )
                .boxed(),
            );

            if !shard_is_parallel {
                break;
            }
        }

        let Some(run) = active.next().await else {
            break;
        };

        match run? {
            PythonShardRun::Succeeded(success) => {
                stats.record_success(success.elapsed);
                shard_outputs.push(PythonShardOutput {
                    shard_number: success.shard_number,
                    path: success.output,
                });
            }
            PythonShardRun::Failed(failure) if failure.is_oom() => {
                stats.record_failure(failure.elapsed);
                stats.oom_retries += 1;
                shard_hints.insert(python_shard_target_key(&failure.shard.target));
                let child_shards = split_python_shard(project_root, &failure.shard, policy)?;
                if child_shards.is_empty()
                    || (child_shards.len() == 1 && child_shards[0].target == failure.shard.target)
                {
                    failure.bail(entry, lang)?;
                }

                tracing::warn!(
                    shard_number = failure.shard_number,
                    target = %failure.shard.target.display(),
                    child_shards = child_shards.len(),
                    status = failure.status_text(),
                    elapsed_ms = failure.elapsed.as_millis(),
                    "scip-python shard hit a heap limit; retrying with smaller shards"
                );

                for child_shard in child_shards.into_iter().rev() {
                    queue.push_front(child_shard);
                }
            }
            PythonShardRun::Failed(failure) => {
                stats.record_failure(failure.elapsed);
                failure.bail(entry, lang)?;
            }
        }
    }

    tracing::info!(
        attempts = stats.attempts,
        succeeded = stats.succeeded,
        failed = stats.failed,
        heap_limit_retries = stats.oom_retries,
        wall_elapsed_ms = started.elapsed().as_millis(),
        total_shard_elapsed_ms = stats.total_shard_elapsed.as_millis(),
        max_shard_elapsed_ms = stats.max_shard_elapsed.as_millis(),
        "finished scip-python shard runs"
    );

    shard_outputs.sort_by_key(|output| output.shard_number);
    let shard_output_paths = shard_outputs
        .into_iter()
        .map(|output| output.path)
        .collect::<Vec<_>>();
    if shard_output_paths.is_empty() {
        anyhow::bail!("scip-python did not produce any successful shard outputs");
    }
    let merged_output = temp_dir.path().join("python-merged.scip");
    merge_scip_files(&shard_output_paths, &merged_output)?;
    let compaction = compact_scip_file(&merged_output)?;
    if compaction.changed() {
        tracing::info!(
            path = %merged_output.display(),
            duplicate_documents = compaction.duplicate_documents,
            duplicate_occurrences = compaction.duplicate_occurrences,
            duplicate_symbols = compaction.duplicate_symbols,
            "compacted duplicate SCIP facts after sharded merge"
        );
    }
    let validation = validate_scip_file(&merged_output)?;
    if !validation.valid {
        let errors = validation
            .errors
            .iter()
            .map(|error| format!("{}: {}", error.kind, error.message))
            .collect::<Vec<_>>()
            .join("; ");
        anyhow::bail!(
            "{} produced invalid {} SCIP output after sharded merge: {}",
            entry.indexer_name,
            lang.name(),
            errors
        );
    }
    let final_stats = validation.stats.unwrap_or_default();
    let output_bytes = std::fs::metadata(&merged_output)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    publish_scip_file_atomically(&merged_output, &output_file)?;
    tracing::info!(
        indexer = %entry.indexer_name,
        lang = lang.name(),
        output = %output_file.display(),
        output_bytes,
        documents = final_stats.documents,
        symbols = final_stats.symbols,
        occurrences = final_stats.occurrences,
        shards = shard_output_paths.len(),
        heap_limit_retries = stats.oom_retries,
        "finished scip-python sharded output"
    );
    if options.use_persistent_hints {
        store_python_shard_hints(project_root, &shard_hints);
    }

    Ok(output_file)
}

enum PythonShardRun {
    Succeeded(PythonShardSuccess),
    Failed(PythonShardFailure),
}

#[derive(Debug)]
struct PythonShardSuccess {
    shard_number: usize,
    output: PathBuf,
    elapsed: Duration,
}

#[derive(Debug)]
struct PythonShardOutput {
    shard_number: usize,
    path: PathBuf,
}

#[derive(Debug, Default)]
struct PythonShardRunStats {
    attempts: usize,
    succeeded: usize,
    failed: usize,
    oom_retries: usize,
    total_shard_elapsed: Duration,
    max_shard_elapsed: Duration,
}

impl PythonShardRunStats {
    fn record_success(&mut self, elapsed: Duration) {
        self.record_attempt(elapsed);
        self.succeeded += 1;
    }

    fn record_failure(&mut self, elapsed: Duration) {
        self.record_attempt(elapsed);
        self.failed += 1;
    }

    fn record_attempt(&mut self, elapsed: Duration) {
        self.attempts += 1;
        self.total_shard_elapsed += elapsed;
        self.max_shard_elapsed = self.max_shard_elapsed.max(elapsed);
    }
}

#[derive(Debug)]
struct PythonShardFailure {
    shard: PythonShard,
    shard_number: usize,
    status: ExitStatus,
    stdout: String,
    stderr: String,
    elapsed: Duration,
}

impl PythonShardFailure {
    fn is_oom(&self) -> bool {
        self.status.code() == Some(134)
            || self
                .stderr
                .to_ascii_lowercase()
                .contains("heap out of memory")
            || self
                .stdout
                .to_ascii_lowercase()
                .contains("heap out of memory")
    }

    fn status_text(&self) -> String {
        self.status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| self.status.to_string())
    }

    fn bail(&self, entry: &IndexerEntry, lang: &Language) -> Result<()> {
        anyhow::bail!(
            "{} exited with status {} for {} shard {} ({})\nstdout:\n{}\nstderr:\n{}",
            entry.indexer_name,
            self.status,
            lang.name(),
            self.shard_number,
            self.shard.target.display(),
            self.stdout.trim(),
            self.stderr.trim()
        )
    }
}

async fn run_python_shard(
    binary: &Path,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    temp_dir: &Path,
    shard_number: usize,
    shard: PythonShard,
) -> Result<PythonShardRun> {
    let shard_output = temp_dir.join(format!("python-shard-{shard_number:04}.scip"));
    let input_root = materialize_python_shard_input(project_root, temp_dir, shard_number, &shard)?;
    let command_target = input_root.as_deref().unwrap_or(&shard.target);
    tracing::debug!(
        shard_number,
        target = %shard.target.display(),
        files = shard.file_count,
        output = %shard_output.display(),
        "running scip-python shard"
    );

    let mut cmd = tokio::process::Command::new(binary);
    cmd.current_dir(project_root);
    cmd.kill_on_drop(true);
    for arg in build_python_shard_args(entry, command_target, &shard_output) {
        cmd.arg(arg);
    }

    let started = Instant::now();
    let output = cmd
        .output()
        .await
        .with_context(|| format!("Failed to execute {}", binary.display()))?;
    let elapsed = started.elapsed();

    if !output.status.success() {
        tracing::warn!(
            shard_number,
            target = %shard.target.display(),
            files = shard.file_count,
            status = %output.status,
            elapsed_ms = elapsed.as_millis(),
            "scip-python shard failed"
        );
        return Ok(PythonShardRun::Failed(PythonShardFailure {
            shard,
            shard_number,
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            elapsed,
        }));
    }

    if !shard_output.exists() {
        anyhow::bail!(
            "Indexer {} did not produce shard output file (expected {})",
            entry.indexer_name,
            shard_output.display()
        );
    }

    let updated_languages = normalize_scip_file_languages(&shard_output, Some(lang.name()))?;
    if updated_languages > 0 {
        tracing::debug!(
            path = %shard_output.display(),
            docs = updated_languages,
            "filled missing SCIP document languages in Python shard"
        );
    }
    let normalization_root = input_root.as_deref().unwrap_or(project_root);
    let updated_paths = relativize_scip_file_document_paths(&shard_output, normalization_root)?;
    if updated_paths > 0 {
        tracing::debug!(
            path = %shard_output.display(),
            docs = updated_paths,
            "relativized SCIP document paths in Python shard"
        );
    }
    if shard.files.is_none() && is_python_source_path(&shard.target) {
        let target_path = normalize_path_component(&shard.target.to_string_lossy());
        let updated_paths = replace_empty_scip_document_paths(&shard_output, &target_path)?;
        if updated_paths > 0 {
            tracing::debug!(
                path = %shard_output.display(),
                target = %target_path,
                docs = updated_paths,
                "repaired empty SCIP document paths in Python file shard"
            );
        }
    }
    if let Some(prefix) = python_shard_document_prefix(project_root, &shard) {
        let updated_paths = prefix_scip_file_document_paths(&shard_output, &prefix)?;
        if updated_paths > 0 {
            tracing::debug!(
                path = %shard_output.display(),
                prefix = %prefix,
                docs = updated_paths,
                "prefixed SCIP document paths in Python shard"
            );
        }
    }
    let compaction = compact_scip_file(&shard_output)?;
    if compaction.changed() {
        tracing::debug!(
            path = %shard_output.display(),
            duplicate_documents = compaction.duplicate_documents,
            duplicate_occurrences = compaction.duplicate_occurrences,
            duplicate_symbols = compaction.duplicate_symbols,
            "compacted duplicate SCIP facts in Python shard"
        );
    }
    let validation = validate_scip_file(&shard_output)?;
    if !validation.valid && !only_empty_index_errors(&validation.errors) {
        let errors = validation
            .errors
            .iter()
            .map(|error| format!("{}: {}", error.kind, error.message))
            .collect::<Vec<_>>()
            .join("; ");
        anyhow::bail!(
            "{} produced invalid {} SCIP output in shard {}: {}",
            entry.indexer_name,
            lang.name(),
            shard_number,
            errors
        );
    } else if !validation.valid {
        tracing::debug!(
            path = %shard_output.display(),
            shard_number,
            "scip-python shard produced an empty index; final merged output remains strictly validated"
        );
    }

    let output_bytes = std::fs::metadata(&shard_output)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    tracing::info!(
        shard_number,
        target = %shard.target.display(),
        files = shard.file_count,
        output = %shard_output.display(),
        output_bytes,
        elapsed_ms = elapsed.as_millis(),
        "finished scip-python shard"
    );

    Ok(PythonShardRun::Succeeded(PythonShardSuccess {
        shard_number,
        output: shard_output,
        elapsed,
    }))
}

fn build_python_shard_args(
    entry: &IndexerEntry,
    target: &Path,
    output_file: &Path,
) -> Vec<OsString> {
    let mut args = Vec::with_capacity(entry.default_args.len() + 4);
    args.extend(entry.default_args.iter().map(OsString::from));
    args.push(OsString::from("--target-only"));
    args.push(target.as_os_str().to_os_string());
    args.push(OsString::from("--output"));
    args.push(output_file.as_os_str().to_os_string());
    args
}

fn materialize_python_shard_input(
    project_root: &Path,
    temp_dir: &Path,
    shard_number: usize,
    shard: &PythonShard,
) -> Result<Option<PathBuf>> {
    let Some(files) = &shard.files else {
        return Ok(None);
    };

    let input_root = temp_dir.join(format!("python-shard-{shard_number:04}-input"));
    std::fs::create_dir_all(&input_root)
        .with_context(|| format!("Failed to create {}", input_root.display()))?;

    for file in files {
        let relative_file = file.strip_prefix(&shard.target).unwrap_or(file.as_path());
        let source = project_root.join(file);
        let destination = input_root.join(relative_file);
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        if let Err(error) = std::fs::hard_link(&source, &destination) {
            tracing::debug!(
                source = %source.display(),
                destination = %destination.display(),
                error = %error,
                "falling back to copying Python shard input"
            );
            std::fs::copy(&source, &destination).with_context(|| {
                format!(
                    "Failed to copy Python shard input {} to {}",
                    source.display(),
                    destination.display()
                )
            })?;
        }
    }

    Ok(Some(input_root))
}

fn count_python_files(project_root: &Path) -> Result<usize> {
    Ok(
        split_python_target(project_root, Path::new(""), usize::MAX)?
            .into_iter()
            .map(|shard| shard.file_count)
            .sum(),
    )
}

fn apply_python_shard_hints(
    project_root: &Path,
    shards: Vec<PythonShard>,
    hints: &HashSet<String>,
    policy: PythonShardPolicy,
) -> Result<Vec<PythonShard>> {
    let policy = policy.normalized();
    let mut planned = Vec::new();

    for shard in shards {
        if !hints.contains(&python_shard_target_key(&shard.target)) {
            planned.push(shard);
            continue;
        }

        let child_shards = split_python_target_for_retry(project_root, &shard, policy)?;
        if child_shards.is_empty()
            || (child_shards.len() == 1 && child_shards[0].target == shard.target)
        {
            planned.push(shard);
        } else {
            planned.extend(child_shards);
        }
    }

    sort_python_shards(&mut planned);
    Ok(planned)
}

fn split_python_shard(
    project_root: &Path,
    shard: &PythonShard,
    policy: PythonShardPolicy,
) -> Result<Vec<PythonShard>> {
    if let Some(files) = &shard.files {
        if files.len() <= 1 {
            return Ok(vec![shard.clone()]);
        }
        let next_limit = files.len().div_ceil(2).max(1);
        let mut child_shards = pack_python_loose_files(&shard.target, files.clone(), next_limit);
        sort_python_shards(&mut child_shards);
        return Ok(child_shards);
    }

    split_python_target_for_retry(project_root, shard, policy)
}

fn plan_python_shards(project_root: &Path, policy: PythonShardPolicy) -> Result<Vec<PythonShard>> {
    let policy = policy.normalized();
    let python_file_count = count_python_files(project_root)?;
    if python_file_count == 0 {
        return Ok(Vec::new());
    }
    if python_file_count <= policy.trigger_file_limit {
        return Ok(vec![PythonShard {
            target: PathBuf::new(),
            file_count: python_file_count,
            files: None,
        }]);
    }
    if python_file_count <= policy.target_file_limit {
        return Ok(vec![PythonShard {
            target: PathBuf::from("."),
            file_count: python_file_count,
            files: None,
        }]);
    }
    split_python_target(project_root, Path::new(""), policy.target_file_limit)
}

fn split_python_target(
    project_root: &Path,
    target: &Path,
    max_files_per_shard: usize,
) -> Result<Vec<PythonShard>> {
    let absolute_target = project_root.join(target);
    if absolute_target.is_file() {
        return Ok(if is_python_source_path(target) {
            vec![PythonShard {
                target: target.to_path_buf(),
                file_count: 1,
                files: None,
            }]
        } else {
            Vec::new()
        });
    }

    let mut children = Vec::new();
    let mut loose_files = Vec::new();
    for entry in std::fs::read_dir(&absolute_target)
        .with_context(|| format!("Failed to read {}", absolute_target.display()))?
    {
        let entry = entry
            .with_context(|| format!("Failed to read entry in {}", absolute_target.display()))?;
        let file_name = entry.file_name();
        let child_target = join_python_target(target, &file_name);
        let file_type = entry.file_type().with_context(|| {
            format!(
                "Failed to read file type for {}",
                project_root.join(&child_target).display()
            )
        })?;

        if file_type.is_dir() {
            if is_ignored_python_dir(&file_name.to_string_lossy()) {
                continue;
            }
            let child_count = count_python_files_under(project_root, &child_target)?;
            if child_count == 0 {
                continue;
            }
            if child_count <= max_files_per_shard {
                children.push(PythonShard {
                    target: child_target,
                    file_count: child_count,
                    files: None,
                });
            } else {
                children.extend(split_python_target(
                    project_root,
                    &child_target,
                    max_files_per_shard,
                )?);
            }
        } else if file_type.is_file() && is_python_source_path(&child_target) {
            loose_files.push(child_target);
        }
    }

    loose_files.sort();
    children.extend(pack_python_loose_files(
        target,
        loose_files,
        max_files_per_shard,
    ));
    sort_python_shards(&mut children);
    Ok(children)
}

fn pack_python_loose_files(
    parent: &Path,
    files: Vec<PathBuf>,
    max_files_per_shard: usize,
) -> Vec<PythonShard> {
    let max_files_per_shard = max_files_per_shard.max(1);
    files
        .chunks(max_files_per_shard)
        .map(|chunk| {
            if chunk.len() == 1 {
                PythonShard {
                    target: chunk[0].clone(),
                    file_count: 1,
                    files: None,
                }
            } else {
                PythonShard {
                    target: parent.to_path_buf(),
                    file_count: chunk.len(),
                    files: Some(chunk.to_vec()),
                }
            }
        })
        .collect()
}

fn split_python_target_for_retry(
    project_root: &Path,
    shard: &PythonShard,
    policy: PythonShardPolicy,
) -> Result<Vec<PythonShard>> {
    let child_shards = split_python_target(project_root, &shard.target, policy.target_file_limit)?;
    if child_shards.is_empty()
        || (child_shards.len() == 1 && child_shards[0].target == shard.target)
    {
        let retry_limit = shard
            .file_count
            .div_ceil(2)
            .max(1)
            .min(policy.target_file_limit);
        split_python_target(project_root, &shard.target, retry_limit)
    } else {
        Ok(child_shards)
    }
}

fn sort_python_shards(shards: &mut [PythonShard]) {
    shards.sort_by(|a, b| a.target.cmp(&b.target).then_with(|| a.files.cmp(&b.files)));
}

fn count_python_files_under(project_root: &Path, target: &Path) -> Result<usize> {
    let absolute_target = project_root.join(target);
    if absolute_target.is_file() {
        return Ok(usize::from(is_python_source_path(target)));
    }

    let mut count = 0usize;
    for entry in std::fs::read_dir(&absolute_target)
        .with_context(|| format!("Failed to read {}", absolute_target.display()))?
    {
        let entry = entry
            .with_context(|| format!("Failed to read entry in {}", absolute_target.display()))?;
        let file_name = entry.file_name();
        let child_target = join_python_target(target, &file_name);
        let file_type = entry.file_type().with_context(|| {
            format!(
                "Failed to read file type for {}",
                project_root.join(&child_target).display()
            )
        })?;
        if file_type.is_dir() {
            if !is_ignored_python_dir(&file_name.to_string_lossy()) {
                count += count_python_files_under(project_root, &child_target)?;
            }
        } else if file_type.is_file() && is_python_source_path(&child_target) {
            count += 1;
        }
    }

    Ok(count)
}

fn python_shard_document_prefix(project_root: &Path, shard: &PythonShard) -> Option<String> {
    if shard.target.as_os_str().is_empty() || shard.target == Path::new(".") {
        return None;
    }

    let prefix = if is_python_source_path(&shard.target) {
        shard.target.parent()
    } else {
        Some(shard.target.as_path())
    }?;
    let prefix = normalize_path_component(&prefix.to_string_lossy());
    if prefix.is_empty() || !project_root.join(prefix.as_str()).exists() {
        None
    } else {
        Some(prefix)
    }
}

fn join_python_target(target: &Path, file_name: &std::ffi::OsStr) -> PathBuf {
    if target.as_os_str().is_empty() || target == Path::new(".") {
        PathBuf::from(file_name)
    } else {
        target.join(file_name)
    }
}

fn is_python_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "py" | "pyi" | "pyw"
            )
        })
        .unwrap_or(false)
}

fn is_ignored_python_dir(file_name: &str) -> bool {
    IGNORED_PYTHON_DIRS
        .iter()
        .any(|ignored| file_name.eq_ignore_ascii_case(ignored))
}

fn can_run_python_shard_in_parallel(shard: &PythonShard) -> bool {
    shard.file_count <= PYTHON_PARALLEL_SHARD_FILE_LIMIT
}

fn python_shard_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(|threads| threads.get())
        .unwrap_or(1)
        .clamp(1, PYTHON_MAX_PARALLEL_SHARDS)
}

fn python_shard_target_key(target: &Path) -> String {
    if target.as_os_str().is_empty() || target == Path::new(".") {
        ".".to_string()
    } else {
        normalize_path_component(&target.to_string_lossy())
    }
}

fn python_shard_hints_path(project_root: &Path) -> Option<PathBuf> {
    let root = std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let normalized = normalize_path_component(&root.to_string_lossy()).to_ascii_lowercase();
    let hash = Sha256::digest(normalized.as_bytes());
    Some(
        dirs::cache_dir()?
            .join("scip-io")
            .join("python-shard-hints")
            .join(format!("{}.json", hex::encode(hash))),
    )
}

fn load_python_shard_hints(project_root: &Path) -> HashSet<String> {
    let Some(path) = python_shard_hints_path(project_root) else {
        return HashSet::new();
    };

    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return HashSet::new(),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to read scip-python shard hints"
            );
            return HashSet::new();
        }
    };

    match serde_json::from_str::<Vec<String>>(&raw) {
        Ok(hints) => hints.into_iter().collect(),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to parse scip-python shard hints"
            );
            HashSet::new()
        }
    }
}

fn store_python_shard_hints(project_root: &Path, hints: &HashSet<String>) {
    if hints.is_empty() {
        return;
    }
    let Some(path) = python_shard_hints_path(project_root) else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };

    if let Err(error) = std::fs::create_dir_all(parent) {
        tracing::warn!(
            path = %parent.display(),
            error = %error,
            "failed to create scip-python shard hint directory"
        );
        return;
    }

    let mut hints = hints.iter().cloned().collect::<Vec<_>>();
    hints.sort();
    match serde_json::to_vec_pretty(&hints) {
        Ok(bytes) => {
            if let Err(error) = std::fs::write(&path, bytes) {
                tracing::warn!(
                    path = %path.display(),
                    error = %error,
                    "failed to write scip-python shard hints"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                error = %error,
                "failed to serialize scip-python shard hints"
            );
        }
    }
}

/// Build argv after the binary name, placing supported config files before
/// the output option so CLIs with positional project arguments can parse them.
pub fn build_indexer_args(
    entry: &IndexerEntry,
    output_file: &Path,
    config_paths: &[PathBuf],
) -> Vec<OsString> {
    let project_root = output_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    build_indexer_args_with_project_root(
        entry,
        output_file,
        config_paths,
        project_root,
        &entry.default_args,
    )
}

pub fn build_indexer_args_with_defaults_for_display(
    entry: &IndexerEntry,
    output_file: &Path,
    config_paths: &[PathBuf],
    default_args: &[String],
) -> Vec<OsString> {
    let project_root = output_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    build_indexer_args_with_project_root(
        entry,
        output_file,
        config_paths,
        project_root,
        default_args,
    )
}

fn build_indexer_args_for_project_with_defaults(
    entry: &IndexerEntry,
    output_file: &Path,
    config_paths: &[PathBuf],
    project_root: &Path,
    default_args: &[String],
) -> Vec<OsString> {
    build_indexer_args_with_project_root(
        entry,
        output_file,
        config_paths,
        Some(project_root),
        default_args,
    )
}

#[cfg(test)]
fn build_indexer_args_for_project(
    entry: &IndexerEntry,
    output_file: &Path,
    config_paths: &[PathBuf],
    project_root: &Path,
) -> Vec<OsString> {
    build_indexer_args_for_project_with_defaults(
        entry,
        output_file,
        config_paths,
        project_root,
        &entry.default_args,
    )
}

fn build_indexer_args_with_project_root(
    entry: &IndexerEntry,
    output_file: &Path,
    config_paths: &[PathBuf],
    project_root: Option<&Path>,
    default_args: &[String],
) -> Vec<OsString> {
    let mut args = Vec::new();
    let mut has_output_arg = false;

    for arg in default_args {
        if arg == "index.scip" {
            args.push(output_file.as_os_str().to_os_string());
            has_output_arg = true;
        } else {
            if arg == "--output" || arg.contains("index.scip") {
                has_output_arg = true;
            }
            args.push(OsString::from(arg));
        }
    }

    args.extend(
        config_paths
            .iter()
            .map(|path| config_path_arg(path, project_root)),
    );
    append_ruby_gem_metadata_fallback(entry, project_root, &mut args);

    // Config-driven indexers need an explicit destination because the default
    // output file can be overwritten when one invocation spans several configs.
    // Known output-capable indexers also receive an explicit temp destination
    // so failed or cancelled runs cannot publish partial final artifacts.
    if (!config_paths.is_empty() || indexer_supports_explicit_output_arg(entry)) && !has_output_arg
    {
        args.push(OsString::from("--output"));
        args.push(output_file.as_os_str().to_os_string());
    }

    args
}

fn append_ruby_gem_metadata_fallback(
    entry: &IndexerEntry,
    project_root: Option<&Path>,
    args: &mut Vec<OsString>,
) {
    if entry.indexer_name != "scip-ruby" || has_arg(args, "--gem-metadata") {
        return;
    }
    let Some(project_root) = project_root else {
        return;
    };
    if project_has_gemspec(project_root) {
        return;
    }

    // scip-ruby needs gem metadata even for Ruby apps that are not packaged as
    // gems. Use a deterministic local package name so app-only projects index
    // instead of failing before SCIP output is produced.
    let package = project_root
        .file_name()
        .and_then(|name| name.to_str())
        .map(sanitize_ruby_gem_name)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "local".to_string());
    args.push(OsString::from("--gem-metadata"));
    args.push(OsString::from(format!("{package}@0.0.0")));
}

fn has_arg(args: &[OsString], flag: &str) -> bool {
    args.iter().any(|arg| {
        let arg = arg.to_string_lossy();
        arg == flag || arg.starts_with(&format!("{flag}="))
    })
}

fn project_has_gemspec(project_root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(project_root) else {
        return false;
    };
    entries.filter_map(|entry| entry.ok()).any(|entry| {
        entry
            .path()
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gemspec"))
    })
}

fn sanitize_ruby_gem_name(name: &str) -> String {
    let mut sanitized = String::new();
    let mut previous_was_separator = false;
    for character in name.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
            sanitized.push(character);
            previous_was_separator = false;
        } else if !previous_was_separator {
            sanitized.push('-');
            previous_was_separator = true;
        }
    }
    sanitized
        .trim_matches(|character| character == '-' || character == '_')
        .to_string()
}

fn indexer_supports_explicit_output_arg(entry: &IndexerEntry) -> bool {
    matches!(
        entry.indexer_name.as_str(),
        "scip-typescript"
            | "scip-python"
            | "rust-analyzer"
            | "scip-go"
            | "scip-dotnet"
            | "scip-java"
    )
}

fn should_retry_with_project_argument_shards(
    entry: &IndexerEntry,
    config_paths: &[PathBuf],
    error: &anyhow::Error,
) -> bool {
    !planner::plan_project_argument_shards(entry, config_paths).is_empty()
        && (error_looks_like_memory_failure(error)
            || error_looks_like_invocation_size_failure(error))
}

fn should_shard_project_arguments_upfront(entry: &IndexerEntry, config_paths: &[PathBuf]) -> bool {
    config_paths.len() > PROJECT_ARGUMENT_SHARD_CONFIG_LIMIT
        && !planner::plan_project_argument_shards(entry, config_paths).is_empty()
}

fn error_looks_like_memory_failure(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    message.contains("heap out of memory")
        || message.contains("out of memory")
        || message.contains("allocation failed")
        || message.contains("status 134")
        || message.contains("exit code: 134")
}

fn error_looks_like_invocation_size_failure(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    message.contains("filename or extension is too long")
        || message.contains("command line is too long")
        || message.contains("argument list too long")
}

fn only_empty_index_errors(errors: &[crate::validate::ValidationError]) -> bool {
    !errors.is_empty()
        && errors
            .iter()
            .all(|error| error.kind.as_str() == "empty_index")
}

fn build_compile_command_shard_args(
    entry: &IndexerEntry,
    compile_commands: &Path,
) -> Vec<OsString> {
    let mut args = Vec::with_capacity(entry.default_args.len() + 1);
    let mut replaced = false;
    for arg in &entry.default_args {
        if arg.starts_with("--compdb-path=") {
            args.push(OsString::from(format!(
                "--compdb-path={}",
                compile_commands.display()
            )));
            replaced = true;
        } else if arg == "compile_commands.json" {
            args.push(compile_commands.as_os_str().to_os_string());
            replaced = true;
        } else {
            args.push(OsString::from(arg));
        }
    }

    if !replaced {
        args.push(OsString::from(format!(
            "--compdb-path={}",
            compile_commands.display()
        )));
    }
    args
}

fn config_path_arg(path: &Path, project_root: Option<&Path>) -> OsString {
    project_root
        .and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path)
        .as_os_str()
        .to_os_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LanguageKind;
    use crate::indexer::backend::BackendCapabilities;
    use crate::indexer::{IndexerEntry, InstallMethod};
    use protobuf::Message;
    use scip::types::{Document, Index};
    use tempfile::TempDir;

    fn entry(default_args: &[&str]) -> IndexerEntry {
        IndexerEntry {
            indexer_name: "test-indexer".into(),
            language: "typescript".into(),
            github_repo: "owner/repo".into(),
            binary_name: "test-indexer".into(),
            version: "1.0.0".into(),
            default_args: default_args.iter().map(|arg| arg.to_string()).collect(),
            output_file: "index.scip".into(),
            install_method: InstallMethod::Unsupported {
                reason: "test".into(),
            },
            backend_capabilities: BackendCapabilities::native(),
        }
    }

    fn named_entry(indexer_name: &str, language: &str, default_args: &[&str]) -> IndexerEntry {
        IndexerEntry {
            indexer_name: indexer_name.into(),
            language: language.into(),
            github_repo: "owner/repo".into(),
            binary_name: indexer_name.into(),
            version: "1.0.0".into(),
            default_args: default_args.iter().map(|arg| arg.to_string()).collect(),
            output_file: "index.scip".into(),
            install_method: InstallMethod::Unsupported {
                reason: "test".into(),
            },
            backend_capabilities: BackendCapabilities::native(),
        }
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn build_indexer_args_places_config_paths_before_output_flag() {
        let args = build_indexer_args(
            &entry(&["index"]),
            Path::new("typescript.scip"),
            &[
                PathBuf::from("tsconfig.json"),
                PathBuf::from("tsconfig.test.json"),
            ],
        );

        assert_eq!(
            strings(args),
            vec![
                "index",
                "tsconfig.json",
                "tsconfig.test.json",
                "--output",
                "typescript.scip",
            ]
        );
    }

    #[test]
    fn build_indexer_args_uses_relative_config_paths_under_output_root() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/project");
        let args = build_indexer_args(
            &entry(&["index"]),
            &root.join("typescript.scip"),
            &[
                root.join("tsconfig.json"),
                root.join("tsconfig.scripts.json"),
            ],
        );

        assert_eq!(
            strings(args),
            vec![
                "index",
                "tsconfig.json",
                "tsconfig.scripts.json",
                "--output",
                root.join("typescript.scip").to_string_lossy().as_ref(),
            ]
        );
    }

    #[test]
    fn build_indexer_args_replaces_default_output_placeholder() {
        let args = build_indexer_args(
            &entry(&["index", "--output", "index.scip"]),
            Path::new("go.scip"),
            &[],
        );

        assert_eq!(strings(args), vec!["index", "--output", "go.scip"]);
    }

    #[test]
    fn build_indexer_args_preserves_default_args_without_configs() {
        let args = build_indexer_args(&entry(&["index"]), Path::new("typescript.scip"), &[]);

        assert_eq!(strings(args), vec!["index"]);
    }

    #[test]
    fn build_indexer_args_adds_temp_output_for_known_output_capable_indexers() {
        let args = build_indexer_args(
            &named_entry("scip-typescript", "typescript", &["index"]),
            Path::new("typescript.scip"),
            &[],
        );

        assert_eq!(strings(args), vec!["index", "--output", "typescript.scip"]);
    }

    #[test]
    fn build_indexer_args_applies_configured_defaults() {
        let defaults = vec![
            "index".to_string(),
            "--output".to_string(),
            "index.scip".to_string(),
            "--".to_string(),
            "-pl".to_string(),
            "core".to_string(),
            "-am".to_string(),
        ];
        let args = build_indexer_args_with_defaults_for_display(
            &named_entry("scip-java", "scala", &["index"]),
            Path::new("scala.scip"),
            &[],
            &defaults,
        );

        assert_eq!(
            strings(args),
            vec![
                "index",
                "--output",
                "scala.scip",
                "--",
                "-pl",
                "core",
                "-am"
            ]
        );
    }

    #[test]
    fn build_indexer_args_uses_scip_go_flags_without_subcommand() {
        let args = build_indexer_args(
            &named_entry("scip-go", "go", &["--output", "index.scip"]),
            Path::new("go.scip"),
            &[],
        );

        assert_eq!(strings(args), vec!["--output", "go.scip"]);
    }

    #[test]
    fn build_indexer_args_uses_scip_ruby_index_file_flag() {
        let args = build_indexer_args(
            &named_entry("scip-ruby", "ruby", &["--index-file", "index.scip", "."]),
            Path::new("ruby.scip"),
            &[],
        );

        assert_eq!(strings(args), vec!["--index-file", "ruby.scip", "."]);
    }

    #[test]
    fn build_indexer_args_adds_ruby_metadata_when_no_gemspec_exists() {
        let dir = TempDir::new().unwrap();
        let output = dir.path().join("ruby.scip");

        let args = build_indexer_args_for_project(
            &named_entry("scip-ruby", "ruby", &["--index-file", "index.scip", "."]),
            &output,
            &[],
            dir.path(),
        );

        let args = strings(args);
        assert!(args.contains(&"--gem-metadata".to_string()));
        assert!(args.iter().any(|arg| arg.ends_with("@0.0.0")));
    }

    #[test]
    fn build_indexer_args_does_not_add_ruby_metadata_when_gemspec_exists() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("smoke.gemspec"), "").unwrap();
        let output = dir.path().join("ruby.scip");

        let args = build_indexer_args_for_project(
            &named_entry("scip-ruby", "ruby", &["--index-file", "index.scip", "."]),
            &output,
            &[],
            dir.path(),
        );

        assert!(!strings(args).contains(&"--gem-metadata".to_string()));
    }

    #[cfg(windows)]
    #[test]
    fn detects_windows_cross_drive_rename_error() {
        let error = std::io::Error::from_raw_os_error(17);

        assert!(is_cross_device_rename_error(&error));
    }

    #[test]
    fn large_project_argument_lists_shard_before_single_invocation() {
        let configs = (0..=PROJECT_ARGUMENT_SHARD_CONFIG_LIMIT)
            .map(|index| PathBuf::from(format!("Project{index}.csproj")))
            .collect::<Vec<_>>();

        assert!(should_shard_project_arguments_upfront(
            &named_entry("scip-dotnet", "csharp", &["index"]),
            &configs
        ));
        assert!(!should_shard_project_arguments_upfront(
            &named_entry("scip-ruby", "ruby", &["index"]),
            &configs
        ));
    }

    #[tokio::test]
    async fn generic_runner_publishes_compacted_temp_output_only_after_success() -> Result<()> {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        let old_hex = scip_fixture_hex("old.ts")?;
        std::fs::write(dir.path().join("typescript.scip"), scip_bytes(&old_hex)?)?;

        let duplicate_hex = scip_fixture_hex_for_paths(&["src/app.ts", "src/app.ts"])?;
        let fake_indexer = dir.path().join("fake-scip-typescript.js");
        std::fs::write(
            &fake_indexer,
            format!(
                r#"
const fs = require("fs");
const args = process.argv.slice(2);
const outputIndex = args.indexOf("--output");
if (outputIndex === -1 || !args[outputIndex + 1]) {{
  process.stderr.write("missing output\n");
  process.exit(2);
}}
fs.writeFileSync(args[outputIndex + 1], Buffer.from("{duplicate_hex}", "hex"));
"#
            ),
        )?;

        let lang = LanguageKind::TypeScript.with_evidence("test".into());
        let fake_arg = fake_indexer.to_string_lossy().to_string();
        let output = run_indexer_with_configs(
            &node,
            &named_entry(
                "scip-typescript",
                "typescript",
                &[fake_arg.as_str(), "index"],
            ),
            dir.path(),
            &lang,
            &[],
        )
        .await?;

        assert_eq!(output, dir.path().join("typescript.scip"));
        let index = Index::parse_from_bytes(&std::fs::read(output)?)?;
        let paths = index
            .documents
            .iter()
            .map(|doc| (doc.relative_path.as_str(), doc.language.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(paths, vec![("src/app.ts", "typescript")]);
        assert!(!dir.path().join("index.scip").exists());

        Ok(())
    }

    #[tokio::test]
    async fn native_runner_injects_required_toolchain_environment() -> Result<()> {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        let go_home = dir.path().join("Go");
        let go_bin = go_home.join("bin");
        std::fs::create_dir_all(&go_bin)?;
        let go_binary = go_bin.join(if cfg!(windows) { "go.exe" } else { "go" });
        std::fs::write(&go_binary, "")?;

        let expected_bin_json = serde_json::to_string(&go_bin.to_string_lossy().to_string())?;
        let scip_hex = scip_fixture_hex("main.go")?;
        let fake_indexer = dir.path().join("fake-scip-go.js");
        std::fs::write(
            &fake_indexer,
            format!(
                r#"
const fs = require("fs");
const path = require("path");
const expected = {expected_bin_json};
const firstPath = (process.env.PATH || "").split(path.delimiter)[0] || "";
if (path.resolve(firstPath).toLowerCase() !== path.resolve(expected).toLowerCase()) {{
  process.stderr.write("toolchain bin was not prepended to PATH: " + firstPath + "\n");
  process.exit(2);
}}
const args = process.argv.slice(2);
const outputIndex = args.indexOf("--output");
if (outputIndex === -1 || !args[outputIndex + 1]) {{
  process.stderr.write("missing output\n");
  process.exit(3);
}}
fs.writeFileSync(args[outputIndex + 1], Buffer.from("{scip_hex}", "hex"));
"#
            ),
        )?;

        let toolchains = ToolchainsConfig {
            go: Some(crate::toolchain::ToolchainHomeConfig {
                home: Some(go_home),
            }),
            java: None,
        };
        let lang = LanguageKind::Go.with_evidence("go.mod".into());
        let fake_arg = fake_indexer.to_string_lossy().to_string();

        let output = run_indexer_with_configs_backend_and_toolchains(
            Some(&node),
            &named_entry(
                "scip-go",
                "go",
                &[fake_arg.as_str(), "index", "--output", "index.scip"],
            ),
            dir.path(),
            &lang,
            &[],
            BackendPreference::native(),
            &toolchains,
        )
        .await?;

        assert_eq!(output, dir.path().join("go.scip"));
        Ok(())
    }

    #[tokio::test]
    async fn generic_runner_keeps_existing_final_output_and_reports_stderr_on_failure() -> Result<()>
    {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        let old_hex = scip_fixture_hex("old.ts")?;
        let old_bytes = scip_bytes(&old_hex)?;
        std::fs::write(dir.path().join("typescript.scip"), &old_bytes)?;

        let fake_indexer = dir.path().join("fake-failing-scip-typescript.js");
        std::fs::write(
            &fake_indexer,
            r#"
process.stdout.write("stdout breadcrumb\n");
process.stderr.write("stderr breadcrumb\n");
process.exit(7);
"#,
        )?;

        let lang = LanguageKind::TypeScript.with_evidence("test".into());
        let fake_arg = fake_indexer.to_string_lossy().to_string();
        let err = run_indexer_with_configs(
            &node,
            &named_entry(
                "scip-typescript",
                "typescript",
                &[fake_arg.as_str(), "index"],
            ),
            dir.path(),
            &lang,
            &[],
        )
        .await
        .expect_err("failing fake indexer should fail");

        let message = format!("{err:#}");
        assert!(message.contains("stdout breadcrumb"));
        assert!(message.contains("stderr breadcrumb"));
        assert_eq!(
            std::fs::read(dir.path().join("typescript.scip"))?,
            old_bytes
        );

        Ok(())
    }

    #[tokio::test]
    async fn generic_runner_does_not_accept_default_output_when_explicit_output_was_requested()
    -> Result<()> {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        let default_hex = scip_fixture_hex("old-index.ts")?;
        let default_bytes = scip_bytes(&default_hex)?;
        std::fs::write(dir.path().join("index.scip"), &default_bytes)?;

        let wrong_hex = scip_fixture_hex("wrong.ts")?;
        let fake_indexer = dir.path().join("fake-wrong-output-scip-typescript.js");
        std::fs::write(
            &fake_indexer,
            format!(
                r#"
const fs = require("fs");
fs.writeFileSync("index.scip", Buffer.from("{wrong_hex}", "hex"));
"#
            ),
        )?;

        let lang = LanguageKind::TypeScript.with_evidence("test".into());
        let fake_arg = fake_indexer.to_string_lossy().to_string();
        let err = run_indexer_with_configs(
            &node,
            &named_entry(
                "scip-typescript",
                "typescript",
                &[fake_arg.as_str(), "index"],
            ),
            dir.path(),
            &lang,
            &[],
        )
        .await
        .expect_err("explicit-output indexer should fail when --output is ignored");

        assert!(format!("{err:#}").contains("did not produce output file"));
        assert_eq!(std::fs::read(dir.path().join("index.scip"))?, default_bytes);
        assert!(!dir.path().join("typescript.scip").exists());

        Ok(())
    }

    #[tokio::test]
    async fn sharded_python_runner_keeps_existing_final_output_on_terminal_failure() -> Result<()> {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        touch(dir.path(), "pkg/a.py")?;
        touch(dir.path(), "pkg/b.py")?;
        let stale_hex = scip_fixture_hex("stale.py")?;
        let stale_bytes = scip_bytes(&stale_hex)?;
        std::fs::write(dir.path().join("python.scip"), &stale_bytes)?;

        let fake_indexer = dir.path().join("fake-terminal-oom-scip-python.js");
        std::fs::write(
            &fake_indexer,
            r#"
process.stderr.write("FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory\n");
process.exit(134);
"#,
        )?;

        let lang = Language {
            kind: LanguageKind::Python,
            evidence: "test".into(),
            evidence_kind: "project_config".into(),
            indexer_ready: true,
            readiness_message: None,
            additional_configs: Vec::new(),
        };
        let err = run_python_indexer_with_file_limit(
            &node,
            &python_entry(vec![fake_indexer.to_string_lossy().to_string()]),
            dir.path(),
            &lang,
            1,
        )
        .await
        .expect_err("terminal Python shard failure should fail");

        assert!(format!("{err:#}").contains("heap out of memory"));
        assert_eq!(std::fs::read(dir.path().join("python.scip"))?, stale_bytes);

        Ok(())
    }

    #[tokio::test]
    async fn sharded_python_runner_allows_empty_intermediate_shards() -> Result<()> {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        touch(dir.path(), "empty/a.py")?;
        touch(dir.path(), "full/b.py")?;

        let empty_hex = hex::encode(Index::new().write_to_bytes()?);
        let full_hex = scip_fixture_hex("b.py")?;
        let fake_indexer = dir.path().join("fake-empty-shard-scip-python.js");
        std::fs::write(
            &fake_indexer,
            format!(
                r#"
const fs = require("fs");
const args = process.argv.slice(2);
const target = args[args.indexOf("--target-only") + 1].replace(/\\/g, "/");
const output = args[args.indexOf("--output") + 1];
const fixtures = {{
  "empty": "{empty_hex}",
  "full": "{full_hex}",
}};
if (!Object.prototype.hasOwnProperty.call(fixtures, target)) {{
  process.stderr.write("unexpected target " + target + "\n");
  process.exit(2);
}}
fs.writeFileSync(output, Buffer.from(fixtures[target], "hex"));
"#
            ),
        )?;

        let lang = Language {
            kind: LanguageKind::Python,
            evidence: "test".into(),
            evidence_kind: "project_config".into(),
            indexer_ready: true,
            readiness_message: None,
            additional_configs: Vec::new(),
        };
        let output = run_python_indexer_with_file_limit(
            &node,
            &python_entry(vec![fake_indexer.to_string_lossy().to_string()]),
            dir.path(),
            &lang,
            1,
        )
        .await?;

        let merged = Index::parse_from_bytes(&std::fs::read(output)?)?;
        let paths = merged
            .documents
            .iter()
            .map(|doc| (doc.relative_path.as_str(), doc.language.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(paths, vec![("full/b.py", "python")]);

        Ok(())
    }

    #[tokio::test]
    async fn project_argument_sharding_retries_memory_failure_and_merges_outputs() -> Result<()> {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        touch(dir.path(), "tsconfig.a.json")?;
        touch(dir.path(), "tsconfig.b.json")?;

        let a_hex = scip_fixture_hex("src/a.ts")?;
        let b_hex = scip_fixture_hex("src/b.ts")?;
        let fake_indexer = dir.path().join("fake-sharded-scip-typescript.js");
        std::fs::write(
            &fake_indexer,
            format!(
                r#"
const fs = require("fs");
const path = require("path");
const args = process.argv.slice(2);
const outputIndex = args.indexOf("--output");
if (outputIndex === -1 || !args[outputIndex + 1]) {{
  process.stderr.write("missing output\n");
  process.exit(2);
}}
const configs = args.filter((arg) => arg.endsWith(".json"));
if (configs.length !== 1) {{
  process.stderr.write("FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory\n");
  process.exit(134);
}}
if (path.isAbsolute(configs[0])) {{
  process.stderr.write("config path should stay repo-relative for indexer argv\n");
  process.exit(3);
}}
const fixtures = {{
  "tsconfig.a.json": "{a_hex}",
  "tsconfig.b.json": "{b_hex}",
}};
const key = configs[0].replace(/\\/g, "/");
if (!fixtures[key]) {{
  process.stderr.write("unexpected config " + key + "\n");
  process.exit(4);
}}
fs.writeFileSync(args[outputIndex + 1], Buffer.from(fixtures[key], "hex"));
"#
            ),
        )?;

        let lang = LanguageKind::TypeScript.with_evidence("test".into());
        let fake_arg = fake_indexer.to_string_lossy().to_string();
        let output = run_indexer_with_configs(
            &node,
            &named_entry(
                "scip-typescript",
                "typescript",
                &[fake_arg.as_str(), "index"],
            ),
            dir.path(),
            &lang,
            &[
                dir.path().join("tsconfig.a.json"),
                dir.path().join("tsconfig.b.json"),
            ],
        )
        .await?;

        let index = Index::parse_from_bytes(&std::fs::read(output)?)?;
        let paths = index
            .documents
            .iter()
            .map(|doc| (doc.relative_path.as_str(), doc.language.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![("src/a.ts", "typescript"), ("src/b.ts", "typescript")]
        );

        Ok(())
    }

    #[tokio::test]
    async fn compile_command_sharding_chunks_and_merges_outputs() -> Result<()> {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        let compile_commands = dir.path().join("compile_commands.json");
        std::fs::write(
            &compile_commands,
            r#"[{"file":"a.cc"},{"file":"b.cc"},{"file":"c.cc"}]"#,
        )?;

        let ab_hex = scip_fixture_hex_for_paths(&["a.cc", "b.cc"])?;
        let c_hex = scip_fixture_hex("c.cc")?;
        let fake_indexer = dir.path().join("fake-scip-clang.js");
        std::fs::write(
            &fake_indexer,
            format!(
                r#"
const fs = require("fs");
const args = process.argv.slice(2);
const compdbArg = args.find((arg) => arg.startsWith("--compdb-path="));
if (!compdbArg) {{
  process.stderr.write("missing compdb\n");
  process.exit(2);
}}
const commands = JSON.parse(fs.readFileSync(compdbArg.slice("--compdb-path=".length), "utf8"));
const key = commands.map((command) => command.file).join(",");
const fixtures = {{
  "a.cc,b.cc": "{ab_hex}",
  "c.cc": "{c_hex}",
}};
if (!fixtures[key]) {{
  process.stderr.write("unexpected compile chunk " + key + "\n");
  process.exit(3);
}}
fs.writeFileSync("index.scip", Buffer.from(fixtures[key], "hex"));
"#
            ),
        )?;

        let lang = LanguageKind::Cpp.with_evidence("test".into());
        let fake_arg = fake_indexer.to_string_lossy().to_string();
        let planned = planner::plan_compile_command_shards(&compile_commands, 2)?;
        let backend_preference = BackendPreference::native();
        let toolchains = ToolchainsConfig::default();
        let output = run_compile_command_sharded_indexer_with_plan(
            Some(&node),
            &named_entry(
                "scip-clang",
                "cpp",
                &[fake_arg.as_str(), "--compdb-path=compile_commands.json"],
            ),
            dir.path(),
            &lang,
            planned,
            &backend_preference,
            &toolchains,
        )
        .await?;

        let index = Index::parse_from_bytes(&std::fs::read(output)?)?;
        let paths = index
            .documents
            .iter()
            .map(|doc| (doc.relative_path.as_str(), doc.language.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![("a.cc", "cpp"), ("b.cc", "cpp"), ("c.cc", "cpp")]
        );
        assert!(!dir.path().join("index.scip").exists());

        Ok(())
    }

    fn scip_bytes(hex: &str) -> Result<Vec<u8>> {
        Ok(hex::decode(hex)?)
    }

    fn python_entry(default_args: Vec<String>) -> IndexerEntry {
        IndexerEntry {
            indexer_name: "scip-python".into(),
            language: "python".into(),
            github_repo: "sourcegraph/scip-python".into(),
            binary_name: "scip-python".into(),
            version: "0.0.0-test".into(),
            default_args,
            output_file: "index.scip".into(),
            install_method: InstallMethod::Unsupported {
                reason: "test".into(),
            },
            backend_capabilities: BackendCapabilities::native(),
        }
    }

    fn touch(root: &Path, relative_path: &str) -> Result<()> {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, "")?;
        Ok(())
    }

    #[test]
    fn plans_python_shards_with_directory_targets_and_ignored_dirs() -> Result<()> {
        let dir = TempDir::new()?;

        touch(dir.path(), "loose.py")?;
        touch(dir.path(), "small/a.py")?;
        touch(dir.path(), "small/b.pyi")?;
        touch(dir.path(), "large/a.py")?;
        touch(dir.path(), "large/b.py")?;
        touch(dir.path(), "large/c.pyw")?;
        touch(dir.path(), ".git/ignored.py")?;
        touch(dir.path(), "node_modules/pkg/ignored.py")?;
        touch(dir.path(), ".venv/ignored.py")?;
        touch(dir.path(), "venv/ignored.py")?;
        touch(dir.path(), "__pycache__/ignored.py")?;
        touch(dir.path(), "target/ignored.py")?;
        touch(dir.path(), "dist/ignored.py")?;
        touch(dir.path(), "build/ignored.py")?;

        let mut targets = plan_python_shards(dir.path(), PythonShardPolicy::uniform(2))?
            .into_iter()
            .map(|shard| shard.target.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>();
        targets.sort();

        assert_eq!(targets, vec!["large", "large/c.pyw", "loose.py", "small"]);

        Ok(())
    }

    #[test]
    fn prefixes_python_shard_outputs_by_directory_or_parent_directory() -> Result<()> {
        let dir = TempDir::new()?;
        touch(dir.path(), "tools/a.py")?;
        std::fs::create_dir_all(dir.path().join("package"))?;

        let dir_shard = PythonShard {
            target: PathBuf::from("package"),
            file_count: 1,
            files: None,
        };
        let file_shard = PythonShard {
            target: PathBuf::from("tools/a.py"),
            file_count: 1,
            files: None,
        };
        let root_file_shard = PythonShard {
            target: PathBuf::from("a.py"),
            file_count: 1,
            files: None,
        };

        assert_eq!(
            python_shard_document_prefix(dir.path(), &dir_shard),
            Some("package".into())
        );
        assert_eq!(
            python_shard_document_prefix(dir.path(), &file_shard),
            Some("tools".into())
        );
        assert_eq!(
            python_shard_document_prefix(dir.path(), &root_file_shard),
            None
        );

        Ok(())
    }

    #[tokio::test]
    async fn sharded_python_runner_merges_prefixed_outputs() -> Result<()> {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        touch(dir.path(), "pkg/a.py")?;
        touch(dir.path(), "pkg/b.py")?;
        touch(dir.path(), "tools/c.py")?;

        let mut fixture = Index::new();
        let mut doc = Document::new();
        doc.relative_path = "file.py".into();
        fixture.documents.push(doc);
        let fixture_hex = hex::encode(fixture.write_to_bytes()?);

        let fake_indexer = dir.path().join("fake-scip-python.js");
        std::fs::write(
            &fake_indexer,
            format!(
                r#"
const fs = require("fs");
const args = process.argv.slice(2);
const targetIndex = args.indexOf("--target-only");
const outputIndex = args.indexOf("--output");
if (targetIndex === -1) {{
  process.stderr.write("FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory\n");
  process.exit(134);
}}
if (outputIndex === -1 || !args[outputIndex + 1]) {{
  process.stderr.write("missing output\n");
  process.exit(2);
}}
fs.writeFileSync(args[outputIndex + 1], Buffer.from("{fixture_hex}", "hex"));
"#
            ),
        )?;

        let lang = Language {
            kind: LanguageKind::Python,
            evidence: "test".into(),
            evidence_kind: "project_config".into(),
            indexer_ready: true,
            readiness_message: None,
            additional_configs: Vec::new(),
        };
        let output = run_python_indexer_with_file_limit(
            &node,
            &python_entry(vec![fake_indexer.to_string_lossy().to_string()]),
            dir.path(),
            &lang,
            2,
        )
        .await?;

        let merged = Index::parse_from_bytes(&std::fs::read(output)?)?;
        let paths = merged
            .documents
            .iter()
            .map(|doc| (doc.relative_path.as_str(), doc.language.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![("pkg/file.py", "python"), ("tools/file.py", "python")]
        );

        Ok(())
    }

    fn scip_fixture_hex(relative_path: &str) -> Result<String> {
        let mut fixture = Index::new();
        let mut doc = Document::new();
        doc.relative_path = relative_path.into();
        fixture.documents.push(doc);
        Ok(hex::encode(fixture.write_to_bytes()?))
    }

    fn scip_fixture_hex_for_paths(relative_paths: &[&str]) -> Result<String> {
        let mut fixture = Index::new();
        for relative_path in relative_paths {
            let mut doc = Document::new();
            doc.relative_path = (*relative_path).into();
            fixture.documents.push(doc);
        }
        Ok(hex::encode(fixture.write_to_bytes()?))
    }

    #[tokio::test]
    async fn sharded_python_runner_recursively_splits_oom_directory_shards() -> Result<()> {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        touch(dir.path(), "pkg/a.py")?;
        touch(dir.path(), "pkg/b.py")?;
        touch(dir.path(), "tools/c.py")?;

        let pkg_a_hex = scip_fixture_hex("a.py")?;
        let pkg_b_hex = scip_fixture_hex("b.py")?;
        let tools_hex = scip_fixture_hex("file.py")?;

        let fake_indexer = dir.path().join("fake-scip-python-oom.js");
        std::fs::write(
            &fake_indexer,
            format!(
                r#"
const fs = require("fs");
const args = process.argv.slice(2);
const targetIndex = args.indexOf("--target-only");
const outputIndex = args.indexOf("--output");
if (targetIndex === -1 || outputIndex === -1 || !args[outputIndex + 1]) {{
  process.exit(2);
}}
const target = args[targetIndex + 1].replace(/\\/g, "/");
if (target === "pkg") {{
  process.stderr.write("FATAL ERROR: Reached heap limit Allocation failed - JavaScript heap out of memory\n");
  process.exit(134);
}}
const fixtures = {{
  "pkg/a.py": "{pkg_a_hex}",
  "pkg/b.py": "{pkg_b_hex}",
  "tools": "{tools_hex}",
}};
if (!fixtures[target]) {{
  process.stderr.write("unexpected target " + target + "\n");
  process.exit(3);
}}
fs.writeFileSync(args[outputIndex + 1], Buffer.from(fixtures[target], "hex"));
"#
            ),
        )?;

        let lang = Language {
            kind: LanguageKind::Python,
            evidence: "test".into(),
            evidence_kind: "project_config".into(),
            indexer_ready: true,
            readiness_message: None,
            additional_configs: Vec::new(),
        };
        let output = run_python_indexer_with_file_limit(
            &node,
            &python_entry(vec![fake_indexer.to_string_lossy().to_string()]),
            dir.path(),
            &lang,
            2,
        )
        .await?;

        let merged = Index::parse_from_bytes(&std::fs::read(output)?)?;
        let paths = merged
            .documents
            .iter()
            .map(|doc| (doc.relative_path.as_str(), doc.language.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![
                ("pkg/a.py", "python"),
                ("pkg/b.py", "python"),
                ("tools/file.py", "python")
            ]
        );

        Ok(())
    }

    #[tokio::test]
    async fn sharded_python_runner_maps_empty_file_target_documents_to_file_paths() -> Result<()> {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        touch(dir.path(), "pkg/a.py")?;
        touch(dir.path(), "pkg/b.py")?;

        let empty_doc_hex = scip_fixture_hex("")?;
        let fake_indexer = dir.path().join("fake-scip-python-empty-file-doc.js");
        std::fs::write(
            &fake_indexer,
            format!(
                r#"
const fs = require("fs");
const args = process.argv.slice(2);
const targetIndex = args.indexOf("--target-only");
const outputIndex = args.indexOf("--output");
if (targetIndex === -1 || outputIndex === -1 || !args[outputIndex + 1]) {{
  process.exit(2);
}}
fs.writeFileSync(args[outputIndex + 1], Buffer.from("{empty_doc_hex}", "hex"));
"#
            ),
        )?;

        let lang = Language {
            kind: LanguageKind::Python,
            evidence: "test".into(),
            evidence_kind: "project_config".into(),
            indexer_ready: true,
            readiness_message: None,
            additional_configs: Vec::new(),
        };
        let output = run_python_indexer_with_file_limit(
            &node,
            &python_entry(vec![fake_indexer.to_string_lossy().to_string()]),
            dir.path(),
            &lang,
            1,
        )
        .await?;

        let merged = Index::parse_from_bytes(&std::fs::read(output)?)?;
        let paths = merged
            .documents
            .iter()
            .map(|doc| (doc.relative_path.as_str(), doc.language.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(paths, vec![("pkg/a.py", "python"), ("pkg/b.py", "python")]);

        Ok(())
    }

    #[tokio::test]
    async fn sharded_python_runner_prefixes_packed_file_batch_outputs() -> Result<()> {
        let Ok(node) = which::which("node") else {
            return Ok(());
        };
        let dir = TempDir::new()?;
        touch(dir.path(), "flat/file0.py")?;
        touch(dir.path(), "flat/file1.py")?;
        touch(dir.path(), "flat/file2.py")?;

        let batch_hex = scip_fixture_hex_for_paths(&["file0.py", "file1.py"])?;
        let empty_doc_hex = scip_fixture_hex("")?;
        let fake_indexer = dir.path().join("fake-scip-python-file-batch.js");
        std::fs::write(
            &fake_indexer,
            format!(
                r#"
const fs = require("fs");
const path = require("path");
const args = process.argv.slice(2);
const targetIndex = args.indexOf("--target-only");
const outputIndex = args.indexOf("--output");
if (targetIndex === -1 || outputIndex === -1 || !args[outputIndex + 1]) {{
  process.exit(2);
}}
const target = args[targetIndex + 1];
const stat = fs.statSync(target);
const key = stat.isDirectory()
  ? fs.readdirSync(target).sort().join(",")
  : path.basename(target);
const fixtures = {{
  "file0.py,file1.py": "{batch_hex}",
  "file2.py": "{empty_doc_hex}",
}};
if (!fixtures[key]) {{
  process.stderr.write("unexpected target " + key + "\n");
  process.exit(3);
}}
fs.writeFileSync(args[outputIndex + 1], Buffer.from(fixtures[key], "hex"));
"#
            ),
        )?;

        let lang = Language {
            kind: LanguageKind::Python,
            evidence: "test".into(),
            evidence_kind: "project_config".into(),
            indexer_ready: true,
            readiness_message: None,
            additional_configs: Vec::new(),
        };
        let output = run_python_indexer_with_file_limit(
            &node,
            &python_entry(vec![fake_indexer.to_string_lossy().to_string()]),
            dir.path(),
            &lang,
            2,
        )
        .await?;

        let merged = Index::parse_from_bytes(&std::fs::read(output)?)?;
        let paths = merged
            .documents
            .iter()
            .map(|doc| (doc.relative_path.as_str(), doc.language.as_str()))
            .collect::<Vec<_>>();

        assert_eq!(
            paths,
            vec![
                ("flat/file0.py", "python"),
                ("flat/file1.py", "python"),
                ("flat/file2.py", "python")
            ]
        );

        Ok(())
    }

    #[test]
    fn plans_python_shards_with_larger_target_budget_after_trigger() -> Result<()> {
        let dir = TempDir::new()?;

        for i in 0..4 {
            touch(dir.path(), &format!("src/file{i}.py"))?;
            touch(dir.path(), &format!("tests/file{i}.py"))?;
        }
        touch(dir.path(), "fixtures/small.py")?;

        let policy = PythonShardPolicy {
            trigger_file_limit: 2,
            target_file_limit: 5,
        };
        let mut targets = plan_python_shards(dir.path(), policy)?
            .into_iter()
            .map(|shard| shard.target.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>();
        targets.sort();

        assert_eq!(targets, vec!["fixtures", "src", "tests"]);

        Ok(())
    }

    #[test]
    fn packs_loose_python_files_in_oversized_directories() -> Result<()> {
        let dir = TempDir::new()?;

        for i in 0..7 {
            touch(dir.path(), &format!("flat/file{i}.py"))?;
        }

        let policy = PythonShardPolicy {
            trigger_file_limit: 2,
            target_file_limit: 3,
        };
        let shards = plan_python_shards(dir.path(), policy)?;
        let counts = shards
            .iter()
            .map(|shard| {
                (
                    shard.target.to_string_lossy().replace('\\', "/"),
                    shard.file_count,
                    shard.files.as_ref().map(Vec::len),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            counts,
            vec![
                ("flat".into(), 3, Some(3)),
                ("flat".into(), 3, Some(3)),
                ("flat/file6.py".into(), 1, None)
            ]
        );

        Ok(())
    }

    #[test]
    fn recursively_splits_failed_python_file_batches() -> Result<()> {
        let dir = TempDir::new()?;
        for i in 0..4 {
            touch(dir.path(), &format!("flat/file{i}.py"))?;
        }

        let shard = PythonShard {
            target: PathBuf::from("flat"),
            file_count: 4,
            files: Some(
                (0..4)
                    .map(|i| PathBuf::from(format!("flat/file{i}.py")))
                    .collect(),
            ),
        };
        let policy = PythonShardPolicy {
            trigger_file_limit: 2,
            target_file_limit: 4,
        };
        let children = split_python_shard(dir.path(), &shard, policy)?;
        let counts = children
            .iter()
            .map(|child| {
                (
                    child.target.to_string_lossy().replace('\\', "/"),
                    child.file_count,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(counts, vec![("flat".into(), 2), ("flat".into(), 2)]);

        Ok(())
    }

    #[test]
    fn retry_split_prefers_directory_children_before_halving_budget() -> Result<()> {
        let dir = TempDir::new()?;
        for i in 0..8 {
            touch(dir.path(), &format!("tests/big/file{i}.py"))?;
        }
        touch(dir.path(), "tests/small/file.py")?;

        let shard = PythonShard {
            target: PathBuf::from("tests"),
            file_count: 9,
            files: None,
        };
        let policy = PythonShardPolicy {
            trigger_file_limit: 2,
            target_file_limit: 10,
        };
        let children = split_python_target_for_retry(dir.path(), &shard, policy)?;
        let targets = children
            .iter()
            .map(|child| child.target.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>();

        assert_eq!(targets, vec!["tests/big", "tests/small"]);

        Ok(())
    }

    #[test]
    fn applies_python_shard_hints_by_presplitting_oom_targets() -> Result<()> {
        let dir = TempDir::new()?;

        for i in 0..4 {
            touch(dir.path(), &format!("src/file{i}.py"))?;
            touch(dir.path(), &format!("tests/api/file{i}.py"))?;
            touch(dir.path(), &format!("tests/unit/file{i}.py"))?;
        }

        let policy = PythonShardPolicy {
            trigger_file_limit: 2,
            target_file_limit: 5,
        };
        let shards = plan_python_shards(dir.path(), policy)?;
        let hints = ["tests".to_string()].into_iter().collect();
        let mut targets = apply_python_shard_hints(dir.path(), shards, &hints, policy)?
            .into_iter()
            .map(|shard| shard.target.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>();
        targets.sort();

        assert_eq!(targets, vec!["src", "tests/api", "tests/unit"]);

        Ok(())
    }
}
