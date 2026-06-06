use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use crate::compile_commands::discover_compile_command_databases;
use crate::detect::languages::LanguageKind;

/// Languages in SCIP-IO's registry whose indexers have a known multi-config
/// contract. This list is based on indexer capability, not local install state.
pub fn supported_additional_config_languages() -> &'static [LanguageKind] {
    &[
        LanguageKind::TypeScript,
        LanguageKind::CSharp,
        LanguageKind::Cpp,
    ]
}

/// Discover directories that contain supported additional config files.
pub fn discover_additional_config_roots(root: &Path) -> Result<Vec<PathBuf>> {
    let mut roots = BTreeSet::new();

    for config in discover_compile_command_databases(root)?.configs {
        if let Some(parent) = config.parent() {
            roots.insert(parent.to_path_buf());
        }
    }

    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !is_hidden_or_ignored(entry))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy();
        if file_name != "compile_commands.json"
            && supported_config_language(&file_name).is_some()
            && let Some(parent) = entry.path().parent()
        {
            roots.insert(parent.to_path_buf());
        }
    }

    Ok(roots.into_iter().collect())
}

/// Discover config files for languages whose indexers accept multiple config
/// paths in one run.
pub fn discover_additional_configs(root: &Path, language: LanguageKind) -> Result<Vec<PathBuf>> {
    if language == LanguageKind::TypeScript {
        return discover_root_level_typescript_configs(root);
    }
    if language == LanguageKind::Cpp {
        return Ok(discover_compile_command_databases(root)?.configs);
    }

    let mut configs = BTreeSet::new();
    let Some(matcher) = config_matcher(language) else {
        return Ok(Vec::new());
    };

    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !is_hidden_or_ignored(entry))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy();
        if matcher(&file_name) {
            configs.insert(entry.path().to_path_buf());
        }
    }

    Ok(configs.into_iter().collect())
}

pub fn supported_config_language(file_name: &str) -> Option<LanguageKind> {
    if is_typescript_config(file_name) {
        Some(LanguageKind::TypeScript)
    } else if is_dotnet_config(file_name) {
        Some(LanguageKind::CSharp)
    } else if file_name == "compile_commands.json" {
        Some(LanguageKind::Cpp)
    } else {
        None
    }
}

fn config_matcher(language: LanguageKind) -> Option<fn(&str) -> bool> {
    match language {
        LanguageKind::TypeScript => Some(is_typescript_config),
        LanguageKind::CSharp => Some(is_dotnet_config),
        _ => None,
    }
}

fn is_typescript_config(file_name: &str) -> bool {
    file_name == "tsconfig.json"
        || (file_name.starts_with("tsconfig.") && file_name.ends_with(".json"))
}

fn discover_root_level_typescript_configs(root: &Path) -> Result<Vec<PathBuf>> {
    let mut configs = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        if is_typescript_config(&file_name) {
            configs.push(entry.path());
        }
    }

    configs.sort_by(|a, b| {
        let a_name = a.file_name().and_then(|name| name.to_str()).unwrap_or("");
        let b_name = b.file_name().and_then(|name| name.to_str()).unwrap_or("");
        let a_primary = a_name == "tsconfig.json";
        let b_primary = b_name == "tsconfig.json";
        b_primary.cmp(&a_primary).then_with(|| a.cmp(b))
    });
    Ok(configs)
}

fn is_dotnet_config(file_name: &str) -> bool {
    file_name.ends_with(".sln") || file_name.ends_with(".csproj") || file_name.ends_with(".vbproj")
}

