use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;
use walkdir::WalkDir;

pub use crate::compile_commands::merge_compile_command_databases;
use crate::indexer::IndexerEntry;

/// Conservative sharding capability advertised by an upstream SCIP indexer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShardCapability {
    PythonTargetOnly,
    ProjectArguments,
    ModuleRoots,
    CompileCommands,
    Unsupported,
}

/// Pure plan for a bounded indexing unit. Runners decide when to execute these
/// shards; the planner only encodes safe boundaries exposed by upstream tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedShard {
    ProjectArgument(PathBuf),
    ModuleRoot(PathBuf),
    CompileCommands {
        compile_commands: PathBuf,
        start: usize,
        end: usize,
    },
}

pub fn shard_capability_for(entry: &IndexerEntry) -> ShardCapability {
    match entry.indexer_name.as_str() {
        "scip-python" => ShardCapability::PythonTargetOnly,
        "scip-typescript" | "scip-dotnet" => ShardCapability::ProjectArguments,
        "scip-go" | "rust-analyzer" | "scip-java" => ShardCapability::ModuleRoots,
        "scip-clang" => ShardCapability::CompileCommands,
        _ => ShardCapability::Unsupported,
    }
}

pub fn plan_project_argument_shards(
    entry: &IndexerEntry,
    config_paths: &[PathBuf],
) -> Vec<PlannedShard> {
    if shard_capability_for(entry) != ShardCapability::ProjectArguments || config_paths.len() <= 1 {
        return Vec::new();
    }

    config_paths
        .iter()
        .cloned()
        .map(PlannedShard::ProjectArgument)
        .collect()
}

pub fn plan_module_root_shards(
    entry: &IndexerEntry,
    project_root: &Path,
) -> Result<Vec<PlannedShard>> {
    let roots = match entry.indexer_name.as_str() {
        "scip-go" => discover_manifest_roots(project_root, |name| name == "go.mod")?,
        "rust-analyzer" => discover_manifest_roots(project_root, |name| name == "Cargo.toml")?,
        "scip-java" => discover_manifest_roots(project_root, is_jvm_build_root)?,
        _ => Vec::new(),
    };

    Ok(roots.into_iter().map(PlannedShard::ModuleRoot).collect())
}

pub fn plan_compile_command_shards(
    compile_commands: &Path,
    max_commands_per_shard: usize,
) -> Result<Vec<PlannedShard>> {
    let max_commands_per_shard = max_commands_per_shard.max(1);
    let command_count = compile_command_count(compile_commands)?;
    if command_count <= max_commands_per_shard {
        return Ok(Vec::new());
    }

    let mut shards = Vec::new();
    let mut start = 0usize;
    while start < command_count {
        let end = (start + max_commands_per_shard).min(command_count);
        shards.push(PlannedShard::CompileCommands {
            compile_commands: compile_commands.to_path_buf(),
            start,
            end,
        });
        start = end;
    }
    Ok(shards)
}

pub fn read_compile_command_chunk(
    compile_commands: &Path,
    start: usize,
    end: usize,
) -> Result<Value> {
    let raw = std::fs::read_to_string(compile_commands)
        .with_context(|| format!("Failed to read {}", compile_commands.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("Failed to parse {}", compile_commands.display()))?;
    let Value::Array(commands) = value else {
        anyhow::bail!("{} is not a JSON array", compile_commands.display());
    };

    let chunk = commands
        .into_iter()
        .skip(start)
        .take(end.saturating_sub(start))
        .collect::<Vec<_>>();
    Ok(Value::Array(chunk))
}

fn compile_command_count(compile_commands: &Path) -> Result<usize> {
    let raw = std::fs::read_to_string(compile_commands)
        .with_context(|| format!("Failed to read {}", compile_commands.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("Failed to parse {}", compile_commands.display()))?;
    let Value::Array(commands) = value else {
        anyhow::bail!("{} is not a JSON array", compile_commands.display());
    };
    Ok(commands.len())
}

fn discover_manifest_roots(root: &Path, matcher: impl Fn(&str) -> bool) -> Result<Vec<PathBuf>> {
    let mut roots = BTreeSet::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !is_hidden_or_ignored(entry))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy();
        if matcher(&file_name)
            && let Some(parent) = entry.path().parent()
        {
            roots.insert(parent.to_path_buf());
        }
    }

    if roots.len() <= 1 {
        return Ok(Vec::new());
    }
    Ok(roots.into_iter().collect())
}

