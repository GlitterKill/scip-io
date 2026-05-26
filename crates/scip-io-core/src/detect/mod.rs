pub mod languages;

pub use languages::Language;

use anyhow::Result;
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Options that control manifest-based language scanning.
#[derive(Debug, Clone, Copy)]
pub struct LanguageScanOptions {
    /// Maximum `walkdir` depth to scan. `None` scans all non-ignored
    /// descendants, while `Some(3)` preserves the original default behavior.
    pub max_depth: Option<usize>,
}

impl Default for LanguageScanOptions {
    fn default() -> Self {
        Self { max_depth: Some(3) }
    }
}

/// Scan a project root and return all detected languages.
pub fn scan_languages(root: &Path) -> Result<Vec<Language>> {
    scan_languages_with_options(root, LanguageScanOptions::default())
}

/// Scan a project root with explicit scan options.
pub fn scan_languages_with_options(
    root: &Path,
    options: LanguageScanOptions,
) -> Result<Vec<Language>> {
    let mut detected: Vec<Language> = Vec::new();
    let mut seen = HashSet::new();

    for entry in walker(root, options) {
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
                if lang.matches_manifest(&file_name)
                    && let Some(existing) =
                        detected.iter_mut().find(|detected| detected.kind == *lang)
                    && is_better_evidence(&relative, &existing.evidence)
                {
                    existing.evidence = relative.clone();
                }
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

fn is_better_evidence(candidate: &str, current: &str) -> bool {
    let candidate_depth = path_depth(candidate);
    let current_depth = path_depth(current);
    candidate_depth < current_depth || (candidate_depth == current_depth && candidate < current)
}

fn path_depth(path: &str) -> usize {
    path.split(['/', '\\']).count().saturating_sub(1)
}

/// Discover manifest-bearing project roots below `root`.
///
/// This intentionally returns directories containing known language manifests,
/// not every directory with source files. Ignored directories are skipped.
pub fn discover_project_roots(root: &Path) -> Result<Vec<PathBuf>> {
    discover_project_roots_with_options(root, LanguageScanOptions { max_depth: None })
}

/// Discover project roots with explicit scan options.
pub fn discover_project_roots_with_options(
    root: &Path,
    options: LanguageScanOptions,
) -> Result<Vec<PathBuf>> {
    let mut roots = BTreeSet::new();

    for entry in walker(root, options) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy();
        if is_language_manifest(&file_name)
            && let Some(parent) = entry.path().parent()
        {
            roots.insert(parent.to_path_buf());
        }
    }

    Ok(roots.into_iter().collect())
}

fn walker(
    root: &Path,
    options: LanguageScanOptions,
) -> impl Iterator<Item = walkdir::Result<walkdir::DirEntry>> {
    let walker = WalkDir::new(root);
    let walker = match options.max_depth {
        Some(depth) => walker.max_depth(depth),
        None => walker,
    };
    walker
        .into_iter()
        .filter_entry(|e| !is_hidden_or_ignored(e))
}

fn is_language_manifest(file_name: &str) -> bool {
    Language::ALL
        .iter()
        .any(|lang| lang.matches_manifest(file_name))
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
        let python_count = langs
            .iter()
            .filter(|l| l.kind == LanguageKind::Python)
            .count();
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
    fn test_evidence_prefers_shallow_manifest() {
        let (_dir, project) = create_fixture_project(&[
            "nested/tsconfig.json",
            "tsconfig.json",
            "z-not-a-manifest.md",
        ]);
        let langs = scan_languages(&project).unwrap();
        let typescript = langs
            .iter()
            .find(|l| l.kind == LanguageKind::TypeScript)
            .unwrap();
        assert_eq!(typescript.evidence(), "tsconfig.json");
    }

    #[test]
    fn test_nested_manifest_detected() {
        // max_depth is 3, so depth-2 should work
        let (_dir, project) = create_fixture_project(&["sub/Cargo.toml"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.iter().any(|l| l.kind == LanguageKind::Rust));
    }

    #[test]
    fn scan_languages_with_options_respects_depth() {
        let (_dir, project) = create_fixture_project(&["services/api/Cargo.toml"]);

        let shallow =
            scan_languages_with_options(&project, LanguageScanOptions { max_depth: Some(2) })
                .unwrap();
        assert!(shallow.is_empty());

        let deep =
            scan_languages_with_options(&project, LanguageScanOptions { max_depth: Some(3) })
                .unwrap();
        assert!(deep.iter().any(|l| l.kind == LanguageKind::Rust));
    }

    #[test]
    fn discover_project_roots_finds_manifest_directories() {
        let (_dir, project) = create_fixture_project(&[
            "services/api/Cargo.toml",
            "packages/web/package.json",
            "node_modules/noise/go.mod",
        ]);

        let roots = discover_project_roots(&project).unwrap();
        let roots: Vec<_> = roots
            .iter()
            .map(|path| {
                path.strip_prefix(&project)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();

        assert_eq!(roots, vec!["packages/web", "services/api"]);
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

    #[test]
    fn test_both_javascript_and_typescript_detected() {
        // A typical TS project has both package.json and tsconfig.json.
        // Detection should report both languages so users see what's
        // in their project; dedup of the actual indexer invocation is
        // the indexing layer's job.
        let (_dir, project) =
            create_fixture_project(&["package.json", "tsconfig.json", "src/index.ts"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.iter().any(|l| l.kind == LanguageKind::TypeScript));
        assert!(langs.iter().any(|l| l.kind == LanguageKind::JavaScript));
    }

    #[test]
    fn test_javascript_detected_without_typescript() {
        // Pure JavaScript project: only package.json, no tsconfig.json.
        let (_dir, project) = create_fixture_project(&["package.json", "src/index.js"]);
        let langs = scan_languages(&project).unwrap();
        assert!(langs.iter().any(|l| l.kind == LanguageKind::JavaScript));
        assert!(!langs.iter().any(|l| l.kind == LanguageKind::TypeScript));
    }
}
