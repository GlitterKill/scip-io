use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, Result};
use protobuf::Message;
use scip::types::{Document, Index, Metadata, SymbolInformation, TextEncoding, ToolInfo};

use crate::scip_language::{
    ScipPublishStats, compact_index, compact_validate_publish_scip_file,
    fill_missing_document_languages,
};

/// Merge multiple SCIP index files into a staged file, validate it, and publish
/// only after all postprocessing succeeds.
pub fn merge_scip_files_atomically(
    inputs: &[impl AsRef<Path>],
    output: &Path,
) -> Result<ScipPublishStats> {
    merge_scip_files_atomically_with_options(inputs, output, MergeScipOptions::default())
}

/// Merge multiple SCIP index files, using `project_root` as the authoritative
/// root for the final artifact, then publish only after postprocessing succeeds.
pub fn merge_scip_files_atomically_with_project_root(
    inputs: &[impl AsRef<Path>],
    output: &Path,
    project_root: &Path,
) -> Result<ScipPublishStats> {
    merge_scip_files_atomically_with_options(
        inputs,
        output,
        MergeScipOptions {
            project_root: Some(project_root),
        },
    )
}

fn merge_scip_files_atomically_with_options(
    inputs: &[impl AsRef<Path>],
    output: &Path,
    options: MergeScipOptions<'_>,
) -> Result<ScipPublishStats> {
    let temp_dir = tempfile::Builder::new()
        .prefix("scip-io-merge-")
        .tempdir()
        .context("Failed to create temporary directory for SCIP merge")?;
    let staged = temp_dir.path().join(
        output
            .file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("index.scip")),
    );
    merge_scip_files_with_options(inputs, &staged, options)?;
    compact_validate_publish_scip_file(&staged, output)
}

/// Merge multiple SCIP index files into a single output file.
pub fn merge_scip_files(inputs: &[impl AsRef<Path>], output: &Path) -> Result<()> {
    merge_scip_files_with_options(inputs, output, MergeScipOptions::default())
}

/// Merge multiple SCIP index files into a single output file, using
/// `project_root` as the authoritative root for the final artifact.
pub fn merge_scip_files_with_project_root(
    inputs: &[impl AsRef<Path>],
    output: &Path,
    project_root: &Path,
) -> Result<()> {
    merge_scip_files_with_options(
        inputs,
        output,
        MergeScipOptions {
            project_root: Some(project_root),
        },
    )
}

#[derive(Clone, Copy, Default)]
struct MergeScipOptions<'a> {
    project_root: Option<&'a Path>,
}

fn merge_scip_files_with_options(
    inputs: &[impl AsRef<Path>],
    output: &Path,
    options: MergeScipOptions<'_>,
) -> Result<()> {
    let mut merged = Index::new();

    // Build merged metadata
    let mut tool_info = ToolInfo::new();
    tool_info.name = "scip-io".into();
    tool_info.version = env!("CARGO_PKG_VERSION").into();

    let mut metadata = Metadata::new();
    metadata.tool_info = Some(tool_info).into();
    let mut metadata_selection = MergeMetadataSelection::default();

    // Track documents by relative_path to handle overlapping files
    let mut doc_map: HashMap<String, Document> = HashMap::new();
    let mut external_symbols: BTreeMap<String, SymbolInformation> = BTreeMap::new();

    for input_path in inputs {
        let input_path = input_path.as_ref();
        tracing::info!(path = %input_path.display(), "reading SCIP index");

        let bytes = std::fs::read(input_path)
            .with_context(|| format!("Failed to read {}", input_path.display()))?;

        let index = Index::parse_from_bytes(&bytes)
            .with_context(|| format!("Failed to parse SCIP index from {}", input_path.display()))?;
        let mut index = index;
        if let Some(input_metadata) = index.metadata.as_ref() {
            metadata_selection.observe(input_metadata, input_path);
        }

        // Older or third-party indexers can omit Document.language. Repair it
        // before merging so the combined file has complete per-document metadata.
        let updated_languages = fill_missing_document_languages(&mut index, None);
        if updated_languages > 0 {
            tracing::info!(
                path = %input_path.display(),
                docs = updated_languages,
                "filled missing SCIP document languages before merge"
            );
        }

        for doc in index.documents {
            let key = doc.relative_path.clone();
            if let Some(existing) = doc_map.get_mut(&key) {
                // Merge occurrences and symbols into existing document
                merge_document(existing, doc);
            } else {
                doc_map.insert(key, doc);
            }
        }

        for symbol in index.external_symbols {
            external_symbols
                .entry(symbol.symbol.clone())
                .or_insert(symbol);
        }
    }

    // Collect documents sorted by path for deterministic output
    let mut documents: Vec<Document> = doc_map.into_values().collect();
    documents.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    merged.documents = documents;
    merged.external_symbols = external_symbols.into_values().collect();
    metadata_selection.apply_to(&mut metadata);
    if let Some(project_root) = options.project_root {
        metadata.project_root = project_root_to_scip_uri(project_root);
    }
    merged.metadata = Some(metadata).into();
    let compaction = compact_index(&mut merged);
    if compaction.changed() {
        tracing::info!(
            duplicate_documents = compaction.duplicate_documents,
            duplicate_occurrences = compaction.duplicate_occurrences,
            duplicate_symbols = compaction.duplicate_symbols,
            "compacted duplicate SCIP facts before writing merged index"
        );
    }

    // Write output
    let bytes = merged
        .write_to_bytes()
        .context("Failed to serialize merged SCIP index")?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, bytes)
        .with_context(|| format!("Failed to write {}", output.display()))?;

    tracing::info!(
        path = %output.display(),
        docs = merged.documents.len(),
        "wrote merged index"
    );

    Ok(())
}

