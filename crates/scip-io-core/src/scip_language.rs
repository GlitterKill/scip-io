use std::path::Path;

use anyhow::{Context, Result};
use protobuf::Message;
use scip::types::Index;

/// Return SCIP's canonical language name when the input is one SCIP-IO supports.
pub fn normalize_language_name(raw: &str) -> Option<&'static str> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "typescript" => Some("typescript"),
        "javascript" => Some("javascript"),
        "python" => Some("python"),
        "rust" => Some("rust"),
        "go" => Some("go"),
        "java" => Some("java"),
        "csharp" | "c#" => Some("csharp"),
        "ruby" => Some("ruby"),
        "kotlin" => Some("kotlin"),
        "cpp" | "c++" => Some("cpp"),
        "scala" => Some("scala"),
        _ => None,
    }
}

/// Infer SCIP language metadata from a document path when an indexer omits it.
pub fn infer_language_from_document_path(relative_path: &str) -> Option<&'static str> {
    let extension = Path::new(relative_path)
        .extension()
        .and_then(|ext| ext.to_str())?
        .to_ascii_lowercase();

    match extension.as_str() {
        "ts" | "tsx" | "mts" | "cts" => Some("typescript"),
        "js" | "jsx" | "mjs" | "cjs" => Some("javascript"),
        "py" | "pyw" => Some("python"),
        "rs" => Some("rust"),
        "go" => Some("go"),
        "java" => Some("java"),
        "cs" => Some("csharp"),
        "rb" | "rake" => Some("ruby"),
        "kt" | "kts" => Some("kotlin"),
        "c" | "h" | "cc" | "hh" | "cpp" | "hpp" | "cxx" | "hxx" => Some("cpp"),
        "scala" | "sc" => Some("scala"),
        _ => None,
    }
}

/// Fill only missing document language fields, preserving metadata from indexers
/// that already provided a more precise value.
pub fn fill_missing_document_languages(
    index: &mut Index,
    fallback_language: Option<&str>,
) -> usize {
    let normalized_fallback = fallback_language.and_then(normalize_language_name);
    let mut updated = 0;

    for doc in &mut index.documents {
        if !doc.language.trim().is_empty() {
            continue;
        }

        let Some(language) =
            infer_language_from_document_path(&doc.relative_path).or(normalized_fallback)
        else {
            continue;
        };

        doc.language = language.to_string();
        updated += 1;
    }

    updated
}

/// Normalize a SCIP file in place after an external indexer writes it.
pub fn normalize_scip_file_languages(
    path: &Path,
    fallback_language: Option<&str>,
) -> Result<usize> {
    let bytes =
        std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut index = Index::parse_from_bytes(&bytes)
        .with_context(|| format!("Failed to parse SCIP index from {}", path.display()))?;

    let updated = fill_missing_document_languages(&mut index, fallback_language);
    if updated == 0 {
        return Ok(0);
    }

    let bytes = index
        .write_to_bytes()
        .context("Failed to serialize normalized SCIP index")?;
    std::fs::write(path, bytes).with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(updated)
}

/// Prefix every document path in a SCIP file in place.
///
/// This is used when separately indexing monorepo sub-project roots. Most
/// indexers emit paths relative to the sub-project root; the final merged index
/// needs paths relative to the top-level workspace to avoid collisions such as
/// `services/a/src/main.rs` and `services/b/src/main.rs` both becoming
/// `src/main.rs`.
pub fn prefix_scip_file_document_paths(path: &Path, prefix: &str) -> Result<usize> {
    let prefix = normalize_path_component(prefix);
    if prefix.is_empty() {
        return Ok(0);
    }

    let bytes =
        std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut index = Index::parse_from_bytes(&bytes)
        .with_context(|| format!("Failed to parse SCIP index from {}", path.display()))?;

    let mut updated = 0;
    for doc in &mut index.documents {
        let relative_path = normalize_path_component(&doc.relative_path);
        if relative_path.is_empty() {
            continue;
        }
        if relative_path == prefix || relative_path.starts_with(&format!("{prefix}/")) {
            continue;
        }
        doc.relative_path = format!("{prefix}/{relative_path}");
        updated += 1;
    }

    if updated == 0 {
        return Ok(0);
    }

    let bytes = index
        .write_to_bytes()
        .context("Failed to serialize path-prefixed SCIP index")?;
    std::fs::write(path, bytes).with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(updated)
}

/// Rewrite absolute document paths under `project_root` back to repo-relative
/// paths.
///
/// Some indexers preserve the exact project/config path shape they receive.
/// On Windows that can leak extended absolute paths such as
/// `//?/F:/repo/src/main.ts` into `Document.relative_path`. SCIP consumers
/// expect this field to be repository-relative, so normalize it before merge.
pub fn relativize_scip_file_document_paths(path: &Path, project_root: &Path) -> Result<usize> {
    let bytes =
        std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut index = Index::parse_from_bytes(&bytes)
        .with_context(|| format!("Failed to parse SCIP index from {}", path.display()))?;

    let project_root = normalize_absolute_path_for_compare(&project_root.display().to_string());
    if project_root.is_empty() {
        return Ok(0);
    }

    let mut updated = 0;
    for doc in &mut index.documents {
        let Some(relative_path) = relativize_document_path(&doc.relative_path, &project_root)
        else {
            continue;
        };
        if relative_path != doc.relative_path {
            doc.relative_path = relative_path;
            updated += 1;
        }
    }

    if updated == 0 {
        return Ok(0);
    }

    let bytes = index
        .write_to_bytes()
        .context("Failed to serialize path-relativized SCIP index")?;
    std::fs::write(path, bytes).with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(updated)
}

