use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use walkdir::WalkDir;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompileCommandDatabaseSkip {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct CompileCommandDiscovery {
    pub configs: Vec<PathBuf>,
    pub skipped: Vec<CompileCommandDatabaseSkip>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct CompileCommandMergeReport {
    pub selected_databases: usize,
    pub skipped_databases: usize,
    pub input_commands: usize,
    pub output_commands: usize,
    pub duplicate_commands: usize,
    pub unique_files: usize,
    pub new_files_vs_primary: usize,
    pub skipped: Vec<CompileCommandDatabaseSkip>,
}

pub fn discover_compile_command_databases(root: &Path) -> Result<CompileCommandDiscovery> {
    let mut candidates = Vec::new();

    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !is_compile_database_ignored(entry))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.file_name().to_string_lossy() != "compile_commands.json" {
            continue;
        }
        if is_compile_database_candidate(root, entry.path()) {
            candidates.push(entry.path().to_path_buf());
        }
    }

    candidates.sort_by_key(|path| compile_database_sort_key(root, path));
    candidates.dedup();

    let mut configs = Vec::new();
    let mut skipped = Vec::new();
    for path in candidates {
        match read_compile_database(&path) {
            Ok(commands) => {
                let cpp_command_count = commands.iter().filter(|entry| is_cpp_entry(entry)).count();
                if cpp_command_count == 0 {
                    skipped.push(CompileCommandDatabaseSkip {
                        path,
                        reason: "no C/C++ compile commands".to_string(),
                    });
                } else {
                    configs.push(path);
                }
            }
            Err(error) => skipped.push(CompileCommandDatabaseSkip {
                path,
                reason: error.to_string(),
            }),
        }
    }

    Ok(CompileCommandDiscovery { configs, skipped })
}

pub fn summarize_compile_command_databases(
    compile_databases: &[PathBuf],
) -> Result<CompileCommandMergeReport> {
    let (_, report) = merge_compile_command_database_entries(compile_databases)?;
    Ok(report)
}

pub fn merge_compile_command_databases(
    compile_databases: &[PathBuf],
    output: &Path,
) -> Result<CompileCommandMergeReport> {
    let (commands, report) = merge_compile_command_database_entries(compile_databases)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    fs::write(output, serde_json::to_vec(&Value::Array(commands))?)
        .with_context(|| format!("Failed to write {}", output.display()))?;
    Ok(report)
}

fn merge_compile_command_database_entries(
    compile_databases: &[PathBuf],
) -> Result<(Vec<Value>, CompileCommandMergeReport)> {
    let mut report = CompileCommandMergeReport {
        selected_databases: compile_databases.len(),
        ..CompileCommandMergeReport::default()
    };
    let mut primary_files = BTreeSet::new();
    let mut output_files = BTreeSet::new();
    let mut seen_commands = HashSet::new();
    let mut merged = Vec::new();

    for (database_index, path) in compile_databases.iter().enumerate() {
        let commands = read_compile_database(path)?;
        for command in commands.into_iter().filter(is_cpp_entry) {
            report.input_commands += 1;
            let Some(file_key) = compile_command_file_key(&command) else {
                continue;
            };
            if database_index == 0 {
                primary_files.insert(file_key.clone());
            }

            let command_key = compile_command_command_key(&command);
            if seen_commands.insert(format!("{file_key}\0{command_key}")) {
                output_files.insert(file_key);
                merged.push(command);
            } else {
                report.duplicate_commands += 1;
            }
        }
    }

    report.output_commands = merged.len();
    report.unique_files = output_files.len();
    report.new_files_vs_primary = output_files.difference(&primary_files).count();
    Ok((merged, report))
}

fn read_compile_database(path: &Path) -> Result<Vec<Value>> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("Failed to parse {}", path.display()))?;
    let Value::Array(commands) = value else {
        bail!("{} is not a JSON array", path.display());
    };
    Ok(commands)
}

fn is_compile_database_candidate(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    if relative == Path::new("compile_commands.json") {
        return true;
    }

    relative
        .parent()
        .is_some_and(|parent| parent.components().any(is_build_output_component))
}

fn is_build_output_component(component: Component<'_>) -> bool {
    let Component::Normal(name) = component else {
        return false;
    };
    let name = name.to_string_lossy();
    name == "build"
        || name.starts_with("build-")
        || name.starts_with("build_")
        || name.starts_with("cmake-build-")
        || name == "out"
        || name.starts_with("out-")
        || name.starts_with("out_")
}

fn compile_database_sort_key(root: &Path, path: &Path) -> (u8, String) {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let display = relative.to_string_lossy().replace('\\', "/");
    let rank = if relative == Path::new("compile_commands.json") {
        0
    } else {
        1
    };
    (rank, display)
}

fn is_compile_database_ignored(entry: &walkdir::DirEntry) -> bool {
    if entry.depth() == 0 {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.')
        || matches!(
            name.as_ref(),
            "node_modules" | "target" | "vendor" | "__pycache__" | "venv" | ".venv" | "dist"
        )
}

fn is_cpp_entry(command: &Value) -> bool {
    command
        .get("file")
        .and_then(Value::as_str)
        .is_some_and(is_cpp_file_path)
}

fn is_cpp_file_path(path: &str) -> bool {
    let Some((_, extension)) = path.rsplit_once('.') else {
        return false;
    };
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "c" | "h" | "cc" | "hh" | "cpp" | "hpp" | "cxx" | "hxx" | "s"
    )
}

fn compile_command_file_key(command: &Value) -> Option<String> {
    let file = command.get("file")?.as_str()?;
    let path = if path_text_is_absolute(file) {
        PathBuf::from(file)
    } else if let Some(directory) = command.get("directory").and_then(Value::as_str) {
        Path::new(directory).join(file)
    } else {
        PathBuf::from(file)
    };
    Some(normalize_path_key(&path))
}