fn project_root_to_scip_uri(project_root: &Path) -> String {
    let mut root = project_root.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = root.strip_prefix("//?/UNC/") {
        root = format!("//{stripped}");
    } else if let Some(stripped) = root.strip_prefix("//?/") {
        root = stripped.to_string();
    }

    if let Some(unc_root) = root.strip_prefix("//") {
        return format!("file://{}", percent_encode_path(unc_root));
    }

    let prefix = if root.starts_with('/') {
        "file://"
    } else {
        "file:///"
    };
    format!("{prefix}{}", percent_encode_path(&root))
}

fn percent_encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for byte in path.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                encoded.push(*byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

#[derive(Default)]
struct MergeMetadataSelection {
    project_root: Option<String>,
    project_root_conflicted: bool,
    text_document_encoding: Option<protobuf::EnumOrUnknown<TextEncoding>>,
    text_document_encoding_conflicted: bool,
}

impl MergeMetadataSelection {
    fn observe(&mut self, source: &Metadata, input_path: &Path) {
        self.observe_project_root(source, input_path);
        self.observe_text_document_encoding(source, input_path);
    }

    fn observe_project_root(&mut self, source: &Metadata, input_path: &Path) {
        let source_project_root = source.project_root.trim();
        if source_project_root.is_empty() || self.project_root_conflicted {
            return;
        }

        let Some(target_project_root) = self.project_root.as_ref() else {
            self.project_root = Some(source_project_root.to_string());
            return;
        };

        if target_project_root != source_project_root {
            tracing::warn!(
                path = %input_path.display(),
                target = %target_project_root,
                source = %source.project_root,
                "conflicting SCIP project roots; leaving merged project root empty"
            );
            self.project_root = None;
            self.project_root_conflicted = true;
        }
    }

    fn observe_text_document_encoding(&mut self, source: &Metadata, input_path: &Path) {
        let unspecified = protobuf::EnumOrUnknown::new(TextEncoding::UnspecifiedTextEncoding);
        let source_encoding = source.text_document_encoding;
        if source_encoding == unspecified || self.text_document_encoding_conflicted {
            return;
        }

        let Some(target_encoding) = self.text_document_encoding else {
            self.text_document_encoding = Some(source_encoding);
            return;
        };

        if target_encoding != source_encoding {
            tracing::warn!(
                path = %input_path.display(),
                target = target_encoding.value(),
                source = source_encoding.value(),
                "conflicting SCIP text document encodings; leaving merged encoding unspecified"
            );
            self.text_document_encoding = None;
            self.text_document_encoding_conflicted = true;
        }
    }

    fn apply_to(self, metadata: &mut Metadata) {
        if let Some(project_root) = self.project_root {
            metadata.project_root = project_root;
        }

        if let Some(text_document_encoding) = self.text_document_encoding {
            metadata.text_document_encoding = text_document_encoding;
        }
    }
}

/// Merge source document's occurrences and symbols into target.
fn merge_document(target: &mut Document, source: Document) {
    let target_language = target.language.trim();
    let source_language = source.language.trim();
    if target_language.is_empty() && !source_language.is_empty() {
        target.language = source_language.to_string();
    } else if !source_language.is_empty() && target_language != source_language {
        tracing::warn!(
            path = %target.relative_path,
            target = %target.language,
            source = %source.language,
            "conflicting SCIP document languages; keeping first document language"
        );
    }

    // Append occurrences (they reference line/col positions, so duplicates are harmless)
    target.occurrences.extend(source.occurrences);

    // Merge symbol information, avoiding duplicates by symbol name
    let existing_symbols: std::collections::HashSet<String> =
        target.symbols.iter().map(|s| s.symbol.clone()).collect();

    for sym in source.symbols {
        if !existing_symbols.contains(&sym.symbol) {
            target.symbols.push(sym);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scip::types::{SymbolInformation, TextEncoding};
    use tempfile::TempDir;

    #[test]
    fn merge_two_indices() -> Result<()> {
        let dir = TempDir::new()?;

        // Create two minimal SCIP indices
        let mut idx1 = Index::new();
        let mut doc1 = Document::new();
        doc1.relative_path = "src/main.rs".into();
        doc1.language = "rust".into();
        idx1.documents.push(doc1);

        let mut idx2 = Index::new();
        let mut doc2 = Document::new();
        doc2.relative_path = "src/lib.ts".into();
        doc2.language = "typescript".into();
        idx2.documents.push(doc2);

        let path1 = dir.path().join("idx1.scip");
        let path2 = dir.path().join("idx2.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path1, idx1.write_to_bytes()?)?;
        std::fs::write(&path2, idx2.write_to_bytes()?)?;

        merge_scip_files(&[&path1, &path2], &out)?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        assert_eq!(merged.documents.len(), 2);
        assert_eq!(merged.documents[0].relative_path, "src/lib.ts");
        assert_eq!(merged.documents[1].relative_path, "src/main.rs");

        Ok(())
    }

    #[test]
    fn merge_preserves_external_symbols() -> Result<()> {
        let dir = TempDir::new()?;

        let mut idx1 = Index::new();
        let mut symbol1 = SymbolInformation::new();
        symbol1.symbol = "cpp std:: vector#".into();
        idx1.external_symbols.push(symbol1);

        let mut idx2 = Index::new();
        let mut duplicate = SymbolInformation::new();
        duplicate.symbol = "cpp std:: vector#".into();
        idx2.external_symbols.push(duplicate);
        let mut symbol2 = SymbolInformation::new();
        symbol2.symbol = "cpp llvm:: StringRef#".into();
        idx2.external_symbols.push(symbol2);

        let path1 = dir.path().join("cpp.scip");
        let path2 = dir.path().join("typescript.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path1, idx1.write_to_bytes()?)?;
        std::fs::write(&path2, idx2.write_to_bytes()?)?;

        merge_scip_files(&[&path1, &path2], &out)?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        let symbols = merged
            .external_symbols
            .iter()
            .map(|symbol| symbol.symbol.as_str())
            .collect::<Vec<_>>();
        assert_eq!(symbols, vec!["cpp llvm:: StringRef#", "cpp std:: vector#"]);

        Ok(())
    }

    #[test]
    fn merge_preserves_input_project_root_and_encoding() -> Result<()> {
        let dir = TempDir::new()?;

        let mut idx = Index::new();
        let mut metadata = Metadata::new();
        metadata.project_root = "file:///repo/root".into();
        metadata.text_document_encoding = protobuf::EnumOrUnknown::new(TextEncoding::UTF8);
        idx.metadata = Some(metadata).into();

        let mut doc = Document::new();
        doc.relative_path = "src/lib.ts".into();
        doc.language = "typescript".into();
        idx.documents.push(doc);

        let path = dir.path().join("typescript.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path, idx.write_to_bytes()?)?;

        merge_scip_files(&[&path], &out)?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        let metadata = merged.metadata.into_option().unwrap();
        assert_eq!(metadata.project_root, "file:///repo/root");
        assert_eq!(
            metadata.text_document_encoding,
            protobuf::EnumOrUnknown::new(TextEncoding::UTF8)
        );
        assert_eq!(metadata.tool_info.into_option().unwrap().name, "scip-io");

        Ok(())
    }

    #[test]
    fn merge_uses_later_metadata_when_earlier_inputs_are_empty() -> Result<()> {
        let dir = TempDir::new()?;

        let mut idx1 = Index::new();
        let mut doc1 = Document::new();
        doc1.relative_path = "src/main.rs".into();
        doc1.language = "rust".into();
        idx1.documents.push(doc1);

        let mut idx2 = Index::new();
        let mut metadata = Metadata::new();
        metadata.project_root = "file:///repo/root".into();
        metadata.text_document_encoding = protobuf::EnumOrUnknown::new(TextEncoding::UTF8);
        idx2.metadata = Some(metadata).into();
        let mut doc2 = Document::new();
        doc2.relative_path = "src/lib.ts".into();
        doc2.language = "typescript".into();
        idx2.documents.push(doc2);

        let path1 = dir.path().join("rust.scip");
        let path2 = dir.path().join("typescript.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path1, idx1.write_to_bytes()?)?;
        std::fs::write(&path2, idx2.write_to_bytes()?)?;

        merge_scip_files(&[&path1, &path2], &out)?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        let metadata = merged.metadata.into_option().unwrap();
        assert_eq!(metadata.project_root, "file:///repo/root");
        assert_eq!(
            metadata.text_document_encoding,
            protobuf::EnumOrUnknown::new(TextEncoding::UTF8)
        );

        Ok(())
    }

    #[test]
    fn merge_drops_ambiguous_project_root_and_encoding() -> Result<()> {
        let dir = TempDir::new()?;

        let mut idx1 = Index::new();
        let mut metadata1 = Metadata::new();
        metadata1.project_root = "file:///repo/packages/a".into();
        metadata1.text_document_encoding = protobuf::EnumOrUnknown::new(TextEncoding::UTF8);
        idx1.metadata = Some(metadata1).into();
        let mut doc1 = Document::new();
        doc1.relative_path = "packages/a/src/lib.ts".into();
        doc1.language = "typescript".into();
        idx1.documents.push(doc1);

        let mut idx2 = Index::new();
        let mut metadata2 = Metadata::new();
        metadata2.project_root = "file:///repo/packages/b".into();
        metadata2.text_document_encoding = protobuf::EnumOrUnknown::new(TextEncoding::UTF16);
        idx2.metadata = Some(metadata2).into();
        let mut doc2 = Document::new();
        doc2.relative_path = "packages/b/src/lib.ts".into();
        doc2.language = "typescript".into();
        idx2.documents.push(doc2);

        let path1 = dir.path().join("package-a.scip");
        let path2 = dir.path().join("package-b.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path1, idx1.write_to_bytes()?)?;
        std::fs::write(&path2, idx2.write_to_bytes()?)?;

        merge_scip_files(&[&path1, &path2], &out)?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        let metadata = merged.metadata.into_option().unwrap();
        assert_eq!(metadata.project_root, "");
        assert_eq!(
            metadata.text_document_encoding,
            protobuf::EnumOrUnknown::new(TextEncoding::UnspecifiedTextEncoding)
        );
        assert_eq!(metadata.tool_info.into_option().unwrap().name, "scip-io");

        Ok(())
    }

    #[test]
    fn merge_with_project_root_uses_authoritative_root() -> Result<()> {
        let dir = TempDir::new()?;

        let mut idx1 = Index::new();
        let mut metadata1 = Metadata::new();
        metadata1.project_root = "file:///repo/packages/a".into();
        idx1.metadata = Some(metadata1).into();
        let mut doc1 = Document::new();
        doc1.relative_path = "packages/a/src/lib.ts".into();
        doc1.language = "typescript".into();
        idx1.documents.push(doc1);

        let mut idx2 = Index::new();
        let mut metadata2 = Metadata::new();
        metadata2.project_root = "file:///repo/packages/b".into();
        idx2.metadata = Some(metadata2).into();
        let mut doc2 = Document::new();
        doc2.relative_path = "packages/b/src/lib.ts".into();
        doc2.language = "typescript".into();
        idx2.documents.push(doc2);

        let path1 = dir.path().join("package-a.scip");
        let path2 = dir.path().join("package-b.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path1, idx1.write_to_bytes()?)?;
        std::fs::write(&path2, idx2.write_to_bytes()?)?;

        merge_scip_files_with_project_root(&[&path1, &path2], &out, dir.path())?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        let metadata = merged.metadata.into_option().unwrap();
        let root = dir.path().to_string_lossy().replace('\\', "/");
        let expected_root = if root.starts_with('/') {
            format!("file://{root}")
        } else {
            format!("file:///{root}")
        };
        assert_eq!(metadata.project_root, expected_root);

        Ok(())
    }

    #[test]
    fn merge_with_project_root_uri_encodes_authoritative_root() -> Result<()> {
        let dir = TempDir::new()?;

        let mut idx = Index::new();
        let mut doc = Document::new();
        doc.relative_path = "src/lib.ts".into();
        doc.language = "typescript".into();
        idx.documents.push(doc);

        let path = dir.path().join("typescript.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path, idx.write_to_bytes()?)?;

        merge_scip_files_with_project_root(&[&path], &out, Path::new("C:\\repo root#1\\project"))?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        let metadata = merged.metadata.into_option().unwrap();
        assert_eq!(metadata.project_root, "file:///C:/repo%20root%231/project");

        Ok(())
    }

    #[test]
    fn project_root_uri_formats_unc_roots_with_server_authority() {
        assert_eq!(
            project_root_to_scip_uri(Path::new("\\\\server\\share root\\repo#1")),
            "file://server/share%20root/repo%231"
        );
    }

    #[test]
    fn project_root_uri_formats_extended_unc_roots_with_server_authority() {
        assert_eq!(
            project_root_to_scip_uri(Path::new("\\\\?\\UNC\\server\\share\\repo")),
            "file://server/share/repo"
        );
    }

    #[test]
    fn merge_overlapping_documents() -> Result<()> {
        let dir = TempDir::new()?;

        let mut idx1 = Index::new();
        let mut doc1 = Document::new();
        doc1.relative_path = "src/shared.rs".into();
        doc1.language = "rust".into();
        idx1.documents.push(doc1);

        let mut idx2 = Index::new();
        let mut doc2 = Document::new();
        doc2.relative_path = "src/shared.rs".into();
        doc2.language = "rust".into();
        idx2.documents.push(doc2);

        let path1 = dir.path().join("idx1.scip");
        let path2 = dir.path().join("idx2.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path1, idx1.write_to_bytes()?)?;
        std::fs::write(&path2, idx2.write_to_bytes()?)?;

        merge_scip_files(&[&path1, &path2], &out)?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        assert_eq!(merged.documents.len(), 1);

        Ok(())
    }

    #[test]
    fn merge_fills_missing_document_languages() -> Result<()> {
        let dir = TempDir::new()?;

        let mut idx = Index::new();
        let mut doc = Document::new();
        doc.relative_path = "src/lib.ts".into();
        idx.documents.push(doc);

        let path = dir.path().join("typescript.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path, idx.write_to_bytes()?)?;

        merge_scip_files(&[&path], &out)?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        assert_eq!(merged.documents[0].language, "typescript");

        Ok(())
    }

    #[test]
    fn merge_does_not_infer_language_from_input_filename() -> Result<()> {
        let dir = TempDir::new()?;

        let mut idx = Index::new();
        let mut doc = Document::new();
        doc.relative_path = "Makefile".into();
        idx.documents.push(doc);

        let path = dir.path().join("cpp.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path, idx.write_to_bytes()?)?;

        merge_scip_files(&[&path], &out)?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        assert_eq!(merged.documents[0].language, "");

        Ok(())
    }

    #[test]
    fn merge_preserves_language_from_duplicate_document() -> Result<()> {
        let dir = TempDir::new()?;

        let mut idx1 = Index::new();
        let mut doc1 = Document::new();
        doc1.relative_path = "Dockerfile".into();
        idx1.documents.push(doc1);

        let mut idx2 = Index::new();
        let mut doc2 = Document::new();
        doc2.relative_path = "Dockerfile".into();
        doc2.language = "dockerfile".into();
        idx2.documents.push(doc2);

        let path1 = dir.path().join("idx1.scip");
        let path2 = dir.path().join("idx2.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path1, idx1.write_to_bytes()?)?;
        std::fs::write(&path2, idx2.write_to_bytes()?)?;

        merge_scip_files(&[&path1, &path2], &out)?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        assert_eq!(merged.documents[0].language, "dockerfile");

        Ok(())
    }

    #[test]
    fn merge_keeps_first_language_for_conflicting_duplicate_documents() -> Result<()> {
        let dir = TempDir::new()?;

        let mut idx1 = Index::new();
        let mut doc1 = Document::new();
        doc1.relative_path = "src/shared.ts".into();
        doc1.language = "typescript".into();
        idx1.documents.push(doc1);

        let mut idx2 = Index::new();
        let mut doc2 = Document::new();
        doc2.relative_path = "src/shared.ts".into();
        doc2.language = "javascript".into();
        idx2.documents.push(doc2);

        let path1 = dir.path().join("idx1.scip");
        let path2 = dir.path().join("idx2.scip");
        let out = dir.path().join("merged.scip");

        std::fs::write(&path1, idx1.write_to_bytes()?)?;
        std::fs::write(&path2, idx2.write_to_bytes()?)?;

        merge_scip_files(&[&path1, &path2], &out)?;

        let merged = Index::parse_from_bytes(&std::fs::read(&out)?)?;
        assert_eq!(merged.documents[0].language, "typescript");

        Ok(())
    }
}
