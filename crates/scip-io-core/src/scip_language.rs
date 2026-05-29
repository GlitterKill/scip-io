use std::collections::{BTreeMap, HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use protobuf::Message;
use scip::types::{Document, Index, Occurrence};

use crate::validate::{IndexStats, validate_scip_file};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScipCompactionStats {
    pub documents_before: usize,
    pub documents_after: usize,
    pub normalized_paths: usize,
    pub duplicate_documents: usize,
    pub duplicate_occurrences: usize,
    pub duplicate_symbols: usize,
}

impl ScipCompactionStats {
    pub fn changed(self) -> bool {
        self.normalized_paths > 0
            || self.duplicate_documents > 0
            || self.duplicate_occurrences > 0
            || self.duplicate_symbols > 0
    }
}

#[derive(Debug, Clone, Default)]
pub struct ScipPublishStats {
    pub index: IndexStats,
    pub compaction: ScipCompactionStats,
}

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

/// Fill missing document language fields and canonicalize known aliases.
///
/// A known source-file extension is more specific than indexer-supplied
/// language metadata. Some JVM-family indexers report every document as
/// `java`, even when the path is Kotlin or Scala, so prefer path inference when
/// it is available and preserve non-empty metadata only for paths we cannot
/// classify confidently.
pub fn fill_missing_document_languages(
    index: &mut Index,
    fallback_language: Option<&str>,
) -> usize {
    let normalized_fallback = fallback_language.and_then(normalize_language_name);
    let mut updated = 0;

    for doc in &mut index.documents {
        let raw_language = doc.language.trim();
        let inferred_language = infer_language_from_document_path(&doc.relative_path);
        let normalized_existing = normalize_language_name(raw_language);
        let language = if let Some(language) = inferred_language {
            language
        } else if let Some(language) = normalized_existing {
            language
        } else if raw_language.is_empty() {
            if let Some(language) = normalized_fallback {
                language
            } else {
                continue;
            }
        } else {
            continue;
        };

        if doc.language != language {
            doc.language = language.to_string();
            updated += 1;
        }
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

/// Compact a SCIP index in memory by merging duplicate document paths and
/// removing repeated occurrence and symbol facts inside each final document.
pub fn compact_index(index: &mut Index) -> ScipCompactionStats {
    let documents_before = index.documents.len();
    let mut stats = ScipCompactionStats {
        documents_before,
        ..Default::default()
    };

    let mut documents_by_path = BTreeMap::<String, Document>::new();
    for mut document in std::mem::take(&mut index.documents) {
        let normalized_path = normalize_path_component(&document.relative_path);
        if !normalized_path.is_empty() && normalized_path != document.relative_path {
            document.relative_path = normalized_path.clone();
            stats.normalized_paths += 1;
        }

        compact_document_facts(&mut document, &mut stats);

        let key = document.relative_path.clone();
        if let Some(existing) = documents_by_path.get_mut(&key) {
            stats.duplicate_documents += 1;
            merge_compacted_document(existing, document, &mut stats);
        } else {
            documents_by_path.insert(key, document);
        }
    }

    index.documents = documents_by_path.into_values().collect();
    stats.documents_after = index.documents.len();
    stats
}

/// Compact a SCIP file in place after an indexer or merge writes it.
pub fn compact_scip_file(path: &Path) -> Result<ScipCompactionStats> {
    let bytes =
        std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut index = Index::parse_from_bytes(&bytes)
        .with_context(|| format!("Failed to parse SCIP index from {}", path.display()))?;

    let stats = compact_index(&mut index);
    if !stats.changed() {
        return Ok(stats);
    }

    let bytes = index
        .write_to_bytes()
        .context("Failed to serialize compacted SCIP index")?;
    std::fs::write(path, bytes).with_context(|| format!("Failed to write {}", path.display()))?;

    Ok(stats)
}

/// Compact and validate a staged SCIP file, then atomically publish it.
///
/// Callers use this for final per-language and merged outputs so a failed
/// validation cannot replace the previous successful index.
pub fn compact_validate_publish_scip_file(
    staged: &Path,
    destination: &Path,
) -> Result<ScipPublishStats> {
    let stats = compact_validate_scip_file(staged)?;
    publish_scip_file_atomically(staged, destination)?;
    Ok(stats)
}

/// Copy an existing SCIP file through the same final-output protection path.
pub fn copy_scip_file_atomically(source: &Path, destination: &Path) -> Result<ScipPublishStats> {
    let temp_dir = tempfile::Builder::new()
        .prefix("scip-io-copy-")
        .tempdir()
        .context("Failed to create temporary directory for SCIP copy")?;
    let staged = temp_dir.path().join(
        destination
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("index.scip")),
    );
    std::fs::copy(source, &staged).with_context(|| {
        format!(
            "Failed to stage {} for copy to {}",
            source.display(),
            destination.display()
        )
    })?;
    compact_validate_publish_scip_file(&staged, destination)
}

pub fn compact_validate_scip_file(path: &Path) -> Result<ScipPublishStats> {
    let compaction = compact_scip_file(path)?;
    let validation = validate_scip_file(path)?;
    if !validation.valid {
        let errors = validation
            .errors
            .iter()
            .map(|error| format!("{}: {}", error.kind, error.message))
            .collect::<Vec<_>>()
            .join("; ");
        anyhow::bail!("Invalid SCIP output after compaction: {errors}");
    }

    Ok(ScipPublishStats {
        index: validation.stats.unwrap_or_default(),
        compaction,
    })
}

pub fn publish_scip_file_atomically(source: &Path, destination: &Path) -> Result<()> {
    let Some(parent) = destination.parent() else {
        anyhow::bail!(
            "Output destination has no parent: {}",
            destination.display()
        );
    };
    std::fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create {}", parent.display()))?;

    let publish_temp = parent.join(unique_sidecar_name(destination, "tmp"));
    std::fs::copy(source, &publish_temp).with_context(|| {
        format!(
            "Failed to stage {} for publishing to {}",
            source.display(),
            destination.display()
        )
    })?;

    let backup = parent.join(unique_sidecar_name(destination, "backup"));
    if destination.exists() {
        std::fs::rename(destination, &backup).with_context(|| {
            format!(
                "Failed to move existing {} to {}",
                destination.display(),
                backup.display()
            )
        })?;
    }

    match std::fs::rename(&publish_temp, destination) {
        Ok(()) => {
            if backup.exists() {
                std::fs::remove_file(&backup)
                    .with_context(|| format!("Failed to remove {}", backup.display()))?;
            }
            Ok(())
        }
        Err(error) => {
            if backup.exists() {
                let _ = std::fs::rename(&backup, destination);
            }
            let _ = std::fs::remove_file(&publish_temp);
            Err(error).with_context(|| {
                format!(
                    "Failed to publish {} to {}",
                    source.display(),
                    destination.display()
                )
            })
        }
    }
}

fn unique_sidecar_name(destination: &Path, suffix: &str) -> PathBuf {
    let file_name = destination
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("index.scip");
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    PathBuf::from(format!(
        ".{file_name}.{pid}.{timestamp}.{suffix}",
        pid = std::process::id()
    ))
}

fn compact_document_facts(document: &mut Document, stats: &mut ScipCompactionStats) {
    let mut occurrence_fingerprints = HashMap::<u64, Vec<usize>>::new();
    let mut compacted_occurrences = Vec::with_capacity(document.occurrences.len());
    for occurrence in std::mem::take(&mut document.occurrences) {
        if !push_unique_occurrence(
            &mut compacted_occurrences,
            &mut occurrence_fingerprints,
            occurrence,
        ) {
            stats.duplicate_occurrences += 1;
        }
    }
    document.occurrences = compacted_occurrences;

    let mut symbol_ids = HashSet::<String>::new();
    let mut compacted_symbols = Vec::with_capacity(document.symbols.len());
    for symbol in std::mem::take(&mut document.symbols) {
        if symbol_ids.insert(symbol.symbol.clone()) {
            compacted_symbols.push(symbol);
        } else {
            stats.duplicate_symbols += 1;
        }
    }
    document.symbols = compacted_symbols;
}

fn merge_compacted_document(
    target: &mut Document,
    source: Document,
    stats: &mut ScipCompactionStats,
) {
    let target_language = target.language.trim();
    let source_language = source.language.trim();
    if target_language.is_empty() && !source_language.is_empty() {
        target.language = source_language.to_string();
    } else if !source_language.is_empty() && target_language != source_language {
        tracing::warn!(
            path = %target.relative_path,
            target = %target.language,
            source = %source.language,
            "conflicting SCIP document languages during compaction; keeping first document language"
        );
    }

    let mut occurrence_fingerprints = occurrence_fingerprints(&target.occurrences);
    for occurrence in source.occurrences {
        if !push_unique_occurrence(
            &mut target.occurrences,
            &mut occurrence_fingerprints,
            occurrence,
        ) {
            stats.duplicate_occurrences += 1;
        }
    }

    let mut symbol_ids = target
        .symbols
        .iter()
        .map(|symbol| symbol.symbol.clone())
        .collect::<HashSet<_>>();
    for symbol in source.symbols {
        if symbol_ids.insert(symbol.symbol.clone()) {
            target.symbols.push(symbol);
        } else {
            stats.duplicate_symbols += 1;
        }
    }
}

fn occurrence_fingerprints(occurrences: &[Occurrence]) -> HashMap<u64, Vec<usize>> {
    let mut fingerprints = HashMap::<u64, Vec<usize>>::new();
    for (index, occurrence) in occurrences.iter().enumerate() {
        fingerprints
            .entry(occurrence_fingerprint(occurrence))
            .or_default()
            .push(index);
    }
    fingerprints
}

fn push_unique_occurrence(
    occurrences: &mut Vec<Occurrence>,
    fingerprints: &mut HashMap<u64, Vec<usize>>,
    occurrence: Occurrence,
) -> bool {
    let fingerprint = occurrence_fingerprint(&occurrence);
    if fingerprints.get(&fingerprint).is_some_and(|indices| {
        indices
            .iter()
            .any(|&index| occurrences[index] == occurrence)
    }) {
        return false;
    }

    let index = occurrences.len();
    occurrences.push(occurrence);
    fingerprints.entry(fingerprint).or_default().push(index);
    true
}

fn occurrence_fingerprint(occurrence: &Occurrence) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();

    occurrence.range.hash(&mut hasher);
    occurrence.symbol.hash(&mut hasher);
    occurrence.symbol_roles.hash(&mut hasher);
    occurrence.override_documentation.hash(&mut hasher);
    occurrence.syntax_kind.value().hash(&mut hasher);
    occurrence.enclosing_range.hash(&mut hasher);
    occurrence.diagnostics.len().hash(&mut hasher);
    for diagnostic in &occurrence.diagnostics {
        match diagnostic.write_to_bytes() {
            Ok(bytes) => bytes.hash(&mut hasher),
            Err(_) => format!("{diagnostic:?}").hash(&mut hasher),
        }
    }

    hasher.finish()
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

/// Rewrite empty document paths when an indexer reports facts for a known
/// single-file target without naming the target in `Document.relative_path`.
pub fn replace_empty_scip_document_paths(path: &Path, replacement: &str) -> Result<usize> {
    let replacement = normalize_path_component(replacement);
    if replacement.is_empty() {
        return Ok(0);
    }

    let bytes =
        std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let mut index = Index::parse_from_bytes(&bytes)
        .with_context(|| format!("Failed to parse SCIP index from {}", path.display()))?;

    let mut updated = 0;
    for doc in &mut index.documents {
        if doc.relative_path.is_empty() {
            doc.relative_path = replacement.clone();
            updated += 1;
        }
    }

    if updated == 0 {
        return Ok(0);
    }

    let bytes = index
        .write_to_bytes()
        .context("Failed to serialize empty-path-repaired SCIP index")?;
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

pub(crate) fn normalize_path_component(path: &str) -> String {
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
    use scip::types::{Document, Occurrence, SymbolInformation};
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
        existing.relative_path = "src/main.unknown".into();
        existing.language = "javascriptreact".into();

        index.documents.push(missing);
        index.documents.push(existing);

        let updated = fill_missing_document_languages(&mut index, Some("typescript"));

        assert_eq!(updated, 1);
        assert_eq!(index.documents[0].language, "typescript");
        assert_eq!(index.documents[1].language, "javascriptreact");
    }

    #[test]
    fn canonicalizes_known_existing_language_aliases() {
        let mut index = Index::new();
        let mut doc = Document::new();
        doc.relative_path = "src/Program.cs".into();
        doc.language = "C#".into();
        index.documents.push(doc);

        let updated = fill_missing_document_languages(&mut index, None);

        assert_eq!(updated, 1);
        assert_eq!(index.documents[0].language, "csharp");
    }

    #[test]
    fn path_extension_overrides_wrong_existing_language() {
        let mut index = Index::new();
        let mut kotlin = Document::new();
        kotlin.relative_path = "src/main/kotlin/App.kt".into();
        kotlin.language = "java".into();
        index.documents.push(kotlin);

        let mut scala = Document::new();
        scala.relative_path = "src/main/scala/App.scala".into();
        scala.language = "java".into();
        index.documents.push(scala);

        let updated = fill_missing_document_languages(&mut index, None);

        assert_eq!(updated, 2);
        assert_eq!(index.documents[0].language, "kotlin");
        assert_eq!(index.documents[1].language, "scala");
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
    fn does_not_prefix_already_root_relative_scip_document_paths() -> Result<()> {
        let mut index = Index::new();
        let mut doc = Document::new();
        doc.relative_path = "services/api/src/main.rs".into();
        index.documents.push(doc);

        let file = NamedTempFile::new()?;
        std::fs::write(file.path(), index.write_to_bytes()?)?;

        let updated = prefix_scip_file_document_paths(file.path(), "services/api")?;
        let normalized = Index::parse_from_bytes(&std::fs::read(file.path())?)?;

        assert_eq!(updated, 0);
        assert_eq!(
            normalized.documents[0].relative_path,
            "services/api/src/main.rs"
        );
        Ok(())
    }

    #[test]
    fn replaces_empty_scip_document_paths_with_known_target_path() -> Result<()> {
        let mut index = Index::new();
        let mut doc = Document::new();
        doc.language = "python".into();
        index.documents.push(doc);

        let file = NamedTempFile::new()?;
        std::fs::write(file.path(), index.write_to_bytes()?)?;

        let updated = replace_empty_scip_document_paths(file.path(), "pkg\\a.py")?;
        let normalized = Index::parse_from_bytes(&std::fs::read(file.path())?)?;

        assert_eq!(updated, 1);
        assert_eq!(normalized.documents[0].relative_path, "pkg/a.py");
        assert_eq!(normalized.documents[0].language, "python");
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

    #[test]
    fn compacts_duplicate_documents_occurrences_and_symbols() {
        let mut index = Index::new();

        let mut occurrence = Occurrence::new();
        occurrence.range = vec![1, 2, 1, 6];
        occurrence.symbol = "local 1".into();

        let mut symbol = SymbolInformation::new();
        symbol.symbol = "local 1".into();
        symbol.display_name = "value".into();

        let mut first = Document::new();
        first.relative_path = "src\\module.py".into();
        first.language = "python".into();
        first.occurrences.push(occurrence.clone());
        first.occurrences.push(occurrence.clone());
        first.symbols.push(symbol.clone());
        first.symbols.push(symbol.clone());

        let mut second = Document::new();
        second.relative_path = "src/module.py".into();
        second.occurrences.push(occurrence);
        second.symbols.push(symbol);

        index.documents.push(first);
        index.documents.push(second);

        let stats = compact_index(&mut index);

        assert_eq!(stats.documents_before, 2);
        assert_eq!(stats.documents_after, 1);
        assert_eq!(stats.duplicate_documents, 1);
        assert_eq!(stats.duplicate_occurrences, 2);
        assert_eq!(stats.duplicate_symbols, 2);
        assert_eq!(index.documents[0].relative_path, "src/module.py");
        assert_eq!(index.documents[0].language, "python");
        assert_eq!(index.documents[0].occurrences.len(), 1);
        assert_eq!(index.documents[0].symbols.len(), 1);
    }

    #[test]
    fn compacts_scip_file_in_place() -> Result<()> {
        let mut index = Index::new();

        let mut first = Document::new();
        first.relative_path = "src/lib.py".into();
        index.documents.push(first);

        let mut second = Document::new();
        second.relative_path = "./src/lib.py".into();
        index.documents.push(second);

        let file = NamedTempFile::new()?;
        std::fs::write(file.path(), index.write_to_bytes()?)?;

        let stats = compact_scip_file(file.path())?;
        let compacted = Index::parse_from_bytes(&std::fs::read(file.path())?)?;

        assert_eq!(stats.duplicate_documents, 1);
        assert_eq!(compacted.documents.len(), 1);
        assert_eq!(compacted.documents[0].relative_path, "src/lib.py");

        Ok(())
    }

    #[test]
    fn compact_scip_file_persists_path_normalization_without_duplicates() -> Result<()> {
        let mut index = Index::new();

        let mut doc = Document::new();
        doc.relative_path = ".\\src\\lib.py".into();
        index.documents.push(doc);

        let file = NamedTempFile::new()?;
        std::fs::write(file.path(), index.write_to_bytes()?)?;

        let stats = compact_scip_file(file.path())?;
        let compacted = Index::parse_from_bytes(&std::fs::read(file.path())?)?;

        assert_eq!(stats.normalized_paths, 1);
        assert_eq!(stats.duplicate_documents, 0);
        assert_eq!(compacted.documents[0].relative_path, "src/lib.py");

        Ok(())
    }
}
