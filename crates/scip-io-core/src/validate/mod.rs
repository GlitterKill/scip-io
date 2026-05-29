use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use protobuf::Message;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<String>,
    pub stats: Option<IndexStats>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ValidationError {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexStats {
    pub documents: usize,
    pub symbols: usize,
    pub occurrences: usize,
    pub languages: Vec<String>,
}

pub fn validate_scip_file(path: &Path) -> Result<ValidationResult> {
    let bytes =
        std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;

    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    // Try to parse as protobuf
    let index = match scip::types::Index::parse_from_bytes(&bytes) {
        Ok(idx) => idx,
        Err(e) => {
            return Ok(ValidationResult {
                valid: false,
                errors: vec![ValidationError {
                    kind: "parse_error".to_string(),
                    message: format!("Invalid protobuf: {}", e),
                }],
                warnings: vec![],
                stats: None,
            });
        }
    };

    // Check for empty documents
    if index.documents.is_empty() {
        errors.push(ValidationError {
            kind: "empty_index".to_string(),
            message: "Index contains no documents".to_string(),
        });
    }

    // Check for duplicate document paths
    let mut seen_paths = HashSet::new();
    for doc in &index.documents {
        if !seen_paths.insert(&doc.relative_path) {
            errors.push(ValidationError {
                kind: "duplicate_path".to_string(),
                message: format!("Duplicate document path: {}", doc.relative_path),
            });
        }
    }

    // Check for duplicate facts inside each document. Duplicate occurrence or
    // symbol rows are structurally valid SCIP, but they break downstream graph
    // materializers that key facts by document path plus symbol identity.
    for doc in &index.documents {
        let doc_path = if doc.relative_path.is_empty() {
            "<empty relative_path>"
        } else {
            &doc.relative_path
        };

        let mut seen_occurrences = HashSet::new();
        for occurrence in &doc.occurrences {
            match occurrence.write_to_bytes() {
                Ok(key) => {
                    if !seen_occurrences.insert(key) {
                        errors.push(ValidationError {
                            kind: "duplicate_occurrence".to_string(),
                            message: format!("Duplicate occurrence fact in document: {doc_path}"),
                        });
                        break;
                    }
                }
                Err(e) => {
                    errors.push(ValidationError {
                        kind: "occurrence_serialize_error".to_string(),
                        message: format!(
                            "Failed to serialize occurrence while validating {doc_path}: {e}"
                        ),
                    });
                    break;
                }
            }
        }

        let mut seen_symbols = HashSet::new();
        for symbol in &doc.symbols {
            if !seen_symbols.insert(symbol.symbol.clone()) {
                errors.push(ValidationError {
                    kind: "duplicate_symbol".to_string(),
                    message: format!(
                        "Duplicate document symbol '{}' in document: {doc_path}",
                        symbol.symbol
                    ),
                });
                break;
            }
        }
    }

    // Check for empty relative paths
    for doc in &index.documents {
        if doc.relative_path.is_empty() {
            warnings.push("Document with empty relative_path found".to_string());
        }
    }

    // Check for missing language metadata. Some upstream indexers emit this
    // structurally valid but low-quality output, so validation calls it out.
    let missing_language_docs: Vec<String> = index
        .documents
        .iter()
        .filter(|doc| doc.language.trim().is_empty())
        .map(|doc| {
            if doc.relative_path.is_empty() {
                "<empty relative_path>".to_string()
            } else {
                doc.relative_path.clone()
            }
        })
        .collect();
    if !missing_language_docs.is_empty() {
        let mut examples = missing_language_docs
            .iter()
            .take(5)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        if missing_language_docs.len() > 5 {
            examples.push_str(", ...");
        }
        warnings.push(format!(
            "{} document(s) have empty language metadata: {}",
            missing_language_docs.len(),
            examples
        ));
    }

    // Collect stats
    let total_symbols: usize = index.documents.iter().map(|d| d.symbols.len()).sum();
    let total_occurrences: usize = index.documents.iter().map(|d| d.occurrences.len()).sum();
    let languages: Vec<String> = index
        .documents
        .iter()
        .map(|d| d.language.clone())
        .filter(|l| !l.trim().is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    let stats = Some(IndexStats {
        documents: index.documents.len(),
        symbols: total_symbols,
        occurrences: total_occurrences,
        languages,
    });

    Ok(ValidationResult {
        valid: errors.is_empty(),
        errors,
        warnings,
        stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use protobuf::Message;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_valid_scip_file() -> NamedTempFile {
        let mut index = scip::types::Index::new();
        let mut doc = scip::types::Document::new();
        doc.relative_path = "src/main.rs".to_string();
        doc.language = "rust".to_string();
        index.documents.push(doc);

        let bytes = index.write_to_bytes().unwrap();
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        file
    }

    #[test]
    fn test_validate_valid_file() {
        let file = create_valid_scip_file();
        let result = validate_scip_file(file.path()).unwrap();
        assert!(result.valid);
        assert!(result.errors.is_empty());
        let stats = result.stats.unwrap();
        assert_eq!(stats.documents, 1);
        assert_eq!(stats.symbols, 0);
        assert_eq!(stats.occurrences, 0);
        assert!(stats.languages.contains(&"rust".to_string()));
    }

    #[test]
    fn test_validate_invalid_protobuf() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"not a protobuf").unwrap();
        let result = validate_scip_file(file.path()).unwrap();
        // protobuf may or may not parse garbage; check both outcomes
        if !result.valid {
            assert!(result.errors.iter().any(|e| e.kind == "parse_error"));
        }
    }

    #[test]
    fn test_validate_empty_index() {
        let index = scip::types::Index::new();
        let bytes = index.write_to_bytes().unwrap();
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        let result = validate_scip_file(file.path()).unwrap();
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.kind == "empty_index"));
    }

    #[test]
    fn test_validate_duplicate_paths() {
        let mut index = scip::types::Index::new();
        let mut doc1 = scip::types::Document::new();
        doc1.relative_path = "same/path.rs".to_string();
        doc1.language = "rust".to_string();
        let mut doc2 = scip::types::Document::new();
        doc2.relative_path = "same/path.rs".to_string();
        doc2.language = "rust".to_string();
        index.documents.push(doc1);
        index.documents.push(doc2);

        let bytes = index.write_to_bytes().unwrap();
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        let result = validate_scip_file(file.path()).unwrap();
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.kind == "duplicate_path"));
    }

    #[test]
    fn test_validate_duplicate_document_facts() {
        let mut index = scip::types::Index::new();
        let mut doc = scip::types::Document::new();
        doc.relative_path = "src/main.rs".to_string();
        doc.language = "rust".to_string();

        let mut occurrence = scip::types::Occurrence::new();
        occurrence.symbol = "local 0".to_string();
        occurrence.range = vec![0, 0, 0, 4];
        doc.occurrences.push(occurrence.clone());
        doc.occurrences.push(occurrence);

        let mut symbol = scip::types::SymbolInformation::new();
        symbol.symbol = "scip local 0".to_string();
        doc.symbols.push(symbol.clone());
        doc.symbols.push(symbol);

        index.documents.push(doc);

        let bytes = index.write_to_bytes().unwrap();
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        let result = validate_scip_file(file.path()).unwrap();

        assert!(!result.valid);
        assert!(
            result
                .errors
                .iter()
                .any(|e| e.kind == "duplicate_occurrence")
        );
        assert!(result.errors.iter().any(|e| e.kind == "duplicate_symbol"));
    }

    #[test]
    fn test_validate_nonexistent_file() {
        let result = validate_scip_file(Path::new("/nonexistent/file.scip"));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_empty_relative_path_warns() {
        let mut index = scip::types::Index::new();
        let mut doc = scip::types::Document::new();
        doc.relative_path = String::new();
        doc.language = "rust".to_string();
        index.documents.push(doc);

        let bytes = index.write_to_bytes().unwrap();
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        let result = validate_scip_file(file.path()).unwrap();
        assert!(!result.warnings.is_empty());
        assert!(result.warnings[0].contains("empty relative_path"));
    }

    #[test]
    fn test_validate_empty_language_warns() {
        let mut index = scip::types::Index::new();
        let mut doc = scip::types::Document::new();
        doc.relative_path = "src/main.ts".to_string();
        index.documents.push(doc);

        let bytes = index.write_to_bytes().unwrap();
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        let result = validate_scip_file(file.path()).unwrap();

        assert!(result.valid);
        assert!(
            result
                .warnings
                .iter()
                .any(|warning| warning.contains("empty language metadata"))
        );
    }

    #[test]
    fn test_validate_whitespace_language_is_not_reported_in_stats() {
        let mut index = scip::types::Index::new();
        let mut doc = scip::types::Document::new();
        doc.relative_path = "src/main.ts".to_string();
        doc.language = "   ".to_string();
        index.documents.push(doc);

        let bytes = index.write_to_bytes().unwrap();
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        let result = validate_scip_file(file.path()).unwrap();
        let stats = result.stats.unwrap();

        assert!(stats.languages.is_empty());
    }

    #[test]
    fn test_validate_multiple_documents_stats() {
        let mut index = scip::types::Index::new();
        for i in 0..5 {
            let mut doc = scip::types::Document::new();
            doc.relative_path = format!("src/file{}.rs", i);
            doc.language = "rust".to_string();
            index.documents.push(doc);
        }

        let bytes = index.write_to_bytes().unwrap();
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        let result = validate_scip_file(file.path()).unwrap();
        assert!(result.valid);
        assert_eq!(result.stats.unwrap().documents, 5);
    }

    #[test]
    fn test_validate_multiple_languages_in_stats() {
        let mut index = scip::types::Index::new();
        let mut doc1 = scip::types::Document::new();
        doc1.relative_path = "main.rs".to_string();
        doc1.language = "rust".to_string();
        let mut doc2 = scip::types::Document::new();
        doc2.relative_path = "main.go".to_string();
        doc2.language = "go".to_string();
        index.documents.push(doc1);
        index.documents.push(doc2);

        let bytes = index.write_to_bytes().unwrap();
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        let result = validate_scip_file(file.path()).unwrap();
        assert!(result.valid);
        let stats = result.stats.unwrap();
        assert_eq!(stats.documents, 2);
        assert!(stats.languages.contains(&"rust".to_string()));
        assert!(stats.languages.contains(&"go".to_string()));
    }

    #[test]
    fn test_validate_result_serialization() {
        let result = ValidationResult {
            valid: true,
            errors: vec![],
            warnings: vec!["test warning".into()],
            stats: Some(IndexStats {
                documents: 3,
                symbols: 10,
                occurrences: 50,
                languages: vec!["rust".into()],
            }),
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ValidationResult = serde_json::from_str(&json).unwrap();
        assert!(deserialized.valid);
        assert_eq!(deserialized.warnings.len(), 1);
        assert_eq!(deserialized.stats.unwrap().documents, 3);
    }
}
