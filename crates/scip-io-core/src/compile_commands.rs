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
    pub databases: Vec<CompileCommandDatabaseCoverage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct CompileCommandCoverageOptions {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub min_new_files: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct CompileCommandSelection {
    pub configs: Vec<PathBuf>,
    pub databases: Vec<CompileCommandDatabaseCoverage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompileCommandDatabaseCoverage {
    pub path: PathBuf,
    pub relative_path: String,
    pub selected: bool,
    pub skip_reason: Option<String>,
    pub input_commands: usize,
    pub output_commands: usize,
    pub duplicate_commands: usize,
    pub unique_files: usize,
    pub new_unique_files: usize,
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

pub fn select_compile_command_databases(
    root: &Path,
    compile_databases: &[PathBuf],
    options: &CompileCommandCoverageOptions,
) -> Result<CompileCommandSelection> {
    let mut selection = CompileCommandSelection::default();
    let mut selected_files = BTreeSet::new();
    let mut selected_commands = HashSet::new();
    let min_new_files = options.min_new_files.unwrap_or(0);

    for path in compile_databases {
        let relative_path = compile_database_relative_path(root, path);
        let commands = read_compile_database(path)?;
        let mut input_commands = 0;
        let mut database_files = BTreeSet::new();
        let mut command_keys = Vec::new();

        for command in commands.iter().filter(|entry| is_cpp_entry(entry)) {
            input_commands += 1;
            let Some(file_key) = compile_command_file_key(command) else {
                continue;
            };
            database_files.insert(file_key.clone());
            command_keys.push(format!(
                "{file_key}\0{}",
                compile_command_command_key(command)
            ));
        }

        let new_unique_files = database_files.difference(&selected_files).count();
        let mut new_command_keys = HashSet::new();
        let mut output_commands = 0;
        let mut duplicate_commands = 0;
        for command_key in &command_keys {
            if selected_commands.contains(command_key)
                || !new_command_keys.insert(command_key.clone())
            {
                duplicate_commands += 1;
            } else {
                output_commands += 1;
            }
        }
        let unique_files = database_files.len();

        let skip_reason = compile_database_coverage_skip_reason(
            &relative_path,
            selection.configs.is_empty(),
            new_unique_files,
            min_new_files,
            options,
        );
        let selected = skip_reason.is_none();
        if selected {
            selected_files.extend(database_files);
            selected_commands.extend(new_command_keys);
            selection.configs.push(path.clone());
        }

        selection.databases.push(CompileCommandDatabaseCoverage {
            path: path.clone(),
            relative_path,
            selected,
            skip_reason,
            input_commands,
            output_commands,
            duplicate_commands,
            unique_files,
            new_unique_files,
        });
    }

    Ok(selection)
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

fn compile_database_coverage_skip_reason(
    relative_path: &str,
    is_first_selected: bool,
    new_unique_files: usize,
    min_new_files: usize,
    options: &CompileCommandCoverageOptions,
) -> Option<String> {
    if !options.include.is_empty() && !path_matches_any_pattern(relative_path, &options.include) {
        return Some("not matched by cpp.coverage.include".to_string());
    }
    if path_matches_any_pattern(relative_path, &options.exclude) {
        return Some("excluded by cpp.coverage.exclude".to_string());
    }
    if !is_first_selected && new_unique_files < min_new_files {
        return Some(format!(
            "adds {new_unique_files} new unique file(s), below cpp.coverage.min_new_files={min_new_files}"
        ));
    }
    None
}

fn path_matches_any_pattern(path: &str, patterns: &[String]) -> bool {
    patterns
        .iter()
        .any(|pattern| path_matches_pattern(path, pattern))
}

fn path_matches_pattern(path: &str, pattern: &str) -> bool {
    let path = path.replace('\\', "/");
    let pattern = pattern.replace('\\', "/");
    if !pattern.contains('*') && !pattern.contains('?') {
        let directory_pattern = pattern.trim_end_matches('/');
        return path == directory_pattern || path.starts_with(&format!("{directory_pattern}/"));
    }
    wildcard_matches(&pattern, &path)
}

fn wildcard_matches(pattern: &str, text: &str) -> bool {
    let pattern = pattern.as_bytes();
    let text = text.as_bytes();
    let mut pattern_index = 0;
    let mut text_index = 0;
    let mut star_index = None;
    let mut star_text_index = 0;

    while text_index < text.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == text[text_index])
        {
            pattern_index += 1;
            text_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            star_text_index = text_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            star_text_index += 1;
            text_index = star_text_index;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }
    pattern_index == pattern.len()
}

fn compile_database_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
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
    fn coverage_selection_skips_excluded_and_low_gain_databases() {
        let dir = TempDir::new().unwrap();
        write_compile_database(
            dir.path(),
            "compile_commands.json",
            r#"[{"directory":"src","file":"a.cc","command":"clang++ -c a.cc"}]"#,
        );
        write_compile_database(
            dir.path(),
            "build-duplicate/compile_commands.json",
            r#"[{"directory":"src","file":"a.cc","command":"clang++ -c a.cc"}]"#,
        );
        write_compile_database(
            dir.path(),
            "build-small/compile_commands.json",
            r#"[{"directory":"src","file":"b.cc","command":"clang++ -c b.cc"}]"#,
        );
        write_compile_database(
            dir.path(),
            "build-large/compile_commands.json",
            r#"[
              {"directory":"src","file":"c.cc","command":"clang++ -c c.cc"},
              {"directory":"src","file":"d.cc","command":"clang++ -c d.cc"}
            ]"#,
        );
        write_compile_database(
            dir.path(),
            "build-excluded/compile_commands.json",
            r#"[{"directory":"src","file":"e.cc","command":"clang++ -c e.cc"}]"#,
        );

        let discovery = discover_compile_command_databases(dir.path()).unwrap();
        let selection = select_compile_command_databases(
            dir.path(),
            &discovery.configs,
            &CompileCommandCoverageOptions {
                exclude: vec!["build-excluded/**".to_string()],
                min_new_files: Some(2),
                ..CompileCommandCoverageOptions::default()
            },
        )
        .unwrap();

        assert_eq!(
            selection.configs,
            vec![
                dir.path().join("compile_commands.json"),
                dir.path().join("build-large/compile_commands.json")
            ]
        );
        let reports = selection
            .databases
            .iter()
            .map(|database| {
                (
                    database.relative_path.clone(),
                    database.selected,
                    database.new_unique_files,
                    database.skip_reason.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            reports,
            vec![
                ("compile_commands.json".to_string(), true, 1, None),
                (
                    "build-duplicate/compile_commands.json".to_string(),
                    false,
                    0,
                    Some(
                        "adds 0 new unique file(s), below cpp.coverage.min_new_files=2".to_string()
                    )
                ),
                (
                    "build-excluded/compile_commands.json".to_string(),
                    false,
                    1,
                    Some("excluded by cpp.coverage.exclude".to_string())
                ),
                (
                    "build-large/compile_commands.json".to_string(),
                    true,
                    2,
                    None
                ),
                (
                    "build-small/compile_commands.json".to_string(),
                    false,
                    1,
                    Some(
                        "adds 1 new unique file(s), below cpp.coverage.min_new_files=2".to_string()
                    )
                )
            ]
        );
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
