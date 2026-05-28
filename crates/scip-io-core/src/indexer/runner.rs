use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;

use anyhow::{Context, Result};

use crate::detect::Language;
use crate::indexer::IndexerEntry;
use crate::merge::merge_scip_files;
use crate::scip_language::{
    compact_scip_file, normalize_path_component, normalize_scip_file_languages,
    prefix_scip_file_document_paths, relativize_scip_file_document_paths,
};

const PYTHON_SHARD_FILE_LIMIT: usize = 750;
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
    if entry.indexer_name == "scip-python" && config_paths.is_empty() {
        return run_python_indexer_with_file_limit(
            binary,
            entry,
            project_root,
            lang,
            PYTHON_SHARD_FILE_LIMIT,
        )
        .await;
    }

    run_indexer_once_with_configs(binary, entry, project_root, lang, config_paths).await
}

async fn run_indexer_once_with_configs(
    binary: &Path,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    config_paths: &[PathBuf],
) -> Result<PathBuf> {
    tracing::info!(
        indexer = %entry.indexer_name,
        lang = lang.name(),
        root = %project_root.display(),
        "running indexer"
    );

    // Build the output path so multiple indexers don't clobber each other
    let output_file = project_root.join(format!("{}.scip", lang.name()));

    let mut cmd = tokio::process::Command::new(binary);
    cmd.current_dir(project_root);

    for arg in build_indexer_args(entry, &output_file, config_paths) {
        cmd.arg(arg);
    }

    let status = cmd
        .status()
        .await
        .with_context(|| format!("Failed to execute {}", binary.display()))?;

    if !status.success() {
        anyhow::bail!(
            "{} exited with status {} for {}",
            entry.indexer_name,
            status,
            lang.name()
        );
    }

    // The indexer might have written to its default name instead
    if !output_file.exists() {
        let default_output = project_root.join(&entry.output_file);
        if default_output.exists() {
            std::fs::rename(&default_output, &output_file)?;
        } else {
            anyhow::bail!(
                "Indexer {} did not produce output file (expected {})",
                entry.indexer_name,
                output_file.display()
            );
        }
    }

    let updated_languages = normalize_scip_file_languages(&output_file, Some(lang.name()))?;
    if updated_languages > 0 {
        tracing::info!(
            path = %output_file.display(),
            docs = updated_languages,
            "filled missing SCIP document languages"
        );
    }
    let updated_paths = relativize_scip_file_document_paths(&output_file, project_root)?;
    if updated_paths > 0 {
        tracing::info!(
            path = %output_file.display(),
            docs = updated_paths,
            "relativized SCIP document paths"
        );
    }
    let compaction = compact_scip_file(&output_file)?;
    if compaction.changed() {
        tracing::info!(
            path = %output_file.display(),
            duplicate_documents = compaction.duplicate_documents,
            duplicate_occurrences = compaction.duplicate_occurrences,
            duplicate_symbols = compaction.duplicate_symbols,
            "compacted duplicate SCIP facts"
        );
    }

    Ok(output_file)
}

async fn run_python_indexer_with_file_limit(
    binary: &Path,
    entry: &IndexerEntry,
    project_root: &Path,
    lang: &Language,
    max_files_per_shard: usize,
) -> Result<PathBuf> {
    let max_files_per_shard = max_files_per_shard.max(1);
    let shards = plan_python_shards(project_root, max_files_per_shard)?;
    let python_file_count = shards.iter().map(|shard| shard.file_count).sum::<usize>();
    if shards.is_empty() || (shards.len() == 1 && shards[0].target.as_os_str().is_empty()) {
        return run_indexer_once_with_configs(binary, entry, project_root, lang, &[]).await;
    }

    let output_file = project_root.join(format!("{}.scip", lang.name()));
    if output_file.exists() {
        std::fs::remove_file(&output_file)
            .with_context(|| format!("Failed to remove stale {}", output_file.display()))?;
    }

    tracing::info!(
        indexer = %entry.indexer_name,
        root = %project_root.display(),
        files = python_file_count,
        shards = shards.len(),
        max_files_per_shard,
        "running scip-python in memory-bounded shards"
    );

    let temp_dir = tempfile::Builder::new()
        .prefix("scip-io-python-shards-")
        .tempdir()
        .context("Failed to create temporary directory for scip-python shards")?;

    let mut shard_outputs = Vec::<PathBuf>::new();
    let mut queue = VecDeque::from(shards);
    let mut shard_counter = 0usize;

    while let Some(shard) = queue.pop_front() {
        shard_counter += 1;
        match run_python_shard(
            binary,
            entry,
            project_root,
            lang,
            temp_dir.path(),
            shard_counter,
            &shard,
        )
        .await?
        {
            PythonShardRun::Succeeded(output) => shard_outputs.push(output),
            PythonShardRun::Failed(failure) if failure.is_oom() => {
                let child_shards =
                    split_python_target(project_root, &failure.shard.target, max_files_per_shard)?;
                if child_shards.is_empty()
                    || (child_shards.len() == 1 && child_shards[0].target == failure.shard.target)
                {
                    failure.bail(entry, lang)?;
                }

                tracing::warn!(
                    target = %failure.shard.target.display(),
                    child_shards = child_shards.len(),
                    status = failure.status_text(),
                    "scip-python shard hit a heap limit; retrying with smaller shards"
                );

                for child_shard in child_shards.into_iter().rev() {
                    queue.push_front(child_shard);
                }
            }
            PythonShardRun::Failed(failure) => failure.bail(entry, lang)?,
        }
    }

    merge_scip_files(&shard_outputs, &output_file)?;
    let compaction = compact_scip_file(&output_file)?;
    if compaction.changed() {
        tracing::info!(
            path = %output_file.display(),
            duplicate_documents = compaction.duplicate_documents,
            duplicate_occurrences = compaction.duplicate_occurrences,
            duplicate_symbols = compaction.duplicate_symbols,
            "compacted duplicate SCIP facts after sharded merge"
        );
    }

    Ok(output_file)
}