fn path_text_is_absolute(path: &str) -> bool {
    Path::new(path).is_absolute()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path
            .as_bytes()
            .get(0..3)
            .is_some_and(|bytes| bytes[1] == b':' && (bytes[2] == b'/' || bytes[2] == b'\\'))
}

fn normalize_path_key(path: &Path) -> String {
    let mut prefix = String::new();
    let mut absolute = false;
    let mut parts = Vec::new();

    for component in path.components() {
        match component {
            Component::Prefix(value) => {
                prefix = value.as_os_str().to_string_lossy().to_ascii_lowercase();
            }
            Component::RootDir => {
                absolute = true;
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.last().is_some_and(|part: &String| part != "..") {
                    parts.pop();
                } else {
                    parts.push("..".to_string());
                }
            }
            Component::Normal(value) => {
                parts.push(value.to_string_lossy().replace('\\', "/"));
            }
        }
    }

    let joined = parts.join("/");
    match (prefix.is_empty(), absolute, joined.is_empty()) {
        (false, _, true) => prefix,
        (false, _, false) => format!("{prefix}/{joined}"),
        (true, true, true) => "/".to_string(),
        (true, true, false) => format!("/{joined}"),
        (true, false, _) => joined,
    }
}

fn compile_command_command_key(command: &Value) -> String {
    if let Some(arguments) = command.get("arguments").and_then(Value::as_array) {
        serde_json::to_string(arguments).unwrap_or_default()
    } else if let Some(command) = command.get("command").and_then(Value::as_str) {
        command.to_string()
    } else {
        serde_json::to_string(command).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmake_compile_databases::{
        CmakeCompileDatabaseConfig, CmakeCompileDatabaseJobStatus, CmakeCompileDatabasePreset,
        plan_cmake_compile_database_generation,
    };
    use tempfile::TempDir;

    fn write_file(root: &Path, relative_path: &str, contents: &str) {
        let path = root.join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn write_compile_database(root: &Path, relative_path: &str, contents: &str) {
        write_file(root, relative_path, contents);
    }

    #[test]
    fn merge_keeps_same_file_with_different_commands() {
        let dir = TempDir::new().unwrap();
        write_compile_database(
            dir.path(),
            "debug/compile_commands.json",
            r#"[{"directory":"src","file":"a.cc","command":"clang++ -DDEBUG -c a.cc"}]"#,
        );
        write_compile_database(
            dir.path(),
            "release/compile_commands.json",
            r#"[{"directory":"src","file":"a.cc","command":"clang++ -DNDEBUG -c a.cc"}]"#,
        );

        let report = summarize_compile_command_databases(&[
            dir.path().join("debug/compile_commands.json"),
            dir.path().join("release/compile_commands.json"),
        ])
        .unwrap();

        assert_eq!(report.output_commands, 2);
        assert_eq!(report.duplicate_commands, 0);
        assert_eq!(report.unique_files, 1);
    }

    #[test]
    fn llvm_broad_preset_plans_three_cmake_build_dirs() {
        let dir = TempDir::new().unwrap();
        write_file(
            dir.path(),
            "llvm/CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)",
        );
        let config = CmakeCompileDatabaseConfig {
            generate_compile_databases: Some(true),
            preset: Some(CmakeCompileDatabasePreset::LlvmBroad),
            generator: Some("Ninja".to_string()),
            ..CmakeCompileDatabaseConfig::default()
        };

        let plan = plan_cmake_compile_database_generation(dir.path(), &config).unwrap();

        assert_eq!(plan.jobs.len(), 3);
        assert_eq!(
            plan.jobs[0].build_dir,
            dir.path().join("build-scip-io-llvm-all-targets")
        );
        assert_eq!(plan.jobs[0].source_dir, dir.path().join("llvm"));
        assert!(
            plan.jobs[0]
                .args
                .contains(&"-DCMAKE_EXPORT_COMPILE_COMMANDS=ON".to_string())
        );
        assert!(
            plan.jobs[0]
                .args
                .contains(&"-DLLVM_TARGETS_TO_BUILD=all".to_string())
        );
        assert_eq!(
            plan.jobs[1].build_dir,
            dir.path().join("build-scip-io-llvm-projects")
        );
        assert!(
            plan.jobs[1]
                .args
                .iter()
                .any(|arg| { arg.starts_with("-DLLVM_ENABLE_PROJECTS=clang;clang-tools-extra") })
        );
        assert_eq!(
            plan.jobs[2].build_dir,
            dir.path().join("build-scip-io-llvm-runtimes")
        );
        assert!(
            plan.jobs[2]
                .args
                .iter()
                .any(|arg| { arg.starts_with("-DLLVM_ENABLE_RUNTIMES=compiler-rt;libc;libcxx") })
        );
    }

    #[test]
    fn cmake_generation_skips_existing_compile_database_without_refresh() {
        let dir = TempDir::new().unwrap();
        write_file(
            dir.path(),
            "llvm/CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)",
        );
        write_compile_database(
            dir.path(),
            "build-scip-io-llvm-all-targets/compile_commands.json",
            r#"[{"directory":".","file":"a.cpp","command":"clang++ -c a.cpp"}]"#,
        );
        let config = CmakeCompileDatabaseConfig {
            generate_compile_databases: Some(true),
            preset: Some(CmakeCompileDatabasePreset::LlvmBroad),
            ..CmakeCompileDatabaseConfig::default()
        };

        let plan = plan_cmake_compile_database_generation(dir.path(), &config).unwrap();

        assert_eq!(plan.jobs[0].status, CmakeCompileDatabaseJobStatus::Existing);
        assert!(plan.jobs[0].compile_commands.exists());
    }
}