fn is_hidden_or_ignored(entry: &walkdir::DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.')
        || name == "node_modules"
        || name == "target"
        || name == "vendor"
        || name == "__pycache__"
        || name == "venv"
        || name == ".venv"
        || name == "dist"
        || name == "build"
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn write_compile_database(root: &Path, relative_path: &str, contents: &str) {
        let path = root.join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn rels(root: &Path, paths: Vec<PathBuf>) -> Vec<String> {
        paths
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    }

    #[test]
    fn typescript_discovers_all_named_tsconfigs_in_stable_order() {
        let (_dir, root) = fixture(&[
            "tsconfig.json",
            "tsconfig.build.json",
            "tsconfig.test.json",
            "nested/tsconfig.extra.json",
            "node_modules/pkg/tsconfig.json",
            "dist/tsconfig.generated.json",
        ]);

        let configs = discover_additional_configs(&root, LanguageKind::TypeScript).unwrap();

        assert_eq!(
            rels(&root, configs),
            vec!["tsconfig.json", "tsconfig.build.json", "tsconfig.test.json"]
        );
    }

    #[test]
    fn csharp_discovers_solution_and_project_configs() {
        let (_dir, root) = fixture(&[
            "App.sln",
            "src/App/App.csproj",
            "src/Legacy/Legacy.vbproj",
            "node_modules/noise/Noise.csproj",
        ]);

        let configs = discover_additional_configs(&root, LanguageKind::CSharp).unwrap();

        assert_eq!(
            rels(&root, configs),
            vec!["App.sln", "src/App/App.csproj", "src/Legacy/Legacy.vbproj"]
        );
    }

    #[test]
    fn cpp_discovers_root_and_build_compile_databases() {
        let (_dir, root) = fixture(&["src/main.cpp"]);
        let valid_root =
            r#"[{"directory":".","file":"src/main.cpp","command":"clang++ -c src/main.cpp"}]"#;
        let valid_build = r#"[{"directory":"build-scip-wsl","file":"../lib/a.cc","arguments":["clang++","-c","../lib/a.cc"]}]"#;
        let valid_cmake = r#"[{"directory":"cmake-build-debug","file":"../tools/tool.c","command":"clang -c ../tools/tool.c"}]"#;
        let source_fixture = r#"[{"directory":"clang/test/Index","file":"fixture.cpp","command":"clang++ -c fixture.cpp"}]"#;
        let non_cpp =
            r#"[{"directory":"build-js","file":"src/app.ts","command":"tsc src/app.ts"}]"#;

        write_compile_database(&root, "compile_commands.json", valid_root);
        write_compile_database(&root, "build-scip-wsl/compile_commands.json", valid_build);
        write_compile_database(
            &root,
            "cmake-build-debug/compile_commands.json",
            valid_cmake,
        );
        write_compile_database(
            &root,
            "clang/test/Index/compile_commands.json",
            source_fixture,
        );
        write_compile_database(&root, "build-js/compile_commands.json", non_cpp);
        write_compile_database(&root, "cmake-build-bad/compile_commands.json", "{not json");

        let configs = discover_additional_configs(&root, LanguageKind::Cpp).unwrap();

        assert_eq!(
            rels(&root, configs),
            vec![
                "compile_commands.json",
                "build-scip-wsl/compile_commands.json",
                "cmake-build-debug/compile_commands.json"
            ]
        );
    }

    #[test]
    fn cpp_additional_config_roots_include_valid_build_databases() {
        let (_dir, root) = fixture(&["src/main.cpp"]);
        write_compile_database(
            &root,
            "build-scip-wsl/compile_commands.json",
            r#"[{"directory":"build-scip-wsl","file":"../src/main.cpp","command":"clang++ -c ../src/main.cpp"}]"#,
        );
        write_compile_database(
            &root,
            "clang/test/Index/compile_commands.json",
            r#"[{"directory":"clang/test/Index","file":"fixture.cpp","command":"clang++ -c fixture.cpp"}]"#,
        );

        let roots = discover_additional_config_roots(&root).unwrap();

        assert_eq!(rels(&root, roots), vec!["build-scip-wsl"]);
    }

    #[test]
    fn unsupported_languages_have_no_additional_configs() {
        let (_dir, root) = fixture(&["Cargo.toml", "alt/Cargo.toml"]);

        let configs = discover_additional_configs(&root, LanguageKind::Rust).unwrap();

        assert!(configs.is_empty());
    }

    #[test]
    fn supported_additional_config_languages_are_registry_backed() {
        for language in supported_additional_config_languages() {
            let lang = language.with_evidence(String::new());
            assert!(
                crate::indexer::registry::REGISTRY
                    .runnable_for(&lang)
                    .is_some()
            );
        }
    }

    #[test]
    fn discovers_roots_with_supported_additional_configs() {
        let (_dir, root) = fixture(&[
            "tools/tsconfig.scripts.json",
            "services/app/App.csproj",
            "src/main.rs",
        ]);

        let roots = discover_additional_config_roots(&root).unwrap();

        assert_eq!(rels(&root, roots), vec!["services/app", "tools"]);
    }
}