enum PythonShardRun {
    Succeeded(PathBuf),
    Failed(PythonShardFailure),
}

#[derive(Debug)]
struct PythonShardFailure {
    shard: PythonShard,
    status: ExitStatus,
    stdout: String,
    stderr: String,
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
            "{} exited with status {} for {} shard {}\nstdout:\n{}\nstderr:\n{}",
            entry.indexer_name,
            self.status,
            lang.name(),
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
    shard: &PythonShard,
) -> Result<PythonShardRun> {
    let shard_output = temp_dir.join(format!("python-shard-{shard_number:04}.scip"));
    tracing::debug!(
        target = %shard.target.display(),
        files = shard.file_count,
        output = %shard_output.display(),
        "running scip-python shard"
    );

    let mut cmd = tokio::process::Command::new(binary);
    cmd.current_dir(project_root);
    for arg in build_python_shard_args(entry, shard, &shard_output) {
        cmd.arg(arg);
    }

    let output = cmd
        .output()
        .await
        .with_context(|| format!("Failed to execute {}", binary.display()))?;

    if !output.status.success() {
        return Ok(PythonShardRun::Failed(PythonShardFailure {
            shard: shard.clone(),
            status: output.status,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
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
    let updated_paths = relativize_scip_file_document_paths(&shard_output, project_root)?;
    if updated_paths > 0 {
        tracing::debug!(
            path = %shard_output.display(),
            docs = updated_paths,
            "relativized SCIP document paths in Python shard"
        );
    }
    if let Some(prefix) = python_shard_document_prefix(project_root, shard) {
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

    Ok(PythonShardRun::Succeeded(shard_output))
}

fn build_python_shard_args(
    entry: &IndexerEntry,
    shard: &PythonShard,
    output_file: &Path,
) -> Vec<OsString> {
    let mut args = Vec::with_capacity(entry.default_args.len() + 4);
    args.extend(entry.default_args.iter().map(OsString::from));
    args.push(OsString::from("--target-only"));
    args.push(shard.target.as_os_str().to_os_string());
    args.push(OsString::from("--output"));
    args.push(output_file.as_os_str().to_os_string());
    args
}

fn count_python_files(project_root: &Path) -> Result<usize> {
    Ok(
        split_python_target(project_root, Path::new(""), usize::MAX)?
            .into_iter()
            .map(|shard| shard.file_count)
            .sum(),
    )
}

fn plan_python_shards(project_root: &Path, max_files_per_shard: usize) -> Result<Vec<PythonShard>> {
    let python_file_count = count_python_files(project_root)?;
    if python_file_count == 0 {
        return Ok(Vec::new());
    }
    if python_file_count <= max_files_per_shard {
        return Ok(vec![PythonShard {
            target: PathBuf::new(),
            file_count: python_file_count,
        }]);
    }
    split_python_target(project_root, Path::new(""), max_files_per_shard)
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
            }]
        } else {
            Vec::new()
        });
    }

    let mut children = Vec::new();
    for entry in std::fs::read_dir(&absolute_target)
        .with_context(|| format!("Failed to read {}", absolute_target.display()))?
    {
        let entry = entry
            .with_context(|| format!("Failed to read entry in {}", absolute_target.display()))?;
        let file_name = entry.file_name();
        let child_target = target.join(&file_name);
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
                });
            } else {
                children.extend(split_python_target(
                    project_root,
                    &child_target,
                    max_files_per_shard,
                )?);
            }
        } else if file_type.is_file() && is_python_source_path(&child_target) {
            children.push(PythonShard {
                target: child_target,
                file_count: 1,
            });
        }
    }

    children.sort_by(|a, b| a.target.cmp(&b.target));
    Ok(children)
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
        let child_target = target.join(&file_name);
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
    if shard.target.as_os_str().is_empty() {
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

/// Build argv after the binary name, placing supported config files before
/// the output option so CLIs with positional project arguments can parse them.
pub fn build_indexer_args(
    entry: &IndexerEntry,
    output_file: &Path,
    config_paths: &[PathBuf],
) -> Vec<OsString> {
    let mut args = Vec::new();
    let mut has_output_arg = false;

    for arg in &entry.default_args {
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

    let project_root = output_file
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty());
    args.extend(
        config_paths
            .iter()
            .map(|path| config_path_arg(path, project_root)),
    );

    // Config-driven indexers need an explicit destination because the default
    // output file can be overwritten when one invocation spans several configs.
    // Keep no-config runs on their historical default argv and let the rename
    // fallback handle indexers that do not expose an output flag.
    if !config_paths.is_empty() && !has_output_arg {
        args.push(OsString::from("--output"));
        args.push(output_file.as_os_str().to_os_string());
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

        let mut targets = plan_python_shards(dir.path(), 2)?
            .into_iter()
            .map(|shard| shard.target.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>();
        targets.sort();

        assert_eq!(
            targets,
            vec![
                "large/a.py",
                "large/b.py",
                "large/c.pyw",
                "loose.py",
                "small"
            ]
        );

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
        };
        let file_shard = PythonShard {
            target: PathBuf::from("tools/a.py"),
            file_count: 1,
        };
        let root_file_shard = PythonShard {
            target: PathBuf::from("a.py"),
            file_count: 1,
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
}
