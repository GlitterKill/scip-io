use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use protobuf::Message;
use scip::types::{Document, Index, Metadata, ToolInfo};

use crate::scip_language::{compact_index, fill_missing_document_languages};

/// Merge multiple SCIP index files into a single output file.
pub fn merge_scip_files(inputs: &[impl AsRef<Path>], output: &Path) -> Result<()> {
    let mut merged = Index::new();

    // Build merged metadata
    let mut tool_info = ToolInfo::new();
    tool_info.name = "scip-io".into();
    tool_info.version = env!("CARGO_PKG_VERSION").into();

    let mut metadata = Metadata::new();
    metadata.tool_info = Some(tool_info).into();
    merged.metadata = Some(metadata).into();

    // Track documents by relative_path to handle overlapping files
    let mut doc_map: HashMap<String, Document> = HashMap::new();

    for input_path in inputs {
        let input_path = input_path.as_ref();
        tracing::info!(path = %input_path.display(), "reading SCIP index");

        let bytes = std::fs::read(input_path)
            .with_context(|| format!("Failed to read {}", input_path.display()))?;

        let index = Index::parse_from_bytes(&bytes)
            .with_context(|| format!("Failed to parse SCIP index from {}", input_path.display()))?;
        let mut index = index;

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
    }

    // Collect documents sorted by path for deterministic output
    let mut documents: Vec<Document> = doc_map.into_values().collect();
    documents.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    merged.documents = documents;
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
