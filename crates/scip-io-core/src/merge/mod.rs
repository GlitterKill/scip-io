use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use protobuf::Message;
use scip::types::{Document, Index, Metadata, ToolInfo};

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

    // Write output
    let bytes = merged.write_to_bytes().context("Failed to serialize merged SCIP index")?;
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
    // Append occurrences (they reference line/col positions, so duplicates are harmless)
    target.occurrences.extend(source.occurrences);

    // Merge symbol information, avoiding duplicates by symbol name
    let existing_symbols: std::collections::HashSet<String> = target
        .symbols
        .iter()
        .map(|s| s.symbol.clone())
        .collect();

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
}
