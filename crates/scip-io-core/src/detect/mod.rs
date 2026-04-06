pub mod languages;

pub use languages::Language;

use anyhow::Result;
use std::path::Path;
use walkdir::WalkDir;

/// Scan a project root and return all detected languages.
pub fn scan_languages(root: &Path) -> Result<Vec<Language>> {
    let mut detected = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for entry in WalkDir::new(root)
        .max_depth(3)
        .into_iter()
        .filter_entry(|e| !is_hidden_or_ignored(e))
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy();
        let relative = entry
            .path()
            .strip_prefix(root)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .into_owned();

        for lang in Language::ALL {
            if seen.contains(lang) {
                continue;
            }
            if lang.matches_manifest(&file_name) {
                seen.insert(*lang);
                detected.push(lang.with_evidence(relative.clone()));
            }
        }
    }

    detected.sort_by_key(|l| l.name());
    Ok(detected)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::languages::LanguageKind;
    use std::fs;
    use tempfile::TempDir;

    /// Create a fixture project inside a tempdir.
    /// Returns (TempDir, project_root) where project_root is a non-hidden
    /// subdirectory, because on Windows TempDir names start with "." which
    /// causes `is_hidden_or_ignored` to skip the entire tree.
    fn create_fixture_project(files: &[&str]) -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let project = dir.path().join("project");
        fs::create_dir_all(&project).unwrap();
        for file in files {
            let path = project.join(file);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(path, "").unwrap();
        }
        (dir, project)
    }

    #[test]
    fn test_detect_typescript() {
        let (_dir, project) = create_fixture_project(&["tsconfig.json", "src/index.ts"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.iter().any(|l| l.kind == LanguageKind::TypeScript));
    }

    #[test]
    fn test_detect_rust() {
        let (_dir, project) = create_fixture_project(&["Cargo.toml", "src/main.rs"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.iter().any(|l| l.kind == LanguageKind::Rust));
    }

    #[test]
    fn test_detect_go() {
        let (_dir, project) = create_fixture_project(&["go.mod", "main.go"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.iter().any(|l| l.kind == LanguageKind::Go));
    }

    #[test]
    fn test_detect_multiple_languages() {
        let (_dir, project) = create_fixture_project(&["Cargo.toml", "pyproject.toml", "go.mod"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.len() >= 3);
        assert!(langs.iter().any(|l| l.kind == LanguageKind::Rust));
        assert!(langs.iter().any(|l| l.kind == LanguageKind::Python));
        assert!(langs.iter().any(|l| l.kind == LanguageKind::Go));
    }

    #[test]
    fn test_detect_empty_project() {
        let (_dir, project) = create_fixture_project(&[]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.is_empty());
    }

    #[test]
    fn test_skips_node_modules() {
        let (_dir, project) = create_fixture_project(&["node_modules/some-pkg/Cargo.toml"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.is_empty());
    }

    #[test]
    fn test_skips_target_dir() {
        let (_dir, project) = create_fixture_project(&["target/debug/Cargo.toml"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.is_empty());
    }

    #[test]
    fn test_skips_vendor_dir() {
        let (_dir, project) = create_fixture_project(&["vendor/lib/go.mod"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.is_empty());
    }

    #[test]
    fn test_skips_hidden_dirs() {
        let (_dir, project) = create_fixture_project(&[".hidden/Cargo.toml"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.is_empty());
    }

    #[test]
    fn test_detect_python_variants() {
        for manifest in &["pyproject.toml", "setup.py", "requirements.txt", "Pipfile"] {
            let (_dir, project) = create_fixture_project(&[manifest]);
            let langs = scan_languages(&project).unwrap();
            assert!(
                langs.iter().any(|l| l.kind == LanguageKind::Python),
                "Failed to detect Python from {}",
                manifest
            );
        }
    }

    #[test]
    fn test_deduplication() {
        // Two manifest files for same language should produce one entry
        let (_dir, project) = create_fixture_project(&["pyproject.toml", "requirements.txt"]);
        let langs = scan_languages(&project).unwrap();
        let python_count = langs.iter().filter(|l| l.kind == LanguageKind::Python).count();
        assert_eq!(python_count, 1);
    }

    #[test]
    fn test_results_sorted_by_name() {
        let (_dir, project) = create_fixture_project(&["go.mod", "Cargo.toml", "tsconfig.json"]);
        let langs = scan_languages(&project).unwrap();
        let names: Vec<&str> = langs.iter().map(|l| l.name()).collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn test_evidence_contains_path() {
        let (_dir, project) = create_fixture_project(&["Cargo.toml"]);
        let langs = scan_languages(&project).unwrap();
        let rust = langs.iter().find(|l| l.kind == LanguageKind::Rust).unwrap();
        assert_eq!(rust.evidence(), "Cargo.toml");
    }

    #[test]
    fn test_nested_manifest_detected() {
        // max_depth is 3, so depth-2 should work
        let (_dir, project) = create_fixture_project(&["sub/Cargo.toml"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.iter().any(|l| l.kind == LanguageKind::Rust));
    }

    #[test]
    fn test_csharp_detected() {
        let (_dir, project) = create_fixture_project(&["MyApp.csproj"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.iter().any(|l| l.kind == LanguageKind::CSharp));
    }

    #[test]
    fn test_java_detected() {
        let (_dir, project) = create_fixture_project(&["pom.xml"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.iter().any(|l| l.kind == LanguageKind::Java));
    }

    #[test]
    fn test_ruby_detected() {
        let (_dir, project) = create_fixture_project(&["Gemfile"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.iter().any(|l| l.kind == LanguageKind::Ruby));
    }

    #[test]
    fn test_kotlin_detected() {
        let (_dir, project) = create_fixture_project(&["build.gradle.kts"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.iter().any(|l| l.kind == LanguageKind::Kotlin));
    }
}