fn is_jvm_build_root(file_name: &str) -> bool {
    matches!(
        file_name,
        "pom.xml"
            | "build.gradle"
            | "build.gradle.kts"
            | "build.sbt"
            | "settings.gradle"
            | "settings.gradle.kts"
    )
}

fn is_hidden_or_ignored(entry: &walkdir::DirEntry) -> bool {
    if entry.depth() == 0 {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.')
        || matches!(
            name.as_ref(),
            "node_modules"
                | "target"
                | "vendor"
                | "__pycache__"
                | "venv"
                | ".venv"
                | "dist"
                | "build"
                | ".gradle"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::languages::LanguageKind;
    use crate::indexer::InstallMethod;
    use crate::indexer::backend::BackendCapabilities;
    use tempfile::TempDir;

    fn entry(name: &str, language: &str) -> IndexerEntry {
        IndexerEntry {
            indexer_name: name.into(),
            language: language.into(),
            github_repo: "owner/repo".into(),
            binary_name: name.into(),
            version: "1.0.0".into(),
            default_args: vec!["index".into()],
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

    fn write_compile_database(root: &Path, relative_path: &str, contents: &str) -> Result<PathBuf> {
        let path = root.join(relative_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, contents)?;
        Ok(path)
    }

    fn rels(root: &Path, shards: Vec<PlannedShard>) -> Vec<String> {
        shards
            .into_iter()
            .map(|shard| match shard {
                PlannedShard::ProjectArgument(path) | PlannedShard::ModuleRoot(path) => path,
                PlannedShard::CompileCommands { .. } => unreachable!(),
            })
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap_or(path.as_path())
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn maps_indexers_to_capabilities() {
        assert_eq!(
            shard_capability_for(&entry("scip-python", "python")),
            ShardCapability::PythonTargetOnly
        );
        assert_eq!(
            shard_capability_for(&entry("scip-typescript", "typescript")),
            ShardCapability::ProjectArguments
        );
        assert_eq!(
            shard_capability_for(&entry("scip-clang", LanguageKind::Cpp.name())),
            ShardCapability::CompileCommands
        );
        assert_eq!(
            shard_capability_for(&entry("scip-ruby", "ruby")),
            ShardCapability::Unsupported
        );
    }

    #[test]
    fn plans_project_argument_shards_for_config_capable_indexers() {
        let shards = plan_project_argument_shards(
            &entry("scip-typescript", "typescript"),
            &[
                PathBuf::from("tsconfig.json"),
                PathBuf::from("tsconfig.test.json"),
            ],
        );

        assert_eq!(
            shards,
            vec![
                PlannedShard::ProjectArgument(PathBuf::from("tsconfig.json")),
                PlannedShard::ProjectArgument(PathBuf::from("tsconfig.test.json")),
            ]
        );
    }

    #[test]
    fn plans_module_root_shards_for_safe_module_indexers() -> Result<()> {
        let dir = TempDir::new()?;
        touch(dir.path(), "go.mod")?;
        touch(dir.path(), "cmd/api/go.mod")?;
        touch(dir.path(), "node_modules/pkg/go.mod")?;
        touch(dir.path(), "services/jobs/go.mod")?;

        let shards = plan_module_root_shards(&entry("scip-go", "go"), dir.path())?;

        assert_eq!(
            rels(dir.path(), shards),
            vec!["", "cmd/api", "services/jobs"]
        );
        Ok(())
    }

    #[test]
    fn plans_jvm_build_root_shards_without_hidden_build_dirs() -> Result<()> {
        let dir = TempDir::new()?;
        touch(dir.path(), "pom.xml")?;
        touch(dir.path(), "plugins/a/build.gradle.kts")?;
        touch(dir.path(), "plugins/b/build.sbt")?;
        touch(dir.path(), ".gradle/generated/build.gradle")?;

        let shards = plan_module_root_shards(&entry("scip-java", "java"), dir.path())?;

        assert_eq!(rels(dir.path(), shards), vec!["", "plugins/a", "plugins/b"]);
        Ok(())
    }

    #[test]
    fn plans_compile_command_chunks() -> Result<()> {
        let dir = TempDir::new()?;
        let compile_commands = dir.path().join("compile_commands.json");
        std::fs::write(
            &compile_commands,
            r#"[{"file":"a.cc"},{"file":"b.cc"},{"file":"c.cc"},{"file":"d.cc"},{"file":"e.cc"}]"#,
        )?;

        let shards = plan_compile_command_shards(&compile_commands, 2)?;

        assert_eq!(
            shards,
            vec![
                PlannedShard::CompileCommands {
                    compile_commands: compile_commands.clone(),
                    start: 0,
                    end: 2,
                },
                PlannedShard::CompileCommands {
                    compile_commands: compile_commands.clone(),
                    start: 2,
                    end: 4,
                },
                PlannedShard::CompileCommands {
                    compile_commands,
                    start: 4,
                    end: 5,
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn extracts_compile_command_chunks() -> Result<()> {
        let dir = TempDir::new()?;
        let compile_commands = dir.path().join("compile_commands.json");
        std::fs::write(
            &compile_commands,
            r#"[{"file":"a.cc"},{"file":"b.cc"},{"file":"c.cc"}]"#,
        )?;

        let chunk = read_compile_command_chunk(&compile_commands, 1, 3)?;

        assert_eq!(chunk, serde_json::json!([{"file":"b.cc"},{"file":"c.cc"}]));
        Ok(())
    }

    #[test]
    fn merges_compile_databases_and_dedupes_identical_commands() -> Result<()> {
        let dir = TempDir::new()?;
        let primary = write_compile_database(
            dir.path(),
            "compile_commands.json",
            r#"[
              {"directory":"src","file":"a.cc","command":"clang++ -c a.cc"},
              {"directory":"src","file":"b.cc","arguments":["clang++","-c","b.cc"]}
            ]"#,
        )?;
        let secondary = write_compile_database(
            dir.path(),
            "build/compile_commands.json",
            r#"[
              {"directory":"src","file":"a.cc","command":"clang++ -c a.cc"},
              {"directory":"build","file":"../src/c.cc","command":"clang++ -c ../src/c.cc"}
            ]"#,
        )?;
        let merged = dir.path().join("merged-compile_commands.json");

        let report = merge_compile_command_databases(&[primary, secondary], &merged)?;
        let commands = read_compile_command_chunk(&merged, 0, usize::MAX)?;

        assert_eq!(
            commands,
            serde_json::json!([
                {"directory":"src","file":"a.cc","command":"clang++ -c a.cc"},
                {"directory":"src","file":"b.cc","arguments":["clang++","-c","b.cc"]},
                {"directory":"build","file":"../src/c.cc","command":"clang++ -c ../src/c.cc"}
            ])
        );
        assert_eq!(report.input_commands, 4);
        assert_eq!(report.output_commands, 3);
        assert_eq!(report.duplicate_commands, 1);
        assert_eq!(report.unique_files, 3);
        assert_eq!(report.new_files_vs_primary, 1);
        Ok(())
    }

    #[test]
    fn merge_compile_databases_keeps_same_file_with_different_commands() -> Result<()> {
        let dir = TempDir::new()?;
        let debug = write_compile_database(
            dir.path(),
            "debug/compile_commands.json",
            r#"[{"directory":"debug","file":"../src/a.cc","command":"clang++ -DDEBUG -c ../src/a.cc"}]"#,
        )?;
        let release = write_compile_database(
            dir.path(),
            "release/compile_commands.json",
            r#"[{"directory":"release","file":"../src/a.cc","command":"clang++ -DNDEBUG -c ../src/a.cc"}]"#,
        )?;
        let merged = dir.path().join("merged-compile_commands.json");

        let report = merge_compile_command_databases(&[debug, release], &merged)?;

        assert_eq!(report.output_commands, 2);
        assert_eq!(report.unique_files, 1);
        assert_eq!(report.duplicate_commands, 0);
        Ok(())
    }
}
