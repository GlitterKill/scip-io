use std::sync::LazyLock;

use crate::detect::Language;
use crate::indexer::{IndexerEntry, InstallMethod};

/// Global registry of known SCIP indexers.
pub static REGISTRY: LazyLock<Registry> = LazyLock::new(Registry::default_registry);

pub struct Registry {
    entries: Vec<IndexerEntry>,
}

impl Registry {
    fn default_registry() -> Self {
        Self {
            entries: vec![
                IndexerEntry {
                    indexer_name: "scip-typescript".into(),
                    language: "typescript".into(),
                    github_repo: "sourcegraph/scip-typescript".into(),
                    binary_name: "scip-typescript".into(),
                    version: "0.4.0".into(),
                    default_args: vec!["index".into()],
                    output_file: "index.scip".into(),
                    install_method: InstallMethod::Npm {
                        package: "@sourcegraph/scip-typescript".into(),
                    },
                },
                IndexerEntry {
                    indexer_name: "scip-typescript".into(),
                    language: "javascript".into(),
                    github_repo: "sourcegraph/scip-typescript".into(),
                    binary_name: "scip-typescript".into(),
                    version: "0.4.0".into(),
                    default_args: vec!["index".into(), "--infer-tsconfig".into()],
                    output_file: "index.scip".into(),
                    install_method: InstallMethod::Npm {
                        package: "@sourcegraph/scip-typescript".into(),
                    },
                },
                IndexerEntry {
                    indexer_name: "scip-python".into(),
                    language: "python".into(),
                    github_repo: "sourcegraph/scip-python".into(),
                    binary_name: "scip-python".into(),
                    version: "0.6.6".into(),
                    default_args: vec!["index".into(), ".".into()],
                    output_file: "index.scip".into(),
                    install_method: InstallMethod::Npm {
                        package: "@sourcegraph/scip-python".into(),
                    },
                },
                IndexerEntry {
                    indexer_name: "rust-analyzer".into(),
                    language: "rust".into(),
                    github_repo: "rust-lang/rust-analyzer".into(),
                    binary_name: "rust-analyzer".into(),
                    version: "2026-03-30".into(),
                    default_args: vec!["scip".into(), ".".into()],
                    output_file: "index.scip".into(),
                    install_method: if cfg!(windows) {
                        InstallMethod::GitHubZip {
                            asset_pattern: "rust-analyzer-{target_triple}.zip".into(),
                            binary_path_in_archive: Some("rust-analyzer.exe".into()),
                        }
                    } else {
                        InstallMethod::GitHubGz {
                            asset_pattern: "rust-analyzer-{target_triple}.gz".into(),
                        }
                    },
                },
                IndexerEntry {
                    indexer_name: "scip-go".into(),
                    language: "go".into(),
                    github_repo: "sourcegraph/scip-go".into(),
                    binary_name: "scip-go".into(),
                    version: "v0.1.26".into(),
                    default_args: vec!["index".into(), "--output".into(), "index.scip".into()],
                    output_file: "index.scip".into(),
                    install_method: InstallMethod::GitHubTarGz {
                        asset_pattern: "scip-go_{version}_{os}_{goreleaser_arch}.tar.gz".into(),
                        binary_path_in_archive: None,
                    },
                },
                IndexerEntry {
                    indexer_name: "scip-java".into(),
                    language: "java".into(),
                    github_repo: "sourcegraph/scip-java".into(),
                    binary_name: "scip-java".into(),
                    version: "v0.12.3".into(),
                    default_args: vec!["index".into()],
                    output_file: "index.scip".into(),
                    install_method: InstallMethod::GitHubLauncher {
                        unix_asset: "scip-java-{version}".into(),
                        windows_asset: "scip-java-{version}.bat".into(),
                    },
                },
                IndexerEntry {
                    indexer_name: "scip-java".into(),
                    language: "scala".into(),
                    github_repo: "sourcegraph/scip-java".into(),
                    binary_name: "scip-java".into(),
                    version: "v0.12.3".into(),
                    default_args: vec!["index".into()],
                    output_file: "index.scip".into(),
                    install_method: InstallMethod::GitHubLauncher {
                        unix_asset: "scip-java-{version}".into(),
                        windows_asset: "scip-java-{version}.bat".into(),
                    },
                },
                IndexerEntry {
                    indexer_name: "scip-dotnet".into(),
                    language: "csharp".into(),
                    github_repo: "sourcegraph/scip-dotnet".into(),
                    binary_name: "scip-dotnet".into(),
                    version: "0.2.13".into(),
                    default_args: vec!["index".into()],
                    output_file: "index.scip".into(),
                    install_method: InstallMethod::DotnetTool {
                        package: "scip-dotnet".into(),
                    },
                },
                IndexerEntry {
                    indexer_name: "scip-ruby".into(),
                    language: "ruby".into(),
                    github_repo: "sourcegraph/scip-ruby".into(),
                    binary_name: "scip-ruby".into(),
                    version: "v0.4.7".into(),
                    default_args: vec!["index".into()],
                    output_file: "index.scip".into(),
                    install_method: InstallMethod::GitHubBinary {
                        asset_pattern: "scip-ruby-{arch}-{os}".into(),
                    },
                },
                IndexerEntry {
                    indexer_name: "scip-kotlin".into(),
                    language: "kotlin".into(),
                    github_repo: "sourcegraph/scip-kotlin".into(),
                    binary_name: "scip-kotlin".into(),
                    version: "0.6.0".into(),
                    default_args: vec!["index".into()],
                    output_file: "index.scip".into(),
                    install_method: InstallMethod::Unsupported {
                        reason: "scip-kotlin is a Kotlin compiler plugin invoked through scip-java, not a standalone binary".into(),
                    },
                },
                IndexerEntry {
                    indexer_name: "scip-clang".into(),
                    language: "cpp".into(),
                    github_repo: "sourcegraph/scip-clang".into(),
                    binary_name: "scip-clang".into(),
                    version: "v0.4.0".into(),
                    default_args: vec!["--compdb-path=compile_commands.json".into()],
                    output_file: "index.scip".into(),
                    install_method: InstallMethod::GitHubBinary {
                        asset_pattern: "scip-clang-{arch}-{os}".into(),
                    },
                },
            ],
        }
    }