fn relativize_document_path(raw_path: &str, project_root: &str) -> Option<String> {
    let normalized = normalize_absolute_path_for_compare(raw_path);
    if normalized == project_root {
        return None;
    }

    if let Some(suffix) = normalized.strip_prefix(&format!("{project_root}/")) {
        let relative_path = normalize_path_component(suffix);
        if !relative_path.is_empty() {
            return Some(relative_path);
        }
    }

    if !looks_absolute_path(&normalized) {
        let relative_path = normalize_path_component(raw_path);
        if relative_path != raw_path {
            return Some(relative_path);
        }
    }

    None
}

fn normalize_path_component(path: &str) -> String {
    path.replace('\\', "/")
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .collect::<Vec<_>>()
        .join("/")
}

fn normalize_absolute_path_for_compare(path: &str) -> String {
    let mut path = path.replace('\\', "/");
    if let Some(rest) = path.strip_prefix("file:///") {
        path = rest.to_string();
    }
    if let Some(rest) = path.strip_prefix("//?/") {
        path = rest.to_string();
    }
    path = path.trim_end_matches('/').to_string();

    let bytes = path.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        let drive = path[0..1].to_ascii_lowercase();
        path.replace_range(0..1, &drive);
    }

    path
}

fn looks_absolute_path(path: &str) -> bool {
    path.starts_with('/') || path.as_bytes().get(1) == Some(&b':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use scip::types::Document;
    use tempfile::NamedTempFile;

    #[test]
    fn infers_languages_from_common_extensions() {
        assert_eq!(
            infer_language_from_document_path("src/main.ts"),
            Some("typescript")
        );
        assert_eq!(
            infer_language_from_document_path("src/main.jsx"),
            Some("javascript")
        );
        assert_eq!(
            infer_language_from_document_path("src/lib.rs"),
            Some("rust")
        );
        assert_eq!(infer_language_from_document_path("src/main.go"), Some("go"));
        assert_eq!(
            infer_language_from_document_path("src/main.cs"),
            Some("csharp")
        );
    }

    #[test]
    fn fills_missing_language_without_overwriting_existing_values() {
        let mut index = Index::new();

        let mut missing = Document::new();
        missing.relative_path = "src/main.ts".into();

        let mut existing = Document::new();
        existing.relative_path = "src/main.js".into();
        existing.language = "javascriptreact".into();

        index.documents.push(missing);
        index.documents.push(existing);

        let updated = fill_missing_document_languages(&mut index, Some("typescript"));

        assert_eq!(updated, 1);
        assert_eq!(index.documents[0].language, "typescript");
        assert_eq!(index.documents[1].language, "javascriptreact");
    }

    #[test]
    fn uses_fallback_when_path_has_no_known_extension() {
        let mut index = Index::new();
        let mut doc = Document::new();
        doc.relative_path = "Makefile".into();
        index.documents.push(doc);

        let updated = fill_missing_document_languages(&mut index, Some("cpp"));

        assert_eq!(updated, 1);
        assert_eq!(index.documents[0].language, "cpp");
    }

    #[test]
    fn normalizes_scip_file_in_place() -> Result<()> {
        let mut index = Index::new();
        let mut doc = Document::new();
        doc.relative_path = "src/main.ts".into();
        index.documents.push(doc);

        let file = NamedTempFile::new()?;
        std::fs::write(file.path(), index.write_to_bytes()?)?;

        let updated = normalize_scip_file_languages(file.path(), Some("typescript"))?;
        let normalized = Index::parse_from_bytes(&std::fs::read(file.path())?)?;

        assert_eq!(updated, 1);
        assert_eq!(normalized.documents[0].language, "typescript");
        Ok(())
    }

    #[test]
    fn prefixes_scip_document_paths_in_place() -> Result<()> {
        let mut index = Index::new();
        let mut doc = Document::new();
        doc.relative_path = "src/main.rs".into();
        index.documents.push(doc);

        let file = NamedTempFile::new()?;
        std::fs::write(file.path(), index.write_to_bytes()?)?;

        let updated = prefix_scip_file_document_paths(file.path(), "services/api")?;
        let normalized = Index::parse_from_bytes(&std::fs::read(file.path())?)?;

        assert_eq!(updated, 1);
        assert_eq!(
            normalized.documents[0].relative_path,
            "services/api/src/main.rs"
        );
        Ok(())
    }

    #[test]
    fn relativizes_absolute_scip_document_paths_in_place() -> Result<()> {
        let mut index = Index::new();
        let mut extended = Document::new();
        extended.relative_path =
            "//?/F:/Claude/projects/sdl-mcp/sdl-mcp/scripts/bench-ppr-weight.ts".into();
        index.documents.push(extended);

        let mut windows = Document::new();
        windows.relative_path =
            "F:\\Claude\\projects\\sdl-mcp\\sdl-mcp\\src\\db\\ladybug.ts".into();
        index.documents.push(windows);

        let mut relative = Document::new();
        relative.relative_path = "src\\indexer\\adapter\\BaseAdapter.ts".into();
        index.documents.push(relative);

        let file = NamedTempFile::new()?;
        std::fs::write(file.path(), index.write_to_bytes()?)?;

        let updated = relativize_scip_file_document_paths(
            file.path(),
            Path::new("F:/Claude/projects/sdl-mcp/sdl-mcp"),
        )?;
        let normalized = Index::parse_from_bytes(&std::fs::read(file.path())?)?;

        assert_eq!(updated, 3);
        assert_eq!(
            normalized.documents[0].relative_path,
            "scripts/bench-ppr-weight.ts"
        );
        assert_eq!(normalized.documents[1].relative_path, "src/db/ladybug.ts");
        assert_eq!(
            normalized.documents[2].relative_path,
            "src/indexer/adapter/BaseAdapter.ts"
        );
        Ok(())
    }
}
