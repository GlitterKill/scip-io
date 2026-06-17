use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::LanguageKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplicitFile {
    pub absolute_path: PathBuf,
    pub repo_relative_path: PathBuf,
    pub language_kind: LanguageKind,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExplicitFileSelection {
    files: Vec<ExplicitFile>,
}

impl ExplicitFileSelection {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn files(&self) -> &[ExplicitFile] {
        &self.files
    }

    /// Return selected files owned by this project root, relative to that root.
    pub fn project_relative_files(
        &self,
        project_root: &Path,
        owned_child_prefixes: &[String],
    ) -> Vec<PathBuf> {
        let project_root = canonical_project_root(project_root);
        self.files
            .iter()
            .filter_map(|file| {
                file.absolute_path
                    .strip_prefix(&project_root)
                    .ok()
                    .map(Path::to_path_buf)
            })
            .filter(|relative| !path_is_owned_by_child_root(relative, owned_child_prefixes))
            .collect()
    }

    pub fn project_language_kinds(
        &self,
        project_root: &Path,
        owned_child_prefixes: &[String],
    ) -> BTreeSet<LanguageKind> {
        let project_root = canonical_project_root(project_root);
        self.files
            .iter()
            .filter(|file| file.absolute_path.starts_with(&project_root))
            .filter(|file| {
                file.absolute_path
                    .strip_prefix(&project_root)
                    .ok()
                    .is_none_or(|relative| {
                        !path_is_owned_by_child_root(relative, owned_child_prefixes)
                    })
            })
            .map(|file| file.language_kind)
            .collect()
    }
}

fn canonical_project_root(project_root: &Path) -> PathBuf {
    project_root
        .canonicalize()
        .unwrap_or_else(|_| project_root.to_path_buf())
}

pub fn read_explicit_file_list(path: &Path) -> Result<Vec<PathBuf>> {
    let raw =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    Ok(raw
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(PathBuf::from)
        .collect())
}

pub fn resolve_explicit_file_selection(
    repo_root: &Path,
    inputs: &[PathBuf],
) -> Result<ExplicitFileSelection> {
    if inputs.is_empty() {
        return Ok(ExplicitFileSelection::default());
    }

    let repo_root = repo_root
        .canonicalize()
        .with_context(|| format!("Failed to resolve repo root {}", repo_root.display()))?;
    let mut seen = BTreeSet::new();
    let mut files = Vec::new();

    for input in inputs {
        let candidate = if input.is_absolute() {
            input.clone()
        } else {
            repo_root.join(input)
        };
        let absolute_path = candidate.canonicalize().with_context(|| {
            format!(
                "Explicit index file {} does not exist",
                display_input_path(input)
            )
        })?;
        if !absolute_path.is_file() {
            bail!(
                "Explicit index file {} is not a regular file",
                display_input_path(input)
            );
        }
        if !absolute_path.starts_with(&repo_root) {
            bail!(
                "Explicit index file {} is outside repo root {}",
                absolute_path.display(),
                repo_root.display()
            );
        }
        let Some(language_kind) = LanguageKind::from_source_path(&absolute_path) else {
            bail!(
                "Explicit index file {} is not a supported source file",
                display_input_path(input)
            );
        };
        if !seen.insert(absolute_path.clone()) {
            continue;
        }
        let repo_relative_path = absolute_path
            .strip_prefix(&repo_root)
            .with_context(|| {
                format!(
                    "Failed to make {} relative to {}",
                    absolute_path.display(),
                    repo_root.display()
                )
            })?
            .to_path_buf();
        files.push(ExplicitFile {
            absolute_path,
            repo_relative_path,
            language_kind,
        });
    }

    Ok(ExplicitFileSelection { files })
}

fn path_is_owned_by_child_root(path: &Path, owned_child_prefixes: &[String]) -> bool {
    let path = normalize_repo_relative_path(path);
    owned_child_prefixes.iter().any(|prefix| {
        let prefix = normalize_path_text(prefix);
        path == prefix || path.starts_with(&format!("{prefix}/"))
    })
}

fn normalize_repo_relative_path(path: &Path) -> String {
    normalize_path_text(&path.to_string_lossy())
}

fn normalize_path_text(path: &str) -> String {
    path.replace('\\', "/")
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn display_input_path(path: &Path) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use super::{read_explicit_file_list, resolve_explicit_file_selection};
    use crate::LanguageKind;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn resolves_repo_relative_source_files() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(root.join("src/lib.rs"), "").unwrap();

        let selection =
            resolve_explicit_file_selection(&root, &[PathBuf::from("src/lib.rs")]).unwrap();

        assert_eq!(selection.len(), 1);
        assert_eq!(
            selection.files()[0].repo_relative_path,
            PathBuf::from("src/lib.rs")
        );
        assert_eq!(selection.files()[0].language_kind, LanguageKind::Rust);
    }

    #[test]
    fn rejects_files_outside_repo_root() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("repo");
        fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("outside.py");
        fs::write(&outside, "").unwrap();

        let error = resolve_explicit_file_selection(&root, &[outside])
            .unwrap_err()
            .to_string();

        assert!(error.contains("outside repo root"));
    }

    #[test]
    fn file_list_skips_blank_lines_and_comments() {
        let dir = TempDir::new().unwrap();
        let list = dir.path().join("files.txt");
        fs::write(&list, "\n# generated\nsrc/a.py\n src/b.py \n").unwrap();

        let files = read_explicit_file_list(&list).unwrap();

        assert_eq!(
            files,
            vec![PathBuf::from("src/a.py"), PathBuf::from("src/b.py")]
        );
    }
}