    /// Get the indexer entry for a detected language.
    pub fn get(&self, lang: &Language) -> Option<&IndexerEntry> {
        self.entries
            .iter()
            .find(|e| e.language == lang.name())
    }

    /// Return all registered indexers.
    pub fn all(&self) -> &[IndexerEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::languages::LanguageKind;

    #[test]
    fn test_registry_has_core_languages() {
        let registry = &*REGISTRY;
        let core_languages = [
            LanguageKind::TypeScript,
            LanguageKind::JavaScript,
            LanguageKind::Python,
            LanguageKind::Rust,
            LanguageKind::Go,
            LanguageKind::Java,
            LanguageKind::CSharp,
            LanguageKind::Ruby,
            LanguageKind::Kotlin,
            LanguageKind::Cpp,
            LanguageKind::Scala,
        ];
        for kind in &core_languages {
            let lang = kind.with_evidence(String::new());
            assert!(
                registry.get(&lang).is_some(),
                "Missing indexer for {:?}",
                kind
            );
        }
    }

    #[test]
    fn test_registry_all_returns_correct_count() {
        let registry = &*REGISTRY;
        let all = registry.all();
        assert_eq!(all.len(), 11, "Expected 11 entries, got {}", all.len());
    }

    #[test]
    fn test_registry_entries_have_valid_fields() {
        let registry = &*REGISTRY;
        for entry in registry.all() {
            assert!(!entry.indexer_name.is_empty(), "Empty indexer_name");
            assert!(!entry.language.is_empty(), "Empty language");
            assert!(!entry.github_repo.is_empty(), "Empty github_repo");
            assert!(!entry.binary_name.is_empty(), "Empty binary_name");
            assert!(!entry.version.is_empty(), "Empty version");
            assert!(!entry.output_file.is_empty(), "Empty output_file");
        }
    }

    #[test]
    fn test_registry_github_repos_have_owner_format() {
        let registry = &*REGISTRY;
        for entry in registry.all() {
            assert!(
                entry.github_repo.contains('/'),
                "github_repo '{}' missing owner/repo format",
                entry.github_repo
            );
        }
    }

    #[test]
    fn test_registry_get_returns_correct_entry() {
        let registry = &*REGISTRY;
        let lang = LanguageKind::Rust.with_evidence(String::new());
        let entry = registry.get(&lang).unwrap();
        assert_eq!(entry.indexer_name, "rust-analyzer");
        assert_eq!(entry.language, "rust");
    }

    #[test]
    fn test_registry_typescript_entry() {
        let registry = &*REGISTRY;
        let lang = LanguageKind::TypeScript.with_evidence(String::new());
        let entry = registry.get(&lang).unwrap();
        assert_eq!(entry.indexer_name, "scip-typescript");
        assert!(matches!(entry.install_method, InstallMethod::Npm { .. }));
    }

    #[test]
    fn test_registry_python_entry() {
        let registry = &*REGISTRY;
        let lang = LanguageKind::Python.with_evidence(String::new());
        let entry = registry.get(&lang).unwrap();
        assert_eq!(entry.indexer_name, "scip-python");
        assert!(matches!(entry.install_method, InstallMethod::Npm { .. }));
    }

    #[test]
    fn test_registry_cpp_entry() {
        let registry = &*REGISTRY;
        let lang = LanguageKind::Cpp.with_evidence(String::new());
        let entry = registry.get(&lang).unwrap();
        assert_eq!(entry.indexer_name, "scip-clang");
        assert!(matches!(entry.install_method, InstallMethod::GitHubBinary { .. }));
    }


    #[test]
    fn test_registry_scala_entry() {
        let registry = &*REGISTRY;
        let lang = LanguageKind::Scala.with_evidence(String::new());
        let entry = registry.get(&lang).unwrap();
        assert_eq!(entry.indexer_name, "scip-java");
        assert!(matches!(entry.install_method, InstallMethod::GitHubLauncher { .. }));
    }
    #[test]
    fn test_registry_kotlin_is_unsupported() {
        let registry = &*REGISTRY;
        let lang = LanguageKind::Kotlin.with_evidence(String::new());
        let entry = registry.get(&lang).unwrap();
        assert!(matches!(entry.install_method, InstallMethod::Unsupported { .. }));
    }

    #[test]
    fn test_registry_install_methods_are_valid() {
        let registry = &*REGISTRY;
        for entry in registry.all() {
            match &entry.install_method {
                InstallMethod::Npm { package } => {
                    assert!(!package.is_empty(), "Empty npm package for {}", entry.indexer_name);
                }
                InstallMethod::DotnetTool { package } => {
                    assert!(!package.is_empty(), "Empty dotnet package for {}", entry.indexer_name);
                }
                InstallMethod::GitHubBinary { asset_pattern }
                | InstallMethod::GitHubGz { asset_pattern }=> {
                    assert!(
                        asset_pattern.contains('{'),
                        "Asset pattern for {} has no placeholders: {}",
                        entry.indexer_name, asset_pattern
                    );
                }
                InstallMethod::GitHubTarGz { asset_pattern, .. } => {
                    assert!(
                        asset_pattern.contains('{'),
                        "Asset pattern for {} has no placeholders: {}",
                        entry.indexer_name, asset_pattern
                    );
                }
                InstallMethod::GitHubZip { asset_pattern, .. } => {
                    assert!(
                        asset_pattern.contains('{'),
                        "Asset pattern for {} has no placeholders: {}",
                        entry.indexer_name, asset_pattern
                    );
                }
                InstallMethod::GitHubLauncher { unix_asset, windows_asset } => {
                    assert!(!unix_asset.is_empty(), "Empty unix_asset for {}", entry.indexer_name);
                    assert!(!windows_asset.is_empty(), "Empty windows_asset for {}", entry.indexer_name);
                }
                InstallMethod::Unsupported { reason } => {
                    assert!(!reason.is_empty(), "Empty reason for {}", entry.indexer_name);
                }
            }
        }
    }

    #[test]
    fn test_registry_all_output_files_are_scip() {
        let registry = &*REGISTRY;
        for entry in registry.all() {
            assert!(
                entry.output_file.ends_with(".scip"),
                "Entry '{}' output_file '{}' does not end with .scip",
                entry.indexer_name,
                entry.output_file
            );
        }
    }

    #[test]
    fn test_registry_all_have_default_args() {
        let registry = &*REGISTRY;
        for entry in registry.all() {
            assert!(
                !entry.default_args.is_empty(),
                "Entry '{}' has no default_args",
                entry.indexer_name
            );
        }
    }

    #[test]
    fn test_registry_unique_language_entries() {
        let registry = &*REGISTRY;
        let all = registry.all();
        let languages: Vec<&str> = all.iter().map(|e| e.language.as_str()).collect();
        let unique: std::collections::HashSet<&&str> = languages.iter().collect();
        assert_eq!(
            languages.len(),
            unique.len(),
            "Registry contains duplicate language entries"
        );
    }
}
